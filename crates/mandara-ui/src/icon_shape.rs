//! One shape for every app icon in the launcher and the dock
//! (`[appearance] app_icon_shape`). Icon themes draw apps as squares,
//! circles and free silhouettes; each sits centred on the same plate (a
//! squircle by default) so a row or grid of them reads as one set.
//!
//! The squircle is a rounded square drawn as a GPU quad, not a path: the
//! dock animates its icons and the launcher's grid shows dozens, and a
//! quad costs nothing per frame where a path is tessellated every frame.

use gpui::{AnyElement, App, IntoElement, ParentElement, Styled, div, px};
use mandara_config::AppIconShape;
use mandara_core::ToGpui as _;
use mandara_theme::{Color, MandaraPalette};

/// The icon's share of its plate: room for the plate's curve to show.
const ICON_SHARE: f32 = 0.74;
/// Corner radius of the squircle, as a share of its side (close to the
/// continuous-corner app shape at icon sizes).
const SQUIRCLE_RADIUS_SHARE: f32 = 0.3;

/// The shape the config asks for.
pub fn current(cx: &App) -> AppIconShape {
    crate::state::config(cx).read(cx).get().appearance.app_icon_shape
}

/// The size to draw an icon at, for a plate `size` wide.
pub fn inner(shape: AppIconShape, size: f32) -> f32 {
    match shape {
        AppIconShape::Original => size,
        AppIconShape::Squircle | AppIconShape::Circle => size * ICON_SHARE,
    }
}

/// The plate behind app icons: a soft surface tone of the theme.
pub fn plate_color(palette: &MandaraPalette) -> Color {
    palette.surface_container_high
}

/// `icon` (drawn at [`inner`] size by the caller) centred on a `size` plate
/// of `shape` filled with `fill`; `Original` draws the icon alone.
pub fn plate(shape: AppIconShape, size: f32, fill: Color, icon: AnyElement) -> AnyElement {
    let base = div().size(px(size)).flex_none().flex().items_center().justify_center();
    match shape {
        AppIconShape::Original => base.child(icon).into_any_element(),
        AppIconShape::Circle => base.rounded_full().bg(fill.hsla()).child(icon).into_any_element(),
        AppIconShape::Squircle => base.rounded(px(size * SQUIRCLE_RADIUS_SHARE)).bg(fill.hsla()).child(icon).into_any_element(),
    }
}

/// The corner radius a plain tile of `size` gets in `shape` (monograms and
/// the launcher's non-app tiles match the app icons).
pub fn radius(shape: AppIconShape, size: f32) -> Option<f32> {
    match shape {
        AppIconShape::Original => None,
        AppIconShape::Circle => Some(size / 2.0),
        AppIconShape::Squircle => Some(size * SQUIRCLE_RADIUS_SHARE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_leave_room_for_the_plate_unless_original() {
        assert_eq!(inner(AppIconShape::Original, 48.0), 48.0);
        assert!(inner(AppIconShape::Squircle, 48.0) < 40.0);
        assert_eq!(radius(AppIconShape::Circle, 48.0), Some(24.0));
        assert_eq!(radius(AppIconShape::Original, 48.0), None);
    }
}
