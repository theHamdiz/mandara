//! Controls a guardian process group while its unreaped leader reserves the id.

use std::ffi::OsString;
use std::io::{self, BufRead as _, Read as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[cfg(test)]
pub(crate) use mandara_services::renderer_worker::CHILD_PATH;
pub(crate) use mandara_services::renderer_worker::{MAX_STDERR, Report};

/// Includes JSON escaping of the bounded stderr tail and protocol overhead.
const MAX_REPORT: u64 = (MAX_STDERR * 6 + 1024) as u64;

#[derive(Clone)]
pub(crate) struct Group(Arc<Mutex<ProcessState>>);
#[derive(Default)]
struct ProcessState {
    pid: Option<i32>,
    cancelled: bool,
    paused: bool,
    finished: bool,
}
impl Group {
    pub fn signal(&self, signal: i32) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match signal {
            libc::SIGTERM | libc::SIGKILL => state.cancelled = true,
            libc::SIGSTOP => state.paused = true,
            libc::SIGCONT => state.paused = false,
            _ => return,
        }
        if let Some(pid) = state.pid {
            // SAFETY: our unreaped child reserves this process group's id.
            unsafe {
                libc::killpg(pid, signal);
            }
        }
    }
    pub fn alive(&self) -> bool {
        self.0.lock().is_ok_and(|state| !state.finished)
    }
}

pub(crate) fn launch(
    program: PathBuf,
    argv: Vec<OsString>,
    reports: async_channel::Sender<Report>,
) -> Group {
    launch_inner(program, argv, reports)
}

fn launch_inner(
    program: PathBuf,
    argv: Vec<OsString>,
    reports: async_channel::Sender<Report>,
) -> Group {
    let group = Group(Arc::new(Mutex::new(ProcessState::default())));
    let shared = group.clone();
    let errors = reports.clone();
    let spawned = std::thread::Builder::new()
        .name("mandara-live-wallpaper".into())
        .spawn(move || {
            supervise(&program, &argv, &shared, &reports);
        });
    if let Err(error) = spawned {
        group.0.lock().unwrap_or_else(|e| e.into_inner()).finished = true;
        let _ = errors.try_send(Report::Spawned(Err(error.to_string())));
    }
    group
}

#[cfg(not(test))]
fn worker_command(program: &Path, argv: &[OsString]) -> io::Result<std::process::Command> {
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command.arg("renderer-worker");
    command.arg(program).arg("--").args(argv);
    Ok(command)
}

#[cfg(test)]
fn worker_command(program: &Path, argv: &[OsString]) -> io::Result<std::process::Command> {
    use std::os::unix::ffi::OsStrExt as _;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    let args: Vec<Vec<u8>> = std::iter::once(program.as_os_str())
        .chain(argv.iter().map(|arg| arg.as_os_str()))
        .map(|arg| arg.as_bytes().to_vec())
        .collect();
    command.args([
        "--exact",
        "renderer_process::tests::worker_entry",
        "--nocapture",
    ]);
    command.env("MANDARA_RENDERER_TEST_ARGS", serde_json::to_string(&args)?);
    Ok(command)
}

fn supervise(
    program: &Path,
    argv: &[OsString],
    group: &Group,
    reports: &async_channel::Sender<Report>,
) {
    let result = supervise_inner(program, argv, group, reports);
    group.0.lock().unwrap_or_else(|e| e.into_inner()).finished = true;
    if let Err(error) = result {
        let _ = reports.try_send(Report::Spawned(Err(error.to_string())));
    }
}

fn supervise_inner(
    program: &Path,
    argv: &[OsString],
    group: &Group,
    reports: &async_channel::Sender<Report>,
) -> io::Result<()> {
    use std::os::unix::process::CommandExt as _;
    if group.0.lock().unwrap_or_else(|e| e.into_inner()).cancelled {
        return Err(io::Error::other("renderer cancelled"));
    }
    let mut command = worker_command(program, argv)?;
    #[cfg(test)]
    let test_args = command
        .get_envs()
        .find(|(key, _)| *key == "MANDARA_RENDERER_TEST_ARGS")
        .and_then(|(_, value)| value.map(OsString::from));
    command
        .env_clear()
        .env("PATH", mandara_services::renderer_worker::CHILD_PATH)
        .current_dir("/")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .process_group(0);
    mandara_services::renderer_worker::inherit_environment(&mut command);
    #[cfg(test)]
    command.env("MANDARA_RENDERER_TEST_ARGS", test_args.unwrap());
    let parent = std::process::id();
    // SIGCONT wakes a paused guardian after controller death so it can read EOF.
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        command.pre_exec(move || {
            // Orphaned stopped groups receive SIGHUP before SIGCONT.
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            mandara_services::renderer_worker::parent_death_signal(libc::SIGCONT, parent)
        });
    }
    let mut worker = command.spawn()?;
    let pid = worker.id() as i32;
    {
        let mut state = group.0.lock().unwrap_or_else(|e| e.into_inner());
        state.pid = Some(pid);
        if state.cancelled || state.paused {
            // SAFETY: the newly started guardian is our unreaped child.
            unsafe {
                libc::killpg(
                    pid,
                    if state.cancelled {
                        libc::SIGKILL
                    } else {
                        libc::SIGSTOP
                    },
                );
            }
        }
    }
    // Keeping the writer open is the guardian's lifetime signal; it carries no data.
    let lifeline = worker.stdin.take();
    let mut output = io::BufReader::new(worker.stdout.take().expect("piped guardian stdout"));
    let mut line = Vec::with_capacity(MAX_STDERR);
    let mut started = false;
    let terminal = loop {
        line.clear();
        match output
            .by_ref()
            .take(MAX_REPORT + 1)
            .read_until(b'\n', &mut line)
        {
            Ok(0) => {
                break Err(io::Error::other(
                    "renderer guardian exited without a terminal report",
                ));
            }
            Ok(_) if line.len() as u64 > MAX_REPORT => {
                break Err(io::Error::other("oversized renderer report"));
            }
            Ok(_) => match serde_json::from_slice::<Report>(&line) {
                Ok(Report::Spawned(Ok(_))) => {
                    started = true;
                    let _ = reports.try_send(Report::Spawned(Ok(pid)));
                }
                Ok(Report::Spawned(Err(error))) => break Err(io::Error::other(error)),
                Ok(Report::Exited(message)) => break Ok(message),
                // Test harness lines are outside the JSON protocol.
                Err(_) => {}
            },
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => break Err(error),
        }
    };
    {
        let mut state = group.0.lock().unwrap_or_else(|e| e.into_inner());
        // Also covers guardian crashes: its unreaped id still reserves the group.
        // SAFETY: kill before clearing the id and reaping our child.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
        state.pid = None;
    }
    drop(lifeline);
    let waited = worker.wait();
    group.0.lock().unwrap_or_else(|e| e.into_inner()).finished = true;
    if started {
        let message = terminal.unwrap_or_else(|error| {
            format!(
                "{error} ({})",
                waited.map(|status| status.to_string()).unwrap_or_default()
            )
        });
        let _ = reports.try_send(Report::Exited(message));
        Ok(())
    } else {
        terminal.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn worker_entry() {
        use std::os::unix::ffi::OsStringExt as _;
        let Ok(args) = std::env::var("MANDARA_RENDERER_TEST_ARGS") else {
            return;
        };
        let args: Vec<Vec<u8>> = serde_json::from_str(&args).unwrap();
        let args: Vec<OsString> = args.into_iter().map(OsString::from_vec).collect();
        println!();
        mandara_services::renderer_worker::run(Path::new(&args[0]), &args[1..]).unwrap();
    }

    fn wait_finished(group: &Group) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while group.alive() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!group.alive());
    }

    fn report(rx: &async_channel::Receiver<Report>, group: &Group) -> Report {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(report) = rx.try_recv() {
                return report;
            }
            if Instant::now() >= deadline {
                group.signal(libc::SIGKILL);
                panic!("renderer did not report within five seconds");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pause_and_resume_reach_the_renderer_group() {
        let (tx, rx) = async_channel::bounded(2);
        let group = launch(
            "/bin/sh".into(),
            ["-c", "sleep 30"].map(OsString::from).into(),
            tx,
        );
        let Report::Spawned(Ok(pid)) = report(&rx, &group) else {
            panic!("renderer did not start")
        };
        for (signal, stopped) in [(libc::SIGSTOP, true), (libc::SIGCONT, false)] {
            group.signal(signal);
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
                let paused = stat
                    .rsplit_once(')')
                    .unwrap()
                    .1
                    .trim_start()
                    .starts_with('T');
                if paused == stopped {
                    break;
                }
                if Instant::now() >= deadline {
                    group.signal(libc::SIGKILL);
                    panic!("renderer pause state did not change");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        group.signal(libc::SIGKILL);
        assert!(matches!(report(&rx, &group), Report::Exited(_)));
        wait_finished(&group);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn guardian_death_reports_failure_and_stops_its_helpers() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("helpers");
        let (tx, rx) = async_channel::bounded(2);
        let group = launch(
            "/bin/sh".into(),
            vec![
                "-c".into(),
                "sleep 30 & echo $$ $! > \"$1\"; wait".into(),
                "renderer".into(),
                marker.clone().into(),
            ],
            tx,
        );
        let Report::Spawned(Ok(guardian)) = report(&rx, &group) else {
            panic!("renderer did not start")
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let helpers = std::fs::read_to_string(marker).unwrap();
        // SAFETY: this guardian is still our unreaped child.
        unsafe {
            libc::kill(guardian, libc::SIGKILL);
        }
        let Report::Exited(message) = report(&rx, &group) else {
            panic!("missing failure report")
        };
        assert!(message.contains("guardian exited"), "{message}");
        for pid in helpers.split_whitespace() {
            while std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                !stat
                    .rsplit_once(')')
                    .unwrap()
                    .1
                    .trim_start()
                    .starts_with('Z')
            }) {
                assert!(Instant::now() < deadline, "helper survived guardian death");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    #[test]
    fn controller_entry() {
        let Some(marker) = std::env::var_os("MANDARA_RENDERER_TEST_PARENT") else {
            return;
        };
        let (tx, rx) = async_channel::bounded(2);
        let group = launch(
            "/bin/sh".into(),
            vec![
                "-c".into(),
                "sleep 30 & echo $$ $! > \"$1\"; wait".into(),
                "renderer".into(),
                marker,
            ],
            tx,
        );
        assert!(matches!(report(&rx, &group), Report::Spawned(Ok(_))));
        if std::env::var("MANDARA_RENDERER_TEST_PAUSED").is_ok_and(|value| value == "1") {
            let marker =
                Path::new(&std::env::var_os("MANDARA_RENDERER_TEST_PARENT").unwrap()).to_path_buf();
            while !marker.exists() {
                std::thread::sleep(Duration::from_millis(10));
            }
            group.signal(libc::SIGSTOP);
            std::fs::write(marker.with_extension("paused"), b"ready").unwrap();
        }
        let _ = std::io::stdin().read(&mut [0]);
        group.signal(libc::SIGKILL);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn abrupt_controller_death_stops_the_renderer_and_its_helper() {
        controller_crash(false);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_paused_guardian_cleans_up_after_controller_death() {
        controller_crash(true);
    }

    #[cfg(target_os = "linux")]
    fn controller_crash(paused: bool) {
        use std::process::{Command, Stdio};
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("pids");
        let mut controller = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "renderer_process::tests::controller_entry",
                "--nocapture",
            ])
            .env("MANDARA_RENDERER_TEST_PARENT", &marker)
            .env(
                "MANDARA_RENDERER_TEST_PAUSED",
                if paused { "1" } else { "" },
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let pids = loop {
            if let Ok(text) = std::fs::read_to_string(&marker) {
                let pids: Vec<u32> = text
                    .split_whitespace()
                    .filter_map(|pid| pid.parse().ok())
                    .collect();
                if pids.len() == 2 && (!paused || marker.with_extension("paused").exists()) {
                    break pids;
                }
            }
            if Instant::now() >= deadline {
                let _ = controller.kill();
                let _ = controller.wait();
                panic!("renderer fixture did not start");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        controller.kill().unwrap();
        controller.wait().unwrap();
        for pid in pids {
            loop {
                let running =
                    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                        stat.rsplit_once(')')
                            .is_some_and(|(_, tail)| !tail.trim_start().starts_with('Z'))
                    });
                if !running {
                    break;
                }
                assert!(Instant::now() < deadline, "orphaned renderer process {pid}");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    #[test]
    fn cancellation_before_launch_does_not_start_a_late_renderer() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("started");
        let group = Group(Arc::new(Mutex::new(ProcessState::default())));
        group.signal(libc::SIGTERM);
        let (tx, rx) = async_channel::bounded(2);
        supervise(
            Path::new("/bin/sh"),
            &[
                "-c".into(),
                "touch \"$1\"".into(),
                "renderer".into(),
                marker.clone().into(),
            ],
            &group,
            &tx,
        );
        assert!(matches!(report(&rx, &group), Report::Spawned(Err(_))));
        assert!(!group.alive());
        assert!(!marker.exists());
    }

    #[test]
    fn an_exited_leader_does_not_wait_for_its_helpers_stderr() {
        let (tx, rx) = async_channel::bounded(2);
        let group = launch(
            "/bin/sh".into(),
            ["-c", "sleep 30 & echo ready >&2; exit 7"]
                .map(OsString::from)
                .into(),
            tx,
        );
        let Report::Spawned(Ok(pid)) = report(&rx, &group) else {
            panic!("renderer did not start")
        };
        let Report::Exited(message) = report(&rx, &group) else {
            panic!("renderer did not exit")
        };
        assert!(
            message.contains("exit status: 7") && message.contains("ready"),
            "{message}"
        );
        wait_finished(&group);
        // SAFETY: existence query only, after the worker has reaped our child.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }

    #[test]
    fn cancellation_during_launch_reaps_every_started_process() {
        for _ in 0..16 {
            let (tx, rx) = async_channel::bounded(2);
            let group = launch(
                "/bin/sh".into(),
                ["-c", "sleep 30"].map(OsString::from).into(),
                tx,
            );
            group.signal(libc::SIGKILL);
            match report(&rx, &group) {
                Report::Spawned(Err(_)) => wait_finished(&group),
                Report::Spawned(Ok(_)) => {
                    assert!(matches!(report(&rx, &group), Report::Exited(_)));
                    wait_finished(&group);
                }
                other => panic!("unexpected first report: {other:?}"),
            }
        }
    }
}
