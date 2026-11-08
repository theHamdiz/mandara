//! The one slider every surface draws: Studio's setting rows and the
//! shell's thin sliders ([`crate::kit::slider`]), and the thick volume,
//! brightness and mixer sliders in panels and the sidebar
//! ([`crate::plugins::panel_fx::big_slider`]). Pointer capture, the dragged
//! value and the coalesced writes live in [`crate::slider_model`]; this
//! module only draws.
//!
//! The look follows Material 3's slider in the house style (rounded SDF
//! quads, palette tokens, light and dark, mirrored in right-to-left
//! languages):
//!
//! - an **active track** with rounded ends up to the value, a short gap, an
//!   upright **handle**, another gap, then the **inactive track** in a tonal
//!   surface colour ending in a small **stop dot**;
//! - while pressed the handle springs narrower (with a little overshoot)
//!   and, on sliders that show a value, a **value bubble** rises above it;
//! - the thick size carries an icon inside the start of the track and a
//!   gradient fill, and is **one continuous track**: the fill runs over it
//!   and the handle is a short grip inside the fill's end, so the part past
//!   the value never reads as a second, separate control (no gaps, no stop
//!   dot).
//!
//! Themes → Style → **Slider handle** (`style.slider_handle`) set to *None*
//! draws no handle, gaps or stop dot: the fill's rounded end alone marks
//! the value (issue #20).
//!
//! The knob and fill draw the pointer's value on the very next frame from
//! the drag state, whatever the owner does with the value.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gpui::{App, Div, ElementId, FontWeight, InteractiveElement as _, ParentElement, SharedString, Stateful, Styled, Window, canvas, div, prelude::FluentBuilder as _, px};
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui, oklab_gradient};
use mandara_theme::{Color, MandaraPalette};

use crate::bidi::Bidi as _;
use crate::slider_model::{IntoWrite, Track};

/// Formats a slider's value (`0..=1`) for its value bubble.
pub type Label = Rc<dyn Fn(f32) -> SharedString>;

/// Which slider to draw.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Size {
    /// The thin slider of settings rows.
    Standard,
    /// The thick pill with an icon inside its start (volume, brightness).
    Large,
}

/// A size's metrics, in logical px.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Dims {
    /// The whole slider's height (its hit area).
    pub h: f32,
    pub track_h: f32,
    pub handle_h: f32,
    pub handle_w: f32,
    /// Between the handle and each track.
    pub gap: f32,
    /// Where the handle's centre sits at values 0 and 1, from each end.
    pub start_inset: f32,
    pub end_inset: f32,
    pub stop_dot: f32,
}

impl Size {
    pub const fn dims(self) -> Dims {
        match self {
            Size::Standard => Dims { h: 24.0, track_h: 10.0, handle_h: 22.0, handle_w: 4.0, gap: 4.0, start_inset: 2.0, end_inset: 2.0, stop_dot: 4.0 },
            // The handle's centre never comes closer to the start than one
            // track height: the fill always holds the icon.
            Size::Large => Dims { h: 40.0, track_h: 40.0, handle_h: 40.0, handle_w: 4.0, gap: 4.0, start_inset: 42.0, end_inset: 4.0, stop_dot: 6.0 },
        }
    }

    /// Where values sit along the slider: the handle's centre runs from
    /// `start_inset` to `end_inset` from the ends, so a press puts it under
    /// the pointer.
    pub const fn track(self) -> Track {
        let d = self.dims();
        Track { start_inset: d.start_inset, end_inset: d.end_inset }
    }
}

/// The pressed handle is this much narrower (Material's pressed handle).
const PRESS_NARROW: f32 = 0.5;
/// Springs a little past the pressed width before it settles.
const PRESS_SPRING: SpringConfig = SpringConfig::new(700.0, 22.0, 1.0);
/// Below this the press has faded out.
const PRESS_VISIBLE: f32 = 0.01;
/// The stop dot hides once the handle comes this close to the end.
const STOP_DOT_HIDE: f32 = 0.97;
/// The thick slider's grip: its height as a share of the track, how far it
/// sits back from the fill's end (its centre, from the handle's centre),
/// and its ink's alpha over the fill.
const GRIP_H: f32 = 0.5;
const GRIP_BACK_PX: f32 = 8.0;
const GRIP_ALPHA: f32 = 0.85;
/// The value bubble: gap above the slider, how far it rises as it appears,
/// the slot it is centred in, its height, padding and text size.
const BUBBLE_GAP_PX: f32 = 6.0;
const BUBBLE_RISE_PX: f32 = 4.0;
const BUBBLE_SLOT_PX: f32 = 96.0;
const BUBBLE_H_PX: f32 = 22.0;
const BUBBLE_PAD_X_PX: f32 = 8.0;
const BUBBLE_TEXT_PX: f32 = 11.0;
/// The thick slider's icon: inset from the start, size, and how far the
/// fill must reach before the icon sits on it.
const ICON_INSET_PX: f32 = 12.0;
const ICON_PX: f32 = 17.0;
const ICON_COVERED_PX: f32 = 34.0;
/// Nominal width of a thick slider, to tell when the fill covers its icon.
const NOMINAL_W_PX: f32 = 260.0;
/// Muted fill alphas `(start, end)`.
const MUTED_FILL: (f32, f32) = (0.45, 0.3);
/// Gradient angles: along the reading direction.
const ANGLE_START_TO_END_DEG: f32 = 90.0;
const HALF_TURN_DEG: f32 = 180.0;

/// How a slider looks beyond its size.
#[derive(Clone, Default)]
pub struct Look {
    /// Inside the thick slider's start.
    pub icon: Option<&'static str>,
    /// Drawn muted (a muted output).
    pub muted: bool,
    /// The value bubble's text while held; no bubble without one.
    pub label: Option<Label>,
}

thread_local! {
    /// Press springs of sliders being pressed or springing back, by key.
    static PRESS: RefCell<HashMap<ElementId, (SpringValue<f32>, FrameClock)>> = RefCell::new(HashMap::new());
}

/// The press amount (`0` resting, `1` pressed, a little past it while the
/// spring overshoots) of slider `key`, stepped for this frame, and whether
/// it is still moving.
fn press_amount(key: &ElementId, held: bool, now: Instant) -> (f32, bool) {
    PRESS.with(|m| {
        let mut m = m.borrow_mut();
        if !held && !m.contains_key(key) {
            return (0.0, false);
        }
        let (spring, clock) = m.entry(key.clone()).or_insert_with(|| (SpringValue::new(0.0, PRESS_SPRING), FrameClock::new()));
        spring.set_target(if held { 1.0 } else { 0.0 });
        let moving = spring.step(clock.tick(now));
        let value = spring.value();
        if !held && !moving {
            m.remove(key);
            return (0.0, false);
        }
        (value, moving)
    })
}

/// The handle's width for a press amount.
pub fn handle_width(d: &Dims, press: f32) -> f32 {
    d.handle_w * (1.0 - PRESS_NARROW * press.max(0.0))
}

/// Draws a slider of `size` for `value` (`0..=1`) and binds its drag model
/// ([`crate::slider_model::bind`]). `on_change` writes a value and returns
/// the write's completion.
pub fn slider<R: IntoWrite>(id: ElementId, value: f32, size: Size, look: Look, palette: &MandaraPalette, on_change: impl Fn(f32, &mut Window, &mut App) -> R + 'static) -> Stateful<Div> {
    let d = size.dims();
    let v = crate::slider_model::shown(&id, value).clamp(0.0, 1.0);
    let rtl = crate::bidi::rtl();
    let handle = crate::style::slider_handle();
    let (press, moving) = press_amount(&id, crate::slider_model::dragging(&id), Instant::now());
    let large = size == Size::Large;

    let (from, to) = match (large, look.muted) {
        (_, true) => (palette.on_surface_variant.with_alpha(MUTED_FILL.0), palette.on_surface_variant.with_alpha(MUTED_FILL.1)),
        (true, false) => (palette.primary, crate::plugins::panel_fx::partner(palette.primary).with_lightness(palette.primary.lightness())),
        (false, false) => (palette.primary, palette.primary),
    };
    let inactive = if large { crate::plugins::panel_fx::wash(crate::plugins::panel_fx::WASH_TRACK, palette) } else { palette.surface_container_high };
    let angle = if rtl { ANGLE_START_TO_END_DEG + HALF_TURN_DEG } else { ANGLE_START_TO_END_DEG };
    let fill = |el: Div| if from == to { el.bg(from.hsla()) } else { el.bg(oklab_gradient(angle, from, to)) };
    // Margins on the reading-start / -end side.
    let margin_start = move |el: Div, m: f32| if rtl { el.mr(px(m)) } else { el.ml(px(m)) };
    let margin_end = move |el: Div, m: f32| if rtl { el.ml(px(m)) } else { el.mr(px(m)) };
    // The thick slider is one continuous track: no gaps around its handle
    // (a grip inside the fill instead) and no stop dot.
    let gapped = handle && !large;
    // Clearance from the handle's centre to each track: none without gaps.
    let side = if gapped { d.handle_w / 2.0 + d.gap } else { 0.0 };
    let track_y = (d.h - d.track_h) / 2.0;
    let track = || div().absolute().top(px(track_y)).h(px(d.track_h)).min_w(px(0.0)).rounded_full();

    // Positions are laid out in the handle's travel: value 0 at its start,
    // 1 at its end (`Size::track`).
    let travel = || div().absolute().top_0().bottom_0().inset_start(px(d.start_inset)).inset_end(px(d.end_inset));
    // The inactive track: from past the handle to the end (under the whole
    // slider without a handle, the fill drawn over it).
    let rest = if gapped {
        (v < 1.0).then(|| margin_start(track().inset_start(gpui::relative(v)).inset_end(px(-d.end_inset)), side).bg(inactive.hsla()))
    } else {
        Some(track().inset_start(px(-d.start_inset)).inset_end(px(-d.end_inset)).bg(inactive.hsla()))
    };
    // The active track: from the start to short of the handle, or without
    // gaps on to the slider's end at value 1.
    let active_end = if gapped { side } else { -d.end_inset * v };
    let active = margin_end(fill(track().inset_start(px(-d.start_inset)).inset_end(gpui::relative(1.0 - v))), active_end);
    let knob = handle.then(|| {
        let w = handle_width(&d, press);
        if large {
            // A grip inside the fill's end, in the fill's own ink.
            let grip_h = d.track_h * GRIP_H;
            let ink = if look.muted { palette.on_surface } else { palette.on_primary };
            margin_start(div().absolute().top(px((d.h - grip_h) / 2.0)).h(px(grip_h)).w(px(w)).inset_start(gpui::relative(v)), -w / 2.0 - GRIP_BACK_PX).rounded_full().bg(ink.with_alpha(GRIP_ALPHA).hsla())
        } else {
            margin_start(div().absolute().top(px((d.h - d.handle_h) / 2.0)).h(px(d.handle_h)).w(px(w)).inset_start(gpui::relative(v)), -w / 2.0).rounded_full().bg(to.hsla())
        }
    });
    // The stop dot at the inactive track's end.
    let stop = (gapped && v < STOP_DOT_HIDE).then(|| {
        div().absolute().top(px((d.h - d.stop_dot) / 2.0)).inset_end(px((d.track_h - d.stop_dot) / 2.0)).size(px(d.stop_dot)).rounded_full().bg(to.hsla())
    });
    let icon = look.icon.map(|name| {
        let on_fill = if look.muted { palette.on_surface } else { palette.on_primary };
        let color: Color = if v * NOMINAL_W_PX > ICON_COVERED_PX { on_fill } else { palette.on_surface_variant };
        div().absolute().top_0().inset_start(px(ICON_INSET_PX)).h_full().flex().items_center().child(crate::icons::icon(name, ICON_PX, color))
    });
    let bubble = look.label.filter(|_| press > PRESS_VISIBLE).map(|label| {
        let shown = press.clamp(0.0, 1.0);
        // A slot centred on the handle, standing on the slider's top edge.
        let slot = margin_start(div().absolute().bottom(px(d.h + BUBBLE_GAP_PX - BUBBLE_RISE_PX * (1.0 - shown))).w(px(BUBBLE_SLOT_PX)).inset_start(gpui::relative(v)), -BUBBLE_SLOT_PX / 2.0);
        travel().child(slot.flex().justify_center().opacity(shown).child(
            div()
                .h(px(BUBBLE_H_PX))
                .px(px(BUBBLE_PAD_X_PX))
                .rounded_full()
                .flex()
                .items_center()
                .bg(palette.on_surface.hsla())
                .text_color(palette.surface_base.hsla())
                .text_size(px(BUBBLE_TEXT_PX))
                .font_weight(FontWeight::MEDIUM)
                .font_features(crate::fonts::tabular_nums())
                .whitespace_nowrap()
                .child(label(v)),
        ))
    });

    let el = div()
        .id(id.clone())
        .relative()
        .h(px(d.h))
        .when(large, |el| el.w_full())
        .cursor_pointer()
        .child(travel().children(rest).child(active).children(knob))
        .children(stop)
        .children(icon)
        .children(bubble)
        // Keeps frames coming while the press springs.
        .child(canvas(|_, _, _| {}, move |_, _, window, _| {
            if moving {
                window.request_animation_frame();
            }
        }));
    crate::slider_model::bind(el, id, size.track(), on_change)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_handle_sits_inside_the_slider_with_room_for_its_gaps() {
        for size in [Size::Standard, Size::Large] {
            let d = size.dims();
            assert!(d.handle_h <= d.h && d.track_h <= d.h, "{size:?}: everything inside the hit area");
            assert!(d.start_inset >= d.handle_w / 2.0 && d.end_inset >= d.handle_w / 2.0, "{size:?}: the handle never leaves the slider");
            assert!(d.stop_dot < d.track_h, "{size:?}: the stop dot fits the track");
            // At value 0 the handle's centre is under the pointer at the start inset.
            let t = size.track();
            assert_eq!(t.position_of(0.0, 0.0, 300.0, false), d.start_inset);
            assert_eq!(t.position_of(1.0, 0.0, 300.0, false), 300.0 - d.end_inset);
        }
        // The thick slider's fill always holds its icon.
        let large = Size::Large.dims();
        assert!(large.start_inset - large.handle_w / 2.0 - large.gap >= ICON_INSET_PX + ICON_PX);
    }

    /// The thick slider is one track: its grip sits inside the fill's end,
    /// clear of the icon even at zero, and never past the slider's end.
    #[test]
    fn the_thick_grip_stays_inside_the_fill_and_clear_of_the_icon() {
        let d = Size::Large.dims();
        // At value 0 the fill ends at the handle's centre (`start_inset`).
        let (grip_start, grip_end) = (d.start_inset - GRIP_BACK_PX - d.handle_w / 2.0, d.start_inset - GRIP_BACK_PX + d.handle_w / 2.0);
        assert!(grip_start > ICON_INSET_PX + ICON_PX, "clear of the icon");
        assert!(grip_end < d.start_inset, "inside the fill");
        // At value 1 the handle's centre is `end_inset` from the end and the
        // fill reaches the end, past the grip.
        assert!(d.handle_w / 2.0 - GRIP_BACK_PX < d.end_inset, "inside the fill's end");
        const { assert!(GRIP_H > 0.0 && GRIP_H < 1.0, "shorter than the track") };
    }

    #[test]
    fn pressing_springs_the_handle_narrower_and_back() {
        let key: ElementId = "press-test".into();
        let t0 = Instant::now();
        let at = |ms: u64| t0 + std::time::Duration::from_millis(ms);
        assert_eq!(press_amount(&key, false, t0), (0.0, false), "untouched: nothing kept");
        let mut peak: f32 = 0.0;
        let mut ms = 0;
        loop {
            let (p, moving) = press_amount(&key, true, at(ms));
            peak = peak.max(p);
            if !moving {
                assert!((p - 1.0).abs() < 1e-3, "settles pressed");
                break;
            }
            ms += 7;
            assert!(ms < 2000, "settles");
        }
        assert!(peak > 1.0, "a springy press overshoots a little ({peak})");
        let d = Size::Standard.dims();
        assert_eq!(handle_width(&d, 1.0), d.handle_w * (1.0 - PRESS_NARROW));
        assert_eq!(handle_width(&d, 0.0), d.handle_w);
        loop {
            let (p, moving) = press_amount(&key, false, at(ms));
            if !moving {
                assert_eq!(p, 0.0);
                break;
            }
            ms += 7;
            assert!(ms < 4000, "springs back");
        }
        assert!(PRESS.with(|m| !m.borrow().contains_key(&key)), "a released slider's spring is dropped");
    }
}
