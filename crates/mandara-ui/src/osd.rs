//! On-screen displays: a small `mandara-osd` layer surface showing an
//! elegant chromatic-glass pill (icon chip, spring-animated level bar and a
//! tabular value). It springs in, follows live changes, fades out after
//! `osd.hold_ms` and then the surface is destroyed (nothing stays mapped).
//!
//! Kinds: volume, microphone, brightness, keyboard layout, Caps / Num /
//! Scroll Lock, keyboard backlight, now playing, power profile, do not
//! disturb, night light, privacy (microphone / camera / screen sharing),
//! audio output switches, battery alerts and custom ones from scripts
//! (`mandara msg osd show …`). Each kind has its own switch in
//! `[osd.kinds]`; `general.osd` switches all of it off.
//!
//! Placement: `general.osd_position` (bottom / top / centre, or a vertical
//! pill on the left / right edge), `osd.offset_*`, `osd.scale`; the output
//! follows `[displays.surfaces.osd]` (the focused one when allowed).
//! The surface respects exclusive zones, so the pill always clears the bar.
//!
//! Other modules call [`show`] with an [`OsdEvent`] (or [`show_kind`] with
//! a kind and an [`OsdPayload`]).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use gpui::{App, AppContext as _, Context, Entity, FontWeight, Global, IntoElement, ParentElement, Render, Styled, Window, WindowHandle, div, prelude::FluentBuilder as _, px};
use mandara_config::{OsdPosition, ShellConfig};
use mandara_core::{FrameClock, InputMaskRegistry, LayerSurfaceSpec, SpringConfig, SpringValue, SurfaceRole, ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PluginRegistry};
use mandara_services::AudioState;
use mandara_services::backlight;
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use crate::fonts::{fonts, tabular_nums};
use crate::hub::Hub;
use crate::icons::{ICON_NAMES, icon};
use crate::kit::text;
use crate::plugins::panel_fx::{elevation, partner, readable, surface_edge};
use crate::state::config;

/// Volume changes smaller than this (fraction of 100 %) don't pop the OSD.
const VOLUME_EPSILON: f32 = 0.004;
/// The surface counts as configured once within this many px of its size.
const SIZE_TOLERANCE_PX: f32 = 2.0;
/// Presence below which a hidden OSD closes its surface.
const CLOSE_PRESENCE: f32 = 0.01;
/// Largest presence overshoot honoured, and the travel toward the edge
/// while entering / leaving (px; half of it for a centred OSD).
const PRESENCE_OVERSHOOT_MAX: f32 = 1.2;
const ENTER_TRAVEL_PX: f32 = 18.0;
const CENTER_TRAVEL_FACTOR: f32 = 0.5;
/// Icon chip metrics and gradients.
const CHIP_PX: f32 = 38.0;
const CHIP_ICON_PX: f32 = 19.0;
const CHIP_GRADIENT_DEG: f32 = 135.0;
const CHIP_GLOW_ALPHA: f32 = 0.30;
const CHIP_GLOW_BLUR_PX: f32 = 10.0;
/// Muted chip / level-bar tints (from → to).
const MUTED_CHIP_ALPHAS: (f32, f32) = (0.22, 0.14);
const MUTED_BAR_ALPHAS: (f32, f32) = (0.40, 0.28);
/// Levels above 100 % (+ this slack) render as "boosted" (amber).
const BOOST_THRESHOLD: f32 = 1.0005;
/// Highest level drawn (boosted volume).
const MAX_LEVEL: f32 = 1.5;
/// Value column min width and body gaps, in px.
const VALUE_MIN_W_PX: f32 = 44.0;
const BODY_GAP_PX: f32 = 7.0;
const TITLE_GAP_PX: f32 = 1.0;
/// Pill padding (chip side / far side) and chip gap, in px.
const PILL_PAD_START_PX: f32 = 8.0;
const PILL_PAD_END_PX: f32 = 20.0;
const PILL_GAP_PX: f32 = 12.0;
/// Pill sheen: lightness lift at the top (dark / light) and drop at the bottom.
const SHEEN_TOP_DARK: f32 = 0.02;
const SHEEN_TOP_LIGHT: f32 = 0.01;
const SHEEN_BOTTOM: f32 = -0.008;
/// CSS gradient angles, in degrees.
const TOP_TO_BOTTOM_DEG: f32 = 180.0;
const LEFT_TO_RIGHT_DEG: f32 = 90.0;
const BOTTOM_TO_TOP_DEG: f32 = 0.0;
/// Level bar: thickness, track alpha (dark / light), tip glow.
const LEVEL_BAR_H_PX: f32 = 6.0;
const TRACK_ALPHA_DARK: f32 = 0.10;
const TRACK_ALPHA_LIGHT: f32 = 0.08;
const TIP_GLOW_MIN_LEVEL: f32 = 0.02;
const TIP_GLOW_ALPHA: f32 = 0.35;
const TIP_GLOW_BLUR_PX: f32 = 6.0;
/// On/off badge metrics and tint (dark / light).
const BADGE_H_PX: f32 = 24.0;
const BADGE_PAD_PX: f32 = 11.0;
const BADGE_TEXT_BUMP_PX: f32 = 0.5;
const BADGE_ALPHA_DARK: f32 = 0.18;
const BADGE_ALPHA_LIGHT: f32 = 0.13;
/// Default brightness step and percent scale for the commands.
const DEFAULT_STEP_PERCENT: f32 = 5.0;
const PERCENT: f32 = 100.0;
/// Slack after the hold so the fade-out wake lands after the deadline.
const FADE_WAKE_SLACK: Duration = Duration::from_millis(16);
/// Shortest hold accepted (so a typo can't make the OSD flash invisibly).
const MIN_HOLD_MS: u64 = 300;
/// Horizontal pill.
const PILL_W: f32 = 300.0;
const PILL_H: f32 = 54.0;
/// Vertical pill (left / right positions, level kinds).
const VPILL_W: f32 = 54.0;
const VPILL_H: f32 = 212.0;
const VPILL_PAD_PX: f32 = 8.0;
const VPILL_GAP_PX: f32 = 10.0;
/// Room around the pill for its shadow and the entrance travel.
const MARGIN_X: f32 = 24.0;
const MARGIN_Y: f32 = 26.0;
/// Gap between the screen edge (or the bar's reserved band) and the pill.
const EDGE_GAP: f32 = 30.0;
/// Scale bounds.
const SCALE_RANGE: (f32, f32) = (0.6, 1.8);
/// Slightly under-damped: the pill lands with a soft bounce.
const PRESENCE_SPRING: SpringConfig = SpringConfig::new(380.0, 24.0, 1.0);
const LEVEL_SPRING: SpringConfig = SpringConfig::new(520.0, 40.0, 1.0);
/// Icon for custom OSDs that name no (known) icon.
const CUSTOM_ICON: &str = "bell";

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OsdKind {
    Volume,
    Mic,
    Brightness,
    Layout,
    CapsLock,
    NumLock,
    ScrollLock,
    KeyboardBacklight,
    Media,
    PowerProfile,
    DoNotDisturb,
    NightLight,
    Privacy,
    OutputDevice,
    Battery,
    Custom,
}

impl OsdKind {
    pub const ALL: [OsdKind; 16] = [
        OsdKind::Volume,
        OsdKind::Mic,
        OsdKind::Brightness,
        OsdKind::Layout,
        OsdKind::CapsLock,
        OsdKind::NumLock,
        OsdKind::ScrollLock,
        OsdKind::KeyboardBacklight,
        OsdKind::Media,
        OsdKind::PowerProfile,
        OsdKind::DoNotDisturb,
        OsdKind::NightLight,
        OsdKind::Privacy,
        OsdKind::OutputDevice,
        OsdKind::Battery,
        OsdKind::Custom,
    ];

    pub fn id(self) -> &'static str {
        match self {
            OsdKind::Volume => "volume",
            OsdKind::Mic => "mic",
            OsdKind::Brightness => "brightness",
            OsdKind::Layout => "layout",
            OsdKind::CapsLock => "capslock",
            OsdKind::NumLock => "numlock",
            OsdKind::ScrollLock => "scrolllock",
            OsdKind::KeyboardBacklight => "kbd-backlight",
            OsdKind::Media => "media",
            OsdKind::PowerProfile => "power-profile",
            OsdKind::DoNotDisturb => "dnd",
            OsdKind::NightLight => "night-light",
            OsdKind::Privacy => "privacy",
            OsdKind::OutputDevice => "output-device",
            OsdKind::Battery => "battery",
            OsdKind::Custom => "custom",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "mic" | "microphone" => OsdKind::Mic,
            "layout" | "keyboard" => OsdKind::Layout,
            "capslock" | "caps" => OsdKind::CapsLock,
            "numlock" | "num" => OsdKind::NumLock,
            "scrolllock" | "scroll" => OsdKind::ScrollLock,
            other => return OsdKind::ALL.into_iter().find(|k| k.id() == other),
        })
    }

    /// Whether `[osd.kinds]` lets this kind show.
    pub fn enabled_in(self, cfg: &ShellConfig) -> bool {
        let k = &cfg.osd.kinds;
        match self {
            OsdKind::Volume => k.volume,
            OsdKind::Mic => k.microphone,
            OsdKind::Brightness => k.brightness,
            OsdKind::Layout => k.keyboard_layout,
            OsdKind::CapsLock | OsdKind::NumLock | OsdKind::ScrollLock => k.lock_keys,
            OsdKind::KeyboardBacklight => k.keyboard_backlight,
            OsdKind::Media => k.media,
            OsdKind::PowerProfile => k.power_profile,
            OsdKind::DoNotDisturb => k.do_not_disturb,
            OsdKind::NightLight => k.night_light,
            OsdKind::Privacy => k.privacy,
            OsdKind::OutputDevice => k.output_device,
            OsdKind::Battery => true,
            OsdKind::Custom => k.custom,
        }
    }

    /// On / off kinds (drawn with a state badge).
    fn is_toggle(self) -> bool {
        matches!(self, OsdKind::CapsLock | OsdKind::NumLock | OsdKind::ScrollLock | OsdKind::DoNotDisturb | OsdKind::NightLight | OsdKind::Privacy)
    }
}

/// What a caller hands [`show_kind`]: a level, an on/off state and text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OsdPayload {
    /// `0..=1` (volume may reach 1.5); `None` draws no level bar.
    pub level: Option<f32>,
    /// Off / muted (dims the chip; toggles show "Off").
    pub off: bool,
    /// Main text (layout name, profile, track…) or the toggle's detail.
    pub label: String,
    /// Replaces the kind's title.
    pub title: Option<String>,
    /// A bundled icon name replacing the kind's icon.
    pub icon: Option<String>,
}

impl OsdPayload {
    pub fn level(level: f32) -> Self {
        Self { level: Some(level), ..Default::default() }
    }

    pub fn toggle(on: bool) -> Self {
        Self { off: !on, ..Default::default() }
    }

    pub fn text(label: impl Into<String>) -> Self {
        Self { label: label.into(), ..Default::default() }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }
}

/// One thing to display.
#[derive(Clone, Debug, PartialEq)]
pub struct OsdEvent {
    pub kind: OsdKind,
    /// `0..=1` (volume may go to 1.5 when boosted); `None` hides the bar.
    pub level: Option<f32>,
    /// Muted / off.
    pub muted: bool,
    /// Free text (keyboard layout name, track, profile…).
    pub label: String,
    /// Title override (custom OSDs, the track title).
    pub title: Option<String>,
    /// Icon override (a bundled icon name).
    pub icon: Option<String>,
}

impl OsdEvent {
    pub fn new(kind: OsdKind, payload: OsdPayload) -> Self {
        Self { kind, level: payload.level, muted: payload.off, label: payload.label, title: payload.title, icon: payload.icon }
    }

    pub fn volume(audio: &AudioState) -> Self {
        Self::new(OsdKind::Volume, OsdPayload { level: Some(audio.output.volume), off: audio.output.muted, ..Default::default() })
    }

    /// Global mute ("Mute all sound") switched on / off.
    pub fn mute_all(on: bool) -> Self {
        let label = if on { t("Every output and app is silent") } else { t("Restored as it was") };
        Self::new(OsdKind::Volume, OsdPayload { off: on, ..Default::default() }.with_title(t("All sound")).with_label(label).with_icon(if on { "volume-off" } else { "volume-high" }))
    }

    pub fn mic(audio: &AudioState) -> Self {
        Self::new(OsdKind::Mic, OsdPayload { level: Some(audio.input.volume), off: audio.input.muted, ..Default::default() })
    }

    pub fn brightness(level: f32) -> Self {
        Self::new(OsdKind::Brightness, OsdPayload::level(level))
    }

    pub fn layout(name: &str) -> Self {
        Self::new(OsdKind::Layout, OsdPayload::text(name))
    }

    pub fn caps(on: bool) -> Self {
        Self::new(OsdKind::CapsLock, OsdPayload::toggle(on))
    }

    pub fn num_lock(on: bool) -> Self {
        Self::new(OsdKind::NumLock, OsdPayload::toggle(on))
    }

    pub fn scroll_lock(on: bool) -> Self {
        Self::new(OsdKind::ScrollLock, OsdPayload::toggle(on))
    }

    pub fn keyboard_backlight(level: f32) -> Self {
        Self::new(OsdKind::KeyboardBacklight, OsdPayload::level(level))
    }

    /// Now playing: `title` by `artist`.
    pub fn media(title: &str, artist: &str) -> Self {
        Self::new(OsdKind::Media, OsdPayload::text(artist).with_title(title))
    }

    pub fn power_profile(profile: &str) -> Self {
        Self::new(OsdKind::PowerProfile, OsdPayload::text(profile_label(profile)))
    }

    pub fn dnd(on: bool) -> Self {
        Self::new(OsdKind::DoNotDisturb, OsdPayload::toggle(on))
    }

    /// Night light on / off, with the temperature in kelvin when known.
    pub fn night_light(on: bool, kelvin: Option<u32>) -> Self {
        Self::new(OsdKind::NightLight, OsdPayload::toggle(on).with_label(kelvin.map(|k| format!("{k} K")).unwrap_or_default()))
    }

    /// `what` ("Microphone", "Camera", "Screen sharing") started / stopped
    /// by `apps`.
    pub fn privacy(icon: &str, what: String, on: bool, apps: &str) -> Self {
        Self::new(OsdKind::Privacy, OsdPayload::toggle(on).with_title(what).with_label(apps).with_icon(icon))
    }

    pub fn output_device(name: &str) -> Self {
        Self::new(OsdKind::OutputDevice, OsdPayload::text(name))
    }

    /// Battery level `0..=1`, plugged in or not; at or below `low`
    /// (Power → low battery) it reads as low (amber, "off" chip).
    pub fn battery(level: f32, charging: bool, low: f32) -> Self {
        Self::new(OsdKind::Battery, OsdPayload { level: Some(level), off: !charging && level <= low, ..Default::default() })
    }

    fn icon(&self) -> &'static str {
        if let Some(name) = self.icon.as_deref() {
            return ICON_NAMES.iter().copied().find(|n| *n == name).unwrap_or(CUSTOM_ICON);
        }
        let level = self.level.unwrap_or(0.0);
        match self.kind {
            OsdKind::Volume if self.muted || level <= 0.001 => "volume-mute",
            OsdKind::Volume if level < 0.5 => "volume-low",
            OsdKind::Volume => "volume-high",
            OsdKind::Mic if self.muted => "mic-off",
            OsdKind::Mic => "mic",
            OsdKind::Brightness => "sun",
            OsdKind::Layout | OsdKind::KeyboardBacklight => "keyboard",
            OsdKind::CapsLock | OsdKind::NumLock | OsdKind::ScrollLock if self.muted => "lock-open",
            OsdKind::CapsLock | OsdKind::NumLock | OsdKind::ScrollLock => "lock",
            OsdKind::Media => "music",
            OsdKind::PowerProfile => "gauge",
            OsdKind::DoNotDisturb => "moon",
            OsdKind::NightLight => "sunset",
            OsdKind::Privacy => "shield",
            OsdKind::OutputDevice => "headphones",
            OsdKind::Battery => "battery",
            OsdKind::Custom => CUSTOM_ICON,
        }
    }

    fn title(&self) -> String {
        if let Some(title) = &self.title {
            return title.clone();
        }
        match self.kind {
            OsdKind::Volume => t("Volume").to_string(),
            OsdKind::Mic => t("Microphone").to_string(),
            OsdKind::Brightness => t("Brightness").to_string(),
            OsdKind::Layout => t("Keyboard layout").to_string(),
            OsdKind::CapsLock => t("Caps Lock").to_string(),
            OsdKind::NumLock => t("Num Lock").to_string(),
            OsdKind::ScrollLock => t("Scroll Lock").to_string(),
            OsdKind::KeyboardBacklight => t("Keyboard backlight").to_string(),
            OsdKind::Media => t("Now playing").to_string(),
            OsdKind::PowerProfile => t("Power mode").to_string(),
            OsdKind::DoNotDisturb => t("Do not disturb").to_string(),
            OsdKind::NightLight => t("Night light").to_string(),
            OsdKind::Privacy => t("Privacy").to_string(),
            OsdKind::OutputDevice => t("Sound output").to_string(),
            OsdKind::Battery => t("Battery").to_string(),
            OsdKind::Custom => String::new(),
        }
    }

    /// The right-hand readout (tabular numerals).
    fn value(&self) -> String {
        match self.kind {
            OsdKind::Volume | OsdKind::Mic if self.muted => t("Muted").to_string(),
            // Global mute: an on / off pill without a level.
            OsdKind::Volume if self.level.is_none() => t("On").to_string(),
            k if k.is_toggle() => if self.muted { t("Off") } else { t("On") }.to_string(),
            _ if self.level.is_none() => String::new(),
            _ => format!("{}%", (self.level.unwrap_or(0.0) * PERCENT).round() as i32),
        }
    }

    /// The body shape this event uses.
    fn body(&self) -> Body {
        if self.level.is_some() {
            Body::Level
        } else if self.kind.is_toggle() || self.kind == OsdKind::Volume {
            Body::Toggle
        } else {
            Body::Label
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Body {
    Level,
    Toggle,
    Label,
}

/// A readable name for a power-profiles-daemon profile.
pub fn profile_label(profile: &str) -> String {
    match profile {
        "power-saver" => t("Power saver").to_string(),
        "balanced" => t("Balanced").to_string(),
        "performance" => t("Performance").to_string(),
        other => other.to_string(),
    }
}

/// Shared OSD state (one per shell).
pub struct Osd {
    event: Option<OsdEvent>,
    shown_at: Option<Instant>,
    window: Option<WindowHandle<OsdView>>,
    /// The geometry the open surface was created with.
    geometry: Option<Geometry>,
    /// Keyboard-LED sampling only runs while OSDs are on.
    leds_enabled: Arc<AtomicBool>,
    last_audio: Option<AudioState>,
    last_layout: String,
    /// The single pending wake-up that fades the OSD out after the hold;
    /// replaced (not stacked) by every new event, so a burst of volume
    /// steps leaves one timer instead of one per step.
    fade_wake: Option<gpui::Task<()>>,
}

#[derive(Clone)]
struct GlobalOsd(Entity<Osd>);
impl Global for GlobalOsd {}

fn osd(cx: &App) -> Option<Entity<Osd>> {
    cx.try_global::<GlobalOsd>().map(|g| g.0.clone())
}

fn enabled(cx: &App) -> bool {
    config(cx).read(cx).get().general.osd
}

fn hold(cfg: &ShellConfig) -> Duration {
    Duration::from_millis(cfg.osd.hold_ms.max(MIN_HOLD_MS))
}

/// Surface and pill sizes for the current settings.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Geometry {
    position: OsdPosition,
    scale: f32,
    offset: (f32, f32),
    surface: (f32, f32),
}

impl Geometry {
    fn of(cfg: &ShellConfig) -> Self {
        let scale = cfg.osd.scale.clamp(SCALE_RANGE.0, SCALE_RANGE.1);
        let position = cfg.general.osd_position;
        let w = (PILL_W + MARGIN_X * 2.0) * scale;
        let h = if position.is_vertical() { (VPILL_H + MARGIN_Y * 2.0) * scale } else { (PILL_H + MARGIN_Y * 2.0) * scale };
        Self { position, scale, offset: (cfg.osd.offset_x, cfg.osd.offset_y), surface: (w.round(), h.round()) }
    }
}

/// Shows `kind` with `payload` (respecting `general.osd` and the kind's
/// switch). Returns whether it is shown.
pub fn show_kind(kind: OsdKind, payload: OsdPayload, cx: &mut App) -> bool {
    show(OsdEvent::new(kind, payload), cx)
}

/// Shows `event` (respecting `general.osd` and `[osd.kinds]`). Returns
/// whether it is shown.
pub fn show(event: OsdEvent, cx: &mut App) -> bool {
    let Some(entity) = osd(cx) else { return false };
    let cfg = config(cx).read(cx).snapshot();
    if !cfg.general.osd || !event.kind.enabled_in(&cfg) {
        return false;
    }
    entity.update(cx, |o, cx| {
        o.event = Some(event);
        o.shown_at = Some(Instant::now());
        cx.notify();
    });
    let geometry = Geometry::of(&cfg);
    let (open, stale) = {
        let o = entity.read(cx);
        let open = o.window.as_ref().is_some_and(|h| cx.windows().iter().any(|w| w.window_id() == h.window_id()));
        (open, o.geometry != Some(geometry))
    };
    if open && stale {
        // Position / scale changed since it opened: reopen in the new place.
        if let Some(h) = entity.update(cx, |o, _| o.window.take()) {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
    }
    if !open || stale {
        let spec = surface_spec(&cfg, geometry, cx);
        match cx.open_window(spec.window_options(), |window, cx| cx.new(|cx| OsdView::new(window, cx))) {
            Ok(handle) => entity.update(cx, |o, _| {
                o.window = Some(handle);
                o.geometry = Some(geometry);
            }),
            Err(err) => tracing::warn!("osd: {err:#}"),
        }
    }
    // Wake the view once the hold expires so it can fade out.
    let wait = hold(&cfg) + FADE_WAKE_SLACK;
    entity.update(cx, |o, cx| {
        o.fade_wake = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |o, cx| {
                o.fade_wake = None;
                cx.notify();
            });
        }));
    });
    true
}

fn surface_spec(cfg: &ShellConfig, g: Geometry, cx: &App) -> LayerSurfaceSpec {
    let s = g.scale;
    let gap = (EDGE_GAP - MARGIN_Y) * s;
    let (ox, oy) = g.offset;
    // Offsets push away from the anchored edge (and nudge centred pills).
    let (anchor, margin) = match g.position {
        OsdPosition::Bottom => (Anchor::BOTTOM, (px(0.0), px(0.0), px(gap + oy), px(0.0))),
        OsdPosition::Top => (Anchor::TOP, (px(gap + oy), px(0.0), px(0.0), px(0.0))),
        OsdPosition::Center => (Anchor::empty(), (px(oy.max(0.0)), px(0.0), px((-oy).max(0.0)), px(0.0))),
        OsdPosition::Left => (Anchor::LEFT, (px(0.0), px(0.0), px(0.0), px(gap + ox))),
        OsdPosition::Right => (Anchor::RIGHT, (px(0.0), px(gap + ox), px(0.0), px(0.0))),
    };
    let _ = cfg;
    LayerSurfaceSpec {
        role: SurfaceRole::Osd,
        layer: Layer::Overlay,
        anchor,
        size: gpui::size(px(g.surface.0), px(g.surface.1)),
        margin,
        // Respect exclusive zones: the pill always clears the bar's band.
        exclusive_zone: Some(px(0.0)),
        keyboard: KeyboardInteractivity::None,
        display: crate::toasts::target_display(mandara_config::outputs::SurfaceKind::Osd, cx),
    }
}

/// Installs the OSD and its automatic triggers.
pub fn init(hub: &Entity<Hub>, cx: &mut App) {
    let leds_enabled = Arc::new(AtomicBool::new(enabled(cx)));
    let entity = cx.new(|_| Osd {
        event: None,
        shown_at: None,
        window: None,
        geometry: None,
        leds_enabled: leds_enabled.clone(),
        last_audio: None,
        last_layout: String::new(),
        fade_wake: None,
    });
    cx.set_global(GlobalOsd(entity.clone()));

    // Audio and keyboard layout, from the hub.
    let osd_entity = entity.clone();
    cx.observe(hub, move |hub, cx| {
        let (audio, layout) = {
            let h = hub.read(cx);
            (h.audio.clone(), h.keyboard_layout.clone())
        };
        let (prev_audio, prev_layout) = {
            let o = osd_entity.read(cx);
            (o.last_audio.clone(), o.last_layout.clone())
        };
        if prev_audio.as_ref() != Some(&audio) || prev_layout != layout {
            osd_entity.update(cx, |o, _| {
                o.last_audio = Some(audio.clone());
                o.last_layout = layout.clone();
            });
        }
        let labels = crate::state::config(cx).read(cx).get().audio.clone();
        if let Some(event) = prev_audio.as_ref().and_then(|prev| audio_change(prev, &audio, &labels)) {
            show(event, cx);
        }
        if !prev_layout.is_empty() && !layout.is_empty() && layout != prev_layout {
            show(OsdEvent::layout(&layout), cx);
        }
    })
    .detach();

    // Follow the toggle: pause LED sampling, close a visible pill.
    let osd_entity = entity.clone();
    cx.observe(&config(cx), move |cfg, cx| {
        let c = cfg.read(cx).get();
        let on = c.general.osd && (c.osd.kinds.lock_keys || c.osd.kinds.keyboard_backlight);
        let master = c.general.osd;
        let flag = osd_entity.read(cx).leds_enabled.clone();
        flag.store(on, Ordering::Relaxed);
        if !master && let Some(h) = osd_entity.update(cx, |o, _| o.window.take()) {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
    })
    .detach();

    if let Some(b) = backlight::primary() {
        remember_backlight(b.level());
    }
    if let Some(rx) = backlight::watch_backlight() {
        cx.spawn(async move |cx| {
            while let Ok(b) = rx.recv().await {
                remember_backlight(b.level());
                let _ = cx.update(|cx| {
                    show(OsdEvent::brightness(b.level()), cx);
                    // The quick settings slider follows the brightness keys.
                    cx.refresh_windows();
                });
            }
        })
        .detach();
    }
    if let Some(rx) = mandara_services::leds::watch(leds_enabled) {
        cx.spawn(async move |cx| {
            use mandara_services::leds::LedChange;
            while let Ok(change) = rx.recv().await {
                let event = match change {
                    LedChange::CapsLock(on) => OsdEvent::caps(on),
                    LedChange::NumLock(on) => OsdEvent::num_lock(on),
                    LedChange::ScrollLock(on) => OsdEvent::scroll_lock(on),
                    LedChange::KeyboardBacklight(level) => OsdEvent::keyboard_backlight(level),
                };
                let _ = cx.update(|cx| show(event, cx));
            }
        })
        .detach();
    }
}

/// What changed between two audio samples, if worth an OSD. A different
/// default output is an output-device OSD, never a volume one.
/// The device popup shows the name given in Studio → Audio, and tells
/// devices apart by their server key (two devices may share a description).
fn audio_change(prev: &AudioState, next: &AudioState, labels: &mandara_config::alerts::AudioConfig) -> Option<OsdEvent> {
    if !prev.available || !next.available {
        return None;
    }
    // Global mute also mutes the default output: one OSD for the switch.
    if prev.all_muted != next.all_muted {
        return Some(OsdEvent::mute_all(next.all_muted));
    }
    let key = |c: &mandara_services::audio::Channel| if c.device.is_empty() { c.name.clone() } else { c.device.clone() };
    let out_same_device = key(&prev.output) == key(&next.output);
    if !out_same_device && !key(&next.output).is_empty() && !key(&prev.output).is_empty() {
        return Some(OsdEvent::output_device(labels.device_label(&next.output.device, &next.output.name)));
    }
    if out_same_device && ((prev.output.volume - next.output.volume).abs() > VOLUME_EPSILON || prev.output.muted != next.output.muted) {
        return Some(OsdEvent::volume(next));
    }
    if prev.input.name == next.input.name && prev.input.muted != next.input.muted {
        return Some(OsdEvent::mic(next));
    }
    None
}

// ── View ─────────────────────────────────────────────────────────────────

pub struct OsdView {
    presence: SpringValue<f32>,
    level: SpringValue<f32>,
    clock: FrameClock,
    mask: InputMaskRegistry,
    last_kind: Option<OsdKind>,
}

impl OsdView {
    fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        if let Some(o) = osd(cx) {
            cx.observe(&o, |_, _, cx| cx.notify()).detach();
        }
        cx.observe(&mandara_core::theme_entity(cx), |_, _, cx| cx.notify()).detach();
        Self { presence: SpringValue::new(0.0, PRESENCE_SPRING), level: SpringValue::new(0.0, LEVEL_SPRING), clock: FrameClock::new(), mask: InputMaskRegistry::new(), last_kind: None }
    }
}

fn settled(window: &Window, g: Geometry) -> bool {
    let vp = window.viewport_size();
    (f32::from(vp.width) - g.surface.0).abs() < SIZE_TOLERANCE_PX && (f32::from(vp.height) - g.surface.1).abs() < SIZE_TOLERANCE_PX
}

impl Render for OsdView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Clicks always pass through: the input region stays empty.
        let commit = self.mask.commit();
        let Some(entity) = osd(cx) else { return div().into_any_element() };
        let (event, shown_at, geometry) = {
            let o = entity.read(cx);
            (o.event.clone(), o.shown_at, o.geometry)
        };
        let Some(g) = geometry else { return div().size_full().child(commit).into_any_element() };
        if !settled(window, g) {
            window.request_animation_frame();
            return div().size_full().child(commit).into_any_element();
        }
        let Some(event) = event else { return div().size_full().child(commit).into_any_element() };
        let cfg = config(cx).read(cx).snapshot();
        let visible = shown_at.is_some_and(|t| t.elapsed() < hold(&cfg)) && cfg.general.osd;
        let reduce = cfg.motion.reduce_motion;
        self.presence.set_instant(reduce);
        self.level.set_instant(reduce);
        self.presence.set_target(if visible { 1.0 } else { 0.0 });
        // A different kind starts its bar from its own value, not a slide
        // from the previous kind's.
        let target = event.level.unwrap_or(0.0).clamp(0.0, MAX_LEVEL);
        if self.last_kind != Some(event.kind) {
            self.last_kind = Some(event.kind);
            self.level.snap_to(target);
        } else {
            self.level.set_target(target);
        }
        let dt = self.clock.tick(Instant::now());
        let a = self.presence.step(dt);
        let b = self.level.step(dt);
        if !visible && self.presence.value() <= CLOSE_PRESENCE && !a {
            entity.update(cx, |o, _| {
                o.window = None;
                o.geometry = None;
            });
            window.remove_window();
            return div().into_any_element();
        }
        if a || b {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        let palette = mandara_core::frame_palette(window, cx);
        let p = self.presence.value();
        // Travel toward the nearest edge while entering / leaving.
        let travel = (1.0 - p.clamp(0.0, PRESENCE_OVERSHOOT_MAX)) * ENTER_TRAVEL_PX * g.scale;
        let (dx, dy) = match g.position {
            OsdPosition::Bottom => (0.0, travel),
            OsdPosition::Top => (0.0, -travel),
            OsdPosition::Center => (0.0, travel * CENTER_TRAVEL_FACTOR),
            OsdPosition::Left => (-travel, 0.0),
            OsdPosition::Right => (travel, 0.0),
        };
        let vertical = g.position.is_vertical() && event.body() == Body::Level;
        let s = g.scale;
        let (pw, ph) = if vertical { (VPILL_W * s, VPILL_H * s) } else { (PILL_W * s, PILL_H * s) };
        // Centre the pill in the surface (left / right pills hug their edge).
        let left = match g.position {
            OsdPosition::Left => MARGIN_X * s,
            OsdPosition::Right => g.surface.0 - MARGIN_X * s - pw,
            _ => (g.surface.0 - pw) / 2.0,
        };
        let top = (g.surface.1 - ph) / 2.0;
        let f = fonts(cx);
        let look = Look { scale: s, opacity: cfg.osd.opacity.clamp(0.0, 1.0), show_value: cfg.osd.show_value, rim: cfg.theme.adjustments.rim_intensity };
        let body = if vertical { vertical_pill(&event, self.level.value(), p, &palette, look).into_any_element() } else { pill(&event, self.level.value(), p, &palette, look).into_any_element() };
        div()
            .size_full()
            .font_family(f.ui.clone())
            .font_weight(crate::style::body_weight())
            .text_color(palette.on_surface.hsla())
            .child(div().absolute().left(px(left + dx)).top(px(top + dy)).opacity(p.clamp(0.0, 1.0)).child(body))
            .child(commit)
            .into_any_element()
    }
}

/// Look settings shared by both pill shapes.
#[derive(Copy, Clone)]
struct Look {
    scale: f32,
    opacity: f32,
    show_value: bool,
    rim: f32,
}

fn chip(event: &OsdEvent, palette: &MandaraPalette, s: f32) -> impl IntoElement {
    let accent = readable(palette.primary, palette);
    let dim = event.muted;
    let (chip_from, chip_to) = if dim {
        (palette.on_surface_variant.with_alpha(MUTED_CHIP_ALPHAS.0), palette.on_surface_variant.with_alpha(MUTED_CHIP_ALPHAS.1))
    } else {
        (palette.primary, partner(palette.primary).with_lightness(palette.primary.lightness()))
    };
    let chip_fg = if dim { palette.on_surface_variant } else { palette.on_primary };
    div()
        .size(px(CHIP_PX * s))
        .flex_none()
        .rounded_full()
        .bg(oklab_gradient(CHIP_GRADIENT_DEG, chip_from, chip_to))
        .when(!dim, |el| el.shadow(vec![crate::plugins::panel_fx::glow(accent.with_alpha(CHIP_GLOW_ALPHA), CHIP_GLOW_BLUR_PX * s, 0.0, 1.0)]))
        .flex()
        .items_center()
        .justify_center()
        .child(icon(event.icon(), CHIP_ICON_PX * s, chip_fg))
}

/// The level bar's gradient: muted grey, boosted amber, else the accent.
fn level_colors(event: &OsdEvent, level: f32, palette: &MandaraPalette) -> (Color, Color, bool) {
    let boosted = level > BOOST_THRESHOLD;
    let low_battery = event.kind == OsdKind::Battery && event.muted;
    if event.muted && !low_battery {
        (palette.on_surface_variant.with_alpha(MUTED_BAR_ALPHAS.0), palette.on_surface_variant.with_alpha(MUTED_BAR_ALPHAS.1), boosted)
    } else if boosted || low_battery {
        (palette.primary, palette.warning_amber, true)
    } else {
        (palette.primary, partner(palette.primary).with_lightness(palette.primary.lightness()), false)
    }
}

fn pill_fill(palette: &MandaraPalette, opacity: f32) -> (Color, Color) {
    let fill = crate::shell::frame_color(palette);
    let fill = fill.with_alpha(fill.alpha() * opacity);
    (fill.lighten(if palette.is_dark { SHEEN_TOP_DARK } else { SHEEN_TOP_LIGHT }), fill.lighten(SHEEN_BOTTOM))
}

fn pill(event: &OsdEvent, level: f32, presence: f32, palette: &MandaraPalette, look: Look) -> impl IntoElement {
    let s = look.scale;
    let dim = event.muted;
    let value = event.value();
    let sz = |v: f32| px(v * s);
    let body = match event.body() {
        Body::Label => div()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .gap(sz(TITLE_GAP_PX))
            .child(div().text_size(sz(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(event.title()))
            .child(div().text_size(sz(text::TITLE)).font_weight(FontWeight::SEMIBOLD).truncate().child(if event.label.is_empty() { t("Unknown").to_string() } else { event.label.clone() })),
        Body::Toggle => div()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .justify_between()
            .gap(sz(BODY_GAP_PX))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(sz(TITLE_GAP_PX))
                    .child(div().text_size(sz(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().child(event.title()))
                    .when(!event.label.is_empty(), |el| el.child(div().text_size(sz(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(event.label.clone()))),
            )
            .child(state_badge(value, !dim, palette, s)),
        Body::Level => {
            let (from, to, warn) = level_colors(event, level, palette);
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(sz(BODY_GAP_PX))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().text_size(sz(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(palette.on_surface_variant.hsla()).truncate().child(event.title()))
                        .when(look.show_value, |el| {
                            el.child(
                                div()
                                    .min_w(sz(VALUE_MIN_W_PX))
                                    .flex()
                                    .justify_end()
                                    .text_size(sz(text::BODY))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .font_features(tabular_nums())
                                    .text_color(if warn && !dim { readable(palette.warning_amber, palette).hsla() } else { palette.on_surface.hsla() })
                                    .child(value),
                            )
                        }),
                )
                .child(level_bar(level.clamp(0.0, 1.0), from, to, palette, s, false))
        }
    };
    let (top, bottom) = pill_fill(palette, look.opacity);
    div()
        .w(sz(PILL_W))
        .h(sz(PILL_H))
        .rounded_full()
        .bg(oklab_gradient(TOP_TO_BOTTOM_DEG, top, bottom))
        .border_1()
        .border_color(surface_edge(palette, look.rim).hsla())
        .shadow(elevation(palette, presence.clamp(0.0, 1.0)))
        .pl(sz(PILL_PAD_START_PX))
        .pr(sz(PILL_PAD_END_PX))
        .flex()
        .items_center()
        .gap(sz(PILL_GAP_PX))
        .child(chip(event, palette, s))
        .child(body)
}

/// Left / right: chip on top, a vertical level bar filling upwards, the
/// value at the bottom.
fn vertical_pill(event: &OsdEvent, level: f32, presence: f32, palette: &MandaraPalette, look: Look) -> impl IntoElement {
    let s = look.scale;
    let (from, to, warn) = level_colors(event, level, palette);
    let (top, bottom) = pill_fill(palette, look.opacity);
    div()
        .w(px(VPILL_W * s))
        .h(px(VPILL_H * s))
        .rounded_full()
        .bg(oklab_gradient(TOP_TO_BOTTOM_DEG, top, bottom))
        .border_1()
        .border_color(surface_edge(palette, look.rim).hsla())
        .shadow(elevation(palette, presence.clamp(0.0, 1.0)))
        .py(px(VPILL_PAD_PX * s))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(VPILL_GAP_PX * s))
        .child(chip(event, palette, s))
        .child(div().flex_1().min_h(px(0.0)).child(level_bar(level.clamp(0.0, 1.0), from, to, palette, s, true)))
        .when(look.show_value, |el| {
            el.child(
                div()
                    .text_size(px(text::LABEL * s))
                    .font_weight(FontWeight::SEMIBOLD)
                    .font_features(tabular_nums())
                    .text_color(if warn && !event.muted { readable(palette.warning_amber, palette).hsla() } else { palette.on_surface.hsla() })
                    .child(format!("{}", (level * PERCENT).round() as i32)),
            )
        })
}

/// A slim rounded track with an OKLab gradient fill and a soft glowing tip.
fn level_bar(v: f32, from: Color, to: Color, palette: &MandaraPalette, s: f32, vertical: bool) -> impl IntoElement {
    let h = LEVEL_BAR_H_PX * s;
    let v = v.clamp(0.0, 1.0);
    let track = palette.on_surface.with_alpha(if palette.is_dark { TRACK_ALPHA_DARK } else { TRACK_ALPHA_LIGHT }).hsla();
    let fill = div()
        .rounded_full()
        .min_w(px(if v > 0.0 { h } else { 0.0 }))
        .when(v > TIP_GLOW_MIN_LEVEL, |el| el.shadow(vec![crate::plugins::panel_fx::glow(to.with_alpha(TIP_GLOW_ALPHA), TIP_GLOW_BLUR_PX * s, 0.0, 0.0)]));
    if vertical {
        div()
            .w(px(h))
            .h_full()
            .rounded_full()
            .bg(track)
            .flex()
            .flex_col()
            .justify_end()
            .child(fill.w_full().h(gpui::relative(v)).min_h(px(if v > 0.0 { h } else { 0.0 })).bg(oklab_gradient(BOTTOM_TO_TOP_DEG, from, to)))
    } else {
        div().w_full().h(px(h)).rounded_full().bg(track).child(fill.h_full().w(gpui::relative(v)).bg(oklab_gradient(LEFT_TO_RIGHT_DEG, from, to)))
    }
}

fn state_badge(label: String, on: bool, palette: &MandaraPalette, s: f32) -> impl IntoElement {
    let accent = readable(palette.primary, palette);
    let tint = if on { accent } else { palette.on_surface_variant };
    div()
        .h(px(BADGE_H_PX * s))
        .px(px(BADGE_PAD_PX * s))
        .flex_none()
        .rounded_full()
        .bg(tint.with_alpha(if palette.is_dark { BADGE_ALPHA_DARK } else { BADGE_ALPHA_LIGHT }).hsla())
        .flex()
        .items_center()
        .text_size(px((text::LABEL + BADGE_TEXT_BUMP_PX) * s))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tint.hsla())
        .child(label)
}

// ── Commands ─────────────────────────────────────────────────────────────

fn event_for(kind: OsdKind, cx: &App) -> Result<OsdEvent, String> {
    let hub = crate::hub::hub(cx);
    let h = hub.read(cx);
    let leds = || mandara_services::leds::current();
    Ok(match kind {
        OsdKind::Volume if h.audio.available => OsdEvent::volume(&h.audio),
        OsdKind::Mic if h.audio.available => OsdEvent::mic(&h.audio),
        OsdKind::OutputDevice if h.audio.available => OsdEvent::output_device(&h.audio.output.name),
        OsdKind::Volume | OsdKind::Mic | OsdKind::OutputDevice => return Err(t("No audio server").into()),
        OsdKind::Brightness => OsdEvent::brightness(backlight::primary().ok_or_else(|| t("This machine has no backlight"))?.level()),
        OsdKind::Layout => OsdEvent::layout(if h.keyboard_layout.is_empty() { t("Unknown") } else { &h.keyboard_layout }),
        OsdKind::CapsLock => OsdEvent::caps(leds().caps),
        OsdKind::NumLock => OsdEvent::num_lock(leds().num),
        OsdKind::ScrollLock => OsdEvent::scroll_lock(leds().scroll),
        OsdKind::KeyboardBacklight => OsdEvent::keyboard_backlight(leds().kbd_backlight.ok_or_else(|| t("This keyboard has no backlight"))?),
        OsdKind::Media => match &h.media {
            Some(m) => OsdEvent::media(&m.title, &m.artist),
            None => return Err(t("Nothing is playing").into()),
        },
        OsdKind::DoNotDisturb => OsdEvent::dnd(h.notifications.quiet()),
        OsdKind::Privacy => {
            let st = crate::alerts::privacy_state(cx);
            OsdEvent::privacy("shield", t("Privacy").to_string(), st.any(), &crate::alerts::privacy_summary(&st))
        }
        OsdKind::NightLight => {
            let on = crate::night_light::active(cx);
            OsdEvent::night_light(on, on.then(|| crate::state::config(cx).read(cx).get().night_light.night_temperature))
        }
        OsdKind::PowerProfile | OsdKind::Battery | OsdKind::Custom => return Err(tf("Use `osd show {kind} …` for this one", &[("kind", &kind.id())])),
    })
}

/// The primary backlight's level (`0..=1`, as `f32` bits) as last read or
/// set; `u32::MAX` while unknown or on a machine without one.
static BACKLIGHT_LEVEL: AtomicU32 = AtomicU32::new(u32::MAX);

fn remember_backlight(level: f32) {
    BACKLIGHT_LEVEL.store(level.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

/// The screen brightness (`0..=1`) for sliders: the laptop backlight, else
/// the external monitors' DDC/CI level. `None` when neither can be changed.
pub fn brightness_level(cx: &App) -> Option<f32> {
    match BACKLIGHT_LEVEL.load(Ordering::Relaxed) {
        u32::MAX => crate::plugins::displays::ddc_level(cx),
        bits => Some(f32::from_bits(bits)),
    }
}

/// Sets the brightness from a slider (`0..=1`); the slider sends its next
/// value once logind has applied this one ([`crate::slider_model`]).
pub fn set_brightness(level: f32, cx: &mut App) -> crate::slider_model::Write {
    match set_level(level, cx) {
        Ok(Output::Pending(rx)) => Some(rx),
        Ok(Output::Value(_)) => None,
        Err(e) => {
            tracing::warn!("brightness: {e}");
            None
        }
    }
}

/// Sets the primary backlight to `level` (`0..=1`) via logind and shows the
/// OSD right away.
fn set_level(level: f32, cx: &mut App) -> Result<Output, String> {
    // Floor from Studio → Displays; external monitors follow over DDC/CI
    // when asked to (and always on desktops without a backlight).
    let level = level.clamp(crate::plugins::displays::min_level(cx), 1.0);
    let primary = backlight::primary();
    let external = crate::plugins::displays::sync_external(level, primary.is_some(), cx);
    let Some(b) = primary else {
        show(OsdEvent::brightness(level), cx);
        return external.ok_or_else(|| t("This machine has no backlight").to_string());
    };
    let raw = b.raw_for(level);
    remember_backlight(raw as f32 / b.max.max(1) as f32);
    show(OsdEvent::brightness(raw as f32 / b.max.max(1) as f32), cx);
    Ok(Output::Pending(backlight::set_brightness(&b.name, raw)))
}

/// Kinds `osd <what>` reads live.
const LIVE_KINDS: &[&str] = &["volume", "mic", "brightness", "layout", "capslock", "numlock", "scrolllock", "kbd-backlight", "media", "dnd", "privacy", "output-device"];

pub fn register(r: &mut PluginRegistry) {
    let owner = "osd";
    r.command(
        owner,
        CommandSpec::new("osd", tk("Show an on-screen display with the current value"), |a, cx| {
            let what = a.str(0)?;
            let kind = OsdKind::parse(what).ok_or_else(|| tf("Unknown OSD \"{name}\"", &[("name", &what)]))?;
            let event = event_for(kind, cx)?;
            let shown = show(event.clone(), cx);
            Ok(json!({ "shown": shown, "value": event.level, "muted": event.muted, "label": event.label, "enabled": enabled(cx) }))
        })
        .arg(ArgSpec::required("what", ArgKind::Enum, tk("which display")).choices(LIVE_KINDS))
        .example("osd volume"),
    );
    r.command(
        owner,
        CommandSpec::new("osd show", tk("Show a custom on-screen display (scripts, plugins)"), |a, cx| {
            let title = a.str(0)?.to_string();
            let mut payload = OsdPayload::default().with_title(title);
            // Remaining words: `--icon <name>`, `--level <0-100>`, `--on`, `--off`, text.
            let mut words: Vec<String> = Vec::new();
            let mut i = 1;
            while let Some(w) = a.get(i) {
                match w {
                    "--icon" => {
                        payload.icon = a.get(i + 1).map(str::to_string);
                        i += 1;
                    }
                    "--level" => {
                        let v = a.get(i + 1).and_then(|v| v.parse::<f32>().ok()).ok_or_else(|| t("--level takes a number from 0 to 100").to_string())?;
                        payload.level = Some((v / PERCENT).clamp(0.0, MAX_LEVEL));
                        i += 1;
                    }
                    "--off" => payload.off = true,
                    "--on" => payload.off = false,
                    other => words.push(other.to_string()),
                }
                i += 1;
            }
            payload.label = words.join(" ");
            let shown = show(OsdEvent::new(OsdKind::Custom, payload), cx);
            Ok(json!({ "shown": shown }))
        })
        .arg(ArgSpec::required("title", ArgKind::String, tk("title")))
        .arg(ArgSpec::optional("text", ArgKind::String, tk("text, plus --icon <name> --level <0-100> --on / --off")))
        .example("osd show Backup --icon check done"),
    );
    r.command(
        owner,
        CommandSpec::new("osd kinds", tk("Every on-screen display kind and whether it is on"), |_, cx| {
            let cfg = config(cx).read(cx).snapshot();
            Ok(json!(OsdKind::ALL.iter().map(|k| json!({ "kind": k.id(), "enabled": cfg.general.osd && k.enabled_in(&cfg) })).collect::<Vec<_>>()))
        }),
    );
    r.command(
        owner,
        CommandSpec::new("brightness status", tk("Backlight devices and levels"), |_, _| {
            let devices = backlight::devices();
            let primary = backlight::primary().map(|b| b.name);
            Ok(json!({ "primary": primary, "devices": devices }))
        }),
    );
    for (path, summary, sign) in [("brightness up", tk("Raise the screen brightness (percent step, default 5)"), 1.0_f32), ("brightness down", tk("Lower the screen brightness (percent step, default 5)"), -1.0)] {
        r.command(
            owner,
            CommandSpec::new_output(path, summary, move |a, cx| {
                let step = a.get(0).map(|s| s.parse::<f32>().map_err(|_| t("Step must be a number").to_string())).transpose()?.unwrap_or(DEFAULT_STEP_PERCENT);
                let current = backlight::primary().map(|b| b.level()).or_else(|| crate::plugins::displays::ddc_level(cx)).ok_or_else(|| t("This machine has no backlight").to_string())?;
                set_level(current + sign * step / PERCENT, cx)
            })
            .arg(ArgSpec::optional("step", ArgKind::Number, tk("percent"))),
        );
    }
    r.command(
        owner,
        CommandSpec::new_output("brightness set", tk("Set the screen brightness in percent"), |a, cx| set_level(a.number(0)? as f32 / PERCENT, cx))
            .arg(ArgSpec::required("percent", ArgKind::Number, "1-100"))
            .example("brightness set 60"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_services::audio::Channel;

    fn audio(volume: f32, muted: bool, mic_muted: bool) -> AudioState {
        AudioState {
            available: true,
            output: Channel { volume, muted, name: "sink".into(), ..Default::default() },
            input: Channel { volume: 0.8, muted: mic_muted, name: "source".into(), ..Default::default() },
            ..Default::default()
        }
    }

    #[test]
    fn detects_volume_mute_and_device_changes() {
        let cfg = mandara_config::alerts::AudioConfig::default();
        let audio_change_d = |a: &AudioState, b: &AudioState| audio_change(a, b, &cfg);
        let a = audio(0.5, false, false);
        assert!(audio_change(&a, &a, &cfg).is_none());
        assert_eq!(audio_change_d(&a, &audio(0.55, false, false)).map(|e| e.kind), Some(OsdKind::Volume));
        assert_eq!(audio_change_d(&a, &audio(0.5, true, false)).map(|e| e.kind), Some(OsdKind::Volume));
        assert_eq!(audio_change_d(&a, &audio(0.5, false, true)).map(|e| e.kind), Some(OsdKind::Mic));
        // Switching devices is a device OSD, not a volume one.
        let mut other = audio(0.9, false, false);
        other.output.name = "headphones".into();
        let e = audio_change(&a, &other, &cfg).unwrap();
        assert_eq!((e.kind, e.label.as_str()), (OsdKind::OutputDevice, "headphones"));
        // Global mute is one toggle OSD, not a volume "Muted".
        let all = AudioState { all_muted: true, ..audio(0.5, true, false) };
        let e = audio_change(&a, &all, &cfg).unwrap();
        assert_eq!((e.body(), e.icon(), e.value()), (Body::Toggle, "volume-off", t("Muted").to_string()));
        assert_eq!(audio_change(&all, &a, &cfg).map(|e| (e.icon(), e.value())), Some(("volume-high", t("On").to_string())));
    }

    /// Studio → Audio renames reach the device popup; two devices with
    /// the same description still count as a switch.
    #[test]
    fn device_popup_uses_renamed_labels_and_server_keys() {
        let mut cfg = mandara_config::alerts::AudioConfig::default();
        cfg.device_labels.insert("bluez_output.AA".into(), "My buds".into());
        let on = |device: &str| AudioState { output: Channel { volume: 0.5, name: "Headset".into(), device: device.into(), ..Default::default() }, ..audio(0.5, false, false) };
        let e = audio_change(&on("alsa_output.usb"), &on("bluez_output.AA"), &cfg).unwrap();
        assert_eq!((e.kind, e.label.as_str()), (OsdKind::OutputDevice, "My buds"));
        let back = audio_change(&on("bluez_output.AA"), &on("alsa_output.usb"), &cfg).unwrap();
        assert_eq!((back.kind, back.label.as_str()), (OsdKind::OutputDevice, "Headset"), "no label: the description");
        assert!(audio_change(&on("alsa_output.usb"), &on("alsa_output.usb"), &cfg).is_none());
    }

    #[test]
    fn readouts_icons_and_bodies() {
        let v = OsdEvent::new(OsdKind::Volume, OsdPayload::level(0.64));
        assert_eq!(v.value(), "64%");
        assert_eq!(v.icon(), "volume-high");
        let m = OsdEvent { muted: true, ..v.clone() };
        assert_eq!(m.icon(), "volume-mute");
        assert_eq!(OsdEvent::caps(true).icon(), "lock");
        assert_eq!(OsdEvent::num_lock(false).value(), t("Off"));
        assert_eq!(OsdEvent::dnd(true).body(), Body::Toggle);
        assert_eq!(OsdEvent::power_profile("power-saver").body(), Body::Label);
        assert_eq!(OsdEvent::keyboard_backlight(0.5).body(), Body::Level);
        // Unknown custom icons fall back; known ones are used.
        let c = OsdEvent::new(OsdKind::Custom, OsdPayload::text("x").with_icon("no-such-icon"));
        assert_eq!(c.icon(), CUSTOM_ICON);
        assert_eq!(OsdEvent::new(OsdKind::Custom, OsdPayload::default().with_icon("check")).icon(), "check");
        for k in OsdKind::ALL {
            assert_eq!(OsdKind::parse(k.id()), Some(k));
        }
        assert_eq!(OsdKind::parse("caps"), Some(OsdKind::CapsLock));
        assert_eq!(OsdKind::parse("nope"), None);
        assert!(LIVE_KINDS.iter().all(|k| OsdKind::parse(k).is_some()));
    }

    #[test]
    fn kinds_follow_their_switches() {
        let mut cfg = ShellConfig::default();
        assert!(OsdKind::Volume.enabled_in(&cfg) && !OsdKind::Media.enabled_in(&cfg));
        cfg.osd.kinds.lock_keys = false;
        assert!(!OsdKind::NumLock.enabled_in(&cfg) && !OsdKind::CapsLock.enabled_in(&cfg));
    }

    #[test]
    fn geometry_fits_the_pill() {
        let mut cfg = ShellConfig::default();
        let g = Geometry::of(&cfg);
        assert!(g.surface.0 > PILL_W && g.surface.1 > PILL_H);
        assert!(EDGE_GAP >= MARGIN_Y);
        cfg.general.osd_position = OsdPosition::Right;
        cfg.osd.scale = 1.5;
        let v = Geometry::of(&cfg);
        assert!(v.surface.1 >= VPILL_H * 1.5 && v.surface.0 >= PILL_W * 1.5);
        cfg.osd.scale = 99.0;
        assert_eq!(Geometry::of(&cfg).scale, SCALE_RANGE.1);
    }

    #[test]
    fn every_kind_switch_gates_its_osd() {
        // Studio → On-screen displays: each `[osd.kinds]` switch silences
        // exactly its kind (lock keys share one; battery alerts can't be).
        let switches: [(OsdKind, fn(&mut ShellConfig, bool)); 12] = [
            (OsdKind::Volume, |c, on| c.osd.kinds.volume = on),
            (OsdKind::Mic, |c, on| c.osd.kinds.microphone = on),
            (OsdKind::Brightness, |c, on| c.osd.kinds.brightness = on),
            (OsdKind::Layout, |c, on| c.osd.kinds.keyboard_layout = on),
            (OsdKind::CapsLock, |c, on| c.osd.kinds.lock_keys = on),
            (OsdKind::KeyboardBacklight, |c, on| c.osd.kinds.keyboard_backlight = on),
            (OsdKind::Media, |c, on| c.osd.kinds.media = on),
            (OsdKind::PowerProfile, |c, on| c.osd.kinds.power_profile = on),
            (OsdKind::DoNotDisturb, |c, on| c.osd.kinds.do_not_disturb = on),
            (OsdKind::NightLight, |c, on| c.osd.kinds.night_light = on),
            (OsdKind::Privacy, |c, on| c.osd.kinds.privacy = on),
            (OsdKind::OutputDevice, |c, on| c.osd.kinds.output_device = on),
        ];
        for (kind, set) in switches {
            let mut cfg = ShellConfig::default();
            set(&mut cfg, true);
            assert!(kind.enabled_in(&cfg), "{kind:?} on");
            set(&mut cfg, false);
            assert!(!kind.enabled_in(&cfg), "{kind:?} off");
        }
        assert!(OsdKind::Battery.enabled_in(&ShellConfig::default()));
        let on = OsdEvent::night_light(true, Some(4000));
        assert_eq!(on.kind, OsdKind::NightLight);
    }
}
