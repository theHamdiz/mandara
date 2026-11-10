//! The wallpaper depth model: monocular depth estimation for the depth
//! clock's "model" mode, pure Rust end to end.
//!
//! **Model.** [Depth Anything V2 Small](https://github.com/DepthAnything/Depth-Anything-V2)
//! (24.8 M parameters, **Apache-2.0**; the Base / Large variants are
//! non-commercial and are not used), as the dynamically quantized ONNX
//! export of `onnx-community/depth-anything-v2-small`, pinned to one
//! revision and checked by SHA-256 ([`URL`], [`SHA256`]): 27 MB, downloaded
//! once into `~/.local/share/mandara/models/depth-anything-v2-small/` when
//! someone picks the model mode (or presses Download), never bundled.
//!
//! Why this one: it was measured against U²-Net-p (salient-object
//! segmentation, 4.7 MB, Apache-2.0) on real wallpapers. U²-Net-p found a
//! single "salient" blob and missed most landscapes (cities came out as
//! noise, mountains as nothing); a depth map orders the whole scene from far
//! to near, which is exactly what tucking a clock *into* a picture needs,
//! and its outlines (antlers, skylines, ridges) are far cleaner. MODNet is
//! portrait-only and non-commercial; IS-Net / BiRefNet are 40–180 MB. The
//! quantized export matches the fp32 one visually at a quarter of the
//! download and less peak memory; the fp16 export runs ~30× slower in
//! tract (no half-precision kernels).
//!
//! **Engine.** `tract-onnx`, the pure-Rust ONNX runtime the OCR feature
//! (`pure-onnx-ocr`) already links: no C / C++ runtime, nothing new built.
//!
//! **Guardrails.** Inference never runs inside the shell. [`run_worker`]
//! starts `mandara depth-worker` (the same binary) once per wallpaper, which
//! first lowers itself to `SCHED_IDLE` and nice [`NICE`], caps its own data
//! segment ([`DATA_CAP_BYTES`]) and CPU time ([`CPU_BUDGET_SECS`]), runs the
//! model single-threaded on a [`INPUT_SIDE`]² downscale with a CPU-time
//! budget checked between graph nodes ([`INFERENCE_BUDGET`]), writes a small
//! 16-bit depth map to the cache and exits, so the model and its buffers are
//! gone the moment it is done. The shell waits off the UI thread and kills
//! it past [`WALL_BUDGET`]. The budgets count CPU time, not wall time: at
//! idle priority a busy machine only stretches the run, it never costs the
//! user anything. Nothing runs at idle and nothing polls: a cached depth map
//! is reused until the wallpaper file changes.
//!
//! **Post-processing** (in the shell, a few MB for a moment): the depth map
//! is normalized, split into "in front of the clock" and "behind it" (where
//! the far backdrop ends, per wallpaper, or the user's split), and the soft split is
//! snapped to the photo's own edges with a guided filter at mask resolution,
//! so outlines follow the picture rather than the model's 37 × 37 patch grid.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use image::{DynamicImage, GrayImage, ImageBuffer, Luma, RgbImage};
use serde::{Deserialize, Serialize};

/// Model id in [`crate::models::MODELS`].
pub const MODEL_ID: &str = "depth-anything-v2-small";
/// The model file inside its folder.
pub const MODEL_FILE: &str = "model.onnx";
/// Project page shown in Studio.
pub const SOURCE: &str = "https://huggingface.co/onnx-community/depth-anything-v2-small";
/// The quantized export, pinned to a revision.
pub const URL: &str = "https://huggingface.co/onnx-community/depth-anything-v2-small/resolve/4472b7362082ad9968fee890ca0f1e5aca36b93d/onnx/model_quantized.onnx";
pub const SIZE: u64 = 27_258_801;
pub const SHA256: &str = "fcf51f1b230362b28690bb9d1809bf0431f29cad20534e3f589bd7285547f20d";

/// Model input side: 37 patches of 14 px, the size it was trained at.
pub const INPUT_SIDE: u32 = 518;
/// ImageNet normalization the model expects.
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// CPU-time budget of the model run, checked between graph nodes (a run
/// takes about 2 s of one core).
pub const INFERENCE_BUDGET: Duration = Duration::from_secs(45);
/// Kernel CPU-time cap of the helper (`RLIMIT_CPU`): above the inference
/// budget, so the budget's clean error normally wins.
pub const CPU_BUDGET_SECS: u64 = 60;
/// Wall-clock limit after which the shell kills the helper (generous: at
/// idle priority a busy machine may starve it for a while).
pub const WALL_BUDGET: Duration = Duration::from_secs(300);
/// Data-segment cap of the helper (`RLIMIT_DATA`): measured peaks stay
/// under a third of this, a pathological image fails instead of swapping.
pub const DATA_CAP_BYTES: u64 = 1024 * 1024 * 1024;
/// Largest decoded wallpaper the helper accepts (bytes of pixels).
pub const IMAGE_ALLOC_CAP: u64 = 512 * 1024 * 1024;
/// Niceness of the helper (the lowest priority).
pub const NICE: i32 = 19;
/// How often the waiting shell thread checks whether the helper exited
/// (only while a helper runs).
const WAIT_STEP: Duration = Duration::from_millis(50);

/// Robust normalization: the depth at these quantiles maps to 0 and 1.
const LOW_QUANTILE: f32 = 0.01;
const HIGH_QUANTILE: f32 = 0.99;
/// Histogram bins for quantiles and Otsu's threshold.
const BINS: usize = 256;
/// Relative difference under which two Otsu scores tie.
const OTSU_TIE: f64 = 1e-9;
/// Half-width of the soft step around the split, in normalized depth.
const SPLIT_BAND: f32 = 0.04;
/// Guided filter radius as a fraction of the mask's longer side, and its
/// regularization (smaller follows the photo's edges more closely).
const GUIDE_RADIUS_FRACTION: f32 = 0.004;
const GUIDE_MIN_RADIUS: usize = 2;
const GUIDE_EPS: f32 = 1e-3;
/// Final edge tightening after the guided filter (alpha below / above
/// these goes fully transparent / opaque).
const EDGE_LOW: f32 = 0.3;
const EDGE_HIGH: f32 = 0.7;
/// A subject smaller than this fraction of the frame is noise, larger than
/// [`MAX_SUBJECT`] leaves nothing behind the clock.
const MIN_SUBJECT: f32 = 0.02;
const MAX_SUBJECT: f32 = 0.9;
/// Raw depth range (between the 1st and 99th percentiles) under which the
/// picture has no real depth. Measured on 19 wallpapers: photos and
/// illustrated scenes span 3.5–7.3, mesh gradients and flat geometric art
/// about 3.0 (and would otherwise be cut along a colour edge).
const MIN_RAW_SPREAD: f32 = 3.2;
/// Lowest automatic split: just past the far background.
const AUTO_MIN_SPLIT: f32 = 0.06;
/// Raw depth stored at full scale in the 16-bit cache (the model's output
/// stays well under it).
const RAW_MAX: f32 = 32.0;

/// The model file.
pub fn model_path() -> PathBuf {
    crate::models::dir(MODEL_ID).join(MODEL_FILE)
}

/// Whether the model is downloaded.
pub fn installed() -> bool {
    crate::models::find(MODEL_ID).is_some_and(|m| m.installed())
}

/// The model's relative inverse depth (≥ 0, larger is nearer), row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct DepthMap {
    pub width: u32,
    pub height: u32,
    pub data: Vec<f32>,
}

impl DepthMap {
    /// Bilinear sample at `(x, y)` in map pixels (clamped).
    fn sample(&self, x: f32, y: f32) -> f32 {
        let (w, h) = (self.width as usize, self.height as usize);
        let (x, y) = (x.clamp(0.0, (w - 1) as f32), y.clamp(0.0, (h - 1) as f32));
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let at = |xx: usize, yy: usize| self.data[yy * w + xx];
        let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
        let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
        top * (1.0 - ty) + bottom * ty
    }

    /// Stored as a 16-bit grayscale PNG (a few hundred KB).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let img: ImageBuffer<Luma<u16>, Vec<u16>> = ImageBuffer::from_fn(self.width, self.height, |x, y| Luma([(self.data[(y * self.width + x) as usize].clamp(0.0, RAW_MAX) / RAW_MAX * u16::MAX as f32).round() as u16]));
        let tmp = path.with_extension("tmp");
        img.save_with_format(&tmp, image::ImageFormat::Png).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?.to_luma16();
        Ok(Self { width: img.width(), height: img.height(), data: img.pixels().map(|p| p.0[0] as f32 / u16::MAX as f32 * RAW_MAX).collect() })
    }
}

// ── Pre-processing ───────────────────────────────────────────────────────

/// `img` squeezed to `side`², as the model's normalized CHW input.
pub fn preprocess(img: &RgbImage, side: u32) -> Vec<f32> {
    let small = image::imageops::resize(img, side, side, image::imageops::FilterType::Triangle);
    let plane = (side * side) as usize;
    let mut out = vec![0.0f32; 3 * plane];
    for (i, p) in small.pixels().enumerate() {
        for c in 0..3 {
            out[c * plane + i] = (p.0[c] as f32 / 255.0 - MEAN[c]) / STD[c];
        }
    }
    out
}

/// The value at quantile `q` of `values` (exact; sorts a copy).
fn quantile(values: &[f32], q: f32) -> f32 {
    let mut v = values.to_vec();
    let k = ((v.len() - 1) as f32 * q).round() as usize;
    *v.select_nth_unstable_by(k, f32::total_cmp).1
}

/// Raw model output (relative inverse depth: larger is nearer) mapped to
/// `0..=1` between its 1st and 99th percentiles, so a few extreme pixels
/// can't flatten the rest.
pub fn normalize(raw: &[f32]) -> Vec<f32> {
    if raw.is_empty() {
        return Vec::new();
    }
    let (p_lo, p_hi) = (quantile(raw, LOW_QUANTILE), quantile(raw, HIGH_QUANTILE));
    if p_hi <= p_lo {
        return vec![0.0; raw.len()];
    }
    let span = p_hi - p_lo;
    raw.iter().map(|v| ((v - p_lo) / span).clamp(0.0, 1.0)).collect()
}

/// The square model output resized back to the wallpaper's aspect (longer
/// side [`INPUT_SIDE`]).
pub fn to_aspect(square: &[f32], side: u32, aspect: (u32, u32)) -> DepthMap {
    let (aw, ah) = (aspect.0.max(1) as f32, aspect.1.max(1) as f32);
    let scale = side as f32 / aw.max(ah);
    let (w, h) = (((aw * scale).round() as u32).max(1), ((ah * scale).round() as u32).max(1));
    let src = DepthMap { width: side, height: side, data: square.to_vec() };
    let (sx, sy) = (side as f32 / w as f32, side as f32 / h as f32);
    let mut data = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            data.push(src.sample((x as f32 + 0.5) * sx - 0.5, (y as f32 + 0.5) * sy - 0.5));
        }
    }
    DepthMap { width: w, height: h, data }
}

// ── Inference ────────────────────────────────────────────────────────────

/// Runs the model at `model` on `img` (blocking; normally inside the
/// helper process). The model is loaded, run once and dropped here;
/// `budget` is this process's CPU time spent so far plus the run's.
pub fn estimate(model: &Path, img: &DynamicImage, budget: Duration) -> Result<DepthMap, String> {
    use tract_onnx::prelude::*;
    let deadline = cpu_time() + budget;
    let side = INPUT_SIDE as usize;
    let plan = tract_onnx::onnx()
        // The export declares symbolic batch / size dims on intermediate
        // values; the concrete input fact below decides them.
        .with_ignore_output_shapes(true)
        .with_ignore_value_info(true)
        .model_for_path(model)
        .and_then(|m| m.with_input_fact(0, f32::fact([1, 3, side, side]).into()))
        .and_then(|m| m.into_optimized())
        .and_then(|m| m.into_runnable())
        .map_err(|e| format!("loading {}: {e}", model.display()))?;
    let rgb = img.to_rgb8();
    let input = Tensor::from_shape(&[1, 3, side, side], &preprocess(&rgb, INPUT_SIDE)).map_err(|e| e.to_string())?;
    drop(rgb);
    let mut state = plan.spawn().map_err(|e| e.to_string())?;
    let out = state
        .run_plan_with_eval(tvec!(input.into()), |ctx, op_state, node, inputs| {
            if cpu_time() > deadline {
                anyhow::bail!("over the {} s CPU budget", budget.as_secs());
            }
            tract_onnx::tract_core::plan::eval(ctx, op_state, node, inputs)
        })
        .map_err(|e| format!("depth model: {e}"))?;
    drop(state);
    drop(plan);
    let view = out[0].to_plain_array_view::<f32>().map_err(|e| e.to_string())?;
    let raw: Vec<f32> = view.iter().map(|v| v.max(0.0)).collect();
    if raw.len() != side * side {
        return Err(format!("depth model returned {} values, expected {}", raw.len(), side * side));
    }
    Ok(to_aspect(&raw, INPUT_SIDE, (img.width(), img.height())))
}

// ── Post-processing ──────────────────────────────────────────────────────

/// Otsu's threshold of `values` in `0..=1` and its separability (between-
/// class variance over total variance, `0..=1`).
pub fn otsu(values: &[f32]) -> (f32, f32) {
    let mut hist = [0f64; BINS];
    for v in values {
        hist[(v.clamp(0.0, 1.0) * (BINS - 1) as f32).round() as usize] += 1.0;
    }
    let total: f64 = hist.iter().sum();
    if total == 0.0 {
        return (0.5, 0.0);
    }
    let level = |i: usize| i as f64 / (BINS - 1) as f64;
    let mean: f64 = hist.iter().enumerate().map(|(i, n)| level(i) * n).sum::<f64>() / total;
    let variance: f64 = hist.iter().enumerate().map(|(i, n)| (level(i) - mean).powi(2) * n).sum::<f64>() / total;
    // Equal scores across empty bins form a plateau; its middle is the
    // threshold (two clean layers split half-way, not at the first one).
    let (mut best, mut plateau) = (0.0f64, (BINS / 2, BINS / 2));
    let (mut w0, mut sum0) = (0.0f64, 0.0f64);
    for (i, n) in hist.iter().enumerate().take(BINS - 1) {
        w0 += n / total;
        sum0 += level(i) * n / total;
        let w1 = 1.0 - w0;
        if w0 <= 0.0 || w1 <= 0.0 {
            continue;
        }
        let (m0, m1) = (sum0 / w0, (mean - sum0) / w1);
        let between = w0 * w1 * (m0 - m1).powi(2);
        if between > best * (1.0 + OTSU_TIE) {
            best = between;
            plateau = (i, i);
        } else if between >= best * (1.0 - OTSU_TIE) {
            plateau.1 = i;
        }
    }
    let best_t = (level(plateau.0) + level(plateau.1 + 1)) / 2.0;
    let separability = if variance > 0.0 { (best / variance) as f32 } else { 0.0 };
    (best_t as f32, separability)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Mean of `src` over a `(2r+1)²` window at every pixel (summed-area
/// table, so the cost doesn't depend on `r`).
fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut sat = vec![0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0f64;
        for x in 0..w {
            row += src[y * w + x] as f64;
            sat[(y + 1) * (w + 1) + x + 1] = sat[y * (w + 1) + x + 1] + row;
        }
    }
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let s = sat[y1 * (w + 1) + x1] - sat[y0 * (w + 1) + x1] - sat[y1 * (w + 1) + x0] + sat[y0 * (w + 1) + x0];
            out[y * w + x] = (s / ((y1 - y0) * (x1 - x0)) as f64) as f32;
        }
    }
    out
}

/// He et al.'s guided filter: `src` smoothed but with the edges of `guide`
/// (both `w × h`, values in `0..=1`).
pub fn guided_filter(guide: &[f32], src: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    let mean_i = box_mean(guide, w, h, r);
    let mean_p = box_mean(src, w, h, r);
    let ii: Vec<f32> = guide.iter().map(|v| v * v).collect();
    let ip: Vec<f32> = guide.iter().zip(src).map(|(a, b)| a * b).collect();
    let corr_i = box_mean(&ii, w, h, r);
    drop(ii);
    let corr_ip = box_mean(&ip, w, h, r);
    drop(ip);
    let mut a = vec![0f32; w * h];
    let mut b = vec![0f32; w * h];
    for k in 0..w * h {
        let var = corr_i[k] - mean_i[k] * mean_i[k];
        let cov = corr_ip[k] - mean_i[k] * mean_p[k];
        a[k] = cov / (var + eps);
        b[k] = mean_p[k] - a[k] * mean_i[k];
    }
    let (ma, mb) = (box_mean(&a, w, h, r), box_mean(&b, w, h, r));
    (0..w * h).map(|k| ma[k] * guide[k] + mb[k]).collect()
}

/// Outcome of [`subject_alpha`].
#[derive(Debug)]
pub struct Split {
    /// Where the clock sits in normalized depth (0 far … 1 near).
    pub threshold: f32,
    /// Raw depth range of the picture (1st to 99th percentile).
    pub spread: f32,
    /// Fraction of the frame in front of the clock.
    pub subject: f32,
}

impl Split {
    pub fn describe(&self) -> String {
        format!("split {:.2}, depth range {:.2}, subject {:.0}%", self.threshold, self.spread, self.subject * 100.0)
    }
}

/// The automatic split: the far background against everything else. Otsu
/// on the whole range separates near from far (a ground plane pulls it to
/// the middle); a second Otsu inside the far class finds where the
/// backdrop (sky, far haze, a plain studio wall) ends.
pub fn auto_split(norm: &[f32]) -> f32 {
    let (first, _) = otsu(norm);
    let far: Vec<f32> = norm.iter().copied().filter(|v| *v < first).collect();
    let (second, _) = otsu(&far);
    second.clamp(AUTO_MIN_SPLIT, first.max(AUTO_MIN_SPLIT))
}

/// The subject alpha (255 = in front of the clock) at the size of `photo`
/// (the wallpaper at mask resolution): everything nearer than the split
/// (`manual`, or Otsu's per wallpaper), with edges snapped to the photo.
/// `Err` says why there is no confident subject.
pub fn subject_alpha(depth: &DepthMap, photo: &RgbImage, manual: Option<f32>) -> Result<(GrayImage, Split), String> {
    let spread = if depth.data.is_empty() { 0.0 } else { quantile(&depth.data, HIGH_QUANTILE) - quantile(&depth.data, LOW_QUANTILE) };
    let norm = DepthMap { data: normalize(&depth.data), ..*depth };
    let threshold = manual.unwrap_or_else(|| auto_split(&norm.data));
    let (w, h) = (photo.width() as usize, photo.height() as usize);
    if w == 0 || h == 0 {
        return Err("empty image".into());
    }
    let (sx, sy) = (depth.width as f32 / w as f32, depth.height as f32 / h as f32);
    let mut soft = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let d = norm.sample((x as f32 + 0.5) * sx - 0.5, (y as f32 + 0.5) * sy - 0.5);
            soft.push(smoothstep(threshold - SPLIT_BAND, threshold + SPLIT_BAND, d));
        }
    }
    let subject = soft.iter().sum::<f32>() / soft.len() as f32;
    let split = Split { threshold, spread, subject };
    if spread < MIN_RAW_SPREAD {
        return Err(format!("the picture has no real depth ({})", split.describe()));
    }
    if subject < MIN_SUBJECT {
        return Err(format!("nothing stands in front of the clock ({})", split.describe()));
    }
    if subject > MAX_SUBJECT {
        return Err(format!("everything would stand in front of the clock ({})", split.describe()));
    }
    let guide: Vec<f32> = photo.pixels().map(|p| (0.2126 * p.0[0] as f32 + 0.7152 * p.0[1] as f32 + 0.0722 * p.0[2] as f32) / 255.0).collect();
    let r = ((w.max(h) as f32 * GUIDE_RADIUS_FRACTION).round() as usize).max(GUIDE_MIN_RADIUS);
    let refined = guided_filter(&guide, &soft, w, h, r, GUIDE_EPS);
    let alpha = GrayImage::from_fn(w as u32, h as u32, |x, y| Luma([(smoothstep(EDGE_LOW, EDGE_HIGH, refined[y as usize * w + x as usize]) * 255.0).round() as u8]));
    Ok((alpha, split))
}

// ── The helper process ───────────────────────────────────────────────────

/// What the helper reports on stdout (one JSON line).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct WorkerReport {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// Wall time of decode + inference + write, ms.
    pub ms: u64,
    /// CPU time the helper used, ms.
    pub cpu_ms: u64,
    /// The helper's peak resident memory, KiB.
    pub peak_rss_kib: u64,
}

/// `/proc/self/status` field `key` in KiB.
fn status_kib(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with(key)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse().ok()))
        .unwrap_or(0)
}

/// CPU time this process has used (user + system, all threads).
fn cpu_time() -> Duration {
    let t = rustix::time::clock_gettime(rustix::time::ClockId::ProcessCPUTime);
    Duration::new(t.tv_sec.max(0) as u64, t.tv_nsec.clamp(0, 999_999_999) as u32)
}

/// Lowers this process to idle priority and caps its memory and CPU time.
fn apply_guardrails() {
    use rustix::process::{Resource, Rlimit, setpriority_process, setrlimit};
    if let Err(e) = setpriority_process(None, NICE) {
        tracing::debug!("depth helper: nice: {e}");
    }
    idle_scheduling();
    for (resource, limit) in [(Resource::Data, DATA_CAP_BYTES), (Resource::Cpu, CPU_BUDGET_SECS)] {
        if let Err(e) = setrlimit(resource, Rlimit { current: Some(limit), maximum: Some(limit) }) {
            tracing::debug!("depth helper: rlimit {resource:?}: {e}");
        }
    }
}

/// Puts this process in the idle scheduling class, so it only runs on CPU
/// time nothing else wants. Failure leaves the normal policy in place.
#[cfg(target_os = "linux")]
fn idle_scheduling() {
    // musl's `sched_param` has more fields than glibc's (the sporadic
    // server's), so it's built zeroed rather than field by field.
    // SAFETY: `sched_param` is a plain C struct of integers; all zeroes is
    // a valid value of it.
    let mut param: libc::sched_param = unsafe { std::mem::zeroed() };
    param.sched_priority = 0;
    // SAFETY: plain syscall on this process with a valid, initialized
    // parameter struct; failure only leaves the normal policy in place.
    if unsafe { libc::sched_setscheduler(0, libc::SCHED_IDLE, &param) } != 0 {
        tracing::debug!("depth helper: SCHED_IDLE unavailable");
    }
}

/// FreeBSD's idle class is `rtprio(2)`'s `RTP_PRIO_IDLE`, at its lowest
/// level, which an unprivileged process may set on itself (rtprio(2)).
#[cfg(target_os = "freebsd")]
fn idle_scheduling() {
    let mut rtp = libc::rtprio { type_: libc::RTP_PRIO_IDLE, prio: libc::RTP_PRIO_MAX };
    // SAFETY: plain syscall on this process (pid 0) with a valid struct.
    if unsafe { libc::rtprio(libc::RTP_SET, 0, &mut rtp) } != 0 {
        tracing::debug!("depth helper: RTP_PRIO_IDLE unavailable");
    }
}

/// Body of `mandara depth-worker <model> <image> <out>`: estimates the
/// depth of `image` into `out` (16-bit PNG) and prints a [`WorkerReport`].
/// Returns the exit status.
pub fn worker_main(model: &Path, image_path: &Path, out: &Path) -> i32 {
    apply_guardrails();
    let started = Instant::now();
    let result = (|| -> Result<(), String> {
        let mut reader = image::ImageReader::open(image_path).map_err(|e| format!("{}: {e}", image_path.display()))?.with_guessed_format().map_err(|e| e.to_string())?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(IMAGE_ALLOC_CAP);
        reader.limits(limits);
        let img = reader.decode().map_err(|e| format!("{}: {e}", image_path.display()))?;
        let depth = estimate(model, &img, INFERENCE_BUDGET)?;
        drop(img);
        depth.save(out)
    })();
    let report = WorkerReport {
        ok: result.is_ok(),
        error: result.err().unwrap_or_default(),
        ms: started.elapsed().as_millis() as u64,
        cpu_ms: cpu_time().as_millis() as u64,
        peak_rss_kib: status_kib("VmHWM:"),
    };
    println!("{}", serde_json::to_string(&report).unwrap_or_default());
    if report.ok { 0 } else { 1 }
}

/// Runs the helper (`exe depth-worker …`) for `image`, writing `out`.
/// Blocks the calling (background) thread; kills the helper past
/// [`WALL_BUDGET`].
pub fn run_worker(exe: &Path, image_path: &Path, out: &Path) -> Result<WorkerReport, String> {
    let model = model_path();
    if !installed() {
        return Err("the depth model isn't downloaded".into());
    }
    let mut child = Command::new(exe)
        .arg("depth-worker")
        .arg(&model)
        .arg(image_path)
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("starting the depth helper: {e}"))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > WALL_BUDGET => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("the depth helper took longer than {} s and was stopped", WALL_BUDGET.as_secs()));
            }
            Ok(None) => std::thread::sleep(WAIT_STEP),
            Err(e) => return Err(format!("depth helper: {e}")),
        }
    };
    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    let report: WorkerReport = stdout.lines().rev().find_map(|l| serde_json::from_str(l).ok()).ok_or_else(|| format!("the depth helper exited without a report ({status})"))?;
    if report.ok { Ok(report) } else { Err(report.error.clone()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_is_chw_and_normalized() {
        let img = RgbImage::from_fn(2, 2, |x, _| if x < 1 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) });
        let t = preprocess(&img, 2);
        assert_eq!(t.len(), 3 * 4);
        // Left column red: R plane high, B plane low; right column the reverse.
        let red = (1.0 - MEAN[0]) / STD[0];
        assert!((t[0] - red).abs() < 1e-4, "{}", t[0]);
        assert!((t[2 * 4 + 1] - (1.0 - MEAN[2]) / STD[2]).abs() < 1e-4);
        assert!((t[2 * 4] - (0.0 - MEAN[2]) / STD[2]).abs() < 1e-4);
    }

    #[test]
    fn normalize_ignores_outliers() {
        let mut raw: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
        raw.push(1000.0);
        let n = normalize(&raw);
        assert!(n.iter().all(|v| (0.0..=1.0).contains(v)));
        // The outlier doesn't squash the rest towards 0.
        assert!(n[500] > 0.4 && n[500] < 0.6, "{}", n[500]);
        assert_eq!(normalize(&[3.0, 3.0]), vec![0.0, 0.0]);
    }

    #[test]
    fn to_aspect_keeps_the_wallpaper_shape() {
        let sq: Vec<f32> = (0..16).map(|i| (i / 4) as f32 / 3.0).collect();
        let d = to_aspect(&sq, 4, (1920, 1080));
        assert_eq!((d.width, d.height), (4, 2));
        // Top rows far, bottom rows near survive the squeeze.
        assert!(d.data[0] < d.data[(d.height as usize - 1) * d.width as usize]);
    }

    #[test]
    fn otsu_splits_two_layers() {
        let values: Vec<f32> = (0..1000).map(|i| if i < 600 { 0.1 } else { 0.8 }).collect();
        let (t, sep) = otsu(&values);
        assert!(t > 0.1 && t < 0.8, "{t}");
        assert!(sep > 0.95, "{sep}");
        // A uniform spread scores 0.75 by construction.
        let uniform: Vec<f32> = (0..1000).map(|i| i as f32 / 999.0).collect();
        assert!((otsu(&uniform).1 - 0.75).abs() < 0.01);
    }

    #[test]
    fn guided_filter_snaps_to_guide_edges() {
        // Guide: a hard step at x = 10; source: a blurry ramp around it.
        let (w, h) = (20, 6);
        let guide: Vec<f32> = (0..w * h).map(|k| if k % w < 10 { 0.0 } else { 1.0 }).collect();
        let src: Vec<f32> = (0..w * h).map(|k| ((k % w) as f32 - 5.0) / 10.0).map(|v| v.clamp(0.0, 1.0)).collect();
        let q = guided_filter(&guide, &src, w, h, 3, 1e-4);
        // Close to the step: the output follows the guide, not the ramp.
        assert!(q[2 * w + 8] < 0.35, "{}", q[2 * w + 8]);
        assert!(q[2 * w + 11] > 0.65, "{}", q[2 * w + 11]);
    }

    #[test]
    fn subject_alpha_cuts_the_near_layer() {
        // Far top half, near bottom half; the photo has the same edge.
        let depth = DepthMap { width: 8, height: 8, data: (0..64).map(|k| if k / 8 < 4 { 0.3 } else { 4.0 }).collect() };
        let photo = RgbImage::from_fn(64, 64, |_, y| if y < 32 { image::Rgb([120, 170, 230]) } else { image::Rgb([30, 60, 30]) });
        let (alpha, split) = subject_alpha(&depth, &photo, None).unwrap();
        assert_eq!((alpha.width(), alpha.height()), (64, 64));
        assert!(alpha.get_pixel(32, 4).0[0] < 10);
        assert!(alpha.get_pixel(32, 60).0[0] > 245);
        assert!((0.4..0.6).contains(&split.subject), "{}", split.subject);
        assert!(subject_alpha(&depth, &photo, Some(0.5)).is_ok());
        // A flat depth map has no foreground.
        let flat = DepthMap { width: 8, height: 8, data: (0..64).map(|k| 1.0 + (k % 3) as f32 * 0.1).collect() };
        assert!(subject_alpha(&flat, &photo, None).unwrap_err().contains("no real depth"));
    }

    #[test]
    fn auto_split_finds_the_far_backdrop() {
        // Sky (0), far ridge (0.25), a ground plane from 0.4 to 1.
        let mut v = vec![0.0f32; 400];
        v.extend(std::iter::repeat_n(0.25, 200));
        v.extend((0..400).map(|i| 0.4 + 0.6 * i as f32 / 399.0));
        let t = auto_split(&v);
        assert!(t > 0.0 && t < 0.25, "between the sky and the ridge: {t}");
    }

    #[test]
    fn depth_map_round_trips_through_png() {
        let dir = std::env::temp_dir().join(format!("mandara-depth-model-{}", std::process::id()));
        let path = dir.join("d.png");
        let d = DepthMap { width: 3, height: 2, data: vec![0.0, 0.25, 0.5, 0.75, 6.0, 0.1] };
        d.save(&path).unwrap();
        let back = DepthMap::load(&path).unwrap();
        assert_eq!((back.width, back.height), (3, 2));
        assert!(back.data.iter().zip(&d.data).all(|(a, b)| (a - b).abs() < 1e-3));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cpu_time_advances() {
        let a = cpu_time();
        let mut x = 0u64;
        for i in 0..2_000_000u64 {
            x = x.wrapping_add(i * i);
        }
        assert!(std::hint::black_box(x) > 0);
        assert!(cpu_time() > a);
    }

    #[test]
    fn worker_report_round_trips() {
        let r = WorkerReport { ok: true, error: String::new(), ms: 1200, cpu_ms: 1100, peak_rss_kib: 300_000 };
        let line = serde_json::to_string(&r).unwrap();
        assert!(!line.contains("error"));
        assert_eq!(serde_json::from_str::<WorkerReport>(&line).unwrap(), r);
    }
}
