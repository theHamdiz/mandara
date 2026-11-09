//! The radio panel and sidebar card.
//!
//! Panel, top to bottom: the now-playing card (the station's art with a
//! glow, a live badge, the station, what plays now, format and buffer
//! health, and a level meter), the transport (previous, play, next,
//! favourite, sleep timer), the volume slider (in the design picked under
//! Themes → Components), then three tabs: your favourites (reorderable),
//! every station by shelf with search (and the directory when it's on),
//! and the station editor. The model is made only once the panel is
//! actually shown; frames are scheduled only while it is visible, and
//! only as often as what moves needs.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext as _, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Pixels, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder as _, px, size,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{Lang, t, tf, tk, tr};
use mandara_plugins::PanelCx;
use mandara_services::radio::{Category, Status, Station, directory};
use mandara_theme::{Color, MandaraPalette};

use crate::plugins::extras::{CONTROL_H, Flash, Segment, caption, search_box, segmented, toast};
use crate::plugins::panel_fx::{
    ANGLE_DIAGONAL_DEG, ANGLE_DOWN_DEG, Cached, PanelShape, WASH_SOFT, alpha, big_slider, chip, empty_state, glass_card, glow, partner, pill_button, readable, redraw_in, wash,
};
use super::{Settings, Shared, Tab, art, existing, model, settings};
use crate::bidi::Bidi as _;
use crate::fonts::tabular_nums;
use crate::icons::{icon, ring};
use crate::kit::{card, icon_button, reveal, text, tokens};
use crate::text_field::TextField;

const PANEL_W: f32 = 420.0;
const PAD: f32 = 16.0;
const GAP: f32 = 12.0;
// The now-playing card.
const HERO_H: f32 = 168.0;
const HERO_PAD: f32 = 14.0;
const ART_PX: f32 = 104.0;
const ART_RADIUS_PX: f32 = 22.0;
const ART_GLOW: (f32, f32) = (0.45, 26.0);
const HERO_GAP_PX: f32 = 14.0;
const BADGE_H_PX: f32 = 20.0;
const BADGE_PAD_X_PX: f32 = 8.0;
const BADGE_DOT_PX: f32 = 6.0;
const BADGE_TEXT_PX: f32 = 10.0;
const STATION_TEXT_PX: f32 = 17.0;
const TITLE_LINES: usize = 2;
const LINE_GAP_PX: f32 = 4.0;
/// The card's tint from the station colours `(dark, light)`.
const HERO_TINT: (f32, f32) = (0.22, 0.14);
// The level meter.
const METER_H_PX: f32 = 22.0;
const METER_BARS: usize = 28;
const METER_BAR_W_PX: f32 = 3.0;
const METER_MIN_PX: f32 = 2.0;
/// The meter's sensitivity: quiet music (RMS 0.05) fills about half.
const METER_GAIN: f32 = 6.0;
const METER_RATE: f32 = 7.0;
const METER_RATE_STEP: f32 = 0.53;
const METER_PHASE_STEP: f32 = 1.31;
const METER_IDLE_ALPHA: f32 = 0.18;
// Transport and volume.
const TRANSPORT_H: f32 = 56.0;
const PLAY_PX: f32 = 54.0;
const PLAY_ICON_PX: f32 = 20.0;
const PLAY_GLOW: (f32, f32, f32) = (0.3, 12.0, 26.0);
const SKIP_PX: f32 = 40.0;
const SIDE_BUTTON_PX: f32 = 36.0;
const VOLUME_H: f32 = 40.0;
const MUTE_PX: f32 = 36.0;
// The tabs.
const TABS_H: f32 = CONTROL_H;
const LIST_H: f32 = 262.0;
const ROW_H_PX: f32 = 46.0;
/// Gap between list rows.
const LIST_GAP_PX: f32 = 2.0;
const ROW_ART_PX: f32 = 32.0;
const ROW_ART_RADIUS_PX: f32 = 9.0;
const ROW_PAD_X_PX: f32 = 8.0;
const ROW_ACTION_PX: f32 = 30.0;
const TILE_ART_PX: f32 = 64.0;
const TILE_ART_RADIUS_PX: f32 = 16.0;
const TILE_W_PX: f32 = 116.0;
const TILE_H_PX: f32 = 104.0;
const SHELF_CHIP_H_PX: f32 = 28.0;
const SHELF_CHIP_PAD_X_PX: f32 = 11.0;
const SHELF_GAP_PX: f32 = 6.0;
const FIELD_H_PX: f32 = 38.0;
const FIELD_PAD_X_PX: f32 = 12.0;
const FIELD_RADIUS_PX: f32 = 12.0;
const ERROR_ICON_PX: f32 = 13.0;
// Cadence of redraws while the panel is open.
const METER_FRAME: Duration = Duration::from_millis(50);
const STATUS_FRAME: Duration = Duration::from_millis(500);
const SLEEP_FRAME: Duration = Duration::from_secs(1);
const SECS_PER_MIN: u64 = 60;

/// Total panel height.
fn panel_height() -> f32 {
    PAD * 2.0 + HERO_H + GAP + TRANSPORT_H + GAP + VOLUME_H + GAP + TABS_H + GAP + LIST_H
}

struct Fields {
    search: Entity<TextField>,
    name: Entity<TextField>,
    url: Entity<TextField>,
}

/// Labels formatted only when what they show changes.
#[derive(Default)]
struct Labels {
    status: Cached<(Lang, Status, u32, bool)>,
    details: Cached<(Lang, &'static str, Option<u32>, u32)>,
    sleep: Cached<(Lang, u64)>,
}

pub(crate) struct View {
    p: PanelCx,
    flash: Flash,
    fields: RefCell<Option<Fields>>,
    labels: RefCell<Labels>,
    /// When the panel last opened (the meter's clock).
    opened: Cell<Option<Instant>>,
}

impl View {
    pub(crate) fn new(p: PanelCx) -> (Self, Rc<Cell<gpui::Size<Pixels>>>) {
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(panel_height()))));
        (Self { p, flash: Flash::default(), fields: RefCell::default(), labels: RefCell::default(), opened: Cell::new(None) }, size_cell)
    }

    fn fields(&self, shared: &Shared, cx: &mut App) -> (Entity<TextField>, Entity<TextField>, Entity<TextField>) {
        let mut slot = self.fields.borrow_mut();
        let f = slot.get_or_insert_with(|| {
            let views = shared.borrow().views.clone();
            let v2 = views.clone();
            let search = cx.new(|cx| TextField::new(String::new(), t("Search stations, shelves, countries…"), true, Rc::new(move |_, _, cx| views.notify(cx)), cx));
            let name = cx.new(|cx| TextField::new(String::new(), t("Station name"), true, Rc::new(|_, _, _| {}), cx));
            let url = cx.new(|cx| TextField::new(String::new(), t("https://… stream, .pls or .m3u address"), true, Rc::new(move |_, _, cx| v2.notify(cx)), cx));
            Fields { search, name, url }
        });
        (f.search.clone(), f.name.clone(), f.url.clone())
    }

    pub(crate) fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let visible = self.p.progress() > 0.0;
        // Nothing is made for a panel that isn't shown.
        let Some(shared) = (if visible { Some(model(cx)) } else { existing(cx) }) else {
            self.opened.set(None);
            return div().into_any_element();
        };
        if !visible {
            self.opened.set(None);
        } else if self.opened.get().is_none() {
            self.opened.set(Some(Instant::now()));
        }
        let views = shared.borrow().views.clone();
        views.track(window);
        let s = settings(cx);
        let reduced = crate::state::config(cx).read(cx).get().motion.reduced(mandara_core::spring::system_reduced_motion());
        let player = shared.borrow().player.clone();
        let status = player.status();
        let buffering = player.buffering();
        if visible {
            if status == Status::Playing && s.meter && !reduced {
                redraw_in(METER_FRAME, window, cx);
            } else if status.active() {
                redraw_in(STATUS_FRAME, window, cx);
            } else if player.sleep_remaining().is_some() {
                redraw_in(SLEEP_FRAME, window, cx);
            }
        }
        let (search, name_field, url_field) = self.fields(&shared, cx);
        let radius = PanelShape::of(cx).inner(PAD);
        let p = &self.p;
        let hero = self.hero(&shared, &s, status, buffering, reduced, radius, palette, window);
        let transport = self.transport(&shared, &s, status, palette);
        let volume = volume_row(&shared, palette);
        let tab = shared.borrow().tab;
        let tabs = tabs(&shared, tab, palette);
        let list: AnyElement = match tab {
            Tab::Favourites => favourites(&shared, &s, palette, &self.flash),
            Tab::Stations => stations(&shared, &s, &search, palette, &self.flash, cx),
            Tab::Add => editor(&shared, &name_field, &url_field, palette, cx),
        };
        div()
            .relative()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(reveal(p, 0, hero))
            .child(reveal(p, 1, transport))
            .child(reveal(p, 2, volume))
            .child(reveal(p, 3, tabs))
            .child(reveal(p, 4, div().h(px(LIST_H)).child(list)))
            .when_some(self.flash.current(window, cx), |el, (ok, msg)| el.child(toast(ok, msg, PAD, palette)))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn hero(&self, shared: &Shared, s: &Settings, status: Status, buffering: bool, reduced: bool, radius: f32, palette: &MandaraPalette, window: &mut Window) -> AnyElement {
        let _ = window;
        let m = shared.borrow();
        let player = &m.player;
        let (tune, title, info, attempt, error) = player.with(|snap| (snap.tune.as_ref().map(|t| (t.id.clone(), t.name.clone())), snap.title.clone(), snap.info.clone(), snap.attempt, snap.error.clone()));
        drop(m);
        let category = tune.as_ref().map_or(Category::Other, |(id, _)| station_category_of(shared, id));
        let (a, b) = match &tune {
            Some((_, name)) => art::colors(name, category, s.art, palette),
            None => (palette.primary, partner(palette.primary)),
        };
        let tint = alpha(HERO_TINT, palette);
        let art_el: AnyElement = match &tune {
            Some((id, name)) => art::tile(id, name, category, s.art, ART_PX, crate::style::radius(ART_RADIUS_PX), palette)
                .when(status.active(), |el| el.shadow(vec![glow(a.with_alpha(ART_GLOW.0), ART_GLOW.1, 0.0, 6.0)]))
                .into_any_element(),
            None => art::placeholder(ART_PX, crate::style::radius(ART_RADIUS_PX), palette),
        };
        let label = self.labels.borrow_mut().status.get((mandara_i18n::language(), status, attempt, buffering), |(_, status, attempt, buffering)| status_text(*status, *attempt, *buffering));
        let live = status == Status::Playing && !buffering;
        let badge_color = if live { readable(palette.urgent_coral, palette) } else if status.active() { readable(palette.warning_amber, palette) } else { palette.on_surface_variant };
        let badge = div()
            .h(px(BADGE_H_PX))
            .px(px(BADGE_PAD_X_PX))
            .flex_none()
            .rounded_full()
            .bg(badge_color.with_alpha(alpha(super::super::panel_fx::TINT_FILL, palette)).hsla())
            .row()
            .items_center()
            .gap(px(tokens::XS + 1.0))
            .child(div().size(px(BADGE_DOT_PX)).rounded_full().bg(badge_color.hsla()))
            .child(div().text_size(px(BADGE_TEXT_PX)).font_weight(FontWeight::BOLD).text_color(badge_color.hsla()).child(label));
        let station_name: SharedString = tune.as_ref().map_or_else(|| t("Radio").into(), |(_, n)| n.clone().into());
        let now: SharedString = match (&title, &tune) {
            (Some(t_), _) => t_.clone().into(),
            (None, Some(_)) if status.active() => t("Live").into(),
            (None, Some(_)) => t("Press play to listen").into(),
            (None, None) => t("Pick a station below, or press play").into(),
        };
        let details = (s.details && tune.is_some() && info.rate > 0).then(|| {
            self.labels.borrow_mut().details.get((mandara_i18n::language(), info.codec.label(), info.kbps, info.rate), |(_, codec, kbps, rate)| {
                let mut parts: Vec<String> = Vec::new();
                if !codec.is_empty() {
                    parts.push((*codec).to_string());
                }
                if let Some(k) = kbps {
                    parts.push(tf("{n} kbit/s", &[("n", k)]));
                }
                parts.push(tf("{n} kHz", &[("n", &format!("{:.1}", f64::from(*rate) / 1000.0))]));
                parts.join(" · ")
            })
        });
        let fill = player_fill(shared);
        let accent = readable(a, palette);
        let buffer = (s.details && status.active()).then(|| {
            let f = fill.unwrap_or(0.0);
            div()
                .row()
                .items_center()
                .gap(px(tokens::XS))
                .child(ring(f, BADGE_H_PX - 4.0, 2.0, accent, palette.on_surface.with_alpha(0.12)))
                .child(div().font_features(tabular_nums()).text_size(px(text::CAPTION)).text_color(palette.on_surface_variant.hsla()).child(super::super::panel_fx::percent_text(f)))
        });
        let shelf = tune.as_ref().map(|_| chip(category.icon(), tr(category.label()), accent, palette));
        let info_col = div()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .gap(px(LINE_GAP_PX))
            .child(div().row().items_center().gap(px(tokens::SM)).child(badge).when_some(buffer, |el, b| el.child(b)))
            .child(div().text_size(px(STATION_TEXT_PX)).font_weight(FontWeight::BOLD).text_color(accent.hsla()).truncate().child(station_name))
            .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).line_clamp(TITLE_LINES).child(crate::bidi::paragraph(now)))
            .child(div().row().items_center().gap(px(tokens::SM)).when_some(shelf, |el, c| el.child(c)).when_some(details, |el, d| el.child(div().text_size(px(text::CAPTION)).text_color(palette.on_surface_variant.hsla()).truncate().child(d))));
        let error_line = error.filter(|_| !status.active() || attempt > 0).map(|e| {
            let coral = readable(palette.urgent_coral, palette);
            div().row().items_center().gap(px(tokens::XS)).child(icon("alert", ERROR_ICON_PX, coral)).child(div().text_size(px(text::CAPTION)).text_color(coral.hsla()).truncate().child(e))
        });
        let meter = (s.meter && error_line.is_none()).then(|| self.meter(&player_level(shared), status, reduced, a, b, palette));
        glass_card(palette)
            .h(px(HERO_H))
            .rounded(px(radius))
            .bg(oklab_gradient(ANGLE_DOWN_DEG, palette.surface_container.mix(a, tint), palette.surface_container.mix(b, tint * 0.5)))
            .p(px(HERO_PAD))
            .flex()
            .flex_col()
            .justify_between()
            .child(div().row().gap(px(HERO_GAP_PX)).items_center().child(art_el).child(info_col))
            .when_some(meter, |el, m| el.child(m))
            .when_some(error_line, |el, e| el.child(e))
            .into_any_element()
    }

    fn meter(&self, level: &f32, status: Status, reduced: bool, a: Color, b: Color, palette: &MandaraPalette) -> AnyElement {
        let playing = status == Status::Playing;
        let t = self.opened.get().map_or(0.0, |o| o.elapsed().as_secs_f32());
        let lvl = (level * METER_GAIN).sqrt().clamp(0.0, 1.0);
        let bars = (0..METER_BARS).map(move |i| {
            let k = i as f32;
            let h = if playing && !reduced {
                let wave = 0.5 + 0.5 * (t * (METER_RATE + k * METER_RATE_STEP) + k * METER_PHASE_STEP).sin();
                METER_MIN_PX + (METER_H_PX - METER_MIN_PX) * lvl * (0.35 + 0.65 * wave)
            } else if playing {
                METER_MIN_PX + (METER_H_PX - METER_MIN_PX) * lvl * 0.6
            } else {
                METER_MIN_PX
            };
            let c = a.mix(b, k / METER_BARS as f32);
            div().w(px(METER_BAR_W_PX)).h(px(h)).rounded_full().bg(if playing { c.hsla() } else { palette.on_surface.with_alpha(METER_IDLE_ALPHA).hsla() })
        });
        div().h(px(METER_H_PX)).ltr_row().items_end().justify_between().children(bars).into_any_element()
    }

    fn transport(&self, shared: &Shared, s: &Settings, status: Status, palette: &MandaraPalette) -> AnyElement {
        let accent = readable(palette.primary, palette);
        let active = status.active();
        let fav_id = shared.borrow().player.with(|snap| snap.tune.as_ref().map(|t| t.id.clone()));
        let favourite = fav_id.as_deref().is_some_and(|id| shared.borrow().lib.as_ref().is_some_and(|lib| lib.is_favourite(id)));
        let level = player_level(shared);
        let play = div()
            .id("radio-play")
            .size(px(PLAY_PX))
            .flex_none()
            .rounded_full()
            .cursor_pointer()
            .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, palette.primary, partner(palette.primary).with_lightness(palette.primary.lightness())))
            .when(active, |el| el.shadow(vec![glow(accent.with_alpha(PLAY_GLOW.0 + level.min(0.3)), PLAY_GLOW.1 + level * PLAY_GLOW.2, 0.0, 1.0)]))
            .flex()
            .items_center()
            .justify_center()
            .child(icon(if active { "pause" } else { "play" }, PLAY_ICON_PX, palette.on_primary))
            .on_click(|_, window, cx| {
                super::toggle(cx);
                window.refresh();
            });
        let flash = self.flash.clone();
        let fav = {
            let id = fav_id.clone();
            let flash = flash.clone();
            icon_button("radio-fav", "heart", SIDE_BUTTON_PX, if favourite { readable(palette.urgent_coral, palette) } else { palette.on_surface_variant }, favourite.then(|| palette.urgent_coral.with_alpha(alpha(super::super::panel_fx::TINT_FILL, palette))), move |window, cx| {
                if let Some(id) = &id {
                    let flash = flash.clone();
                    super::toggle_favourite(id, cx, move |e, _| flash.show(false, e));
                }
                window.refresh();
            })
        };
        let sleeping = shared.borrow().player.sleep_remaining();
        let minutes = s.sleep_minutes;
        let sleep_label: SharedString = match sleeping {
            Some((left, _)) => self.labels.borrow_mut().sleep.get((mandara_i18n::language(), left.as_secs().div_ceil(SECS_PER_MIN)), |(_, m)| tf("{n} min", &[("n", m)])),
            None => SharedString::default(),
        };
        let sleep = div()
            .id("radio-sleep")
            .h(px(SIDE_BUTTON_PX))
            .min_w(px(SIDE_BUTTON_PX))
            .px(px(if sleeping.is_some() { tokens::SM } else { 0.0 }))
            .flex_none()
            .rounded_full()
            .cursor_pointer()
            .row()
            .items_center()
            .justify_center()
            .gap(px(tokens::XS))
            .when(sleeping.is_some(), |el| el.bg(accent.with_alpha(alpha(super::super::panel_fx::TINT_FILL, palette)).hsla()))
            .hover(|st| st.bg(wash(WASH_SOFT, palette).hsla()))
            .child(icon("moon", SIDE_BUTTON_PX * 0.48, if sleeping.is_some() { accent } else { palette.on_surface_variant }))
            .when(sleeping.is_some(), |el| el.child(div().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(accent.hsla()).child(sleep_label)))
            .on_click(move |_, window, cx| {
                // Off → the chosen length; on → off.
                let on = existing(cx).is_some_and(|m| m.borrow().player.sleep_remaining().is_some());
                super::set_sleep(if on { None } else { Some(minutes) }, cx);
                window.refresh();
            });
        div()
            .h(px(TRANSPORT_H))
            .row()
            .items_center()
            .justify_between()
            .child(div().w(px(SIDE_BUTTON_PX * 2.0 + tokens::SM)).row().gap(px(tokens::SM)).child(fav).child(sleep))
            .child(
                div()
                    .ltr_row()
                    .items_center()
                    .gap(px(tokens::LG))
                    .child(icon_button("radio-prev", "previous", SKIP_PX, palette.on_surface, None, |window, cx| {
                        super::previous(cx);
                        window.refresh();
                    }))
                    .child(play)
                    .child(icon_button("radio-next", "next", SKIP_PX, palette.on_surface, None, |window, cx| {
                        super::next(cx);
                        window.refresh();
                    })),
            )
            .child(div().w(px(SIDE_BUTTON_PX * 2.0 + tokens::SM)))
            .into_any_element()
    }
}

/// The shelf of `id` with the model already borrowed elsewhere.
fn station_category_of(shared: &Shared, id: &str) -> Category {
    if let Some(c) = mandara_services::radio::station::curated(id) {
        return c.category;
    }
    shared.borrow().lib.as_ref().and_then(|lib| lib.station(id).map(|s| s.category())).unwrap_or(Category::Other)
}

fn player_fill(shared: &Shared) -> Option<f32> {
    shared.borrow().player.buffer_fill()
}

fn player_level(shared: &Shared) -> f32 {
    shared.borrow().player.level()
}

fn status_text(status: Status, attempt: u32, buffering: bool) -> String {
    match status {
        Status::Playing if buffering => t("BUFFERING").into(),
        Status::Playing => t("LIVE").into(),
        Status::Connecting => t("CONNECTING").into(),
        Status::Buffering => t("BUFFERING").into(),
        Status::Reconnecting => tf("RECONNECTING · {n}", &[("n", &attempt)]),
        Status::Paused => t("PAUSED").into(),
        Status::Stopped => t("OFF AIR").into(),
    }
}

fn volume_row(shared: &Shared, palette: &MandaraPalette) -> AnyElement {
    let (volume, muted) = {
        let m = shared.borrow();
        (m.player.volume(), m.player.muted())
    };
    let slider = big_slider("radio-volume", volume, if muted { "volume-mute" } else { "volume-high" }, muted, palette, |v, window, cx| {
        super::set_volume(v, cx);
        window.refresh();
    });
    div()
        .h(px(VOLUME_H))
        .row()
        .items_center()
        .gap(px(tokens::SM))
        .child(div().flex_1().min_w(px(0.0)).child(slider))
        .child(icon_button("radio-mute", if muted { "volume-mute" } else { "volume-low" }, MUTE_PX, if muted { readable(palette.urgent_coral, palette) } else { palette.on_surface_variant }, None, |window, cx| {
            super::toggle_mute(cx);
            window.refresh();
        }))
        .into_any_element()
}

fn tabs(shared: &Shared, tab: Tab, palette: &MandaraPalette) -> AnyElement {
    let seg = |label: &'static str, icon_name: &'static str, which: Tab| {
        let shared = shared.clone();
        Segment {
            label: SharedString::from(tr(label)),
            icon: Some(icon_name),
            active: tab == which,
            on_click: Rc::new(move |window, cx| {
                {
                    let mut m = shared.borrow_mut();
                    m.tab = which;
                    if which != Tab::Add {
                        m.editor = super::Editor::default();
                    }
                }
                let _ = cx;
                window.refresh();
            }),
        }
    };
    segmented("radio-tabs", vec![seg(tk("Favourites"), "heart", Tab::Favourites), seg(tk("Stations"), "radio", Tab::Stations), seg(tk("Add"), "plus", Tab::Add)], palette)
}

/// A station row: art, name, a subtitle, a favourite heart and an action.
#[allow(clippy::too_many_arguments)]
fn station_row(index: usize, st: Station<'_>, current: Option<&str>, favourite: bool, s: &Settings, palette: &MandaraPalette, flash: &Flash, edit: Option<Rc<dyn Fn(&mut Window, &mut App)>>) -> AnyElement {
    let id = st.id().to_string();
    let on = current == Some(st.id());
    let accent = readable(palette.primary, palette);
    let hover = wash(WASH_SOFT, palette);
    let subtitle = row_subtitle(&st);
    let fav = {
        let id = id.clone();
        let flash = flash.clone();
        icon_button(("radio-row-fav", index), "heart", ROW_ACTION_PX, if favourite { readable(palette.urgent_coral, palette) } else { palette.on_surface_variant.with_alpha(0.6) }, None, move |window, cx| {
            let flash = flash.clone();
            super::toggle_favourite(&id, cx, move |e, _| flash.show(false, e));
            window.refresh();
        })
    };
    let play_id = id.clone();
    div()
        .id(("radio-row", index))
        .h(px(ROW_H_PX))
        .px(px(ROW_PAD_X_PX))
        .flex_none()
        .rounded(px(crate::style::radius(ROW_ART_RADIUS_PX + 4.0)))
        .row()
        .items_center()
        .gap(px(tokens::MD))
        .cursor_pointer()
        .when(on, |el| el.bg(accent.with_alpha(alpha(super::super::panel_fx::TINT_FILL, palette)).hsla()))
        .when(!on, |el| el.hover(move |st| st.bg(hover.hsla())))
        .child(art::tile(st.id(), st.name(), st.category(), s.art, ROW_ART_PX, crate::style::radius(ROW_ART_RADIUS_PX), palette))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(text::BODY)).font_weight(if on { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM }).text_color(if on { accent.hsla() } else { palette.on_surface.hsla() }).truncate().child(st.name().to_string()))
                .child(div().text_size(px(text::CAPTION)).text_color(palette.on_surface_variant.hsla()).truncate().child(subtitle)),
        )
        .when_some(edit, |el, edit| el.child(icon_button(("radio-row-edit", index), "pencil", ROW_ACTION_PX, palette.on_surface_variant, None, move |window, cx| edit(window, cx))))
        .child(fav)
        .on_click(move |_, window, cx| {
            super::play(&play_id, cx);
            window.refresh();
        })
        .into_any_element()
}

/// "Jazz · FR · MP3 128" (built per row; rows are built only for the
/// visible tab).
fn row_subtitle(st: &Station<'_>) -> String {
    let mut s = tr(st.category().label());
    if !st.country().is_empty() {
        s.push_str(" · ");
        s.push_str(st.country());
    }
    if let Some(k) = st.kbps() {
        s.push_str(" · ");
        s.push_str(&tf("{n} kbit/s", &[("n", &k)]));
    }
    s
}

fn favourites(shared: &Shared, s: &Settings, palette: &MandaraPalette, flash: &Flash) -> AnyElement {
    let m = shared.borrow();
    let Some(lib) = m.lib.as_ref() else {
        return empty_state("radio", t("Opening your stations…"), "", LIST_H, palette).into_any_element();
    };
    if lib.favourites.is_empty() {
        return empty_state("heart", t("No favourites yet"), t("Tap the heart on any station to keep it here"), LIST_H, palette).into_any_element();
    }
    let current = m.player.with(|snap| snap.tune.as_ref().map(|t| t.id.clone()));
    let manage = m.manage;
    let accent = readable(palette.primary, palette);
    let head = div()
        .row()
        .items_center()
        .justify_between()
        .child(caption(tf("{n} favourites", &[("n", &lib.favourites.len())]), palette))
        .child({
            let shared = shared.clone();
            pill_button("radio-manage", Some(if manage { "check" } else { "pencil" }), if manage { t("Done") } else { t("Reorder") }, accent, palette, move |window, _| {
                let on = !shared.borrow().manage;
                shared.borrow_mut().manage = on;
                window.refresh();
            })
        });
    let body: AnyElement = if manage {
        let count = lib.favourites.len();
        div()
            .flex()
            .flex_col()
            .gap(px(tokens::XS))
            .children(lib.favourites.iter().enumerate().filter_map(|(i, id)| {
                let st = lib.station(id)?;
                let (up, down) = (id.clone(), id.clone());
                Some(
                    div()
                        .h(px(ROW_H_PX))
                        .px(px(ROW_PAD_X_PX))
                        .row()
                        .items_center()
                        .gap(px(tokens::MD))
                        .child(art::tile(st.id(), st.name(), st.category(), s.art, ROW_ART_PX, crate::style::radius(ROW_ART_RADIUS_PX), palette))
                        .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY)).child(st.name().to_string()))
                        .when(i > 0, |el| {
                            el.child(icon_button(("radio-up", i), "chevron-up", ROW_ACTION_PX, palette.on_surface_variant, None, move |window, cx| {
                                super::move_favourite(&up, -1, cx);
                                window.refresh();
                            }))
                        })
                        .when(i + 1 < count, |el| {
                            el.child(icon_button(("radio-down", i), "chevron-down", ROW_ACTION_PX, palette.on_surface_variant, None, move |window, cx| {
                                super::move_favourite(&down, 1, cx);
                                window.refresh();
                            }))
                        }),
                )
            }))
            .into_any_element()
    } else {
        div()
            .flex()
            .flex_wrap()
            .gap(px(tokens::SM))
            .children(lib.favourites.iter().enumerate().filter_map(|(i, id)| {
                let st = lib.station(id)?;
                let on = current.as_deref() == Some(st.id());
                let play_id = id.clone();
                Some(
                    div()
                        .id(("radio-fav-tile", i))
                        .w(px(TILE_W_PX))
                        .h(px(TILE_H_PX))
                        .rounded(px(crate::style::radius(TILE_ART_RADIUS_PX)))
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(tokens::XS + 2.0))
                        .cursor_pointer()
                        .when(on, |el| el.bg(accent.with_alpha(alpha(super::super::panel_fx::TINT_FILL, palette)).hsla()))
                        .hover(|st| st.bg(wash(WASH_SOFT, palette).hsla()))
                        .child(art::tile(st.id(), st.name(), st.category(), s.art, TILE_ART_PX, crate::style::radius(TILE_ART_RADIUS_PX), palette))
                        .child(div().max_w(px(TILE_W_PX - tokens::SM)).truncate().text_size(px(text::LABEL)).font_weight(if on { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM }).text_color(if on { accent.hsla() } else { palette.on_surface.hsla() }).child(st.name().to_string()))
                        .on_click(move |_, window, cx| {
                            super::play(&play_id, cx);
                            window.refresh();
                        }),
                )
            }))
            .into_any_element()
    };
    let _ = flash;
    div()
        .size_full()
        .flex()
        .flex_col()
        .gap(px(tokens::SM))
        .child(head)
        .child(div().id("radio-fav-scroll").flex_1().min_h(px(0.0)).overflow_y_scroll().child(body))
        .into_any_element()
}

fn shelf_chip(id: (&'static str, usize), label: SharedString, icon_name: &'static str, active: bool, palette: &MandaraPalette, on_click: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let hover = wash(WASH_SOFT, palette);
    div()
        .id(id)
        .h(px(SHELF_CHIP_H_PX))
        .px(px(SHELF_CHIP_PAD_X_PX))
        .flex_none()
        .rounded_full()
        .row()
        .items_center()
        .gap(px(tokens::XS))
        .cursor_pointer()
        .text_size(px(text::LABEL))
        .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
        .when(active, |el| el.bg(accent.with_alpha(alpha(super::super::panel_fx::TINT_FILL, palette)).hsla()).text_color(accent.hsla()))
        .when(!active, |el| el.bg(palette.on_surface.with_alpha(0.05).hsla()).text_color(palette.on_surface_variant.hsla()).hover(move |s| s.bg(hover.hsla())))
        .child(icon(icon_name, text::LABEL + 1.0, if active { accent } else { palette.on_surface_variant }))
        .child(label)
        .on_click(move |_, window, cx| on_click(window, cx))
        .into_any_element()
}

fn stations(shared: &Shared, s: &Settings, search: &Entity<TextField>, palette: &MandaraPalette, flash: &Flash, cx: &mut App) -> AnyElement {
    let query = search.read(cx).value.clone();
    let lower = query.trim().to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    let m = shared.borrow();
    let chosen = m.category;
    let current = m.player.with(|snap| snap.tune.as_ref().map(|t| t.id.clone()));
    let mut chips: Vec<AnyElement> = Vec::with_capacity(Category::ALL.len() + 1);
    {
        let shared = shared.clone();
        chips.push(shelf_chip(("radio-shelf", usize::MAX), t("All").into(), "grid", chosen.is_none(), palette, move |window, _| {
            shared.borrow_mut().category = None;
            window.refresh();
        }));
    }
    let custom_present = m.lib.as_ref().is_some_and(|l| !l.custom.is_empty());
    for c in Category::ALL.into_iter().filter(|c| s.categories[c.index()] && (*c != Category::Other || custom_present)) {
        let shared = shared.clone();
        chips.push(shelf_chip(("radio-shelf", c.index()), tr(c.label()).into(), c.icon(), chosen == Some(c), palette, move |window, _| {
            let mut m = shared.borrow_mut();
            m.category = if m.category == Some(c) { None } else { Some(c) };
            window.refresh();
        }));
    }
    let shelves = div().id("radio-shelves").flex_none().overflow_x_scroll().row().gap(px(SHELF_GAP_PX)).children(chips);
    let mut rows: Vec<AnyElement> = Vec::new();
    let fallback = mandara_services::radio::Library::default();
    let lib = m.lib.as_ref().unwrap_or(&fallback);
    // Shelf names in the current language, made once per list.
    let labels: Vec<String> = if words.is_empty() { Vec::new() } else { Category::ALL.iter().map(|c| tr(c.label()).to_lowercase()).collect() };
    for (i, st) in lib.all().enumerate() {
        let c = st.category();
        if !s.categories[c.index()] || chosen.is_some_and(|ch| ch != c) {
            continue;
        }
        if !words.is_empty() && !st.matches(&words, &labels[c.index()]) {
            continue;
        }
        let edit: Option<Rc<dyn Fn(&mut Window, &mut App)>> = st.is_custom().then(|| {
            let (shared, id) = (shared.clone(), st.id().to_string());
            Rc::new(move |window: &mut Window, cx: &mut App| {
                open_editor(&shared, Some(&id), cx);
                window.refresh();
            }) as Rc<dyn Fn(&mut Window, &mut App)>
        });
        rows.push(station_row(i, st, current.as_deref(), lib.is_favourite(st.id()), s, palette, flash, edit));
    }
    let found_none = rows.is_empty();
    // The directory, on request.
    let dir = &m.directory;
    let mut dir_rows: Vec<AnyElement> = Vec::new();
    if s.directory && !query.trim().is_empty() {
        let accent = readable(palette.primary, palette);
        if dir.query != query.trim() || (!dir.busy && dir.results.is_empty() && dir.error.is_none()) {
            let q = query.clone();
            dir_rows.push(
                pill_button("radio-dir-search", Some("globe"), tf("Search radio-browser.info for “{q}”", &[("q", &query.trim())]), accent, palette, move |window, cx| {
                    super::search_directory(&q, cx);
                    window.refresh();
                })
                .into_any_element(),
            );
        } else if dir.busy {
            dir_rows.push(caption(t("Searching the directory…"), palette).into_any_element());
        } else if let Some(e) = &dir.error {
            dir_rows.push(caption(e.clone(), palette).into_any_element());
        } else {
            dir_rows.push(caption(tf("{n} from radio-browser.info", &[("n", &dir.results.len())]), palette).into_any_element());
            for (i, f) in dir.results.iter().enumerate() {
                dir_rows.push(found_row(i, f, current.as_deref(), palette));
            }
        }
    }
    drop(m);
    let list = div()
        .id("radio-list")
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(LIST_GAP_PX))
        .when(found_none && dir_rows.is_empty(), |el| el.child(empty_state("search", t("No station matches"), t("Try another word or shelf"), LIST_H - CONTROL_H - SHELF_CHIP_H_PX - tokens::SM * 2.0, palette)))
        .children(rows)
        .when(!dir_rows.is_empty(), |el| el.child(div().pt(px(tokens::SM)).flex().flex_col().gap(px(tokens::XS)).children(dir_rows)));
    div()
        .size_full()
        .flex()
        .flex_col()
        .gap(px(tokens::SM))
        .child(div().row().child(search_box("radio-search-clear", search, &query, palette)))
        .child(shelves)
        .child(list)
        .into_any_element()
}

fn found_row(i: usize, f: &directory::Found, current: Option<&str>, palette: &MandaraPalette) -> AnyElement {
    let dir_id = format!("dir-{}", f.uuid);
    let on = current == Some(dir_id.as_str());
    let accent = readable(palette.primary, palette);
    let hover = wash(WASH_SOFT, palette);
    let mut sub = Vec::new();
    if !f.country.is_empty() {
        sub.push(f.country.clone());
    }
    if !f.codec.is_empty() {
        sub.push(f.codec.clone());
    }
    if f.kbps > 0 {
        sub.push(tf("{n} kbit/s", &[("n", &f.kbps)]));
    }
    let found = f.clone();
    div()
        .id(("radio-found", i))
        .h(px(ROW_H_PX))
        .px(px(ROW_PAD_X_PX))
        .rounded(px(crate::style::radius(ROW_ART_RADIUS_PX + 4.0)))
        .row()
        .items_center()
        .gap(px(tokens::MD))
        .cursor_pointer()
        .when(on, |el| el.bg(accent.with_alpha(alpha(super::super::panel_fx::TINT_FILL, palette)).hsla()))
        .when(!on, |el| el.hover(move |s| s.bg(hover.hsla())))
        .child(art::tile(&dir_id, &f.name, Category::Other, super::ArtStyle::Soft, ROW_ART_PX, crate::style::radius(ROW_ART_RADIUS_PX), palette))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(text::BODY)).truncate().child(f.name.clone()))
                .child(div().text_size(px(text::CAPTION)).text_color(palette.on_surface_variant.hsla()).truncate().child(sub.join(" · "))),
        )
        .child(icon("play", ROW_ACTION_PX * 0.45, palette.on_surface_variant))
        .on_click(move |_, window, cx| {
            super::play_found(&found, cx);
            window.refresh();
        })
        .into_any_element()
}

/// Opens the editor on one of your stations (or a new one).
fn open_editor(shared: &Shared, id: Option<&str>, cx: &mut App) {
    let mut m = shared.borrow_mut();
    m.tab = Tab::Add;
    m.editor = super::Editor { editing: id.map(str::to_string), ..super::Editor::default() };
    let existing = id.and_then(|id| m.lib.as_ref().and_then(|lib| lib.custom.iter().find(|c| c.id == id).cloned()));
    m.editor.category = existing.as_ref().map(|c| c.category);
    drop(m);
    // The editor's fields are filled from the station.
    PENDING_FILL.with(|p| *p.borrow_mut() = Some(existing.map(|c| (c.name, c.url)).unwrap_or_default()));
    let _ = cx;
}

/// Puts `name` and `url` in the editor's fields on its next render.
pub(super) fn fill_editor(name: String, url: String) {
    PENDING_FILL.with(|p| *p.borrow_mut() = Some((name, url)));
}

thread_local! {
    /// Values to put in the editor's fields on its next render.
    static PENDING_FILL: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
}

fn field_box(field: &Entity<TextField>, palette: &MandaraPalette) -> AnyElement {
    div()
        .h(px(FIELD_H_PX))
        .px(px(FIELD_PAD_X_PX))
        .rounded(px(crate::style::radius(FIELD_RADIUS_PX)))
        .bg(palette.surface_container_high.hsla())
        .flex()
        .items_center()
        .child(div().flex_1().min_w(px(0.0)).child(field.clone()))
        .into_any_element()
}

fn editor(shared: &Shared, name: &Entity<TextField>, url: &Entity<TextField>, palette: &MandaraPalette, cx: &mut App) -> AnyElement {
    if let Some((n, u)) = PENDING_FILL.with(|p| p.borrow_mut().take()) {
        name.update(cx, |f, _| f.set(&n));
        url.update(cx, |f, _| f.set(&u));
    }
    let m = shared.borrow();
    let ed = &m.editor;
    let editing = ed.editing.clone();
    let chosen = ed.category.unwrap_or(Category::Other);
    let testing = ed.testing.is_some();
    let result = ed.result.clone();
    drop(m);
    let accent = readable(palette.primary, palette);
    let url_value = url.read(cx).value.clone();
    let checked = mandara_services::radio::station::validate_url(&url_value);
    // Other (the default for a new station) first, so the chosen shelf shows.
    let order = std::iter::once(Category::Other).chain(Category::ALL.into_iter().filter(|c| *c != Category::Other));
    let shelves = div().id("radio-edit-shelves").flex_none().overflow_x_scroll().row().gap(px(SHELF_GAP_PX)).children(order.map(|c| {
        let shared = shared.clone();
        shelf_chip(("radio-edit-shelf", c.index()), tr(c.label()).into(), c.icon(), chosen == c, palette, move |window, _| {
            shared.borrow_mut().editor.category = Some(c);
            window.refresh();
        })
    }));
    let save = {
        let (shared, name, url) = (shared.clone(), name.clone(), url.clone());
        let editing = editing.clone();
        let label = if testing {
            t("Testing…")
        } else if editing.is_some() {
            t("Test and save")
        } else {
            t("Test and add")
        };
        pill_button("radio-edit-save", Some(if testing { "refresh" } else { "check" }), label, accent, palette, move |window, cx| {
            if shared.borrow().editor.testing.is_some() {
                return;
            }
            let n = name.read(cx).value.clone();
            let u = url.read(cx).value.trim().to_string();
            let c = shared.borrow().editor.category.unwrap_or(Category::Other);
            let _ = super::test_and_save(n, u, c, editing.clone(), cx);
            window.refresh();
        })
    };
    let remove = editing.clone().map(|id| {
        let shared = shared.clone();
        pill_button("radio-edit-remove", Some("trash"), t("Remove"), readable(palette.urgent_coral, palette), palette, move |window, cx| {
            super::remove_station(&id, cx);
            let mut m = shared.borrow_mut();
            m.editor = super::Editor::default();
            m.tab = Tab::Stations;
            window.refresh();
        })
    });
    let status: Option<AnyElement> = match (&result, checked) {
        (Some(Ok(msg)), _) => Some(note("check", msg.clone(), readable(palette.success_emerald, palette), palette)),
        (Some(Err(e)), _) => Some(note("alert", e.clone(), readable(palette.urgent_coral, palette), palette)),
        (None, Err(e)) if !url_value.trim().is_empty() => Some(note("alert", tr(e.message()), readable(palette.warning_amber, palette), palette)),
        _ => None,
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .gap(px(tokens::SM))
        .child(caption(if editing.is_some() { t("Change your station") } else { t("Add a station by its stream address; Mandara listens for a moment to check it plays") }, palette))
        .child(field_box(url, palette))
        .child(field_box(name, palette))
        .child(caption(t("Shelf"), palette))
        .child(shelves)
        .when_some(status, |el, s| el.child(s))
        .child(div().flex_1())
        .child(div().row().gap(px(tokens::SM)).justify_end().when_some(remove, |el, r| el.child(r)).child(save))
        .into_any_element()
}

fn note(icon_name: &'static str, msg: String, color: Color, palette: &MandaraPalette) -> AnyElement {
    let _ = palette;
    div().row().items_center().gap(px(tokens::XS)).child(icon(icon_name, ERROR_ICON_PX, color)).child(div().flex_1().min_w(px(0.0)).text_size(px(text::LABEL)).text_color(color.hsla()).child(crate::bidi::paragraph(msg))).into_any_element()
}

// ── Sidebar card ─────────────────────────────────────────────────────────

const SIDE_ART_PX: f32 = 52.0;
const SIDE_ART_RADIUS_PX: f32 = 14.0;
const SIDE_PLAY_PX: f32 = 38.0;
const SIDE_SKIP_PX: f32 = 32.0;
const SIDE_HEART_PX: f32 = 30.0;
/// The play glyph as a share of its button.
const SIDE_PLAY_GLYPH: f32 = 0.42;
/// The card's second gradient stop takes half the tint.
const SIDE_TINT_FALLOFF: f32 = 0.5;

/// The sidebar card: the station's art, what plays and a live dot, a
/// favourite heart, previous / play / next (through your favourites) and
/// the volume in the slider design picked under Themes → Components. It
/// makes nothing until you use it: a fresh card shows the usual-station
/// prompt without loading the radio.
pub(super) fn sidebar(palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let s = settings(cx);
    let accent = readable(palette.primary, palette);
    let shared = existing(cx);
    if let Some(m) = &shared {
        m.borrow().views.track(window);
    }
    let (status, tune, title) = shared.as_ref().map_or((Status::Stopped, None, None), |m| m.borrow().player.with(|snap| (snap.status, snap.tune.as_ref().map(|t| (t.id.clone(), t.name.clone())), snap.title.clone())));
    let (volume, muted, favourite) = shared.as_ref().map_or((mandara_services::radio::player::DEFAULT_VOLUME, false, false), |m| {
        let m = m.borrow();
        let fav = tune.as_ref().is_some_and(|(id, _)| m.lib.as_ref().is_some_and(|lib| lib.is_favourite(id)));
        (m.player.volume(), m.player.muted(), fav)
    });
    let active = status.active();
    let buffering = shared.as_ref().is_some_and(|m| m.borrow().player.buffering());
    let art_el: AnyElement = match &tune {
        Some((id, name)) => art::tile(id, name, super::station_category(id, cx), s.art, SIDE_ART_PX, crate::style::radius(SIDE_ART_RADIUS_PX), palette).into_any_element(),
        None => art::placeholder(SIDE_ART_PX, crate::style::radius(SIDE_ART_RADIUS_PX), palette),
    };
    let (line1, line2): (SharedString, SharedString) = match (&tune, &title) {
        (Some((_, name)), Some(t_)) => (t_.clone().into(), name.clone().into()),
        (Some((_, name)), None) => (name.clone().into(), if active { t("Live").into() } else { t("Paused").into() }),
        (None, _) => (t("Radio").into(), t("Press play for your usual station").into()),
    };
    let (a, b) = tune.as_ref().map_or((palette.primary, partner(palette.primary)), |(id, name)| art::colors(name, super::station_category(id, cx), s.art, palette));
    let tint = alpha(HERO_TINT, palette);
    let dot_color = if status == Status::Playing && !buffering { readable(palette.urgent_coral, palette) } else { readable(palette.warning_amber, palette) };
    let play = div()
        .id("radio-side-play")
        .size(px(SIDE_PLAY_PX))
        .flex_none()
        .rounded_full()
        .cursor_pointer()
        .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, palette.primary, partner(palette.primary).with_lightness(palette.primary.lightness())))
        .flex()
        .items_center()
        .justify_center()
        .child(icon(if active { "pause" } else { "play" }, SIDE_PLAY_PX * SIDE_PLAY_GLYPH, palette.on_primary))
        .on_click(|_, window, cx| {
            super::toggle(cx);
            window.refresh();
        });
    let heart = tune.as_ref().map(|(id, _)| {
        let id = id.clone();
        icon_button("radio-side-fav", "heart", SIDE_HEART_PX, if favourite { readable(palette.urgent_coral, palette) } else { palette.on_surface_variant }, None, move |window, cx| {
            super::toggle_favourite(&id, cx, |e, _| tracing::warn!("radio: {e}"));
            window.refresh();
        })
    });
    let slider = big_slider("radio-side-volume", volume, if muted { "volume-mute" } else { "volume-high" }, muted, palette, |v, window, cx| {
        super::set_volume(v, cx);
        window.refresh();
    });
    card(palette)
        .bg(oklab_gradient(ANGLE_DOWN_DEG, palette.surface_container.mix(a, tint), palette.surface_container.mix(b, tint * SIDE_TINT_FALLOFF)))
        .flex()
        .flex_col()
        .gap(px(tokens::MD))
        .child(
            div()
                .row()
                .items_center()
                .gap(px(tokens::MD))
                .child(art_el)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(div().font_weight(FontWeight::SEMIBOLD).truncate().child(line1))
                        .child(
                            div()
                                .row()
                                .items_center()
                                .gap(px(tokens::XS))
                                .when(active, |el| el.child(div().size(px(BADGE_DOT_PX)).flex_none().rounded_full().bg(dot_color.hsla())))
                                .child(div().min_w(px(0.0)).text_size(px(text::LABEL)).text_color(accent.hsla()).truncate().child(line2)),
                        ),
                )
                .when_some(heart, |el, h| el.child(h)),
        )
        .child(
            div()
                .ltr_row()
                .items_center()
                .justify_center()
                .gap(px(tokens::LG))
                .child(icon_button("radio-side-prev", "previous", SIDE_SKIP_PX, palette.on_surface, None, |window, cx| {
                    super::previous(cx);
                    window.refresh();
                }))
                .child(play)
                .child(icon_button("radio-side-next", "next", SIDE_SKIP_PX, palette.on_surface, None, |window, cx| {
                    super::next(cx);
                    window.refresh();
                })),
        )
        .child(slider)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_words_cover_every_state() {
        for (st, attempt, buffering) in [(Status::Playing, 0, false), (Status::Playing, 0, true), (Status::Connecting, 0, false), (Status::Buffering, 0, false), (Status::Reconnecting, 3, false), (Status::Paused, 0, false), (Status::Stopped, 0, false)] {
            assert!(!status_text(st, attempt, buffering).is_empty());
        }
        assert!(status_text(Status::Reconnecting, 3, false).contains('3'));
        assert!(panel_height() > LIST_H + HERO_H);
        assert!(crate::sidebar_tabs::SIDEBAR_WIDGETS.contains(&super::super::ID), "the radio offers a sidebar card");

    }
}
