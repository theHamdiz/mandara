//! A renderer guardian that survives its controller long enough to stop helpers.

use std::ffi::OsString;
use std::io::{self, Read as _, Write as _};
use std::os::fd::AsRawFd as _;
use std::path::Path;

/// Maximum stderr retained from an external renderer.
pub const MAX_STDERR: usize = 2048;
/// Absolute search path for renderer helpers.
pub const CHILD_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
/// Display, user folders, language and GPU selection needed by a renderer.
const CHILD_ENV: &[&str] = &[
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "HOME",
    "LANG",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "XDG_DATA_HOME",
    "XDG_DATA_DIRS",
    "DRI_PRIME",
];
/// Check for an exited leader even when a helper keeps stderr open.
const EXIT_CHECK_MS: i32 = 250;

/// Guardian notifications, one JSON value per line on stdout.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum Report {
    Spawned(Result<i32, String>),
    Exited(String),
}

/// Give the guardian and renderer only their display and GPU environment.
pub fn inherit_environment(cmd: &mut std::process::Command) {
    for key in CHILD_ENV {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
}

/// Establishes parent-death behavior before exec and detects a fork-time death.
///
/// # Safety
/// Call only in the child between fork and exec; this uses async-signal-safe syscalls.
pub unsafe fn parent_death_signal(signal: i32, parent: u32) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, signal) } == -1 {
        return Err(io::Error::last_os_error());
    }
    #[cfg(target_os = "freebsd")]
    {
        let mut signal = signal;
        if unsafe {
            libc::procctl(
                libc::P_PID,
                0,
                libc::PROC_PDEATHSIG_CTL,
                (&raw mut signal).cast(),
            )
        } == -1
        {
            return Err(io::Error::last_os_error());
        }
    }
    if unsafe { libc::getppid() } as u32 != parent {
        return Err(io::Error::from_raw_os_error(libc::ESRCH));
    }
    Ok(())
}

fn report(report: Report) -> io::Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer(&mut out, &report)?;
    writeln!(out)?;
    out.flush()
}

/// The guardian leads the group, whose id stays reserved by its controller.
/// EOF ends the entire group; SIGCONT on parent death also wakes a paused group.
pub fn run(program: &Path, argv: &[OsString]) -> io::Result<()> {
    // SAFETY: getpgrp is a read-only identity query.
    if unsafe { libc::getpgrp() } != std::process::id() as i32 {
        return Err(io::Error::other(
            "renderer guardian needs its own process group",
        ));
    }
    // SIGTERM targets renderer helpers too. The guardian reports their exit first.
    // SAFETY: process-local disposition, before spawning the renderer.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
    std::thread::Builder::new()
        .name("renderer-control".into())
        .spawn(move || {
            let mut byte = [0];
            let mut input = io::stdin().lock();
            loop {
                match input.read(&mut byte) {
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    _ => break,
                }
            }
            // SAFETY: this process leads its dedicated group, with no unrelated members.
            unsafe {
                libc::killpg(std::process::id() as i32, libc::SIGKILL);
            }
        })?;
    render(program, argv)
}

fn render(program: &Path, argv: &[OsString]) -> io::Result<()> {
    use std::os::unix::process::CommandExt as _;
    let mut command = std::process::Command::new(program);
    command
        .args(argv)
        .env_clear()
        .env("PATH", CHILD_PATH)
        .current_dir("/")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    inherit_environment(&mut command);
    let parent = std::process::id();
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        command.pre_exec(move || {
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
            parent_death_signal(libc::SIGKILL, parent)
        });
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            report(Report::Spawned(Err(error.to_string())))?;
            return Ok(());
        }
    };
    report(Report::Spawned(Ok(child.id() as i32)))?;
    let tail = wait_and_drain(child.stderr.take(), child.id() as i32);
    let status = child.wait()?.to_string();
    let message = if tail.trim().is_empty() {
        status
    } else {
        format!("{status}: {}", tail.trim())
    };
    report(Report::Exited(message))
}

/// Bound the drain after leader exit even if a helper ignores termination.
const EXIT_DRAIN_BYTES: usize = 1024 * 1024;

/// Drains without letting an inherited pipe keep a dead renderer alive.
/// WNOWAIT reserves the leader's id until its entire group has been stopped.
fn wait_and_drain(mut pipe: Option<std::process::ChildStderr>, pid: i32) -> String {
    let (mut kept, mut buf) = (
        std::collections::VecDeque::with_capacity(MAX_STDERR),
        [0u8; 512],
    );
    let mut exited_bytes = 0;
    loop {
        // SAFETY: only inspect our unreaped child; WNOHANG never blocks.
        let ended = unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
            ) == 0
                && info.si_pid() != 0
        };
        if ended {
            // SAFETY: the leader is still unreaped, reserving the group id.
            unsafe { libc::killpg(std::process::id() as i32, libc::SIGTERM) };
        }
        let mut fd = libc::pollfd {
            fd: pipe.as_ref().map_or(-1, |p| p.as_raw_fd()),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: fd is one initialized pollfd; negative descriptors are ignored.
        let ready = unsafe { libc::poll(&raw mut fd, 1, if ended { 0 } else { EXIT_CHECK_MS }) };
        if ready > 0
            && let Some(reader) = &mut pipe
        {
            match reader.read(&mut buf) {
                Ok(0) => pipe = None,
                Ok(n) => {
                    if ended {
                        exited_bytes += n;
                        if exited_bytes > EXIT_DRAIN_BYTES {
                            break;
                        }
                    }
                    for &b in &buf[..n] {
                        if kept.len() == MAX_STDERR {
                            kept.pop_front();
                        }
                        kept.push_back(b);
                    }
                    continue;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => pipe = None,
            }
        }
        if ended {
            break;
        }
    }
    let bytes: Vec<u8> = kept.into();
    String::from_utf8_lossy(&bytes).into_owned()
}
