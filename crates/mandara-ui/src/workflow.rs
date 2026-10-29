//! Starts the everyday-workflow features once the hub is up: the shared
//! window list, clipboard policy, screen time and the dock surfaces.

use gpui::App;

/// Call after [`crate::hub::init_hub`] and [`crate::overlays::init_overlays`].
pub fn init(cx: &mut App) {
    crate::windows::init(cx);
    crate::plugins::clipboard::init(cx);
    crate::plugins::screen_time::init(cx);
    crate::plugins::dock::init(cx);
    crate::cards::init(cx);
}
