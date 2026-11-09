//! Every component design computes its frames without allocating: the cut
//! corners' rows, the progress designs' pieces and wave dots, the switch
//! thumb, check box and radio geometry, the button morph and the connected
//! and card corners are fixed arrays, iterators and plain numbers turned
//! straight into bounds.

use std::hint::black_box;

use gpui::{point, px};
use mandara_ui::components::{ButtonDesign, ProgressDesign, SurfaceCorners, ToggleDesign, buttons, chips, cut, progress, surfaces, toggles};

#[path = "support/allocations.rs"]
mod allocations;
use allocations::allocations;

/// Frames per measurement (a two-second animation at 144 Hz).
const FRAMES: usize = 288;
/// Widths the shell draws meters and buttons at.
const WIDTHS: [f32; 3] = [64.0, 180.0, 360.0];
/// Display scales: the owner's 1.5 and a 2× panel.
const SCALES: [f32; 2] = [1.5, 2.0];

/// One frame of every design at animation progress `t`, folded into a
/// number so nothing is optimised away.
fn frame(t: f32, w: f32, scale: f32, rtl: bool) -> f32 {
    let mut sum = 0.0;
    for design in ButtonDesign::ALL {
        sum += buttons::radius(design, 34.0, t) + buttons::icon_radius(design, 32.0, t);
    }
    for row in cut::rows(34.0, buttons::cut(34.0), scale) {
        sum += row.y + row.inset;
    }
    for (x, y, ew, eh) in cut::edges(w, 34.0, buttons::cut(34.0), 1.0) {
        sum += x + y + ew + eh;
    }
    for design in [ToggleDesign::Material, ToggleDesign::Expressive, ToggleDesign::Round] {
        let th = toggles::thumb(design, t);
        sum += th.x + th.w + toggles::check_radius(design, 18.0, t) + toggles::radio_dot(design, 18.0, t);
    }
    for design in [ProgressDesign::Material, ProgressDesign::Wavy, ProgressDesign::Segmented] {
        for piece in progress::pieces(design, w, 6.0, t.clamp(0.0, 1.0)) {
            let b = piece.bounds(point(px(2.0), px(3.0)), w, rtl);
            sum += f32::from(b.origin.x) + f32::from(piece.corners(rtl).top_left);
        }
    }
    let (dots, d) = progress::wave(w, 6.0, t.clamp(0.0, 1.0));
    for (x, y) in dots {
        sum += if rtl { w - x } else { x } + y + d;
    }
    for i in 0..3 {
        let place = chips::Place::in_reading_order(i, 3, rtl);
        sum += f32::from(chips::connected_corners(place, i == 1, 28.0).top_left) + f32::from(chips::group_corners(place, 14.0, 28.0).top_right);
    }
    for family in SurfaceCorners::ALL {
        sum += f32::from(surfaces::corners(family, 18.0, rtl).top_left);
        // Squircle and cut cards: their corner rows.
        if let Some(corner) = surfaces::shaped(family, 18.0) {
            for row in cut::shape_rows(w.min(160.0), corner, scale) {
                sum += row.inset + row.stretch;
            }
        }
    }
    for design in ProgressDesign::ALL {
        for arc in progress::ring_arcs(design, t.clamp(0.0, 1.0)) {
            sum += arc.from + arc.sweep;
        }
    }
    sum
}

#[test]
fn every_component_design_draws_its_frames_without_allocating() {
    for w in WIDTHS {
        for scale in SCALES {
            for rtl in [false, true] {
                let counted = allocations(|| {
                    for i in 0..FRAMES {
                        black_box(frame(i as f32 / FRAMES as f32 * 1.2 - 0.1, w, scale, rtl));
                    }
                });
                assert_eq!(counted, (0, 0), "{w} px at {scale}×, rtl {rtl}: no allocation per frame");
            }
        }
    }
    // A positive control: collecting the same geometry does allocate.
    let control = allocations(|| {
        black_box(cut::rows(34.0, 9.0, 1.5).collect::<Vec<_>>());
        black_box(progress::pieces(ProgressDesign::Segmented, 360.0, 6.0, 0.5).collect::<Vec<_>>());
    });
    assert!(control.0 >= 2, "the counter sees allocations ({control:?})");
    eprintln!("component designs: 0 allocations over {FRAMES} frames each; collecting control {control:?}");
}
