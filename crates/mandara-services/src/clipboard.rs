//! Clipboard history, natively over Wayland: a `zwlr_data_control_v1`
//! watcher reads every new selection in-process, entries (text and images)
//! persist under `~/.cache/mandara/clipboard/`, restores are served with
//! `wl-clipboard-rs`, and auto-paste types Ctrl+V through a
//! `zwp_virtual_keyboard_v1`. Entries flagged as secrets by password
//! managers are never recorded.
//!
//! What is kept follows [`ClipboardPolicy`] (Studio → Plugins → Clipboard):
//! size limits, pinned entries that survive clears, apps whose copies are
//! never recorded, age-based clearing, and re-offering the last copy when
//! the app it came from quits.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::mpsc;
use std::time::Duration;

use mandara_config::workflow::ClipboardConfig;

use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{zwp_virtual_keyboard_manager_v1, zwp_virtual_keyboard_v1};
use wayland_protocols_wlr::data_control::v1::client::{zwlr_data_control_device_v1, zwlr_data_control_manager_v1, zwlr_data_control_offer_v1};

use crate::actor::{Envelope, ServiceHandle, reply};

/// Bytes per KiB / MiB (the policy's size limits are given in those).
const KIB: usize = 1024;
const MIB: usize = 1024 * 1024;
/// How often entries older than the auto-clear age are dropped.
const PRUNE_EVERY: Duration = Duration::from_secs(60);
/// Seconds per hour.
const SECS_PER_HOUR: i64 = 3600;
/// Hard ceiling of any selection read, whatever the policy says.
const READ_CEILING: usize = 256 * MIB;
/// A selection source that hasn't finished sending by then is given up on.
const READ_TIMEOUT: Duration = Duration::from_secs(3);
/// Chunk size for reading a selection out of its pipe.
const READ_CHUNK: usize = 64 * 1024;
/// Time for the launcher / panel to release keyboard focus before Ctrl+V.
const PASTE_DELAY: Duration = Duration::from_millis(180);
/// States buffered for the UI, commands from the UI / CLI, and selections
/// queued by the watcher.
const UPDATE_QUEUE: usize = 8;
const COMMAND_QUEUE: usize = 32;
const EVENT_QUEUE: usize = 16;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ClipContent {
    Text { text: String },
    Image { path: PathBuf, mime: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipEntry {
    pub id: u64,
    pub content: ClipContent,
    /// Unix seconds.
    pub copied_at: i64,
    /// Pinned entries survive clears and never fall off the end.
    #[serde(default)]
    pub pinned: bool,
    /// App focused when it was copied (best effort; empty if unknown).
    #[serde(default)]
    pub app_id: String,
}

/// What the history keeps (from `[clipboard]`).
#[derive(Clone, Debug, PartialEq)]
pub struct ClipboardPolicy {
    pub enabled: bool,
    pub max_entries: usize,
    pub max_pinned: usize,
    pub max_text: usize,
    pub images: bool,
    pub max_image: usize,
    pub persist: bool,
    pub keep_from_closed_apps: bool,
    pub exclude_password_managers: bool,
    /// Lowercase app ids whose copies are skipped.
    pub excluded_apps: Vec<String>,
    /// Unpinned entries older than this are dropped.
    pub max_age: Option<Duration>,
}

impl Default for ClipboardPolicy {
    fn default() -> Self {
        Self::from(&ClipboardConfig::default())
    }
}

impl From<&ClipboardConfig> for ClipboardPolicy {
    fn from(c: &ClipboardConfig) -> Self {
        Self {
            enabled: c.enabled,
            max_entries: c.max_entries.max(1),
            max_pinned: c.max_pinned,
            max_text: c.max_text_kb as usize * KIB,
            images: c.images,
            max_image: c.max_image_mb as usize * MIB,
            persist: c.persist,
            keep_from_closed_apps: c.keep_from_closed_apps,
            exclude_password_managers: c.exclude_password_managers,
            excluded_apps: c.exclude_apps.iter().map(|a| a.trim().to_lowercase()).filter(|a| !a.is_empty()).collect(),
            max_age: (c.auto_clear_hours > 0).then(|| Duration::from_secs(u64::from(c.auto_clear_hours) * SECS_PER_HOUR as u64)),
        }
    }
}

impl ClipboardPolicy {
    /// Whether copies made while `app_id` has focus are skipped.
    pub fn excludes(&self, app_id: &str) -> bool {
        let app = app_id.trim().to_lowercase();
        !app.is_empty() && self.excluded_apps.iter().any(|x| app == *x || app.contains(x.as_str()))
    }
}

/// Policy plus the focused app, shared with the selection watcher thread.
#[derive(Default)]
struct Shared {
    policy: ClipboardPolicy,
    focused_app: String,
}

impl ClipEntry {
    pub fn preview(&self, max: usize) -> String {
        match &self.content {
            ClipContent::Text { text } => {
                let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if flat.chars().count() > max { format!("{}…", flat.chars().take(max).collect::<String>()) } else { flat }
            }
            ClipContent::Image { mime, .. } => tf("Image ({mime})", &[("mime", mime)]),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ClipboardState {
    pub available: bool,
    /// Newest first.
    pub entries: Vec<ClipEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClipboardCommand {
    /// Put an entry back on the clipboard; optionally paste it (Ctrl+V).
    Restore { id: u64, paste: bool },
    Delete(u64),
    /// Forget the history; pinned entries stay unless `pinned`.
    Clear { pinned: bool },
    Pin { id: u64, pinned: bool },
    /// New settings (applied to the history at once).
    Configure(ClipboardConfig),
    /// The focused window's app id (for per-app exclusion).
    FocusedApp(String),
}

pub type ClipboardService = ServiceHandle<ClipboardState, ClipboardCommand>;

fn store_dir() -> PathBuf {
    mandara_config::paths::cache_dir().join("clipboard")
}

fn hash(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// Whether the offer is flagged as a secret by a password manager.
pub fn is_secret(types: &[&str]) -> bool {
    types.iter().any(|t| t.contains("passwordManagerHint") || *t == "x-kde-passwordManagerHint")
}

/// Decides what (if anything) to record from the offered MIME types.
pub fn classify(types: &[&str]) -> Option<&'static str> {
    if is_secret(types) {
        return None;
    }
    if types.iter().any(|t| *t == "image/png") {
        Some("image/png")
    } else if types.iter().any(|t| t.starts_with("image/")) {
        Some("image")
    } else if types.iter().any(|t| t.starts_with("text/") || *t == "UTF8_STRING" || *t == "STRING") {
        Some("text")
    } else {
        None
    }
}

fn load() -> Vec<ClipEntry> {
    std::fs::read_to_string(store_dir().join("history.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save(entries: &[ClipEntry]) {
    let dir = store_dir();
    if std::fs::create_dir_all(&dir).is_ok()
        && let Ok(json) = serde_json::to_vec(entries)
    {
        let tmp = dir.join("history.json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(tmp, dir.join("history.json"));
        }
    }
}

/// Picks the MIME type to read for a classification.
fn pick_mime<'a>(kind: &str, types: &[&'a str]) -> Option<&'a str> {
    match kind {
        "text" => ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "STRING", "TEXT"]
            .iter()
            .find_map(|want| types.iter().find(|t| *t == want).copied())
            .or_else(|| types.iter().find(|t| t.starts_with("text/")).copied()),
        "image/png" => types.iter().find(|t| **t == "image/png").copied(),
        _ => types.iter().find(|t| t.starts_with("image/")).copied(),
    }
}

/// Builds an entry from the offered types under `policy`, reading the
/// chosen type with `read` (which returns the raw bytes).
fn capture(types: &[&str], policy: &ClipboardPolicy, app_id: &str, read: impl FnOnce(&str) -> Option<Vec<u8>>) -> Option<ClipEntry> {
    if !policy.enabled || policy.excludes(app_id) {
        return None;
    }
    let kind = if policy.exclude_password_managers {
        classify(types)?
    } else {
        // Without the password-manager filter, classify as if the hint were absent.
        let open: Vec<&str> = types.iter().copied().filter(|t| !is_secret(&[t])).collect();
        classify(&open)?
    };
    if kind != "text" && !policy.images {
        return None;
    }
    let mime = pick_mime(kind, types)?;
    let bytes = read(mime)?;
    let content = if kind == "text" {
        if bytes.is_empty() || bytes.len() > policy.max_text {
            return None;
        }
        let text = String::from_utf8(bytes).ok()?;
        if text.trim().is_empty() {
            return None;
        }
        ClipContent::Text { text }
    } else {
        if bytes.is_empty() || bytes.len() > policy.max_image {
            return None;
        }
        let ext = mime.trim_start_matches("image/").split(';').next().unwrap_or("img");
        let dir = store_dir().join("images");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(format!("{:016x}.{ext}", hash(&bytes)));
        if !path.exists() {
            std::fs::write(&path, &bytes).ok()?;
        }
        ClipContent::Image { path, mime: mime.to_string() }
    };
    let key = match &content {
        ClipContent::Text { text } => hash(text.as_bytes()),
        ClipContent::Image { path, .. } => hash(path.to_string_lossy().as_bytes()),
    };
    Some(ClipEntry { id: key, content, copied_at: chrono::Local::now().timestamp(), pinned: false, app_id: app_id.to_string() })
}

/// Inserts newest-first, de-duplicating by id (a re-copied pinned entry
/// stays pinned) and enforcing the policy's limits.
pub fn push(entries: &mut Vec<ClipEntry>, mut entry: ClipEntry, policy: &ClipboardPolicy) {
    if let Some(old) = entries.iter().find(|e| e.id == entry.id) {
        entry.pinned |= old.pinned;
    }
    entries.retain(|e| e.id != entry.id);
    entries.insert(0, entry);
    enforce_limits(entries, policy);
}

/// Caps unpinned entries at `max_entries` and pinned ones at `max_pinned`
/// (the oldest go first) and drops unpinned entries past the max age.
pub fn enforce_limits(entries: &mut Vec<ClipEntry>, policy: &ClipboardPolicy) {
    let now = chrono::Local::now().timestamp();
    let max_age = policy.max_age.map(|d| d.as_secs() as i64);
    let (mut pinned, mut loose) = (0usize, 0usize);
    entries.retain(|e| {
        if e.pinned {
            pinned += 1;
            pinned <= policy.max_pinned
        } else {
            loose += 1;
            loose <= policy.max_entries && max_age.is_none_or(|age| now - e.copied_at <= age)
        }
    });
}

/// Removes image files no entry refers to any more.
fn sweep_images(entries: &[ClipEntry]) {
    let Ok(dir) = std::fs::read_dir(store_dir().join("images")) else { return };
    let keep: std::collections::HashSet<PathBuf> = entries
        .iter()
        .filter_map(|e| match &e.content {
            ClipContent::Image { path, .. } => Some(path.clone()),
            ClipContent::Text { .. } => None,
        })
        .collect();
    for f in dir.flatten() {
        if !keep.contains(&f.path()) {
            let _ = std::fs::remove_file(f.path());
        }
    }
}

/// Takes ownership of the clipboard with `bytes`, serving it from a
/// background thread until another client replaces the selection.
pub fn serve(bytes: Vec<u8>, mime: Option<String>) -> Result<(), String> {
    use wl_clipboard_rs::copy::{MimeType, Options, Source};
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("mandara-clipboard-serve".into())
        .spawn(move || {
            let mut opts = Options::new();
            opts.foreground(true);
            let mime = match mime {
                Some(m) => MimeType::Specific(m),
                None => MimeType::Text,
            };
            // Preparing takes the selection; serving blocks until another
            // app replaces it. Report success / failure right after taking it.
            match opts.prepare_copy(Source::Bytes(bytes.into_boxed_slice()), mime) {
                Ok(prepared) => {
                    let _ = tx.send(Ok(()));
                    if let Err(err) = prepared.serve() {
                        tracing::warn!("clipboard serve failed: {err}");
                    }
                }
                Err(err) => {
                    let _ = tx.send(Err(tf("Could not set the clipboard: {error}", &[("error", &err)])));
                }
            }
        })
        .map_err(|e| e.to_string())?;
    rx.recv().map_err(|_| t("The clipboard thread failed").to_string())?
}

fn restore(entry: &ClipEntry, paste: bool) -> Result<String, String> {
    match &entry.content {
        ClipContent::Text { text } => serve(text.clone().into_bytes(), None)?,
        ClipContent::Image { path, mime } => serve(std::fs::read(path).map_err(|e| e.to_string())?, Some(mime.clone()))?,
    }
    if paste {
        // Give the launcher/panel time to release keyboard focus.
        std::thread::sleep(PASTE_DELAY);
        keyboard::ctrl_v()?;
    }
    Ok(if paste { "pasted".into() } else { "copied".into() })
}

/// Selection watcher over `zwlr_data_control_manager_v1`.
mod watch {
    use super::*;

    type Offer = zwlr_data_control_offer_v1::ZwlrDataControlOfferV1;
    type Device = zwlr_data_control_device_v1::ZwlrDataControlDeviceV1;
    type Manager = zwlr_data_control_manager_v1::ZwlrDataControlManagerV1;

    pub enum Event {
        Ready(bool),
        Entry(ClipEntry),
        /// The selection went away (its owner quit or cleared it).
        Cleared,
    }

    struct State {
        conn: Connection,
        events: async_channel::Sender<Event>,
        current: Option<Offer>,
        shared: Arc<Mutex<Shared>>,
    }

    impl State {
        /// Reads the selection as `mime`. `poll(2)` bounds the wait, so a
        /// source that never closes its end can't stall the watcher (and no
        /// helper thread is needed).
        fn read(&self, offer: &Offer, mime: &str) -> Option<Vec<u8>> {
            use rustix::event::{PollFd, PollFlags, Timespec, poll};
            let (mut rd, wr) = std::io::pipe().ok()?;
            offer.receive(mime.to_string(), wr.as_fd());
            self.conn.flush().ok()?;
            drop(wr);
            let deadline = std::time::Instant::now() + READ_TIMEOUT;
            let mut buf = Vec::new();
            let mut chunk = vec![0u8; READ_CHUNK];
            loop {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                if left.is_zero() {
                    return None;
                }
                let timeout = Timespec { tv_sec: left.as_secs() as i64, tv_nsec: i64::from(left.subsec_nanos()) };
                let mut fds = [PollFd::new(&rd, PollFlags::IN)];
                match poll(&mut fds, Some(&timeout)) {
                    Ok(0) => return None,
                    Ok(_) => {}
                    Err(rustix::io::Errno::INTR) => continue,
                    Err(_) => return None,
                }
                match rd.read(&mut chunk) {
                    Ok(0) => return Some(buf),
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if buf.len() > READ_CEILING {
                            return Some(buf);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => return None,
                }
            }
        }
    }

    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
        fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
    }
    impl Dispatch<wl_seat::WlSeat, ()> for State {
        fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }
    impl Dispatch<Manager, ()> for State {
        fn event(_: &mut Self, _: &Manager, _: zwlr_data_control_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }
    impl Dispatch<Offer, Mutex<Vec<String>>> for State {
        fn event(_: &mut Self, _: &Offer, event: zwlr_data_control_offer_v1::Event, types: &Mutex<Vec<String>>, _: &Connection, _: &QueueHandle<Self>) {
            if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event
                && let Ok(mut t) = types.lock()
            {
                t.push(mime_type);
            }
        }
    }
    impl Dispatch<Device, ()> for State {
        fn event(state: &mut Self, _: &Device, event: zwlr_data_control_device_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
            use zwlr_data_control_device_v1::Event as E;
            match event {
                E::Selection { id } => {
                    if let Some(old) = state.current.take() {
                        old.destroy();
                    }
                    let Some(offer) = id else {
                        let _ = state.events.send_blocking(Event::Cleared);
                        return;
                    };
                    let types: Vec<String> = offer.data::<Mutex<Vec<String>>>().and_then(|t| t.lock().ok().map(|t| t.clone())).unwrap_or_default();
                    let refs: Vec<&str> = types.iter().map(String::as_str).collect();
                    let (policy, app) = state.shared.lock().map(|s| (s.policy.clone(), s.focused_app.clone())).unwrap_or_default();
                    if let Some(entry) = capture(&refs, &policy, &app, |mime| state.read(&offer, mime)) {
                        let _ = state.events.send_blocking(Event::Entry(entry));
                    }
                    state.current = Some(offer);
                }
                E::PrimarySelection { id: Some(offer) } => offer.destroy(),
                E::Finished => {
                    let _ = state.events.send_blocking(Event::Ready(false));
                }
                _ => {}
            }
        }
        event_created_child!(State, Device, [
            zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (Offer, Mutex::new(Vec::new())),
        ]);
    }

    /// Runs the watcher until the connection drops; reports availability.
    pub fn run(events: async_channel::Sender<Event>, shared: Arc<Mutex<Shared>>) -> Result<(), String> {
        let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=8, ()).map_err(|e| format!("wl_seat: {e}"))?;
        let manager: Manager = match globals.bind(&qh, 1..=2, ()) {
            Ok(m) => m,
            Err(err) => {
                let _ = events.send_blocking(Event::Ready(false));
                return Err(format!("zwlr_data_control_manager_v1: {err}"));
            }
        };
        let _device = manager.get_data_device(&seat, &qh, ());
        let _ = events.send_blocking(Event::Ready(true));
        let mut state = State { conn: conn.clone(), events, current: None, shared };
        loop {
            queue.blocking_dispatch(&mut state).map_err(|e| e.to_string())?;
            // The history service ended (the clipboard plugin was turned
            // off): stop watching at this wake-up.
            if state.events.is_closed() {
                return Ok(());
            }
        }
    }
}

/// Synthetic key presses over `zwp_virtual_keyboard_v1`.
pub mod keyboard {
    use super::*;

    type VkManager = zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
    type Vk = zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;

    /// evdev key codes.
    const KEY_LEFTCTRL: u32 = 29;
    const KEY_V: u32 = 47;
    /// `Control` modifier mask in the standard `complete` xkb types.
    const MOD_CTRL: u32 = 1 << 2;
    const KEYMAP: &str = "xkb_keymap {\n  xkb_keycodes { include \"evdev+aliases(qwerty)\" };\n  xkb_types { include \"complete\" };\n  xkb_compat { include \"complete\" };\n  xkb_symbols { include \"pc+us+inet(evdev)\" };\n};\n";

    struct State;
    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
        fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
    }
    impl Dispatch<wl_seat::WlSeat, ()> for State {
        fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }
    impl Dispatch<VkManager, ()> for State {
        fn event(_: &mut Self, _: &VkManager, _: zwp_virtual_keyboard_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }
    impl Dispatch<Vk, ()> for State {
        fn event(_: &mut Self, _: &Vk, _: zwp_virtual_keyboard_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }

    fn keymap_fd() -> Result<(std::os::fd::OwnedFd, u32), String> {
        let fd = rustix::fs::memfd_create("mandara-keymap", rustix::fs::MemfdFlags::CLOEXEC).map_err(|e| e.to_string())?;
        let mut file = std::fs::File::from(fd);
        let mut bytes = KEYMAP.as_bytes().to_vec();
        bytes.push(0);
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        Ok((file.into(), bytes.len() as u32))
    }

    /// Types Ctrl+V into the focused window.
    pub fn ctrl_v() -> Result<(), String> {
        chord(&["ctrl"], KEY_V)
    }

    /// evdev code for a modifier name.
    fn modifier(name: &str) -> Option<(u32, u32)> {
        // (evdev key, xkb modifier mask in the standard keymap)
        Some(match name {
            "shift" => (42, 1 << 0),
            "ctrl" => (KEY_LEFTCTRL, MOD_CTRL),
            "alt" => (56, 1 << 3),
            "super" => (125, 1 << 6),
            _ => return None,
        })
    }

    /// evdev code for a plain key name (letters, digits, a few named keys).
    pub fn key_code(name: &str) -> Option<u32> {
        const ROW1: &str = "qwertyuiop";
        const ROW2: &str = "asdfghjkl";
        const ROW3: &str = "zxcvbnm";
        let n = name.to_ascii_lowercase();
        if n.len() == 1 {
            let c = n.chars().next()?;
            if let Some(i) = ROW1.find(c) {
                return Some(16 + i as u32);
            }
            if let Some(i) = ROW2.find(c) {
                return Some(30 + i as u32);
            }
            if let Some(i) = ROW3.find(c) {
                return Some(44 + i as u32);
            }
            if let Some(d) = c.to_digit(10) {
                return Some(if d == 0 { 11 } else { 1 + d });
            }
        }
        Some(match n.as_str() {
            "space" => 57,
            "enter" | "return" => 28,
            "escape" | "esc" => 1,
            "tab" => 15,
            "backspace" => 14,
            "delete" => 111,
            "print" => 99,
            _ => return None,
        })
    }

    /// Presses `mods` + `key` (evdev code) on a virtual keyboard.
    pub fn chord(mods: &[&str], key: u32) -> Result<(), String> {
        chord_held(mods, key, std::time::Duration::ZERO)
    }

    /// Like [`chord`], but keeps `mods` held for `hold` after tapping `key`
    /// (a hand holding Alt after Alt+Tab).
    pub fn chord_held(mods: &[&str], key: u32, hold: std::time::Duration) -> Result<(), String> {
        let mods: Vec<(u32, u32)> = mods.iter().map(|m| modifier(m).ok_or(format!("unknown modifier {m:?}"))).collect::<Result<_, _>>()?;
        let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=8, ()).map_err(|e| format!("wl_seat: {e}"))?;
        let manager: VkManager = globals.bind(&qh, 1..=1, ()).map_err(|e| format!("zwp_virtual_keyboard_manager_v1: {e}"))?;
        let kb = manager.create_virtual_keyboard(&seat, &qh, ());
        let (fd, size) = keymap_fd()?;
        kb.keymap(1 /* XKB_V1 */, fd.as_fd(), size);
        let mut state = State;
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
        let mut t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u32).unwrap_or(0);
        let mut mask = 0;
        for (code, m) in &mods {
            kb.key(t, *code, 1);
            mask |= m;
            kb.modifiers(mask, 0, 0, 0);
            t += 1;
        }
        kb.key(t, key, 1);
        kb.key(t + 1, key, 0);
        t += 2;
        if !hold.is_zero() {
            queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
            std::thread::sleep(hold);
            t += hold.as_millis() as u32;
        }
        for (code, _) in mods.iter().rev() {
            kb.key(t, *code, 0);
            t += 1;
        }
        kb.modifiers(0, 0, 0, 0);
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
        kb.destroy();
        let _ = conn.flush();
        Ok(())
    }
}

pub fn spawn_clipboard_service() -> ClipboardService {
    spawn_clipboard_service_with(&ClipboardConfig::default())
}

/// Starts the clipboard history under `config` (later changes arrive as
/// [`ClipboardCommand::Configure`]).
pub fn spawn_clipboard_service_with(config: &ClipboardConfig) -> ClipboardService {
    let (tx, updates) = async_channel::bounded(UPDATE_QUEUE);
    let (commands, rx) = async_channel::bounded::<Envelope<ClipboardCommand>>(COMMAND_QUEUE);
    let (events_tx, events) = async_channel::bounded::<watch::Event>(EVENT_QUEUE);
    let start_policy = ClipboardPolicy::from(config);
    let clear_on_start = config.clear_on_start;
    let shared = Arc::new(Mutex::new(Shared { policy: start_policy.clone(), focused_app: String::new() }));
    // A dedicated thread: the Wayland event queue blocks in dispatch.
    let watch_shared = shared.clone();
    let _ = std::thread::Builder::new().name("mandara-clipboard-watch".into()).spawn(move || {
        if let Err(err) = watch::run(events_tx, watch_shared) {
            tracing::warn!("clipboard watcher: {err}");
        }
    });
    crate::runtime::spawn("clipboard", async move {
        use futures::FutureExt as _;
        let mut policy = start_policy;
        let mut available = false;
        let persist = policy.persist;
        let mut entries = crate::runtime::unblock(move || if persist { load() } else { Vec::new() }).await;
        if clear_on_start {
            entries.retain(|e| e.pinned);
        }
        enforce_limits(&mut entries, &policy);
        {
            let (snapshot, persist) = (entries.clone(), policy.persist);
            crate::runtime::unblock(move || {
                if persist { save(&snapshot) } else { forget_disk() }
                sweep_images(&snapshot);
            })
            .await;
        }
        let _ = tx.send(ClipboardState { available, entries: entries.clone() }).await;
        let mut events = events;
        // Keeps a never-firing replacement channel open once the watcher ends.
        let mut _parked: Option<async_channel::Sender<watch::Event>> = None;
        let mut prune = std::pin::pin!(crate::runtime::sleep(PRUNE_EVERY).fuse());
        loop {
            let mut changed = false;
            futures::select! {
                cmd = rx.recv().fuse() => {
                    let Ok(Envelope { command, reply: r }) = cmd else { return };
                    let result = match command {
                        ClipboardCommand::Restore { id, paste } => match entries.iter().find(|e| e.id == id).cloned() {
                            Some(entry) => crate::runtime::unblock(move || restore(&entry, paste)).await,
                            None => Err(tf("No clipboard entry {id}", &[("id", &id)])),
                        },
                        ClipboardCommand::Delete(id) => {
                            let before = entries.len();
                            entries.retain(|e| e.id != id);
                            changed = before != entries.len();
                            if changed { Ok("deleted".into()) } else { Err(tf("No clipboard entry {id}", &[("id", &id)])) }
                        }
                        ClipboardCommand::Clear { pinned } => {
                            entries.retain(|e| e.pinned && !pinned);
                            changed = true;
                            Ok("cleared".into())
                        }
                        ClipboardCommand::Pin { id, pinned } => match entries.iter_mut().find(|e| e.id == id) {
                            Some(e) => {
                                e.pinned = pinned;
                                enforce_limits(&mut entries, &policy);
                                changed = true;
                                Ok(if pinned { "pinned".into() } else { "unpinned".into() })
                            }
                            None => Err(tf("No clipboard entry {id}", &[("id", &id)])),
                        },
                        ClipboardCommand::Configure(config) => {
                            let next = ClipboardPolicy::from(&config);
                            changed = next != policy;
                            if let Ok(mut s) = shared.lock() {
                                s.policy = next.clone();
                            }
                            policy = next;
                            enforce_limits(&mut entries, &policy);
                            Ok("configured".into())
                        }
                        ClipboardCommand::FocusedApp(app) => {
                            if let Ok(mut s) = shared.lock() {
                                s.focused_app = app;
                            }
                            Ok("ok".into())
                        }
                    };
                    reply(&r, result);
                }
                ev = events.recv().fuse() => match ev {
                    Ok(watch::Event::Entry(entry)) => {
                        push(&mut entries, entry, &policy);
                        changed = true;
                    }
                    Ok(watch::Event::Ready(ok)) => {
                        changed = available != ok;
                        available = ok;
                    }
                    Ok(watch::Event::Cleared) => {
                        // The app owning the selection quit: offer its last
                        // copy ourselves so it can still be pasted.
                        if policy.keep_from_closed_apps && let Some(last) = entries.first().cloned() {
                            if let Err(err) = crate::runtime::unblock(move || restore(&last, false)).await {
                                tracing::debug!("clipboard: keeping the last copy failed: {err}");
                            }
                        }
                    }
                    // The watcher ended: keep serving commands (a closed
                    // channel would resolve immediately, so park a fresh one).
                    Err(_) => {
                        let (parked, never) = async_channel::bounded(1);
                        _parked = Some(parked);
                        events = never;
                    }
                },
                _ = prune.as_mut() => {
                    prune.set(crate::runtime::sleep(PRUNE_EVERY).fuse());
                    let before = entries.len();
                    enforce_limits(&mut entries, &policy);
                    changed = before != entries.len();
                }
            }
            if changed {
                let (snapshot, persist) = (entries.clone(), policy.persist);
                crate::runtime::unblock(move || {
                    if persist { save(&snapshot) } else { forget_disk() }
                    sweep_images(&snapshot);
                })
                .await;
                if tx.send(ClipboardState { available, entries: entries.clone() }).await.is_err() {
                    return;
                }
            }
        }
    });
    ServiceHandle::new(updates, commands)
}

/// Deletes the saved history file (history kept in memory only).
fn forget_disk() {
    let _ = std::fs::remove_file(store_dir().join("history.json"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> ClipEntry {
        ClipEntry { id: hash(t.as_bytes()), content: ClipContent::Text { text: t.into() }, copied_at: chrono::Local::now().timestamp(), pinned: false, app_id: String::new() }
    }

    #[test]
    fn classifies_and_skips_secrets() {
        assert_eq!(classify(&["text/plain;charset=utf-8", "UTF8_STRING"]), Some("text"));
        assert_eq!(classify(&["image/png", "text/html"]), Some("image/png"));
        assert_eq!(classify(&["text/plain", "x-kde-passwordManagerHint"]), None);
        assert_eq!(classify(&["application/x-thing"]), None);
    }

    #[test]
    fn push_dedupes_newest_first() {
        let mut list = vec![text("a"), text("b")];
        push(&mut list, text("b"), &ClipboardPolicy::default());
        assert_eq!(list[0].preview(10), "b");
        assert_eq!(list.len(), 2);
        let long = text("lorem   ipsum\n dolor sit amet");
        assert_eq!(long.preview(11), "lorem ipsum…");
    }

    #[test]
    fn limits_keep_pins_and_drop_old_entries() {
        let policy = ClipboardPolicy { max_entries: 2, max_pinned: 1, max_age: Some(Duration::from_secs(3600)), ..ClipboardPolicy::default() };
        let mut list = Vec::new();
        for t in ["a", "b", "c"] {
            push(&mut list, text(t), &policy);
        }
        assert_eq!(list.iter().map(|e| e.preview(5)).collect::<Vec<_>>(), ["c", "b"]);
        list[1].pinned = true;
        let mut old = text("ancient");
        old.copied_at -= 2 * 3600;
        list.push(old);
        push(&mut list, text("d"), &policy);
        // "b" is pinned (kept beyond the two loose ones), "ancient" aged out.
        assert_eq!(list.iter().map(|e| e.preview(9)).collect::<Vec<_>>(), ["d", "c", "b"]);
        // Re-copying a pinned entry keeps its pin.
        push(&mut list, text("b"), &policy);
        assert!(list[0].pinned);
    }

    #[test]
    fn policy_from_config() {
        let mut c = ClipboardConfig::default();
        c.exclude_apps = vec!["KeePassXC".into(), " ".into()];
        c.max_text_kb = 1;
        c.auto_clear_hours = 2;
        let p = ClipboardPolicy::from(&c);
        assert!(p.excludes("org.keepassxc.KeePassXC") && !p.excludes("firefox") && !p.excludes(""));
        assert_eq!(p.max_text, 1024);
        assert_eq!(p.max_age, Some(Duration::from_secs(7200)));
        // Disabled or excluded: nothing is captured (and nothing is read).
        let read = |_: &str| -> Option<Vec<u8>> { panic!("must not read") };
        assert!(capture(&["text/plain"], &ClipboardPolicy { enabled: false, ..p.clone() }, "", read).is_none());
        assert!(capture(&["text/plain"], &p, "org.keepassxc.KeePassXC", read).is_none());
        assert!(capture(&["image/png"], &ClipboardPolicy { images: false, ..p.clone() }, "", read).is_none());
        let e = capture(&["text/plain"], &p, "zed", |_| Some(b"hi".to_vec())).unwrap();
        assert_eq!((e.app_id.as_str(), e.preview(5).as_str()), ("zed", "hi"));
        assert!(capture(&["text/plain"], &p, "zed", |_| Some(vec![b'x'; 2048])).is_none(), "over the text limit");
    }
}
