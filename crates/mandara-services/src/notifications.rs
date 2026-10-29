//! Notification server: implements `org.freedesktop.Notifications` (spec
//! 1.2) on the session bus with body markup, actions, inline replies
//! (`inline-reply` / `NotificationReplied`), icons and raw `image-data`,
//! replacement ids, the `transient` / `resident` / `suppress-sound` /
//! `sound-*` / `value` hints, a persistent history and do-not-disturb in
//! four forms: manual, until a time, automatic (screen sharing, fullscreen)
//! and quiet hours.
//!
//! Every decision (per-app rules, popups, history, sounds, timeouts) comes
//! from [`crate::notify_policy`], configured live with
//! [`NotificationCommand::Configure`]. The history is persisted off the
//! executor, debounced: a burst of notifications costs one write.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mandara_config::alerts::NotificationsConfig;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;
use zbus::{fdo, interface};

use crate::actor::{Envelope, ServiceHandle, reply};
pub use crate::notify_policy::Span;
use crate::notify_policy::{Incoming, Policy, parse_markup, quiet_hours_active};

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Urgency {
    Low,
    #[default]
    Normal,
    Critical,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    /// Icon name or path (`app_icon`, the `image-path` hint, or a decoded
    /// `image-data` hint saved to the cache).
    pub icon: String,
    pub summary: String,
    /// Plain text; styling is in [`Notification::markup`].
    pub body: String,
    /// Styled runs of `body` (`body-markup`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markup: Vec<Span>,
    /// `(action key, label)` pairs.
    pub actions: Vec<(String, String)>,
    pub urgency: Urgency,
    /// Unix seconds.
    pub timestamp: i64,
    /// Requested expiry in ms (`-1` = server default, `0` = never).
    pub expire_timeout: i32,
    pub read: bool,
    /// Shown as a transient popup (cleared once it times out).
    pub popup: bool,
    /// The `desktop-entry` hint.
    #[serde(default)]
    pub desktop_entry: String,
    /// The `category` hint.
    #[serde(default)]
    pub category: String,
    /// Popup lifetime the server decided, ms (0 = until dismissed).
    #[serde(default)]
    pub display_ms: u32,
    /// Removed once its popup closes (rules, `transient` hint, history off).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub transient: bool,
    /// Survives invoking an action (`resident` hint).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resident: bool,
    /// Progress percentage (`value` hint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
    /// Repeats folded into this one (`dedupe`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub repeats: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Notification {
    /// The app offers an inline reply.
    pub fn can_reply(&self) -> bool {
        self.actions.iter().any(|(k, _)| k == INLINE_REPLY)
    }
}

/// A sound the UI should play for a new notification.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Chime {
    /// Increments per chime, so repeats of the same notification ring again.
    pub seq: u64,
    pub urgency: Urgency,
    /// `sound-name` / `sound-file` hint.
    pub sound: Option<String>,
}

/// Why do not disturb switched itself on.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutoDnd {
    ScreenShare,
    Fullscreen,
    QuietHours,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct NotificationsState {
    /// `true` once the bus name is owned.
    pub active: bool,
    /// Do not disturb switched on by hand.
    pub dnd: bool,
    /// Manual do not disturb ends at this Unix time.
    pub dnd_until: Option<i64>,
    /// Automatic do not disturb in effect, and why.
    pub auto_dnd: Vec<AutoDnd>,
    /// Newest first.
    pub items: Vec<Notification>,
    /// The latest notification that wants a sound.
    pub chime: Option<Chime>,
}

impl NotificationsState {
    pub fn unread(&self) -> usize {
        self.items.iter().filter(|n| !n.read).count()
    }

    /// Do not disturb in any form.
    pub fn quiet(&self) -> bool {
        self.dnd || !self.auto_dnd.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum NotificationCommand {
    Dismiss(u32),
    DismissApp(String),
    DismissAll,
    Invoke { id: u32, action: String },
    /// Manual do not disturb on / off (clears any end time).
    SetDnd(bool),
    /// Do not disturb until this Unix time.
    DndUntil(i64),
    /// Automatic do not disturb for `reason` on / off.
    Auto(AutoDnd, bool),
    MarkAllRead,
    /// Hides a popup without removing it from history.
    Expire(u32),
    /// Posts a notification from the shell itself (e.g. Pomodoro).
    Post { app: String, icon: String, summary: String, body: String },
    /// Posts a notification with an urgency (`notifications send --urgency`,
    /// Studio's preview buttons).
    PostUrgent { app: String, icon: String, summary: String, body: String, urgency: Urgency },
    /// Inline reply to a notification offering `inline-reply`.
    Reply { id: u32, text: String },
    /// Applies `[notifications]` (rules, history limits, daemon on/off).
    Configure(Box<NotificationsConfig>),
}

pub type NotificationService = ServiceHandle<NotificationsState, NotificationCommand>;

/// Quiet time before the history is written to disk.
const PERSIST_DEBOUNCE: Duration = Duration::from_secs(2);
/// State snapshots buffered for the UI.
const UPDATE_QUEUE: usize = 32;
/// Commands buffered from the UI and CLI.
const COMMAND_QUEUE: usize = 64;
/// How long startup waits for the UI's first `Configure` before taking the
/// bus name (the UI sends it immediately; this only bounds a missing UI).
const CONFIGURE_WAIT: Duration = Duration::from_millis(800);
/// The housekeeping task wakes at least this often (history age, timed
/// do not disturb, quiet hours).
const HOUSEKEEPING: Duration = Duration::from_secs(60);
/// `urgency` hint values (spec: 0 low, 1 normal, 2 critical).
const URGENCY_LOW: u8 = 0;
const URGENCY_CRITICAL: u8 = 2;
/// `expire_timeout` meaning "server default".
const EXPIRE_DEFAULT: i32 = -1;
/// The action key of inline replies (KDE / GNOME extension).
pub const INLINE_REPLY: &str = "inline-reply";
/// Largest `image-data` accepted (pixels on a side).
const IMAGE_DATA_MAX_SIDE: i32 = 1024;
const SECS_PER_HOUR: i64 = 3600;
const BUS_NAME: &str = "org.freedesktop.Notifications";
const OBJECT_PATH: &str = "/org/freedesktop/Notifications";

fn history_path() -> PathBuf {
    mandara_config::paths::cache_dir().join("notifications.json")
}

fn image_dir() -> PathBuf {
    mandara_config::paths::cache_dir().join("notification-images")
}

struct Shared {
    state: Mutex<NotificationsState>,
    policy: Mutex<Policy>,
    next_id: Mutex<u32>,
    chimes: Mutex<u64>,
    updates: async_channel::Sender<NotificationsState>,
    /// Wakes the persister (one pending wake-up is enough).
    persist: async_channel::Sender<()>,
}

/// An incoming notification before the policy runs.
struct Draft {
    n: Notification,
    transient_hint: bool,
    suppress_sound: bool,
    sound: Option<String>,
}

impl Shared {
    fn publish(&self) {
        let snapshot = self.state.lock().clone();
        let _ = self.updates.try_send(snapshot);
        let _ = self.persist.try_send(());
    }

    fn fresh_id(&self) -> u32 {
        let mut next = self.next_id.lock();
        *next = next.wrapping_add(1).max(1);
        *next
    }

    /// Runs the policy and stores the notification. Returns its id (also
    /// for blocked ones: the spec wants an id either way).
    fn insert(&self, draft: Draft, replaces: u32) -> u32 {
        let Draft { mut n, transient_hint, suppress_sound, sound } = draft;
        // Never hold the state and policy locks together.
        let quiet = self.state.lock().quiet();
        let decision = {
            let policy = self.policy.lock();
            let incoming = Incoming {
                app: &n.app_name,
                desktop_entry: &n.desktop_entry,
                summary: &n.summary,
                body: &n.body,
                category: &n.category,
                urgency: n.urgency,
                expire_timeout: n.expire_timeout,
                transient: transient_hint,
                suppress_sound,
            };
            (policy.decide(&incoming, quiet), policy.cfg.dedupe, policy.cfg.history_max)
        };
        let (d, dedupe, history_max) = decision;
        if d.block {
            return if replaces != 0 { replaces } else { self.fresh_id() };
        }
        n.urgency = d.urgency;
        n.display_ms = d.display_ms;
        n.popup = d.popup;
        n.transient = !d.history;
        if n.transient && !n.popup {
            // Nothing to show and nothing to keep.
            return if replaces != 0 { replaces } else { self.fresh_id() };
        }
        let id = {
            let mut st = self.state.lock();
            let duplicate = (replaces == 0 && dedupe)
                .then(|| st.items.iter().find(|x| x.app_name == n.app_name && x.summary == n.summary && x.body == n.body).map(|x| (x.id, x.repeats)))
                .flatten();
            let id = match (replaces, duplicate) {
                (0, Some((id, repeats))) => {
                    n.repeats = repeats + 1;
                    id
                }
                (0, None) => self.fresh_id(),
                (r, _) => r,
            };
            n.id = id;
            st.items.retain(|x| x.id != id);
            st.items.insert(0, n);
            truncate_history(&mut st.items, history_max);
            if d.sound {
                let mut seq = self.chimes.lock();
                *seq += 1;
                st.chime = Some(Chime { seq: *seq, urgency: d.urgency, sound });
            }
            id
        };
        self.publish();
        id
    }

    /// Re-evaluates timed / scheduled quiet modes and history age. Returns
    /// whether anything changed.
    fn housekeeping(&self) -> bool {
        let now = chrono::Local::now();
        let (quiet_hours, max_age, allow_critical) = {
            let p = self.policy.lock();
            (quiet_hours_active(&p.cfg, now.time()), p.cfg.history_max_age_hours, p.cfg.dnd_allow_critical)
        };
        let mut st = self.state.lock();
        let before = (st.dnd, st.dnd_until, st.auto_dnd.clone(), st.items.len());
        if st.dnd_until.is_some_and(|until| now.timestamp() >= until) {
            st.dnd = false;
            st.dnd_until = None;
        }
        set_reason(&mut st.auto_dnd, AutoDnd::QuietHours, quiet_hours);
        if max_age > 0 {
            let cutoff = now.timestamp() - i64::from(max_age) * SECS_PER_HOUR;
            st.items.retain(|n| n.popup || n.timestamp >= cutoff);
        }
        let changed = before != (st.dnd, st.dnd_until, st.auto_dnd.clone(), st.items.len());
        if changed && st.quiet() {
            hide_popups(&mut st.items, allow_critical);
        }
        changed
    }
}

/// Keeps at most `max` history entries (popups showing right now count but
/// are never cut).
fn truncate_history(items: &mut Vec<Notification>, max: usize) {
    let mut kept = 0usize;
    items.retain(|n| {
        if n.popup {
            return true;
        }
        kept += 1;
        kept <= max
    });
}

fn set_reason(list: &mut Vec<AutoDnd>, reason: AutoDnd, on: bool) {
    let has = list.contains(&reason);
    if on && !has {
        list.push(reason);
    } else if !on && has {
        list.retain(|r| *r != reason);
    }
}

/// Do not disturb took effect: popups that wouldn't be allowed now close
/// (critical ones stay when allowed); transient ones are dropped.
fn hide_popups(items: &mut Vec<Notification>, allow_critical: bool) {
    for n in items.iter_mut() {
        if !(n.urgency == Urgency::Critical && allow_critical) {
            n.popup = false;
        }
    }
    items.retain(|n| n.popup || !n.transient);
}

struct Server {
    shared: Arc<Shared>,
}

/// Reason codes for `NotificationClosed`.
const EXPIRED: u32 = 1;
const DISMISSED: u32 = 2;
const CLOSED_BY_CALL: u32 = 3;

fn hint_str(hints: &HashMap<String, OwnedValue>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| hints.get(*k).and_then(|v| String::try_from(v.try_clone().ok()?).ok())).filter(|s| !s.is_empty())
}

fn hint_bool(hints: &HashMap<String, OwnedValue>, key: &str) -> bool {
    hints.get(key).and_then(|v| bool::try_from(v.try_clone().ok()?).ok()).unwrap_or(false)
}

/// Saves an `image-data` hint (`(iiibiiay)`: width, height, rowstride,
/// has alpha, bits per sample, channels, pixels) as a cached PNG.
fn save_image_data(v: &OwnedValue) -> Option<String> {
    let (w, h, stride, alpha, bits, channels, data): (i32, i32, i32, bool, i32, i32, Vec<u8>) = v.try_clone().ok()?.try_into().ok()?;
    const BITS: i32 = 8;
    let want = if alpha { 4 } else { 3 };
    if bits != BITS || channels != want || !(1..=IMAGE_DATA_MAX_SIDE).contains(&w) || !(1..=IMAGE_DATA_MAX_SIDE).contains(&h) || stride < w * channels {
        return None;
    }
    let (w, h, stride, ch) = (w as usize, h as usize, stride as usize, channels as usize);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let line = data.get(row * stride..row * stride + w * ch)?;
        for px in line.chunks_exact(ch) {
            rgba.extend_from_slice(&px[..3]);
            rgba.push(if alpha { px[3] } else { u8::MAX });
        }
    }
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    rgba.hash(&mut hasher);
    let dir = image_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{:016x}.png", hasher.finish()));
    if !path.exists() {
        image::RgbaImage::from_raw(w as u32, h as u32, rgba)?.save(&path).ok()?;
    }
    Some(path.to_string_lossy().into_owned())
}

#[interface(name = "org.freedesktop.Notifications")]
impl Server {
    fn get_capabilities(&self) -> Vec<&'static str> {
        let markup = self.shared.policy.lock().cfg.markup;
        let mut caps = vec!["body", "actions", "icon-static", "persistence", "body-hyperlinks", "inline-reply", "sound"];
        if markup {
            caps.push("body-markup");
        }
        caps
    }

    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let urgency = hints
            .get("urgency")
            .and_then(|v| u8::try_from(v.try_clone().ok()?).ok())
            .map(|u| match u {
                URGENCY_LOW => Urgency::Low,
                URGENCY_CRITICAL => Urgency::Critical,
                _ => Urgency::Normal,
            })
            .unwrap_or_default();
        let image_data = ["image-data", "image_data", "icon_data"].iter().find_map(|k| hints.get(*k)).and_then(save_image_data);
        let icon = image_data.or_else(|| hint_str(&hints, &["image-path", "image_path"])).unwrap_or(app_icon);
        let actions = actions.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0].clone(), c[1].clone())).collect();
        let progress = hints.get("value").and_then(|v| i32::try_from(v.try_clone().ok()?).ok()).map(|v| v.clamp(0, 100) as u8);
        let markup = self.shared.policy.lock().cfg.markup;
        let (body, spans) = parse_markup(&body);
        let n = Notification {
            id: 0,
            app_name,
            icon,
            summary,
            body,
            markup: if markup { spans } else { Vec::new() },
            actions,
            urgency,
            timestamp: chrono::Local::now().timestamp(),
            expire_timeout,
            read: false,
            popup: true,
            desktop_entry: hint_str(&hints, &["desktop-entry"]).unwrap_or_default(),
            category: hint_str(&hints, &["category"]).unwrap_or_default(),
            display_ms: 0,
            transient: false,
            resident: hint_bool(&hints, "resident"),
            progress,
            repeats: 0,
        };
        let draft = Draft {
            n,
            transient_hint: hint_bool(&hints, "transient"),
            suppress_sound: hint_bool(&hints, "suppress-sound"),
            sound: hint_str(&hints, &["sound-file", "sound-name"]),
        };
        self.shared.insert(draft, replaces_id)
    }

    async fn close_notification(&self, id: u32, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        self.shared.state.lock().items.retain(|n| n.id != id);
        self.shared.publish();
        let _ = Server::notification_closed(&emitter, id, CLOSED_BY_CALL).await;
    }

    fn get_server_information(&self) -> (&'static str, &'static str, &'static str, &'static str) {
        ("Mandara Shell", "Mandara", env!("CARGO_PKG_VERSION"), "1.2")
    }

    #[zbus(signal)]
    async fn notification_closed(emitter: &SignalEmitter<'_>, id: u32, reason: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn action_invoked(emitter: &SignalEmitter<'_>, id: u32, action_key: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn notification_replied(emitter: &SignalEmitter<'_>, id: u32, text: &str) -> zbus::Result<()>;
}

/// Very small markup stripper (`<b>`, `<i>`, `<a href>`…) and entity decoder.
pub fn strip_markup(s: &str) -> String {
    parse_markup(s).0
}

/// Reads the saved history (popups are not restored).
fn load_history() -> Vec<Notification> {
    std::fs::read_to_string(history_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<Notification>>(&t).ok())
        .map(|mut items| {
            for n in &mut items {
                n.popup = false;
            }
            items.retain(|n| !n.transient);
            items
        })
        .unwrap_or_default()
}

/// Writes the history atomically (temp file + rename).
fn save_history(items: &[Notification]) -> std::io::Result<()> {
    let path = history_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec(items).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(tmp, path)
}

/// Persists the history after each burst of changes settles.
async fn persister(shared: Arc<Shared>, wake: async_channel::Receiver<()>) {
    while wake.recv().await.is_ok() {
        crate::runtime::sleep(PERSIST_DEBOUNCE).await;
        while wake.try_recv().is_ok() {}
        let items: Vec<Notification> = shared.state.lock().items.iter().filter(|n| !n.transient).cloned().collect();
        if let Err(err) = crate::runtime::unblock(move || save_history(&items)).await {
            tracing::warn!("notifications: saving history: {err}");
        }
    }
}

/// Wakes for timed do not disturb, quiet-hour boundaries and history age.
async fn housekeeper(shared: Arc<Shared>, poke: async_channel::Receiver<()>) {
    use futures::FutureExt as _;
    loop {
        if shared.housekeeping() {
            shared.publish();
        }
        // Sleep until the next minute boundary (quiet hours are HH:MM), or
        // until a command changes the schedule.
        let secs = chrono::Local::now().timestamp().rem_euclid(HOUSEKEEPING.as_secs() as i64) as u64;
        let wait = HOUSEKEEPING.saturating_sub(Duration::from_secs(secs));
        futures::select! {
            _ = crate::runtime::sleep(wait).fuse() => {}
            r = poke.recv().fuse() => if r.is_err() { return },
        }
    }
}

/// Owns or releases the bus name to follow `notifications.daemon`.
async fn follow_daemon_setting(conn: &zbus::Connection, shared: &Arc<Shared>, want: bool) {
    let owned = shared.state.lock().active;
    if want && !owned {
        let flags = fdo::RequestNameFlags::ReplaceExisting | fdo::RequestNameFlags::AllowReplacement;
        match conn.request_name_with_flags(BUS_NAME, flags.into()).await {
            Ok(fdo::RequestNameReply::PrimaryOwner | fdo::RequestNameReply::AlreadyOwner) => shared.state.lock().active = true,
            Ok(_) => {
                // Queued behind a daemon that refuses replacement (e.g.
                // mako): take over as soon as it exits.
                tracing::warn!("another notification daemon owns {BUS_NAME}; queued until it exits");
                let watcher = shared.clone();
                let conn2 = conn.clone();
                crate::runtime::spawn("notifications-takeover", async move {
                    use futures::StreamExt;
                    let Ok(dbus) = fdo::DBusProxy::new(&conn2).await else { return };
                    let Ok(mut acquired) = dbus.receive_name_acquired().await else { return };
                    while let Some(sig) = acquired.next().await {
                        if sig.args().is_ok_and(|a| a.name() == BUS_NAME) {
                            watcher.state.lock().active = true;
                            watcher.publish();
                            tracing::info!("now serving {BUS_NAME}");
                            break;
                        }
                    }
                });
            }
            Err(err) => tracing::warn!("notifications: {err}"),
        }
    } else if !want {
        // Also leaves the queue when waiting behind another daemon.
        if let Err(err) = conn.release_name(BUS_NAME).await {
            tracing::debug!("notifications: releasing {BUS_NAME}: {err}");
        }
        shared.state.lock().active = false;
        tracing::info!("built-in notification daemon off; {BUS_NAME} released");
    }
}

pub fn spawn_notification_service() -> NotificationService {
    let (tx, updates) = async_channel::bounded(UPDATE_QUEUE);
    let (commands, rx) = async_channel::bounded::<Envelope<NotificationCommand>>(COMMAND_QUEUE);
    crate::runtime::spawn("notifications", async move {
        use futures::FutureExt as _;
        let history = crate::runtime::unblock(load_history).await;
        let next = history.iter().map(|n| n.id).max().unwrap_or(0);
        let (persist, persist_rx) = async_channel::bounded(1);
        let shared = Arc::new(Shared {
            state: Mutex::new(NotificationsState { items: history, ..Default::default() }),
            policy: Mutex::new(Policy::default()),
            next_id: Mutex::new(next),
            chimes: Mutex::new(0),
            updates: tx,
            persist,
        });
        crate::runtime::spawn("notifications-persist", persister(shared.clone(), persist_rx));
        let (house_tx, house_rx) = async_channel::bounded::<()>(1);
        crate::runtime::spawn("notifications-housekeeping", housekeeper(shared.clone(), house_rx));
        // The UI configures us right away; apply that before deciding
        // whether to own the bus name at all.
        let mut pending: Option<Envelope<NotificationCommand>> = None;
        futures::select! {
            first = rx.recv().fuse() => match first {
                Ok(Envelope { command: NotificationCommand::Configure(cfg), reply: r }) => {
                    *shared.policy.lock() = Policy::new(*cfg);
                    reply(&r, Ok("configured".into()));
                }
                Ok(other) => pending = Some(other),
                Err(_) => return,
            },
            _ = crate::runtime::sleep(CONFIGURE_WAIT).fuse() => {}
        }
        let conn = match zbus::connection::Builder::session().and_then(|b| b.serve_at(OBJECT_PATH, Server { shared: shared.clone() })) {
            Ok(b) => match crate::runtime::connect(b).await {
                Ok(c) => c,
                Err(err) => {
                    tracing::error!("notifications: {err}");
                    return;
                }
            },
            Err(err) => {
                tracing::error!("notifications: {err}");
                return;
            }
        };
        let daemon = shared.policy.lock().cfg.daemon;
        follow_daemon_setting(&conn, &shared, daemon).await;
        shared.housekeeping();
        shared.publish();
        let emitter = SignalEmitter::new(&conn, OBJECT_PATH).ok();
        loop {
            let envelope = match pending.take() {
                Some(e) => e,
                None => match rx.recv().await {
                    Ok(e) => e,
                    Err(_) => return,
                },
            };
            let Envelope { command, reply: r } = envelope;
            let mut closed: Vec<(u32, u32)> = Vec::new();
            let mut invoked: Option<(u32, String)> = None;
            let mut replied: Option<(u32, String)> = None;
            let result: Result<String, String> = match command {
                NotificationCommand::Configure(cfg) => {
                    let want = cfg.daemon;
                    let changed_daemon = shared.policy.lock().cfg.daemon != want;
                    {
                        let mut st = shared.state.lock();
                        if !cfg.history {
                            st.items.retain(|n| n.popup);
                        }
                        truncate_history(&mut st.items, cfg.history_max);
                    }
                    *shared.policy.lock() = Policy::new(*cfg);
                    if changed_daemon {
                        follow_daemon_setting(&conn, &shared, want).await;
                    }
                    let _ = house_tx.try_send(());
                    Ok("configured".into())
                }
                NotificationCommand::Post { app, icon, summary, body } => {
                    let n = Notification {
                        app_name: app,
                        icon,
                        summary,
                        body,
                        timestamp: chrono::Local::now().timestamp(),
                        expire_timeout: EXPIRE_DEFAULT,
                        popup: true,
                        ..Default::default()
                    };
                    let id = shared.insert(Draft { n, transient_hint: false, suppress_sound: false, sound: None }, 0);
                    reply(&r, Ok(format!("posted {id}")));
                    continue;
                }
                NotificationCommand::PostUrgent { app, icon, summary, body, urgency } => {
                    let n = Notification {
                        app_name: app,
                        icon,
                        summary,
                        body,
                        urgency,
                        timestamp: chrono::Local::now().timestamp(),
                        expire_timeout: EXPIRE_DEFAULT,
                        popup: true,
                        ..Default::default()
                    };
                    let id = shared.insert(Draft { n, transient_hint: false, suppress_sound: false, sound: None }, 0);
                    reply(&r, Ok(format!("posted {id}")));
                    continue;
                }
                command => {
                    let (keep_dismissed, allow_critical) = {
                        let p = shared.policy.lock();
                        (p.cfg.keep_dismissed, p.cfg.dnd_allow_critical)
                    };
                    let mut st = shared.state.lock();
                    match command {
                        NotificationCommand::Dismiss(id) => {
                            let popup_only = keep_dismissed && st.items.iter().any(|n| n.id == id && n.popup && !n.transient);
                            if popup_only {
                                if let Some(n) = st.items.iter_mut().find(|n| n.id == id) {
                                    n.popup = false;
                                    n.read = true;
                                }
                            } else {
                                st.items.retain(|n| n.id != id);
                            }
                            closed.push((id, DISMISSED));
                            Ok("dismissed".into())
                        }
                        NotificationCommand::DismissApp(app) => {
                            st.items.retain(|n| {
                                let keep = n.app_name != app;
                                if !keep {
                                    closed.push((n.id, DISMISSED));
                                }
                                keep
                            });
                            Ok(format!("cleared {app}"))
                        }
                        NotificationCommand::DismissAll => {
                            closed.extend(st.items.iter().map(|n| (n.id, DISMISSED)));
                            st.items.clear();
                            Ok("cleared".into())
                        }
                        NotificationCommand::Invoke { id, action } => {
                            invoked = Some((id, action));
                            let resident = st.items.iter().any(|n| n.id == id && n.resident);
                            if resident {
                                if let Some(n) = st.items.iter_mut().find(|n| n.id == id) {
                                    n.popup = false;
                                    n.read = true;
                                }
                            } else {
                                st.items.retain(|n| n.id != id);
                                closed.push((id, DISMISSED));
                            }
                            Ok("invoked".into())
                        }
                        NotificationCommand::Reply { id, text } => {
                            if !st.items.iter().any(|n| n.id == id && n.can_reply()) {
                                Err(format!("notification {id} takes no reply"))
                            } else {
                                replied = Some((id, text));
                                st.items.retain(|n| n.id != id);
                                closed.push((id, DISMISSED));
                                Ok("replied".into())
                            }
                        }
                        NotificationCommand::SetDnd(on) => {
                            st.dnd = on;
                            st.dnd_until = None;
                            if on {
                                hide_popups(&mut st.items, allow_critical);
                            }
                            Ok(format!("do not disturb {}", if on { "on" } else { "off" }))
                        }
                        NotificationCommand::DndUntil(until) => {
                            st.dnd = true;
                            st.dnd_until = Some(until);
                            hide_popups(&mut st.items, allow_critical);
                            let _ = house_tx.try_send(());
                            Ok(format!("do not disturb until {until}"))
                        }
                        NotificationCommand::Auto(reason, on) => {
                            set_reason(&mut st.auto_dnd, reason, on);
                            if on {
                                hide_popups(&mut st.items, allow_critical);
                            }
                            Ok(format!("automatic do not disturb {}", if st.auto_dnd.is_empty() { "off" } else { "on" }))
                        }
                        NotificationCommand::MarkAllRead => {
                            for n in &mut st.items {
                                n.read = true;
                            }
                            Ok("read".into())
                        }
                        NotificationCommand::Expire(id) => {
                            let transient = st.items.iter().any(|n| n.id == id && n.transient);
                            if transient {
                                st.items.retain(|n| n.id != id);
                            } else if let Some(n) = st.items.iter_mut().find(|n| n.id == id) {
                                n.popup = false;
                            }
                            closed.push((id, EXPIRED));
                            Ok("expired".into())
                        }
                        NotificationCommand::Post { .. } | NotificationCommand::PostUrgent { .. } | NotificationCommand::Configure(_) => unreachable!("handled above"),
                    }
                }
            };
            shared.publish();
            if let Some(em) = &emitter {
                if let Some((id, action)) = &invoked {
                    let _ = Server::action_invoked(em, *id, action).await;
                }
                if let Some((id, text)) = &replied {
                    let _ = Server::notification_replied(em, *id, text).await;
                }
                for (id, reason) in closed.into_iter().filter(|(_, reason)| *reason != EXPIRED) {
                    let _ = Server::notification_closed(em, id, reason).await;
                }
            }
            reply(&r, result);
        }
    });
    ServiceHandle::new(updates, commands)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_markup() {
        assert_eq!(strip_markup("<b>Hi</b> &amp; <a href='x'>there</a>"), "Hi & there");
    }

    fn note(id: u32, read: bool, popup: bool) -> Notification {
        Notification { id, app_name: "a".into(), summary: "s".into(), read, popup, ..Default::default() }
    }

    #[test]
    fn unread_counts() {
        let st = NotificationsState { active: true, items: vec![note(1, false, false), note(2, true, false), note(3, false, false)], ..Default::default() };
        assert_eq!(st.unread(), 2);
        assert!(!st.quiet());
        let auto = NotificationsState { auto_dnd: vec![AutoDnd::ScreenShare], ..Default::default() };
        assert!(auto.quiet());
    }

    #[test]
    fn history_truncation_spares_live_popups() {
        let mut items = vec![note(5, false, true), note(4, false, false), note(3, false, false), note(2, false, true), note(1, false, false)];
        truncate_history(&mut items, 1);
        assert_eq!(items.iter().map(|n| n.id).collect::<Vec<_>>(), vec![5, 4, 2]);
    }

    #[test]
    fn dnd_hides_popups_but_keeps_critical() {
        let mut crit = note(1, false, true);
        crit.urgency = Urgency::Critical;
        let mut transient = note(2, false, true);
        transient.transient = true;
        let mut items = vec![crit, transient, note(3, false, true)];
        hide_popups(&mut items, true);
        assert_eq!(items.iter().map(|n| (n.id, n.popup)).collect::<Vec<_>>(), vec![(1, true), (3, false)]);
        let mut reasons = Vec::new();
        set_reason(&mut reasons, AutoDnd::Fullscreen, true);
        set_reason(&mut reasons, AutoDnd::Fullscreen, true);
        assert_eq!(reasons, vec![AutoDnd::Fullscreen]);
        set_reason(&mut reasons, AutoDnd::Fullscreen, false);
        assert!(reasons.is_empty());
    }

    #[test]
    fn legacy_history_entries_load() {
        let old = r#"[{"id":3,"app_name":"a","icon":"","summary":"s","body":"b","actions":[],"urgency":"normal","timestamp":1,"expire_timeout":-1,"read":true,"popup":false}]"#;
        let items: Vec<Notification> = serde_json::from_str(old).unwrap();
        assert_eq!(items[0].display_ms, 0);
        assert!(!items[0].transient && items[0].markup.is_empty());
    }
}
