//! Gesture state belongs to one scrubber view, including its latest layout.

use std::cell::Cell;

use gpui::{Bounds, Pixels};

/// Shared by a view's current and previous event handlers across redraws.
/// Panel and sidebar instances keep separate geometry and drag previews.
#[derive(Default)]
pub(super) struct ScrubState {
    pub drag: Cell<Option<f32>>,
    pub bounds: Cell<Bounds<Pixels>>,
}

impl ScrubState {
    /// Maps pointer input through the latest painted track, clamped at its ends.
    pub fn fraction(&self, x: Pixels) -> f32 {
        let bounds = self.bounds.get();
        ((f32::from(x) - f32::from(bounds.origin.x)) / f32::from(bounds.size.width).max(1.0))
            .clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use gpui::{point, px, size};

    use super::*;

    /// Old handlers follow relayouts, and another surface cannot alter the gesture.
    #[test]
    fn handlers_follow_relayout_without_sharing_other_surfaces() {
        let panel = Rc::new(ScrubState::default());
        let handler = panel.clone();
        let sidebar = ScrubState::default();
        panel.bounds.set(Bounds {
            origin: point(px(10.0), px(0.0)),
            size: size(px(100.0), px(10.0)),
        });
        assert_eq!(handler.fraction(px(35.0)), 0.25);
        handler.drag.set(Some(0.25));
        panel.bounds.set(Bounds {
            origin: point(px(20.0), px(0.0)),
            size: size(px(50.0), px(10.0)),
        });
        assert_eq!(handler.fraction(px(35.0)), 0.3);
        assert_eq!(handler.fraction(px(-10.0)), 0.0);
        assert_eq!(handler.fraction(px(100.0)), 1.0);
        assert_eq!(panel.drag.take(), Some(0.25));
        assert!(handler.drag.get().is_none());
        assert!(sidebar.drag.get().is_none());
        assert_eq!(sidebar.bounds.get(), Bounds::default());
        panel.bounds.set(Bounds::default());
        assert_eq!(
            handler.fraction(px(0.5)),
            0.5,
            "an unpainted track cannot divide by zero"
        );
    }
}
