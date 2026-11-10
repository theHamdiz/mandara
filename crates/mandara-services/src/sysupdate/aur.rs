//! AUR packages, updated by the installed AUR helper as you.
//!
//! An AUR helper never runs as root (paru and yay refuse to): the job runs
//! it as a rootless step, like Flatpak, without a terminal. Its `PATH`
//! starts with the step's private askpass folder ([`super::askpass`]),
//! where `sudo` is Mandara's wrapper for `sudo -A` and `SUDO_ASKPASS` is
//! Mandara itself, so every `sudo` the helper (or makepkg) starts asks
//! through Mandara's own password dialog. Repository packages are the
//! system step's (pacman, as root); this step only upgrades AUR packages.
//!
//! | Helper | Unattended AUR-only upgrade |
//! |---|---|
//! | paru | `paru -Sua --noconfirm --skipreview --noupgrademenu --sudo sudo --sudoloop --color never` |
//! | yay | `yay -Sua --noconfirm --answerclean None --answerdiff None --answeredit None --answerupgrade None --sudo sudo --sudoloop` |
//! | pikaur | `pikaur -Sua --noconfirm --noedit --nodiff` |
//! | Aura | `aura -Au --noconfirm` |
//! | trizen | `trizen -Sua --noconfirm --noedit --noinfo --nocolors --sudo_command=sudo` |
//! | Shelly | `shelly upgrade aur --no-confirm` (`SHELLY_ELEVATOR=sudo`) |
//!
//! `--sudo sudo` (and trizen's `--sudo_command`, Shelly's
//! `SHELLY_ELEVATOR`) override a configured doas, which has no askpass;
//! `--sudoloop` keeps one approval alive through long builds. Nothing here
//! forces a package (`--overwrite`, `--nodeps`): a conflict stops the
//! step, and the panel offers the terminal for it.
//!
//! Progress comes from what every helper prints: makepkg's `==> Making
//! package: name version`, `==> Retrieving sources...`, `==> Starting
//! build()...`, and pacman's own install lines.

use serde::{Deserialize, Serialize};

use super::parse::{self, Tracker};
use super::plan::{self, Step, StepKind};
use super::{FailureKind, Phase};

/// The AUR helpers Mandara can run, in the order "Automatic" prefers them.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AurHelper {
    Paru,
    Yay,
    Pikaur,
    Aura,
    Trizen,
    /// CachyOS's package manager (repositories, AUR, Flatpak, AppImage).
    Shelly,
}

impl AurHelper {
    /// Preference order: paru, then yay, then the others.
    pub const ALL: [AurHelper; 6] = [AurHelper::Paru, AurHelper::Yay, AurHelper::Pikaur, AurHelper::Aura, AurHelper::Trizen, AurHelper::Shelly];

    /// The program, and the id in settings (`plugins.settings.updates.aur_helper`).
    pub fn id(self) -> &'static str {
        match self {
            AurHelper::Paru => "paru",
            AurHelper::Yay => "yay",
            AurHelper::Pikaur => "pikaur",
            AurHelper::Aura => "aura",
            AurHelper::Trizen => "trizen",
            AurHelper::Shelly => "shelly",
        }
    }

    pub fn from_id(id: &str) -> Option<AurHelper> {
        AurHelper::ALL.into_iter().find(|h| h.id().eq_ignore_ascii_case(id))
    }

    /// Its name (a proper noun, not translated).
    pub fn label(self) -> &'static str {
        match self {
            AurHelper::Aura => "Aura",
            AurHelper::Shelly => "Shelly",
            other => other.id(),
        }
    }

    /// The read-only "which AUR packages are outdated" query (`name old ->
    /// new` lines), or `None` for helpers without one (the AUR's web API
    /// is asked instead, see [`crate::updates`]).
    pub fn check_args(self) -> Option<&'static [&'static str]> {
        match self {
            AurHelper::Paru | AurHelper::Yay | AurHelper::Pikaur | AurHelper::Trizen => Some(&["-Qua"]),
            AurHelper::Aura | AurHelper::Shelly => None,
        }
    }

    /// The fixed arguments of the unattended, AUR-only upgrade.
    pub fn upgrade_args(self) -> &'static [&'static str] {
        match self {
            AurHelper::Paru => &["-Sua", "--noconfirm", "--skipreview", "--noupgrademenu", "--sudo", "sudo", "--sudoloop", "--color", "never"],
            AurHelper::Yay => &["-Sua", "--noconfirm", "--answerclean", "None", "--answerdiff", "None", "--answeredit", "None", "--answerupgrade", "None", "--sudo", "sudo", "--sudoloop"],
            AurHelper::Pikaur => &["-Sua", "--noconfirm", "--noedit", "--nodiff"],
            AurHelper::Aura => &["-Au", "--noconfirm"],
            AurHelper::Trizen => &["-Sua", "--noconfirm", "--noedit", "--noinfo", "--nocolors", "--sudo_command=sudo"],
            AurHelper::Shelly => &["upgrade", "aur", "--no-confirm"],
        }
    }

    /// Extra environment of the upgrade: Shelly would pick doas (which
    /// has no askpass) when both are installed.
    pub fn env(self) -> &'static [(&'static str, &'static str)] {
        match self {
            AurHelper::Shelly => &[("SHELLY_ELEVATOR", "sudo")],
            _ => &[],
        }
    }

    /// The upgrade step, run as you.
    pub fn step(self) -> Step {
        Step::new(StepKind::Aur, self.id(), self.upgrade_args()).env(self.env())
    }

    /// The interactive command the terminal fallback runs.
    pub fn terminal_command(self) -> &'static str {
        match self {
            AurHelper::Paru => "paru -Sua",
            AurHelper::Yay => "yay -Sua",
            AurHelper::Pikaur => "pikaur -Sua",
            AurHelper::Aura => "aura -Au",
            AurHelper::Trizen => "trizen -Sua",
            AurHelper::Shelly => "shelly upgrade aur",
        }
    }
}

/// The helper to use: `preferred` when it is installed, else the first
/// installed one in [`AurHelper::ALL`] order.
pub fn pick(preferred: Option<AurHelper>, installed: impl Fn(AurHelper) -> bool) -> Option<AurHelper> {
    preferred.filter(|&h| installed(h)).or_else(|| AurHelper::ALL.into_iter().find(|&h| installed(h)))
}

/// The installed helper that answers the read-only check (`-Qua`), in
/// preference order.
pub fn checker(installed: impl Fn(AurHelper) -> bool) -> Option<AurHelper> {
    AurHelper::ALL.into_iter().find(|&h| h.check_args().is_some() && installed(h))
}

/// Failure messages of AUR helpers, makepkg and sudo (before the generic
/// ones). The askpass and sudo answers come first: they explain the rest.
const PATTERNS: &[(&str, FailureKind)] = &[
    ("a terminal is required to read the password", FailureKind::NoPrompt),
    ("no askpass program specified", FailureKind::NoPrompt),
    ("mandara's password dialog", FailureKind::NoPrompt),
    ("no password was provided", FailureKind::NotAuthorized),
    ("incorrect password attempt", FailureKind::NotAuthorized),
    ("a password is required", FailureKind::NotAuthorized),
    ("is not in the sudoers file", FailureKind::NotAuthorized),
    ("unable to lock database", FailureKind::Locked),
    ("unresolvable package conflicts", FailureKind::Conflict),
    ("conflicting files", FailureKind::Conflict),
    ("are in conflict", FailureKind::Conflict),
    ("could not satisfy dependencies", FailureKind::Conflict),
    ("could not find all required packages", FailureKind::Conflict),
    ("unknown public key", FailureKind::Keyring),
    ("one or more pgp signatures could not be verified", FailureKind::Keyring),
    ("failed to build", FailureKind::BuildFailed),
    ("packages failed to build", FailureKind::BuildFailed),
    ("error making:", FailureKind::BuildFailed),
    ("a failure occurred in", FailureKind::BuildFailed),
    ("failure while downloading", FailureKind::Network),
    ("failed to download sources", FailureKind::Network),
    ("could not download", FailureKind::Network),
];

/// What a failed AUR step's last lines say.
pub fn classify(tail: &[&str]) -> FailureKind {
    plan::classify_with(PATTERNS, tail)
}

/// Whether a failure of the AUR step can only be finished by a person in a
/// terminal (the panel then offers it).
pub fn needs_terminal(kind: FailureKind) -> bool {
    matches!(kind, FailureKind::Conflict | FailureKind::BuildFailed | FailureKind::Keyring | FailureKind::Other)
}

/// `name-version-release` → `name` (versions never contain `-`).
fn strip_version(full: &str) -> &str {
    let mut cuts = full.rmatch_indices('-');
    match (cuts.next(), cuts.next()) {
        (Some(_), Some((at, _))) => &full[..at],
        _ => full,
    }
}

/// The package a helper's failure line names.
fn failed_package(l: &str) -> Option<&str> {
    // paru: `error: failed to build 'cbonsai-1.4.2-1': …`
    if let Some(rest) = l.strip_prefix("error: failed to build '") {
        return rest.split_once('\'').map(|(full, _)| strip_version(full));
    }
    // paru: `error: packages failed to build: cbonsai-1.4.2-1 …`
    if let Some(rest) = l.strip_prefix("error: packages failed to build:") {
        return rest.split_whitespace().next().map(strip_version);
    }
    // yay: ` -> error making: cbonsai-exit status 4`
    if let Some(rest) = l.strip_prefix("-> error making:") {
        return rest.trim().rsplit_once("-exit status").map(|(name, _)| name.trim());
    }
    None
}

/// Feeds one output line of the AUR step to the tracker.
pub fn parse_line(line: &str, t: &mut Tracker) {
    let l = line.trim();
    if l.is_empty() {
        return;
    }
    // makepkg's own progress, the same under every helper.
    if let Some(rest) = l.strip_prefix("==> ") {
        if let Some(what) = rest.strip_prefix("Making package: ") {
            let name = what.split_whitespace().next().unwrap_or_default();
            set_building(t, name);
            t.phase = Phase::AurBuild;
            t.set_current(name);
        } else if rest.starts_with("Retrieving sources") || rest.starts_with("Downloading ") {
            t.phase = Phase::Downloading;
            let name = t.building.clone();
            t.set_current(&name);
        } else if rest.starts_with("Starting build()") || rest.starts_with("Starting package()") || rest.starts_with("Starting prepare()") || rest.starts_with("Starting pkgver()") {
            t.phase = Phase::AurBuild;
            let name = t.building.clone();
            t.set_current(&name);
        } else if let Some(what) = rest.strip_prefix("Finished making: ") {
            t.built += 1;
            t.build_total = t.build_total.max(t.built);
            t.phase = Phase::AurBuild;
            t.set_current(what.split_whitespace().next().unwrap_or_default());
        } else if rest.starts_with("ERROR:") && t.failed_package.is_none() && !t.building.is_empty() {
            t.failed_package = Some(t.building.clone());
        }
        return;
    }
    // yay: ` -> cbonsai-1.4.2-1 already made -- skipping build`.
    if l.starts_with("-> ") && l.ends_with("already made -- skipping build") {
        t.built += 1;
        t.build_total = t.build_total.max(t.built);
        return;
    }
    if let Some(name) = failed_package(l) {
        if t.failed_package.is_none() && !name.is_empty() {
            t.failed_package = Some(name.to_string());
        }
        return;
    }
    // The upgrade plan: paru's `Aur (3)` table header, yay's `:: 3
    // packages to upgrade/install.` and `AUR Explicit (3): …`.
    if let Some(n) = aur_count(l) {
        t.build_total = n;
        t.planned = true;
        return;
    }
    // A pacman transaction (repository dependencies, or the built
    // packages) counts its own installs.
    if l.starts_with("Package (") || l.starts_with("Packages (") {
        t.installed = 0;
    }
    if l == "there is nothing to do" || l.ends_with(" there is nothing to do") || l == ":: There are no AUR updates" {
        t.nothing_to_do = true;
        return;
    }
    // Installing the built packages (and repository dependencies): pacman.
    let before = t.phase;
    parse::pacman(StepKind::Install, l, t);
    if t.phase == Phase::Refreshing || (t.phase == Phase::Downloading && before != Phase::Downloading) {
        // pacman's section headers inside a build (makedepends) don't
        // change what the step is doing.
        t.phase = before;
    }
}

/// The count of AUR packages to build: `Aur (3)`, `AUR Explicit (3): …`
/// or `:: 3 packages to upgrade/install.`.
fn aur_count(l: &str) -> Option<u32> {
    if let Some(rest) = l.strip_prefix(":: ") {
        let (n, what) = rest.split_once(' ')?;
        return (what.starts_with("package to upgrade") || what.starts_with("packages to upgrade")).then(|| n.parse().ok()).flatten();
    }
    let rest = l.strip_prefix("Aur").or_else(|| l.strip_prefix("AUR"))?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix("Explicit").map_or(rest, str::trim_start);
    rest.strip_prefix('(')?.split_once(')')?.0.trim().parse().ok()
}

fn set_building(t: &mut Tracker, name: &str) {
    if t.building != name {
        t.building.clear();
        t.building.push_str(&name[..name.floor_char_boundary(parse::CURRENT_MAX)]);
    }
}

#[cfg(test)]
mod tests;
