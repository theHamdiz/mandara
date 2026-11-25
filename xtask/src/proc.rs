//! Process plumbing for the headless sessions (`resource-check`,
//! `sandbox`): spawning detached or parent-bound children, reading `/proc`,
//! and signalling only processes this tool started.

use std::collections::BTreeMap;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Resource, Rlimit, Signal};

use crate::Result;

/// Largest file a session child may write (the shell's log): a runaway error
/// loop once wrote a 3.6 GB log. Same cap as `ulimit -f 200000` (KiB).
pub const MAX_FILE_BYTES: u64 = 200_000 * 1024;
/// How long a terminated process gets before it is killed outright.
const TERM_GRACE: Duration = Duration::from_secs(3);
/// Poll interval while waiting for files and processes.
pub const POLL: Duration = Duration::from_millis(100);

/// How a spawned child relates to this process.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lifetime {
    /// Dies with this process (`PR_SET_PDEATHSIG`): `resource-check`.
    Bound,
    /// Its own session, outliving this process: `sandbox up`.
    Detached,
}

/// Spawns `cmd` with stdin from /dev/null, stdout and stderr to `log`, the
/// file-size cap, and the given lifetime.
pub fn spawn(cmd: &mut Command, log: &Path, lifetime: Lifetime) -> Result<Child> {
    let out = std::fs::File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    cmd.stdin(Stdio::null()).stdout(out).stderr(err);
    // SAFETY: the closure runs between fork and exec and only makes raw
    // system calls (setrlimit, setsid, prctl), which are async-signal-safe;
    // it neither allocates nor takes locks.
    unsafe {
        cmd.pre_exec(move || {
            rustix::process::setrlimit(Resource::Fsize, Rlimit { current: Some(MAX_FILE_BYTES), maximum: Some(MAX_FILE_BYTES) })?;
            match lifetime {
                Lifetime::Bound => rustix::process::set_parent_process_death_signal(Some(Signal::TERM))?,
                Lifetime::Detached => {
                    rustix::process::setsid()?;
                }
            }
            Ok(())
        });
    }
    cmd.spawn().map_err(|e| format!("starting {:?}: {e}", cmd.get_program()))
}

/// Waits until `ready()` holds, for at most `timeout`.
pub fn wait_until(timeout: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if ready() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

/// True while `pid` exists (zombies included).
pub fn alive(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists() && stat_state(pid).is_some_and(|s| s != 'Z')
}

/// The fields of `/proc/<pid>/stat` after the command name (field 3 on).
fn stat_fields(pid: i32) -> Option<Vec<String>> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    Some(rest.split_whitespace().map(String::from).collect())
}

fn stat_state(pid: i32) -> Option<char> {
    stat_fields(pid)?.first()?.chars().next()
}

/// Index of `ppid` (stat field 4) among the fields after the name.
const STAT_PPID: usize = 1;
/// Index of `utime` (stat field 14) among the fields after the name.
const STAT_UTIME: usize = 11;
/// Index of `stime` (stat field 15) among the fields after the name.
const STAT_STIME: usize = 12;

/// User + system CPU time of `pid`, in clock ticks.
pub fn cpu_ticks(pid: i32) -> Option<u64> {
    let f = stat_fields(pid)?;
    Some(f.get(STAT_UTIME)?.parse::<u64>().ok()? + f.get(STAT_STIME)?.parse::<u64>().ok()?)
}

/// `KEY=VALUE` entries of `/proc/<pid>/environ` (empty when unreadable).
pub fn environ(pid: i32) -> Vec<String> {
    std::fs::read(format!("/proc/{pid}/environ"))
        .map(|b| b.split(|&c| c == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect())
        .unwrap_or_default()
}

/// The argv of `pid` (empty when unreadable).
pub fn cmdline(pid: i32) -> Vec<String> {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|b| b.split(|&c| c == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect())
        .unwrap_or_default()
}

/// Every process id currently in /proc.
pub fn all_pids() -> Vec<i32> {
    std::fs::read_dir("/proc").map(|d| d.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect()).unwrap_or_default()
}

/// `pid` and its descendants, children first.
pub fn tree(pid: i32) -> Vec<i32> {
    let mut children: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
    for p in all_pids() {
        if let Some(ppid) = stat_fields(p).and_then(|f| f.get(STAT_PPID)?.parse().ok()) {
            children.entry(ppid).or_default().push(p);
        }
    }
    fn walk(p: i32, children: &BTreeMap<i32, Vec<i32>>, out: &mut Vec<i32>) {
        for &c in children.get(&p).into_iter().flatten() {
            walk(c, children, out);
        }
        out.push(p);
    }
    let mut out = Vec::new();
    walk(pid, &children, &mut out);
    out
}

/// Sends `signal` to `pid`.
pub fn signal(pid: i32, signal: Signal) -> bool {
    Pid::from_raw(pid).is_some_and(|p| rustix::process::kill_process(p, signal).is_ok())
}

/// Terminates `pid` (SIGTERM, then SIGKILL after a grace period).
pub fn terminate(pid: i32) {
    if !signal(pid, Signal::TERM) {
        return;
    }
    if !wait_until(TERM_GRACE, || !alive(pid)) {
        signal(pid, Signal::KILL);
    }
}

/// Terminates a child this process owns and reaps it.
pub fn stop_child(child: &mut Child) {
    if matches!(child.try_wait(), Ok(None)) {
        if let Ok(pid) = i32::try_from(child.id()) {
            signal(pid, Signal::TERM);
        }
        let start = Instant::now();
        while matches!(child.try_wait(), Ok(None)) && start.elapsed() < TERM_GRACE {
            std::thread::sleep(POLL);
        }
        let _ = child.kill();
    }
    let _ = child.wait();
}

/// The last `n` lines of a log file.
pub fn tail(path: &Path, n: usize) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}
