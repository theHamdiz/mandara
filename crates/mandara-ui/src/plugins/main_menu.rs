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
//! and the cards on it are solid.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{App, FontWeight, ImageSource, InteractiveElement, IntoElement, ObjectFit, ParentElement, StatefulInteractiveElement, Styled, StyledImage as _, Window, div, img, prelude::FluentBuilder as _, px, size};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk, tr};
use mandara_plugins::{CommandSpec, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry};
use mandara_services::MediaCommand;
use mandara_config::power::{SessionEntry, SessionEntryKind};
use mandara_theme::{MandaraPalette, ThemeSource, catalog};
use serde_json::json;

use super::fn_view;
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::{card, icon_button, reveal, text, tokens};
use crate::state::{config, shell, update_config};

pub const ID: &str = "main-menu";

const WIDTH: f32 = 640.0;
/// Before the first measurement (the menu is as tall as its page).
const HEIGHT: f32 = 468.0;
/// Size changes smaller than this don't resize the panel.
const SIZE_EPSILON: f32 = 0.5;
const PAD: f32 = 12.0;
/// The rail: pages, then shortcuts, then settings.
const RAIL_W: f32 = 48.0;
const RAIL_BUTTON: f32 = 32.0;
const RAIL_ICON: f32 = 16.0;
const RAIL_GAP: f32 = 3.0;
/// The hairline between the rail's groups.
const RAIL_RULE_W: f32 = 20.0;
const RAIL_RULE_ALPHA: f32 = 0.3;

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
}

fn close(cx: &mut App) {
    shell(cx).update(cx, |s, cx| s.set_panel(None, cx));
}

const APPS: &[Action] = &[
    Action { icon: "grid", label: tk("Launcher"), hint: "Super+Space", run: |cx| {
        close(cx);
        shell(cx).update(cx, |s, cx| s.set_launcher(true, cx));
    } },
    Action { icon: "image", label: tk("Wallpapers"), hint: tk("picker"), run: |cx| {
        close(cx);
        shell(cx).update(cx, |s, cx| s.set_wallpaper_picker(true, cx));
    } },
    Action { icon: "sidebar", label: tk("Sidebar"), hint: tk("toggle"), run: |cx| {
        close(cx);
        let s = shell(cx);
        let open = !s.read(cx).sidebar_visible();
        s.update(cx, |s, cx| s.set_sidebar(open, cx));
    } },
    Action { icon: "monitor", label: tk("Mandara Studio"), hint: tk("settings"), run: |cx| {
        close(cx);
        crate::state::open_studio(cx);
    } },
    Action { icon: "crop", label: tk("Screenshot"), hint: "Super+Shift+S", run: |cx| {
        close(cx);
        crate::snip::open(crate::snip::SnipMode::Region, cx);
    } },
    Action { icon: "clipboard", label: tk("Clipboard history"), hint: "Super+Shift+V", run: |cx| {
        shell(cx).update(cx, |s, cx| s.set_panel(Some("clipboard".into()), cx));
    } },
    Action { icon: "pencil", label: tk("Edit desktop"), hint: tk("widgets"), run: |cx| {
        close(cx);
        crate::desktop_edit::enter(cx);
    } },
];

const LOOK: &[Action] = &[
    Action { icon: "moon", label: tk("Toggle light / dark"), hint: "Super+Shift+T", run: |cx| {
        let mode = config(cx).read(cx).get().theme.mode.toggled();
        update_config(cx, |c| c.theme.mode = mode);
    } },
    Action { icon: "sun", label: tk("Next theme"), hint: "theme next", run: |cx| {
        let next = match &config(cx).read(cx).get().theme.source {
            ThemeSource::Preset { preset } => catalog::next_id(preset),
            _ => catalog::DEFAULT_THEME.to_string(),
        };
        apply_theme(next, cx);
    } },
    Action { icon: "image", label: tk("Wallpapers"), hint: tk("picker"), run: |cx| {
        close(cx);
        shell(cx).update(cx, |s, cx| s.set_wallpaper_picker(true, cx));
    } },
];

const SESSION: &[Action] = &[
    Action { icon: "lock", label: tk("Lock"), hint: "", run: |cx| session(SessionEntryKind::Lock, cx) },
    Action { icon: "logout", label: tk("Log out"), hint: "", run: |cx| session(SessionEntryKind::Logout, cx) },
    Action { icon: "moon", label: tk("Sleep"), hint: "", run: |cx| session(SessionEntryKind::Suspend, cx) },
    Action { icon: "restart", label: tk("Restart"), hint: "", run: |cx| session(SessionEntryKind::Reboot, cx) },
    Action { icon: "power", label: tk("Shut down"), hint: "", run: |cx| session(SessionEntryKind::Shutdown, cx) },
    Action { icon: "refresh", label: tk("Reload config"), hint: "", run: |cx| {
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
struct Me {
    name: String,
    login: String,
    host: String,
    picture: Option<PathBuf>,
}

fn me() -> Me {
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

/// A reveal that keeps its row's full height (the rail).
fn fill_row(p: &PanelCx, index: usize, el: impl IntoElement) -> gpui::AnyElement {
    let r = p.reveal(index);
    div().relative().top(px(r.offset)).opacity(r.opacity).flex().flex_col().child(el).into_any_element()
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

/// A page of action tiles, three to a row (a short last row spreads
/// across); the rows share the page's height.
fn tiles(actions: &[Action], palette: &MandaraPalette, p: &PanelCx) -> gpui::AnyElement {
    let mut grid = div().flex_1().flex().flex_col().gap(px(tokens::SM));
    for (r, chunk) in actions.chunks(TILES_PER_ROW).enumerate() {
        let mut row = div().flex_1().min_h(px(0.0)).flex().gap(px(tokens::SM));
        for (c, a) in chunk.iter().enumerate() {
            let run = a.run;
            row = row.child(
                div()
                    .id(("menu-tile", r * TILES_PER_ROW + c))
                    .flex_1()
                    .min_w(px(0.0))
                    .p(px(tokens::MD))
                    .rounded(px(crate::style::card_radius()))
                    .bg(palette.surface_container.hsla())
                    .hover(|s| s.bg(palette.surface_container_high.hsla()))
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .justify_between()
                    .child(icon(a.icon, TILE_ICON, palette.primary))
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
fn look(palette: &MandaraPalette, p: &PanelCx, cx: &App) -> gpui::AnyElement {
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
        .child(div().h(px(LOOK_TILES_H)).flex().flex_col().child(tiles(LOOK, palette, p)))
        .child(fill(p, 2, themes_card))
        .into_any_element()
}

fn render(page: &Rc<Cell<Page>>, fit: &Rc<Cell<gpui::Size<gpui::Pixels>>>, palette: &MandaraPalette, p: &PanelCx, window: &mut Window, cx: &mut App) -> gpui::AnyElement {
    let now = page.get();
    let rail_button = |id: (&'static str, usize), glyph: &'static str, on: bool| {
        div()
            .id(id)
            .size(px(RAIL_BUTTON))
            .rounded(px(crate::style::radius(tokens::MD)))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .when(on, |el| el.bg(palette.primary.with_alpha(0.18).hsla()))
            .hover(|s| s.bg(palette.surface_container_high.hsla()))
            .child(icon(glyph, RAIL_ICON, if on { palette.primary } else { palette.on_surface_variant }))
    };
    let rule = || div().w(px(RAIL_RULE_W)).h(px(1.0)).my(px(RAIL_GAP)).bg(palette.outline.with_alpha(RAIL_RULE_ALPHA).hsla());
    // As tall as the menu, its buttons in the middle.
    let mut rail = div().w(px(RAIL_W)).flex_none().flex().flex_col().items_center().justify_center().gap(px(RAIL_GAP)).py(px(tokens::SM)).rounded(px(crate::style::card_radius())).bg(palette.surface_container.hsla());
    // The menu's own pages.
    for (i, pg) in Page::ALL.into_iter().enumerate() {
        let page = page.clone();
        rail = rail.child(rail_button(("menu-page", i), pg.icon(), pg == now).on_click(move |_, window, _| {
            page.set(pg);
            window.refresh();
        }));
    }
    // Shortcuts: each opens its own panel in the menu's place.
    rail = rail.child(rule());
    for (i, (glyph, id, _)) in SHORTCUTS.into_iter().enumerate() {
        rail = rail.child(rail_button(("menu-shortcut", i), glyph, false).on_click(move |_, _, cx| shell(cx).update(cx, |s, cx| s.set_panel(Some(id.into()), cx))));
    }
    // Settings.
    rail = rail.child(rule()).child(rail_button(("menu-settings", 0), "sliders", false).on_click(|_, _, cx| {
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
    let body = match now {
        Page::Home => home(palette, p, cx),
        Page::Apps => tiles(APPS, palette, p),
        Page::Look => look(palette, p, cx),
        Page::Session => tiles(SESSION, palette, p),
    };
    let _ = window;
    // Home sets the menu's height (it's the fullest page); the other pages
    // keep it and spread over it, so switching pages doesn't jump.
    let home_page = now == Page::Home;
    let fit = fit.clone();
    let inner = f32::from(fit.get().height) - PAD * 2.0;
    let probe = gpui::canvas(
        move |b, _, _| {
            let h = f32::from(b.size.height) + PAD * 2.0;
            if home_page && (f32::from(fit.get().height) - h).abs() > SIZE_EPSILON {
                fit.set(size(px(WIDTH), px(h)));
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
        let fit = Rc::new(Cell::new(size(px(WIDTH), px(HEIGHT))));
        let measured = fit.clone();
        let panel = fn_view(None, move |palette, window, cx| render(&page, &measured, palette, &p, window, cx), window, cx);
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
