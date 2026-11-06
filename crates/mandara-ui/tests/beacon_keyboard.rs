//! Native Beacon keyboard navigation in the marked private resource desktop.

use std::io::{Read as _, Write as _};
use std::path::Path;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use image::RgbaImage;
use mandara_config::{Locale, ShellConfig};
use mandara_ui::launcher_place::{BEACON_BAR_H, BeaconPlacement, Rect, beacon};
use serde_json::{Value, json};

#[path = "support/private_desktop.rs"]
mod private_desktop;
use private_desktop::PrivateDesktop;

/// The fixed resource desktop is 4K at scale 1.5.
const SCREEN: (f32, f32) = (2560.0, 1440.0);
const SCALE: f32 = 1.5;
/// Allow input and the software renderer to finish before capturing pixels.
const SETTLE: Duration = Duration::from_millis(350);
/// Private shell requests and clipboard reads must not leave the test blocked.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// One expected character plus one byte to detect unexpected clipboard text.
const CLIPBOARD_READ_LIMIT: u64 = 2;
/// Render native focus before the next synthetic key, as a person would.
const KEY_SETTLE_MS: &str = "100";
/// The inner button sample excludes its focus rim and surrounding shadow.
const SAMPLE_EDGE: f32 = 32.0;
/// Keep a seat keyboard present between short wtype calls in headless Mango.
const KEYBOARD_LIFETIME: Duration = Duration::from_secs(60);
/// Beacon's fixed card padding, category strip and first app-row sample.
const CATEGORY_PAD: f32 = 8.0;
const CATEGORY_H: f32 = 30.0;
const GRID_SAMPLE_H: f32 = 80.0;
/// Allow antialiased rim pixels while distinguishing them from the chip fill.
const RIM_TOLERANCE: u8 = 96;

struct Process(Child);

impl Process {
    /// Bounds every short-lived input/capture tool and reaps it through Drop on failure.
    fn finish(&mut self) {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() { assert!(status.success(), "native tool failed: {status}"); return; }
            assert!(Instant::now() < deadline, "native tool timed out");
            std::thread::sleep(SETTLE.min(deadline.saturating_duration_since(Instant::now())));
        }
    }
}

impl Drop for Process {
    /// Reaps only this test's recorded child, including on panic or timeout.
    fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
}

/// Calls the bound private shell; the guard checks this exact peer before writing.
fn command(fixture: &PrivateDesktop, args: &[&str]) -> Value {
    fixture.request(args).expect("verified private shell request")
}

/// Config fields changed by either native test, restored even after an assertion fails.
const CHANGED_CONFIG: [&str; 5] = ["/general/locale", "/motion/reduce_motion", "/launcher/look/beacon_width", "/launcher/look/grid_categories", "/launcher/providers/clipboard/enabled"];

struct RestoreConfig<'a> {
    fixture: &'a PrivateDesktop,
    fields: Vec<(&'static str, Value)>,
}

impl<'a> RestoreConfig<'a> {
    /// Snapshot exact original values before the test changes its private configuration.
    fn new(fixture: &'a PrivateDesktop) -> Self {
        let config = command(fixture, &["config", "get"]);
        let fields = CHANGED_CONFIG.into_iter().map(|path| (path, config.pointer(path).expect("fixture config field").clone())).collect();
        Self { fixture, fields }
    }
}

/// Stop immediately if a guarded request fails, including a changed fixture identity.
fn restore_config(fields: &[(&str, Value)], mut request: impl FnMut(&[&str]) -> std::io::Result<Value>) -> std::io::Result<()> {
    request(&["launcher", "close"])?;
    for (path, value) in fields { request(&["config", "set", path, &value.to_string()])?; }
    Ok(())
}

impl Drop for RestoreConfig<'_> {
    /// Restoration is best effort and never unwraps or panics during unwinding.
    fn drop(&mut self) {
        if let Err(error) = restore_config(&self.fields, |args| self.fixture.request(args)) {
            let _ = writeln!(std::io::stderr(), "private Beacon config restoration stopped: {error}");
        }
    }
}

/// Sends real Wayland key events to the validated display. Pace complete key
/// presses and allow the resulting frame to settle before examining pixels.
fn input(fixture: &PrivateDesktop, args: &[&str]) {
    let mut paced = Vec::new();
    let mut key = false;
    for arg in args {
        paced.push(*arg);
        if key { paced.extend(["-s", KEY_SETTLE_MS]); }
        key = *arg == "-k";
    }
    Process(fixture.tool("wtype").unwrap().args(paced).spawn().unwrap()).finish();
    std::thread::sleep(SETTLE);
}

/// Rechecks isolation before reading the native clipboard. The child inherits
/// only the private Wayland endpoint; both its lifetime and captured bytes are
/// bounded, including when selection transfer never completes.
fn clipboard(fixture: &PrivateDesktop) -> Vec<u8> {
    let mut process = Process(fixture.tool("wl-paste").unwrap().arg("--no-newline").stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap());
    let output = process.0.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = output.take(CLIPBOARD_READ_LIMIT).read_to_end(&mut bytes).map(|_| bytes);
        let _ = tx.send(result);
    });
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let status = loop {
        if let Some(status) = process.0.try_wait().unwrap() { break status; }
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "private clipboard reader timed out");
        std::thread::sleep(SETTLE.min(remaining));
    };
    assert!(status.success(), "private clipboard reader failed: {status}");
    rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("private clipboard read timed out").expect("private clipboard read failed")
}

/// Captures the native output, checking its expected 4K geometry so button
/// samples cannot silently read the wrong display or scale.
fn capture(fixture: &PrivateDesktop) -> RgbaImage {
    fixture.check().unwrap();
    let directory = tempfile::tempdir_in(&fixture.runtime).unwrap();
    let path = directory.path().join("beacon.png");
    Process(fixture.tool("grim").unwrap().arg(&path).spawn().unwrap()).finish();
    let image = image::open(path).unwrap().to_rgba8();
    assert_eq!(image.dimensions(), ((SCREEN.0 * SCALE) as u32, (SCREEN.1 * SCALE) as u32));
    image
}

/// Samples one button in device pixels: its interior tests selection without
/// focus-rim noise, while the whole button tests visible keyboard focus.
fn sample(image: &RgbaImage, placement: BeaconPlacement, index: usize, rim: bool) -> RgbaImage {
    let edge = if rim { BEACON_BAR_H } else { SAMPLE_EDGE };
    let rect = Rect::new(placement.first_mode.x + placement.mode_step * index as f32, placement.first_mode.y, BEACON_BAR_H, BEACON_BAR_H);
    let inset = (BEACON_BAR_H - edge) / 2.0;
    image::imageops::crop_imm(image, ((rect.x + inset) * SCALE).round() as u32, ((rect.y + inset) * SCALE).round() as u32, (edge * SCALE).round() as u32, (edge * SCALE).round() as u32).to_image()
}

/// Separates category focus/selection from actual app-grid filtering pixels.
fn category_sample(image: &RgbaImage, placement: BeaconPlacement, grid: bool) -> RgbaImage {
    let rect = placement.card;
    let y = rect.y + CATEGORY_PAD + if grid { CATEGORY_H + CATEGORY_PAD } else { 0.0 };
    let height = if grid { GRID_SAMPLE_H } else { CATEGORY_H };
    image::imageops::crop_imm(image, ((rect.x + CATEGORY_PAD) * SCALE).round() as u32, (y * SCALE).round() as u32,
        ((rect.w - CATEGORY_PAD * 2.0) * SCALE).round() as u32, (height * SCALE).round() as u32).to_image()
}

/// Both rounded end rims must appear within the viewport. A clipped sliver
/// changes pixels too, but contains only one end of the focused category.
fn complete_category_focus(strip: &RgbaImage, accent: image::Rgba<u8>, rtl: bool) -> bool {
    let range = if rtl { 0..strip.width() / 2 } else { strip.width() / 2..strip.width() };
    let mut edges = 0;
    let mut first = None;
    let mut last = 0;
    let mut inside = false;
    // Above the label, the rounded corners inset both rims from the crop edge.
    let y = strip.height() / 4;
    for x in range {
        let pixel = strip.get_pixel(x, y);
        let matches = (0..3).all(|channel| pixel[channel].abs_diff(accent[channel]) <= RIM_TOLERANCE);
        if matches {
            if !inside { edges += 1; first.get_or_insert(x); }
            last = x;
        }
        inside = matches;
    }
    edges >= 2 && first.is_some_and(|first| last - first >= (CATEGORY_H * SCALE) as u32)
}

/// Pixel changes alone cannot accept a focused chip clipped at either edge.
#[test]
fn category_focus_probe_rejects_a_clipped_end() {
    let accent = image::Rgba([60, 140, 180, 255]);
    for (rtl, ends) in [(true, [3, 80]), (false, [219, 296])] {
        let mut strip = RgbaImage::new(300, 45);
        for x in ends { strip.put_pixel(x, 11, accent); }
        assert!(complete_category_focus(&strip, accent, rtl));
        strip.put_pixel(ends[0], 11, image::Rgba([0, 0, 0, 0]));
        assert!(!complete_category_focus(&strip, accent, rtl));
    }
}

/// Exercises native focus and activation in English and Arabic, including
/// hidden providers and the first character typed from a button. Escape first
/// clears its active mode, then closes Beacon when no mode remains.
#[test]
#[ignore = "requires the marked 4K/1.5 resource desktop, Beacon, grim, wtype and wl-paste"]
fn tab_shift_tab_enter_and_space_reach_beacon_modes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture = PrivateDesktop::from_env("MANDARA_BEACON_TEST", &root, ".mandara-beacon-keyboard-test").expect("verified private desktop");
    let _restore = RestoreConfig::new(&fixture);
    let _keyboard = Process(fixture.tool("wtype").unwrap().args(["-s", &KEYBOARD_LIFETIME.as_millis().to_string()]).spawn().unwrap());
    std::thread::sleep(SETTLE);
    assert_eq!(command(&fixture, &["config", "get", "/launcher/look/style"]), "beacon");
    assert_eq!(command(&fixture, &["config", "get", "/general/offline"]), true);
    command(&fixture, &["config", "set", "/motion/reduce_motion", "true"]);

    for (locale, rtl) in [(Locale::English, false), (Locale::Arabic, true)] {
        let language = serde_json::to_string(&locale).unwrap();
        command(&fixture, &["launcher", "close"]);
        std::thread::sleep(SETTLE);
        command(&fixture, &["config", "set", "/general/locale", &language]);
        command(&fixture, &["launcher", "open"]);
        std::thread::sleep(SETTLE);
        let cfg: ShellConfig = serde_json::from_value(command(&fixture, &["config", "get"])).unwrap();
        let placement = beacon(&cfg, SCREEN, true, 4, rtl);
        let initial = capture(&fixture);
        input(&fixture, &["q"]);
        input(&fixture, &["-M", "ctrl", "-k", "a", "-k", "x", "-m", "ctrl"]);
        assert_eq!(clipboard(&fixture).as_slice(), b"q", "seed a different selection so an empty query cannot pass using stale clipboard text ({language})");
        input(&fixture, &["-k", "Tab"]);
        let focused = capture(&fixture);
        assert_eq!(sample(&initial, placement, 0, false), sample(&focused, placement, 0, false), "Tab focuses Apps without selecting it");
        assert_ne!(sample(&initial, placement, 0, true), sample(&focused, placement, 0, true), "keyboard focus needs a visible rim ({language})");
        input(&fixture, &["z"]);
        input(&fixture, &["-M", "ctrl", "-k", "a", "-k", "c", "-m", "ctrl"]);
        assert_eq!(clipboard(&fixture).as_slice(), b"z", "the first character typed from a focused mode button must reach the actual query ({language})");
        input(&fixture, &["-M", "ctrl", "-k", "a", "-m", "ctrl", "-k", "BackSpace", "-k", "Tab"]);
        input(&fixture, &["-k", "Return"]);
        let apps = sample(&capture(&fixture), placement, 0, false);
        assert_ne!(apps, sample(&initial, placement, 0, false), "Enter must select Apps ({language})");
        input(&fixture, &["-M", "ctrl", "-k", "1", "-m", "ctrl"]);
        input(&fixture, &["-M", "ctrl", "-k", "1", "-m", "ctrl"]);
        assert_eq!(apps, sample(&capture(&fixture), placement, 0, false), "Ctrl+1 keeps its selection behavior");
        // Ctrl+1 returned focus to the search: traverse Apps, then Files.
        input(&fixture, &["-k", "Tab", "-k", "Tab", "-k", "space"]);
        let files = sample(&capture(&fixture), placement, 1, false);
        assert_ne!(files, sample(&initial, placement, 1, false), "Space must select Files ({language})");
        input(&fixture, &["-M", "shift", "-k", "Tab", "-m", "shift", "-k", "Return"]);
        assert_eq!(apps, sample(&capture(&fixture), placement, 0, false), "Shift+Tab must return from Files to Apps ({language})");
        // Leaving Apps hides its category stops before checking mode-only wrap.
        input(&fixture, &["-M", "ctrl", "-k", "2", "-m", "ctrl", "-M", "shift", "-k", "Tab", "-m", "shift", "-k", "space"]);
        assert_ne!(sample(&capture(&fixture), placement, 3, false), sample(&initial, placement, 3, false), "reverse traversal wraps through search to Clipboard ({language})");
        assert_eq!(command(&fixture, &["shell", "state"])["launcher"], true, "mode activation must not launch a result or dismiss Beacon");
        input(&fixture, &["-k", "Escape"]);
        assert_eq!(command(&fixture, &["shell", "state"])["launcher"], true, "Escape from the selected Clipboard button must clear its mode and keep Beacon open ({language})");
        let cleared = capture(&fixture);
        assert_eq!(sample(&cleared, placement, 3, false), sample(&initial, placement, 3, false), "Escape must remove the active mode's selection ({language})");
        input(&fixture, &["-k", "Tab"]);
        let neutral_focus = capture(&fixture);
        assert_eq!(sample(&neutral_focus, placement, 0, false), sample(&initial, placement, 0, false), "Tab must focus Apps without selecting a mode after Escape ({language})");
        assert_ne!(sample(&neutral_focus, placement, 0, true), sample(&cleared, placement, 0, true), "the second Escape must start from a visibly focused button ({language})");
        input(&fixture, &["-k", "Escape"]);
        assert_eq!(command(&fixture, &["shell", "state"])["launcher"], false, "Escape from a focused button with no active mode must close Beacon ({language})");
    }
    command(&fixture, &["launcher", "close"]);
    std::thread::sleep(SETTLE);
    command(&fixture, &["config", "set", "/launcher/providers/clipboard/enabled", "false"]);
    command(&fixture, &["launcher", "open"]);
    std::thread::sleep(SETTLE);
    let cfg: ShellConfig = serde_json::from_value(command(&fixture, &["config", "get"])).unwrap();
    let placement = beacon(&cfg, SCREEN, true, 3, true);
    let initial = capture(&fixture);
    input(&fixture, &["-M", "shift", "-k", "Tab", "-m", "shift", "-k", "Return"]);
    assert_ne!(sample(&capture(&fixture), placement, 2, false), sample(&initial, placement, 2, false), "hidden Clipboard must be skipped so reverse traversal reaches Actions");
    command(&fixture, &["launcher", "close"]);
}

/// A narrow Apps grid includes overflowing categories. Reverse traversal must
/// reveal its final chip; Enter/Space filter real app tiles, and forward focus
/// returns to All/Pinned after the visible modes without launching an app.
#[test]
#[ignore = "requires the marked Beacon fixture, harmless demo apps in every category, one pinned demo, grim, wtype and wl-paste"]
fn apps_categories_join_native_focus_and_reveal_overflow() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture = PrivateDesktop::from_env("MANDARA_BEACON_TEST", &root, ".mandara-beacon-keyboard-test").expect("verified private desktop");
    let _restore = RestoreConfig::new(&fixture);
    assert_eq!(command(&fixture, &["config", "get", "/general/offline"]), true);
    let pinned = command(&fixture, &["config", "get", "/launcher/pinned"]);
    assert!(pinned.as_array().unwrap().iter().any(|id| id == "mandara-beacon-demo-0"), "pin the harmless fixture app before opening Beacon");
    command(&fixture, &["config", "set", "/launcher/look/beacon_width", "240"]);
    command(&fixture, &["config", "set", "/launcher/look/grid_categories", "true"]);
    command(&fixture, &["config", "set", "/motion/reduce_motion", "true"]);
    let _keyboard = Process(fixture.tool("wtype").unwrap().args(["-s", &KEYBOARD_LIFETIME.as_millis().to_string()]).spawn().unwrap());
    for (locale, rtl) in [(Locale::English, false), (Locale::Arabic, true)] {
        command(&fixture, &["launcher", "close"]);
        command(&fixture, &["config", "set", "/general/locale", &serde_json::to_string(&locale).unwrap()]);
        command(&fixture, &["launcher", "open"]);
        std::thread::sleep(SETTLE);
        input(&fixture, &["q", "-M", "ctrl", "-k", "a", "-k", "x", "-m", "ctrl"]);
        assert_eq!(clipboard(&fixture).as_slice(), b"q");
        input(&fixture, &["-M", "ctrl", "-k", "1", "-m", "ctrl"]);
        let cfg: ShellConfig = serde_json::from_value(command(&fixture, &["config", "get"])).unwrap();
        let place = beacon(&cfg, SCREEN, true, 4, rtl);
        let all = capture(&fixture);
        input(&fixture, &["-M", "shift", "-k", "Tab", "-m", "shift"]);
        let last = capture(&fixture);
        assert!(category_sample(&all, place, false) != category_sample(&last, place, false), "reverse focus must visibly reveal the off-edge final category ({locale:?})");
        category_sample(&last, place, false).save(fixture.artifact(&format!("beacon-categories-{locale:?}-last-focus.png")).unwrap()).unwrap();
        let accent = *all.get_pixel(((place.first_mode.x + BEACON_BAR_H / 2.0) * SCALE).round() as u32,
            ((place.first_mode.y + BEACON_BAR_H * 0.2) * SCALE).round() as u32);
        assert!(complete_category_focus(&category_sample(&last, place, false), accent, rtl), "focused category must show both ends inside the viewport ({locale:?})");
        input(&fixture, &["-k", "Return"]);
        let filtered = capture(&fixture);
        assert!(category_sample(&all, place, true) != category_sample(&filtered, place, true), "Enter must filter actual app tiles ({locale:?})");
        assert_eq!(command(&fixture, &["shell", "state"])["launcher"], true);
        input(&fixture, &["-k", "space"]);
        assert!(category_sample(&filtered, place, true) != category_sample(&capture(&fixture), place, true), "Space must restore All from the selected category ({locale:?})");
        input(&fixture, &["-M", "ctrl", "-k", "1", "-k", "1", "-m", "ctrl"]);
        input(&fixture, &["-k", "Tab", "-k", "Tab", "-k", "Tab", "-k", "Tab", "-k", "Tab"]);
        let all_focus = capture(&fixture);
        assert!(category_sample(&all, place, false) != category_sample(&all_focus, place, false), "All needs visible native focus after the four mode stops ({locale:?})");
        input(&fixture, &["-k", "Tab", "-k", "Return"]);
        let pinned_only = capture(&fixture);
        assert!(category_sample(&all, place, true) != category_sample(&pinned_only, place, true), "Pinned must filter actual app tiles ({locale:?})");
        category_sample(&pinned_only, place, false).save(fixture.artifact(&format!("beacon-categories-{locale:?}-pinned.png")).unwrap()).unwrap();
        input(&fixture, &["-k", "space", "-M", "shift", "-k", "Tab", "-m", "shift", "-k", "Return"]);
        assert_eq!(command(&fixture, &["shell", "state"])["launcher"], true, "category activation must not launch an app");
        input(&fixture, &["z", "-M", "ctrl", "-k", "a", "-k", "c", "-m", "ctrl"]);
        assert_eq!(clipboard(&fixture).as_slice(), b"z", "typing from a focused category must preserve its first character");
        input(&fixture, &["-M", "ctrl", "-k", "a", "-m", "ctrl", "-k", "BackSpace", "-k", "Escape", "-k", "Escape"]);
        assert_eq!(command(&fixture, &["shell", "state"])["launcher"], false);
    }
}

/// All original values are restored, including a provider originally disabled.
#[test]
fn config_restoration_preserves_values_and_stops_after_a_rejected_request() {
    let fields = [("/general/locale", json!("arabic")), ("/motion/reduce_motion", json!(false)), ("/launcher/look/beacon_width", json!(420)), ("/launcher/look/grid_categories", json!(false)), ("/launcher/providers/clipboard/enabled", json!(false))];
    let mut sent = Vec::new();
    restore_config(&fields, |args| { sent.push(args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>()); Ok(Value::Null) }).unwrap();
    assert_eq!(sent[0], ["launcher", "close"]);
    for (request, (path, value)) in sent[1..].iter().zip(&fields) {
        assert_eq!(&request[..3], ["config", "set", path]);
        assert_eq!(serde_json::from_str::<Value>(&request[3]).unwrap(), *value);
    }
    let mut attempts = 0;
    assert!(restore_config(&fields, |_| { attempts += 1; Err(std::io::Error::other("fixture identity changed")) }).is_err());
    assert_eq!(attempts, 1, "no config writes follow a rejected guard");
    let mut attempts = 0;
    assert!(restore_config(&fields, |_| { attempts += 1; if attempts == 3 { Err(std::io::Error::other("fixture identity changed")) } else { Ok(Value::Null) } }).is_err());
    assert_eq!(attempts, 3, "restoration stops as soon as identity is lost");
}
