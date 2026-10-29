//! Distro-agnostic app uninstaller.
//!
//! Given an app's desktop entry, [`detect_owner`] finds who installed it —
//! Flatpak, Snap, an AppImage, Nix, a user-local entry, or whichever system
//! package manager owns the entry (or its program): pacman, dpkg/APT, RPM
//! (DNF / Zypper / YUM), XBPS, APK, Portage, eopkg. Managers are probed at
//! runtime; no distro is assumed.
//!
//! [`plan`] then builds what removal would do: the exact argv (system
//! packages go through `pkexec`, never `sudo` or a shell string), what else
//! the package manager would take with it (from its own dry run), refusals
//! (protected packages, NixOS system profile, Mandara itself) and the
//! optional leftovers in `$HOME` (config, data, state, cache, sandboxes),
//! matched conservatively by exact, case-insensitive name. [`execute`] runs
//! it; files only ever go to the freedesktop Trash ([`trash`]), never
//! deleted.
//!
//! Every command runs through a [`Runner`], so tests use a mock that
//! records argv. `MANDARA_UNINSTALL_DRY_RUN=1` forces dry runs everywhere
//! (nothing runs, nothing moves), and [`SystemRunner`] refuses to run
//! anything inside unit tests.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};

/// Forces dry runs in every process that reads it (Studio, the shell, CLI).
pub const DRY_RUN_ENV: &str = "MANDARA_UNINSTALL_DRY_RUN";
/// Owner queries and previews are quick; give up after this.
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
/// A removal waits for the polkit prompt and the package manager.
const REMOVE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const POLL: Duration = Duration::from_millis(50);
/// Stop summing a leftover's size after this many entries.
const SIZE_SCAN_MAX_ENTRIES: usize = 200_000;
/// Names shorter than this never match a leftover folder.
const MIN_NAME_LEN: usize = 2;
/// `org.gnome.Nautilus`: the last segment of reverse-DNS ids with at least
/// this many parts also names leftovers…
const REVERSE_DNS_MIN_PARTS: usize = 3;
/// …when it is at least this long.
const LAST_SEGMENT_MIN_LEN: usize = 4;
/// Lines of a package manager's error shown to the user.
const ERROR_LINES: usize = 4;
const PKEXEC: &str = "pkexec";

/// True when `MANDARA_UNINSTALL_DRY_RUN` is set to anything but `0` / empty.
pub fn forced_dry_run() -> bool {
    std::env::var(DRY_RUN_ENV).is_ok_and(|v| !v.is_empty() && v != "0")
}

// ── Running commands ─────────────────────────────────────────────────────

/// A finished command: exit code (None = killed by a signal) and output.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CmdOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CmdOutput {
    pub fn ok(stdout: &str) -> Self {
        Self { code: Some(0), stdout: stdout.into(), stderr: String::new() }
    }

    pub fn failed(code: i32, stderr: &str) -> Self {
        Self { code: Some(code), stdout: String::new(), stderr: stderr.into() }
    }

    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// stdout and stderr together (some tools print plans on either).
    fn all(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

/// Runs argv vectors (never a shell string) and finds programs.
pub trait Runner: Send + Sync {
    fn run(&self, argv: &[String], timeout: Duration) -> Result<CmdOutput, String>;
    /// `program` is on `$PATH`.
    fn has(&self, program: &str) -> bool;
}

/// The real runner: `LANG=C`, no stdin, a timeout, both pipes drained.
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, argv: &[String], timeout: Duration) -> Result<CmdOutput, String> {
        // Unit tests must never reach a real package manager.
        if cfg!(test) {
            return Err("SystemRunner is disabled in tests".into());
        }
        let (program, args) = argv.split_first().ok_or_else(|| t("Nothing to run").to_string())?;
        let mut child = Command::new(program)
            .args(args)
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| tf("Could not run {program}: {error}", &[("program", program), ("error", &e)]))?;
        let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                if let Some(mut p) = pipe {
                    let _ = p.read_to_end(&mut buf);
                }
                String::from_utf8_lossy(&buf).into_owned()
            })
        };
        let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
        let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() >= timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(tf("{program} timed out", &[("program", program)]));
                }
                Ok(None) => std::thread::sleep(POLL),
                Err(e) => return Err(e.to_string()),
            }
        };
        Ok(CmdOutput { code: status.code(), stdout: out.join().unwrap_or_default(), stderr: err.join().unwrap_or_default() })
    }

    fn has(&self, program: &str) -> bool {
        crate::updates::find_program(program).is_some()
    }
}

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

// ── Environment ──────────────────────────────────────────────────────────

/// Where to look and what to protect; tests build one on a temp `$HOME`.
#[derive(Clone, Debug, Default)]
pub struct Env {
    pub home: PathBuf,
    pub config_home: PathBuf,
    pub data_home: PathBuf,
    pub state_home: PathBuf,
    pub cache_home: PathBuf,
    pub uid: u32,
    /// Desktop ids that need an extra confirmation (your default terminal,
    /// browser, file manager).
    pub default_apps: Vec<String>,
    /// Programs whose packages must never be removed (the shell, the
    /// compositor), as paths.
    pub vital_programs: Vec<PathBuf>,
    /// Dry run regardless of what the caller asks.
    pub force_dry_run: bool,
}

impl Env {
    /// XDG defaults under `home`.
    pub fn with_home(home: &Path) -> Self {
        Self {
            home: home.to_path_buf(),
            config_home: home.join(".config"),
            data_home: home.join(".local/share"),
            state_home: home.join(".local/state"),
            cache_home: home.join(".cache"),
            uid: rustix::process::getuid().as_raw(),
            ..Default::default()
        }
    }

    /// This user's directories, the running shell and compositor as vital
    /// programs, and [`forced_dry_run`].
    pub fn current() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let mut env = Self::with_home(&home);
        if let Some(d) = dirs::config_dir() {
            env.config_home = d;
        }
        if let Some(d) = dirs::data_dir() {
            env.data_home = d;
        }
        if let Some(d) = dirs::state_dir() {
            env.state_home = d;
        }
        if let Some(d) = dirs::cache_dir() {
            env.cache_home = d;
        }
        env.vital_programs.extend(std::env::current_exe().ok());
        for program in VITAL_PROGRAMS {
            env.vital_programs.extend(crate::updates::find_program(program));
        }
        env.force_dry_run = forced_dry_run();
        env
    }
}

/// Programs whose owning package is never removed (compositors, portal).
const VITAL_PROGRAMS: &[&str] = &["mandara", "mango", "niri", "Hyprland", "xdg-desktop-portal", "pkexec"];

/// Package names never removed from here, whatever the package manager says.
const PROTECTED_PACKAGES: &[&str] = &[
    "mandara", "mandara-shell", "mandara-shell-git", "mangowc", "mangowc-git", "mango", "mangowm", "niri", "niri-git", "hyprland", "hyprland-git", "base", "base-devel", "filesystem", "glibc", "libc6", "musl", "systemd", "systemd-libs",
    "openrc", "runit", "bash", "coreutils", "util-linux", "linux", "linux-lts", "linux-zen", "linux-hardened", "linux-cachyos", "kernel", "kernel-core", "linux-image-generic", "sudo", "polkit", "policykit-1", "pacman", "apt", "dpkg", "dnf", "dnf5",
    "yum", "rpm", "zypper", "xbps", "apk-tools", "portage", "eopkg", "nix", "flatpak", "snapd", "dbus", "dbus-broker", "wayland", "mesa", "pipewire", "wireplumber",
];
/// Package-name prefixes never removed (every desktop portal backend).
const PROTECTED_PREFIXES: &[&str] = &["xdg-desktop-portal"];
/// Desktop ids of Mandara itself.
const OWN_ID_PREFIX: &str = "org.mandara.";

fn is_protected_package(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    PROTECTED_PACKAGES.contains(&n.as_str()) || PROTECTED_PREFIXES.iter().any(|p| n.starts_with(p))
}

// ── The app ──────────────────────────────────────────────────────────────

/// The parts of a desktop entry the uninstaller needs.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct AppRef {
    /// `firefox.desktop`.
    pub id: String,
    pub name: String,
    pub exec: String,
    pub icon: String,
    pub wm_class: String,
    /// `X-Flatpak=org.gimp.GIMP`.
    pub flatpak: Option<String>,
    /// `X-SnapInstanceName=spotify`.
    pub snap: Option<String>,
    /// The desktop file.
    pub path: PathBuf,
}

/// `foo` or `foo.desktop` → `foo.desktop`.
pub fn desktop_id(id: &str) -> String {
    let id = id.trim();
    if id.ends_with(".desktop") { id.to_string() } else { format!("{id}.desktop") }
}

impl AppRef {
    pub fn parse(id: &str, path: &Path, text: &str) -> Self {
        let mut app = Self { id: desktop_id(id), path: path.to_path_buf(), ..Default::default() };
        let mut main = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                main = line == "[Desktop Entry]";
                continue;
            }
            let Some((k, v)) = line.split_once('=').filter(|_| main) else { continue };
            let v = v.trim().to_string();
            match k.trim() {
                "Name" => app.name = v,
                "Exec" => app.exec = v,
                "Icon" => app.icon = v,
                "StartupWMClass" => app.wm_class = v,
                "X-Flatpak" if !v.is_empty() => app.flatpak = Some(v),
                "X-SnapInstanceName" if !v.is_empty() => app.snap = Some(v),
                _ => {}
            }
        }
        app
    }

    /// The installed app with this desktop id (`firefox` or `firefox.desktop`).
    pub fn find(id: &str) -> Option<Self> {
        let id = desktop_id(id);
        let entry = mandara_config::apps::find_desktop(&id)?;
        let text = std::fs::read_to_string(&entry.path).ok()?;
        Some(Self::parse(&id, &entry.path, &text))
    }

    /// The desktop id without `.desktop`.
    pub fn stem(&self) -> &str {
        self.id.strip_suffix(".desktop").unwrap_or(&self.id)
    }

    /// The program `Exec` starts (after `env VAR=… `), as written.
    pub fn program(&self) -> String {
        exec_words(&self.exec).into_iter().find(|w| w != "env" && !(w.contains('=') && !w.starts_with('/'))).unwrap_or_default()
    }
}

/// `Exec` split into words: double quotes group, field codes dropped.
fn exec_words(exec: &str) -> Vec<String> {
    let mut words = Vec::new();
    let (mut word, mut quoted, mut escaped) = (String::new(), false, false);
    for c in exec.chars() {
        match c {
            _ if escaped => {
                word.push(c);
                escaped = false;
            }
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words.retain(|w| !(w.len() == 2 && w.starts_with('%')));
    words
}

// ── Sources ──────────────────────────────────────────────────────────────

/// Who installed an app.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Pacman,
    Apt,
    Dnf,
    Zypper,
    Yum,
    Rpm,
    Xbps,
    Apk,
    Portage,
    Eopkg,
    Nix,
    Flatpak,
    Snap,
    AppImage,
    /// Installed by Mandara's app installer (recorded in its registry).
    Mandara,
    Local,
    Unknown,
}

impl Source {
    /// Badge text (proper names stay untranslated).
    pub fn label(self) -> &'static str {
        match self {
            Source::Pacman => "Pacman",
            Source::Apt => "APT",
            Source::Dnf => "DNF",
            Source::Zypper => "Zypper",
            Source::Yum => "YUM",
            Source::Rpm => "RPM",
            Source::Xbps => "XBPS",
            Source::Apk => "APK",
            Source::Portage => "Portage",
            Source::Eopkg => "eopkg",
            Source::Nix => "Nix",
            Source::Flatpak => "Flatpak",
            Source::Snap => "Snap",
            Source::AppImage => "AppImage",
            Source::Mandara => "Mandara",
            Source::Local => t("Local"),
            Source::Unknown => t("Unknown"),
        }
    }

    /// A system package manager (removal needs polkit).
    pub fn is_system(self) -> bool {
        matches!(self, Source::Pacman | Source::Apt | Source::Dnf | Source::Zypper | Source::Yum | Source::Rpm | Source::Xbps | Source::Apk | Source::Portage | Source::Eopkg)
    }
}

/// The system package managers present, in probe order, with the program
/// that answers "who owns this file".
const SYSTEM_PROBES: &[(&str, Source)] = &[("pacman", Source::Pacman), ("dpkg", Source::Apt), ("rpm", Source::Rpm), ("xbps-query", Source::Xbps), ("apk", Source::Apk), ("qfile", Source::Portage), ("eopkg", Source::Eopkg)];

/// RPM front-ends, best first.
const RPM_FRONTENDS: &[(&str, Source)] = &[("dnf", Source::Dnf), ("dnf5", Source::Dnf), ("zypper", Source::Zypper), ("yum", Source::Yum)];

/// The system package manager to name in badges (first present), if any.
pub fn system_manager(runner: &dyn Runner) -> Option<Source> {
    let (_, source) = SYSTEM_PROBES.iter().find(|(probe, _)| runner.has(probe))?;
    Some(if *source == Source::Rpm { rpm_frontend(runner).0 } else { *source })
}

fn rpm_frontend(runner: &dyn Runner) -> (Source, &'static str) {
    RPM_FRONTENDS.iter().find(|(p, _)| runner.has(p)).map(|(p, s)| (*s, *p)).unwrap_or((Source::Rpm, "rpm"))
}

/// The source of an app judged from paths only (no command runs): cheap
/// enough for a badge on every app. System entries get `system`.
pub fn quick_source(app: &AppRef, env: &Env, system: Option<Source>) -> Source {
    let path = app.path.to_string_lossy();
    if mandara_installed(app, env).is_some() {
        Source::Mandara
    } else if app.flatpak.is_some() || path.contains("/flatpak/exports/") {
        Source::Flatpak
    } else if app.snap.is_some() || path.starts_with(SNAP_DESKTOP_DIR) {
        Source::Snap
    } else if appimage_of(app).is_some() {
        Source::AppImage
    } else if nix_store_path(&app.path).is_some() || path.contains("/.nix-profile/") || path.starts_with(NIX_SYSTEM_PROFILE) {
        Source::Nix
    } else if app.path.starts_with(&env.home) {
        Source::Local
    } else {
        system.unwrap_or(Source::Unknown)
    }
}

const SNAP_DESKTOP_DIR: &str = "/var/lib/snapd/desktop";
const NIX_STORE: &str = "/nix/store/";
const NIX_SYSTEM_PROFILE: &str = "/run/current-system";

/// The AppImage an entry starts (`Exec=/home/me/Apps/Foo-1.2.AppImage`).
fn appimage_of(app: &AppRef) -> Option<PathBuf> {
    let program = app.program();
    program.to_ascii_lowercase().ends_with(".appimage").then(|| PathBuf::from(program)).filter(|p| p.is_absolute())
}

/// `/nix/store/<hash>-<name>` of a path inside the store (resolving links).
fn nix_store_path(path: &Path) -> Option<PathBuf> {
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let rest = real.to_str()?.strip_prefix(NIX_STORE)?;
    let top = rest.split('/').next().filter(|s| !s.is_empty())?;
    Some(PathBuf::from(NIX_STORE).join(top))
}

/// `<hash>-firefox-128.0` → `firefox`.
fn nix_pname(store: &Path) -> String {
    let base = store.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let name = base.split_once('-').map(|(_, n)| n).unwrap_or(&base);
    strip_version(name, '-')
}

/// Cuts `name-1.2.3` (or `name_1.2`) at the first separator followed by a digit.
fn strip_version(name: &str, sep: char) -> String {
    let bytes: Vec<char> = name.chars().collect();
    for i in 0..bytes.len().saturating_sub(1) {
        if bytes[i] == sep && bytes[i + 1].is_ascii_digit() && i > 0 {
            return bytes[..i].iter().collect();
        }
    }
    name.to_string()
}

/// Who owns an app.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Owner {
    pub source: Source,
    /// Package name, Flatpak / Snap id, Nix element; empty for files.
    pub package: String,
    /// Installed for this user only (no polkit needed).
    pub user: bool,
    /// Files removed as part of the uninstall (an AppImage, a local
    /// entry, its icon) — they go to the Trash.
    pub files: Vec<PathBuf>,
}

impl Owner {
    fn new(source: Source, package: impl Into<String>, user: bool) -> Self {
        Self { source, package: package.into(), user, files: Vec::new() }
    }
}

/// The app installer's record of `app`, for apps it installed as files
/// (AppImage / portable).
fn mandara_installed(app: &AppRef, env: &Env) -> Option<crate::app_install::registry::Entry> {
    crate::app_install::registry::by_desktop(&env.data_home, &app.path).filter(|e| e.family.is_some_and(|f| f.is_rootless_files()))
}

/// Finds who installed `app`. Apps from Mandara's installer remove
/// exactly the files it recorded; a Flatpak it installed also loses its
/// PATH shim.
pub fn detect_owner(app: &AppRef, env: &Env, runner: &dyn Runner) -> Owner {
    if let Some(entry) = mandara_installed(app, env) {
        let mut owner = Owner::new(Source::Mandara, entry.id, true);
        owner.files = entry.paths.into_iter().filter(|p| p.starts_with(&env.home) && p.symlink_metadata().is_ok()).collect();
        if !owner.files.contains(&app.path) {
            owner.files.push(app.path.clone());
        }
        return owner;
    }
    let mut owner = detect_owner_by_path(app, env, runner);
    if owner.source == Source::Flatpak
        && let Some(shim) = crate::app_install::registry::by_flatpak(&env.data_home, &owner.package).and_then(|e| e.shim)
        && shim.symlink_metadata().is_ok()
    {
        owner.files.push(shim);
    }
    owner
}

fn detect_owner_by_path(app: &AppRef, env: &Env, runner: &dyn Runner) -> Owner {
    let path = app.path.to_string_lossy().to_string();
    let user = app.path.starts_with(&env.home);
    // Flatpak exports (`X-Flatpak`, or the exports folder).
    if app.flatpak.is_some() || path.contains("/flatpak/exports/") {
        let id = app.flatpak.clone().unwrap_or_else(|| app.stem().to_string());
        return Owner::new(Source::Flatpak, id, user);
    }
    // Snaps: `/var/lib/snapd/desktop/applications/<snap>_<app>.desktop`.
    if app.snap.is_some() || path.starts_with(SNAP_DESKTOP_DIR) {
        let name = app.snap.clone().unwrap_or_else(|| app.stem().split('_').next().unwrap_or_default().to_string());
        return Owner::new(Source::Snap, name, false);
    }
    if let Some(image) = appimage_of(app) {
        let mut owner = Owner::new(Source::AppImage, String::new(), true);
        if image.starts_with(&env.home) {
            owner.files.push(image);
        }
        if user {
            owner.files.push(app.path.clone());
        }
        owner.files.extend(appimage_icons(app, env));
        return owner;
    }
    if let Some(store) = nix_store_path(&app.path).or_else(|| resolve_program(app).and_then(|p| nix_store_path(&p))) {
        // The system profile is declared in configuration.nix: refused later.
        let user = !path.starts_with(NIX_SYSTEM_PROFILE);
        return Owner::new(Source::Nix, store.to_string_lossy().to_string(), user);
    }
    if user {
        let mut owner = Owner::new(Source::Local, String::new(), true);
        owner.files.push(app.path.clone());
        return owner;
    }
    // System packages: ask every manager present who owns the entry, then
    // its program.
    let mut targets = vec![app.path.clone()];
    targets.extend(resolve_program(app));
    for (probe, source) in SYSTEM_PROBES.iter().filter(|(p, _)| runner.has(p)) {
        for target in &targets {
            if let Some(package) = query_owner(*source, target, runner) {
                let source = if *source == Source::Rpm { rpm_frontend(runner).0 } else { *source };
                tracing::debug!("{} owns {} ({probe})", package, target.display());
                return Owner::new(source, package, false);
            }
        }
    }
    Owner::new(Source::Unknown, String::new(), false)
}

/// The absolute path of the entry's program (searched on `$PATH`).
fn resolve_program(app: &AppRef) -> Option<PathBuf> {
    let program = app.program();
    if program.is_empty() {
        return None;
    }
    if program.starts_with('/') {
        return Some(PathBuf::from(program));
    }
    crate::updates::find_program(&program)
}

/// AppImageLauncher / appimaged icons: `Icon=appimagekit_<hash>_<name>`
/// installed under `~/.local/share/icons`, or an absolute icon path in
/// `$HOME` that names the AppImage.
fn appimage_icons(app: &AppRef, env: &Env) -> Vec<PathBuf> {
    let icon = app.icon.trim();
    if icon.starts_with('/') {
        let p = PathBuf::from(icon);
        return if p.starts_with(&env.home) && p.exists() { vec![p] } else { Vec::new() };
    }
    if !icon.starts_with(APPIMAGEKIT_PREFIX) {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut stack = vec![env.data_home.join("icons")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let Ok(kind) = e.file_type() else { continue };
            let p = e.path();
            if kind.is_dir() {
                stack.push(p);
            } else if p.file_stem().is_some_and(|s| s == icon) {
                found.push(p);
            }
        }
    }
    found.sort();
    found
}
const APPIMAGEKIT_PREFIX: &str = "appimagekit_";

/// Asks one package manager who owns `path`.
fn query_owner(source: Source, path: &Path, runner: &dyn Runner) -> Option<String> {
    let p = path.to_string_lossy().to_string();
    let cmd: Vec<String> = match source {
        Source::Pacman => argv(&["pacman", "-Qoq", &p]),
        Source::Apt => argv(&["dpkg", "-S", &p]),
        Source::Rpm => argv(&["rpm", "-qf", "--qf", "%{NAME}\\n", &p]),
        Source::Xbps => argv(&["xbps-query", "-o", &p]),
        Source::Apk => argv(&["apk", "info", "--who-owns", &p]),
        Source::Portage => argv(&["qfile", "-qC", &p]),
        Source::Eopkg => argv(&["eopkg", "search-file", &p]),
        _ => return None,
    };
    let out = runner.run(&cmd, QUERY_TIMEOUT).ok()?;
    if !out.success() {
        return None;
    }
    parse_owner(source, &out.stdout)
}

/// The package name in an owner query's output.
pub fn parse_owner(source: Source, stdout: &str) -> Option<String> {
    let line = stdout.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("diversion"))?;
    let name = match source {
        // `firefox`
        Source::Pacman | Source::Rpm | Source::Portage => line.to_string(),
        // `firefox-esr:amd64, foo: /usr/share/applications/firefox.desktop`
        Source::Apt => {
            let (pkgs, _) = line.split_once(": ")?;
            let first = pkgs.split(',').next()?.trim();
            first.split(':').next()?.to_string()
        }
        // `firefox-118.0_1: /usr/share/applications/firefox.desktop (regular file)`
        Source::Xbps => {
            let (pkgver, _) = line.split_once(": ")?;
            pkgver.rsplit_once('-').map(|(n, _)| n).unwrap_or(pkgver).to_string()
        }
        // `/usr/share/applications/firefox.desktop is owned by firefox-118.0-r0`
        Source::Apk => {
            let (_, pkgver) = line.split_once(" is owned by ")?;
            let mut parts = pkgver.trim().rsplitn(3, '-');
            let (_rel, _ver) = (parts.next()?, parts.next()?);
            parts.next()?.to_string()
        }
        // `Package firefox has file /usr/share/applications/firefox.desktop`
        Source::Eopkg => stdout.lines().find_map(|l| l.trim().strip_prefix("Package ").and_then(|r| r.split_whitespace().next()))?.to_string(),
        _ => return None,
    };
    (!name.is_empty() && !name.contains(char::is_whitespace)).then_some(name)
}

// ── Commands per source ──────────────────────────────────────────────────

/// The removal argv (without `pkexec`).
fn remove_argv(owner: &Owner, runner: &dyn Runner) -> Option<Vec<String>> {
    let pkg = owner.package.as_str();
    Some(match owner.source {
        Source::Pacman => argv(&["pacman", "-Rs", "--noconfirm", pkg]),
        Source::Apt => argv(&["apt-get", "remove", "-y", pkg]),
        Source::Dnf | Source::Yum => argv(&[rpm_frontend(runner).1, "remove", "-y", pkg]),
        Source::Zypper => argv(&["zypper", "--non-interactive", "remove", pkg]),
        Source::Rpm => argv(&["rpm", "-e", pkg]),
        Source::Xbps => argv(&["xbps-remove", "-Ry", pkg]),
        Source::Apk => argv(&["apk", "del", pkg]),
        Source::Portage => argv(&["emerge", "--depclean", pkg]),
        Source::Eopkg => argv(&["eopkg", "remove", "-y", pkg]),
        Source::Flatpak => argv(&["flatpak", "uninstall", if owner.user { "--user" } else { "--system" }, "--noninteractive", "-y", pkg]),
        Source::Snap => argv(&["snap", "remove", pkg]),
        Source::Nix if runner.has("nix") && nix_profile_v2(runner) => argv(&["nix", "profile", "remove", pkg]),
        Source::Nix => {
            let name = nix_pname(Path::new(pkg));
            argv(&["nix-env", "-e", &name])
        }
        Source::AppImage | Source::Mandara | Source::Local | Source::Unknown => return None,
    })
}

/// `nix profile` manages the user profile (a `manifest.json` exists).
fn nix_profile_v2(runner: &dyn Runner) -> bool {
    runner.run(&argv(&["nix", "profile", "list", "--json"]), QUERY_TIMEOUT).is_ok_and(|o| o.success())
}

/// Removals that need root go through polkit.
fn needs_pkexec(owner: &Owner) -> bool {
    owner.source.is_system() || owner.source == Source::Snap
}

/// The package manager's own dry run (read-only, unprivileged).
fn preview_argv(owner: &Owner, runner: &dyn Runner) -> Option<Vec<String>> {
    let pkg = owner.package.as_str();
    Some(match owner.source {
        Source::Pacman => argv(&["pacman", "-Rs", "--print", "--print-format", "%n", pkg]),
        Source::Apt => argv(&["apt-get", "-s", "remove", pkg]),
        Source::Dnf | Source::Yum => argv(&[rpm_frontend(runner).1, "remove", "--assumeno", pkg]),
        Source::Zypper => argv(&["zypper", "--non-interactive", "remove", "--dry-run", pkg]),
        Source::Rpm => argv(&["rpm", "-e", "--test", pkg]),
        Source::Xbps => argv(&["xbps-remove", "-Rn", pkg]),
        Source::Apk => argv(&["apk", "del", "--simulate", pkg]),
        Source::Portage => argv(&["emerge", "--pretend", "--depclean", pkg]),
        Source::Eopkg => argv(&["eopkg", "remove", "--dry-run", pkg]),
        _ => return None,
    })
}

/// What a dry run said.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Preview {
    /// Packages removed (may include the package itself).
    pub removes: Vec<String>,
    /// Reasons the package manager refuses.
    pub blockers: Vec<String>,
    /// The output couldn't be read (the plan still shows the command).
    pub unreadable: bool,
}

/// Reads a dry run's output.
pub fn parse_preview(source: Source, package: &str, out: &CmdOutput) -> Preview {
    let all = out.all();
    let mut p = Preview::default();
    let manager = source.label();
    let says = |msg: &str| tf("{manager} says: {message}", &[("manager", &manager), ("message", &msg.trim())]);
    match source {
        Source::Pacman => {
            if out.success() {
                p.removes = out.stdout.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with(':')).map(String::from).collect();
            } else {
                p.blockers = all.lines().filter(|l| l.contains("breaks dependency") || l.contains("required by")).map(says).collect();
            }
        }
        Source::Apt => {
            p.removes = out.stdout.lines().filter_map(|l| l.strip_prefix("Remv ")).filter_map(|r| r.split_whitespace().next()).map(String::from).collect();
            if all.to_ascii_lowercase().contains("essential packages") {
                p.blockers.push(says(t("removing it would remove essential packages")));
            }
        }
        Source::Dnf | Source::Yum | Source::Zypper => {
            if all.to_ascii_lowercase().contains("protected") {
                p.blockers.push(says(all.lines().find(|l| l.to_ascii_lowercase().contains("protected")).unwrap_or_default()));
            }
            let mut on = false;
            for line in out.stdout.lines() {
                let l = line.trim();
                let lower = l.to_ascii_lowercase();
                if lower.starts_with("removing") || lower.contains("going to be removed") {
                    on = true;
                    continue;
                }
                if l.is_empty() || lower.starts_with("transaction summary") || lower.starts_with("operation aborted") || lower.starts_with("installing") || lower.starts_with("upgrading") {
                    on = false;
                    continue;
                }
                if on {
                    // dnf: one package per line (`firefox x86_64 …`);
                    // zypper: several names per line.
                    if source == Source::Zypper {
                        p.removes.extend(l.split_whitespace().map(String::from));
                    } else if let Some(name) = l.split_whitespace().next().filter(|w| !w.starts_with('=') && !w.eq_ignore_ascii_case("package")) {
                        p.removes.push(name.to_string());
                    }
                }
            }
        }
        Source::Rpm => {
            // `rpm -e --test`: `foo is needed by (installed) bar-1.0.x86_64`.
            let needed: Vec<String> = all.lines().filter_map(|l| l.split_once("needed by (installed) ").map(|(_, r)| r.trim().to_string())).collect();
            if !needed.is_empty() {
                p.blockers.push(says(&tf("needed by {packages}", &[("packages", &needed.join(", "))])));
            } else if out.success() {
                p.removes.push(package.to_string());
            }
        }
        Source::Xbps => {
            p.removes = out.stdout.lines().filter(|l| l.contains(" remove ")).filter_map(|l| l.split_whitespace().next()).map(|v| v.rsplit_once('-').map(|(n, _)| n).unwrap_or(v).to_string()).collect();
            p.blockers = all.lines().filter(|l| l.contains("breaks installed")).map(says).collect();
        }
        Source::Apk => {
            p.removes = all.lines().filter_map(|l| l.split_once("Purging ").and_then(|(_, r)| r.split_whitespace().next())).map(String::from).collect();
        }
        Source::Portage => {
            p.removes = all.lines().filter_map(|l| l.trim().strip_prefix("selected:")).flat_map(|r| r.split_whitespace().map(String::from).collect::<Vec<_>>()).filter(|s| s != "none").collect();
            if out.success() && p.removes.is_empty() {
                p.blockers.push(says(t("other packages or the system set still need it")));
            }
        }
        // `The following list of packages will be removed` / `in the
        // respective order to satisfy dependencies:` / `gimp babl`.
        Source::Eopkg => {
            let mut lines = all.lines().skip_while(|l| !l.contains("will be removed")).skip(1);
            if let Some(list) = lines.find(|l| !l.trim().is_empty() && !l.trim_end().ends_with(':')) {
                p.removes.extend(list.split_whitespace().map(String::from));
            }
        }
        _ => {}
    }
    p.removes.dedup();
    p.unreadable = p.removes.is_empty() && p.blockers.is_empty();
    p
}

/// Essential / required packages as the package manager flags them.
fn flagged_essential(owner: &Owner, runner: &dyn Runner) -> bool {
    match owner.source {
        Source::Apt => runner
            .run(&argv(&["dpkg-query", "-W", "-f=${Essential} ${Priority}", &owner.package]), QUERY_TIMEOUT)
            .is_ok_and(|o| o.success() && (o.stdout.split_whitespace().next() == Some("yes") || o.stdout.split_whitespace().nth(1) == Some("required"))),
        _ => false,
    }
}

// ── Leftovers ────────────────────────────────────────────────────────────

/// Where a leftover lives.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LeftoverKind {
    Config,
    Data,
    State,
    Cache,
    /// `~/.var/app/<id>`, `~/snap/<name>`.
    Sandbox,
    /// A program file in `$HOME` a local entry starts.
    Program,
}

impl LeftoverKind {
    pub fn label(self) -> &'static str {
        match self {
            LeftoverKind::Config => t("Settings"),
            LeftoverKind::Data => t("App data"),
            LeftoverKind::State => t("State"),
            LeftoverKind::Cache => t("Cache"),
            LeftoverKind::Sandbox => t("Sandbox data"),
            LeftoverKind::Program => t("Program file"),
        }
    }
}

/// A file or folder the app likely left in `$HOME`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Leftover {
    pub path: PathBuf,
    pub bytes: u64,
    pub kind: LeftoverKind,
}

/// Folder names never offered (shared or system state, Mandara's own).
const NEVER_NAMES: &[&str] = &[
    "autostart", "applications", "systemd", "environment.d", "dconf", "fontconfig", "fonts", "icons", "themes", "mime", "menus", "gtk-2.0", "gtk-3.0", "gtk-4.0", "pulse", "pipewire", "wireplumber", "keyrings", "gnupg", "ssh", "trash", "flatpak", "snap",
    "nix", "user-dirs.dirs", "user-dirs.locale", "mimeapps.list", "recently-used.xbel", "xdg-desktop-portal", "dbus-1", "desktop-directories", "sounds", "color-schemes", "kdeglobals", "qt5ct", "qt6ct", "kvantum",
];
/// Program names that say nothing about the app (launchers, interpreters).
const GENERIC_PROGRAMS: &[&str] =
    &["env", "sh", "bash", "zsh", "fish", "python", "python3", "java", "node", "electron", "wine", "flatpak", "snap", "gtk-launch", "xdg-open", "sudo", "pkexec", "dbus-launch", "exo-open", "kioclient", "kioclient5", "kstart", "kstart5", "uwsm", "app2unit", "mono", "perl", "ruby", "desktop", "app", "apps", "client", "main", "bin", "run", "launcher", "start"];

fn never_offered(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("mandara") || NEVER_NAMES.contains(&n.as_str()) || GENERIC_PROGRAMS.contains(&n.as_str())
}

/// Names the app's leftovers may carry: its desktop id, the last part of
/// a reverse-DNS id, its program, `StartupWMClass`, package, Flatpak / Snap
/// id and AppImage name. Lowercased, deduplicated, generic names dropped.
pub fn residual_names(app: &AppRef, owner: &Owner) -> Vec<String> {
    let mut names = vec![app.stem().to_string(), app.wm_class.clone()];
    let parts: Vec<&str> = app.stem().split('.').collect();
    if parts.len() >= REVERSE_DNS_MIN_PARTS
        && let Some(last) = parts.last().filter(|l| l.len() >= LAST_SEGMENT_MIN_LEN)
    {
        names.push(last.to_string());
    }
    let program = app.program();
    let base = Path::new(&program).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    if base.to_ascii_lowercase().ends_with(".appimage") {
        let stem = &base[..base.len() - ".appimage".len()];
        names.push(strip_version(&strip_version(stem, '-'), '_'));
    } else {
        names.push(base);
    }
    match owner.source {
        Source::Nix => names.push(nix_pname(Path::new(&owner.package))),
        Source::Flatpak | Source::Snap => names.push(owner.package.clone()),
        s if s.is_system() => names.push(owner.package.clone()),
        _ => {}
    }
    let mut out: Vec<String> = names.into_iter().map(|n| n.trim().to_ascii_lowercase()).filter(|n| n.len() >= MIN_NAME_LEN && !n.contains('/') && n != "." && n != ".." && !never_offered(n)).collect();
    out.sort();
    out.dedup();
    out
}

/// Leftovers in `$HOME`: entries of the XDG config / data / state / cache
/// folders whose name equals (ignoring case) one of [`residual_names`],
/// plus `~/.var/app/<flatpak id>` and `~/snap/<snap>`. Never anything
/// outside `$HOME`, never the folders themselves, nothing fuzzy.
pub fn scan_leftovers(app: &AppRef, owner: &Owner, env: &Env) -> Vec<Leftover> {
    let names = residual_names(app, owner);
    let mut out: Vec<Leftover> = Vec::new();
    let home = &env.home;
    let inside = |p: &Path| !home.as_os_str().is_empty() && p.starts_with(home) && p != home.as_path();
    let bases = [(&env.config_home, LeftoverKind::Config), (&env.data_home, LeftoverKind::Data), (&env.state_home, LeftoverKind::State), (&env.cache_home, LeftoverKind::Cache)];
    for (base, kind) in bases {
        if !inside(base) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(base) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            if names.contains(&name) && !never_offered(&name) {
                out.push(Leftover { path: e.path(), bytes: 0, kind });
            }
        }
    }
    let mut sandbox = |p: PathBuf| {
        if inside(&p) && p.symlink_metadata().is_ok() {
            out.push(Leftover { path: p, bytes: 0, kind: LeftoverKind::Sandbox });
        }
    };
    let flatpak_id = (owner.source == Source::Flatpak).then(|| owner.package.clone()).or_else(|| app.flatpak.clone());
    if let Some(id) = flatpak_id.filter(|i| !i.is_empty() && !i.contains('/')) {
        sandbox(home.join(".var/app").join(id));
    }
    if owner.source == Source::Snap && !owner.package.is_empty() && !owner.package.contains('/') {
        sandbox(home.join("snap").join(&owner.package));
    }
    out.retain(|l| inside(&l.path) && !owner.files.contains(&l.path));
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.dedup_by(|a, b| a.path == b.path);
    for l in &mut out {
        l.bytes = disk_size(&l.path);
    }
    out
}

/// Bytes under `path` (links not followed; stops after a cap of entries).
pub fn disk_size(path: &Path) -> u64 {
    let mut total = 0;
    let mut seen = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(meta) = p.symlink_metadata() else { continue };
        seen += 1;
        if seen > SIZE_SCAN_MAX_ENTRIES {
            break;
        }
        if meta.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&p) {
                stack.extend(entries.flatten().map(|e| e.path()));
            }
        } else {
            total += meta.len();
        }
    }
    total
}

// ── Plan ─────────────────────────────────────────────────────────────────

/// Everything removal would do, shown before anything happens.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Plan {
    pub app: AppRef,
    pub owner: Owner,
    /// The exact argv that runs (with `pkexec` for system packages).
    pub command: Option<Vec<String>>,
    /// Other packages the package manager removes too.
    pub also_removes: Vec<String>,
    /// Why it can't be uninstalled here (empty = it can).
    pub blockers: Vec<String>,
    /// Needs an extra "yes, I mean it" (a default app).
    pub cautions: Vec<String>,
    /// The dry run couldn't be read; shown next to the command.
    pub preview_note: Option<String>,
    /// Optional cleanup, unchecked by default.
    pub leftovers: Vec<Leftover>,
}

impl Plan {
    pub fn can_uninstall(&self) -> bool {
        self.blockers.is_empty()
    }
}

/// Works out how to uninstall `app` (runs only read-only queries).
pub fn plan(app: &AppRef, env: &Env, runner: &dyn Runner) -> Plan {
    let owner = detect_owner(app, env, runner);
    let mut blockers = Vec::new();
    let mut cautions = Vec::new();
    let mut also_removes = Vec::new();
    let mut preview_note = None;
    if app.id.starts_with(OWN_ID_PREFIX) {
        blockers.push(t("Mandara can't uninstall itself from here").to_string());
    }
    match owner.source {
        Source::Unknown => blockers.push(t("No package manager on this system says it installed this app").to_string()),
        Source::Nix if !owner.user => blockers.push(t("Installed by your NixOS configuration: remove it from configuration.nix").to_string()),
        _ => {}
    }
    if owner.source.is_system() {
        let vital = vital_packages(&owner, env, runner);
        if is_protected_package(&owner.package) || flagged_essential(&owner, runner) || vital.contains(&owner.package) {
            blockers.push(tf("{package} is part of your system and can't be removed here", &[("package", &owner.package)]));
        }
        if let Some(cmd) = preview_argv(&owner, runner) {
            match runner.run(&cmd, QUERY_TIMEOUT) {
                Ok(out) => {
                    let mut preview = parse_preview(owner.source, &owner.package, &out);
                    // dnf / zypper may insist on root even for a dry run:
                    // ask rpm what depends on the package instead.
                    if preview.unreadable
                        && matches!(owner.source, Source::Dnf | Source::Zypper | Source::Yum)
                        && runner.has("rpm")
                        && let Ok(test) = runner.run(&argv(&["rpm", "-e", "--test", &owner.package]), QUERY_TIMEOUT)
                    {
                            let needed: Vec<String> = test.all().lines().filter_map(|l| l.split_once("needed by (installed) ").map(|(_, r)| r.trim().to_string())).collect();
                            preview = Preview { removes: needed, blockers: Vec::new(), unreadable: !test.success() && test.stderr.contains("not installed") };
                    }
                    if preview.unreadable && !out.success() {
                        preview_note = Some(tf("Couldn't preview what else gets removed: {error}", &[("error", &last_lines(&out, 1))]));
                    }
                    for name in &preview.removes {
                        if name != &owner.package && (is_protected_package(name) || vital.contains(name)) {
                            blockers.push(tf("{package} is part of your system and can't be removed here", &[("package", name)]));
                        }
                    }
                    also_removes = preview.removes.into_iter().filter(|n| n != &owner.package).collect();
                    blockers.extend(preview.blockers);
                }
                Err(e) => preview_note = Some(tf("Couldn't preview what else gets removed: {error}", &[("error", &e)])),
            }
        }
    }
    let lower_id = app.id.to_ascii_lowercase();
    if env.default_apps.iter().any(|d| desktop_id(d).to_ascii_lowercase() == lower_id) {
        cautions.push(tf("{app} is one of your default apps (terminal, browser or file manager)", &[("app", &app.name)]));
    }
    let command = remove_argv(&owner, runner).map(|cmd| if needs_pkexec(&owner) { std::iter::once(PKEXEC.to_string()).chain(cmd).collect() } else { cmd });
    let mut leftovers = scan_leftovers(app, &owner, env);
    // A local entry's program in `$HOME` (`~/.local/bin/tool`) is offered,
    // unchecked, like a leftover.
    if owner.source == Source::Local
        && let Some(program) = resolve_program(app).filter(|p| p.starts_with(&env.home) && p.is_file())
    {
        leftovers.push(Leftover { bytes: disk_size(&program), path: program, kind: LeftoverKind::Program });
    }
    blockers.dedup();
    Plan { app: app.clone(), owner, command, also_removes, blockers, cautions, preview_note, leftovers }
}

/// Packages owning the shell, the compositor and the portal (never removed).
fn vital_packages(owner: &Owner, env: &Env, runner: &dyn Runner) -> Vec<String> {
    let probe = match owner.source {
        Source::Dnf | Source::Zypper | Source::Yum => Source::Rpm,
        s => s,
    };
    env.vital_programs.iter().filter_map(|p| query_owner(probe, p, runner)).collect()
}

fn last_lines(out: &CmdOutput, n: usize) -> String {
    let text = if out.stderr.trim().is_empty() { &out.stdout } else { &out.stderr };
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

// ── Execution ────────────────────────────────────────────────────────────

/// What the user picked in the confirmation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Choices {
    /// Leftovers to move to the Trash (each must be in the plan).
    pub purge: Vec<PathBuf>,
    /// Yes to every caution (removing a default app).
    pub confirm_cautions: bool,
    pub dry_run: bool,
}

/// What happened (or, in a dry run, would happen).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Outcome {
    pub dry_run: bool,
    /// The command that ran (or would run).
    pub command: Option<Vec<String>>,
    /// Moved to the Trash: original → trashed location (empty in a dry run).
    pub trashed: Vec<(PathBuf, PathBuf)>,
    /// What would move to the Trash (dry run) or failed to.
    pub would_trash: Vec<PathBuf>,
    /// Non-fatal problems (a leftover that couldn't be trashed).
    pub errors: Vec<String>,
}

/// Uninstalls per `plan` and `choices`. Refuses blocked plans, unconfirmed
/// cautions and purge paths that aren't the plan's leftovers. A failed
/// command returns the package manager's message and trashes nothing.
pub fn execute(plan: &Plan, choices: &Choices, env: &Env, runner: &dyn Runner) -> Result<Outcome, String> {
    if let Some(b) = plan.blockers.first() {
        return Err(b.clone());
    }
    if !plan.cautions.is_empty() && !choices.confirm_cautions {
        return Err(plan.cautions.join("; "));
    }
    for p in &choices.purge {
        if !plan.leftovers.iter().any(|l| &l.path == p) {
            return Err(tf("Not one of this app's leftovers: {path}", &[("path", &p.display())]));
        }
    }
    let mut files: Vec<PathBuf> = plan.owner.files.iter().filter(|f| f.symlink_metadata().is_ok()).cloned().collect();
    files.extend(choices.purge.iter().cloned());
    let dry_run = choices.dry_run || env.force_dry_run;
    let mut outcome = Outcome { dry_run, command: plan.command.clone(), ..Default::default() };
    if dry_run {
        outcome.would_trash = files;
        return Ok(outcome);
    }
    if let Some(cmd) = &plan.command {
        let out = runner.run(cmd, REMOVE_TIMEOUT)?;
        if !out.success() {
            let msg = last_lines(&out, ERROR_LINES);
            return Err(if msg.is_empty() { tf("{program} exited with status {code}", &[("program", &cmd[0]), ("code", &out.code.unwrap_or(-1))]) } else { msg });
        }
    }
    for f in files {
        match trash(&f, env) {
            Ok(to) => outcome.trashed.push((f, to)),
            Err(e) => {
                outcome.errors.push(e);
                outcome.would_trash.push(f);
            }
        }
    }
    // Forget what the app installer recorded (no-op for other apps).
    if let Err(e) = crate::app_install::registry::forget(&env.data_home, &plan.app.path, &plan.owner.package) {
        outcome.errors.push(e);
    }
    Ok(outcome)
}

// ── Trash (freedesktop.org Trash spec 1.0) ───────────────────────────────

/// Moves `path` to the Trash: `$XDG_DATA_HOME/Trash` when it's on the same
/// drive, else `$topdir/.Trash-$uid`. Writes the `.trashinfo` first
/// (`O_EXCL`), then renames; never copies, never deletes. Returns where
/// the file went.
pub fn trash(path: &Path, env: &Env) -> Result<PathBuf, String> {
    use std::io::Write as _;
    use std::os::unix::fs::MetadataExt as _;
    let refuse = || tf("Refusing to move {path} to the Trash", &[("path", &path.display())]);
    if !path.is_absolute() || path.parent().is_none() || env.home.starts_with(path) || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(refuse());
    }
    let fail = |e: &dyn std::fmt::Display| tf("Can't move {path} to the Trash: {error}", &[("path", &path.display()), ("error", &e)]);
    path.symlink_metadata().map_err(|e| fail(&e))?;
    let parent = path.parent().ok_or_else(refuse)?;
    let dev = parent.metadata().map_err(|e| fail(&e))?.dev();
    let home_trash = env.data_home.join("Trash");
    std::fs::create_dir_all(home_trash.join("files")).map_err(|e| fail(&e))?;
    std::fs::create_dir_all(home_trash.join("info")).map_err(|e| fail(&e))?;
    let home_dev = home_trash.metadata().map_err(|e| fail(&e))?.dev();
    let (trash_dir, recorded) = if dev == home_dev {
        (home_trash, path.to_path_buf())
    } else {
        let top = mount_top(parent, dev);
        let dir = top.join(format!(".Trash-{}", env.uid));
        for sub in ["files", "info"] {
            std::fs::create_dir_all(dir.join(sub)).map_err(|e| fail(&e))?;
        }
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(TRASH_DIR_MODE));
        }
        (dir, path.strip_prefix(&top).unwrap_or(path).to_path_buf())
    };
    let name = path.file_name().ok_or_else(refuse)?.to_string_lossy().to_string();
    let date = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
    for n in 1..=TRASH_NAME_ATTEMPTS {
        let candidate = if n == 1 { name.clone() } else { format!("{name}.{n}") };
        let info = trash_dir.join("info").join(format!("{candidate}.trashinfo"));
        let dest = trash_dir.join("files").join(&candidate);
        if dest.symlink_metadata().is_ok() {
            continue;
        }
        let mut file = match std::fs::OpenOptions::new().write(true).create_new(true).open(&info) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(fail(&e)),
        };
        let text = format!("[Trash Info]\nPath={}\nDeletionDate={date}\n", percent_encode(&recorded));
        if let Err(e) = file.write_all(text.as_bytes()) {
            let _ = std::fs::remove_file(&info);
            return Err(fail(&e));
        }
        return match std::fs::rename(path, &dest) {
            Ok(()) => Ok(dest),
            Err(e) => {
                let _ = std::fs::remove_file(&info);
                Err(fail(&e))
            }
        };
    }
    Err(refuse())
}
/// `.Trash-$uid` is private to its user.
const TRASH_DIR_MODE: u32 = 0o700;
/// `name`, `name.2`, … `name.<this>`.
const TRASH_NAME_ATTEMPTS: usize = 10_000;

/// The top directory of the mount holding `dir` (whose device is `dev`).
fn mount_top(dir: &Path, dev: u64) -> PathBuf {
    use std::os::unix::fs::MetadataExt as _;
    let mut top = dir.to_path_buf();
    while let Some(parent) = top.parent() {
        if parent.metadata().map(|m| m.dev()).ok() != Some(dev) {
            break;
        }
        top = parent.to_path_buf();
    }
    top
}

/// RFC 2396 escaping of a path (`/` kept), as `.trashinfo` wants.
pub fn percent_encode(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt as _;
    let mut out = String::new();
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~/!*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ── Convenience (Studio and `mandara msg apps …`) ───────────────────────

/// Desktop ids of your default browser, file manager and terminal (from
/// `mimeapps.list` and `[apps]`): uninstalling one needs a confirmation.
pub fn default_app_ids(apps: &mandara_config::apps::AppsConfig, entries: &[mandara_config::apps::DesktopEntry]) -> Vec<String> {
    use mandara_config::apps::{default_for, exec_program, mimeapps_file};
    let file = mimeapps_file();
    let mut ids: Vec<String> = DEFAULT_MIMES.iter().filter_map(|m| default_for(&file, m)).collect();
    let base = |cmd: &str| cmd.split_whitespace().next().map(|w| Path::new(w).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()).unwrap_or_default();
    for cmd in [apps.terminal_command(), apps.file_manager_command(), apps.browser_command()].into_iter().flatten() {
        let program = base(&cmd);
        if !program.is_empty() {
            ids.extend(entries.iter().filter(|e| base(&exec_program(&e.exec)) == program).map(|e| e.id.clone()));
        }
    }
    ids.sort();
    ids.dedup();
    ids
}
/// The defaults whose apps need a confirmation: browser, file manager.
const DEFAULT_MIMES: &[&str] = &["x-scheme-handler/https", "inode/directory"];

/// [`Env::current`] plus your default apps.
pub fn env_for(apps: &mandara_config::apps::AppsConfig) -> Env {
    let mut env = Env::current();
    env.default_apps = default_app_ids(apps, &mandara_config::apps::desktop_entries());
    env
}

/// One app in the Installed list.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InstalledApp {
    #[serde(flatten)]
    pub app: AppRef,
    /// Judged from paths only ([`quick_source`]).
    pub source: Source,
    pub comment: String,
    pub no_display: bool,
    /// Bytes of an AppImage (cheap); `None` for packages.
    pub bytes: Option<u64>,
}

/// Every app with a desktop entry (helpers with `NoDisplay` included and
/// flagged; `Hidden` ones skipped), sorted by name.
pub fn installed(env: &Env, runner: &dyn Runner) -> Vec<InstalledApp> {
    let system = system_manager(runner);
    let mut out: Vec<InstalledApp> = mandara_config::apps::desktop_entries()
        .into_iter()
        .filter(|e| !e.hidden)
        .filter_map(|e| {
            let text = std::fs::read_to_string(&e.path).ok()?;
            let app = AppRef::parse(&e.id, &e.path, &text);
            let source = quick_source(&app, env, system);
            let bytes = (source == Source::AppImage).then(|| appimage_of(&app).and_then(|p| p.metadata().ok()).map(|m| m.len())).flatten();
            Some(InstalledApp { app, source, comment: e.comment, no_display: e.no_display, bytes })
        })
        .collect();
    out.sort_by_key(|a| a.app.name.to_lowercase());
    out
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Records every argv; answers from canned outputs keyed by the
    /// joined argv (or its first two words), `has` from a program list.
    #[derive(Default)]
    struct Mock {
        programs: Vec<&'static str>,
        replies: HashMap<String, CmdOutput>,
        calls: Mutex<Vec<Vec<String>>>,
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
            self.calls.lock().unwrap().iter().map(|c| c.join(" ")).collect()
        }
    }

    impl Runner for Mock {
        fn run(&self, argv: &[String], _: Duration) -> Result<CmdOutput, String> {
            self.calls.lock().unwrap().push(argv.to_vec());
            Ok(self.replies.get(&argv.join(" ")).cloned().unwrap_or_else(|| CmdOutput::failed(1, "not mocked")))
        }
        fn has(&self, program: &str) -> bool {
            self.programs.contains(&program)
        }
    }

    fn app(id: &str, path: &str, text: &str) -> AppRef {
        AppRef::parse(id, Path::new(path), &format!("[Desktop Entry]\n{text}"))
    }

    fn home() -> (tempfile::TempDir, Env) {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::with_home(dir.path());
        (dir, env)
    }

    #[test]
    fn system_runner_is_disabled_in_tests() {
        assert!(SystemRunner.run(&argv(&["true"]), QUERY_TIMEOUT).is_err());
    }

    #[test]
    fn exec_program_skips_env_and_field_codes() {
        assert_eq!(app("a", "/x", "Exec=env FOO=1 /opt/a/bin/a %U").program(), "/opt/a/bin/a");
        assert_eq!(app("a", "/x", "Exec=\"/home/me/My Apps/Foo.AppImage\" --no-sandbox %u").program(), "/home/me/My Apps/Foo.AppImage");
        assert_eq!(app("a", "/x", "Exec=code --new-window").program(), "code");
    }

    #[test]
    fn owner_queries_per_manager() {
        let path = "/usr/share/applications/firefox.desktop";
        let a = app("firefox", path, "Name=Firefox\nExec=/nonexistent/firefox %u");
        let (_d, env) = home();
        let cases: Vec<(&[&'static str], String, &str, Source)> = vec![
            (&["pacman"], format!("pacman -Qoq {path}"), "firefox\n", Source::Pacman),
            (&["dpkg"], format!("dpkg -S {path}"), "firefox-esr:amd64: /usr/share/applications/firefox.desktop\n", Source::Apt),
            (&["rpm", "dnf"], format!("rpm -qf --qf %{{NAME}}\\n {path}"), "firefox\n", Source::Dnf),
            (&["rpm", "zypper"], format!("rpm -qf --qf %{{NAME}}\\n {path}"), "firefox\n", Source::Zypper),
            (&["rpm"], format!("rpm -qf --qf %{{NAME}}\\n {path}"), "firefox\n", Source::Rpm),
            (&["xbps-query"], format!("xbps-query -o {path}"), "firefox-118.0_1: /usr/share/applications/firefox.desktop (regular file)\n", Source::Xbps),
            (&["apk"], format!("apk info --who-owns {path}"), "/usr/share/applications/firefox.desktop is owned by firefox-esr-115.3.0-r0\n", Source::Apk),
            (&["qfile"], format!("qfile -qC {path}"), "www-client/firefox\n", Source::Portage),
            (&["eopkg"], format!("eopkg search-file {path}"), "Searching for /usr/share/...\nPackage firefox has file /usr/share/applications/firefox.desktop\n", Source::Eopkg),
        ];
        for (programs, cmd, reply, source) in cases {
            let mock = Mock::with(programs).reply(&cmd, CmdOutput::ok(reply));
            let owner = detect_owner(&a, &env, &mock);
            assert_eq!(owner.source, source, "{cmd}");
            let expected = match source {
                Source::Apt | Source::Apk => "firefox-esr",
                Source::Portage => "www-client/firefox",
                _ => "firefox",
            };
            assert_eq!(owner.package, expected, "{cmd}");
            assert!(!owner.user);
        }
        // Nobody owns it.
        assert_eq!(detect_owner(&a, &env, &Mock::with(&["pacman"])).source, Source::Unknown);
    }

    #[test]
    fn flatpak_snap_appimage_and_local_owners() {
        let (_d, env) = home();
        let mock = Mock::default();
        let f = app("org.gimp.GIMP", "/var/lib/flatpak/exports/share/applications/org.gimp.GIMP.desktop", "Exec=/usr/bin/flatpak run org.gimp.GIMP\nX-Flatpak=org.gimp.GIMP");
        let o = detect_owner(&f, &env, &mock);
        assert_eq!((o.source, o.package.as_str(), o.user), (Source::Flatpak, "org.gimp.GIMP", false));
        let p = plan(&f, &env, &mock);
        assert_eq!(p.command.unwrap().join(" "), "flatpak uninstall --system --noninteractive -y org.gimp.GIMP");

        let user_flatpak = env.data_home.join("flatpak/exports/share/applications/com.spotify.Client.desktop");
        let uf = app("com.spotify.Client", user_flatpak.to_str().unwrap(), "Exec=flatpak run com.spotify.Client");
        let o = detect_owner(&uf, &env, &mock);
        assert!(o.user && o.package == "com.spotify.Client");

        let s = app("spotify_spotify", "/var/lib/snapd/desktop/applications/spotify_spotify.desktop", "Exec=/snap/bin/spotify");
        let p = plan(&s, &env, &mock);
        assert_eq!((p.owner.source, p.owner.package.as_str()), (Source::Snap, "spotify"));
        assert_eq!(p.command.unwrap().join(" "), "pkexec snap remove spotify");

        let image = env.home.join("Applications/Obsidian-1.4.16.AppImage");
        std::fs::create_dir_all(image.parent().unwrap()).unwrap();
        std::fs::write(&image, b"elf").unwrap();
        let entry = env.data_home.join("applications/appimagekit_abc-Obsidian.desktop");
        let icon = env.data_home.join("icons/hicolor/256x256/apps/appimagekit_abc_obsidian.png");
        std::fs::create_dir_all(icon.parent().unwrap()).unwrap();
        std::fs::write(&icon, b"png").unwrap();
        let ai = app("appimagekit_abc-Obsidian", entry.to_str().unwrap(), &format!("Exec={} %U\nIcon=appimagekit_abc_obsidian", image.display()));
        let o = detect_owner(&ai, &env, &mock);
        assert_eq!(o.source, Source::AppImage);
        assert_eq!(o.files, vec![image.clone(), entry.clone(), icon.clone()]);
        assert!(remove_argv(&o, &mock).is_none());

        let local = app("my-tool", env.data_home.join("applications/my-tool.desktop").to_str().unwrap(), "Exec=my-tool");
        let o = detect_owner(&local, &env, &mock);
        assert_eq!((o.source, o.files.len()), (Source::Local, 1));
    }

    #[test]
    fn nix_owners() {
        let (_d, env) = home();
        let a = app("firefox", "/nix/store/abc123-firefox-128.0/share/applications/firefox.desktop", "Exec=firefox");
        let mock = Mock::with(&["nix"]).reply("nix profile list --json", CmdOutput::ok("{}"));
        let p = plan(&a, &env, &mock);
        assert_eq!(p.owner.source, Source::Nix);
        assert_eq!(p.command.unwrap().join(" "), "nix profile remove /nix/store/abc123-firefox-128.0");
        let legacy = Mock::default();
        assert_eq!(plan(&a, &env, &legacy).command.unwrap().join(" "), "nix-env -e firefox");
        assert_eq!(nix_pname(Path::new("/nix/store/abc-gnome-text-editor-46.3")), "gnome-text-editor");
    }

    fn pacman_firefox() -> (AppRef, Mock) {
        let path = "/usr/share/applications/firefox.desktop";
        let a = app("firefox", path, "Name=Firefox\nExec=/nonexistent/firefox");
        let mock = Mock::with(&["pacman"]).reply(&format!("pacman -Qoq {path}"), CmdOutput::ok("firefox\n"));
        (a, mock)
    }

    #[test]
    fn system_plan_uses_pkexec_argv_and_preview() {
        let (_d, env) = home();
        let (a, mock) = pacman_firefox();
        let mock = mock.reply("pacman -Rs --print --print-format %n firefox", CmdOutput::ok("firefox\nmozilla-common\n"));
        let p = plan(&a, &env, &mock);
        assert!(p.can_uninstall(), "{:?}", p.blockers);
        assert_eq!(p.command.clone().unwrap(), argv(&["pkexec", "pacman", "-Rs", "--noconfirm", "firefox"]));
        assert_eq!(p.also_removes, vec!["mozilla-common".to_string()]);
        // A dry run runs nothing.
        let before = mock.calls().len();
        let out = execute(&p, &Choices { dry_run: true, ..Default::default() }, &env, &mock).unwrap();
        assert!(out.dry_run && out.trashed.is_empty());
        assert_eq!(mock.calls().len(), before);
        // The forced dry run too, whatever the caller asks.
        let forced = Env { force_dry_run: true, ..env.clone() };
        assert!(execute(&p, &Choices::default(), &forced, &mock).unwrap().dry_run);
        assert_eq!(mock.calls().len(), before);
        // A real run sends exactly the plan's argv.
        let mock = mock.reply("pkexec pacman -Rs --noconfirm firefox", CmdOutput::ok(""));
        execute(&p, &Choices::default(), &env, &mock).unwrap();
        assert_eq!(mock.calls().last().unwrap(), "pkexec pacman -Rs --noconfirm firefox");
    }

    #[test]
    fn failures_carry_the_package_managers_message() {
        let (_d, env) = home();
        let (a, mock) = pacman_firefox();
        let mock = mock.reply("pacman -Rs --print --print-format %n firefox", CmdOutput::ok("firefox\n")).reply("pkexec pacman -Rs --noconfirm firefox", CmdOutput::failed(1, "error: failed to init transaction (unable to lock database)\n"));
        let p = plan(&a, &env, &mock);
        assert_eq!(execute(&p, &Choices::default(), &env, &mock).unwrap_err(), "error: failed to init transaction (unable to lock database)");
    }

    #[test]
    fn protected_and_dependent_packages_are_refused() {
        let (_d, env) = home();
        let path = "/usr/share/applications/niri.desktop";
        let a = app("niri", path, "Exec=/nonexistent/niri");
        let mock = Mock::with(&["pacman"]).reply(&format!("pacman -Qoq {path}"), CmdOutput::ok("niri\n"));
        let p = plan(&a, &env, &mock);
        assert!(!p.can_uninstall());
        assert!(execute(&p, &Choices::default(), &env, &mock).is_err());
        assert!(!mock.calls().iter().any(|c| c.starts_with("pkexec")));
        // pacman refuses: something requires it.
        let (a, mock) = pacman_firefox();
        let mock = mock.reply("pacman -Rs --print --print-format %n firefox", CmdOutput::failed(1, "error: failed to prepare transaction\n:: removing firefox breaks dependency 'firefox' required by firefox-tridactyl\n"));
        assert!(plan(&a, &env, &mock).blockers.iter().any(|b| b.contains("required by firefox-tridactyl")));
        // apt: essential flag, and essential packages in the simulation.
        let path = "/usr/share/applications/bash.desktop";
        let b = app("bashy", path, "Exec=/nonexistent/x");
        let mock = Mock::with(&["dpkg", "apt-get"]).reply(&format!("dpkg -S {path}"), CmdOutput::ok("login: /x\n")).reply("dpkg-query -W -f=${Essential} ${Priority} login", CmdOutput::ok("yes required"));
        assert!(!plan(&b, &env, &mock).can_uninstall());
        let mock = Mock::with(&["dpkg", "apt-get"])
            .reply(&format!("dpkg -S {path}"), CmdOutput::ok("gedit: /x\n"))
            .reply("apt-get -s remove gedit", CmdOutput::ok("Remv gedit [44.2-1]\nRemv gedit-plugins [44.1-1]\n"));
        let p = plan(&b, &env, &mock);
        assert!(p.can_uninstall());
        assert_eq!(p.also_removes, vec!["gedit-plugins".to_string()]);
        // A dependent that is protected blocks.
        let mock = Mock::with(&["dpkg", "apt-get"]).reply(&format!("dpkg -S {path}"), CmdOutput::ok("gedit: /x\n")).reply("apt-get -s remove gedit", CmdOutput::ok("Remv gedit [1]\nRemv xdg-desktop-portal-gnome [1]\n"));
        assert!(!plan(&b, &env, &mock).can_uninstall());
        // Mandara itself.
        let own = app("org.mandara.studio", "/usr/share/applications/org.mandara.studio.desktop", "Exec=mandara studio");
        assert!(!plan(&own, &env, &Mock::default()).can_uninstall());
    }

    #[test]
    fn default_apps_need_confirmation() {
        let (_d, mut env) = home();
        env.default_apps = vec!["firefox".into()];
        let (a, mock) = pacman_firefox();
        let mock = mock.reply("pacman -Rs --print --print-format %n firefox", CmdOutput::ok("firefox\n"));
        let p = plan(&a, &env, &mock);
        assert_eq!(p.cautions.len(), 1);
        assert!(execute(&p, &Choices { dry_run: true, ..Default::default() }, &env, &mock).is_err());
        assert!(execute(&p, &Choices { dry_run: true, confirm_cautions: true, ..Default::default() }, &env, &mock).is_ok());
    }

    #[test]
    fn previews_parse_per_manager() {
        let dnf = CmdOutput { code: Some(1), stdout: "Dependencies resolved.\n Package  Arch  Version  Repo  Size\nRemoving:\n gimp  x86_64 2.10 @fedora 90 M\nRemoving unused dependencies:\n babl  x86_64 0.1 @fedora 2 M\n\nTransaction Summary\n".into(), stderr: "Operation aborted.\n".into() };
        assert_eq!(parse_preview(Source::Dnf, "gimp", &dnf).removes, vec!["gimp", "babl"]);
        let zyp = CmdOutput::ok("The following 2 packages are going to be REMOVED:\n  gimp gimp-plugins\n\n2 packages to remove.\n");
        assert_eq!(parse_preview(Source::Zypper, "gimp", &zyp).removes, vec!["gimp", "gimp-plugins"]);
        let prot = CmdOutput::failed(1, "Problem: The operation would result in removing the following protected packages: systemd\n");
        assert!(!parse_preview(Source::Dnf, "x", &prot).blockers.is_empty());
        let rpm = CmdOutput::failed(1, "error: Failed dependencies:\n\tgimp is needed by (installed) gimp-help-2.10\n");
        assert!(parse_preview(Source::Rpm, "gimp", &rpm).blockers[0].contains("gimp-help-2.10"));
        let xbps = CmdOutput::ok("gimp-2.10.36_1 remove x86_64 https://repo 0B\n");
        assert_eq!(parse_preview(Source::Xbps, "gimp", &xbps).removes, vec!["gimp"]);
        let apk = CmdOutput::ok("(1/2) Purging gimp (2.10.36-r0)\n(2/2) Purging babl (0.1-r0)\nOK\n");
        assert_eq!(parse_preview(Source::Apk, "gimp", &apk).removes, vec!["gimp", "babl"]);
        let emerge = CmdOutput::ok(" media-gfx/gimp\n    selected: media-gfx/gimp-2.10.36 \n   protected: none \n     omitted: none \n");
        assert_eq!(parse_preview(Source::Portage, "media-gfx/gimp", &emerge).removes, vec!["media-gfx/gimp-2.10.36"]);
        let eopkg = CmdOutput::ok("The following list of packages will be removed\nin the respective order to satisfy dependencies:\ngimp\n");
        assert_eq!(parse_preview(Source::Eopkg, "gimp", &eopkg).removes, vec!["gimp"]);
    }

    #[test]
    fn dnf_without_root_falls_back_to_rpm_test() {
        let (_d, env) = home();
        let path = "/usr/share/applications/gimp.desktop";
        let a = app("gimp", path, "Exec=/nonexistent/gimp");
        let mock = Mock::with(&["rpm", "dnf"])
            .reply(&format!("rpm -qf --qf %{{NAME}}\\n {path}"), CmdOutput::ok("gimp\n"))
            .reply("dnf remove --assumeno gimp", CmdOutput::failed(1, "Error: This command has to be run with superuser privileges"))
            .reply("rpm -e --test gimp", CmdOutput::failed(1, "error: Failed dependencies:\n\tgimp is needed by (installed) gimp-help-2.10\n"));
        let p = plan(&a, &env, &mock);
        assert_eq!(p.command.clone().unwrap().join(" "), "pkexec dnf remove -y gimp");
        assert_eq!(p.also_removes, vec!["gimp-help-2.10".to_string()]);
        assert!(p.preview_note.is_none());
    }

    fn mkdirs(env: &Env, dirs: &[&str]) {
        for d in dirs {
            std::fs::create_dir_all(env.home.join(d)).unwrap();
        }
    }

    #[test]
    fn leftovers_match_exact_names_only() {
        let (_d, env) = home();
        mkdirs(&env, &[".config/Code", ".config/codeblocks", ".config/Code - OSS", ".cache/code", ".local/share/code-server", ".local/state/code", ".config/mandara", ".config/autostart", "Documents/code"]);
        let a = app("code", "/usr/share/applications/code.desktop", "Exec=/usr/bin/code %F");
        let owner = Owner::new(Source::Pacman, "code", false);
        std::fs::write(env.home.join(".config/Code/settings.json"), b"{}").unwrap();
        let found: Vec<String> = scan_leftovers(&a, &owner, &env).iter().map(|l| l.path.strip_prefix(&env.home).unwrap().display().to_string()).collect();
        assert_eq!(found, vec![".cache/code", ".config/Code", ".local/state/code"]);
        assert_eq!(scan_leftovers(&a, &owner, &env).iter().find(|l| l.path.ends_with("Code")).unwrap().bytes, 2);
        // `Code - OSS` only when it is the app's own window class.
        let oss = app("code-oss", "/usr/share/applications/code-oss.desktop", "Exec=/usr/bin/code-oss\nStartupWMClass=Code - OSS");
        let found = scan_leftovers(&oss, &Owner::new(Source::Pacman, "code-oss", false), &env);
        assert!(found.iter().any(|l| l.path.ends_with("Code - OSS")));
        assert!(!found.iter().any(|l| l.path.ends_with("codeblocks") || l.path.ends_with("Code")));
    }

    #[test]
    fn leftovers_never_leave_home_or_touch_shared_folders() {
        let (_d, mut env) = home();
        mkdirs(&env, &[".config/systemd", ".config/gtk-3.0", ".var/app/org.gimp.GIMP", ".config/GIMP", "snap/spotify"]);
        // A config home outside $HOME is never scanned.
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("GIMP")).unwrap();
        env.config_home = outside.path().to_path_buf();
        let f = app("org.gimp.GIMP", "/var/lib/flatpak/exports/share/applications/org.gimp.GIMP.desktop", "Exec=flatpak run org.gimp.GIMP\nX-Flatpak=org.gimp.GIMP\nStartupWMClass=gimp");
        let owner = Owner::new(Source::Flatpak, "org.gimp.GIMP", false);
        let found = scan_leftovers(&f, &owner, &env);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, LeftoverKind::Sandbox);
        // Generic programs and shared names never become names.
        let s = app("systemd-thing", "/usr/share/applications/x.desktop", "Exec=env FOO=1 bash -c systemd\nStartupWMClass=gtk-3.0");
        assert_eq!(residual_names(&s, &Owner::new(Source::Pacman, "systemd", false)), vec!["systemd-thing".to_string()]);
        let snap = scan_leftovers(&app("spotify_spotify", "/var/lib/snapd/desktop/applications/spotify_spotify.desktop", "Exec=/snap/bin/spotify"), &Owner::new(Source::Snap, "spotify", false), &env);
        assert!(snap.iter().any(|l| l.path.ends_with("snap/spotify")));
        // Reverse-DNS: the last part names leftovers when long enough.
        assert!(residual_names(&app("org.gnome.Nautilus", "/x", "Exec=nautilus"), &Owner::new(Source::Unknown, "", false)).contains(&"nautilus".to_string()));
        assert!(!residual_names(&app("org.kde.kate", "/x", "Exec=kate"), &Owner::new(Source::Unknown, "", false)).contains(&"org".to_string()));
    }

    #[test]
    fn trash_follows_the_spec_and_never_deletes() {
        let (_d, env) = home();
        let target = env.home.join(".config/My App");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("a.conf"), b"x").unwrap();
        let to = trash(&target, &env).unwrap();
        assert!(!target.exists() && to.join("a.conf").exists());
        assert_eq!(to, env.data_home.join("Trash/files/My App"));
        let info = std::fs::read_to_string(env.data_home.join("Trash/info/My App.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath="));
        assert!(info.contains(&format!("Path={}/.config/My%20App\n", percent_encode(&env.home))));
        assert!(info.lines().any(|l| l.starts_with("DeletionDate=") && l.len() == "DeletionDate=2026-10-28T12:00:00".len()));
        // Same name again gets a fresh slot.
        std::fs::create_dir_all(&target).unwrap();
        assert_eq!(trash(&target, &env).unwrap(), env.data_home.join("Trash/files/My App.2"));
        // $HOME, relative paths and `..` are refused.
        assert!(trash(&env.home, &env).is_err());
        assert!(trash(Path::new("relative"), &env).is_err());
        assert!(trash(&env.home.join(".config/../.config"), &env).is_err());
    }

    #[test]
    fn purge_trashes_only_planned_leftovers() {
        let (_d, env) = home();
        mkdirs(&env, &[".config/my-tool", ".config/other"]);
        let entry = env.data_home.join("applications/my-tool.desktop");
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, "[Desktop Entry]\nExec=my-tool\n").unwrap();
        let a = AppRef::parse("my-tool", &entry, "[Desktop Entry]\nExec=my-tool\n");
        let mock = Mock::default();
        let p = plan(&a, &env, &mock);
        assert_eq!(p.owner.source, Source::Local);
        assert!(p.command.is_none());
        let leftover = env.home.join(".config/my-tool");
        assert_eq!(p.leftovers.iter().map(|l| l.path.clone()).collect::<Vec<_>>(), vec![leftover.clone()]);
        // A path outside the plan is refused.
        assert!(execute(&p, &Choices { purge: vec![env.home.join(".config/other")], ..Default::default() }, &env, &mock).is_err());
        // Dry run: nothing moves.
        let out = execute(&p, &Choices { purge: vec![leftover.clone()], dry_run: true, ..Default::default() }, &env, &mock).unwrap();
        assert_eq!(out.would_trash, vec![entry.clone(), leftover.clone()]);
        assert!(entry.exists() && leftover.exists());
        // Real: the entry and the chosen leftover go to the Trash.
        let out = execute(&p, &Choices { purge: vec![leftover.clone()], ..Default::default() }, &env, &mock).unwrap();
        assert_eq!(out.trashed.len(), 2);
        assert!(!entry.exists() && !leftover.exists() && env.home.join(".config/other").exists());
        assert!(mock.calls().is_empty());
    }
}
