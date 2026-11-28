//! Task progress (built-in `progress` plugin, opt-in): a bar pill that
//! scripts, long builds and AI agents push progress to with
//! `mandara msg progress …`.
//!
//! Tasks live in one shared in-memory board
//! ([`mandara_services::progress::Board`]); nothing is polled. The bar chip
//! shows a ring with the percent of the most recent running task (an animated
//! running light while it has no value yet) and `+N` for the others; finished tasks stay
//! in the bar for 30 s and in the panel until cleared. A task with no update
//! for 10 minutes shows as stalled. The panel lists every task with an OKLCH
//! gradient bar (a shimmer when the amount is unknown), elapsed time, an ETA
//! from the rate so far and a status pill.
//!
//! Timers: the only animation (running light / shimmer) ticks while an
//! indeterminate task is on screen; elapsed times tick once a second while
//! the panel is open; otherwise one timer waits for the next stall or for a
//! finished task to leave the bar.
//!
//! Settings (`[plugins.settings.progress]`): `notify` (post a desktop
//! notification when a task finishes or fails, default on), and
//! `show_when_idle` (draw a quiet ring in the bar when no task is active,
//! default off).
//!
//! Commands: `progress start <id> [label…]`, `progress set <id> <value>
//! [label…]` (`40`, `40%` or `3/10`), `progress touch <id> [label…]` (refresh
//! activity without changing progress), `progress done <id> [message…]`,
//! `progress fail <id> [message…]`, `progress remove <id>`, `progress list`,
//! `progress clear`.

use std::cell::{Cell, RefCell};
use std::f32::consts::TAU;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext as _, Background, Bounds, Entity, FontWeight, Global, IntoElement, ParentElement, SharedString, Styled, Window, canvas, div, fill, point,
    prelude::FluentBuilder as _, px, relative, size,
};
use mandara_config::ShellConfig;
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{Lang, t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::NotificationCommand;
use mandara_services::progress::{self as pg, Amount, Board, Status, Task};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::extras::caption;
use super::panel_fx::{ANGLE_RIGHT_DEG, EXPIRY_MARGIN, HEADER_H, duration_short, empty_state, glass_card, header, partner, percent_text, pill_button, readable, redraw_in, wash, WASH_TRACK};
use super::{fn_view, follow, json};
use crate::fonts::{fonts, tabular_nums};
use crate::icons::ring;
use crate::kit::{icon_button, reveal, text};

pub const ID: &str = "progress";

mod order;
mod state;
use order::ordered_tasks;
use state::TaskState;

// ── Layout ───────────────────────────────────────────────────────────────

const PANEL_W: f32 = 380.0;
const PAD: f32 = 14.0;
const GAP: f32 = 12.0;
const EMPTY_H: f32 = 170.0;
/// Task rows: height, gap, padding, the gradient bar's height.
const ROW_H: f32 = 76.0;
const ROW_GAP: f32 = 6.0;
const ROW_PAD_X_PX: f32 = 12.0;
const ROW_PAD_Y_PX: f32 = 10.0;
const ROW_LINE_GAP_PX: f32 = 6.0;
const BAR_TRACK_H: f32 = 6.0;
/// Status pill and the remove button.
const PILL_H_PX: f32 = 20.0;
const PILL_PAD_X_PX: f32 = 8.0;
const PILL_FILL_ALPHA: f32 = 0.16;
const REMOVE_PX: f32 = 22.0;
const INLINE_GAP_PX: f32 = 6.0;
/// Most rows shown; the rest are summarised.
const MAX_ROWS: usize = 7;
const MORE_H: f32 = 18.0;
/// Bar chip: ring, stroke, gaps, the finished glyph, the +N badge.
const BAR_RING_PX: f32 = 15.0;
const BAR_RING_STROKE_PX: f32 = 2.2;
const BAR_GAP_PX: f32 = 5.0;
const BAR_MORE_PX: f32 = 10.5;
const RING_TRACK_ALPHA: f32 = 0.16;
/// The running light has eight cells. The count is part of the indicator's
/// identity, not a dimension someone would tune; the bar chip is laid out for
/// exactly eight, so it stays fixed while the cell size is a setting.
const LIGHT_CELLS: usize = 8;

/// Defaults and bounded ranges for the user-facing light settings. The
/// defaults are the values the indicator shipped with. The ranges keep the
/// row inside a bar pill and the pulse readable without becoming a strobe.
const LIGHT_W_DEFAULT: f64 = 3.0;
const LIGHT_W_RANGE: (f64, f64) = (2.0, 6.0);
const LIGHT_H_DEFAULT: f64 = 8.0;
const LIGHT_H_RANGE: (f64, f64) = (4.0, 16.0);
const LIGHT_GAP_DEFAULT: f64 = 2.0;
const LIGHT_GAP_RANGE: (f64, f64) = (0.0, 6.0);
const LIGHT_PERIOD_DEFAULT: f64 = 1.6;
const LIGHT_PERIOD_RANGE: (f64, f64) = (0.5, 5.0);
const LIGHT_PHASE_DEFAULT: f64 = 0.18;
const LIGHT_PHASE_RANGE: (f64, f64) = (0.0, 0.8);
const LIGHT_MIN_ALPHA_DEFAULT: f64 = 0.3;
const LIGHT_ALPHA_RANGE: (f64, f64) = (0.0, 1.0);
const LIGHT_MAX_ALPHA_DEFAULT: f64 = 1.0;
const LIGHT_IDLE_ALPHA_DEFAULT: f64 = 0.55;
/// Shimmer: segment width (share of the track) and passes per second.
const SHIMMER_W: f32 = 0.32;
const SHIMMER_PER_S: f32 = 0.7;
/// The running light and shimmer redraw at this pace (smooth, but not the
/// full display rate for a pill in the bar).
const ANIMATION_FRAME: Duration = Duration::from_millis(33);
/// Elapsed times in the panel tick once a second.
const SECOND: Duration = Duration::from_secs(1);
const MS_PER_S: i64 = 1000;

// ── Model ────────────────────────────────────────────────────────────────

/// The shared board, with a revision that bumps on every change.
pub struct Progress {
    pub board: Board,
    rev: u64,
}

struct GlobalProgress(Entity<Progress>);
impl Global for GlobalProgress {}

/// The shared model (created on first use).
pub fn model(cx: &mut App) -> Entity<Progress> {
    if let Some(g) = cx.try_global::<GlobalProgress>() {
        return g.0.clone();
    }
    let entity = cx.new(|_| Progress { board: Board::default(), rev: 0 });
    cx.set_global(GlobalProgress(entity.clone()));
    entity
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Applies `f` to the board and re-renders the views that show it.
fn change<R>(cx: &mut App, f: impl FnOnce(&mut Board, i64) -> Result<R, String>) -> Result<R, String> {
    let m = model(cx);
    m.update(cx, |p, cx| {
        let out = f(&mut p.board, now_ms())?;
        p.rev += 1;
        cx.notify();
        Ok(out)
    })
}

/// Posts "finished" / "failed" when the `notify` setting is on.
fn announce(task: &Task, cx: &mut App) {
    if !crate::state::config(cx).read(cx).get().plugins.setting(ID, "notify", true) {
        return;
    }
    let ok = task.status == Status::Done;
    let summary = if ok { tf("{task} finished", &[("task", &task.label)]) } else { tf("{task} failed", &[("task", &task.label)]) };
    let took = tf("Took {time}", &[("time", &duration_short(task.elapsed_ms(now_ms()) / MS_PER_S))]);
    let body = task.message.as_ref().map(|m| format!("{m}\n{took}")).unwrap_or(took);
    // Keep the same light vocabulary everywhere: no check / alert glyphs.
    crate::plugins::notifications::send(cx, NotificationCommand::Post { app: t("Task progress").into(), icon: "gauge".into(), summary, body });
}

fn task_json(task: &Task, now: i64) -> serde_json::Value {
    let fraction = task.fraction();
    let mut v = json(task);
    v["fraction"] = json!(fraction);
    v["percent"] = json!(fraction.map(|f| (f * 100.0).round()));
    v["elapsed_s"] = json!(task.elapsed_ms(now) / MS_PER_S);
    v["eta_s"] = json!(task.eta_ms().map(|e| e / MS_PER_S));
    v["stalled"] = json!(task.stalled(now));
    v
}

// ── Look ─────────────────────────────────────────────────────────────────

/// A task's state colour (not yet made readable).
fn tint(task: &Task, now: i64, palette: &MandaraPalette) -> Color {
    TaskState::from(task).color(now, palette)
}

/// Seconds since the process started, for animation phases.
fn clock_s() -> f32 {
    thread_local! {
        static EPOCH: Instant = Instant::now();
    }
    EPOCH.with(|e| e.elapsed().as_secs_f32())
}

/// The gradient stops depend only on the row's resolved colour (one of a few
/// palette roles), so keep the handful built so far instead of rebuilding a
/// fill for every visible row on every render.
fn cached_gradient(color: Color) -> Background {
    thread_local! {
        static GRADIENTS: RefCell<Vec<(Color, Background)>> = const { RefCell::new(Vec::new()) };
    }
    GRADIENTS.with(|cell| {
        let mut cache = cell.borrow_mut();
        if let Some((_, fill)) = cache.iter().find(|(c, _)| *c == color) {
            return *fill;
        }
        let fill = oklab_gradient(ANGLE_RIGHT_DEG, color, partner(color).with_lightness(color.lightness()));
        // At most the four task-state colours plus a little slack for a
        // palette change; drop the oldest rather than grow without bound.
        if cache.len() >= 8 {
            cache.remove(0);
        }
        cache.push((color, fill));
        fill
    })
}

/// The panel's progress bar: an OKLCH gradient fill, or a shimmer sweeping
/// the track while the amount is unknown.
fn progress_bar(fraction: Option<f64>, color: Color, palette: &MandaraPalette) -> AnyElement {
    let track = div().relative().w_full().h(px(BAR_TRACK_H)).rounded_full().overflow_hidden().bg(wash(WASH_TRACK, palette).hsla());
    let fill = cached_gradient(color);
    match fraction {
        Some(f) => {
            let f = f.clamp(0.0, 1.0) as f32;
            track.child(div().h_full().w(relative(f)).min_w(px(if f > 0.0 { BAR_TRACK_H } else { 0.0 })).rounded_full().bg(fill)).into_any_element()
        }
        None => {
            let pos = (clock_s() * SHIMMER_PER_S).fract() * (1.0 + SHIMMER_W) - SHIMMER_W;
            track.child(div().absolute().top_0().h_full().left(relative(pos)).w(relative(SHIMMER_W)).rounded_full().bg(fill)).into_any_element()
        }
    }
}

/// Everything a row shows, formatted once per second (not per frame).
struct RowText {
    id: SharedString,
    label: SharedString,
    amount: Option<SharedString>,
    detail: SharedString,
    fraction: Option<f64>,
    state: TaskState,
    finished: bool,
}

fn row_text(task: &Task, now: i64) -> RowText {
    let mut detail = vec![duration_short(task.elapsed_ms(now) / MS_PER_S)];
    if let Some(eta) = task.eta_ms() {
        detail.push(tf("about {time} left", &[("time", &duration_short(eta.max(MS_PER_S) / MS_PER_S))]));
    }
    if let Some(m) = &task.message {
        detail.push(m.clone());
    }
    let amount = task.amount.map(|a| match a {
        Amount::Steps { done, total } => SharedString::from(format!("{done} / {total}")),
        Amount::Fraction { value } => percent_text(value as f32),
    });
    RowText {
        id: task.id.clone().into(),
        label: task.label.clone().into(),
        amount,
        detail: detail.join(" · ").into(),
        // Only a running task without a value shimmers; a finished one sits still.
        fraction: task.fraction().or((task.status != Status::Running).then_some(0.0)),
        state: TaskState::from(task),
        finished: matches!(task.status, Status::Done | Status::Failed),
    }
}

fn row(i: usize, r: &RowText, now: i64, palette: &MandaraPalette, mono: &SharedString) -> AnyElement {
    let base = r.state.color(now, palette);
    let tint = readable(base, palette);
    let id = r.id.to_string();
    let pill = div()
        .h(px(PILL_H_PX))
        .px(px(PILL_PAD_X_PX))
        .flex_none()
        .rounded_full()
        .bg(tint.with_alpha(PILL_FILL_ALPHA).hsla())
        .flex()
        .items_center()
        .text_size(px(text::LABEL))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tint.hsla())
        .child(r.state.label(now));
    glass_card(palette)
        .h(px(ROW_H))
        .px(px(ROW_PAD_X_PX))
        .py(px(ROW_PAD_Y_PX))
        .flex()
        .flex_col()
        .justify_between()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(INLINE_GAP_PX))
                .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(r.label.clone()))
                .when_some(r.amount.clone(), |el, a| el.child(div().flex_none().font_features(tabular_nums()).text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(a)))
                .child(pill)
                .when(r.finished, |el| {
                    el.child(icon_button(("progress-remove", i), "close", REMOVE_PX, palette.on_surface_variant, None, move |_, cx| {
                        let _ = change(cx, |b, _| b.remove(&id));
                    }))
                }),
        )
        .child(progress_bar(r.fraction, base, palette))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(ROW_LINE_GAP_PX))
                .child(caption(r.id.clone(), palette).flex_none().font_family(mono.clone()))
                .child(caption(r.detail.clone(), palette).flex_1().min_w(px(0.0)).truncate().font_features(tabular_nums())),
        )
        .into_any_element()
}

fn panel_height(rows: usize, more: bool) -> f32 {
    let body = if rows == 0 { EMPTY_H } else { rows as f32 * ROW_H + (rows as f32 - 1.0) * ROW_GAP + if more { ROW_GAP + MORE_H } else { 0.0 } };
    PAD * 2.0 + HEADER_H + GAP + body
}

/// Formatted task text depends on the board, elapsed second and resolved locale.
#[derive(Default)]
struct RowCache {
    entry: Option<((u64, i64, Lang), Rc<[RowText]>)>,
}

impl RowCache {
    /// Retain only visible formatted rows, rebuilding after a data, second or locale change.
    fn get(&mut self, board: &Board, revision: u64, now: i64) -> Rc<[RowText]> {
        let key = (revision, now / MS_PER_S, mandara_i18n::language());
        if let Some((previous, rows)) = &self.entry && *previous == key {
            return rows.clone();
        }
        let rows: Rc<[RowText]> = ordered_tasks(&board.tasks).take(MAX_ROWS).map(|task| row_text(task, now)).collect();
        self.entry = Some((key, rows.clone()));
        rows
    }
}

struct PanelView {
    p: PanelCx,
    size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>,
    cache: RefCell<RowCache>,
}

impl PanelView {
    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let now = now_ms();
        let m = model(cx);
        let (rows, more, running, waiting, finished, any_unknown, next) = {
            let pr = m.read(cx);
            let b = &pr.board;
            let more = b.tasks.len().saturating_sub(MAX_ROWS);
            let rows = self.cache.borrow_mut().get(b, pr.rev, now);
            let running = b.running().count();
            let waiting = b.waiting().count();
            // A cached row with no fraction is an indeterminate (shimmering)
            // task. The rows are already the ordered, visible ones, so this
            // avoids a second scan over the whole board every render.
            let any_unknown = rows.iter().any(|r| r.fraction.is_none());
            (rows, more, running, waiting, b.tasks.len() - running - waiting, any_unknown, b.next_change(now))
        };
        if self.p.progress() > 0.0 {
            if any_unknown {
                redraw_in(ANIMATION_FRAME, window, cx);
            } else if running > 0 || waiting > 0 {
                // A waiting task has no stall deadline, so `next` cannot keep
                // its elapsed-second text fresh. Refresh once per second; the
                // waiting light and progress track stay stationary.
                redraw_in(SECOND, window, cx);
            }
            if let Some(at) = next {
                redraw_in(Duration::from_millis(u64::try_from(at - now).unwrap_or(0)) + EXPIRY_MARGIN, window, cx);
            }
        }
        self.size_cell.set(size(px(PANEL_W), px(panel_height(rows.len(), more > 0))));
        let accent = readable(palette.primary, palette);
        let subtitle = match (running, finished) {
            (0, 0) => t("Nothing running").to_string(),
            (r, 0) => tf("{n} running", &[("n", &r)]),
            (0, f) => tf("{n} finished", &[("n", &f)]),
            (r, f) => tf("{running} running · {finished} finished", &[("running", &r), ("finished", &f)]),
        };
        let subtitle = if waiting > 0 {
            format!("{} · {}", subtitle, tf("{n} waiting", &[("n", &waiting)]))
        } else {
            subtitle
        };
        let trailing = (finished > 0).then(|| {
            pill_button("progress-clear", Some("trash"), t("Clear finished"), accent, palette, |_, cx| {
                let _ = change(cx, |b, _| Ok(b.clear_finished()));
            })
            .into_any_element()
        });
        let mono = fonts(cx).mono.clone();
        let body: AnyElement = if rows.is_empty() {
            empty_state("gauge", t("No tasks yet"), t("Scripts report here: mandara msg progress start build"), EMPTY_H, palette).into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(ROW_GAP))
                .children(rows.iter().enumerate().map(|(i, r)| reveal(&self.p, i + 1, row(i, r, now, palette, &mono))))
                .when(more > 0, |el| el.child(caption(tf("+{n} more (mandara msg progress list)", &[("n", &more)]), palette).h(px(MORE_H)).flex().items_center().justify_center()))
                .into_any_element()
        };
        div()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(reveal(&self.p, 0, header("gauge", accent, t("Task progress"), subtitle, palette, trailing)))
            .child(body)
            .into_any_element()
    }
}

/// The user-tunable look of the running light, read once per config snapshot
/// rather than nine times per animated frame.
#[derive(Clone, Copy)]
struct LightLook {
    width: f32,
    height: f32,
    gap: f32,
    period: f32,
    phase: f32,
    min_alpha: f32,
    max_alpha: f32,
    idle_alpha: f32,
}

/// Keep the pulse range ordered: a min above max must not invert the fade.
fn ordered_brightness(min: f32, max: f32) -> (f32, f32) {
    (min.min(max), max)
}

/// Read the look (and the idle-ring toggle) from one config snapshot. Kept
/// pure so the bounded settings are covered by a unit test.
fn light_look_from(config: &ShellConfig) -> (LightLook, bool) {
    let p = &config.plugins;
    let num = |key: &str, default: f64, range: (f64, f64)| -> f32 {
        p.setting(ID, key, default).clamp(range.0, range.1) as f32
    };
    let max_alpha = num("light_max_alpha", LIGHT_MAX_ALPHA_DEFAULT, LIGHT_ALPHA_RANGE);
    let min_alpha = num("light_min_alpha", LIGHT_MIN_ALPHA_DEFAULT, LIGHT_ALPHA_RANGE);
    let (min_alpha, max_alpha) = ordered_brightness(min_alpha, max_alpha);
    (
        LightLook {
            width: num("light_width", LIGHT_W_DEFAULT, LIGHT_W_RANGE),
            height: num("light_height", LIGHT_H_DEFAULT, LIGHT_H_RANGE),
            gap: num("light_gap", LIGHT_GAP_DEFAULT, LIGHT_GAP_RANGE),
            period: num("light_period", LIGHT_PERIOD_DEFAULT, LIGHT_PERIOD_RANGE),
            phase: num("light_phase", LIGHT_PHASE_DEFAULT, LIGHT_PHASE_RANGE),
            min_alpha,
            max_alpha,
            idle_alpha: num("light_idle_alpha", LIGHT_IDLE_ALPHA_DEFAULT, LIGHT_ALPHA_RANGE),
        },
        p.setting(ID, "show_when_idle", false),
    )
}

#[derive(Default)]
struct LightLookCache {
    config: Option<Arc<ShellConfig>>,
    look: Option<LightLook>,
    show_when_idle: bool,
}

fn light_look(cx: &App) -> (LightLook, bool) {
    thread_local! {
        static CACHE: RefCell<LightLookCache> = RefCell::default();
    }
    // `ConfigStore::update` replaces its `Arc` on every real change, so
    // pointer identity invalidates the cache exactly when a setting changes.
    let config = crate::state::config(cx).read(cx).snapshot();
    CACHE.with(|cell| {
        let mut cache = cell.borrow_mut();
        if let (Some(previous), Some(look)) = (&cache.config, cache.look)
            && Arc::ptr_eq(previous, &config)
        {
            return (look, cache.show_when_idle);
        }
        let (look, show_when_idle) = light_look_from(&config);
        cache.config = Some(config);
        cache.look = Some(look);
        cache.show_when_idle = show_when_idle;
        (look, show_when_idle)
    })
}

/// How the eight-cell light is drawn.
#[derive(Clone, Copy)]
enum LightMode {
    /// Quiet, dim row while no task is showing.
    Idle,
    /// Soft brightness ripple while a task is active.
    Pulse,
    /// All cells bright and steady: waiting (amber), done (green) or failed (red).
    Solid,
}

/// The eight-cell running light: cells stay in place and breathe in
/// brightness while a task is active, or hold steady for idle / done / failed.
/// Painted as one canvas so a 33 ms frame rebuilds one element, not eight
/// layout nodes with eight colour conversions.
fn running_light(color: Color, mode: LightMode, look: &LightLook) -> AnyElement {
    let (width, height, gap) = (look.width, look.height, look.gap);
    let (period, phase) = (look.period, look.phase);
    let (min_alpha, max_alpha, idle_alpha) = (look.min_alpha, look.max_alpha, look.idle_alpha);
    let total = width * LIGHT_CELLS as f32 + gap * (LIGHT_CELLS as f32 - 1.0);
    let radius = px(width.min(height) * 0.5);
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            // Monotonic seconds: Unix seconds in f32 lose the fractional part
            // at this magnitude, so the pulse would appear frozen for minutes.
            let seconds = clock_s();
            for i in 0..LIGHT_CELLS {
                let alpha = match mode {
                    LightMode::Idle => idle_alpha,
                    LightMode::Solid => max_alpha,
                    LightMode::Pulse => {
                        // A small phase offset gives a soft ripple without any
                        // cell travelling left or right.
                        let p = seconds / period * TAU - i as f32 * phase;
                        min_alpha + (max_alpha - min_alpha) * (0.5 + 0.5 * p.cos())
                    }
                };
                let x = px(i as f32 * (width + gap));
                let rect = Bounds::new(bounds.origin + point(x, px(0.0)), size(px(width), px(height)));
                window.paint_subpixel_quad(fill(rect, color.with_alpha(alpha).hsla()).corner_radii(radius));
            }
        },
    )
    .w(px(total))
    .h(px(height))
    .into_any_element()
}

/// The bar chip: an animated running light for an indeterminate task (a ring
/// once it has a value), `+N` for the others, a solid amber light while a task
/// waits for the user, a solid green / red light for 30 s after the last task
/// finished, and — when `show_when_idle` is on — a quiet dim light while nothing
/// is running.
fn bar_view(palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    /// What the chip shows.
    enum Show {
        Running { fraction: Option<f64>, tint: Color, others: usize },
        Waiting { tint: Color, others: usize },
        Finished(bool),
        Nothing,
    }
    let now = now_ms();
    let (look, show_when_idle) = light_look(cx);
    let m = model(cx);
    let (show, next) = {
        let b = &m.read(cx).board;
        let mut running = b.running();
        let first = running.next();
        // A decision takes priority over background work: the light holds
        // steady amber until the user answers instead of breathing on.
        let show = if let Some(task) = b.waiting().next() {
            Show::Waiting { tint: tint(task, now, palette), others: b.running().count() + b.waiting().count() - 1 }
        } else {
            match first {
                Some(task) => Show::Running { fraction: task.fraction(), tint: tint(task, now, palette), others: running.count() },
                None => b.tasks.iter().find(|t| t.in_bar(now)).map_or(Show::Nothing, |t| Show::Finished(t.status == Status::Done)),
            }
        };
        (show, b.next_change(now))
    };
    if let Some(at) = next {
        redraw_in(Duration::from_millis(u64::try_from(at - now).unwrap_or(0)) + EXPIRY_MARGIN, window, cx);
    }
    let chip = div().flex().items_center().gap(px(BAR_GAP_PX)).font_features(tabular_nums());
    match show {
        Show::Running { fraction, tint, others } => {
            let color = readable(tint, palette);
            let track = palette.on_surface.with_alpha(RING_TRACK_ALPHA);
            let d = crate::style::bar_icon(BAR_RING_PX);
            let ring_el = match fraction {
                Some(f) => ring(f as f32, d, BAR_RING_STROKE_PX, color, track).into_any_element(),
                None => {
                    redraw_in(ANIMATION_FRAME, window, cx);
                    running_light(color, LightMode::Pulse, &look)
                }
            };
            chip.child(ring_el)
                .when_some(fraction, |el, f| el.child(div().font_weight(FontWeight::SEMIBOLD).child(percent_text(f as f32))))
                .when(others > 0, |el| el.child(div().text_size(px(BAR_MORE_PX)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(format!("+{others}"))))
                .into_any_element()
        }
        Show::Waiting { tint, others } => {
            // Hold the light steady: the agent is blocked on a decision, so
            // breathing would wrongly read as active work.
            let color = readable(tint, palette);
            chip.child(running_light(color, LightMode::Solid, &look))
                .when(others > 0, |el| el.child(div().text_size(px(BAR_MORE_PX)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(format!("+{others}"))))
                .into_any_element()
        }
        Show::Finished(ok) => {
            // Keep the same light vocabulary: solid green for done, red for
            // failed, instead of a one-off check / alert icon.
            let color = readable(if ok { palette.success_emerald } else { palette.urgent_coral }, palette);
            chip.child(running_light(color, LightMode::Solid, &look)).into_any_element()
        }
        Show::Nothing => {
            // Default off: leave the pill empty so existing installs don't
            // suddenly get a permanent chip. When on, the dim light row keeps
            // the same vocabulary as running / done / failed.
            if show_when_idle {
                chip.child(running_light(readable(palette.on_surface, palette), LightMode::Idle, &look)).into_any_element()
            } else {
                chip.into_any_element()
            }
        }
    }
}

// ── Registration ─────────────────────────────────────────────────────────

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Task progress"), tk("A bar pill that scripts, builds and AI agents report progress to"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let m = model(cx);
        let bar = fn_view(None, bar_view, window, cx);
        follow(&bar, &m, cx);
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(panel_height(0, false)))));
        let view = PanelView { p: panel_cx.clone(), size_cell: size_cell.clone(), cache: RefCell::default() };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        follow(&panel, &m, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
    });
    let id_arg = || ArgSpec::required("id", ArgKind::String, tk("task id, e.g. build"));
    r.command(
        ID,
        CommandSpec::new("progress start", tk("Start a task (indeterminate until a value is set)"), |a, cx| {
            let id = a.str(0)?.to_string();
            let label = a.rest(1);
            let task = change(cx, |b, now| b.start(&id, &label, now).cloned())?;
            Ok(task_json(&task, now_ms()))
        })
        .arg(id_arg())
        .arg(ArgSpec::optional("label", ArgKind::String, tk("what it is doing")))
        .example("progress start build Building the release"),
    );
    r.command(
        ID,
        CommandSpec::new("progress set", tk("Set a task's progress: a percent (40 or 40%) or a count (3/10), optionally a new label"), |a, cx| {
            let id = a.str(0)?.to_string();
            let amount = pg::parse_amount(a.str(1)?)?;
            let label = a.rest(2);
            let task = change(cx, |b, now| b.set(&id, amount, &label, now).cloned())?;
            Ok(task_json(&task, now_ms()))
        })
        .arg(id_arg())
        .arg(ArgSpec::required("value", ArgKind::String, tk("percent (40, 40%) or count (3/10)")))
        .arg(ArgSpec::optional("label", ArgKind::String, tk("new label")))
        .example("progress set build 40%")
        .example("progress set tests 120/300 Running tests"),
    );
    r.command(
        ID,
        CommandSpec::new("progress touch", tk("Refresh a task's activity without changing its progress"), |a, cx| {
            let id = a.str(0)?.to_string();
            let label = a.rest(1);
            let task = change(cx, |b, now| b.touch(&id, &label, now).cloned())?;
            Ok(task_json(&task, now_ms()))
        })
        .arg(id_arg())
        .arg(ArgSpec::optional("label", ArgKind::String, tk("what it is doing")))
        .example("progress touch build Still compiling"),
    );
    r.command(
        ID,
        CommandSpec::new("progress wait", tk("Waiting"), |a, cx| {
            let id = a.str(0)?.to_string();
            let label = a.rest(1);
            let task = change(cx, |b, now| b.wait(&id, &label, now).cloned())?;
            Ok(task_json(&task, now_ms()))
        })
        .arg(id_arg())
        .arg(ArgSpec::optional("label", ArgKind::String, tk("what it is doing")))
        .example("progress wait build Needs confirmation"),
    );
    r.command(
        ID,
        CommandSpec::new("progress done", tk("Mark a task finished (posts a notification)"), |a, cx| {
            let id = a.str(0)?.to_string();
            let message = a.rest(1);
            let task = change(cx, |b, now| b.done(&id, &message, now))?;
            announce(&task, cx);
            Ok(task_json(&task, now_ms()))
        })
        .arg(id_arg())
        .arg(ArgSpec::optional("message", ArgKind::String, tk("result to show")))
        .example("progress done build All green"),
    );
    r.command(
        ID,
        CommandSpec::new("progress fail", tk("Mark a task failed (posts a notification)"), |a, cx| {
            let id = a.str(0)?.to_string();
            let message = a.rest(1);
            let task = change(cx, |b, now| b.fail(&id, &message, now))?;
            announce(&task, cx);
            Ok(task_json(&task, now_ms()))
        })
        .arg(id_arg())
        .arg(ArgSpec::optional("message", ArgKind::String, tk("what went wrong")))
        .example("progress fail build 3 tests failed"),
    );
    r.command(
        ID,
        CommandSpec::new("progress remove", tk("Remove a task from the bar and the panel"), |a, cx| {
            let id = a.rest(0);
            change(cx, |b, _| b.remove(&id))?;
            Ok(json!({ "removed": id }))
        })
        .arg(id_arg())
        .example("progress remove build"),
    );
    r.command(
        ID,
        CommandSpec::new("progress list", tk("Every task with its progress, elapsed time, ETA and status (JSON)"), |_, cx| {
            let now = now_ms();
            let m = model(cx);
            Ok(json!(m.read(cx).board.tasks.iter().map(|t| task_json(t, now)).collect::<Vec<_>>()))
        })
        .example("progress list"),
    );
    r.command(
        ID,
        CommandSpec::new("progress clear", tk("Remove every finished task"), |_, cx| {
            let n = change(cx, |b, _| Ok(b.clear_finished()))?;
            Ok(json!({ "cleared": n }))
        })
        .example("progress clear"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_range_stays_ordered() {
        // The defaults and other valid pairs are untouched.
        assert_eq!(ordered_brightness(0.3, 1.0), (0.3, 1.0));
        assert_eq!(ordered_brightness(0.4, 0.8), (0.4, 0.8));
        // A reversed pair collapses to a steady light at the maximum.
        assert_eq!(ordered_brightness(0.9, 0.2), (0.2, 0.2));
    }

    /// A locale change rebuilds text; a theme frame reuses it with live semantic colours.
    #[test]
    fn row_cache_tracks_locale_without_freezing_theme_colours() {
        let mut board = Board::default();
        board.start("build", "Build", 1).unwrap();
        let mut cache = RowCache::default();
        let english = {
            let _language = mandara_i18n::scoped(Some(Lang::En));
            let rows = cache.get(&board, 1, 1000);
            assert!(Rc::ptr_eq(&rows, &cache.get(&board, 1, 1000)));
            assert_eq!(rows[0].state.label(1000), "Running");
            rows
        };
        let _language = mandara_i18n::scoped(Some(Lang::Ar));
        let arabic = cache.get(&board, 1, 1000);
        assert!(!Rc::ptr_eq(&english, &arabic));
        assert_eq!(arabic[0].state.label(1000), mandara_i18n::t_in(Lang::Ar, "Running"));
        let theme = &mandara_theme::catalog::builtin()[0];
        let light = theme.palette(false);
        let dark = theme.palette(true);
        assert_ne!(light.primary, dark.primary, "fixture palettes must have distinct primary tokens");
        assert_eq!(arabic[0].state.color(1000, &light), light.primary);
        assert_eq!(arabic[0].state.color(1000, &dark), dark.primary);
        assert!(Rc::ptr_eq(&arabic, &cache.get(&board, 1, 1000)), "theme sampling must not reformat unchanged text");
        let later = cache.get(&board, 1, 2000);
        assert!(!Rc::ptr_eq(&arabic, &later));
        let changed = cache.get(&board, 2, 2000);
        assert!(!Rc::ptr_eq(&later, &changed));
        eprintln!("progress rows: cached RowText={} B, semantic TaskState={} B, RowCache={} B", size_of::<RowText>(), size_of::<TaskState>(), size_of::<RowCache>());
    }

    #[test]
    fn panel_height_fits_rows() {
        assert_eq!(panel_height(0, false), PAD * 2.0 + HEADER_H + GAP + EMPTY_H);
        assert_eq!(panel_height(2, false), PAD * 2.0 + HEADER_H + GAP + ROW_H * 2.0 + ROW_GAP);
        assert!(panel_height(MAX_ROWS, true) > panel_height(MAX_ROWS, false));
    }

    #[test]
    fn show_when_idle_defaults_off() {
        let cfg = mandara_config::ShellConfig::default();
        assert!(!cfg.plugins.setting(ID, "show_when_idle", false));
    }

    /// The cached look must carry every setting, including the idle toggle,
    /// so the bar does not re-read the config eight times per frame.
    #[test]
    fn light_look_snapshot_carries_all_settings() {
        let cfg = ShellConfig::default();
        let (look, show_when_idle) = light_look_from(&cfg);
        assert_eq!(look.width, LIGHT_W_DEFAULT as f32);
        assert_eq!(look.height, LIGHT_H_DEFAULT as f32);
        assert_eq!(look.gap, LIGHT_GAP_DEFAULT as f32);
        assert_eq!(look.period, LIGHT_PERIOD_DEFAULT as f32);
        assert_eq!(look.phase, LIGHT_PHASE_DEFAULT as f32);
        assert_eq!(look.min_alpha, LIGHT_MIN_ALPHA_DEFAULT as f32);
        assert_eq!(look.max_alpha, LIGHT_MAX_ALPHA_DEFAULT as f32);
        assert_eq!(look.idle_alpha, LIGHT_IDLE_ALPHA_DEFAULT as f32);
        assert!(!show_when_idle);
    }

    /// Indeterminate rows are exactly the cached rows with no fraction; this
    /// replaces a second scan of the whole board on every panel render.
    #[test]
    fn cached_rows_report_indeterminate_work() {
        let row = |fraction| RowText {
            id: "build".into(),
            label: "Build".into(),
            amount: None,
            detail: "".into(),
            fraction,
            state: TaskState::Done,
            finished: false,
        };
        assert!(row(None).fraction.is_none());
        assert!(!row(Some(0.0)).fraction.is_none());
    }
}
