//! An empty gesture target has one visible drop zone; opened shelves keep their chrome.

use gpui::{Bounds, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels, Styled, div, point, px, prelude::FluentBuilder as _};
use mandara_core::ToGpui as _;
use mandara_i18n::t;
use mandara_theme::MandaraPalette;

use super::Origin;
use crate::kit::{text, tokens};

/// Preserve the empty area's inset inside an explicitly opened shelf.
const SHELF_INSET: f32 = 14.0;
/// The inner outline follows the existing shelf's smaller corner radius.
const SHELF_RADIUS: f32 = 16.0;
/// The empty-state symbol remains the same size in both presentations.
const BADGE_SIZE: f32 = 52.0;
const ICON_SIZE: f32 = 24.0;
/// Quiet outlines and badge fill keep the drop prompt readable in either palette.
const BORDER_ALPHA: f32 = 0.25;
const BADGE_ALPHA: f32 = 0.14;
/// A live drag highlights the existing outline instead of adding another frame.
const ACTIVE_ALPHA: f32 = 0.9;

pub(super) fn frameless(origin: Option<Origin>, items: usize) -> bool {
    items == 0 && matches!(origin, Some(Origin::Shake | Origin::Edge(_)))
}

pub(super) fn can_dismiss(origin: Option<Origin>, items: usize, busy: u32) -> bool {
    busy == 0 && frameless(origin, items)
}

pub(super) fn input_bounds(card: Bounds<Pixels>, offset: (f32, f32), opacity: f32) -> Option<Bounds<Pixels>> {
    if opacity > 0.0 { Some(Bounds::new(card.origin + point(px(offset.0), px(offset.1)), card.size)) } else { None }
}

pub(super) fn empty(p: &MandaraPalette, busy: bool, frameless: bool, active: bool, fill: Hsla, radius: f32, layout: &crate::bidi::ParagraphLayout) -> impl IntoElement {
    div().size_full().when(!frameless, |el| el.p(px(SHELF_INSET))).child(
        div().id("stash-drop-zone").size_full()
            .rounded(px(crate::style::radius(if frameless { radius } else { SHELF_RADIUS })))
            .when(frameless, |el| el.bg(fill))
            .border_2().border_dashed()
            .border_color(if frameless && active { p.primary.with_alpha(ACTIVE_ALPHA) } else { p.on_surface_variant.with_alpha(BORDER_ALPHA) }.hsla())
            .flex().flex_col().items_center().justify_center().gap(px(tokens::SM))
            .child(div().size(px(BADGE_SIZE)).rounded_full().bg(p.primary.with_alpha(BADGE_ALPHA).hsla()).flex().items_center().justify_center().child(crate::icons::icon("download", ICON_SIZE, p.primary)))
            .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface.hsla()).child(if busy { t("Adding…") } else { t("Drop anything here") }))
            .child(div().w_full().min_w(px(0.0)).px(px(tokens::MD)).text_center().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(crate::bidi::editable_paragraph(t("Files, pictures, text and links wait here until you drag them out"), layout))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::stash::ScreenEdge;

    #[test]
    fn only_empty_gesture_targets_hide_the_shelf_frame() {
        for origin in [Origin::Shake, Origin::Edge(ScreenEdge::Left), Origin::Edge(ScreenEdge::Right), Origin::Edge(ScreenEdge::Top), Origin::Edge(ScreenEdge::Bottom)] {
            assert!(frameless(Some(origin), 0), "{origin:?}");
            assert!(!frameless(Some(origin), 1), "a completed drop becomes a shelf");
        }
        for origin in [None, Some(Origin::Asked)] {
            assert!(!frameless(origin, 0), "standalone and restored shelves retain their controls");
            assert!(!frameless(origin, 1));
        }
    }

    #[test]
    fn input_follows_the_painted_target_and_disappears_when_transparent() {
        let origin = 26.0;
        let width = 320.0;
        let height = 380.0;
        let card = Bounds::new(point(px(origin), px(origin)), gpui::size(px(width), px(height)));
        assert_eq!(input_bounds(card, (0.0, 40.0), 0.0), None);
        let offset = (-12.0, 20.0);
        let visible = input_bounds(card, offset, 0.5).unwrap();
        assert_eq!(visible.origin, card.origin + point(px(offset.0), px(offset.1)));
        assert_eq!(visible.size, card.size);
        assert_eq!(input_bounds(card, (0.0, 0.0), 1.0), Some(card));
    }

    #[test]
    fn abandoned_targets_close_without_discarding_a_pending_or_completed_drop() {
        for origin in [Some(Origin::Shake), Some(Origin::Edge(ScreenEdge::Bottom))] {
            assert!(can_dismiss(origin, 0, 0));
            assert!(!can_dismiss(origin, 0, 1));
            assert!(!can_dismiss(origin, 1, 0));
        }
        assert!(!can_dismiss(Some(Origin::Asked), 0, 0));
        assert!(!can_dismiss(None, 0, 0));
    }
}
