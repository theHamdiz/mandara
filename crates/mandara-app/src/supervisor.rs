//! The restart supervisor: `mandara` with no subcommand.
//!
//! It runs `mandara run` as a child and brings it back when it dies, and
//! also when it stops answering: a watchdog probes the shell's IPC socket
//! (the probe is answered by the shell's UI thread, see
//! [`ipc::WATCHDOG_COMMAND`]) and kills a shell that misses several probes in
//! a row, which then restarts like any other crash. Shutdown asks the shell to
//! stop with `SIGTERM` and kills it if it has not exited within
//! [`SHUTDOWN_GRACE`].

use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use mandara_config::{ConfigStore, paths};

use crate::ipc;

/// Exit code a shell uses to ask its supervisor for an immediate restart.
pub const RESTART_CODE: i32 = 75;
/// Crashes are counted over this window…
const CRASH_WINDOW: Duration = Duration::from_secs(60);
/// …and supervision gives up after more than this many in it.
const MAX_CRASHES: usize = 6;
/// Restart delay after the first crash; it doubles with each further one…
const BACKOFF_FIRST: Duration = Duration::from_millis(250);
/// …up to this.
const BACKOFF_MAX: Duration = Duration::from_secs(5);
/// Restarts the shell asked for itself (`mandara msg restart`) that come back
/// at once; further ones inside [`CRASH_WINDOW`] are slowed down like crashes,
/// so a shell that exits with [`RESTART_CODE`] on every start cannot spin.
const FREE_RESTARTS: usize = 2;
/// How often the running shell is probed.
const PROBE_INTERVAL: Duration = Duration::from_secs(5);
/// How long one probe may take before it counts as unanswered.
const PROBE_TIMEOUT: Duration = Duration::from_secs(6);
/// Consecutive unanswered probes after which the shell is killed.
const MAX_MISSES: u32 = 3;
/// A shell that has not answered yet gets this long to start before its
/// silence counts.
const STARTUP_GRACE: Duration = Duration::from_secs(90);
/// After `SIGTERM`, a shell that is still running is killed after this many
/// seconds.
const SHUTDOWN_GRACE: libc::c_uint = 8;

/// What one probe of the shell found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Probe {
    /// The shell replied (a refusal counts).
    Answered,
    /// No reply in time, or nobody listening.
    Silent,
}

/// What the watchdog makes of the latest probe.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// Healthy, or still starting.
    Alive,
    /// This many probes in a row have gone unanswered.
    Missed(u32),
    /// Too many in a row: kill the shell.
    Hung,
}

/// Counts unanswered probes of one shell run.
struct Watchdog {
    started: Instant,
    /// Whether the shell has answered at least once: from then on its
    /// silence counts at once.
    answered: bool,
    misses: u32,
}

impl Watchdog {
    fn new(started: Instant) -> Self {
        Self { started, answered: false, misses: 0 }
    }

    fn observe(&mut self, probe: Probe, now: Instant) -> Verdict {
        match probe {
            Probe::Answered => {
                self.answered = true;
                self.misses = 0;
                Verdict::Alive
            }
            // Starting up (the UI thread is not running yet): not a hang.
            Probe::Silent if !self.answered && now.duration_since(self.started) < STARTUP_GRACE => Verdict::Alive,
            Probe::Silent => {
                self.misses += 1;
                if self.misses >= MAX_MISSES { Verdict::Hung } else { Verdict::Missed(self.misses) }
            }
        }
    }

    /// Starts counting again (the shell was left running).
    fn forgive(&mut self) {
        self.misses = 0;
    }
}

/// Runs the probes off the supervisor's thread: connecting to a socket whose
/// owner has stopped accepting can block, and the supervisor must still get
/// to kill that shell. A probe that has not finished is waited for again
/// instead of starting another thread beside it.
#[derive(Default)]
struct Prober {
    pending: Option<mpsc::Receiver<Probe>>,
}

impl Prober {
    fn probe(&mut self, timeout: Duration, run: impl FnOnce() -> Probe + Send + 'static) -> Probe {
        let rx = match self.pending.take() {
            Some(rx) => rx,
            None => {
                let (tx, rx) = mpsc::channel();
                let spawned = std::thread::Builder::new().name("mandara-watchdog-probe".into()).spawn(move || {
                    let _ = tx.send(run());
                });
                if let Err(err) = spawned {
                    tracing::warn!("watchdog: could not start a probe thread: {err}");
                    return Probe::Silent;
                }
                rx
            }
        };
        match rx.recv_timeout(timeout) {
            Ok(probe) => probe,
            Err(RecvTimeoutError::Timeout) => {
                self.pending = Some(rx);
                Probe::Silent
            }
            Err(RecvTimeoutError::Disconnected) => Probe::Silent,
        }
    }
}

/// One probe over the IPC socket.
fn probe_socket(socket: &std::path::Path) -> Probe {
    probe_of(ipc::send(socket, &[ipc::WATCHDOG_COMMAND.to_string()]))
}

/// Any reply proves the shell's IPC thread is alive, including a refusal
/// ("too busy", "too many requests"): a shell saturated with other clients
/// must not be killed. Only a timeout or a failed connection is a miss.
fn probe_of<E>(reply: Result<ipc::WireResponse, E>) -> Probe {
    if reply.is_ok() { Probe::Answered } else { Probe::Silent }
}

/// How long to wait before the next start, or that supervision gives up.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    Restart(Duration),
    /// Crashes in the window.
    GiveUp(usize),
}

/// Remembers recent crashes and restart requests.
#[derive(Default)]
struct Restarts {
    crashes: Vec<Instant>,
    requested: Vec<Instant>,
}

fn backoff(doublings: usize) -> Duration {
    let doublings = u32::try_from(doublings).unwrap_or(u32::MAX);
    BACKOFF_FIRST.saturating_mul(2u32.saturating_pow(doublings)).min(BACKOFF_MAX)
}

fn record(events: &mut Vec<Instant>, now: Instant) -> usize {
    events.retain(|t| now.duration_since(*t) < CRASH_WINDOW);
    events.push(now);
    events.len()
}

impl Restarts {
    /// The shell crashed (or was killed as unresponsive).
    fn crashed(&mut self, now: Instant) -> Next {
        let count = record(&mut self.crashes, now);
        if count > MAX_CRASHES { Next::GiveUp(count) } else { Next::Restart(backoff(count - 1)) }
    }

    /// The shell asked to be restarted: immediately at first, then slowed
    /// down; it never gives up, since a person can legitimately restart often.
    fn requested(&mut self, now: Instant) -> Duration {
        let count = record(&mut self.requested, now);
        if count <= FREE_RESTARTS { Duration::ZERO } else { backoff(count - FREE_RESTARTS - 1) }
    }
}

/// Sleeps for `duration`, returning early when `stop` is set by a signal
/// (`nanosleep` is cut short by the signal handler).
fn sleep_unless_stopped(duration: Duration, stop: &std::sync::atomic::AtomicBool) {
    use std::sync::atomic::Ordering;
    // Avoid `libc::time_t`: deprecated on musl (width will change); `tv_sec`
    // still accepts the same integer via `as _` on every libc we ship.
    let mut left = libc::timespec {
        tv_sec: i64::try_from(duration.as_secs()).unwrap_or(i64::MAX) as _,
        tv_nsec: libc::c_long::try_from(duration.subsec_nanos()).unwrap_or(0),
    };
    while !stop.load(Ordering::SeqCst) {
        let request = left;
        // SAFETY: both pointers refer to live timespecs.
        if unsafe { libc::nanosleep(&request, &mut left) } == 0 {
            return;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return;
        }
    }
}

/// Runs `mandara run` as a child and restarts it whenever it exits
/// abnormally (panic that escaped, signal, GPU loss…) or stops answering. A
/// clean exit (`mandara msg quit`) ends supervision; too many crashes in a
/// short time stop restarts so a broken build can't spin forever.
pub fn supervise() -> Result<()> {
    use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

    static STOP: AtomicBool = AtomicBool::new(false);
    /// The running shell's pid (0 between runs), for the signal handlers.
    static CHILD: AtomicI32 = AtomicI32::new(0);
    /// Asks the shell to stop and schedules its kill.
    fn terminate(pid: libc::pid_t) {
        // SAFETY: kill(2) and alarm(2) are async-signal-safe; the pid is our own child.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
            libc::alarm(SHUTDOWN_GRACE);
        }
    }
    extern "C" fn on_signal(_: libc::c_int) {
        STOP.store(true, Ordering::SeqCst);
        let pid = CHILD.load(Ordering::SeqCst);
        if pid > 0 {
            terminate(pid);
        }
    }
    extern "C" fn on_alarm(_: libc::c_int) {
        let pid = CHILD.load(Ordering::SeqCst);
        if pid > 0 {
            // SAFETY: kill(2) is async-signal-safe; the pid is our own child.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
    // SAFETY: the handlers only touch atomics and async-signal-safe calls.
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGHUP, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGALRM, on_alarm as *const () as libc::sighandler_t);
    }
    let exe = paths::self_exe();
    let socket = paths::ipc_socket();
    let mut restarts = Restarts::default();
    loop {
        // A stop that arrived while waiting out a delay: no new shell.
        if STOP.load(Ordering::SeqCst) {
            return Ok(());
        }
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("run").env("MANDARA_SUPERVISED", "1");
        // SAFETY: prctl/procctl in the forked child before exec; no allocation.
        unsafe {
            cmd.pre_exec(|| {
                // The shell dies with its supervisor.
                #[cfg(target_os = "linux")]
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                #[cfg(target_os = "freebsd")]
                {
                    let mut signal = libc::SIGTERM;
                    libc::procctl(libc::P_PID, 0, libc::PROC_PDEATHSIG_CTL, (&raw mut signal).cast());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn().context("starting the shell")?;
        let started = Instant::now();
        let pid = libc::pid_t::try_from(child.id()).context("shell pid out of range")?;
        CHILD.store(pid, Ordering::SeqCst);
        // A signal that landed before the pid was published.
        if STOP.load(Ordering::SeqCst) {
            terminate(pid);
        }
        // The exit is reaped on its own thread so this one can probe the
        // shell meanwhile; nothing wakes this thread while the shell is fine
        // except the probe interval.
        let (exited_tx, exited) = mpsc::channel();
        std::thread::Builder::new()
            .name("mandara-supervisor-wait".into())
            .spawn(move || {
                let _ = exited_tx.send(child.wait());
            })
            .context("watching the shell")?;
        let mut watchdog = Watchdog::new(started);
        let mut prober = Prober::default();
        let mut killed = false;
        let status = loop {
            match exited.recv_timeout(PROBE_INTERVAL) {
                Ok(status) => break status.context("waiting for the shell")?,
                Err(RecvTimeoutError::Disconnected) => anyhow::bail!("the thread waiting for the shell ended unexpectedly"),
                Err(RecvTimeoutError::Timeout) => {}
            }
            // Stopping, or already killed and about to be reaped.
            if STOP.load(Ordering::SeqCst) || killed {
                continue;
            }
            let socket = socket.clone();
            let probe = prober.probe(PROBE_TIMEOUT, move || probe_socket(&socket));
            match watchdog.observe(probe, Instant::now()) {
                Verdict::Alive => {}
                Verdict::Missed(misses) => tracing::warn!("Mandara did not answer the watchdog ({misses} of {MAX_MISSES})"),
                Verdict::Hung => {
                    if ConfigStore::open_default().map(|s| s.get().general.auto_restart).unwrap_or(true) {
                        tracing::error!("Mandara stopped answering ({MAX_MISSES} watchdog probes in a row); killing it");
                        // SAFETY: plain kill(2) on our own child, which is not reaped before its exit is received.
                        unsafe {
                            libc::kill(pid, libc::SIGKILL);
                        }
                        killed = true;
                    } else {
                        tracing::warn!("Mandara has stopped answering; auto-restart is off (Studio → General), so it is left running");
                        watchdog.forgive();
                    }
                }
            }
        };
        CHILD.store(0, Ordering::SeqCst);
        if STOP.load(Ordering::SeqCst) {
            return Ok(());
        }
        if status.success() {
            return Ok(());
        }
        // `mandara msg restart`: come back at once (a few times in a row).
        if status.code() == Some(RESTART_CODE) {
            sleep_unless_stopped(restarts.requested(Instant::now()), &STOP);
            continue;
        }
        let auto_restart = ConfigStore::open_default().map(|s| s.get().general.auto_restart).unwrap_or(true);
        if !auto_restart {
            tracing::warn!("Mandara exited abnormally; auto-restart is off (Studio → General), so it stays stopped");
            return Ok(());
        }
        let why = match (killed, status.code(), status.signal()) {
            (true, ..) => "it stopped answering".to_string(),
            (_, _, Some(sig)) => format!("signal {sig}"),
            (_, Some(code), _) => format!("exit code {code}"),
            _ => "unknown reason".into(),
        };
        match restarts.crashed(Instant::now()) {
            Next::GiveUp(crashes) => {
                tracing::error!("Mandara crashed {crashes} times in a minute ({why}); not restarting. Run `mandara run` to see the error.");
                anyhow::bail!("shell crash loop");
            }
            Next::Restart(delay) => {
                tracing::warn!("Mandara exited ({why}); restarting in {} ms", delay.as_millis());
                sleep_unless_stopped(delay, &STOP);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn at(start: Instant, secs: u64) -> Instant {
        start + Duration::from_secs(secs)
    }

    #[test]
    fn a_silent_shell_is_left_alone_while_it_starts() {
        let start = Instant::now();
        let mut dog = Watchdog::new(start);
        for secs in (0..STARTUP_GRACE.as_secs()).step_by(5) {
            assert_eq!(dog.observe(Probe::Silent, at(start, secs)), Verdict::Alive);
        }
    }

    #[test]
    fn silence_after_the_grace_period_counts_even_if_it_never_answered() {
        let start = Instant::now();
        let mut dog = Watchdog::new(start);
        let late = STARTUP_GRACE.as_secs();
        assert_eq!(dog.observe(Probe::Silent, at(start, late)), Verdict::Missed(1));
        assert_eq!(dog.observe(Probe::Silent, at(start, late + 5)), Verdict::Missed(2));
        assert_eq!(dog.observe(Probe::Silent, at(start, late + 10)), Verdict::Hung);
    }

    #[test]
    fn once_it_has_answered_silence_counts_at_once_and_an_answer_resets_the_count() {
        let start = Instant::now();
        let mut dog = Watchdog::new(start);
        assert_eq!(dog.observe(Probe::Answered, at(start, 1)), Verdict::Alive);
        for round in 0..3 {
            let base = 10 + round * 100;
            assert_eq!(dog.observe(Probe::Silent, at(start, base)), Verdict::Missed(1));
            assert_eq!(dog.observe(Probe::Silent, at(start, base + 5)), Verdict::Missed(2));
            assert_eq!(dog.observe(Probe::Answered, at(start, base + 10)), Verdict::Alive);
        }
        assert_eq!(dog.observe(Probe::Silent, at(start, 500)), Verdict::Missed(1));
        assert_eq!(dog.observe(Probe::Silent, at(start, 505)), Verdict::Missed(2));
        assert_eq!(dog.observe(Probe::Silent, at(start, 510)), Verdict::Hung);
    }

    #[test]
    fn a_forgiven_shell_needs_a_full_run_of_misses_again() {
        let start = Instant::now();
        let mut dog = Watchdog::new(start);
        dog.observe(Probe::Answered, start);
        for secs in [10, 15] {
            dog.observe(Probe::Silent, at(start, secs));
        }
        assert_eq!(dog.observe(Probe::Silent, at(start, 20)), Verdict::Hung);
        dog.forgive();
        assert_eq!(dog.observe(Probe::Silent, at(start, 25)), Verdict::Missed(1));
    }

    #[test]
    fn crashes_back_off_and_give_up_after_the_cap() {
        let start = Instant::now();
        let mut restarts = Restarts::default();
        assert_eq!(restarts.crashed(at(start, 0)), Next::Restart(Duration::from_millis(250)));
        assert_eq!(restarts.crashed(at(start, 1)), Next::Restart(Duration::from_millis(500)));
        assert_eq!(restarts.crashed(at(start, 2)), Next::Restart(Duration::from_secs(1)));
        assert_eq!(restarts.crashed(at(start, 3)), Next::Restart(Duration::from_secs(2)));
        assert_eq!(restarts.crashed(at(start, 4)), Next::Restart(Duration::from_secs(4)));
        assert_eq!(restarts.crashed(at(start, 5)), Next::Restart(BACKOFF_MAX));
        assert_eq!(restarts.crashed(at(start, 6)), Next::GiveUp(MAX_CRASHES + 1));
    }

    #[test]
    fn old_crashes_leave_the_window() {
        let start = Instant::now();
        let mut restarts = Restarts::default();
        for secs in 0..MAX_CRASHES as u64 {
            restarts.crashed(at(start, secs));
        }
        let later = CRASH_WINDOW.as_secs() + MAX_CRASHES as u64;
        assert_eq!(restarts.crashed(at(start, later)), Next::Restart(BACKOFF_FIRST));
    }

    #[test]
    fn requested_restarts_are_free_at_first_then_throttled_and_never_give_up() {
        let start = Instant::now();
        let mut restarts = Restarts::default();
        assert_eq!(restarts.requested(at(start, 0)), Duration::ZERO);
        assert_eq!(restarts.requested(at(start, 1)), Duration::ZERO);
        assert_eq!(restarts.requested(at(start, 2)), BACKOFF_FIRST);
        assert_eq!(restarts.requested(at(start, 3)), BACKOFF_FIRST * 2);
        for secs in 4..40 {
            assert!(restarts.requested(at(start, secs)) <= BACKOFF_MAX);
        }
        assert_eq!(restarts.requested(at(start, 41)), BACKOFF_MAX);
        // A calm minute later it is immediate again, and crashes are counted apart.
        assert_eq!(restarts.requested(at(start, 200)), Duration::ZERO);
        assert_eq!(restarts.crashed(at(start, 200)), Next::Restart(BACKOFF_FIRST));
    }

    #[test]
    fn a_probe_that_never_finishes_is_waited_for_not_repeated() {
        let spawned = Arc::new(AtomicUsize::new(0));
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let mut prober = Prober::default();
        let count = spawned.clone();
        assert_eq!(
            prober.probe(Duration::from_millis(30), move || {
                count.fetch_add(1, Ordering::SeqCst);
                let _ = release_rx.recv();
                Probe::Answered
            }),
            Probe::Silent
        );
        // Still blocked: the next round waits on the same probe.
        let count = spawned.clone();
        assert_eq!(
            prober.probe(Duration::from_millis(30), move || {
                count.fetch_add(1, Ordering::SeqCst);
                Probe::Answered
            }),
            Probe::Silent
        );
        release_tx.send(()).unwrap();
        assert_eq!(prober.probe(Duration::from_secs(5), || Probe::Silent), Probe::Answered);
        assert_eq!(spawned.load(Ordering::SeqCst), 1);
        // Finished: the next probe is a new one.
        assert_eq!(prober.probe(Duration::from_secs(5), || Probe::Answered), Probe::Answered);
    }

    #[test]
    fn a_refusal_is_an_answer_and_only_a_failed_exchange_is_silence() {
        assert_eq!(probe_of::<()>(Ok(ipc::WireResponse::ok(serde_json::json!("pong")))), Probe::Answered);
        assert_eq!(probe_of::<()>(Ok(ipc::WireResponse::err("the shell is too busy to take another request"))), Probe::Answered);
        assert_eq!(probe_of::<()>(Ok(ipc::WireResponse::err("too many requests at once"))), Probe::Answered);
        assert_eq!(probe_of(Err("timed out")), Probe::Silent);
    }

    #[test]
    fn a_stop_cuts_the_wait_between_restarts_short() {
        use std::sync::atomic::AtomicBool;
        let stop = AtomicBool::new(true);
        let begun = Instant::now();
        sleep_unless_stopped(Duration::from_secs(30), &stop);
        assert!(begun.elapsed() < Duration::from_secs(5));
        let go = AtomicBool::new(false);
        let begun = Instant::now();
        sleep_unless_stopped(Duration::from_millis(50), &go);
        assert!(begun.elapsed() >= Duration::from_millis(50));
    }
}
