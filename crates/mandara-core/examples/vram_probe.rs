//! Measures the GPU memory one GPUI layer surface costs at a given size.
//!
//! Opens `count` invisible (transparent, input-masked, background-layer)
//! surfaces of `width × height` logical px one by one and prints the
//! process's VRAM (DRM fdinfo) after each settles:
//!
//! ```sh
//! cargo run -p mandara-core --example vram_probe -- 2560 1440 2
//! ```
//!
//! With a fourth argument `paths=<side>`, each surface keeps a small
//! (almost invisible) ring path drawn, like the bar's progress rings, and
//! also animates a big ring inside a `side × side` box for [`PATH_FRAMES`]
//! frames, like an opened sidebar, then [`IDLE_FRAMES`] more frames with only
//! the small ring; VRAM is printed while the big ring is drawn and after,
//! which shows the cost of path rendering and whether it is given back.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, AppContext as _, Context, IntoElement, ParentElement as _, PathBuilder, Render, Styled as _, Window, canvas, div,
    point, px, size,
};
use mandara_core::{LayerSurfaceSpec, SurfaceRole};

/// Settle time after opening a surface before sampling, so the first frame
/// (and its lazily created textures) exists.
const SETTLE: Duration = Duration::from_millis(1500);
/// Default probe size: the user's 2560×1440 logical output.
const DEFAULT_W: f32 = 2560.0;
const DEFAULT_H: f32 = 1440.0;
const KIB_PER_MIB: u64 = 1024;
/// Frames drawn with the ring path, then without any path (more than the
/// renderer's idle-release threshold).
const PATH_FRAMES: u32 = 90;
const IDLE_FRAMES: u32 = 150;
/// Ring stroke width and alpha (nearly invisible on the desktop).
const RING_STROKE_PX: f32 = 2.0;
const RING_ALPHA: f32 = 0.02;
/// Side of the always-drawn small ring, logical px.
const SMALL_RING_PX: f32 = 24.0;
/// Segments of the ring polyline.
const RING_SEGMENTS: usize = 96;

struct Blank {
    cleared: bool,
    /// Side of the box the ring path animates in (`None`: no paths).
    path_box: Option<f32>,
    frame: u32,
    /// Phase the probe loop reads: 0 drawing paths, 1 idle frames, 2 done.
    phase: Rc<Cell<u8>>,
}

impl Render for Blank {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if !self.cleared {
            window.set_input_region(Some(&[]));
            self.cleared = true;
        }
        let Some(side) = self.path_box else { return div().size_full() };
        self.frame += 1;
        let drawing = self.frame <= PATH_FRAMES;
        self.phase.set(if drawing { 0 } else if self.frame <= PATH_FRAMES + IDLE_FRAMES { 1 } else { 2 });
        if self.phase.get() < 2 {
            window.request_animation_frame();
        }
        // A small ring always drawn (like the bar's progress rings), plus the
        // big one while `drawing` (like an opened sidebar's paths).
        let root = div().size_full().child(div().absolute().size(px(SMALL_RING_PX)).child(ring(SMALL_RING_PX, 1.0)));
        if !drawing {
            return root;
        }
        root.child(div().absolute().size_full().child(ring(side, self.frame as f32 / PATH_FRAMES as f32)))
    }
}

/// A ring path growing inside a `side × side` box at the window origin.
fn ring(side: f32, grow: f32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let r = side / 2.0 * (0.5 + 0.5 * grow.min(1.0)) - RING_STROKE_PX;
            let c = (f32::from(bounds.origin.x) + side / 2.0, f32::from(bounds.origin.y) + side / 2.0);
            let mut pb = PathBuilder::stroke(px(RING_STROKE_PX));
            for i in 0..=RING_SEGMENTS {
                let a = i as f32 / RING_SEGMENTS as f32 * std::f32::consts::TAU;
                let p = point(px(c.0 + r * a.cos()), px(c.1 + r * a.sin()));
                if i == 0 { pb.move_to(p) } else { pb.line_to(p) }
            }
            if let Ok(path) = pb.build() {
                window.paint_path(path, gpui::white().opacity(RING_ALPHA));
            }
        },
    )
    .size_full()
}

/// Sum of `drm-memory-vram` over this process's DRM fds, in KiB.
fn vram_kib() -> u64 {
    let mut total = 0;
    let mut seen = std::collections::HashSet::new();
    for entry in std::fs::read_dir("/proc/self/fdinfo").into_iter().flatten().flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
        let client = text.lines().find_map(|l| l.strip_prefix("drm-client-id:")).map(|s| s.trim().to_string());
        let Some(client) = client else { continue };
        if !seen.insert(client) {
            continue;
        }
        total += text
            .lines()
            .filter_map(|l| l.strip_prefix("drm-memory-vram:"))
            .filter_map(|v| v.split_whitespace().next()?.parse::<u64>().ok())
            .sum::<u64>();
    }
    total
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |i: usize, default: f32| args.get(i).and_then(|a| a.parse().ok()).unwrap_or(default);
    let (w, h) = (arg(1, DEFAULT_W), arg(2, DEFAULT_H));
    let count = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(1_usize);
    let path_box: Option<f32> = args.get(4).and_then(|a| a.strip_prefix("paths=")).and_then(|s| s.parse().ok());
    gpui_platform::application().run(move |cx: &mut App| {
        cx.spawn(async move |cx| {
            println!("baseline: {} MiB", vram_kib() / KIB_PER_MIB);
            let mut last = vram_kib();
            for i in 0..count {
                let spec = LayerSurfaceSpec::background(size(px(w), px(h)), None);
                let mut spec = spec;
                spec.role = SurfaceRole::Exclusion;
                let phase = Rc::new(Cell::new(0_u8));
                let view_phase = phase.clone();
                let opened = cx.update(|cx| {
                    cx.open_window(spec.window_options(), |_, cx| cx.new(|_| Blank { cleared: false, path_box, frame: 0, phase: view_phase }))
                });
                if let Err(err) = opened {
                    eprintln!("open failed: {err:#}");
                    break;
                }
                if path_box.is_some() {
                    let mut peak = 0;
                    while phase.get() == 0 {
                        peak = peak.max(vram_kib());
                        cx.background_executor().timer(Duration::from_millis(50)).await;
                    }
                    while phase.get() < 2 {
                        cx.background_executor().timer(Duration::from_millis(50)).await;
                    }
                    cx.background_executor().timer(SETTLE).await;
                    let now = vram_kib();
                    println!(
                        "surface {} ({w}×{h}, ring paths in {}²): +{} MiB while drawing the big ring, +{} MiB {IDLE_FRAMES} frames after",
                        i + 1,
                        path_box.unwrap_or_default(),
                        peak.saturating_sub(last) / KIB_PER_MIB,
                        now.saturating_sub(last) / KIB_PER_MIB
                    );
                    last = now;
                    continue;
                }
                cx.background_executor().timer(SETTLE).await;
                let now = vram_kib();
                println!("surface {} ({w}×{h}): +{} MiB (total {} MiB)", i + 1, now.saturating_sub(last) / KIB_PER_MIB, now / KIB_PER_MIB);
                last = now;
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
