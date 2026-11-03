//! Pure geometry of notification popups joined to the bar
//! (`notifications.joined`): the line the popup drawer hangs from, how far
//! it keeps from the screen sides and how wide it may grow. Kept free of
//! GPUI state (like [`crate::launcher_place`]) so every bar style and edge
//! is unit-tested.
//!
//! * **Edge frame and hairline** (flush): the drawer pours out of the bar's
//!   far edge with concave flares, like a panel, clear of the bar's screen
//!   corner flares (and of a side bar's or pinned sidebar's fillet there).
//! * **Floating frame**: out of the slab's lower edge, on its straight part
//!   (clear of its rounded ends).
//! * **Notch**: out of the notch, on its straight part, whichever column
//!   the popups use; the notch spans most of the edge, so popups at a
//!   corner hang from its end rather than collide with its shoulder.
//! * **Glass pills, island, micro pill** on the popups' edge: there is no
//!   edge to hang from (a pill is narrower than a popup and its row covers
//!   the screen edge), so popups stay floating cards ([`solve`] is `None`).
//! * **The other edge** (the bar elsewhere, a side bar, no bar on this
//!   output): out of the bare screen edge with flares, or out of the screen
//!   frame's strip beside a side bar.
//! * **A bottom dock** holds the bottom edge: bottom popups stay cards.
//!
//! While the bar auto-hides the line follows it, so the drawer slides with
//! the bar and hangs from the screen edge once it is gone.
//!
//! Everything is top-oriented, in surface coordinates measured from the
//! screen edge the popups sit on (the surface is pulled over the bar's
//! reserved band); [`body_origin`] mirrors bottom drawers.

use mandara_config::BarPosition;
use mandara_config::alerts::ToastPosition;

use crate::bar::layout::Metrics;

/// Extra room keeping the drawer's flares clear of the bar's own corner
/// flares (as panels do).
pub const FLARE_CLEARANCE: f32 = 2.0;

/// Everything a joined placement depends on.
#[derive(Copy, Clone, Debug)]
pub struct Inputs {
    pub position: ToastPosition,
    /// The bar's geometry, and whether the bar shows on the popups' output.
    pub bar: Metrics,
    pub bar_here: bool,
    /// How far the bar has slid toward its edge (auto-hide), px.
    pub bar_slid: f32,
    /// The output's logical width.
    pub screen_w: f32,
    /// Closest the drawer comes to a screen side, px.
    pub min_margin: f32,
    /// A dock holds the bottom edge.
    pub dock_bottom: bool,
    /// Fillet radius where a side bar or a pinned sidebar meets the
    /// popups' edge, `(left, right)` (0: nothing there).
    pub side_fillets: (f32, f32),
}

/// Where the popup drawer hangs.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Drawer {
    /// Distance from the screen edge to the line it hangs from.
    pub line: f32,
    /// The line is the bar's own far edge (not a bare screen edge).
    pub on_bar: bool,
    /// Distance from the surface side the popups sit at (both sides for
    /// centred popups) to the drawer.
    pub margin: f32,
    /// Concave flare radius where it meets the line.
    pub flare: f32,
    /// Widest the drawer may be.
    pub max_width: f32,
}

/// Solves the joined placement; `None`: nothing to join, the popups stay
/// floating cards.
pub fn solve(i: &Inputs) -> Option<Drawer> {
    let m = &i.bar;
    let bottom = i.position.is_bottom();
    if bottom && i.dock_bottom {
        return None;
    }
    let edge = if bottom { BarPosition::Bottom } else { BarPosition::Top };
    let on_bar = i.bar_here && m.position == edge;
    if on_bar && !m.attached() {
        return None;
    }
    let flare = m.flare.max(0.0);
    let column = i.position.column();
    let (fillet_l, fillet_r) = i.side_fillets;
    // The corner piece on the popups' side (both, centred): what the
    // drawer's own flare must stay clear of.
    let side = |left: f32, right: f32| match column {
        -1 => left,
        1 => right,
        _ => left.max(right),
    };
    let room = |corner: f32| (corner + flare + FLARE_CLEARANCE).max(i.min_margin);
    let (line, margin) = if on_bar {
        let line = (m.bar_far_edge() - i.bar_slid.max(0.0)).max(0.0);
        let corner = if m.notch() {
            // The notch's straight edge starts past its rounded end.
            f32::from(m.frame(i.screen_w).origin.x) + m.frame_radius()
        } else if m.floating_frame {
            m.side_gap + m.frame_radius()
        } else {
            side(m.flare.max(fillet_l), m.flare.max(fillet_r))
        };
        (line, room(corner))
    } else if i.bar_here && m.vertical {
        // Beside a side bar: its flare (or the frame opening's corner) on
        // its side, the frame's strip plus that corner on the other.
        let frame = m.screen_frame.max(0.0);
        let far = if frame > 0.0 { frame + m.flare } else { 0.0 };
        let (l, r) = if m.position == BarPosition::Left { (m.flare, far) } else { (far, m.flare) };
        (frame, room(side(l.max(fillet_l), r.max(fillet_r))))
    } else {
        (0.0, room(side(fillet_l, fillet_r)))
    };
    let max_width = (i.screen_w - margin * 2.0).max(0.0);
    Some(Drawer { line, on_bar, margin, flare, max_width })
}

/// Where a drawer `w × h` starts in a surface `surface` big, for popups
/// in `column` (`-1` left, `0` centre, `1` right), on the bottom edge when
/// `bottom` (mirrored).
pub fn body_origin(d: &Drawer, column: i8, bottom: bool, (sw, sh): (f32, f32), (w, h): (f32, f32)) -> (f32, f32) {
    let x = match column {
        -1 => d.margin,
        1 => sw - d.margin - w,
        _ => (sw - w) / 2.0,
    };
    let y = if bottom { sh - d.line - h } else { d.line };
    (x, y)
}

/// Width of the popup surface that holds a drawer `w` wide: the drawer
/// with its margin on both sides (centred popups use both).
pub fn surface_width(d: &Drawer, w: f32) -> f32 {
    w.min(d.max_width) + d.margin * 2.0
}

/// How far the stack reaches from the line: the furthest popup end, each
/// popup `(offset, height, presence)` counting only as much as it is
/// present, plus the padding (which shrinks away with the last popup).
pub fn extent(items: impl IntoIterator<Item = (f32, f32, f32)>, pad: f32) -> f32 {
    let (mut reach, mut presence) = (0.0_f32, 0.0_f32);
    for (offset, height, p) in items {
        let p = p.clamp(0.0, 1.0);
        reach = reach.max(offset.max(0.0) + height.max(0.0) * p);
        presence = presence.max(p);
    }
    reach + pad * 2.0 * presence
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::{BarLayout, ShellConfig};

    const SCREEN_W: f32 = 1600.0;
    const MIN: f32 = 14.0;

    fn metrics(layout: BarLayout, position: BarPosition, floating: bool) -> Metrics {
        let mut cfg = ShellConfig::default();
        cfg.bar.layout = layout;
        cfg.bar.position = position;
        cfg.bar.floating_frame = floating;
        Metrics::of(&cfg, 1.0)
    }

    fn inputs(position: ToastPosition, bar: Metrics) -> Inputs {
        Inputs { position, bar, bar_here: true, bar_slid: 0.0, screen_w: SCREEN_W, min_margin: MIN, dock_bottom: false, side_fillets: (0.0, 0.0) }
    }

    #[test]
    fn hangs_from_the_edge_frame_clear_of_its_corner_flares() {
        let m = metrics(BarLayout::MinimalEdge, BarPosition::Top, false);
        let d = solve(&inputs(ToastPosition::TopRight, m)).unwrap();
        assert!(d.on_bar);
        assert_eq!(d.line, m.height);
        assert_eq!(d.flare, m.flare);
        // Its flare ends past the bar's screen-corner flare.
        assert!(d.margin - d.flare >= m.flare + FLARE_CLEARANCE - 1e-4);
        // At the right: the flare stays inside the surface.
        let w = 380.0;
        let sw = surface_width(&d, w);
        let (x, y) = body_origin(&d, 1, false, (sw, 600.0), (w, 100.0));
        assert!(x - d.flare >= 0.0 && x + w + d.flare <= sw);
        assert_eq!(y, m.height);
        assert_eq!(x + w, sw - d.margin);
    }

    #[test]
    fn a_bottom_bar_mirrors_the_drawer() {
        let m = metrics(BarLayout::MinimalEdge, BarPosition::Bottom, false);
        let d = solve(&inputs(ToastPosition::BottomLeft, m)).unwrap();
        assert!(d.on_bar);
        let (x, y) = body_origin(&d, -1, true, (500.0, 700.0), (380.0, 120.0));
        assert_eq!(x, d.margin);
        assert_eq!(y, 700.0 - m.height - 120.0, "rests on the bar's top edge");
    }

    #[test]
    fn popups_on_the_other_edge_hang_from_the_bare_screen_edge() {
        let m = metrics(BarLayout::MinimalEdge, BarPosition::Top, false);
        let d = solve(&inputs(ToastPosition::BottomRight, m)).unwrap();
        assert!(!d.on_bar);
        assert_eq!(d.line, 0.0);
        assert_eq!(d.margin, (m.flare + FLARE_CLEARANCE).max(MIN));
        // No bar on this output: the same.
        let d = solve(&Inputs { bar_here: false, ..inputs(ToastPosition::TopRight, m) }).unwrap();
        assert!(!d.on_bar && d.line == 0.0);
    }

    #[test]
    fn floating_pills_leave_nothing_to_join_on_their_edge() {
        for layout in [BarLayout::FloatingPills, BarLayout::ContiguousIsland, BarLayout::Micro] {
            let m = metrics(layout, BarPosition::Top, false);
            assert!(solve(&inputs(ToastPosition::TopRight, m)).is_none(), "{layout:?}");
            // The other edge is bare: popups hang from it.
            assert!(solve(&inputs(ToastPosition::BottomCenter, m)).is_some(), "{layout:?}");
        }
    }

    #[test]
    fn a_bottom_dock_keeps_bottom_popups_as_cards() {
        let m = metrics(BarLayout::MinimalEdge, BarPosition::Top, false);
        let i = Inputs { dock_bottom: true, ..inputs(ToastPosition::BottomRight, m) };
        assert!(solve(&i).is_none());
        assert!(solve(&Inputs { position: ToastPosition::TopRight, ..i }).is_some());
    }

    #[test]
    fn the_floating_frame_keeps_the_drawer_on_its_straight_edge() {
        let m = metrics(BarLayout::MinimalEdge, BarPosition::Top, true);
        assert!(m.floating_frame);
        let d = solve(&inputs(ToastPosition::TopLeft, m)).unwrap();
        assert_eq!(d.line, m.edge_gap + m.height);
        assert!(d.margin - d.flare >= m.side_gap + m.frame_radius());
    }

    #[test]
    fn the_notch_holds_corner_popups_on_its_straight_edge() {
        let m = metrics(BarLayout::Notch, BarPosition::Top, false);
        let frame = m.frame(SCREEN_W);
        let (nx, nw) = (f32::from(frame.origin.x), f32::from(frame.size.width));
        for (position, column) in [(ToastPosition::TopLeft, -1), (ToastPosition::TopCenter, 0), (ToastPosition::TopRight, 1)] {
            let d = solve(&inputs(position, m)).unwrap();
            assert!(d.on_bar && d.line == m.height);
            let w = 380.0_f32.min(d.max_width);
            // A surface as wide as the screen (as for a centred one).
            let (x, _) = body_origin(&d, column, false, (SCREEN_W, 600.0), (w, 100.0));
            assert!(x - d.flare >= nx + m.frame_radius() - 1e-3, "{position:?}");
            assert!(x + w + d.flare <= nx + nw - m.frame_radius() + 1e-3, "{position:?}");
        }
    }

    #[test]
    fn auto_hide_carries_the_line_with_the_bar() {
        let m = metrics(BarLayout::MinimalEdge, BarPosition::Top, false);
        let half = solve(&Inputs { bar_slid: m.height / 2.0, ..inputs(ToastPosition::TopRight, m) }).unwrap();
        assert_eq!(half.line, m.height / 2.0);
        let gone = solve(&Inputs { bar_slid: m.height + 2.0, ..inputs(ToastPosition::TopRight, m) }).unwrap();
        assert_eq!(gone.line, 0.0, "hangs from the screen edge once the bar is gone");
    }

    #[test]
    fn beside_a_side_bar_it_hangs_from_the_screen_frame() {
        let mut cfg = ShellConfig::default();
        cfg.bar.position = BarPosition::Left;
        cfg.bar.screen_frame = true;
        let m = Metrics::of(&cfg, 1.0);
        assert!(m.screen_frame > 0.0);
        let d = solve(&inputs(ToastPosition::TopRight, m)).unwrap();
        assert!(!d.on_bar);
        assert_eq!(d.line, m.screen_frame);
        // Clear of the right strip and the opening's corner flare.
        assert!(d.margin - d.flare >= m.screen_frame + m.flare);
        // On the bar's side only its flare.
        let left = solve(&inputs(ToastPosition::TopLeft, m)).unwrap();
        assert_eq!(left.margin, (m.flare * 2.0 + FLARE_CLEARANCE).max(MIN));
        // Without a frame the edge is bare.
        cfg.bar.screen_frame = false;
        let bare = solve(&inputs(ToastPosition::TopRight, Metrics::of(&cfg, 1.0))).unwrap();
        assert_eq!(bare.line, 0.0);
    }

    #[test]
    fn side_fillets_push_the_drawer_clear() {
        let m = metrics(BarLayout::MinimalEdge, BarPosition::Top, false);
        let big = m.flare + 20.0;
        let d = solve(&Inputs { side_fillets: (0.0, big), ..inputs(ToastPosition::TopRight, m) }).unwrap();
        assert_eq!(d.margin, big + m.flare + FLARE_CLEARANCE);
        let left = solve(&Inputs { side_fillets: (0.0, big), ..inputs(ToastPosition::TopLeft, m) }).unwrap();
        assert!(left.margin < d.margin, "a fillet on the other side doesn't matter");
    }

    #[test]
    fn the_stack_extent_grows_and_shrinks_with_presence() {
        let pad = 8.0;
        assert_eq!(extent([], pad), 0.0, "nothing: no drawer");
        assert_eq!(extent([(0.0, 80.0, 0.0)], pad), 0.0, "arriving: starts closed");
        assert_eq!(extent([(0.0, 80.0, 0.5)], pad), 40.0 + pad);
        assert_eq!(extent([(0.0, 80.0, 1.0), (90.0, 60.0, 1.0)], pad), 150.0 + pad * 2.0);
        // A spring overshoot never grows it past full.
        assert_eq!(extent([(0.0, 80.0, 1.2)], pad), 80.0 + pad * 2.0);
    }
}
