//! A launcher and an attached Notch share one contour at their meeting ends.

use gpui::{AnyElement, Hsla, IntoElement, ParentElement as _, Styled, div, px};
use mandara_config::workflow::LauncherEdge;
use mandara_core::{
    DrawerEnd, Edge, FlareCenter, concave_flare, edge_drawer_ends, snap, snap_rect,
};

use super::Rect;

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct Notch {
    pub span: (f32, f32),
    pub radius: f32,
}

/// Fixed-size paint geometry; the bar's own rounded-corner cutouts are filled
/// only where the animated launcher touches them. No path or scratch allocation.
pub(crate) struct Drawer {
    ends: [DrawerEnd; 2],
    corners: [Option<Corner>; 2],
}

#[derive(Copy, Clone, Debug, PartialEq)]
struct Corner {
    square: Rect,
    clip: Rect,
    center: FlareCenter,
}

impl Drawer {
    /// Keep ordinary drawers unchanged; a Notch supplies the finite span into
    /// which its launcher's concave shoulders can merge.
    pub(super) fn new(
        current: Rect,
        edge: LauncherEdge,
        line: f32,
        radius: f32,
        flare: f32,
        notch: Option<Notch>,
        scale: f32,
    ) -> Self {
        let mut shape = Self {
            ends: [DrawerEnd::flared(radius, flare); 2],
            corners: [None; 2],
        };
        let Some(notch) = notch else { return shape };
        let (x, y, w, h) = snap_rect(current.x, current.y, current.w, current.h, scale);
        let line = snap(line, scale);
        let touches = match edge {
            LauncherEdge::Top => y <= line && y + h > line,
            LauncherEdge::Bottom => y < line && y + h >= line,
        };
        if !touches || w <= 0.0 {
            return shape;
        }
        let start = snap(notch.span.0, scale);
        let end = snap(notch.span.0 + notch.span.1, scale);
        let r = snap(notch.radius, scale).clamp(0.0, (end - start).max(0.0) / 2.0);
        shape.ends[0].flare = flare.min((x - start - r).max(0.0));
        shape.ends[1].flare = flare.min((end - r - x - w).max(0.0));
        let (cy, centers) = match edge {
            LauncherEdge::Top => (line - r, [FlareCenter::TopRight, FlareCenter::TopLeft]),
            LauncherEdge::Bottom => (line, [FlareCenter::BottomRight, FlareCenter::BottomLeft]),
        };
        for (index, (cx, center)) in [start, end - r].into_iter().zip(centers).enumerate() {
            let left = cx.max(x);
            let right = (cx + r).min(x + w);
            if right > left && r > 0.0 {
                shape.corners[index] = Some(Corner {
                    square: Rect::new(cx, cy, r, r),
                    clip: Rect::new(left, cy, if x <= cx && x + w >= cx + r { r } else { right - left }, r),
                    center,
                });
            }
        }
        shape
    }

    /// Reuse the drawer's SDF ends and the complementary SDF arcs of the bar.
    /// Clipping matters while a button morph covers only part of a bar corner.
    pub(crate) fn element(self, edge: Edge, rect: Rect, color: Hsla, scale: f32) -> AnyElement {
        let [start, end] = self.ends;
        div()
            .absolute()
            .size_full()
            .child(edge_drawer_ends(
                edge, rect.x, rect.y, rect.w, rect.h, start, end, color, scale,
            ))
            .children(
                self.corners
                    .into_iter()
                    .flatten()
                    .map(|corner| corner.element(color)),
            )
            .into_any_element()
    }
}

impl Corner {
    /// Paint only the missing part of the rounded bar, using its exact circle.
    fn element(self, color: Hsla) -> AnyElement {
        let Self {
            square,
            clip,
            center,
        } = self;
        if square == clip {
            return concave_flare(square.x, square.y, square.w, center, color);
        }
        div()
            .absolute()
            .left(px(clip.x))
            .top(px(clip.y))
            .w(px(clip.w))
            .h(px(clip.h))
            .overflow_hidden()
            .child(concave_flare(
                square.x - clip.x,
                square.y - clip.y,
                square.w,
                center,
                color,
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// At either output edge, the aligned side has one uninterrupted contour;
    /// the inner side retains its normal shoulder and free-corner radius.
    #[test]
    fn aligned_notch_ends_merge_without_an_outward_launcher_shoulder() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for edge in [LauncherEdge::Top, LauncherEdge::Bottom] {
                let line = if edge == LauncherEdge::Top {
                    36.0
                } else {
                    1404.0
                };
                for right in [false, true] {
                    let x = if right { 1080.0 } else { 880.0 };
                    let y = if edge == LauncherEdge::Top {
                        line
                    } else {
                        line - 480.0
                    };
                    let rect = Rect::new(x, y, 600.0, 480.0);
                    let shape = Drawer::new(
                        rect,
                        edge,
                        line,
                        24.0,
                        18.0,
                        Some(Notch {
                            span: (880.0, 800.0),
                            radius: 18.0,
                        }),
                        scale,
                    );
                    let outer = usize::from(right);
                    assert_eq!(shape.ends[outer], DrawerEnd::flared(24.0, 0.0));
                    assert_eq!(shape.ends[1 - outer], DrawerEnd::flared(24.0, 18.0));
                    let corner = shape.corners[outer].expect("the bar's cutout must also be filled, not just its launcher's flare removed");
                    assert_eq!(corner.square, corner.clip);
                    let bar_radius = (18.0 * scale).round() / scale;
                    assert_eq!(corner.clip.x, if right { 1680.0 - bar_radius } else { 880.0 });
                    assert_eq!(
                        corner.clip.y,
                        if edge == LauncherEdge::Top {
                            line - bar_radius
                        } else {
                            line
                        }
                    );
                    assert!(shape.corners[1 - outer].is_none());
                    // The arc is the exact complement of the bar's rounded
                    // corner: throughout the shared square there is no hole.
                    for u in [0.25_f32, 4.0, 9.0, 17.75] {
                        for v in [0.25_f32, 4.0, 9.0, 17.75] {
                            let (cx, cy) = match corner.center {
                                FlareCenter::TopLeft => (0.0, 0.0),
                                FlareCenter::TopRight => (18.0, 0.0),
                                FlareCenter::BottomLeft => (0.0, 18.0),
                                FlareCenter::BottomRight => (18.0, 18.0),
                            };
                            let bar_cx = if right { 0.0 } else { 18.0 };
                            let bar_cy = if edge == LauncherEdge::Top { 0.0 } else { 18.0 };
                            let in_bar = (u - bar_cx).hypot(v - bar_cy) <= 18.0;
                            let in_patch = (u - cx).hypot(v - cy) >= 18.0;
                            assert!(
                                in_bar || in_patch,
                                "unfilled bar/launcher seam at {u},{v}: {edge:?}/{right}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// A moving button can reveal part of a corner, but never paint beside
    /// the current launcher; closed or separated drawers cannot alter the bar.
    #[test]
    fn morph_corner_repair_is_clipped_and_absent_when_closed_or_detached() {
        let notch = Some(Notch {
            span: (880.0, 800.0),
            radius: 18.0,
        });
        let rect = Rect::new(889.0, 36.0, 600.0, 40.0);
        let shape = Drawer::new(rect, LauncherEdge::Top, 36.0, 24.0, 18.0, notch, 1.5);
        let corner = shape.corners[0].unwrap();
        assert_eq!(corner.square, Rect::new(880.0, 18.0, 18.0, 18.0));
        assert_eq!(corner.clip.x, snap(rect.x, 1.5));
        assert_eq!(corner.clip.right(), 898.0);
        assert_eq!((corner.clip.y, corner.clip.h), (18.0, 18.0));
        assert_eq!(shape.ends[0].flare, 0.0);
        for rect in [Rect { h: 0.0, ..rect }, Rect { y: 40.0, ..rect }] {
            assert!(
                Drawer::new(rect, LauncherEdge::Top, 36.0, 24.0, 18.0, notch, 1.5)
                    .corners
                    .iter()
                    .all(Option::is_none)
            );
        }
        let ordinary = Drawer::new(rect, LauncherEdge::Top, 36.0, 24.0, 18.0, None, 1.5);
        assert_eq!(ordinary.ends, [DrawerEnd::flared(24.0, 18.0); 2]);
        assert!(ordinary.corners.iter().all(Option::is_none));
    }

    /// The production placement supplies the real Notch corner only on its
    /// own edge. Physical left/right anchoring survives either button order.
    #[test]
    fn placement_repairs_only_attached_notch_corners_in_both_button_orders() {
        use mandara_config::workflow::LauncherPosition;
        use mandara_config::{BarLayout, BarPosition, ShellConfig};
        for (bar, left, right) in [
            (
                BarPosition::Top,
                LauncherPosition::TopLeft,
                LauncherPosition::TopRight,
            ),
            (
                BarPosition::Bottom,
                LauncherPosition::BottomLeft,
                LauncherPosition::BottomRight,
            ),
        ] {
            for (position, outer) in [(left, 0), (right, 1)] {
                for button in [(900.0, 30.0), (1630.0, 30.0)] {
                    let mut cfg = ShellConfig::default();
                    cfg.bar.position = bar;
                    cfg.bar.layout = BarLayout::Notch;
                    cfg.launcher.look.position = position;
                    cfg.launcher.look.align_to_button = false;
                    cfg.plugins
                        .disabled
                        .push(mandara_config::workflow::DOCK_PLUGIN_ID.into());
                    let p = super::super::for_config_with_bar(
                        &cfg,
                        (2560.0, 1440.0),
                        480.0,
                        true,
                        Some((880.0, 800.0)),
                        Some(button),
                    );
                    let shape = p.drawer(p.rect, 24.0, 1.5);
                    assert_eq!(shape.ends[outer].flare, 0.0);
                    assert!(shape.corners[outer].is_some());
                    assert!(shape.ends[1 - outer].flare > 0.0);
                    for layout in [BarLayout::Micro, BarLayout::ContiguousIsland] {
                        cfg.bar.layout = layout;
                        let p = super::super::for_config_with_bar(
                            &cfg,
                            (2560.0, 1440.0),
                            480.0,
                            true,
                            Some((880.0, 800.0)),
                            Some(button),
                        );
                        assert!(
                            p.drawer(p.rect, 24.0, 1.5)
                                .corners
                                .iter()
                                .all(Option::is_none)
                        );
                    }
                }
            }
        }
    }
}
