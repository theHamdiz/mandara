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
        self.card_elevation_with(style, rim, palette, true)
    }

    #[doc(hidden)]
    fn card_elevation_with(self, style: CardStyle, rim: Vec<BoxShadow>, palette: &MandaraPalette, ornament: bool) -> Self {
        // Inked: every card outlined in ink, on its hard shadow.
        let ink = if ornament { crate::plugins::panel_fx::ink_detail_px() } else { 0.0 };
        if ink > 0.0 {
            let shadow = if crate::style::ink_shadow() > 0.0 { crate::plugins::panel_fx::elevation_scaled(palette, 1.0, 1.0) } else { Vec::new() };
            let el = self.border(px(ink)).border_color(crate::plugins::panel_fx::ink(palette).hsla());
            return if shadow.is_empty() { el } else { el.shadow(shadow) };
        }
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
        self.card_surface_with(family, style, r, fill, rim, palette, false, true)
    }

    /// [`Self::card_surface`] for a card that clips its children to its
    /// corners (art, hover fills and bands that reach its edge). A cut or
    /// squircle card clips them to the rounded rectangle inside its shape,
    /// while its own drawn shape paints outside that clip, so the corners
    /// stay cut or squircle instead of being rounded off by it.
    fn clipped_card_surface(self, r: f32, fill: Fill, rim: Vec<BoxShadow>, palette: &MandaraPalette) -> Self {
        self.clipped_card_surface_as(crate::style::surface_corners(), crate::style::card_style(), r, fill, rim, palette)
    }

    /// [`Self::clipped_card_surface`] in `family` and `style`.
    fn clipped_card_surface_as(self, family: SurfaceCorners, style: CardStyle, r: f32, fill: Fill, rim: Vec<BoxShadow>, palette: &MandaraPalette) -> Self {
        crate::style::ClipRounded::clip_rounded(self.card_surface_with(family, style, r, fill, rim, palette, true, true))
    }

    /// Clip the same card geometry without inheriting the current ink finish.
    /// Catalog previews add their own snapshot's texture and shadow afterward.
    fn clipped_card_geometry_as(self, family: SurfaceCorners, r: f32, fill: Fill, rim: Vec<BoxShadow>, palette: &MandaraPalette) -> Self {
        crate::style::ClipRounded::clip_rounded(self.card_surface_with(family, CardStyle::Tonal, r, fill, rim, palette, true, false))
    }

    /// The card surface; `clipped`: the card clips its children, and its
    /// drawn shape escapes that clip ([`gpui::Window::with_outer_content_mask`]).
    #[doc(hidden)]
    fn card_surface_with(self, family: SurfaceCorners, style: CardStyle, r: f32, fill: Fill, rim: Vec<BoxShadow>, palette: &MandaraPalette, clipped: bool, ornament: bool) -> Self {
        let Some(corner) = shaped(family, r) else {
            let el = self.surface_corners_as(family, r);
            let el = match fill {
                Fill(c, None) => el.bg(c.hsla()),
                Fill(top, Some(bottom)) => el.bg(oklab_gradient(crate::plugins::panel_fx::ANGLE_DOWN_DEG, top, bottom)),
            };
            // Inked (`[style.ink]`): a clipping card shades its far corner
            // with the ink texture, under its children.
            let texture = if clipped && ornament { crate::plugins::panel_fx::ink_texture(palette) } else { None };
            let el = match texture {
                Some(t) => el.relative().child(t),
                None => el,
            };
            return el.card_elevation_with(style, rim, palette, ornament);
        };
        let outline = (style == CardStyle::Outlined).then(|| (palette.outline.with_alpha(OUTLINE_ALPHA), OUTLINE_PX));
        let ink = super::cut::Ink { fill: Some(fill.0), fill_end: fill.1, outline };
        let el = self.surface_corners_as(family, r);
        let el = match style {
            CardStyle::Raised if crate::style::shadows() => el.shadow(crate::plugins::panel_fx::elevation_scaled(palette, 1.0, RAISED_ELEVATION)),
            _ => el,
        };
        el.relative().child(
            canvas(|_, _, _| {}, move |bounds, _, window, _| {
                // A clipping card's own mask is the innermost one here: the
                // shape paints under the clip its ancestors set instead.
                if clipped {
                    window.with_outer_content_mask(|window| super::cut::paint_shape(window, bounds, corner, ink));
                } else {
                    super::cut::paint_shape(window, bounds, corner, ink);
                }
            })
            .absolute()
            .inset_0(),
        )
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

    /// A clipping card (kit's `card`, `glass_card`) clips its children to
    /// the rounded rectangle inside a cut or squircle shape, but the shape
    /// itself paints under its ancestors' clip within the card's rectangle:
    /// none of the card's own rounding, so cut corners stay cut and
    /// squircles stay squircles, while a rounded ancestor's corner it shares
    /// still rounds it.
    #[test]
    fn clipping_cards_keep_their_drawn_corners() {
        use gpui::{Bounds, ContentMask, point, size};
        let palette = mandara_theme::engine::default_palette(true);
        for family in [SurfaceCorners::Squircle, SurfaceCorners::Cut] {
            let corner = shaped(family, 18.0).unwrap();
            let mut card = gpui::div().clipped_card_surface_as(family, CardStyle::Tonal, 18.0, Fill(palette.surface_container, None), Vec::new(), &palette);
            let style = card.style();
            let mut geometry = gpui::div().clipped_card_geometry_as(family, 18.0, Fill(palette.surface_container, None), Vec::new(), &palette);
            assert_eq!(geometry.style().corner_radii, style.corner_radii);
            assert_eq!(geometry.style().overflow, style.overflow);
            assert_eq!((style.overflow.x, style.overflow.y), (Some(gpui::Overflow::Clip), Some(gpui::Overflow::Clip)), "{family:?}: children are clipped");
            assert_eq!(style.corner_radii.top_left, Some(px(corner.inscribed_radius()).into()), "{family:?}: to the rounded rectangle inside the shape");
            // A panel rounded at 24 px, the card flush with its top-left corner.
            let panel = ContentMask { bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(400.0), px(300.0))), corner_radii: Corners::all(px(24.0)) };
            let own = ContentMask { bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(200.0), px(120.0))), corner_radii: corners(family, 18.0, false) };
            let card_mask = own.intersect(&panel);
            let outline = card_mask.outline_within(&panel);
            assert_eq!(outline.bounds, card_mask.bounds, "{family:?}: still inside the card");
            assert_eq!(outline.corner_radii.top_left, px(24.0), "{family:?}: the panel's corner still rounds it");
            for r in [outline.corner_radii.top_right, outline.corner_radii.bottom_left, outline.corner_radii.bottom_right] {
                assert_eq!(r, px(0.0), "{family:?}: the card's own rounding no longer clips its shape");
            }
        }
    }
}
