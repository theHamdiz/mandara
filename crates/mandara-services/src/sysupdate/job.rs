//! The user side of a system update: one owned worker thread per job.
//!
//! The worker starts the root helper through pkexec (so the shell's polkit
//! agent shows Mandara's password dialog), reads its output incrementally
//! into a bounded [`JobState`] (a capped ring of recent log lines plus the
//! parsed progress), then runs the rootless Flatpak step. The UI reads the
//! state under a short lock and is pinged through a coalescing callback,
//! never per line. The job keeps running when its panel closes; nothing
//! but a cancel request at a safe point stops it early.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mandara_i18n::t;
use serde::{Deserialize, Serialize};

use super::helper::{self, CancelSignal, Runner};
use super::nixos::{self, InputChange, NixAction, Setup};
use super::parse::Tracker;
use super::plan::{self, BackendId, Step, StepKind};
use super::protocol::{self, Mark, Outcome};
use super::{Failure, FailureKind, Phase};

/// Log lines kept for the UI (older ones are dropped; the full log is in
/// [`log_path`]).
pub const LOG_LINES: usize = 300;
/// pkexec: the dialog was dismissed / authorization failed.
const PKEXEC_DISMISSED: i32 = 126;
const PKEXEC_FAILED: i32 = 127;
/// Debian and Ubuntu's flag file for "a restart is required".
const REBOOT_REQUIRED: &str = "/run/reboot-required";
/// The running kernel's modules folder parent.
const MODULES: &str = "/usr/lib/modules";
/// Lines of pkexec's own stderr kept for the failure message.
const STDERR_LINES: usize = 8;

/// What to update.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Request {
    /// The system packages, then (optionally) Flatpak.
    System { flatpak: bool },
    /// Only Flatpak (no password).
    Flatpak,
    /// A NixOS action.
    Nix { action: NixAction },
}

impl Request {
    fn root_args(&self) -> Option<Vec<String>> {
        match self {
            Request::System { .. } => Some(vec!["apply".into()]),
            Request::Flatpak => None,
            Request::Nix { action } => Some(action.args()),
        }
    }

    fn flatpak(&self) -> bool {
        matches!(self, Request::System { flatpak: true } | Request::Flatpak)
    }
}

/// How a finished job ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "result")]
pub enum JobOutcome {
    Done { nothing_to_do: bool },
    Cancelled,
    Failed(Failure),
}

/// The summary of a finished job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finished {
    pub outcome: JobOutcome,
    pub elapsed_secs: u64,
    /// Packages the update changed.
    pub changed: u32,
    /// Why a restart is recommended (a package name, or a reason).
    pub restart: Option<String>,
    /// NixOS clean-up: the space freed.
    pub freed: Option<String>,
    /// NixOS: flake inputs that changed.
    pub inputs: Vec<InputChange>,
    /// Repositories that couldn't be refreshed.
    pub warnings: u32,
}

/// Everything the UI shows about a job.
#[derive(Debug)]
pub struct JobState {
    pub request: Request,
    pub backend: Option<BackendId>,
    pub tracker: Tracker,
    /// 1-based step of the helper's plan, and how many there are (the
    /// Flatpak step counts as one more).
    pub step: u32,
    pub steps: u32,
    pub step_kind: Option<StepKind>,
    pub cancel_requested: bool,
    /// A rootless step (Flatpak) runs now: a cancel goes to it, not to
    /// the root helper. Set and read under this lock, so a cancel and a
    /// step change never cross.
    in_user_step: bool,
    /// The helper answered a cancel request with "not now".
    pub cancel_refused: bool,
    pub log: VecDeque<String>,
    /// Lines that fell out of [`JobState::log`].
    pub dropped: u64,
    pub started: Instant,
    pub finished: Option<Finished>,
    /// Bumped on every change (the UI can skip unchanged renders).
    pub revision: u64,
}

impl JobState {
    fn new(request: Request) -> JobState {
        JobState {
            request,
            backend: None,
            tracker: Tracker::default(),
            step: 0,
            steps: 0,
            step_kind: None,
            cancel_requested: false,
            in_user_step: false,
            cancel_refused: false,
            log: VecDeque::with_capacity(LOG_LINES),
            dropped: 0,
            started: Instant::now(),
            finished: None,
            revision: 0,
        }
    }

    fn push_log(&mut self, line: &str) {
        if self.log.len() == LOG_LINES {
            // Reuse the oldest line's buffer for the newest.
            let mut old = self.log.pop_front().unwrap_or_default();
            old.clear();
            old.push_str(line);
            self.log.push_back(old);
            self.dropped += 1;
        } else {
            self.log.push_back(line.to_string());
        }
    }

    /// Whether a cancel can be offered right now.
    pub fn cancellable(&self) -> bool {
        self.finished.is_none() && !self.cancel_requested && self.step_kind.is_some_and(StepKind::cancellable)
    }

    /// 0–1 over the whole job: finished steps plus the current one's share.
    pub fn overall(&self) -> f32 {
        if self.finished.is_some() {
            return 1.0;
        }
        let steps = self.steps.max(1) as f32;
        let within = self.tracker.fraction().unwrap_or(0.0);
        ((self.step.saturating_sub(1) as f32 + within) / steps).clamp(0.0, 1.0)
    }

    pub fn phase(&self) -> Phase {
        if self.finished.is_some() { Phase::Finished } else { self.tracker.phase }
    }
}

/// A running (or finished) job. Cheap to clone; the worker owns the
/// processes.
#[derive(Clone)]
pub struct Job {
    state: Arc<Mutex<JobState>>,
    cancel: Arc<Mutex<Option<std::sync::mpsc::Sender<()>>>>,
    user_cancel: CancelSignal,
}

impl Job {
    /// Reads the state under the lock (keep `f` short: no formatting of
    /// the whole log).
    pub fn with<R>(&self, f: impl FnOnce(&JobState) -> R) -> R {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        f(&state)
    }

    pub fn finished(&self) -> bool {
        self.with(|s| s.finished.is_some())
    }

    /// Asks to stop. Honoured only while a cancellable step runs (the
    /// helper decides for the root part).
    pub fn cancel(&self) -> bool {
        // The request goes to whichever part owns the running step, decided
        // under the state lock that step changes also take: a root step
        // finishing at this moment can't hand it on to Flatpak.
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let ok = s.cancellable();
        if ok {
            s.cancel_requested = true;
            s.cancel_refused = false;
            s.revision += 1;
            if s.in_user_step {
                self.user_cancel.request();
            } else if let Some(tx) = self.cancel.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                let _ = tx.send(());
            }
        }
        ok
    }
}

/// The started root helper.
pub struct RootProc {
    pub stdout: Box<dyn Read + Send>,
    pub stderr: Box<dyn Read + Send>,
    pub stdin: Box<dyn Write + Send>,
    pub wait: Box<dyn FnOnce() -> std::io::Result<Option<i32>> + Send>,
}

/// Starts the privileged helper and the rootless steps (the real one uses
/// pkexec; tests and the sandbox replay recorded output).
pub trait Launcher: Send + Sync {
    /// Makes sure a root-owned helper that speaks this protocol exists
    /// (installing or refreshing it costs one approval). Returns it.
    fn prepare(&self) -> Result<PathBuf, Failure>;
    fn root(&self, admin: &Path, args: &[String]) -> std::io::Result<RootProc>;
    /// Runs a rootless step (as the user).
    fn user(&self) -> &dyn Runner;
    /// This system's setup, for NixOS jobs.
    fn nix_paths(&self) -> nixos::NixPaths {
        nixos::NixPaths::system()
    }
    /// Whether Flatpak is installed (its step is skipped otherwise).
    fn has_flatpak(&self) -> bool {
        crate::updates::find_program("flatpak").is_some()
    }
    /// Facts for the restart check: `(kernel release, modules folder,
    /// reboot-required flag)`.
    fn restart_facts(&self) -> (String, PathBuf, PathBuf) {
        (super::kernel_release(), PathBuf::from(MODULES), PathBuf::from(REBOOT_REQUIRED))
    }
}

/// Runs rootless programs (Flatpak, a user-owned flake's update) found on
/// `$PATH`, with the user's environment.
pub struct UserRunner;

impl Runner for UserRunner {
    fn run(&self, step: &Step, line: &mut dyn FnMut(&str), cancel: &CancelSignal) -> std::io::Result<Option<i32>> {
        use std::os::unix::process::CommandExt as _;
        let program = crate::updates::find_program(step.program).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, format!("{} is not installed", step.program)))?;
        let mut child = Command::new(program)
            .args(&step.args)
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()?;
        helper::pump(&mut child, line, cancel, step.kind.cancellable())
    }
}

/// The real launcher: pkexec with the root-owned admin helper.
pub struct PkexecLauncher;

/// Why the admin helper couldn't be prepared.
fn helper_failure(code: Option<i32>, stderr: &str) -> Failure {
    let kind = match code {
        Some(PKEXEC_DISMISSED) => FailureKind::NotAuthorized,
        Some(PKEXEC_FAILED) if no_agent(stderr) => FailureKind::NoPrompt,
        Some(PKEXEC_FAILED) => FailureKind::NotAuthorized,
        _ => FailureKind::Helper,
    };
    Failure { kind, detail: last_line(stderr).to_string() }
}

fn no_agent(stderr: &str) -> bool {
    plan::contains_ci(stderr, "no authentication agent") || plan::contains_ci(stderr, "cannot run program") && plan::contains_ci(stderr, "polkit")
}

fn last_line(text: &str) -> &str {
    text.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or_default()
}

impl Launcher for PkexecLauncher {
    fn prepare(&self) -> Result<PathBuf, Failure> {
        use mandara_config::admin;
        if crate::updates::find_program("pkexec").is_none() {
            return Err(Failure { kind: FailureKind::NoPrompt, detail: t("pkexec is not installed").to_string() });
        }
        if let Some(exe) = admin::admin_exe()
            && helper::helper_speaks_protocol(&exe)
        {
            return Ok(exe);
        }
        // No helper yet, or an older one that doesn't know this action:
        // copy this binary into place (one approval, as Studio does).
        let me = std::env::current_exe().map_err(|e| Failure { kind: FailureKind::Helper, detail: e.to_string() })?;
        let (prog, args) = admin::setup_command(&me);
        let out = Command::new(prog).args(args).stdin(Stdio::null()).output().map_err(|e| Failure { kind: FailureKind::NoPrompt, detail: e.to_string() })?;
        if !out.status.success() {
            return Err(helper_failure(out.status.code(), &String::from_utf8_lossy(&out.stderr)));
        }
        admin::admin_exe().filter(|exe| helper::helper_speaks_protocol(exe)).ok_or_else(|| Failure { kind: FailureKind::Helper, detail: t("The admin helper couldn't be installed, so nothing was changed.").to_string() })
    }

    fn root(&self, admin: &Path, args: &[String]) -> std::io::Result<RootProc> {
        let (prog, argv) = mandara_config::admin::pkexec(admin, std::iter::once("system-update".into()).chain(args.iter().map(Into::into)));
        let mut child = Command::new(prog).args(argv).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let missing = || std::io::Error::other("pipe");
        let stdout = Box::new(child.stdout.take().ok_or_else(missing)?);
        let stderr = Box::new(child.stderr.take().ok_or_else(missing)?);
        let stdin = Box::new(child.stdin.take().ok_or_else(missing)?);
        Ok(RootProc { stdout, stderr, stdin, wait: Box::new(move || Ok(child.wait()?.code())) })
    }

    fn user(&self) -> &dyn Runner {
        &UserRunner
    }
}

/// `~/.cache/mandara/system-update.log`: the whole output of the last job.
pub fn log_path() -> PathBuf {
    mandara_config::paths::cache_dir().join("system-update.log")
}

/// Starts `request` on its own worker thread; `changed` is called (from
/// that thread) after state changes, and should only schedule a redraw.
pub fn start(request: Request, launcher: Arc<dyn Launcher>, changed: Arc<dyn Fn() + Send + Sync>) -> Job {
    let state = Arc::new(Mutex::new(JobState::new(request.clone())));
    let (tx, rx) = std::sync::mpsc::channel();
    let job = Job { state: state.clone(), cancel: Arc::new(Mutex::new(Some(tx))), user_cancel: CancelSignal::default() };
    let worker = job.clone();
    let spawned = std::thread::Builder::new().name("mandara-sysupdate".into()).spawn(move || {
        let started = Instant::now();
        let log_file = std::fs::File::create(log_path()).ok().map(std::io::BufWriter::new);
        let mut w = Worker { job: &worker, changed: &*changed, log_file, stderr_tail: VecDeque::with_capacity(STDERR_LINES) };
        let finished = w.run(&request, &*launcher, rx, started);
        if let Some(f) = w.log_file.as_mut() {
            let _ = f.flush();
        }
        // The cancel channel goes with the worker.
        worker.cancel.lock().unwrap_or_else(|e| e.into_inner()).take();
        w.update(|s| s.finished = Some(finished));
    });
    if let Err(e) = spawned {
        let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
        s.finished = Some(Finished { outcome: JobOutcome::Failed(Failure { kind: FailureKind::Other, detail: e.to_string() }), elapsed_secs: 0, changed: 0, restart: None, freed: None, inputs: Vec::new(), warnings: 0 });
    }
    job
}

struct Worker<'a> {
    job: &'a Job,
    changed: &'a (dyn Fn() + Send + Sync),
    log_file: Option<std::io::BufWriter<std::fs::File>>,
    stderr_tail: VecDeque<String>,
}

impl Worker<'_> {
    fn update(&mut self, f: impl FnOnce(&mut JobState)) {
        {
            let mut s = self.job.state.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut s);
            s.revision += 1;
        }
        (self.changed)();
    }

    fn log(&mut self, s: &mut JobState, line: &str) {
        s.push_log(line);
        if let Some(f) = self.log_file.as_mut() {
            let _ = writeln!(f, "{line}");
        }
    }

    fn run(&mut self, request: &Request, launcher: &dyn Launcher, cancel_rx: std::sync::mpsc::Receiver<()>, started: Instant) -> Finished {
        let mut finished = Finished { outcome: JobOutcome::Done { nothing_to_do: true }, elapsed_secs: 0, changed: 0, restart: None, freed: None, inputs: Vec::new(), warnings: 0 };
        let nix_paths = launcher.nix_paths();
        // A NixOS update: the flake's inputs before, a user-owned flake
        // updated as its owner first.
        let mut lock_before = None;
        let mut user_flake: Option<PathBuf> = None;
        if let Request::Nix { action: NixAction::Update(_) } = request
            && let Setup::Flake { dir, root_owned, .. } = nixos::setup(&nix_paths)
        {
            lock_before = Some((dir.join("flake.lock"), std::fs::read_to_string(dir.join("flake.lock")).map(|t| nixos::locked_inputs(&t)).unwrap_or_default()));
            if !root_owned {
                user_flake = Some(dir);
            }
        }
        let root_args = request.root_args();
        let extra_steps = u32::from(request.flatpak()) + u32::from(user_flake.is_some());
        self.update(|s| s.steps = extra_steps);

        if let Some(dir) = user_flake {
            let step = nixos::user_flake_update(&dir);
            if let Err(f) = self.user_step(launcher, &step) {
                finished.outcome = f;
                return self.finish(finished, started, launcher);
            }
        }

        if let Some(args) = root_args {
            self.update(|s| s.tracker.phase = Phase::Authorizing);
            let admin = match launcher.prepare() {
                Ok(a) => a,
                Err(f) => {
                    finished.outcome = JobOutcome::Failed(f);
                    return self.finish(finished, started, launcher);
                }
            };
            match self.root_part(launcher, &admin, &args, cancel_rx) {
                Ok(()) => {}
                Err(outcome) => {
                    finished.outcome = outcome;
                    return self.finish(finished, started, launcher);
                }
            }
        }

        if request.flatpak() {
            let step = plan::flatpak_step();
            if launcher.has_flatpak()
                && let Err(outcome) = self.user_step(launcher, &step)
            {
                finished.outcome = outcome;
                return self.finish(finished, started, launcher);
            }
        }

        if let Some((lock, before)) = lock_before {
            let after = std::fs::read_to_string(lock).map(|t| nixos::locked_inputs(&t)).unwrap_or_default();
            finished.inputs = nixos::input_changes(&before, &after);
        }
        // A NixOS action always does something (switching is not counted
        // in packages).
        let nothing = self.job.with(|s| s.tracker.changed == 0 && s.tracker.built == 0) && !matches!(request, Request::Nix { .. });
        finished.outcome = JobOutcome::Done { nothing_to_do: nothing && finished.inputs.is_empty() };
        self.finish(finished, started, launcher)
    }

    fn finish(&mut self, mut f: Finished, started: Instant, launcher: &dyn Launcher) -> Finished {
        f.elapsed_secs = started.elapsed().as_secs();
        let (kernel, modules, flag) = launcher.restart_facts();
        let (changed, restart, freed, warnings, reboot) = self.job.with(|s| (s.tracker.changed, s.tracker.restart_package.clone(), s.tracker.freed.clone(), s.tracker.warnings, s.tracker.reboot));
        f.changed = changed;
        f.warnings = warnings;
        f.freed = Some(freed).filter(|s| !s.is_empty());
        f.restart = restart.or_else(|| (reboot || flag.exists()).then(|| t("The package manager asks for a restart").to_string())).or_else(|| {
            (changed > 0 && super::kernel_modules_missing(&kernel, &modules)).then(|| t("The running kernel was replaced").to_string())
        });
        if matches!(f.outcome, JobOutcome::Cancelled | JobOutcome::Failed(_)) && f.changed == 0 {
            f.restart = None;
        }
        f
    }

    /// Runs a rootless step through the launcher's user runner.
    fn user_step(&mut self, launcher: &dyn Launcher, step: &Step) -> Result<(), JobOutcome> {
        let kind = step.kind;
        let cancel = self.job.user_cancel.clone();
        self.update(|s| {
            s.step += 1;
            s.steps = s.steps.max(s.step);
            s.step_kind = Some(kind);
            s.tracker.begin_step(kind);
            s.cancel_refused = false;
            // A request the root helper never answered was for its step,
            // not this one.
            if !s.in_user_step {
                s.in_user_step = true;
                s.cancel_requested = false;
                cancel.clear();
            }
        });
        let mut tail: VecDeque<String> = VecDeque::with_capacity(16);
        let job = self.job;
        let changed = self.changed;
        let log_file = &mut self.log_file;
        let result = launcher.user().run(
            step,
            &mut |line| {
                let line = protocol::clean_line(line);
                {
                    let mut s = job.state.lock().unwrap_or_else(|e| e.into_inner());
                    match kind {
                        StepKind::Flatpak => flatpak_line(&line, &mut s.tracker),
                        _ => plan::backend(BackendId::Nix).parse(kind, &line, &mut s.tracker),
                    }
                    s.push_log(&line);
                    s.revision += 1;
                }
                if let Some(f) = log_file.as_mut() {
                    let _ = writeln!(f, "{line}");
                }
                if tail.len() == 16 {
                    tail.pop_front();
                }
                tail.push_back(line.into_owned());
                changed();
            },
            &cancel,
        );
        self.update(|s| s.step_kind = None);
        match result {
            Ok(Some(0)) => Ok(()),
            _ if cancel.requested() => Err(JobOutcome::Cancelled),
            Ok(code) => {
                let lines: Vec<&str> = tail.iter().map(String::as_str).collect();
                let kind = plan::classify_with(&[], &lines);
                Err(JobOutcome::Failed(Failure { kind, detail: lines.iter().rfind(|l| !l.trim().is_empty()).map_or_else(|| format!("exit {code:?}"), |l| l.to_string()) }))
            }
            Err(e) => Err(JobOutcome::Failed(Failure { kind: FailureKind::Other, detail: e.to_string() })),
        }
    }

    /// Runs the root helper and follows its output.
    fn root_part(&mut self, launcher: &dyn Launcher, admin: &Path, args: &[String], cancel_rx: std::sync::mpsc::Receiver<()>) -> Result<(), JobOutcome> {
        let proc = launcher.root(admin, args).map_err(|e| JobOutcome::Failed(Failure { kind: FailureKind::NoPrompt, detail: e.to_string() }))?;
        let RootProc { stdout, stderr, mut stdin, wait } = proc;
        // Cancel requests go to the helper's standard input.
        let cancel_thread = std::thread::Builder::new().name("mandara-sysupdate-cancel".into()).spawn(move || {
            while cancel_rx.recv().is_ok() {
                if writeln!(stdin, "{}", protocol::CANCEL).and_then(|_| stdin.flush()).is_err() {
                    break;
                }
            }
        });
        let (err_tx, err_rx) = std::sync::mpsc::sync_channel::<String>(64);
        let err_thread = std::thread::spawn(move || helper::read_lines(stderr, &mut |l| err_tx.send(l.to_string()).is_ok()));
        let base_steps = self.job.with(|s| s.steps);
        let mut done: Option<Outcome> = None;
        let user_cancel = self.job.user_cancel.clone();
        helper::read_lines(stdout, &mut |raw| {
            while let Ok(e) = err_rx.try_recv() {
                self.stderr_line(&e);
            }
            if let Some(mark) = Mark::parse(raw) {
                match mark {
                    Mark::Begin { backend, steps } => self.update(|s| {
                        s.backend = BackendId::from_id(&backend);
                        s.steps = base_steps + steps;
                        s.tracker.phase = Phase::Preparing;
                    }),
                    Mark::Step { kind, .. } => {
                        self.update(|s| {
                            // A root step owns cancels again, even after a
                            // rootless one (your NixOS flake's update): a
                            // request left for that step doesn't carry over.
                            if s.in_user_step {
                                s.in_user_step = false;
                                s.cancel_requested = false;
                                user_cancel.clear();
                            }
                            s.step += 1;
                            // A step inserted by the plan (a retry) grows it.
                            s.steps = s.steps.max(s.step);
                            s.step_kind = Some(kind);
                            s.tracker.begin_step(kind);
                        });
                    }
                    Mark::Exit { .. } => self.update(|s| s.step_kind = None),
                    Mark::Cancel { accepted } => {
                        if !accepted {
                            // Refused for the root part: it mustn't stop
                            // the user steps after it (Flatpak) either.
                            self.job.user_cancel.clear();
                        }
                        self.update(|s| {
                            if !accepted {
                                s.cancel_requested = false;
                                s.cancel_refused = true;
                            }
                        })
                    }
                    Mark::Reboot => self.update(|s| s.tracker.reboot = true),
                    Mark::Done(o) => done = Some(o),
                }
                return true;
            }
            let line = protocol::clean_line(raw);
            let mut show = None;
            {
                let mut s = self.job.state.lock().unwrap_or_else(|e| e.into_inner());
                let backend = s.backend;
                let kind = s.step_kind.unwrap_or(StepKind::Install);
                if let Some(b) = backend {
                    plan::backend(b).parse(kind, &line, &mut s.tracker);
                }
                if let Some(text) = display(backend, &line) {
                    show = Some(text.into_owned());
                }
                s.revision += 1;
            }
            if let Some(text) = show {
                let mut s = self.job.state.lock().unwrap_or_else(|e| e.into_inner());
                for l in text.lines() {
                    s.push_log(l);
                    if let Some(f) = self.log_file.as_mut() {
                        let _ = writeln!(f, "{l}");
                    }
                }
            }
            (self.changed)();
            true
        });
        let _ = err_thread.join();
        while let Ok(e) = err_rx.try_recv() {
            self.stderr_line(&e);
        }
        let code = wait().ok().flatten();
        // The worker's sender is dropped when the job ends; the cancel
        // thread then stops on its own.
        drop(cancel_thread);
        let stderr: String = self.stderr_tail.iter().map(String::as_str).collect::<Vec<_>>().join("\n");
        match done {
            Some(Outcome::Ok) => Ok(()),
            Some(Outcome::Cancelled) => Err(JobOutcome::Cancelled),
            Some(Outcome::Failed(kind)) => {
                let detail = self.job.with(|s| s.log.iter().rev().find(|l| !l.trim().is_empty()).cloned().unwrap_or_default());
                Err(JobOutcome::Failed(Failure { kind, detail }))
            }
            // No answer from the helper: pkexec itself stopped it.
            None => Err(JobOutcome::Failed(helper_failure(code, &stderr))),
        }
    }

    fn stderr_line(&mut self, line: &str) {
        let line = protocol::clean_line(line).into_owned();
        if self.stderr_tail.len() == STDERR_LINES {
            self.stderr_tail.pop_front();
        }
        self.stderr_tail.push_back(line.clone());
        let mut s = self.job.state.lock().unwrap_or_else(|e| e.into_inner());
        self.log(&mut s, &line);
    }
}

/// The text a helper line shows in the log (`None`: machine-only, such as
/// apt's status lines and nix's progress numbers).
fn display(backend: Option<BackendId>, line: &str) -> Option<std::borrow::Cow<'_, str>> {
    match backend {
        Some(BackendId::Apt) if line.starts_with("dlstatus:") || line.starts_with("pmstatus:") => None,
        Some(BackendId::Nix) => {
            let mut scratch = Tracker::default();
            match nixos::nix_line(line, &mut scratch) {
                nixos::NixLine::Show(t) => Some(t),
                nixos::NixLine::Hide => None,
            }
        }
        _ if line.trim().is_empty() => None,
        _ => Some(std::borrow::Cow::Borrowed(line)),
    }
}

/// `flatpak update --noninteractive`: "Updating <ref>" / "Installing
/// <ref>" per operation, "Nothing to do." when current.
pub fn flatpak_line(line: &str, t: &mut Tracker) {
    let l = line.trim();
    if l == "Nothing to do." {
        t.nothing_to_do = true;
        return;
    }
    for verb in ["Updating ", "Installing "] {
        if let Some(r) = l.strip_prefix(verb)
            && (r.contains('/') || r.contains('.'))
        {
            t.installed += 1;
            t.changed += 1;
            t.total = t.total.max(t.installed);
            t.set_current(r.trim_end_matches('…').trim_end_matches("..."));
            return;
        }
    }
}

/// How long the UI waits between redraws while a job runs.
pub const REDRAW_EVERY: Duration = Duration::from_millis(100);

#[cfg(test)]
mod tests;
