//! Studio → About → "What's new": every release's notes from the
//! `CHANGELOG.md` compiled into this build (`mandara_version::changelog`),
//! the running version open. The update card shows an available
//! release's notes (from GitHub) with the same [`release_notes`].

use std::sync::OnceLock;

use gpui::prelude::FluentBuilder as _;
use gpui::{AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, px};
use mandara_core::ToGpui;
use mandara_i18n::t;
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_version::changelog::{self, Release};

use super::StudioView;
use crate::tokens::*;

/// The compiled-in changelog, parsed once.
pub(super) fn releases() -> &'static [Release] {
    static RELEASES: OnceLock<Vec<Release>> = OnceLock::new();
    RELEASES.get_or_init(changelog::releases)
}

/// The release this build shipped as (opened first).
pub(super) fn current_version() -> Option<String> {
    changelog::current(releases(), mandara_version::version()).map(|r| r.version.clone())
}

/// A section's icon, tone and translated title.
fn section_style(title: &str, p: &MandaraPalette) -> (&'static str, Color, SharedString) {
    match title {
        "New" => ("plus", p.primary, t("New").into()),
        "Improved" => ("arrow-up", p.secondary, t("Improved").into()),
        "Fixed" => ("check", p.success_emerald, t("Fixed").into()),
        "Under the hood" => ("sliders", p.on_surface_variant, t("Under the hood").into()),
        "Changes" => ("list-check", p.tertiary, t("Changes").into()),
        other => ("circle", p.on_surface_variant, SharedString::from(other.to_string())),
    }
}

/// A release's summary and sections.
pub(super) fn release_notes(r: &Release, p: &MandaraPalette) -> AnyElement {
    let summary = (!r.summary.is_empty()).then(|| div().text_size(px(TEXT_13_PX)).line_height(px(LEADING_19_PX)).text_color(p.on_surface.with_alpha(ALPHA_85).hsla()).child(mandara_ui::bidi::paragraph(changelog::plain(&r.summary))));
    let sections = r.sections.iter().filter(|s| !s.items.is_empty()).map(|s| {
        let (glyph, tone, title) = section_style(&s.title, p);
        let items = s.items.iter().map(|item| {
            div()
                .row()
                .items_start()
                .gap(px(SPACE_10_PX))
                .child(div().flex_none().mt(px(SPACE_8_PX)).size(px(DIM_6_PX)).rounded_full().bg(tone.with_alpha(ALPHA_80).hsla()))
                .child(div().flex_1().min_w(px(0.0)).text_size(px(TEXT_12_5_PX)).line_height(px(LEADING_19_PX)).child(mandara_ui::bidi::paragraph(changelog::plain(item))))
        });
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_8_PX))
            .child(div().row().items_center().gap(px(SPACE_6_PX)).child(icon(glyph, ICON_14_PX, tone)).child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(tone.hsla()).child(title)))
            .children(items)
    });
    div().row().flex_col().gap(px(SPACE_14_PX)).children(summary).children(sections).into_any_element()
}

impl StudioView {
    /// The What's new card: one row per release, the open one expanded.
    pub(super) fn whats_new_card(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let open = self.updates.whats_new.clone();
        let rows = releases().iter().map(|r| {
            let expanded = open.as_deref() == Some(r.version.as_str());
            let version = r.version.clone();
            let label = if r.is_unreleased() { t("Not released yet").to_string() } else { format!("v{}", r.version) };
            let header = div()
                .id(SharedString::from(format!("whats-new-{}", r.version)))
                .row()
                .items_center()
                .gap(px(SPACE_12_PX))
                .px(px(SPACE_14_PX))
                .py(px(SPACE_10_PX))
                .rounded(px(RADIUS_12_PX))
                .cursor_pointer()
                .hover(|s| s.bg(p.surface_container_high.with_alpha(ALPHA_80).hsla()))
                .child(div().flex_none().text_size(px(TEXT_13_PX)).font_weight(FontWeight::SEMIBOLD).font_features(mandara_ui::fonts::tabular_nums()).text_color(if expanded { p.primary.hsla() } else { p.on_surface.hsla() }).child(label))
                .children(r.date.clone().map(|d| div().flex_none().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(d)))
                // Collapsed rows preview the summary on one line.
                .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).when(!expanded, |el| el.child(changelog::plain(&r.summary))))
                .child(icon(if expanded { "arrow-down" } else { mandara_ui::bidi::dir_icon("chevron-right") }, ICON_14_PX, p.on_surface_variant))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.updates.whats_new = if this.updates.whats_new.as_deref() == Some(version.as_str()) { None } else { Some(version.clone()) };
                    cx.notify();
                }));
            div().row().flex_col().child(header).when(expanded, |el| el.child(div().px(px(SPACE_14_PX)).pt(px(SPACE_4_PX)).pb(px(SPACE_14_PX)).child(release_notes(r, p))))
        });
        div()
            .id("about-whats-new")
            .rounded(px(RADIUS_22_PX))
            .bg(p.surface_container.hsla())
            .p(px(SPACE_20_PX))
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_16_PX))
                    .pb(px(SPACE_6_PX))
                    .child(div().flex_none().size(px(DIM_48_PX)).rounded_full().bg(p.tertiary.with_alpha(ALPHA_15).hsla()).row().items_center().justify_center().child(icon("release-notes", ICON_22_PX, p.tertiary)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(DIM_220_PX))
                            .row()
                            .flex_col()
                            .gap(px(SPACE_4_PX))
                            .child(div().text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).child(t("What's new")))
                            .child(div().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("What changed in each release, in plain words. Run mandara changelog to see it in a terminal.")))),
                    ),
            )
            .children(rows)
            .into_any_element()
    }
}
