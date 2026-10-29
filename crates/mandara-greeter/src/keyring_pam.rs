//! Unlock GNOME Keyring and KWallet when you log in through Mandara Login
//! (`mandara greeter keyring-unlock [--apply|--remove] [--dry-run]`).
//!
//! Neither keyring has a D-Bus call that takes a password, so the only way
//! to open them without their own dialog is PAM: at login,
//! `pam_gnome_keyring.so` / `pam_kwallet5.so` keep the password you typed
//! (auth) and unlock the keyring with it when the session opens (session).
//! This module adds those lines to greetd's PAM service file:
//!
//! * only for modules that are installed (searched in [`MODULE_DIRS`]) and
//!   not already run by that stack (includes / substacks are followed, so a
//!   distribution that already wires them in `common-auth` is left alone);
//! * the auth line right after the stack that checks the password (the
//!   last `include` / `substack` / `@include` whose stack runs `pam_unix`
//!   or a similar module), the session line after the last session line;
//! * each group wrapped in `# mandara-keyring begin` / `end` markers, so
//!   `--remove` takes out exactly those lines and nothing else;
//! * lines start with `-` (skip silently if the module is uninstalled later).
//!
//! When `/etc/pam.d/<service>` doesn't exist but the distribution ships
//! `/usr/lib/pam.d/<service>` (openSUSE), a copy is created in `/etc/pam.d`
//! (which takes precedence) and `--remove` deletes it again.
//!
//! Every write is planned first, needs root, is backed up under
//! [`crate::manager::BACKUP_DIR`] (so `greeter restore latest` undoes it
//! too), verified after writing and rolled back on any failure. Everything
//! takes a [`SysRoot`] so tests run against fixture trees.
//!
//! Only password logins unlock keyrings: autologin has no password to hand
//! over, and each keyring's password must equal the login password.

use std::fmt::Write as _;

use serde::Serialize;

use crate::install::GREETD_CONFIG;
use crate::manager::{BACKUP_DIR, Backup, Family, FileWrite, Manifest, SysRoot, write_atomic};

/// Where PAM service files are looked up, highest precedence first.
pub const PAM_DIRS: [&str; 2] = ["/etc/pam.d", "/usr/lib/pam.d"];
/// The directory Mandara writes to (never the vendor one).
pub const ETC_PAM_DIR: &str = "/etc/pam.d";
/// The distribution's directory (openSUSE ships service files there).
pub const VENDOR_PAM_DIR: &str = "/usr/lib/pam.d";
/// PAM module directories across distributions and architectures.
pub const MODULE_DIRS: [&str; 10] = [
    "/usr/lib/security",
    "/usr/lib64/security",
    "/lib/security",
    "/lib64/security",
    "/lib/x86_64-linux-gnu/security",
    "/usr/lib/x86_64-linux-gnu/security",
    "/lib/aarch64-linux-gnu/security",
    "/usr/lib/aarch64-linux-gnu/security",
    "/lib/i386-linux-gnu/security",
    "/usr/lib/i386-linux-gnu/security",
];
/// greetd's PAM service unless its config names another one.
pub const DEFAULT_SERVICE: &str = "greetd";
/// First line of a block Mandara added.
pub const BEGIN: &str = "# mandara-keyring begin";
/// Last line of a block Mandara added.
pub const END: &str = "# mandara-keyring end";
/// Backup manifest target for this change (`greeter restore` says so).
pub const BACKUP_TARGET: &str = "keyring-unlock";
/// Last line of every plan: the installer and Studio look for these.
pub const STATUS_UP_TO_DATE: &str = "Status: up to date";
pub const STATUS_CHANGES: &str = "Status: changes needed";
pub const STATUS_NOTHING: &str = "Status: no keyring PAM module is installed";
/// Includes followed at most this deep (PAM itself refuses deeper ones).
const MAX_INCLUDE_DEPTH: usize = 8;
/// Modules that check the password (the auth line goes after their stack).
const PASSWORD_MODULES: &[&str] = &["pam_unix.so", "pam_unix2.so", "pam_sss.so", "pam_systemd_home.so", "pam_ldap.so", "pam_krb5.so", "pam_winbind.so"];
/// Column widths of the lines Mandara writes (`-auth      optional     …`).
const TYPE_COLUMN: usize = 10;
const CONTROL_COLUMN: usize = 12;
/// Control of every line Mandara writes: a keyring never blocks a login.
const CONTROL: &str = "optional";
const AUTH: &str = "auth";
const SESSION: &str = "session";
const PAM_HEADER: &str = "#%PAM";

/// A keyring PAM can unlock at login.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Keyring {
    Gnome,
    KWallet,
}

impl Keyring {
    pub const ALL: [Keyring; 2] = [Keyring::Gnome, Keyring::KWallet];

    pub fn name(self) -> &'static str {
        match self {
            Keyring::Gnome => "GNOME Keyring",
            Keyring::KWallet => "KWallet",
        }
    }

    /// Module file names, preferred first. KWallet's module keeps the
    /// `pam_kwallet5.so` name for KDE Frameworks 6 (kwalletd6 / ksecretd);
    /// `pam_kwallet.so` is the KDE 4 one some distributions still carry.
    pub fn modules(self) -> &'static [&'static str] {
        match self {
            Keyring::Gnome => &["pam_gnome_keyring.so"],
            Keyring::KWallet => &["pam_kwallet5.so", "pam_kwallet6.so", "pam_kwallet.so"],
        }
    }

    /// Arguments of the auth line. pam_kwallet skips logins it doesn't
    /// see as graphical (`XDG_SESSION_TYPE` is set only after greetd's
    /// authentication), so it needs `force_run`.
    fn auth_args(self) -> &'static str {
        match self {
            Keyring::Gnome => "",
            Keyring::KWallet => "force_run",
        }
    }

    /// Arguments of the session line (`auto_start` starts the daemon).
    fn session_args(self) -> &'static str {
        match self {
            Keyring::Gnome => "auto_start",
            Keyring::KWallet => "auto_start force_run",
        }
    }

    /// The keyring's own daemon (installed without its PAM module → hint).
    fn daemons(self) -> &'static [&'static str] {
        match self {
            Keyring::Gnome => &["gnome-keyring-daemon"],
            Keyring::KWallet => &["kwalletd6", "kwalletd5", "ksecretd"],
        }
    }

    /// The package that provides the PAM module.
    pub fn package(self, family: Family) -> &'static str {
        match (self, family) {
            (Keyring::Gnome, Family::Arch) => "gnome-keyring",
            (Keyring::Gnome, Family::Debian) => "libpam-gnome-keyring",
            (Keyring::Gnome, Family::Fedora | Family::Suse) => "gnome-keyring-pam",
            (Keyring::Gnome, Family::Gentoo) => "gnome-base/gnome-keyring (USE=pam)",
            (Keyring::Gnome, _) => "gnome-keyring / libpam-gnome-keyring / gnome-keyring-pam",
            (Keyring::KWallet, Family::Arch) => "kwallet-pam",
            (Keyring::KWallet, Family::Debian) => "libpam-kwallet5",
            (Keyring::KWallet, Family::Fedora) => "pam-kwallet",
            (Keyring::KWallet, Family::Gentoo) => "kde-plasma/kwallet-pam",
            (Keyring::KWallet, _) => "kwallet-pam / libpam-kwallet5 / pam-kwallet / pam_kwallet6",
        }
    }
}

/// Whether one stage (auth or session) of the login runs a keyring module.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Hook {
    Missing,
    /// In a `# mandara-keyring` block.
    Mandara,
    /// Already wired by the distribution or by hand (the file it's in).
    Elsewhere(String),
}

impl Hook {
    pub fn present(&self) -> bool {
        !matches!(self, Hook::Missing)
    }
}

/// One keyring's PAM state for the greetd service.
#[derive(Clone, Debug, Serialize)]
pub struct ModuleStatus {
    pub keyring: Keyring,
    /// Installed module file name, if any.
    pub module: Option<String>,
    /// Where it is installed (absolute system path).
    pub module_path: Option<String>,
    /// The keyring daemon itself is installed.
    pub daemon: bool,
    pub auth: Hook,
    pub session: Hook,
    /// Package hint when the module is missing.
    pub package: &'static str,
}

impl ModuleStatus {
    pub fn installed(&self) -> bool {
        self.module.is_some()
    }

    /// Both stages run the module: a password login unlocks it.
    pub fn unlocks_at_login(&self) -> bool {
        self.auth.present() && self.session.present()
    }

    /// Mandara's lines do (part of) the work.
    pub fn by_mandara(&self) -> bool {
        self.auth == Hook::Mandara || self.session == Hook::Mandara
    }
}

/// Add the lines or take them out.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Add,
    Remove,
}

// ── PAM file parsing ─────────────────────────────────────────────────────

/// One rule of a PAM service file.
#[derive(Clone, Debug)]
struct Rule {
    /// `auth` / `account` / `password` / `session` (leading `-` dropped);
    /// None for Debian's `@include`.
    ty: Option<String>,
    control: String,
    /// Module path, or the included service's name.
    target: String,
}

impl Rule {
    fn parse(line: &str) -> Option<Rule> {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            return None;
        }
        if let Some(rest) = t.strip_prefix("@include") {
            return Some(Rule { ty: None, control: "@include".into(), target: rest.trim().to_string() });
        }
        let mut words = t.split_whitespace();
        let ty = words.next()?.trim_start_matches('-').to_ascii_lowercase();
        let mut control = words.next()?.to_string();
        // `[success=1 default=ignore]` spans several words.
        if control.starts_with('[') {
            while !control.ends_with(']') {
                let Some(w) = words.next() else { break };
                control.push(' ');
                control.push_str(w);
            }
        }
        let target = words.next()?.to_string();
        Some(Rule { ty: Some(ty), control, target })
    }

    fn is_include(&self) -> bool {
        self.ty.is_none() || matches!(self.control.as_str(), "include" | "substack")
    }

    fn module(&self) -> Option<&str> {
        (!self.is_include()).then(|| self.target.rsplit('/').next().unwrap_or(&self.target))
    }

    /// A rule of type `ty` (an `@include` may hold any type).
    fn may_be(&self, ty: &str) -> bool {
        self.ty.as_deref().is_none_or(|t| t == ty)
    }
}

/// A module one stage runs, and the file the line is in.
#[derive(Clone, Debug)]
struct ModuleRef {
    module: String,
    file: String,
    control: String,
}

/// An included service file: (absolute system path, text).
fn resolve(root: &SysRoot, name: &str) -> Option<(String, String)> {
    if name.starts_with('/') {
        return root.read(name).map(|t| (name.to_string(), t));
    }
    PAM_DIRS.iter().find_map(|d| {
        let path = format!("{d}/{name}");
        root.read(&path).map(|t| (path, t))
    })
}

/// Every module the `ty` stage of `text` runs, following includes.
/// `skip(line)` hides lines of the top file (Mandara's own blocks).
fn collect(root: &SysRoot, text: &str, file: &str, ty: &str, depth: usize, skip: &dyn Fn(usize) -> bool, out: &mut Vec<ModuleRef>) {
    for (i, line) in text.lines().enumerate() {
        if skip(i) {
            continue;
        }
        let Some(rule) = Rule::parse(line) else { continue };
        if !rule.may_be(ty) {
            continue;
        }
        match rule.module() {
            Some(m) => out.push(ModuleRef { module: m.to_string(), file: file.to_string(), control: rule.control.clone() }),
            None => {
                if depth < MAX_INCLUDE_DEPTH
                    && let Some((path, inc)) = resolve(root, &rule.target)
                {
                    collect(root, &inc, &path, ty, depth + 1, &|_| false, out);
                }
            }
        }
    }
}

/// Line ranges (inclusive) of Mandara's blocks; Err on unbalanced markers.
fn blocks(text: &str) -> Result<Vec<(usize, usize)>, String> {
    let mut out = Vec::new();
    let mut open = None;
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.starts_with(BEGIN) {
            if open.is_some() {
                return Err(format!("line {}: a second `{BEGIN}` before `{END}`", i + 1));
            }
            open = Some(i);
        } else if t.starts_with(END) {
            let Some(start) = open.take() else { return Err(format!("line {}: `{END}` without `{BEGIN}`", i + 1)) };
            out.push((start, i));
        }
    }
    if let Some(start) = open {
        return Err(format!("line {}: `{BEGIN}` is never closed", start + 1));
    }
    Ok(out)
}

fn in_blocks(ranges: &[(usize, usize)], i: usize) -> bool {
    ranges.iter().any(|&(a, b)| (a..=b).contains(&i))
}

/// `text` without Mandara's blocks (every other byte kept).
fn strip_blocks(text: &str, ranges: &[(usize, usize)]) -> String {
    let mut out: String = text.lines().enumerate().filter(|(i, _)| !in_blocks(ranges, *i)).map(|(_, l)| format!("{l}\n")).collect();
    if !text.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}

/// The rule lines inside Mandara's blocks.
fn block_rules(text: &str) -> Vec<String> {
    let Ok(ranges) = blocks(text) else { return Vec::new() };
    text.lines().enumerate().filter(|(i, l)| in_blocks(&ranges, *i) && Rule::parse(l).is_some()).map(|(_, l)| l.trim().to_string()).collect()
}

/// A rule line as Mandara writes it.
fn rule_line(ty: &str, module: &str, args: &str) -> String {
    let ty = format!("-{ty}");
    format!("{ty:<TYPE_COLUMN$} {CONTROL:<CONTROL_COLUMN$} {module} {args}").trim_end().to_string()
}

fn block(note: &str, rules: &[String]) -> Vec<String> {
    let mut v = vec![format!("{BEGIN} — {note}")];
    v.extend(rules.iter().cloned());
    v.push(END.to_string());
    v
}

/// Does `rule` (an include) lead to a stage `ty` that runs a password module?
fn runs_password_module(root: &SysRoot, rule: &Rule, ty: &str) -> bool {
    match rule.module() {
        Some(m) => PASSWORD_MODULES.contains(&m),
        None => resolve(root, &rule.target).is_some_and(|(path, inc)| {
            let mut refs = Vec::new();
            collect(root, &inc, &path, ty, 1, &|_| false, &mut refs);
            refs.iter().any(|r| PASSWORD_MODULES.contains(&r.module.as_str()))
        }),
    }
}

/// Does the line run anything of stage `ty` (an `@include` counts when
/// the included file has such lines)?
fn has_stage(root: &SysRoot, rule: &Rule, ty: &str) -> bool {
    match &rule.ty {
        Some(t) => t == ty,
        None => resolve(root, &rule.target).is_some_and(|(_, inc)| inc.lines().filter_map(Rule::parse).any(|r| r.may_be(ty))),
    }
}

/// Where the auth block goes (after this line index) and the rule there.
fn auth_anchor(root: &SysRoot, lines: &[&str]) -> Option<(usize, Rule)> {
    let rules: Vec<(usize, Rule)> = lines.iter().enumerate().filter_map(|(i, l)| Rule::parse(l).map(|r| (i, r))).filter(|(_, r)| has_stage(root, r, AUTH)).collect();
    rules.iter().rev().find(|(_, r)| runs_password_module(root, r, AUTH)).or(rules.last()).cloned()
}

/// Where the session block goes (after this line; None: the end).
fn session_anchor(root: &SysRoot, lines: &[&str]) -> Option<usize> {
    lines.iter().enumerate().rev().find(|(_, l)| Rule::parse(l).is_some_and(|r| has_stage(root, &r, SESSION))).map(|(i, _)| i)
}

/// `base` with the two blocks inserted.
fn compose(root: &SysRoot, base: &str, auth: &[String], session: &[String], note: &str) -> Result<String, String> {
    let lines: Vec<&str> = base.lines().collect();
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    let auth_at = if auth.is_empty() { None } else { Some(auth_anchor(root, &lines).ok_or("the file has no auth lines; it doesn't look like a PAM service file")?.0 + 1) };
    let session_at = (!session.is_empty()).then(|| session_anchor(root, &lines).map_or(out.len(), |i| i + 1));
    // Insert the later one first so the other index stays valid.
    let mut inserts: Vec<(usize, Vec<String>)> = Vec::new();
    if let Some(at) = auth_at {
        inserts.push((at, block(note, auth)));
    }
    if let Some(at) = session_at {
        inserts.push((at, block(note, session)));
    }
    inserts.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    for (at, lines) in inserts {
        out.splice(at..at, lines);
    }
    let mut s = out.join("\n");
    s.push('\n');
    Ok(s)
}

/// Adds the "copied from the vendor file" header block (after `#%PAM-1.0`).
fn with_copy_header(text: &str, vendor_path: &str) -> String {
    let header = block(&format!("copied from {vendor_path} by `mandara greeter keyring-unlock`; --remove deletes this copy"), &[]);
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let at = usize::from(lines.first().is_some_and(|l| l.starts_with(PAM_HEADER)));
    lines.splice(at..at, header);
    let mut s = lines.join("\n");
    s.push('\n');
    s
}

/// True when `text` carries the copy header.
fn is_vendor_copy(text: &str) -> bool {
    text.lines().any(|l| l.trim().starts_with(BEGIN) && l.contains(VENDOR_PAM_DIR) && l.contains("copied from"))
}

/// The PAM service greetd authenticates user sessions with
/// (`[general] service`, default `greetd`).
pub fn greetd_service(root: &SysRoot) -> String {
    root.read(GREETD_CONFIG)
        .and_then(|t| toml::from_str::<toml::Table>(&t).ok())
        .and_then(|t| Some(t.get("general")?.get("service")?.as_str()?.to_string()))
        .filter(|s| !s.is_empty() && !s.contains('/'))
        .unwrap_or_else(|| DEFAULT_SERVICE.to_string())
}

/// greetd logs someone in without a password (`[initial_session]`).
pub fn autologin(root: &SysRoot) -> bool {
    root.read(GREETD_CONFIG).and_then(|t| toml::from_str::<toml::Table>(&t).ok()).is_some_and(|t| t.contains_key("initial_session"))
}

/// The installed module of `k` (file name, absolute path).
fn installed_module(root: &SysRoot, k: Keyring) -> Option<(String, String)> {
    k.modules().iter().find_map(|m| MODULE_DIRS.iter().map(|d| format!("{d}/{m}")).find(|p| root.path(p).is_file()).map(|p| (m.to_string(), p)))
}

/// Each keyring's state in `text` (the service file at `file`).
fn statuses(root: &SysRoot, text: &str, file: &str, ranges: &[(usize, usize)], family: Family) -> Vec<ModuleStatus> {
    let hook = |ty: &str, k: Keyring| {
        let mut ours = Vec::new();
        collect(root, text, file, ty, 0, &|i| !in_blocks(ranges, i), &mut ours);
        let mut theirs = Vec::new();
        collect(root, text, file, ty, 0, &|i| in_blocks(ranges, i), &mut theirs);
        let is_k = |r: &ModuleRef| k.modules().contains(&r.module.as_str());
        match (ours.iter().any(is_k), theirs.iter().find(|r| is_k(r))) {
            (_, Some(r)) => Hook::Elsewhere(r.file.clone()),
            (true, None) => Hook::Mandara,
            (false, None) => Hook::Missing,
        }
    };
    Keyring::ALL
        .iter()
        .map(|&k| {
            let installed = installed_module(root, k);
            ModuleStatus {
                keyring: k,
                module: installed.as_ref().map(|(m, _)| m.clone()),
                module_path: installed.map(|(_, p)| p),
                daemon: k.daemons().iter().any(|d| root.has_binary(d)),
                auth: hook(AUTH, k),
                session: hook(SESSION, k),
                package: k.package(family),
            }
        })
        .collect()
}

// ── The plan ─────────────────────────────────────────────────────────────

/// What `greeter keyring-unlock` would change, built from what is on disk.
#[derive(Clone, Debug)]
pub struct KeyringPlan {
    pub action: Action,
    /// greetd's PAM service name.
    pub service: String,
    /// The file Mandara writes: `/etc/pam.d/<service>`.
    pub path: String,
    /// The distribution's file it starts from when `path` doesn't exist.
    pub vendor: Option<String>,
    /// `path`'s text now (None: it doesn't exist).
    pub previous: Option<String>,
    /// `path`'s text afterwards (None: it doesn't exist afterwards).
    pub contents: Option<String>,
    /// Each keyring's state now.
    pub modules: Vec<ModuleStatus>,
    /// Rule lines the change adds / removes.
    pub adding: Vec<String>,
    pub removing: Vec<String>,
    /// greetd autologin is configured (no password, so no unlock).
    pub autologin: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl KeyringPlan {
    pub fn build(root: &SysRoot, action: Action) -> KeyringPlan {
        let service = greetd_service(root);
        let family = crate::manager::family(root);
        let path = format!("{ETC_PAM_DIR}/{service}");
        let vendor_path = format!("{VENDOR_PAM_DIR}/{service}");
        let previous = root.read(&path);
        let vendor_text = root.read(&vendor_path);
        let mut plan = KeyringPlan {
            action,
            service,
            path: path.clone(),
            vendor: previous.is_none().then(|| vendor_text.as_ref().map(|_| vendor_path.clone())).flatten(),
            previous: previous.clone(),
            contents: previous.clone(),
            modules: Vec::new(),
            adding: Vec::new(),
            removing: Vec::new(),
            autologin: autologin(root),
            errors: Vec::new(),
            warnings: Vec::new(),
        };
        let Some((file, text)) = previous.clone().map(|t| (path.clone(), t)).or_else(|| vendor_text.clone().map(|t| (vendor_path.clone(), t))) else {
            plan.errors.push(format!("greetd's PAM service `{}` wasn't found ({path} or {vendor_path}); is greetd installed?", plan.service));
            plan.modules = statuses(root, "", &path, &[], family);
            return plan;
        };
        let ranges = match blocks(&text) {
            Ok(r) => r,
            Err(e) => {
                plan.errors.push(format!("{file}: unbalanced Mandara markers ({e}); fix the file by hand first"));
                Vec::new()
            }
        };
        plan.modules = statuses(root, &text, &file, &ranges, family);
        if !plan.errors.is_empty() {
            return plan;
        }
        let base = strip_blocks(&text, &ranges);
        // Mandara's own copy of the vendor file keeps its header (and goes
        // away once nothing of Mandara's is left in it).
        let own_copy = previous.as_deref().is_some_and(is_vendor_copy) && vendor_text.as_deref().is_some_and(|v| same_text(v, &base));
        let note = "added by `mandara greeter keyring-unlock`; --remove takes it out";
        plan.contents = match action {
            Action::Add => {
                let fresh = statuses(root, &base, &file, &[], family);
                let (mut auth, mut session) = (Vec::new(), Vec::new());
                for s in &fresh {
                    let Some(module) = &s.module else { continue };
                    if !s.auth.present() {
                        auth.push(rule_line(AUTH, module, s.keyring.auth_args()));
                    }
                    if !s.session.present() {
                        session.push(rule_line(SESSION, module, s.keyring.session_args()));
                    }
                }
                if auth.is_empty() && session.is_empty() {
                    // Nothing (left) to add: drop a stale block, keep the rest.
                    previous.as_ref().filter(|_| !own_copy).map(|_| base.clone())
                } else {
                    match compose(root, &base, &auth, &session, note) {
                        Ok(new) if previous.is_none() || own_copy => Some(with_copy_header(&new, &vendor_path)),
                        Ok(new) => Some(new),
                        Err(e) => {
                            plan.errors.push(format!("{file}: {e}"));
                            return plan;
                        }
                    }
                }
            }
            Action::Remove => match &previous {
                None => None,
                Some(_) if own_copy => None,
                Some(_) => Some(base.clone()),
            },
        };
        let old = block_rules(previous.as_deref().unwrap_or_default());
        let new = block_rules(plan.contents.as_deref().unwrap_or_default());
        plan.adding = new.iter().filter(|l| !old.contains(l)).cloned().collect();
        plan.removing = old.iter().filter(|l| !new.contains(l)).cloned().collect();
        if let Err(e) = plan.validate(&base) {
            plan.errors.push(e);
        }
        // Hints and caveats.
        for s in &plan.modules {
            if !s.installed() && s.daemon {
                plan.warnings.push(format!("{} is installed but its PAM module isn't (package {}); install it to unlock {} at login.", s.keyring.name(), s.package, s.keyring.name()));
            }
        }
        if action == Action::Add && plan.autologin {
            plan.warnings.push(format!("{GREETD_CONFIG} has an [initial_session] (autologin): that login has no password, so keyrings stay locked until you unlock them."));
        }
        if action == Action::Add && plan.adding.iter().any(|l| l.starts_with(&format!("-{AUTH}"))) {
            let lines: Vec<&str> = base.lines().collect();
            if let Some((_, rule)) = auth_anchor(root, &lines)
                && rule.control == "include"
                && let Some((inc_path, inc)) = resolve(root, &rule.target)
            {
                let mut refs = Vec::new();
                collect(root, &inc, &inc_path, AUTH, 1, &|_| false, &mut refs);
                if refs.iter().any(|r| r.control == "sufficient" || r.control.contains("=done")) {
                    plan.warnings.push(format!("`auth include {}` can end the auth stack early, so the keyring auth line may not run; changing it to `auth substack {}` fixes that (not done automatically).", rule.target, rule.target));
                }
            }
        }
        plan
    }

    /// The result differs from what's there.
    pub fn changes(&self) -> bool {
        self.errors.is_empty() && self.contents != self.previous
    }

    /// No keyring PAM module is installed at all.
    pub fn nothing_installed(&self) -> bool {
        self.modules.iter().all(|m| !m.installed())
    }

    /// Checks the new text: Mandara's blocks are balanced and hold only
    /// optional auth / session rules, and without them the file is exactly
    /// what it was.
    fn validate(&self, base: &str) -> Result<(), String> {
        let Some(new) = &self.contents else {
            return Ok(());
        };
        let ranges = blocks(new).map_err(|e| format!("internal error: the new file has unbalanced markers ({e})"))?;
        let stripped = strip_blocks(new, &ranges);
        if !same_text(&stripped, base) {
            return Err(format!("internal error: the rewrite of {} would change lines outside Mandara's blocks (nothing was changed)", self.path));
        }
        for line in block_rules(new) {
            let rule = Rule::parse(&line).ok_or("internal error: unparsable rule")?;
            let ok = matches!(rule.ty.as_deref(), Some(AUTH | SESSION)) && rule.control == CONTROL && rule.module().is_some_and(|m| Keyring::ALL.iter().any(|k| k.modules().contains(&m)));
            if !ok {
                return Err(format!("internal error: unexpected rule `{line}` in a Mandara block"));
            }
        }
        Ok(())
    }

    pub fn describe(&self) -> String {
        let mut s = String::new();
        let verb = match self.action {
            Action::Add => "unlock at login",
            Action::Remove => "remove the login unlock",
        };
        let _ = writeln!(s, "Keyrings — {verb} plan (greetd PAM service `{}`)\n", self.service);
        match &self.vendor {
            Some(v) => {
                let _ = writeln!(s, "  File:   {} (not there yet; starts from the distribution's {v})", self.path);
            }
            None => {
                let _ = writeln!(s, "  File:   {}", self.path);
            }
        }
        for m in &self.modules {
            let state = match (&m.module, m.unlocks_at_login()) {
                (None, _) => format!("module not installed (package {})", m.package),
                (Some(module), true) if m.by_mandara() => format!("{module}: unlocks at login (Mandara's lines)"),
                (Some(module), true) => format!("{module}: unlocks at login (already set up{})", elsewhere_note(m)),
                (Some(module), false) => format!("{module}: installed, not unlocked at login"),
            };
            let _ = writeln!(s, "  {:<14} {state}", m.keyring.name());
        }
        if self.changes() {
            let _ = writeln!(s, "\n  Changes (backed up first to {BACKUP_DIR}/<timestamp>/):");
            match &self.contents {
                None => {
                    let _ = writeln!(s, "    delete {} (Mandara's copy; {} applies again)", self.path, self.vendor_path());
                }
                Some(new) => {
                    let ranges = blocks(new).unwrap_or_default();
                    for (i, line) in new.lines().enumerate() {
                        let mark = if in_blocks(&ranges, i) { "+" } else { " " };
                        let _ = writeln!(s, "    {mark} {line}");
                    }
                }
            }
            for line in &self.removing {
                let _ = writeln!(s, "    - {line}");
            }
        } else if self.errors.is_empty() {
            let _ = writeln!(s, "\n  Nothing to change.");
        }
        if self.action == Action::Add && self.errors.is_empty() {
            let _ = writeln!(s, "\n  Works when you log in with your password through Mandara Login (not autologin),");
            let _ = writeln!(s, "  and only for keyrings whose password is your login password.");
            let _ = writeln!(s, "  Undo: sudo mandara greeter keyring-unlock --remove  (or: sudo mandara greeter restore latest)");
        }
        for e in &self.errors {
            let _ = writeln!(s, "\n  error: {e}");
        }
        for w in &self.warnings {
            let _ = writeln!(s, "\n  warning: {w}");
        }
        let status = if !self.errors.is_empty() {
            "Status: error"
        } else if self.changes() {
            STATUS_CHANGES
        } else if self.action == Action::Add && self.nothing_installed() {
            STATUS_NOTHING
        } else {
            STATUS_UP_TO_DATE
        };
        let _ = writeln!(s, "\n{status}");
        s
    }

    fn vendor_path(&self) -> String {
        format!("{VENDOR_PAM_DIR}/{}", self.service)
    }

    /// Performs the plan as root (backup, write, verify; rollback on error).
    pub fn execute(&self, root: &SysRoot) -> Result<String, String> {
        if !self.errors.is_empty() {
            return Err(format!("{}\n(nothing was changed)", self.errors.join("\n")));
        }
        if !self.changes() {
            return Ok("Nothing to change.".into());
        }
        if !crate::install::is_root() {
            return Err("not running as root: re-run with sudo (or from Studio) to change greetd's PAM file (nothing was changed)".into());
        }
        self.perform(root)
    }

    /// [`Self::execute`] without the root check (tests use fixture roots).
    pub(crate) fn perform(&self, root: &SysRoot) -> Result<String, String> {
        if !self.errors.is_empty() {
            return Err(format!("{}\n(nothing was changed)", self.errors.join("\n")));
        }
        if !self.changes() {
            return Ok("Nothing to change.".into());
        }
        // The file must still be what the plan was built from.
        if root.read(&self.path) != self.previous {
            return Err(format!("{} changed since the plan was made; run it again (nothing was changed)", self.path));
        }
        let manifest = Manifest { created: chrono::Local::now().to_rfc3339(), target: BACKUP_TARGET.into(), previous_unit: None, new_unit: None, units_switched: false, files: Vec::new() };
        let write = FileWrite { path: self.path.clone(), contents: self.contents.clone().unwrap_or_default(), previous: self.previous.clone() };
        let backup = Backup::create(root, manifest, std::slice::from_ref(&write))?;
        let mut log = vec![format!("backed up to {}", backup.logical_dir())];
        let p = root.path(&self.path);
        let mode = std::fs::metadata(&p).ok().map(|m| m.permissions());
        let result = (|| -> Result<(), String> {
            match &self.contents {
                Some(text) => {
                    if let Some(parent) = p.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
                    }
                    write_atomic(&p, text)?;
                    if let Some(mode) = &mode {
                        std::fs::set_permissions(&p, mode.clone()).map_err(|e| format!("{}: {e}", self.path))?;
                    }
                    if root.read(&self.path).as_deref() != Some(text.as_str()) {
                        return Err(format!("{} doesn't read back as written", self.path));
                    }
                    log.push(format!("wrote {}", self.path));
                }
                None => {
                    std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", self.path))?;
                    log.push(format!("removed {} (Mandara's copy)", self.path));
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            let tail = match backup.restore_files(root) {
                Ok(_) => "everything was rolled back".to_string(),
                Err(err) => format!("rollback problem: {err}"),
            };
            return Err(format!("{e}\n({tail})"));
        }
        let mut s: String = log.iter().map(|l| format!("✓ {l}\n")).collect();
        match self.action {
            Action::Add => {
                let names: Vec<&str> = self.modules.iter().filter(|m| m.installed()).map(|m| m.keyring.name()).collect();
                let _ = writeln!(s, "\n{} unlock at your next password login through Mandara Login.", names.join(" and "));
                let _ = write!(s, "Undo: sudo mandara greeter keyring-unlock --remove");
            }
            Action::Remove => {
                let _ = write!(s, "\nKeyrings are no longer unlocked at login by Mandara's lines.");
            }
        }
        Ok(s)
    }
}

/// Equal up to a missing final newline (rewrites always end with one).
fn same_text(a: &str, b: &str) -> bool {
    a.trim_end_matches('\n') == b.trim_end_matches('\n')
}

fn elsewhere_note(m: &ModuleStatus) -> String {
    match (&m.auth, &m.session) {
        (Hook::Elsewhere(f), _) | (_, Hook::Elsewhere(f)) => format!(" in {f}"),
        _ => String::new(),
    }
}

#[cfg(test)]
#[path = "keyring_pam_tests.rs"]
mod tests;
