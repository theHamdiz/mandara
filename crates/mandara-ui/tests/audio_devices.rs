//! Keyboard switching through the production picker in a marked demo desktop.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

use serde_json::Value;

/// Let device enumeration, keyboard dispatch and the server reply finish.
const SETTLE: Duration = Duration::from_secs(2);

struct Keyboard(Child);
impl Drop for Keyboard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Only a runtime created for this checkout may receive synthetic input.
fn private_runtime(runtime: &Path, sandbox: &Path) -> bool {
    runtime.canonicalize().is_ok_and(|path| path.starts_with(std::env::temp_dir()))
        && sandbox.canonicalize().is_ok_and(|root| {
            std::fs::read_to_string(runtime.join(".mandara-sandbox"))
                .is_ok_and(|marker| marker == root.to_string_lossy())
        })
}

#[test]
fn audio_keyboard_fixture_rejects_an_unmarked_desktop() {
    let runtime = tempfile::tempdir().unwrap();
    let sandbox = tempfile::tempdir().unwrap();
    assert!(!private_runtime(runtime.path(), sandbox.path()));
    std::fs::write(runtime.path().join(".mandara-sandbox"), "another desktop").unwrap();
    assert!(!private_runtime(runtime.path(), sandbox.path()));
}

#[test]
#[ignore = "requires a marked private desktop with speakers/headset Pulse outputs and wtype"]
fn tab_and_enter_switch_the_default_output_once() {
    let sandbox = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/sandbox");
    let runtime = PathBuf::from(std::env::var_os("MANDARA_AUDIO_TEST_RUNTIME").expect("private runtime"));
    assert!(private_runtime(&runtime, &sandbox), "refusing to drive an unmarked desktop");
    let xtask = PathBuf::from(std::env::var_os("MANDARA_AUDIO_TEST_XTASK").expect("sandbox controller"));
    let control = |words: &[&str]| {
        let output = Command::new(&xtask).env("MANDARA_SANDBOX_RUNTIME", &runtime).arg("sandbox").args(words).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        output.stdout
    };
    let devices: Value = serde_json::from_slice(&control(&["msg", "--json", "audio", "devices"])).unwrap();
    let outputs = devices["data"]["outputs"].as_array().unwrap();
    assert_eq!(outputs.len(), 2, "use the dedicated two-output fixture");
    for name in ["speakers", "headset"] {
        assert!(outputs.iter().any(|device| device["name"] == name));
    }
    let status = Command::new("pactl").env_clear().env("HOME", sandbox.join("home"))
        .args(["--server", &format!("unix:{}/pulse/native", runtime.display()), "set-default-sink", "speakers"])
        .status().unwrap();
    assert!(status.success());
    control(&["msg", "panel", "close"]);
    control(&["msg", "panel", "open", "audio"]);
    std::thread::sleep(SETTLE);
    let _keyboard = Keyboard(Command::new("wtype").env_clear().env("XDG_RUNTIME_DIR", &runtime).env("WAYLAND_DISPLAY", "wayland-0")
        .args(["-s", "300", "-k", "Tab", "-s", "100", "-k", "Return", "-s", "300", "-k", "Tab", "-s", "100", "-k", "Tab", "-s", "100", "-k", "Return", "-s", "10000"])
        .spawn().unwrap());
    std::thread::sleep(SETTLE);
    let devices: Value = serde_json::from_slice(&control(&["msg", "--json", "audio", "devices"])).unwrap();
    let default = devices["data"]["outputs"].as_array().unwrap().iter().find(|device| device["is_default"] == true).unwrap();
    assert_eq!(default["name"], "headset", "Enter must activate the picker once, then activate its focused choice once");
    control(&["msg", "panel", "close"]);
}
