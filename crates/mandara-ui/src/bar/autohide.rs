//! Bar auto-hide (`[bar.auto_hide]`) and per-output bar visibility
//! (`[displays.surfaces.bar]`, with the legacy `bar.monitors` honoured).
//!
//! The bar slides off its edge on a spring (the shell surface itself never
//! moves, per the Caelestia input-masking law); while hidden only a thin
//! reveal strip along that edge stays in the input region. Touching it
//! slides the bar back; leaving the bar for `delay_ms` hides it again.
//! An open panel or sidebar always keeps the bar out.

use std::time::{Duration, Instant};

use gpui::{AnyElement, Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, Task, div, px};
use mandara_config::desktop::AutoHideMode;
use mandara_config::{BarPosition, ShellConfig};
use mandara_core::{InputMaskRegistry, SpringConfig, SpringValue};

use crate::hub::Hub;

/// Reveal / hide spring (a touch softer than chip hovers).
const REVEAL_SPRING: SpringConfig = SpringConfig::new(320.0, 30.0, 1.0);
/// How long the bar peeks out after a workspace / tag switch.
const WORKSPACE_PEEK: Duration = Duration::from_millis(1200);
/// Shown fraction below which the reveal strip is offered.
const STRIP_BELOW: f32 = 0.5;
/// Extra travel past the band so shadows / flares leave the screen too, px.
const HIDE_OVERSHOOT_PX: f32 = 2.0;
/// Element id of the reveal strip and of the hover band.
const STRIP_ID: &str = "bar-reveal-strip";
const BAND_ID: &str = "bar-hover-band";

/// Whether the bar is shown on output `monitor` at all.
pub fn bar_on_monitor(cfg: &ShellConfig, monitor: &str) -> bool {
    cfg.bar_on_output(monitor)
}

/// Whether the bar should reserve its exclusive zone on `monitor`: always
/// for "never hide" and "hide for fullscreen" (fullscreen windows ignore
/// zones anyway); never when it floats over windows.
pub fn reserves_space(cfg: &ShellConfig, monitor: &str) -> bool {
    bar_on_monitor(cfg, monitor) && matches!(mode(cfg), AutoHideMode::Never | AutoHideMode::Fullscreen)
}

/// The auto-hide mode in effect: a side bar always stays (its reveal strip
/// would sit on the edge windows scroll and drag against).
pub fn mode(cfg: &ShellConfig) -> AutoHideMode {
    if cfg.bar.position.is_vertical() { AutoHideMode::Never } else { cfg.bar.auto_hide.mode }
}

/// `(any window covers the workspace, a fullscreen window is shown)` on
/// `monitor`'s active workspace / tags.
pub fn coverage(hub: &Hub, monitor: &str) -> (bool, bool) {
    let mask = hub.navigation.get(monitor).map(|n| n.active_mask()).unwrap_or(0);
    let mut covered = false;
    let mut fullscreen = false;
    for c in hub.clients.iter().filter(|c| c.monitor == monitor) {
        let visible = c.tags.iter().any(|t| *t >= 1 && *t <= 32 && mask & (1u32 << (t - 1)) != 0);
        if !visible {
            continue;
        }
        covered |= c.fullscreen || !c.floating;
        fullscreen |= c.fullscreen;
    }
    (covered, fullscreen)
}

/// Whether the bar should currently be hidden (ignoring the pointer).
pub fn wants_hidden(mode: AutoHideMode, covered: bool, fullscreen: bool) -> bool {
    match mode {
        AutoHideMode::Never => false,
        AutoHideMode::Always => true,
        AutoHideMode::Overlap => covered,
        AutoHideMode::Fullscreen => fullscreen,
    }
}

pub struct AutoHide {
    /// 1 = fully shown, 0 = fully hidden.
    shown: SpringValue<f32>,
    /// The pointer is on the bar (or its reveal strip).
    hovering: bool,
    /// Pending hide after the pointer left.
    hide_task: Option<Task<()>>,
    /// Hold the bar out until then (workspace switch peek).
    peek_until: Option<Instant>,
    last_mask: Option<u32>,
    covered: bool,
    fullscreen: bool,
}

impl AutoHide {
    /// Starts tracking window coverage for `monitor`; `get` reaches this
    /// state inside the host view.
    pub fn new<V: 'static>(monitor: String, get: fn(&mut V) -> &mut AutoHide, cx: &mut Context<V>) -> Self {
        let hub = crate::hub::hub(cx);
        cx.observe(&hub, move |this: &mut V, hub, cx| {
            let (covered, fullscreen) = coverage(hub.read(cx), &monitor);
            let mask = hub.read(cx).navigation.get(&monitor).map(|n| n.active_mask());
            let state = get(this);
            let mut changed = (covered, fullscreen) != (state.covered, state.fullscreen);
            state.covered = covered;
            state.fullscreen = fullscreen;
            if mask != state.last_mask {
                if state.last_mask.is_some() && crate::state::config(cx).read(cx).get().bar.auto_hide.reveal_on_workspace_switch {
                    state.peek_until = Some(Instant::now() + WORKSPACE_PEEK);
                    // Re-render once the peek is over.
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(WORKSPACE_PEEK).await;
                        let _ = this.update(cx, |_, cx| cx.notify());
                    })
                    .detach();
                    changed = true;
                }
                state.last_mask = mask;
            }
            if changed {
                cx.notify();
            }
        })
        .detach();
        Self { shown: SpringValue::new(1.0, REVEAL_SPRING), hovering: false, hide_task: None, peek_until: None, last_mask: None, covered: false, fullscreen: false }
    }

    /// Sets the spring target for this frame.
    pub fn update(&mut self, cfg: &ShellConfig, monitor: &str, keep_out: bool) {
        let shown = if !bar_on_monitor(cfg, monitor) {
            false
        } else {
            let peeking = self.peek_until.is_some_and(|t| Instant::now() < t);
            keep_out || self.hovering || peeking || !wants_hidden(mode(cfg), self.covered, self.fullscreen)
        };
        self.shown.set_target(if shown { 1.0 } else { 0.0 });
        if cfg.motion.reduce_motion {
            self.shown.snap_to_target();
        }
    }

    pub fn step(&mut self, dt: f32) -> bool {
        self.shown.step(dt)
    }

    /// Vertical shift of every bar piece for a band `band_h` px tall.
    pub fn shift(&self, cfg: &ShellConfig, band_h: f32) -> f32 {
        let hidden = 1.0 - self.shown.value().clamp(0.0, 1.0);
        let travel = (band_h + HIDE_OVERSHOOT_PX) * hidden;
        if cfg.bar.position == BarPosition::Bottom { travel } else { -travel }
    }

    /// The bar is (mostly) off-screen.
    pub fn is_hidden(&self) -> bool {
        self.shown.value() < STRIP_BELOW
    }

    fn set_hover<V: 'static>(&mut self, hovered: bool, delay: Duration, get: fn(&mut V) -> &mut AutoHide, cx: &mut Context<V>) {
        if hovered {
            self.hovering = true;
            self.hide_task = None;
            cx.notify();
            return;
        }
        self.hide_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |this, cx| {
                let state = get(this);
                state.hovering = false;
                state.hide_task = None;
                cx.notify();
            });
        }));
    }

    /// The reveal strip while hidden, or a non-blocking hover band over the
    /// bar while shown (so leaving the bar starts the hide timer).
    pub fn elements<V: 'static>(
        &self,
        cfg: &ShellConfig,
        monitor: &str,
        surface: (f32, f32),
        band: (f32, f32),
        strip_px: f32,
        mask: &InputMaskRegistry,
        get: fn(&mut V) -> &mut AutoHide,
        cx: &mut Context<V>,
    ) -> Option<AnyElement> {
        if mode(cfg) == AutoHideMode::Never || !bar_on_monitor(cfg, monitor) {
            return None;
        }
        let delay = Duration::from_millis(cfg.bar.auto_hide.delay_ms);
        let bottom = cfg.bar.position == BarPosition::Bottom;
        // Entering counts only on real pointer motion: GPUI keeps the last
        // pointer position after it left the surface, so a hover computed
        // from it when the strip / band appears would be stale (the bar
        // would reveal itself or never hide).
        let left = move |this: &mut V, hovered: &bool, _: &mut gpui::Window, cx: &mut Context<V>| {
            if !*hovered {
                get(this).set_hover(false, delay, get, cx);
            }
        };
        let moved = move |this: &mut V, _: &gpui::MouseMoveEvent, _: &mut gpui::Window, cx: &mut Context<V>| {
            if !get(this).hovering || get(this).hide_task.is_some() {
                get(this).set_hover(true, delay, get, cx);
            }
        };
        if self.is_hidden() {
            Some(
                div()
                    .id(STRIP_ID)
                    .absolute()
                    .left_0()
                    .top(px(if bottom { surface.1 - strip_px } else { 0.0 }))
                    .w(px(surface.0))
                    .h(px(strip_px))
                    .on_mouse_move(cx.listener(moved))
                    .on_hover(cx.listener(left))
                    .child(mask.region())
                    .into_any_element(),
            )
        } else {
            // Tracks the pointer over the bar band without joining the
            // input region (the bar's own pieces already do).
            Some(
                div()
                    .id(BAND_ID)
                    .absolute()
                    .left_0()
                    .top(px(band.0))
                    .w(px(surface.0))
                    .h(px(band.1))
                    .on_mouse_move(cx.listener(moved))
                    .on_hover(cx.listener(left))
                    .into_any_element(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_decide_visibility() {
        assert!(!wants_hidden(AutoHideMode::Never, true, true));
        assert!(wants_hidden(AutoHideMode::Always, false, false));
        assert!(wants_hidden(AutoHideMode::Overlap, true, false) && !wants_hidden(AutoHideMode::Overlap, false, false));
        assert!(wants_hidden(AutoHideMode::Fullscreen, true, true) && !wants_hidden(AutoHideMode::Fullscreen, true, false));
    }

    #[test]
    fn space_is_reserved_only_when_the_bar_stays() {
        let mut cfg = ShellConfig::default();
        assert!(reserves_space(&cfg, "DP-1"));
        cfg.bar.auto_hide.mode = AutoHideMode::Fullscreen;
        assert!(reserves_space(&cfg, "DP-1"));
        cfg.bar.auto_hide.mode = AutoHideMode::Overlap;
        assert!(!reserves_space(&cfg, "DP-1"));
        cfg.bar.auto_hide.mode = AutoHideMode::Never;
        cfg.bar.monitors = " eDP-1, HDMI-A-1".into();
        assert!(!reserves_space(&cfg, "DP-1") && reserves_space(&cfg, "eDP-1"));
        // The Displays page's assignment replaces the old list.
        cfg.bar.monitors.clear();
        cfg.displays.surfaces.bar = mandara_config::outputs::ScreenSet { show_on: mandara_config::outputs::ScreenChoice::Selected, outputs: vec!["DP-1".into()] };
        assert!(reserves_space(&cfg, "DP-1") && !reserves_space(&cfg, "eDP-1"));
    }
}
