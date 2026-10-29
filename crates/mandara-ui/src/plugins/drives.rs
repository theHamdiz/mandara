//! Drives: storage and removable media through UDisks2. The bar chip shows
//! how many removable drives are plugged in and pulses (with an optional
//! notification) when a new one appears. The panel lists removable drives
//! and this computer's disks with SDF usage rings, filesystem, encryption
//! and mount details; volumes mount, unmount, open in the file manager,
//! and LUKS containers unlock through an inline passphrase sheet. "Safely
//! remove" unmounts and locks everything on a drive, then ejects or powers
//! it off. Privileged steps prompt through polkit (the shell's own agent).
//!
//! One UDisks2 actor is shared by every bar instance and stops when the
//! last chip / panel goes away; usage figures refresh only while the panel
//! is open. Commands: `drives list|mount|unmount|eject|unlock|lock|open`.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::hash::{Hash as _, Hasher as _};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, FocusHandle, FontWeight, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px, size,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::drives::{Drive, DrivesCommand, DrivesService, DrivesState, Volume, spawn_drives_service, wipe};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::fn_view;
use super::panel_fx::{
    ANGLE_DOWN_DEG, ANGLE_RIGHT_DEG, EXPIRY_MARGIN, HEADER_H, PILL_H_PX, PanelShape, SECTION_H_PX, SMALL_ICON_PX, ViewSet, alpha, dot_ring, empty_state,
    glass_card, glow, header, icon_chip, partner, percent_text, readable, redraw_in, section,
};
use crate::fonts::tabular_nums;
use crate::icons::icon;
use crate::kit::{icon_button, reveal, text, tokens};
use crate::widgets::system::format_bytes;

pub const ID: &str = "drives";

const PANEL_W: f32 = 470.0;
const PAD: f32 = 14.0;
const GAP: f32 = tokens::MD;
const SECTION_H: f32 = SECTION_H_PX;
const SECTION_GAP: f32 = 6.0;
const CARD_PAD: f32 = 10.0;
const CARD_GAP: f32 = 10.0;
const DRIVE_HEAD_H: f32 = 40.0;
const VOL_H: f32 = 56.0;
const VOL_GAP: f32 = 2.0;
const TILE: f32 = 36.0;
const RING_D: f32 = 40.0;
const RING_DOTS: usize = 26;
const RING_DOT: f32 = 2.6;
/// Usage above this percent turns the ring amber → coral
/// (`full_warn_percent`).
pub const DEFAULT_FULL_WARN_PERCENT: i64 = 90;
pub const FULL_WARN_RANGE: (i64, i64) = (50, 99);
/// Percent → fraction.
const PERCENT: f32 = 100.0;
const EMPTY_H: f32 = 190.0;
const SMALL_EMPTY_H: f32 = 132.0;
/// Visible list height before it scrolls.
const LIST_MAX_H: f32 = 560.0;
const UNLOCK_H: f32 = 182.0;
const FIELD_H: f32 = 38.0;
const BUTTON_H: f32 = 32.0;
/// Bullets drawn at most for a long passphrase.
const MAX_BULLETS: usize = 24;
/// Usage figures refresh at this cadence (seconds) while the panel is
/// open (`usage_every_s`).
pub const DEFAULT_USAGE_EVERY_S: i64 = 10;
pub const USAGE_EVERY_S_RANGE: (i64, i64) = (2, 300);

/// Usage refresh cadence (`usage_every_s`).
fn usage_every(cx: &App) -> Duration {
    let s: i64 = crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "usage_every_s", DEFAULT_USAGE_EVERY_S);
    Duration::from_secs(s.clamp(USAGE_EVERY_S_RANGE.0, USAGE_EVERY_S_RANGE.1) as u64)
}

/// Fill fraction where the ring turns amber (`full_warn_percent`).
fn full_warn(cx: &App) -> f32 {
    let pct: i64 = crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "full_warn_percent", DEFAULT_FULL_WARN_PERCENT);
    pct.clamp(FULL_WARN_RANGE.0, FULL_WARN_RANGE.1) as f32 / PERCENT
}
const TOAST_H: f32 = 36.0;
/// The chip pulses this long after a drive is plugged in.
const PULSE: Duration = Duration::from_millis(3200);
/// Glow beats during a pulse.
const PULSE_BEATS: f32 = 3.0;
/// Mounts the shell never offers to unmount (the running system).
const PROTECTED_MOUNTS: [&str; 7] = ["/", "/home", "/boot", "/efi", "/var", "/usr", "/nix"];

// Passphrase field.
const FIELD_PAD_X_PX: f32 = 14.0;
const FIELD_ICON_PX: f32 = 14.0;
/// Focused border alpha, focus glow (alpha, blur) and placeholder alpha.
const FIELD_FOCUS_BORDER_ALPHA: f32 = 0.7;
const FIELD_FOCUS_GLOW: (f32, f32) = (0.22, 10.0);
const FIELD_PLACEHOLDER_ALPHA: f32 = 0.7;

/// `ink_on`: backgrounds lighter than this get dark ink.
const INK_SWITCH_L: f32 = 0.64;
/// Dark ink `(lightness, chroma)` and light ink lightness (OKLCH).
const DARK_INK: (f32, f32) = (0.2, 0.02);
const LIGHT_INK_L: f32 = 0.99;

// Badges.
const BADGE_H_PX: f32 = 16.0;
const BADGE_PAD_X_PX: f32 = 6.0;
const BADGE_GAP_PX: f32 = 3.0;
const BADGE_TEXT_PX: f32 = 10.0;
const BADGE_ICON_PX: f32 = 9.0;
/// Badge fill alpha `(dark, light)`.
const BADGE_FILL: (f32, f32) = (0.14, 0.1);

// Action pills.
const ACTION_PAD_X_PX: f32 = 11.0;
const ACTION_GAP_PX: f32 = 5.0;
/// Action label: half a step above the label size.
const ACTION_TEXT_PX: f32 = text::LABEL + 0.5;
/// Soft / hover fill alphas of a secondary action `(dark, light)`.
const ACTION_SOFT: (f32, f32) = (0.16, 0.11);
const ACTION_HOVER: (f32, f32) = (0.26, 0.18);
/// Glow under a primary action: (alpha, blur, dy).
const ACTION_GLOW: (f32, f32, f32) = (0.3, 8.0, 1.0);
/// "Working…" pill fill alpha.
const WORKING_FILL_ALPHA: f32 = 0.06;

// Volume rows.
const RING_TRACK: (f32, f32) = (0.12, 0.1);
const RING_GLYPH_PX: f32 = 14.0;
const RING_TEXT_PX: f32 = 10.0;
const RING_IDLE_GLYPH_ALPHA: f32 = 0.7;
const ROW_BUTTON_PX: f32 = 28.0;
/// Fill behind a row's icon buttons `(dark, light)`.
const ROW_BUTTON_FILL: (f32, f32) = (0.12, 0.08);
/// Row hover wash `(dark, light)`.
const ROW_HOVER: (f32, f32) = (0.05, 0.04);
const ROW_PAD_X_PX: f32 = 6.0;
const ROW_GAP_PX: f32 = 10.0;
const ROW_LINE_GAP_PX: f32 = 2.0;
const ROW_TITLE_GAP_PX: f32 = 6.0;
const ROW_DETAIL_ALPHA: f32 = 0.85;
/// Rows are rounded a little more than concentric inside the card, never
/// below this radius.
const ROW_RADIUS_EXTRA_PX: f32 = 4.0;
const ROW_MIN_RADIUS_PX: f32 = 10.0;
/// Gap between a drive's head row and its volumes.
const HEAD_GAP_PX: f32 = 6.0;

// Unlock sheet.
const CANCEL_PAD_X_PX: f32 = 16.0;
const UNLOCK_PAD_X_PX: f32 = 18.0;
const BUTTON_FILL_ALPHA: f32 = 0.06;
const CANCEL_HOVER_OPACITY: f32 = 0.85;
const BUSY_OPACITY: f32 = 0.6;
/// Glow under the unlock button: (alpha, blur, dy).
const UNLOCK_GLOW: (f32, f32, f32) = (0.3, 10.0, 2.0);
const UNLOCK_ICON_PX: f32 = 13.0;
const SHEET_PAD_PX: f32 = 14.0;

// Toast.
const TOAST_PAD_L_PX: f32 = 6.0;
const TOAST_PAD_R_PX: f32 = 14.0;
/// Toast drop shadow: alpha `(dark, light)`, blur and dy.
const TOAST_SHADOW: (f32, f32) = (0.45, 0.14);
const TOAST_SHADOW_BLUR_PX: f32 = 14.0;
const TOAST_SHADOW_DY_PX: f32 = 4.0;
const TOAST_BADGE_PX: f32 = 24.0;
const TOAST_BADGE_ALPHA: f32 = 0.16;
/// The toast keeps this much panel padding clear on each side.
const TOAST_SIDE_PADS: f32 = 4.0;

// Panel.
const REFRESH_BUTTON_PX: f32 = 30.0;
/// Height of the fade over a clipped list's bottom edge.
const LIST_FADE_PX: f32 = 22.0;
/// Size changes smaller than this don't resize the panel.
const RESIZE_EPSILON_PX: f32 = 0.5;

// Bar chip.
const CHIP_GAP_PX: f32 = 4.0;
const CHIP_GLYPH_BOX_PX: f32 = 20.0;
const CHIP_GLYPH_PX: f32 = 15.0;
/// Pulse: the tint mixes toward the accent from this base share.
const PULSE_TINT_BASE: f32 = 0.4;
/// Pulse fill alpha, glow alpha, blur and spread at full beat.
const PULSE_FILL_ALPHA: f32 = 0.2;
const PULSE_GLOW_ALPHA: f32 = 0.5;
const PULSE_GLOW_BLUR_PX: f32 = 10.0;
const PULSE_GLOW_SPREAD_PX: f32 = 1.5;

/// A stable element id for a UDisks object path (no per-frame `format!`).
fn path_id(prefix: &'static str, path: &str) -> ElementId {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    ElementId::NamedInteger(SharedString::new_static(prefix), h.finish())
}

// ── Shared model ─────────────────────────────────────────────────────────

struct UnlockSheet {
    /// Object path of the LUKS container.
    device: String,
    name: String,
    error: Option<String>,
    busy: bool,
}

struct Model {
    service: DrivesService,
    state: DrivesState,
    received: bool,
    seen_plug: u64,
    pulse_at: Option<Instant>,
    /// Object paths with an action in flight.
    busy: HashSet<String>,
    /// (ok, message, shown at).
    status: Option<(bool, SharedString, Instant)>,
    unlock: Option<UnlockSheet>,
    usage_at: Option<Instant>,
    /// A close watch is running.
    close_watch: Rc<Cell<bool>>,
    /// The chip(s) and panel showing this model.
    views: ViewSet,
}

impl Model {
    fn new(service: DrivesService) -> Self {
        Self {
            service,
            state: DrivesState::default(),
            received: false,
            seen_plug: 0,
            pulse_at: None,
            busy: HashSet::new(),
            status: None,
            unlock: None,
            usage_at: None,
            close_watch: Rc::default(),
            views: ViewSet::default(),
        }
    }

    /// Takes a new state; returns the name of a newly plugged drive.
    fn absorb(&mut self, state: DrivesState) -> Option<String> {
        let first = !self.received;
        self.received = true;
        let plugged = (!first && state.plug_seq > self.seen_plug).then(|| state.last_plugged.clone()).flatten();
        self.seen_plug = state.plug_seq;
        if plugged.is_some() {
            self.pulse_at = Some(Instant::now());
        }
        // An unlock sheet closes once its container is unlocked elsewhere.
        if let Some(sheet) = &self.unlock
            && !sheet.busy
            && state.find_volume(&sheet.device).is_none_or(|(_, v)| !v.locked())
        {
            self.unlock = None;
        }
        self.state = state;
        plugged
    }

    fn pulse(&self) -> Option<f32> {
        let e = self.pulse_at?.elapsed();
        (e < PULSE).then(|| {
            let t = e.as_secs_f32() / PULSE.as_secs_f32();
            // Raised cosine (0..=1) fading out over the pulse.
            (1.0 - t) * (1.0 + (t * std::f32::consts::TAU * PULSE_BEATS).cos()) / 2.0
        })
    }
}

type Shared = Rc<RefCell<Model>>;

thread_local! {
    static SHARED: RefCell<Weak<RefCell<Model>>> = RefCell::new(Weak::new());
}

fn live() -> Option<Shared> {
    SHARED.with(|s| s.borrow().upgrade())
}

/// The shared model, starting the UDisks2 actor on first use.
fn shared(cx: &mut App) -> Shared {
    if let Some(m) = live() {
        return m;
    }
    let service = spawn_drives_service();
    let updates = service.updates.clone();
    let m = Rc::new(RefCell::new(Model::new(service)));
    SHARED.with(|s| *s.borrow_mut() = Rc::downgrade(&m));
    let weak = Rc::downgrade(&m);
    cx.spawn(async move |cx| {
        while let Ok(state) = updates.recv().await {
            let Some(m) = weak.upgrade() else { break };
            let (plugged, views) = {
                let mut mb = m.borrow_mut();
                (mb.absorb(state), mb.views.clone())
            };
            drop(m);
            cx.update(|cx| {
                if let Some(name) = plugged {
                    announce(&name, cx);
                }
                views.notify(cx);
            });
        }
    })
    .detach();
    m
}

fn notify_on_plug(cx: &App) -> bool {
    crate::state::config(cx).read(cx).get().plugins.setting(ID, "notify_on_plug", true)
}

fn show_system(cx: &App) -> bool {
    crate::state::config(cx).read(cx).get().plugins.setting(ID, "show_system", false)
}

fn announce(name: &str, cx: &mut App) {
    if !notify_on_plug(cx) {
        return;
    }
    crate::plugins::notifications::send(
        cx,
        mandara_services::NotificationCommand::Post {
            app: t("Drives").into(),
            icon: "drive-removable-media".into(),
            summary: tf("{drive} connected", &[("drive", &name)]),
            body: t("Open the drives panel to mount or safely remove it").into(),
        },
    );
}

/// Sends `command`, marking `key` busy until the reply lands as a toast.
fn act(m: &Shared, key: String, command: DrivesCommand, cx: &mut App) {
    let (rx, views) = {
        let mut mb = m.borrow_mut();
        if !mb.busy.insert(key.clone()) {
            return;
        }
        (mb.service.request(command), mb.views.clone())
    };
    // Show the "Working…" state right away.
    views.notify(cx);
    let m = m.clone();
    cx.spawn(async move |cx| {
        let result = rx.recv().await.unwrap_or_else(|_| Err(t("The service did not reply").into()));
        let views = {
            let mut mb = m.borrow_mut();
            mb.busy.remove(&key);
            mb.status = Some(match result {
                Ok(msg) => (true, msg.into(), Instant::now()),
                Err(e) => (false, e.into(), Instant::now()),
            });
            mb.views.clone()
        };
        views.notify_async(cx);
    })
    .detach();
}

/// Unlocks the sheet's container with `passphrase` (moved, then wiped by
/// the actor); a failure keeps the sheet open with the error.
fn submit_unlock(m: &Shared, passphrase: String, cx: &mut App) {
    let mut passphrase = passphrase;
    let rx = {
        let mut mb = m.borrow_mut();
        let Some(sheet) = mb.unlock.as_mut().filter(|s| !s.busy) else {
            wipe(&mut passphrase);
            return;
        };
        if passphrase.is_empty() {
            sheet.error = Some(t("Enter the passphrase").into());
            let views = mb.views.clone();
            drop(mb);
            views.notify(cx);
            return;
        }
        sheet.busy = true;
        sheet.error = None;
        let device = sheet.device.clone();
        mb.service.request(DrivesCommand::Unlock { device, passphrase })
    };
    let views = m.borrow().views.clone();
    views.notify(cx);
    let m = m.clone();
    cx.spawn(async move |cx| {
        let result = rx.recv().await.unwrap_or_else(|_| Err(t("The service did not reply").into()));
        let views = {
            let mut mb = m.borrow_mut();
            match result {
                Ok(msg) => {
                    mb.unlock = None;
                    mb.status = Some((true, msg.into(), Instant::now()));
                }
                Err(e) => {
                    if let Some(sheet) = mb.unlock.as_mut() {
                        sheet.busy = false;
                        sheet.error = Some(e);
                    }
                }
            }
            mb.views.clone()
        };
        views.notify_async(cx);
    })
    .detach();
}

// ── Passphrase field ─────────────────────────────────────────────────────

type Submit = Rc<dyn Fn(String, &mut Window, &mut App)>;
type Cancel = Rc<dyn Fn(&mut Window, &mut App)>;

/// A focusable, masked passphrase entry: bullets instead of text, Enter
/// submits (handing the secret over and clearing the field), Escape wipes
/// it and cancels. The value is never logged or rendered.
struct PassField {
    value: String,
    focus: FocusHandle,
    on_submit: Submit,
    on_cancel: Cancel,
}

impl PassField {
    fn new(on_submit: Submit, on_cancel: Cancel, cx: &mut Context<Self>) -> Self {
        Self { value: String::new(), focus: cx.focus_handle(), on_submit, on_cancel }
    }

    fn clear(&mut self) {
        wipe(&mut self.value);
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let secret = std::mem::take(&mut self.value);
        (self.on_submit)(secret, window, cx);
    }
}

impl Drop for PassField {
    fn drop(&mut self) {
        wipe(&mut self.value);
    }
}

impl Render for PassField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = mandara_core::frame_palette(window, cx);
        let focused = self.focus.is_focused(window);
        let count = self.value.chars().count();
        let accent = readable(palette.primary, &palette);
        let shown = if count == 0 && !focused {
            t("Passphrase").to_string()
        } else {
            let bullets: String = "•".repeat(count.min(MAX_BULLETS));
            if focused { format!("{bullets}▏") } else { bullets }
        };
        div()
            .id("drive-passphrase")
            .track_focus(&self.focus)
            .w_full()
            .h(px(FIELD_H))
            .px(px(FIELD_PAD_X_PX))
            .rounded_full()
            .flex()
            .items_center()
            .gap(px(tokens::SM))
            .bg(palette.surface_container_high.hsla())
            .border_1()
            .border_color(if focused { accent.with_alpha(FIELD_FOCUS_BORDER_ALPHA).hsla() } else { palette.surface_container_high.hsla() })
            .when(focused, |el| el.shadow(vec![glow(accent.with_alpha(FIELD_FOCUS_GLOW.0), FIELD_FOCUS_GLOW.1, 0.0, 0.0)]))
            .cursor_text()
            .child(icon("lock", FIELD_ICON_PX, if focused { accent } else { palette.on_surface_variant }))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(if count == 0 { text::BODY } else { text::TITLE }))
                    .text_color(if count == 0 { palette.on_surface_variant.with_alpha(FIELD_PLACEHOLDER_ALPHA).hsla() } else { palette.on_surface.hsla() })
                    .child(shown),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.focus.focus(window, cx);
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                let k = &e.keystroke;
                match k.key.as_str() {
                    "enter" => this.submit(window, cx),
                    "escape" => {
                        this.clear();
                        (this.on_cancel.clone())(window, cx);
                    }
                    "backspace" => {
                        if k.modifiers.control {
                            this.clear();
                        } else {
                            this.value.pop();
                        }
                    }
                    "v" if k.modifiers.control => {
                        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                            this.value.push_str(text.trim_end_matches(['\n', '\r']));
                        }
                    }
                    _ => match k.key_char.as_ref().filter(|_| !k.modifiers.control && !k.modifiers.alt) {
                        Some(ch) => this.value.push_str(ch),
                        None => return,
                    },
                }
                cx.stop_propagation();
                cx.notify();
            }))
    }
}

// ── Pieces ───────────────────────────────────────────────────────────────

/// Legible foreground on a solid `bg`.
fn ink_on(bg: Color) -> Color {
    if bg.lightness() > INK_SWITCH_L { Color::oklch(DARK_INK.0, DARK_INK.1, bg.hue()) } else { Color::oklch(LIGHT_INK_L, 0.0, 0.0) }
}

fn badge(label: impl Into<SharedString>, tint: Color, icon_name: Option<&'static str>, palette: &MandaraPalette) -> AnyElement {
    div()
        .flex_none()
        .h(px(BADGE_H_PX))
        .px(px(BADGE_PAD_X_PX))
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(BADGE_GAP_PX))
        .bg(tint.with_alpha(alpha(BADGE_FILL, palette)).hsla())
        .text_size(px(BADGE_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tint.hsla())
        .when_some(icon_name, |el, i| el.child(icon(i, BADGE_ICON_PX, tint)))
        .child(label.into())
        .into_any_element()
}

/// A compact action pill. `primary` fills it with the accent gradient.
fn action(
    id: ElementId,
    icon_name: &'static str,
    label: &'static str,
    tint: Color,
    primary: bool,
    palette: &MandaraPalette,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let soft = tint.with_alpha(alpha(ACTION_SOFT, palette));
    let hover = tint.with_alpha(alpha(ACTION_HOVER, palette));
    let ink = ink_on(tint);
    let (glow_alpha, glow_blur, glow_dy) = ACTION_GLOW;
    div()
        .id(id)
        .h(px(PILL_H_PX))
        .px(px(ACTION_PAD_X_PX))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(ACTION_GAP_PX))
        .cursor_pointer()
        .text_size(px(ACTION_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .when(primary, |el| {
            el.bg(oklab_gradient(ANGLE_RIGHT_DEG, tint, partner(tint).with_lightness(tint.lightness())))
                .text_color(ink.hsla())
                .shadow(vec![glow(tint.with_alpha(glow_alpha), glow_blur, 0.0, glow_dy)])
        })
        .when(!primary, |el| el.bg(soft.hsla()).text_color(tint.hsla()).hover(move |s| s.bg(hover.hsla())))
        .child(icon(icon_name, SMALL_ICON_PX, if primary { ink } else { tint }))
        .child(label)
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_click(window, cx);
        })
        .into_any_element()
}

fn working(palette: &MandaraPalette) -> AnyElement {
    div()
        .h(px(PILL_H_PX))
        .px(px(ACTION_PAD_X_PX))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(ACTION_GAP_PX))
        .bg(palette.on_surface.with_alpha(WORKING_FILL_ALPHA).hsla())
        .text_size(px(ACTION_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(palette.on_surface_variant.hsla())
        .child(icon("timer", SMALL_ICON_PX, palette.on_surface_variant))
        .child(t("Working…"))
        .into_any_element()
}

fn drive_kind(d: &Drive) -> String {
    match d.bus.as_str() {
        "usb" => t("USB drive").into(),
        "sdio" => t("SD card").into(),
        "ieee1394" => "FireWire".into(),
        _ if d.optical => t("Optical drive").into(),
        _ if d.rotation_rate == 0 => t("Solid-state drive").into(),
        _ if d.rotation_rate > 0 => t("Hard disk").into(),
        _ => t("Drive").into(),
    }
}

fn volume_name(v: &Volume) -> String {
    let inner = v.effective();
    if !inner.label.is_empty() {
        inner.label.clone()
    } else if !v.label.is_empty() {
        v.label.clone()
    } else if let Some(mount) = v.primary_mount() {
        // Unlabelled but mounted: name it after where it lives.
        if mount == "/" { t("System").into() } else { mount.rsplit('/').find(|s| !s.is_empty()).unwrap_or(mount).to_string() }
    } else {
        tf("{size} volume", &[("size", &format_bytes(v.size))])
    }
}

/// SDF dot ring with the usage percent (or a state glyph) inside.
fn usage_ring(v: &Volume, warn: f32, palette: &MandaraPalette) -> AnyElement {
    let track = palette.on_surface.with_alpha(alpha(RING_TRACK, palette));
    let usage = v.effective().usage;
    let frac = usage.map(|(u, total)| if total > 0 { u as f32 / total as f32 } else { 0.0 }).unwrap_or(0.0);
    let (from, to) = if frac >= warn {
        (readable(palette.warning_amber, palette), readable(palette.urgent_coral, palette))
    } else {
        (readable(palette.primary, palette), readable(partner(palette.primary), palette))
    };
    let inner: AnyElement = if v.locked() {
        icon("lock", RING_GLYPH_PX, readable(palette.tertiary, palette)).into_any_element()
    } else if usage.is_some() {
        div().font_features(tabular_nums()).text_size(px(RING_TEXT_PX)).font_weight(FontWeight::SEMIBOLD).child(percent_text(frac)).into_any_element()
    } else {
        icon("disk", RING_GLYPH_PX, palette.on_surface_variant.with_alpha(RING_IDLE_GLYPH_ALPHA)).into_any_element()
    };
    dot_ring(frac, RING_D, RING_DOTS, RING_DOT, from, to, track, inner).into_any_element()
}

fn volume_row(v: &Volume, m: &Shared, palette: &MandaraPalette, radius: f32, warn: f32) -> AnyElement {
    let inner = v.effective();
    let busy = {
        let mb = m.borrow();
        mb.busy.contains(&v.object_path)
    };
    let accent = readable(palette.primary, palette);
    let tertiary = readable(palette.tertiary, palette);
    let mounts = &inner.mount_points;
    let primary_mount = v.primary_mount().map(str::to_string);
    let detail = if v.locked() {
        tf("Locked · {size}", &[("size", &format_bytes(v.size))])
    } else if let Some(mount) = &primary_mount {
        let more = if mounts.len() > 1 { format!(" +{}", mounts.len() - 1) } else { String::new() };
        match inner.usage {
            Some((used, total)) => tf("{mount}{more} · {used} of {total}", &[("mount", mount), ("more", &more), ("used", &format_bytes(used)), ("total", &format_bytes(total))]),
            None => format!("{mount}{more}"),
        }
    } else {
        tf("Not mounted · {size}", &[("size", &format_bytes(v.size))])
    };
    let fs: &str = if inner.fs_type.is_empty() { &v.fs_type } else { &inner.fs_type };
    let button_fill = alpha(ROW_BUTTON_FILL, palette);

    let mut actions: Vec<AnyElement> = Vec::new();
    if busy {
        actions.push(working(palette));
    } else if v.locked() {
        let (m2, dev, name) = (m.clone(), v.object_path.clone(), volume_name(v));
        actions.push(action(path_id("drv-unlock", &v.object_path), "lock-open", t("Unlock"), tertiary, true, palette, move |_, cx| {
            let views = {
                let mut mb = m2.borrow_mut();
                mb.unlock = Some(UnlockSheet { device: dev.clone(), name: name.clone(), error: None, busy: false });
                mb.views.clone()
            };
            views.notify(cx);
        }));
    } else {
        if let Some(mount) = primary_mount.clone() {
            actions.push(
                icon_button(path_id("drv-open", &v.object_path), "folder", ROW_BUTTON_PX, accent, Some(accent.with_alpha(button_fill)), move |_, _| {
                    let uri = mandara_services::drives::file_uri(&mount);
                    std::thread::spawn(move || {
                        if let Err(e) = mandara_services::session::open_uri(&uri) {
                            tracing::warn!("drives: {e}");
                        }
                    });
                })
                .into_any_element(),
            );
            if !mounts.iter().any(|m| PROTECTED_MOUNTS.contains(&m.as_str())) {
                let (m2, dev) = (m.clone(), v.object_path.clone());
                actions.push(action(path_id("drv-umount", &v.object_path), "eject", t("Unmount"), accent, false, palette, move |_, cx| {
                    act(&m2, dev.clone(), DrivesCommand::Unmount(dev.clone()), cx)
                }));
            }
        } else if inner.has_filesystem {
            let (m2, dev) = (m.clone(), v.object_path.clone());
            actions.push(action(path_id("drv-mount", &v.object_path), "plus", t("Mount"), accent, true, palette, move |_, cx| {
                act(&m2, dev.clone(), DrivesCommand::Mount(dev.clone()), cx)
            }));
        }
        if v.encrypted && !mounts.iter().any(|m| PROTECTED_MOUNTS.contains(&m.as_str())) {
            let (m2, dev) = (m.clone(), v.object_path.clone());
            actions.push(
                icon_button(path_id("drv-lock", &v.object_path), "lock", ROW_BUTTON_PX, tertiary, Some(tertiary.with_alpha(button_fill)), move |_, cx| {
                    act(&m2, dev.clone(), DrivesCommand::Lock(dev.clone()), cx)
                })
                .into_any_element(),
            );
        }
    }
    let hover = palette.on_surface.with_alpha(alpha(ROW_HOVER, palette));
    div()
        .id(path_id("drv-vol", &v.object_path))
        .h(px(VOL_H))
        .flex_none()
        .px(px(ROW_PAD_X_PX))
        .rounded(px(radius))
        .flex()
        .items_center()
        .gap(px(ROW_GAP_PX))
        .hover(move |s| s.bg(hover.hsla()))
        .child(usage_ring(v, warn, palette))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(ROW_LINE_GAP_PX))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(ROW_TITLE_GAP_PX))
                        .min_w(px(0.0))
                        .child(div().min_w(px(0.0)).truncate().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child(volume_name(v)))
                        .when(!fs.is_empty() && fs != "crypto_LUKS", |el| el.child(badge(SharedString::from(fs.to_string()), accent, None, palette)))
                        .when(v.encrypted, |el| el.child(badge(t("Encrypted"), tertiary, Some("lock"), palette)))
                        .when(inner.read_only, |el| el.child(badge(t("Read-only"), palette.on_surface_variant, None, palette))),
                )
                .child(
                    div()
                        .font_features(tabular_nums())
                        .text_size(px(text::LABEL))
                        .text_color(palette.on_surface_variant.with_alpha(ROW_DETAIL_ALPHA).hsla())
                        .truncate()
                        .child(format!("{} · {detail}", v.device_name())),
                ),
        )
        .child(div().flex_none().flex().items_center().gap(px(ROW_TITLE_GAP_PX)).children(actions))
        .into_any_element()
}

/// One drive: head row plus its volumes. Returns the card and its height.
fn drive_card(d: &Drive, m: &Shared, palette: &MandaraPalette, radius: f32, show_system: bool, warn: f32) -> (AnyElement, f32) {
    let volumes: Vec<&Volume> = d.volumes.iter().filter(|v| show_system || !v.system).collect();
    let removable = d.is_removable();
    let accent = readable(if removable { palette.secondary } else { palette.primary }, palette);
    let busy = m.borrow().busy.contains(&d.object_path);
    let mut sub = vec![drive_kind(d)];
    if d.size > 0 {
        sub.push(format_bytes(d.size));
    }
    if !d.device.is_empty() {
        sub.push(d.device_name().to_string());
    }
    let trailing: Option<AnyElement> = removable.then(|| {
        if busy {
            working(palette)
        } else {
            let eject = d.media_removable && d.ejectable;
            let (m2, path) = (m.clone(), d.object_path.clone());
            let coral = readable(palette.urgent_coral, palette);
            action(
                path_id("drv-remove", &d.object_path),
                if eject { "eject" } else { "power" },
                if eject { t("Eject") } else { t("Safely remove") },
                coral,
                false,
                palette,
                move |_, cx| act(&m2, path.clone(), DrivesCommand::SafelyRemove(path.clone()), cx),
            )
        }
    });
    let head = div()
        .h(px(DRIVE_HEAD_H))
        .flex()
        .items_center()
        .gap(px(ROW_GAP_PX))
        .child(icon_chip(if removable { "usb" } else { "disk" }, accent, TILE))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().child(d.display_name()))
                .child(div().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(sub.join(" · "))),
        )
        .when_some(trailing, |el, t| el.child(t));
    let row_r = (radius - CARD_PAD + ROW_RADIUS_EXTRA_PX).max(ROW_MIN_RADIUS_PX);
    let rows: Vec<AnyElement> = if volumes.is_empty() {
        vec![
            div()
                .h(px(VOL_H))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(text::BODY_S))
                .text_color(palette.on_surface_variant.hsla())
                .child(if removable { t("No readable volumes — insert media or format the drive") } else { t("No visible volumes") })
                .into_any_element(),
        ]
    } else {
        volumes.iter().map(|v| volume_row(v, m, palette, row_r, warn)).collect()
    };
    let n = rows.len() as f32;
    let height = CARD_PAD * 2.0 + DRIVE_HEAD_H + HEAD_GAP_PX + n * VOL_H + (n - 1.0).max(0.0) * VOL_GAP;
    let card = glass_card(palette)
        .rounded(px(radius))
        .flex_none()
        .h(px(height))
        .p(px(CARD_PAD))
        .flex()
        .flex_col()
        .child(head)
        .child(div().h(px(HEAD_GAP_PX)))
        .child(div().flex().flex_col().gap(px(VOL_GAP)).children(rows))
        .into_any_element();
    (card, height)
}

fn unlock_card(sheet: &UnlockSheet, field: &Entity<PassField>, m: &Shared, palette: &MandaraPalette, radius: f32) -> AnyElement {
    let tertiary = readable(palette.tertiary, palette);
    let accent = readable(palette.primary, palette);
    let coral = readable(palette.urgent_coral, palette);
    let (m_cancel, f_cancel) = (m.clone(), field.clone());
    let (m_ok, f_ok) = (m.clone(), field.clone());
    let cancel = div()
        .id("drv-unlock-cancel")
        .h(px(BUTTON_H))
        .px(px(CANCEL_PAD_X_PX))
        .rounded_full()
        .flex()
        .items_center()
        .cursor_pointer()
        .bg(palette.on_surface.with_alpha(BUTTON_FILL_ALPHA).hsla())
        .hover(|s| s.opacity(CANCEL_HOVER_OPACITY))
        .text_size(px(text::BODY_S))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(palette.on_surface_variant.hsla())
        .child(t("Cancel"))
        .on_click(move |_, _, cx| {
            f_cancel.update(cx, |f, _| f.clear());
            let views = {
                let mut mb = m_cancel.borrow_mut();
                mb.unlock = None;
                mb.views.clone()
            };
            views.notify(cx);
        });
    let ink = ink_on(accent);
    let (glow_alpha, glow_blur, glow_dy) = UNLOCK_GLOW;
    let unlock = div()
        .id("drv-unlock-go")
        .h(px(BUTTON_H))
        .px(px(UNLOCK_PAD_X_PX))
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(ROW_TITLE_GAP_PX))
        .cursor_pointer()
        .bg(oklab_gradient(ANGLE_RIGHT_DEG, accent, partner(accent).with_lightness(accent.lightness())))
        .shadow(vec![glow(accent.with_alpha(glow_alpha), glow_blur, 0.0, glow_dy)])
        .text_size(px(text::BODY_S))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ink.hsla())
        .when(sheet.busy, |el| el.opacity(BUSY_OPACITY))
        .child(icon(if sheet.busy { "timer" } else { "lock-open" }, UNLOCK_ICON_PX, ink))
        .child(if sheet.busy { t("Unlocking…") } else { t("Unlock") })
        .on_click(move |_, _, cx| {
            let secret = f_ok.update(cx, |f, _| std::mem::take(&mut f.value));
            submit_unlock(&m_ok, secret, cx);
        });
    glass_card(palette)
        .rounded(px(radius))
        .h(px(UNLOCK_H))
        .p(px(SHEET_PAD_PX))
        .flex()
        .flex_col()
        .gap(px(ROW_GAP_PX))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(tokens::MD))
                .child(icon_chip("lock", tertiary, TILE))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().child(tf("Unlock {name}", &[("name", &sheet.name)])))
                        .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(t("This volume is encrypted. Enter its passphrase to open it."))),
                ),
        )
        .child(field.clone())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(tokens::SM))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_size(px(text::LABEL))
                        .text_color(if sheet.error.is_some() { coral.hsla() } else { palette.on_surface_variant.hsla() })
                        .child(sheet.error.as_ref().map_or_else(|| SharedString::new_static(t("Enter unlocks · Esc cancels")), |e| SharedString::from(e.clone()))),
                )
                .child(cancel)
                .child(unlock),
        )
        .into_any_element()
}

fn toast(ok: bool, msg: SharedString, palette: &MandaraPalette) -> AnyElement {
    let tint = readable(if ok { palette.success_emerald } else { palette.urgent_coral }, palette);
    div()
        .absolute()
        .left(px(PAD))
        .right(px(PAD))
        .bottom(px(PAD))
        .flex()
        .justify_center()
        .child(
            div()
                .max_w(px(PANEL_W - PAD * TOAST_SIDE_PADS))
                .h(px(TOAST_H))
                .pl(px(TOAST_PAD_L_PX))
                .pr(px(TOAST_PAD_R_PX))
                .rounded_full()
                .bg(palette.surface_container_high.hsla())
                .shadow(vec![glow(Color::oklch(0.0, 0.0, 0.0).with_alpha(alpha(TOAST_SHADOW, palette)), TOAST_SHADOW_BLUR_PX, 0.0, TOAST_SHADOW_DY_PX)])
                .flex()
                .items_center()
                .gap(px(tokens::SM))
                .child(
                    div()
                        .size(px(TOAST_BADGE_PX))
                        .flex_none()
                        .rounded_full()
                        .bg(tint.with_alpha(TOAST_BADGE_ALPHA).hsla())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(if ok { "check" } else { "close" }, SMALL_ICON_PX, tint)),
                )
                .child(div().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).child(msg)),
        )
        .into_any_element()
}

// ── Panel ────────────────────────────────────────────────────────────────

struct View {
    p: PanelCx,
    m: Shared,
    field: Entity<PassField>,
    size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>,
    /// The unlock sheet that last grabbed keyboard focus.
    focused_sheet: RefCell<Option<String>>,
}

impl View {
    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let m = &self.m;
        m.borrow().views.track(window);
        let open = self.p.progress() > 0.0;
        if open {
            let every = usage_every(cx);
            let due = m.borrow().usage_at.is_none_or(|t| t.elapsed() >= every);
            if due {
                let mut mb = m.borrow_mut();
                mb.usage_at = Some(Instant::now());
                mb.service.send(DrivesCommand::Refresh);
            }
            redraw_in(every, window, cx);
            let watch = m.borrow().close_watch.clone();
            if !watch.get() {
                // Closing drops panel-only state: the toast, an idle unlock
                // sheet and any half-typed passphrase (wiped).
                let (freed, field) = (m.clone(), self.field.clone());
                super::processes::on_panel_close(&self.p, &watch, cx, move |cx| {
                    field.update(cx, |f, _| f.clear());
                    let mut mb = freed.borrow_mut();
                    mb.status = None;
                    if mb.unlock.as_ref().is_some_and(|s| !s.busy) {
                        mb.unlock = None;
                    }
                });
            }
        }
        let show_system = show_system(cx);
        let warn = full_warn(cx);
        let shape = PanelShape::of(cx);
        let card_r = shape.inner(PAD);

        // Transient status.
        let status = {
            let mut mb = m.borrow_mut();
            let toast_for = crate::state::config(cx).read(cx).snapshot().panels.toast();
            if mb.status.as_ref().is_some_and(|(_, _, at)| at.elapsed() >= toast_for) {
                mb.status = None;
            }
            mb.status.clone()
        };
        if let Some((_, _, at)) = &status {
            redraw_in(crate::state::config(cx).read(cx).snapshot().panels.toast().saturating_sub(at.elapsed()) + EXPIRY_MARGIN, window, cx);
        }

        // Focus the passphrase field once per opened sheet.
        let sheet_key = m.borrow().unlock.as_ref().map(|s| s.device.clone());
        if *self.focused_sheet.borrow() != sheet_key {
            if sheet_key.is_some() && open {
                self.field.update(cx, |f, cx| {
                    f.clear();
                    f.focus.focus(window, cx);
                });
            }
            *self.focused_sheet.borrow_mut() = sheet_key;
        }

        // Read the state in place (no per-frame clone of every drive).
        let mb = m.borrow();
        let (state, received) = (&mb.state, mb.received);
        let accent = readable(palette.primary, palette);
        let mounted = state.drives.iter().flat_map(|d| d.volumes.iter()).filter(|v| v.mounted() && (show_system || !v.system)).count();
        let subtitle = if !received {
            t("Reading drives…").to_string()
        } else if !state.available {
            t("UDisks2 is not available").to_string()
        } else {
            tf("{drives} drives · {mounted} mounted", &[("drives", &state.drives.len()), ("mounted", &mounted)])
        };
        let refresh = {
            let m = m.clone();
            icon_button("drv-refresh", "refresh", REFRESH_BUTTON_PX, palette.on_surface_variant, None, move |_, cx| {
                let views = {
                    let mut mb = m.borrow_mut();
                    mb.usage_at = Some(Instant::now());
                    mb.service.send(DrivesCommand::Refresh);
                    mb.views.clone()
                };
                views.notify(cx);
            })
            .into_any_element()
        };
        let head = header("disk", accent, t("Drives"), subtitle, palette, Some(refresh));

        // ── Body ─────────────────────────────────────────────────────────
        let mut blocks: Vec<AnyElement> = Vec::new();
        let mut list_h = 0.0;
        if !received {
            blocks.push(empty_state("disk", t("Reading drives…"), t("Asking UDisks2 for storage devices"), EMPTY_H, palette).into_any_element());
            list_h = EMPTY_H;
        } else if !state.available {
            blocks.push(empty_state("alert", t("Storage service unavailable"), t("UDisks2 isn't running — install or start udisks2"), EMPTY_H, palette).into_any_element());
            list_h = EMPTY_H;
        } else {
            let removable = state.drives.iter().filter(|d| d.is_removable());
            let internal = state.drives.iter().filter(|d| !d.is_removable());
            let (n_removable, n_internal) = (removable.clone().count(), internal.clone().count());
            let count = |n: usize| div().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(n.to_string()).into_any_element();
            blocks.push(section(t("Removable"), palette, (n_removable > 0).then(|| count(n_removable))).into_any_element());
            list_h += SECTION_H + SECTION_GAP;
            if n_removable == 0 {
                blocks.push(empty_state("usb", t("No removable drives"), t("Plug in a USB drive or SD card to see it here"), SMALL_EMPTY_H, palette).into_any_element());
                list_h += SMALL_EMPTY_H + SECTION_GAP;
            } else {
                for d in removable {
                    let (card, h) = drive_card(d, m, palette, card_r, show_system, warn);
                    blocks.push(card);
                    list_h += h + SECTION_GAP;
                }
            }
            if n_internal > 0 {
                blocks.push(div().h(px(CARD_GAP - SECTION_GAP)).into_any_element());
                blocks.push(section(t("This computer"), palette, Some(count(n_internal))).into_any_element());
                list_h += CARD_GAP - SECTION_GAP + SECTION_H + SECTION_GAP;
                for d in internal {
                    let (card, h) = drive_card(d, m, palette, card_r, show_system, warn);
                    blocks.push(card);
                    list_h += h + SECTION_GAP;
                }
            }
            list_h -= SECTION_GAP;
        }
        let clipped = list_h > LIST_MAX_H;
        let list_h = list_h.min(LIST_MAX_H);

        let sheet = mb.unlock.as_ref().map(|s| unlock_card(s, &self.field, m, palette, card_r));
        drop(mb);
        let sheet_h = if sheet.is_some() { UNLOCK_H + GAP } else { 0.0 };
        let height = PAD * 2.0 + HEADER_H + GAP + sheet_h + list_h;
        if (f32::from(self.size_cell.get().height) - height).abs() > RESIZE_EPSILON_PX {
            self.size_cell.set(size(px(PANEL_W), px(height)));
        }

        let fill = crate::shell::frame_color(palette);
        let list = div()
            .relative()
            .h(px(list_h))
            .child(div().id("drv-list").size_full().overflow_y_scroll().flex().flex_col().gap(px(SECTION_GAP)).children(blocks))
            .when(clipped, |el| el.child(div().absolute().left_0().right_0().bottom_0().h(px(LIST_FADE_PX)).bg(oklab_gradient(ANGLE_DOWN_DEG, fill.with_alpha(0.0), fill))));

        let p = &self.p;
        div()
            .relative()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .child(reveal(p, 0, head))
            .child(div().h(px(GAP)))
            .when_some(sheet, |el, s| el.child(reveal(p, 1, s)).child(div().h(px(GAP))))
            .child(reveal(p, 2, list))
            .when_some(status, |el, (ok, msg, _)| el.child(toast(ok, msg, palette)))
            .into_any_element()
    }
}

/// The bar chip: drive glyph, removable count, and a glow pulse after a
/// plug-in.
fn chip(m: &Shared, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let (count, pulse) = {
        let mb = m.borrow();
        mb.views.track(window);
        (mb.state.removable().count(), mb.pulse().filter(|_| notify_on_plug(cx)))
    };
    if pulse.is_some() {
        // Frame-synced while (and only while) the plug-in pulse runs.
        window.request_animation_frame();
    }
    let accent = readable(palette.primary, palette);
    let k = pulse.unwrap_or(0.0);
    let tint = if pulse.is_some() { palette.on_surface.mix(accent, PULSE_TINT_BASE + (1.0 - PULSE_TINT_BASE) * k) } else { palette.on_surface };
    div()
        .flex()
        .items_center()
        .gap(px(CHIP_GAP_PX))
        .child(
            div()
                .size(px(crate::style::bar_icon(CHIP_GLYPH_BOX_PX)))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .when(pulse.is_some(), |el| {
                    el.bg(accent.with_alpha(PULSE_FILL_ALPHA * k).hsla()).shadow(vec![glow(accent.with_alpha(PULSE_GLOW_ALPHA * k), PULSE_GLOW_BLUR_PX * k, PULSE_GLOW_SPREAD_PX * k, 0.0)])
                })
                .child(icon(if count > 0 { "usb" } else { "disk" }, crate::style::bar_icon(CHIP_GLYPH_PX), tint)),
        )
        .when(count > 0, |el| el.child(div().font_features(tabular_nums()).text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(tint.hsla()).child(count.to_string())))
        .into_any_element()
}

/// The live service, or a short-lived one for a CLI call (it drains the
/// queued command and exits once the handle drops).
fn service() -> DrivesService {
    live().map(|m| m.borrow().service.clone()).unwrap_or_else(spawn_drives_service)
}

fn request(_cx: &mut App, command: DrivesCommand) -> Result<Output, String> {
    Ok(Output::from(service().request(command)))
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Drives"), tk("Mounted filesystems and removable drives: mount, unlock and safely remove"), Some(Zone::Right)), |window, cx, panel_cx| {
        let m = shared(cx);
        let bar = {
            let m = m.clone();
            fn_view(None, move |palette, window, cx| chip(&m, palette, window, cx), window, cx)
        };
        let (m_submit, m_cancel) = (m.clone(), m.clone());
        let field = cx.new(|cx| {
            PassField::new(
                Rc::new(move |secret, _, cx| submit_unlock(&m_submit, secret, cx)),
                Rc::new(move |_, cx| {
                    let views = {
                        let mut mb = m_cancel.borrow_mut();
                        mb.unlock = None;
                        mb.views.clone()
                    };
                    views.notify(cx);
                }),
                cx,
            )
        });
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(EMPTY_H + HEADER_H + PAD * 2.0 + GAP))));
        let view = View { p: panel_cx.clone(), m, field, size_cell: size_cell.clone(), focused_sheet: RefCell::new(None) };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
    });
    r.command(
        ID,
        CommandSpec::new_output("drives list", tk("Drives and their volumes: filesystem, mount points, usage, encryption (JSON)"), |_, cx| {
            if let Some(m) = live().filter(|m| m.borrow().received) {
                return Ok(Output::Value(json!(m.borrow().state.drives)));
            }
            request(cx, DrivesCommand::List)
        })
        .example("drives list"),
    );
    let device = || ArgSpec::required("device", ArgKind::String, tk("device node (/dev/sdb1 or sdb1), label or UUID"));
    r.command(ID, CommandSpec::new_output("drives mount", tk("Mount a volume (polkit may ask for a password)"), |a, cx| request(cx, DrivesCommand::Mount(a.str(0)?.to_string()))).arg(device()).example("drives mount sdb1"));
    r.command(ID, CommandSpec::new_output("drives unmount", tk("Unmount a volume"), |a, cx| request(cx, DrivesCommand::Unmount(a.str(0)?.to_string()))).arg(device()).example("drives unmount sdb1"));
    r.command(
        ID,
        CommandSpec::new_output("drives eject", tk("Safely remove a drive: unmount and lock its volumes, then eject or power it off"), |a, cx| request(cx, DrivesCommand::SafelyRemove(a.str(0)?.to_string())))
            .arg(ArgSpec::required("device", ArgKind::String, tk("drive (/dev/sdb or its model) or any of its volumes")))
            .example("drives eject sdb"),
    );
    r.command(
        ID,
        CommandSpec::new_output("drives unlock", tk("Unlock an encrypted (LUKS) volume and mount it"), |a, cx| {
            let device = a.str(0)?.to_string();
            let passphrase = a.str(1)?.to_string();
            request(cx, DrivesCommand::Unlock { device, passphrase })
        })
        .arg(device())
        .arg(ArgSpec::required("passphrase", ArgKind::String, tk("LUKS passphrase (visible in the process list — prefer the panel)")))
        .example("drives unlock sdb2 'correct horse'"),
    );
    r.command(ID, CommandSpec::new_output("drives lock", tk("Unmount and lock an unlocked encrypted volume"), |a, cx| request(cx, DrivesCommand::Lock(a.str(0)?.to_string()))).arg(device()).example("drives lock sdb2"));
    r.command(ID, CommandSpec::new_output("drives open", tk("Open a volume in the file manager (mounting it first if needed)"), |a, cx| request(cx, DrivesCommand::Open(a.str(0)?.to_string()))).arg(device()).example("drives open PHOTOS"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(seq: u64, name: &str) -> DrivesState {
        DrivesState { available: true, plug_seq: seq, last_plugged: Some(name.into()), ..Default::default() }
    }

    #[test]
    fn first_state_never_announces_and_later_plugs_pulse() {
        let mut m = Model::new(spawn_drives_service());
        assert_eq!(m.absorb(state(3, "Old")), None, "drives present at startup are not announced");
        assert!(m.pulse().is_none());
        assert_eq!(m.absorb(state(3, "Old")), None);
        assert_eq!(m.absorb(state(4, "Stick")).as_deref(), Some("Stick"));
        let k = m.pulse().expect("pulsing right after a plug-in");
        assert!((0.0..=1.0).contains(&k));
    }

    #[test]
    fn unlock_sheet_closes_when_volume_goes_away() {
        let mut m = Model::new(spawn_drives_service());
        m.unlock = Some(UnlockSheet { device: "/b/sdb2".into(), name: "vault".into(), error: None, busy: false });
        m.absorb(DrivesState { available: true, ..Default::default() });
        assert!(m.unlock.is_none());
    }

    #[test]
    fn names_and_kinds() {
        let v = Volume { size: 32_000_000_000, ..Default::default() };
        assert_eq!(volume_name(&v), tf("{size} volume", &[("size", &format_bytes(32_000_000_000))]));
        let labelled = Volume { label: "PHOTOS".into(), ..Default::default() };
        assert_eq!(volume_name(&labelled), "PHOTOS");
        let root = Volume { mount_points: vec!["/home".into(), "/".into()], ..Default::default() };
        assert_eq!(volume_name(&root), t("System"));
        let data = Volume { mount_points: vec!["/mnt/media/projects".into(), "/mnt/media/archive".into()], ..Default::default() };
        assert_eq!(volume_name(&data), "archive");
        assert_eq!(drive_kind(&Drive { bus: "usb".into(), ..Default::default() }), t("USB drive"));
        assert_eq!(drive_kind(&Drive { rotation_rate: 0, ..Default::default() }), t("Solid-state drive"));
        assert!(ink_on(Color::oklch(0.85, 0.1, 30.0)).lightness() < 0.4);
    }
}
