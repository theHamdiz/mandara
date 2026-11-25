//! Edit-mode geometry: dragging, snapping, scaling and re-anchoring a
//! widget, in logical px of its output. Pure functions (tested without a
//! compositor); [`super`] turns their results into config edits.
//!
//! A widget has two boxes: its *bounds* (what is painted, the outline) and
//! its *outer* box (what [`Placement::origin_in`] positions; they differ by
//! the effect margin and anti-aliasing). A drag moves the bounds with the
//! pointer, snaps them to the screen edges and centre, keeps the outer box
//! inside the safe area exactly as the renderer does, and derives the new
//! offsets from the outer box, so what is drawn is where the outline is.

use mandara_config::desktop_widgets::{Anchor, Insets, Placement, ranges};

/// `[x, y, w, h]`.
pub type Box4 = [f32; 4];

/// Within this many px of an edge or a centre line, a drag snaps to it.
pub const SNAP_PX: f32 = 8.0;

/// A snap guide: a vertical line at `x` or a horizontal one at `y`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Guide {
    Vertical(f32),
    Horizontal(f32),
}

/// The anchor nearest a point at fractions `(fx, fy)` of the display
/// (thirds: left / centre / right, top / middle / bottom).
pub fn nearest_anchor(fx: f32, fy: f32) -> Anchor {
    let pick = |f: f32| if f < 1.0 / 3.0 { 0 } else if f > 2.0 / 3.0 { 2 } else { 1 };
    Anchor::ALL[pick(fy) * 3 + pick(fx)]
}

/// `a` mirrored left ↔ right (cards follow the reading direction).
pub fn mirror(a: Anchor) -> Anchor {
    let i = Anchor::ALL.iter().position(|x| *x == a).unwrap_or(4);
    Anchor::ALL[(i / 3) * 3 + (2 - i % 3)]
}

/// Start of a `len` long span placed at `at`, moved inside `lo..hi`
/// (centred when it can't fit) — the renderer's clamp.
pub fn fit(at: f32, len: f32, lo: f32, hi: f32) -> f32 {
    if len > hi - lo { lo + (hi - lo - len) / 2.0 } else { at.clamp(lo, hi - len) }
}

/// The shift that snaps the span `a..a+len` to the nearest of `lines`
/// (its start to `lo`, its end to `hi`, its middle to `mid`), and the
/// guide line, when one is within [`SNAP_PX`].
pub fn snap_axis(a: f32, len: f32, lo: f32, hi: f32, mid: f32) -> (f32, Option<f32>) {
    let candidates = [(lo - a, lo), (hi - (a + len), hi), (mid - (a + len / 2.0), mid)];
    candidates.into_iter().filter(|(d, _)| d.abs() <= SNAP_PX).min_by(|x, y| x.0.abs().total_cmp(&y.0.abs())).map_or((0.0, None), |(d, line)| (d, Some(line)))
}

/// The area a widget lives in: the display less what it keeps clear of
/// (`[x0, y0, x1, y1]`), and the lines it snaps to.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Area {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    /// The display's centre (snapping), not the area's.
    pub mid: (f32, f32),
}

impl Area {
    pub fn of(size: (f32, f32), safe: Insets) -> Self {
        Self { x0: safe.left, y0: safe.top, x1: size.0 - safe.right, y1: size.1 - safe.bottom, mid: (size.0 / 2.0, size.1 / 2.0) }
    }

    pub fn rect(origin: (f32, f32), size: (f32, f32), mid: (f32, f32)) -> Self {
        Self { x0: origin.0, y0: origin.1, x1: origin.0 + size.0, y1: origin.1 + size.1, mid }
    }

    pub fn size(&self) -> (f32, f32) {
        (self.x1 - self.x0, self.y1 - self.y0)
    }
}

/// The result of a drag step: where the outline goes, where the outer box
/// goes, and the guides to show.
#[derive(Clone, Debug, PartialEq)]
pub struct Moved {
    pub bounds: Box4,
    pub outer: Box4,
    pub guides: Vec<Guide>,
}

/// Moves a widget whose bounds were `bounds` and outer box `outer` by the
/// pointer delta `d`: snapped (unless `free`) and clamped so the outer box
/// stays in `area`.
pub fn drag(bounds: Box4, outer: Box4, d: (f32, f32), area: &Area, free: bool) -> Moved {
    let (mut dx, mut dy) = d;
    let mut guides = Vec::new();
    if !free {
        let (sx, gx) = snap_axis(bounds[0] + dx, bounds[2], area.x0, area.x1, area.mid.0);
        let (sy, gy) = snap_axis(bounds[1] + dy, bounds[3], area.y0, area.y1, area.mid.1);
        dx += sx;
        dy += sy;
        guides.extend(gx.map(Guide::Vertical));
        guides.extend(gy.map(Guide::Horizontal));
    }
    let ox = fit(outer[0] + dx, outer[2], area.x0, area.x1);
    let oy = fit(outer[1] + dy, outer[3], area.y0, area.y1);
    let (dx, dy) = (ox - outer[0], oy - outer[1]);
    // A clamped axis no longer sits on its guide.
    guides.retain(|g| match g {
        Guide::Vertical(x) => (bounds[0] + dx - x).abs() < 0.5 || (bounds[0] + bounds[2] + dx - x).abs() < 0.5 || (bounds[0] + bounds[2] / 2.0 + dx - x).abs() < 0.5,
        Guide::Horizontal(y) => (bounds[1] + dy - y).abs() < 0.5 || (bounds[1] + bounds[3] + dy - y).abs() < 0.5 || (bounds[1] + bounds[3] / 2.0 + dy - y).abs() < 0.5,
    });
    Moved { bounds: [bounds[0] + dx, bounds[1] + dy, bounds[2], bounds[3]], outer: [ox, oy, outer[2], outer[3]], guides }
}

/// The offsets that put an `outer` box where it is on a `size` display
/// for `place`'s anchor.
pub fn offsets(place: &Placement, size: (f32, f32), outer: Box4) -> (f32, f32) {
    place.offset_for(size, (outer[2], outer[3]), (outer[0], outer[1]))
}

/// `place` re-anchored to the anchor nearest the outer box's centre, with
/// the offsets that keep the box where it is.
pub fn reanchor(place: &Placement, size: (f32, f32), outer: Box4) -> Placement {
    let (cx, cy) = ((outer[0] + outer[2] / 2.0) / size.0.max(1.0), (outer[1] + outer[3] / 2.0) / size.1.max(1.0));
    let mut p = place.clone();
    p.anchor = nearest_anchor(cx, cy);
    (p.x, p.y) = offsets(&p, size, outer);
    p
}

/// The scale a corner drag gives: the start scale times how much farther
/// the pointer is from the `fixed` point than where the drag started,
/// clamped to the allowed range. Returns the scale and the factor applied.
pub fn scale_by(start_scale: f32, fixed: (f32, f32), from: (f32, f32), to: (f32, f32)) -> (f32, f32) {
    let dist = |p: (f32, f32)| ((p.0 - fixed.0).powi(2) + (p.1 - fixed.1).powi(2)).sqrt();
    let f = dist(to) / dist(from).max(1.0);
    let s = (start_scale * f).clamp(ranges::SCALE.0, ranges::SCALE.1);
    (s, s / start_scale.max(f32::EPSILON))
}

/// `b` scaled by `f` about the point `fixed`.
pub fn scaled(b: Box4, fixed: (f32, f32), f: f32) -> Box4 {
    [fixed.0 + (b[0] - fixed.0) * f, fixed.1 + (b[1] - fixed.1) * f, b[2] * f, b[3] * f]
}

/// Where a new widget goes for a click at `at` on a `size` display: the
/// nearest anchor, with the offsets that pin that anchor point of the
/// widget's box to the click.
pub fn place_at(at: (f32, f32), size: (f32, f32)) -> (Anchor, f32, f32) {
    let (w, h) = (size.0.max(1.0), size.1.max(1.0));
    let a = nearest_anchor(at.0 / w, at.1 / h);
    let (fx, fy) = a.fraction();
    (a, (at.0 - fx * w) / w, (at.1 - fy * h) / h)
}

/// The card-local form of `p` (cards mirror left ↔ right in right-to-left
/// layouts, see `mandara_config::desktop_cards::card_origin`): its anchor
/// and x as they are on screen.
pub fn card_screen_place(p: &Placement, rtl: bool) -> Placement {
    let mut q = p.clone();
    if rtl {
        q.anchor = mirror(q.anchor);
        q.x = -q.x;
    }
    q
}

/// Back from [`card_screen_place`] (the mirror is its own inverse).
pub fn card_stored_place(p: &Placement, rtl: bool) -> Placement {
    card_screen_place(p, rtl)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Area {
        Area::of((1000.0, 600.0), Insets { top: 40.0, right: 10.0, bottom: 10.0, left: 10.0 })
    }

    #[test]
    fn anchors_by_thirds_and_mirror() {
        assert_eq!(nearest_anchor(0.1, 0.1), Anchor::TopLeft);
        assert_eq!(nearest_anchor(0.5, 0.5), Anchor::Center);
        assert_eq!(nearest_anchor(0.9, 0.8), Anchor::BottomRight);
        assert_eq!(mirror(Anchor::TopLeft), Anchor::TopRight);
        assert_eq!(mirror(Anchor::Bottom), Anchor::Bottom);
        assert_eq!(mirror(mirror(Anchor::Left)), Anchor::Left);
    }

    #[test]
    fn drags_snap_to_edges_and_centres() {
        let b = [100.0, 100.0, 200.0, 100.0];
        // The left edge lands 5 px from the safe edge: snapped onto it.
        let m = drag(b, b, (-85.0, 0.0), &area(), false);
        assert_eq!(m.bounds[0], 10.0);
        assert!(m.guides.contains(&Guide::Vertical(10.0)));
        // The centre lands 4 px from the display's centre line.
        let m = drag(b, b, (296.0, 0.0), &area(), false);
        assert_eq!(m.bounds[0] + m.bounds[2] / 2.0, 500.0);
        assert!(m.guides.contains(&Guide::Vertical(500.0)));
        // Free drags don't snap.
        let m = drag(b, b, (296.0, 0.0), &area(), true);
        assert_eq!(m.bounds[0], 396.0);
        assert!(m.guides.is_empty());
    }

    #[test]
    fn drags_keep_the_outer_box_in_the_safe_area() {
        // The outer box is 6 px bigger on every side than the bounds.
        let b = [100.0, 100.0, 200.0, 100.0];
        let o = [94.0, 94.0, 212.0, 112.0];
        let m = drag(b, o, (0.0, -500.0), &area(), true);
        assert_eq!(m.outer[1], 40.0, "clear of the bar");
        assert_eq!(m.bounds[1], 46.0, "the outline moves with it");
        let m = drag(b, o, (2000.0, 0.0), &area(), false);
        assert_eq!(m.outer[0] + m.outer[2], 990.0);
    }

    #[test]
    fn offsets_round_trip_through_the_placement() {
        let size = (1000.0, 600.0);
        let outer = [700.0, 380.0, 200.0, 100.0];
        let p = reanchor(&Placement::default(), size, outer);
        assert_eq!(p.anchor, Anchor::BottomRight);
        let (x, y) = p.origin(size, (200.0, 100.0));
        assert!((x - 700.0).abs() < 1e-3 && (y - 380.0).abs() < 1e-3, "{x} {y}");
    }

    #[test]
    fn corner_drags_scale_about_the_fixed_corner() {
        let (s, f) = scale_by(1.0, (0.0, 0.0), (100.0, 0.0), (150.0, 0.0));
        assert!((s - 1.5).abs() < 1e-5 && (f - 1.5).abs() < 1e-5);
        let (s, _) = scale_by(1.0, (0.0, 0.0), (100.0, 0.0), (100_000.0, 0.0));
        assert_eq!(s, ranges::SCALE.1);
        assert_eq!(scaled([10.0, 10.0, 20.0, 10.0], (10.0, 10.0), 2.0), [10.0, 10.0, 40.0, 20.0]);
    }

    #[test]
    fn new_widgets_pin_their_anchor_point_to_the_click() {
        let size = (1000.0, 600.0);
        let (a, x, y) = place_at((900.0, 100.0), size);
        assert_eq!(a, Anchor::TopRight);
        let p = Placement { anchor: a, x, y, ..Placement::default() };
        let (left, top) = p.origin(size, (100.0, 50.0));
        assert!((left + 100.0 - 900.0).abs() < 1e-3 && (top - 100.0).abs() < 1e-3);
        let (a, ..) = place_at((500.0, 300.0), size);
        assert_eq!(a, Anchor::Center);
    }

    #[test]
    fn cards_mirror_in_right_to_left_layouts() {
        let p = Placement { anchor: Anchor::BottomRight, x: -0.02, ..Placement::default() };
        let s = card_screen_place(&p, true);
        assert_eq!((s.anchor, s.x), (Anchor::BottomLeft, 0.02));
        assert_eq!(card_stored_place(&s, true), p);
        assert_eq!(card_screen_place(&p, false), p);
    }
}
