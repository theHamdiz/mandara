//! The snipping tool's compact surfaces, and the controls both capture
//! toolbars share:
//!
//! - [`DockView`], the idle toolbar: snipping is paused, the frozen frame
//!   is gone and the desktop is usable (open a panel, then capture). Its
//!   own layer surface, sized to the toolbar, centred on the top or bottom
//!   edge below the bar. It steps aside while a bar panel or the sidebar is
//!   open, so the pointer can travel into the panel.
//! - [`CountdownView`], the pill counting down before a delayed capture. It
//!   takes no pointer input and is gone before the frame is taken.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    AnyView, App, AppContext as _, Bounds, ClickEvent, Context, Div, FocusHandle, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Pixels, Render, SharedString,
    Size, Stateful, StatefulInteractiveElement, Styled, Window, canvas, div, point, prelude::FluentBuilder as _, px, size,
};
use mandara_config::workflow::{ScreenshotSettings, ToolbarPosition};
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui};
use mandara_i18n::{t, tf};
use mandara_theme::{Color, MandaraPalette};

use super::bar;
use super::flow::Input;
use super::session;
use super::SnipMode;
use crate::bidi::Bidi as _;
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::{alpha, text};

/// Compact-surface geometry at `toolbar_scale` 1.
pub(super) mod geo {
    use crate::kit::tokens;
    /// Gap between a compact surface and the screen edge (below the bar's
    /// exclusive zone when there is one).
    pub const EDGE_GAP: f32 = tokens::SM;
    /// Room around the toolbar for its drop shadow.
    pub const SHADOW_ROOM: f32 = super::bar::SHADOW_BLUR + super::bar::SHADOW_Y;
    /// Room on the screen side of the toolbar for its tooltips.
    pub const TIP_LANE: f32 = 40.0;
    /// Surface width before the toolbar has been measured.
    pub const FIRST_W: f32 = 900.0;
    /// Countdown pill diameter.
    pub const COUNT_D: f32 = 60.0;
    /// Tooltip padding and corner radius.
    pub const TIP_PAD_X: f32 = tokens::SM;
    pub const TIP_PAD_Y: f32 = tokens::XS;
    pub const TIP_RADIUS: f32 = tokens::SM;
}

/// Frames the idle toolbar waits for the compositor to apply its measured
/// size before it shows anyway.
const SIZE_FRAMES: u32 = 90;
/// Fade below which the idle toolbar no longer takes input.
const SHOWN: f32 = 0.05;
/// Countdown digit pulse: dimmest point, then the spring back to full.
const PULSE_FROM: f32 = 0.35;

fn fade_spring() -> SpringValue<f32> {
    SpringValue::new(0.0, SpringConfig::new(300.0, 30.0, 1.0))
}

/// A toolbar hover tooltip (the action and its shortcut).
struct Tip {
    text: SharedString,
    bg: Color,
    fg: Color,
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .font_family(fonts(cx).ui.clone())
            .px(px(geo::TIP_PAD_X))
            .py(px(geo::TIP_PAD_Y))
            .rounded(px(crate::style::radius(geo::TIP_RADIUS)))
            .bg(self.bg.hsla())
            .text_color(self.fg.hsla())
            .text_size(px(text::LABEL))
            .whitespace_nowrap()
            .child(self.text.clone())
    }
}

pub(super) fn tip(text: impl Into<SharedString>, p: &MandaraPalette) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let (text, bg, fg) = (text.into(), p.on_surface, p.surface_base);
    move |_, cx| cx.new(|_| Tip { text: text.clone(), bg, fg }).into()
}

/// The glass frame of a capture toolbar (children in reading order).
pub(super) fn frame(palette: &MandaraPalette, ts: f32, fade: f32) -> Div {
    let z = move |v: f32| px(v * ts);
    div()
        .h(z(bar::H))
        .px(z(bar::PAD))
        .flex_none()
        .rounded(px(crate::style::radius(bar::RADIUS) * ts))
        .bg(crate::shell::frame_color(palette).hsla())
        .opacity(fade)
        .shadow(vec![gpui::BoxShadow {
            color: gpui::black().opacity(alpha::KNOB_SHADOW),
            offset: point(px(0.0), z(bar::SHADOW_Y)),
            blur_radius: z(bar::SHADOW_BLUR),
            spread_radius: px(0.0),
            inset: false,
        }])
        .row()
        .items_center()
        .gap(z(bar::GAP))
        .whitespace_nowrap()
        .text_size(z(text::BODY))
}

/// A hairline between toolbar groups.
pub(super) fn separator(palette: &MandaraPalette, ts: f32) -> Div {
    div().w(px(1.0)).h(px(bar::SEPARATOR_H * ts)).mx(px(bar::SEPARATOR_MX * ts)).bg(palette.outline.with_alpha(0.4).hsla())
}

/// A region / window / screen chip; the tooltip names its key.
pub(super) fn mode_chip(id: &'static str, mode: SnipMode, active: bool, palette: &MandaraPalette, ts: f32) -> Stateful<Div> {
    let (glyph, label, key) = match mode {
        SnipMode::Region => ("crop", t("Region"), "R"),
        SnipMode::Window => ("window", t("Window"), "W"),
        SnipMode::Screen => ("monitor", t("Screen"), "S"),
    };
    let fg = if active { palette.on_primary } else { palette.on_surface };
    div()
        .id(id)
        .h(px(bar::BUTTON * ts))
        .px(px(bar::MODE_PAD * ts))
        .rounded(px(crate::style::radius(bar::BUTTON_RADIUS) * ts))
        .row()
        .items_center()
        .gap(px(bar::MODE_GAP * ts))
        .cursor_pointer()
        .when(active, |el| el.bg(palette.primary.hsla()).text_color(palette.on_primary.hsla()))
        .when(!active, |el| el.hover(|s| s.bg(palette.on_surface.with_alpha(alpha::HOVER_FILLED).hsla())))
        .child(icon(glyph, bar::ICON * ts, fg))
        .child(label)
        .tooltip(tip(format!("{label} · {key}"), palette))
}

/// A square icon button with a tooltip.
pub(super) fn icon_button(id: &'static str, glyph: &'static str, hint: impl Into<SharedString>, palette: &MandaraPalette, ts: f32) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(bar::BUTTON * ts))
        .flex_none()
        .rounded(px(crate::style::radius(bar::BUTTON_RADIUS) * ts))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(palette.on_surface.with_alpha(alpha::HOVER_FILLED).hsla()))
        .child(icon(glyph, bar::ICON * ts, palette.on_surface))
        .tooltip(tip(hint, palette))
}

/// An icon + label button; `primary` is filled with the accent.
fn text_button(id: &'static str, glyph: &'static str, label: impl Into<SharedString>, hint: impl Into<SharedString>, primary: bool, palette: &MandaraPalette, ts: f32) -> Stateful<Div> {
    let (bg, fg) = if primary { (palette.primary, palette.on_primary) } else { (palette.on_surface.with_alpha(alpha::HOVER), palette.on_surface) };
    div()
        .id(id)
        .h(px(bar::BUTTON * ts))
        .px(px(bar::ACTION_PAD * ts))
        .rounded(px(crate::style::radius(bar::BUTTON_RADIUS) * ts))
        .row()
        .items_center()
        .gap(px(bar::ACTION_GAP * ts))
        .font_weight(FontWeight::MEDIUM)
        .bg(bg.hsla())
        .text_color(fg.hsla())
        .cursor_pointer()
        .hover(|s| s.opacity(0.88))
        .active(|s| s.opacity(0.7))
        .child(icon(glyph, bar::ICON * ts, fg))
        .child(label.into())
        .tooltip(tip(hint, palette))
}

/// The tooltip of the move button for a toolbar at `position`.
pub(super) fn move_hint(position: ToolbarPosition) -> &'static str {
    match position {
        ToolbarPosition::Top => t("Move the toolbar to the bottom · M"),
        ToolbarPosition::Bottom => t("Move the toolbar to the top · M"),
    }
}

/// The move button's glyph: an arrow towards the other edge.
pub(super) fn move_glyph(position: ToolbarPosition) -> &'static str {
    match position {
        ToolbarPosition::Top => "arrow-down",
        ToolbarPosition::Bottom => "arrow-up",
    }
}

/// The tooltip of "Capture in N s".
pub(super) fn delay_hint(secs: u64) -> String {
    tf("Capture in {seconds} s, time to open a panel · T · Esc cancels the countdown", &[("seconds", &secs)])
}

/// The idle toolbar: capture now or after the countdown, the mode, the
/// toolbar's edge, close.
pub struct DockView {
    shot: ScreenshotSettings,
    focus: FocusHandle,
    fade: SpringValue<f32>,
    clock: FrameClock,
    /// The toolbar's bounds last frame, in window coordinates.
    measured: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The surface size last requested.
    size: Size<Pixels>,
    /// The input region last set (`None`: empty).
    region: Option<Option<Bounds<Pixels>>>,
    sizing_frames: u32,
}

impl DockView {
    /// The surface before the toolbar has been measured.
    pub(super) fn first_size(shot: &ScreenshotSettings) -> Size<Pixels> {
        let ts = shot.toolbar_scale();
        size(px(geo::FIRST_W * ts), px((bar::H + geo::SHADOW_ROOM * 2.0 + geo::TIP_LANE) * ts))
    }

    pub(super) fn new(shot: ScreenshotSettings, surface: Size<Pixels>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Step aside while a bar panel or the sidebar is open.
        cx.observe(&crate::state::shell(cx), |_, _, cx| cx.notify()).detach();
        cx.observe(&crate::state::config(cx), |this: &mut Self, config, cx| {
            this.shot = ScreenshotSettings::from_plugins(&config.read(cx).get().plugins);
            cx.notify();
        })
        .detach();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self { shot, focus, fade: fade_spring(), clock: FrameClock::new(), measured: Rc::new(Cell::new(None)), size: surface, region: None, sizing_frames: 0 }
    }

    fn key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        match e.keystroke.key.as_str() {
            "escape" => session::send(Input::Escape, cx),
            "enter" | "space" => session::send(Input::Capture, cx),
            "t" => session::capture_in(cx),
            "m" => {
                session::flip_position(cx);
            }
            "r" => self.set_mode(SnipMode::Region, cx),
            "w" => self.set_mode(SnipMode::Window, cx),
            "s" => self.set_mode(SnipMode::Screen, cx),
            _ => {}
        }
    }

    fn set_mode(&mut self, mode: SnipMode, cx: &mut Context<Self>) {
        session::set_mode(mode, cx);
        cx.notify();
    }
}

impl Render for DockView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = mandara_core::frame_palette(window, cx);
        let ts = self.shot.toolbar_scale();
        let z = move |v: f32| px(v * ts);
        let bottom = self.shot.toolbar_position == ToolbarPosition::Bottom;

        // Size the surface to the toolbar measured last frame.
        let measured = self.measured.get();
        if let Some(b) = measured {
            let want = size(b.size.width + z(geo::SHADOW_ROOM * 2.0), z(bar::H + geo::SHADOW_ROOM * 2.0 + geo::TIP_LANE));
            if (want.width - self.size.width).abs() > px(1.0) || (want.height - self.size.height).abs() > px(1.0) {
                window.resize(want);
                self.size = want;
            }
        }
        let viewport = window.viewport_size();
        let applied = (viewport.width - self.size.width).abs() <= px(1.0);
        let sized = measured.is_some() && (applied || self.sizing_frames >= SIZE_FRAMES);
        if !sized {
            self.sizing_frames += 1;
        }
        let shell = crate::state::shell(cx).read(cx);
        let tucked = shell.panel.is_some() || shell.sidebar_open;
        self.fade.set_target(if sized && !tucked { 1.0 } else { 0.0 });
        let dt = self.clock.tick(Instant::now());
        if self.fade.step(dt) || !sized {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        let fade = self.fade.value().clamp(0.0, 1.0);

        // Input only over the visible toolbar; none while it steps aside.
        let region = measured.filter(|_| fade > SHOWN && !tucked);
        if self.region != Some(region) {
            window.set_input_region(Some(region.as_slice()));
            self.region = Some(region);
        }

        let mode = session::mode(cx);
        let position = self.shot.toolbar_position;
        let secs = self.shot.countdown_secs();
        let chip = |id: &'static str, m: SnipMode, cx: &mut Context<Self>| {
            mode_chip(id, m, mode == m, &palette, ts).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_mode(m, cx)))
        };
        let probe = self.measured.clone();
        let toolbar = frame(&palette, ts, fade)
            .relative()
            .child(chip("dock-region", SnipMode::Region, cx))
            .child(chip("dock-window", SnipMode::Window, cx))
            .child(chip("dock-screen", SnipMode::Screen, cx))
            .child(separator(&palette, ts))
            .child(
                text_button("dock-now", "camera", t("Capture now"), t("Take a fresh frame and snip · Enter"), true, &palette, ts)
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| session::send(Input::Capture, cx))),
            )
            .child(
                text_button("dock-delay", "timer", tf("Capture in {seconds} s", &[("seconds", &secs)]), delay_hint(secs), false, &palette, ts)
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| session::capture_in(cx))),
            )
            .child(separator(&palette, ts))
            .child(icon_button("dock-move", move_glyph(position), move_hint(position), &palette, ts).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                session::flip_position(cx);
            })))
            .child(
                icon_button("dock-close", "close", t("Close the screenshot tool · Esc"), &palette, ts)
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| session::send(Input::Close, cx))),
            )
            // Measures the toolbar for the surface size and input region.
            .child(canvas(move |b, _, _| probe.set(Some(b)), |_, _, _, _| {}).absolute().inset_0());

        div()
            .id("snip-dock")
            .size_full()
            .track_focus(&self.focus)
            .font_family(fonts(cx).ui.clone())
            .text_color(palette.on_surface.hsla())
            .flex()
            .flex_col()
            .items_center()
            .when(bottom, |el| el.justify_end().pb(z(geo::SHADOW_ROOM)))
            .when(!bottom, |el| el.pt(z(geo::SHADOW_ROOM)))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.key(e, cx)))
            .child(toolbar)
    }
}

/// The pill counting down to a delayed capture. Esc cancels (its keyboard
/// is exclusive; the pointer passes through).
pub struct CountdownView {
    left: u32,
    ts: f32,
    focus: FocusHandle,
    pulse: SpringValue<f32>,
    clock: FrameClock,
    masked: bool,
}

impl CountdownView {
    pub(super) fn surface_size(shot: &ScreenshotSettings) -> Size<Pixels> {
        let side = px((geo::COUNT_D + geo::SHADOW_ROOM * 2.0) * shot.toolbar_scale());
        size(side, side)
    }

    pub(super) fn new(left: u32, shot: ScreenshotSettings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let mut pulse = fade_spring();
        pulse.set_target(1.0);
        Self { left, ts: shot.toolbar_scale(), focus, pulse, clock: FrameClock::new(), masked: false }
    }

    pub(super) fn set_left(&mut self, left: u32, cx: &mut Context<Self>) {
        self.left = left;
        self.pulse.snap_to(PULSE_FROM);
        self.pulse.set_target(1.0);
        cx.notify();
    }
}

impl Render for CountdownView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.masked {
            window.set_input_region(Some(&[]));
            self.masked = true;
        }
        let palette = mandara_core::frame_palette(window, cx);
        let dt = self.clock.tick(Instant::now());
        if self.pulse.step(dt) {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        let pulse = self.pulse.value().clamp(0.0, 1.0);
        let ts = self.ts;
        let z = move |v: f32| px(v * ts);
        div()
            .id("snip-countdown")
            .size_full()
            .track_focus(&self.focus)
            .flex()
            .items_center()
            .justify_center()
            .on_key_down(cx.listener(|_, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    session::send(Input::Escape, cx);
                }
            }))
            .child(
                div()
                    .size(z(geo::COUNT_D))
                    .rounded_full()
                    .bg(crate::shell::frame_color(&palette).hsla())
                    .shadow(vec![gpui::BoxShadow {
                        color: gpui::black().opacity(alpha::KNOB_SHADOW),
                        offset: point(px(0.0), z(bar::SHADOW_Y)),
                        blur_radius: z(bar::SHADOW_BLUR),
                        spread_radius: px(0.0),
                        inset: false,
                    }])
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_family(fonts(cx).ui.clone())
                    .font_weight(FontWeight::SEMIBOLD)
                    .font_features(tabular_nums())
                    .text_size(z(text::HEADLINE))
                    .text_color(palette.primary.hsla())
                    .opacity(pulse)
                    .child(self.left.to_string()),
            )
    }
}
