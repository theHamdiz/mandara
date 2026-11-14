//! Short-lived native commands with bounded output and a process-group deadline.

use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use std::io::Write as _;

use anyhow::{Context as _, Result, bail, ensure};
use mandara_theme::templates::custom::Command as RenderedCommand;
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};

/// Dynamic paths and diagnostic messages are small; scripts must not stream data here.
const MAX_OUTPUT: usize = 64 * 1024;
/// A command gets enough time for app reloads without blocking the next theme forever.
const TIMEOUT_SECONDS: u64 = 30;
pub const TIMEOUT: Duration = Duration::from_secs(TIMEOUT_SECONDS);
/// Only running commands are polled; no thread remains after the job completes.
const WAIT_MILLIS: u64 = 20;
const WAIT: Duration = Duration::from_millis(WAIT_MILLIS);
/// Check cancellation between batches even when a tool continuously fills its pipe.
const READ_BATCH_CHUNKS: usize = 64;

struct Group {
    child: Child,
    completed: bool,
}
impl Drop for Group {
    fn drop(&mut self) {
        if !self.completed {
            super::super::hooks::kill_group(self.child.id(), libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

fn read_available(pipe: &mut impl std::io::Read, consume: &mut impl FnMut(&[u8]) -> Result<()>) -> Result<bool> {
    let mut buffer = [0; 4096];
    for _ in 0..READ_BATCH_CHUNKS {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                consume(&buffer[..n])?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(false)
}

pub fn run(
    command: &RenderedCommand,
    directory: &Path,
    environment: &[(String, String)],
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<String> {
    let mut process = Command::new("sh");
    process
        .args(["-c", &command.shell])
        .current_dir(directory)
        .envs(environment.iter().map(|(key, value)| (key, value)))
        .envs(command.environment.iter().map(|(key, value)| (key, value)));
    native(process, cancel, timeout).map(|output| output.0)
}

/// Run a native checker with the same output, cancellation and child-process bounds.
pub(crate) fn native(
    command: Command,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<(String, String)> {
    let mut output = Vec::new();
    let diagnostic = stream_native(command, cancel, timeout, &mut |bytes| {
        ensure!(bytes.len() <= MAX_OUTPUT.saturating_sub(output.len()), "native command output exceeds {MAX_OUTPUT} bytes");
        output.extend_from_slice(bytes);
        Ok(())
    })?;
    Ok((String::from_utf8(output).context("command returned non-UTF-8 output")?, diagnostic))
}

/// Stream a native job through bounded chunks, sharing cancellation and process-group cleanup.
/// The consumer bounds its retained data; stderr always has the short diagnostic limit.
pub(crate) fn stream_native(
    command: Command,
    cancel: &AtomicBool,
    timeout: Duration,
    consume: &mut impl FnMut(&[u8]) -> Result<()>,
) -> Result<String> {
    stream(command, cancel, timeout, false, consume)
}

/// The root helper cancels its own child after a request on its private input pipe.
pub(crate) fn stream_native_controlled(
    command: Command,
    cancel: &AtomicBool,
    timeout: Duration,
    consume: &mut impl FnMut(&[u8]) -> Result<()>,
) -> Result<String> {
    stream(command, cancel, timeout, true, consume)
}

fn stream(
    mut command: Command,
    cancel: &AtomicBool,
    timeout: Duration,
    controlled: bool,
    consume: &mut impl FnMut(&[u8]) -> Result<()>,
) -> Result<String> {
    ensure!(
        !cancel.load(Ordering::Acquire),
        "native command was cancelled"
    );
    let mut group = Group {
        child: command
            .stdin(if controlled { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .context("starting native command")?,
        completed: false,
    };
    let mut stdout = group
        .child
        .stdout
        .take()
        .context("template stdout is unavailable")?;
    let mut stderr = group
        .child
        .stderr
        .take()
        .context("template stderr is unavailable")?;
    fcntl_setfl(&stdout, fcntl_getfl(&stdout)? | OFlags::NONBLOCK)?;
    fcntl_setfl(&stderr, fcntl_getfl(&stderr)? | OFlags::NONBLOCK)?;
    let mut input = group.child.stdin.take();
    let mut cancel_sent = false;
    let mut diagnostic = Vec::new();
    let mut diagnostic_chunk = |bytes: &[u8]| {
        ensure!(bytes.len() <= MAX_OUTPUT.saturating_sub(diagnostic.len()), "command diagnostics exceed {MAX_OUTPUT} bytes");
        diagnostic.extend_from_slice(bytes);
        Ok(())
    };
    let mut deadline = Instant::now() + timeout;
    let mut exited = None;
    loop {
        let output_drained = read_available(&mut stdout, consume)?;
        let diagnostic_drained = read_available(&mut stderr, &mut diagnostic_chunk)?;
        if exited.is_none() { exited = group.child.try_wait()?; }
        if let Some(status) = exited.filter(|_| output_drained && diagnostic_drained) {
            ensure!(controlled || !cancel_sent, "native command was cancelled");
            ensure!(
                status.success(),
                "native command exited {status}: {}",
                String::from_utf8_lossy(&diagnostic).trim()
            );
            let diagnostic = String::from_utf8_lossy(&diagnostic).into_owned();
            group.completed = true;
            return Ok(diagnostic);
        }
        if cancel.load(Ordering::Acquire) && !cancel_sent {
                if controlled {
                    let input = input.as_mut().context("administrator control pipe is unavailable")?;
                    input.write_all(b"cancel\n")?;
                    input.flush()?;
                } else {
                    // Restic releases its repository lock and btrfs scrub
                    // records progress and cancels its ioctl on SIGINT.
                    crate::hooks::kill_group(group.child.id(), libc::SIGINT);
                }
                cancel_sent = true;
                deadline = Instant::now() + TIMEOUT;
        }
        if Instant::now() >= deadline {
            bail!("native command timed out");
        }
        if output_drained && diagnostic_drained { std::thread::sleep(WAIT); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuous_output_cannot_starve_cancellation() {
        let cancel = AtomicBool::new(false);
        let mut command = Command::new("sh");
        command.args(["-c", "while :; do printf 'continuous output'; done"]);
        let started = Instant::now();
        let result = stream_native(command, &cancel, TIMEOUT, &mut |_| { cancel.store(true, Ordering::Release); Ok(()) });
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert!(started.elapsed() < TIMEOUT);
    }

    #[test]
    fn cancellation_allows_a_native_tools_cleanup_before_the_hard_deadline() {
        let root = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let mut command = Command::new("sh");
        command.current_dir(root.path()).args(["-c", "trap 'printf cleaned > receipt; exit 0' INT; printf ready; while :; do :; done"]);
        let result = stream_native(command, &cancel, TIMEOUT, &mut |_| { cancel.store(true, Ordering::Release); Ok(()) });
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert_eq!(std::fs::read_to_string(root.path().join("receipt")).unwrap(), "cleaned");
    }

    #[test]
    fn administrator_cancellation_is_sent_over_the_control_pipe() {
        let cancel = AtomicBool::new(false);
        let mut command = Command::new("sh");
        command.args(["-c", "printf 'ready\\n'; read -r request; [ \"$request\" = cancel ] && printf 'cancelled safely'"]);
        let mut output = Vec::new();
        stream_native_controlled(command, &cancel, TIMEOUT, &mut |bytes| { output.extend_from_slice(bytes); cancel.store(true, Ordering::Release); Ok(()) }).unwrap();
        assert_eq!(output, b"ready\ncancelled safely");
    }

    #[test]
    fn streaming_jobs_do_not_retain_the_entire_progress_log() {
        let mut command = Command::new("sh");
        command.args(["-c", "i=0; while [ $i -lt 20000 ]; do printf 'progress update\\n'; i=$((i+1)); done"]);
        let mut bytes = 0;
        stream_native(command, &AtomicBool::new(false), TIMEOUT, &mut |chunk| {
            bytes += chunk.len();
            Ok(())
        }).unwrap();
        assert_eq!(bytes, 20000 * "progress update\n".len());
        assert!(bytes > MAX_OUTPUT);
    }

    struct Cleanup(i32);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            crate::hooks::kill_group(self.0 as u32, libc::SIGKILL);
        }
    }

    fn command(shell: &str) -> RenderedCommand {
        RenderedCommand {
            shell: shell.into(),
            environment: Vec::new(),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn failed_reload_commands_do_not_leave_their_background_children_running() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            run(
                &command("sleep 30 >/dev/null 2>&1 & printf '%s %s' \"$$\" \"$!\" > pids; exit 1"),
                root.path(),
                &[],
                &AtomicBool::new(false),
                TIMEOUT
            )
            .is_err()
        );
        let pids = std::fs::read_to_string(root.path().join("pids"))
            .unwrap()
            .split_whitespace()
            .map(|pid| pid.parse::<i32>().unwrap())
            .collect::<Vec<_>>();
        let _cleanup = Cleanup(pids[0]);
        for _ in 0..100 {
            let stopped = match std::fs::read_to_string(format!("/proc/{}/stat", pids[1])) {
                Ok(stat) => stat
                    .rsplit_once(") ")
                    .is_some_and(|(_, fields)| fields.starts_with(['Z', 'X'])),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                Err(error) => panic!("reading the test child's state: {error}"),
            };
            if stopped {
                return;
            }
            std::thread::sleep(WAIT);
        }
        panic!("the failed command left its child running");
    }

    #[test]
    fn a_successful_reload_can_leave_its_application_running() {
        let root = tempfile::tempdir().unwrap();
        let output = run(
            &command("sleep 30 >/dev/null 2>&1 & printf '%s %s' \"$$\" \"$!\""),
            root.path(),
            &[],
            &AtomicBool::new(false),
            TIMEOUT,
        )
        .unwrap();
        let pids = output
            .split_whitespace()
            .map(|pid| pid.parse::<i32>().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(pids.len(), 2);
        let _cleanup = Cleanup(pids[0]);
        assert_eq!(
            unsafe { libc::kill(pids[1], 0) },
            0,
            "the application outlives its reload command"
        );
    }

    #[test]
    fn output_errors_and_deadlines_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        assert_eq!(
            run(
                &command("printf '%s' \"$TEST_VALUE\""),
                dir.path(),
                &[("TEST_VALUE".into(), "a 'quote' $(false)".into())],
                &cancel,
                TIMEOUT
            )
            .unwrap(),
            "a 'quote' $(false)"
        );
        assert!(
            run(
                &command("printf diagnostic >&2; exit 3"),
                dir.path(),
                &[],
                &cancel,
                TIMEOUT
            )
            .unwrap_err()
            .to_string()
            .contains("diagnostic")
        );
        assert!(
            run(&command("yes x"), dir.path(), &[], &cancel, TIMEOUT)
                .unwrap_err()
                .to_string()
                .contains("exceeds")
        );
        let started = Instant::now();
        assert!(
            run(&command("sleep 30 & wait"), dir.path(), &[], &cancel, WAIT)
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        cancel.store(true, Ordering::Release);
        assert!(
            run(
                &command("touch should-not-exist"),
                dir.path(),
                &[],
                &cancel,
                TIMEOUT
            )
            .is_err()
        );
        assert!(!dir.path().join("should-not-exist").exists());
    }
}
