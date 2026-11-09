//! Progress designs (`style.progress_design`): the shell's meters
//! ([`crate::plugins::panel_fx::meter`]: disks, temperatures, batteries,
//! processes, reading progress) and Studio's progress bars.
//!
//! - **Classic**: drawn by each bar's own code (a rounded track, the fill
//!   over it).
//! - **Material**: Material 3's linear indicator: the fill, a gap as wide
//!   as the bar is thick, the track, and a stop dot at its end.
//! - **Wavy**: Material 3 Expressive's wavy indicator: the fill drawn as a
//!   wave (flat near empty and full), then the gap and the track. The wave
//!   swings inside the bar's own height (a thinner stroke leaves it room),
//!   so a wavy bar takes exactly a classic bar's space; it never moves on
//!   its own.
//! - **Segmented**: ten rounded steps.
//!
//! [`pieces`] and [`wave`] are the pure geometry, in px from the bar's
//! reading start and top (mirrored right to left when painted), fixed-size
//! iterators that allocate nothing.

use gpui::{Bounds, Div, ParentElement as _, Pixels, Styled, canvas, div, point, px};
use mandara_core::{ToGpui as _, oklab_gradient};
use mandara_theme::Color;

use super::ProgressDesign;
use crate::slider::{Piece, PieceKind};

pub use crate::icons::ring_as;

/// Steps of the segmented design.
pub const SEGMENTS: usize = 10;
/// The Material gap is as wide as the bar is thick, within these bounds.
const GAP_PX: (f32, f32) = (2.0, 6.0);
/// The stop dot hides once the fill comes this close to the end.
const STOP_HIDE: f32 = 0.97;
/// Gap between segments as a share of the bar's thickness, and its least.
const SEGMENT_GAP_SHARE: f32 = 0.6;
const SEGMENT_GAP_MIN_PX: f32 = 2.0;
/// The wave's stroke as a share of the bar's thickness (Material's 4 dp
/// stroke in its 10 dp container), its wavelength as a share of the bar's
/// thickness (Material's 40 dp in 10), and the spacing of the dots that
/// draw it.
const WAVE_STROKE_SHARE: f32 = 0.4;
/// The thickest the wave gets (Material's 4 dp stroke): thick meters keep a
/// fine wave.
const WAVE_STROKE_MAX_PX: f32 = 4.0;
const WAVE_LENGTH_SHARE: f32 = 4.0;
/// The shortest wavelength (Material's indeterminate 20 dp): thin meters keep a
/// gentle wave.
const WAVE_LENGTH_MIN_PX: f32 = 20.0;
const WAVE_STEP_PX: f32 = 1.0;
/// The wave flattens below and above these values (Material's amplitude
/// function), easing over this much of the bar.
const WAVE_FLAT: (f32, f32) = (0.1, 0.95);
const WAVE_EASE: f32 = 0.05;
/// Gradient angle along the reading direction.
const ANGLE_START_TO_END_DEG: f32 = 90.0;
const HALF_TURN_DEG: f32 = 180.0;

/// How tall a bar of thickness `h` is drawn in `design`: `h` in every
/// design, so changing designs never moves what sits around a meter.
pub fn height(_design: ProgressDesign, h: f32) -> f32 {
    h
}

/// The wave's stroke (and the wavy track's thickness) for a bar `h` thick.
pub fn wave_stroke(h: f32) -> f32 {
    (h * WAVE_STROKE_SHARE).min(WAVE_STROKE_MAX_PX)
}

fn gap(h: f32) -> f32 {
    h.clamp(GAP_PX.0, GAP_PX.1)
}

/// The rounded pieces of a bar `w` wide whose stroke is `h` thick, at
/// `v` (`0..=1`) in `design`, inside a box `height(design, h)` tall. The
/// wavy design's fill is the [`wave`]; its pieces are the track and dot.
pub fn pieces(design: ProgressDesign, w: f32, h: f32, v: f32) -> impl Iterator<Item = Piece> {
    let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    let w = w.max(0.0);
    let box_h = height(design, h);
    // The wavy bar's track is as thick as its wave.
    let h = if design == ProgressDesign::Wavy { wave_stroke(h) } else { h };
    let (y, r) = ((box_h - h) / 2.0, h / 2.0);
    let fill_end = (v * w).max(if v > 0.0 { h.min(w) } else { 0.0 });
    let g = gap(h);
    let segments = if design == ProgressDesign::Segmented { SEGMENTS } else { 0 };
    let seg_gap = (h * SEGMENT_GAP_SHARE).max(SEGMENT_GAP_MIN_PX);
    let seg_w = ((w - seg_gap * (SEGMENTS as f32 - 1.0)) / SEGMENTS as f32).max(0.0);
    let at = v * w;
    // Material and Wavy: fill (not for Wavy), track past the gap, stop dot.
    let linear = matches!(design, ProgressDesign::Material | ProgressDesign::Wavy);
    let track_start = if v > 0.0 { fill_end + g } else { 0.0 };
    let (track_w, fill_w) = (w - track_start, fill_end.min(w));
    let track = (linear && track_start < w - r).then_some(Piece { kind: PieceKind::Rest, x: track_start, y, w: track_w, h, start_r: r.min(track_w / 2.0), end_r: r.min(track_w / 2.0) });
    let fill = (design == ProgressDesign::Material && v > 0.0).then_some(Piece { kind: PieceKind::Fill { t0: 0.0, t1: 1.0 }, x: 0.0, y, w: fill_w, h, start_r: r.min(fill_w / 2.0), end_r: r.min(fill_w / 2.0) });
    let stop = (linear && v < STOP_HIDE && track.is_some_and(|t| t.w >= 2.0 * h)).then_some(Piece { kind: PieceKind::Knob, x: w - h, y, w: h, h, start_r: r, end_r: r });
    let steps = (0..segments).flat_map(move |i| {
        let x = i as f32 * (seg_w + seg_gap);
        let end = x + seg_w;
        let filled = at.clamp(x, end);
        let sr = r.min(seg_w / 2.0);
        let rest = (filled < end).then_some(Piece { kind: PieceKind::Rest, x, y, w: seg_w, h, start_r: sr, end_r: sr });
        let fill = (filled > x).then(|| {
            let fw = filled - x;
            Piece { kind: PieceKind::Fill { t0: x / at.max(f32::EPSILON), t1: filled / at.max(f32::EPSILON) }, x, y, w: fw, h, start_r: sr.min(fw / 2.0), end_r: sr.min(fw / 2.0) }
        });
        rest.into_iter().chain(fill)
    });
    track.into_iter().chain(fill).chain(stop).chain(steps)
}

/// The wavy design's fill: centres of the SDF dots that draw it, and their
/// diameter, for a bar `w` wide with a stroke `h` thick at `v`. The wave
/// swings inside the bar's box and flattens near empty and full.
pub fn wave(w: f32, h: f32, v: f32) -> (impl Iterator<Item = (f32, f32)>, f32) {
    let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    let stroke = wave_stroke(h);
    let box_h = h;
    let amp_max = ((box_h - stroke) / 2.0).max(0.0);
    let ease = |x: f32| x.clamp(0.0, 1.0);
    let amp = amp_max * ease((v - WAVE_FLAT.0) / WAVE_EASE).min(ease((WAVE_FLAT.1 - v) / WAVE_EASE));
    let (x0, x1) = (stroke / 2.0, (v * w).max(if v > 0.0 { stroke } else { 0.0 }).min(w) - stroke / 2.0);
    let n = if v > 0.0 && x1 >= x0 { ((x1 - x0) / WAVE_STEP_PX) as usize + 1 } else { 0 };
    let k = std::f32::consts::TAU / (box_h * WAVE_LENGTH_SHARE).max(WAVE_LENGTH_MIN_PX);
    let cy = box_h / 2.0;
    ((0..n).map(move |i| {
        let x = (x0 + i as f32 * WAVE_STEP_PX).min(x1);
        (x, cy + amp * (k * x).sin())
    }), stroke)
}

/// A bar `h` thick in a non-classic `design` at `value`, its fill running
/// `from → to` over the `track` colour, as wide as its parent.
pub fn bar(value: f32, h: f32, from: Color, to: Color, track: Color, design: ProgressDesign) -> Div {
    let rtl = crate::bidi::rtl();
    let angle = if rtl { ANGLE_START_TO_END_DEG + HALF_TURN_DEG } else { ANGLE_START_TO_END_DEG };
    div().w_full().h(px(height(design, h))).child(
        canvas(|_, _, _| {}, move |bounds: Bounds<Pixels>, _, window, _| {
            let w = f32::from(bounds.size.width);
            for piece in pieces(design, w, h, value) {
                let (b, corners) = (piece.bounds(bounds.origin, w, rtl), piece.corners(rtl));
                let background: gpui::Background = match piece.kind {
                    PieceKind::Rest => track.hsla().into(),
                    PieceKind::Knob => to.hsla().into(),
                    PieceKind::Fill { t0, t1 } if from != to => oklab_gradient(angle, from.mix(to, t0), from.mix(to, t1)),
                    PieceKind::Fill { .. } => from.hsla().into(),
                };
                window.paint_quad(gpui::fill(b, background).corner_radii(corners));
            }
            if design == ProgressDesign::Wavy {
                let (dots, d) = wave(w, h, value);
                let r = d / 2.0;
                let span = (value.clamp(0.0, 1.0) * w).max(1.0);
                for (x, y) in dots {
                    let color = from.mix(to, (x / span).clamp(0.0, 1.0));
                    let x = if rtl { w - x } else { x };
                    let dot = Bounds::new(point(bounds.origin.x + px(x - r), bounds.origin.y + px(y - r)), gpui::size(px(d), px(d)));
                    window.paint_subpixel_quad(gpui::fill(dot, color.hsla()).corner_radii(px(r)));
                }
            }
        })
        .size_full(),
    )
}

// ── Rings ────────────────────────────────────────────────────────────────

/// What a ring arc draws.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ArcKind {
    /// The track past the value.
    Track,
    /// The fill up to the value.
    Fill,
    /// The fill drawn as a wave (the Wavy design).
    Wave,
}

/// One arc of a progress ring: where it starts and how far it sweeps,
/// radians clockwise from 3 o'clock (the fill starts at 12 o'clock).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Arc {
    pub kind: ArcKind,
    pub from: f32,
    pub sweep: f32,
}

/// Steps of the segmented ring.
pub const RING_SEGMENTS: usize = 12;
/// The gap between the fill and the track (Material's track-active space)
/// and between segments, radians.
const RING_GAP_RAD: f32 = 0.22;
const RING_SEGMENT_GAP_RAD: f32 = 0.14;
/// The wavy ring: waves around a full turn, and the amplitude as a share
/// of the stroke.
pub const RING_WAVES: f32 = 10.0;
const RING_WAVE_AMPLITUDE: f32 = 0.45;

/// The arcs of a ring at `v` (`0..=1`) in `design`: at most
/// [`RING_SEGMENTS`] + 1 of them, nothing allocated.
pub fn ring_arcs(design: ProgressDesign, v: f32) -> impl Iterator<Item = Arc> {
    use std::f32::consts::{FRAC_PI_2, TAU};
    let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    let start = -FRAC_PI_2;
    let filled = TAU * v;
    let gapped = matches!(design, ProgressDesign::Material | ProgressDesign::Wavy);
    let fill_kind = if design == ProgressDesign::Wavy { ArcKind::Wave } else { ArcKind::Fill };
    // Classic: the whole track, the fill over it.
    let classic = (design == ProgressDesign::Classic).then_some(Arc { kind: ArcKind::Track, from: 0.0, sweep: TAU });
    let fill = (!matches!(design, ProgressDesign::Segmented) && v > 0.005).then_some(Arc { kind: fill_kind, from: start, sweep: filled });
    // Material and Wavy: the track from past the fill to short of its start.
    let track = gapped.then(|| {
        let (from, sweep) = if v > 0.005 { (start + filled + RING_GAP_RAD, TAU - filled - 2.0 * RING_GAP_RAD) } else { (start, TAU) };
        Arc { kind: ArcKind::Track, from, sweep }
    }).filter(|a| a.sweep > 0.0);
    let segments = if design == ProgressDesign::Segmented { RING_SEGMENTS } else { 0 };
    let each = TAU / RING_SEGMENTS as f32;
    let steps = (0..segments).flat_map(move |i| {
        let from = start + i as f32 * each + RING_SEGMENT_GAP_RAD / 2.0;
        let sweep = each - RING_SEGMENT_GAP_RAD;
        let lit = (filled - i as f32 * each - RING_SEGMENT_GAP_RAD / 2.0).clamp(0.0, sweep);
        let track = (lit < sweep).then_some(Arc { kind: ArcKind::Track, from: from + lit, sweep: sweep - lit });
        let fill = (lit > 0.0).then_some(Arc { kind: ArcKind::Fill, from, sweep: lit });
        track.into_iter().chain(fill)
    });
    classic.into_iter().chain(track).chain(fill).chain(steps)
}

/// The wavy ring's radius at angle `a` for a ring of mid radius `r` and
/// stroke `stroke` (the wave stays inside `r ± stroke`).
pub fn ring_wave_radius(a: f32, r: f32, stroke: f32) -> f32 {
    r + stroke * RING_WAVE_AMPLITUDE * (RING_WAVES * a).sin()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_arcs_stay_in_one_turn_and_fill_to_the_value() {
        use std::f32::consts::TAU;
        for design in ProgressDesign::ALL {
            for i in 0..=20 {
                let v = i as f32 / 20.0;
                let arcs: Vec<Arc> = ring_arcs(design, v).collect();
                assert!(arcs.len() <= RING_SEGMENTS + 1, "{design:?}");
                let fill: f32 = arcs.iter().filter(|a| a.kind != ArcKind::Track).map(|a| a.sweep).sum();
                for a in &arcs {
                    assert!(a.sweep > 0.0 && a.sweep <= TAU + 1e-4, "{design:?} {v}: {a:?}");
                }
                match design {
                    ProgressDesign::Segmented => assert!(fill <= TAU * v + 1e-3),
                    _ if v > 0.005 => assert!((fill - TAU * v).abs() < 1e-3, "{design:?} {v}: fill {fill}"),
                    _ => assert_eq!(fill, 0.0),
                }
                if design != ProgressDesign::Classic {
                    // Nothing overlaps: the arcs fit in one turn.
                    let total: f32 = arcs.iter().map(|a| a.sweep).sum();
                    assert!(total <= TAU + 1e-3, "{design:?} {v}: {total}");
                }
            }
        }
        // Inside the ring's stroke band whatever the angle.
        for k in 0..360 {
            let a = k as f32 / 360.0 * TAU;
            let r = ring_wave_radius(a, 10.0, 2.0);
            assert!((r - 10.0).abs() <= 2.0 * RING_WAVE_AMPLITUDE + 1e-4);
        }
    }

    const WIDTHS: [f32; 4] = [40.0, 96.0, 180.0, 360.0];
    const THICK: [f32; 3] = [4.0, 6.0, 8.0];
    const DESIGNS: [ProgressDesign; 3] = [ProgressDesign::Material, ProgressDesign::Wavy, ProgressDesign::Segmented];

    fn values() -> impl Iterator<Item = f32> {
        (0..=20).map(|i| i as f32 / 20.0)
    }

    #[test]
    fn every_piece_and_wave_dot_stays_inside_the_bar() {
        for design in DESIGNS {
            for w in WIDTHS {
                for h in THICK {
                    let box_h = height(design, h);
                    for v in values() {
                        for p in pieces(design, w, h, v) {
                            assert!(p.x >= -1e-3 && p.end() <= w + 1e-3 && p.w >= 0.0, "{design:?} {w} {h} {v}: {p:?}");
                            assert!(p.y >= 0.0 && p.y + p.h <= box_h + 1e-3);
                            assert!(p.start_r <= p.h / 2.0 + 1e-4 && p.end_r <= p.h / 2.0 + 1e-4 && p.start_r <= p.w / 2.0 + 1e-3);
                            // Mirrored right to left, it covers the same span.
                            let (l, r) = (p.bounds(point(px(0.0), px(0.0)), w, false), p.bounds(point(px(0.0), px(0.0)), w, true));
                            assert!((f32::from(l.origin.x) - (w - f32::from(r.origin.x) - p.w)).abs() < 1e-3);
                        }
                        let (dots, d) = wave(w, h, v);
                        for (x, y) in dots {
                            assert!(x - d / 2.0 >= -1e-3 && x + d / 2.0 <= w + 1e-3, "{w} {h} {v}: x {x}");
                            assert!(y - d / 2.0 >= -1e-3 && y + d / 2.0 <= h + 1e-3, "{w} {h} {v}: y {y}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_fill_tracks_the_value_and_material_keeps_its_gap_and_stop() {
        let (w, h) = (200.0, 4.0);
        let fill = |design, v| pieces(design, w, h, v).filter(|p| matches!(p.kind, PieceKind::Fill { .. })).map(|p| p.w).sum::<f32>();
        assert_eq!(fill(ProgressDesign::Material, 0.0), 0.0);
        assert!((fill(ProgressDesign::Material, 0.5) - 100.0).abs() < 1e-3);
        assert!((fill(ProgressDesign::Segmented, 1.0) - (w - 9.0 * (h * SEGMENT_GAP_SHARE).max(SEGMENT_GAP_MIN_PX))).abs() < 1e-2, "every step full");
        let half: Vec<Piece> = pieces(ProgressDesign::Material, w, h, 0.5).collect();
        let track = half.iter().find(|p| p.kind == PieceKind::Rest).unwrap();
        assert!((track.x - (100.0 + gap(h))).abs() < 1e-3, "a gap after the fill");
        assert!(half.iter().any(|p| p.kind == PieceKind::Knob), "a stop dot");
        assert!(!pieces(ProgressDesign::Material, w, h, 1.0).any(|p| p.kind == PieceKind::Knob), "no dot when full");
        // Flat near empty and full, swinging in between.
        let swing = |v| { let (d, _) = wave(w, h, v); d.map(|(_, y)| (y - h / 2.0).abs()).fold(0.0, f32::max) };
        assert!(swing(0.05) < 1e-3 && swing(0.99) < 1e-3 && swing(0.5) > 1.0);
        for design in ProgressDesign::ALL {
            assert_eq!(height(design, h), h, "{design:?}: a classic bar's height");
        }
    }
}
