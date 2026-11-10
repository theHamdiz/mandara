//! The main menu (right-click empty bar space, or `mandara msg menu`): a
//! small control center. A rail on the start side holds the menu's own
//! pages (Home, Apps, Look) and, under a rule (and a "Panels" caption in
//! the full rail), shortcuts that open another panel in the menu's place.
//! The header holds Studio, the power menu and close; the rail repeats
//! none of them.
//!
//! * Home: you (picture, name, user@host, uptime, version) over your
//!   wallpaper, on a scrim that keeps them readable on any picture; what's
//!   playing with its controls, or the time and weather; the quick toggles
//!   (the sidebar's quick settings) and the output volume.
//! * Apps: launcher, wallpapers, sidebar, Studio, screenshot, clipboard
//!   history, desktop edit mode, reload the config.
//! * Look: light / dark, next theme, the themes themselves, wallpapers.
//!
//! The panel is glass (`panels.menu_translucent`, `panels.menu_opacity`)
//! and the cards on it are solid. Every action tile wears its own faint
//! ornament in its own accent (`panels.menu_patterns`, see [`ornament`]).

pub(crate) mod ornament;
mod theme_card;
mod card_decoration;

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
use mandara_config::surfaces::{MenuRail, MenuRailHeight};
use mandara_theme::{MandaraPalette, ThemeSource, catalog};
use serde_json::json;

use super::fn_view_observing;
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
/// The full rail's "Panels" caption over the shortcuts: its inset matches
/// the rows' text.
const RAIL_CAPTION_PAD: f32 = 2.0;
/// About one line of that caption (only the stretched rail's sums use it).
const RAIL_CAPTION_H: f32 = 18.0;
/// The stretched rail (`panels.menu_rail_height`) fills the surface under
/// its buttons with a mashrabiya lattice once that is at least this tall
/// (less would be a sliver of beads, not a screen)...
const RAIL_LATTICE_MIN: f32 = 24.0;
/// ...rising at most this far from the rail's foot (the rest stays the
/// rail's plain surface, and only a few copies are ever drawn)...
const RAIL_LATTICE_MAX: f32 = 240.0;
/// ...as copies of a seamless square this big (12 px between beads, the
/// density of the Look page's ornaments).
const RAIL_LATTICE_PX: f32 = 96.0;
/// The lattice's ink and wash: the mashrabiya's, in the theme's own accent.
const RAIL_ORNAMENT: Ornament = Ornament::motif(Motif::Mashrabiya, 0.0);
/// CSS angle of a gradient running from the top down.
const DOWN_DEG: f32 = 180.0;

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
/// The Look page: its one row of tiles, then the themes, three to a row.
const LOOK_TILES_H: f32 = 84.0;
const THEMES_PER_ROW: usize = 3;
/// Theme rows shown without scrolling.
const THEME_ROWS_FIT: usize = 6;
/// The profile card's shade over the wallpaper (start → end).
const SHADE: (f32, f32) = (0.72, 0.18);
/// The scrim behind the profile text: a near-black in the theme's hue
/// (white text needs a dark ground in light themes too), opaque enough that
/// the softest line keeps WCAG AA (4.5:1) even over a white picture
/// (`the_profile_text_reads_on_any_picture`). It is a soft shadow of the
/// avatar and text, in from the card's start edge: full strength until this
/// far past the text's end, then fading out over a Gaussian of this
/// deviation (GPUI's blur radius), with no edge to see.
const SCRIM_LIGHTNESS: f32 = 0.16;
const SCRIM_CHROMA: f32 = 0.02;
const SCRIM_ALPHA: f32 = 0.9;
const SCRIM_REACH: f32 = 50.0;
const SCRIM_SOFTNESS: f32 = 20.0;
/// WCAG AA for body text.
#[cfg(test)]
const AA: f32 = 4.5;
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
}

impl Page {
    /// The power menu is the header's power button; the rail doesn't repeat it.
    const ALL: [Page; 3] = [Page::Home, Page::Apps, Page::Look];

    fn title(self) -> &'static str {
        match self {
            Page::Home => tk("Home"),
            Page::Apps => tk("Apps & tools"),
            Page::Look => tk("Look"),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Page::Home => "home",
            Page::Apps => "grid",
            Page::Look => "palette",
        }
    }
}

type Run = fn(&mut App);

/// One action on the Apps / Look pages.
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
    Action { icon: "refresh", label: tk("Reload config"), ornament: Ornament::motif(Motif::Pharos, 90.0), hint: "", run: |cx| {
        close(cx);
        let _ = crate::state::reload_config_from_disk(cx);
    } },
    // Last: shown only while the bar's gaps open this menu ([`apps`]).
    Action { icon: "pointer", label: tk("Desktop menu"), ornament: Ornament::motif(Motif::Zellige, -96.0), hint: tk("wallpaper & widgets"), run: crate::bar::gaps::open_desktop_menu },
];

/// The Apps page's tiles. While a right-click in the bar's gaps opens this
/// menu (`bar.right_click_gaps`) the desktop's menu is one tile away: the
/// last one, left out otherwise.
fn apps(cfg: &mandara_config::ShellConfig) -> &'static [Action] {
    if cfg.bar_gaps_open_menu() { APPS } else { &APPS[..APPS.len() - 1] }
}

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
    let host = crate::widgets::system::hostname();
    let home = user.map(|u| u.home).or_else(dirs::home_dir).unwrap_or_default();
    let picture = [home.join(".face"), PathBuf::from(format!("/var/lib/AccountsService/icons/{login}"))].into_iter().find(|p| p.is_file());
    Me { name, login, host, picture }
}

/// "3 h 17 min" since boot.
pub(crate) fn uptime_secs() -> Option<u64> {
    crate::widgets::system::uptime_secs()
}

fn uptime() -> Option<String> {
    let secs = uptime_secs()?;
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    Some(match (d, h) {
        (0, 0) => tf("{m} min", &[("m", &m)]),
        (0, _) => tf("{h} h {m} min", &[("h", &h), ("m", &m)]),
        _ => tf("{d} d {h} h", &[("d", &d), ("h", &h)]),
    })
}

// ── Pages ───────────────────────────────────────────────────────────────

/// The scrim behind the profile card's text, in `palette`'s hue.
fn profile_scrim(palette: &MandaraPalette) -> mandara_theme::Color {
    mandara_theme::Color::oklcha(SCRIM_LIGHTNESS, SCRIM_CHROMA, palette.primary.hue(), SCRIM_ALPHA)
}

fn profile(palette: &MandaraPalette, tc: &TileCx, cx: &App) -> gpui::AnyElement {
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
    let scrim = profile_scrim(palette);
    let lines = div()
        .flex()
        .flex_col()
        .gap(px(LINE_GAP))
        .min_w(px(0.0))
        .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).text_color(white).truncate().child(me.name))
        .child(div().text_size(px(text::BODY_S)).text_color(white.opacity(SOFT_TEXT)).truncate().child(format!("{}@{}", me.login, me.host)))
        .when_some(uptime(), |el, up| el.child(div().text_size(px(text::BODY_S)).text_color(white.opacity(SOFT_TEXT)).child(tf("Up {time}", &[("time", &up)]))))
        .child(div().text_size(px(text::LABEL)).text_color(white.opacity(SOFT_TEXT)).child(format!("Mandara {}", mandara_services::self_update::running_version())));
    card_decoration::plain(div().id("menu-home-profile"), palette, tc)
        .relative()
        .h(px(PROFILE_H))
        .rounded(px(crate::style::card_radius()))
        .overflow_hidden()
        .when_some(wall, |el, w| el.child(img(ImageSource::from(w)).absolute().top_0().left_0().size_full().object_fit(ObjectFit::Cover)))
        .child(div().absolute().top_0().left_0().size_full().bg(mandara_core::oklab_gradient(90.0, mandara_theme::Color::oklcha(0.12, 0.0, 0.0, SHADE.0), mandara_theme::Color::oklcha(0.12, 0.0, 0.0, SHADE.1))))
        .child(
            div().absolute().top_0().left_0().size_full().px(px(tokens::LG)).flex().items_center().child(
                // You, on the scrim: a soft dark shadow of you and your text,
                // running in from the card's start edge (and off its top and
                // bottom, which clip it) and fading out just past the text,
                // so the lines read on a busy or bright picture with no box
                // around them.
                div()
                    .relative()
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .gap(px(tokens::LG))
                    .child(
                        div()
                            .absolute()
                            .top(px(-PROFILE_H))
                            .bottom(px(-PROFILE_H))
                            .left(px(-PROFILE_H))
                            .right_0()
                            .shadow(vec![gpui::BoxShadow {
                                color: scrim.hsla(),
                                offset: gpui::point(px(0.0), px(0.0)),
                                blur_radius: px(SCRIM_SOFTNESS),
                                spread_radius: px(SCRIM_REACH),
                                inset: false,
                            }]),
                    )
                    .child(avatar)
                    .child(lines),
            ),
        )
        .into_any_element()
}

/// What's playing, or nothing.
fn media(palette: &MandaraPalette, tc: &TileCx, cx: &App) -> Option<gpui::AnyElement> {
    let hub = crate::hub::hub(cx).read(cx);
    let m = hub.media.as_ref().filter(|m| !m.title.is_empty())?;
    let playing = m.status == mandara_services::PlaybackStatus::Playing;
    let art = match &m.art_path {
        Some(p) => img(p.clone()).size(px(ART)).rounded(px(tokens::MD)).object_fit(ObjectFit::Cover).into_any_element(),
        None => div().size(px(ART)).rounded(px(tokens::MD)).bg(palette.surface_container_high.hsla()).flex().items_center().justify_center().child(icon("music", ART * 0.4, palette.on_surface_variant)).into_any_element(),
    };
    let transport = |id: &'static str, glyph: &'static str, command: MediaCommand| icon_button(id, glyph, TRANSPORT, palette.on_surface, None, move |_, cx| super::media::send(cx, command.clone()));
    Some(
        home_card("menu-home-media", Ornament::motif(Motif::Rings, 24.0), palette, tc)
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
fn clock(palette: &MandaraPalette, tc: &TileCx, cx: &App) -> gpui::AnyElement {
    let cfg = config(cx).read(cx).snapshot();
    let now = chrono::Local::now();
    let time = now.format(if cfg.general.clock_24h { "%H:%M" } else { "%-I:%M" }).to_string();
    let date = crate::widgets::clock::localized(&now, t("%A, %-d %B"));
    let weather = crate::hub::hub(cx).read(cx).weather.weather.as_ref().map(|w| format!("{} · {}", cfg.units().format_temperature(w.temp), tr(w.condition.label())));
    home_card("menu-home-clock", Ornament::motif(Motif::Shamsa, 0.0), palette, tc)
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

fn home(palette: &MandaraPalette, p: &PanelCx, tc: &TileCx, window: &mut Window, cx: &mut App) -> gpui::AnyElement {
    if tc.patterns {
        let mut glow = tc.glow.borrow_mut();
        glow.follow(palette);
        if glow.step() { window.request_animation_frame(); }
    }
    // The cards on the left share the toggles' height.
    let left = div().flex_1().min_w(px(0.0)).flex().flex_col().gap(px(tokens::SM)).children(media(palette, tc, cx).map(|m| div().flex_1().flex().flex_col().child(m))).child(div().flex_1().flex().flex_col().child(clock(palette, tc, cx)));
    let toggles = div().w(px(TOGGLES_W)).flex_none().child(super::quick_settings::tiles_with(palette, cx, |tile, key, active| {
        card_decoration::toggle(tile, key, home_toggle_ornament(key), palette, &TileCx { compact: true, ..*tc }, active)
    }));
    let volume = home_card("menu-home-volume", Ornament::motif(Motif::Waves, -24.0), palette, &TileCx { compact: true, ..*tc }).child(super::audio::volume_row(palette, cx));
    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap(px(tokens::SM))
        .child(reveal(p, 1, profile(palette, tc, cx)))
        .child(fill(p, 2, div().flex_1().flex().gap(px(tokens::SM)).child(left).child(toggles)))
        .child(reveal(p, 3, volume))
        .into_any_element()
}

/// Home content uses the same configurable frame as the action tiles.
fn home_card(key: &'static str, art: Ornament, palette: &MandaraPalette, tc: &TileCx) -> gpui::Stateful<gpui::Div> {
    card_decoration::decorate(card(palette).id(key).relative(), key, art, palette, tc, false).0
}

/// Give quick controls geometric art without replacing their on/off colors.
fn home_toggle_ornament(key: &str) -> Ornament {
    let motif = match key {
        "qs-wifi" | "qs-bt" => Motif::Rings,
        "qs-theme" | "qs-night-light" => Motif::Shamsa,
        "qs-awake" => Motif::Waves,
        "qs-mute-all" => Motif::Columns,
        "qs-presentation" => Motif::Frames,
        _ => Motif::Mashrabiya,
    };
    Ornament::motif(motif, 0.0)
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
    inks: HashMap<(&'static str, bool, bool), TileInk>,
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
    fn ink(&mut self, key: &'static str, o: Ornament, palette: &MandaraPalette, active: bool) -> TileInk {
        self.inks.entry((key, o.is_word(), active)).or_insert_with(|| TileInk::new(o, palette)).clone()
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
    surface: MenuTileSurface,
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
            let tile = div()
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
            let (tile, glyph) = card_decoration::decorate(tile, a.label, a.ornament.shown(tc.calligraphy), palette, tc, false);
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
fn look(palette: &MandaraPalette, p: &PanelCx, tc: &TileCx, cache: &Rc<RefCell<theme_card::Catalog>>, window: &mut Window, cx: &mut App) -> gpui::AnyElement {
    let current = match &config(cx).read(cx).get().theme.source {
        ThemeSource::Preset { preset } => preset.clone(),
        _ => String::new(),
    };
    let dark = config(cx).read(cx).get().theme.mode.is_dark();
    let themes = theme_card::Catalog::choices(cache, dark, cx);
    let fits = themes.len().div_ceil(THEMES_PER_ROW) <= THEME_ROWS_FIT;
    let mut grid = theme_card::grid(fits);
    for (r, chunk) in themes.chunks(THEMES_PER_ROW).enumerate() {
        let mut row = theme_card::row(fits);
        for (c, choice) in chunk.iter().enumerate() {
            row = row.child(theme_card::card(choice, r * THEMES_PER_ROW + c, &current, palette));
        }
        for _ in chunk.len()..THEMES_PER_ROW {
            row = row.child(div().flex_1());
        }
        grid = grid.child(row);
    }
    // The rows share the card while they fit; more (your own themes) scroll.
    let (scroll, bar) = crate::scrollbar::keyed("menu-themes");
    let list = div().id("menu-themes").flex_1().min_h(px(0.0)).flex().flex_col().when(!fits, |el| el.overflow_y_scroll().track_scroll(&scroll)).child(grid);
    let themes_card = card(palette)
        .flex_1()
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .gap(px(tokens::SM))
        .child(div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(t("Themes")))
        .child(if fits { list.into_any_element() } else { theme_card::scroll_viewport(list, &scroll, &bar, palette).into_any_element() });
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(tokens::SM))
        .child(div().h(px(LOOK_TILES_H)).flex().flex_col().child(tiles(LOOK, palette, p, &TileCx { compact: true, ..*tc }, window)))
        .child(fill(p, 2, themes_card))
        .into_any_element()
}

fn render(page: &Rc<Cell<Page>>, glow: &Rc<RefCell<Glow>>, themes: &Rc<RefCell<theme_card::Catalog>>, fit: &Rc<Cell<gpui::Size<gpui::Pixels>>>, palette: &MandaraPalette, p: &PanelCx, window: &mut Window, cx: &mut App) -> gpui::AnyElement {
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
    let panels = &config(cx).read(cx).get().panels;
    let patterns = panels.menu_patterns;
    let plan = rail_plan(panels.menu_rail_height, compact, patterns, f32::from(fit.get().height) - PAD * 2.0);
    // Hugging: as tall as its buttons (it shrinks with fewer entries),
    // centred in the menu's height. Stretched: as tall as the panel, the
    // buttons one tight group at its top, the rest the rail's own surface.
    let mut rail = div()
        .w(px(if compact { RAIL_COMPACT_W } else { RAIL_W }))
        .when(plan.stretch, |el| el.flex_1())
        .when(!plan.stretch, |el| el.flex_none())
        .flex()
        .flex_col()
        .when(plan.stretch, |el| el.justify_start())
        .when(!plan.stretch, |el| el.justify_center())
        .gap(px(RAIL_GAP))
        .p(px(tokens::XS))
        .rounded(px(crate::style::card_radius()))
        .bg(palette.surface_container.hsla())
        .when(compact, |el| el.items_center());
    if plan.lattice > 0.0 {
        // Under the buttons (drawn first, out of the flow).
        let mut glow = glow.borrow_mut();
        glow.follow(palette);
        let ink = glow.ink("menu-rail", RAIL_ORNAMENT, palette, false);
        rail = crate::style::ClipRounded::clip_rounded(rail.relative()).child(rail_lattice(&ink, palette, if compact { RAIL_COMPACT_W } else { RAIL_W }, plan.lattice));
    }
    // The menu's own pages.
    for (i, pg) in Page::ALL.into_iter().enumerate() {
        let page = page.clone();
        rail = rail.child(rail_button(("menu-page", i), pg.icon(), pg.title(), pg == now).on_click(move |_, window, _| {
            page.set(pg);
            window.refresh();
        }));
    }
    // Shortcuts: each opens its own panel in the menu's place. A rule (and
    // in the full rail a caption) sets them apart from the pages above.
    // Studio and the power menu are the header's buttons, never repeated here.
    rail = rail.child(rule()).when(!compact, |el| {
        el.child(
            div()
                .px(px(tokens::SM + RAIL_CAPTION_PAD))
                .text_size(px(text::LABEL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.on_surface_variant.with_alpha(crate::kit::alpha::SECONDARY_TEXT).hsla())
                .truncate()
                .text_start()
                .child(crate::bidi::paragraph(t("Panels"))),
        )
    });
    for (i, (glyph, id, name)) in SHORTCUTS.into_iter().enumerate() {
        rail = rail.child(rail_button(("menu-shortcut", i), glyph, name, false).on_click(move |_, _, cx| shell(cx).update(cx, |s, cx| s.set_panel(Some(id.into()), cx))));
    }
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
    let tc = TileCx { glow, patterns, calligraphy: panels.menu_calligraphy, compact: false, surface: panels.menu_tile_surface, shadow: panels.menu_tile_shadow() };
    let body = match now {
        Page::Home => home(palette, p, &tc, window, cx),
        Page::Apps => tiles(apps(config(cx).read(cx).get()), palette, p, &tc, window),
        Page::Look => look(palette, p, &tc, themes, window, cx),
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

/// How the rail sits beside the panel.
#[derive(Copy, Clone, Debug, PartialEq)]
struct RailPlan {
    /// As tall as the panel (`panels.menu_rail_height`), else as its
    /// buttons.
    stretch: bool,
    /// The rail's height in a panel whose content is `available` tall.
    height: f32,
    /// Where its buttons start, from the rail's top.
    group_top: f32,
    /// The rail's own surface left under the buttons.
    spare: f32,
    /// How far up from the rail's foot the stretched rail's lattice rises
    /// into that surface (0: none).
    lattice: f32,
}

/// The rail's buttons, rules and caption, top to bottom, with their gaps
/// and padding: what a hugging rail is tall.
fn rail_content_height(compact: bool) -> f32 {
    let buttons = (Page::ALL.len() + SHORTCUTS.len()) as f32;
    let rule = 1.0 + RAIL_GAP * 2.0;
    let caption = if compact { 0.0 } else { RAIL_CAPTION_H };
    let children = buttons + 1.0 + if compact { 0.0 } else { 1.0 };
    buttons * RAIL_BUTTON + rule + caption + RAIL_GAP * (children - 1.0) + tokens::XS * 2.0
}

/// The rail's layout beside a panel whose content is `available` px tall.
/// The buttons keep their one gap ([`RAIL_GAP`]) in every mode: a
/// stretched rail never spreads them, it only grows its own surface under
/// them. A panel no taller than the buttons leaves nothing to stretch.
fn rail_plan(mode: MenuRailHeight, compact: bool, patterns: bool, available: f32) -> RailPlan {
    let content = rail_content_height(compact);
    let room = available.max(content);
    match mode {
        MenuRailHeight::Hug => RailPlan { stretch: false, height: content, group_top: (room - content) / 2.0, spare: 0.0, lattice: 0.0 },
        MenuRailHeight::Stretch => {
            let spare = room - content;
            let lattice = if patterns && spare >= RAIL_LATTICE_MIN { spare.min(RAIL_LATTICE_MAX) } else { 0.0 };
            RailPlan { stretch: true, height: room, group_top: 0.0, spare, lattice }
        }
    }
}

/// The stretched rail's surface under its buttons, `h` px up from the foot
/// of a rail `w` wide: the tiles' accent wash deepening toward the foot,
/// and the mashrabiya lattice in the tiles' ink, dissolving into the
/// rail's plain surface on its way up to the buttons. Centred, so it reads
/// the same in either direction. At most a few copies of one cached SVG.
fn rail_lattice(ink: &TileInk, palette: &MandaraPalette, w: f32, h: f32) -> gpui::AnyElement {
    let (cols, rows) = ((w / RAIL_LATTICE_PX).ceil() as usize, (h / RAIL_LATTICE_PX).ceil() as usize);
    let left = (w - cols as f32 * RAIL_LATTICE_PX) / 2.0;
    // The tiles' ink under the pointer: still capped for the labels'
    // contrast, and nothing but the lowest buttons ever sits on it.
    let colour = ink.shade.with_alpha(ink.shade_alpha.1).hsla();
    let rest = palette.surface_container;
    // Clipped: the top row of copies reaches past `h`, under the buttons.
    let mut el = crate::style::ClipRounded::clip_rounded(div().absolute().left_0().right_0().bottom_0().h(px(h))).bg(mandara_core::oklab_gradient(DOWN_DEG, rest, ink.wash_rest));
    for row in 0..rows {
        for col in 0..cols {
            el = el.child(
                gpui::svg().path(ornament::RAIL_LATTICE).absolute().left(px(left + col as f32 * RAIL_LATTICE_PX)).bottom(px(row as f32 * RAIL_LATTICE_PX)).size(px(RAIL_LATTICE_PX)).text_color(colour),
            );
        }
    }
    // The rail's own surface over the lattice's top, clearing toward the foot.
    el.child(div().absolute().inset_0().bg(mandara_core::oklab_gradient(DOWN_DEG, rest, rest.with_alpha(0.0)))).into_any_element()
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Main menu"), tk("Right-click empty bar space"), None), |window, cx, panel_cx| {
        let p = panel_cx.clone();
        let page = Rc::new(Cell::new(Page::Home));
        let glow = Rc::new(RefCell::new(Glow::default()));
        let fit = Rc::new(Cell::new(size(px(WIDTH), px(HEIGHT))));
        let measured = fit.clone();
        let themes = Rc::new(RefCell::new(theme_card::Catalog::default()));
        let clear = themes.clone();
        let panel = fn_view_observing(None, move |cx| {
            cx.observe(&shell(cx), move |_, state, cx| {
                if state.read(cx).panel.as_deref() != Some(ID) { clear.borrow_mut().clear(); }
            }).detach();
        }, move |palette, window, cx| render(&page, &glow, &themes, &measured, palette, &p, window, cx), window, cx);
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

    /// Every entry of the old list is still on a page. Lock, log out,
    /// sleep, restart and shut down are the header's power menu.
    #[test]
    fn every_old_entry_is_still_here() {
        let labels: Vec<&str> = APPS.iter().chain(LOOK).map(|a| a.label).collect();
        for old in ["Launcher", "Wallpapers", "Sidebar", "Mandara Studio", "Screenshot", "Clipboard history", "Toggle light / dark", "Next theme", "Reload config"] {
            assert!(labels.contains(&old), "{old} is missing");
        }
    }

    /// The rail holds pages and panel shortcuts only: nothing it shows
    /// repeats a header button (Studio's sliders, the power menu).
    #[test]
    fn the_rail_repeats_no_header_button() {
        const HEADER: [&str; 3] = ["sliders", "power", "close"];
        for page in Page::ALL {
            assert!(!HEADER.contains(&page.icon()), "{:?} repeats a header button", page);
        }
        for (glyph, _, name) in SHORTCUTS {
            assert!(!HEADER.contains(&glyph), "{name} repeats a header button");
        }
    }

    /// The rail hugs its buttons unless stretched. Stretched beside a tall
    /// panel it is the panel's height with its buttons one tight group at
    /// the top (never spread out) and the rest its own surface, wearing the
    /// ornament once that is tall enough; beside a short panel there is
    /// nothing to stretch and it is drawn just like the hugging rail.
    #[test]
    fn the_rail_stretches_without_spreading_its_buttons() {
        assert_eq!(mandara_config::surfaces::PanelsConfig::default().menu_rail_height, MenuRailHeight::Hug);
        for compact in [true, false] {
            let content = rail_content_height(compact);
            assert!(content > RAIL_BUTTON * 10.0, "ten buttons, a rule and their gaps");
            for patterns in [true, false] {
                // A tall panel (many quick toggles, large text) and the
                // shipped one.
                for tall in [content + 400.0, content + 60.0] {
                    let hug = rail_plan(MenuRailHeight::Hug, compact, patterns, tall);
                    assert_eq!((hug.stretch, hug.height, hug.spare, hug.lattice), (false, content, 0.0, 0.0));
                    assert_eq!(hug.group_top, (tall - content) / 2.0, "centred beside the panel");
                    let stretched = rail_plan(MenuRailHeight::Stretch, compact, patterns, tall);
                    assert!(stretched.stretch);
                    assert_eq!(stretched.height, tall, "as tall as the panel");
                    assert_eq!(stretched.group_top, 0.0, "the buttons at the top");
                    assert_eq!(stretched.spare, tall - content, "the buttons keep their own height and gaps");
                    assert_eq!(stretched.lattice > 0.0, patterns, "the lattice follows the tile patterns");
                    assert!(stretched.lattice <= stretched.spare.min(RAIL_LATTICE_MAX), "under the buttons, never past its cap");
                    // A handful of copies of one cached drawing at most.
                    let width = if compact { RAIL_COMPACT_W } else { RAIL_W };
                    let copies = (width / RAIL_LATTICE_PX).ceil() * (stretched.lattice / RAIL_LATTICE_PX).ceil();
                    assert!(copies <= 6.0, "{copies} copies");
                }
                // A short panel: nothing to stretch, no sliver of pattern.
                for short in [content - 120.0, content, content + RAIL_LATTICE_MIN - 1.0] {
                    let stretched = rail_plan(MenuRailHeight::Stretch, compact, patterns, short);
                    let hug = rail_plan(MenuRailHeight::Hug, compact, patterns, short);
                    assert_eq!(stretched.height, short.max(content), "never shorter than its buttons");
                    assert_eq!(stretched.lattice, 0.0, "{short}: too little surface for the lattice");
                    if short <= content {
                        assert_eq!((stretched.height, stretched.spare), (hug.height, hug.spare), "{short}: just the hugging rail");
                        assert_eq!(hug.group_top, 0.0);
                    }
                }
            }
        }
    }

    /// The desktop's menu is a tile away while the bar's gaps open this
    /// menu; otherwise the Apps page is as it always was.
    #[test]
    fn the_desktop_menu_tile_shows_only_while_the_gaps_open_the_menu() {
        let last = APPS.last().unwrap();
        assert_eq!(last.label, "Desktop menu");
        let mut cfg = mandara_config::ShellConfig::default();
        assert_eq!(apps(&cfg).len(), APPS.len() - 1, "off by default");
        cfg.bar.right_click_gaps = true;
        assert_eq!(apps(&cfg).len(), APPS.len() - 1, "the full-width edge frame has no gaps");
        cfg.bar.layout = mandara_config::BarLayout::FloatingPills;
        assert_eq!(apps(&cfg).len(), APPS.len());
        assert!(apps(&cfg).len().is_multiple_of(TILES_PER_ROW), "three full rows");
    }

    /// Φ(x), the standard normal CDF, by Simpson's rule over [-8, x].
    fn gaussian_cdf(x: f32) -> f32 {
        let (a, n) = (-8.0_f64, 2000);
        let h = (f64::from(x) - a) / f64::from(n);
        let f = |t: f64| (-t * t / 2.0).exp() / (2.0 * std::f64::consts::PI).sqrt();
        let sum: f64 = (0..=n).map(|i| {
            let w = if i == 0 || i == n { 1.0 } else if i % 2 == 1 { 4.0 } else { 2.0 };
            w * f(a + h * f64::from(i))
        }).sum();
        (sum * h / 3.0) as f32
    }

    /// The profile's softest line (white at `SOFT_TEXT`) keeps WCAG AA on
    /// the scrim over any picture, in every shipped theme, dark and light,
    /// whether the compositor blends in linear light or in sRGB.
    #[test]
    fn the_profile_text_reads_on_any_picture() {
        use mandara_theme::Color;
        let white = Color::oklch(1.0, 0.0, 0.0);
        // Gamma-space compositing, the other way a renderer may blend.
        let over_srgb = |top: Color, under: Color| {
            let (t, u) = (top.to_srgba(), under.to_srgba());
            let a = t.alpha;
            let ch = |x: f32, y: f32| ((x * a + y * (1.0 - a)) * 255.0).round().clamp(0.0, 255.0) as u8;
            Color::from_rgba8(ch(t.red, u.red), ch(t.green, u.green), ch(t.blue, u.blue), 255)
        };
        // White is the hardest picture for white text; the others guard
        // against a hue the scrim's tint might lift.
        let pictures = [white, Color::oklch(0.5, 0.0, 0.0), Color::oklch(0.9, 0.18, 95.0), Color::oklch(0.7, 0.2, 145.0), Color::oklch(0.65, 0.25, 25.0), Color::oklch(0.0, 0.0, 0.0)];
        let mut checked = 0;
        for theme in mandara_theme::catalog::builtin() {
            for dark in [true, false] {
                let palette = theme.palette(dark);
                let id = &theme.id;
                // The text's end is `SCRIM_REACH` inside the blurred
                // shadow's end edge; its other edges lie off the card.
                let core = gaussian_cdf(SCRIM_REACH / SCRIM_SOFTNESS);
                let scrim = profile_scrim(&palette);
                let scrim = scrim.with_alpha(scrim.alpha() * core);
                for picture in pictures {
                    let linear = |t: Color, u: Color| t.over(u);
                    let blends: [&dyn Fn(Color, Color) -> Color; 2] = [&linear, &over_srgb];
                    for over in blends {
                        let ground = over(scrim, picture);
                        for (alpha, line) in [(1.0, "name"), (SOFT_TEXT, "details")] {
                            let ink = over(white.with_alpha(alpha), ground);
                            let ratio = ink.contrast_ratio(ground);
                            assert!(ratio >= AA, "{id} (dark {dark}): {line} at {ratio:.2}:1 over {picture:?}");
                        }
                    }
                }
                checked += 1;
            }
        }
        assert!(checked > 0, "no themes checked");
    }
}
