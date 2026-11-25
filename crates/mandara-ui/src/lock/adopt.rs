//! A locker that was already running when the shell started.
//!
//! The locker has its own process group and outlives a shell that restarts
//! or crashes while the session is locked. The new shell knows nothing of
//! it (`CHILDREN` is empty), so without this it would treat the session as
//! unlocked: video wallpapers and widgets keep running behind the lock, the
//! idle stages skip their locked steps, and a lock request would start a
//! second locker. The marker file ([`super::locked_marker`]) holds the
//! locker's pid; it counts only when that pid is a live `mandara lock`.

use std::sync::atomic::Ordering;

use super::{ADOPTED_LOCKER, CHILDREN, announce, lock_now, locked_marker};

/// The pid in the marker file, when `alive` says it is a real locker. A
/// marker a crashed locker (or Plasma) left behind, or whose pid was reused
/// by another program, names nobody.
fn locker_in_marker(marker: &str, alive: impl Fn(u32) -> bool) -> Option<u32> {
    let pid: u32 = marker.trim().parse().ok()?;
    alive(pid).then_some(pid)
}

/// Whether `raw`, a process's `/proc/<pid>/cmdline` (NUL-separated), is a
/// real locker: `<exe> lock` without `--demo`.
fn is_locker_cmdline(raw: &[u8]) -> bool {
    let mut args = raw.split(|b| *b == 0);
    args.next(); // the executable
    args.next() == Some(b"lock") && !args.any(|arg| arg == b"--demo")
}

fn is_locker_process(pid: u32) -> bool {
    std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|raw| is_locker_cmdline(&raw))
}

/// The running real locker the marker names, if any.
pub fn marker_locker() -> Option<u32> {
    locker_in_marker(&std::fs::read_to_string(locked_marker()).ok()?, is_locker_process)
}

/// Blocks until process `pid` has exited. A pidfd turns readable then, so
/// nothing polls; a process that is already gone returns at once.
fn wait_for_exit(pid: u32) -> std::io::Result<()> {
    use rustix::event::{PollFd, PollFlags, poll};
    let Some(pid) = i32::try_from(pid).ok().and_then(rustix::process::Pid::from_raw) else { return Ok(()) };
    let fd = match rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::SRCH) => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    loop {
        match poll(&mut [PollFd::new(&fd, PollFlags::IN)], None) {
            Ok(_) => return Ok(()),
            Err(rustix::io::Errno::INTR) => {}
            Err(err) => return Err(err.into()),
        }
    }
}

/// Shell startup: takes over a locker that is already running. Until it
/// ends, [`super::is_locked`] is true, and when it ends listeners hear the
/// unlock. A locker that died with the marker left behind never unlocked,
/// so the session is still locked: a new locker is started.
pub fn adopt_running_locker() {
    if CHILDREN.lock().is_ok_and(|c| c.real) {
        return;
    }
    let Some(pid) = marker_locker() else { return };
    tracing::info!("lock screen: found the running locker (pid {pid}) from before this shell started");
    ADOPTED_LOCKER.store(true, Ordering::Relaxed);
    let spawned = std::thread::Builder::new().name("mandara-lock-adopted".into()).spawn(move || {
        if let Err(err) = wait_for_exit(pid) {
            tracing::warn!("lock screen: cannot watch the running locker (pid {pid}): {err}");
        }
        exited(pid);
    });
    if let Err(err) = spawned {
        tracing::warn!("lock screen: cannot watch the running locker: {err}");
        ADOPTED_LOCKER.store(false, Ordering::Relaxed);
    }
}

fn exited(pid: u32) {
    ADOPTED_LOCKER.store(false, Ordering::Relaxed);
    // A locker that unlocked removed the marker before it exited.
    let marker_names_it = std::fs::read_to_string(locked_marker()).is_ok_and(|m| m.trim().parse() == Ok(pid));
    if marker_names_it {
        tracing::error!("lock screen: the running locker (pid {pid}) ended while the session was locked; starting a new one");
        match lock_now() {
            Ok(_) => return,
            Err(err) => tracing::error!("lock screen: {err}"),
        }
    }
    let _ = std::fs::remove_file(locked_marker());
    announce(false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn only_a_real_lock_command_line_counts() {
        assert!(is_locker_cmdline(b"/usr/bin/mandara\0lock\0"));
        assert!(!is_locker_cmdline(b"/usr/bin/mandara\0lock\0--demo\0--timeout\x0060\0"), "the preview never locks");
        assert!(!is_locker_cmdline(b"/usr/bin/mandara\0msg\0lock\0"));
        assert!(!is_locker_cmdline(b"/usr/bin/mandara\0"));
        assert!(!is_locker_cmdline(b"vim\0lock.txt\0"));
        assert!(!is_locker_cmdline(b""), "a zombie has an empty command line");
    }

    #[test]
    fn the_marker_names_a_locker_only_while_it_runs() {
        let alive = |pid| pid == 4242;
        assert_eq!(locker_in_marker("4242", alive), Some(4242));
        assert_eq!(locker_in_marker("4242\n", alive), Some(4242));
        assert_eq!(locker_in_marker("4243", alive), None, "a pid nobody uses (or another program)");
        assert_eq!(locker_in_marker("", alive), None, "Plasma's marker is empty");
        assert_eq!(locker_in_marker("not a pid", alive), None);
    }

    #[test]
    fn this_test_process_is_not_a_locker() {
        assert!(!is_locker_process(std::process::id()));
    }

    #[test]
    fn waiting_for_a_process_ends_when_it_exits() {
        let mut child = Command::new("sleep").arg("0.3").stdout(Stdio::null()).spawn().expect("sleep");
        let pid = child.id();
        let started = Instant::now();
        wait_for_exit(pid).expect("pidfd");
        assert!(started.elapsed() >= Duration::from_millis(100), "returned before the process exited");
        child.wait().expect("reap");
        // Gone (and reaped) already: returns at once.
        let started = Instant::now();
        wait_for_exit(pid).expect("a missing process is not an error");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
