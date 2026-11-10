//! Ghostline: the wireframe pieces. Cyberpunk made clean: a graphite
//! ground, ice-white hairlines, one cold accent (the theme's primary) and
//! a sparing second one (its tertiary), everything measured.
//!
//! * [`hud_overlay`] (`background.overlay = "hud"`): corner brackets, a
//!   faint grid, tick rulers along two edges and a slow scan line.
//! * [`GreeterView::ruler_clock`] (`clock.face = "ruler"`): monospace
//!   digits stacked beside a ruler of the day whose marker is the time.
//! * [`GreeterView::hud_card`] (`card.style = "hud"`): a bracketed panel
//!   with a ruled header, live readouts, the user, the field and the
//!   session picker.
//! * [`GreeterView::terminal_field`] (`password.style = "terminal"`): a
//!   prompt, one cell per character and a block caret.
//! * [`led_status`]: the status line under the terminal and recessed
//!   fields (a small light and the message).
//!
//! Readouts are real or left out: the host name, the uptime and the time
//! zone are read once, then advanced per minute ([`Telemetry`]).

use std::time::Instant;

use gpui::{AnyElement, ClickEvent, Context, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Transformation, div, prelude::FluentBuilder as _, px, svg};
use mandara_core::password_symbols::{self as symbols, Symbol};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::fonts::tabular_nums;

use super::{BESIDE_GAP, CLOCK_IDLE_GROWTH, CLOCK_SUCCESS_GROWTH, CARET_HOLD, Field, Frame, GreeterView, Phase, STACKED_LINE, glow, icon, snap_size, soft_shadow};
use crate::config::{CardStyle, ClockFace, Element, GreeterConfig, PasswordMask, SubmitButton};
use crate::i18n::tl;

// ── Ink ─────────────────────────────────────────────────────────────────
/// Hairlines, labels and readouts are white at these opacities over the
/// graphite (labels and readouts stay above WCAG AA there; see the tests).
const HAIRLINE_ALPHA: f32 = 0.13;
const LABEL_ALPHA: f32 = 0.56;
const VALUE_ALPHA: f32 = 0.92;
/// The panel's and the field's graphite (OKLCH lightness, chroma, hue).
const PANEL_INK: (f32, f32, f32) = (0.175, 0.006, 250.0);
const FIELD_INK: (f32, f32, f32) = (0.135, 0.005, 250.0);
/// The accents' lightness floor (the theme's primary and tertiary).
const ACCENT_LIGHTNESS: f32 = 0.78;

// ── The overlay ─────────────────────────────────────────────────────────
/// Brackets sit this far in from the screen's corners; their arms.
const FRAME_INSET: f32 = 28.0;
const BRACKET_ARM: f32 = 22.0;
const BRACKET_ALPHA: f32 = 0.34;
/// The faint grid's pitch and opacity.
const GRID_STEP: f32 = 120.0;
const GRID_ALPHA: f32 = 0.028;
/// Edge rulers: tick pitch, minor / major length and opacity; every fifth
/// tick is major. The bottom ruler stops at this share of the width
/// (clear of the power buttons).
const TICK_STEP: f32 = 20.0;
const TICK_MINOR: (f32, f32) = (4.0, 0.14);
const TICK_MAJOR: (f32, f32) = (9.0, 0.30);
const MAJOR_EVERY: usize = 5;
const BOTTOM_RULER_SHARE: f32 = 0.58;
/// Room between a bracket and the ruler that follows it.
const RULER_CLEARANCE: f32 = 20.0;
/// The scan line: one sweep from top to bottom (s), its trail (share of
/// the height) and opacities.
const SCAN_PERIOD_S: f32 = 16.0;
const SCAN_TRAIL: f32 = 0.16;
const SCAN_TRAIL_ALPHA: f32 = 0.035;
const SCAN_LINE_ALPHA: f32 = 0.16;

// ── The ruler clock ─────────────────────────────────────────────────────
/// The ruler's width (labels and ticks) and the gap to the digits.
pub(super) const RULER_W: f32 = 52.0;
pub(super) const RULER_GAP: f32 = 26.0;
/// Half-hour ticks across the day; hour and six-hour ticks are longer.
const DAY_TICKS: usize = 48;
const TICKS_PER_LABEL: usize = 12;
const DAY_TICK: [(f32, f32); 3] = [(5.0, 0.18), (9.0, 0.32), (15.0, 0.6)];
const DAY_LABELS: [&str; 5] = ["00", "06", "12", "18", "24"];
const RULER_LABEL_PX: f32 = 10.0;
/// The time marker: its length over the ticks and its thickness.
const MARKER_W: f32 = 30.0;
const MARKER_H: f32 = 2.0;
const MINUTES_PER_DAY: f32 = 1440.0;
#[cfg(test)]
/// Monospace digits advance this share of the size (JetBrains Mono's 600
/// units per em; other monospace families are close).
pub(super) const MONO_ADVANCE: f32 = 0.6;
/// Between the digits and the date line under them.
pub(super) const DATE_GAP: f32 = 14.0;
pub(super) const DATE_LINE: f32 = 1.35;

// ── The panel ───────────────────────────────────────────────────────────
pub(super) const HEADER_H: f32 = 16.0;
pub(super) const SECTION_GAP: f32 = 12.0;
pub(super) const TICKS_H: f32 = 6.0;
pub(super) const READOUTS_H: f32 = 32.0;
pub(super) const BLOCK_GAP: f32 = 22.0;
pub(super) const LABEL_H: f32 = 14.0;
pub(super) const LABEL_GAP: f32 = 8.0;
pub(super) const STATUS_TOP: f32 = 8.0;
pub(super) const STATUS_H: f32 = 20.0;
pub(super) const SESSION_GAP: f32 = 16.0;
#[cfg(test)]
/// A manual login adds the username line and this gap above the field.
pub(super) const USERNAME_GAP: f32 = 12.0;
#[cfg(test)]
/// The session picker's height as a share of the field's.
pub(super) const SESSION_SHARE: f32 = 0.9;
const PANEL_TICK: f32 = 8.0;
const LED: f32 = 6.0;
const LABEL_PX: f32 = 10.5;
const READOUT_LABEL_PX: f32 = 9.5;
const READOUT_VALUE_PX: f32 = 12.5;
const READOUT_GAP: f32 = 18.0;
const BRACKET_PANEL: f32 = 14.0;
const BACK_PX: f32 = 22.0;
/// The avatar beside the name, and between them.
pub(super) const USER_GAP: f32 = 14.0;

// ── The terminal field ──────────────────────────────────────────────────
const PROMPT_SHARE: f32 = 0.42;
const CELL_SHARE: f32 = 0.2;
const CELL_GAP: f32 = 6.0;
const CARET_SHARE: f32 = 0.42;
const EMPTY_DOT: f32 = 2.0;
const EMPTY_ALPHA: f32 = 0.2;
const FIELD_PAD_START: f32 = 14.0;
const FIELD_PAD_END: f32 = 8.0;
const FIELD_GAP: f32 = 10.0;
const KEYCAP_W: f32 = 30.0;
const KEYCAP_H: f32 = 22.0;
const FOCUS_BAR: f32 = 2.0;
/// The caret blinks this often (ms per half period).
const BLINK_MS: u128 = 530;
/// A wrong password flashes the field for this long (s).
const FLASH_S: f32 = 0.9;

/// One device pixel or more, in logical pixels (crisp at any scale).
pub(super) fn hairline(scale: f32) -> f32 {
    scale.floor().max(1.0) / scale.max(0.01)
}

fn snap(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale.max(0.01)
}

fn ink((l, c, h): (f32, f32, f32), alpha: f32) -> Color {
    Color::oklcha(l, c, h, alpha)
}

/// The accent, kept light enough to read on graphite.
fn lit(c: Color) -> Color {
    c.with_lightness(c.lightness().max(ACCENT_LIGHTNESS))
}

/// A div with a `w`-px border on every side.
fn bordered(el: gpui::Div, w: f32) -> gpui::Div {
    let mut el = el;
    let w: gpui::AbsoluteLength = px(w).into();
    let widths = &mut el.style().border_widths;
    (widths.top, widths.right, widths.bottom, widths.left) = (Some(w), Some(w), Some(w), Some(w));
    el
}

/// The terminal field's graphite.
pub(super) fn field_fill(focused: bool) -> Hsla {
    ink(FIELD_INK, if focused { 0.97 } else { 0.92 }).hsla()
}

/// Whether `cfg` shows readouts or the look's labels (and so needs a
/// [`Telemetry`]).
pub(super) fn shows_telemetry(cfg: &GreeterConfig) -> bool {
    let card = cfg.card.enabled && cfg.card.style != CardStyle::Plain && cfg.placement(Element::User).visible;
    card || cfg.clock.face == ClockFace::Ruler || cfg.password.submit == SubmitButton::Pill
}

/// The readouts and the looks' labels, in the current language.
pub(super) struct Telemetry {
    lang: mandara_i18n::Lang,
    /// The local minute (Unix minutes) the readouts were made for.
    minute: i64,
    /// Uptime when first read, and when (advanced from there).
    boot: Option<(u64, Instant)>,
    pub host: SharedString,
    pub uptime: Option<SharedString>,
    pub zone: SharedString,
    pub sign_in: SharedString,
    pub welcome: SharedString,
    pub keyboard: SharedString,
    pub uptime_label: SharedString,
    pub zone_label: SharedString,
    pub password: SharedString,
}

impl Telemetry {
    /// Brings `slot` up to date for `now`: labels per language, readouts
    /// per minute; the host name and the uptime are read once.
    pub fn refresh(slot: &mut Option<Self>, now: &chrono::DateTime<chrono::Local>) {
        let lang = mandara_i18n::language();
        let minute = now.timestamp().div_euclid(60);
        let t = slot.get_or_insert_with(|| Self {
            lang,
            minute: i64::MIN,
            boot: mandara_ui::widgets::system::uptime_secs().map(|s| (s, Instant::now())),
            host: SharedString::from(mandara_ui::widgets::system::hostname().trim().to_string()),
            uptime: None,
            zone: SharedString::default(),
            sign_in: SharedString::default(),
            welcome: SharedString::default(),
            keyboard: SharedString::default(),
            uptime_label: SharedString::default(),
            zone_label: SharedString::default(),
            password: SharedString::default(),
        });
        if t.minute == i64::MIN || t.lang != lang {
            t.lang = lang;
            t.sign_in = tl("Sign in").into();
            t.welcome = tl("Welcome back").into();
            t.keyboard = tl("Keyboard").to_uppercase().into();
            t.uptime_label = tl("Uptime").to_uppercase().into();
            t.zone_label = tl("Time zone").to_uppercase().into();
            t.password = tl("Password").to_uppercase().into();
            t.minute = i64::MIN + 1;
        }
        if t.minute != minute {
            t.minute = minute;
            t.uptime = t.boot.map(|(secs, at)| mandara_ui::widgets::system::format_uptime(secs + at.elapsed().as_secs()).into());
            t.zone = zone_label(now.offset().local_minus_utc()).into();
        }
    }
}

/// `UTC+02:00`, `UTC−05:30`, `UTC` for an offset in seconds east of UTC.
pub(super) fn zone_label(offset_s: i32) -> String {
    if offset_s == 0 {
        return "UTC".into();
    }
    let sign = if offset_s < 0 { '−' } else { '+' };
    let m = offset_s.unsigned_abs() / 60;
    format!("UTC{sign}{:02}:{:02}", m / 60, m % 60)
}

/// The measured frame over the backdrop.
pub(super) fn hud_overlay(f: &Frame, p: &MandaraPalette) -> AnyElement {
    let (vw, vh, u, s) = (f.vw, f.vh, f.u, f.scale);
    let hair = hairline(s);
    let white = f.white;
    let mut root = div().absolute().top_0().left_0().w(px(vw)).h(px(vh));
    // The grid.
    let step = GRID_STEP * u;
    let grid = white.opacity(GRID_ALPHA);
    let mut x = step;
    while x < vw {
        root = root.child(div().absolute().left(px(snap(x, s))).top_0().w(px(hair)).h(px(vh)).bg(grid));
        x += step;
    }
    let mut y = step;
    while y < vh {
        root = root.child(div().absolute().top(px(snap(y, s))).left_0().h(px(hair)).w(px(vw)).bg(grid));
        y += step;
    }
    // Corner brackets.
    let (m, arm) = (snap(FRAME_INSET * u, s), snap(BRACKET_ARM * u, s));
    let bracket = white.opacity(BRACKET_ALPHA);
    for (left, top) in [(true, true), (false, true), (true, false), (false, false)] {
        let h = div().absolute().w(px(arm)).h(px(hair)).bg(bracket);
        let v = div().absolute().w(px(hair)).h(px(arm)).bg(bracket);
        let place = |el: gpui::Div| {
            let el = if left { el.left(px(m)) } else { el.right(px(m)) };
            if top { el.top(px(m)) } else { el.bottom(px(m)) }
        };
        root = root.child(place(h)).child(place(v));
    }
    // The left ruler (ticks pointing in) and the bottom one (pointing up).
    let tick = |major: bool| if major { TICK_MAJOR } else { TICK_MINOR };
    let pitch = TICK_STEP * u;
    let start = m + arm + RULER_CLEARANCE * u;
    let mut i = 0;
    let mut y = start;
    while y < vh - start {
        let (len, alpha) = tick(i % MAJOR_EVERY == 0);
        root = root.child(div().absolute().left(px(m)).top(px(snap(y, s))).w(px(snap(len * u, s))).h(px(hair)).bg(white.opacity(alpha)));
        y += pitch;
        i += 1;
    }
    let (mut i, mut x) = (0, start);
    while x < vw * BOTTOM_RULER_SHARE {
        let (len, alpha) = tick(i % MAJOR_EVERY == 0);
        let len = snap(len * u, s);
        root = root.child(div().absolute().left(px(snap(x, s))).top(px(snap(vh - m - len, s))).w(px(hair)).h(px(len)).bg(white.opacity(alpha)));
        x += pitch;
        i += 1;
    }
    // The scan line sweeps down; still with reduced motion (it is left out).
    if !f.still {
        let trail = vh * SCAN_TRAIL;
        let phase = (f.t / SCAN_PERIOD_S).fract();
        let top = phase * (vh + trail) - trail;
        let cyan = p.primary;
        root = root
            .child(div().absolute().left_0().top(px(top)).w(px(vw)).h(px(trail)).bg(oklab_gradient(180.0, cyan.with_alpha(0.0), cyan.with_alpha(SCAN_TRAIL_ALPHA))))
            .child(div().absolute().left_0().top(px(snap(top + trail, s))).w(px(vw)).h(px(hair)).bg(cyan.with_alpha(SCAN_LINE_ALPHA).hsla()));
    }
    root.into_any_element()
}

#[cfg(test)]
/// The ruler clock's reference size (awake, at scale 1): width and height.
pub(super) fn ruler_clock_size(cfg: &GreeterConfig) -> (f32, f32) {
    let c = &cfg.clock;
    let digits = if c.seconds { 8 } else { 5 };
    let (cols, lines) = if c.stacked { (2.0, if c.seconds { 3.0 } else { 2.0 }) } else { (digits as f32, 1.0) };
    let line = c.size * if c.stacked { STACKED_LINE } else { 1.08 };
    let date = if cfg.placement(Element::Date).visible { DATE_GAP + cfg.date.size * DATE_LINE } else { 0.0 };
    (RULER_W + RULER_GAP + cols * MONO_ADVANCE * c.size, lines * line + date)
}

#[cfg(test)]
/// The wireframe card's reference height (scale 1).
pub(super) fn hud_card_height(cfg: &GreeterConfig, manual: bool) -> f32 {
    let c = &cfg.card;
    let field = cfg.password.height;
    let username = if manual { field + USERNAME_GAP } else { 0.0 };
    let session = if c.session_inside && cfg.placement(Element::Session).visible { SESSION_GAP + field * SESSION_SHARE } else { 0.0 };
    let top = HEADER_H + SECTION_GAP + TICKS_H + SECTION_GAP + READOUTS_H + BLOCK_GAP;
    let user = cfg.user.avatar_size.max(cfg.user.name_size * 1.3 + 18.0) + BLOCK_GAP;
    2.0 * c.padding + top + user + LABEL_H + LABEL_GAP + username + field + STATUS_TOP + STATUS_H + session
}

/// A small light of `color`.
fn led(d: f32, color: Hsla) -> gpui::Div {
    div().flex_none().size(px(d)).rounded(px(d * 0.2)).bg(color).shadow(vec![glow(color.opacity(0.55), d * 1.6, 0.0)])
}

/// The status line under the terminal and recessed fields: a small light
/// (coral for errors, amber for Caps Lock, the accent otherwise) and the
/// message, from the field's start edge.
pub(super) fn led_status(f: &Frame, k: f32, width: f32, status: Option<(Option<&'static str>, String, Hsla)>, error: bool, p: &MandaraPalette) -> AnyElement {
    div()
        .mt(px(STATUS_TOP * k))
        .h(px(STATUS_H * k))
        .w(px(width))
        .flex()
        .items_center()
        .gap(px(8.0 * k))
        .when(f.rtl, |el| el.flex_row_reverse())
        .text_size(px(12.5 * k))
        .when_some(status, |el, (ic, text, color)| {
            let light = if error {
                p.urgent_coral.with_lightness(p.urgent_coral.lightness().max(0.72)).hsla()
            } else if ic.is_some() {
                p.warning_amber.with_lightness(p.warning_amber.lightness().max(0.78)).hsla()
            } else {
                lit(p.primary).hsla()
            };
            el.child(led(LED * k, light)).child(div().truncate().text_color(color).child(text))
        })
        .into_any_element()
}

impl GreeterView {
    /// How far a wrong-password flash still shows (1 → 0).
    pub(super) fn flash_amount(&self, f: &Frame) -> f32 {
        self.flash_at.map_or(0.0, |at| (1.0 - f.now.saturating_duration_since(at).as_secs_f32() / FLASH_S).max(0.0))
    }

    /// Monospace digits stacked beside a ruler of the day, the date and
    /// the time zone under them.
    pub(super) fn ruler_clock(&self, f: &Frame, k: f32) -> AnyElement {
        let c = &self.config.clock;
        let p = &self.palette;
        let idle = 1.0 - self.wake.value().clamp(0.0, 1.0);
        let size = snap_size(c.size * k * (1.0 + CLOCK_IDLE_GROWTH * idle) * (1.0 + CLOCK_SUCCESS_GROWTH * f.success));
        let lines = if c.stacked { if c.seconds { 3.0 } else { 2.0 } } else { 1.0 };
        let height = lines * size * if c.stacked { STACKED_LINE } else { 1.08 };
        let ruler = self.day_ruler(f, k, height, p);
        let face = div().flex().items_start().gap(px(RULER_GAP * k)).child(ruler).child(self.clock_el(f, k));
        let d = &self.config.date;
        let date = self.config.placement(Element::Date).visible.then(|| {
            let text = self.date_text.as_ref().map(|(_, t)| t.clone()).unwrap_or_default();
            let font = match d.font.trim() {
                "" if f.rtl => f.ui_font.clone(),
                "" => f.mono_font.clone(),
                family => SharedString::from(family.to_string()),
            };
            let zone = self.telemetry.as_ref().map(|t| t.zone.clone()).filter(|z| !z.is_empty());
            div()
                .mt(px(DATE_GAP * k))
                .flex()
                .items_center()
                .gap(px(10.0 * k))
                .when(f.rtl, |el| el.flex_row_reverse())
                .font_family(font)
                .text_size(px(d.size * k))
                .line_height(px(d.size * k * DATE_LINE))
                .font_weight(super::weight(d.weight))
                .child(div().text_color(f.white.opacity(d.opacity)).child(text))
                .when_some(zone, |el, zone| {
                    el.child(div().size(px(3.0 * k)).bg(lit(p.primary).hsla())).child(div().font_family(f.mono_font.clone()).text_color(f.white.opacity(LABEL_ALPHA.max(d.opacity * 0.85))).child(zone))
                })
        });
        div().flex().flex_col().items_end().child(face).children(date).into_any_element()
    }

    /// The day's ruler, `height` tall: half-hour ticks, labels every six
    /// hours and a marker at the time.
    fn day_ruler(&self, f: &Frame, k: f32, height: f32, p: &MandaraPalette) -> AnyElement {
        let s = f.scale;
        let hair = hairline(s);
        let w = RULER_W * k;
        let label_h = RULER_LABEL_PX * k * 1.2;
        let mut el = div().relative().flex_none().w(px(w)).h(px(height));
        for i in 0..=DAY_TICKS {
            let y = snap(i as f32 / DAY_TICKS as f32 * (height - hair), s);
            let kind = if i % TICKS_PER_LABEL == 0 { 2 } else if i % 2 == 0 { 1 } else { 0 };
            let (len, alpha) = DAY_TICK[kind];
            el = el.child(div().absolute().right_0().top(px(y)).w(px(snap(len * k, s))).h(px(hair)).bg(f.white.opacity(alpha)));
            if kind == 2 {
                let top = (y - label_h / 2.0).clamp(0.0, (height - label_h).max(0.0));
                el = el.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(top))
                        .h(px(label_h))
                        .flex()
                        .items_center()
                        .font_family(f.mono_font.clone())
                        .font_features(tabular_nums())
                        .text_size(px(RULER_LABEL_PX * k))
                        .text_color(f.white.opacity(LABEL_ALPHA))
                        .child(SharedString::new_static(DAY_LABELS[i / TICKS_PER_LABEL])),
                );
            }
        }
        let y = snap(f32::from(self.day_minute) / MINUTES_PER_DAY * (height - MARKER_H * k), s);
        let marker = lit(p.tertiary).hsla();
        el.child(div().absolute().right_0().top(px(y)).w(px(MARKER_W * k)).h(px(snap(MARKER_H * k, s).max(hair))).bg(marker).shadow(vec![glow(marker.opacity(0.5), 8.0 * k, 0.0)])).into_any_element()
    }

    /// The bracketed wireframe card.
    pub(super) fn hud_card(&self, f: &Frame, k: f32, caps: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = &self.config.card;
        let p = &self.palette;
        let s = f.scale;
        let hair = hairline(s);
        let w = c.width * k;
        let pad = c.padding * k;
        let inner = w - 2.0 * pad;
        let white = f.white;
        let accent = lit(p.primary);
        let fill = ink(PANEL_INK, if f.high_contrast { 0.98 } else { c.opacity.clamp(0.0, 1.0) });
        let edge = white.opacity(if c.border > 0.0 { HAIRLINE_ALPHA } else { 0.0 });
        let label = |text: SharedString| {
            div().font_family(if f.rtl { f.ui_font.clone() } else { f.mono_font.clone() }).text_size(px(LABEL_PX * k)).text_color(white.opacity(LABEL_ALPHA)).child(text)
        };
        let tel = self.telemetry.as_ref();
        let mut card = bordered(div(), hair).relative().w(px(w)).p(px(pad)).rounded(px(c.radius * k)).bg(fill.hsla()).border_color(edge).shadow(vec![soft_shadow(60.0 * k, 0.5, 24.0 * k), glow(accent.with_alpha(0.05).hsla(), 80.0 * k, 0.0)]).flex().flex_col();
        // Brackets on the corners.
        let arm = snap(BRACKET_PANEL * k, s);
        let bracket = accent.with_alpha(0.9).hsla();
        for (left, top) in [(true, true), (false, true), (true, false), (false, false)] {
            let place = |el: gpui::Div| {
                let el = if left { el.left(px(-hair)) } else { el.right(px(-hair)) };
                if top { el.top(px(-hair)) } else { el.bottom(px(-hair)) }
            };
            card = card.child(place(div().absolute().w(px(arm)).h(px(hair)).bg(bracket))).child(place(div().absolute().w(px(hair)).h(px(arm)).bg(bracket)));
        }
        // Header: a light, "Sign in", the host name, the way back.
        let switcher = self.users.len() > 1 || self.manual.is_some();
        let back = (c.back_button && switcher && !self.is_preview()).then(|| {
            bordered(div(), hair)
                .id("card-back")
                .flex_none()
                .size(px(BACK_PX * k))
                .rounded(px(2.0 * k))
                .border_color(white.opacity(0.22))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|st| st.bg(gpui::white().opacity(0.08)))
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.field = Field::User;
                    this.activity();
                    cx.notify();
                }))
                .child(icon(if f.rtl { "arrow-right" } else { "arrow-left" }, BACK_PX * k * 0.5, white.opacity(0.9)))
        });
        let host = tel.map(|t| t.host.clone()).filter(|h| !h.is_empty());
        let header = div()
            .h(px(HEADER_H * k))
            .flex()
            .items_center()
            .gap(px(8.0 * k))
            .when(f.rtl, |el| el.flex_row_reverse())
            .child(led(LED * k, lit(p.tertiary).hsla()))
            .child(label(tel.map(|t| t.sign_in.clone()).unwrap_or_default()).text_color(white.opacity(0.8)))
            .child(div().flex_1())
            .when_some(host, |el, host| el.child(label(host).font_family(f.mono_font.clone()).max_w(px(inner * 0.5)).truncate()))
            .children(back);
        // A ruled scale across the panel.
        let pitch = PANEL_TICK * k;
        let count = (inner / pitch).floor() as usize + 1;
        let mut ticks = div().relative().h(px(TICKS_H * k)).w(px(inner));
        for i in 0..count {
            let major = i % MAJOR_EVERY == 0;
            let len = snap(if major { TICKS_H } else { TICKS_H / 2.0 } * k, s);
            let x = snap(i as f32 * pitch, s).min(inner - hair);
            let el = div().absolute().bottom_0().w(px(hair)).h(px(len)).bg(white.opacity(if major { 0.38 } else { 0.18 }));
            ticks = ticks.child(if f.rtl { el.right(px(x)) } else { el.left(px(x)) });
        }
        // Live readouts: the keyboard layout, uptime and time zone.
        let mut readouts = div().h(px(READOUTS_H * k)).flex().items_center().gap(px(READOUT_GAP * k)).when(f.rtl, |el| el.flex_row_reverse());
        if let Some(t) = tel {
            let cells = [(t.keyboard.clone(), Some(f.keyboard.clone())), (t.uptime_label.clone(), t.uptime.clone()), (t.zone_label.clone(), Some(t.zone.clone()).filter(|z| !z.is_empty()))];
            let mut first = true;
            for (name, value) in cells {
                let Some(value) = value else { continue };
                if !first {
                    readouts = readouts.child(div().w(px(hair)).h(px(READOUTS_H * k * 0.7)).bg(white.opacity(HAIRLINE_ALPHA)));
                }
                first = false;
                readouts = readouts.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.0 * k))
                        .when(f.rtl, |el| el.items_end())
                        .child(div().font_family(if f.rtl { f.ui_font.clone() } else { f.mono_font.clone() }).text_size(px(READOUT_LABEL_PX * k)).text_color(white.opacity(LABEL_ALPHA)).child(name))
                        .child(div().font_family(f.mono_font.clone()).font_features(tabular_nums()).text_size(px(READOUT_VALUE_PX * k)).text_color(white.opacity(VALUE_ALPHA)).child(value)),
                );
            }
        }
        let field_label = div()
            .h(px(LABEL_H * k))
            .flex()
            .items_center()
            .when(f.rtl, |el| el.flex_row_reverse())
            .child(label(tel.map(|t| t.password.clone()).unwrap_or_default()))
            .child(div().flex_1())
            .child(div().h(px(hair)).w(px(inner * 0.5)).bg(white.opacity(HAIRLINE_ALPHA)));
        card = card
            .child(header)
            .child(div().h(px(SECTION_GAP * k)))
            .child(ticks)
            .child(div().h(px(SECTION_GAP * k)))
            .child(readouts)
            .child(div().h(px(BLOCK_GAP * k)))
            .child(self.user_row(f, k, true, cx))
            .child(div().h(px(BLOCK_GAP * k)))
            .child(field_label)
            .child(div().h(px(LABEL_GAP * k)))
            .child(div().flex().flex_col().when(f.rtl, |el| el.items_end()).child(self.password_el(f, k, caps, cx)));
        if c.session_inside && !self.sessions.is_empty() && self.config.placement(Element::Session).visible {
            card = card.child(div().h(px(SESSION_GAP * k))).child(self.session_el(f, k, cx));
        }
        card.into_any_element()
    }

    /// The avatar beside the name and the login name (`mono`: the login in
    /// monospace), clickable to switch people.
    pub(super) fn user_row(&self, f: &Frame, k: f32, mono: bool, cx: &mut Context<Self>) -> AnyElement {
        let u = &self.config.user;
        let entry = if self.manual.is_none() { self.users.get(self.user_ix) } else { None };
        let d = u.avatar_size * k * self.pop.value().max(0.0);
        let (name, login): (SharedString, Option<SharedString>) = match (&self.manual, entry) {
            (None, Some(e)) => (e.user.display.clone().into(), (e.user.name != e.user.display).then(|| SharedString::from(e.user.name.clone()))),
            _ if self.is_preview() => (tl("Your Name").into(), None),
            _ => (tl("Other user").into(), None),
        };
        let clickable = !self.is_preview() && (self.users.len() > 1 || !self.demo());
        let avatar = div()
            .id("avatar")
            .flex_none()
            .size(px(u.avatar_size * k))
            .flex()
            .items_center()
            .justify_center()
            .when(clickable, |el| {
                el.cursor_pointer().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.field = Field::User;
                    this.activity();
                    cx.notify();
                }))
            })
            .child(self.avatar_el(entry, d, f));
        div()
            .h(px(u.avatar_size.max(u.name_size * 1.3 + 18.0) * k))
            .flex()
            .items_center()
            .gap(px(USER_GAP * k))
            .when(f.rtl, |el| el.flex_row_reverse())
            .child(avatar)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.0 * k))
                    .when(f.rtl, |el| el.items_end())
                    .when(u.show_name, |el| el.child(div().truncate().text_size(px(u.name_size * k)).font_weight(FontWeight::SEMIBOLD).text_color(f.white.opacity(0.96)).child(name)))
                    .when_some(login, |el, login| {
                        el.child(div().truncate().when(mono, |el| el.font_family(f.mono_font.clone())).text_size(px(12.0 * k)).text_color(f.white.opacity(LABEL_ALPHA)).child(login))
                    }),
            )
            .into_any_element()
    }

    /// The terminal line: a prompt, one cell per character (or the text,
    /// for a name or a revealed password) and a block caret; the reveal
    /// and sign-in keys at its end.
    pub(super) fn terminal_field(&self, f: &Frame, k: f32, id: &'static str, text: String, secret: bool, placeholder: String, focused: bool, unfold: f32, cx: &mut Context<Self>) -> AnyElement {
        let pc = &self.config.password;
        let p = &self.palette;
        let s = f.scale;
        let hair = hairline(s);
        let white = f.white;
        let accent = lit(p.primary);
        let h = pc.height * k;
        let is_pw = id == "password";
        let beside = is_pw && pc.submit == SubmitButton::Beside;
        let full_w = self.field_width(k) - if beside { h + BESIDE_GAP * k } else { 0.0 };
        let w = h + (full_w - h) * unfold.clamp(0.0, 1.0);
        let busy = self.phase == Phase::Busy && is_pw;
        let empty = text.is_empty();
        let blink = f.reduced || (self.started.elapsed().as_millis() / BLINK_MS).is_multiple_of(2) || self.last_key.elapsed() < CARET_HOLD;
        let caret_on = focused && !busy && !self.is_preview() && blink;
        let flash = if is_pw { self.flash_amount(f) } else { 0.0 };
        let coral = p.urgent_coral.with_lightness(p.urgent_coral.lightness().max(0.7));
        let edge = if flash > 0.0 {
            coral.with_alpha(0.4 + 0.6 * flash).hsla()
        } else if focused && pc.accent_border {
            accent.with_alpha(0.75).hsla()
        } else {
            white.opacity(if focused { 0.3 } else { HAIRLINE_ALPHA })
        };
        let mut fill = field_fill(focused);
        if flash > 0.0 {
            fill = super::blend(fill, coral.with_alpha(0.22 * flash).hsla());
        }
        let field = if is_pw { Field::Password } else { Field::Username };
        let field_font = if f.rtl && !secret { f.ui_font.clone() } else { f.mono_font.clone() };
        // Room between the prompt and the keys at the end.
        let reveal = is_pw && pc.reveal_toggle && !empty && !busy;
        let keycap = is_pw && pc.submit == SubmitButton::Inside && !busy;
        let trail_w = if busy { h * 0.62 } else { f32::from(u8::from(reveal)) * h * 0.62 + f32::from(u8::from(keycap)) * (KEYCAP_W + 4.0) * k };
        let prompt_w = h * PROMPT_SHARE;
        let room = (w - (FIELD_PAD_START + FIELD_PAD_END) * k - prompt_w - 2.0 * FIELD_GAP * k - trail_w).max(0.0);
        let block = |el: gpui::Div| el.flex_none().w(px(snap(h * CELL_SHARE, s))).h(px(snap(h * CARET_SHARE, s))).bg(accent.hsla());
        let content: AnyElement = if secret && !empty && !(self.reveal && is_pw) {
            self.cells(f, k, h, room, text.chars().count(), caret_on)
        } else {
            let caret = caret_on.then(|| block(div()));
            let line = div().flex_1().min_w_0().h(px(h)).flex().items_center().gap(px(4.0 * k)).overflow_hidden().font_family(field_font).text_size(px(pc.text_size * k)).when(f.rtl, |el| el.flex_row_reverse());
            if empty {
                line.children(caret).child(div().truncate().text_color(white.opacity(LABEL_ALPHA)).child(placeholder)).into_any_element()
            } else {
                line.child(div().truncate().text_color(white.opacity(0.96)).child(text)).children(caret).into_any_element()
            }
        };
        let trailing: Option<AnyElement> = if busy {
            Some(scan(h * 0.62, k, self.started.elapsed().as_secs_f32(), accent.hsla(), f.still))
        } else if reveal || keycap {
            let reveal_el = reveal.then(|| {
                div()
                    .id("reveal")
                    .size(px(h * 0.62))
                    .rounded(px(2.0 * k))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|st| st.bg(gpui::white().opacity(0.08)))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.reveal = !this.reveal;
                        cx.notify();
                    }))
                    .child(icon(if self.reveal { "eye-off" } else { "eye" }, h * 0.34, white.opacity(0.8)))
            });
            let key_ink = if empty { white.opacity(0.4) } else { accent.hsla() };
            let keycap_el = keycap.then(|| {
                bordered(div(), hair)
                    .id("submit")
                    .w(px(KEYCAP_W * k))
                    .h(px(KEYCAP_H * k))
                    .rounded(px(2.0 * k))
                    .border_color(if empty { white.opacity(0.22) } else { accent.with_alpha(0.7).hsla() })
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|st| st.bg(gpui::white().opacity(0.08)))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.submit(cx)))
                    .child(icon(if f.rtl { "arrow-left" } else { "arrow-right" }, KEYCAP_H * k * 0.55, key_ink))
            });
            Some(div().flex().items_center().gap(px(4.0 * k)).when(f.rtl, |el| el.flex_row_reverse()).children(reveal_el).children(keycap_el).into_any_element())
        } else {
            None
        };
        let prompt = div()
            .flex_none()
            .w(px(prompt_w))
            .flex()
            .items_center()
            .justify_center()
            .font_family(f.mono_font.clone())
            .text_size(px(h * PROMPT_SHARE))
            .text_color(if focused { accent.hsla() } else { white.opacity(LABEL_ALPHA) })
            .child(SharedString::new_static(if f.rtl { "‹" } else { "›" }));
        let shake = if is_pw { self.shake.value() * f.u } else { 0.0 };
        let bar = focused.then(|| {
            let el = div().absolute().top_0().bottom_0().w(px(snap(FOCUS_BAR * k, s).max(hair))).bg(if flash > 0.0 { coral.hsla() } else { accent.hsla() });
            if f.rtl { el.right_0() } else { el.left_0() }
        });
        bordered(div(), hair)
            .id(id)
            .relative()
            .ml(px(shake))
            .w(px(w))
            .h(px(h))
            .rounded(px(2.0 * k * self.config.theme.roundness.max(0.5)))
            .bg(fill)
            .border_color(edge)
            .when(!f.rtl, |el| el.pl(px(FIELD_PAD_START * k)).pr(px(FIELD_PAD_END * k)))
            .when(f.rtl, |el| el.pr(px(FIELD_PAD_START * k)).pl(px(FIELD_PAD_END * k)).flex_row_reverse())
            .flex()
            .items_center()
            .gap(px(FIELD_GAP * k))
            .cursor_text()
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.field = field;
                this.session_menu = false;
                this.activity();
                cx.notify();
            }))
            .children(bar)
            .when(unfold > 0.6, |el| el.child(prompt).child(content).child(div().flex_1()).children(trailing))
            .into_any_element()
    }

    /// One cell per typed character in fixed slots from the start edge,
    /// the caret in the next one, faint dots in the rest; a full row
    /// scrolls one slot per character.
    fn cells(&self, f: &Frame, k: f32, h: f32, room: f32, n: usize, caret_on: bool) -> AnyElement {
        let pc = &self.config.password;
        let s = f.scale;
        let cell = snap(h * CELL_SHARE, s);
        let slot = snap(cell + CELL_GAP * k, s);
        let slots = (((room + CELL_GAP * k) / slot).floor() as usize).max(1);
        let first = (n + 1).saturating_sub(slots);
        let animate = pc.keystroke_ripple && !f.reduced;
        let born = self.dots.len().saturating_sub(n);
        let ink = f.white.opacity(0.94);
        let accent = lit(self.palette.primary).hsla();
        let caret_h = snap(h * CARET_SHARE, s);
        let dot = snap(EMPTY_DOT * k, s).max(hairline(s));
        let mut el = div().relative().flex_none().w(px(room)).h(px(h));
        for i in 0..slots {
            let ix = first + i;
            let x = i as f32 * slot;
            let at = |child: gpui::Div, size: f32| {
                let child = child.absolute().top(px(snap((h - size) / 2.0, s)));
                if f.rtl { child.right(px(x)) } else { child.left(px(x)) }
            };
            el = if ix < n {
                let shape = if pc.mask == PasswordMask::Dots { Symbol::Square } else { symbols::symbol_at(self.mask_seed, ix) };
                let (scale, opacity) = self.dots.get(born + ix).map_or((1.0, 1.0), |at| symbols::appear(f.now.saturating_duration_since(*at), animate));
                el.child(at(div().size(px(cell)).flex().items_center().justify_center(), cell).child(svg().path(shape.asset_path()).size(px(cell)).flex_none().text_color(ink.opacity(opacity)).with_transformation(Transformation::scale(gpui::size(scale, scale)))))
            } else if ix == n && caret_on {
                el.child(at(div().w(px(cell)).h(px(caret_h)).bg(accent), caret_h))
            } else {
                el.child(at(div().w(px(cell)).h(px(dot)).flex().justify_center(), dot).child(div().size(px(dot)).bg(f.white.opacity(EMPTY_ALPHA))))
            };
        }
        el.into_any_element()
    }
}

/// Three squares lighting in turn (checking); lit and still with reduced
/// motion.
fn scan(size: f32, k: f32, t: f32, color: Hsla, still: bool) -> AnyElement {
    let mut row = div().w(px(size * 1.4)).h(px(size)).flex().items_center().justify_center().gap(px(3.0 * k));
    for i in 0..3 {
        let phase = if still { 1.0 } else { (t * 5.0 - i as f32 * 0.7).sin() * 0.5 + 0.5 };
        row = row.child(div().size(px(5.0 * k)).bg(color.opacity(0.3 + 0.7 * phase)));
    }
    row.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_zones_read_like_utc_offsets() {
        assert_eq!(zone_label(0), "UTC");
        assert_eq!(zone_label(7200), "UTC+02:00");
        assert_eq!(zone_label(-(5 * 3600 + 30 * 60)), "UTC−05:30");
        assert_eq!(zone_label(20_700), "UTC+05:45");
    }

    #[test]
    fn hairlines_are_whole_device_pixels() {
        assert_eq!(hairline(1.0), 1.0);
        assert!((hairline(1.5) * 1.5 - 1.0).abs() < 1e-6);
        assert_eq!(hairline(2.0), 1.0);
    }

    /// Labels, readouts and the date stay readable (WCAG AA, 4.5:1) on
    /// the graphite panel and ground.
    #[test]
    fn ghostline_text_meets_aa_contrast() {
        let ground = Color::from_hex("#0b0d10").unwrap();
        let panel = ink(PANEL_INK, 0.86).over(ground);
        let field = ink(FIELD_INK, 0.92).over(panel);
        let white = |a: f32| Color::oklcha(1.0, 0.0, 0.0, a);
        for (what, alpha, on) in [("label on panel", LABEL_ALPHA, panel), ("value on panel", VALUE_ALPHA, panel), ("placeholder in field", LABEL_ALPHA, field), ("label on ground", LABEL_ALPHA, ground)] {
            let ratio = white(alpha).over(on).contrast_ratio(on);
            assert!(ratio >= 4.5, "{what}: {ratio:.2}");
        }
        let mut cfg = GreeterConfig::default();
        cfg.apply_look("ghostline");
        let accent = lit(crate::view::palette_for(&cfg).primary);
        assert!(accent.contrast_ratio(field) >= 4.5, "the accent on the field");
        assert!(white(cfg.date.opacity).over(ground).contrast_ratio(ground) >= 4.5, "the date");
    }
}
