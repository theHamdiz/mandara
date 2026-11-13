//! Short-lived template commands with bounded output and a process-group deadline.

use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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

struct Group {
    child: Child,
    exited: bool,
}
impl Drop for Group {
    fn drop(&mut self) {
        if !self.exited {
            super::super::hooks::kill_group(self.child.id(), libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

fn read_available(pipe: &mut impl std::io::Read, bytes: &mut Vec<u8>) -> Result<()> {
    let mut buffer = [0; 4096];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                ensure!(
                    n <= MAX_OUTPUT.saturating_sub(bytes.len()),
                    "template command output exceeds {MAX_OUTPUT} bytes"
                );
                bytes.extend_from_slice(&buffer[..n]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
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
pub(super) fn native(
    mut command: Command,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<(String, String)> {
    ensure!(
        !cancel.load(Ordering::Acquire),
        "template command was cancelled"
    );
    let mut group = Group {
        child: command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .context("starting app theme command")?,
        exited: false,
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
    let mut output = Vec::new();
    let mut diagnostic = Vec::new();
    let deadline = Instant::now() + timeout;
    loop {
        read_available(&mut stdout, &mut output)?;
        read_available(&mut stderr, &mut diagnostic)?;
        if let Some(status) = group.child.try_wait()? {
            read_available(&mut stdout, &mut output)?;
            read_available(&mut stderr, &mut diagnostic)?;
            group.exited = true;
            ensure!(
                status.success(),
                "template command exited {status}: {}",
                String::from_utf8_lossy(&diagnostic).trim()
            );
            return Ok((
                String::from_utf8(output).context("template command returned non-UTF-8 paths")?,
                String::from_utf8_lossy(&diagnostic).into_owned(),
            ));
        }
        if cancel.load(Ordering::Acquire) {
            bail!("template command was cancelled");
        }
        if Instant::now() >= deadline {
            bail!("template command timed out");
        }
        std::thread::sleep(WAIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(shell: &str) -> RenderedCommand {
        RenderedCommand {
            shell: shell.into(),
            environment: Vec::new(),
        }
    }

    #[test]
    fn a_successful_reload_can_leave_its_application_running() {
        struct Cleanup(i32);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                crate::hooks::kill_group(self.0 as u32, libc::SIGKILL);
            }
        }
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
