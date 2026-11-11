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
//!
//! The same dialog answers the AUR step's `sudo` during a system update
//! ([`mandara_services::sysupdate::askpass`]): its requests come through
//! [`DialogPrompter`] instead of polkit, and the typed password goes back
//! to the askpass, never through polkit or a file.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use gpui::{
    App, AppContext as _, BoxShadow, ClickEvent, Context, FocusHandle, FontWeight, Global, InteractiveElement, IntoElement, KeyDownEvent,
    ObjectFit, ParentElement, Render, StatefulInteractiveElement, Styled, StyledImage as _, Window, WindowHandle, div, img, point,
    prelude::FluentBuilder as _, px,
};
use mandara_core::{FrameClock, LayerSurfaceSpec, SpringConfig, SpringValue, SurfaceRole, ToGpui};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use mandara_services::polkit::{AdminUser, AgentStatus, AuthRequest, Outcome, PolkitAgent, PolkitEvent, PolkitReply};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use crate::bidi::Bidi as _;
use crate::first_frame::{self, FirstFrame};
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
    queue: VecDeque<Queued>,
    pub status: Option<AgentStatus>,
}

/// A request waiting for (or shown in) the dialog.
struct Queued {
    request: AuthRequest,
    agent: PolkitAgent,
    early: Early,
}

/// What the helper said before the request's dialog opened: it asks
/// right away, often while the previous request's dialog is still fading
/// out (Studio's "refresh the helper, then apply" asks twice in a row).
/// Dropping that prompt left the next dialog waiting for one forever.
#[derive(Default)]
struct Early {
    prompt: Option<PolkitEvent>,
    note: Option<PolkitEvent>,
}

impl Early {
    fn keep(&mut self, event: PolkitEvent) {
        match event {
            PolkitEvent::Prompt { .. } => self.prompt = Some(event),
            PolkitEvent::Message { .. } => self.note = Some(event),
            // A fresh prompt follows a retry; the old one is spent.
            PolkitEvent::Retry { .. } => self.prompt = None,
            _ => {}
        }
    }

    /// In arrival order: a module's message comes before its prompt.
    fn take(&mut self) -> [Option<PolkitEvent>; 2] {
        [self.note.take(), self.prompt.take()]
    }
}

/// Keeps `event` for its queued request when no dialog shows it yet.
fn stash(queue: &mut VecDeque<Queued>, cookie: &str, event: PolkitEvent) {
    if let Some(q) = queue.iter_mut().find(|q| q.request.cookie == cookie) {
        q.early.keep(event);
    }
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
    init_askpass(cx);
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
            cx.global_mut::<PolkitUi>().queue.push_back(Queued { request, agent, early: Early::default() });
            show_next(cx);
        }
        PolkitEvent::Close { cookie, outcome } => {
            cx.global_mut::<PolkitUi>().queue.retain(|q| q.request.cookie != cookie);
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
            let mut event = Some(other);
            with_dialog(cx, &cookie, |d, cx| {
                if let Some(event) = event.take() {
                    d.on_event(event);
                }
                cx.notify();
            });
            if let Some(event) = event {
                stash(&mut cx.global_mut::<PolkitUi>().queue, &cookie, event);
            }
        }
    }
}

/// Runs `f` on the dialog when it shows `cookie`'s request.
fn with_dialog(cx: &mut App, cookie: &str, f: impl FnOnce(&mut PolkitDialog, &mut Context<PolkitDialog>)) {
    let handle = cx.global::<PolkitUi>().window;
    if let Some(handle) = handle {
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
    let Some(front) = cx.global_mut::<PolkitUi>().queue.front_mut() else { return };
    let (request, agent, early) = (front.request.clone(), front.agent.clone(), front.early.take());
    // The focused monitor: its display and size together (issue #8).
    let (display, (w, h)) = crate::outputs::overlay_target(cx);
    let mut spec = LayerSurfaceSpec::overlay(SurfaceRole::Lock, gpui::size(px(w), px(h)), display).with_keyboard(KeyboardInteractivity::Exclusive);
    spec.layer = Layer::Overlay;
    spec.anchor = Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT;
    let handle = cx
        .open_window(spec.window_options(), move |window, cx| {
            cx.new(|cx| {
                let mut dialog = PolkitDialog::new(request, agent, (w, h), window, cx);
                early.into_iter().flatten().for_each(|e| dialog.on_event(e));
                dialog
            })
        })
        .map_err(|e| tracing::error!("polkit dialog: {e:#}"))
        .ok();
    cx.global_mut::<PolkitUi>().window = handle;
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
    /// Waits for the size of the output it opened for, a few frames at
    /// most ([`crate::first_frame`]).
    first: FirstFrame,
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
        first_frame::guard("polkit prompt", window, cx, |this| this.first.drawn(), |this, cx| this.cancel(cx));
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
        // The AUR step's sudo asks with its own prompt, already known; a
        // second request means the last password was wrong.
        let askpass = request.cookie.starts_with(ASKPASS_COOKIE);
        let detail = |key: &str| request.details.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        let prompt = askpass.then(|| (detail(DETAIL_PROMPT).filter(|p| !p.is_empty()).unwrap_or_else(|| t("Password").to_string()), false));
        let note = (askpass && detail(DETAIL_RETRY).is_some()).then(|| (t("That didn't work — try again").to_string(), true));
        Self {
            request,
            agent,
            user_ix: 0,
            prompt,
            input: String::with_capacity(256),
            pending_submit: false,
            busy: false,
            note,
            focus,
            appear,
            shake: SpringValue::new(0.0, SHAKE),
            flash: SpringValue::new(0.0, SpringConfig::critical(90.0)),
            clock: FrameClock::new(),
            closing: false,
            outcome: None,
            first: FirstFrame::new(expected),
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
        let settled = self.first.ready((f32::from(vp.width), f32::from(vp.height)));
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
            cx.defer(show_next);
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
        let caret_on = (self.blink_from.elapsed().as_millis() / 530).is_multiple_of(2) && !self.busy;
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

// ─────────────────────────── the AUR step's sudo ───────────────────────────

/// One password request of the AUR step's askpass, opened on the UI
/// thread.
struct AskJob {
    request: AuthRequest,
    agent: PolkitAgent,
}

/// Cookies of the askpass's requests (the dialog then starts with sudo's
/// prompt, kept in the request's details, instead of waiting for one).
const ASKPASS_COOKIE: &str = "mandara-askpass-";
const DETAIL_PROMPT: &str = "prompt";
const DETAIL_RETRY: &str = "retry";

/// Shows the askpass's requests in the polkit dialog. Called on the job's
/// askpass thread; only the dialog itself is opened on the UI thread.
struct DialogPrompter {
    jobs: async_channel::Sender<AskJob>,
    relay: gpui::BackgroundExecutor,
}

static ASKPASS_REQUESTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl mandara_services::sysupdate::askpass::Prompter for DialogPrompter {
    fn ask(&self, request: mandara_services::sysupdate::askpass::AskRequest) -> mandara_services::sysupdate::askpass::Pending {
        use mandara_services::sysupdate::askpass::{Pending, Secret};
        let n = ASKPASS_REQUESTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let cookie = format!("{ASKPASS_COOKIE}{n}");
        let (events, erx) = async_channel::unbounded::<PolkitEvent>();
        let (rtx, replies) = async_channel::unbounded::<PolkitReply>();
        let (answer, answer_rx) = std::sync::mpsc::channel::<Option<Secret>>();
        let me = std::env::var("USER").unwrap_or_default();
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        let auth = AuthRequest {
            cookie: cookie.clone(),
            action_id: format!("sudo · {}", request.helper),
            message: tf("{helper} needs your password for sudo to install the AUR packages it built. sudo asks separately: the system update's approval can't be passed on to it.", &[("helper", &request.helper)]),
            icon_name: String::new(),
            details: std::iter::once((DETAIL_PROMPT.to_string(), request.prompt.clone())).chain((request.attempt > 1).then(|| (DETAIL_RETRY.to_string(), String::new()))).collect(),
            users: vec![AdminUser { uid, name: me.clone(), display: me }],
        };
        let job = AskJob { request: auth, agent: PolkitAgent::from_channels(erx, rtx.clone()) };
        // The dialog's answers go back to the askpass; the password moves
        // into a Secret (wiped when dropped) without a copy.
        let relay_cookie = cookie.clone();
        self.relay
            .spawn(async move {
                while let Ok(reply) = replies.recv().await {
                    let outcome = match reply {
                        PolkitReply::Answer { text, .. } => {
                            let _ = answer.send(Some(Secret::from_string(text)));
                            Outcome::Authorized
                        }
                        PolkitReply::Cancel { .. } => {
                            let _ = answer.send(None);
                            Outcome::Cancelled
                        }
                        PolkitReply::SelectUser { .. } => continue,
                    };
                    let _ = events.try_send(PolkitEvent::Close { cookie: relay_cookie.clone(), outcome });
                    break;
                }
            })
            .detach();
        queue_ask(&self.jobs, job, &rtx, &cookie);
        Pending {
            answer: answer_rx,
            close: Box::new(move || {
                let _ = rtx.try_send(PolkitReply::Cancel { cookie: cookie.clone() });
            }),
        }
    }
}

/// Hands `job` to the UI thread. When it can't be (the dialog is gone or
/// its queue is full), no dialog will answer: the request is answered
/// "cancelled" through `replies` so the askpass doesn't wait for its
/// timeout.
fn queue_ask(jobs: &async_channel::Sender<AskJob>, job: AskJob, replies: &async_channel::Sender<PolkitReply>, cookie: &str) {
    if jobs.try_send(job).is_err() {
        tracing::warn!("askpass: the password dialog isn't available");
        let _ = replies.try_send(PolkitReply::Cancel { cookie: cookie.to_owned() });
    }
}

/// Registers the dialog as the AUR step's password prompt.
fn init_askpass(cx: &mut App) {
    let (jobs, rx) = async_channel::bounded::<AskJob>(4);
    mandara_services::sysupdate::askpass::set_prompter(std::sync::Arc::new(DialogPrompter { jobs, relay: cx.background_executor().clone() }));
    cx.spawn(async move |cx| {
        while let Ok(job) = rx.recv().await {
            cx.update(|cx| open_askpass(job, cx));
        }
    })
    .detach();
}

fn open_askpass(job: AskJob, cx: &mut App) {
    let AskJob { request, agent } = job;
    pump(agent.clone(), cx);
    handle_event(PolkitEvent::Begin(request), agent, cx);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn job(cookie: &str) -> (AskJob, async_channel::Sender<PolkitReply>, async_channel::Receiver<PolkitReply>) {
        let (_etx, erx) = async_channel::unbounded::<PolkitEvent>();
        let (rtx, rrx) = async_channel::unbounded::<PolkitReply>();
        let request = AuthRequest { cookie: cookie.into(), action_id: String::new(), message: String::new(), icon_name: String::new(), details: Vec::new(), users: Vec::new() };
        (AskJob { request, agent: PolkitAgent::from_channels(erx, rtx.clone()) }, rtx, rrx)
    }

    /// The second of two back-to-back requests (Studio: refresh the
    /// helper, then apply) is asked for while the first dialog fades out:
    /// its prompt waits for its own dialog instead of being dropped.
    #[test]
    fn a_prompt_before_its_dialog_opens_is_kept_for_it() {
        let (first, ..) = job("first");
        let (second, ..) = job("second");
        let mut queue: VecDeque<Queued> = [first, second].into_iter().map(|j| Queued { request: j.request, agent: j.agent, early: Early::default() }).collect();
        let prompt = |cookie: &str| PolkitEvent::Prompt { cookie: cookie.into(), user: "layla".into(), text: "Password: ".into(), echo: false };
        stash(&mut queue, "second", PolkitEvent::Message { cookie: "second".into(), text: "2 attempts left".into(), error: false });
        stash(&mut queue, "second", prompt("second"));
        stash(&mut queue, "gone", prompt("gone"));
        queue.pop_front();
        let early = queue.front_mut().map(|q| q.early.take());
        assert!(matches!(early, Some([Some(PolkitEvent::Message { .. }), Some(PolkitEvent::Prompt { ref cookie, .. })]) if cookie == "second"));
        assert!(queue.front_mut().is_some_and(|q| matches!(q.early.take(), [None, None])), "replayed once");
        let mut early = Early::default();
        early.keep(prompt("x"));
        early.keep(PolkitEvent::Retry { cookie: "x".into(), message: String::new() });
        assert!(matches!(early.take(), [None, None]), "a retry spends the old prompt");
    }

    #[test]
    fn an_askpass_request_with_no_dialog_is_cancelled_at_once() {
        // The dialog's side is gone.
        let (jobs, rx) = async_channel::bounded::<AskJob>(1);
        drop(rx);
        let (ask, rtx, rrx) = job("mandara-askpass-7");
        queue_ask(&jobs, ask, &rtx, "mandara-askpass-7");
        assert!(matches!(rrx.try_recv(), Ok(PolkitReply::Cancel { cookie }) if cookie == "mandara-askpass-7"));
        // Its queue is full.
        let (jobs, rx) = async_channel::bounded::<AskJob>(1);
        let (first, rtx1, rrx1) = job("mandara-askpass-8");
        queue_ask(&jobs, first, &rtx1, "mandara-askpass-8");
        assert!(rrx1.try_recv().is_err(), "a queued request waits for its dialog");
        let (second, rtx2, rrx2) = job("mandara-askpass-9");
        queue_ask(&jobs, second, &rtx2, "mandara-askpass-9");
        assert!(matches!(rrx2.try_recv(), Ok(PolkitReply::Cancel { cookie }) if cookie == "mandara-askpass-9"));
        assert_eq!(rx.try_recv().map(|j| j.request.cookie).ok().as_deref(), Some("mandara-askpass-8"));
    }
}
