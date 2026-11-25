//! An external recorder (GPU Screen Recorder, wf-recorder) as a child
//! process: started from an argument vector (no shell), stopped with
//! SIGINT so it finalizes the file, paused with SIGUSR2 where supported.
//! One thread waits for the process (no polling) and keeps the last lines
//! it printed, so a crash or a refused option comes back with its own
//! words.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::BufRead as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mandara_i18n::tf;
use rustix::process::{Pid, Signal, kill_process};

/// How long a stopped recorder may take to finish its file before it is
/// killed.
const STOP_TIMEOUT: Duration = Duration::from_secs(20);
/// Lines of the recorder's error output kept for a failure message.
const TAIL_LINES: usize = 12;

/// How a recorder process ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exit {
    /// Exit code, or `None` when a signal ended it.
    pub code: Option<i32>,
    /// The last lines it printed on stderr.
    pub stderr: String,
}

impl Exit {
    /// The most telling line of its output (errors first).
    pub fn reason(&self) -> Option<String> {
        let lines: Vec<&str> = self.stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        lines.iter().rev().find(|l| l.to_ascii_lowercase().contains("error")).or(lines.last()).map(|l| l.to_string())
    }
}

/// A running external recorder.
pub struct External {
    program: String,
    pid: Pid,
    exited: Arc<AtomicBool>,
    done: std::sync::mpsc::Receiver<Exit>,
    pub path: PathBuf,
}

/// Starts `program` with `args`, writing `path`. `ended` closes when the
/// process exits (stopped, crashed or refused its options).
pub fn spawn(program: &Path, args: Vec<OsString>, path: PathBuf, ended: async_channel::Sender<()>) -> Result<External, String> {
    let name = program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| tf("Could not create {path}: {error}", &[("path", &dir.display()), ("error", &e)]))?;
    }
    tracing::info!(target: "mandara::recorder", program = %program.display(), ?args, "starting");
    let mut child = Command::new(program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| tf("Could not start {program}: {error}", &[("program", &name), ("error", &e)]))?;
    let Some(pid) = Pid::from_raw(i32::try_from(child.id()).unwrap_or(0)) else {
        // Never leave a recorder running that nothing controls.
        let _ = child.kill();
        let _ = child.wait();
        return Err(tf("Could not start {program}: {error}", &[("program", &name), ("error", &"no process id")]));
    };
    let tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
    let reader = child.stderr.take().map(|err| {
        let tail = tail.clone();
        std::thread::Builder::new().name("mandara-rec-stderr".into()).spawn(move || {
            for line in std::io::BufReader::new(err).lines().map_while(Result::ok) {
                tracing::debug!(target: "mandara::recorder", "{line}");
                if let Ok(mut t) = tail.lock() {
                    if t.len() == TAIL_LINES {
                        t.pop_front();
                    }
                    t.push_back(line);
                }
            }
        })
    });
    let exited = Arc::new(AtomicBool::new(false));
    let (tx, done) = std::sync::mpsc::channel();
    let exited2 = exited.clone();
    let waiter = std::thread::Builder::new()
        .name("mandara-rec-wait".into())
        .spawn(move || {
            let status = child.wait();
            exited2.store(true, Ordering::SeqCst);
            // The pipe closes with the process; take every last line.
            if let Some(Ok(handle)) = reader {
                let _ = handle.join();
            }
            let stderr = tail.lock().map(|t| t.iter().cloned().collect::<Vec<_>>().join("\n")).unwrap_or_default();
            let code = status.ok().and_then(|s| s.code());
            tracing::info!(target: "mandara::recorder", ?code, "recorder exited");
            let _ = tx.send(Exit { code, stderr });
            drop(ended);
        });
    if let Err(e) = waiter {
        // The thread (and the child handle in it) is gone: stop and reap
        // the recorder by its pid so it never records unattended.
        reap(pid);
        return Err(tf("Could not start {program}: {error}", &[("program", &name), ("error", &e)]));
    }
    Ok(External { program: name, pid, exited, done, path })
}

/// Kills and reaps a recorder whose `Child` handle was lost.
fn reap(pid: Pid) {
    let _ = kill_process(pid, Signal::KILL);
    let _ = rustix::process::waitpid(Some(pid), rustix::process::WaitOptions::empty());
}

impl External {
    fn signal(&self, signal: Signal) -> Result<(), String> {
        if self.exited.load(Ordering::SeqCst) {
            return Err(tf("{program} is no longer running", &[("program", &self.program)]));
        }
        kill_process(self.pid, signal).map_err(|e| e.to_string())
    }

    /// GPU Screen Recorder pauses and resumes on SIGUSR2 (a toggle).
    pub fn toggle_pause(&self) -> Result<(), String> {
        self.signal(Signal::USR2)
    }

    pub fn is_running(&self) -> bool {
        !self.exited.load(Ordering::SeqCst)
    }

    /// Asks the recorder to finish (SIGINT) and waits for it; kills it
    /// after [`STOP_TIMEOUT`]. Blocking: call off the UI thread.
    pub fn stop(self) -> Exit {
        let _ = self.signal(Signal::INT);
        match self.done.recv_timeout(STOP_TIMEOUT) {
            Ok(exit) => exit,
            Err(_) => {
                tracing::warn!(target: "mandara::recorder", "{} did not stop in time; killing it", self.program);
                let _ = self.signal(Signal::KILL);
                self.done.recv().unwrap_or(Exit { code: None, stderr: String::new() })
            }
        }
    }

    /// How it ended, once it has (after [`spawn`]'s `ended` closed).
    pub fn exit(&self) -> Option<Exit> {
        self.done.try_recv().ok()
    }

    /// Waits up to `timeout` for the process to end on its own.
    pub fn wait_exit(&self, timeout: Duration) -> Option<Exit> {
        self.done.recv_timeout(timeout).ok()
    }

    pub fn program(&self) -> &str {
        &self.program
    }
}

impl Drop for External {
    /// A recorder never outlives the shell: an abandoned one is asked to
    /// finish its file.
    fn drop(&mut self) {
        if self.is_running() {
            let _ = self.signal(Signal::INT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lets the test script install its signal handler.
    const SETTLE: Duration = Duration::from_millis(200);

    #[test]
    fn exit_reason_prefers_errors() {
        let e = Exit { code: Some(1), stderr: "gsr info: starting\ngsr error: failed to find monitor HEADLESS-1\nusage: …".into() };
        assert_eq!(e.reason().as_deref(), Some("gsr error: failed to find monitor HEADLESS-1"));
        let e = Exit { code: Some(1), stderr: "\nsomething went wrong\n".into() };
        assert_eq!(e.reason().as_deref(), Some("something went wrong"));
        assert_eq!(Exit { code: Some(0), stderr: String::new() }.reason(), None);
    }

    /// A real child: stopped with SIGINT, its stderr tail kept, `ended`
    /// closing when it exits.
    #[test]
    fn stops_a_real_process_and_reports_its_output() {
        let Some(sh) = super::super::backend::which("sh") else { return };
        let (tx, ended) = async_channel::bounded::<()>(1);
        let script = "echo first >&2; trap 'echo stopped >&2; exit 0' INT; while :; do sleep 0.05; done";
        let p = spawn(&sh, vec!["-c".into(), script.into()], std::env::temp_dir().join("mandara-ext-test"), tx).unwrap();
        assert!(p.is_running());
        std::thread::sleep(SETTLE);
        let exit = p.stop();
        assert_eq!(exit.code, Some(0));
        assert!(exit.stderr.contains("first") && exit.stderr.contains("stopped"), "{exit:?}");
        assert!(ended.recv_blocking().is_err(), "ended closes with the process");
    }

    #[test]
    fn a_failing_process_ends_on_its_own() {
        let Some(sh) = super::super::backend::which("sh") else { return };
        let (tx, ended) = async_channel::bounded::<()>(1);
        let p = spawn(&sh, vec!["-c".into(), "echo 'error: bad option -k' >&2; exit 2".into()], std::env::temp_dir().join("mandara-ext-test2"), tx).unwrap();
        assert!(ended.recv_blocking().is_err());
        let exit = p.exit().expect("reported");
        assert_eq!(exit.code, Some(2));
        assert_eq!(exit.reason().as_deref(), Some("error: bad option -k"));
        assert!(p.toggle_pause().is_err(), "no signals to a finished process");
    }

    /// A recorder whose handle was lost is killed and reaped by pid (no
    /// process, not even a zombie, is left).
    #[test]
    fn reap_stops_an_orphaned_recorder() {
        let Some(sleep) = super::super::backend::which("sleep") else { return };
        let child = Command::new(sleep).arg("30").spawn().unwrap();
        let pid = Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap();
        drop(child);
        reap(pid);
        assert!(!std::path::Path::new(&format!("/proc/{}", pid.as_raw_nonzero())).exists(), "killed and reaped");
    }

    #[test]
    fn a_missing_program_is_an_error() {
        let (tx, _ended) = async_channel::bounded::<()>(1);
        assert!(spawn(Path::new("/nonexistent/mandara-recorder"), Vec::new(), std::env::temp_dir().join("x"), tx).is_err());
    }
}
