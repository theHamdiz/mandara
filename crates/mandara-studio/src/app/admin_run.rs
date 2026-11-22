//! Runs Mandara as root for Studio's administrator actions (login screen,
//! keyring unlock), always through a root-owned binary
//! ([`mandara_config::admin`]): never your own copy, which your account can
//! change. With none in place yet, or only one from an older version, the
//! same approval installs the admin helper first.

use std::ffi::OsString;
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mandara_config::admin;
use mandara_i18n::t;

/// Longest an action may run before it is stopped. It covers a person typing
/// the polkit password, so it is generous; it only exists so a hung child can
/// never keep the page busy forever.
const RUN_DEADLINE: Duration = Duration::from_secs(10 * 60);
/// How often a running child is checked.
const POLL: Duration = Duration::from_millis(100);

/// Runs argv; Ok(stdout) on success, Err(stderr or stdout) otherwise. Stops
/// the child and returns an error once [`RUN_DEADLINE`] passes.
pub(super) fn run(prog: OsString, args: Vec<OsString>) -> Result<String, String> {
    run_within(prog, args, RUN_DEADLINE)
}

fn run_within(prog: OsString, args: Vec<OsString>, deadline: Duration) -> Result<String, String> {
    let mut child = Command::new(&prog)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", prog.to_string_lossy()))?;
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= deadline => {
                // pkexec turns setuid-root after authorizing, so the kill can
                // be refused: the child is reaped by a detached waiter either
                // way, and the pipe readers end with it.
                let _ = child.kill();
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Err(t("The administrator action took too long and was stopped").to_string());
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(e) => return Err(e.to_string()),
        }
    };
    let out = out.join().unwrap_or_default().trim().to_string();
    if status.success() {
        return Ok(out);
    }
    let err = err.join().unwrap_or_default().trim().to_string();
    Err([out, err].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n"))
}

/// Reads a child's pipe to the end on its own thread.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut p) = pipe {
            let _ = p.read_to_end(&mut buf);
        }
        String::from_utf8_lossy(&buf).into_owned()
    })
}

/// The message while waiting for the password.
pub(super) fn waiting_message() -> String {
    t("Waiting for authorization…").to_string()
}

/// `mandara <args>` as root (blocking: call off the UI thread).
pub(super) fn as_root(args: Vec<OsString>) -> Result<String, String> {
    // An older root-owned copy may not read what this build wrote (a login
    // screen draft with a newer element): refresh the helper first, in the
    // same approval.
    let (prog, args) = match admin::current_admin_exe() {
        Some(exe) => admin::pkexec(&exe, args),
        None => admin::setup_and_run(args).map_err(|e| e.to_string())?,
    };
    run(prog, args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> (OsString, Vec<OsString>) {
        ("sh".into(), vec!["-c".into(), script.into()])
    }

    #[test]
    fn success_returns_trimmed_stdout() {
        let (p, a) = sh("echo hi");
        assert_eq!(run_within(p, a, Duration::from_secs(30)).unwrap(), "hi");
    }

    #[test]
    fn failure_returns_both_streams() {
        let (p, a) = sh("echo out; echo err >&2; exit 3");
        assert_eq!(run_within(p, a, Duration::from_secs(30)).unwrap_err(), "out\nerr");
    }

    #[test]
    fn a_hung_child_is_stopped_at_the_deadline() {
        let (p, a) = sh("sleep 30");
        let started = Instant::now();
        let e = run_within(p, a, Duration::from_millis(200)).unwrap_err();
        assert_eq!(e, t("The administrator action took too long and was stopped"));
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
