//! The extension platform's policy, independent of any GUI: the API level
//! and features a host offers, the capabilities a plugin may ask for (and
//! the person grants and revokes), the outlets plugin.toml declares
//! (launcher providers, Studio tabs, served operations) and the limits on
//! views, payloads and queues. `wasm.rs` enforces them at the boundary and
//! the shell's brokers check them again before acting.
//!
//! Capabilities are narrow and namespaced. A plugin declares each in
//! `permissions`; declaring grants nothing. The person grants and revokes
//! each one in Studio (`[plugins.grants.<id>]`), and every request is
//! checked against the live grants when it arrives.

use std::fmt;
use std::time::Duration;

use mandara_i18n::tk;
use serde::{Deserialize, Serialize};

use crate::wasm::{Kind, Node, View};

/// The extension API level this host speaks (`platform.host-info.api`).
pub const API_LEVEL: u32 = 1;

/// Every feature this host supports. A plugin's `negotiate` names the
/// ones it uses; loading stops when one is missing, so it never
/// half-works on an older Mandara.
pub const FEATURES: &[&str] = &[
    "ui.layout", "ui.input", "ui.toggle", "ui.slider", "ui.choice", "ui.grid", "ui.scroll", "ui.list", "ui.image", "ui.canvas", "ui.keys", "launcher", "studio-tabs", "broker", "plugin-calls",
];

/// Features a plugin asks for that this host lacks.
pub fn missing_features(wanted: &[String]) -> Vec<&str> {
    wanted.iter().map(String::as_str).filter(|f| !FEATURES.contains(f)).collect()
}

// ── Limits ───────────────────────────────────────────────────────────────

/// Most nodes one view may have; a larger view is refused, not truncated.
pub const MAX_VIEW_NODES: usize = 2048;
/// Most bytes of text one view may carry (every string in it together).
pub const MAX_VIEW_BYTES: usize = 256 * 1024;
/// Most canvas marks in one view.
pub const MAX_VIEW_MARKS: usize = 2048;
/// Most options one choice may offer.
pub const MAX_CHOICE_OPTIONS: usize = 32;
/// Most rows a virtual list may declare.
pub const MAX_LIST_ROWS: u32 = 100_000;
/// Largest request payload (JSON) a plugin may submit.
pub const MAX_PAYLOAD_BYTES: usize = 16 * 1024;
/// Largest reply payload delivered to a plugin (a larger one is an error).
pub const MAX_REPLY_BYTES: usize = 256 * 1024;
/// Requests one plugin may have waiting or running at once (open
/// subscriptions don't count; they have their own limit).
pub const MAX_PENDING_REQUESTS: usize = 8;
/// Open subscriptions per plugin.
pub const MAX_SUBSCRIPTIONS: usize = 8;
/// Replies held for a plugin before the oldest are dropped.
pub const MAX_INBOX: usize = 32;
/// Image handles one plugin instance may hold; the oldest is released
/// when a new one is issued past it.
pub const MAX_IMAGE_HANDLES: usize = 256;
/// Most results one launcher query returns.
pub const MAX_HITS: usize = 24;
/// Most launcher providers, Studio tabs and served operations one plugin
/// may declare.
pub const MAX_LAUNCHER_PROVIDERS: usize = 4;
pub const MAX_STUDIO_TABS: usize = 4;
pub const MAX_SERVED_OPERATIONS: usize = 16;
/// Launcher queries run on every keystroke, so they get a tighter time
/// budget than other calls (running past it disables the plugin).
pub const LAUNCHER_BUDGET: Duration = Duration::from_millis(30);
/// Longest outlet, provider, capability and operation names.
pub const MAX_NAME_LEN: usize = 64;

// ── Capabilities ─────────────────────────────────────────────────────────

/// Authority over something outside the plugin. Parsed from the strings in
/// plugin.toml's `permissions` and in broker requests.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Capability {
    /// `commands`: queue Mandara's interface commands (the v0.1
    /// permission; declaring it is consent, as before).
    Commands,
    /// `media.read`: what's playing (title, artist, album, position,
    /// state) and its cover, as an image handle.
    MediaRead,
    /// `media.control`: play, pause, skip, seek and the player's volume.
    MediaControl,
    /// `wallpaper.read`: the current wallpaper and the images in the
    /// person's wallpaper folder, as names and image handles.
    WallpaperRead,
    /// `wallpaper.set`: set a wallpaper from an image handle the host
    /// issued for a wallpaper or a granted folder.
    WallpaperSet,
    /// `files.folders`: the images in folders the person chose for this
    /// plugin in Studio (names and image handles, never paths).
    Folders,
    /// `notifications.send`: post a notification.
    Notify,
    /// `windows.read`: the open windows (app, title, workspace) and which
    /// one has focus.
    WindowsRead,
    /// `windows.control`: focus or close a window (closing needs a click
    /// or key press in the plugin first).
    WindowsControl,
    /// `workspaces.read`: the workspaces or tags and which are shown.
    WorkspacesRead,
    /// `workspaces.control`: switch to a workspace or tag.
    WorkspacesControl,
    /// `system.read`: processor, memory, temperature and battery.
    SystemRead,
    /// `clipboard.read`: the latest copied text, only right after a click
    /// or key press in the plugin.
    ClipboardRead,
    /// `clipboard.write`: copy text.
    ClipboardWrite,
    /// `open.web`: open an https:// page in the browser, only right after a
    /// click or key press in the plugin.
    OpenWeb,
    /// `osd.show`: show an on-screen display.
    Osd,
    /// `theme.control`: switch between light and dark.
    ThemeControl,
    /// `events`: hear about desktop changes (theme, wallpaper; windows,
    /// workspaces and power with their read capabilities).
    Events,
    /// `timers`: wake up later or every so often (no grant needed; the
    /// plugin's own time).
    Timers,
    /// `launcher.global`: see every launcher search (plain searches, not
    /// only ones typed after its prefix) and add results to them.
    LauncherGlobal,
    /// `net:<host>`: HTTPS requests to exactly this host.
    Net(String),
    /// `plugins.call:<id>`: call the operations plugin `<id>` serves.
    Call(String),
}

impl Capability {
    pub const COMMANDS: &'static str = "commands";

    /// Parses a capability name; the error explains what's wrong.
    pub fn parse(s: &str) -> Result<Self, String> {
        Ok(match s {
            Self::COMMANDS => Self::Commands,
            "media.read" => Self::MediaRead,
            "media.control" => Self::MediaControl,
            "wallpaper.read" => Self::WallpaperRead,
            "wallpaper.set" => Self::WallpaperSet,
            "files.folders" => Self::Folders,
            "notifications.send" => Self::Notify,
            "windows.read" => Self::WindowsRead,
            "windows.control" => Self::WindowsControl,
            "workspaces.read" => Self::WorkspacesRead,
            "workspaces.control" => Self::WorkspacesControl,
            "system.read" => Self::SystemRead,
            "clipboard.read" => Self::ClipboardRead,
            "clipboard.write" => Self::ClipboardWrite,
            "open.web" => Self::OpenWeb,
            "osd.show" => Self::Osd,
            "theme.control" => Self::ThemeControl,
            "events" => Self::Events,
            "timers" => Self::Timers,
            "launcher.global" => Self::LauncherGlobal,
            _ => {
                if let Some(host) = s.strip_prefix("net:") {
                    return valid_host(host).then(|| Self::Net(host.to_ascii_lowercase())).ok_or_else(|| format!("`{s}`: name one host, such as net:api.example.com (no scheme, path, port or wildcard)"));
                }
                if let Some(id) = s.strip_prefix("plugins.call:") {
                    return crate::wasm::valid_id(id).then(|| Self::Call(id.to_string())).ok_or_else(|| format!("`{s}`: name a plugin id after plugins.call:"));
                }
                return Err(format!("unknown permission `{s}`"));
            }
        })
    }

    /// Whether the person must grant it in Studio. `commands` keeps its
    /// v0.1 meaning: declared means allowed.
    pub fn needs_grant(&self) -> bool {
        !matches!(self, Self::Commands | Self::Timers)
    }

    /// Operations that only go ahead right after a click, key press, typed
    /// text, launcher choice or command in the plugin (user activation), so
    /// a plugin can't do them on its own in the background.
    pub fn needs_activation(&self, operation: &str) -> bool {
        matches!((self, operation), (Self::ClipboardRead, "text") | (Self::OpenWeb, "open") | (Self::WallpaperSet, "apply") | (Self::WindowsControl, "close") | (Self::Folders, "choose"))
    }

    /// The operations a request with this capability may name.
    pub fn operations(&self) -> &'static [&'static str] {
        match self {
            Self::Commands => &[],
            Self::MediaRead => &["status", "subscribe"],
            Self::MediaControl => &["play-pause", "next", "previous", "seek", "volume"],
            Self::WallpaperRead => &["current", "list"],
            Self::WallpaperSet => &["apply"],
            Self::Folders => &["folders", "list", "choose"],
            Self::Notify => &["send"],
            Self::WindowsRead => &["list", "active"],
            Self::WindowsControl => &["focus", "close"],
            Self::WorkspacesRead => &["list"],
            Self::WorkspacesControl => &["view"],
            Self::SystemRead => &["resources", "power"],
            Self::ClipboardRead => &["text"],
            Self::ClipboardWrite => &["text"],
            Self::OpenWeb => &["open"],
            Self::Osd => &["show"],
            Self::ThemeControl => &["mode"],
            Self::Events => &["subscribe"],
            Self::Timers => &["after", "every"],
            // Not a broker service: it lets global [[launcher]] providers see
            // plain searches.
            Self::LauncherGlobal => &[],
            Self::Net(_) => &["fetch"],
            // The callee's `serves` list decides.
            Self::Call(_) => &[],
        }
    }

    /// What it allows, in English (a `tk` key: Studio translates it).
    pub fn describe(&self) -> &'static str {
        match self {
            Self::Commands => tk("Run Mandara's interface commands (open panels, control media and volume, post notifications)"),
            Self::MediaRead => tk("See what's playing and its cover"),
            Self::MediaControl => tk("Play, pause, skip and seek media"),
            Self::WallpaperRead => tk("See your wallpapers"),
            Self::WallpaperSet => tk("Change the wallpaper"),
            Self::Folders => tk("See the images in folders you choose for it"),
            Self::Notify => tk("Send notifications"),
            Self::WindowsRead => tk("See your open windows"),
            Self::WindowsControl => tk("Focus and close windows"),
            Self::WorkspacesRead => tk("See your workspaces"),
            Self::WorkspacesControl => tk("Switch workspaces"),
            Self::SystemRead => tk("See processor, memory and battery use"),
            Self::ClipboardRead => tk("Read the text you copied, when you use it"),
            Self::ClipboardWrite => tk("Copy text"),
            Self::OpenWeb => tk("Open web pages in your browser, when you use it"),
            Self::Osd => tk("Show on-screen displays"),
            Self::ThemeControl => tk("Switch between light and dark"),
            Self::Events => tk("Hear about desktop changes"),
            Self::Timers => tk("Wake up on a timer"),
            Self::LauncherGlobal => tk("See everything you search for in the launcher"),
            Self::Net(_) => tk("Connect to {host}"),
            Self::Call(_) => tk("Talk to the {plugin} plugin"),
        }
    }

    /// The placeholder value [`Self::describe`] names, if any.
    pub fn target(&self) -> Option<(&'static str, &str)> {
        match self {
            Self::Net(host) => Some(("host", host)),
            Self::Call(id) => Some(("plugin", id)),
            _ => None,
        }
    }

    /// The icon Studio shows beside it.
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Commands => "terminal",
            Self::MediaRead | Self::MediaControl => "music",
            Self::WallpaperRead | Self::WallpaperSet => "image",
            Self::Folders => "folder",
            Self::Notify => "bell",
            Self::WindowsRead | Self::WindowsControl => "window",
            Self::WorkspacesRead | Self::WorkspacesControl => "grid",
            Self::SystemRead => "cpu",
            Self::ClipboardRead | Self::ClipboardWrite => "clipboard",
            Self::OpenWeb => "external",
            Self::Osd => "info",
            Self::ThemeControl => "sun",
            Self::Events => "zap",
            Self::Timers => "timer",
            Self::LauncherGlobal => "search",
            Self::Net(_) => "globe",
            Self::Call(_) => "plug",
        }
    }

    /// Whether a subscription operation (replies keep coming until cancelled).
    pub fn is_subscription(&self, operation: &str) -> bool {
        matches!((self, operation), (Self::MediaRead, "subscribe") | (Self::Events, "subscribe") | (Self::Timers, "every"))
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Commands => f.write_str(Self::COMMANDS),
            Self::MediaRead => f.write_str("media.read"),
            Self::MediaControl => f.write_str("media.control"),
            Self::WallpaperRead => f.write_str("wallpaper.read"),
            Self::WallpaperSet => f.write_str("wallpaper.set"),
            Self::Folders => f.write_str("files.folders"),
            Self::Notify => f.write_str("notifications.send"),
            Self::WindowsRead => f.write_str("windows.read"),
            Self::WindowsControl => f.write_str("windows.control"),
            Self::WorkspacesRead => f.write_str("workspaces.read"),
            Self::WorkspacesControl => f.write_str("workspaces.control"),
            Self::SystemRead => f.write_str("system.read"),
            Self::ClipboardRead => f.write_str("clipboard.read"),
            Self::ClipboardWrite => f.write_str("clipboard.write"),
            Self::OpenWeb => f.write_str("open.web"),
            Self::Osd => f.write_str("osd.show"),
            Self::ThemeControl => f.write_str("theme.control"),
            Self::Events => f.write_str("events"),
            Self::Timers => f.write_str("timers"),
            Self::LauncherGlobal => f.write_str("launcher.global"),
            Self::Net(host) => write!(f, "net:{host}"),
            Self::Call(id) => write!(f, "plugins.call:{id}"),
        }
    }
}

/// Topics `events` `subscribe` takes, and the read capability each needs
/// besides `events` (none for theme and wallpaper changes).
pub const EVENT_TOPICS: &[(&str, Option<Capability>)] = &[("theme", None), ("wallpaper", None), ("windows", Some(Capability::WindowsRead)), ("workspaces", Some(Capability::WorkspacesRead)), ("power", Some(Capability::SystemRead))];

/// Timer bounds: a one-shot timer waits 5 s to 1 day; a repeating one at
/// least a minute (plugins don't poll).
pub const TIMER_MIN_MS: u64 = 5000;
pub const TIMER_MAX_MS: u64 = 86_400_000;
pub const TIMER_EVERY_MIN_MS: u64 = 60_000;

// ── Private storage ──────────────────────────────────────────────────────

/// A plugin's private key-value storage: longest key and value, most keys
/// and most bytes in all. Kept in `$XDG_DATA_HOME/mandara/plugin-storage/<id>.json`.
pub const MAX_STORAGE_KEY: usize = 128;
pub const MAX_STORAGE_VALUE: usize = 64 * 1024;
pub const MAX_STORAGE_KEYS: usize = 1024;
pub const MAX_STORAGE_BYTES: usize = 1024 * 1024;

// ── Commands ─────────────────────────────────────────────────────────────

/// A command the plugin offers (`[[commands]]`): run as `mandara msg plugins
/// run <plugin> <name> [args…]`, from compositor shortcuts, hooks, skills
/// and agents; the plugin gets a `command` event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManifestCommand {
    pub name: String,
    #[serde(default)]
    pub summary: String,
}

/// Most commands one plugin may offer, and most words after the name.
pub const MAX_COMMANDS: usize = 16;
pub const MAX_COMMAND_ARGS: usize = 8;

/// Valid commands and problems with the rest.
pub fn split_commands(items: Vec<ManifestCommand>) -> (Vec<ManifestCommand>, Vec<String>) {
    let (mut valid, mut problems): (Vec<ManifestCommand>, Vec<String>) = (Vec::new(), Vec::new());
    for c in items {
        if !valid_name(&c.name) {
            problems.push(format!("command `{}` must be lowercase letters, digits or -", c.name));
        } else if valid.iter().any(|v| v.name == c.name) {
            problems.push(format!("duplicate command `{}` ignored", c.name));
        } else if valid.len() >= MAX_COMMANDS {
            problems.push(format!("command `{}` ignored: at most {MAX_COMMANDS}", c.name));
        } else {
            valid.push(c);
        }
    }
    (valid, problems)
}

/// A DNS host name: letters, digits, `-` and dots, at least one dot, no
/// leading or trailing dot or dash in a label, and not an IP literal (a
/// plugin names the service it talks to, not an address on your network).
pub fn valid_host(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    host.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        && !labels.last().is_some_and(|tld| tld.chars().all(|c| c.is_ascii_digit()))
        && !host.eq_ignore_ascii_case("localhost")
        && !labels.last().is_some_and(|tld| tld.eq_ignore_ascii_case("localhost") || tld.eq_ignore_ascii_case("local") || tld.eq_ignore_ascii_case("internal"))
}

/// The declared capabilities (valid ones) and problems with the rest.
pub fn split_capabilities(permissions: &[String]) -> (Vec<Capability>, Vec<String>) {
    let (mut valid, mut problems) = (Vec::new(), Vec::new());
    for p in permissions {
        match Capability::parse(p) {
            Ok(c) if valid.contains(&c) => problems.push(format!("permission `{p}` is listed twice")),
            Ok(c) => valid.push(c),
            Err(e) => problems.push(e),
        }
    }
    (valid, problems)
}

/// What one plugin may do right now: what it declared, and of that, what
/// the person granted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Access {
    pub declared: Vec<Capability>,
    pub granted: Vec<Capability>,
    /// Folders granted for `files.folders` (empty unless it is granted).
    pub folders: Vec<std::path::PathBuf>,
}

impl Access {
    /// From plugin.toml's `permissions` and the person's grants
    /// (`[plugins.grants.<id>].capabilities`). A grant for something the
    /// plugin doesn't declare is ignored; `commands` needs no grant.
    pub fn new(permissions: &[String], grants: &[String]) -> Self {
        let declared = split_capabilities(permissions).0;
        let granted = declared.iter().filter(|c| !c.needs_grant() || grants.iter().any(|g| Capability::parse(g).is_ok_and(|g| g == **c))).cloned().collect();
        Self { declared, granted, folders: Vec::new() }
    }

    /// [`Self::new`] from a plugin's config grant, folders included.
    pub fn from_grant(permissions: &[String], grant: Option<&Grant>) -> Self {
        let mut access = Self::new(permissions, grant.map(|g| g.capabilities.as_slice()).unwrap_or_default());
        if access.allows(&Capability::Folders) {
            access.folders = grant.map(|g| g.folders.clone()).unwrap_or_default();
        }
        access
    }

    /// Whether a folder is granted right now.
    pub fn allows_folder(&self, folder: &std::path::Path) -> bool {
        self.allows(&Capability::Folders) && self.folders.iter().any(|f| f == folder)
    }

    /// Only `commands`, as v0.1 plugins have it (vet and tests).
    pub fn commands(allow: bool) -> Self {
        let c = if allow { vec![Capability::Commands] } else { Vec::new() };
        Self { declared: c.clone(), granted: c, folders: Vec::new() }
    }

    pub fn allows(&self, c: &Capability) -> bool {
        self.granted.contains(c)
    }

    pub fn declares(&self, c: &Capability) -> bool {
        self.declared.contains(c)
    }

    /// Why a request for `c` with `operation` can't go ahead, if it can't.
    /// `activated`: the plugin is handling a click, key press, typed text,
    /// launcher choice or command right now.
    pub fn refuse_now(&self, c: &Capability, operation: &str, activated: bool) -> Option<String> {
        if let Some(why) = self.refuse(c, operation) {
            return Some(why);
        }
        (c.needs_activation(operation) && !activated).then(|| format!("`{c}` `{operation}` only works right after a click or key press in the plugin"))
    }

    /// [`Self::refuse_now`] without the activation rule (the shell's second
    /// check, which runs after the plugin's call returned).
    pub fn refuse(&self, c: &Capability, operation: &str) -> Option<String> {
        if matches!(c, Capability::Commands) {
            return Some("use host::command for commands".into());
        }
        if !self.declares(c) {
            return Some(format!("`{c}` isn't in this plugin's permissions"));
        }
        if !self.allows(c) {
            return Some(format!("`{c}` hasn't been granted (Studio → Plugins → this plugin → Permissions)"));
        }
        if !matches!(c, Capability::Call(_)) && !c.operations().contains(&operation) {
            return Some(format!("`{c}` has no operation `{operation}`"));
        }
        if operation.is_empty() || operation.len() > MAX_NAME_LEN {
            return Some("the operation name is empty or too long".into());
        }
        None
    }
}

/// The person's grants for one plugin (`[plugins.grants.<id>]`).
pub use mandara_config::PluginGrant as Grant;

/// The capability an extension plugin also needs for an interface command
/// it queues with `host::command` (the v0.1 `commands` permission must not
/// get around a capability the person revoked). `None`: `commands` alone.
pub fn command_capability(path: &str) -> Option<Capability> {
    Some(match path {
        p if p.starts_with("media ") => Capability::MediaControl,
        "notifications send" => Capability::Notify,
        "theme mode" => Capability::ThemeControl,
        "workspace view" | "workspace toggle" | "workspace cycle-layout" => Capability::WorkspacesControl,
        "osd" | "osd show" => Capability::Osd,
        // Searching runs every provider, other plugins' included.
        "launcher search" => Capability::LauncherGlobal,
        _ => return None,
    })
}

// ── Outlets in plugin.toml ───────────────────────────────────────────────

/// A launcher provider (`[[launcher]]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManifestLauncher {
    /// Unique within the plugin (`launcher-query` gets it).
    pub id: String,
    /// Shown in the launcher's hints and Studio.
    pub name: String,
    /// Typed first, it searches only this provider (`moon `).
    #[serde(default)]
    pub prefix: String,
    /// Also mixed into plain searches.
    #[serde(default)]
    pub global: bool,
    /// A bundled Mandara icon for its results.
    #[serde(default = "default_outlet_icon")]
    pub icon: String,
}

/// A tab in a Studio page's tab bar (`[[studio-tabs]]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManifestStudioTab {
    /// The page: one of [`STUDIO_TAB_PAGES`].
    pub page: String,
    /// Unique within the plugin (the `outlet` of its views).
    pub id: String,
    pub title: String,
    #[serde(default = "default_outlet_icon")]
    pub icon: String,
}

fn default_outlet_icon() -> String {
    "plug".into()
}

/// The plugin's panel size (`[panel]`), logical px; clamped to
/// [`PANEL_WIDTHS`] and [`PANEL_HEIGHTS`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManifestPanel {
    #[serde(default = "default_panel_width")]
    pub width: f32,
    #[serde(default = "default_panel_height")]
    pub height: f32,
}

/// The panel size a plugin gets without `[panel]` (and v0.1 plugins).
pub const DEFAULT_PANEL: ManifestPanel = ManifestPanel { width: 360.0, height: 380.0 };
/// Panel sizes a plugin may ask for.
pub const PANEL_WIDTHS: std::ops::RangeInclusive<f32> = 280.0..=720.0;
pub const PANEL_HEIGHTS: std::ops::RangeInclusive<f32> = 200.0..=880.0;

fn default_panel_width() -> f32 {
    DEFAULT_PANEL.width
}

fn default_panel_height() -> f32 {
    DEFAULT_PANEL.height
}

impl ManifestPanel {
    /// The size, clamped to what Mandara allows.
    pub fn clamped(self) -> Self {
        let fit = |v: f32, r: &std::ops::RangeInclusive<f32>, d: f32| if v.is_finite() { v.clamp(*r.start(), *r.end()) } else { d };
        Self { width: fit(self.width, &PANEL_WIDTHS, DEFAULT_PANEL.width), height: fit(self.height, &PANEL_HEIGHTS, DEFAULT_PANEL.height) }
    }
}

/// The Studio pages whose tab bars take plugin tabs.
pub const STUDIO_TAB_PAGES: &[&str] = &["themes", "bars", "desktop", "wallpaper"];

/// Outlet and operation names: 1–64 lowercase letters, digits and `-`.
pub fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_NAME_LEN && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Valid launcher providers and problems with the rest.
pub fn split_launchers(items: Vec<ManifestLauncher>) -> (Vec<ManifestLauncher>, Vec<String>) {
    let (mut valid, mut problems): (Vec<ManifestLauncher>, Vec<String>) = (Vec::new(), Vec::new());
    for l in items {
        if !valid_name(&l.id) {
            problems.push(format!("launcher provider id `{}` must be lowercase letters, digits or -", l.id));
        } else if valid.iter().any(|v| v.id == l.id) {
            problems.push(format!("duplicate launcher provider `{}` ignored", l.id));
        } else if valid.len() >= MAX_LAUNCHER_PROVIDERS {
            problems.push(format!("launcher provider `{}` ignored: at most {MAX_LAUNCHER_PROVIDERS}", l.id));
        } else if l.name.trim().is_empty() {
            problems.push(format!("launcher provider `{}` has no name", l.id));
        } else if l.prefix.trim().is_empty() && !l.global {
            problems.push(format!("launcher provider `{}` needs a prefix or global = true", l.id));
        } else if l.prefix.chars().count() > MAX_PREFIX_CHARS || l.prefix.starts_with(['/', '=', '>', '!']) {
            problems.push(format!("launcher provider `{}`: the prefix must be at most {MAX_PREFIX_CHARS} characters and not start with / = > or !", l.id));
        } else {
            valid.push(l);
        }
    }
    (valid, problems)
}

/// Longest launcher prefix.
pub const MAX_PREFIX_CHARS: usize = 12;

/// Valid Studio tabs and problems with the rest.
pub fn split_studio_tabs(items: Vec<ManifestStudioTab>) -> (Vec<ManifestStudioTab>, Vec<String>) {
    let (mut valid, mut problems): (Vec<ManifestStudioTab>, Vec<String>) = (Vec::new(), Vec::new());
    for tab in items {
        if !valid_name(&tab.id) {
            problems.push(format!("Studio tab id `{}` must be lowercase letters, digits or -", tab.id));
        } else if !STUDIO_TAB_PAGES.contains(&tab.page.as_str()) {
            problems.push(format!("Studio tab `{}`: page `{}` takes no tabs (one of {})", tab.id, tab.page, STUDIO_TAB_PAGES.join(", ")));
        } else if valid.iter().any(|v| v.id == tab.id) {
            problems.push(format!("duplicate Studio tab `{}` ignored", tab.id));
        } else if valid.len() >= MAX_STUDIO_TABS {
            problems.push(format!("Studio tab `{}` ignored: at most {MAX_STUDIO_TABS}", tab.id));
        } else if tab.title.trim().is_empty() {
            problems.push(format!("Studio tab `{}` has no title", tab.id));
        } else {
            valid.push(tab);
        }
    }
    (valid, problems)
}

/// Valid served operations and problems with the rest.
pub fn split_serves(items: Vec<String>) -> (Vec<String>, Vec<String>) {
    let (mut valid, mut problems): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    for op in items {
        if !valid_name(&op) {
            problems.push(format!("served operation `{op}` must be lowercase letters, digits or -"));
        } else if valid.contains(&op) {
            problems.push(format!("served operation `{op}` is listed twice"));
        } else if valid.len() >= MAX_SERVED_OPERATIONS {
            problems.push(format!("served operation `{op}` ignored: at most {MAX_SERVED_OPERATIONS}"));
        } else {
            valid.push(op);
        }
    }
    (valid, problems)
}

/// An http(s) URL Studio may open for "Source code" / "Report an issue".
pub fn valid_link(url: &str) -> bool {
    url.len() <= 512 && (url.starts_with("https://") || url.starts_with("http://")) && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Where a plugin's source lives: `repository`, else `homepage` when it is
/// a code host's repository page.
pub fn source_url(repository: &str, homepage: &str) -> Option<String> {
    if valid_link(repository) {
        return Some(repository.trim_end_matches('/').to_string());
    }
    github_repo(homepage).map(|r| format!("https://github.com/{r}"))
}

/// Where to report an issue: `support`, else the GitHub repository's
/// issue tracker.
pub fn issues_url(support: &str, repository: &str, homepage: &str) -> Option<String> {
    if valid_link(support) {
        return Some(support.to_string());
    }
    [repository, homepage].into_iter().find_map(github_repo).map(|r| format!("https://github.com/{r}/issues"))
}

/// `owner/repo` of a `https://github.com/owner/repo[/…]` URL.
pub fn github_repo(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://github.com/")?;
    let mut parts = rest.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?.trim_end_matches(".git"));
    let ok = |s: &str| !s.is_empty() && s != "." && s != ".." && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    (ok(owner) && ok(repo)).then(|| format!("{owner}/{repo}"))
}

// ── View checks ──────────────────────────────────────────────────────────

/// Why a view is refused, if it is: too many nodes, marks or bytes; a
/// child index out of range; or a list declaring too many rows. Checked
/// before anything is drawn, so a hostile tree costs one linear scan.
pub fn check_view(view: &View) -> Result<(), String> {
    if view.nodes.len() > MAX_VIEW_NODES {
        return Err(format!("the view has {} nodes (at most {MAX_VIEW_NODES})", view.nodes.len()));
    }
    if view.root as usize >= view.nodes.len() {
        return Err("the view's root isn't one of its nodes".into());
    }
    let (mut bytes, mut marks, mut edges) = (view.focus.len(), 0usize, 0usize);
    // A tree: every node has at most one parent and the root has none, so
    // nothing is drawn twice and no cycle is reachable from the root.
    let mut parent = vec![false; view.nodes.len()];
    for n in &view.nodes {
        bytes += node_bytes(n);
        marks += n.marks.len();
        edges += n.children.len();
        if edges > MAX_VIEW_NODES {
            return Err(format!("the view lists more than {MAX_VIEW_NODES} children"));
        }
        for &c in &n.children {
            let Some(seen) = parent.get_mut(c as usize) else { return Err("a node lists a child that isn't in the view".into()) };
            if std::mem::replace(seen, true) || c == view.root {
                return Err("a node is listed as a child twice (views are trees)".into());
            }
        }
        if n.options.len() > MAX_CHOICE_OPTIONS {
            return Err(format!("a choice has {} options (at most {MAX_CHOICE_OPTIONS})", n.options.len()));
        }
        if n.kind == Kind::List && n.index > MAX_LIST_ROWS {
            return Err(format!("a list declares {} rows (at most {MAX_LIST_ROWS})", n.index));
        }
    }
    if bytes > MAX_VIEW_BYTES {
        return Err(format!("the view carries {bytes} bytes of text (at most {MAX_VIEW_BYTES})"));
    }
    if marks > MAX_VIEW_MARKS {
        return Err(format!("the view has {marks} canvas marks (at most {MAX_VIEW_MARKS})"));
    }
    Ok(())
}

fn node_bytes(n: &Node) -> usize {
    n.id.len() + n.text.len() + n.hint.len() + n.action.len() + n.options.iter().map(|o| o.value.len() + o.label.len()).sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_round_trip_and_reject_wide_authority() {
        for s in ["commands", "media.read", "media.control", "wallpaper.read", "wallpaper.set", "files.folders", "notifications.send", "windows.read", "windows.control", "workspaces.read", "workspaces.control", "system.read", "clipboard.read", "clipboard.write", "open.web", "osd.show", "theme.control", "events", "timers", "launcher.global", "net:api.example.com", "plugins.call:moon-phase"] {
            assert_eq!(Capability::parse(s).unwrap().to_string(), s);
        }
        for bad in ["net:*", "net:*.example.com", "net:https://example.com", "net:example.com:443", "net:example.com/path", "net:localhost", "net:192.168.1.1", "net:printer.local", "net:example", "plugins.call:Bad Id", "files", "media", ""] {
            assert!(Capability::parse(bad).is_err(), "{bad} must be refused");
        }
        assert_eq!(Capability::parse("net:API.Example.com").unwrap(), Capability::Net("api.example.com".into()));
    }

    #[test]
    fn declaring_grants_nothing_and_grants_need_a_declaration() {
        let declared = ["media.read".to_string(), "net:api.example.com".into(), "commands".into()];
        let none = Access::new(&declared, &[]);
        assert!(none.allows(&Capability::Commands), "commands keeps its v0.1 meaning");
        assert!(!none.allows(&Capability::MediaRead));
        assert!(none.refuse(&Capability::MediaRead, "status").unwrap().contains("granted"));
        let granted = Access::new(&declared, &["media.read".into(), "wallpaper.set".into()]);
        assert!(granted.allows(&Capability::MediaRead));
        assert!(!granted.allows(&Capability::WallpaperSet), "a grant for an undeclared capability is ignored");
        assert!(granted.refuse(&Capability::WallpaperSet, "apply").unwrap().contains("permissions"));
        assert!(granted.refuse(&Capability::MediaRead, "status").is_none());
        assert!(granted.refuse(&Capability::MediaRead, "delete-everything").unwrap().contains("no operation"));
        assert!(granted.refuse(&Capability::Commands, "x").is_some(), "commands go through host::command only");
    }

    #[test]
    fn sensitive_operations_need_user_activation() {
        let a = Access::new(&["clipboard.read".into(), "open.web".into(), "clipboard.write".into()], &["clipboard.read".into(), "open.web".into(), "clipboard.write".into()]);
        assert!(a.refuse_now(&Capability::ClipboardRead, "text", false).unwrap().contains("click"));
        assert!(a.refuse_now(&Capability::ClipboardRead, "text", true).is_none());
        assert!(a.refuse_now(&Capability::OpenWeb, "open", false).is_some());
        assert!(a.refuse_now(&Capability::ClipboardWrite, "text", false).is_none(), "copying needs no gesture");
        let timers = Access::new(&["timers".into()], &[]);
        assert!(timers.allows(&Capability::Timers), "timers need no grant");
        assert!(Capability::Timers.is_subscription("every") && !Capability::Timers.is_subscription("after"));
    }

    #[test]
    fn views_must_be_trees() {
        use crate::wasm::{Align, Style, Tone};
        let node = |children: Vec<u32>| Node {
            id: String::new(), kind: Kind::Column, text: String::new(), hint: String::new(), action: String::new(), value: 0.0, min: 0.0, max: 0.0, step: 0.0, index: 0, children,
            style: Style { width: 0.0, height: 0.0, min_width: 0.0, min_height: 0.0, padding: 0.0, gap: 0.0, radius: 0.0, size: 0.0, grow: false, bold: false, align: Align::Auto, tone: Tone::Normal },
            options: Vec::new(), image: 0, marks: Vec::new(), disabled: false,
        };
        let view = |nodes: Vec<Node>| View { nodes, root: 0, focus: String::new() };
        assert!(check_view(&view(vec![node(vec![1, 2]), node(vec![]), node(vec![])])).is_ok());
        assert!(check_view(&view(vec![node(vec![1, 1]), node(vec![])])).unwrap_err().contains("trees"), "a shared child");
        assert!(check_view(&view(vec![node(vec![1]), node(vec![0])])).unwrap_err().contains("trees"), "a cycle through the root");
        assert!(check_view(&view(vec![node(vec![0])])).is_err(), "the root as its own child");
        assert!(check_view(&view(vec![node(vec![5])])).is_err(), "a missing child");
        let many: Vec<u32> = std::iter::repeat_n(1, MAX_VIEW_NODES + 1).collect();
        assert!(check_view(&view(vec![node(many), node(vec![])])).is_err(), "too many child entries");
    }

    #[test]
    fn legacy_commands_need_the_matching_capability() {
        assert_eq!(command_capability("media play-pause"), Some(Capability::MediaControl));
        assert_eq!(command_capability("notifications send"), Some(Capability::Notify));
        assert_eq!(command_capability("launcher search"), Some(Capability::LauncherGlobal));
        assert_eq!(command_capability("panel"), None);
    }

    #[test]
    fn outlets_are_validated() {
        let l = |id: &str, prefix: &str, global: bool| ManifestLauncher { id: id.into(), name: "Moon".into(), prefix: prefix.into(), global, icon: "moon".into() };
        let (ok, problems) = split_launchers(vec![l("moon", "moon ", false), l("moon", "m ", false), l("Bad", "x", false), l("none", "", false), l("slash", "/x", false), l("glob", "", true)]);
        assert_eq!(ok.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), ["moon", "glob"]);
        assert_eq!(problems.len(), 4, "{problems:?}");
        let tab = |page: &str, id: &str| ManifestStudioTab { page: page.into(), id: id.into(), title: "Moon".into(), icon: "moon".into() };
        let (ok, problems) = split_studio_tabs(vec![tab("desktop", "moon"), tab("security", "x"), tab("desktop", "moon")]);
        assert_eq!((ok.len(), problems.len()), (1, 2));
        let (ok, problems) = split_serves(vec!["phase".into(), "phase".into(), "Bad".into()]);
        assert_eq!((ok, problems.len()), (vec!["phase".to_string()], 2));
    }

    #[test]
    fn links_come_from_the_manifest_or_its_repository() {
        assert_eq!(source_url("", "https://github.com/me/moon"), Some("https://github.com/me/moon".into()));
        assert_eq!(source_url("https://codeberg.org/me/moon/", ""), Some("https://codeberg.org/me/moon".into()));
        assert_eq!(source_url("", "https://example.com"), None);
        assert_eq!(issues_url("", "", "https://github.com/me/moon/tree/main"), Some("https://github.com/me/moon/issues".into()));
        assert_eq!(issues_url("https://example.com/help", "", ""), Some("https://example.com/help".into()));
        assert_eq!(issues_url("javascript:alert(1)", "", ""), None);
        assert!(github_repo("https://github.com/../x").is_none());
    }

    #[test]
    fn features_name_what_is_missing() {
        assert!(missing_features(&["ui.input".into(), "launcher".into()]).is_empty());
        assert_eq!(missing_features(&["ui.input".into(), "ui.webview".into()]), ["ui.webview"]);
    }
}
