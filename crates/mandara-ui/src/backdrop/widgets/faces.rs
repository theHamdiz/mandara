//! The clock faces added after the first ten: a stacked card clock, the
//! time in a shape, dot-matrix and pixel digits, a digit per box, a slanted
//! HUD, free text lines, hands over giant numerals and an orbit — plus the
//! configurable analog dial (scalloped, dotted, numbered, blade hands).
//!
//! Like [`super::layout`], every function returns marks in physical px for
//! `k` physical px per reference px; only seconds go to the live plane.

use std::f32::consts::{FRAC_PI_2, TAU};

use mandara_config::desktop_widgets::{AnalogOptions, ClockWidget, DateStyle, DialMarks, FontRole, HandStyle, PlateShape, SecondsStyle, TemplateLine};
use mandara_core::shape::{Shape, rotate};

use super::frame::shape_of;
use super::layout::{ClockStrings, InkRole, Mark, MarkKind, Measure, Run, caps, face, line, run, stack, style, tracked};

/// Reference sizes (px at a 1080 px tall display).
mod size {
    pub const STACK_DIGITS: f32 = 190.0;
    pub const STACK_DATE: f32 = 24.0;
    pub const STACK_DATE_GAP: f32 = 6.0;
    pub const SHAPE_BOX: f32 = 300.0;
    pub const SHAPE_TIME: f32 = 84.0;
    pub const SHAPE_DATE: f32 = 22.0;
    pub const SHAPE_DATE_GAP: f32 = 16.0;
    /// Degrees the spinning shape turns per minute.
    pub const SHAPE_SPIN_DEG: f32 = 6.0;
    pub const DOT_PITCH: f32 = 15.0;
    pub const DOT_RADIUS: f32 = 0.42;
    pub const PIXEL: f32 = 22.0;
    pub const GLYPH_GAP: f32 = 1.0;
    pub const MATRIX_DATE: f32 = 15.0;
    pub const MATRIX_DATE_GAP: f32 = 18.0;
    pub const MATRIX_TRACKING: f32 = 0.22;
    pub const BOX_W: f32 = 92.0;
    pub const BOX_H: f32 = 124.0;
    pub const BOX_GAP: f32 = 10.0;
    pub const BOX_PAIR_GAP: f32 = 30.0;
    pub const BOX_DIGIT: f32 = 92.0;
    pub const BOX_RIM: f32 = 2.0;
    pub const BOX_RIM_ALPHA: f32 = 0.22;
    pub const CHIP: f32 = 15.0;
    pub const CHIP_PAD_X: f32 = 9.0;
    pub const CHIP_PAD_Y: f32 = 3.0;
    pub const CHIP_GAP: f32 = 6.0;
    pub const WEEK: f32 = 15.0;
    pub const WEEK_CELL: f32 = 34.0;
    pub const WEEK_GAP: f32 = 16.0;
    pub const HUD_TIME: f32 = 150.0;
    pub const HUD_META_SMALL: f32 = 18.0;
    pub const HUD_META_DAY: f32 = 40.0;
    pub const HUD_META_PART: f32 = 26.0;
    pub const HUD_GAP: f32 = 22.0;
    pub const HUD_MOON: f32 = 34.0;
    pub const HUD_TRACKING: f32 = 0.15;
    pub const TEMPLATE_GAP: f32 = 4.0;
    pub const DIVIDER_W: f32 = 2.0;
    pub const DIVIDER_GAP: f32 = 22.0;
    pub const NUMERALS_SIZE: f32 = 250.0;
    pub const NUMERALS_R: f32 = 150.0;
    pub const ORBIT_INNER: f32 = 86.0;
    pub const ORBIT_OUTER: f32 = 150.0;
    pub const ORBIT_RING_W: f32 = 2.0;
    pub const ORBIT_RING_ALPHA: f32 = 0.28;
    pub const ORBIT_HOUR: f32 = 15.0;
    pub const ORBIT_MINUTE: f32 = 10.0;
    pub const ORBIT_SUN: f32 = 44.0;
    pub const ORBIT_TIME: f32 = 22.0;
    pub const ORBIT_LABEL: f32 = 15.0;
    pub const ORBIT_LABEL_GAP: f32 = 18.0;
    pub const MOON_DARK_ALPHA: f32 = 0.2;
}

/// Points along a moon's lit limb and terminator.
const MOON_STEPS: usize = 32;
/// Hours on a dial; minutes per hour.
const HOURS: usize = 12;
const MINUTES: f32 = 60.0;
/// Days in a week.
const WEEK_DAYS: usize = 7;

fn at(c: (f32, f32), turn: f32, reach: f32) -> (f32, f32) {
    let a = turn * TAU - FRAC_PI_2;
    (c.0 + reach * a.cos(), c.1 + reach * a.sin())
}

/// A text mark centred on `c`.
fn centred(text: String, s: super::layout::TextStyle, c: (f32, f32), m: &mut dyn Measure, ink: InkRole) -> Mark {
    let (w, h) = m.measure(&text, &s);
    Mark::new(MarkKind::Text { text, style: s, x: c.0 - w / 2.0, y: c.1 - h / 2.0, w, h }, ink)
}

/// The moon at `phase` (0 new … ½ full) as a dark disc and its lit part.
pub fn moon(c: (f32, f32), r: f32, phase: f32, lit: InkRole) -> Vec<Mark> {
    let mut marks = vec![Mark::new(MarkKind::Disc { c, r }, lit).dim(size::MOON_DARK_ALPHA)];
    let waxing = phase < 0.5;
    let side = if waxing { 1.0 } else { -1.0 };
    let k = (phase * TAU).cos();
    let mut pts = Vec::with_capacity(MOON_STEPS * 2 + 2);
    for i in 0..=MOON_STEPS {
        let t = -FRAC_PI_2 + std::f32::consts::PI * i as f32 / MOON_STEPS as f32;
        pts.push((c.0 + side * r * t.cos(), c.1 + r * t.sin()));
    }
    for i in (0..=MOON_STEPS).rev() {
        let t = -FRAC_PI_2 + std::f32::consts::PI * i as f32 / MOON_STEPS as f32;
        pts.push((c.0 + side * r * k * t.cos(), c.1 + r * t.sin()));
    }
    marks.push(Mark::new(MarkKind::Poly { pts }, lit));
    marks
}

// ── Stack ────────────────────────────────────────────────────────────────

/// Hours over minutes in huge numerals, the date under them.
pub fn stack_face(c: &ClockWidget, s: &ClockStrings, k: f32, align: f32, rtl: bool, m: &mut dyn Measure) -> Vec<Mark> {
    use size::*;
    let px = |v: f32| v * k;
    let role = c.look.font;
    let digits = style(role, px(STACK_DIGITS), face::BOLD);
    let (hours, (hw, hh)) = line(vec![run(&s.hours, digits.clone(), InkRole::Fg)], 0.0, 0.0, false, m);
    let (minutes, (mw, mh)) = line(vec![run(&s.minutes, digits, InkRole::Accent)], 0.0, 0.0, false, m);
    let gap = hh * c.face_options.line_gap();
    let mut lines = vec![(hours, (hw, hh)), (minutes, (mw, mh))];
    if c.date {
        let ds = tracked(style(role, px(STACK_DATE), face::SEMIBOLD), size::MATRIX_TRACKING / 2.0);
        let date = caps(&format!("{}{}{}", s.weekday_short, face::SEPARATOR, s.day));
        lines.push(line(vec![Run { opacity: face::DIM, ..run(date, ds, InkRole::Fg) }], 0.0, 0.0, rtl, m));
    }
    let date_gap = px(STACK_DATE_GAP);
    let mut y = 0.0;
    let width = lines.iter().map(|l| l.1.0).fold(0.0, f32::max);
    let mut out = Vec::new();
    for (i, (mut marks, (w, h))) in lines.into_iter().enumerate() {
        for mk in &mut marks {
            mk.shift((width - w) * align, y);
        }
        out.extend(marks);
        y += h + if i == 0 { gap } else { date_gap };
    }
    out
}

// ── Shape ────────────────────────────────────────────────────────────────

/// The time centred in a filled shape (turning a step every minute when
/// `spin` is on), the date under it.
pub fn shape_face(c: &ClockWidget, s: &ClockStrings, k: f32, m: &mut dyn Measure) -> Vec<Mark> {
    use size::*;
    let px = |v: f32| v * k;
    let side = px(SHAPE_BOX);
    let shape = shape_of(c.face_options.shape, &c.look, k);
    let centre = (side / 2.0, side / 2.0);
    let turn = if c.face_options.spin { (s.hour * MINUTES as u32 + s.minute) as f32 * SHAPE_SPIN_DEG } else { 0.0 };
    let pts: Vec<(f32, f32)> = shape.outline(side, side).into_iter().map(|p| rotate(p, centre, turn)).collect();
    let mut marks = vec![Mark::new(MarkKind::Poly { pts }, InkRole::Accent)];
    marks.push(centred(s.time.clone(), style(c.look.font, px(SHAPE_TIME), face::BOLD), centre, m, InkRole::OnAccent));
    if c.date {
        let ds = style(c.look.font, px(SHAPE_DATE), face::MEDIUM);
        let date = format!("{}{}{}", s.weekday, face::SEPARATOR, s.day_month);
        let (dw, dh) = m.measure(&date, &ds);
        marks.push(Mark::new(MarkKind::Text { text: date, style: ds, x: (side - dw) / 2.0, y: side + px(SHAPE_DATE_GAP), w: dw, h: dh }, InkRole::Fg).dim(face::DIM));
    }
    marks
}

// ── Dot matrix and pixels ────────────────────────────────────────────────

/// 5 × 7 dot-matrix digits (Western and Eastern Arabic), row by row, `1` lit.
fn dot_glyph(ch: char) -> Option<&'static [&'static str]> {
    Some(match ch {
        '0' => &["01110", "10001", "10011", "10101", "11001", "10001", "01110"],
        '1' => &["00100", "01100", "00100", "00100", "00100", "00100", "01110"],
        '2' => &["01110", "10001", "00001", "00010", "00100", "01000", "11111"],
        '3' => &["11111", "00010", "00100", "00010", "00001", "10001", "01110"],
        '4' => &["00010", "00110", "01010", "10010", "11111", "00010", "00010"],
        '5' => &["11111", "10000", "11110", "00001", "00001", "10001", "01110"],
        '6' => &["00110", "01000", "10000", "11110", "10001", "10001", "01110"],
        '7' => &["11111", "00001", "00010", "00100", "01000", "01000", "01000"],
        '8' => &["01110", "10001", "10001", "01110", "10001", "10001", "01110"],
        '9' => &["01110", "10001", "10001", "01111", "00001", "00010", "01100"],
        '٠' => &["00000", "00000", "00100", "01010", "00100", "00000", "00000"],
        '١' => &["01000", "00100", "00100", "00100", "00100", "00100", "00100"],
        '٢' => &["10001", "10010", "11100", "10000", "10000", "10000", "10000"],
        '٣' => &["10101", "10101", "11110", "10000", "10000", "10000", "10000"],
        '٤' => &["00111", "01000", "00110", "01000", "10000", "10000", "01111"],
        '٥' => &["00000", "01110", "10001", "10001", "10001", "01110", "00000"],
        '٦' => &["11110", "00010", "00010", "00010", "00010", "00010", "00010"],
        '٧' => &["10001", "10001", "01010", "01010", "01010", "00100", "00100"],
        '٨' => &["00100", "00100", "01010", "01010", "01010", "10001", "10001"],
        '٩' => &["01110", "10010", "10010", "01110", "00010", "00010", "00010"],
        ':' => &["0", "0", "1", "0", "1", "0", "0"],
        _ => return None,
    })
}

/// 3 × 5 pixel digits (Western and Eastern Arabic).
fn pixel_glyph(ch: char) -> Option<&'static [&'static str]> {
    Some(match ch {
        '0' => &["111", "101", "101", "101", "111"],
        '1' => &["010", "110", "010", "010", "111"],
        '2' => &["111", "001", "111", "100", "111"],
        '3' => &["111", "001", "111", "001", "111"],
        '4' => &["101", "101", "111", "001", "001"],
        '5' => &["111", "100", "111", "001", "111"],
        '6' => &["111", "100", "111", "101", "111"],
        '7' => &["111", "001", "010", "010", "010"],
        '8' => &["111", "101", "111", "101", "111"],
        '9' => &["111", "101", "111", "001", "111"],
        '٠' => &["000", "000", "010", "000", "000"],
        '١' => &["100", "010", "010", "010", "010"],
        '٢' => &["101", "110", "100", "100", "100"],
        '٣' => &["111", "110", "100", "100", "100"],
        '٤' => &["011", "100", "010", "100", "011"],
        '٥' => &["010", "101", "101", "010", "000"],
        '٦' => &["111", "001", "001", "001", "001"],
        '٧' => &["101", "101", "101", "010", "010"],
        '٨' => &["010", "010", "101", "101", "101"],
        '٩' => &["111", "101", "111", "001", "001"],
        ':' => &["0", "1", "0", "1", "0"],
        _ => return None,
    })
}

/// The time as dots (`dots`: round dot-matrix, else square pixels), lit
/// dots in the ink and unlit ones faint; the date in small caps under it.
pub fn matrix_face(c: &ClockWidget, s: &ClockStrings, k: f32, align: f32, dots: bool, m: &mut dyn Measure) -> Vec<Mark> {
    use size::*;
    let px = |v: f32| v * k;
    let pitch = if dots { px(DOT_PITCH) } else { px(PIXEL) };
    let square = !dots || c.face_options.square_dots;
    let unlit = if dots { c.face_options.unlit() } else { 0.0 };
    let mut marks = Vec::new();
    let mut x = 0.0;
    // Digits stay left to right (a time), whatever the language.
    let text = format!("{}:{}", s.hours, s.minutes);
    for ch in text.chars() {
        let Some(rows) = (if dots { dot_glyph(ch) } else { pixel_glyph(ch) }) else { continue };
        let colon = ch == ':';
        for (r, row) in rows.iter().enumerate() {
            for (col, bit) in row.chars().enumerate() {
                let lit = bit == '1';
                if !lit && unlit <= 0.0 {
                    continue;
                }
                let (cx, cy) = (x + (col as f32 + 0.5) * pitch, (r as f32 + 0.5) * pitch);
                let kind = if square {
                    let side = if dots { pitch * DOT_RADIUS * 2.0 } else { pitch };
                    MarkKind::Rect { x: cx - side / 2.0, y: cy - side / 2.0, w: side, h: side, radius: 0.0, width: 0.0 }
                } else {
                    MarkKind::Disc { c: (cx, cy), r: pitch * DOT_RADIUS }
                };
                let ink = if colon { InkRole::Accent } else { InkRole::Fg };
                marks.push(Mark::new(kind, ink).dim(if lit { if colon { face::DIM } else { 1.0 } } else { unlit }));
            }
        }
        x += (rows[0].len() as f32 + GLYPH_GAP) * pitch;
    }
    let width = x - GLYPH_GAP * pitch;
    let height = if dots { 7.0 } else { 5.0 } * pitch;
    if c.date {
        let ds = tracked(style(FontRole::Mono, px(MATRIX_DATE), face::MEDIUM), MATRIX_TRACKING);
        let date = caps(&format!("{}{}{}", s.weekday, face::SEPARATOR, s.day_month));
        let (dw, dh) = m.measure(&date, &ds);
        let dx = (width - dw).max(0.0) * align - (dw - width).max(0.0) * (1.0 - align);
        marks.push(Mark::new(MarkKind::Text { text: date, style: ds, x: dx, y: height + px(MATRIX_DATE_GAP), w: dw, h: dh }, InkRole::Fg).dim(face::DIM));
    }
    marks
}

// ── Boxes ────────────────────────────────────────────────────────────────

/// Each digit in its own box, an AM / PM pair of chips and a week strip.
pub fn boxes_face(c: &ClockWidget, s: &ClockStrings, k: f32, m: &mut dyn Measure) -> Vec<Mark> {
    use size::*;
    let px = |v: f32| v * k;
    let role = c.look.font;
    let shape = shape_of(c.face_options.box_shape, &c.look, k);
    let shape = match shape {
        // Boxes are small: their corners follow the box, not the plate.
        Shape::Rect { radius } => Shape::Rect { radius: radius.min(px(BOX_W) / 4.0) },
        s => s,
    };
    let (bw, bh) = (px(BOX_W), px(BOX_H));
    let ds = style(role, px(BOX_DIGIT), face::BOLD);
    let mut marks = Vec::new();
    let mut x = 0.0;
    let digits: Vec<char> = s.hours.chars().chain(s.minutes.chars()).collect();
    for (i, d) in digits.iter().enumerate() {
        if i == 2 {
            x += px(BOX_PAIR_GAP) - px(BOX_GAP);
            marks.push(Mark::new(MarkKind::Disc { c: (x - px(BOX_PAIR_GAP) / 2.0 + px(BOX_GAP) / 2.0, bh * 0.36), r: px(BOX_RIM) * 2.0 }, InkRole::Accent).flat());
            marks.push(Mark::new(MarkKind::Disc { c: (x - px(BOX_PAIR_GAP) / 2.0 + px(BOX_GAP) / 2.0, bh * 0.64), r: px(BOX_RIM) * 2.0 }, InkRole::Accent).flat());
        }
        marks.push(Mark::new(MarkKind::Shaped { x, y: 0.0, w: bw, h: bh, shape, width: 0.0 }, InkRole::Plate));
        marks.push(Mark::new(MarkKind::Shaped { x, y: 0.0, w: bw, h: bh, shape, width: px(BOX_RIM) }, InkRole::OnPlate).dim(BOX_RIM_ALPHA));
        marks.push(centred(d.to_string(), ds.clone(), (x + bw / 2.0, bh / 2.0), m, InkRole::OnPlate));
        x += bw + px(BOX_GAP);
    }
    let width = x - px(BOX_GAP);
    let mut y = bh;
    if s.meridiem.is_some() {
        y += px(CHIP_GAP) * 2.0;
        let cs = style(role, px(CHIP), face::SEMIBOLD);
        let pm = s.hour >= HOURS as u32;
        let chips: Vec<(String, bool)> = s.meridiem_pair.iter().cloned().zip([!pm, pm]).collect();
        let sizes: Vec<(f32, f32)> = chips.iter().map(|(t, _)| m.measure(t, &cs)).collect();
        let total: f32 = sizes.iter().map(|(w, _)| w + 2.0 * px(CHIP_PAD_X)).sum::<f32>() + px(CHIP_GAP);
        let mut cx = width - total;
        let chip_h = sizes.iter().map(|(_, h)| *h).fold(0.0, f32::max) + 2.0 * px(CHIP_PAD_Y);
        for ((t, on), (tw, th)) in chips.into_iter().zip(sizes) {
            let (w, h) = (tw + 2.0 * px(CHIP_PAD_X), th + 2.0 * px(CHIP_PAD_Y));
            let pill = MarkKind::Rect { x: cx, y, w, h, radius: h / 2.0, width: if on { 0.0 } else { px(BOX_RIM) / 2.0 } };
            marks.push(Mark::new(pill, if on { InkRole::Accent } else { InkRole::Fg }).dim(if on { 1.0 } else { face::DIM }));
            marks.push(Mark::new(MarkKind::Text { text: t, style: cs.clone(), x: cx + px(CHIP_PAD_X), y: y + px(CHIP_PAD_Y), w: tw, h: th }, if on { InkRole::OnAccent } else { InkRole::Fg }).dim(if on { 1.0 } else { face::DIM }));
            cx += w + px(CHIP_GAP);
        }
        y += chip_h;
    }
    if c.face_options.week_strip && s.week.len() == WEEK_DAYS {
        y += px(WEEK_GAP);
        marks.extend(week_strip(&s.week, s.rtl, style(role, px(WEEK), face::SEMIBOLD), width, y, k, m));
    }
    marks
}

/// The boxes clock's week strip at `y`, `width` wide: today's label on an
/// accent pill sized to the label. The row is a whole cell tall plus a
/// little room under it, so a frame round the clock never touches the pill.
fn week_strip(week: &[(String, bool)], rtl: bool, ws: super::layout::TextStyle, width: f32, y: f32, k: f32, m: &mut dyn Measure) -> Vec<Mark> {
    use size::*;
    let px = |v: f32| v * k;
    let labels: Vec<(String, bool, (f32, f32))> = week.iter().map(|(l, t)| (caps(l), *t, m.measure(&caps(l), &ws))).collect();
    let text_h = labels.iter().map(|l| l.2.1).fold(0.0, f32::max);
    let pill_h = px(WEEK_CELL).max(text_h + 2.0 * px(CHIP_PAD_Y));
    let cell = (width / WEEK_DAYS as f32).min(px(WEEK_CELL) * 1.6);
    let x0 = (width - cell * WEEK_DAYS as f32) / 2.0;
    let mut marks = Vec::new();
    let order: Vec<&(String, bool, (f32, f32))> = if rtl { labels.iter().rev().collect() } else { labels.iter().collect() };
    for (i, (label, today, (tw, _))) in order.into_iter().enumerate() {
        let c0 = (x0 + cell * (i as f32 + 0.5), y + pill_h / 2.0);
        if *today {
            let pw = pill_h.max(tw + 2.0 * px(CHIP_PAD_X));
            marks.push(Mark::new(MarkKind::Rect { x: c0.0 - pw / 2.0, y, w: pw, h: pill_h, radius: pill_h / 2.0, width: 0.0 }, InkRole::Accent).flat());
        }
        marks.push(centred(label.clone(), ws.clone(), c0, m, if *today { InkRole::OnAccent } else { InkRole::Fg }).dim(if *today { 1.0 } else { face::DIM }).flat());
    }
    // Room under the row (invisible), part of the clock's size.
    marks.push(Mark::new(MarkKind::Rect { x: 0.0, y: y + pill_h, w: width, h: px(WEEK_GAP) / 2.0, radius: 0.0, width: 0.0 }, InkRole::Fg).dim(0.0));
    marks
}

// ── HUD ──────────────────────────────────────────────────────────────────

/// Heavy slanted time, then a slanted column: the part of the day, the
/// weekday, the date, and the moon.
pub fn hud_face(c: &ClockWidget, s: &ClockStrings, k: f32, rtl: bool, m: &mut dyn Measure) -> Vec<Mark> {
    use size::*;
    let px = |v: f32| v * k;
    let role = c.look.font;
    let slant = c.face_options.slant().to_radians().tan();
    let ts = style(role, px(HUD_TIME), face::BLACK);
    let (tw, th) = m.measure(&s.time, &ts);
    let mut marks = vec![Mark::new(MarkKind::SlantText { text: s.time.clone(), style: ts, x: 0.0, y: 0.0, w: tw, h: th, slant }, InkRole::Fg)];
    let mut col = Vec::new();
    let mut y = th * 0.12;
    for (text, size, weight, ink, tracking) in [
        (caps(&s.day_part), HUD_META_SMALL, face::BOLD, InkRole::Fg, HUD_TRACKING),
        (s.weekday_short.clone(), HUD_META_DAY, face::BLACK, InkRole::Fg, 0.0),
        (caps(&s.day_month), HUD_META_PART, face::BOLD, InkRole::Accent, 0.0),
    ] {
        if !c.date && ink == InkRole::Accent {
            continue;
        }
        let st = tracked(style(role, px(size), weight), tracking);
        let (w, h) = m.measure(&text, &st);
        col.push((Mark::new(MarkKind::SlantText { text, style: st, x: 0.0, y, w, h, slant }, ink).flat(), w));
        y += h;
    }
    let col_w = col.iter().map(|(_, w)| *w).fold(0.0, f32::max);
    let gap = px(HUD_GAP);
    let col_x = tw + th * slant.max(0.0) + gap;
    for (mut mk, _) in col {
        mk.shift(col_x, 0.0);
        marks.push(mk);
    }
    if c.face_options.moon {
        let r = px(HUD_MOON);
        marks.extend(moon((col_x + col_w + gap + r, th * 0.5), r, s.moon_phase, InkRole::Accent).into_iter().map(Mark::flat));
    }
    if rtl {
        // The time stays where reading starts: on the right.
        let width = marks.iter().map(|m| m.bounds().2).fold(0.0, f32::max);
        for mk in &mut marks {
            let (x0, _, x1, _) = mk.bounds();
            mk.shift(width - x1 - x0, 0.0);
        }
    }
    marks
}

// ── Template ─────────────────────────────────────────────────────────────

/// `{token}`s of a template line filled in from the clock's strings.
pub fn fill(template: &str, s: &ClockStrings) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let token = &rest[open + 1..open + close];
        match s.token(token) {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[open..open + close + 1]),
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out
}

/// Free lines from the template, each with its size, weight and opacity;
/// with `divider` the first line stands beside the others, a rule
/// between.
pub fn template_face(c: &ClockWidget, s: &ClockStrings, k: f32, align: f32, rtl: bool, m: &mut dyn Measure) -> Vec<Mark> {
    use size::*;
    let px = |v: f32| v * k;
    let role = c.look.font;
    let lines: Vec<&TemplateLine> = c.lines.iter().take(mandara_config::desktop_widgets::ranges::TEMPLATE_LINES).collect();
    let laid: Vec<_> = lines
        .iter()
        .map(|l| {
            let text = fill(&l.text, s);
            let text = if l.caps { caps(&text) } else { text };
            let st = style(role, px(l.size()), l.weight());
            line(vec![Run { opacity: l.opacity(), ..run(text, st, if l.accent { InkRole::Accent } else { InkRole::Fg }) }], 0.0, 0.0, rtl, m)
        })
        .filter(|(marks, _)| !marks.is_empty())
        .collect();
    if !c.face_options.divider || laid.len() < 2 {
        return stack(laid, px(TEMPLATE_GAP), align);
    }
    let mut laid = laid.into_iter();
    let Some((mut first, (fw, fh))) = laid.next() else { return Vec::new() };
    let rest: Vec<_> = laid.collect();
    let rest_w = rest.iter().map(|l| l.1.0).fold(0.0, f32::max);
    let rest_h: f32 = rest.iter().map(|l| l.1.1).sum::<f32>() + px(TEMPLATE_GAP) * (rest.len() as f32 - 1.0);
    let mut rest = stack(rest, px(TEMPLATE_GAP), if rtl { 1.0 } else { 0.0 });
    let h = fh.max(rest_h);
    let rule_w = px(DIVIDER_W);
    let (first_x, rule_x, rest_x) = if rtl { (rest_w + 2.0 * px(DIVIDER_GAP) + rule_w, rest_w + px(DIVIDER_GAP), 0.0) } else { (0.0, fw + px(DIVIDER_GAP), fw + 2.0 * px(DIVIDER_GAP) + rule_w) };
    for mk in &mut first {
        mk.shift(first_x, (h - fh) / 2.0);
    }
    for mk in &mut rest {
        mk.shift(rest_x, (h - rest_h) / 2.0);
    }
    let mut marks = first;
    marks.push(Mark::new(MarkKind::Line { a: (rule_x + rule_w / 2.0, h * 0.1), b: (rule_x + rule_w / 2.0, h * 0.9), width: rule_w }, InkRole::Accent));
    marks.extend(rest);
    marks
}

// ── Analog dials ─────────────────────────────────────────────────────────

/// A hand from the centre: a rounded line, or a blade (filled or outlined).
fn hand(c: (f32, f32), turn: f32, reach: f32, width: f32, style: HandStyle, ink: InkRole) -> Mark {
    let tip = at(c, turn, reach);
    match style {
        HandStyle::Classic => Mark::new(MarkKind::Line { a: c, b: tip, width }, ink),
        HandStyle::Fill | HandStyle::Hollow => {
            // A blade: widest a fifth of the way out, a rounded tail.
            let a = turn * TAU - FRAC_PI_2;
            let (dx, dy) = (a.cos(), a.sin());
            let (nx, ny) = (-dy, dx);
            let half = width * BLADE_WIDTH;
            let base = (c.0 + dx * reach * BLADE_WAIST, c.1 + dy * reach * BLADE_WAIST);
            let tail = (c.0 - dx * width, c.1 - dy * width);
            let pts = vec![tail, (base.0 + nx * half, base.1 + ny * half), tip, (base.0 - nx * half, base.1 - ny * half)];
            if style == HandStyle::Fill {
                Mark::new(MarkKind::Poly { pts }, ink)
            } else {
                let mut closed = pts;
                closed.push(tail);
                Mark::new(MarkKind::Polyline { pts: closed, width: width * HOLLOW_STROKE }, ink)
            }
        }
    }
}

/// How much wider than the line width a blade hand gets, where its widest
/// point sits along it, and the outline of a hollow one.
const BLADE_WIDTH: f32 = 1.4;
const BLADE_WAIST: f32 = 0.22;
const HOLLOW_STROKE: f32 = 0.32;

/// Analog dial reference sizes (the classic face's, so its default stays
/// exactly as it was).
mod dial {
    pub const R: f32 = 150.0;
    pub const RIM_W: f32 = 2.0;
    pub const RIM_ALPHA: f32 = 0.28;
    pub const MAJOR_TICK: f32 = 18.0;
    pub const MAJOR_W: f32 = 5.0;
    pub const MINOR_TICK: f32 = 10.0;
    pub const MINOR_W: f32 = 2.5;
    pub const TICK_INSET: f32 = 8.0;
    pub const NUMBER: f32 = 28.0;
    pub const NUMBER_INSET: f32 = 38.0;
    pub const HOUR_REACH: f32 = 0.52;
    pub const MINUTE_REACH: f32 = 0.8;
    pub const HOUR_REACH_NUMBERED: f32 = 0.4;
    pub const MINUTE_REACH_NUMBERED: f32 = 0.6;
    /// The hour hand is this much wider than the minute hand.
    pub const HOUR_TO_MINUTE_W: f32 = 10.0 / 6.0;
    pub const SECOND_REACH: f32 = 0.88;
    pub const SECOND_TAIL: f32 = 0.16;
    pub const SECOND_W: f32 = 2.5;
    pub const SECOND_DOT: f32 = 7.0;
    pub const CAP: f32 = 9.0;
    pub const SECOND_CAP: f32 = 5.0;
    pub const DATE: f32 = 24.0;
    pub const DATE_GAP: f32 = 24.0;
    pub const DOT_MAJOR: f32 = 6.0;
    pub const DOT_MINOR: f32 = 3.5;
    pub const BUBBLE: f32 = 26.0;
    pub const BUBBLE_TEXT: f32 = 22.0;
    pub const BUBBLE_AT: f32 = 0.55;
}

/// The analog face with its options. With every option at its default it
/// draws exactly what the first analog face drew.
pub fn analog_face(c: &ClockWidget, s: &ClockStrings, k: f32, role: FontRole, align: f32, m: &mut dyn Measure) -> Vec<Mark> {
    let o = &c.analog;
    let px = |v: f32| v * k;
    let r = px(dial::R);
    let centre = (r, r);
    let mut marks = dial_marks(c, o, s, k, centre, r, role, m);
    let on = if o.dial_fill { InkRole::OnPlate } else { InkRole::Fg };
    let numbered = (o.marks == DialMarks::Ticks && c.dial_numbers) || o.marks == DialMarks::Numbers;
    let (hour_reach, minute_reach) = if numbered { (dial::HOUR_REACH_NUMBERED, dial::MINUTE_REACH_NUMBERED) } else { (dial::HOUR_REACH, dial::MINUTE_REACH) };
    let minute_turn = (s.minute as f32 + s.second as f32 / MINUTES) / MINUTES;
    let hour_turn = ((s.hour % HOURS as u32) as f32 + s.minute as f32 / MINUTES) / HOURS as f32;
    let mw = px(o.hand_width());
    marks.push(hand(centre, hour_turn, r * hour_reach, mw * dial::HOUR_TO_MINUTE_W, o.hour_hand, on));
    marks.push(hand(centre, minute_turn, r * minute_reach, mw, if o.hour_hand == HandStyle::Hollow { HandStyle::Hollow } else { HandStyle::Classic }, on));
    marks.push(Mark::new(MarkKind::Disc { c: centre, r: px(dial::CAP) }, on));
    if c.seconds {
        let turn = s.second as f32 / MINUTES;
        match o.seconds_style {
            SecondsStyle::Hand => {
                let tail = at(centre, turn + 0.5, r * dial::SECOND_TAIL);
                marks.push(Mark::new(MarkKind::Line { a: tail, b: at(centre, turn, r * dial::SECOND_REACH), width: px(dial::SECOND_W) }, InkRole::Accent).live());
                marks.push(Mark::new(MarkKind::Disc { c: centre, r: px(dial::SECOND_CAP) }, InkRole::Accent).live());
            }
            SecondsStyle::Dot => marks.push(Mark::new(MarkKind::Disc { c: at(centre, turn, r * dial::SECOND_REACH), r: px(dial::SECOND_DOT) }, InkRole::Accent).live()),
        }
        // The live plane covers the whole sweep (the hand moves inside it).
        marks.push(Mark::new(MarkKind::Disc { c: centre, r: r * dial::SECOND_REACH + px(dial::SECOND_DOT) }, InkRole::Fg).dim(0.0).live());
    }
    if c.date {
        match o.date_style {
            DateStyle::Under => {
                let ds = style(role, px(dial::DATE), face::MEDIUM);
                let date = if s.day_month.is_empty() { s.weekday.clone() } else { format!("{}{}{}", s.weekday, face::SEPARATOR, s.day_month) };
                let (dw, dh) = m.measure(&date, &ds);
                let x = (2.0 * r - dw) * align;
                marks.push(Mark::new(MarkKind::Text { text: date, style: ds, x, y: 2.0 * r + px(dial::DATE_GAP), w: dw, h: dh }, InkRole::Fg).dim(face::DIM));
            }
            DateStyle::Bubble | DateStyle::Box => {
                // The day at three o'clock (nine in right-to-left text).
                let side = if s.rtl { 0.75 } else { 0.25 };
                let c0 = at(centre, side, r * dial::BUBBLE_AT);
                let b = px(dial::BUBBLE);
                let kind = if o.date_style == DateStyle::Bubble {
                    MarkKind::Disc { c: c0, r: b }
                } else {
                    MarkKind::Rect { x: c0.0 - b, y: c0.1 - b * 0.75, w: 2.0 * b, h: 1.5 * b, radius: b * 0.2, width: px(dial::RIM_W) }
                };
                let filled = o.date_style == DateStyle::Bubble;
                marks.push(Mark::new(kind, if filled { InkRole::Accent } else { on }));
                marks.push(centred(s.day.clone(), style(role, px(dial::BUBBLE_TEXT), face::BOLD), c0, m, if filled { InkRole::OnAccent } else { on }));
            }
        }
    }
    marks
}

/// The dial itself (its rim or filled shape) and its marks.
#[allow(clippy::too_many_arguments)]
fn dial_marks(c: &ClockWidget, o: &AnalogOptions, s: &ClockStrings, k: f32, centre: (f32, f32), r: f32, role: FontRole, m: &mut dyn Measure) -> Vec<Mark> {
    let px = |v: f32| v * k;
    let mut marks = Vec::new();
    let plain = o.dial == PlateShape::Circle && !o.dial_fill && o.dial_turn == 0.0;
    if plain {
        marks.push(Mark::new(MarkKind::Arc { c: centre, r: r - px(dial::RIM_W) / 2.0, width: px(dial::RIM_W), start: 0.0, sweep: TAU }, InkRole::Fg).dim(dial::RIM_ALPHA));
    } else {
        let shape = shape_of(o.dial, &c.look, k);
        let pts: Vec<(f32, f32)> = shape.outline(2.0 * r, 2.0 * r).into_iter().map(|p| rotate(p, centre, o.dial_turn)).collect();
        if o.dial_fill {
            marks.push(Mark::new(MarkKind::Poly { pts }, InkRole::Plate));
        } else {
            let mut closed = pts;
            if let Some(first) = closed.first().copied() {
                closed.push(first);
            }
            marks.push(Mark::new(MarkKind::Polyline { pts: closed, width: px(dial::RIM_W) }, InkRole::Fg).dim(dial::RIM_ALPHA));
        }
    }
    let on = if o.dial_fill { InkRole::OnPlate } else { InkRole::Fg };
    // Shapes with lobes pull in at the edge: marks sit a lobe deeper.
    let inset = match o.dial {
        PlateShape::Cookie | PlateShape::Flower => r * c.look.plate_depth(),
        _ => 0.0,
    };
    for i in 0..HOURS {
        let turn = i as f32 / HOURS as f32;
        let major = i % (HOURS / 4) == 0;
        match o.marks {
            DialMarks::None => {}
            DialMarks::Dots => {
                let d = if major { px(dial::DOT_MAJOR) } else { px(dial::DOT_MINOR) };
                marks.push(Mark::new(MarkKind::Disc { c: at(centre, turn, r - inset - px(dial::TICK_INSET) - px(dial::DOT_MAJOR)), r: d }, if major { InkRole::Accent } else { on }).dim(if major { 1.0 } else { face::DIM }));
            }
            DialMarks::Numbers => {
                let text = s.all_hours[i].clone();
                marks.push(centred(text, style(role, px(dial::NUMBER), face::MEDIUM), at(centre, turn, r - inset - px(dial::NUMBER_INSET)), m, if major { InkRole::Accent } else { on }));
            }
            DialMarks::Ticks => {
                if major && c.dial_numbers {
                    let text = s.hour_numbers[i / (HOURS / 4)].clone();
                    marks.push(centred(text, style(role, px(dial::NUMBER), face::MEDIUM), at(centre, turn, r - inset - px(dial::NUMBER_INSET)), m, on));
                    continue;
                }
                let (len, w) = if major { (px(dial::MAJOR_TICK), px(dial::MAJOR_W)) } else { (px(dial::MINOR_TICK), px(dial::MINOR_W)) };
                let outer = r - inset - px(dial::TICK_INSET);
                marks.push(Mark::new(MarkKind::Line { a: at(centre, turn, outer - len), b: at(centre, turn, outer), width: w }, if major { InkRole::Accent } else { on }).dim(if major { 1.0 } else { face::DIM }));
            }
        }
    }
    marks
}

// ── Numerals and orbit ───────────────────────────────────────────────────

/// Analog hands over the hour and the minute written as giant numerals.
pub fn numerals_face(c: &ClockWidget, s: &ClockStrings, k: f32, m: &mut dyn Measure) -> Vec<Mark> {
    use size::*;
    let px = |v: f32| v * k;
    let r = px(NUMERALS_R);
    let centre = (r, r);
    let ns = style(c.look.font, px(NUMERALS_SIZE), face::BLACK);
    let alpha = c.face_options.numerals();
    let (hw, hh) = m.measure(&s.hours, &ns);
    let (mw, mh) = m.measure(&s.minutes, &ns);
    // The hours up and to the start, the minutes down and to the end,
    // overlapping in the middle.
    let mut marks = vec![
        Mark::new(MarkKind::Text { text: s.hours.clone(), style: ns.clone(), x: r - hw * 0.8, y: r - hh * 0.86, w: hw, h: hh }, InkRole::Fg).dim(alpha),
        Mark::new(MarkKind::Text { text: s.minutes.clone(), style: ns, x: r - mw * 0.2, y: r - mh * 0.14, w: mw, h: mh }, InkRole::Accent).dim(alpha),
    ];
    let o = &c.analog;
    let minute_turn = s.minute as f32 / MINUTES;
    let hour_turn = ((s.hour % HOURS as u32) as f32 + s.minute as f32 / MINUTES) / HOURS as f32;
    let mw = px(o.hand_width());
    marks.push(hand(centre, hour_turn, r * dial::HOUR_REACH, mw * dial::HOUR_TO_MINUTE_W, o.hour_hand, InkRole::Fg));
    marks.push(hand(centre, minute_turn, r * dial::MINUTE_REACH, mw, if o.hour_hand == HandStyle::Hollow { HandStyle::Hollow } else { HandStyle::Classic }, InkRole::Fg));
    marks.push(Mark::new(MarkKind::Disc { c: centre, r: px(dial::CAP) }, InkRole::Accent));
    marks
}

/// The hour and the minute as bodies on two rings round a small sun.
pub fn orbit_face(c: &ClockWidget, s: &ClockStrings, k: f32, m: &mut dyn Measure) -> Vec<Mark> {
    use size::*;
    let px = |v: f32| v * k;
    let (ri, ro) = (px(ORBIT_INNER), px(ORBIT_OUTER));
    let pad = if c.face_options.labels { px(ORBIT_LABEL_GAP) + px(ORBIT_LABEL) } else { px(ORBIT_HOUR) };
    let centre = (ro + pad, ro + pad);
    let mut marks = Vec::new();
    for rr in [ri, ro] {
        marks.push(Mark::new(MarkKind::Arc { c: centre, r: rr, width: px(ORBIT_RING_W), start: 0.0, sweep: TAU }, InkRole::Fg).dim(ORBIT_RING_ALPHA));
    }
    marks.push(Mark::new(MarkKind::Disc { c: centre, r: px(ORBIT_SUN) }, InkRole::Accent).dim(face::DIM));
    let hour_turn = ((s.hour % HOURS as u32) as f32 + s.minute as f32 / MINUTES) / HOURS as f32;
    let minute_turn = s.minute as f32 / MINUTES;
    marks.push(Mark::new(MarkKind::Disc { c: at(centre, hour_turn, ri), r: px(ORBIT_HOUR) }, InkRole::Accent));
    marks.push(Mark::new(MarkKind::Disc { c: at(centre, minute_turn, ro), r: px(ORBIT_MINUTE) }, InkRole::Fg));
    marks.push(centred(s.time.clone(), style(c.look.font, px(ORBIT_TIME), face::SEMIBOLD), centre, m, InkRole::OnAccent));
    if c.face_options.labels {
        for (i, n) in s.hour_numbers.iter().enumerate() {
            let turn = i as f32 / 4.0;
            marks.push(centred(n.clone(), style(c.look.font, px(ORBIT_LABEL), face::MEDIUM), at(centre, turn, ro + px(ORBIT_LABEL_GAP)), m, InkRole::Fg).dim(face::DIM));
        }
    }
    marks
}

#[cfg(test)]
mod tests {
    use super::super::layout::tests::{Fixed, strings};
    use super::*;
    use mandara_config::desktop_widgets::ClockFace;

    #[test]
    fn templates_fill_known_tokens_and_keep_the_rest() {
        let s = strings();
        assert_eq!(fill("{weekday} │ {day} {month}", &s), "Saturday │ 22 August");
        assert_eq!(fill("{HH}:{mm} {nope} {", &s), "12:35 {nope} {");
    }

    #[test]
    fn every_glyph_has_its_rows() {
        for ch in "0123456789٠١٢٣٤٥٦٧٨٩:".chars() {
            let d = dot_glyph(ch).unwrap();
            assert_eq!(d.len(), 7, "{ch}");
            assert!(d.iter().all(|r| r.len() == d[0].len()), "{ch}");
            let p = pixel_glyph(ch).unwrap();
            assert_eq!(p.len(), 5, "{ch}");
            assert!(p.iter().all(|r| r.len() == p[0].len()), "{ch}");
        }
    }

    #[test]
    fn the_moon_is_dark_at_new_and_lit_at_full() {
        let area = |pts: &[(f32, f32)]| (0..pts.len()).map(|i| pts[i].0 * pts[(i + 1) % pts.len()].1 - pts[(i + 1) % pts.len()].0 * pts[i].1).sum::<f32>().abs() / 2.0;
        let lit = |phase: f32| match &moon((0.0, 0.0), 10.0, phase, InkRole::Fg)[1].kind {
            MarkKind::Poly { pts } => area(pts),
            _ => unreachable!(),
        };
        let disc = std::f32::consts::PI * 100.0;
        assert!(lit(0.0) < disc * 0.02 && lit(0.5) > disc * 0.97 && (lit(0.25) - disc / 2.0).abs() < disc * 0.03);
    }

    #[test]
    fn every_new_face_lays_out_inside_its_box() {
        for face in [ClockFace::Stack, ClockFace::Shape, ClockFace::DotMatrix, ClockFace::Pixel, ClockFace::Boxes, ClockFace::Hud, ClockFace::Template, ClockFace::Numerals, ClockFace::Orbit, ClockFace::Analog] {
            for divider in [false, true] {
                let mut c = ClockWidget { face, seconds: true, ..ClockWidget::default() };
                c.face_options.divider = divider;
                c.analog.marks = DialMarks::Numbers;
                c.analog.date_style = DateStyle::Bubble;
                let comp = super::super::layout::clock(&c, &strings(), 1.0, 0.0, false, &mut Fixed);
                assert!(comp.size.0 > 50.0 && comp.size.1 > 50.0, "{face:?}: {:?}", comp.size);
                for m in &comp.marks {
                    let b = m.bounds();
                    assert!(b.0 >= -0.01 && b.1 >= -0.01 && b.2 <= comp.size.0 + 0.01 && b.3 <= comp.size.1 + 0.01, "{face:?}: {m:?}");
                }
            }
        }
    }

    #[test]
    fn the_week_strip_sits_inside_the_boxes_clock_frame() {
        use mandara_config::desktop_widgets::{Look, Plate};
        let c = ClockWidget { face: ClockFace::Boxes, ..ClockWidget::default() };
        for k in [0.5, 1.0, 2.5] {
            for (padding, radius, outline) in [(0.0, 64.0, 2.0), (6.0, 24.0, 0.0), (24.0, 0.0, 4.0)] {
                let look = Look { plate: Plate::Solid, plate_padding: padding, plate_radius: radius, outline_width: outline, ..Look::default() };
                let comp = super::super::layout::with_plate(super::super::layout::clock(&c, &strings(), k, 0.0, false, &mut Fixed), &look, k, 0.0, &mut Fixed);
                let plate = comp.marks.iter().find_map(|m| match m.kind {
                    MarkKind::Plate { x, y, w, h, .. } => Some((x, y, w, h)),
                    _ => None,
                });
                let (px0, py0, pw, ph) = plate.expect("a plate");
                // The corners cut `r (1 − 1/√2)` in: the strip keeps clear.
                let r = (radius * k).min(pw.min(ph) / 2.0);
                let inset = r * (1.0 - std::f32::consts::FRAC_1_SQRT_2);
                for m in comp.marks.iter().filter(|m| m.opacity > 0.0 && !matches!(m.kind, MarkKind::Plate { .. } | MarkKind::Shaped { .. })) {
                    let b = m.bounds();
                    assert!(b.0 >= px0 + inset - 0.01 && b.2 <= px0 + pw - inset + 0.01, "k={k} pad={padding}: {m:?} sticks out sideways");
                    assert!(b.3 <= py0 + ph - inset + 0.01, "k={k} pad={padding} r={radius}: {m:?} runs into the frame's bottom");
                }
            }
        }
    }
}
