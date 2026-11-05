//! Exercises the real exclusive-layer switcher and Hyprland's focus reply.
//! The compositor, two application windows and Mandara must already be
//! running in a private test session; this never starts a desktop session.

use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mandara_compositor::{CompositorBackend, HyprlandDriver};
use serde_json::Value;

/// Explicit opt-in to commands that change the isolated session's focus.
const MARKER: &str = "mandara-hyprland-switcher-test\n";
/// A software-rendered test desktop can take several frames to map or close.
const DEADLINE: Duration = Duration::from_secs(5);
/// Poll only while this test waits for a compositor acknowledgement.
const STEP: Duration = Duration::from_millis(20);
/// Both opt-in tests drive the same isolated compositor.
static SESSION: Mutex<()> = Mutex::new(());

/// Commands require an explicit marker in a temporary directory.
fn private_runtime(path: &Path) -> bool {
    path.canonicalize().is_ok_and(|p| p.starts_with(std::env::temp_dir()) && std::fs::read_to_string(p.join(".mandara-switcher-test")).is_ok_and(|s| s == MARKER))
}

/// Uses the real shell IPC reply, including its success envelope.
fn msg(bin: &Path, words: &[&str]) -> Value {
    let output = Command::new(bin).args(["msg", "--json"]).args(words).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    reply["data"].clone()
}

/// The surface must actually be mapped before the test sends Commit;
/// committing before the compositor receives the keyboard grab misses the race.
fn overlay_mapped(runtime: &Path) -> bool {
    /// Find the switcher namespace inside either version of Hyprland's nested layer reply.
    fn contains_switcher(value: &Value) -> bool {
        match value {
            Value::Array(values) => values.iter().any(contains_switcher),
            Value::Object(fields) => fields.get("namespace").and_then(Value::as_str) == Some("mandara-switcher") || fields.values().any(contains_switcher),
            _ => false,
        }
    }
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").unwrap();
    let mut socket = UnixStream::connect(runtime.join("hypr").join(sig).join(".socket.sock")).unwrap();
    socket.set_read_timeout(Some(DEADLINE)).unwrap();
    socket.write_all(b"j/layers").unwrap();
    let mut reply = String::new();
    socket.read_to_string(&mut reply).unwrap();
    contains_switcher(&serde_json::from_str(&reply).unwrap())
}

#[test]
fn switcher_fixture_rejects_an_unmarked_directory() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!private_runtime(dir.path()));
    std::fs::write(dir.path().join(".mandara-switcher-test"), "another test\n").unwrap();
    assert!(!private_runtime(dir.path()));
    std::fs::write(dir.path().join(".mandara-switcher-test"), MARKER).unwrap();
    assert!(private_runtime(dir.path()));
}

/// After the exclusive overlay closes, the selected application must own focus.
#[test]
#[ignore = "requires an explicitly marked private Hyprland session with two application windows and Mandara running"]
fn committing_the_exclusive_switcher_focuses_the_selected_window() {
    let _session = SESSION.lock().unwrap();
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("private XDG_RUNTIME_DIR"));
    assert!(private_runtime(&runtime), "refusing to send commands outside the marked temporary session");
    let bin = PathBuf::from(std::env::var_os("MANDARA_SWITCHER_TEST_BIN").expect("MANDARA_SWITCHER_TEST_BIN"));
    let driver = HyprlandDriver::from_env().expect("private Hyprland socket");
    let before = driver.get_active_client().unwrap().expect("an application must have focus");
    let opened = msg(&bin, &["switcher", "open"]);
    assert!(opened["windows"].as_u64().unwrap() >= 2);
    let end = Instant::now() + DEADLINE;
    while !overlay_mapped(&runtime) && Instant::now() < end {
        std::thread::sleep(STEP);
    }
    assert!(overlay_mapped(&runtime), "exclusive switcher surface did not map");
    let status = msg(&bin, &["switcher", "status"]);
    let selected = status["selected"].as_u64().unwrap() as usize;
    let target = &status["windows"][selected];
    let app = target["app_id"].as_str().unwrap();
    let title = target["title"].as_str().unwrap();
    assert_ne!((before.app_id.as_str(), before.title.as_str()), (app, title), "the fixture must select a different window");
    msg(&bin, &["switcher", "commit"]);
    let end = Instant::now() + DEADLINE;
    loop {
        let active = driver.get_active_client().unwrap();
        let closed = !msg(&bin, &["switcher", "status"])["open"].as_bool().unwrap();
        if closed && active.as_ref().is_some_and(|c| c.app_id == app && c.title == title) {
            break;
        }
        assert!(Instant::now() < end, "switcher closed={closed}; expected {app}/{title}, focused={active:?}");
        std::thread::sleep(STEP);
    }
}

/// A real modifier-release gesture must close the overlay and activate another window.
#[test]
#[ignore = "requires the marked private Hyprland session, generated Alt-Tab bindings, two windows and wtype"]
fn releasing_alt_focuses_the_previous_window() {
    let _session = SESSION.lock().unwrap();
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("private XDG_RUNTIME_DIR"));
    assert!(private_runtime(&runtime), "refusing to send keys outside the marked temporary session");
    let bin = PathBuf::from(std::env::var_os("MANDARA_SWITCHER_TEST_BIN").expect("MANDARA_SWITCHER_TEST_BIN"));
    let driver = HyprlandDriver::from_env().expect("private Hyprland socket");
    assert!(driver.get_all_clients().unwrap().len() >= 2);
    let before = driver.get_active_client().unwrap().expect("an application must have focus");
    // The sleep keeps this virtual keyboard alive until the overlay has mapped.
    let output = Command::new("wtype").args(["-M", "alt", "-k", "Tab", "-s", "1000", "-m", "alt"]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let end = Instant::now() + DEADLINE;
    loop {
        let closed = !msg(&bin, &["switcher", "status"])["open"].as_bool().unwrap();
        let active = driver.get_active_client().unwrap();
        if closed && active.as_ref().is_some_and(|c| c.id != before.id) { break; }
        assert!(Instant::now() < end, "Alt release did not focus another window; closed={closed}, before={before:?}, active={active:?}");
        std::thread::sleep(STEP);
    }
}
