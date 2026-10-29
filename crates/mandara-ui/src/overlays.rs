//! Keyboard-driven overlays opened on demand as their own layer surfaces
//! (exclusive keyboard focus while open):
//!
//! * **Launcher** — rises out of the bottom edge (or drops from the top, or
//!   floats in the middle): results above, search at the bottom. Plain text
//!   searches apps (fuzzy, frecency-ranked), `>` lists actions (themes,
//!   appearance, session), `=` evaluates arithmetic, and the providers in
//!   [`crate::launcher_ext`] answer their prefixes (`:` emoji, `@` windows,
//!   `#` clipboard, `*` wallpapers, `~` files).
//! * **Session dock** — slides in from its edge (opposite the sidebar; left
//!   in RTL) over a scrim: big
//!   rounded buttons whose corner radius morphs on focus / press.

use crate::bidi::Bidi as _;
use std::time::Instant;

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use gpui::{
    AnyWindowHandle, App, AppContext as _, ClickEvent, ClipboardItem, Context, Entity, FocusHandle, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ObjectFit, ParentElement, Render, ScrollEvent,
    StatefulInteractiveElement, Styled, StyledImage as _, Window, WindowHandle, div, img, prelude::FluentBuilder as _, px,
};
use mandara_core::{Edge, FrameClock, LayerSurfaceSpec, SpringConfig, SpringValue, SurfaceRole, ToGpui, edge_drawer_snapped};
use mandara_i18n::{t, tf, tk, tr};
use mandara_services::SessionAction;
use mandara_theme::{Color, MandaraPalette, catalog};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as MatcherConfig, Matcher};

use crate::apps::{self, App as DesktopApp};
use crate::launcher_ext::{self, Hit, HitAction, Lead, UsageSnapshot};
use crate::fonts::fonts;
use crate::icons::icon;
use crate::kit::text;
use crate::shell::frame_color;
use crate::state::{ShellState, config, shell};

/// Polls (one frame) until the launcher has closed before running the follow-up.
const CLOSE_POLL: std::time::Duration = std::time::Duration::from_millis(16);
/// Lets the compositor hand focus back after the launcher closes.
const REFOCUS_SETTLE: std::time::Duration = std::time::Duration::from_millis(50);
/// Text caret blink half-period (platform convention).
const CARET_BLINK: std::time::Duration = std::time::Duration::from_millis(530);

const ROW_H: f32 = 52.0;
/// Row height in the compact list.
const ROW_H_COMPACT: f32 = 38.0;
/// Grid layout: column bounds, tile height and icon size.
const GRID_COLUMNS_MIN: usize = 3;
const GRID_COLUMNS_MAX: usize = 10;
const GRID_TILE_H: f32 = 92.0;
const GRID_ICON: f32 = 44.0;
/// Emoji glyph size in result rows.
const EMOJI_LEAD_PX: f32 = 24.0;
/// Corner radius of image thumbnails in result rows.
const THUMB_RADIUS: f32 = 8.0;
/// Highest screen dim behind the launcher.
const DIM_MAX: f32 = 0.7;
/// Leading tile of a result row (normal / compact), its corner radius and
/// glyph sizes as fractions of the tile.
const ROW_LEAD_PX: f32 = 34.0;
const ROW_LEAD_COMPACT_PX: f32 = 26.0;
const LEAD_RADIUS_FRAC: f32 = 0.3;
const LEAD_ICON_FRAC: f32 = 0.53;
const APP_ICON_FRAC: f32 = 0.88;
/// Pointer slack around the launcher before leaving counts, logical px.
const LEAVE_SLACK_PX: f32 = 24.0;
/// Results kept per query (the visible window scrolls through them).
const MAX_RESULTS: usize = 48;
const SEARCH_H: f32 = 52.0;
const PAD: f32 = 14.0;
/// Slightly under-damped (ζ ≈ 0.6) so the drawer springs up with a touch of
/// overshoot (`[panels] spring` overrides it, as for panels).
const DRAWER_SPRING: SpringConfig = SpringConfig::new(320.0, 21.5, 1.0);
/// Closing: critically damped, quicker than the opening.
const CLOSE_STIFFNESS: f32 = 420.0;
/// Open progress (0–1) resolution: invisible below ~half a pixel of travel.
const OPEN_REST_EPSILON: f32 = 0.001;
/// Gap between the results and the search field.
const LIST_GAP: f32 = 8.0;
/// Fewest result rows shown, however low the height is set.
const MIN_ROWS: usize = 2;
/// Bar fills at or above this alpha count as opaque (the drawer then
/// overlaps the bar a hairline to hide the seam, as panels do).
const OPAQUE_ALPHA: f32 = 0.999;
/// Frames to wait for the surface to settle at the focused output's size
/// before accepting any output's size (the compositor picked another one).
const SETTLE_FALLBACK_FRAMES: u8 = 6;
/// Viewport sizes within this many px of an output count as settled.
const SETTLE_SLACK_PX: f32 = 2.0;
/// Time for the focused window to take the keyboard back before an
/// auto-paste.
const AUTO_PASTE_DELAY: std::time::Duration = std::time::Duration::from_millis(120);

/// Output geometry measured at startup via xdg-output.
#[derive(Clone, Default)]
pub struct Outputs(pub Vec<mandara_core::OutputGeometry>);
impl gpui::Global for Outputs {}

/// Logical size of the primary output (for other full-output overlays).
pub fn primary_output_size(cx: &App) -> (f32, f32) {
    output_size(cx)
}

/// Logical size of the primary output (overlays cover it entirely).
fn output_size(cx: &App) -> (f32, f32) {
    cx.try_global::<Outputs>()
        .and_then(|o| o.0.first().map(|g| (g.width as f32, g.height as f32)))
        .or_else(|| mandara_core::query_outputs().ok().and_then(|o| o.first().map(|g| (g.width as f32, g.height as f32))))
        .unwrap_or((1920.0, 1080.0))
}

fn overlay_spec(role: SurfaceRole, w: f32, h: f32) -> gpui::WindowOptions {
    let mut spec = LayerSurfaceSpec::overlay(role, gpui::size(px(w), px(h)), None).with_keyboard(KeyboardInteractivity::Exclusive);
    spec.layer = Layer::Overlay;
    spec.anchor = Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT;
    spec.window_options()
}

/// One launcher result.
#[derive(Clone)]
enum Item {
    App(DesktopApp),
    Calc { expr: String, value: f64 },
    /// A provider result (emoji, window, clipboard entry, wallpaper, file,
    /// app action).
    Hit(Hit),
    /// Everything else: actions, plugins, commands, Studio sections, slash
    /// commands and skills.
    Entry(Entry),
}

#[derive(Clone)]
struct Entry {
    label: String,
    detail: String,
    icon: &'static str,
    /// Small kind badge shown at the end of the row.
    badge: &'static str,
    /// Extra search words (synonyms) beyond label and detail.
    keywords: String,
    run: Run,
}

/// What an entry runs. Actions, plugins and Studio sections are documented
/// commands too (`theme set …`, `panel open …`, `studio …`), so everything
/// the launcher does goes through the command registry (and `--dry-run`).
#[derive(Clone)]
enum Run {
    /// A command line (`mandara msg` words: the path plus any arguments the
    /// entry fixes, like a theme id or an enum choice) and the arguments
    /// still to type (required ones are prompted for; typed words
    /// after the line are passed on as arguments.
    Command { words: Vec<String>, args: Vec<mandara_plugins::ArgSpec> },
    Slash { command: mandara_config::SlashCommand, query: String },
    Skill(mandara_config::Skill),
}

/// What activating a result does (see [`plan`]).
#[derive(Clone)]
enum Effect {
    /// Keep the launcher open with this query: arguments are still missing.
    Prompt(String),
    App(DesktopApp),
    Copy(String),
    /// Open a file or folder with its default app.
    Open(std::path::PathBuf),
    /// Run an app's desktop action.
    AppAction(DesktopApp, apps::DesktopAction),
    /// Command lines run in order through the registry.
    Commands(Vec<Vec<String>>),
    Url(String),
    Shell(String),
    Fail(String),
}

fn entry(label: impl Into<String>, detail: impl Into<String>, icon: &'static str, badge: &'static str, keywords: &str, run: Run) -> Item {
    Item::Entry(Entry { label: label.into(), detail: detail.into(), icon, badge, keywords: keywords.into(), run })
}

fn words(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

/// A fixed command line (no typed arguments).
fn line(line: &str) -> Run {
    Run::Command { words: words(line), args: Vec::new() }
}

/// A config icon name as a bundled icon (falls back to the grid).
fn bundled_icon(name: &str) -> &'static str {
    crate::icons::ICON_NAMES.iter().find(|n| **n == name).copied().unwrap_or("grid")
}

fn actions() -> Vec<Item> {
    let mut out: Vec<Item> = catalog::all()
        .iter()
        .map(|theme| {
            entry(
                tf("Theme: {name}", &[("name", &theme.name)]),
                tr(&theme.description),
                "sun",
                tk("Theme"),
                &format!("color palette look {}", theme.id),
                line(&format!("theme set {}", theme.id)),
            )
        })
        .collect();
    let a = |label: &str, detail: &str, icon: &'static str, keywords: &str, command: &str| entry(label, detail, icon, tk("Action"), keywords, line(command));
    // Details that are command lines stay untranslated.
    out.extend([
        a(t("Light appearance"), "theme mode light", "sun", "day bright", "theme mode light"),
        a(t("Dark appearance"), "theme mode dark", "moon", "night", "theme mode dark"),
        a(t("Toggle sidebar"), "sidebar toggle", "sidebar", "drawer panel", "sidebar toggle"),
        a(t("Change wallpaper"), t("Open the wallpaper picker"), "image", "wallpaper background backdrop desktop picture photo", "wallpaper picker open"),
        a(t("Random wallpaper"), t("Pick a random image from your wallpaper folder"), "refresh", "wallpaper background shuffle desktop picture", "wallpaper random"),
        a(t("Take a screenshot"), t("Select a region to capture"), "crop", SNIP_WORDS, "screenshot region"),
        a(t("Record the screen"), t("AV1 video of the whole screen"), "circle", "screen recording video capture", "record start"),
        a(t("Record a region"), t("Choose an area to record"), "crop", "screen recording video capture area", "record region"),
        a(t("Clipboard history"), t("Recent copies"), "clipboard", "paste copy history", "panel open clipboard"),
    ]);
    if mandara_services::keepassxc::installed() {
        out.push(a(t("Unlock KeePassXC"), t("Type a database password in Mandara's dialog"), "key", "keepass keepassxc password manager vault kdbx database unlock", "keepassxc unlock"));
    }
    for s in SessionAction::ALL {
        let icon = match s {
            SessionAction::Lock => "lock",
            SessionAction::Logout => "logout",
            SessionAction::Suspend => "moon",
            SessionAction::Hibernate => "hibernate",
            SessionAction::Reboot => "restart",
            SessionAction::Shutdown => "power",
        };
        // Choosing the entry is the confirmation `session run` asks for.
        let command = if s.is_destructive() { format!("session run {} confirm", s.id()) } else { format!("session run {}", s.id()) };
        let keywords = match s {
            SessionAction::Lock => "power lock screen session",
            SessionAction::Logout => "power log out sign out exit session",
            SessionAction::Suspend => "power sleep suspend",
            SessionAction::Hibernate => "power hibernate suspend disk",
            SessionAction::Reboot => "power reboot restart computer",
            SessionAction::Shutdown => "power shut down off poweroff computer",
        };
        out.push(entry(s.label(), command.clone(), icon, tk("Session"), keywords, line(&command)));
    }
    out
}

/// Plugins whose bar item has no panel: what opening them from the
/// launcher does instead (the rest open their panel).
fn plugin_line(id: &str) -> String {
    match id {
        "session" => "session menu open".into(),
        "wallpaper" => "wallpaper picker open".into(),
        crate::plugins::wallpaper_generator::ID => "studio wallpaper".into(),
        "quick-settings" => "sidebar open".into(),
        "keyboard-layout" | "workspaces" | "active-window" => format!("studio plugins/{id}"),
        _ => format!("panel open {id}"),
    }
}

/// Commands not listed: the launcher's own verbs and Studio plumbing.
const HIDDEN_COMMANDS: &[&str] = &["launcher", "launcher search", "launcher run", "config apply"];
/// Commands whose first argument is not offered as one entry per choice
/// (`panel open` needs a plugin; session actions have their own entries).
const NO_EXPAND: &[&str] = &["panel", "session run"];

/// The entries of one documented command: the command itself, plus one per
/// value of an enum / on-off first argument (`theme mode dark`,
/// `notifications dnd on`) so they're found and run without typing.
fn command_entries(c: &mandara_plugins::CommandSpec) -> Vec<Item> {
    use mandara_plugins::ArgKind;
    let detail = tr(c.summary);
    let keywords = format!("command {}", c.owner);
    let make = |fixed: Option<&str>| {
        let mut label = c.path.to_string();
        let mut words = words(c.path);
        let rest = match fixed {
            Some(v) => {
                label = format!("{label} {v}");
                words.push(v.to_string());
                &c.args[1..]
            }
            None => &c.args[..],
        };
        for a in rest {
            label.push_str(&if a.required { format!(" <{}>", a.name) } else { format!(" [{}]", a.name) });
        }
        let run = Run::Command { words, args: rest.to_vec() };
        entry(label, detail.clone(), "chevron-right", tk("Command"), &keywords, run)
    };
    let choices: Vec<&str> = match c.args.first() {
        Some(a) if !NO_EXPAND.contains(&c.path) => match a.kind {
            ArgKind::Enum => a.choices.iter().copied().filter(|v| !v.starts_with("--")).collect(),
            ArgKind::Bool => vec!["on", "off"],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    // The bare command, unless every use of it is one of the choices.
    if choices.is_empty() || !c.args[0].required {
        out.push(make(None));
    }
    out.extend(choices.into_iter().map(|v| make(Some(v))));
    out
}

/// Studio sections reachable from the launcher (slug, title, icon, synonyms).
/// Every slug must open its section with `mandara studio <slug>` (checked by
/// Studio's tests against its `Section` list).
pub const STUDIO_SECTIONS: &[(&str, &str, &str, &str)] = &[
    ("home", tk("Home"), "grid", "overview start dashboard status quick settings"),
    // Look & feel
    ("themes", tk("Themes"), "droplet", "appearance looks colors palette wallpaper colors material you dark light icons roundness app themes"),
    ("wallpaper", tk("Wallpaper"), "image", "background desktop picture folder picker depth clock"),
    ("desktop-widgets", tk("Desktop widgets"), "clock", "desktop clock system monitor decorations visualizer cards now playing calendar arrange edit"),
    ("typography", tk("Typography"), "text-scan", "fonts text size"),
    ("motion", tk("Motion"), "waves", "animation springs speed"),
    // Desktop
    ("bar", tk("Bar"), "window", "top bar panel height layout"),
    ("sidebar", tk("Sidebars & hover"), "sidebar", "drawer cards pin notifications tab hover edges power dock open on hover"),
    ("dock", tk("Dock"), "box", "dock taskbar pinned apps magnification auto hide"),
    ("launcher", tk("Launcher"), "search", "slash commands skills icon emoji providers pinned hidden apps frecency usage"),
    ("window-switcher", tk("Window switcher"), "copy", "alt tab switcher windows screen time wellbeing limits"),
    ("notifications", tk("Notifications"), "bell", "popups toasts do not disturb dnd quiet hours rules mute history"),
    ("on-screen-displays", tk("On-screen displays"), "sliders", "osd volume brightness pill indicator caps lock"),
    // System
    ("displays", tk("Displays"), "monitor", "monitors screens resolution refresh rate scale rotation arrangement profiles vrr brightness ddc night light gamma blue light warm"),
    ("audio", tk("Audio & media"), "volume-high", "sound mixer visualizer media lyrics music"),
    ("network", tk("Network & Bluetooth"), "wifi-4", "wifi wi-fi vpn wireguard openvpn hotspot tethering saved networks forget dns ip static ethernet bluetooth pairing pair trust adapter"),
    ("power", tk("Power & idle"), "power", "idle sleep suspend hibernate lock screen off dpms caffeine keep awake battery low critical profile session menu shutdown reboot firmware hooks automation"),
    ("input", tk("Input"), "pointer", "keyboard layout repeat numlock mouse touchpad trackpad tap natural scrolling acceleration cursor theme size focus follows mouse"),
    ("keyboard-shortcuts", tk("Keyboard shortcuts"), "keyboard", "keybinds hotkeys bindings shortcuts"),
    ("date", tk("Date, time & location"), "calendar", "clock time date format strftime week numbers calendar ics events reminders location weather geoclue"),
    ("applications", tk("Applications"), "package", "default apps browser terminal file manager mime autostart startup login apps"),
    // Security & accounts
    ("users", tk("Users"), "user", "accounts avatar password user"),
    ("login-screen", tk("Login screen"), "key", "greeter display manager sddm login"),
    ("lock-appearance", tk("Lock appearance"), "lock", "lockscreen blur clock avatar media notifications battery oled"),
    ("privacy", tk("Privacy"), "shield", "microphone camera screen sharing indicator in use"),
    // Extend
    ("plugins", tk("Plugins"), "zap", "extensions add-ons"),
    ("compositor", tk("Compositor"), "tiles", "mango niri hyprland borders gaps keybinds rules"),
    // Mandara
    ("general", tk("General"), "globe", "language clock week restart"),
    ("profiles", tk("Profiles & backup"), "download", "profile export import backup restore reset undo recent changes support report"),
    ("performance", tk("Performance"), "gauge", "cpu memory ram usage frames"),
    ("maintenance", tk("Maintenance"), "refresh", "repair heal fix doctor factory reset backup restore download models fonts cache"),
    ("about", tk("About"), "note", "version info"),
];

/// Everything searchable besides apps, built when the launcher opens.
/// Words people type for the screenshot tool (Windows' Snipping Tool,
/// "snip", Print Screen…).
const SNIP_WORDS: &str = "screen capture snip snipping tool snippet sniptool print printscreen prtsc grab";

/// Extra search words for built-in plugins, by id: what people call them
/// when they don't know Mandara's name.
const PLUGIN_SYNONYMS: &[(&str, &str)] = &[(crate::plugins::screenshot::ID, SNIP_WORDS)];

fn build_index(cx: &App) -> Vec<Item> {
    let cfg = config(cx).read(cx).snapshot();
    build_index_with(cx.global::<mandara_plugins::PluginRegistry>(), &cfg)
}

fn build_index_with(registry: &mandara_plugins::PluginRegistry, cfg: &mandara_config::ShellConfig) -> Vec<Item> {
    let mut out = actions();
    // Actions of opt-in plugins, while they are on.
    if registry.is_enabled(crate::plugins::wallpaper_generator::ID, &cfg.plugins) {
        out.push(entry(t("Generate wallpaper"), t("A new procedural wallpaper from the generator"), "sparkles", tk("Action"), "wallpaper generate generator procedural new art background random", line("wallgen generate random")));
    }
    for m in registry.manifests() {
        if m.id == crate::plugins::session::LAUNCHER_ID || !registry.is_enabled(&m.id, &cfg.plugins) {
            continue;
        }
        let synonyms = PLUGIN_SYNONYMS.iter().find(|(id, _)| *id == m.id).map_or("", |(_, words)| words);
        let keywords = format!("plugin {} {synonyms}", m.id);
        out.push(entry(m.display_name(), m.display_description(), "grid", tk("Plugin"), &keywords, line(&plugin_line(&m.id))));
        out.push(entry(
            tf("Plugin settings: {name}", &[("name", &m.display_name())]),
            t("Open in Mandara Studio"),
            "sliders",
            tk("Studio"),
            &format!("settings preferences studio {keywords}"),
            line(&format!("studio plugins/{}/settings", m.id)),
        ));
    }
    for c in registry.commands.all() {
        if c.path.starts_with("debug ") || HIDDEN_COMMANDS.contains(&c.path) {
            continue;
        }
        // Commands of an off (or removed) plugin stay out of the launcher.
        if !c.works_when_off && registry.get(&c.owner).is_some() && !registry.is_enabled(&c.owner, &cfg.plugins) {
            continue;
        }
        out.extend(command_entries(c));
    }
    for (slug, title, icon, keywords) in STUDIO_SECTIONS {
        out.push(entry(tf("Settings: {title}", &[("title", &tr(title))]), t("Open in Mandara Studio"), icon, tk("Studio"), &format!("settings preferences studio {keywords}"), line(&format!("studio {slug}"))));
    }
    for sc in &cfg.launcher.commands {
        out.push(entry(format!("/{} — {}", sc.trigger, tr(&sc.title)), tr(&sc.description), bundled_icon(&sc.icon), tk("Slash"), "slash command", Run::Slash { command: sc.clone(), query: String::new() }));
    }
    for sk in &cfg.launcher.skills {
        out.push(entry(tr(&sk.name), if sk.description.is_empty() { sk.steps.join(" → ") } else { tr(&sk.description) }, bundled_icon(&sk.icon), tk("Skill"), "skill routine", Run::Skill(sk.clone())));
    }
    out
}

fn haystack(item: &Item) -> String {
    match item {
        Item::App(a) => a.haystack(),
        Item::Entry(e) => format!("{} {} {}", e.label, e.detail, e.keywords),
        Item::Hit(h) => h.haystack(),
        Item::Calc { .. } => String::new(),
    }
}

/// The key launches of `item` are counted under (frecency).
fn usage_key(item: &Item) -> String {
    match item {
        Item::App(a) => format!("app:{}", a.id),
        Item::Hit(h) => h.usage_key(),
        Item::Entry(e) => match &e.run {
            Run::Command { words, .. } => format!("cmd:{}", words.join(" ")),
            Run::Slash { command, .. } => format!("slash:{}", command.trigger),
            Run::Skill(skill) => format!("skill:{}", skill.name),
        },
        Item::Calc { .. } => String::new(),
    }
}

/// Live data the providers search, gathered when the launcher opens.
#[derive(Clone, Default)]
struct SearchCtx {
    providers: mandara_config::workflow::LauncherProviders,
    usage: UsageSnapshot,
    pinned: Vec<String>,
    recent_count: usize,
    windows: Vec<mandara_services::toplevels::Toplevel>,
    clipboard: Vec<mandara_services::ClipEntry>,
    wallpapers: Vec<std::path::PathBuf>,
    recent_files: Vec<std::path::PathBuf>,
    /// Desktop actions of the apps (empty when they aren't listed).
    app_actions: Vec<Hit>,
}

impl SearchCtx {
    fn gather(apps: &[DesktopApp], cx: &App) -> Self {
        let cfg = config(cx).read(cx).snapshot();
        let l = &cfg.launcher;
        let providers = l.providers.clone();
        let wants = |id: &str| launcher_ext::provider(&providers, id).is_some_and(|p| p.enabled);
        if wants("files") {
            launcher_ext::warm_file_index();
        }
        Self {
            usage: launcher_ext::usage_snapshot(&cfg),
            pinned: l.pinned.clone(),
            recent_count: l.usage.recent_count as usize,
            windows: if wants("windows") { crate::windows::windows(cx).map(|w| w.read(cx).list().to_vec()).unwrap_or_default() } else { Vec::new() },
            clipboard: if wants("clipboard") { crate::hub::hub(cx).read(cx).clipboard.entries.clone() } else { Vec::new() },
            wallpapers: if wants("wallpapers") { crate::wallpaper::list(&crate::wallpaper::directory(cx)) } else { Vec::new() },
            recent_files: if wants("files") { launcher_ext::recent_files() } else { Vec::new() },
            app_actions: if l.look.app_actions { launcher_ext::action_hits(apps) } else { Vec::new() },
            providers,
        }
    }

    /// Results of provider `id` for `q`.
    fn provider_hits(&self, id: &str, q: &str, limit: usize) -> Vec<Hit> {
        match id {
            "emoji" => launcher_ext::emoji_hits(q, limit),
            "windows" => launcher_ext::window_hits(q, &self.windows, limit),
            "clipboard" => launcher_ext::clipboard_hits(q, &self.clipboard, limit),
            "wallpapers" => launcher_ext::wallpaper_hits(q, &self.wallpapers, limit),
            "files" => launcher_ext::file_hits(q, &self.recent_files, limit),
            _ => Vec::new(),
        }
    }
}

/// Percent-encodes a query for a URL template (`%20` for spaces, valid in
/// paths and query strings alike).
fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Single-quotes a word for `sh`.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Read-only commands whose reply is the point: run from the launcher, the
/// reply is shown as a notification (so `prayer today` isn't a silent no-op).
const OUTPUT_COMMANDS: &[&str] = &[
    "audio devices", "audio status", "audio streams", "authenticator code", "authenticator list", "bluetooth status",
    "brightness status", "clipboard list", "clock now", "color-picker history", "color-picker pick", "color-picker show",
    "compositor clients", "compositor monitors", "compositor status", "config get", "config show", "containers list",
    "containers logs", "containers stats", "diagnostics frames", "diagnostics memory", "diagnostics views", "drives list",
    "info", "language list", "media status", "models list", "net-speed addresses", "net-speed public-ip",
    "net-speed status", "net-speed test", "network status", "network wifi list", "notes list", "notes show",
    "notifications list", "ocr", "ping", "plugins list", "power battery", "power history", "power status", "prayer next",
    "prayer today", "processes list", "services failed", "services list", "services logs", "services status",
    "soundscapes status", "system resources", "theme current", "theme list", "theme seeds", "todo list", "tray menu",
    "tray status", "updates check", "updates list", "updates news", "updates sources", "wallpaper current", "window list",
    "launcher usage list", "launcher emoji", "dock status", "screentime today", "screentime week", "screentime app",
    "wallpaper list", "wallgen list", "wallgen recipe show", "wallgen status", "weather", "window active", "workspace list", "world-clock at", "world-clock list",
    "world-clock search",
    // Power & idle (powerctl).
    "idle status", "power alerts", "hooks list", "hooks status", "hooks test", "session entries", "session boot-entries",
];

/// Commands that capture the screen or open a keyboard-grabbing overlay:
/// they wait until the launcher's surface is gone (else the snip tool
/// freezes a frame with the launcher in it, or two overlays fight over the
/// keyboard).
const AFTER_CLOSE: &[&str] = &[
    "screenshot", "record", "color-picker pick", "color-picker demo", "authenticator scan", "wallpaper picker",
    "session menu", "session run", "session do", "lock", "polkit", "keyring-prompt", "keepassxc unlock", "ocr", "switcher",
];

fn starts_with_line(words: &[String], line: &str) -> bool {
    let line: Vec<&str> = line.split(' ').collect();
    words.len() >= line.len() && line.iter().zip(words).all(|(l, w)| l == w)
}

/// One-line-per-item text of a command reply for a notification body.
fn summarize(value: &serde_json::Value) -> String {
    use serde_json::Value;
    fn inline(v: &Value) -> String {
        match v {
            Value::String(s) => s.clone(),
            Value::Null => "—".into(),
            Value::Object(map) => {
                const NAMES: [&str; 9] = ["name", "label", "title", "summary", "city", "unit", "id", "path", "text"];
                const STATES: [&str; 5] = ["state", "status", "time", "value", "active"];
                // A known name field, else the first text field that isn't a state.
                let name_key = NAMES
                    .iter()
                    .copied()
                    .find(|k| map.get(*k).is_some_and(|v| !v.is_null()))
                    .or_else(|| map.iter().find(|(k, v)| v.is_string() && !STATES.contains(&k.as_str())).map(|(k, _)| k.as_str()));
                let name = name_key.and_then(|k| map.get(k)).map(inline);
                let state = STATES.iter().filter(|k| Some(**k) != name_key).find_map(|k| map.get(*k).filter(|v| !v.is_null() && !v.is_object()).map(inline));
                match (name, state) {
                    (Some(n), Some(s)) => format!("{n} — {s}"),
                    (Some(n), None) => n,
                    _ => {
                        let s = v.to_string();
                        if s.chars().count() > 90 { format!("{}…", s.chars().take(90).collect::<String>()) } else { s }
                    }
                }
            }
            Value::Array(items) => format!("[{}]", items.iter().take(4).map(inline).collect::<Vec<_>>().join(", ")),
            other => other.to_string(),
        }
    }
    const MAX_LINES: usize = 10;
    let lines: Vec<String> = match value {
        Value::Array(items) if items.is_empty() => vec![t("Nothing to show").to_string()],
        Value::Array(items) => {
            let mut lines: Vec<String> = items.iter().take(MAX_LINES).map(inline).collect();
            if items.len() > MAX_LINES {
                lines.push(tf("…and {count} more", &[("count", &(items.len() - MAX_LINES))]));
            }
            lines
        }
        Value::Object(map) => map.iter().take(MAX_LINES).map(|(k, v)| format!("{k}: {}", inline(v))).collect(),
        other => vec![inline(other)],
    };
    let text = lines.join("\n");
    if text.chars().count() > 900 { format!("{}…", text.chars().take(900).collect::<String>()) } else { text }
}

fn notify(summary: String, body: String, cx: &mut App) {
    crate::plugins::notifications::send(cx, mandara_services::NotificationCommand::Post { app: t("Launcher").into(), icon: "search".into(), summary, body });
}

fn notify_failure(command: &str, err: String, cx: &mut App) {
    notify(tf("`{command}` failed", &[("command", &command)]), err, cx);
}

/// Runs a command line through the documented command registry; errors
/// (and pending failures) surface as notifications, and so do the replies of
/// read-only commands ([`OUTPUT_COMMANDS`], `… status`).
fn run_command(words: Vec<String>, cx: &mut App) {
    let registry = cx.global::<mandara_plugins::PluginRegistry>().commands.clone();
    let label = words.join(" ");
    let show = registry.resolve(&words).is_ok_and(|r| OUTPUT_COMMANDS.contains(&r.spec.path) || words.last().is_some_and(|w| w == "status"));
    // Run as from inside the shell even when `launcher run` came over IPC:
    // nothing typed into the launcher is a held-modifier gesture.
    match crate::dismiss::with_origin(crate::dismiss::Origin::InProcess, cx, |cx| registry.execute(&words, cx)) {
        Ok(mandara_plugins::Output::Pending(rx)) => {
            cx.spawn(async move |cx| {
                let result = rx.recv().await;
                cx.update(|cx| match result {
                    Ok(Ok(reply)) if show => notify(label, reply, cx),
                    Ok(Err(err)) => notify_failure(&label, err, cx),
                    _ => {}
                });
            })
            .detach();
        }
        Ok(mandara_plugins::Output::Value(value)) => {
            if show {
                notify(label, summarize(&value), cx);
            }
        }
        Err(err) => notify_failure(&label, err, cx),
    }
}

/// What a slash command does with `query` substituted for `{query}`
/// (percent-encoded in URLs, one shell word in `exec:` lines).
fn slash_effect(command: &mandara_config::SlashCommand, query: &str) -> Effect {
    let action = command.action.trim();
    if let Some(url) = action.strip_prefix("url:") {
        Effect::Url(url.trim().replace("{query}", &url_encode(query)))
    } else if let Some(cmd) = action.strip_prefix("mandara:") {
        Effect::Commands(vec![words(&cmd.replace("{query}", query))])
    } else if let Some(sh) = action.strip_prefix("exec:") {
        // A template that already quotes the placeholder gets one quoting.
        let line = sh.trim().replace("'{query}'", "{query}").replace("\"{query}\"", "{query}").replace("{query}", &shell_quote(query));
        Effect::Shell(line)
    } else {
        Effect::Fail(tf("slash command /{trigger}: unknown action {action} (use url:, mandara: or exec:)", &[("trigger", &command.trigger), ("action", &format!("{action:?}"))]))
    }
}

/// Words typed after a command line (`/audio volume set 40` → `40`),
/// matched word by word, ignoring case and the `/` or `>` prefix.
fn typed_args(query: &str, line: &[String]) -> Vec<String> {
    let typed: Vec<&str> = query.trim().trim_start_matches(['/', '>']).split_whitespace().collect();
    if typed.len() > line.len() && typed.iter().zip(line).all(|(t, l)| t.eq_ignore_ascii_case(l)) {
        typed[line.len()..].iter().map(|s| s.to_string()).collect()
    } else {
        Vec::new()
    }
}

/// What activating `item` (Enter / click) does, given the typed query.
fn plan(item: &Item, query: &str) -> Effect {
    match item {
        Item::App(app) => Effect::App(app.clone()),
        Item::Calc { value, .. } => Effect::Copy(format_number(*value)),
        Item::Hit(h) => match &h.action {
            HitAction::Copy(text) => Effect::Copy(text.clone()),
            HitAction::Command(words) => Effect::Commands(vec![words.clone()]),
            HitAction::Open(path) => Effect::Open(path.clone()),
            HitAction::AppAction(app, action) => Effect::AppAction(app.clone(), action.clone()),
        },
        Item::Entry(e) => match &e.run {
            Run::Command { words, args: specs } => {
                let mut args = if specs.is_empty() { Vec::new() } else { typed_args(query, words) };
                // Outside slash mode, words that don't fit the first argument
                // are search text ("lock screen"), not arguments.
                if !query.trim_start().starts_with('/') && args.first().is_some_and(|a| !accepts(&specs[0], a)) {
                    args.clear();
                }
                if specs.iter().any(|a| a.required) && args.is_empty() {
                    return Effect::Prompt(format!("/{} ", words.join(" ")));
                }
                Effect::Commands(vec![words.iter().cloned().chain(args).collect()])
            }
            Run::Slash { command, query } => {
                if query.trim().is_empty() && command.action.contains("{query}") {
                    Effect::Prompt(format!("/{} ", command.trigger))
                } else {
                    slash_effect(command, query.trim())
                }
            }
            Run::Skill(skill) => Effect::Commands(skill.steps.iter().map(|s| words(s)).filter(|w| !w.is_empty()).collect()),
        },
    }
}

/// A plain-text account of an effect (`launcher run --dry-run`, tests).
fn describe(effect: &Effect) -> String {
    match effect {
        Effect::Prompt(q) => format!("prompt: {q}"),
        Effect::App(app) => format!("app: {} ({})", app.name, app.exec),
        Effect::Copy(s) => format!("copy: {s}"),
        Effect::Open(path) => format!("open: {}", path.display()),
        Effect::AppAction(app, action) => format!("app action: {} — {} ({})", app.name, action.name, action.exec),
        Effect::Commands(lines) => format!("command: {}", lines.iter().map(|w| w.join(" ")).collect::<Vec<_>>().join(" ; ")),
        Effect::Url(url) => format!("url: {url}"),
        Effect::Shell(line) => format!("exec: {line}"),
        Effect::Fail(err) => format!("error: {err}"),
    }
}

/// Runs `f` once the launcher's surface has closed (immediately when it
/// isn't open), plus a frame for the compositor to drop it.
fn after_launcher_closed(cx: &mut App, f: impl FnOnce(&mut App) + 'static) {
    cx.spawn(async move |cx| {
        for _ in 0..120 {
            let open = cx.update(|cx| cx.try_global::<Overlays>().is_some_and(|o| is_open(&o.launcher, cx)));
            if !open {
                break;
            }
            cx.background_executor().timer(CLOSE_POLL).await;
        }
        cx.background_executor().timer(REFOCUS_SETTLE).await;
        cx.update(f);
    })
    .detach();
}

/// Carries out an effect (after the launcher started closing).
fn execute(effect: Effect, cx: &mut App) {
    match effect {
        Effect::Prompt(_) => {}
        Effect::App(app) => {
            let options = config(cx).read(cx).get().launch_options();
            if let Err(err) = apps::launch_with(&app, None, &options) {
                notify_failure(&app.name, err, cx);
            }
        }
        Effect::AppAction(app, action) => {
            let options = config(cx).read(cx).get().launch_options();
            if let Err(err) = apps::launch_with(&app, Some(&action), &options) {
                notify_failure(&app.name, err, cx);
            }
        }
        Effect::Open(path) => {
            std::thread::spawn(move || {
                if let Err(err) = mandara_services::session::open_uri(&format!("file://{}", path.display())) {
                    tracing::warn!("{err}");
                }
            });
        }
        Effect::Copy(text) => {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            // Auto-paste: type Ctrl+V once the launcher has given the
            // keyboard back to the window underneath.
            if config(cx).read(cx).get().launcher.launch.auto_paste {
                after_launcher_closed(cx, |_| {
                    std::thread::spawn(|| {
                        std::thread::sleep(AUTO_PASTE_DELAY);
                        if let Err(err) = mandara_services::clipboard::keyboard::ctrl_v() {
                            tracing::warn!("auto-paste: {err}");
                        }
                    });
                });
            }
        }
        Effect::Commands(lines) => {
            let wait = lines.iter().any(|w| AFTER_CLOSE.iter().any(|l| starts_with_line(w, l)));
            let run = move |cx: &mut App| {
                for words in lines {
                    run_command(words, cx);
                }
            };
            if wait {
                after_launcher_closed(cx, run);
            } else {
                run(cx);
            }
        }
        Effect::Url(url) => {
            std::thread::spawn(move || {
                if let Err(err) = mandara_services::session::open_uri(&url) {
                    tracing::warn!("{err}");
                }
            });
        }
        Effect::Shell(line) => {
            use std::os::unix::process::CommandExt as _;
            if let Err(err) = mandara_services::reaper::spawn(std::process::Command::new("sh").arg("-c").arg(&line).stdin(std::process::Stdio::null()).process_group(0)) {
                notify_failure(&line, err.to_string(), cx);
            }
        }
        Effect::Fail(err) => notify(t("Launcher").into(), err, cx),
    }
}

/// What the launcher shows for `query` with no live provider data (tests).
#[cfg(test)]
fn search(query: &str, apps: &[DesktopApp], index: &[Item]) -> Vec<Item> {
    search_with(query, apps, index, &SearchCtx::default())
}

/// What the launcher shows for `query` (shared with `launcher search`).
fn search_with(query: &str, apps: &[DesktopApp], index: &[Item], ctx: &SearchCtx) -> Vec<Item> {
    let q = query.trim();
    let routed = launcher_ext::route(q, &ctx.providers);
    let actions_prefix = routed.filter(|(id, _)| *id == "actions").map(|(_, rest)| rest);
    if let Some(expr) = q.strip_prefix('=') {
        apps::calculate(expr).map(|value| vec![Item::Calc { expr: expr.trim().to_string(), value }]).unwrap_or_default()
    } else if let Some((id, rest)) = routed.filter(|(id, _)| *id != "actions" && !q.starts_with('/')) {
        ctx.provider_hits(id, rest, launcher_ext::PROVIDER_LIMIT).into_iter().map(Item::Hit).collect()
    } else if let Some(rest) = actions_prefix {
        // `>`: actions, plugins, commands, Studio and skills (no apps).
        let all: Vec<Item> = index.iter().filter(|i| matches!(i, Item::Entry(e) if !matches!(e.run, Run::Slash { .. }))).cloned().collect();
        let rest = rest.trim();
        if rest.is_empty() {
            return all;
        }
        // A command line with arguments (`>audio volume set 40`) leads.
        let lines = command_matches(rest, index, false);
        let names: Vec<String> = lines.iter().map(|i| item_names(i).0).collect();
        let fuzzy = rank(fuzzy(all, rest, haystack), rest).into_iter().filter(|i| !names.contains(&item_names(i).0));
        lines.into_iter().chain(fuzzy).collect()
    } else if let Some(rest) = q.strip_prefix('/') {
        slash_results(rest, index)
    } else if q.is_empty() {
        let (order, _, _) = launcher_ext::browse_order(apps, &ctx.pinned, &ctx.usage, ctx.recent_count);
        order.into_iter().map(Item::App).collect()
    } else {
        // Apps and everything else in one ranked list (actions only when
        // that provider joins plain searches).
        let actions_global = ctx.providers.actions.enabled && ctx.providers.actions.global;
        let entries = index.iter().filter(|i| actions_global && !matches!(i, Item::Entry(e) if matches!(e.run, Run::Slash { .. }))).cloned();
        let hits = launcher_ext::globals(&ctx.providers)
            .filter(|id| *id != "actions")
            .flat_map(|id| ctx.provider_hits(id, q, launcher_ext::global_limit()))
            .chain(ctx.app_actions.iter().cloned())
            .map(Item::Hit);
        let pool: Vec<Item> = apps.iter().cloned().map(Item::App).chain(entries).chain(hits).collect();
        let lines = if actions_global { command_matches(q, index, false) } else { Vec::new() };
        let names: Vec<String> = lines.iter().map(|i| item_names(i).0).collect();
        let fuzzy = rank_with(fuzzy(pool, q, haystack), q, &ctx.usage).into_iter().filter(|i| !names.contains(&item_names(i).0));
        let mut found: Vec<Item> = lines.into_iter().chain(fuzzy).collect();
        // A bare arithmetic expression also offers its result.
        if let Some(value) = apps::calculate(q).filter(|_| q.chars().any(|c| "+-*/^%(".contains(c))) {
            found.insert(0, Item::Calc { expr: q.to_string(), value });
        }
        // Last: search the web for it (the first `url:` slash command).
        if let Some(Item::Entry(web)) = index
            .iter()
            .filter(|i| matches!(i, Item::Entry(Entry { run: Run::Slash { command, .. }, .. }) if command.action.trim().starts_with("url:") && command.action.contains("{query}")))
            .min_by_key(|i| !matches!(i, Item::Entry(Entry { run: Run::Slash { command, .. }, .. }) if command.trigger == "g"))
        {
            let Run::Slash { command, .. } = &web.run else { unreachable!() };
            let mut web = web.clone();
            web.label = format!("{} — “{q}”", tr(&command.title));
            web.run = Run::Slash { command: command.clone(), query: q.to_string() };
            found.push(Item::Entry(web));
        }
        found
    }
}

/// The name an item is listed under (and, for commands, its command line).
fn item_names(item: &Item) -> (String, String) {
    match item {
        Item::App(a) => (a.name.to_lowercase(), String::new()),
        Item::Entry(e) => (e.label.trim_start_matches('/').to_lowercase(), match &e.run {
            Run::Command { words, .. } => words.join(" ").to_lowercase(),
            _ => String::new(),
        }),
        Item::Hit(h) => (h.label.to_lowercase(), String::new()),
        Item::Calc { .. } => (String::new(), String::new()),
    }
}

/// Whether `q` starts a word of `name`.
fn word_prefix(name: &str, q: &str) -> bool {
    name.match_indices(q).any(|(i, _)| i == 0 || name[..i].chars().next_back().is_some_and(|c| !c.is_alphanumeric()))
}

/// Whether every typed word starts a word of the entry's name, synonyms or
/// detail (`lock screen` finds Lock before letters scattered elsewhere). A
/// single word counts against the name and synonyms only (`snip` finds the
/// screenshot tool; a description's every word would be noise).
fn all_words_prefix(item: &Item, q: &str) -> bool {
    let words: Vec<&str> = q.split_whitespace().collect();
    let single = words.len() == 1;
    let text = match item {
        Item::Entry(e) if single => format!("{} {}", e.label, e.keywords).to_lowercase(),
        Item::Entry(e) => format!("{} {} {}", e.label, e.keywords, e.detail).to_lowercase(),
        Item::App(a) if single => format!("{} {}", a.name, a.keywords).to_lowercase(),
        Item::App(a) => a.haystack().to_lowercase(),
        Item::Hit(h) => h.haystack().to_lowercase(),
        Item::Calc { .. } => return false,
    };
    !words.is_empty() && words.iter().all(|w| word_prefix(&text, w))
}

/// Exact names lead, then names starting with the query, then names with a
/// word starting with it, then entries matching every typed word at a word
/// start; within a tier raw commands follow apps, plugins
/// and actions (`weather` opens the panel before printing the JSON), and
/// fuzzy order holds.
fn rank(found: Vec<Item>, q: &str) -> Vec<Item> {
    rank_with(found, q, &UsageSnapshot::default())
}

/// [`rank`], with frecency deciding within a tier (before fuzzy order).
fn rank_with(mut found: Vec<Item>, q: &str, usage: &UsageSnapshot) -> Vec<Item> {
    let q = q.trim().to_lowercase();
    found.sort_by_key(|i| {
        let (name, line) = item_names(i);
        let line = (!line.is_empty()).then_some(line);
        let tier = if name == q || line.as_deref() == Some(q.as_str()) {
            0
        } else if name.starts_with(&q) || line.as_deref().is_some_and(|l| l.starts_with(&q)) {
            1
        } else if word_prefix(&name, &q) {
            2
        } else if all_words_prefix(i, &q) {
            3
        } else {
            4
        };
        (tier, matches!(i, Item::Entry(e) if e.badge == tk("Command")), std::cmp::Reverse(usage.frecency(&usage_key(i))))
    });
    found
}

/// `/trigger words…`: slash commands and skills by trigger, then any
/// documented command whose path the typed words start with (the rest
/// become its arguments).
fn slash_results(rest: &str, index: &[Item]) -> Vec<Item> {
    let words: Vec<&str> = rest.split_whitespace().collect();
    let first = words.first().copied().unwrap_or("").to_lowercase();
    let tail = words.get(1..).map(|w| w.join(" ")).unwrap_or_default();
    let mut out = Vec::new();
    for item in index {
        let Item::Entry(e) = item else { continue };
        match &e.run {
            Run::Slash { command, .. } if command.trigger.to_lowercase().starts_with(&first) => {
                let mut e = e.clone();
                let exact = command.trigger.eq_ignore_ascii_case(&first);
                if exact && !tail.is_empty() {
                    e.label = format!("{} — “{tail}”", tr(&command.title));
                }
                e.run = Run::Slash { command: command.clone(), query: if exact { tail.clone() } else { String::new() } };
                out.push(Item::Entry(e));
            }
            Run::Skill(skill) if skill.name.to_lowercase().replace(' ', "-").starts_with(&first) => out.push(item.clone()),
            _ => {}
        }
    }
    out.extend(command_matches(&words.join(" "), index, true).into_iter().map(|mut item| {
        if let Item::Entry(e) = &mut item {
            e.label = format!("/{}", e.label);
        }
        item
    }));
    out
}

/// Whether a typed word is a valid value for an argument.
fn accepts(arg: &mandara_plugins::ArgSpec, raw: &str) -> bool {
    use mandara_plugins::ArgKind;
    match arg.kind {
        ArgKind::String => true,
        ArgKind::Integer => raw.parse::<i64>().is_ok(),
        ArgKind::Number => raw.parse::<f64>().is_ok(),
        ArgKind::Bool => matches!(raw, "true" | "false" | "on" | "off" | "1" | "0"),
        ArgKind::Enum => arg.choices.iter().any(|c| c.eq_ignore_ascii_case(raw)),
    }
}

/// Documented commands for typed words: the exact line, then lines the
/// words extend (arguments being typed; longest first), then lines the
/// words start. Outside slash mode only the lines with typed arguments
/// (fuzzy ranking finds the rest).
fn command_matches(typed: &str, index: &[Item], slash: bool) -> Vec<Item> {
    let typed = typed.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let (mut exact, mut extended, mut completed) = (Vec::new(), Vec::new(), Vec::new());
    for item in index {
        let Item::Entry(e) = item else { continue };
        let Run::Command { words, args } = &e.run else { continue };
        if e.badge != tk("Command") {
            continue;
        }
        let line = words.join(" ").to_lowercase();
        if line == typed {
            if slash {
                exact.push(item.clone());
            }
        } else if let Some(arg) = args.first()
            && let Some(extra) = typed.strip_prefix(&format!("{line} "))
            // Outside slash mode the first typed argument must fit, so a
            // phrase like "lock screen" isn't taken for `lock <mode>`.
            && (slash || accepts(arg, extra.split(' ').next().unwrap_or_default()))
        {
            extended.push((line.len(), item.clone()));
        } else if slash && line.starts_with(&typed) {
            completed.push(item.clone());
        }
    }
    extended.sort_by_key(|(len, _)| std::cmp::Reverse(*len));
    exact.into_iter().chain(extended.into_iter().map(|(_, i)| i)).chain(completed).collect()
}

/// Installed apps as the launcher lists them (hidden apps removed, your
/// names and icons applied).
fn launcher_apps(cx: &App) -> Vec<DesktopApp> {
    launcher_ext::customize(apps::discover(), &config(cx).read(cx).get().launcher)
}

/// JSON view of launcher results (for `launcher search`).
pub fn search_json(query: &str, cx: &App) -> serde_json::Value {
    let apps = launcher_apps(cx);
    let index = build_index(cx);
    let ctx = SearchCtx::gather(&apps, cx);
    let results: Vec<serde_json::Value> = search_with(query, &apps, &index, &ctx)
        .into_iter()
        .take(12)
        .map(|i| result_json(&i, query))
        .collect();
    serde_json::Value::Array(results)
}

/// One result as JSON, with what Enter would do (`effect`).
fn result_json(item: &Item, query: &str) -> serde_json::Value {
    let effect = describe(&plan(item, query));
    match item {
        Item::App(a) => serde_json::json!({ "kind": "app", "label": a.name, "detail": a.comment, "effect": effect }),
        Item::Calc { expr, value } => serde_json::json!({ "kind": "calc", "label": format_number(*value), "detail": expr, "effect": effect }),
        Item::Hit(h) => serde_json::json!({ "kind": h.provider, "label": h.label, "detail": h.detail, "effect": effect }),
        Item::Entry(e) => serde_json::json!({ "kind": e.badge.to_lowercase(), "label": e.label, "detail": e.detail, "effect": effect }),
    }
}

/// `launcher run <query…> [--index N] [--dry-run]`: activates result N
/// (default 0) for `query` exactly as Enter in the launcher would; with
/// `--dry-run`, only reports what it would do.
pub fn run_json(args: &[String], cx: &mut App) -> Result<serde_json::Value, String> {
    let (mut dry_run, mut index, mut query) = (false, 0usize, Vec::new());
    let mut it = args.iter();
    while let Some(word) = it.next() {
        match word.as_str() {
            "--dry-run" => dry_run = true,
            "--index" => index = it.next().and_then(|n| n.parse().ok()).ok_or_else(|| t("--index needs a number").to_string())?,
            _ => query.push(word.as_str()),
        }
    }
    let query = query.join(" ");
    let apps = launcher_apps(cx);
    let ctx = SearchCtx::gather(&apps, cx);
    let results = search_with(&query, &apps, &build_index(cx), &ctx);
    let item = results.get(index).ok_or_else(|| tf("No result #{index} for {query} ({count} results)", &[("index", &index), ("query", &format!("{query:?}")), ("count", &results.len())]))?;
    let mut out = result_json(item, &query);
    out["dry_run"] = serde_json::Value::Bool(dry_run);
    if !dry_run {
        match plan(item, &query) {
            Effect::Prompt(_) => {}
            effect => {
                launcher_ext::record_use(&usage_key(item), &config(cx).read(cx).snapshot());
                // As from the launcher: it closes, then the effect runs.
                let s = shell(cx);
                if s.read(cx).launcher_open {
                    s.update(cx, |s, cx| s.set_launcher(false, cx));
                }
                execute(effect, cx);
            }
        }
    }
    Ok(out)
}

pub struct LauncherView {
    shell: Entity<ShellState>,
    focus: FocusHandle,
    query: String,
    /// Caret position in `query`, in chars.
    caret: usize,
    /// Ctrl+A: the whole query is selected (typing replaces it).
    select_all: bool,
    /// Where the query text was laid out (for click-to-place-caret).
    text_bounds: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
    /// Caret blink phase restarts on every edit (solid while typing).
    blink_from: Instant,
    blink_task: Option<gpui::Task<()>>,
    apps: Vec<DesktopApp>,
    /// Actions, plugins, commands, settings, slash commands and skills.
    index: Vec<Item>,
    /// Provider data (windows, clipboard, wallpapers, files, usage).
    ctx: SearchCtx,
    results: Vec<Item>,
    /// The launcher's rectangle last frame `(x, y, w, h)`, for pointer tests.
    rect: (f32, f32, f32, f32),
    selected: usize,
    /// Touchpad travel not yet turned into a row step (logical px).
    wheel: f32,
    /// A wheel step moved the selection: rows sliding under a still pointer
    /// don't take it back by hover until the pointer moves.
    wheel_hold: bool,
    /// Where the launcher rests (springs on result-count and placement
    /// changes, so `launcher placement` re-places it live).
    rest: SpringValue<gpui::Bounds<gpui::Pixels>>,
    /// Open progress 0–1 (from its start rect to `rest`).
    open: SpringValue<f32>,
    /// `rest` has been placed (snapped) once.
    placed: bool,
    highlight: SpringValue<f32>,
    /// Horizontal highlight position (grid layout).
    highlight_x: SpringValue<f32>,
    /// Thumbnails of image results (`None` while being made).
    thumbs: std::collections::HashMap<std::path::PathBuf, Option<std::path::PathBuf>>,
    clock: FrameClock,
    closing: bool,
    /// Pointer has been inside the drawer (enables leave-to-close).
    entered: bool,
    leave_task: Option<gpui::Task<()>>,
    /// The focused output's real logical size; nothing is drawn until the
    /// surface's viewport matches it (GPUI's first frames use an
    /// integer-scale guess).
    expected: (f32, f32),
    /// Frames spent waiting for that size.
    settle_frames: u8,
    /// The output the launcher opened on (once settled).
    output: Option<String>,
    /// When the surface was created (open latency, logged at debug level).
    created: Instant,
}

/// Whether a viewport is `size` (within the settle slack).
fn same_size(vp: gpui::Size<gpui::Pixels>, size: (f32, f32)) -> bool {
    (f32::from(vp.width) - size.0).abs() < SETTLE_SLACK_PX && (f32::from(vp.height) - size.1).abs() < SETTLE_SLACK_PX
}

/// Whether a freshly created surface has settled at its real size.
fn viewport_settled(window: &Window, expected: (f32, f32)) -> bool {
    same_size(window.viewport_size(), expected)
}

/// The focused output (the compositor opens overlays there), else the
/// first one.
fn focused_output(cx: &App) -> Option<mandara_core::OutputGeometry> {
    let outputs = &cx.try_global::<Outputs>()?.0;
    let focused = cx.try_global::<crate::hub::GlobalHub>().and_then(|h| h.0.read(cx).focused.as_ref().map(|c| c.monitor.clone()));
    focused.and_then(|m| outputs.iter().find(|o| o.name == m)).or_else(|| outputs.first()).cloned()
}

impl LauncherView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shell = shell(cx);
        cx.observe(&shell, |this: &mut Self, shell, cx| {
            // Reopening while the close animation runs revives the surface.
            this.closing = !shell.read(cx).launcher_open;
            if !this.closing {
                this.entered = false;
                this.leave_task = None;
            }
            cx.notify();
        })
        .detach();
        cx.observe(&mandara_core::theme_entity(cx), |_, _, cx| cx.notify()).detach();
        // Studio / `launcher placement` edits re-place it while open.
        cx.observe(&config(cx), |_, _, cx| cx.notify()).detach();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let mut this = Self {
            shell,
            focus,
            query: String::new(),
            caret: 0,
            select_all: false,
            text_bounds: Default::default(),
            blink_from: Instant::now(),
            blink_task: None,
            apps: Vec::new(),
            index: build_index(cx),
            ctx: SearchCtx::default(),
            results: Vec::new(),
            rect: (0.0, 0.0, 0.0, 0.0),
            selected: 0,
            wheel: 0.0,
            wheel_hold: false,
            rest: SpringValue::new(gpui::Bounds::default(), DRAWER_SPRING),
            open: SpringValue::new(0.0, DRAWER_SPRING).with_rest_epsilon(OPEN_REST_EPSILON),
            placed: false,
            highlight: SpringValue::new(0.0, SpringConfig::new(520.0, 38.0, 1.0)),
            highlight_x: SpringValue::new(0.0, SpringConfig::new(520.0, 38.0, 1.0)),
            thumbs: Default::default(),
            clock: FrameClock::new(),
            closing: false,
            entered: false,
            leave_task: None,
            expected: focused_output(cx).map(|o| (o.width as f32, o.height as f32)).unwrap_or_else(|| output_size(cx)),
            settle_frames: 0,
            output: None,
            created: Instant::now(),
        };
        this.apps = launcher_apps(cx);
        this.ctx = SearchCtx::gather(&this.apps, cx);
        if config(cx).read(cx).get().launcher.look.remember_query {
            let q = launcher_ext::last_query();
            this.caret = q.chars().count();
            this.query = q;
            this.select_all = !this.query.is_empty();
        }
        this.refilter();
        // Caret blink: re-render twice a second while open.
        this.blink_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CARET_BLINK).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        }));
        this
    }

    /// Places the caret at the character nearest to `x` (window coords).
    fn caret_at(&mut self, x: gpui::Pixels, window: &mut Window, cx: &App) {
        let Some(bounds) = self.text_bounds.get() else { return };
        let font = gpui::font(fonts(cx).ui.clone());
        let run = gpui::TextRun { len: self.query.len(), font, color: gpui::black(), background_color: None, underline: None, strikethrough: None };
        let line = window.text_system().shape_line(self.query.clone().into(), px(text::BODY + 1.0), &[run], None);
        let byte = line.closest_index_for_x(x - bounds.origin.x);
        self.caret = self.query[..byte.min(self.query.len())].chars().count();
        self.select_all = false;
        self.blink_from = Instant::now();
    }

    /// Replaces the selection (if any) before an edit.
    fn take_selection(&mut self) {
        if self.select_all {
            self.query.clear();
            self.caret = 0;
            self.select_all = false;
        }
    }

    fn refilter(&mut self) {
        self.results = search_with(&self.query, &self.apps, &self.index, &self.ctx);
        self.selected = 0;
    }

    /// The grid shows (app tiles while browsing), with its column count.
    fn grid_columns(&self, cx: &App) -> Option<usize> {
        let look = &config(cx).read(cx).get().launcher.look;
        (look.layout == mandara_config::workflow::LauncherLayout::Grid && self.query.trim().is_empty()).then(|| (look.grid_columns as usize).clamp(GRID_COLUMNS_MIN, GRID_COLUMNS_MAX))
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        launcher_ext::remember_query(&self.query);
        self.closing = true;
        self.shell.update(cx, |s, cx| s.set_launcher(false, cx));
    }

    /// Enter / click: prompts for missing arguments (the launcher stays
    /// open), otherwise closes the launcher and runs the result.
    fn activate(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(item) = self.results.get(index) else { return };
        match plan(item, &self.query) {
            Effect::Prompt(query) => {
                self.set_query(query);
                cx.notify();
            }
            effect => {
                launcher_ext::record_use(&usage_key(item), &config(cx).read(cx).snapshot());
                self.close(cx);
                execute(effect, cx);
            }
        }
    }

    fn set_query(&mut self, q: String) {
        self.caret = q.chars().count();
        self.query = q;
        self.refilter();
    }

    /// Byte offset of the caret.
    fn caret_byte(&self) -> usize {
        self.query.char_indices().nth(self.caret).map(|(i, _)| i).unwrap_or(self.query.len())
    }

    fn on_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let count = self.results.len().min(MAX_RESULTS);
        self.blink_from = Instant::now();
        // Grid browsing: arrows move in two dimensions.
        if let Some(cols) = self.grid_columns(cx).filter(|_| count > 0 && !k.modifiers.control) {
            let step: Option<isize> = match k.key.as_str() {
                "left" => Some(-1),
                "right" | "tab" => Some(1),
                "up" => Some(-(cols as isize)),
                "down" => Some(cols as isize),
                _ => None,
            };
            if let Some(d) = step {
                self.selected = (self.selected as isize + d).clamp(0, count as isize - 1) as usize;
                cx.notify();
                return;
            }
        }
        // Ctrl+P pins / unpins the selected app (empty query: browse view).
        if k.modifiers.control && k.key == "p"
            && let Some(Item::App(app)) = self.results.get(self.selected)
        {
            let id = app.id.clone();
            crate::state::update_config(cx, move |c| {
                if let Some(i) = c.launcher.pinned.iter().position(|p| *p == id) {
                    c.launcher.pinned.remove(i);
                } else {
                    c.launcher.pinned.push(id);
                }
            });
            self.ctx.pinned = config(cx).read(cx).get().launcher.pinned.clone();
            let keep = self.selected;
            self.refilter();
            self.selected = keep.min(self.results.len().saturating_sub(1));
            cx.notify();
            return;
        }
        if k.modifiers.control {
            match k.key.as_str() {
                "a" => {
                    self.select_all = !self.query.is_empty();
                    cx.notify();
                    return;
                }
                "c" | "x" => {
                    if !self.query.is_empty() {
                        cx.write_to_clipboard(ClipboardItem::new_string(self.query.clone()));
                    }
                    if k.key == "x" {
                        self.query.clear();
                        self.caret = 0;
                        self.select_all = false;
                        self.refilter();
                    }
                    cx.notify();
                    return;
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        self.take_selection();
                        let text = text.replace(['\n', '\r'], " ");
                        let at = self.caret_byte();
                        self.query.insert_str(at, &text);
                        self.caret += text.chars().count();
                        self.refilter();
                    }
                    cx.notify();
                    return;
                }
                _ => {}
            }
        }
        if matches!(k.key.as_str(), "left" | "right" | "home" | "end") && self.select_all {
            self.select_all = false;
            self.caret = if matches!(k.key.as_str(), "left" | "home") { 0 } else { self.query.chars().count() };
            cx.notify();
            return;
        }
        if matches!(k.key.as_str(), "backspace" | "delete") && self.select_all {
            self.take_selection();
            self.refilter();
            cx.notify();
            return;
        }
        match k.key.as_str() {
            "escape" => self.close(cx),
            "enter" => self.activate(self.selected, cx),
            "down" | "tab" if count > 0 => self.selected = (self.selected + 1) % count,
            "up" if count > 0 => self.selected = (self.selected + count - 1) % count,
            "left" => self.caret = self.caret.saturating_sub(1),
            "right" => self.caret = (self.caret + 1).min(self.query.chars().count()),
            "home" => self.caret = 0,
            "end" => self.caret = self.query.chars().count(),
            "backspace" => {
                if k.modifiers.control {
                    // Delete the word before the caret.
                    let head: String = self.query.chars().take(self.caret).collect();
                    let tail: String = self.query.chars().skip(self.caret).collect();
                    let mut head = head;
                    crate::text_field::delete_word(&mut head);
                    self.caret = head.chars().count();
                    self.query = head + &tail;
                } else if self.caret > 0 {
                    let at = self.query.char_indices().nth(self.caret - 1).map(|(i, _)| i).unwrap_or(0);
                    self.query.remove(at);
                    self.caret -= 1;
                }
                self.refilter();
            }
            "delete" => {
                if self.caret < self.query.chars().count() {
                    let at = self.caret_byte();
                    self.query.remove(at);
                    self.refilter();
                }
            }
            _ => {
                if let Some(ch) = k.key_char.as_ref().filter(|_| !k.modifiers.control && !k.modifiers.alt) {
                    self.take_selection();
                    let at = self.caret_byte();
                    self.query.insert_str(at, ch);
                    self.caret += ch.chars().count();
                    self.refilter();
                }
            }
        }
        cx.notify();
    }
}

fn fuzzy(items: Vec<Item>, needle: &str, text: impl Fn(&Item) -> String) -> Vec<Item> {
    let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
    let pattern = Pattern::parse(needle, CaseMatching::Ignore, Normalization::Smart);
    let texts: Vec<(usize, String)> = items.iter().enumerate().map(|(i, it)| (i, text(it))).collect();
    let mut scored: Vec<(usize, u32)> = pattern
        .match_list(texts.iter().map(|(i, t)| IndexedText(*i, t.as_str())), &mut matcher)
        .into_iter()
        .map(|(t, score)| (t.0, score))
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored.into_iter().map(|(i, _)| items[i].clone()).collect()
}

struct IndexedText<'a>(usize, &'a str);
impl AsRef<str> for IndexedText<'_> {
    fn as_ref(&self) -> &str {
        self.1
    }
}

fn format_number(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 && v.abs() < 1e15 { format!("{}", v.round() as i64) } else { format!("{v:.6}").trim_end_matches('0').trim_end_matches('.').to_string() }
}

fn monogram(name: &str, size: f32, palette: &MandaraPalette) -> impl IntoElement {
    let letter = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    let hue = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) % 360;
    let bg = Color::oklch(if palette.is_dark { 0.42 } else { 0.86 }, 0.09, hue as f32);
    div()
        .size(px(size))
        .flex_none()
        .rounded(px(crate::style::radius(size * LEAD_RADIUS_FRAC)))
        .bg(bg.hsla())
        .flex()
        .items_center()
        .justify_center()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(bg.best_on(Color::hex("#ffffff"), Color::hex("#101010")).hsla())
        .child(letter)
}

impl LauncherView {
    /// A cached thumbnail of `path` (wallpapers and copied images), made off
    /// the UI thread on first use.
    fn thumb(&mut self, path: &std::path::Path, cx: &mut Context<Self>) -> Option<std::path::PathBuf> {
        if let Some(hit) = self.thumbs.get(path) {
            return hit.clone();
        }
        self.thumbs.insert(path.to_path_buf(), None);
        let src = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let made = cx.background_executor().spawn({
                let src = src.clone();
                async move { crate::wallpaper::thumbnail(&src).ok() }
            });
            let made = made.await;
            let _ = this.update(cx, |this, cx| {
                this.thumbs.insert(src, made);
                cx.notify();
            });
        })
        .detach();
        None
    }

    /// The small badge of a result row (`App`, `Pinned`, `Flatpak`…).
    fn badge(&self, i: usize, item: &Item, look: &mandara_config::workflow::LauncherLook) -> Option<String> {
        match item {
            Item::Entry(e) => Some(tr(e.badge)),
            Item::Hit(h) => Some(tr(h.badge)),
            Item::App(a) if self.query.trim().is_empty() && self.ctx.pinned.iter().any(|p| p.eq_ignore_ascii_case(&a.id)) => Some(t("Pinned").to_string()),
            Item::App(a) if self.query.trim().is_empty() && i < self.ctx.pinned.len() + self.ctx.recent_count && self.ctx.usage.frecency(&format!("app:{}", a.id)) > 0 => Some(t("Recent").to_string()),
            Item::App(a) if look.origin_badges => a.origin.badge().map(tr),
            _ => None,
        }
    }

    /// The leading tile of a result.
    fn lead(&mut self, item: &Item, size: f32, palette: &MandaraPalette, cx: &mut Context<Self>) -> gpui::AnyElement {
        let tile = || div().size(px(size)).flex_none().rounded(px(crate::style::radius(size * LEAD_RADIUS_FRAC))).flex().items_center().justify_center();
        // Monochrome: the theme's symbolic icon, tinted like the bar's.
        if let Item::App(app) = item
            && crate::state::config(cx).read(cx).get().appearance.launcher_icons == mandara_config::IconStyle::Monochrome
            && let Some(symbolic) = crate::plugins::workspaces::symbolic_icon(&app.id, size * APP_ICON_FRAC, palette.on_surface)
        {
            return tile().child(symbolic).into_any_element();
        }
        match item {
            Item::App(app) => match &app.icon {
                Some(path) => div().size(px(size)).flex_none().child(img(path.clone()).size_full().object_fit(ObjectFit::Contain)).into_any_element(),
                None => monogram(&app.name, size, palette).into_any_element(),
            },
            Item::Entry(e) => tile().bg(palette.surface_container_high.hsla()).child(icon(e.icon, size * LEAD_ICON_FRAC, palette.primary)).into_any_element(),
            Item::Calc { .. } => tile().bg(palette.primary_container.hsla()).text_color(palette.primary.hsla()).child("=").into_any_element(),
            Item::Hit(h) => match &h.lead {
                Lead::Glyph(g) => tile().bg(palette.surface_container_high.hsla()).text_size(px(EMOJI_LEAD_PX * size / ROW_LEAD_PX)).child(g.clone()).into_any_element(),
                Lead::AppIcon(id) => tile().child(crate::plugins::workspaces::app_icon(id, size * APP_ICON_FRAC, palette.on_surface, false, palette)).into_any_element(),
                Lead::Icon(name) => tile().bg(palette.surface_container_high.hsla()).child(icon(name, size * LEAD_ICON_FRAC, palette.primary)).into_any_element(),
                Lead::Image(path) => match self.thumb(path, cx) {
                    Some(thumb) => img(thumb).size(px(size)).flex_none().rounded(px(crate::style::radius(THUMB_RADIUS))).object_fit(ObjectFit::Cover).into_any_element(),
                    None => tile().bg(palette.surface_container_high.hsla()).child(icon("image", size * LEAD_ICON_FRAC, palette.on_surface_variant)).into_any_element(),
                },
            },
        }
    }
}

impl LauncherView {
    /// Whether the surface has its real size yet (the focused output's,
    /// or — after a few frames — any output's, should the compositor have
    /// opened it elsewhere); remembers the output it settled on.
    fn settle(&mut self, window: &Window, cx: &App) -> bool {
        if self.output.is_some() || self.placed {
            return true;
        }
        let vp = window.viewport_size();
        let outputs = cx.try_global::<Outputs>().map(|o| o.0.as_slice()).unwrap_or_default();
        let focused = focused_output(cx);
        let found = if viewport_settled(window, self.expected) {
            focused.filter(|o| same_size(vp, (o.width as f32, o.height as f32))).or_else(|| outputs.iter().find(|o| same_size(vp, (o.width as f32, o.height as f32))).cloned())
        } else if self.settle_frames >= SETTLE_FALLBACK_FRAMES {
            outputs.iter().find(|o| same_size(vp, (o.width as f32, o.height as f32))).cloned()
        } else {
            self.settle_frames += 1;
            return false;
        };
        match found {
            Some(o) => {
                self.output = Some(o.name);
                true
            }
            // Settled at the expected size but no output list (tests,
            // early startup): fine, just no per-output details.
            None => viewport_settled(window, self.expected),
        }
    }
}

impl Render for LauncherView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::launcher_place::{Join, Rect};
        use mandara_config::surfaces::{PanelAnimation, ranges};
        use mandara_config::workflow::{LauncherEdge, SearchPosition};
        // Counted in `diagnostics frames` while open (its own interval key).
        let _frame = crate::plugins::diagnostics::frame(cx.entity_id().as_u64());
        if !self.settle(window, cx) && !self.closing {
            window.request_animation_frame();
            return div().id("launcher-root").size_full().track_focus(&self.focus).into_any_element();
        }
        let palette = mandara_core::frame_palette(window, cx);
        let f = fonts(cx);
        let cfg = config(cx).read(cx).snapshot();
        let look = cfg.launcher.look.clone();
        let viewport = window.viewport_size();
        let (sw, sh) = (f32::from(viewport.width), f32::from(viewport.height));
        let row_h = if look.compact { ROW_H_COMPACT } else { ROW_H };
        let lead_px = if look.compact { ROW_LEAD_COMPACT_PX } else { ROW_LEAD_PX };
        // Rows: as many results as configured, as fit the height setting.
        let chrome = PAD * 2.0 + LIST_GAP + SEARCH_H;
        let fit = ((look.size().1 - chrome) / row_h).floor().max(0.0) as usize;
        let max_rows = cfg.launcher.max_results.clamp(3, 12).min(fit).max(MIN_ROWS);
        let grid = self.grid_columns(cx);
        let n = self.results.len().min(MAX_RESULTS);
        // Visible window and list height (the width comes from the placement).
        let (list_h, first_visible, shown, cell_h) = match grid {
            Some(cols) => {
                let total_rows = n.div_ceil(cols).max(1);
                let vis_rows = (((max_rows as f32 * ROW_H) / GRID_TILE_H).floor() as usize).max(MIN_ROWS).min(total_rows);
                let first_row = (self.selected / cols).saturating_sub(vis_rows - 1);
                ((vis_rows as f32) * GRID_TILE_H, first_row * cols, vis_rows * cols, GRID_TILE_H)
            }
            None => {
                let rows = n.min(max_rows);
                (if rows == 0 { row_h } else { rows as f32 * row_h }, self.selected.saturating_sub(max_rows - 1), max_rows, row_h)
            }
        };
        let full_h = chrome + list_h;
        // Where it rests for this placement, bar style, dock and output.
        let output = self.output.as_deref();
        let bar_here = output.is_none_or(|o| cfg.bar_on_output(o));
        let place = crate::launcher_place::for_config(&cfg, (sw, sh), full_h, bar_here, crate::launcher_place::button_on(output, cx));
        let width = place.rect.w;
        let full_h = place.rect.h;
        let cell_w = match grid {
            Some(cols) => (width - PAD * 2.0) / cols as f32,
            None => width - PAD * 2.0,
        };
        // Springs: the panels' spring (or the user's own), the panels'
        // animation style; reduced motion snaps (inside the step).
        let (spring, custom) = crate::shell::spring_for(DRAWER_SPRING, &cfg.panels.spring);
        let instant = cfg.panels.animation == PanelAnimation::None;
        if !self.placed {
            tracing::debug!(target: "mandara::launcher", "first frame {:.1} ms after opening ({:?} on {:?})", self.created.elapsed().as_secs_f32() * 1000.0, look.position, self.output);
            self.rest.snap_to(place.rect.to_bounds());
            self.open.snap_to(0.0);
            self.placed = true;
        }
        self.rest.set_config(spring);
        self.open.set_config(if self.closing { SpringConfig::critical(CLOSE_STIFFNESS) } else { spring });
        self.rest.set_instant(instant);
        self.open.set_instant(instant);
        self.rest.set_target(place.rect.to_bounds());
        self.open.set_target(if self.closing { 0.0 } else { 1.0 });
        let rel = self.selected.saturating_sub(first_visible);
        match grid {
            Some(cols) => {
                self.highlight.set_target((rel / cols) as f32 * cell_h);
                self.highlight_x.set_target((rel % cols) as f32 * cell_w);
            }
            None => {
                self.highlight.set_target(rel as f32 * cell_h);
                self.highlight_x.set_target(0.0);
            }
        }
        let dt = self.clock.tick(Instant::now());
        let a = crate::shell::step_spring(&mut self.rest, custom, dt);
        let opening = crate::shell::step_spring(&mut self.open, custom, dt);
        let b = self.highlight.step(dt);
        let c = self.highlight_x.step(dt);
        if self.closing && !opening {
            window.remove_window();
        } else if a || opening || b || c {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        let rest = Rect::from_bounds(self.rest.value());
        let opened = self.open.value();
        // Grows from its start (the launcher pill, a sliver on the bar's
        // edge, a slide's offset or a smaller centered card) to its rest.
        let cur = place.start.lerp(rest, opened);
        let progress = opened.clamp(0.0, 1.0);
        let (x, y, h) = (cur.x, cur.y, cur.h);
        self.rect = (x, y, cur.w, h);
        let search_top = match look.search_position {
            SearchPosition::Top => true,
            SearchPosition::Bottom => false,
            SearchPosition::Auto => place.edge != Some(LauncherEdge::Bottom),
        };

        let ignore_mouse = look.ignore_mouse;
        let mut rows_el: Vec<gpui::AnyElement> = Vec::new();
        let items: Vec<(usize, Item)> = self.results.iter().enumerate().skip(first_visible).take(shown).map(|(i, it)| (i, it.clone())).collect();
        for (i, item) in items {
            let badge = self.badge(i, &item, &look);
            let hover = cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && !ignore_mouse && !this.wheel_hold {
                    this.selected = i;
                    cx.notify();
                }
            });
            let click = cx.listener(move |this, _: &ClickEvent, _, cx| this.activate(i, cx));
            if grid.is_some() {
                let lead = self.lead(&item, GRID_ICON, &palette, cx);
                let title = match &item {
                    Item::App(a) => a.name.clone(),
                    Item::Entry(e) => e.label.clone(),
                    Item::Hit(h) => h.label.clone(),
                    Item::Calc { value, .. } => format_number(*value),
                };
                rows_el.push(
                    div()
                        .id(("tile", i))
                        .w(px(cell_w))
                        .h(px(cell_h))
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(6.0))
                        .cursor_pointer()
                        .child(lead)
                        .child(div().w_full().px(px(6.0)).text_center().truncate().text_size(px(text::LABEL)).child(title))
                        .on_hover(hover)
                        .on_click(click)
                        .into_any_element(),
                );
                continue;
            }
            let lead = self.lead(&item, lead_px, &palette, cx);
            let (title, sub) = match &item {
                Item::App(app) => (app.name.clone(), app.comment.clone()),
                Item::Entry(e) => (e.label.clone(), e.detail.clone()),
                Item::Hit(h) => (h.label.clone(), h.detail.clone()),
                Item::Calc { expr, value } => (format_number(*value), tf("{expression} · Enter copies", &[("expression", expr)])),
            };
            rows_el.push(
                div()
                    .id(("row", i))
                    .h(px(row_h))
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .cursor_pointer()
                    .child(lead)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .child(div().truncate().text_size(px(text::BODY)).child(title))
                            .when(!look.compact && !sub.is_empty(), |el| el.child(div().truncate().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(sub))),
                    )
                    .when_some(badge, |el, b| {
                        el.child(div().flex_none().px(px(8.0)).h(px(20.0)).rounded_full().flex().items_center().bg(palette.on_surface.with_alpha(0.06).hsla()).text_size(px(10.5)).text_color(palette.on_surface_variant.hsla()).child(b))
                    })
                    .on_hover(hover)
                    .on_click(click)
                    .into_any_element(),
            );
        }
        // The wheel (or a touchpad) steps the selection a row at a time, and
        // the window follows it like the arrow keys.
        let wheel_step = grid.unwrap_or(1);
        let wheel_row = cell_h;
        let list = div()
            .relative()
            .h(px(list_h))
            .on_scroll(cx.listener(move |this, e: &ScrollEvent, _, cx| {
                let count = this.results.len().min(MAX_RESULTS);
                if count == 0 {
                    return;
                }
                // A wheel notch is one row; a touchpad's pixels add up to
                // rows. Down is negative.
                let rows = match e.delta {
                    gpui::ScrollDelta::Lines(l) => -l.y.signum(),
                    gpui::ScrollDelta::Pixels(p) => {
                        this.wheel -= f32::from(p.y);
                        let whole = (this.wheel / wheel_row).trunc();
                        this.wheel -= whole * wheel_row;
                        whole
                    }
                };
                if rows == 0.0 {
                    return;
                }
                let next = this.selected as isize + rows as isize * wheel_step as isize;
                this.selected = next.clamp(0, count as isize - 1) as usize;
                this.wheel_hold = true;
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, _: &gpui::MouseMoveEvent, _, _| this.wheel_hold = false))
            .child(
                div()
                    .absolute()
                    .left(px(self.highlight_x.value()))
                    .top(px(self.highlight.value()))
                    .w(px(cell_w))
                    .h(px(cell_h))
                    .rounded(px(crate::style::radius(16.0)))
                    .bg(palette.on_surface.with_alpha(0.08).hsla())
                    .when(self.results.is_empty(), |el| el.opacity(0.0)),
            )
            .when(self.results.is_empty(), |el| {
                el.child(div().size_full().flex().items_center().justify_center().text_color(palette.on_surface_variant.hsla()).child(t("No results")))
            })
            .child(div().flex().when(grid.is_some(), |el| el.flex_wrap()).when(grid.is_none(), |el| el.flex_col()).children(rows_el));
        let placeholder = self.query.is_empty();
        let hint = {
            let p = &cfg.launcher.providers;
            let mut parts = vec![t("Apps, settings, commands").to_string(), t("“/” slash commands").to_string(), t("“=” calculate").to_string()];
            if p.emoji.enabled && !p.emoji.prefix.is_empty() {
                parts.push(tf("“{prefix}” emoji", &[("prefix", &p.emoji.prefix)]));
            }
            if p.windows.enabled && !p.windows.prefix.is_empty() {
                parts.push(tf("“{prefix}” windows", &[("prefix", &p.windows.prefix)]));
            }
            parts.join(" · ")
        };
        let search = div()
            .h(px(SEARCH_H))
            .px(px(16.0))
            .rounded(px(crate::style::radius(SEARCH_H / 2.0)))
            .bg(palette.surface_container_high.hsla())
            .flex()
            .items_center()
            .gap(px(10.0))
            .id("launcher-search")
            .cursor_text()
            .border_2()
            .border_color(palette.primary.with_alpha(0.55).hsla())
            .child(icon("search", 18.0, palette.on_surface_variant))
            .child({
                // Blinking caret, solid for half a second after each edit.
                let caret_on = (self.blink_from.elapsed().as_millis() / 530) % 2 == 0;
                let caret = div().w(px(2.0)).h(px(19.0)).rounded(px(crate::style::radius(1.0))).flex_none().bg(palette.primary.hsla()).when(!caret_on, |el| el.opacity(0.0));
                let bounds = self.text_bounds.clone();
                let field = div()
                    .flex_1()
                    .min_w(px(0.0))
                    .relative()
                    .h_full()
                    .flex()
                    .items_center()
                    .cursor_text()
                    .text_size(px(text::BODY + 1.0))
                    .child(gpui::canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {}).absolute().inset_0());
                if placeholder {
                    field.child(caret).child(div().pl(px(2.0)).truncate().text_color(palette.on_surface_variant.with_alpha(0.7).hsla()).child(hint))
                } else if self.select_all {
                    field.child(div().px(px(1.0)).rounded(px(crate::style::radius(3.0))).bg(palette.primary.with_alpha(0.28).hsla()).text_color(palette.on_surface.hsla()).child(self.query.clone()))
                } else {
                    let (head, tail) = self.query.split_at(self.caret_byte());
                    field
                        .text_color(palette.on_surface.hsla())
                        .when(!head.is_empty(), |el| el.child(head.to_string()))
                        .child(caret)
                        .when(!tail.is_empty(), |el| el.child(tail.to_string()))
                }
            })
            // Clicking the field focuses it and puts the caret where you clicked.
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &gpui::MouseDownEvent, window, cx| {
                this.focus.focus(window, cx);
                if e.click_count >= 2 {
                    this.select_all = !this.query.is_empty();
                } else {
                    this.caret_at(e.position.x, window, cx);
                }
                cx.notify();
            }));

        // Hover-leave only (clicks are hit-tested by the element tree): slack
        // on every side, so drifting just past an edge or onto a flare
        // doesn't start the close timer.
        let inside = |this: &Self, p: gpui::Point<gpui::Pixels>, slack: f32| {
            let (rx, ry, rw, rh) = this.rect;
            crate::dismiss::Rect::new(rx, ry, rw, rh).contains(f32::from(p.x), f32::from(p.y), slack)
        };
        // Edge drawers grow concave flares beside the drawer (shrinking with
        // it): they count as the launcher, not as outside.
        let flare = place.flare.min(h);
        // The content rests where the launcher rests, pinned to the edge it
        // grows from; the shape reveals it.
        let body_y = if place.edge == Some(LauncherEdge::Bottom) { rest.bottom() - full_h } else { rest.y };
        let body = div()
            .absolute()
            .left(px(rest.x - x))
            .top(px(body_y - y))
            .w(px(rest.w))
            .h(px(full_h))
            .p(px(PAD))
            .flex()
            .gap(px(LIST_GAP))
            .opacity(((progress - 0.3) / 0.5).clamp(0.0, 1.0));
        // The search field sits on the side the drawer comes from (or as
        // configured), and leads as the drawer opens.
        let body = if search_top { body.flex_col().child(search).child(list) } else { body.flex_col().justify_end().child(list).child(search) };
        // Fills as panels: joined to the bar it takes the bar's own fill
        // (tint, opacity) so the join is seamless; otherwise the panel fill.
        let frame = frame_color(&palette);
        let panel_alpha = ranges::OPACITY.clamp(cfg.panels.opacity, 1.0);
        let bar_opacity = ranges::OPACITY.clamp(cfg.bar.opacity, 1.0);
        let joined = place.keep_bright.is_some() && place.join == Join::Flares;
        let fill = if joined {
            let tinted = if cfg.bar.tint > 0.0 { frame.mix(palette.primary, ranges::BAR_TINT.clamp(cfg.bar.tint, 0.0)) } else { frame };
            tinted.with_alpha(frame.alpha() * bar_opacity * panel_alpha).hsla()
        } else {
            frame.with_alpha(frame.alpha() * panel_alpha).hsla()
        };
        // A hairline into an opaque bar hides the anti-aliased seam.
        let seam = if joined && bar_opacity >= OPAQUE_ALPHA && !cfg.bar.frame_floats() { 1.0 } else { 0.0 };
        let radius = crate::style::panel_radius(&cfg.bar);
        let scale = window.scale_factor();
        let surface = match (place.join, place.edge) {
            (Join::Flares | Join::Flush, Some(edge)) => {
                let (edge, sy) = match edge {
                    LauncherEdge::Top => (Edge::Top, y - seam),
                    LauncherEdge::Bottom => (Edge::Bottom, y),
                };
                edge_drawer_snapped(edge, x, sy, cur.w, h + seam, radius, flare, fill, scale)
            }
            _ => {
                // Morphing out of a floating pill: the pill's radius first.
                let r = place.start_radius.map_or(radius, |r0| r0 + (radius - r0) * progress);
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(cur.w))
                    .h(px(h))
                    .rounded(px(r))
                    .bg(fill)
                    .border_1()
                    .border_color(crate::plugins::panel_fx::surface_edge(&palette, cfg.theme.adjustments.rim_intensity).hsla())
                    .shadow(crate::plugins::panel_fx::elevation_scaled(&palette, progress, cfg.panels.shadow_strength))
                    .into_any_element()
            }
        };
        // The center card (and a slide) fade in whole.
        let surface = if place.fades && progress < 1.0 { div().absolute().size_full().opacity(progress).child(surface).into_any_element() } else { surface };
        let dim = look.dim.clamp(0.0, DIM_MAX) * progress;
        // Joined to the bar, the dim leaves the bar's band bright.
        let dim_el = (dim > 0.0).then(|| {
            let el = div().absolute().left_0().right_0().bg(gpui::black().opacity(dim));
            match place.keep_bright {
                Some((by, bh)) if by <= 0.0 => el.top(px(bh)).bottom_0(),
                Some((_, bh)) => el.top_0().bottom(px(bh)),
                None => el.top_0().bottom_0(),
            }
        });
        let hover_close = place.hover_close;

        div()
            .id("launcher-root")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .font_family(f.ui.clone())
            .text_start()
            .font_weight(crate::style::body_weight())
            .text_color(palette.on_surface.hsla())
            .text_size(px(13.0 * cfg.typography.scale))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.on_key(e, cx)))
            .on_mouse_move(cx.listener(move |this, e: &gpui::MouseMoveEvent, _, cx| {
                if inside(this, e.position, LEAVE_SLACK_PX) {
                    this.entered = true;
                    this.leave_task = None;
                } else if this.entered && this.leave_task.is_none() && hover_close {
                    // Grace before closing (`panels.leave_delay_ms`).
                    let delay = crate::state::config(cx).read(cx).snapshot().panels.leave_delay();
                    this.leave_task = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(delay).await;
                        let _ = this.update(cx, |this, cx| this.close(cx));
                    }));
                }
            }))
            // Click outside the launcher dismisses: a full-surface catcher
            // (with the dim), the drawer (and its flares) occlude it.
            .child(
                div()
                    .id("launcher-outside")
                    .absolute()
                    .inset_0()
                    .children(dim_el)
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| this.close(cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| this.close(cx))),
            )
            .child(surface)
            .child(div().id("launcher-frame").absolute().left(px(x - flare)).top(px(y)).w(px(cur.w + 2.0 * flare)).h(px(h)).occlude())
            .child(
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(cur.w))
                    .h(px(h))
                    .overflow_hidden()
                    .when(place.fades, |el| el.opacity(progress))
                    .child(body),
            )
            .into_any_element()
    }
}

// ── Session dock ─────────────────────────────────────────────────────────
// The power / session menu (dock or grid, configurable entries,
// hold-to-confirm) lives in `crate::powerctl::session_menu`.

pub use crate::powerctl::session_menu::SessionView;

// ── Window management ────────────────────────────────────────────────────

#[derive(Default)]
struct Overlays {
    launcher: Option<WindowHandle<LauncherView>>,
    session: Option<WindowHandle<SessionView>>,
    picker: Option<WindowHandle<crate::wallpaper::PickerView>>,
}

impl gpui::Global for Overlays {}

fn is_open<V: 'static>(handle: &Option<WindowHandle<V>>, cx: &App) -> bool {
    handle.as_ref().is_some_and(|h| cx.windows().iter().any(|w: &AnyWindowHandle| w.window_id() == h.window_id()))
}

/// Opens / closes overlay surfaces as the shell state changes.
pub fn init_overlays(cx: &mut App) {
    cx.set_global(Overlays::default());
    let shell = shell(cx);
    cx.observe(&shell, |shell, cx| {
        let (launcher, session, picker) = (shell.read(cx).launcher_open, shell.read(cx).session_open, shell.read(cx).wallpaper_open);
        let mut overlays = std::mem::take(cx.global_mut::<Overlays>());
        let (w, h) = if (launcher && !is_open(&overlays.launcher, cx)) || (session && !is_open(&overlays.session, cx)) || (picker && !is_open(&overlays.picker, cx)) {
            output_size(cx)
        } else {
            (1920.0, 1080.0)
        };
        if launcher && !is_open(&overlays.launcher, cx) {
            overlays.launcher = cx
                .open_window(overlay_spec(SurfaceRole::Launcher, w, h), |window, cx| cx.new(|cx| LauncherView::new(window, cx)))
                .map_err(|e| tracing::error!("launcher: {e:#}"))
                .ok();
        }
        if session && !is_open(&overlays.session, cx) {
            overlays.session = cx
                .open_window(overlay_spec(SurfaceRole::Lock, w, h), |window, cx| cx.new(|cx| SessionView::new(window, cx)))
                .map_err(|e| tracing::error!("session: {e:#}"))
                .ok();
        }
        if picker && !is_open(&overlays.picker, cx) {
            overlays.picker = cx
                .open_window(overlay_spec(SurfaceRole::Launcher, w, h), move |window, cx| cx.new(|cx| crate::wallpaper::PickerView::new((w, h), window, cx)))
                .map_err(|e| tracing::error!("wallpaper picker: {e:#}"))
                .ok();
        }
        cx.set_global(overlays);
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_plugins::PluginRegistry;

    /// The built-in registry with every plugin switched on.
    fn fixture() -> (PluginRegistry, mandara_config::ShellConfig, Vec<Item>) {
        let mut registry = PluginRegistry::default();
        crate::plugins::register_builtin(&mut registry);
        let mut cfg = mandara_config::ShellConfig::default();
        cfg.plugins.enabled = registry.manifests().map(|m| m.id.clone()).collect();
        cfg.plugins.disabled.clear();
        let index = build_index_with(&registry, &cfg);
        (registry, cfg, index)
    }

    fn entries(index: &[Item]) -> impl Iterator<Item = &Entry> {
        index.iter().filter_map(|i| match i {
            Item::Entry(e) => Some(e),
            _ => None,
        })
    }

    fn effect(results: &[Item], query: &str) -> String {
        describe(&plan(results.first().expect("a result"), query))
    }

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    /// A command line names a registered command; complete lines resolve
    /// with valid arguments, Studio and panel targets exist.
    fn assert_line(registry: &PluginRegistry, words: &[String], complete: bool, label: &str) {
        let commands = &registry.commands;
        assert!(commands.all().iter().any(|c| starts_with_line(words, c.path)), "`{label}` runs `{}`, which is not a command", words.join(" "));
        if complete {
            if let Err(err) = commands.resolve(words) {
                panic!("`{label}` runs `{}`: {err}", words.join(" "));
            }
        }
        let ids: Vec<&str> = registry.manifests().map(|m| m.id.as_str()).collect();
        match words.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["studio", target] => {
                let mut parts = target.split('/');
                let section = parts.next().unwrap();
                if section == "plugins" && let Some(id) = parts.next() {
                    assert!(ids.contains(&id), "`{label}` opens Studio on unknown plugin {id}");
                    assert!(matches!(parts.next(), None | Some("settings")), "`{label}`: bad deep link {target}");
                } else {
                    assert!(STUDIO_SECTIONS.iter().any(|(slug, ..)| *slug == section), "`{label}` opens unknown Studio section {section}");
                }
            }
            ["panel", "open", id] => assert!(ids.contains(id), "`{label}` opens the panel of unknown plugin {id}"),
            _ => {}
        }
    }

    #[test]
    fn off_plugins_leave_the_launcher() {
        let (registry, mut cfg, _) = fixture();
        let mentions = |cfg: &mandara_config::ShellConfig| {
            entries(&build_index_with(&registry, cfg)).filter(|e| format!("{} {}", e.label, e.keywords).to_lowercase().contains("prayer")).count()
        };
        assert!(mentions(&cfg) > 0, "on: plugin entry and its commands");
        mandara_plugins::lifecycle::remove(&mut cfg, crate::plugins::prayer::ID, "Prayer times").unwrap();
        assert_eq!(mentions(&cfg), 0, "removed: no entries, no commands");
        mandara_plugins::lifecycle::restore(&mut cfg, crate::plugins::prayer::ID, None);
        assert!(mentions(&cfg) > 0, "restored");
    }

    #[test]
    fn every_entry_runs_a_real_command() {
        let (registry, cfg, index) = fixture();
        assert!(entries(&index).count() > 150);
        for e in entries(&index) {
            let item = Item::Entry(e.clone());
            match plan(&item, "") {
                Effect::Commands(lines) => {
                    assert!(!lines.is_empty(), "{}", e.label);
                    for words in &lines {
                        assert_line(&registry, words, true, &e.label);
                    }
                }
                Effect::Prompt(query) => {
                    let Run::Command { words, args } = &e.run else {
                        // Slash commands with `{query}` prompt for it.
                        assert!(matches!(e.run, Run::Slash { .. }), "{} prompts unexpectedly", e.label);
                        continue;
                    };
                    assert!(args.iter().any(|a| a.required), "{} prompts without required arguments", e.label);
                    assert_eq!(query, format!("/{} ", words.join(" ")));
                    assert_line(&registry, words, false, &e.label);
                }
                Effect::Url(_) | Effect::Shell(_) => assert!(matches!(e.run, Run::Slash { .. })),
                other => panic!("{} → {}", e.label, describe(&other)),
            }
        }
        // Slash commands (`mandara:`) and skills of the default config run real commands.
        for sc in &cfg.launcher.commands {
            if let Effect::Commands(lines) = slash_effect(sc, "buy milk") {
                lines.iter().for_each(|w| assert_line(&registry, w, true, &sc.trigger));
            }
        }
        for sk in &cfg.launcher.skills {
            for step in &sk.steps {
                assert_line(&registry, &words(step), true, &sk.name);
            }
        }
    }

    #[test]
    fn every_command_and_studio_section_is_listed() {
        let (registry, _, index) = fixture();
        let lines: Vec<Vec<String>> = entries(&index)
            .filter_map(|e| match &e.run {
                Run::Command { words, .. } => Some(words.clone()),
                _ => None,
            })
            .collect();
        for c in registry.commands.all() {
            if c.path.starts_with("debug ") || HIDDEN_COMMANDS.contains(&c.path) {
                continue;
            }
            assert!(lines.iter().any(|w| starts_with_line(w, c.path)), "`{}` is missing from the launcher", c.path);
        }
        for (slug, ..) in STUDIO_SECTIONS {
            assert!(lines.iter().any(|w| w == &strings(&["studio", slug])), "Studio section {slug} is missing");
            assert!(crate::plugins::core_commands::STUDIO_SECTION_ARG.contains(&format!(" {slug},")) || crate::plugins::core_commands::STUDIO_SECTION_ARG.contains(&format!(" {slug})")) || crate::plugins::core_commands::STUDIO_SECTION_ARG.contains(&format!("({slug},")), "`studio` command help doesn't list {slug}");
        }
        for m in registry.manifests().filter(|m| m.id != crate::plugins::session::LAUNCHER_ID) {
            assert!(lines.iter().any(|w| w == &strings(&["studio", &format!("plugins/{}/settings", m.id)])), "{} has no settings entry", m.id);
        }
        // Every theme has an entry.
        for theme in catalog::all() {
            assert!(lines.iter().any(|w| w == &strings(&["theme", "set", &theme.id])), "theme {} is missing", theme.id);
        }
        // The lists steering execution name real commands.
        for path in HIDDEN_COMMANDS.iter().chain(NO_EXPAND).chain(OUTPUT_COMMANDS).chain(AFTER_CLOSE) {
            assert!(registry.commands.all().iter().any(|c| c.path == *path || c.path.starts_with(&format!("{path} ")) || path.starts_with(&format!("{} ", c.path))), "`{path}` is not a command");
        }
    }

    #[test]
    fn exact_command_names_rank_first() {
        let (_, _, index) = fixture();
        let first = |q: &str| {
            let results = search(q, &[], &index);
            item_names(results.first().unwrap_or_else(|| panic!("no results for {q}"))).1
        };
        assert_eq!(first("theme mode dark"), "theme mode dark");
        assert_eq!(first("wallpaper random"), "wallpaper random");
        assert_eq!(first("weather"), "panel open weather");
        assert_eq!(first("prayer today"), "prayer today");
        assert_eq!(first("> notifications dnd on"), "notifications dnd on");
        assert_eq!(first(">dark appearance"), "theme mode dark");
        // A web search is always offered last.
        let results = search("zzqx unlikely words", &[], &index);
        assert!(effect(&results[results.len() - 1..], "").starts_with("url: https://duckduckgo.com/?q=zzqx%20unlikely%20words"));
    }

    #[test]
    fn snipping_words_find_the_screenshot_tool() {
        let (_, _, index) = fixture();
        for q in ["snip", "snipping tool", "Snipping Tool", "printscreen"] {
            let results = search(q, &[], &index);
            let line = item_names(results.first().unwrap_or_else(|| panic!("no results for {q}"))).1;
            assert!(line == "screenshot region" || line == plugin_line(crate::plugins::screenshot::ID), "{q}: first result runs `{line}`");
        }
    }

    #[test]
    fn arguments_are_prompted_for_and_passed_on() {
        let (_, _, index) = fixture();
        // Choosing a command that needs arguments asks for them…
        let results = search("audio volume set", &[], &index);
        assert_eq!(effect(&results, "audio volume set"), "prompt: /audio volume set ");
        // …and what's typed after the line is passed on (any case, `/` or `>`).
        for q in ["/audio volume set 40", "/Audio Volume SET 40", ">audio volume set 40"] {
            let results = search(q, &[], &index);
            assert_eq!(effect(&results, q), "command: audio volume set 40", "{q}");
        }
        let q = "/network wifi connect My Home Net";
        assert_eq!(effect(&search(q, &[], &index), q), "command: network wifi connect My Home Net");
        // Enum arguments are listed as their own entries.
        let modes: Vec<String> = search("/theme mode", &[], &index).iter().map(|i| item_names(i).1).collect();
        assert_eq!(modes, ["theme mode light", "theme mode dark", "theme mode toggle"]);
        let q = "/soundscapes volume sea";
        assert_eq!(effect(&search(q, &[], &index), q), "prompt: /soundscapes volume sea ");
        let q = "/soundscapes volume sea 40";
        assert_eq!(effect(&search(q, &[], &index), q), "command: soundscapes volume sea 40");
        // Optional arguments: run bare, or with what was typed.
        let q = "/audio volume up 10";
        assert_eq!(effect(&search(q, &[], &index), q), "command: audio volume up 10");
        assert_eq!(effect(&search("/audio volume up", &[], &index), "/audio volume up"), "command: audio volume up");
        // Plain text only takes typed arguments that fit the command.
        let q = "audio volume up 10";
        assert_eq!(effect(&search(q, &[], &index), q), "command: audio volume up 10");
        let q = "lock screen";
        assert_eq!(effect(&search(q, &[], &index), q), "command: session run lock");
        // …and choosing `lock [mode]` for that text locks plainly.
        let lock = search(q, &[], &index).into_iter().find(|i| item_names(i).1 == "lock").expect("lock command");
        assert_eq!(describe(&plan(&lock, q)), "command: lock");
    }

    #[test]
    fn slash_commands_substitute_and_quote() {
        let (_, _, index) = fixture();
        let q = "/g rust & “gpui” 100%";
        assert_eq!(effect(&search(q, &[], &index), q), "url: https://duckduckgo.com/?q=rust%20%26%20%E2%80%9Cgpui%E2%80%9D%20100%25");
        assert_eq!(effect(&search("/g", &[], &index), "/g"), "prompt: /g ");
        let q = "/todo buy milk";
        assert_eq!(effect(&search(q, &[], &index), q), "command: todo add buy milk");
        // `#Topic` passes through to the plugin, which files the task there.
        let q = "/todo #Work send the report";
        assert_eq!(effect(&search(q, &[], &index), q), "command: todo add #Work send the report");
        let q = "/note #Recipes koshary";
        assert_eq!(effect(&search(q, &[], &index), q), "command: notes add #Recipes koshary");
        let exec = |action: &str, query: &str| {
            let sc = mandara_config::SlashCommand { trigger: "x".into(), action: action.into(), ..Default::default() };
            describe(&slash_effect(&sc, query))
        };
        assert_eq!(exec("exec:notify-send {query}", "it's $HOME"), r"exec: notify-send 'it'\''s $HOME'");
        assert_eq!(exec("exec:notify-send '{query}'", "a b"), "exec: notify-send 'a b'");
        assert_eq!(exec("exec:notify-send \"{query}\"", "a b"), "exec: notify-send 'a b'");
        assert!(exec("ftp:x", "q").starts_with("error:"));
        // Skills run their steps in order.
        let results = search("/wind", &[], &index);
        assert_eq!(effect(&results, "/wind"), "command: notifications dnd off ; pomodoro reset ; theme mode dark");
    }

    #[test]
    fn calculator_results() {
        let (_, _, index) = fixture();
        assert_eq!(effect(&search("=2+3*4", &[], &index), "=2+3*4"), "copy: 14");
        assert_eq!(effect(&search("=(1+2)^2", &[], &index), ""), "copy: 9");
        // A bare expression leads the plain list.
        assert_eq!(effect(&search("sqrt(16)+1", &[], &index), ""), "copy: 5");
        assert!(search("=2+", &[], &index).is_empty());
        assert_eq!(format_number(0.1 + 0.2), "0.3");
    }

    #[test]
    fn query_replies_are_summarized() {
        let v = serde_json::json!([{ "name": "Fajr", "time": "04:51" }, { "name": "Dhuhr", "time": "11:42" }]);
        assert_eq!(summarize(&v), "Fajr — 04:51\nDhuhr — 11:42");
        let v = serde_json::json!([{ "prayer": "Fajr", "arabic": "الفجر", "time": "05:43" }]);
        assert_eq!(summarize(&v), "Fajr — 05:43");
        assert_eq!(summarize(&serde_json::json!({ "mode": "dark", "theme": null })), "mode: dark\ntheme: —");
        assert_eq!(summarize(&serde_json::json!("pong")), "pong");
        let many: Vec<u32> = (0..15).collect();
        assert!(summarize(&serde_json::json!(many)).ends_with("…and 5 more"));
    }

    #[test]
    fn the_launcher_stacks_above_a_bottom_dock() {
        use crate::launcher_place::{Join, for_config};
        use mandara_config::workflow::{DOCK_PLUGIN_ID, LauncherPosition};
        const SCREEN: (f32, f32) = (1920.0, 1080.0);
        let mut cfg = mandara_config::ShellConfig::default();
        cfg.plugins.enabled.retain(|p| p != DOCK_PLUGIN_ID);
        // No dock: the shipped drawer pours out of the bottom screen edge.
        let off = for_config(&cfg, SCREEN, 400.0, true, None);
        assert_eq!((off.join, off.rect.bottom()), (Join::Flares, SCREEN.1));
        // A bottom dock: a floating card right above it.
        cfg.plugins.enabled.push(DOCK_PLUGIN_ID.into());
        let dock = cfg.dock_bottom_clearance().expect("a bottom dock");
        for pos in [LauncherPosition::BottomLeft, LauncherPosition::BottomCenter, LauncherPosition::BottomRight, LauncherPosition::Center] {
            cfg.launcher.look.position = pos;
            let on = for_config(&cfg, SCREEN, 400.0, true, None);
            assert_eq!(on.join, Join::Card, "{pos:?}");
            assert!(on.rect.bottom() <= SCREEN.1 - dock + 0.01, "{pos:?} under the dock: {on:?}");
        }
        assert!(!cfg.launcher_hover_enabled(), "click-only while the dock is on");
        // Top launchers never meet a bottom dock.
        cfg.launcher.look.position = LauncherPosition::TopCenter;
        assert_eq!(for_config(&cfg, SCREEN, 400.0, true, None).join, Join::Flares);
    }
}
