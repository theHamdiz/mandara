//! Studio → Plugins: the plugin detail and settings modals.
//!
//! Clicking a plugin card opens its **product page** over Studio: a hero
//! over the plugin's blurred cover with the icon, name, maker and the
//! Get / On button; an info strip (category, version, access, where it
//! fits); the cover art band with "more like this"; the description; what's
//! new (an update); what it may access (built-ins: the system services they
//! use; third-party: their manifest permissions); its settings; where it
//! fits (add to the bar or sidebar); and managing it (homepage, folder,
//! remove / restore, uninstall). The **settings modal** lists every setting that
//! belongs to the plugin — its `[plugins.settings.<id>]` table, the shell
//! sections it owns (weather → units, wallpaper → the whole wallpaper
//! section, …) and, for third-party plugins, the `[[settings]]` its
//! plugin.toml declares — with the same live controls as every other page.
//!
//! Both spring in over a dimmed scrim, close on Esc, a click outside or the
//! close button, and keep keyboard focus inside while open.

use mandara_ui::bidi::Bidi as _;
use std::cell::RefCell;
use std::collections::HashMap;

use gpui::{
    AnyElement, ClickEvent, Context, FocusHandle, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px,
};
use mandara_config::ShellConfig;
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui};
use mandara_i18n::{t, tf, tk, tr};
use mandara_plugins::{ManifestSetting, PluginManifest, PluginOrigin, PluginRegistry, SettingKind, Zone};
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;
use super::designer::{Lane, NO_BAR, SIDEBAR_CAPABLE, place};
use mandara_ui::plugins::lifecycle::Action;
use super::market_page::reload_plugins;
use super::store_page as store;
use super::{PLUGIN_CATEGORIES, StudioView, grid, plugin_icon, plugin_tile};
use crate::registry::{self, Control, Section, Setting};
use crate::tokens::*;

/// Which modal a plugin is shown in.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ModalView {
    Detail,
    Settings,
}

/// Modal state, owned by [`StudioView`].
pub struct PluginModal {
    /// The plugin shown (kept while the modal animates out).
    target: Option<(String, ModalView)>,
    open: bool,
    appear: SpringValue<f32>,
    clock: FrameClock,
    focus: Option<FocusHandle>,
    /// Uninstall asks once more before deleting the folder.
    confirm_uninstall: bool,
    /// "Remove from Mandara" asks once more before removing a built-in.
    confirm_remove: bool,
    /// The Installed tab's "Removed Mandara plugins" section is expanded.
    pub(super) removed_open: bool,
    message: Option<(bool, String)>,
    /// Window scale factor of the last frame (icon resolution).
    pub(super) scale: f32,
}

impl Default for PluginModal {
    fn default() -> Self {
        Self {
            target: None,
            open: false,
            appear: SpringValue::new(0.0, APPEAR_SPRING),
            clock: FrameClock::new(),
            focus: None,
            confirm_uninstall: false,
            confirm_remove: false,
            removed_open: false,
            message: None,
            scale: 1.0,
        }
    }
}

/// Entrance: lively, a touch of overshoot. Exit: critically damped so the
/// card never flashes back past zero.
const APPEAR_SPRING: SpringConfig = SpringConfig::new(420.0, 30.0, 1.0);
const EXIT_STIFFNESS: f32 = 520.0;
/// Scrim opacity when fully open (light themes get a softer dim).
const SCRIM_DARK: f32 = 0.62;
const SCRIM_LIGHT: f32 = 0.42;
/// The scrim is a deep shade of the theme's primary hue (OKLCH lightness
/// at the top and bottom, and the share of its chroma kept), so dimming
/// tints instead of graying out.
const SCRIM_TOP_LIGHTNESS: f32 = 0.2;
const SCRIM_BOTTOM_LIGHTNESS: f32 = 0.1;
const SCRIM_CHROMA: f32 = 0.35;
/// The card rises this far and grows from this fraction of its width.
const RISE_PX: f32 = 22.0;
const START_WIDTH: f32 = 0.94;
/// Card widths and the margin kept to the window edges.
const DETAIL_WIDTH: f32 = 820.0;
const SETTINGS_WIDTH: f32 = 760.0;
const EDGE_MARGIN: f32 = 16.0;
const CARD_RADIUS: f32 = 30.0;
const CARD_PAD: f32 = 14.0;
/// The close button's inset from the card padding.
const CLOSE_INSET: f32 = 10.0;
/// Room kept beside the title for the close button.
const CLOSE_CLEARANCE: f32 = 8.0;
/// Card drop shadow: opacity (× the fade), offset, blur and inward spread.
const CARD_SHADOW_ALPHA: f32 = 0.38;
const CARD_SHADOW_Y: f32 = 24.0;
const CARD_SHADOW_BLUR: f32 = 60.0;
const CARD_SHADOW_INSET: f32 = 8.0;
/// Gradient angles: top-to-bottom and the diagonal of covers.
const GRADIENT_VERTICAL_DEG: f32 = 180.0;
const GRADIENT_DIAGONAL_DEG: f32 = 135.0;
/// Artwork tile corner radius, as a fraction of its side.
const TILE_RADIUS: f32 = 0.3;
/// The hero tile's drop shadow: offset, blur and inward spread.
const HERO_SHADOW_Y: f32 = 8.0;
const HERO_SHADOW_BLUR: f32 = 22.0;
const HERO_SHADOW_INSET: f32 = 4.0;
/// Hover lift of a filled button.
const BUTTON_HOVER_LIGHTEN: f32 = 0.04;
/// A declared number setting without a step slides in this many steps.
const DEFAULT_SLIDER_STEPS: f64 = 100.0;
/// Product page hero: its height (the blurred cover behind the title), the
/// icon tile and its ring, the title size, and how much of the art shows
/// through the wash at the top.
const PRODUCT_HERO_H: f32 = 236.0;
const PRODUCT_TILE: f32 = 96.0;
const HERO_RING: f32 = 5.0;
const PRODUCT_TITLE_PX: f32 = 30.0;
const HERO_WASH_TOP_ALPHA: f32 = 0.1;
/// Info strip cells, permission cards.
const INFO_GRID: CardGrid = CardGrid { min_w: 140.0, max_w: 1000.0, gap: SPACE_8_PX };
const PERMISSION_GRID: CardGrid = CardGrid { min_w: 300.0, max_w: 1000.0, gap: SPACE_8_PX };
/// Art band: the cover's share beside "more like this", the width below
/// which they stack, corner radius, and the related tiles.
const PREVIEW_MAIN_SHARE: f32 = 0.66;
const PREVIEW_STACK_BELOW_PX: f32 = 520.0;
const PREVIEW_RADIUS: f32 = 20.0;
const RELATED_MAX: usize = 2;
const RELATED_STACKED_ASPECT: f32 = 0.32;
/// Setting names previewed on the settings card.
const SETTINGS_PREVIEW_MAX: usize = 4;
const SETTINGS_TILE: f32 = 46.0;
const CLOSE_SIZE: f32 = 34.0;
const BODY_PAD: f32 = 26.0;
const PERMISSION_TILE: f32 = 34.0;

/// Shell settings each built-in plugin owns besides its own
/// `/plugins/settings/<id>/` table: exact paths, or prefixes ending in `/`.
const OWNED_SETTINGS: &[(&str, &[&str])] = &[
    ("weather", &["/general/units", "/weather/", "/location/shared"]),
    // With one shared location on, the plugins' own coordinates are unused:
    // the switch sits next to them.
    ("prayer-times", &["/location/shared"]),
    ("privacy", &["/privacy/"]),
    ("clock", &["/general/clock_24h", "/general/week_start", "/bar/show_seconds"]),
    ("wallpaper", &["/wallpaper/"]),
    ("launcher", &["/launcher/", "/appearance/launcher_icon", "/appearance/launcher_icons", "/edges/launcher_bottom"]),
    ("clipboard", &["/clipboard/"]),
    ("dock", &["/dock/"]),
    ("screen-time", &["/screen_time/"]),
    ("notifications", &["/sidebar/notifications_tab"]),
    ("session", &["/edges/session_right", "/session_menu/"]),
    ("workspaces", &["/appearance/bar_icons", "/appearance/icon_theme"]),
    ("language", &["/general/locale"]),
    ("audio", &["/general/osd", "/general/osd_position"]),
    ("keyboard-layout", &["/general/osd"]),
    ("quick-settings", &["/sidebar/enabled", "/sidebar/side", "/sidebar/hover_reveal", "/sidebar/mode", "/sidebar/width"]),
];

/// What a built-in plugin touches on the system.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Access {
    Network,
    Location,
    DBus,
    Compositor,
    Audio,
    Clipboard,
    Screen,
    Files,
    Processes,
    Commands,
    Sandboxed,
}

impl Access {
    fn icon(self) -> &'static str {
        match self {
            Access::Network => "globe",
            Access::Location => "crosshair",
            Access::DBus => "bolt",
            Access::Compositor => "monitor",
            Access::Audio => "volume-high",
            Access::Clipboard => "clipboard",
            Access::Screen => "camera",
            Access::Files => "note",
            Access::Processes => "cpu",
            Access::Commands => "keyboard",
            Access::Sandboxed => "check",
        }
    }

    /// English title (`tk`), shown through `tr`.
    fn title(self) -> &'static str {
        match self {
            Access::Network => tk("Network"),
            Access::Location => tk("Location"),
            Access::DBus => tk("System services"),
            Access::Compositor => tk("Compositor"),
            Access::Audio => tk("Audio"),
            Access::Clipboard => tk("Clipboard"),
            Access::Screen => tk("Screen capture"),
            Access::Files => tk("Files"),
            Access::Processes => tk("Processes"),
            Access::Commands => tk("Commands"),
            Access::Sandboxed => tk("Sandboxed"),
        }
    }

    fn tone(self, p: &MandaraPalette) -> Color {
        match self {
            Access::Sandboxed => p.success_emerald,
            Access::Commands | Access::Processes | Access::Screen => p.warning_amber,
            _ => p.primary,
        }
    }
}

/// Built-ins' system access, each with what it is used for (`tk`).
const BUILTIN_ACCESS: &[(&str, &[(Access, &str)])] = &[
    ("weather", &[(Access::Network, tk("Fetches forecasts from open-meteo.com")), (Access::Location, tk("Uses the coordinates you set — never GPS"))]),
    ("prayer-times", &[(Access::Location, tk("Computes times offline from the coordinates you set"))]),
    (
        "media",
        &[
            (Access::DBus, tk("Reads and controls players over MPRIS")),
            (Access::Network, tk("Fetches synced lyrics from lrclib.net while the panel is open")),
            (Access::Audio, tk("Reads the output level for the spectrum (PipeWire / PulseAudio)")),
        ],
    ),
    ("audio", &[(Access::Audio, tk("Changes volumes and devices (PipeWire / PulseAudio)"))]),
    ("network", &[(Access::DBus, tk("Lists and joins networks through NetworkManager"))]),
    ("bluetooth", &[(Access::DBus, tk("Pairs and connects devices through BlueZ"))]),
    ("notifications", &[(Access::DBus, tk("Receives notifications as the desktop notification server"))]),
    ("tray", &[(Access::DBus, tk("Hosts status icons and their menus (StatusNotifier)"))]),
    ("quick-settings", &[(Access::DBus, tk("Toggles Wi-Fi, Bluetooth, power profiles and do-not-disturb"))]),
    ("session", &[(Access::DBus, tk("Locks, suspends, restarts and powers off through logind"))]),
    ("clipboard", &[(Access::Clipboard, tk("Keeps a history of what you copy")), (Access::Files, tk("Saves the history in Mandara's data folder"))]),
    ("dock", &[(Access::Compositor, tk("Lists, focuses and closes windows (foreign-toplevel)")), (Access::Processes, tk("Starts the apps you click"))]),
    ("screen-time", &[(Access::Compositor, tk("Follows which window has focus")), (Access::Files, tk("Keeps daily totals in Mandara's state folder"))]),
    (
        "screenshot",
        &[
            (Access::Screen, tk("Captures screens, windows and regions")),
            (Access::Clipboard, tk("Copies captures")),
            (Access::Files, tk("Saves captures to ~/Pictures/Screenshots")),
        ],
    ),
    ("wallpaper", &[(Access::Files, tk("Reads images from your wallpaper folder"))]),
    ("wallhaven", &[(Access::Network, tk("Searches wallhaven.cc and downloads the wallpapers you pick")), (Access::Files, tk("Saves the wallpapers you pick in Mandara's data folder"))]),
    ("cursors", &[(Access::Network, tk("Downloads the cursor themes you pick from their GitHub releases")), (Access::Files, tk("Installs them to ~/.local/share/icons")), (Access::Compositor, tk("Sets the cursor in Mandara's compositor drop-in and live"))]),
    ("workspaces", &[(Access::Compositor, tk("Reads and switches tags or workspaces"))]),
    ("active-window", &[(Access::Compositor, tk("Reads the focused window's title"))]),
    ("keyboard-layout", &[(Access::Compositor, tk("Reads and switches keyboard layouts"))]),
    ("launcher", &[(Access::Files, tk("Reads installed applications")), (Access::Processes, tk("Starts the apps and commands you pick"))]),
    ("resources", &[(Access::Processes, tk("Reads CPU, memory and disk usage"))]),
    ("processes", &[(Access::Processes, tk("Lists running processes and ends the ones you choose"))]),
    ("todo", &[(Access::Files, tk("Saves your tasks in Mandara's data folder"))]),
    ("notes", &[(Access::Files, tk("Saves your notes in Mandara's data folder"))]),
];

/// Third-party permission → what it allows (`tk`).
fn permission_text(name: &str) -> (Access, &'static str) {
    match name {
        "commands" => (Access::Commands, tk("Can run Mandara commands: open panels, change settings, control media")),
        _ => (Access::Commands, tk("A permission this version of Mandara doesn't know; it has no effect")),
    }
}

thread_local! {
    /// Third-party `[[settings]]` turned into registry settings, built once
    /// per manifest and language (the registry needs `'static` text).
    static DECLARED: RefCell<HashMap<String, Vec<Setting>>> = RefCell::new(HashMap::new());
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Registry settings for a third-party plugin's declarations.
fn declared_settings(m: &PluginManifest) -> Vec<Setting> {
    let PluginOrigin::ThirdParty { path } = &m.origin else { return Vec::new() };
    if m.settings.is_empty() {
        return Vec::new();
    }
    let lang = mandara_i18n::language();
    let key = format!("{}\u{1f}{}\u{1f}{}", m.id, lang.code(), serde_json::to_string(&m.settings).unwrap_or_default());
    if let Some(hit) = DECLARED.with(|d| d.borrow().get(&key).cloned()) {
        return hit;
    }
    let mut catalogs = mandara_plugins::wasm::PluginCatalogs::new(path);
    let mut tr_plugin = |s: &str| leak(catalogs.translate(s));
    let group = leak(m.display_name());
    let list: Vec<Setting> = m
        .settings
        .iter()
        .map(|s: &ManifestSetting| {
            let control = match s.kind {
                SettingKind::Bool => Control::Toggle,
                SettingKind::Number => match (s.min, s.max) {
                    (Some(min), Some(max)) => Control::Slider { min, max, step: s.step.unwrap_or(((max - min) / DEFAULT_SLIDER_STEPS).max(f64::EPSILON)), unit: leak(s.unit.clone()) },
                    (min, max) => Control::Number { min: min.unwrap_or(f64::MIN), max: max.unwrap_or(f64::MAX) },
                },
                SettingKind::Text => Control::Text { placeholder: leak(s.default.as_ref().and_then(|v| v.as_str()).unwrap_or_default().to_string()) },
                SettingKind::Choice => {
                    let choices: Vec<(&'static str, &'static str)> = s.choices.iter().map(|c| (leak(c.value().to_string()), tr_plugin(c.label()))).collect();
                    Control::Choice(Box::leak(choices.into_boxed_slice()))
                }
            };
            Setting {
                section: Section::Plugins,
                group,
                path: leak(format!("/plugins/settings/{}/{}", m.id, s.key)),
                label: if s.label.is_empty() { leak(s.key.clone()) } else { tr_plugin(&s.label) },
                help: if s.help.is_empty() { "" } else { tr_plugin(&s.help) },
                control,
                default: Some(s.default_json()),
            }
        })
        .collect();
    DECLARED.with(|d| d.borrow_mut().insert(key, list.clone()));
    list
}

/// Whether the registry setting at `path` belongs to plugin `id`.
fn owns(id: &str, path: &str) -> bool {
    // A setting of an opt-in plugin is that plugin's alone (Wallhaven's
    // options sit under `/wallpaper/` but aren't the wallpaper plugin's).
    if let Some(owner) = crate::registry::required_plugin(path) {
        return owner == id;
    }
    in_own_table(id, path)
        || OWNED_SETTINGS
            .iter()
            .filter(|(owner, _)| *owner == id)
            .flat_map(|(_, paths)| paths.iter())
            .any(|p| if p.ends_with('/') { path.starts_with(p) } else { path == *p })
}

/// Whether `path` is in plugin `id`'s own `[plugins.settings.<id>]` table.
fn in_own_table(id: &str, path: &str) -> bool {
    path.strip_prefix("/plugins/settings/").and_then(|rest| rest.split_once('/')).is_some_and(|(owner, _)| owner == id)
}

/// Every setting that belongs to `m`: its own table first, then the shell
/// settings it drives (each in registry order), then its declared ones.
pub(super) fn plugin_settings(m: &PluginManifest) -> Vec<Setting> {
    let mut list: Vec<Setting> = match m.origin {
        PluginOrigin::Builtin => {
            let mut owned: Vec<Setting> = registry::settings().iter().filter(|s| owns(&m.id, s.path)).cloned().collect();
            owned.sort_by_key(|s| !in_own_table(&m.id, s.path));
            owned
        }
        PluginOrigin::ThirdParty { .. } => Vec::new(),
    };
    list.extend(declared_settings(m));
    list
}

/// Whether `m` has any settings (without building them: the plugin grid
/// asks this for every card on every frame).
pub(super) fn has_settings(m: &PluginManifest) -> bool {
    match m.origin {
        PluginOrigin::Builtin => registry::settings().iter().any(|s| owns(&m.id, s.path)),
        PluginOrigin::ThirdParty { .. } => !m.settings.is_empty(),
    }
}

/// Where a plugin came from, with its badge color.
fn origin(m: &PluginManifest, p: &MandaraPalette) -> (&'static str, Color) {
    match &m.origin {
        PluginOrigin::Builtin => (tk("Built-in"), p.on_surface_variant),
        PluginOrigin::ThirdParty { path } => {
            let s = path.to_string_lossy();
            if s.contains("plugins-dev") {
                (tk("Development"), p.tertiary)
            } else if !s.starts_with(&*mandara_config::paths::data_dir().to_string_lossy()) {
                (tk("System"), p.secondary)
            } else if path.join(".source").is_file() {
                (tk("Marketplace"), p.primary)
            } else {
                (tk("Installed"), p.primary)
            }
        }
    }
}

pub(super) fn category(m: &PluginManifest) -> &'static str {
    match m.origin {
        PluginOrigin::ThirdParty { .. } => tk("Third-party"),
        PluginOrigin::Builtin => PLUGIN_CATEGORIES.iter().find(|(_, ids)| ids.contains(&m.id.as_str())).map(|(t, _)| *t).unwrap_or(tk("More")),
    }
}

/// Where `id` currently sits in the bar, if anywhere.
fn bar_lane(cfg: &ShellConfig, id: &str) -> Option<Lane> {
    let has = |v: &[String]| v.iter().any(|x| x == id);
    if has(&cfg.bar.left) {
        Some(Lane::Left)
    } else if has(&cfg.bar.center) {
        Some(Lane::Center)
    } else if has(&cfg.bar.right) {
        Some(Lane::Right)
    } else {
        None
    }
}

fn lane_label(lane: Lane) -> &'static str {
    match lane {
        Lane::Left => tk("Left"),
        Lane::Center => tk("Center"),
        Lane::Right => tk("Right"),
        Lane::Sidebar | Lane::Tab(_) => tk("Sidebar"),
        Lane::Tray => tk("Not placed"),
    }
}

fn zone_lane(zone: Option<Zone>) -> Lane {
    match zone {
        Some(Zone::Left) => Lane::Left,
        Some(Zone::Center) => Lane::Center,
        _ => Lane::Right,
    }
}

/// A rounded action button.
fn action(id: impl Into<SharedString>, icon_name: &'static str, label: String, primary: bool, p: &MandaraPalette) -> gpui::Stateful<gpui::Div> {
    let (bg, fg) = if primary { (p.primary, p.on_primary) } else { (p.surface_container_high, p.on_surface) };
    div()
        .id(id.into())
        .h(px(DIM_36_PX))
        .px(px(SPACE_16_PX))
        .rounded_full()
        .row()
        .flex_none()
        .items_center()
        .gap(px(SPACE_8_PX))
        .cursor_pointer()
        .bg(bg.hsla())
        .text_color(fg.hsla())
        .text_size(px(TEXT_12_5_PX))
        .font_weight(FontWeight::MEDIUM)
        .hover(move |s| s.bg(if primary { bg.lighten(BUTTON_HOVER_LIGHTEN).hsla() } else { p.primary.with_alpha(ALPHA_14).hsla() }))
        .child(icon(icon_name, ICON_14_PX, if primary { p.on_primary } else { p.on_surface_variant }))
        .child(label)
}

fn chip(text: String, color: Color) -> gpui::Div {
    div()
        .px(px(SPACE_10_PX))
        .h(px(DIM_24_PX))
        .rounded_full()
        .row()
        .flex_none()
        .items_center()
        .bg(color.with_alpha(ALPHA_13).hsla())
        .text_color(color.hsla())
        .text_size(px(TEXT_11_5_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}

/// A product-page section: its title, then the caller's content.
fn section(title: &'static str, p: &MandaraPalette) -> gpui::Div {
    div().row().flex_col().gap(px(SPACE_10_PX)).child(div().text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface.hsla()).child(title))
}

/// A destructive action (remove, uninstall), filled once it asks to confirm.
fn danger(id: &'static str, label: &'static str, confirming: bool, p: &MandaraPalette) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(DIM_36_PX))
        .px(px(SPACE_16_PX))
        .rounded_full()
        .row()
        .flex_none()
        .items_center()
        .gap(px(SPACE_8_PX))
        .cursor_pointer()
        .bg(p.urgent_coral.with_alpha(if confirming { ALPHA_90 } else { ALPHA_12 }).hsla())
        .text_color(if confirming { p.on_primary.hsla() } else { p.urgent_coral.hsla() })
        .text_size(px(TEXT_12_5_PX))
        .font_weight(FontWeight::MEDIUM)
        .hover(|s| s.opacity(ALPHA_88))
        .child(icon("trash", ICON_14_PX, if confirming { p.on_primary } else { p.urgent_coral }))
        .child(label)
}

/// Stands in for the on / off switch of an essential plugin.
pub(super) fn always_on_chip(p: &MandaraPalette) -> gpui::Div {
    div()
        .flex_none()
        .h(px(DIM_28_PX))
        .px(px(SPACE_10_PX))
        .rounded_full()
        .row()
        .items_center()
        .gap(px(SPACE_6_PX))
        .bg(p.primary.with_alpha(ALPHA_12).hsla())
        .text_color(p.primary.hsla())
        .text_size(px(TEXT_12_PX))
        .font_weight(FontWeight::MEDIUM)
        .child(icon("lock", ICON_13_PX, p.primary))
        .child(t("Always on"))
}

impl StudioView {
    /// Opens `id`'s detail or settings modal (Plugins page underneath).
    pub fn open_plugin(&mut self, id: &str, view: ModalView, window: &mut Window, cx: &mut Context<Self>) {
        let Some(manifest) = cx.global::<PluginRegistry>().manifests().find(|m| m.id == id).cloned() else {
            tracing::warn!("Studio: no plugin `{id}`");
            return;
        };
        // The wallpaper generator's settings are its studio on the Wallpaper page.
        if view == ModalView::Settings && id == mandara_ui::plugins::wallpaper_generator::ID {
            self.show(Section::Wallpaper, cx);
            return;
        }
        // A settings deep link to a plugin without settings shows its details.
        let view = if view == ModalView::Settings && plugin_settings(&manifest).is_empty() { ModalView::Detail } else { view };
        // From elsewhere (a deep link): the store front underneath.
        if self.section != Section::Plugins {
            self.plugins_tab = super::plugins_local::PluginsTab::default();
        }
        self.section = Section::Plugins;
        let reopening = self.modal.open && self.modal.target.as_ref().is_some_and(|(open_id, _)| open_id == id);
        self.modal.target = Some((id.to_string(), view));
        if !reopening {
            self.modal.confirm_uninstall = false;
            self.modal.confirm_remove = false;
            self.modal.message = None;
        }
        self.modal.open = true;
        self.modal.appear.set_config(APPEAR_SPRING);
        self.error = None;
        let focus = self.modal.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        focus.focus(window, cx);
        cx.notify();
    }

    /// Whether a plugin modal is up (or animating out).
    pub(super) fn modal_open(&self) -> bool {
        self.modal.target.is_some()
    }

    /// Turns a plugin on / off, removes or restores it (the shell follows
    /// live). A refusal (an essential plugin) shows in the page banner and
    /// in the open modal.
    pub(super) fn plugin_lifecycle(&mut self, id: &str, action: Action, cx: &mut Context<Self>) {
        match mandara_ui::plugins::lifecycle::apply(id, action, cx) {
            Ok(()) => {
                self.error = None;
                self.modal.message = None;
            }
            Err(e) => {
                self.modal.message = Some((false, e.clone()));
                self.error = Some(e);
            }
        }
        self.modal.confirm_remove = false;
        cx.notify();
    }

    /// Starts the exit animation.
    pub fn close_plugin(&mut self, cx: &mut Context<Self>) {
        self.modal.open = false;
        self.modal.confirm_uninstall = false;
        self.modal.confirm_remove = false;
        self.modal.appear.set_config(SpringConfig::critical(EXIT_STIFFNESS));
        cx.notify();
    }

    /// The modal layer (scrim + card), while one is open or animating out.
    pub(super) fn plugin_modal_layer(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.modal.scale = window.scale_factor();
        let (id, view) = self.modal.target.clone()?;
        let reduce = cfg.motion.reduce_motion;
        self.modal.appear.set_instant(reduce);
        self.modal.appear.set_target(if self.modal.open { 1.0 } else { 0.0 });
        let dt = self.modal.clock.tick(std::time::Instant::now());
        if self.modal.appear.step(dt) {
            window.request_animation_frame();
        } else {
            self.modal.clock.idle();
        }
        let a = self.modal.appear.value();
        if !self.modal.open && a <= f32::EPSILON {
            self.modal.target = None;
            return None;
        }
        let Some(m) = cx.global::<PluginRegistry>().manifests().find(|m| m.id == id).cloned() else {
            self.modal.target = None;
            return None;
        };
        let viewport = window.viewport_size();
        let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));
        let full_w = (if view == ModalView::Detail { DETAIL_WIDTH } else { SETTINGS_WIDTH }).min(vw - EDGE_MARGIN * 2.0);
        let max_h = vh - EDGE_MARGIN * 2.0;
        let fade = a.clamp(0.0, 1.0);
        let grow = START_WIDTH + (1.0 - START_WIDTH) * a;
        let body = match view {
            ModalView::Detail => self.plugin_detail_body(&m, full_w, cfg, p, cx),
            ModalView::Settings => self.plugin_settings_body(&m, cfg, p, cx),
        };
        let rtl = mandara_i18n::language().is_rtl();
        let close = div()
            .id("plugin-modal-close")
            .absolute()
            .top(px(CARD_PAD + CLOSE_INSET))
            .when(rtl, |el| el.left(px(CARD_PAD + CLOSE_INSET)))
            .when(!rtl, |el| el.right(px(CARD_PAD + CLOSE_INSET)))
            .size(px(CLOSE_SIZE))
            .rounded_full()
            .row()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(p.surface_base.with_alpha(ALPHA_72).hsla())
            .shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
            .hover(|s| s.bg(p.surface_container_high.hsla()))
            .child(icon("close", ICON_15_PX, p.on_surface))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close_plugin(cx)));
        let scrim_alpha = if p.is_dark { SCRIM_DARK } else { SCRIM_LIGHT } * fade;
        let card = div()
            .id("plugin-modal-card")
            .relative()
            .top(px(RISE_PX * (1.0 - a)))
            .w(px(full_w * grow))
            .max_h(px(max_h))
            .row()
            .flex_col()
            .overflow_hidden()
            .rounded(px(CARD_RADIUS))
            .bg(p.surface_container_low.hsla())
            .opacity(fade)
            .shadow({
                let mut s = mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow);
                s.push(gpui::BoxShadow { color: gpui::black().opacity(CARD_SHADOW_ALPHA * fade), offset: gpui::point(px(0.0), px(CARD_SHADOW_Y)), blur_radius: px(CARD_SHADOW_BLUR), spread_radius: px(-CARD_SHADOW_INSET), inset: false });
                s
            })
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child({
                let (scroll, bar) = mandara_ui::scrollbar::keyed("plugin-modal-scroll");
                let body = div()
                    .id("plugin-modal-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    // Content keeps its final width while the card grows (no reflow).
                    .child(div().w(px(full_w)).mx(px(-(full_w * (1.0 - grow)) / 2.0)).child(body));
                mandara_ui::scrollbar::with_scrollbar(body, &scroll, &bar, p).flex_1().min_h(px(0.0)).flex().flex_col()
            })
            .child(close);
        let focus = self.modal.focus.clone();
        Some(
            div()
                .id("plugin-modal")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .row()
                .items_center()
                .justify_center()
                .when_some(focus, |el, f| el.track_focus(&f))
                // A heavy, tinted dim stands in for backdrop blur.
                .bg({
                    let shade = |l: f32| p.primary.with_lightness(l).scale_chroma(SCRIM_CHROMA).with_alpha(scrim_alpha);
                    mandara_core::oklab_gradient(GRADIENT_VERTICAL_DEG, shade(SCRIM_TOP_LIGHTNESS), shade(SCRIM_BOTTOM_LIGHTNESS))
                })
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close_plugin(cx)))
                .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, _, cx| match e.keystroke.key.as_str() {
                    "escape" => {
                        this.close_plugin(cx);
                        cx.stop_propagation();
                    }
                    // Focus stays in the modal.
                    "tab" => cx.stop_propagation(),
                    _ => {}
                }))
                .child(card)
                .into_any_element(),
        )
    }

    /// The product page: a hero over the plugin's blurred cover with the
    /// icon, name, maker and Get / On; an info strip; the cover art band
    /// with "more like this"; the description; what's new; privacy and
    /// system access; settings; where it fits; and managing it.
    fn plugin_detail_body(&mut self, m: &PluginManifest, width: f32, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        store::covers_begin(cx.entity().downgrade());
        let enabled = cx.global::<PluginRegistry>().is_enabled(&m.id, &cfg.plugins);
        let rtl = mandara_i18n::language().is_rtl();
        let fonts = mandara_ui::fonts::fonts(cx);
        let scale = self.modal.scale;
        let inner_w = width - BODY_PAD * 2.0;
        let art = store::art_of(m);
        let builtin = m.origin == PluginOrigin::Builtin;
        let essential = if builtin { mandara_plugins::lifecycle::essential_reason(&m.id) } else { None };
        let removed = builtin && cfg.plugins.is_removed(&m.id);
        let (origin_label, origin_color) = origin(m, p);
        let author = if m.author.is_empty() { t("Unknown author").to_string() } else { m.author.clone() };
        let byline = if m.version.is_empty() { tf("by {author}", &[("author", &author)]) } else { tf("by {author} · v{version}", &[("author", &author), ("version", &m.version)]) };
        let has_settings = has_settings(m);

        // ── Hero ────────────────────────────────────────────────────────
        let wash = || div().size_full().bg(mandara_core::oklab_gradient(GRADIENT_DIAGONAL_DEG, p.primary.with_alpha(ALPHA_45), p.tertiary.with_alpha(ALPHA_30))).into_any_element();
        let backdrop = store::art_box(art.clone(), width, PRODUCT_HERO_H, scale, store::Detail::Ambient, wash());
        let get = self.get_button("modal", m, enabled, store::ButtonSize::Large, cfg, p, cx);
        let settings_btn = has_settings.then(|| {
            let id = m.id.clone();
            store::pill("plugin-modal-settings", Some("sliders"), t("Settings"), store::Tone::Glass, store::ButtonSize::Large, p).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.open_plugin(&id, ModalView::Settings, window, cx)))
        });
        let tile = div()
            .flex_none()
            .p(px(HERO_RING))
            .rounded(px((PRODUCT_TILE + HERO_RING * 2.0) * TILE_RADIUS))
            .bg(p.surface_container_low.hsla())
            .shadow(vec![gpui::BoxShadow { color: gpui::black().opacity(ALPHA_28), offset: gpui::point(px(0.0), px(HERO_SHADOW_Y)), blur_radius: px(HERO_SHADOW_BLUR), spread_radius: px(-HERO_SHADOW_INSET), inset: false }])
            .child(plugin_tile(m, PRODUCT_TILE, true, scale, p));
        let chips = div()
            .row()
            .flex_wrap()
            .gap(px(SPACE_6_PX))
            .child(chip(tr(origin_label), origin_color))
            .child(chip(tr(category(m)), p.secondary))
            .when(removed, |el| el.child(chip(t("Removed").to_string(), p.urgent_coral)))
            .when(!enabled && !removed, |el| el.child(chip(t("Off").to_string(), p.on_surface_variant)));
        let hero = div()
            .relative()
            .w(px(width))
            .h(px(PRODUCT_HERO_H))
            .overflow_hidden()
            .child(backdrop)
            // The art melts into the page below it.
            .child(div().absolute().top_0().left_0().size_full().bg(mandara_core::oklab_gradient(GRADIENT_VERTICAL_DEG, p.surface_container_low.with_alpha(HERO_WASH_TOP_ALPHA), p.surface_container_low)))
            .child(mandara_core::grain_overlay())
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(SPACE_4_PX))
                    .px(px(BODY_PAD))
                    .row()
                    .flex_wrap()
                    .items_end()
                    .gap(px(SPACE_18_PX))
                    .child(tile)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(DIM_200_PX))
                            .row()
                            .flex_col()
                            .gap(px(SPACE_6_PX))
                            .child(chips)
                            .child(div().font_family(fonts.display.clone()).text_size(px(PRODUCT_TITLE_PX)).font_weight(FontWeight::SEMIBOLD).truncate().child(m.display_name()))
                            .child(div().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(byline)),
                    )
                    .child(div().row().items_center().gap(px(SPACE_8_PX)).pb(px(SPACE_6_PX)).children(settings_btn).child(get)),
            );

        // ── Info strip ──────────────────────────────────────────────────
        let bar_capable = !NO_BAR.contains(&m.id.as_str());
        let sidebar_capable = SIDEBAR_CAPABLE.contains(&m.id.as_str()) || matches!(m.origin, PluginOrigin::ThirdParty { .. });
        let access: Vec<(Access, &'static str)> = match &m.origin {
            PluginOrigin::Builtin => BUILTIN_ACCESS.iter().find(|(id, _)| *id == m.id).map(|(_, a)| a.to_vec()).unwrap_or_default(),
            PluginOrigin::ThirdParty { .. } => {
                let mut list: Vec<(Access, &'static str)> = m.permissions.iter().map(|n| permission_text(n)).collect();
                list.push((Access::Sandboxed, tk("Runs sandboxed: no files, network or devices, with a time limit on every call")));
                list
            }
        };
        let areas = access.iter().filter(|(a, _)| *a != Access::Sandboxed).count();
        let fits_value = match (bar_capable, sidebar_capable) {
            (true, true) => t("Bar and sidebar"),
            (true, false) => t("Bar"),
            (false, true) => t("Sidebar"),
            (false, false) => t("Background"),
        };
        let info = |label: &'static str, value: String, glyph: &'static str| {
            div()
                .flex_1()
                .min_w(px(0.0))
                .px(px(SPACE_14_PX))
                .py(px(SPACE_12_PX))
                .rounded(px(RADIUS_16_PX))
                .bg(p.surface_container.hsla())
                .row()
                .flex_col()
                .gap(px(SPACE_4_PX))
                .child(div().text_size(px(TEXT_10_5_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(label))
                .child(div().row().items_center().gap(px(SPACE_6_PX)).child(icon(glyph, ICON_14_PX, p.primary)).child(div().min_w(px(0.0)).text_size(px(TEXT_13_5_PX)).font_weight(FontWeight::SEMIBOLD).font_features(mandara_ui::fonts::tabular_nums()).truncate().child(value)))
                .into_any_element()
        };
        let cells = vec![
            info(t("Category"), tr(category(m)), plugin_icon(&m.id)),
            info(t("Version"), if m.version.is_empty() { t("Built in").to_string() } else { format!("v{}", m.version) }, "package"),
            info(t("Access"), if areas == 0 { t("Sandboxed").to_string() } else { tf("{count} areas", &[("count", &areas)]) }, if areas == 0 { "shield" } else { "key" }),
            info(t("Fits in"), fits_value.to_string(), "sidebar"),
        ];
        let info_strip = grid::fit(inner_w, INFO_GRID).balanced(cells.len()).render(cells);

        // ── Art band: the cover, and more like this ─────────────────────
        let related: Vec<PluginManifest> = if builtin {
            let mine = category(m);
            cx.global::<PluginRegistry>().manifests().filter(|o| o.id != m.id && o.origin == PluginOrigin::Builtin && category(o) == mine && store::art_of(o).is_some()).take(RELATED_MAX).cloned().collect()
        } else {
            Vec::new()
        };
        let side_by_side = !related.is_empty() && inner_w >= PREVIEW_STACK_BELOW_PX;
        let main_w = if side_by_side { ((inner_w - SPACE_10_PX) * PREVIEW_MAIN_SHARE).floor() } else { inner_w };
        let main_h = (main_w * STORE_COVER_ASPECT).floor();
        let preview = div()
            .relative()
            .flex_none()
            .rounded(px(PREVIEW_RADIUS))
            .overflow_hidden()
            .child(store::art_box(art, main_w, main_h, scale, store::Detail::Sharp, store::plugin_placeholder(m, main_w, main_h, p)))
            .child(store::rim_overlay(PREVIEW_RADIUS, p));
        let mut band = div().row().gap(px(SPACE_10_PX)).when(!side_by_side, |el| el.flex_col()).child(preview);
        if !related.is_empty() {
            let side_w = if side_by_side { inner_w - SPACE_10_PX - main_w } else { inner_w };
            let tile_h = if side_by_side { ((main_h - SPACE_10_PX * (related.len() - 1) as f32) / related.len() as f32).floor() } else { (side_w * RELATED_STACKED_ASPECT).floor() };
            let mut side = div().row().flex_col().gap(px(SPACE_10_PX));
            for o in &related {
                let id = o.id.clone();
                side = side.child(
                    div()
                        .id(SharedString::from(format!("plugin-modal-related-{}", o.id)))
                        .relative()
                        .rounded(px(PREVIEW_RADIUS))
                        .overflow_hidden()
                        .cursor_pointer()
                        .child(store::art_box(store::art_of(o), side_w, tile_h, scale, store::Detail::Sharp, store::plugin_placeholder(o, side_w, tile_h, p)))
                        .child(div().absolute().top_0().left_0().size_full().bg(mandara_core::oklab_gradient(GRADIENT_VERTICAL_DEG, p.surface_dim.with_alpha(0.0), p.surface_dim.with_alpha(ALPHA_85))))
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .bottom_0()
                                .p(px(SPACE_12_PX))
                                .row()
                                .flex_col()
                                .child(div().text_size(px(TEXT_10_5_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(t("More like this")))
                                .child(div().text_size(px(TEXT_13_5_PX)).font_weight(FontWeight::SEMIBOLD).truncate().child(o.display_name())),
                        )
                        .child(store::rim_overlay(PREVIEW_RADIUS, p))
                        .hover(|s| s.opacity(ALPHA_90))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.open_plugin(&id, ModalView::Detail, window, cx))),
                );
            }
            band = band.child(side);
        }

        // ── Privacy & system access ─────────────────────────────────────
        let access_rows = if access.is_empty() { vec![(Access::Sandboxed, tk("Needs no system access beyond drawing its own view"))] } else { access };
        let permission_cards: Vec<AnyElement> = access_rows
            .into_iter()
            .map(|(kind, what)| {
                let tone = kind.tone(p);
                div()
                    .flex_1()
                    .h_full()
                    .px(px(SPACE_14_PX))
                    .py(px(SPACE_12_PX))
                    .rounded(px(RADIUS_16_PX))
                    .bg(p.surface_container.hsla())
                    .row()
                    .items_start()
                    .gap(px(SPACE_12_PX))
                    .child(div().size(px(PERMISSION_TILE)).flex_none().rounded(px(RADIUS_11_PX)).row().items_center().justify_center().bg(tone.with_alpha(ALPHA_14).hsla()).child(icon(kind.icon(), ICON_16_PX, tone)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .row()
                            .flex_col()
                            .gap(px(SPACE_3_PX))
                            .when(rtl, |el| el.items_end())
                            .child(div().text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).child(mandara_ui::bidi::paragraph(tr(kind.title()))))
                            .child(div().text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tr(what)))),
                    )
                    .into_any_element()
            })
            .collect();
        let permissions = grid::fit(inner_w, PERMISSION_GRID).render(permission_cards);

        // ── Settings ────────────────────────────────────────────────────
        let settings_card = {
            let list = if has_settings { plugin_settings(m) } else { Vec::new() };
            let id = m.id.clone();
            let mut card = div()
                .px(px(SPACE_16_PX))
                .py(px(SPACE_14_PX))
                .rounded(px(RADIUS_18_PX))
                .bg(p.surface_container.hsla())
                .row()
                .flex_wrap()
                .items_center()
                .gap(px(SPACE_12_PX))
                .child(div().size(px(PERMISSION_TILE)).flex_none().rounded(px(RADIUS_11_PX)).row().items_center().justify_center().bg(p.primary.with_alpha(ALPHA_14).hsla()).child(icon("sliders", ICON_16_PX, p.primary)));
            if list.is_empty() {
                card = card.child(div().flex_1().min_w(px(DIM_200_PX)).row().flex_col().gap(px(SPACE_3_PX)).child(div().text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).child(t("Nothing to set up"))).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("This plugin works without settings."))));
            } else {
                let names = div().row().flex_wrap().gap(px(SPACE_6_PX)).children(list.iter().take(SETTINGS_PREVIEW_MAX).map(|s| chip(tr(s.label), p.on_surface_variant)));
                card = card
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(DIM_200_PX))
                            .row()
                            .flex_col()
                            .gap(px(SPACE_6_PX))
                            .child(div().text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).font_features(mandara_ui::fonts::tabular_nums()).child(tf("{n} settings · changes apply live", &[("n", &list.len())])))
                            .child(names),
                    )
                    .child(action("plugin-modal-open-settings", "sliders", t("Open settings").into(), true, p).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.open_plugin(&id, ModalView::Settings, window, cx))));
            }
            card
        };

        // ── Where it fits ───────────────────────────────────────────────
        let lane = bar_lane(cfg, &m.id);
        let in_sidebar = cfg.sidebar.has_card(&m.id);
        let mut fits = div().row().flex_wrap().items_center().gap(px(SPACE_6_PX));
        if bar_capable {
            let default = zone_lane(m.zone);
            for l in [Lane::Left, Lane::Center, Lane::Right] {
                let here = lane == Some(l);
                let color = if here { p.primary } else if l == default { p.secondary } else { p.on_surface_variant };
                let label = if l == default && !here { tf("{zone} · default", &[("zone", &tr(lane_label(l)))]) } else { tr(lane_label(l)) };
                fits = fits.child(chip(label, color));
            }
        }
        if sidebar_capable {
            fits = fits.child(chip(tr(lane_label(Lane::Sidebar)), if in_sidebar { p.primary } else { p.on_surface_variant }));
        }
        if !bar_capable && !sidebar_capable {
            fits = fits.child(chip(t("Works in the background").to_string(), p.on_surface_variant));
        }
        let mut placement = div().row().flex_wrap().items_center().gap(px(SPACE_8_PX));
        if bar_capable {
            let id = m.id.clone();
            placement = match lane {
                None => {
                    let to = zone_lane(m.zone);
                    placement.child(action("plugin-modal-add-bar", "plus", t("Add to bar").into(), true, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| place(&id, Lane::Tray, to, None, cx))))
                }
                Some(from) => placement.child(
                    action("plugin-modal-remove-bar", "minus", tf("Remove from bar ({zone})", &[("zone", &tr(lane_label(from)))]), false, p)
                        .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| place(&id, from, Lane::Tray, None, cx))),
                ),
            };
        }
        if sidebar_capable && cfg.sidebar.enabled {
            let id = m.id.clone();
            placement = if in_sidebar {
                placement.child(action("plugin-modal-remove-sidebar", "sidebar", t("Remove from sidebar").into(), false, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| place(&id, Lane::Sidebar, Lane::Tray, None, cx))))
            } else {
                placement.child(action("plugin-modal-add-sidebar", "sidebar", t("Add to sidebar").into(), false, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| place(&id, Lane::Tray, Lane::Sidebar, None, cx))))
            };
        }

        // ── Manage ──────────────────────────────────────────────────────
        let mut manage = div().row().flex_wrap().items_center().gap(px(SPACE_8_PX));
        if removed {
            let id = m.id.clone();
            manage = manage.child(action("plugin-modal-restore", "undo", t("Restore").into(), true, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.plugin_lifecycle(&id, Action::Restore, cx))));
        } else if builtin && essential.is_none() {
            let (id, confirming) = (m.id.clone(), self.modal.confirm_remove);
            manage = manage.child(danger("plugin-modal-remove", if confirming { t("Confirm remove") } else { t("Remove from Mandara") }, confirming, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if !this.modal.confirm_remove {
                    this.modal.confirm_remove = true;
                    this.modal.message = Some((true, t("Removing turns it off and takes it out of the bar and sidebar. It waits under Removed Mandara plugins until you restore it.").to_string()));
                    cx.notify();
                    return;
                }
                this.plugin_lifecycle(&id, Action::Remove, cx);
            })));
        }
        if let PluginOrigin::ThirdParty { path } = &m.origin {
            if !m.homepage.is_empty() {
                let url = m.homepage.clone();
                manage = manage.child(action("plugin-modal-homepage", "globe", t("Homepage").into(), false, p).on_click(move |_, _, _| {
                    let _ = mandara_services::session::open_uri(&url);
                }));
            }
            let dir = path.clone();
            manage = manage.child(action("plugin-modal-folder", "folder", t("Show folder").into(), false, p).on_click(move |_, _, _| {
                let _ = mandara_services::session::open_uri(&format!("file://{}", dir.display()));
            }));
            // Only Mandara-managed installs are removed here: development
            // folders hold your source, system ones belong to the package manager.
            if matches!(origin_label, "Installed" | "Marketplace") {
                let (id, dir, confirming) = (m.id.clone(), path.clone(), self.modal.confirm_uninstall);
                manage = manage.child(danger("plugin-modal-uninstall", if confirming { t("Confirm uninstall") } else { t("Uninstall") }, confirming, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if !this.modal.confirm_uninstall {
                        this.modal.confirm_uninstall = true;
                        cx.notify();
                        return;
                    }
                    this.uninstall_plugin(&id, &dir, cx);
                })));
            }
        }
        let message = self.modal.message.clone().map(|(ok, text)| {
            div()
                .px(px(SPACE_14_PX))
                .py(px(SPACE_10_PX))
                .rounded(px(RADIUS_12_PX))
                .text_size(px(TEXT_12_5_PX))
                .bg(if ok { p.success_emerald.with_alpha(ALPHA_12).hsla() } else { p.urgent_coral.with_alpha(ALPHA_12).hsla() })
                .text_color(if ok { p.success_emerald.hsla() } else { p.urgent_coral.hsla() })
                .child(text)
        });
        let whats_new = self.plugin_update_action(&m.id, p, cx).map(|update| {
            section(t("What's new"), p).child(
                div()
                    .px(px(SPACE_16_PX))
                    .py(px(SPACE_14_PX))
                    .rounded(px(RADIUS_18_PX))
                    .bg(p.warning_amber.with_alpha(ALPHA_10).hsla())
                    .row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .child(div().flex_1().min_w(px(DIM_200_PX)).text_size(px(TEXT_12_5_PX)).text_color(p.on_surface.hsla()).child(t("A newer release is ready. Your settings and on / off state are kept.")))
                    .child(update),
            )
        });
        // Installed although this release failed the compliance check.
        self.market_local_prime(cx);
        let flag_note = self.installed_flag(&m.id).map(|f| super::market_page::flag_note(&f, p));
        let essential_note = essential.map(|why| {
            div()
                .px(px(SPACE_14_PX))
                .py(px(SPACE_10_PX))
                .rounded(px(RADIUS_12_PX))
                .row()
                .items_start()
                .gap(px(SPACE_8_PX))
                .bg(p.primary.with_alpha(ALPHA_12).hsla())
                .text_size(px(TEXT_12_5_PX))
                .text_color(p.on_surface.hsla())
                .child(icon("lock", ICON_14_PX, p.primary))
                .child(div().flex_1().min_w(px(0.0)).child(mandara_ui::bidi::paragraph(tf("Part of Mandara's core, so it is always on. {reason}", &[("reason", &tr(why))]))))
        });

        div()
            .row()
            .flex_col()
            .child(hero)
            .child(
                div()
                    .px(px(BODY_PAD))
                    .pt(px(SPACE_18_PX))
                    .pb(px(BODY_PAD))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_22_PX))
                    .child(info_strip)
                    .children(essential_note)
                    .children(flag_note)
                    .children(message)
                    .child(band)
                    .child(section(t("About"), p).child(div().text_size(px(TEXT_13_5_PX)).line_height(px(LEADING_21_PX)).text_color(p.on_surface.with_alpha(ALPHA_86).hsla()).when(rtl, |el| el.text_right()).child(mandara_ui::bidi::paragraph(m.display_description()))))
                    .children(whats_new)
                    .child(section(if builtin { t("System access") } else { t("Permissions") }, p).child(permissions))
                    .child(section(t("Settings"), p).child(settings_card))
                    .child(section(t("Fits in"), p).child(fits).child(placement))
                    .child(section(t("Manage"), p).child(manage)),
            )
            .into_any_element()
    }

    fn plugin_settings_body(&mut self, m: &PluginManifest, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let list = plugin_settings(m);
        let rtl = mandara_i18n::language().is_rtl();
        let fonts = mandara_ui::fonts::fonts(cx);
        let id = m.id.clone();
        let header = div()
            .row()
            .items_center()
            .gap(px(SPACE_14_PX))
            // Room for the close button.
            .when(rtl, |el| el.pl(px(CLOSE_SIZE + CLOSE_CLEARANCE)))
            .when(!rtl, |el| el.pr(px(CLOSE_SIZE + CLOSE_CLEARANCE)))
            .child(plugin_tile(m, SETTINGS_TILE, true, self.modal.scale, p))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .row()
                    .flex_col()
                    .gap(px(2.0))
                    .when(rtl, |el| el.items_end())
                    .child(div().font_family(fonts.display.clone()).text_size(px(TEXT_20_PX)).font_weight(FontWeight::SEMIBOLD).child(tf("{plugin} settings", &[("plugin", &m.display_name())])))
                    .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(tf("{n} settings · changes apply live", &[("n", &list.len())]))),
            )
            .child(action("plugin-modal-about", mandara_ui::bidi::dir_icon("chevron-right"), t("About").into(), false, p).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.open_plugin(&id, ModalView::Detail, window, cx))));
        let body = if list.is_empty() {
            div()
                .py(px(SPACE_30_PX))
                .row()
                .flex_col()
                .items_center()
                .gap(px(SPACE_8_PX))
                .child(icon("sliders", ICON_26_PX, p.on_surface_variant.with_alpha(ALPHA_60)))
                .child(div().font_weight(FontWeight::MEDIUM).child(t("Nothing to set up")))
                .child(div().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("This plugin works without settings.")))
                .into_any_element()
        } else {
            self.rows(&list, cfg, p, cx)
        };
        div()
            .p(px(BODY_PAD))
            .row()
            .flex_col()
            .gap(px(SPACE_22_PX))
            .child(header)
            .when_some(self.error.clone(), |el, e| el.child(div().px(px(SPACE_14_PX)).py(px(SPACE_10_PX)).rounded(px(RADIUS_12_PX)).bg(p.urgent_coral.with_alpha(ALPHA_15).hsla()).text_color(p.urgent_coral.hsla()).child(e)))
            .child(body)
            .into_any_element()
    }

    /// Removes an installed third-party plugin and reloads the plugin list.
    fn uninstall_plugin(&mut self, id: &str, dir: &std::path::Path, cx: &mut Context<Self>) {
        let source = std::fs::read_to_string(dir.join(".source")).ok().and_then(|s| s.lines().next().map(str::to_string));
        let result = match source {
            Some(repo) => mandara_services::marketplace::uninstall(&repo).map(|_| ()),
            None => std::fs::remove_dir_all(dir).map_err(|e| e.to_string()),
        };
        match result {
            Ok(()) => {
                let id = id.to_string();
                update_config(cx, |c| {
                    c.plugins.enabled.retain(|x| x != &id);
                    for lane in [&mut c.bar.left, &mut c.bar.center, &mut c.bar.right] {
                        lane.retain(|x| x != &id);
                    }
                    for tab in &mut c.sidebar.tabs {
                        tab.cards.retain(|x| x != &id);
                    }
                });
                reload_plugins(cx);
                self.close_plugin(cx);
            }
            Err(e) => {
                self.modal.confirm_uninstall = false;
                self.modal.message = Some((false, tf("Couldn't uninstall: {error}", &[("error", &e)])));
                cx.notify();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn plugins_own_their_tables_and_sections() {
        assert!(owns("weather", "/plugins/settings/weather/latitude"));
        assert!(!owns("weather", "/plugins/settings/weathervane/x"));
        assert!(owns("weather", "/general/units"));
        assert!(owns("wallpaper", "/wallpaper/clock_x"));
        assert!(!owns("clock", "/general/units"));
        assert!(owns("media", "/plugins/settings/media/visualizer_bars"));
    }

    #[test]
    fn builtins_with_settings_list_them() {
        let all = registry::settings();
        let count = |id: &str| all.iter().filter(|s| owns(id, s.path)).count();
        assert!(count("weather") >= 6, "weather: location, units, animation");
        assert!(count("prayer-times") >= 4);
        assert!(count("screenshot") >= 4);
        assert!(count("wallpaper") >= 20);
        assert!(count("pomodoro") >= 5);
        // Every plugin table in the registry has an owner (no orphans).
        for s in all.iter().filter(|s| s.path.starts_with("/plugins/settings/")) {
            let owner = s.path.trim_start_matches("/plugins/settings/").split('/').next().unwrap();
            assert!(owns(owner, s.path), "{}", s.path);
        }
    }

    #[test]
    fn owned_paths_exist_in_the_registry() {
        let all = registry::settings();
        for (id, paths) in OWNED_SETTINGS {
            for path in *paths {
                let found = all.iter().any(|s| if path.ends_with('/') { s.path.starts_with(path) } else { s.path == *path });
                assert!(found, "{id}: {path} isn't a registry setting");
            }
        }
    }

    #[test]
    fn declared_settings_become_live_controls() {
        let toml_text = r#"
            [[settings]]
            key = "hemisphere"
            type = "choice"
            label = "Hemisphere"
            default = "north"
            choices = [{ value = "north", label = "Northern" }, "south"]

            [[settings]]
            key = "radius"
            type = "number"
            min = 1
            max = 10
            step = 1

            [[settings]]
            key = "show"
            type = "bool"
        "#;
        #[derive(serde::Deserialize)]
        struct Raw {
            settings: Vec<ManifestSetting>,
        }
        let raw: Raw = toml::from_str(toml_text).unwrap();
        let mut m = PluginManifest::builtin("moon-test", "Moon test", "", None);
        m.origin = PluginOrigin::ThirdParty { path: std::env::temp_dir() };
        m.settings = mandara_plugins::valid_settings("moon-test", raw.settings);
        let list = plugin_settings(&m);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].path, "/plugins/settings/moon-test/hemisphere");
        assert!(matches!(list[0].control, Control::Choice(c) if c.len() == 2 && c[1] == ("south", "south")));
        assert!(matches!(list[1].control, Control::Slider { min, max, .. } if min == 1.0 && max == 10.0));
        assert_eq!(list[2].control, Control::Toggle);
        // Values round-trip through the config like any registry setting.
        let cfg = ShellConfig::default();
        assert_eq!(registry::get(&cfg, &list[0]), Value::from("north"));
        let next = registry::set(&cfg, &list[0], Value::from("south")).unwrap();
        assert_eq!(next.plugins.setting("moon-test", "hemisphere", String::new()), "south");
        // Cached: the same leaked settings come back.
        assert_eq!(plugin_settings(&m)[0].path.as_ptr(), list[0].path.as_ptr());
    }

    #[test]
    fn every_builtin_access_entry_names_a_category_plugin() {
        let known: Vec<&str> = PLUGIN_CATEGORIES.iter().flat_map(|(_, ids)| ids.iter().copied()).chain(["launcher", "tray"]).collect();
        for (id, _) in BUILTIN_ACCESS {
            assert!(known.contains(id), "{id}");
        }
    }
}
