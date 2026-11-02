//! Eye breaks (built-in `breaks` plugin, opt-in): gentle 20-20-20
//! reminders. After `every_min` of active work it is time to look at
//! something 6 metres away for `short_s` seconds; every `long_every_min` a
//! longer `long_min` break. Only time at the keyboard counts: idleness comes
//! from the shell's idle tracking (`ext-idle-notify`, see
//! [`crate::powerctl::idle`]; with idle stages switched off every minute
//! counts), and being away for at least a break's length counts as that
//! break.
//!
//! The schedule is [`mandara_services::breaks::Schedule`]; one timer sleeps
//! until the next moment anything can happen (a break due, a break over, a
//! pause ending) instead of ticking. When a break is due it posts a desktop
//! notification, or with `style = "overlay"` dims every screen with a
//! countdown ring and Skip / Snooze buttons (the overlay never takes
//! keyboard focus). While a fullscreen window has focus (a game, a video)
//! the reminder waits. The panel ticks at most once a second while open;
//! the bar chip once a minute. Today's taken / skipped counts are kept in
//! `~/.local/state/mandara/breaks.json`.
//!
//! Settings (`[plugins.settings.breaks]`): `every_min` (20), `short_s`
//! (20), `long_every_min` (60, 0 = no long breaks), `long_min` (5),
//! `style` (`notification` / `overlay`), `skip_fullscreen` (on).
//!
//! Commands: `breaks status`, `breaks now [short|long]`, `breaks skip`,
//! `breaks snooze [minutes]`, `breaks pause [minutes]`, `breaks resume`.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, Global, IntoElement, ParentElement, Render, Styled, Task, Window, WindowHandle, div,
    prelude::FluentBuilder as _, px, size,
};
use mandara_config::ShellConfig;
use mandara_core::{LayerSurfaceSpec, SurfaceRole, ToGpui};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::NotificationCommand;
use mandara_services::breaks::{self as bk, Event, Kind, Outcome, Rules, Schedule, Tally};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::extras::caption;
use super::panel_fx::{EXPIRY_MARGIN, HEADER_H, PILL_H_PX, chip, dot_ring, duration_short, glass_card, header, partner, pill_button, readable, redraw_in, wash, WASH_IDLE_BAR};
use super::{fn_view, follow};
use crate::fonts::{fonts, tabular_nums};
use crate::icons::{icon, ring};
use crate::kit::{format_duration, reveal, text, tokens};
use crate::state::config;

pub const ID: &str = "breaks";

// ── Behaviour ────────────────────────────────────────────────────────────

const DEFAULT_EVERY_MIN: i64 = 20;
const DEFAULT_SHORT_S: i64 = 20;
const DEFAULT_LONG_EVERY_MIN: i64 = 60;
const DEFAULT_LONG_MIN: i64 = 5;
/// Bounds on the settings (minutes / seconds).
const MAX_EVERY_MIN: i64 = 240;
const MAX_SHORT_S: i64 = 600;
const MAX_LONG_MIN: i64 = 60;
/// A notification reminder unanswered this long counts as skipped.
const MISS_AFTER: Duration = Duration::from_secs(5 * 60);
/// A due break waits this long behind a fullscreen window, then asks again.
const FULLSCREEN_RETRY: Duration = Duration::from_secs(60);
/// Default snooze and pause lengths, minutes, and the longest accepted.
const SNOOZE_MIN: u64 = 5;
const PAUSE_MIN: u64 = 60;
const MAX_POSTPONE_MIN: i64 = 24 * 60;
const SECS_PER_MIN: u64 = 60;
/// The bar ring turns amber in the last minute.
const LAST_MINUTE: Duration = Duration::from_secs(60);
const SECOND: Duration = Duration::from_secs(1);

// ── Layout ───────────────────────────────────────────────────────────────

const PANEL_W: f32 = 360.0;
const PAD: f32 = 14.0;
const GAP: f32 = 12.0;
/// Hero card: height, padding, the dotted ring and its dots.
const HERO_H: f32 = 164.0;
const HERO_PAD_PX: f32 = 16.0;
const HERO_RING_PX: f32 = 132.0;
const HERO_DOTS: usize = 48;
const HERO_DOT_PX: f32 = 4.0;
const HERO_CLOCK_PX: f32 = 30.0;
const HERO_STAT_GAP_PX: f32 = 10.0;
const STAT_LINE_GAP_PX: f32 = 2.0;
/// Bar chip.
const BAR_RING_PX: f32 = 15.0;
const BAR_RING_STROKE_PX: f32 = 2.2;
const BAR_ICON_PX: f32 = 15.0;
const BAR_GAP_PX: f32 = 5.0;
const RING_TRACK_ALPHA: f32 = 0.16;
/// The overlay: veil depth and fade-in, the card and its ring.
const VEIL_ALPHA: f32 = 0.78;
const VEIL_DEPTH: f32 = 0.6;
const VEIL_BASE_HEX: &str = "#000000";
const FADE_IN: Duration = Duration::from_millis(700);
const OVERLAY_RING_PX: f32 = 220.0;
const OVERLAY_DOTS: usize = 60;
const OVERLAY_DOT_PX: f32 = 5.0;
const OVERLAY_CLOCK_PX: f32 = 46.0;
const OVERLAY_TITLE_PX: f32 = 22.0;
const OVERLAY_GAP_PX: f32 = 18.0;
const OVERLAY_CARD_PAD_PX: f32 = 32.0;
const OVERLAY_CARD_W_PX: f32 = 420.0;
/// Output size assumed when nothing reports one.
const FALLBACK_OUTPUT: (f32, f32) = (1920.0, 1080.0);

// ── Settings ─────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Style {
    Notification,
    Overlay,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Settings {
    rules: Rules,
    style: Style,
    skip_fullscreen: bool,
}

fn minutes(m: i64) -> Duration {
    Duration::from_secs(u64::try_from(m).unwrap_or(0) * SECS_PER_MIN)
}

fn settings(cfg: &ShellConfig) -> Settings {
    let get = |key: &str, default: i64| cfg.plugins.setting(ID, key, default);
    let long_every = get("long_every_min", DEFAULT_LONG_EVERY_MIN).clamp(0, MAX_EVERY_MIN);
    Settings {
        rules: Rules {
            every: minutes(get("every_min", DEFAULT_EVERY_MIN).clamp(1, MAX_EVERY_MIN)),
            short: Duration::from_secs(u64::try_from(get("short_s", DEFAULT_SHORT_S).clamp(5, MAX_SHORT_S)).unwrap_or(20)),
            long_every: (long_every > 0).then(|| minutes(long_every)),
            long: minutes(get("long_min", DEFAULT_LONG_MIN).clamp(1, MAX_LONG_MIN)),
        },
        style: if cfg.plugins.setting(ID, "style", String::from("notification")) == "overlay" { Style::Overlay } else { Style::Notification },
        skip_fullscreen: cfg.plugins.setting(ID, "skip_fullscreen", true),
    }
}

fn miss_after(style: Style) -> Option<Duration> {
    (style == Style::Notification).then_some(MISS_AFTER)
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

// ── Model ────────────────────────────────────────────────────────────────

pub struct Breaks {
    /// `None` while the plugin is off.
    schedule: Option<Schedule>,
    tally: Tally,
    /// Sleeps until the schedule's next moment (dropping it cancels).
    timer: Option<Task<()>>,
    /// The idle state last seen from the idle manager.
    idle_seen: bool,
}

struct GlobalBreaks(Entity<Breaks>);
impl Global for GlobalBreaks {}

pub fn model(cx: &mut App) -> Entity<Breaks> {
    if let Some(g) = cx.try_global::<GlobalBreaks>() {
        return g.0.clone();
    }
    let entity = cx.new(|_| Breaks { schedule: None, tally: bk::load_tally(), timer: None, idle_seen: false });
    cx.set_global(GlobalBreaks(entity.clone()));
    entity
}

fn current_settings(cx: &App) -> Settings {
    settings(&config(cx).read(cx).snapshot())
}

impl Breaks {
    /// Starts or stops with the plugin switch.
    fn sync_enabled(&mut self, cx: &mut Context<Self>) {
        let cfg = config(cx).read(cx).snapshot();
        let on = cx.try_global::<PluginRegistry>().is_some_and(|r| r.is_enabled(ID, &cfg.plugins));
        match (on, self.schedule.is_some()) {
            (true, false) => {
                let mut s = Schedule::new(Instant::now());
                if self.idle_seen {
                    s.went_idle(Instant::now(), Instant::now());
                }
                self.schedule = Some(s);
            }
            (false, true) => {
                self.schedule = None;
                overlay_close(cx);
            }
            _ => {}
        }
        self.reschedule(cx);
    }

    /// Arms the one timer for the schedule's next moment.
    fn reschedule(&mut self, cx: &mut Context<Self>) {
        let st = current_settings(cx);
        let now = Instant::now();
        let wake = self.schedule.as_ref().and_then(|s| s.wake_at(&st.rules, now, miss_after(st.style)));
        self.timer = wake.map(|at| {
            let delay = at.saturating_duration_since(now) + EXPIRY_MARGIN;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(delay).await;
                let _ = this.update(cx, |b, cx| b.wake(cx));
            })
        });
        cx.notify();
    }

    fn record(&mut self, outcome: Outcome, cx: &mut Context<Self>) {
        self.tally.record(&today(), outcome);
        let tally = self.tally.clone();
        cx.background_executor().spawn(async move { bk::save_tally(&tally) }).detach();
    }

    /// The timer fired: handle whatever happened, then sleep again.
    fn wake(&mut self, cx: &mut Context<Self>) {
        let st = current_settings(cx);
        let now = Instant::now();
        let mut events = Vec::new();
        if let Some(s) = self.schedule.as_mut() {
            while let Some(e) = s.poll(&st.rules, now, miss_after(st.style)) {
                events.push(e);
            }
        }
        for event in events {
            match event {
                Event::Due(kind) => self.due(kind, st, cx),
                Event::Over(kind) => {
                    self.record(Outcome::Taken(kind), cx);
                    overlay_close(cx);
                }
                Event::Missed(kind) => self.record(Outcome::Skipped(kind), cx),
                Event::Unpaused => {}
            }
        }
        self.reschedule(cx);
    }

    fn due(&mut self, kind: Kind, st: Settings, cx: &mut Context<Self>) {
        let fullscreen = crate::hub::hub(cx).read(cx).focused.as_ref().is_some_and(|c| c.fullscreen);
        let Some(s) = self.schedule.as_mut() else { return };
        if st.skip_fullscreen && fullscreen {
            // Wait behind the game or video; ask again in a minute.
            s.snooze(FULLSCREEN_RETRY, Instant::now());
            return;
        }
        match st.style {
            Style::Overlay => {
                let kind = s.take(&st.rules, Some(kind), Instant::now());
                overlay_open(kind, st.rules.length(kind), cx);
            }
            Style::Notification => {
                let (summary, body) = match kind {
                    Kind::Short => (t("Time for an eye break").to_string(), tf("Look at something 6 metres away for {n} seconds", &[("n", &st.rules.short.as_secs())])),
                    Kind::Long => (t("Time for a longer break").to_string(), tf("Stand up, stretch and rest your eyes for {n} minutes", &[("n", &(st.rules.long.as_secs() / SECS_PER_MIN))])),
                };
                super::notifications::send(cx, NotificationCommand::Post { app: t("Eye breaks").into(), icon: "eye".into(), summary, body });
            }
        }
    }

    /// Follows the idle manager's idle / active transitions.
    fn idle_changed(&mut self, idle_for: Option<Duration>, cx: &mut Context<Self>) {
        let now = Instant::now();
        match (idle_for, self.idle_seen) {
            (Some(d), false) => {
                self.idle_seen = true;
                if let Some(s) = self.schedule.as_mut() {
                    s.went_idle(now.checked_sub(d).unwrap_or(now), now);
                }
            }
            (None, true) => {
                self.idle_seen = false;
                let rules = current_settings(cx).rules;
                let outcome = self.schedule.as_mut().and_then(|s| s.came_back(&rules, now));
                if let Some(o) = outcome {
                    self.record(o, cx);
                }
            }
            _ => return,
        }
        self.reschedule(cx);
    }
}

/// Runs an action on the schedule (an error while the plugin is off).
fn act<R>(cx: &mut App, f: impl FnOnce(&mut Schedule, &Settings, Instant) -> R) -> Result<R, String> {
    let st = current_settings(cx);
    let m = model(cx);
    m.update(cx, |b, cx| {
        let s = b.schedule.as_mut().ok_or_else(|| t("Eye breaks are off").to_string())?;
        let out = f(s, &st, Instant::now());
        b.reschedule(cx);
        Ok(out)
    })
}

fn take_now(kind: Option<Kind>, cx: &mut App) -> Result<Kind, String> {
    let kind = act(cx, |s, st, now| s.take(&st.rules, kind, now))?;
    let length = current_settings(cx).rules.length(kind);
    overlay_open(kind, length, cx);
    Ok(kind)
}

fn skip(cx: &mut App) -> Result<Outcome, String> {
    let outcome = act(cx, |s, st, now| s.skip(&st.rules, now))?;
    let m = model(cx);
    m.update(cx, |b, cx| b.record(outcome, cx));
    overlay_close(cx);
    Ok(outcome)
}

fn snooze(mins: u64, cx: &mut App) -> Result<(), String> {
    act(cx, |s, _, now| s.snooze(Duration::from_secs(mins * SECS_PER_MIN), now))?;
    overlay_close(cx);
    Ok(())
}

fn pause(mins: u64, cx: &mut App) -> Result<(), String> {
    act(cx, |s, _, now| s.pause(Duration::from_secs(mins * SECS_PER_MIN), now))?;
    overlay_close(cx);
    Ok(())
}

/// Starts following the plugin switch and the idle manager (call after
/// [`crate::powerctl::init`]).
pub fn init(cx: &mut App) {
    let m = model(cx);
    m.update(cx, |b, cx| b.sync_enabled(cx));
    let mut last = config(cx).read(cx).snapshot().plugins.clone();
    let e = m.clone();
    cx.observe(&config(cx), move |c, cx| {
        let plugins = c.read(cx).snapshot().plugins.clone();
        if plugins != last {
            last = plugins;
            e.update(cx, |b, cx| b.sync_enabled(cx));
        }
    })
    .detach();
    if let Some(idle) = crate::powerctl::idle::manager(cx) {
        let e = m.clone();
        cx.observe(&idle, move |idle, cx| {
            let idle_for = idle.read(cx).idle_for();
            e.update(cx, |b, cx| b.idle_changed(idle_for, cx));
        })
        .detach();
    }
}

// ── What the views show ──────────────────────────────────────────────────

/// A snapshot of the schedule for display.
struct Now {
    state: &'static str,
    kind: Kind,
    /// Time left to the next break (or of the current one).
    left: Duration,
    /// The ring: how far toward the next break (or through the current one).
    frac: f32,
    long_in: Option<Duration>,
    paused_until: Option<Instant>,
    on_break: bool,
    due: bool,
    idle: bool,
}

fn snapshot(s: &Schedule, st: &Settings, now: Instant) -> Now {
    let r = &st.rules;
    let kind = s.pending.map(|(k, _)| k).or(s.on_break.map(|(k, ..)| k)).unwrap_or_else(|| s.next_kind(r, now));
    let span = match kind {
        Kind::Short => r.every,
        Kind::Long => r.long_every.unwrap_or(r.every),
    };
    let (left, frac, state) = if let Some((_, start, end)) = s.on_break {
        let len = end.saturating_duration_since(start).max(SECOND);
        let left = end.saturating_duration_since(now);
        (left, 1.0 - left.as_secs_f32() / len.as_secs_f32(), "on-break")
    } else if s.paused_until.is_some() {
        (s.until_due(r, now), 1.0 - s.until_due(r, now).as_secs_f32() / span.as_secs_f32().max(1.0), "paused")
    } else if s.pending.is_some() {
        (Duration::ZERO, 1.0, "due")
    } else {
        let until = s.until_due(r, now);
        let left = s.snoozed_until.map_or(until, |u| until.max(u.saturating_duration_since(now)));
        let state = if s.idle() {
            "away"
        } else if s.snoozed_until.is_some_and(|u| u > now) {
            "snoozed"
        } else {
            "working"
        };
        (left, 1.0 - until.as_secs_f32() / span.as_secs_f32().max(1.0), state)
    };
    Now {
        state,
        kind,
        left,
        frac: frac.clamp(0.0, 1.0),
        long_in: s.until_long(r, now),
        paused_until: s.paused_until,
        on_break: s.on_break.is_some(),
        due: s.pending.is_some(),
        idle: s.idle(),
    }
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Short => "short",
        Kind::Long => "long",
    }
}

fn status_json(cx: &mut App) -> serde_json::Value {
    let st = current_settings(cx);
    let m = model(cx);
    let b = m.read(cx);
    let (taken, skipped) = b.tally.on(&today());
    let Some(s) = b.schedule.as_ref() else { return json!({ "enabled": false, "today": { "taken": taken, "skipped": skipped } }) };
    let now = Instant::now();
    let n = snapshot(s, &st, now);
    let (active_short, active_long) = s.active(now);
    json!({
        "enabled": true,
        "state": n.state,
        "next": kind_name(n.kind),
        "next_in_s": n.left.as_secs(),
        "long_in_s": n.long_in.map(|d| d.as_secs()),
        "active_s": active_short.as_secs(),
        "active_since_long_s": active_long.as_secs(),
        "paused_for_s": s.paused_until.map(|u| u.saturating_duration_since(now).as_secs()),
        "snoozed_for_s": s.snoozed_until.map(|u| u.saturating_duration_since(now).as_secs()).filter(|s| *s > 0),
        "idle_tracking": crate::powerctl::idle::manager(cx).and_then(|m| m.read(cx).available()).unwrap_or(false),
        "style": if st.style == Style::Overlay { "overlay" } else { "notification" },
        "every_min": st.rules.every.as_secs() / SECS_PER_MIN,
        "short_s": st.rules.short.as_secs(),
        "long_every_min": st.rules.long_every.map(|d| d.as_secs() / SECS_PER_MIN),
        "long_min": st.rules.long.as_secs() / SECS_PER_MIN,
        "today": { "taken": taken, "skipped": skipped },
    })
}

/// `HH:MM` local wall time `d` from now.
fn wall_time_in(d: Duration) -> String {
    let at = chrono::Local::now() + chrono::Duration::from_std(d).unwrap_or_default();
    at.format("%H:%M").to_string()
}

// ── Bar chip ─────────────────────────────────────────────────────────────

fn bar_view(palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let st = current_settings(cx);
    let now = Instant::now();
    let m = model(cx);
    let Some(n) = m.read(cx).schedule.as_ref().map(|s| snapshot(s, &st, now)) else { return div().into_any_element() };
    let chip = div().flex().items_center().gap(px(BAR_GAP_PX)).font_features(tabular_nums());
    if n.paused_until.is_some() {
        return chip.child(icon("pause", crate::style::bar_icon(BAR_ICON_PX), palette.on_surface_variant)).into_any_element();
    }
    if n.on_break || n.due {
        let tint = readable(if n.on_break { palette.success_emerald } else { palette.warning_amber }, palette);
        return chip.child(icon("eye", crate::style::bar_icon(BAR_ICON_PX), tint)).into_any_element();
    }
    // The label changes once a minute (and the ring turns amber for the
    // last one): redraw then, not before. Away, the clock stands still.
    if !n.idle {
        let into_minute = Duration::from_secs(n.left.as_secs() % SECS_PER_MIN) + Duration::from_millis(u64::from(n.left.subsec_millis()));
        redraw_in(if into_minute.is_zero() { LAST_MINUTE } else { into_minute } + EXPIRY_MARGIN, window, cx);
    }
    let last = n.left <= LAST_MINUTE;
    let color = readable(if last { palette.warning_amber } else { palette.primary }, palette);
    let mins = n.left.as_secs().div_ceil(SECS_PER_MIN).max(1);
    chip.child(ring(n.frac, crate::style::bar_icon(BAR_RING_PX), BAR_RING_STROKE_PX, color, palette.on_surface.with_alpha(RING_TRACK_ALPHA)))
        .child(div().font_weight(FontWeight::SEMIBOLD).text_color(if last { color.hsla() } else { palette.on_surface.hsla() }).child(tf("{n} min", &[("n", &mins)])))
        .into_any_element()
}

// ── Panel ────────────────────────────────────────────────────────────────

fn stat(label: &'static str, value: String, palette: &MandaraPalette) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(STAT_LINE_GAP_PX))
        .child(caption(label, palette))
        .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).font_features(tabular_nums()).child(value))
        .into_any_element()
}

fn panel_height() -> f32 {
    PAD * 2.0 + HEADER_H + GAP + HERO_H + GAP + PILL_H_PX
}

fn panel_view(p: &PanelCx, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let st = current_settings(cx);
    let now = Instant::now();
    let m = model(cx);
    let (snap, (taken, skipped)) = {
        let b = m.read(cx);
        (b.schedule.as_ref().map(|s| snapshot(s, &st, now)), b.tally.on(&today()))
    };
    let accent = readable(palette.primary, palette);
    let rhythm = match st.rules.long_every {
        Some(le) => tf("Every {every} min · {short} s · long break every {long} min", &[("every", &(st.rules.every.as_secs() / SECS_PER_MIN)), ("short", &st.rules.short.as_secs()), ("long", &(le.as_secs() / SECS_PER_MIN))]),
        None => tf("Every {every} min · {short} s", &[("every", &(st.rules.every.as_secs() / SECS_PER_MIN)), ("short", &st.rules.short.as_secs())]),
    };
    let today_chip = chip("check", tf("{taken} taken · {skipped} skipped", &[("taken", &taken), ("skipped", &skipped)]), accent, palette).into_any_element();
    let head = header("eye", accent, t("Eye breaks"), rhythm, palette, Some(today_chip));
    let Some(n) = snap else {
        return div().size_full().p(px(PAD)).child(head).into_any_element();
    };
    if p.progress() > 0.0 && !n.idle {
        redraw_in(SECOND, window, cx);
    }
    let display = fonts(cx).display.clone();
    let ring_color = match n.state {
        "on-break" => palette.success_emerald,
        "due" => palette.warning_amber,
        _ if n.left <= LAST_MINUTE => palette.warning_amber,
        _ => palette.primary,
    };
    let ring_color = readable(ring_color, palette);
    let (clock, under) = match n.state {
        "paused" => (t("Paused").to_string(), n.paused_until.map(|u| tf("until {time}", &[("time", &wall_time_in(u.saturating_duration_since(now)))])).unwrap_or_default()),
        "due" => (t("Now").to_string(), t("Time for a break").to_string()),
        "on-break" => (format_duration(n.left.as_secs()), if n.kind == Kind::Long { t("Long break").to_string() } else { t("Look far away").to_string() }),
        "away" => (format_duration(n.left.as_secs()), t("Away · the clock waits").to_string()),
        _ => (format_duration(n.left.as_secs()), if n.kind == Kind::Long { t("until a long break").to_string() } else { t("until a short break").to_string() }),
    };
    let inner = div()
        .flex()
        .flex_col()
        .items_center()
        .child(div().font_family(display).font_features(tabular_nums()).text_size(px(HERO_CLOCK_PX)).font_weight(FontWeight::MEDIUM).child(clock))
        .child(caption(under, palette).text_center())
        .into_any_element();
    let dial = dot_ring(n.frac, HERO_RING_PX, HERO_DOTS, HERO_DOT_PX, ring_color, partner(ring_color).with_lightness(ring_color.lightness()), wash(WASH_IDLE_BAR, palette), inner);
    let long = match n.long_in {
        Some(d) if n.kind != Kind::Long => duration_short(i64::try_from(d.as_secs()).unwrap_or(0)),
        Some(_) => t("Next").to_string(),
        None => t("Off").to_string(),
    };
    let hero = glass_card(palette)
        .h(px(HERO_H))
        .p(px(HERO_PAD_PX))
        .flex()
        .items_center()
        .gap(px(tokens::LG))
        .child(dial)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(HERO_STAT_GAP_PX))
                .child(stat(t("NEXT LONG BREAK"), long, palette))
                .child(stat(t("BREAKS TODAY"), tf("{taken} taken", &[("taken", &taken)]), palette))
                .child(stat(t("SKIPPED TODAY"), skipped.to_string(), palette)),
        );
    let paused = n.paused_until.is_some();
    let actions = div()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(tokens::SM))
        .when(!n.on_break, |el| {
            el.child(pill_button("breaks-now", Some("eye"), t("Take a break"), accent, palette, |_, cx| {
                let _ = take_now(None, cx);
            }))
        })
        // A break on screen or a reminder out can be skipped.
        .when(n.on_break || n.due, |el| {
            el.child(pill_button("breaks-skip", Some("skip"), t("Skip"), accent, palette, |_, cx| {
                let _ = skip(cx);
            }))
        })
        .child(pill_button("breaks-snooze", Some("clock"), tf("Snooze {n} min", &[("n", &SNOOZE_MIN)]), accent, palette, |_, cx| {
            let _ = snooze(SNOOZE_MIN, cx);
        }))
        // Three buttons fit: with a reminder out, pausing gives way to Skip.
        .when(!n.due, |el| {
            el.child(if paused {
                pill_button("breaks-resume", Some("play"), t("Resume"), accent, palette, |_, cx| {
                    let _ = act(cx, |s, _, now| s.resume(now));
                })
            } else {
                pill_button("breaks-pause", Some("pause"), t("Pause 1 h"), accent, palette, |_, cx| {
                    let _ = pause(PAUSE_MIN, cx);
                })
            })
        });
    div()
        .size_full()
        .p(px(PAD))
        .flex()
        .flex_col()
        .gap(px(GAP))
        .child(reveal(p, 0, head))
        .child(reveal(p, 1, hero))
        .child(reveal(p, 2, actions))
        .into_any_element()
}

// ── Overlay ──────────────────────────────────────────────────────────────

#[derive(Default)]
struct Overlay {
    windows: Vec<WindowHandle<BreakOverlay>>,
}
impl Global for Overlay {}

struct BreakOverlay {
    kind: Kind,
    started: Instant,
    length: Duration,
}

impl Render for BreakOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = mandara_core::frame_palette(window, cx);
        let now = Instant::now();
        let shown = now.duration_since(self.started);
        let fade = (shown.as_secs_f32() / FADE_IN.as_secs_f32()).clamp(0.0, 1.0);
        let left = self.length.saturating_sub(shown);
        if fade < 1.0 {
            window.request_animation_frame();
        } else {
            // The countdown changes once a second.
            redraw_in(Duration::from_millis(u64::from(left.subsec_millis())) + EXPIRY_MARGIN, window, cx);
        }
        let eased = fade * fade * (3.0 - 2.0 * fade);
        let veil = palette.surface_dim.mix(Color::hex(VEIL_BASE_HEX), VEIL_DEPTH).with_alpha(VEIL_ALPHA * eased);
        let accent = readable(palette.success_emerald, &palette);
        let frac = 1.0 - left.as_secs_f32() / self.length.as_secs_f32().max(1.0);
        let f = fonts(cx);
        let (title, hint) = match self.kind {
            Kind::Short => (t("Look at something 6 metres away"), t("Let your eyes relax: blink slowly and soften your gaze")),
            Kind::Long => (t("Time to stand up and stretch"), t("Walk around, drink some water and rest your eyes")),
        };
        let inner = div()
            .font_family(f.display.clone())
            .font_features(tabular_nums())
            .text_size(px(OVERLAY_CLOCK_PX))
            .font_weight(FontWeight::MEDIUM)
            .child(format_duration(left.as_secs() + u64::from(left.subsec_millis() > 0)))
            .into_any_element();
        let card = glass_card(&palette)
            .w(px(OVERLAY_CARD_W_PX))
            .p(px(OVERLAY_CARD_PAD_PX))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(OVERLAY_GAP_PX))
            .opacity(eased)
            .text_color(palette.on_surface.hsla())
            .child(dot_ring(frac, OVERLAY_RING_PX, OVERLAY_DOTS, OVERLAY_DOT_PX, accent, partner(accent).with_lightness(accent.lightness()), wash(WASH_IDLE_BAR, &palette), inner))
            .child(div().text_size(px(OVERLAY_TITLE_PX)).font_weight(FontWeight::SEMIBOLD).text_center().child(title))
            .child(div().text_size(px(text::BODY)).text_color(palette.on_surface_variant.hsla()).text_center().child(hint))
            .child(
                div()
                    .flex()
                    .gap(px(tokens::SM))
                    .child(pill_button("break-overlay-skip", Some("skip"), t("Skip"), palette.on_surface_variant, &palette, |_, cx| {
                        let _ = skip(cx);
                    }))
                    .child(pill_button("break-overlay-snooze", Some("clock"), tf("Snooze {n} min", &[("n", &SNOOZE_MIN)]), readable(palette.primary, &palette), &palette, |_, cx| {
                        let _ = snooze(SNOOZE_MIN, cx);
                    })),
            );
        div().size_full().bg(veil.hsla()).font_family(f.ui.clone()).flex().items_center().justify_center().child(card)
    }
}

/// Dims every output with the break countdown (no keyboard focus is taken).
fn overlay_open(kind: Kind, length: Duration, cx: &mut App) {
    overlay_close(cx);
    let displays = cx.displays();
    let mut paired: Vec<(Option<gpui::DisplayId>, Option<mandara_core::OutputGeometry>)> = crate::outputs::paired(cx).into_iter().map(|(id, g)| (Some(id), g)).collect();
    if paired.is_empty() {
        paired.push((None, None));
    }
    let started = Instant::now();
    let mut windows = Vec::with_capacity(paired.len());
    for (display, geo) in paired {
        let (w, h) = geo
            .map(|g| (g.width as f32, g.height as f32))
            .or_else(|| displays.iter().find(|d| Some(d.id()) == display).map(|d| (f32::from(d.bounds().size.width), f32::from(d.bounds().size.height))))
            .unwrap_or(FALLBACK_OUTPUT);
        let spec = LayerSurfaceSpec::overlay(SurfaceRole::Osd, size(px(w), px(h)), display);
        match cx.open_window(spec.window_options(), move |_, cx| cx.new(|_| BreakOverlay { kind, started, length })) {
            Ok(handle) => windows.push(handle),
            Err(e) => tracing::warn!("eye break overlay: {e:#}"),
        }
    }
    cx.set_global(Overlay { windows });
}

fn overlay_close(cx: &mut App) {
    let Some(o) = cx.try_global::<Overlay>() else { return };
    for h in o.windows.clone() {
        let _ = h.update(cx, |_, window, _| window.remove_window());
    }
    cx.set_global(Overlay::default());
}

// ── Registration ─────────────────────────────────────────────────────────

fn postpone_arg(a: &mandara_plugins::Args, default: u64) -> Result<u64, String> {
    match a.get(0) {
        None => Ok(default),
        Some(_) => Ok(u64::try_from(a.int(0)?.clamp(1, MAX_POSTPONE_MIN)).unwrap_or(default)),
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Eye breaks"), tk("Gentle 20-20-20 reminders to rest your eyes"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let m = model(cx);
        let bar = fn_view(None, bar_view, window, cx);
        follow(&bar, &m, cx);
        let p = panel_cx.clone();
        let panel = fn_view(None, move |palette, window, cx| panel_view(&p, palette, window, cx), window, cx);
        follow(&panel, &m, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: Rc::new(Cell::new(size(px(PANEL_W), px(panel_height())))) }), ..PluginInstance::empty() }
    });
    r.command(ID, CommandSpec::new("breaks status", tk("Time to the next break, the next long break, today's counts and the rhythm (JSON)"), |_, cx| Ok(status_json(cx))).example("breaks status"));
    r.command(
        ID,
        CommandSpec::new("breaks now", tk("Take a break now (shows the countdown overlay)"), |a, cx| {
            let kind = match a.get(0) {
                Some("long") => Some(Kind::Long),
                Some(_) => Some(Kind::Short),
                None => None,
            };
            take_now(kind, cx)?;
            Ok(status_json(cx))
        })
        .arg(ArgSpec::optional("kind", ArgKind::Enum, tk("short or long (default: the one due, else short)")).choices(&["short", "long"]))
        .example("breaks now")
        .example("breaks now long"),
    );
    r.command(
        ID,
        CommandSpec::new("breaks skip", tk("Skip the current or next break; the clock starts over"), |_, cx| {
            skip(cx)?;
            Ok(status_json(cx))
        })
        .example("breaks skip"),
    );
    r.command(
        ID,
        CommandSpec::new("breaks snooze", tk("Postpone the break by N minutes (default 5)"), |a, cx| {
            snooze(postpone_arg(a, SNOOZE_MIN)?, cx)?;
            Ok(status_json(cx))
        })
        .arg(ArgSpec::optional("minutes", ArgKind::Integer, tk("how long, in minutes")))
        .example("breaks snooze 10"),
    );
    r.command(
        ID,
        CommandSpec::new("breaks pause", tk("Stop reminders and the clock for N minutes (default 60)"), |a, cx| {
            pause(postpone_arg(a, PAUSE_MIN)?, cx)?;
            Ok(status_json(cx))
        })
        .arg(ArgSpec::optional("minutes", ArgKind::Integer, tk("how long, in minutes")))
        .example("breaks pause 90"),
    );
    r.command(
        ID,
        CommandSpec::new("breaks resume", tk("Resume reminders after a pause"), |_, cx| {
            act(cx, |s, _, now| s.resume(now))?;
            Ok(status_json(cx))
        })
        .example("breaks resume"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_follow_the_config_within_bounds() {
        let mut cfg = ShellConfig::default();
        let s = settings(&cfg);
        assert_eq!(s.rules.every, Duration::from_secs(20 * 60));
        assert_eq!(s.rules.short, Duration::from_secs(20));
        assert_eq!(s.rules.long_every, Some(Duration::from_secs(60 * 60)));
        assert_eq!((s.style, s.skip_fullscreen), (Style::Notification, true));
        let t = cfg.plugins.settings.entry(ID.into()).or_default();
        t.insert("long_every_min".into(), toml::Value::Integer(0));
        t.insert("every_min".into(), toml::Value::Integer(0));
        t.insert("style".into(), toml::Value::String("overlay".into()));
        let s = settings(&cfg);
        assert_eq!(s.rules.long_every, None, "0 switches long breaks off");
        assert_eq!(s.rules.every, Duration::from_secs(60), "at least a minute");
        assert_eq!(s.style, Style::Overlay);
    }

    #[test]
    fn snapshot_reads_the_schedule() {
        let st = settings(&ShellConfig::default());
        let t0 = Instant::now();
        let mut s = Schedule::new(t0);
        let n = snapshot(&s, &st, t0 + Duration::from_secs(5 * 60));
        assert_eq!((n.state, n.kind, n.left), ("working", Kind::Short, Duration::from_secs(15 * 60)));
        assert!((n.frac - 0.25).abs() < 1e-3);
        s.pause(Duration::from_secs(3600), t0);
        assert_eq!(snapshot(&s, &st, t0).state, "paused");
        s.resume(t0);
        s.take(&st.rules, None, t0);
        let n = snapshot(&s, &st, t0 + Duration::from_secs(5));
        assert_eq!((n.state, n.left), ("on-break", Duration::from_secs(15)));
    }
}
