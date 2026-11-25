//! Shape outlines shared by every surface that can take a shape other than
//! a rounded rectangle: desktop-widget plates today, bar segments and
//! panels later.
//!
//! [`Shape::outline`] turns a shape and a box into a closed polygon (box
//! coordinates, clockwise in screen space, starting at the top). The
//! polygon is sampled finely enough to look smooth at its size, so the same
//! points can fill a `tiny_skia` path ([`Shape::skia_path`]) on the software
//! canvases or a GPUI path ([`Shape::gpui_path`]) on GPU surfaces.
//!
//! Everything here is pure geometry: deterministic (the deckled edge is
//! seeded), allocation-light and independent of the theme.

use std::f32::consts::{PI, TAU};

/// A shape a surface can take, with its own parameters.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Shape {
    /// A rectangle with circular corners of `radius` (clamped to half the
    /// shorter side).
    Rect { radius: f32 },
    /// An ellipse filling the box (a circle in a square box).
    Circle,
    /// A superellipse: flat sides with continuous-curvature corners.
    Squircle,
    /// A scalloped "cookie": `lobes` round bumps around an ellipse, each
    /// `depth` (fraction of the radius) deep.
    Cookie { lobes: u32, depth: f32 },
    /// A flower: `petals` petals with pinched joins, `depth` deep.
    Flower { petals: u32, depth: f32 },
    /// A regular polygon with `sides` sides (5 = pentagon), point up,
    /// corners rounded by `radius`.
    Polygon { sides: u32, radius: f32 },
    /// A rectangle with its corners cut off at 45° by `cut`.
    Chamfer { cut: f32 },
    /// A parallelogram leaning by `degrees` (positive leans right),
    /// corners rounded by `radius`.
    Skew { degrees: f32, radius: f32 },
    /// A rectangle with torn ("deckled") paper edges: `depth` is the tear's
    /// amplitude as a fraction of the shorter side, `seed` picks the tear.
    Deckle { depth: f32, seed: u32 },
}

/// Points per full turn of a curve at 100 px radius; scaled with the size
/// so small shapes stay cheap and big ones smooth.
const POINTS_PER_TURN_AT_100: f32 = 96.0;
/// Fewest and most points a curved outline gets.
const MIN_POINTS: usize = 24;
const MAX_POINTS: usize = 720;
/// Superellipse exponent: 2 is an ellipse, higher is squarer. Five reads
/// as a soft "squircle" at any aspect.
const SQUIRCLE_EXPONENT: f32 = 5.0;
/// Points per corner arc of a rounded polygon.
const CORNER_STEPS: usize = 10;
/// Fewest and most sides / lobes a polygon or a cookie can have.
pub const MIN_SIDES: u32 = 3;
pub const MAX_LOBES: u32 = 24;
/// Deepest lobe or tear, as a fraction of the radius / shorter side.
pub const MAX_DEPTH: f32 = 0.45;
/// Steepest lean of a skewed shape, degrees.
pub const MAX_SKEW_DEG: f32 = 45.0;
/// Length of one tear of a deckled edge, px.
const DECKLE_STEP_PX: f32 = 5.0;

impl Shape {
    /// A plain rectangle (square corners).
    pub const SQUARE: Shape = Shape::Rect { radius: 0.0 };

    /// The closed outline of the shape in a `w × h` box at the origin.
    pub fn outline(&self, w: f32, h: f32) -> Vec<(f32, f32)> {
        if w <= 0.0 || h <= 0.0 {
            return Vec::new();
        }
        let (cx, cy, rx, ry) = (w / 2.0, h / 2.0, w / 2.0, h / 2.0);
        let n = samples(rx.max(ry));
        match *self {
            Shape::Rect { radius } => rounded_polygon(&[(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)], radius.clamp(0.0, w.min(h) / 2.0)),
            Shape::Circle => polar(n, |_| 1.0, (cx, cy, rx, ry)),
            Shape::Squircle => (0..n)
                .map(|i| {
                    let a = i as f32 / n as f32 * TAU - PI / 2.0;
                    let (s, c) = a.sin_cos();
                    let e = 2.0 / SQUIRCLE_EXPONENT;
                    (cx + rx * c.signum() * c.abs().powf(e), cy + ry * s.signum() * s.abs().powf(e))
                })
                .collect(),
            Shape::Cookie { lobes, depth } => {
                let (lobes, d) = (lobes.clamp(MIN_SIDES, MAX_LOBES) as f32, depth.clamp(0.0, MAX_DEPTH));
                // Round bumps, round valleys: a raised cosine between 1 − d and 1.
                polar(n.max(lobes as usize * CORNER_STEPS), |a| 1.0 - d * (1.0 - (lobes * a).cos()) / 2.0, (cx, cy, rx, ry))
            }
            Shape::Flower { petals, depth } => {
                let (petals, d) = (petals.clamp(MIN_SIDES, MAX_LOBES) as f32, depth.clamp(0.0, MAX_DEPTH));
                // Full petals that pinch to a point between them.
                polar(n.max(petals as usize * CORNER_STEPS), |a| 1.0 - d + d * (petals * a / 2.0).cos().abs(), (cx, cy, rx, ry))
            }
            Shape::Polygon { sides, radius } => {
                let sides = sides.clamp(MIN_SIDES, MAX_LOBES);
                let corners: Vec<(f32, f32)> = (0..sides)
                    .map(|i| {
                        let a = i as f32 / sides as f32 * TAU - PI / 2.0;
                        (a.cos(), a.sin())
                    })
                    .collect();
                // Stretch the polygon's own bounds to fill the box.
                let (x0, x1) = corners.iter().fold((f32::MAX, f32::MIN), |b, p| (b.0.min(p.0), b.1.max(p.0)));
                let (y0, y1) = corners.iter().fold((f32::MAX, f32::MIN), |b, p| (b.0.min(p.1), b.1.max(p.1)));
                let fit: Vec<(f32, f32)> = corners.iter().map(|p| ((p.0 - x0) / (x1 - x0) * w, (p.1 - y0) / (y1 - y0) * h)).collect();
                rounded_polygon(&fit, radius.max(0.0))
            }
            Shape::Chamfer { cut } => {
                let c = cut.clamp(0.0, w.min(h) / 2.0);
                vec![(c, 0.0), (w - c, 0.0), (w, c), (w, h - c), (w - c, h), (c, h), (0.0, h - c), (0.0, c)]
            }
            Shape::Skew { degrees, radius } => {
                let lean = h * degrees.clamp(-MAX_SKEW_DEG, MAX_SKEW_DEG).to_radians().tan();
                // Kept inside the box: the slant eats into the width.
                let lean = lean.clamp(-w / 2.0, w / 2.0);
                let (l, r) = (lean.max(0.0), (-lean).max(0.0));
                let pts = [(l, 0.0), (w - r, 0.0), (w - l, h), (r, h)];
                rounded_polygon(&pts, radius.max(0.0))
            }
            Shape::Deckle { depth, seed } => deckle(w, h, depth.clamp(0.0, MAX_DEPTH) * w.min(h), seed),
        }
    }

    /// Whether this is a plain rounded rectangle (drawn with exact arcs
    /// by the callers that have them).
    pub fn is_rect(&self) -> bool {
        matches!(self, Shape::Rect { .. })
    }

    /// The outline as a filled `tiny_skia` path at `(x, y)`.
    pub fn skia_path(&self, x: f32, y: f32, w: f32, h: f32) -> Option<tiny_skia::Path> {
        to_skia(self.outline(w, h).into_iter().map(|(px, py)| (x + px, y + py)))
    }

    /// The outline as a filled GPUI path at `origin` (logical px).
    pub fn gpui_path(&self, origin: gpui::Point<gpui::Pixels>, w: f32, h: f32) -> Option<gpui::Path<gpui::Pixels>> {
        let pts = self.outline(w, h);
        let (first, rest) = pts.split_first()?;
        let at = |p: &(f32, f32)| gpui::point(origin.x + gpui::px(p.0), origin.y + gpui::px(p.1));
        let mut pb = gpui::PathBuilder::fill();
        pb.move_to(at(first));
        for p in rest {
            pb.line_to(at(p));
        }
        pb.close();
        pb.build().ok()
    }
}

/// A closed `tiny_skia` path through `pts`.
pub fn to_skia(pts: impl IntoIterator<Item = (f32, f32)>) -> Option<tiny_skia::Path> {
    let mut pb = tiny_skia::PathBuilder::new();
    for (i, (x, y)) in pts.into_iter().enumerate() {
        if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
    }
    pb.close();
    pb.finish()
}

/// Points for a curve of radius `r` px.
fn samples(r: f32) -> usize {
    ((r / 100.0 * POINTS_PER_TURN_AT_100).ceil() as usize).clamp(MIN_POINTS, MAX_POINTS)
}

/// `n` points of the polar curve `radius(angle)` (1 = the box's ellipse),
/// starting at the top and going clockwise on screen.
fn polar(n: usize, radius: impl Fn(f32) -> f32, (cx, cy, rx, ry): (f32, f32, f32, f32)) -> Vec<(f32, f32)> {
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32 * TAU;
            let a = t - PI / 2.0;
            let r = radius(t);
            (cx + rx * r * a.cos(), cy + ry * r * a.sin())
        })
        .collect()
}

/// `corners` with each corner replaced by a circular arc of `radius`
/// (shrunk where the neighbouring edges are too short).
pub fn rounded_polygon(corners: &[(f32, f32)], radius: f32) -> Vec<(f32, f32)> {
    let n = corners.len();
    if radius <= 0.0 || n < 3 {
        return corners.to_vec();
    }
    let mut out = Vec::with_capacity(n * (CORNER_STEPS + 1));
    for i in 0..n {
        let (p, c, q) = (corners[(i + n - 1) % n], corners[i], corners[(i + 1) % n]);
        let (a, b) = (unit((p.0 - c.0, p.1 - c.1)), unit((q.0 - c.0, q.1 - c.1)));
        let cos = (a.0 * b.0 + a.1 * b.1).clamp(-1.0, 1.0);
        let half = cos.acos() / 2.0;
        if half < 1e-3 {
            out.push(c);
            continue;
        }
        // Distance from the corner to where the arc meets each edge,
        // at most half of either edge.
        let reach = (radius / half.tan()).min(dist(p, c) / 2.0).min(dist(q, c) / 2.0);
        let r = reach * half.tan();
        let (s, e) = ((c.0 + a.0 * reach, c.1 + a.1 * reach), (c.0 + b.0 * reach, c.1 + b.1 * reach));
        // The arc's centre lies on the bisector.
        let bis = unit((a.0 + b.0, a.1 + b.1));
        let d = r / half.sin();
        let o = (c.0 + bis.0 * d, c.1 + bis.1 * d);
        let (a0, mut a1) = ((s.1 - o.1).atan2(s.0 - o.0), (e.1 - o.1).atan2(e.0 - o.0));
        // The short way round.
        while a1 - a0 > PI {
            a1 -= TAU;
        }
        while a0 - a1 > PI {
            a1 += TAU;
        }
        for k in 0..=CORNER_STEPS {
            let t = a0 + (a1 - a0) * k as f32 / CORNER_STEPS as f32;
            out.push((o.0 + r * t.cos(), o.1 + r * t.sin()));
        }
    }
    out
}

fn unit((x, y): (f32, f32)) -> (f32, f32) {
    let l = (x * x + y * y).sqrt().max(1e-6);
    (x / l, y / l)
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// A deterministic value in `0..1` for `(seed, i)`.
fn hash01(seed: u32, i: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9) ^ i.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    x as f32 / u32::MAX as f32
}

/// A rectangle whose edges are torn inwards by up to `amp` px.
fn deckle(w: f32, h: f32, amp: f32, seed: u32) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let mut k = 0u32;
    // Each edge from one corner to the next, the tear pointing inwards.
    let edges = [((0.0, 0.0), (w, 0.0), (0.0, 1.0)), ((w, 0.0), (w, h), (-1.0, 0.0)), ((w, h), (0.0, h), (0.0, -1.0)), ((0.0, h), (0.0, 0.0), (1.0, 0.0))];
    for (a, b, inward) in edges {
        let len = dist(a, b);
        let steps = ((len / DECKLE_STEP_PX).ceil() as usize).max(2);
        for i in 0..steps {
            let t = i as f32 / steps as f32;
            // Corners stay put so the sheet keeps its size.
            let tear = if i == 0 { 0.0 } else { amp * hash01(seed, k) };
            k += 1;
            out.push((a.0 + (b.0 - a.0) * t + inward.0 * tear, a.1 + (b.1 - a.1) * t + inward.1 * tear));
        }
    }
    out
}

/// Rotates `p` by `degrees` (clockwise on screen) about `c`.
pub fn rotate((x, y): (f32, f32), (cx, cy): (f32, f32), degrees: f32) -> (f32, f32) {
    let (s, c) = degrees.to_radians().sin_cos();
    let (dx, dy) = (x - cx, y - cy);
    (cx + dx * c - dy * s, cy + dx * s + dy * c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(pts: &[(f32, f32)]) -> (f32, f32, f32, f32) {
        pts.iter().fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |b, p| (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1)))
    }

    const ALL: [Shape; 9] = [
        Shape::Rect { radius: 12.0 },
        Shape::Circle,
        Shape::Squircle,
        Shape::Cookie { lobes: 12, depth: 0.1 },
        Shape::Flower { petals: 6, depth: 0.2 },
        Shape::Polygon { sides: 5, radius: 8.0 },
        Shape::Chamfer { cut: 10.0 },
        Shape::Skew { degrees: -12.0, radius: 4.0 },
        Shape::Deckle { depth: 0.04, seed: 7 },
    ];

    #[test]
    fn every_shape_stays_in_its_box_and_closes() {
        for s in ALL {
            let pts = s.outline(200.0, 120.0);
            assert!(pts.len() >= 4, "{s:?}");
            let (x0, y0, x1, y1) = bounds(&pts);
            assert!(x0 >= -0.01 && y0 >= -0.01 && x1 <= 200.01 && y1 <= 120.01, "{s:?}: {x0} {y0} {x1} {y1}");
            // Each fills most of its box.
            assert!(x1 - x0 > 150.0 && y1 - y0 > 90.0, "{s:?}");
            assert!(s.skia_path(0.0, 0.0, 200.0, 120.0).is_some(), "{s:?}");
        }
        assert!(Shape::Circle.outline(0.0, 10.0).is_empty());
    }

    #[test]
    fn circles_are_round_and_cookies_have_their_lobes() {
        let pts = Shape::Circle.outline(100.0, 100.0);
        assert!(pts.iter().all(|p| (dist(*p, (50.0, 50.0)) - 50.0).abs() < 0.01));
        // A 12-lobe cookie touches the full radius 12 times.
        let pts = Shape::Cookie { lobes: 12, depth: 0.2 }.outline(100.0, 100.0);
        let r: Vec<f32> = pts.iter().map(|p| dist(*p, (50.0, 50.0))).collect();
        let peaks = (0..r.len()).filter(|&i| r[i] > r[(i + r.len() - 1) % r.len()] && r[i] >= r[(i + 1) % r.len()]).count();
        assert_eq!(peaks, 12);
        assert!(r.iter().cloned().fold(f32::MAX, f32::min) < 41.0);
    }

    #[test]
    fn rounded_rect_radius_and_chamfer_cut_are_exact() {
        let pts = Shape::Rect { radius: 10.0 }.outline(100.0, 60.0);
        // The corner arc passes 10 px from the corner circle's centre.
        let c = (10.0, 10.0);
        let corner: Vec<&(f32, f32)> = pts.iter().filter(|p| p.0 <= 10.0 && p.1 <= 10.0).collect();
        assert!(!corner.is_empty() && corner.iter().all(|p| (dist(**p, c) - 10.0).abs() < 0.05));
        let ch = Shape::Chamfer { cut: 6.0 }.outline(50.0, 50.0);
        assert_eq!(ch[0], (6.0, 0.0));
        assert_eq!(Shape::SQUARE.outline(5.0, 5.0), vec![(0.0, 0.0), (5.0, 0.0), (5.0, 5.0), (0.0, 5.0)]);
    }

    #[test]
    fn skew_leans_and_deckle_is_deterministic() {
        let pts = Shape::Skew { degrees: 45.0, radius: 0.0 }.outline(200.0, 40.0);
        assert!((pts[0].0 - 40.0).abs() < 0.01 && (pts[3].0 - 0.0).abs() < 0.01, "{pts:?}");
        let a = Shape::Deckle { depth: 0.05, seed: 3 }.outline(100.0, 80.0);
        assert_eq!(a, Shape::Deckle { depth: 0.05, seed: 3 }.outline(100.0, 80.0));
        assert_ne!(a, Shape::Deckle { depth: 0.05, seed: 4 }.outline(100.0, 80.0));
    }

    #[test]
    fn rotation_turns_clockwise() {
        let p = rotate((10.0, 0.0), (0.0, 0.0), 90.0);
        assert!((p.0).abs() < 1e-4 && (p.1 - 10.0).abs() < 1e-4);
    }
}
