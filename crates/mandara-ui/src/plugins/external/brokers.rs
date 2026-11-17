//! The shell's side of the extension broker: runs the requests third-party
//! plugins queued (`broker.submit`) against Mandara's own services, and
//! hands the results back. Every request is checked again here against the
//! live grants (the plugin's copy may be a render old), payloads are read
//! with strict types and bounds, and nothing a plugin sends becomes a path,
//! a command line or an address it didn't get from the host.
//!
//! * `media.read`: the playing track (`status`, `subscribe`), its cover as
//!   an image handle. One hub observer serves every subscribed plugin and
//!   exists only while one is subscribed.
//! * `media.control`: play / pause, skip, seek and the player's volume.
//! * `wallpaper.read` / `wallpaper.set`: the wallpaper folder as names and
//!   image handles, paged; setting one from a handle the host issued.
//! * `files.folders`: images in folders the person chose in Studio, and the
//!   project folders directly inside them (handles, previews, titles).
//! * `wallpaper.live`: a project played as a live wallpaper by a renderer
//!   the host knows, starts and stops itself ([`live`]).
//! * `notifications.send`, `net:<host>` (HTTPS to that host only, off the
//!   UI thread, two at a time) and `plugins.call:<id>` (run on the next
//!   tick, so two plugins can never call into each other re-entrantly).
//! * `windows.*`, `workspaces.*`, `system.read`, `osd.show` and
//!   `theme.control` through the shell's own documented commands (the same
//!   JSON `mandara msg` returns), never a command line the plugin wrote.
//! * `clipboard.*`, `open.web` (https only), `events` (theme, wallpaper,
//!   windows, workspaces, power: one hub and one config observer, only
//!   while someone listens) and `timers` (tasks dropped with the plugin).
//!
//! Directory listings run on the background executor; network requests on
//! short-lived threads. Results for a plugin that unloaded or cancelled
//! are dropped.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use gpui::App;
use mandara_plugins::extension::{Access, Capability};
use mandara_plugins::wasm::{ImageEntry, ImageOrigin, RenderContext, Request, SurfaceKind};
use serde_json::{Value, json};

use super::{after_call, discovered, instance, loaded, views_of};
use crate::hub::hub;

pub(crate) mod live;
mod files;
mod queue;
pub mod relay;

/// Most items one page of a listing returns.
pub const MAX_PAGE: usize = 60;
/// Most entries read from one folder (a huge folder is cut here).
const MAX_FOLDER_SCAN: usize = 20_000;
/// Network requests running at once, for every plugin together.
const MAX_NET_IN_FLIGHT: usize = 2;
/// Queued work is bounded across plugins as well as inside each plugin's broker.
const MAX_NET_QUEUED: usize = 32;
/// Longest title and body a plugin's notification may have (bytes).
const MAX_NOTIFY_TEXT: usize = 512;

/// How long results wait before they are handed to plugins (a frame at
/// 120 Hz): replies stay prompt and a reply loop can't spin the UI thread.
const DELIVERY_TICK: std::time::Duration = std::time::Duration::from_millis(8);
/// Hub and config changes are looked at for `events` at most this often.
const EVENTS_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(250);

thread_local! {
    /// Results waiting for the next delivery tick, and whether one is set.
    static INBOX: RefCell<(Vec<(String, u64, Result<String, String>)>, bool)> = const { RefCell::new((Vec::new(), false)) };
    /// An `events` look is scheduled.
    static EVENTS_PENDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Open media subscriptions: plugin id → request ids.
    static MEDIA_SUBS: RefCell<HashMap<String, Vec<u64>>> = RefCell::new(HashMap::new());
    /// The hub observer behind them, and the last snapshot sent (so an
    /// unrelated hub change sends nothing).
    static MEDIA_WATCH: RefCell<Option<(gpui::Subscription, Value)>> = const { RefCell::new(None) };
    /// Network requests waiting for a slot, and how many run.
    static NET_QUEUE: RefCell<queue::Queue<NetJob, MAX_NET_IN_FLIGHT, MAX_NET_QUEUED>> = const { RefCell::new(queue::Queue::new()) };
    /// `events` subscriptions: plugin id → (request id, topic).
    static EVENT_SUBS: RefCell<HashMap<String, Vec<(u64, &'static str)>>> = RefCell::new(HashMap::new());
    /// The hub and config observers behind them, and the last value sent
    /// per topic (an unrelated change sends nothing).
    static EVENT_WATCH: RefCell<Option<(gpui::Subscription, gpui::Subscription, HashMap<&'static str, Value>)>> = const { RefCell::new(None) };
    /// Running timers: (plugin, request id) → the task (dropping it stops it).
    static TIMERS: RefCell<HashMap<(String, u64), gpui::Task<()>>> = RefCell::new(HashMap::new());
}

/// Longest text a plugin may copy, and an OSD title.
const MAX_CLIPBOARD_TEXT: usize = 64 * 1024;
const MAX_OSD_TITLE: usize = 128;
/// Window ids a plugin may name (the compositor's own ids).
const MAX_WINDOW_ID: usize = 64;
/// Workspaces or tags a plugin may switch to.
const MAX_WORKSPACE: u64 = 32;

struct NetJob {
    plugin: String,
    id: u64,
    host: String,
    url: String,
    method: String,
    body: Option<String>,
    operation: NetOperation,
}

#[derive(Clone, Copy)]
enum NetOperation { Text, Binary, Image }

enum NetAnswer { Reply(String), Image { status: u16, path: PathBuf } }

fn fetch_net(job: &NetJob) -> Result<NetAnswer, String> {
    use mandara_services::plugin_net as net;
    match job.operation {
        NetOperation::Text => net::fetch(&job.host, &job.url, &job.method, job.body.as_deref()).map(|r| NetAnswer::Reply(json!({"status":r.status,"body":r.body}).to_string())),
        NetOperation::Binary => net::fetch_bytes(&job.host, &job.url, &job.method, job.body.as_deref(), net::MAX_BINARY_BYTES)
            .map(|r| NetAnswer::Reply(json!({"status":r.status,"encoding":"base64","body":net::encode_bytes(&r.body)}).to_string())),
        NetOperation::Image => {
            if !matches!(job.method.as_str(), "" | "GET" | "get") || job.body.as_ref().is_some_and(|b| !b.is_empty()) {
                return Err("image downloads use GET without a body".into());
            }
            let response = net::fetch_bytes(&job.host, &job.url, "GET", None, net::MAX_IMAGE_BYTES)?;
            if !(200..300).contains(&response.status) { return Err(format!("image download returned HTTP {}", response.status)); }
            let path = net::images::store(&response.body)?;
            Ok(NetAnswer::Image { status: response.status, path })
        }
    }
}

/// The context of `replies` events (they aren't tied to one surface).
fn reply_context() -> RenderContext {
    RenderContext { surface: SurfaceKind::Panel, outlet: String::new(), instance: String::new(), width: 0.0, height: 0.0, numerals: String::new(), calendar: String::new(), language: String::new() }
}

/// Queues a result for plugin `plugin`'s request `id`. Results are handed
/// over on the next delivery tick, never from inside the call that asked:
/// a plugin answering every reply with a new request can't recurse into
/// the shell, and a ping-pong between plugins runs at most once a tick.
pub(super) fn deliver(plugin: &str, id: u64, result: Result<Value, String>, cx: &mut App) {
    deliver_raw(plugin, id, result.map(|v| v.to_string()), cx);
}

/// Large binary/file replies are encoded on their worker before reaching the UI.
fn deliver_raw(plugin: &str, id: u64, result: Result<String, String>, cx: &mut App) {
    let schedule = INBOX.with(|i| {
        let mut i = i.borrow_mut();
        i.0.push((plugin.to_string(), id, result));
        !std::mem::replace(&mut i.1, true)
    });
    if schedule {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(DELIVERY_TICK).await;
            cx.update(flush_inbox);
        })
        .detach();
    }
}

/// The delivery tick: queued results go to their plugins, each plugin gets
/// one `replies` event, runs what it queued, and re-renders.
fn flush_inbox(cx: &mut App) {
    let results = INBOX.with(|i| {
        let mut i = i.borrow_mut();
        i.1 = false;
        std::mem::take(&mut i.0)
    });
    let mut plugins: Vec<String> = Vec::new();
    for (plugin, id, result) in results {
        let Some(shared) = loaded(&plugin) else { continue };
        let delivered = shared.borrow_mut().as_mut().is_some_and(|p| p.deliver(id, result));
        if delivered && !plugins.contains(&plugin) {
            plugins.push(plugin);
        }
    }
    for plugin in &plugins {
        let Some(shared) = loaded(plugin) else { continue };
        if let Some(p) = shared.borrow_mut().as_mut() {
            p.flush_replies(&reply_context());
        }
        after_call(plugin, &shared, cx);
        views_of(plugin).notify(cx);
    }
    super::launcher_updates::notify(&plugins, cx);
}

/// The plugin's access as the config says now.
fn live_access(plugin: &str, cx: &App) -> Option<Access> {
    let d = discovered(plugin)?;
    if !cx.global::<mandara_plugins::PluginRegistry>().is_enabled(plugin, &crate::state::config(cx).read(cx).get().plugins) { return None; }
    Some(Access::from_grant(&d.permissions, crate::state::config(cx).read(cx).get().plugins.grants.get(plugin)))
}

/// Runs `requests` from plugin `plugin`.
pub(super) fn dispatch(plugin: &str, requests: Vec<Request>, cx: &mut App) {
    for r in requests {
        let refused = match live_access(plugin, cx) {
            Some(access) => access.refuse(&r.capability, &r.operation),
            None => Some("the plugin isn't registered".into()),
        };
        if let Some(why) = refused {
            deliver(plugin, r.id, Err(why), cx);
            continue;
        }
        let result = run(plugin, &r, cx);
        if let Some(result) = result {
            deliver(plugin, r.id, result, cx);
        }
    }
}

/// Runs one request. `Some` is an immediate answer; `None` means the answer
/// comes later (a subscription, a listing, a network request).
fn run(plugin: &str, r: &Request, cx: &mut App) -> Option<Result<Value, String>> {
    let op = r.operation.as_str();
    if r.capability == Capability::WallpaperLive && crate::hub::try_hub(cx).is_none() {
        return relay::forward(plugin, r, cx);
    }
    // Studio runs plugin tabs without the shell's services.
    let shell_only = matches!(
        r.capability,
        Capability::MediaRead | Capability::MediaControl | Capability::Notify | Capability::Events | Capability::WindowsRead | Capability::WindowsControl | Capability::WorkspacesRead | Capability::WorkspacesControl | Capability::SystemRead | Capability::Osd | Capability::WallpaperLive
    );
    if shell_only && crate::hub::try_hub(cx).is_none() {
        return Some(Err("this works in the shell, not in Studio".into()));
    }
    Some(match (&r.capability, op) {
        (Capability::MediaRead, "status") => Ok(media_snapshot(plugin, cx)),
        (Capability::MediaRead, "subscribe") => {
            subscribe_media(plugin, r.id, cx);
            return None;
        }
        (Capability::MediaControl, _) => media_control(op, &r.payload, cx),
        (Capability::WallpaperRead, "current") => Ok(current_wallpaper(plugin, cx)),
        (Capability::WallpaperRead, "list") => {
            let dir = crate::wallpaper::directory(cx);
            list_later(plugin, r.id, page(&r.payload), ImageOrigin::Wallpaper, move || crate::wallpaper::list(&dir), cx);
            return None;
        }
        (Capability::WallpaperSet, "apply") => apply_wallpaper(plugin, &r.payload, cx),
        (Capability::Folders, "folders") => Ok(folders(plugin, cx)),
        (Capability::Folders, "list") => {
            // A granted folder's own pictures, or a project's inside it.
            let (dir, folder) = match r.payload.get("project").and_then(Value::as_u64) {
                Some(handle) => match project_of(plugin, handle, cx) {
                    Ok(found) => found,
                    Err(e) => return Some(Err(e)),
                },
                None => match granted_folder(plugin, &r.payload, cx) {
                    Some(folder) => (folder.clone(), folder),
                    None => return Some(Err("no such granted folder".into())),
                },
            };
            let root = folder.clone();
            list_later(plugin, r.id, page(&r.payload), ImageOrigin::Folder(folder), move || images_in(&dir, &root), cx);
            return None;
        }
        (Capability::Folders, "projects") => {
            let Some(folder) = granted_folder(plugin, &r.payload, cx) else { return Some(Err("no such granted folder".into())) };
            projects_later(plugin, r.id, page(&r.payload), folder, cx);
            return None;
        }
        (Capability::Folders, "choose") => {
            // The person picks folders in Studio, on this plugin's page.
            crate::state::open_studio_at(Some(format!("plugins/{plugin}")), cx);
            Ok(json!({ "opened": "studio" }))
        }
        (Capability::FilesRead, _) => return files::run(plugin, r, cx),
        (Capability::WallpaperLive, "status") => {
            live::status(plugin, r.id, cx);
            return None;
        }
        (Capability::WallpaperLive, "render") => return live::render(plugin, r.id, &r.payload, cx),
        (Capability::WallpaperLive, "stop") => live::stop(plugin, &r.payload, cx),
        (Capability::Notify, "send") => notify(plugin, &r.payload, cx),
        (Capability::Net(host), "fetch" | "fetch-binary" | "fetch-image") => {
            let operation = match op { "fetch-binary" => NetOperation::Binary, "fetch-image" => NetOperation::Image, _ => NetOperation::Text };
            queue_net(NetJob { plugin: plugin.to_string(), id: r.id, host: host.clone(), url: text(&r.payload, "url"), method: text(&r.payload, "method"), body: r.payload.get("body").and_then(Value::as_str).map(str::to_string), operation }, cx);
            return None;
        }
        (Capability::Call(callee), _) => {
            // Next tick: the callee may call back without re-entering.
            let (caller, callee, op, payload, id) = (plugin.to_string(), callee.clone(), op.to_string(), r.payload.clone(), r.id);
            cx.defer(move |cx| {
                let answer = call_plugin(&caller, &callee, &op, &payload, cx);
                deliver(&caller, id, answer, cx);
            });
            return None;
        }
        (Capability::WindowsRead, "list") => return shell_command(plugin, r.id, &["window", "list"], cx),
        (Capability::WindowsRead, "active") => return shell_command(plugin, r.id, &["window", "active"], cx),
        (Capability::WindowsControl, "focus" | "close") => match window_id(&r.payload) {
            Ok(window) => return shell_command(plugin, r.id, &["window", op, &window], cx),
            Err(e) => Err(e),
        },
        (Capability::WorkspacesRead, "list") => return shell_command(plugin, r.id, &["workspace", "list"], cx),
        (Capability::WorkspacesControl, "view") => match r.payload.get("workspace").and_then(Value::as_u64).filter(|n| (1..=MAX_WORKSPACE).contains(n)) {
            Some(n) => return shell_command(plugin, r.id, &["workspace", "view", &n.to_string()], cx),
            None => Err(format!("`workspace` must be a number from 1 to {MAX_WORKSPACE}")),
        },
        (Capability::SystemRead, "resources") => return shell_command(plugin, r.id, &["system", "resources"], cx),
        (Capability::SystemRead, "power") => return shell_command(plugin, r.id, &["power", "status"], cx),
        (Capability::ClipboardRead, "text") => Ok(json!({ "text": cx.read_from_clipboard().and_then(|c| c.text()).map(|t| clip(&t, MAX_CLIPBOARD_TEXT)) })),
        (Capability::ClipboardWrite, "text") => {
            let text = text(&r.payload, "text");
            if text.len() > MAX_CLIPBOARD_TEXT {
                Err(format!("at most {MAX_CLIPBOARD_TEXT} bytes"))
            } else {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                Ok(json!({ "copied": true }))
            }
        }
        (Capability::OpenWeb, "open") => {
            let url = text(&r.payload, "url");
            if url.starts_with("https://") && mandara_services::web_api::web_url(&url) && !url.chars().any(char::is_whitespace) {
                // The desktop portal call waits; never on the UI thread.
                let (plugin, id) = (plugin.to_string(), r.id);
                let task = cx.background_executor().spawn(async move { mandara_services::session::open_uri(&url).map(|_| json!({ "opened": true })).map_err(|e| e.to_string()) });
                cx.spawn(async move |cx| {
                    let answer = task.await;
                    cx.update(|cx| deliver(&plugin, id, answer, cx));
                })
                .detach();
                return None;
            } else {
                Err("only https:// addresses without a user name or password".into())
            }
        }
        (Capability::Osd, "show") => {
            let title = clip(&text(&r.payload, "title"), MAX_OSD_TITLE);
            if title.trim().is_empty() {
                Err("`title` is needed".into())
            } else {
                let mut words = vec!["osd".to_string(), "show".into(), title];
                if let Some(icon) = r.payload.get("icon").and_then(Value::as_str).and_then(|i| crate::icons::ICON_NAMES.iter().find(|x| **x == i)) {
                    words.extend(["--icon".to_string(), (*icon).to_string()]);
                }
                if let Some(level) = r.payload.get("level").and_then(Value::as_f64).filter(|v| v.is_finite()) {
                    words.extend(["--level".to_string(), format!("{:.0}", level.clamp(0.0, 100.0))]);
                }
                let words: Vec<&str> = words.iter().map(String::as_str).collect();
                return shell_command(plugin, r.id, &words, cx);
            }
        }
        (Capability::ThemeControl, "mode") => match r.payload.get("mode").and_then(Value::as_str) {
            Some(mode @ ("light" | "dark")) => return shell_command(plugin, r.id, &["theme", "mode", mode], cx),
            _ => Err("`mode` must be \"light\" or \"dark\"".into()),
        },
        (Capability::Events, "subscribe") => {
            let topic = mandara_plugins::extension::EVENT_TOPICS.iter().map(|(t, _)| *t).find(|t| r.payload.get("topic").and_then(Value::as_str) == Some(*t));
            match topic {
                Some(topic) => {
                    subscribe_events(plugin, r.id, topic, cx);
                    return None;
                }
                None => Err("unknown events topic".into()),
            }
        }
        (Capability::Timers, "after" | "every") => {
            let ms = r.payload.get("ms").and_then(Value::as_u64).unwrap_or(0);
            let every = op == "every";
            let (min, max) = if every { (mandara_plugins::extension::TIMER_EVERY_MIN_MS, mandara_plugins::extension::TIMER_MAX_MS) } else { (mandara_plugins::extension::TIMER_MIN_MS, mandara_plugins::extension::TIMER_MAX_MS) };
            if !(min..=max).contains(&ms) {
                Err(format!("`ms` must be from {min} to {max}"))
            } else {
                start_timer(plugin, r.id, std::time::Duration::from_millis(ms), every, cx);
                return None;
            }
        }
        _ => Err(format!("`{}` has no operation `{op}`", r.capability)),
    })
}

/// Runs one of the shell's own documented commands for a request and
/// answers with its JSON (now, or when a pending command finishes).
fn shell_command(plugin: &str, id: u64, words: &[&str], cx: &mut App) -> Option<Result<Value, String>> {
    let words: Vec<String> = words.iter().map(|w| (*w).to_string()).collect();
    let registry = cx.global::<mandara_plugins::PluginRegistry>().commands.clone();
    command_output(plugin, id, registry.execute(&words, cx), cx)
}

/// Both local services and Studio relays deliver through the same pending reply path.
pub(super) fn command_output(plugin: &str, id: u64, output: Result<mandara_plugins::Output, String>, cx: &mut App) -> Option<Result<Value, String>> {
    match output {
        Ok(mandara_plugins::Output::Value(v)) => Some(Ok(v)),
        Ok(mandara_plugins::Output::Pending(rx)) => {
            let plugin = plugin.to_string();
            cx.spawn(async move |cx| {
                let answer = rx.recv().await.unwrap_or_else(|_| Err("the command stopped".into()));
                cx.update(|cx| deliver(&plugin, id, answer.map(|s| serde_json::from_str(&s).unwrap_or(Value::String(s))), cx));
            })
            .detach();
            None
        }
        Err(e) => Some(Err(e)),
    }
}

/// A window id from a request: the compositor's own id, as a string or a
/// number, letters, digits, `-`, `_` and `:` only.
fn window_id(payload: &serde_json::Map<String, Value>) -> Result<String, String> {
    let id = match payload.get("id") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => return Err("`id` (a window id) is needed".into()),
    };
    let ok = !id.is_empty() && id.len() <= MAX_WINDOW_ID && id.chars().all(|c| c.is_ascii_alphanumeric() || "-_:".contains(c));
    ok.then_some(id).ok_or_else(|| "that isn't a window id".into())
}

/// `s` cut to at most `max` bytes at a character boundary.
fn clip(s: &str, max: usize) -> String {
    let mut at = s.len().min(max);
    while !s.is_char_boundary(at) {
        at -= 1;
    }
    s[..at].to_string()
}

fn text(payload: &serde_json::Map<String, Value>, key: &str) -> String {
    payload.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// `{offset, limit}` of a listing, bounded.
fn page(payload: &serde_json::Map<String, Value>) -> (usize, usize) {
    let num = |k: &str| payload.get(k).and_then(Value::as_u64).map(|n| n.min(MAX_FOLDER_SCAN as u64) as usize);
    (num("offset").unwrap_or(0), num("limit").unwrap_or(MAX_PAGE).clamp(1, MAX_PAGE))
}

// ── Media ────────────────────────────────────────────────────────────────

/// The playing track as JSON, with a cover handle issued to `plugin`.
fn media_snapshot(plugin: &str, cx: &mut App) -> Value {
    let Some(m) = hub(cx).read(cx).media.clone() else { return json!({ "playing": false, "status": "stopped" }) };
    let art = m.art_path.as_ref().filter(|p| mandara_plugins::wasm::is_image(p)).and_then(|path| issue(plugin, path.clone(), ImageOrigin::MediaArt)).unwrap_or(0);
    let status = match m.status {
        mandara_services::mpris::PlaybackStatus::Playing => "playing",
        mandara_services::mpris::PlaybackStatus::Paused => "paused",
        mandara_services::mpris::PlaybackStatus::Stopped => "stopped",
    };
    json!({
        "playing": m.status == mandara_services::mpris::PlaybackStatus::Playing,
        "status": status,
        "player": m.identity,
        "title": m.title,
        "artist": m.artist,
        "album": m.album,
        "position": m.position_now(Instant::now()).as_secs_f64(),
        "length": m.length.map(|l| l.as_secs_f64()),
        "rate": m.rate,
        "volume": m.volume,
        "can_next": m.can_go_next,
        "can_previous": m.can_go_previous,
        "can_seek": m.can_seek,
        "art": art,
        "at_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
    })
}

/// An image handle in plugin `plugin`'s live instance.
fn issue(plugin: &str, path: PathBuf, origin: ImageOrigin) -> Option<u64> {
    let shared = loaded(plugin)?;
    let mut guard = shared.borrow_mut();
    guard.as_mut()?.issue_image(ImageEntry { path, origin })
}

fn subscribe_media(plugin: &str, id: u64, cx: &mut App) {
    MEDIA_SUBS.with(|s| s.borrow_mut().entry(plugin.to_string()).or_default().push(id));
    let watching = MEDIA_WATCH.with(|w| w.borrow().is_some());
    if !watching {
        let subscription = cx.observe(&hub(cx), |_, cx| media_changed(cx));
        MEDIA_WATCH.with(|w| *w.borrow_mut() = Some((subscription, Value::Null)));
    }
    // The first update is the state now.
    let snapshot = media_snapshot(plugin, cx);
    deliver(plugin, id, Ok(snapshot), cx);
}

/// The hub changed: send the new track to every subscriber when it
/// differs from the last one sent (positions move without notifying).
fn media_changed(cx: &mut App) {
    let key = hub(cx).read(cx).media.as_ref().map(|m| json!([m.status as u8, m.title, m.artist, m.album, m.length.map(|l| l.as_millis() as u64), m.position.as_millis() as u64, m.volume, m.art_path]));
    let key = key.unwrap_or(Value::Null);
    let changed = MEDIA_WATCH.with(|w| match w.borrow_mut().as_mut() {
        Some((_, last)) if *last != key => {
            *last = key;
            true
        }
        _ => false,
    });
    if !changed {
        return;
    }
    let subs: Vec<(String, Vec<u64>)> = MEDIA_SUBS.with(|s| s.borrow().iter().map(|(k, v)| (k.clone(), v.clone())).collect());
    for (plugin, ids) in subs {
        let snapshot = media_snapshot(&plugin, cx);
        for id in ids {
            deliver(&plugin, id, Ok(snapshot.clone()), cx);
        }
    }
}

fn media_control(op: &str, payload: &serde_json::Map<String, Value>, cx: &mut App) -> Result<Value, String> {
    use mandara_services::mpris::MediaCommand;
    let number = |k: &str| payload.get(k).and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| format!("`{k}` must be a number"));
    let command = match op {
        "play-pause" => MediaCommand::PlayPause,
        "next" => MediaCommand::Next,
        "previous" => MediaCommand::Previous,
        // Fallible: a huge number would make `from_secs_f64` panic.
        "seek" => MediaCommand::SeekTo(std::time::Duration::try_from_secs_f64(number("seconds")?.max(0.0)).map_err(|_| "`seconds` is out of range".to_string())?),
        "volume" => {
            if hub(cx).read(cx).media.as_ref().and_then(|m| m.volume).is_none() {
                return Err("this player doesn't let you change its volume".into());
            }
            MediaCommand::SetVolume(number("level")?.clamp(0.0, 1.0))
        }
        _ => return Err(format!("no media operation `{op}`")),
    };
    crate::plugins::media::send(cx, command);
    Ok(json!({ "sent": op }))
}

// ── Wallpapers and folders ───────────────────────────────────────────────

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn current_wallpaper(plugin: &str, cx: &mut App) -> Value {
    let path = crate::state::config(cx).read(cx).get().wallpaper.path.clone();
    match path.filter(|p| p.is_file()) {
        Some(path) => {
            let name = file_name(&path);
            let image = issue(plugin, path, ImageOrigin::Wallpaper).unwrap_or(0);
            json!({ "name": name, "image": image })
        }
        None => json!(null),
    }
}

/// Images directly inside `dir`, which must itself be inside granted
/// folder `root` (no hidden files, sorted by name, at most
/// [`MAX_FOLDER_SCAN`]). Links are resolved now and the resolved paths are
/// returned, so a link, or a folder swapped for one since it was listed,
/// can't lead out of `root`.
fn images_in(dir: &Path, root: &Path) -> Vec<PathBuf> {
    let Ok(root) = root.canonicalize() else { return Vec::new() };
    let Some(dir) = live::contained(dir, &root) else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .take(MAX_FOLDER_SCAN)
        .map(|e| e.path())
        .filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) && mandara_plugins::wasm::is_image(p))
        // A link pointing out of the folder isn't in it.
        .filter_map(|p| live::contained(&p, &dir))
        .filter(|real| real.is_file())
        .collect();
    live::sort_unique(&mut out);
    out
}

/// Lists in the background, then answers one page with handles.
fn list_later(plugin: &str, id: u64, (offset, limit): (usize, usize), origin: ImageOrigin, scan: impl FnOnce() -> Vec<PathBuf> + Send + 'static, cx: &mut App) {
    let task = cx.background_executor().spawn(async move {
        let all = scan();
        let total = all.len();
        (total, all.into_iter().skip(offset).take(limit).collect::<Vec<_>>())
    });
    let plugin = plugin.to_string();
    cx.spawn(async move |cx| {
        let (total, items) = task.await;
        cx.update(|cx| {
            let items: Vec<Value> = items.into_iter().map(|path| json!({ "name": file_name(&path), "image": issue(&plugin, path, origin.clone()).unwrap_or(0) })).collect();
            deliver(&plugin, id, Ok(json!({ "total": total, "offset": offset, "items": items })), cx);
        });
    })
    .detach();
}

/// One project of a `projects` page, read in the background.
struct ProjectRow {
    dir: PathBuf,
    title: String,
    kind: String,
    preview: Option<PathBuf>,
}

/// The project folders in granted folder `folder`, one page of them with
/// their titles, kinds and previews read in the background; the handles
/// are issued on the UI thread.
fn projects_later(plugin: &str, id: u64, (offset, limit): (usize, usize), folder: PathBuf, cx: &mut App) {
    let scan = folder.clone();
    let task = cx.background_executor().spawn(async move {
        let all = live::projects_in(&scan, MAX_FOLDER_SCAN);
        let total = all.len();
        let rows: Vec<ProjectRow> = all
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|dir| {
                let info = live::project_info(&dir);
                let preview = live::project_preview(&dir, &info);
                ProjectRow { dir, title: info.title, kind: info.kind, preview }
            })
            .collect();
        (total, rows)
    });
    let plugin = plugin.to_string();
    cx.spawn(async move |cx| {
        let (total, rows) = task.await;
        cx.update(|cx| {
            let items: Vec<Value> = rows
                .into_iter()
                .map(|r| {
                    let preview = r.preview.and_then(|p| issue(&plugin, p, ImageOrigin::Folder(folder.clone()))).unwrap_or(0);
                    let name = file_name(&r.dir);
                    let project = issue(&plugin, r.dir, ImageOrigin::Project(folder.clone())).unwrap_or(0);
                    json!({ "project": project, "name": name, "title": r.title, "kind": r.kind, "live": live::runnable(&r.kind), "preview": preview })
                })
                .collect();
            deliver(&plugin, id, Ok(json!({ "total": total, "offset": offset, "items": items })), cx);
        });
    })
    .detach();
}

/// The project folder behind `handle` and the granted folder it is in,
/// while that folder is still granted.
fn project_of(plugin: &str, handle: u64, cx: &App) -> Result<(PathBuf, PathBuf), String> {
    let entry = loaded(plugin).and_then(|s| s.borrow().as_ref().and_then(|p| p.image(handle).cloned())).ok_or("that project handle isn't this plugin's (or its grant was revoked)")?;
    let ImageOrigin::Project(folder) = entry.origin else { return Err("that handle isn't a project (use `projects`)".into()) };
    if !crate::state::config(cx).read(cx).get().plugins.granted_folders(plugin).contains(&folder) {
        return Err("that folder is no longer granted".into());
    }
    Ok((entry.path, folder))
}

fn apply_wallpaper(plugin: &str, payload: &serde_json::Map<String, Value>, cx: &mut App) -> Result<Value, String> {
    let handle = payload.get("image").and_then(Value::as_u64).ok_or("`image` must be an image handle")?;
    let entry = loaded(plugin).and_then(|s| s.borrow().as_ref().and_then(|p| p.image(handle).cloned())).ok_or("that image handle isn't this plugin's (or its grant was revoked)")?;
    // Only wallpapers and pictures from folders the person granted: not a
    // plugin's own files or a cover.
    if !matches!(entry.origin, ImageOrigin::Wallpaper | ImageOrigin::Folder(_)) {
        return Err("only wallpapers and pictures from granted folders can be set".into());
    }
    if let ImageOrigin::Folder(folder) = &entry.origin
        && !crate::state::config(cx).read(cx).get().plugins.granted_folders(plugin).contains(folder)
    {
        return Err("that folder is no longer granted".into());
    }
    if !entry.path.is_file() || !crate::wallpaper::is_wallpaper(&entry.path) {
        return Err("that picture can't be used as a wallpaper".into());
    }
    let name = file_name(&entry.path);
    crate::wallpaper::apply(entry.path, cx);
    Ok(json!({ "applied": name }))
}

fn folders(plugin: &str, cx: &App) -> Value {
    let cfg = crate::state::config(cx).read(cx).get();
    Value::Array(cfg.plugins.granted_folders(plugin).iter().enumerate().map(|(i, f)| json!({ "id": i, "name": file_name(f) })).collect())
}

fn granted_folder(plugin: &str, payload: &serde_json::Map<String, Value>, cx: &App) -> Option<PathBuf> {
    let at = payload.get("folder").and_then(Value::as_u64)? as usize;
    crate::state::config(cx).read(cx).get().plugins.granted_folders(plugin).get(at).cloned()
}

// ── Notifications ────────────────────────────────────────────────────────

fn clipped(s: &str) -> String {
    let mut s = s.trim().to_string();
    if s.len() > MAX_NOTIFY_TEXT {
        let mut at = MAX_NOTIFY_TEXT;
        while !s.is_char_boundary(at) {
            at -= 1;
        }
        s.truncate(at);
    }
    s
}

fn notify(plugin: &str, payload: &serde_json::Map<String, Value>, cx: &mut App) -> Result<Value, String> {
    let summary = clipped(&text(payload, "title"));
    if summary.is_empty() {
        return Err("`title` is needed".into());
    }
    let app = discovered(plugin).map(|d| d.name).unwrap_or_else(|| plugin.to_string());
    let service = hub(cx).read(cx).notification_service.clone().ok_or("notifications are unavailable")?;
    service.send(mandara_services::notifications::NotificationCommand::PostUrgent { app, icon: String::new(), summary, body: clipped(&text(payload, "body")), urgency: mandara_services::notifications::Urgency::Normal });
    Ok(json!({ "sent": true }))
}

// ── Network ──────────────────────────────────────────────────────────────

fn queue_net(job: NetJob, cx: &mut App) {
    if let Some(job) = NET_QUEUE.with(|q| q.borrow_mut().push(job).err()) {
        deliver(&job.plugin, job.id, Err("the network request queue is full".into()), cx);
        return;
    }
    pump_net(cx);
}

/// Starts waiting requests up to [`MAX_NET_IN_FLIGHT`], each on its own
/// short-lived thread; the answer comes back through a channel.
fn pump_net(cx: &mut App) {
    loop {
        let job = NET_QUEUE.with(|q| q.borrow_mut().next_job());
        let Some(job) = job else { return };
        // Still allowed and still wanted? A revoked grant or a cancelled
        // request must not reach the network (a POST has effects).
        let allowed = live_access(&job.plugin, cx).is_some_and(|a| a.allows(&Capability::Net(job.host.clone())));
        let open = loaded(&job.plugin).is_some_and(|p| p.borrow().as_ref().is_some_and(|p| p.is_open(job.id)));
        if !allowed || !open {
            NET_QUEUE.with(|q| q.borrow_mut().complete());
            continue;
        }
        let (tx, rx) = async_channel::bounded(1);
        let (plugin, id, host) = (job.plugin.clone(), job.id, job.host.clone());
        let spawned = std::thread::Builder::new().name("mandara-plugin-net".into()).spawn(move || {
            let _ = tx.send_blocking(fetch_net(&job));
        });
        if spawned.is_err() {
            NET_QUEUE.with(|q| q.borrow_mut().complete());
            deliver(&plugin, id, Err("couldn't start the request".into()), cx);
            continue;
        }
        cx.spawn(async move |cx| {
            let answer = rx.recv().await.unwrap_or_else(|_| Err("the request stopped".into()));
            cx.update(|cx| {
                NET_QUEUE.with(|q| q.borrow_mut().complete());
                let allowed = live_access(&plugin, cx).is_some_and(|a| a.allows(&Capability::Net(host.clone())));
                let open = loaded(&plugin).is_some_and(|p| p.borrow().as_ref().is_some_and(|p| p.is_open(id)));
                if allowed && open {
                    match answer {
                        Ok(NetAnswer::Reply(reply)) => deliver_raw(&plugin, id, Ok(reply), cx),
                        Ok(NetAnswer::Image { status, path }) => match issue(&plugin, path, ImageOrigin::Network(host)) {
                            Some(image) => deliver(&plugin, id, Ok(json!({"status":status,"image":image})), cx),
                            None => deliver(&plugin, id, Err("no image handles are left".into()), cx),
                        },
                        Err(error) => deliver(&plugin, id, Err(error), cx),
                    }
                }
                pump_net(cx);
            });
        })
        .detach();
    }
}

// ── Plugin calls ─────────────────────────────────────────────────────────

fn call_plugin(caller: &str, callee: &str, op: &str, payload: &serde_json::Map<String, Value>, cx: &mut App) -> Result<Value, String> {
    if caller == callee {
        return Err("a plugin can't call itself".into());
    }
    let d = discovered(callee).ok_or_else(|| format!("no plugin `{callee}`"))?;
    if !d.serves.iter().any(|s| s == op) {
        return Err(format!("`{callee}` doesn't serve `{op}`"));
    }
    let enabled = cx.global::<mandara_plugins::PluginRegistry>().is_enabled(callee, &crate::state::config(cx).read(cx).get().plugins);
    if !enabled {
        return Err(format!("`{callee}` is switched off"));
    }
    let shared = instance(&d, cx);
    let answer = match shared.borrow_mut().as_mut() {
        Some(p) => p.on_call(caller, op, &Value::Object(payload.clone()).to_string()),
        None => Err(format!("`{callee}` couldn't be loaded")),
    };
    after_call(callee, &shared, cx);
    let body = answer?;
    serde_json::from_str(&body).map_err(|_| format!("`{callee}` answered with invalid JSON"))
}

// ── Events ───────────────────────────────────────────────────────────────

fn subscribe_events(plugin: &str, id: u64, topic: &'static str, cx: &mut App) {
    EVENT_SUBS.with(|s| s.borrow_mut().entry(plugin.to_string()).or_default().push((id, topic)));
    let watching = EVENT_WATCH.with(|w| w.borrow().is_some());
    if !watching {
        let hub = cx.observe(&hub(cx), |_, cx| events_soon(cx));
        let config = cx.observe(&crate::state::config(cx), |_, cx| events_soon(cx));
        EVENT_WATCH.with(|w| *w.borrow_mut() = Some((hub, config, HashMap::new())));
    }
    // The first update is the state now.
    let now = topic_value(topic, cx);
    deliver(plugin, id, Ok(now), cx);
}

/// What `topic` looks like now (what subscribers receive).
fn topic_value(topic: &str, cx: &mut App) -> Value {
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let run = |words: &[&str], cx: &mut App| -> Value {
        let words: Vec<String> = words.iter().map(|w| (*w).to_string()).collect();
        let registry = cx.global::<mandara_plugins::PluginRegistry>().commands.clone();
        match registry.execute(&words, cx) {
            Ok(mandara_plugins::Output::Value(v)) => v,
            _ => Value::Null,
        }
    };
    match topic {
        "theme" => json!({ "dark": cfg.theme.mode.is_dark() }),
        "wallpaper" => json!({ "name": cfg.wallpaper.path.as_deref().map(file_name) }),
        "windows" => run(&["window", "list"], cx),
        "workspaces" => run(&["workspace", "list"], cx),
        "power" => run(&["power", "status"], cx),
        "live-wallpaper" => live::snapshot(cx),
        _ => Value::Null,
    }
}

/// The hub or the config changed: look once, a little later (bursts of
/// changes coalesce into one look).
fn events_soon(cx: &mut App) {
    if EVENTS_PENDING.with(|p| p.replace(true)) {
        return;
    }
    cx.spawn(async move |cx| {
        cx.background_executor().timer(EVENTS_DEBOUNCE).await;
        cx.update(|cx| {
            EVENTS_PENDING.with(|p| p.set(false));
            events_changed(cx);
        });
    })
    .detach();
}

/// Sends each subscribed topic that differs from what was sent last.
fn events_changed(cx: &mut App) {
    let subs: Vec<(String, Vec<(u64, &'static str)>)> = EVENT_SUBS.with(|s| s.borrow().iter().map(|(k, v)| (k.clone(), v.clone())).collect());
    let mut topics: Vec<&'static str> = subs.iter().flat_map(|(_, v)| v.iter().map(|(_, t)| *t)).collect();
    topics.sort_unstable();
    topics.dedup();
    for topic in topics {
        let now = topic_value(topic, cx);
        let changed = EVENT_WATCH.with(|w| match w.borrow_mut().as_mut() {
            Some((_, _, last)) => last.insert(topic, now.clone()).as_ref() != Some(&now),
            None => false,
        });
        if !changed {
            continue;
        }
        for (plugin, ids) in &subs {
            for (id, t) in ids {
                if *t == topic {
                    deliver(plugin, *id, Ok(json!({ "topic": topic, "value": now })), cx);
                }
            }
        }
    }
}

// ── Timers ───────────────────────────────────────────────────────────────

fn start_timer(plugin: &str, id: u64, every: std::time::Duration, repeat: bool, cx: &mut App) {
    let key = (plugin.to_string(), id);
    let task = cx.spawn({
        let plugin = plugin.to_string();
        async move |cx| {
            loop {
                cx.background_executor().timer(every).await;
                let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
                let alive = cx.update(|cx| {
                    deliver(&plugin, id, Ok(json!({ "at_ms": at })), cx);
                    loaded(&plugin).is_some_and(|p| p.borrow().as_ref().is_some_and(|p| p.is_open(id)))
                });
                if !repeat || !alive {
                    break;
                }
            }
            let _ = cx.update(|_| TIMERS.with(|t| t.borrow_mut().remove(&(plugin.clone(), id))));
        }
    });
    TIMERS.with(|t| t.borrow_mut().insert(key, task));
}

// ── Lifecycle ────────────────────────────────────────────────────────────

/// Plugin `plugin` cancelled these (or lost their grant): stop them.
pub(super) fn stop(plugin: &str, ids: &[u64], cx: &mut App) {
    if ids.is_empty() {
        return;
    }
    files::stop(plugin, Some(ids));
    relay::stop(plugin, Some(ids));
    MEDIA_SUBS.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(list) = s.get_mut(plugin) {
            list.retain(|id| !ids.contains(id));
            if list.is_empty() {
                s.remove(plugin);
            }
        }
    });
    EVENT_SUBS.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(list) = s.get_mut(plugin) {
            list.retain(|(id, _)| !ids.contains(id));
            if list.is_empty() {
                s.remove(plugin);
            }
        }
    });
    TIMERS.with(|t| t.borrow_mut().retain(|(p, id), _| p != plugin || !ids.contains(id)));
    NET_QUEUE.with(|q| q.borrow_mut().retain(|j| j.plugin != plugin || !ids.contains(&j.id)));
    release_watch_if_idle(cx);
}

/// Plugin `plugin` unloaded: its subscriptions and queued network requests go.
pub(super) fn forget(plugin: &str, cx: &mut App) {
    files::stop(plugin, None);
    relay::stop(plugin, None);
    super::launcher_updates::all(cx);
    MEDIA_SUBS.with(|s| s.borrow_mut().remove(plugin));
    EVENT_SUBS.with(|s| s.borrow_mut().remove(plugin));
    TIMERS.with(|t| t.borrow_mut().retain(|(p, _), _| p != plugin));
    NET_QUEUE.with(|q| q.borrow_mut().retain(|j| j.plugin != plugin));
    release_watch_if_idle(cx);
}

/// Drops the hub observer once no plugin is subscribed.
fn release_watch_if_idle(_cx: &mut App) {
    if MEDIA_SUBS.with(|s| s.borrow().is_empty()) {
        MEDIA_WATCH.with(|w| w.borrow_mut().take());
    }
    if EVENT_SUBS.with(|s| s.borrow().is_empty()) {
        EVENT_WATCH.with(|w| w.borrow_mut().take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_are_bounded() {
        let map = |v: Value| v.as_object().cloned().unwrap();
        assert_eq!(page(&map(json!({}))), (0, MAX_PAGE));
        assert_eq!(page(&map(json!({ "offset": 5, "limit": 10_000 }))), (5, MAX_PAGE));
        assert_eq!(page(&map(json!({ "offset": u64::MAX, "limit": 0 }))), (MAX_FOLDER_SCAN, 1));
        assert_eq!(page(&map(json!({ "offset": "x", "limit": -3 }))), (0, MAX_PAGE));
    }

    #[test]
    fn folders_list_only_visible_images() {
        let dir = std::env::temp_dir().join(format!("mandara-broker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for f in ["b.png", "A.jpg", ".hidden.png", "notes.txt", "c.webp"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        std::fs::write(dir.join("sub/deep.png"), b"x").unwrap();
        let names: Vec<String> = images_in(&dir, &dir).iter().map(|p| file_name(p)).collect();
        assert_eq!(names, ["A.jpg", "b.png", "c.webp"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn notification_text_is_clipped_at_a_character_boundary() {
        let long = "é".repeat(MAX_NOTIFY_TEXT);
        let c = clipped(&long);
        assert!(c.len() <= MAX_NOTIFY_TEXT && c.chars().all(|ch| ch == 'é'));
    }
}
