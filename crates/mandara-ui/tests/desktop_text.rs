//! Native editing uses only a recorded, isolated resource desktop.

use std::io::{self, Read as _};
use std::path::Path;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use image::RgbaImage;

#[path = "support/private_desktop.rs"]
mod private_desktop;
use private_desktop::PrivateDesktop;

/// Input is paced so complete native frames separate actions.
const SETTLE: Duration = Duration::from_millis(350);
/// Failed native tools are reaped before the fixture can be reused.
const TOOL_TIMEOUT: Duration = Duration::from_secs(5);
/// Keep a seat keyboard present between short wtype processes.
const KEYBOARD_LIFETIME: &str = "180000";
/// Contains only this test's short draft.
const CLIPBOARD_LIMIT: u64 = 4096;
/// The fixed 4K/1.5 fixture places only the note editor in this logical crop.
const NOTE_REGION: &str = "1300,150 800x500";
const NOTE_ORIGIN: (u32, u32) = (1300, 150);
const NOTE_PIXELS: (u32, u32) = (1200, 750);
const SCALE: f32 = 1.5;
/// A focus rim changes strongly; ignore small antialiasing differences.
const FOCUS_DELTA: u8 = 32;
/// The square icon button fits these device-pixel bounds at the fixture scale.
const FOCUS_SIDE: std::ops::RangeInclusive<u32> = 36..=96;
/// Keep the virtual device present until the native click and capture settle.
const POINTER_HOLD_MS: &str = "4000";

struct Process(Child);

impl Process {
    fn finish(&mut self) {
        let deadline = Instant::now() + TOOL_TIMEOUT;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "native tool failed: {status}");
                return;
            }
            assert!(Instant::now() < deadline, "native tool timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
}

fn command(fixture: &PrivateDesktop, args: &[&str]) -> Value {
    fixture.request(args).unwrap_or_else(|error| panic!("verified private request {args:?}: {error}"))
}

/// Await the actual shared state after a debounced or background save.
fn wait_for(fixture: &PrivateDesktop, args: &[&str], ready: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + TOOL_TIMEOUT;
    loop {
        let value = command(fixture, args);
        if ready(&value) { return value; }
        assert!(Instant::now() < deadline, "private state did not settle for {args:?}: {value}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Await the real keyboard focus, without treating an allocated window as mapped.
fn keyboard_ready(fixture: &PrivateDesktop) -> io::Result<bool> {
    let deadline = Instant::now() + TOOL_TIMEOUT;
    loop {
        let state = fixture.request(&["shell", "state"])?;
        match state.get("desktop_editor_ready") {
            Some(Value::Null) => return Ok(false),
            Some(Value::Bool(true)) => return Ok(true),
            Some(Value::Bool(false)) => {}
            _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "missing desktop keyboard readiness")),
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "desktop editor did not receive keyboard focus"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn input(fixture: &PrivateDesktop, args: &[&str]) {
    assert!(keyboard_ready(fixture).expect("verified desktop keyboard readiness"), "the desktop editor must be open before sending keys");
    Process(fixture.tool("wtype").unwrap().args(["-d", "80"]).args(args).spawn().unwrap()).finish();
    std::thread::sleep(SETTLE);
}

fn clipboard(fixture: &PrivateDesktop) -> String {
    let mut process = Process(fixture.tool("wl-paste").unwrap().arg("--no-newline").stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap());
    let output = process.0.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let result = output.take(CLIPBOARD_LIMIT).read_to_string(&mut text).map(|_| text);
        let _ = tx.send(result);
    });
    let text = rx.recv_timeout(TOOL_TIMEOUT).unwrap().unwrap();
    process.finish();
    text
}

struct Restore<'a> {
    fixture: &'a PrivateDesktop,
    config: Vec<(&'static str, Value)>,
    tasks: Vec<String>,
    notes: Vec<String>,
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        let restore = || -> io::Result<()> {
            if keyboard_ready(self.fixture)? {
                let mut escape = Process(self.fixture.tool("wtype")?.args(["-k", "Escape"]).spawn()?);
                let deadline = Instant::now() + TOOL_TIMEOUT;
                while escape.0.try_wait()?.is_none() {
                    if Instant::now() >= deadline { return Err(io::Error::other("editor cleanup timed out")); }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            for id in &self.tasks { self.fixture.request(&["todo", "remove", id])?; }
            for id in &self.notes { self.fixture.request(&["notes", "delete", id])?; }
            for (path, value) in &self.config { self.fixture.request(&["config", "set", path, &value.to_string()])?; }
            Ok(())
        };
        if let Err(error) = restore() { eprintln!("private desktop text restoration stopped: {error}"); }
    }
}

fn assert_no_panel(fixture: &PrivateDesktop) {
    let state = command(fixture, &["shell", "state"]);
    assert!(state["panel"].is_null(), "desktop editing must not open a plugin dropdown");
    assert_eq!(state.get("desktop_editor_ready"), Some(&Value::Null), "a closed desktop editor must report no input target");
}

/// The count is used only to prove that the editor window was removed.
fn window_count(fixture: &PrivateDesktop) -> u64 {
    command(fixture, &["diagnostics", "memory"])["counts"]["windows"].as_u64().unwrap()
}

/// Window removal follows the save receipt rather than a fixed frame delay.
fn wait_windows(fixture: &PrivateDesktop, expected: u64) {
    wait_for(fixture, &["diagnostics", "memory"], |value| value["counts"]["windows"].as_u64() == Some(expected));
}

/// Captures the note's fixed region through the same verified display as input.
fn note_capture(fixture: &PrivateDesktop, name: &str) -> RgbaImage {
    let path = fixture.artifact(name).unwrap();
    Process(fixture.tool("grim").unwrap().args(["-g", NOTE_REGION]).arg(&path).spawn().unwrap()).finish();
    let image = image::open(path).unwrap().into_rgba8();
    assert_eq!(image.dimensions(), NOTE_PIXELS, "the native fixture must use the recorded output scale");
    image
}

/// Locate one complete square focus rim, rejecting wide text-field borders,
/// caret changes, clipped outlines and ambiguous candidates instead of guessing.
fn cancel_focus_rect(before: &RgbaImage, focused: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
    if before.dimensions() != focused.dimensions() { return None; }
    let (width, height) = before.dimensions();
    let mut changed: Vec<bool> = before.pixels().zip(focused.pixels()).map(|(a, b)| (0..3).any(|channel| a[channel].abs_diff(b[channel]) >= FOCUS_DELTA)).collect();
    let mut stack = Vec::new();
    let mut found = None;
    for index in 0..changed.len() {
        if !std::mem::replace(&mut changed[index], false) { continue; }
        stack.push(index);
        let (mut left, mut top, mut right, mut bottom, mut count) = (width, height, 0, 0, 0_u32);
        while let Some(at) = stack.pop() {
            let (x, y) = (at as u32 % width, at as u32 / width);
            left = left.min(x); top = top.min(y); right = right.max(x); bottom = bottom.max(y); count += 1;
            for next in [(x > 0).then(|| at - 1), (x + 1 < width).then(|| at + 1), (y > 0).then(|| at - width as usize), (y + 1 < height).then(|| at + width as usize)].into_iter().flatten() {
                if std::mem::replace(&mut changed[next], false) { stack.push(next); }
            }
        }
        let (w, h) = (right - left + 1, bottom - top + 1);
        if left > 0 && top > 0 && right + 1 < width && bottom + 1 < height
            && FOCUS_SIDE.contains(&w) && FOCUS_SIDE.contains(&h)
            && w.max(h) * 4 <= w.min(h) * 5 && count >= w + h && count * 2 < w * h
        {
            if found.is_some() { return None; }
            found = Some((left, top, w, h));
        }
    }
    found
}

/// Only the new Cancel rim is a valid target; other focus changes cannot pick it.
#[test]
fn cancel_locator_requires_one_whole_square_rim() {
    use image::Rgba;
    let before = RgbaImage::from_pixel(240, 180, Rgba([20, 20, 20, 255]));
    let mut focused = before.clone();
    let rim = |image: &mut RgbaImage, x, y, width, height| {
        for row in y..y + height { for column in x..x + width {
            if row < y + 2 || row + 2 >= y + height || column < x + 2 || column + 2 >= x + width {
                image.put_pixel(column, row, Rgba([60, 190, 170, 255]));
            }
        }}
    };
    rim(&mut focused, 145, 24, 54, 54);
    rim(&mut focused, 8, 100, 220, 64);
    rim(&mut focused, 40, 30, 2, 32);
    assert_eq!(cancel_focus_rect(&before, &focused), Some((145, 24, 54, 54)));
    let mut ambiguous = focused.clone();
    rim(&mut ambiguous, 60, 24, 54, 54);
    assert_eq!(cancel_focus_rect(&before, &ambiguous), None);
    let mut clipped = before.clone();
    rim(&mut clipped, 0, 24, 54, 54);
    assert_eq!(cancel_focus_rect(&before, &clipped), None);
    assert_eq!(cancel_focus_rect(&before, &before), None);
}

/// The click target comes from the real Cancel focus outline, in either locale.
fn click_mouse_cancel(fixture: &PrivateDesktop, pointer: &Path, label: &str) {
    let before = note_capture(fixture, &format!("desktop-text-{label}-before-cancel-focus.png"));
    input(fixture, &["-k", "Tab", "-k", "Tab"]);
    let focused = note_capture(fixture, &format!("desktop-text-{label}-cancel-focus.png"));
    let (x, y, width, height) = cancel_focus_rect(&before, &focused).expect("one whole visible Cancel focus rim");
    let x = NOTE_ORIGIN.0 + ((x + width / 2) as f32 / SCALE).round() as u32;
    let y = NOTE_ORIGIN.1 + ((y + height / 2) as f32 / SCALE).round() as u32;
    Process(fixture.tool(pointer.to_str().unwrap()).unwrap().env("MANDARA_LAYOUT", "2560x1440").env("VPOINTER_HOLD_MS", POINTER_HOLD_MS).args(["click", &x.to_string(), &y.to_string()]).spawn().unwrap()).finish();
    std::thread::sleep(SETTLE);
}

/// Real key input updates the shared book, retains completion and Markdown,
/// cancels safely, and preserves conflicting drafts without opening a panel.
#[test]
#[ignore = "requires the guarded offline 4K/1.5 desktop-text fixture, wtype, wl-paste, grim and MANDARA_DESKTOP_TEXT_TEST_POINTER"]
fn desktop_tasks_and_notes_are_edited_without_their_plugin_panels() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture = PrivateDesktop::from_env("MANDARA_DESKTOP_TEXT_TEST", &root, ".mandara-desktop-text-test").expect("verified private desktop");
    let pointer = std::fs::canonicalize(std::env::var_os("MANDARA_DESKTOP_TEXT_TEST_POINTER").expect("prebuilt private virtual-pointer driver")).unwrap();
    assert_eq!(command(&fixture, &["config", "get", "/general/offline"]), true);
    let config = command(&fixture, &["config", "get"]);
    let mut restore = Restore { fixture: &fixture, config: ["/desktop/widgets", "/plugins", "/general/locale", "/motion/reduce_motion", "/theme/mode"].into_iter().map(|path| (path, config.pointer(path).unwrap().clone())).collect(), tasks: Vec::new(), notes: Vec::new() };
    let _keyboard = Process(fixture.tool("wtype").unwrap().args(["-s", KEYBOARD_LIFETIME]).spawn().unwrap());
    command(&fixture, &["plugins", "enable", "todo"]);
    command(&fixture, &["plugins", "enable", "notes"]);
    command(&fixture, &["panel", "close"]);
    command(&fixture, &["config", "set", "/motion/reduce_motion", "true"]);
    command(&fixture, &["config", "set", "/desktop/widgets/hidden", "false"]);
    let widgets = json!([
        {"id":"qa-inline-todo","kind":"todo","place":{"anchor":"top-left","x":0.08,"y":0.15}},
        {"id":"qa-inline-note","kind":"note","note":"Desktop note","place":{"anchor":"top-left","x":0.55,"y":0.15}}
    ]);
    command(&fixture, &["config", "set", "/desktop/widgets/items", &widgets.to_string()]);
    for (language, locale, mode) in [("en", mandara_config::Locale::English, "light"), ("ar", mandara_config::Locale::Arabic, "light"), ("en", mandara_config::Locale::English, "dark"), ("ar", mandara_config::Locale::Arabic, "dark")] {
        command(&fixture, &["config", "set", "/general/locale", &serde_json::to_string(&locale).unwrap()]);
        command(&fixture, &["config", "set", "/theme/mode", mode]);
        let closed_windows = window_count(&fixture);
        let task = command(&fixture, &["todo", "add", "Original task"])["id"].to_string();
        restore.tasks.push(task.clone());
        std::thread::sleep(SETTLE);
        command(&fixture, &["desktop", "widgets", "press", "qa-inline-todo", &format!("todo-edit:{task}")]);
        wait_windows(&fixture, closed_windows + 1);
        input(&fixture, &["-M", "ctrl", "-k", "a", "-m", "ctrl", "Updated task"]);
        command(&fixture, &["todo", "done", &task]);
        input(&fixture, &["-k", "Return"]);
        let task_id = task.parse::<u64>().unwrap();
        let tasks = wait_for(&fixture, &["todo", "list"], |tasks| tasks.as_array().unwrap().iter().any(|item| item["id"].as_u64() == Some(task_id) && item["text"] == "Updated task" && item["done"] == true));
        let edited = tasks.as_array().unwrap().iter().find(|item| item["id"].as_u64() == Some(task_id)).unwrap();
        assert_eq!(edited["text"], "Updated task");
        assert_eq!(edited["done"], true);
        wait_windows(&fixture, closed_windows);
        assert_no_panel(&fixture);

        let before_tasks = command(&fixture, &["todo", "list"]);
        let before_lists = command(&fixture, &["todo", "lists"]);
        command(&fixture, &["desktop", "widgets", "press", "qa-inline-todo", "todo-add"]);
        wait_windows(&fixture, closed_windows + 1);
        input(&fixture, &["#Work", "-k", "Return", "-M", "ctrl", "-k", "a", "-k", "c", "-m", "ctrl"]);
        assert_eq!(clipboard(&fixture), "#Work", "an invalid task must retain its editable draft");
        assert_eq!(window_count(&fixture), closed_windows + 1, "a list-only task must keep its editor open");
        assert_eq!(command(&fixture, &["todo", "list"]), before_tasks, "a list-only draft must not create a task");
        assert_eq!(command(&fixture, &["todo", "lists"]), before_lists, "a list-only draft must not create a list");
        let validation = fixture.artifact(&format!("desktop-text-{language}-{mode}-task-validation.png")).unwrap();
        Process(fixture.tool("grim").unwrap().arg(validation).spawn().unwrap()).finish();
        input(&fixture, &["-k", "Escape"]);
        wait_windows(&fixture, closed_windows);
        command(&fixture, &["desktop", "widgets", "press", "qa-inline-todo", "todo-add"]);
        wait_windows(&fixture, closed_windows + 1);
        let added_text = format!("Added on the desktop {language}-{mode}");
        input(&fixture, &[&added_text, "-k", "Return"]);
        let tasks = wait_for(&fixture, &["todo", "list"], |tasks| tasks.as_array().unwrap().iter().any(|item| item["text"] == added_text));
        let added = tasks.as_array().unwrap().iter().find(|item| item["text"] == added_text).unwrap()["id"].to_string();
        restore.tasks.push(added);
        wait_windows(&fixture, closed_windows);
        assert_no_panel(&fixture);

        let title = format!("Desktop note {language}-{mode}");
        let edited_title = format!("Edited desktop note {language}-{mode}");
        let original = format!("# {title}\nOriginal body");
        let note = command(&fixture, &["notes", "add", &original])["id"].to_string();
        restore.notes.push(note.clone());
        command(&fixture, &["config", "set", "/desktop/widgets/items/1/note", &title]);
        std::thread::sleep(SETTLE);
        command(&fixture, &["desktop", "widgets", "press", "qa-inline-note", &format!("note:{title}")]);
        wait_windows(&fixture, closed_windows + 1);
        let markdown = format!("# {edited_title}\nFirst line\nSecond line");
        input(&fixture, &["-M", "ctrl", "-k", "a", "-m", "ctrl", &markdown, "-M", "ctrl", "-k", "Return", "-m", "ctrl"]);
        wait_for(&fixture, &["notes", "show", &note], |note| note["text"] == markdown);
        wait_for(&fixture, &["config", "get", "/desktop/widgets/items/1/note"], |title| title.as_str() == Some(edited_title.as_str()));
        wait_windows(&fixture, closed_windows);
        assert_no_panel(&fixture);

        command(&fixture, &["desktop", "widgets", "press", "qa-inline-note", &format!("note:{edited_title}")]);
        wait_windows(&fixture, closed_windows + 1);
        input(&fixture, &["-M", "ctrl", "-k", "a", "-m", "ctrl", "Cancelled draft", "-k", "Escape"]);
        wait_windows(&fixture, closed_windows);
        assert_eq!(command(&fixture, &["notes", "show", &note])["text"], markdown);

        command(&fixture, &["desktop", "widgets", "press", "qa-inline-note", &format!("note:{edited_title}")]);
        wait_windows(&fixture, closed_windows + 1);
        input(&fixture, &["-M", "ctrl", "-k", "a", "-m", "ctrl", "Discard this mouse-cancelled draft"]);
        click_mouse_cancel(&fixture, &pointer, &format!("{language}-{mode}"));
        wait_windows(&fixture, closed_windows);
        assert_eq!(command(&fixture, &["notes", "show", &note])["text"], markdown, "Cancel mousedown must not save the dirty field");
        let _ = note_capture(&fixture, &format!("desktop-text-{language}-{mode}-mouse-cancelled.png"));
        assert_no_panel(&fixture);

        command(&fixture, &["desktop", "widgets", "press", "qa-inline-note", &format!("note:{edited_title}")]);
        wait_windows(&fixture, closed_windows + 1);
        input(&fixture, &["-M", "ctrl", "-k", "a", "-m", "ctrl", "Keep this unsaved draft"]);
        command(&fixture, &["notes", "append", &note, "External edit"]);
        input(&fixture, &["-M", "ctrl", "-k", "Return", "-m", "ctrl", "-M", "ctrl", "-k", "a", "-k", "c", "-m", "ctrl"]);
        assert!(command(&fixture, &["notes", "show", &note])["text"].as_str().unwrap().ends_with("External edit"));
        assert_eq!(clipboard(&fixture), "Keep this unsaved draft");
        let shot = fixture.artifact(&format!("desktop-text-{language}-{mode}.png")).unwrap();
        Process(fixture.tool("grim").unwrap().arg(&shot).spawn().unwrap()).finish();
        input(&fixture, &["-k", "Escape"]);
        wait_windows(&fixture, closed_windows);
        assert_no_panel(&fixture);
    }
}
