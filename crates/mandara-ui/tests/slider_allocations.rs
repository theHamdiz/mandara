//! Every thick-slider design paints its frames without allocating: the
//! press spring (and the Wavy design's wave phase) reuse the held slider's
//! entry, and the canvas-drawn pieces and wave dots are fixed arrays and
//! iterators turned straight into bounds.

use std::hint::black_box;
use std::time::{Duration, Instant};

use gpui::{ElementId, point, px};
use mandara_ui::slider::{PieceKind, SliderDesign, pill_pieces, press_state, segment_pieces, wave_dots};

#[path = "support/allocations.rs"]
mod allocations;
use allocations::allocations;

/// Frames drawn per measurement (a long drag at 144 Hz).
const FRAMES: usize = 288;
/// One 144 Hz frame.
const FRAME: Duration = Duration::from_micros(6944);
/// Slider widths the shell draws: a mixer row and the sidebar.
const WIDTHS: [f32; 2] = [180.0, 360.0];

/// Everything one frame of `design` computes for painting, folded into a
/// number so nothing is optimised away.
fn frame(design: SliderDesign, key: &ElementId, now: Instant, width: f32, v: f32, rtl: bool) -> f32 {
    let press = press_state(key, true, design == SliderDesign::Wavy, now);
    let origin = point(px(8.0), px(4.0));
    let mut sum = press.amount + press.phase;
    match design {
        SliderDesign::Pill => {
            for piece in pill_pieces(width, v, press.amount) {
                let b = piece.bounds(origin, width, rtl);
                sum += f32::from(b.origin.x) + f32::from(piece.corners(rtl).top_left);
            }
        }
        SliderDesign::Segmented => {
            for piece in segment_pieces(width, v) {
                let b = piece.bounds(origin, width, rtl);
                sum += f32::from(b.size.width) + if let PieceKind::Fill { t1, .. } = piece.kind { t1 } else { 0.0 };
            }
        }
        SliderDesign::Wavy => {
            for (x, y) in wave_dots(width, v, true, press.amount, press.phase) {
                sum += if rtl { width - x } else { x } + y;
            }
        }
        // The divs of these two carry no per-frame geometry of their own.
        SliderDesign::Classic | SliderDesign::Continuous => sum += mandara_ui::slider::handle_width(&mandara_ui::slider::Size::Large.dims(), press.amount),
    }
    sum
}

#[test]
fn every_slider_design_paints_its_frames_without_allocating() {
    let start = Instant::now();
    for (n, design) in SliderDesign::ALL.into_iter().enumerate() {
        let key: ElementId = ("slider-allocations", n).into();
        // The press itself creates the slider's entry once.
        black_box(frame(design, &key, start, WIDTHS[0], 0.5, false));
        for width in WIDTHS {
            for rtl in [false, true] {
                let counted = allocations(|| {
                    for i in 0..FRAMES {
                        let v = i as f32 / FRAMES as f32;
                        black_box(frame(design, &key, start + FRAME * (i as u32 + 1), width, v, rtl));
                    }
                });
                assert_eq!(counted, (0, 0), "{design:?} at {width} px, rtl {rtl}: no allocation per frame");
            }
        }
        // Released, the entry settles and is dropped.
        let mut t = start + FRAME * (FRAMES as u32 + 1);
        while press_state(&key, false, design == SliderDesign::Wavy, t).moving {
            t += FRAME;
        }
    }
    // A positive control: collecting the same geometry does allocate.
    let control = allocations(|| {
        black_box(wave_dots(360.0, 0.8, true, 1.0, 0.5).collect::<Vec<_>>());
        black_box(segment_pieces(360.0, 0.5).collect::<Vec<_>>());
    });
    assert!(control.0 >= 2, "the counter sees allocations ({control:?})");
    eprintln!("slider designs: 0 allocations over {FRAMES} frames each; collecting control {control:?}");
}
