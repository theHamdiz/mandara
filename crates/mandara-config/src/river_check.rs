//! River's stand-in for a config checker, for the tests: River has none, so
//! the commands Mandara would send are run in a throwaway headless River
//! (its own runtime dir, home and config, the headless backend with
//! software rendering, no input devices) through `riverctl`, which speaks
//! the same `river-control` protocol as Mandara's driver. A command passes
//! when `riverctl` exits 0 and prints nothing (River answers a failed
//! command with an error and an overwritten bind with a note).
//!
//! Skipped (`None`) where River's classic `riverctl` isn't installed; CI's
//! `compositor-checkers` job installs it.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long River gets to create its Wayland socket.
const START_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(20);

/// Kills River when the check ends (or panics).
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn installed() -> bool {
    let ok = |p: &str| Command::new(p).arg("-version").stdin(Stdio::null()).output().is_ok_and(|o| o.status.success());
    ok("river") && ok("riverctl")
}

/// Runs `commands` against a headless River whose `init` holds `init`
/// (shell lines, e.g. the user's own maps): every command that failed or
/// printed something, with what River said; `None` when River isn't
/// installed.
pub(crate) fn run_commands_with_init(init: &str, commands: &[Vec<String>]) -> Option<Vec<String>> {
    if !installed() {
        eprintln!("river (classic, with riverctl) isn't installed; skipped");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config");
    std::fs::create_dir_all(config.join("river")).unwrap();
    let init_path = config.join("river/init");
    // init's last line marks it done, so its maps are all in before
    // Mandara's commands run.
    let done = dir.path().join("init-done");
    std::fs::write(&init_path, format!("#!/bin/sh\n{init}\ntouch {}\n", crate::compositor::shell_quote(&done.display().to_string()))).unwrap();
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&init_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let runtime = dir.path().join("rt");
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new().mode(0o700).create(&runtime).unwrap();
    }
    let env = |cmd: &mut Command| {
        cmd.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", dir.path())
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("WLR_BACKENDS", "headless")
            .env("WLR_RENDERER", "pixman")
            .env("WLR_LIBINPUT_NO_DEVICES", "1")
            .stdin(Stdio::null());
    };
    let mut river = Command::new("river");
    env(&mut river);
    let _river = Running(river.stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    let display = wait_for_socket(&runtime).expect("River created no Wayland socket");
    let deadline = Instant::now() + START_TIMEOUT;
    while !done.exists() {
        assert!(Instant::now() < deadline, "River's init never finished");
        std::thread::sleep(POLL);
    }
    let mut failures = Vec::new();
    for words in commands {
        let mut ctl = Command::new("riverctl");
        env(&mut ctl);
        let out = ctl.args(words).env("WAYLAND_DISPLAY", &display).output().unwrap();
        let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)).trim().to_string();
        if !out.status.success() || !said.is_empty() {
            failures.push(format!("{} → {}", crate::compositor::riverctl_line(words), if said.is_empty() { format!("exit {}", out.status) } else { said }));
        }
    }
    Some(failures)
}

/// [`run_commands_with_init`] with an empty `init`.
pub(crate) fn run_commands(commands: &[Vec<String>]) -> Option<Vec<String>> {
    run_commands_with_init("", commands)
}

fn wait_for_socket(runtime: &Path) -> Option<String> {
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        let found = std::fs::read_dir(runtime).ok()?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| n.starts_with("wayland-") && !n.ends_with(".lock"));
        if found.is_some() {
            return found;
        }
        std::thread::sleep(POLL);
    }
    None
}
