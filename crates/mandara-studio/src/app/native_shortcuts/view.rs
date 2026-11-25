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
        let details = self.native_details(p, cx);
        let layout = workspace_layout(self.page.column_w, self.native_shortcuts.draft.is_some());
        let split = layout == WorkspaceLayout::Split;
        let state = &self.native_shortcuts;
        let mut content = div().w_full().flex_1().min_h(px(0.0)).min_w(px(0.0)).row().flex_col().gap(px(SPACE_14_PX))
            .child(div().w_full().min_w(px(0.0)).row().flex_wrap().items_center().gap(px(SPACE_10_PX))
                .child(div().flex_1().min_w(px(DIM_200_PX)).child(state.search.as_ref().unwrap().clone()))
                .child(div().id("native-help").p(px(SPACE_10_PX)).tooltip(compositor_page::tip(t("Edit individual values in your compositor's shortcuts. Save keeps a backup, checks the configuration and applies the change. Computed bindings stay read-only."), p)).child(icon("info", ICON_17_PX, p.on_surface_variant)))
                .child(button("native-refresh", "refresh", t("Refresh"), false, p).on_click(cx.listener(|view, _, _, cx| { if !view.native_shortcuts.busy { view.native_shortcuts.requests.refresh(); let cfg = mandara_ui::state::config(cx).read(cx).snapshot(); view.native_scan(&cfg, cx); cx.notify(); } })))
                .when(state.undo.is_some(), |el| {
                    let enabled = state.available_undo().is_some();
                    let reason = if state.draft.is_some() { t("Save or cancel this edit before using Undo.") } else { t("Saving shortcuts…") };
                    el.child(button("native-undo", "undo", t("Undo"), false, p)
                        .tab_stop(enabled)
                        .when(!enabled, |button| button.opacity(ALPHA_40).aria_description(reason))
                        .tooltip(compositor_page::tip(if enabled { t("Undo") } else { reason }, p))
                        .on_click(cx.listener(|view, _, _, cx| view.native_save(true, cx))))
                }));
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
        let mut workspace = div()
            .w_full()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .row()
            .gap(px(SPACE_18_PX))
            .when(!split, |el| el.flex_col());
        if let Some(data) = state.data.clone() {
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
            content = content.child(
                div()
                    .w_full()
                    .min_w(px(0.0))
                    .row()
                    .items_center()
                    .justify_between()
                    .gap(px(SPACE_14_PX))
                    .text_size(px(TEXT_12_PX))
                    .text_color(p.on_surface_variant.hsla())
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(data.source_label.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .child(tf("{count} shortcuts", &[("count", &count)])),
                    ),
            );
            if count > 0 && layout != WorkspaceLayout::Editor {
                let palette = p.clone();
                let view = cx.entity().downgrade();
                let locked =
                    state.draft.as_ref().is_some_and(|draft| draft.dirty(cx)) || state.busy;
                let mono = mandara_ui::fonts::fonts(cx).mono.clone();
                let cursor = state.cursor;
                let selected = state.draft.is_some()
                    || state
                        .list_focus
                        .as_ref()
                        .is_some_and(|focus| focus.is_focused(window));
                let group_focus = state.list_focus.as_ref().unwrap().clone();
                workspace = workspace.child(
                    div()
                        .id("native-shortcut-list")
                        .relative()
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_hidden()
                        .p(px(SPACE_12_PX))
                        .rounded(px(RADIUS_14_PX))
                        .border_1()
                        .border_color(p.outline.hsla())
                        .bg(p.surface_container.hsla())
                        .w_full()
                        .min_w(px(0.0))
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
                            if state.busy
                                || state.draft.as_ref().is_some_and(|draft| draft.dirty(cx))
                            {
                                return;
                            }
                            if key.key == "enter" || key.key == "return" {
                                if let Some(&index) = state.filtered.get(state.cursor) {
                                    view.native_select(index, window, cx);
                                }
                                cx.stop_propagation();
                            } else if let Some(next) = row_target(
                                state.cursor,
                                state.filtered.len(),
                                (f32::from(
                                    state.scroll.0.borrow().base_handle.bounds().size.height,
                                ) / ROW_HEIGHT)
                                    .floor() as usize,
                                &key.key,
                            ) {
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
                                            .w_full()
                                            .h(px(ROW_HEIGHT))
                                            .p(px(SPACE_14_PX))
                                            .rounded(px(RADIUS_10_PX))
                                            .border_b_1()
                                            .border_color(p.outline.with_alpha(ALPHA_20).hsla())
                                            .row()
                                            .items_center()
                                            .gap(px(SPACE_16_PX))
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
                                            .when(selected && at == cursor, |el| {
                                                el.border_1().border_color(p.primary.hsla())
                                            })
                                            .child(
                                                div()
                                                    .min_w(px(DIM_120_PX))
                                                    .w(px(DIM_150_PX))
                                                    .flex_shrink_0()
                                                    .max_w(px(DIM_200_PX))
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .text_ellipsis()
                                                    .px(px(SPACE_10_PX))
                                                    .py(px(SPACE_6_PX))
                                                    .rounded(px(RADIUS_6_PX))
                                                    .bg(p.surface_container_high.hsla())
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
                                                el.child(
                                                    div()
                                                        .id(("native-clash", index))
                                                        .tooltip(compositor_page::tip(
                                                            t("Also used by Mandara"),
                                                            p,
                                                        ))
                                                        .child(icon(
                                                            "warning",
                                                            ICON_15_PX,
                                                            p.warning_amber,
                                                        )),
                                                )
                                            })
                                            .when(binding.read_only.is_some(), |el| {
                                                el.child(
                                                    div()
                                                        .id(("native-readonly", index))
                                                        .tooltip(compositor_page::tip(
                                                            tr(binding.read_only.unwrap()),
                                                            p,
                                                        ))
                                                        .child(icon(
                                                            "lock",
                                                            ICON_14_PX,
                                                            p.on_surface_variant,
                                                        )),
                                                )
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
                            .w_full()
                            .with_sizing_behavior(ListSizingBehavior::Auto)
                            .h_full()
                            .min_h(px(0.0))
                            .track_scroll(&state.scroll),
                        )
                        .child(mandara_ui::scrollbar::overlay_scrollbar(
                            &state.scroll.0.borrow().base_handle,
                            &mandara_ui::scrollbar::keyed_state("native-shortcuts"),
                            p,
                        )),
                );
            }
        }
        let details = div()
            .id("native-details")
            .min_h(px(0.0))
            .overflow_hidden()
            .row()
            .flex_col()
            .p(px(SPACE_24_PX))
            .rounded(px(RADIUS_14_PX))
            .bg(p.surface_container.hsla())
            .when(split, |el| el.w(px(DETAILS_WIDTH)).h_full().flex_shrink_0())
            .when(!split, |el| el.flex_1().h_full().w_full())
            .child(details);
        if layout != WorkspaceLayout::List {
            workspace = workspace.child(details);
        }
        content.child(workspace).into_any_element()
    }
}
