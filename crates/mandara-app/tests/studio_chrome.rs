//! Native Studio chrome checks against an explicitly selected private sandbox.

use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::Value;

const READY_TIMEOUT: Duration = Duration::from_secs(15);
const PROBE_INTERVAL: Duration = Duration::from_millis(100);
const APPEARANCE_SETTLE: Duration = Duration::from_secs(2);
const TOOLTIP_SETTLE: Duration = Duration::from_millis(1500);
const MAX_LOG_BYTES: u64 = 256 * 1024;
const MAX_CROP_BYTES: usize = 1024 * 1024;
/// The first traffic-light target, inset from the title bar's leading edge.
const FIRST_CONTROL_CENTER: f64 = 22.0;
/// Below the lights, so a changed crop proves more than their hover glyphs.
const TOOLTIP_REGION: (i64, i64, u32, u32) = (0, 10, 120, 35);
/// Sample navigation labels away from the overlay scrollbar and rounded edges.
const NAV_CROP: (f64, f64) = (120.0, 180.0);
/// Enough wheel steps to reach either end of the complete navigation list.
const NAV_SCROLL_STEPS: &str = "120";
const NAV_SCROLL_BACK: &str = "-120";
/// Let hover fills and scrolling settle before comparing the same crop.
const NAV_SETTLE: Duration = Duration::from_millis(300);

struct Sandbox {
    xtask: PathBuf,
    runtime: PathBuf,
    log: PathBuf,
    studio: Option<Value>,
    _lock: File,
}

fn lock_sandbox(runtime: &Path) -> File {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(runtime.join("studio-chrome-test.lock"))
        .unwrap();
    lock.try_lock()
        .expect("another Studio chrome test is using this sandbox");
    lock
}

#[test]
fn sandbox_lock_excludes_another_test_until_the_guard_is_dropped() {
    let directory = tempfile::tempdir().unwrap();
    let guard = lock_sandbox(directory.path());
    let contender = OpenOptions::new()
        .write(true)
        .open(directory.path().join("studio-chrome-test.lock"))
        .unwrap();
    assert!(contender.try_lock().is_err());
    drop(guard);
    assert!(contender.try_lock().is_ok());
}

impl Sandbox {
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.xtask);
        command
            .env("MANDARA_SANDBOX_RUNTIME", &self.runtime)
            .arg("sandbox")
            .args(args);
        command
    }

    fn run(&self, args: &[&str]) -> String {
        let output = self.command(args).output().expect("run sandbox command");
        successful(args, &output);
        String::from_utf8(output.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut words = vec!["msg", "--json"];
        words.extend_from_slice(args);
        let reply: Value = serde_json::from_str(&self.run(&words)).unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
        reply["data"].clone()
    }

    fn client(&self) -> Option<Value> {
        self.json(&["compositor", "clients"])
            .as_array()
            .unwrap()
            .iter()
            .find(|client| client["app_id"] == "org.mandara.studio")
            .cloned()
    }

    fn wait(&self, ready: impl Fn() -> bool) {
        let deadline = Instant::now() + READY_TIMEOUT;
        while !ready() {
            assert!(
                Instant::now() < deadline,
                "sandbox did not reach the expected state"
            );
            std::thread::sleep(PROBE_INTERVAL);
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let Some(original) = &self.studio else { return };
        let _ = self.command(&["studio", "stop"]).output();
        let original = original.to_string();
        let restored = self
            .command(&["msg", "--json", "config", "set", "/studio", &original])
            .output();
        assert!(
            restored.is_ok_and(|output| output.status.success()) || std::thread::panicking(),
            "could not restore the sandbox's Studio appearance"
        );
    }
}

fn successful(args: &[&str], output: &Output) {
    assert!(
        output.status.success(),
        "{args:?}: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn display_from_env(text: &str) -> Option<&str> {
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix("export WAYLAND_DISPLAY="))?;
    let display = line.strip_prefix('\'')?.strip_suffix('\'')?;
    (!display.is_empty()
        && display
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && display != "."
        && display != "..")
        .then_some(display)
}

#[test]
fn capture_display_rejects_paths_and_shell_syntax() {
    assert_eq!(
        display_from_env("export WAYLAND_DISPLAY='wayland-0'"),
        Some("wayland-0")
    );
    for display in [
        "",
        "..",
        "/run/user/1000/wayland-0",
        "../wayland-0",
        "$(command)",
        "wayland-0' extra='",
    ] {
        assert_eq!(
            display_from_env(&format!("export WAYLAND_DISPLAY='{display}'")),
            None
        );
    }
}

#[test]
#[ignore = "requires a running private MangoWM sandbox, grim, MANDARA_TEST_XTASK and MANDARA_STUDIO_TEST_RUNTIME"]
fn window_control_tooltip_uses_the_studio_font_and_close_removes_the_window() {
    let runtime = PathBuf::from(
        std::env::var_os("MANDARA_STUDIO_TEST_RUNTIME").expect("set MANDARA_STUDIO_TEST_RUNTIME"),
    )
    .canonicalize()
    .unwrap();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    assert!(runtime.starts_with("/tmp") || runtime.starts_with("/var/tmp"));
    assert_eq!(
        fs::read_to_string(runtime.join(".mandara-sandbox"))
            .unwrap()
            .trim(),
        repo.join("target/sandbox").to_str().unwrap()
    );
    let xtask =
        PathBuf::from(std::env::var_os("MANDARA_TEST_XTASK").expect("set MANDARA_TEST_XTASK"));
    let lock = lock_sandbox(&runtime);
    let mut sandbox = Sandbox {
        xtask,
        runtime,
        log: repo.join("target/sandbox/logs/shell.log"),
        studio: None,
        _lock: lock,
    };
    sandbox.studio = Some(sandbox.json(&["config", "get", "/studio"]));
    let environment = sandbox.run(&["env"]);
    assert!(
        environment
            .lines()
            .any(|line| line == "export DBUS_SYSTEM_BUS_ADDRESS='unix:path=/nonexistent'")
    );
    let display = display_from_env(&environment).expect("private Wayland socket name");
    sandbox.run(&["studio", "stop"]);
    sandbox.run(&["shell", "restart"]);
    sandbox.wait(|| {
        sandbox
            .command(&["msg", "ping"])
            .output()
            .is_ok_and(|output| output.status.success())
    });
    sandbox.json(&["studio", "look", "apply", "cupertino-glass"]);
    sandbox.run(&["studio", "themes/studio"]);
    sandbox.wait(|| sandbox.client().is_some());
    std::thread::sleep(APPEARANCE_SETTLE);
    let client = sandbox.client().unwrap();
    let frame = sandbox.json(&["config", "get", "/studio/frame"]);
    let geometry = client["geometry"].as_array().unwrap();
    let inset = frame["inset"].as_f64().unwrap() + frame["border"].as_f64().unwrap();
    let x = (geometry[0].as_f64().unwrap() + inset + FIRST_CONTROL_CENTER).round() as i64;
    let y = (geometry[1].as_f64().unwrap() + inset + frame["title_height"].as_f64().unwrap() / 2.0)
        .round() as i64;
    let region = format!(
        "{},{} {}x{}",
        x + TOOLTIP_REGION.0,
        y + TOOLTIP_REGION.1,
        TOOLTIP_REGION.2,
        TOOLTIP_REGION.3
    );
    let scratch = tempfile::tempdir().unwrap();
    let capture = |region: &str, name: &str| {
        let path = scratch.path().join(name);
        let output = Command::new("grim")
            .env("XDG_RUNTIME_DIR", &sandbox.runtime)
            .env("WAYLAND_DISPLAY", display)
            .env("DBUS_SYSTEM_BUS_ADDRESS", "unix:path=/nonexistent")
            .args(["-g", region])
            .arg(&path)
            .output()
            .expect("capture tooltip region");
        successful(&["grim", "-g", region], &output);
        assert!(fs::metadata(&path).unwrap().len() <= MAX_CROP_BYTES as u64);
        fs::read(path).unwrap()
    };
    let nav_x =
        geometry[0].as_f64().unwrap() + inset + frame["sidebar_width"].as_f64().unwrap() / 2.0;
    let nav_y = geometry[1].as_f64().unwrap() + geometry[3].as_f64().unwrap() / 2.0;
    let nav_region = format!(
        "{},{} {}x{}",
        (nav_x - NAV_CROP.0 / 2.0).round(),
        (nav_y - NAV_CROP.1 / 2.0).round(),
        NAV_CROP.0,
        NAV_CROP.1
    );
    let away_x = (geometry[0].as_f64().unwrap() + geometry[2].as_f64().unwrap() / 2.0)
        .round()
        .to_string();
    let nav_x = nav_x.round().to_string();
    let nav_y = nav_y.round().to_string();
    sandbox.run(&["pointer", "move", &away_x, &nav_y]);
    std::thread::sleep(NAV_SETTLE);
    let nav_before = capture(&nav_region, "nav-before.png");
    sandbox.run(&["pointer", "scroll", &nav_x, &nav_y, NAV_SCROLL_STEPS]);
    sandbox.run(&["pointer", "move", &away_x, &nav_y]);
    std::thread::sleep(NAV_SETTLE);
    assert!(
        nav_before != capture(&nav_region, "nav-scrolled.png"),
        "the sidebar must scroll within the window"
    );
    sandbox.run(&["pointer", "scroll", &nav_x, &nav_y, NAV_SCROLL_BACK]);
    sandbox.run(&["pointer", "move", &away_x, &nav_y]);
    std::thread::sleep(NAV_SETTLE);
    assert!(nav_before == capture(&nav_region, "nav-restored.png"));
    let before = capture(&region, "before.png");
    assert_eq!(
        before,
        capture(&region, "stable.png"),
        "the tooltip region must be still before hovering"
    );
    let log_start = fs::metadata(&sandbox.log).unwrap().len();
    let (x, y) = (x.to_string(), y.to_string());
    let mut hover = sandbox
        .command(&["pointer", "flyby", &x, &y, "4000"])
        .spawn()
        .unwrap();
    std::thread::sleep(TOOLTIP_SETTLE);
    let shown = capture(&region, "tooltip.png");
    assert!(hover.wait().unwrap().success());
    assert_ne!(before, shown, "the tooltip must actually be visible");
    let mut log = File::open(&sandbox.log).unwrap();
    log.seek(SeekFrom::Start(log_start)).unwrap();
    let mut appended = Vec::new();
    log.take(MAX_LOG_BYTES + 1)
        .read_to_end(&mut appended)
        .unwrap();
    assert!(appended.len() <= MAX_LOG_BYTES as usize);
    let appended = String::from_utf8_lossy(&appended);
    assert!(!appended.contains("failed to load font"), "{appended}");
    sandbox.run(&["pointer", "click", &x, &y]);
    sandbox.wait(|| sandbox.client().is_none());
}
