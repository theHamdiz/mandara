//! Task progress (built-in `progress` plugin, opt-in): a bar pill that
//! scripts, long builds and AI agents push progress to with
//! `mandara msg progress …`.
//!
//! Tasks live in one shared in-memory board
//! ([`mandara_services::progress::Board`]); nothing is polled. The bar chip
//! shows a ring with the percent of the most recent running task (a spinner
//! while it has no value yet) and `+N` for the others; finished tasks stay
//! in the bar for 30 s and in the panel until cleared. A task with no update
//! for 10 minutes shows as stalled. The panel lists every task with an OKLCH
//! gradient bar (a shimmer when the amount is unknown), elapsed time, an ETA
//! from the rate so far and a status pill.
//!
//! Timers: the only animation (spinner / shimmer) ticks while an
//! indeterminate task is on screen; elapsed times tick once a second while
//! the panel is open; otherwise one timer waits for the next stall or for a
//! finished task to leave the bar.
//!
//! Settings (`[plugins.settings.progress]`): `notify` (post a desktop
//! notification when a task finishes or fails, default on).
//!
//! Commands: `progress start <id> [label…]`, `progress set <id> <value>
//! [label…]` (`40`, `40%` or `3/10`), `progress done <id> [message…]`,
//! `progress fail <id> [message…]`, `progress remove <id>`, `progress list`,
//! `progress clear`.

use std::cell::{Cell, RefCell};
use std::f32::consts::TAU;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext as _, Entity, FontWeight, Global, IntoElement, ParentElement, SharedString, Styled, Window, canvas, div,
    prelude::FluentBuilder as _, px, relative, size,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::NotificationCommand;
use mandara_services::progress::{self as pg, Amount, Board, Status, Task};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::extras::caption;
use super::panel_fx::{ANGLE_RIGHT_DEG, EXPIRY_MARGIN, HEADER_H, duration_short, empty_state, glass_card, header, partner, percent_text, pill_button, readable, redraw_in, wash, WASH_TRACK};
use super::{fn_view, follow, json};
use crate::fonts::{fonts, tabular_nums};
use crate::icons::{icon, ring, stroke_arc};
use crate::kit::{icon_button, reveal, text};

pub const ID: &str = "progress";

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
const BAR_ICON_PX: f32 = 14.0;
const BAR_MORE_PX: f32 = 10.5;
const RING_TRACK_ALPHA: f32 = 0.16;
/// Indeterminate spinner: lit share of the circle and turns per second.
const SPIN_SWEEP: f32 = 0.28;
const SPIN_TURNS_PER_S: f32 = 0.9;
/// Shimmer: segment width (share of the track) and passes per second.
const SHIMMER_W: f32 = 0.32;
const SHIMMER_PER_S: f32 = 0.7;
/// The spinner and shimmer redraw at this pace (smooth, but not the full
/// display rate for a pill in the bar).
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
    crate::plugins::notifications::send(cx, NotificationCommand::Post { app: t("Task progress").into(), icon: if ok { "check" } else { "alert" }.into(), summary, body });
}

fn task_json(task: &Task, now: i64) -> serde_json::Value {
    let mut v = json(task);
    v["fraction"] = json!(task.fraction());
    v["percent"] = json!(task.fraction().map(|f| (f * 100.0).round()));
    v["elapsed_s"] = json!(task.elapsed_ms(now) / MS_PER_S);
    v["eta_s"] = json!(task.eta_ms().map(|e| e / MS_PER_S));
    v["stalled"] = json!(task.stalled(now));
    v
}

// ── Look ─────────────────────────────────────────────────────────────────

/// A task's state colour (not yet made readable).
fn tint(task: &Task, now: i64, palette: &MandaraPalette) -> Color {
    match task.status {
        Status::Done => palette.success_emerald,
        Status::Failed => palette.urgent_coral,
        Status::Running if task.stalled(now) => palette.warning_amber,
        Status::Running => palette.primary,
    }
}

fn status_label(task: &Task, now: i64) -> &'static str {
    match task.status {
        Status::Done => t("Done"),
        Status::Failed => t("Failed"),
        Status::Running if task.stalled(now) => t("Stalled"),
        Status::Running => t("Running"),
    }
}

/// Seconds since the process started, for animation phases.
fn clock_s() -> f32 {
    thread_local! {
        static EPOCH: Instant = Instant::now();
    }
    EPOCH.with(|e| e.elapsed().as_secs_f32())
}

/// An indeterminate spinner: a faint track and a turning arc.
fn spinner(d: f32, stroke: f32, color: Color, track: Color) -> impl IntoElement {
    let phase = (clock_s() * SPIN_TURNS_PER_S).fract() * TAU;
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            stroke_arc(window, bounds, stroke, 0.0, TAU, track);
            stroke_arc(window, bounds, stroke, phase, TAU * SPIN_SWEEP, color);
        },
    )
    .size(px(d))
    .flex_none()
}

/// The panel's progress bar: an OKLCH gradient fill, or a shimmer sweeping
/// the track while the amount is unknown.
fn progress_bar(fraction: Option<f64>, color: Color, palette: &MandaraPalette) -> AnyElement {
    let track = div().relative().w_full().h(px(BAR_TRACK_H)).rounded_full().overflow_hidden().bg(wash(WASH_TRACK, palette).hsla());
    let fill = oklab_gradient(ANGLE_RIGHT_DEG, color, partner(color).with_lightness(color.lightness()));
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
    status: &'static str,
    amount: Option<SharedString>,
    detail: SharedString,
    fraction: Option<f64>,
    tint: Color,
    finished: bool,
}

fn row_text(task: &Task, now: i64, palette: &MandaraPalette) -> RowText {
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
        status: status_label(task, now),
        amount,
        detail: detail.join(" · ").into(),
        // Only a running task without a value shimmers; a finished one sits still.
        fraction: task.fraction().or((task.status != Status::Running).then_some(0.0)),
        tint: tint(task, now, palette),
        finished: task.status != Status::Running,
    }
}

fn row(i: usize, r: &RowText, palette: &MandaraPalette, mono: &SharedString) -> AnyElement {
    let tint = readable(r.tint, palette);
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
        .child(r.status);
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
        .child(progress_bar(r.fraction, r.tint, palette))
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

struct PanelView {
    p: PanelCx,
    size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>,
    /// Rows formatted for `(revision, second)`.
    cache: RefCell<Option<((u64, i64), Rc<[RowText]>)>>,
}

impl PanelView {
    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let now = now_ms();
        let m = model(cx);
        let (rows, more, running, finished, any_unknown, next) = {
            let pr = m.read(cx);
            let b = &pr.board;
            // Running first (newest first), then finished.
            let mut order: Vec<&Task> = b.running().collect();
            order.extend(b.tasks.iter().filter(|t| t.status != Status::Running));
            let more = order.len().saturating_sub(MAX_ROWS);
            order.truncate(MAX_ROWS);
            let key = (pr.rev, now / MS_PER_S);
            let mut cache = self.cache.borrow_mut();
            let rows = match cache.as_ref() {
                Some((k, rows)) if *k == key => rows.clone(),
                _ => {
                    let rows: Rc<[RowText]> = order.iter().map(|t| row_text(t, now, palette)).collect();
                    *cache = Some((key, rows.clone()));
                    rows
                }
            };
            let running = b.running().count();
            (rows, more, running, b.tasks.len() - running, order.iter().any(|t| t.status == Status::Running && t.amount.is_none()), b.next_change(now))
        };
        if self.p.progress() > 0.0 {
            if any_unknown {
                redraw_in(ANIMATION_FRAME, window, cx);
            } else if running > 0 {
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
                .children(rows.iter().enumerate().map(|(i, r)| reveal(&self.p, i + 1, row(i, r, palette, &mono))))
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

/// The bar chip: a ring for the newest running task, `+N` for the others,
/// a ✓ / ! for 30 s after the last one finished, nothing when idle.
fn bar_view(palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    /// What the chip shows.
    enum Show {
        Running { fraction: Option<f64>, tint: Color, others: usize },
        Finished(bool),
        Nothing,
    }
    let now = now_ms();
    let m = model(cx);
    let (show, next) = {
        let b = &m.read(cx).board;
        let mut running = b.running();
        let show = match running.next() {
            Some(task) => Show::Running { fraction: task.fraction(), tint: tint(task, now, palette), others: running.count() },
            None => b.tasks.iter().find(|t| t.in_bar(now)).map_or(Show::Nothing, |t| Show::Finished(t.status == Status::Done)),
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
                    spinner(d, BAR_RING_STROKE_PX, color, track).into_any_element()
                }
            };
            chip.child(ring_el)
                .when_some(fraction, |el, f| el.child(div().font_weight(FontWeight::SEMIBOLD).child(percent_text(f as f32))))
                .when(others > 0, |el| el.child(div().text_size(px(BAR_MORE_PX)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(format!("+{others}"))))
                .into_any_element()
        }
        Show::Finished(ok) => {
            let color = readable(if ok { palette.success_emerald } else { palette.urgent_coral }, palette);
            chip.child(icon(if ok { "check" } else { "alert" }, crate::style::bar_icon(BAR_ICON_PX), color)).into_any_element()
        }
        Show::Nothing => div().into_any_element(),
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
    fn panel_height_fits_rows() {
        assert_eq!(panel_height(0, false), PAD * 2.0 + HEADER_H + GAP + EMPTY_H);
        assert_eq!(panel_height(2, false), PAD * 2.0 + HEADER_H + GAP + ROW_H * 2.0 + ROW_GAP);
        assert!(panel_height(MAX_ROWS, true) > panel_height(MAX_ROWS, false));
    }
}
