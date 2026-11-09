//! Surface designs: the corner family (`style.surface_corners`) and the
//! elevation (`style.card_style`) of cards: the shell's panel and sidebar
//! cards ([`crate::kit::card`], [`crate::plugins::panel_fx::glass_card`])
//! and Studio's settings cards.
//!
//! The corner size still comes from the card's own radius and
//! `style.radius_scale`; the family only shapes it:
//!
//! - **Rounded** (the default): every corner alike.
//! - **Asymmetric**: Material 3 Expressive's mixed corners, the leading
//!   top and trailing bottom corners larger and the other two small
//!   (mirrored right to left).
//! - **Square**: small, crisp corners.
//!
//! Elevation: **Tonal** (the default) is the card as it always was;
//! **Raised** adds a soft shadow (only while `style.shadows` is on);
//! **Outlined** trades the specular rim for a fine outline. Panels and
//! popups keep `style.shadows` and `style.borders`.
//!
//! Children stay clipped to the card's shape: GPUI's rounded content masks
//! take a radius per corner.

use gpui::{BoxShadow, Corners, ParentElement, Pixels, Styled, canvas, px};
use mandara_core::{ToGpui as _, oklab_gradient};
use mandara_theme::{Color, MandaraPalette};

use super::cut::Corner;
use super::{CardStyle, SurfaceCorners};

/// The asymmetric family's large and small corners as multiples of the
/// card's radius, and the largest a large corner gets.
const ASYMMETRIC_LARGE: f32 = 1.6;
const ASYMMETRIC_SMALL: f32 = 0.35;
const ASYMMETRIC_MAX_PX: f32 = 36.0;
/// The square family's corners as a share of the card's radius.
const SQUARE_SHARE: f32 = 0.3;
/// The outlined card's edge alpha over the palette's outline colour.
const OUTLINE_ALPHA: f32 = 0.45;
/// The raised card's shadow strength (a fraction of the panel elevation).
const RAISED_ELEVATION: f32 = 0.6;
/// A squircle corner reaches this many times the card's radius along each
/// edge (a continuous corner looks as round as a circle of about two thirds
/// its reach), at most this far; a cut takes this share of the radius.
const SQUIRCLE_REACH: f32 = 1.6;
const SQUIRCLE_MAX_PX: f32 = 44.0;
const CUT_SHARE: f32 = 0.6;
/// The outlined card's edge.
const OUTLINE_PX: f32 = 1.0;

/// The drawn corner of a card whose (already scaled) radius is `r` in a
/// family drawn row by row ([`super::cut`]); `None` for the families the
/// rounded rectangle draws itself.
pub fn shaped(family: SurfaceCorners, r: f32) -> Option<Corner> {
    let r = r.max(0.0);
    match family {
        SurfaceCorners::Squircle => Some(Corner::Squircle((r * SQUIRCLE_REACH).min(SQUIRCLE_MAX_PX))),
        SurfaceCorners::Cut => Some(Corner::Cut(r * CUT_SHARE)),
        SurfaceCorners::Rounded | SurfaceCorners::Asymmetric | SurfaceCorners::Square => None,
    }
}

/// A card's fill: one colour, or a gradient from the first (top) to the
/// second (bottom).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Fill(pub Color, pub Option<Color>);

/// The corners of a surface whose (already scaled) radius is `r`, in
/// `family`.
pub fn corners(family: SurfaceCorners, r: f32, rtl: bool) -> Corners<Pixels> {
    let r = r.max(0.0);
    match family {
        SurfaceCorners::Rounded => Corners::all(px(r)),
        SurfaceCorners::Square => Corners::all(px(r * SQUARE_SHARE)),
        // Children clip to the rounded rectangle inside the drawn shape.
        SurfaceCorners::Squircle | SurfaceCorners::Cut => Corners::all(px(shaped(family, r).map_or(r, Corner::inscribed_radius))),
        SurfaceCorners::Asymmetric => {
            let (large, small) = (px((r * ASYMMETRIC_LARGE).min(ASYMMETRIC_MAX_PX).max(r)), px(r * ASYMMETRIC_SMALL));
            // Leading top and trailing bottom: top-left and bottom-right in
            // left-to-right languages.
            if rtl {
                Corners { top_left: small, top_right: large, bottom_left: large, bottom_right: small }
            } else {
                Corners { top_left: large, top_right: small, bottom_left: small, bottom_right: large }
            }
        }
    }
}

/// Shapes and lifts surfaces in the current component designs.
pub trait Surface: Styled + Sized {
    /// Corners for a surface of radius `r` (already scaled) in the
    /// current corner family.
    fn surface_corners(self, r: f32) -> Self {
        self.surface_corners_as(crate::style::surface_corners(), r)
    }

    /// [`Self::surface_corners`] in `family`.
    fn surface_corners_as(self, family: SurfaceCorners, r: f32) -> Self {
        self.with_corners(corners(family, r, crate::bidi::rtl()))
    }

    /// Sets each corner's radius.
    fn with_corners(mut self, c: Corners<Pixels>) -> Self {
        let s = self.style();
        s.corner_radii.top_left = Some(c.top_left.into());
        s.corner_radii.top_right = Some(c.top_right.into());
        s.corner_radii.bottom_left = Some(c.bottom_left.into());
        s.corner_radii.bottom_right = Some(c.bottom_right.into());
        self
    }

    /// The current card style's edge and shadow, after `rim` (the card's
    /// own specular rim, kept by the tonal and raised styles).
    fn card_elevation(self, rim: Vec<BoxShadow>, palette: &MandaraPalette) -> Self {
        self.card_elevation_as(crate::style::card_style(), rim, palette)
    }

    /// [`Self::card_elevation`] in `style`.
    fn card_elevation_as(self, style: CardStyle, rim: Vec<BoxShadow>, palette: &MandaraPalette) -> Self {
        match style {
            CardStyle::Tonal => if rim.is_empty() { self } else { self.shadow(rim) },
            CardStyle::Raised => {
                let mut shadows = rim;
                if crate::style::shadows() {
                    shadows.extend(crate::plugins::panel_fx::elevation_scaled(palette, 1.0, RAISED_ELEVATION));
                }
                if shadows.is_empty() { self } else { self.shadow(shadows) }
            }
            CardStyle::Outlined => self.border_1().border_color(palette.outline.with_alpha(OUTLINE_ALPHA).hsla()),
        }
    }
}

impl<E: Styled> Surface for E {}

/// A card's whole surface in the current component designs: its corners,
/// fill, specular `rim` and style. Squircle and cut cards are drawn row by
/// row behind their children ([`super::cut::paint_shape`], no path built)
/// and clip the children to the rounded rectangle inside their shape; the
/// rim (an inset shadow that follows rounded corners) is left out there.
/// Call it before adding the card's children.
pub trait CardSurface: Surface + ParentElement {
    fn card_surface(self, r: f32, fill: Fill, rim: Vec<BoxShadow>, palette: &MandaraPalette) -> Self {
        self.card_surface_as(crate::style::surface_corners(), crate::style::card_style(), r, fill, rim, palette)
    }

    /// [`Self::card_surface`] in `family` and `style`.
    fn card_surface_as(self, family: SurfaceCorners, style: CardStyle, r: f32, fill: Fill, rim: Vec<BoxShadow>, palette: &MandaraPalette) -> Self {
        let Some(corner) = shaped(family, r) else {
            let el = self.surface_corners_as(family, r);
            let el = match fill {
                Fill(c, None) => el.bg(c.hsla()),
                Fill(top, Some(bottom)) => el.bg(oklab_gradient(crate::plugins::panel_fx::ANGLE_DOWN_DEG, top, bottom)),
            };
            return el.card_elevation_as(style, rim, palette);
        };
        let outline = (style == CardStyle::Outlined).then(|| (palette.outline.with_alpha(OUTLINE_ALPHA), OUTLINE_PX));
        let ink = super::cut::Ink { fill: Some(fill.0), fill_end: fill.1, outline };
        let el = self.surface_corners_as(family, r);
        let el = match style {
            CardStyle::Raised if crate::style::shadows() => el.shadow(crate::plugins::panel_fx::elevation_scaled(palette, 1.0, RAISED_ELEVATION)),
            _ => el,
        };
        el.relative().child(canvas(|_, _, _| {}, move |bounds, _, window, _| super::cut::paint_shape(window, bounds, corner, ink)).absolute().inset_0())
    }
}

impl<E: Surface + ParentElement> CardSurface for E {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_is_the_plain_radius_and_asymmetric_mirrors() {
        for r in [0.0, 8.0, 18.0, 28.0] {
            assert_eq!(corners(SurfaceCorners::Rounded, r, false), Corners::all(px(r)));
            assert_eq!(corners(SurfaceCorners::Rounded, r, true), Corners::all(px(r)));
            assert!(corners(SurfaceCorners::Square, r, false).top_left <= px(r));
            let (ltr, rtl) = (corners(SurfaceCorners::Asymmetric, r, false), corners(SurfaceCorners::Asymmetric, r, true));
            assert_eq!((ltr.top_left, ltr.bottom_right), (rtl.top_right, rtl.bottom_left), "{r}: mirrored");
            assert_eq!((ltr.top_right, ltr.bottom_left), (rtl.top_left, rtl.bottom_right));
            assert!(ltr.top_left >= ltr.top_right && ltr.top_left <= px(ASYMMETRIC_MAX_PX.max(r)));
        }
        assert_eq!(corners(SurfaceCorners::Rounded, -4.0, false), Corners::all(px(0.0)));
        // Squircle and cut cards clip their children inside the drawn shape.
        for family in [SurfaceCorners::Squircle, SurfaceCorners::Cut] {
            let corner = shaped(family, 18.0).unwrap();
            assert_eq!(corners(family, 18.0, false), Corners::all(px(corner.inscribed_radius())));
        }
        assert!(shaped(SurfaceCorners::Rounded, 18.0).is_none() && shaped(SurfaceCorners::Asymmetric, 18.0).is_none());
    }
}
