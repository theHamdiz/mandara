//! The main menu (right-click empty bar space, or `mandara msg menu`): a
//! small control center. A rail of pages on the start side (Home, Apps,
//! Look, Session) and a header with Studio, the power menu and close.
//!
//! * Home: you (picture, name, user@host, uptime, version) over your
//!   wallpaper; what's playing with its controls, or the time and weather;
//!   and the quick toggles (the sidebar's quick settings).
//! * Apps: launcher, wallpapers, sidebar, Studio, screenshot, clipboard
//!   history, desktop edit mode.
//! * Look: light / dark, next theme, the themes themselves, wallpapers.
//! * Session: lock, the power menu, reload the config.
//!
//! The panel is glass (`panels.menu_translucent`, `panels.menu_opacity`)
//! and the cards on it are solid. Every action tile wears its own faint
//! ornament in its own accent (`panels.menu_patterns`, see [`ornament`]).

pub(crate) mod ornament;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use gpui::{App, FontWeight, ImageSource, InteractiveElement, IntoElement, ObjectFit, ParentElement, StatefulInteractiveElement, Styled, StyledImage as _, Window, div, img, prelude::FluentBuilder as _, px, size};
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui};
use mandara_i18n::{t, tf, tk, tr};
use mandara_plugins::{CommandSpec, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry};
use mandara_services::MediaCommand;
use mandara_config::power::{SessionEntry, SessionEntryKind};
use mandara_config::surfaces::MenuRail;
use mandara_theme::{MandaraPalette, ThemeSource, catalog};
use serde_json::json;

use super::fn_view;
use crate::bidi::Bidi as _;
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::{card, icon_button, reveal, text, tokens};
use crate::state::{config, shell, update_config};
use mandara_config::surfaces::MenuTileSurface;
use ornament::{Motif, Ornament};

pub const ID: &str = "main-menu";

const WIDTH: f32 = 760.0;
/// With the compact rail (icons only) the menu is narrower by the width the
/// labels took, so the cards keep their size either way.
const WIDTH_COMPACT: f32 = 640.0;
/// Before the first measurement (the menu is as tall as its page).
const HEIGHT: f32 = 468.0;
/// Size changes smaller than this don't resize the panel.
const SIZE_EPSILON: f32 = 0.5;
const PAD: f32 = 12.0;
/// The rail: pages, then shortcuts, then settings. Each row shows its name
/// beside the icon, so the rail is wide enough for the longest label at the
/// default size (a longer translation truncates rather than clipping the
/// panel) and the panel grew by the same amount to keep the cards their
/// width. Compact hides the names and takes the old icon-only width back.
const RAIL_W: f32 = 168.0;
const RAIL_COMPACT_W: f32 = 48.0;
const RAIL_BUTTON: f32 = 32.0;
const RAIL_ICON: f32 = 17.0;
/// The compact rail keeps the original icon size and the open page's
/// light tint.
const RAIL_COMPACT_ICON: f32 = 16.0;
const RAIL_ON_TINT: f32 = 0.18;
const RAIL_GAP: f32 = 3.0;
/// The hairline between the rail's groups (as wide as the icons).
const RAIL_RULE_ALPHA: f32 = 0.3;
const RAIL_COMPACT_RULE_W: f32 = 20.0;

/// Rail shortcuts: a panel of its own each (icon, plugin id, name).
const SHORTCUTS: [(&str, &str, &str); 7] = [
    ("volume-high", "audio", tk("Sound")),
    ("wifi-3", "network", tk("Network")),
    ("bluetooth", "bluetooth", tk("Bluetooth")),
    ("gauge", "resources", tk("System monitor")),
    ("cloud-sun", "weather", tk("Weather")),
    ("calendar", "clock", tk("Calendar")),
    ("bell", "notifications", tk("Notifications")),
];
/// Header buttons.
const HEADER_H: f32 = 36.0;
const HEADER_BUTTON: f32 = 30.0;
/// Home: the profile card, the avatar, the quick toggles' column.
const PROFILE_H: f32 = 132.0;
const AVATAR: f32 = 76.0;
const TOGGLES_W: f32 = 268.0;
/// Media card art and transport buttons.
const ART: f32 = 76.0;
const TRANSPORT: f32 = 30.0;
/// Clock card text.
const CLOCK_TEXT: f32 = 40.0;
/// Action tiles (Apps, Look, Session), three to a row, and the theme
/// swatches.
const TILE_ICON: f32 = 22.0;
const TILES_PER_ROW: usize = 3;
const SWATCH: f32 = 22.0;
/// The Look page: its one row of tiles, then the themes, three to a row.
const LOOK_TILES_H: f32 = 84.0;
const THEMES_PER_ROW: usize = 3;
const THEME_ROW_H: f32 = 38.0;
/// Theme rows shown without scrolling.
const THEME_ROWS_FIT: usize = 6;
/// The profile card's shade over the wallpaper (start → end).
const SHADE: (f32, f32) = (0.72, 0.18);
/// Between stacked lines of text (a name over its details).
const LINE_GAP: f32 = 2.0;
/// Secondary text on the profile card.
const SOFT_TEXT: f32 = 0.78;
/// Tile ornaments. Kufic words are drawn at one scale (px per font unit,
/// so every word's strokes match) in the tile's top-end corner, level
/// with the icon; a geometric motif is a square of `MOTIF_PX` sitting in
/// the bottom-end corner, overhanging both edges so the tile crops it.
const WORD_PX_PER_UNIT: f32 = 0.046;
const WORD_INSET: f32 = 10.0;
const MOTIF_PX: f32 = 136.0;
const MOTIF_INSET: f32 = -24.0;
/// The Look page's short tiles draw their ornaments this much smaller,
/// and tuck a motif further into the corner, clear of the label.
const COMPACT: f32 = 0.7;
const COMPACT_MOTIF_INSET: f32 = -38.0;
/// The ornament's ink at rest and under the pointer (an upper bound: the
/// label's WCAG contrast caps it per theme, [`ornament::ink_alpha`]).
/// Solid calligraphy needs less than hairline geometry to read the same.
const WORD_INK: (f32, f32) = (0.09, 0.14);
const MOTIF_INK: (f32, f32) = (0.22, 0.34);
/// The lighter lip's ink, and how far it peeks out (toward the
/// bottom-right, where the rim's light falls).
const WORD_LIP: (f32, f32) = (0.10, 0.16);
const MOTIF_LIP: (f32, f32) = (0.22, 0.34);
const LIP_OFFSET_PX: f32 = 1.0;
/// The ink that moves away from the text's lightness (the shade on a
/// dark tile, the lip on a light one, [`ornament::inks`]) can only add to
/// the text's contrast; it takes this much more ink to read the same.
const AWAY_GAIN: f32 = 3.0;
/// How far the tile's surface leans toward its accent at the ornament's
/// corner (an OKLCH mix): an ambient wash, not a colour block.
const WASH: f32 = 0.08;
/// CSS angles of that wash: toward the bottom-end corner.
const WASH_ANGLE_LTR: f32 = 135.0;
const WASH_ANGLE_RTL: f32 = 225.0;
/// Under the pointer the ornament drifts this far into the tile.
const DRIFT_PX: f32 = 3.0;
/// Glass tiles (`panels.menu_tile_surface`): how much of the tile's fill
/// covers the menu's glass behind it.
const GLASS_FILL: f32 = 0.55;
/// The tiles' drop shadow at full strength (`panels.menu_tile_shadow`):
/// kept tight, since compositors blur layer surfaces wherever alpha > 0.
const SHADOW_ALPHA: f32 = 0.45;
const SHADOW_OFFSET_PX: f32 = 3.0;
const SHADOW_BLUR_PX: f32 = 10.0;
/// OKLCH lightness of the shadow's ink (a near-black).
const SHADOW_LIGHTNESS: f32 = 0.08;
/// The hover glow's spring: quick and settled, no overshoot to speak of.
const GLOW_SPRING: SpringConfig = SpringConfig::new(320.0, 34.0, 1.0);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Page {
    Home,
    Apps,
    Look,
    Session,
}

impl Page {
    const ALL: [Page; 4] = [Page::Home, Page::Apps, Page::Look, Page::Session];

    fn title(self) -> &'static str {
        match self {
            Page::Home => tk("Home"),
            Page::Apps => tk("Apps & tools"),
            Page::Look => tk("Look"),
            Page::Session => tk("Session"),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Page::Home => "home",
            Page::Apps => "grid",
            Page::Look => "palette",
            Page::Session => "power",
        }
    }
}

type Run = fn(&mut App);

/// One action on the Apps / Look / Session pages.
struct Action {
    icon: &'static str,
    label: &'static str,
    hint: &'static str,
    run: Run,
    /// Its tile's ornament and accent hue.
    ornament: Ornament,
}

fn close(cx: &mut App) {
    shell(cx).update(cx, |s, cx| s.set_panel(None, cx));
}

const APPS: &[Action] = &[
    Action { icon: "grid", label: tk("Launcher"), ornament: Ornament::word("launcher", Motif::Grid, 0.0), hint: "Super+Space", run: |cx| {
        close(cx);
        shell(cx).update(cx, |s, cx| s.set_launcher(true, cx));
    } },
    Action { icon: "image", label: tk("Wallpapers"), ornament: Ornament::word("wallpapers", Motif::Chevrons, 24.0), hint: tk("picker"), run: |cx| {
        close(cx);
        shell(cx).update(cx, |s, cx| s.set_wallpaper_picker(true, cx));
    } },
    Action { icon: "sidebar", label: tk("Sidebar"), ornament: Ornament::word("sidebar", Motif::Columns, 48.0), hint: tk("toggle"), run: |cx| {
        close(cx);
        let s = shell(cx);
        let open = !s.read(cx).sidebar_visible();
        s.update(cx, |s, cx| s.set_sidebar(open, cx));
    } },
    Action { icon: "monitor", label: tk("Mandara Studio"), ornament: Ornament::word("studio", Motif::Honeycomb, -24.0), hint: tk("settings"), run: |cx| {
        close(cx);
        crate::state::open_studio(cx);
    } },
    Action { icon: "crop", label: tk("Screenshot"), ornament: Ornament::word("screenshot", Motif::Frames, -48.0), hint: "Super+Shift+S", run: |cx| {
        close(cx);
        crate::snip::open(crate::snip::SnipMode::Region, cx);
    } },
    Action { icon: "clipboard", label: tk("Clipboard history"), ornament: Ornament::word("clipboard", Motif::Sheets, 72.0), hint: "Super+Shift+V", run: |cx| {
        shell(cx).update(cx, |s, cx| s.set_panel(Some("clipboard".into()), cx));
    } },
    Action { icon: "pencil", label: tk("Edit desktop"), ornament: Ornament::word("edit", Motif::Lattice, -72.0), hint: tk("widgets"), run: |cx| {
        close(cx);
        crate::desktop_edit::enter(cx);
    } },
];

const LOOK: &[Action] = &[
    Action { icon: "moon", label: tk("Toggle light / dark"), ornament: Ornament::motif(Motif::Shamsa, 0.0), hint: "Super+Shift+T", run: |cx| {
        let mode = config(cx).read(cx).get().theme.mode.toggled();
        update_config(cx, |c| c.theme.mode = mode);
    } },
    Action { icon: "sun", label: tk("Next theme"), ornament: Ornament::motif(Motif::Zellige, 40.0), hint: "theme next", run: |cx| {
        let next = match &config(cx).read(cx).get().theme.source {
            ThemeSource::Preset { preset } => catalog::next_id(preset),
            _ => catalog::DEFAULT_THEME.to_string(),
        };
        apply_theme(next, cx);
    } },
    Action { icon: "image", label: tk("Wallpapers"), ornament: Ornament::word("wallpapers", Motif::Chevrons, 24.0), hint: tk("picker"), run: |cx| {
        close(cx);
        shell(cx).update(cx, |s, cx| s.set_wallpaper_picker(true, cx));
    } },
];

const SESSION: &[Action] = &[
    Action { icon: "lock", label: tk("Lock"), ornament: Ornament::motif(Motif::Mashrabiya, 0.0), hint: "", run: |cx| session(SessionEntryKind::Lock, cx) },
    Action { icon: "logout", label: tk("Log out"), ornament: Ornament::motif(Motif::Doorway, 30.0), hint: "", run: |cx| session(SessionEntryKind::Logout, cx) },
    Action { icon: "moon", label: tk("Sleep"), ornament: Ornament::motif(Motif::Waves, -40.0), hint: "", run: |cx| session(SessionEntryKind::Suspend, cx) },
    Action { icon: "restart", label: tk("Restart"), ornament: Ornament::motif(Motif::Rings, 60.0), hint: "", run: |cx| session(SessionEntryKind::Reboot, cx) },
    Action { icon: "power", label: tk("Shut down"), ornament: Ornament::motif(Motif::Sunset, -70.0), hint: "", run: |cx| session(SessionEntryKind::Shutdown, cx) },
    Action { icon: "refresh", label: tk("Reload config"), ornament: Ornament::motif(Motif::Pharos, 90.0), hint: "", run: |cx| {
        close(cx);
        let _ = crate::state::reload_config_from_disk(cx);
    } },
];

/// Runs a session action the way the power menu does (its failures show
/// as a notification).
fn session(kind: SessionEntryKind, cx: &mut App) {
    close(cx);
    let cfg = config(cx).read(cx).snapshot();
    if let Err(e) = crate::powerctl::session_menu::run(&SessionEntry::new(kind, true), &cfg, cx) {
        tracing::warn!("main menu: {e}");
    }
}

fn apply_theme(id: String, cx: &mut App) {
    update_config(cx, |c| {
        c.theme.source = ThemeSource::Preset { preset: id };
        c.wallpaper.match_colors = false;
    });
}

// ── You ─────────────────────────────────────────────────────────────────

/// Your name, login, host, picture, uptime.
pub(crate) struct Me {
    pub(crate) name: String,
    pub(crate) login: String,
    pub(crate) host: String,
    pub(crate) picture: Option<PathBuf>,
}

pub(crate) fn me() -> Me {
    let uid = rustix::process::getuid().as_raw();
    let user = std::fs::read_to_string("/etc/passwd").ok().and_then(|t| crate::lock::parse_passwd(&t, uid));
    let login = user.as_ref().map(|u| u.name.clone()).or_else(|| std::env::var("USER").ok()).unwrap_or_default();
    let name = user.as_ref().map(|u| u.display.clone()).filter(|d| !d.is_empty()).unwrap_or_else(|| login.clone());
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname").map(|h| h.trim().to_string()).unwrap_or_default();
    let home = user.map(|u| u.home).or_else(dirs::home_dir).unwrap_or_default();
    let picture = [home.join(".face"), PathBuf::from(format!("/var/lib/AccountsService/icons/{login}"))].into_iter().find(|p| p.is_file());
    Me { name, login, host, picture }
}

/// "3 h 17 min" since boot.
pub(crate) fn uptime_secs() -> Option<u64> {
    Some(std::fs::read_to_string("/proc/uptime").ok()?.split_whitespace().next()?.parse::<f64>().ok()? as u64)
}

fn uptime() -> Option<String> {
    let secs = std::fs::read_to_string("/proc/uptime").ok()?.split_whitespace().next()?.parse::<f64>().ok()? as u64;
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    Some(match (d, h) {
        (0, 0) => tf("{m} min", &[("m", &m)]),
        (0, _) => tf("{h} h {m} min", &[("h", &h), ("m", &m)]),
        _ => tf("{d} d {h} h", &[("d", &d), ("h", &h)]),
    })
}

// ── Pages ───────────────────────────────────────────────────────────────

fn profile(palette: &MandaraPalette, cx: &App) -> gpui::AnyElement {
    let me = me();
    let cfg = config(cx).read(cx).snapshot();
    let wall = mandara_config::desktop::effective_wallpaper(cfg.wallpaper.path.as_ref(), &cfg.wallpaper.variants, None, cfg.theme.mode.is_dark()).and_then(|p| crate::wallpaper::still(&p).ok());
    let avatar = match &me.picture {
        Some(p) => img(p.clone()).size(px(AVATAR)).rounded_full().object_fit(ObjectFit::Cover).into_any_element(),
        None => div()
            .size(px(AVATAR))
            .rounded_full()
            .bg(palette.primary.hsla())
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(AVATAR * 0.36))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(palette.on_primary.hsla())
            .child(mandara_services::accounts::initials(&me.name))
            .into_any_element(),
    };
    let white = gpui::white();
    let lines = div()
        .flex()
        .flex_col()
        .gap(px(LINE_GAP))
        .min_w(px(0.0))
        .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).text_color(white).truncate().child(me.name))
        .child(div().text_size(px(text::BODY_S)).text_color(white.opacity(SOFT_TEXT)).truncate().child(format!("{}@{}", me.login, me.host)))
        .when_some(uptime(), |el, up| el.child(div().text_size(px(text::BODY_S)).text_color(white.opacity(SOFT_TEXT)).child(tf("Up {time}", &[("time", &up)]))))
        .child(div().text_size(px(text::LABEL)).text_color(white.opacity(SOFT_TEXT)).child(format!("Mandara {}", mandara_services::self_update::running_version())));
    div()
        .relative()
        .h(px(PROFILE_H))
        .rounded(px(crate::style::card_radius()))
        .overflow_hidden()
        .bg(palette.surface_container.hsla())
        .when_some(wall, |el, w| el.child(img(ImageSource::from(w)).absolute().top_0().left_0().size_full().object_fit(ObjectFit::Cover)))
        .child(div().absolute().top_0().left_0().size_full().bg(mandara_core::oklab_gradient(90.0, mandara_theme::Color::oklcha(0.12, 0.0, 0.0, SHADE.0), mandara_theme::Color::oklcha(0.12, 0.0, 0.0, SHADE.1))))
        .child(div().absolute().top_0().left_0().size_full().px(px(tokens::LG)).flex().items_center().gap(px(tokens::LG)).child(avatar).child(lines))
        .into_any_element()
}

/// What's playing, or nothing.
fn media(palette: &MandaraPalette, cx: &App) -> Option<gpui::AnyElement> {
    let hub = crate::hub::hub(cx).read(cx);
    let m = hub.media.as_ref().filter(|m| !m.title.is_empty())?;
    let playing = m.status == mandara_services::PlaybackStatus::Playing;
    let art = match &m.art_path {
        Some(p) => img(p.clone()).size(px(ART)).rounded(px(tokens::MD)).object_fit(ObjectFit::Cover).into_any_element(),
        None => div().size(px(ART)).rounded(px(tokens::MD)).bg(palette.surface_container_high.hsla()).flex().items_center().justify_center().child(icon("music", ART * 0.4, palette.on_surface_variant)).into_any_element(),
    };
    let transport = |id: &'static str, glyph: &'static str, command: MediaCommand| icon_button(id, glyph, TRANSPORT, palette.on_surface, None, move |_, cx| super::media::send(cx, command.clone()));
    Some(
        card(palette)
            .flex_1()
            .flex()
            .gap(px(tokens::MD))
            .items_center()
            .child(art)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(LINE_GAP))
                    .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().child(m.title.clone()))
                    .child(div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).truncate().child(m.artist.clone()))
                    .child(
                        div()
                            .flex()
                            .gap(px(tokens::XS))
                            .pt(px(tokens::XS))
                            .child(transport("menu-prev", "previous", MediaCommand::Previous))
                            .child(transport("menu-play", if playing { "pause" } else { "play" }, MediaCommand::PlayPause))
                            .child(transport("menu-next", "next", MediaCommand::Next)),
                    ),
            )
            .into_any_element(),
    )
}

/// The time, the date and the weather.
fn clock(palette: &MandaraPalette, cx: &App) -> gpui::AnyElement {
    let cfg = config(cx).read(cx).snapshot();
    let now = chrono::Local::now();
    let time = now.format(if cfg.general.clock_24h { "%H:%M" } else { "%-I:%M" }).to_string();
    let date = crate::widgets::clock::localized(&now, t("%A, %-d %B"));
    let weather = crate::hub::hub(cx).read(cx).weather.weather.as_ref().map(|w| format!("{} · {}", cfg.units().format_temperature(w.temp), tr(w.condition.label())));
    card(palette)
        .flex_1()
        .flex()
        .items_center()
        .gap(px(tokens::LG))
        .child(div().font_family(fonts(cx).display).font_features(tabular_nums()).text_size(px(CLOCK_TEXT)).font_weight(FontWeight::SEMIBOLD).text_color(palette.primary.hsla()).child(time))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w(px(0.0))
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child(date))
                .when_some(weather, |el, w| el.child(div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).truncate().child(w))),
        )
        .into_any_element()
}

/// A reveal that fills what's left of its column.
fn fill(p: &PanelCx, index: usize, el: impl IntoElement) -> gpui::AnyElement {
    let r = p.reveal(index);
    div().relative().top(px(r.offset)).opacity(r.opacity).flex_1().min_h(px(0.0)).flex().flex_col().child(el).into_any_element()
}

/// A reveal that keeps its row's full height and centres its content in
/// it (the rail: exactly as much space above it as below).
fn fill_row(p: &PanelCx, index: usize, el: impl IntoElement) -> gpui::AnyElement {
    let r = p.reveal(index);
    div().relative().top(px(r.offset)).opacity(r.opacity).flex().flex_col().justify_center().child(el).into_any_element()
}

fn home(palette: &MandaraPalette, p: &PanelCx, cx: &mut App) -> gpui::AnyElement {
    // The cards on the left share the toggles' height.
    let left = div().flex_1().min_w(px(0.0)).flex().flex_col().gap(px(tokens::SM)).children(media(palette, cx).map(|m| div().flex_1().flex().flex_col().child(m))).child(div().flex_1().flex().flex_col().child(clock(palette, cx)));
    let toggles = div().w(px(TOGGLES_W)).flex_none().child(super::quick_settings::tiles(palette, cx));
    let volume = card(palette).child(super::audio::volume_row(palette, cx));
    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap(px(tokens::SM))
        .child(reveal(p, 1, profile(palette, cx)))
        .child(fill(p, 2, div().flex_1().flex().gap(px(tokens::SM)).child(left).child(toggles)))
        .child(reveal(p, 3, volume))
        .into_any_element()
}

/// Each tile's hover glow, springing between rest (0) and under the
/// pointer (1), and each tile's colours. Springs step only while one
/// moves (idle, nothing runs); colours are worked out once per palette.
#[derive(Default)]
struct Glow {
    springs: HashMap<&'static str, SpringValue<f32>>,
    clock: FrameClock,
    /// The palette the colours were worked out for, and the colours per
    /// tile (by label).
    palette: Option<PaletteKey>,
    inks: HashMap<(&'static str, bool), TileInk>,
}

/// The palette roles a tile's colours depend on, as their OKLCH bits
/// (cheap to compare on every render).
type PaletteKey = [[u32; 3]; 5];

fn palette_key(p: &MandaraPalette) -> PaletteKey {
    [p.primary, p.surface_container, p.surface_container_high, p.on_surface, p.on_surface_variant].map(|c| [c.lightness(), c.chroma(), c.hue()].map(f32::to_bits))
}

/// A tile's colours: its ornament's inks with their alphas (rest, under
/// the pointer), its icon, and its surface at rest and hovered.
#[derive(Clone)]
struct TileInk {
    shade: mandara_theme::Color,
    lip: mandara_theme::Color,
    shade_alpha: (f32, f32),
    lip_alpha: (f32, f32),
    icon: mandara_theme::Color,
    wash_rest: mandara_theme::Color,
    wash_high: mandara_theme::Color,
}

impl TileInk {
    fn new(o: Ornament, palette: &MandaraPalette) -> Self {
        let accent = ornament::accent(palette, o.hue);
        let (rest, high) = (palette.surface_container, palette.surface_container_high);
        let (wash_rest, wash_high) = (rest.mix(accent, WASH), high.mix(accent, WASH));
        let texts = [palette.on_surface, palette.on_surface_variant];
        let inks = ornament::inks(accent, rest, palette.on_surface);
        let (shade_gain, lip_gain) = if inks.carved { (AWAY_GAIN, 1.0) } else { (1.0, AWAY_GAIN) };
        let (shade, lip) = if o.is_word() { (WORD_INK, WORD_LIP) } else { (MOTIF_INK, MOTIF_LIP) };
        // The busiest spot under the text: the washed corner, inked at
        // most; under the pointer the ink deepens as far as the text allows.
        let alpha = |colour, (rest_want, hover_want): (f32, f32)| {
            let at_rest = ornament::ink_alpha(colour, wash_rest, &texts, rest_want).min(ornament::ink_alpha(colour, wash_high, &texts, rest_want));
            (at_rest, ornament::ink_alpha(colour, wash_high, &texts, hover_want).max(at_rest))
        };
        Self {
            shade: inks.shade,
            lip: inks.lip,
            shade_alpha: alpha(inks.shade, (shade.0 * shade_gain, shade.1 * shade_gain)),
            lip_alpha: alpha(inks.lip, (lip.0 * lip_gain, lip.1 * lip_gain)),
            icon: ornament::icon_ink(ornament::icon_ink(accent, palette.primary, rest), palette.primary, high),
            wash_rest,
            wash_high,
        }
    }
}

impl Glow {
    fn value(&self, key: &'static str) -> f32 {
        self.springs.get(key).map_or(0.0, |s| s.value())
    }

    fn aim(&mut self, key: &'static str, on: bool) {
        self.springs.entry(key).or_insert_with(|| SpringValue::new(0.0, GLOW_SPRING)).set_target(if on { 1.0 } else { 0.0 });
    }

    /// Advances every moving spring one frame; whether any still moves.
    fn step(&mut self) -> bool {
        if !self.springs.values().any(|s| s.is_animating()) {
            // The next hover starts from a nominal frame, not the idle gap.
            self.clock = FrameClock::new();
            return false;
        }
        let dt = self.clock.tick(Instant::now());
        let profile = mandara_core::spring::motion_profile();
        let mut moving = false;
        for s in self.springs.values_mut() {
            moving |= s.step_in(dt, &profile);
        }
        moving
    }

    /// Forgets the tiles' colours when the palette changed.
    fn follow(&mut self, palette: &MandaraPalette) {
        let key = palette_key(palette);
        if self.palette != Some(key) {
            self.palette = Some(key);
            self.inks.clear();
        }
    }

    /// `a`'s colours, worked out on first use and kept until the palette
    /// changes ([`Self::follow`]).
    fn ink(&mut self, a: &Action, o: Ornament, palette: &MandaraPalette) -> TileInk {
        self.inks.entry((a.label, o.is_word())).or_insert_with(|| TileInk::new(o, palette)).clone()
    }
}

/// What a page of tiles draws with.
#[derive(Copy, Clone)]
struct TileCx<'a> {
    glow: &'a Rc<RefCell<Glow>>,
    /// `panels.menu_patterns`.
    patterns: bool,
    /// `panels.menu_calligraphy`: Kufic words on the app and tool tiles.
    calligraphy: bool,
    /// Short tiles (the Look page's one row).
    compact: bool,
    /// `panels.menu_tile_surface`.
    surface: mandara_config::surfaces::MenuTileSurface,
    /// `panels.menu_tile_shadow`, 0–1.
    shadow: f32,
}

/// A tile's ornament (its lip, then its body), its icon colour and its
/// surface (rest, hover).
fn ornament_layer(o: Ornament, ink: &TileInk, palette: &MandaraPalette, glow: f32, compact: bool) -> ([gpui::AnyElement; 2], mandara_theme::Color, gpui::Background, gpui::Background) {
    let rtl = crate::bidi::rtl();
    let angle = if rtl { WASH_ANGLE_RTL } else { WASH_ANGLE_LTR };
    let drift = DRIFT_PX * glow;
    let (k, motif_inset) = if compact { (COMPACT, COMPACT_MOTIF_INSET) } else { (1.0, MOTIF_INSET) };
    let (w, h) = match o.units() {
        Some((w, h)) => (w * WORD_PX_PER_UNIT * k, h * WORD_PX_PER_UNIT * k),
        None => (MOTIF_PX * k, MOTIF_PX * k),
    };
    let layer = |colour: mandara_theme::Color, (rest, hover): (f32, f32), dx: f32, dy: f32| {
        let alpha = rest + (hover - rest) * glow;
        let el = gpui::svg().path(o.path()).absolute().w(px(w)).h(px(h)).text_color(colour.with_alpha(alpha).hsla());
        // Words hang from the top-end corner, motifs sit in the bottom-end
        // one; both drift into the tile under the pointer.
        let el = if o.is_word() { el.top(px(WORD_INSET + drift + dy)) } else { el.bottom(px(motif_inset + drift - dy)) };
        let end = if o.is_word() { WORD_INSET } else { motif_inset };
        (if rtl { el.left(px(end + drift + dx)) } else { el.right(px(end + drift - dx)) }).into_any_element()
    };
    let art = [layer(ink.lip, ink.lip_alpha, LIP_OFFSET_PX, LIP_OFFSET_PX), layer(ink.shade, ink.shade_alpha, 0.0, 0.0)];
    let (rest, high) = (palette.surface_container, palette.surface_container_high);
    (art, ink.icon, mandara_core::oklab_gradient(angle, rest, ink.wash_rest), mandara_core::oklab_gradient(angle, high, ink.wash_high))
}

/// A page of action tiles, three to a row (a short last row spreads
/// across); the rows share the page's height.
fn tiles(actions: &[Action], palette: &MandaraPalette, p: &PanelCx, tc: &TileCx, window: &mut Window) -> gpui::AnyElement {
    if tc.patterns {
        let mut g = tc.glow.borrow_mut();
        g.follow(palette);
        if g.step() {
            window.request_animation_frame();
        }
    }
    let mut grid = div().flex_1().flex().flex_col().gap(px(tokens::SM));
    for (r, chunk) in actions.chunks(TILES_PER_ROW).enumerate() {
        let mut row = div().flex_1().min_h(px(0.0)).flex().gap(px(tokens::SM));
        for (c, a) in chunk.iter().enumerate() {
            let run = a.run;
            let mut tile = div()
                .id(("menu-tile", r * TILES_PER_ROW + c))
                .relative()
                .flex_1()
                .min_w(px(0.0))
                .p(px(tokens::MD))
                .rounded(px(crate::style::card_radius()))
                .cursor_pointer()
                .flex()
                .flex_col()
                .justify_between();
            let (plain_rest, plain_high): (gpui::Background, gpui::Background) = match tc.surface {
                MenuTileSurface::Glass => (palette.surface_container.with_alpha(GLASS_FILL).hsla().into(), palette.surface_container_high.with_alpha(GLASS_FILL).hsla().into()),
                _ => (palette.surface_container.hsla().into(), palette.surface_container_high.hsla().into()),
            };
            // Raised and glass tiles wear the specular rim; any tile may
            // cast a shadow.
            let mut shadows = if tc.surface == MenuTileSurface::Flat { Vec::new() } else { mandara_core::glass_rim(palette.specular_rim_light, palette.specular_rim_shadow) };
            if tc.shadow > 0.0 {
                shadows.push(gpui::BoxShadow {
                    color: mandara_theme::Color::oklcha(SHADOW_LIGHTNESS, 0.0, 0.0, SHADOW_ALPHA * tc.shadow).hsla(),
                    offset: gpui::point(px(0.0), px(SHADOW_OFFSET_PX * tc.shadow)),
                    blur_radius: px(SHADOW_BLUR_PX * tc.shadow),
                    spread_radius: px(0.0),
                    inset: false,
                });
            }
            tile = tile.shadow(shadows);
            let glyph = if tc.patterns {
                let ([lip, body], ink, rest, high) = {
                    let mut g = tc.glow.borrow_mut();
                    let o = a.ornament.shown(tc.calligraphy);
                    let ink = g.ink(a, o, palette);
                    ornament_layer(o, &ink, palette, g.value(a.label), tc.compact)
                };
                // Only raised tiles lean into their accent; flat and glass
                // keep their plain fill.
                let (rest, high) = if tc.surface == MenuTileSurface::Raised { (rest, high) } else { (plain_rest, plain_high) };
                let glow = tc.glow.clone();
                let key = a.label;
                tile = crate::style::ClipRounded::clip_rounded(tile)
                    .bg(rest)
                    .hover(move |s| s.bg(high))
                    .on_hover(move |on: &bool, window, _| {
                        glow.borrow_mut().aim(key, *on);
                        window.refresh();
                    })
                    // A flat tile's ornament has no engraved lip.
                    .when(tc.surface != MenuTileSurface::Flat, |el| el.child(lip))
                    .child(body);
                ink
            } else {
                tile = tile.bg(plain_rest).hover(move |s| s.bg(plain_high));
                palette.primary
            };
            row = row.child(
                // The icon leads the label: at the start, right in RTL.
                tile.child(div().flex().when(crate::bidi::rtl(), |el| el.justify_end()).child(icon(a.icon, TILE_ICON, glyph)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(LINE_GAP))
                            .child(div().truncate().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child(crate::bidi::paragraph(tr(a.label))))
                            .when(!a.hint.is_empty(), |el| el.child(div().truncate().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(crate::bidi::paragraph(tr(a.hint))))),
                    )
                    .on_click(move |_, _, cx| run(cx)),
            );
        }
        grid = grid.child(row);
    }
    fill(p, 1, grid)
}

/// The Look page: its actions, then every theme to pick from.
fn look(palette: &MandaraPalette, p: &PanelCx, tc: &TileCx, window: &mut Window, cx: &App) -> gpui::AnyElement {
    let current = match &config(cx).read(cx).get().theme.source {
        ThemeSource::Preset { preset } => preset.clone(),
        _ => String::new(),
    };
    let dark = config(cx).read(cx).get().theme.mode.is_dark();
    let themes = catalog::all();
    let mut grid = div().flex_1().flex().flex_col().gap(px(tokens::XS));
    for (r, chunk) in themes.chunks(THEMES_PER_ROW).enumerate() {
        let mut row = div().flex_1().min_h(px(THEME_ROW_H)).flex().gap(px(tokens::XS));
        for (c, def) in chunk.iter().enumerate() {
            let pal = def.palette(dark);
            let on = def.id == current;
            let id = def.id.clone();
            row = row.child(
                div()
                    .id(("menu-theme", r * THEMES_PER_ROW + c))
                    .flex_1()
                    .min_w(px(0.0))
                    .px(px(tokens::SM))
                    .flex()
                    .items_center()
                    .gap(px(tokens::SM))
                    .rounded(px(crate::style::radius(tokens::MD)))
                    .cursor_pointer()
                    .when(on, |el| el.bg(palette.primary.with_alpha(0.18).hsla()))
                    .hover(|s| s.bg(palette.surface_container_high.hsla()))
                    .on_click(move |_, _, cx| apply_theme(id.clone(), cx))
                    .child(
                        div()
                            .size(px(SWATCH))
                            .flex_none()
                            .rounded_full()
                            .border_2()
                            .border_color(if on { palette.primary.hsla() } else { palette.outline.with_alpha(RAIL_RULE_ALPHA).hsla() })
                            .bg(mandara_core::oklab_gradient(135.0, pal.surface_dim, pal.primary)),
                    )
                    .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).when(on, |el| el.font_weight(FontWeight::SEMIBOLD)).child(def.name.clone())),
            );
        }
        for _ in chunk.len()..THEMES_PER_ROW {
            row = row.child(div().flex_1().px(px(tokens::SM)));
        }
        grid = grid.child(row);
    }
    // The rows share the card while they fit; more (your own themes) scroll.
    let (scroll, bar) = crate::scrollbar::keyed("menu-themes");
    let fits = themes.len().div_ceil(THEMES_PER_ROW) <= THEME_ROWS_FIT;
    let list = div().id("menu-themes").flex_1().min_h(px(0.0)).flex().flex_col().when(!fits, |el| el.overflow_y_scroll().track_scroll(&scroll)).child(grid);
    let themes_card = card(palette)
        .flex_1()
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .gap(px(tokens::SM))
        .child(div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(t("Themes")))
        .child(if fits { list.into_any_element() } else { crate::scrollbar::with_scrollbar(list, &scroll, &bar, palette).flex_1().min_h(px(0.0)).into_any_element() });
    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap(px(tokens::SM))
        .child(div().h(px(LOOK_TILES_H)).flex().flex_col().child(tiles(LOOK, palette, p, &TileCx { compact: true, ..*tc }, window)))
        .child(fill(p, 2, themes_card))
        .into_any_element()
}

fn render(page: &Rc<Cell<Page>>, glow: &Rc<RefCell<Glow>>, fit: &Rc<Cell<gpui::Size<gpui::Pixels>>>, palette: &MandaraPalette, p: &PanelCx, window: &mut Window, cx: &mut App) -> gpui::AnyElement {
    let now = page.get();
    let compact = config(cx).read(cx).get().panels.menu_rail == MenuRail::Compact;
    let width = if compact { WIDTH_COMPACT } else { WIDTH };
    // A rail row: the entry's icon and name (or just the icon in compact).
    let rail_button = |id: (&'static str, usize), glyph: &'static str, label: &'static str, on: bool| {
        let row = div()
            .id(id)
            .h(px(RAIL_BUTTON))
            .rounded(px(crate::style::radius(tokens::MD)))
            .row()
            .items_center()
            .cursor_pointer()
            .text_size(px(text::BODY_S))
            .font_weight(FontWeight::MEDIUM)
            .when(compact, |el| el.w(px(RAIL_BUTTON)).justify_center())
            .when(!compact, |el| el.w_full().px(px(tokens::SM)).gap(px(tokens::SM)))
            // Full fills the open page like the sidebar's tabs; compact keeps
            // the rail's original light tint.
            .when(on && !compact, |el| el.bg(palette.primary.hsla()).text_color(palette.on_primary.hsla()).font_weight(FontWeight::SEMIBOLD))
            .when(on && compact, |el| el.bg(palette.primary.with_alpha(RAIL_ON_TINT).hsla()))
            .when(!on, |el| el.text_color(palette.on_surface_variant.hsla()))
            .when(!on || compact, |el| el.hover(|s| s.bg(palette.surface_container_high.hsla())))
            .child(icon(
                glyph,
                if compact { RAIL_COMPACT_ICON } else { RAIL_ICON },
                match (on, compact) {
                    (true, false) => palette.on_primary,
                    (true, true) => palette.primary,
                    (false, _) => palette.on_surface_variant,
                },
            ));
        if compact { row } else { row.child(div().flex_1().min_w(px(0.0)).truncate().text_start().child(crate::bidi::paragraph(tr(label)))) }
    };
    let rule = || {
        div()
            .h(px(1.0))
            .my(px(RAIL_GAP))
            .when(compact, |el| el.w(px(RAIL_COMPACT_RULE_W)))
            .when(!compact, |el| el.mx(px(tokens::SM)))
            .bg(palette.outline.with_alpha(RAIL_RULE_ALPHA).hsla())
    };
    // As tall as its buttons, centred in the menu's height.
    let mut rail = div()
        .w(px(if compact { RAIL_COMPACT_W } else { RAIL_W }))
        .flex_none()
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(RAIL_GAP))
        .p(px(tokens::XS))
        .rounded(px(crate::style::card_radius()))
        .bg(palette.surface_container.hsla())
        .when(compact, |el| el.items_center());
    // The menu's own pages.
    for (i, pg) in Page::ALL.into_iter().enumerate() {
        let page = page.clone();
        rail = rail.child(rail_button(("menu-page", i), pg.icon(), pg.title(), pg == now).on_click(move |_, window, _| {
            page.set(pg);
            window.refresh();
        }));
    }
    // Shortcuts: each opens its own panel in the menu's place.
    rail = rail.child(rule());
    for (i, (glyph, id, name)) in SHORTCUTS.into_iter().enumerate() {
        rail = rail.child(rail_button(("menu-shortcut", i), glyph, name, false).on_click(move |_, _, cx| shell(cx).update(cx, |s, cx| s.set_panel(Some(id.into()), cx))));
    }
    // Settings.
    rail = rail.child(rule()).child(rail_button(("menu-settings", 0), "sliders", tk("Mandara Studio"), false).on_click(|_, _, cx| {
        close(cx);
        crate::state::open_studio(cx);
    }));
    let header = div()
        .h(px(HEADER_H))
        .flex()
        .items_center()
        .gap(px(tokens::XS))
        .child(div().flex_1().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).child(crate::bidi::paragraph(tr(now.title()))))
        .child(icon_button("menu-studio", "sliders", HEADER_BUTTON, palette.on_surface_variant, None, |_, cx| {
            close(cx);
            crate::state::open_studio(cx);
        }))
        .child(icon_button("menu-power", "power", HEADER_BUTTON, palette.on_primary, Some(palette.primary), |_, cx| {
            close(cx);
            shell(cx).update(cx, |s, cx| s.set_session(true, cx));
        }))
        .child(icon_button("menu-close", "close", HEADER_BUTTON, palette.on_surface_variant, None, |_, cx| close(cx)));
    let panels = &config(cx).read(cx).get().panels;
    let tc = TileCx { glow, patterns: panels.menu_patterns, calligraphy: panels.menu_calligraphy, compact: false, surface: panels.menu_tile_surface, shadow: panels.menu_tile_shadow() };
    let body = match now {
        Page::Home => home(palette, p, cx),
        Page::Apps => tiles(APPS, palette, p, &tc, window),
        Page::Look => look(palette, p, &tc, window, cx),
        Page::Session => tiles(SESSION, palette, p, &tc, window),
    };
    // Home sets the menu's height (it's the fullest page); the other pages
    // keep it and spread over it, so switching pages doesn't jump.
    let home_page = now == Page::Home;
    let fit = fit.clone();
    let inner = f32::from(fit.get().height) - PAD * 2.0;
    let probe = gpui::canvas(
        move |b, _, _| {
            let cur = fit.get();
            let h = f32::from(b.size.height) + PAD * 2.0;
            // Home sets the height (it's the fullest page); the width follows
            // the rail mode on whichever page is open.
            let next = if home_page && (f32::from(cur.height) - h).abs() > SIZE_EPSILON {
                size(px(width), px(h))
            } else if (f32::from(cur.width) - width).abs() > SIZE_EPSILON {
                size(px(width), cur.height)
            } else {
                cur
            };
            if next != cur {
                fit.set(next);
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full();
    div()
        .size_full()
        .p(px(PAD))
        .child(
            div()
                .relative()
                .flex()
                .gap(px(PAD))
                .when(!home_page, |el| el.h(px(inner)))
                .child(probe)
                .child(fill_row(p, 0, rail))
                .child(div().flex_1().min_w(px(0.0)).flex().flex_col().gap(px(tokens::SM)).child(header).child(div().flex_1().min_h(px(0.0)).flex().flex_col().child(body))),
        )
        .into_any_element()
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Main menu"), tk("Right-click empty bar space"), None), |window, cx, panel_cx| {
        let p = panel_cx.clone();
        let page = Rc::new(Cell::new(Page::Home));
        let glow = Rc::new(RefCell::new(Glow::default()));
        let fit = Rc::new(Cell::new(size(px(WIDTH), px(HEIGHT))));
        let measured = fit.clone();
        let panel = fn_view(None, move |palette, window, cx| render(&page, &glow, &measured, palette, &p, window, cx), window, cx);
        PluginInstance { panel: Some(PanelSpec { view: panel, size: fit }), ..PluginInstance::empty() }
    });
    r.command(ID, CommandSpec::new("menu", mandara_i18n::tk("Open or close the main menu"), |_, cx| {
        let s = shell(cx);
        s.update(cx, |s, cx| s.toggle_panel(ID, cx));
        Ok(json!({ "open": s.read(cx).panel.as_deref() == Some(ID) }))
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each rail mode only takes the width the menu gained: the cards keep
    /// the width they had when the rail was icon-only (640 − 48).
    #[test]
    fn each_rail_keeps_the_cards_their_width() {
        assert_eq!(WIDTH - RAIL_W, 640.0 - 48.0);
        assert_eq!(WIDTH_COMPACT - RAIL_COMPACT_W, WIDTH - RAIL_W);
    }

    /// Every entry of the old list is still on a page.
    #[test]
    fn every_old_entry_is_still_here() {
        let labels: Vec<&str> = APPS.iter().chain(LOOK).chain(SESSION).map(|a| a.label).collect();
        // "Power…" is the header's power button.
        for old in ["Launcher", "Wallpapers", "Sidebar", "Mandara Studio", "Screenshot", "Clipboard history", "Toggle light / dark", "Next theme", "Lock", "Reload config"] {
            assert!(labels.contains(&old), "{old} is missing");
        }
    }
}
