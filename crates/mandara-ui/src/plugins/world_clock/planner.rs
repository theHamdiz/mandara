//! Planner input shares the latest layout across redraws and remains left to right.

use std::cell::Cell;

use gpui::{Bounds, Pixels};

use crate::slider_model::Track;

/// Time moves from earlier to later physically left to right in every UI language.
pub(super) fn fraction(bounds: &Cell<Bounds<Pixels>>, x: Pixels) -> f32 {
    let bounds = bounds.get();
    Track::EDGE_TO_EDGE.value_at(f32::from(x), f32::from(bounds.origin.x), f32::from(bounds.size.width), false)
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use gpui::{point, px, size};

    use super::*;

    /// A prior handler follows relayout, while another planner keeps separate bounds.
    #[test]
    fn retained_handlers_follow_relayout_and_keep_independent_geometry() {
        let bounds = Rc::new(Cell::new(Bounds::new(point(px(10.0), px(0.0)), size(px(100.0), px(10.0)))));
        let handler = bounds.clone();
        let other = Cell::new(Bounds::<Pixels>::default());
        assert!(Rc::ptr_eq(&bounds, &handler));
        assert_eq!(fraction(&handler, px(35.0)), 0.25);
        bounds.set(Bounds::new(point(px(20.0), px(0.0)), size(px(50.0), px(10.0))));
        assert_eq!(fraction(&handler, px(35.0)), 0.3);
        assert_eq!(fraction(&handler, px(-10.0)), 0.0);
        assert_eq!(fraction(&handler, px(100.0)), 1.0);
        assert_eq!(other.get(), Bounds::default());
        for language in [mandara_i18n::Lang::En, mandara_i18n::Lang::Ar] {
            let _language = mandara_i18n::scoped(Some(language));
            assert_eq!(fraction(&handler, px(20.0)), 0.0);
            assert_eq!(fraction(&handler, px(70.0)), 1.0);
        }
        bounds.set(Bounds::default());
        assert_eq!(fraction(&handler, px(0.5)), 0.5);
    }
}
