//! NixOS: generations instead of package updates.
//!
//! Read-only, as the user: the system generations
//! (`/nix/var/nix/profiles/system-<N>-link`, with their date, NixOS
//! version, kernel and configuration revision, which is current and which
//! booted), home-manager generations, the flake's inputs before and after
//! an update (`flake.lock`), and what changed between two generations
//! (`nix store diff-closures`, closure sizes from `nix path-info -S`).
//!
//! As root, through the helper, only the fixed actions of [`NixAction`]:
//! update (flake inputs or channels) and rebuild, roll back, switch to an
//! existing generation, and clean up old generations. The flake is always
//! the system's own (`/etc/nixos/flake.nix`, followed if it is a link),
//! never a reference passed in; generation numbers must exist; the current
//! and the booted generation are never deleted.
//!
//! Builds run as `nix build --log-format internal-json`, whose structured
//! progress (paths copied, derivations built) drives the progress bar;
//! `nixos-rebuild` then switches to the built system the official way
//! (nothing is rebuilt: it finds the result in the store).

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::parse::Tracker;
use super::plan::{BackendId, Step, StepKind, SystemBackend};
use super::{FailureKind, Phase};

/// Generations listed at most (a system rarely keeps more).
pub const MAX_GENERATIONS: usize = 200;
/// Bounds of the clean-up actions.
pub const KEEP_RANGE: std::ops::RangeInclusive<u32> = 1..=100;
pub const DAYS_RANGE: std::ops::RangeInclusive<u32> = 1..=3650;
/// Package changes kept from one diff.
pub const MAX_CHANGES: usize = 2000;
/// Flake inputs compared at most.
pub const MAX_INPUTS: usize = 64;
/// The experimental features every `nix` command here needs.
const NIX_FEATURES: [&str; 2] = ["--extra-experimental-features", "nix-command flakes"];
/// NixOS's default `nix.nixPath`: root's channels and the system
/// configuration (pkexec clears the environment, so it is set here).
const NIX_PATH: &[(&str, &str)] =
    &[("NIX_PATH", "nixpkgs=/nix/var/nix/profiles/per-user/root/channels/nixos:nixos-config=/etc/nixos/configuration.nix:/nix/var/nix/profiles/per-user/root/channels")];
/// Runs the switch in its own systemd unit, as `nixos-rebuild` does, so it
/// finishes even if the session (and this helper) goes away meanwhile.
const SYSTEMD_RUN: [&str; 7] = ["--collect", "--no-ask-password", "--pipe", "--quiet", "--service-type=exec", "--unit=mandara-switch-to-configuration", "--wait"];
const SECS_PER_DAY: i64 = 86_400;

/// The files the NixOS side reads (a fixture root in tests).
#[derive(Clone, Debug)]
pub struct NixPaths {
    pub profiles: PathBuf,
    pub booted: PathBuf,
    pub etc_nixos: PathBuf,
    pub root_channels: PathBuf,
    pub hostname: PathBuf,
    /// Programs run as root must be root-owned files in `/nix/store`
    /// (always on for the real system; off only for fixture trees in tests
    /// and the sandbox, which never run anything as root).
    pub trusted_store: bool,
}

impl NixPaths {
    pub fn system() -> NixPaths {
        NixPaths { trusted_store: true, ..NixPaths::under(Path::new("/")) }
    }

    pub fn under(root: &Path) -> NixPaths {
        NixPaths {
            profiles: root.join("nix/var/nix/profiles"),
            booted: root.join("run/booted-system"),
            etc_nixos: root.join("etc/nixos"),
            root_channels: root.join("nix/var/nix/profiles/per-user/root/channels"),
            hostname: root.join("proc/sys/kernel/hostname"),
            trusted_store: false,
        }
    }

    pub fn system_profile(&self) -> PathBuf {
        self.profiles.join("system")
    }

    pub fn generation_link(&self, n: u32) -> PathBuf {
        self.profiles.join(format!("system-{n}-link"))
    }
}

// ── Actions ──────────────────────────────────────────────────────────────

/// What `nixos-rebuild` does with the new generation.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RebuildMode {
    /// Build, switch now and boot it next time.
    Switch,
    /// Build and boot it next time; the running system stays.
    Boot,
    /// Build and switch now, without making it the boot default.
    Test,
    /// Only build it.
    Build,
}

impl RebuildMode {
    pub const ALL: [RebuildMode; 4] = [RebuildMode::Switch, RebuildMode::Boot, RebuildMode::Test, RebuildMode::Build];

    pub fn id(self) -> &'static str {
        match self {
            RebuildMode::Switch => "switch",
            RebuildMode::Boot => "boot",
            RebuildMode::Test => "test",
            RebuildMode::Build => "build",
        }
    }

    pub fn from_id(id: &str) -> Option<RebuildMode> {
        RebuildMode::ALL.into_iter().find(|m| m.id() == id)
    }
}

/// Old generations to delete.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Cleanup {
    /// Keep the newest N (and always the current and the booted one).
    KeepLatest(u32),
    /// Delete those older than N days (never the current or booted one).
    OlderThanDays(u32),
}

/// The NixOS actions the helper accepts.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NixAction {
    /// Update the flake inputs (or channels), then rebuild.
    Update(RebuildMode),
    /// Rebuild the configuration as it is.
    Rebuild(RebuildMode),
    Rollback,
    /// Switch to an existing generation.
    SwitchTo(u32),
    Clean(Cleanup),
}

/// A number in `range`: ASCII digits only (no sign, no spaces), at most
/// nine of them.
fn number(word: &str, range: std::ops::RangeInclusive<u32>) -> Result<u32, String> {
    if word.is_empty() || word.len() > 9 || !word.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("not a number: {word:?}"));
    }
    let n: u32 = word.parse().map_err(|_| format!("not a number: {word:?}"))?;
    range.contains(&n).then_some(n).ok_or_else(|| format!("{n} is out of range"))
}

impl NixAction {
    pub const USAGE: &'static str = "update|rebuild switch|boot|test|build | rollback | switch-to <generation> | clean keep <1-100> | clean older-than <1-3650>";

    pub fn parse(words: &[&str]) -> Result<NixAction, String> {
        let usage = || format!("usage: mandara system-update nixos {}", NixAction::USAGE);
        match words {
            ["update", mode] => RebuildMode::from_id(mode).map(NixAction::Update).ok_or_else(usage),
            ["rebuild", mode] => RebuildMode::from_id(mode).map(NixAction::Rebuild).ok_or_else(usage),
            ["rollback"] => Ok(NixAction::Rollback),
            ["switch-to", n] => number(n, 1..=u32::MAX / 2).map(NixAction::SwitchTo),
            ["clean", "keep", n] => number(n, KEEP_RANGE).map(|n| NixAction::Clean(Cleanup::KeepLatest(n))),
            ["clean", "older-than", n] => number(n, DAYS_RANGE).map(|n| NixAction::Clean(Cleanup::OlderThanDays(n))),
            _ => Err(usage()),
        }
    }

    /// The helper's arguments for this action (after `system-update`).
    pub fn args(self) -> Vec<String> {
        let mut out = vec!["nixos".to_string()];
        match self {
            NixAction::Update(m) => out.extend(["update".into(), m.id().into()]),
            NixAction::Rebuild(m) => out.extend(["rebuild".into(), m.id().into()]),
            NixAction::Rollback => out.push("rollback".into()),
            NixAction::SwitchTo(n) => out.extend(["switch-to".into(), n.to_string()]),
            NixAction::Clean(Cleanup::KeepLatest(n)) => out.extend(["clean".into(), "keep".into(), n.to_string()]),
            NixAction::Clean(Cleanup::OlderThanDays(n)) => out.extend(["clean".into(), "older-than".into(), n.to_string()]),
        }
        out
    }
}

// ── The system's setup ───────────────────────────────────────────────────

/// How this NixOS system is configured.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Setup {
    /// `/etc/nixos/flake.nix` (followed when it is a link): its folder,
    /// whether root owns it (else its owner updates the inputs), and the
    /// configuration's name (the host name).
    Flake { dir: PathBuf, root_owned: bool, host: String },
    /// Channels: root's `nixos` channel, when one is set.
    Channels { channel: Option<String> },
}

/// A host name as `nixosConfigurations.<host>` uses it.
fn valid_host(h: &str) -> bool {
    !h.is_empty() && h.len() <= 64 && h.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') && !h.starts_with('-')
}

pub fn setup(paths: &NixPaths) -> Setup {
    let flake = paths.etc_nixos.join("flake.nix");
    if let Ok(real) = std::fs::canonicalize(&flake)
        && let Some(dir) = real.parent()
    {
        let host = std::fs::read_to_string(&paths.hostname).map(|h| h.trim().to_string()).unwrap_or_default();
        return Setup::Flake { dir: dir.to_path_buf(), root_owned: mandara_config::admin::trusted_root_file(dir), host };
    }
    let channel = std::fs::read_link(paths.root_channels.join("nixos")).ok().or_else(|| std::fs::canonicalize(paths.root_channels.join("nixos")).ok());
    Setup::Channels { channel: channel.and_then(|p| p.parent().and_then(|p| p.file_name()).map(|n| store_name(&n.to_string_lossy()).to_string())) }
}

/// `/nix/store/<hash>-<name>` → `<name>`.
pub fn store_name(base: &str) -> &str {
    match base.split_once('-') {
        Some((hash, name)) if hash.len() == 32 => name,
        _ => base,
    }
}

// ── Generations ──────────────────────────────────────────────────────────

/// One system generation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Generation {
    pub number: u32,
    /// Unix seconds (the profile link's time).
    pub date: i64,
    pub nixos_version: String,
    pub kernel: String,
    pub current: bool,
    pub booted: bool,
    /// `system.configurationRevision`, when set (filled in lazily).
    pub revision: Option<String>,
    pub store: PathBuf,
}

/// `system-42-link` → 42.
fn link_number(name: &str, profile: &str) -> Option<u32> {
    let n = name.strip_prefix(profile)?.strip_prefix('-')?.strip_suffix("-link")?;
    number(n, 0..=u32::MAX).ok()
}

/// The kernel version of a generation (`kernel` → `…-linux-6.6.30/bzImage`).
fn kernel_of(store: &Path) -> String {
    let Ok(target) = std::fs::read_link(store.join("kernel")) else { return String::new() };
    let pkg = target.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name = store_name(&pkg);
    name.strip_prefix("linux-").unwrap_or(name).to_string()
}

/// The system generations, newest first (at most [`MAX_GENERATIONS`]).
pub fn generations(paths: &NixPaths) -> Vec<Generation> {
    use std::os::unix::fs::MetadataExt as _;
    let current = std::fs::read_link(paths.system_profile()).ok().and_then(|t| t.file_name().and_then(|n| link_number(&n.to_string_lossy(), "system")));
    let booted = std::fs::canonicalize(&paths.booted).ok();
    let mut out: Vec<Generation> = std::fs::read_dir(&paths.profiles)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let number = link_number(&e.file_name().to_string_lossy(), "system")?;
            let store = std::fs::canonicalize(e.path()).unwrap_or_else(|_| std::fs::read_link(e.path()).unwrap_or_default());
            let date = std::fs::symlink_metadata(e.path()).map(|m| m.mtime()).unwrap_or_default();
            Some(Generation {
                number,
                date,
                nixos_version: std::fs::read_to_string(store.join("nixos-version")).map(|s| s.trim().to_string()).unwrap_or_default(),
                kernel: kernel_of(&store),
                current: current == Some(number),
                booted: booted.as_deref() == Some(store.as_path()),
                revision: None,
                store,
            })
        })
        .collect();
    out.sort_unstable_by_key(|g| std::cmp::Reverse(g.number));
    out.truncate(MAX_GENERATIONS);
    out
}

/// The configuration revision of a generation, from its own
/// `nixos-version` (run as the user, with a short timeout).
pub fn revision(store: &Path) -> Option<String> {
    let exe = store.join("sw/bin/nixos-version");
    let out = std::process::Command::new(exe).arg("--configuration-revision").stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    let rev = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !rev.is_empty() && rev.len() <= 128).then_some(rev)
}

/// `nixos-rebuild list-generations --json`, where available (used when
/// the profile folder can't be read).
pub fn parse_list_generations_json(text: &str) -> Vec<Generation> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Row {
        generation: u32,
        #[serde(default)]
        date: String,
        #[serde(default)]
        nixos_version: String,
        #[serde(default)]
        kernel_version: String,
        #[serde(default)]
        configuration_revision: String,
        #[serde(default)]
        current: bool,
    }
    let rows: Vec<Row> = serde_json::from_str(text).unwrap_or_default();
    let mut out: Vec<Generation> = rows
        .into_iter()
        .take(MAX_GENERATIONS)
        .map(|r| Generation {
            number: r.generation,
            date: chrono::NaiveDateTime::parse_from_str(&r.date, "%Y-%m-%d %H:%M:%S").map(|d| d.and_utc().timestamp()).unwrap_or_default(),
            nixos_version: r.nixos_version,
            kernel: r.kernel_version,
            current: r.current,
            booted: false,
            revision: Some(r.configuration_revision).filter(|s| !s.is_empty()),
            store: PathBuf::new(),
        })
        .collect();
    out.sort_unstable_by_key(|g| std::cmp::Reverse(g.number));
    out
}

/// One home-manager generation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeGeneration {
    pub number: u32,
    pub date: i64,
    pub current: bool,
}

/// Where home-manager keeps the user's profile links.
pub fn home_profile_dirs(home: &Path, user: &str) -> [PathBuf; 2] {
    [home.join(".local/state/nix/profiles"), PathBuf::from("/nix/var/nix/profiles/per-user").join(user)]
}

/// Home-manager generations, newest first (empty without home-manager).
pub fn home_generations(dirs: &[PathBuf]) -> Vec<HomeGeneration> {
    use std::os::unix::fs::MetadataExt as _;
    for dir in dirs {
        let current = std::fs::read_link(dir.join("home-manager")).ok().and_then(|t| t.file_name().and_then(|n| link_number(&n.to_string_lossy(), "home-manager")));
        let mut out: Vec<HomeGeneration> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let number = link_number(&e.file_name().to_string_lossy(), "home-manager")?;
                let date = std::fs::symlink_metadata(e.path()).map(|m| m.mtime()).unwrap_or_default();
                Some(HomeGeneration { number, date, current: current == Some(number) })
            })
            .collect();
        if !out.is_empty() {
            out.sort_unstable_by_key(|g| std::cmp::Reverse(g.number));
            out.truncate(MAX_GENERATIONS);
            return out;
        }
    }
    Vec::new()
}

/// The generation numbers a clean-up deletes: never the current or the
/// booted one (`now`: unix seconds).
pub fn to_delete(gens: &[Generation], cleanup: Cleanup, now: i64) -> Vec<u32> {
    let mut newest: Vec<u32> = gens.iter().map(|g| g.number).collect();
    newest.sort_unstable_by(|a, b| b.cmp(a));
    let mut out: Vec<u32> = gens
        .iter()
        .filter(|g| !g.current && !g.booted)
        .filter(|g| match cleanup {
            Cleanup::KeepLatest(n) => !newest.iter().take(n as usize).any(|k| *k == g.number),
            Cleanup::OlderThanDays(d) => now - g.date > i64::from(d) * SECS_PER_DAY,
        })
        .map(|g| g.number)
        .collect();
    out.sort_unstable();
    out
}

// ── The root side's plan ─────────────────────────────────────────────────

/// The Git work tree a flake in `dir` belongs to, found as Nix finds it:
/// the nearest folder from `dir` up (not `/`) that has a `.git`.
pub fn git_work_tree(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().take_while(|p| p.parent().is_some()).find(|p| p.join(".git").symlink_metadata().is_ok()).map(Path::to_path_buf)
}

fn nix_args(rest: &[&str]) -> Vec<String> {
    NIX_FEATURES.iter().chain(rest).map(|s| s.to_string()).collect()
}

/// The program of a generation, checked to be in the Nix store.
fn store_program(store: &Path, rel: &str, paths: &NixPaths) -> Result<PathBuf, String> {
    let exe = std::fs::canonicalize(store.join(rel)).map_err(|e| format!("{}: {e}", store.join(rel).display()))?;
    if paths.trusted_store && (!exe.starts_with("/nix/store/") || !mandara_config::admin::trusted_root_file(&exe)) {
        return Err(format!("{} is not a root-owned program in the Nix store", exe.display()));
    }
    Ok(exe)
}

/// `systemd-run … <exe> <verb>`.
fn switch_step(exe: PathBuf, verb: &str) -> Step {
    let mut args: Vec<String> = SYSTEMD_RUN.iter().map(|s| s.to_string()).collect();
    args.push(exe.to_string_lossy().into_owned());
    args.push(verb.to_string());
    Step::owned(StepKind::Activate, "systemd-run", args)
}

/// The fixed steps of `action` on this system. Everything is found by the
/// root side itself; numbers are checked against the real generations.
pub fn plan(action: &NixAction, paths: &NixPaths) -> Result<Vec<Step>, String> {
    let profile = paths.system_profile().to_string_lossy().into_owned();
    match *action {
        NixAction::Update(mode) | NixAction::Rebuild(mode) => {
            let update = matches!(action, NixAction::Update(_));
            let mut steps = Vec::new();
            match setup(paths) {
                Setup::Flake { dir, root_owned, host } => {
                    if !valid_host(&host) {
                        return Err(format!("the host name {host:?} can't name a flake configuration"));
                    }
                    let dir_s = dir.to_string_lossy().into_owned();
                    // A flake its owner keeps elsewhere (`/etc/nixos/flake.nix`
                    // linking into a home folder) is updated as that user,
                    // before the helper runs; root never writes there.
                    if update && root_owned {
                        steps.push(Step::owned(StepKind::FlakeUpdate, "nix", nix_args(&["flake", "update", "--flake", &dir_s])));
                    }
                    // Root's Git refuses a work tree another user owns
                    // ("not owned by current user"), and pkexec sets no
                    // `SUDO_UID` to excuse it: both evaluations trust that
                    // one repository. Its owner already decides what root
                    // builds and activates, so this grants nothing new.
                    let work_tree = git_work_tree(&dir);
                    let attr = format!("{dir_s}#nixosConfigurations.\"{host}\".config.system.build.toplevel");
                    steps.push(Step::owned(StepKind::Build, "nix", nix_args(&["build", "--log-format", "internal-json", "-v", "--no-link", "--print-out-paths", &attr])).trust_git(work_tree.clone()));
                    if mode != RebuildMode::Build {
                        steps.push(Step::owned(StepKind::Activate, "nixos-rebuild", vec![mode.id().into(), "--flake".into(), format!("{dir_s}#{host}")]).trust_git(work_tree));
                    }
                }
                Setup::Channels { .. } => {
                    if update {
                        steps.push(Step::new(StepKind::ChannelUpdate, "nix-channel", &["--update"]).env(NIX_PATH));
                    }
                    steps.push(Step::new(StepKind::Build, "nix-build", &["--log-format", "internal-json", "-v", "--no-out-link", "<nixpkgs/nixos>", "-A", "system"]).env(NIX_PATH));
                    if mode != RebuildMode::Build {
                        steps.push(Step::new(StepKind::Activate, "nixos-rebuild", &[mode.id()]).env(NIX_PATH));
                    }
                }
            }
            Ok(steps)
        }
        NixAction::Rollback => Ok(vec![Step::new(StepKind::Activate, "nixos-rebuild", &["switch", "--rollback"]).env(NIX_PATH)]),
        NixAction::SwitchTo(n) => {
            let gens = generations(paths);
            let generation = gens.iter().find(|g| g.number == n).ok_or_else(|| format!("generation {n} doesn't exist"))?;
            if generation.current {
                return Err(format!("generation {n} is already the current one"));
            }
            let exe = store_program(&generation.store, "bin/switch-to-configuration", paths)?;
            Ok(vec![Step::owned(StepKind::Activate, "nix-env", vec!["-p".into(), profile, "--switch-generation".into(), n.to_string()]), switch_step(exe, "switch")])
        }
        NixAction::Clean(cleanup) => {
            let gens = generations(paths);
            let delete = to_delete(&gens, cleanup, chrono::Utc::now().timestamp());
            let mut steps = Vec::new();
            if !delete.is_empty() {
                let mut args = vec!["-p".to_string(), profile, "--delete-generations".into()];
                args.extend(delete.iter().map(u32::to_string));
                steps.push(Step::owned(StepKind::DeleteGenerations, "nix-env", args));
            }
            steps.push(Step::new(StepKind::CollectGarbage, "nix-collect-garbage", &[]));
            if !delete.is_empty()
                && let Some(current) = gens.iter().find(|g| g.current)
            {
                // Drops the deleted generations from the boot menu.
                steps.push(switch_step(store_program(&current.store, "bin/switch-to-configuration", paths)?, "boot"));
            }
            Ok(steps)
        }
    }
}

// ── Progress from nix's internal-json log ────────────────────────────────

/// nix's activity types (libutil/logging.hh).
const ACT_FILE_TRANSFER: u32 = 101;
const ACT_COPY_PATHS: u32 = 103;
const ACT_BUILDS: u32 = 104;
const ACT_BUILD: u32 = 105;
const ACT_SUBSTITUTE: u32 = 108;
const ACT_COPY_PATH: u32 = 100;
/// nix's result types.
const RES_BUILD_LOG_LINE: u32 = 101;
const RES_PROGRESS: u32 = 105;
/// Messages at this level or lower are shown (error, warn, notice, info).
const SHOWN_LEVEL: u8 = 3;
const NIX_PREFIX: &str = "@nix ";

#[derive(Deserialize)]
struct Head<'a> {
    #[serde(borrow)]
    action: Cow<'a, str>,
    #[serde(default)]
    id: u64,
    #[serde(default, rename = "type")]
    kind: u32,
    #[serde(default)]
    level: u8,
    #[serde(default, borrow)]
    text: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    msg: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
struct Progress {
    fields: [u64; 4],
}

#[derive(Deserialize)]
struct LogLine<'a> {
    #[serde(borrow)]
    fields: (Cow<'a, str>,),
}

/// What one log line means for the progress UI.
#[derive(Debug, PartialEq, Eq)]
pub enum NixLine<'a> {
    /// Text worth showing in the log.
    Show(Cow<'a, str>),
    /// Machine-only (progress numbers).
    Hide,
}

/// `/nix/store/<hash>-hello-2.12.1.drv` → `hello-2.12.1`.
fn short_path(p: &str) -> &str {
    let base = p.rsplit('/').next().unwrap_or(p);
    store_name(base.strip_suffix(".drv").unwrap_or(base))
}

/// Feeds a nix log line to `t` (aggregate activities are remembered in
/// `t.nix_ids`); returns what to show.
pub fn nix_line<'a>(line: &'a str, t: &mut Tracker) -> NixLine<'a> {
    let Some(json) = line.strip_prefix(NIX_PREFIX) else {
        if let Some(freed) = parse_gc_summary(line) {
            t.freed = freed.to_string();
        }
        return NixLine::Show(Cow::Borrowed(line));
    };
    let Ok(head) = serde_json::from_str::<Head<'_>>(json) else { return NixLine::Hide };
    match head.action.as_ref() {
        "start" => {
            match head.kind {
                ACT_BUILDS => t.nix_builds = head.id,
                ACT_COPY_PATHS => t.nix_copies = head.id,
                ACT_BUILD => {
                    t.phase = Phase::Building;
                    if let Some(text) = &head.text
                        && let Some(drv) = text.strip_prefix("building '").and_then(|r| r.split('\'').next())
                    {
                        let name = short_path(drv).to_string();
                        t.current = name;
                    }
                }
                ACT_COPY_PATH | ACT_SUBSTITUTE => {
                    if let Some(text) = &head.text
                        && let Some(p) = text.strip_prefix("copying path '").and_then(|r| r.split('\'').next())
                    {
                        if t.built == 0 && t.build_total == 0 {
                            t.phase = Phase::Downloading;
                        }
                        t.current = short_path(p).to_string();
                    }
                }
                ACT_FILE_TRANSFER => {}
                _ => {}
            }
            match head.text {
                Some(text) if !text.is_empty() && head.level <= SHOWN_LEVEL => NixLine::Show(text),
                _ => NixLine::Hide,
            }
        }
        "result" => match head.kind {
            RES_PROGRESS if head.id != 0 && (head.id == t.nix_builds || head.id == t.nix_copies) => {
                if let Ok(p) = serde_json::from_str::<Progress>(json) {
                    let [done, expected, _, _] = p.fields;
                    let (done, expected) = (u32::try_from(done).unwrap_or(u32::MAX), u32::try_from(expected).unwrap_or(u32::MAX));
                    if head.id == t.nix_builds {
                        t.built = done;
                        t.build_total = expected;
                    } else {
                        t.downloaded = done;
                        t.download_total = expected;
                    }
                }
                NixLine::Hide
            }
            RES_BUILD_LOG_LINE => match serde_json::from_str::<LogLine<'_>>(json) {
                Ok(l) => NixLine::Show(l.fields.0),
                Err(_) => NixLine::Hide,
            },
            _ => NixLine::Hide,
        },
        "msg" if head.level <= SHOWN_LEVEL => match head.msg {
            Some(m) => NixLine::Show(m),
            None => NixLine::Hide,
        },
        _ => NixLine::Hide,
    }
}

/// `10 store paths deleted, 15.4 KiB freed` → `15.4 KiB`.
pub fn parse_gc_summary(line: &str) -> Option<&str> {
    let rest = line.trim().split_once(" store paths deleted, ")?.1;
    rest.strip_suffix(" freed")
}

/// The NixOS "backend": its steps come from [`plan`].
pub struct NixBackend;

impl SystemBackend for NixBackend {
    fn id(&self) -> BackendId {
        BackendId::Nix
    }

    fn steps(&self) -> Vec<Step> {
        Vec::new()
    }

    fn parse(&self, step: StepKind, line: &str, t: &mut Tracker) {
        if step == StepKind::Build || line.starts_with(NIX_PREFIX) {
            let _ = nix_line(line, t);
        } else if let Some(freed) = parse_gc_summary(line) {
            t.freed = freed.to_string();
        } else if step == StepKind::Activate && !line.trim().is_empty() {
            t.current = line.trim().chars().take(120).collect();
        }
    }

    fn failure_patterns(&self) -> &'static [(&'static str, FailureKind)] {
        &[
            ("cannot delete current version", FailureKind::Conflict),
            ("could not acquire lock", FailureKind::Locked),
            ("waiting for lock", FailureKind::Locked),
            ("unable to download", FailureKind::Network),
            ("unable to connect", FailureKind::Network),
            ("no space left on device", FailureKind::DiskFull),
        ]
    }
}

// ── Flake inputs ─────────────────────────────────────────────────────────

/// One locked flake input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedInput {
    pub name: String,
    pub rev: String,
    /// Unix seconds.
    pub last_modified: i64,
}

/// An input whose revision changed in an update.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputChange {
    pub name: String,
    pub old: Option<LockedInput>,
    pub new: Option<LockedInput>,
}

/// The root flake's direct inputs from `flake.lock` text.
pub fn locked_inputs(lock: &str) -> Vec<LockedInput> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(lock) else { return Vec::new() };
    let nodes = &v["nodes"];
    let root = v["root"].as_str().unwrap_or("root");
    let Some(inputs) = nodes[root]["inputs"].as_object() else { return Vec::new() };
    let mut out: Vec<LockedInput> = inputs
        .iter()
        .take(MAX_INPUTS)
        .filter_map(|(name, node)| {
            // A follows-path (`["nixpkgs"]`) names no node of its own.
            let node = node.as_str()?;
            let locked = &nodes[node]["locked"];
            let rev = locked["rev"].as_str().or_else(|| locked["narHash"].as_str()).unwrap_or_default().to_string();
            Some(LockedInput { name: name.clone(), rev, last_modified: locked["lastModified"].as_i64().unwrap_or_default() })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The inputs that changed between two `flake.lock`s.
pub fn input_changes(before: &[LockedInput], after: &[LockedInput]) -> Vec<InputChange> {
    let mut out = Vec::new();
    for a in after {
        let old = before.iter().find(|b| b.name == a.name);
        if old.is_none_or(|o| o.rev != a.rev) {
            out.push(InputChange { name: a.name.clone(), old: old.cloned(), new: Some(a.clone()) });
        }
    }
    for b in before.iter().filter(|b| !after.iter().any(|a| a.name == b.name)) {
        out.push(InputChange { name: b.name.clone(), old: Some(b.clone()), new: None });
    }
    out
}

/// argv updating a user-owned flake's inputs (run as that user).
pub fn user_flake_update(dir: &Path) -> Step {
    Step::owned(StepKind::FlakeUpdate, "nix", nix_args(&["flake", "update", "--flake", &dir.to_string_lossy()]))
}

// ── Differences between generations ──────────────────────────────────────

/// One package that changed between two generations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageChange {
    pub name: String,
    /// Versions before (empty: added).
    pub old: String,
    /// Versions after (empty: removed).
    pub new: String,
    /// Size change in bytes (0 when not shown).
    pub delta: i64,
}

/// What changed between two generations.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diff {
    pub changes: Vec<PackageChange>,
    /// Closure size change in bytes (`nix path-info -S`), when known.
    pub closure_delta: Option<i64>,
}

/// argv of `nix store diff-closures <a> <b>` (as the user).
pub fn diff_argv(a: &Path, b: &Path) -> Vec<String> {
    nix_args(&["store", "diff-closures", &a.to_string_lossy(), &b.to_string_lossy()])
}

/// argv of `nix path-info -S <paths…>` (closure sizes, as the user).
pub fn path_info_argv(paths: &[&Path]) -> Vec<String> {
    let mut v = nix_args(&["path-info", "-S"]);
    v.extend(paths.iter().map(|p| p.to_string_lossy().into_owned()));
    v
}

/// `1.2 MiB` / `-300.0 KiB` → bytes.
fn parse_size(s: &str) -> Option<i64> {
    let (num, unit) = s.trim().split_once(' ')?;
    let n: f64 = num.parse().ok()?;
    let mul = match unit {
        "B" | "bytes" => 1.0,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "TiB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((n * mul).round() as i64)
}

/// `nix store diff-closures` output (`name: 1.0 → 1.1, +12.3 KiB`; `∅`
/// for none, `ε` for an empty version; terminal colours are removed).
pub fn parse_diff_closures(text: &str) -> Vec<PackageChange> {
    let mut out = Vec::new();
    for raw in text.lines().take(MAX_CHANGES) {
        let line = super::protocol::clean_line(raw);
        let Some((name, rest)) = line.split_once(": ") else { continue };
        let Some((old, after)) = rest.split_once(" → ") else { continue };
        let (new, size) = match after.split_once(", ") {
            Some((n, s)) => (n, parse_size(s.trim_start_matches('+')).unwrap_or(0)),
            None => (after, 0),
        };
        let clean = |v: &str| if v == "∅" { String::new() } else { v.replace('ε', "") };
        out.push(PackageChange { name: name.to_string(), old: clean(old.trim()), new: clean(new.trim()), delta: size });
    }
    out
}

/// What changed from generation `a` to `b`: runs `nix store
/// diff-closures` and `nix path-info -S` as the user (blocking: call it
/// off the UI thread).
pub fn closure_diff(a: &Path, b: &Path) -> Result<Diff, String> {
    #[cfg(all(feature = "fake-system-update", debug_assertions))]
    if super::fake_active() {
        return Ok(super::fake::diff());
    }
    let a = std::fs::canonicalize(a).map_err(|e| format!("{}: {e}", a.display()))?;
    let b = std::fs::canonicalize(b).map_err(|e| format!("{}: {e}", b.display()))?;
    let nix = crate::updates::find_program("nix").ok_or_else(|| "nix is not installed".to_string())?;
    let run = |argv: Vec<String>| -> Result<String, String> {
        let out = std::process::Command::new(&nix).args(argv).env("LANG", "C").env("LC_ALL", "C").stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).lines().rfind(|l| !l.trim().is_empty()).unwrap_or_default().to_string())
        }
    };
    let changes = parse_diff_closures(&run(diff_argv(&a, &b))?);
    let sizes = run(path_info_argv(&[&a, &b])).map(|t| parse_path_info(&t)).unwrap_or_default();
    let size = |p: &Path| sizes.iter().find(|(q, _)| q == p).map(|(_, s)| *s);
    Ok(Diff { changes, closure_delta: size(&b).zip(size(&a)).map(|(b, a)| b - a) })
}

/// `nix path-info -S` rows: `<path>\t<closure bytes>`.
pub fn parse_path_info(text: &str) -> Vec<(PathBuf, i64)> {
    text.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            Some((PathBuf::from(w.next()?), w.next()?.parse().ok()?))
        })
        .collect()
}

#[cfg(test)]
mod tests;
