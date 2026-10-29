//! Video wallpapers on the software backdrop.
//!
//! A wallpaper whose file is an AV1 clip (`.webm`, `.mkv`, `.ivf`) is
//! first shown like a still: its first frame (cached as a poster) goes
//! through the normal load → crossfade path, so transitions from and to
//! images, the clock and the widgets work unchanged. Once it is on screen
//! and nothing is transitioning, playback takes over the same wallpaper
//! plane:
//!
//! * **One decoder per clip.** A [`FrameStream`] thread per playing clip
//!   (shared by every output showing it) decodes YUV pictures into a
//!   two-picture queue. Each output converts the shared picture *directly*
//!   into its own `wl_shm` buffer ([`Host::paint_pixels`], buffers reused,
//!   so nothing is allocated per frame), at the clip's resolution for the
//!   visible crop; the compositor scales it to the output with
//!   `wp_viewporter` ([`placement`]).
//! * **Frame pacing** ([`plan`]): frames are presented at their timestamps,
//!   at most [`WallpaperVideo::fps_cap`] per second, gated by the output's
//!   frame callback: nothing is drawn while the previous frame waits for
//!   the compositor. After a pause the clock is rebased, never fast-
//!   forwarded.
//! * **Pausing costs nothing.** Playback stops ([`may_play`]) while the
//!   session is locked, the screens are off, on battery (optional) and
//!   while windows cover the desktop; it also stops by itself on an output
//!   whose compositor withholds frame callbacks (a surface nobody sees).
//!   A stopped clip schedules no timer; its decoder blocks on the full
//!   queue (no CPU, no wake-ups) and is dropped after [`PARK_RELEASE`]
//!   (memory back to a still's), restarting from the first frame later.
//! * With no video wallpaper nothing here exists: no thread, no timer.
//!
//! The depth effect's subject cut-out is skipped for videos (the subject
//! moves; a mask from one frame would be wrong for the rest), so the clock
//! and widgets sit on top of the clip.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mandara_config::desktop::{VideoPause, WallpaperFill, WallpaperVideo};
use mandara_core::shm_canvas::{Host, Place, Rect, Waker, snap};
use mandara_media::{Crop, FrameStream, PixelOrder, StreamEvent, StreamFrame, StreamOptions, Target};

pub use mandara_media::is_video;

/// AV1 worker threads per playing clip. Four decode 1440p30 in real time
/// on ordinary laptops while leaving cores to the compositor and apps;
/// more threads only add per-frame synchronisation overhead.
const DECODER_THREADS: usize = 4;
/// Frames rav1d decodes in parallel. Each one in flight holds a picture
/// (≈ 3 MiB at 1080p); two keep the pipeline busy at a wallpaper's pace.
const DECODER_FRAME_DELAY: u32 = 2;
/// Pictures pulled ahead of presentation (the queue plus the one being
/// looked at): enough to drop a frame when the cap is below the clip rate.
const LOOKAHEAD: usize = mandara_media::player::STREAM_QUEUE + 1;
/// A stopped clip keeps its decoder this long (resuming continues
/// seamlessly), then drops it to free its memory.
pub const PARK_RELEASE: Duration = Duration::from_secs(60);
/// A frame this late (decoder behind, or playback resumed after a pause)
/// rebases the clock instead of skipping ahead to catch up.
pub const LATE_REBASE: Duration = Duration::from_millis(250);
/// Buffers larger than 1440p convert on several threads. A 1440p frame
/// converts in about 2 ms on one core, so below this, spawning helpers per
/// frame would cost more than it saves.
const PARALLEL_CONVERT_PIXELS: u64 = 2560 * 1440;
/// Most threads one conversion uses.
const MAX_CONVERT_THREADS: usize = 4;

// ── Pure decisions (tested) ──────────────────────────────────────────────

/// What the shell knows about one output when deciding whether its video
/// wallpaper may play.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Conditions {
    pub locked: bool,
    pub screens_off: bool,
    pub on_battery: bool,
    /// A tiled or fullscreen window covers the visible workspace.
    pub covered: bool,
    /// A fullscreen window is on the visible workspace.
    pub fullscreen: bool,
}

/// Whether a video wallpaper may play under `c`.
pub fn may_play(v: &WallpaperVideo, c: &Conditions) -> bool {
    let windows = match v.pause_when {
        VideoPause::Covered => c.covered || c.fullscreen,
        VideoPause::Fullscreen => c.fullscreen,
        VideoPause::Never => false,
    };
    !v.still_frame && !c.locked && !c.screens_off && !(v.pause_on_battery && c.on_battery) && !windows
}

/// The fill a video wallpaper uses: clips don't tile, so Tile shows the
/// clip once, centred (the poster follows the same rule).
pub fn effective_fill(path: &Path, fill: WallpaperFill) -> WallpaperFill {
    if fill == WallpaperFill::Tile && is_video(path) { WallpaperFill::Center } else { fill }
}

/// Where and at what size a clip's frames are drawn on one output.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    /// The part of the frame that is visible (frame pixels).
    pub crop: Crop,
    /// Where the plane sits.
    pub place: Place,
    /// The plane's buffer size (what the crop is converted to).
    pub buffer: (u32, u32),
}

/// How a `clip`-sized frame fills a `physical`-sized output (`scale120`
/// for the logical grid) with `fill`. With `viewport` the buffer stays at
/// the crop's resolution (halved with a box filter when the crop is at
/// least twice the destination, and never larger than the destination)
/// and the compositor scales it; without, the buffer has the destination's
/// size.
pub fn placement(fill: WallpaperFill, clip: (u32, u32), physical: (u32, u32), scale120: u32, viewport: bool) -> Placement {
    let (cw, ch) = (clip.0.max(1), clip.1.max(1));
    let (ow, oh) = (physical.0.max(1), physical.1.max(1));
    let full = Rect::new(0, 0, ow, oh);
    let centred = |w: u32, h: u32| Rect::new((ow as i32 - w as i32) / 2, (oh as i32 - h as i32) / 2, w, h);
    let even = |v: u32| v & !1;
    let (crop, dest) = match fill {
        WallpaperFill::Cover => {
            let s = (ow as f64 / cw as f64).max(oh as f64 / ch as f64);
            let (w, h) = (((ow as f64 / s).round() as u32).clamp(1, cw), ((oh as f64 / s).round() as u32).clamp(1, ch));
            (Crop { x: even((cw - w) / 2), y: (ch - h) / 2, w, h }, full)
        }
        WallpaperFill::Contain => {
            let s = (ow as f64 / cw as f64).min(oh as f64 / ch as f64);
            let (w, h) = (((cw as f64 * s).round() as u32).clamp(1, ow), ((ch as f64 * s).round() as u32).clamp(1, oh));
            (Crop { x: 0, y: 0, w: cw, h: ch }, centred(w, h))
        }
        WallpaperFill::Stretch => (Crop { x: 0, y: 0, w: cw, h: ch }, full),
        WallpaperFill::Center | WallpaperFill::Tile => {
            let (w, h) = (cw.min(ow), ch.min(oh));
            (Crop { x: even((cw - w) / 2), y: (ch - h) / 2, w, h }, centred(w, h))
        }
    };
    let dest = if dest == full { full } else { snap(dest, scale120).intersect(&full) };
    let buffer = if !viewport {
        (dest.w, dest.h)
    } else if crop.w >= dest.w * 2 && crop.h >= dest.h * 2 {
        // Box-filter down by two (the crop rounded to even), the compositor
        // does the rest.
        ((crop.w / 2).max(1), (crop.h / 2).max(1))
    } else if crop.w > dest.w || crop.h > dest.h {
        (dest.w, dest.h)
    } else {
        (crop.w, crop.h)
    };
    let crop = if viewport && buffer.0 * 2 <= crop.w && buffer.1 * 2 <= crop.h { Crop { w: buffer.0 * 2, h: buffer.1 * 2, ..crop } } else { crop };
    let place = if dest == full && buffer == (ow, oh) {
        Place::Full
    } else if buffer == (dest.w, dest.h) {
        Place::Rect(dest)
    } else {
        Place::Scaled { rect: dest, buffer }
    };
    Placement { crop, place, buffer }
}

/// The presentation clock of one clip.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Clock {
    /// Wall-clock instant of timestamp zero.
    pub origin: Option<Instant>,
    /// When the last presented frame was due (its ideal time, so timer
    /// lateness never accumulates into a slower clip).
    pub last: Option<Instant>,
}

/// What to do with the frames waiting (timestamps, oldest first).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Nothing decoded yet.
    Starved,
    /// The next frame is due then.
    Wait(Instant),
    /// Present frame `index` (dropping the older ones); `rebase` restarts
    /// the clock at it (first frame, or playback was paused / fell behind).
    Show { index: usize, rebase: bool },
}

/// Frame pacing: the newest frame whose timestamp is due, no more often
/// than `min_step` (the frame-rate cap), with late starts rebased.
pub fn plan(clock: &Clock, pts: &[Duration], min_step: Duration, now: Instant) -> Plan {
    let Some(&first) = pts.first() else { return Plan::Starved };
    let Some(origin) = clock.origin else { return Plan::Show { index: 0, rebase: true } };
    let at = |p: Duration| origin + p;
    let due = clock.last.map_or(at(first), |l| at(first).max(l + min_step));
    if now < due {
        return Plan::Wait(due);
    }
    if now.saturating_duration_since(at(first)) > LATE_REBASE {
        return Plan::Show { index: 0, rebase: true };
    }
    let index = pts.iter().rposition(|p| at(*p) <= now).unwrap_or(0);
    Plan::Show { index, rebase: false }
}

impl Clock {
    /// Records that the frame with timestamp `pts` was presented at `now`
    /// under a cap of `min_step`.
    pub fn presented(&mut self, pts: Duration, rebase: bool, min_step: Duration, now: Instant) {
        if rebase || self.origin.is_none() {
            self.origin = Some(now.checked_sub(pts).unwrap_or(now));
            self.last = None;
        }
        let ideal = self.origin.map_or(now, |o| o + pts);
        self.last = Some(self.last.map_or(ideal, |l| ideal.max(l + min_step)).min(now));
    }
}

/// Threads for converting a `w × h` buffer.
pub fn convert_threads(w: u32, h: u32) -> usize {
    if u64::from(w) * u64::from(h) <= PARALLEL_CONVERT_PIXELS {
        return 1;
    }
    static HALF_THE_CORES: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *HALF_THE_CORES.get_or_init(|| std::thread::available_parallelism().map(|n| n.get() / 2).unwrap_or(1).clamp(1, MAX_CONVERT_THREADS))
}

/// Shortest interval between two presented frames for a cap of `fps`.
pub fn min_step(fps: u32) -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(fps.max(1)))
}

// ── Renderer state ───────────────────────────────────────────────────────

/// One output showing a clip: what it should paint now.
pub struct Consumer {
    /// The output (`wl_output` global) and the wallpaper plane it paints.
    pub id: u32,
    pub plane: usize,
    pub physical: (u32, u32),
    pub scale120: u32,
    pub fill: WallpaperFill,
    /// Shell rules allow playback here ([`may_play`]).
    pub allowed: bool,
}

/// What an output last painted of its clip.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutVideo {
    pub path: Option<PathBuf>,
    /// The wallpaper plane it painted (a re-shown clip lands on the other).
    pub plane: usize,
    pub serial: u64,
    /// The plane currently keeps spare buffers (playing).
    pub keeping: bool,
}

struct Clip {
    stream: Option<FrameStream>,
    /// Decoded, not yet presented (oldest first).
    queue: VecDeque<StreamFrame>,
    /// The frame on screen, kept for outputs that catch up.
    current: Option<StreamFrame>,
    /// Bumped per presented frame.
    serial: u64,
    clock: Clock,
    /// No output could take frames since then.
    idle_since: Option<Instant>,
    /// When to run again for this clip (`None`: an event will wake us).
    wake: Option<Instant>,
    /// The stream failed: stay on the poster.
    failed: bool,
}

impl Clip {
    fn new() -> Self {
        Self { stream: None, queue: VecDeque::with_capacity(LOOKAHEAD), current: None, serial: 0, clock: Clock::default(), idle_since: None, wake: None, failed: false }
    }

    /// Drops the decoder and its pictures (memory back to a still's).
    fn release(&mut self) {
        self.stream = None;
        self.queue.clear();
        self.current = None;
        self.clock = Clock::default();
        self.idle_since = None;
        self.wake = None;
    }
}

/// Every clip being shown.
#[derive(Default)]
pub struct Videos {
    clips: HashMap<PathBuf, Clip>,
}

impl Videos {
    /// Runs playback for every output showing a clip (`shown`: path →
    /// consumers; outputs mid-transition are left out by the caller and
    /// keep their frame). `outs` gives each output's painted state.
    pub fn step(&mut self, host: &mut Host, shown: &[(PathBuf, Vec<Consumer>)], states: &mut HashMap<u32, OutVideo>, fps_cap: u32, waker: &Arc<Waker>, now: Instant) {
        self.clips.retain(|path, _| shown.iter().any(|(p, _)| p == path));
        for (path, consumers) in shown {
            let clip = self.clips.entry(path.clone()).or_insert_with(Clip::new);
            clip.wake = None;
            let allowed: Vec<&Consumer> = consumers.iter().filter(|c| c.allowed).collect();
            // Outputs shell rules stop keep their frame, and their spare
            // buffers go.
            for c in consumers.iter().filter(|c| !c.allowed) {
                if let Some(s) = states.get_mut(&c.id).filter(|s| s.keeping) {
                    host.set_keep(c.id, c.plane, false);
                    s.keeping = false;
                }
            }
            // Ready: allowed and not waiting for the compositor.
            let ready: Vec<&Consumer> = allowed.iter().copied().filter(|c| host.awaiting_frame(c.id).is_none()).collect();
            if allowed.is_empty() || clip.failed {
                let since = *clip.idle_since.get_or_insert(now);
                if clip.stream.is_some() {
                    if now.saturating_duration_since(since) >= PARK_RELEASE {
                        clip.release();
                    } else {
                        clip.wake = Some(since + PARK_RELEASE);
                    }
                }
                continue;
            }
            if clip.stream.is_none() {
                let w = waker.clone();
                clip.stream = Some(FrameStream::spawn(
                    StreamOptions { path: path.clone(), threads: DECODER_THREADS, frame_delay: DECODER_FRAME_DELAY, lite: false, queue: mandara_media::player::STREAM_QUEUE },
                    move || w.wake(),
                ));
            }
            // Outputs that missed the frame on screen catch up first.
            if let Some(cur) = clip.current.clone() {
                for c in &ready {
                    if states.get(&c.id).is_none_or(|s| s.path.as_deref() != Some(path) || s.plane != c.plane || s.serial != clip.serial) {
                        present(host, c, &cur, path, clip.serial, states);
                    }
                }
            }
            let ready: Vec<&Consumer> = ready.into_iter().filter(|c| host.awaiting_frame(c.id).is_none()).collect();
            if ready.is_empty() {
                // Woken by the frame callback — or, on an output the
                // compositor no longer draws, not at all until the decoder
                // is released [`PARK_RELEASE`] after the last frame.
                let since = *clip.idle_since.get_or_insert(clip.clock.last.unwrap_or(now));
                if now.saturating_duration_since(since) >= PARK_RELEASE {
                    clip.release();
                } else {
                    clip.wake = Some(since + PARK_RELEASE);
                }
                continue;
            }
            clip.idle_since = None;
            if !pull(clip) {
                continue;
            }
            let pts: Vec<Duration> = clip.queue.iter().map(|f| f.pts).collect();
            match plan(&clip.clock, &pts, min_step(fps_cap), now) {
                Plan::Starved => {
                    // Woken by the decoder once a picture is queued.
                    if let Some(StreamEvent::Frame(f)) = clip.stream.as_ref().and_then(|s| s.wake_on_next()) {
                        clip.queue.push_back(f);
                        clip.wake = Some(now);
                    }
                }
                Plan::Wait(at) => clip.wake = Some(at),
                Plan::Show { index, rebase } => {
                    clip.queue.drain(..index);
                    let Some(frame) = clip.queue.pop_front() else { continue };
                    clip.clock.presented(frame.pts, rebase, min_step(fps_cap), now);
                    clip.serial += 1;
                    for c in &ready {
                        present(host, c, &frame, path, clip.serial, states);
                    }
                    clip.current = Some(frame);
                    // Refill; the next due time is planned once the frame
                    // callbacks are back (this only covers a compositor
                    // that never sends them: the decoder is let go).
                    pull(clip);
                    clip.wake = Some(now + PARK_RELEASE);
                }
            }
        }
    }

    /// The earliest moment any clip needs the renderer again.
    pub fn next_wake(&self, now: Instant) -> Option<Duration> {
        self.clips.values().filter_map(|c| c.wake).min().map(|t| t.saturating_duration_since(now))
    }

    /// No clip is shown (nothing to do, nothing allocated).
    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }
}

/// Moves decoded pictures into the clip's lookahead; `false` when the
/// stream failed (the poster stays).
fn pull(clip: &mut Clip) -> bool {
    let Some(stream) = clip.stream.as_ref() else { return false };
    while clip.queue.len() < LOOKAHEAD {
        match stream.try_recv() {
            Some(StreamEvent::Frame(f)) => clip.queue.push_back(f),
            Some(StreamEvent::Info(_)) => {}
            Some(StreamEvent::Error(e)) => {
                tracing::warn!("video wallpaper: {e}");
                clip.failed = true;
                clip.release();
                return false;
            }
            None => break,
        }
    }
    true
}

/// Converts `frame` into consumer `c`'s wallpaper plane and commits it.
fn present(host: &mut Host, c: &Consumer, frame: &StreamFrame, path: &Path, serial: u64, states: &mut HashMap<u32, OutVideo>) {
    let pic = &frame.picture;
    let p = placement(c.fill, (pic.width() as u32, pic.height() as u32), c.physical, c.scale120, host.supports_viewport());
    let threads = convert_threads(p.buffer.0, p.buffer.1);
    let painted = host.paint_pixels(c.id, c.plane, p.place, true, |data, w, h, bgra| {
        let mut dst = Target { data, width: w, height: h, stride: w as usize * 4, order: if bgra { PixelOrder::Bgra } else { PixelOrder::Rgba } };
        pic.convert_region(p.crop, &mut dst, threads);
    });
    match painted {
        Ok(true) => {
            if let Err(err) = host.commit(c.id, true) {
                tracing::warn!("video wallpaper: {err:#}");
            }
            states.insert(c.id, OutVideo { path: Some(path.to_path_buf()), plane: c.plane, serial, keeping: true });
        }
        // Every buffer still with the compositor: this output skips a frame.
        Ok(false) => {}
        Err(err) => tracing::warn!("video wallpaper: {err:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S120: u32 = 120;

    fn cond() -> Conditions {
        Conditions::default()
    }

    #[test]
    fn pause_rules() {
        let v = WallpaperVideo::default();
        assert!(may_play(&v, &cond()));
        assert!(!may_play(&v, &Conditions { locked: true, ..cond() }));
        assert!(!may_play(&v, &Conditions { screens_off: true, ..cond() }));
        assert!(!may_play(&v, &Conditions { on_battery: true, ..cond() }));
        assert!(may_play(&WallpaperVideo { pause_on_battery: false, ..v.clone() }, &Conditions { on_battery: true, ..cond() }));
        // Covered: tiled or fullscreen windows stop it by default…
        assert!(!may_play(&v, &Conditions { covered: true, ..cond() }));
        assert!(!may_play(&v, &Conditions { fullscreen: true, ..cond() }));
        // …only fullscreen ones with `fullscreen`, none with `never`.
        let fs = WallpaperVideo { pause_when: VideoPause::Fullscreen, ..v.clone() };
        assert!(may_play(&fs, &Conditions { covered: true, ..cond() }));
        assert!(!may_play(&fs, &Conditions { fullscreen: true, ..cond() }));
        let never = WallpaperVideo { pause_when: VideoPause::Never, ..v.clone() };
        assert!(may_play(&never, &Conditions { covered: true, fullscreen: true, ..cond() }));
        assert!(!may_play(&never, &Conditions { locked: true, ..cond() }), "locked always stops");
        assert!(!may_play(&WallpaperVideo { still_frame: true, ..v }, &cond()));
    }

    #[test]
    fn videos_never_tile() {
        assert_eq!(effective_fill(Path::new("/w/a.webm"), WallpaperFill::Tile), WallpaperFill::Center);
        assert_eq!(effective_fill(Path::new("/w/a.webm"), WallpaperFill::Cover), WallpaperFill::Cover);
        assert_eq!(effective_fill(Path::new("/w/a.png"), WallpaperFill::Tile), WallpaperFill::Tile);
    }

    #[test]
    fn cover_same_aspect_keeps_the_clip_resolution() {
        // 1080p clip on a 1440p output: the compositor upscales.
        let p = placement(WallpaperFill::Cover, (1920, 1080), (2560, 1440), S120, true);
        assert_eq!(p.crop, Crop { x: 0, y: 0, w: 1920, h: 1080 });
        assert_eq!(p.buffer, (1920, 1080));
        assert_eq!(p.place, Place::Scaled { rect: Rect::new(0, 0, 2560, 1440), buffer: (1920, 1080) });
        // Same size: drawn 1:1 over the whole output.
        let p = placement(WallpaperFill::Cover, (2560, 1440), (2560, 1440), S120, true);
        assert_eq!((p.place, p.buffer), (Place::Full, (2560, 1440)));
    }

    #[test]
    fn cover_crops_the_overflow_centred_on_an_even_column() {
        // 16:9 clip on a 16:10 output: the sides are cropped.
        let p = placement(WallpaperFill::Cover, (1920, 1080), (1920, 1200), S120, true);
        assert_eq!(p.crop, Crop { x: 96, y: 0, w: 1728, h: 1080 });
        assert_eq!(p.place, Place::Scaled { rect: Rect::new(0, 0, 1920, 1200), buffer: (1728, 1080) });
        // Odd offsets round down to even (4:2:0 chroma pairs).
        let p = placement(WallpaperFill::Cover, (1921, 1080), (1920, 1200), S120, true);
        assert_eq!(p.crop.x % 2, 0);
        // Ultrawide output: top and bottom cropped.
        let p = placement(WallpaperFill::Cover, (1920, 1080), (3440, 1440), S120, true);
        assert_eq!((p.crop.w, p.crop.h, p.crop.y), (1920, 804, 138));
    }

    #[test]
    fn big_clips_are_halved_then_capped_at_the_destination() {
        // 4K clip on a 1080p output: 2× box filter to 1080p, drawn 1:1.
        let p = placement(WallpaperFill::Cover, (3840, 2160), (1920, 1080), S120, true);
        assert_eq!((p.crop, p.buffer, p.place), (Crop { x: 0, y: 0, w: 3840, h: 2160 }, (1920, 1080), Place::Full));
        assert_eq!(mandara_media::yuv::sampling(p.crop, p.buffer.0, p.buffer.1), mandara_media::Sampling::Half);
        // 4K on 1440p: under 2×, converted straight to the output size.
        let p = placement(WallpaperFill::Cover, (3840, 2160), (2560, 1440), S120, true);
        assert_eq!((p.buffer, p.place), ((2560, 1440), Place::Full));
    }

    #[test]
    fn contain_letterboxes_on_the_logical_grid() {
        // 16:9 clip on 16:10 at 1.5×: bands top and bottom, rect snapped
        // to the 3 px grid.
        let p = placement(WallpaperFill::Contain, (1920, 1080), (2880, 1800), 180, true);
        let Place::Scaled { rect, buffer } = p.place else { panic!("{:?}", p.place) };
        assert_eq!(buffer, (1920, 1080));
        assert_eq!((rect.x, rect.w), (0, 2880));
        assert!(rect.y % 3 == 0 && rect.h % 3 == 0, "{rect:?}");
        assert!(rect.y <= 90 && rect.bottom() >= 1710, "covers the picture: {rect:?}");
    }

    #[test]
    fn center_and_stretch() {
        // A small clip sits in the middle at its own size.
        let p = placement(WallpaperFill::Center, (1280, 720), (1920, 1080), S120, true);
        assert_eq!(p.place, Place::Rect(Rect::new(320, 180, 1280, 720)));
        // A big one is cropped to the output.
        let p = placement(WallpaperFill::Center, (3840, 2160), (1920, 1080), S120, true);
        assert_eq!((p.crop, p.place), (Crop { x: 960, y: 540, w: 1920, h: 1080 }, Place::Full));
        let p = placement(WallpaperFill::Stretch, (1280, 1024), (1920, 1080), S120, true);
        assert_eq!(p.place, Place::Scaled { rect: Rect::new(0, 0, 1920, 1080), buffer: (1280, 1024) });
    }

    #[test]
    fn without_viewporter_the_buffer_matches_the_destination() {
        let p = placement(WallpaperFill::Cover, (1920, 1080), (2560, 1440), S120, false);
        assert_eq!((p.buffer, p.place), ((2560, 1440), Place::Full));
        assert_eq!(mandara_media::yuv::sampling(p.crop, p.buffer.0, p.buffer.1), mandara_media::Sampling::Nearest);
    }

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    #[test]
    fn pacing_shows_frames_on_time_and_caps_the_rate() {
        let t0 = Instant::now();
        let mut clock = Clock::default();
        // First frame: shown at once, clock starts.
        assert_eq!(plan(&clock, &[ms(0), ms(33)], min_step(30), t0), Plan::Show { index: 0, rebase: true });
        clock.presented(ms(0), true, min_step(30), t0);
        // Next one waits for its timestamp (and the cap).
        let step = min_step(30);
        assert_eq!(plan(&clock, &[step], step, t0 + ms(10)), Plan::Wait(t0 + step));
        assert_eq!(plan(&clock, &[step], step, t0 + ms(34)), Plan::Show { index: 0, rebase: false });
        // A late timer doesn't slow the clip: the next frame is due one
        // step after the last one was *due*, not after it was shown.
        clock.presented(step, false, step, t0 + ms(36));
        assert_eq!(plan(&clock, &[step * 2], step, t0 + ms(40)), Plan::Wait(t0 + step * 2));
        // A 60 fps clip capped at 30: the in-between frame is dropped.
        let mut c60 = Clock::default();
        c60.presented(ms(0), true, min_step(30), t0);
        let pts = [ms(17), ms(33)];
        assert_eq!(plan(&c60, &pts, min_step(30), t0 + ms(20)), Plan::Wait(t0 + min_step(30)));
        assert_eq!(plan(&c60, &pts, min_step(30), t0 + ms(34)), Plan::Show { index: 1, rebase: false });
        // Nothing decoded yet.
        assert_eq!(plan(&clock, &[], min_step(30), t0), Plan::Starved);
    }

    #[test]
    fn pacing_rebases_after_a_pause_instead_of_racing() {
        let t0 = Instant::now();
        let mut clock = Clock::default();
        clock.presented(ms(0), true, min_step(30), t0);
        // Paused (covered) for 10 s: the next frame plays now, not 300 at once.
        let later = t0 + Duration::from_secs(10);
        assert_eq!(plan(&clock, &[ms(33), ms(67)], min_step(30), later), Plan::Show { index: 0, rebase: true });
        clock.presented(ms(33), true, min_step(30), later);
        assert_eq!(plan(&clock, &[ms(67)], min_step(30), later + ms(5)), Plan::Wait(later + min_step(30).max(ms(34))));
        // Seamless loop: timestamps keep counting past the clip length.
        let mut c = Clock::default();
        c.presented(Duration::from_secs(10), true, min_step(30), t0);
        assert_eq!(plan(&c, &[Duration::from_secs(10) + ms(33)], min_step(30), t0 + ms(40)), Plan::Show { index: 0, rebase: false });
    }

    #[test]
    fn conversion_threads_scale_with_the_buffer() {
        assert_eq!(convert_threads(1920, 1080), 1);
        assert_eq!(convert_threads(2560, 1440), 1);
        assert!((1..=MAX_CONVERT_THREADS).contains(&convert_threads(3840, 2160)));
        assert_eq!(min_step(0), Duration::from_secs(1));
    }
}
