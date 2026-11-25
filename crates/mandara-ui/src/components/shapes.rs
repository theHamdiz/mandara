//! Material 3 Expressive shapes (cookie, clover, flower, burst) for round
//! buttons and badges: the bar's icon items (`bar.icon_shape`) and the
//! side bars' pill items.
//!
//! Each shape is the union of a body disc and a ring of lobe discs, all
//! painted as SDF circles: every edge is anti-aliased by the quad shader
//! itself (no tessellated path, no row bands), it costs one quad per disc,
//! and nothing is allocated. Neighbouring lobes overlap, so where they meet
//! the outline dips into a soft dent; the body fills the middle without
//! reaching past the dents. The discs overlap, so the shapes need an opaque
//! colour (the bar composites its item colour over its own fill first and
//! falls back to a plain circle over a see-through bar).
//!
//! A shape's unit discs are worked out once per shape and kept
//! ([`unit`]); a frame only scales them into place ([`discs`]).

use std::sync::OnceLock;

use gpui::{Bounds, Pixels, Window, point, px, size};
use mandara_core::ToGpui as _;
use mandara_theme::Color;

/// A Material 3 Expressive shape.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Shape {
    Circle,
    /// Nine soft lobes.
    Cookie,
    /// Four round leaves on the diagonals.
    Clover,
    /// Eight round petals.
    Flower,
    /// Twelve small scallops.
    Burst,
}

impl Shape {
    pub const ALL: [Shape; 5] = [Shape::Circle, Shape::Cookie, Shape::Clover, Shape::Flower, Shape::Burst];

    fn index(self) -> usize {
        match self {
            Shape::Circle => 0,
            Shape::Cookie => 1,
            Shape::Clover => 2,
            Shape::Flower => 3,
            Shape::Burst => 4,
        }
    }

    /// How a shape is built: its lobes, their radius (of the shape's
    /// radius 1), the body's radius and where the first lobe points
    /// (radians from straight up, clockwise).
    fn spec(self) -> Spec {
        match self {
            Shape::Circle => Spec { lobes: 0, lobe: 0.0, body: 1.0, phase: 0.0 },
            Shape::Cookie => Spec { lobes: 9, lobe: 0.38, body: 0.86, phase: 0.0 },
            Shape::Clover => Spec { lobes: 4, lobe: 0.5, body: 0.66, phase: std::f32::consts::FRAC_PI_4 },
            Shape::Flower => Spec { lobes: 8, lobe: 0.32, body: 0.78, phase: 0.0 },
            Shape::Burst => Spec { lobes: 12, lobe: 0.24, body: 0.84, phase: 0.0 },
        }
    }
}

#[derive(Copy, Clone, Debug)]
struct Spec {
    lobes: u32,
    lobe: f32,
    body: f32,
    phase: f32,
}

/// The most discs a shape is made of (twelve lobes and the body).
pub const MAX_DISCS: usize = 13;

/// One disc: its centre and radius. In unit space a shape fills the
/// circle of radius 1 around the origin, y pointing down.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Disc {
    pub x: f32,
    pub y: f32,
    pub r: f32,
}

/// A shape's discs in unit space and how many there are.
type UnitDiscs = ([Disc; MAX_DISCS], usize);

static UNITS: [OnceLock<UnitDiscs>; 5] = [OnceLock::new(), OnceLock::new(), OnceLock::new(), OnceLock::new(), OnceLock::new()];

fn build(shape: Shape) -> UnitDiscs {
    let s = shape.spec();
    let mut out = [Disc::default(); MAX_DISCS];
    out[0] = Disc { x: 0.0, y: 0.0, r: s.body };
    let ring = 1.0 - s.lobe;
    for i in 0..s.lobes as usize {
        let a = s.phase + std::f32::consts::TAU * i as f32 / s.lobes as f32;
        out[i + 1] = Disc { x: ring * a.sin(), y: -ring * a.cos(), r: s.lobe };
    }
    (out, 1 + s.lobes as usize)
}

/// `shape`'s discs in unit space, worked out on first use and kept.
pub fn unit(shape: Shape) -> &'static [Disc] {
    let (discs, n) = UNITS[shape.index()].get_or_init(|| build(shape));
    &discs[..*n]
}

/// The discs of `shape` filling the `d`-wide square at `(x, y)`.
pub fn discs(shape: Shape, x: f32, y: f32, d: f32) -> impl Iterator<Item = Disc> {
    let half = d.max(0.0) / 2.0;
    let (cx, cy) = (x + half, y + half);
    unit(shape).iter().map(move |u| Disc { x: cx + u.x * half, y: cy + u.y * half, r: u.r * half })
}

/// Paints `shape` in `color` (opaque, see the module docs) centred in
/// `bounds`, as large as its shorter side.
pub fn paint(window: &mut Window, bounds: Bounds<Pixels>, shape: Shape, color: Color) {
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let d = w.min(h);
    if d <= 0.0 {
        return;
    }
    let (x, y) = (f32::from(bounds.origin.x) + (w - d) / 2.0, f32::from(bounds.origin.y) + (h - d) / 2.0);
    let fill = color.hsla();
    for disc in discs(shape, x, y, d) {
        let b = Bounds::new(point(px(disc.x - disc.r), px(disc.y - disc.r)), size(px(disc.r * 2.0), px(disc.r * 2.0)));
        window.paint_subpixel_quad(gpui::fill(b, fill).corner_radii(px(disc.r)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether a unit-space point is inside the shape.
    fn inside(shape: Shape, x: f32, y: f32) -> bool {
        unit(shape).iter().any(|d| (x - d.x).hypot(y - d.y) <= d.r)
    }

    #[test]
    fn every_shape_fills_its_circle_and_no_more() {
        for shape in Shape::ALL {
            let discs = unit(shape);
            assert!(!discs.is_empty() && discs.len() <= MAX_DISCS);
            for d in discs {
                assert!(d.x.hypot(d.y) + d.r <= 1.0 + 1e-5, "{shape:?}: {d:?} stays inside the circle");
                assert!(d.r > 0.0);
            }
            // The middle and the tip pointing up are filled.
            assert!(inside(shape, 0.0, 0.0), "{shape:?}: the middle");
            let tip = if shape == Shape::Clover { (std::f32::consts::FRAC_1_SQRT_2 * 0.98, -std::f32::consts::FRAC_1_SQRT_2 * 0.98) } else { (0.0, -0.98) };
            assert!(inside(shape, tip.0, tip.1), "{shape:?}: its first lobe");
        }
        // A circle is one disc.
        assert_eq!(unit(Shape::Circle), &[Disc { x: 0.0, y: 0.0, r: 1.0 }]);
    }

    #[test]
    fn lobed_shapes_dip_between_their_lobes_and_the_body_stays_inside() {
        for shape in [Shape::Cookie, Shape::Clover, Shape::Flower, Shape::Burst] {
            let s = shape.spec();
            let ring = 1.0 - s.lobe;
            let step = std::f32::consts::TAU / s.lobes as f32;
            for i in 0..s.lobes {
                // Halfway between two lobes, near the edge: outside (a dent).
                let a = s.phase + step * (i as f32 + 0.5);
                assert!(!inside(shape, 0.97 * a.sin(), -0.97 * a.cos()), "{shape:?}: a dent between lobes {i} and {}", i + 1);
                // Where neighbouring lobes meet is the dent's depth: the body
                // never reaches past it (else the dents would fill in).
                let half = ring * (step / 2.0).sin();
                let dent = ring * (step / 2.0).cos() + (s.lobe * s.lobe - half * half).max(0.0).sqrt();
                assert!(s.body <= dent + 1e-4, "{shape:?}: body {} inside the dent {dent}", s.body);
                assert!(s.lobe > half, "{shape:?}: neighbouring lobes overlap (no gaps)");
                // And every lobe overlaps the body (one connected shape).
                assert!(ring - s.lobe < s.body);
            }
        }
    }

    #[test]
    fn shapes_are_mirror_symmetric_so_they_read_the_same_both_ways() {
        for shape in Shape::ALL {
            for d in unit(shape) {
                assert!(unit(shape).iter().any(|e| (e.x + d.x).abs() < 1e-4 && (e.y - d.y).abs() < 1e-4 && (e.r - d.r).abs() < 1e-6), "{shape:?}: {d:?} has a mirror twin");
            }
        }
    }

    #[test]
    fn placed_discs_stay_in_their_square() {
        for shape in Shape::ALL {
            for (x, y, d) in [(10.0, 4.0, 40.0), (0.0, 0.0, 18.5), (-3.0, 7.0, 0.0)] {
                for disc in discs(shape, x, y, d) {
                    assert!(disc.x - disc.r >= x - 1e-3 && disc.x + disc.r <= x + d + 1e-3);
                    assert!(disc.y - disc.r >= y - 1e-3 && disc.y + disc.r <= y + d + 1e-3);
                }
            }
        }
    }
}
