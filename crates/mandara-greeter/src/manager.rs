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
/// Timestamped switch backups (one folder each, with a [`MANIFEST`]).
pub const BACKUP_DIR: &str = "/var/lib/mandara-greeter/backups";
pub const MANIFEST: &str = "manifest.toml";
/// `MANDARA_GREETER_DRY_RUN=1` forces `use` / `restore` (and Studio's
/// "Use" button) to only print the plan.
pub const DRY_RUN_ENV: &str = "MANDARA_GREETER_DRY_RUN";
/// greetd's default greeter user when its config names none.
pub const DEFAULT_GREETER_USER: &str = "greeter";
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
/// Shown in every plan and result: when the change becomes visible.
pub const TAKES_EFFECT: &str = "Takes effect at the next boot (or the next time the login screen starts); the running session is not touched.";

/// A filesystem root: `/` for the real system, a fixture tree in tests.
#[derive(Clone, Debug)]
pub struct SysRoot {
    base: PathBuf,
    /// Also search `$PATH` for binaries (the real system only).
    use_path_env: bool,
}

impl SysRoot {
    pub fn system() -> SysRoot {
        SysRoot { base: PathBuf::from("/"), use_path_env: true }
    }

    pub fn at(base: impl Into<PathBuf>) -> SysRoot {
        SysRoot { base: base.into(), use_path_env: false }
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
        UNIT_DIRS.iter().any(|d| self.path(d).join(unit).exists())
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

/// The display manager behind `<root>/etc/systemd/system/display-manager.service`.
pub fn display_manager(root: &SysRoot) -> Option<String> {
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
                out.push(miss(GREETD, &login_package(family, GREETD)));
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
    let cmd_line = format!("command = {}", toml_string(command));
    let Some(text) = existing.filter(|t| !t.trim().is_empty()) else {
        return format!("{FRESH_CONFIG_HEADER}\n\n[terminal]\nvt = {FRESH_CONFIG_VT}\n\n[default_session]\n{cmd_line}\nuser = {}\n", toml_string(default_user));
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
    vec!["systemctl".into(), verb.into(), unit.into()]
}

/// The command that undoes `cmd` (`enable` ↔ `disable`).
fn inverse(cmd: &[String]) -> Option<Vec<String>> {
    let verb = match cmd.get(1)?.as_str() {
        "enable" => "disable",
        "disable" => "enable",
        _ => return None,
    };
    Some(systemctl(verb, cmd.get(2)?))
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
                    let contents = set_default_session_command(previous.as_deref(), &command, &greeter_user);
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
                        commands.push(systemctl("disable", &unit_of(dm)));
                    }
                    commands.push(systemctl("enable", GREETD_UNIT));
                }
            }
            Kind::DisplayManager => {
                let unit = target.units.iter().find(|u| root.has_unit(u)).or(target.units.first()).map(|u| u.to_string());
                already_active = current.as_deref().and_then(manager_for_unit).is_some_and(|m| m.id == target.id);
                if !already_active {
                    if let Some(dm) = &current {
                        commands.push(systemctl("disable", &unit_of(dm)));
                    }
                    if let Some(u) = &unit {
                        commands.push(systemctl("enable", u));
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
            files: Vec::new(),
        };
        let mut backup = Backup::create(root, manifest, &self.writes)?;
        let mut log = vec![format!("backed up to {}", backup.logical_dir())];
        let mut done: Vec<Vec<String>> = Vec::new();
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
                    done.push(c.clone());
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            let mut notes = Vec::new();
            for c in done.iter().rev() {
                if let Some(undo) = inverse(c)
                    && let Err(err) = runner.run(&undo)
                {
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
    #[serde(default)]
    pub files: Vec<BackedUpFile>,
}

pub(crate) struct Backup {
    pub(crate) id: String,
    dir: PathBuf,
    pub(crate) manifest: Manifest,
}

impl Backup {
    pub(crate) fn create(root: &SysRoot, mut manifest: Manifest, writes: &[FileWrite]) -> Result<Backup, String> {
        let base = root.path(BACKUP_DIR);
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

    fn save(&self) -> Result<(), String> {
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
    let mut done = Vec::new();
    for f in &manifest.files {
        let p = root.path(&f.path);
        match &f.copy {
            Some(copy) => {
                let text = std::fs::read_to_string(dir.join(copy)).map_err(|e| format!("reading the backup of {}: {e}", f.path))?;
                write_atomic(&p, &text)?;
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
    let mut ids: Vec<String> = std::fs::read_dir(root.path(BACKUP_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join(MANIFEST).is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    ids.sort();
    ids
}

/// `mandara greeter restore <id|latest>`.
#[derive(Debug)]
pub struct RestorePlan {
    pub id: String,
    dir: PathBuf,
    pub manifest: Manifest,
    pub commands: Vec<Vec<String>>,
}

impl RestorePlan {
    pub fn build(root: &SysRoot, which: &str) -> Result<RestorePlan, String> {
        let all = backups(root);
        let id = if which == "latest" { all.last().cloned() } else { all.iter().find(|i| *i == which).cloned() };
        let Some(id) = id else {
            return Err(if all.is_empty() { format!("no switch backups in {BACKUP_DIR}") } else { format!("no backup {which:?}; available: {}", all.join(", ")) });
        };
        let dir = root.path(BACKUP_DIR).join(&id);
        let text = std::fs::read_to_string(dir.join(MANIFEST)).map_err(|e| format!("{}: {e}", dir.display()))?;
        let manifest: Manifest = toml::from_str(&text).map_err(|e| format!("backup {id}: invalid manifest: {e}"))?;
        let mut commands = Vec::new();
        if manifest.units_switched && manifest.previous_unit != manifest.new_unit {
            if let Some(u) = &manifest.new_unit {
                commands.push(systemctl("disable", u));
            }
            if let Some(u) = &manifest.previous_unit {
                commands.push(systemctl("enable", u));
            }
        }
        Ok(RestorePlan { id, dir, manifest, commands })
    }

    pub fn describe(&self) -> String {
        let mut s = format!("Login screen — restore plan (backup {}, made {})\n\n", self.id, self.manifest.created);
        if self.manifest.target == crate::keyring_pam::BACKUP_TARGET {
            let _ = writeln!(s, "  Undoes a `greeter keyring-unlock` change.");
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
        if !self.commands.is_empty() {
            let _ = writeln!(s, "\n  Services (run with --apply; nothing is started or stopped):");
            for c in &self.commands {
                let _ = writeln!(s, "    {}", c.join(" "));
            }
        }
        let _ = writeln!(s, "\n  {TAKES_EFFECT}");
        s
    }

    pub fn execute(&self, root: &SysRoot, apply: bool, runner: &mut dyn Runner) -> Result<String, String> {
        if !crate::install::is_root() {
            return Err("not running as root: re-run with sudo to restore (nothing was changed)".into());
        }
        self.perform(root, apply, runner)
    }

    pub(crate) fn perform(&self, root: &SysRoot, apply: bool, runner: &mut dyn Runner) -> Result<String, String> {
        let mut log = restore_files(root, &self.dir, &self.manifest)?;
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
            let _ = writeln!(s, "\n{TAKES_EFFECT}");
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
