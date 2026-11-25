//! Progress from the package managers' own output, one pure function per
//! family, fed line by line. Tested against output recorded in clean
//! containers of each distribution (`fixtures/`).
//!
//! The parsers never allocate per line except to remember the current
//! item's name (bounded) and the first package that calls for a restart.

use super::Phase;
use super::plan::StepKind;

/// Longest current-item label kept.
pub(crate) const CURRENT_MAX: usize = 120;

/// The running update's progress, as far as the output tells.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tracker {
    pub phase: Phase,
    pub step: Option<StepKind>,
    /// Packages in the transaction (0: not known yet).
    pub total: u32,
    /// Packages downloaded so far, and how many will be (0: unknown).
    pub downloaded: u32,
    pub download_total: u32,
    /// Packages installed, upgraded or removed so far.
    pub installed: u32,
    /// Hooks / triggers run, of how many (0: unknown).
    pub hooks: u32,
    pub hooks_total: u32,
    /// A direct percentage of the current phase (apt, xbps, nix), 0–100.
    pub percent: Option<f32>,
    /// What is being worked on (a package, a repository, a hook).
    pub current: String,
    /// Packages the update changed (installed, upgraded, replaced).
    pub changed: u32,
    /// The package manager said there was nothing to do.
    pub nothing_to_do: bool,
    /// A transaction was planned in this step (xbps prints nothing at all
    /// when up to date).
    pub planned: bool,
    /// xbps must update itself first.
    pub self_update_needed: bool,
    /// The package manager asked for a restart.
    pub reboot: bool,
    /// The first updated package that calls for a restart.
    pub restart_package: Option<String>,
    /// Steps re-run or inserted so far (bounds retries).
    pub restarts: u32,
    /// Non-fatal warnings (a repository that couldn't be refreshed).
    pub warnings: u32,
    /// FreeBSD: the running release is past its end of life
    /// (freebsd-update still fetched what it had, then said so).
    pub end_of_life: bool,
    /// NixOS: derivations built of how many.
    pub built: u32,
    pub build_total: u32,
    /// NixOS: the ids of nix's aggregate "builds" and "copy paths"
    /// activities, whose progress results carry the counts.
    pub nix_builds: u64,
    pub nix_copies: u64,
    /// NixOS: space the garbage collection freed (`15.4 KiB`).
    pub freed: String,
    /// AUR: the package being built (makepkg's `Making package`).
    pub building: String,
    /// AUR: the first package that didn't build.
    pub failed_package: Option<String>,
}

impl Tracker {
    /// A step starts: its default phase, and per-step flags reset.
    pub fn begin_step(&mut self, kind: StepKind) {
        self.step = Some(kind);
        self.phase = kind.phase();
        self.percent = None;
        self.nothing_to_do = false;
        self.planned = false;
        self.self_update_needed = false;
        self.current.clear();
        match kind {
            // Each of these steps prints its own transaction plan.
            StepKind::Download => {
                self.downloaded = 0;
                self.download_total = 0;
                self.total = 0;
            }
            StepKind::Install | StepKind::SelfUpdate => {
                self.installed = 0;
                self.hooks = 0;
                self.hooks_total = 0;
                self.total = 0;
            }
            StepKind::Aur => {
                self.built = 0;
                self.build_total = 0;
                self.installed = 0;
                self.total = 0;
                self.downloaded = 0;
                self.download_total = 0;
                self.hooks = 0;
                self.hooks_total = 0;
                self.building.clear();
                self.failed_package = None;
            }
            StepKind::Build => {
                self.built = 0;
                self.build_total = 0;
                self.downloaded = 0;
                self.download_total = 0;
            }
            _ => {}
        }
    }

    /// Sets the current item (bounded, reusing the buffer).
    pub fn set_current(&mut self, s: &str) {
        let s = s.trim();
        let s = &s[..s.floor_char_boundary(CURRENT_MAX)];
        if self.current != s {
            self.current.clear();
            self.current.push_str(s);
        }
    }

    /// `package` was installed or upgraded.
    pub fn changed_package(&mut self, package: &str) {
        self.changed += 1;
        if self.restart_package.is_none() && super::needs_restart(package) {
            self.restart_package = Some(package.to_string());
        }
    }

    fn phase(&mut self, p: Phase) {
        self.phase = p;
        if p != Phase::Downloading && p != Phase::Refreshing {
            self.percent = None;
        }
    }

    /// Done / total of the current phase, when known.
    pub fn counts(&self) -> Option<(u32, u32)> {
        let (done, total) = match self.phase {
            Phase::Downloading => (self.downloaded, if self.download_total > 0 { self.download_total } else { self.total }),
            Phase::Installing | Phase::Verifying => (self.installed, self.total),
            Phase::Hooks => (self.hooks, self.hooks_total),
            Phase::Building if self.build_total > 0 => (self.built, self.build_total),
            Phase::Building => (self.downloaded, self.download_total),
            Phase::AurBuild => (self.built, self.build_total),
            _ => return None,
        };
        (total > 0).then_some((done.min(total), total))
    }

    /// 0–1 within the current phase, when known.
    pub fn fraction(&self) -> Option<f32> {
        if let Some(p) = self.percent {
            return Some((p / 100.0).clamp(0.0, 1.0));
        }
        self.counts().map(|(d, t)| d as f32 / t as f32)
    }
}

/// `(n/m) rest` → `(n, m, rest)`; pacman pads `( 1/12)`.
fn counted(line: &str) -> Option<(u32, u32, &str)> {
    let rest = line.trim_start().strip_prefix('(')?;
    let (inner, after) = rest.split_once(')')?;
    let (n, m) = inner.split_once('/')?;
    Some((n.trim().parse().ok()?, m.trim().parse().ok()?, after.trim_start()))
}

/// The number after `prefix` at the start of `line` (`Packages (12)`).
fn paren_count(line: &str, prefix: &str) -> Option<u32> {
    line.strip_prefix(prefix)?.trim_start().strip_prefix('(')?.split_once(')')?.0.trim().parse().ok()
}

/// The leading number of `line` (`46 upgraded, …`).
fn leading_number(line: &str) -> Option<u32> {
    let end = line.find(|c: char| !c.is_ascii_digit()).unwrap_or(line.len());
    line[..end].parse().ok()
}

// ── pacman ───────────────────────────────────────────────────────────────

const PACMAN_VERBS: [(&str, bool); 5] = [("upgrading ", true), ("installing ", true), ("reinstalling ", true), ("downgrading ", true), ("removing ", false)];
const PACMAN_CHECKS: [&str; 5] = ["checking keyring", "checking package integrity", "loading package files", "checking for file conflicts", "checking available disk space"];

/// pacman without a terminal (`--noconfirm`, no progress bars): section
/// headers (`:: …`), `Package (N)`, `name downloading...`, `upgrading
/// name...` and `(n/m)` hooks. With a terminal the verbs carry `(n/m)`
/// too; both are read.
pub fn pacman(step: StepKind, line: &str, t: &mut Tracker) {
    let l = line.trim();
    if l == "there is nothing to do" {
        t.nothing_to_do = true;
        return;
    }
    if let Some(n) = paren_count(l, "Packages").or_else(|| paren_count(l, "Package")) {
        t.total = n;
        t.planned = true;
        if step == StepKind::Download {
            t.download_total = n;
        }
        return;
    }
    if let Some(rest) = l.strip_prefix(":: ") {
        if rest.starts_with("Synchronizing package databases") {
            t.phase(Phase::Refreshing);
        } else if rest.starts_with("Retrieving packages") {
            t.phase(Phase::Downloading);
        } else if rest.starts_with("Processing package changes") {
            t.phase(Phase::Installing);
        } else if rest.starts_with("Running post-transaction hooks") || rest.starts_with("Running pre-transaction hooks") {
            t.phase(Phase::Hooks);
            t.hooks = 0;
            t.hooks_total = 0;
        }
        return;
    }
    if let Some(name) = l.strip_suffix(" downloading...") {
        if t.phase == Phase::Downloading {
            t.downloaded += 1;
        }
        t.set_current(name);
        return;
    }
    if PACMAN_CHECKS.iter().any(|c| l.starts_with(c)) {
        t.phase(Phase::Verifying);
        t.set_current(l.trim_end_matches('.'));
        return;
    }
    let (count, body) = match counted(l) {
        Some((n, m, rest)) => (Some((n, m)), rest),
        None => (None, l),
    };
    if t.phase == Phase::Hooks {
        if let Some((n, m)) = count {
            t.hooks = n;
            t.hooks_total = m;
            t.set_current(body.trim_end_matches('.'));
        }
        return;
    }
    for (verb, changes) in PACMAN_VERBS {
        if let Some(rest) = body.strip_prefix(verb) {
            let name = rest.trim_end_matches('.').split_whitespace().next().unwrap_or_default();
            t.phase(Phase::Installing);
            t.installed = count.map_or(t.installed + 1, |(n, _)| n);
            if let Some((_, m)) = count {
                t.total = m;
            }
            t.set_current(name);
            if changes {
                t.changed_package(name);
            }
            return;
        }
    }
    if let Some((n, m)) = count {
        // `(1/8) checking keys in keyring` and the like.
        t.set_current(body);
        if t.phase == Phase::Verifying {
            t.installed = 0;
            let _ = (n, m);
        }
    }
}

// ── apt ──────────────────────────────────────────────────────────────────

/// `dlstatus:` / `pmstatus:` → (item, percent, description). The item may
/// contain colons (`libc6:amd64`): the percent is the first field after it
/// that is a number.
fn apt_status(rest: &str) -> Option<(&str, f32, &str)> {
    let mut cut = 0;
    for (i, field) in rest.split(':').enumerate() {
        let start = cut;
        cut += field.len() + 1;
        if i > 0
            && let Ok(p) = field.parse::<f32>()
        {
            let item = &rest[..start.saturating_sub(1)];
            let desc = rest.get(cut..).unwrap_or_default();
            return Some((item, p, desc));
        }
    }
    None
}

/// `apt-get` with `APT::Status-Fd=1`: `dlstatus:` lines while fetching,
/// `pmstatus:` while dpkg works, plus its plain summary lines.
pub fn apt(step: StepKind, line: &str, t: &mut Tracker) {
    let l = line.trim();
    if let Some(rest) = l.strip_prefix("dlstatus:") {
        if let Some((_, p, desc)) = apt_status(rest) {
            t.percent = Some(p);
            if step == StepKind::Download {
                t.phase(Phase::Downloading);
                // "Retrieving file 7 of 46"
                if let Some((n, m)) = desc.strip_prefix("Retrieving file ").and_then(|d| d.split_whitespace().next().zip(d.split(" of ").nth(1))) {
                    t.downloaded = n.parse::<u32>().unwrap_or(t.downloaded).saturating_sub(1);
                    t.download_total = m.split_whitespace().next().and_then(|m| m.parse().ok()).unwrap_or(t.download_total);
                }
            }
            t.set_current(desc);
        }
        return;
    }
    if let Some(rest) = l.strip_prefix("pmstatus:") {
        if let Some((item, p, desc)) = apt_status(rest) {
            if item == "dpkg-exec" {
                return;
            }
            t.phase(if desc.starts_with("Running") || desc.contains("trigger") { Phase::Hooks } else { Phase::Installing });
            t.percent = Some(p);
            t.set_current(desc);
        }
        return;
    }
    // "46 upgraded, 0 newly installed, 0 to remove and 0 not upgraded."
    if l.contains(" upgraded, ") && l.contains(" newly installed, ") {
        let mut sum = 0;
        for part in l.trim_end_matches('.').split(", ") {
            let part = part.split(" and ").next().unwrap_or(part);
            if let Some(n) = leading_number(part) {
                sum += n;
            }
        }
        t.total = sum;
        t.planned = sum > 0;
        t.nothing_to_do = sum == 0;
        return;
    }
    if let Some(rest) = l.strip_prefix("Unpacking ") {
        let name = rest.split_whitespace().next().unwrap_or_default();
        let name = name.split(':').next().unwrap_or(name);
        t.installed += 1;
        t.changed_package(name);
        return;
    }
    if l.starts_with("Processing triggers for ") {
        t.phase(Phase::Hooks);
        t.set_current(l);
    }
}

// ── dnf / dnf5 ───────────────────────────────────────────────────────────

/// `name-version-release.arch` → `name`.
fn rpm_name(nevra: &str) -> &str {
    let no_arch = nevra.rsplit_once('.').map_or(nevra, |(n, _)| n);
    let mut parts = no_arch.rsplitn(3, '-');
    let (_release, _version) = (parts.next(), parts.next());
    parts.next().unwrap_or(no_arch)
}

const DNF_TRANSACTION: [(&str, bool); 8] =
    [("Upgrading", true), ("Installing", true), ("Reinstalling", true), ("Downgrading", true), ("Replacing", true), ("Removing", false), ("Erasing", false), ("Cleanup", false)];

/// dnf 4 (`(3/14): name.rpm  …`, `  Upgrading : nevra   3/28`) and dnf5
/// (`[ 3/14] name …`, `[ 3/28] Upgrading nevra …`).
pub fn dnf(step: StepKind, line: &str, t: &mut Tracker) {
    let l = line.trim();
    if l == "Nothing to do." || l == "Dependencies resolved.\nNothing to do." {
        t.nothing_to_do = true;
        return;
    }
    // dnf 4 summary: "Upgrade  12 Packages" / "Install   2 Packages".
    for verb in ["Upgrade ", "Install ", "Remove ", "Downgrade ", "Reinstall "] {
        if let Some(rest) = l.strip_prefix(verb)
            && (rest.trim_end().ends_with("Packages") || rest.trim_end().ends_with("Package"))
            && let Some(n) = rest.split_whitespace().next().and_then(|n| n.parse::<u32>().ok())
        {
            t.total += n;
            t.planned = true;
            return;
        }
    }
    // dnf5 summary: "Upgrading:          12 packages".
    for verb in ["Upgrading:", "Installing:", "Removing:", "Downgrading:", "Reinstalling:", "Installing dependencies:", "Installing weak dependencies:"] {
        if let Some(rest) = l.strip_prefix(verb)
            && (rest.trim_end().ends_with("packages") || rest.trim_end().ends_with("package"))
            && let Some(n) = rest.split_whitespace().next().and_then(|n| n.parse::<u32>().ok())
        {
            t.total += n;
            t.planned = true;
            return;
        }
    }
    if let Some(rest) = l.strip_prefix(">>> Running ") {
        t.set_current(rest);
        return;
    }
    if l.starts_with("Updating and loading repositories") {
        t.phase(Phase::Refreshing);
        return;
    }
    if l.starts_with("Downloading Packages") {
        t.phase(Phase::Downloading);
        return;
    }
    if l.starts_with("Running transaction check") || l.starts_with("Running transaction test") || l.starts_with("Verifying") && !l.contains(':') {
        t.phase(Phase::Verifying);
        return;
    }
    // dnf 4 download: "(3/14): foo-1.0-1.fc40.x86_64.rpm   1.2 MB/s | 300 kB  00:00"
    if let Some((n, m, rest)) = counted(l)
        && let Some(file) = rest.strip_prefix(": ").or_else(|| rest.strip_prefix(':'))
    {
        t.phase(Phase::Downloading);
        t.downloaded = n;
        t.download_total = m;
        t.set_current(file.split_whitespace().next().unwrap_or_default());
        return;
    }
    // dnf5: "[ 3/14] foo-1.0-1.fc41.x86_64  100% | …" (download) or
    // "[ 3/28] Upgrading foo-0:1.0-1.fc41.x86_64  100% | …" (transaction).
    if let Some(rest) = l.strip_prefix('[')
        && let Some((inner, after)) = rest.split_once(']')
        && let Some((n, m)) = inner.split_once('/')
        && let (Ok(n), Ok(m)) = (n.trim().parse::<u32>(), m.trim().parse::<u32>())
    {
        let after = after.trim_start();
        let mut words = after.split_whitespace();
        let first = words.next().unwrap_or_default();
        if let Some((_, changes)) = DNF_TRANSACTION.iter().find(|(v, _)| *v == first) {
            let nevra = words.next().unwrap_or_default();
            transaction(t, n, m, first, nevra, *changes);
        } else if first == "Verify" || first == "Verifying" || first == "Prepare" || first == "Running" {
            t.phase(if first == "Running" { Phase::Hooks } else { Phase::Verifying });
            t.set_current(after.split(" 100%").next().unwrap_or(after));
        } else if step == StepKind::Download || t.phase == Phase::Downloading || t.phase == Phase::Refreshing {
            t.phase(Phase::Downloading);
            t.downloaded = n;
            t.download_total = m;
            t.set_current(first);
        }
        return;
    }
    // dnf 4 transaction: "  Upgrading        : foo-1.0-1.fc40.x86_64   3/28"
    if let Some((verb, rest)) = l.split_once(':') {
        let verb = verb.trim();
        let rest = rest.trim();
        let mut words = rest.split_whitespace();
        let (nevra, count) = (words.next().unwrap_or_default(), words.next_back().and_then(|c| c.split_once('/')));
        if let Some((n, m)) = count.and_then(|(n, m)| Some((n.parse::<u32>().ok()?, m.parse::<u32>().ok()?))) {
            if let Some((_, changes)) = DNF_TRANSACTION.iter().find(|(v, _)| *v == verb) {
                transaction(t, n, m, verb, nevra, *changes);
            } else if verb == "Running scriptlet" {
                t.phase(Phase::Installing);
                t.set_current(nevra);
            } else if verb == "Verifying" {
                t.phase(Phase::Verifying);
                t.installed = n;
                t.total = m;
                t.set_current(nevra);
            }
        }
    }
}

/// `name-0:1.0-…` → `name` (the dash before `<digits>:`).
fn epoch_name(nevra: &str) -> Option<&str> {
    nevra.match_indices('-').map(|(i, _)| i).find(|&i| {
        let rest = &nevra[i + 1..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        digits > 0 && rest.as_bytes().get(digits) == Some(&b':')
    }).map(|i| &nevra[..i])
}

fn transaction(t: &mut Tracker, n: u32, m: u32, verb: &str, nevra: &str, changes: bool) {
    t.phase(Phase::Installing);
    t.installed = n;
    t.total = m;
    // dnf5 writes `name-0:1.0-1.fc41.x86_64` (with an epoch), cut to the
    // column width; dnf 4 `name-1.0-1.fc40.x86_64`.
    let name = epoch_name(nevra).unwrap_or_else(|| rpm_name(nevra));
    t.set_current(nevra);
    if changes && verb != "Cleanup" {
        t.changed_package(name);
    }
}

// ── zypper ───────────────────────────────────────────────────────────────

/// zypper `--non-interactive` plain output.
pub fn zypper(_step: StepKind, line: &str, t: &mut Tracker) {
    let l = line.trim();
    if l == "Nothing to do." {
        t.nothing_to_do = true;
        return;
    }
    if let Some(name) = l.strip_prefix("Retrieving repository '").and_then(|r| r.split_once('\'')).map(|(n, _)| n) {
        t.phase(Phase::Refreshing);
        t.set_current(name);
        return;
    }
    if l.starts_with("Warning: Skipping repository") || l.starts_with("Skipping repository") {
        t.warnings += 1;
        return;
    }
    // "1 package to upgrade, 11 new, 1 to remove." (removals download
    // nothing).
    if leading_number(l).is_some() && l.ends_with('.') && (l.contains(" to upgrade") || l.contains(" new") || l.contains(" to remove") || l.contains(" to downgrade") || l.contains(" to reinstall")) {
        let (mut sum, mut fetched) = (0, 0);
        for part in l.trim_end_matches('.').split(", ") {
            let part = part.trim();
            if let Some(n) = leading_number(part) {
                sum += n;
                if !part.ends_with("to remove") {
                    fetched += n;
                }
            }
        }
        if sum > 0 {
            t.total = sum;
            t.planned = true;
            t.download_total = fetched;
        }
        return;
    }
    // "Preloading: libcom_err2-1.47.4-2.1.x86_64.rpm [done]" (parallel
    // download), "In cache libcurl4-8.23.0-3.1.x86_64.rpm (11/12),  586.6 KiB".
    if let Some(rest) = l.strip_prefix("Preloading: ") {
        t.phase(Phase::Downloading);
        if rest.ends_with("[done]") {
            t.downloaded += 1;
        }
        t.set_current(rest.split_whitespace().next().unwrap_or_default());
        return;
    }
    if let Some(rest) = l.strip_prefix("In cache ") {
        if let Some((n, m)) = rest.rsplit_once(" (").and_then(|(_, c)| c.split_once(')')).and_then(|(c, _)| c.split_once('/')).and_then(|(n, m)| Some((n.parse::<u32>().ok()?, m.parse::<u32>().ok()?))) {
            t.phase(Phase::Downloading);
            t.downloaded = n;
            t.download_total = m;
        }
        t.set_current(rest.split_whitespace().next().unwrap_or_default());
        return;
    }
    // "Retrieving: bash-5.2.37-1.1.x86_64 (openSUSE-Tumbleweed-Oss) (3/23),   1.2 MiB"
    // or "Retrieving package bash-5.2.37-1.1.x86_64 (3/23),   1.2 MiB"
    if let Some(rest) = l.strip_prefix("Retrieving: ").or_else(|| l.strip_prefix("Retrieving package ")) {
        t.phase(Phase::Downloading);
        if let Some((n, m)) = rest.rsplit_once(" (").and_then(|(_, c)| c.split_once(')')).and_then(|(c, _)| c.split_once('/')).and_then(|(n, m)| Some((n.parse::<u32>().ok()?, m.parse::<u32>().ok()?))) {
            t.downloaded = n;
            t.download_total = m;
        } else {
            t.downloaded += 1;
        }
        t.set_current(rest.split_whitespace().next().unwrap_or_default());
        return;
    }
    if l.starts_with("Checking for file conflicts") {
        t.phase(Phase::Verifying);
        return;
    }
    if l.starts_with("Executing %posttrans") || l.starts_with("Running post-transaction") || l.starts_with("Executing post-transaction") {
        t.phase(Phase::Hooks);
        return;
    }
    if l.contains("reboot is required") || l.contains("core libraries or services have been updated") {
        t.reboot = true;
        return;
    }
    // "(3/23) Installing: bash-5.2.37-1.1.x86_64 .....[done]"
    if let Some((n, m, rest)) = counted(l) {
        for (verb, changes) in [("Installing: ", true), ("Removing: ", false), ("Removing ", false), ("Installing ", true)] {
            if let Some(nevra) = rest.strip_prefix(verb) {
                let nevra = nevra.split_whitespace().next().unwrap_or_default();
                t.phase(Phase::Installing);
                t.installed = n;
                t.total = m;
                t.set_current(nevra);
                if changes {
                    t.changed_package(rpm_name(nevra));
                }
                return;
            }
        }
    }
}

// ── xbps ─────────────────────────────────────────────────────────────────

/// `glibc-2.41_1` → `glibc`.
fn xbps_name(pkgver: &str) -> &str {
    pkgver.rsplit_once('-').map_or(pkgver, |(n, _)| n)
}

/// `xbps-install` (no terminal: one line per progress update).
pub fn xbps(step: StepKind, line: &str, t: &mut Tracker) {
    let l = line.trim();
    if l.contains("package must be updated") {
        t.self_update_needed = true;
        return;
    }
    // "68 packages will be downloaded:" / "4 packages will be updated:"
    if let Some((n, what)) = l.split_once(" will be ").and_then(|(n, w)| Some((leading_number(n)?, w))) {
        t.planned = true;
        if what.starts_with("downloaded") {
            t.download_total = n;
        } else if what.starts_with("installed") || what.starts_with("updated") || what.starts_with("removed") || what.starts_with("reinstalled") || what.starts_with("downgraded") {
            // Each listing comes once per transaction; sum them.
            t.total += n;
        }
        return;
    }
    if let Some(stage) = l.strip_prefix("[*] ") {
        t.phase(match stage {
            s if s.starts_with("Downloading") => Phase::Downloading,
            s if s.starts_with("Verifying") || s.starts_with("Collecting") => Phase::Verifying,
            s if s.starts_with("Unpacking") => Phase::Installing,
            s if s.starts_with("Configuring") => Phase::Hooks,
            s if s.starts_with("Updating repository") => Phase::Refreshing,
            _ => t.phase,
        });
        if t.phase == Phase::Hooks {
            t.hooks_total = t.total;
        }
        return;
    }
    // "glibc-2.41_1.x86_64.xbps: [8956KB 85%] 204KB/s ETA: 00m06s"
    // "glibc-2.41_1.x86_64.xbps: 8956KB [avg rate: 270KB/s]" (finished)
    if let Some((file, rest)) = l.split_once(".xbps: ") {
        let pkg = file.rsplit_once('.').map_or(file, |(p, _)| p);
        t.phase(Phase::Downloading);
        if rest.contains("[avg rate:") {
            t.downloaded += 1;
            t.percent = None;
        } else if let Some(p) = rest.split_whitespace().nth(1).and_then(|p| p.strip_suffix("%]")).and_then(|p| p.parse::<f32>().ok()) {
            // One file's percent: shown as the item, the bar follows files.
            let _ = p;
        }
        t.set_current(pkg);
        return;
    }
    if let Some((pkgver, what)) = l.split_once(": ") {
        if what.starts_with("unpacking") {
            t.phase(Phase::Installing);
            t.installed += 1;
            t.set_current(pkgver);
            t.changed_package(xbps_name(pkgver));
        } else if what.starts_with("updating to") {
            t.set_current(pkgver);
        } else if what.starts_with("removing") && !what.starts_with("removing obsolete") && !what.starts_with("removed") {
            t.phase(Phase::Installing);
            t.installed += 1;
            t.set_current(pkgver);
        } else if what.starts_with("configuring") {
            t.phase(Phase::Hooks);
            t.hooks += 1;
            t.set_current(pkgver);
        } else if what.starts_with("verifying") || what.starts_with("collecting") {
            t.phase(Phase::Verifying);
            t.set_current(pkgver);
        }
    }
    let _ = step;
}

// ── apk ──────────────────────────────────────────────────────────────────

/// `apk upgrade --no-progress`.
pub fn apk(step: StepKind, line: &str, t: &mut Tracker) {
    let l = line.trim();
    if let Some(url) = l.strip_prefix("fetch ") {
        t.phase(Phase::Refreshing);
        t.set_current(url);
        return;
    }
    if l.starts_with("Upgrading critical system libraries") || l.starts_with("Continuing the upgrade transaction") {
        t.set_current(l.trim_end_matches(':'));
        return;
    }
    if let Some(name) = l.strip_prefix("Executing ") {
        if name.ends_with(".trigger") {
            t.phase(Phase::Hooks);
        }
        t.set_current(name);
        return;
    }
    if let Some((n, m, rest)) = counted(l) {
        let mut words = rest.split_whitespace();
        let verb = words.next().unwrap_or_default();
        let name = words.next().unwrap_or_default();
        t.phase(Phase::Installing);
        t.installed = n;
        t.total = m;
        t.planned = true;
        t.set_current(name);
        if matches!(verb, "Upgrading" | "Installing" | "Replacing" | "Downgrading" | "Reinstalling") {
            t.changed_package(name);
        }
        return;
    }
    if l.starts_with("OK: ") && step == StepKind::Install && !t.planned {
        t.nothing_to_do = true;
    }
}

// ── pkg (FreeBSD) ────────────────────────────────────────────────────────

/// pkg's job verbs: (`[n/m] <verb> …`, whether it changes a package).
const PKG_VERBS: [(&str, bool); 5] = [("Upgrading ", true), ("Installing ", true), ("Reinstalling ", true), ("Downgrading ", true), ("Deinstalling ", false)];
/// Kernel modules from packages (`drm-61-kmod`): a restart loads them.
const KMOD_SUFFIX: &str = "-kmod";
/// A packaged base's kernel (`FreeBSD-kernel-generic`; not its `-dbg`
/// symbols): a restart boots it.
const BASE_KERNEL_PREFIX: &str = "FreeBSD-kernel-";
const DEBUG_SUFFIX: &str = "-dbg";

/// Whether updating pkg package `name` needs a restart to take effect.
fn pkg_needs_restart(name: &str) -> bool {
    name.ends_with(KMOD_SUFFIX) || (name.starts_with(BASE_KERNEL_PREFIX) && !name.ends_with(DEBUG_SUFFIX))
}

/// `[ 3/12] rest` → `(3, 12, rest)`: pkg's job counter (padded to the
/// width of the total).
fn bracketed(line: &str) -> Option<(u32, u32, &str)> {
    let rest = line.strip_prefix('[')?;
    let (inner, after) = rest.split_once(']')?;
    let (n, m) = inner.split_once('/')?;
    Some((n.trim().parse().ok()?, m.trim().parse().ok()?, after.trim_start()))
}

/// `name-1.2.3_1` → `name` (pkg versions never contain `-`).
fn pkg_name(pkgver: &str) -> &str {
    pkgver.rsplit_once('-').map_or(pkgver, |(n, _)| n)
}

/// `pkg update` and `pkg upgrade` without a terminal: one line per job
/// (`[n/m] Fetching name-1.0: .......... done`, `[n/m] Upgrading name from
/// 1.0 to 1.1...`), the summary (`The following N package(s) will be
/// affected`) and the catalogue's own lines.
pub fn pkg(step: StepKind, line: &str, t: &mut Tracker) {
    let l = line.trim();
    if l == "Your packages are up to date." {
        t.nothing_to_do = true;
        return;
    }
    if let Some(repo) = l.strip_prefix("Updating ").and_then(|r| r.strip_suffix(" repository catalogue...")) {
        t.phase(Phase::Refreshing);
        t.set_current(repo);
        return;
    }
    // The catalogue's own files and entries: the repository stays the
    // current item.
    if l.starts_with("Processing entries") || l.starts_with("Fetching meta.conf") || l.starts_with("Fetching data.pkg") || l.starts_with("Fetching packagesite") {
        return;
    }
    if l.starts_with("Checking for upgrades") || l.starts_with("Processing candidates") {
        t.set_current(l.split(':').next().unwrap_or(l));
        return;
    }
    // "The following 12 package(s) will be affected (of 0 checked):"
    if let Some(n) = l.strip_prefix("The following ").and_then(|r| r.split_once(' ')).filter(|(_, r)| r.starts_with("package(s) will be affected")).and_then(|(n, _)| n.parse::<u32>().ok()) {
        t.total = n;
        t.planned = true;
        if step == StepKind::Download {
            t.download_total = n;
        }
        return;
    }
    if l.starts_with("Checking integrity...") {
        t.phase(Phase::Verifying);
        t.set_current(l.trim_end_matches(|c: char| c != '.').trim_end_matches('.'));
        return;
    }
    if let Some(trigger) = l.strip_prefix("==> Running trigger: ") {
        t.phase(Phase::Hooks);
        t.hooks += 1;
        t.set_current(trigger);
        return;
    }
    let Some((n, m, body)) = bracketed(l) else { return };
    if let Some(item) = body.strip_prefix("Fetching ") {
        let name = item.split(':').next().unwrap_or(item);
        t.phase(Phase::Downloading);
        t.download_total = m;
        // The line arrives once the file is in (`… done`).
        t.downloaded = if body.ends_with("done") { n } else { n.saturating_sub(1) };
        t.set_current(name);
        return;
    }
    if body.starts_with("Extracting ") || body.starts_with("Deleting files for ") {
        return;
    }
    for (verb, changes) in PKG_VERBS {
        if let Some(rest) = body.strip_prefix(verb) {
            // "Upgrading curl from 8.9.1 to 8.10.1..." or "Installing curl-8.10.1..."
            let rest = rest.trim_end_matches('.');
            let name = match rest.split_once(" from ") {
                Some((name, _)) => name,
                None => pkg_name(rest),
            };
            t.phase(Phase::Installing);
            t.installed = n;
            t.total = m;
            t.set_current(rest);
            if changes {
                t.changed_package(name);
                if t.restart_package.is_none() && pkg_needs_restart(name) {
                    t.restart_package = Some(name.to_string());
                }
            }
            return;
        }
    }
}

// ── freebsd-update ───────────────────────────────────────────────────────

/// Where the kernel lives: a fetched update that changes it needs a
/// restart.
const FREEBSD_KERNEL_DIR: &str = "/boot/kernel/";
/// What freebsd-update prints while it installs.
const FREEBSD_INSTALLING: &str = "Installing updates...";

/// `freebsd-update fetch` / `install` (`--not-running-from-cron`,
/// `PAGER=cat`): its progress lines (`Fetching 42 patches.....10....20.
/// done.`), the file lists of the fetched update and its "nothing to do"
/// answers. A system freebsd-update doesn't manage (not a -RELEASE, or a
/// packaged base) counts as nothing to do, with a warning.
pub fn freebsd_update(step: StepKind, line: &str, t: &mut Tracker) {
    let l = line.trim();
    if l.starts_with("No updates needed to update system to") || l == "No updates are available to install." {
        t.nothing_to_do = true;
        return;
    }
    if l.starts_with("Cannot upgrade from a version that is not a release") || l.starts_with("freebsd-update: Cannot upgrade from a version that is not a release") || l.starts_with("freebsd-update is incompatible with the use of packaged base") {
        t.nothing_to_do = true;
        // `install` repeats a packaged base's refusal: one warning.
        if step != StepKind::Install {
            t.warnings += 1;
        }
        return;
    }
    if l.contains("HAS PASSED ITS END-OF-LIFE DATE") {
        t.end_of_life = true;
        t.warnings += 1;
        return;
    }
    if l.starts_with("Kernel updates have been installed.") {
        t.reboot = true;
        return;
    }
    if l.starts_with("Looking up ") || l.starts_with("Fetching metadata") || l.starts_with("Applying metadata patches") || l.starts_with("Fetching public key") {
        t.phase(Phase::Refreshing);
        t.set_current(l.split("...").next().unwrap_or(l));
        return;
    }
    if l.starts_with("Inspecting system") || l.starts_with("Preparing to download files") {
        t.phase(Phase::Verifying);
        t.set_current(l.split("...").next().unwrap_or(l));
        return;
    }
    // "Fetching 42 patches.....10....20....30....40. done." /
    // "Fetching 3 files... done." (the line ends when they are in).
    if let Some(rest) = l.strip_prefix("Fetching ")
        && let Some((n, what)) = rest.split_once(' ')
        && let Ok(n) = n.parse::<u32>()
        && (what.starts_with("patches") || what.starts_with("files"))
    {
        t.phase(Phase::Downloading);
        t.downloaded = n;
        t.download_total = n;
        t.set_current(&rest[..rest.find('.').unwrap_or(rest.len())]);
        return;
    }
    if l.starts_with("Applying patches") {
        t.set_current(l.split("...").next().unwrap_or(l));
        return;
    }
    if l.starts_with("The following files will be") {
        t.planned = true;
        return;
    }
    if l.starts_with('/') && step == StepKind::Download && t.planned {
        // One line per file of the fetched update.
        t.total += 1;
        if l.starts_with(FREEBSD_KERNEL_DIR) {
            t.reboot = true;
        }
        return;
    }
    if l.starts_with("Creating snapshot of existing boot environment") {
        t.phase(Phase::Verifying);
        t.set_current(l.split("...").next().unwrap_or(l));
        return;
    }
    if l.starts_with(FREEBSD_INSTALLING) {
        t.phase(Phase::Installing);
        t.set_current(FREEBSD_INSTALLING.trim_end_matches('.'));
        if l.ends_with("done.") {
            t.changed += 1;
        }
        return;
    }
    // certctl's lines may come between "Installing updates..." and its
    // " done.".
    if l == "done." && step == StepKind::Install && t.current == FREEBSD_INSTALLING.trim_end_matches('.') {
        t.changed += 1;
    }
}

#[cfg(test)]
mod tests;
