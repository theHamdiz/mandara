//! Cut (chamfered) and squircle (superellipse) corners from SDF quads: the
//! Cut button design and the Cut and Squircle card corners.
//!
//! GPUI's quads have circular corners only, and a tessellated path would
//! give the edges hard, uneven steps (and build and allocate a path every
//! frame). So such a shape is drawn as its body plus, across each corner
//! band, one quad per device-pixel row whose ends sit on the curve: the rows
//! tile the band exactly (they start on the device grid) and each end is
//! anti-aliased along the row, which draws a clean diagonal or superellipse
//! edge. An outline is the four straight edges plus a short stroke per row
//! along each corner.
//!
//! The children of a card in these shapes are clipped to a rounded
//! rectangle that fits inside the shape ([`Corner::inscribed_radius`]): the
//! renderer's rounded content masks clip them exactly, never past the
//! drawn edge, without a new mask kind in the shaders.
//!
//! [`shape_rows`] and [`edges`] are the pure geometry (iterators and fixed
//! arrays, nothing allocated), [`paint_shape`] puts it on screen.

use gpui::{Bounds, Pixels, Window, point, px, size};
use mandara_core::{ToGpui as _, oklab_gradient};
use mandara_theme::Color;

/// The most rows one corner band draws (a 24 px corner at 4× scale); a
/// larger one is drawn coarser rather than with more quads.
pub const MAX_ROWS: u32 = 96;
/// The superellipse's exponent: 2 is a circle, higher is squarer; five
/// reads as a smooth "continuous" corner.
pub const SQUIRCLE_EXPONENT: f32 = 5.0;

/// A corner shape and its extent along each edge, logical px.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Corner {
    /// Cut at 45 degrees, `c` along each edge.
    Cut(f32),
    /// A superellipse quadrant reaching `r` along each edge.
    Squircle(f32),
}

impl Corner {
    /// How far the corner reaches along each edge.
    pub fn extent(self) -> f32 {
        match self {
            Corner::Cut(c) | Corner::Squircle(c) => c.max(0.0),
        }
    }

    /// The same corner reaching at most `max`.
    pub fn clamped(self, max: f32) -> Self {
        let max = max.max(0.0);
        match self {
            Corner::Cut(c) => Corner::Cut(c.clamp(0.0, max)),
            Corner::Squircle(r) => Corner::Squircle(r.clamp(0.0, max)),
        }
    }

    /// How far in from its side the shape's edge sits at `d` px from its
    /// top (or bottom) edge.
    pub fn inset(self, d: f32) -> f32 {
        match self {
            Corner::Cut(c) => (c - d).max(0.0),
            Corner::Squircle(r) if r > 0.0 => {
                let dy = ((r - d) / r).clamp(0.0, 1.0);
                r - r * (1.0 - dy.powf(SQUIRCLE_EXPONENT)).max(0.0).powf(1.0 / SQUIRCLE_EXPONENT)
            }
            Corner::Squircle(_) => 0.0,
        }
    }

    /// The radius of a rounded rectangle that fits inside this corner: a
    /// cut's circle touches its diagonal (`c · (1 + 1/√2)`), a squircle's is
    /// its own reach (the superellipse bulges past that circle).
    pub fn inscribed_radius(self) -> f32 {
        match self {
            Corner::Cut(c) => c.max(0.0) * (1.0 + std::f32::consts::FRAC_1_SQRT_2),
            Corner::Squircle(r) => r.max(0.0),
        }
    }
}

/// One device-pixel row of a corner band: its top and height from the
/// shape's (grid-aligned) top, how far its ends sit in from each side, and
/// how wide an outline `1` px thick is across it there, logical px.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Row {
    pub y: f32,
    pub h: f32,
    pub inset: f32,
    /// The edge's horizontal run per unit of outline thickness (`1/cos` of
    /// its slope; `√2` on a 45 degree cut).
    pub stretch: f32,
}

/// The cut a shape `w × h` can take: at most a third of its shorter side.
pub fn clamp_cut(cut: f32, w: f32, h: f32) -> f32 {
    cut.clamp(0.0, (w.min(h) / 3.0).max(0.0))
}

/// The rows of both corner bands of a shape `h` tall with `corner`s, at
/// `scale` device px per logical px: the top band from the top down, then
/// the bottom band. Rows are one device px tall (coarser past
/// [`MAX_ROWS`]) and tile each band without gaps or overlaps.
pub fn shape_rows(h: f32, corner: Corner, scale: f32) -> impl Iterator<Item = Row> {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let corner = corner.clamped(h / 2.0);
    let reach = corner.extent();
    let n = ((reach * scale).round() as u32).clamp(if reach > 0.0 { 1 } else { 0 }, MAX_ROWS);
    let step = if n > 0 { reach / n as f32 } else { 0.0 };
    (0..2 * n).map(move |i| {
        let (k, bottom) = if i < n { (i, false) } else { (i - n, true) };
        // Distance of the row's middle from the shape's top (or bottom) edge.
        let mid = (k as f32 + 0.5) * step;
        let inset = corner.inset(mid);
        let slope = (corner.inset(mid - step / 2.0) - corner.inset(mid + step / 2.0)) / step.max(f32::EPSILON);
        let y = if bottom { h - (k as f32 + 1.0) * step } else { k as f32 * step };
        Row { y, h: step, inset, stretch: (1.0 + slope * slope).sqrt() }
    })
}

/// The rows of both cut bands of a shape `h` tall with corners cut by
/// `cut` (the Cut button).
pub fn rows(h: f32, cut: f32, scale: f32) -> impl Iterator<Item = Row> {
    shape_rows(h, Corner::Cut(cut), scale)
}

/// The straight edges of an outline `t` thick on a `w × h` shape whose
/// corners reach `reach`: `(x, y, w, h)` of the top, bottom, start and end
/// edges.
pub fn edges(w: f32, h: f32, reach: f32, t: f32) -> [(f32, f32, f32, f32); 4] {
    let (span_w, span_h) = ((w - 2.0 * reach).max(0.0), (h - 2.0 * reach).max(0.0));
    [(reach, 0.0, span_w, t), (reach, h - t, span_w, t), (0.0, reach, t, span_h), (w - t, reach, t, span_h)]
}

/// How wide a stroke `t` thick is across one row of a 45 degree edge.
pub fn diagonal_width(t: f32) -> f32 {
    t * std::f32::consts::SQRT_2
}

/// What fills a shape: its fill (a vertical gradient when `fill_end` is
/// set, top to bottom), then an optional outline `(colour, thickness)`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Ink {
    pub fill: Option<Color>,
    pub fill_end: Option<Color>,
    pub outline: Option<(Color, f32)>,
}

impl Ink {
    /// A solid fill and an optional outline.
    pub const fn solid(fill: Option<Color>, outline: Option<(Color, f32)>) -> Self {
        Self { fill, fill_end: None, outline }
    }
}

/// Paints a shape filling `bounds` with its corners cut by `cut` (clamped
/// to [`clamp_cut`]): the Cut button.
pub fn paint(window: &mut Window, bounds: Bounds<Pixels>, cut: f32, ink: Ink) {
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    paint_shape(window, bounds, Corner::Cut(clamp_cut(cut, w, h)), ink);
}

/// Paints a shape filling `bounds` with `corner`s (at most half its shorter
/// side).
pub fn paint_shape(window: &mut Window, bounds: Bounds<Pixels>, corner: Corner, ink: Ink) {
    let scale = window.scale_factor();
    let w = f32::from(bounds.size.width);
    // Height and corner on the device grid too: every row edge lands on it,
    // so neighbouring rows never overlap or leave a seam.
    let snap = |v: f32| (v * scale).round() / scale;
    let h = snap(f32::from(bounds.size.height));
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let corner = corner.clamped(snap(corner.extent().min(w / 2.0).min(h / 2.0)));
    let reach = corner.extent();
    // The bands start on the device grid, so their rows tile it exactly.
    let x0 = f32::from(bounds.origin.x);
    let y0 = (f32::from(bounds.origin.y) * scale).round() / scale;
    let at = |x: f32, y: f32, qw: f32, qh: f32| Bounds::new(point(px(x0 + x), px(y0 + y)), size(px(qw), px(qh)));
    let quad = |window: &mut Window, x: f32, y: f32, qw: f32, qh: f32, color: Color| {
        if qw > 0.0 && qh > 0.0 {
            window.paint_subpixel_quad(gpui::fill(at(x, y, qw, qh), color.hsla()));
        }
    };
    if let Some(top) = ink.fill {
        let bottom = ink.fill_end.unwrap_or(top);
        // The colour at `y` down the shape (a vertical gradient, or solid).
        let shade = |y: f32| if top == bottom { top } else { top.mix(bottom, (y / h).clamp(0.0, 1.0)) };
        let body_h = h - 2.0 * reach;
        if body_h > 0.0 {
            let b = at(0.0, reach, w, body_h);
            if top == bottom {
                window.paint_subpixel_quad(gpui::fill(b, top.hsla()));
            } else {
                window.paint_subpixel_quad(gpui::fill(b, oklab_gradient(crate::plugins::panel_fx::ANGLE_DOWN_DEG, shade(reach), shade(h - reach))));
            }
        }
        for r in shape_rows(h, corner, scale) {
            quad(window, r.inset, r.y, w - 2.0 * r.inset, r.h, shade(r.y + r.h / 2.0));
        }
    }
    if let Some((color, t)) = ink.outline {
        for (x, y, ew, eh) in edges(w, h, reach, t) {
            quad(window, x, y, ew, eh, color);
        }
        for r in shape_rows(h, corner, scale) {
            // The stroke runs from the edge inward, as far as the straight
            // edge it meets (its run grows where the curve flattens).
            let run = (t * r.stretch).min((reach - r.inset).max(t)).min(w / 2.0 - r.inset);
            quad(window, r.inset, r.y, run, r.h, color);
            quad(window, w - r.inset - run, r.y, run, r.h, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCALES: [f32; 4] = [1.0, 1.25, 1.5, 2.0];

    #[test]
    fn rows_tile_both_bands_inside_the_shape() {
        for scale in SCALES {
            for (h, reach) in [(28.0, 6.0), (34.0, 8.0), (40.0, 0.5), (12.0, 20.0), (120.0, 27.0)] {
                for corner in [Corner::Cut(reach), Corner::Squircle(reach)] {
                    let reach_c: f32 = f32::min(reach, h / 2.0);
                    let all: Vec<Row> = shape_rows(h, corner, scale).collect();
                    let n = all.len() / 2;
                    assert_eq!(all.len(), 2 * n);
                    // Top band: from 0 down to the corner's reach, rows touching.
                    let mut y = 0.0;
                    for r in &all[..n] {
                        assert!((r.y - y).abs() < 1e-4, "{scale} {h} {corner:?}: no gap");
                        y = r.y + r.h;
                        assert!(r.inset >= 0.0 && r.inset <= reach_c, "inside the corner");
                        assert!(r.stretch >= 1.0);
                    }
                    assert!((y - reach_c).abs() < 1e-3, "{scale}: the top band ends at the corner's reach");
                    // Bottom band mirrors it.
                    for (top, bottom) in all[..n].iter().zip(&all[n..]) {
                        assert!((bottom.y - (h - top.y - top.h)).abs() < 1e-3);
                        assert_eq!(bottom.inset, top.inset);
                    }
                    // Insets shrink toward the body.
                    for pair in all[..n].windows(2) {
                        assert!(pair[0].inset >= pair[1].inset);
                    }
                }
            }
        }
        // A cut's edge is a straight 45 degree line.
        let cut: Vec<Row> = rows(34.0, 8.0, 1.5).collect();
        for pair in cut[..cut.len() / 2].windows(2) {
            assert!((pair[0].inset - pair[1].inset - pair[0].h).abs() < 1e-3);
            assert!((pair[0].stretch - std::f32::consts::SQRT_2).abs() < 1e-3);
        }
        assert_eq!(rows(30.0, 0.0, 1.5).count(), 0, "no cut, no rows");
        assert!(shape_rows(400.0, Corner::Squircle(200.0), 4.0).count() <= 2 * MAX_ROWS as usize);
    }

    /// Children are clipped to a rounded rectangle that never leaves the
    /// drawn shape: the inscribed circle sits inside every corner row.
    #[test]
    fn the_inscribed_radius_stays_inside_the_corner() {
        for reach in [6.0f32, 12.0, 18.0, 27.0, 40.0] {
            for corner in [Corner::Cut(reach), Corner::Squircle(reach)] {
                let rc = corner.inscribed_radius();
                for i in 0..=400 {
                    let d = rc * i as f32 / 400.0;
                    let circle = if d >= rc { 0.0 } else { rc - (rc * rc - (rc - d) * (rc - d)).max(0.0).sqrt() };
                    assert!(circle + 1e-3 >= corner.inset(d), "{corner:?}: the clip at {d} is inside ({circle} ≥ {})", corner.inset(d));
                }
            }
        }
        // A squircle meets its edges at its reach and bulges past the circle.
        let sq = Corner::Squircle(20.0);
        assert!((sq.inset(0.0) - 20.0).abs() < 1e-3 && sq.inset(20.0) < 1e-3);
        assert!(sq.inset(10.0) < 20.0 - (20.0f32 * 20.0 - 10.0 * 10.0).sqrt(), "squarer than a circle");
    }

    #[test]
    fn the_outline_edges_stay_inside_and_the_cut_is_clamped() {
        let (w, h, t) = (90.0, 30.0, 1.0);
        let cut = clamp_cut(16.0, w, h);
        assert_eq!(cut, 10.0, "at most a third of the shorter side");
        for (x, y, ew, eh) in edges(w, h, cut, t) {
            assert!(x >= 0.0 && y >= 0.0 && x + ew <= w + 1e-4 && y + eh <= h + 1e-4);
        }
        assert!(diagonal_width(1.0) > 1.0 && diagonal_width(1.0) < 1.5);
        assert_eq!(clamp_cut(-3.0, w, h), 0.0);
    }
}
