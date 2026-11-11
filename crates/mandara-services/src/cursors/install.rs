//! Installing cursor themes into `~/.local/share/icons/<Theme>/`, from the
//! catalog or from an archive, folder or link you pick, and removing them
//! again.
//!
//! * **Download** — the pinned release file is streamed to
//!   `~/.cache/mandara/cursors/`, hashed as it arrives and kept only when
//!   size and SHA-256 match the catalog (`theme_pkg::fetch`).
//! * **Unpack** — pure-Rust readers (gzip, xz, zstd, bzip2, zip) into a
//!   hidden scratch folder beside the icons folder; paths leaving it are
//!   refused (`app_install::archive`). Only the theme's folder is moved
//!   into place, after checking it holds an `index.theme` and readable
//!   XCursor files. Scripts and everything else in the archive are never
//!   run or kept.
//! * **Import** — an archive or folder you pick, or an `https://` link you
//!   paste: there is no checksum for those, so they are recorded as
//!   unverified (Studio asks first). Every cursor theme found in it is
//!   checked the same way and installed under its folder's name.
//! * **Manifest** — `~/.local/share/mandara/cursor-themes.json` lists what
//!   Mandara installed. A folder that exists but isn't listed belongs to
//!   you or a package and is never replaced or removed.

use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};

use super::catalog::{Asset, CursorVariant};
use super::{Role, xcursor};
use crate::app_install::archive::ExtractOptions;
use crate::theme_pkg::{self, Pin, Scratch, cancelled};

/// Largest download accepted (every catalog file is far below it).
pub const MAX_DOWNLOAD: u64 = 64 << 20;
/// Largest total an import unpacks.
const MAX_UNPACKED: u64 = 1 << 30;
/// Most entries an import unpacks.
const MAX_ENTRIES: usize = 100_000;
/// How deep an import is searched for themes.
const IMPORT_DEPTH: usize = 3;
const MANIFEST_FILE: &str = "cursor-themes.json";
/// Prefix of the scratch folders unpacking happens in.
const SCRATCH_PREFIX: &str = ".mandara-cursor-";
/// Names never installed over.
const RESERVED: [&str; 2] = ["default", "hicolor"];

fn limits(keep: Option<&dyn Fn(&Path) -> bool>) -> ExtractOptions<'_> {
    ExtractOptions { max_bytes: MAX_UNPACKED, max_entries: MAX_ENTRIES, keep }
}

/// Where themes go and where Mandara keeps its records.
#[derive(Clone, Debug)]
pub struct Env {
    /// `~/.local/share/icons` (`$XDG_DATA_HOME/icons`).
    pub icons: PathBuf,
    /// Downloads in progress (`~/.cache/mandara/cursors`).
    pub cache: PathBuf,
    /// The install manifest.
    pub manifest: PathBuf,
}

impl Env {
    pub fn current() -> Self {
        let data = dirs::data_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"));
        Self { icons: data.join("icons"), cache: mandara_config::paths::cache_dir().join("cursors"), manifest: mandara_config::paths::data_dir().join(MANIFEST_FILE) }
    }

    /// Everything under `root` (tests, sandboxes).
    pub fn under(root: &Path) -> Self {
        Self { icons: root.join("icons"), cache: root.join("cache"), manifest: root.join(MANIFEST_FILE) }
    }
}

fn yes() -> bool {
    true
}

/// A theme Mandara installed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    /// XCursor theme name (the folder's name).
    pub theme: String,
    /// Catalog id and variant (empty for an import).
    pub catalog: String,
    pub variant: String,
    pub sha256: String,
    pub dir: PathBuf,
    /// Unix seconds.
    pub installed_at: u64,
    /// Where an import came from (a path or a link).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Checked against the catalog's pinned checksum.
    #[serde(default = "yes")]
    pub verified: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub themes: Vec<Installed>,
}

impl Manifest {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        mandara_config::write_atomic(path, text.as_bytes()).map_err(|e| format!("{e:#}"))
    }

    pub fn get(&self, theme: &str) -> Option<&Installed> {
        self.themes.iter().find(|i| i.theme == theme)
    }
}

/// The themes Mandara installed (and may remove).
pub fn installed(env: &Env) -> Vec<Installed> {
    Manifest::load(&env.manifest).themes
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Download,
    Unpack,
    Done,
}

/// An install's progress: bytes of the current phase.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    pub phase: Phase,
    pub done: u64,
    pub total: u64,
}

impl Progress {
    /// Overall share done, `0..=1` (downloading is most of the wait).
    pub fn fraction(&self) -> f32 {
        const DOWNLOAD_SHARE: f32 = 0.85;
        let part = if self.total == 0 { 0.0 } else { (self.done as f32 / self.total as f32).min(1.0) };
        match self.phase {
            Phase::Download => part * DOWNLOAD_SHARE,
            Phase::Unpack => DOWNLOAD_SHARE + part * (1.0 - DOWNLOAD_SHARE),
            Phase::Done => 1.0,
        }
    }
}

/// SHA-256 of a file, lower-case hex.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    theme_pkg::sha256_file(path)
}

/// Downloads `asset` into `cache` (reusing a verified earlier download)
/// and returns the file, checked against the pinned size and SHA-256.
pub fn download(asset: &Asset, cache: &Path, progress: &dyn Fn(Progress), cancel: &AtomicBool) -> Result<PathBuf, String> {
    let pin = Pin { sha256: asset.sha256, size: asset.size };
    theme_pkg::fetch(asset.url, Some(pin), MAX_DOWNLOAD, cache, &|done, total| progress(Progress { phase: Phase::Download, done, total }), cancel).map(|f| f.path)
}

/// Whether `dir` is a real cursor theme: an `index.theme` and a `cursors/`
/// folder whose arrow (or any file) parses as XCursor.
pub fn is_cursor_theme(dir: &Path) -> bool {
    let cursors = dir.join("cursors");
    let real_dir = |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir());
    if !real_dir(dir) || !real_dir(&cursors) || !dir.join("index.theme").is_file() {
        return false;
    }
    let parses = |p: &Path| std::fs::File::open(p).is_ok_and(|f| xcursor::read_toc(&mut BufReader::new(f)).is_ok_and(|toc| !xcursor::sizes(&toc).is_empty()));
    let arrow = Role::Default.names().iter().map(|n| cursors.join(n)).find(|p| p.is_file());
    match arrow {
        Some(p) => parses(&p),
        None => std::fs::read_dir(&cursors).into_iter().flatten().flatten().any(|e| parses(&e.path())),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn installable_name(name: &str) -> bool {
    theme_pkg::valid_name(name) && !RESERVED.contains(&name)
}

/// Moves the checked theme folder `source` to `env.icons/theme` (replacing
/// Mandara's own copy) and records it.
fn place(source: &Path, theme: &str, entry: Installed, manifest: &mut Manifest, env: &Env) -> Result<Installed, String> {
    let target = env.icons.join(theme);
    let ours = manifest.get(theme).is_some();
    if std::fs::symlink_metadata(&target).is_ok() {
        if !ours {
            return Err(tf("{theme} is already installed (not by Mandara), so it was left as it is", &[("theme", &theme)]));
        }
        std::fs::remove_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    std::fs::rename(source, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    let entry = Installed { dir: target, ..entry };
    manifest.themes.retain(|i| i.theme != theme);
    manifest.themes.push(entry.clone());
    Ok(entry)
}

/// Installs `variant` (of catalog theme `catalog_id`) from an already
/// verified `archive_path`.
pub fn install_archive(archive_path: &Path, catalog_id: &str, variant: &CursorVariant, sha256: &str, env: &Env, progress: &dyn Fn(Progress), cancel: &AtomicBool) -> Result<Installed, String> {
    let theme = variant.theme;
    if !installable_name(theme) {
        return Err(tf("“{theme}” isn't a valid theme name", &[("theme", &theme)]));
    }
    let target = env.icons.join(theme);
    let mut manifest = Manifest::load(&env.manifest);
    if std::fs::symlink_metadata(&target).is_ok() && manifest.get(theme).is_none() {
        return Err(tf("{theme} is already installed (not by Mandara), so it was left as it is", &[("theme", &theme)]));
    }
    std::fs::create_dir_all(&env.icons).map_err(|e| format!("{}: {e}", env.icons.display()))?;
    let scratch = Scratch::in_dir(&env.icons, SCRATCH_PREFIX);
    let total = std::fs::metadata(archive_path).map(|m| m.len()).unwrap_or(0);
    progress(Progress { phase: Phase::Unpack, done: 0, total });
    // The unpacked size isn't known up front: report it against the
    // archive's size, capped.
    theme_pkg::unpack(archive_path, &scratch.0, &ExtractOptions::APPS, cancel, &|done| progress(Progress { phase: Phase::Unpack, done: done.min(total), total }))?;
    cancelled(cancel)?;
    let source = scratch.0.join(variant.asset.folder);
    if !source.starts_with(&scratch.0) || !is_cursor_theme(&source) {
        return Err(tf("The download has no cursor theme at {folder}", &[("folder", &variant.asset.folder)]));
    }
    let entry = Installed { theme: theme.to_string(), catalog: catalog_id.to_string(), variant: variant.id.to_string(), sha256: sha256.to_string(), dir: target, installed_at: now_secs(), source: None, verified: true };
    let entry = place(&source, theme, entry, &mut manifest, env)?;
    drop(scratch);
    manifest.save(&env.manifest)?;
    progress(Progress { phase: Phase::Done, done: 1, total: 1 });
    Ok(entry)
}

/// Downloads, verifies and installs `variant`. The downloaded archive is
/// deleted afterwards (the installed folder is the preview source).
pub fn install(catalog_id: &str, variant: &CursorVariant, env: &Env, progress: &dyn Fn(Progress), cancel: &AtomicBool) -> Result<Installed, String> {
    let archive_path = download(&variant.asset, &env.cache, progress, cancel)?;
    let result = install_archive(&archive_path, catalog_id, variant, variant.asset.sha256, env, progress, cancel);
    // Variants sharing one archive (Qogir, Phinger, Vimix) download it once
    // per install; the cache stays empty between installs.
    let _ = std::fs::remove_file(&archive_path);
    result
}

/// Something to import (the same shape as for icon themes).
pub use crate::icons::install::ImportSource;

/// What an import did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    pub installed: Vec<Installed>,
    /// Themes found but not installed, with why.
    pub skipped: Vec<(String, String)>,
}

/// Imports every cursor theme in `source` (unverified: no pinned checksum).
/// Themes whose name is taken by one Mandara didn't install are skipped.
/// Fails when nothing could be installed.
pub fn import(source: &ImportSource, env: &Env, progress: &dyn Fn(Progress), cancel: &AtomicBool) -> Result<ImportReport, String> {
    std::fs::create_dir_all(&env.icons).map_err(|e| format!("{}: {e}", env.icons.display()))?;
    let scratch = Scratch::in_dir(&env.icons, SCRATCH_PREFIX);
    let raw = scratch.0.join("raw");
    let is_theme = |d: &Path| d.join("index.theme").is_file() || d.join("cursors").is_dir();
    let (origin, sha256) = match source {
        ImportSource::Path(path) => {
            let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
            if meta.is_dir() {
                // Only the theme folders in it are copied.
                for (i, theme) in theme_pkg::find_dirs(path, IMPORT_DEPTH, &is_theme).iter().enumerate() {
                    let name = theme.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    theme_pkg::copy_tree(theme, &raw.join(i.to_string()).join(name), &limits(None), cancel, &|_| {})?;
                }
                (path.display().to_string(), String::new())
            } else if theme_pkg::is_archive(path) {
                if meta.len() > MAX_DOWNLOAD {
                    return Err(t("The archive is too large to install").to_string());
                }
                let total = meta.len();
                theme_pkg::unpack(path, &raw, &limits(None), cancel, &|done| progress(Progress { phase: Phase::Unpack, done: done.min(total), total }))?;
                (path.display().to_string(), theme_pkg::sha256_file(path)?)
            } else {
                return Err(t("Pick a cursor theme folder or an archive (.tar.gz, .tar.xz, .tar.zst, .tar.bz2 or .zip)").to_string());
            }
        }
        ImportSource::Url(url) => {
            let fetched = theme_pkg::fetch(url, None, MAX_DOWNLOAD, &env.cache, &|done, total| progress(Progress { phase: Phase::Download, done, total }), cancel)?;
            let total = fetched.size;
            let unpacked = theme_pkg::unpack(&fetched.path, &raw, &limits(None), cancel, &|done| progress(Progress { phase: Phase::Unpack, done: done.min(total), total }));
            let _ = std::fs::remove_file(&fetched.path);
            unpacked?;
            (url.clone(), fetched.sha256)
        }
    };
    cancelled(cancel)?;
    let found = theme_pkg::find_dirs(&raw, IMPORT_DEPTH, &is_theme);
    let mut manifest = Manifest::load(&env.manifest);
    let mut report = ImportReport::default();
    for dir in found {
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !installable_name(&name) || report.installed.iter().any(|i| i.theme == name) {
            report.skipped.push((name, t("not a usable theme name").to_string()));
            continue;
        }
        if !is_cursor_theme(&dir) {
            report.skipped.push((name, t("no readable XCursor files in cursors/").to_string()));
            continue;
        }
        // Links leaving the theme go; nothing else is changed.
        if let Some(parent) = dir.parent() {
            theme_pkg::tidy_theme(parent, &name, &[], &[], cancel)?;
        }
        let entry = Installed { theme: name.clone(), catalog: String::new(), variant: String::new(), sha256: sha256.clone(), dir: PathBuf::new(), installed_at: now_secs(), source: Some(origin.clone()), verified: false };
        match place(&dir, &name, entry, &mut manifest, env) {
            Ok(e) => report.installed.push(e),
            Err(e) => report.skipped.push((name, e)),
        }
    }
    drop(scratch);
    if report.installed.is_empty() {
        if report.skipped.is_empty() {
            return Err(t("No cursor theme found there (a folder with a cursors/ folder and an index.theme)").to_string());
        }
        let why = report.skipped.iter().map(|(n, w)| format!("{n}: {w}")).collect::<Vec<_>>().join("; ");
        return Err(tf("Nothing was installed ({reasons})", &[("reasons", &why)]));
    }
    manifest.save(&env.manifest)?;
    progress(Progress { phase: Phase::Done, done: 1, total: 1 });
    Ok(report)
}

/// Removes a theme Mandara installed. Folders Mandara didn't install are
/// refused, and only the exact folder it recorded is deleted.
pub fn remove(theme: &str, env: &Env) -> Result<Installed, String> {
    let mut manifest = Manifest::load(&env.manifest);
    let Some(entry) = manifest.get(theme).cloned() else {
        return Err(tf("{theme} wasn't installed by Mandara, so it stays", &[("theme", &theme)]));
    };
    let expected = env.icons.join(theme);
    if !installable_name(theme) || entry.dir != expected {
        return Err(tf("{theme} wasn't installed by Mandara, so it stays", &[("theme", &theme)]));
    }
    match std::fs::symlink_metadata(&expected) {
        // Replaced by a link since: drop the record, leave the link.
        Ok(m) if m.file_type().is_symlink() => {}
        Ok(_) => std::fs::remove_dir_all(&expected).map_err(|e| format!("{}: {e}", expected.display()))?,
        Err(_) => {}
    }
    manifest.themes.retain(|i| i.theme != theme);
    manifest.save(&env.manifest)?;
    Ok(entry)
}

/// Leftover scratch folders from an interrupted install.
pub fn clean_scratch(env: &Env) {
    theme_pkg::clean_scratch(&env.icons, SCRATCH_PREFIX);
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::super::catalog::Asset;
    use super::super::tests::cursor_file;
    use super::*;
    use crate::app_install::archive::tests::tar_gz;

    static NO_CANCEL: AtomicBool = AtomicBool::new(false);

    const fn variant(theme: &'static str, folder: &'static str) -> CursorVariant {
        CursorVariant { id: "test", label: "Test", theme, swatch: 0, asset: Asset { url: "https://github.com/example/example", sha256: "", size: 0, folder } }
    }

    static GOOD: CursorVariant = variant("Fancy-Cursors", "pkg/Fancy");

    /// An archive with a theme at `pkg/Fancy` plus an installer script.
    fn good_archive(dir: &Path) -> PathBuf {
        let arrow = cursor_file(0xff00_0000);
        let bytes = tar_gz(
            &[
                ("pkg/Fancy/index.theme", b"[Icon Theme]\nName=Fancy\n", 0o644),
                ("pkg/Fancy/cursors/left_ptr", &arrow, 0o644),
                ("pkg/install.sh", b"#!/bin/sh\nrm -rf ~\n", 0o755),
            ],
            &[("pkg/Fancy/cursors/default", "left_ptr")],
        );
        let path = dir.join("fancy.tar.gz");
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn env(root: &Path) -> Env {
        Env::under(root)
    }

    #[test]
    fn installs_only_the_theme_folder_and_records_it() {
        let root = tempfile::tempdir().unwrap();
        let env = env(root.path());
        let archive = good_archive(root.path());
        let seen = std::cell::RefCell::new(Vec::new());
        let entry = install_archive(&archive, "fancy", &GOOD, "abc", &env, &|p| seen.borrow_mut().push(p.phase), &NO_CANCEL).unwrap();
        let dir = env.icons.join("Fancy-Cursors");
        assert_eq!(entry.dir, dir);
        assert!(is_cursor_theme(&dir));
        assert!(dir.join("cursors/default").is_symlink(), "in-theme links are kept");
        // Nothing else from the archive, and no scratch folder left behind.
        let names: Vec<String> = std::fs::read_dir(&env.icons).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert_eq!(names, vec!["Fancy-Cursors"]);
        assert!(!env.icons.join("install.sh").exists() && !dir.join("install.sh").exists());
        assert_eq!(installed(&env), vec![entry]);
        assert_eq!(seen.borrow().last(), Some(&Phase::Done));
        // The arrow previews from the installed copy.
        let img = super::super::preview("Fancy-Cursors", Role::Default, 24, std::slice::from_ref(&env.icons)).unwrap();
        assert_eq!(img.nominal, 24);
        // Reinstalling replaces Mandara's own folder.
        install_archive(&archive, "fancy", &GOOD, "def", &env, &|_| {}, &NO_CANCEL).unwrap();
        assert_eq!(installed(&env).len(), 1);
        assert_eq!(installed(&env)[0].sha256, "def");
    }

    #[test]
    fn remove_deletes_only_what_mandara_installed() {
        let root = tempfile::tempdir().unwrap();
        let env = env(root.path());
        install_archive(&good_archive(root.path()), "fancy", &GOOD, "abc", &env, &|_| {}, &NO_CANCEL).unwrap();
        // A theme the user installed themselves.
        super::super::tests::make_theme(&env.icons, "Mine", &[("left_ptr", 0xff00_0000)], None);
        assert!(remove("Mine", &env).is_err());
        assert!(env.icons.join("Mine/cursors/left_ptr").exists());
        assert!(remove("../Mine", &env).is_err());
        // A tampered manifest pointing elsewhere is refused.
        let mut m = Manifest::load(&env.manifest);
        m.themes.push(Installed { theme: "Mine".into(), catalog: "x".into(), variant: "x".into(), sha256: String::new(), dir: root.path().to_path_buf(), installed_at: 0, source: None, verified: true });
        m.save(&env.manifest).unwrap();
        assert!(remove("Mine", &env).is_err());
        assert!(root.path().join("icons/Mine").exists());
        // Mandara's own goes, record and folder.
        remove("Fancy-Cursors", &env).unwrap();
        assert!(!env.icons.join("Fancy-Cursors").exists());
        assert!(installed(&env).iter().all(|i| i.theme != "Fancy-Cursors"));
    }

    #[test]
    fn never_replaces_a_theme_mandara_did_not_install() {
        let root = tempfile::tempdir().unwrap();
        let env = env(root.path());
        super::super::tests::make_theme(&env.icons, "Fancy-Cursors", &[("left_ptr", 0xff12_3456)], None);
        let before = std::fs::read(env.icons.join("Fancy-Cursors/cursors/left_ptr")).unwrap();
        assert!(install_archive(&good_archive(root.path()), "fancy", &GOOD, "abc", &env, &|_| {}, &NO_CANCEL).is_err());
        assert_eq!(std::fs::read(env.icons.join("Fancy-Cursors/cursors/left_ptr")).unwrap(), before);
        assert!(installed(&env).is_empty());
    }

    /// A `.tar.gz` whose one entry is named `name` (raw headers, so names
    /// `tar::Builder` refuses can be written).
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
    fn path_traversal_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let env = env(root.path().join("home").as_path());
        for evil in ["../escape", "pkg/../../escape", "/tmp/escape"] {
            let path = root.path().join("evil.tar.gz");
            std::fs::write(&path, raw_tar_gz(evil, b"pwned")).unwrap();
            assert!(install_archive(&path, "evil", &GOOD, "abc", &env, &|_| {}, &NO_CANCEL).is_err(), "{evil}");
            assert!(!root.path().join("escape").exists() && !root.path().join("home/escape").exists(), "{evil}");
        }
        // No scratch folders survive the failures.
        assert!(std::fs::read_dir(&env.icons).map(|r| r.count()).unwrap_or(0) == 0);
        assert!(installed(&env).is_empty());
    }

    #[test]
    fn symlinks_out_of_the_theme_are_dropped() {
        let root = tempfile::tempdir().unwrap();
        let env = env(root.path());
        let arrow = cursor_file(0xff00_0000);
        let bytes = tar_gz(&[("pkg/Fancy/index.theme", b"[Icon Theme]\n", 0o644), ("pkg/Fancy/cursors/left_ptr", &arrow, 0o644)], &[("pkg/Fancy/cursors/hand2", "../../../../etc/passwd")]);
        let path = root.path().join("links.tar.gz");
        std::fs::write(&path, bytes).unwrap();
        install_archive(&path, "fancy", &GOOD, "abc", &env, &|_| {}, &NO_CANCEL).unwrap();
        assert!(std::fs::symlink_metadata(env.icons.join("Fancy-Cursors/cursors/hand2")).is_err());
    }

    #[test]
    fn a_folder_without_cursors_is_not_installed() {
        let root = tempfile::tempdir().unwrap();
        let env = env(root.path());
        let bytes = tar_gz(&[("pkg/Fancy/index.theme", b"[Icon Theme]\n", 0o644), ("pkg/Fancy/cursors/left_ptr", b"not a cursor", 0o644)], &[]);
        let path = root.path().join("bad.tar.gz");
        std::fs::write(&path, bytes).unwrap();
        assert!(install_archive(&path, "fancy", &GOOD, "abc", &env, &|_| {}, &NO_CANCEL).is_err());
        static MISSING: CursorVariant = variant("Other", "pkg/Nope");
        assert!(install_archive(&good_archive(root.path()), "fancy", &MISSING, "abc", &env, &|_| {}, &NO_CANCEL).is_err());
        static BAD_NAME: CursorVariant = variant("../Evil", "pkg/Fancy");
        assert!(install_archive(&good_archive(root.path()), "fancy", &BAD_NAME, "abc", &env, &|_| {}, &NO_CANCEL).is_err());
        assert!(installed(&env).is_empty());
    }

    #[test]
    fn unpacks_bzip2_and_zip() {
        let root = tempfile::tempdir().unwrap();
        let env = env(root.path());
        let arrow = cursor_file(0xff00_0000);
        // .tar.bz2
        let mut builder = tar::Builder::new(Vec::new());
        for (name, data) in [("pkg/Fancy/index.theme", &b"[Icon Theme]\n"[..]), ("pkg/Fancy/cursors/left_ptr", &arrow[..])] {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            builder.append_data(&mut h, name, data).unwrap();
        }
        let tar_bytes = builder.into_inner().unwrap();
        let mut bz = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        bz.write_all(&tar_bytes).unwrap();
        let path = root.path().join("t.tar.bz2");
        std::fs::write(&path, bz.finish().unwrap()).unwrap();
        install_archive(&path, "fancy", &GOOD, "abc", &env, &|_| {}, &NO_CANCEL).unwrap();
        remove("Fancy-Cursors", &env).unwrap();
        // .zip
        let path = root.path().join("t.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("pkg/Fancy/index.theme", opts).unwrap();
        zip.write_all(b"[Icon Theme]\n").unwrap();
        zip.start_file("pkg/Fancy/cursors/left_ptr", opts).unwrap();
        zip.write_all(&arrow).unwrap();
        zip.finish().unwrap();
        install_archive(&path, "fancy", &GOOD, "abc", &env, &|_| {}, &NO_CANCEL).unwrap();
        assert!(is_cursor_theme(&env.icons.join("Fancy-Cursors")));
    }

    #[test]
    fn verified_downloads_are_reused_and_bad_ones_are_not() {
        let root = tempfile::tempdir().unwrap();
        let data = b"cursor theme bytes";
        let cache = root.path().join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("probe"), data).unwrap();
        let sha = sha256_file(&cache.join("probe")).unwrap();
        std::fs::rename(cache.join("probe"), cache.join(&sha)).unwrap();
        let asset = Asset { url: "https://invalid.invalid/never-fetched", sha256: Box::leak(sha.clone().into_boxed_str()), size: data.len() as u64, folder: "x" };
        assert_eq!(download(&asset, &cache, &|_| {}, &NO_CANCEL).unwrap(), cache.join(&sha));
        assert_eq!(sha256_file(&cache.join(&sha)).unwrap(), sha);
    }

    #[test]
    fn progress_fraction_is_monotonic() {
        let p = |phase, done, total| Progress { phase, done, total }.fraction();
        assert_eq!(p(Phase::Download, 0, 100), 0.0);
        assert!(p(Phase::Download, 100, 100) < p(Phase::Unpack, 1, 100));
        assert!(p(Phase::Unpack, 100, 100) <= p(Phase::Done, 1, 1));
        assert_eq!(p(Phase::Download, 5, 0), 0.0);
    }

    /// With `MANDARA_CURSOR_ARCHIVES` naming a folder of the real release
    /// files (any names; matched by SHA-256), every catalog variant
    /// installs, previews each shape and removes cleanly.
    #[test]
    fn every_catalog_variant_installs_from_its_release() {
        let Some(dir) = std::env::var_os("MANDARA_CURSOR_ARCHIVES") else { return };
        let files: Vec<(String, PathBuf)> = std::fs::read_dir(dir).unwrap().flatten().map(|e| (sha256_file(&e.path()).unwrap(), e.path())).collect();
        let root = tempfile::tempdir().unwrap();
        let env = env(root.path());
        for t in super::super::catalog::CATALOG {
            for v in t.variants {
                let (_, file) = files.iter().find(|(sha, _)| sha == v.asset.sha256).unwrap_or_else(|| panic!("{}/{}: release file missing", t.id, v.id));
                assert_eq!(std::fs::metadata(file).unwrap().len(), v.asset.size, "{}/{}", t.id, v.id);
                install_archive(file, t.id, v, v.asset.sha256, &env, &|_| {}, &NO_CANCEL).unwrap_or_else(|e| panic!("{}/{}: {e}", t.id, v.id));
                for role in Role::ALL {
                    // Every theme here draws the shapes the gallery shows.
                    let found = super::super::preview(v.theme, role, 48, std::slice::from_ref(&env.icons));
                    assert!(found.is_some() || !Role::STRIP.contains(&role), "{}/{}: no {role:?}", t.id, v.id);
                }
                remove(v.theme, &env).unwrap();
            }
        }
        assert!(installed(&env).is_empty());
        assert_eq!(std::fs::read_dir(&env.icons).unwrap().count(), 0);
    }

    #[test]
    fn imports_external_archives_and_folders_unverified() {
        let root = tempfile::tempdir().unwrap();
        let env = env(&root.path().join("home"));
        // An archive: the theme keeps its own folder name; the script stays out.
        let report = import(&ImportSource::Path(good_archive(root.path())), &env, &|_| {}, &NO_CANCEL).unwrap();
        assert_eq!(report.installed.len(), 1);
        let entry = &report.installed[0];
        assert_eq!(entry.theme, "Fancy");
        assert!(!entry.verified && entry.catalog.is_empty() && entry.source.as_deref().is_some_and(|s| s.ends_with("fancy.tar.gz")) && !entry.sha256.is_empty());
        assert!(is_cursor_theme(&env.icons.join("Fancy")));
        assert!(!env.icons.join("install.sh").exists());
        // A folder with a theme and an escaping link.
        let mine = root.path().join("mine");
        super::super::tests::make_theme(&mine, "Mine-Cursors", &[("left_ptr", 0xff00_0000)], None);
        std::os::unix::fs::symlink("/etc/passwd", mine.join("Mine-Cursors/cursors/hand2")).unwrap();
        let r = import(&ImportSource::Path(mine.clone()), &env, &|_| {}, &NO_CANCEL).unwrap();
        assert_eq!(r.installed[0].theme, "Mine-Cursors");
        assert!(std::fs::symlink_metadata(env.icons.join("Mine-Cursors/cursors/hand2")).is_err());
        assert!(mine.join("Mine-Cursors/cursors/left_ptr").is_file(), "copied, not moved");
        // An icon theme or unreadable cursors are refused with the reason.
        let bad = root.path().join("bad");
        std::fs::create_dir_all(bad.join("B/cursors")).unwrap();
        std::fs::write(bad.join("B/index.theme"), "[Icon Theme]\n").unwrap();
        std::fs::write(bad.join("B/cursors/left_ptr"), "not a cursor").unwrap();
        assert!(import(&ImportSource::Path(bad), &env, &|_| {}, &NO_CANCEL).unwrap_err().contains("XCursor"));
        // A name a theme of yours already has: left alone.
        super::super::tests::make_theme(&env.icons, "Taken", &[("left_ptr", 0xff12_3456)], None);
        let taken = root.path().join("taken");
        super::super::tests::make_theme(&taken, "Taken", &[("left_ptr", 0xff00_0000)], None);
        let before = std::fs::read(env.icons.join("Taken/cursors/left_ptr")).unwrap();
        assert!(import(&ImportSource::Path(taken), &env, &|_| {}, &NO_CANCEL).is_err());
        assert_eq!(std::fs::read(env.icons.join("Taken/cursors/left_ptr")).unwrap(), before);
        assert!(import(&ImportSource::Url("http://example.org/c.tar.gz".into()), &env, &|_| {}, &NO_CANCEL).is_err(), "https only");
        let txt = root.path().join("x.txt");
        std::fs::write(&txt, "hi").unwrap();
        assert!(import(&ImportSource::Path(txt), &env, &|_| {}, &NO_CANCEL).is_err());
        // Imports remove like catalog themes.
        remove("Fancy", &env).unwrap();
        remove("Mine-Cursors", &env).unwrap();
        assert!(installed(&env).is_empty());
        let names: Vec<String> = std::fs::read_dir(&env.icons).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert_eq!(names, vec!["Taken"]);
    }

    #[test]
    fn scratch_folders_are_cleaned() {
        let root = tempfile::tempdir().unwrap();
        let env = env(root.path());
        std::fs::create_dir_all(env.icons.join(format!("{SCRATCH_PREFIX}1-2/x"))).unwrap();
        std::fs::create_dir_all(env.icons.join("Keep/cursors")).unwrap();
        clean_scratch(&env);
        let names: Vec<String> = std::fs::read_dir(&env.icons).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert_eq!(names, vec!["Keep"]);
    }
}
