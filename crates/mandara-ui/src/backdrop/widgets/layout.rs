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
use mandara_core::shape::Shape;

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
    /// The plate behind the widget.
    Plate,
    /// The outline around the plate's shape.
    Outline,
    /// The chrome (tape, a window's title bar details).
    Chrome,
    /// Print paper (a polaroid's border, a note's sheet).
    Paper,
    /// Ink on paper.
    OnPaper,
    /// The theme's text colour, on shapes filled with the plate's colour.
    OnPlate,
    /// A colour of its own (`0xRRGGBB`): palette swatches, tiles.
    Rgb(u32),
    /// A light pastel of the accent, its hue turned by this many degrees
    /// (sticky notes, tabs, tiles); ink on it is [`InkRole::OnPaper`].
    Pastel(u16),
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
    /// The plate behind a widget in `shape` (a rounded rectangle keeps its
    /// exact arcs), with a specular rim `rim` px wide (0 = none).
    Plate { x: f32, y: f32, w: f32, h: f32, radius: f32, rim: f32, shape: Shape },
    /// Any shared shape, filled when `width` is 0, else outlined.
    Shaped { x: f32, y: f32, w: f32, h: f32, shape: Shape, width: f32 },
    /// An open stroked line through `pts`.
    Polyline { pts: Vec<(f32, f32)>, width: f32 },
    /// A picture drawn into `w × h` (covering it), clipped to `shape` and
    /// turned `turn` degrees inside it (a spinning record's label).
    Image { x: f32, y: f32, w: f32, h: f32, pic: Picture, shape: Shape, turn: f32 },
    /// `mask`'s alpha filled with the ink, stretched over `w × h` (globes,
    /// procedural shading).
    Mask { x: f32, y: f32, w: f32, h: f32, mask: Picture },
    /// Text leaning by `slant` (run per rise, positive leans right): the
    /// measured box at `(x, y)`; the lean reaches `h × slant` beyond it.
    SlantText { text: String, style: TextStyle, x: f32, y: f32, w: f32, h: f32, slant: f32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub kind: MarkKind,
    pub ink: InkRole,
    pub opacity: f32,
    /// Drawn in the widget's live plane.
    pub live: bool,
    /// Casts the widget's directional shadow (small print doesn't: a hard
    /// shadow under it reads as doubled text).
    pub casts: bool,
}

impl Mark {
    pub fn new(kind: MarkKind, ink: InkRole) -> Self {
        Self { kind, ink, opacity: 1.0, live: false, casts: true }
    }

    /// Casts no directional shadow.
    pub fn flat(mut self) -> Self {
        self.casts = false;
        self
    }

    pub fn dim(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }

    pub fn live(mut self) -> Self {
        self.live = true;
        self
    }

    /// Axis-aligned bounds `(x0, y0, x1, y1)` including stroke widths.
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        match &self.kind {
            MarkKind::Text { x, y, w, h, .. } | MarkKind::SideText { x, y, w, h, .. } => (*x, *y, x + w, y + h),
            MarkKind::SlantText { x, y, w, h, slant, .. } => (x + (h * slant).min(0.0), *y, x + w + (h * slant).max(0.0), y + h),
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
            MarkKind::Plate { x, y, w, h, .. } | MarkKind::Image { x, y, w, h, .. } | MarkKind::Mask { x, y, w, h, .. } => (*x, *y, x + w, y + h),
            MarkKind::Shaped { x, y, w, h, width, .. } => (x - width / 2.0, y - width / 2.0, x + w + width / 2.0, y + h + width / 2.0),
            MarkKind::Polyline { pts, width } => {
                let b = pts.iter().fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |b, p| (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1)));
                (b.0 - width / 2.0, b.1 - width / 2.0, b.2 + width / 2.0, b.3 + width / 2.0)
            }
        }
    }

    pub fn shift(&mut self, dx: f32, dy: f32) {
        let mv = |p: &mut (f32, f32)| {
            p.0 += dx;
            p.1 += dy;
        };
        match &mut self.kind {
            MarkKind::Text { x, y, .. }
            | MarkKind::SideText { x, y, .. }
            | MarkKind::Rect { x, y, .. }
            | MarkKind::Wave { x, y, .. }
            | MarkKind::Icon { x, y, .. }
            | MarkKind::Graph { x, y, .. }
            | MarkKind::Plate { x, y, .. }
            | MarkKind::Shaped { x, y, .. }
            | MarkKind::SlantText { x, y, .. }
            | MarkKind::Mask { x, y, .. }
            | MarkKind::Image { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
            MarkKind::Poly { pts } | MarkKind::Polyline { pts, .. } => pts.iter_mut().for_each(mv),
            MarkKind::Line { a, b, .. } => {
                mv(a);
                mv(b);
            }
            MarkKind::Arc { c, .. } | MarkKind::Disc { c, .. } => mv(c),
        }
    }
}

/// A laid-out widget: its marks, its box size (physical px) and the
/// places a click does something.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Composition {
    pub marks: Vec<Mark>,
    pub size: (f32, f32),
    pub hits: Vec<Hit>,
}

/// A clickable area of a widget (composition px) and what it does.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub rect: (f32, f32, f32, f32),
    pub action: super::actions::Action,
}

/// A decoded picture for [`MarkKind::Image`]; compared by `key` (the
/// source and the size it was decoded at), never by its pixels.
#[derive(Clone)]
pub struct Picture {
    pub key: u64,
    pub pixmap: std::sync::Arc<tiny_skia::Pixmap>,
}

impl PartialEq for Picture {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({:x})", self.key)
    }
}

impl Composition {
    /// Union of the bounds of the marks with `live == live`, if any.
    pub fn bounds(&self, live: bool) -> Option<(f32, f32, f32, f32)> {
        self.marks.iter().filter(|m| m.live == live).map(Mark::bounds).reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
    }

    /// A composition of `marks`, moved to start at the origin.
    pub fn of(marks: Vec<Mark>) -> Self {
        let mut c = Self { marks, ..Self::default() };
        c.normalize();
        c
    }

    /// [`Self::of`] with clickable areas (in the marks' coordinates).
    pub fn with_hits(marks: Vec<Mark>, hits: Vec<Hit>) -> Self {
        let mut c = Self { marks, hits, ..Self::default() };
        c.normalize();
        c
    }

    /// Moves everything so the marks start at the origin and recomputes
    /// the size.
    pub fn normalize(&mut self) {
        let all = self.marks.iter().map(Mark::bounds).reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)));
        let Some((x0, y0, x1, y1)) = all else {
            self.size = (0.0, 0.0);
            return;
        };
        for m in &mut self.marks {
            m.shift(-x0, -y0);
        }
        for h in &mut self.hits {
            h.rect.0 -= x0;
            h.rect.1 -= y0;
        }
        self.size = ((x1 - x0).max(0.0), (y1 - y0).max(0.0));
    }

    /// Moves every mark and hit by `(dx, dy)`.
    pub fn shift(&mut self, dx: f32, dy: f32) {
        for m in &mut self.marks {
            m.shift(dx, dy);
        }
        for h in &mut self.hits {
            h.rect.0 += dx;
            h.rect.1 += dy;
        }
    }

    /// The action under `(x, y)` (composition px), the last one drawn
    /// winning.
    pub fn hit(&self, (x, y): (f32, f32)) -> Option<&super::actions::Action> {
        self.hits.iter().rev().find(|h| x >= h.rect.0 && y >= h.rect.1 && x < h.rect.0 + h.rect.2 && y < h.rect.1 + h.rect.3).map(|h| &h.action)
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
    /// The clock's text reads right to left.
    pub rtl: bool,
    /// The time as numbers (the analog hands).
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// `12`, `3`, `6`, `9` in the clock's digits (the analog dial).
    pub hour_numbers: [String; 4],
    /// `12`, `1` … `11` in the clock's digits.
    pub all_hours: [String; 12],
    /// `AM` and `PM` in the clock's language.
    pub meridiem_pair: [String; 2],
    pub year: String,
    pub month_short: String,
    /// `08`.
    pub month_number: String,
    /// `Evening`.
    pub day_part: String,
    /// The day in the Hijri calendar (`5 Jumada al-Awwal 1448`), when known.
    pub hijri: String,
    /// The moon's age, 0 (new) … ½ (full) … 1.
    pub moon_phase: f32,
    /// This week's days (short names) from the week's first day, and
    /// whether each is today.
    pub week: Vec<(String, bool)>,
}

impl ClockStrings {
    /// The value of a template token (`HH`, `weekday`, …).
    pub fn token(&self, token: &str) -> Option<String> {
        let h12 = self.all_hours[(self.hour % 12) as usize].clone();
        Some(match token {
            "HH" => self.hours.clone(),
            "H" => match self.hours.trim_start_matches(['0', '٠']) {
                "" => self.hours.chars().last().map(String::from).unwrap_or_default(),
                t => t.to_string(),
            },
            "hh" => format!("{h12:0>2}"),
            "h" => h12,
            "mm" => self.minutes.clone(),
            "ss" => self.seconds.clone(),
            "ampm" => self.meridiem_pair[usize::from(self.hour >= 12)].clone(),
            "time" => self.time.clone(),
            "weekday" => self.weekday.clone(),
            "wd" => self.weekday_short.clone(),
            "day" => self.day.clone(),
            "dd" => format!("{:0>2}", self.day),
            "month" => self.month.clone(),
            "mon" => self.month_short.clone(),
            "MM" => self.month_number.clone(),
            "year" => self.year.clone(),
            "greeting" => self.greeting.clone(),
            "daypart" => self.day_part.clone(),
            "hijri" => self.hijri.clone(),
            _ => return None,
        })
    }
}

/// What joins two parts of a line (`Sunday · 1 November`).
pub use self::face::SEPARATOR;

/// Reference sizes of the faces (px at a 1080 px tall display).
pub(super) mod face {
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
pub(super) struct Run {
    pub text: String,
    pub style: TextStyle,
    pub ink: InkRole,
    pub opacity: f32,
    pub live: bool,
    /// Gap after the run, physical px.
    pub gap: f32,
}

pub(super) fn run(text: impl Into<String>, style: TextStyle, ink: InkRole) -> Run {
    Run { text: text.into(), style, ink, opacity: 1.0, live: false, gap: 0.0 }
}

/// Lays out runs left to right (right to left when `rtl`) with their
/// baselines roughly aligned (bottoms of the line boxes), top at `y`.
/// Returns the marks and the line's `(width, height)`.
pub(super) fn line(runs: Vec<Run>, x: f32, y: f32, rtl: bool, m: &mut dyn Measure) -> (Vec<Mark>, (f32, f32)) {
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
pub(super) fn stack(lines: Vec<(Vec<Mark>, (f32, f32))>, gap: f32, align: f32) -> Vec<Mark> {
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

pub(super) fn style(role: FontRole, size: f32, weight: u16) -> TextStyle {
    TextStyle { role, size, weight, tracking: 0.0 }
}

pub(super) fn tracked(mut s: TextStyle, tracking: f32) -> TextStyle {
    s.tracking = tracking;
    s
}

/// `text` broken into lines no wider than `width` (at spaces; a word
/// longer than a line is cut), at most `max` lines, the last one ending in
/// `…` when text was left over.
pub fn wrap(text: &str, st: &TextStyle, width: f32, max: usize, m: &mut dyn Measure) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for para in text.lines() {
        let mut cur = String::new();
        for word in para.split_whitespace() {
            let next = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if m.measure(&next, st).0 <= width || cur.is_empty() && m.measure(word, st).0 <= width {
                cur = next;
                continue;
            }
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            cur = fit(word, st, width, m);
        }
        lines.push(cur);
        if lines.len() > max {
            break;
        }
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    if lines.len() > max {
        lines.truncate(max);
        if let Some(last) = lines.last_mut() {
            *last = fit(&format!("{last} …"), st, width, m);
            if !last.ends_with('…') {
                last.push('…');
            }
        }
    }
    lines
}

/// Whether `text` reads right to left: its first letter is Arabic,
/// Hebrew, Syriac or Thaana (user content in any interface language).
pub fn reads_rtl(text: &str) -> Option<bool> {
    text.chars().find(|c| c.is_alphabetic()).map(|c| matches!(u32::from(c), 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF))
}

/// `text` cut (with `…`) to fit `width`.
pub fn fit(text: &str, st: &TextStyle, width: f32, m: &mut dyn Measure) -> String {
    if m.measure(text, st).0 <= width {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let cand: String = chars[..mid].iter().collect::<String>() + "…";
        if m.measure(&cand, st).0 <= width { lo = mid } else { hi = mid - 1 }
    }
    chars[..lo].iter().collect::<String>().trim_end().to_string() + "…"
}

/// Upper-cases Latin text (scripts without case are unchanged).
pub(super) fn caps(s: &str) -> String {
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
            if !s.numeric_date.is_empty() {
                runs.push(Run { opacity: DIM, ..run(DASH, light.clone(), InkRole::Fg) });
                runs.push(run(&s.numeric_date, light, InkRole::Fg));
            }
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
            if !s.day_month.is_empty() {
                let ds = tracked(style(role, px(DAY_DATE), MEDIUM), GREET_TRACKING / 2.0);
                // Day and month as designed; both calendars when asked.
                let date = caps(&if s.day_month.contains(SEPARATOR) { s.day_month.clone() } else { format!("{} {}", s.day, s.month) });
                let (dw, dh) = m.measure(&date, &ds);
                let dy = top + bh + px(DAY_ROW_GAP);
                row2.push(Mark::new(MarkKind::Text { text: date, style: ds, x: (row1_w.max(bw) - dw) * align, y: dy, w: dw, h: dh }, InkRole::Fg).dim(DIM));
            }
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
            let lines: Vec<_> = [
                line(vec![run(caps(&s.greeting), small.clone(), InkRole::Accent)], 0.0, 0.0, rtl, m),
                line(vec![Run { opacity: DIM, ..run(c.subtitle.trim(), style(role, px(GREET_SMALL), MEDIUM), InkRole::Fg) }], 0.0, 0.0, rtl, m),
                line(vec![run(caps(&s.weekday_short), style(role, px(GREET_DAY), HEAVY), InkRole::Fg)], 0.0, 0.0, rtl, m),
                line(vec![run(caps(&s.day_month), small, InkRole::Fg)], 0.0, 0.0, rtl, m),
                line(vec![run(&s.time, style(role, px(GREET_TIME), LIGHT), InkRole::Fg)], 0.0, 0.0, rtl, m),
            ]
            .into_iter()
            .filter(|(marks, _)| !marks.is_empty())
            .collect();
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
            let mut runs = vec![run(&s.weekday, TextStyle { weight: SEMIBOLD, ..ds.clone() }, InkRole::Accent)];
            if !s.day_month.is_empty() {
                runs.push(Run { opacity: DIM, ..run(format!("{SEPARATOR}{}", s.day_month), ds, InkRole::Fg) });
            }
            let date = line(runs, 0.0, 0.0, rtl, m);
            marks.extend(stack(vec![time, date], px(MIN_GAP), align));
        }
        ClockFace::Analog => marks.extend(super::faces::analog_face(c, s, k, role, align, m)),
        ClockFace::Stack => marks.extend(super::faces::stack_face(c, s, k, align, rtl, m)),
        ClockFace::Shape => marks.extend(super::faces::shape_face(c, s, k, m)),
        ClockFace::DotMatrix => marks.extend(super::faces::matrix_face(c, s, k, align, true, m)),
        ClockFace::Pixel => marks.extend(super::faces::matrix_face(c, s, k, align, false, m)),
        ClockFace::Boxes => marks.extend(super::faces::boxes_face(c, s, k, m)),
        ClockFace::Hud => marks.extend(super::faces::hud_face(c, s, k, rtl, m)),
        ClockFace::Template => marks.extend(super::faces::template_face(c, s, k, align, rtl, m)),
        ClockFace::Numerals => marks.extend(super::faces::numerals_face(c, s, k, m)),
        ClockFace::Orbit => marks.extend(super::faces::orbit_face(c, s, k, m)),
    }
    let mut comp = Composition { marks, ..Composition::default() };
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
    let date = if s.day_month.is_empty() { s.weekday.clone() } else { format!("{}{SEPARATOR}{}", s.weekday, s.day_month) };
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
    /// The reading as a share (0..=1) when it is one (a gauge shows it).
    pub share: Option<f32>,
    /// A sparkline (and an optional second series) instead of a value.
    pub graph: Option<(Vec<f32>, Option<Vec<f32>>)>,
    /// A temperature or the fan as a part of its usual range (0..=1), for
    /// the thermal arcs.
    pub level: Option<f32>,
    /// A shorter label for tight spots (`RAM` for Memory).
    pub short: Option<String>,
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
    let mut comp = Composition { marks, ..Composition::default() };
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
/// Washi tape: its opacity, the depth of the torn teeth (of its height)
/// and the stripes' lean.
const TAPE_ALPHA: f32 = 0.62;
const TAPE_TOOTH: f32 = 0.12;
const TAPE_LEAN: f32 = 0.35;

/// A decoration (unrotated: the painter rotates the plane's content).
/// `clockwise`: which way vertical text turns its sideways runs.
pub fn decoration(d: &Decoration, k: f32, clockwise: bool, align: f32, m: &mut dyn Measure) -> Composition {
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
                marks.extend(stack(lines, 0.0, align));
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
        DecorShape::Tape => {
            // A translucent strip with torn ends and diagonal stripes.
            let tooth = (h * TAPE_TOOTH).max(stroke);
            let mut pts = vec![(0.0, 0.0), (w, 0.0)];
            let teeth = (h / tooth).round().max(2.0) as usize;
            for i in 1..=teeth {
                pts.push((if i % 2 == 1 { w - tooth } else { w }, h * i as f32 / teeth as f32));
            }
            pts.push((0.0, h));
            for i in (1..teeth).rev() {
                pts.push((if i % 2 == 1 { tooth } else { 0.0 }, h * i as f32 / teeth as f32));
            }
            marks.push(Mark::new(MarkKind::Poly { pts }, InkRole::Accent).dim(TAPE_ALPHA));
            let n = d.count.max(1);
            for i in 1..=n {
                let x = w * i as f32 / (n + 1) as f32;
                marks.push(Mark::new(MarkKind::Line { a: (x - h * TAPE_LEAN, h), b: (x + h * TAPE_LEAN, 0.0), width: stroke }, InkRole::Paper).dim(TAPE_ALPHA / 2.0));
            }
        }
    }
    let mut comp = Composition { marks, ..Composition::default() };
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

// ── Plate ────────────────────────────────────────────────────────────────

/// Puts `look`'s plate, outline, header and chrome around `comp` (see
/// [`super::frame`]).
pub fn with_plate(comp: Composition, look: &mandara_config::desktop_widgets::Look, k: f32, align: f32, m: &mut dyn Measure) -> Composition {
    super::frame::frame(comp, look, k, align, m)
}

// ── Gauges ───────────────────────────────────────────────────────────────

mod gauge {
    pub const R: f32 = 38.0;
    pub const W: f32 = 6.5;
    pub const GAP: f32 = 22.0;
    pub const VALUE: f32 = 19.0;
    pub const LABEL: f32 = 13.0;
    pub const LABEL_GAP: f32 = 8.0;
    pub const ROW_GAP: f32 = 18.0;
    pub const TRACK_ALPHA: f32 = 0.18;
    pub const LABEL_ALPHA: f32 = 0.7;
    pub const TRACKING: f32 = 0.12;
    /// Where the arc starts (a gap at the bottom): fraction of a turn.
    pub const SWEEP_TURN: f32 = 0.75;
    /// A quarter turn, degrees (angles from the top to angles from 3
    /// o'clock).
    pub const QUARTER_DEG: f32 = 90.0;
}

/// The monitor as ring gauges (the readings that are a share), the other
/// rows as a column under them.
pub fn monitor_gauges(w: &MonitorWidget, lines: &[MonitorLine], k: f32, rtl: bool, m: &mut dyn Measure) -> Composition {
    use gauge::*;
    let px = |v: f32| v * k;
    let role = w.look.font;
    let (shares, rest): (Vec<&MonitorLine>, Vec<&MonitorLine>) = lines.iter().partition(|l| l.share.is_some());
    let width = px(w.width.clamp(mandara_config::desktop_widgets::ranges::MONITOR_WIDTH_PX.0, mandara_config::desktop_widgets::ranges::MONITOR_WIDTH_PX.1));
    let cell = 2.0 * px(R) + px(GAP);
    let per_row = ((width + px(GAP)) / cell).floor().max(1.0) as usize;
    let mut marks = Vec::new();
    let vs = style(role, px(VALUE), face::SEMIBOLD);
    let ls = tracked(style(role, px(LABEL), face::MEDIUM), TRACKING);
    let mut y = 0.0;
    for row in shares.chunks(per_row) {
        let mut row_h: f32 = 0.0;
        for (i, l) in row.iter().enumerate() {
            let slot = if rtl { row.len() - 1 - i } else { i };
            let c = (px(R) + slot as f32 * cell, y + px(R));
            // Degrees from the top, clockwise; the default leaves the gap
            // at the bottom where it always was.
            let start = (w.start_angle - QUARTER_DEG).to_radians();
            let sweep = SWEEP_TURN * TAU;
            let rw = px(w.ring_width());
            let ring_style = super::monitor_more::RingStyle { wavy: w.wavy, dotted: w.dotted };
            marks.extend(super::monitor_more::ring(c, px(R), rw, start, sweep, ring_style, InkRole::Fg).into_iter().map(|m| m.dim(TRACK_ALPHA)));
            let f = l.share.unwrap_or(0.0).clamp(0.0, 1.0);
            if f > 0.0 {
                marks.extend(super::monitor_more::ring(c, px(R), rw, start, sweep * f, ring_style, InkRole::Accent).into_iter().map(Mark::live));
            }
            // The live plane always covers the whole ring.
            marks.push(Mark::new(MarkKind::Arc { c, r: px(R), width: rw + px(W), start: 0.0, sweep: TAU }, InkRole::Fg).dim(0.0).live());
            let (vw, vh) = m.measure(&l.value, &vs);
            marks.push(Mark::new(MarkKind::Text { text: l.value.clone(), style: vs.clone(), x: c.0 - vw / 2.0, y: c.1 - vh / 2.0, w: vw, h: vh }, InkRole::Fg).live());
            let small = tracked(style(role, px(super::monitor_more::LABEL_SMALL), face::MEDIUM), TRACKING);
            let (label, st) = super::monitor_more::slot_label(&l.label, l.short.as_deref(), cell - px(super::monitor_more::LABEL_SPACE), &ls, &small, m);
            let (lw, lh) = m.measure(&label, &st);
            let ly = y + 2.0 * px(R) + px(LABEL_GAP);
            marks.push(Mark::new(MarkKind::Text { text: label, style: st, x: c.0 - lw / 2.0, y: ly, w: lw, h: lh }, InkRole::Fg).dim(LABEL_ALPHA));
            row_h = row_h.max(ly + lh - y);
        }
        y += row_h + px(ROW_GAP);
    }
    if !rest.is_empty() {
        let column = MonitorWidget { title: String::new(), ..w.clone() };
        let rest: Vec<MonitorLine> = rest.into_iter().cloned().collect();
        let col = monitor(&column, &rest, k, rtl, m);
        for mut mk in col.marks {
            mk.shift(0.0, y);
            marks.push(mk);
        }
    }
    let mut comp = Composition { marks, ..Composition::default() };
    comp.normalize();
    comp
}

// ── Weather ──────────────────────────────────────────────────────────────

/// Everything the weather widget writes, formatted for its language.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WeatherStrings {
    pub icon: &'static str,
    pub temp: String,
    pub condition: String,
    /// `Alexandria`, empty when hidden.
    pub place: String,
    /// `Feels like 25° · Humidity 60% · 12 km/h`, empty when hidden.
    pub details: String,
    /// Forecast days: weekday, icon, `27° / 18°`.
    pub days: Vec<(String, &'static str, String)>,
    pub rtl: bool,
}

mod weather {
    pub const ICON: f32 = 76.0;
    pub const TEMP: f32 = 84.0;
    pub const CONDITION: f32 = 26.0;
    pub const SMALL: f32 = 17.0;
    pub const GAP: f32 = 18.0;
    pub const LINE_GAP: f32 = 4.0;
    pub const DAYS_GAP: f32 = 22.0;
    pub const DAY_ICON: f32 = 30.0;
    pub const DAY_GAP: f32 = 26.0;
    pub const DAY_ROW_GAP: f32 = 6.0;
    pub const RULE_W: f32 = 1.0;
    pub const RULE_ALPHA: f32 = 0.25;
    pub const COMPACT_ICON: f32 = 40.0;
    pub const COMPACT_TEMP: f32 = 40.0;
    pub const COMPACT_TEXT: f32 = 22.0;
}

/// The weather widget.
pub fn weather_widget(w: &mandara_config::desktop_widgets::WeatherWidget, s: &WeatherStrings, k: f32, align: f32, m: &mut dyn Measure) -> Composition {
    use mandara_config::desktop_widgets::WeatherLayout;
    use weather::*;
    let px = |v: f32| v * k;
    let role = w.look.font;
    let mut marks = Vec::new();
    match w.layout {
        WeatherLayout::Compact => {
            let icon = px(COMPACT_ICON);
            let ts = style(role, px(COMPACT_TEMP), face::SEMIBOLD);
            let (tw, th) = m.measure(&s.temp, &ts);
            let cs = style(role, px(COMPACT_TEXT), face::MEDIUM);
            let (cw, ch) = m.measure(&s.condition, &cs);
            let h = icon.max(th).max(ch);
            let gap = px(GAP) / 2.0;
            let items = [(icon, 0), (tw, 1), (cw, 2)];
            let total: f32 = items.iter().map(|(w, _)| w).sum::<f32>() + 2.0 * gap;
            let mut x = if s.rtl { total } else { 0.0 };
            for (iw, kind) in items {
                let left = if s.rtl { x - iw } else { x };
                match kind {
                    0 => marks.push(Mark::new(MarkKind::Icon { name: s.icon, x: left, y: (h - icon) / 2.0, size: icon }, InkRole::Accent)),
                    1 => marks.push(Mark::new(MarkKind::Text { text: s.temp.clone(), style: ts.clone(), x: left, y: (h - th) / 2.0, w: tw, h: th }, InkRole::Fg)),
                    _ => marks.push(Mark::new(MarkKind::Text { text: s.condition.clone(), style: cs.clone(), x: left, y: (h - ch) / 2.0, w: cw, h: ch }, InkRole::Fg).dim(face::DIM)),
                }
                x = if s.rtl { left - gap } else { x + iw + gap };
            }
        }
        // Drawn from the whole forecast by `super::weather_more`.
        WeatherLayout::Sentence | WeatherLayout::Disc | WeatherLayout::Hourly => {}
        WeatherLayout::Hero => {
            let icon = px(ICON);
            let ts = style(role, px(TEMP), face::LIGHT);
            let head = line(vec![run(&s.temp, ts, InkRole::Fg)], 0.0, 0.0, s.rtl, m);
            let mut text_lines = vec![head, line(vec![run(&s.condition, style(role, px(CONDITION), face::MEDIUM), InkRole::Fg)], 0.0, 0.0, s.rtl, m)];
            let small = style(role, px(SMALL), face::REGULAR);
            for t in [&s.place, &s.details] {
                if !t.is_empty() {
                    text_lines.push(line(vec![Run { opacity: face::DIM, ..run(t.as_str(), small.clone(), InkRole::Fg) }], 0.0, 0.0, s.rtl, m));
                }
            }
            let block_w = text_lines.iter().map(|(_, s)| s.0).fold(0.0, f32::max);
            let block_h: f32 = text_lines.iter().map(|(_, s)| s.1).sum::<f32>() + px(LINE_GAP) * (text_lines.len() as f32 - 1.0);
            let text_align = if s.rtl { 1.0 } else { 0.0 };
            let mut block = stack(text_lines, px(LINE_GAP), text_align);
            let (icon_x, text_x) = if s.rtl { (block_w + px(GAP), 0.0) } else { (0.0, icon + px(GAP)) };
            for mk in &mut block {
                mk.shift(text_x, 0.0);
            }
            marks.push(Mark::new(MarkKind::Icon { name: s.icon, x: icon_x, y: (block_h - icon).max(0.0) / 2.0, size: icon }, InkRole::Accent));
            marks.extend(block);
            let top_w = icon + px(GAP) + block_w;
            if !s.days.is_empty() {
                let y0 = block_h.max(icon) + px(DAYS_GAP);
                marks.push(Mark::new(MarkKind::Line { a: (0.0, y0 - px(DAYS_GAP) / 2.0), b: (top_w, y0 - px(DAYS_GAP) / 2.0), width: px(RULE_W) }, InkRole::Fg).dim(RULE_ALPHA));
                let mut cols = Vec::new();
                for (day, ic, range) in &s.days {
                    let ds = tracked(style(role, px(SMALL), face::SEMIBOLD), gauge::TRACKING);
                    let d = caps(day);
                    let (dw, dh) = m.measure(&d, &ds);
                    let (rw, rh) = m.measure(range, &small);
                    let cw = dw.max(rw).max(px(DAY_ICON));
                    let mut col = vec![Mark::new(MarkKind::Text { text: d, style: ds, x: (cw - dw) / 2.0, y: 0.0, w: dw, h: dh }, InkRole::Fg).dim(face::DIM)];
                    let iy = dh + px(DAY_ROW_GAP);
                    col.push(Mark::new(MarkKind::Icon { name: ic, x: (cw - px(DAY_ICON)) / 2.0, y: iy, size: px(DAY_ICON) }, InkRole::Accent));
                    col.push(Mark::new(MarkKind::Text { text: range.clone(), style: small.clone(), x: (cw - rw) / 2.0, y: iy + px(DAY_ICON) + px(DAY_ROW_GAP), w: rw, h: rh }, InkRole::Fg));
                    cols.push((col, cw));
                }
                let total: f32 = cols.iter().map(|(_, w)| w).sum::<f32>() + px(DAY_GAP) * (cols.len() as f32 - 1.0);
                let mut x = (top_w - total).max(0.0) * align;
                if s.rtl {
                    cols.reverse();
                }
                for (col, cw) in cols {
                    for mut mk in col {
                        mk.shift(x, y0);
                        marks.push(mk);
                    }
                    x += cw + px(DAY_GAP);
                }
            }
        }
    }
    let mut comp = Composition { marks, ..Composition::default() };
    comp.normalize();
    comp
}

// ── World clock ──────────────────────────────────────────────────────────

/// One city of the world clock.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CityLine {
    pub name: String,
    pub time: String,
    /// `+2 h · Tomorrow`, empty when hidden or the same.
    pub note: String,
    /// Night there.
    pub night: bool,
}

mod world {
    pub const NAME: f32 = 20.0;
    pub const TIME: f32 = 30.0;
    pub const NOTE: f32 = 14.0;
    pub const ROW_GAP: f32 = 14.0;
    pub const COL_GAP: f32 = 34.0;
    pub const NAME_GAP: f32 = 28.0;
    pub const NOTE_GAP: f32 = 2.0;
    pub const DOT: f32 = 4.0;
    pub const DOT_GAP: f32 = 10.0;
    pub const NIGHT_ALPHA: f32 = 0.45;
    pub const TRACKING: f32 = 0.08;
}

/// The world clock: a column (or a row) of cities.
pub fn world_clock(w: &mandara_config::desktop_widgets::WorldClockWidget, cities: &[CityLine], k: f32, rtl: bool, m: &mut dyn Measure) -> Composition {
    use world::*;
    let px = |v: f32| v * k;
    let role = w.look.font;
    let ns = style(role, px(NAME), face::MEDIUM);
    let ts = style(role, px(TIME), face::SEMIBOLD);
    let es = tracked(style(role, px(NOTE), face::MEDIUM), TRACKING);
    let mut marks = Vec::new();
    if w.horizontal {
        let mut x = 0.0;
        let cols: Vec<&CityLine> = if rtl { cities.iter().rev().collect() } else { cities.iter().collect() };
        for c in cols {
            let name = caps(&c.name);
            let (nw, nh) = m.measure(&name, &es);
            let (tw, th) = m.measure(&c.time, &ts);
            let (ew, eh) = m.measure(&c.note, &es);
            let cw = nw.max(tw).max(ew);
            marks.push(Mark::new(MarkKind::Text { text: name, style: es.clone(), x: x + (cw - nw) / 2.0, y: 0.0, w: nw, h: nh }, InkRole::Accent));
            marks.push(Mark::new(MarkKind::Text { text: c.time.clone(), style: ts.clone(), x: x + (cw - tw) / 2.0, y: nh + px(NOTE_GAP), w: tw, h: th }, InkRole::Fg).dim(if c.night { face::DIM } else { 1.0 }));
            if !c.note.is_empty() {
                marks.push(Mark::new(MarkKind::Text { text: c.note.clone(), style: es.clone(), x: x + (cw - ew) / 2.0, y: nh + th + 2.0 * px(NOTE_GAP), w: ew, h: eh }, InkRole::Fg).dim(face::DIM));
            }
            x += cw + px(COL_GAP);
        }
    } else {
        let rows: Vec<(f32, f32, f32)> = cities.iter().map(|c| (m.measure(&c.name, &ns).0, m.measure(&c.time, &ts).0, m.measure(&c.note, &es).0)).collect();
        let name_w = rows.iter().map(|r| r.0.max(r.2)).fold(0.0, f32::max) + px(DOT) * 2.0 + px(DOT_GAP);
        let time_w = rows.iter().map(|r| r.1).fold(0.0, f32::max);
        let width = name_w + px(NAME_GAP) + time_w;
        let mut y = 0.0;
        for (c, (nw, tw, ew)) in cities.iter().zip(rows) {
            let (_, th) = m.measure(&c.time, &ts);
            let (_, nh) = m.measure(&c.name, &ns);
            let (_, eh) = m.measure(&c.note, &es);
            let dot_x = if rtl { width - px(DOT) } else { px(DOT) };
            let text_x = |w: f32| if rtl { width - 2.0 * px(DOT) - px(DOT_GAP) - w } else { 2.0 * px(DOT) + px(DOT_GAP) };
            let time_x = if rtl { 0.0 } else { width - tw };
            let name_y = if c.note.is_empty() { y + (th - nh) / 2.0 } else { y };
            marks.push(Mark::new(MarkKind::Disc { c: (dot_x, name_y + nh / 2.0), r: px(DOT) }, InkRole::Accent).dim(if c.night { NIGHT_ALPHA } else { 1.0 }));
            marks.push(Mark::new(MarkKind::Text { text: c.name.clone(), style: ns.clone(), x: text_x(nw), y: name_y, w: nw, h: nh }, InkRole::Fg));
            if !c.note.is_empty() {
                marks.push(Mark::new(MarkKind::Text { text: c.note.clone(), style: es.clone(), x: text_x(ew), y: name_y + nh + px(NOTE_GAP), w: ew, h: eh }, InkRole::Fg).dim(face::DIM));
            }
            marks.push(Mark::new(MarkKind::Text { text: c.time.clone(), style: ts.clone(), x: time_x, y, w: tw, h: th }, InkRole::Fg).dim(if c.night { face::DIM } else { 1.0 }));
            y += th.max(nh + if c.note.is_empty() { 0.0 } else { eh + px(NOTE_GAP) }) + px(ROW_GAP);
        }
    }
    let mut comp = Composition { marks, ..Composition::default() };
    comp.normalize();
    comp
}

// ── Countdown ────────────────────────────────────────────────────────────

/// The countdown's texts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CountdownStrings {
    /// `DAYS UNTIL`, `DAYS SINCE` or `TODAY`.
    pub label: String,
    /// The count (empty on the day itself).
    pub count: String,
    pub title: String,
    /// The target date, empty when hidden.
    pub date: String,
    pub rtl: bool,
}

mod countdown {
    pub const LABEL: f32 = 18.0;
    pub const COUNT: f32 = 150.0;
    pub const TITLE: f32 = 30.0;
    pub const DATE: f32 = 18.0;
    pub const GAP: f32 = 6.0;
    pub const TRACKING: f32 = 0.3;
    pub const COUNT_LINE: f32 = 0.86;
}

/// The countdown: a label, the count large, the title and the date.
pub fn countdown(c: &mandara_config::desktop_widgets::CountdownWidget, s: &CountdownStrings, k: f32, align: f32, m: &mut dyn Measure) -> Composition {
    use countdown::*;
    let px = |v: f32| v * k;
    let role = c.look.font;
    let mut lines = vec![line(vec![run(caps(&s.label), tracked(style(role, px(LABEL), face::BOLD), TRACKING), InkRole::Accent)], 0.0, 0.0, s.rtl, m)];
    if !s.count.is_empty() {
        let (mut marks, (w, h)) = line(vec![run(&s.count, style(role, px(COUNT), face::BLACK), InkRole::Fg)], 0.0, 0.0, s.rtl, m);
        // Big digits carry a tall line box: tighten it under them.
        let h = h * COUNT_LINE;
        for mk in &mut marks {
            mk.shift(0.0, -(h / COUNT_LINE - h) / 2.0);
        }
        lines.push((marks, (w, h)));
    }
    if !s.title.is_empty() {
        lines.push(line(vec![run(&s.title, style(role, px(TITLE), face::SEMIBOLD), InkRole::Fg)], 0.0, 0.0, s.rtl, m));
    }
    if !s.date.is_empty() {
        lines.push(line(vec![Run { opacity: face::DIM, ..run(&s.date, style(role, px(DATE), face::MEDIUM), InkRole::Fg) }], 0.0, 0.0, s.rtl, m));
    }
    let mut comp = Composition { marks: stack(lines, px(GAP), align), ..Composition::default() };
    comp.normalize();
    comp
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
            rtl: false,
            hour: 0,
            minute: 35,
            second: 7,
            hour_numbers: ["12".into(), "3".into(), "6".into(), "9".into()],
            all_hours: ["12", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11"].map(String::from),
            meridiem_pair: ["AM".into(), "PM".into()],
            year: "2026".into(),
            month_short: "Aug".into(),
            month_number: "08".into(),
            day_part: "Night".into(),
            hijri: String::new(),
            moon_phase: 0.3,
            week: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].iter().map(|d| (d.to_string(), *d == "Sat")).collect(),
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
        let stripes = decoration(&Decoration { shape: DecorShape::Stripes, width: 200.0, height: 100.0, count: 4, ..Default::default() }, 1.0, true, 0.5, &mut Fixed);
        assert_eq!(stripes.marks.len(), 4);
        assert!((stripes.size.0 - 200.0).abs() < 1e-3 && stripes.size.1 < 100.0 && stripes.size.1 > 80.0);
        let disc = decoration(&Decoration { shape: DecorShape::Disc, width: 120.0, ..Default::default() }, 2.0, true, 0.5, &mut Fixed);
        assert_eq!(disc.size, (240.0, 240.0));
        let waves = decoration(&Decoration { shape: DecorShape::Waves, count: 3, ..Default::default() }, 1.0, true, 0.5, &mut Fixed);
        assert_eq!(waves.marks.len(), 3);
        let text = decoration(&Decoration { text: "المندرة".into(), ..Default::default() }, 1.0, true, 0.5, &mut Fixed);
        assert_eq!(text.marks.len(), 1);
    }
}
