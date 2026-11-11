//! Unpacking theme archives and copying theme folders, safely.
//!
//! Boot themes end up in root-owned folders the bootloader reads, so only
//! theme files are kept:
//!
//! * regular files only: symlinks, hard links, devices and FIFOs are
//!   skipped; a path that is absolute or climbs out with `..` fails the
//!   whole archive;
//! * only the file types themes use ([`KEEP_EXT`]) plus licence files:
//!   install scripts and anything else in the archive are never kept, let
//!   alone run;
//! * caps on the number of entries, each file's size and the total
//!   ([`Limits`]), checked while reading (a lying header can't get past
//!   them).
//!
//! [`copy_tree`] applies the same rules to a folder (an imported theme,
//! or the root side's copy of a staged theme).

use std::io::{BufReader, Read, Write as _};
use std::path::{Component, Path, PathBuf};

use mandara_i18n::{t, tf};

/// File types themes are made of.
pub const KEEP_EXT: &[&str] = &["png", "jpg", "jpeg", "tga", "bmp", "pf2", "txt", "conf", "plymouth", "script", "ttf", "otf"];
/// Licence and attribution files kept alongside (by name, any case).
const KEEP_NAMES: &[&str] = &["license", "licence", "copying", "license.md", "license.txt", "copying.md", "authors", "credits"];

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    pub entries: usize,
    pub file_bytes: u64,
    pub total_bytes: u64,
}

/// Theme archives: the largest catalog theme (grub2-themes with every
/// resolution) unpacks to about 40 MB.
pub const THEME_LIMITS: Limits = Limits { entries: 20_000, file_bytes: 24 << 20, total_bytes: 160 << 20 };
/// What one installed theme may hold.
pub const INSTALL_LIMITS: Limits = Limits { entries: 4_000, file_bytes: 24 << 20, total_bytes: 64 << 20 };

const GZIP_MAGIC: &[u8] = b"\x1f\x8b";
const XZ_MAGIC: &[u8] = b"\xfd7zXZ\x00";
const ZSTD_MAGIC: &[u8] = b"\x28\xb5\x2f\xfd";
const BZIP2_MAGIC: &[u8] = b"BZh";
const ZIP_MAGIC: &[u8] = b"PK\x03\x04";
/// `ustar` at byte 257 of a plain tar.
const USTAR_AT: usize = 257;
const USTAR: &[u8] = b"ustar";
const COPY_CHUNK: usize = 64 << 10;

/// Whether a theme keeps the file `rel`.
pub fn kept(rel: &Path) -> bool {
    let Some(name) = rel.file_name().and_then(|n| n.to_str()) else { return false };
    let lower = name.to_ascii_lowercase();
    if KEEP_NAMES.contains(&lower.as_str()) {
        return true;
    }
    !lower.starts_with('.') && Path::new(&lower).extension().and_then(|e| e.to_str()).is_some_and(|e| KEEP_EXT.contains(&e))
}

/// A plain relative path (no root, no `..`).
pub fn safe_relative(rel: &Path) -> bool {
    !rel.as_os_str().is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

struct Budget {
    limits: Limits,
    entries: usize,
    total: u64,
}

impl Budget {
    fn entry(&mut self) -> Result<(), String> {
        self.entries += 1;
        if self.entries > self.limits.entries {
            return Err(t("The theme has too many files").to_string());
        }
        Ok(())
    }

    /// Copies `from` to a new file `to`, counting bytes as they come.
    fn copy(&mut self, from: &mut dyn Read, to: &Path) -> Result<(), String> {
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(to).map_err(|e| format!("{}: {e}", to.display()))?;
        let mut buf = vec![0u8; COPY_CHUNK];
        let mut size = 0u64;
        loop {
            let n = from.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            size += n as u64;
            self.total += n as u64;
            if size > self.limits.file_bytes {
                return Err(tf("{file} is too large for a theme", &[("file", &to.file_name().unwrap_or_default().to_string_lossy())]));
            }
            if self.total > self.limits.total_bytes {
                return Err(t("The theme is too large").to_string());
            }
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

/// The decoded tar stream of `path` (gzip, xz, zstd, bzip2 or plain).
fn tar_stream(path: &Path) -> Result<Box<dyn Read + Send>, String> {
    let mut head = [0u8; USTAR_AT + 8];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).map_err(|e| e.to_string())?;
    let head = &head[..n];
    if head.starts_with(GZIP_MAGIC) || head.starts_with(XZ_MAGIC) || head.starts_with(ZSTD_MAGIC) {
        return crate::app_install::archive::tar_reader(path, crate::app_install::detect::Format::TarGz);
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if head.starts_with(BZIP2_MAGIC) {
        return Ok(Box::new(bzip2::read::MultiBzDecoder::new(BufReader::new(file))));
    }
    if head.len() >= USTAR_AT + USTAR.len() && &head[USTAR_AT..USTAR_AT + USTAR.len()] == USTAR {
        return Ok(Box::new(BufReader::new(file)));
    }
    Err(t("Not a theme archive (expected .tar, .tar.gz, .tar.xz, .tar.zst, .tar.bz2 or .zip)").to_string())
}

fn is_zip(path: &Path) -> bool {
    let mut head = [0u8; 4];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && head == ZIP_MAGIC
}

/// Unpacks the theme files of `archive` into `dest` (created; must not
/// exist yet).
pub fn unpack(archive: &Path, dest: &Path, limits: Limits) -> Result<(), String> {
    if dest.exists() {
        return Err(format!("{} already exists", dest.display()));
    }
    std::fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let mut budget = Budget { limits, entries: 0, total: 0 };
    let unsafe_path = || t("The archive contains unsafe paths").to_string();
    if is_zip(archive) {
        let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipArchive::new(BufReader::new(file)).map_err(|e| e.to_string())?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
            budget.entry()?;
            let rel = PathBuf::from(&*entry.name().map_err(|e| e.to_string())?);
            if !safe_relative(&rel) {
                return Err(unsafe_path());
            }
            // Symlinks are stored as files with the link's mode bits.
            const S_IFMT: u32 = 0o170_000;
            const S_IFREG: u32 = 0o100_000;
            let regular = entry.unix_mode().is_none_or(|m| m & S_IFMT == S_IFREG || m & S_IFMT == 0);
            if entry.is_dir() || !regular || !kept(&rel) {
                continue;
            }
            budget.copy(&mut entry, &dest.join(&rel))?;
        }
        return Ok(());
    }
    let mut tar = tar::Archive::new(tar_stream(archive)?);
    for entry in tar.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        budget.entry()?;
        let rel = entry.path().map_err(|e| e.to_string())?.into_owned();
        if rel.components().all(|c| c == Component::CurDir) {
            continue;
        }
        if !safe_relative(&rel) {
            return Err(unsafe_path());
        }
        // Links, devices, FIFOs and metadata entries are never kept.
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind == tar::EntryType::Continuous) || !kept(&rel) {
            continue;
        }
        budget.copy(&mut entry, &dest.join(&rel))?;
    }
    Ok(())
}

/// Copies the theme files of folder `from` into `to` (created; must not
/// exist yet) with the same rules as [`unpack`]: symlinks anywhere in the
/// tree are skipped, never followed.
pub fn copy_tree(from: &Path, to: &Path, limits: Limits) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(from).map_err(|e| format!("{}: {e}", from.display()))?;
    if !meta.is_dir() {
        return Err(tf("{path} isn't a folder", &[("path", &from.display())]));
    }
    if to.exists() {
        return Err(format!("{} already exists", to.display()));
    }
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let mut budget = Budget { limits, entries: 0, total: 0 };
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let dir = from.join(&rel);
        let mut entries: Vec<_> = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            budget.entry()?;
            let child = rel.join(e.file_name());
            // symlink_metadata: a link is a link, whatever it points to.
            let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
            if m.is_dir() {
                stack.push(child);
            } else if m.is_file() && kept(&child) {
                let mut f = std::fs::File::open(e.path()).map_err(|err| format!("{}: {err}", e.path().display()))?;
                budget.copy(&mut f, &to.join(&child))?;
            }
        }
    }
    Ok(())
}

/// Every regular file under `dir`, relative, sorted (symlinks skipped).
pub fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        for e in std::fs::read_dir(dir.join(&rel)).into_iter().flatten().flatten() {
            let child = rel.join(e.file_name());
            match std::fs::symlink_metadata(e.path()) {
                Ok(m) if m.is_dir() => stack.push(child),
                Ok(m) if m.is_file() => out.push(child),
                _ => {}
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tar(.gz) with `files` (path, bytes) and `links` (path → target).
    pub(crate) fn tar_bytes(files: &[(&str, &[u8])], links: &[(&str, &str)], gzip: bool) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (path, data) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_entry_type(tar::EntryType::Regular);
            // set_path refuses `..`: write the raw name for the hostile cases.
            let name: &mut [u8] = &mut h.as_old_mut().name;
            name[..path.len()].copy_from_slice(path.as_bytes());
            h.set_cksum();
            b.append(&h, *data).unwrap();
        }
        for (path, target) in links {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            b.append_link(&mut h, path, target).unwrap();
        }
        let raw = b.into_inner().unwrap();
        if !gzip {
            return raw;
        }
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&raw).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn keeps_only_theme_files() {
        let dir = tempfile::tempdir().unwrap();
        let arc = dir.path().join("t.tar.gz");
        std::fs::write(&arc, tar_bytes(&[("t/theme.txt", b"title-text: \"\""), ("t/background.png", b"png"), ("t/install.sh", b"rm -rf /"), ("t/update.py", b"x"), ("t/LICENSE", b"MIT"), ("t/.hidden.png", b"x")], &[("t/link.png", "/etc/shadow")], true)).unwrap();
        let out = dir.path().join("out");
        unpack(&arc, &out, THEME_LIMITS).unwrap();
        assert_eq!(files(&out), ["t/LICENSE", "t/background.png", "t/theme.txt"].map(PathBuf::from));
        assert!(std::fs::symlink_metadata(out.join("t/link.png")).is_err(), "links are never created");
    }

    #[test]
    fn plain_tar_and_zip_unpack_too() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("p.tar");
        std::fs::write(&plain, tar_bytes(&[("theme.txt", b"x"), ("icons/arch.png", b"y")], &[], false)).unwrap();
        unpack(&plain, &dir.path().join("p"), THEME_LIMITS).unwrap();
        assert_eq!(files(&dir.path().join("p")), ["icons/arch.png", "theme.txt"].map(PathBuf::from));
        let zipped = dir.path().join("z.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&zipped).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file("z/theme.conf", opts).unwrap();
            w.write_all(b"banner themes/z/bg.png").unwrap();
            w.start_file("z/run.sh", opts).unwrap();
            w.write_all(b"echo").unwrap();
            w.finish().unwrap();
        }
        unpack(&zipped, &dir.path().join("z"), THEME_LIMITS).unwrap();
        assert_eq!(files(&dir.path().join("z")), [PathBuf::from("z/theme.conf")]);
    }

    #[test]
    fn traversal_fails_the_archive() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["../evil.png", "/etc/evil.png", "a/../../evil.png"] {
            let arc = dir.path().join("bad.tar");
            std::fs::write(&arc, tar_bytes(&[(bad, b"x")], &[], false)).unwrap();
            let out = dir.path().join(format!("o{}", bad.len()));
            assert!(unpack(&arc, &out, THEME_LIMITS).is_err(), "{bad}");
            assert!(!dir.path().join("evil.png").exists());
        }
        std::fs::write(dir.path().join("junk"), b"not an archive at all").unwrap();
        assert!(unpack(&dir.path().join("junk"), &dir.path().join("j"), THEME_LIMITS).is_err());
    }

    #[test]
    fn size_and_count_caps_hold() {
        let dir = tempfile::tempdir().unwrap();
        let big = vec![0u8; 2048];
        let arc = dir.path().join("big.tar");
        std::fs::write(&arc, tar_bytes(&[("a.png", &big), ("b.png", &big)], &[], false)).unwrap();
        let tight = Limits { entries: 10, file_bytes: 1024, total_bytes: 1 << 20 };
        assert!(unpack(&arc, &dir.path().join("x"), tight).is_err(), "a file over the cap");
        let few = Limits { entries: 1, file_bytes: 1 << 20, total_bytes: 1 << 20 };
        assert!(unpack(&arc, &dir.path().join("y"), few).is_err(), "too many entries");
        let total = Limits { entries: 10, file_bytes: 4096, total_bytes: 3000 };
        assert!(unpack(&arc, &dir.path().join("z"), total).is_err(), "over the total");
    }

    #[test]
    fn copy_tree_skips_links_and_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("icons")).unwrap();
        std::fs::write(src.join("theme.txt"), "x").unwrap();
        std::fs::write(src.join("icons/a.png"), "x").unwrap();
        std::fs::write(src.join("evil.sh"), "x").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", src.join("passwd.txt")).unwrap();
        std::os::unix::fs::symlink("/etc", src.join("etc")).unwrap();
        let to = dir.path().join("to");
        copy_tree(&src, &to, INSTALL_LIMITS).unwrap();
        assert_eq!(files(&to), ["icons/a.png", "theme.txt"].map(PathBuf::from));
        assert!(copy_tree(&src.join("theme.txt"), &dir.path().join("t2"), INSTALL_LIMITS).is_err());
        assert!(copy_tree(&src.join("etc"), &dir.path().join("t3"), INSTALL_LIMITS).is_err(), "a linked folder isn't followed");
    }
}
