//! The root side: `mandara system-update …`, started by the shell through
//! `pkexec <admin helper>`.
//!
//! This is a security boundary. The helper:
//!
//! * accepts only the fixed actions of [`Action::parse`]; every argument
//!   is validated (generation numbers are integers that exist, counts
//!   and day spans are bounded), and anything else is refused;
//! * detects the system itself from root-owned files, never from what
//!   the caller says;
//! * runs only the fixed argv of [`super::plan`] and [`super::nixos`],
//!   with programs from [`super::SAFE_DIRS`] that root owns, a cleared
//!   environment and no standard input;
//! * reads one request from its own standard input, `cancel`, and honours
//!   it only during a cancellable step. A closed input never stops an
//!   update half way.

use std::io::{BufRead as _, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use super::nixos::{self, NixAction};
use super::parse::Tracker;
use super::plan::{self, Next, Step, SystemBackend};
use super::protocol::{self, CANCEL, Mark, Outcome};
use super::{FailureKind, System};

/// Exit codes of the helper.
pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILED: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_NOT_ROOT: i32 = 3;
pub const EXIT_UNSUPPORTED: i32 = 4;
pub const EXIT_CANCELLED: i32 = 130;
/// Time a cancelled program gets to stop after SIGINT before SIGTERM.
const STOP_GRACE: Duration = Duration::from_secs(15);
/// How often a running step looks at the cancel flag.
const CANCEL_POLL: Duration = Duration::from_millis(100);
/// Output lines kept by the root side for its own decisions.
const TAIL_LINES: usize = 40;

/// What the helper was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Print [`protocol::PROTOCOL`] (no root needed).
    Protocol,
    /// A full system update with the detected package manager.
    Apply,
    /// A NixOS action.
    Nix(NixAction),
}

impl Action {
    /// Parses the arguments after `system-update`. Strict: unknown words,
    /// extra words and out-of-range numbers are refused.
    pub fn parse(args: &[String]) -> Result<Action, String> {
        let words: Vec<&str> = args.iter().map(String::as_str).collect();
        match words.as_slice() {
            ["--protocol"] => Ok(Action::Protocol),
            ["apply"] => Ok(Action::Apply),
            ["nixos", rest @ ..] => NixAction::parse(rest).map(Action::Nix),
            _ => Err(format!("usage: mandara system-update apply | --protocol | nixos {}", NixAction::USAGE)),
        }
    }
}

/// The cancel request, shared by the stdin watcher and the running step.
#[derive(Clone, Default)]
pub struct CancelSignal {
    requested: Arc<AtomicBool>,
    /// A cancellable step is running.
    cancellable: Arc<AtomicBool>,
}

impl CancelSignal {
    pub fn requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    pub fn request(&self) {
        self.requested.store(true, Ordering::SeqCst);
    }

    /// Accepts a request only while a cancellable step runs.
    pub fn try_request(&self) -> bool {
        let ok = self.cancellable.load(Ordering::SeqCst);
        if ok {
            self.request();
        }
        ok
    }

    fn set_cancellable(&self, yes: bool) {
        self.cancellable.store(yes, Ordering::SeqCst);
    }
}

/// Starts and waits for the steps' programs (the real one runs them;
/// tests and the sandbox replay recorded output).
pub trait Runner: Sync {
    /// Runs `step`, handing every output line to `line`, until it exits.
    /// A cancellable step stops early when `cancel` is requested.
    fn run(&self, step: &Step, line: &mut dyn FnMut(&str), cancel: &CancelSignal) -> std::io::Result<Option<i32>>;
}

/// Runs the programs as they are (as root, from the safe folders).
pub struct SystemRunner;

/// `program` in the safe folders, only when root owns it and the folders
/// above it.
pub fn trusted_program(program: &str) -> std::io::Result<PathBuf> {
    let path = super::find_safe(program).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, format!("{program} is not installed")))?;
    let real = std::fs::canonicalize(&path)?;
    if !mandara_config::admin::trusted_root_file(&real) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("{} is not owned by root", real.display())));
    }
    Ok(path)
}

/// A program in the Nix store (a generation's own tools), root-owned.
pub fn trusted_store_program(exe: &Path) -> std::io::Result<PathBuf> {
    let real = std::fs::canonicalize(exe)?;
    if !real.starts_with("/nix/store/") || !mandara_config::admin::trusted_root_file(&real) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("{} is not a root-owned program in the Nix store", real.display())));
    }
    Ok(real)
}

impl Runner for SystemRunner {
    fn run(&self, step: &Step, line: &mut dyn FnMut(&str), cancel: &CancelSignal) -> std::io::Result<Option<i32>> {
        use std::os::unix::process::CommandExt as _;
        let program = match &step.exe {
            Some(exe) => trusted_store_program(exe)?,
            None => trusted_program(step.program)?,
        };
        let mut cmd = Command::new(&program);
        cmd.args(&step.args)
            .env_clear()
            .env("PATH", super::SAFE_PATH)
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .env("HOME", "/root")
            .env("TERM", "dumb")
            .envs(step.env.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir("/")
            // Its own process group, so a cancel reaches its children too.
            .process_group(0);
        let mut child = cmd.spawn()?;
        pump(&mut child, line, cancel, step.kind.cancellable())
    }
}

/// Reads `child`'s stdout and stderr line by line (one bounded line buffer
/// per pipe) until it exits.
pub fn pump(child: &mut std::process::Child, line: &mut dyn FnMut(&str), cancel: &CancelSignal, cancellable: bool) -> std::io::Result<Option<i32>> {
    let (tx, rx) = mpsc::sync_channel::<String>(256);
    let readers: Vec<_> = [child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>)]
        .into_iter()
        .flatten()
        .map(|pipe| {
            let tx = tx.clone();
            std::thread::spawn(move || read_lines(pipe, &mut |l| tx.send(l.to_string()).is_ok()))
        })
        .collect();
    drop(tx);
    let mut signalled = None;
    loop {
        match rx.recv_timeout(CANCEL_POLL) {
            Ok(l) => line(&l),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if cancellable && cancel.requested() {
            let pid = child.id() as i32;
            match signalled {
                None => {
                    // SAFETY: signalling our own child's process group.
                    unsafe { libc::kill(-pid, libc::SIGINT) };
                    signalled = Some(std::time::Instant::now());
                }
                Some(at) if at.elapsed() > STOP_GRACE => {
                    // SAFETY: as above.
                    unsafe { libc::kill(-pid, libc::SIGTERM) };
                    signalled = Some(std::time::Instant::now());
                }
                Some(_) => {}
            }
        }
    }
    for r in readers {
        let _ = r.join();
    }
    Ok(child.wait()?.code())
}

/// Splits `pipe` into lines (`\n` or `\r`), each cut to
/// [`protocol::MAX_LINE`], without growing past it on a never-ending line.
pub fn read_lines(pipe: impl std::io::Read, each: &mut dyn FnMut(&str) -> bool) {
    let mut reader = std::io::BufReader::new(pipe);
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    loop {
        let available = match reader.fill_buf() {
            Ok([]) => break,
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let mut used = 0;
        let mut stop = false;
        for &b in available {
            used += 1;
            if b == b'\n' || b == b'\r' {
                if !buf.is_empty() && !each(&String::from_utf8_lossy(&buf)) {
                    stop = true;
                    break;
                }
                buf.clear();
            } else if buf.len() < protocol::MAX_LINE {
                buf.push(b);
            }
        }
        reader.consume(used);
        if stop {
            return;
        }
    }
    if !buf.is_empty() {
        each(&String::from_utf8_lossy(&buf));
    }
}

/// Writes lines to the caller, ignoring a closed pipe (the shell went
/// away: the update carries on).
pub struct Out<'a> {
    w: &'a mut (dyn Write + Send),
}

impl<'a> Out<'a> {
    pub fn new(w: &'a mut (dyn Write + Send)) -> Self {
        Out { w }
    }

    pub fn line(&mut self, s: &str) {
        let _ = writeln!(self.w, "{}", protocol::clean_line(s));
        let _ = self.w.flush();
    }

    pub fn mark(&mut self, m: &Mark) {
        let _ = writeln!(self.w, "{}", m.line());
        let _ = self.w.flush();
    }
}

/// Runs `steps` in order with `backend`'s rules, writing markers and the
/// output to `out`. Returns how it ended.
pub fn execute(steps: Vec<Step>, backend: &dyn SystemBackend, runner: &dyn Runner, out: &mut Out<'_>, cancel: &CancelSignal) -> Outcome {
    let mut queue: std::collections::VecDeque<Step> = steps.into();
    let mut tracker = Tracker::default();
    let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::with_capacity(TAIL_LINES);
    let mut index = 0u32;
    while let Some(step) = queue.pop_front() {
        if cancel.requested() {
            return Outcome::Cancelled;
        }
        index += 1;
        out.mark(&Mark::Step { index, kind: step.kind });
        tracker.begin_step(step.kind);
        tail.clear();
        cancel.set_cancellable(step.kind.cancellable());
        let result = runner.run(
            &step,
            &mut |l| {
                let l = protocol::clean_line(l);
                // A marker-looking line from a program is shown, not obeyed.
                if l.starts_with(protocol::MARK.trim_end()) {
                    return;
                }
                backend.parse(step.kind, &l, &mut tracker);
                if tail.len() == TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(l.to_string());
                out.line(&l);
            },
            cancel,
        );
        cancel.set_cancellable(false);
        let code = match result {
            Ok(code) => code,
            Err(e) => {
                out.line(&format!("{}: {e}", step.program));
                out.mark(&Mark::Exit { index, code: Some(127) });
                return Outcome::Failed(FailureKind::Other);
            }
        };
        out.mark(&Mark::Exit { index, code });
        if step.kind.cancellable() && cancel.requested() {
            return Outcome::Cancelled;
        }
        match backend.after(&step, code, &tracker) {
            Next::Continue => {}
            Next::Finish => break,
            Next::Insert(s) => {
                tracker.restarts += 1;
                queue.push_front(s);
            }
            Next::Fail(kind) => {
                let lines: Vec<&str> = tail.iter().map(String::as_str).collect();
                let kind = if kind == FailureKind::Other { backend.classify(&lines) } else { kind };
                return Outcome::Failed(kind);
            }
        }
    }
    if tracker.reboot {
        out.mark(&Mark::Reboot);
    }
    Outcome::Ok
}

/// Watches standard input for `cancel` and answers on `out`.
fn watch_stdin(cancel: CancelSignal, out: Arc<std::sync::Mutex<Box<dyn Write + Send>>>) {
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            if line.trim() != CANCEL {
                continue;
            }
            let accepted = cancel.try_request();
            if let Ok(mut w) = out.lock() {
                let _ = writeln!(w, "{}", Mark::Cancel { accepted }.line());
                let _ = w.flush();
            }
        }
    });
}

/// A shared writer, so the stdin watcher's answers don't tear lines.
struct SharedOut(Arc<std::sync::Mutex<Box<dyn Write + Send>>>);

impl Write for SharedOut {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().map_err(|_| std::io::Error::other("poisoned"))?.write(buf)
    }

    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.0.lock().map_err(|_| std::io::Error::other("poisoned"))?.write_all(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().map_err(|_| std::io::Error::other("poisoned"))?.flush()
    }
}

fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

/// `mandara system-update <args>`: the process exit code.
pub fn main(args: &[String]) -> i32 {
    let action = match Action::parse(args) {
        Ok(a) => a,
        Err(usage) => {
            eprintln!("{usage}");
            return EXIT_USAGE;
        }
    };
    if action == Action::Protocol {
        println!("{}", protocol::PROTOCOL);
        return EXIT_OK;
    }
    if !is_root() {
        eprintln!("mandara system-update must run as root (through pkexec)");
        return EXIT_NOT_ROOT;
    }
    let shared: Arc<std::sync::Mutex<Box<dyn Write + Send>>> = Arc::new(std::sync::Mutex::new(Box::new(std::io::stdout())));
    let cancel = CancelSignal::default();
    watch_stdin(cancel.clone(), shared.clone());
    let mut writer = SharedOut(shared);
    let mut out = Out::new(&mut writer);
    run_action(&action, System::detect(), &SystemRunner, &mut out, &cancel, &nixos::NixPaths::system())
}

/// Plans and runs `action` on `system`; writes the `begin` and `done`
/// markers. The exit code of the helper.
pub fn run_action(action: &Action, system: System, runner: &dyn Runner, out: &mut Out<'_>, cancel: &CancelSignal, nix: &nixos::NixPaths) -> i32 {
    let (backend, steps): (&dyn SystemBackend, Vec<Step>) = match (action, system) {
        (Action::Apply, System::Managed(id)) => {
            let b = plan::backend(id);
            (b, b.steps())
        }
        (Action::Nix(nix_action), System::NixOs) => match nixos::plan(nix_action, nix) {
            Ok(steps) => (&nixos::NixBackend, steps),
            Err(e) => {
                out.line(&e);
                out.mark(&Mark::Done(Outcome::Failed(FailureKind::Other)));
                return EXIT_USAGE;
            }
        },
        (_, system) => {
            out.line(&format!("this action doesn't apply to this system ({system:?})"));
            out.mark(&Mark::Done(Outcome::Failed(FailureKind::Other)));
            return EXIT_UNSUPPORTED;
        }
    };
    out.mark(&Mark::Begin { backend: backend_id(backend), steps: steps.len() as u32 });
    let outcome = execute(steps, backend, runner, out, cancel);
    out.mark(&Mark::Done(outcome));
    match outcome {
        Outcome::Ok => EXIT_OK,
        Outcome::Cancelled => EXIT_CANCELLED,
        Outcome::Failed(_) => EXIT_FAILED,
    }
}

fn backend_id(b: &dyn SystemBackend) -> String {
    b.id().id().to_string()
}

/// Whether `path` is a root-owned admin binary that understands this
/// protocol (`--protocol` runs it as the user: no root needed).
pub fn helper_speaks_protocol(admin: &Path) -> bool {
    Command::new(admin)
        .args(["system-update", "--protocol"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u32>().ok())
        == Some(protocol::PROTOCOL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysupdate::plan::BackendId;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn only_fixed_actions_are_accepted() {
        assert_eq!(Action::parse(&words("apply")), Ok(Action::Apply));
        assert_eq!(Action::parse(&words("--protocol")), Ok(Action::Protocol));
        for bad in ["", "apply now", "apply --overwrite *", "apply; rm -rf /", "pacman -Syu", "run sh -c id", "APPLY", "--protocol apply", "nixos", "nixos rm -rf"] {
            assert!(Action::parse(&words(bad)).is_err(), "{bad:?} must be refused");
        }
        // An argument with spaces or a newline is one word, still refused.
        assert!(Action::parse(&["apply\nnixos".to_string()]).is_err());
        assert!(Action::parse(&["apply".to_string(), String::new()]).is_err());
    }

    #[test]
    fn needs_root_for_everything_but_the_protocol() {
        if is_root() {
            return;
        }
        assert_eq!(main(&words("apply")), EXIT_NOT_ROOT);
        assert_eq!(main(&words("nixos rollback")), EXIT_NOT_ROOT);
        assert_eq!(main(&words("bogus")), EXIT_USAGE);
    }

    #[test]
    fn untrusted_programs_are_refused() {
        // Built by your user, so not root-owned.
        let me = std::env::current_exe().unwrap();
        assert!(!mandara_config::admin::trusted_root_file(&me));
        assert!(trusted_program("definitely-not-installed-mandara").is_err());
        assert!(trusted_program("../../tmp/x").is_err());
    }

    #[test]
    fn lines_split_on_cr_and_lf_and_stay_bounded() {
        let mut got = Vec::new();
        let input = format!("a\r\nb\rc\n{}\nlast", "x".repeat(protocol::MAX_LINE * 3));
        read_lines(input.as_bytes(), &mut |l| {
            got.push(l.to_string());
            true
        });
        assert_eq!(got.len(), 5);
        assert_eq!(&got[..3], ["a", "b", "c"]);
        assert_eq!(got[3].len(), protocol::MAX_LINE);
        assert_eq!(got[4], "last");
    }

    #[test]
    fn unsupported_systems_are_refused() {
        let mut buf: Vec<u8> = Vec::new();
        let mut out = Out::new(&mut buf);
        let code = run_action(&Action::Apply, System::NixOs, &crate::sysupdate::fake::Replay::new(BackendId::Pacman), &mut out, &CancelSignal::default(), &nixos::NixPaths::system());
        assert_eq!(code, EXIT_UNSUPPORTED);
        let text = String::from_utf8(buf).unwrap();
        assert!(text.ends_with(&format!("{}\n", Mark::Done(Outcome::Failed(FailureKind::Other)).line())));
    }
}
