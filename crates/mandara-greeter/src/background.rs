//! The animated backdrop: AV1 video clips (looped, crossfaded), a Ken Burns
//! slideshow of stills, or a generated OKLCH aurora — always moving.
//!
//! Layers stack bottom → top; the top layer fades in over the others and,
//! once opaque, everything under it is released (its frames dropped from
//! the GPU atlas). The aurora is the permanent floor, so there is never a
//! black frame while a clip or image loads.
//!
//! Video playback is clocked by wall time: each frame the newest decoded
//! frame whose timestamp is due is shown and older ones are dropped. When
//! the decoder can't keep up, the clock pauses instead of jumping (no
//! stutter, just a slower clip), and sustained stalls switch to lite
//! decoding at reduced resolution, then to the stills / aurora fallback.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use gpui::{AnyElement, BoxShadow, Hsla, ImageSource, IntoElement, ObjectFit, ParentElement as _, RenderImage, Styled, StyledImage as _, div, img, point, px};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_theme::{Color, MandaraPalette};

use crate::media::{ClipInfo, Player, PlayerEvent, PlayerOptions, VideoFrame};

/// Playback considered warmed up after this (decoder spin-up stalls ignored).
const WARMUP: Duration = Duration::from_millis(1500);
/// Stall share over the window that triggers a downgrade.
const STALL_LIMIT: f32 = 0.22;
const STALL_WINDOW: Duration = Duration::from_secs(4);
/// Softening at or above which clips decode in lite mode (the blur hides
/// the lost detail).
const LITE_BLUR: f32 = 0.35;

#[derive(Clone, Debug)]
pub struct BackgroundOptions {
    pub clips: Vec<PathBuf>,
    pub stills: Vec<PathBuf>,
    /// Output size in physical pixels.
    pub target: (u32, u32),
    pub lite: bool,
    pub video_enabled: bool,
    /// Minimum time each clip is shown when there are several.
    pub clip_seconds: u32,
    pub video_fade: Duration,
    pub still_fade: Duration,
    pub still_life: Duration,
    /// Ken Burns zoom range.
    pub ken_burns: f32,
    /// Video playback speed.
    pub rate: f32,
    /// 0–1 softening (stills blurred, videos at reduced resolution).
    pub blur: f32,
    /// Solid floor instead of the aurora.
    pub solid: Option<Color>,
    /// Freeze the aurora / Ken Burns drift (reduced motion).
    pub still_motion: bool,
}

/// Output size assumed before the real one is known, physical px.
const DEFAULT_TARGET: (u32, u32) = (1920, 1080);
/// Minimum time each clip plays when there are several, seconds.
const DEFAULT_CLIP_SECONDS: u32 = 30;
/// Cross-fades between clips and between stills, and a still's life.
const DEFAULT_VIDEO_FADE: Duration = Duration::from_millis(1400);
const DEFAULT_STILL_FADE: Duration = Duration::from_millis(2600);
const DEFAULT_STILL_LIFE: Duration = Duration::from_secs(16);
/// Ken Burns zoom range (fraction of the image).
const DEFAULT_KEN_BURNS: f32 = 0.12;
/// A clip plays at least this long before handing over, seconds.
const MIN_CLIP_SECONDS: u32 = 5;
/// The next clip starts this much before the current one's fade-out.
const HANDOVER_LEAD: Duration = Duration::from_millis(400);
/// The next still is requested this much before its fade must start.
const STILL_PRELOAD_LEAD: Duration = Duration::from_millis(1200);
/// Frame step assumed when a clip has no nominal rate.
const FALLBACK_FRAME_STEP: Duration = Duration::from_millis(40);
/// Slowest playback rate honored (avoids dividing by ~0).
const MIN_PLAYBACK_RATE: f32 = 0.05;

impl Default for BackgroundOptions {
    fn default() -> Self {
        Self {
            clips: Vec::new(),
            stills: Vec::new(),
            target: DEFAULT_TARGET,
            lite: false,
            video_enabled: true,
            clip_seconds: DEFAULT_CLIP_SECONDS,
            video_fade: DEFAULT_VIDEO_FADE,
            still_fade: DEFAULT_STILL_FADE,
            still_life: DEFAULT_STILL_LIFE,
            ken_burns: DEFAULT_KEN_BURNS,
            rate: 1.0,
            blur: 0.0,
            solid: None,
            still_motion: false,
        }
    }
}

struct VideoLayer {
    player: Player,
    lite: bool,
    clip: usize,
    info: Option<ClipInfo>,
    /// Wall-clock instant corresponding to pts 0.
    origin: Option<Instant>,
    current: Option<VideoFrame>,
    next: Option<VideoFrame>,
    ended: bool,
    error: Option<String>,
    first_frame_at: Option<Instant>,
    stall_window: (Instant, Duration),
    struggling: bool,
    successor_started: bool,
    dropped: u64,
    shown: u64,
}

struct StillLayer {
    /// The only still: zoom back and forth forever instead of stopping.
    cycle: bool,
    image: Arc<RenderImage>,
    size: (f32, f32),
    zoom: (f32, f32),
    pan: ((f32, f32), (f32, f32)),
}

enum Content {
    Video(Box<VideoLayer>),
    Still(StillLayer),
}

struct Layer {
    content: Content,
    /// When the fade-in started (`None` until the layer has a picture).
    shown_at: Option<Instant>,
    fade: Duration,
}

impl Layer {
    fn opacity(&self, now: Instant) -> f32 {
        match self.shown_at {
            None => 0.0,
            Some(t) => ease(now.saturating_duration_since(t).as_secs_f32() / self.fade.as_secs_f32().max(1e-3)),
        }
    }
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// What is currently painting (for the diagnostics line and tests).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Showing {
    Aurora,
    Still(usize),
    Video { clip: usize, lite: bool },
}

pub struct Background {
    opts: BackgroundOptions,
    layers: Vec<Layer>,
    failed_clips: Vec<bool>,
    video_given_up: bool,
    next_clip: usize,
    next_still: usize,
    failed_stills: usize,
    loading_still: Option<Receiver<Option<(usize, Arc<RenderImage>, (u32, u32))>>>,
    trash: Vec<Arc<RenderImage>>,
    started: Instant,
    seed: u64,
    pub last_error: Option<String>,
}

impl Background {
    pub fn new(mut opts: BackgroundOptions, shuffle: bool) -> Self {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7) | 1;
        if shuffle {
            shuffle_paths(&mut opts.clips, seed);
            shuffle_paths(&mut opts.stills, seed.rotate_left(17));
        }
        let failed_clips = vec![false; opts.clips.len()];
        let mut bg = Background {
            opts,
            layers: Vec::new(),
            failed_clips,
            video_given_up: false,
            next_clip: 0,
            next_still: 0,
            failed_stills: 0,
            loading_still: None,
            trash: Vec::new(),
            started: Instant::now(),
            seed,
            last_error: None,
        };
        if bg.video_active() {
            bg.start_clip(bg.next_clip, bg.opts.lite);
        }
        bg
    }

    fn video_active(&self) -> bool {
        self.opts.video_enabled && !self.video_given_up && self.failed_clips.iter().any(|f| !f)
    }

    /// Updates the output size (frames are converted for it).
    pub fn set_target(&mut self, target: (u32, u32)) {
        self.opts.target = target;
    }

    /// The output size frames are converted for, physical px.
    pub fn target(&self) -> (u32, u32) {
        self.opts.target
    }

    /// Applies new timings (clip length, fades, a still's life, Ken Burns,
    /// playback rate) without restarting anything: what is playing keeps
    /// playing, at the new rate from the frame on screen, and the next
    /// fade / slide uses the rest.
    pub fn retime(&mut self, timing: &BackgroundOptions, now: Instant) {
        let (old_rate, new_rate) = (self.opts.rate, timing.rate);
        if old_rate != new_rate {
            for layer in &mut self.layers {
                if let Content::Video(v) = &mut layer.content
                    && let Some(origin) = v.origin
                {
                    v.origin = Some(rebase_origin(origin, now, old_rate, new_rate));
                }
            }
        }
        self.opts.clip_seconds = timing.clip_seconds;
        self.opts.video_fade = timing.video_fade;
        self.opts.still_fade = timing.still_fade;
        self.opts.still_life = timing.still_life;
        self.opts.ken_burns = timing.ken_burns;
        self.opts.rate = new_rate;
    }

    /// Whether the backdrop's first picture is on screen (or there is
    /// nothing to wait for: aurora / solid only, or every source failed).
    pub fn ready(&self) -> bool {
        let pending = self.video_active() || (!self.opts.stills.is_empty() && self.failed_stills < self.opts.stills.len());
        !pending || self.layers.iter().any(|l| l.shown_at.is_some())
    }

    /// Whether a clip is playing (it needs every frame).
    pub fn has_video(&self) -> bool {
        self.layers.iter().any(|l| matches!(l.content, Content::Video(_)))
    }

    pub fn showing(&self) -> Showing {
        match self.layers.iter().rev().find(|l| l.shown_at.is_some()).map(|l| &l.content) {
            Some(Content::Video(v)) => Showing::Video { clip: v.clip, lite: v.lite },
            Some(Content::Still(_)) => Showing::Still(self.next_still.saturating_sub(1)),
            None => Showing::Aurora,
        }
    }

    fn start_clip(&mut self, clip: usize, lite: bool) {
        let n = self.opts.clips.len();
        if n == 0 {
            return;
        }
        // Next playable clip at or after `clip`.
        let Some(clip) = (0..n).map(|i| (clip + i) % n).find(|i| !self.failed_clips[*i]) else { return };
        let play_for = (n > 1).then(|| Duration::from_secs(self.opts.clip_seconds.max(MIN_CLIP_SECONDS) as u64));
        let player = crate::media::spawn(PlayerOptions {
            path: self.opts.clips[clip].clone(),
            target: self.opts.target,
            lite: lite || self.opts.blur >= LITE_BLUR,
            play_for,
            queue: crate::media::FRAME_QUEUE,
        });
        tracing::info!("background: playing {}{}", self.opts.clips[clip].display(), if lite { " (lite)" } else { "" });
        let now = Instant::now();
        self.next_clip = (clip + 1) % n;
        self.layers.push(Layer {
            content: Content::Video(Box::new(VideoLayer {
                player,
                lite,
                clip,
                info: None,
                origin: None,
                current: None,
                next: None,
                ended: false,
                error: None,
                first_frame_at: None,
                stall_window: (now, Duration::ZERO),
                struggling: false,
                successor_started: false,
                dropped: 0,
                shown: 0,
            })),
            shown_at: None,
            fade: self.opts.video_fade,
        });
    }

    fn request_still(&mut self) {
        if self.loading_still.is_some() || self.opts.stills.is_empty() || self.failed_stills >= self.opts.stills.len() {
            return;
        }
        let ix = self.next_still % self.opts.stills.len();
        self.next_still += 1;
        let path = self.opts.stills[ix].clone();
        tracing::info!("background: still {}", path.display());
        let target = self.opts.target;
        let blur = self.opts.blur;
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new().name("mandara-greeter-still".into()).spawn(move || {
            let _ = tx.send(load_still(&path, target, blur).map(|(img, size)| (ix, img, size)));
        });
        self.loading_still = Some(rx);
    }

    fn push_still(&mut self, image: Arc<RenderImage>, size: (u32, u32)) {
        // Alternate zoom direction and pick a pan diagonal per slide.
        self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let r = |shift: u32| ((self.seed >> shift) & 0xFFFF) as f32 / 65535.0 * 2.0 - 1.0;
        let zoom_in = self.next_still % 2 == 1;
        let kb = self.opts.ken_burns;
        let zoom = if zoom_in { (1.02, 1.02 + kb) } else { (1.02 + kb, 1.02) };
        let a = (r(3) * 0.8, r(19) * 0.8);
        let pan = (a, (-a.0 * 0.9 + r(35) * 0.2, -a.1 * 0.9 + r(51) * 0.2));
        self.layers.push(Layer {
            content: Content::Still(StillLayer { cycle: self.opts.stills.len() == 1, image, size: (size.0 as f32, size.1 as f32), zoom, pan }),
            shown_at: Some(Instant::now()),
            fade: self.opts.still_fade,
        });
    }

    /// Advances playback; returns whether anything changed visually (the
    /// view animates every frame anyway, this is for diagnostics).
    pub fn tick(&mut self, now: Instant) {
        let clip_count = self.opts.clips.len();
        let mut start_next: Option<(usize, bool)> = None;
        let mut failed: Vec<(usize, String)> = Vec::new();
        let mut give_up_video = false;
        let top_ix = self.layers.len().saturating_sub(1);
        for (i, layer) in self.layers.iter_mut().enumerate() {
            let Content::Video(v) = &mut layer.content else { continue };
            pump(v, now, self.opts.rate, &mut self.trash);
            if layer.shown_at.is_none() && v.current.is_some() {
                layer.shown_at = Some(now);
            }
            if let Some(err) = v.error.take() {
                failed.push((v.clip, err));
                continue;
            }
            if i != top_ix {
                continue;
            }
            // Downgrade when the decoder can't keep up.
            if v.struggling && !v.successor_started {
                v.successor_started = true;
                if v.lite {
                    tracing::warn!("background: video too heavy even in lite mode; using stills");
                    give_up_video = true;
                } else {
                    tracing::warn!("background: decoder falling behind; switching to lite decoding");
                    start_next = Some((v.clip, true));
                }
                continue;
            }
            // Hand over to the next clip before this one ends.
            if clip_count > 1 && !v.successor_started {
                let playing = v.origin.map(|o| now.saturating_duration_since(o));
                let planned_end = v.info.as_ref().and_then(|i| i.duration).map(|d| {
                    let min = Duration::from_secs(self.opts.clip_seconds.max(MIN_CLIP_SECONDS) as u64);
                    let loops = (min.as_secs_f64() / d.as_secs_f64().max(0.1)).ceil().max(1.0);
                    d.mul_f64(loops)
                });
                let near_end = match (playing, planned_end) {
                    (Some(p), Some(end)) => p.mul_f32(self.opts.rate) + self.opts.video_fade + HANDOVER_LEAD >= end,
                    _ => false,
                };
                if near_end || v.ended {
                    v.successor_started = true;
                    start_next = Some((self.next_clip, v.lite));
                }
            }
        }
        for (clip, err) in failed {
            tracing::warn!("background: {err}");
            self.last_error = Some(err);
            if let Some(f) = self.failed_clips.get_mut(clip) {
                *f = true;
            }
            // Drop the broken layer (one that showed frames stays frozen
            // until the next clip fades over it).
            self.layers.retain(|l| !matches!(&l.content, Content::Video(v) if v.clip == clip && v.current.is_none()));
            start_next = Some((self.next_clip, self.opts.lite));
        }
        if give_up_video {
            self.video_given_up = true;
        }
        if let Some((clip, lite)) = start_next
            && self.video_active()
        {
            self.start_clip(clip, lite);
        }

        // Stills when video isn't (or is no longer) available.
        if !self.video_active() {
            if let Some(rx) = &self.loading_still {
                match rx.try_recv() {
                    Ok(Some((_, image, size))) => {
                        self.loading_still = None;
                        self.push_still(image, size);
                    }
                    Ok(None) | Err(TryRecvError::Disconnected) => {
                        self.loading_still = None;
                        self.failed_stills += 1;
                    }
                    Err(TryRecvError::Empty) => {}
                }
            }
            let need = match self.layers.last() {
                Some(Layer { content: Content::Still(_), shown_at, .. }) => {
                    let age = shown_at.map(|t| now.saturating_duration_since(t)).unwrap_or_default();
                    self.opts.stills.len() > 1 && age + self.opts.still_fade + STILL_PRELOAD_LEAD >= self.opts.still_life
                }
                // Nothing yet, or a (given-up / failed) video on top.
                _ => true,
            };
            if need {
                self.request_still();
            }
        }

        // Release everything under a fully opaque top layer.
        if let Some(pos) = self.layers.iter().rposition(|l| l.opacity(now) >= 1.0)
            && pos > 0
        {
            for layer in self.layers.drain(..pos) {
                release(layer, &mut self.trash);
            }
        }
    }

    /// Images to remove from the GPU atlas (call `window.drop_image`, then
    /// hand them to [`Background::dispose`]).
    pub fn take_trash(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.trash)
    }

    /// Frees an image's pixels off the UI thread when a player is running
    /// (its converter thread drops them), else here.
    pub fn dispose(&self, image: Arc<RenderImage>) {
        let player = self.layers.iter().rev().find_map(|l| match &l.content {
            Content::Video(v) => Some(&v.player),
            Content::Still(_) => None,
        });
        match player {
            Some(p) => p.recycle(image),
            None => drop(image),
        }
    }

    /// Whether the aurora floor is visible (no opaque layer covers it).
    fn aurora_visible(&self, now: Instant) -> bool {
        !self.layers.iter().any(|l| l.opacity(now) >= 1.0)
    }

    /// The backdrop element for a `vw × vh` viewport. `zoom` ≥ 1 pushes in
    /// (the success transition).
    /// `shift` moves the picture (pointer parallax, px); it is overscanned
    /// so edges never show.
    pub fn element(&self, vw: f32, vh: f32, zoom: f32, shift: (f32, f32), palette: &MandaraPalette, now: Instant) -> AnyElement {
        let floor = self.opts.solid.unwrap_or(palette.surface_dim);
        let mut root = div().absolute().top_0().left_0().w(px(vw)).h(px(vh)).overflow_hidden().bg(floor.hsla());
        let over = 1.0 + 2.0 * shift.0.abs().max(shift.1.abs()) / vw.min(vh).max(1.0);
        let zoom = zoom * over;
        if self.aurora_visible(now) && self.opts.solid.is_none() {
            let t = if self.opts.still_motion { 0.0 } else { now.saturating_duration_since(self.started).as_secs_f32() };
            root = root.child(aurora(vw, vh, palette, t, shift));
        }
        for layer in &self.layers {
            let alpha = layer.opacity(now);
            if alpha <= 0.0 {
                continue;
            }
            let el = match &layer.content {
                Content::Video(v) => match &v.current {
                    Some(f) => cover(f.image.clone(), (f.width as f32, f.height as f32), vw, vh, zoom, (0.0, 0.0), shift),
                    None => continue,
                },
                Content::Still(s) => {
                    let age = layer.shown_at.map(|t| now.saturating_duration_since(t).as_secs_f32()).unwrap_or(0.0);
                    let phase = if self.opts.still_motion { 0.5 } else { age / self.opts.still_life.as_secs_f32() };
                    let e = if s.cycle {
                        // Triangle wave → eased: in, out, in…
                        ease(1.0 - ((phase % 2.0) - 1.0).abs())
                    } else {
                        // Keep drifting slightly past the slide's life while
                        // the next one fades in.
                        ease(phase.clamp(0.0, 1.3) / 1.3) * 1.3
                    };
                    let z = s.zoom.0 + (s.zoom.1 - s.zoom.0) * e.min(1.0) + (e - 1.0).max(0.0) * 0.03;
                    let p = (s.pan.0.0 + (s.pan.1.0 - s.pan.0.0) * e.min(1.0), s.pan.0.1 + (s.pan.1.1 - s.pan.0.1) * e.min(1.0));
                    cover(s.image.clone(), s.size, vw, vh, z * zoom, p, shift)
                }
            };
            root = root.child(div().absolute().top_0().left_0().size_full().opacity(alpha).child(el));
        }
        root.into_any_element()
    }

    /// Newest video layer: (frames shown, frames dropped, average decode +
    /// convert cost per frame, seconds played, lite).
    pub fn video_stats(&self, now: Instant) -> Option<(u64, u64, Duration, f32, bool)> {
        self.layers.iter().rev().find_map(|l| match &l.content {
            Content::Video(v) => Some((
                v.shown,
                v.dropped,
                v.player.stats.frame_cost(),
                v.first_frame_at.map(|t| now.saturating_duration_since(t).as_secs_f32()).unwrap_or(0.0),
                v.lite,
            )),
            _ => None,
        })
    }
}

/// The wall-clock origin (instant of pts 0) that keeps the media time at
/// `now` unchanged when the playback rate goes from `old_rate` to
/// `new_rate`, so a rate change neither jumps ahead nor stalls.
fn rebase_origin(origin: Instant, now: Instant, old_rate: f32, new_rate: f32) -> Instant {
    let (old_rate, new_rate) = (old_rate.max(MIN_PLAYBACK_RATE), new_rate.max(MIN_PLAYBACK_RATE));
    if old_rate == new_rate {
        return origin;
    }
    let media = now.saturating_duration_since(origin).mul_f32(old_rate);
    now.checked_sub(media.div_f32(new_rate)).unwrap_or(origin)
}

/// Pulls decoded frames and picks the one due now.
fn pump(v: &mut VideoLayer, now: Instant, rate: f32, trash: &mut Vec<Arc<RenderImage>>) {
    // Wall-clock time of a timestamp at this playback rate.
    let wall = |pts: Duration| pts.div_f32(rate.max(MIN_PLAYBACK_RATE));
    let interval = wall(v.info.as_ref().filter(|i| i.fps > 0.0).map(|i| Duration::from_secs_f32(1.0 / i.fps)).unwrap_or(FALLBACK_FRAME_STEP));
    loop {
        if v.next.is_none() {
            match v.player.try_recv() {
                Some(PlayerEvent::Info(i)) => {
                    v.info = Some(i);
                    continue;
                }
                Some(PlayerEvent::Frame(f)) => v.next = Some(f),
                Some(PlayerEvent::End) => {
                    v.ended = true;
                    break;
                }
                Some(PlayerEvent::Error(e)) => {
                    v.error = Some(e);
                    break;
                }
                None => break,
            }
        }
        let Some(next) = v.next.as_ref() else { break };
        let origin = *v.origin.get_or_insert_with(|| now.checked_sub(wall(next.pts)).unwrap_or(now));
        if v.current.is_none() || origin + wall(next.pts) <= now {
            if v.current.is_some() && v.first_frame_at.is_some() {
                // More than one frame due in this tick → the extras are dropped.
                if origin + wall(next.pts) + interval <= now {
                    v.dropped += 1;
                }
            }
            if let Some(old) = v.current.replace(v.next.take().expect("checked")) {
                trash.push(old.image);
            }
            v.shown += 1;
            v.first_frame_at.get_or_insert(now);
        } else {
            break;
        }
    }
    // Stall: the next frame is overdue and nothing is queued.
    if let (Some(origin), Some(cur), None, false) = (v.origin, v.current.as_ref(), v.next.as_ref(), v.ended) {
        let late = now.saturating_duration_since(origin + wall(cur.pts));
        if late > interval * 2 {
            let pause = late - interval * 2;
            // Pause the clock rather than jump ahead later.
            v.origin = Some(origin + pause);
            let warm = v.first_frame_at.is_some_and(|t| now.saturating_duration_since(t) > WARMUP);
            if warm {
                v.stall_window.1 += pause;
            }
        }
    }
    if now.saturating_duration_since(v.stall_window.0) >= STALL_WINDOW {
        let share = v.stall_window.1.as_secs_f32() / STALL_WINDOW.as_secs_f32();
        if share > STALL_LIMIT {
            v.struggling = true;
        }
        v.stall_window = (now, Duration::ZERO);
    }
}

fn release(layer: Layer, trash: &mut Vec<Arc<RenderImage>>) {
    match layer.content {
        Content::Video(v) => {
            let v = *v;
            if let Some(f) = v.current {
                trash.push(f.image);
            }
            if let Some(f) = v.next {
                trash.push(f.image);
            }
        }
        Content::Still(s) => trash.push(s.image),
    }
}

/// An image scaled to cover the viewport, zoomed and panned (`pan` in
/// −1…1 of the available overflow on each axis).
fn cover(image: Arc<RenderImage>, size: (f32, f32), vw: f32, vh: f32, zoom: f32, pan: (f32, f32), shift: (f32, f32)) -> AnyElement {
    let (iw, ih) = (size.0.max(1.0), size.1.max(1.0));
    let scale = (vw / iw).max(vh / ih) * zoom.max(1.0);
    let (w, h) = (iw * scale, ih * scale);
    let (ox, oy) = ((w - vw) / 2.0, (h - vh) / 2.0);
    let x = (-ox + pan.0.clamp(-1.0, 1.0) * ox * 0.8 + shift.0).max(vw - w).min(0.0);
    let y = (-oy + pan.1.clamp(-1.0, 1.0) * oy * 0.8 + shift.1).max(vh - h).min(0.0);
    img(ImageSource::Render(image)).absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).object_fit(ObjectFit::Fill).into_any_element()
}

/// Soft blob: a shadow-only circle (no fill), so its edge is a Gaussian.
fn blob(cx: f32, cy: f32, r: f32, color: Hsla) -> AnyElement {
    div()
        .absolute()
        .left(px(cx - r * 0.5))
        .top(px(cy - r * 0.5))
        .size(px(r))
        .rounded_full()
        .shadow(vec![BoxShadow { color, offset: point(px(0.0), px(0.0)), blur_radius: px(r * 0.9), spread_radius: px(r * 0.15), inset: false }])
        .into_any_element()
}

/// The generated aurora: a deep OKLCH gradient with drifting glows tinted
/// from the theme, dithered with micro-grain.
pub fn aurora(vw: f32, vh: f32, p: &MandaraPalette, t: f32, shift: (f32, f32)) -> AnyElement {
    // Hues from the theme (achromatic tokens fall back to rotations of the
    // primary), at fixed vivid OKLCH lightness / chroma so every theme gets
    // a luminous sky rather than muddy glows.
    let hue = |c: Color, fallback: f32| if c.chroma() < 0.02 { (p.primary.hue() + fallback).rem_euclid(360.0) } else { c.hue() };
    let (h1, h2, h3, h4) = (hue(p.primary, 0.0), hue(p.tertiary, 70.0), hue(p.secondary, -60.0), hue(p.accent_gradient_end, 30.0));
    // The sky takes the surface's hue (indigo for Montaza Twilight, sea
    // blue for Corniche); glows take the accents.
    let hs = hue(p.surface_base, 180.0);
    let base_top = Color::oklch(0.25, 0.09, hs);
    let base_bottom = Color::oklch(0.11, 0.07, (hs + 18.0).rem_euclid(360.0));
    let m = vw.max(vh);
    let wobble = |period: f32, phase: f32| (t / period * std::f32::consts::TAU + phase).sin();
    let glow = |l: f32, c: f32, h: f32, a: f32| Color::oklcha(l, c, h, a).hsla();
    let glows = [
        (0.50 + 0.22 * wobble(89.0, 1.7), 0.42 + 0.10 * wobble(31.0, 0.9), 0.75, glow(0.50, 0.19, (hs + 25.0).rem_euclid(360.0), 0.95)),
        (0.18 + 0.10 * wobble(47.0, 0.0), 0.30 + 0.08 * wobble(61.0, 1.0), 0.50, glow(0.68, 0.17, h1, 0.9)),
        (0.82 + 0.08 * wobble(53.0, 2.0), 0.22 + 0.10 * wobble(43.0, 0.5), 0.46, glow(0.64, 0.16, h2, 0.85)),
        (0.58 + 0.14 * wobble(71.0, 4.0), 0.84 + 0.06 * wobble(37.0, 3.0), 0.56, glow(0.60, 0.16, h4, 0.75)),
        (0.08 + 0.06 * wobble(59.0, 5.0), 0.88 + 0.05 * wobble(67.0, 2.5), 0.38, glow(0.74, 0.14, h3, 0.7)),
    ];
    let mut root = div().absolute().top_0().left_0().w(px(vw)).h(px(vh)).bg(oklab_gradient(160.0, base_top, base_bottom));
    for (x, y, r, color) in glows {
        // Bigger glows sit deeper: they parallax less.
        let depth = 1.4 - r;
        root = root.child(blob(x * vw + shift.0 * depth, y * vh + shift.1 * depth, r * m * 0.55, color));
    }
    root.child(grain(vw, vh)).into_any_element()
}

/// Anti-banding grain: one noise image the size of the viewport (each
/// noise pixel ≈ 1 logical px), generated once per size. Tiling a small
/// tile shows seams at fractional scales and stretching it shows blotches.
pub fn grain(vw: f32, vh: f32) -> AnyElement {
    thread_local! {
        static CACHE: std::cell::RefCell<Option<((u32, u32), Arc<RenderImage>)>> = const { std::cell::RefCell::new(None) };
    }
    let size = ((vw.ceil() as u32).clamp(1, 4096), (vh.ceil() as u32).clamp(1, 4096));
    let image = CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((s, img)) = c.as_ref()
            && *s == size
        {
            return img.clone();
        }
        let mut state: u32 = 0x9E37_79B9;
        let buf = image::RgbaImage::from_fn(size.0, size.1, |_, _| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let v = if state & 1 == 0 { 255 } else { 0 };
            image::Rgba([v, v, v, 4 - ((state >> 8) & 1) as u8])
        });
        let img = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buf)]));
        *c = Some((size, img.clone()));
        img
    });
    img(ImageSource::Render(image)).absolute().top_0().left_0().w(px(vw)).h(px(vh)).object_fit(ObjectFit::Fill).into_any_element()
}

/// Decodes and downsizes a still to at most ~1.25× the output (Ken Burns
/// zooms in up to 16 %), as BGRA.
fn load_still(path: &std::path::Path, target: (u32, u32), blur: f32) -> Option<(Arc<RenderImage>, (u32, u32))> {
    let img = image::open(path).map_err(|e| tracing::warn!("background {}: {e}", path.display())).ok()?;
    let (tw, th) = ((target.0.max(640) as f32 * 1.25) as u32, (target.1.max(360) as f32 * 1.25) as u32);
    let scale = (tw as f32 / img.width() as f32).max(th as f32 / img.height() as f32);
    let img = if scale < 1.0 {
        img.resize((img.width() as f32 * scale) as u32, (img.height() as f32 * scale) as u32, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    // Soften: blur a quarter-size copy (the GPU's upscale adds the rest).
    let img = if blur > 0.01 {
        let small = img.resize((img.width() / 4).max(16), (img.height() / 4).max(9), image::imageops::FilterType::Triangle);
        small.blur(0.5 + blur * 9.0)
    } else {
        img
    };
    let mut rgba = img.to_rgba8();
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let size = (rgba.width(), rgba.height());
    Some((Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(rgba)])), size))
}

fn shuffle_paths(v: &mut [PathBuf], mut seed: u64) {
    for i in (1..v.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        v.swap(i, (seed % (i as u64 + 1)) as usize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_change_keeps_the_media_time() {
        let origin = Instant::now();
        let now = origin + Duration::from_secs(10);
        // 10 s at 1× = 10 s of media; at 0.5× that is 20 s of wall clock.
        let rebased = rebase_origin(origin, now, 1.0, 0.5);
        let media_after = now.saturating_duration_since(rebased).mul_f32(0.5);
        assert!((media_after.as_secs_f32() - 10.0).abs() < 1e-3);
        // Same rate: unchanged.
        assert_eq!(rebase_origin(origin, now, 1.0, 1.0), origin);
    }

    #[test]
    fn easing_is_clamped_and_smooth() {
        assert_eq!(ease(-1.0), 0.0);
        assert_eq!(ease(2.0), 1.0);
        assert!((ease(0.5) - 0.5).abs() < 1e-6);
        assert!(ease(0.25) < 0.25 && ease(0.75) > 0.75);
    }

    #[test]
    fn shuffle_is_a_permutation() {
        let mut v: Vec<PathBuf> = (0..20).map(|i| PathBuf::from(i.to_string())).collect();
        shuffle_paths(&mut v, 12345);
        let mut sorted: Vec<u32> = v.iter().map(|p| p.to_string_lossy().parse().unwrap()).collect();
        assert_ne!(sorted, (0..20).collect::<Vec<_>>());
        sorted.sort();
        assert_eq!(sorted, (0..20).collect::<Vec<_>>());
    }

    #[test]
    fn no_sources_means_aurora() {
        let bg = Background::new(BackgroundOptions::default(), false);
        assert_eq!(bg.showing(), Showing::Aurora);
    }

    #[test]
    fn broken_clips_fall_back_to_stills() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.webm");
        std::fs::write(&bad, b"nope").unwrap();
        let still = dir.path().join("s.png");
        image::RgbaImage::from_pixel(64, 36, image::Rgba([10, 20, 200, 255])).save(&still).unwrap();
        let mut bg = Background::new(BackgroundOptions { clips: vec![bad], stills: vec![still], target: (640, 360), ..Default::default() }, false);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !matches!(bg.showing(), Showing::Still(_)) {
            bg.tick(Instant::now());
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(bg.showing(), Showing::Still(_)), "{:?}", bg.showing());
        assert!(bg.last_error.is_some());
    }
}
