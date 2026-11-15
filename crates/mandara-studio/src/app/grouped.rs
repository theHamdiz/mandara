//! Grouped lists: the macOS-style building blocks Studio's system pages
//! share, so About, Maintenance, Profiles, Privacy and the hardware pages
//! read as one calm design.
//!
//! * [`section`]: a small header (title, caption, controls at the end)
//!   over an inset [`group`].
//! * [`group`]: one surface in the card design picked under Themes →
//!   Components, its rows divided by inset hairlines (no stacked cards).
//! * [`Row`]: a quiet icon tile, a title over a muted caption and its
//!   controls at the end; on a narrow page the controls drop under the
//!   text instead of squeezing it.
//! * [`status`]: a dot and a word: the one place a row shows a colour.

use gpui::{AnyElement, Div, FontWeight, IntoElement, ParentElement, SharedString, Styled, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui;
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::bidi::Bidi as _;
use mandara_ui::components::surfaces::{CardSurface as _, Fill};
use mandara_ui::icons::icon;

use crate::tokens::*;

/// A group's corner radius: the card design's, or Studio's own.
pub(super) fn radius() -> f32 {
    mandara_ui::style::studio_card_metrics().map_or(GROUP_RADIUS_PX, |(r, _)| r)
}

/// An inset group: `rows` on one surface, divided by hairlines that start
/// after the rows' icon tiles (`inset`: they have one).
pub(super) fn group(rows: Vec<AnyElement>, inset: bool, p: &MandaraPalette) -> Div {
    let line = hairline(p);
    let start = if inset { GROUP_ROW_PAD_X_PX + GROUP_TILE_PX + GROUP_ROW_GAP_PX } else { GROUP_ROW_PAD_X_PX };
    let mut body = div().clipped_card_surface(radius(), Fill(p.surface_container, None), Vec::new(), p).row().flex_col();
    for (i, row) in rows.into_iter().enumerate() {
        if i > 0 {
            body = body.child(div().w_full().pad_start(px(start)).child(div().h(px(BORDER_1_PX)).w_full().bg(line)));
        }
        body = body.child(row);
    }
    body
}

/// The hairline between rows and above a group's footer.
pub(super) fn hairline(p: &MandaraPalette) -> gpui::Hsla {
    p.on_surface.with_alpha(if p.is_dark { GROUP_HAIRLINE_DARK_ALPHA } else { GROUP_HAIRLINE_LIGHT_ALPHA }).hsla()
}

/// A group's header: the title, an optional caption and controls at the
/// end, aligned with the rows' text.
pub(super) fn header(title: impl Into<SharedString>, caption: Option<SharedString>, trailing: Vec<AnyElement>, p: &MandaraPalette) -> AnyElement {
    div()
        .row()
        .flex_wrap()
        .items_end()
        .gap(px(SPACE_12_PX))
        .px(px(SPACE_4_PX))
        .child(
            div()
                .flex_1()
                .min_w(px(DIM_200_PX))
                .row()
                .flex_col()
                .cross_start()
                .gap(px(SPACE_3_PX))
                .child(div().text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).child(mandara_ui::bidi::paragraph(title.into())))
                .when_some(caption, |el, c| el.child(div().max_w(px(TEXT_MEASURE_PX)).text_size(px(TEXT_12_5_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(c)))),
        )
        .children(trailing)
        .into_any_element()
}

/// A header over a group.
pub(super) fn section(header: AnyElement, body: impl IntoElement) -> Div {
    div().w_full().min_w(px(0.0)).row().flex_col().gap(px(GROUP_HEADER_GAP_PX)).child(header).child(body)
}

/// A row's state: a small dot in `tone` and a muted word.
pub(super) fn status(label: impl Into<SharedString>, tone: Color, p: &MandaraPalette) -> AnyElement {
    div()
        .flex_none()
        .row()
        .items_center()
        .gap(px(SPACE_6_PX))
        .text_size(px(TEXT_12_PX))
        .text_color(p.on_surface_variant.hsla())
        .child(div().size(px(DIM_6_PX)).flex_none().rounded_full().bg(tone.hsla()))
        .child(label.into())
        .into_any_element()
}

/// A row's icon tile: a neutral rounded square (an accent one when the
/// row is the current choice).
pub(super) fn tile(glyph: &'static str, accent: bool, p: &MandaraPalette) -> AnyElement {
    let (bg, fg) = if accent { (p.primary.with_alpha(ALPHA_15), p.primary) } else { (p.on_surface.with_alpha(ALPHA_6), p.on_surface_variant) };
    div().size(px(GROUP_TILE_PX)).flex_none().rounded(px(GROUP_TILE_RADIUS_PX)).bg(bg.hsla()).row().items_center().justify_center().child(icon(glyph, ICON_15_PX, fg)).into_any_element()
}

/// One row of a group.
pub(super) struct Row {
    glyph: Option<&'static str>,
    accent: bool,
    title: SharedString,
    caption: Option<SharedString>,
    mono: Option<SharedString>,
    trailing: Vec<AnyElement>,
    wrap: bool,
}

/// A row titled `title`, with `glyph` in its tile.
pub(super) fn row(glyph: Option<&'static str>, title: impl Into<SharedString>) -> Row {
    Row { glyph, accent: false, title: title.into(), caption: None, mono: None, trailing: Vec::new(), wrap: false }
}

impl Row {
    /// The muted line under the title.
    pub fn caption(mut self, caption: impl Into<SharedString>) -> Self {
        self.caption = Some(caption.into()).filter(|c: &SharedString| !c.is_empty());
        self
    }

    /// The caption may wrap (long help) instead of ending in an ellipsis.
    pub fn wrap(mut self) -> Self {
        self.wrap = true;
        self
    }

    /// The tile in the accent (the current choice, a running state).
    pub fn accent(mut self, on: bool) -> Self {
        self.accent = on;
        self
    }

    /// A value in the mono font on the trailing side (paths, builds).
    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.mono = Some(value.into());
        self
    }

    /// A control at the row's end.
    pub fn trailing(mut self, el: impl IntoElement) -> Self {
        self.trailing.push(el.into_any_element());
        self
    }

    /// The row; with `stack` (a narrow page) its controls go under the text.
    pub fn render(self, stack: bool, mono: SharedString, p: &MandaraPalette) -> AnyElement {
        let caption = self.caption.map(|c| {
            div()
                .w_full()
                .text_size(px(TEXT_12_PX))
                .line_height(px(LEADING_17_PX))
                .text_color(p.on_surface_variant.hsla())
                .when(!self.wrap, |el| el.truncate().text_start().child(c.clone()))
                .when(self.wrap, |el| el.child(mandara_ui::bidi::paragraph(c)))
        });
        let text = div()
            .flex_1()
            .min_w(px(0.0))
            .row()
            .flex_col()
            .cross_start()
            .gap(px(SPACE_3_PX))
            .child(div().w_full().text_size(px(TEXT_13_5_PX)).font_weight(FontWeight::MEDIUM).child(mandara_ui::bidi::paragraph(self.title)))
            .children(caption);
        let value = self.mono.map(|v| div().min_w(px(0.0)).max_w(px(GROUP_VALUE_MAX_W_PX)).truncate().font_family(mono).text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(v).into_any_element());
        let has_trailing = value.is_some() || !self.trailing.is_empty();
        let controls = div().flex_none().max_w_full().row().flex_wrap().items_center().gap(px(SPACE_8_PX)).children(value).children(self.trailing);
        let lead = div().flex_1().min_w(px(0.0)).row().items_center().gap(px(GROUP_ROW_GAP_PX)).children(self.glyph.map(|g| tile(g, self.accent, p))).child(text);
        let row = div().w_full().min_h(px(GROUP_ROW_MIN_H_PX)).px(px(GROUP_ROW_PAD_X_PX)).py(px(GROUP_ROW_PAD_Y_PX)).row();
        if stack && has_trailing {
            let indent = if self.glyph.is_some() { GROUP_TILE_PX + GROUP_ROW_GAP_PX } else { 0.0 };
            row.flex_col().cross_start().gap(px(SPACE_10_PX)).child(lead.w_full()).child(controls.pad_start(px(indent))).into_any_element()
        } else {
            row.items_center().gap(px(SPACE_16_PX)).child(lead).when(has_trailing, |el| el.child(controls)).into_any_element()
        }
    }
}

/// A group's empty state: a quiet line where its rows would be.
pub(super) fn empty(title: impl Into<SharedString>, caption: impl Into<SharedString>, p: &MandaraPalette) -> AnyElement {
    div()
        .px(px(GROUP_ROW_PAD_X_PX))
        .py(px(SPACE_16_PX))
        .row()
        .flex_col()
        .cross_start()
        .gap(px(SPACE_3_PX))
        .child(div().text_size(px(TEXT_13_5_PX)).font_weight(FontWeight::MEDIUM).child(title.into()))
        .child(div().max_w(px(TEXT_MEASURE_PX)).text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(caption)))
        .into_any_element()
}

/// A state worth knowing (a service that can't be reached, nothing to
/// show yet): a tile in `tone`, the title and the reason, which may wrap.
pub(super) fn notice_row(glyph: &'static str, tone: Color, title: impl Into<SharedString>, caption: impl Into<SharedString>, p: &MandaraPalette) -> AnyElement {
    div()
        .w_full()
        .min_h(px(GROUP_ROW_MIN_H_PX))
        .px(px(GROUP_ROW_PAD_X_PX))
        .py(px(GROUP_ROW_PAD_Y_PX))
        .row()
        .items_center()
        .gap(px(GROUP_ROW_GAP_PX))
        .child(div().size(px(GROUP_TILE_PX)).flex_none().rounded(px(GROUP_TILE_RADIUS_PX)).bg(tone.with_alpha(ALPHA_15).hsla()).row().items_center().justify_center().child(icon(glyph, ICON_15_PX, tone)))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .row()
                .flex_col()
                .cross_start()
                .gap(px(SPACE_3_PX))
                .child(div().text_size(px(TEXT_13_5_PX)).font_weight(FontWeight::MEDIUM).child(mandara_ui::bidi::paragraph(title.into())))
                .child(div().max_w(px(TEXT_MEASURE_PX)).text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(caption))),
        )
        .into_any_element()
}

/// [`notice_row`] on its own group surface.
pub(super) fn notice(glyph: &'static str, tone: Color, title: impl Into<SharedString>, caption: impl Into<SharedString>, p: &MandaraPalette) -> AnyElement {
    group(vec![notice_row(glyph, tone, title, caption, p)], false, p).into_any_element()
}
