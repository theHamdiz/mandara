//! Collapsible page sections ("folds"): every settings group on a page,
//! and the long bespoke cards, sit in one. The first fold on a page starts
//! open so a page never looks empty; the rest start closed with a one-line
//! preview of what's inside, so a page reads as a short table of contents.
//! Search results open everything. Your choices are remembered per page
//! ([`crate::ui_state::UiState::sections`]); "Expand all / Collapse all"
//! sits next to the title of pages with several folds.

use gpui::{AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui;
use mandara_i18n::t;
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::style::ClipRounded as _;

use super::StudioView;
use crate::tokens::*;

/// Pages with at least this many folds get "Expand all / Collapse all".
pub(super) const TOOLBAR_MIN_FOLDS: usize = 3;
/// Labels shown in a closed fold's preview line.
pub(super) const PREVIEW_LABELS: usize = 4;

/// What a fold shows in its header.
pub(super) struct FoldHead {
    pub title: SharedString,
    /// One line under the title while closed (what's inside).
    pub preview: String,
    /// A small count chip ("6").
    pub badge: Option<String>,
    /// Right-aligned extras (the group's Reset).
    pub trailing: Option<AnyElement>,
    /// Closed until opened, even as the page's first fold (bespoke
    /// sections; the first settings group opens instead).
    pub closed: bool,
}

/// "Height, Corner roundness, Opacity and 3 more".
pub(super) fn preview_of<'a>(labels: impl IntoIterator<Item = &'a str>) -> String {
    let labels: Vec<&str> = labels.into_iter().collect();
    let shown: Vec<String> = labels.iter().take(PREVIEW_LABELS).map(|l| mandara_i18n::tr(l)).collect();
    let rest = labels.len().saturating_sub(PREVIEW_LABELS);
    if rest == 0 { shown.join(", ") } else { mandara_i18n::tf("{list} and {count} more", &[("list", &shown.join(", ")), ("count", &rest)]) }
}

impl StudioView {
    /// The page-unique key of a fold (`Bar/Layout`).
    pub(super) fn fold_key(&self, name: &str) -> String {
        format!("{:?}/{name}", self.section)
    }

    /// Whether the fold `key` on this page is open: your choice, else its
    /// default (the page's first settings group opens).
    pub(super) fn fold_open(&self, key: &str) -> bool {
        let default = self.fold_keys.iter().find(|(k, _)| k == key).is_some_and(|(_, open)| *open);
        !self.query.is_empty() || self.ui.sections.get(key).copied().unwrap_or(default)
    }

    /// A collapsible section; `body` is only built while it's open.
    pub(super) fn fold(&mut self, name: &str, head: FoldHead, p: &MandaraPalette, cx: &mut Context<Self>, body: impl FnOnce(&mut Self, &mut Context<Self>) -> AnyElement) -> AnyElement {
        let key = self.fold_key(name);
        let first_group = !head.closed && !self.fold_keys.iter().any(|(_, open)| *open);
        self.fold_keys.push((key.clone(), first_group));
        let open = self.fold_open(&key);
        let toggle_key = key.clone();
        let header = div()
            .id(SharedString::from(format!("fold-{key}")))
            .min_h(px(DIM_52_PX))
            .px(px(SPACE_18_PX))
            .py(px(SPACE_12_PX))
            .row()
            .items_center()
            .gap(px(SPACE_14_PX))
            .cursor_pointer()
            .rounded(px(RADIUS_20_PX))
            .hover(|el| el.bg(p.on_surface.with_alpha(ALPHA_3_5).hsla()))
            .child(div().flex_none().size(px(DIM_28_PX)).rounded_full().bg(if open { p.primary.with_alpha(ALPHA_15).hsla() } else { p.surface_container_high.hsla() }).row().items_center().justify_center().child(icon(
                if open { "arrow-down" } else { mandara_ui::bidi::dir_icon("chevron-right") },
                ICON_14_PX,
                if open { p.primary } else { p.on_surface_variant },
            )))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_3_PX))
                    .child(div().text_size(px(TEXT_14_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface.hsla()).child(mandara_ui::bidi::paragraph(head.title)))
                    .when(!open && !head.preview.is_empty(), |el| el.child(div().truncate().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(head.preview))),
            )
            .children(head.trailing)
            .children(head.badge.map(|b| {
                div()
                    .flex_none()
                    .min_w(px(DIM_24_PX))
                    .h(px(DIM_22_PX))
                    .px(px(SPACE_8_PX))
                    .rounded_full()
                    .bg(p.surface_container_high.hsla())
                    .row()
                    .items_center()
                    .justify_center()
                    .text_size(px(TEXT_11_5_PX))
                    .font_weight(FontWeight::MEDIUM)
                    .font_features(mandara_ui::fonts::tabular_nums())
                    .text_color(p.on_surface_variant.hsla())
                    .child(b)
            }))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                let now = !this.fold_open(&toggle_key);
                this.ui.sections.insert(toggle_key.clone(), now);
                crate::ui_state::save(&this.ui);
                cx.notify();
            }));
        let content = open.then(|| body(self, cx));
        div()
            .clip_rounded()
            .rounded(px(RADIUS_20_PX))
            .bg(p.surface_container.hsla())
            .row()
            .flex_col()
            .child(header)
            .children(content.map(|c| div().border_t_1().border_color(p.on_surface.with_alpha(ALPHA_6).hsla()).p(px(SPACE_6_PX)).child(c)))
            .into_any_element()
    }

    /// "Expand all · Collapse all" for the page just built (`None` with few
    /// folds, and in search results, where everything is open anyway).
    pub(super) fn fold_toolbar(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.fold_keys.len() < TOOLBAR_MIN_FOLDS || !self.query.is_empty() {
            return None;
        }
        let all_open = self.fold_keys.iter().all(|(k, _)| self.fold_open(k));
        let all_closed = self.fold_keys.iter().all(|(k, _)| !self.fold_open(k));
        let link = |id: &'static str, label: SharedString, glyph: &'static str, enabled: bool| {
            div()
                .id(id)
                .h(px(DIM_30_PX))
                .px(px(SPACE_12_PX))
                .rounded_full()
                .row()
                .items_center()
                .gap(px(SPACE_6_PX))
                .text_size(px(TEXT_12_5_PX))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if enabled { p.on_surface.hsla() } else { p.on_surface_variant.with_alpha(ALPHA_50).hsla() })
                .when(enabled, |el| el.cursor_pointer().hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla())))
                .child(icon(glyph, ICON_14_PX, if enabled { p.on_surface_variant } else { p.on_surface_variant.with_alpha(ALPHA_50) }))
                .child(label)
        };
        let set_all = |open: bool| {
            move |this: &mut Self, _: &ClickEvent, _: &mut gpui::Window, cx: &mut Context<Self>| {
                for (k, _) in this.fold_keys.clone() {
                    this.ui.sections.insert(k, open);
                }
                crate::ui_state::save(&this.ui);
                cx.notify();
            }
        };
        Some(
            div()
                .row()
                .items_center()
                .gap(px(SPACE_4_PX))
                .child(link("folds-expand", t("Expand all").into(), "arrow-down", !all_open).when(!all_open, |el| el.on_click(cx.listener(set_all(true)))))
                .child(link("folds-collapse", t("Collapse all").into(), "arrow-up", !all_closed).when(!all_closed, |el| el.on_click(cx.listener(set_all(false)))))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_name_the_first_settings_and_count_the_rest() {
        assert_eq!(preview_of(["Height", "Opacity"]), "Height, Opacity");
        assert_eq!(preview_of(["A", "B", "C", "D", "E", "F"]), "A, B, C, D and 2 more");
        assert_eq!(preview_of(Vec::<&str>::new()), "");
    }
}
