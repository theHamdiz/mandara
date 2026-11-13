//! Review catalog writes before recording the user's activation request.

mod scroll;

use super::{Job, StudioView, button, layout};
use crate::tokens::*;
use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tf, tk, tr};
use mandara_services::app_themes::approval::Review;
use mandara_theme::MandaraPalette;
use std::sync::{Arc, atomic::AtomicBool};

/// Keep long multi-client manifests bounded during each view build.
const ROWS_PER_PAGE: usize = 16;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind { File, Command, Action }
struct Line {
    kind: Kind,
    text: SharedString,
}
pub(super) struct Pending {
    id: SharedString,
    fingerprint: String,
    lines: Arc<[Line]>,
    shared_palette: bool,
    page: usize,
    scroll: gpui::ScrollHandle,
    scrollbar: std::rc::Rc<mandara_ui::scrollbar::ScrollbarState>,
}
impl From<Review> for Pending {
    fn from(review: Review) -> Self {
        let mut lines = Vec::with_capacity(review.files.len() + review.commands.len() + review.actions.len());
        lines.extend(review.files.into_iter().map(|path| Line {
            kind: Kind::File,
            text: path.to_string_lossy().into_owned().into(),
        }));
        lines.extend(review.commands.into_iter().map(|command| Line {
            kind: Kind::Command,
            text: command.into(),
        }));
        lines.extend(review.actions.into_iter().map(|action| Line {
            kind: Kind::Action,
            text: match action.as_str() {
                "kde-color-scheme" => tk("Apply the color scheme to KDE apps and notify running apps.").into(),
                "firefox-theme" => tk("Enable browser theming and publish the shared app palette.").into(),
                _ => action.into(),
            },
        }));
        Self {
            id: review.id.into(),
            fingerprint: review.fingerprint,
            lines: lines.into(),
            shared_palette: review.shared_palette,
            page: 0,
            scroll: gpui::ScrollHandle::new(),
            scrollbar: mandara_ui::scrollbar::ScrollbarState::new(),
        }
    }
}
// The warning is normal-sized text; the accent role is too faint in light mode.
fn warning_ink(palette: &MandaraPalette) -> mandara_theme::Color {
    palette.on_surface
}

fn page_range(page: usize, len: usize) -> std::ops::Range<usize> {
    let page = page.min(len.div_ceil(ROWS_PER_PAGE).max(1) - 1);
    let start = page * ROWS_PER_PAGE;
    start..(start + ROWS_PER_PAGE).min(len)
}

impl StudioView {
    pub(super) fn template_review(
        &self,
        palette: &MandaraPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(review) = &self.app_templates.review else {
            return div().into_any_element();
        };
        let pages = review.lines.len().div_ceil(ROWS_PER_PAGE).max(1);
        let mut panel = div().id("app-template-review").on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
            if event.keystroke.key == "escape" { this.app_templates.review = None; cx.stop_propagation(); cx.notify(); }
        })).flex().flex_col().gap(px(SPACE_12_PX)).p(px(SPACE_16_PX)).rounded(px(RADIUS_16_PX)).bg(palette.surface_container.hsla())
            .child(layout::controls_row().text_size(px(TEXT_13_5_PX)).child(t("Review")).child(review.id.clone()))
            .child(div().text_size(px(TEXT_12_PX)).child(t("Review files and commands before enabling this template. Commands can change other files too. Updates need a new review.")));
        if review.shared_palette {
            panel = panel.child(div().text_size(px(TEXT_12_PX)).text_color(warning_ink(palette).hsla()).child(t("Review and approve the shared browser palette first. Other Pywal apps will read these colors too.")));
        }
        let focused = palette.surface_container_high.hsla();
        let mut rows = div()
            .id("app-template-review-lines")
            .focusable()
            .tab_index(0)
            .focus(move |style| style.bg(focused))
            .track_scroll(&review.scroll)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if let Some(review) = &this.app_templates.review
                    && scroll::key(&review.scroll, &event.keystroke)
                {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .flex()
            .flex_col()
            .gap(px(SPACE_8_PX))
            .max_h(px(DIM_320_PX))
            .overflow_y_scroll();
        for line in &review.lines[page_range(review.page, review.lines.len())] {
            rows = rows.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(SPACE_4_PX))
                    .child(
                        div()
                            .text_size(px(TEXT_11_5_PX))
                            .text_color(palette.on_surface_variant.hsla())
                            .child(match line.kind {
                                Kind::Command => t("Commands"),
                                Kind::File => t("Files"),
                                Kind::Action => t("Actions"),
                            }),
                    )
                    .child(div().text_size(px(TEXT_12_PX)).child(if line.kind == Kind::Action { SharedString::from(tr(&line.text)) } else { line.text.clone() })),
            );
        }
        panel = panel.child(mandara_ui::scrollbar::with_scrollbar(
            rows,
            &review.scroll,
            &review.scrollbar,
            palette,
        ));
        if pages > 1 {
            let [previous_icon, next_icon] = layout::paging_icons();
            panel = panel.child(
                layout::controls_row()
                    .items_center()
                    .child(
                        button(
                            "app-template-review-previous",
                            previous_icon,
                            t("Previous"),
                            false,
                            palette,
                        )
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, _, cx| {
                                if let Some(review) = &mut this.app_templates.review {
                                    review.page = review.page.saturating_sub(1);
                                    review.scroll.set_offset(gpui::Point::default());
                                }
                                cx.notify();
                            },
                        )),
                    )
                    .child(div().text_size(px(TEXT_12_PX)).child(tf(
                        "Page {page} of {pages}",
                        &[("page", &(review.page + 1)), ("pages", &pages)],
                    )))
                    .child(
                        button(
                            "app-template-review-next",
                            next_icon,
                            t("Next"),
                            false,
                            palette,
                        )
                        .on_click(cx.listener(
                            move |this, _: &ClickEvent, _, cx| {
                                if let Some(review) = &mut this.app_templates.review {
                                    review.page = (review.page + 1).min(pages - 1);
                                    review.scroll.set_offset(gpui::Point::default());
                                }
                                cx.notify();
                            },
                        )),
                    ),
            );
        }
        panel
            .child(
                layout::controls_row()
                    .child(
                        button(
                            "app-template-review-enable",
                            "check",
                            t("Enable"),
                            true,
                            palette,
                        )
                        .on_click(
                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.approve_app_template(cx)
                            }),
                        ),
                    )
                    .child(
                        button(
                            "app-template-review-cancel",
                            "close",
                            t("Cancel"),
                            false,
                            palette,
                        )
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, _, cx| {
                                this.app_templates.review = None;
                                cx.notify();
                            },
                        )),
                    ),
            )
            .into_any_element()
    }

    fn approve_app_template(&mut self, cx: &mut Context<Self>) {
        if self.app_templates.busy {
            return;
        }
        let Some(review) = self.app_templates.review.take() else {
            return;
        };
        let mut next = mandara_ui::state::config(cx)
            .read(cx)
            .get()
            .integrations
            .app_themes
            .clone();
        if let Err(error) = next.approve(&review.id, &review.fingerprint) {
            self.app_templates.error = Some(error.to_string());
            self.app_templates.review = Some(review);
            cx.notify();
            return;
        }
        // A later disable edits this intent immediately; completion only updates
        // the report, so it cannot re-enable a template the user switched off.
        mandara_ui::state::update_config(cx, |config| config.integrations.app_themes = next);
        let palette = mandara_core::theme_entity(cx).read(cx).target().clone();
        let report = mandara_ui::state::export_app_themes_report(palette, cx);
        self.app_templates.busy = true;
        let task = cx.spawn(async move |view, cx| {
            let report = report.await;
            let _ = view.update(cx, |this, cx| {
                this.app_templates.error = (!report.errors.is_empty()).then(|| {
                    report
                        .errors
                        .iter()
                        .map(|(id, error)| format!("{id}: {error}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                });
                this.app_templates.busy = false;
                cx.notify();
            });
        });
        self.app_templates.job = Some(Job {
            cancel: Arc::new(AtomicBool::new(false)),
            _task: task,
        });
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_palette_warning_meets_normal_text_contrast_in_both_modes() {
        // WCAG AA requires at least 4.5:1 for this text size.
        const MIN_CONTRAST: f32 = 4.5;
        for dark in [false, true] {
            let palette = mandara_theme::engine::default_palette(dark);
            assert!(
                warning_ink(&palette).contrast_ratio(palette.surface_container) >= MIN_CONTRAST
            );
        }
    }

    #[test]
    fn every_destination_and_command_is_reachable_on_bounded_review_pages() {
        let count = ROWS_PER_PAGE * 2 + 1;
        let visible = (0..count.div_ceil(ROWS_PER_PAGE))
            .flat_map(|page| page_range(page, count))
            .collect::<Vec<_>>();
        assert_eq!(visible, (0..count).collect::<Vec<_>>());
        assert_eq!(page_range(usize::MAX, count), count - 1..count);
        assert_eq!(page_range(0, 0), 0..0);
    }
    #[test]
    fn native_actions_remain_visible_when_no_setup_commands_are_present() {
        let pending = Pending::from(Review {
            id: "community:kde".into(), revision: "test".into(), files: Vec::new(), commands: Vec::new(),
            actions: vec!["kde-color-scheme".into(), "firefox-theme".into()], fingerprint: "review".into(), shared_palette: true,
        });
        assert_eq!(pending.lines.len(), 2);
        assert!(pending.lines.iter().all(|line| line.kind == Kind::Action));
        assert!(pending.lines[0].text.contains("KDE"));
        assert!(pending.lines[1].text.contains("shared app palette"));
    }

}
