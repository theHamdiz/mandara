//! Paints a [`Composition`]'s marks into a plane (tiny-skia + the shared
//! text painter), with the widget's colours, opacity and optional soft
//! shadow / glow.

use mandara_config::desktop_widgets::{Effect, FontRole, Look, Tone};
use mandara_theme::Color;
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PixmapMut, PixmapPaint, Stroke, Transform};

use super::layout::{Composition, InkRole, Mark, MarkKind, Measure, TextStyle};
use crate::backdrop::paint::{FontNames, icon_pixmap};
use crate::desktop::Ink;
use crate::lock::paint::{self as lp, Font, TextPainter};

/// Shadow / glow: blur radius and shadow drop, reference px; their opacity.
const EFFECT_BLUR_PX: f32 = 7.0;
const SHADOW_DROP_PX: f32 = 2.0;
const SHADOW_ALPHA: f32 = 0.5;
const GLOW_ALPHA: f32 = 0.75;
/// A sideways run of vertical text is turned this far.
const QUARTER_TURN_DEG: f32 = 90.0;
/// Box-blur passes (three approximate a Gaussian).
const BLUR_PASSES: usize = 3;
/// Margin a plane keeps around its marks for anti-aliasing, reference px.
pub const AA_MARGIN_PX: f32 = 2.0;
/// Arc segments per full turn.
const ARC_STEPS: f32 = 180.0;
const MIN_ARC_STEPS: usize = 8;
/// Wave polyline points per period.
const WAVE_STEPS_PER_PERIOD: f32 = 24.0;
/// Opacity of the area under a sparkline (of its line's).
const GRAPH_FILL_ALPHA: f32 = 0.16;
/// Accents lighter than this get dark text on them.
const ON_ACCENT_SWITCH: f32 = 0.62;
const ON_ACCENT_DARK: f32 = 0.16;
const ON_ACCENT_LIGHT: f32 = 0.98;

/// The resolved colours of one widget.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Inks {
    pub fg: Color,
    pub accent: Color,
    pub on_accent: Color,
    /// Shadow colour (the opposite of the ink's lightness).
    pub shadow: Color,
}

impl Inks {
    fn of(&self, role: InkRole) -> Color {
        match role {
            InkRole::Fg => self.fg,
            InkRole::Accent => self.accent,
            InkRole::OnAccent => self.on_accent,
        }
    }
}

/// `#rrggbb` or `oklch(L C H)` (L as 0–1 or a percentage).
pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if s.starts_with('#') {
        return Color::from_hex(s).ok();
    }
    let inner = s.strip_prefix("oklch(")?.strip_suffix(')')?;
    let mut it = inner.split([' ', ',', '/']).filter(|p| !p.is_empty());
    let l = it.next()?;
    let l = match l.strip_suffix('%') {
        Some(p) => p.parse::<f32>().ok()? / 100.0,
        None => l.parse::<f32>().ok()?,
    };
    let c = it.next()?.parse::<f32>().ok()?;
    let h = it.next()?.trim_end_matches("deg").parse::<f32>().ok()?;
    Some(Color::oklch(l.clamp(0.0, 1.0), c.max(0.0), h))
}

/// Colours for `look` over a backdrop that gave `auto` (light ink over
/// dark wallpaper and vice versa).
pub fn inks(look: &Look, auto: &Ink, on_surface: Color) -> Inks {
    let pick = |tone: Tone, custom: &str, fallback: Color| match tone {
        Tone::Auto => fallback,
        Tone::OnSurface => on_surface,
        Tone::Accent => auto.accent,
        Tone::Custom => parse_color(custom).unwrap_or(fallback),
    };
    let fg = pick(look.tone, &look.custom_color, auto.fg);
    let acc = pick(look.accent, &look.custom_accent, auto.accent);
    let on_accent = if acc.lightness() > ON_ACCENT_SWITCH { Color::oklch(ON_ACCENT_DARK, acc.chroma() * 0.3, acc.hue()) } else { Color::oklch(ON_ACCENT_LIGHT, acc.chroma() * 0.2, acc.hue()) };
    let shadow = if fg.lightness() > 0.5 { Color::oklch(0.08, 0.0, 0.0) } else { Color::oklch(0.98, 0.0, 0.0) };
    Inks { fg, accent: acc, on_accent, shadow }
}

/// The font family of a role.
pub fn family(fonts: &FontNames, role: FontRole) -> &str {
    match role {
        FontRole::Display => &fonts.display,
        FontRole::Ui => &fonts.ui,
        FontRole::Arabic => &fonts.arabic,
        FontRole::Mono => &fonts.mono,
    }
}

/// [`Measure`] with the real fonts.
pub struct Measurer<'a> {
    pub text: &'a mut TextPainter,
    pub fonts: &'a FontNames,
}

impl Measure for Measurer<'_> {
    fn measure(&mut self, text: &str, s: &TextStyle) -> (f32, f32) {
        let font = Font { family: family(self.fonts, s.role), size: s.size, weight: s.weight, tabular: true, tracking: s.tracking };
        self.text.measure(text, &font)
    }
}

/// How a plane is painted.
pub struct Style<'a> {
    pub fonts: &'a FontNames,
    pub inks: Inks,
    pub opacity: f32,
    pub effect: Effect,
    /// Physical px per reference px.
    pub k: f32,
}

/// Extra margin the effect needs around the marks, physical px.
pub fn effect_margin(effect: Effect, k: f32) -> f32 {
    match effect {
        Effect::None => AA_MARGIN_PX * k,
        Effect::Shadow | Effect::Glow => (EFFECT_BLUR_PX * BLUR_PASSES as f32 + SHADOW_DROP_PX + AA_MARGIN_PX) * k,
    }
}

/// Paints the marks with `live == live` into `pm`, whose top-left is at
/// `origin` in the composition's coordinates.
pub fn paint(pm: &mut PixmapMut<'_>, text: &mut TextPainter, comp: &Composition, live: bool, origin: (f32, f32), st: &Style<'_>) {
    let marks: Vec<&Mark> = comp.marks.iter().filter(|m| m.live == live && m.opacity > 0.0).collect();
    if marks.is_empty() {
        return;
    }
    let shift = (-origin.0, -origin.1);
    if st.effect != Effect::None {
        let (color, alpha, drop) = match st.effect {
            Effect::Shadow => (st.inks.shadow, SHADOW_ALPHA, SHADOW_DROP_PX * st.k),
            _ => (st.inks.fg, GLOW_ALPHA, 0.0),
        };
        let flat = Inks { fg: color, accent: if st.effect == Effect::Glow { st.inks.accent } else { color }, on_accent: color, shadow: color };
        for m in &marks {
            if m.ink != InkRole::OnAccent {
                draw(pm, text, m, (shift.0, shift.1 + drop), &flat, st.fonts, alpha * st.opacity);
            }
        }
        box_blur(pm, (EFFECT_BLUR_PX * st.k).round().max(1.0) as usize);
    }
    for m in &marks {
        draw(pm, text, m, shift, &st.inks, st.fonts, st.opacity);
    }
}

fn solid(c: Color, opacity: f32) -> Paint<'static> {
    lp::solid(c, opacity)
}

fn stroke(width: f32) -> Stroke {
    Stroke { width: width.max(0.1), line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() }
}

fn polyline(pts: impl Iterator<Item = (f32, f32)>, close: bool) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    for (i, (x, y)) in pts.enumerate() {
        if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
    }
    if close {
        pb.close();
    }
    pb.finish()
}

fn draw(pm: &mut PixmapMut<'_>, text: &mut TextPainter, m: &Mark, (dx, dy): (f32, f32), inks: &Inks, fonts: &FontNames, opacity: f32) {
    let color = inks.of(m.ink);
    let alpha = (m.opacity * opacity).clamp(0.0, 1.0);
    let t = Transform::identity();
    match &m.kind {
        MarkKind::Text { text: s, style, x, y, .. } => {
            let font = Font { family: family(fonts, style.role), size: style.size, weight: style.weight, tabular: true, tracking: style.tracking };
            text.draw(pm, s, &font, x + dx, y + dy, color, alpha);
        }
        MarkKind::SideText { text: s, style, x, y, w, h, clockwise } => {
            // Shaped upright into a scratch line, then turned into place.
            let font = Font { family: family(fonts, style.role), size: style.size, weight: style.weight, tabular: true, tracking: style.tracking };
            let Some(mut line) = Pixmap::new(h.ceil().max(1.0) as u32, w.ceil().max(1.0) as u32) else { return };
            // Opaque first, faded as a whole: joined glyphs overlap, and
            // fading each would brighten the joins.
            text.draw(&mut line.as_mut(), s, &font, 0.0, 0.0, color, 1.0);
            let degrees = if *clockwise { QUARTER_TURN_DEG } else { -QUARTER_TURN_DEG };
            draw_rotated(pm, &line, degrees, (x + dx + w / 2.0, y + dy + h / 2.0), alpha);
        }
        MarkKind::Poly { pts } => {
            if let Some(p) = polyline(pts.iter().map(|p| (p.0 + dx, p.1 + dy)), true) {
                pm.fill_path(&p, &solid(color, alpha), FillRule::Winding, t, None);
            }
        }
        MarkKind::Line { a, b, width } => {
            if let Some(p) = polyline([(a.0 + dx, a.1 + dy), (b.0 + dx, b.1 + dy)].into_iter(), false) {
                pm.stroke_path(&p, &solid(color, alpha), &stroke(*width), t, None);
            }
        }
        MarkKind::Arc { c, r, width, start, sweep } => {
            let steps = ((sweep.abs() / std::f32::consts::TAU * ARC_STEPS).ceil() as usize).max(MIN_ARC_STEPS);
            let full = (sweep.abs() - std::f32::consts::TAU).abs() < 1e-4;
            let pts = (0..=steps).map(|i| {
                let a = start + sweep * i as f32 / steps as f32;
                (c.0 + dx + r * a.cos(), c.1 + dy + r * a.sin())
            });
            if let Some(p) = polyline(pts, full) {
                pm.stroke_path(&p, &solid(color, alpha), &stroke(*width), t, None);
            }
        }
        MarkKind::Disc { c, r } => lp::fill(pm, &lp::circle(c.0 + dx, c.1 + dy, *r), &solid(color, alpha)),
        MarkKind::Rect { x, y, w, h, radius, width } => {
            let path = lp::rounded_rect(x + dx, y + dy, *w, *h, *radius);
            if *width > 0.0 { lp::stroke(pm, &path, &solid(color, alpha), *width) } else { lp::fill(pm, &path, &solid(color, alpha)) }
        }
        MarkKind::Wave { x, y, w, amp, waves, width } => {
            let n = ((waves * WAVE_STEPS_PER_PERIOD).ceil() as usize).max(MIN_ARC_STEPS);
            let pts = (0..=n).map(|i| {
                let f = i as f32 / n as f32;
                (x + dx + w * f, y + dy + amp * (f * waves * std::f32::consts::TAU).sin())
            });
            if let Some(p) = polyline(pts, false) {
                pm.stroke_path(&p, &solid(color, alpha), &stroke(*width), t, None);
            }
        }
        MarkKind::Icon { name, x, y, size } => {
            if let Some(icon) = icon_pixmap(name, size.round().max(1.0) as u32, color) {
                let paint = PixmapPaint { opacity: alpha, ..PixmapPaint::default() };
                pm.draw_pixmap((x + dx).round() as i32, (y + dy).round() as i32, icon.as_ref(), &paint, t, None);
            }
        }
        MarkKind::Graph { x, y, w, h, values, width } => {
            if values.len() < 2 {
                return;
            }
            let n = values.len() - 1;
            let pt = |i: usize, v: f32| (x + dx + w * i as f32 / n as f32, y + dy + h * (1.0 - v.clamp(0.0, 1.0)));
            let line = values.iter().enumerate().map(|(i, v)| pt(i, *v));
            let area = line.clone().chain([(x + dx + w, y + dy + h), (x + dx, y + dy + h)]);
            if let Some(p) = polyline(area, true) {
                pm.fill_path(&p, &solid(color, alpha * GRAPH_FILL_ALPHA), FillRule::Winding, t, None);
            }
            if let Some(p) = polyline(line, false) {
                pm.stroke_path(&p, &solid(color, alpha), &stroke(*width), t, None);
            }
        }
    }
}

/// In-place separable box blur of a premultiplied plane, `passes` times.
fn box_blur(pm: &mut PixmapMut<'_>, radius: usize) {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    if w == 0 || h == 0 || radius == 0 {
        return;
    }
    let data = pm.data_mut();
    let mut line: Vec<[u32; 4]> = vec![[0; 4]; w.max(h)];
    let win = (2 * radius + 1) as u32;
    let mut blur_1d = |data: &mut [u8], start: usize, stride: usize, len: usize| {
        for (i, px) in line.iter_mut().take(len).enumerate() {
            let o = (start + i * stride) * 4;
            *px = [data[o] as u32, data[o + 1] as u32, data[o + 2] as u32, data[o + 3] as u32];
        }
        let at = |i: isize, line: &[[u32; 4]]| if i < 0 || i as usize >= len { [0; 4] } else { line[i as usize] };
        let mut sum = [0u32; 4];
        for i in -(radius as isize)..=(radius as isize) {
            let p = at(i, &line);
            for c in 0..4 {
                sum[c] += p[c];
            }
        }
        let src: Vec<[u32; 4]> = line[..len].to_vec();
        for i in 0..len {
            let o = (start + i * stride) * 4;
            for c in 0..4 {
                data[o + c] = (sum[c] / win) as u8;
            }
            let add = at(i as isize + radius as isize + 1, &src);
            let sub = at(i as isize - radius as isize, &src);
            for c in 0..4 {
                sum[c] = sum[c] + add[c] - sub[c];
            }
        }
    };
    for _ in 0..BLUR_PASSES {
        for y in 0..h {
            blur_1d(data, y * w, 1, w);
        }
        for x in 0..w {
            blur_1d(data, x, w, h);
        }
    }
}

/// Draws `src` rotated by `degrees` about its centre into `pm`, centred
/// at `(cx, cy)`, at `opacity` (decorations, sideways text).
pub fn draw_rotated(pm: &mut PixmapMut<'_>, src: &Pixmap, degrees: f32, (cx, cy): (f32, f32), opacity: f32) {
    let (w, h) = (src.width() as f32, src.height() as f32);
    let t = Transform::from_translate(-w / 2.0, -h / 2.0).post_rotate(degrees).post_translate(cx, cy);
    let paint = PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, opacity: opacity.clamp(0.0, 1.0), ..PixmapPaint::default() };
    pm.draw_pixmap(0, 0, src.as_ref(), &paint, t, None);
}

/// Axis-aligned size of a `w × h` box rotated by `degrees`.
pub fn rotated_size((w, h): (f32, f32), degrees: f32) -> (f32, f32) {
    let a = degrees.to_radians();
    let (s, c) = (a.sin().abs(), a.cos().abs());
    (w * c + h * s, w * s + h * c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_custom_colours() {
        assert!(parse_color("#ff0000").is_some_and(|c| c.to_rgba8()[0] == 255));
        let c = parse_color("oklch(70% 0.1 200)").unwrap();
        assert!((c.lightness() - 0.7).abs() < 0.01);
        assert!(parse_color("oklch(0.5, 0.05, 30deg)").is_some());
        assert!(parse_color("teal").is_none() && parse_color("oklch(1 2)").is_none());
    }

    #[test]
    fn rotation_grows_the_box() {
        let (w, h) = rotated_size((100.0, 20.0), 90.0);
        assert!((w - 20.0).abs() < 1e-3 && (h - 100.0).abs() < 1e-3);
        let (w, _) = rotated_size((100.0, 0.0), 45.0);
        assert!((w - 70.710_68).abs() < 1e-3);
    }

    #[test]
    fn blur_spreads_and_keeps_energy_roughly() {
        let mut pm = Pixmap::new(41, 41).unwrap();
        for y in 14..27 {
            for x in 14..27 {
                let i = (y * 41 + x) * 4;
                pm.data_mut()[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        box_blur(&mut pm.as_mut(), 2);
        let d = pm.data();
        let a = |x: usize, y: usize| d[(y * 41 + x) * 4 + 3];
        assert!(a(14, 20) < 255 && a(11, 20) > 0, "spread past the edge");
        assert!(a(20, 20) > 200, "the middle stays solid");
        assert_eq!(d[3], 0, "far corner untouched");
    }
}
