//! Text-field designs (`style.input_design`): every
//! [`crate::text_field::TextField`] (Studio's rows, the shell's editors and
//! forms) and the search boxes around them
//! ([`crate::plugins::extras::search_box`]).
//!
//! - **Classic**: drawn by the field's own code (a rounded tonal field with
//!   an accent edge while focused).
//! - **Filled**: Material 3's filled field: a tonal container with small
//!   top corners and square bottom ones, and an underline that turns
//!   accent and thicker while focused.
//! - **Outlined**: Material 3's outlined field: an outline that turns
//!   accent and thicker while focused; once the field is focused or holds
//!   text, its hint floats up into the outline as a small label.
//! - **Pill**: a fully rounded tonal field with an accent ring while
//!   focused.
//!
//! A field inside a search box ([`TextField::embedded`]) draws no box of
//! its own in these designs: the box draws it around the icon, the text
//! and the clear button.
//!
//! [`TextField::embedded`]: crate::text_field::TextField::embedded

use std::cell::Cell;
use std::rc::Rc;

use gpui::{Bounds, BoxShadow, ContentMask, Corners, IntoElement, ParentElement, Pixels, SharedString, Styled, canvas, div, point, prelude::FluentBuilder, px};
use mandara_core::ToGpui as _;
use mandara_theme::{Color, MandaraPalette};

use super::InputDesign;
use crate::bidi::Bidi as _;

/// Material's field corner (`CornerExtraSmall`), px before the corner scale.
pub const FIELD_RADIUS_PX: f32 = 4.0;
/// A multi-line pill field's corner (it can't be a full pill).
const PILL_MULTILINE_RADIUS_PX: f32 = 16.0;
/// Edge thickness at rest and while focused (Material's 1 and 2 dp).
pub const EDGE_PX: (f32, f32) = (1.0, 2.0);
/// The filled field's container: how far it leans toward the text colour.
const FILLED_TINT: f32 = 0.05;
/// The resting edge's alpha over the palette's outline.
const EDGE_ALPHA: f32 = 0.8;
/// The floating label: text size, its padding inside the outline's notch,
/// and how far in from the field's start it sits.
pub const LABEL_PX: f32 = 10.5;
const LABEL_PAD_PX: f32 = 4.0;
const LABEL_INSET_PX: f32 = 8.0;
/// The pill's focus ring.
const RING_PX: f32 = 2.0;
/// The floating label's line height, as a multiple of its size.
const LABEL_LEADING: f32 = 1.4;
/// A sample field: the text field's single-line height, side padding and
/// its classic corner (`crate::text_field`'s own).
const SAMPLE_H_PX: f32 = 32.0;
const SAMPLE_PAD_X_PX: f32 = 12.0;
const CLASSIC_RADIUS_PX: f32 = 10.0;

/// The colours and edges of a field in `design`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Look {
    /// Its container (`None`: see-through).
    pub fill: Option<Color>,
    /// The edge: `(colour, thickness)`.
    pub edge: Option<(Color, f32)>,
}

/// How a field in a non-classic `design` looks, `focused` or not.
pub fn look(design: InputDesign, focused: bool, palette: &MandaraPalette) -> Look {
    let edge_px = if focused { EDGE_PX.1 } else { EDGE_PX.0 };
    let edge_ink = if focused { palette.primary } else { palette.outline.with_alpha(EDGE_ALPHA) };
    match design {
        InputDesign::Classic => Look { fill: Some(palette.surface_container_high), edge: None },
        InputDesign::Filled => Look { fill: Some(palette.surface_container_high.mix(palette.on_surface, FILLED_TINT)), edge: Some((if focused { palette.primary } else { palette.on_surface_variant }, edge_px)) },
        InputDesign::Outlined => Look { fill: None, edge: Some((edge_ink, edge_px)) },
        InputDesign::Pill => Look { fill: Some(palette.surface_container_high), edge: focused.then_some((palette.primary, RING_PX)) },
    }
}

/// Dresses a field's (or search box's) root `el` in a non-classic
/// `design`. The root must be `relative`.
pub fn dress<E: Styled + ParentElement + FluentBuilder>(el: E, design: InputDesign, focused: bool, multiline: bool, palette: &MandaraPalette) -> E {
    dress_labelled(el, design, focused, multiline, None, palette)
}

/// [`dress`] with the outlined design's floating `label` (the field's hint,
/// once it is focused or holds text): it sits in a gap cut into the top
/// edge, so it reads on any background.
pub fn dress_labelled<E: Styled + ParentElement + FluentBuilder>(el: E, design: InputDesign, focused: bool, multiline: bool, label: Option<SharedString>, palette: &MandaraPalette) -> E {
    let l = look(design, focused, palette);
    let r = crate::style::radius(FIELD_RADIUS_PX);
    let el = el.when_some(l.fill, |el, c| el.bg(c.hsla()));
    match design {
        InputDesign::Classic => el,
        InputDesign::Filled => {
            let (ink, t) = l.edge.unwrap_or((palette.on_surface_variant, EDGE_PX.0));
            el.rounded_tl(px(r)).rounded_tr(px(r)).child(div().absolute().left_0().right_0().bottom_0().h(px(t)).bg(ink.hsla()))
        }
        InputDesign::Outlined => {
            let (ink, t) = l.edge.unwrap_or((palette.outline, EDGE_PX.0));
            // Where the label sits along the top edge, found as it lays out
            // (prepaint runs before any paint), so the edge leaves it a gap.
            let notch: Rc<Cell<Option<(Pixels, Pixels)>>> = Rc::new(Cell::new(None));
            let label = label.map(|text| floating_label(text, focused, notch.clone(), palette));
            el.rounded(px(r)).children(label).child(notched_ring(ink, t, r, notch))
        }
        InputDesign::Pill => {
            let el = if multiline { el.rounded(px(crate::style::radius(PILL_MULTILINE_RADIUS_PX))) } else { el.rounded_full() };
            match l.edge {
                Some((ink, t)) => el.shadow(vec![BoxShadow { color: ink.hsla(), offset: point(px(0.0), px(0.0)), blur_radius: px(0.0), spread_radius: px(t), inset: false }]),
                None => el,
            }
        }
    }
}

/// A search box's frame `el` around an embedded field (see
/// [`crate::text_field::TextField::embedded`]): the classic one is rounded by
/// `classic_radius` (before the corner scale) on the tonal container; the
/// other designs draw the field's own shape, `focused` or not.
pub fn search_frame(el: gpui::Div, classic_radius: f32, focused: bool, palette: &MandaraPalette) -> gpui::Div {
    match crate::style::input_design() {
        InputDesign::Classic => el.rounded(px(crate::style::radius(classic_radius))).bg(palette.surface_container_high.hsla()),
        design => dress(el.relative(), design, focused, false, palette),
    }
}

/// Marks `field` as sitting in a search box and says whether it is focused
/// (as it drew itself last; a field that gains or loses focus redraws its
/// window, so the box follows on the next frame).
pub fn embed(field: &gpui::Entity<crate::text_field::TextField>, cx: &mut gpui::App) -> bool {
    field.update(cx, |f, _| f.embed_in_box())
}

/// The outlined design's floating label: `text` straddling the top edge,
/// recording its span in `notch` for [`notched_ring`] (no patch behind it:
/// the edge itself stops short of the label).
fn floating_label(text: SharedString, focused: bool, notch: Rc<Cell<Option<(Pixels, Pixels)>>>, palette: &MandaraPalette) -> impl IntoElement {
    let half = LABEL_PX * LABEL_LEADING / 2.0;
    div()
        .absolute()
        .top(px(-half))
        .inset_start(px(LABEL_INSET_PX))
        .px(px(LABEL_PAD_PX))
        .text_size(px(LABEL_PX))
        .line_height(px(2.0 * half))
        .whitespace_nowrap()
        .text_color(if focused { palette.primary.hsla() } else { palette.on_surface_variant.hsla() })
        .child(text)
        .child(canvas(move |bounds, _, _| notch.set(Some((bounds.left(), bounds.right()))), |_, _, _, _| {}).absolute().top_0().left_0().size_full())
}

/// The three rectangles an outline is painted through to leave a gap
/// `x0..x1` in its top edge, `t` thick, inside `bounds`.
pub fn notch_masks(bounds: Bounds<Pixels>, (x0, x1): (Pixels, Pixels), t: f32) -> [Bounds<Pixels>; 3] {
    let (top, bottom, left, right) = (bounds.top(), bounds.bottom(), bounds.left(), bounds.right());
    let (x0, x1) = (x0.clamp(left, right), x1.clamp(left, right));
    [
        Bounds::from_corners(point(left, top), point(x0, bottom)),
        Bounds::from_corners(point(x1, top), point(right, bottom)),
        Bounds::from_corners(point(x0, (top + px(t)).min(bottom)), point(x1, bottom)),
    ]
}

/// The outlined design's edge, `t` thick and `r` round, with a gap where
/// the label recorded in `notch` sits (the whole ring without one).
fn notched_ring(ink: Color, t: f32, r: f32, notch: Rc<Cell<Option<(Pixels, Pixels)>>>) -> impl IntoElement {
    canvas(|_, _, _| {}, move |bounds, _, window, _| {
        let ring = gpui::quad(bounds, Corners::all(px(r)), gpui::transparent_black(), px(t), ink.hsla(), gpui::BorderStyle::Solid);
        match notch.get() {
            None => window.paint_quad(ring),
            Some(span) => {
                for mask in notch_masks(bounds, span, t) {
                    window.with_content_mask(Some(ContentMask { bounds: mask, corner_radii: Corners::default() }), |window| window.paint_quad(ring.clone()));
                }
            }
        }
    })
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// A still picture of a field in `design` holding `text` (with the outlined
/// design's floating `hint`): Studio's previews draw these instead of live
/// fields, so they keep no state.
pub fn sample(design: InputDesign, text: impl Into<SharedString>, hint: impl Into<SharedString>, palette: &MandaraPalette) -> gpui::Div {
    let base = div().relative().h(px(SAMPLE_H_PX)).px(px(SAMPLE_PAD_X_PX)).flex().items_center().whitespace_nowrap().text_color(palette.on_surface.hsla()).child(text.into());
    match design {
        InputDesign::Classic => base.rounded(px(crate::style::radius(CLASSIC_RADIUS_PX))).bg(palette.surface_container_high.hsla()).border_1().border_color(palette.surface_container_high.hsla()),
        InputDesign::Outlined => dress_labelled(base, design, false, false, Some(hint.into()), palette),
        _ => dress(base, design, false, false, palette),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_thickens_the_edge_in_the_accent() {
        let p = mandara_theme::engine::default_palette(true);
        for design in [InputDesign::Filled, InputDesign::Outlined] {
            let (rest, focus) = (look(design, false, &p), look(design, true, &p));
            assert!(rest.edge.unwrap().1 < focus.edge.unwrap().1, "{design:?}");
            assert_eq!(focus.edge.unwrap().0, p.primary, "{design:?}");
        }
        assert!(look(InputDesign::Outlined, false, &p).fill.is_none(), "outlined fields are see-through");
        assert!(look(InputDesign::Pill, false, &p).edge.is_none() && look(InputDesign::Pill, true, &p).edge.is_some(), "the ring shows only while focused");
        // The resting edge stands off the field's container.
        let filled = look(InputDesign::Filled, false, &p);
        assert_ne!(filled.fill, Some(filled.edge.unwrap().0));
    }

    /// The edge is painted through three rectangles that leave exactly the
    /// label's span open along the top, in either reading direction.
    #[test]
    fn the_notch_leaves_a_gap_only_behind_the_label() {
        let b = Bounds::new(point(px(10.0), px(20.0)), gpui::size(px(200.0), px(32.0)));
        for span in [(px(30.0), px(80.0)), (px(140.0), px(200.0)), (px(0.0), px(500.0))] {
            let t = 2.0;
            let [l, r, under] = notch_masks(b, span, t);
            let (x0, x1) = (span.0.clamp(b.left(), b.right()), span.1.clamp(b.left(), b.right()));
            assert_eq!((l.left(), l.right(), l.top(), l.bottom()), (b.left(), x0, b.top(), b.bottom()));
            assert_eq!((r.left(), r.right()), (x1, b.right()));
            assert_eq!((under.left(), under.right(), under.top(), under.bottom()), (x0, x1, b.top() + px(t), b.bottom()));
            // The three cover the field except the gap in the top edge.
            let covered = |x: f32, y: f32| [l, r, under].iter().any(|m| m.contains(&point(px(x), px(y))));
            assert!(!covered(f32::from(x0 + px(1.0)).min(f32::from(x1) - 0.5), f32::from(b.top()) + 0.5), "the top edge is open behind the label");
            assert!(covered(f32::from(b.left()) + 1.0, f32::from(b.top()) + 0.5) || x0 <= b.left() + px(1.0));
            assert!(covered(f32::from(x0 + px(1.0)).min(f32::from(x1) - 0.5), f32::from(b.bottom()) - 1.0), "the bottom edge stays whole");
        }
    }
}
