//! System tray (session bus): a `org.kde.StatusNotifierWatcher` when the
//! name is free (otherwise Mandara registers as a host with the existing
//! watcher), live `StatusNotifierItem` tracking (title, status, icons,
//! tooltip, menu) driven by the items' `New*` signals, and a
//! `com.canonical.dbusmenu` client for their menus (submenus, separators,
//! check / radio items, icons, enabled / visible) with `AboutToShow`,
//! `Event("clicked")` and `LayoutUpdated` handling.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash as _, Hasher as _};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::{FutureExt as _, StreamExt as _};
use parking_lot::Mutex;
use serde::Serialize;
use zbus::message::Header;
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedValue, Value};
use zbus::{Connection, MatchRule, Message, MessageStream, fdo, interface};

use crate::actor::{Envelope, Reply, ServiceHandle, proxy, reply};

/// dbusmenu layout updates are coalesced for this long (protocol chatter).
const MENU_SETTLE: Duration = Duration::from_millis(120);

const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const KDE_ITEM: &str = "org.kde.StatusNotifierItem";
const FDO_ITEM: &str = "org.freedesktop.StatusNotifierItem";
const DBUSMENU: &str = "com.canonical.dbusmenu";
const DEFAULT_ITEM_PATH: &str = "/StatusNotifierItem";
/// Preferred pixmap edge (covers a ~22 px icon at 2x).
const WANT_PIXMAP: i32 = 48;
/// Item signals arrive in bursts (icon + tooltip + title): coalesce them.
const DEBOUNCE: Duration = Duration::from_millis(40);
/// Grace period for an item to export its object before we adopt it.
const ADOPT_DELAY: Duration = Duration::from_millis(400);
/// Wait when nothing is scheduled (the select! still wakes on events).
const IDLE_WAIT: Duration = Duration::from_secs(3600);
/// Queued signals per subscription (menus can burst).
const SIGNAL_QUEUE: usize = 256;
/// States buffered for the UI; commands (and item registrations) queued.
const UPDATE_QUEUE: usize = 16;
const COMMAND_QUEUE: usize = 64;

// ── Public model ─────────────────────────────────────────────────────────

/// A straight-alpha RGBA image (converted from the protocol's ARGB32).
#[derive(Clone, Debug, Serialize)]
pub struct Pixmap {
    pub width: u32,
    pub height: u32,
    #[serde(skip)]
    pub rgba: Arc<[u8]>,
    /// Content hash (cheap equality and cache key).
    #[serde(skip)]
    pub hash: u64,
}

impl PartialEq for Pixmap {
    fn eq(&self, other: &Self) -> bool {
        self.width == other.width && self.height == other.height && self.hash == other.hash
    }
}

impl Pixmap {
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Option<Self> {
        if width == 0 || height == 0 || rgba.len() != (width * height * 4) as usize {
            return None;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (width, height).hash(&mut h);
        rgba.hash(&mut h);
        Some(Self { width, height, rgba: rgba.into(), hash: h.finish() })
    }

    /// Network-byte-order ARGB32 (the SNI wire format) to RGBA.
    pub fn from_argb(width: u32, height: u32, argb: &[u8]) -> Option<Self> {
        if argb.len() != (width as usize) * (height as usize) * 4 {
            return None;
        }
        let rgba = argb.chunks_exact(4).flat_map(|p| [p[1], p[2], p[3], p[0]]).collect();
        Self::from_rgba(width, height, rgba)
    }

    /// Decodes an encoded image (dbusmenu `icon-data` is PNG).
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let img = image::load_from_memory(bytes).ok()?.to_rgba8();
        let (w, h) = img.dimensions();
        Self::from_rgba(w, h, img.into_raw())
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ItemStatus {
    /// Nothing interesting; hosts may hide it.
    Passive,
    #[default]
    Active,
    NeedsAttention,
}

impl ItemStatus {
    fn parse(s: &str) -> Self {
        match s {
            "Passive" => ItemStatus::Passive,
            "NeedsAttention" => ItemStatus::NeedsAttention,
            _ => ItemStatus::Active,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Tooltip {
    pub icon_name: String,
    pub title: String,
    /// Markup stripped.
    pub body: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TrayItem {
    /// Stable identity: `service` + `path`.
    pub key: String,
    /// Bus name the item registered with (well-known or unique).
    pub service: String,
    pub path: String,
    /// Unique bus name of the owner (signals come from it).
    #[serde(skip)]
    pub owner: String,
    /// Interface the item implements (`org.kde.…` or `org.freedesktop.…`).
    #[serde(skip)]
    iface: &'static str,
    pub id: String,
    pub title: String,
    pub category: String,
    pub status: ItemStatus,
    pub icon_name: String,
    /// Extra icon directory to search for `icon_name`.
    pub icon_theme_path: String,
    pub icon: Option<Pixmap>,
    pub attention_icon_name: String,
    pub attention_icon: Option<Pixmap>,
    pub overlay_icon_name: String,
    pub tooltip: Option<Tooltip>,
    /// dbusmenu object path, if the item exports a menu.
    pub menu: Option<String>,
    /// The item only supports its menu (no `Activate`).
    pub item_is_menu: bool,
}

impl TrayItem {
    /// The best human name: title, tooltip title, then id.
    pub fn display_name(&self) -> String {
        let title = self.title.trim();
        let tooltip = self.tooltip.as_ref().map(|t| t.title.trim()).unwrap_or("");
        if !title.is_empty() {
            // `easyeffects` → its nicer tooltip spelling `Easy Effects`.
            let squash = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
            return if squash(tooltip) == squash(title) { tooltip.to_string() } else { title.to_string() };
        }
        let id = prettify_id(self.id.trim());
        // `Docker Desktop` (id) beats `Docker Desktop stopped` (tooltip),
        // but a web app's `ChatGPT` tooltip beats its generic `chrome` id.
        if !id.is_empty() && (tooltip.is_empty() || tooltip.to_lowercase().starts_with(&id.to_lowercase())) {
            return id;
        }
        if !tooltip.is_empty() {
            return tooltip.to_string();
        }
        self.service.clone()
    }

    /// Whether `query` names this item (key, id, title or service).
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim();
        self.key == q
            || self.service == q
            || self.id.eq_ignore_ascii_case(q)
            || self.title.eq_ignore_ascii_case(q)
            || self.display_name().eq_ignore_ascii_case(q)
            || (!q.is_empty() && self.display_name().to_lowercase().contains(&q.to_lowercase()))
    }
}

/// `Docker Desktop_status_icon_1` → `Docker Desktop`.
fn prettify_id(s: &str) -> String {
    let s = s.split("_status_icon").next().unwrap_or(s);
    s.trim_end_matches(|c: char| c == '_' || c == '-').to_string()
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToggleKind {
    #[default]
    None,
    Checkmark,
    Radio,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct MenuNode {
    pub id: i32,
    /// Mnemonic underscores removed.
    pub label: String,
    pub separator: bool,
    pub enabled: bool,
    pub visible: bool,
    pub toggle: ToggleKind,
    /// `Some(on)` for check / radio items; `None` when indeterminate.
    pub toggled: Option<bool>,
    pub icon_name: String,
    pub icon: Option<Pixmap>,
    /// Human shortcut, e.g. `Ctrl+Q`.
    pub shortcut: String,
    /// Has (or will lazily load) children.
    pub submenu: bool,
    pub children: Vec<MenuNode>,
}

impl MenuNode {
    /// Depth-first search by id.
    pub fn find(&self, id: i32) -> Option<&MenuNode> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TrayMenu {
    /// Item key.
    pub key: String,
    pub revision: u32,
    pub root: MenuNode,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrayRole {
    /// Not connected to the session bus (yet).
    #[default]
    Offline,
    /// Mandara owns `org.kde.StatusNotifierWatcher`.
    Watcher,
    /// Another watcher exists; Mandara is one of its hosts.
    Host,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TrayState {
    pub role: TrayRole,
    /// In registration order.
    pub items: Vec<TrayItem>,
    /// The menu currently open (one at a time).
    pub menu: Option<TrayMenu>,
}

impl TrayState {
    pub fn find(&self, query: &str) -> Option<&TrayItem> {
        self.items.iter().find(|i| i.key == query).or_else(|| self.items.iter().find(|i| i.matches(query)))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TrayCommand {
    /// Primary action at screen position `(x, y)`.
    Activate { item: String, x: i32, y: i32 },
    /// Middle-click action.
    SecondaryActivate { item: String, x: i32, y: i32 },
    /// Ask the item to show its own context menu (items without dbusmenu).
    ContextMenu { item: String, x: i32, y: i32 },
    /// Scroll steps (positive = down / right).
    Scroll { item: String, delta: i32, horizontal: bool },
    /// Loads the item's dbusmenu into [`TrayState::menu`] and follows it.
    OpenMenu(String),
    CloseMenu,
    /// Activates menu entry `id` of the open menu.
    MenuClick { id: i32 },
    /// Tells the app a submenu is about to show (it may fill it lazily).
    MenuAboutToShow { id: i32 },
}

pub type TrayService = ServiceHandle<TrayState, TrayCommand>;

// ── Watcher object ───────────────────────────────────────────────────────

#[derive(Default)]
struct WatcherCore {
    items: Mutex<Vec<String>>,
    hosts: Mutex<Vec<String>>,
}

struct Watcher {
    core: Arc<WatcherCore>,
    registered: async_channel::Sender<String>,
}

/// `service/path` address for a registration. Apps pass either a bus name
/// (path defaults to `/StatusNotifierItem`) or just an object path (the
/// sender is the service).
fn item_address(service: &str, sender: &str) -> String {
    if service.starts_with('/') {
        format!("{sender}{service}")
    } else if service.contains('/') {
        service.to_string()
    } else {
        format!("{service}{DEFAULT_ITEM_PATH}")
    }
}

/// Splits a `service/path` address.
fn split_address(address: &str) -> (String, String) {
    match address.find('/') {
        Some(i) => (address[..i].to_string(), address[i..].to_string()),
        None => (address.to_string(), DEFAULT_ITEM_PATH.to_string()),
    }
}

#[interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(&self, service: &str, #[zbus(header)] header: Header<'_>, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
        let address = item_address(service, &sender);
        let fresh = {
            let mut items = self.core.items.lock();
            let fresh = !items.contains(&address);
            if fresh {
                items.push(address.clone());
            }
            fresh
        };
        if fresh {
            let _ = Watcher::status_notifier_item_registered(&emitter, &address).await;
            let _ = self.registered_status_notifier_items_changed(&emitter).await;
        }
        let _ = self.registered.send(address).await;
    }

    async fn register_status_notifier_host(&self, service: &str, #[zbus(header)] header: Header<'_>, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        let name = if service.is_empty() { header.sender().map(|s| s.to_string()).unwrap_or_default() } else { service.to_string() };
        {
            let mut hosts = self.core.hosts.lock();
            if !hosts.contains(&name) {
                hosts.push(name);
            }
        }
        let _ = Watcher::status_notifier_host_registered(&emitter).await;
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.core.items.lock().clone()
    }

    /// Mandara itself is always a host.
    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(emitter: &SignalEmitter<'_>, service: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(emitter: &SignalEmitter<'_>, service: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_unregistered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

// ── Value helpers ────────────────────────────────────────────────────────

fn unwrap_variant<'a, 'b>(v: &'a Value<'b>) -> &'a Value<'b> {
    match v {
        Value::Value(inner) => unwrap_variant(inner),
        other => other,
    }
}

fn v_str(v: &Value) -> Option<String> {
    match unwrap_variant(v) {
        Value::Str(s) => Some(s.to_string()),
        Value::ObjectPath(p) => Some(p.to_string()),
        _ => None,
    }
}

fn v_bool(v: &Value) -> Option<bool> {
    match unwrap_variant(v) {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

fn v_i64(v: &Value) -> Option<i64> {
    Some(match unwrap_variant(v) {
        Value::I32(n) => *n as i64,
        Value::U32(n) => *n as i64,
        Value::I16(n) => *n as i64,
        Value::U16(n) => *n as i64,
        Value::I64(n) => *n,
        Value::U64(n) => *n as i64,
        Value::U8(n) => *n as i64,
        _ => return None,
    })
}

fn v_bytes(v: &Value) -> Option<Vec<u8>> {
    match unwrap_variant(v) {
        Value::Array(a) => a.inner().iter().map(|x| if let Value::U8(b) = x { Some(*b) } else { None }).collect(),
        _ => None,
    }
}

/// Picks the best entry of an `a(iiay)` pixmap list: the smallest one at
/// least [`WANT_PIXMAP`] wide, else the largest.
fn v_pixmap(v: &Value) -> Option<Pixmap> {
    let Value::Array(list) = unwrap_variant(v) else { return None };
    let mut candidates: Vec<(i32, i32, &Value)> = Vec::new();
    for entry in list.inner() {
        let Value::Structure(s) = unwrap_variant(entry) else { continue };
        let f = s.fields();
        if f.len() < 3 {
            continue;
        }
        let (Some(w), Some(h)) = (v_i64(&f[0]), v_i64(&f[1])) else { continue };
        if w > 0 && h > 0 && w <= 1024 && h <= 1024 {
            candidates.push((w as i32, h as i32, &f[2]));
        }
    }
    let pick = candidates
        .iter()
        .filter(|(w, _, _)| *w >= WANT_PIXMAP)
        .min_by_key(|(w, _, _)| *w)
        .or_else(|| candidates.iter().max_by_key(|(w, _, _)| *w))?;
    let bytes = v_bytes(pick.2)?;
    Pixmap::from_argb(pick.0 as u32, pick.1 as u32, &bytes)
}

fn v_tooltip(v: &Value) -> Option<Tooltip> {
    let Value::Structure(s) = unwrap_variant(v) else { return None };
    let f = s.fields();
    if f.len() < 4 {
        return None;
    }
    let t = Tooltip {
        icon_name: v_str(&f[0]).unwrap_or_default(),
        title: crate::notifications::strip_markup(&v_str(&f[2]).unwrap_or_default()).trim().to_string(),
        body: crate::notifications::strip_markup(&v_str(&f[3]).unwrap_or_default()).trim().to_string(),
    };
    (!t.title.is_empty() || !t.body.is_empty()).then_some(t)
}

/// Removes GTK-style mnemonics: `_File` → `File`, `__` → `_`.
pub fn strip_mnemonic(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '_' {
            if chars.peek() == Some(&'_') {
                chars.next();
                out.push('_');
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// dbusmenu `shortcut` (`aas`, e.g. `[["Control", "q"]]`) → `Ctrl+Q`.
fn v_shortcut(v: &Value) -> String {
    let Value::Array(chords) = unwrap_variant(v) else { return String::new() };
    let Some(Value::Array(first)) = chords.inner().first().map(unwrap_variant) else { return String::new() };
    first
        .inner()
        .iter()
        .filter_map(v_str)
        .map(|k| match k.as_str() {
            "Control" => "Ctrl".to_string(),
            "Super" => "Super".to_string(),
            k if k.chars().count() == 1 => k.to_uppercase(),
            k => k.to_string(),
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Parses one `(ia{sv}av)` layout node.
fn parse_node(id: i32, props: &HashMap<String, OwnedValue>, children: &[OwnedValue]) -> MenuNode {
    let get = |k: &str| props.get(k).map(|v| &**v);
    let toggle = match get("toggle-type").and_then(v_str).as_deref() {
        Some("checkmark") => ToggleKind::Checkmark,
        Some("radio") => ToggleKind::Radio,
        _ => ToggleKind::None,
    };
    let state = get("toggle-state").and_then(v_i64).unwrap_or(0);
    let kids: Vec<MenuNode> = children.iter().filter_map(|c| parse_value_node(c)).collect();
    MenuNode {
        id,
        label: strip_mnemonic(&get("label").and_then(v_str).unwrap_or_default()),
        separator: get("type").and_then(v_str).as_deref() == Some("separator"),
        enabled: get("enabled").and_then(v_bool).unwrap_or(true),
        visible: get("visible").and_then(v_bool).unwrap_or(true),
        toggle,
        toggled: if toggle == ToggleKind::None { None } else { match state { 1 => Some(true), 0 => Some(false), _ => None } },
        icon_name: get("icon-name").and_then(v_str).unwrap_or_default(),
        icon: get("icon-data").and_then(v_bytes).and_then(|b| Pixmap::decode(&b)),
        shortcut: get("shortcut").map(v_shortcut).unwrap_or_default(),
        submenu: get("children-display").and_then(v_str).as_deref() == Some("submenu") || !kids.is_empty(),
        children: kids,
    }
}

/// A child node arrives as a variant holding `(ia{sv}av)`.
fn parse_value_node(v: &Value) -> Option<MenuNode> {
    let Value::Structure(s) = unwrap_variant(v) else { return None };
    let f = s.fields();
    if f.len() < 3 {
        return None;
    }
    let id = v_i64(&f[0])? as i32;
    let mut props: HashMap<String, OwnedValue> = HashMap::new();
    if let Value::Dict(d) = unwrap_variant(&f[1]) {
        for (k, val) in d.iter() {
            if let (Some(k), Ok(val)) = (v_str(k), OwnedValue::try_from(unwrap_variant(val).try_clone().ok()?)) {
                props.insert(k, val);
            }
        }
    }
    let children: Vec<OwnedValue> = match unwrap_variant(&f[2]) {
        Value::Array(a) => a.inner().iter().filter_map(|c| c.try_clone().ok().and_then(|c| OwnedValue::try_from(c).ok())).collect(),
        _ => Vec::new(),
    };
    Some(parse_node(id, &props, &children))
}

// ── Actor ────────────────────────────────────────────────────────────────

type Layout = (u32, (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>));

struct Actor {
    conn: Connection,
    dbus: fdo::DBusProxy<'static>,
    state: TrayState,
    core: Arc<WatcherCore>,
    registered_tx: async_channel::Sender<String>,
    /// Item keys whose properties need re-reading.
    dirty: HashSet<String>,
    menu_dirty: bool,
    deadline: Option<Instant>,
    serving: bool,
}

impl Actor {
    async fn emitter_signal_unregistered(&self, address: &str) {
        if let Ok(emitter) = SignalEmitter::new(&self.conn, WATCHER_PATH) {
            let _ = Watcher::status_notifier_item_unregistered(&emitter, address).await;
            if let Ok(iface) = self.conn.object_server().interface::<_, Watcher>(WATCHER_PATH).await {
                let _ = iface.get().await.registered_status_notifier_items_changed(iface.signal_emitter()).await;
            }
        }
    }

    /// Tries to own the watcher name; falls back to registering as a host.
    async fn claim_role(&mut self) {
        if !self.serving {
            let watcher = Watcher { core: self.core.clone(), registered: self.registered_tx.clone() };
            self.serving = self.conn.object_server().at(WATCHER_PATH, watcher).await.unwrap_or(false);
        }
        let reply = self.conn.request_name_with_flags(WATCHER_NAME, fdo::RequestNameFlags::DoNotQueue.into()).await;
        match reply {
            Ok(fdo::RequestNameReply::PrimaryOwner | fdo::RequestNameReply::AlreadyOwner) => {
                tracing::info!("tray: serving {WATCHER_NAME}");
                self.state.role = TrayRole::Watcher;
                if let Ok(emitter) = SignalEmitter::new(&self.conn, WATCHER_PATH) {
                    let _ = Watcher::status_notifier_host_registered(&emitter).await;
                }
                // Items that announced themselves before any watcher existed
                // keep their conventional names: adopt them.
                if let Ok(names) = self.dbus.list_names().await {
                    for name in names {
                        let n = name.as_str();
                        if n.starts_with("org.kde.StatusNotifierItem-") || n.starts_with("org.freedesktop.StatusNotifierItem-") {
                            let address = format!("{n}{DEFAULT_ITEM_PATH}");
                            if self.add_item(&address).await {
                                let fresh = {
                                    let mut items = self.core.items.lock();
                                    let fresh = !items.contains(&address);
                                    if fresh {
                                        items.push(address.clone());
                                    }
                                    fresh
                                };
                                if fresh && let Ok(emitter) = SignalEmitter::new(&self.conn, WATCHER_PATH) {
                                    let _ = Watcher::status_notifier_item_registered(&emitter, &address).await;
                                }
                            }
                        }
                    }
                }
                let known = self.core.items.lock().clone();
                for address in known {
                    self.add_item(&address).await;
                }
            }
            _ => {
                tracing::info!("tray: another StatusNotifierWatcher is running; acting as a host");
                self.state.role = TrayRole::Host;
                let host = format!("org.kde.StatusNotifierHost-{}", std::process::id());
                let _ = self.conn.request_name(host.as_str()).await;
                if let Ok(w) = proxy(&self.conn, WATCHER_NAME, WATCHER_PATH, WATCHER_NAME).await {
                    let _: zbus::Result<()> = w.call("RegisterStatusNotifierHost", &(host.as_str(),)).await;
                    if let Ok(items) = w.get_property::<Vec<String>>("RegisteredStatusNotifierItems").await {
                        for address in items {
                            self.add_item(&address).await;
                        }
                    }
                }
            }
        }
    }

    /// Adds (or refreshes) the item at `address`. Returns whether it exists.
    async fn add_item(&mut self, address: &str) -> bool {
        let (service, path) = split_address(address);
        let owner = if service.starts_with(':') {
            service.clone()
        } else {
            let Ok(name) = BusName::try_from(service.as_str()) else { return false };
            match self.dbus.get_name_owner(name).await {
                Ok(o) => o.to_string(),
                Err(_) => return false,
            }
        };
        // The same object may be known by its well-known and unique name.
        if let Some(existing) = self.state.items.iter().find(|i| i.owner == owner && i.path == path) {
            let key = existing.key.clone();
            self.mark_dirty(key);
            return true;
        }
        let mut item = TrayItem { key: format!("{service}{path}"), service, path, owner, ..Default::default() };
        if !read_item(&self.conn, &mut item).await {
            return false;
        }
        tracing::debug!("tray: + {} ({})", item.display_name(), item.key);
        self.state.items.push(item);
        true
    }

    fn remove_where(&mut self, pred: impl Fn(&TrayItem) -> bool) -> Vec<TrayItem> {
        let (gone, keep): (Vec<TrayItem>, Vec<TrayItem>) = std::mem::take(&mut self.state.items).into_iter().partition(|i| pred(i));
        self.state.items = keep;
        if let Some(menu) = &self.state.menu
            && gone.iter().any(|g| g.key == menu.key)
        {
            self.state.menu = None;
        }
        gone
    }

    fn mark_dirty(&mut self, key: String) {
        self.dirty.insert(key);
        self.deadline.get_or_insert_with(|| Instant::now() + DEBOUNCE);
    }

    fn item(&self, query: &str) -> Result<TrayItem, String> {
        self.state.find(query).cloned().ok_or_else(|| mandara_i18n::tf("No tray item matching \"{query}\"", &[("query", &query)]))
    }

    async fn on_name_owner_changed(&mut self, name: String, new_owner: String) {
        if new_owner.is_empty() {
            let gone = self.remove_where(|i| i.service == name || i.owner == name);
            if self.state.role == TrayRole::Watcher {
                let addresses: Vec<String> = {
                    let mut items = self.core.items.lock();
                    let (drop, keep): (Vec<String>, Vec<String>) = std::mem::take(&mut *items).into_iter().partition(|a| {
                        let (service, path) = split_address(a);
                        service == name || gone.iter().any(|g| g.owner == service && g.path == path)
                    });
                    *items = keep;
                    drop
                };
                for a in addresses {
                    self.emitter_signal_unregistered(&a).await;
                }
                self.core.hosts.lock().retain(|h| *h != name);
            }
            // The watcher we were hosting for went away: take over.
            if name == WATCHER_NAME && self.state.role == TrayRole::Host {
                self.claim_role().await;
            }
        } else if name == WATCHER_NAME && self.state.role == TrayRole::Host {
            // A new watcher replaced the old one: register with it again.
            self.claim_role().await;
        } else if self.state.role == TrayRole::Watcher && (name.starts_with("org.kde.StatusNotifierItem-") || name.starts_with("org.freedesktop.StatusNotifierItem-")) {
            // Give the app a moment to export its object, then adopt it if
            // it never registers itself.
            let address = format!("{name}{DEFAULT_ITEM_PATH}");
            let tx = self.registered_tx.clone();
            crate::runtime::spawn("tray-adopt", async move {
                crate::runtime::sleep(ADOPT_DELAY).await;
                let _ = tx.send(address).await;
            });
        }
    }

    async fn on_signal(&mut self, msg: &Message) {
        let header = msg.header();
        let (Some(iface), Some(member)) = (header.interface(), header.member()) else { return };
        let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
        let path = header.path().map(|p| p.to_string()).unwrap_or_default();
        match (iface.as_str(), member.as_str()) {
            ("org.freedesktop.DBus", "NameOwnerChanged") => {
                if let Ok((name, _old, new)) = msg.body().deserialize::<(String, String, String)>() {
                    self.on_name_owner_changed(name, new).await;
                }
            }
            (KDE_ITEM | FDO_ITEM, m) => {
                let keys: Vec<String> = self.state.items.iter().filter(|i| i.owner == sender && i.path == path).map(|i| i.key.clone()).collect();
                if m == "NewStatus"
                    && let Ok(status) = msg.body().deserialize::<String>()
                {
                    for item in self.state.items.iter_mut().filter(|i| keys.contains(&i.key)) {
                        item.status = ItemStatus::parse(&status);
                    }
                } else {
                    for k in keys {
                        self.mark_dirty(k);
                    }
                }
            }
            (DBUSMENU, "LayoutUpdated" | "ItemsPropertiesUpdated" | "ItemActivationRequested") => {
                let open = self.state.menu.as_ref().and_then(|m| self.state.items.iter().find(|i| i.key == m.key));
                if open.is_some_and(|i| i.owner == sender && i.menu.as_deref() == Some(path.as_str())) {
                    self.menu_dirty = true;
                    self.deadline.get_or_insert_with(|| Instant::now() + DEBOUNCE);
                }
            }
            (WATCHER_NAME, "StatusNotifierItemRegistered") if self.state.role == TrayRole::Host => {
                if let Ok(address) = msg.body().deserialize::<String>() {
                    self.add_item(&address).await;
                }
            }
            (WATCHER_NAME, "StatusNotifierItemUnregistered") if self.state.role == TrayRole::Host => {
                if let Ok(address) = msg.body().deserialize::<String>() {
                    let (service, path) = split_address(&address);
                    self.remove_where(|i| (i.service == service || i.owner == service) && i.path == path);
                }
            }
            _ => {}
        }
    }

    async fn flush_dirty(&mut self) {
        self.deadline = None;
        for key in std::mem::take(&mut self.dirty) {
            let Some(pos) = self.state.items.iter().position(|i| i.key == key) else { continue };
            let mut item = self.state.items[pos].clone();
            if read_item(&self.conn, &mut item).await {
                self.state.items[pos] = item;
            }
        }
        if std::mem::take(&mut self.menu_dirty)
            && let Some(key) = self.state.menu.as_ref().map(|m| m.key.clone())
            && let Ok(item) = self.item(&key)
        {
            match fetch_menu(&self.conn, &item).await {
                Ok(menu) => self.state.menu = Some(menu),
                Err(err) => tracing::debug!("tray: menu refresh failed: {err}"),
            }
        }
    }

    async fn item_call(&self, item: &TrayItem, method: &str, args: &(impl serde::Serialize + zbus::zvariant::DynamicType + Sync)) -> Result<(), String> {
        let p = proxy(&self.conn, &item.owner, &item.path, item.iface).await.map_err(|e| e.to_string())?;
        let result: zbus::Result<()> = p.call(method, args).await;
        result.map_err(|e| e.to_string())
    }

    fn menu_path(item: &TrayItem) -> Result<String, String> {
        item.menu.clone().ok_or_else(|| mandara_i18n::t("This item has no menu").to_string())
    }

    async fn execute(&mut self, command: TrayCommand) -> Result<String, String> {
        match command {
            TrayCommand::Activate { item, x, y } => {
                let item = self.item(&item)?;
                self.item_call(&item, "Activate", &(x, y)).await?;
                Ok(format!("activated {}", item.display_name()))
            }
            TrayCommand::SecondaryActivate { item, x, y } => {
                let item = self.item(&item)?;
                self.item_call(&item, "SecondaryActivate", &(x, y)).await?;
                Ok(format!("secondary-activated {}", item.display_name()))
            }
            TrayCommand::ContextMenu { item, x, y } => {
                let item = self.item(&item)?;
                self.item_call(&item, "ContextMenu", &(x, y)).await?;
                Ok(format!("context menu for {}", item.display_name()))
            }
            TrayCommand::Scroll { item, delta, horizontal } => {
                let item = self.item(&item)?;
                self.item_call(&item, "Scroll", &(delta, if horizontal { "horizontal" } else { "vertical" })).await?;
                Ok(format!("scrolled {}", item.display_name()))
            }
            TrayCommand::OpenMenu(query) => {
                let item = self.item(&query)?;
                if let Some(prev) = self.state.menu.take()
                    && prev.key != item.key
                    && let Ok(old) = self.item(&prev.key)
                {
                    menu_event(&self.conn, &old, 0, "closed").await;
                }
                let menu = fetch_menu(&self.conn, &item).await?;
                menu_event(&self.conn, &item, 0, "opened").await;
                let mut text = format!("{} (revision {})\n", item.display_name(), menu.revision);
                outline(&menu.root, 0, &mut text);
                self.state.menu = Some(menu);
                Ok(text.trim_end().to_string())
            }
            TrayCommand::CloseMenu => {
                if let Some(menu) = self.state.menu.take()
                    && let Ok(item) = self.item(&menu.key)
                {
                    menu_event(&self.conn, &item, 0, "closed").await;
                }
                Ok("menu closed".into())
            }
            TrayCommand::MenuClick { id } => {
                let menu = self.state.menu.clone().ok_or_else(|| mandara_i18n::t("No tray menu is open").to_string())?;
                let item = self.item(&menu.key)?;
                let label = menu.root.find(id).map(|n| n.label.clone()).ok_or_else(|| mandara_i18n::tf("No menu entry {id}", &[("id", &id)]))?;
                let path = Self::menu_path(&item)?;
                let p = proxy(&self.conn, &item.owner, &path, DBUSMENU).await.map_err(|e| e.to_string())?;
                let r: zbus::Result<()> = p.call("Event", &(id, "clicked", Value::from(0i32), 0u32)).await;
                r.map_err(|e| e.to_string())?;
                // Apps update check / radio state right after a click.
                self.menu_dirty = true;
                self.deadline.get_or_insert_with(|| Instant::now() + MENU_SETTLE);
                Ok(format!("clicked \"{label}\""))
            }
            TrayCommand::MenuAboutToShow { id } => {
                let menu = self.state.menu.clone().ok_or_else(|| mandara_i18n::t("No tray menu is open").to_string())?;
                let item = self.item(&menu.key)?;
                let path = Self::menu_path(&item)?;
                let p = proxy(&self.conn, &item.owner, &path, DBUSMENU).await.map_err(|e| e.to_string())?;
                let r: zbus::Result<bool> = p.call("AboutToShow", &(id,)).await;
                menu_event(&self.conn, &item, id, "opened").await;
                if r.unwrap_or(false) {
                    self.state.menu = Some(fetch_menu(&self.conn, &item).await?);
                }
                Ok("ok".into())
            }
        }
    }
}

/// Indented `id  label` listing of a menu tree (CLI output).
fn outline(node: &MenuNode, depth: usize, out: &mut String) {
    use std::fmt::Write as _;
    for c in node.children.iter().filter(|c| c.visible) {
        let indent = "  ".repeat(depth);
        if c.separator {
            let _ = writeln!(out, "{indent}     ────");
            continue;
        }
        let mark = match (c.toggle, c.toggled) {
            (ToggleKind::Checkmark, Some(true)) => "[x] ",
            (ToggleKind::Checkmark, _) => "[ ] ",
            (ToggleKind::Radio, Some(true)) => "(•) ",
            (ToggleKind::Radio, _) => "( ) ",
            _ => "",
        };
        let flags = format!("{}{}", if c.enabled { "" } else { " (disabled)" }, if c.submenu { " ▸" } else { "" });
        let _ = writeln!(out, "{indent}{:>4} {mark}{}{flags}", c.id, c.label);
        outline(c, depth + 1, out);
    }
}

async fn menu_event(conn: &Connection, item: &TrayItem, id: i32, event: &str) {
    let Some(path) = item.menu.as_deref() else { return };
    if let Ok(p) = proxy(conn, &item.owner, path, DBUSMENU).await {
        let _: zbus::Result<()> = p.call("Event", &(id, event, Value::from(0i32), 0u32)).await;
    }
}

/// `AboutToShow(0)` then the full layout tree.
async fn fetch_menu(conn: &Connection, item: &TrayItem) -> Result<TrayMenu, String> {
    let path = item.menu.clone().ok_or_else(|| mandara_i18n::t("This item has no menu").to_string())?;
    let p = proxy(conn, &item.owner, &path, DBUSMENU).await.map_err(|e| e.to_string())?;
    let _: zbus::Result<bool> = p.call("AboutToShow", &(0i32,)).await;
    let (revision, (id, props, children)): Layout = p.call("GetLayout", &(0i32, -1i32, Vec::<String>::new())).await.map_err(|e| e.to_string())?;
    Ok(TrayMenu { key: item.key.clone(), revision, root: parse_node(id, &props, &children) })
}

/// Reads every property of `item`. Returns `false` if the object doesn't
/// answer at all (not an item, or already gone).
async fn read_item(conn: &Connection, item: &mut TrayItem) -> bool {
    let Ok(props_proxy) = proxy(conn, &item.owner, &item.path, "org.freedesktop.DBus.Properties").await else { return false };
    let mut props: HashMap<String, OwnedValue> = HashMap::new();
    let ifaces: &[&'static str] = if item.iface.is_empty() { &[KDE_ITEM, FDO_ITEM] } else if item.iface == KDE_ITEM { &[KDE_ITEM] } else { &[FDO_ITEM] };
    let mut iface = None;
    for i in ifaces {
        let all: zbus::Result<HashMap<String, OwnedValue>> = props_proxy.call("GetAll", &(*i,)).await;
        if let Ok(all) = all
            && !all.is_empty()
        {
            props = all;
            iface = Some(*i);
            break;
        }
    }
    if iface.is_none() {
        // Some implementations (Chromium) fail GetAll when one property is
        // missing; read them one by one.
        for i in ifaces {
            for name in ["Id", "Title", "Category", "Status", "IconName", "IconThemePath", "IconPixmap", "AttentionIconName", "AttentionIconPixmap", "OverlayIconName", "ToolTip", "Menu", "ItemIsMenu"] {
                let v: zbus::Result<OwnedValue> = props_proxy.call("Get", &(*i, name)).await;
                if let Ok(v) = v {
                    props.insert(name.to_string(), v);
                }
            }
            if !props.is_empty() {
                iface = Some(*i);
                break;
            }
        }
    }
    let Some(iface) = iface else { return false };
    item.iface = iface;
    let get = |k: &str| props.get(k).map(|v| &**v);
    let s = |k: &str| get(k).and_then(v_str).unwrap_or_default();
    item.id = s("Id");
    item.title = s("Title");
    item.category = s("Category");
    item.status = ItemStatus::parse(&s("Status"));
    item.icon_name = s("IconName");
    item.icon_theme_path = s("IconThemePath");
    item.icon = get("IconPixmap").and_then(v_pixmap);
    item.attention_icon_name = s("AttentionIconName");
    item.attention_icon = get("AttentionIconPixmap").and_then(v_pixmap);
    item.overlay_icon_name = s("OverlayIconName");
    item.tooltip = get("ToolTip").and_then(v_tooltip);
    item.menu = get("Menu").and_then(v_str).filter(|p| !p.is_empty() && p != "/");
    item.item_is_menu = get("ItemIsMenu").and_then(v_bool).unwrap_or(false);
    true
}

async fn signal_streams(conn: &Connection) -> zbus::Result<futures::stream::SelectAll<MessageStream>> {
    use zbus::message::Type;
    let rules = [
        MatchRule::builder().msg_type(Type::Signal).sender("org.freedesktop.DBus")?.interface("org.freedesktop.DBus")?.member("NameOwnerChanged")?.build(),
        MatchRule::builder().msg_type(Type::Signal).interface(KDE_ITEM)?.build(),
        MatchRule::builder().msg_type(Type::Signal).interface(FDO_ITEM)?.build(),
        MatchRule::builder().msg_type(Type::Signal).interface(DBUSMENU)?.build(),
        MatchRule::builder().msg_type(Type::Signal).interface(WATCHER_NAME)?.build(),
    ];
    let mut streams = Vec::new();
    for rule in rules {
        streams.push(MessageStream::for_match_rule(rule, conn, Some(SIGNAL_QUEUE)).await?);
    }
    Ok(futures::stream::select_all(streams))
}

pub fn spawn_tray_service() -> TrayService {
    let (tx, updates) = async_channel::bounded(UPDATE_QUEUE);
    let (commands, rx) = async_channel::bounded::<Envelope<TrayCommand>>(COMMAND_QUEUE);
    crate::runtime::spawn("tray", async move {
        if let Err(err) = run(tx.clone(), rx).await {
            tracing::warn!("tray: {err:#}");
            let _ = tx.send(TrayState::default()).await;
        }
    });
    ServiceHandle::new(updates, commands)
}

async fn run(tx: async_channel::Sender<TrayState>, rx: async_channel::Receiver<Envelope<TrayCommand>>) -> anyhow::Result<()> {
    // Private connection: it owns the watcher name, serves its object and
    // carries busy signal streams.
    let conn = crate::runtime::connect(zbus::connection::Builder::session()?).await?;
    let dbus = fdo::DBusProxy::new(&conn).await?;
    let mut signals = signal_streams(&conn).await?;
    let (registered_tx, registered_rx) = async_channel::bounded::<String>(COMMAND_QUEUE);
    let mut actor = Actor {
        conn,
        dbus,
        state: TrayState::default(),
        core: Arc::new(WatcherCore::default()),
        registered_tx,
        dirty: HashSet::new(),
        menu_dirty: false,
        deadline: None,
        serving: false,
    };
    actor.claim_role().await;
    let mut last: Option<TrayState> = None;
    loop {
        if last.as_ref() != Some(&actor.state) {
            if tx.send(actor.state.clone()).await.is_err() {
                return Ok(());
            }
            last = Some(actor.state.clone());
        }
        let wait = actor.deadline.map(|d| d.saturating_duration_since(Instant::now())).unwrap_or(IDLE_WAIT);
        futures::select! {
            cmd = rx.recv().fuse() => match cmd {
                Ok(Envelope { command, reply: r }) => {
                    let result = actor.execute(command).await;
                    respond(&r, result);
                }
                Err(_) => return Ok(()),
            },
            msg = signals.next().fuse() => match msg {
                Some(Ok(msg)) => actor.on_signal(&msg).await,
                Some(Err(err)) => tracing::debug!("tray: signal stream: {err}"),
                None => return Ok(()),
            },
            address = registered_rx.recv().fuse() => {
                if let Ok(address) = address {
                    actor.add_item(&address).await;
                }
            },
            _ = futures::FutureExt::fuse(async_io::Timer::after(wait)) => {
                if actor.deadline.is_some_and(|d| d <= Instant::now()) {
                    actor.flush_dirty().await;
                }
            },
        }
    }
}

fn respond(r: &Option<Reply>, result: Result<String, String>) {
    reply(r, result);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(item_address("org.kde.StatusNotifierItem-1-1", ":1.5"), "org.kde.StatusNotifierItem-1-1/StatusNotifierItem");
        assert_eq!(item_address("/org/ayatana/NotificationItem/x", ":1.5"), ":1.5/org/ayatana/NotificationItem/x");
        assert_eq!(item_address(":1.9/StatusNotifierItem", ":1.5"), ":1.9/StatusNotifierItem");
        assert_eq!(split_address(":1.5/org/ayatana/x"), (":1.5".into(), "/org/ayatana/x".into()));
        assert_eq!(split_address("org.x"), ("org.x".into(), "/StatusNotifierItem".into()));
    }

    #[test]
    fn mnemonics_and_names() {
        assert_eq!(strip_mnemonic("_Quit"), "Quit");
        assert_eq!(strip_mnemonic("Save__As"), "Save_As");
        assert_eq!(prettify_id("Docker Desktop_status_icon_1"), "Docker Desktop");
        let item = TrayItem { id: "nm-applet".into(), ..Default::default() };
        assert_eq!(item.display_name(), "nm-applet");
        assert!(item.matches("NM-APPLET"));
    }

    #[test]
    fn argb_to_rgba() {
        let p = Pixmap::from_argb(1, 1, &[0x80, 0x10, 0x20, 0x30]).unwrap();
        assert_eq!(&*p.rgba, &[0x10, 0x20, 0x30, 0x80]);
        assert!(Pixmap::from_argb(2, 2, &[0; 4]).is_none());
    }

    #[test]
    fn parses_layout_nodes() {
        let mut props = HashMap::new();
        props.insert("label".to_string(), OwnedValue::try_from(Value::from("_Open")).unwrap());
        props.insert("toggle-type".to_string(), OwnedValue::try_from(Value::from("checkmark")).unwrap());
        props.insert("toggle-state".to_string(), OwnedValue::from(1i32));
        props.insert("enabled".to_string(), OwnedValue::from(false));
        let node = parse_node(7, &props, &[]);
        assert_eq!(node.label, "Open");
        assert_eq!(node.toggle, ToggleKind::Checkmark);
        assert_eq!(node.toggled, Some(true));
        assert!(!node.enabled && node.visible && !node.submenu);
    }
}
