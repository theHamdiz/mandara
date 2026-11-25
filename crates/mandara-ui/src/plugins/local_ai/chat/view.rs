//! Chat presentation: a compact toolbar, reading space and one composer.

use super::{Shared, State, WIDTH, clear, following_tail, load_catalog, model, notify, submit};
use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::{alpha, text, tokens};
use crate::plugins::panel_fx::{glass_card, readable};
use gpui::{
    AnyElement, App, ClipboardItem, Div, ElementId, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, SharedString, Stateful, StatefulInteractiveElement, Styled,
    Window, div, prelude::FluentBuilder as _, px,
};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_services::local_ai::chat::{Provider, Role, Settings};
use mandara_theme::MandaraPalette;
use std::sync::Arc;

/// Toolbar targets leave enough room for translated labels.
const CONTROL_H: f32 = 32.0;
const CONTROL_ICON: f32 = 14.0;
/// The composer keeps its actions outside the text's scroll region.
const COMPOSER_TEXT_H: f32 = 100.0;
/// A long catalog scrolls over the conversation rather than squeezing it.
const CATALOG_H: f32 = 220.0;
const CATALOG_TOP: f32 = CONTROL_H * 2.0 + tokens::LG + tokens::SM * 2.0;
/// A user message leaves a readable gutter beside the assistant's replies.
const MESSAGE_GUTTER: f32 = 56.0;
const MESSAGE_LINE_H: f32 = 20.0;
/// The empty-state mark is static; it holds no images or animation task.
const MARK_SIDE: f32 = 64.0;
const MARK_GLYPH: f32 = 28.0;
const INTRO_W: f32 = 340.0;
const INTRO_LINE_H: f32 = 32.0;
const DISABLED_OPACITY: f32 = 0.45;
/// Search reaches every model; only a small menu is built in one frame.
const VISIBLE_MODELS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComposerAction {
    Disabled,
    Send,
    Stop,
}
impl ComposerAction {
    fn new(busy: bool, prompt: &str, model: &str) -> Self {
        if busy {
            Self::Stop
        } else if prompt.trim().is_empty() || model.is_empty() {
            Self::Disabled
        } else {
            Self::Send
        }
    }
}

fn action(
    id: impl Into<ElementId>,
    glyph: &'static str,
    label: impl Into<SharedString>,
    enabled: bool,
    primary: bool,
    p: &MandaraPalette,
    click: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let foreground = if primary {
        p.on_primary
    } else {
        p.on_surface_variant
    };
    let hover = if primary {
        p.primary.mix(p.on_primary, alpha::HOVER)
    } else {
        p.on_surface.with_alpha(alpha::HOVER)
    };
    div()
        .id(id)
        .h(px(CONTROL_H))
        .px(px(tokens::SM))
        .flex_none()
        .rounded(px(crate::style::radius(tokens::MD)))
        .row()
        .items_center()
        .justify_center()
        .gap(px(tokens::XS))
        .text_size(px(text::LABEL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(foreground.hsla())
        .when(primary, |el| el.bg(p.primary.hsla()))
        .when(enabled, |el| {
            el.cursor_pointer()
                .hover(move |style| style.bg(hover.hsla()))
        })
        .when(!enabled, |el| el.opacity(DISABLED_OPACITY))
        .child(icon(glyph, CONTROL_ICON, foreground))
        .child(div().min_w_0().truncate().child(label.into()))
        .focusable()
        .tab_index(0)
        .on_click(move |_, window, cx| {
            if enabled {
                click(window, cx);
            }
        })
}

fn toolbar(state: &State, shared: &Shared, settings: &Settings, p: &MandaraPalette) -> Div {
    let reset = shared.clone();
    let draft = state.input.clone();
    let search = state.search.clone();
    let query = state.query.clone();
    let models = shared.clone();
    let can_manage = settings.provider == Provider::Ollama;
    div()
        .row()
        .items_center()
        .gap(px(tokens::SM))
        .flex_none()
        .child(icon(
            "sparkles",
            CONTROL_H - tokens::SM,
            readable(p.primary, p),
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(text::TITLE))
                .font_weight(FontWeight::SEMIBOLD)
                .truncate()
                .child(t("Local AI")),
        )
        .child(action(
            "ai-chat-reset",
            "plus",
            t("New chat"),
            true,
            false,
            p,
            move |_, cx| {
                clear(&mut reset.borrow_mut());
                for field in [&draft, &search] {
                    field.update(cx, |field, cx| {
                        field.set("");
                        cx.notify();
                    });
                }
                *query.borrow_mut() = String::new();
                notify(&reset, cx);
            },
        ))
        .child(action(
            "ai-models-tab",
            "layers",
            t("Models"),
            can_manage,
            false,
            p,
            move |_, cx| {
                models.borrow_mut().models = true;
                notify(&models, cx);
            },
        ))
        .child(action(
            "ai-chat-settings",
            "sliders",
            t("Settings"),
            true,
            false,
            p,
            |_, cx| {
                crate::state::open_studio_at(Some("plugins/local-ai/settings".into()), cx);
            },
        ))
}

fn model_picker(
    state: &State,
    shared: &Shared,
    settings: &Arc<Settings>,
    name: SharedString,
    p: &MandaraPalette,
) -> Div {
    let choose = shared.clone();
    let search = state.search.clone();
    let refresh = shared.clone();
    let refresh_settings = settings.clone();
    let busy = shared.borrow().catalog.borrow().busy();
    let hover = p.on_surface.with_alpha(alpha::HOVER);
    div()
        .row()
        .items_center()
        .gap(px(tokens::SM))
        .flex_none()
        .child(
            div()
                .id("ai-model-picker")
                .flex_1()
                .min_w_0()
                .h(px(CONTROL_H))
                .px(px(tokens::SM))
                .rounded(px(crate::style::radius(tokens::SM)))
                .bg(p.on_surface.with_alpha(alpha::HOVER).hsla())
                .row()
                .items_center()
                .gap(px(tokens::SM))
                .cursor_pointer()
                .hover(move |style| style.bg(hover.hsla()))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(text::LABEL))
                        .text_color(p.on_surface_variant.hsla())
                        .child(mandara_i18n::tr(settings.provider.name())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(text::BODY_S))
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .child(if name.is_empty() {
                            SharedString::from(t("Choose a model or workspace"))
                        } else {
                            name
                        }),
                )
                .child(icon("chevron-down", CONTROL_ICON, p.on_surface_variant))
                .focusable()
                .tab_index(0)
                .on_click(move |_, window, cx| {
                    let picker = !choose.borrow().picker;
                    choose.borrow_mut().picker = picker;
                    if picker {
                        search.update(cx, |field, cx| field.focus(window, cx));
                    }
                    notify(&choose, cx);
                }),
        )
        .child(action(
            "ai-models-refresh",
            "refresh",
            t("Refresh"),
            !busy,
            false,
            p,
            move |_, cx| {
                load_catalog(&refresh, refresh_settings.clone(), true, cx);
            },
        ))
}

fn catalog(state: &State, shared: &Shared, p: &MandaraPalette) -> Div {
    let model = shared.borrow();
    let data = model.catalog.borrow();
    let query = state.query.borrow();
    let mut rows = div()
        .id("ai-model-choices")
        .min_h_0()
        .max_h(px(CATALOG_H))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(tokens::XS));
    if let Some(names) = &data.value {
        let mut matches = 0;
        for (index, name) in names
            .iter()
            .filter(|name| name.folded.contains(query.as_str()))
            .take(VISIBLE_MODELS)
            .enumerate()
        {
            matches += 1;
            let choose = shared.clone();
            let name = name.label.clone();
            let input = state.input.clone();
            rows = rows.child(
                action(
                    ("ai-choice", index),
                    "box",
                    name.clone(),
                    true,
                    false,
                    p,
                    move |window, cx| {
                        let mut model = choose.borrow_mut();
                        clear(&mut model);
                        model.selected = Some(name.clone());
                        drop(model);
                        input.update(cx, |field, cx| field.focus(window, cx));
                        notify(&choose, cx);
                    },
                )
                .w_full()
                .justify_start(),
            );
        }
        if matches == 0 {
            rows = rows.child(div().text_size(px(text::BODY_S)).child(t("No results")));
        }
        if names.len() > VISIBLE_MODELS {
            rows = rows.child(
                div()
                    .text_size(px(text::LABEL))
                    .text_color(p.on_surface_variant.hsla())
                    .child(t("Search to find more models or workspaces")),
            );
        }
    }
    if data.value.is_none() && data.busy() {
        rows = rows.child(div().text_size(px(text::BODY_S)).child(t("Loading…")));
    }
    if let Some(error) = &data.error {
        rows = rows.child(error_line(error, p));
    }
    glass_card(p)
        .absolute()
        .top(px(CATALOG_TOP))
        .left(px(tokens::LG))
        .right(px(tokens::LG))
        .p(px(tokens::MD))
        .flex()
        .flex_col()
        .gap(px(tokens::SM))
        .occlude()
        .child(state.search.clone())
        .child(rows)
}

fn introduction(p: &MandaraPalette) -> Div {
    let accent = readable(p.primary, p);
    div().flex_1().min_h_0().flex().flex_col().justify_center().cross_start()
        .gap(px(tokens::XL)).py(px(tokens::XXL))
        .child(glass_card(p).size(px(MARK_SIDE)).flex_none().flex().items_center().justify_center()
            .child(icon("sparkles", MARK_GLYPH, accent)))
        .child(div().max_w(px(INTRO_W)).text_size(px(text::HEADLINE)).line_height(px(INTRO_LINE_H))
            .font_weight(FontWeight::SEMIBOLD).child(crate::bidi::paragraph(t("A little room to think"))))
        .child(div().max_w(px(INTRO_W)).text_size(px(text::BODY)).line_height(px(MESSAGE_LINE_H))
            .text_color(p.on_surface_variant.hsla())
            .child(crate::bidi::paragraph(t("Ask your local model. Mandara clears this chat when you close the panel. Your server may keep its own history."))))
}

fn error_line(error: &str, p: &MandaraPalette) -> Div {
    div()
        .row()
        .gap(px(tokens::SM))
        .text_size(px(text::BODY_S))
        .line_height(px(MESSAGE_LINE_H))
        .text_color(readable(p.warning_amber, p).hsla())
        .child(icon("alert", CONTROL_ICON, readable(p.warning_amber, p)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(crate::bidi::paragraph(error.to_owned())),
        )
}

fn transcript(state: &State, shared: &Shared, p: &MandaraPalette) -> Stateful<Div> {
    let model = shared.borrow();
    if following_tail(
        f32::from(state.scroll.offset().y),
        f32::from(state.scroll.max_offset().y),
    ) {
        state.scroll.scroll_to_bottom();
    }
    let mut body = div()
        .id("ai-chat-transcript")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(&state.scroll)
        .flex()
        .flex_col()
        .gap(px(tokens::XL))
        .py(px(tokens::SM));
    if model.history.is_empty() {
        body = body.child(introduction(p));
    }
    for (index, message) in model.history.iter().enumerate() {
        let text = SharedString::from(message.content.clone());
        let user = message.role == Role::User;
        let mut block = div()
            .flex()
            .flex_col()
            .gap(px(tokens::SM))
            .text_size(px(text::BODY))
            .line_height(px(MESSAGE_LINE_H));
        if user {
            block = block
                .max_w(px(WIDTH - tokens::LG * 2.0 - MESSAGE_GUTTER))
                .p(px(tokens::MD))
                .rounded(px(crate::style::radius(tokens::LG)))
                .bg(p.primary.with_alpha(alpha::HOVER).hsla())
                .child(
                    div()
                        .text_size(px(text::LABEL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(p.on_surface_variant.hsla())
                        .child(t("You")),
                )
                .child(crate::bidi::paragraph(text));
        } else {
            let copy = message.content.clone();
            block = block
                .w_full()
                .child(
                    div()
                        .row()
                        .items_center()
                        .gap(px(tokens::SM))
                        .child(icon("sparkles", CONTROL_ICON, readable(p.primary, p)))
                        .child(
                            div()
                                .text_size(px(text::LABEL))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(p.on_surface_variant.hsla())
                                .child(t("Assistant")),
                        ),
                )
                .child(crate::bidi::paragraph(text))
                .child(div().row().child(action(
                    ("ai-copy-reply", index),
                    "copy",
                    t("Copy"),
                    true,
                    false,
                    p,
                    move |_, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.to_string()));
                    },
                )));
        }
        body = body.child(if user {
            div().w_full().row().child(block.push_end())
        } else {
            block
        });
    }
    if let Some(progress) = &model.progress {
        let mut progress = progress.lock().unwrap_or_else(|error| error.into_inner());
        let text = if progress.text.is_empty() {
            t("Waiting for the model…").into()
        } else {
            progress.display()
        };
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(px(tokens::SM))
                .text_size(px(text::BODY))
                .line_height(px(MESSAGE_LINE_H))
                .child(
                    div()
                        .row()
                        .items_center()
                        .gap(px(tokens::SM))
                        .child(icon("sparkles", CONTROL_ICON, readable(p.primary, p)))
                        .child(
                            div()
                                .text_size(px(text::LABEL))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(p.on_surface_variant.hsla())
                                .child(t("Assistant")),
                        ),
                )
                .child(crate::bidi::paragraph(text)),
        );
    }
    if let Some(error) = &model.error {
        body = body.child(error_line(error, p));
    }
    body
}

fn composer(
    state: &State,
    shared: &Shared,
    name: &str,
    p: &MandaraPalette,
    window: &Window,
    cx: &App,
) -> Stateful<Div> {
    let input = state.input.read(cx);
    let mode = ComposerAction::new(shared.borrow().running, &input.value, name);
    let focused = input.is_focused(window);
    let bytes = input.value.len();
    let send = shared.clone();
    let send_input = state.input.clone();
    let keyboard = shared.clone();
    let keyboard_input = state.input.clone();
    let hint = if bytes == 0 {
        t("Ctrl+Enter to send · Enter for a new line").to_owned()
    } else {
        tf("{bytes} / 8192 bytes", &[("bytes", &bytes)])
    };
    glass_card(p)
        .id("ai-chat-composer")
        .flex_none()
        .p(px(tokens::MD))
        .border_1()
        .border_color(
            p.primary
                .with_alpha(if focused { alpha::SELECTED } else { 0.0 })
                .hsla(),
        )
        .flex()
        .flex_col()
        .gap(px(tokens::XS))
        .child(
            div()
                .id("ai-chat-input-scroll")
                .h(px(COMPOSER_TEXT_H))
                .overflow_y_scroll()
                .child(state.input.clone()),
        )
        .child(
            div()
                .row()
                .items_center()
                .gap(px(tokens::SM))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(text::LABEL))
                        .text_color(p.on_surface_variant.hsla())
                        .font_features(crate::fonts::tabular_nums())
                        .child(crate::bidi::paragraph(crate::bidi::ltr_isolate(&hint))),
                )
                .child(action(
                    "ai-chat-submit",
                    if mode == ComposerAction::Stop {
                        "stop"
                    } else {
                        "arrow-up"
                    },
                    if mode == ComposerAction::Stop {
                        t("Stop")
                    } else {
                        t("Send")
                    },
                    mode != ComposerAction::Disabled,
                    mode == ComposerAction::Send,
                    p,
                    move |_, cx| {
                        if mode == ComposerAction::Stop {
                            super::stop(cx);
                        } else {
                            submit(&send, &send_input, cx);
                        }
                    },
                )),
        )
        .capture_key_down(move |event: &KeyDownEvent, _, cx| {
            if event.keystroke.key == "enter" && event.keystroke.modifiers.control {
                cx.stop_propagation();
                if mode == ComposerAction::Send {
                    submit(&keyboard, &keyboard_input, cx);
                }
            }
        })
}

pub(super) fn render(
    state: &State,
    p: &MandaraPalette,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let shared = model();
    let settings = super::super::cached_settings(&crate::state::config(cx).read(cx).snapshot());
    if shared.borrow().settings.as_ref() != Some(&settings) {
        let mut model = shared.borrow_mut();
        clear(&mut model);
        model.selected = Some(settings.model.as_str().into());
        model.settings = Some(settings.clone());
        super::super::evict();
    }
    let data = shared.borrow().catalog.clone();
    data.borrow().views.track(window);
    load_catalog(&shared, settings.clone(), false, cx);
    let name = shared.borrow().selected.clone().unwrap_or_default();
    let picker = shared.borrow().picker;
    let mut column = div()
        .relative()
        .size_full()
        .p(px(tokens::LG))
        .flex()
        .flex_col()
        .gap(px(tokens::SM))
        .child(toolbar(state, &shared, &settings, p))
        .child(model_picker(state, &shared, &settings, name.clone(), p));
    if settings.provider != Provider::Ollama {
        column = column.child(
            div()
                .text_size(px(text::LABEL))
                .text_color(p.on_surface_variant.hsla())
                .child(crate::bidi::paragraph(t(
                    "This server manages model loading and memory use.",
                ))),
        );
    }
    if !picker && let Some(error) = &data.borrow().error {
        column = column.child(error_line(error, p));
    }
    let dismiss = shared.clone();
    column
        .child(transcript(state, &shared, p))
        .child(composer(state, &shared, &name, p, window, cx))
        .when(picker, |el| el.child(catalog(state, &shared, p)))
        .capture_key_down(move |event: &KeyDownEvent, _, cx| {
            if picker && event.keystroke.key == "escape" {
                dismiss.borrow_mut().picker = false;
                notify(&dismiss, cx);
                cx.stop_propagation();
            }
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keyboard_and_pointer_offer_one_valid_composer_action() {
        for busy in [false, true] {
            for prompt in ["", " \n ", "hello"] {
                for model in ["", "chosen"] {
                    let action = ComposerAction::new(busy, prompt, model);
                    assert_eq!(action == ComposerAction::Stop, busy);
                    assert_eq!(
                        action == ComposerAction::Send,
                        !busy && !prompt.trim().is_empty() && !model.is_empty()
                    );
                }
            }
        }
    }
    #[test]
    fn completed_message_text_reuses_the_service_allocation() {
        let source: std::sync::Arc<str> = "shared message ".repeat(100).into();
        let shown = SharedString::from(source.clone());
        assert_eq!(shown.as_ptr(), source.as_ptr());
    }
}
