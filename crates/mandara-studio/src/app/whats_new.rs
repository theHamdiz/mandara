//! Studio → About → "What's new": the newest releases' notes from the
//! `CHANGELOG.md` compiled into this build (`mandara_version::changelog`),
//! the running version open. Only [`SHOWN`] releases are parsed, on
//! About's first show, and they leave memory with the page; the full
//! history stays one click away on GitHub and in `mandara changelog`.
//! The update card shows an available release's notes (from GitHub) with
//! the same [`release_notes`].

use gpui::prelude::FluentBuilder as _;
use gpui::{AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, px};
use mandara_core::ToGpui;
use mandara_i18n::t;
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_version::changelog::{self, Release};

use super::{StudioView, grouped};
use crate::tokens::*;

/// Releases About lists (and keeps in memory while it shows).
pub(super) const SHOWN: usize = 5;

/// The release this build shipped as (opened first). Reads only the
/// newest releases and keeps none of them.
pub(super) fn current_version() -> Option<String> {
    changelog::current(&changelog::recent(SHOWN), mandara_version::version()).map(|r| r.version.clone())
}

/// A section's icon, tone and translated title.
fn section_style(title: &str, p: &MandaraPalette) -> (&'static str, Color, SharedString) {
    match title {
        "New" => ("plus", p.primary, t("New").into()),
        "Improved" => ("arrow-up", p.on_surface_variant, t("Improved").into()),
        "Fixed" => ("check", p.on_surface_variant, t("Fixed").into()),
        "Under the hood" => ("sliders", p.on_surface_variant, t("Under the hood").into()),
        "Changes" => ("list-check", p.on_surface_variant, t("Changes").into()),
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
    /// What's new: a row per recent release, the open one expanded under
    /// its row, and the full history on GitHub.
    pub(super) fn whats_new_card(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let mono = mandara_ui::fonts::fonts(cx).mono.clone();
        let open = self.updates.whats_new.clone();
        let releases = self.updates.changelog.get_or_insert_with(|| changelog::recent(SHOWN));
        let mut rows = Vec::with_capacity(releases.len());
        for r in releases.iter() {
            let expanded = open.as_deref() == Some(r.version.as_str());
            let version = r.version.clone();
            let label = if r.is_unreleased() { t("Not released yet").to_string() } else { format!("{} {}", t("Mandara"), mandara_ui::bidi::ltr_isolate(&r.version)) };
            let caption = match (&r.date, expanded) {
                (Some(d), true) => d.clone(),
                (Some(d), false) => format!("{d} · {}", changelog::plain(&r.summary)),
                (None, true) => String::new(),
                (None, false) => changelog::plain(&r.summary),
            };
            let chevron = icon(if expanded { "arrow-down" } else { mandara_ui::bidi::dir_icon("chevron-right") }, ICON_14_PX, p.on_surface_variant);
            let head = grouped::row(Some("release-notes"), label).caption(caption).accent(expanded).trailing(chevron).render(false, mono.clone(), p);
            rows.push(
                div()
                    .id(SharedString::from(format!("whats-new-{}", r.version)))
                    .w_full()
                    .row()
                    .flex_col()
                    .child(div().id(SharedString::from(format!("whats-new-head-{}", r.version))).cursor_pointer().hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_3).hsla())).child(head).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.updates.whats_new = if this.updates.whats_new.as_deref() == Some(version.as_str()) { None } else { Some(version.clone()) };
                        cx.notify();
                    })))
                    .when(expanded, |el| el.child(div().pad_start(px(GROUP_ROW_PAD_X_PX + GROUP_TILE_PX + GROUP_ROW_GAP_PX)).pad_end(px(GROUP_ROW_PAD_X_PX)).pb(px(SPACE_16_PX)).child(release_notes(r, p))))
                    .into_any_element(),
            );
        }
        let history = div()
            .id("about-history")
            .flex_none()
            .h(px(DIM_30_PX))
            .px(px(SPACE_10_PX))
            .rounded_full()
            .row()
            .items_center()
            .gap(px(SPACE_6_PX))
            .cursor_pointer()
            .text_size(px(TEXT_12_5_PX))
            .font_weight(FontWeight::MEDIUM)
            .text_color(p.primary.hsla())
            .hover(|s| s.bg(p.primary.with_alpha(ALPHA_10).hsla()))
            .child(t("Full history"))
            .child(icon("external", ICON_13_PX, p.primary))
            .on_click(|_, _, _| super::open_link(super::RELEASES_URL));
        let header = grouped::header(t("What's new"), Some(t("The latest releases in plain words. Run mandara changelog to see every release in a terminal.").into()), vec![history.into_any_element()], p);
        grouped::section(header, grouped::group(rows, true, p)).id("about-whats-new").into_any_element()
    }
}
