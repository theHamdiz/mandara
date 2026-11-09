//! Region selection as a pure state machine: where a press lands on the
//! current selection (a corner or edge handle, inside it, or outside) and
//! what dragging from there does to the rectangle.
//!
//! - **Handles.** A press near a corner resizes both axes from it, near an
//!   edge one axis; the opposite sides stay put, so dragging past them
//!   mirrors the selection instead of collapsing it. The grip reaches past
//!   the drawn dot ([`GRIP_MOUSE_PX`], wider for a finger with
//!   [`GRIP_TOUCH_PX`]), but only a third of a side inwards, so a small
//!   selection can still be moved.
//! - **Move.** A press inside drags the selection, kept on the output.
//! - **New.** A press anywhere else starts a new selection. It replaces the
//!   current one only once the pointer travels [`NEW_SLOP_PX`], so a stray
//!   click keeps your selection (and its annotations).
//!
//! Everything is in logical pixels, the space GPUI reports pointer events
//! and lays out in, so a fractional output scale needs no conversion here;
//! the capture maps logical to physical pixels when it crops.

use super::Rect;

/// How far a mouse press may land from a handle or edge and still grab it,
/// logical px: past the 10 px dot, so the edge is easy to catch.
pub const GRIP_MOUSE_PX: f32 = 12.0;
/// The same reach for a finger (a 48 px target around each handle).
pub const GRIP_TOUCH_PX: f32 = 24.0;
/// A press outside replaces the selection once it has moved this far.
pub const NEW_SLOP_PX: f32 = 3.0;
/// Smallest side that still counts as a selection.
pub const MIN_SIDE_PX: f32 = 2.0;
/// The grip reaches inwards at most this share of a side.
const INNER_SHARE: f32 = 1.0 / 3.0;

/// One side of an axis: left / top (`Start`) or right / bottom (`End`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Side {
    Start,
    End,
}

/// What a press on the selection holds.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Grip {
    /// A handle: the side of each axis it drags (`None`: that axis stays).
    /// Both set is a corner, one set is an edge.
    Resize { x: Option<Side>, y: Option<Side> },
    /// Inside the selection: drag it as a whole.
    Move,
}

/// Whether `r` is big enough to be a selection.
pub fn usable(r: Rect) -> bool {
    r.w >= MIN_SIDE_PX && r.h >= MIN_SIDE_PX
}

/// The side of the span `lo..lo + len` that `p` is within `reach` of.
fn near(p: f32, lo: f32, len: f32, reach: f32) -> Option<Side> {
    let inner = reach.min(len * INNER_SHARE);
    let (from_lo, from_hi) = (p - lo, lo + len - p);
    let at_lo = (-reach..=inner).contains(&from_lo);
    let at_hi = (-reach..=inner).contains(&from_hi);
    match (at_lo, at_hi) {
        (true, true) => Some(if from_lo.abs() <= from_hi.abs() { Side::Start } else { Side::End }),
        (true, false) => Some(Side::Start),
        (false, true) => Some(Side::End),
        (false, false) => None,
    }
}

/// What a press at `p` grabs on `sel` (`None`: outside, a new selection).
pub fn hit(sel: Rect, p: (f32, f32), reach: f32) -> Option<Grip> {
    let band = |p: f32, lo: f32, len: f32| p >= lo - reach && p <= lo + len + reach;
    if !usable(sel) || !band(p.0, sel.x, sel.w) || !band(p.1, sel.y, sel.h) {
        return None;
    }
    match (near(p.0, sel.x, sel.w, reach), near(p.1, sel.y, sel.h, reach)) {
        // In the grown box but near no side: inside.
        (None, None) => Some(Grip::Move),
        (x, y) => Some(Grip::Resize { x, y }),
    }
}

/// A drag in progress.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Drag {
    /// From a press outside the selection; `live` once it moved far enough
    /// to replace `previous`.
    New { start: (f32, f32), live: bool, previous: Option<Rect> },
    /// A handle of `from`: the sides it doesn't hold stay put.
    Resize { from: Rect, x: Option<Side>, y: Option<Side> },
    /// The whole of `from`, following the pointer from `start`.
    Move { from: Rect, start: (f32, f32) },
}

/// How a drag ended.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Release {
    /// The selection to keep.
    pub selection: Option<Rect>,
    /// A new selection was drawn (not a click, not an adjustment): the old
    /// annotations go, and "capture on release" applies.
    pub fresh: bool,
}

/// One axis of a resize: the span `lo..lo + len` with `side` at `p`.
fn resize_axis(lo: f32, len: f32, side: Option<Side>, p: f32) -> (f32, f32) {
    let anchor = match side {
        None => return (lo, len),
        Some(Side::Start) => lo + len,
        Some(Side::End) => lo,
    };
    (anchor.min(p), (anchor - p).abs())
}

/// `p` kept inside `b`.
fn clamp_to(p: (f32, f32), b: Rect) -> (f32, f32) {
    (p.0.max(b.x).min(b.x + b.w), p.1.max(b.y).min(b.y + b.h))
}

/// The side `p` is on of a held axis, after mirroring past `anchor`.
fn side_now(side: Option<Side>, lo: f32, len: f32, p: f32) -> Option<Side> {
    let anchor = match side? {
        Side::Start => lo + len,
        Side::End => lo,
    };
    Some(if p < anchor { Side::Start } else { Side::End })
}

impl Drag {
    /// A press at `p`, with `selection` on screen (`None` or too small:
    /// always a new selection).
    pub fn begin(selection: Option<Rect>, p: (f32, f32), reach: f32) -> Self {
        let current = selection.filter(|r| usable(*r));
        match current.and_then(|r| hit(r, p, reach).map(|g| (r, g))) {
            Some((from, Grip::Resize { x, y })) => Drag::Resize { from, x, y },
            Some((from, Grip::Move)) => Drag::Move { from, start: p },
            None => Drag::New { start: p, live: false, previous: selection },
        }
    }

    /// The pointer is at `p` on an output of size `bounds`: the selection
    /// to show now (`None`: leave it as it is).
    pub fn update(&mut self, p: (f32, f32), bounds: Rect) -> Option<Rect> {
        match self {
            Drag::New { start, live, .. } => {
                let p = clamp_to(p, bounds);
                if !*live && (p.0 - start.0).abs().max((p.1 - start.1).abs()) < NEW_SLOP_PX {
                    return None;
                }
                *live = true;
                Some(Rect::from_points(clamp_to(*start, bounds), p))
            }
            Drag::Resize { from, x, y } => {
                let p = clamp_to(p, bounds);
                let (rx, rw) = resize_axis(from.x, from.w, *x, p.0);
                let (ry, rh) = resize_axis(from.y, from.h, *y, p.1);
                Some(Rect { x: rx, y: ry, w: rw, h: rh })
            }
            Drag::Move { from, start } => {
                // Kept whole on the output: stops at its edges.
                let shift = |lo: f32, len: f32, d: f32, b_lo: f32, b_len: f32| (lo + d).min(b_lo + b_len - len).max(b_lo);
                Some(Rect {
                    x: shift(from.x, from.w, p.0 - start.0, bounds.x, bounds.w),
                    y: shift(from.y, from.h, p.1 - start.1, bounds.y, bounds.h),
                    ..*from
                })
            }
        }
    }

    /// What the drag holds with the pointer at `p` (for the cursor): a
    /// resize flips to the other side once dragged past the opposite one.
    pub fn grip(&self, p: (f32, f32)) -> Option<Grip> {
        match self {
            Drag::New { .. } => None,
            Drag::Resize { from, x, y } => Some(Grip::Resize { x: side_now(*x, from.x, from.w, p.0), y: side_now(*y, from.y, from.h, p.1) }),
            Drag::Move { .. } => Some(Grip::Move),
        }
    }

    /// Whether a new selection is being drawn over the old one.
    pub fn drawing_new(&self) -> bool {
        matches!(self, Drag::New { live: true, .. })
    }

    /// The button came up with `current` on screen.
    pub fn release(&self, current: Option<Rect>) -> Release {
        match *self {
            // A click: nothing changed.
            Drag::New { live: false, previous, .. } => Release { selection: previous, fresh: false },
            Drag::New { live: true, .. } => Release { selection: current, fresh: true },
            // Squeezed below a usable size: back to where it was.
            Drag::Resize { from, .. } | Drag::Move { from, .. } => Release { selection: Some(current.filter(|r| usable(*r)).unwrap_or(from)), fresh: false },
        }
    }

    /// The system took the touch (or the drag was abandoned): the selection
    /// before the press.
    pub fn cancel(&self) -> Option<Rect> {
        match *self {
            Drag::New { previous, .. } => previous,
            Drag::Resize { from, .. } | Drag::Move { from, .. } => Some(from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 };
    const SEL: Rect = Rect { x: 100.0, y: 200.0, w: 400.0, h: 300.0 };

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    fn drag(from: (f32, f32), to: (f32, f32), reach: f32) -> (Drag, Option<Rect>) {
        let mut d = Drag::begin(Some(SEL), from, reach);
        let shown = d.update(to, SCREEN);
        (d, shown)
    }

    fn resize(x: Option<Side>, y: Option<Side>) -> Option<Grip> {
        Some(Grip::Resize { x, y })
    }

    #[test]
    fn every_handle_is_hit() {
        use Side::{End, Start};
        let m = GRIP_MOUSE_PX;
        for (p, grip) in [
            ((100.0, 200.0), resize(Some(Start), Some(Start))),
            ((500.0, 200.0), resize(Some(End), Some(Start))),
            ((100.0, 500.0), resize(Some(Start), Some(End))),
            ((500.0, 500.0), resize(Some(End), Some(End))),
            ((300.0, 200.0), resize(None, Some(Start))),
            ((300.0, 500.0), resize(None, Some(End))),
            ((100.0, 350.0), resize(Some(Start), None)),
            ((500.0, 350.0), resize(Some(End), None)),
            ((300.0, 350.0), Some(Grip::Move)),
        ] {
            assert_eq!(hit(SEL, p, m), grip, "{p:?}");
        }
    }

    #[test]
    fn handles_reach_past_the_drawn_dot() {
        let m = GRIP_MOUSE_PX;
        // Outside the corner, diagonally, within the grip.
        assert_eq!(hit(SEL, (100.0 - m + 1.0, 200.0 - m + 1.0), m), resize(Some(Side::Start), Some(Side::Start)));
        // Just outside an edge, and just inside it.
        assert_eq!(hit(SEL, (300.0, 500.0 + m - 1.0), m), resize(None, Some(Side::End)));
        assert_eq!(hit(SEL, (500.0 - m + 1.0, 350.0), m), resize(Some(Side::End), None));
        // Beyond the grip: a new selection.
        assert_eq!(hit(SEL, (100.0 - m - 1.0, 350.0), m), None);
        assert_eq!(hit(SEL, (300.0, 200.0 - m - 1.0), m), None);
        assert_eq!(hit(SEL, (900.0, 900.0), m), None);
    }

    #[test]
    fn a_finger_reaches_further_than_the_mouse() {
        let p = (100.0 - GRIP_MOUSE_PX - 6.0, 350.0);
        assert_eq!(hit(SEL, p, GRIP_MOUSE_PX), None);
        assert_eq!(hit(SEL, p, GRIP_TOUCH_PX), resize(Some(Side::Start), None));
        assert!(matches!(Drag::begin(Some(SEL), p, GRIP_TOUCH_PX), Drag::Resize { x: Some(Side::Start), y: None, .. }));
    }

    #[test]
    fn a_small_selection_can_still_be_moved() {
        let small = r(100.0, 100.0, 30.0, 30.0);
        assert_eq!(hit(small, (115.0, 115.0), GRIP_TOUCH_PX), Some(Grip::Move));
        assert_eq!(hit(small, (101.0, 101.0), GRIP_TOUCH_PX), resize(Some(Side::Start), Some(Side::Start)));
        // A sliver: the nearer side wins.
        let sliver = r(100.0, 100.0, 4.0, 200.0);
        assert_eq!(hit(sliver, (105.0, 200.0), GRIP_MOUSE_PX), resize(Some(Side::End), None));
        assert_eq!(hit(sliver, (99.0, 200.0), GRIP_MOUSE_PX), resize(Some(Side::Start), None));
    }

    #[test]
    fn corners_resize_both_axes_and_keep_the_opposite_corner() {
        let (_, shown) = drag((500.0, 500.0), (600.0, 650.0), GRIP_MOUSE_PX);
        assert_eq!(shown, Some(r(100.0, 200.0, 500.0, 450.0)));
        let (_, shown) = drag((100.0, 200.0), (50.0, 250.0), GRIP_MOUSE_PX);
        assert_eq!(shown, Some(r(50.0, 250.0, 450.0, 250.0)));
    }

    #[test]
    fn edges_resize_one_axis() {
        // The right edge, dragged diagonally: only the width changes.
        let (_, shown) = drag((500.0, 350.0), (560.0, 900.0), GRIP_MOUSE_PX);
        assert_eq!(shown, Some(r(100.0, 200.0, 460.0, 300.0)));
        // The top edge.
        let (_, shown) = drag((300.0, 200.0), (0.0, 150.0), GRIP_MOUSE_PX);
        assert_eq!(shown, Some(r(100.0, 150.0, 400.0, 350.0)));
    }

    #[test]
    fn dragging_past_the_opposite_side_mirrors() {
        // Bottom-right corner dragged above and left of the top-left one.
        let (d, shown) = drag((500.0, 500.0), (40.0, 120.0), GRIP_MOUSE_PX);
        assert_eq!(shown, Some(r(40.0, 120.0, 60.0, 80.0)));
        assert_eq!(d.grip((40.0, 120.0)), resize(Some(Side::Start), Some(Side::Start)));
        // The left edge dragged past the right one.
        let (d, shown) = drag((100.0, 350.0), (700.0, 350.0), GRIP_MOUSE_PX);
        assert_eq!(shown, Some(r(500.0, 200.0, 200.0, 300.0)));
        assert_eq!(d.grip((700.0, 350.0)), resize(Some(Side::End), None));
    }

    #[test]
    fn resizing_is_clamped_to_the_output() {
        let (_, shown) = drag((500.0, 500.0), (5000.0, 5000.0), GRIP_MOUSE_PX);
        assert_eq!(shown, Some(r(100.0, 200.0, 1820.0, 880.0)));
        let (_, shown) = drag((100.0, 200.0), (-50.0, -50.0), GRIP_MOUSE_PX);
        assert_eq!(shown, Some(r(0.0, 0.0, 500.0, 500.0)));
    }

    #[test]
    fn inside_moves_and_stays_on_the_output() {
        let (d, shown) = drag((300.0, 350.0), (350.0, 300.0), GRIP_MOUSE_PX);
        assert_eq!(shown, Some(r(150.0, 150.0, 400.0, 300.0)));
        assert_eq!(d.grip((350.0, 300.0)), Some(Grip::Move));
        // Pushed into the bottom-right and top-left corners.
        let (_, shown) = drag((300.0, 350.0), (3000.0, 3000.0), GRIP_MOUSE_PX);
        assert_eq!(shown, Some(r(1520.0, 780.0, 400.0, 300.0)));
        let (_, shown) = drag((300.0, 350.0), (-3000.0, -3000.0), GRIP_MOUSE_PX);
        assert_eq!(shown, Some(r(0.0, 0.0, 400.0, 300.0)));
    }

    #[test]
    fn outside_starts_a_new_selection_only_after_moving() {
        let mut d = Drag::begin(Some(SEL), (900.0, 700.0), GRIP_MOUSE_PX);
        assert!(matches!(d, Drag::New { live: false, .. }));
        assert_eq!(d.update((901.0, 701.0), SCREEN), None, "a click-sized wobble keeps the selection");
        assert_eq!(d.release(Some(SEL)), Release { selection: Some(SEL), fresh: false });
        let shown = d.update((1000.0, 650.0), SCREEN);
        assert_eq!(shown, Some(r(900.0, 650.0, 100.0, 50.0)));
        assert!(d.drawing_new());
        assert_eq!(d.release(shown), Release { selection: shown, fresh: true });
        // Clamped to the output too.
        assert_eq!(d.update((4000.0, -10.0), SCREEN), Some(r(900.0, 0.0, 1020.0, 700.0)));
    }

    #[test]
    fn without_a_selection_every_press_is_new() {
        assert!(matches!(Drag::begin(None, (100.0, 200.0), GRIP_MOUSE_PX), Drag::New { previous: None, .. }));
        // Too small to be a selection: no handles to grab.
        let dot = r(100.0, 200.0, 1.0, 1.0);
        assert!(matches!(Drag::begin(Some(dot), (100.0, 200.0), GRIP_MOUSE_PX), Drag::New { .. }));
    }

    #[test]
    fn adjusting_never_counts_as_a_new_selection() {
        for from in [(500.0, 500.0), (300.0, 200.0), (300.0, 350.0)] {
            let (d, shown) = drag(from, (520.0, 540.0), GRIP_MOUSE_PX);
            assert!(!d.drawing_new());
            assert_eq!(d.release(shown), Release { selection: shown, fresh: false }, "{from:?}");
        }
    }

    #[test]
    fn a_resize_squeezed_flat_springs_back() {
        let (d, shown) = drag((500.0, 350.0), (100.5, 350.0), GRIP_MOUSE_PX);
        assert!(shown.is_some_and(|r| !usable(r)));
        assert_eq!(d.release(shown).selection, Some(SEL));
    }

    #[test]
    fn cancel_restores_the_selection_before_the_press() {
        let (d, _) = drag((500.0, 500.0), (700.0, 700.0), GRIP_MOUSE_PX);
        assert_eq!(d.cancel(), Some(SEL));
        let (d, _) = drag((900.0, 900.0), (1000.0, 1000.0), GRIP_MOUSE_PX);
        assert_eq!(d.cancel(), Some(SEL));
    }

    #[test]
    fn fractional_scale_positions_hit_and_resize_exactly() {
        // At 1.5x a physical pixel is 2/3 of a logical one: GPUI reports
        // positions like 100.6667. The handles still catch them and the
        // axes a handle doesn't hold keep their exact fractional values.
        let third = 1.0 / 3.0;
        let sel = r(100.0 + 2.0 * third, 200.0 + third, 400.0 + third, 300.0 + 2.0 * third);
        let corner = (sel.x + sel.w + third, sel.y + sel.h - 2.0 * third);
        assert_eq!(hit(sel, corner, GRIP_MOUSE_PX), resize(Some(Side::End), Some(Side::End)));
        let mut d = Drag::begin(Some(sel), (sel.x + sel.w, sel.y + sel.h / 2.0), GRIP_MOUSE_PX);
        let shown = d.update((sel.x + sel.w + 10.0 + third, 9999.0), SCREEN).expect("resized");
        assert_eq!((shown.x, shown.y, shown.h), (sel.x, sel.y, sel.h));
        assert!((shown.w - (sel.w + 10.0 + third)).abs() < 1e-3);
    }
}
