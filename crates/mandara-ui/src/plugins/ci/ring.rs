//! The workflow chip's dotted ring, painted without per-dot layout nodes.

use std::sync::LazyLock;
use std::time::Duration;

use gpui::{AnyElement, Bounds, IntoElement as _, Styled as _, canvas, fill, point, px, size};
use mandara_core::ToGpui as _;
use mandara_theme::Color;

/// Ten evenly spaced dots keep the running state legible at bar size.
const DOTS: usize = 10;
const DOT_PX: f32 = 2.6;
/// The four-dot tail fades toward the quiet track.
const TAIL: usize = 4;
const TAIL_FADE: f32 = 0.22;
/// One step advances the head by one dot.
pub(super) const STEP: Duration = Duration::from_millis(110);

static DIRECTIONS: LazyLock<[(f32, f32); DOTS]> = LazyLock::new(|| std::array::from_fn(|i| {
    let angle = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * i as f32 / DOTS as f32;
    (angle.cos(), angle.sin())
}));

fn head(elapsed: Duration, reduce_motion: bool) -> usize {
    if reduce_motion { 0 } else { (elapsed.as_millis() / STEP.as_millis() % DOTS as u128) as usize }
}

fn dot(side: f32, index: usize) -> (f32, f32) {
    let (dx, dy) = DIRECTIONS[index];
    let half = side / 2.0;
    let radius = half - DOT_PX / 2.0;
    (half + radius * dx - DOT_PX / 2.0, half + radius * dy - DOT_PX / 2.0)
}

pub(super) fn draw(side: f32, elapsed: Duration, reduce_motion: bool, accent: Color, track: Color) -> AnyElement {
    let head = head(elapsed, reduce_motion);
    canvas(|_, _, _| (), move |bounds, _, window, _| {
        for i in 0..DOTS {
            let behind = (head + DOTS - i) % DOTS;
            let ink = if behind < TAIL { accent.with_alpha(1.0 - behind as f32 * TAIL_FADE) } else { track };
            let (x, y) = dot(side, i);
            let rect = Bounds::new(bounds.origin + point(px(x), px(y)), size(px(DOT_PX), px(DOT_PX)));
            window.paint_subpixel_quad(fill(rect, ink.hsla()).corner_radii(px(DOT_PX / 2.0)));
        }
    }).absolute().size_full().into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduced_motion_keeps_the_running_indicator_stationary() {
        assert_eq!(head(Duration::ZERO, false), 0);
        assert_eq!(head(STEP, false), 1);
        assert_eq!(head(STEP * DOTS as u32, false), 0);
        assert_eq!(head(STEP, true), 0);
        assert_eq!(head(STEP * 97, true), 0);
    }

    #[test]
    fn dots_fit_the_chip_and_opposite_dots_stay_symmetric() {
        let side = super::super::CHIP_BOX_PX;
        let epsilon = side * f32::EPSILON;
        for i in 0..DOTS {
            let (x, y) = dot(side, i);
            assert!(x >= -epsilon && y >= -epsilon);
            assert!(x + DOT_PX <= side + epsilon && y + DOT_PX <= side + epsilon);
            let (opposite_x, opposite_y) = dot(side, (i + DOTS / 2) % DOTS);
            assert!((x + opposite_x + DOT_PX - side).abs() < super::super::SIZE_EPSILON_PX);
            assert!((y + opposite_y + DOT_PX - side).abs() < super::super::SIZE_EPSILON_PX);
        }
    }
}
