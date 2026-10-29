//! Seamless video loops: frames at loop phases `t = i / n` encoded to AV1
//! WebM through the screen recorder's [`VideoSink`]. Every style's motion
//! is periodic in `t` (orbits, whole cycles of sines, noise lattices that
//! wrap in depth), so the frame after the last is the first and the clip
//! loops without a seam. Rendering of the next frame overlaps encoding of
//! the current one.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mandara_i18n::t;
use mandara_services::recording::{SinkOptions, VideoSink};

use crate::output::{self, Generated};
use crate::recipe::Recipe;
use crate::render;
use crate::resolution::Resolution;

/// Loop length limits (seconds) and frame-rate limits.
const MIN_SECONDS: f32 = 1.0;
const MAX_SECONDS: f32 = 60.0;
const MIN_FPS: u32 = 1;
const MAX_FPS: u32 = 60;
/// Default loop: 10 s at 24 fps, 1080p, 10-bit.
const DEFAULT_SECONDS: f32 = 10.0;
const DEFAULT_FPS: u32 = 24;
const DEFAULT_QUALITY: u8 = 70;
/// rav1e's fastest preset: pure-Rust rav1e is slow, and slow, calm motion
/// gains little from the slower presets.
const DEFAULT_SPEED: u8 = 10;
/// Quantizer at quality 0 and 100 (rav1e's 0–255 scale, lower is better).
const QUANTIZER_WORST: f32 = 200.0;
const QUANTIZER_BEST: f32 = 40.0;
/// The minimum quantizer is this share of the base one.
const MIN_QUANTIZER_SHARE: f32 = 0.6;
/// Frames rendered ahead of the encoder.
const RENDER_AHEAD: usize = 2;

/// How [`render_video`] encodes.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoOptions {
    /// Loop length in seconds (1–60).
    pub seconds: f32,
    /// Frames per second (1–60).
    pub fps: u32,
    pub resolution: Resolution,
    /// 0 (smallest file) – 100 (best picture).
    pub quality: u8,
    /// rav1e speed preset: 0 (slowest, smallest) – 10 (fastest).
    pub speed: u8,
    /// 10-bit AV1: smoother dark gradients for a little more decode work.
    pub ten_bit: bool,
    /// File stem; default `<style>-<short seed>-<W>x<H>-loop`.
    pub name: Option<String>,
}

impl Default for VideoOptions {
    fn default() -> Self {
        VideoOptions { seconds: DEFAULT_SECONDS, fps: DEFAULT_FPS, resolution: Resolution::Fhd1080, quality: DEFAULT_QUALITY, speed: DEFAULT_SPEED, ten_bit: true, name: None }
    }
}

impl VideoOptions {
    /// Frames in the loop.
    pub fn frames(&self) -> u32 {
        let s = if self.seconds.is_finite() { self.seconds.clamp(MIN_SECONDS, MAX_SECONDS) } else { DEFAULT_SECONDS };
        ((s * self.fps.clamp(MIN_FPS, MAX_FPS) as f32).round() as u32).max(1)
    }
}

/// Renders the recipe's seamless loop to `dir` as AV1 WebM (atomically,
/// with a recipe sidecar). `progress` gets 0–1 as frames are encoded;
/// setting `cancel` stops early and removes the partial file.
pub fn render_video(recipe: &Recipe, opts: &VideoOptions, dir: &Path, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<Generated, String> {
    let (w, h) = opts.resolution.size();
    let (w, h) = ((w & !1).max(2), (h & !1).max(2));
    let fps = opts.fps.clamp(MIN_FPS, MAX_FPS);
    let frames = opts.frames();
    let stem = opts.name.clone().unwrap_or_else(|| format!("{}-loop", output::default_stem(recipe, w, h)));
    let path = output::unique_path(dir, &stem, "webm");
    let tmp = output::temp_path(&path);
    std::fs::create_dir_all(dir).map_err(|e| output::io_error(dir, &e))?;
    let q = QUANTIZER_WORST + (QUANTIZER_BEST - QUANTIZER_WORST) * opts.quality.min(100) as f32 / 100.0;
    let sink_opts = SinkOptions {
        speed: opts.speed,
        quantizer: q as u8,
        min_quantizer: (q * MIN_QUANTIZER_SHARE) as u8,
        low_latency: false,
        // One key frame: a loop restarts at frame 0.
        max_key_interval: frames,
        min_key_interval: frames,
        ten_bit: opts.ten_bit,
        ..SinkOptions::new(w, h, fps)
    };
    let result = (|| -> Result<u64, String> {
        let mut sink = VideoSink::create(&tmp, &sink_opts)?;
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u16>>(RENDER_AHEAD);
        std::thread::scope(|s| -> Result<u64, String> {
            // Owned here so an early return drops it and unblocks the renderer.
            let rx = rx;
            s.spawn(move || {
                for i in 0..frames {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let frame = render::render_rgb16(recipe, w, h, i as f32 / frames as f32);
                    if tx.send(frame).is_err() {
                        break;
                    }
                }
            });
            for i in 0..frames {
                if cancel.load(Ordering::Relaxed) {
                    return Err(t("Cancelled").into());
                }
                let Ok(frame) = rx.recv() else { return Err(t("Cancelled").into()) };
                sink.push_rgb16(&frame, Duration::from_secs_f64(i as f64 / fps as f64))?;
                progress((i + 1) as f32 / frames as f32);
            }
            Ok(0)
        })?;
        let (_, bytes) = sink.finish(Duration::from_secs_f64(frames as f64 / fps as f64))?;
        Ok(bytes)
    })();
    let bytes = match result {
        Ok(b) => b,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };
    if cancel.load(Ordering::Relaxed) {
        let _ = std::fs::remove_file(&tmp);
        return Err(t("Cancelled").into());
    }
    std::fs::rename(&tmp, &path).map_err(|e| output::io_error(&path, &e))?;
    let recipe_path = output::write_sidecar(&path, recipe, w, h, "video")?;
    Ok(Generated { path, recipe_path, width: w, height: h, bytes })
}
