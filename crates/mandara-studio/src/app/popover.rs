//! Floating popovers anchored to a trigger (the font picker's list, and any
//! Studio picker that needs one).
//!
//! The trigger keeps its size in its row: the popover is drawn later, over
//! everything ([`gpui::deferred`] + [`gpui::anchored`]), so opening it never
//! moves or widens the row, card or page around it. [`Anchor`] places it
//! under the trigger, or above when there's more room there, aligned to the
//! trigger's reading start (its right edge in right-to-left languages) and
//! kept inside the window. It springs in and out with the shell's motion
//! settings ([`Appear`]) and closes on a click outside it ([`layer`]).

use std::cell::Cell;
use std::rc::Rc;

use gpui::{AnyElement, Anchor as Corner, Bounds, InteractiveElement, IntoElement, ParentElement, Pixels, Styled, Window, anchored, canvas, deferred, div, point, px};
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui};
use mandara_theme::MandaraPalette;

use crate::tokens::*;

/// Gap between the trigger and the popover.
pub const GAP_PX: f32 = SPACE_6_PX;
/// Room kept between the popover and the window's edges.
pub const MARGIN_PX: f32 = SPACE_10_PX;
/// The popover slides this far while it fades in or out.
const SLIDE_PX: f32 = SPACE_8_PX;
/// Opening: quick with a hint of give; closing: critically damped, so it
/// never flashes back past zero.
const OPEN_SPRING: SpringConfig = SpringConfig::new(520.0, 34.0, 1.0);
const CLOSE_STIFFNESS: f32 = 640.0;
/// Shadow under the popover (offset down and blur), kept tight: a wide
/// translucent shadow becomes a halo under compositor blur.
const SHADOW_Y_PX: f32 = 8.0;
const SHADOW_BLUR_PX: f32 = 22.0;
/// Below this opacity a closing popover is gone.
const GONE: f32 = 0.01;

/// The trigger's window bounds, written each frame while it paints (so the
/// popover can be placed against them the next frame).
#[derive(Clone, Default)]
pub struct TriggerBounds(Rc<Cell<Option<Bounds<Pixels>>>>);

impl TriggerBounds {
    /// An invisible element filling the trigger that records its bounds.
    pub fn tracker(&self) -> impl IntoElement {
        let cell = self.0.clone();
        canvas(move |bounds, _, _| cell.set(Some(bounds)), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
    }

    pub fn get(&self) -> Option<Bounds<Pixels>> {
        self.0.get()
    }
}

/// Where a popover goes: the window point it hangs from, the corner of it
/// that sits there, and how tall it may be.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub x: f32,
    pub y: f32,
    pub above: bool,
    pub rtl: bool,
    /// The most the popover may be tall here.
    pub max_h: f32,
}

impl Anchor {
    /// Places a popover of `height` against a trigger `(x, y, w, h)` in a
    /// window `viewport_h` tall: under it, or above it when there isn't
    /// room under it and there is more above.
    pub fn place(trigger: (f32, f32, f32, f32), height: f32, viewport_h: f32, rtl: bool) -> Self {
        let (x, y, w, h) = trigger;
        let below = (viewport_h - (y + h) - GAP_PX - MARGIN_PX).max(0.0);
        let over = (y - GAP_PX - MARGIN_PX).max(0.0);
        let above = below < height && over > below;
        Self { x: if rtl { x + w } else { x }, y: if above { y - GAP_PX } else { y + h + GAP_PX }, above, rtl, max_h: if above { over } else { below } }
    }

    fn corner(&self) -> Corner {
        match (self.above, self.rtl) {
            (false, false) => Corner::TopLeft,
            (false, true) => Corner::TopRight,
            (true, false) => Corner::BottomLeft,
            (true, true) => Corner::BottomRight,
        }
    }
}

/// A popover's open / close motion.
pub struct Appear {
    open: bool,
    value: SpringValue<f32>,
    clock: FrameClock,
}

impl Default for Appear {
    fn default() -> Self {
        Self { open: false, value: SpringValue::new(0.0, OPEN_SPRING), clock: FrameClock::new() }
    }
}

impl Appear {
    pub fn open(&mut self) {
        self.open = true;
        self.value.set_config(OPEN_SPRING);
    }

    pub fn close(&mut self) {
        self.open = false;
        self.value.set_config(SpringConfig::critical(CLOSE_STIFFNESS));
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Steps the motion for this frame; `None` once it has closed.
    pub fn frame(&mut self, reduce_motion: bool, window: &mut Window) -> Option<f32> {
        self.value.set_instant(reduce_motion);
        self.value.set_target(if self.open { 1.0 } else { 0.0 });
        let dt = self.clock.tick(std::time::Instant::now());
        if self.value.step(dt) {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        let a = self.value.value().clamp(0.0, 1.0);
        (self.open || a > GONE).then_some(a)
    }
}

/// The popover surface: the card glass of Studio with its rim and a tight
/// shadow.
pub fn surface(width: f32, p: &MandaraPalette) -> gpui::Div {
    div()
        .w(px(width))
        .rounded(px(RADIUS_16_PX))
        .bg(p.surface_container_high.hsla())
        .border_1()
        .border_color(p.on_surface.with_alpha(ALPHA_10).hsla())
        .shadow(vec![gpui::BoxShadow { color: gpui::black().opacity(ALPHA_35), offset: point(px(0.0), px(SHADOW_Y_PX)), blur_radius: px(SHADOW_BLUR_PX), spread_radius: px(0.0), inset: false }])
}

/// The floating layer: `panel` at `anchor`, faded and slid by `appear`
/// (0 → 1), closing through `on_outside` when the pointer goes down
/// anywhere else.
pub fn layer(id: &'static str, anchor: Anchor, appear: f32, panel: impl IntoElement, on_outside: impl Fn(&mut Window, &mut gpui::App) + 'static) -> AnyElement {
    // Slides from the trigger's side.
    let slide = (1.0 - appear) * SLIDE_PX * if anchor.above { 1.0 } else { -1.0 };
    deferred(
        anchored().position(point(px(anchor.x), px(anchor.y))).anchor(anchor.corner()).snap_to_window_with_margin(px(MARGIN_PX)).child(
            div()
                .id(id)
                .occlude()
                .opacity(appear)
                .relative()
                .top(px(slide))
                .on_mouse_down_out(move |_, window, cx| on_outside(window, cx))
                .child(panel),
        ),
    )
    .with_priority(1)
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_below_unless_there_is_more_room_above() {
        let a = Anchor::place((100.0, 100.0, 300.0, 36.0), 320.0, 800.0, false);
        assert!(!a.above);
        assert_eq!((a.x, a.y), (100.0, 100.0 + 36.0 + GAP_PX));
        let b = Anchor::place((100.0, 700.0, 300.0, 36.0), 320.0, 800.0, false);
        assert!(b.above);
        assert_eq!(b.y, 700.0 - GAP_PX);
        assert!(b.max_h >= 320.0);
        // Right to left: hangs from the trigger's right edge.
        assert_eq!(Anchor::place((100.0, 100.0, 300.0, 36.0), 320.0, 800.0, true).x, 400.0);
        // Little room either way: the larger side, and the height it allows.
        let c = Anchor::place((0.0, 320.0, 300.0, 36.0), 500.0, 640.0, false);
        assert!(c.above && c.max_h < 500.0);
    }
}
