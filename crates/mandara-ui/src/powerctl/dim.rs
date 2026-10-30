//! The fade before an idle stage: every output slowly sinks into the
//! palette's darkest surface tone (an OKLCH-tinted near-black rather than
//! flat grey), with a small glass pill saying what is about to happen.
//!
//! The overlay never takes input (its input region is empty), so the first
//! mouse move or key press reaches the compositor, which reports the user
//! active again; the idle manager then fades the overlay back out. Windows
//! exist only while fading (a full-output surface costs real VRAM).

use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, FontWeight, Global, IntoElement, ParentElement, Render, Styled, Window, WindowHandle, div, px, size};
use mandara_core::{LayerSurfaceSpec, SurfaceRole, ToGpui};
use mandara_i18n::t;

use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::text;

/// Darkest the overlay gets (the stage itself takes over from there).
const MAX_ALPHA: f32 = 0.92;
/// Fade-out after input, seconds.
const FADE_OUT: Duration = Duration::from_millis(260);
/// The hint pill appears once this much of the fade has passed.
const HINT_FROM: f32 = 0.25;
/// Hint pill geometry, logical px.
const HINT_PAD_X_PX: f32 = 18.0;
const HINT_PAD_Y_PX: f32 = 10.0;
const HINT_GAP_PX: f32 = 10.0;
const HINT_ICON_PX: f32 = 16.0;
/// The hint's glass tint over the dimmed screen.
const HINT_FILL_ALPHA: f32 = 0.72;
const HINT_RIM_ALPHA: f32 = 0.16;
/// The veil: the palette's dimmest surface pulled most of the way to black
/// (keeps a hint of the theme's hue instead of flat grey).
const VEIL_BASE_HEX: &str = "#000000";
const VEIL_DEPTH: f32 = 0.7;
/// Output size assumed when nothing reports one.
const FALLBACK_OUTPUT: (f32, f32) = (1920.0, 1080.0);

/// What the fade leads to (only changes the hint).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DimStage {
    Lock,
    ScreensOff,
    /// `idle dim-preview`: nothing follows.
    Preview,
}

#[derive(Default)]
struct Dim {
    windows: Vec<WindowHandle<DimView>>,
}
impl Global for Dim {}

pub struct DimView {
    started: Instant,
    duration: Duration,
    stage: DimStage,
    /// Set when the fade is cancelled: fading out since then.
    leaving: Option<Instant>,
    /// The alpha when leaving started (fade out from there).
    left_at_alpha: f32,
    input_cleared: bool,
}

/// Smoothstep: slow start (barely noticeable at first), firm end.
fn ease(p: f32) -> f32 {
    let p = p.clamp(0.0, 1.0);
    p * p * (3.0 - 2.0 * p)
}

impl DimView {
    fn alpha(&self, now: Instant) -> f32 {
        match self.leaving {
            Some(since) => self.left_at_alpha * (1.0 - (now.duration_since(since).as_secs_f32() / FADE_OUT.as_secs_f32()).clamp(0.0, 1.0)),
            None => MAX_ALPHA * ease(now.duration_since(self.started).as_secs_f32() / self.duration.as_secs_f32().max(f32::EPSILON)),
        }
    }

    fn progress(&self, now: Instant) -> f32 {
        (now.duration_since(self.started).as_secs_f32() / self.duration.as_secs_f32().max(f32::EPSILON)).clamp(0.0, 1.0)
    }
}

impl Render for DimView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.input_cleared {
            window.set_input_region(Some(&[]));
            self.input_cleared = true;
        }
        let now = Instant::now();
        let alpha = self.alpha(now);
        let finished_leaving = self.leaving.is_some_and(|since| now.duration_since(since) >= FADE_OUT);
        if finished_leaving {
            window.remove_window();
        } else if self.leaving.is_some() || self.progress(now) < 1.0 {
            window.request_animation_frame();
        }
        let palette = mandara_core::frame_palette(window, cx);
        let f = fonts(cx);
        let p = self.progress(now);
        let hint_alpha = if self.leaving.is_some() { alpha / MAX_ALPHA } else { ((p - HINT_FROM) / (1.0 - HINT_FROM)).clamp(0.0, 1.0) };
        let remaining = self.duration.saturating_sub(now.duration_since(self.started)).as_secs() + 1;
        let (glyph, text_line) = match self.stage {
            DimStage::Lock => ("lock", mandara_i18n::tf("Locking in {seconds} s — move the mouse to stay", &[("seconds", &remaining)])),
            DimStage::ScreensOff => ("moon", mandara_i18n::tf("Screens off in {seconds} s — move the mouse to stay", &[("seconds", &remaining)])),
            DimStage::Preview => ("sun", t("This is how the screen fades before locking").to_string()),
        };
        if self.stage != DimStage::Preview && self.leaving.is_none() {
            // The countdown changes every second.
            window.request_animation_frame();
        }
        let veil = palette.surface_dim.mix(mandara_theme::Color::hex(VEIL_BASE_HEX), VEIL_DEPTH);
        div()
            .size_full()
            .bg(veil.with_alpha(alpha).hsla())
            .flex()
            .items_center()
            .justify_center()
            .font_family(f.ui.clone())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(HINT_GAP_PX))
                    .px(px(HINT_PAD_X_PX))
                    .py(px(HINT_PAD_Y_PX))
                    .rounded_full()
                    .bg(palette.surface_container_high.with_alpha(HINT_FILL_ALPHA * hint_alpha).hsla())
                    .border_1()
                    .border_color(palette.specular_rim_light.with_alpha(HINT_RIM_ALPHA * hint_alpha).hsla())
                    .opacity(hint_alpha)
                    .child(icon(glyph, HINT_ICON_PX, palette.primary))
                    .child(div().text_size(px(text::BODY)).font_features(tabular_nums()).font_weight(FontWeight::MEDIUM).text_color(palette.on_surface.hsla()).child(text_line)),
            )
    }
}

/// Starts fading every output over `duration` toward `stage`.
pub fn start(duration: Duration, stage: DimStage, cx: &mut App) {
    stop_now(cx);
    let displays = cx.displays();
    // Each display with its own output's size (paired by name).
    let mut paired: Vec<(Option<gpui::DisplayId>, Option<mandara_core::OutputGeometry>)> = crate::outputs::paired(cx).into_iter().map(|(id, g)| (Some(id), g)).collect();
    if paired.is_empty() {
        paired.push((None, None));
    }
    let mut windows = Vec::with_capacity(paired.len());
    for (display, geo) in paired {
        let (w, h) = geo
            .map(|g| (g.width as f32, g.height as f32))
            .or_else(|| displays.iter().find(|d| Some(d.id()) == display).map(|d| (f32::from(d.bounds().size.width), f32::from(d.bounds().size.height))))
            .unwrap_or(FALLBACK_OUTPUT);
        let spec = LayerSurfaceSpec::overlay(SurfaceRole::Osd, size(px(w), px(h)), display);
        let started = Instant::now();
        match cx.open_window(spec.window_options(), move |_, cx| cx.new(|_| DimView { started, duration, stage, leaving: None, left_at_alpha: 0.0, input_cleared: false })) {
            Ok(handle) => windows.push(handle),
            Err(e) => tracing::warn!("idle fade: {e:#}"),
        }
    }
    cx.set_global(Dim { windows });
}

/// Fades the overlay out (input arrived, or the stage took over).
pub fn stop(cx: &mut App) {
    let Some(dim) = cx.try_global::<Dim>() else { return };
    let windows = dim.windows.clone();
    let now = Instant::now();
    for h in windows {
        let _ = h.update(cx, |view, window, cx| {
            if view.leaving.is_none() {
                view.left_at_alpha = view.alpha(now);
                view.leaving = Some(now);
                window.request_animation_frame();
                cx.notify();
            }
        });
    }
    cx.set_global(Dim::default());
}

/// Removes the overlay at once (a new fade replaces it).
fn stop_now(cx: &mut App) {
    let Some(dim) = cx.try_global::<Dim>() else { return };
    for h in dim.windows.clone() {
        let _ = h.update(cx, |_, window, _| window.remove_window());
    }
    cx.set_global(Dim::default());
}

/// Whether a fade is showing.
pub fn active(cx: &App) -> bool {
    cx.try_global::<Dim>().is_some_and(|d| !d.windows.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_eases_in_and_out() {
        let started = Instant::now();
        let mut v = DimView { started, duration: Duration::from_secs(8), stage: DimStage::Lock, leaving: None, left_at_alpha: 0.0, input_cleared: true };
        assert!(v.alpha(started) < 1e-4);
        let mid = v.alpha(started + Duration::from_secs(4));
        assert!((mid - MAX_ALPHA / 2.0).abs() < 1e-3, "{mid}");
        assert!((v.alpha(started + Duration::from_secs(9)) - MAX_ALPHA).abs() < 1e-4);
        let leave = started + Duration::from_secs(4);
        v.left_at_alpha = v.alpha(leave);
        v.leaving = Some(leave);
        assert!(v.alpha(leave + FADE_OUT) < 1e-4);
    }
}
