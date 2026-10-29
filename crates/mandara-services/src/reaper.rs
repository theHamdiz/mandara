//! Reaps fire-and-forget children (apps, links, hot-corner and launcher
//! commands) so none is left as a zombie until the shell exits.
//!
//! A thread per child would sit blocked for as long as each app runs (a
//! browser: hours). Instead one `mandara-reaper` thread polls every adopted
//! child with `try_wait` and exits once the last one is gone; the next
//! [`adopt`] starts it again.

use std::process::Child;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// How often finished children are collected.
pub const REAP_EVERY: Duration = Duration::from_secs(2);

struct Reaper {
    children: Vec<Child>,
    /// The reaper thread is alive.
    running: bool,
}

static STATE: Mutex<Reaper> = Mutex::new(Reaper { children: Vec::new(), running: false });
/// Wakes the reaper early (a new child), so its first check is prompt.
static WAKE: Condvar = Condvar::new();

/// Takes ownership of `child` and waits for it in the background.
pub fn adopt(child: Child) {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.children.push(child);
    if state.running {
        WAKE.notify_one();
        return;
    }
    state.running = true;
    let started = std::thread::Builder::new().name("mandara-reaper".into()).spawn(run);
    if let Err(err) = started {
        // Without a thread the children are still waited for at the next adopt.
        state.running = false;
        tracing::warn!("reaper thread: {err}");
    }
}

/// Spawns `command` and adopts the child.
pub fn spawn(command: &mut std::process::Command) -> std::io::Result<u32> {
    let child = command.spawn()?;
    let pid = child.id();
    adopt(child);
    Ok(pid)
}

/// Children still running (for tests and diagnostics).
pub fn pending() -> usize {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).children.len()
}

/// Drops every child that has exited (or can no longer be waited for).
fn collect(children: &mut Vec<Child>) {
    children.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
}

fn run() {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        collect(&mut state.children);
        if state.children.is_empty() {
            state.running = false;
            return;
        }
        state = WAKE.wait_timeout(state, REAP_EVERY).unwrap_or_else(|e| e.into_inner()).0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::Instant;

    /// Upper bound for the reaper to notice exited children in tests.
    const SETTLE: Duration = Duration::from_secs(10);
    /// How often the tests look again.
    const POLL: Duration = Duration::from_millis(50);

    fn zombie(pid: u32) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| s.rsplit(')').next().is_some_and(|rest| rest.trim_start().starts_with('Z')))
    }

    #[test]
    fn exited_children_are_reaped_and_the_thread_stops() {
        let pids: Vec<u32> = (0..3).map(|_| spawn(&mut Command::new("true")).expect("spawn true")).collect();
        let deadline = Instant::now() + SETTLE;
        while (pending() > 0 || pids.iter().any(|&p| zombie(p))) && Instant::now() < deadline {
            std::thread::sleep(POLL);
        }
        assert!(pids.iter().all(|&p| !zombie(p)), "a child was left as a zombie");
        assert_eq!(pending(), 0);
        while STATE.lock().unwrap().running && Instant::now() < deadline {
            std::thread::sleep(POLL);
        }
        assert!(!STATE.lock().unwrap().running, "the reaper thread should exit when idle");
    }
}
