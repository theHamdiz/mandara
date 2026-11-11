//! What the cursor and icon theme installers share: HTTPS downloads with a
//! size cap, progress and cancel (checked against a pinned SHA-256 when
//! the theme comes from a catalog), safe unpacking into a scratch folder,
//! copying a folder someone picked, and tidying an unpacked theme so
//! nothing in it points outside where it was installed.
//!
//! Nothing here runs anything that came with a theme: archives are read
//! with the pure-Rust readers of [`crate::app_install::archive`], which
//! refuse absolute paths and `..`, skip devices, FIFOs and hard links and
//! keep only symlinks that stay inside the unpacked tree.

use std::io::{BufReader, Read, Write as _};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mandara_i18n::{t, tf};
use sha2::Digest as _;

use crate::app_install::archive::{self, ExtractOptions};
use crate::app_install::detect::Format;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// The whole body must arrive within its size at this rate, but never
/// less than [`MIN_BODY_TIMEOUT`] nor more than [`MAX_BODY_TIMEOUT`] (a
/// large theme on a slow link still finishes; Cancel stops it sooner).
const SLOWEST_BYTES_PER_SEC: u64 = 8 << 10;
const MIN_BODY_TIMEOUT: Duration = Duration::from_secs(15 * 60);
pub const MAX_BODY_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// Download / hash chunk (progress and cancel granularity).
pub const CHUNK: usize = 64 << 10;
/// Longest URL accepted.
const MAX_URL_LEN: usize = 2048;
/// Longest theme folder name accepted.
const MAX_NAME_LEN: usize = 255;
const BZIP2_MAGIC: &[u8] = b"BZh";
const ZIP_MAGIC: &[u8] = b"PK\x03\x04";
const GZIP_MAGIC: &[u8] = b"\x1f\x8b";
const XZ_MAGIC: &[u8] = b"\xfd7zXZ\x00";
const ZSTD_MAGIC: &[u8] = b"\x28\xb5\x2f\xfd";
const FILE_MODE: u32 = 0o644;
const DIR_MODE: u32 = 0o755;

/// A catalog download's expected checksum and size.
#[derive(Copy, Clone, Debug)]
pub struct Pin<'a> {
    /// Lower-case hex SHA-256.
    pub sha256: &'a str,
    pub size: u64,
}

/// A finished download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// SHA-256 of a file, lower-case hex.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(hex(&hasher.finalize()));
        }
        hasher.update(&buf[..n]);
    }
}

/// `Err` once `cancel` is set.
pub fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) { Err(t("Cancelled").to_string()) } else { Ok(()) }
}

/// Checks that `url` is a plain `https://` address with a host.
pub fn require_https(url: &str) -> Result<(), String> {
    let rest = url.strip_prefix("https://").ok_or_else(|| t("Only https:// downloads are allowed").to_string())?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let bad = url.len() > MAX_URL_LEN || host.is_empty() || host.contains('@') || url.chars().any(|c| c.is_whitespace() || c.is_control());
    if bad { Err(t("That isn't a valid https:// address").to_string()) } else { Ok(()) }
}

/// A theme folder name: one path component that isn't hidden.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME_LEN && !name.starts_with('.') && !name.contains(['/', '\0', '\\']) && name.trim() == name
}

/// Downloads `url` into `cache`. With a `pin` the file is kept only when
/// its size and SHA-256 match (and an earlier verified copy is reused);
/// without one, it is cut off past `max` bytes and its SHA-256 is only
/// reported. `progress(done, total)` follows the bytes (`total` is 0 when
/// the server doesn't say).
pub fn fetch(url: &str, pin: Option<Pin<'_>>, max: u64, cache: &Path, progress: &dyn Fn(u64, u64), cancel: &AtomicBool) -> Result<Fetched, String> {
    require_https(url)?;
    if let Some(pin) = pin {
        let file = cache.join(pin.sha256);
        if file.is_file() && sha256_file(&file).is_ok_and(|h| h == pin.sha256) {
            return Ok(Fetched { path: file, sha256: pin.sha256.to_string(), size: pin.size });
        }
    }
    crate::offline::check()?;
    std::fs::create_dir_all(cache).map_err(|e| format!("{}: {e}", cache.display()))?;
    let agent: ureq::Agent = ureq::Agent::config_builder().https_only(true).timeout_connect(Some(CONNECT_TIMEOUT)).timeout_recv_body(Some(body_timeout(pin.map(|p| p.size)))).build().into();
    let mut response = agent.get(url).header("User-Agent", concat!("mandara-shell/", env!("CARGO_PKG_VERSION"))).call().map_err(|e| tf("Download failed: {error}", &[("error", &e)]))?;
    let limit = pin.map_or(max, |p| p.size.min(max));
    let announced = response.body().content_length().unwrap_or(0);
    if announced > limit {
        return Err(t("The download is larger than allowed").to_string());
    }
    let total = pin.map_or(announced, |p| p.size);
    let mut reader = response.body_mut().with_config().limit(limit + 1).reader();
    clean_stale(cache, DOWNLOAD_PREFIX);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let part = cache.join(format!("{DOWNLOAD_PREFIX}{}-{nanos}.part", std::process::id()));
    let result = (|| {
        let mut out = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
        let mut hasher = sha2::Sha256::new();
        let mut buf = vec![0u8; CHUNK];
        let mut got = 0u64;
        loop {
            cancelled(cancel)?;
            let n = reader.read(&mut buf).map_err(|e| tf("Download failed: {error}", &[("error", &e)]))?;
            if n == 0 {
                break;
            }
            got += n as u64;
            if got > limit {
                return Err(t("The download is larger than allowed").to_string());
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            progress(got, total);
        }
        out.sync_all().map_err(|e| e.to_string())?;
        let sha256 = hex(&hasher.finalize());
        if let Some(pin) = pin
            && (got != pin.size || sha256 != pin.sha256)
        {
            return Err(t("The download doesn't match its pinned checksum, so it wasn't installed").to_string());
        }
        if got == 0 {
            return Err(t("The download is empty").to_string());
        }
        let file = cache.join(&sha256);
        std::fs::rename(&part, &file).map_err(|e| e.to_string())?;
        Ok(Fetched { path: file, sha256, size: got })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// How long the body of a download of `expected` bytes may take (the
/// longest when the size is unknown).
fn body_timeout(expected: Option<u64>) -> Duration {
    expected.map_or(MAX_BODY_TIMEOUT, |bytes| Duration::from_secs(bytes / SLOWEST_BYTES_PER_SEC).clamp(MIN_BODY_TIMEOUT, MAX_BODY_TIMEOUT))
}

/// Removes a scratch folder when dropped (success or failure).
pub struct Scratch(pub PathBuf);

impl Scratch {
    /// A new, not yet created scratch path in `dir` named `<prefix><pid>-<nanos>`.
    /// Leftovers of interrupted installs there go first ([`clean_stale`]).
    pub fn in_dir(dir: &Path, prefix: &str) -> Self {
        clean_stale(dir, prefix);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        Self(dir.join(format!("{prefix}{}-{nanos}", std::process::id())))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Removes leftover scratch folders named `<prefix>…` from `dir`.
pub fn clean_scratch(dir: &Path, prefix: &str) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with(prefix) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// How old a leftover scratch folder or partial download must be before an
/// install removes it (no install runs this long, so it isn't one in
/// progress in Studio or the shell).
const STALE_AFTER: Duration = Duration::from_secs(6 * 60 * 60);
/// Prefix of partial downloads in a cache folder.
pub const DOWNLOAD_PREFIX: &str = ".download-";

/// Removes leftovers named `<prefix>…` from `dir` that an interrupted
/// install left behind more than [`STALE_AFTER`] ago.
pub fn clean_stale(dir: &Path, prefix: &str) {
    let now = std::time::SystemTime::now();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| now.duration_since(t).ok()).is_some_and(|age| age > STALE_AFTER);
        if old && e.file_name().to_string_lossy().starts_with(prefix) {
            let path = e.path();
            let _ = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        }
    }
}

/// Whether `path` starts like an archive [`unpack`] reads.
pub fn is_archive(path: &Path) -> bool {
    let mut head = [0u8; 6];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    let head = &head[..n];
    [ZIP_MAGIC, BZIP2_MAGIC, GZIP_MAGIC, XZ_MAGIC, ZSTD_MAGIC].iter().any(|m| head.starts_with(m))
}

/// Unpacks `archive_path` (zip, or a tar compressed with gzip, xz, zstd or
/// bzip2, picked by its first bytes) into `dest` within `opts`.
pub fn unpack(archive_path: &Path, dest: &Path, opts: &ExtractOptions<'_>, cancel: &AtomicBool, on_bytes: archive::OnBytes<'_>) -> Result<u64, String> {
    let mut head = [0u8; 6];
    let n = std::fs::File::open(archive_path).and_then(|mut f| f.read(&mut head)).map_err(|e| e.to_string())?;
    let head = &head[..n];
    if head.starts_with(ZIP_MAGIC) {
        archive::extract_with(archive_path, Format::Zip, dest, opts, cancel, on_bytes)
    } else if head.starts_with(BZIP2_MAGIC) {
        let file = std::fs::File::open(archive_path).map_err(|e| e.to_string())?;
        archive::extract_tar_with(Box::new(bzip2::read::MultiBzDecoder::new(BufReader::new(file))), dest, opts, cancel, on_bytes)
    } else if [GZIP_MAGIC, XZ_MAGIC, ZSTD_MAGIC].iter().any(|m| head.starts_with(m)) {
        archive::extract_with(archive_path, Format::TarGz, dest, opts, cancel, on_bytes)
    } else {
        Err(t("That isn't an archive Mandara can read (.tar.gz, .tar.xz, .tar.zst, .tar.bz2 or .zip)").to_string())
    }
}

/// Copies the folder `from` to the new folder `to` within `opts` (its
/// `keep` filter is ignored): regular files and folders, and symlinks
/// that stay inside `from`. Everything else is skipped. Returns bytes
/// copied.
pub fn copy_tree(from: &Path, to: &Path, opts: &ExtractOptions<'_>, cancel: &AtomicBool, on_bytes: archive::OnBytes<'_>) -> Result<u64, String> {
    if !std::fs::symlink_metadata(from).is_ok_and(|m| m.is_dir()) {
        return Err(tf("{path} isn't a folder", &[("path", &from.display())]));
    }
    let (mut bytes, mut entries, mut written) = (0u64, 0usize, 0u64);
    let mut stack = vec![PathBuf::new()];
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    while let Some(rel) = stack.pop() {
        cancelled(cancel)?;
        let dir = from.join(&rel);
        let list = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for e in list.flatten() {
            let child = rel.join(e.file_name());
            let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
            entries += 1;
            bytes = bytes.saturating_add(if meta.is_file() { meta.len() } else { 0 });
            if entries > opts.max_entries {
                return Err(t("The archive has too many files").to_string());
            }
            if bytes > opts.max_bytes {
                return Err(t("The archive is too large to install").to_string());
            }
            let out = to.join(&child);
            let kind = meta.file_type();
            if kind.is_dir() {
                std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
                stack.push(child);
            } else if kind.is_symlink() {
                if let Ok(target) = std::fs::read_link(e.path())
                    && archive::link_stays_inside(&child, &target)
                {
                    std::os::unix::fs::symlink(&target, &out).map_err(|e| e.to_string())?;
                }
            } else if kind.is_file() {
                let mut src = std::fs::File::open(e.path()).map_err(|e| e.to_string())?;
                archive::copy_stream(&mut src, &out, FILE_MODE, cancel, on_bytes, &mut written)?;
            }
        }
    }
    Ok(written)
}

/// `rel` with `.` and `..` folded away lexically; `None` when it climbs
/// above its start.
pub fn normalize(rel: &Path) -> Option<PathBuf> {
    let mut out: Vec<&std::ffi::OsStr> = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(n) => out.push(n),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop()?;
            }
            _ => return None,
        }
    }
    Some(out.iter().collect())
}

/// What [`tidy_theme`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tidied {
    /// Symlinks removed for pointing outside the allowed folders.
    pub links_dropped: usize,
    /// Top-level files and folders removed (scripts, repository files).
    pub extras_dropped: usize,
    /// Sibling folders the theme's symlinks point into.
    pub link_deps: Vec<String>,
}

/// Licence and credit files kept at a theme's top level.
fn credit_file(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    ["LICENSE", "LICENCE", "COPYING", "AUTHORS", "CREDITS", "COPYRIGHT"].iter().any(|k| upper.starts_with(k))
}

/// Tidies the theme folder `parent/name` in place:
///
/// * a symlink stays only when it resolves (lexically) inside `name` or
///   one of the `siblings` folders next to it; the rest are removed;
/// * file permissions become `0644` (no executable bits) and folders
///   `0755`;
/// * at the top level only `keep_dirs`, folders a kept symlink points
///   into, `index.theme`, `icon-theme.cache` and licence / credit files
///   stay (with `keep_dirs` empty every top-level folder stays); scripts
///   and repository files go.
pub fn tidy_theme(parent: &Path, name: &str, siblings: &[&str], keep_dirs: &[String], cancel: &AtomicBool) -> Result<Tidied, String> {
    use std::os::unix::fs::PermissionsExt as _;
    let root = parent.join(name);
    let mut out = Tidied::default();
    let mut linked_top: Vec<String> = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        cancelled(cancel)?;
        let dir = root.join(&rel);
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(DIR_MODE));
        let Ok(list) = std::fs::read_dir(&dir) else { continue };
        for e in list.flatten() {
            let child = rel.join(e.file_name());
            let path = e.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            let kind = meta.file_type();
            if kind.is_symlink() {
                let target = std::fs::read_link(&path).unwrap_or_default();
                // Resolved against the parent folder: `name/child/..target`.
                let resolved = if target.is_absolute() { None } else { normalize(&Path::new(name).join(child.parent().unwrap_or(Path::new(""))).join(&target)) };
                let first = resolved.as_ref().and_then(|r| r.components().next()).map(|c| c.as_os_str().to_string_lossy().into_owned());
                match first {
                    Some(f) if f == name => {
                        if let Some(top) = resolved.as_ref().and_then(|r| r.components().nth(1)) {
                            let top = top.as_os_str().to_string_lossy().into_owned();
                            if !linked_top.contains(&top) {
                                linked_top.push(top);
                            }
                        }
                    }
                    Some(f) if siblings.contains(&f.as_str()) => {
                        if !out.link_deps.contains(&f) {
                            out.link_deps.push(f);
                        }
                    }
                    _ => {
                        std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                        out.links_dropped += 1;
                    }
                }
            } else if kind.is_dir() {
                stack.push(child);
            } else if kind.is_file() {
                if meta.permissions().mode() & 0o7777 != FILE_MODE {
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(FILE_MODE));
                }
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    for e in std::fs::read_dir(&root).map_err(|e| e.to_string())?.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
        let keep = if n.starts_with('.') {
            false
        } else if meta.is_dir() || (meta.file_type().is_symlink() && e.path().is_dir()) {
            keep_dirs.is_empty() || keep_dirs.contains(&n) || linked_top.contains(&n)
        } else {
            n == "index.theme" || n == "icon-theme.cache" || credit_file(&n) || (meta.file_type().is_symlink() && (keep_dirs.contains(&n) || linked_top.contains(&n)))
        };
        if !keep {
            let removed = if meta.is_dir() { std::fs::remove_dir_all(e.path()) } else { std::fs::remove_file(e.path()) };
            removed.map_err(|err| format!("{}: {err}", e.path().display()))?;
            out.extras_dropped += 1;
        }
    }
    Ok(out)
}

/// Merges the folder `from` into `into` (created when missing): entries
/// move across, a later layer's file replacing an earlier one's. Both must
/// be on one file system (scratch folders sit beside the target).
pub fn merge_into(from: &Path, into: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(into).is_err() {
        if let Some(parent) = into.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        return std::fs::rename(from, into).map_err(|e| format!("{}: {e}", into.display()));
    }
    for e in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))?.flatten() {
        let src = e.path();
        let dst = into.join(e.file_name());
        let src_dir = std::fs::symlink_metadata(&src).is_ok_and(|m| m.is_dir());
        let dst_meta = std::fs::symlink_metadata(&dst).ok();
        match dst_meta {
            Some(m) if m.is_dir() && src_dir => merge_into(&src, &dst)?,
            Some(m) => {
                if m.is_dir() {
                    std::fs::remove_dir_all(&dst).map_err(|e| e.to_string())?;
                } else {
                    std::fs::remove_file(&dst).map_err(|e| e.to_string())?;
                }
                std::fs::rename(&src, &dst).map_err(|e| e.to_string())?;
            }
            None => std::fs::rename(&src, &dst).map_err(|e| e.to_string())?,
        }
    }
    Ok(())
}

/// Folders under `root` (to `depth` levels, `root` included) for which
/// `is_theme` holds, shallowest first. A theme's own subfolders are not
/// searched.
pub fn find_dirs(root: &Path, depth: usize, is_theme: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut level = vec![root.to_path_buf()];
    for _ in 0..=depth {
        let mut next = Vec::new();
        for dir in level {
            if is_theme(&dir) {
                found.push(dir);
                continue;
            }
            let Ok(list) = std::fs::read_dir(&dir) else { continue };
            let mut subs: Vec<PathBuf> = list.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| !n.to_string_lossy().starts_with('.') && n != "__MACOSX")).collect();
            subs.sort();
            next.extend(subs);
        }
        level = next;
    }
    found
}

/// Runs `gtk-update-icon-cache` on `dir` when it is installed (never
/// required: themes work without a cache). Returns whether it succeeded.
pub fn update_icon_cache(dir: &Path) -> bool {
    std::process::Command::new("gtk-update-icon-cache")
        .args(["--quiet", "--force", "--ignore-theme-index"])
        .arg(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_install::archive::tests::tar_gz;

    static NO_CANCEL: AtomicBool = AtomicBool::new(false);

    #[test]
    fn only_plain_https_urls_pass() {
        for ok in ["https://github.com/a/b/archive/refs/tags/1.tar.gz", "https://example.org/x.zip?dl=1"] {
            assert!(require_https(ok).is_ok(), "{ok}");
        }
        for bad in ["http://github.com/a.tar.gz", "ftp://x/y", "file:///etc/passwd", "https://", "https:///x", "https://user@host/x", "https://a b/c", "github.com/x.tar.gz"] {
            assert!(require_https(bad).is_err(), "{bad}");
        }
        assert!(require_https(&format!("https://h/{}", "a".repeat(MAX_URL_LEN))).is_err());
        let blocked = fetch("http://example.org/a.tar.gz", None, 10, Path::new("/nonexistent"), &|_, _| {}, &NO_CANCEL);
        assert!(blocked.is_err());
    }

    #[test]
    fn body_timeouts_scale_with_size_within_bounds() {
        assert_eq!(body_timeout(None), MAX_BODY_TIMEOUT);
        assert_eq!(body_timeout(Some(1)), MIN_BODY_TIMEOUT);
        assert_eq!(body_timeout(Some(10 << 30)), MAX_BODY_TIMEOUT);
        let mid = body_timeout(Some(10 << 20));
        assert!(mid > MIN_BODY_TIMEOUT && mid < MAX_BODY_TIMEOUT, "{mid:?}");
    }

    #[test]
    fn only_stale_leftovers_are_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join(".download-1-1.part");
        let old = dir.path().join(".download-1-2.part");
        let keep = dir.path().join("abc");
        for p in [&fresh, &old, &keep] {
            std::fs::write(p, b"x").unwrap();
        }
        let long_ago = std::time::SystemTime::now() - STALE_AFTER - Duration::from_secs(60);
        std::fs::File::options().write(true).open(&old).unwrap().set_modified(long_ago).unwrap();
        std::fs::File::options().write(true).open(&keep).unwrap().set_modified(long_ago).unwrap();
        clean_stale(dir.path(), DOWNLOAD_PREFIX);
        assert!(fresh.exists(), "an install in progress keeps its file");
        assert!(!old.exists());
        assert!(keep.exists(), "only its own prefix");
    }

    #[test]
    fn names_are_one_visible_component() {
        assert!(valid_name("Papirus-Dark") && valid_name("Suru++") && valid_name("We10X"));
        for bad in ["", ".hidden", "a/b", "..", "x\0", " pad", "a\\b"] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn normalize_folds_and_refuses_climbing() {
        assert_eq!(normalize(Path::new("a/./b/../c")), Some(PathBuf::from("a/c")));
        assert_eq!(normalize(Path::new("a/../../b")), None);
        assert_eq!(normalize(Path::new("/etc")), None);
    }

    #[test]
    fn cached_pinned_downloads_are_reused() {
        let dir = tempfile::tempdir().unwrap();
        let data = b"theme bytes";
        let sha = hex(&sha2::Sha256::digest(data));
        std::fs::write(dir.path().join(&sha), data).unwrap();
        let got = fetch("https://invalid.invalid/never", Some(Pin { sha256: &sha, size: data.len() as u64 }), 1 << 20, dir.path(), &|_, _| {}, &NO_CANCEL).unwrap();
        assert_eq!(got.sha256, sha);
    }

    #[test]
    fn unpack_refuses_unknown_formats_and_filters_entries() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("x.bin");
        std::fs::write(&junk, b"not an archive").unwrap();
        assert!(!is_archive(&junk));
        assert!(unpack(&junk, &dir.path().join("o"), &ExtractOptions::APPS, &NO_CANCEL, &|_| {}).is_err());
        let a = dir.path().join("a.tar.gz");
        std::fs::write(&a, tar_gz(&[("top/keep/x", b"1", 0o644), ("top/skip/y", b"2", 0o644)], &[])).unwrap();
        assert!(is_archive(&a));
        let keep = |p: &Path| p.starts_with("top/keep");
        unpack(&a, &dir.path().join("o"), &ExtractOptions { keep: Some(&keep), ..ExtractOptions::APPS }, &NO_CANCEL, &|_| {}).unwrap();
        assert!(dir.path().join("o/top/keep/x").is_file());
        assert!(!dir.path().join("o/top/skip").exists());
        // Limits count only what is kept.
        let tiny = ExtractOptions { max_bytes: 0, ..ExtractOptions::APPS };
        assert!(unpack(&a, &dir.path().join("o2"), &tiny, &NO_CANCEL, &|_| {}).is_err());
        let few = ExtractOptions { max_entries: 1, ..ExtractOptions::APPS };
        assert!(unpack(&a, &dir.path().join("o3"), &few, &NO_CANCEL, &|_| {}).is_err());
    }

    #[test]
    fn tidy_drops_escaping_links_scripts_and_exec_bits() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("T");
        std::fs::create_dir_all(root.join("48/apps")).unwrap();
        std::fs::create_dir_all(root.join("src/apps")).unwrap();
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::create_dir_all(root.join(".github")).unwrap();
        std::fs::create_dir_all(dir.path().join("Base/48/apps")).unwrap();
        std::fs::write(root.join("index.theme"), "[Icon Theme]\n").unwrap();
        std::fs::write(root.join("install.sh"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(root.join("install.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(root.join("COPYING"), "GPL").unwrap();
        std::fs::write(root.join("src/apps/a.svg"), "<svg/>").unwrap();
        std::fs::write(root.join("48/apps/x.svg"), "<svg/>").unwrap();
        std::fs::set_permissions(root.join("48/apps/x.svg"), std::fs::Permissions::from_mode(0o4755)).unwrap();
        let link = |at: &str, to: &str| std::os::unix::fs::symlink(to, root.join(at)).unwrap();
        link("48/apps/in.svg", "x.svg");
        link("48/apps/src.svg", "../../src/apps/a.svg");
        link("48/apps/base.svg", "../../../Base/48/apps/b.svg");
        link("48/apps/out.svg", "../../../Other/48/apps/b.svg");
        link("48/apps/abs.svg", "/etc/passwd");
        link("48/apps/up.svg", "../../../../etc/passwd");
        let done = tidy_theme(dir.path(), "T", &["Base"], &["48".into()], &NO_CANCEL).unwrap();
        assert_eq!(done.links_dropped, 3);
        assert_eq!(done.link_deps, vec!["Base".to_string()]);
        for kept in ["48/apps/in.svg", "48/apps/src.svg", "48/apps/base.svg", "index.theme", "COPYING"] {
            assert!(std::fs::symlink_metadata(root.join(kept)).is_ok(), "{kept}");
        }
        for gone in ["48/apps/out.svg", "48/apps/abs.svg", "48/apps/up.svg", "install.sh", "scripts", ".github"] {
            assert!(std::fs::symlink_metadata(root.join(gone)).is_err(), "{gone}");
        }
        assert!(root.join("src/apps/a.svg").is_file(), "a folder a kept link points into stays");
        assert_eq!(std::fs::metadata(root.join("48/apps/x.svg")).unwrap().permissions().mode() & 0o7777, 0o644);
    }

    #[test]
    fn merge_layers_later_wins() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b, out) = (dir.path().join("a"), dir.path().join("b"), dir.path().join("out"));
        std::fs::create_dir_all(a.join("16/apps")).unwrap();
        std::fs::create_dir_all(b.join("16/apps")).unwrap();
        std::fs::write(a.join("16/apps/x.svg"), "a").unwrap();
        std::fs::write(a.join("16/apps/y.svg"), "a").unwrap();
        std::fs::write(b.join("16/apps/y.svg"), "b").unwrap();
        std::os::unix::fs::symlink("x.svg", b.join("16/apps/z.svg")).unwrap();
        merge_into(&a, &out).unwrap();
        merge_into(&b, &out).unwrap();
        assert_eq!(std::fs::read_to_string(out.join("16/apps/x.svg")).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(out.join("16/apps/y.svg")).unwrap(), "b");
        assert_eq!(std::fs::read_to_string(out.join("16/apps/z.svg")).unwrap(), "a");
    }

    #[test]
    fn copy_tree_keeps_inside_links_and_caps_size() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("T/48")).unwrap();
        std::fs::write(src.join("T/48/a.svg"), "<svg/>").unwrap();
        std::os::unix::fs::symlink("a.svg", src.join("T/48/b.svg")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", src.join("T/48/c.svg")).unwrap();
        std::os::unix::fs::symlink("../../../outside", src.join("T/48/d.svg")).unwrap();
        let out = dir.path().join("out");
        copy_tree(&src, &out, &ExtractOptions::APPS, &NO_CANCEL, &|_| {}).unwrap();
        assert!(out.join("T/48/a.svg").is_file() && out.join("T/48/b.svg").is_symlink());
        assert!(std::fs::symlink_metadata(out.join("T/48/c.svg")).is_err() && std::fs::symlink_metadata(out.join("T/48/d.svg")).is_err());
        let few = ExtractOptions { max_entries: 2, ..ExtractOptions::APPS };
        assert!(copy_tree(&src, &dir.path().join("out2"), &few, &NO_CANCEL, &|_| {}).is_err());
    }

    #[test]
    fn find_dirs_stops_at_themes() {
        let dir = tempfile::tempdir().unwrap();
        for d in ["pkg/A", "pkg/A/inner", "pkg/B", "other/deep/deeper/C"] {
            std::fs::create_dir_all(dir.path().join(d)).unwrap();
        }
        for d in ["pkg/A", "pkg/A/inner", "pkg/B", "other/deep/deeper/C"] {
            std::fs::write(dir.path().join(d).join("index.theme"), "").unwrap();
        }
        let found = find_dirs(dir.path(), 3, &|p| p.join("index.theme").is_file());
        let names: Vec<_> = found.iter().map(|p| p.strip_prefix(dir.path()).unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["pkg/A", "pkg/B"]);
    }
}
