//! Overlay dimming around a bar that stays bright while its drawer is open.

use gpui::{Div, Hsla, ParentElement, Styled, div, px};
use mandara_config::BarPosition;
use mandara_core::{FlareCenter, concave_flare, snap, snap_rect};

use super::layout::Metrics;
use crate::launcher_place::Rect;

/// Dim the output outside the joined bar. The notch's shoulders and rounded
/// ends leave wallpaper visible, so excluding its whole band leaves bright
/// patches at the screen corners.
pub fn element(
    screen: (f32, f32),
    bar: &Metrics,
    band: Option<(f32, f32)>,
    color: Hsla,
    scale: f32,
) -> Div {
    let surround = NotchSurround::of(screen, bar, band, scale);
    let band = surround.map(|s| (s.left.y, s.left.h)).or(band);
    let dim = div().absolute().left_0().right_0().bg(color);
    let mut root = div().absolute().size_full().child(match band {
        Some((y, h)) if y <= 0.0 => dim.top(px(h)).bottom_0(),
        Some((y, _)) => dim.top_0().h(px(y)),
        None => dim.top_0().bottom_0(),
    });
    if let Some(surround) = surround {
        let NotchSurround {
            left,
            right,
            radius,
            flare,
            bottom,
        } = surround;
        let region = |r: Rect, start: bool| {
            // A shoulder may exceed half the bar's height. Give its quad
            // enough room for the full arc, then clip it to the bar band.
            let fill = div()
                .absolute()
                .left_0()
                .right_0()
                .h(px(r.h.max(flare * 2.0)))
                .bg(color);
            let fill = match (bottom, start) {
                (false, true) => fill.top_0().rounded_tr(px(flare)),
                (false, false) => fill.top_0().rounded_tl(px(flare)),
                (true, true) => fill.bottom_0().rounded_br(px(flare)),
                (true, false) => fill.bottom_0().rounded_bl(px(flare)),
            };
            div()
                .absolute()
                .left(px(r.x))
                .top(px(r.y))
                .w(px(r.w))
                .h(px(r.h))
                .overflow_hidden()
                .child(fill)
        };
        root = root.child(region(left, true)).child(region(right, false));
        // Fill the wallpaper outside each rounded far corner, without
        // covering the bar itself. These use the bar's same SDF arcs.
        let y = if bottom {
            surround.left.y
        } else {
            surround.left.y + surround.left.h - radius
        };
        let (a, b) = if bottom {
            (FlareCenter::BottomRight, FlareCenter::BottomLeft)
        } else {
            (FlareCenter::TopRight, FlareCenter::TopLeft)
        };
        root = root
            .child(concave_flare(surround.left.right(), y, radius, a, color))
            .child(concave_flare(
                surround.right.x - radius,
                y,
                radius,
                b,
                color,
            ));
    }
    root
}

#[derive(Copy, Clone, Debug, PartialEq)]
struct NotchSurround {
    left: Rect,
    right: Rect,
    radius: f32,
    flare: f32,
    bottom: bool,
}

impl NotchSurround {
    fn of(
        (sw, sh): (f32, f32),
        bar: &Metrics,
        band: Option<(f32, f32)>,
        scale: f32,
    ) -> Option<Self> {
        if band.is_none() || !bar.notch() {
            return None;
        }
        let frame = super::layout::orient(bar.position, bar.frame(sw), (sw, sh));
        let (x, y, w, h) = snap_rect(
            f32::from(frame.origin.x),
            f32::from(frame.origin.y),
            f32::from(frame.size.width),
            bar.height,
            scale,
        );
        let bottom = bar.position == BarPosition::Bottom;
        Some(Self {
            left: Rect::new(0.0, y, x, h),
            right: Rect::new(x + w, y, (sw - x - w).max(0.0), h),
            radius: bar.frame_radius().min(w / 2.0),
            flare: snap(bar.flare, scale).clamp(0.0, h),
            bottom,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::BarLayout;

    #[test]
    fn joined_notch_dims_wallpaper_at_both_screen_corners() {
        for position in [BarPosition::Top, BarPosition::Bottom] {
            for scale in [1.0, 1.25, 1.5, 2.0] {
                let bar = Metrics::default_for(BarLayout::Notch, position);
                let y = if position == BarPosition::Top {
                    0.0
                } else {
                    1080.0 - bar.height
                };
                let shape = NotchSurround::of((1920.0, 1080.0), &bar, Some((y, bar.height)), scale)
                    .unwrap();
                let frame = bar.frame(1920.0);
                assert!((shape.left.w - f32::from(frame.origin.x)).abs() < 1.0 / scale);
                assert!((shape.left.h - bar.height).abs() < 1.0 / scale);
                assert_eq!(
                    shape.left.y,
                    if position == BarPosition::Top {
                        0.0
                    } else {
                        snap(1080.0 - bar.height, scale)
                    }
                );
                assert_eq!(shape.right.right(), 1920.0);
                assert!(
                    (shape.right.x - f32::from(frame.origin.x + frame.size.width)).abs()
                        < 2.0 / scale
                );
                assert!(
                    shape.left.w > shape.flare && shape.right.w > shape.flare,
                    "screen corners are covered beyond the notch shoulders"
                );
                assert!(
                    shape.left.right() < 960.0 && shape.right.x > 960.0,
                    "the actual bar stays bright"
                );
                assert_eq!(shape.radius, bar.frame_radius());
                assert_eq!(shape.bottom, position == BarPosition::Bottom);
            }
        }
    }

    #[test]
    fn other_placements_keep_the_existing_dim_boundary() {
        let screen = (1920.0, 1080.0);
        let notch = Metrics::default_for(BarLayout::Notch, BarPosition::Top);
        assert_eq!(
            NotchSurround::of(screen, &notch, None, 1.0),
            None,
            "a menu away from the bar dims the whole output"
        );
        for layout in [
            BarLayout::MinimalEdge,
            BarLayout::Hairline,
            BarLayout::FloatingPills,
            BarLayout::ContiguousIsland,
            BarLayout::Micro,
        ] {
            let bar = Metrics::default_for(layout, BarPosition::Top);
            assert_eq!(
                NotchSurround::of(screen, &bar, Some((0.0, bar.height)), 1.0),
                None
            );
        }
    }
}
