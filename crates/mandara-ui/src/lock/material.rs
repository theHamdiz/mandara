//! The lock screen's Material You design (`lock.design = "material-you"`).
//!
//! Material 3 Expressive on the software canvas: colours made from the
//! wallpaper (Material's tonal scheme from its leading colour, picked when
//! the wallpaper is decoded), a big stacked clock whose variable weight
//! grows from thin as the screen appears, the weather, the track and the
//! notification count as tonal chips, and one tonal card holding the
//! scalloped avatar, the name and a filled password field with a tonal
//! button beside it. The avatar turns a little with each keystroke and
//! morphs into a circle while the password is checked; the button squares
//! off at the same time and shows Material's loading indicator. Corner
//! sizes are Material's (32 px cards, 16 px fields, 8 px chips) and the
//! springs are its expressive motion tokens; with reduced motion every
//! spring snaps.
//!
//! The Mandara design keeps its own painter in `mod.rs`; nothing here runs
//! for it.

use std::time::Instant;

use mandara_core::password_symbols::{self as symbols, Row, Symbol};
use mandara_core::session_lock::SurfaceInfo;
use mandara_core::{SpringConfig, avatar_shape};
use mandara_i18n::t;
use mandara_theme::{Color, MandaraPalette};
use tiny_skia::PixmapMut;

use super::paint::{self, Font};
use super::{L, LockScreen, Note, Phase, initials};
use crate::components::motion::Feel;
use crate::components::tonal::Tonal;
use crate::components::{ButtonDesign, InputDesign, buttons, inputs};

/// Each keystroke turns the avatar this far (radians).
pub(super) const SPIN_PER_KEY: f32 = std::f32::consts::PI / 12.0;
/// How far toward a circle the avatar morphs while checking.
pub(super) const MORPH_CHECKING: f32 = 0.6;

// Layout, logical px on a 1080 px tall screen (scaled like the Mandara
// design's, `login_layout`).
const DATE_TOP: f32 = 64.0;
const DATE_SIZE: f32 = 20.0;
const CLOCK_TOP: f32 = 96.0;
const CLOCK_SIZE: f32 = 132.0;
/// Stacked lines, as a share of the digits' size.
const CLOCK_LINE: f32 = 0.98;
/// The clock's weight: thin as the screen appears, regular once shown,
/// in steps (each weight is shaped once).
const CLOCK_WEIGHT: (f32, f32) = (250.0, 500.0);
const WEIGHT_STEP: f32 = 50.0;
const CHIP_H: f32 = 32.0;
const CHIP_TEXT: f32 = 13.0;
const CHIP_PAD: f32 = 12.0;
const CHIP_GAP: f32 = 8.0;
const CHIP_RADIUS: f32 = 8.0;
/// The info chips' tonal surface over the wallpaper.
const CHIP_ALPHA: f32 = 0.85;
const CARD_CENTER: f32 = 170.0;
const CARD_W: f32 = 400.0;
const CARD_PAD: f32 = 28.0;
const CARD_RADIUS: f32 = 32.0;
const CARD_ALPHA: f32 = 0.92;
const AVATAR: f32 = 104.0;
/// A picture inside the scalloped shape, share of its size.
const AVATAR_INNER: f32 = 0.84;
const NAME_SIZE: f32 = 20.0;
const NAME_LINE: f32 = 26.0;
const GAP: f32 = 12.0;
const FIELD_GAP: f32 = 20.0;
const FIELD_H: f32 = 56.0;
const FIELD_RADIUS: f32 = 16.0;
const FIELD_PAD: f32 = 16.0;
const FIELD_ALPHA: f32 = 0.96;
/// The password symbols' text size in the field.
const SYMBOL_TEXT: f32 = 20.0;
const BUTTON_GAP: f32 = 8.0;
const LABEL_SIZE: f32 = 11.5;
const SUPPORT_H: f32 = 24.0;
const SUPPORT_SIZE: f32 = 12.5;
const FOOT_MARGIN: f32 = 28.0;
/// The tinted hours' lightness on dark themes (a light tone of the
/// accent) and its ceiling on light themes.
const TINT_LIGHTNESS: (f32, f32) = (0.84, 0.45);
/// The loading indicator turns this fast (radians per second) and breathes
/// at this rate.
const LOADER_TURN: f32 = 4.0;
const LOADER_BREATH: f32 = 3.0;
/// The indicator's smallest size while it breathes, share of its largest.
const LOADER_REST: f32 = 0.8;

/// The share of the wallpaper (from the top) whose lightness sets the
/// extra scrim.
pub(super) const LUMA_SHARE: f32 = 0.5;

/// The extra scrim over a wallpaper this light (none for a dark one).
pub(super) fn extra_scrim(luma: Option<f32>) -> f32 {
    luma.map_or(0.0, crate::components::tonal::scrim_for_luma)
}

/// A Material motion token as a lock-screen spring.
pub(super) fn spring(feel: Feel) -> SpringConfig {
    let (ratio, stiffness) = feel.tokens();
    SpringConfig::new(stiffness, 2.0 * ratio * stiffness.sqrt(), 1.0)
}

/// The clock's weight `presence` of the way in, in steps.
pub(super) fn clock_weight(presence: f32) -> u16 {
    let w = CLOCK_WEIGHT.0 + (CLOCK_WEIGHT.1 - CLOCK_WEIGHT.0) * presence.clamp(0.0, 1.0);
    ((w / WEIGHT_STEP).round() * WEIGHT_STEP) as u16
}

/// The card's height (unscaled) with or without the avatar.
pub(super) fn card_height(avatar: bool) -> f32 {
    CARD_PAD * 2.0 + if avatar { AVATAR + GAP } else { 0.0 } + NAME_LINE + FIELD_GAP + FIELD_H + SUPPORT_H
}

/// Where the card sits on a `height`-tall screen (logical px): top and
/// bottom edges.
pub(super) fn card_span(height: f32, avatar: bool) -> (f32, f32) {
    let u = L::scale(height);
    let centre = height / 2.0 + CARD_CENTER * u;
    let h = card_height(avatar) * u;
    (centre - h / 2.0, centre + h / 2.0)
}

/// A rectangle whose top corners are rounded by `r` (Material's filled
/// field).
fn top_rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h).max(0.0);
    const K: f32 = 0.552_284_8;
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(x, y + h);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - r * K, x + r - r * K, y, x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + r * K, y, x + w, y + r - r * K, x + w, y + r);
    pb.line_to(x + w, y + h);
    pb.close();
    pb.finish()
}

/// A light (dark theme) or deep (light theme) tone of the accent.
fn tint(p: &MandaraPalette) -> Color {
    let l = p.primary.lightness();
    p.primary.with_lightness(if p.is_dark { l.max(TINT_LIGHTNESS.0) } else { l.min(TINT_LIGHTNESS.1) })
}

impl LockScreen {
    /// The Material You design over the backdrop and widgets.
    pub(super) fn draw_material(&mut self, info: &SurfaceInfo, pm: &mut PixmapMut<'_>, now: Instant) {
        let s = info.scale;
        let (fw, fh) = (pm.width() as f32, pm.height() as f32);
        let cx = fw / 2.0;
        let p = self.palette.clone();
        let tonal = Tonal::of(&p);
        let reveal = self.reveal.value().clamp(0.0, 1.2);
        let exit = self.exit.value().clamp(0.0, 1.0);
        let presence = (reveal.min(1.0) * (1.0 - exit)).clamp(0.0, 1.0);
        let alpha = if self.demo { 1.0 } else { presence };
        let lift = (1.0 - reveal) * 28.0 * s - exit * 22.0 * s;
        let unit = L::scale(info.height as f32) * s;

        // Date over a big stacked clock: hours in a tone of the accent,
        // minutes in the text colour; the weight grows as it appears.
        if self.look.cfg.show_date {
            let local = chrono::Local::now();
            let date = match &self.look.date_pattern {
                Some(pattern) => crate::widgets::clock::format_with(&local, pattern, t("%A, %-d %B")),
                None => crate::widgets::clock::localized(&local, t("%A, %-d %B")),
            };
            let font = Font { family: &self.fonts.ui, size: DATE_SIZE * unit, weight: 500, tabular: true, tracking: 0.0 };
            self.text.draw_centered(pm, &date, &font, cx, DATE_TOP * unit + lift, p.on_surface, alpha * 0.92);
        }
        let (time, ampm) = self.time_strings();
        let display = self.fonts.display.clone();
        let size = CLOCK_SIZE * unit;
        let clock = Font { family: &display, size, weight: clock_weight(if self.look.reduced_motion { 1.0 } else { presence }), tabular: true, tracking: -0.01 };
        let top = CLOCK_TOP * unit + lift;
        let mut bottom = top;
        match time.split_once(':') {
            Some((hours, minutes)) if !self.look.custom_time => {
                // Seconds (a custom pattern) stay on the minutes line.
                let (_, lh) = self.text.measure(hours, &clock);
                self.text.draw_centered(pm, hours, &clock, cx, top, tint(&p), alpha);
                let y = top + size * CLOCK_LINE;
                let (mw, _) = self.text.draw_centered(pm, minutes, &clock, cx, y, p.on_surface, alpha);
                if let Some(a) = &ampm {
                    let small = Font { family: &self.fonts.ui, size: size * 0.2, weight: 500, tabular: false, tracking: 0.06 };
                    self.text.draw(pm, a, &small, cx + mw / 2.0 + 10.0 * s, y + lh * 0.55, p.on_surface_variant, alpha);
                }
                bottom = y + lh;
            }
            _ => {
                let (_, lh) = self.text.draw_centered(pm, &time, &clock, cx, top, p.on_surface, alpha);
                bottom += lh;
            }
        }
        self.draw_info_chips(pm, cx, bottom, alpha, unit, &tonal);
        if !info.focused {
            if self.demo {
                paint::fade(pm, presence);
            }
            return;
        }

        // The tonal card.
        let avatar = self.look.cfg.show_avatar;
        let card_w = (CARD_W * unit).min(fw - 2.0 * FOOT_MARGIN * s);
        let card_h = card_height(avatar) * unit;
        let card_x = cx - card_w / 2.0;
        let card_y = card_span(info.height as f32, avatar).0 * s + lift * 0.6;
        let card = paint::rounded_rect(card_x, card_y, card_w, card_h, CARD_RADIUS * unit);
        paint::soft_shadow(pm, card_x, card_y, card_w, card_h, CARD_RADIUS * unit, 12.0 * s, 0.22 * alpha);
        paint::fill(pm, &card, &paint::solid(p.surface_container, CARD_ALPHA * alpha));
        let pad = CARD_PAD * unit;
        let mut y = card_y + pad;

        // The scalloped avatar.
        if avatar {
            let side = AVATAR * unit;
            let (ax, ay) = (cx - side / 2.0, y);
            if let Some(path) = avatar_shape::skia_path(self.morph.value(), ax, ay, side) {
                let turn = tiny_skia::Transform::from_rotate_at(self.spin.value().to_degrees(), cx, ay + side / 2.0);
                pm.fill_path(&path, &paint::solid(tonal.primary_container, alpha), tiny_skia::FillRule::Winding, turn, None);
            }
            let r = side / 2.0 * AVATAR_INNER;
            match &self.avatar {
                Some(img) => paint::image_circle(pm, img, cx, ay + side / 2.0, r, alpha),
                None => {
                    let mono = Font { family: &self.fonts.display, size: r * 0.82, weight: 600, tabular: false, tracking: 0.0 };
                    let letters = initials(&self.user.display);
                    let (_, lh) = self.text.measure(&letters, &mono);
                    self.text.draw_centered(pm, &letters, &mono, cx, ay + side / 2.0 - lh / 2.0, tonal.on_primary_container, alpha);
                }
            }
            y += side + GAP * unit;
        }
        let name = Font { family: &self.fonts.ui, size: NAME_SIZE * unit, weight: 600, tabular: false, tracking: 0.0 };
        self.text.draw_centered(pm, &self.user.display, &name, cx, y, p.on_surface, alpha);
        y += (NAME_LINE + FIELD_GAP) * unit;

        // The filled field and the button beside it (on the reading end).
        let fh_px = FIELD_H * unit;
        let inner_w = card_w - 2.0 * pad;
        let field_w = inner_w - fh_px - BUTTON_GAP * unit;
        let shake = self.shake.value() * s;
        let (field_x, button_x) = if self.rtl { (card_x + pad + fh_px + BUTTON_GAP * unit, card_x + pad) } else { (card_x + pad, card_x + pad + field_w + BUTTON_GAP * unit) };
        let (field_x, button_x) = (field_x + shake, button_x + shake);
        let look = inputs::look(InputDesign::Filled, true, &p);
        let container = look.fill.unwrap_or(p.surface_container_high);
        paint::fill(pm, &top_rounded(field_x, y, field_w, fh_px, FIELD_RADIUS * unit), &paint::solid(container, FIELD_ALPHA * alpha));
        let flash = self.flash.value().clamp(0.0, 1.0);
        let (edge, edge_px) = look.edge.unwrap_or((p.primary, inputs::EDGE_PX.1));
        let edge = if flash > 0.01 { edge.mix(p.urgent_coral, flash) } else { edge };
        let error = matches!(self.note, Some(Note::Error(_)));
        let edge = if error { p.urgent_coral } else { edge };
        if let Some(rect) = tiny_skia::Rect::from_xywh(field_x, y + fh_px - edge_px * s, field_w, edge_px * s) {
            pm.fill_rect(rect, &paint::solid(edge, alpha), tiny_skia::Transform::identity(), None);
        }
        let n = self.dots_visible();
        let text_px = SYMBOL_TEXT * unit;
        let field_l = field_x + FIELD_PAD * unit;
        let field_r = field_x + field_w - FIELD_PAD * unit;
        let hint = Font { family: &self.fonts.ui, size: 15.0 * unit, weight: 400, tabular: false, tracking: 0.0 };
        let mut row_y = y + fh_px / 2.0;
        if n == 0 && self.phase == Phase::Idle {
            // The hint, from the field's start edge.
            let label = t("Enter password");
            let (lw, lh) = self.text.measure(label, &hint);
            let lx = if self.rtl { field_r - lw } else { field_l };
            self.text.draw(pm, label, &hint, lx, y + (fh_px - lh) / 2.0, p.on_surface_variant, alpha);
        } else {
            // Typing: the hint floats up as a small label.
            let label = t("Enter password");
            let small = Font { family: &self.fonts.ui, size: LABEL_SIZE * unit, weight: 500, tabular: false, tracking: 0.0 };
            let (lw, _) = self.text.measure(label, &small);
            let lx = if self.rtl { field_r - lw } else { field_l };
            self.text.draw(pm, label, &small, lx, y + 7.0 * unit, if error { p.urgent_coral } else { p.primary }, alpha);
            row_y = y + fh_px * 0.62;
        }
        self.draw_symbols(pm, (field_l, field_r, row_y), text_px, &p, alpha, now);

        // The button: tonal with a padlock while empty, filled with an arrow
        // once there is something to send; it squares off and shows the
        // loading indicator while checking, then a check.
        let busy = self.phase != Phase::Idle;
        let filled = n > 0 || busy;
        let radius = buttons::icon_radius(ButtonDesign::Expressive, fh_px, self.morph.value() / MORPH_CHECKING);
        let (bg, ink) = if filled { (p.primary, p.on_primary) } else { (tonal.secondary_container, tonal.on_secondary_container) };
        paint::fill(pm, &paint::rounded_rect(button_x, y, fh_px, fh_px, radius), &paint::solid(bg, alpha));
        let (bx, by, br) = (button_x + fh_px / 2.0, y + fh_px / 2.0, fh_px * 0.22);
        match self.phase {
            Phase::Checking => {
                // Material's loading indicator: a soft cookie turning as
                // it breathes (still with reduced motion).
                let t = now.duration_since(self.started).as_secs_f32();
                let t = if self.look.reduced_motion { 0.0 } else { t };
                let side = br * 2.6 * (LOADER_REST + (1.0 - LOADER_REST) * ((t * LOADER_BREATH).sin() * 0.5 + 0.5));
                if let Some(path) = Symbol::Cookie6.skia_path() {
                    let turn = tiny_skia::Transform::from_row(side, 0.0, 0.0, side, bx - side / 2.0, by - side / 2.0).post_concat(tiny_skia::Transform::from_rotate_at((t * LOADER_TURN).to_degrees(), bx, by));
                    pm.fill_path(path, &paint::solid(ink, alpha), tiny_skia::FillRule::Winding, turn, None);
                }
            }
            Phase::Unlocking => super::draw_check(pm, bx, by, br, ink, alpha, s),
            Phase::Idle if filled => {
                if self.rtl {
                    draw_arrow_back(pm, bx, by, br, ink, alpha, s);
                } else {
                    super::draw_arrow(pm, bx, by, br, ink, alpha, s);
                }
            }
            Phase::Idle => super::draw_padlock(pm, bx, by, br * 1.1, ink, alpha, s),
        }

        // Supporting text under the field, from its start edge.
        let support = Font { family: &self.fonts.ui, size: SUPPORT_SIZE * unit, weight: 500, tabular: false, tracking: 0.0 };
        let sy = y + fh_px + 6.0 * unit;
        let line: Option<(&str, Color)> = match &self.note {
            Some(Note::Error(m)) => Some((m.as_str(), p.urgent_coral.with_lightness(p.urgent_coral.lightness().max(if p.is_dark { TINT_LIGHTNESS.0 } else { 0.0 })))),
            Some(Note::Info(m)) => Some((m.as_str(), p.on_surface_variant)),
            None if self.phase == Phase::Checking => Some((t("Checking…"), p.on_surface_variant)),
            None if self.keyboard.caps_lock => Some((t("Caps Lock is on"), p.warning_amber)),
            None => None,
        };
        if let Some((text, ink)) = line {
            let (lw, _) = self.text.measure(text, &support);
            let lx = if self.rtl { field_r - lw } else { field_l };
            self.text.draw(pm, text, &support, lx, sy, ink, alpha);
        }

        // Footer: battery and keyboard layout as assist chips, the demo hint.
        let foot_y = fh - (FOOT_MARGIN + CHIP_H) * unit;
        if self.look.cfg.show_battery
            && let Some((pct, charging)) = super::look::battery()
        {
            let label = if charging { format!("{pct}% ⚡") } else { format!("{pct}%") };
            let ink = if pct <= self.look.cfg.low_battery() && !charging { p.urgent_coral } else { p.on_surface };
            let w = self.chip_width(&label, unit);
            let x = if self.rtl { fw - FOOT_MARGIN * unit - w } else { FOOT_MARGIN * unit };
            self.draw_chip(pm, (x, foot_y), &label, None, Some(p.outline), ink, alpha, unit);
        }
        if !self.keyboard.layout.is_empty() {
            let layout = self.keyboard.layout.clone();
            let w = self.chip_width(&layout, unit);
            let x = if self.rtl { FOOT_MARGIN * unit } else { fw - FOOT_MARGIN * unit - w };
            self.draw_chip(pm, (x, foot_y), &layout, None, Some(p.outline), p.on_surface, alpha, unit);
        }
        if self.demo {
            let foot = Font { family: &self.fonts.ui, size: 12.5 * s, weight: 450, tabular: false, tracking: 0.02 };
            self.text.draw_centered(pm, t("Preview — any password unlocks · Esc closes"), &foot, cx, foot_y + 8.0 * unit, p.on_surface_variant, 0.8 * alpha);
            paint::fade(pm, presence);
        }
    }

    /// The masked password in the field, from its start edge.
    fn draw_symbols(&mut self, pm: &mut PixmapMut<'_>, (left, right, cy): (f32, f32, f32), text_px: f32, p: &MandaraPalette, alpha: f32, now: Instant) {
        let n = self.dots_visible();
        let left = left.round();
        let width = right.round() - left;
        let row = Row::new(width, text_px, 1.0);
        let dots = self.look.cfg.password_symbols == mandara_config::desktop::PasswordSymbols::Dots;
        let animate = self.animate_symbols();
        let elapsed = now.duration_since(self.started).as_secs_f32();
        let checking = self.phase == Phase::Checking;
        let seed = self.symbol_seed;
        let draw = |pm: &mut PixmapMut<'_>, ix: usize, x: f32, scale: f32, opacity: f32, slot: usize| {
            let (shape, d) = if dots { (Symbol::Circle, (text_px * symbols::DOT_EM).round()) } else { (symbols::symbol_at(seed, ix), row.size) };
            let Some(path) = shape.skia_path() else { return };
            let wave = if checking && animate { 0.45 + 0.55 * ((elapsed * 6.0 - slot as f32 * 0.55).sin() * 0.5 + 0.5) } else { 1.0 };
            let k = d * scale.max(0.0);
            let (sx, sy) = (x + row.size / 2.0, cy.round());
            let tr = tiny_skia::Transform::from_row(k, 0.0, 0.0, k, sx - k / 2.0, sy - k / 2.0);
            pm.fill_path(path, &paint::solid(p.on_surface, alpha * opacity * wave), tiny_skia::FillRule::Winding, tr, None);
        };
        let first = row.first_shown(n);
        for ix in first..n {
            let Some(x) = row.left(ix, n, left, width, self.rtl) else { continue };
            let (scale, opacity) = match (self.phase, self.typed.get(ix)) {
                (Phase::Idle, Some(at)) => symbols::appear(now.saturating_duration_since(*at), animate),
                _ => (1.0, 1.0),
            };
            draw(pm, ix, x, scale, opacity, ix - first);
        }
        if self.phase == Phase::Idle {
            for (ix, scale, opacity) in self.fades.live(now, animate) {
                if let Some(x) = row.left(ix, n, left, width, self.rtl) {
                    draw(pm, ix, x, scale, opacity, 0);
                }
            }
        }
    }

    /// Weather, the track playing and the notification count as tonal
    /// chips centred under the clock.
    fn draw_info_chips(&mut self, pm: &mut PixmapMut<'_>, cx: f32, top: f32, alpha: f32, unit: f32, tonal: &Tonal) {
        let snap = &self.look.snapshot;
        let mut parts: smallvec::SmallVec<[String; 3]> = smallvec::SmallVec::new();
        if let Some(w) = &snap.weather {
            parts.push(w.clone());
        }
        if let Some(m) = &snap.media {
            parts.push(format!("♪ {m}"));
        }
        if snap.notifications > 0 {
            parts.push(mandara_i18n::tf("{count} notifications", &[("count", &snap.notifications)]));
        }
        if parts.is_empty() {
            return;
        }
        let widths: smallvec::SmallVec<[f32; 3]> = parts.iter().map(|p| self.chip_width(p, unit)).collect();
        let total = widths.iter().sum::<f32>() + CHIP_GAP * unit * (parts.len() - 1) as f32;
        let mut x = cx - total / 2.0;
        let y = top + 8.0 * unit;
        let order: smallvec::SmallVec<[usize; 3]> = if self.rtl { (0..parts.len()).rev().collect() } else { (0..parts.len()).collect() };
        for i in order {
            self.draw_chip(pm, (x, y), &parts[i], Some(tonal.surface_highest.with_alpha(CHIP_ALPHA)), None, self.palette.on_surface, alpha, unit);
            x += widths[i] + CHIP_GAP * unit;
        }
    }

    fn chip_width(&mut self, text: &str, unit: f32) -> f32 {
        let font = Font { family: &self.fonts.ui, size: CHIP_TEXT * unit, weight: 500, tabular: true, tracking: 0.0 };
        self.text.measure(text, &font).0 + 2.0 * CHIP_PAD * unit
    }

    /// A Material chip at `(x, y)`: tonal (`fill`) or outlined.
    #[expect(clippy::too_many_arguments, reason = "a chip's place, text and three colours")]
    fn draw_chip(&mut self, pm: &mut PixmapMut<'_>, (x, y): (f32, f32), text: &str, fill: Option<Color>, outline: Option<Color>, ink: Color, alpha: f32, unit: f32) {
        let font = Font { family: &self.fonts.ui, size: CHIP_TEXT * unit, weight: 500, tabular: true, tracking: 0.0 };
        let (tw, th) = self.text.measure(text, &font);
        let (w, h) = (tw + 2.0 * CHIP_PAD * unit, CHIP_H * unit);
        let shape = paint::rounded_rect(x, y, w, h, CHIP_RADIUS * unit);
        if let Some(c) = fill {
            paint::fill(pm, &shape, &paint::solid(c, alpha));
        }
        if let Some(c) = outline {
            paint::stroke(pm, &shape, &paint::solid(c, 0.9 * alpha), unit.max(1.0));
        }
        self.text.draw(pm, text, &font, x + CHIP_PAD * unit, y + (h - th) / 2.0, ink, alpha);
    }
}

/// An arrow pointing left (the send button in right-to-left languages).
fn draw_arrow_back(pm: &mut PixmapMut<'_>, cx: f32, cy: f32, r: f32, c: Color, alpha: f32, s: f32) {
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(cx + r, cy);
    pb.line_to(cx - r, cy);
    pb.move_to(cx - r * 0.25, cy - r * 0.75);
    pb.line_to(cx - r, cy);
    pb.line_to(cx - r * 0.25, cy + r * 0.75);
    if let Some(path) = pb.finish() {
        pm.stroke_path(&path, &paint::solid(c, alpha), &super::round_stroke(2.2 * s), tiny_skia::Transform::identity(), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_wallpapers_get_a_deeper_scrim() {
        assert_eq!(extra_scrim(None), 0.0);
        assert_eq!(extra_scrim(Some(0.2)), 0.0, "dark wallpapers keep theirs");
        assert!(extra_scrim(Some(0.9)) > extra_scrim(Some(0.6)));
        assert!(extra_scrim(Some(1.0)) <= 0.6);
    }

    #[test]
    fn the_clock_grows_from_thin_in_steps() {
        assert_eq!(clock_weight(0.0), 250);
        assert_eq!(clock_weight(1.0), 500);
        assert_eq!(clock_weight(7.0), 500);
        for k in 0..=20 {
            let w = clock_weight(k as f32 / 20.0);
            assert_eq!(w % WEIGHT_STEP as u16, 0, "{w}");
        }
    }

    /// The card clears the clock and the footer on every screen size.
    #[test]
    fn the_card_fits_between_the_clock_and_the_footer() {
        for height in [600.0, 720.0, 768.0, 900.0, 1080.0, 1440.0, 1600.0, 2160.0] {
            let u = L::scale(height);
            let clock_bottom = (CLOCK_TOP + CLOCK_SIZE * (CLOCK_LINE + 1.25) + 8.0 + CHIP_H) * u;
            for avatar in [true, false] {
                let (top, bottom) = card_span(height, avatar);
                assert!(top > clock_bottom, "{height}: card top {top} under the clock {clock_bottom}");
                assert!(bottom < height - (FOOT_MARGIN + CHIP_H) * u, "{height}: card bottom {bottom} above the footer");
            }
        }
    }
}
