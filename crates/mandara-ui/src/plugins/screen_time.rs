//! Screen time (built-in `screen-time` plugin, opt-in): how long each app
//! had focus, per day, with a 7-day chart, today's top apps and gentle
//! reminders for daily limits.
//!
//! Focus comes from the window list (compositor-neutral foreign-toplevel
//! events, the compositor's own focus events as a fallback); time stops
//! counting after `[screen_time] idle_minutes` without input
//! (`ext-idle-notify`). Everything stays on this computer, in
//! `~/.local/state/mandara/screen-time.json`; `track = false` stops
//! recording, `screentime reset` forgets.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use chrono::{Datelike, Local, NaiveDate};
use gpui::{AnyElement, App, AppContext as _, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, Window, div, prelude::FluentBuilder as _, px, size};
use mandara_config::workflow::ScreenTimeConfig;
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::toplevels::ToplevelCommand;
use mandara_theme::MandaraPalette;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::fn_view;
use super::panel_fx::{ANGLE_DOWN_DEG, HEADER_H, empty_state, header, meter, partner, readable, section, wash, WASH_FAINT};
use crate::fonts::tabular_nums;
use crate::icons::icon;
use crate::kit::{reveal, text};
use crate::state::{config, update_config};

pub const ID: &str = "screen-time";

/// Time is added to the focused app this often.
const TICK: Duration = Duration::from_secs(10);
/// The store is written at most this often.
const SAVE_EVERY: Duration = Duration::from_secs(60);
/// Elapsed time beyond this between ticks is treated as a suspend gap.
const MAX_STEP: Duration = Duration::from_secs(60);
/// Seconds per minute / hour.
const SECS_PER_MIN: u64 = 60;
const SECS_PER_HOUR: u64 = 3600;
/// Milliseconds per minute (idle timeout).
const MS_PER_MIN: u32 = 60_000;
/// Days in the weekly chart.
const WEEK: usize = 7;
/// Apps listed in the panel.
const TOP_APPS: usize = 6;
/// Panel geometry.
const PANEL_W: f32 = 400.0;
const PAD: f32 = 14.0;
const GAP: f32 = 12.0;
const HERO_H: f32 = 64.0;
const CHART_H: f32 = 118.0;
const BAR_W: f32 = 26.0;
const BAR_RADIUS: f32 = 8.0;
const DAY_LABEL_H: f32 = 16.0;
const APP_ROW_H: f32 = 40.0;
const APP_ICON_PX: f32 = 24.0;
const METER_H: f32 = 5.0;
const EMPTY_H: f32 = 150.0;
const FOOTER_H: f32 = 22.0;
/// Bar chip glyph.
const BAR_ICON_PX: f32 = 15.0;
/// Shortest bar drawn for a day with any time, logical px.
const MIN_BAR_PX: f32 = 3.0;

/// Seconds of focus per app, per day (`YYYY-MM-DD`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub days: BTreeMap<String, BTreeMap<String, u64>>,
}

impl Store {
    pub fn add(&mut self, day: NaiveDate, app: &str, secs: u64) {
        if secs > 0 && !app.is_empty() {
            *self.days.entry(day.to_string()).or_default().entry(app.to_string()).or_default() += secs;
        }
    }

    pub fn day(&self, day: NaiveDate) -> Option<&BTreeMap<String, u64>> {
        self.days.get(&day.to_string())
    }

    pub fn total(&self, day: NaiveDate) -> u64 {
        self.day(day).map(|d| d.values().sum()).unwrap_or(0)
    }

    /// Apps of `day`, longest first.
    pub fn top(&self, day: NaiveDate) -> Vec<(String, u64)> {
        let mut v: Vec<(String, u64)> = self.day(day).map(|d| d.iter().map(|(k, v)| (k.clone(), *v)).collect()).unwrap_or_default();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }

    /// Totals of the `n` days ending at `today`, oldest first.
    pub fn last_days(&self, today: NaiveDate, n: usize) -> Vec<(NaiveDate, u64)> {
        (0..n).rev().filter_map(|back| today.checked_sub_days(chrono::Days::new(back as u64))).map(|d| (d, self.total(d))).collect()
    }

    /// Drops days older than `keep` days before `today`.
    pub fn prune(&mut self, today: NaiveDate, keep: u32) {
        let Some(cutoff) = today.checked_sub_days(chrono::Days::new(u64::from(keep))) else { return };
        self.days.retain(|d, _| NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok_and(|d| d > cutoff));
    }
}

/// `2h 05m`, `14m`, `<1m`.
pub fn hm(secs: u64) -> String {
    let (h, m) = (secs / SECS_PER_HOUR, (secs % SECS_PER_HOUR) / SECS_PER_MIN);
    match (h, m) {
        (0, 0) if secs > 0 => t("<1m").to_string(),
        (0, m) => tf("{m}m", &[("m", &m)]),
        (h, m) => tf("{h}h {m}m", &[("h", &h), ("m", &format!("{m:02}"))]),
    }
}

fn store_path() -> PathBuf {
    dirs::state_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/state")).join("mandara").join("screen-time.json")
}

fn load() -> Store {
    std::fs::read_to_string(store_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(store: &Store) {
    let path = store_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec(store) {
        let _ = mandara_config::write_atomic(&path, &json);
    }
}

/// Live tracker state (an entity: panel and bar re-render on changes).
pub struct Tracker {
    pub store: Store,
    /// App with focus and since when (`None` while idle / not tracking).
    current: Option<(String, Instant)>,
    idle: bool,
    dirty: bool,
    saved_at: Instant,
    /// Limits already announced today (`date/app` or `date/*`).
    notified: BTreeSet<String>,
}

impl Tracker {
    /// Books the running interval to the current app.
    fn flush(&mut self, cfg: &ScreenTimeConfig) {
        if let Some((app, since)) = self.current.as_mut() {
            let step = since.elapsed();
            *since = Instant::now();
            if cfg.track && !self.idle && step <= MAX_STEP {
                self.store.add(Local::now().date_naive(), app, step.as_secs());
                self.dirty = true;
            }
        }
    }

    fn focus(&mut self, app: Option<String>, cfg: &ScreenTimeConfig) {
        self.flush(cfg);
        let app = app.filter(|a| !a.is_empty() && !crate::windows::is_shell(a) && !cfg.exclude.iter().any(|x| x.eq_ignore_ascii_case(a)));
        self.current = app.map(|a| (a, Instant::now()));
    }
}

struct GlobalTracker(Entity<Tracker>);
impl gpui::Global for GlobalTracker {}

fn tracker(cx: &App) -> Option<Entity<Tracker>> {
    cx.try_global::<GlobalTracker>().map(|g| g.0.clone())
}

fn notify(summary: String, body: String, cx: &mut App) {
    crate::plugins::notifications::send(cx, mandara_services::NotificationCommand::Post { app: t("Screen time").into(), icon: "timer".into(), summary, body });
}

/// Limits reached and not yet announced today: `(summary, body)` each,
/// marked as announced in `notified`.
pub fn due_limits(store: &Store, notified: &mut BTreeSet<String>, cfg: &ScreenTimeConfig, today: NaiveDate) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if cfg.daily_limit_min > 0 && store.total(today) >= u64::from(cfg.daily_limit_min) * SECS_PER_MIN && notified.insert(format!("{today}/*")) {
        out.push((t("Daily screen time reached").to_string(), tf("{time} today", &[("time", &hm(store.total(today)))])));
    }
    for (app, limit) in &cfg.app_limits {
        let used = store.day(today).and_then(|d| d.iter().find(|(k, _)| k.eq_ignore_ascii_case(app)).map(|(_, v)| *v)).unwrap_or(0);
        if *limit > 0 && used >= u64::from(*limit) * SECS_PER_MIN && notified.insert(format!("{today}/{app}")) {
            let name = super::window::app_name(app);
            out.push((tf("{app}: daily limit reached", &[("app", &name)]), tf("{time} today", &[("time", &hm(used))])));
        }
    }
    out
}

/// Starts tracking (only while the plugin is enabled) and keeps going.
pub fn init(cx: &mut App) {
    let entity = cx.new(|_| Tracker { store: load(), current: None, idle: false, dirty: false, saved_at: Instant::now(), notified: BTreeSet::new() });
    cx.set_global(GlobalTracker(entity.clone()));
    let windows = crate::windows::init(cx);
    // Idle timeout from the settings.
    let apply_idle = |cx: &mut App| {
        let cfg = config(cx).read(cx).snapshot();
        let enabled = cx.global::<PluginRegistry>().is_enabled(ID, &cfg.plugins) && cfg.screen_time.track;
        let ms = if enabled { cfg.screen_time.idle_minutes.saturating_mul(MS_PER_MIN) } else { 0 };
        if let Some(w) = crate::windows::windows(cx) {
            w.read(cx).send(ToplevelCommand::IdleTimeout(ms));
        }
    };
    apply_idle(cx);
    let mut last = config(cx).read(cx).get().screen_time.idle_minutes;
    let mut last_track = config(cx).read(cx).get().screen_time.track;
    let mut last_plugins = config(cx).read(cx).get().plugins.clone();
    let mut last_exclude = config(cx).read(cx).get().screen_time.exclude.clone();
    let excluding = entity.clone();
    cx.observe(&config(cx), move |store, cx| {
        let c = store.read(cx).snapshot();
        // Turning tracking on or off also starts / stops the idle timeout.
        if c.screen_time.idle_minutes != last || c.screen_time.track != last_track || c.plugins != last_plugins {
            last = c.screen_time.idle_minutes;
            last_track = c.screen_time.track;
            last_plugins = c.plugins.clone();
            apply_idle(cx);
            sync_ticker(cx);
        }
        // An app excluded while focused stops counting right away.
        if c.screen_time.exclude != last_exclude {
            last_exclude = c.screen_time.exclude.clone();
            excluding.update(cx, |tr, _| {
                let current = tr.current.as_ref().map(|(a, _)| a.clone());
                tr.focus(current, &c.screen_time);
            });
        }
    })
    .detach();
    let t = entity.clone();
    cx.observe(&windows, move |w, cx| {
        let cfg = config(cx).read(cx).snapshot();
        let enabled = cx.global::<PluginRegistry>().is_enabled(ID, &cfg.plugins);
        let state = w.read(cx).state.clone();
        let app = if state.available { state.active().map(|a| a.app_id.clone()) } else { crate::hub::hub(cx).read(cx).focused.as_ref().map(|c| c.app_id.clone()) };
        t.update(cx, |tr, cx| {
            let idle_changed = tr.idle != state.idle;
            if idle_changed {
                tr.flush(&cfg.screen_time);
                tr.idle = state.idle;
            }
            let current = tr.current.as_ref().map(|(a, _)| a.clone());
            let next = if enabled && cfg.screen_time.track { app } else { None };
            if current != next {
                tr.focus(next, &cfg.screen_time);
                cx.notify();
            }
        });
    })
    .detach();
    sync_ticker(cx);
}

thread_local! {
    /// The periodic booking / limits / saving loop; runs only while the
    /// plugin is on (dropping the task stops it).
    static TICKER: RefCell<Option<gpui::Task<()>>> = const { RefCell::new(None) };
}

/// Starts the periodic loop when the plugin is on; stops it (after booking
/// and saving what was recorded) when it is turned off or removed.
fn sync_ticker(cx: &mut App) {
    let cfg = config(cx).read(cx).snapshot();
    let on = cx.global::<PluginRegistry>().is_enabled(ID, &cfg.plugins);
    let running = TICKER.with(|t| t.borrow().is_some());
    if on && !running {
        let task = cx.spawn(async move |cx| tick_loop(cx).await);
        TICKER.with(|t| *t.borrow_mut() = Some(task));
    } else if !on && running {
        TICKER.with(|t| t.borrow_mut().take());
        let Some(t) = tracker(cx) else { return };
        let pending = t.update(cx, |tr, _| {
            tr.focus(None, &cfg.screen_time);
            tr.flush(&cfg.screen_time);
            std::mem::take(&mut tr.dirty).then(|| {
                tr.saved_at = Instant::now();
                tr.store.clone()
            })
        });
        if let Some(store) = pending {
            cx.background_executor().spawn(async move { save(&store) }).detach();
        }
    }
}

/// Periodic booking, limits and saving.
async fn tick_loop(cx: &mut gpui::AsyncApp) {
    loop {
        cx.background_executor().timer(TICK).await;
        let done = cx.update(|cx| {
            let cfg = config(cx).read(cx).snapshot();
            let Some(t) = tracker(cx) else { return true };
            let mut save_now = None;
            t.update(cx, |tr, cx| {
                tr.flush(&cfg.screen_time);
                if tr.dirty {
                    cx.notify();
                }
                if tr.dirty && tr.saved_at.elapsed() >= SAVE_EVERY {
                    tr.store.prune(Local::now().date_naive(), cfg.screen_time.retention_days);
                    tr.dirty = false;
                    tr.saved_at = Instant::now();
                    save_now = Some(tr.store.clone());
                }
            });
            if cfg.screen_time.track {
                let today = Local::now().date_naive();
                let due = t.update(cx, |tr, _| due_limits(&tr.store, &mut tr.notified, &cfg.screen_time, today));
                for (summary, body) in due {
                    notify(summary, body, cx);
                }
            }
            if let Some(store) = save_now {
                cx.background_executor().spawn(async move { save(&store) }).detach();
            }
            false
        });
        if done {
            break;
        }
    }
}

/// The tracked store with the running interval included.
fn snapshot(cx: &App) -> Store {
    let Some(t) = tracker(cx) else { return load() };
    let tr = t.read(cx);
    let mut s = tr.store.clone();
    if let Some((app, since)) = &tr.current
        && !tr.idle
        && since.elapsed() <= MAX_STEP
    {
        s.add(Local::now().date_naive(), app, since.elapsed().as_secs());
    }
    s
}

// ── Panel ────────────────────────────────────────────────────────────────

fn weekday_short(d: NaiveDate) -> &'static str {
    match d.weekday() {
        chrono::Weekday::Mon => tk("Mon"),
        chrono::Weekday::Tue => tk("Tue"),
        chrono::Weekday::Wed => tk("Wed"),
        chrono::Weekday::Thu => tk("Thu"),
        chrono::Weekday::Fri => tk("Fri"),
        chrono::Weekday::Sat => tk("Sat"),
        chrono::Weekday::Sun => tk("Sun"),
    }
}

fn week_chart(days: &[(NaiveDate, u64)], today: NaiveDate, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let max = days.iter().map(|(_, s)| *s).max().unwrap_or(0).max(SECS_PER_HOUR);
    let bars = days.iter().map(|(d, secs)| {
        let frac = *secs as f32 / max as f32;
        let h = if *secs == 0 { 0.0 } else { (frac * (CHART_H - DAY_LABEL_H - 18.0)).max(MIN_BAR_PX) };
        let is_today = *d == today;
        let (from, to) = if is_today { (partner(accent), accent) } else { (accent.with_alpha(0.45), accent.with_alpha(0.3)) };
        div()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_end()
            .gap(px(4.0))
            .child(div().text_size(px(text::LABEL - 1.0)).font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).when(*secs == 0 || !is_today, |el| el.opacity(if is_today { 1.0 } else { 0.0 })).child(hm(*secs)))
            .child(div().w(px(BAR_W)).h(px(h)).rounded(px(crate::style::radius(BAR_RADIUS))).bg(oklab_gradient(ANGLE_DOWN_DEG, from, to)))
            .child(div().h(px(DAY_LABEL_H)).text_size(px(text::LABEL)).when(is_today, |el| el.font_weight(FontWeight::SEMIBOLD).text_color(accent.hsla())).when(!is_today, |el| el.text_color(palette.on_surface_variant.hsla())).child(mandara_i18n::tr(weekday_short(*d))))
    });
    div().h(px(CHART_H)).p(px(8.0)).rounded(px(crate::style::radius(14.0))).bg(wash(WASH_FAINT, palette).hsla()).flex().items_end().children(bars).into_any_element()
}

fn panel_view(p: mandara_plugins::PanelCx, size_cell: Rc<std::cell::Cell<gpui::Size<gpui::Pixels>>>) -> impl Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement {
    move |palette, _, cx| {
        let cfg = config(cx).read(cx).snapshot();
        let store = snapshot(cx);
        let today = Local::now().date_naive();
        let total = store.total(today);
        let top: Vec<(String, u64)> = store.top(today).into_iter().take(TOP_APPS).collect();
        let accent = readable(palette.primary, palette);
        let tracking = cfg.screen_time.track;
        let subtitle: SharedString = if tracking { t("Today · stays on this computer").into() } else { t("Tracking is paused").into() };
        let toggle = super::panel_fx::pill_button("st-track", Some(if tracking { "pause" } else { "play" }), if tracking { t("Pause") } else { t("Resume") }, accent, palette, move |_, cx| {
            update_config(cx, |c| c.screen_time.track = !tracking);
        });
        let limit = cfg.screen_time.daily_limit_min;
        let hero = div()
            .h(px(HERO_H))
            .flex()
            .items_center()
            .gap(px(14.0))
            .child(div().font_features(tabular_nums()).text_size(px(text::HEADLINE + 4.0)).font_weight(FontWeight::SEMIBOLD).child(hm(total)))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(if limit > 0 { tf("of {limit} daily limit", &[("limit", &hm(u64::from(limit) * SECS_PER_MIN))]) } else { t("screen time today").to_string() }))
                    .when(limit > 0, |el| el.child(meter(total as f32 / (u64::from(limit) * SECS_PER_MIN) as f32, METER_H, accent, partner(accent), palette))),
            );
        let max = top.first().map(|(_, s)| *s).unwrap_or(1).max(1);
        let rows = top.iter().map(|(app, secs)| {
            div()
                .h(px(APP_ROW_H))
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(super::workspaces::app_icon(app, APP_ICON_PX, palette.on_surface, false, palette))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(div().flex().justify_between().child(div().truncate().text_size(px(text::BODY_S)).child(super::window::app_name(app))).child(div().flex_none().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(hm(*secs))))
                        .child(meter(*secs as f32 / max as f32, METER_H, accent.with_alpha(0.9), partner(accent), palette)),
                )
        });
        let days = store.last_days(today, WEEK);
        let body = if total == 0 && days.iter().all(|(_, s)| *s == 0) {
            div().child(empty_state("timer", if tracking { t("Nothing yet today") } else { t("Tracking is paused") }, t("Time spent in each app shows up here"), EMPTY_H, palette)).into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(GAP))
                .child(hero)
                .child(section(t("LAST 7 DAYS"), palette, None))
                .child(week_chart(&days, today, palette))
                .when(!top.is_empty(), |el| el.child(section(t("TOP APPS TODAY"), palette, None)).child(div().flex().flex_col().children(rows)))
                .into_any_element()
        };
        let content_h = if total == 0 && days.iter().all(|(_, s)| *s == 0) {
            EMPTY_H
        } else {
            HERO_H + GAP + super::panel_fx::SECTION_H_PX + GAP + CHART_H + if top.is_empty() { 0.0 } else { GAP + super::panel_fx::SECTION_H_PX + GAP + top.len() as f32 * APP_ROW_H }
        };
        size_cell.set(size(px(PANEL_W), px(PAD * 2.0 + HEADER_H + GAP + content_h + GAP + FOOTER_H)));
        div()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .child(reveal(&p, 0, header("timer", accent, t("Screen time"), subtitle, palette, Some(toggle.into_any_element()))))
            .child(div().h(px(GAP)))
            .child(reveal(&p, 1, body))
            .child(div().h(px(GAP)))
            .child(div().h(px(FOOTER_H)).flex().items_center().gap(px(6.0)).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(icon("shield", 12.0, palette.on_surface_variant)).child(t("Private: recorded only on this computer")))
            .into_any_element()
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Screen time"), tk("How long you spend in each app, per day, with limits"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let bar = fn_view(
            Some(Duration::from_secs(SECS_PER_MIN)),
            |palette, _, cx| {
                let total = snapshot(cx).total(Local::now().date_naive());
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(icon("timer", crate::style::bar_icon(BAR_ICON_PX), palette.on_surface))
                    .when(total >= SECS_PER_MIN, |el| el.child(div().font_features(tabular_nums()).child(hm(total))))
                    .into_any_element()
            },
            window,
            cx,
        );
        if let Some(t) = tracker(cx) {
            super::follow(&bar, &t, cx);
        }
        let size_cell = Rc::new(std::cell::Cell::new(size(px(PANEL_W), px(EMPTY_H + HEADER_H + PAD * 2.0))));
        let panel = fn_view(Some(Duration::from_secs(SECS_PER_MIN)), panel_view(panel_cx.clone(), size_cell.clone()), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
    });
    r.command(
        ID,
        CommandSpec::new("screentime today", tk("Today's screen time per app"), |_, cx| {
            let s = snapshot(cx);
            let today = Local::now().date_naive();
            Ok(json!({ "date": today.to_string(), "total_seconds": s.total(today), "total": hm(s.total(today)), "apps": s.top(today).into_iter().map(|(a, secs)| json!({ "app": a, "seconds": secs, "time": hm(secs) })).collect::<Vec<_>>() }))
        }),
    );
    r.command(
        ID,
        CommandSpec::new("screentime week", tk("Screen time of the last seven days"), |_, cx| {
            let s = snapshot(cx);
            let days = s.last_days(Local::now().date_naive(), WEEK);
            Ok(json!(days.into_iter().map(|(d, secs)| json!({ "date": d.to_string(), "seconds": secs, "time": hm(secs) })).collect::<Vec<_>>()))
        }),
    );
    r.command(
        ID,
        CommandSpec::new("screentime app", tk("One app's screen time over the last seven days"), |a, cx| {
            let app = a.rest(0);
            let s = snapshot(cx);
            let today = Local::now().date_naive();
            let days: Vec<serde_json::Value> = (0..WEEK as u64)
                .rev()
                .filter_map(|b| today.checked_sub_days(chrono::Days::new(b)))
                .map(|d| {
                    let secs = s.day(d).and_then(|m| m.iter().find(|(k, _)| k.eq_ignore_ascii_case(app.trim())).map(|(_, v)| *v)).unwrap_or(0);
                    json!({ "date": d.to_string(), "seconds": secs, "time": hm(secs) })
                })
                .collect();
            Ok(json!({ "app": app.trim(), "days": days }))
        })
        .arg(ArgSpec::required("app", ArgKind::String, tk("app id, e.g. firefox"))),
    );
    r.command(
        ID,
        CommandSpec::new("screentime reset", tk("Forget recorded screen time (today, or all)"), |a, cx| {
            let all = a.get(0) == Some("all");
            let Some(t) = tracker(cx) else { return Err(t("Screen time is not running").to_string()) };
            let store = t.update(cx, |tr, cx| {
                if all {
                    tr.store.days.clear();
                } else {
                    tr.store.days.remove(&Local::now().date_naive().to_string());
                }
                tr.notified.clear();
                cx.notify();
                tr.store.clone()
            });
            save(&store);
            Ok(json!({ "reset": if all { "all" } else { "today" } }))
        })
        .arg(ArgSpec::required("scope", ArgKind::Enum, tk("today or all")).choices(&["today", "all"])),
    );
    r.command(
        ID,
        CommandSpec::new("screentime track", tk("Pause or resume recording screen time"), |a, cx| {
            let on = match a.str(0)? {
                "on" => true,
                "off" => false,
                _ => !config(cx).read(cx).get().screen_time.track,
            };
            update_config(cx, |c| c.screen_time.track = on);
            Ok(json!({ "tracking": on }))
        })
        .arg(ArgSpec::required("state", ArgKind::Enum, tk("on, off or toggle")).choices(&["on", "off", "toggle"])),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn store_adds_ranks_and_prunes() {
        let mut s = Store::default();
        s.add(d("2026-10-27"), "zed", 600);
        s.add(d("2026-10-27"), "firefox", 900);
        s.add(d("2026-10-27"), "zed", 600);
        s.add(d("2026-10-20"), "zed", 60);
        s.add(d("2026-10-27"), "", 99);
        assert_eq!(s.total(d("2026-10-27")), 2100);
        assert_eq!(s.top(d("2026-10-27"))[0], ("zed".to_string(), 1200));
        let week = s.last_days(d("2026-10-27"), 7);
        assert_eq!(week.len(), 7);
        assert_eq!(week.last().unwrap().1, 2100);
        assert_eq!(week.first().unwrap().0, d("2026-10-21"));
        s.prune(d("2026-10-27"), 3);
        assert!(s.day(d("2026-10-20")).is_none() && s.day(d("2026-10-27")).is_some());
    }

    #[test]
    fn tracker_books_focus_and_skips_excluded() {
        let cfg = ScreenTimeConfig { exclude: vec!["Secret".into()], ..ScreenTimeConfig::default() };
        let mut tr = Tracker { store: Store::default(), current: None, idle: false, dirty: false, saved_at: Instant::now(), notified: BTreeSet::new() };
        let mut notified = BTreeSet::new();
        let limits = ScreenTimeConfig { daily_limit_min: 1, ..ScreenTimeConfig::default() };
        let mut big = Store::default();
        big.add(Local::now().date_naive(), "zed", 120);
        assert_eq!(due_limits(&big, &mut notified, &limits, Local::now().date_naive()).len(), 1);
        assert!(due_limits(&big, &mut notified, &limits, Local::now().date_naive()).is_empty(), "announced once a day");
        tr.focus(Some("secret".into()), &cfg);
        assert!(tr.current.is_none());
        tr.focus(Some("zed".into()), &cfg);
        tr.current.as_mut().unwrap().1 -= Duration::from_secs(5);
        tr.focus(None, &cfg);
        assert_eq!(tr.store.total(Local::now().date_naive()), 5);
        // Idle time isn't booked.
        tr.focus(Some("zed".into()), &cfg);
        tr.idle = true;
        tr.current.as_mut().unwrap().1 -= Duration::from_secs(30);
        tr.flush(&cfg);
        assert_eq!(tr.store.total(Local::now().date_naive()), 5);
    }

    #[test]
    fn durations_read_well() {
        assert_eq!(hm(0), "0m");
        assert_eq!(hm(30), "<1m");
        assert_eq!(hm(14 * 60), "14m");
        assert_eq!(hm(2 * 3600 + 5 * 60), "2h 05m");
    }

    #[test]
    fn excluding_the_focused_app_stops_counting_at_once() {
        // What the config observer does when `exclude` changes.
        let mut tr = Tracker { store: Store::default(), current: None, idle: false, dirty: false, saved_at: Instant::now(), notified: BTreeSet::new() };
        let open = ScreenTimeConfig::default();
        tr.focus(Some("zed".into()), &open);
        assert!(tr.current.is_some());
        let closed = ScreenTimeConfig { exclude: vec!["Zed".into()], ..ScreenTimeConfig::default() };
        let current = tr.current.as_ref().map(|(a, _)| a.clone());
        tr.focus(current, &closed);
        assert!(tr.current.is_none(), "an excluded app no longer counts");
        // Removing it again resumes on the next focus.
        tr.focus(Some("zed".into()), &open);
        assert!(tr.current.is_some());
    }
}
