//! Whole jobs through the real root-side loop and protocol, with
//! recorded output in place of pkexec and the package managers.

use super::*;
use crate::sysupdate::System;
use crate::sysupdate::fake::{FakeLauncher, Replay, Scenario, nixos_tree};
use crate::sysupdate::nixos::{Cleanup, NixPaths, RebuildMode};

const WAIT: Duration = Duration::from_secs(20);

fn launcher(system: System, replay: Replay) -> Arc<dyn Launcher> {
    Arc::new(FakeLauncher::new(system, replay, NixPaths::under(Path::new("/nonexistent"))))
}

fn wait(job: &Job) -> Finished {
    let start = Instant::now();
    while start.elapsed() < WAIT {
        if let Some(f) = job.with(|s| s.finished.clone()) {
            return f;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the job didn't finish");
}

fn wait_for(job: &Job, what: impl Fn(&JobState) -> bool) {
    let start = Instant::now();
    while start.elapsed() < WAIT {
        if job.with(&what) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out");
}

fn quiet() -> Arc<dyn Fn() + Send + Sync> {
    Arc::new(|| {})
}

fn run(system: System, replay: Replay, request: Request) -> (Job, Finished) {
    let job = start(request, launcher(system, replay), quiet());
    let f = wait(&job);
    (job, f)
}

#[test]
fn every_family_updates_end_to_end() {
    for b in [BackendId::Pacman, BackendId::Apt, BackendId::Dnf, BackendId::ZypperDup, BackendId::ZypperUpdate, BackendId::Xbps, BackendId::Apk] {
        let (job, f) = run(System::Managed(b), Replay::new(b), Request::System { flatpak: true });
        assert_eq!(f.outcome, JobOutcome::Done { nothing_to_do: false }, "{b:?}");
        assert!(f.changed > 0, "{b:?}");
        job.with(|s| {
            assert_eq!(s.backend, Some(b));
            assert_eq!(s.steps as usize, plan::backend(b).steps().len() + 1, "{b:?}: the plan and Flatpak");
            assert_eq!(s.step, s.steps, "{b:?}: every step ran");
            assert!(!s.log.is_empty());
            assert!(s.log.iter().all(|l| !l.starts_with(protocol::MARK.trim_end()) && !l.starts_with("dlstatus:") && !l.starts_with("pmstatus:")), "{b:?}: markers and machine status stay out of the log");
            assert!(s.log.len() <= LOG_LINES);
            assert_eq!(s.overall(), 1.0);
        });
    }
}

#[test]
fn restart_is_suggested_when_core_packages_change() {
    let (_, apt) = run(System::Managed(BackendId::Apt), Replay::new(BackendId::Apt), Request::System { flatpak: false });
    assert_eq!(apt.restart.as_deref(), Some("libc6"));
    let (_, apk) = run(System::Managed(BackendId::Apk), Replay::new(BackendId::Apk), Request::System { flatpak: false });
    assert_eq!(apk.restart.as_deref(), Some("musl"));
    let (_, pacman) = run(System::Managed(BackendId::Pacman), Replay::new(BackendId::Pacman), Request::System { flatpak: false });
    assert_eq!(pacman.restart, None);
}

#[test]
fn up_to_date_systems_stop_early() {
    for b in [BackendId::Pacman, BackendId::Apt, BackendId::Dnf, BackendId::ZypperDup, BackendId::Xbps] {
        let (job, f) = run(System::Managed(b), Replay::new(b).scenario(Scenario::UpToDate), Request::System { flatpak: false });
        assert_eq!(f.outcome, JobOutcome::Done { nothing_to_do: true }, "{b:?}");
        job.with(|s| assert!(s.step < s.steps, "{b:?}: the install step was skipped"));
    }
}

#[test]
fn locks_and_network_failures_are_explained() {
    let (_, f) = run(System::Managed(BackendId::Pacman), Replay::new(BackendId::Pacman).scenario(Scenario::Locked), Request::System { flatpak: true });
    let JobOutcome::Failed(failure) = f.outcome else { panic!("{f:?}") };
    assert_eq!(failure.kind, FailureKind::Locked);
    assert!(failure.detail.contains("unable to lock database"), "{}", failure.detail);
    let (_, f) = run(System::Managed(BackendId::Apt), Replay::new(BackendId::Apt).scenario(Scenario::Locked), Request::System { flatpak: false });
    assert!(matches!(f.outcome, JobOutcome::Failed(Failure { kind: FailureKind::Locked, .. })), "{f:?}");
    let (job, f) = run(System::Managed(BackendId::Xbps), Replay::new(BackendId::Xbps).scenario(Scenario::Fails), Request::System { flatpak: true });
    assert!(matches!(f.outcome, JobOutcome::Failed(Failure { kind: FailureKind::Network, .. })), "{f:?}");
    job.with(|s| assert_eq!(s.step, 2, "stopped after the failed download; Flatpak didn't run"));
}

#[test]
fn cancel_during_a_download_stops_safely() {
    let replay = Replay::new(BackendId::Apt).hold(StepKind::Download, 20);
    let job = start(Request::System { flatpak: true }, launcher(System::Managed(BackendId::Apt), replay), quiet());
    wait_for(&job, |s| s.step_kind == Some(StepKind::Download) && s.tracker.planned);
    assert!(job.with(JobState::cancellable));
    assert!(job.cancel());
    assert!(!job.cancel(), "only once");
    let f = wait(&job);
    assert_eq!(f.outcome, JobOutcome::Cancelled);
    job.with(|s| assert_eq!(s.step, 2, "the install step never started"));
}

#[test]
fn cancel_is_refused_while_installing() {
    // The UI doesn't offer it…
    let mut s = JobState::new(Request::System { flatpak: false });
    s.step_kind = Some(StepKind::Install);
    assert!(!s.cancellable());
    s.step_kind = Some(StepKind::Activate);
    assert!(!s.cancellable());
    // …and the helper refuses a request that arrives anyway.
    struct Probe(Mutex<Vec<(StepKind, bool)>>);
    impl Runner for Probe {
        fn run(&self, step: &Step, _: &mut dyn FnMut(&str), cancel: &CancelSignal) -> std::io::Result<Option<i32>> {
            let accepted = cancel.try_request();
            self.0.lock().unwrap().push((step.kind, accepted));
            Ok(Some(0))
        }
    }
    let probe = Probe(Mutex::new(Vec::new()));
    let steps = vec![plan::Step::new(StepKind::Install, "pacman", &["-Su"]), plan::Step::new(StepKind::Activate, "nixos-rebuild", &["switch"])];
    let mut buf: Vec<u8> = Vec::new();
    let mut out = helper::Out::new(&mut buf);
    let outcome = helper::execute(steps, plan::backend(BackendId::Pacman), &probe, &mut out, &CancelSignal::default());
    assert_eq!(outcome, Outcome::Ok);
    assert_eq!(*probe.0.lock().unwrap(), [(StepKind::Install, false), (StepKind::Activate, false)]);
}

#[test]
fn a_refused_cancel_doesnt_stop_flatpak_afterwards() {
    // A helper that answers a cancel as the download ends (the step the
    // card still shows) by refusing it, then installs.
    struct LateRefusal;
    impl Launcher for LateRefusal {
        fn prepare(&self) -> Result<PathBuf, Failure> {
            Ok(PathBuf::from("/usr/local/lib/mandara/mandara-admin"))
        }
        fn root(&self, _: &Path, _: &[String]) -> std::io::Result<RootProc> {
            let (out_r, mut out_w) = std::io::pipe()?;
            let (in_r, in_w) = std::io::pipe()?;
            let (err_r, err_w) = std::io::pipe()?;
            let handle = std::thread::spawn(move || {
                use std::io::BufRead as _;
                let mut say = |m: Mark| writeln!(out_w, "{}", m.line()).is_ok();
                say(Mark::Begin { backend: "pacman".into(), steps: 2 });
                say(Mark::Step { index: 1, kind: StepKind::Download });
                let asked = std::io::BufReader::new(in_r).lines().map_while(Result::ok).any(|l| l.trim() == protocol::CANCEL);
                say(Mark::Cancel { accepted: !asked });
                say(Mark::Exit { index: 1, code: Some(0) });
                say(Mark::Step { index: 2, kind: StepKind::Install });
                say(Mark::Exit { index: 2, code: Some(0) });
                say(Mark::Done(Outcome::Ok));
                drop(err_w);
                Some(0)
            });
            Ok(RootProc { stdout: Box::new(out_r), stderr: Box::new(err_r), stdin: Box::new(in_w), wait: Box::new(move || Ok(handle.join().ok().flatten())) })
        }
        fn has_flatpak(&self) -> bool {
            true
        }
        fn user(&self) -> &dyn Runner {
            static R: std::sync::OnceLock<Replay> = std::sync::OnceLock::new();
            R.get_or_init(|| Replay::new(BackendId::Pacman))
        }
    }
    let job = start(Request::System { flatpak: true }, Arc::new(LateRefusal), quiet());
    wait_for(&job, |s| s.step_kind == Some(StepKind::Download));
    assert!(job.cancel());
    let f = wait(&job);
    assert_eq!(f.outcome, JobOutcome::Done { nothing_to_do: false }, "Flatpak ran after the refused cancel");
    assert_eq!(f.changed, 3, "the recorded Flatpak update");
    job.with(|s| {
        assert!(!s.cancel_requested, "not left stopping");
        assert_eq!(s.step, s.steps, "every step ran");
    });
}

#[test]
fn a_cancel_the_root_step_never_answers_doesnt_reach_flatpak() {
    // The helper finishes its root part just as the cancel arrives, so it
    // never reads or answers it. The request was for that step; Flatpak
    // must still run.
    static ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    struct GoneBeforeAnswering;
    impl Launcher for GoneBeforeAnswering {
        fn prepare(&self) -> Result<PathBuf, Failure> {
            Ok(PathBuf::from("/usr/local/lib/mandara/mandara-admin"))
        }
        fn root(&self, _: &Path, _: &[String]) -> std::io::Result<RootProc> {
            let (out_r, mut out_w) = std::io::pipe()?;
            let (_in_r, in_w) = std::io::pipe()?;
            let (err_r, err_w) = std::io::pipe()?;
            let handle = std::thread::spawn(move || {
                let mut say = |m: Mark| writeln!(out_w, "{}", m.line()).is_ok();
                say(Mark::Begin { backend: "pacman".into(), steps: 1 });
                say(Mark::Step { index: 1, kind: StepKind::Download });
                while !ASKED.load(std::sync::atomic::Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(2));
                }
                say(Mark::Exit { index: 1, code: Some(0) });
                say(Mark::Done(Outcome::Ok));
                drop(err_w);
                Some(0)
            });
            Ok(RootProc { stdout: Box::new(out_r), stderr: Box::new(err_r), stdin: Box::new(in_w), wait: Box::new(move || Ok(handle.join().ok().flatten())) })
        }
        fn has_flatpak(&self) -> bool {
            true
        }
        fn user(&self) -> &dyn Runner {
            static R: std::sync::OnceLock<Replay> = std::sync::OnceLock::new();
            R.get_or_init(|| Replay::new(BackendId::Pacman))
        }
    }
    let job = start(Request::System { flatpak: true }, Arc::new(GoneBeforeAnswering), quiet());
    wait_for(&job, |s| s.step_kind == Some(StepKind::Download));
    assert!(job.cancel());
    ASKED.store(true, std::sync::atomic::Ordering::SeqCst);
    let f = wait(&job);
    assert_eq!(f.outcome, JobOutcome::Done { nothing_to_do: false }, "Flatpak ran: the cancel was the root step's");
    assert_eq!(f.changed, 3, "the recorded Flatpak update");
    job.with(|s| assert!(!s.cancel_requested, "not left stopping"));
}

#[test]
fn flatpak_only_needs_no_helper() {
    struct NoRoot;
    impl Launcher for NoRoot {
        fn prepare(&self) -> Result<PathBuf, Failure> {
            panic!("Flatpak updates never ask for a password")
        }
        fn root(&self, _: &Path, _: &[String]) -> std::io::Result<RootProc> {
            panic!("never root")
        }
        fn has_flatpak(&self) -> bool {
            true
        }
        fn user(&self) -> &dyn Runner {
            static R: std::sync::OnceLock<Replay> = std::sync::OnceLock::new();
            R.get_or_init(|| Replay::new(BackendId::Pacman))
        }
    }
    let job = start(Request::Flatpak, Arc::new(NoRoot), quiet());
    let f = wait(&job);
    assert_eq!(f.outcome, JobOutcome::Done { nothing_to_do: false });
    assert_eq!(f.changed, 3);
}

#[test]
fn pkexec_answers_are_explained() {
    assert_eq!(helper_failure(Some(126), "").kind, FailureKind::NotAuthorized);
    assert_eq!(helper_failure(Some(127), "Error executing command as another user: Not authorized").kind, FailureKind::NotAuthorized);
    let none = helper_failure(Some(127), "Error executing command as another user: No authentication agent found.");
    assert_eq!(none.kind, FailureKind::NoPrompt);
    assert_eq!(none.detail, "Error executing command as another user: No authentication agent found.");
    assert_eq!(helper_failure(Some(1), "").kind, FailureKind::Helper);
}

#[test]
fn the_log_is_a_bounded_ring() {
    let mut s = JobState::new(Request::Flatpak);
    let first = s.log.capacity();
    for i in 0..LOG_LINES + 50 {
        s.push_log(&format!("line {i}"));
    }
    assert_eq!(s.log.len(), LOG_LINES);
    assert_eq!(s.dropped, 50);
    assert_eq!(s.log.front().unwrap(), "line 50");
    assert_eq!(s.log.capacity(), first, "never grows past its first allocation");
}

#[test]
fn flatpak_lines() {
    let mut t = Tracker::default();
    for l in ["Looking for updates…", "Updating app/org.gnome.Calculator/x86_64/stable", "Installing runtime/org.gnome.Platform.Locale/x86_64/48", "Nothing to do."] {
        flatpak_line(l, &mut t);
    }
    assert_eq!((t.installed, t.changed), (2, 2));
    assert!(t.nothing_to_do);
}

fn nix_job(action: NixAction, scenario: Scenario) -> (tempfile::TempDir, Job, Finished) {
    let dir = tempfile::tempdir().unwrap();
    let paths = nixos_tree(dir.path()).unwrap();
    let replay = Replay::new(BackendId::Nix).scenario(scenario).flake_dir(paths.etc_nixos.clone());
    let job = start(Request::Nix { action }, Arc::new(FakeLauncher::new(System::NixOs, replay, paths)), quiet());
    let f = wait(&job);
    (dir, job, f)
}

#[test]
fn nixos_update_reports_changed_inputs() {
    let (_dir, job, f) = nix_job(NixAction::Update(RebuildMode::Switch), Scenario::Success);
    assert_eq!(f.outcome, JobOutcome::Done { nothing_to_do: false });
    assert_eq!(f.inputs.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["flake-utils", "systems"]);
    job.with(|s| {
        assert_eq!(s.steps, 3, "your flake's update, the build, the switch");
        assert_eq!(s.backend, Some(BackendId::Nix));
        assert!(s.log.iter().any(|l| l.contains("copying path")), "nix's activities are shown as text");
        assert!(s.log.iter().all(|l| !l.starts_with("@nix ")));
    });
}

#[test]
fn a_cancel_during_the_build_after_your_flakes_update_reaches_the_helper() {
    // Your flake's update runs as you, then the build as root: the cancel
    // belongs to the root build again, not to the step before it.
    let dir = tempfile::tempdir().unwrap();
    let paths = nixos_tree(dir.path()).unwrap();
    let replay = Replay::new(BackendId::Nix).scenario(Scenario::Success).flake_dir(paths.etc_nixos.clone()).hold(StepKind::Build, 20);
    let job = start(Request::Nix { action: NixAction::Update(RebuildMode::Switch) }, Arc::new(FakeLauncher::new(System::NixOs, replay, paths)), quiet());
    wait_for(&job, |s| s.step_kind == Some(StepKind::Build));
    assert!(job.cancel());
    let f = wait(&job);
    assert_eq!(f.outcome, JobOutcome::Cancelled, "the helper heard it and stopped the build");
}

#[test]
fn nixos_build_failure_and_clean_up() {
    let (_dir, _, f) = nix_job(NixAction::Rebuild(RebuildMode::Switch), Scenario::Fails);
    assert!(matches!(f.outcome, JobOutcome::Failed(_)), "{f:?}");
    let (_dir, job, f) = nix_job(NixAction::Clean(Cleanup::KeepLatest(3)), Scenario::Success);
    assert_eq!(f.outcome, JobOutcome::Done { nothing_to_do: false });
    assert_eq!(f.freed.as_deref(), Some("15.4 KiB"));
    job.with(|s| assert_eq!(s.steps, 3));
}

#[test]
fn nixos_refuses_a_missing_generation() {
    let (_dir, _, f) = nix_job(NixAction::SwitchTo(99), Scenario::Success);
    let JobOutcome::Failed(failure) = f.outcome else { panic!("{f:?}") };
    assert!(failure.detail.contains("doesn't exist"), "{}", failure.detail);
}
