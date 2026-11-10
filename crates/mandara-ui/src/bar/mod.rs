//! Bar geometry shared by the shell host.

pub mod layout;
pub mod items;
pub mod autohide;
pub mod gaps;
pub mod side;
pub(crate) mod scrim;

use std::cell::Cell;

use gpui::{AnyElement, App, Bounds, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId, Pixels, Window};

thread_local! {
    /// The axis of the bar whose item or panel is being laid out right now
    /// (`Some(true)`: a side bar), set by [`Oriented`]. A side bar beside a
    /// top bar holds the same plugins in their column form, so the config's
    /// bar position alone can't tell a plugin which form to draw.
    static AXIS: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Whether the bar item or panel being drawn sits on a side bar: bar
/// plugins then draw their compact column form (stacked clock digits, a
/// vertical tag strip, icons over values). Content wider than the side bar
/// is hidden by the shell. Outside a bar item it follows the bar's edge.
pub fn vertical(cx: &App) -> bool {
    AXIS.with(Cell::get).unwrap_or_else(|| crate::state::config(cx).read(cx).get().bar.position.is_vertical())
}

/// Runs `f` with [`vertical`] answering `axis`.
fn with_axis<R>(axis: bool, f: impl FnOnce() -> R) -> R {
    let before = AXIS.with(|a| a.replace(Some(axis)));
    let out = f();
    AXIS.with(|a| a.set(before));
    out
}

/// Lays out and paints `child` (a plugin's bar item or panel) for a bar on
/// a side edge or not: plugins asking [`vertical`] while they render get
/// that answer. A transparent wrapper: the child keeps its own layout.
pub struct Oriented {
    vertical: bool,
    child: AnyElement,
}

/// `child` drawn for a side bar (`vertical`) or a top / bottom one.
pub fn oriented(vertical: bool, child: impl IntoElement) -> Oriented {
    Oriented { vertical, child: child.into_any_element() }
}

impl IntoElement for Oriented {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for Oriented {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        // Uncached views render here.
        (with_axis(self.vertical, || self.child.request_layout(window, cx)), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) {
        // Cached views re-render here.
        with_axis(self.vertical, || {
            self.child.prepaint(window, cx);
        });
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window, cx: &mut App) {
        with_axis(self.vertical, || self.child.paint(window, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_axis_is_scoped_and_nests() {
        assert_eq!(AXIS.with(Cell::get), None);
        with_axis(true, || {
            assert_eq!(AXIS.with(Cell::get), Some(true));
            with_axis(false, || assert_eq!(AXIS.with(Cell::get), Some(false)));
            assert_eq!(AXIS.with(Cell::get), Some(true), "an inner item restores the outer axis");
        });
        assert_eq!(AXIS.with(Cell::get), None);
    }
}
