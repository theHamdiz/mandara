//! Masked password characters, shared by the lock screen (software
//! painted) and Mandara Login (GPUI).
//!
//! Three things live here so both screens behave the same:
//!
//! - **Layout** ([`Row`]): every symbol sits in a box of one fixed size at
//!   a fixed advance from the field's start edge (the right edge in
//!   right-to-left languages). Both are snapped to whole device pixels, so a
//!   symbol's position depends only on its index: adding or deleting a
//!   character never moves the others. When the field is full the row
//!   scrolls one slot per character, keeping the newest symbol visible.
//! - **Shapes** ([`Symbol`], [`symbol_at`]): a set of small soft shapes
//!   after Material 3's expressive shape library. The shape of a character
//!   comes from its position and a per-entry seed, never from the
//!   character, and two neighbours never share a shape.
//! - **Motion** ([`appear`], [`vanish`], [`Fades`]): a short spring pop
//!   when a symbol arrives and a quick shrink-and-fade when it is deleted;
//!   with reduced motion both are instant.
//!
//! Outlines are built once per process ([`Symbol::outline`]) and handed out
//! as shared data: an SVG for GPUI's sprite atlas ([`Symbol::svg`], one
//! rasterization per shape and size) and a `tiny_skia` path in a unit box
//! ([`Symbol::skia_path`]) that the lock screen fills through a transform.
//! Nothing here allocates per frame or per keystroke.

use std::f32::consts::{PI, TAU};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::spring::SpringConfig;

/// A symbol's box, as a share of the field's text size.
pub const SIZE_EM: f32 = 0.74;
/// A dot's diameter (the plain "Dots" style), as a share of the text size.
pub const DOT_EM: f32 = 0.52;
/// Distance from one symbol's box to the next, as a share of the text size.
pub const ADVANCE_EM: f32 = 1.1;
/// The pop of a new symbol plays for this long.
pub const POP: Duration = Duration::from_millis(300);
/// A deleted symbol shrinks and fades for this long.
pub const FADE: Duration = Duration::from_millis(140);
/// The pop: snappy, with a small overshoot (about 10 %).
const POP_SPRING: SpringConfig = SpringConfig { stiffness: 700.0, damping: 30.0, mass: 1.0 };
/// A new symbol fades in over the first part of its pop.
const FADE_IN_S: f32 = 0.08;
/// A popping symbol starts at this share of its size.
const POP_FROM: f32 = 0.35;
/// A deleted symbol shrinks to this share of its size as it fades.
const VANISH_TO: f32 = 0.4;
/// How many deletions can fade out at once (fast backspacing).
const FADE_SLOTS: usize = 4;
/// Points per full turn of an outline's arcs: smooth at the largest size
/// a symbol is drawn at.
const ARC_STEPS_PER_TURN: f32 = 48.0;
/// The SVG view box (the outline's unit box scaled up for compact text).
const SVG_BOX: f32 = 100.0;

/// One masked character's shape.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Symbol {
    Circle,
    Square,
    Triangle,
    Diamond,
    Pentagon,
    Arch,
    Semicircle,
    Oval,
    Cookie6,
    Clover4,
}

impl Symbol {
    /// How many shapes there are.
    pub const COUNT: usize = 10;

    /// Every shape, in the order shapes are shuffled from. Chosen from
    /// Material's expressive set for reading apart at text size: shapes
    /// with fine detail (bursts, 8-leaf clovers, many-sided cookies) blur
    /// into circles there, and star-like ones are avoided altogether.
    pub const ALL: [Symbol; Symbol::COUNT] = [
        Symbol::Circle,
        Symbol::Square,
        Symbol::Triangle,
        Symbol::Diamond,
        Symbol::Pentagon,
        Symbol::Arch,
        Symbol::Semicircle,
        Symbol::Oval,
        Symbol::Cookie6,
        Symbol::Clover4,
    ];

    fn index(self) -> usize {
        self as usize
    }

    /// The asset path GPUI loads this shape from (served by the shell's
    /// asset source from [`Symbol::svg`]).
    pub fn asset_path(self) -> &'static str {
        match self {
            Symbol::Circle => "password-symbols/circle.svg",
            Symbol::Square => "password-symbols/square.svg",
            Symbol::Triangle => "password-symbols/triangle.svg",
            Symbol::Diamond => "password-symbols/diamond.svg",
            Symbol::Pentagon => "password-symbols/pentagon.svg",
            Symbol::Arch => "password-symbols/arch.svg",
            Symbol::Semicircle => "password-symbols/semicircle.svg",
            Symbol::Oval => "password-symbols/oval.svg",
            Symbol::Cookie6 => "password-symbols/cookie-6.svg",
            Symbol::Clover4 => "password-symbols/clover-4.svg",
        }
    }

    /// The shape for an asset path, if it is one.
    pub fn from_asset_path(path: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.asset_path() == path)
    }

    /// The closed outline in a unit box (fitted and centred, y down,
    /// clockwise on screen).
    pub fn outline(self) -> &'static [(f32, f32)] {
        static OUTLINES: OnceLock<[Vec<(f32, f32)>; Symbol::COUNT]> = OnceLock::new();
        &OUTLINES.get_or_init(|| Symbol::ALL.map(|s| normalized(s.raw_outline())))[self.index()]
    }

    /// A single-colour SVG of the shape (black, tinted when drawn).
    pub fn svg(self) -> &'static str {
        static SVGS: OnceLock<[String; Symbol::COUNT]> = OnceLock::new();
        &SVGS.get_or_init(|| Symbol::ALL.map(|s| svg_text(s.outline())))[self.index()]
    }

    /// The outline as a `tiny_skia` path in a unit box: fill it with a
    /// transform that scales by the symbol's size.
    pub fn skia_path(self) -> Option<&'static tiny_skia::Path> {
        static PATHS: OnceLock<[Option<tiny_skia::Path>; Symbol::COUNT]> = OnceLock::new();
        PATHS.get_or_init(|| Symbol::ALL.map(|s| crate::shape::to_skia(s.outline().iter().copied()))).get(self.index())?.as_ref()
    }

    /// Corner list of the shape, in its source coordinates, after the
    /// Material 3 expressive shape definitions (`MaterialShapes`).
    fn raw_outline(self) -> Vec<(f32, f32)> {
        match self {
            Symbol::Circle => ellipse(1.0, 1.0, 0.0),
            Symbol::Square => rounded(&rect(1.0, 1.0, [0.3; 4])),
            Symbol::Triangle => rounded(&rotated(&regular(3, 0.2), -90.0)),
            Symbol::Diamond => rounded(&repeat(&[(0.500, 1.096, 0.151), (0.040, 0.500, 0.159)], 2, false)),
            Symbol::Pentagon => rounded(&repeat(&[(0.500, -0.009, 0.172), (1.030, 0.365, 0.164), (0.828, 0.970, 0.169)], 1, true)),
            Symbol::Arch => {
                let mut v = regular(4, 0.0);
                for (c, r) in v.iter_mut().zip([1.0, 1.0, 0.2, 0.2]) {
                    c.2 = r;
                }
                rounded(&rotated(&v, -135.0))
            }
            Symbol::Semicircle => rounded(&rect(1.6, 1.0, [0.2, 0.2, 1.0, 1.0])),
            Symbol::Oval => ellipse(1.0, 0.64, -45.0),
            Symbol::Cookie6 => rounded(&repeat(&[(0.723, 0.884, 0.394), (0.500, 1.099, 0.398)], 6, false)),
            Symbol::Clover4 => rounded(&repeat(&[(0.500, 0.074, 0.0), (0.725, -0.099, 0.476)], 4, true)),
        }
    }
}

// ── Shape sequence ──────────────────────────────────────────────────────

/// SplitMix64: a well-mixed 64-bit step.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// A fresh seed for the next entry (after the field is cleared), so each
/// attempt shows a different sequence.
pub fn next_seed(seed: u64) -> u64 {
    mix(seed)
}

/// A seed for a new screen, from the clock.
pub fn seed_from_clock() -> u64 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
    mix(nanos ^ u64::from(std::process::id()))
}

/// The shuffled shape order of one cycle of [`Symbol::ALL`].
fn cycle(seed: u64, c: usize) -> [u8; Symbol::ALL.len()] {
    let mut order: [u8; Symbol::ALL.len()] = std::array::from_fn(|i| i as u8);
    let mut x = mix(seed ^ (c as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93));
    for i in (1..order.len()).rev() {
        x = mix(x);
        order.swap(i, (x % (i as u64 + 1)) as usize);
    }
    order
}

/// The shape of the character at `ix` in an entry started with `seed`.
///
/// Every run of [`Symbol::ALL`]`.len()` characters uses each shape once, in
/// an order shuffled per run; where one run ends with the shape the next
/// would start with, the next run's first two swap. So neighbours always
/// differ, and a position's shape never depends on what follows it.
pub fn symbol_at(seed: u64, ix: usize) -> Symbol {
    let n = Symbol::ALL.len();
    let c = ix / n;
    let mut order = cycle(seed, c);
    if c > 0 && order[0] == cycle(seed, c - 1)[n - 1] {
        order.swap(0, 1);
    }
    Symbol::ALL[order[ix % n] as usize]
}

// ── Layout ──────────────────────────────────────────────────────────────

/// The masked row of a password field: a fixed box per symbol at a fixed
/// advance, both whole device pixels.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Row {
    /// Side of a symbol's box, logical px.
    pub size: f32,
    /// Start of one box to the start of the next, logical px.
    pub advance: f32,
    /// Symbols that fit; more scroll.
    pub capacity: usize,
}

impl Row {
    /// The row for `width` logical px of room at `text_size`, on a surface
    /// with `scale` device pixels per logical pixel.
    pub fn new(width: f32, text_size: f32, scale: f32) -> Self {
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let snap = |v: f32| (v * scale).round().max(1.0) / scale;
        let size = snap(text_size * SIZE_EM);
        let advance = snap(text_size * ADVANCE_EM).max(size);
        let room = if width.is_finite() { width } else { 0.0 };
        let capacity = if room <= size { 1 } else { ((room - size) / advance).floor() as usize + 1 };
        Self { size, advance, capacity }
    }

    /// The first symbol shown when `n` are typed (the rest scrolled out).
    pub fn first_shown(&self, n: usize) -> usize {
        n.saturating_sub(self.capacity)
    }

    /// Distance of symbol `ix`'s box from the start edge (left, or right in
    /// right-to-left languages) with `n` typed, or `None` when it is
    /// scrolled out. A symbol deleted a moment ago (`ix == n`) keeps the
    /// slot it had, so it can fade out in place.
    pub fn offset(&self, ix: usize, n: usize) -> Option<f32> {
        let first = self.first_shown(n);
        (ix >= first && ix <= n && ix - first < self.capacity).then(|| (ix - first) as f32 * self.advance)
    }

    /// Where the caret goes after the last symbol, from the start edge.
    pub fn caret_offset(&self, n: usize) -> f32 {
        if n == 0 { 0.0 } else { (n - self.first_shown(n)) as f32 * self.advance - (self.advance - self.size) / 2.0 }
    }

    /// The left edge of symbol `ix`'s box in a field running from `left`
    /// for `width`, mirrored for right-to-left.
    pub fn left(&self, ix: usize, n: usize, left: f32, width: f32, rtl: bool) -> Option<f32> {
        self.offset(ix, n).map(|o| if rtl { left + width - o - self.size } else { left + o })
    }
}

// ── Motion ──────────────────────────────────────────────────────────────

/// Scale and opacity of a symbol `age` after it was typed.
pub fn appear(age: Duration, animate: bool) -> (f32, f32) {
    if !animate || age >= POP {
        return (1.0, 1.0);
    }
    let t = age.as_secs_f32();
    let x = POP_SPRING.solve(1.0 - POP_FROM, 0.0, t).0;
    ((1.0 - x).max(0.0), (t / FADE_IN_S).min(1.0))
}

/// Scale and opacity of a deleted symbol `age` after its deletion, or
/// `None` once it is gone.
pub fn vanish(age: Duration, animate: bool) -> Option<(f32, f32)> {
    if !animate || age >= FADE {
        return None;
    }
    let k = age.as_secs_f32() / FADE.as_secs_f32();
    let ease = k * k;
    Some((1.0 - (1.0 - VANISH_TO) * ease, 1.0 - ease))
}

/// Deleted symbols still fading out, in a fixed handful of slots.
#[derive(Copy, Clone, Debug, Default)]
pub struct Fades {
    slots: [Option<(usize, Instant)>; FADE_SLOTS],
}

impl Fades {
    /// Symbol `ix` was deleted at `now`.
    pub fn push(&mut self, ix: usize, now: Instant) {
        // A slot whose fade is over, else the oldest.
        let slot = self.slots.iter().position(|s| s.is_none_or(|(_, at)| now.saturating_duration_since(at) >= FADE)).unwrap_or_else(|| {
            let oldest = self.slots.iter().enumerate().min_by_key(|(_, s)| s.map(|(_, at)| at));
            oldest.map_or(0, |(i, _)| i)
        });
        self.slots[slot] = Some((ix, now));
    }

    /// A character was typed at `ix`: an older fade there is superseded.
    pub fn typed(&mut self, ix: usize) {
        for s in &mut self.slots {
            if s.is_some_and(|(i, _)| i >= ix) {
                *s = None;
            }
        }
    }

    pub fn clear(&mut self) {
        self.slots = [None; FADE_SLOTS];
    }

    /// Fades still playing: `(index, scale, opacity)`.
    pub fn live(&self, now: Instant, animate: bool) -> impl Iterator<Item = (usize, f32, f32)> + '_ {
        self.slots.iter().flatten().filter_map(move |&(ix, at)| vanish(now.saturating_duration_since(at), animate).map(|(s, a)| (ix, s, a)))
    }

    /// Whether any fade still needs frames.
    pub fn active(&self, now: Instant, animate: bool) -> bool {
        self.live(now, animate).next().is_some()
    }
}

// ── Geometry ────────────────────────────────────────────────────────────

/// A corner: position and rounding radius (in the shape's own units).
type Corner = (f32, f32, f32);

/// A regular polygon of radius 1 around the origin, first corner at +x.
fn regular(n: usize, rounding: f32) -> Vec<Corner> {
    (0..n)
        .map(|i| {
            let a = TAU * i as f32 / n as f32;
            (a.cos(), a.sin(), rounding)
        })
        .collect()
}

/// A rectangle around the origin: corners bottom-right, bottom-left,
/// top-left, top-right (the Material order).
fn rect(w: f32, h: f32, r: [f32; 4]) -> Vec<Corner> {
    let (x, y) = (w / 2.0, h / 2.0);
    vec![(x, y, r[0]), (-x, y, r[1]), (-x, -y, r[2]), (x, -y, r[3])]
}

fn rotated(corners: &[Corner], degrees: f32) -> Vec<Corner> {
    corners
        .iter()
        .map(|&(x, y, r)| {
            let (px, py) = crate::shape::rotate((x, y), (0.0, 0.0), degrees);
            (px, py, r)
        })
        .collect()
}

/// Material's `customPolygon`: `points` around the centre (0.5, 0.5),
/// repeated `reps` times by rotation, or mirrored into `2 × reps` sections.
fn repeat(points: &[Corner], reps: usize, mirroring: bool) -> Vec<Corner> {
    let (cx, cy) = (0.5, 0.5);
    let polar: Vec<(f32, f32, f32)> = points.iter().map(|&(x, y, r)| ((y - cy).atan2(x - cx), ((x - cx).powi(2) + (y - cy).powi(2)).sqrt(), r)).collect();
    let place = |a: f32, d: f32, r: f32| (cx + d * a.cos(), cy + d * a.sin(), r);
    let mut out = Vec::new();
    if mirroring {
        let sections = reps * 2;
        let section = TAU / sections as f32;
        for it in 0..sections {
            let even = it % 2 == 0;
            for k in 0..polar.len() {
                let i = if even { k } else { polar.len() - 1 - k };
                if i == 0 && !even {
                    continue;
                }
                let (a, d, r) = polar[i];
                let angle = if even { section * it as f32 + a } else { section * (it + 1) as f32 - a + 2.0 * polar[0].0 };
                out.push(place(angle, d, r));
            }
        }
    } else {
        for it in 0..reps {
            for &(a, d, r) in &polar {
                out.push(place(a + TAU * it as f32 / reps as f32, d, r));
            }
        }
    }
    out
}

/// An ellipse with radii `rx`, `ry`, turned by `degrees`.
fn ellipse(rx: f32, ry: f32, degrees: f32) -> Vec<(f32, f32)> {
    let n = ARC_STEPS_PER_TURN as usize * 2;
    (0..n)
        .map(|i| {
            let a = TAU * i as f32 / n as f32 - PI / 2.0;
            crate::shape::rotate((rx * a.cos(), ry * a.sin()), (0.0, 0.0), degrees)
        })
        .collect()
}

/// The outline of `corners` with each corner rounded by its own radius.
/// Where two roundings would overlap on an edge, both shrink in
/// proportion (as Material's rounded polygons do).
fn rounded(corners: &[Corner]) -> Vec<(f32, f32)> {
    let n = corners.len();
    let pt = |i: usize| (corners[i % n].0, corners[i % n].1);
    // Per corner: half the turn between its edges and how far along each
    // edge its rounding wants to reach.
    let mut half = vec![0.0f32; n];
    let mut want = vec![0.0f32; n];
    for i in 0..n {
        let (p, c, q) = (pt(i + n - 1), pt(i), pt(i + 1));
        let (a, b) = (unit(sub(p, c)), unit(sub(q, c)));
        let h = (a.0 * b.0 + a.1 * b.1).clamp(-1.0, 1.0).acos() / 2.0;
        half[i] = h;
        want[i] = if h > 1e-3 && corners[i].2 > 0.0 { corners[i].2 / h.tan() } else { 0.0 };
    }
    // Each edge's room, shared in proportion between its two corners.
    let fit: Vec<f32> = (0..n)
        .map(|i| {
            let (len, w) = (dist(pt(i), pt(i + 1)), want[i] + want[(i + 1) % n]);
            if w > len && w > 0.0 { len / w } else { 1.0 }
        })
        .collect();
    let mut out = Vec::with_capacity(n * 8);
    for i in 0..n {
        let reach = want[i] * fit[i].min(fit[(i + n - 1) % n]);
        let c = pt(i);
        if reach <= 1e-5 {
            out.push(c);
            continue;
        }
        let (a, b) = (unit(sub(pt(i + n - 1), c)), unit(sub(pt(i + 1), c)));
        let r = reach * half[i].tan();
        let bis = unit((a.0 + b.0, a.1 + b.1));
        let d = r / half[i].sin();
        let o = (c.0 + bis.0 * d, c.1 + bis.1 * d);
        let (s, e) = ((c.0 + a.0 * reach, c.1 + a.1 * reach), (c.0 + b.0 * reach, c.1 + b.1 * reach));
        let a0 = (s.1 - o.1).atan2(s.0 - o.0);
        let mut a1 = (e.1 - o.1).atan2(e.0 - o.0);
        while a1 - a0 > PI {
            a1 -= TAU;
        }
        while a0 - a1 > PI {
            a1 += TAU;
        }
        let steps = (((a1 - a0).abs() / TAU * ARC_STEPS_PER_TURN).ceil() as usize).max(2);
        for k in 0..=steps {
            let t = a0 + (a1 - a0) * k as f32 / steps as f32;
            out.push((o.0 + r * t.cos(), o.1 + r * t.sin()));
        }
    }
    out
}

/// `pts` fitted into the unit box (aspect kept), centred, clockwise on
/// screen.
fn normalized(mut pts: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in &pts {
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    let side = (x1 - x0).max(y1 - y0).max(1e-6);
    let (ox, oy) = ((side - (x1 - x0)) / 2.0, (side - (y1 - y0)) / 2.0);
    for p in &mut pts {
        *p = ((p.0 - x0 + ox) / side, (p.1 - y0 + oy) / side);
    }
    // Shoelace: positive area is clockwise with y down.
    let area: f32 = (0..pts.len()).map(|i| {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        a.0 * b.1 - b.0 * a.1
    }).sum();
    if area < 0.0 {
        pts.reverse();
    }
    pts
}

fn svg_text(pts: &[(f32, f32)]) -> String {
    use std::fmt::Write as _;
    let mut d = String::with_capacity(pts.len() * 14 + 120);
    let _ = write!(d, r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {SVG_BOX} {SVG_BOX}"><path fill="black" d=""#);
    for (i, (x, y)) in pts.iter().enumerate() {
        let _ = write!(d, "{}{:.2} {:.2}", if i == 0 { "M" } else { "L" }, x * SVG_BOX, y * SVG_BOX);
    }
    d.push_str(r#"Z"/></svg>"#);
    d
}

fn sub(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    (a.0 - b.0, a.1 - b.1)
}

fn unit((x, y): (f32, f32)) -> (f32, f32) {
    let l = (x * x + y * y).sqrt().max(1e-6);
    (x / l, y / l)
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    let (x, y) = sub(a, b);
    (x * x + y * y).sqrt()
}

#[cfg(test)]
#[path = "password_symbols_tests.rs"]
mod tests;
