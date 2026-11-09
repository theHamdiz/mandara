//! Recorded output in place of real package managers: the unit tests, and
//! the sandbox's progress screenshots (feature `fake-system-update`, debug
//! builds only; a release never contains it).
//!
//! The output in `fixtures/` was recorded by running the real commands as
//! root in clean rootless containers of each distribution. The fake runs
//! the real root-side loop ([`super::helper::run_action`]) in-process
//! with these recordings, so the progress UI sees exactly the stream the
//! real helper writes. Only the Flatpak, `nixos-rebuild` activation and
//! generation-deletion texts are illustrative (no container can run them).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::helper::{self, Action, CancelSignal, Out, Runner};
use super::job::{Launcher, RootProc};
use super::nixos::{self, NixPaths};
use super::plan::{BackendId, FREEBSD_UPDATE, Step, StepKind};
use super::{Failure, System};

macro_rules! fixture {
    ($path:literal) => {
        include_str!(concat!("fixtures/", $path))
    };
}

/// What the recording shows.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Scenario {
    Success,
    UpToDate,
    /// Another program holds the lock.
    Locked,
    /// A build fails (NixOS) / a download fails (Void).
    Fails,
}

/// How often a held replay looks for a cancel.
const HOLD_POLL: Duration = Duration::from_millis(50);
/// Pause between replayed lines in the sandbox (`…_DELAY_MS` overrides).
#[cfg(all(feature = "fake-system-update", debug_assertions))]
const DEFAULT_DELAY: Duration = Duration::from_millis(40);
/// Fixture generations are this many days apart.
const GENERATION_SPACING_DAYS: u64 = 9;
const SECS_PER_DAY: u64 = 86_400;
const SECS_PER_HOUR: u64 = 3_600;

/// Illustrative output for steps no container can record.
const FLATPAK: &str = "Looking for updates…\nUpdating app/org.gnome.Calculator/x86_64/stable\nUpdating runtime/org.gnome.Platform/x86_64/48\nUpdating app/com.spotify.Client/x86_64/stable\nUpdates complete.\n";
const NIXOS_ACTIVATE: &str = "building the system configuration...\nactivating the configuration...\nsetting up /etc...\nreloading user units for mandara...\nrestarting sysinit-reactivation.target\nthe following new units were started: run-credentials.mount\n";
const NIXOS_DELETE: &str = "removing profile version 38\nremoving profile version 39\nremoving profile version 40\n";

/// The recording and exit code of `kind` for `backend`.
pub fn recording(backend: BackendId, kind: StepKind, scenario: Scenario) -> (&'static str, i32) {
    use BackendId as B;
    use Scenario as S;
    use StepKind as K;
    match (backend, kind, scenario) {
        (_, K::Flatpak, _) => (FLATPAK, 0),
        (B::Pacman, K::Download, S::UpToDate) => (fixture!("arch/uptodate.txt"), 0),
        (B::Pacman, K::Download, S::Locked) => (fixture!("arch/locked.txt"), 1),
        (B::Pacman, K::Download, _) => (fixture!("arch/download.txt"), 0),
        (B::Pacman, _, _) => (fixture!("arch/install.txt"), 0),
        (B::Apt, K::Refresh, S::Locked) => (fixture!("debian/locked.txt"), 100),
        (B::Apt, K::Refresh, _) => (fixture!("debian/refresh.txt"), 0),
        (B::Apt, K::Download, S::UpToDate) => (fixture!("debian/uptodate.txt"), 0),
        (B::Apt, K::Download, _) => (fixture!("debian/download.txt"), 0),
        (B::Apt, _, _) => (fixture!("debian/install.txt"), 0),
        (B::Dnf, K::Download, S::UpToDate) => (fixture!("dnf5/uptodate.txt"), 0),
        (B::Dnf, K::Download, _) => (fixture!("dnf5/download.txt"), 0),
        (B::Dnf, _, _) => (fixture!("dnf5/install.txt"), 0),
        (B::ZypperDup | B::ZypperUpdate, K::Refresh, S::Locked) => (fixture!("zypper/broken-repo.txt"), 4),
        (B::ZypperDup | B::ZypperUpdate, K::Refresh, _) => (fixture!("zypper/refresh.txt"), 0),
        (B::ZypperDup | B::ZypperUpdate, K::Download, S::UpToDate) => (fixture!("zypper/uptodate.txt"), 0),
        (B::ZypperDup | B::ZypperUpdate, K::Download, _) => (fixture!("zypper/download.txt"), 0),
        (B::ZypperDup | B::ZypperUpdate, _, _) => (fixture!("zypper/install.txt"), 0),
        (B::Xbps, K::Refresh, _) => (fixture!("xbps/sync.txt"), 0),
        (B::Xbps, K::Download, S::UpToDate) => (fixture!("xbps/uptodate.txt"), 0),
        (B::Xbps, K::Download, S::Fails) => (fixture!("xbps/download-failed.txt"), 5),
        (B::Xbps, K::Download, _) => (fixture!("xbps/download.txt"), 0),
        (B::Xbps, _, _) => (fixture!("xbps/install.txt"), 0),
        (B::Apk, K::Refresh, _) => (fixture!("apk/refresh.txt"), 0),
        (B::Apk, _, S::UpToDate) => (fixture!("apk/uptodate.txt"), 0),
        (B::Apk, _, _) => (fixture!("apk/install.txt"), 0),
        (B::Nix, K::FlakeUpdate, _) => (fixture!("nix/flake-update.txt"), 0),
        (B::Nix, K::ChannelUpdate, _) => (fixture!("nix/channel-update.txt"), 0),
        (B::Nix, K::Build, S::Fails) => (fixture!("nix/build-fail.txt"), 1),
        (B::Nix, K::Build, _) => (fixture!("nix/build-substitute.txt"), 0),
        (B::Nix, K::DeleteGenerations, _) => (NIXOS_DELETE, 0),
        (B::Nix, K::CollectGarbage, _) => (fixture!("nix/gc.txt"), 0),
        (B::Nix, _, _) => (NIXOS_ACTIVATE, 0),
        (B::Pkg, K::Refresh, S::Locked) => (fixture!("freebsd/locked.txt"), 1),
        (B::Pkg, K::Refresh, _) => (fixture!("freebsd/refresh.txt"), 0),
        (B::Pkg, _, S::UpToDate) => (fixture!("freebsd/uptodate.txt"), 0),
        (B::Pkg, K::Download, _) => (fixture!("freebsd/download.txt"), 0),
        (B::Pkg, _, _) => (fixture!("freebsd/install.txt"), 0),
    }
}

/// The recording of `step`: FreeBSD's base-system steps (freebsd-update)
/// have their own, the rest follow [`recording`].
pub fn recording_of(backend: BackendId, step: &Step, scenario: Scenario) -> (&'static str, i32) {
    if backend != BackendId::Pkg || step.program != FREEBSD_UPDATE {
        return recording(backend, step.kind, scenario);
    }
    match (step.kind, scenario) {
        (StepKind::Download, Scenario::UpToDate) => (fixture!("freebsd/base-uptodate.txt"), 0),
        (StepKind::Download, Scenario::Fails) => (fixture!("freebsd/base-not-release.txt"), 1),
        (StepKind::Download, _) => (fixture!("freebsd/base-fetch.txt"), 0),
        (_, Scenario::UpToDate) => ("No updates are available to install.\nRun 'freebsd-update [options] fetch' first.\n", super::plan::FREEBSD_UPDATE_NOTHING),
        _ => (fixture!("freebsd/base-install.txt"), 0),
    }
}

/// The recorded `nix store diff-closures` and `path-info -S` outputs.
pub fn diff() -> nixos::Diff {
    let changes = nixos::parse_diff_closures(fixture!("nix/diff-closures.txt"));
    let sizes = nixos::parse_path_info(fixture!("nix/path-info.txt"));
    nixos::Diff { changes, closure_delta: (sizes.len() == 2).then(|| sizes[0].1 - sizes[1].1) }
}

/// Replays recordings as if they were the programs' output.
pub struct Replay {
    backend: BackendId,
    scenario: Scenario,
    delay: Duration,
    /// `(step, line)`: wait there until cancelled (mid-progress shots).
    hold: Option<(StepKind, usize)>,
    /// A flake folder whose `flake.lock` the fake update rewrites.
    flake_dir: Option<PathBuf>,
}

impl Replay {
    pub fn new(backend: BackendId) -> Replay {
        Replay { backend, scenario: Scenario::Success, delay: Duration::ZERO, hold: None, flake_dir: None }
    }

    pub fn scenario(mut self, s: Scenario) -> Replay {
        self.scenario = s;
        self
    }

    /// Waits at line `line` of `step` until cancelled.
    pub fn hold(mut self, step: StepKind, line: usize) -> Replay {
        self.hold = Some((step, line));
        self
    }

    /// The flake folder the fake inputs update rewrites.
    pub fn flake_dir(mut self, dir: PathBuf) -> Replay {
        self.flake_dir = Some(dir);
        self
    }
}

impl Runner for Replay {
    fn run(&self, step: &Step, line: &mut dyn FnMut(&str), cancel: &CancelSignal) -> std::io::Result<Option<i32>> {
        let (text, code) = recording_of(self.backend, step, self.scenario);
        for (i, l) in text.lines().enumerate() {
            if step.kind.cancellable() && cancel.requested() {
                return Ok(Some(130));
            }
            if self.hold.is_some_and(|(k, at)| k == step.kind && at == i) {
                while !cancel.requested() {
                    std::thread::sleep(HOLD_POLL);
                }
                return Ok(Some(130));
            }
            line(l);
            if !self.delay.is_zero() {
                std::thread::sleep(self.delay);
            }
        }
        if step.kind == StepKind::FlakeUpdate
            && let Some(dir) = &self.flake_dir
        {
            let _ = std::fs::write(dir.join("flake.lock"), fixture!("nix/flake.lock.new"));
        }
        Ok(Some(code))
    }
}

/// A launcher whose "root helper" is the real root-side loop, in-process,
/// replaying recordings.
pub struct FakeLauncher {
    pub system: System,
    pub replay: Arc<Replay>,
    pub nix: NixPaths,
}

struct ReplayRef(Arc<Replay>);

impl Runner for ReplayRef {
    fn run(&self, step: &Step, line: &mut dyn FnMut(&str), cancel: &CancelSignal) -> std::io::Result<Option<i32>> {
        self.0.run(step, line, cancel)
    }
}

impl FakeLauncher {
    pub fn new(system: System, replay: Replay, nix: NixPaths) -> FakeLauncher {
        FakeLauncher { system, replay: Arc::new(replay), nix }
    }
}

impl Launcher for FakeLauncher {
    fn prepare(&self) -> Result<PathBuf, Failure> {
        Ok(PathBuf::from("/usr/local/lib/mandara/mandara-admin"))
    }

    fn root(&self, _admin: &Path, args: &[String]) -> std::io::Result<RootProc> {
        let action = Action::parse(args).map_err(std::io::Error::other)?;
        let (out_r, mut out_w) = std::io::pipe()?;
        let (in_r, in_w) = std::io::pipe()?;
        let (err_r, err_w) = std::io::pipe()?;
        let (system, replay, nix) = (self.system, ReplayRef(self.replay.clone()), self.nix.clone());
        let handle = std::thread::spawn(move || {
            let cancel = CancelSignal::default();
            // The helper's stdin watcher.
            let watch = cancel.clone();
            let answers = Arc::new(Mutex::new(Vec::<bool>::new()));
            let answered = answers.clone();
            std::thread::spawn(move || {
                use std::io::BufRead as _;
                for l in std::io::BufReader::new(in_r).lines() {
                    let Ok(l) = l else { break };
                    if l.trim() == super::protocol::CANCEL {
                        let ok = watch.try_request();
                        answered.lock().unwrap_or_else(|e| e.into_inner()).push(ok);
                    }
                }
            });
            let mut sink = AnswerWriter { inner: &mut out_w, answers };
            let mut out = Out::new(&mut sink);
            let code = helper::run_action(&action, system, &replay, &mut out, &cancel, &nix);
            drop(err_w);
            Some(code)
        });
        Ok(RootProc {
            stdout: Box::new(out_r),
            stderr: Box::new(err_r),
            stdin: Box::new(in_w),
            wait: Box::new(move || Ok(handle.join().ok().flatten())),
        })
    }

    fn user(&self) -> &dyn Runner {
        &*self.replay
    }

    fn nix_paths(&self) -> NixPaths {
        self.nix.clone()
    }

    fn has_flatpak(&self) -> bool {
        true
    }

    fn restart_facts(&self) -> (String, PathBuf, PathBuf) {
        (String::new(), PathBuf::from("/nonexistent"), PathBuf::from("/nonexistent"))
    }
}

/// Writes the stdin watcher's cancel answers between lines, as the real
/// helper's shared writer does.
struct AnswerWriter<'a> {
    inner: &'a mut std::io::PipeWriter,
    answers: Arc<Mutex<Vec<bool>>>,
}

impl std::io::Write for AnswerWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let pending: Vec<bool> = std::mem::take(&mut *self.answers.lock().unwrap_or_else(|e| e.into_inner()));
        for accepted in pending {
            writeln!(self.inner, "{}", super::protocol::Mark::Cancel { accepted }.line())?;
        }
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

// ── A fixture NixOS system ───────────────────────────────────────────────

/// Builds a small NixOS profile tree under `root`: generations 38–44
/// (44 current, 43 booted), each with a NixOS version and a kernel, a
/// flake in `etc/nixos`, a host name and two home-manager generations.
pub fn nixos_tree(root: &Path) -> std::io::Result<NixPaths> {
    use std::os::unix::fs::symlink;
    let paths = NixPaths::under(root);
    let store = root.join("nix/store");
    std::fs::create_dir_all(&paths.profiles)?;
    std::fs::create_dir_all(&store)?;
    let versions = [(38, "24.11.719113.cf8cc1201be8", "6.6.62"), (39, "24.11.719501.cf8cc1201be8", "6.6.63"), (40, "25.05.802216.3016b4b15d13", "6.12.30"), (41, "25.05.803471.3016b4b15d13", "6.12.31"), (42, "25.05.806273.7a732ed41ca0", "6.12.33"), (43, "25.11.20251020.a1b2c3d4", "6.17.4"), (44, "25.11.20251106.e5f6a7b8", "6.17.7")];
    for (i, (n, version, kernel)) in versions.iter().enumerate() {
        let hash = format!("{:0>32}", format!("{n}gen{i}"));
        let sys = store.join(format!("{hash}-nixos-system-mandara-{version}"));
        let linux = store.join(format!("{:0>32}-linux-{kernel}", format!("{n}k")));
        std::fs::create_dir_all(sys.join("bin"))?;
        std::fs::create_dir_all(&linux)?;
        std::fs::write(linux.join("bzImage"), b"")?;
        std::fs::write(sys.join("nixos-version"), version)?;
        std::fs::write(sys.join("bin/switch-to-configuration"), b"")?;
        let _ = std::fs::remove_file(sys.join("kernel"));
        symlink(linux.join("bzImage"), sys.join("kernel"))?;
        let link = paths.generation_link(*n);
        let _ = std::fs::remove_file(&link);
        symlink(&sys, &link)?;
        // Spread the dates: a few days apart, the newest today.
        let age = Duration::from_secs(SECS_PER_DAY * (versions.len() - 1 - i) as u64 * GENERATION_SPACING_DAYS + SECS_PER_HOUR);
        let when = std::time::SystemTime::now() - age;
        let _ = set_link_time(&link, when);
        if *n == 43 {
            std::fs::create_dir_all(root.join("run"))?;
            let _ = std::fs::remove_file(&paths.booted);
            symlink(&sys, &paths.booted)?;
        }
    }
    let _ = std::fs::remove_file(paths.system_profile());
    symlink("system-44-link", paths.system_profile())?;
    std::fs::create_dir_all(&paths.etc_nixos)?;
    std::fs::write(paths.etc_nixos.join("flake.nix"), "{ outputs = _: { }; }\n")?;
    std::fs::write(paths.etc_nixos.join("flake.lock"), fixture!("nix/flake.lock.old"))?;
    std::fs::create_dir_all(paths.hostname.parent().unwrap_or(root))?;
    std::fs::write(&paths.hostname, "mandara\n")?;
    let hm = root.join("home/.local/state/nix/profiles");
    std::fs::create_dir_all(&hm)?;
    for n in [11, 12] {
        let target = store.join(format!("{:0>32}-home-manager-generation", format!("hm{n}")));
        std::fs::create_dir_all(&target)?;
        let link = hm.join(format!("home-manager-{n}-link"));
        let _ = std::fs::remove_file(&link);
        symlink(&target, &link)?;
    }
    let _ = std::fs::remove_file(hm.join("home-manager"));
    symlink("home-manager-12-link", hm.join("home-manager"))?;
    Ok(paths)
}

fn set_link_time(link: &Path, when: std::time::SystemTime) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt as _;
    let secs = when.duration_since(std::time::UNIX_EPOCH).map_err(std::io::Error::other)?.as_secs() as libc::time_t;
    let times = [libc::timespec { tv_sec: secs, tv_nsec: 0 }, libc::timespec { tv_sec: secs, tv_nsec: 0 }];
    let c = std::ffi::CString::new(link.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
    // SAFETY: a valid C path and two timespecs; AT_SYMLINK_NOFOLLOW sets
    // the link's own times.
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), libc::AT_SYMLINK_NOFOLLOW) };
    if rc == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

/// The sandbox configuration: `MANDARA_FAKE_SYSTEM_UPDATE=<backend>[:<scenario>]`
/// (`pacman`, `apt:locked`, `nixos`, `nixos:fails`…), with
/// `MANDARA_FAKE_SYSTEM_UPDATE_DELAY_MS` per line and
/// `MANDARA_FAKE_SYSTEM_UPDATE_HOLD=<step>:<line>` to pause mid-way.
#[cfg(all(feature = "fake-system-update", debug_assertions))]
pub fn from_env() -> Option<(System, Scenario)> {
    let spec = std::env::var("MANDARA_FAKE_SYSTEM_UPDATE").ok()?;
    let (name, scenario) = spec.split_once(':').unwrap_or((&spec, ""));
    let system = if name == "nixos" { System::NixOs } else { System::Managed(BackendId::from_id(name)?) };
    let scenario = match scenario {
        "uptodate" => Scenario::UpToDate,
        "locked" => Scenario::Locked,
        "fails" => Scenario::Fails,
        _ => Scenario::Success,
    };
    Some((system, scenario))
}

/// The sandbox's launcher (a fixture NixOS tree under the runtime folder).
#[cfg(all(feature = "fake-system-update", debug_assertions))]
pub fn launcher() -> Option<FakeLauncher> {
    let (system, scenario) = from_env()?;
    let delay = std::env::var("MANDARA_FAKE_SYSTEM_UPDATE_DELAY_MS").ok().and_then(|d| d.parse().ok()).map_or(DEFAULT_DELAY, Duration::from_millis);
    let hold = std::env::var("MANDARA_FAKE_SYSTEM_UPDATE_HOLD").ok().and_then(|h| {
        let (k, n) = h.split_once(':')?;
        Some((StepKind::from_id(k)?, n.parse().ok()?))
    });
    let nix = nixos_tree(&fake_root()).ok()?;
    let backend = match system {
        System::Managed(b) => b,
        _ => BackendId::Nix,
    };
    let replay = Replay { backend, scenario, delay, hold, flake_dir: Some(nix.etc_nixos.clone()) };
    Some(FakeLauncher::new(system, replay, nix))
}

/// Where the sandbox's fixture NixOS tree lives.
#[cfg(all(feature = "fake-system-update", debug_assertions))]
pub fn fake_root() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("mandara-fake-nixos")
}
