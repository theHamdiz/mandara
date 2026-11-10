//! Login-manager detection and switching (`mandara greeter list`, `use`,
//! `restore`).
//!
//! * [`detect`] reads which display manager `display-manager.service`
//!   points to, which greeter greetd's `[default_session] command` runs,
//!   and which alternatives are installed (binaries, systemd units).
//! * [`SwitchPlan`] is the switch to one of [`MANAGERS`]: for a greetd
//!   greeter only the `command` line of `/etc/greetd/config.toml` changes
//!   (comments, `user` and other tables are kept); for another display
//!   manager `display-manager.service` moves (disable the current one,
//!   enable the chosen one). Nothing is ever started or stopped, so the
//!   running session is untouched and the change shows at the next boot.
//! * Every switch first saves a timestamped backup under [`BACKUP_DIR`]
//!   (the files it changes plus a manifest); a failed step rolls back, and
//!   [`RestorePlan`] (`mandara greeter restore latest`) undoes a switch.
//!
//! On FreeBSD ([`SysRoot::freebsd`]) there is no systemd: the enabled
//! login manager is the one whose `<name>_enable` is `YES` in rc.conf, its
//! rc script lives in `/usr/local/etc/rc.d` or `/etc/rc.d`, and switching
//! is `sysrc -s <old> <old>_enable=NO` + `sysrc -s <new> <new>_enable=YES`
//! (`-s` makes sysrc change the setting in a service's own
//! `rc.conf.d/<name>` too, so the effective value changes). Undoing puts
//! back exactly the value each variable had before, in the file it was
//! in, or removes a variable no rc file set. Unit names (`sddm.service`)
//! stay the plan's and the backups' vocabulary; only the commands differ.
//!
//! Everything takes a [`SysRoot`] so tests run against fixture trees.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::install::{GREETD_CONFIG, INSTALLED_EXE, Kiosk, install_runtime, under};
pub use mandara_install::Family;
use mandara_install::family::OsRelease;
use mandara_install::packages::login_package;
use mandara_install::{PackageManager, Privilege};

/// systemd's alias for the enabled display manager.
pub const DISPLAY_MANAGER_LINK: &str = "/etc/systemd/system/display-manager.service";
/// Where systemd units are looked up.
pub const UNIT_DIRS: [&str; 4] = ["/etc/systemd/system", "/usr/lib/systemd/system", "/lib/systemd/system", "/usr/local/lib/systemd/system"];
/// Where greeter binaries are looked up (plus `$PATH` on the real system).
pub const BIN_DIRS: [&str; 6] = ["/usr/local/sbin", "/usr/local/bin", "/usr/sbin", "/usr/bin", "/sbin", "/bin"];
/// Timestamped switch backups (one folder each, with a [`MANIFEST`]): only
/// root may change them, outside the greeter's state folder, whose owner could otherwise
/// swap in a manifest that `restore` then carries out as root.
pub const BACKUP_DIR: &str = "/var/lib/mandara-greeter-backups";
/// Where backups were kept before, inside the greeter-owned state folder.
/// Restore still finds them, but only folders and manifests that root (or
/// the user looking, for a dry run) owns and nobody else may write.
pub const LEGACY_BACKUP_DIR: &str = "/var/lib/mandara-greeter/backups";
/// Readable like the system files the backups copy, so a dry-run restore
/// shows its plan without sudo; only root writes.
const BACKUP_DIR_MODE: u32 = 0o755;
pub const MANIFEST: &str = "manifest.toml";
/// `MANDARA_GREETER_DRY_RUN=1` forces `use` / `restore` (and Studio's
/// "Use" button) to only print the plan.
pub const DRY_RUN_ENV: &str = "MANDARA_GREETER_DRY_RUN";
/// greetd's default greeter user when its config names none.
pub const DEFAULT_GREETER_USER: &str = "greeter";
/// How to build greetd where it isn't packaged (FreeBSD).
pub const FREEBSD_GREETD_DOC: &str = "https://github.com/theHamdiz/mandara/blob/main/docs/login-manager.md#freebsd";
/// Id of Mandara Login in [`MANAGERS`]: the default choice everywhere.
pub const MANDARA: &str = "mandara";
const GREETD: &str = "greetd";
const GREETD_UNIT: &str = "greetd.service";
const CAGE: &str = "cage";
const UNIT_SUFFIX: &str = ".service";
/// Read size when comparing the installed greeter with this binary.
const COMPARE_CHUNK: usize = 64 * 1024;
/// Marks a plan that only refreshes Mandara Login's installed copy (the
/// installer looks for it).
pub const OUTDATED: &str = "out of date";
const BACKUP_ID_FORMAT: &str = "%Y%m%d-%H%M%S";
/// Header comment of a greetd config written from scratch.
const FRESH_CONFIG_HEADER: &str = "# Written by `mandara greeter use`.";
/// VT greetd runs on in a config written from scratch.
const FRESH_CONFIG_VT: i64 = 1;
/// FreeBSD: `/dev/ttyv8`, past the eight gettys of `/etc/ttys`.
const FREEBSD_FRESH_CONFIG_VT: i64 = 8;
/// Where FreeBSD's rc scripts live (ports, then base).
pub const RC_DIRS: [&str; 2] = ["/usr/local/etc/rc.d", "/etc/rc.d"];
/// rc.conf files, in the order rc.subr reads them (later wins); a
/// service's own `rc.conf.d/<name>` comes last.
pub const RC_CONF_FILES: [&str; 3] = ["/etc/defaults/rc.conf", "/etc/rc.conf", "/etc/rc.conf.local"];
pub const RC_CONF_DIRS: [&str; 2] = ["/etc/rc.conf.d", "/usr/local/etc/rc.conf.d"];
/// Shown in every plan and result: when the change becomes visible.
pub const TAKES_EFFECT: &str = "Takes effect at the next boot (or the next time the login screen starts); the running session is not touched.";
/// … for a polkit rule, which polkit notices by itself.
pub const POLKIT_TAKES_EFFECT: &str = "Takes effect right away; polkit reads its rules again by itself.";

/// A filesystem root: `/` for the real system, a fixture tree in tests.
#[derive(Clone, Debug)]
pub struct SysRoot {
    base: PathBuf,
    /// Also search `$PATH` for binaries (the real system only).
    use_path_env: bool,
    /// A FreeBSD system: rc.conf services, OpenPAM, `/usr/local` ports.
    freebsd: bool,
}

impl SysRoot {
    pub fn system() -> SysRoot {
        SysRoot { base: PathBuf::from("/"), use_path_env: true, freebsd: cfg!(target_os = "freebsd") }
    }

    pub fn at(base: impl Into<PathBuf>) -> SysRoot {
        SysRoot { base: base.into(), use_path_env: false, freebsd: false }
    }

    /// The same root, as a FreeBSD system (tests on any platform).
    pub fn as_freebsd(mut self) -> SysRoot {
        self.freebsd = true;
        self
    }

    /// Whether services are FreeBSD rc scripts (no systemd).
    pub fn freebsd(&self) -> bool {
        self.freebsd
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

    /// `abs` (an absolute system path) inside this root.
    pub fn path(&self, abs: &str) -> PathBuf {
        under(&self.base, abs)
    }

    pub fn has_binary(&self, name: &str) -> bool {
        if BIN_DIRS.iter().any(|d| self.path(d).join(name).is_file()) {
            return true;
        }
        self.use_path_env && std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
    }

    pub fn has_unit(&self, unit: &str) -> bool {
        if self.freebsd {
            return RC_DIRS.iter().any(|d| self.path(d).join(rc_name(unit)).is_file());
        }
        UNIT_DIRS.iter().any(|d| self.path(d).join(unit).exists())
    }

    /// Whether rc.conf turns `service` on (`<service>_enable="YES"`).
    pub fn rc_enabled(&self, service: &str) -> bool {
        self.rc_last(service, true).is_some_and(|v| rc_yes(&v))
    }

    /// The value `<service>_enable` has in the rc files sysrc edits (not
    /// `/etc/defaults/rc.conf`): None when none of them sets it.
    pub fn rc_setting(&self, service: &str) -> Option<String> {
        self.rc_last(service, false)
    }

    /// The last value rc.subr's `load_rc_config <service>` gives
    /// `<service>_enable`, optionally counting the defaults file.
    fn rc_last(&self, service: &str, defaults: bool) -> Option<String> {
        let var = rc_var(service);
        let skip = usize::from(!defaults);
        self.rc_files(service).iter().skip(skip).rev().find_map(|f| self.read(f).and_then(|t| rc_value(&t, &var)))
    }

    /// The rc files `load_rc_config <service>` reads, in order (later
    /// wins): [`RC_CONF_FILES`], then the service's own `rc.conf.d/<name>`
    /// in each of [`RC_CONF_DIRS`], a file or a folder of files.
    fn rc_files(&self, service: &str) -> Vec<String> {
        let mut out: Vec<String> = RC_CONF_FILES.iter().map(|f| f.to_string()).collect();
        for d in RC_CONF_DIRS {
            let own = format!("{d}/{service}");
            let p = self.path(&own);
            if p.is_dir() {
                let mut names: Vec<String> = std::fs::read_dir(&p).into_iter().flatten().flatten().filter(|e| e.path().is_file()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with('.')).collect();
                names.sort();
                out.extend(names.into_iter().map(|n| format!("{own}/{n}")));
            } else {
                out.push(own);
            }
        }
        out
    }

    pub(crate) fn read(&self, abs: &str) -> Option<String> {
        std::fs::read_to_string(self.path(abs)).ok()
    }
}

/// How a login manager is started.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Kind {
    /// A greeter run by greetd (`[default_session] command`).
    GreetdGreeter,
    /// A display manager with its own systemd unit.
    DisplayManager,
}

/// Package names by distribution family (what `greeter use --install`
/// installs when it's missing; several are separated by spaces). Families
/// without their own column use the Arch names (Debian's on Debian, the
/// Fedora ones on openSUSE too).
#[derive(Copy, Clone, Debug)]
pub struct Packages {
    pub arch: &'static str,
    pub debian: &'static str,
    pub fedora: &'static str,
}

impl Packages {
    const fn same(name: &'static str) -> Packages {
        Packages { arch: name, debian: name, fedora: name }
    }

    pub fn for_family(&self, f: Family) -> &'static str {
        match f {
            Family::Debian => self.debian,
            Family::Fedora | Family::Suse => self.fedora,
            _ => self.arch,
        }
    }
}

/// A login screen Mandara knows how to switch to.
#[derive(Debug)]
pub struct LoginManager {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: Kind,
    /// Any of these binaries means it's installed (greetd greeters).
    pub binaries: &'static [&'static str],
    /// Its systemd units, preferred first (display managers).
    pub units: &'static [&'static str],
    /// greetd `default_session.command` (greetd greeters; Mandara's comes
    /// from its kiosk compositor instead).
    pub command: &'static str,
    pub packages: Packages,
    /// Studio icon name.
    pub icon: &'static str,
}

impl LoginManager {
    pub fn is_mandara(&self) -> bool {
        self.id == MANDARA
    }

    /// The greetd command this greeter runs with (`kiosk` is Mandara's).
    pub fn greetd_command(&self, kiosk: Kiosk) -> String {
        if self.is_mandara() { kiosk.command() } else { self.command.to_string() }
    }
}

/// Every login screen `mandara greeter use` can switch to. Mandara Login
/// comes first: it is the default and recommended choice.
pub const MANAGERS: &[LoginManager] = &[
    LoginManager { id: MANDARA, name: "Mandara Login", kind: Kind::GreetdGreeter, binaries: &[], units: &[GREETD_UNIT], command: "", packages: Packages::same("mandara"), icon: "lock" },
    LoginManager { id: "tuigreet", name: "tuigreet", kind: Kind::GreetdGreeter, binaries: &["tuigreet"], units: &[GREETD_UNIT], command: "tuigreet --time --remember --remember-session", packages: Packages { arch: "greetd-tuigreet", debian: "tuigreet", fedora: "tuigreet" }, icon: "terminal" },
    LoginManager { id: "regreet", name: "ReGreet", kind: Kind::GreetdGreeter, binaries: &["regreet"], units: &[GREETD_UNIT], command: "cage -s -- regreet", packages: Packages { arch: "greetd-regreet", debian: "regreet", fedora: "greetd-regreet" }, icon: "user" },
    LoginManager { id: "gtkgreet", name: "gtkgreet", kind: Kind::GreetdGreeter, binaries: &["gtkgreet"], units: &[GREETD_UNIT], command: "cage -s -- gtkgreet", packages: Packages { arch: "greetd-gtkgreet", debian: "gtkgreet", fedora: "gtkgreet" }, icon: "user" },
    LoginManager { id: "nwg-hello", name: "nwg-hello", kind: Kind::GreetdGreeter, binaries: &["nwg-hello"], units: &[GREETD_UNIT], command: "cage -s -- nwg-hello", packages: Packages::same("nwg-hello"), icon: "user" },
    LoginManager { id: "qtgreet", name: "QtGreet", kind: Kind::GreetdGreeter, binaries: &["qtgreet"], units: &[GREETD_UNIT], command: "cage -s -- qtgreet", packages: Packages { arch: "qtgreet", debian: "qtgreet", fedora: "qtgreet" }, icon: "user" },
    LoginManager { id: "agreety", name: "agreety", kind: Kind::GreetdGreeter, binaries: &["agreety"], units: &[GREETD_UNIT], command: "agreety --cmd /bin/sh", packages: Packages { arch: "greetd-agreety", debian: "greetd", fedora: "greetd" }, icon: "terminal" },
    LoginManager { id: "sddm", name: "SDDM", kind: Kind::DisplayManager, binaries: &["sddm"], units: &["sddm.service"], command: "", packages: Packages::same("sddm"), icon: "monitor" },
    LoginManager { id: "plasmalogin", name: "Plasma Login Manager", kind: Kind::DisplayManager, binaries: &["plasmalogin"], units: &["plasmalogin.service"], command: "", packages: Packages::same("plasma-login-manager"), icon: "monitor" },
    LoginManager { id: "gdm", name: "GDM", kind: Kind::DisplayManager, binaries: &["gdm", "gdm3"], units: &["gdm.service", "gdm3.service"], command: "", packages: Packages { arch: "gdm", debian: "gdm3", fedora: "gdm" }, icon: "monitor" },
    // LightDM shows nothing without a greeter of its own.
    LoginManager { id: "lightdm", name: "LightDM", kind: Kind::DisplayManager, binaries: &["lightdm"], units: &["lightdm.service"], command: "", packages: Packages { arch: "lightdm lightdm-gtk-greeter", debian: "lightdm lightdm-gtk-greeter", fedora: "lightdm lightdm-gtk" }, icon: "monitor" },
    LoginManager { id: "ly", name: "Ly", kind: Kind::DisplayManager, binaries: &["ly", "ly-dm"], units: &["ly.service"], command: "", packages: Packages::same("ly"), icon: "terminal" },
    LoginManager { id: "lxdm", name: "LXDM", kind: Kind::DisplayManager, binaries: &["lxdm"], units: &["lxdm.service"], command: "", packages: Packages::same("lxdm"), icon: "monitor" },
];

/// Looks a login manager up by id (case-insensitive; `mandara-login` and
/// `gdm3` are accepted too).
pub fn find(id: &str) -> Option<&'static LoginManager> {
    let id = id.trim().to_ascii_lowercase();
    let id = match id.as_str() {
        "mandara-login" | "mandara-greeter" => MANDARA,
        "gdm3" => "gdm",
        "ly-dm" => "ly",
        other => other,
    };
    MANAGERS.iter().find(|m| m.id == id)
}

/// The distribution family of `root` (`/etc/os-release`, else the package
/// manager installed there).
pub fn family(root: &SysRoot) -> Family {
    let os = OsRelease::parse(&root.read("/etc/os-release").or_else(|| root.read("/usr/lib/os-release")).unwrap_or_default());
    Family::detect(&os, |bin| root.has_binary(bin))
}

/// Something a login manager needs that isn't installed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Missing {
    /// What's missing (a program or a systemd unit).
    pub what: String,
    /// Package that provides it on this distribution.
    pub package: String,
}

impl Missing {
    pub fn describe(&self) -> String {
        if self.package.is_empty() {
            return format!("{} isn't installed, and this system has no package for it — build it first ({FREEBSD_GREETD_DOC})", self.what);
        }
        format!("{} isn't installed — install it first (package {})", self.what, self.package)
    }
}

/// One entry of [`Detection::options`].
#[derive(Clone, Debug, Serialize)]
pub struct Availability {
    pub id: &'static str,
    pub missing: Vec<Missing>,
}

impl Availability {
    pub fn available(&self) -> bool {
        self.missing.is_empty()
    }

    pub fn manager(&self) -> &'static LoginManager {
        find(self.id).unwrap_or(&MANAGERS[0])
    }
}

/// greetd's `/etc/greetd/config.toml`.
#[derive(Clone, Debug, Default, Serialize)]
pub struct GreetdConfig {
    pub exists: bool,
    pub command: Option<String>,
    pub user: Option<String>,
    /// Set when the file doesn't parse (switching refuses to touch it).
    pub error: Option<String>,
}

impl GreetdConfig {
    pub fn read(root: &SysRoot) -> GreetdConfig {
        let Some(text) = root.read(GREETD_CONFIG) else { return GreetdConfig::default() };
        match toml::from_str::<toml::Table>(&text) {
            Ok(t) => {
                let session = t.get("default_session");
                let get = |k: &str| session.and_then(|s| s.get(k)).and_then(|v| v.as_str()).map(str::to_string);
                GreetdConfig { exists: true, command: get("command"), user: get("user"), error: None }
            }
            Err(e) => GreetdConfig { exists: true, error: Some(e.to_string().trim().to_string()), ..GreetdConfig::default() },
        }
    }
}

/// What is set up now and what could be.
#[derive(Clone, Debug, Serialize)]
pub struct Detection {
    /// Unit behind `display-manager.service` without `.service` (`sddm`,
    /// `gdm3`, `greetd`…), if one is enabled.
    pub display_manager: Option<String>,
    pub greetd: GreetdConfig,
    /// Id (in [`MANAGERS`]) of the active login screen, when recognised.
    pub active: Option<&'static str>,
    /// Every entry of [`MANAGERS`], in order, with what it's missing.
    pub options: Vec<Availability>,
    #[serde(skip)]
    pub family: Family,
}

impl Detection {
    pub fn option(&self, id: &str) -> Option<&Availability> {
        self.options.iter().find(|o| o.id == id)
    }

    /// Human-readable name of what's active ("greetd · tuigreet", "SDDM",
    /// "greetd · custom command", "none").
    pub fn active_label(&self) -> String {
        match (self.display_manager.as_deref(), self.active.and_then(find)) {
            (Some(GREETD), Some(m)) => format!("greetd · {}", m.name),
            (Some(GREETD), None) => "greetd · custom greeter".into(),
            (_, Some(m)) => m.name.to_string(),
            (Some(dm), None) => dm.to_string(),
            (None, None) => "none".into(),
        }
    }
}

/// The display manager behind `<root>/etc/systemd/system/display-manager.service`
/// (FreeBSD: the first known login manager rc.conf enables).
pub fn display_manager(root: &SysRoot) -> Option<String> {
    if root.freebsd() {
        return std::iter::once(GREETD).chain(MANAGERS.iter().filter(|m| m.kind == Kind::DisplayManager).flat_map(|m| m.units.iter().map(|u| rc_name(u)))).find(|name| root.rc_enabled(name)).map(str::to_string);
    }
    let target = std::fs::read_link(root.path(DISPLAY_MANAGER_LINK)).ok()?;
    let name = target.file_name()?.to_string_lossy().trim_end_matches(UNIT_SUFFIX).to_string();
    (!name.is_empty()).then_some(name)
}

/// Which known greeter a greetd command runs. Wrappers (`cage -s --`,
/// `env`, a kiosk config path) are looked through; any mention of
/// `mandara` means Mandara Login.
pub fn greeter_in_command(command: &str) -> Option<&'static str> {
    if command.contains(MANDARA) {
        return Some(MANDARA);
    }
    command.split_whitespace().find_map(|tok| {
        let base = tok.rsplit('/').next().unwrap_or(tok);
        MANAGERS.iter().find(|m| m.kind == Kind::GreetdGreeter && m.binaries.contains(&base)).map(|m| m.id)
    })
}

/// The manager whose unit is `dm` (`gdm3` → GDM, `ly@tty2` → Ly).
fn manager_for_unit(dm: &str) -> Option<&'static LoginManager> {
    let stem = dm.split('@').next().unwrap_or(dm);
    MANAGERS.iter().find(|m| m.kind == Kind::DisplayManager && (m.id == stem || m.units.iter().any(|u| u.trim_end_matches(UNIT_SUFFIX) == stem)))
}

fn greetd_installed(root: &SysRoot) -> bool {
    root.has_unit(GREETD_UNIT) || root.has_binary(GREETD)
}

/// What `m` lacks on `root` (empty: ready to use). Mandara Login is this
/// binary, so it only needs greetd and its kiosk compositor.
pub fn missing_for(root: &SysRoot, m: &LoginManager, kiosk: Kiosk, family: Family) -> Vec<Missing> {
    let mut out = Vec::new();
    let miss = |what: &str, package: &str| Missing { what: what.to_string(), package: package.to_string() };
    match m.kind {
        Kind::GreetdGreeter => {
            if !greetd_installed(root) {
                // Not packaged for FreeBSD yet: no package to install.
                let package = if family == Family::FreeBSD { String::new() } else { login_package(family, GREETD) };
                out.push(miss(GREETD, &package));
            }
            if !m.is_mandara() && !m.binaries.iter().any(|b| root.has_binary(b)) {
                out.push(miss(m.binaries.first().copied().unwrap_or(m.id), m.packages.for_family(family)));
            }
            let command = m.greetd_command(kiosk);
            let wrapper = command.split_whitespace().next().unwrap_or_default();
            let kiosk_prog = if m.is_mandara() { kiosk.program() } else { wrapper };
            if (m.is_mandara() || wrapper == CAGE) && !root.has_binary(kiosk_prog) {
                out.push(miss(kiosk_prog, &login_package(family, kiosk_prog)));
            }
        }
        Kind::DisplayManager => {
            if !m.units.iter().any(|u| root.has_unit(u)) {
                out.push(miss(m.units.first().copied().unwrap_or(m.id), m.packages.for_family(family)));
            }
        }
    }
    out
}

/// The packages providing `missing`, each once, in order.
pub fn packages_for(missing: &[Missing]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in missing.iter().flat_map(|m| m.package.split_whitespace()) {
        if !out.iter().any(|p| p == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// argv installing `packages` with `root`'s package manager (the shared
/// [`PackageManager`]: the family's own when present, else the first one
/// installed); run it as root ([`as_root`]). `None` when nothing is to
/// install or no package manager is known.
pub fn install_command(root: &SysRoot, family: Family, packages: &[String]) -> Option<Vec<String>> {
    if packages.is_empty() {
        return None;
    }
    Some(PackageManager::detect(family, |bin| root.has_binary(bin))?.install_argv(packages))
}

/// `argv` run as root: unchanged when this process is root, else through
/// sudo / doas / run0 / pkexec (`None` when none is installed).
pub fn as_root(root: &SysRoot, argv: Vec<String>) -> Option<Vec<String>> {
    Privilege::detect(crate::install::is_root(), |bin| root.has_binary(bin)).wrap(argv)
}

/// Detects the active login manager and what else is installed under `root`.
pub fn detect(root: &SysRoot) -> Detection {
    let family = family(root);
    let display_manager = display_manager(root);
    let greetd = GreetdConfig::read(root);
    let active = match display_manager.as_deref() {
        Some(GREETD) => greetd.command.as_deref().and_then(greeter_in_command),
        Some(dm) => manager_for_unit(dm).map(|m| m.id),
        None => None,
    };
    let options = MANAGERS.iter().map(|m| Availability { id: m.id, missing: missing_for(root, m, Kiosk::Cage, family) }).collect();
    Detection { display_manager, greetd, active, options, family }
}

/// `greeter list`: the active login screen and every choice.
pub fn describe_list(d: &Detection) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "Login screen now: {}", d.active_label());
    if let Some(e) = &d.greetd.error {
        let _ = writeln!(s, "  warning: {GREETD_CONFIG} doesn't parse: {e}");
    }
    let _ = writeln!(s);
    for o in &d.options {
        let m = o.manager();
        let mark = if d.active == Some(m.id) { "●" } else { "○" };
        let kind = match m.kind {
            Kind::GreetdGreeter => "greetd greeter",
            Kind::DisplayManager => "display manager",
        };
        let mut notes = Vec::new();
        if m.is_mandara() {
            notes.push("recommended · default".to_string());
        }
        if d.active == Some(m.id) {
            notes.push("active".into());
        }
        notes.extend(o.missing.iter().map(Missing::describe));
        let _ = writeln!(s, "  {mark} {:<10} {:<14} {:<16} {}", m.id, m.name, kind, notes.join(" · "));
    }
    let _ = writeln!(s, "\nSwitch with: sudo mandara greeter use <id> --apply  (--dry-run shows the plan)");
    s
}

// ── greetd config editing ────────────────────────────────────────────────

/// A TOML basic string for `s`.
fn toml_string(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

/// The table a header line opens (`[default_session]` → `default_session`).
fn header_name(line: &str) -> Option<&str> {
    let t = line.trim();
    if !t.starts_with('[') || t.starts_with("[[") {
        return None;
    }
    let end = t.find(']')?;
    Some(t[1..end].trim())
}

/// The key a `key = value` line assigns.
fn line_key(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if t.starts_with('#') || t.starts_with('[') {
        return None;
    }
    let (k, _) = t.split_once('=')?;
    Some(k.trim().trim_matches('"'))
}

/// Sets `[default_session] command` in greetd's config text, keeping every
/// other line (comments, `user`, `[terminal]`, `initial_session`) as is.
/// Without a config, writes a fresh one (vt 1, `user = default_user`).
pub fn set_default_session_command(existing: Option<&str>, command: &str, default_user: &str) -> String {
    set_default_session_command_on(existing, command, default_user, FRESH_CONFIG_VT)
}

/// [`set_default_session_command`] with the VT a fresh config uses.
fn set_default_session_command_on(existing: Option<&str>, command: &str, default_user: &str, fresh_vt: i64) -> String {
    let cmd_line = format!("command = {}", toml_string(command));
    let Some(text) = existing.filter(|t| !t.trim().is_empty()) else {
        return format!("{FRESH_CONFIG_HEADER}\n\n[terminal]\nvt = {fresh_vt}\n\n[default_session]\n{cmd_line}\nuser = {}\n", toml_string(default_user));
    };
    let mut out: Vec<String> = Vec::new();
    let mut in_session = false;
    let mut header_at = None;
    let mut done = false;
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if let Some(name) = header_name(line) {
            in_session = name == "default_session";
            if in_session {
                header_at = Some(out.len());
            }
            out.push(line.to_string());
            continue;
        }
        if in_session && !done && line_key(line) == Some("command") {
            let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
            out.push(format!("{indent}{cmd_line}"));
            done = true;
            // Skip the rest of a multi-line string value.
            let value = line.split_once('=').map(|(_, v)| v.trim()).unwrap_or_default();
            for quote in ["\"\"\"", "'''"] {
                if value.starts_with(quote) && !value[quote.len()..].contains(quote) {
                    for rest in lines.by_ref() {
                        if rest.contains(quote) {
                            break;
                        }
                    }
                }
            }
            continue;
        }
        out.push(line.to_string());
    }
    if !done {
        match header_at {
            Some(i) => out.insert(i + 1, cmd_line),
            None => {
                if out.last().is_some_and(|l| !l.trim().is_empty()) {
                    out.push(String::new());
                }
                out.push("[default_session]".into());
                out.push(cmd_line);
                out.push(format!("user = {}", toml_string(default_user)));
            }
        }
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// Checks a rewritten greetd config: it parses, runs `command`, and every
/// other value is what it was before.
pub fn validate_greetd_rewrite(old: Option<&str>, new: &str, command: &str) -> Result<(), String> {
    let new_t: toml::Table = toml::from_str(new).map_err(|e| format!("the new {GREETD_CONFIG} doesn't parse: {e}"))?;
    let got = new_t.get("default_session").and_then(|s| s.get("command")).and_then(|v| v.as_str());
    if got != Some(command) {
        return Err(format!("the new {GREETD_CONFIG} doesn't set [default_session] command"));
    }
    if let Some(old) = old.filter(|t| !t.trim().is_empty()) {
        let mut old_t: toml::Table = toml::from_str(old).map_err(|e| format!("{GREETD_CONFIG} doesn't parse: {e}"))?;
        let mut expected = new_t.clone();
        for t in [&mut old_t, &mut expected] {
            if let Some(toml::Value::Table(s)) = t.get_mut("default_session") {
                s.remove("command");
                if s.is_empty() {
                    t.remove("default_session");
                }
            }
        }
        // A config without a [default_session] gains one with `user`.
        if !old_t.contains_key("default_session")
            && let Some(toml::Value::Table(s)) = expected.get_mut("default_session")
        {
            s.remove("user");
            if s.is_empty() {
                expected.remove("default_session");
            }
        }
        if old_t != expected {
            return Err(format!("rewriting {GREETD_CONFIG} would change more than [default_session] command; edit it by hand"));
        }
    }
    Ok(())
}

// ── Switching ────────────────────────────────────────────────────────────

/// Runs the service commands (`systemctl enable/disable …`).
pub trait Runner {
    fn run(&mut self, argv: &[String]) -> Result<(), String>;
}

/// Runs commands for real (argv only, no shell).
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&mut self, argv: &[String]) -> Result<(), String> {
        let (prog, args) = argv.split_first().ok_or("empty command")?;
        println!("$ {}", argv.join(" "));
        let status = std::process::Command::new(prog).args(args).status().map_err(|e| format!("{prog}: {e}"))?;
        if status.success() { Ok(()) } else { Err(format!("`{}` failed ({status})", argv.join(" "))) }
    }
}

fn systemctl(verb: &str, unit: &str) -> Vec<String> {
    vec![SYSTEMCTL.into(), verb.into(), unit.into()]
}

/// `unit`'s rc script name (`sddm.service` → `sddm`).
pub fn rc_name(unit: &str) -> &str {
    unit.trim_end_matches(UNIT_SUFFIX)
}

/// `<service>_enable`, the variable rc.subr checks.
fn rc_var(service: &str) -> String {
    format!("{}_enable", service.replace('-', "_"))
}

/// `sysrc -s <name> <name>_enable=<value>`. With `-s`, sysrc also searches
/// the service's `rc.conf.d/<name>` overrides and changes the last
/// assignment rc.subr reads (appending to `/etc/rc.conf` when there is
/// none), so the service's effective setting is the one written.
fn sysrc(unit: &str, value: &str) -> Vec<String> {
    let name = rc_name(unit);
    vec![SYSRC.into(), "-s".into(), name.into(), format!("{}={value}", rc_var(name))]
}

/// Enables or disables `unit` at boot on `root` (nothing is started or
/// stopped).
pub(crate) fn service_command(root: &SysRoot, verb: &str, unit: &str) -> Vec<String> {
    if root.freebsd() { sysrc(unit, if verb == "enable" { RC_YES } else { RC_NO }) } else { systemctl(verb, unit) }
}

/// The command that undoes [`service_command`]`(root, verb, unit)`, read
/// from `root` before that runs: systemd flips `enable` / `disable`; on
/// FreeBSD the variable gets back the value it had (sysrc writes it to the
/// same last file), or is removed again when no rc file set it.
pub(crate) fn undo_command(root: &SysRoot, verb: &str, unit: &str) -> Vec<String> {
    if !root.freebsd() {
        return systemctl(if verb == "enable" { "disable" } else { "enable" }, unit);
    }
    let name = rc_name(unit);
    match root.rc_setting(name) {
        Some(value) => sysrc(unit, &value),
        None => vec![SYSRC.into(), "-s".into(), name.into(), "-x".into(), rc_var(name)],
    }
}

const SYSRC: &str = "sysrc";
const SYSTEMCTL: &str = "systemctl";

const RC_YES: &str = "YES";
const RC_NO: &str = "NO";

/// The last value `var` gets in rc.conf-style `text` (`var="YES"`,
/// comments and quotes dropped).
pub fn rc_value(text: &str, var: &str) -> Option<String> {
    text.lines()
        .rev()
        .find_map(|l| {
            let l = l.trim();
            let (k, v) = l.split_once('=')?;
            if k.trim() != var || l.starts_with('#') {
                return None;
            }
            let v = v.trim();
            let v = match v.chars().next() {
                Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or_default(),
                _ => v.split(['#', ' ', '\t', ';']).next().unwrap_or_default(),
            };
            Some(v.to_string())
        })
}

/// rc.subr's `checkyesno`: `YES`, `TRUE`, `ON` or `1`, any case.
pub fn rc_yes(value: &str) -> bool {
    matches!(value.trim().to_ascii_uppercase().as_str(), "YES" | "TRUE" | "ON" | "1")
}

/// The unit name for display manager `dm` (`sddm` → `sddm.service`).
fn unit_of(dm: &str) -> String {
    if dm.ends_with(UNIT_SUFFIX) { dm.to_string() } else { format!("{dm}{UNIT_SUFFIX}") }
}

/// A config file the switch writes.
#[derive(Clone, Debug)]
pub struct FileWrite {
    /// Absolute system path.
    pub path: String,
    pub contents: String,
    /// The current text (None: the file doesn't exist yet).
    pub previous: Option<String>,
}

/// The switch to one login manager, built from what is on disk.
#[derive(Clone, Debug)]
pub struct SwitchPlan {
    pub target: &'static LoginManager,
    pub kiosk: Kiosk,
    pub exe: PathBuf,
    pub detection: Detection,
    pub greeter_user: String,
    pub writes: Vec<FileWrite>,
    /// Put Mandara Login's binary, media folders and state dir in place.
    pub install_mandara: bool,
    /// The unit `display-manager.service` points to afterwards.
    pub new_unit: Option<String>,
    /// `systemctl` commands (run with `--apply` only).
    pub commands: Vec<Vec<String>>,
    /// `undo[i]` reverses `commands[i]` (worked out before they run).
    pub undo: Vec<Vec<String>>,
    pub already_active: bool,
    /// Validation failures: the plan refuses to run while any are set.
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl SwitchPlan {
    /// Plans switching `root` to the login manager `id`.
    pub fn build(root: &SysRoot, id: &str, kiosk: Kiosk, exe: PathBuf) -> Result<SwitchPlan, String> {
        let target = find(id).ok_or_else(|| {
            let ids: Vec<&str> = MANAGERS.iter().map(|m| m.id).collect();
            format!("unknown login manager {id:?} (one of: {})", ids.join(", "))
        })?;
        let detection = detect(root);
        let mut errors: Vec<String> = missing_for(root, target, kiosk, detection.family).iter().map(Missing::describe).collect();
        let mut warnings = Vec::new();
        let mut writes = Vec::new();
        let mut commands = Vec::new();
        let mut undo = Vec::new();
        let mut service = |verb: &str, unit: &str| {
            commands.push(service_command(root, verb, unit));
            undo.push(undo_command(root, verb, unit));
        };
        let current = detection.display_manager.clone();
        let greeter_user = detection.greetd.user.clone().unwrap_or_else(|| DEFAULT_GREETER_USER.into());
        let new_unit;
        let already_active;
        match target.kind {
            Kind::GreetdGreeter => {
                let command = target.greetd_command(kiosk);
                let same_command = detection.greetd.command.as_deref() == Some(command.as_str());
                // Another greeter keeps its own flags when it's already the one running.
                let same_greeter = !target.is_mandara() && detection.active == Some(target.id);
                already_active = current.as_deref() == Some(GREETD) && (same_command || same_greeter);
                new_unit = Some(GREETD_UNIT.to_string());
                if let Some(e) = &detection.greetd.error {
                    errors.push(format!("{GREETD_CONFIG} doesn't parse ({e}); fix it first"));
                } else if !already_active {
                    let previous = root.read(GREETD_CONFIG);
                    let vt = if root.freebsd() { FREEBSD_FRESH_CONFIG_VT } else { FRESH_CONFIG_VT };
                    let contents = set_default_session_command_on(previous.as_deref(), &command, &greeter_user, vt);
                    if let Err(e) = validate_greetd_rewrite(previous.as_deref(), &contents, &command) {
                        errors.push(e);
                    }
                    writes.push(FileWrite { path: GREETD_CONFIG.into(), contents, previous });
                    if target.is_mandara()
                        && let Some((path, contents)) = kiosk.config_file()
                    {
                        writes.push(FileWrite { path: path.into(), contents, previous: root.read(path) });
                    }
                }
                if target.is_mandara() && crate::install::passwd_ids_in(root.base(), &greeter_user).is_none() {
                    warnings.push(format!("user `{greeter_user}` does not exist yet (the greetd package normally creates it)."));
                }
                if current.as_deref() != Some(GREETD) {
                    if let Some(dm) = &current {
                        service("disable", &unit_of(dm));
                    }
                    service("enable", GREETD_UNIT);
                }
            }
            Kind::DisplayManager => {
                let unit = target.units.iter().find(|u| root.has_unit(u)).or(target.units.first()).map(|u| u.to_string());
                already_active = current.as_deref().and_then(manager_for_unit).is_some_and(|m| m.id == target.id);
                if !already_active {
                    if let Some(dm) = &current {
                        service("disable", &unit_of(dm));
                    }
                    if let Some(u) = &unit {
                        service("enable", u);
                    }
                }
                new_unit = unit;
            }
        }
        if let Some(dm) = current.as_deref()
            && dm != GREETD
            && manager_for_unit(dm).is_none()
        {
            warnings.push(format!("display-manager.service points to `{dm}`, which isn't a login manager Mandara knows; check before applying."));
        }
        let exe_path = exe.clone();
        Ok(SwitchPlan {
            target,
            kiosk,
            exe,
            detection,
            greeter_user,
            // An upgrade refreshes an outdated installed copy even when
            // Mandara Login is already the login screen.
            install_mandara: target.is_mandara() && (!already_active || !same_contents(&exe_path, &root.path(INSTALLED_EXE))),
            writes,
            new_unit,
            commands: if already_active { Vec::new() } else { commands },
            undo: if already_active { Vec::new() } else { undo },
            already_active,
            errors,
            warnings,
        })
    }

    pub fn describe(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "Login screen — switch plan\n");
        let _ = writeln!(s, "  Now:    {}", self.detection.active_label());
        let what = match self.target.kind {
            Kind::GreetdGreeter => "greetd greeter",
            Kind::DisplayManager => "display manager",
        };
        let _ = writeln!(s, "  Target: {} ({what})", self.target.name);
        if self.already_active && self.install_mandara {
            let _ = writeln!(s, "\n  {} is already the login screen; its installed copy is {OUTDATED}:", self.target.name);
            let _ = writeln!(s, "    install  {INSTALLED_EXE} (copy of {})", self.exe.display());
        } else if self.already_active {
            let _ = writeln!(s, "\n  {} is already the login screen; nothing to change.", self.target.name);
        } else {
            if self.install_mandara || !self.writes.is_empty() {
                let _ = writeln!(s, "\n  Files (each one backed up first to {BACKUP_DIR}/<timestamp>/):");
            }
            if self.install_mandara {
                let _ = writeln!(s, "    install  {INSTALLED_EXE} (copy of {}), media folders, state dir", self.exe.display());
            }
            for w in &self.writes {
                let verb = if w.previous.is_some() { "update" } else { "create" };
                let _ = writeln!(s, "    {verb:<8} {}", w.path);
                if w.path == GREETD_CONFIG {
                    if let Some(old) = w.previous.as_deref().and_then(|p| toml::from_str::<toml::Table>(p).ok()).and_then(|t| Some(t.get("default_session")?.get("command")?.as_str()?.to_string())) {
                        let _ = writeln!(s, "        - command = {}", toml_string(&old));
                    }
                    let new = self.target.greetd_command(self.kiosk);
                    let _ = writeln!(s, "        + command = {}", toml_string(&new));
                    if w.previous.is_some() {
                        let _ = writeln!(s, "        (every other line is kept; user = {:?})", self.greeter_user);
                    }
                }
            }
            if !self.commands.is_empty() {
                let _ = writeln!(s, "\n  Services (run with --apply; nothing is started or stopped):");
                for c in &self.commands {
                    let _ = writeln!(s, "    {}", c.join(" "));
                }
            }
            let _ = writeln!(s, "\n  {TAKES_EFFECT}");
            let _ = writeln!(s, "  Undo: sudo mandara greeter restore latest");
        }
        for e in &self.errors {
            let _ = writeln!(s, "\n  error: {e}");
        }
        for w in &self.warnings {
            let _ = writeln!(s, "\n  warning: {w}");
        }
        s
    }

    /// Performs the plan as root: backup, files, then (with `apply`) the
    /// service commands. Any failure rolls back what was done.
    pub fn execute(&self, root: &SysRoot, apply: bool, runner: &mut dyn Runner) -> Result<String, String> {
        if !self.errors.is_empty() {
            return Err(format!("{}\n(nothing was changed)", self.errors.join("\n")));
        }
        if !crate::install::is_root() {
            return Err("not running as root: re-run with sudo (or from Studio) to switch (nothing was changed)".into());
        }
        self.perform(root, apply, runner)
    }

    /// [`Self::execute`] without the root check (tests use fixture roots).
    pub(crate) fn perform(&self, root: &SysRoot, apply: bool, runner: &mut dyn Runner) -> Result<String, String> {
        if !self.errors.is_empty() {
            return Err(format!("{}\n(nothing was changed)", self.errors.join("\n")));
        }
        if self.already_active {
            if self.install_mandara {
                let log = install_runtime(root.base(), &self.exe, &self.greeter_user)?;
                return Ok(format!("{}\n{} is already the login screen; its installed copy was refreshed.", log.iter().map(|l| format!("✓ {l}")).collect::<Vec<_>>().join("\n"), self.target.name));
            }
            return Ok(format!("{} is already the login screen; nothing changed.", self.target.name));
        }
        let manifest = Manifest {
            created: chrono::Local::now().to_rfc3339(),
            target: self.target.id.to_string(),
            previous_unit: self.detection.display_manager.as_deref().map(unit_of),
            new_unit: self.new_unit.clone(),
            units_switched: false,
            undo: self.undo.iter().rev().cloned().collect(),
            files: Vec::new(),
            left_groups: Vec::new(),
        };
        let mut backup = Backup::create(root, manifest, &self.writes)?;
        let mut log = vec![format!("backed up to {}", backup.logical_dir())];
        // How many service commands ran (rollback undoes those).
        let mut done = 0;
        let result = (|| -> Result<(), String> {
            if self.install_mandara {
                log.extend(install_runtime(root.base(), &self.exe, &self.greeter_user)?);
            }
            for w in &self.writes {
                let p = root.path(&w.path);
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
                }
                write_atomic(&p, &w.contents)?;
                log.push(format!("wrote {}", w.path));
            }
            if apply {
                for c in &self.commands {
                    runner.run(c)?;
                    done += 1;
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            let mut notes = Vec::new();
            for undo in self.undo[..done].iter().rev() {
                if let Err(err) = runner.run(undo) {
                    notes.push(err);
                }
            }
            if let Err(err) = backup.restore_files(root) {
                notes.push(err);
            }
            let tail = if notes.is_empty() { "everything was rolled back".to_string() } else { format!("rollback problems: {}", notes.join("; ")) };
            return Err(format!("{e}\n({tail})"));
        }
        if apply && !self.commands.is_empty() {
            backup.manifest.units_switched = true;
            backup.save()?;
        }
        let mut s: String = log.iter().map(|l| format!("✓ {l}\n")).collect();
        if !apply && !self.commands.is_empty() {
            let _ = writeln!(s, "\nFiles are in place. To switch at the next boot run:");
            for c in &self.commands {
                let _ = writeln!(s, "    sudo {}", c.join(" "));
            }
            let _ = writeln!(s, "(or re-run with --apply)");
        } else {
            let _ = writeln!(s, "\n{} is now the login screen. {TAKES_EFFECT}", self.target.name);
        }
        let _ = write!(s, "Undo: sudo mandara greeter restore {}", backup.id);
        Ok(s)
    }
}

/// Byte-for-byte equal files (false when either can't be read).
fn same_contents(a: &Path, b: &Path) -> bool {
    use std::io::Read as _;
    let (Ok(fa), Ok(fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else { return false };
    if fa.metadata().map(|m| m.len()).ok() != fb.metadata().map(|m| m.len()).ok() {
        return false;
    }
    let (mut ra, mut rb) = (std::io::BufReader::new(fa), std::io::BufReader::new(fb));
    let (mut ba, mut bb) = ([0u8; COMPARE_CHUNK], [0u8; COMPARE_CHUNK]);
    loop {
        let (Ok(na), Ok(nb)) = (ra.read(&mut ba), rb.read(&mut bb)) else { return false };
        if na != nb || ba[..na] != bb[..nb] {
            return false;
        }
        if na == 0 {
            return true;
        }
    }
}

pub(crate) fn write_atomic(path: &Path, contents: &str) -> Result<(), String> {
    let tmp = path.with_extension("mandara-new");
    std::fs::write(&tmp, contents).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

// ── Backups ──────────────────────────────────────────────────────────────

/// One file in a backup.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackedUpFile {
    /// Absolute system path.
    pub path: String,
    /// Copy inside the backup folder; None: the file didn't exist.
    pub copy: Option<String>,
}

/// `manifest.toml` of a switch backup.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub created: String,
    pub target: String,
    /// `display-manager.service` before the switch.
    pub previous_unit: Option<String>,
    /// … and after it.
    pub new_unit: Option<String>,
    /// The service commands ran (`--apply`), so restore reverses them.
    pub units_switched: bool,
    /// The commands that reverse them, in order (`sysrc` puts back each
    /// rc.conf value exactly). Older backups have none: restore then
    /// derives them from the units.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub undo: Vec<Vec<String>>,
    #[serde(default)]
    pub files: Vec<BackedUpFile>,
    /// Group memberships the change took away (`greeter power-rule`
    /// taking the login screen's user out of `operator`). Restore never
    /// adds them back; it shows the command instead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left_groups: Vec<LeftGroup>,
}

/// A user the change took out of a group.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeftGroup {
    pub user: String,
    pub group: String,
}

pub(crate) struct Backup {
    pub(crate) id: String,
    dir: PathBuf,
    pub(crate) manifest: Manifest,
}

impl Backup {
    pub(crate) fn create(root: &SysRoot, mut manifest: Manifest, writes: &[FileWrite]) -> Result<Backup, String> {
        let base = root.path(BACKUP_DIR);
        if let Some(parent) = base.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        {
            use std::os::unix::fs::DirBuilderExt as _;
            match std::fs::DirBuilder::new().mode(BACKUP_DIR_MODE).create(&base) {
                Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(format!("{}: {e}", base.display())),
                _ => {}
            }
        }
        let stamp = chrono::Local::now().format(BACKUP_ID_FORMAT).to_string();
        let mut id = stamp.clone();
        let mut n = 1;
        while base.join(&id).exists() {
            n += 1;
            id = format!("{stamp}-{n}");
        }
        let dir = base.join(&id);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for w in writes {
            let copy = match &w.previous {
                Some(text) => {
                    let name = w.path.trim_start_matches('/').replace('/', "__");
                    std::fs::write(dir.join(&name), text).map_err(|e| format!("backing up {}: {e}", w.path))?;
                    Some(name)
                }
                None => None,
            };
            manifest.files.push(BackedUpFile { path: w.path.clone(), copy });
        }
        let b = Backup { id, dir, manifest };
        b.save()?;
        Ok(b)
    }

    pub(crate) fn save(&self) -> Result<(), String> {
        let text = toml::to_string(&self.manifest).map_err(|e| e.to_string())?;
        std::fs::write(self.dir.join(MANIFEST), text).map_err(|e| format!("writing the backup manifest: {e}"))
    }

    pub(crate) fn logical_dir(&self) -> String {
        format!("{BACKUP_DIR}/{}", self.id)
    }

    /// Puts every backed-up file back (and removes files that didn't exist).
    pub(crate) fn restore_files(&self, root: &SysRoot) -> Result<Vec<String>, String> {
        restore_files(root, &self.dir, &self.manifest)
    }
}

fn restore_files(root: &SysRoot, dir: &Path, manifest: &Manifest) -> Result<Vec<String>, String> {
    let mut copies = Vec::with_capacity(manifest.files.len());
    for f in &manifest.files {
        copies.push(match &f.copy {
            Some(copy) => Some(std::fs::read_to_string(dir.join(copy)).map_err(|e| format!("reading the backup of {}: {e}", f.path))?),
            None => None,
        });
    }
    put_back(root, manifest, &copies)
}

/// Writes `copies` (one per manifest file, `None` where the file didn't
/// exist) back in place.
fn put_back(root: &SysRoot, manifest: &Manifest, copies: &[Option<String>]) -> Result<Vec<String>, String> {
    let mut done = Vec::new();
    for (f, copy) in manifest.files.iter().zip(copies) {
        let p = root.path(&f.path);
        match copy {
            Some(text) => {
                write_atomic(&p, text)?;
                done.push(format!("restored {}", f.path));
            }
            None => {
                if p.exists() {
                    std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", f.path))?;
                    done.push(format!("removed {} (it didn't exist before)", f.path));
                }
            }
        }
    }
    Ok(done)
}

/// Backup ids under `root`, oldest first.
pub fn backups(root: &SysRoot) -> Vec<String> {
    backup_dirs(root).into_iter().map(|(id, _)| id).collect()
}

/// Backups under `root` and their folders, oldest first: those in
/// [`BACKUP_DIR`], then trusted ones left in [`LEGACY_BACKUP_DIR`].
fn backup_dirs(root: &SysRoot) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    for (base, legacy) in [(root.path(BACKUP_DIR), false), (root.path(LEGACY_BACKUP_DIR), true)] {
        if legacy && !owned_by_us(&base) {
            continue;
        }
        for entry in std::fs::read_dir(&base).into_iter().flatten().flatten() {
            let (dir, id) = (entry.path(), entry.file_name().to_string_lossy().into_owned());
            let manifest = dir.join(MANIFEST);
            let usable = if legacy { owned_by_us(&dir) && owned_by_us(&manifest) && manifest.is_file() } else { manifest.is_file() };
            if usable && !found.iter().any(|(known, _)| *known == id) {
                found.push((id, dir));
            }
        }
    }
    found.sort();
    found
}

/// Whether `path` itself (not a link's target) belongs to root or to the
/// user running this (root, for a real restore), and only its owner may
/// write it.
fn owned_by_us(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    // SAFETY: geteuid has no preconditions.
    let me = unsafe { libc::geteuid() };
    std::fs::symlink_metadata(path).is_ok_and(|m| !m.file_type().is_symlink() && (m.uid() == 0 || m.uid() == me) && m.mode() & 0o022 == 0)
}

/// A unit name a switch can record: `<name>.service` in systemd's unit
/// name alphabet, no path and no leading `-` (never read as an option).
fn plain_unit(unit: &str) -> bool {
    unit.strip_suffix(UNIT_SUFFIX).is_some_and(|stem| !stem.is_empty() && !stem.starts_with('-') && stem.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | ':')))
}

/// A value an rc variable can be put back to (`YES`, `NO`, `onestart`…).
fn plain_rc_value(value: &str) -> bool {
    value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Whether `c` is an undo command a switch between `units` writes on
/// `root`: [`undo_command`]'s shape for one of those units, with the
/// service tool of `root`'s system. Anything else (another program, a
/// unit file path, another rc variable) is refused.
fn expected_undo(root: &SysRoot, c: &[String], units: &[&str]) -> bool {
    let ours = |name: &str| units.iter().any(|u| rc_name(u) == name);
    match c {
        [tool, verb, unit] if !root.freebsd() && tool == SYSTEMCTL => matches!(verb.as_str(), "enable" | "disable") && units.contains(&unit.as_str()),
        [tool, s, name, set] if root.freebsd() && tool == SYSRC && s == "-s" && ours(name) => set.split_once('=').is_some_and(|(var, value)| var == rc_var(name) && plain_rc_value(value)),
        [tool, s, name, x, var] if root.freebsd() && tool == SYSRC && s == "-s" && x == "-x" && ours(name) => *var == rc_var(name),
        _ => false,
    }
}

/// Opens `name` inside the already opened folder `dir` (or the path
/// `name` itself without one) without following a link, and checks what
/// was opened, not the path: a folder or a single-link file, belonging to
/// root or to us, that nobody else may write. Every later read goes
/// through the handle, so swapping the path afterwards changes nothing.
fn open_trusted(dir: Option<&std::fs::File>, name: &Path, folder: bool) -> Result<std::fs::File, String> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::MetadataExt as _;
    let c = std::ffi::CString::new(name.as_os_str().as_bytes()).map_err(|_| format!("{}: not a plain name", name.display()))?;
    let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | if folder { libc::O_DIRECTORY } else { libc::O_NONBLOCK };
    // SAFETY: `c` is a valid NUL-terminated string and the folder's
    // descriptor (if any) stays open for the call; a returned descriptor
    // is owned by the File below and nothing else.
    let fd = unsafe { libc::openat(dir.map_or(libc::AT_FDCWD, |d| d.as_raw_fd()), c.as_ptr(), flags) };
    if fd < 0 {
        return Err(format!("{}: {}", name.display(), std::io::Error::last_os_error()));
    }
    // SAFETY: `fd` was just opened above and is owned by nothing else.
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let m = file.metadata().map_err(|e| format!("{}: {e}", name.display()))?;
    // SAFETY: geteuid has no preconditions.
    let me = unsafe { libc::geteuid() };
    let kind = if folder { m.is_dir() } else { m.is_file() && m.nlink() == 1 };
    if !kind || !(m.uid() == 0 || m.uid() == me) || m.mode() & 0o022 != 0 {
        return Err(format!("{}: not a backup Mandara made", name.display()));
    }
    Ok(file)
}

/// A backup's manifest and copies, read whole through verified handles
/// (see [`open_trusted`]) from the folder `base`/`id`.
fn read_backup(base: &Path, id: &str) -> Result<(Manifest, Vec<Option<String>>), String> {
    use std::io::Read as _;
    let read = |dir: &std::fs::File, name: &str| -> Result<String, String> {
        // One plain name inside the backup, never a path out of it.
        if name.is_empty() || name.starts_with('.') || name.contains('/') {
            return Err(format!("backup {id}: unexpected file name {name:?}"));
        }
        let mut text = String::new();
        open_trusted(Some(dir), Path::new(name), false)?.read_to_string(&mut text).map_err(|e| format!("backup {id}: {name}: {e}"))?;
        Ok(text)
    };
    let dir = open_trusted(Some(&open_trusted(None, base, true)?), Path::new(id), true)?;
    let manifest: Manifest = toml::from_str(&read(&dir, MANIFEST)?).map_err(|e| format!("backup {id}: invalid manifest: {e}"))?;
    let mut copies = Vec::with_capacity(manifest.files.len());
    for f in &manifest.files {
        copies.push(match &f.copy {
            Some(copy) => Some(read(&dir, copy)?),
            None => None,
        });
    }
    Ok((manifest, copies))
}

/// `mandara greeter restore <id|latest>`.
#[derive(Debug)]
pub struct RestorePlan {
    pub id: String,
    pub manifest: Manifest,
    /// The backed-up files' contents, one per manifest file (`None`: it
    /// didn't exist), read when the plan was made.
    copies: Vec<Option<String>>,
    pub commands: Vec<Vec<String>>,
}

impl RestorePlan {
    pub fn build(root: &SysRoot, which: &str) -> Result<RestorePlan, String> {
        let all = backup_dirs(root);
        let found = if which == "latest" { all.last() } else { all.iter().find(|(i, _)| i == which) };
        let Some((id, dir)) = found.cloned() else {
            let ids: Vec<&str> = all.iter().map(|(i, _)| i.as_str()).collect();
            return Err(if all.is_empty() { format!("no switch backups in {BACKUP_DIR}") } else { format!("no backup {which:?}; available: {}", ids.join(", ")) });
        };
        // Read whole now, through checked handles: the old folder sits in
        // the greeter user's own state folder, and nothing it swaps in
        // after this point is ever read.
        let base = dir.parent().ok_or_else(|| format!("backup {id}: no folder"))?;
        let (manifest, copies) = read_backup(base, &id)?;
        let mut commands = Vec::new();
        // The units the switch moved between, as a switch records them.
        let recorded = [&manifest.previous_unit, &manifest.new_unit];
        if manifest.units_switched
            && let Some(u) = recorded.into_iter().flatten().find(|u| !plain_unit(u))
        {
            return Err(format!("backup {id}: unexpected unit `{u}`"));
        }
        let units: Vec<&str> = recorded.into_iter().flatten().map(String::as_str).collect();
        if manifest.units_switched && !manifest.undo.is_empty() {
            // Only the service commands that switch writes, never another
            // program, unit or rc variable.
            if let Some(c) = manifest.undo.iter().find(|c| !expected_undo(root, c, &units)) {
                return Err(format!("backup {id}: unexpected undo command `{}`", c.join(" ")));
            }
            commands.clone_from(&manifest.undo);
        } else if manifest.units_switched && manifest.previous_unit != manifest.new_unit {
            if let Some(u) = &manifest.new_unit {
                commands.push(service_command(root, "disable", u));
            }
            if let Some(u) = &manifest.previous_unit {
                commands.push(service_command(root, "enable", u));
            }
        }
        Ok(RestorePlan { id, manifest, copies, commands })
    }

    pub fn describe(&self) -> String {
        let mut s = format!("Login screen — restore plan (backup {}, made {})\n\n", self.id, self.manifest.created);
        if self.manifest.target == crate::keyring_pam::BACKUP_TARGET {
            let _ = writeln!(s, "  Undoes a `greeter keyring-unlock` change.");
        } else if self.manifest.target == crate::power_rule::BACKUP_TARGET {
            let _ = writeln!(s, "  Undoes a `greeter power-rule` change.");
        } else {
            let _ = writeln!(s, "  Undoes the switch to {}.", find(&self.manifest.target).map_or(self.manifest.target.as_str(), |m| m.name));
        }
        for f in &self.manifest.files {
            match f.copy {
                Some(_) => {
                    let _ = writeln!(s, "    restore  {}", f.path);
                }
                None => {
                    let _ = writeln!(s, "    remove   {} (it didn't exist before)", f.path);
                }
            }
        }
        for g in &self.manifest.left_groups {
            let _ = writeln!(s, "    keeps {} out of {} (that group can read raw disks; by hand: pw groupmod {} -m {})", g.user, g.group, g.group, g.user);
        }
        if !self.commands.is_empty() {
            let _ = writeln!(s, "\n  Services (run with --apply; nothing is started or stopped):");
            for c in &self.commands {
                let _ = writeln!(s, "    {}", c.join(" "));
            }
        }
        let _ = writeln!(s, "\n  {}", self.takes_effect());
        s
    }

    /// polkit reads a changed rule right away; everything else waits for
    /// the login screen's next start.
    fn takes_effect(&self) -> &'static str {
        if self.manifest.target == crate::power_rule::BACKUP_TARGET { POLKIT_TAKES_EFFECT } else { TAKES_EFFECT }
    }

    pub fn execute(&self, root: &SysRoot, apply: bool, runner: &mut dyn Runner) -> Result<String, String> {
        if !crate::install::is_root() {
            return Err("not running as root: re-run with sudo to restore (nothing was changed)".into());
        }
        self.perform(root, apply, runner)
    }

    pub(crate) fn perform(&self, root: &SysRoot, apply: bool, runner: &mut dyn Runner) -> Result<String, String> {
        let mut log = put_back(root, &self.manifest, &self.copies)?;
        if apply {
            for c in &self.commands {
                runner.run(c)?;
            }
            if !self.commands.is_empty() {
                log.push("display manager switched back".into());
            }
        }
        let mut s: String = log.iter().map(|l| format!("✓ {l}\n")).collect();
        if !apply && !self.commands.is_empty() {
            let _ = writeln!(s, "\nTo switch the display manager back at the next boot run:");
            for c in &self.commands {
                let _ = writeln!(s, "    sudo {}", c.join(" "));
            }
            let _ = writeln!(s, "(or re-run with --apply)");
        } else {
            let _ = writeln!(s, "\n{}", self.takes_effect());
        }
        Ok(s)
    }
}

/// True when [`DRY_RUN_ENV`] forces plans-only.
pub fn forced_dry_run() -> bool {
    std::env::var(DRY_RUN_ENV).is_ok_and(|v| !v.is_empty() && v != "0")
}

#[cfg(test)]
mod tests;
