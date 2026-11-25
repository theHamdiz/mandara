//! Power: battery, charge history and power-profiles-daemon switcher.
//!
//! The bar chip is the battery (level + charging bolt, tabular percent) on
//! machines that have one, otherwise the active profile's glyph tinted with
//! its accent; right-click cycles the profile. While the battery charges the
//! chip breathes gently (`power.charging_pulse`, never with reduced motion);
//! while it is critical it pulses coral (`power.critical_pulse`).
//!
//! The panel has two styles (`power.panel_style`). **Classic**, the
//! default, is drawn by [`classic`]: profile cards first, then a battery
//! card with a dotted ring, stats and charge-history bars; it names no CPU
//! scaling driver. **Modern**, drawn here, leads with each battery: a large charge gauge (SDF dots along
//! the arc, the state inside), the time left, health / capacity / chemistry
//! and a charge-history area graph from UPower's `GetHistory`. Below it the
//! profiles that exist form a segmented control with a sliding indicator,
//! followed by the "performance degraded" reason and the apps holding a
//! profile; then the charge limit (only when UPower can switch it) and
//! peripheral batteries (mice, keyboards, headsets). Technical readouts —
//! rate, energy, voltage, cycles, temperature, the profile and CPU scaling
//! drivers, a firmware-set charge limit — sit in a collapsed *Details*
//! fold. A desktop without a battery gets a calm "No battery" card instead.
//!
//! One actor is shared by every bar instance (thread-local `Weak`); it
//! follows `PropertiesChanged` from both daemons and stops when the last
//! chip or panel is dropped. History is fetched only while the panel is
//! open. Commands: `power status|profile|cycle|battery|history|charge-limit`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, App, Bounds, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement, Pixels, SharedString,
    StatefulInteractiveElement, Styled, Window, canvas, div, fill, point, prelude::FluentBuilder as _, px, size,
};
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry};
use mandara_services::power::{self, ChargeState, DeviceKind, HistoryPoint, PowerCommand, PowerDevice, PowerService, PowerState, Profiles};
use mandara_config::power::PowerPanelStyle;
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::fn_view;
use super::panel_fx::{
    ANGLE_DIAGONAL_DEG, ANGLE_DOWN_DEG, EXPIRY_MARGIN, HEADER_H, PARTNER_HUE_DEG, PERCENT_STEP, PanelShape, SECTION_H_PX, SMALL_ICON_PX, ViewSet, WASH_FAINT,
    WASH_SOFT, alpha, empty_state, glass_card, glow, header, icon_chip, meter, partner, percent_text, readable, redraw_in, rim, section,
    switch, wash,
};
use crate::bidi::Bidi as _;
use crate::fonts::tabular_nums;
use crate::icons::icon;
use crate::kit::{reveal, text, tokens};

mod classic;

pub const ID: &str = "power";

/// Defaults for `plugins.settings.power.*`.
const DEFAULT_SHOW_PERIPHERALS: bool = true;
const DEFAULT_HISTORY_HOURS: i64 = 12;
const HISTORY_HOURS_RANGE: (i64, i64) = (1, 48);

/// Panel width of the Modern and the Classic style (three profile cards
/// side by side need the wider one).
const MODERN_W: f32 = 420.0;
const CLASSIC_W: f32 = 468.0;
const PAD: f32 = 14.0;
const GAP: f32 = tokens::MD;
const SECTION_H: f32 = SECTION_H_PX;
const SECTION_GAP: f32 = tokens::SM;
const HOLD_H: f32 = 34.0;
const NOTICE_H: f32 = 34.0;
const ERROR_H: f32 = 74.0;
const NO_BATTERY_H: f32 = 76.0;
const PERIPHERAL_H: f32 = 46.0;
const LOADING_H: f32 = 180.0;
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

// Battery hero card.
const HERO_PAD_PX: f32 = 18.0;
/// The battery's name above its gauge (only with several batteries).
const HERO_NAME_H: f32 = 18.0;
const HERO_NAME_GAP: f32 = 6.0;
/// Charge gauge: diameter and stroke.
const GAUGE_D: f32 = 172.0;
const GAUGE_STROKE: f32 = 12.0;
/// Distance between the arc's overlapping dots, as a share of the stroke
/// (small enough that the union reads as one smooth stroke).
const GAUGE_DOT_STEP: f32 = 0.18;
/// Below this charge the arc is a single dot (no zero-length sweep).
const GAUGE_MIN_FRAC: f32 = 0.005;
/// Head marker: diameter as a share of the stroke.
const GAUGE_HEAD: f32 = 0.42;
/// Track ring alpha `(dark, light)`.
const GAUGE_TRACK: (f32, f32) = (0.09, 0.08);
/// The inner disc sits this far inside the stroke.
const GAUGE_DISC_INSET_PX: f32 = 10.0;
/// Inner disc fill and its inset accent halo: alpha `(dark, light)`, blur.
const GAUGE_DISC_FILL: (f32, f32) = (0.025, 0.03);
const GAUGE_DISC_HALO: (f32, f32) = (0.16, 0.12);
const GAUGE_DISC_HALO_BLUR_PX: f32 = 26.0;
/// Percent figure inside the gauge, its unit and their line height.
const GAUGE_VALUE_PX: f32 = 46.0;
const GAUGE_VALUE_LINE_PX: f32 = 50.0;
const GAUGE_UNIT_PX: f32 = 18.0;
/// The unit sits this far below the figure's cap height.
const GAUGE_UNIT_DROP_PX: f32 = 6.0;
const GAUGE_STATE_GAP_PX: f32 = 2.0;
/// Time-left line under the gauge.
const TIME_GAP: f32 = 12.0;
const TIME_H: f32 = 22.0;
/// Hairline between the gauge and the stats.
const DIVIDER_GAP: f32 = 14.0;
const DIVIDER_W: f32 = 1.0;
/// Stats row: height, value size and the gap between value and label.
const STATS_H: f32 = 40.0;
const STAT_VALUE_PX: f32 = 17.0;
const STAT_VALUE_LINE_PX: f32 = 22.0;
const STAT_GAP_PX: f32 = 2.0;
const STATS_COL_GAP_PX: f32 = 12.0;
/// Charge history graph: gap above, plot height, axis labels.
const GRAPH_GAP: f32 = 16.0;
const GRAPH_H: f32 = 56.0;
const AXIS_GAP: f32 = 6.0;
const AXIS_H: f32 = 14.0;
/// Samples the history is resampled to before drawing.
const GRAPH_SAMPLES: usize = 96;
/// Width of one area column (whole logical pixels keep columns seamless).
const GRAPH_COL_PX: f32 = 2.0;
/// Area fill alpha at the top of a full column, line width, the "now" dot
/// and its halo.
const GRAPH_AREA_ALPHA: f32 = 0.34;
const GRAPH_LINE_PX: f32 = 2.0;
const GRAPH_NOW_PX: f32 = 6.0;
const GRAPH_NOW_HALO_PX: f32 = 14.0;
const GRAPH_NOW_HALO_ALPHA: f32 = 0.22;
/// Faint well shown while the first history read runs.
const GRAPH_WELL_RADIUS_PX: f32 = 8.0;

// Profiles segmented control.
const SEG_H: f32 = 44.0;
const SEG_PAD: f32 = 4.0;
const SEG_RADIUS_PX: f32 = 16.0;
const SEG_ICON_PX: f32 = 14.0;
const SEG_MARK_PX: f32 = 11.0;
const SEG_INNER_GAP_PX: f32 = 6.0;
/// How much of the profile's accent pair tints the indicator.
const SEG_TINT: (f32, f32) = (0.2, 0.12);
/// Hover wash on inactive segments `(dark, light)`.
const SEG_HOVER: (f32, f32) = (0.05, 0.04);
/// The indicator slides with this spring.
const SEG_SPRING: SpringConfig = SpringConfig::SNAPPY;
/// The indicator rests once it is this close to its slot (in slots).
const SEG_SETTLE: f32 = 0.002;

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

// Charge limit card.
const LIMIT_H: f32 = 62.0;
const LIMIT_PAD_X_PX: f32 = 14.0;
const LIMIT_CHIP_PX: f32 = 34.0;
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
const PERIPH_GAP_PX: f32 = 10.0;
/// Inner padding of the peripherals card.
const PERIPH_CARD_PAD_PX: f32 = 4.0;

// Details fold.
const FOLD_H: f32 = 34.0;
const FOLD_PAD_X_PX: f32 = 12.0;
const FOLD_CHEVRON_PX: f32 = 12.0;
const FOLD_HOVER: (f32, f32) = (0.05, 0.04);
const DETAILS_PAD_PX: f32 = 12.0;
const DETAIL_ROW_H: f32 = 26.0;
const DETAIL_HEAD_H: f32 = 22.0;
const DETAIL_GROUP_GAP: f32 = 8.0;

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
    /// The *Details* fold is open (kept for the session).
    details_open: bool,
    /// The profile indicator's position (in segments) and its clock; made
    /// at the active slot when the panel first draws, freed on close.
    segment: Option<(SpringValue<f32>, FrameClock)>,
    /// The chip(s) and panel showing this model.
    views: ViewSet,
}

impl Model {
    /// Frees panel-only state (charge history samples, the toast, the
    /// indicator spring) once the panel has closed; the chip only needs
    /// `state`.
    fn free_panel(&mut self) {
        self.history = HashMap::new();
        self.history_in_flight = false;
        self.history_epoch = self.history_epoch.wrapping_add(1);
        self.status = None;
        self.segment = None;
    }
}

type Shared = Rc<RefCell<Model>>;

thread_local! {
    static SHARED: RefCell<Weak<RefCell<Model>>> = const { RefCell::new(Weak::new()) };
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
        details_open: false,
        segment: None,
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
        due.into_iter()
            .map(|path| {
                let points = power::history(&path, "charge", span, HISTORY_RESOLUTION).unwrap_or_default();
                (path, points)
            })
            .collect::<Vec<_>>()
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

/// The line under the gauge: the estimate, "Estimating…" while UPower is
/// still working it out, "On mains power" when full or held; nothing for
/// an unknown state.
fn time_line(d: &PowerDevice) -> Option<String> {
    match d.state {
        ChargeState::Charging | ChargeState::Discharging => Some(estimate_text(d).unwrap_or_else(|| t("Estimating…").into())),
        ChargeState::FullyCharged | ChargeState::PendingCharge => Some(t("On mains power").into()),
        _ => None,
    }
}

fn profile_label(name: &str) -> String {
    match name {
        "performance" => t("Performance").into(),
        "balanced" => t("Balanced").into(),
        "power-saver" => t("Power saver").into(),
        other => other.to_string(),
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

/// `"{value} Wh"` with one decimal.
fn wh(v: f64) -> String {
    format!("{v:.1} Wh")
}

/// The charge limit as one sentence: `Stops at 80%, resumes below 60%`.
fn limit_text(start: Option<u32>, end: Option<u32>) -> String {
    match (start, end) {
        (Some(s), Some(e)) => tf("Stops at {end}%, resumes below {start}%", &[("end", &e), ("start", &s)]),
        (None, Some(e)) => tf("Stops charging at {end}%", &[("end", &e)]),
        _ => t("Stops charging early to slow wear").into(),
    }
}

fn now_unix() -> u32 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| u32::try_from(d.as_secs()).unwrap_or(u32::MAX)).unwrap_or(0)
}

// ── Charge history curve ─────────────────────────────────────────────────

/// The charge over the last `span`, resampled to `samples` evenly spaced
/// points (oldest first, `0..=1`). UPower's samples are joined linearly and
/// the live `current` charge anchors the end at `now`; times before the
/// first sample stay `None` (never invented). Empty without any sample.
fn charge_curve(points: &[HistoryPoint], now: u32, span: Duration, samples: usize, current: Option<f64>) -> Vec<Option<f32>> {
    let now = f64::from(now);
    let mut anchors: Vec<(f64, f64)> = points.iter().map(|p| (f64::from(p.time), p.value)).filter(|(t, _)| *t <= now).collect();
    if anchors.is_empty() || samples == 0 {
        return Vec::new();
    }
    if let Some(c) = current {
        anchors.push((now, c));
    }
    let span_s = span.as_secs().max(1) as f64;
    let start = now - span_s;
    let last = (samples - 1).max(1) as f64;
    (0..samples)
        .map(|i| {
            let at = start + span_s * i as f64 / last;
            let after = anchors.partition_point(|(t, _)| *t <= at);
            let v = match (after.checked_sub(1).map(|i| anchors[i]), anchors.get(after).copied()) {
                (None, _) => return None,
                (Some((_, v)), None) => v,
                (Some((t0, v0)), Some((t1, v1))) => {
                    let k = if t1 > t0 { (at - t0) / (t1 - t0) } else { 1.0 };
                    v0 + (v1 - v0) * k
                }
            };
            Some((v / PERCENT_FULL).clamp(0.0, 1.0) as f32)
        })
        .collect()
}

/// `curve` read at `x` (`0..=1` across it), linearly between neighbours;
/// `None` in gaps.
fn curve_at(curve: &[Option<f32>], x: f32) -> Option<f32> {
    let n = curve.len();
    if n == 0 {
        return None;
    }
    let pos = x.clamp(0.0, 1.0) * (n - 1) as f32;
    let i = (pos.floor() as usize).min(n - 1);
    let k = pos - i as f32;
    match (curve[i], curve.get(i + 1).copied().flatten()) {
        (Some(a), Some(b)) => Some(a + (b - a) * k),
        (Some(a), None) => Some(a),
        (None, Some(b)) if k > 0.5 => Some(b),
        _ => None,
    }
}

// ── Drawing ──────────────────────────────────────────────────────────────

fn dot_bounds(x: f32, y: f32, d: f32) -> Bounds<Pixels> {
    Bounds { origin: point(px(x - d / 2.0), px(y - d / 2.0)), size: size(px(d), px(d)) }
}

/// A round dot (an SDF quad).
fn paint_dot(window: &mut Window, x: f32, y: f32, d: f32, c: Color) {
    window.paint_subpixel_quad(fill(dot_bounds(x, y, d), c.hsla()).corner_radii(px(d / 2.0)));
}

/// The charge gauge: a soft track ring (an SDF border) and the charge arc
/// from 12 o'clock clockwise, painted as overlapping round dots coloured
/// `from → to` along the arc, with a small marker at its head. `inner` is
/// centred over an inner disc with an ambient halo of the charge colour.
pub fn charge_gauge(frac: f32, (from, to): (Color, Color), palette: &MandaraPalette, inner: AnyElement) -> AnyElement {
    let frac = frac.clamp(0.0, 1.0);
    let track = palette.on_surface.with_alpha(alpha(GAUGE_TRACK, palette));
    let head_ink = palette.surface_container;
    let disc = GAUGE_D - (GAUGE_STROKE + GAUGE_DISC_INSET_PX) * 2.0;
    let halo = gpui::BoxShadow {
        color: from.with_alpha(alpha(GAUGE_DISC_HALO, palette)).hsla(),
        offset: point(px(0.0), px(0.0)),
        blur_radius: px(GAUGE_DISC_HALO_BLUR_PX),
        spread_radius: px(0.0),
        inset: true,
    };
    let arc = canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let side = f32::from(b.size.width).min(f32::from(b.size.height));
            let (cx, cy) = (f32::from(b.origin.x) + side / 2.0, f32::from(b.origin.y) + side / 2.0);
            let r = (side - GAUGE_STROKE) / 2.0;
            let at = |a: f32| (cx + r * a.cos(), cy + r * a.sin());
            let top = -std::f32::consts::FRAC_PI_2;
            if frac < GAUGE_MIN_FRAC {
                return;
            }
            let sweep = std::f32::consts::TAU * frac;
            let steps = ((sweep * r) / (GAUGE_STROKE * GAUGE_DOT_STEP)).ceil().max(1.0) as usize;
            for i in 0..=steps {
                let k = i as f32 / steps as f32;
                let (x, y) = at(top + sweep * k);
                paint_dot(window, x, y, GAUGE_STROKE, from.mix(to, k));
            }
            let (hx, hy) = at(top + sweep);
            paint_dot(window, hx, hy, GAUGE_STROKE * GAUGE_HEAD, head_ink);
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full();
    div()
        .relative()
        .size(px(GAUGE_D))
        .flex_none()
        .child(div().absolute().top_0().left_0().size_full().rounded_full().border(px(GAUGE_STROKE)).border_color(track.hsla()))
        .child(
            div()
                .absolute()
                .left(px((GAUGE_D - disc) / 2.0))
                .top(px((GAUGE_D - disc) / 2.0))
                .size(px(disc))
                .rounded_full()
                .bg(wash(GAUGE_DISC_FILL, palette).hsla())
                .shadow(vec![halo]),
        )
        .child(arc)
        .child(div().absolute().top_0().left_0().size_full().flex().flex_col().items_center().justify_center().child(inner))
        .into_any_element()
}

/// The charge history as an area graph: whole-pixel columns whose fill
/// fades toward the baseline (alpha linear in height, so neighbouring
/// columns share one gradient), a line of overlapping dots along the top
/// and a dot at "now". Time runs left to right in every language.
fn charge_graph(curve: Vec<Option<f32>>, (from, to): (Color, Color)) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let (x0, y0) = (f32::from(b.origin.x).round(), f32::from(b.origin.y));
            let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
            // The line's dots must stay inside the plot.
            let (top, plot) = (y0 + GRAPH_LINE_PX / 2.0, h - GRAPH_LINE_PX);
            let cols = (w / GRAPH_COL_PX).floor().max(1.0) as usize;
            let mut prev: Option<(f32, f32)> = None;
            let mut last: Option<(f32, f32, Color)> = None;
            for c in 0..cols {
                let x01 = (c as f32 + 0.5) / cols as f32;
                let Some(v) = curve_at(&curve, x01) else {
                    prev = None;
                    continue;
                };
                let tint = from.mix(to, x01);
                let x = x0 + c as f32 * GRAPH_COL_PX;
                let y = top + (1.0 - v) * plot;
                let column = Bounds { origin: point(px(x), px(y)), size: size(px(GRAPH_COL_PX), px(y0 + h - y)) };
                window.paint_quad(fill(column, oklab_gradient(ANGLE_DOWN_DEG, tint.with_alpha(GRAPH_AREA_ALPHA * v), tint.with_alpha(0.0))));
                let cx = x + GRAPH_COL_PX / 2.0;
                // Steep steps get extra dots so the line never breaks.
                if let Some((px0, py0)) = prev {
                    let extra = ((y - py0).abs() / (GRAPH_LINE_PX / 2.0)).ceil() as usize;
                    for i in 1..extra {
                        let k = i as f32 / extra as f32;
                        paint_dot(window, px0 + (cx - px0) * k, py0 + (y - py0) * k, GRAPH_LINE_PX, tint);
                    }
                }
                paint_dot(window, cx, y, GRAPH_LINE_PX, tint);
                prev = Some((cx, y));
                last = Some((cx, y, tint));
            }
            if let Some((x, y, c)) = last {
                paint_dot(window, x, y, GRAPH_NOW_HALO_PX, c.with_alpha(GRAPH_NOW_HALO_ALPHA));
                paint_dot(window, x, y, GRAPH_NOW_PX, c);
            }
        },
    )
    .w_full()
    .h(px(GRAPH_H))
}

// ── Pieces ───────────────────────────────────────────────────────────────

fn stat(value: impl Into<SharedString>, label: impl Into<SharedString>, tint: Option<Color>, palette: &MandaraPalette) -> AnyElement {
    div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(px(STAT_GAP_PX))
        .child(
            div()
                .font_features(tabular_nums())
                .text_size(px(STAT_VALUE_PX))
                .line_height(px(STAT_VALUE_LINE_PX))
                .font_weight(FontWeight::MEDIUM)
                .when_some(tint, |el, c| el.text_color(c.hsla()))
                .truncate()
                .child(value.into()),
        )
        .child(div().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(label.into()))
        .into_any_element()
}

/// Health, capacity against design, and chemistry (cycles when the
/// chemistry is unknown); whatever UPower doesn't report is left out.
fn battery_stats(d: &PowerDevice, palette: &MandaraPalette) -> smallvec::SmallVec<[AnyElement; 3]> {
    let mut out = smallvec::SmallVec::new();
    if let Some(h) = d.health() {
        let tint = (h < WORN_HEALTH).then(|| readable(palette.warning_amber, palette));
        out.push(stat(percent_text(h as f32), t("Health"), tint, palette));
    }
    if d.energy_full > 0.0 {
        let label: SharedString = if d.energy_full_design > 0.0 { tf("of {design} Wh", &[("design", &format!("{:.1}", d.energy_full_design))]).into() } else { t("Capacity").into() };
        out.push(stat(wh(d.energy_full), label, None, palette));
    }
    if !d.technology.is_empty() {
        out.push(stat(d.technology.clone(), t("Chemistry"), None, palette));
    } else if let Some(c) = d.charge_cycles {
        out.push(stat(c.to_string(), t("Cycles"), None, palette));
    }
    out
}

/// One battery: gauge, time left, stats and the charge-history graph.
fn battery_hero(d: &PowerDevice, named: bool, history: Option<&(Instant, Duration, Vec<HistoryPoint>)>, span: Duration, radius: f32, low_percent: f64, palette: &MandaraPalette) -> (AnyElement, f32) {
    let colors = level_colors(d, low_percent, palette);
    let frac = level_fraction(d.percentage);
    let muted = palette.on_surface_variant;
    let figure = (frac / PERCENT_STEP).round() as i32;
    let state = div()
        .row()
        .items_center()
        .gap(px(tokens::XS))
        .when(d.state.is_charging(), |el| el.child(icon("zap", SMALL_ICON_PX, colors.0)))
        .child(div().text_size(px(text::BODY_S)).text_color(muted.hsla()).child(state_text(d)));
    let inner = div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(GAUGE_STATE_GAP_PX))
        .child(
            div()
                .ltr_row()
                .items_start()
                .child(div().font_features(tabular_nums()).text_size(px(GAUGE_VALUE_PX)).line_height(px(GAUGE_VALUE_LINE_PX)).font_weight(FontWeight::LIGHT).child(figure.to_string()))
                .child(div().mt(px(GAUGE_UNIT_DROP_PX)).text_size(px(GAUGE_UNIT_PX)).font_weight(FontWeight::LIGHT).text_color(muted.hsla()).child("%")),
        )
        .child(state)
        .into_any_element();
    let gauge = div().w_full().flex().justify_center().child(charge_gauge(frac, colors, palette, inner));
    let time = div()
        .mt(px(TIME_GAP))
        .h(px(TIME_H))
        .flex()
        .items_center()
        .justify_center()
        .font_features(tabular_nums())
        .text_size(px(text::TITLE))
        .font_weight(FontWeight::MEDIUM)
        .children(time_line(d));
    let mut height = HERO_PAD_PX * 2.0 + GAUGE_D + TIME_GAP + TIME_H;
    let mut card = glass_card(palette).rounded(px(radius)).p(px(HERO_PAD_PX)).flex().flex_col();
    if named {
        card = card.child(
            div()
                .h(px(HERO_NAME_H))
                .mb(px(HERO_NAME_GAP))
                .text_size(px(text::LABEL))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(muted.hsla())
                .truncate()
                .child(d.display_name()),
        );
        height += HERO_NAME_H + HERO_NAME_GAP;
    }
    card = card.child(gauge).child(time);

    let stats = battery_stats(d, palette);
    if !stats.is_empty() {
        card = card
            .child(div().my(px(DIVIDER_GAP)).h(px(DIVIDER_W)).w_full().bg(wash(WASH_SOFT, palette).hsla()))
            .child(div().h(px(STATS_H)).row().items_center().gap(px(STATS_COL_GAP_PX)).children(stats));
        height += DIVIDER_GAP * 2.0 + DIVIDER_W + STATS_H;
    }

    // Charge history: a quiet well while the first read runs, the graph
    // once samples arrive, nothing when UPower keeps no history.
    let plot = match history {
        None => Some(div().h(px(GRAPH_H)).w_full().rounded(px(crate::style::radius(GRAPH_WELL_RADIUS_PX))).bg(wash(WASH_FAINT, palette).hsla()).into_any_element()),
        Some((_, _, points)) => {
            let curve = charge_curve(points, now_unix(), span, GRAPH_SAMPLES, Some(d.percentage));
            curve.iter().any(Option::is_some).then(|| charge_graph(curve, colors).into_any_element())
        }
    };
    if let Some(plot) = plot {
        let hours = span.as_secs() / SECS_PER_HOUR;
        let axis = div()
            .mt(px(AXIS_GAP))
            .h(px(AXIS_H))
            .ltr_row()
            .items_center()
            .justify_between()
            .font_features(tabular_nums())
            .text_size(px(text::LABEL))
            .text_color(muted.hsla())
            .child(tf("{hours} h ago", &[("hours", &hours)]))
            .child(t("Now"));
        card = card.child(div().mt(px(GRAPH_GAP)).child(plot)).child(axis);
        height += GRAPH_GAP + GRAPH_H + AXIS_GAP + AXIS_H;
    }
    (card.into_any_element(), height)
}

/// The profiles that exist as a segmented control; the indicator, drawn at
/// `slot` (in segments from the left), slides between them on a spring.
fn profile_switcher(profiles: &Profiles, switching: Option<&str>, slot: f32, m: &Shared, palette: &MandaraPalette) -> AnyElement {
    let n = profiles.available.len().max(1);
    let shown = switching.unwrap_or(&profiles.active);
    let (from, to) = profile_accent(shown, palette);
    let seg_w = (MODERN_W - PAD * 2.0 - SEG_PAD * 2.0) / n as f32;
    let outer_r = crate::style::radius(SEG_RADIUS_PX);
    let inner_r = crate::style::concentric(outer_r, SEG_PAD);
    let base = palette.surface_container_high;
    let indicator = div()
        .absolute()
        .top(px(SEG_PAD))
        .left(px(SEG_PAD + slot * seg_w))
        .w(px(seg_w))
        .h(px(SEG_H - SEG_PAD * 2.0))
        .rounded(px(inner_r))
        .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, base.mix(from, SEG_TINT.0), base.mix(to, SEG_TINT.1)))
        .shadow(rim(palette));
    let hover = palette.on_surface.with_alpha(alpha(SEG_HOVER, palette));
    let segments = profiles.available.iter().map(|p| {
        let name = p.name.as_str();
        let active = name == shown;
        let pending = switching == Some(name);
        let held = profiles.holds.iter().any(|h| h.profile == name);
        let tint = if active { readable(profile_accent(name, palette).0, palette) } else { palette.on_surface_variant };
        let (m, target) = (m.clone(), name.to_string());
        div()
            .id(hashed_id("power-profile", name))
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .rounded(px(inner_r))
            .row()
            .items_center()
            .justify_center()
            .gap(px(SEG_INNER_GAP_PX))
            .cursor_pointer()
            .when(!active, |el| el.hover(move |s| s.bg(hover.hsla())))
            .child(icon(if pending { "timer" } else { profile_icon(name) }, SEG_ICON_PX, tint))
            .child(
                div()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(text::BODY_S))
                    .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                    .text_color(if active { palette.on_surface.hsla() } else { palette.on_surface_variant.hsla() })
                    .child(profile_label(name)),
            )
            .when(held && !pending, |el| el.child(icon("pin", SEG_MARK_PX, palette.on_surface_variant)))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                set_profile(&m, target.clone(), cx);
            })
    });
    div()
        .relative()
        .h(px(SEG_H))
        .p(px(SEG_PAD))
        .rounded(px(outer_r))
        .bg(wash(WASH_FAINT, palette).hsla())
        .child(indicator)
        .child(div().relative().size_full().row().children(segments))
        .into_any_element()
}

/// The indicator's slot (`0..n`, left to right on screen) for the active
/// or pending profile.
fn profile_slot(profiles: &Profiles, switching: Option<&str>, rtl: bool) -> f32 {
    let shown = switching.unwrap_or(&profiles.active);
    let n = profiles.available.len();
    let i = profiles.available.iter().position(|p| p.name == shown).unwrap_or(0);
    let visual = if rtl { n.saturating_sub(1 + i) } else { i };
    visual as f32
}

fn hold_row(app: &str, reason: &str, profile: &str, palette: &MandaraPalette) -> AnyElement {
    let (from, _) = profile_accent(profile, palette);
    let tint = readable(from, palette);
    div()
        .h(px(HOLD_H - ROW_INSET_PX))
        .px(px(ROW_PAD_X_PX))
        .rounded(px(crate::style::radius(ROW_RADIUS_PX)))
        .bg(palette.on_surface.with_alpha(alpha(HOLD_FILL, palette)).hsla())
        .row()
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
        .row()
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
        .row()
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

/// The charge-limit switch, for a battery whose limit UPower can switch.
fn limit_card(d: &PowerDevice, m: &Shared, busy: bool, radius: f32, palette: &MandaraPalette) -> Option<AnyElement> {
    let th = d.threshold.as_ref().filter(|th| th.supported)?;
    let (m, path, on) = (m.clone(), d.path.clone(), th.enabled);
    Some(
        glass_card(palette)
            .id(hashed_id("power-limit", &d.path))
            .rounded(px(radius))
            .h(px(LIMIT_H))
            .px(px(LIMIT_PAD_X_PX))
            .row()
            .items_center()
            .gap(px(tokens::MD))
            .cursor_pointer()
            .when(busy, |el| el.opacity(LIMIT_BUSY_OPACITY))
            .child(icon_chip("heart", readable(palette.success_emerald, palette), LIMIT_CHIP_PX))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(INFO_LINE_GAP_PX))
                    .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child(t("Charge limit")))
                    .child(div().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(limit_text(th.start, th.end))),
            )
            .child(switch(on, palette))
            .on_click(move |_, _, cx| {
                if !m.borrow().limit_busy {
                    set_limit(&m, path.clone(), !on, cx);
                }
            })
            .into_any_element(),
    )
}

fn peripheral_row(d: &PowerDevice, low_percent: f64, palette: &MandaraPalette) -> AnyElement {
    let (from, to) = level_colors(d, low_percent, palette);
    let frac = level_fraction(d.percentage);
    div()
        .h(px(PERIPHERAL_H))
        .px(px(PERIPH_PAD_X_PX))
        .row()
        .items_center()
        .gap(px(PERIPH_GAP_PX))
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

/// One group of the *Details* fold: a heading and label / value rows.
struct DetailGroup {
    title: String,
    rows: Vec<(&'static str, String)>,
}

/// Technical readouts kept out of the main view: per battery its rate,
/// energy, voltage, cycles, temperature and a firmware-set charge limit;
/// the active profile's drivers (the CPU scaling driver such as
/// `intel_pstate` or `amd_pstate`, the platform driver).
fn detail_groups(batteries: &[&PowerDevice], profiles: Option<&Profiles>) -> Vec<DetailGroup> {
    let mut groups = Vec::new();
    for b in batteries {
        let mut rows: Vec<(&'static str, String)> = Vec::new();
        if b.energy_rate > 0.0 {
            rows.push((if b.state.is_charging() { t("Charging rate") } else { t("Power draw") }, format!("{:.1} W", b.energy_rate)));
        }
        if b.energy > 0.0 {
            rows.push((t("Energy now"), wh(b.energy)));
        }
        if b.energy_full_design > 0.0 {
            rows.push((t("Design capacity"), wh(b.energy_full_design)));
        }
        if b.voltage > 0.0 {
            rows.push((t("Voltage"), format!("{:.1} V", b.voltage)));
        }
        if let Some(c) = b.charge_cycles {
            rows.push((t("Cycles"), c.to_string()));
        }
        if let Some(c) = b.temperature {
            rows.push((t("Temperature"), format!("{c:.0} °C")));
        }
        // A limit set by firmware or root, which UPower can't switch.
        if let Some(th) = b.threshold.as_ref().filter(|th| !th.supported) {
            let state = if th.enabled { limit_text(th.start, th.end) } else { t("Off").into() };
            rows.push((t("Charge limit"), tf("{limit} · set by firmware or root (read-only)", &[("limit", &state)])));
        }
        if !rows.is_empty() {
            let title = if batteries.len() > 1 { b.display_name() } else { t("Battery").into() };
            groups.push(DetailGroup { title, rows });
        }
    }
    if let Some(p) = profiles.and_then(|ps| ps.available.iter().find(|x| x.name == ps.active)) {
        let mut rows: Vec<(&'static str, String)> = Vec::new();
        if let Some(cpu) = p.cpu_driver.as_deref() {
            rows.push((t("CPU scaling driver"), cpu.to_string()));
        }
        if let Some(platform) = p.platform_driver.as_deref() {
            rows.push((t("Platform driver"), platform.to_string()));
        }
        if !p.driver.is_empty() && p.cpu_driver.as_deref() != Some(p.driver.as_str()) {
            rows.push((t("Profile driver"), p.driver.clone()));
        }
        if !rows.is_empty() {
            groups.push(DetailGroup { title: tf("Power profile · {profile}", &[("profile", &profile_label(&p.name))]), rows });
        }
    }
    groups
}

/// How far the *Details* chevron turns: none when closed; a quarter turn
/// when open, clockwise for the right-pointing chevron and anticlockwise
/// for the left-pointing one right-to-left languages get, so it points
/// down either way.
fn fold_chevron_turn(open: bool, rtl: bool) -> f32 {
    match (open, rtl) {
        (false, _) => 0.0,
        (true, false) => std::f32::consts::FRAC_PI_2,
        (true, true) => -std::f32::consts::FRAC_PI_2,
    }
}

/// The *Details* fold: a header row, and when open a card of groups.
fn details(groups: &[DetailGroup], open: bool, m: &Shared, radius: f32, palette: &MandaraPalette) -> (AnyElement, f32) {
    let muted = palette.on_surface_variant;
    let hover = palette.on_surface.with_alpha(alpha(FOLD_HOVER, palette));
    let toggle = m.clone();
    let chevron = gpui::svg()
        .path(format!("icons/{}.svg", crate::bidi::dir_icon("chevron-right")))
        .size(px(FOLD_CHEVRON_PX))
        .flex_none()
        .text_color(muted.hsla())
        .with_transformation(gpui::Transformation::rotate(gpui::radians(fold_chevron_turn(open, crate::bidi::rtl()))));
    let head = div()
        .id("power-details")
        .h(px(FOLD_H))
        .px(px(FOLD_PAD_X_PX))
        .rounded(px(radius))
        .row()
        .items_center()
        .gap(px(tokens::SM))
        .cursor_pointer()
        .hover(move |s| s.bg(hover.hsla()))
        .child(div().flex_1().text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).text_color(muted.hsla()).child(t("Details")))
        .child(chevron)
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            let views = {
                let mut mb = toggle.borrow_mut();
                mb.details_open = !mb.details_open;
                mb.views.clone()
            };
            views.notify(cx);
        });
    let mut height = FOLD_H;
    let mut col = div().flex().flex_col().child(head);
    if open {
        let mut card = glass_card(palette).rounded(px(radius)).p(px(DETAILS_PAD_PX)).flex().flex_col().gap(px(DETAIL_GROUP_GAP));
        let mut card_h = DETAILS_PAD_PX * 2.0 + DETAIL_GROUP_GAP * groups.len().saturating_sub(1) as f32;
        for g in groups {
            let rows = g.rows.iter().map(|(label, value)| {
                div()
                    .h(px(DETAIL_ROW_H))
                    .row()
                    .items_center()
                    .justify_between()
                    .gap(px(tokens::MD))
                    .text_size(px(text::BODY_S))
                    .child(div().flex_none().text_color(muted.hsla()).child(*label))
                    .child(div().min_w(px(0.0)).truncate().font_features(tabular_nums()).font_weight(FontWeight::MEDIUM).child(value.clone()))
            });
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().h(px(DETAIL_HEAD_H)).flex().items_center().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(muted.hsla()).truncate().child(g.title.clone()))
                    .children(rows),
            );
            card_h += DETAIL_HEAD_H + DETAIL_ROW_H * g.rows.len() as f32;
        }
        col = col.child(div().mt(px(SECTION_GAP)).child(card));
        height += SECTION_GAP + card_h;
    }
    (col.into_any_element(), height)
}

fn toast(ok: bool, msg: SharedString, width: f32, palette: &MandaraPalette) -> AnyElement {
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
                .max_w(px(width - PAD * TOAST_SIDE_PADS))
                .h(px(TOAST_H))
                .pl(px(TOAST_PAD_L_PX))
                .pr(px(TOAST_PAD_R_PX))
                .rounded_full()
                .bg(palette.surface_container_high.hsla())
                .shadow(vec![glow(Color::oklch(0.0, 0.0, 0.0).with_alpha(alpha(TOAST_SHADOW, palette)), TOAST_SHADOW_BLUR_PX, 0.0, TOAST_SHADOW_DY_PX)])
                .row()
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
/// Charging "breathing": slower and softer than the critical pulse, and as
/// cheap — the chip alone redraws, at this cadence, only while charging.
const CHARGE_PULSE_PERIOD_S: f32 = 2.4;
const CHARGE_PULSE_FRAME: Duration = Duration::from_millis(40);
const CHARGE_PULSE_MIN_ALPHA: f32 = 0.45;

/// A breath: `1` at the start of each `period`, easing down to `min` half
/// way and back (a raised cosine, so it never jerks).
fn breath(secs: f32, period: f32, min: f32) -> f32 {
    let wave = 0.5 + 0.5 * (std::f32::consts::TAU * secs / period.max(f32::EPSILON)).cos();
    min + (1.0 - min) * wave
}

fn wall_secs() -> f32 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| (d.as_secs_f64() % f64::from(u16::MAX)) as f32).unwrap_or(0.0)
}

/// The chip's alpha while the charge is critical (`power.critical_pulse`).
fn critical_pulse(window: &mut Window, cx: &mut App) -> Option<f32> {
    if !crate::powerctl::battery::critical(cx) {
        return None;
    }
    redraw_in(PULSE_FRAME, window, cx);
    Some(breath(wall_secs(), PULSE_PERIOD_S, PULSE_MIN_ALPHA))
}

/// The charge fill's alpha while the battery charges (`power.charging_pulse`):
/// off with reduced motion, while the bar is put away, and once the battery
/// stops taking charge (full, held at its limit).
fn charging_pulse(m: &Shared, window: &mut Window, cx: &mut App) -> Option<f32> {
    let cfg = crate::state::config(cx).read(cx).get();
    if !cfg.power.charging_pulse || cfg.motion.reduced(mandara_core::spring::system_reduced_motion()) || crate::bar::autohide::hidden_by_command() {
        return None;
    }
    if !m.borrow().state.batteries().next().is_some_and(|b| b.state == ChargeState::Charging) {
        return None;
    }
    redraw_in(CHARGE_PULSE_FRAME, window, cx);
    Some(breath(wall_secs(), CHARGE_PULSE_PERIOD_S, CHARGE_PULSE_MIN_ALPHA))
}

/// How the chip breathes this frame.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Pulse {
    /// Coral, while critical (the alpha of this frame).
    Critical(f32),
    /// The charge colour, while charging.
    Charging(f32),
}

fn chip_view(m: &Shared, palette: &MandaraPalette, window: &Window, pulse: Option<Pulse>, low_percent: f64, show_percent: bool) -> AnyElement {
    let mb = m.borrow();
    mb.views.track(window);
    if let Some(b) = mb.state.batteries().next() {
        let (from, _) = level_colors(b, low_percent, palette);
        let low = b.percentage <= low_percent && !b.state.is_charging();
        let from = match pulse {
            Some(Pulse::Critical(alpha)) => palette.urgent_coral.with_alpha(alpha),
            Some(Pulse::Charging(alpha)) => from.with_alpha(alpha),
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
            .when(show_percent, |el| el.child(div().font_features(tabular_nums()).text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).child(percent_text(frac))))
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
    /// The body's scroll position and its overlay bar.
    scroll: (gpui::ScrollHandle, Rc<crate::scrollbar::ScrollbarState>),
}

impl View {
    /// The indicator's slot this frame: made at `target` on first use, then
    /// springing toward it (a frame is requested while it moves).
    fn slide(&self, target: f32, window: &Window) -> f32 {
        let mut mb = self.m.borrow_mut();
        let (spring, clock) = mb.segment.get_or_insert_with(|| (SpringValue::new(target, SEG_SPRING), FrameClock::new()));
        if spring.target() != target {
            spring.set_target(target);
        }
        if !spring.is_animating() {
            clock.idle();
            return spring.value();
        }
        spring.step(clock.tick(Instant::now()));
        if (spring.value() - target).abs() < SEG_SETTLE {
            spring.snap_to_target();
            clock.idle();
        } else {
            window.request_animation_frame();
        }
        spring.value()
    }

    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let m = &self.m;
        m.borrow().views.track(window);
        let span = history_span(cx);
        let peripherals_on = show_peripherals(cx);
        let (low, peripheral_low) = (low_percent(cx), peripheral_low_percent(cx));
        let classic = crate::state::config(cx).read(cx).get().power.panel_style == PowerPanelStyle::Classic;
        let width = if classic { CLASSIC_W } else { MODERN_W };
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
        // The Modern indicator's slot is settled before the model is
        // borrowed for the build below.
        let slot = if classic {
            None
        } else {
            let target = {
                let mb = m.borrow();
                mb.state.profiles.as_ref().filter(|p| !p.available.is_empty()).map(|p| profile_slot(p, mb.switching.as_ref().map(|(s, _)| s.as_str()), crate::bidi::rtl()))
            };
            target.map(|target| self.slide(target, window))
        };

        let mb = m.borrow();
        let state = &mb.state;
        let accent = readable(palette.primary, palette);
        let batteries: smallvec::SmallVec<[&PowerDevice; INLINE_BATTERIES]> = state.batteries().collect();
        let peripherals: smallvec::SmallVec<[&PowerDevice; INLINE_PERIPHERALS]> = if peripherals_on { state.peripherals().collect() } else { smallvec::SmallVec::new() };

        // ── Header ───────────────────────────────────────────────────────
        let subtitle: String = if classic {
            classic::subtitle(batteries.first().copied(), state.profiles.as_ref().map(|p| p.active.as_str()), state.ready)
        } else {
            match batteries.first() {
                Some(b) => b.display_name(),
                None if !state.ready => t("Reading power state…").into(),
                None if state.upower => t("On mains power").into(),
                None => state.profiles.as_ref().map(|p| profile_label(&p.active)).unwrap_or_default(),
            }
        };
        let glyph = if classic || batteries.is_empty() { "gauge" } else { "battery" };
        let head = header(glyph, accent, t("Power"), subtitle, palette, None);

        let mut blocks: Vec<AnyElement> = Vec::new();
        let mut height = PAD * 2.0 + HEADER_H;
        let mut push = |el: AnyElement, h: f32, blocks: &mut Vec<AnyElement>| {
            blocks.push(el);
            height += GAP + h;
        };

        if !state.ready {
            push(empty_state("gauge", t("Reading power state…"), t("Asking power-profiles-daemon and UPower"), LOADING_H, palette).into_any_element(), LOADING_H, &mut blocks);
        } else {
            let switching = mb.switching.as_ref().map(|(s, _)| s.as_str());
            let profiles = state.profiles.as_ref().filter(|p| !p.available.is_empty());
            let unavailable = || {
                let hint = if cfg!(target_os = "freebsd") {
                    t("FreeBSD has no power-profiles-daemon; powerd(8) sets the processor's speed there")
                } else if state.profiles_error.as_ref().is_some_and(|e| !e.is_empty()) {
                    t("Install and enable power-profiles-daemon to switch profiles")
                } else {
                    t("No profiles reported by the daemon")
                };
                (info_card("gauge", t("Power profiles unavailable"), hint, ERROR_H, palette.warning_amber, card_r, palette), ERROR_H)
            };
            // Classic: the profiles lead.
            if classic {
                let (el, h) = match profiles {
                    Some(p) => classic::profiles_block(p, switching, m, card_r, palette),
                    None => unavailable(),
                };
                push(el, h, &mut blocks);
            }

            // ── Batteries ────────────────────────────────────────────────
            if !state.upower {
                push(info_card("battery", t("UPower is not running"), t("Battery details need the upower service"), ERROR_H, palette.warning_amber, card_r, palette), ERROR_H, &mut blocks);
            } else if batteries.is_empty() {
                push(info_card("plug", t("No battery"), t("This computer runs on mains power"), NO_BATTERY_H, palette.primary, card_r, palette), NO_BATTERY_H, &mut blocks);
            } else if classic {
                let (el, h) = classic::batteries_block(&batteries, &mb.history, span, m, mb.limit_busy, card_r, low, palette);
                push(el, h, &mut blocks);
            } else {
                let named = batteries.len() > 1;
                for b in &batteries {
                    let (card, h) = battery_hero(b, named, mb.history.get(&b.path), span, card_r, low, palette);
                    push(card, h, &mut blocks);
                }
            }

            // ── Profiles (Modern) ────────────────────────────────────────
            match (profiles, slot) {
                _ if classic => {}
                (Some(profiles), Some(slot)) => {
                    let mut col = div().flex().flex_col().gap(px(SECTION_GAP)).child(profile_switcher(profiles, switching, slot, m, palette));
                    let mut h = SEG_H;
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
                    let (el, h) = unavailable();
                    push(el, h, &mut blocks);
                }
            }

            // ── Charge limit (Modern; Classic shows it in the battery card) ─
            for b in batteries.iter().filter(|_| !classic) {
                if let Some(card) = limit_card(b, m, mb.limit_busy, card_r, palette) {
                    push(card, LIMIT_H, &mut blocks);
                }
            }

            // ── Peripherals ──────────────────────────────────────────────
            if !peripherals.is_empty() {
                let list = glass_card(palette).rounded(px(card_r)).p(px(PERIPH_CARD_PAD_PX)).flex().flex_col().children(peripherals.iter().map(|d| peripheral_row(d, peripheral_low, palette)));
                let h = SECTION_H + SECTION_GAP + PERIPH_CARD_PAD_PX * 2.0 + PERIPHERAL_H * peripherals.len() as f32;
                push(div().flex().flex_col().gap(px(SECTION_GAP)).child(section(t("Devices"), palette, None)).child(list).into_any_element(), h, &mut blocks);
            }

            // ── Details (Modern) ─────────────────────────────────────────
            let groups = if classic { Vec::new() } else { detail_groups(&batteries, state.profiles.as_ref()) };
            if !groups.is_empty() {
                let (el, h) = details(&groups, mb.details_open, m, card_r, palette);
                push(el, h, &mut blocks);
            }
        }
        drop((batteries, peripherals));
        drop(mb);
        let shown = self.size_cell.get();
        if (f32::from(shown.height) - height).abs() > RESIZE_EPSILON_PX || (f32::from(shown.width) - width).abs() > RESIZE_EPSILON_PX {
            self.size_cell.set(size(px(width), px(height)));
        }

        // Under the fixed header the blocks scroll when the screen is too
        // short for them (a small laptop, the open *Details*); they keep
        // their size while the panel springs open instead of squeezing.
        let p = &self.p;
        let mut body = div().flex_none().w_full().flex().flex_col();
        for (i, b) in blocks.into_iter().enumerate() {
            body = body.child(div().flex_none().h(px(GAP))).child(div().flex_none().child(reveal(p, i + 1, b)));
        }
        let (handle, bar) = &self.scroll;
        let scroller = div().id("power-scroll").flex_1().min_h(px(0.0)).overflow_y_scroll().track_scroll(handle).child(body);
        div()
            .relative()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .child(div().flex_none().child(reveal(p, 0, head)))
            .child(crate::scrollbar::with_scrollbar(scroller, handle, bar, palette).flex_1().min_h(px(0.0)).flex().flex_col())
            .when_some(status, |el, (ok, msg, _)| el.child(toast(ok, msg, width, palette)))
            .into_any_element()
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
                let pulse = critical_pulse(window, cx).map(Pulse::Critical).or_else(|| charging_pulse(&bar_m, window, cx).map(Pulse::Charging));
                // `[plugins.settings.power] bar_percent`: the charge beside the battery.
                let show_percent = crate::state::config(cx).read(cx).get().plugins.setting(ID, "bar_percent", true);
                chip_view(&bar_m, palette, window, pulse, low_percent(cx), show_percent)
            },
            window,
            cx,
        );
        let first_w = if crate::state::config(cx).read(cx).get().power.panel_style == PowerPanelStyle::Classic { CLASSIC_W } else { MODERN_W };
        let size_cell = Rc::new(Cell::new(size(px(first_w), px(PAD * 2.0 + HEADER_H + GAP + LOADING_H))));
        let view = View { p: panel_cx.clone(), m: m.clone(), size_cell: size_cell.clone(), scroll: (gpui::ScrollHandle::new(), crate::scrollbar::ScrollbarState::new()) };
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
    use mandara_services::power::{Profile, Threshold};

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
    fn the_time_line_never_shows_a_made_up_estimate() {
        let mut d = PowerDevice { state: ChargeState::Discharging, ..Default::default() };
        assert_eq!(time_line(&d).as_deref(), Some("Estimating…"));
        d.time_to_empty = 3 * 3600 + 49 * 60;
        assert_eq!(time_line(&d).as_deref(), Some("3 h 49 min left"));
        d.state = ChargeState::FullyCharged;
        assert_eq!(time_line(&d).as_deref(), Some("On mains power"));
        // Held at the charge limit with no estimate: mains power, never
        // "Estimating…" (both styles show this line).
        d = PowerDevice { state: ChargeState::PendingCharge, ..Default::default() };
        assert_eq!(time_line(&d).as_deref(), Some("On mains power"));
        d.state = ChargeState::Unknown;
        assert_eq!(time_line(&d), None);
    }

    #[test]
    fn the_open_details_chevron_points_down_in_both_directions() {
        // Screen coordinates (y grows down): the closed chevron points to
        // the reading end, the open one down.
        let turned = |(x, y): (f32, f32), a: f32| (x * a.cos() - y * a.sin(), x * a.sin() + y * a.cos());
        for (rtl, closed) in [(false, (1.0, 0.0)), (true, (-1.0, 0.0))] {
            assert_eq!(fold_chevron_turn(false, rtl), 0.0);
            let (x, y) = turned(closed, fold_chevron_turn(true, rtl));
            assert!(x.abs() < 1e-6 && (y - 1.0).abs() < 1e-6, "rtl={rtl}: ({x}, {y})");
        }
    }

    #[test]
    fn profiles_have_labels_and_icons() {
        assert_eq!(profile_label("power-saver"), "Power saver");
        assert_eq!(profile_icon("performance"), "zap");
        assert_eq!(profile_icon("custom"), "gauge");
        assert_eq!(profile_label("custom"), "custom");
        assert!(degraded_text("lap-detected").contains("lap"));
    }

    fn profiles(names: &[&str], active: &str) -> Profiles {
        let p = |n: &&str| Profile { name: n.to_string(), driver: "multiple".into(), cpu_driver: Some("intel_pstate".into()), platform_driver: Some("platform_profile".into()) };
        Profiles { active: active.into(), available: names.iter().map(p).collect(), ..Default::default() }
    }

    #[test]
    fn the_indicator_sits_on_the_shown_profile_and_mirrors_in_rtl() {
        let three = profiles(&["power-saver", "balanced", "performance"], "balanced");
        assert_eq!(profile_slot(&three, None, false), 1.0);
        assert_eq!(profile_slot(&three, Some("performance"), false), 2.0);
        assert_eq!(profile_slot(&three, Some("performance"), true), 0.0);
        // Only the profiles the daemon offers are slots.
        let two = profiles(&["power-saver", "balanced"], "power-saver");
        assert_eq!(profile_slot(&two, None, false), 0.0);
        assert_eq!(profile_slot(&two, None, true), 1.0);
    }

    #[test]
    fn drivers_live_in_the_details() {
        let b = PowerDevice { kind: DeviceKind::Battery, state: ChargeState::Discharging, energy_rate: 9.6, energy: 38.8, energy_full_design: 61.8, voltage: 12.4, charge_cycles: Some(214), temperature: Some(31.5), ..Default::default() };
        let p = profiles(&["power-saver", "balanced", "performance"], "balanced");
        let groups = detail_groups(&[&b], Some(&p));
        assert_eq!(groups.len(), 2);
        let battery: Vec<&str> = groups[0].rows.iter().map(|(l, _)| *l).collect();
        assert_eq!(battery, ["Power draw", "Energy now", "Design capacity", "Voltage", "Cycles", "Temperature"]);
        let values: Vec<&str> = groups[1].rows.iter().map(|(_, v)| v.as_str()).collect();
        assert_eq!(values, ["intel_pstate", "platform_profile", "multiple"]);
        // A desktop: drivers only.
        assert_eq!(detail_groups(&[], Some(&p)).len(), 1);
        assert!(detail_groups(&[], None).is_empty());
    }

    #[test]
    fn a_read_only_limit_is_a_detail_and_a_switchable_one_is_not() {
        let mut b = PowerDevice { kind: DeviceKind::Battery, threshold: Some(Threshold { supported: false, enabled: true, start: None, end: Some(80) }), ..Default::default() };
        let rows = &detail_groups(&[&b], None)[0].rows;
        assert_eq!(rows[0].0, "Charge limit");
        assert!(rows[0].1.contains("80%") && rows[0].1.contains("read-only"));
        b.threshold = Some(Threshold { supported: true, enabled: true, start: Some(60), end: Some(80) });
        assert!(detail_groups(&[&b], None).is_empty());
        assert_eq!(limit_text(Some(60), Some(80)), "Stops at 80%, resumes below 60%");
    }

    fn pt(time: u32, value: f64) -> HistoryPoint {
        HistoryPoint { time, value, state: ChargeState::Discharging }
    }

    #[test]
    fn the_curve_joins_samples_and_ends_at_the_live_charge() {
        let span = Duration::from_secs(400);
        // Samples at 600 (80 %) and 800 (60 %); now = 1000 with 50 % live.
        let c = charge_curve(&[pt(600, 80.0), pt(800, 60.0)], 1000, span, 5, Some(50.0));
        assert_eq!(c, vec![Some(0.8), Some(0.7), Some(0.6), Some(0.55), Some(0.5)]);
        // Nothing before the first sample is invented.
        let c = charge_curve(&[pt(900, 40.0)], 1000, span, 5, None);
        assert_eq!(c, vec![None, None, None, Some(0.4), Some(0.4)]);
        // No history, no curve.
        assert!(charge_curve(&[], 1000, span, 5, Some(50.0)).is_empty());
        // Samples from the future (clock skew) are ignored.
        assert!(charge_curve(&[pt(2000, 40.0)], 1000, span, 5, None).is_empty());
    }

    #[test]
    fn the_curve_reads_between_samples_and_respects_gaps() {
        let c = [None, Some(0.2), Some(0.6)];
        assert_eq!(curve_at(&c, 0.0), None);
        assert_eq!(curve_at(&c, 0.5), Some(0.2));
        assert!((curve_at(&c, 0.75).unwrap() - 0.4).abs() < 1e-6);
        assert_eq!(curve_at(&c, 1.0), Some(0.6));
        assert_eq!(curve_at(&[], 0.5), None);
    }

    #[test]
    fn breathing_is_gentle_and_periodic() {
        assert!((breath(0.0, 2.4, 0.45) - 1.0).abs() < 1e-6);
        assert!((breath(1.2, 2.4, 0.45) - 0.45).abs() < 1e-6);
        assert!((breath(2.4, 2.4, 0.45) - 1.0).abs() < 1e-5);
        for i in 0..100 {
            let a = breath(i as f32 * 0.07, 2.4, 0.45);
            assert!((0.45..=1.0).contains(&a));
        }
    }
}
