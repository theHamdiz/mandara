//! Power: power-profiles-daemon switcher and battery health.
//!
//! The bar chip is the battery (level + charging bolt, tabular percent) on
//! machines that have one, otherwise the active profile's glyph tinted with
//! its accent; right-click cycles the profile. The panel offers the three
//! profiles as cards (Performance / Balanced / Power saver, with their
//! drivers), the apps holding a profile and the "performance degraded"
//! reason; then each battery as a hero card — a dotted charge ring, state,
//! rate in watts and the time estimate, health (full vs design capacity),
//! cycle count, temperature, the charge limit (switchable when UPower
//! supports it, read-only from sysfs otherwise) and a charge-history
//! sparkline from UPower's `GetHistory`; peripheral batteries (mice,
//! keyboards, headsets) close the panel. A desktop without a battery gets a
//! calm "No battery" card instead.
//!
//! One actor is shared by every bar instance (thread-local `Weak`); it
//! follows `PropertiesChanged` from both daemons and stops when the last
//! chip or panel is dropped. History is fetched only while the panel is
//! open. Commands: `power status|profile|cycle|battery|history|charge-limit`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::hash::{Hash as _, Hasher as _};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, App, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, Window, div, prelude::FluentBuilder as _, px, size,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry};
use mandara_services::power::{self, ChargeState, DeviceKind, HistoryPoint, PowerCommand, PowerDevice, PowerService, PowerState, Profiles};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::fn_view;
use super::panel_fx::{
    ANGLE_DIAGONAL_DEG, EXPIRY_MARGIN, HEADER_H, PARTNER_HUE_DEG, PanelShape, SECTION_H_PX, SMALL_ICON_PX, ViewSet, WASH_FAINT, alpha, chip, dot_ring,
    empty_state, glass_card, glow, header, icon_chip, meter, partner, percent_text, readable, redraw_in, section, switch,
};
use crate::fonts::tabular_nums;
use crate::icons::icon;
use crate::kit::{reveal, text, tokens};

pub const ID: &str = "power";

/// Defaults for `plugins.settings.power.*`.
const DEFAULT_SHOW_PERIPHERALS: bool = true;
const DEFAULT_HISTORY_HOURS: i64 = 12;
const HISTORY_HOURS_RANGE: (i64, i64) = (1, 48);

const PANEL_W: f32 = 468.0;
const PAD: f32 = 14.0;
const GAP: f32 = tokens::MD;
const SECTION_H: f32 = SECTION_H_PX;
const SECTION_GAP: f32 = tokens::SM;
const PROFILE_H: f32 = 108.0;
const PROFILE_GAP: f32 = 8.0;
const HOLD_H: f32 = 34.0;
const NOTICE_H: f32 = 34.0;
const ERROR_H: f32 = 74.0;
const NO_BATTERY_H: f32 = 76.0;
const HERO_H: f32 = 112.0;
const STATS_H: f32 = 58.0;
const HISTORY_H: f32 = 74.0;
const LIMIT_H: f32 = 46.0;
const BATTERY_INNER_GAP: f32 = 10.0;
const PERIPHERAL_H: f32 = 46.0;
const LOADING_H: f32 = 180.0;
/// Charge ring geometry.
const RING_D: f32 = 88.0;
const RING_DOTS: usize = 40;
const RING_DOT: f32 = 3.2;
/// Sparkline columns across the history span.
const HISTORY_BUCKETS: usize = 48;
/// UPower history resolution (samples requested).
const HISTORY_RESOLUTION: u32 = 240;
/// History is re-read this often while the panel is open.
const HISTORY_REFRESH: Duration = Duration::from_secs(60);
/// The charge (percent) below which a discharging battery reads as low:
/// the battery alert's threshold (`power.low_percent`), so the chip turns
/// amber exactly when the "Battery low" warning fires.
fn low_percent(cx: &App) -> f64 {
    f64::from(crate::state::config(cx).read(cx).get().power.low_percent)
}

/// The same for mice, keyboards and headsets (`power.peripheral_low_percent`).
fn peripheral_low_percent(cx: &App) -> f64 {
    f64::from(crate::state::config(cx).read(cx).get().power.peripheral_low_percent)
}
/// Health below this is called out.
const WORN_HEALTH: f64 = 0.8;
/// A "switching…" state without a reply is dropped after this.
const SWITCH_TIMEOUT: Duration = Duration::from_secs(30);
const TOAST_H: f32 = 36.0;
/// Live readouts (time estimates) tick at this cadence while open.
const TICK: Duration = Duration::from_secs(5);
const SECS_PER_MIN: i64 = 60;
const MINS_PER_HOUR: i64 = 60;
const SECS_PER_HOUR: u64 = 3600;
/// UPower reports charge in percent.
const PERCENT_FULL: f64 = 100.0;

/// `ink_on`: backgrounds lighter than this get dark ink.
const INK_SWITCH_L: f32 = 0.64;
/// Dark ink `(lightness, chroma)` and light ink lightness (OKLCH).
const DARK_INK: (f32, f32) = (0.2, 0.02);
const LIGHT_INK_L: f32 = 0.99;

// Profile cards.
const PROFILE_PAD_PX: f32 = 12.0;
const PROFILE_GLYPH_BOX_PX: f32 = 34.0;
const PROFILE_GLYPH_RADIUS_PX: f32 = 12.0;
const PROFILE_GLYPH_PX: f32 = 18.0;
const PROFILE_CHECK_PX: f32 = 20.0;
const PROFILE_MARK_PX: f32 = 14.0;
const PROFILE_PIN_PX: f32 = 13.0;
const PROFILE_DRIVER_TEXT_PX: f32 = 10.0;
/// Subtitle ink alpha on an active card, driver line alpha on top of that.
const ACTIVE_SUB_ALPHA: f32 = 0.78;
const DRIVER_ALPHA: f32 = 0.75;
/// Glyph box / check fills on the active card.
const ACTIVE_GLYPH_FILL_ALPHA: f32 = 0.16;
const ACTIVE_CHECK_FILL_ALPHA: f32 = 0.2;
/// Active card glow: alpha `(dark, light)`, blur and dy.
const ACTIVE_GLOW: (f32, f32) = (0.32, 0.26);
const ACTIVE_GLOW_BLUR_PX: f32 = 14.0;
const ACTIVE_GLOW_DY_PX: f32 = 3.0;
/// Hover wash on inactive cards `(dark, light)`.
const CARD_HOVER: (f32, f32) = (0.05, 0.04);

// Notice / hold rows.
/// Rows are this much shorter than their slot.
const ROW_INSET_PX: f32 = 4.0;
const ROW_PAD_X_PX: f32 = 10.0;
const ROW_RADIUS_PX: f32 = 10.0;
const ROW_ICON_PX: f32 = 13.0;
const HOLD_FILL: (f32, f32) = (0.04, 0.035);
const NOTICE_FILL: (f32, f32) = (0.12, 0.09);

// Info cards.
const INFO_PAD_X_PX: f32 = 14.0;
const INFO_CHIP_PX: f32 = 38.0;
const INFO_LINE_GAP_PX: f32 = 2.0;
const STAT_LINE_GAP_PX: f32 = 3.0;

// History sparkline.
const SPARK_PAD_PX: f32 = 3.0;
const SPARK_RADIUS_PX: f32 = 8.0;
const SPARK_GAP_PX: f32 = 2.0;
const SPARK_COL_MIN_PX: f32 = 2.0;
const SPARK_COL_RADIUS_PX: f32 = 1.5;
/// Oldest columns fade by this share.
const SPARK_AGE_FADE: f32 = 0.45;
/// History bar start colour alpha.
const HISTORY_FROM_ALPHA: f32 = 0.7;
/// Gap between the "Charge history" label row and the bars.
const HISTORY_LABEL_GAP_PX: f32 = 4.0;

// Battery card.
const BATTERY_PAD_PX: f32 = 14.0;
const RING_TRACK: (f32, f32) = (0.12, 0.1);
const RING_VALUE_PX: f32 = 22.0;
const RING_VALUE_LINE_PX: f32 = 24.0;
const HEADLINE_GAP_PX: f32 = 4.0;
/// Battery state line: two points above the title size.
const STATE_TEXT_PX: f32 = text::TITLE + 2.0;
const CHIPS_GAP_PX: f32 = 6.0;
const CHIPS_TOP_PX: f32 = 2.0;
const HERO_GAP_PX: f32 = 16.0;
const HEALTH_METER_W_PX: f32 = 64.0;
const HEALTH_METER_H_PX: f32 = 4.0;
const STATS_PAD_X_PX: f32 = 12.0;
/// Stats well radius: concentric inside the card, never below the minimum.
const STATS_RADIUS_INSET_PX: f32 = 6.0;
const STATS_MIN_RADIUS_PX: f32 = 10.0;
const STATS_FILL: (f32, f32) = (0.035, 0.03);
const STATS_GAP_PX: f32 = 10.0;
const LIMIT_ICON_PX: f32 = 15.0;
const LIMIT_BUSY_OPACITY: f32 = 0.6;

// Peripheral rows.
const PERIPH_PAD_X_PX: f32 = 8.0;
const PERIPH_GLYPH_BOX_PX: f32 = 32.0;
const PERIPH_GLYPH_RADIUS_PX: f32 = 10.0;
const PERIPH_GLYPH_PX: f32 = 16.0;
const PERIPH_FILL: (f32, f32) = (0.06, 0.045);
const PERIPH_METER_W_PX: f32 = 72.0;
const PERIPH_METER_H_PX: f32 = 5.0;
const PERIPH_VALUE_W_PX: f32 = 40.0;
/// Inner padding of the peripherals card.
const PERIPH_CARD_PAD_PX: f32 = 4.0;

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

// Bar chip.
const CHIP_GAP_PX: f32 = 5.0;
const CHIP_BOLT_PX: f32 = 11.0;
const CHIP_GLYPH_PX: f32 = 15.0;
const CHIP_OUTLINE_ALPHA: f32 = 0.7;
/// Profile glyph alpha before / after the daemon answered.
const CHIP_IDLE_ALPHA: (f32, f32) = (0.7, 0.45);

/// Size changes smaller than this don't resize the panel.
const RESIZE_EPSILON_PX: f32 = 0.5;
/// Devices listed without a heap allocation per render.
const INLINE_BATTERIES: usize = 2;
const INLINE_PERIPHERALS: usize = 4;

/// A stable element id for a device path or profile (no per-frame `format!`).
fn hashed_id(prefix: &'static str, key: &str) -> ElementId {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    ElementId::NamedInteger(SharedString::new_static(prefix), h.finish())
}

// ── Shared model ─────────────────────────────────────────────────────────

struct Model {
    service: PowerService,
    state: PowerState,
    /// Profile being switched to (until the daemon reports it).
    switching: Option<(String, Instant)>,
    limit_busy: bool,
    /// (ok, message, shown at).
    status: Option<(bool, SharedString, Instant)>,
    /// Device path → (fetched at, span, samples).
    history: HashMap<String, (Instant, Duration, Vec<HistoryPoint>)>,
    history_in_flight: bool,
    /// Bumped when the panel closes and the history is freed; a fetch from
    /// an older epoch is dropped on arrival.
    history_epoch: u64,
    /// A close watch is running.
    close_watch: Rc<Cell<bool>>,
    /// The chip(s) and panel showing this model.
    views: ViewSet,
}

impl Model {
    /// Frees panel-only state (charge history samples, the toast) once the
    /// panel has closed; the chip only needs `state`.
    fn free_panel(&mut self) {
        self.history = HashMap::new();
        self.history_in_flight = false;
        self.history_epoch = self.history_epoch.wrapping_add(1);
        self.status = None;
    }
}

type Shared = Rc<RefCell<Model>>;

thread_local! {
    static SHARED: RefCell<Weak<RefCell<Model>>> = RefCell::new(Weak::new());
}

/// The shared model, starting the power actor on first use. It stops once
/// every chip and panel holding it is gone.
fn shared(cx: &mut App) -> Shared {
    if let Some(m) = SHARED.with(|s| s.borrow().upgrade()) {
        return m;
    }
    let service = power::spawn_power_service();
    let updates = service.updates.clone();
    let m = Rc::new(RefCell::new(Model {
        service,
        state: PowerState::default(),
        switching: None,
        limit_busy: false,
        status: None,
        history: HashMap::new(),
        history_in_flight: false,
        history_epoch: 0,
        close_watch: Rc::default(),
        views: ViewSet::default(),
    }));
    SHARED.with(|s| *s.borrow_mut() = Rc::downgrade(&m));
    let weak = Rc::downgrade(&m);
    cx.spawn(async move |cx| {
        while let Ok(state) = updates.recv().await {
            let Some(m) = weak.upgrade() else { break };
            let views = {
                let mut mb = m.borrow_mut();
                if mb.switching.as_ref().is_some_and(|(p, _)| state.profiles.as_ref().is_some_and(|ps| &ps.active == p)) {
                    mb.switching = None;
                }
                mb.state = state;
                mb.views.clone()
            };
            drop(m);
            views.notify_async(cx);
        }
    })
    .detach();
    m
}

fn show_peripherals(cx: &App) -> bool {
    crate::state::config(cx).read(cx).get().plugins.setting(ID, "show_peripherals", DEFAULT_SHOW_PERIPHERALS)
}

/// Hours → span, clamped to the supported range (so the cast is lossless).
fn hours_span(hours: i64) -> Duration {
    Duration::from_secs(hours.clamp(HISTORY_HOURS_RANGE.0, HISTORY_HOURS_RANGE.1).unsigned_abs() * SECS_PER_HOUR)
}

fn history_span(cx: &App) -> Duration {
    hours_span(crate::state::config(cx).read(cx).get().plugins.setting(ID, "history_hours", DEFAULT_HISTORY_HOURS))
}

fn set_profile(m: &Shared, name: String, cx: &mut App) {
    let (rx, views) = {
        let mut mb = m.borrow_mut();
        if mb.state.profiles.as_ref().is_some_and(|p| p.active == name) {
            return;
        }
        mb.switching = Some((name.clone(), Instant::now()));
        (mb.service.request(PowerCommand::SetProfile(name)), mb.views.clone())
    };
    views.notify(cx);
    let m = m.clone();
    cx.spawn(async move |cx| {
        let result = rx.recv().await.unwrap_or_else(|_| Err(t("The service did not reply").into()));
        let failed = {
            let mut mb = m.borrow_mut();
            match result {
                Err(e) => {
                    mb.switching = None;
                    mb.status = Some((false, e.into(), Instant::now()));
                    true
                }
                // Success shows once the daemon reports the new profile.
                Ok(_) => false,
            }
        };
        if failed {
            views.notify_async(cx);
        }
    })
    .detach();
}

fn set_limit(m: &Shared, device: String, enabled: bool, cx: &mut App) {
    let (rx, views) = {
        let mut mb = m.borrow_mut();
        mb.limit_busy = true;
        (mb.service.request(PowerCommand::SetChargeLimit { device, enabled }), mb.views.clone())
    };
    views.notify(cx);
    let m = m.clone();
    cx.spawn(async move |cx| {
        let result = rx.recv().await.unwrap_or_else(|_| Err(t("The service did not reply").into()));
        {
            let mut mb = m.borrow_mut();
            mb.limit_busy = false;
            mb.status = Some(match result {
                Ok(msg) => (true, msg.into(), Instant::now()),
                Err(e) => (false, e.into(), Instant::now()),
            });
        }
        views.notify_async(cx);
    })
    .detach();
}

/// Reads the charge history of every battery off-thread when it is due.
fn refresh_history(m: &Shared, span: Duration, cx: &mut App) {
    let (due, epoch): (Vec<String>, u64) = {
        let mb = m.borrow();
        if mb.history_in_flight {
            return;
        }
        let due = mb
            .state
            .batteries()
            .filter(|b| mb.history.get(&b.path).is_none_or(|(at, s, _)| at.elapsed() >= HISTORY_REFRESH || *s != span))
            .map(|b| b.path.clone())
            .collect();
        (due, mb.history_epoch)
    };
    if due.is_empty() {
        return;
    }
    m.borrow_mut().history_in_flight = true;
    let task = cx.background_executor().spawn(async move {
        due.into_iter().map(|path| {
            let points = power::history(&path, "charge", span, HISTORY_RESOLUTION).unwrap_or_default();
            (path, points)
        }).collect::<Vec<_>>()
    });
    let m = m.clone();
    cx.spawn(async move |cx| {
        let results = task.await;
        let views = {
            let mut mb = m.borrow_mut();
            if mb.history_epoch != epoch {
                // The panel closed while this fetch ran: keep it freed.
                return;
            }
            mb.history_in_flight = false;
            for (path, points) in results {
                mb.history.insert(path, (Instant::now(), span, points));
            }
            // Forget batteries that went away (bounded by present devices).
            let present: Vec<String> = mb.state.batteries().map(|b| b.path.clone()).collect();
            mb.history.retain(|path, _| present.contains(path));
            mb.views.clone()
        };
        views.notify_async(cx);
    })
    .detach();
}

// ── Formatting ───────────────────────────────────────────────────────────

/// `2 h 05 min`, `45 min`, `< 1 min`.
fn duration_text(secs: i64) -> String {
    // Round up to whole minutes.
    let mins = (secs.max(0) + SECS_PER_MIN - 1) / SECS_PER_MIN;
    let (h, m) = (mins / MINS_PER_HOUR, mins % MINS_PER_HOUR);
    if h > 0 {
        tf("{h} h {m} min", &[("h", &h), ("m", &format!("{m:02}"))])
    } else if m > 0 {
        tf("{m} min", &[("m", &m)])
    } else {
        t("< 1 min").into()
    }
}

fn state_text(d: &PowerDevice) -> String {
    match d.state {
        ChargeState::Charging => t("Charging").into(),
        ChargeState::Discharging => t("On battery").into(),
        ChargeState::FullyCharged => t("Fully charged").into(),
        ChargeState::Empty => t("Empty").into(),
        ChargeState::PendingCharge => t("Plugged in, not charging").into(),
        ChargeState::PendingDischarge => t("Waiting to discharge").into(),
        ChargeState::Unknown => t("Unknown state").into(),
    }
}

/// `Full in 1 h 20 min` / `3 h 10 min left` (empty when unknown).
fn estimate_text(d: &PowerDevice) -> Option<String> {
    let secs = d.time_left()?;
    let span = duration_text(secs);
    Some(if d.state.is_charging() { tf("Full in {time}", &[("time", &span)]) } else { tf("{time} left", &[("time", &span)]) })
}

fn profile_label(name: &str) -> String {
    match name {
        "performance" => t("Performance").into(),
        "balanced" => t("Balanced").into(),
        "power-saver" => t("Power saver").into(),
        other => other.to_string(),
    }
}

fn profile_hint(name: &str) -> String {
    match name {
        "performance" => t("Full speed, more heat").into(),
        "balanced" => t("Speed and efficiency").into(),
        "power-saver" => t("Cooler, quieter, longer").into(),
        _ => String::new(),
    }
}

fn profile_icon(name: &str) -> &'static str {
    match name {
        "performance" => "zap",
        "power-saver" => "leaf",
        _ => "gauge",
    }
}

/// Gradient pair of a profile (warm for speed, green for saving).
fn profile_accent(name: &str, palette: &MandaraPalette) -> (Color, Color) {
    match name {
        "performance" => (palette.warning_amber, palette.urgent_coral),
        "power-saver" => (palette.success_emerald, palette.success_emerald.rotate_hue(PARTNER_HUE_DEG)),
        _ => (palette.primary, partner(palette.primary)),
    }
}

fn degraded_text(reason: &str) -> String {
    match reason {
        "lap-detected" => t("On your lap — performance is limited").into(),
        "high-operating-temperature" => t("Running hot — performance is limited").into(),
        other => tf("Performance limited: {reason}", &[("reason", &other)]),
    }
}

fn kind_icon(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Mouse | DeviceKind::Touchpad | DeviceKind::Pen => "pointer",
        DeviceKind::Keyboard => "keyboard",
        DeviceKind::Headset | DeviceKind::Headphones | DeviceKind::Speakers | DeviceKind::OtherAudio => "headphones",
        DeviceKind::GamingInput | DeviceKind::Toy => "grid",
        DeviceKind::Phone | DeviceKind::Tablet | DeviceKind::Pda | DeviceKind::MediaPlayer | DeviceKind::Wearable => "monitor",
        DeviceKind::BluetoothGeneric => "bluetooth",
        _ => "battery",
    }
}

fn kind_label(kind: DeviceKind) -> String {
    match kind {
        DeviceKind::Mouse => t("Mouse").into(),
        DeviceKind::Keyboard => t("Keyboard").into(),
        DeviceKind::Touchpad => t("Touchpad").into(),
        DeviceKind::Pen => t("Pen").into(),
        DeviceKind::Headset => t("Headset").into(),
        DeviceKind::Headphones => t("Headphones").into(),
        DeviceKind::Speakers => t("Speakers").into(),
        DeviceKind::GamingInput => t("Controller").into(),
        DeviceKind::Phone => t("Phone").into(),
        DeviceKind::Tablet => t("Tablet").into(),
        DeviceKind::Ups => t("UPS").into(),
        _ => t("Device").into(),
    }
}

/// Charge colours: coral when low, emerald while charging, else the accent.
fn level_colors(d: &PowerDevice, low_percent: f64, palette: &MandaraPalette) -> (Color, Color) {
    if d.state.is_charging() || d.state == ChargeState::FullyCharged {
        (readable(palette.success_emerald, palette), readable(palette.success_emerald.rotate_hue(PARTNER_HUE_DEG), palette))
    } else if d.percentage <= low_percent {
        (readable(palette.warning_amber, palette), readable(palette.urgent_coral, palette))
    } else {
        (readable(palette.primary, palette), readable(partner(palette.primary), palette))
    }
}

/// Legible foreground on a solid `bg` (dark ink on light fills).
fn ink_on(bg: Color) -> Color {
    if bg.lightness() > INK_SWITCH_L { Color::oklch(DARK_INK.0, DARK_INK.1, bg.hue()) } else { Color::oklch(LIGHT_INK_L, 0.0, 0.0) }
}

fn now_unix() -> u32 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| u32::try_from(d.as_secs()).unwrap_or(u32::MAX)).unwrap_or(0)
}

// ── Pieces ───────────────────────────────────────────────────────────────

fn profile_card(name: &str, profiles: &Profiles, switching: Option<&str>, m: &Shared, radius: f32, palette: &MandaraPalette) -> AnyElement {
    let active = profiles.active == name;
    let pending = switching == Some(name);
    let (from, to) = profile_accent(name, palette);
    let accent = readable(from, palette);
    let driver = profiles.available.iter().find(|p| p.name == name).map(|p| p.cpu_driver.as_deref().unwrap_or(&p.driver)).unwrap_or_default();
    // Midpoint of the card gradient (halfway between both ends).
    let mid = from.mix(to, 0.5);
    let ink = if active { ink_on(mid) } else { palette.on_surface };
    let sub = if active { ink.with_alpha(ACTIVE_SUB_ALPHA) } else { palette.on_surface_variant };
    let held = profiles.holds.iter().any(|h| h.profile == name);
    let hover = palette.on_surface.with_alpha(alpha(CARD_HOVER, palette));
    let (m, target) = (m.clone(), name.to_string());
    let base = if active {
        div()
            .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, from, to))
            .shadow(vec![glow(mid.with_alpha(alpha(ACTIVE_GLOW, palette)), ACTIVE_GLOW_BLUR_PX, 0.0, ACTIVE_GLOW_DY_PX)])
    } else {
        glass_card(palette)
    };
    base.id(hashed_id("power-profile", name))
        .relative()
        .flex_1()
        .min_w(px(0.0))
        .h(px(PROFILE_H))
        .rounded(px(radius))
        .p(px(PROFILE_PAD_PX))
        .flex()
        .flex_col()
        .justify_between()
        .cursor_pointer()
        .when(!active, |el| el.hover(move |s| s.bg(hover.hsla())))
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .child(if active {
                    div()
                        .size(px(PROFILE_GLYPH_BOX_PX))
                        .rounded(px(crate::style::radius(PROFILE_GLYPH_RADIUS_PX)))
                        .bg(ink.with_alpha(ACTIVE_GLYPH_FILL_ALPHA).hsla())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(profile_icon(name), PROFILE_GLYPH_PX, ink))
                        .into_any_element()
                } else {
                    icon_chip(profile_icon(name), accent, PROFILE_GLYPH_BOX_PX).into_any_element()
                })
                .when(active, |el| {
                    el.child(
                        div()
                            .size(px(PROFILE_CHECK_PX))
                            .rounded_full()
                            .bg(ink.with_alpha(ACTIVE_CHECK_FILL_ALPHA).hsla())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon("check", SMALL_ICON_PX, ink)),
                    )
                })
                .when(pending, |el| el.child(icon("timer", PROFILE_MARK_PX, accent)))
                .when(held && !active && !pending, |el| el.child(icon("pin", PROFILE_PIN_PX, palette.on_surface_variant))),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).text_color(ink.hsla()).truncate().child(profile_label(name)))
                .child(
                    div()
                        .text_size(px(text::LABEL))
                        .text_color(sub.hsla())
                        .truncate()
                        .child(if pending { t("Switching…").to_string() } else { profile_hint(name) }),
                )
                .when(!driver.is_empty(), |el| {
                    el.child(
                        div()
                            .text_size(px(PROFILE_DRIVER_TEXT_PX))
                            .font_features(tabular_nums())
                            .text_color(sub.with_alpha(DRIVER_ALPHA).hsla())
                            .truncate()
                            .child(SharedString::from(driver.to_string())),
                    )
                }),
        )
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            set_profile(&m, target.clone(), cx);
        })
        .into_any_element()
}

fn hold_row(app: &str, reason: &str, profile: &str, palette: &MandaraPalette) -> AnyElement {
    let (from, _) = profile_accent(profile, palette);
    let tint = readable(from, palette);
    div()
        .h(px(HOLD_H - ROW_INSET_PX))
        .px(px(ROW_PAD_X_PX))
        .rounded(px(crate::style::radius(ROW_RADIUS_PX)))
        .bg(palette.on_surface.with_alpha(alpha(HOLD_FILL, palette)).hsla())
        .flex()
        .items_center()
        .gap(px(tokens::SM))
        .child(icon("pin", SMALL_ICON_PX, tint))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(text::BODY_S))
                .child(if reason.is_empty() { tf("{app} holds {profile}", &[("app", &app), ("profile", &profile_label(profile))]) } else { tf("{app} holds {profile} · {reason}", &[("app", &app), ("profile", &profile_label(profile)), ("reason", &reason)]) }),
        )
        .into_any_element()
}

fn notice(icon_name: &'static str, msg: String, tint: Color, palette: &MandaraPalette) -> AnyElement {
    let tint = readable(tint, palette);
    div()
        .h(px(NOTICE_H - ROW_INSET_PX))
        .px(px(ROW_PAD_X_PX))
        .rounded(px(crate::style::radius(ROW_RADIUS_PX)))
        .bg(tint.with_alpha(alpha(NOTICE_FILL, palette)).hsla())
        .flex()
        .items_center()
        .gap(px(tokens::SM))
        .child(icon(icon_name, ROW_ICON_PX, tint))
        .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(tint.hsla()).child(msg))
        .into_any_element()
}

/// A compact explanatory card (missing daemon, no battery).
fn info_card(icon_name: &'static str, title: &'static str, hint: impl Into<SharedString>, height: f32, tint: Color, radius: f32, palette: &MandaraPalette) -> AnyElement {
    glass_card(palette)
        .rounded(px(radius))
        .h(px(height))
        .px(px(INFO_PAD_X_PX))
        .flex()
        .items_center()
        .gap(px(tokens::MD))
        .child(icon_chip(icon_name, readable(tint, palette), INFO_CHIP_PX))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(INFO_LINE_GAP_PX))
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(hint.into())),
        )
        .into_any_element()
}

fn stat(label: &'static str, value: impl Into<SharedString>, extra: Option<AnyElement>, palette: &MandaraPalette) -> AnyElement {
    div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(px(STAT_LINE_GAP_PX))
        .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).truncate().child(label))
        .child(div().font_features(tabular_nums()).text_size(px(text::TITLE)).font_weight(FontWeight::MEDIUM).truncate().child(value.into()))
        .when_some(extra, |el, e| el.child(e))
        .into_any_element()
}

/// Columns for the bucketed history; empty slots stay empty (never fake data).
fn history_bars(buckets: &[Option<f32>], height: f32, (from, to): (Color, Color), palette: &MandaraPalette) -> AnyElement {
    let n = buckets.len().max(1);
    let well = palette.on_surface.with_alpha(alpha(WASH_FAINT, palette));
    let inner = height - SPARK_PAD_PX * 2.0;
    div()
        .h(px(height))
        .w_full()
        .p(px(SPARK_PAD_PX))
        .rounded(px(crate::style::radius(SPARK_RADIUS_PX)))
        .bg(well.hsla())
        .flex()
        .items_end()
        .gap(px(SPARK_GAP_PX))
        .children(buckets.iter().enumerate().map(move |(i, b)| {
            let col = div().flex_1().min_w(px(1.0)).h_full().flex().flex_col().justify_end();
            match b {
                Some(v) => {
                    let age = 1.0 - i as f32 / n as f32;
                    col.child(
                        div()
                            .w_full()
                            .h(px((v * inner).max(SPARK_COL_MIN_PX)))
                            .rounded(px(crate::style::radius(SPARK_COL_RADIUS_PX)))
                            .bg(from.mix(to, *v).with_alpha(1.0 - age * SPARK_AGE_FADE).hsla()),
                    )
                }
                None => col,
            }
        }))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn battery_card(d: &PowerDevice, history: Option<&(Instant, Duration, Vec<HistoryPoint>)>, span: Duration, m: &Shared, limit_busy: bool, radius: f32, low_percent: f64, palette: &MandaraPalette) -> (AnyElement, f32) {
    let (from, to) = level_colors(d, low_percent, palette);
    let track = palette.on_surface.with_alpha(alpha(RING_TRACK, palette));
    let frac = level_fraction(d.percentage);
    let ring_inner = div()
        .flex()
        .flex_col()
        .items_center()
        .child(div().font_features(tabular_nums()).text_size(px(RING_VALUE_PX)).line_height(px(RING_VALUE_LINE_PX)).font_weight(FontWeight::MEDIUM).child(percent_text(frac)))
        .when(d.state.is_charging(), |el| el.child(icon("zap", SMALL_ICON_PX, from)))
        .into_any_element();
    let ring = dot_ring(frac, RING_D, RING_DOTS, RING_DOT, from, to, track, ring_inner);
    let rate = (d.energy_rate > 0.0).then(|| format!("{:.1} W", d.energy_rate));
    let headline = div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(px(HEADLINE_GAP_PX))
        .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).truncate().child(d.display_name()))
        .child(div().text_size(px(STATE_TEXT_PX)).font_weight(FontWeight::SEMIBOLD).child(state_text(d)))
        .child(div().font_features(tabular_nums()).text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(estimate_text(d).unwrap_or_else(|| t("Estimating…").into())))
        .child(
            div()
                .flex()
                .gap(px(CHIPS_GAP_PX))
                .mt(px(CHIPS_TOP_PX))
                .when_some(rate, |el, r| el.child(chip(if d.state.is_charging() { "plug" } else { "zap" }, r, from, palette)))
                .when(!d.technology.is_empty(), |el| el.child(chip("battery", d.technology.clone(), palette.on_surface_variant, palette))),
        );
    let hero = div().h(px(HERO_H)).flex().items_center().gap(px(HERO_GAP_PX)).child(ring).child(headline);

    // Health, cycles, energy, temperature.
    let health = d.health();
    let worn = health.is_some_and(|h| h < WORN_HEALTH);
    let health_value: SharedString = health.map_or(SharedString::new_static("–"), |h| percent_text(h as f32));
    let health_tint = if worn { (palette.warning_amber, palette.urgent_coral) } else { (palette.success_emerald, palette.success_emerald.rotate_hue(PARTNER_HUE_DEG)) };
    let health_meter = health.map(|h| {
        div()
            .w(px(HEALTH_METER_W_PX))
            .child(meter(h as f32, HEALTH_METER_H_PX, readable(health_tint.0, palette), readable(health_tint.1, palette), palette))
            .into_any_element()
    });
    let energy: SharedString =
        if d.energy_full > 0.0 && d.energy_full_design > 0.0 { format!("{:.1} / {:.1} Wh", d.energy_full, d.energy_full_design).into() } else { SharedString::new_static("–") };
    let stats = div()
        .h(px(STATS_H))
        .px(px(STATS_PAD_X_PX))
        .rounded(px(crate::style::concentric(radius, STATS_RADIUS_INSET_PX).max(crate::style::radius(STATS_MIN_RADIUS_PX))))
        .bg(palette.on_surface.with_alpha(alpha(STATS_FILL, palette)).hsla())
        .flex()
        .items_center()
        .gap(px(STATS_GAP_PX))
        .child(stat(t("Health"), health_value, health_meter, palette))
        .child(stat(t("Cycles"), d.charge_cycles.map_or(SharedString::new_static("–"), |c| c.to_string().into()), None, palette))
        .child(stat(t("Capacity"), energy, None, palette))
        .when_some(d.temperature, |el, c| el.child(stat(t("Temperature"), format!("{c:.0} °C"), None, palette)));

    // Charge history.
    let hours = span.as_secs() / SECS_PER_HOUR;
    let bars_h = HISTORY_H - SECTION_H - HISTORY_LABEL_GAP_PX;
    let history_el = match history {
        Some((_, _, points)) if !points.is_empty() => {
            let buckets = power::bucket_history(points, now_unix(), span, HISTORY_BUCKETS, PERCENT_FULL);
            history_bars(&buckets, bars_h, (from.with_alpha(HISTORY_FROM_ALPHA), to), palette)
        }
        Some(_) => div()
            .h(px(bars_h))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(text::BODY_S))
            .text_color(palette.on_surface_variant.hsla())
            .child(t("UPower has no history for this span yet"))
            .into_any_element(),
        None => div()
            .h(px(bars_h))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(text::BODY_S))
            .text_color(palette.on_surface_variant.hsla())
            .child(t("Reading history…"))
            .into_any_element(),
    };
    let history_block = div()
        .h(px(HISTORY_H))
        .flex()
        .flex_col()
        .gap(px(HISTORY_LABEL_GAP_PX))
        .child(
            div()
                .h(px(SECTION_H))
                .flex()
                .items_center()
                .justify_between()
                .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(t("Charge history")))
                .child(div().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(tf("last {hours} h", &[("hours", &hours)]))),
        )
        .child(history_el);

    // Charge limit.
    let limit = d.threshold.as_ref().map(|th| {
        let end = th.end.map(|e| tf("Stops charging at {end}%", &[("end", &e)])).unwrap_or_else(|| t("Stops charging early to slow wear").into());
        let detail = match (th.start, th.supported) {
            (Some(s), true) => tf("{limit} · resumes below {start}%", &[("limit", &end), ("start", &s)]),
            (_, true) => end,
            (_, false) => tf("{limit} · set by firmware or root (read-only)", &[("limit", &end)]),
        };
        let row = div()
            .id(hashed_id("power-limit", &d.path))
            .h(px(LIMIT_H))
            .flex()
            .items_center()
            .gap(px(STATS_GAP_PX))
            .child(icon("heart", LIMIT_ICON_PX, readable(palette.success_emerald, palette)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child(t("Charge limit")))
                    .child(div().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(detail)),
            );
        if th.supported {
            let (m, path, on) = (m.clone(), d.path.clone(), th.enabled);
            row.cursor_pointer()
                .when(limit_busy, |el| el.opacity(LIMIT_BUSY_OPACITY))
                .child(switch(on, palette))
                .on_click(move |_, _, cx| {
                    if !m.borrow().limit_busy {
                        set_limit(&m, path.clone(), !on, cx);
                    }
                })
                .into_any_element()
        } else {
            row.child(chip("lock", if th.enabled { t("On") } else { t("Off") }, palette.on_surface_variant, palette)).into_any_element()
        }
    });
    let has_limit = limit.is_some();

    let mut height = BATTERY_PAD_PX * 2.0 + HERO_H + BATTERY_INNER_GAP + STATS_H + BATTERY_INNER_GAP + HISTORY_H;
    if has_limit {
        height += BATTERY_INNER_GAP / 2.0 + LIMIT_H;
    }
    let card = glass_card(palette)
        .rounded(px(radius))
        .p(px(BATTERY_PAD_PX))
        .flex()
        .flex_col()
        .gap(px(BATTERY_INNER_GAP))
        .child(hero)
        .child(stats)
        .child(history_block)
        .when_some(limit, |el, l| el.child(div().mt(px(-BATTERY_INNER_GAP / 2.0)).child(l)))
        .into_any_element();
    (card, height)
}

fn peripheral_row(d: &PowerDevice, low_percent: f64, palette: &MandaraPalette) -> AnyElement {
    let (from, to) = level_colors(d, low_percent, palette);
    let frac = level_fraction(d.percentage);
    div()
        .h(px(PERIPHERAL_H))
        .px(px(PERIPH_PAD_X_PX))
        .flex()
        .items_center()
        .gap(px(STATS_GAP_PX))
        .child(
            div()
                .size(px(PERIPH_GLYPH_BOX_PX))
                .flex_none()
                .rounded(px(crate::style::radius(PERIPH_GLYPH_RADIUS_PX)))
                .bg(palette.on_surface.with_alpha(alpha(PERIPH_FILL, palette)).hsla())
                .flex()
                .items_center()
                .justify_center()
                .child(icon(kind_icon(d.kind), PERIPH_GLYPH_PX, palette.on_surface_variant)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).truncate().child(d.display_name()))
                .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(if d.state.is_charging() { tf("{kind} · charging", &[("kind", &kind_label(d.kind))]) } else { kind_label(d.kind) })),
        )
        .child(div().w(px(PERIPH_METER_W_PX)).flex_none().child(meter(frac, PERIPH_METER_H_PX, from, to, palette)))
        .child(
            div()
                .w(px(PERIPH_VALUE_W_PX))
                .flex_none()
                .flex()
                .justify_end()
                .font_features(tabular_nums())
                .text_size(px(text::BODY_S))
                .font_weight(FontWeight::SEMIBOLD)
                .child(percent_text(frac)),
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

// ── Bar chip ─────────────────────────────────────────────────────────────

/// Critical-battery "breathing": one cycle per period, redrawn at a gentle
/// frame rate (the battery is nearly empty — no 144 Hz animation then).
const PULSE_PERIOD_S: f32 = 1.6;
const PULSE_FRAME: Duration = Duration::from_millis(50);
const PULSE_MIN_ALPHA: f32 = 0.35;

/// The chip's alpha while the charge is critical (`power.critical_pulse`).
fn critical_pulse(window: &mut Window, cx: &mut App) -> Option<f32> {
    if !crate::powerctl::battery::critical(cx) {
        return None;
    }
    super::panel_fx::redraw_in(PULSE_FRAME, window, cx);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f32()).unwrap_or(0.0);
    let wave = 0.5 + 0.5 * (std::f32::consts::TAU * t / PULSE_PERIOD_S).cos();
    Some(PULSE_MIN_ALPHA + (1.0 - PULSE_MIN_ALPHA) * wave)
}

fn chip_view(m: &Shared, palette: &MandaraPalette, window: &Window, pulse: Option<f32>, low_percent: f64) -> AnyElement {
    let mb = m.borrow();
    mb.views.track(window);
    if let Some(b) = mb.state.batteries().next() {
        let (from, _) = level_colors(b, low_percent, palette);
        let low = b.percentage <= low_percent && !b.state.is_charging();
        let from = match pulse {
            Some(alpha) => palette.urgent_coral.with_alpha(alpha),
            None => from,
        };
        let fill = if low || b.state.is_charging() || pulse.is_some() { from } else { palette.on_surface };
        let frac = level_fraction(b.percentage);
        return div()
            .flex()
            .items_center()
            .gap(px(CHIP_GAP_PX))
            .child(crate::icons::battery(frac, b.state.is_charging(), fill, palette.on_surface.with_alpha(CHIP_OUTLINE_ALPHA)))
            .when(b.state.is_charging(), |el| el.child(icon("zap", CHIP_BOLT_PX, from)))
            .child(div().font_features(tabular_nums()).text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).child(percent_text(frac)))
            .into_any_element();
    }
    match mb.state.profiles.as_ref() {
        Some(p) => {
            let name = mb.switching.as_ref().map(|(s, _)| s.as_str()).unwrap_or(&p.active);
            let tint = if name == "balanced" { palette.on_surface } else { readable(profile_accent(name, palette).0, palette) };
            icon(profile_icon(name), crate::style::bar_icon(CHIP_GLYPH_PX), tint).into_any_element()
        }
        None => {
            let (waiting, answered) = CHIP_IDLE_ALPHA;
            icon("gauge", crate::style::bar_icon(CHIP_GLYPH_PX), palette.on_surface.with_alpha(if mb.state.ready { answered } else { waiting })).into_any_element()
        }
    }
}

/// UPower percent (`0..=100`) → fraction (`0..=1`).
fn level_fraction(percentage: f64) -> f32 {
    (percentage.clamp(0.0, PERCENT_FULL) / PERCENT_FULL) as f32
}

// ── Panel ────────────────────────────────────────────────────────────────

struct View {
    p: PanelCx,
    m: Shared,
    size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>,
}

impl View {
    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let m = &self.m;
        m.borrow().views.track(window);
        let span = history_span(cx);
        let peripherals_on = show_peripherals(cx);
        let (low, peripheral_low) = (low_percent(cx), peripheral_low_percent(cx));
        if self.p.progress() > 0.0 {
            refresh_history(m, span, cx);
            redraw_in(TICK, window, cx);
            let watch = m.borrow().close_watch.clone();
            if !watch.get() {
                let freed = m.clone();
                super::processes::on_panel_close(&self.p, &watch, cx, move |_| freed.borrow_mut().free_panel());
            }
        }
        let shape = PanelShape::of(cx);
        let card_r = shape.inner(PAD);
        let status = {
            let mut mb = m.borrow_mut();
            let toast_for = crate::state::config(cx).read(cx).snapshot().panels.toast();
            if mb.status.as_ref().is_some_and(|(_, _, at)| at.elapsed() >= toast_for) {
                mb.status = None;
            }
            if mb.switching.as_ref().is_some_and(|(_, at)| at.elapsed() >= SWITCH_TIMEOUT) {
                mb.switching = None;
            }
            mb.status.clone()
        };
        if let Some((_, _, at)) = &status {
            redraw_in(crate::state::config(cx).read(cx).snapshot().panels.toast().saturating_sub(at.elapsed()) + EXPIRY_MARGIN, window, cx);
        }
        let mb = m.borrow();
        let state = &mb.state;
        let accent = readable(palette.primary, palette);
        let batteries: smallvec::SmallVec<[&PowerDevice; INLINE_BATTERIES]> = state.batteries().collect();
        let peripherals: smallvec::SmallVec<[&PowerDevice; INLINE_PERIPHERALS]> = if peripherals_on { state.peripherals().collect() } else { smallvec::SmallVec::new() };

        // ── Header ───────────────────────────────────────────────────────
        let profile_name = state.profiles.as_ref().map(|p| profile_label(&p.active));
        let subtitle = match (batteries.first(), &profile_name) {
            (Some(b), Some(p)) => tf("{pct}% · {state} · {profile}", &[("pct", &b.percentage.round()), ("state", &state_text(b)), ("profile", p)]),
            (Some(b), None) => tf("{pct}% · {state}", &[("pct", &b.percentage.round()), ("state", &state_text(b))]),
            (None, Some(p)) => tf("On mains power · {profile}", &[("profile", p)]),
            (None, None) if !state.ready => t("Reading power state…").into(),
            (None, None) => t("On mains power").into(),
        };
        let driver = state
            .profiles
            .as_ref()
            .and_then(|p| p.available.iter().find(|x| x.name == p.active))
            .map(|x| x.cpu_driver.as_deref().unwrap_or(&x.driver))
            .filter(|d| !d.is_empty());
        let head = header("gauge", accent, t("Power"), subtitle, palette, driver.map(|d| chip("cpu", d.to_string(), accent, palette).into_any_element()));

        let mut blocks: Vec<AnyElement> = Vec::new();
        let mut height = PAD * 2.0 + HEADER_H;
        let mut push = |el: AnyElement, h: f32, blocks: &mut Vec<AnyElement>| {
            blocks.push(el);
            height += GAP + h;
        };

        if !state.ready {
            push(empty_state("gauge", t("Reading power state…"), t("Asking power-profiles-daemon and UPower"), LOADING_H, palette).into_any_element(), LOADING_H, &mut blocks);
        } else {
            // ── Profiles ─────────────────────────────────────────────────
            match &state.profiles {
                Some(profiles) if !profiles.available.is_empty() => {
                    let switching = mb.switching.as_ref().map(|(s, _)| s.as_str());
                    let mut col = div().flex().flex_col().gap(px(SECTION_GAP)).child(section(tk("Power profile"), palette, None));
                    let mut h = SECTION_H + SECTION_GAP + PROFILE_H;
                    col = col.child(div().flex().gap(px(PROFILE_GAP)).children(profiles.available.iter().map(|p| profile_card(&p.name, profiles, switching, m, card_r, palette))));
                    if !profiles.degraded.is_empty() {
                        col = col.child(notice("thermometer", degraded_text(&profiles.degraded), palette.warning_amber, palette));
                        h += SECTION_GAP + NOTICE_H - ROW_INSET_PX;
                    }
                    for hold in &profiles.holds {
                        col = col.child(hold_row(&hold.application_id, &hold.reason, &hold.profile, palette));
                        h += SECTION_GAP + HOLD_H - ROW_INSET_PX;
                    }
                    push(col.into_any_element(), h, &mut blocks);
                }
                _ => {
                    let hint = if state.profiles_error.as_ref().is_some_and(|e| !e.is_empty()) {
                        t("Install and enable power-profiles-daemon to switch profiles")
                    } else {
                        t("No profiles reported by the daemon")
                    };
                    push(info_card("gauge", t("Power profiles unavailable"), hint, ERROR_H, palette.warning_amber, card_r, palette), ERROR_H, &mut blocks);
                }
            }

            // ── Batteries ────────────────────────────────────────────────
            if !state.upower {
                push(info_card("battery", t("UPower is not running"), t("Battery details need the upower service"), ERROR_H, palette.warning_amber, card_r, palette), ERROR_H, &mut blocks);
            } else if batteries.is_empty() {
                push(info_card("plug", t("No battery"), t("This computer runs on mains power"), NO_BATTERY_H, palette.primary, card_r, palette), NO_BATTERY_H, &mut blocks);
            } else {
                let mut col = div().flex().flex_col().gap(px(SECTION_GAP)).child(section(if batteries.len() > 1 { tk("Batteries") } else { tk("Battery") }, palette, None));
                let mut h = SECTION_H;
                for b in &batteries {
                    let (card, ch) = battery_card(b, mb.history.get(&b.path), span, m, mb.limit_busy, card_r, low, palette);
                    col = col.child(card);
                    h += SECTION_GAP + ch;
                }
                push(col.into_any_element(), h, &mut blocks);
            }

            // ── Peripherals ──────────────────────────────────────────────
            if !peripherals.is_empty() {
                let list = glass_card(palette).rounded(px(card_r)).p(px(PERIPH_CARD_PAD_PX)).flex().flex_col().children(peripherals.iter().map(|d| peripheral_row(d, peripheral_low, palette)));
                let h = SECTION_H + SECTION_GAP + PERIPH_CARD_PAD_PX * 2.0 + PERIPHERAL_H * peripherals.len() as f32;
                push(div().flex().flex_col().gap(px(SECTION_GAP)).child(section(tk("Devices"), palette, None)).child(list).into_any_element(), h, &mut blocks);
            }
        }
        drop((batteries, peripherals));
        drop(mb);
        if (f32::from(self.size_cell.get().height) - height).abs() > RESIZE_EPSILON_PX {
            self.size_cell.set(size(px(PANEL_W), px(height)));
        }

        let p = &self.p;
        let mut root = div().relative().size_full().p(px(PAD)).flex().flex_col().child(reveal(p, 0, head));
        for (i, b) in blocks.into_iter().enumerate() {
            root = root.child(div().h(px(GAP))).child(reveal(p, i + 1, b));
        }
        root.when_some(status, |el, (ok, msg, _)| el.child(toast(ok, msg, palette))).into_any_element()
    }
}

// ── Commands ─────────────────────────────────────────────────────────────

/// Runs blocking work on its own thread and hands the JSON back as text.
fn pending(f: impl FnOnce() -> Result<serde_json::Value, String> + Send + 'static) -> Output {
    let (tx, rx) = async_channel::bounded(1);
    let _ = std::thread::Builder::new().name("mandara-power-cmd".into()).spawn(move || {
        let _ = tx.send_blocking(f().map(|v| serde_json::to_string_pretty(&v).unwrap_or_default()));
    });
    Output::Pending(rx)
}

fn battery_json(d: &PowerDevice) -> serde_json::Value {
    json!({
        "name": d.display_name(),
        "path": d.path,
        "percentage": d.percentage,
        "state": d.state,
        "rate_w": d.energy_rate,
        "seconds_left": d.time_left(),
        "health": d.health(),
        "energy_full_wh": d.energy_full,
        "energy_full_design_wh": d.energy_full_design,
        "cycles": d.charge_cycles,
        "temperature_c": d.temperature,
        "technology": d.technology,
        "charge_limit": d.threshold,
    })
}

fn switch_profile(target: Option<String>) -> Result<serde_json::Value, String> {
    let state = power::snapshot();
    let profiles = state.profiles.ok_or_else(|| state.profiles_error.unwrap_or_else(|| t("power-profiles-daemon is not running").into()))?;
    let name = match target {
        Some(n) => n,
        None => profiles.next().map(str::to_string).ok_or_else(|| t("No profiles reported by the daemon").to_string())?,
    };
    if profiles.active != name {
        power::execute(power::plan_set_profile(&profiles, &name)?)?;
    }
    Ok(json!({ "previous": profiles.active, "active": name }))
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Power"), tk("Power profiles, battery health and charge history"), None), |window, cx, panel_cx| {
        let m = shared(cx);
        let bar_m = m.clone();
        let bar = fn_view(
            None,
            move |palette, window, cx| {
                let pulse = critical_pulse(window, cx);
                chip_view(&bar_m, palette, window, pulse, low_percent(cx))
            },
            window,
            cx,
        );
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(PAD * 2.0 + HEADER_H + GAP + LOADING_H))));
        let view = View { p: panel_cx.clone(), m: m.clone(), size_cell: size_cell.clone() };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        let cycle_m = m.clone();
        PluginInstance {
            bar: Some(bar),
            panel: Some(PanelSpec { view: panel, size: size_cell }),
            on_secondary_click: Some(Rc::new(move |_, cx| {
                let next = cycle_m.borrow().state.profiles.as_ref().and_then(|p| p.next().map(str::to_string));
                if let Some(next) = next {
                    set_profile(&cycle_m, next, cx);
                }
            })),
            ..PluginInstance::empty()
        }
    });
    r.command(
        ID,
        CommandSpec::new_output("power status", tk("Power profile, batteries and peripheral batteries (JSON)"), |_, _| Ok(pending(|| Ok(super::json(power::snapshot()))))),
    );
    r.command(
        ID,
        CommandSpec::new_output("power profile", tk("Show the power profile, or switch to one (polkit may ask)"), |a, _| {
            let target = a.get(0).map(str::to_string);
            Ok(pending(move || match target {
                None => {
                    let s = power::snapshot();
                    let p = s.profiles.ok_or_else(|| s.profiles_error.unwrap_or_default())?;
                    Ok(json!({ "active": p.active, "available": p.available, "holds": p.holds, "degraded": p.degraded }))
                }
                Some(name) => switch_profile(Some(name)),
            }))
        })
        .arg(ArgSpec::optional("profile", ArgKind::String, tk("profile to switch to")).choices(&["performance", "balanced", "power-saver"]))
        .example("power profile balanced"),
    );
    r.command(ID, CommandSpec::new_output("power cycle", tk("Switch to the next power profile"), |_, _| Ok(pending(|| switch_profile(None)))));
    r.command(
        ID,
        CommandSpec::new_output("power battery", tk("Battery health, cycles, rate and time estimates (JSON)"), |_, _| {
            Ok(pending(|| Ok(serde_json::Value::Array(power::snapshot().batteries().map(battery_json).collect()))))
        }),
    );
    r.command(
        ID,
        CommandSpec::new_output("power history", tk("Charge history of the first battery (JSON samples)"), |a, _| {
            let hours = match a.get(0) {
                Some(_) => a.int(0)?,
                None => DEFAULT_HISTORY_HOURS,
            }
            .clamp(HISTORY_HOURS_RANGE.0, HISTORY_HOURS_RANGE.1);
            Ok(pending(move || {
                let s = power::snapshot();
                let b = s.batteries().next().ok_or_else(|| t("No battery").to_string())?;
                let points = power::history(&b.path, "charge", hours_span(hours), HISTORY_RESOLUTION)?;
                Ok(json!({ "battery": b.display_name(), "hours": hours, "samples": points }))
            }))
        })
        .arg(ArgSpec::optional("hours", ArgKind::Integer, tk("time span in hours (1–48)")))
        .example("power history 6"),
    );
    r.command(
        ID,
        CommandSpec::new_output("power charge-limit", tk("Turn the battery charge limit on or off (UPower 1.90+, polkit may ask)"), |a, _| {
            let enabled = a.bool(0)?;
            Ok(pending(move || {
                let s = power::snapshot();
                let b = s.batteries().find(|b| b.threshold.is_some()).or_else(|| s.batteries().next()).ok_or_else(|| t("No battery").to_string())?;
                power::execute(power::plan_charge_limit(b, enabled)?)?;
                Ok(json!({ "battery": b.display_name(), "charge_limit": enabled }))
            }))
        })
        .arg(ArgSpec::required("state", ArgKind::Bool, tk("on / off")))
        .example("power charge-limit on"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration_text(30), "1 min");
        assert_eq!(duration_text(0), "< 1 min");
        assert_eq!(duration_text(45 * 60), "45 min");
        assert_eq!(duration_text(2 * 3600 + 5 * 60), "2 h 05 min");
    }

    #[test]
    fn estimates_follow_direction() {
        let mut d = PowerDevice { state: ChargeState::Discharging, time_to_empty: 3600, time_to_full: 600, ..Default::default() };
        assert_eq!(estimate_text(&d).as_deref(), Some("1 h 00 min left"));
        d.state = ChargeState::Charging;
        assert_eq!(estimate_text(&d).as_deref(), Some("Full in 10 min"));
        d.time_to_full = 0;
        assert_eq!(estimate_text(&d), None);
    }

    #[test]
    fn profiles_have_labels_and_icons() {
        assert_eq!(profile_label("power-saver"), "Power saver");
        assert_eq!(profile_icon("performance"), "zap");
        assert_eq!(profile_icon("custom"), "gauge");
        assert_eq!(profile_label("custom"), "custom");
        assert!(degraded_text("lap-detected").contains("lap"));
    }

    #[test]
    fn ink_contrasts_with_fill() {
        assert!(ink_on(Color::oklch(0.8, 0.12, 30.0)).lightness() < 0.4);
        assert!(ink_on(Color::oklch(0.5, 0.15, 30.0)).lightness() > 0.9);
    }
}
