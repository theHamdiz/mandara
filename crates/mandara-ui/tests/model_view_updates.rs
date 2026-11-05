//! Model-owned views stay cached when an unrelated service changes.

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

/// The fixture's service replies and renderer settle before counters are read.
const SETTLE: Duration = Duration::from_millis(500);
/// Requests stay bounded even if a fixture process exits unexpectedly.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Space notifications across separate frames rather than one batched update.
const EVENT_SPACING: Duration = Duration::from_millis(100);
const MODEL_FILES: [&str; 5] = ["ci.rs:", "feeds.rs:", "home_assistant.rs:", "phone_connect.rs:", "translator.rs:"];
const MODEL_IDS: [&str; 5] = ["ci", "feeds", "home-assistant", "phone-connect", "translator"];

/// Only the resource fixture's private runtime accepts synthetic notifications.
fn private_runtime(runtime: &Path, root: &Path) -> bool {
    runtime.canonicalize().is_ok_and(|path| {
        path.starts_with(std::env::temp_dir())
            && path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("mrc."))
            && root.canonicalize().is_ok_and(|root| {
                std::fs::read_to_string(path.join(".mandara-plugin-resource-test"))
                    .is_ok_and(|marker| marker == root.to_string_lossy())
            })
    })
}

fn command(runtime: &Path, args: &[&str]) -> Value {
    let mut socket = UnixStream::connect(runtime.join("mandara.sock")).unwrap();
    socket.set_read_timeout(Some(REQUEST_TIMEOUT)).unwrap();
    socket.set_write_timeout(Some(REQUEST_TIMEOUT)).unwrap();
    writeln!(socket, "{}", json!({"args": args})).unwrap();
    let mut line = String::new();
    BufReader::new(socket).read_line(&mut line).unwrap();
    let reply: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    reply["data"].clone()
}

fn renders(rows: &Value, file: &str) -> u64 {
    rows.as_array().unwrap().iter()
        .filter(|row| row["view"].as_str().is_some_and(|view| view.starts_with(file)))
        .map(|row| row["renders"].as_u64().unwrap()).sum()
}

#[test]
fn plugin_resource_fixture_rejects_an_unmarked_runtime() {
    let root = tempfile::tempdir().unwrap();
    let runtime = tempfile::Builder::new().prefix("mrc.").tempdir().unwrap();
    assert!(!private_runtime(runtime.path(), root.path()));
    std::fs::write(runtime.path().join(".mandara-plugin-resource-test"), "another checkout").unwrap();
    assert!(!private_runtime(runtime.path(), root.path()));
    std::fs::write(runtime.path().join(".mandara-plugin-resource-test"), root.path().canonicalize().unwrap().to_string_lossy().as_bytes()).unwrap();
    assert!(private_runtime(runtime.path(), root.path()));
}

#[test]
#[ignore = "requires the marked private resource fixture with the five model-only bar chips"]
fn unrelated_notifications_leave_model_views_cached() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runtime = PathBuf::from(std::env::var_os("MANDARA_PLUGIN_UPDATE_TEST_RUNTIME").expect("private resource runtime"));
    assert!(private_runtime(&runtime, &root), "refusing to drive an unmarked desktop");
    let layout = command(&runtime, &["config", "get", "/bar/right"]);
    assert_eq!(layout, json!(MODEL_IDS), "use the dedicated model-view fixture");
    assert_eq!(command(&runtime, &["config", "get", "/general/offline"]), true);
    assert_eq!(command(&runtime, &["config", "get", "/notifications/min_urgency"]), "normal", "history-only events keep popup-window creation out of the comparison");
    std::thread::sleep(SETTLE);
    let initial = command(&runtime, &["diagnostics", "views", "reset"]);
    for file in MODEL_FILES { assert!(renders(&initial, file) > 0, "{file} must have rendered before the comparison"); }
    for index in 0..20 {
        command(&runtime, &["notifications", "send", "--low", &format!("Resource check {index}")]);
        std::thread::sleep(EVENT_SPACING);
    }
    std::thread::sleep(SETTLE);
    let after = command(&runtime, &["diagnostics", "views", "reset"]);
    for file in MODEL_FILES { assert_eq!(renders(&after, file), 0, "unrelated notifications rebuilt {file}"); }

    // The CI model still owns its notifications: an explicit refresh updates
    // its views without invalidating the four independent integration chips.
    command(&runtime, &["ci", "refresh"]);
    std::thread::sleep(SETTLE);
    let own = command(&runtime, &["diagnostics", "views"]);
    assert!(renders(&own, "ci.rs:") > 0, "an own-model refresh must remain visible");
    for file in &MODEL_FILES[1..] { assert_eq!(renders(&own, file), 0, "a CI refresh rebuilt {file}"); }
}
