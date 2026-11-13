//! Native shortcut list, keyboard navigation and field editor.

use super::*;

impl StudioView {
    pub(super) fn compositor_shortcuts(
        &mut self,
        cfg: &ShellConfig,
        p: &MandaraPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if window.is_visible() {
            self.native_shortcuts.visible = true;
            self.native_scan(cfg, cx);
        }
        if self.native_shortcuts.search.is_none() {
            let view = cx.entity().downgrade();
            self.native_shortcuts.search = Some(cx.new(|cx| {
                TextField::new(
                    self.native_shortcuts.query.clone(),
                    t("Search shortcuts"),
                    true,
                    Rc::new(move |text, _, cx| {
                        let _ = view.update(cx, |view, cx| {
                            view.native_shortcuts.query = text;
                            view.native_shortcuts.refilter();
                            cx.notify();
                        });
                    }),
                    cx,
                )
            }));
        }
        if self.native_shortcuts.list_focus.is_none() {
            self.native_shortcuts.list_focus = Some(cx.focus_handle().tab_index(0).tab_stop(true));
        }
        let state = &self.native_shortcuts;
        let mut content = div().w_full().min_w(px(0.0)).row().flex_col().gap(px(SPACE_14_PX))
            .child(div().whitespace_normal().text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Edit individual values in your compositor's shortcuts. Save keeps a backup, checks the configuration and applies the change. Computed bindings stay read-only."))))
            .child(div().w_full().min_w(px(0.0)).row().flex_wrap().items_center().gap(px(SPACE_10_PX))
                .child(div().flex_1().min_w(px(DIM_200_PX)).child(state.search.as_ref().unwrap().clone()))
                .child(button("native-refresh", "refresh", t("Refresh"), false, p).on_click(cx.listener(|view, _, _, cx| { if !view.native_shortcuts.busy { view.native_shortcuts.requests.refresh(); let cfg = mandara_ui::state::config(cx).read(cx).snapshot(); view.native_scan(&cfg, cx); cx.notify(); } })))
                .when(state.undo.is_some(), |el| el.child(button("native-undo", "undo", t("Undo"), false, p).on_click(cx.listener(|view, _, _, cx| view.native_save(true, cx))))));
        if let Some((success, message)) = &state.message {
            content = content.child(
                div()
                    .p(px(SPACE_12_PX))
                    .rounded(px(RADIUS_12_PX))
                    .bg(if *success {
                        p.success_emerald
                    } else {
                        p.urgent_coral
                    }
                    .with_alpha(ALPHA_10)
                    .hsla())
                    .child(message.clone()),
            );
        }
        if state.busy {
            content = content.child(t("Saving shortcuts…"));
        }
        if state.loading {
            content = content.child(t("Reading compositor shortcuts…"));
        }
        if let Some(data) = state.data.clone() {
            content = content.child(
                div()
                    .text_size(px(TEXT_12_PX))
                    .text_color(p.on_surface_variant.hsla())
                    .child(format!(
                        "{} · {}",
                        data.session.target.name(),
                        data.session.root.display()
                    )),
            );
            for (file, error) in &data.session.scan.errors {
                content = content.child(
                    div()
                        .text_color(p.urgent_coral.hsla())
                        .child(format!("{}: {error}", file.display())),
                );
            }
            let filtered = state.filtered.clone();
            let count = filtered.len();
            if count == 0 && !state.loading {
                content = content.child(t("No shortcuts found in your compositor config."));
            }
            if count > 0 {
                let palette = p.clone();
                let view = cx.entity().downgrade();
                let locked = state.draft.is_some() || state.busy;
                let mono = mandara_ui::fonts::fonts(cx).mono.clone();
                let cursor = state.cursor;
                let group_focus = state.list_focus.as_ref().unwrap().clone();
                content = content.child(
                    div()
                        .id("native-shortcut-list")
                        .track_focus(&group_focus)
                        .tab_index(0)
                        .focus(|style| style.border_1().border_color(p.primary.hsla()))
                        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                            group_focus.focus(window, cx)
                        })
                        .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, window, cx| {
                            let key = &event.keystroke;
                            if key.modifiers.control || key.modifiers.alt || key.modifiers.platform
                            {
                                return;
                            }
                            let state = &mut view.native_shortcuts;
                            if state.busy || state.draft.is_some() {
                                return;
                            }
                            if key.key == "enter" || key.key == "return" {
                                if let Some(&index) = state.filtered.get(state.cursor) {
                                    view.native_select(index, window, cx);
                                }
                                cx.stop_propagation();
                            } else if let Some(next) =
                                row_target(state.cursor, state.filtered.len(), &key.key)
                            {
                                state.cursor = next;
                                state
                                    .scroll
                                    .scroll_to_item(next, gpui::ScrollStrategy::Nearest);
                                cx.notify();
                                cx.stop_propagation();
                            }
                        }))
                        .child(
                            uniform_list("native-shortcut-rows", count, move |range, _, _| {
                                range
                                    .map(|at| {
                                        let index = filtered[at];
                                        let row = &data.rows[index];
                                        let binding = &data.session.scan.sources[row.source]
                                            .bindings[row.binding];
                                        let view = view.clone();
                                        let drawn = data.clone();
                                        let p = &palette;
                                        div()
                                            .id(("native-row", index))
                                            .h(px(DIM_64_PX))
                                            .p(px(SPACE_10_PX))
                                            .rounded(px(RADIUS_10_PX))
                                            .row()
                                            .items_center()
                                            .gap(px(SPACE_12_PX))
                                            .cursor_pointer()
                                            .bg(if row.clashes {
                                                p.warning_amber.with_alpha(ALPHA_10)
                                            } else {
                                                p.surface_container
                                            }
                                            .hsla())
                                            .hover(|style| {
                                                style.bg(p.surface_container_high.hsla())
                                            })
                                            .when(at == cursor, |el| {
                                                el.border_1().border_color(p.primary.hsla())
                                            })
                                            .child(
                                                div()
                                                    .min_w(px(DIM_120_PX))
                                                    .max_w(px(DIM_200_PX))
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .text_ellipsis()
                                                    .font_family(mono.clone())
                                                    .text_size(px(TEXT_12_PX))
                                                    .child(row.chord.clone()),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w(px(0.0))
                                                    .row()
                                                    .flex_col()
                                                    .child(
                                                        div()
                                                            .overflow_hidden()
                                                            .whitespace_nowrap()
                                                            .text_ellipsis()
                                                            .child(row.action.clone()),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(TEXT_11_PX))
                                                            .text_color(p.on_surface_variant.hsla())
                                                            .overflow_hidden()
                                                            .whitespace_nowrap()
                                                            .text_ellipsis()
                                                            .child(row.location.clone()),
                                                    ),
                                            )
                                            .when(row.clashes, |el| {
                                                el.child(icon(
                                                    "warning",
                                                    ICON_15_PX,
                                                    p.warning_amber,
                                                ))
                                            })
                                            .when(binding.read_only.is_some(), |el| {
                                                el.child(icon(
                                                    "lock",
                                                    ICON_14_PX,
                                                    p.on_surface_variant,
                                                ))
                                            })
                                            .on_click(move |_, window, cx| {
                                                if !locked {
                                                    let _ = view.update(cx, |view, cx| {
                                                        if view
                                                            .native_shortcuts
                                                            .data
                                                            .as_ref()
                                                            .is_some_and(|current| {
                                                                Rc::ptr_eq(current, &drawn)
                                                            })
                                                        {
                                                            view.native_select(index, window, cx)
                                                        }
                                                    });
                                                }
                                            })
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .with_sizing_behavior(ListSizingBehavior::Infer)
                            .track_scroll(&state.scroll)
                            .max_h(px(LIST_HEIGHT)),
                        ),
                );
                content = content.child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("A warning marks a shortcut also used by Mandara. Check its mode and flags before changing it.")));
            }
        }
        if let Some(draft) = &self.native_shortcuts.draft {
            let mut fields = div().row().flex_col().gap(px(SPACE_12_PX)).child(
                div()
                    .text_size(px(TEXT_12_PX))
                    .text_color(p.on_surface_variant.hsla())
                    .child(tf(
                        "{file}, line {line}",
                        &[
                            ("file", &draft.id.file.display()),
                            ("line", &draft.binding.line),
                        ],
                    )),
            );
            fields = fields.child(
                div()
                    .text_size(px(TEXT_12_PX))
                    .text_color(p.on_surface_variant.hsla())
                    .child(t(
                        "Save or cancel this edit before selecting another shortcut.",
                    )),
            );
            if let Some(reason) = draft.binding.read_only {
                fields = fields.child(tr(reason));
            }
            for (field, input) in draft.binding.fields.iter().zip(&draft.fields) {
                let label = field.name.as_deref().map_or_else(
                    || tr(field.label),
                    |name| format!("{} · {name}", tr(field.label)),
                );
                let mut row = div().row().flex_col().gap(px(SPACE_6_PX)).child(label);
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
            let dirty = draft
                .fields
                .iter()
                .zip(&draft.binding.fields)
                .any(|(input, field)| {
                    input
                        .as_ref()
                        .is_some_and(|input| input.read(cx).value != field.value)
                });
            fields = fields.child(
                div()
                    .row()
                    .flex_wrap()
                    .gap(px(SPACE_10_PX))
                    .when(dirty && !self.native_shortcuts.busy, |el| {
                        el.child(
                            button("native-save", "check", t("Save"), true, p).on_click(
                                cx.listener(|view, _, _, cx| view.native_save(false, cx)),
                            ),
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
                    }),
            );
            content = content.child(fields);
        }
        content.into_any_element()
    }
}
