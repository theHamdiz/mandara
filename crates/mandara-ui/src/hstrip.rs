//! Horizontal strips (filter chips, collection pickers) that scroll the way
//! people expect, not only with a horizontal wheel or touchpad:
//!
//! * a vertical wheel scrolls them sideways;
//! * they can be grabbed and dragged; a press that moved more than
//!   [`DRAG_SLOP_PX`] is a drag, and [`HStrip::dragged`] tells the chips
//!   inside to ignore the click that ends it;
//! * small chevron buttons appear at an edge with more beyond it and page
//!   the strip by most of its width.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, Div, ElementId, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, ScrollHandle, Stateful, StatefulInteractiveElement as _, Styled as _, div, point,
    prelude::FluentBuilder as _, px,
};
use mandara_core::ToGpui as _;
use mandara_theme::MandaraPalette;

use crate::icons::icon;

/// Movement (logical px) after which a press is a drag, not a click.
pub const DRAG_SLOP_PX: f32 = 4.0;
/// Chevron buttons: side, glyph size and how far one press pages (share of
/// the visible width).
const CHEVRON_PX: f32 = 24.0;
const CHEVRON_ICON_PX: f32 = 13.0;
const PAGE_SHARE: f32 = 0.7;
/// Offsets closer than this to an end count as at that end.
const EDGE_EPSILON_PX: f32 = 1.0;

#[derive(Copy, Clone)]
struct Drag {
    start_x: f32,
    start_offset: f32,
    moved: bool,
}

/// A strip's scroll position and its drag in progress.
#[derive(Clone, Default)]
pub struct HStrip {
    pub scroll: ScrollHandle,
    drag: Rc<Cell<Option<Drag>>>,
}

impl HStrip {
    pub fn new() -> Self {
        Self::default()
    }

    /// The press under way moved far enough to be a drag: a chip's click
    /// handler should do nothing.
    pub fn dragged(&self) -> bool {
        self.drag.get().is_some_and(|d| d.moved)
    }

    /// Moves to `x` (≤ 0), clamped to the content.
    fn set_x(scroll: &ScrollHandle, x: f32) {
        let max = f32::from(scroll.max_offset().x).max(0.0);
        let y = scroll.offset().y;
        scroll.set_offset(point(px(x.clamp(-max, 0.0)), y));
    }

    /// The scrolling row (`overflow_x_scroll`, tracked), with the wheel and
    /// drag handling. Add the children and layout styles to it.
    pub fn row(&self, id: impl Into<ElementId>) -> Stateful<Div> {
        let (wheel, down, moving, drag_down, drag_move, drag_up, drag_out) =
            (self.scroll.clone(), self.scroll.clone(), self.scroll.clone(), self.drag.clone(), self.drag.clone(), self.drag.clone(), self.drag.clone());
        div()
            .id(id)
            .overflow_x_scroll()
            .track_scroll(&self.scroll)
            .on_scroll(move |e, window, cx| {
                let d = e.delta.pixel_delta(window.line_height());
                // A plain vertical wheel: scroll sideways. Horizontal deltas
                // (touchpads, tilt wheels) are handled by the scroll itself.
                if d.y.abs() > d.x.abs() {
                    Self::set_x(&wheel, f32::from(wheel.offset().x) + f32::from(d.y));
                    window.refresh();
                    cx.stop_propagation();
                }
            })
            .on_mouse_down(MouseButton::Left, move |e, _, _| {
                drag_down.set(Some(Drag { start_x: e.position.x.into(), start_offset: down.offset().x.into(), moved: false }));
            })
            .on_mouse_move(move |e, window, _| {
                let Some(mut d) = drag_move.get() else { return };
                if e.pressed_button != Some(MouseButton::Left) {
                    drag_move.set(None);
                    return;
                }
                let dx = f32::from(e.position.x) - d.start_x;
                if !d.moved && dx.abs() < DRAG_SLOP_PX {
                    return;
                }
                d.moved = true;
                drag_move.set(Some(d));
                Self::set_x(&moving, d.start_offset + dx);
                window.refresh();
            })
            // Chips' clicks run first (they read `dragged`), then the drag ends.
            .on_mouse_up(MouseButton::Left, move |_, _, _| drag_up.set(None))
            .on_mouse_up_out(MouseButton::Left, move |_, _, _| drag_out.set(None))
    }

    /// `row` with chevrons over the ends that have more beyond them.
    pub fn with_chevrons(&self, id: &'static str, row: Stateful<Div>, height: f32, palette: &MandaraPalette) -> AnyElement {
        let offset = -f32::from(self.scroll.offset().x);
        let max = f32::from(self.scroll.max_offset().x);
        let width = f32::from(self.scroll.bounds().size.width);
        let hover = palette.primary.with_alpha(0.18);
        let chevron = |side: &'static str, glyph: &'static str, toward: f32| {
            let scroll = self.scroll.clone();
            div()
                .id((id, if side == "left" { 0u64 } else { 1 }))
                .absolute()
                .top(px((height - CHEVRON_PX) / 2.0))
                .when(side == "left", |el| el.left_0())
                .when(side == "right", |el| el.right_0())
                .size(px(CHEVRON_PX))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .bg(palette.surface_container_high.hsla())
                .shadow(vec![crate::plugins::panel_fx::glow(palette.surface_dim.with_alpha(0.6), 8.0, 0.0, 2.0)])
                .hover(move |s| s.bg(hover.hsla()))
                .child(icon(glyph, CHEVRON_ICON_PX, palette.on_surface))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    let page = (width * PAGE_SHARE).max(CHEVRON_PX);
                    Self::set_x(&scroll, f32::from(scroll.offset().x) - toward * page);
                    window.refresh();
                })
        };
        div()
            .relative()
            .w_full()
            .h(px(height))
            .child(row.w_full().h(px(height)))
            .when(offset > EDGE_EPSILON_PX, |el| el.child(chevron("left", "chevron-left", -1.0)))
            .when(max - offset > EDGE_EPSILON_PX, |el| el.child(chevron("right", "chevron-right", 1.0)))
            .into_any_element()
    }
}
