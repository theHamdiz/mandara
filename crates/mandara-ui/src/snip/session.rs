//! The running snipping tool (one at a time): its [`Stage`], the capture
//! mode it resumes with, and the surface each stage shows: the frozen
//! overlay while snipping, the compact toolbar while idle, the countdown
//! pill before a delayed capture. Nothing of the tool is on screen while a
//! frame is taken.
//!
//! Every change goes through [`send`], which defers to the end of the
//! current update: views call it from their own listeners, and closing a
//! window while it is being updated is not allowed.

use std::time::Duration;

use async_channel::Sender;
use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use gpui::{App, AppContext as _, Global, WindowHandle, px, size};
use mandara_config::workflow::{ScreenshotSettings, ToolbarPosition};
use mandara_core::{LayerSurfaceSpec, SurfaceRole};
use mandara_i18n::{t, tf};

use super::dock::{CountdownView, DockView};
use super::flow::{Input, Stage};
use super::{Preselect, SnipMode, SnipView, frozen_image, report};

/// After the idle toolbar or the countdown unmaps, the compositor needs a
/// frame or two before a capture no longer shows it.
const HIDE_SETTLE: Duration = Duration::from_millis(150);
/// The frozen overlay fades out before the next frame is taken.
const OVERLAY_SETTLE: Duration = super::RECORD_START_DELAY;
/// Countdown step.
const TICK: Duration = Duration::from_secs(1);

/// Something to do once the screen is clear, instead of opening the
/// overlay (a command's direct window / screen capture).
pub(crate) type Job = Box<dyn FnOnce(&mut App)>;

struct Session {
    stage: Stage,
    /// Bumped on every transition except ticks and the frame arriving, so
    /// stale timers and frames drop out.
    generation: u64,
    mode: SnipMode,
    record: bool,
    /// What the next overlay opens with selected (the first one only).
    preselect: Preselect,
    job: Option<Job>,
    /// The command waiting for the overlay to open.
    reply: Option<Sender<Result<String, String>>>,
    snip: Option<WindowHandle<SnipView>>,
    dock: Option<WindowHandle<DockView>>,
    countdown: Option<WindowHandle<CountdownView>>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            stage: Stage::Closed,
            generation: 0,
            mode: SnipMode::Region,
            record: false,
            preselect: Preselect::None,
            job: None,
            reply: None,
            snip: None,
            dock: None,
            countdown: None,
        }
    }
}

impl Global for Session {}

fn settings(cx: &App) -> ScreenshotSettings {
    ScreenshotSettings::from_plugins(&crate::state::config(cx).read(cx).get().plugins)
}

/// Starts the tool: capture now (`secs` 0) or after a countdown. A `job`
/// replaces the overlay; `reply` hears when the overlay opens (or that the
/// capture was cancelled).
pub(crate) fn begin(mode: SnipMode, record: bool, preselect: Preselect, secs: u64, job: Option<Job>, reply: Option<Sender<Result<String, String>>>, cx: &mut App) {
    let stale = {
        let s = cx.default_global::<Session>();
        s.mode = mode;
        s.record = record;
        s.preselect = preselect;
        s.job = job;
        std::mem::replace(&mut s.reply, reply)
    };
    if let Some(tx) = stale {
        let _ = tx.try_send(Err("capture cancelled".into()));
    }
    send(Input::Delay(u32::try_from(secs).unwrap_or(u32::MAX)), cx);
}

/// Applies `input` once the current update is over.
pub(super) fn send(input: Input, cx: &mut App) {
    cx.defer(move |cx| apply(input, cx));
}

/// The mode the tool resumes with.
pub(super) fn mode(cx: &App) -> SnipMode {
    cx.try_global::<Session>().map_or(SnipMode::Region, |s| s.mode)
}

pub(super) fn set_mode(mode: SnipMode, cx: &mut App) {
    cx.default_global::<Session>().mode = mode;
}

/// Leaves snipping (Esc / pause) and keeps the tool as the idle toolbar.
pub(super) fn pause(mode: SnipMode, input: Input, cx: &mut App) {
    set_mode(mode, cx);
    send(input, cx);
}

/// "Capture in N s" from either toolbar.
pub(super) fn capture_in(cx: &mut App) {
    let secs = settings(cx).countdown_secs();
    send(Input::Delay(u32::try_from(secs).unwrap_or(u32::MAX)), cx);
}

/// Moves the capture toolbar to the other screen edge (saved to
/// `screenshot.toolbar_position`) and returns the new edge. The idle
/// toolbar is reopened there (a layer surface cannot change its anchor).
pub(super) fn flip_position(cx: &mut App) -> ToolbarPosition {
    let next = settings(cx).flipped_position();
    let value = toml::Value::try_from(next).unwrap_or_else(|_| toml::Value::String("top".into()));
    crate::state::update_config(cx, |c| {
        c.plugins.settings.entry(crate::plugins::screenshot::ID.to_string()).or_default().insert("toolbar_position".into(), value);
    });
    cx.defer(|cx| {
        let old = {
            let s = cx.default_global::<Session>();
            if s.stage != Stage::Idle {
                return;
            }
            s.dock.take()
        };
        if let Some(h) = old {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
        open_dock(cx);
    });
    next
}

fn current(generation: u64, stage: Stage, cx: &App) -> bool {
    cx.try_global::<Session>().is_some_and(|s| s.generation == generation && s.stage == stage)
}

fn apply(input: Input, cx: &mut App) {
    let s = cx.default_global::<Session>();
    let from = s.stage;
    let to = from.next(input);
    let restart = matches!(input, Input::Capture | Input::Delay(_));
    if to == from && !restart {
        return;
    }
    tracing::debug!(target: "mandara::snip", ?from, ?input, ?to, "stage");
    if !matches!(input, Input::Tick | Input::Captured) {
        s.generation += 1;
    }
    s.stage = to;
    let generation = s.generation;
    // Surfaces the new stage doesn't show go away.
    let snip = if to == Stage::Snipping { None } else { s.snip.take() };
    let dock = if to == Stage::Idle { None } else { s.dock.take() };
    let countdown = if matches!(to, Stage::Countdown { .. }) { None } else { s.countdown.take() };
    // A pending command reply or job only lives until the tool settles.
    let reply = if matches!(to, Stage::Closed | Stage::Idle) {
        s.job = None;
        s.preselect = Preselect::None;
        s.reply.take()
    } else {
        None
    };
    if let Some(h) = snip {
        let _ = h.update(cx, |view, _, cx| {
            view.closing = true;
            cx.notify();
        });
    }
    for h in dock.map(|h| h.into()).into_iter().chain(countdown.map(|h| h.into())) {
        let h: gpui::AnyWindowHandle = h;
        let _ = h.update(cx, |_, window, _| window.remove_window());
    }
    if let Some(tx) = reply {
        let _ = tx.try_send(Err("capture cancelled".into()));
    }
    match to {
        Stage::Capturing => {
            let settle = match from {
                Stage::Snipping => OVERLAY_SETTLE,
                stage if stage.visible() => HIDE_SETTLE,
                _ => Duration::ZERO,
            };
            capture(generation, settle, cx);
        }
        Stage::Idle => {
            if cx.default_global::<Session>().dock.is_none() {
                open_dock(cx);
            }
        }
        Stage::Countdown { left, .. } => {
            let open = cx.default_global::<Session>().countdown;
            match open {
                Some(h) => {
                    let _ = h.update(cx, |view, _, cx| view.set_left(left, cx));
                }
                None => open_countdown(left, cx),
            }
            if input != Input::Tick {
                tick(generation, cx);
            }
        }
        Stage::Closed | Stage::Snipping => {}
    }
}

/// Counts the countdown down once a second while it is the same one.
fn tick(generation: u64, cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(TICK).await;
            let more = cx.update(|cx| {
                let counting = |cx: &App| cx.try_global::<Session>().is_some_and(|s| s.generation == generation && matches!(s.stage, Stage::Countdown { .. }));
                if !counting(cx) {
                    return false;
                }
                apply(Input::Tick, cx);
                counting(cx)
            });
            if !more {
                break;
            }
        }
    })
    .detach();
}

/// Takes a fresh frame once the tool's surfaces are gone, then opens the
/// overlay on it (or runs the pending job).
fn capture(generation: u64, settle: Duration, cx: &mut App) {
    let monitor = crate::hub::hub(cx).read(cx).focused.as_ref().map(|c| c.monitor.clone()).unwrap_or_default();
    let (record, job) = {
        let s = cx.default_global::<Session>();
        let job = s.job.take();
        // A job answers its command itself; the reply kept here was only
        // for a cancelled countdown.
        if job.is_some() {
            s.reply = None;
        }
        (s.record, job)
    };
    let cursor = !record && settings(cx).cursor;
    cx.spawn(async move |cx| {
        if !settle.is_zero() {
            cx.background_executor().timer(settle).await;
        }
        if !cx.update(|cx| current(generation, Stage::Capturing, cx)) {
            return;
        }
        if let Some(job) = job {
            cx.update(|cx| {
                job(cx);
                apply(Input::Close, cx);
            });
            return;
        }
        let task = cx.background_executor().spawn(async move {
            mandara_plugins::guard("screen capture", || mandara_services::screencopy::capture_all_with(cursor).map_err(|e| format!("{e:#}")))
                .unwrap_or_else(|| Err(t("internal error while capturing (logged)").into()))
        });
        let captures = task.await;
        cx.update(|cx| {
            if !current(generation, Stage::Capturing, cx) {
                return;
            }
            let fail = |err: String, cx: &mut App| {
                report(Err(err), cx);
                apply(Input::Close, cx);
            };
            let captures = match captures {
                Ok(c) => c,
                Err(err) => return fail(tf("could not capture the screen: {error}", &[("error", &err)]), cx),
            };
            let Some(capture) = captures.iter().find(|c| c.output == monitor).or(captures.first()).cloned() else {
                return fail(t("no outputs to capture").into(), cx);
            };
            let Some(frozen) = frozen_image(&capture) else {
                return fail(t("the compositor returned an unexpected frame size").into(), cx);
            };
            open_overlay(capture, frozen, cx);
        });
    })
    .detach();
}

fn open_overlay(capture: mandara_services::screencopy::Capture, frozen: std::sync::Arc<gpui::RenderImage>, cx: &mut App) {
    let (mode, record, preselect) = {
        let s = cx.default_global::<Session>();
        (s.mode, s.record, std::mem::replace(&mut s.preselect, Preselect::None))
    };
    let (w, h) = (capture.logical.2.max(1) as f32, capture.logical.3.max(1) as f32);
    let mut spec = LayerSurfaceSpec::overlay(SurfaceRole::Lock, size(px(w), px(h)), None).with_keyboard(KeyboardInteractivity::Exclusive);
    spec.layer = Layer::Overlay;
    spec.anchor = Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT;
    match cx.open_window(spec.window_options(), move |window, cx| cx.new(|cx| SnipView::new(capture, frozen, mode, record, preselect, window, cx))) {
        Ok(handle) => {
            let reply = {
                let s = cx.default_global::<Session>();
                s.snip = Some(handle);
                s.reply.take()
            };
            if let Some(tx) = reply {
                let _ = tx.try_send(Ok("snipping tool opened".into()));
            }
            apply(Input::Captured, cx);
        }
        Err(e) => {
            tracing::error!("snip overlay: {e:#}");
            apply(Input::Close, cx);
        }
    }
}

/// The toolbar's anchor and the surface margin for a compact snip surface.
fn edge_spec(bottom: bool, surface: gpui::Size<gpui::Pixels>, keyboard: KeyboardInteractivity) -> LayerSurfaceSpec {
    let mut spec = LayerSurfaceSpec::overlay(SurfaceRole::Osd, surface, None).with_keyboard(keyboard);
    spec.anchor = if bottom { Anchor::BOTTOM } else { Anchor::TOP };
    // Below the bar (its exclusive zone), not over it.
    spec.exclusive_zone = Some(px(0.0));
    let gap = px(super::dock::geo::EDGE_GAP);
    spec.margin = if bottom { (px(0.0), px(0.0), gap, px(0.0)) } else { (gap, px(0.0), px(0.0), px(0.0)) };
    spec
}

fn open_dock(cx: &mut App) {
    let shot = settings(cx);
    let bottom = shot.toolbar_position == ToolbarPosition::Bottom;
    let surface = DockView::first_size(&shot);
    // On demand: Esc and the shortcuts work once it is clicked, and
    // nothing is grabbed from the desktop.
    let spec = edge_spec(bottom, surface, KeyboardInteractivity::OnDemand);
    match cx.open_window(spec.window_options(), move |window, cx| cx.new(|cx| DockView::new(shot, surface, window, cx))) {
        Ok(h) => cx.default_global::<Session>().dock = Some(h),
        Err(e) => {
            tracing::error!("snip toolbar: {e:#}");
            apply(Input::Close, cx);
        }
    }
}

fn open_countdown(left: u32, cx: &mut App) {
    let shot = settings(cx);
    // In the corner away from the bar (reading end), clear of the panels
    // being opened and of the app dock.
    let top = crate::state::config(cx).read(cx).get().bar.position == mandara_config::BarPosition::Bottom;
    let mut spec = edge_spec(!top, CountdownView::surface_size(&shot), KeyboardInteractivity::Exclusive);
    let side = if crate::bidi::rtl() { Anchor::LEFT } else { Anchor::RIGHT };
    spec.anchor |= side;
    let gap = px(super::dock::geo::EDGE_GAP);
    spec.margin = if top { (gap, gap, px(0.0), gap) } else { (px(0.0), gap, gap, gap) };
    // Exclusive keyboard so Esc cancels; the pointer passes through to the
    // bar and its panels (empty input region).
    match cx.open_window(spec.window_options(), move |window, cx| cx.new(|cx| CountdownView::new(left, shot, window, cx))) {
        Ok(h) => cx.default_global::<Session>().countdown = Some(h),
        Err(e) => {
            // Count down without the pill rather than not at all.
            tracing::warn!("snip countdown: {e:#}");
        }
    }
}
