//! Review catalog writes before recording the user's activation request.

use super::{Job, StudioView, button};
use crate::tokens::*;
use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tf};
use mandara_services::app_themes::approval::Review;
use mandara_theme::MandaraPalette;
use std::sync::{Arc, atomic::AtomicBool};

/// Keep long multi-client manifests bounded during each view build.
const ROWS_PER_PAGE: usize = 16;
struct Line {
    command: bool,
    text: SharedString,
}
pub(super) struct Pending {
    id: String,
    fingerprint: String,
    lines: Arc<[Line]>,
    shared_palette: bool,
    page: usize,
}
impl From<Review> for Pending {
    fn from(review: Review) -> Self {
        let mut lines = Vec::with_capacity(review.files.len() + review.commands.len());
        lines.extend(review.files.into_iter().map(|path| Line {
            command: false,
            text: path.to_string_lossy().into_owned().into(),
        }));
        lines.extend(review.commands.into_iter().map(|command| Line {
            command: true,
            text: command.into(),
        }));
        Self {
            id: review.id,
            fingerprint: review.fingerprint,
            lines: lines.into(),
            shared_palette: review.shared_palette,
            page: 0,
        }
    }
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
            .child(div().text_size(px(TEXT_13_5_PX)).child(t("Review")))
            .child(div().text_size(px(TEXT_12_PX)).child(t("Review files and commands before enabling this template. Commands can change other files too. Updates need a new review.")));
        if review.shared_palette {
            panel = panel.child(div().text_size(px(TEXT_12_PX)).text_color(palette.warning_amber.hsla()).child(t("Review and approve the shared browser palette first. Other Pywal apps will read these colors too.")));
        }
        let mut rows = div()
            .id("app-template-review-lines")
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
                            .child(if line.command {
                                t("Commands")
                            } else {
                                t("Files")
                            }),
                    )
                    .child(div().text_size(px(TEXT_12_PX)).child(line.text.clone())),
            );
        }
        panel = panel.child(rows);
        if pages > 1 {
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .child(
                        button(
                            "app-template-review-previous",
                            "chevron-left",
                            t("Previous"),
                            false,
                            palette,
                        )
                        .focusable()
                        .tab_index(0)
                        .focus(|style| style.opacity(ALPHA_88))
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, _, cx| {
                                if let Some(review) = &mut this.app_templates.review {
                                    review.page = review.page.saturating_sub(1);
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
                            "chevron-right",
                            t("Next"),
                            false,
                            palette,
                        )
                        .focusable()
                        .tab_index(0)
                        .focus(|style| style.opacity(ALPHA_88))
                        .on_click(cx.listener(
                            move |this, _: &ClickEvent, _, cx| {
                                if let Some(review) = &mut this.app_templates.review {
                                    review.page = (review.page + 1).min(pages - 1);
                                }
                                cx.notify();
                            },
                        )),
                    ),
            );
        }
        panel
            .child(
                div()
                    .flex()
                    .gap(px(SPACE_8_PX))
                    .child(
                        button(
                            "app-template-review-enable",
                            "check",
                            t("Enable"),
                            true,
                            palette,
                        )
                        .focusable()
                        .tab_index(0)
                        .focus(|style| style.opacity(ALPHA_88))
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
                        .focusable()
                        .tab_index(0)
                        .focus(|style| style.opacity(ALPHA_88))
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
    fn every_destination_and_command_is_reachable_on_bounded_review_pages() {
        let count = ROWS_PER_PAGE * 2 + 1;
        let visible = (0..count.div_ceil(ROWS_PER_PAGE))
            .flat_map(|page| page_range(page, count))
            .collect::<Vec<_>>();
        assert_eq!(visible, (0..count).collect::<Vec<_>>());
        assert_eq!(page_range(usize::MAX, count), count - 1..count);
        assert_eq!(page_range(0, 0), 0..0);
    }
}
