//! Depth layer 3's masks: where they come from, the on-disk cache, the
//! cut-out drawn over the clock and widgets, coverage queries, and the
//! shared per-wallpaper job state.
//!
//! A mask comes from (in order) the user — any PNG with alpha (or a
//! grayscale matte) named after the wallpaper's file stem in
//! `~/.local/share/mandara/depth_masks/` — or the plugin's mode: the
//! hand-tuned segmentation ([`super::hand_tuned`], cached as
//! `~/.cache/mandara/depth_masks/<wallpaper-hash>.png`) or the depth model
//! (`<hash>.depth.png`, the model's depth map, and
//! `<hash>.model-<split>.png`, the cut at the chosen split). A wallpaper a
//! mode declines is remembered in `<hash>.none` / `<hash>.model.none`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::{App, AppContext as _, Context, Entity, Global};
use image::{DynamicImage, GrayImage, RgbaImage};
use mandara_config::{DepthMode, WallpaperConfig};
use mandara_services::depth_model::{self, DepthMap, WorkerReport};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use serde_json::json;

use super::MASK_MAX;

/// Steps of the manual split in a [`Request`] (and in cache names).
pub const SPLIT_STEPS: f32 = 1000.0;

/// What a mask is made with. `None` where a request is optional means the
/// depth effect is off.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Request {
    pub mode: DepthMode,
    /// Model mode's manual split in thousandths; `None` = automatic.
    pub split: Option<u16>,
}

impl Request {
    /// The request `w` makes, or `None` with the depth effect off.
    pub fn of(w: &WallpaperConfig) -> Option<Self> {
        w.depth_effect.then(|| Self {
            mode: w.depth.mode,
            split: match w.depth.mode {
                DepthMode::Model => w.depth.manual_split().map(|s| (s * SPLIT_STEPS).round() as u16),
                DepthMode::HandTuned => None,
            },
        })
    }

    fn split_fraction(self) -> Option<f32> {
        self.split.map(|s| s as f32 / SPLIT_STEPS)
    }

    /// Cache-name suffix of a model mask: `auto` or the split.
    fn model_key(self) -> String {
        self.split.map(|s| format!("s{s:04}")).unwrap_or_else(|| "auto".into())
    }
}

/// Reason given while the model is missing (Studio and `depth status`
/// show it; the shell starts the download).
pub const MODEL_MISSING: &str = "the depth model isn't downloaded yet";

// ── Paths ────────────────────────────────────────────────────────────────

/// Stable hash of a wallpaper file (path, size, mtime): FNV-1a, so cache
/// names survive toolchain upgrades.
pub fn wallpaper_hash(path: &Path) -> String {
    let meta = std::fs::metadata(path).ok();
    let mtime = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    let text = format!("{}\u{1f}{}\u{1f}{mtime}", path.display(), meta.map(|m| m.len()).unwrap_or(0));
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The cache file `<hash><suffix>` of `wallpaper`.
fn cached(wallpaper: &Path, suffix: &str) -> PathBuf {
    mandara_config::paths::depth_masks_dir().join(format!("{}{suffix}", wallpaper_hash(wallpaper)))
}

/// Generated mask for `wallpaper` under `request`.
pub fn generated_path(wallpaper: &Path, request: Request) -> PathBuf {
    match request.mode {
        DepthMode::HandTuned => cached(wallpaper, ".png"),
        DepthMode::Model => cached(wallpaper, &format!(".model-{}.png", request.model_key())),
    }
}

/// Marker recording that a mode declined this wallpaper (and why).
fn miss_path(wallpaper: &Path, request: Request) -> PathBuf {
    match request.mode {
        DepthMode::HandTuned => cached(wallpaper, ".none"),
        DepthMode::Model => cached(wallpaper, &format!(".model-{}.none", request.model_key())),
    }
}

/// The model's depth map of `wallpaper` (16-bit PNG): one model run per
/// wallpaper file, whatever the split.
pub fn depth_path(wallpaper: &Path) -> PathBuf {
    cached(wallpaper, ".depth.png")
}

/// Removes every cached file of `wallpaper` for `mode` (and, for the model,
/// its depth map).
fn forget(wallpaper: &Path, mode: DepthMode) {
    let hash = wallpaper_hash(wallpaper);
    let Ok(dir) = std::fs::read_dir(mandara_config::paths::depth_masks_dir()) else { return };
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(rest) = name.strip_prefix(&hash) else { continue };
        let model_file = rest.starts_with(".model-") || rest == ".depth.png";
        if model_file == (mode == DepthMode::Model) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Where a user-provided mask for `wallpaper` lives.
pub fn user_mask_path(wallpaper: &Path) -> Option<PathBuf> {
    let stem = wallpaper.file_stem()?;
    Some(mandara_config::paths::data_dir().join("depth_masks").join(stem).with_extension("png"))
}

// ── Masks on disk ────────────────────────────────────────────────────────

/// Alpha of a mask image: its alpha channel when it has real transparency,
/// else its luminance (a grayscale matte).
pub fn load_alpha(path: &Path) -> Result<GrayImage, String> {
    let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        if rgba.pixels().any(|p| p.0[3] < 250) {
            return Ok(GrayImage::from_fn(rgba.width(), rgba.height(), |x, y| image::Luma([rgba.get_pixel(x, y).0[3]])));
        }
    }
    Ok(img.to_luma8())
}

fn write_mask(alpha: &GrayImage, out: &Path) -> Result<(), String> {
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let rgba = RgbaImage::from_fn(alpha.width(), alpha.height(), |x, y| image::Rgba([255, 255, 255, alpha.get_pixel(x, y).0[0]]));
    let tmp = out.with_extension("tmp");
    rgba.save_with_format(&tmp, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, out).map_err(|e| e.to_string())
}

/// A resolved mask and where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub path: PathBuf,
    /// `custom` or `generated`.
    pub source: &'static str,
    pub detail: String,
}

/// The last model run in this shell (for `depth status`).
static LAST_RUN: Mutex<Option<(PathBuf, WorkerReport)>> = Mutex::new(None);

/// The last model run: wallpaper and the helper's measurements.
pub fn last_run() -> Option<(PathBuf, WorkerReport)> {
    LAST_RUN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Finds or generates the mask for `wallpaper` (blocking; run off the UI
/// thread). `Err` explains why there is none. `force` regenerates.
pub fn resolve(wallpaper: &Path, request: Option<Request>, force: bool) -> Result<Resolved, String> {
    let Some(request) = request else { return Err("the depth effect is off".into()) };
    if mandara_media::is_video(wallpaper) {
        return Err("video wallpapers have no depth subject (it moves)".into());
    }
    if let Some(user) = user_mask_path(wallpaper).filter(|p| p.is_file()) {
        return Ok(Resolved { path: user, source: "custom", detail: String::new() });
    }
    let (out, miss) = (generated_path(wallpaper, request), miss_path(wallpaper, request));
    if force {
        forget(wallpaper, request.mode);
    }
    if out.is_file() {
        return Ok(Resolved { path: out, source: source_name(request.mode), detail: "cached".into() });
    }
    if let Ok(reason) = std::fs::read_to_string(&miss) {
        return Err(format!("skipped: {}", reason.trim()));
    }
    let made = match request.mode {
        DepthMode::HandTuned => hand_tuned(wallpaper)?,
        DepthMode::Model => model(wallpaper, request)?,
    };
    match made {
        Ok((alpha, detail)) => {
            write_mask(&alpha, &out)?;
            Ok(Resolved { path: out, source: source_name(request.mode), detail })
        }
        Err(reason) => {
            if let Some(dir) = miss.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&miss, &reason);
            Err(format!("skipped: {reason}"))
        }
    }
}

/// `source` of a generated mask.
fn source_name(mode: DepthMode) -> &'static str {
    match mode {
        DepthMode::HandTuned => "hand-tuned",
        DepthMode::Model => "model",
    }
}

/// A mode's verdict: the alpha and its stats, or why it declined.
type Made = Result<(GrayImage, String), String>;

fn hand_tuned(wallpaper: &Path) -> Result<Made, String> {
    let img = mandara_theme::material::open_shared(wallpaper).map_err(|e| format!("{}: {e}", wallpaper.display()))?;
    let seg = super::hand_tuned::segment(&img);
    Ok(seg.alpha.map(|a| (a, seg.reason.clone())).ok_or(seg.reason))
}

/// Model mode: the depth map (from cache, else one helper run), then the
/// cut at the requested split. An outer `Err` is a failure to retry later
/// (no model, helper stopped); an inner one a verdict on this wallpaper.
fn model(wallpaper: &Path, request: Request) -> Result<Made, String> {
    let depth_file = depth_path(wallpaper);
    let mut detail = String::new();
    if !depth_file.is_file() {
        if !depth_model::installed() {
            return Err(MODEL_MISSING.into());
        }
        let report = depth_model::run_worker(Path::new(SELF_EXE), wallpaper, &depth_file)?;
        tracing::info!("depth model for {}: {} ms, {} ms CPU, peak {} KiB", wallpaper.display(), report.ms, report.cpu_ms, report.peak_rss_kib);
        detail = format!("model run {} ms, peak {} MiB; ", report.ms, report.peak_rss_kib / KIB_PER_MIB);
        *LAST_RUN.lock().unwrap_or_else(|e| e.into_inner()) = Some((wallpaper.to_path_buf(), report));
    }
    let depth = DepthMap::load(&depth_file)?;
    let img = mandara_theme::material::open_shared(wallpaper).map_err(|e| format!("{}: {e}", wallpaper.display()))?;
    let photo = mask_sized(&img).to_rgb8();
    drop(img);
    Ok(depth_model::subject_alpha(&depth, &photo, request.split_fraction()).map(|(alpha, split)| (alpha, format!("{detail}{}", split.describe()))))
}

/// The running shell's own binary image: the helper is always the same
/// build, even after an update replaced (or a rebuild deleted) the file the
/// shell was started from.
const SELF_EXE: &str = "/proc/self/exe";

/// KiB in a MiB (report formatting).
const KIB_PER_MIB: u64 = 1024;

/// `img` at mask resolution (longer side at most [`MASK_MAX`]).
pub(crate) fn mask_sized(img: &DynamicImage) -> DynamicImage {
    let scale = (MASK_MAX as f32 / img.width().max(img.height()) as f32).min(1.0);
    if scale >= 1.0 {
        return img.clone();
    }
    let (w, h) = (((img.width() as f32 * scale).round() as u32).max(1), ((img.height() as f32 * scale).round() as u32).max(1));
    crate::wallpaper::shrink(img, w, h)
}

/// The mask aligned to a displayed image of `size`: the display copy is
/// either the original (stretch to fit) or its centre crop (`cropped`).
pub fn align(alpha: &GrayImage, size: (u32, u32), cropped: bool) -> GrayImage {
    let (w, h) = size;
    if !cropped || (alpha.width(), alpha.height()) == (w, h) {
        return image::imageops::resize(alpha, w, h, image::imageops::FilterType::Triangle);
    }
    let dynimg = DynamicImage::ImageLuma8(alpha.clone());
    crate::wallpaper::cover(&dynimg, w, h).to_luma8()
}

/// Layer 3 pixels: the displayed wallpaper with the subject alpha, as
/// straight BGRA (what GPUI's `RenderImage` expects), plus a small copy of
/// the aligned alpha for [`coverage`] queries.
pub fn cutout(display: &Path, source: &Path, mask: &Path) -> Result<CutoutImage, String> {
    let shown = image::open(display).map_err(|e| format!("{}: {e}", display.display()))?.to_rgb8();
    let alpha = align(&load_alpha(mask)?, (shown.width(), shown.height()), display != source);
    let small = image::imageops::thumbnail(&alpha, COVERAGE_MAP_W, (COVERAGE_MAP_W * alpha.height() / alpha.width().max(1)).max(1));
    // Only the subject's pixels differ from the wallpaper drawn beneath, so
    // the layer-3 texture is cropped to the mask's opaque bounding box:
    // outside it the cut-out is fully transparent. A full-output BGRA cut-out
    // was ~33 MB in RAM and again in the GPU atlas.
    let crop = alpha_bounds(&alpha).unwrap_or((0, 0, 1, 1));
    let (cx0, cy0, cw, ch) = crop;
    let bgra = RgbaImage::from_fn(cw, ch, |x, y| {
        let p = shown.get_pixel(cx0 + x, cy0 + y).0;
        image::Rgba([p[2], p[1], p[0], alpha.get_pixel(cx0 + x, cy0 + y).0[0]])
    });
    Ok(CutoutImage { bgra, map: small, full: (shown.width(), shown.height()), crop })
}

/// Width of the coarse subject-coverage map, in px.
pub(crate) const COVERAGE_MAP_W: u32 = 256;

/// A layer-3 cut-out cropped to its subject.
pub struct CutoutImage {
    /// Premultiplied-free BGRA pixels of the crop.
    pub bgra: RgbaImage,
    /// Small aligned alpha of the whole frame (for coverage queries).
    pub map: GrayImage,
    /// Size of the full displayed image the crop was taken from.
    pub full: (u32, u32),
    /// The crop `(x, y, w, h)` in full-image pixels.
    pub crop: (u32, u32, u32, u32),
}

/// Bounding box `(x, y, w, h)` of the non-zero pixels, or `None` if empty.
pub(crate) fn alpha_bounds(alpha: &GrayImage) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in alpha.enumerate_pixels() {
        if p.0[0] > 0 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    (x0 <= x1 && y0 <= y1).then(|| (x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

/// Where a crop of an image drawn with `object-fit: cover` into a
/// `viewport` lands, as `(x, y, w, h)` in viewport units.
pub fn cover_rect(full: (u32, u32), crop: (u32, u32, u32, u32), viewport: (f32, f32)) -> (f32, f32, f32, f32) {
    let (iw, ih) = (full.0.max(1) as f32, full.1.max(1) as f32);
    let scale = (viewport.0 / iw).max(viewport.1 / ih);
    let (ox, oy) = ((viewport.0 - iw * scale) / 2.0, (viewport.1 - ih * scale) / 2.0);
    (ox + crop.0 as f32 * scale, oy + crop.1 as f32 * scale, crop.2 as f32 * scale, crop.3 as f32 * scale)
}

/// Mean subject alpha (`0..=1`) over a rectangle given in fractions of the
/// frame `(x0, y0, x1, y1)`.
pub fn coverage(map: &GrayImage, rect: (f32, f32, f32, f32)) -> f32 {
    let (w, h) = (map.width() as f32, map.height() as f32);
    let x0 = (rect.0.clamp(0.0, 1.0) * w) as u32;
    let x1 = ((rect.2.clamp(0.0, 1.0) * w).ceil() as u32).clamp(x0 + 1, map.width());
    let y0 = (rect.1.clamp(0.0, 1.0) * h) as u32;
    let y1 = ((rect.3.clamp(0.0, 1.0) * h).ceil() as u32).clamp(y0 + 1, map.height());
    if x0 >= map.width() || y0 >= map.height() {
        return 0.0;
    }
    let (mut sum, mut n) = (0u64, 0u64);
    for y in y0..y1 {
        for x in x0..x1 {
            sum += map.get_pixel(x, y).0[0] as u64;
            n += 1;
        }
    }
    sum as f32 / (n.max(1) * 255) as f32
}

/// How much of the cut-out to draw: a subject may overlap the clock, but
/// never swallow it (fades out between 45 % and 70 % coverage).
pub fn overlap_alpha(coverage: f32) -> f32 {
    1.0 - super::hand_tuned::smoothstep(0.45, 0.7, coverage)
}

// ── Shared state (one job per wallpaper, all outputs) ────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum MaskStatus {
    Pending,
    Ready(Resolved),
    Unavailable(String),
}

type Reply = async_channel::Sender<Result<String, String>>;
type Key = (PathBuf, Request);

/// The model download, at most one at a time.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Download {
    #[default]
    Idle,
    Running,
    /// The last attempt failed (offline, network): not retried on its own.
    Failed(String),
}

#[derive(Default)]
pub struct DepthMasks {
    states: HashMap<Key, MaskStatus>,
    waiters: HashMap<Key, Vec<Reply>>,
    /// Bumped on regeneration so views rebuild their cut-outs.
    pub generation: u64,
    pub download: Download,
    /// Bytes of the model received so far while [`Download::Running`]
    /// (written by the download thread, read by `depth status`).
    pub downloaded: Arc<AtomicU64>,
    download_waiters: Vec<Reply>,
}

#[derive(Clone)]
struct GlobalDepthMasks(Entity<DepthMasks>);
impl Global for GlobalDepthMasks {}

pub fn masks(cx: &mut App) -> Entity<DepthMasks> {
    if let Some(g) = cx.try_global::<GlobalDepthMasks>() {
        return g.0.clone();
    }
    let entity = cx.new(|_| DepthMasks::default());
    cx.set_global(GlobalDepthMasks(entity.clone()));
    entity
}

fn describe(status: &MaskStatus) -> Result<String, String> {
    match status {
        MaskStatus::Ready(r) => Ok(json!({ "mask": r.path, "source": r.source, "detail": r.detail }).to_string()),
        MaskStatus::Unavailable(why) => Err(why.clone()),
        MaskStatus::Pending => Err("still generating".into()),
    }
}

/// Status line while the model downloads.
const DOWNLOADING: &str = "downloading the depth model";

impl DepthMasks {
    /// The mask status for `wallpaper`, starting a background job when it
    /// isn't known yet (and, in model mode without the model, its download).
    pub fn status(&mut self, wallpaper: &Path, request: Option<Request>, cx: &mut Context<Self>) -> MaskStatus {
        let Some(request) = request else { return MaskStatus::Unavailable("off".into()) };
        let key = (wallpaper.to_path_buf(), request);
        if let Some(s) = self.states.get(&key) {
            return s.clone();
        }
        if let Some(waiting) = self.awaiting_model(wallpaper, request, cx) {
            return waiting;
        }
        self.start(key, false, None, cx);
        MaskStatus::Pending
    }

    /// In model mode without the model (and no mask of your own): starts
    /// the download once and says so.
    fn awaiting_model(&mut self, wallpaper: &Path, request: Request, cx: &mut Context<Self>) -> Option<MaskStatus> {
        if request.mode != DepthMode::Model || depth_model::installed() || user_mask_path(wallpaper).is_some_and(|p| p.is_file()) || depth_path(wallpaper).is_file() {
            return None;
        }
        match &self.download {
            Download::Idle => {
                self.download(None, cx);
                Some(MaskStatus::Unavailable(DOWNLOADING.into()))
            }
            Download::Running => Some(MaskStatus::Unavailable(DOWNLOADING.into())),
            Download::Failed(why) => Some(MaskStatus::Unavailable(format!("{MODEL_MISSING}: {why}"))),
        }
    }

    /// Peeks without starting anything.
    pub fn known(&self, wallpaper: &Path, request: Option<Request>) -> Option<MaskStatus> {
        self.states.get(&(wallpaper.to_path_buf(), request?)).cloned()
    }

    /// Discards the cached mask for `wallpaper` and builds it again; `reply`
    /// receives the outcome.
    pub fn regenerate(&mut self, wallpaper: &Path, request: Request, reply: Option<Reply>, cx: &mut Context<Self>) {
        self.states.retain(|(p, _), _| p != wallpaper);
        self.generation += 1;
        if let Some(waiting) = self.awaiting_model(wallpaper, request, cx) {
            if let Some(r) = reply {
                let _ = r.try_send(describe(&waiting));
            }
            cx.notify();
            return;
        }
        self.start((wallpaper.to_path_buf(), request), true, reply, cx);
        cx.notify();
    }

    /// Downloads the depth model (checksum-verified) in the background;
    /// model-mode masks are looked up again when it lands.
    pub fn download(&mut self, reply: Option<Reply>, cx: &mut Context<Self>) {
        self.download_waiters.extend(reply);
        if self.download == Download::Running {
            return;
        }
        self.download = Download::Running;
        self.downloaded.store(0, Ordering::Relaxed);
        let downloaded = self.downloaded.clone();
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            let model = mandara_services::models::find(depth_model::MODEL_ID).ok_or("unknown model")?;
            model.download(&|done, _| downloaded.store(done, Ordering::Relaxed))?;
            Ok::<_, String>(model.dir())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.download = match &result {
                    Ok(_) => Download::Idle,
                    Err(e) => {
                        tracing::warn!("depth model download: {e}");
                        Download::Failed(e.clone())
                    }
                };
                // Model-mode answers given while it was missing are stale.
                this.states.retain(|(_, r), _| r.mode != DepthMode::Model);
                this.generation += 1;
                let reply = result.map(|dir| json!({ "model": depth_model::MODEL_ID, "installed": true, "path": dir }).to_string());
                for w in this.download_waiters.drain(..) {
                    let _ = w.try_send(reply.clone());
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start(&mut self, key: Key, force: bool, reply: Option<Reply>, cx: &mut Context<Self>) {
        if let Some(r) = reply {
            self.waiters.entry(key.clone()).or_default().push(r);
        }
        if !force && self.states.get(&key) == Some(&MaskStatus::Pending) {
            return;
        }
        self.states.insert(key.clone(), MaskStatus::Pending);
        let (path, request) = key.clone();
        let task = cx.background_executor().spawn(async move { resolve(&path, Some(request), force) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let status = match result {
                    Ok(r) => MaskStatus::Ready(r),
                    Err(e) => {
                        tracing::info!("depth mask for {}: {e}", key.0.display());
                        MaskStatus::Unavailable(e)
                    }
                };
                for w in this.waiters.remove(&key).unwrap_or_default() {
                    let _ = w.try_send(describe(&status));
                }
                this.states.insert(key, status);
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutout_crop_bounds_and_cover_mapping() {
        let mut a = GrayImage::new(10, 8);
        a.put_pixel(2, 3, image::Luma([200]));
        a.put_pixel(6, 5, image::Luma([1]));
        assert_eq!(alpha_bounds(&a), Some((2, 3, 5, 3)));
        assert_eq!(alpha_bounds(&GrayImage::new(4, 4)), None);
        // Same aspect: cover is a plain scale.
        assert_eq!(cover_rect((100, 50), (10, 5, 20, 10), (200.0, 100.0)), (20.0, 10.0, 40.0, 20.0));
        // Wider image than viewport: centred horizontally.
        let r = cover_rect((200, 100), (0, 0, 200, 100), (100.0, 100.0));
        assert_eq!(r, (-50.0, 0.0, 200.0, 100.0));
    }





    #[test]
    fn mask_alpha_from_alpha_or_luminance() {
        let dir = std::env::temp_dir().join(format!("mandara-depth-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let with_alpha = dir.join("a.png");
        RgbaImage::from_fn(4, 4, |x, _| image::Rgba([10, 20, 30, if x < 2 { 0 } else { 255 }])).save(&with_alpha).unwrap();
        let a = load_alpha(&with_alpha).unwrap();
        assert_eq!((a.get_pixel(0, 0).0[0], a.get_pixel(3, 0).0[0]), (0, 255));
        let matte = dir.join("m.png");
        GrayImage::from_fn(4, 4, |x, _| image::Luma([if x < 2 { 0 } else { 255 }])).save(&matte).unwrap();
        let m = load_alpha(&matte).unwrap();
        assert_eq!((m.get_pixel(0, 0).0[0], m.get_pixel(3, 0).0[0]), (0, 255));
        // Aligned to a centre-cropped display copy.
        let aligned = align(&m, (2, 4), true);
        assert_eq!((aligned.width(), aligned.height()), (2, 4));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn coverage_and_overlap_limit() {
        let map = GrayImage::from_fn(100, 100, |_, y| image::Luma([if y >= 50 { 255 } else { 0 }]));
        assert!(coverage(&map, (0.0, 0.0, 1.0, 0.4)) < 0.01);
        assert!((coverage(&map, (0.0, 0.0, 1.0, 1.0)) - 0.5).abs() < 0.02);
        assert!(coverage(&map, (0.2, 0.6, 0.8, 0.9)) > 0.99);
        assert_eq!(overlap_alpha(0.2), 1.0);
        assert_eq!(overlap_alpha(0.9), 0.0);
    }

    #[test]
    fn hash_is_stable_and_file_specific() {
        let a = wallpaper_hash(Path::new("/nonexistent/a.jpg"));
        assert_eq!(a, wallpaper_hash(Path::new("/nonexistent/a.jpg")));
        assert_ne!(a, wallpaper_hash(Path::new("/nonexistent/b.jpg")));
        assert_eq!(a.len(), 16);
        assert!(user_mask_path(Path::new("/w/dunes.jpg")).unwrap().ends_with("depth_masks/dunes.png"));
    }

    #[test]
    fn cache_names_follow_mode_and_split() {
        let w = Path::new("/nonexistent/dunes.jpg");
        let hash = wallpaper_hash(w);
        let hand = Request { mode: DepthMode::HandTuned, split: None };
        let auto = Request { mode: DepthMode::Model, split: None };
        let manual = Request { mode: DepthMode::Model, split: Some(420) };
        let name = |p: PathBuf| p.file_name().unwrap().to_string_lossy().to_string();
        // Hand-tuned keeps the cache names masks had before the plugin.
        assert_eq!(name(generated_path(w, hand)), format!("{hash}.png"));
        assert_eq!(name(miss_path(w, hand)), format!("{hash}.none"));
        assert_eq!(name(generated_path(w, auto)), format!("{hash}.model-auto.png"));
        assert_eq!(name(generated_path(w, manual)), format!("{hash}.model-s0420.png"));
        assert_eq!(name(miss_path(w, manual)), format!("{hash}.model-s0420.none"));
        assert_eq!(name(depth_path(w)), format!("{hash}.depth.png"));
    }

    #[test]
    fn requests_follow_the_config() {
        let mut w = WallpaperConfig::default();
        assert_eq!(Request::of(&w), Some(Request { mode: DepthMode::HandTuned, split: None }));
        w.depth.mode = DepthMode::Model;
        assert_eq!(Request::of(&w), Some(Request { mode: DepthMode::Model, split: None }));
        w.depth.auto_split = false;
        w.depth.split = 0.42;
        assert_eq!(Request::of(&w), Some(Request { mode: DepthMode::Model, split: Some(420) }));
        // The split is the model's alone, and is clamped to its range.
        w.depth.split = 5.0;
        assert_eq!(Request::of(&w).and_then(|r| r.split), Some((mandara_config::defaults::DEPTH_SPLIT_RANGE.1 * SPLIT_STEPS).round() as u16));
        w.depth.mode = DepthMode::HandTuned;
        assert_eq!(Request::of(&w).and_then(|r| r.split), None);
        w.depth_effect = false;
        assert_eq!(Request::of(&w), None);
    }

}
