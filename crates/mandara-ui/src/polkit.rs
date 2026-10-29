//! The polkit authentication dialog: a modal overlay (exclusive keyboard,
//! dimmed scrim) that springs up when an app needs administrator rights.
//!
//! It shows the action's message and icon, who is authenticating (a chip
//! picker when several admins qualify — Tab cycles), a chromatic-glass
//! password pill fed by the helper's own prompt, and Cancel / Authenticate.
//! A wrong password shakes the pill and flashes it coral; the helper retries
//! with a fresh prompt. The agent itself lives in
//! [`mandara_services::polkit`]; requests queue up and show one at a time.
//!
//! `mandara msg polkit demo` opens the dialog with a fake request (any
//! password but "wrong" succeeds) so it can be previewed safely.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use gpui::{
    App, AppContext as _, BoxShadow, ClickEvent, Context, FocusHandle, FontWeight, Global, InteractiveElement, IntoElement, KeyDownEvent,
    ObjectFit, ParentElement, Render, StatefulInteractiveElement, Styled, StyledImage as _, Window, WindowHandle, div, img, point,
    prelude::FluentBuilder as _, px,
};
use mandara_core::{FrameClock, LayerSurfaceSpec, SpringConfig, SpringValue, SurfaceRole, ToGpui};
use mandara_i18n::{t, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use mandara_services::polkit::{AdminUser, AgentStatus, AuthRequest, Outcome, PolkitAgent, PolkitEvent, PolkitReply};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use crate::bidi::Bidi as _;
use crate::icons::icon;

/// Text caret blink half-period (platform convention).
const CARET_BLINK: Duration = Duration::from_millis(530);
/// Fake password check of the polkit dialog demo.
const DEMO_CHECK_DELAY: Duration = Duration::from_millis(500);

/// Dialog geometry, shared with the keyring prompt. The card's width is
/// `general.auth_dialog_width`; corners follow the radius scale.
pub(crate) mod m {
    use crate::kit::{metrics, text, tokens};
    /// Card padding, corner radius and the gap between its blocks.
    pub const CARD_PAD: f32 = tokens::XXL - tokens::XS;
    pub const CARD_RADIUS: f32 = 26.0;
    pub const CARD_GAP: f32 = tokens::LG;
    /// How far the card rises while it appears.
    pub const APPEAR_RISE: f32 = 26.0;
    /// Header: icon disc, the icon inside it (theme / fallback glyph),
    /// gap to the titles and between them.
    pub const DISC: f32 = 52.0;
    pub const DISC_IMAGE: f32 = 30.0;
    pub const DISC_GLYPH: f32 = 24.0;
    pub const HEADER_GAP: f32 = 14.0;
    pub const TITLE_GAP: f32 = metrics::NUDGE;
    pub const TITLE: f32 = 17.0;
    pub const SUBTITLE: f32 = text::BODY_S;
    /// The request's message.
    pub const MESSAGE: f32 = 13.5;
    pub const MESSAGE_LEADING: f32 = tokens::XL;
    /// User chips (several admins): gap, padding, monogram, label.
    pub const CHIP_GAP: f32 = tokens::SM;
    pub const CHIP_PAD_START: f32 = tokens::XS;
    pub const CHIP_PAD_END: f32 = tokens::MD;
    pub const CHIP_PAD_Y: f32 = tokens::XS;
    pub const CHIP_MONOGRAM: f32 = 24.0;
    pub const CHIP_TEXT: f32 = 12.5;
    /// The single admin: monogram, gap, name and login.
    pub const USER_MONOGRAM: f32 = 30.0;
    pub const USER_GAP: f32 = tokens::SM + metrics::NUDGE;
    pub const USER_NAME: f32 = text::BODY;
    pub const USER_LOGIN: f32 = 11.5;
    /// Password pill: height, padding, radius, gap, text, lock glyph,
    /// hidden-character dots and the caret.
    pub const PILL_H: f32 = 46.0;
    pub const PILL_PAD: f32 = 18.0;
    pub const PILL_RADIUS: f32 = PILL_H / 2.0;
    pub const PILL_GAP: f32 = tokens::SM + metrics::NUDGE;
    pub const PILL_TEXT: f32 = 14.0;
    pub const PILL_GLYPH: f32 = 15.0;
    pub const DOT: f32 = tokens::SM;
    pub const DOT_GAP: f32 = 7.0;
    pub const CARET_W: f32 = 1.5;
    pub const CARET_H: f32 = 18.0;
    /// Pill shadow (blur, alpha, y offset).
    pub const PILL_SHADOW: (f32, f32, f32) = (14.0, 0.18, 4.0);
    /// Card shadow (blur, alpha, y offset).
    pub const CARD_SHADOW: (f32, f32, f32) = (48.0, 0.38, 18.0);
    /// Notes under the pill and the action id.
    pub const NOTE: f32 = 12.5;
    pub const NOTE_GAP: f32 = tokens::SM;
    pub const DETAILS: f32 = text::LABEL;
    /// Buttons: height, padding, radius, gap, label size, glow.
    pub const BUTTON_H: f32 = 40.0;
    pub const BUTTON_PAD: f32 = tokens::XL;
    pub const CANCEL_PAD: f32 = 18.0;
    pub const BUTTON_RADIUS: f32 = BUTTON_H / 2.0;
    pub const BUTTON_GAP: f32 = tokens::SM;
    pub const BUTTON_TEXT: f32 = 13.5;
    pub const BUTTONS_GAP: f32 = tokens::SM + metrics::NUDGE;
    pub const GLOW_Y: f32 = tokens::XS;
    pub const GLOW_BLUR: f32 = tokens::LG;
}
const SHAKE: SpringConfig = SpringConfig::new(620.0, 13.0, 1.0);
const APPEAR: SpringConfig = SpringConfig::new(300.0, 23.0, 1.0);

/// Shell-wide agent state.
#[derive(Default)]
pub struct PolkitUi {
    /// Keeps the live agent handle (the event pump holds a clone too).
    _agent: Option<PolkitAgent>,
    window: Option<WindowHandle<PolkitDialog>>,
    queue: VecDeque<(AuthRequest, PolkitAgent)>,
    pub status: Option<AgentStatus>,
}

impl Global for PolkitUi {}

fn is_open(handle: &Option<WindowHandle<PolkitDialog>>, cx: &App) -> bool {
    handle.as_ref().is_some_and(|h| cx.windows().iter().any(|w| w.window_id() == h.window_id()))
}

/// Starts the agent (when `general.polkit_agent`) and wires its events to
/// the dialog; switching the setting in Studio starts or stops it live.
pub fn init(cx: &mut App) {
    let mut enabled = crate::state::config(cx).read(cx).get().general.polkit_agent;
    let agent = mandara_services::polkit::spawn_polkit_agent(enabled);
    cx.set_global(PolkitUi { _agent: Some(agent.clone()), ..Default::default() });
    pump(agent, cx);
    cx.observe(&crate::state::config(cx), move |store, cx| {
        let want = store.read(cx).get().general.polkit_agent;
        if want != enabled {
            enabled = want;
            set_enabled(want, cx);
        }
    })
    .detach();
}

/// Replaces the running agent with a fresh one that is registered (`on`)
/// or disabled; the old one unregisters from polkit.
fn set_enabled(on: bool, cx: &mut App) {
    if let Some(old) = cx.try_global::<PolkitUi>().and_then(|ui| ui._agent.clone()) {
        old.stop();
    }
    let agent = mandara_services::polkit::spawn_polkit_agent(on);
    let ui = cx.default_global::<PolkitUi>();
    ui._agent = Some(agent.clone());
    ui.status = None;
    tracing::info!("polkit: authentication agent {}", if on { "starting" } else { "off" });
    pump(agent, cx);
}

/// Forwards one agent's events into the UI.
fn pump(agent: PolkitAgent, cx: &mut App) {
    let events = agent.events.clone();
    cx.spawn(async move |cx| {
        while let Ok(event) = events.recv().await {
            let agent = agent.clone();
            cx.update(|cx| handle_event(event, agent, cx));
        }
    })
    .detach();
}

fn handle_event(event: PolkitEvent, agent: PolkitAgent, cx: &mut App) {
    // A replaced agent's last words (its "stopped" status) must not
    // overwrite the new agent's state.
    if agent.is_stopped() {
        return;
    }
    if !cx.has_global::<PolkitUi>() {
        cx.set_global(PolkitUi::default());
    }
    match event {
        PolkitEvent::Status(status) => cx.global_mut::<PolkitUi>().status = Some(status),
        PolkitEvent::Begin(request) => {
            cx.global_mut::<PolkitUi>().queue.push_back((request, agent));
            show_next(cx);
        }
        PolkitEvent::Close { cookie, outcome } => {
            cx.global_mut::<PolkitUi>().queue.retain(|(r, _)| r.cookie != cookie);
            with_dialog(cx, &cookie, |d, cx| {
                d.closing = true;
                d.outcome = Some(outcome);
                cx.notify();
            });
            // The next request opens once this dialog has animated away.
        }
        other => {
            let cookie = match &other {
                PolkitEvent::Prompt { cookie, .. } | PolkitEvent::Message { cookie, .. } | PolkitEvent::Retry { cookie, .. } => cookie.clone(),
                _ => return,
            };
            with_dialog(cx, &cookie, move |d, cx| {
                d.on_event(other);
                cx.notify();
            });
        }
    }
}

fn with_dialog(cx: &mut App, cookie: &str, f: impl FnOnce(&mut PolkitDialog, &mut Context<PolkitDialog>)) {
    let handle = cx.global::<PolkitUi>().window;
    if let Some(handle) = handle {
        let cookie = cookie.to_string();
        let _ = handle.update(cx, |d, _, cx| {
            if d.request.cookie == cookie {
                f(d, cx);
            }
        });
    }
}

/// Opens the dialog for the oldest queued request, if none is showing.
fn show_next(cx: &mut App) {
    let open = is_open(&cx.global::<PolkitUi>().window, cx);
    if open {
        return;
    }
    let Some((request, agent)) = cx.global::<PolkitUi>().queue.front().cloned() else { return };
    let (w, h) = output_size(cx);
    let mut spec = LayerSurfaceSpec::overlay(SurfaceRole::Lock, gpui::size(px(w), px(h)), None).with_keyboard(KeyboardInteractivity::Exclusive);
    spec.layer = Layer::Overlay;
    spec.anchor = Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT;
    let handle = cx
        .open_window(spec.window_options(), move |window, cx| cx.new(|cx| PolkitDialog::new(request, agent, (w, h), window, cx)))
        .map_err(|e| tracing::error!("polkit dialog: {e:#}"))
        .ok();
    cx.global_mut::<PolkitUi>().window = handle;
}

fn output_size(cx: &App) -> (f32, f32) {
    cx.try_global::<crate::overlays::Outputs>()
        .and_then(|o| o.0.first().map(|g| (g.width as f32, g.height as f32)))
        .or_else(|| mandara_core::query_outputs().ok().and_then(|o| o.first().map(|g| (g.width as f32, g.height as f32))))
        .unwrap_or((1920.0, 1080.0))
}

// ─────────────────────────────── the dialog ───────────────────────────────

pub struct PolkitDialog {
    request: AuthRequest,
    agent: PolkitAgent,
    user_ix: usize,
    /// The helper's current question: `(text, echo)`.
    prompt: Option<(String, bool)>,
    input: String,
    /// Enter was pressed before the helper asked; send on the next prompt.
    pending_submit: bool,
    busy: bool,
    note: Option<(String, bool)>,
    focus: FocusHandle,
    appear: SpringValue<f32>,
    shake: SpringValue<f32>,
    flash: SpringValue<f32>,
    clock: FrameClock,
    closing: bool,
    outcome: Option<Outcome>,
    expected: (f32, f32),
    blink_from: Instant,
    _blink: gpui::Task<()>,
}

fn wipe(s: &mut String) {
    // SAFETY: zero bytes are valid UTF-8.
    unsafe { s.as_bytes_mut().fill(0) };
    s.clear();
}

impl PolkitDialog {
    fn new(request: AuthRequest, agent: PolkitAgent, expected: (f32, f32), window: &mut Window, cx: &mut Context<Self>) -> Self {
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
        Self {
            request,
            agent,
            user_ix: 0,
            prompt: None,
            input: String::with_capacity(256),
            pending_submit: false,
            busy: false,
            note: None,
            focus,
            appear,
            shake: SpringValue::new(0.0, SHAKE),
            flash: SpringValue::new(0.0, SpringConfig::critical(90.0)),
            clock: FrameClock::new(),
            closing: false,
            outcome: None,
            expected,
            blink_from: Instant::now(),
            _blink: blink,
        }
    }

    fn cookie(&self) -> String {
        self.request.cookie.clone()
    }

    fn on_event(&mut self, event: PolkitEvent) {
        match event {
            PolkitEvent::Prompt { text, echo, .. } => {
                self.prompt = Some((text, echo));
                self.busy = false;
                if self.pending_submit {
                    self.pending_submit = false;
                    self.submit();
                }
            }
            PolkitEvent::Message { text, error, .. } => self.note = Some((text.trim().to_string(), error)),
            PolkitEvent::Retry { message, .. } => {
                self.busy = false;
                self.prompt = None;
                wipe(&mut self.input);
                self.shake.add_velocity([-2400.0, 0.0, 0.0, 0.0]);
                self.flash.snap_to(1.0);
                self.flash.set_target(0.0);
                // Keep a module's own error (e.g. a lockout notice) if it came first.
                if !self.note.as_ref().is_some_and(|(_, error)| *error) {
                    self.note = Some((message, true));
                }
            }
            _ => {}
        }
    }

    fn submit(&mut self) {
        if self.busy || self.closing {
            return;
        }
        if self.input.is_empty() {
            self.shake.add_velocity([-1400.0, 0.0, 0.0, 0.0]);
            return;
        }
        if self.prompt.is_none() {
            self.pending_submit = true;
            self.busy = true;
            return;
        }
        let text = self.input.clone();
        wipe(&mut self.input);
        self.agent.reply(PolkitReply::Answer { cookie: self.cookie(), text });
        self.prompt = None;
        self.busy = true;
        self.note = None;
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.closing {
            self.agent.reply(PolkitReply::Cancel { cookie: self.cookie() });
            self.closing = true;
            self.outcome = Some(Outcome::Cancelled);
            cx.notify();
        }
    }

    fn select_user(&mut self, ix: usize) {
        if ix == self.user_ix || ix >= self.request.users.len() || self.closing {
            return;
        }
        self.user_ix = ix;
        self.prompt = None;
        self.busy = false;
        self.pending_submit = false;
        self.note = None;
        wipe(&mut self.input);
        let user = self.request.users[ix].name.clone();
        self.agent.reply(PolkitReply::SelectUser { cookie: self.cookie(), user });
    }

    fn on_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        self.blink_from = Instant::now();
        match k.key.as_str() {
            "escape" => return self.cancel(cx),
            "enter" => self.submit(),
            "tab" if self.request.users.len() > 1 => {
                let n = self.request.users.len();
                let next = if k.modifiers.shift { (self.user_ix + n - 1) % n } else { (self.user_ix + 1) % n };
                self.select_user(next);
            }
            "backspace" if !self.busy => {
                if k.modifiers.control {
                    wipe(&mut self.input);
                } else {
                    self.input.pop();
                }
            }
            "u" if k.modifiers.control && !self.busy => wipe(&mut self.input),
            _ => {
                if let Some(ch) = k.key_char.as_ref().filter(|_| !k.modifiers.control && !k.modifiers.alt && !self.busy)
                    && self.input.len() + ch.len() <= 255
                {
                    self.input.push_str(ch);
                    if self.note.as_ref().is_some_and(|(_, error)| !error) {
                        self.note = None;
                    }
                }
            }
        }
        cx.notify();
    }
}

fn monogram(user: &AdminUser, size: f32, p: &MandaraPalette) -> impl IntoElement {
    let letter: String = user.display.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .bg(mandara_core::oklab_gradient(135.0, p.accent_gradient_start, p.accent_gradient_end))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(size * 0.46))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.on_primary.hsla())
        .child(letter)
}

/// The card width of the polkit and keyring dialogs (`general.auth_dialog_width`).
pub(crate) fn auth_dialog_width(cfg: &mandara_config::ShellConfig) -> f32 {
    cfg.general.auth_dialog_width()
}

fn soft_shadow(blur: f32, alpha: f32, y: f32) -> BoxShadow {
    BoxShadow { color: Color::hex("#000000").with_alpha(alpha).hsla(), offset: point(px(0.0), px(y)), blur_radius: px(blur), spread_radius: px(0.0), inset: false }
}

impl Render for PolkitDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vp = window.viewport_size();
        let settled = (f32::from(vp.width) - self.expected.0).abs() < 2.0 && (f32::from(vp.height) - self.expected.1).abs() < 2.0;
        if !settled && !self.closing {
            window.request_animation_frame();
            return div().id("polkit-root").size_full().track_focus(&self.focus).into_any_element();
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
            // Let the next queued request open.
            cx.defer(|cx| show_next(cx));
        } else if moving || self.busy {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        let a = self.appear.value();
        let shown = a.clamp(0.0, 1.0);
        let scale_y = (1.0 - a) * m::APPEAR_RISE;
        let card_w = auth_dialog_width(crate::state::config(cx).read(cx).get());

        // Header: action icon (theme icon when available) in a tinted disc.
        let icon_el = match crate::icon_theme::resolve(&self.request.icon_name, None, false).filter(|_| !self.request.icon_name.is_empty()) {
            Some(resolved) => img(resolved.path).size(px(m::DISC_IMAGE)).object_fit(ObjectFit::Contain).into_any_element(),
            None => icon("lock", m::DISC_GLYPH, p.primary).into_any_element(),
        };
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
                    .child(icon_el),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(m::TITLE_GAP))
                    .min_w_0()
                    .child(div().text_size(px(m::TITLE)).font_weight(FontWeight::SEMIBOLD).child(t("Authentication required")))
                    .child(div().text_size(px(m::SUBTITLE)).text_color(p.on_surface_variant.hsla()).child(t("An app needs administrator rights"))),
            );

        let message = div().text_size(px(m::MESSAGE)).line_height(px(m::MESSAGE_LEADING)).text_color(p.on_surface.with_alpha(0.86).hsla()).child(crate::bidi::paragraph(self.request.message.clone()));

        // Who authenticates.
        let users = self.request.users.clone();
        let identity = if users.len() > 1 {
            div()
                .row()
                .flex_wrap()
                .gap(px(m::CHIP_GAP))
                .children(users.iter().enumerate().map(|(i, u)| {
                    let selected = i == self.user_ix;
                    div()
                        .id(("polkit-user", i))
                        .row()
                        .items_center()
                        .gap(px(m::CHIP_GAP))
                        .pad_start(px(m::CHIP_PAD_START))
                        .pad_end(px(m::CHIP_PAD_END))
                        .py(px(m::CHIP_PAD_Y))
                        .rounded_full()
                        .cursor_pointer()
                        .bg(if selected { p.primary.with_alpha(0.16).hsla() } else { p.surface_container_high.with_alpha(0.6).hsla() })
                        .border_1()
                        .border_color(if selected { p.primary.with_alpha(0.55).hsla() } else { p.outline.with_alpha(0.18).hsla() })
                        .child(monogram(u, m::CHIP_MONOGRAM, &p))
                        .child(div().text_size(px(m::CHIP_TEXT)).font_weight(if selected { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }).child(u.display.clone()))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.select_user(i);
                            cx.notify();
                        }))
                }))
                .into_any_element()
        } else if let Some(u) = users.first() {
            div()
                .row()
                .items_center()
                .gap(px(m::USER_GAP))
                .child(monogram(u, m::USER_MONOGRAM, &p))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(m::USER_NAME)).font_weight(FontWeight::SEMIBOLD).child(u.display.clone()))
                        .child(div().text_size(px(m::USER_LOGIN)).text_color(p.on_surface_variant.hsla()).child(u.name.clone())),
                )
                .into_any_element()
        } else {
            div().into_any_element()
        };

        // Password pill.
        let echo = self.prompt.as_ref().is_some_and(|(_, e)| *e);
        let placeholder = self.prompt.as_ref().map(|(text, _)| text.trim().trim_end_matches(':').to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| t("Password").into());
        let caret_on = ((self.blink_from.elapsed().as_millis() / 530) % 2 == 0) && !self.busy;
        let content = if self.input.is_empty() {
            div().text_color(p.on_surface_variant.with_alpha(0.7).hsla()).child(placeholder).into_any_element()
        } else if echo {
            div().child(self.input.clone()).into_any_element()
        } else {
            let n = self.input.chars().count().min(28);
            div().flex().items_center().gap(px(m::DOT_GAP)).children((0..n).map(|_| div().size(px(m::DOT)).rounded_full().bg(p.on_surface.hsla()))).into_any_element()
        };
        let flash = self.flash.value().clamp(0.0, 1.0);
        let rim = p.urgent_coral.with_alpha(flash).hsla();
        let mut pill_shadows = mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow);
        pill_shadows.push(soft_shadow(m::PILL_SHADOW.0, m::PILL_SHADOW.1, m::PILL_SHADOW.2));
        let pill = div()
            .ml(px(self.shake.value()))
            .h(px(m::PILL_H))
            .px(px(m::PILL_PAD))
            .rounded(px(crate::style::radius(m::PILL_RADIUS)))
            .bg(p.surface_container_high.with_alpha(0.75).hsla())
            .border_1()
            .border_color(if flash > 0.02 { rim } else { p.outline.with_alpha(0.16).hsla() })
            .shadow(pill_shadows)
            .row()
            .items_center()
            .gap(px(m::PILL_GAP))
            .text_size(px(m::PILL_TEXT))
            .child(icon("lock", m::PILL_GLYPH, p.on_surface_variant))
            .child({
                let empty = self.input.is_empty();
                let caret = || div().w(px(m::CARET_W)).h(px(m::CARET_H)).bg(p.primary.hsla());
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .when(caret_on && empty, |d| d.child(caret()))
                    .child(content)
                    .when(caret_on && echo && !empty, |d| d.child(caret()))
            });

        let note = self.note.clone().map(|(text, error)| {
            div().text_size(px(m::NOTE)).text_color(if error { p.urgent_coral.hsla() } else { p.on_surface_variant.hsla() }).child(text)
        });

        let details = div()
            .text_size(px(m::DETAILS))
            .font_family(f.mono.clone())
            .text_color(p.on_surface_variant.with_alpha(0.6).hsla())
            .overflow_hidden()
            .child(self.request.action_id.clone());

        let busy = self.busy;
        let ready = !self.input.is_empty() && !busy;
        let authenticate = div()
            .id("polkit-authenticate")
            .h(px(m::BUTTON_H))
            .px(px(m::BUTTON_PAD))
            .rounded(px(crate::style::radius(m::BUTTON_RADIUS)))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(m::BUTTON_GAP))
            .font_weight(FontWeight::SEMIBOLD)
            .text_size(px(m::BUTTON_TEXT))
            .text_color(p.on_primary.hsla())
            .bg(mandara_core::oklab_gradient(135.0, p.accent_gradient_start, p.accent_gradient_end))
            .shadow(vec![BoxShadow { color: p.primary_glow.with_alpha(0.35).hsla(), offset: point(px(0.0), px(m::GLOW_Y)), blur_radius: px(m::GLOW_BLUR), spread_radius: px(0.0), inset: false }])
            .opacity(if ready || busy { 1.0 } else { 0.55 })
            .cursor_pointer()
            .child(if busy { t("Checking…") } else { t("Authenticate") })
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.submit();
                cx.notify();
            }));
        let cancel = div()
            .id("polkit-cancel")
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
            .child(t("Cancel"))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.cancel(cx)));

        let mut card_shadows = mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow);
        card_shadows.push(soft_shadow(m::CARD_SHADOW.0, m::CARD_SHADOW.1, m::CARD_SHADOW.2));
        let card = div()
            .id("polkit-card")
            .occlude()
            .w(px(card_w))
            .max_w(window.viewport_size().width - px(m::CARD_PAD * 2.0))
            .mt(px(scale_y))
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
            .child(message)
            .child(identity)
            .child(div().flex().flex_col().gap(px(m::NOTE_GAP)).child(pill).children(note))
            .child(details)
            .child(div().row().justify_end().gap(px(m::BUTTONS_GAP)).child(cancel).child(authenticate));

        div()
            .id("polkit-root")
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.on_key(e, cx)))
            .font_family(f.ui.clone())
            .text_start()
            .text_color(p.on_surface.hsla())
            .bg(p.surface_dim.with_alpha(0.52 * shown).hsla())
            .flex()
            .items_center()
            .justify_center()
            .child(card)
            .into_any_element()
    }
}

// ─────────────────────────────── commands ───────────────────────────────

/// A fake agent for `polkit demo`: "wrong" fails, anything else succeeds.
fn demo_agent() -> (PolkitAgent, AuthRequest) {
    let (etx, erx) = async_channel::unbounded();
    let (rtx, rrx) = async_channel::unbounded::<PolkitReply>();
    let agent = PolkitAgent::from_channels(erx, rtx);
    let me = std::env::var("USER").unwrap_or_else(|_| "you".into());
    let request = AuthRequest {
        cookie: format!("demo-{}", std::process::id()),
        action_id: "org.mandara.demo.preview".into(),
        message: t("Authentication is needed to preview Mandara's polkit dialog (nothing will run)").into(),
        icon_name: String::new(),
        details: Vec::new(),
        users: vec![AdminUser { uid: 1000, name: me.clone(), display: me }, AdminUser { uid: 0, name: "root".into(), display: "root".into() }],
    };
    let cookie = request.cookie.clone();
    let user = request.users[0].name.clone();
    let _ = std::thread::Builder::new().name("mandara-polkit-demo".into()).spawn(move || {
        let prompt = |user: &str| PolkitEvent::Prompt { cookie: cookie.clone(), user: user.into(), text: "Password: ".into(), echo: false };
        let _ = etx.send_blocking(prompt(&user));
        while let Ok(reply) = rrx.recv_blocking() {
            match reply {
                PolkitReply::Answer { text, .. } => {
                    std::thread::sleep(DEMO_CHECK_DELAY);
                    if text == "wrong" {
                        let _ = etx.send_blocking(PolkitEvent::Retry { cookie: cookie.clone(), message: t("That didn't work — try again").into() });
                        let _ = etx.send_blocking(prompt(&user));
                    } else {
                        let _ = etx.send_blocking(PolkitEvent::Close { cookie: cookie.clone(), outcome: Outcome::Authorized });
                        break;
                    }
                }
                PolkitReply::SelectUser { user: next, .. } => {
                    let _ = etx.send_blocking(prompt(&next));
                }
                PolkitReply::Cancel { .. } => {
                    let _ = etx.send_blocking(PolkitEvent::Close { cookie: cookie.clone(), outcome: Outcome::Cancelled });
                    break;
                }
            }
        }
    });
    (agent, request)
}

/// Opens the dialog with a fake request (safe preview).
pub fn open_demo(cx: &mut App) {
    let (agent, request) = demo_agent();
    pump(agent.clone(), cx);
    handle_event(PolkitEvent::Begin(request), agent, cx);
}

/// Closes whatever dialog is showing (cancelling its request).
pub fn close(cx: &mut App) -> bool {
    let Some(handle) = cx.try_global::<PolkitUi>().and_then(|u| u.window) else { return false };
    handle.update(cx, |d, _, cx| d.cancel(cx)).is_ok()
}

fn status_json(cx: &App) -> serde_json::Value {
    let status = cx.try_global::<PolkitUi>().and_then(|u| u.status.clone());
    match status {
        Some(AgentStatus::Registered { session }) => json!({ "agent": "registered", "session": session }),
        Some(AgentStatus::AnotherAgent(msg)) => json!({ "agent": "another-agent", "detail": msg }),
        Some(AgentStatus::Disabled) => json!({ "agent": "disabled" }),
        Some(AgentStatus::Unavailable(msg)) => json!({ "agent": "unavailable", "detail": msg }),
        None => json!({ "agent": "starting" }),
    }
}

/// `polkit status | demo | close`.
pub fn register(r: &mut PluginRegistry) {
    r.command(
        "polkit",
        CommandSpec::new("polkit", tk("Polkit agent status; demo previews the authentication dialog; close cancels it"), |a, cx| match a.str(0)? {
            "demo" => {
                open_demo(cx);
                Ok(json!({ "demo": true }))
            }
            "close" => Ok(json!({ "closed": close(cx) })),
            _ => Ok(status_json(cx)),
        })
        .arg(ArgSpec::required("action", ArgKind::Enum, tk("what to do")).choices(&["status", "demo", "close"]))
        .example("polkit status")
        .example("polkit demo"),
    );
}
