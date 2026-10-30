//! Compact, scannable rows for Themes → Looks and Designed themes: a small
//! swatch (the palette's surface holding its four accents), the name and
//! one line of detail, the current one marked. Hovering a row previews it
//! in a tooltip (the big mock is drawn only then); "Show previews" brings
//! back the large cards. Every row is a few quads: a long catalog stays
//! one screen tall and cheap to draw.

use gpui::{AnyElement, AnyView, App, AppContext as _, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString, Stateful, Styled, Window, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui as _;
use mandara_i18n::t;
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;

use super::compositor_page::chip;
use crate::tokens::*;

/// The rows' grid: dense, two to four columns on a normal Studio window.
pub(super) const SWATCH_GRID: CardGrid = CardGrid { min_w: 236.0, max_w: 380.0, gap: SPACE_8_PX };
/// The swatch: the palette's surface with its accents as dots.
const SWATCH_W_PX: f32 = DIM_52_PX;
const SWATCH_H_PX: f32 = DIM_34_PX;
const SWATCH_DOT_PX: f32 = DIM_10_PX;
/// The hover preview's mock size.
pub(super) const TIP_MOCK_W_PX: f32 = 280.0;
pub(super) const TIP_MOCK_H_PX: f32 = 168.0;

/// The accents a swatch shows, in order.
fn accents(tp: &MandaraPalette) -> [Color; 4] {
    [tp.primary, tp.secondary, tp.tertiary, tp.accent_gradient_end]
}

/// A small swatch of `tp`: its base surface (so light and dark read at a
/// glance) holding its four accents.
pub(super) fn swatch(tp: &MandaraPalette) -> gpui::Div {
    div()
        .w(px(SWATCH_W_PX))
        .h(px(SWATCH_H_PX))
        .flex_none()
        .rounded(px(RADIUS_10_PX))
        .bg(tp.surface_base.hsla())
        .border_1()
        .border_color(tp.outline.with_alpha(ALPHA_25).hsla())
        .ltr_row()
        .items_center()
        .justify_center()
        .gap(px(SPACE_3_PX))
        .children(accents(tp).into_iter().map(|c| div().size(px(SWATCH_DOT_PX)).rounded_full().bg(c.hsla())))
}

/// One compact row: swatch, name (in `font` when given) and one line of
/// detail, the active one tinted, bordered and checked. The caller adds the
/// click and the tooltip.
pub(super) fn row(id: impl Into<gpui::ElementId>, tp: &MandaraPalette, name: SharedString, font: Option<SharedString>, detail: String, active: bool, p: &MandaraPalette) -> Stateful<gpui::Div> {
    div()
        .id(id.into())
        .flex_1()
        .min_w(px(0.0))
        .px(px(SPACE_8_PX))
        .py(px(SPACE_8_PX))
        .rounded(px(RADIUS_16_PX))
        .row()
        .items_center()
        .gap(px(SPACE_10_PX))
        .cursor_pointer()
        .bg(if active { p.primary.with_alpha(ALPHA_12).hsla() } else { p.surface_container.hsla() })
        .border_2()
        .border_color(if active { p.primary.hsla() } else { gpui::transparent_black() })
        .hover(|s| s.bg(p.primary.with_alpha(ALPHA_8).hsla()))
        .child(swatch(tp))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .row()
                .flex_col()
                .gap(px(SPACE_3_PX))
                .child(div().truncate().text_start().text_size(px(TEXT_13_5_PX)).font_weight(FontWeight::SEMIBOLD).when_some(font, |el, f| el.font_family(f)).child(name))
                .child(div().truncate().text_start().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(detail)),
        )
        .when(active, |el| el.child(div().size(px(DIM_20_PX)).flex_none().rounded_full().bg(p.primary.hsla()).row().items_center().justify_center().child(icon("check", ICON_12_PX, p.on_primary))))
}

/// The "Show previews" switch beside a section's title.
pub(super) fn previews_chip(id: &'static str, on: bool, p: &MandaraPalette) -> Stateful<gpui::Div> {
    chip(id, t("Show previews"), on, p).child(icon(if on { "check" } else { "image" }, ICON_12_PX, if on { p.on_primary } else { p.primary }))
}

/// A hover preview: any element drawn inside a raised card.
struct Preview {
    build: Box<dyn Fn() -> AnyElement>,
    bg: gpui::Hsla,
}

impl Render for Preview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().p(px(SPACE_6_PX)).rounded(px(RADIUS_16_PX)).bg(self.bg).shadow_lg().child((self.build)())
    }
}

/// A tooltip builder showing what `build` draws (called only while the
/// tooltip is up).
pub(super) fn preview_tip(p: &MandaraPalette, build: impl Fn() -> AnyElement + Clone + 'static) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let bg = p.surface_container_high.hsla();
    move |_, cx| {
        let build = build.clone();
        cx.new(|_| Preview { build: Box::new(build), bg }).into()
    }
}
