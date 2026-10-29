//! The keyring unlock dialog: Mandara's face for gnome-keyring's system
//! prompts (unlock a keyring, choose a new keyring's password, confirm).
//!
//! A modal overlay like the polkit dialog (exclusive keyboard, dimmed scrim,
//! chromatic-glass card): the keyring icon, the daemon's title / message /
//! description, a password pill with show/hide and a Caps Lock hint, a
//! confirmation pill and strength meter when creating a keyring, the
//! optional "unlock automatically" choice, and Cancel / Unlock. A wrong
//! password comes back as the same prompt with a warning: the pill shakes
//! and flashes coral. The prompter lives in
//! [`mandara_services::keyring_prompt`]; callers queue and show one at a time.
//!
//! The dialog never closes on focus loss (MangoWM sends keyboard leave +
//! enter before a click lands); only Cancel, Escape or the caller close it.
//!
//! `mandara msg keyring-prompt demo` opens it with a fake prompt (no D-Bus;
//! "wrong" is rejected once, anything else unlocks) for a safe preview.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use gpui::{
    App, AppContext as _, BoxShadow, ClickEvent, Context, FocusHandle, FontWeight, Global, InteractiveElement, IntoElement, KeyDownEvent,
    ModifiersChangedEvent, ParentElement, Render, StatefulInteractiveElement, Styled, Window, WindowHandle, div, point,
    prelude::FluentBuilder as _, px,
};
use mandara_core::{FrameClock, LayerSurfaceSpec, SpringConfig, SpringValue, SurfaceRole, ToGpui};
use mandara_i18n::{t, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use mandara_services::keyring_prompt::{
    KeyringEvent, KeyringPrompter, KeyringReply, PromptKind, PromptProps, PromptRequest, PrompterStatus, password_strength,
};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;
use zeroize::Zeroizing;

use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::polkit::m;

/// Text caret blink half-period (platform convention).
const CARET_BLINK: Duration = Duration::from_millis(530);
/// The demo's fake check.
const DEMO_CHECK_DELAY: Duration = Duration::from_millis(600);
const SHAKE: SpringConfig = SpringConfig::new(620.0, 13.0, 1.0);
const APPEAR: SpringConfig = SpringConfig::new(300.0, 23.0, 1.0);
/// Kick given to the shake spring: wrong password / empty submit.
const SHAKE_WRONG: f32 = -2400.0;
const SHAKE_EMPTY: f32 = -1400.0;
/// Longest password the fields accept (bytes).
const MAX_INPUT: usize = 512;
/// Dots drawn at most for a hidden password.
const MAX_DOTS: usize = 28;
/// Strength meter segments (`password_strength` tops out here).
const STRENGTH_SEGMENTS: i32 = 4;

/// Shell-wide prompter state.
#[derive(Default)]
pub struct KeyringUi {
    _prompter: Option<KeyringPrompter>,
    window: Option<WindowHandle<KeyringDialog>>,
    queue: VecDeque<(PromptRequest, KeyringPrompter)>,
    pub status: Option<PrompterStatus>,
}

impl Global for KeyringUi {}

fn is_open(handle: &Option<WindowHandle<KeyringDialog>>, cx: &App) -> bool {
    handle.as_ref().is_some_and(|h| cx.windows().iter().any(|w| w.window_id() == h.window_id()))
}

/// Starts the prompter (when `general.keyring_prompt`) and wires it to the
/// dialog; switching the setting in Studio starts or stops it live.
pub fn init(cx: &mut App) {
    let mut enabled = crate::state::config(cx).read(cx).get().general.keyring_prompt;
    let prompter = mandara_services::keyring_prompt::spawn_keyring_prompt(enabled);
    cx.set_global(KeyringUi { _prompter: Some(prompter.clone()), ..Default::default() });
    pump(prompter, cx);
    cx.observe(&crate::state::config(cx), move |store, cx| {
        let want = store.read(cx).get().general.keyring_prompt;
        if want != enabled {
            enabled = want;
            set_enabled(want, cx);
        }
    })
    .detach();
}

/// Replaces the running prompter with a fresh one (`on`) or a disabled
/// one; the old one tells its callers it is done and releases the name.
fn set_enabled(on: bool, cx: &mut App) {
    if let Some(old) = cx.try_global::<KeyringUi>().and_then(|ui| ui._prompter.clone()) {
        old.stop();
    }
    let prompter = mandara_services::keyring_prompt::spawn_keyring_prompt(on);
    let ui = cx.default_global::<KeyringUi>();
    ui._prompter = Some(prompter.clone());
    ui.status = None;
    tracing::info!("keyring prompt: {}", if on { "starting" } else { "off" });
    pump(prompter, cx);
}

fn pump(prompter: KeyringPrompter, cx: &mut App) {
    let events = prompter.events.clone();
    cx.spawn(async move |cx| {
        while let Ok(event) = events.recv().await {
            let prompter = prompter.clone();
            cx.update(|cx| handle_event(event, prompter, cx));
        }
    })
    .detach();
}

fn handle_event(event: KeyringEvent, prompter: KeyringPrompter, cx: &mut App) {
    // A replaced prompter's last words must not overwrite the new state.
    if prompter.is_stopped() {
        return;
    }
    if !cx.has_global::<KeyringUi>() {
        cx.set_global(KeyringUi::default());
    }
    match event {
        KeyringEvent::Status(status) => cx.global_mut::<KeyringUi>().status = Some(status),
        KeyringEvent::Show(request) => {
            // The same prompt again (a retry): update the open dialog.
            let id = request.id;
            let mut pending = Some(request);
            with_dialog(cx, id, |d, cx| {
                if let Some(request) = pending.take() {
                    d.update_request(request);
                }
                cx.notify();
            });
            if let Some(request) = pending {
                let ui = cx.global_mut::<KeyringUi>();
                match ui.queue.iter_mut().find(|(r, _)| r.id == id) {
                    Some(slot) => slot.0 = request,
                    None => ui.queue.push_back((request, prompter)),
                }
                show_next(cx);
            }
        }
        KeyringEvent::Close { id } => {
            cx.global_mut::<KeyringUi>().queue.retain(|(r, _)| r.id != id);
            with_dialog(cx, id, |d, cx| {
                d.closing = true;
                cx.notify();
            });
        }
    }
}

/// Runs `f` on the open dialog when it shows prompt `id`.
fn with_dialog(cx: &mut App, id: u64, f: impl FnOnce(&mut KeyringDialog, &mut Context<KeyringDialog>)) {
    let handle = cx.global::<KeyringUi>().window;
    if let Some(handle) = handle {
        let _ = handle.update(cx, |d, _, cx| {
            if d.request.id == id && !d.closing {
                f(d, cx);
            }
        });
    }
}

fn show_next(cx: &mut App) {
    if is_open(&cx.global::<KeyringUi>().window, cx) {
        return;
    }
    let Some((request, prompter)) = cx.global_mut::<KeyringUi>().queue.pop_front() else { return };
    let (w, h) = output_size(cx);
    // The lock role: its namespace already carries every compositor's
    // no-animation / no-shadow layer rules (the polkit dialog uses it too).
    let mut spec = LayerSurfaceSpec::overlay(SurfaceRole::Lock, gpui::size(px(w), px(h)), None).with_keyboard(KeyboardInteractivity::Exclusive);
    spec.layer = Layer::Overlay;
    spec.anchor = Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT;
    let handle = cx
        .open_window(spec.window_options(), move |window, cx| cx.new(|cx| KeyringDialog::new(request, prompter, (w, h), window, cx)))
        .map_err(|e| tracing::error!("keyring dialog: {e:#}"))
        .ok();
    cx.global_mut::<KeyringUi>().window = handle;
}

fn output_size(cx: &App) -> (f32, f32) {
    cx.try_global::<crate::overlays::Outputs>()
        .and_then(|o| o.0.first().map(|g| (g.width as f32, g.height as f32)))
        .or_else(|| mandara_core::query_outputs().ok().and_then(|o| o.first().map(|g| (g.width as f32, g.height as f32))))
        .unwrap_or((1920.0, 1080.0))
}

// ─────────────────────────────── the dialog ───────────────────────────────

/// The fields: the password, and its confirmation for a new keyring.
const FIELD_PASSWORD: usize = 0;
const FIELD_CONFIRM: usize = 1;

pub struct KeyringDialog {
    request: PromptRequest,
    prompter: KeyringPrompter,
    fields: [Zeroizing<String>; 2],
    active: usize,
    reveal: bool,
    choice: bool,
    /// Answer sent; waiting for the caller (next prompt or close).
    busy: bool,
    /// Our own complaint (passwords differ), shown over the warning.
    note: Option<String>,
    focus: FocusHandle,
    appear: SpringValue<f32>,
    shake: SpringValue<f32>,
    flash: SpringValue<f32>,
    clock: FrameClock,
    closing: bool,
    expected: (f32, f32),
    blink_from: Instant,
    _blink: gpui::Task<()>,
}

fn wipe(s: &mut String) {
    // SAFETY: zero bytes are valid UTF-8.
    unsafe { s.as_bytes_mut().fill(0) };
    s.clear();
}

impl KeyringDialog {
    fn new(request: PromptRequest, prompter: KeyringPrompter, expected: (f32, f32), window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&mandara_core::theme_entity(cx), |_, _, cx| cx.notify()).detach();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let mut appear = SpringValue::new(0.0, APPEAR);
        appear.set_target(1.0);
        let blink = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CARET_BLINK).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        let choice = request.props.choice_chosen;
        Self {
            request,
            prompter,
            fields: [Zeroizing::new(String::with_capacity(MAX_INPUT)), Zeroizing::new(String::with_capacity(MAX_INPUT))],
            active: FIELD_PASSWORD,
            reveal: false,
            choice,
            busy: false,
            note: None,
            focus,
            appear,
            shake: SpringValue::new(0.0, SHAKE),
            flash: SpringValue::new(0.0, SpringConfig::critical(90.0)),
            clock: FrameClock::new(),
            closing: false,
            expected,
            blink_from: Instant::now(),
            _blink: blink,
        }
    }

    fn props(&self) -> &PromptProps {
        &self.request.props
    }

    fn wipe_fields(&mut self) {
        for f in &mut self.fields {
            wipe(f);
        }
        self.active = FIELD_PASSWORD;
    }

    /// The caller asks again (typically after a wrong password).
    fn update_request(&mut self, request: PromptRequest) {
        let retry = self.busy && !request.props.warning.is_empty();
        self.request = request;
        self.choice = self.request.props.choice_chosen;
        self.busy = false;
        self.note = None;
        self.wipe_fields();
        if retry {
            self.shake.add_velocity([SHAKE_WRONG, 0.0, 0.0, 0.0]);
            self.flash.snap_to(1.0);
            self.flash.set_target(0.0);
        }
    }

    fn reject(&mut self, note: Option<String>, kick: f32) {
        self.shake.add_velocity([kick, 0.0, 0.0, 0.0]);
        self.note = note;
    }

    fn submit(&mut self) {
        if self.busy || self.closing {
            return;
        }
        let id = self.request.id;
        match self.request.kind {
            PromptKind::Confirm => self.prompter.reply(KeyringReply::Confirm { id, choice: self.choice }),
            PromptKind::Password => {
                let new = self.props().password_new;
                if !new && self.fields[FIELD_PASSWORD].is_empty() {
                    return self.reject(None, SHAKE_EMPTY);
                }
                if new && self.fields[FIELD_PASSWORD].as_str() != self.fields[FIELD_CONFIRM].as_str() {
                    self.active = FIELD_CONFIRM;
                    wipe(&mut self.fields[FIELD_CONFIRM]);
                    return self.reject(Some(t("The passwords don't match").into()), SHAKE_WRONG);
                }
                let password = Zeroizing::new(self.fields[FIELD_PASSWORD].as_str().to_string());
                self.wipe_fields();
                self.prompter.reply(KeyringReply::Password { id, password, choice: self.choice });
            }
        }
        self.busy = true;
        self.note = None;
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.closing {
            self.prompter.reply(KeyringReply::Cancel { id: self.request.id });
            self.wipe_fields();
            self.closing = true;
            cx.notify();
        }
    }

    fn field_count(&self) -> usize {
        match self.request.kind {
            PromptKind::Password if self.props().password_new => 2,
            PromptKind::Password => 1,
            PromptKind::Confirm => 0,
        }
    }

    fn on_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        self.blink_from = Instant::now();
        let fields = self.field_count();
        match k.key.as_str() {
            "escape" => return self.cancel(cx),
            "enter" => {
                // New keyring: Enter in the first field moves to the second.
                if fields == 2 && self.active == FIELD_PASSWORD && self.fields[FIELD_CONFIRM].is_empty() && !self.fields[FIELD_PASSWORD].is_empty() {
                    self.active = FIELD_CONFIRM;
                } else {
                    self.submit();
                }
            }
            "tab" if fields == 2 => self.active = if self.active == FIELD_PASSWORD { FIELD_CONFIRM } else { FIELD_PASSWORD },
            _ if fields == 0 || self.busy => {}
            "backspace" => {
                let field = &mut self.fields[self.active];
                if k.modifiers.control {
                    wipe(field);
                } else {
                    field.pop();
                }
            }
            "u" if k.modifiers.control => wipe(&mut self.fields[self.active]),
            _ => {
                if let Some(ch) = k.key_char.as_ref().filter(|_| !k.modifiers.control && !k.modifiers.alt)
                    && self.fields[self.active].len() + ch.len() <= MAX_INPUT
                {
                    self.fields[self.active].push_str(ch);
                    self.note = None;
                }
            }
        }
        cx.notify();
    }
}

pub(crate) fn soft_shadow(blur: f32, alpha: f32, y: f32) -> BoxShadow {
    BoxShadow { color: Color::hex("#000000").with_alpha(alpha).hsla(), offset: point(px(0.0), px(y)), blur_radius: px(blur), spread_radius: px(0.0), inset: false }
}

/// A wrapping paragraph that may be right to left ([`crate::bidi::paragraph`]).
pub(crate) fn flow_text(text: &str, size: f32, line_height: f32) -> gpui::AnyElement {
    div().w_full().text_size(px(size)).line_height(px(line_height)).child(crate::bidi::paragraph(text.to_string())).into_any_element()
}

/// Keyring-only geometry (the rest is the polkit dialog's, [`m`]).
/// Show / hide button in the password pill, and the pill's end padding.
const REVEAL_BUTTON: f32 = 32.0;
const REVEAL_PAD_END: f32 = crate::kit::tokens::SM;
/// Strength meter: gap to its label, segment gap and height, label width.
const METER_GAP: f32 = crate::kit::tokens::SM + crate::kit::metrics::NUDGE;
const METER_SEGMENT_GAP: f32 = crate::kit::tokens::XS;
const METER_H: f32 = crate::kit::tokens::XS;
const METER_LABEL_W: f32 = 56.0;
/// Caps Lock / warning rows: icon–text gap.
const HINT_GAP: f32 = 6.0;
/// "Unlock automatically" checkbox.
const CHECKBOX: f32 = crate::kit::tokens::XL;
const CHECKBOX_RADIUS: f32 = 6.0;

/// Meter colour and label for a strength score.
fn strength_style(score: i32, p: &MandaraPalette) -> (Color, String) {
    match score {
        i32::MIN..=1 => (p.urgent_coral, t("Weak").into()),
        2 => (p.warning_amber, t("Fair").into()),
        3 => (p.primary, t("Good").into()),
        _ => (p.success_emerald, t("Strong").into()),
    }
}

impl Render for KeyringDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vp = window.viewport_size();
        let settled = (f32::from(vp.width) - self.expected.0).abs() < 2.0 && (f32::from(vp.height) - self.expected.1).abs() < 2.0;
        if !settled && !self.closing {
            window.request_animation_frame();
            return div().id("keyring-root").size_full().track_focus(&self.focus).into_any_element();
        }
        let p = mandara_core::frame_palette(window, cx);
        let f = crate::fonts::fonts(cx);
        self.appear.set_config(if self.closing { SpringConfig::critical(420.0) } else { APPEAR });
        self.appear.set_target(if self.closing { 0.0 } else { 1.0 });
        let dt = self.clock.tick(Instant::now());
        let mut moving = self.appear.step(dt);
        moving |= self.shake.step(dt);
        moving |= self.flash.step(dt);
        if self.closing && self.appear.value() <= 0.01 {
            window.remove_window();
            cx.defer(show_next);
        } else if moving || self.busy {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        let a = self.appear.value();
        let shown = a.clamp(0.0, 1.0);
        let lift = (1.0 - a) * m::APPEAR_RISE;
        let props = self.request.props.clone();
        let kind = self.request.kind;
        let new = kind == PromptKind::Password && props.password_new;

        // Header: the keyring icon in a tinted disc, title and message.
        let (fallback_title, fallback_message) = match kind {
            PromptKind::Password if new => (t("New keyring"), t("Choose a password for the new keyring")),
            PromptKind::Password => (t("Unlock keyring"), t("Authentication required")),
            PromptKind::Confirm => (t("Keyring"), t("Please confirm")),
        };
        let title = if props.title.is_empty() { fallback_title.to_string() } else { props.title.clone() };
        let message = if props.message.is_empty() { fallback_message.to_string() } else { props.message.clone() };
        let header = div()
            .row()
            .items_center()
            .gap(px(m::HEADER_GAP))
            .child(
                div()
                    .size(px(m::DISC))
                    .flex_none()
                    .rounded_full()
                    .bg(p.primary.with_alpha(0.14).hsla())
                    .border_1()
                    .border_color(p.primary.with_alpha(0.28).hsla())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(if new { "key" } else { "lock" }, 24.0, p.primary)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(m::TITLE_GAP))
                    .min_w_0()
                    .child(div().text_size(px(m::TITLE)).font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(div().text_size(px(m::SUBTITLE)).text_color(p.on_surface_variant.hsla()).child(message)),
            );

        let description = (!props.description.is_empty())
            .then(|| div().w_full().text_color(p.on_surface.with_alpha(0.86).hsla()).child(flow_text(&props.description, 13.5, 20.0)));

        // Password pills.
        let caret_on = (self.blink_from.elapsed().as_millis() / CARET_BLINK.as_millis()).is_multiple_of(2) && !self.busy;
        let flash = self.flash.value().clamp(0.0, 1.0);
        let field_count = self.field_count();
        let pill = |ix: usize, this: &Self, cx: &mut Context<Self>| {
            let placeholder = match (ix, new) {
                (FIELD_CONFIRM, _) => t("Confirm password"),
                (_, true) => t("New password"),
                _ => t("Password"),
            };
            let text = this.fields[ix].as_str();
            let active = this.active == ix;
            let content = if text.is_empty() {
                div().text_color(p.on_surface_variant.with_alpha(0.7).hsla()).child(placeholder).into_any_element()
            } else if this.reveal {
                div().child(text.to_string()).into_any_element()
            } else {
                let n = text.chars().count().min(MAX_DOTS);
                div().flex().items_center().gap(px(m::DOT_GAP)).children((0..n).map(|_| div().size(px(m::DOT)).rounded_full().bg(p.on_surface.hsla()))).into_any_element()
            };
            let mut shadows = mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow);
            shadows.push(soft_shadow(m::PILL_SHADOW.0, m::PILL_SHADOW.1, m::PILL_SHADOW.2));
            let rim = if flash > 0.02 {
                p.urgent_coral.with_alpha(flash).hsla()
            } else if active && field_count > 1 {
                p.primary.with_alpha(0.55).hsla()
            } else {
                p.outline.with_alpha(0.16).hsla()
            };
            let empty = text.is_empty();
            let caret = || div().w(px(m::CARET_W)).h(px(m::CARET_H)).bg(p.primary.hsla());
            div()
                .id(("keyring-field", ix))
                .ml(px(this.shake.value()))
                .h(px(m::PILL_H))
                .pl(px(m::PILL_PAD))
                .pr(px(REVEAL_PAD_END))
                .rounded(px(crate::style::radius(m::PILL_RADIUS)))
                .bg(p.surface_container_high.with_alpha(0.75).hsla())
                .border_1()
                .border_color(rim)
                .shadow(shadows)
                .row()
                .items_center()
                .gap(px(m::PILL_GAP))
                .text_size(px(m::PILL_TEXT))
                .cursor_text()
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.active = ix;
                    cx.notify();
                }))
                .child(icon(if ix == FIELD_CONFIRM { "check" } else { "lock" }, 15.0, p.on_surface_variant))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .min_w_0()
                        .overflow_hidden()
                        .when(caret_on && active && empty, |d| d.child(caret()))
                        .child(content)
                        .when(caret_on && active && !empty, |d| d.child(caret())),
                )
                .when(ix == FIELD_PASSWORD, |d| {
                    d.child(
                        div()
                            .id("keyring-reveal")
                            .size(px(REVEAL_BUTTON))
                            .flex_none()
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(p.on_surface.with_alpha(0.08).hsla()))
                            .child(icon(if this.reveal { "eye-off" } else { "eye" }, 16.0, p.on_surface_variant))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.reveal = !this.reveal;
                                cx.notify();
                            })),
                    )
                })
        };
        let mut inputs = div().flex().flex_col().gap(px(m::NOTE_GAP));
        for ix in 0..field_count {
            inputs = inputs.child(pill(ix, self, cx));
        }

        // Strength meter (new keyring).
        let meter = new.then(|| {
            let score = password_strength(&self.fields[FIELD_PASSWORD]);
            let (color, label) = strength_style(score, &p);
            div()
                .row()
                .items_center()
                .gap(px(METER_GAP))
                .child(div().flex_1().row().gap(px(METER_SEGMENT_GAP)).children((0..STRENGTH_SEGMENTS).map(|i| {
                    div().flex_1().h(px(METER_H)).rounded_full().bg(if i < score { color.hsla() } else { p.outline.with_alpha(0.18).hsla() })
                })))
                .child(
                    div()
                        .min_w(px(METER_LABEL_W))
                        .text_size(px(m::USER_LOGIN))
                        .text_color(if score > 0 { color.hsla() } else { p.on_surface_variant.hsla() })
                        .child(if score > 0 { label } else { t("Strength").into() }),
                )
        });

        let caps = (field_count > 0 && window.capslock().on).then(|| {
            div()
                .row()
                .items_center()
                .gap(px(HINT_GAP))
                .text_size(px(m::SUBTITLE))
                .text_color(p.warning_amber.hsla())
                .child(icon("arrow-up", 13.0, p.warning_amber))
                .child(t("Caps Lock is on"))
        });

        let warning = self.note.clone().or_else(|| (!props.warning.is_empty()).then(|| props.warning.clone()));
        let warning = warning.map(|text| {
            div().row().items_center().gap(px(HINT_GAP)).text_color(p.urgent_coral.hsla()).child(icon("alert", 14.0, p.urgent_coral)).child(div().flex_1().min_w_0().child(flow_text(&text, 12.5, 17.0)))
        });

        // "Automatically unlock this keyring whenever I'm logged in".
        let choice = (!props.choice_label.is_empty()).then(|| {
            let on = self.choice;
            div()
                .id("keyring-choice")
                .row()
                .items_center()
                .gap(px(m::USER_GAP))
                .cursor_pointer()
                .child(
                    div()
                        .size(px(CHECKBOX))
                        .flex_none()
                        .rounded(px(crate::style::radius(CHECKBOX_RADIUS)))
                        .border_1()
                        .border_color(if on { p.primary.hsla() } else { p.outline.with_alpha(0.45).hsla() })
                        .bg(if on { p.primary.hsla() } else { p.surface_container_high.with_alpha(0.6).hsla() })
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(on, |d| d.child(icon("check", 14.0, p.on_primary))),
                )
                .child(div().flex_1().min_w_0().child(flow_text(&props.choice_label, 13.0, 18.0)))
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    if !this.busy {
                        this.choice = !this.choice;
                        cx.notify();
                    }
                }))
        });

        let busy = self.busy;
        let ready = match kind {
            PromptKind::Confirm => true,
            PromptKind::Password if new => self.fields[FIELD_PASSWORD].as_str() == self.fields[FIELD_CONFIRM].as_str(),
            PromptKind::Password => !self.fields[FIELD_PASSWORD].is_empty(),
        } && !busy;
        let continue_label = if !props.continue_label.is_empty() {
            props.continue_label.clone()
        } else {
            match kind {
                PromptKind::Password if new => t("Create").into(),
                PromptKind::Password => t("Unlock").into(),
                PromptKind::Confirm => t("Continue").into(),
            }
        };
        let cancel_label = if props.cancel_label.is_empty() { t("Cancel").to_string() } else { props.cancel_label.clone() };
        let primary = div()
            .id("keyring-continue")
            .h(px(m::BUTTON_H))
            .px(px(m::BUTTON_PAD))
            .rounded(px(crate::style::radius(m::BUTTON_RADIUS)))
            .flex()
            .items_center()
            .justify_center()
            .font_weight(FontWeight::SEMIBOLD)
            .text_size(px(m::BUTTON_TEXT))
            .text_color(p.on_primary.hsla())
            .bg(mandara_core::oklab_gradient(135.0, p.accent_gradient_start, p.accent_gradient_end))
            .shadow(vec![BoxShadow { color: p.primary_glow.with_alpha(0.35).hsla(), offset: point(px(0.0), px(m::GLOW_Y)), blur_radius: px(m::GLOW_BLUR), spread_radius: px(0.0), inset: false }])
            .opacity(if ready || busy { 1.0 } else { 0.55 })
            .cursor_pointer()
            .child(if busy { t("Checking…").to_string() } else { continue_label })
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.submit();
                cx.notify();
            }));
        let cancel = div()
            .id("keyring-cancel")
            .h(px(m::BUTTON_H))
            .px(px(m::CANCEL_PAD))
            .rounded(px(crate::style::radius(m::BUTTON_RADIUS)))
            .flex()
            .items_center()
            .text_size(px(m::BUTTON_TEXT))
            .text_color(p.on_surface.hsla())
            .bg(p.surface_container_high.with_alpha(0.55).hsla())
            .hover(|s| s.bg(p.surface_container_high.hsla()))
            .cursor_pointer()
            .child(cancel_label)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.cancel(cx)));

        let mut card_shadows = mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow);
        card_shadows.push(soft_shadow(m::CARD_SHADOW.0, m::CARD_SHADOW.1, m::CARD_SHADOW.2));
        let card = div()
            .id("keyring-card")
            .occlude()
            .w(px(crate::polkit::auth_dialog_width(crate::state::config(cx).read(cx).get())))
            .mt(px(lift))
            .opacity(shown)
            .p(px(m::CARD_PAD))
            .rounded(px(crate::style::radius(m::CARD_RADIUS)))
            .bg(p.surface_container.with_alpha(0.94).hsla())
            .border_1()
            .border_color(p.outline.with_alpha(0.14).hsla())
            .shadow(card_shadows)
            .flex()
            .flex_col()
            .gap(px(m::CARD_GAP))
            .child(header)
            .children(description)
            .when(field_count > 0 || warning.is_some() || meter.is_some(), |d| {
                d.child(div().flex().flex_col().gap(px(m::NOTE_GAP)).child(inputs).children(meter).children(caps).children(warning))
            })
            .children(choice)
            .child(div().row().justify_end().gap(px(m::BUTTONS_GAP)).child(cancel).child(primary));

        div()
            .id("keyring-root")
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.on_key(e, cx)))
            .on_modifiers_changed(cx.listener(|_, _: &ModifiersChangedEvent, _, cx| cx.notify()))
            .font_family(f.ui.clone())
            .text_color(p.on_surface.hsla())
            .text_start()
            .bg(p.surface_dim.with_alpha(0.52 * shown).hsla())
            .flex()
            .items_center()
            .justify_center()
            .child(card)
            .into_any_element()
    }
}

// ─────────────────────────────── commands ───────────────────────────────

/// Which fake prompt `keyring-prompt demo` shows.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DemoKind {
    Unlock,
    Create,
    Confirm,
}

fn demo_props(kind: DemoKind) -> (PromptKind, PromptProps) {
    match kind {
        DemoKind::Unlock => (
            PromptKind::Password,
            PromptProps {
                title: t("Unlock keyring").into(),
                message: t("Authentication required").into(),
                description: t("An app wants access to the keyring “Login”, but it is locked (preview: nothing is unlocked)").into(),
                choice_label: t("Automatically unlock this keyring whenever I'm logged in").into(),
                ..Default::default()
            },
        ),
        DemoKind::Create => (
            PromptKind::Password,
            PromptProps {
                title: t("New keyring").into(),
                message: t("Choose a password for the new keyring").into(),
                description: t("An app wants to create a new keyring called “Mandara”. Choose the password you want to use for it (preview: nothing is created)").into(),
                password_new: true,
                ..Default::default()
            },
        ),
        DemoKind::Confirm => (
            PromptKind::Confirm,
            PromptProps {
                title: t("Store passwords unencrypted?").into(),
                message: t("Store passwords unencrypted?").into(),
                description: t("By choosing an empty password, your stored passwords will not be safely encrypted. They will be accessible by anyone with access to your files (preview)").into(),
                ..Default::default()
            },
        ),
    }
}

/// A fake prompter for the demo: "wrong" fails once with a warning, any
/// other answer unlocks; nothing touches D-Bus.
fn demo_prompter(kind: DemoKind) -> (KeyringPrompter, PromptRequest) {
    let (etx, erx) = async_channel::unbounded();
    let (rtx, rrx) = async_channel::unbounded::<KeyringReply>();
    let prompter = KeyringPrompter::from_channels(erx, rtx);
    let (prompt_kind, props) = demo_props(kind);
    // Far above the real prompter's ids, which count up from 1.
    let id = u64::MAX - u64::from(std::process::id());
    let request = PromptRequest { id, kind: prompt_kind, props };
    let first = request.clone();
    let _ = std::thread::Builder::new().name("mandara-keyring-demo".into()).spawn(move || {
        while let Ok(reply) = rrx.recv_blocking() {
            match reply {
                KeyringReply::Password { password, .. } => {
                    std::thread::sleep(DEMO_CHECK_DELAY);
                    if password.as_str() == "wrong" {
                        let mut retry = first.clone();
                        retry.props.warning = t("The unlock password was incorrect").into();
                        let _ = etx.send_blocking(KeyringEvent::Show(retry));
                        continue;
                    }
                    let _ = etx.send_blocking(KeyringEvent::Close { id });
                    break;
                }
                KeyringReply::Confirm { .. } | KeyringReply::Cancel { .. } => {
                    let _ = etx.send_blocking(KeyringEvent::Close { id });
                    break;
                }
            }
        }
    });
    (prompter, request)
}

/// Opens the dialog with a fake prompt (safe preview).
pub fn open_demo(kind: DemoKind, cx: &mut App) {
    let (prompter, request) = demo_prompter(kind);
    pump(prompter.clone(), cx);
    handle_event(KeyringEvent::Show(request), prompter, cx);
}

/// Cancels whatever keyring dialog is showing.
pub fn close(cx: &mut App) -> bool {
    let Some(handle) = cx.try_global::<KeyringUi>().and_then(|u| u.window) else { return false };
    handle.update(cx, |d, _, cx| d.cancel(cx)).is_ok()
}

fn status_json(cx: &App) -> serde_json::Value {
    let status = cx.try_global::<KeyringUi>().and_then(|u| u.status.clone());
    match status {
        Some(PrompterStatus::Serving) => json!({ "prompter": "serving", "name": mandara_services::keyring_prompt::BUS_NAME }),
        Some(PrompterStatus::AnotherPrompter) => json!({ "prompter": "another-prompter" }),
        Some(PrompterStatus::Disabled) => json!({ "prompter": "disabled" }),
        Some(PrompterStatus::Unavailable(msg)) => json!({ "prompter": "unavailable", "detail": msg }),
        None => json!({ "prompter": "starting" }),
    }
}

/// `keyring-prompt status | demo [unlock|create|confirm] | close`.
pub fn register(r: &mut PluginRegistry) {
    r.command(
        "keyring-prompt",
        CommandSpec::new("keyring-prompt", tk("Keyring prompter status; demo previews the unlock dialog; close cancels it"), |a, cx| match a.str(0)? {
            "demo" => {
                let kind = match a.str(1).unwrap_or("unlock") {
                    "create" | "new" => DemoKind::Create,
                    "confirm" => DemoKind::Confirm,
                    _ => DemoKind::Unlock,
                };
                open_demo(kind, cx);
                Ok(json!({ "demo": format!("{kind:?}").to_lowercase() }))
            }
            "close" => Ok(json!({ "closed": close(cx) })),
            _ => Ok(status_json(cx)),
        })
        .arg(ArgSpec::required("action", ArgKind::Enum, tk("what to do")).choices(&["status", "demo", "close"]))
        .arg(ArgSpec::optional("kind", ArgKind::Enum, tk("which demo prompt")).choices(&["unlock", "create", "confirm"]))
        .example("keyring-prompt status")
        .example("keyring-prompt demo")
        .example("keyring-prompt demo create"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_prompts_cover_every_kind() {
        let (k, p) = demo_props(DemoKind::Unlock);
        assert_eq!(k, PromptKind::Password);
        assert!(!p.choice_label.is_empty() && !p.password_new);
        let (k, p) = demo_props(DemoKind::Create);
        assert!(k == PromptKind::Password && p.password_new);
        assert_eq!(demo_props(DemoKind::Confirm).0, PromptKind::Confirm);
    }

    #[test]
    fn demo_rejects_wrong_once_then_closes() {
        let (prompter, req) = demo_prompter(DemoKind::Unlock);
        prompter.reply(KeyringReply::Password { id: req.id, password: Zeroizing::new("wrong".into()), choice: false });
        match prompter.events.recv_blocking() {
            Ok(KeyringEvent::Show(retry)) => {
                assert_eq!(retry.id, req.id);
                assert!(!retry.props.warning.is_empty());
            }
            other => panic!("expected a retry, got {other:?}"),
        }
        prompter.reply(KeyringReply::Password { id: req.id, password: Zeroizing::new("sea glass".into()), choice: true });
        assert_eq!(prompter.events.recv_blocking(), Ok(KeyringEvent::Close { id: req.id }));
    }
}
