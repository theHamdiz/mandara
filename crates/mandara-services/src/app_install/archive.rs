//! Portable app archives (`.tar.gz` / `.tar.xz` / `.tar.zst` / `.zip`):
//! pure-Rust decoding, safe extraction and finding the app inside.
//!
//! Extraction refuses absolute paths and `..`, keeps only symlinks whose
//! target stays inside the extracted tree, skips hard links, devices and
//! FIFOs, strips setuid / setgid bits, and caps the total size and entry
//! count. Bytes are reported as they are written; a cancel flag stops it
//! between chunks.

use std::io::{BufRead, BufReader, Read, Write as _};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use mandara_i18n::{t, tf};

use super::detect::{self, Format};

/// Largest total an archive may unpack to.
pub const MAX_EXTRACT_BYTES: u64 = 16 << 30;
/// Most entries an archive may hold.
const MAX_ENTRIES: usize = 400_000;
/// Copy chunk (progress and cancel granularity).
pub const CHUNK: usize = 1 << 20;
/// Permission bits kept from archives (no setuid / setgid / sticky).
const MODE_MASK: u32 = 0o777;
const DIR_MODE: u32 = 0o755;
/// Directories searched for the main program / icon / entry, from the root.
const SCAN_DEPTH: usize = 4;
/// Icons in these folders are preferred.
const ICON_HINTS: &[&str] = &["icon", "logo", "pixmaps", "icons"];
const ELF_MAGIC: &[u8] = b"\x7fELF";
const GZIP_MAGIC: &[u8] = b"\x1f\x8b";
const XZ_MAGIC: &[u8] = b"\xfd7zXZ\x00";
const ZSTD_MAGIC: &[u8] = b"\x28\xb5\x2f\xfd";

/// A streaming reader over a compressed tar.
pub fn tar_reader(path: &Path, format: Format) -> Result<Box<dyn Read + Send>, String> {
    if !matches!(format, Format::TarGz | Format::TarXz | Format::TarZst | Format::Pacman | Format::Xbps | Format::Apk) {
        return Err(t("Not a tar archive").to_string());
    }
    // Packages share containers: the magic picks the decoder.
    let head = detect::head(path).map_err(|e| e.to_string())?;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let buffered = BufReader::new(file);
    Ok(if head.starts_with(GZIP_MAGIC) {
        Box::new(flate2::read::MultiGzDecoder::new(buffered))
    } else if head.starts_with(ZSTD_MAGIC) {
        Box::new(ZstdFrames::new(buffered))
    } else if head.starts_with(XZ_MAGIC) {
        xz_reader(buffered)
    } else {
        return Err(t("Not a tar archive").to_string());
    })
}

/// lzma-rs decodes into a writer: run it on a thread feeding a pipe.
/// Dropping the reader ends the thread (its next write fails).
fn xz_reader<R: BufRead + Send + 'static>(mut input: R) -> Box<dyn Read + Send> {
    match std::io::pipe() {
        Ok((reader, mut writer)) => {
            let _ = std::thread::Builder::new().name("mandara-xz".into()).spawn(move || {
                let _ = lzma_rs::xz_decompress(&mut input, &mut writer);
            });
            Box::new(reader)
        }
        Err(e) => Box::new(FailingReader(e.to_string())),
    }
}

struct FailingReader(String);

impl Read for FailingReader {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other(self.0.clone()))
    }
}

/// Reads concatenated zstd frames (multi-threaded `zstd -T0` writes many).
pub struct ZstdFrames<R: BufRead> {
    decoder: Option<ruzstd::decoding::StreamingDecoder<R, ruzstd::decoding::FrameDecoder>>,
    source: Option<R>,
}

impl<R: BufRead> ZstdFrames<R> {
    pub fn new(source: R) -> Self {
        Self { decoder: None, source: Some(source) }
    }
}

impl<R: BufRead> Read for ZstdFrames<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if let Some(d) = self.decoder.as_mut() {
                let n = d.read(buf)?;
                if n > 0 || buf.is_empty() {
                    return Ok(n);
                }
                self.source = self.decoder.take().map(|d| d.into_inner());
            }
            let Some(mut source) = self.source.take() else { return Ok(0) };
            if source.fill_buf()?.is_empty() {
                return Ok(0);
            }
            self.decoder = Some(ruzstd::decoding::StreamingDecoder::new(source).map_err(std::io::Error::other)?);
        }
    }
}

/// Names of the first `limit` entries of a tar (for telling packages
/// apart), without extracting anything.
pub fn first_entries(path: &Path, format: Format, limit: usize) -> Vec<String> {
    let Ok(reader) = tar_reader(path, format) else { return Vec::new() };
    let mut archive = tar::Archive::new(reader);
    let Ok(entries) = archive.entries() else { return Vec::new() };
    entries.take(limit).filter_map(|e| e.ok()).filter_map(|e| e.path().ok().map(|p| p.to_string_lossy().trim_start_matches("./").to_string())).collect()
}

/// What a pass over an archive found before anything is extracted.
#[derive(Clone, Debug, Default)]
pub struct Peek {
    /// Unpacked size (the extraction's progress total).
    pub total: u64,
    pub entries: usize,
    /// The shallowest `.desktop` file: path and text.
    pub desktop: Option<(PathBuf, String)>,
    /// The best icon candidate seen: path and bytes.
    pub icon: Option<(PathBuf, Vec<u8>)>,
    /// Architecture of the first executable ELF near the top.
    pub arch: Option<detect::Arch>,
}

/// Depth (path components) up to which programs are sniffed for an arch.
const PEEK_PROGRAM_DEPTH: usize = 3;
const PEEK_DESKTOP_BYTES: u64 = 256 << 10;

fn icon_score(path: &Path, hint: &str) -> Option<(bool, bool, usize)> {
    let lower = path.to_string_lossy().to_lowercase();
    let ext_ok = lower.ends_with(".png") || lower.ends_with(".svg");
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let named = !hint.is_empty() && stem.contains(hint);
    let hinted = ICON_HINTS.iter().any(|h| lower.contains(h));
    (ext_ok && (named || hinted)).then(|| (named, lower.ends_with(".svg"), usize::MAX - path.components().count()))
}

/// One streaming pass: total size, entry, icon, program architecture.
/// Reads only small files' contents; stops at [`MAX_ENTRIES`].
pub fn peek(path: &Path, format: Format, hint: &str) -> Result<Peek, String> {
    let hint = hint.to_lowercase();
    let mut out = Peek::default();
    let mut best_icon: Option<(bool, bool, usize)> = None;
    let mut consider = |rel: &Path, size: u64, mode: u32, read: &mut dyn Read, out: &mut Peek| -> Result<(), String> {
        out.total += size;
        out.entries += 1;
        if out.entries > MAX_ENTRIES || out.total > MAX_EXTRACT_BYTES {
            return Err(t("The archive is too large to install").to_string());
        }
        let depth = rel.components().count();
        if rel.extension().is_some_and(|e| e == "desktop") && size <= PEEK_DESKTOP_BYTES && out.desktop.as_ref().is_none_or(|(p, _)| p.components().count() > depth) {
            let mut text = String::new();
            read.take(PEEK_DESKTOP_BYTES).read_to_string(&mut text).map_err(|e| e.to_string())?;
            out.desktop = Some((rel.to_path_buf(), text));
        } else if let Some(score) = icon_score(rel, &hint).filter(|_| size <= super::appimage::MAX_ICON_BYTES)
            && best_icon.is_none_or(|b| score > b)
        {
            let mut bytes = Vec::new();
            read.take(super::appimage::MAX_ICON_BYTES).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            best_icon = Some(score);
            out.icon = Some((rel.to_path_buf(), bytes));
        } else if out.arch.is_none() && mode & 0o111 != 0 && depth <= PEEK_PROGRAM_DEPTH && !rel.to_string_lossy().contains(".so") {
            let mut head = vec![0u8; detect::SNIFF_BYTES];
            let n = read.read(&mut head).unwrap_or(0);
            out.arch = detect::elf_info(&head[..n]).map(|e| e.arch);
        }
        Ok(())
    };
    if format == Format::Zip {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipArchive::new(BufReader::new(file)).map_err(|e| e.to_string())?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
            if entry.is_dir() {
                continue;
            }
            let Some(rel) = entry.enclosed_name() else { continue };
            let (size, mode) = (entry.size(), entry.unix_mode().unwrap_or(0o644));
            consider(&rel, size, mode, &mut entry, &mut out)?;
        }
        return Ok(out);
    }
    let mut archive = tar::Archive::new(tar_reader(path, format)?);
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let rel = entry.path().map_err(|e| e.to_string())?.into_owned();
        let (size, mode) = (entry.header().size().unwrap_or(0), entry.header().mode().unwrap_or(0o644));
        consider(&rel, size, mode, &mut entry, &mut out)?;
    }
    Ok(out)
}

/// The bytes of tar member `name` (at most `limit`).
pub fn read_member(reader: Box<dyn Read + Send>, name: &str, limit: u64) -> Option<Vec<u8>> {
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries().ok()? {
        let mut entry = entry.ok()?;
        let path = entry.path().ok()?.to_string_lossy().trim_start_matches("./").to_string();
        if path == name {
            let mut out = Vec::new();
            (&mut entry).take(limit).read_to_end(&mut out).ok()?;
            return Some(out);
        }
    }
    None
}

/// Whether `rel` is a plain relative path (no root, no `..`).
fn safe_relative(rel: &Path) -> bool {
    !rel.as_os_str().is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// Whether a symlink at `at` (relative to the root) pointing to `target`
/// stays inside the root.
fn link_stays_inside(at: &Path, target: &Path) -> bool {
    if target.is_absolute() {
        return false;
    }
    let mut depth: i64 = at.parent().map(|p| p.components().filter(|c| matches!(c, Component::Normal(_))).count() as i64).unwrap_or(0);
    for c in target.components() {
        match c {
            Component::Normal(_) => depth += 1,
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            Component::CurDir => {}
            _ => return false,
        }
    }
    true
}

/// Progress of an extraction: bytes written so far.
pub type OnBytes<'a> = &'a dyn Fn(u64);

struct Limits {
    total: u64,
    entries: usize,
}

impl Limits {
    fn add(&mut self, bytes: u64) -> Result<(), String> {
        self.total += bytes;
        self.entries += 1;
        if self.total > MAX_EXTRACT_BYTES {
            return Err(t("The archive is too large to install").to_string());
        }
        if self.entries > MAX_ENTRIES {
            return Err(t("The archive has too many files").to_string());
        }
        Ok(())
    }
}

/// Copies `from` into a new file at `dest` in [`CHUNK`]s.
pub fn copy_stream(from: &mut dyn Read, dest: &Path, mode: u32, cancel: &AtomicBool, on_bytes: OnBytes<'_>, written: &mut u64) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut out = std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(dest).map_err(|e| tf("Couldn't write {path}: {error}", &[("path", &dest.display()), ("error", &e)]))?;
    let mut buf = vec![0u8; CHUNK];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(t("Cancelled").to_string());
        }
        let n = from.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        *written += n as u64;
        on_bytes(*written);
    }
    out.sync_all().map_err(|e| e.to_string())
}

fn ensure_parent(dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Extracts `path` into the empty folder `dest`. Returns bytes written.
pub fn extract(path: &Path, format: Format, dest: &Path, cancel: &AtomicBool, on_bytes: OnBytes<'_>) -> Result<u64, String> {
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let mut limits = Limits { total: 0, entries: 0 };
    let mut written = 0u64;
    let unsafe_path = || t("The archive contains unsafe paths").to_string();
    if format == Format::Zip {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipArchive::new(BufReader::new(file)).map_err(|e| e.to_string())?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
            let rel = entry.enclosed_name().filter(|p| safe_relative(p)).ok_or_else(unsafe_path)?;
            limits.add(entry.size())?;
            let out = dest.join(&rel);
            let mode = entry.unix_mode().unwrap_or(0o644);
            const S_IFMT: u32 = 0o170_000;
            const S_IFLNK: u32 = 0o120_000;
            if entry.is_dir() {
                std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            } else if mode & S_IFMT == S_IFLNK {
                let mut target = String::new();
                (&mut entry).take(u64::from(u16::MAX)).read_to_string(&mut target).map_err(|e| e.to_string())?;
                if link_stays_inside(&rel, Path::new(&target)) {
                    ensure_parent(&out)?;
                    std::os::unix::fs::symlink(&target, &out).map_err(|e| e.to_string())?;
                }
            } else {
                ensure_parent(&out)?;
                copy_stream(&mut entry, &out, (mode & MODE_MASK) | 0o600, cancel, on_bytes, &mut written)?;
            }
        }
        return Ok(written);
    }
    extract_tar(tar_reader(path, format)?, dest, cancel, on_bytes)
}

/// Extracts an already-decoded tar stream into `dest` with the same rules
/// as [`extract`] (formats [`tar_reader`] doesn't know, such as the
/// `.tar.bz2` cursor themes ship as). Returns bytes written.
pub fn extract_tar(reader: Box<dyn Read + Send>, dest: &Path, cancel: &AtomicBool, on_bytes: OnBytes<'_>) -> Result<u64, String> {
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let mut limits = Limits { total: 0, entries: 0 };
    let mut written = 0u64;
    let unsafe_path = || t("The archive contains unsafe paths").to_string();
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries().map_err(|e| e.to_string())? {
        if cancel.load(Ordering::Relaxed) {
            return Err(t("Cancelled").to_string());
        }
        let mut entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.header().entry_type();
        let rel = entry.path().map_err(|e| e.to_string())?.into_owned();
        // `./` alone is the root folder.
        if rel.components().all(|c| c == Component::CurDir) {
            continue;
        }
        if !safe_relative(&rel) {
            return Err(unsafe_path());
        }
        limits.add(entry.header().size().unwrap_or(0))?;
        let out = dest.join(&rel);
        let mode = entry.header().mode().unwrap_or(0o644) & MODE_MASK;
        if kind.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        } else if kind.is_symlink() {
            let Some(target) = entry.link_name().ok().flatten().map(|t| t.into_owned()) else { continue };
            if link_stays_inside(&rel, &target) {
                ensure_parent(&out)?;
                let _ = std::fs::remove_file(&out);
                std::os::unix::fs::symlink(&target, &out).map_err(|e| e.to_string())?;
            }
        } else if kind.is_file() || kind == tar::EntryType::Continuous {
            ensure_parent(&out)?;
            let _ = std::fs::remove_file(&out);
            copy_stream(&mut entry, &out, mode | 0o600, cancel, on_bytes, &mut written)?;
        }
        // Hard links, devices, FIFOs and PAX/GNU metadata entries: skipped.
    }
    // Directories made writable for the owner so the app dir can be
    // trashed / replaced later.
    set_dir_modes(dest);
    Ok(written)
}

fn set_dir_modes(root: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(DIR_MODE));
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(e.path());
            }
        }
    }
}

/// When everything sits in one top-level folder, that folder.
pub fn content_root(dest: &Path) -> PathBuf {
    let entries: Vec<PathBuf> = std::fs::read_dir(dest).map(|r| r.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n != "__MACOSX")).collect()).unwrap_or_default();
    match entries.as_slice() {
        [one] if one.is_dir() && !one.is_symlink() => one.clone(),
        _ => dest.to_path_buf(),
    }
}

/// What was found inside an unpacked portable app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    /// The program to run (relative to the root).
    pub program: Option<PathBuf>,
    /// A packaged desktop entry (relative).
    pub desktop: Option<PathBuf>,
    pub icon: Option<PathBuf>,
}

fn is_elf(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && magic == ELF_MAGIC
}

fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    path.symlink_metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Every file under `root` up to [`SCAN_DEPTH`], relative, sorted.
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() && depth + 1 < SCAN_DEPTH {
                stack.push((p, depth + 1));
            } else if kind.is_file()
                && let Ok(rel) = p.strip_prefix(root)
            {
                out.push(rel.to_path_buf());
            }
        }
    }
    out.sort();
    out
}

/// Finds the program, entry and icon of an unpacked app. `hint` is the
/// archive's name without version (`obsidian`), preferred when several
/// programs are present.
pub fn scan(root: &Path, hint: &str) -> Layout {
    let files = walk(root);
    let hint = hint.to_lowercase();
    let stem = |p: &Path| p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let depth = |p: &Path| p.components().count();
    let desktop = files.iter().filter(|p| p.extension().is_some_and(|e| e == "desktop")).min_by_key(|p| (depth(p), !stem(p).contains(&hint))).cloned();
    // The entry's own Exec, when it points inside the archive.
    let from_entry = desktop.as_ref().and_then(|d| {
        let text = std::fs::read_to_string(root.join(d)).ok()?;
        let exec = super::meta::desktop_value(&text, "Exec")?;
        let (program, _) = super::meta::split_exec(&exec);
        let name = Path::new(&program).file_name()?.to_string_lossy().to_string();
        files.iter().find(|f| f.file_name().is_some_and(|n| n.to_string_lossy() == name) && is_executable_file(&root.join(f))).cloned()
    });
    let program = from_entry.or_else(|| {
        let mut candidates: Vec<&PathBuf> = files
            .iter()
            .filter(|f| {
                let full = root.join(f);
                is_executable_file(&full) && is_elf(&full) && !f.to_string_lossy().contains(".so") && !f.components().any(|c| c.as_os_str() == "lib")
            })
            .collect();
        // Named like the archive, then shallowest, then biggest.
        candidates.sort_by_key(|f| {
            let s = stem(f);
            let named = s == hint || (!hint.is_empty() && s.starts_with(&hint));
            let size = root.join(f).metadata().map(|m| m.len()).unwrap_or(0);
            (!named, depth(f), std::cmp::Reverse(size))
        });
        candidates.first().map(|p| (*p).clone())
    });
    let icon_name = desktop.as_ref().and_then(|d| std::fs::read_to_string(root.join(d)).ok()).and_then(|t| super::meta::desktop_value(&t, "Icon")).map(|i| Path::new(&i).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or(i)).unwrap_or_default();
    let icon = files
        .iter()
        .filter(|f| f.extension().is_some_and(|e| e == "png" || e == "svg"))
        .filter(|f| {
            let s = stem(f);
            (!icon_name.is_empty() && s == icon_name) || (!hint.is_empty() && s.contains(&hint)) || ICON_HINTS.iter().any(|h| f.to_string_lossy().to_lowercase().contains(h))
        })
        .max_by_key(|f| {
            let s = stem(f);
            let exact = !icon_name.is_empty() && s == icon_name;
            let svg = f.extension().is_some_and(|e| e == "svg");
            let size = root.join(f).metadata().map(|m| m.len()).unwrap_or(0);
            (exact, svg, size)
        })
        .cloned();
    Layout { program, desktop, icon }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A `.tar.gz` with `files` (`(path, bytes, mode)`) and `links`.
    pub fn tar_gz(files: &[(&str, &[u8], u32)], links: &[(&str, &str)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (path, bytes, mode) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(bytes.len() as u64);
            h.set_mode(*mode);
            h.set_entry_type(tar::EntryType::Regular);
            builder.append_data(&mut h, path, *bytes).unwrap();
        }
        for (path, target) in links {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            builder.append_link(&mut h, path, target).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    /// Writes raw tar headers so a test can include names `tar::Builder`
    /// itself refuses (`../escape`).
    fn raw_tar_gz(name: &str, bytes: &[u8]) -> Vec<u8> {
        let mut h = tar::Header::new_old();
        h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
        h.set_size(bytes.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        let mut raw = h.as_bytes().to_vec();
        raw.extend_from_slice(bytes);
        raw.resize(raw.len().div_ceil(512) * 512 + 1024, 0);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&raw).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn extracts_and_finds_the_app() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("Tool-2.1.0-linux.tar.gz");
        let elf = b"\x7fELF\x02\x01\x01\0rest";
        std::fs::write(
            &archive,
            tar_gz(
                &[("Tool-2.1.0/tool", elf, 0o4755), ("Tool-2.1.0/lib/libfoo.so", elf, 0o755), ("Tool-2.1.0/resources/icon.png", b"\x89PNG\r\n\x1a\n", 0o644), ("Tool-2.1.0/README", b"hi", 0o644)],
                &[("Tool-2.1.0/tool-link", "tool"), ("Tool-2.1.0/evil", "../../../etc/passwd")],
            ),
        )
        .unwrap();
        let dest = dir.path().join("out");
        let cancel = AtomicBool::new(false);
        let written = extract(&archive, Format::TarGz, &dest, &cancel, &|_| {}).unwrap();
        assert!(written > 0);
        let root = content_root(&dest);
        assert_eq!(root, dest.join("Tool-2.1.0"));
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(root.join("tool").metadata().unwrap().permissions().mode() & 0o7777, 0o755, "setuid stripped");
        assert!(root.join("tool-link").is_symlink());
        assert!(!root.join("evil").exists() && root.join("evil").symlink_metadata().is_err(), "escaping link skipped");
        let layout = scan(&root, "tool");
        assert_eq!(layout.program, Some(PathBuf::from("tool")));
        assert_eq!(layout.icon, Some(PathBuf::from("resources/icon.png")));
    }

    #[test]
    fn refuses_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("bad.tar.gz");
        std::fs::write(&archive, raw_tar_gz("../escape", b"x")).unwrap();
        let err = extract(&archive, Format::TarGz, &dir.path().join("out"), &AtomicBool::new(false), &|_| {}).unwrap_err();
        assert!(err.contains("unsafe"), "{err}");
        assert!(!dir.path().join("escape").exists());
    }

    #[test]
    fn xz_and_multi_frame_zstd_tars_decode() {
        let dir = tempfile::tempdir().unwrap();
        let raw = {
            let mut b = tar::Builder::new(Vec::new());
            let mut h = tar::Header::new_gnu();
            h.set_size(5);
            h.set_mode(0o755);
            b.append_data(&mut h, "app/run", &b"hello"[..]).unwrap();
            b.into_inner().unwrap()
        };
        let mut xz = Vec::new();
        lzma_rs::xz_compress(&mut std::io::Cursor::new(&raw), &mut xz).unwrap();
        // Two frames, as `zstd -T0` writes for big inputs.
        let (a, b) = raw.split_at(raw.len() / 2);
        let mut zst = ruzstd::encoding::compress_to_vec(a, ruzstd::encoding::CompressionLevel::Fastest);
        zst.extend(ruzstd::encoding::compress_to_vec(b, ruzstd::encoding::CompressionLevel::Fastest));
        for (name, bytes, format) in [("a.tar.xz", xz, Format::TarXz), ("a.tar.zst", zst, Format::TarZst)] {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(detect::detect(&path), Ok(format));
            assert_eq!(peek(&path, format, "app").unwrap().total, 5, "{name}");
            let out = dir.path().join(format!("{name}.out"));
            extract(&path, format, &out, &AtomicBool::new(false), &|_| {}).unwrap();
            assert_eq!(std::fs::read(out.join("app/run")).unwrap(), b"hello", "{name}");
        }
    }

    #[test]
    fn link_rules() {
        assert!(link_stays_inside(Path::new("a/b"), Path::new("c")));
        assert!(link_stays_inside(Path::new("a/b"), Path::new("../c")));
        assert!(!link_stays_inside(Path::new("a/b"), Path::new("../../c")));
        assert!(!link_stays_inside(Path::new("a"), Path::new("/etc/passwd")));
    }

    #[test]
    fn cancel_stops_extraction() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.tar.gz");
        std::fs::write(&archive, tar_gz(&[("a/x", b"1", 0o644)], &[])).unwrap();
        let err = extract(&archive, Format::TarGz, &dir.path().join("out"), &AtomicBool::new(true), &|_| {}).unwrap_err();
        assert_eq!(err, t("Cancelled"));
    }
}
