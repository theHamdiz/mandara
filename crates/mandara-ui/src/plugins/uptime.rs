//! Uptime monitor: watches websites (`https://…`, `http://…`) and services
//! (`host:port`, a TCP connect) and tells you when one goes down or comes
//! back. The bar chip is one status dot (emerald: all up, amber: something
//! answers slowly, coral: something is down, with how many). The panel
//! shows a summary, then every check with its state, latency, a sparkline
//! of the last 30 results, uptime and when it was last checked.
//!
//! This plugin is a monitor, so unlike most it keeps checking in the
//! background while it is enabled and placed in the bar (whether or not
//! the bar is visible), once per `interval_s`, never more often. Probes run
//! in parallel on their own threads (`mandara_services::uptime`), never on
//! the UI thread; offline mode pauses them. Settings
//! (`[plugins.settings.uptime]`): `checks`, `interval_s`, `notify`.
//! Commands: `uptime status`, `uptime check`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, AsyncApp, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder as _, px, size,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::uptime::{self as up, Check, Health, Probe, Results};
use mandara_theme::{Color, MandaraPalette};
use serde_json::{Value, json};

use super::extras::caption;
use super::fn_view;
use super::panel_fx::{
    ANGLE_DIAGONAL_DEG, EXPIRY_MARGIN, HEADER_H, PILL_H_PX, SECTION_H_PX, SMALL_ICON_PX, ViewSet, WASH_FAINT, alpha, duration_short, empty_state, glass_card, glow, header, partner,
    pill_button, readable, redraw_in, section, wash,
};
use crate::bidi::Bidi as _;
use crate::fonts::tabular_nums;
use crate::kit::{reveal, text, tokens};

pub const ID: &str = "uptime";

// ── Layout ───────────────────────────────────────────────────────────────

const PANEL_W: f32 = 460.0;
const PAD: f32 = 14.0;
const GAP: f32 = tokens::MD;
const EMPTY_H: f32 = 170.0;
const SECTION_GAP_PX: f32 = tokens::XS + 2.0;
/// Summary card.
const HERO_H: f32 = 78.0;
const HERO_PAD_PX: f32 = 16.0;
const HERO_FILL: (f32, f32) = (0.22, 0.08);
const HERO_DOT_PX: f32 = 14.0;
const HERO_TITLE_PX: f32 = 18.0;
const HERO_GLOW: (f32, f32) = (0.6, 10.0);
/// Skipped-entry note.
const NOTE_H: f32 = 18.0;
/// Check rows.
const ROW_H: f32 = 60.0;
const ROW_GAP_PX: f32 = 4.0;
const ROW_PAD_X_PX: f32 = 12.0;
const ROW_RADIUS_PX: f32 = 12.0;
const ROW_LINE_GAP_PX: f32 = 5.0;
const VISIBLE_ROWS: f32 = 6.5;
const DOT_PX: f32 = 8.0;
const DOT_GLOW: (f32, f32) = (0.5, 5.0);
const STATUS_H_PX: f32 = 20.0;
const STATUS_PAD_X_PX: f32 = 8.0;
const STATUS_TEXT_PX: f32 = 10.5;
const STATUS_FILL_ALPHA: f32 = 0.15;
const LATENCY_W_PX: f32 = 62.0;
const UPTIME_W_PX: f32 = 40.0;
/// Mini sparkline: size, column gap / radius / floor, down-column height.
const SPARK_W_PX: f32 = 96.0;
const SPARK_H_PX: f32 = 16.0;
const SPARK_GAP_PX: f32 = 1.0;
const SPARK_COL_RADIUS_PX: f32 = 1.0;
const SPARK_MIN_FRAC: f32 = 0.18;
const SPARK_AGE_FADE: f32 = 0.45;
/// Spinner of the Check now pill.
const SPIN: Duration = Duration::from_millis(900);
const BUSY_PAD_X_PX: f32 = 12.0;
/// Bar chip.
const CHIP_DOT_PX: f32 = 8.0;
const CHIP_GAP_PX: f32 = 5.0;
const CHIP_GLOW: (f32, f32) = (0.55, 6.0);
const CHIP_IDLE_ALPHA: f32 = 0.5;
const SIZE_EPSILON_PX: f32 = 0.5;
/// Gradient partners fade to this share of the accent's alpha.
const PARTNER_FADE: f32 = 0.3;
/// A check's name never takes more than this before its target.
const NAME_MAX_W_PX: f32 = 180.0;
/// Fraction → percent.
const PERCENT: f32 = 100.0;

// ── Timing ───────────────────────────────────────────────────────────────

/// The open panel redraws this often ("checked 20 s ago").
const PANEL_EVERY: Duration = Duration::from_secs(5);
/// The monitor sleeps at most this long at a time (follows a changed
/// interval soon).
const MAX_SLEEP: Duration = Duration::from_secs(60);

// ── Settings ─────────────────────────────────────────────────────────────

struct Settings {
    checks: String,
    every: Duration,
    notify: bool,
}

fn settings(cx: &App) -> Settings {
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let secs: i64 = cfg.plugins.setting(ID, "interval_s", up::DEFAULT_INTERVAL_S);
    Settings {
        checks: cfg.plugins.setting(ID, "checks", String::new()),
        every: Duration::from_secs(secs.clamp(up::INTERVAL_S_RANGE.0, up::INTERVAL_S_RANGE.1).unsigned_abs()),
        notify: cfg.plugins.setting(ID, "notify", true),
    }
}

// ── Monitor (one per shell process, kept alive by the bar chips) ─────────

/// One check with its results and the labels formatted from them.
struct Row {
    check: Check,
    results: Results,
    health: Option<Health>,
    latency: SharedString,
    uptime: SharedString,
    target: SharedString,
}

impl Row {
    fn new(check: Check) -> Self {
        let target = check.target.display().into();
        Self { check, results: Results::default(), health: None, latency: "–".into(), uptime: "–".into(), target }
    }

    fn push(&mut self, p: Probe) {
        self.health = Some(Health::of(&p));
        self.latency = match (p.up, p.ms) {
            (true, Some(ms)) => tf("{ms} ms", &[("ms", &ms)]).into(),
            _ => "–".into(),
        };
        self.results.push(p);
        self.uptime = self.results.uptime().map(|u| format!("{}%", (u * PERCENT).round() as i64)).unwrap_or_else(|| "–".into()).into();
    }
}

/// A check that changed between up and down.
struct Transition {
    name: String,
    up: bool,
    detail: String,
}

struct Mon {
    /// The `checks` setting the rows were built from.
    setting: String,
    every: Duration,
    rows: Vec<Row>,
    problems: Vec<String>,
    /// Bumped whenever the rows are rebuilt (stale rounds are dropped).
    generation: u64,
    running: bool,
    last_round: Option<Instant>,
    offline: bool,
    views: ViewSet,
}

impl Mon {
    fn new() -> Self {
        Self { setting: String::new(), every: Duration::from_secs(up::DEFAULT_INTERVAL_S.unsigned_abs()), rows: Vec::new(), problems: Vec::new(), generation: 0, running: false, last_round: None, offline: false, views: ViewSet::default() }
    }

    /// Follows the settings; returns whether the checks changed.
    fn sync(&mut self, s: &Settings) -> bool {
        self.every = s.every;
        if self.setting == s.checks && self.generation > 0 {
            return false;
        }
        let (checks, problems) = up::parse_checks(&s.checks);
        let mut old = std::mem::take(&mut self.rows);
        self.rows = checks
            .into_iter()
            .map(|c| match old.iter().position(|r| r.check == c) {
                Some(i) => old.swap_remove(i),
                None => Row::new(c),
            })
            .collect();
        self.problems = problems;
        self.setting = s.checks.clone();
        self.generation += 1;
        true
    }

    fn counts(&self) -> (usize, usize, usize) {
        let of = |h: Health| self.rows.iter().filter(|r| r.health == Some(h)).count();
        (of(Health::Up), of(Health::Slow), of(Health::Down))
    }

    /// Folds a round's results in; returns the up / down changes.
    fn apply(&mut self, generation: u64, probes: Vec<Probe>) -> Vec<Transition> {
        if generation != self.generation {
            return Vec::new();
        }
        let mut changes = Vec::new();
        for (row, p) in self.rows.iter_mut().zip(probes) {
            let was = row.results.last().map(|l| l.up);
            if let Some(was) = was
                && was != p.up
            {
                let detail = if p.up { p.ms.map(|ms| tf("Answered in {ms} ms", &[("ms", &ms)])).unwrap_or_default() } else { p.reason.clone() };
                changes.push(Transition { name: row.check.name.clone(), up: p.up, detail });
            }
            row.push(p);
        }
        changes
    }

    fn status_json(&self) -> Value {
        let checks: Vec<Value> = self
            .rows
            .iter()
            .map(|r| {
                let last = r.results.last();
                json!({
                    "name": r.check.name, "target": r.check.target.display(), "kind": r.check.target,
                    "status": r.health.map_or(Value::from("pending"), |h| json!(h)),
                    "latency_ms": last.and_then(|p| p.ms), "reason": last.map(|p| p.reason.clone()).filter(|r| !r.is_empty()),
                    "checked_at": last.map(|p| p.at), "uptime": r.results.uptime(), "results": r.results.len(),
                })
            })
            .collect();
        let (_, slow, down) = self.counts();
        json!({ "checks": checks, "down": down, "slow": slow, "interval_s": self.every.as_secs(), "offline_paused": self.offline, "skipped": self.problems })
    }
}

thread_local! {
    static SHARED: RefCell<Weak<RefCell<Mon>>> = const { RefCell::new(Weak::new()) };
}

fn current() -> Option<Rc<RefCell<Mon>>> {
    SHARED.with(|w| w.borrow().upgrade())
}

/// The monitor every bar chip shares; it stops once the last chip is gone.
fn shared(cx: &mut App) -> Rc<RefCell<Mon>> {
    if let Some(m) = current() {
        return m;
    }
    let mon = Rc::new(RefCell::new(Mon::new()));
    mon.borrow_mut().sync(&settings(cx));
    SHARED.with(|w| *w.borrow_mut() = Rc::downgrade(&mon));
    let weak = Rc::downgrade(&mon);
    cx.spawn(async move |cx| {
        loop {
            let wait = {
                let Some(mon) = weak.upgrade() else { break };
                let s = cx.update(|cx| settings(cx));
                let mut m = mon.borrow_mut();
                m.sync(&s);
                match m.last_round {
                    _ if m.running => MAX_SLEEP,
                    Some(at) if at.elapsed() < m.every => (m.every - at.elapsed()).min(MAX_SLEEP),
                    _ => Duration::ZERO,
                }
            };
            if wait.is_zero() {
                run_round(weak.clone(), cx).await;
            } else {
                cx.background_executor().timer(wait).await;
            }
        }
    })
    .detach();
    mon
}

/// Probes every check once (parallel, off the UI thread), records the
/// results, notifies the views and posts up / down notifications.
async fn run_round(weak: Weak<RefCell<Mon>>, cx: &mut AsyncApp) {
    let (targets, generation) = {
        let Some(mon) = weak.upgrade() else { return };
        let mut m = mon.borrow_mut();
        if m.running {
            return;
        }
        m.last_round = Some(Instant::now());
        m.offline = mandara_services::offline::is_offline();
        if m.offline || m.rows.is_empty() {
            let views = m.views.clone();
            drop(m);
            views.notify_async(cx);
            return;
        }
        m.running = true;
        (m.rows.iter().map(|r| r.check.target.clone()).collect::<Vec<_>>(), m.generation)
    };
    let views = weak.upgrade().map(|m| m.borrow().views.clone());
    if let Some(v) = &views {
        v.notify_async(cx);
    }
    let probes = mandara_services::runtime::unblock(move || up::probe_all(&targets, up::TIMEOUT)).await;
    let Some(mon) = weak.upgrade() else { return };
    let (changes, views) = {
        let mut m = mon.borrow_mut();
        m.running = false;
        m.last_round = Some(Instant::now());
        (m.apply(generation, probes), m.views.clone())
    };
    cx.update(|cx| {
        views.notify(cx);
        if !changes.is_empty() && settings(cx).notify {
            for c in changes {
                let summary = if c.up { tf("{name} is back up", &[("name", &c.name)]) } else { tf("{name} is down", &[("name", &c.name)]) };
                super::notifications::send(cx, mandara_services::NotificationCommand::Post { app: t("Uptime monitor").into(), icon: "heart".into(), summary, body: c.detail });
            }
        }
    });
}

/// Starts a round now (unless one is running).
fn check_now(cx: &mut App) {
    if let Some(mon) = current() {
        let weak = Rc::downgrade(&mon);
        cx.spawn(async move |cx| run_round(weak, cx).await).detach();
    }
}

// ── Pieces ───────────────────────────────────────────────────────────────

fn health_color(h: Option<Health>, palette: &MandaraPalette) -> Color {
    match h {
        Some(Health::Up) => readable(palette.success_emerald, palette),
        Some(Health::Slow) => readable(palette.warning_amber, palette),
        Some(Health::Down) => readable(palette.urgent_coral, palette),
        None => palette.on_surface_variant.with_alpha(CHIP_IDLE_ALPHA),
    }
}

fn health_label(h: Option<Health>) -> &'static str {
    match h {
        Some(Health::Up) => t("Up"),
        Some(Health::Slow) => t("Slow"),
        Some(Health::Down) => t("Down"),
        None => t("Waiting"),
    }
}

/// The overall state: worst first.
fn overall(down: usize, slow: usize) -> Option<Health> {
    if down > 0 {
        Some(Health::Down)
    } else if slow > 0 {
        Some(Health::Slow)
    } else {
        None
    }
}

/// Latency columns, newest at the right; down results are short coral
/// ticks, slow ones amber.
fn mini_spark(results: &Results, palette: &MandaraPalette) -> AnyElement {
    let peak = results.peak_ms().max(1) as f32;
    let len = results.len();
    let cap = up::HISTORY;
    let cols = (0..cap).map(|slot| {
        let col = div().flex_1().min_w(px(1.0)).h_full().flex().flex_col().justify_end();
        let Some(p) = slot.checked_sub(cap - len).and_then(|i| results.iter().nth(i)) else { return col };
        let age = 1.0 - slot as f32 / cap as f32;
        let health = Health::of(p);
        let frac = match (health, p.ms) {
            (Health::Down, _) | (_, None) => SPARK_MIN_FRAC,
            (_, Some(ms)) => (ms as f32 / peak).clamp(SPARK_MIN_FRAC, 1.0),
        };
        let c = health_color(Some(health), palette).with_alpha(1.0 - age * SPARK_AGE_FADE);
        col.child(div().w_full().h(px(frac * SPARK_H_PX)).rounded(px(SPARK_COL_RADIUS_PX)).bg(c.hsla()))
    });
    div().w(px(SPARK_W_PX)).h(px(SPARK_H_PX)).flex_none().ltr_row().items_end().gap(px(SPARK_GAP_PX)).children(cols).into_any_element()
}

fn ago(at: i64) -> String {
    tf("checked {span} ago", &[("span", &duration_short(chrono::Utc::now().timestamp() - at))])
}

fn row_view(r: &Row, palette: &MandaraPalette) -> AnyElement {
    let tint = health_color(r.health, palette);
    let last = r.results.last();
    let when: String = match last {
        Some(p) if !p.up && !p.reason.is_empty() => format!("{} · {}", p.reason, ago(p.at)),
        Some(p) => ago(p.at),
        None => t("not checked yet").into(),
    };
    let status = div()
        .h(px(STATUS_H_PX))
        .px(px(STATUS_PAD_X_PX))
        .flex_none()
        .rounded_full()
        .bg(tint.with_alpha(STATUS_FILL_ALPHA).hsla())
        .flex()
        .items_center()
        .text_size(px(STATUS_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tint.hsla())
        .child(health_label(r.health));
    div()
        .h(px(ROW_H))
        .flex_none()
        .px(px(ROW_PAD_X_PX))
        .rounded(px(crate::style::radius(ROW_RADIUS_PX)))
        .bg(wash(WASH_FAINT, palette).hsla())
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(ROW_LINE_GAP_PX))
        .child(
            div()
                .row()
                .items_center()
                .gap(px(tokens::SM))
                .child(div().size(px(DOT_PX)).flex_none().rounded_full().bg(tint.hsla()).when(r.health.is_some(), |el| el.shadow(vec![glow(tint.with_alpha(DOT_GLOW.0), DOT_GLOW.1, 0.0, 0.0)])))
                .child(div().flex_none().max_w(px(NAME_MAX_W_PX)).truncate().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(r.check.name.clone()))
                .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(r.target.clone()))
                .child(status)
                .child(div().w(px(LATENCY_W_PX)).flex_none().text_end().font_features(tabular_nums()).text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).child(r.latency.clone())),
        )
        .child(
            div()
                .row()
                .items_center()
                .gap(px(tokens::SM))
                .child(div().w(px(DOT_PX)).flex_none())
                .child(caption(when, palette).flex_1().min_w(px(0.0)).truncate().font_features(tabular_nums()))
                .child(mini_spark(&r.results, palette))
                .child(div().w(px(UPTIME_W_PX)).flex_none().text_end().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(r.uptime.clone())),
        )
        .into_any_element()
}

fn hero(m: &Mon, palette: &MandaraPalette) -> AnyElement {
    let (upn, slow, down) = m.counts();
    let total = m.rows.len();
    let state = if m.offline { None } else { overall(down, slow).or((upn > 0).then_some(Health::Up)) };
    let tint = health_color(state, palette);
    let title: String = if m.offline {
        t("Paused: offline mode is on").into()
    } else {
        match state {
            Some(Health::Down) => tf("{count} of {total} down", &[("count", &down), ("total", &total)]),
            Some(Health::Slow) => tf("{count} answering slowly", &[("count", &slow)]),
            Some(Health::Up) if upn == total => t("All systems up").into(),
            _ => t("Checking…").into(),
        }
    };
    let results: Vec<f32> = m.rows.iter().filter_map(|r| r.results.uptime()).collect();
    let detail = if results.is_empty() {
        tf("{count} checks · every {span}", &[("count", &total), ("span", &duration_short(m.every.as_secs() as i64))])
    } else {
        let mean = results.iter().sum::<f32>() / results.len() as f32;
        tf("{count} checks · {uptime}% uptime · every {span}", &[("count", &total), ("uptime", &format!("{:.1}", mean * PERCENT)), ("span", &duration_short(m.every.as_secs() as i64))])
    };
    glass_card(palette)
        .h(px(HERO_H))
        .px(px(HERO_PAD_PX))
        .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, tint.with_alpha(alpha(HERO_FILL, palette)), partner(tint).with_alpha(alpha(HERO_FILL, palette) * PARTNER_FADE)))
        .row()
        .items_center()
        .gap(px(tokens::MD))
        .child(div().size(px(HERO_DOT_PX)).flex_none().rounded_full().bg(tint.hsla()).shadow(vec![glow(tint.with_alpha(HERO_GLOW.0), HERO_GLOW.1, 0.0, 0.0)]))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(HERO_TITLE_PX)).font_weight(FontWeight::SEMIBOLD).truncate().child(title))
                .child(caption(detail, palette).font_features(tabular_nums())),
        )
        .into_any_element()
}

fn check_button(running: bool, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    if running {
        return div()
            .h(px(PILL_H_PX))
            .px(px(BUSY_PAD_X_PX))
            .flex_none()
            .rounded_full()
            .bg(wash(WASH_FAINT, palette).hsla())
            .row()
            .items_center()
            .gap(px(SECTION_GAP_PX))
            .text_size(px(text::LABEL))
            .text_color(palette.on_surface_variant.hsla())
            .child(
                gpui::svg()
                    .path("icons/refresh.svg")
                    .size(px(SMALL_ICON_PX))
                    .flex_none()
                    .text_color(accent.hsla())
                    .with_animation("up-spin", Animation::new(SPIN).repeat(), |svg, d| svg.with_transformation(gpui::Transformation::rotate(gpui::radians(d * std::f32::consts::TAU)))),
            )
            .child(t("Checking…"))
            .into_any_element();
    }
    pill_button("up-check", Some("refresh"), t("Check now"), accent, palette, |_, cx| check_now(cx)).into_any_element()
}

// ── Views ────────────────────────────────────────────────────────────────

/// Bar chips and panels follow the settings as they render.
fn follow_settings(mon: &Rc<RefCell<Mon>>, cx: &mut App) {
    let changed = mon.borrow_mut().sync(&settings(cx));
    if changed {
        mon.borrow_mut().last_round = None;
        check_now(cx);
    }
}

fn bar_view(mon: &Rc<RefCell<Mon>>, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    mon.borrow().views.track(window);
    follow_settings(mon, cx);
    let m = mon.borrow();
    let (upn, slow, down) = m.counts();
    let state = if m.offline || m.rows.is_empty() { None } else { overall(down, slow).or((upn > 0).then_some(Health::Up)) };
    let tint = health_color(state, palette);
    div()
        .flex()
        .items_center()
        .gap(px(CHIP_GAP_PX))
        .child(div().size(px(CHIP_DOT_PX)).flex_none().rounded_full().bg(tint.hsla()).when(state.is_some(), |el| el.shadow(vec![glow(tint.with_alpha(CHIP_GLOW.0), CHIP_GLOW.1, 0.0, 0.0)])))
        .when(down > 0 && !m.offline, |el| el.child(div().font_features(tabular_nums()).text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(tint.hsla()).child(down.to_string())))
        .into_any_element()
}

struct View {
    p: PanelCx,
    mon: Rc<RefCell<Mon>>,
    size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>,
}

impl View {
    fn set_height(&self, h: f32) {
        if (f32::from(self.size_cell.get().height) - h).abs() > SIZE_EPSILON_PX {
            self.size_cell.set(size(px(PANEL_W), px(h)));
        }
    }

    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        self.mon.borrow().views.track(window);
        follow_settings(&self.mon, cx);
        if self.p.progress() > 0.0 {
            redraw_in(PANEL_EVERY + EXPIRY_MARGIN, window, cx);
        }
        let m = self.mon.borrow();
        let accent = readable(palette.primary, palette);
        let subtitle: String = match m.last_round {
            Some(at) => tf("Every {every} · last check {span} ago", &[("every", &duration_short(m.every.as_secs() as i64)), ("span", &duration_short(at.elapsed().as_secs() as i64))]),
            None => tf("Every {every}", &[("every", &duration_short(m.every.as_secs() as i64))]),
        };
        let trailing = (!m.rows.is_empty()).then(|| check_button(m.running, palette));
        let mut h = PAD * 2.0 + HEADER_H;
        let mut blocks: Vec<AnyElement> = Vec::new();
        if m.rows.is_empty() {
            h += GAP + EMPTY_H;
            blocks.push(empty_state("heart", t("Nothing to watch yet"), t("Add checks in Studio → Plugins → Uptime monitor"), EMPTY_H, palette).into_any_element());
        } else {
            h += GAP + HERO_H;
            blocks.push(hero(&m, palette));
            let list_h = (m.rows.len() as f32).min(VISIBLE_ROWS) * (ROW_H + ROW_GAP_PX) - ROW_GAP_PX;
            h += GAP + SECTION_H_PX + SECTION_GAP_PX + list_h;
            let rows: Vec<AnyElement> = m.rows.iter().map(|r| row_view(r, palette)).collect();
            blocks.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(SECTION_GAP_PX))
                    .child(section(t("Checks"), palette, None))
                    .child(div().h(px(list_h)).child(div().id("up-rows").size_full().overflow_y_scroll().flex().flex_col().gap(px(ROW_GAP_PX)).children(rows)))
                    .into_any_element(),
            );
        }
        if !m.problems.is_empty() {
            h += GAP + NOTE_H;
            let coral = readable(palette.urgent_coral, palette);
            blocks.push(div().h(px(NOTE_H)).truncate().text_size(px(text::LABEL)).text_color(coral.hsla()).child(tf("Skipped: {problems}", &[("problems", &m.problems.join("; "))])).into_any_element());
        }
        drop(m);
        self.set_height(h);
        let mut col = div().flex().flex_col().gap(px(GAP)).child(reveal(&self.p, 0, header("heart", accent, t("Uptime monitor"), subtitle, palette, trailing)));
        for (i, b) in blocks.into_iter().enumerate() {
            col = col.child(reveal(&self.p, i + 1, b));
        }
        div().size_full().p(px(PAD)).child(col).into_any_element()
    }
}

// ── Commands ─────────────────────────────────────────────────────────────

/// A one-off round for when no bar chip runs the monitor.
fn one_off(cx: &App) -> Output {
    let s = settings(cx);
    crate::powerctl::pending(move || {
        let mut m = Mon::new();
        m.sync(&s);
        let targets: Vec<_> = m.rows.iter().map(|r| r.check.target.clone()).collect();
        let g = m.generation;
        let probes = up::probe_all(&targets, up::TIMEOUT);
        m.apply(g, probes);
        Ok(m.status_json())
    })
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Uptime monitor"), tk("Watches your sites and services and tells you when one goes down"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let mon = shared(cx);
        let bar_mon = mon.clone();
        let bar = fn_view(None, move |palette, window, cx| bar_view(&bar_mon, palette, window, cx), window, cx);
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(PAD * 2.0 + HEADER_H + GAP + EMPTY_H))));
        let view = View { p: panel_cx.clone(), mon, size_cell: size_cell.clone() };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
    });
    r.command(
        ID,
        CommandSpec::new_output("uptime status", tk("Every check with its state, latency, uptime and last result (JSON)"), |_, cx| {
            Ok(match current() {
                Some(m) if m.borrow().last_round.is_some() => Output::Value(m.borrow().status_json()),
                _ => one_off(cx),
            })
        }),
    );
    r.command(
        ID,
        CommandSpec::new_output("uptime check", tk("Check everything now and answer with the results (JSON)"), |_, cx| {
            let Some(mon) = current() else { return Ok(one_off(cx)) };
            let (tx, rx) = async_channel::bounded(1);
            let weak = Rc::downgrade(&mon);
            cx.spawn(async move |cx| {
                run_round(weak.clone(), cx).await;
                let reply = weak.upgrade().map(|m| m.borrow().status_json().to_string()).ok_or_else(|| t("The uptime monitor stopped").to_string());
                let _ = tx.try_send(reply);
            })
            .detach();
            Ok(Output::Pending(rx))
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(up: bool, ms: u32) -> Probe {
        Probe { up, ms: Some(ms), reason: if up { String::new() } else { "connection refused".into() }, at: 0 }
    }

    fn settings_for(checks: &str) -> Settings {
        Settings { checks: checks.into(), every: Duration::from_secs(60), notify: true }
    }

    #[test]
    fn transitions_fire_on_changes_only() {
        let mut m = Mon::new();
        assert!(m.sync(&settings_for("NAS=nas.lan:22, Blog=https://example.com")));
        let g = m.generation;
        assert!(m.apply(g, vec![probe(true, 12), probe(false, 0)]).is_empty(), "the first result is a baseline");
        assert!(m.apply(g, vec![probe(true, 15), probe(false, 0)]).is_empty());
        let changes = m.apply(g, vec![probe(false, 0), probe(true, 80)]);
        assert_eq!(changes.len(), 2);
        assert!(!changes[0].up && changes[0].name == "NAS" && changes[0].detail == "connection refused");
        assert!(changes[1].up && changes[1].detail == "Answered in 80 ms");
        assert_eq!(m.counts(), (1, 0, 1));
        assert_eq!(m.rows[1].latency.as_ref(), "80 ms");
        assert_eq!(m.rows[1].uptime.as_ref(), "33%");
    }

    #[test]
    fn settings_changes_keep_known_results_and_drop_stale_rounds() {
        let mut m = Mon::new();
        m.sync(&settings_for("NAS=nas.lan:22"));
        let g = m.generation;
        m.apply(g, vec![probe(true, 5)]);
        assert!(!m.sync(&settings_for("NAS=nas.lan:22")), "unchanged");
        assert!(m.sync(&settings_for("Blog=https://example.com, NAS=nas.lan:22")));
        assert_eq!(m.rows[1].results.len(), 1, "NAS keeps its history");
        assert!(m.apply(g, vec![probe(false, 0), probe(false, 0)]).is_empty(), "a round for the old list is ignored");
        assert_eq!(m.rows[1].results.len(), 1);
        assert_eq!(overall(0, 0), None);
        assert_eq!(overall(1, 3), Some(Health::Down));
        assert_eq!(overall(0, 1), Some(Health::Slow));
    }
}
