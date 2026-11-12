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
        assert!(output.status.success(), "{args:?}: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap()
    }

    fn wait(&self, ready: impl Fn() -> bool) {
        let deadline = Instant::now() + START_TIMEOUT;
        while !ready() {
            assert!(Instant::now() < deadline, "sandbox did not reach the expected state");
            std::thread::sleep(PROBE_INTERVAL);
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        self.command(&["down"]);
    }
}

#[test]
#[ignore = "requires MangoWM, bwrap, grim and MANDARA_TEST_XTASK pointing to a built xtask"]
fn fractional_scale_changes_keep_the_shell_alive_without_other_windows() {
    let xtask = std::env::var_os("MANDARA_TEST_XTASK").expect("set MANDARA_TEST_XTASK to the built xtask binary");
    let sandbox = Sandbox { xtask: PathBuf::from(xtask), runtime: tempfile::Builder::new().prefix("mdrs-").tempdir().unwrap() };
    let config = tempfile::NamedTempFile::new().unwrap();
    // No dock, panels or GPUI wallpaper can keep the application alive while
    // the shell and exclusion strip are replaced. Studio is a separate process.
    std::fs::write(config.path(), "[general]\noffline = true\n[plugins]\nenabled = [\"clock\"]\n[wallpaper]\nenabled = false\n[sidebar]\nenabled = false\n").unwrap();
    let binary = Path::new(env!("CARGO_BIN_EXE_mandara"));
    sandbox.run(&["up", "--bin", binary.to_str().unwrap(), "--no-langpacks", "--config", config.path().to_str().unwrap(), "--size", "3840x2160", "--scale", "1.5"]);
    sandbox.wait(|| sandbox.command(&["msg", "ping"]).status.success());

    for scale in ["1.25", "1.5", "1.25", "1.5"] {
        sandbox.run(&["msg", "display", "set", "HEADLESS-1", "scale", scale]);
        std::thread::sleep(SCALE_SETTLE);
        assert_eq!(sandbox.run(&["msg", "ping"]).trim(), "pong", "scale {scale}");
        sandbox.run(&["msg", "display", "confirm"]);
    }

    // Keeping the shell alive without windows must still allow a deliberate exit.
    sandbox.run(&["msg", "quit"]);
    sandbox.wait(|| !sandbox.runtime.path().join("mandara.sock").exists());
}
