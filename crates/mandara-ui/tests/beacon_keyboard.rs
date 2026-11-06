//! Native Beacon keyboard navigation in the marked private resource desktop.

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

use image::RgbaImage;
use mandara_config::{Locale, ShellConfig};
use mandara_ui::launcher_place::{BEACON_BAR_H, BeaconPlacement, Rect, beacon};
use serde_json::{Value, json};

/// The fixed resource desktop is 4K at scale 1.5.
const SCREEN: (f32, f32) = (2560.0, 1440.0);
const SCALE: f32 = 1.5;
/// Allow input and the software renderer to finish before capturing pixels.
const SETTLE: Duration = Duration::from_millis(350);
/// A dead private shell cannot leave the test blocked on its reply.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Render native focus before the next synthetic key, as a person would.
const KEY_SETTLE_MS: &str = "100";
/// The inner button sample excludes its focus rim and surrounding shadow.
const SAMPLE_EDGE: f32 = 32.0;
/// Keep a seat keyboard present between short wtype calls in headless Mango.
const KEYBOARD_LIFETIME: Duration = Duration::from_secs(60);

struct Keyboard(Child);

impl Drop for Keyboard {
    fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
}

fn private_runtime(runtime: &Path, root: &Path) -> bool {
    let Ok(temporary) = std::env::temp_dir().canonicalize() else { return false; };
    runtime.canonicalize().is_ok_and(|runtime| runtime.starts_with(temporary)
        && runtime.file_name().is_some_and(|name| name.to_string_lossy().starts_with("mrc."))
        && root.canonicalize().is_ok_and(|root| std::fs::read_to_string(runtime.join(".mandara-beacon-keyboard-test")).is_ok_and(|marker| marker == root.to_string_lossy())))
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

fn input(runtime: &Path, display: &str, args: &[&str]) {
    let mut paced = Vec::new();
    let mut key = false;
    for arg in args {
        paced.push(*arg);
        if key { paced.extend(["-s", KEY_SETTLE_MS]); }
        key = *arg == "-k";
    }
    let status = Command::new("wtype").env_clear().env("XDG_RUNTIME_DIR", runtime).env("WAYLAND_DISPLAY", display).args(paced).status().unwrap();
    assert!(status.success());
    std::thread::sleep(SETTLE);
}

fn capture(runtime: &Path, display: &str) -> RgbaImage {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("beacon.png");
    let status = Command::new("grim").env_clear().env("XDG_RUNTIME_DIR", runtime).env("WAYLAND_DISPLAY", display).arg(&path).status().unwrap();
    assert!(status.success());
    let image = image::open(path).unwrap().to_rgba8();
    assert_eq!(image.dimensions(), ((SCREEN.0 * SCALE) as u32, (SCREEN.1 * SCALE) as u32));
    image
}

fn sample(image: &RgbaImage, placement: BeaconPlacement, index: usize, rim: bool) -> RgbaImage {
    let edge = if rim { BEACON_BAR_H } else { SAMPLE_EDGE };
    let rect = Rect::new(placement.first_mode.x + placement.mode_step * index as f32, placement.first_mode.y, BEACON_BAR_H, BEACON_BAR_H);
    let inset = (BEACON_BAR_H - edge) / 2.0;
    image::imageops::crop_imm(image, ((rect.x + inset) * SCALE).round() as u32, ((rect.y + inset) * SCALE).round() as u32, (edge * SCALE).round() as u32, (edge * SCALE).round() as u32).to_image()
}

#[test]
fn beacon_keyboard_fixture_rejects_missing_and_mismatched_markers() {
    let root = tempfile::tempdir().unwrap();
    let runtime = tempfile::Builder::new().prefix("mrc.").tempdir().unwrap();
    assert!(!private_runtime(runtime.path(), root.path()));
    std::fs::write(runtime.path().join(".mandara-beacon-keyboard-test"), "another checkout").unwrap();
    assert!(!private_runtime(runtime.path(), root.path()));
    std::fs::write(runtime.path().join(".mandara-beacon-keyboard-test"), root.path().canonicalize().unwrap().to_string_lossy().as_bytes()).unwrap();
    assert!(private_runtime(runtime.path(), root.path()));
}

#[test]
#[ignore = "requires the marked 4K/1.5 resource desktop, Beacon, grim and wtype"]
fn tab_shift_tab_enter_and_space_reach_beacon_modes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runtime = PathBuf::from(std::env::var_os("MANDARA_BEACON_TEST_RUNTIME").expect("private resource runtime"));
    assert!(private_runtime(&runtime, &root), "refusing synthetic input outside the marked desktop");
    let display = std::env::var("MANDARA_BEACON_TEST_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    let wayland = Path::new(&display);
    assert!(!wayland.is_absolute() && wayland.components().count() == 1, "use a socket in the marked runtime");
    let _keyboard = Keyboard(Command::new("wtype").env_clear().env("XDG_RUNTIME_DIR", &runtime).env("WAYLAND_DISPLAY", &display)
        .args(["-s", &KEYBOARD_LIFETIME.as_millis().to_string()]).spawn().unwrap());
    std::thread::sleep(SETTLE);
    assert_eq!(command(&runtime, &["config", "get", "/launcher/look/style"]), "beacon");
    assert_eq!(command(&runtime, &["config", "get", "/general/offline"]), true);
    command(&runtime, &["config", "set", "/motion/reduce_motion", "true"]);

    for (locale, rtl) in [(Locale::English, false), (Locale::Arabic, true)] {
        let language = serde_json::to_string(&locale).unwrap();
        command(&runtime, &["launcher", "close"]);
        std::thread::sleep(SETTLE);
        command(&runtime, &["config", "set", "/general/locale", &language]);
        command(&runtime, &["launcher", "open"]);
        std::thread::sleep(SETTLE);
        let cfg: ShellConfig = serde_json::from_value(command(&runtime, &["config", "get"])).unwrap();
        let placement = beacon(&cfg, SCREEN, true, 4, rtl);
        let initial = capture(&runtime, &display);
        input(&runtime, &display, &["-k", "Tab"]);
        let focused = capture(&runtime, &display);
        assert_eq!(sample(&initial, placement, 0, false), sample(&focused, placement, 0, false), "Tab focuses Apps without selecting it");
        assert_ne!(sample(&initial, placement, 0, true), sample(&focused, placement, 0, true), "keyboard focus needs a visible rim ({language})");
        input(&runtime, &display, &["-k", "Return"]);
        let apps = sample(&capture(&runtime, &display), placement, 0, false);
        assert_ne!(apps, sample(&initial, placement, 0, false), "Enter must select Apps ({language})");
        input(&runtime, &display, &["-M", "ctrl", "-k", "1", "-m", "ctrl"]);
        input(&runtime, &display, &["-M", "ctrl", "-k", "1", "-m", "ctrl"]);
        assert_eq!(apps, sample(&capture(&runtime, &display), placement, 0, false), "Ctrl+1 keeps its selection behavior");
        // Ctrl+1 returned focus to the search: traverse Apps, then Files.
        input(&runtime, &display, &["-k", "Tab", "-k", "Tab", "-k", "space"]);
        let files = sample(&capture(&runtime, &display), placement, 1, false);
        assert_ne!(files, sample(&initial, placement, 1, false), "Space must select Files ({language})");
        input(&runtime, &display, &["-M", "shift", "-k", "Tab", "-m", "shift", "-k", "Return"]);
        assert_eq!(apps, sample(&capture(&runtime, &display), placement, 0, false), "Shift+Tab must return from Files to Apps ({language})");
        input(&runtime, &display, &["-M", "shift", "-k", "Tab", "-k", "Tab", "-m", "shift", "-k", "space"]);
        assert_ne!(sample(&capture(&runtime, &display), placement, 3, false), sample(&initial, placement, 3, false), "reverse traversal wraps through search to Clipboard ({language})");
        assert_eq!(command(&runtime, &["shell", "state"])["launcher"], true, "mode activation must not launch a result or dismiss Beacon");
    }
    command(&runtime, &["launcher", "close"]);
    std::thread::sleep(SETTLE);
    command(&runtime, &["config", "set", "/launcher/providers/clipboard/enabled", "false"]);
    command(&runtime, &["launcher", "open"]);
    std::thread::sleep(SETTLE);
    let cfg: ShellConfig = serde_json::from_value(command(&runtime, &["config", "get"])).unwrap();
    let placement = beacon(&cfg, SCREEN, true, 3, true);
    let initial = capture(&runtime, &display);
    input(&runtime, &display, &["-M", "shift", "-k", "Tab", "-m", "shift", "-k", "Return"]);
    assert_ne!(sample(&capture(&runtime, &display), placement, 2, false), sample(&initial, placement, 2, false), "hidden Clipboard must be skipped so reverse traversal reaches Actions");
    command(&runtime, &["launcher", "close"]);
    command(&runtime, &["config", "set", "/launcher/providers/clipboard/enabled", "true"]);
}
