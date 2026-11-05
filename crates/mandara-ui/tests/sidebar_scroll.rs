//! Runs the production sidebar in the explicitly marked demo desktop.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::{Value, json};

/// Let software-rendered frames and layout changes settle before a capture.
const SETTLE: Duration = Duration::from_millis(500);

/// Only the repository's disposable sandbox can be driven by this fixture.
fn private_runtime(runtime: &Path, sandbox: &Path) -> bool {
    runtime
        .canonicalize()
        .is_ok_and(|path| path.starts_with(std::env::temp_dir()))
        && sandbox.canonicalize().is_ok_and(|root| {
            std::fs::read_to_string(runtime.join(".mandara-sandbox"))
                .is_ok_and(|marker| marker == root.to_string_lossy())
        })
}

struct Session {
    xtask: PathBuf,
    runtime: PathBuf,
    config: PathBuf,
    original: Vec<u8>,
}

impl Session {
    /// Calls the sandbox controller, which verifies the runtime marker again.
    fn command(&self, words: &[&str]) -> Vec<u8> {
        let output = Command::new(&self.xtask)
            .env("MANDARA_SANDBOX_RUNTIME", &self.runtime)
            .arg("sandbox")
            .args(words)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    /// Set one demo preference without touching the real configuration directory.
    fn set(&self, path: &str, value: Value) {
        self.command(&["msg", "config", "set", path, &value.to_string()]);
    }

    /// Capture only the static tab header; clocks and live data are outside it.
    fn header(&self, path: &Path) -> image::RgbaImage {
        self.command(&["shot", path.to_str().unwrap()]);
        let image = image::open(path).unwrap().to_rgba8();
        assert_eq!(
            image.dimensions(),
            (1920, 1080),
            "use the 1920×1080 scale-1 demo fixture"
        );
        image::imageops::crop_imm(&image, 16, 46, 210, 30).to_image()
    }

    /// Compare resting frames, so the opening spring cannot imitate scrolling.
    fn stable_header(&self, path: &Path) -> image::RgbaImage {
        let mut previous = self.header(path);
        for _ in 0..6 {
            std::thread::sleep(SETTLE);
            let next = self.header(path);
            if previous == next {
                return next;
            }
            previous = next;
        }
        panic!("the sidebar header did not settle");
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Restore even if a visual assertion fails. The shell's watcher
        // reloads this private file after the test has finished driving it.
        let _ = std::fs::write(&self.config, &self.original);
        let _ = Command::new(&self.xtask)
            .env("MANDARA_SANDBOX_RUNTIME", &self.runtime)
            .args(["sandbox", "msg", "sidebar", "close"])
            .output();
    }
}

#[test]
fn sidebar_fixture_rejects_other_runtime_markers() {
    let runtime = tempfile::tempdir().unwrap();
    let sandbox = tempfile::tempdir().unwrap();
    assert!(!private_runtime(runtime.path(), sandbox.path()));
    std::fs::write(runtime.path().join(".mandara-sandbox"), "another sandbox").unwrap();
    assert!(!private_runtime(runtime.path(), sandbox.path()));
    std::fs::write(
        runtime.path().join(".mandara-sandbox"),
        sandbox
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .as_bytes(),
    )
    .unwrap();
    assert!(private_runtime(runtime.path(), sandbox.path()));
}

/// Overflowing cards must extend the scroll range without dismissing the private sidebar.
#[test]
#[ignore = "requires the explicitly marked 1920×1080 scale-1 demo desktop and its xtask binary"]
fn wheel_reaches_sidebar_cards_below_the_viewport() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let sandbox = root.join("target/sandbox");
    let runtime = PathBuf::from(
        std::env::var_os("MANDARA_SIDEBAR_TEST_RUNTIME").expect("private sandbox runtime"),
    );
    assert!(
        private_runtime(&runtime, &sandbox),
        "refusing to drive an unmarked desktop"
    );
    let config = sandbox.join("home/config/mandara/config.toml");
    let original = std::fs::read(&config).unwrap();
    let parsed: Value = toml::from_str(std::str::from_utf8(&original).unwrap()).unwrap();
    let session = Session {
        xtask: PathBuf::from(
            std::env::var_os("MANDARA_SIDEBAR_TEST_XTASK").expect("sandbox xtask binary"),
        ),
        runtime,
        config,
        original,
    };
    let mut enabled = parsed["plugins"]["enabled"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for id in ["todo", "notes", "pomodoro"] {
        if !enabled.iter().any(|entry| entry == id) {
            enabled.push(json!(id));
        }
    }
    session.command(&["studio", "stop"]);
    session.set("/plugins/enabled", json!(enabled));
    session.set("/sidebar/side", json!("left"));
    session.set("/sidebar/always_first_tab", json!(true));
    session.set("/sidebar/look/header", json!("regular"));
    session.set("/sidebar/look/tab_position", json!("top"));
    session.set("/sidebar/look/card_spacing", json!(48));
    session.set(
        "/sidebar/tabs/0/cards",
        json!([
            "quick-settings",
            "weather",
            "audio",
            "resources",
            "todo",
            "notes",
            "pomodoro"
        ]),
    );
    session.command(&["msg", "sidebar", "open"]);
    std::thread::sleep(SETTLE);
    session.command(&["pointer", "scroll", "180", "600", "-40"]);
    std::thread::sleep(SETTLE);
    let files = tempfile::tempdir().unwrap();
    let artifact_dir = std::env::var_os("MANDARA_SIDEBAR_TEST_ARTIFACTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| files.path().to_path_buf());
    std::fs::create_dir_all(&artifact_dir).unwrap();
    let before = session.stable_header(&artifact_dir.join("before.png"));
    session.command(&["pointer", "scroll", "180", "600", "40"]);
    std::thread::sleep(SETTLE);
    let after = session.stable_header(&artifact_dir.join("after.png"));
    assert!(
        before != after,
        "the header stayed fixed: overflowing cards have no scroll range"
    );
    let state: Value =
        serde_json::from_slice(&session.command(&["msg", "--json", "shell", "state"])).unwrap();
    assert_eq!(
        state["data"]["sidebar"], true,
        "scrolling inside the sidebar must not dismiss it"
    );
}
