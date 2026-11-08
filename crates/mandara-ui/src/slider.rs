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
//!   gradient fill.
//!
//! Themes → Components → **Slider design** (`style.slider_design`) picks how
//! the thick sliders draw; thin sliders always keep the classic look:
//!
//! - **Classic** (the default): the layout above;
//! - **Continuous**: one continuous track, the fill running over it, with a
//!   short grip inside the fill's end instead of the gaps, handle and stop
//!   dot;
//! - **Pill**: one continuous track with a round knob on the fill's end that
//!   stretches into a pill while pressed;
//! - **Wavy**: the classic layout with a gentle wave along the fill, which
//!   flows while the slider is held (never on idle frames, and not at all
//!   with reduced motion);
//! - **Segmented**: ten rounded steps that fill one after another.
//!
//! Themes → Components → **Slider handle** (`style.slider_handle`) set to
//! *None* draws no handle, gaps or stop dot: the fill's rounded end alone
//! marks the value (issue #20). The Pill and Segmented designs mark the
//! value their own way and ignore it.
//!
//! The canvas-drawn parts (the pill, the steps, the wave) come from the pure
//! geometry below ([`pill_pieces`], [`segment_pieces`], [`wave_dots`]):
//! fixed arrays and iterators, nothing allocated per frame, and the wave is
//! a run of SDF dots rather than a tessellated path.
//!
//! The knob and fill draw the pointer's value on the very next frame from
//! the drag state, whatever the owner does with the value.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gpui::{App, Bounds, Corners, Div, ElementId, FontWeight, InteractiveElement as _, ParentElement, Pixels, SharedString, Stateful, Styled, Window, canvas, div, point, prelude::FluentBuilder as _, px};
pub use mandara_config::desktop::SliderDesign;
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
/// The Continuous design's grip: its height as a share of the track, how
/// far it sits back from the fill's end (its centre, from the handle's
/// centre), and its ink's alpha over the fill.
const GRIP_H: f32 = 0.5;
const GRIP_BACK_PX: f32 = 8.0;
const GRIP_ALPHA: f32 = 0.85;
/// The Pill design's knob: its resting size (a circle), how much it grows
/// while pressed (into an upright pill), its ring, the room it keeps from
/// the slider's end and from the icon, and its glow `(alpha, blur)`.
const KNOB_PX: f32 = 24.0;
const KNOB_GROW_PX: (f32, f32) = (2.0, 10.0);
const KNOB_RING_PX: f32 = 2.0;
const KNOB_END_PAD_PX: f32 = 4.0;
const KNOB_ICON_GAP_PX: f32 = 2.0;
const KNOB_GLOW: (f32, f32) = (0.35, 6.0);
/// The Segmented design: how many steps share the travel, the gap between
/// steps and the radius of their inner corners.
pub const SEGMENTS: usize = 10;
const SEGMENT_GAP_PX: f32 = 3.0;
const SEGMENT_INNER_RADIUS_PX: f32 = 6.0;
/// The Wavy design's wave: amplitude, wavelength, line width, dot spacing,
/// its room from the icon and from the fill's end, the run it fades in
/// and out over, how much it swells while held, and how fast it flows
/// then (radians per second at the normal animation speed).
const WAVE_AMPLITUDE_PX: f32 = 3.5;
const WAVE_LENGTH_PX: f32 = 26.0;
const WAVE_STROKE_PX: f32 = 2.5;
const WAVE_STEP_PX: f32 = 1.0;
const WAVE_ICON_GAP_PX: f32 = 6.0;
const WAVE_END_GAP_PX: f32 = 8.0;
const WAVE_TAPER_PX: f32 = WAVE_LENGTH_PX / 2.0;
const WAVE_SWELL: f32 = 0.25;
const WAVE_FLOW_RAD_S: f32 = std::f32::consts::TAU * 1.5;
/// How far the wave's ink leans toward the fill (a softer line than the
/// icon's).
const WAVE_SOFTEN: f32 = 0.3;
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
    /// The thick slider's design; `None` follows `style.slider_design`
    /// (Studio's previews draw each design whatever is set).
    pub design: Option<SliderDesign>,
}

thread_local! {
    /// Press springs (and the Wavy design's wave phase) of sliders being
    /// pressed or settling back, by key.
    static PRESS: RefCell<HashMap<ElementId, Pressing>> = RefCell::new(HashMap::new());
}

/// A slider being pressed or settling back.
struct Pressing {
    spring: SpringValue<f32>,
    clock: FrameClock,
    /// The wave's phase, radians in `0..TAU`.
    phase: f32,
}

/// A slider's press for this frame.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Press {
    /// `0` resting, `1` pressed, a little past it while the spring
    /// overshoots.
    pub amount: f32,
    /// The Wavy design's wave phase, radians (`0` at rest).
    pub phase: f32,
    /// Still changing: the caller asks for another frame.
    pub moving: bool,
}

impl Press {
    const REST: Press = Press { amount: 0.0, phase: 0.0, moving: false };
}

/// The press of slider `key`, stepped for this frame. With `flow` (the
/// Wavy design) its wave runs while `held` and, once let go, on to the end
/// of its wavelength, where it looks exactly like the resting wave, so
/// nothing jumps when the entry is dropped. Reduced motion keeps the wave
/// still. Nothing is kept for a slider at rest, and a held slider's entry
/// is reused frame to frame (no allocation).
pub fn press_state(key: &ElementId, held: bool, flow: bool, now: Instant) -> Press {
    press_state_in(key, held, flow, now, &mandara_core::spring::motion_profile())
}

/// [`press_state`] under an explicit motion profile.
fn press_state_in(key: &ElementId, held: bool, flow: bool, now: Instant, profile: &mandara_core::spring::MotionProfile) -> Press {
    use std::f32::consts::TAU;
    PRESS.with(|m| {
        let mut m = m.borrow_mut();
        if !held && !m.contains_key(key) {
            return Press::REST;
        }
        let entry = m.entry(key.clone()).or_insert_with(|| Pressing { spring: SpringValue::new(0.0, PRESS_SPRING), clock: FrameClock::new(), phase: 0.0 });
        entry.spring.set_target(if held { 1.0 } else { 0.0 });
        let dt = entry.clock.tick(now);
        let springing = entry.spring.step_in(dt, profile);
        let still = !flow || profile.reduce;
        let flowing = if still || (!held && entry.phase == 0.0) {
            entry.phase = 0.0;
            false
        } else {
            let next = entry.phase + dt * WAVE_FLOW_RAD_S * mandara_core::spring::time_scale();
            // Let go: the wave finishes its wavelength, the resting wave again.
            entry.phase = match (next >= TAU, held) {
                (true, true) => next - TAU,
                (true, false) => 0.0,
                (false, _) => next,
            };
            held || entry.phase != 0.0
        };
        let (amount, phase) = (entry.spring.value(), entry.phase);
        if !held && !springing && !flowing {
            m.remove(key);
            return Press::REST;
        }
        Press { amount, phase, moving: springing || flowing }
    })
}

/// The handle's width for a press amount.
pub fn handle_width(d: &Dims, press: f32) -> f32 {
    d.handle_w * (1.0 - PRESS_NARROW * press.max(0.0))
}

// ── Geometry of the canvas-drawn designs ─────────────────────────────────

/// One rounded rectangle of a thick slider's design, in logical px from the
/// slider's reading start (`x`) and its top (`y`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Piece {
    pub kind: PieceKind,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Corner radii on the reading-start and reading-end side.
    pub start_r: f32,
    pub end_r: f32,
}

/// What a [`Piece`] is.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum PieceKind {
    /// Track past the value.
    Rest,
    /// Fill, its gradient running from `t0` to `t1` of the whole fill's
    /// colours (`0` at the slider's start, `1` at the fill's end).
    Fill { t0: f32, t1: f32 },
    /// The Pill design's knob.
    Knob,
}

impl Piece {
    /// Where the piece ends along the slider.
    pub fn end(&self) -> f32 {
        self.x + self.w
    }

    /// The piece's bounds in a slider at `origin`, `width` wide, mirrored in
    /// right-to-left languages.
    pub fn bounds(&self, origin: gpui::Point<Pixels>, width: f32, rtl: bool) -> Bounds<Pixels> {
        let x = if rtl { width - self.end() } else { self.x };
        Bounds::new(point(origin.x + px(x), origin.y + px(self.y)), gpui::size(px(self.w), px(self.h)))
    }

    /// The piece's corner radii, mirrored in right-to-left languages.
    pub fn corners(&self, rtl: bool) -> Corners<Pixels> {
        let (left, right) = if rtl { (self.end_r, self.start_r) } else { (self.start_r, self.end_r) };
        Corners { top_left: px(left), bottom_left: px(left), top_right: px(right), bottom_right: px(right) }
    }
}

/// Where value `v` sits along a thick slider `width` wide: the handle's
/// centre, as the drag model maps the pointer (`Size::track`).
fn value_x(v: f32, width: f32) -> f32 {
    let d = Size::Large.dims();
    d.start_inset + v * (width - d.start_inset - d.end_inset).max(0.0)
}

/// Where a gap-free fill ends: at the value, reaching the slider's end at
/// value 1 (the Continuous design's fill).
fn open_fill_end(v: f32, width: f32) -> f32 {
    (value_x(v, width) + Size::Large.dims().end_inset * v).min(width)
}

/// The thick slider's icon ends here, from the reading start.
const ICON_END_PX: f32 = ICON_INSET_PX + ICON_PX;

/// The Pill design at value `v` and press `press` on a slider `width`
/// wide: the track, the fill over it, and the knob on the fill's end
/// (kept clear of the icon and inside the slider's end, so it stops short
/// of the pointer only at the very ends).
pub fn pill_pieces(width: f32, v: f32, press: f32) -> [Piece; 3] {
    let d = Size::Large.dims();
    let (y, h, r) = ((d.h - d.track_h) / 2.0, d.track_h, d.track_h / 2.0);
    let rest = Piece { kind: PieceKind::Rest, x: 0.0, y, w: width, h, start_r: r, end_r: r };
    let fill = Piece { kind: PieceKind::Fill { t0: 0.0, t1: 1.0 }, x: 0.0, y, w: open_fill_end(v, width), h, start_r: r, end_r: r };
    let p = press.max(0.0);
    let kw = KNOB_PX + KNOB_GROW_PX.0 * p;
    let kh = (KNOB_PX + KNOB_GROW_PX.1 * p).min(d.track_h - 2.0 * KNOB_END_PAD_PX);
    let lo = ICON_END_PX + KNOB_ICON_GAP_PX + kw / 2.0;
    let hi = (width - KNOB_END_PAD_PX - kw / 2.0).max(lo);
    let centre = value_x(v, width).clamp(lo, hi);
    let kr = kw.min(kh) / 2.0;
    let knob = Piece { kind: PieceKind::Knob, x: centre - kw / 2.0, y: (d.h - kh) / 2.0, w: kw, h: kh, start_r: kr, end_r: kr };
    [rest, fill, knob]
}

/// The Segmented design at value `v` on a slider `width` wide: [`SEGMENTS`]
/// steps share the handle's travel (the first also holds the icon, the last
/// runs to the end), each a track piece unless it is full, then the fill
/// over it up to the value. Outer ends are fully rounded, inner corners
/// softly.
pub fn segment_pieces(width: f32, v: f32) -> impl Iterator<Item = Piece> {
    let d = Size::Large.dims();
    let (y, h, outer) = ((d.h - d.track_h) / 2.0, d.track_h, d.track_h / 2.0);
    let travel = (width - d.start_inset - d.end_inset).max(0.0);
    let at = open_fill_end(v, width);
    let boundary = move |k: usize| d.start_inset + travel * k as f32 / SEGMENTS as f32;
    (0..SEGMENTS).flat_map(move |i| {
        let last = i + 1 == SEGMENTS;
        let start = if i == 0 { 0.0 } else { boundary(i) + SEGMENT_GAP_PX / 2.0 };
        let end = if last { width } else { boundary(i + 1) - SEGMENT_GAP_PX / 2.0 };
        let w = (end - start).max(0.0);
        let start_r = (if i == 0 { outer } else { SEGMENT_INNER_RADIUS_PX }).min(w / 2.0);
        let end_r = (if last { outer } else { SEGMENT_INNER_RADIUS_PX }).min(w / 2.0);
        let filled = at.min(end);
        let rest = (filled < end).then_some(Piece { kind: PieceKind::Rest, x: start, y, w, h, start_r, end_r });
        let fill = (filled > start && at > 0.0).then(|| {
            let fw = filled - start;
            let fill_end_r = if filled < end { SEGMENT_INNER_RADIUS_PX } else { end_r };
            Piece { kind: PieceKind::Fill { t0: start / at, t1: filled / at }, x: start, y, w: fw, h, start_r: start_r.min(fw / 2.0), end_r: fill_end_r.min(fw / 2.0) }
        });
        rest.into_iter().chain(fill)
    })
}

/// The Wavy design's wave on a slider `width` wide: the centres of the SDF
/// dots that draw it (from the reading start and the top), along the fill
/// from past the icon to short of the fill's end (which sits before the
/// handle's gap with a `handle`), fading in and out at its ends. `press`
/// swells it a little; `phase` moves it along. Nothing is drawn until the
/// fill holds a whole wavelength.
pub fn wave_dots(width: f32, v: f32, handle: bool, press: f32, phase: f32) -> impl Iterator<Item = (f32, f32)> {
    let d = Size::Large.dims();
    let fill_end = if handle { value_x(v, width) - (d.handle_w / 2.0 + d.gap) } else { open_fill_end(v, width) };
    let (x0, x1) = (ICON_END_PX + WAVE_ICON_GAP_PX, fill_end - WAVE_END_GAP_PX);
    let n = if x1 - x0 >= WAVE_LENGTH_PX { ((x1 - x0) / WAVE_STEP_PX) as usize + 1 } else { 0 };
    let amp = WAVE_AMPLITUDE_PX * (1.0 + WAVE_SWELL * press.clamp(0.0, 1.0));
    let (cy, k) = (d.h / 2.0, std::f32::consts::TAU / WAVE_LENGTH_PX);
    (0..n).map(move |i| {
        let x = (x0 + i as f32 * WAVE_STEP_PX).min(x1);
        let t = ((x - x0).min(x1 - x) / WAVE_TAPER_PX).clamp(0.0, 1.0);
        let taper = t * t * (3.0 - 2.0 * t);
        // It flows toward the slider's end.
        (x, cy + amp * taper * (k * (x - x0) - phase).sin())
    })
}

/// The colours a canvas-drawn design paints with.
#[derive(Copy, Clone)]
struct Inks {
    from: Color,
    to: Color,
    rest: Color,
    knob: Color,
    ring: Color,
    angle: f32,
}

/// Paints one piece of a thick slider (the rest of the track, a stretch of
/// fill, or the knob) inside `bounds`, mirrored in right-to-left languages.
fn paint_piece(window: &mut Window, piece: &Piece, bounds: Bounds<Pixels>, rtl: bool, inks: Inks) {
    let width = f32::from(bounds.size.width);
    let (b, corners) = (piece.bounds(bounds.origin, width, rtl), piece.corners(rtl));
    match piece.kind {
        PieceKind::Rest => window.paint_quad(gpui::fill(b, inks.rest.hsla()).corner_radii(corners)),
        PieceKind::Fill { t0, t1 } => {
            let background = if inks.from == inks.to {
                inks.from.hsla().into()
            } else if t0 == 0.0 && t1 == 1.0 {
                oklab_gradient(inks.angle, inks.from, inks.to)
            } else {
                oklab_gradient(inks.angle, inks.from.mix(inks.to, t0), inks.from.mix(inks.to, t1))
            };
            window.paint_quad(gpui::fill(b, background).corner_radii(corners));
        }
        PieceKind::Knob => {
            let glow = crate::plugins::panel_fx::glow(inks.ring.with_alpha(KNOB_GLOW.0), KNOB_GLOW.1, 0.0, 1.0);
            window.paint_drop_shadows(b, corners, &[glow]);
            window.paint_quad(gpui::quad(b, corners, inks.knob.hsla(), px(KNOB_RING_PX), inks.ring.hsla(), gpui::BorderStyle::Solid));
        }
    }
}

/// Draws a slider of `size` for `value` (`0..=1`) and binds its drag model
/// ([`crate::slider_model::bind`]). `on_change` writes a value and returns
/// the write's completion.
pub fn slider<R: IntoWrite>(id: ElementId, value: f32, size: Size, look: Look, palette: &MandaraPalette, on_change: impl Fn(f32, &mut Window, &mut App) -> R + 'static) -> Stateful<Div> {
    let d = size.dims();
    let v = crate::slider_model::shown(&id, value).clamp(0.0, 1.0);
    let rtl = crate::bidi::rtl();
    let large = size == Size::Large;
    // Thin sliders always keep the classic look.
    let design = if large { look.design.unwrap_or_else(crate::style::slider_design) } else { SliderDesign::Classic };
    let handle = crate::style::slider_handle() && design.uses_handle();
    let Press { amount: press, phase, moving } = press_state(&id, crate::slider_model::dragging(&id), design == SliderDesign::Wavy, Instant::now());

    let (from, to) = match (large, look.muted) {
        (_, true) => (palette.on_surface_variant.with_alpha(MUTED_FILL.0), palette.on_surface_variant.with_alpha(MUTED_FILL.1)),
        (true, false) => (palette.primary, crate::plugins::panel_fx::partner(palette.primary).with_lightness(palette.primary.lightness())),
        (false, false) => (palette.primary, palette.primary),
    };
    let inactive = if large { crate::plugins::panel_fx::wash(crate::plugins::panel_fx::WASH_TRACK, palette) } else { palette.surface_container_high };
    let on_fill = if look.muted { palette.on_surface } else { palette.on_primary };
    let angle = if rtl { ANGLE_START_TO_END_DEG + HALF_TURN_DEG } else { ANGLE_START_TO_END_DEG };
    let fill = |el: Div| if from == to { el.bg(from.hsla()) } else { el.bg(oklab_gradient(angle, from, to)) };
    // Margins on the reading-start / -end side.
    let margin_start = move |el: Div, m: f32| if rtl { el.mr(px(m)) } else { el.ml(px(m)) };
    let margin_end = move |el: Div, m: f32| if rtl { el.ml(px(m)) } else { el.mr(px(m)) };
    // Classic and Wavy set the handle between two gaps and end the track in
    // a stop dot; Continuous runs one track with a grip inside the fill.
    let gapped = handle && matches!(design, SliderDesign::Classic | SliderDesign::Wavy);
    // Pill and Segmented paint their whole track on a canvas.
    let painted = matches!(design, SliderDesign::Pill | SliderDesign::Segmented);
    // Clearance from the handle's centre to each track: none without gaps.
    let side = if gapped { d.handle_w / 2.0 + d.gap } else { 0.0 };
    let track_y = (d.h - d.track_h) / 2.0;
    let track = || div().absolute().top(px(track_y)).h(px(d.track_h)).min_w(px(0.0)).rounded_full();

    // Positions are laid out in the handle's travel: value 0 at its start,
    // 1 at its end (`Size::track`).
    let travel = || div().absolute().top_0().bottom_0().inset_start(px(d.start_inset)).inset_end(px(d.end_inset));
    // The inactive track: from past the handle to the end (under the whole
    // slider without gaps, the fill drawn over it).
    let rest = if painted {
        None
    } else if gapped {
        (v < 1.0).then(|| margin_start(track().inset_start(gpui::relative(v)).inset_end(px(-d.end_inset)), side).bg(inactive.hsla()))
    } else {
        Some(track().inset_start(px(-d.start_inset)).inset_end(px(-d.end_inset)).bg(inactive.hsla()))
    };
    // The active track: from the start to short of the handle, or without
    // gaps on to the slider's end at value 1.
    let active_end = if gapped { side } else { -d.end_inset * v };
    let active = (!painted).then(|| margin_end(fill(track().inset_start(px(-d.start_inset)).inset_end(gpui::relative(1.0 - v))), active_end));
    let knob = handle.then(|| {
        let w = handle_width(&d, press);
        if design == SliderDesign::Continuous {
            // A grip inside the fill's end, in the fill's own ink.
            let grip_h = d.track_h * GRIP_H;
            margin_start(div().absolute().top(px((d.h - grip_h) / 2.0)).h(px(grip_h)).w(px(w)).inset_start(gpui::relative(v)), -w / 2.0 - GRIP_BACK_PX).rounded_full().bg(on_fill.with_alpha(GRIP_ALPHA).hsla())
        } else {
            margin_start(div().absolute().top(px((d.h - d.handle_h) / 2.0)).h(px(d.handle_h)).w(px(w)).inset_start(gpui::relative(v)), -w / 2.0).rounded_full().bg(to.hsla())
        }
    });
    // The stop dot at the inactive track's end.
    let stop = (gapped && v < STOP_DOT_HIDE).then(|| {
        div().absolute().top(px((d.h - d.stop_dot) / 2.0)).inset_end(px((d.track_h - d.stop_dot) / 2.0)).size(px(d.stop_dot)).rounded_full().bg(to.hsla())
    });
    // The canvas-drawn parts: the pill and the steps under the icon, the
    // wave on the fill.
    let inks = Inks {
        from,
        to,
        rest: if design == SliderDesign::Segmented { inactive.mix(crate::plugins::panel_fx::wash(crate::plugins::panel_fx::WASH_STRONG, palette), press.clamp(0.0, 1.0)) } else { inactive },
        // A light bead in both themes (the lighter of the surface and its ink).
        knob: if palette.surface_base.lightness() < palette.on_surface.lightness() { palette.on_surface } else { palette.surface_base },
        ring: if look.muted { palette.on_surface_variant } else { palette.primary },
        angle,
    };
    // The wave: the fill's ink softened toward the fill, opaque (its dots
    // overlap, so a translucent ink would bead).
    let wave_ink = if look.muted { on_fill } else { on_fill.mix(from, WAVE_SOFTEN) };
    let drawn = match design {
        SliderDesign::Pill => Some(
            canvas(|_, _, _| {}, move |bounds, _, window, _| {
                for piece in pill_pieces(f32::from(bounds.size.width), v, press) {
                    paint_piece(window, &piece, bounds, rtl, inks);
                }
            })
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        ),
        SliderDesign::Segmented => Some(
            canvas(|_, _, _| {}, move |bounds, _, window, _| {
                for piece in segment_pieces(f32::from(bounds.size.width), v) {
                    paint_piece(window, &piece, bounds, rtl, inks);
                }
            })
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        ),
        SliderDesign::Wavy => Some(
            canvas(|_, _, _| {}, move |bounds, _, window, _| {
                let width = f32::from(bounds.size.width);
                let r = WAVE_STROKE_PX / 2.0;
                for (x, y) in wave_dots(width, v, handle, press, phase) {
                    let x = if rtl { width - x } else { x };
                    let dot = Bounds::new(point(bounds.origin.x + px(x - r), bounds.origin.y + px(y - r)), gpui::size(px(WAVE_STROKE_PX), px(WAVE_STROKE_PX)));
                    // Unsnapped, so the curve stays smooth.
                    window.paint_subpixel_quad(gpui::fill(dot, wave_ink.hsla()).corner_radii(px(r)));
                }
            })
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        ),
        SliderDesign::Classic | SliderDesign::Continuous => None,
    };
    let icon = look.icon.map(|name| {
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
        .child(travel().children(rest).children(active).children(knob))
        .children(stop)
        .children(drawn)
        .children(icon)
        .children(bubble)
        // Keeps frames coming while the press springs (and the wave flows).
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

    /// Thick slider widths the shell draws (mixer rows up to the sidebar).
    const WIDTHS: [f32; 4] = [120.0, 180.0, 260.0, 420.0];
    /// Values from empty to full.
    fn values() -> impl Iterator<Item = f32> {
        (0..=40).map(|i| i as f32 / 40.0)
    }
    /// Presses from resting past a springy overshoot.
    const PRESSES: [f32; 4] = [0.0, 0.5, 1.0, 1.2];
    /// The default motion, whatever other tests set globally.
    const MOTION: mandara_core::spring::MotionProfile = mandara_core::spring::MotionProfile { feel: mandara_core::SpringConfig::new(mandara_core::spring::DEFAULT_STIFFNESS, mandara_core::spring::DEFAULT_DAMPING, mandara_core::spring::DEFAULT_MASS), reduce: false };

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

    /// The classic thick slider is v0.22.0's: the same metrics, the handle
    /// between two gaps, a stop dot until the very end.
    #[test]
    fn the_classic_design_keeps_the_v0_22_metrics() {
        assert_eq!(SliderDesign::default(), SliderDesign::Classic);
        assert_eq!(Size::Large.dims(), Dims { h: 40.0, track_h: 40.0, handle_h: 40.0, handle_w: 4.0, gap: 4.0, start_inset: 42.0, end_inset: 4.0, stop_dot: 6.0 });
        assert_eq!(Size::Standard.dims(), Dims { h: 24.0, track_h: 10.0, handle_h: 22.0, handle_w: 4.0, gap: 4.0, start_inset: 2.0, end_inset: 2.0, stop_dot: 4.0 });
        assert_eq!((PRESS_NARROW, STOP_DOT_HIDE), (0.5, 0.97));
        assert!(SliderDesign::Classic.uses_handle() && SliderDesign::Wavy.uses_handle() && SliderDesign::Continuous.uses_handle());
        assert!(!SliderDesign::Pill.uses_handle() && !SliderDesign::Segmented.uses_handle());
    }

    /// The Continuous design is one track: its grip sits inside the fill's
    /// end, clear of the icon even at zero, and never past the slider's end.
    #[test]
    fn the_continuous_grip_stays_inside_the_fill_and_clear_of_the_icon() {
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
    fn the_pill_knob_stays_on_the_track_clear_of_the_icon_and_grows_when_pressed() {
        let d = Size::Large.dims();
        for width in WIDTHS {
            for v in values() {
                for press in PRESSES {
                    let [rest, fill, knob] = pill_pieces(width, v, press);
                    assert_eq!((rest.x, rest.w), (0.0, width), "the track spans the slider");
                    assert!(fill.x == 0.0 && fill.end() <= width + 1e-3, "{width} {v}: the fill stays inside");
                    assert!(knob.x >= ICON_END_PX + KNOB_ICON_GAP_PX - 1e-3, "{width} {v} {press}: clear of the icon ({})", knob.x);
                    assert!(knob.end() <= width - KNOB_END_PAD_PX + 1e-3, "{width} {v} {press}: inside the slider's end");
                    assert!(knob.y >= 0.0 && knob.y + knob.h <= d.h, "inside the hit area");
                    assert!(knob.start_r * 2.0 <= knob.w.min(knob.h) + 1e-3, "fully rounded, never past its own size");
                    // Between the ends it sits under the pointer, on the
                    // fill's end (the fill reaches its centre).
                    let centre = value_x(v, width);
                    if centre - knob.w / 2.0 > ICON_END_PX + KNOB_ICON_GAP_PX && centre + knob.w / 2.0 < width - KNOB_END_PAD_PX {
                        assert!((knob.x + knob.w / 2.0 - centre).abs() < 1e-3, "{width} {v}: under the pointer");
                        assert!(fill.end() + 1e-3 >= centre, "{width} {v}: on the fill's end");
                    }
                }
            }
        }
        let ([.., resting], [.., held]) = (pill_pieces(260.0, 0.5, 0.0), pill_pieces(260.0, 0.5, 1.0));
        assert_eq!((resting.w, resting.h), (KNOB_PX, KNOB_PX), "a circle at rest");
        assert!(held.w > resting.w && held.h > held.w, "an upright pill while pressed");
    }

    #[test]
    fn segments_fill_in_order_and_never_overlap() {
        for width in WIDTHS {
            let mut last_filled = 0.0;
            for v in values() {
                // Each step's extent: its track, or its fill when full.
                let mut steps: Vec<(f32, f32)> = Vec::new();
                let (mut filled, mut cells) = (0.0, 0);
                for piece in segment_pieces(width, v) {
                    assert!(piece.x >= -1e-3 && piece.end() <= width + 1e-3, "{width} {v}: inside the slider");
                    assert!(piece.start_r <= piece.w / 2.0 + 1e-3 && piece.end_r <= piece.w / 2.0 + 1e-3 && piece.start_r <= piece.h / 2.0, "radii fit the piece");
                    match piece.kind {
                        PieceKind::Rest => cells += 1,
                        PieceKind::Fill { t0, t1 } => {
                            assert!(t0 >= 0.0 && t1 <= 1.0 + 1e-4 && t0 < t1, "a slice of the fill's gradient");
                            assert!(piece.x >= filled - 1e-3, "{width} {v}: fills run in order");
                            filled = piece.end();
                        }
                        PieceKind::Knob => panic!("no knob on the steps"),
                    }
                    match steps.last_mut() {
                        Some((start, end)) if (piece.x - *start).abs() < 1e-3 => *end = end.max(piece.end()),
                        _ => steps.push((piece.x, piece.end())),
                    }
                }
                assert_eq!(steps.len(), SEGMENTS, "{width} {v}: every step drawn once");
                assert!(steps.windows(2).all(|w| w[1].0 - w[0].1 >= SEGMENT_GAP_PX - 1e-3), "{width} {v}: a gap between steps, never an overlap");
                assert!(filled >= last_filled - 1e-3, "{width}: more value, more fill");
                assert!(filled >= ICON_END_PX, "{width} {v}: the fill holds the icon");
                last_filled = filled;
                if v == 1.0 {
                    assert_eq!(cells, 0, "full: no track left");
                    assert!((filled - width).abs() < 1e-3, "full: the last step fills to the end");
                }
            }
            assert_eq!(segment_pieces(width, 0.0).filter(|p| p.kind == PieceKind::Rest).count(), SEGMENTS, "{width}: every step shows at zero");
        }
    }

    #[test]
    fn the_wave_rides_the_fill_clear_of_the_icon_and_the_handle() {
        let d = Size::Large.dims();
        for width in WIDTHS {
            for v in values() {
                for handle in [true, false] {
                    for press in PRESSES {
                        let fill_end = if handle { value_x(v, width) - d.handle_w / 2.0 - d.gap } else { open_fill_end(v, width) };
                        for (x, y) in wave_dots(width, v, handle, press, 1.0) {
                            assert!(x - WAVE_STROKE_PX / 2.0 >= ICON_END_PX, "{width} {v}: clear of the icon");
                            assert!(x + WAVE_STROKE_PX / 2.0 <= fill_end, "{width} {v} {handle}: on the fill, short of the handle");
                            assert!(y - WAVE_STROKE_PX / 2.0 >= 0.0 && y + WAVE_STROKE_PX / 2.0 <= d.h, "inside the track");
                        }
                    }
                }
            }
        }
        // Too little fill for a wavelength: no wave.
        assert_eq!(wave_dots(260.0, 0.0, true, 0.0, 0.0).count(), 0);
        // A whole wavelength on is the resting wave again, so letting go
        // ends on exactly the picture it started from.
        let rest: Vec<_> = wave_dots(260.0, 0.8, true, 0.0, 0.0).collect();
        let around: Vec<_> = wave_dots(260.0, 0.8, true, 0.0, std::f32::consts::TAU).collect();
        assert!(rest.iter().zip(&around).all(|(a, b)| (a.1 - b.1).abs() < 1e-3));
        assert!(rest.iter().any(|(_, y)| (y - d.h / 2.0).abs() > WAVE_AMPLITUDE_PX * 0.9), "it really waves");
    }

    #[test]
    fn pieces_mirror_in_right_to_left_languages() {
        let origin = point(px(10.0), px(5.0));
        for piece in pill_pieces(260.0, 0.3, 1.0).into_iter().chain(segment_pieces(260.0, 0.45)) {
            let (ltr, rtl) = (piece.bounds(origin, 260.0, false), piece.bounds(origin, 260.0, true));
            assert_eq!(ltr.size, rtl.size);
            assert!((f32::from(ltr.origin.x - origin.x) + f32::from(rtl.origin.x - origin.x) + piece.w - 260.0).abs() < 1e-3, "mirrored about the slider's middle");
            let (l, r) = (piece.corners(false), piece.corners(true));
            assert_eq!((l.top_left, l.top_right), (r.top_right, r.top_left), "the start's corners move to the right");
        }
    }

    #[test]
    fn pressing_springs_the_handle_narrower_and_back() {
        let key: ElementId = "press-test".into();
        let t0 = Instant::now();
        let at = |ms: u64| t0 + std::time::Duration::from_millis(ms);
        assert_eq!(press_state_in(&key, false, false, t0, &MOTION), Press::REST, "untouched: nothing kept");
        let mut peak: f32 = 0.0;
        let mut ms = 0;
        loop {
            let p = press_state_in(&key, true, false, at(ms), &MOTION);
            assert_eq!(p.phase, 0.0, "no wave without flow");
            peak = peak.max(p.amount);
            if !p.moving {
                assert!((p.amount - 1.0).abs() < 1e-3, "settles pressed");
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
            let p = press_state_in(&key, false, false, at(ms), &MOTION);
            if !p.moving {
                assert_eq!(p.amount, 0.0);
                break;
            }
            ms += 7;
            assert!(ms < 4000, "springs back");
        }
        assert!(PRESS.with(|m| !m.borrow().contains_key(&key)), "a released slider's spring is dropped");
    }

    #[test]
    fn the_wave_flows_only_while_held_and_comes_to_rest_where_it_began() {
        let key: ElementId = "wave-test".into();
        let t0 = Instant::now();
        let at = |ms: u64| t0 + std::time::Duration::from_millis(ms);
        let mut ms = 0;
        let mut phases = Vec::new();
        for _ in 0..120 {
            let p = press_state_in(&key, true, true, at(ms), &MOTION);
            assert!(p.moving, "held: frames keep coming while it flows");
            assert!((0.0..std::f32::consts::TAU).contains(&p.phase));
            phases.push(p.phase);
            ms += 7;
        }
        assert!(phases.windows(2).any(|w| w[1] != w[0]), "it flows");
        loop {
            let p = press_state_in(&key, false, true, at(ms), &MOTION);
            if !p.moving {
                assert_eq!(p, Press::REST, "at rest the wave is back where it started");
                break;
            }
            ms += 7;
            assert!(ms < 6000, "it settles after a wavelength at most");
        }
        assert!(PRESS.with(|m| !m.borrow().contains_key(&key)), "nothing kept once still");
        assert_eq!(press_state_in(&key, false, true, at(ms + 7), &MOTION), Press::REST, "no idle frames");
    }

    #[test]
    fn reduced_motion_keeps_the_wave_still() {
        let reduced = mandara_core::spring::MotionProfile { reduce: true, ..Default::default() };
        let key: ElementId = "wave-reduced".into();
        let t0 = Instant::now();
        let held = press_state_in(&key, true, true, t0, &reduced);
        let later = press_state_in(&key, true, true, t0 + std::time::Duration::from_millis(200), &reduced);
        let released = press_state_in(&key, false, true, t0 + std::time::Duration::from_millis(207), &reduced);
        assert_eq!((held.phase, later.phase), (0.0, 0.0), "no flow");
        assert!(!later.moving, "no frames asked for while held still");
        assert_eq!(released, Press::REST, "let go: at rest at once");
    }
}
