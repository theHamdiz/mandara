//! The app installer engine: install apps from files, macOS-style.
//!
//! [`inspect`] detects a file's format by its magic bytes
//! ([`detect`]) and reads what it is — name, version, icon, architecture —
//! without running anything: AppImages through their embedded squashfs
//! ([`appimage`]), archives in one streaming pass ([`archive`]), Flatpak
//! bundles / refs ([`flatpak`]) and native packages ([`native`]) from their
//! metadata. [`plan`] then says exactly what installing does; [`execute`]
//! does it, reporting [`Progress`] and honouring a cancel flag.
//!
//! * **AppImage / portable archives** — rootless: copied / unpacked into
//!   `~/.local/share/mandara/apps/<id>/` (configurable) through a staging
//!   folder renamed into place, with a desktop entry
//!   (`~/.local/share/applications/mandara-<id>.desktop`, absolute
//!   `Exec=`), its icon in `~/.local/share/icons/hicolor/<size>/apps/` and
//!   an optional PATH shim (`~/.local/bin/<name>`, never replacing a
//!   different file). Failures roll back.
//! * **Flatpak** — `flatpak install --user --noninteractive` (rootless).
//! * **Native packages** — only through the system's own package manager
//!   via `pkexec`, after its own simulation; foreign packages are refused
//!   with an explanation.
//!
//! Everything installed is recorded in [`registry`], which the uninstaller
//! uses to remove exactly those files. Commands go through
//! [`crate::app_uninstall::Runner`] (tests mock it). The dropped file is
//! never executed. `MANDARA_INSTALL_DRY_RUN=1` forces dry runs everywhere.

pub mod appimage;
pub mod archive;
pub mod detect;
pub mod flatpak;
pub mod meta;
pub mod native;
pub mod registry;

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

pub use detect::{Arch, Family, Format, Unsupported};
pub use meta::IconData;

use crate::app_uninstall::{CmdOutput, Runner};

/// The first-party plugin's id (its settings live in
/// `[plugins.settings.app-installer]`).
pub const PLUGIN_ID: &str = "app-installer";
/// Forces dry runs in every process that reads it.
pub const DRY_RUN_ENV: &str = "MANDARA_INSTALL_DRY_RUN";
pub(crate) const PKEXEC: &str = "pkexec";
/// Package managers and Flatpak wait for downloads and polkit.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
/// Desktop entries and icons Mandara writes start with this.
pub const ENTRY_PREFIX: &str = "mandara-";
const STAGE_PREFIX: &str = ".stage-";
const BACKUP_PREFIX: &str = ".old-";
/// Highest `-<n>` suffix tried for "Keep both".
const MAX_COPIES: u32 = 99;
/// Lines of a failing command's output shown.
const ERROR_LINES: usize = 4;
const EXEC_MODE: u32 = 0o755;
const FILE_MODE: u32 = 0o644;
/// Desktop entry markers.
const MARK_ID: &str = "X-Mandara-App-Id";
const MARK_FORMAT: &str = "X-Mandara-Format";
/// Default settings.
pub const DEFAULT_FLATPAK_SCOPE: &str = "user";
pub const AFTER_KEEP: &str = "keep";
pub const AFTER_TRASH: &str = "trash";

/// True when `MANDARA_INSTALL_DRY_RUN` is set to anything but `0` / empty.
pub fn forced_dry_run() -> bool {
    std::env::var(DRY_RUN_ENV).is_ok_and(|v| !v.is_empty() && v != "0")
}

// ── Settings ─────────────────────────────────────────────────────────────

/// `[plugins.settings.app-installer]`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InstallSettings {
    /// Where rootless apps go; empty = `~/.local/share/mandara/apps`.
    pub install_dir: String,
    /// "Add to PATH" starts switched on.
    pub path_shims: bool,
    /// Ask before rootless installs (native ones always ask).
    pub confirm_rootless: bool,
    /// `user` or `system`.
    pub flatpak_scope: String,
    pub allow_native: bool,
    /// `keep` or `trash` the original file afterwards.
    pub after_install: String,
    pub notify: bool,
}

impl Default for InstallSettings {
    fn default() -> Self {
        Self { install_dir: String::new(), path_shims: false, confirm_rootless: true, flatpak_scope: DEFAULT_FLATPAK_SCOPE.into(), allow_native: true, after_install: AFTER_KEEP.into(), notify: true }
    }
}

impl InstallSettings {
    pub fn from_plugins(p: &mandara_config::PluginsConfig) -> Self {
        let d = Self::default();
        Self {
            install_dir: p.setting(PLUGIN_ID, "install_dir", d.install_dir),
            path_shims: p.setting(PLUGIN_ID, "path_shims", d.path_shims),
            confirm_rootless: p.setting(PLUGIN_ID, "confirm_rootless", d.confirm_rootless),
            flatpak_scope: p.setting(PLUGIN_ID, "flatpak_scope", d.flatpak_scope),
            allow_native: p.setting(PLUGIN_ID, "allow_native", d.allow_native),
            after_install: p.setting(PLUGIN_ID, "after_install", d.after_install),
            notify: p.setting(PLUGIN_ID, "notify", d.notify),
        }
    }

    pub fn flatpak_user(&self) -> bool {
        self.flatpak_scope != "system"
    }

    pub fn trash_after(&self) -> bool {
        self.after_install == AFTER_TRASH
    }
}

// ── Environment ──────────────────────────────────────────────────────────

/// Where installs go; tests build one on a temp `$HOME`.
#[derive(Clone, Debug)]
pub struct InstallEnv {
    pub home: PathBuf,
    pub data_home: PathBuf,
    pub cache_home: PathBuf,
    /// Rootless apps' folders.
    pub apps_dir: PathBuf,
    pub bin_dir: PathBuf,
    pub host: Arch,
    pub force_dry_run: bool,
}

impl InstallEnv {
    pub fn with_home(home: &Path) -> Self {
        let data_home = home.join(".local/share");
        Self { home: home.to_path_buf(), apps_dir: registry::apps_root(&data_home), data_home, cache_home: home.join(".cache"), bin_dir: home.join(".local/bin"), host: Arch::host(), force_dry_run: false }
    }

    /// This user's folders, the configured install folder and
    /// [`forced_dry_run`].
    pub fn current(settings: &InstallSettings) -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let mut env = Self::with_home(&home);
        if let Some(d) = dirs::data_dir() {
            env.apps_dir = registry::apps_root(&d);
            env.data_home = d;
        }
        if let Some(d) = dirs::cache_dir() {
            env.cache_home = d;
        }
        if let Some(d) = dirs::executable_dir() {
            env.bin_dir = d;
        }
        let custom = settings.install_dir.trim();
        if !custom.is_empty() {
            env.apps_dir = match custom.strip_prefix("~/") {
                Some(rest) => home.join(rest),
                None => PathBuf::from(custom),
            };
        }
        env.force_dry_run = forced_dry_run() || crate::app_uninstall::forced_dry_run();
        env
    }

    pub fn applications_dir(&self) -> PathBuf {
        self.data_home.join("applications")
    }

    pub fn hicolor_dir(&self) -> PathBuf {
        self.data_home.join("icons").join("hicolor")
    }

    /// Where the confirmation sheet's icon / screenshot previews go.
    pub fn preview_dir(&self) -> PathBuf {
        self.cache_home.join("mandara").join("installer")
    }

    /// The uninstaller's environment over the same folders (for the Trash).
    pub fn uninstall_env(&self) -> crate::app_uninstall::Env {
        let mut env = crate::app_uninstall::Env::with_home(&self.home);
        env.data_home = self.data_home.clone();
        env.cache_home = self.cache_home.clone();
        env
    }
}

// ── Detection ────────────────────────────────────────────────────────────

/// Cheap: whether `path` is a file Mandara can install (magic-byte sniff
/// of the first page; nothing decompressed or run).
pub fn accepts(path: &Path) -> bool {
    path.is_file() && detect::detect(path).is_ok()
}

/// File-picker globs for every supported format.
pub fn picker_globs() -> Vec<&'static str> {
    Format::ALL.iter().flat_map(|f| f.globs().iter().copied()).collect()
}

/// MIME types of the supported formats (for portals that filter by type).
pub const MIME_TYPES: &[&str] = &[
    "application/vnd.appimage",
    "application/x-iso9660-appimage",
    "application/vnd.flatpak",
    "application/vnd.flatpak.ref",
    "application/x-compressed-tar",
    "application/x-xz-compressed-tar",
    "application/x-zstd-compressed-tar",
    "application/zip",
    "application/vnd.debian.binary-package",
    "application/x-rpm",
    "application/x-alpine-package",
];

fn unsupported_message(path: &Path, why: Option<Unsupported>) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    match why {
        Some(Unsupported::AppImageType1) => t("This is an old (type 1) AppImage, which Mandara can't read safely. Download a newer build of the app.").to_string(),
        Some(Unsupported::Snap) => t("Snap packages install from the Snap Store with snap, not from files.").to_string(),
        Some(Unsupported::BareElf) => tf("{file} is a program, not an app package: move it to ~/.local/bin to use it.", &[("file", &name)]),
        None => tf("{file} isn't an app Mandara can install (AppImage, Flatpak, archive or package)", &[("file", &name)]),
    }
}

// ── Inspection ───────────────────────────────────────────────────────────

/// What a file is.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Inspection {
    pub path: PathBuf,
    pub file_name: String,
    pub format: Option<Format>,
    pub family: Option<Family>,
    /// The file's size.
    pub size: u64,
    /// Unpacked size, when known (archives).
    pub unpacked: Option<u64>,
    pub id: String,
    pub name: String,
    pub version: String,
    pub summary: String,
    pub arch: Option<Arch>,
    /// A preview copy of the app's icon (for the confirmation card).
    pub icon_file: Option<PathBuf>,
    #[serde(skip)]
    pub icon: Option<IconData>,
    /// Suggested PATH command.
    pub command_name: Option<String>,
    #[serde(skip)]
    pub desktop_text: Option<String>,
    pub flatpak: Option<flatpak::FlatpakInfo>,
    pub package: Option<native::PackageMeta>,
    /// Longer description (AppStream / package), plain text, one paragraph
    /// or `• ` item per line.
    pub description: String,
    /// Who makes it (AppStream developer, package maintainer).
    pub developer: String,
    /// Freedesktop categories (`Office`, `Game`, …).
    pub categories: Vec<String>,
    /// Downscaled previews of screenshots shipped in the file (at most
    /// [`meta::MAX_SCREENSHOTS`]; written next to the icon preview).
    pub screenshots: Vec<PathBuf>,
}

impl Inspection {
    pub fn format(&self) -> Format {
        self.format.unwrap_or(Format::AppImage)
    }

    pub fn family(&self) -> Family {
        self.family.unwrap_or(Family::AppImage)
    }

    /// Badge text: `AppImage`, `Flatpak`, `Portable`, `Native (pacman)`.
    pub fn badge(&self, manager: Option<crate::app_uninstall::Source>) -> String {
        match (self.family(), manager) {
            (Family::Native, Some(m)) => format!("{} ({})", Family::Native.label(), m.label()),
            (f, _) => f.label().to_string(),
        }
    }
}

fn file_stem_clean(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let lower = name.to_lowercase();
    let mut stem = name.clone();
    for ext in [".appimage", ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.zst", ".tzst", ".zip", ".flatpakref", ".flatpak"] {
        if lower.ends_with(ext) {
            stem = name[..name.len() - ext.len()].to_string();
            break;
        }
    }
    let stripped = meta::strip_version_suffix(&stem);
    // `-linux`, `-x86_64` left over after the version is cut.
    stripped.trim_end_matches("-linux").trim_end_matches("-x86_64").trim_end_matches("_x86_64").to_string()
}

/// Screenshot previews fit in this box (logical px of a sheet twice over).
const SCREENSHOT_PREVIEW_W: u32 = 960;
const SCREENSHOT_PREVIEW_H: u32 = 600;

/// Writes a downscaled screenshot preview (`None`: not an image).
fn write_screenshot(env: &InstallEnv, id: &str, n: usize, bytes: &[u8]) -> Option<PathBuf> {
    let img = image::load_from_memory(bytes).ok()?;
    let small = if img.width() > SCREENSHOT_PREVIEW_W || img.height() > SCREENSHOT_PREVIEW_H { img.thumbnail(SCREENSHOT_PREVIEW_W, SCREENSHOT_PREVIEW_H) } else { img };
    let dir = env.preview_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{id}-shot{n}.png"));
    small.save_with_format(&path, image::ImageFormat::Png).ok()?;
    Some(path)
}

/// The preview files an inspection wrote (its icon and screenshots), only
/// those inside `env`'s preview folder.
pub fn preview_files(i: &Inspection, env: &InstallEnv) -> Vec<PathBuf> {
    let dir = env.preview_dir();
    i.icon_file.iter().chain(&i.screenshots).filter(|p| p.starts_with(&dir)).cloned().collect()
}

/// Deletes preview files (missing ones are fine); never anything outside
/// the preview folder.
pub fn remove_previews(paths: &[PathBuf], env: &InstallEnv) {
    let dir = env.preview_dir();
    for p in paths.iter().filter(|p| p.starts_with(&dir) && p.parent() == Some(dir.as_path())) {
        let _ = std::fs::remove_file(p);
    }
}

/// Deletes previews older than `max_age` (left by a crash or a hidden
/// job); folders (native scratch repositories) are left alone. Returns how
/// many went.
pub fn prune_previews(env: &InstallEnv, max_age: std::time::Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(env.preview_dir()) else { return 0 };
    let now = std::time::SystemTime::now();
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| e.metadata().and_then(|m| m.modified()).is_ok_and(|m| now.duration_since(m).is_ok_and(|age| age > max_age)))
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

/// Desktop-entry `Categories=` (`Office;TextEditor;`).
fn desktop_categories(text: &str) -> Vec<String> {
    meta::desktop_value(text, "Categories").map(|c| c.split(';').map(str::trim).filter(|c| !c.is_empty()).map(String::from).collect()).unwrap_or_default()
}

/// Writes the icon preview used by the confirmation card.
fn write_preview(env: &InstallEnv, id: &str, icon: &IconData) -> Option<PathBuf> {
    let dir = env.preview_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{id}.{}", icon.kind.extension()));
    std::fs::write(&path, &icon.bytes).ok()?;
    Some(path)
}

/// Reads what `path` is (runs nothing; native packages are read from their
/// metadata, not the package manager).
pub fn inspect(path: &Path, env: &InstallEnv) -> Result<Inspection, String> {
    let path = std::fs::canonicalize(path).map_err(|e| tf("Can't open {path}: {error}", &[("path", &path.display()), ("error", &e)]))?;
    let format = detect::detect(&path).map_err(|why| unsupported_message(&path, why))?;
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let stem = file_stem_clean(&path);
    let mut i = Inspection {
        file_name: path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        path: path.clone(),
        format: Some(format),
        family: Some(format.family()),
        size,
        name: stem.clone(),
        ..Default::default()
    };
    let apply_desktop = |i: &mut Inspection, text: &str| {
        if let Some(n) = meta::desktop_value(text, "Name").filter(|n| !n.is_empty()) {
            i.name = n;
        }
        if let Some(c) = meta::desktop_value(text, "Comment").filter(|_| i.summary.is_empty()) {
            i.summary = c;
        }
        if let Some(v) = meta::desktop_value(text, "X-AppImage-Version").or_else(|| meta::desktop_value(text, "Version").filter(|v| v != "1.0" && v != "1.5")).filter(|_| i.version.is_empty()) {
            i.version = v;
        }
        if let Some(exec) = meta::desktop_value(text, "Exec") {
            i.command_name = meta::shim_name(&meta::split_exec(&exec).0);
        }
        if i.categories.is_empty() {
            i.categories = desktop_categories(text);
        }
        i.desktop_text = Some(text.to_string());
    };
    let apply_stream = |i: &mut Inspection, stream: &meta::AppStream| {
        if !stream.description.is_empty() {
            i.description = stream.description.clone();
        }
        if !stream.developer.is_empty() {
            i.developer = stream.developer.clone();
        }
        if !stream.categories.is_empty() {
            i.categories = stream.categories.clone();
        }
    };
    let mut shots: Vec<Vec<u8>> = Vec::new();
    match format.family() {
        Family::AppImage => {
            let m = appimage::read(&path)?;
            i.arch = m.arch;
            if let Some(stream) = &m.appstream {
                i.version = stream.version.clone();
                i.summary = stream.summary.clone();
            }
            if let Some((file, text)) = &m.desktop {
                apply_desktop(&mut i, text);
                i.id = m.appstream.as_ref().map(|s| s.id.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| file.trim_end_matches(".desktop").to_string());
            }
            if let Some(stream) = m.appstream.as_ref().filter(|s| !s.name.is_empty()) {
                i.name = stream.name.clone();
            }
            if let Some(stream) = &m.appstream {
                apply_stream(&mut i, stream);
            }
            i.icon = m.icon;
            shots = m.screenshots;
        }
        Family::Portable => {
            let peek = archive::peek(&path, format, &stem.to_lowercase())?;
            i.unpacked = Some(peek.total);
            i.arch = peek.arch;
            if let Some((_, text)) = &peek.desktop {
                apply_desktop(&mut i, text);
            }
            i.icon = peek.icon.and_then(|(_, b)| IconData::from_bytes(b));
            i.id = stem.clone();
            if i.command_name.is_none() {
                i.command_name = meta::shim_name(&stem);
            }
        }
        Family::Flatpak => {
            let (info, extras) = if format == Format::FlatpakRef { (flatpak::read_ref(&path), None) } else { flatpak::read_bundle_full(&path).map_or((None, None), |(i, e)| (Some(i), Some(e))) };
            let info = info.ok_or_else(|| t("This Flatpak file doesn't name an app").to_string())?;
            if let Some(extras) = extras {
                if let Some(stream) = &extras.appstream {
                    apply_stream(&mut i, stream);
                }
                i.icon = extras.icon;
            }
            i.id = info.id.clone();
            if !info.title.is_empty() {
                i.name = info.title.clone();
            } else {
                i.name = info.id.rsplit('.').next().unwrap_or(&info.id).to_string();
            }
            i.summary = info.summary.clone();
            i.version = info.branch.clone();
            i.arch = Arch::from_package_word(&info.arch);
            i.command_name = meta::shim_name(&i.name);
            i.flatpak = Some(info);
        }
        Family::Native => {
            let m = native::meta(&path, format);
            i.id = m.name.clone();
            i.name = if m.name.is_empty() { stem.clone() } else { m.name.clone() };
            i.version = m.version.clone();
            i.summary = m.summary.clone();
            i.arch = m.arch;
            i.description = m.description.clone();
            i.developer = m.maintainer.clone();
            i.unpacked = m.installed_size;
            i.package = Some(m);
        }
    }
    if i.version.is_empty()
        && let Some(v) = version_from_name(&i.file_name)
    {
        i.version = v;
    }
    i.id = meta::sanitize_id(&i.id).or_else(|| meta::sanitize_id(&stem)).or_else(|| meta::sanitize_id(&i.name)).unwrap_or_else(|| "app".into());
    if i.command_name.is_none() && i.family().is_rootless_files() {
        i.command_name = meta::shim_name(&i.name);
    }
    if let Some(icon) = &i.icon {
        i.icon_file = write_preview(env, &i.id, icon);
    }
    // A description that only repeats the summary adds nothing.
    if i.description.trim() == i.summary.trim() {
        i.description.clear();
    }
    i.screenshots = shots.iter().enumerate().filter_map(|(n, b)| write_screenshot(env, &i.id, n, b)).collect();
    Ok(i)
}

/// Orders version strings by their numeric parts (`1.10` > `1.9`); text
/// parts compare as text, and an empty version is unknown (equal).
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if a.is_empty() || b.is_empty() {
        return Ordering::Equal;
    }
    let parts = |v: &str| v.trim_start_matches(['v', 'V']).split(['.', '-', '_', '+', '~']).map(str::to_string).collect::<Vec<_>>();
    let (pa, pb) = (parts(a), parts(b));
    for n in 0..pa.len().max(pb.len()) {
        let (x, y) = (pa.get(n).map_or("0", String::as_str), pb.get(n).map_or("0", String::as_str));
        let o = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            _ => x.cmp(y),
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

/// `Obsidian-1.6.7.AppImage` → `1.6.7`.
fn version_from_name(name: &str) -> Option<String> {
    let start = name.char_indices().find(|(i, c)| c.is_ascii_digit() && *i > 0 && name[..*i].ends_with(['-', '_', 'v']))?.0;
    let v: String = name[start..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let v = v.trim_end_matches('.').to_string();
    (v.contains('.')).then_some(v)
}

// ── Plan ─────────────────────────────────────────────────────────────────

/// What to do when the same app is already installed.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnDuplicate {
    /// Replace it (keeping its settings): the default.
    #[default]
    Update,
    /// Same as update, for the same version.
    Reinstall,
    /// Install next to it under a new id.
    KeepBoth,
}

/// An installed copy of the same app.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Duplicate {
    pub id: String,
    pub version: String,
    pub same_version: bool,
}

/// The PATH shim on offer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ShimPlan {
    pub name: String,
    pub path: PathBuf,
    /// Why it can't be added (a different file is already there).
    pub conflict: Option<String>,
}

/// Everything installing would do, shown before anything happens.
#[derive(Clone, Debug, Serialize)]
pub struct Plan {
    pub inspection: Inspection,
    /// No password needed.
    pub rootless: bool,
    pub needs_password: bool,
    /// The app's folder (rootless files).
    pub target: Option<PathBuf>,
    pub desktop_id: Option<String>,
    pub shim: Option<ShimPlan>,
    /// The command that runs (Flatpak / package manager).
    pub command: Option<Vec<String>>,
    pub native: Option<native::NativePlan>,
    /// The package manager, for native packages.
    pub manager: Option<crate::app_uninstall::Source>,
    pub duplicate: Option<Duplicate>,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
    /// "What will happen", in order.
    pub steps: Vec<String>,
    /// Whether to ask before installing (settings, native, duplicates).
    pub confirm: bool,
    /// Defaults the confirmation starts with.
    pub default_choices: Choices,
}

impl Plan {
    pub fn can_install(&self) -> bool {
        self.blockers.is_empty()
    }

    pub fn badge(&self) -> String {
        self.inspection.badge(self.manager)
    }
}

/// Where a shim at `path` stands: `None` when it is free or already ours
/// (a link to `target`, or the same script).
fn shim_conflict(path: &Path, target: &ShimTarget) -> Option<String> {
    let meta = path.symlink_metadata().ok()?;
    let ours = match target {
        ShimTarget::Link(to) => meta.file_type().is_symlink() && std::fs::read_link(path).is_ok_and(|l| &l == to),
        ShimTarget::Script(text) => meta.is_file() && std::fs::read_to_string(path).is_ok_and(|s| &s == text),
    };
    (!ours).then(|| tf("{path} already exists, so no command is added", &[("path", &tilde(path))]))
}

enum ShimTarget {
    Link(PathBuf),
    Script(String),
}

/// `/home/me/x` → `~/x` for messages.
pub fn tilde(path: &Path) -> String {
    match dirs::home_dir().and_then(|h| path.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// The program inside the app folder a rootless install runs.
fn appimage_file_name(id: &str) -> String {
    format!("{id}.AppImage")
}

/// Works out how to install `path` (runs only read-only queries: the
/// package manager's simulation, `flatpak info`).
pub fn plan(path: &Path, settings: &InstallSettings, env: &InstallEnv, runner: &dyn Runner) -> Result<Plan, String> {
    let inspection = inspect(path, env)?;
    plan_for(inspection, settings, env, runner)
}

/// [`plan`] for an inspection already made.
pub fn plan_for(inspection: Inspection, settings: &InstallSettings, env: &InstallEnv, runner: &dyn Runner) -> Result<Plan, String> {
    let family = inspection.family();
    let mut p = Plan {
        rootless: family != Family::Native,
        needs_password: family == Family::Native,
        target: None,
        desktop_id: None,
        shim: None,
        command: None,
        native: None,
        manager: None,
        duplicate: None,
        blockers: Vec::new(),
        warnings: Vec::new(),
        steps: Vec::new(),
        confirm: settings.confirm_rootless,
        default_choices: Choices { path_shim: settings.path_shims, trash_source: settings.trash_after(), ..Default::default() },
        inspection,
    };
    let i = &p.inspection;
    if let Some(arch) = i.arch.filter(|a| *a != env.host) {
        p.warnings.push(tf("Built for {arch}, but this computer is {host}: it probably won't start.", &[("arch", &arch.label()), ("host", &env.host.label())]));
    }
    let size = human_bytes(i.size);
    match family {
        Family::AppImage | Family::Portable => {
            let target = env.apps_dir.join(&i.id);
            if let Some(old) = registry::load(&env.data_home).apps.into_iter().find(|e| e.id == i.id) {
                if compare_versions(&i.version, &old.version) == std::cmp::Ordering::Less {
                    p.warnings.push(tf("This is an older version ({new}) than the one installed ({old}).", &[("new", &i.version), ("old", &old.version)]));
                }
                p.duplicate = Some(Duplicate { id: old.id.clone(), same_version: old.version == i.version, version: old.version });
            }
            let program = if family == Family::AppImage { target.join(appimage_file_name(&i.id)) } else { target.join("…") };
            if let Some(name) = &i.command_name {
                let path = env.bin_dir.join(name);
                let conflict = if family == Family::AppImage {
                    shim_conflict(&path, &ShimTarget::Link(program.clone()))
                } else if path.symlink_metadata().is_ok() && !portable_shim_is_ours(&path, &target) {
                    Some(tf("{path} already exists, so no command is added", &[("path", &tilde(&path))]))
                } else {
                    None
                };
                p.shim = Some(ShimPlan { name: name.clone(), path, conflict });
            }
            p.desktop_id = Some(format!("{ENTRY_PREFIX}{}", i.id));
            p.steps.push(if family == Family::AppImage {
                tf("Copy {file} ({size}) to {dir}", &[("file", &i.file_name), ("size", &size), ("dir", &tilde(&target))])
            } else {
                tf("Unpack {file} ({size}) into {dir}", &[("file", &i.file_name), ("size", &human_bytes(i.unpacked.unwrap_or(i.size))), ("dir", &tilde(&target))])
            });
            p.steps.push(tf("Add {app} to your apps (~/.local/share/applications)", &[("app", &i.name)]));
            if i.icon.is_some() {
                p.steps.push(t("Install its icon").to_string());
            }
            p.steps.push(t("Nothing from the file runs while installing, and no password is needed").to_string());
            p.target = Some(target);
        }
        Family::Flatpak => {
            if !runner.has("flatpak") {
                p.blockers.push(t("Flatpak isn't installed. Install it with your package manager, then try again.").to_string());
            }
            let user = settings.flatpak_user();
            p.rootless = user;
            p.needs_password = !user;
            let id = i.flatpak.as_ref().map(|f| f.id.clone()).unwrap_or_default();
            if runner.has("flatpak") && runner.run(&flatpak::info_argv(&id, user), QUERY_TIMEOUT).is_ok_and(|o| o.success()) {
                p.duplicate = Some(Duplicate { id: id.clone(), version: String::new(), same_version: false });
            }
            let cmd = flatpak::install_argv(&i.path, i.format() == Format::FlatpakBundle, user);
            p.steps.push(tf("Run {command}", &[("command", &cmd.join(" "))]));
            if i.format() == Format::FlatpakRef {
                p.steps.push(t("Flatpak downloads the app and any runtime it needs from its remote").to_string());
            }
            p.steps.push(if user { t("Installed for you only: no password needed").to_string() } else { t("Installed for everyone: Flatpak asks for your password").to_string() });
            if let Some(name) = &i.command_name {
                let path = env.bin_dir.join(name);
                let conflict = shim_conflict(&path, &ShimTarget::Script(flatpak::shim_script(&id)));
                p.shim = Some(ShimPlan { name: name.clone(), path, conflict });
            }
            p.desktop_id = Some(id);
            p.command = Some(cmd);
            p.confirm |= !user;
        }
        Family::Native => {
            p.confirm = true;
            if !settings.allow_native {
                p.blockers.push(t("Installing native packages is turned off in the App Installer settings").to_string());
            }
            let scratch = env.cache_home.join("mandara").join("installer").join(format!("native-{}", std::process::id()));
            match native::plan(&i.path, i.format(), &i.id, &scratch, runner) {
                Ok(np) => {
                    p.manager = np.manager;
                    p.steps.push(tf("Ask for your password, then run {command}", &[("command", &np.command.join(" "))]));
                    if !np.simulation.installs.is_empty() {
                        p.steps.push(tf("{manager} installs: {packages}", &[("manager", &np.manager.map(|m| m.label()).unwrap_or_default()), ("packages", &np.simulation.installs.join(", "))]));
                    }
                    for c in &np.simulation.conflicts {
                        p.warnings.push(c.clone());
                    }
                    if let Some(n) = &np.simulation.note {
                        p.warnings.push(n.clone());
                    }
                    p.command = Some(np.command.clone());
                    p.native = Some(np);
                }
                Err(why) => p.blockers.push(why),
            }
        }
    }
    p.confirm |= p.duplicate.is_some() || !p.warnings.is_empty() || !p.blockers.is_empty();
    Ok(p)
}

/// A portable shim is ours when it links into the app's folder.
fn portable_shim_is_ours(path: &Path, target: &Path) -> bool {
    std::fs::read_link(path).is_ok_and(|l| l.starts_with(target))
}

/// `1.2 MB`.
pub fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    const STEP: f64 = 1000.0;
    let mut v = n as f64;
    let mut unit = 0;
    while v >= STEP && unit + 1 < UNITS.len() {
        v /= STEP;
        unit += 1;
    }
    if unit == 0 { format!("{n} B") } else { format!("{v:.1} {}", UNITS[unit]) }
}

// ── Execution ────────────────────────────────────────────────────────────

/// What the user picked in the confirmation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Choices {
    /// Add the PATH shim.
    pub path_shim: bool,
    pub on_duplicate: OnDuplicate,
    pub dry_run: bool,
    /// Move the original file to the Trash afterwards.
    pub trash_source: bool,
}

/// A job's phase.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Inspect,
    Confirm,
    Install,
    Integrate,
    Done,
    Failed,
    Cancelled,
}

impl Phase {
    pub fn finished(self) -> bool {
        matches!(self, Phase::Done | Phase::Failed | Phase::Cancelled)
    }

    /// Short status text.
    pub fn label(self) -> &'static str {
        match self {
            Phase::Inspect => t("Reading the app…"),
            Phase::Confirm => t("Waiting for you"),
            Phase::Install => t("Installing…"),
            Phase::Integrate => t("Adding to your apps…"),
            Phase::Done => t("Installed"),
            Phase::Failed => t("Couldn't install"),
            Phase::Cancelled => t("Cancelled"),
        }
    }
}

/// A progress report.
#[derive(Copy, Clone, Debug, PartialEq, Serialize)]
pub struct Progress {
    pub phase: Phase,
    /// Bytes copied / extracted so far, of `total` (0 = unknown).
    pub done: u64,
    pub total: u64,
}

impl Progress {
    /// 0..1 when the total is known.
    pub fn fraction(&self) -> Option<f32> {
        (self.total > 0).then(|| (self.done as f64 / self.total as f64).clamp(0.0, 1.0) as f32)
    }
}

/// What happened (or, in a dry run, would happen).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Outcome {
    pub dry_run: bool,
    pub id: String,
    pub name: String,
    pub desktop_id: Option<String>,
    pub desktop_file: Option<PathBuf>,
    /// The program a rootless install runs.
    pub program: Option<PathBuf>,
    /// Everything written.
    pub paths: Vec<PathBuf>,
    pub shim: Option<PathBuf>,
    pub command: Option<Vec<String>>,
    pub steps: Vec<String>,
    pub warnings: Vec<String>,
    pub trashed_source: Option<PathBuf>,
}

/// Undo log for a rootless install.
#[derive(Default)]
struct Undo {
    created: Vec<PathBuf>,
    /// (moved-away original, where it was).
    backups: Vec<(PathBuf, PathBuf)>,
}

impl Undo {
    fn rollback(self) {
        for p in self.created.iter().rev() {
            let _ = if p.is_dir() && !p.is_symlink() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
        }
        for (backup, original) in self.backups {
            let _ = std::fs::rename(backup, original);
        }
    }

    fn commit(self) {
        for (backup, _) in self.backups {
            let _ = if backup.is_dir() && !backup.is_symlink() { std::fs::remove_dir_all(&backup) } else { std::fs::remove_file(&backup) };
        }
    }
}

static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);

fn unique_suffix() -> String {
    format!("{}-{}", std::process::id(), NEXT_STAGE.fetch_add(1, Ordering::Relaxed))
}

fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|e| e.to_string())
}

/// Writes `bytes` to `path` atomically (temp + rename).
fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let dir = path.parent().ok_or_else(|| t("No folder to write into").to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(".{}.{}.tmp", path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), unique_suffix()));
    std::fs::write(&tmp, bytes).map_err(|e| tf("Couldn't write {path}: {error}", &[("path", &tilde(path)), ("error", &e)]))?;
    set_mode(&tmp, mode)?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn sha256_file(path: &Path, cancel: &AtomicBool) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = vec![0u8; archive::CHUNK];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(t("Cancelled").to_string());
        }
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn last_lines(out: &CmdOutput) -> String {
    let text = if out.stderr.trim().is_empty() { &out.stdout } else { &out.stderr };
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(ERROR_LINES)..].join("\n")
}

/// A free id for "Keep both": `<id>-2`, `<id>-3`, …
fn free_id(base: &str, env: &InstallEnv) -> String {
    let taken = registry::load(&env.data_home);
    (2..=MAX_COPIES).map(|n| format!("{base}-{n}")).find(|id| !env.apps_dir.join(id).exists() && !taken.apps.iter().any(|e| &e.id == id)).unwrap_or_else(|| format!("{base}-{}", unique_suffix()))
}

/// Installs per `plan` and `choices`. Refuses blocked plans. Rootless
/// installs are atomic (staging folder renamed into place; everything
/// written is undone on failure or cancel).
pub fn execute(plan: &Plan, choices: &Choices, env: &InstallEnv, runner: &dyn Runner, progress: &dyn Fn(Progress), cancel: &AtomicBool) -> Result<Outcome, String> {
    if let Some(b) = plan.blockers.first() {
        return Err(b.clone());
    }
    let dry_run = choices.dry_run || env.force_dry_run;
    let i = &plan.inspection;
    let mut outcome = Outcome { dry_run, id: i.id.clone(), name: i.name.clone(), desktop_id: plan.desktop_id.clone(), command: plan.command.clone(), steps: plan.steps.clone(), warnings: plan.warnings.clone(), ..Default::default() };
    if dry_run {
        if choices.path_shim
            && let Some(s) = plan.shim.as_ref().filter(|s| s.conflict.is_none())
        {
            outcome.shim = Some(s.path.clone());
            outcome.steps.push(tf("Add the command {name} to {dir}", &[("name", &s.name), ("dir", &tilde(&env.bin_dir))]));
        }
        return Ok(outcome);
    }
    progress(Progress { phase: Phase::Install, done: 0, total: i.unpacked.unwrap_or(i.size) });
    let result = match i.family() {
        Family::AppImage | Family::Portable => install_files(plan, choices, env, progress, cancel, &mut outcome),
        Family::Flatpak => install_flatpak(plan, choices, env, runner, progress, &mut outcome),
        Family::Native => install_native(plan, env, runner, progress, &mut outcome),
    };
    result?;
    if choices.trash_source {
        match crate::app_uninstall::trash(&i.path, &env.uninstall_env()) {
            Ok(to) => outcome.trashed_source = Some(to),
            Err(e) => outcome.warnings.push(e),
        }
    }
    progress(Progress { phase: Phase::Done, done: 1, total: 1 });
    Ok(outcome)
}

fn install_files(plan: &Plan, choices: &Choices, env: &InstallEnv, progress: &dyn Fn(Progress), cancel: &AtomicBool, outcome: &mut Outcome) -> Result<(), String> {
    let i = &plan.inspection;
    let family = i.family();
    let keep_both = plan.duplicate.is_some() && choices.on_duplicate == OnDuplicate::KeepBoth;
    let id = if keep_both { free_id(&i.id, env) } else { i.id.clone() };
    let name = if keep_both && !i.version.is_empty() { format!("{} ({})", i.name, i.version) } else { i.name.clone() };
    let target = env.apps_dir.join(&id);
    let desktop_id = format!("{ENTRY_PREFIX}{id}");
    let desktop_file = env.applications_dir().join(format!("{desktop_id}.desktop"));
    std::fs::create_dir_all(&env.apps_dir).map_err(|e| e.to_string())?;
    let stage = env.apps_dir.join(format!("{STAGE_PREFIX}{id}-{}", unique_suffix()));
    let mut undo = Undo::default();
    let mut run = |undo: &mut Undo| -> Result<(PathBuf, String, Vec<PathBuf>, Option<PathBuf>), String> {
        std::fs::create_dir(&stage).map_err(|e| e.to_string())?;
        undo.created.push(stage.clone());
        let total = i.unpacked.unwrap_or(i.size);
        let report = |done: u64| progress(Progress { phase: Phase::Install, done, total });
        // Copy / unpack into the stage.
        let (rel_program, sha) = if family == Family::AppImage {
            let file = appimage_file_name(&id);
            let mut src = std::fs::File::open(&i.path).map_err(|e| e.to_string())?;
            let mut hashing = HashingReader { inner: &mut src, hasher: sha2::Sha256::new() };
            let mut written = 0;
            archive::copy_stream(&mut hashing, &stage.join(&file), EXEC_MODE, cancel, &report, &mut written)?;
            let sha = hex(&hashing.hasher.finalize());
            set_mode(&stage.join(&file), EXEC_MODE)?;
            (PathBuf::from(file), sha)
        } else {
            let unpack = stage.join("unpack");
            archive::extract(&i.path, i.format(), &unpack, cancel, &report)?;
            let root = archive::content_root(&unpack);
            let layout = archive::scan(&root, &i.id.to_lowercase());
            let program = layout.program.clone().ok_or_else(|| t("No program to run was found in this archive").to_string())?;
            // The app's files become the stage's contents.
            let content = stage.join("app");
            std::fs::rename(&root, &content).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_dir_all(&unpack);
            set_mode(&content.join(&program), EXEC_MODE)?;
            let sha = sha256_file(&i.path, cancel)?;
            (Path::new("app").join(program), sha)
        };
        if cancel.load(Ordering::Relaxed) {
            return Err(t("Cancelled").to_string());
        }
        progress(Progress { phase: Phase::Integrate, done: 0, total: 0 });
        // Swap the stage into place (an update moves the old copy aside).
        if target.symlink_metadata().is_ok() {
            let backup = env.apps_dir.join(format!("{BACKUP_PREFIX}{id}-{}", unique_suffix()));
            std::fs::rename(&target, &backup).map_err(|e| e.to_string())?;
            undo.backups.push((backup, target.clone()));
        }
        std::fs::rename(&stage, &target).map_err(|e| e.to_string())?;
        undo.created.retain(|p| p != &stage);
        undo.created.push(target.clone());
        let program = target.join(&rel_program);
        let mut written = vec![target.clone()];
        // Icon.
        let icon_name = i.icon.as_ref().map(|_| desktop_id.clone());
        if let Some(icon) = &i.icon {
            let dir = env.hicolor_dir().join(icon.hicolor_dir()).join("apps");
            let path = dir.join(format!("{desktop_id}.{}", icon.kind.extension()));
            backup_existing(&path, &mut *undo)?;
            write_atomic(&path, &icon.bytes, FILE_MODE)?;
            undo.created.push(path.clone());
            written.push(path);
        }
        // Desktop entry.
        let markers = [(MARK_ID, id.as_str()), (MARK_FORMAT, if family == Family::AppImage { "appimage" } else { "portable" })];
        let packaged = i.desktop_text.as_deref().map(|text| meta::rewrite_desktop(text, &program, icon_name.as_deref(), &markers)).filter(|t| !t.is_empty());
        let mut entry = packaged.unwrap_or_else(|| meta::synthesize_desktop(&name, &program, icon_name.as_deref(), &i.summary, &markers));
        if keep_both {
            entry = set_name(&entry, &name);
        }
        backup_existing(&desktop_file, &mut *undo)?;
        write_atomic(&desktop_file, entry.as_bytes(), FILE_MODE)?;
        undo.created.push(desktop_file.clone());
        written.push(desktop_file.clone());
        // PATH shim.
        let mut shim = None;
        if choices.path_shim
            && let Some(s) = plan.shim.as_ref()
        {
            let path = if keep_both { env.bin_dir.join(format!("{}-{}", s.name, id.rsplit('-').next().unwrap_or_default())) } else { s.path.clone() };
            let conflict = path.symlink_metadata().is_ok() && !(std::fs::read_link(&path).is_ok_and(|l| l == program || l.starts_with(&target)));
            if conflict {
                outcome.warnings.push(tf("{path} already exists, so no command is added", &[("path", &tilde(&path))]));
            } else {
                std::fs::create_dir_all(&env.bin_dir).map_err(|e| e.to_string())?;
                let _ = std::fs::remove_file(&path);
                std::os::unix::fs::symlink(&program, &path).map_err(|e| e.to_string())?;
                undo.created.push(path.clone());
                written.push(path.clone());
                shim = Some(path);
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(t("Cancelled").to_string());
        }
        Ok((program, sha, written, shim))
    };
    match run(&mut undo) {
        Ok((program, sha, written, shim)) => {
            let old = registry::load(&env.data_home).apps.into_iter().find(|e| e.id == id);
            let entry = registry::Entry {
                id: id.clone(),
                name: name.clone(),
                version: i.version.clone(),
                family: Some(family),
                format: i.format,
                source_file: i.file_name.clone(),
                sha256: sha,
                installed_at: now_secs(),
                desktop_id: Some(desktop_id.clone()),
                desktop_file: Some(desktop_file.clone()),
                dir: Some(target.clone()),
                paths: written.clone(),
                shim: shim.clone(),
                ..Default::default()
            };
            if let Err(e) = registry::record(&env.data_home, entry) {
                undo.rollback();
                return Err(e);
            }
            undo.commit();
            // Files of the old copy the new one didn't rewrite (an icon of
            // another size, an old shim name).
            if let Some(old) = old {
                for p in old.paths.iter().filter(|p| !written.contains(p) && owned_by_install(p, &old, env)) {
                    let _ = std::fs::remove_file(p);
                }
            }
            outcome.id = id;
            outcome.name = name;
            outcome.desktop_id = Some(desktop_id);
            outcome.desktop_file = Some(desktop_file);
            outcome.program = Some(program);
            outcome.paths = written;
            outcome.shim = shim;
            Ok(())
        }
        Err(e) => {
            undo.rollback();
            let _ = std::fs::remove_dir_all(&stage);
            Err(e)
        }
    }
}

/// Paths of an old install that may be cleaned up after an update: its
/// entry, icons and a shim linking into its folder (never anything else).
fn owned_by_install(path: &Path, entry: &registry::Entry, env: &InstallEnv) -> bool {
    let name_ok = path.file_name().is_some_and(|n| n.to_string_lossy().starts_with(ENTRY_PREFIX));
    let in_known = path.starts_with(env.applications_dir()) || path.starts_with(env.hicolor_dir());
    let shim_ok = entry.shim.as_deref() == Some(path) && entry.dir.as_ref().is_some_and(|d| std::fs::read_link(path).is_ok_and(|l| l.starts_with(d)));
    (name_ok && in_known && path.is_file()) || shim_ok
}

/// Moves an existing file aside (restored on rollback, dropped on commit).
fn backup_existing(path: &Path, undo: &mut Undo) -> Result<(), String> {
    if path.symlink_metadata().is_ok() {
        let backup = path.with_file_name(format!(".{}.{}", path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), unique_suffix()));
        std::fs::rename(path, &backup).map_err(|e| e.to_string())?;
        undo.backups.push((backup, path.to_path_buf()));
    }
    Ok(())
}

/// Replaces `Name=` in the main group.
fn set_name(entry: &str, name: &str) -> String {
    let mut in_main = false;
    entry
        .lines()
        .map(|l| {
            if l.starts_with('[') {
                in_main = l.trim() == "[Desktop Entry]";
            }
            if in_main && l.starts_with("Name=") { format!("Name={name}") } else { l.to_string() }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

struct HashingReader<'a, R: std::io::Read> {
    inner: &'a mut R,
    hasher: sha2::Sha256,
}

impl<R: std::io::Read> std::io::Read for HashingReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
}

fn run_checked(runner: &dyn Runner, cmd: &[String]) -> Result<(), String> {
    let out = runner.run(cmd, INSTALL_TIMEOUT)?;
    if out.success() {
        return Ok(());
    }
    let msg = last_lines(&out);
    Err(if msg.is_empty() { tf("{program} exited with status {code}", &[("program", &cmd[0]), ("code", &out.code.unwrap_or(-1))]) } else { msg })
}

fn install_flatpak(plan: &Plan, choices: &Choices, env: &InstallEnv, runner: &dyn Runner, progress: &dyn Fn(Progress), outcome: &mut Outcome) -> Result<(), String> {
    let i = &plan.inspection;
    let cmd = plan.command.clone().ok_or_else(|| t("Nothing to run").to_string())?;
    let id = i.flatpak.as_ref().map(|f| f.id.clone()).unwrap_or_else(|| i.id.clone());
    let sha = sha256_file(&i.path, &AtomicBool::new(false)).unwrap_or_default();
    run_checked(runner, &cmd)?;
    progress(Progress { phase: Phase::Integrate, done: 0, total: 0 });
    let mut paths = Vec::new();
    if choices.path_shim
        && let Some(s) = plan.shim.as_ref()
    {
        let script = flatpak::shim_script(&id);
        if shim_conflict(&s.path, &ShimTarget::Script(script.clone())).is_none() {
            write_atomic(&s.path, script.as_bytes(), EXEC_MODE)?;
            paths.push(s.path.clone());
            outcome.shim = Some(s.path.clone());
        } else {
            outcome.warnings.push(tf("{path} already exists, so no command is added", &[("path", &tilde(&s.path))]));
        }
    }
    registry::record(
        &env.data_home,
        registry::Entry {
            id: id.clone(),
            name: i.name.clone(),
            version: i.version.clone(),
            family: Some(Family::Flatpak),
            format: i.format,
            source_file: i.file_name.clone(),
            sha256: sha,
            installed_at: now_secs(),
            desktop_id: Some(id.clone()),
            paths: paths.clone(),
            shim: outcome.shim.clone(),
            flatpak: Some(id.clone()),
            flatpak_user: Some(!plan.needs_password),
            ..Default::default()
        },
    )?;
    outcome.id = id.clone();
    outcome.desktop_id = Some(id);
    outcome.paths = paths;
    Ok(())
}

fn install_native(plan: &Plan, env: &InstallEnv, runner: &dyn Runner, progress: &dyn Fn(Progress), outcome: &mut Outcome) -> Result<(), String> {
    let i = &plan.inspection;
    let np = plan.native.as_ref().ok_or_else(|| t("Nothing to run").to_string())?;
    let sha = sha256_file(&i.path, &AtomicBool::new(false)).unwrap_or_default();
    run_checked(runner, &np.command)?;
    progress(Progress { phase: Phase::Integrate, done: 0, total: 0 });
    let desktop_id = np.manager.and_then(|m| native::desktop_of(m, &np.package, runner));
    registry::record(
        &env.data_home,
        registry::Entry {
            id: i.id.clone(),
            name: i.name.clone(),
            version: i.version.clone(),
            family: Some(Family::Native),
            format: i.format,
            source_file: i.file_name.clone(),
            sha256: sha,
            installed_at: now_secs(),
            desktop_id: desktop_id.clone(),
            package: Some(np.package.clone()),
            ..Default::default()
        },
    )?;
    outcome.desktop_id = desktop_id;
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Mock {
        programs: Vec<&'static str>,
        replies: HashMap<String, CmdOutput>,
        calls: Mutex<Vec<String>>,
    }

    impl Mock {
        fn with(programs: &[&'static str]) -> Self {
            Self { programs: programs.to_vec(), ..Default::default() }
        }
        fn reply(mut self, cmd: &str, out: CmdOutput) -> Self {
            self.replies.insert(cmd.into(), out);
            self
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Runner for Mock {
        fn run(&self, argv: &[String], _: Duration) -> Result<CmdOutput, String> {
            let line = argv.join(" ");
            self.calls.lock().unwrap().push(line.clone());
            Ok(self.replies.get(&line).cloned().unwrap_or_else(|| CmdOutput::failed(1, "not mocked")))
        }
        fn has(&self, program: &str) -> bool {
            self.programs.contains(&program)
        }
    }

    fn home() -> (tempfile::TempDir, InstallEnv) {
        let dir = tempfile::tempdir().unwrap();
        let env = InstallEnv::with_home(dir.path());
        (dir, env)
    }

    fn hello_appimage(dir: &Path, version: &str) -> PathBuf {
        let desktop = format!("[Desktop Entry]\nType=Application\nName=Hello\nExec=hello %U\nIcon=hello\nCategories=Utility;\nX-AppImage-Version={version}\n");
        let bytes = appimage::tests::fixture(&[("/hello.desktop", desktop.as_bytes()), ("/hello.png", appimage::tests::PNG_1X1), ("/AppRun", b"#!/bin/sh\nexit 1\n")], &[("/.DirIcon", "hello.png")]);
        let path = dir.join(format!("Hello-{version}-x86_64.AppImage"));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn settings() -> InstallSettings {
        InstallSettings::default()
    }

    static NO_CANCEL: AtomicBool = AtomicBool::new(false);

    #[test]
    fn appimage_installs_rootless_with_entry_icon_shim_and_registry() {
        let (dir, env) = home();
        let file = hello_appimage(dir.path(), "1.2.3");
        let mock = Mock::default();
        let plan = plan(&file, &settings(), &env, &mock).unwrap();
        assert!(plan.rootless && !plan.needs_password && plan.can_install(), "{:?}", plan.blockers);
        assert_eq!(plan.inspection.name, "Hello");
        assert_eq!(plan.inspection.version, "1.2.3");
        assert_eq!(plan.inspection.id, "hello");
        assert_eq!(plan.badge(), "AppImage");
        assert!(plan.inspection.icon_file.as_ref().is_some_and(|p| p.is_file()));
        let seen = Mutex::new(Vec::new());
        let choices = Choices { path_shim: true, ..Default::default() };
        let out = execute(&plan, &choices, &env, &mock, &|p| seen.lock().unwrap().push(p.phase), &NO_CANCEL).unwrap();
        assert!(mock.calls().is_empty(), "nothing ran: {:?}", mock.calls());
        let program = env.apps_dir.join("hello/hello.AppImage");
        assert_eq!(out.program.as_deref(), Some(program.as_path()));
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(program.metadata().unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read(&program).unwrap(), std::fs::read(&file).unwrap());
        let entry = std::fs::read_to_string(env.applications_dir().join("mandara-hello.desktop")).unwrap();
        assert!(entry.contains(&format!("Exec={} %U", program.display())), "{entry}");
        assert!(entry.contains("Icon=mandara-hello") && entry.contains("X-Mandara-App-Id=hello"));
        assert!(env.hicolor_dir().join("64x64/apps/mandara-hello.png").is_file());
        assert_eq!(std::fs::read_link(env.bin_dir.join("hello")).unwrap(), program);
        let phases = seen.lock().unwrap().clone();
        assert_eq!((phases.first(), phases.last()), (Some(&Phase::Install), Some(&Phase::Done)));
        assert!(phases.contains(&Phase::Integrate));
        let reg = registry::installed(&env.data_home);
        assert_eq!(reg.len(), 1);
        assert_eq!(reg[0].sha256.len(), 64);
        assert_eq!(reg[0].paths.len(), 4, "dir, icon, entry, shim: {:?}", reg[0].paths);
        // No stray staging folders.
        assert!(std::fs::read_dir(&env.apps_dir).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with('.') || e.file_name() == "registry.toml"));
    }

    #[test]
    fn versions_order_numerically() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions("1.10.0", "1.9.2"), Greater);
        assert_eq!(compare_versions("v2.0", "2.0.0"), Equal);
        assert_eq!(compare_versions("1.0-rc1", "1.0-rc2"), Less);
        assert_eq!(compare_versions("", "3"), Equal);
    }

    #[test]
    fn inspection_reads_description_developer_categories_and_screenshots() {
        let (dir, env) = home();
        let stream = br#"<component><id>org.example.Shots</id><name>Shots</name><summary>Takes pictures</summary><developer><name>Example Co</name></developer><description><p>Snap &amp; share.</p><ul><li>Fast</li></ul></description><categories><category>Graphics</category></categories><screenshots><screenshot type="default"><image>usr/share/shots/one.png</image></screenshot><screenshot><image>https://example.org/remote.png</image></screenshot></screenshots></component>"#;
        let desktop = b"[Desktop Entry]
Type=Application
Name=Shots
Exec=shots
Icon=shots
Categories=Utility;
";
        let bytes = appimage::tests::fixture(&[("/shots.desktop", desktop), ("/shots.png", appimage::tests::PNG_1X1), ("/usr/share/metainfo/org.example.Shots.metainfo.xml", stream), ("/usr/share/shots/one.png", &flatpak::tests::png(1600, 1000)), ("/AppRun", b"#!/bin/sh
")], &[]);
        let file = dir.path().join("Shots-1.0-x86_64.AppImage");
        std::fs::write(&file, bytes).unwrap();
        let i = inspect(&file, &env).unwrap();
        assert_eq!(i.developer, "Example Co");
        assert_eq!(i.description, "Snap & share.\n• Fast");
        assert_eq!(i.categories, vec!["Graphics"], "AppStream wins over the desktop entry");
        assert_eq!(i.screenshots.len(), 1, "remote screenshots are never fetched");
        let shot = image::open(&i.screenshots[0]).unwrap();
        assert_eq!((shot.width(), shot.height()), (SCREENSHOT_PREVIEW_W, SCREENSHOT_PREVIEW_H), "downscaled to fit the preview box");
        // A Flatpak bundle: permissions, AppStream and icon from its header.
        let bundle = dir.path().join("hello.flatpak");
        std::fs::write(&bundle, flatpak::tests::bundle_fixture()).unwrap();
        let f = inspect(&bundle, &env).unwrap();
        assert_eq!((f.name.as_str(), f.developer.as_str()), ("Hello", "Example"));
        assert!(f.flatpak.as_ref().and_then(|x| x.permissions.as_ref()).is_some_and(|p| p.shared.contains(&"network".to_string())));
        assert!(f.icon_file.is_some());
    }

    #[test]
    fn previews_are_removed_and_pruned() {
        let (dir, env) = home();
        let file = hello_appimage(dir.path(), "1.0.0");
        let i = inspect(&file, &env).unwrap();
        let previews = preview_files(&i, &env);
        assert_eq!(previews.len(), 1, "the icon preview");
        assert!(previews[0].is_file());
        // Nothing outside the preview folder is ever touched.
        remove_previews(&[file.clone()], &env);
        assert!(file.is_file());
        remove_previews(&previews, &env);
        assert!(!previews[0].exists());
        // Pruning: old files go, fresh ones and folders stay.
        let d = env.preview_dir();
        std::fs::write(d.join("old.png"), b"x").unwrap();
        std::fs::create_dir_all(d.join("native-1")).unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        std::fs::File::options().write(true).open(d.join("old.png")).unwrap().set_modified(old).unwrap();
        std::fs::write(d.join("fresh.png"), b"x").unwrap();
        assert_eq!(prune_previews(&env, std::time::Duration::from_secs(3600)), 1);
        assert!(d.join("fresh.png").is_file() && d.join("native-1").is_dir() && !d.join("old.png").exists());
    }

    #[test]
    fn older_versions_are_flagged() {
        let (dir, env) = home();
        let mock = Mock::default();
        let v2 = hello_appimage(dir.path(), "2.0.0");
        execute(&plan(&v2, &settings(), &env, &mock).unwrap(), &Choices::default(), &env, &mock, &|_| {}, &NO_CANCEL).unwrap();
        let v1 = hello_appimage(dir.path(), "1.0.0");
        let p = plan(&v1, &settings(), &env, &mock).unwrap();
        assert!(p.warnings.iter().any(|w| w.contains("older version")), "{:?}", p.warnings);
    }

    #[test]
    fn duplicates_update_or_keep_both() {
        let (dir, env) = home();
        let mock = Mock::default();
        let v1 = hello_appimage(dir.path(), "1.0.0");
        let p1 = plan(&v1, &settings(), &env, &mock).unwrap();
        assert!(p1.duplicate.is_none());
        execute(&p1, &Choices::default(), &env, &mock, &|_| {}, &NO_CANCEL).unwrap();
        let v2 = hello_appimage(dir.path(), "2.0.0");
        let p2 = plan(&v2, &settings(), &env, &mock).unwrap();
        let dup = p2.duplicate.clone().unwrap();
        assert_eq!((dup.version.as_str(), dup.same_version, p2.confirm), ("1.0.0", false, true));
        execute(&p2, &Choices::default(), &env, &mock, &|_| {}, &NO_CANCEL).unwrap();
        let reg = registry::installed(&env.data_home);
        assert_eq!((reg.len(), reg[0].version.as_str()), (1, "2.0.0"), "update replaces");
        assert!(std::fs::read_dir(&env.apps_dir).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with(BACKUP_PREFIX)), "old copy cleaned");
        let p3 = plan(&v2, &settings(), &env, &mock).unwrap();
        assert!(p3.duplicate.as_ref().unwrap().same_version);
        let out = execute(&p3, &Choices { on_duplicate: OnDuplicate::KeepBoth, ..Default::default() }, &env, &mock, &|_| {}, &NO_CANCEL).unwrap();
        assert_eq!(out.id, "hello-2");
        assert_eq!(registry::installed(&env.data_home).len(), 2);
        assert!(env.applications_dir().join("mandara-hello-2.desktop").is_file());
    }

    #[test]
    fn cancel_rolls_back_everything() {
        let (dir, env) = home();
        let file = hello_appimage(dir.path(), "1.0.0");
        let mock = Mock::default();
        let p = plan(&file, &settings(), &env, &mock).unwrap();
        let err = execute(&p, &Choices { path_shim: true, ..Default::default() }, &env, &mock, &|_| {}, &AtomicBool::new(true)).unwrap_err();
        assert_eq!(err, t("Cancelled"));
        assert!(!env.apps_dir.join("hello").exists());
        assert!(!env.applications_dir().join("mandara-hello.desktop").exists());
        assert!(env.bin_dir.join("hello").symlink_metadata().is_err());
        assert!(registry::installed(&env.data_home).is_empty());
        assert!(std::fs::read_dir(&env.apps_dir).map(|r| r.count()).unwrap_or(0) == 0, "stage removed");
    }

    #[test]
    fn shims_never_replace_other_files() {
        let (dir, env) = home();
        std::fs::create_dir_all(&env.bin_dir).unwrap();
        std::fs::write(env.bin_dir.join("hello"), "mine").unwrap();
        let file = hello_appimage(dir.path(), "1.0.0");
        let mock = Mock::default();
        let p = plan(&file, &settings(), &env, &mock).unwrap();
        assert!(p.shim.as_ref().unwrap().conflict.is_some());
        let out = execute(&p, &Choices { path_shim: true, ..Default::default() }, &env, &mock, &|_| {}, &NO_CANCEL).unwrap();
        assert!(out.shim.is_none() && !out.warnings.is_empty());
        assert_eq!(std::fs::read_to_string(env.bin_dir.join("hello")).unwrap(), "mine");
    }

    #[test]
    fn dry_run_touches_nothing() {
        let (dir, mut env) = home();
        env.force_dry_run = true;
        let file = hello_appimage(dir.path(), "1.0.0");
        let mock = Mock::default();
        let p = plan(&file, &settings(), &env, &mock).unwrap();
        let out = execute(&p, &Choices { path_shim: true, trash_source: true, ..Default::default() }, &env, &mock, &|_| panic!("no progress in a dry run"), &NO_CANCEL).unwrap();
        assert!(out.dry_run && !out.steps.is_empty());
        assert!(!env.apps_dir.join("hello").exists() && file.exists());
        assert!(registry::installed(&env.data_home).is_empty());
    }

    #[test]
    fn portable_tar_gz_finds_program_and_icon() {
        let (dir, env) = home();
        let archive_path = dir.path().join("Tool-2.1.0-linux.tar.gz");
        let elf = {
            let mut h = vec![0u8; 64];
            h[..4].copy_from_slice(b"\x7fELF");
            h[4] = 2;
            h[5] = 1;
            h[18..20].copy_from_slice(&(if cfg!(target_arch = "aarch64") { 183u16 } else { 62u16 }).to_le_bytes());
            h
        };
        std::fs::write(&archive_path, archive::tests::tar_gz(&[("Tool-2.1.0/tool", &elf, 0o755), ("Tool-2.1.0/icons/tool.png", appimage::tests::PNG_1X1, 0o644), ("Tool-2.1.0/README.md", b"hi", 0o644)], &[])).unwrap();
        let mock = Mock::default();
        let p = plan(&archive_path, &settings(), &env, &mock).unwrap();
        assert_eq!(p.inspection.family(), Family::Portable);
        assert_eq!(p.inspection.name, "Tool");
        assert_eq!(p.inspection.version, "2.1.0");
        assert_eq!(p.inspection.arch, Some(Arch::host()));
        assert!(p.inspection.icon.is_some());
        let out = execute(&p, &Choices { path_shim: true, ..Default::default() }, &env, &mock, &|_| {}, &NO_CANCEL).unwrap();
        let program = env.apps_dir.join("Tool/app/tool");
        assert_eq!(out.program.as_deref(), Some(program.as_path()));
        let entry = std::fs::read_to_string(env.applications_dir().join("mandara-Tool.desktop")).unwrap();
        assert!(entry.contains(&format!("Exec={}", program.display())) && entry.contains("Name=Tool"), "{entry}");
        assert_eq!(std::fs::read_link(env.bin_dir.join("tool")).unwrap(), program);
    }

    #[test]
    fn flatpakref_installs_with_flatpak_user_and_a_script_shim() {
        let (dir, env) = home();
        let file = dir.path().join("maps.flatpakref");
        std::fs::write(&file, "[Flatpak Ref]\nName=org.gnome.Maps\nBranch=stable\nTitle=Maps\nUrl=https://dl.flathub.org/repo/\n").unwrap();
        let cmd = format!("flatpak install --user --noninteractive -y --from {}", file.display());
        let mock = Mock::with(&["flatpak"]).reply(&cmd, CmdOutput::ok("done"));
        let p = plan(&file, &settings(), &env, &mock).unwrap();
        assert!(p.rootless && p.can_install());
        assert_eq!(p.command.as_ref().unwrap().join(" "), cmd);
        let out = execute(&p, &Choices { path_shim: true, ..Default::default() }, &env, &mock, &|_| {}, &NO_CANCEL).unwrap();
        assert!(mock.calls().contains(&cmd));
        assert_eq!(out.desktop_id.as_deref(), Some("org.gnome.Maps"));
        assert_eq!(std::fs::read_to_string(env.bin_dir.join("maps")).unwrap(), "#!/bin/sh\nexec flatpak run org.gnome.Maps \"$@\"\n");
        assert_eq!(registry::installed(&env.data_home)[0].flatpak.as_deref(), Some("org.gnome.Maps"));
        // Without Flatpak there's nothing to install with.
        assert!(!plan(&file, &settings(), &env, &Mock::default()).unwrap().can_install());
    }

    #[test]
    fn native_packages_use_the_system_manager_or_explain() {
        let (dir, env) = home();
        let deb = dir.path().join("code_1.92.0_amd64.deb");
        std::fs::write(&deb, b"!<arch>\ndebian-binary   0           0     0     100644  4         `\n2.0\n").unwrap();
        // Arch Linux: a .deb is foreign.
        let arch = Mock::with(&["pacman"]);
        let p = plan(&deb, &settings(), &env, &arch).unwrap();
        assert!(!p.can_install() && p.blockers[0].contains("Flatpak"), "{:?}", p.blockers);
        assert!(arch.calls().is_empty(), "no install attempted");
        // Debian: apt-get's simulation, then pkexec apt-get install.
        let sim = format!("apt-get -s install {}", deb.display());
        let install = format!("pkexec apt-get install -y {}", deb.display());
        let debian = Mock::with(&["dpkg", "apt-get"]).reply(&sim, CmdOutput::ok("Inst libfoo (1)\nInst code (1.92.0 local)\n")).reply(&install, CmdOutput::ok("")).reply("dpkg -L code", CmdOutput::ok("/usr/share/applications/code.desktop\n"));
        let p = plan(&deb, &settings(), &env, &debian).unwrap();
        assert!(p.can_install() && p.needs_password && p.confirm, "{:?}", p.blockers);
        assert_eq!(p.badge(), "Native (APT)");
        assert_eq!(p.native.as_ref().unwrap().simulation.installs, vec!["libfoo", "code"]);
        let out = execute(&p, &Choices::default(), &env, &debian, &|_| {}, &NO_CANCEL).unwrap();
        assert!(debian.calls().contains(&install));
        assert_eq!(out.desktop_id.as_deref(), Some("code"));
        // Turned off in the settings.
        let off = InstallSettings { allow_native: false, ..settings() };
        assert!(!plan(&deb, &off, &env, &debian).unwrap().can_install());
    }

    #[test]
    fn unsupported_files_are_explained() {
        let (dir, env) = home();
        let txt = dir.path().join("notes.txt");
        std::fs::write(&txt, "hello").unwrap();
        assert!(!accepts(&txt));
        assert!(inspect(&txt, &env).unwrap_err().contains("notes.txt"));
        let app = hello_appimage(dir.path(), "1.0.0");
        assert!(accepts(&app));
    }

    #[test]
    fn uninstaller_removes_exactly_the_recorded_files() {
        use crate::app_uninstall as un;
        let (dir, env) = home();
        let file = hello_appimage(dir.path(), "1.0.0");
        let mock = Mock::default();
        let p = plan(&file, &settings(), &env, &mock).unwrap();
        let out = execute(&p, &Choices { path_shim: true, ..Default::default() }, &env, &mock, &|_| {}, &NO_CANCEL).unwrap();
        // Something of the user's next to it must survive.
        std::fs::write(env.applications_dir().join("mine.desktop"), "[Desktop Entry]\nName=Mine\n").unwrap();
        let desktop = out.desktop_file.clone().unwrap();
        let text = std::fs::read_to_string(&desktop).unwrap();
        let app = un::AppRef::parse("mandara-hello", &desktop, &text);
        let uenv = env.uninstall_env();
        assert_eq!(un::quick_source(&app, &uenv, None), un::Source::Mandara);
        let uplan = un::plan(&app, &uenv, &mock);
        assert_eq!(uplan.owner.source, un::Source::Mandara);
        assert!(uplan.command.is_none() && uplan.can_uninstall(), "{:?}", uplan.blockers);
        let mut expected = out.paths.clone();
        expected.sort();
        let mut files = uplan.owner.files.clone();
        files.sort();
        assert_eq!(files, expected);
        let done = un::execute(&uplan, &un::Choices::default(), &uenv, &mock).unwrap();
        assert_eq!(done.trashed.len(), expected.len(), "{:?}", done.errors);
        for p in &expected {
            assert!(p.symlink_metadata().is_err(), "{} still there", p.display());
        }
        assert!(env.applications_dir().join("mine.desktop").is_file());
        assert!(file.is_file(), "the original download stays");
        assert!(registry::installed(&env.data_home).is_empty(), "forgotten");
    }

    #[test]
    fn helpers() {
        assert_eq!(version_from_name("Obsidian-1.6.7.AppImage").as_deref(), Some("1.6.7"));
        assert_eq!(version_from_name("tool.zip"), None);
        assert_eq!(human_bytes(1_500_000), "1.5 MB");
        assert_eq!(file_stem_clean(Path::new("Obsidian-1.6.7.AppImage")), "Obsidian");
        assert!(picker_globs().contains(&"*.AppImage"));
    }
}
