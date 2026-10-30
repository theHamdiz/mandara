//! Wallpaper: Mandara draws the desktop background itself (one
//! `mandara-wallpaper` background layer per output, crossfading between
//! images) and offers a minimal picker — wallpapers floating centred on a
//! light veil, the focused one large in the middle — with thumbnails cached under
//! `~/.cache/mandara/thumbnails/` by bounded jobs on GPUI's background
//! executor.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext as _, Task, BoxShadow, Context, Entity, FocusHandle, FontWeight, Global, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ObjectFit, ParentElement, Render, ScrollEvent, StatefulInteractiveElement, Styled, StyledImage as _, Window, div, img, point,
    prelude::FluentBuilder as _, px,
};
use mandara_config::ConfigStore;
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui};
use mandara_i18n::{t, tf};
use mandara_services::wallhaven::{self, Wall};
use mandara_theme::{SchemeVariant, ThemeSource};

use crate::dismiss::KeepInside as _;
use crate::fonts::fonts;
use crate::icons::icon;
use crate::kit::text;
use crate::shell::frame_color;
use crate::style::ClipRounded as _;
use crate::state::{ShellState, config, shell, update_config};

/// An online wallpaper is prefetched once the pointer rests on it this long (network politeness).
const PREFETCH_DWELL: Duration = Duration::from_millis(450);
/// Online search waits this long after the last keystroke (API rate politeness).
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(600);

/// Image files the library lists (video clips: [`mandara_media::VIDEO_EXTENSIONS`]).
const EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];
const THUMB_W: u32 = 720;
const THUMB_H: u32 = 450;
/// Re-encode quality of picker thumbnails.
const THUMB_QUALITY: u8 = 82;
/// Re-encode quality of display copies (wallpapers larger than the output).
const DISPLAY_QUALITY: u8 = 92;
/// Re-encode quality of a video wallpaper's poster (its first frame, shown
/// while it crossfades in and wherever a still is needed): high enough
/// that the hand-over to the live frames is invisible.
const POSTER_QUALITY: u8 = 95;

// ── Library ──────────────────────────────────────────────────────────────

/// Whether `path` is a wallpaper the library lists: an image, or an AV1
/// video clip (`.webm`, `.mkv`, `.ivf`).
pub fn is_wallpaper(path: &Path) -> bool {
    path.extension().and_then(|x| x.to_str()).is_some_and(|x| EXTENSIONS.contains(&x.to_ascii_lowercase().as_str())) || mandara_media::is_video(path)
}

/// Wallpapers (images and video clips) directly inside `dir` (and one
/// level of sub-folders), sorted by name.
pub fn list(dir: &Path) -> Vec<PathBuf> {
    fn scan(dir: &Path, depth: u8, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth > 0 && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
                    scan(&p, depth - 1, out);
                }
            } else if is_wallpaper(&p) {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    scan(dir, 1, &mut out);
    out.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    out
}

pub fn directory(cx: &App) -> PathBuf {
    mandara_config::paths::expand_tilde(&config(cx).read(cx).get().wallpaper.directory)
}

/// A cache key that changes when the file does.
pub(crate) fn cache_key(path: &Path, tag: &str) -> String {
    let meta = std::fs::metadata(path).ok();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    tag.hash(&mut h);
    meta.as_ref().map(|m| m.len()).hash(&mut h);
    meta.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).hash(&mut h);
    format!("{:016x}.jpg", h.finish())
}

/// Center-crops `img` to `w:h` and resizes it to exactly `w × h`.
pub(crate) fn cover(img: &image::DynamicImage, w: u32, h: u32) -> image::DynamicImage {
    let (iw, ih) = (img.width().max(1), img.height().max(1));
    let target = w as f32 / h as f32;
    let (cw, ch) = if iw as f32 / ih as f32 > target { ((ih as f32 * target) as u32, ih) } else { (iw, (iw as f32 / target) as u32) };
    img.crop_imm((iw - cw) / 2, (ih - ch) / 2, cw.max(1), ch.max(1)).resize_exact(w, h, image::imageops::FilterType::Triangle)
}

pub(crate) fn save_jpeg(img: &image::DynamicImage, out: &Path, quality: u8) -> Result<(), String> {
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = out.with_extension("tmp");
    let file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(file), quality).encode_image(&img.to_rgb8()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, out).map_err(|e| e.to_string())
}

/// A picker thumbnail (cached); a video's comes from its first frame.
pub fn thumbnail(path: &Path) -> Result<PathBuf, String> {
    thumbnail_in(path, &mandara_config::paths::cache_dir())
}

/// [`thumbnail`] with the cache under `cache`.
pub fn thumbnail_in(path: &Path, cache: &Path) -> Result<PathBuf, String> {
    let out = cache.join("thumbnails").join(cache_key(path, &format!("thumb{THUMB_W}")));
    if out.exists() {
        return Ok(out);
    }
    let img = open_still(path)?;
    save_jpeg(&cover(&img, THUMB_W, THUMB_H), &out, THUMB_QUALITY)?;
    Ok(out)
}

/// Decodes a wallpaper as a picture: the image itself, or a video's first
/// frame.
pub fn open_still(path: &Path) -> Result<image::DynamicImage, String> {
    if mandara_media::is_video(path) {
        return mandara_media::first_frame(path).map(image::DynamicImage::ImageRgba8);
    }
    image::open(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The file to read when a wallpaper is needed as a picture (fill copies,
/// the lock screen, previews): the image itself, or for a video its
/// poster, the first frame cached under `~/.cache/mandara/posters/`.
pub fn still(path: &Path) -> Result<PathBuf, String> {
    still_in(path, &mandara_config::paths::cache_dir())
}

/// [`still`] with the cache under `cache`.
pub fn still_in(path: &Path, cache: &Path) -> Result<PathBuf, String> {
    if !mandara_media::is_video(path) {
        return Ok(path.to_path_buf());
    }
    let out = cache.join("posters").join(cache_key(path, "poster"));
    if !out.exists() {
        save_jpeg(&open_still(path)?, &out, POSTER_QUALITY)?;
    }
    Ok(out)
}

/// Horizontal lean of a slice: the top edge sits this fraction of the
/// height to the right of the bottom edge.
pub const SHEAR: f32 = 0.2;

/// A wallpaper rendered as a slanted slice (`w × h` box plus the lean):
/// anti-aliased parallelogram with transparent corners, cached as PNG.
pub fn slant(path: &Path, w: u32, h: u32) -> Result<PathBuf, String> {
    use tiny_skia::{FillRule, FilterQuality, IntSize, Paint, PathBuilder, Pattern, Pixmap, SpreadMode, Transform};
    let dir = mandara_config::paths::cache_dir().join("slices");
    let out = dir.join(cache_key(path, &format!("slant{w}x{h}")).replace(".jpg", ".png"));
    if out.exists() {
        return Ok(out);
    }
    let src = if w <= THUMB_W && h <= THUMB_H { image::open(thumbnail(path)?).map_err(|e| format!("{}: {e}", path.display()))? } else { open_still(path)? };
    let cov = cover(&src, w, h).to_rgba8();
    let off = (h as f32 * SHEAR).round() as u32;
    let size = IntSize::from_wh(w, h).ok_or("bad size")?;
    // Opaque photos: straight RGBA equals premultiplied.
    let source = Pixmap::from_vec(cov.into_raw(), size).ok_or("bad source")?;
    let mut canvas = Pixmap::new(w + off, h).ok_or("bad canvas")?;
    let mut pb = PathBuilder::new();
    pb.move_to(off as f32, 0.0);
    pb.line_to((w + off) as f32, 0.0);
    pb.line_to(w as f32, h as f32);
    pb.line_to(0.0, h as f32);
    pb.close();
    let shape = pb.finish().ok_or("bad path")?;
    let mut paint = Paint::default();
    paint.anti_alias = true;
    // x' = x + off − SHEAR·y: the photo leans with its frame.
    paint.shader = Pattern::new(source.as_ref(), SpreadMode::Pad, FilterQuality::Bilinear, 1.0, Transform::from_row(1.0, 0.0, -SHEAR, 1.0, off as f32, 0.0));
    canvas.fill_path(&shape, &paint, FillRule::Winding, Transform::identity(), None);
    // Back to straight alpha for PNG.
    let mut rgba = canvas.take();
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    let img = image::RgbaImage::from_raw(w + off, h, rgba).ok_or("bad output")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tmp = out.with_extension("tmp");
    img.save_with_format(&tmp, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
    prune(&dir, 400 * 1024 * 1024);
    Ok(out)
}

/// Keeps a cache directory under `limit` bytes, oldest files first.
fn prune(dir: &Path, limit: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= limit {
        return;
    }
    files.sort();
    for (_, len, path) in files {
        if total <= limit * 9 / 10 {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

/// A copy no larger than the output (physical pixels), so a 6000 px photo
/// isn't kept on the GPU at full size. Small images are used as they are.
pub fn display_copy(path: &Path, w: u32, h: u32) -> Result<PathBuf, String> {
    let out = mandara_config::paths::cache_dir().join("wallpapers").join(cache_key(path, &format!("{w}x{h}")));
    if out.exists() {
        return Ok(out);
    }
    let source = still(path)?;
    let (iw, ih) = image::image_dimensions(&source).map_err(|e| format!("{}: {e}", path.display()))?;
    if iw <= w + w / 4 && ih <= h + h / 4 {
        return Ok(source);
    }
    let img = image::open(&source).map_err(|e| format!("{}: {e}", path.display()))?;
    save_jpeg(&cover(&img, w, h), &out, DISPLAY_QUALITY)?;
    Ok(out)
}

// ── Thumbnail cache (background executor) ───────────────────────────────

/// A thumbnail job: `(image, width, height)`; `0 × 0` is the plain
/// picker thumbnail, any other size a slanted slice of that size.
pub type Job = (PathBuf, u32, u32);

/// Most thumbnail decodes in flight at once. Jobs run on GPUI's shared
/// background executor (no dedicated threads); the cap bounds the transient
/// memory of full-size decodes (a 4K JPEG decodes to ~33 MB).
const MAX_THUMB_JOBS: usize = 3;

#[derive(Default)]
pub struct Thumbnails {
    ready: HashMap<Job, PathBuf>,
    failed: HashSet<Job>,
    requested: HashSet<Job>,
    /// Waiting jobs; the most important is last (popped first).
    queue: Vec<Job>,
    in_flight: usize,
}

impl Thumbnails {
    pub fn get(&self, path: &Path) -> Option<&PathBuf> {
        self.ready.get(&(path.to_path_buf(), 0, 0))
    }

    pub fn failed(&self, path: &Path) -> bool {
        self.failed.contains(&(path.to_path_buf(), 0, 0))
    }

    pub fn get_job(&self, job: &Job) -> Option<&PathBuf> {
        self.ready.get(job)
    }

    /// Whether `job` failed (an unreadable or corrupt file).
    pub fn failed_job(&self, job: &Job) -> bool {
        self.failed.contains(job)
    }

    /// Queues plain thumbnails (most important last).
    pub fn request(&mut self, paths: impl IntoIterator<Item = PathBuf>, cx: &mut Context<Self>) {
        self.request_jobs(paths.into_iter().map(|p| (p, 0, 0)), cx);
    }

    /// Queues jobs (most important last: jobs are taken from the end).
    pub fn request_jobs(&mut self, jobs: impl IntoIterator<Item = Job>, cx: &mut Context<Self>) {
        for job in jobs {
            if self.ready.contains_key(&job) || self.failed.contains(&job) {
                continue;
            }
            if self.requested.insert(job.clone()) {
                self.queue.push(job);
            } else if let Some(i) = self.queue.iter().position(|x| *x == job) {
                // Re-prioritise something already queued.
                let job = self.queue.remove(i);
                self.queue.push(job);
            }
        }
        self.pump(cx);
    }

    /// Starts queued jobs up to [`MAX_THUMB_JOBS`].
    fn pump(&mut self, cx: &mut Context<Self>) {
        while self.in_flight < MAX_THUMB_JOBS {
            let Some(job) = self.queue.pop() else { break };
            self.in_flight += 1;
            let work = job.clone();
            let task = cx.background_executor().spawn(async move {
                if work.1 == 0 { thumbnail(&work.0) } else { slant(&work.0, work.1, work.2) }
            });
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |t, cx| {
                    t.in_flight -= 1;
                    t.requested.remove(&job);
                    match result {
                        Ok(thumb) => {
                            t.ready.insert(job, thumb);
                        }
                        Err(err) => {
                            tracing::debug!("thumbnail: {err}");
                            t.failed.insert(job);
                        }
                    }
                    t.pump(cx);
                    cx.notify();
                });
            })
            .detach();
        }
        if self.queue.is_empty() {
            self.queue.shrink_to_fit();
        }
    }
}

#[derive(Clone)]
struct GlobalThumbnails(Entity<Thumbnails>);
impl Global for GlobalThumbnails {}

pub fn thumbnails(cx: &mut App) -> Entity<Thumbnails> {
    if let Some(g) = cx.try_global::<GlobalThumbnails>() {
        return g.0.clone();
    }
    let entity = cx.new(|_| Thumbnails::default());
    cx.set_global(GlobalThumbnails(entity.clone()));
    entity
}

/// Sets the wallpaper (the theme follows it when "match colors" is on —
/// see [`reconcile_theme`]).
pub fn apply(path: PathBuf, cx: &mut App) {
    update_config(cx, |c| c.wallpaper.path = Some(path));
}

/// Keeps the theme source consistent with `wallpaper.match_colors`:
/// on → Material You colors from the current wallpaper (remembering the
/// catalog theme it replaced); off → back to that theme. Returns `true` if
/// it changed anything.
pub fn reconcile_theme(c: &mut mandara_config::ShellConfig) -> bool {
    // The colours follow the image on screen: the light / dark variant
    // when one is set, else the main wallpaper.
    let shown = mandara_config::desktop::effective_wallpaper(c.wallpaper.path.as_ref(), &c.wallpaper.variants, None, c.theme.mode.is_dark());
    match (&c.theme.source, c.wallpaper.match_colors, shown) {
        // A new wallpaper keeps the scheme style; the seed choice resets.
        (ThemeSource::Wallpaper { path, variant, .. }, true, Some(wp)) if *path != wp => {
            c.theme.source = ThemeSource::Wallpaper { path: wp, variant: *variant, seed: 0 };
            true
        }
        (ThemeSource::Wallpaper { .. }, true, _) => false,
        (source, true, Some(wp)) => {
            if let ThemeSource::Preset { preset } = source {
                c.theme.last_preset = Some(preset.clone());
            }
            c.theme.source = ThemeSource::Wallpaper { path: wp, variant: SchemeVariant::default(), seed: 0 };
            true
        }
        (ThemeSource::Wallpaper { .. }, false, _) => {
            let preset = c.theme.last_preset.clone().unwrap_or_else(|| mandara_theme::DEFAULT_THEME.to_string());
            c.theme.source = ThemeSource::Preset { preset };
            true
        }
        _ => false,
    }
}

/// Watches the config and applies [`reconcile_theme`] (shell process).
pub fn init_theme_sync(cx: &mut App) {
    let store = config(cx);
    let sync = |cx: &mut App| {
        let mut next = (*config(cx).read(cx).snapshot()).clone();
        if reconcile_theme(&mut next) {
            update_config(cx, |c| *c = next);
        }
    };
    sync(cx);
    cx.observe(&store, move |_, cx| sync(cx)).detach();
}

/// A window matching `covers` is on output `monitor`'s active workspace /
/// tags.
fn window_on(monitor: &str, cx: &App, covers: impl Fn(&mandara_compositor::ClientInfo) -> bool) -> bool {
    let Some(hub) = cx.try_global::<crate::hub::GlobalHub>().map(|g| g.0.clone()) else { return false };
    let hub = hub.read(cx);
    let mask = hub.navigation.get(monitor).map(|n| n.active_mask()).unwrap_or(0);
    hub.clients.iter().any(|c| c.monitor == monitor && covers(c) && c.tags.iter().any(|t| *t >= 1 && *t <= 32 && mask & (1u32 << (t - 1)) != 0))
}

/// No tiled or fullscreen window covers output `monitor`'s active
/// workspace / tags (so the desktop, and its audio ring, can be seen).
pub(crate) fn desktop_visible(monitor: &str, cx: &App) -> bool {
    !window_on(monitor, cx, |c| c.fullscreen || !c.floating)
}

/// A fullscreen window is on output `monitor`'s active workspace / tags.
pub(crate) fn fullscreen_on(monitor: &str, cx: &App) -> bool {
    window_on(monitor, cx, |c| c.fullscreen)
}

/// A player is playing something with a title.
pub(crate) fn music_playing(cx: &App) -> bool {
    cx.try_global::<crate::hub::GlobalHub>()
        .is_some_and(|g| g.0.read(cx).media.as_ref().is_some_and(|m| m.status == mandara_services::PlaybackStatus::Playing && !m.title.is_empty()))
}

// ── Background surface ───────────────────────────────────────────────────

pub struct WallpaperView {
    config: Entity<ConfigStore>,
    /// Output this background belongs to (for "is the desktop visible").
    monitor: Option<String>,
    /// The configured source path being shown (or being prepared).
    target: Option<PathBuf>,
    current: Option<PathBuf>,
    previous: Option<PathBuf>,
    /// Pixel sizes of the shown images and the fill they were prepared for.
    current_size: (u32, u32),
    previous_size: (u32, u32),
    fill: mandara_config::desktop::WallpaperFill,
    since: Instant,
    input_cleared: bool,
    /// Small copy of the shown wallpaper: picks the clock's ink.
    tone: Option<Arc<image::RgbImage>>,
    /// Depth layer 3: the subject cut-out over the clock (and the one
    /// fading out).
    masks: Entity<crate::depth::DepthMasks>,
    cutout: Option<Cutout>,
    retired: Option<Cutout>,
    cutout_key: Option<(PathBuf, PathBuf, u64)>,
    stale_images: Vec<Arc<gpui::RenderImage>>,
    /// Audio ring: spectrum while music plays and the desktop shows.
    tap: std::rc::Rc<std::cell::RefCell<crate::plugins::panel_fx::SpectrumTap>>,
    ring: f32,
    ring_at: Instant,
    /// Date pill text, rebuilt once a minute.
    date: Option<(i64, crate::desktop::DateLine)>,
}

/// A layer-3 image with its fade.
struct Cutout {
    /// The subject's pixels, cropped to its bounding box.
    image: Arc<gpui::RenderImage>,
    /// Full displayed image size and the crop within it (for placement).
    full: (u32, u32),
    crop: (u32, u32, u32, u32),
    /// Small aligned alpha: how much of the clock the subject covers.
    map: Arc<image::GrayImage>,
    at: Instant,
    fade: Duration,
}

impl Cutout {
    fn progress(&self) -> f32 {
        let t = (self.at.elapsed().as_secs_f32() / self.fade.as_secs_f32().max(0.001)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
}

impl WallpaperView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self::with_monitor(None, cx)
    }

    /// The background for output `monitor`.
    pub fn on_monitor(monitor: String, cx: &mut Context<Self>) -> Self {
        Self::with_monitor(Some(monitor), cx)
    }

    fn with_monitor(monitor: Option<String>, cx: &mut Context<Self>) -> Self {
        let config = config(cx);
        cx.observe(&config, |_, _, cx| cx.notify()).detach();
        let masks = crate::depth::masks(cx);
        cx.observe(&masks, |_, _, cx| cx.notify()).detach();
        let now = Instant::now();
        Self {
            config,
            monitor,
            target: None,
            current: None,
            previous: None,
            current_size: (0, 0),
            previous_size: (0, 0),
            fill: mandara_config::desktop::WallpaperFill::default(),
            since: now,
            input_cleared: false,
            tone: None,
            masks,
            cutout: None,
            retired: None,
            cutout_key: None,
            stale_images: Vec::new(),
            tap: Default::default(),
            ring: 0.0,
            ring_at: now,
            date: None,
        }
    }

    /// Prepares a display-sized copy off the UI thread, then crossfades.
    fn load(&mut self, path: PathBuf, size: (u32, u32), fill: mandara_config::desktop::WallpaperFill, cx: &mut Context<Self>) {
        self.fill = fill;
        self.target = Some(path.clone());
        let task = cx.background_executor().spawn(async move {
            let (shown, shown_size) = crate::wallpaper_fill::prepare(&path, fill, size.0, size.1)?;
            let tone = image::open(&shown).ok().map(|i| Arc::new(i.thumbnail(TONE_MAP_PX, TONE_MAP_PX).to_rgb8()));
            Ok::<_, String>((path, (shown, shown_size), tone))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok((source, (shown, shown_size), tone)) if this.target.as_ref() == Some(&source) => {
                    this.previous = this.current.take();
                    this.previous_size = this.current_size;
                    this.current = Some(shown);
                    this.current_size = shown_size;
                    this.tone = tone;
                    this.since = Instant::now();
                    // The old subject leaves with its wallpaper.
                    let fade = Duration::from_millis(this.config.read(cx).get().wallpaper.crossfade_ms.max(1));
                    this.retire(fade);
                    this.cutout_key = None;
                    cx.notify();
                }
                Ok(_) => {}
                Err(err) => tracing::warn!("wallpaper: {err}"),
            });
        })
        .detach();
    }

    fn retire(&mut self, fade: Duration) {
        if let Some(old) = self.retired.take() {
            self.stale_images.push(old.image);
        }
        self.retired = self.cutout.take().map(|c| Cutout { at: Instant::now(), fade, ..c });
    }

    fn clear_cutouts(&mut self) {
        self.stale_images.extend(self.cutout.take().map(|c| c.image));
        self.stale_images.extend(self.retired.take().map(|c| c.image));
        self.cutout_key = None;
    }

    /// Keeps layer 3 in step with the shown wallpaper and its mask.
    fn sync_cutout(&mut self, mode: mandara_config::DepthMask, cx: &mut Context<Self>) {
        let (Some(source), Some(shown)) = (self.target.clone(), self.current.clone()) else { return };
        if mode == mandara_config::DepthMask::Off {
            if self.cutout.is_some() {
                self.retire(CUTOUT_EXIT_FADE);
                self.cutout_key = None;
            }
            return;
        }
        let status = self.masks.update(cx, |m, cx| m.status(&source, mode, cx));
        let crate::depth::MaskStatus::Ready(resolved) = status else {
            if matches!(status, crate::depth::MaskStatus::Unavailable(_)) && self.cutout.is_some() {
                self.retire(CUTOUT_EXIT_FADE);
                self.cutout_key = None;
            }
            return;
        };
        let key = (shown.clone(), resolved.path.clone(), self.masks.read(cx).generation);
        if self.cutout_key.as_ref() == Some(&key) {
            return;
        }
        self.cutout_key = Some(key.clone());
        let mask = resolved.path;
        let task = cx.background_executor().spawn(async move {
            crate::depth::cutout(&shown, &source, &mask).map(|c| (Arc::new(gpui::RenderImage::new(smallvec::smallvec![image::Frame::new(c.bgra)])), Arc::new(c.map), c.full, c.crop))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.cutout_key.as_ref() != Some(&key) {
                    return;
                }
                match result {
                    Ok((image, map, full, crop)) => {
                        this.retire(CUTOUT_FADE);
                        this.cutout = Some(Cutout { image, full, crop, map, at: Instant::now(), fade: CUTOUT_FADE });
                    }
                    Err(err) => tracing::warn!("depth cut-out: {err}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn desktop_visible(&self, cx: &App) -> bool {
        self.monitor.as_deref().is_none_or(|m| desktop_visible(m, cx))
    }

    fn music_playing(cx: &App) -> bool {
        music_playing(cx)
    }

    /// Depth layer 2 for this frame (and the redraw it needs).
    fn clock_layer(&mut self, cfg: &mandara_config::ShellConfig, window: &mut Window, cx: &mut Context<Self>) -> (gpui::AnyElement, Option<gpui::AnyElement>) {
        use crate::desktop;
        let vp = window.viewport_size();
        let (w, h) = (f32::from(vp.width).max(1.0), f32::from(vp.height).max(1.0));
        let wc = &cfg.wallpaper;
        let l = desktop::layout(w, h, wc);
        let now = chrono::Local::now();
        let lines = desktop::clock_lines(&now.naive_local(), wc.clock_style, cfg.general.clock_24h);
        let tone = self.tone.as_ref().map(|t| desktop::region_tone(t, (l.center.0 / w, l.center.1 / h), (l.size * 1.3 / w, l.block_h * 0.6 / h)));
        let palette = mandara_core::frame_palette(window, cx);
        let ink = desktop::ink_for(tone, palette.primary);
        let minute = now.timestamp() / 60;
        let date = if wc.show_date {
            if self.date.as_ref().is_none_or(|(m, _)| *m != minute) {
                self.date = Some((minute, desktop::date_line(cfg, wc.show_weather, cx)));
            }
            self.date.as_ref().map(|(_, d)| d.clone())
        } else {
            None
        };
        // The ring listens only while music plays and the desktop shows.
        let want = wc.audio_ring && Self::music_playing(cx) && self.desktop_visible(cx);
        let bands = crate::plugins::panel_fx::SpectrumTap::bands(&self.tap, want, cx);
        let dt = self.ring_at.elapsed().as_secs_f32().min(RING_MAX_DT_S);
        self.ring_at = Instant::now();
        let target = if bands.is_some() { 1.0 } else { 0.0 };
        self.ring += (target - self.ring) * (1.0 - (-dt * RING_EASE_RATE).exp());
        if self.ring < RING_REST && target == 0.0 {
            self.ring = 0.0;
        }
        let delay = if self.ring > 0.0 || bands.is_some() {
            RING_FRAME
        } else if want {
            // Waiting for sound: poll gently.
            RING_POLL
        } else {
            // Next minute boundary (the clock shows minutes only).
            let into_minute = u64::try_from(now.timestamp_millis().rem_euclid(MS_PER_MINUTE)).unwrap_or(0);
            Duration::from_millis(MS_PER_MINUTE.unsigned_abs() - into_minute) + MINUTE_WAKE_SLACK
        };
        crate::plugins::panel_fx::redraw_in(delay, window, cx);
        desktop::element(desktop::ClockFrame { layout: l, lines, style: wc.clock_style, ink, date, bands, ring: self.ring, fonts: fonts(cx), width: w })
    }
}

impl Render for WallpaperView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.input_cleared {
            // The desktop background never takes pointer input.
            window.set_input_region(Some(&[]));
            self.input_cleared = true;
        }
        for image in self.stale_images.drain(..) {
            let _ = window.drop_image(image);
        }
        let cfg = self.config.read(cx).snapshot();
        if !cfg.wallpaper.enabled {
            self.target = None;
            for shown in [self.current.take(), self.previous.take()].into_iter().flatten() {
                crate::image_cache::evict_path(&shown, window, cx);
            }
            self.clear_cutouts();
            return div().size_full();
        }
        // Per-output and light / dark variants, then the main wallpaper.
        let wanted = mandara_config::desktop::effective_wallpaper(cfg.wallpaper.path.as_ref(), &cfg.wallpaper.variants, self.monitor.as_deref(), cfg.theme.mode.is_dark());
        if wanted != self.target || (wanted.is_some() && cfg.wallpaper.fill != self.fill) {
            match wanted {
                Some(path) => {
                    let vp = window.viewport_size();
                    let scale = window.scale_factor();
                    let size = ((f32::from(vp.width) * scale).round() as u32, (f32::from(vp.height) * scale).round() as u32);
                    self.load(path, size, cfg.wallpaper.fill, cx);
                }
                None => {
                    self.target = None;
                    if let Some(shown) = self.current.take() {
                        crate::image_cache::evict_path(&shown, window, cx);
                    }
                    self.clear_cutouts();
                }
            }
        }
        let instant = cfg.motion.reduce_motion || cfg.wallpaper.transition == mandara_config::desktop::WallpaperTransition::None;
        let fade = Duration::from_millis(if instant { 1 } else { cfg.wallpaper.crossfade_ms.max(1) });
        let t = (self.since.elapsed().as_secs_f32() / fade.as_secs_f32()).clamp(0.0, 1.0);
        if t < 1.0 {
            window.request_animation_frame();
        } else if let Some(old) = self.previous.take() {
            // The crossfade is over: release the outgoing image (decoded
            // copy + atlas texture), unless it is shown again.
            if self.current.as_ref() != Some(&old) {
                crate::image_cache::evict_path(&old, window, cx);
            }
        }
        let eased = t * t * (3.0 - 2.0 * t);

        // Layer 2 (clock) needs a wallpaper to sit in; layer 3 needs both.
        let clock = cfg.wallpaper.desktop_clock && self.current.is_some();
        // The subject cut-out is aligned to a cover-cropped image only.
        let covers = self.fill == mandara_config::desktop::WallpaperFill::Cover;
        let mode = if clock && covers && cfg.wallpaper.depth_effect { cfg.wallpaper.depth_mask } else { mandara_config::DepthMask::Off };
        self.sync_cutout(mode, cx);
        let (clock_el, pill_el) = match clock.then(|| self.clock_layer(&cfg, window, cx)) {
            Some((c, p)) => (Some(c), p),
            None => (None, None),
        };
        let fading = |c: &Cutout| c.progress() < 1.0;
        if self.cutout.as_ref().is_some_and(fading) || self.retired.as_ref().is_some_and(fading) {
            window.request_animation_frame();
        }
        if self.retired.as_ref().is_some_and(|r| !fading(r)) {
            if let Some(r) = self.retired.take() {
                self.stale_images.push(r.image);
            }
        }
        let crossfading = self.previous.is_some();
        // A subject may overlap the clock but never swallow it; the date
        // pill rises above a subject that would hide it.
        let vp = window.viewport_size();
        let (vw, vh) = (f32::from(vp.width).max(1.0), f32::from(vp.height).max(1.0));
        let clock_layout = crate::desktop::layout(vw, vh, &cfg.wallpaper);
        let frac = |b: (f32, f32, f32, f32)| (b.0 / vw, b.1 / vh, b.2 / vw, b.3 / vh);
        let digits = frac(crate::desktop::digit_box(&clock_layout, cfg.wallpaper.clock_style));
        let limit = |c: &Cutout| -> f32 { crate::depth::overlap_alpha(crate::depth::coverage(&c.map, digits)) };
        let pill_on_top = self.cutout.as_ref().is_some_and(|c| crate::depth::coverage(&c.map, frac(crate::desktop::pill_box(&clock_layout))) * limit(c) > crate::desktop::PILL_LIFT);
        let (pill_below, pill_above) = if pill_on_top { (None, pill_el) } else { (pill_el, None) };
        let cut = |c: &Cutout, alpha: f32| {
            let (x, y, w, h) = crate::depth::cover_rect(c.full, c.crop, (vw, vh));
            img(gpui::ImageSource::Render(c.image.clone())).absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).object_fit(ObjectFit::Fill).opacity(alpha)
        };

        let scale = window.scale_factor();
        let fill = self.fill;
        let layer = |p: &PathBuf, size: (u32, u32)| crate::wallpaper_fill::layer(p, size, fill, (vw, vh), scale);
        let rtl = cfg.general.locale.is_arabic();
        let transition = cfg.wallpaper.transition;
        let fill_bg = (fill != mandara_config::desktop::WallpaperFill::Cover && self.current.is_some()).then(|| crate::wallpaper_fill::fill_color(&cfg.wallpaper.fill_color).hsla());
        div()
            .size_full()
            .relative()
            // 1 · wallpaper (crossfading)
            .when_some(fill_bg, |el, bg| el.bg(bg))
            .when_some(self.previous.as_ref(), |el, p| el.child(layer(p, self.previous_size)))
            .when_some(self.current.as_ref(), |el, p| el.child(crate::wallpaper_fill::reveal(layer(p, self.current_size), transition, if crossfading { eased } else { 1.0 }, (vw, vh), rtl)))
            // 2 · clock, date, weather, audio ring
            .when_some(clock_el, |el, c| el.child(c))
            .when_some(pill_below, |el, p| el.child(p))
            // 3 · the subject in front of the clock
            .when_some(self.retired.as_ref(), |el, c| el.child(cut(c, (1.0 - c.progress()) * limit(c))))
            .when_some(self.cutout.as_ref(), |el, c| el.child(cut(c, c.progress() * limit(c) * if crossfading { eased } else { 1.0 })))
            .when_some(pill_above, |el, p| el.child(p))
    }
}

/// Side of the downscaled copy used to pick clock ink from the wallpaper.
const TONE_MAP_PX: u32 = 160;
/// Audio ring: frame period while visible, poll period while waiting for
/// sound, easing rate (1/s), rest threshold and longest easing step.
const RING_FRAME: Duration = Duration::from_millis(33);
const RING_POLL: Duration = Duration::from_millis(250);
const RING_EASE_RATE: f32 = 5.0;
const RING_REST: f32 = 0.005;
const RING_MAX_DT_S: f32 = 0.25;
/// Milliseconds per minute, and the slack after a minute boundary before
/// the clock redraws (so the new minute is already current).
const MS_PER_MINUTE: i64 = 60_000;
const MINUTE_WAKE_SLACK: Duration = Duration::from_millis(20);

/// Fade of a depth cut-out arriving (and of the one it replaces).
const CUTOUT_FADE: Duration = Duration::from_millis(400);
/// Fade of a cut-out leaving because depth was switched off / unavailable.
const CUTOUT_EXIT_FADE: Duration = Duration::from_millis(350);

// ── Picker overlay ───────────────────────────────────────────────────────

/// The "video" badge on picker cards: inset from the card corner, height,
/// glyph size and glass opacity.
const BADGE_INSET: f32 = crate::kit::tokens::MD;
const BADGE_H: f32 = crate::kit::tokens::XL + crate::kit::tokens::XS;
const BADGE_ICON: f32 = text::LABEL;
const BADGE_GLASS: f32 = 0.86;

/// The badge marking a video wallpaper (`label`: with the word, else the
/// glyph alone for narrow slices).
fn video_badge(palette: &mandara_theme::MandaraPalette, label: bool) -> gpui::Div {
    div()
        .absolute()
        .top(px(BADGE_INSET))
        .h(px(BADGE_H))
        .min_w(px(BADGE_H))
        .px(px(if label { crate::kit::tokens::SM } else { 0.0 }))
        .rounded_full()
        .bg(frame_color(palette).with_alpha(BADGE_GLASS).hsla())
        .flex()
        .items_center()
        .justify_center()
        .gap(px(crate::kit::tokens::XS))
        .text_size(px(text::LABEL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.on_surface.hsla())
        .child(icon("play", BADGE_ICON, palette.on_surface))
        .when(label, |el| el.child(t("Video")))
}

/// The badge marking a wallpaper the generator made: the video badge's
/// glass at the bottom corner, the sparkle in the accent color.
fn generated_badge(palette: &mandara_theme::MandaraPalette, label: bool) -> gpui::Div {
    div()
        .absolute()
        .bottom(px(BADGE_INSET))
        .left(px(BADGE_INSET))
        .h(px(BADGE_H))
        .min_w(px(BADGE_H))
        .px(px(if label { crate::kit::tokens::SM } else { 0.0 }))
        .rounded_full()
        .bg(frame_color(palette).with_alpha(BADGE_GLASS).hsla())
        .flex()
        .items_center()
        .justify_center()
        .gap(px(crate::kit::tokens::XS))
        .text_size(px(text::LABEL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.on_surface.hsla())
        .child(icon("sparkles", BADGE_ICON, palette.primary))
        .when(label, |el| el.child(t("Generated")))
}

/// The mark on a card whose file can't be read: glyph size and opacity.
const UNREADABLE_GLYPH: f32 = 22.0;
const UNREADABLE_ALPHA: f32 = 0.6;

/// Progress fractions are shown as whole percents.
const PERCENT: f32 = 100.0;

const PICKER_SPRING: SpringConfig = SpringConfig::new(320.0, 22.0, 1.0);
/// Focused card width; neighbours shrink to `SMALL_W` (all 16:10).
/// Carousel side-tile width at size 1.0; the focused tile is
/// `picker_focus` times wider.
const SMALL_W: f32 = 340.0;
const GAP: f32 = 26.0;

/// Where the picker's wallpapers come from.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Source {
    Local,
    Wallhaven,
}

/// Wallhaven browsing state.
#[derive(Default)]
struct Online {
    /// Parallel to `all`: the result behind each entry.
    walls: Vec<Wall>,
    page: u32,
    last_page: u32,
    loading: bool,
    error: Option<String>,
    /// Debounced search after typing.
    search_task: Option<Task<()>>,
    /// Full-resolution prefetch of the focused result.
    prefetch_task: Option<Task<()>>,
    prefetch_for: Option<String>,
    /// A result being downloaded to apply.
    applying: Option<String>,
}

pub struct PickerView {
    shell: Entity<ShellState>,
    source: Source,
    online: Online,
    thumbs: Entity<Thumbnails>,
    focus: FocusHandle,
    all: Vec<PathBuf>,
    shown: Vec<usize>,
    query: String,
    selected: usize,
    offset: SpringValue<f32>,
    appear: SpringValue<f32>,
    clock: FrameClock,
    closing: bool,
    expected: (f32, f32),
    scroll_accum: f32,
    /// Wallpaper generator: the local items made by it (the "Generated"
    /// marker, Remix), the picker's own job and its last error.
    generated: HashSet<PathBuf>,
    gen_job: Option<u64>,
    gen_error: Option<String>,
    /// Images drawn this session (thumbnails, slices, previews): released
    /// from GPUI's image cache when the picker goes, or every session kept
    /// its decoded pixels for good.
    painted: HashSet<PathBuf>,
    /// Frames spent waiting for the surface to take the expected size.
    settle_frames: u8,
}

/// Frames a new picker waits for its surface to take the expected size
/// before drawing at the size it has (the compositor put it on an output
/// of another size): without a limit it stayed invisible, holding the
/// keyboard, and Esc never reached it.
const SETTLE_FALLBACK_FRAMES: u8 = 6;
/// Viewport sizes within this many logical px count as the expected one.
const SETTLE_SLACK_PX: f32 = 2.0;

/// Whether a picker of viewport `viewport` that expected `expected` may
/// draw, after `waited` frames of waiting.
fn picker_settled(viewport: (f32, f32), expected: (f32, f32), waited: u8) -> bool {
    ((viewport.0 - expected.0).abs() <= SETTLE_SLACK_PX && (viewport.1 - expected.1).abs() <= SETTLE_SLACK_PX) || waited >= SETTLE_FALLBACK_FRAMES
}

impl PickerView {
    pub fn new(expected: (f32, f32), window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shell = shell(cx);
        cx.observe(&shell, |this: &mut Self, shell, cx| {
            this.closing = !shell.read(cx).wallpaper_open;
            cx.notify();
        })
        .detach();
        let thumbs = thumbnails(cx);
        cx.observe(&thumbs, |_, _, cx| cx.notify()).detach();
        cx.observe(&config(cx), |_, _, cx| cx.notify()).detach();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let all = list(&directory(cx));
        let current = config(cx).read(cx).get().wallpaper.path.clone();
        let selected = current.and_then(|c| all.iter().position(|p| *p == c)).unwrap_or(0);
        let mut offset = SpringValue::new(selected as f32, PICKER_SPRING);
        offset.set_target(selected as f32);
        // No local wallpapers: browse Wallhaven instead (only with the
        // opt-in Wallhaven plugin on; otherwise no request is ever made).
        let online_first = all.is_empty() && config(cx).read(cx).get().wallhaven_enabled();
        let mut this = Self {
            shell,
            source: Source::Local,
            online: Online::default(),
            thumbs,
            focus,
            shown: (0..all.len()).collect(),
            all,
            query: String::new(),
            selected,
            offset,
            appear: SpringValue::new(0.0, PICKER_SPRING),
            clock: FrameClock::new(),
            closing: false,
            expected,
            scroll_accum: 0.0,
            generated: HashSet::new(),
            gen_job: None,
            gen_error: None,
            painted: HashSet::new(),
            settle_frames: 0,
        };
        // A picker removed without its outro (its output went away) still
        // lets go of its images.
        cx.on_release(|this: &mut Self, cx| {
            for path in this.painted.drain() {
                crate::image_cache::forget_path(&path, cx);
            }
        })
        .detach();
        if crate::plugins::wallpaper_generator::enabled(cx) {
            this.generated = crate::plugins::wallpaper_generator::model::generated_among(&this.all);
            let generator = crate::plugins::wallpaper_generator::wallgen(cx);
            cx.observe(&generator, |this: &mut Self, generator, cx| this.generator_changed(&generator, cx)).detach();
        }
        if online_first {
            this.set_source(Source::Wallhaven, cx);
        }
        this
    }

    /// Switches between the local folder and Wallhaven.
    fn set_source(&mut self, source: Source, cx: &mut Context<Self>) {
        if source == self.source {
            return;
        }
        self.source = source;
        self.query.clear();
        self.selected = 0;
        self.offset = SpringValue::new(0.0, PICKER_SPRING);
        self.online = Online::default();
        match source {
            Source::Local => {
                self.all = list(&directory(cx));
                self.shown = (0..self.all.len()).collect();
                if crate::plugins::wallpaper_generator::enabled(cx) {
                    self.generated = crate::plugins::wallpaper_generator::model::generated_among(&self.all);
                }
            }
            Source::Wallhaven => {
                self.all.clear();
                self.shown.clear();
                self.search_online(true, cx);
            }
        }
        cx.notify();
    }

    /// Fetches a Wallhaven page (and its previews): a fresh search when
    /// `reset`, else the next page appended.
    fn search_online(&mut self, reset: bool, cx: &mut Context<Self>) {
        let cfg = config(cx).read(cx).snapshot();
        if (self.online.loading && !reset) || !cfg.wallhaven_enabled() {
            return;
        }
        let text = if self.query.trim().is_empty() { cfg.wallpaper.wallhaven_query.clone() } else { self.query.clone() };
        let page = if reset { 1 } else { self.online.page + 1 };
        let q = wallhaven::Query { text, sorting: if self.query.trim().is_empty() { cfg.wallpaper.wallhaven_sorting.clone() } else { "relevance".into() }, atleast: "1920x1080".into(), page };
        self.online.loading = true;
        self.online.error = None;
        let task = cx.background_executor().spawn(async move {
            let page = wallhaven::search(&q)?;
            // Previews in parallel, keeping result order.
            let previews: Vec<Option<PathBuf>> = std::thread::scope(|s| {
                let handles: Vec<_> = page.walls.iter().map(|w| s.spawn(move || wallhaven::preview(w).ok())).collect();
                handles.into_iter().map(|h| h.join().ok().flatten()).collect()
            });
            Ok::<_, String>((page, previews))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.online.loading = false;
                match result {
                    Ok((page, previews)) => {
                        if reset {
                            this.all.clear();
                            this.online.walls.clear();
                            this.selected = 0;
                            this.offset = SpringValue::new(0.0, PICKER_SPRING);
                        }
                        for (w, p) in page.walls.into_iter().zip(previews) {
                            if let Some(p) = p {
                                this.all.push(p);
                                this.online.walls.push(w);
                            }
                        }
                        this.shown = (0..this.all.len()).collect();
                        this.online.page = page.page;
                        this.online.last_page = page.last_page;
                        if this.all.is_empty() {
                            this.online.error = Some(t("No Wallhaven results").into());
                        }
                    }
                    Err(e) => this.online.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Wallhaven: loads the focused result at full resolution after a short
    /// dwell (sharper preview, instant apply), and the next page near the end.
    fn online_upkeep(&mut self, cx: &mut Context<Self>) {
        if self.source != Source::Wallhaven {
            return;
        }
        if !self.online.loading && self.online.page < self.online.last_page && self.selected + 5 >= self.shown.len() && !self.shown.is_empty() {
            self.search_online(false, cx);
        }
        let Some(&i) = self.shown.get(self.selected) else { return };
        let Some(wall) = self.online.walls.get(i).cloned() else { return };
        if self.online.prefetch_for.as_deref() == Some(wall.id.as_str()) || self.all[i].starts_with(wallhaven::download_dir()) {
            return;
        }
        self.online.prefetch_for = Some(wall.id.clone());
        self.online.prefetch_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREFETCH_DWELL).await;
            let w = wall.clone();
            let full = cx.background_executor().spawn(async move { wallhaven::download(&w) }).await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(path) = full {
                    if let Some(pos) = this.online.walls.iter().position(|x| x.id == wall.id) {
                        this.all[pos] = path;
                    }
                }
                cx.notify();
            });
        }));
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        self.shell.update(cx, |s, cx| s.set_wallpaper_picker(false, cx));
    }

    /// The focused local wallpaper, if the generator made it.
    fn focused_generated(&self) -> Option<&PathBuf> {
        if self.source != Source::Local {
            return None;
        }
        self.shown.get(self.selected).map(|&i| &self.all[i]).filter(|p| self.generated.contains(*p))
    }

    /// Generate: a variation of the focused generated wallpaper, else a
    /// random one; a second press while it runs cancels it.
    fn generate(&mut self, cx: &mut Context<Self>) {
        use crate::plugins::wallpaper_generator as wg;
        let generator = wg::wallgen(cx);
        if self.gen_job.is_some() && generator.read(cx).busy() {
            generator.update(cx, |w, cx| w.cancel(cx));
            return;
        }
        let base = self.focused_generated().and_then(|p| mandara_wallgen::recipe_for(p));
        self.gen_error = None;
        match wg::generate_from_picker(base, cx) {
            Ok(id) => self.gen_job = Some(id),
            Err(e) => self.gen_error = Some(e),
        }
        cx.notify();
    }

    /// The picker's job ended: list the new file and focus it.
    fn generator_changed(&mut self, generator: &Entity<crate::plugins::wallpaper_generator::Wallgen>, cx: &mut Context<Self>) {
        let Some(job) = self.gen_job else {
            cx.notify();
            return;
        };
        let Some(finished) = generator.read(cx).finished.clone().filter(|f| f.id == job) else {
            cx.notify();
            return;
        };
        self.gen_job = None;
        match finished.result {
            Ok(done) if self.source == Source::Local => {
                self.all = list(&directory(cx));
                self.generated = crate::plugins::wallpaper_generator::model::generated_among(&self.all);
                self.query.clear();
                self.shown = (0..self.all.len()).collect();
                if let Some(slot) = self.all.iter().position(|p| *p == done.file.path) {
                    self.selected = slot;
                }
            }
            Ok(_) => {}
            Err(e) => self.gen_error = Some(e),
        }
        cx.notify();
    }

    /// Re-filters by the query, keeping the selected image selected when
    /// it still matches.
    fn filter(&mut self) {
        if self.source == Source::Wallhaven {
            return;
        }
        let keep = self.shown.get(self.selected).copied();
        let q = self.query.to_lowercase();
        self.shown = (0..self.all.len()).filter(|&i| q.is_empty() || self.all[i].file_stem().is_some_and(|s| s.to_string_lossy().to_lowercase().contains(&q))).collect();
        let selected = keep.and_then(|k| self.shown.iter().position(|&i| i == k));
        self.selected = selected.unwrap_or(0);
        if selected.is_none() {
            self.offset = SpringValue::new(0.0, PICKER_SPRING);
        }
    }

    fn step(&mut self, delta: isize) {
        if self.shown.is_empty() {
            return;
        }
        self.selected = (self.selected as isize + delta).clamp(0, self.shown.len() as isize - 1) as usize;
    }

    fn apply_selected(&mut self, cx: &mut Context<Self>) {
        let Some(&i) = self.shown.get(self.selected) else { return };
        if self.source == Source::Wallhaven && !self.all[i].starts_with(wallhaven::download_dir()) {
            let Some(wall) = self.online.walls.get(i).cloned() else { return };
            self.online.applying = Some(wall.id.clone());
            cx.spawn(async move |this, cx| {
                let full = cx.background_executor().spawn(async move { wallhaven::download(&wall) }).await;
                let _ = this.update(cx, |this, cx| {
                    this.online.applying = None;
                    match full {
                        Ok(path) => {
                            if let Some(slot) = this.all.get_mut(i) {
                                *slot = path.clone();
                            }
                            apply(path, cx);
                        }
                        Err(e) => this.online.error = Some(e),
                    }
                    cx.notify();
                });
            })
            .detach();
            cx.notify();
            return;
        }
        apply(self.all[i].clone(), cx);
    }

    /// Typing in Wallhaven mode searches after a pause.
    fn query_changed(&mut self, cx: &mut Context<Self>) {
        if self.source != Source::Wallhaven {
            self.filter();
            return;
        }
        self.online.search_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| this.search_online(true, cx));
        }));
    }

    fn on_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        match k.key.as_str() {
            "escape" if !self.query.is_empty() => {
                self.query.clear();
                self.query_changed(cx);
            }
            "tab" if config(cx).read(cx).get().wallhaven_enabled() => {
                let next = if self.source == Source::Local { Source::Wallhaven } else { Source::Local };
                self.set_source(next, cx);
            }
            "escape" => self.close(cx),
            "left" => self.step(-1),
            "right" => self.step(1),
            "pageup" => self.step(-5),
            "pagedown" => self.step(5),
            "home" => self.step(-(self.shown.len() as isize)),
            "end" => self.step(self.shown.len() as isize),
            "enter" => self.apply_selected(cx),
            "backspace" => {
                if self.query.pop().is_some() {
                    self.query_changed(cx);
                }
            }
            _ => {
                if let Some(ch) = k.key_char.as_ref().filter(|_| !k.modifiers.control && !k.modifiers.alt) {
                    self.query.push_str(ch);
                    self.query_changed(cx);
                }
            }
        }
        self.online_upkeep(cx);
        cx.notify();
    }
}

/// Horizontal centre of a carousel card `rel` slots from the focused one
/// (focused cards `big` wide, others `small`), interpolated continuously so
/// the carousel glides while the offset spring moves.
fn slot_center(rel: f32, big: f32, small: f32, gap: f32) -> f32 {
    rel.signum() * (rel.abs() * (small + gap) + (big - small) / 2.0 * rel.abs().min(1.0))
}

fn slot_width(rel: f32, big: f32, small: f32) -> f32 {
    small + (big - small) * (1.0 - rel.abs()).clamp(0.0, 1.0)
}

/// Slices layout (Omarchy-style): a large 16:9 preview with slanted slices
/// overlapping on either side.
#[derive(Copy, Clone)]
struct Slices {
    preview_x: f32,
    center_y: f32,
    expanded: (f32, f32),
    slice: (f32, f32),
    spacing: f32,
}

impl Slices {
    fn new(sw: f32, sh: f32, size: f32, center_y: f32) -> Self {
        let ew = (sw * 0.5 * size).min(sw * 0.86);
        let eh = (sh * 0.7).min(ew * 9.0 / 16.0);
        let slice = (ew * 0.28, eh * 0.94);
        Self { preview_x: (sw - ew) / 2.0, center_y, expanded: (ew, eh), slice, spacing: -slice.0 * 0.2 }
    }

    fn step(&self) -> f32 {
        self.slice.0 + self.spacing
    }

    /// Box `(x, y, w, h)` of the item `rel` (integer) places from the focus.
    fn box_at(&self, rel: f32) -> (f32, f32, f32, f32) {
        let (ew, eh) = self.expanded;
        let (w, h, x) = if rel.abs() < 1e-4 {
            (ew, eh, self.preview_x)
        } else if rel < 0.0 {
            (self.slice.0, self.slice.1, self.preview_x + rel * self.step())
        } else {
            (self.slice.0, self.slice.1, self.preview_x + ew + self.spacing + (rel - 1.0) * self.step())
        };
        (x, self.center_y - h / 2.0, w, h)
    }

    /// Interpolated box for a fractional position (the strip glides).
    fn lerp(&self, rel: f32) -> (f32, f32, f32, f32) {
        let a = rel.floor();
        let t = rel - a;
        let (l, r) = (self.box_at(a), self.box_at(a + 1.0));
        (l.0 + (r.0 - l.0) * t, l.1 + (r.1 - l.1) * t, l.2 + (r.2 - l.2) * t, l.3 + (r.3 - l.3) * t)
    }
}

impl Render for PickerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vp = window.viewport_size();
        let (sw, sh) = (f32::from(vp.width), f32::from(vp.height));
        if !picker_settled((sw, sh), self.expected, self.settle_frames) && !self.closing {
            self.settle_frames = self.settle_frames.saturating_add(1);
            window.request_animation_frame();
            return div().id("picker-root").size_full().track_focus(&self.focus).into_any_element();
        }
        let palette = mandara_core::frame_palette(window, cx);
        let f = fonts(cx);
        let cfg = config(cx).read(cx).snapshot();
        // Exit without overshoot; the entrance keeps its spring.
        self.appear.set_config(if self.closing { SpringConfig::critical(420.0) } else { PICKER_SPRING });
        self.appear.set_target(if self.closing { 0.0 } else { 1.0 });
        self.offset.set_target(self.selected as f32);
        let dt = self.clock.tick(Instant::now());
        let moving = self.appear.step(dt) | self.offset.step(dt);
        if self.closing && self.appear.value() <= 0.01 {
            for path in self.painted.drain() {
                crate::image_cache::evict_path(&path, window, cx);
            }
            window.remove_window();
            return div().id("picker-root").size_full().into_any_element();
        }
        if moving {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        let appear = self.appear.value().clamp(0.0, 1.2);
        let shown_alpha = appear.min(1.0);
        let offset = self.offset.value();
        let (cx0, cy0) = (sw / 2.0, sh / 2.0 - 30.0);
        let style = cfg.wallpaper.picker_style;
        let size = cfg.wallpaper.picker_size.clamp(0.6, 1.6);
        // Carousel metrics.
        let small = SMALL_W * size;
        let big = (small * cfg.wallpaper.picker_focus.clamp(1.5, 4.0)).min(sw * 0.62);
        let gap = GAP * size;
        // Slices metrics.
        let slices = Slices::new(sw, sh, size, cy0);
        let reach = match style {
            mandara_config::PickerStyle::Carousel => ((sw / 2.0) / (small + gap)).ceil() as isize + 1,
            mandara_config::PickerStyle::Slices => ((slices.preview_x / slices.step()).ceil() as isize).clamp(1, 6) + 1,
        };

        // Thumbnails for what's visible, nearest first (workers pop from the end).
        let mut wanted: Vec<PathBuf> = Vec::new();
        let reach_thumbs = if style == mandara_config::PickerStyle::Carousel { reach + 4 } else { -1 };
        for d in (0..=reach_thumbs).rev() {
            for i in [self.selected as isize + d, self.selected as isize - d] {
                if i >= 0 && (i as usize) < self.shown.len() {
                    wanted.push(self.all[self.shown[i as usize]].clone());
                }
            }
        }
        self.thumbs.update(cx, |t, cx| t.request(wanted, cx));

        let current = cfg.wallpaper.path.clone();
        let thumbs = self.thumbs.read(cx);
        let mut cards = Vec::new();
        if style == mandara_config::PickerStyle::Slices && !self.shown.is_empty() {
            // Slanted variants at device-pixel sizes (quantized so the cache
            // keys stay stable), the preview one a bit under full resolution.
            let scale = window.scale_factor();
            let q8 = |v: f32| ((v * scale / 8.0).round() * 8.0).max(8.0) as u32;
            let (sjw, sjh) = (q8(slices.slice.0), q8(slices.slice.1));
            let (bjw, bjh) = (q8(slices.expanded.0 * 0.8), q8(slices.expanded.1 * 0.8));
            let lo = (self.selected as isize - reach).max(0) as usize;
            let hi = ((self.selected as isize + reach) as usize).min(self.shown.len() - 1);
            let mut jobs: Vec<Job> = Vec::new();
            for slot in (lo..=hi).rev() {
                jobs.push((self.all[self.shown[slot]].clone(), sjw, sjh));
            }
            for d in [1isize, -1, 0] {
                let i = self.selected as isize + d;
                if i >= 0 && (i as usize) < self.shown.len() {
                    jobs.push((self.all[self.shown[i as usize]].clone(), bjw, bjh));
                }
            }
            self.thumbs.update(cx, |t, cx| t.request_jobs(jobs, cx));
            let thumbs = self.thumbs.read(cx);
            // Far slices first so nearer ones overlap them.
            let mut order: Vec<usize> = (lo..=hi).collect();
            order.sort_by(|a, b| (*b as f32 - offset).abs().total_cmp(&(*a as f32 - offset).abs()));
            for slot in order {
                let path = &self.all[self.shown[slot]];
                let rel = slot as f32 - offset;
                let (x, y, w, h) = slices.lerp(rel);
                let y = y + (1.0 - shown_alpha) * 40.0;
                let lean = SHEAR * h;
                let (left, width) = (x - lean / 2.0, w + lean);
                if left + width < 0.0 || left > sw {
                    continue;
                }
                let e = (1.0 - rel.abs()).clamp(0.0, 1.0);
                let is_video = mandara_media::is_video(path);
                let is_generated = self.generated.contains(path);
                let slice_img = thumbs.get_job(&(path.clone(), sjw, sjh)).cloned();
                let big_img = thumbs.get_job(&(path.clone(), bjw, bjh)).cloned();
                self.painted.extend(slice_img.iter().chain(big_img.iter().filter(|_| e > 0.0)).cloned());
                let ready = slice_img.is_some() || big_img.is_some();
                // An unreadable file (corrupt, or not an image despite its
                // name) says so instead of looking forever like it loads.
                let unreadable = !ready && thumbs.failed_job(&(path.clone(), sjw, sjh));
                let placeholder = palette.surface_container_high.with_alpha(0.7);
                cards.push(
                    div()
                        .id(("slice", slot))
                        .absolute()
                        .left(px(left))
                        .top(px(y))
                        .w(px(width))
                        .h(px(h))
                        .cursor_pointer()
                        .opacity(shown_alpha)
                        .when_some(slice_img, |el, p| el.child(img(p).absolute().inset_0().size_full().object_fit(ObjectFit::Fill).opacity(if big_img.is_some() { 1.0 - e } else { 1.0 })))
                        .when_some(big_img.filter(|_| e > 0.0), |el, p| el.child(img(p).absolute().inset_0().size_full().object_fit(ObjectFit::Fill).opacity(e)))
                        // Frame: dim when unfocused, light rim brightening with focus.
                        .child(
                            gpui::canvas(
                                |_, _, _| {},
                                move |b, _, window, _| {
                                    let (bx, by) = (f32::from(b.origin.x), f32::from(b.origin.y));
                                    let (bw, bh) = (f32::from(b.size.width), f32::from(b.size.height));
                                    let l = SHEAR * bh;
                                    let pts = [(bx + l, by), (bx + bw, by), (bx + bw - l, by + bh), (bx, by + bh)];
                                    let shape = |mut pb: gpui::PathBuilder| {
                                        for (i, (px_, py_)) in pts.iter().enumerate() {
                                            let pt = point(px(*px_), px(*py_));
                                            if i == 0 { pb.move_to(pt) } else { pb.line_to(pt) }
                                        }
                                        pb.close();
                                        pb.build().ok()
                                    };
                                    if !ready {
                                        if let Some(path) = shape(gpui::PathBuilder::fill()) {
                                            window.paint_path(path, placeholder.hsla());
                                        }
                                    } else if e < 1.0 {
                                        if let Some(path) = shape(gpui::PathBuilder::fill()) {
                                            window.paint_path(path, gpui::black().opacity(0.16 * (1.0 - e)));
                                        }
                                    }
                                    if let Some(path) = shape(gpui::PathBuilder::stroke(px(1.0 + e))) {
                                        window.paint_path(path, gpui::Rgba { r: 1.0, g: 0.97, b: 0.93, a: 0.28 + 0.72 * e });
                                    }
                                },
                            )
                            .absolute()
                            .inset_0(),
                        )
                        .when(unreadable, |el| el.child(div().absolute().inset_0().flex().items_center().justify_center().child(icon("close", UNREADABLE_GLYPH, palette.on_surface_variant.with_alpha(UNREADABLE_ALPHA)))))
                        // Inside the slant: the top edge starts one lean in.
                        .when(is_video, |el| el.child(video_badge(&palette, false).left(px(lean + BADGE_INSET))))
                        .when(is_generated, |el| el.child(generated_badge(&palette, false)))
                        .keep_inside()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.selected == slot {
                                this.apply_selected(cx);
                            } else {
                                this.selected = slot;
                                this.online_upkeep(cx);
                            }
                            cx.notify();
                        })),
                );
            }
        } else if !self.shown.is_empty() {
            let lo = (self.selected as isize - reach).max(0) as usize;
            let hi = ((self.selected as isize + reach) as usize).min(self.shown.len() - 1);
            for slot in lo..=hi {
                let path = &self.all[self.shown[slot]];
                let rel = slot as f32 - offset;
                let w = slot_width(rel, big, small);
                let h = w / 1.6;
                // Entrance: cards rise and fan out from the centre.
                let x = cx0 + slot_center(rel, big, small, gap) * (0.85 + 0.15 * shown_alpha) - w / 2.0;
                let y = cy0 - h / 2.0 + (1.0 - shown_alpha) * 50.0;
                if x + w < 0.0 || x > sw {
                    continue;
                }
                let focus = (1.0 - rel.abs()).clamp(0.0, 1.0);
                let alpha = (1.0 - (rel.abs() - 1.0).max(0.0) * 0.16).clamp(0.25, 1.0) * shown_alpha;
                let radius = 14.0 + 8.0 * focus;
                let is_current = current.as_deref() == Some(path.as_path());
                let is_video = mandara_media::is_video(path);
                let is_generated = self.generated.contains(path);
                let thumb = thumbs.get(path).cloned();
                self.painted.extend(thumb.iter().cloned());
                let failed = thumbs.failed(path);
                cards.push(
                    div()
                        .id(("wall", slot))
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .w(px(w))
                        .h(px(h))
                        .rounded(px(crate::style::radius(radius)))
                        .clip_rounded()
                        .cursor_pointer()
                        .opacity(alpha)
                        .bg(palette.surface_container_high.with_alpha(0.6).hsla())
                        .shadow(vec![BoxShadow {
                            color: gpui::black().opacity(0.22 + 0.2 * focus),
                            offset: point(px(0.0), px(8.0 + 10.0 * focus)),
                            blur_radius: px(18.0 + 22.0 * focus),
                            spread_radius: px(0.0),
                            inset: false,
                        }])
                        .when(focus > 0.6, |el| el.border_2().border_color(gpui::white().opacity(0.85)))
                        .child(match thumb {
                            Some(t) => img(t).size_full().rounded(px(crate::style::radius(radius))).object_fit(ObjectFit::Cover).into_any_element(),
                            None => div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon(if failed { "close" } else { "image" }, UNREADABLE_GLYPH, palette.on_surface_variant.with_alpha(UNREADABLE_ALPHA)))
                                .into_any_element(),
                        })
                        .when(is_video, |el| el.child(video_badge(&palette, true).left(px(BADGE_INSET))))
                        .when(is_generated, |el| el.child(generated_badge(&palette, true)))
                        .when(is_current, |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .top(px(10.0))
                                    .right(px(10.0))
                                    .size(px(24.0))
                                    .rounded_full()
                                    .bg(palette.primary.hsla())
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(icon("check", 14.0, palette.on_primary)),
                            )
                        })
                        .keep_inside()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.selected == slot {
                                this.apply_selected(cx);
                            } else {
                                this.selected = slot;
                                this.online_upkeep(cx);
                            }
                            cx.notify();
                        })),
                );
            }
        }

        // The light controls: one floating pill under the focused card.
        let online = self.source == Source::Wallhaven;
        let name = if online {
            match (&self.online.applying, &self.online.error, self.online.loading && self.all.is_empty()) {
                (Some(_), _, _) => t("Downloading…").to_string(),
                (_, Some(e), _) => e.clone(),
                (_, _, true) => t("Searching Wallhaven…").to_string(),
                _ => self.shown.get(self.selected).and_then(|&i| self.online.walls.get(i)).map(|w| tf("Wallhaven · {resolution}", &[("resolution", &w.resolution)])).unwrap_or_default(),
            }
        } else {
            self.shown.get(self.selected).and_then(|&i| self.all[i].file_stem()).map(|s| s.to_string_lossy().replace(['-', '_'], " ")).unwrap_or_default()
        };
        // The wallpaper generator's controls (local folder, plugin on).
        let generator = self.source == Source::Local && crate::plugins::wallpaper_generator::enabled(cx);
        let generating = self.gen_job.and(crate::plugins::wallpaper_generator::existing(cx)).and_then(|g| g.read(cx).progress()).map(|(p, _)| p);
        let remix = self.focused_generated().cloned();
        let name = match (&self.gen_error, generating) {
            (Some(e), None) if generator => e.clone(),
            _ => name,
        };
        let match_colors = cfg.wallpaper.match_colors;
        let glass = frame_color(&palette).with_alpha(0.92);
        let pill_btn = |id: &'static str, icon_name: &'static str, label: Option<gpui::SharedString>, on: bool| {
            div()
                .id(id)
                .h(px(30.0))
                .px(px(if label.is_some() { 12.0 } else { 8.0 }))
                .rounded_full()
                .flex()
                .items_center()
                .gap(px(6.0))
                .cursor_pointer()
                .text_size(px(text::BODY_S))
                .font_weight(FontWeight::MEDIUM)
                .bg(if on { palette.primary.hsla() } else { gpui::transparent_black() })
                .text_color(if on { palette.on_primary.hsla() } else { palette.on_surface.hsla() })
                .when(on, |el| el.hover(|s| s.opacity(0.9)))
                .when(!on, |el| el.hover(|s| s.bg(palette.on_surface.with_alpha(0.08).hsla())))
                .child(icon(icon_name, 14.0, if on { palette.on_primary } else { palette.on_surface_variant }))
                .when_some(label, |el, l| el.child(l))
        };
        let searching = !self.query.is_empty();
        let controls = div()
            .id("picker-controls")
            .flex()
            .items_center()
            .gap(px(4.0))
            .h(px(42.0))
            .px(px(6.0))
            .rounded_full()
            .bg(glass.hsla())
            .shadow(vec![BoxShadow { color: gpui::black().opacity(0.2), offset: point(px(0.0), px(6.0)), blur_radius: px(18.0), spread_radius: px(0.0), inset: false }])
            .keep_inside()
            .child(
                div()
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(icon("search", 13.0, palette.on_surface_variant))
                    .child(
                        div()
                            .min_w(px(90.0))
                            .text_size(px(text::BODY_S))
                            .text_color(if searching { palette.on_surface.hsla() } else { palette.on_surface_variant.with_alpha(0.75).hsla() })
                            .child(if searching { format!("{}▏", self.query) } else if online { t("Search Wallhaven").to_string() } else { t("Type to filter").to_string() }),
                    ),
            )
            .child(div().w(px(1.0)).h(px(18.0)).bg(palette.on_surface.with_alpha(0.12).hsla()))
            .child(
                div()
                    .px(px(12.0))
                    .max_w(px(260.0))
                    .truncate()
                    .text_size(px(text::BODY_S))
                    .font_weight(FontWeight::MEDIUM)
                    .child(if online { name } else if self.all.is_empty() { tf("No images in {directory}", &[("directory", &cfg.wallpaper.directory)]) } else if cfg.wallpaper.picker_names { name } else { String::new() }),
            )
            .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).font_features(crate::fonts::tabular_nums()).child(format!("{} / {}", (self.selected + 1).min(self.shown.len()), self.shown.len())))
            .child(div().w(px(1.0)).h(px(18.0)).mx(px(6.0)).bg(palette.on_surface.with_alpha(0.12).hsla()))
            .when(cfg.wallhaven_enabled(), |el| {
                el.child(pill_btn("wall-source", if online { "cloud" } else { "image" }, Some(if online { "Wallhaven" } else { t("Local") }.into()), online).on_click(cx.listener(|this, _, _, cx| {
                    let next = if this.source == Source::Local { Source::Wallhaven } else { Source::Local };
                    this.set_source(next, cx);
                })))
            })
            .child(pill_btn("wall-match", "sun", Some(t("Match colors").into()), match_colors).on_click(|_, _, cx| update_config(cx, |c| c.wallpaper.match_colors = !c.wallpaper.match_colors)))
            // Wallpaper generator: Remix the focused generated one in
            // Studio, or Generate a new one (a second press cancels).
            .when(generator && remix.is_some(), |el| {
                el.child(pill_btn("wall-remix", "sliders", Some(t("Remix").into()), false).on_click(cx.listener(move |this, _, _, cx| {
                    let Some(path) = this.focused_generated().cloned() else { return };
                    match crate::plugins::wallpaper_generator::remix_in_studio(&path, cx) {
                        Ok(()) => this.close(cx),
                        Err(e) => this.gen_error = Some(e),
                    }
                    cx.notify();
                })))
            })
            .when(generator, |el| {
                let label: gpui::SharedString = match generating {
                    Some(Some(p)) => tf("Generating… {percent}%", &[("percent", &((p * PERCENT).round() as u32))]).into(),
                    Some(None) => t("Generating…").into(),
                    None => t("Generate").into(),
                };
                el.child(
                    pill_btn("wall-generate", if generating.is_some() { "close" } else { "sparkles" }, Some(label), generating.is_some())
                        .font_features(crate::fonts::tabular_nums())
                        .on_click(cx.listener(|this, _, _, cx| this.generate(cx))),
                )
            })
            .child(pill_btn("wall-random", "refresh", None, false).on_click(cx.listener(|this, _, _, cx| {
                if !this.shown.is_empty() {
                    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() as usize).unwrap_or(0);
                    this.selected = n % this.shown.len();
                    this.apply_selected(cx);
                }
                cx.notify();
            })))
            .child(pill_btn("wall-close", "close", None, false).on_click(cx.listener(|this, _, _, cx| this.close(cx))));
        let half_h = match style {
            mandara_config::PickerStyle::Carousel => big / 1.6 / 2.0,
            mandara_config::PickerStyle::Slices => slices.expanded.1 / 2.0,
        };
        let controls_top = cy0 + half_h + 30.0 + (1.0 - shown_alpha) * 30.0;

        div()
            .id("picker-root")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .font_family(f.ui.clone())
            .text_color(palette.on_surface.hsla())
            .text_size(px(13.0 * cfg.typography.scale))
            // A light veil so the floating cards read against any desktop.
            .bg(gpui::black().opacity(0.32 * shown_alpha))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.on_key(e, cx)))
            .on_scroll(cx.listener(|this, e: &ScrollEvent, _, cx| {
                let d = e.delta.pixel_delta(px(40.0));
                this.scroll_accum += f32::from(d.y) + f32::from(d.x);
                while this.scroll_accum.abs() >= 40.0 {
                    this.step(if this.scroll_accum > 0.0 { -1 } else { 1 });
                    this.scroll_accum -= 40.0 * this.scroll_accum.signum();
                }
                this.online_upkeep(cx);
                cx.notify();
            }))
            // Cards and controls stop propagation, so any press reaching the
            // root is outside them: dismiss.
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close(cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| this.close(cx)))
            .children(cards)
            .child(div().absolute().left_0().right_0().top(px(controls_top)).flex().justify_center().opacity(shown_alpha).child(controls))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_colors_switches_and_restores_theme() {
        let mut c = mandara_config::ShellConfig::default();
        c.theme.source = ThemeSource::Preset { preset: "rose-pine".into() };
        c.wallpaper.path = Some(PathBuf::from("/w/a.jpg"));
        c.wallpaper.match_colors = true;
        assert!(reconcile_theme(&mut c));
        assert_eq!(c.theme.source, ThemeSource::Wallpaper { path: "/w/a.jpg".into(), variant: SchemeVariant::default(), seed: 0 });
        assert!(!reconcile_theme(&mut c), "stable once applied");
        c.wallpaper.path = Some(PathBuf::from("/w/b.jpg"));
        assert!(reconcile_theme(&mut c));
        assert!(matches!(&c.theme.source, ThemeSource::Wallpaper { path, .. } if path.ends_with("b.jpg")));
        // A light / dark variant on screen drives the colours.
        let dark = c.theme.mode.is_dark();
        let variant = Some(PathBuf::from("/w/variant.jpg"));
        if dark { c.wallpaper.variants.dark = variant } else { c.wallpaper.variants.light = variant }
        assert!(reconcile_theme(&mut c));
        assert!(matches!(&c.theme.source, ThemeSource::Wallpaper { path, .. } if path.ends_with("variant.jpg")));
        c.wallpaper.match_colors = false;
        assert!(reconcile_theme(&mut c));
        assert_eq!(c.theme.source, ThemeSource::Preset { preset: "rose-pine".into() });
    }

    #[test]
    fn picker_draws_on_an_output_of_another_size_after_a_few_frames() {
        let first = (1707.0, 960.0);
        // The expected size (within the slack) draws at once.
        assert!(picker_settled(first, first, 0));
        assert!(picker_settled((first.0 + SETTLE_SLACK_PX, first.1), first, 0));
        // GPUI's first frames use a guess: wait for the real size...
        let other = (1920.0, 1080.0);
        assert!(!picker_settled(other, first, 0));
        assert!(!picker_settled(other, first, SETTLE_FALLBACK_FRAMES - 1));
        // ...but never forever (it stayed invisible, holding the keyboard).
        assert!(picker_settled(other, first, SETTLE_FALLBACK_FRAMES));
        assert!(picker_settled(other, first, u8::MAX));
    }

    #[test]
    fn cover_crops_to_aspect() {
        let img = image::DynamicImage::new_rgb8(1000, 400);
        let out = cover(&img, 480, 300);
        assert_eq!((out.width(), out.height()), (480, 300));
        let tall = image::DynamicImage::new_rgb8(300, 900);
        assert_eq!(cover(&tall, 480, 300).dimensions_tuple(), (480, 300));
    }

    trait Dims {
        fn dimensions_tuple(&self) -> (u32, u32);
    }
    impl Dims for image::DynamicImage {
        fn dimensions_tuple(&self) -> (u32, u32) {
            (self.width(), self.height())
        }
    }

    #[test]
    fn lists_images_sorted_and_skips_others() {
        let dir = std::env::temp_dir().join(format!("mandara-wall-test-{}", std::process::id()));
        let sub = dir.join("nested");
        std::fs::create_dir_all(&sub).unwrap();
        for name in ["b.jpg", "A.PNG", "notes.txt"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        std::fs::write(sub.join("c.webp"), b"x").unwrap();
        let names: Vec<String> = list(&dir).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["A.PNG", "b.jpg", "c.webp"]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
