//! On-demand text editing at a desktop widget's painted position.

mod draft;
mod continuation;
pub(crate) mod placement;
mod store;

use std::rc::Rc;

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Global, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, StatefulInteractiveElement, Styled, Task,
    Window, WindowHandle, WindowKind, div, prelude::FluentBuilder as _, px,
};
use mandara_config::desktop_widgets::WidgetKind;
use mandara_core::{SurfaceRole, ToGpui};
use mandara_i18n::{t, tf};

use crate::backdrop::widgets::actions::Action;
use crate::bidi::Bidi as _;
use crate::kit::{card, text, tokens};
use crate::productivity::productivity;
use crate::state::{config, update_config};
use crate::text_field::TextField;

use draft::{Draft, SaveError};
use placement::Placement;
use store::Target;

/// A persistent rim makes both keyboard actions visible on a filled card.
const FOCUS_RIM_PX: f32 = 2.0;
/// The shipped cross makes the cancel action visible beside Done.
const CANCEL_ICON: &str = "close";

#[derive(Default)]
struct Open(Option<WindowHandle<Editor>>);
impl Global for Open {}

/// A deferred desktop action survives failed saves and runs only after success.
pub(crate) fn after_save(cx: &mut App, action: impl FnOnce(&mut App) + 'static) {
    let mut action = Some(action);
    if let Some(handle) = cx.try_global::<Open>().and_then(|open| open.0)
        && handle.update(cx, |editor, window, cx| {
            editor.next.defer(Box::new(action.take().unwrap()));
            editor.finish(window, cx);
        }).is_ok() { return; }
    if let Some(action) = action { action(cx); }
}

/// Command callers see the retained validation/IO error, or the pending save status.
pub(crate) fn save_error(cx: &App) -> String {
    cx.try_global::<Open>().and_then(|open| open.0).and_then(|handle| handle.read(cx).ok())
        .and_then(|editor| editor.error.clone()).unwrap_or_else(|| t("Saving…").to_owned())
}

/// A window exists before Wayland grants its keyboard focus. `None` means closed.
pub(crate) fn input_ready(cx: &mut App) -> Option<bool> {
    let handle = cx.try_global::<Open>()?.0?;
    handle.update(cx, |editor, window, cx| {
        receives_keys(window.is_window_active(), [editor.field.read(cx).is_focused(window),
            editor.save_focus.is_focused(window), editor.cancel_focus.is_focused(window)])
    }).ok()
}

/// Whether the editor takes keys: its window is active and one of its
/// controls (field, Save, Cancel) has focus.
fn receives_keys(active: bool, controls: [bool; 3]) -> bool {
    active && controls.into_iter().any(|focused| focused)
}

/// Save and release the current editor. A conflict or write error retains it.
pub(crate) fn finish(cx: &mut App) -> bool {
    let Some(handle) = cx.try_global::<Open>().and_then(|open| open.0) else {
        return true;
    };
    handle.update(cx, |editor, window, cx| editor.finish(window, cx)).unwrap_or_else(|_| {
        cx.set_global(Open::default());
        crate::backdrop::text_editing(false, cx);
        true
    })
}

/// Open from the renderer's actual hit geometry, including the clicked output.
pub(crate) fn open(widget: String, action: Action, placement: Placement, cx: &mut App) {
    if crate::lock::is_locked() || crate::desktop_edit::active(cx) { return; }
    after_save(cx, move |cx| open_new(widget, action, placement, cx));
}

/// Opens an editor for the widget's task or note over its hit geometry,
/// once any previous editor has saved. Does nothing while locked, while
/// arranging widgets, or when the widget or its item is gone.
fn open_new(widget: String, action: Action, placement: Placement, cx: &mut App) {
    if crate::lock::is_locked() || crate::desktop_edit::active(cx) { return; }
    let cfg = config(cx).read(cx).snapshot();
    let Some(definition) = cfg.desktop.widgets.get(&widget).filter(|definition| definition.enabled && !cfg.desktop.widgets.hidden && crate::desktop_plugins::shows(definition, &cfg, cx)) else {
        return;
    };
    let p = productivity(cx);
    let target = {
        let model = p.read(cx);
        let target = match (&definition.kind, action) {
            (WidgetKind::Todo(todo), Action::TodoAdd) => Target::Task { id: None, list: model.book.find_list(&todo.list).unwrap_or(crate::todo_lists::INBOX_ID) },
            (WidgetKind::Todo(_), Action::TodoEdit(id)) => {
                let Some(task) = model.book.todos.iter().find(|task| task.id == id) else {
                    return;
                };
                Target::Task { id: Some(id), list: task.list }
            }
            (WidgetKind::Note(_) | WidgetKind::Notebook(_), Action::Note(title)) => {
                let note = model.notes.notes.iter().filter(|note| !note.text.trim().is_empty()).find(|note| title.is_empty() || note.title().eq_ignore_ascii_case(&title));
                Target::Note { id: note.map(|note| note.id), topic: note.map_or(crate::notes_store::DEFAULT_TOPIC, |note| note.topic) }
            }
            _ => return,
        };
        if !target.ready(model) { return; }
        target
    };
    let Some([x, y, width, height]) = placement.editor_bounds(target.multiline()) else {
        return;
    };
    crate::desktop_menu::close(cx);
    let original = target.text(p.read(cx)).map(str::to_owned);
    let note_title = match target {
        Target::Note { id: Some(id), .. } => p.read(cx).notes.note(id).map(|note| note.title()),
        _ => None,
    };
    let mut options = crate::plugins::dock::layer_options(
        SurfaceRole::Desktop,
        Layer::Top,
        Anchor::TOP | Anchor::LEFT,
        (width, height),
        Some(crate::desktop_menu::IGNORE_ZONES),
        KeyboardInteractivity::Exclusive,
        crate::outputs::display_for(&placement.output, cx),
    );
    if let WindowKind::LayerShell(layer) = &mut options.kind {
        layer.margin = Some((px(y), gpui::Pixels::ZERO, gpui::Pixels::ZERO, px(x)));
    }
    match cx.open_window(options, move |window, cx| cx.new(|cx| Editor::new(widget, target, Draft::new(original), note_title, window, cx))) {
        Ok(handle) => {
            cx.set_global(Open(Some(handle)));
            crate::backdrop::text_editing(true, cx);
        }
        Err(error) => tracing::warn!("opening desktop text editor: {error:#}"),
    }
}

/// One open desktop text editor: its target, the draft with the text it
/// opened, and the save in flight. One at most, held in `Open`.
struct Editor {
    widget: String,
    target: Target,
    draft: Draft,
    note_title: Option<String>,
    field: Entity<TextField>,
    save_focus: FocusHandle,
    cancel_focus: FocusHandle,
    error: Option<String>,
    settle: Option<Task<()>>,
    saving: bool,
    notes_root: Option<std::path::PathBuf>,
    next: continuation::Continuation<Box<dyn FnOnce(&mut App)>>,
}

impl Editor {
    /// Builds the field and saves (then closes) when the widget is hidden,
    /// the editor loses activation for longer than the settle time, or
    /// another window takes focus.
    fn new(widget: String, target: Target, draft: Draft, note_title: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let weak = cx.weak_entity();
        let value = draft.text().to_owned();
        let field = cx.new(|cx| {
            let field = TextField::new(
                value,
                if target.multiline() { t("Write in Markdown…") } else { t("Add a task…") },
                false,
                Rc::new(move |_, window, cx| {
                    let weak = weak.clone();
                    window.defer(cx, move |window, cx| {
                        let _ = weak.update(cx, |editor, cx| editor.finish(window, cx));
                    });
                }),
                cx,
            );
            let field = field.parent_commits();
            if target.multiline() { field.multiline().markdown() } else { field }
        });
        field.update(cx, |field, cx| field.focus(window, cx));
        cx.observe(&field, |_, _, cx| cx.notify()).detach();
        cx.observe_in(&config(cx), window, |editor, store, window, cx| {
            let hidden = {
                let cfg = store.read(cx).get();
                cfg.desktop.widgets.hidden || cfg.desktop.widgets.get(&editor.widget).is_none_or(|widget| {
                    !widget.enabled || !crate::desktop_plugins::shows(widget, cfg, cx) || !matches!((&widget.kind, editor.target),
                        (WidgetKind::Todo(_), Target::Task { .. }) | (WidgetKind::Note(_) | WidgetKind::Notebook(_), Target::Note { .. }))
                })
            };
            if hidden {
                editor.finish(window, cx);
            }
        })
        .detach();
        cx.observe_window_activation(window, |editor: &mut Self, window, cx| {
            if window.is_window_active() {
                editor.settle = None;
                return;
            }
            editor.settle = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(crate::dismiss::DEACTIVATE_SETTLE).await;
                let _ = this.update_in(cx, |editor, window, cx| {
                    editor.settle = None;
                    if !window.is_window_active() {
                        editor.finish(window, cx);
                    }
                });
            }));
        })
        .detach();
        let hub = crate::hub::hub(cx);
        let mut last = hub.read(cx).focused.as_ref().map(|client| client.id);
        cx.observe_in(&hub, window, move |editor, hub, window, cx| {
            let current = hub.read(cx).focused.as_ref().map(|client| client.id);
            let outside = current.is_some() && current != last;
            last = current;
            if outside {
                editor.finish(window, cx);
            }
        })
        .detach();
        let notes_root = target.multiline().then(|| productivity(cx).read(cx).notes.root.clone());
        Self { widget, target, draft, note_title, field, save_focus: cx.focus_handle(), cancel_focus: cx.focus_handle(), error: None, settle: None, saving: false, notes_root, next: continuation::Continuation::new() }
    }

    /// Saves the draft (checked against the text it opened, on the worker)
    /// and closes on success. Returns `true` only when nothing needed saving;
    /// otherwise the result arrives later and a conflict keeps the editor
    /// open with its text.
    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.saving { return false; }
        let value = self.field.read(cx).value.clone();
        let current = productivity(cx);
        let retry = self.draft.retrying();
        let checked = self.draft.prepare(&value, self.target.text(current.read(cx)));
        match checked {
            Ok(false) => { self.finish_saved(window, cx); return true; }
            Err(error) => { self.show_error(error, cx); return false; }
            Ok(true) => {}
        }
        let original = self.draft.original().map(str::to_owned);
        let stored = match self.target {
            Target::Task { id: None, .. } => crate::todo_lists::parse_quick_add(&value).1.to_owned(),
            Target::Task { .. } => value.trim().to_owned(),
            Target::Note { .. } => value.clone(),
        };
        let pending = current.update(cx, |model, cx| store::write(model, cx, self.target, value.clone(), original, retry, self.notes_root.as_deref()));
        let pending = match pending {
            Ok(pending) => pending,
            Err(error) => { self.show_error(error, cx); return false; }
        };
        self.saving = true;
        self.error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let done = pending.recv().await;
            let _ = this.update_in(cx, |editor, window, cx| {
                editor.saving = false;
                match done {
                    Ok(done) => {
                        editor.target = done.target;
                        if let Err(error) = done.result { editor.show_error(error, cx); return; }
                        editor.draft.committed(&stored);
                        match editor.next.after_write(editor.field.read(cx).value != value) {
                            continuation::AfterWrite::KeepEditing => { cx.notify(); return; }
                            continuation::AfterWrite::SaveAgain => { editor.finish(window, cx); return; }
                            continuation::AfterWrite::Finish => {}
                        }
                        // A later writer may have changed the same task while disk IO ran.
                        let model = productivity(cx);
                        if !stored.trim().is_empty() && editor.target.text(model.read(cx)) != Some(stored.as_str()) {
                            editor.show_error(SaveError::Changed, cx);
                            return;
                        }
                        editor.finish_saved(window, cx);
                    }
                    Err(error) => editor.show_error(SaveError::Store(error.to_string()), cx),
                }
            });
        }).detach();
        false
    }

    /// Keeps the editor open with the error, cancelling any queued follow-up.
    fn show_error(&mut self, error: SaveError<String>, cx: &mut Context<Self>) {
        let _ = self.next.complete(false);
        self.error = Some(match error {
            SaveError::Changed => t("This item changed elsewhere. Your text is still here.").into(),
            SaveError::Removed => t("This item was removed. Your text is still here.").into(),
            SaveError::Store(error) => tf("Couldn't save: {error}", &[("error", &error)]),
        });
        cx.notify();
    }

    /// After a save: a note widget that named the note follows its new
    /// title, the editor closes and a queued follow-up runs.
    fn finish_saved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Target::Note { id: Some(id), .. } = self.target {
            let title = productivity(cx).read(cx).notes.note(id).map(|note| note.title());
            if let Some(title) = title.filter(|title| self.note_title.as_ref() != Some(title)) {
                update_config(cx, |config| {
                    if let Some(widget) = config.desktop.widgets.get_mut(&self.widget)
                        && let WidgetKind::Note(note) = &mut widget.kind
                        && !note.note.is_empty()
                        && self.note_title.as_ref().is_none_or(|old| note.note.eq_ignore_ascii_case(old))
                    { note.note = title; }
                });
            }
        }
        let next = self.next.complete(true);
        self.close(window, cx);
        if let Some(action) = next { action(cx); }
    }

    /// Removes the editor window without saving and clears the open-editor
    /// state if it is still this one.
    fn close(&self, window: &mut Window, cx: &mut App) {
        if cx.try_global::<Open>().and_then(|open| open.0).is_some_and(|handle| gpui::AnyWindowHandle::from(handle) == window.window_handle()) {
            cx.set_global(Open::default());
            crate::backdrop::text_editing(false, cx);
        }
        window.remove_window();
    }

    /// Tab cycles field, Save and Cancel; Escape cancels; Ctrl+Enter (or
    /// Enter in a one-line task) and Enter/Space on Save save.
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        if key.key == "tab" && !key.modifiers.control && !key.modifiers.alt && !key.modifiers.platform {
            let current = if self.field.read(cx).is_focused(window) {
                0
            } else if self.save_focus.is_focused(window) {
                1
            } else {
                2
            };
            match (current + if key.modifiers.shift { 2 } else { 1 }) % 3 {
                0 => self.field.update(cx, |field, cx| field.focus(window, cx)),
                1 => self.save_focus.focus(window, cx),
                _ => self.cancel_focus.focus(window, cx),
            }
            cx.stop_propagation();
        } else if key.key == "escape" || (self.cancel_focus.is_focused(window) && matches!(key.key.as_str(), "enter" | "space")) {
            cx.stop_propagation();
            self.close(window, cx);
        } else if (key.key == "enter" && (key.modifiers.control || (!self.target.multiline() && self.field.read(cx).is_focused(window))))
            || (self.save_focus.is_focused(window) && matches!(key.key.as_str(), "enter" | "space"))
        {
            cx.stop_propagation();
            self.finish(window, cx);
        }
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = mandara_core::frame_palette(window, cx);
        card(&palette)
            .id("desktop-text-editor")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(tokens::SM))
            .font_family(crate::fonts::fonts(cx).ui)
            .text_size(px(text::BODY))
            .text_color(palette.on_surface.hsla())
            .capture_key_down(cx.listener(Self::key))
            .child(
                div().row().items_center().justify_between().child(crate::kit::title(if self.target.multiline() { t("Notes") } else { t("To-do") }, &palette)).child(
                    div()
                        .row()
                        .items_center()
                        .gap(px(tokens::SM))
                        .child(
                            div()
                                .id("desktop-text-cancel")
                                .track_focus(&self.cancel_focus)
                                .tab_index(0)
                                .role(gpui::accesskit::Role::Button)
                                .aria_label(t("Cancel"))
                                .cursor_pointer()
                                .border(px(FOCUS_RIM_PX))
                                .border_color(if self.cancel_focus.is_focused(window) { palette.primary.hsla() } else { palette.surface_container.hsla() })
                                .rounded(px(tokens::SM))
                                .p(px(tokens::SM))
                                .child(crate::icons::icon(CANCEL_ICON, tokens::LG, palette.on_surface))
                                .on_click(cx.listener(|editor, _, window, cx| editor.close(window, cx))),
                        )
                        .child(
                            div()
                                .id("desktop-text-save")
                                .track_focus(&self.save_focus)
                                .tab_index(0)
                                .role(gpui::accesskit::Role::Button)
                                .aria_label(t("Done"))
                                .cursor_pointer()
                                .border(px(FOCUS_RIM_PX))
                                .border_color(if self.save_focus.is_focused(window) { palette.on_surface.hsla() } else { palette.primary.hsla() })
                                .rounded(px(tokens::SM))
                                .px(px(tokens::MD))
                                .py(px(tokens::SM))
                                .bg(palette.primary.hsla())
                                .text_color(palette.on_primary.hsla())
                                .child(if self.saving { t("Saving…") } else { t("Done") })
                                .on_click(cx.listener(|editor, _, window, cx| {
                                    editor.finish(window, cx);
                                })),
                        ),
                ),
            )
            .child(div().id("desktop-text-scroll").flex_1().min_h_0().overflow_y_scroll().child(self.field.clone()))
            .when_some(self.error.as_ref(), |element, error| element.child(div().text_color(palette.urgent_coral.hsla()).child(error.clone())))
    }
}

#[cfg(test)]
mod tests {
    use super::{CANCEL_ICON, receives_keys};
    use gpui::AssetSource as _;

    #[test]
    fn the_cancel_control_uses_a_bundled_drawable_icon() {
        assert!(crate::icons::MandaraAssets.load(&format!("icons/{CANCEL_ICON}.svg")).unwrap().is_some());
    }

    #[test]
    fn keyboard_readiness_wire_contract_requires_both_os_and_editor_focus() {
        use serde_json::{Value, json};
        for (state, expected) in [
            (None, Value::Null),
            (Some((false, [true, false, false])), json!(false)),
            (Some((true, [true, false, false])), json!(true)),
            (Some((true, [false, true, false])), json!(true)),
            (Some((true, [false, false, true])), json!(true)),
            (Some((true, [false, false, false])), json!(false)),
        ] {
            assert_eq!(json!(state.map(|(active, controls)| receives_keys(active, controls))), expected);
        }
    }
}
