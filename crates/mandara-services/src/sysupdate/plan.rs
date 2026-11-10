//! The fixed commands of a system update, one backend per family.
//!
//! Every step is a constant argv: nothing in it comes from the user, the
//! shell or the environment, so the root side can run it as it is. Steps
//! never pass force or overwrite options (`--overwrite`, `--force-*`
//! of the package itself, `--allow-downgrade`…): a conflict that needs a
//! person stops the update and shows the output. The one dpkg option used,
//! `--force-confdef --force-confold`, only means "keep the configuration
//! files you changed" instead of asking (dpkg would otherwise stop on its
//! prompt with no terminal attached).
//!
//! | Family | Refresh (cancellable) | Download (cancellable) | Install |
//! |---|---|---|---|
//! | Arch | `pacman -Syuw --noconfirm` (sync + download) | — | `pacman -Su --noconfirm` |
//! | Debian, Ubuntu | `apt-get update` | `apt-get -y -d dist-upgrade` | `apt-get -y dist-upgrade` (`APT::Status-Fd` progress) |
//! | Fedora | — | `dnf -y upgrade --refresh --downloadonly` | `dnf -y upgrade` |
//! | openSUSE Tumbleweed | `zypper -n refresh` | `zypper -n dup --download-only` | `zypper -n dup` |
//! | openSUSE Leap | `zypper -n refresh` | `zypper -n update --download-only` | `zypper -n update` |
//! | Void | `xbps-install -S` | `xbps-install -uyD` | `xbps-install -uy` |
//! | Alpine | `apk update` | — | `apk upgrade` |
//! | FreeBSD | `pkg update` | `pkg upgrade -U -F -y`, then `freebsd-update fetch` | `pkg upgrade -U -y`, then `freebsd-update install` |
//!
//! Splitting refresh and download from the install step makes the safe
//! point explicit: cancelling is offered until the install step starts,
//! never during it.

use serde::{Deserialize, Serialize};

use super::parse::{self, Tracker};
use super::{FailureKind, Phase};

/// What a step does (decides its phase and whether it may be cancelled).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepKind {
    Refresh,
    Download,
    Install,
    /// The package manager must update itself first (xbps, apk).
    SelfUpdate,
    Flatpak,
    FlakeUpdate,
    ChannelUpdate,
    Build,
    Activate,
    DeleteGenerations,
    CollectGarbage,
    Bootloader,
    /// AUR packages, built and installed by the AUR helper as you.
    Aur,
}

impl StepKind {
    pub const ALL: [StepKind; 13] = [
        StepKind::Refresh,
        StepKind::Download,
        StepKind::Install,
        StepKind::SelfUpdate,
        StepKind::Flatpak,
        StepKind::FlakeUpdate,
        StepKind::ChannelUpdate,
        StepKind::Build,
        StepKind::Activate,
        StepKind::DeleteGenerations,
        StepKind::CollectGarbage,
        StepKind::Bootloader,
        StepKind::Aur,
    ];

    pub fn id(self) -> &'static str {
        match self {
            StepKind::Refresh => "refresh",
            StepKind::Download => "download",
            StepKind::Install => "install",
            StepKind::SelfUpdate => "self-update",
            StepKind::Flatpak => "flatpak",
            StepKind::FlakeUpdate => "flake-update",
            StepKind::ChannelUpdate => "channel-update",
            StepKind::Build => "build",
            StepKind::Activate => "activate",
            StepKind::DeleteGenerations => "delete-generations",
            StepKind::CollectGarbage => "collect-garbage",
            StepKind::Bootloader => "bootloader",
            StepKind::Aur => "aur",
        }
    }

    pub fn from_id(id: &str) -> Option<StepKind> {
        StepKind::ALL.into_iter().find(|k| k.id() == id)
    }

    /// Whether stopping now leaves the system as it was: refreshing,
    /// downloading and building only add to caches, and Flatpak deploys
    /// each app atomically. Installing, activating and deleting are never
    /// interrupted. The AUR step is stopped only while it builds (the job
    /// holds a cancel back while the helper's pacman installs).
    pub fn cancellable(self) -> bool {
        matches!(self, StepKind::Refresh | StepKind::Download | StepKind::FlakeUpdate | StepKind::ChannelUpdate | StepKind::Build | StepKind::Flatpak | StepKind::Aur)
    }

    /// The phase shown when the step starts (parsers refine it).
    pub fn phase(self) -> Phase {
        match self {
            StepKind::Refresh | StepKind::FlakeUpdate | StepKind::ChannelUpdate => Phase::Refreshing,
            StepKind::Download => Phase::Downloading,
            StepKind::Install | StepKind::SelfUpdate => Phase::Installing,
            StepKind::Flatpak => Phase::Flatpak,
            StepKind::Build => Phase::Building,
            StepKind::Activate | StepKind::Bootloader => Phase::Activating,
            StepKind::DeleteGenerations | StepKind::CollectGarbage => Phase::Cleaning,
            StepKind::Aur => Phase::AurBuild,
        }
    }
}

/// One fixed command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub kind: StepKind,
    /// A bare program name, looked up in [`super::SAFE_DIRS`] only.
    pub program: &'static str,
    /// NixOS only: the exact program in the Nix store (a generation's
    /// `switch-to-configuration`), checked to be root-owned in the store.
    pub exe: Option<std::path::PathBuf>,
    pub args: Vec<String>,
    /// Extra environment (on top of the fixed `PATH`, `LANG=C`…).
    pub env: &'static [(&'static str, &'static str)],
    /// NixOS flakes: the Git work tree root may read for this one command
    /// although another user owns it (a `safe.directory` entry in a Git
    /// configuration given to this program only; see
    /// [`super::helper::GitTrust`]).
    pub git_safe_dir: Option<std::path::PathBuf>,
}

impl Step {
    pub fn new(kind: StepKind, program: &'static str, args: &[&str]) -> Step {
        Step { kind, program, exe: None, args: args.iter().map(|a| a.to_string()).collect(), env: &[], git_safe_dir: None }
    }

    /// With owned arguments (paths the root side found itself).
    pub fn owned(kind: StepKind, program: &'static str, args: Vec<String>) -> Step {
        Step { kind, program, exe: None, args, env: &[], git_safe_dir: None }
    }

    /// Lets root's Git read the work tree `dir` for this step only.
    pub fn trust_git(mut self, dir: Option<std::path::PathBuf>) -> Step {
        self.git_safe_dir = dir;
        self
    }

    pub fn env(mut self, env: &'static [(&'static str, &'static str)]) -> Step {
        self.env = env;
        self
    }

    /// `program args…`, for logs and tests.
    pub fn display(&self) -> String {
        let program = self.exe.as_deref().and_then(|p| p.to_str()).unwrap_or(self.program);
        std::iter::once(program).chain(self.args.iter().map(String::as_str)).collect::<Vec<_>>().join(" ")
    }
}

/// What happens after a step exits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Next {
    Continue,
    /// Run this step before going on (a package manager that must update
    /// itself first, or asks to be run again).
    Insert(Step),
    /// The update finished successfully early (nothing left to do).
    Finish,
    Fail(FailureKind),
}

/// The package managers Mandara drives.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackendId {
    Pacman,
    Apt,
    Dnf,
    ZypperDup,
    ZypperUpdate,
    Xbps,
    Apk,
    /// NixOS generations (planned by [`super::nixos::plan`]).
    Nix,
    /// FreeBSD: packages with pkg, then the base system with
    /// freebsd-update.
    Pkg,
}

impl BackendId {
    pub const ALL: [BackendId; 9] = [BackendId::Pacman, BackendId::Apt, BackendId::Dnf, BackendId::ZypperDup, BackendId::ZypperUpdate, BackendId::Xbps, BackendId::Apk, BackendId::Nix, BackendId::Pkg];

    pub fn id(self) -> &'static str {
        match self {
            BackendId::Pacman => "pacman",
            BackendId::Apt => "apt",
            BackendId::Dnf => "dnf",
            BackendId::ZypperDup => "zypper-dup",
            BackendId::ZypperUpdate => "zypper-update",
            BackendId::Xbps => "xbps",
            BackendId::Apk => "apk",
            BackendId::Nix => "nix",
            BackendId::Pkg => "pkg",
        }
    }

    pub fn from_id(id: &str) -> Option<BackendId> {
        BackendId::ALL.into_iter().find(|b| b.id() == id)
    }

    /// The package manager's name (a proper noun, not translated).
    pub fn manager_label(self) -> &'static str {
        match self {
            BackendId::Pacman => "pacman",
            BackendId::Apt => "APT",
            BackendId::Dnf => "DNF",
            BackendId::ZypperDup | BackendId::ZypperUpdate => "Zypper",
            BackendId::Xbps => "XBPS",
            BackendId::Apk => "apk",
            BackendId::Nix => "Nix",
            BackendId::Pkg => "pkg",
        }
    }
}

/// One family's update: its steps, how to read their output and what a
/// failure means.
pub trait SystemBackend: Sync {
    fn id(&self) -> BackendId;
    /// The fixed steps of a full update, in order.
    fn steps(&self) -> Vec<Step>;
    /// Feeds one output line of a `step` to the progress tracker.
    fn parse(&self, step: StepKind, line: &str, t: &mut Tracker);
    /// The family's own failure messages (checked before the generic ones).
    fn failure_patterns(&self) -> &'static [(&'static str, FailureKind)];
    /// After `step` exited with `code`.
    fn after(&self, step: &Step, code: Option<i32>, t: &Tracker) -> Next {
        let _ = t;
        match (step.kind, code) {
            (_, Some(0)) => Next::Continue,
            _ => Next::Fail(FailureKind::Other),
        }
    }
    /// Classifies a failed step's output (`tail`: its last lines).
    fn classify(&self, tail: &[&str]) -> FailureKind {
        classify_with(self.failure_patterns(), tail)
    }
}

/// Generic failure messages every package manager may print.
const GENERIC: &[(&str, FailureKind)] = &[
    ("no space left on device", FailureKind::DiskFull),
    ("not enough free disk space", FailureKind::DiskFull),
    ("temporary failure in name resolution", FailureKind::Network),
    ("could not resolve host", FailureKind::Network),
    ("could not resolve", FailureKind::Network),
    ("network is unreachable", FailureKind::Network),
    ("connection timed out", FailureKind::Network),
    ("connection refused", FailureKind::Network),
];

/// The first pattern (lower-case substring) any of `tail`'s lines
/// matches, family patterns first.
pub fn classify_with(patterns: &[(&str, FailureKind)], tail: &[&str]) -> FailureKind {
    for table in [patterns, GENERIC] {
        for (pattern, kind) in table {
            if tail.iter().any(|line| contains_ci(line, pattern)) {
                return *kind;
            }
        }
    }
    FailureKind::Other
}

/// Case-insensitive ASCII substring test without allocating.
pub fn contains_ci(hay: &str, needle: &str) -> bool {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    n.is_empty() || h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

// ── Arch ─────────────────────────────────────────────────────────────────

pub struct Pacman;

impl SystemBackend for Pacman {
    fn id(&self) -> BackendId {
        BackendId::Pacman
    }

    /// `-Syuw` syncs the databases and downloads the whole upgrade; `-Su`
    /// then installs it. Together they are exactly `-Syu`: never a partial
    /// upgrade, and no AUR helper or makepkg runs as root.
    fn steps(&self) -> Vec<Step> {
        vec![Step::new(StepKind::Download, "pacman", &["-Syuw", "--noconfirm", "--color", "never"]), Step::new(StepKind::Install, "pacman", &["-Su", "--noconfirm", "--color", "never"])]
    }

    fn parse(&self, step: StepKind, line: &str, t: &mut Tracker) {
        parse::pacman(step, line, t);
    }

    fn failure_patterns(&self) -> &'static [(&'static str, FailureKind)] {
        &[
            ("unable to lock database", FailureKind::Locked),
            ("db.lck", FailureKind::Locked),
            ("conflicting files", FailureKind::Conflict),
            ("exists in filesystem", FailureKind::Conflict),
            ("are in conflict", FailureKind::Conflict),
            ("unresolvable package conflicts", FailureKind::Conflict),
            ("could not satisfy dependencies", FailureKind::Conflict),
            ("failed to prepare transaction", FailureKind::Conflict),
            ("pgp signature", FailureKind::Keyring),
            ("unknown trust", FailureKind::Keyring),
            ("could not be looked up remotely", FailureKind::Keyring),
            ("not enough free disk space", FailureKind::DiskFull),
            ("failed retrieving file", FailureKind::Network),
            ("failed to synchronize all databases", FailureKind::Network),
        ]
    }

    fn after(&self, step: &Step, code: Option<i32>, t: &Tracker) -> Next {
        match (step.kind, code) {
            // Nothing to download means nothing to install either.
            (StepKind::Download, Some(0)) if t.nothing_to_do => Next::Finish,
            (_, Some(0)) => Next::Continue,
            _ => Next::Fail(FailureKind::Other),
        }
    }
}

// ── Debian / Ubuntu ──────────────────────────────────────────────────────

pub struct Apt;

/// apt and dpkg run unattended; dpkg keeps changed configuration files.
const APT_ENV: &[(&str, &str)] = &[("DEBIAN_FRONTEND", "noninteractive"), ("APT_LISTCHANGES_FRONTEND", "none"), ("NEEDRESTART_MODE", "l")];
/// Machine-readable progress on stdout (`dlstatus:` / `pmstatus:` lines).
const APT_STATUS: [&str; 2] = ["-o", "APT::Status-Fd=1"];

impl SystemBackend for Apt {
    fn id(&self) -> BackendId {
        BackendId::Apt
    }

    fn steps(&self) -> Vec<Step> {
        let s = APT_STATUS;
        vec![
            Step::new(StepKind::Refresh, "apt-get", &[s[0], s[1], "update"]).env(APT_ENV),
            Step::new(StepKind::Download, "apt-get", &["-y", "-d", s[0], s[1], "dist-upgrade"]).env(APT_ENV),
            Step::new(
                StepKind::Install,
                "apt-get",
                &["-y", s[0], s[1], "-o", "Dpkg::Progress-Fancy=0", "-o", "Dpkg::Use-Pty=0", "-o", "Dpkg::Options::=--force-confdef", "-o", "Dpkg::Options::=--force-confold", "dist-upgrade"],
            )
            .env(APT_ENV),
        ]
    }

    fn parse(&self, step: StepKind, line: &str, t: &mut Tracker) {
        parse::apt(step, line, t);
    }

    fn failure_patterns(&self) -> &'static [(&'static str, FailureKind)] {
        &[
            ("could not get lock", FailureKind::Locked),
            ("unable to acquire the dpkg frontend lock", FailureKind::Locked),
            ("is another process using it", FailureKind::Locked),
            ("dpkg was interrupted", FailureKind::Conflict),
            ("unmet dependencies", FailureKind::Conflict),
            ("held broken packages", FailureKind::Conflict),
            ("trying to overwrite", FailureKind::Conflict),
            ("you don't have enough free space", FailureKind::DiskFull),
            ("no_pubkey", FailureKind::Keyring),
            ("is not signed", FailureKind::Keyring),
            ("signatures couldn't be verified", FailureKind::Keyring),
            ("temporary failure resolving", FailureKind::Network),
            ("failed to fetch", FailureKind::Network),
            ("could not connect", FailureKind::Network),
        ]
    }

    fn after(&self, step: &Step, code: Option<i32>, t: &Tracker) -> Next {
        match (step.kind, code) {
            (StepKind::Download, Some(0)) if t.nothing_to_do => Next::Finish,
            // `apt-get update` exits 0 even when a mirror failed ("W:
            // Failed to fetch"); the install step reports what's missing.
            (_, Some(0)) => Next::Continue,
            _ => Next::Fail(FailureKind::Other),
        }
    }
}

// ── Fedora ───────────────────────────────────────────────────────────────

/// dnf 4 or dnf5 (`dnf` is dnf5 on Fedora 41 and later; both take these
/// options).
pub struct Dnf;

impl SystemBackend for Dnf {
    fn id(&self) -> BackendId {
        BackendId::Dnf
    }

    fn steps(&self) -> Vec<Step> {
        vec![Step::new(StepKind::Download, "dnf", &["-y", "--color=never", "upgrade", "--refresh", "--downloadonly"]), Step::new(StepKind::Install, "dnf", &["-y", "--color=never", "upgrade"])]
    }

    fn parse(&self, step: StepKind, line: &str, t: &mut Tracker) {
        parse::dnf(step, line, t);
    }

    fn failure_patterns(&self) -> &'static [(&'static str, FailureKind)] {
        &[
            ("waiting for process with pid", FailureKind::Locked),
            ("transaction lock", FailureKind::Locked),
            ("another copy is running", FailureKind::Locked),
            ("conflicts with file from package", FailureKind::Conflict),
            ("problem:", FailureKind::Conflict),
            ("problem 1:", FailureKind::Conflict),
            ("transaction check error", FailureKind::Conflict),
            ("disk requirements", FailureKind::DiskFull),
            ("more space needed", FailureKind::DiskFull),
            ("gpg check failed", FailureKind::Keyring),
            ("is not installed", FailureKind::Keyring),
            ("curl error", FailureKind::Network),
            ("cannot download", FailureKind::Network),
            ("failed to download metadata", FailureKind::Network),
        ]
    }

    fn after(&self, step: &Step, code: Option<i32>, t: &Tracker) -> Next {
        match (step.kind, code) {
            (StepKind::Download, Some(0)) if t.nothing_to_do => Next::Finish,
            (_, Some(0)) => Next::Continue,
            _ => Next::Fail(FailureKind::Other),
        }
    }
}

// ── openSUSE ─────────────────────────────────────────────────────────────

/// zypper's "the update stack was updated, run me again" exit code.
pub const ZYPPER_RESTART_NEEDED: i32 = 103;
/// zypper's "a reboot is needed" exit code.
pub const ZYPPER_REBOOT_NEEDED: i32 = 102;
/// zypper's "some repositories were skipped" exit code (the rest worked).
pub const ZYPPER_REPOS_SKIPPED: i32 = 106;
/// zypper's "system management is locked" exit code.
pub const ZYPPER_LOCKED: i32 = 7;

/// `dup` on Tumbleweed and Slowroll (rolling), `update` on Leap.
pub struct Zypper {
    pub dup: bool,
}

impl Zypper {
    fn verb(&self) -> &'static str {
        if self.dup { "dup" } else { "update" }
    }

    fn install(&self) -> Step {
        Step::new(StepKind::Install, "zypper", &["--non-interactive", "--no-color", self.verb()])
    }
}

impl SystemBackend for Zypper {
    fn id(&self) -> BackendId {
        if self.dup { BackendId::ZypperDup } else { BackendId::ZypperUpdate }
    }

    fn steps(&self) -> Vec<Step> {
        vec![
            Step::new(StepKind::Refresh, "zypper", &["--non-interactive", "--no-color", "refresh"]),
            Step::new(StepKind::Download, "zypper", &["--non-interactive", "--no-color", self.verb(), "--download-only"]),
            self.install(),
        ]
    }

    fn parse(&self, step: StepKind, line: &str, t: &mut Tracker) {
        parse::zypper(step, line, t);
    }

    fn failure_patterns(&self) -> &'static [(&'static str, FailureKind)] {
        &[
            ("system management is locked", FailureKind::Locked),
            ("problem:", FailureKind::Conflict),
            ("file conflicts", FailureKind::Conflict),
            ("signature verification failed", FailureKind::Keyring),
            ("untrusted", FailureKind::Keyring),
            ("download (curl) error", FailureKind::Network),
            ("problem retrieving", FailureKind::Network),
            ("valid metadata not found", FailureKind::Network),
        ]
    }

    fn after(&self, step: &Step, code: Option<i32>, t: &Tracker) -> Next {
        match (step.kind, code) {
            (StepKind::Download, Some(0 | ZYPPER_REPOS_SKIPPED)) if t.nothing_to_do => Next::Finish,
            (_, Some(0)) => Next::Continue,
            // Some repositories couldn't be refreshed (a dead mirror or
            // third-party repository): the others still work, and zypper
            // skips the broken ones (exit 106) in the next steps.
            (StepKind::Refresh, Some(_)) if t.warnings > 0 => Next::Continue,
            (StepKind::Download | StepKind::Install, Some(ZYPPER_REPOS_SKIPPED)) => Next::Continue,
            (StepKind::Install, Some(ZYPPER_REBOOT_NEEDED)) => Next::Continue,
            // The package manager updated itself: run the install again,
            // once (the tracker counts the restarts).
            (StepKind::Install, Some(ZYPPER_RESTART_NEEDED)) if t.restarts == 0 => Next::Insert(self.install()),
            (StepKind::Install, Some(ZYPPER_RESTART_NEEDED)) => Next::Continue,
            (_, Some(ZYPPER_LOCKED)) => Next::Fail(FailureKind::Locked),
            _ => Next::Fail(FailureKind::Other),
        }
    }
}

// ── Void ─────────────────────────────────────────────────────────────────

pub struct Xbps;

impl SystemBackend for Xbps {
    fn id(&self) -> BackendId {
        BackendId::Xbps
    }

    fn steps(&self) -> Vec<Step> {
        vec![Step::new(StepKind::Refresh, "xbps-install", &["-S"]), Step::new(StepKind::Download, "xbps-install", &["-uyD"]), Step::new(StepKind::Install, "xbps-install", &["-uy"])]
    }

    fn parse(&self, step: StepKind, line: &str, t: &mut Tracker) {
        parse::xbps(step, line, t);
    }

    fn failure_patterns(&self) -> &'static [(&'static str, FailureKind)] {
        &[
            ("failed to lock", FailureKind::Locked),
            ("resource busy", FailureKind::Locked),
            ("conflicts with", FailureKind::Conflict),
            ("unresolved", FailureKind::Conflict),
            ("broken", FailureKind::Conflict),
            ("not enough free space", FailureKind::DiskFull),
            ("failed to download", FailureKind::Network),
            ("signature is not valid", FailureKind::Keyring),
            ("invalid signature", FailureKind::Keyring),
            ("[reposync] failed", FailureKind::Network),
            ("failed to fetch", FailureKind::Network),
        ]
    }

    /// xbps refuses to upgrade anything while xbps itself is outdated
    /// ("The 'xbps' package must be updated"): it is updated first, then
    /// the same step runs again.
    fn after(&self, step: &Step, code: Option<i32>, t: &Tracker) -> Next {
        match (step.kind, code) {
            (StepKind::Download | StepKind::Install, _) if t.self_update_needed && t.restarts == 0 => Next::Insert(Step::new(StepKind::SelfUpdate, "xbps-install", &["-uy", "xbps"])),
            (StepKind::SelfUpdate, Some(0)) => Next::Insert(Step::new(StepKind::Download, "xbps-install", &["-uyD"])),
            // Up to date, xbps prints nothing at all.
            (StepKind::Download, Some(0)) if !t.planned => Next::Finish,
            (_, Some(0)) => Next::Continue,
            _ => Next::Fail(FailureKind::Other),
        }
    }
}

// ── Alpine ───────────────────────────────────────────────────────────────

pub struct Apk;

impl SystemBackend for Apk {
    fn id(&self) -> BackendId {
        BackendId::Apk
    }

    /// apk upgrades itself and the C library first inside `apk upgrade`
    /// ("Upgrading critical system libraries and apk-tools"), then
    /// continues with the new apk on its own.
    fn steps(&self) -> Vec<Step> {
        vec![Step::new(StepKind::Refresh, "apk", &["update"]), Step::new(StepKind::Install, "apk", &["upgrade", "--no-progress"])]
    }

    fn parse(&self, step: StepKind, line: &str, t: &mut Tracker) {
        parse::apk(step, line, t);
    }

    fn failure_patterns(&self) -> &'static [(&'static str, FailureKind)] {
        &[
            ("unable to lock database", FailureKind::Locked),
            ("unable to select packages", FailureKind::Conflict),
            ("breaks:", FailureKind::Conflict),
            ("conflicts:", FailureKind::Conflict),
            ("untrusted signature", FailureKind::Keyring),
            ("bad signature", FailureKind::Keyring),
            ("network error", FailureKind::Network),
            ("temporary error", FailureKind::Network),
            ("dns lookup error", FailureKind::Network),
        ]
    }
}

// ── FreeBSD ──────────────────────────────────────────────────────────────

/// `freebsd-update`, the base system's updater.
pub const FREEBSD_UPDATE: &str = "freebsd-update";
/// freebsd-update refuses to `fetch` without a terminal unless told it
/// runs unattended (it would otherwise ask to be run from cron).
const FREEBSD_UPDATE_UNATTENDED: &str = "--not-running-from-cron";
/// freebsd-update pages its file lists through `$PAGER` (`less` when
/// unset): `cat` prints them straight into the log.
const FREEBSD_UPDATE_ENV: &[(&str, &str)] = &[("PAGER", "cat")];
/// freebsd-update's exit code for "nothing was fetched to install".
pub const FREEBSD_UPDATE_NOTHING: i32 = 2;
/// freebsd-update's exit code when it refuses the system (not a -RELEASE,
/// or a packaged base); also any other error, so it counts only with the
/// refusal's words in the output.
pub const FREEBSD_UPDATE_UNMANAGED: i32 = 1;

/// FreeBSD: third-party packages with pkg (refreshed, downloaded, then
/// installed; `-U` because the catalogue was just refreshed), then the
/// base system's security and errata patches with freebsd-update. The
/// steps are interleaved so every download happens before anything is
/// installed: cancelling stays possible until `pkg upgrade` installs.
pub struct Pkg;

impl Pkg {
    fn freebsd_update(kind: StepKind, verb: &'static str) -> Step {
        Step::new(kind, FREEBSD_UPDATE, &[FREEBSD_UPDATE_UNATTENDED, verb]).env(FREEBSD_UPDATE_ENV)
    }
}

impl SystemBackend for Pkg {
    fn id(&self) -> BackendId {
        BackendId::Pkg
    }

    fn steps(&self) -> Vec<Step> {
        vec![
            Step::new(StepKind::Refresh, "pkg", &["update"]),
            Step::new(StepKind::Download, "pkg", &["upgrade", "-U", "-F", "-y"]),
            Pkg::freebsd_update(StepKind::Download, "fetch"),
            Step::new(StepKind::Install, "pkg", &["upgrade", "-U", "-y"]),
            Pkg::freebsd_update(StepKind::Install, "install"),
        ]
    }

    fn parse(&self, step: StepKind, line: &str, t: &mut Tracker) {
        // Both programs run in download and install steps; their output
        // never overlaps, so each parser ignores the other's lines.
        parse::pkg(step, line, t);
        parse::freebsd_update(step, line, t);
    }

    fn failure_patterns(&self) -> &'static [(&'static str, FailureKind)] {
        &[
            ("cannot get an advisory lock", FailureKind::Locked),
            ("locked by another process", FailureKind::Locked),
            ("not enough space in", FailureKind::DiskFull),
            ("conflicts with", FailureKind::Conflict),
            ("conflicting", FailureKind::Conflict),
            ("is locked and may not be modified", FailureKind::Conflict),
            ("missing dependency", FailureKind::Conflict),
            ("no trusted public keys found", FailureKind::Keyring),
            ("invalid signature", FailureKind::Keyring),
            ("signature verification failed", FailureKind::Keyring),
            ("checksum mismatch", FailureKind::Keyring),
            ("key has incorrect hash", FailureKind::Keyring),
            ("no address record", FailureKind::Network),
            ("unable to update repository", FailureKind::Network),
            ("error updating repositories", FailureKind::Network),
            ("mirrors... none found", FailureKind::Network),
            ("no mirrors remaining", FailureKind::Network),
            ("operation timed out", FailureKind::Network),
        ]
    }

    fn after(&self, step: &Step, code: Option<i32>, t: &Tracker) -> Next {
        let base = step.program == FREEBSD_UPDATE;
        match (step.kind, code) {
            // Nothing to fetch for the base system (exit 0), or
            // freebsd-update doesn't manage it (exit 1: a -STABLE system,
            // packaged base): the packages downloaded before still install.
            (StepKind::Download, Some(0 | FREEBSD_UPDATE_UNMANAGED)) if base && t.nothing_to_do => Next::Continue,
            // Past its end of life, freebsd-update still fetches what was
            // published, then exits 1 with a warning: install that.
            (StepKind::Download, Some(1)) if base && t.end_of_life => Next::Continue,
            // Then nothing is fetched to install (exit 2; a -STABLE system
            // too), or packaged base is refused again (exit 1).
            (StepKind::Install, Some(FREEBSD_UPDATE_NOTHING | FREEBSD_UPDATE_UNMANAGED)) if base && t.nothing_to_do => Next::Continue,
            (_, Some(0)) => Next::Continue,
            _ => Next::Fail(FailureKind::Other),
        }
    }
}

/// The backend of `id`.
pub fn backend(id: BackendId) -> &'static dyn SystemBackend {
    match id {
        BackendId::Pacman => &Pacman,
        BackendId::Apt => &Apt,
        BackendId::Dnf => &Dnf,
        BackendId::ZypperDup => &Zypper { dup: true },
        BackendId::ZypperUpdate => &Zypper { dup: false },
        BackendId::Xbps => &Xbps,
        BackendId::Apk => &Apk,
        BackendId::Nix => &super::nixos::NixBackend,
        BackendId::Pkg => &Pkg,
    }
}

// ── Flatpak (as the user) ────────────────────────────────────────────────

/// `flatpak update -y --noninteractive`, run as the user: never root.
/// System-wide installations are updated through flatpak's own polkit
/// rule, so the same password dialog appears when one is needed.
pub fn flatpak_step() -> Step {
    Step::new(StepKind::Flatpak, "flatpak", &["update", "-y", "--noninteractive"])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact commands, per family: any change here is a change to what
    /// runs as root.
    #[test]
    fn fixed_commands_per_family() {
        let shown = |id: BackendId| backend(id).steps().iter().map(|s| format!("{}: {}", s.kind.id(), s.display())).collect::<Vec<_>>();
        assert_eq!(shown(BackendId::Pacman), ["download: pacman -Syuw --noconfirm --color never", "install: pacman -Su --noconfirm --color never"]);
        assert_eq!(
            shown(BackendId::Apt),
            [
                "refresh: apt-get -o APT::Status-Fd=1 update",
                "download: apt-get -y -d -o APT::Status-Fd=1 dist-upgrade",
                "install: apt-get -y -o APT::Status-Fd=1 -o Dpkg::Progress-Fancy=0 -o Dpkg::Use-Pty=0 -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold dist-upgrade"
            ]
        );
        assert_eq!(shown(BackendId::Dnf), ["download: dnf -y --color=never upgrade --refresh --downloadonly", "install: dnf -y --color=never upgrade"]);
        assert_eq!(
            shown(BackendId::ZypperDup),
            ["refresh: zypper --non-interactive --no-color refresh", "download: zypper --non-interactive --no-color dup --download-only", "install: zypper --non-interactive --no-color dup"]
        );
        assert_eq!(shown(BackendId::ZypperUpdate)[2], "install: zypper --non-interactive --no-color update");
        assert_eq!(shown(BackendId::Xbps), ["refresh: xbps-install -S", "download: xbps-install -uyD", "install: xbps-install -uy"]);
        assert_eq!(shown(BackendId::Apk), ["refresh: apk update", "install: apk upgrade --no-progress"]);
        assert_eq!(
            shown(BackendId::Pkg),
            [
                "refresh: pkg update",
                "download: pkg upgrade -U -F -y",
                "download: freebsd-update --not-running-from-cron fetch",
                "install: pkg upgrade -U -y",
                "install: freebsd-update --not-running-from-cron install"
            ]
        );
        assert!(backend(BackendId::Pkg).steps().iter().filter(|s| s.program == FREEBSD_UPDATE).all(|s| s.env == [("PAGER", "cat")]), "freebsd-update's lists never wait in a pager");
        assert_eq!(flatpak_step().display(), "flatpak update -y --noninteractive");
    }

    /// Never a force or overwrite option of the package manager itself.
    #[test]
    fn no_step_forces_anything() {
        for id in BackendId::ALL {
            for s in backend(id).steps() {
                for a in &s.args {
                    let a = a.to_ascii_lowercase();
                    assert!(!a.contains("overwrite") && !a.contains("nodeps") && !a.contains("allow-downgrade") && !a.contains("--force ") && a != "--force", "{id:?}: {a}");
                    if a.contains("force") {
                        assert!(a == "dpkg::options::=--force-confdef" || a == "dpkg::options::=--force-confold", "{a}");
                    }
                }
                assert!(!s.program.contains('/'), "programs come from the safe folders only");
            }
        }
    }

    #[test]
    fn only_refresh_download_and_build_are_cancellable() {
        let yes = [StepKind::Refresh, StepKind::Download, StepKind::FlakeUpdate, StepKind::ChannelUpdate, StepKind::Build, StepKind::Flatpak, StepKind::Aur];
        for k in StepKind::ALL {
            assert_eq!(k.cancellable(), yes.contains(&k), "{k:?}");
            assert_eq!(StepKind::from_id(k.id()), Some(k));
        }
        for id in BackendId::ALL {
            assert_eq!(BackendId::from_id(id.id()), Some(id));
            if let Some(last) = backend(id).steps().last() {
                assert!(!last.kind.cancellable(), "{id:?}: the install step is never cancelled");
            }
        }
    }

    #[test]
    fn zypper_reruns_once_after_updating_itself() {
        let z = Zypper { dup: true };
        let install = z.install();
        let mut t = Tracker::default();
        assert_eq!(z.after(&install, Some(ZYPPER_RESTART_NEEDED), &t), Next::Insert(install.clone()));
        t.restarts = 1;
        assert_eq!(z.after(&install, Some(ZYPPER_RESTART_NEEDED), &t), Next::Continue);
        assert_eq!(z.after(&install, Some(ZYPPER_REBOOT_NEEDED), &t), Next::Continue);
        assert_eq!(z.after(&install, Some(ZYPPER_LOCKED), &t), Next::Fail(FailureKind::Locked));
    }

    #[test]
    fn classifies_failures() {
        let p = Pacman.failure_patterns();
        assert_eq!(classify_with(p, &[":: Synchronizing package databases...", "error: failed to synchronize all databases (unable to lock database)"]), FailureKind::Locked);
        assert_eq!(classify_with(p, &["error: failed to commit transaction (conflicting files)", "foo: /usr/bin/foo exists in filesystem"]), FailureKind::Conflict);
        assert_eq!(classify_with(p, &["error: something odd"]), FailureKind::Other);
        assert_eq!(classify_with(&[], &["curl: (6) Could not resolve host: mirror"]), FailureKind::Network);
        assert!(contains_ci("E: Could not get LOCK /var/lib/dpkg/lock-frontend", "could not get lock"));
    }
}
