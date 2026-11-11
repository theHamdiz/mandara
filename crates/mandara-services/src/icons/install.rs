//! Installing icon themes into `$XDG_DATA_HOME/icons/<Theme>/`, importing
//! ones you picked or pasted a link to, and removing them again.
//!
//! * **Catalog** — the pinned archive is streamed to
//!   `~/.cache/mandara/icon-themes/`, hashed as it arrives and kept only
//!   when size and SHA-256 match. Only the archive folders the variant
//!   needs are unpacked (into a hidden scratch folder beside the icons
//!   folder), merged into place, checked (`index.theme`, icons present),
//!   tidied (no executable bits, no symlink leaving the theme, no scripts
//!   or repository files) and moved in. `gtk-update-icon-cache` runs when
//!   installed.
//! * **Import** — an archive or folder you pick, or an `https://` link you
//!   paste (no checksum exists for those: they are marked unverified and
//!   Studio asks first). The same unpacking, checks and tidying apply;
//!   every icon theme found in it is installed.
//! * **Manifest** — `~/.local/share/mandara/icon-themes.json` records what
//!   Mandara installed and which folders it put there. A folder that
//!   exists but isn't Mandara's belongs to you or a package and is never
//!   replaced or removed. A folder several themes need (`Papirus` for
//!   `Papirus-Dark`) goes only with the last of them.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};

use super::IndexTheme;
use super::catalog::{IconTheme, IconVariant};
use crate::app_install::archive::ExtractOptions;
use crate::theme_pkg::{self, Pin, Scratch, cancelled};

/// Largest download accepted (the largest catalog archive is ~110 MB).
pub const MAX_DOWNLOAD: u64 = 200 << 20;
/// Largest total unpacked.
pub const MAX_UNPACKED: u64 = 2 << 30;
/// Most files, folders and links unpacked.
pub const MAX_ENTRIES: usize = 400_000;
/// How deep an import is searched for themes.
const IMPORT_DEPTH: usize = 3;
const MANIFEST_FILE: &str = "icon-themes.json";
/// Prefix of the scratch folders unpacking happens in.
const SCRATCH_PREFIX: &str = ".mandara-icons-";
/// Folder names never installed over (the fallback theme and the
/// default-cursor folder).
const RESERVED: [&str; 2] = ["hicolor", "default"];
/// Archive suffixes stripped to name a theme that sits at an archive's root.
const ARCHIVE_SUFFIXES: [&str; 7] = [".tar.gz", ".tar.xz", ".tar.zst", ".tar.bz2", ".tgz", ".txz", ".zip"];

fn limits(keep: Option<&dyn Fn(&Path) -> bool>) -> ExtractOptions<'_> {
    ExtractOptions { max_bytes: MAX_UNPACKED, max_entries: MAX_ENTRIES, keep }
}

/// Where themes go and where Mandara keeps its records.
#[derive(Clone, Debug)]
pub struct Env {
    /// `$XDG_DATA_HOME/icons` (`~/.local/share/icons`).
    pub icons: PathBuf,
    /// Downloads in progress (`~/.cache/mandara/icon-themes`).
    pub cache: PathBuf,
    /// The install manifest.
    pub manifest: PathBuf,
}

impl Env {
    pub fn current() -> Self {
        let data = dirs::data_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"));
        Self { icons: data.join("icons"), cache: mandara_config::paths::cache_dir().join("icon-themes"), manifest: mandara_config::paths::data_dir().join(MANIFEST_FILE) }
    }

    /// Everything under `root` (tests, sandboxes).
    pub fn under(root: &Path) -> Self {
        Self { icons: root.join("icons"), cache: root.join("cache"), manifest: root.join(MANIFEST_FILE) }
    }
}

/// Where an installed theme came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Source {
    Catalog { id: String, variant: String },
    File { path: PathBuf },
    Url { url: String },
}

/// A theme Mandara installed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    /// The icon theme name (its folder's name).
    pub theme: String,
    /// `Name=` from its `index.theme`.
    pub title: String,
    /// Every folder it needs, its own first.
    pub dirs: Vec<String>,
    pub source: Source,
    /// SHA-256 of the archive (empty for a folder).
    #[serde(default)]
    pub sha256: String,
    /// Checked against the catalog's pinned checksum.
    pub verified: bool,
    /// Unix seconds.
    pub installed_at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub themes: Vec<Installed>,
    /// The folders Mandara put in the icons folder (and may remove).
    #[serde(default)]
    pub owned: Vec<String>,
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

    fn owns(&self, dir: &str) -> bool {
        self.owned.iter().any(|o| o == dir)
    }

    fn record(&mut self, entry: Installed, placed: &[String]) {
        for p in placed {
            if !self.owns(p) {
                self.owned.push(p.clone());
            }
        }
        self.themes.retain(|i| i.theme != entry.theme);
        self.themes.push(entry);
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
    Install,
    Done,
}

/// An install's progress: bytes (or folders) of the current phase.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    pub phase: Phase,
    pub done: u64,
    pub total: u64,
}

impl Progress {
    /// Overall share done, `0..=1`.
    pub fn fraction(&self) -> f32 {
        const DOWNLOAD: f32 = 0.6;
        const UNPACK: f32 = 0.3;
        let part = if self.total == 0 { 0.0 } else { (self.done as f32 / self.total as f32).min(1.0) };
        match self.phase {
            Phase::Download => part * DOWNLOAD,
            Phase::Unpack => DOWNLOAD + part * UNPACK,
            Phase::Install => DOWNLOAD + UNPACK + part * (1.0 - DOWNLOAD - UNPACK),
            Phase::Done => 1.0,
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn installable_name(name: &str) -> bool {
    theme_pkg::valid_name(name) && !RESERVED.contains(&name)
}

/// Fills `%NAME%` in `dir/index.theme` with `name` (dashes as spaces), as
/// the theme's own script would.
fn fill_name(dir: &Path, name: &str) -> Result<(), String> {
    let path = dir.join("index.theme");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    std::fs::write(&path, text.replace("%NAME%", &name.replace('-', " "))).map_err(|e| e.to_string())
}

/// Checks the assembled theme `parent/name`, tidies it and returns its
/// index and the sibling folders its links point into.
fn finish(parent: &Path, name: &str, siblings: &[&str], cancel: &AtomicBool) -> Result<(IndexTheme, Vec<String>), String> {
    let dir = parent.join(name);
    let index = IndexTheme::read(&dir).map_err(|e| tf("{theme}: {error}", &[("theme", &name), ("error", &e)]))?;
    let tidied = theme_pkg::tidy_theme(parent, name, siblings, &index.top_dirs(), cancel)?;
    if !super::is_icon_theme(&dir) {
        return Err(tf("{theme} has no icons in the folders its index.theme lists", &[("theme", &name)]));
    }
    Ok((index, tidied.link_deps))
}

/// Moves `from` to `env.icons/name`, replacing Mandara's own copy.
fn place(from: &Path, env: &Env, name: &str, manifest: &Manifest) -> Result<PathBuf, String> {
    let target = env.icons.join(name);
    match std::fs::symlink_metadata(&target) {
        Ok(_) if !manifest.owns(name) => return Err(tf("{theme} is already installed (not by Mandara), so it was left as it is", &[("theme", &name)])),
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?,
        Ok(_) => std::fs::remove_file(&target).map_err(|e| format!("{}: {e}", target.display()))?,
        Err(_) => {}
    }
    std::fs::rename(from, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    // A cache speeds up lookups; themes work without one.
    theme_pkg::update_icon_cache(&target);
    Ok(target)
}

/// Installs `variant` of catalog theme `theme` from an already verified
/// `archive_path`.
pub fn install_archive(archive_path: &Path, theme: &IconTheme, variant: &IconVariant, sha256: &str, env: &Env, progress: &dyn Fn(Progress), cancel: &AtomicBool) -> Result<Installed, String> {
    if !variant.dirs.iter().all(|d| installable_name(d.name)) || variant.dirs.first().map(|d| d.name) != Some(variant.theme) {
        return Err(tf("“{theme}” isn't a valid theme name", &[("theme", &variant.theme)]));
    }
    let mut manifest = Manifest::load(&env.manifest);
    if exists(&env.icons.join(variant.theme)) && !manifest.owns(variant.theme) {
        return Err(tf("{theme} is already installed (not by Mandara), so it was left as it is", &[("theme", &variant.theme)]));
    }
    // Its own folder always; a folder it needs only when not there yet
    // (yours or a package's is used as it is).
    let to_place: Vec<&super::catalog::Dir> = variant.dirs.iter().enumerate().filter(|(i, d)| *i == 0 || !exists(&env.icons.join(d.name))).map(|(_, d)| d).collect();
    std::fs::create_dir_all(&env.icons).map_err(|e| format!("{}: {e}", env.icons.display()))?;
    let scratch = Scratch::in_dir(&env.icons, SCRATCH_PREFIX);
    let raw = scratch.0.join("raw");
    let out = scratch.0.join("out");
    let layers: Vec<&str> = to_place.iter().flat_map(|d| d.layers.iter().copied()).collect();
    let keep = |p: &Path| layers.iter().any(|l| p.starts_with(l) || Path::new(l).starts_with(p));
    let total = std::fs::metadata(archive_path).map(|m| m.len()).unwrap_or(0);
    progress(Progress { phase: Phase::Unpack, done: 0, total });
    // The unpacked size isn't known up front: reported against the
    // archive's size, capped.
    theme_pkg::unpack(archive_path, &raw, &limits(Some(&keep)), cancel, &|done| progress(Progress { phase: Phase::Unpack, done: done.min(total), total }))?;
    cancelled(cancel)?;
    let siblings: Vec<&str> = variant.dirs.iter().map(|d| d.name).collect();
    let steps = to_place.len() as u64;
    // Every folder is assembled before any is checked: a variant's links
    // point into its siblings.
    for dir in &to_place {
        let assembled = out.join(dir.name);
        for layer in dir.layers {
            let src = raw.join(layer);
            if !std::fs::symlink_metadata(&src).is_ok_and(|m| m.is_dir()) {
                return Err(tf("The download has no icon theme at {folder}", &[("folder", layer)]));
            }
            theme_pkg::merge_into(&src, &assembled)?;
        }
        if dir.fill_name {
            fill_name(&assembled, dir.name)?;
        }
    }
    // Siblings already installed stand in for themselves while checking.
    for dir in variant.dirs.iter().filter(|d| !to_place.iter().any(|p| p.name == d.name)) {
        std::os::unix::fs::symlink(env.icons.join(dir.name), out.join(dir.name)).map_err(|e| e.to_string())?;
    }
    let mut index = None;
    for (i, dir) in to_place.iter().enumerate() {
        progress(Progress { phase: Phase::Install, done: i as u64, total: steps });
        let (parsed, _) = finish(&out, dir.name, &siblings, cancel)?;
        if i == 0 {
            index = Some(parsed);
        }
    }
    cancelled(cancel)?;
    let mut placed = Vec::new();
    for dir in &to_place {
        place(&out.join(dir.name), env, dir.name, &manifest)?;
        placed.push(dir.name.to_string());
    }
    drop(scratch);
    let entry = Installed {
        theme: variant.theme.to_string(),
        title: index.map(|i| i.name).unwrap_or_else(|| variant.theme.to_string()),
        dirs: siblings.iter().map(|s| s.to_string()).collect(),
        source: Source::Catalog { id: theme.id.to_string(), variant: variant.id.to_string() },
        sha256: sha256.to_string(),
        verified: true,
        installed_at: now_secs(),
    };
    manifest.record(entry.clone(), &placed);
    manifest.save(&env.manifest)?;
    progress(Progress { phase: Phase::Done, done: 1, total: 1 });
    Ok(entry)
}

/// Downloads, verifies and installs `variant`. The download is deleted
/// afterwards (the installed folder is what previews read).
pub fn install(theme: &IconTheme, variant: &IconVariant, env: &Env, progress: &dyn Fn(Progress), cancel: &AtomicBool) -> Result<Installed, String> {
    let pin = Pin { sha256: theme.archive.sha256, size: theme.archive.size };
    let fetched = theme_pkg::fetch(theme.archive.url, Some(pin), MAX_DOWNLOAD, &env.cache, &|done, total| progress(Progress { phase: Phase::Download, done, total }), cancel)?;
    let result = install_archive(&fetched.path, theme, variant, theme.archive.sha256, env, progress, cancel);
    let _ = std::fs::remove_file(&fetched.path);
    result
}

/// Something to import.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportSource {
    /// An archive or a folder on this computer.
    Path(PathBuf),
    /// An `https://` link to an archive (no checksum: unverified).
    Url(String),
}

/// What an import did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    pub installed: Vec<Installed>,
    /// Themes found but not installed, with why.
    pub skipped: Vec<(String, String)>,
}

/// Folder names too generic to install a theme under (`Dark`, `src`):
/// such a theme takes its `Name=` instead.
const GENERIC_NAMES: [&str; 13] = ["dark", "light", "icons", "icon", "theme", "themes", "src", "dist", "default", "main", "master", "build", "imported"];

/// The folder name an imported theme installs under: its own, or, when
/// that is generic, its `Name=` with spaces as dashes.
fn import_name(dir: &Path) -> String {
    let own = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !GENERIC_NAMES.contains(&own.to_lowercase().as_str()) {
        return own;
    }
    IndexTheme::read(dir).ok().map(|i| i.name.split_whitespace().collect::<Vec<_>>().join("-")).filter(|n| installable_name(n)).unwrap_or(own)
}

/// `Foo.tar.gz` → `Foo`.
fn archive_stem(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    ARCHIVE_SUFFIXES.iter().find(|s| lower.ends_with(*s)).map(|s| name[..name.len() - s.len()].to_string()).unwrap_or_else(|| name.to_string())
}

/// Unpacks or copies `source` into `raw` and returns where it came from
/// and the archive's SHA-256 (empty for a folder). For a theme sitting at
/// the root of what was picked, it ends up in `raw/<name>`.
fn stage(source: &ImportSource, raw: &Path, env: &Env, progress: &dyn Fn(Progress), cancel: &AtomicBool) -> Result<(Source, String), String> {
    let unpack = |archive: &Path, stem: String| -> Result<(), String> {
        let total = std::fs::metadata(archive).map(|m| m.len()).unwrap_or(0);
        let into = raw.join("archive");
        theme_pkg::unpack(archive, &into, &limits(None), cancel, &|done| progress(Progress { phase: Phase::Unpack, done: done.min(total), total }))?;
        // A theme at the archive's root takes the archive's name.
        if into.join("index.theme").is_file() {
            let named = raw.join(if theme_pkg::valid_name(&stem) { stem } else { "imported".into() });
            std::fs::rename(&into, named).map_err(|e| e.to_string())?;
        }
        Ok(())
    };
    match source {
        ImportSource::Path(path) => {
            let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if meta.is_dir() {
                // Only the theme folders in it are copied (never a whole
                // folder tree picked by mistake).
                let themes = theme_pkg::find_dirs(path, IMPORT_DEPTH, &|d| d.join("index.theme").is_file());
                std::fs::create_dir_all(raw).map_err(|e| e.to_string())?;
                for (i, theme) in themes.iter().enumerate() {
                    let own = theme.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let own = if theme == path { name.clone() } else { own };
                    let into = raw.join(i.to_string()).join(if theme_pkg::valid_name(&own) { own } else { "imported".into() });
                    theme_pkg::copy_tree(theme, &into, &limits(None), cancel, &|_| {})?;
                }
                Ok((Source::File { path: path.clone() }, String::new()))
            } else if theme_pkg::is_archive(path) {
                if meta.len() > MAX_DOWNLOAD {
                    return Err(t("The archive is too large to install").to_string());
                }
                unpack(path, archive_stem(&name))?;
                Ok((Source::File { path: path.clone() }, theme_pkg::sha256_file(path)?))
            } else {
                Err(t("Pick an icon theme folder or an archive (.tar.gz, .tar.xz, .tar.zst, .tar.bz2 or .zip)").to_string())
            }
        }
        ImportSource::Url(url) => {
            let fetched = theme_pkg::fetch(url, None, MAX_DOWNLOAD, &env.cache, &|done, total| progress(Progress { phase: Phase::Download, done, total }), cancel)?;
            let last = url.split(['?', '#']).next().unwrap_or_default().rsplit('/').next().unwrap_or_default().to_string();
            let result = unpack(&fetched.path, archive_stem(&last));
            let _ = std::fs::remove_file(&fetched.path);
            result.map(|()| (Source::Url { url: url.clone() }, fetched.sha256))
        }
    }
}

/// Imports every icon theme in `source` (unverified: there is no pinned
/// checksum). Themes whose name is taken by one Mandara didn't install
/// are skipped. Fails when nothing could be installed.
pub fn import(source: &ImportSource, env: &Env, progress: &dyn Fn(Progress), cancel: &AtomicBool) -> Result<ImportReport, String> {
    std::fs::create_dir_all(&env.icons).map_err(|e| format!("{}: {e}", env.icons.display()))?;
    let scratch = Scratch::in_dir(&env.icons, SCRATCH_PREFIX);
    let raw = scratch.0.join("raw");
    let (origin, sha256) = stage(source, &raw, env, progress, cancel)?;
    cancelled(cancel)?;
    let found = theme_pkg::find_dirs(&raw, IMPORT_DEPTH, &|d| d.join("index.theme").is_file());
    if found.is_empty() {
        return Err(t("No icon theme found there (a folder with an index.theme)").to_string());
    }
    let mut manifest = Manifest::load(&env.manifest);
    let mut report = ImportReport::default();
    // Every theme folder, staged side by side so their links between each
    // other stay valid.
    let out = scratch.0.join("out");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let mut names: Vec<String> = Vec::new();
    for dir in &found {
        let name = import_name(dir);
        if !installable_name(&name) {
            report.skipped.push((name, t("not a usable theme name").to_string()));
            continue;
        }
        if names.contains(&name) {
            report.skipped.push((name, t("found twice").to_string()));
            continue;
        }
        if exists(&env.icons.join(&name)) && !manifest.owns(&name) {
            report.skipped.push((name.clone(), tf("{theme} is already installed (not by Mandara), so it was left as it is", &[("theme", &name)])));
            continue;
        }
        std::fs::rename(dir, out.join(&name)).map_err(|e| e.to_string())?;
        names.push(name);
    }
    let siblings: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut ready: Vec<(String, IndexTheme, Vec<String>)> = Vec::new();
    for name in &names {
        match finish(&out, name, &siblings, cancel) {
            Ok((index, deps)) => ready.push((name.clone(), index, deps)),
            Err(e) => {
                // A cursor theme was picked here by mistake: say where it goes.
                let cursor = out.join(name).join("cursors").is_dir();
                report.skipped.push((name.clone(), if cursor { t("a cursor theme: import it under Themes → Cursors").to_string() } else { e }));
            }
        }
    }
    cancelled(cancel)?;
    let steps = ready.len() as u64;
    for (i, (name, index, deps)) in ready.into_iter().enumerate() {
        progress(Progress { phase: Phase::Install, done: i as u64, total: steps });
        if let Err(e) = place(&out.join(&name), env, &name, &manifest) {
            report.skipped.push((name, e));
            continue;
        }
        let mut dirs = vec![name.clone()];
        dirs.extend(deps.into_iter().filter(|d| d != &name && names.contains(d)));
        let entry = Installed { theme: name.clone(), title: index.name, dirs, source: origin.clone(), sha256: sha256.clone(), verified: false, installed_at: now_secs() };
        manifest.record(entry.clone(), std::slice::from_ref(&name));
        report.installed.push(entry);
    }
    drop(scratch);
    // Folders that are themes but failed checks were never moved in.
    if report.installed.is_empty() {
        let why = report.skipped.iter().map(|(n, w)| format!("{n}: {w}")).collect::<Vec<_>>().join("; ");
        return Err(tf("Nothing was installed ({reasons})", &[("reasons", &why)]));
    }
    manifest.save(&env.manifest)?;
    progress(Progress { phase: Phase::Done, done: 1, total: 1 });
    Ok(report)
}

/// Removes a theme Mandara installed, with the folders only it needed.
/// Themes Mandara didn't install are refused, and only folders it put
/// there are deleted. Returns the folders removed.
pub fn remove(theme: &str, env: &Env) -> Result<Vec<String>, String> {
    let mut manifest = Manifest::load(&env.manifest);
    let refused = || tf("{theme} wasn't installed by Mandara, so it stays", &[("theme", &theme)]);
    let Some(entry) = manifest.get(theme).cloned() else { return Err(refused()) };
    if !installable_name(theme) {
        return Err(refused());
    }
    let others: Vec<&String> = manifest.themes.iter().filter(|i| i.theme != theme).flat_map(|i| &i.dirs).collect();
    let going: Vec<String> = entry.dirs.iter().filter(|d| installable_name(d) && manifest.owns(d) && !others.contains(d)).cloned().collect();
    for dir in &going {
        let path = env.icons.join(dir);
        match std::fs::symlink_metadata(&path) {
            // Replaced by a link since: drop the record, leave the link.
            Ok(m) if m.file_type().is_symlink() => {}
            Ok(_) => std::fs::remove_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?,
            Err(_) => {}
        }
    }
    manifest.owned.retain(|o| !going.contains(o));
    manifest.themes.retain(|i| i.theme != theme);
    manifest.save(&env.manifest)?;
    Ok(going)
}

/// Leftover scratch folders from an interrupted install.
pub fn clean_scratch(env: &Env) {
    theme_pkg::clean_scratch(&env.icons, SCRATCH_PREFIX);
}

#[cfg(test)]
mod tests {
    use super::super::catalog::{Asset, Dir, IconTheme, IconVariant, Style, Tone};
    use super::super::tests::INDEX;
    use super::*;
    use crate::app_install::archive::tests::tar_gz;

    static NO_CANCEL: AtomicBool = AtomicBool::new(false);
    const SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";

    static FANCY_DIRS: [Dir; 1] = [Dir { name: "Fancy", layers: &["pkg/Fancy"], fill_name: false }];
    static DARK_DIRS: [Dir; 2] = [Dir { name: "Fancy-Dark", layers: &["pkg/Fancy-Dark"], fill_name: false }, Dir { name: "Fancy", layers: &["pkg/Fancy"], fill_name: false }];
    static LAYERED_DIRS: [Dir; 1] = [Dir { name: "Layered", layers: &["pkg/src", "pkg/links"], fill_name: true }];
    static VARIANTS: [IconVariant; 3] = [
        IconVariant { id: "default", label: "Default", theme: "Fancy", dark: false, dirs: &FANCY_DIRS },
        IconVariant { id: "dark", label: "Dark", theme: "Fancy-Dark", dark: true, dirs: &DARK_DIRS },
        IconVariant { id: "layered", label: "Layered", theme: "Layered", dark: false, dirs: &LAYERED_DIRS },
    ];
    static THEME: IconTheme = IconTheme {
        id: "fancy",
        name: "Fancy",
        author: "Test",
        license: "MIT",
        homepage: "https://github.com/example/fancy",
        description: "Test",
        stars: 1,
        style: Style::Flat,
        tone: Tone::Both,
        swatch: 0,
        archive: Asset { url: "https://github.com/example/fancy/archive/1.tar.gz", sha256: "", size: 0 },
        variants: &VARIANTS,
    };

    /// An archive like a real release: two themes (Dark linking into the
    /// base), a layered `src` + `links` theme, an installer script, repo
    /// files and an escaping link.
    fn archive(dir: &Path) -> PathBuf {
        let dark_index = INDEX.replace("Name=Fancy", "Name=Fancy Dark").replace("Inherits=hicolor", "Inherits=Fancy,hicolor");
        let layered_index = INDEX.replace("Name=Fancy", "Name=%NAME%");
        let bytes = tar_gz(
            &[
                ("pkg/install.sh", b"#!/bin/sh\nrm -rf ~\n", 0o755),
                ("pkg/Fancy/index.theme", INDEX.as_bytes(), 0o644),
                ("pkg/Fancy/48/apps/firefox.svg", SVG, 0o755),
                ("pkg/Fancy/48/places/folder.svg", SVG, 0o644),
                ("pkg/Fancy/README.md", b"readme", 0o644),
                ("pkg/Fancy/LICENSE", b"MIT", 0o644),
                ("pkg/Fancy/scripts/build.sh", b"#!/bin/sh\n", 0o755),
                ("pkg/Fancy-Dark/index.theme", dark_index.as_bytes(), 0o644),
                ("pkg/Fancy-Dark/48/places/folder.svg", SVG, 0o644),
                ("pkg/src/index.theme", layered_index.as_bytes(), 0o644),
                ("pkg/src/48/apps/firefox.svg", SVG, 0o644),
            ],
            &[("pkg/Fancy-Dark/48/apps", "../../Fancy/48/apps"), ("pkg/links/48/apps/web-browser.svg", "firefox.svg"), ("pkg/Fancy/48/apps/evil.svg", "../../../install.sh")],
        );
        let path = dir.join("fancy.tar.gz");
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        v.sort();
        v
    }

    #[test]
    fn installs_only_the_theme_folders_tidied() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().unwrap();
        let env = Env::under(root.path());
        let seen = std::cell::RefCell::new(Vec::new());
        let entry = install_archive(&archive(root.path()), &THEME, &VARIANTS[0], "abc", &env, &|p| seen.borrow_mut().push(p.phase), &NO_CANCEL).unwrap();
        assert_eq!(entry.dirs, vec!["Fancy"]);
        assert!(entry.verified && entry.title == "Fancy");
        assert_eq!(names(&env.icons), vec!["Fancy"], "nothing else from the archive, no scratch left");
        let dir = env.icons.join("Fancy");
        assert!(super::super::is_icon_theme(&dir));
        assert!(dir.join("LICENSE").is_file(), "credits stay");
        for gone in ["README.md", "scripts", "48/apps/evil.svg"] {
            assert!(std::fs::symlink_metadata(dir.join(gone)).is_err(), "{gone}");
        }
        assert_eq!(std::fs::metadata(dir.join("48/apps/firefox.svg")).unwrap().permissions().mode() & 0o777, 0o644, "no executable bits");
        assert_eq!(seen.borrow().last(), Some(&Phase::Done));
        assert_eq!(installed(&env), vec![entry]);
    }

    #[test]
    fn a_variant_brings_the_folders_it_links_into_and_shares_them() {
        let root = tempfile::tempdir().unwrap();
        let env = Env::under(root.path());
        let a = archive(root.path());
        let dark = install_archive(&a, &THEME, &VARIANTS[1], "abc", &env, &|_| {}, &NO_CANCEL).unwrap();
        assert_eq!(dark.dirs, vec!["Fancy-Dark", "Fancy"]);
        assert_eq!(names(&env.icons), vec!["Fancy", "Fancy-Dark"]);
        assert!(env.icons.join("Fancy-Dark/48/apps").is_symlink() && env.icons.join("Fancy-Dark/48/apps/firefox.svg").is_file(), "the link into the base resolves");
        install_archive(&a, &THEME, &VARIANTS[0], "abc", &env, &|_| {}, &NO_CANCEL).unwrap();
        // Removing Dark keeps Fancy (the default variant needs it).
        assert_eq!(remove("Fancy-Dark", &env).unwrap(), vec!["Fancy-Dark"]);
        assert_eq!(names(&env.icons), vec!["Fancy"]);
        assert_eq!(remove("Fancy", &env).unwrap(), vec!["Fancy"]);
        assert!(names(&env.icons).is_empty());
        assert!(Manifest::load(&env.manifest).owned.is_empty());
        // The other order: the base first, then Dark links into it.
        install_archive(&a, &THEME, &VARIANTS[0], "abc", &env, &|_| {}, &NO_CANCEL).unwrap();
        let dark = install_archive(&a, &THEME, &VARIANTS[1], "abc", &env, &|_| {}, &NO_CANCEL).unwrap();
        assert_eq!(dark.dirs, vec!["Fancy-Dark", "Fancy"]);
        assert!(env.icons.join("Fancy-Dark/48/apps/firefox.svg").is_file());
        // Removing the base keeps it while Dark needs it.
        assert!(remove("Fancy", &env).unwrap().is_empty());
        assert!(env.icons.join("Fancy").is_dir());
        assert_eq!(remove("Fancy-Dark", &env).unwrap(), vec!["Fancy-Dark", "Fancy"]);
        assert!(names(&env.icons).is_empty());
    }

    #[test]
    fn layers_merge_and_the_name_is_filled_in() {
        let root = tempfile::tempdir().unwrap();
        let env = Env::under(root.path());
        let entry = install_archive(&archive(root.path()), &THEME, &VARIANTS[2], "abc", &env, &|_| {}, &NO_CANCEL).unwrap();
        assert_eq!(entry.title, "Layered");
        let dir = env.icons.join("Layered");
        assert!(dir.join("48/apps/web-browser.svg").is_symlink() && dir.join("48/apps/web-browser.svg").is_file());
        assert!(std::fs::read_to_string(dir.join("index.theme")).unwrap().contains("Name=Layered\n"));
    }

    #[test]
    fn never_replaces_or_removes_what_mandara_did_not_install() {
        let root = tempfile::tempdir().unwrap();
        let env = Env::under(root.path());
        super::super::tests::make_theme(&env.icons, "Fancy", INDEX, &["48/apps/mine.svg"]);
        assert!(install_archive(&archive(root.path()), &THEME, &VARIANTS[0], "abc", &env, &|_| {}, &NO_CANCEL).is_err());
        assert!(env.icons.join("Fancy/48/apps/mine.svg").is_file());
        // Dark installs, using your Fancy as it is (not taking it over).
        let dark = install_archive(&archive(root.path()), &THEME, &VARIANTS[1], "abc", &env, &|_| {}, &NO_CANCEL).unwrap();
        assert_eq!(dark.dirs, vec!["Fancy-Dark", "Fancy"]);
        assert_eq!(Manifest::load(&env.manifest).owned, vec!["Fancy-Dark"]);
        assert!(remove("Fancy", &env).is_err());
        assert!(remove("../Fancy", &env).is_err());
        remove("Fancy-Dark", &env).unwrap();
        assert!(env.icons.join("Fancy/48/apps/mine.svg").is_file(), "yours stays");
    }

    /// A `.tar.gz` whose one entry is named `name` (raw headers, so names
    /// `tar::Builder` refuses can be written).
    fn raw_tar_gz(name: &str, bytes: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
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
    fn path_traversal_and_bad_archives_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let env = Env::under(&root.path().join("home"));
        for evil in ["../escape", "pkg/../../escape", "/tmp/escape-mandara-icons"] {
            let path = root.path().join("evil.tar.gz");
            std::fs::write(&path, raw_tar_gz(evil, b"pwned")).unwrap();
            assert!(import(&ImportSource::Path(path.clone()), &env, &|_| {}, &NO_CANCEL).is_err(), "{evil}");
            assert!(!root.path().join("escape").exists() && !root.path().join("home/escape").exists(), "{evil}");
        }
        // Missing layers and a bad index are refused.
        static MISSING: [Dir; 1] = [Dir { name: "Nope", layers: &["pkg/Nope"], fill_name: false }];
        static MISSING_V: IconVariant = IconVariant { id: "x", label: "x", theme: "Nope", dark: false, dirs: &MISSING };
        assert!(install_archive(&archive(root.path()), &THEME, &MISSING_V, "abc", &env, &|_| {}, &NO_CANCEL).is_err());
        let bad = root.path().join("bad.tar.gz");
        std::fs::write(&bad, tar_gz(&[("pkg/Fancy/index.theme", b"[Icon Theme]\nName=%NAME%\nDirectories=48/apps\n", 0o644), ("pkg/Fancy/48/apps/a.svg", SVG, 0o644)], &[])).unwrap();
        assert!(install_archive(&bad, &THEME, &VARIANTS[0], "abc", &env, &|_| {}, &NO_CANCEL).is_err());
        static EVIL_NAME: [Dir; 1] = [Dir { name: "../Evil", layers: &["pkg/Fancy"], fill_name: false }];
        static EVIL_V: IconVariant = IconVariant { id: "x", label: "x", theme: "../Evil", dark: false, dirs: &EVIL_NAME };
        assert!(install_archive(&archive(root.path()), &THEME, &EVIL_V, "abc", &env, &|_| {}, &NO_CANCEL).is_err());
        // No scratch folders survive the failures.
        assert!(names(&env.icons).is_empty(), "{:?}", names(&env.icons));
        assert!(installed(&env).is_empty());
    }

    #[test]
    fn oversized_and_crowded_archives_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("o");
        let a = archive(root.path());
        let small = ExtractOptions { max_bytes: 8, max_entries: MAX_ENTRIES, keep: None };
        assert!(theme_pkg::unpack(&a, &dest, &small, &NO_CANCEL, &|_| {}).unwrap_err().contains("too large"));
        let few = ExtractOptions { max_bytes: MAX_UNPACKED, max_entries: 3, keep: None };
        assert!(theme_pkg::unpack(&a, &root.path().join("o2"), &few, &NO_CANCEL, &|_| {}).unwrap_err().contains("too many"));
        // A sparse file past the download cap.
        let big = root.path().join("big.tar.gz");
        std::fs::write(&big, [0x1f, 0x8b]).unwrap();
        std::fs::OpenOptions::new().write(true).open(&big).unwrap().set_len(MAX_DOWNLOAD + 1).unwrap();
        let env = Env::under(&root.path().join("home"));
        assert!(import(&ImportSource::Path(big), &env, &|_| {}, &NO_CANCEL).unwrap_err().contains("too large"));
    }

    #[test]
    fn imports_archives_and_folders_and_skips_what_is_not_ours() {
        let root = tempfile::tempdir().unwrap();
        let env = Env::under(&root.path().join("home"));
        let report = import(&ImportSource::Path(archive(root.path())), &env, &|_| {}, &NO_CANCEL).unwrap();
        let got: Vec<&str> = report.installed.iter().map(|i| i.theme.as_str()).collect();
        assert_eq!(got, vec!["Fancy", "Fancy-Dark"]);
        assert!(report.installed.iter().all(|i| !i.verified && matches!(i.source, Source::File { .. }) && !i.sha256.is_empty()));
        assert_eq!(report.installed[1].dirs, vec!["Fancy-Dark", "Fancy"], "a link dependency inside the import");
        // `src` has a %NAME% placeholder: refused with the reason.
        assert!(report.skipped.iter().any(|(n, _)| n == "src"));
        assert!(!env.icons.join("install.sh").exists());
        // A folder: the theme folder itself, under its own name.
        let mine = root.path().join("mine");
        super::super::tests::make_theme(&mine, "Folder-Theme", INDEX, &["48/apps/a.svg"]);
        std::os::unix::fs::symlink("/etc/passwd", mine.join("Folder-Theme/48/apps/b.svg")).unwrap();
        let r = import(&ImportSource::Path(mine.join("Folder-Theme")), &env, &|_| {}, &NO_CANCEL).unwrap();
        assert_eq!(r.installed[0].theme, "Folder-Theme");
        assert!(std::fs::symlink_metadata(env.icons.join("Folder-Theme/48/apps/b.svg")).is_err());
        assert!(mine.join("Folder-Theme/48/apps/a.svg").is_file(), "the picked folder is copied, not moved");
        // Its name now taken by your own theme: skipped, nothing replaced.
        super::super::tests::make_theme(&env.icons, "Other", INDEX, &["48/apps/mine.svg"]);
        let mut other = root.path().join("o");
        super::super::tests::make_theme(&other, "Other", INDEX, &["48/apps/x.svg"]);
        other.push("Other");
        assert!(import(&ImportSource::Path(other), &env, &|_| {}, &NO_CANCEL).is_err());
        assert!(env.icons.join("Other/48/apps/mine.svg").is_file());
        // Something that isn't a theme.
        let empty = root.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(import(&ImportSource::Path(empty), &env, &|_| {}, &NO_CANCEL).is_err());
        let not = root.path().join("x.txt");
        std::fs::write(&not, "hello").unwrap();
        assert!(import(&ImportSource::Path(not), &env, &|_| {}, &NO_CANCEL).is_err());
        assert!(import(&ImportSource::Url("http://example.org/x.tar.gz".into()), &env, &|_| {}, &NO_CANCEL).is_err(), "https only");
        // A cursor theme is pointed to the cursor gallery.
        let cur = root.path().join("cur");
        std::fs::create_dir_all(cur.join("C/cursors")).unwrap();
        std::fs::write(cur.join("C/index.theme"), "[Icon Theme]\nName=C\n").unwrap();
        let err = import(&ImportSource::Path(cur.join("C")), &env, &|_| {}, &NO_CANCEL).unwrap_err();
        assert!(err.contains("cursor"), "{err}");
        // Imports remove like catalog themes.
        remove("Fancy-Dark", &env).unwrap();
        remove("Fancy", &env).unwrap();
        remove("Folder-Theme", &env).unwrap();
        assert_eq!(names(&env.icons), vec!["Other"]);
    }

    #[test]
    fn generic_folders_take_their_theme_name() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("z.tar.gz");
        let dark = INDEX.replace("Name=Fancy", "Name=Zafiro icons Dark");
        std::fs::write(&a, tar_gz(&[("Zafiro/Dark/index.theme", dark.as_bytes(), 0o644), ("Zafiro/Dark/48/apps/a.svg", SVG, 0o644), ("Zafiro/Fancy/index.theme", INDEX.as_bytes(), 0o644), ("Zafiro/Fancy/48/apps/a.svg", SVG, 0o644)], &[])).unwrap();
        let env = Env::under(&root.path().join("home"));
        let r = import(&ImportSource::Path(a), &env, &|_| {}, &NO_CANCEL).unwrap();
        let got: Vec<&str> = r.installed.iter().map(|i| i.theme.as_str()).collect();
        assert_eq!(got, vec!["Zafiro-icons-Dark", "Fancy"]);
        assert!(env.icons.join("Zafiro-icons-Dark/48/apps/a.svg").is_file());
    }

    #[test]
    fn root_themes_take_the_archive_name() {
        assert_eq!(archive_stem("Nordic-Icons.tar.xz"), "Nordic-Icons");
        assert_eq!(archive_stem("x.ZIP"), "x");
        assert_eq!(archive_stem("plain"), "plain");
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Rooted.tar.gz");
        std::fs::write(&path, tar_gz(&[("index.theme", INDEX.as_bytes(), 0o644), ("48/apps/a.svg", SVG, 0o644)], &[])).unwrap();
        let env = Env::under(&root.path().join("home"));
        let r = import(&ImportSource::Path(path), &env, &|_| {}, &NO_CANCEL).unwrap();
        assert_eq!(r.installed[0].theme, "Rooted");
    }

    #[test]
    fn cancel_leaves_nothing_behind() {
        let root = tempfile::tempdir().unwrap();
        let env = Env::under(root.path());
        assert!(install_archive(&archive(root.path()), &THEME, &VARIANTS[0], "abc", &env, &|_| {}, &AtomicBool::new(true)).is_err());
        assert!(names(&env.icons).is_empty());
    }

    #[test]
    fn progress_fraction_is_monotonic() {
        let p = |phase, done, total| Progress { phase, done, total }.fraction();
        assert_eq!(p(Phase::Download, 0, 100), 0.0);
        assert!(p(Phase::Download, 100, 100) <= p(Phase::Unpack, 0, 100));
        assert!(p(Phase::Unpack, 100, 100) <= p(Phase::Install, 0, 1));
        assert!(p(Phase::Install, 1, 1) <= p(Phase::Done, 1, 1));
        assert_eq!(p(Phase::Download, 5, 0), 0.0);
    }

    /// With `MANDARA_ICON_ARCHIVES` naming a folder of the real release
    /// files (any names; matched by SHA-256), every catalog variant
    /// installs, previews and removes cleanly.
    #[test]
    fn every_catalog_variant_installs_from_its_release() {
        let Some(dir) = std::env::var_os("MANDARA_ICON_ARCHIVES") else { return };
        let files: Vec<(String, PathBuf)> = std::fs::read_dir(dir).unwrap().flatten().map(|e| (theme_pkg::sha256_file(&e.path()).unwrap(), e.path())).collect();
        let root = tempfile::tempdir_in(std::env::var_os("MANDARA_ICON_TMP").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)).unwrap();
        let env = Env::under(root.path());
        let only = std::env::var("MANDARA_ICON_ONLY").ok();
        for t in super::super::catalog::CATALOG.iter().filter(|t| only.as_deref().is_none_or(|o| o.split(',').any(|id| id == t.id))) {
            let (_, file) = files.iter().find(|(sha, _)| sha == t.archive.sha256).unwrap_or_else(|| panic!("{}: release file missing", t.id));
            assert_eq!(std::fs::metadata(file).unwrap().len(), t.archive.size, "{}", t.id);
            for v in t.variants {
                let entry = install_archive(file, t, v, t.archive.sha256, &env, &|_| {}, &NO_CANCEL).unwrap_or_else(|e| panic!("{}/{}: {e}", t.id, v.id));
                let previews = super::super::preview_paths(v.theme, 48, std::slice::from_ref(&env.icons));
                let shown = previews.iter().flatten().count();
                eprintln!("{}/{}: {} folders, {shown}/{} previews, title {:?}", t.id, v.id, entry.dirs.len(), previews.len(), entry.title);
                assert!(shown > 0, "{}/{}: no preview icon at all", t.id, v.id);
                remove(v.theme, &env).unwrap();
                assert!(names(&env.icons).is_empty(), "{}/{}: left {:?}", t.id, v.id, names(&env.icons));
            }
        }
        assert!(installed(&env).is_empty());
    }
}
