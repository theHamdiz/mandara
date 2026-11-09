//! The bar item: the radio glyph while off; the station's art, a live dot
//! and a now-playing marquee while a station is chosen. The marquee moves
//! only while the station plays, the text is wider than its room and the
//! bar is drawn (a hidden bar schedules no frames); reduced motion keeps
//! it still. Its text is measured once per title, font and size.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use gpui::{AnyElement, App, IntoElement, ParentElement, SharedString, Styled, Window, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui;
use mandara_services::radio::Status;
use mandara_theme::{Color, MandaraPalette};

use super::{BarStyle, existing, settings};
use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::plugins::panel_fx::{readable, redraw_in};

const ICON_PX: f32 = 15.0;
const ART_PX: f32 = 18.0;
const ART_RADIUS_PX: f32 = 5.0;
const GAP_PX: f32 = 6.0;
const DOT_PX: f32 = 6.0;
/// The live dot breathes between these alphas over this period.
const DOT_ALPHA: (f32, f32) = (0.45, 1.0);
const DOT_PERIOD_S: f32 = 1.6;
/// Marquee frame cadence and the blank between two passes of the text.
const MARQUEE_FRAME: Duration = Duration::from_millis(33);
const MARQUEE_GAP_PX: f32 = 36.0;
/// Hold the start of the title this long before it scrolls.
const MARQUEE_HOLD_S: f32 = 1.5;
/// Dot cadence while buffering or reconnecting without a marquee.
const DOT_FRAME: Duration = Duration::from_millis(80);

#[derive(Default)]
struct Marquee {
    /// `(text, font, size bits)` measured, and its width.
    key: Option<(SharedString, SharedString, u32)>,
    width: f32,
    /// When this title started (the scroll's clock).
    since: Option<Instant>,
}

thread_local! {
    static MARQUEE: RefCell<Marquee> = RefCell::new(Marquee::default());
}

/// The live dot's colour: coral while playing live, amber while it waits.
fn dot_color(status: Status, buffering: bool, palette: &MandaraPalette) -> Color {
    match status {
        Status::Playing if !buffering => readable(palette.urgent_coral, palette),
        Status::Paused | Status::Stopped => palette.on_surface_variant,
        _ => readable(palette.warning_amber, palette),
    }
}

/// The width of `text` in the bar's font (measured on change only).
fn measure(text: &SharedString, font: &SharedString, size: f32, window: &mut Window) -> f32 {
    MARQUEE.with(|m| {
        let mut m = m.borrow_mut();
        let fresh = m.key.as_ref().is_none_or(|(t, f, s)| t != text || f != font || *s != size.to_bits());
        if fresh {
            let run = gpui::TextRun { len: text.len(), font: gpui::font(font.clone()), color: gpui::black(), background_color: None, underline: None, strikethrough: None };
            m.width = f32::from(window.text_system().layout_line(text, px(size), &[run], None).width);
            m.key = Some((text.clone(), font.clone(), size.to_bits()));
            m.since = Some(Instant::now());
        }
        m.width
    })
}

/// Seconds since the current title appeared.
fn title_age() -> f32 {
    MARQUEE.with(|m| m.borrow().since.map_or(0.0, |s| s.elapsed().as_secs_f32()))
}

/// Frees the measured title (the plugin went off, or the station stopped).
pub(super) fn clear() {
    MARQUEE.with(|m| *m.borrow_mut() = Marquee::default());
}

pub(super) fn render(palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let glyph = || icon("radio", crate::style::bar_icon(ICON_PX), palette.on_surface).into_any_element();
    let Some(shared) = existing(cx) else { return glyph() };
    let s = settings(cx);
    let m = shared.borrow();
    m.views.track(window);
    let player = m.player.clone();
    let buffering = player.buffering();
    let (status, tune, title) = player.with(|snap| (snap.status, snap.tune.as_ref().map(|t| (t.id.clone(), t.name.clone())), snap.title.clone()));
    drop(m);
    let Some((id, name)) = tune.filter(|_| status != Status::Stopped) else {
        clear();
        return glyph();
    };
    if s.bar_style == BarStyle::Icon {
        return div().row().items_center().gap(px(GAP_PX)).child(glyph()).when(status.active(), |el| el.child(div().size(px(DOT_PX)).rounded_full().bg(dot_color(status, buffering, palette).hsla()))).into_any_element();
    }
    let category = super::station_category(&id, cx);
    let reduced = crate::state::config(cx).read(cx).get().motion.reduced(mandara_core::spring::system_reduced_motion());
    let live = status == Status::Playing && !buffering;
    // The dot breathes only while it waits (connecting, buffering,
    // reconnecting): playing costs no frames unless the marquee scrolls.
    let breathing = status.active() && !live && !reduced;
    let dot_alpha = if breathing {
        let t = title_age() / DOT_PERIOD_S * std::f32::consts::TAU;
        DOT_ALPHA.0 + (DOT_ALPHA.1 - DOT_ALPHA.0) * (0.5 + 0.5 * t.cos())
    } else {
        DOT_ALPHA.1
    };
    let dot = div().size(px(DOT_PX)).flex_none().rounded_full().bg(dot_color(status, buffering, palette).with_alpha(dot_alpha).hsla());
    let art = super::art::tile(&id, &name, category, s.art, crate::style::bar_icon(ART_PX), crate::style::radius(ART_RADIUS_PX), palette);
    let mut row = div().row().items_center().gap(px(GAP_PX)).child(art).child(dot);
    let mut animating = false;
    if s.bar_style == BarStyle::Full {
        let text: SharedString = title.unwrap_or(name).into();
        let cfg = crate::state::config(cx).read(cx).snapshot();
        let size = crate::shell::bar_text_px(&cfg);
        let font = crate::fonts::fonts(cx).bar;
        let width = measure(&text, &font, size, window);
        let room = s.bar_width;
        let scroll = width > room && live && s.marquee_px_s > 0.0 && !reduced;
        let label = if scroll {
            animating = true;
            let travel = width + MARQUEE_GAP_PX;
            let offset = ((title_age() - MARQUEE_HOLD_S).max(0.0) * s.marquee_px_s) % travel;
            // Two copies, so the text wraps around without a jump; the
            // marquee runs toward the reading direction's start.
            let sign = if crate::bidi::rtl() { 1.0 } else { -1.0 };
            div()
                .w(px(room))
                .h(px(size * TEXT_LINE))
                .relative()
                .overflow_hidden()
                .child(div().absolute().top_0().left(px(sign * offset)).whitespace_nowrap().text_color(palette.on_surface_variant.hsla()).child(text.clone()))
                .child(div().absolute().top_0().left(px(sign * (offset - travel))).whitespace_nowrap().text_color(palette.on_surface_variant.hsla()).child(text))
                .into_any_element()
        } else {
            div().max_w(px(room)).truncate().whitespace_nowrap().text_color(palette.on_surface_variant.hsla()).child(text).into_any_element()
        };
        row = row.child(label);
    }
    if animating {
        redraw_in(MARQUEE_FRAME, window, cx);
    } else if breathing {
        redraw_in(DOT_FRAME, window, cx);
    }
    row.into_any_element()
}

/// Line height of the marquee strip as a share of the text size.
const TEXT_LINE: f32 = 1.4;
