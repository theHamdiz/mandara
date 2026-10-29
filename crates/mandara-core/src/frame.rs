//! The liquid edge frame: the bar strip, its concave screen-corner flares and
//! a dashboard panel pouring out of it.
//!
//! ```text
//!  ┌──────────────────────────────────────────────────────┐  ← screen top
//!  │                       bar strip                      │
//!  ╰╮            ╭─╮                    ╭─╮            ╭╯   ← flares
//!               │   panel (springs)   │
//!               ╰─────────────────────╯
//! ```
//!
//! Everything is drawn with GPUI's SDF quads (analytically anti-aliased, no
//! tessellation jaggies). Concave flares are an SDF border ring clipped to a
//! square. Fills are opaque, so adjoining pieces overlap without seams.

use gpui::{AnyElement, Hsla, IntoElement, ParentElement as _, Styled, div, px};

/// Shapes thinner than this (logical px) are not drawn at all.
const MIN_VISIBLE_PX: f32 = 0.5;
/// Anti-aliasing slack added to a flare's SDF ring, in px.
const FLARE_AA_SLACK_PX: f32 = 1.5;
/// Opaque pieces overlap their neighbour by this much to hide seams, in px.
const SEAM_OVERLAP_PX: f32 = 1.0;
/// Screen-corner flares never exceed this fraction of the bar width.
const MAX_SCREEN_FLARE_FRACTION: f32 = 0.25;
/// Panels narrower than this (px) are treated as closed.
const MIN_PANEL_W_PX: f32 = 1.0;

/// Snaps a logical coordinate to the physical pixel grid. Adjoining opaque
/// shapes whose shared edge falls mid-pixel each half-cover that pixel and
/// leave a faint seam (very visible at fractional scales like 1.5×); snapping
/// both sides to the same device pixel makes joins invisible.
pub fn snap(v: f32, scale: f32) -> f32 {
    if scale <= 0.0 { v } else { (v * scale).round() / scale }
}

/// Snaps a rect by its edges (so neighbours sharing an edge agree).
pub fn snap_rect(x: f32, y: f32, w: f32, h: f32, scale: f32) -> (f32, f32, f32, f32) {
    let (x0, y0) = (snap(x, scale), snap(y, scale));
    let (x1, y1) = (snap(x + w, scale), snap(y + h, scale));
    (x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

/// A dashboard panel hanging from the bar.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PanelShape {
    pub x: f32,
    pub width: f32,
    /// Height below the bar strip.
    pub height: f32,
    /// Bottom corner radius.
    pub radius: f32,
    /// Concave flare radius where the panel meets the bar.
    pub flare: f32,
}

/// Geometry of the whole frame, in surface-local logical pixels.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FrameShape {
    pub width: f32,
    pub bar_height: f32,
    /// Concave flare radius at the screen corners (0 disables).
    pub screen_flare: f32,
    pub panel: Option<PanelShape>,
}

/// Which corner of its bounding square a concave flare's arc is centered on.
/// The filled region hugs the *opposite* corner.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FlareCenter {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Screen edge a drawer is attached to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

/// A drawer attached to a screen edge (or to the bar), with rounded free
/// corners and concave flares where it meets the edge. `x, y, w, h` is the
/// drawer rect in surface coordinates; flares extend outside it along the
/// attached edge.
pub fn edge_drawer(edge: Edge, x: f32, y: f32, w: f32, h: f32, radius: f32, flare: f32, color: Hsla) -> AnyElement {
    edge_drawer_snapped(edge, x, y, w, h, radius, flare, color, 0.0)
}

/// [`edge_drawer`] with geometry snapped to the device pixel grid at `scale`.
#[allow(clippy::too_many_arguments)]
pub fn edge_drawer_snapped(edge: Edge, x: f32, y: f32, w: f32, h: f32, radius: f32, flare: f32, color: Hsla, scale: f32) -> AnyElement {
    let (x, y, w, h) = snap_rect(x, y, w, h, scale);
    let flare = snap(flare, scale);
    if w < MIN_VISIBLE_PX || h < MIN_VISIBLE_PX {
        return div().into_any_element();
    }
    let along = match edge {
        Edge::Top | Edge::Bottom => h,
        Edge::Left | Edge::Right => w,
    };
    let across = match edge {
        Edge::Top | Edge::Bottom => w,
        Edge::Left | Edge::Right => h,
    };
    let radius = radius.min(along / 2.0).min(across / 2.0).max(0.0);
    // Flares shrink as the drawer emerges so they never outgrow it.
    let f = flare.min(along).max(0.0);
    let body = div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).bg(color);
    let (body, a, b) = match edge {
        Edge::Top => (
            body.rounded_b(px(radius)),
            concave_flare(x - f, y, f, FlareCenter::BottomLeft, color),
            concave_flare(x + w, y, f, FlareCenter::BottomRight, color),
        ),
        Edge::Bottom => (
            body.rounded_t(px(radius)),
            concave_flare(x - f, y + h - f, f, FlareCenter::TopLeft, color),
            concave_flare(x + w, y + h - f, f, FlareCenter::TopRight, color),
        ),
        Edge::Left => (
            body.rounded_r(px(radius)),
            concave_flare(x, y - f, f, FlareCenter::TopRight, color),
            concave_flare(x, y + h, f, FlareCenter::BottomRight, color),
        ),
        Edge::Right => (
            body.rounded_l(px(radius)),
            concave_flare(x + w - f, y - f, f, FlareCenter::TopLeft, color),
            concave_flare(x + w - f, y + h, f, FlareCenter::BottomLeft, color),
        ),
    };
    div().absolute().top_0().left_0().size_full().child(body).child(a).child(b).into_any_element()
}

/// A concave fillet: the `r×r` square at `(x, y)` filled with `color`
/// everywhere *outside* the quarter circle of radius `r` centered on the
/// chosen bottom corner. Built as an SDF border ring clipped by the square,
/// so the arc is as smooth as any rounded rect.
pub fn concave_flare(x: f32, y: f32, r: f32, center: FlareCenter, color: Hsla) -> AnyElement {
    if r <= 0.0 {
        return div().into_any_element();
    }
    // The ring must cover from radius r out to the square's far corner
    // (r·√2), plus a pixel of slack for anti-aliasing.
    let ring = r * (std::f32::consts::SQRT_2 - 1.0) + FLARE_AA_SLACK_PX;
    let outer = r + ring;
    let (cx, cy) = match center {
        FlareCenter::TopLeft => (0.0, 0.0),
        FlareCenter::TopRight => (r, 0.0),
        FlareCenter::BottomLeft => (0.0, r),
        FlareCenter::BottomRight => (r, r),
    };
    let mut circle = div()
        .absolute()
        .left(px(cx - outer))
        .top(px(cy - outer))
        .size(px(outer * 2.0))
        .rounded_full()
        .border_color(color);
    let width: gpui::AbsoluteLength = px(ring).into();
    let widths = &mut circle.style().border_widths;
    widths.top = Some(width);
    widths.right = Some(width);
    widths.bottom = Some(width);
    widths.left = Some(width);
    div()
        .absolute()
        .left(px(x))
        .top(px(y))
        .size(px(r))
        .overflow_hidden()
        .child(circle)
        .into_any_element()
}

impl FrameShape {
    /// Clamps radii so pieces never overlap awkwardly, whatever the animation
    /// state.
    fn sanitized(&self) -> (f32, Option<PanelShape>) {
        let r = self.screen_flare.clamp(0.0, self.width * MAX_SCREEN_FLARE_FRACTION);
        let panel = self.panel.and_then(|panel| {
            if panel.height < MIN_VISIBLE_PX || panel.width < MIN_PANEL_W_PX {
                return None;
            }
            let radius = panel.radius.min(panel.height / 2.0).min(panel.width / 2.0).max(0.0);
            let flare = panel.flare.min(panel.height - radius).max(0.0);
            // Keep the panel and its flares clear of the screen flares.
            let min_x = r + flare;
            let max_x = (self.width - r - flare - panel.width).max(min_x);
            Some(PanelShape { x: panel.x.clamp(min_x, max_x), radius, flare, ..panel })
        });
        (r, panel)
    }

    /// The panel rect after clamping, if visible (for content placement and
    /// input masking).
    pub fn resolved_panel(&self) -> Option<PanelShape> {
        self.sanitized().1
    }

    /// Builds the frame as positioned SDF elements filled with `color`.
    pub fn element(&self, color: Hsla) -> AnyElement {
        let (r, panel) = self.sanitized();
        let h = self.bar_height;
        let mut root = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(div().absolute().top_0().left_0().w(px(self.width)).h(px(h)).bg(color))
            .child(concave_flare(0.0, h, r, FlareCenter::BottomRight, color))
            .child(concave_flare(self.width - r, h, r, FlareCenter::BottomLeft, color));
        if let Some(PanelShape { x, width, height, radius, flare }) = panel {
            root = root
                .child(
                    div()
                        .absolute()
                        // Overlap the bar by a pixel: opaque, so no seam.
                        .top(px(h - SEAM_OVERLAP_PX))
                        .left(px(x))
                        .w(px(width))
                        .h(px(height + SEAM_OVERLAP_PX))
                        .rounded_b(px(radius))
                        .bg(color),
                )
                .child(concave_flare(x - flare, h, flare, FlareCenter::BottomLeft, color))
                .child(concave_flare(x + width, h, flare, FlareCenter::BottomRight, color));
        }
        root.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapping_aligns_shared_edges() {
        // At 1.5×, 13.4 and 13.2 both land on device pixel 20 → 13.333.
        assert!((snap(13.4, 1.5) - snap(13.2, 1.5)).abs() < 1e-6);
        let (x, _, w, _) = snap_rect(10.1, 0.0, 20.3, 5.0, 1.5);
        assert!((x * 1.5 - (x * 1.5).round()).abs() < 1e-4);
        assert!(((x + w) * 1.5 - ((x + w) * 1.5).round()).abs() < 1e-4);
        assert_eq!(snap(7.25, 0.0), 7.25);
    }

    #[test]
    fn panel_is_clamped_clear_of_screen_flares() {
        let shape = FrameShape {
            width: 1000.0,
            bar_height: 30.0,
            screen_flare: 14.0,
            panel: Some(PanelShape { x: 0.0, width: 420.0, height: 500.0, radius: 22.0, flare: 16.0 }),
        };
        let panel = shape.resolved_panel().unwrap();
        assert_eq!(panel.x, 30.0);
        let right = FrameShape { panel: Some(PanelShape { x: 900.0, ..panel }), ..shape };
        assert_eq!(right.resolved_panel().unwrap().x, 1000.0 - 14.0 - 16.0 - 420.0);
    }

    #[test]
    fn tiny_panels_shrink_their_radii() {
        let shape = FrameShape {
            width: 1000.0,
            bar_height: 30.0,
            screen_flare: 14.0,
            panel: Some(PanelShape { x: 300.0, width: 120.0, height: 10.0, radius: 22.0, flare: 16.0 }),
        };
        let panel = shape.resolved_panel().unwrap();
        assert!(panel.radius <= 5.0 && panel.flare <= 5.0 + 1e-4);
        let closed = FrameShape { panel: Some(PanelShape { height: 0.1, ..panel }), ..shape };
        assert!(closed.resolved_panel().is_none());
    }
}
