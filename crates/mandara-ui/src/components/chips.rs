//! Chip and segmented-choice designs (`style.chip_design`): Studio's
//! choices (every setting with options), the shell's segmented controls
//! ([`crate::plugins::extras::segmented`]), Studio's filter chips and the
//! panels' small info chips ([`crate::plugins::panel_fx::chip`]).
//!
//! - **Classic**: drawn by each control's own code (pills on a tonal track,
//!   filled chips).
//! - **Material**: Material 3's outlined segmented buttons (one outlined
//!   group, a fine divider between options, the chosen one tinted), and
//!   outlined filter chips with small corners and a check when chosen.
//! - **Connected**: Material 3 Expressive's connected button group:
//!   separate tonal buttons a hair apart, small inner corners and round
//!   outer ends; the chosen one fills and rounds fully. Chips are
//!   outlined pills.
//!
//! The helpers here give the shape and colours; the controls lay out their
//! options as before, so measured widths and grids stay the same.

use gpui::{Corners, Pixels, px};
use mandara_theme::{Color, MandaraPalette};

use super::ChipDesign;

/// Material's chip corner (`CornerSmall`), px before the corner scale.
pub const CHIP_RADIUS_PX: f32 = 8.0;
/// A connected group's inner corners, px before the corner scale.
pub const INNER_RADIUS_PX: f32 = 4.0;
/// The outline's alpha over the palette's outline colour.
const OUTLINE_ALPHA: f32 = 0.55;
/// The chosen option's tint over the accent (dark, light theme).
const CHOSEN_TINT: (f32, f32) = (0.22, 0.16);

/// Where an option sits in its row, left to right on screen.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// It is the row's leftmost / rightmost option.
    pub left_end: bool,
    pub right_end: bool,
}

impl Place {
    /// Option `i` of `n` in reading order (mirrored right to left).
    pub fn in_reading_order(i: usize, n: usize, rtl: bool) -> Self {
        let (first, last) = (i == 0, i + 1 == n);
        if rtl { Place { left_end: last, right_end: first } } else { Place { left_end: first, right_end: last } }
    }

    /// Option `i` of `n` laid out left to right in every language.
    pub fn visual(i: usize, n: usize) -> Self {
        Place { left_end: i == 0, right_end: i + 1 == n }
    }
}

/// Corners of an option `h` tall at `place` in a connected group: round
/// outer ends, small inner corners; a chosen one rounds fully.
pub fn connected_corners(place: Place, chosen: bool, h: f32) -> Corners<Pixels> {
    let full = h / 2.0;
    let inner = crate::style::radius(INNER_RADIUS_PX).min(full);
    let (l, r) = if chosen { (full, full) } else { (if place.left_end { full } else { inner }, if place.right_end { full } else { inner }) };
    Corners { top_left: px(l), bottom_left: px(l), top_right: px(r), bottom_right: px(r) }
}

/// Corners of an option `h` tall at `place` inside a Material outlined
/// group `radius` round: only the group's own ends are rounded.
pub fn group_corners(place: Place, radius: f32, h: f32) -> Corners<Pixels> {
    let r = radius.min(h / 2.0);
    let (l, rr) = (if place.left_end { r } else { 0.0 }, if place.right_end { r } else { 0.0 });
    Corners { top_left: px(l), bottom_left: px(l), top_right: px(rr), bottom_right: px(rr) }
}

/// A chip's corner radius `h` tall in a non-classic `design`.
pub fn chip_radius(design: ChipDesign, h: f32) -> f32 {
    match design {
        ChipDesign::Material => crate::style::radius(CHIP_RADIUS_PX).min(h / 2.0),
        ChipDesign::Classic | ChipDesign::Connected => h / 2.0,
    }
}

/// The outline of outlined chips and groups.
pub fn outline(palette: &MandaraPalette) -> Color {
    palette.outline.with_alpha(OUTLINE_ALPHA)
}

/// The chosen option's tint (Material's secondary container, from the
/// accent).
pub fn chosen(palette: &MandaraPalette) -> Color {
    palette.primary.with_alpha(if palette.is_dark { CHOSEN_TINT.0 } else { CHOSEN_TINT.1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ends_round_and_mirror_right_to_left() {
        let n = 4;
        for rtl in [false, true] {
            let places: Vec<Place> = (0..n).map(|i| Place::in_reading_order(i, n, rtl)).collect();
            assert_eq!(places.iter().filter(|p| p.left_end).count(), 1);
            assert_eq!(places.iter().filter(|p| p.right_end).count(), 1);
            // The first option sits at the reading start.
            assert_eq!(places[0].left_end, !rtl);
            assert_eq!(places[0].right_end, rtl);
        }
        let only = Place::in_reading_order(0, 1, false);
        assert!(only.left_end && only.right_end);
        assert_eq!(Place::visual(0, 3), Place { left_end: true, right_end: false });
    }

    #[test]
    fn corners_fit_the_option_and_the_chosen_one_rounds() {
        for h in [24.0, 28.0, 34.0] {
            for i in 0..3 {
                let place = Place::visual(i, 3);
                for chosen in [false, true] {
                    let c = connected_corners(place, chosen, h);
                    for r in [c.top_left, c.top_right, c.bottom_left, c.bottom_right] {
                        assert!(f32::from(r) >= 0.0 && f32::from(r) <= h / 2.0);
                    }
                }
                let g = group_corners(place, 99.0, h);
                assert_eq!(f32::from(g.top_left) > 0.0, place.left_end);
                assert_eq!(f32::from(g.top_right) > 0.0, place.right_end);
            }
            let middle = connected_corners(Place::visual(1, 3), false, h);
            assert!(middle.top_left < px(h / 2.0), "inner corners are small");
            assert_eq!(connected_corners(Place::visual(1, 3), true, h).top_left, px(h / 2.0));
            assert!(chip_radius(ChipDesign::Material, h) <= h / 2.0);
        }
    }
}
