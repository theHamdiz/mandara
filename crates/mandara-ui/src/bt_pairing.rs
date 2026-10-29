//! The Bluetooth pairing dialog: a modal overlay (like the polkit dialog)
//! answering [`mandara_services::bt_agent`] requests — compare a six-digit
//! passkey, type a PIN or passkey shown on the other device, show one to
//! type there, or allow a device / service. One request shows at a time;
//! BlueZ cancelling a request closes its dialog.
//!
//! `mandara msg bluetooth pairing-demo <kind>` previews the dialog with a
//! fake device (answers go nowhere).

use std::collections::VecDeque;
use std::time::Instant;

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use gpui::{
    App, AppContext as _, BoxShadow, ClickEvent, Context, FocusHandle, FontWeight, Global, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    Render, StatefulInteractiveElement, Styled, Window, WindowHandle, div, point, prelude::FluentBuilder as _, px,
};
use mandara_core::{FrameClock, LayerSurfaceSpec, SpringConfig, SpringValue, SurfaceRole, ToGpui};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use mandara_services::bt_agent::{self, AgentStatus, PairingAgent, PairingEvent, PairingKind, PairingReply, PairingRequest};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use crate::icons::icon;

/// Card geometry and type, logical px.
const CARD_W_PX: f32 = 420.0;
const CARD_PAD_PX: f32 = 24.0;
const CARD_RADIUS_PX: f32 = 26.0;
const CARD_GAP_PX: f32 = 16.0;
const DISC_PX: f32 = 52.0;
const DISC_ICON_PX: f32 = 24.0;
const TITLE_PX: f32 = 17.0;
const BODY_PX: f32 = 13.5;
const BODY_LEADING_PX: f32 = 20.0;
const SMALL_PX: f32 = 12.0;
/// The big code: digit size, cell size and gap.
const CODE_DIGIT_PX: f32 = 30.0;
const CODE_CELL_W_PX: f32 = 44.0;
const CODE_CELL_H_PX: f32 = 56.0;
const CODE_GAP_PX: f32 = 8.0;
const CODE_CELL_RADIUS_PX: f32 = 14.0;
/// Input pill and buttons.
const PILL_H_PX: f32 = 46.0;
const PILL_RADIUS_PX: f32 = 23.0;
const BUTTON_H_PX: f32 = 40.0;
const BUTTON_PAD_PX: f32 = 18.0;
const BUTTON_RADIUS_PX: f32 = 20.0;
const GAP_SMALL_PX: f32 = 8.0;
const GAP_PX: f32 = 10.0;
const HEADER_GAP_PX: f32 = 14.0;
/// Entrance slide distance, px, and the backdrop dim.
const RISE_PX: f32 = 26.0;
const SCRIM_ALPHA: f32 = 0.52;
const CARD_ALPHA: f32 = 0.94;
const SHADOW_BLUR_PX: f32 = 48.0;
const SHADOW_ALPHA: f32 = 0.38;
const SHADOW_Y_PX: f32 = 18.0;
const GLOW_ALPHA: f32 = 0.35;
const GLOW_BLUR_PX: f32 = 16.0;
const GLOW_Y_PX: f32 = 4.0;
const TINT_ALPHA: f32 = 0.14;
const TINT_BORDER_ALPHA: f32 = 0.28;
const OUTLINE_ALPHA: f32 = 0.14;
const DISABLED_ALPHA: f32 = 0.55;
const HINT_ALPHA: f32 = 0.7;
/// Gradient angle of the primary button.
const GRADIENT_DEG: f32 = 135.0;
/// Close when the appear spring is this close to zero.
const GONE: f32 = 0.01;
/// Viewport sizes within this many px count as the requested size.
const SETTLE_EPSILON_PX: f32 = 2.0;
/// Fallback output size before the outputs are known.
const FALLBACK_OUTPUT: (f32, f32) = (1920.0, 1080.0);
/// Longest PIN BlueZ takes.
const PIN_MAX_CHARS: usize = 16;
const PASSKEY_DIGITS: usize = 6;
const APPEAR: SpringConfig = SpringConfig::new(300.0, 23.0, 1.0);
const DISAPPEAR_STIFFNESS: f32 = 420.0;

/// Shell-wide state.
#[derive(Default)]
pub struct PairingUi {
    /// Keeps the agent's reply channel alive for the shell's lifetime.
    _agent: Option<PairingAgent>,
    window: Option<WindowHandle<PairingDialog>>,
    queue: VecDeque<(PairingRequest, PairingAgent)>,
    pub status: Option<AgentStatus>,
}
impl Global for PairingUi {}

/// Starts the agent (when `bluetooth.pairing_agent`) and wires it to the dialog.
pub fn init(cx: &mut App) {
    let enabled = crate::state::config(cx).read(cx).get().bluetooth.pairing_agent;
    let agent = bt_agent::spawn_pairing_agent(enabled);
    cx.set_global(PairingUi { _agent: Some(agent.clone()), ..Default::default() });
    pump(agent, cx);
}

fn pump(agent: PairingAgent, cx: &mut App) {
    let events = agent.events.clone();
    cx.spawn(async move |cx| {
        while let Ok(event) = events.recv().await {
            let agent = agent.clone();
            cx.update(|cx| handle_event(event, agent, cx));
        }
    })
    .detach();
}

fn handle_event(event: PairingEvent, agent: PairingAgent, cx: &mut App) {
    if !cx.has_global::<PairingUi>() {
        cx.set_global(PairingUi::default());
    }
    match event {
        PairingEvent::Status(s) => cx.global_mut::<PairingUi>().status = Some(s),
        PairingEvent::Request(req) => {
            cx.global_mut::<PairingUi>().queue.push_back((req, agent));
            show_next(cx);
        }
        PairingEvent::Cancelled(id) => {
            cx.global_mut::<PairingUi>().queue.retain(|(r, _)| r.id != id);
            if let Some(handle) = cx.global::<PairingUi>().window {
                let _ = handle.update(cx, |d, _, cx| {
                    if d.request.id == id {
                        d.close(cx);
                    }
                });
            }
        }
    }
}

fn is_open(handle: &Option<WindowHandle<PairingDialog>>, cx: &App) -> bool {
    handle.as_ref().is_some_and(|h| cx.windows().iter().any(|w| w.window_id() == h.window_id()))
}

fn output_size(cx: &App) -> (f32, f32) {
    cx.try_global::<crate::overlays::Outputs>()
        .and_then(|o| o.0.first().map(|g| (g.width as f32, g.height as f32)))
        .or_else(|| mandara_core::query_outputs().ok().and_then(|o| o.first().map(|g| (g.width as f32, g.height as f32))))
        .unwrap_or(FALLBACK_OUTPUT)
}

fn show_next(cx: &mut App) {
    if is_open(&cx.global::<PairingUi>().window, cx) {
        return;
    }
    let Some((request, agent)) = cx.global_mut::<PairingUi>().queue.pop_front() else { return };
    let (w, h) = output_size(cx);
    let mut spec = LayerSurfaceSpec::overlay(SurfaceRole::Lock, gpui::size(px(w), px(h)), None).with_keyboard(KeyboardInteractivity::Exclusive);
    spec.layer = Layer::Overlay;
    spec.anchor = Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT;
    let handle = cx
        .open_window(spec.window_options(), move |window, cx| cx.new(|cx| PairingDialog::new(request, agent, (w, h), window, cx)))
        .map_err(|e| tracing::error!("pairing dialog: {e:#}"))
        .ok();
    cx.global_mut::<PairingUi>().window = handle;
}

/// A readable name for common Bluetooth service UUIDs.
pub fn service_name(uuid: &str) -> String {
    let short = uuid.get(4..8).unwrap_or(uuid).to_ascii_lowercase();
    let name = match short.as_str() {
        "110a" | "110b" | "110d" => tk("Audio streaming"),
        "110c" | "110e" | "110f" => tk("Media remote control"),
        "1108" | "111e" | "111f" | "1112" => tk("Calls (headset)"),
        "1124" | "1812" => tk("Keyboard / mouse input"),
        "1105" | "1106" => tk("File transfer"),
        "112f" => tk("Phonebook access"),
        "1132" | "1133" | "1134" => tk("Messages"),
        "1115" | "1116" | "1117" => tk("Network access"),
        "180f" => tk("Battery level"),
        _ => return uuid.to_string(),
    };
    mandara_i18n::tr(name)
}

/// The text typed so far is acceptable for the request.
fn input_ok(kind: &PairingKind, input: &str) -> bool {
    match kind {
        PairingKind::EnterPin => bt_agent::valid_pin(input),
        PairingKind::EnterPasskey => bt_agent::parse_passkey(input).is_some(),
        _ => true,
    }
}

pub struct PairingDialog {
    request: PairingRequest,
    agent: PairingAgent,
    input: String,
    focus: FocusHandle,
    appear: SpringValue<f32>,
    clock: FrameClock,
    closing: bool,
    answered: bool,
    expected: (f32, f32),
}

impl PairingDialog {
    fn new(request: PairingRequest, agent: PairingAgent, expected: (f32, f32), window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&mandara_core::theme_entity(cx), |_, _, cx| cx.notify()).detach();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let mut appear = SpringValue::new(0.0, APPEAR);
        appear.set_target(1.0);
        Self { request, agent, input: String::new(), focus, appear, clock: FrameClock::new(), closing: false, answered: false, expected }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        cx.notify();
    }

    fn answer(&mut self, accept: bool, cx: &mut Context<Self>) {
        if self.answered {
            return;
        }
        let id = self.request.id;
        let reply = match (&self.request.kind, accept) {
            (_, false) => PairingReply::Reject(id),
            (PairingKind::EnterPin, true) if input_ok(&self.request.kind, &self.input) => PairingReply::Pin(id, self.input.clone()),
            (PairingKind::EnterPasskey, true) => match bt_agent::parse_passkey(&self.input) {
                Some(p) => PairingReply::Passkey(id, p),
                None => return,
            },
            (PairingKind::EnterPin, true) => return,
            (kind, true) if kind.is_display() => {
                // Nothing to answer: just hide it.
                self.answered = true;
                return self.close(cx);
            }
            (_, true) => PairingReply::Accept(id),
        };
        self.answered = true;
        self.agent.reply(reply);
        self.close(cx);
    }

    fn on_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        match k.key.as_str() {
            "escape" => return self.answer(false, cx),
            "enter" => return self.answer(true, cx),
            "backspace" => {
                self.input.pop();
            }
            _ => {
                let typing = matches!(self.request.kind, PairingKind::EnterPin | PairingKind::EnterPasskey);
                if let Some(ch) = k.key_char.as_ref().filter(|_| typing && !k.modifiers.control && !k.modifiers.alt) {
                    let max = if self.request.kind == PairingKind::EnterPasskey { PASSKEY_DIGITS } else { PIN_MAX_CHARS };
                    let digits_only = self.request.kind == PairingKind::EnterPasskey;
                    if self.input.chars().count() < max && (!digits_only || ch.chars().all(|c| c.is_ascii_digit())) {
                        self.input.push_str(ch);
                    }
                }
            }
        }
        cx.notify();
    }
}

impl Drop for PairingDialog {
    fn drop(&mut self) {
        // Closed without an answer (e.g. the shell exits): reject.
        if !self.answered && !self.request.kind.is_display() {
            self.agent.reply(PairingReply::Reject(self.request.id));
        }
    }
}

fn shadow(color: gpui::Hsla, blur: f32, y: f32) -> BoxShadow {
    BoxShadow { color, offset: point(px(0.0), px(y)), blur_radius: px(blur), spread_radius: px(0.0), inset: false }
}

fn device_icon(bluez_icon: &str) -> &'static str {
    match bluez_icon {
        i if i.starts_with("audio") => "headphones",
        i if i.starts_with("input-keyboard") => "keyboard",
        i if i.starts_with("input") => "crosshair",
                _ => "bluetooth",
    }
}

/// Six big digit cells (`entered` of them highlighted while the other
/// device types them).
fn code_cells(code: &str, entered: Option<usize>, p: &MandaraPalette, mono: gpui::SharedString) -> impl IntoElement {
    div().flex().justify_center().gap(px(CODE_GAP_PX)).children(code.chars().enumerate().map(move |(i, c)| {
        let done = entered.is_some_and(|n| i < n);
        div()
            .w(px(CODE_CELL_W_PX))
            .h(px(CODE_CELL_H_PX))
            .rounded(px(crate::style::radius(CODE_CELL_RADIUS_PX)))
            .flex()
            .items_center()
            .justify_center()
            .bg(if done { p.primary.with_alpha(TINT_ALPHA).hsla() } else { p.surface_container_high.hsla() })
            .border_1()
            .border_color(if done { p.primary.with_alpha(TINT_BORDER_ALPHA).hsla() } else { p.outline.with_alpha(OUTLINE_ALPHA).hsla() })
            .font_family(mono.clone())
            .font_features(crate::fonts::tabular_nums())
            .text_size(px(CODE_DIGIT_PX))
            .font_weight(FontWeight::SEMIBOLD)
            .child(c.to_string())
    }))
}

impl Render for PairingDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vp = window.viewport_size();
        let settled = (f32::from(vp.width) - self.expected.0).abs() < SETTLE_EPSILON_PX && (f32::from(vp.height) - self.expected.1).abs() < SETTLE_EPSILON_PX;
        if !settled && !self.closing {
            window.request_animation_frame();
            return div().id("bt-pair-root").size_full().track_focus(&self.focus).into_any_element();
        }
        let p = mandara_core::frame_palette(window, cx);
        let f = crate::fonts::fonts(cx);
        self.appear.set_config(if self.closing { SpringConfig::critical(DISAPPEAR_STIFFNESS) } else { APPEAR });
        self.appear.set_target(if self.closing { 0.0 } else { 1.0 });
        let moving = self.appear.step(self.clock.tick(Instant::now()));
        if self.closing && self.appear.value() <= GONE {
            window.remove_window();
            cx.defer(show_next);
        } else if moving {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        let shown = self.appear.value().clamp(0.0, 1.0);
        let device = self.request.device.clone();
        let (title, body, accept, reject): (String, String, String, String) = match &self.request.kind {
            PairingKind::Confirm { .. } => (t("Pair with this device?").into(), tf("Check that {device} shows the same code.", &[("device", &device)]), t("Pair").into(), t("Doesn't match").into()),
            PairingKind::EnterPin => (t("Enter the PIN").into(), tf("Type the PIN for {device} (often 0000 or 1234, or shown on its screen).", &[("device", &device)]), t("Pair").into(), t("Cancel").into()),
            PairingKind::EnterPasskey => (t("Enter the passkey").into(), tf("Type the six-digit code shown on {device}.", &[("device", &device)]), t("Pair").into(), t("Cancel").into()),
            PairingKind::DisplayPin { .. } | PairingKind::DisplayPasskey { .. } => {
                (t("Type this code on the device").into(), tf("Enter this code on {device}, then press Enter on it.", &[("device", &device)]), t("Hide").into(), t("Cancel").into())
            }
            PairingKind::Authorize => (t("Allow this device?").into(), tf("{device} wants to pair with this computer.", &[("device", &device)]), t("Allow").into(), t("Deny").into()),
            PairingKind::AuthorizeService { uuid } => (
                t("Allow this connection?").into(),
                tf("{device} wants to use: {service}.", &[("device", &device), ("service", &service_name(uuid))]),
                t("Allow").into(),
                t("Deny").into(),
            ),
        };
        let header = div()
            .flex()
            .items_center()
            .gap(px(HEADER_GAP_PX))
            .child(
                div()
                    .size(px(DISC_PX))
                    .flex_none()
                    .rounded_full()
                    .bg(p.primary.with_alpha(TINT_ALPHA).hsla())
                    .border_1()
                    .border_color(p.primary.with_alpha(TINT_BORDER_ALPHA).hsla())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(device_icon(&self.request.icon), DISC_ICON_PX, p.primary)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(div().text_size(px(TITLE_PX)).font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(div().text_size(px(SMALL_PX)).text_color(p.on_surface_variant.hsla()).font_family(f.mono.clone()).child(format!("{device} · {}", self.request.address))),
            );
        let middle = match &self.request.kind {
            PairingKind::Confirm { passkey } => Some(code_cells(&bt_agent::format_passkey(*passkey), None, &p, f.mono.clone()).into_any_element()),
            PairingKind::DisplayPasskey { passkey, entered } => Some(code_cells(&bt_agent::format_passkey(*passkey), Some(*entered as usize), &p, f.mono.clone()).into_any_element()),
            PairingKind::DisplayPin { pin } => Some(code_cells(pin, None, &p, f.mono.clone()).into_any_element()),
            PairingKind::EnterPin | PairingKind::EnterPasskey => {
                let placeholder = if self.request.kind == PairingKind::EnterPasskey { t("6 digits") } else { t("PIN") };
                Some(
                    div()
                        .h(px(PILL_H_PX))
                        .px(px(BUTTON_PAD_PX))
                        .rounded(px(crate::style::radius(PILL_RADIUS_PX)))
                        .bg(p.surface_container_high.hsla())
                        .border_1()
                        .border_color(p.outline.with_alpha(OUTLINE_ALPHA).hsla())
                        .shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
                        .flex()
                        .items_center()
                        .gap(px(GAP_PX))
                        .font_family(f.mono.clone())
                        .font_features(crate::fonts::tabular_nums())
                        .child(icon("keyboard", ICON_PX, p.on_surface_variant))
                        .child(if self.input.is_empty() {
                            div().text_color(p.on_surface_variant.with_alpha(HINT_ALPHA).hsla()).child(placeholder).into_any_element()
                        } else {
                            div().child(self.input.clone()).into_any_element()
                        })
                        .into_any_element(),
                )
            }
            _ => None,
        };
        let ready = input_ok(&self.request.kind, &self.input) && (!matches!(self.request.kind, PairingKind::EnterPin | PairingKind::EnterPasskey) || !self.input.is_empty());
        let primary = div()
            .id("bt-pair-accept")
            .h(px(BUTTON_H_PX))
            .px(px(BUTTON_PAD_PX))
            .rounded(px(crate::style::radius(BUTTON_RADIUS_PX)))
            .flex()
            .items_center()
            .font_weight(FontWeight::SEMIBOLD)
            .text_size(px(BODY_PX))
            .text_color(p.on_primary.hsla())
            .bg(mandara_core::oklab_gradient(GRADIENT_DEG, p.accent_gradient_start, p.accent_gradient_end))
            .shadow(vec![shadow(p.primary_glow.with_alpha(GLOW_ALPHA).hsla(), GLOW_BLUR_PX, GLOW_Y_PX)])
            .opacity(if ready { 1.0 } else { DISABLED_ALPHA })
            .cursor_pointer()
            .child(accept)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.answer(true, cx)));
        let secondary = div()
            .id("bt-pair-reject")
            .h(px(BUTTON_H_PX))
            .px(px(BUTTON_PAD_PX))
            .rounded(px(crate::style::radius(BUTTON_RADIUS_PX)))
            .flex()
            .items_center()
            .text_size(px(BODY_PX))
            .bg(p.surface_container_high.hsla())
            .hover(|s| s.opacity(HINT_ALPHA))
            .cursor_pointer()
            .child(reject)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.answer(false, cx)));
        let mut card_shadows = mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow);
        card_shadows.push(shadow(Color::hex("#000000").with_alpha(SHADOW_ALPHA).hsla(), SHADOW_BLUR_PX, SHADOW_Y_PX));
        let display_only = self.request.kind.is_display();
        let card = div()
            .id("bt-pair-card")
            .occlude()
            .w(px(CARD_W_PX))
            .mt(px((1.0 - shown) * RISE_PX))
            .opacity(shown)
            .p(px(CARD_PAD_PX))
            .rounded(px(crate::style::radius(CARD_RADIUS_PX)))
            .bg(p.surface_container.with_alpha(CARD_ALPHA).hsla())
            .border_1()
            .border_color(p.outline.with_alpha(OUTLINE_ALPHA).hsla())
            .shadow(card_shadows)
            .flex()
            .flex_col()
            .gap(px(CARD_GAP_PX))
            .child(header)
            .child(div().text_size(px(BODY_PX)).line_height(px(BODY_LEADING_PX)).child(crate::bidi::paragraph(body)))
            .children(middle)
            .child(div().flex().justify_end().gap(px(GAP_SMALL_PX)).when(!display_only, |el| el.child(secondary)).child(primary));
        div()
            .id("bt-pair-root")
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.on_key(e, cx)))
            .font_family(f.ui.clone())
            .text_color(p.on_surface.hsla())
            .bg(p.surface_dim.with_alpha(SCRIM_ALPHA * shown).hsla())
            .flex()
            .items_center()
            .justify_center()
            .child(card)
            .into_any_element()
    }
}

/// Icon size inside the input pill.
const ICON_PX: f32 = 15.0;

fn status_json(cx: &App) -> serde_json::Value {
    match cx.try_global::<PairingUi>().and_then(|u| u.status.clone()) {
        Some(AgentStatus::Registered) => json!({ "agent": "registered" }),
        Some(AgentStatus::Disabled) => json!({ "agent": "disabled" }),
        Some(AgentStatus::Unavailable(e)) => json!({ "agent": "unavailable", "detail": e }),
        None => json!({ "agent": "starting" }),
    }
}

const DEMO_KINDS: &[&str] = &["confirm", "pin", "passkey", "display", "authorize", "service"];

pub fn register(r: &mut PluginRegistry) {
    r.command("bluetooth", CommandSpec::new("bluetooth agent", tk("Whether Mandara's pairing agent is registered with BlueZ"), |_, cx| Ok(status_json(cx))));
    r.command(
        "bluetooth",
        CommandSpec::new("bluetooth pairing-demo", tk("Preview the pairing dialog with a fake device (nothing is paired)"), |a, cx| {
            let kind = match a.str(0)? {
                "pin" => PairingKind::EnterPin,
                "passkey" => PairingKind::EnterPasskey,
                "display" => PairingKind::DisplayPasskey { passkey: 482_913, entered: 2 },
                "authorize" => PairingKind::Authorize,
                "service" => PairingKind::AuthorizeService { uuid: "0000110d-0000-1000-8000-00805f9b34fb".into() },
                _ => PairingKind::Confirm { passkey: 482_913 },
            };
            let agent = bt_agent::demo_agent(kind);
            pump(agent, cx);
            Ok(json!({ "demo": a.str(0)? }))
        })
        .arg(ArgSpec::required("kind", ArgKind::Enum, tk("which prompt")).choices(DEMO_KINDS)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_names_and_input_rules() {
        assert_eq!(service_name("0000110d-0000-1000-8000-00805f9b34fb"), "Audio streaming");
        assert_eq!(service_name("weird"), "weird");
        assert!(input_ok(&PairingKind::EnterPasskey, "123456"));
        assert!(!input_ok(&PairingKind::EnterPasskey, "12a"));
        assert!(input_ok(&PairingKind::EnterPin, "0000"));
        assert!(input_ok(&PairingKind::Authorize, ""));
        assert_eq!(device_icon("audio-headset"), "headphones");
        assert_eq!(device_icon("input-keyboard"), "keyboard");
    }
}
