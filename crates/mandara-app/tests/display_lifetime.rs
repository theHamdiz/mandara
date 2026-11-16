//! Native regression for replacing the shell's last output surfaces.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const START_TIMEOUT: Duration = Duration::from_secs(30);
const PROBE_INTERVAL: Duration = Duration::from_millis(100);
const SCALE_SETTLE: Duration = Duration::from_secs(1);

struct Sandbox {
    xtask: PathBuf,
    runtime: tempfile::TempDir,
}

impl Sandbox {
    fn command(&self, args: &[&str]) -> std::process::Output {
        Command::new(&self.xtask)
            .env("MANDARA_SANDBOX_RUNTIME", self.runtime.path())
            .args(["sandbox"])
            .args(args)
            .output()
            .expect("run sandbox command")
    }

    fn run(&self, args: &[&str]) -> String {
        let output = self.command(args);
        assert!(
            output.status.success(),
            "{args:?}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn wait(&self, ready: impl FnMut() -> bool) {
        self.wait_with_diagnostics(ready, String::new);
    }

    fn wait_with_diagnostics(
        &self,
        mut ready: impl FnMut() -> bool,
        diagnostics: impl Fn() -> String,
    ) {
        let deadline = Instant::now() + START_TIMEOUT;
        while !ready() {
            assert!(
                Instant::now() < deadline,
                "sandbox did not reach the expected state: {}",
                diagnostics()
            );
            std::thread::sleep(PROBE_INTERVAL);
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires MangoWM, bwrap, grim and MANDARA_TEST_XTASK pointing to a built xtask"]
fn a_display_disconnect_fails_instead_of_stopping_the_supervisor() {
    display_disconnect(false);
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires MangoWM, bwrap, grim and MANDARA_TEST_XTASK pointing to a built xtask"]
fn the_supervisor_starts_another_shell_after_a_display_disconnect() {
    display_disconnect(true);
}

#[cfg(target_os = "linux")]
fn display_disconnect(supervised: bool) {
    use std::{
        ffi::OsString,
        io::Write as _,
        os::unix::{ffi::OsStringExt as _, fs::MetadataExt as _},
        process::Stdio,
    };

    let xtask = std::env::var_os("MANDARA_TEST_XTASK")
        .expect("set MANDARA_TEST_XTASK to the built xtask binary");
    let sandbox = Sandbox {
        xtask: PathBuf::from(xtask),
        runtime: tempfile::Builder::new().prefix("mdrs-").tempdir().unwrap(),
    };
    let binary = Path::new(env!("CARGO_BIN_EXE_mandara"));
    let config = sandbox.runtime.path().join("config.toml");
    std::fs::write(&config, "[general]\nauto_restart = true\n").unwrap();
    sandbox.run(&["up", "--bin", binary.to_str().unwrap(), "--no-langpacks", "--config", config.to_str().unwrap()]);
    sandbox.wait(|| sandbox.command(&["msg", "ping"]).status.success());

    // Reuse the capsule xtask created, retaining its hidden home and dead system bus.
    let recorded_pid = |name| {
        std::fs::read_to_string(sandbox.runtime.path().join("pids").join(name))
            .unwrap()
            .lines()
            .last()
            .unwrap()
            .parse::<i32>()
            .unwrap()
    };
    let process_env = |pid| {
        let bytes = std::fs::read(format!("/proc/{pid}/environ")).unwrap();
        let pairs: Vec<_> = bytes
            .split(|&byte| byte == 0)
            .filter_map(|entry| {
                let split = entry.iter().position(|&byte| byte == b'=')?;
                Some((
                    OsString::from_vec(entry[..split].to_vec()),
                    OsString::from_vec(entry[split + 1..].to_vec()),
                ))
            })
            .collect();
        assert!(pairs.iter().any(|(key, value)| key == "XDG_RUNTIME_DIR"
            && value.as_os_str() == sandbox.runtime.path().as_os_str()));
        pairs
    };
    let pid = recorded_pid("shell");
    let args: Vec<_> = std::fs::read(format!("/proc/{pid}/cmdline"))
        .unwrap()
        .split(|&byte| byte == 0)
        .filter(|arg| !arg.is_empty())
        .map(|arg| OsString::from_vec(arg.to_vec()))
        .collect();
    assert_eq!(
        &args[args.len() - 2..],
        &[binary.as_os_str(), std::ffi::OsStr::new("run")]
    );
    let env = process_env(pid);
    assert!(
        env.iter()
            .any(|(key, value)| key == "DBUS_SYSTEM_BUS_ADDRESS"
                && value == "unix:path=/nonexistent")
    );
    sandbox.run(&["msg", "quit"]);
    sandbox.wait(|| !sandbox.command(&["msg", "ping"]).status.success());

    let stderr = tempfile::NamedTempFile::new().unwrap();
    let replay_args = if supervised {
        &args[1..args.len() - 1]
    } else {
        &args[1..]
    };
    let mut child = Command::new(&args[0])
        .arg("--die-with-parent")
        .args(replay_args)
        .env_clear()
        .envs(env)
        .env("RUST_LOG", "info")
        .stdout(Stdio::null())
        .stderr(stderr.reopen().unwrap())
        .spawn()
        .unwrap();
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(sandbox.runtime.path().join("pids/shell"))
            .unwrap(),
        "{}",
        child.id()
    )
    .unwrap();
    sandbox.wait_with_diagnostics(
        || sandbox.command(&["msg", "ping"]).status.success(),
        || std::fs::read_to_string(stderr.path()).unwrap(),
    );
    let inner = std::fs::read_to_string(format!("/proc/{0}/task/{0}/children", child.id()))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse::<u32>()
        .unwrap();
    // Keep the namespace pinned until all of its processes have gone.
    let namespace = std::fs::File::open(format!("/proc/{inner}/ns/pid")).unwrap();
    let namespace_id = namespace.metadata().unwrap().ino();
    assert_ne!(
        namespace_id,
        std::fs::metadata("/proc/self/ns/pid").unwrap().ino()
    );
    let compositor = recorded_pid("mango");
    process_env(compositor);
    // SAFETY: this is the recorded compositor of this test's private runtime.
    assert_eq!(unsafe { libc::kill(compositor, libc::SIGTERM) }, 0);
    if supervised {
        sandbox.wait_with_diagnostics(
            || {
                std::fs::read_to_string(stderr.path())
                    .unwrap()
                    .matches("config:")
                    .count()
                    >= 2
            },
            || std::fs::read_to_string(stderr.path()).unwrap(),
        );
        assert!(child.try_wait().unwrap().is_none());
        sandbox.run(&["shell", "stop"]);
    }
    sandbox.wait_with_diagnostics(
        || child.try_wait().unwrap().is_some(),
        || std::fs::read_to_string(stderr.path()).unwrap(),
    );
    let status = child.wait().unwrap();
    let diagnostics = std::fs::read_to_string(stderr.path()).unwrap();
    if !supervised {
        assert_eq!(status.code(), Some(1), "{}", diagnostics);
    }
    assert!(diagnostics.contains("display event loop stopped without a quit request"));
    if supervised {
        assert!(diagnostics.contains("Mandara exited (exit code 1); restarting"));
    }
    sandbox.wait_with_diagnostics(
        || {
            std::fs::read_dir("/proc")
                .unwrap()
                .flatten()
                .all(|process| {
                !std::fs::metadata(process.path().join("ns/pid"))
                        .is_ok_and(|metadata| metadata.ino() == namespace_id)
                })
        },
        || diagnostics.clone(),
    );
    drop(namespace);
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        self.command(&["down"]);
    }
}

#[test]
#[ignore = "requires MangoWM, bwrap, grim and MANDARA_TEST_XTASK pointing to a built xtask"]
fn fractional_scale_changes_keep_the_shell_alive_without_other_windows() {
    let xtask = std::env::var_os("MANDARA_TEST_XTASK")
        .expect("set MANDARA_TEST_XTASK to the built xtask binary");
    let sandbox = Sandbox {
        xtask: PathBuf::from(xtask),
        runtime: tempfile::Builder::new().prefix("mdrs-").tempdir().unwrap(),
    };
    let config = tempfile::NamedTempFile::new().unwrap();
    // No dock, panels or GPUI wallpaper can keep the application alive while
    // the shell and exclusion strip are replaced. Studio is a separate process.
    std::fs::write(config.path(), "[general]\noffline = true\n[plugins]\nenabled = [\"clock\"]\n[wallpaper]\nenabled = false\n[sidebar]\nenabled = false\n").unwrap();
    let binary = Path::new(env!("CARGO_BIN_EXE_mandara"));
    sandbox.run(&[
        "up",
        "--bin",
        binary.to_str().unwrap(),
        "--no-langpacks",
        "--config",
        config.path().to_str().unwrap(),
        "--size",
        "3840x2160",
        "--scale",
        "1.5",
    ]);
    sandbox.wait(|| sandbox.command(&["msg", "ping"]).status.success());

    for scale in ["1.25", "1.5", "1.25", "1.5"] {
        sandbox.run(&["msg", "display", "set", "HEADLESS-1", "scale", scale]);
        std::thread::sleep(SCALE_SETTLE);
        assert_eq!(
            sandbox.run(&["msg", "ping"]).trim(),
            "pong",
            "scale {scale}"
        );
        sandbox.run(&["msg", "display", "confirm"]);
    }

    // Keeping the shell alive without windows must still allow a deliberate exit.
    sandbox.run(&["msg", "quit"]);
    sandbox.wait(|| !sandbox.runtime.path().join("mandara.sock").exists());
}
