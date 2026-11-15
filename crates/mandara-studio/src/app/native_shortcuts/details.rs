//! The selected binding's fields, independently scrollable beside the list.
use super::*;

impl StudioView {
    pub(super) fn native_details(&self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        if let Some(draft) = &self.native_shortcuts.draft {
            let mut header = div()
                .w_full()
                .min_w(px(0.0))
                .row()
                .flex_col()
                .gap(px(SPACE_18_PX))
                .child(div().text_size(px(TEXT_18_PX)).child(t("Shortcut")))
                .child(
                    div()
                        .text_size(px(TEXT_12_PX))
                        .text_color(p.on_surface_variant.hsla())
                        .whitespace_normal()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(tf(
                            "{file}, line {line}",
                            &[
                                ("file", &draft.id.file.display()),
                                ("line", &draft.binding.line),
                            ],
                        )),
                );
            if draft.dirty(cx) {
                header = header.child(
                    div()
                        .text_size(px(TEXT_12_PX))
                        .text_color(p.on_surface_variant.hsla())
                        .child(t(
                            "Save or cancel this edit before selecting another shortcut.",
                        )),
                );
            }
            if let Some(reason) = draft.binding.read_only {
                header = header.child(tr(reason));
            }
            let mut fields = div()
                .id("native-field-scroll")
                .w_full()
                .min_w(px(0.0))
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .track_scroll(&self.native_shortcuts.detail_scroll)
                .row()
                .flex_col()
                .gap(px(SPACE_16_PX));
            for (field, input) in draft.binding.fields.iter().zip(&draft.fields) {
                let label = field.name.as_deref().map_or_else(
                    || tr(field.label),
                    |name| format!("{} · {name}", tr(field.label)),
                );
                let mut row = div()
                    .w_full()
                    .min_w(px(0.0))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_8_PX))
                    .child(
                        div()
                            .text_size(px(TEXT_12_PX))
                            .text_color(p.on_surface_variant.hsla())
                            .child(label),
                    );
                if let Some(input) = input
                    && !self.native_shortcuts.busy
                {
                    row = row.child(input.clone());
                } else if let Some(input) = input {
                    row = row.child(input.read(cx).value.clone());
                } else {
                    row = row.child(
                        div()
                            .text_color(p.on_surface_variant.hsla())
                            .whitespace_normal()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(field.value.clone()),
                    );
                }
                if let Some(reason) = field.read_only {
                    row = row.child(
                        div()
                            .text_size(px(TEXT_12_PX))
                            .text_color(p.on_surface_variant.hsla())
                            .child(tr(reason)),
                    );
                }
                fields = fields.child(row);
            }
            let dirty = draft.dirty(cx);
            let footer = div()
                .row()
                .flex_wrap()
                .pt(px(SPACE_16_PX))
                .border_t_1()
                .border_color(p.outline.with_alpha(ALPHA_20).hsla())
                .gap(px(SPACE_10_PX))
                .when(dirty && !self.native_shortcuts.busy, |el| {
                    el.child(
                        button("native-save", "check", t("Save"), true, p)
                            .on_click(cx.listener(|view, _, _, cx| view.native_save(false, cx))),
                    )
                })
                .when(!self.native_shortcuts.busy, |el| {
                    el.child(
                        button("native-cancel", "close", t("Cancel"), false, p).on_click(
                            cx.listener(|view, _, _, cx| {
                                view.native_shortcuts.draft = None;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        button(
                            "native-discard-refresh",
                            "refresh",
                            t("Discard and refresh"),
                            false,
                            p,
                        )
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.native_shortcuts.draft = None;
                            view.native_shortcuts.message = None;
                            view.native_shortcuts.requests.refresh();
                            cx.notify();
                        })),
                    )
                });
            return div()
                .h_full()
                .min_h(px(0.0))
                .row()
                .flex_col()
                .gap(px(SPACE_16_PX))
                .child(header.flex_shrink_0())
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h(px(0.0))
                        .child(fields.h_full())
                        .child(mandara_ui::scrollbar::overlay_scrollbar(
                            &self.native_shortcuts.detail_scroll,
                            &mandara_ui::scrollbar::keyed_state("native-shortcut-details"),
                            p,
                        )),
                )
                .child(footer.flex_shrink_0())
                .into_any_element();
        }
        div()
            .flex_1()
            .row()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(SPACE_14_PX))
            .text_color(p.on_surface_variant.hsla())
            .child(icon("keyboard", ICON_26_PX, p.primary))
            .child(
                div()
                    .whitespace_normal()
                    .child(t("Select a shortcut to edit its values.")),
            )
            .into_any_element()
    }
}
