//! Layout of the painted desktop widgets as lists of [`Mark`]s in physical
//! pixels, relative to the widget's own box. Pure functions over a
//! [`Measure`] (the live [`crate::lock::paint::TextPainter`], or a fixed
//! advance in tests), shared by the live wallpaper renderer, the offscreen
//! preview and Studio's thumbnails.
//!
//! Every size is a *reference* pixel at a 1080 px tall display (the
//! `REF_*` consts) multiplied by `k` (physical px per reference px). Marks
//! flagged `live` go to the widget's second plane, the only one redrawn
//! between minutes (seconds, system readings).

use std::f32::consts::{FRAC_PI_2, TAU};

use mandara_config::desktop_widgets::{ClockFace, ClockWidget, DecorShape, Decoration, FontRole, MonitorRow, MonitorWidget, RingProgress};

/// What a text mark is set in.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub role: FontRole,
    /// Physical px.
    pub size: f32,
    pub weight: u16,
    /// Letter spacing, em.
    pub tracking: f32,
}

/// Measures text: `(advance width, line height)` in physical px.
pub trait Measure {
    fn measure(&mut self, text: &str, style: &TextStyle) -> (f32, f32);
}

/// Which colour a mark takes (resolved by the painter).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum InkRole {
    Fg,
    Accent,
    /// Text on an accent-filled shape.
    OnAccent,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MarkKind {
    /// Top-left at `(x, y)`, measured `w × h`.
    Text { text: String, style: TextStyle, x: f32, y: f32, w: f32, h: f32 },
    /// A shaped line turned a quarter (vertical text's sideways runs):
    /// the turned box at `(x, y)`, `w` wide (the line's height) and `h`
    /// tall (its advance).
    SideText { text: String, style: TextStyle, x: f32, y: f32, w: f32, h: f32, clockwise: bool },
    /// A filled polygon.
    Poly { pts: Vec<(f32, f32)> },
    Line { a: (f32, f32), b: (f32, f32), width: f32 },
    /// A stroked arc, radians clockwise from 3 o'clock.
    Arc { c: (f32, f32), r: f32, width: f32, start: f32, sweep: f32 },
    Disc { c: (f32, f32), r: f32 },
    /// Filled when `width` is 0, else an outline.
    Rect { x: f32, y: f32, w: f32, h: f32, radius: f32, width: f32 },
    /// A horizontal sine line `waves` periods long.
    Wave { x: f32, y: f32, w: f32, amp: f32, waves: f32, width: f32 },
    /// A bundled icon.
    Icon { name: &'static str, x: f32, y: f32, size: f32 },
    /// A sparkline of `values` (0..=1, oldest first) filling the box.
    Graph { x: f32, y: f32, w: f32, h: f32, values: Vec<f32>, width: f32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub kind: MarkKind,
    pub ink: InkRole,
    pub opacity: f32,
    /// Drawn in the widget's live plane.
    pub live: bool,
}

impl Mark {
    fn new(kind: MarkKind, ink: InkRole) -> Self {
        Self { kind, ink, opacity: 1.0, live: false }
    }

    fn dim(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }

    fn live(mut self) -> Self {
        self.live = true;
        self
    }

    /// Axis-aligned bounds `(x0, y0, x1, y1)` including stroke widths.
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        match &self.kind {
            MarkKind::Text { x, y, w, h, .. } | MarkKind::SideText { x, y, w, h, .. } => (*x, *y, x + w, y + h),
            MarkKind::Poly { pts } => pts.iter().fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |b, p| (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1))),
            MarkKind::Line { a, b, width } => {
                let h = width / 2.0;
                (a.0.min(b.0) - h, a.1.min(b.1) - h, a.0.max(b.0) + h, a.1.max(b.1) + h)
            }
            MarkKind::Arc { c, r, width, .. } => {
                let e = r + width / 2.0;
                (c.0 - e, c.1 - e, c.0 + e, c.1 + e)
            }
            MarkKind::Disc { c, r } => (c.0 - r, c.1 - r, c.0 + r, c.1 + r),
            MarkKind::Rect { x, y, w, h, width, .. } => (x - width / 2.0, y - width / 2.0, x + w + width / 2.0, y + h + width / 2.0),
            MarkKind::Wave { x, y, w, amp, width, .. } => (*x, y - amp - width, x + w, y + amp + width),
            MarkKind::Icon { x, y, size, .. } => (*x, *y, x + size, y + size),
            MarkKind::Graph { x, y, w, h, width, .. } => (*x, y - width, x + w, y + h + width),
        }
    }

    fn shift(&mut self, dx: f32, dy: f32) {
        let mv = |p: &mut (f32, f32)| {
            p.0 += dx;
            p.1 += dy;
        };
        match &mut self.kind {
            MarkKind::Text { x, y, .. } | MarkKind::SideText { x, y, .. } | MarkKind::Rect { x, y, .. } | MarkKind::Wave { x, y, .. } | MarkKind::Icon { x, y, .. } | MarkKind::Graph { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
            MarkKind::Poly { pts } => pts.iter_mut().for_each(mv),
            MarkKind::Line { a, b, .. } => {
                mv(a);
                mv(b);
            }
            MarkKind::Arc { c, .. } | MarkKind::Disc { c, .. } => mv(c),
        }
    }
}

/// A laid-out widget: its marks and its box size (physical px).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Composition {
    pub marks: Vec<Mark>,
    pub size: (f32, f32),
}

impl Composition {
    /// Union of the bounds of the marks with `live == live`, if any.
    pub fn bounds(&self, live: bool) -> Option<(f32, f32, f32, f32)> {
        self.marks.iter().filter(|m| m.live == live).map(Mark::bounds).reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
    }

    /// Moves everything so the marks start at the origin and recomputes
    /// the size.
    fn normalize(&mut self) {
        let all = self.marks.iter().map(Mark::bounds).reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)));
        let Some((x0, y0, x1, y1)) = all else {
            self.size = (0.0, 0.0);
            return;
        };
        for m in &mut self.marks {
            m.shift(-x0, -y0);
        }
        self.size = ((x1 - x0).max(0.0), (y1 - y0).max(0.0));
    }
}

// ── Clock ────────────────────────────────────────────────────────────────

/// Every string a clock face may show, formatted once per tick (localized,
/// digits already converted when Eastern Arabic numerals are on).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClockStrings {
    pub hours: String,
    pub minutes: String,
    pub seconds: String,
    /// `AM` / `PM` on 12-hour clocks.
    pub meridiem: Option<String>,
    /// `HH:MM`.
    pub time: String,
    pub weekday: String,
    pub weekday_short: String,
    pub day: String,
    pub month: String,
    /// `09/04` (day / month in the order of the language).
    pub numeric_date: String,
    /// `22 August`.
    pub day_month: String,
    /// `Good night`.
    pub greeting: String,
    /// `(icon, temperature)` when weather is on and known.
    pub weather: Option<(&'static str, String)>,
    /// Fraction of the hour / of the day gone.
    pub hour_fraction: f32,
    pub day_fraction: f32,
}

/// Reference sizes of the faces (px at a 1080 px tall display).
mod face {
    pub const SUPER_DIGITS: f32 = 230.0;
    pub const SUPER_LINE: f32 = 0.8;
    pub const SUPER_SMALL: f32 = 46.0;
    pub const SUPER_GAP: f32 = 14.0;
    pub const SUPER_SMALL_DROP: f32 = 0.18;
    pub const HEADLINE_TIME: f32 = 104.0;
    pub const HEADLINE_SMALL: f32 = 36.0;
    pub const HEADLINE_GAP: f32 = 18.0;
    pub const DAY_WEEKDAY: f32 = 92.0;
    pub const DAY_ICON: f32 = 46.0;
    pub const DAY_TEMP: f32 = 42.0;
    pub const DAY_GAP: f32 = 18.0;
    pub const DAY_DIVIDER_W: f32 = 3.0;
    pub const DAY_SLANT: f32 = 0.3;
    pub const DAY_TIME: f32 = 50.0;
    pub const DAY_BOX_PAD_X: f32 = 22.0;
    pub const DAY_BOX_PAD_Y: f32 = 6.0;
    pub const DAY_DATE: f32 = 20.0;
    pub const DAY_ROW_GAP: f32 = 12.0;
    pub const RING_R: f32 = 140.0;
    pub const RING_W: f32 = 7.0;
    pub const RING_TRACK_ALPHA: f32 = 0.2;
    pub const RING_TIME: f32 = 70.0;
    pub const RING_DATE: f32 = 24.0;
    pub const RING_DATE_GAP: f32 = 26.0;
    pub const GREET_SMALL: f32 = 22.0;
    pub const GREET_DAY: f32 = 78.0;
    pub const GREET_TIME: f32 = 44.0;
    pub const GREET_GAP: f32 = 14.0;
    pub const GREET_RULE_GAP: f32 = 30.0;
    pub const GREET_RULE_W: f32 = 1.5;
    pub const GREET_TRACKING: f32 = 0.32;
    pub const NUMERAL: f32 = 330.0;
    pub const NUMERAL_MONTH: f32 = 76.0;
    pub const NUMERAL_MONTH_AT: f32 = 0.56;
    pub const NUMERAL_TIME: f32 = 30.0;
    pub const NUMERAL_TRACKING: f32 = 0.28;
    pub const NUMERAL_ALPHA: f32 = 0.92;
    pub const MIN_TIME: f32 = 128.0;
    pub const MIN_DATE: f32 = 28.0;
    pub const MIN_GAP: f32 = 4.0;
    pub const DIM: f32 = 0.72;
    pub const SEPARATOR: &str = " · ";
    pub const DASH: &str = " — ";
    /// Weights.
    pub const HEAVY: u16 = 800;
    pub const BLACK: u16 = 900;
    pub const BOLD: u16 = 700;
    pub const SEMIBOLD: u16 = 600;
    pub const MEDIUM: u16 = 500;
    pub const REGULAR: u16 = 400;
    pub const LIGHT: u16 = 300;
    pub const THIN: u16 = 200;
}

/// One horizontal run of a line.
struct Run {
    text: String,
    style: TextStyle,
    ink: InkRole,
    opacity: f32,
    live: bool,
    /// Gap after the run, physical px.
    gap: f32,
}

fn run(text: impl Into<String>, style: TextStyle, ink: InkRole) -> Run {
    Run { text: text.into(), style, ink, opacity: 1.0, live: false, gap: 0.0 }
}

/// Lays out runs left to right (right to left when `rtl`) with their
/// baselines roughly aligned (bottoms of the line boxes), top at `y`.
/// Returns the marks and the line's `(width, height)`.
fn line(runs: Vec<Run>, x: f32, y: f32, rtl: bool, m: &mut dyn Measure) -> (Vec<Mark>, (f32, f32)) {
    let sized: Vec<(Run, (f32, f32))> = runs.into_iter().filter(|r| !r.text.is_empty()).map(|r| {
        let s = m.measure(&r.text, &r.style);
        (r, s)
    }).collect();
    let height = sized.iter().map(|(_, s)| s.1).fold(0.0, f32::max);
    let width: f32 = sized.iter().enumerate().map(|(i, (r, s))| s.0 + if i + 1 < sized.len() { r.gap } else { 0.0 }).sum();
    let mut cx = if rtl { x + width } else { x };
    let mut out = Vec::new();
    let n = sized.len();
    for (i, (r, (w, h))) in sized.into_iter().enumerate() {
        let gap = if i + 1 < n { r.gap } else { 0.0 };
        let left = if rtl { cx - w } else { cx };
        let mut mark = Mark::new(MarkKind::Text { text: r.text, style: r.style, x: left, y: y + height - h, w, h }, r.ink).dim(r.opacity);
        mark.live = r.live;
        out.push(mark);
        cx = if rtl { left - gap } else { cx + w + gap };
    }
    (out, (width, height))
}

/// Stacks lines, each aligned by `align` (0 left, ½ centre, 1 right).
fn stack(lines: Vec<(Vec<Mark>, (f32, f32))>, gap: f32, align: f32) -> Vec<Mark> {
    let width = lines.iter().map(|(_, s)| s.0).fold(0.0, f32::max);
    let mut y = 0.0;
    let mut out = Vec::new();
    for (mut marks, (w, h)) in lines {
        let dx = (width - w) * align;
        for m in &mut marks {
            m.shift(dx, y);
        }
        out.extend(marks);
        y += h + gap;
    }
    out
}

fn style(role: FontRole, size: f32, weight: u16) -> TextStyle {
    TextStyle { role, size, weight, tracking: 0.0 }
}

fn tracked(mut s: TextStyle, tracking: f32) -> TextStyle {
    s.tracking = tracking;
    s
}

/// Upper-cases Latin text (scripts without case are unchanged).
fn caps(s: &str) -> String {
    s.to_uppercase()
}

/// Lays out a clock `face` (not the depth faces, which have their own
/// renderer) at `k` physical px per reference px, lines aligned by `align`.
pub fn clock(c: &ClockWidget, s: &ClockStrings, k: f32, align: f32, rtl: bool, m: &mut dyn Measure) -> Composition {
    use face::*;
    let role = c.look.font;
    let px = |v: f32| v * k;
    let mut marks = Vec::new();
    match c.face {
        ClockFace::Superscript | ClockFace::Stacked | ClockFace::Classic | ClockFace::Arabic => {
            let digits = style(role, px(SUPER_DIGITS), HEAVY);
            let (h_marks, (hw, hh)) = line(vec![run(&s.hours, digits.clone(), InkRole::Fg)], 0.0, 0.0, false, m);
            let (mut m_marks, (mw, _)) = line(vec![run(&s.minutes, digits, InkRole::Fg)], 0.0, 0.0, false, m);
            let top = hh * SUPER_LINE;
            for mk in &mut m_marks {
                mk.shift(0.0, top);
            }
            let block_w = hw.max(mw);
            let small = style(role, px(SUPER_SMALL), SEMIBOLD);
            // Seconds (live) or the AM/PM marker beside the hours.
            let sup = if c.seconds { Some((s.seconds.clone(), true)) } else { s.meridiem.clone().map(|t| (t, false)) };
            marks.extend(h_marks);
            marks.extend(m_marks);
            if let Some((text, live)) = sup {
                let (w, h) = m.measure(&text, &small);
                // Seconds keep a fixed box (tabular digits): "00" wide.
                let w = if live { m.measure("00", &small).0.max(w) } else { w };
                let x = if rtl { -px(SUPER_GAP) - w } else { block_w + px(SUPER_GAP) };
                let mut mark = Mark::new(MarkKind::Text { text, style: small, x, y: hh * SUPER_SMALL_DROP, w, h }, InkRole::Accent);
                mark.live = live;
                marks.push(mark);
            }
        }
        ClockFace::Headline => {
            let big = style(role, px(HEADLINE_TIME), SEMIBOLD);
            let light = style(role, px(HEADLINE_TIME), THIN);
            let small = style(role, px(HEADLINE_SMALL), MEDIUM);
            let mut runs = vec![Run { gap: px(HEADLINE_GAP) / 2.0, ..run(&s.time, big, InkRole::Fg) }];
            if let Some(mer) = &s.meridiem {
                runs.push(Run { gap: 0.0, ..run(mer, small, InkRole::Accent) });
            }
            runs.push(Run { opacity: DIM, ..run(DASH, light.clone(), InkRole::Fg) });
            runs.push(run(&s.numeric_date, light, InkRole::Fg));
            marks.extend(line(runs, 0.0, 0.0, rtl, m).0);
        }
        ClockFace::DayBlock => {
            let weekday = style(role, px(DAY_WEEKDAY), BLACK);
            let (wd_w, wd_h) = m.measure(&caps(&s.weekday), &weekday);
            let mut x = 0.0;
            let mut row1 = Vec::new();
            if let Some((icon, temp)) = &s.weather {
                let icon_size = px(DAY_ICON);
                row1.push(Mark::new(MarkKind::Icon { name: icon, x, y: (wd_h - icon_size) / 2.0, size: icon_size }, InkRole::Fg));
                x += icon_size + px(DAY_GAP) / 2.0;
                let ts = style(role, px(DAY_TEMP), MEDIUM);
                let (tw, th) = m.measure(temp, &ts);
                row1.push(Mark::new(MarkKind::Text { text: temp.clone(), style: ts, x, y: (wd_h - th) / 2.0, w: tw, h: th }, InkRole::Fg));
                x += tw + px(DAY_GAP);
                // The slanted divider.
                let lean = wd_h * DAY_SLANT;
                row1.push(Mark::new(MarkKind::Line { a: (x, wd_h * 0.92), b: (x + lean, wd_h * 0.08), width: px(DAY_DIVIDER_W) }, InkRole::Accent));
                x += lean + px(DAY_GAP);
            }
            row1.push(Mark::new(MarkKind::Text { text: caps(&s.weekday), style: weekday, x, y: 0.0, w: wd_w, h: wd_h }, InkRole::Fg));
            let row1_w = x + wd_w;
            // The time in a slanted solid box.
            let ts = style(role, px(DAY_TIME), BOLD);
            let (tw, th) = m.measure(&s.time, &ts);
            let (bw, bh) = (tw + 2.0 * px(DAY_BOX_PAD_X), th + 2.0 * px(DAY_BOX_PAD_Y));
            let top = wd_h + px(DAY_ROW_GAP);
            let box_x = (row1_w - bw).max(0.0) * align;
            let (box_pts, _) = skewed_box(box_x, top, bw, bh, DAY_SLANT);
            let mut row2 = vec![Mark::new(MarkKind::Poly { pts: box_pts }, InkRole::Accent)];
            row2.push(Mark::new(MarkKind::Text { text: s.time.clone(), style: ts, x: box_x + (bw - tw) / 2.0, y: top + (bh - th) / 2.0, w: tw, h: th }, InkRole::OnAccent));
            let ds = tracked(style(role, px(DAY_DATE), MEDIUM), GREET_TRACKING / 2.0);
            let date = caps(&format!("{} {}", s.day, s.month));
            let (dw, dh) = m.measure(&date, &ds);
            let dy = top + bh + px(DAY_ROW_GAP);
            row2.push(Mark::new(MarkKind::Text { text: date, style: ds, x: (row1_w.max(bw) - dw) * align, y: dy, w: dw, h: dh }, InkRole::Fg).dim(DIM));
            if rtl {
                // Mirror the first row so the weekday leads from the right.
                let w = row1_w;
                for mk in &mut row1 {
                    let (x0, _, x1, _) = mk.bounds();
                    mk.shift(w - x1 - x0, 0.0);
                }
            }
            marks.extend(row1);
            marks.extend(row2);
        }
        ClockFace::Ring => {
            let (r, w) = (px(RING_R), px(RING_W));
            marks.extend(ring_face(s, k, role, (r + w, r + w), r, w, c.ring_progress, m));
        }
        ClockFace::Greeting => {
            let small = tracked(style(role, px(GREET_SMALL), SEMIBOLD), GREET_TRACKING);
            let lines = vec![
                line(vec![run(caps(&s.greeting), small.clone(), InkRole::Accent)], 0.0, 0.0, rtl, m),
                line(vec![run(caps(&s.weekday_short), style(role, px(GREET_DAY), HEAVY), InkRole::Fg)], 0.0, 0.0, rtl, m),
                line(vec![run(caps(&s.day_month), small, InkRole::Fg)], 0.0, 0.0, rtl, m),
                line(vec![run(&s.time, style(role, px(GREET_TIME), LIGHT), InkRole::Fg)], 0.0, 0.0, rtl, m),
            ];
            let col_w = lines.iter().map(|(_, s)| s.0).fold(0.0, f32::max);
            let col_h: f32 = lines.iter().map(|(_, s)| s.1).sum::<f32>() + px(GREET_GAP) * (lines.len() as f32 - 1.0);
            let mut col = stack(lines, px(GREET_GAP), 0.5);
            let rule_x = px(GREET_RULE_GAP);
            for mk in &mut col {
                mk.shift(rule_x, 0.0);
            }
            let rw = px(GREET_RULE_W);
            marks.push(Mark::new(MarkKind::Line { a: (rw / 2.0, 0.0), b: (rw / 2.0, col_h), width: rw }, InkRole::Fg).dim(DIM));
            let right = rule_x * 2.0 + col_w - rw / 2.0;
            marks.push(Mark::new(MarkKind::Line { a: (right, 0.0), b: (right, col_h), width: rw }, InkRole::Fg).dim(DIM));
            marks.extend(col);
        }
        ClockFace::DayNumeral => {
            let big = style(role, px(NUMERAL), BLACK);
            let (nw, nh) = m.measure(&s.day, &big);
            let ms = tracked(style(role, px(NUMERAL_MONTH), LIGHT), NUMERAL_TRACKING);
            let month = caps(&s.month);
            let (mw, mh) = m.measure(&month, &ms);
            let w = nw.max(mw);
            marks.push(Mark::new(MarkKind::Text { text: s.day.clone(), style: big, x: (w - nw) / 2.0, y: 0.0, w: nw, h: nh }, InkRole::Fg).dim(NUMERAL_ALPHA));
            marks.push(Mark::new(MarkKind::Text { text: month, style: ms, x: (w - mw) / 2.0, y: nh * NUMERAL_MONTH_AT - mh / 2.0, w: mw, h: mh }, InkRole::Accent));
            let ts = tracked(style(role, px(NUMERAL_TIME), MEDIUM), NUMERAL_TRACKING / 2.0);
            let (tw, th) = m.measure(&s.time, &ts);
            marks.push(Mark::new(MarkKind::Text { text: s.time.clone(), style: ts, x: (w - tw) / 2.0, y: nh, w: tw, h: th }, InkRole::Fg).dim(DIM));
        }
        ClockFace::Minimal => {
            let time = line(vec![run(&s.time, style(role, px(MIN_TIME), LIGHT), InkRole::Fg)], 0.0, 0.0, rtl, m);
            let ds = style(role, px(MIN_DATE), REGULAR);
            let date = line(
                vec![
                    Run { ..run(&s.weekday, TextStyle { weight: SEMIBOLD, ..ds.clone() }, InkRole::Accent) },
                    Run { opacity: DIM, ..run(format!("{SEPARATOR}{}", s.day_month), ds, InkRole::Fg) },
                ],
                0.0,
                0.0,
                rtl,
                m,
            );
            marks.extend(stack(vec![time, date], px(MIN_GAP), align));
        }
    }
    let mut comp = Composition { marks, size: (0.0, 0.0) };
    comp.normalize();
    comp
}

/// The ring face: track, progress arc, time inside, date below.
#[allow(clippy::too_many_arguments)]
pub fn ring_face(s: &ClockStrings, k: f32, role: FontRole, c: (f32, f32), r: f32, w: f32, progress: RingProgress, m: &mut dyn Measure) -> Vec<Mark> {
    use face::*;
    let px = |v: f32| v * k;
    let frac = match progress {
        RingProgress::Hour => s.hour_fraction,
        RingProgress::Day => s.day_fraction,
    }
    .clamp(0.0, 1.0);
    let mut marks = vec![Mark::new(MarkKind::Arc { c, r, width: w, start: -FRAC_PI_2, sweep: TAU }, InkRole::Fg).dim(RING_TRACK_ALPHA)];
    if frac > 0.0 {
        marks.push(Mark::new(MarkKind::Arc { c, r, width: w, start: -FRAC_PI_2, sweep: TAU * frac }, InkRole::Accent));
    }
    let ts = style(role, px(RING_TIME), SEMIBOLD);
    let (tw, th) = m.measure(&s.time, &ts);
    marks.push(Mark::new(MarkKind::Text { text: s.time.clone(), style: ts, x: c.0 - tw / 2.0, y: c.1 - th / 2.0, w: tw, h: th }, InkRole::Fg));
    let ds = style(role, px(RING_DATE), MEDIUM);
    let date = format!("{}{SEPARATOR}{}", s.weekday, s.day_month);
    let (dw, dh) = m.measure(&date, &ds);
    marks.push(Mark::new(MarkKind::Text { text: date, style: ds, x: c.0 - dw / 2.0, y: c.1 + r + w + px(RING_DATE_GAP), w: dw, h: dh }, InkRole::Fg).dim(DIM));
    marks
}

/// A parallelogram `w × h` at `(x, y)` leaning right by `slant` (run per
/// rise); returns its corners (clockwise from top-left) and the lean.
pub fn skewed_box(x: f32, y: f32, w: f32, h: f32, slant: f32) -> (Vec<(f32, f32)>, f32) {
    let lean = h * slant;
    (vec![(x + lean, y), (x + w + lean, y), (x + w, y + h), (x, y + h)], lean)
}

// ── System monitor ───────────────────────────────────────────────────────

/// One row's texts, formatted per sample.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MonitorLine {
    pub row: Option<MonitorRow>,
    pub label: String,
    pub value: String,
    /// A thin bar under the row (0..=1).
    pub bar: Option<f32>,
    /// A sparkline (and an optional second series) instead of a value.
    pub graph: Option<(Vec<f32>, Option<Vec<f32>>)>,
}

mod monitor {
    pub const TEXT: f32 = 17.0;
    pub const TITLE: f32 = 13.0;
    pub const ROW_GAP: f32 = 9.0;
    pub const BAR_H: f32 = 3.0;
    pub const BAR_GAP: f32 = 5.0;
    pub const GRAPH_H: f32 = 34.0;
    pub const GRAPH_W: f32 = 1.5;
    pub const TITLE_GAP: f32 = 12.0;
    pub const TRACKING: f32 = 0.06;
    pub const TITLE_TRACKING: f32 = 0.3;
    pub const LABEL_ALPHA: f32 = 0.66;
    pub const TRACK_ALPHA: f32 = 0.18;
    pub const WEIGHT: u16 = 500;
}

/// The monitor column. Labels and the title are static; values, bars and
/// graphs are live.
pub fn monitor(w: &MonitorWidget, lines: &[MonitorLine], k: f32, rtl: bool, m: &mut dyn Measure) -> Composition {
    use monitor::*;
    let px = |v: f32| v * k;
    let role = w.look.font;
    let width = px(w.width.clamp(mandara_config::desktop_widgets::ranges::MONITOR_WIDTH_PX.0, mandara_config::desktop_widgets::ranges::MONITOR_WIDTH_PX.1));
    let text = tracked(style(role, px(TEXT), WEIGHT), TRACKING);
    let mut marks = Vec::new();
    let mut y = 0.0;
    if !w.title.trim().is_empty() {
        let ts = tracked(style(role, px(TITLE), face::BOLD), TITLE_TRACKING);
        let t = caps(w.title.trim());
        let (tw, th) = m.measure(&t, &ts);
        marks.push(Mark::new(MarkKind::Text { text: t, style: ts, x: if rtl { width - tw } else { 0.0 }, y, w: tw, h: th }, InkRole::Accent));
        y += th + px(TITLE_GAP);
    }
    let (_, line_h) = m.measure("0", &text);
    for l in lines {
        if let Some((a, b)) = &l.graph {
            let h = px(GRAPH_H);
            marks.push(Mark::new(MarkKind::Graph { x: 0.0, y, w: width, h, values: a.clone(), width: px(GRAPH_W) }, InkRole::Accent).live());
            if let Some(b) = b {
                marks.push(Mark::new(MarkKind::Graph { x: 0.0, y, w: width, h, values: b.clone(), width: px(GRAPH_W) }, InkRole::Fg).dim(LABEL_ALPHA).live());
            }
            y += h + px(ROW_GAP);
            continue;
        }
        let (lw, lh) = m.measure(&l.label, &text);
        let (vw, vh) = m.measure(&l.value, &text);
        let (lx, vx) = if rtl { (width - lw, 0.0) } else { (0.0, width - vw) };
        marks.push(Mark::new(MarkKind::Text { text: l.label.clone(), style: text.clone(), x: lx, y, w: lw, h: lh }, InkRole::Fg).dim(LABEL_ALPHA));
        marks.push(Mark::new(MarkKind::Text { text: l.value.clone(), style: text.clone(), x: vx, y, w: vw, h: vh }, InkRole::Fg).live());
        y += line_h;
        if let Some(f) = l.bar {
            let bh = px(BAR_H);
            let by = y + px(BAR_GAP) - px(ROW_GAP) / 2.0;
            marks.push(Mark::new(MarkKind::Rect { x: 0.0, y: by, w: width, h: bh, radius: bh / 2.0, width: 0.0 }, InkRole::Fg).dim(TRACK_ALPHA).live());
            let fw = width * f.clamp(0.0, 1.0);
            if fw > 0.0 {
                let x = if rtl { width - fw } else { 0.0 };
                marks.push(Mark::new(MarkKind::Rect { x, y: by, w: fw, h: bh, radius: bh / 2.0, width: 0.0 }, InkRole::Accent).live());
            }
            y += px(BAR_GAP) + bh;
        }
        y += px(ROW_GAP);
    }
    // A fixed box: the full width, whatever the values measure.
    marks.push(Mark::new(MarkKind::Rect { x: 0.0, y: 0.0, w: width, h: (y - px(ROW_GAP)).max(0.0), radius: 0.0, width: 0.0 }, InkRole::Fg).dim(0.0));
    let mut comp = Composition { marks, size: (0.0, 0.0) };
    comp.normalize();
    comp
}

// ── Decorations ──────────────────────────────────────────────────────────

/// Gap between vertical text columns, em.
const VERTICAL_COLUMN_GAP_EM: f32 = 0.35;
/// Vertical text advance per character, em (CJK ideographs are square).
const VERTICAL_ADVANCE_EM: f32 = 1.08;
/// Wave amplitude as a fraction of the gap between lines.
const WAVE_AMP: f32 = 0.28;
/// Periods per wave line, per 100 reference px of width.
const WAVE_PERIODS_PER_100PX: f32 = 0.9;
/// Gap between stripes, fraction of a stripe slot.
const STRIPE_GAP: f32 = 0.45;

/// A decoration (unrotated: the painter rotates the plane's content).
/// `clockwise`: which way vertical text turns its sideways runs.
pub fn decoration(d: &Decoration, k: f32, clockwise: bool, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let r = mandara_config::desktop_widgets::ranges::SHAPE_SIZE_PX;
    let (w, h) = (px(d.width.clamp(r.0, r.1)), px(d.height.clamp(r.0, r.1)));
    let sr = mandara_config::desktop_widgets::ranges::STROKE_PX;
    let stroke = px(d.stroke.clamp(sr.0, sr.1));
    let rr = mandara_config::desktop_widgets::ranges::REPEAT;
    let count = d.count.clamp(rr.0, rr.1) as usize;
    let mut marks = Vec::new();
    match d.shape {
        DecorShape::Text => {
            let tr = mandara_config::desktop_widgets::ranges::TEXT_SIZE_PX;
            let size = px(d.size.clamp(tr.0, tr.1));
            let wr = mandara_config::desktop_widgets::ranges::WEIGHT;
            let ts = TextStyle { role: d.look.font, size, weight: d.weight.clamp(wr.0, wr.1), tracking: d.tracking };
            if d.vertical {
                marks.extend(vertical_text(&d.text, &ts, Vertical { clockwise, upright_latin: d.upright_latin }, m));
            } else {
                let lines: Vec<_> = d.text.lines().map(|l| line(vec![run(l, ts.clone(), InkRole::Fg)], 0.0, 0.0, false, m)).collect();
                marks.extend(stack(lines, 0.0, 0.5));
            }
        }
        DecorShape::Disc => marks.push(Mark::new(MarkKind::Disc { c: (w / 2.0, w / 2.0), r: w / 2.0 }, InkRole::Fg)),
        DecorShape::Circle => marks.push(Mark::new(MarkKind::Arc { c: (w / 2.0, w / 2.0), r: (w - stroke) / 2.0, width: stroke, start: 0.0, sweep: TAU }, InkRole::Fg)),
        DecorShape::Stripes => {
            let slot = h / count as f32;
            let bar = slot * (1.0 - STRIPE_GAP);
            for i in 0..count {
                marks.push(Mark::new(MarkKind::Rect { x: 0.0, y: i as f32 * slot, w, h: bar, radius: 0.0, width: 0.0 }, InkRole::Fg));
            }
        }
        DecorShape::Waves => {
            let slot = h / count as f32;
            let amp = slot * WAVE_AMP;
            let waves = (d.width / 100.0 * WAVE_PERIODS_PER_100PX).max(0.5);
            for i in 0..count {
                marks.push(Mark::new(MarkKind::Wave { x: 0.0, y: (i as f32 + 0.5) * slot, w, amp, waves, width: stroke }, InkRole::Fg));
            }
        }
        DecorShape::Rule => marks.push(Mark::new(MarkKind::Line { a: (0.0, stroke / 2.0), b: (w, stroke / 2.0), width: stroke }, InkRole::Fg)),
        DecorShape::Frame => marks.push(Mark::new(MarkKind::Rect { x: stroke / 2.0, y: stroke / 2.0, w: w - stroke, h: h - stroke, radius: 0.0, width: stroke }, InkRole::Fg)),
    }
    let mut comp = Composition { marks, size: (0.0, 0.0) };
    comp.normalize();
    comp
}

/// How vertical text treats the runs that don't stand upright.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Vertical {
    /// Sideways runs turn clockwise (else counter-clockwise).
    pub clockwise: bool,
    /// Latin letters and digits stand upright like CJK.
    pub upright_latin: bool,
}

/// Code points that stand upright in vertical text (Unicode's
/// `Vertical_Orientation=U` blocks): Hangul Jamo, the CJK radicals through
/// Yi (kana, bopomofo, CJK symbols and punctuation, ideographs), Hangul
/// syllables, CJK compatibility ideographs and forms, full-width forms,
/// and the supplementary ideograph planes.
const UPRIGHT_RANGES: [std::ops::RangeInclusive<char>; 7] = [
    '\u{1100}'..='\u{11FF}',
    '\u{2E80}'..='\u{A4CF}',
    '\u{AC00}'..='\u{D7AF}',
    '\u{F900}'..='\u{FAFF}',
    '\u{FE30}'..='\u{FE4F}',
    '\u{FF00}'..='\u{FFEF}',
    '\u{20000}'..='\u{3FFFF}',
];

/// Whether `c` stands upright (`Some(true)`), turns with its line
/// (`Some(false)`), or follows its neighbours (`None`: spaces,
/// punctuation, symbols).
fn stands_upright(c: char, upright_latin: bool) -> Option<bool> {
    if UPRIGHT_RANGES.iter().any(|r| r.contains(&c)) {
        Some(true)
    } else if crate::bidi::is_rtl_char(c) {
        // Arabic, Hebrew, Syriac, …: joined / right-to-left, never stacked.
        Some(false)
    } else if c.is_alphanumeric() {
        Some(upright_latin && (c.is_ascii_alphanumeric() || c.is_alphabetic() && (c as u32) < LATIN_END))
    } else {
        None
    }
}

/// End of the Latin blocks (Basic Latin … Latin Extended-B, IPA).
const LATIN_END: u32 = 0x0300;

/// Splits a line into runs that stand upright or turn: neutral characters
/// join the run before them (the first run when they lead); a line of
/// neutrals only stands upright.
fn vertical_runs(line: &str, upright_latin: bool) -> Vec<(bool, String)> {
    let first = line.chars().find_map(|c| stands_upright(c, upright_latin)).unwrap_or(true);
    let mut runs: Vec<(bool, String)> = Vec::new();
    for c in line.chars() {
        let up = stands_upright(c, upright_latin).unwrap_or_else(|| runs.last().map_or(first, |r| r.0));
        match runs.last_mut() {
            Some(r) if r.0 == up => r.1.push(c),
            _ => runs.push((up, c.to_string())),
        }
    }
    runs
}

/// Top-to-bottom text, each line of the text a column, columns from right
/// to left (East Asian vertical writing). CJK and kana stand upright, one
/// character under the other; every other run keeps its shaped line —
/// Arabic letters stay joined — turned a quarter as `v` says.
pub fn vertical_text(text: &str, ts: &TextStyle, v: Vertical, m: &mut dyn Measure) -> Vec<Mark> {
    let advance = ts.size * (VERTICAL_ADVANCE_EM + ts.tracking);
    // Room above and below a turned run, like an upright character's.
    let pad = ts.size * (VERTICAL_ADVANCE_EM - 1.0) / 2.0;
    let gap = ts.size * VERTICAL_COLUMN_GAP_EM;
    // Each column's marks, laid out around x = 0, and its width.
    let mut columns: Vec<(Vec<Mark>, f32)> = Vec::new();
    for col in text.lines() {
        let mut marks = Vec::new();
        let mut width = ts.size;
        let mut y = 0.0;
        for (upright, run) in vertical_runs(col, v.upright_latin) {
            if upright {
                for ch in run.chars().filter(|c| !c.is_whitespace() || *c == '\u{3000}') {
                    let s = ch.to_string();
                    let (w, h) = m.measure(&s, ts);
                    marks.push(Mark::new(MarkKind::Text { text: s, style: ts.clone(), x: -w / 2.0, y: y + (advance - h) / 2.0, w, h }, InkRole::Fg));
                    y += advance;
                }
                continue;
            }
            let run = run.trim();
            if run.is_empty() {
                continue;
            }
            let (len, thick) = m.measure(run, ts);
            width = width.max(thick);
            marks.push(Mark::new(MarkKind::SideText { text: run.to_string(), style: ts.clone(), x: -thick / 2.0, y: y + pad, w: thick, h: len, clockwise: v.clockwise }, InkRole::Fg));
            y += len + 2.0 * pad;
        }
        columns.push((marks, width));
    }
    // Rightmost column first.
    let mut out = Vec::new();
    let mut right: f32 = columns.iter().map(|(_, w)| w + gap).sum::<f32>() - gap;
    for (marks, w) in columns {
        let cx = right - w / 2.0;
        for mut mk in marks {
            mk.shift(cx, 0.0);
            out.push(mk);
        }
        right -= w + gap;
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Fixed advances: digits and letters 0.6 em, lines 1.2 em.
    pub struct Fixed;

    impl Measure for Fixed {
        fn measure(&mut self, text: &str, s: &TextStyle) -> (f32, f32) {
            (text.chars().count() as f32 * s.size * 0.6, s.size * 1.2)
        }
    }

    pub fn strings() -> ClockStrings {
        ClockStrings {
            hours: "12".into(),
            minutes: "35".into(),
            seconds: "07".into(),
            meridiem: Some("AM".into()),
            time: "12:35".into(),
            weekday: "Saturday".into(),
            weekday_short: "Sat".into(),
            day: "22".into(),
            month: "August".into(),
            numeric_date: "22/08".into(),
            day_month: "22 August".into(),
            greeting: "Good night".into(),
            weather: Some(("sun", "23°".into())),
            hour_fraction: 0.25,
            day_fraction: 0.5,
        }
    }

    fn widget(face: ClockFace) -> ClockWidget {
        ClockWidget { face, ..Default::default() }
    }

    fn inside(c: &Composition) -> bool {
        c.marks.iter().all(|m| {
            let b = m.bounds();
            b.0 >= -0.01 && b.1 >= -0.01 && b.2 <= c.size.0 + 0.01 && b.3 <= c.size.1 + 0.01
        })
    }

    #[test]
    fn every_face_lays_out_inside_its_box() {
        for face in ClockFace::ALL {
            let c = clock(&widget(face), &strings(), 2.0, 0.0, false, &mut Fixed);
            assert!(c.size.0 > 0.0 && c.size.1 > 0.0, "{face:?}");
            assert!(inside(&c), "{face:?}");
            assert!(c.bounds(true).is_none(), "{face:?}: no live plane without seconds");
            let rtl = clock(&widget(face), &strings(), 2.0, 1.0, true, &mut Fixed);
            assert!(inside(&rtl), "{face:?} rtl");
        }
    }

    #[test]
    fn superscript_seconds_go_to_a_fixed_live_plane() {
        let c = clock(&ClockWidget { face: ClockFace::Superscript, seconds: true, ..Default::default() }, &strings(), 1.0, 0.0, false, &mut Fixed);
        let live = c.bounds(true).expect("seconds");
        let fixed = c.bounds(false).unwrap();
        assert!(live.0 >= fixed.0 + face::SUPER_DIGITS * 1.2 - 0.01, "beside the two-digit hours");
        assert!(live.1 < face::SUPER_DIGITS * face::SUPER_LINE, "next to the hours, not the minutes");
        let other = clock(&ClockWidget { face: ClockFace::Superscript, seconds: true, ..Default::default() }, &ClockStrings { seconds: "1".into(), ..strings() }, 1.0, 0.0, false, &mut Fixed);
        assert_eq!(other.bounds(true).map(|b| b.2 - b.0), Some(live.2 - live.0), "the seconds box never jitters");
        // Without seconds the AM/PM marker takes the spot, statically.
        let ampm = clock(&widget(ClockFace::Superscript), &strings(), 1.0, 0.0, false, &mut Fixed);
        assert!(ampm.marks.iter().any(|m| matches!(&m.kind, MarkKind::Text { text, .. } if text == "AM")));
    }

    #[test]
    fn skewed_box_leans_by_its_slant() {
        let (pts, lean) = skewed_box(10.0, 20.0, 100.0, 40.0, 0.3);
        assert!((lean - 12.0).abs() < 1e-5);
        assert_eq!(pts[0], (22.0, 20.0));
        assert_eq!(pts[2], (110.0, 60.0));
        // Opposite sides stay parallel and as wide as the box.
        assert!(((pts[1].0 - pts[0].0) - (pts[2].0 - pts[3].0)).abs() < 1e-5);
        let c = clock(&widget(ClockFace::DayBlock), &strings(), 1.0, 0.0, false, &mut Fixed);
        let boxed = c.marks.iter().find_map(|m| match &m.kind {
            MarkKind::Poly { pts } => Some(pts.clone()),
            _ => None,
        });
        let pts = boxed.expect("the time box");
        let on_accent = c.marks.iter().find(|m| m.ink == InkRole::OnAccent).expect("time on the box");
        let (tx0, ty0, tx1, ty1) = on_accent.bounds();
        assert!(tx0 >= pts[3].0 && tx1 <= pts[1].0 && ty0 >= pts[0].1 && ty1 <= pts[2].1, "time inside its box");
        // No weather: no icon and no divider.
        let plain = clock(&widget(ClockFace::DayBlock), &ClockStrings { weather: None, ..strings() }, 1.0, 0.0, false, &mut Fixed);
        assert!(!plain.marks.iter().any(|m| matches!(m.kind, MarkKind::Icon { .. } | MarkKind::Line { .. })));
    }

    #[test]
    fn ring_arc_follows_the_progress() {
        let marks = ring_face(&strings(), 1.0, FontRole::Display, (150.0, 150.0), 140.0, 7.0, RingProgress::Hour, &mut Fixed);
        let arcs: Vec<(f32, f32)> = marks.iter().filter_map(|m| match m.kind {
            MarkKind::Arc { start, sweep, .. } => Some((start, sweep)),
            _ => None,
        }).collect();
        assert_eq!(arcs.len(), 2, "track and progress");
        assert!((arcs[0].1 - TAU).abs() < 1e-5);
        assert!((arcs[1].0 + FRAC_PI_2).abs() < 1e-5, "from 12 o'clock");
        assert!((arcs[1].1 - TAU * 0.25).abs() < 1e-5, "a quarter past");
        let day = ring_face(&strings(), 1.0, FontRole::Display, (150.0, 150.0), 140.0, 7.0, RingProgress::Day, &mut Fixed);
        assert!(day.iter().any(|m| matches!(m.kind, MarkKind::Arc { sweep, .. } if (sweep - TAU * 0.5).abs() < 1e-5)));
        let none = ring_face(&ClockStrings { hour_fraction: 0.0, ..strings() }, 1.0, FontRole::Display, (150.0, 150.0), 140.0, 7.0, RingProgress::Hour, &mut Fixed);
        assert_eq!(none.iter().filter(|m| matches!(m.kind, MarkKind::Arc { .. })).count(), 1, "no zero-length arc");
        // The time is centred in the ring, the date below it.
        let time = marks.iter().find(|m| matches!(&m.kind, MarkKind::Text { text, .. } if text == "12:35")).unwrap().bounds();
        assert!(((time.0 + time.2) / 2.0 - 150.0).abs() < 1e-3 && ((time.1 + time.3) / 2.0 - 150.0).abs() < 1e-3);
        let date = marks.last().unwrap().bounds();
        assert!(date.1 > 150.0 + 140.0);
    }

    #[test]
    fn vertical_text_runs_down_in_right_to_left_columns() {
        let ts = TextStyle { role: FontRole::Display, size: 40.0, weight: 400, tracking: 0.0 };
        let marks = vertical_text("一二三\n四五", &ts, Vertical { clockwise: true, upright_latin: false }, &mut Fixed);
        assert_eq!(marks.len(), 5);
        let pos: Vec<(f32, f32)> = marks.iter().map(|m| {
            let b = m.bounds();
            ((b.0 + b.2) / 2.0, b.1)
        }).collect();
        // First column on the right, characters going down.
        assert!(pos[0].0 > pos[3].0, "first line is the rightmost column");
        assert!((pos[0].0 - pos[1].0).abs() < 1e-4 && pos[1].1 > pos[0].1 && pos[2].1 > pos[1].1);
        assert!((pos[1].1 - pos[0].1 - 40.0 * VERTICAL_ADVANCE_EM).abs() < 1e-3);
    }

    #[test]
    fn vertical_arabic_keeps_its_letters_joined_on_one_turned_line() {
        let ts = TextStyle { role: FontRole::Arabic, size: 40.0, weight: 400, tracking: 0.0 };
        let cw = Vertical { clockwise: true, upright_latin: false };
        // Arabic is never stacked letter by letter: one shaped, turned run.
        let marks = vertical_text("المندرة", &ts, cw, &mut Fixed);
        assert_eq!(marks.len(), 1, "{marks:?}");
        let MarkKind::SideText { text, w, h, clockwise, .. } = &marks[0].kind else { panic!("{marks:?}") };
        assert_eq!(text, "المندرة");
        assert!(*clockwise);
        // Turned: as tall as the line is long, as wide as it is high.
        assert!((h - 7.0 * 0.6 * 40.0).abs() < 1e-3 && (w - 1.2 * 40.0).abs() < 1e-3, "{w} × {h}");
        // Spaces and the middle dot stay inside the run (one line).
        let line = vertical_text("الإسكندرية · المندرة", &ts, cw, &mut Fixed);
        assert_eq!(line.len(), 1);
        // Hebrew too; the turn follows the setting.
        let he = vertical_text("שלום", &ts, Vertical { clockwise: false, ..cw }, &mut Fixed);
        assert!(matches!(&he[0].kind, MarkKind::SideText { clockwise: false, .. }));
        // Mixed: kana stand upright, the Latin word turns, in reading order.
        let mixed = vertical_text("マンダラ Mandara", &ts, cw, &mut Fixed);
        let kinds: Vec<bool> = mixed.iter().map(|m| matches!(m.kind, MarkKind::SideText { .. })).collect();
        assert_eq!(kinds, [false, false, false, false, true]);
        assert!(mixed[4].bounds().1 > mixed[3].bounds().1, "the turned word comes after the kana");
        // Latin capitals may stand upright when asked.
        let up = vertical_text("MANDARA", &ts, Vertical { upright_latin: true, ..cw }, &mut Fixed);
        assert_eq!(up.len(), 7);
        assert!(up.iter().all(|m| matches!(m.kind, MarkKind::Text { .. })));
        // Arabic never stands upright, even then.
        assert_eq!(vertical_text("المندرة", &ts, Vertical { upright_latin: true, ..cw }, &mut Fixed).len(), 1);
    }

    #[test]
    fn greeting_sits_between_two_rules() {
        let c = clock(&widget(ClockFace::Greeting), &strings(), 1.0, 0.5, false, &mut Fixed);
        let rules: Vec<f32> = c.marks.iter().filter_map(|m| match m.kind {
            MarkKind::Line { a, b, .. } if (a.0 - b.0).abs() < 1e-5 => Some(a.0),
            _ => None,
        }).collect();
        assert_eq!(rules.len(), 2);
        let texts = c.marks.iter().filter(|m| matches!(m.kind, MarkKind::Text { .. }));
        for t in texts {
            let b = t.bounds();
            assert!(b.0 > rules[0] && b.2 < rules[1], "text between the rules");
        }
        assert!(c.marks.iter().any(|m| matches!(&m.kind, MarkKind::Text { text, .. } if text == "GOOD NIGHT")));
    }

    #[test]
    fn monitor_values_are_live_and_the_box_is_fixed() {
        let w = MonitorWidget::default();
        let row = |label: &str, value: &str| MonitorLine { label: label.into(), value: value.into(), ..Default::default() };
        let lines = vec![row("CPU", "4%"), MonitorLine { bar: Some(0.5), ..row("Disk", "1 / 2 TB") }, MonitorLine { graph: Some((vec![0.1, 0.5], Some(vec![0.2]))), ..Default::default() }];
        let a = monitor(&w, &lines, 1.0, false, &mut Fixed);
        let b = monitor(&w, &[row("CPU", "100%"), MonitorLine { bar: Some(0.9), ..row("Disk", "1.9 / 2 TB") }, lines[2].clone()], 1.0, false, &mut Fixed);
        assert_eq!(a.size, b.size, "values never resize the widget");
        assert!((a.size.0 - w.width).abs() < 1e-3);
        let label = a.marks.iter().find(|m| matches!(&m.kind, MarkKind::Text { text, .. } if text == "CPU")).unwrap();
        assert!(!label.live);
        let value = a.marks.iter().find(|m| matches!(&m.kind, MarkKind::Text { text, .. } if text == "4%")).unwrap();
        assert!(value.live && (value.bounds().2 - w.width).abs() < 1e-3, "right-aligned value");
        assert_eq!(a.marks.iter().filter(|m| matches!(m.kind, MarkKind::Graph { .. })).count(), 2);
    }

    #[test]
    fn decorations_fill_their_extent() {
        let stripes = decoration(&Decoration { shape: DecorShape::Stripes, width: 200.0, height: 100.0, count: 4, ..Default::default() }, 1.0, true, &mut Fixed);
        assert_eq!(stripes.marks.len(), 4);
        assert!((stripes.size.0 - 200.0).abs() < 1e-3 && stripes.size.1 < 100.0 && stripes.size.1 > 80.0);
        let disc = decoration(&Decoration { shape: DecorShape::Disc, width: 120.0, ..Default::default() }, 2.0, true, &mut Fixed);
        assert_eq!(disc.size, (240.0, 240.0));
        let waves = decoration(&Decoration { shape: DecorShape::Waves, count: 3, ..Default::default() }, 1.0, true, &mut Fixed);
        assert_eq!(waves.marks.len(), 3);
        let text = decoration(&Decoration { text: "المندرة".into(), ..Default::default() }, 1.0, true, &mut Fixed);
        assert_eq!(text.marks.len(), 1);
    }
}
