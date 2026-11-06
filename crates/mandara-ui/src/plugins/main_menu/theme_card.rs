//! Theme choices use their actual surface and accent tokens, with no raster assets.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    App, FontWeight, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, canvas, div, point, prelude::FluentBuilder as _, px, size,
};
use mandara_core::ToGpui;
use mandara_theme::{Color, MandaraPalette, catalog};

use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::{text, tokens};
use crate::style::ClipRounded as _;
use crate::theme_names::ThemeName;

/// Scrolling rows keep this compact height; fitting rows may share extra space.
pub(super) const ROW_H: f32 = 48.0;
/// A small desktop-shaped material sample leaves names most of the card width.
const PREVIEW_W: f32 = 38.0;
const PREVIEW_H: f32 = 30.0;
/// Keep labels readable without forcing long catalog names onto one line.
const LABEL_LINE_H: f32 = 14.0;
/// A selected card gets a quiet tonal fill rather than a second accent palette.
const SELECTED_TINT: f32 = 0.06;
const HOVER_TINT: f32 = 0.04;
/// The inset hairline defines the sample without a heavy outline.
const RIM_ALPHA: f32 = 0.28;
/// Selection stays visible in a small reserved slot, so titles never shift.
const CHECK_PX: f32 = 11.0;
/// Miniature text strokes are illustrative UI shapes, not fabricated content.
const PREVIEW_INK: f32 = 0.3;

#[derive(Clone, Copy)]
struct Preview {
    base: Color,
    rail: Color,
    window: Color,
    ink: Color,
    accents: [Color; 3],
}

impl Preview {
    /// Retain only the tokens the miniature needs, not another full palette.
    fn from_palette(p: &MandaraPalette) -> Self {
        Self {
            base: p.surface_dim,
            rail: p.surface_container,
            window: p.surface_container_high,
            ink: p.on_surface,
            accents: [p.primary, p.secondary, p.tertiary],
        }
    }
}

pub(super) struct Choice {
    id: String,
    name: String,
    builtin: bool,
    preview: Preview,
}

impl ThemeName for Choice {
    fn name(&self) -> &str {
        &self.name
    }
    fn is_builtin(&self) -> bool {
        self.builtin
    }
}

impl Choice {
    /// Expand each catalog entry once per appearance while the menu is visible.
    fn from_def(def: &catalog::ThemeDef, dark: bool) -> Self {
        Self {
            id: def.id.clone(),
            name: def.name.clone(),
            builtin: matches!(def.origin, catalog::ThemeOrigin::Builtin),
            preview: Preview::from_palette(&def.palette(dark)),
        }
    }
}

/// Fitting catalogs share the viewport height; longer catalogs retain content
/// height so their parent can scroll without compressing cards.
pub(super) fn grid(fits: bool) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(tokens::XS))
        .when(fits, |el| el.flex_1().min_h_0())
        .when(!fits, |el| el.flex_none())
}

/// Rows receive the grid's remaining height, keeping compact cards readable.
pub(super) fn row(fits: bool) -> gpui::Div {
    div()
        .row()
        .gap(px(tokens::XS))
        .when(fits, |el| el.flex_1().min_h(px(ROW_H)))
        .when(!fits, |el| el.flex_none().h(px(ROW_H)))
}

/// The wrapper must constrain its flex child so extra rows scroll inside it.
pub(super) fn scroll_viewport(
    list: impl IntoElement,
    scroll: &gpui::ScrollHandle,
    bar: &Rc<crate::scrollbar::ScrollbarState>,
    palette: &MandaraPalette,
) -> gpui::Div {
    crate::scrollbar::with_scrollbar(list, scroll, bar, palette)
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
}

/// Width belongs to the row; height follows its allocated grid track.
fn card_layout() -> gpui::Div {
    div().flex_1().min_w_0().h_full()
}

#[derive(Default)]
pub(super) struct Catalog {
    dark: Option<bool>,
    revision: u64,
    entries: Rc<Vec<Choice>>,
}

impl Catalog {
    /// Closing the menu releases labels and samples and invalidates late loads.
    pub fn clear(&mut self) {
        if self.dark.take().is_some() {
            self.revision = self.revision.wrapping_add(1);
            self.entries = Rc::default();
        }
    }

    /// Ignore a user-catalog load that completed after closing or changing mode.
    fn complete(&mut self, revision: u64, dark: bool, choices: Vec<Choice>) -> bool {
        if self.revision == revision && self.dark == Some(dark) {
            self.entries = Rc::new(choices);
            true
        } else {
            false
        }
    }

    /// Built-ins appear immediately; user TOML and palette expansion run off-thread.
    pub fn choices(cache: &Rc<RefCell<Self>>, dark: bool, cx: &mut App) -> Rc<Vec<Choice>> {
        if crate::state::shell(cx).read(cx).panel.as_deref() != Some(super::ID) {
            return cache.borrow().entries.clone();
        }
        if cache.borrow().dark != Some(dark) {
            let revision = {
                let mut state = cache.borrow_mut();
                state.revision = state.revision.wrapping_add(1);
                state.dark = Some(dark);
                state.entries = Rc::new(
                    catalog::builtin()
                        .iter()
                        .map(|def| Choice::from_def(def, dark))
                        .collect(),
                );
                state.revision
            };
            let weak = Rc::downgrade(cache);
            cx.spawn(async move |cx| {
                let choices = cx
                    .background_executor()
                    .spawn(async move {
                        catalog::all()
                            .iter()
                            .map(|def| Choice::from_def(def, dark))
                            .collect()
                    })
                    .await;
                cx.update(|cx| {
                    if weak
                        .upgrade()
                        .is_some_and(|cache| cache.borrow_mut().complete(revision, dark, choices))
                    {
                        cx.refresh_windows();
                    }
                });
            })
            .detach();
        }
        cache.borrow().entries.clone()
    }
}

/// The selection fill remains in the current interface's surface family.
fn surface(p: &MandaraPalette, selected: bool) -> Color {
    p.surface_base
        .mix(p.primary, if selected { SELECTED_TINT } else { 0.0 })
}

/// A small window, navigation strip and three accents describe the real palette.
fn sample(p: Preview) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let w = f32::from(bounds.size.width);
            let h = f32::from(bounds.size.height);
            let mut rect = |x: f32, y: f32, width: f32, height: f32, radius: f32, colour: Color| {
                window.paint_subpixel_quad(gpui::quad(
                    gpui::Bounds::new(
                        bounds.origin + point(px(x * w), px(y * h)),
                        size(px(width * w), px(height * h)),
                    ),
                    px(radius * h),
                    colour.hsla(),
                    gpui::Pixels::ZERO,
                    gpui::transparent_black(),
                    gpui::BorderStyle::default(),
                ));
            };
            rect(0.0, 0.0, 1.0, 1.0, 0.2, p.base);
            rect(0.07, 0.12, 0.15, 0.76, 0.07, p.rail);
            rect(0.29, 0.18, 0.63, 0.66, 0.11, p.window);
            rect(0.37, 0.29, 0.23, 0.07, 0.03, p.accents[0]);
            rect(0.37, 0.45, 0.42, 0.045, 0.02, p.ink.with_alpha(PREVIEW_INK));
            for (i, accent) in p.accents.into_iter().enumerate() {
                rect(0.37 + i as f32 * 0.16, 0.63, 0.1, 0.1, 0.03, accent);
            }
        },
    )
    .w(px(PREVIEW_W))
    .h(px(PREVIEW_H))
    .flex_none()
}

/// Pointer and keyboard activation share the same theme selection action.
pub(super) fn card(
    choice: &Choice,
    index: usize,
    current: &str,
    p: &MandaraPalette,
) -> impl IntoElement {
    let selected = choice.id == current;
    let name = choice.localized_name();
    let fill = surface(p, selected);
    let hover = p.surface_base.mix(p.primary, HOVER_TINT).hsla();
    let id = choice.id.clone();
    card_layout()
        .id(("menu-theme", index))
        .role(gpui::Role::RadioButton)
        .aria_label(name.clone())
        .aria_toggled(if selected {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .focusable()
        .tab_index(index as isize)
        .px(px(tokens::SM))
        .row()
        .items_center()
        .gap(px(tokens::XS))
        .rounded(px(crate::style::radius(tokens::MD)))
        .bg(fill.hsla())
        .border_1()
        .border_color(if selected {
            p.primary.with_alpha(RIM_ALPHA).hsla()
        } else {
            p.outline.with_alpha(RIM_ALPHA).hsla()
        })
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .focus_visible(move |style| style.border_color(p.primary.hsla()))
        .on_click(move |_, _, cx| super::apply_theme(id.clone(), cx))
        .child(
            div()
                .rounded(px(crate::style::radius(tokens::SM)))
                .clip_rounded()
                .child(sample(choice.preview)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_start()
                .text_size(px(text::BODY_S))
                .line_height(px(LABEL_LINE_H))
                .font_weight(if selected {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(p.on_surface.hsla())
                .child(crate::bidi::paragraph(name)),
        )
        .child(
            div()
                .w(px(CHECK_PX))
                .flex_none()
                .when(selected, |el| el.child(icon("check", CHECK_PX, p.primary))),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every flex boundary must pass spare height down to the actual card.
    #[test]
    fn fitting_catalogs_stretch_the_grid_rows_and_cards() {
        let mut grid = grid(true);
        let mut row = row(true);
        let mut card = card_layout();
        assert_eq!(grid.style().flex_grow, Some(1.0));
        assert_eq!(row.style().flex_grow, Some(1.0));
        assert_eq!(grid.style().size.height, None);
        assert_eq!(row.style().size.height, None);
        assert_eq!(row.style().min_size.height, Some(px(ROW_H).into()));
        assert_eq!(card.style().size.height, Some(gpui::relative(1.0).into()));
    }

    /// Extra user themes scroll at the existing row height instead of shrinking.
    #[test]
    fn overflowing_catalogs_keep_compact_rows_for_scrolling() {
        let mut grid = grid(false);
        let mut row = row(false);
        assert_eq!(grid.style().flex_grow, Some(0.0));
        assert_eq!(row.style().flex_grow, Some(0.0));
        assert_eq!(row.style().flex_shrink, Some(0.0));
        assert_eq!(row.style().size.height, Some(px(ROW_H).into()));
    }

    /// A block wrapper lets the list expand beyond the menu instead of scrolling.
    #[test]
    fn overflow_wrapper_passes_its_bounded_height_to_the_list() {
        let palette = catalog::builtin()[0].palette(true);
        let mut viewport = scroll_viewport(
            div().flex_1().min_h_0(),
            &gpui::ScrollHandle::new(),
            &crate::scrollbar::ScrollbarState::new(),
            &palette,
        );
        assert_eq!(viewport.style().display, Some(gpui::Display::Flex));
        assert_eq!(
            viewport.style().flex_direction,
            Some(gpui::FlexDirection::Column)
        );
        assert_eq!(viewport.style().flex_grow, Some(1.0));
        assert_eq!(viewport.style().min_size.height, Some(px(0.0).into()));
    }

    /// A cached preview follows locale changes without reloading its palette.
    #[test]
    fn builtin_labels_translate_when_the_card_is_rendered() {
        let def = catalog::builtin()
            .iter()
            .find(|def| def.id == catalog::DEFAULT_THEME)
            .unwrap();
        let choice = Choice::from_def(def, true);
        let arabic =
            mandara_i18n::with_language(Some(mandara_i18n::Lang::Ar), || choice.localized_name());
        assert_ne!(arabic, def.name);
        assert_eq!(
            arabic,
            mandara_i18n::with_language(Some(mandara_i18n::Lang::Ar), || mandara_i18n::tr(
                &def.name
            ))
        );
        assert_eq!(
            mandara_i18n::with_language(Some(mandara_i18n::Lang::En), || choice.localized_name()),
            def.name
        );
    }

    /// User labels stay literal even when their id or text matches a built-in key.
    #[test]
    fn user_overrides_and_named_themes_keep_their_authored_labels() {
        let mut def = catalog::builtin()[0].clone();
        def.origin = catalog::ThemeOrigin::User {
            path: "/tmp/authored-theme.toml".into(),
        };
        for name in [def.name.clone(), "Dark".into()] {
            def.name = name.clone();
            let choice = Choice::from_def(&def, true);
            assert_eq!(
                mandara_i18n::with_language(Some(mandara_i18n::Lang::Ar), || choice
                    .localized_name()),
                name
            );
        }
    }

    /// All catalog appearances keep the card label readable in both states.
    #[test]
    fn selected_and_idle_cards_keep_aa_text_contrast() {
        for def in catalog::builtin() {
            for dark in [true, false] {
                let p = def.palette(dark);
                for selected in [true, false] {
                    assert!(
                        p.on_surface.contrast_ratio(surface(&p, selected)) >= 4.5,
                        "{} {dark} {selected}",
                        def.id
                    );
                }
            }
        }
    }

    /// Cached previews use the catalog tokens rather than a generic gradient.
    #[test]
    fn samples_keep_each_themes_surface_and_three_accents() {
        for def in catalog::builtin() {
            for dark in [true, false] {
                let p = def.palette(dark);
                let choice = Choice::from_def(def, dark);
                assert_eq!(choice.preview.base, p.surface_dim);
                assert_eq!(choice.preview.accents, [p.primary, p.secondary, p.tertiary]);
            }
        }
    }

    /// A completed old request cannot restore images or labels after close.
    #[test]
    fn closing_or_changing_mode_rejects_stale_catalog_results() {
        let mut cache = Catalog {
            dark: Some(true),
            revision: 1,
            ..Default::default()
        };
        cache.clear();
        cache.complete(
            1,
            true,
            vec![Choice::from_def(&catalog::builtin()[0], true)],
        );
        assert!(cache.entries.is_empty());
        cache.dark = Some(false);
        cache.complete(
            1,
            true,
            vec![Choice::from_def(&catalog::builtin()[0], true)],
        );
        assert!(cache.entries.is_empty());
        cache.complete(
            cache.revision,
            false,
            vec![Choice::from_def(&catalog::builtin()[0], false)],
        );
        assert_eq!(cache.entries.len(), 1);
    }
}
