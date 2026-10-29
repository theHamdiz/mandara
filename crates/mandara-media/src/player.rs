//! Threaded playback, two flavours sharing one decode loop.
//!
//! * [`Player`] — demux → AV1 decode → BGRA → a frame type of the caller's
//!   choosing (the greeter makes GPUI `RenderImage`s), pushed through a
//!   small bounded ring ([`FRAME_QUEUE`] frames). Decoding and conversion
//!   run on two threads so they overlap. Frames the UI has finished with
//!   travel back to the converter thread ([`Player::recycle`]), which frees
//!   them: multi-megabyte frees never run on the UI thread.
//! * [`FrameStream`] — demux → AV1 decode only: shared, reference-counted
//!   YUV [`Picture`]s for a consumer that converts them itself straight into
//!   its own buffers (the desktop background converts each picture directly
//!   into every output's shared-memory buffer, scaled for its fill mode).
//!
//! Both loop seamlessly (timestamps keep counting across passes) and give
//! back-pressure: the decoder blocks — no polling, no wake-ups — once it is
//! a full queue ahead. A consumer that stops reading therefore parks the
//! decoder; dropping the handle stops it.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::{Duration, Instant};

use super::av1::{Av1Decoder, DecoderSettings, Picture};
use super::{ClipInfo, Demuxer};

/// Converted frames buffered ahead of the UI (each is a full BGRA frame,
/// so this bounds the player's memory).
pub const FRAME_QUEUE: usize = 3;
/// Smallest usable ring: the UI holds one frame while the next is queued.
const MIN_QUEUE: usize = 2;
/// Decoded pictures waiting for conversion (decode overlaps conversion).
const PICTURE_QUEUE: usize = 2;
/// Most AV1 worker threads for a greeter clip; half the cores at most, so
/// the compositor and the UI thread always have room.
const MAX_DECODER_THREADS: usize = 8;
/// Frame step assumed when a clip has neither a nominal rate nor two frames.
pub const FALLBACK_FRAME_STEP: Duration = Duration::from_millis(33);

/// Decoder worker threads for this machine (greeter player).
fn decoder_threads() -> usize {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(MAX_DECODER_THREADS);
    (cores / 2).clamp(1, MAX_DECODER_THREADS)
}

/// A frame ready to paint.
#[derive(Clone)]
pub struct VideoFrame<T> {
    /// Presentation time from the start of playback (loops keep counting).
    pub pts: Duration,
    pub image: T,
    pub width: u32,
    pub height: u32,
}

pub enum PlayerEvent<T> {
    Info(ClipInfo),
    Frame(VideoFrame<T>),
    /// The clip finished (only when not looping forever).
    End,
    Error(String),
}

#[derive(Clone, Debug)]
pub struct PlayerOptions {
    pub path: PathBuf,
    /// The output's size in physical pixels; frames much larger than this
    /// are converted at half resolution.
    pub target: (u32, u32),
    /// Lite decoding (see [`DecoderSettings::lite`]) and half-resolution
    /// conversion for anything above 1080p.
    pub lite: bool,
    /// Loop until at least this much has played, then end; `None` loops
    /// forever.
    pub play_for: Option<Duration>,
    /// Frames buffered ahead of the UI.
    pub queue: usize,
}

/// Counters shared with the UI (diagnostics and the slow-machine fallback).
#[derive(Default, Debug)]
pub struct PlayerStats {
    pub frames: AtomicU64,
    /// Total decode + convert time, microseconds.
    pub busy_us: AtomicU64,
}

impl PlayerStats {
    /// Average decode + convert time per frame.
    pub fn frame_cost(&self) -> Duration {
        let n = self.frames.load(Ordering::Relaxed).max(1);
        Duration::from_micros(self.busy_us.load(Ordering::Relaxed) / n)
    }
}

/// Turns a converted BGRA frame (`pixels, width, height`) into the frame
/// type the UI paints; `None` skips the frame.
pub type MakeFrame<T> = Box<dyn FnMut(Vec<u8>, u32, u32) -> Option<T> + Send>;

pub struct Player<T> {
    rx: Receiver<PlayerEvent<T>>,
    /// Frames the UI is done with, freed on the converter thread.
    recycle: SyncSender<T>,
    stop: Arc<AtomicBool>,
    pub stats: Arc<PlayerStats>,
    pub path: PathBuf,
}

impl<T: Send + 'static> Player<T> {
    /// Starts decoding `opts.path`; `make` wraps each converted BGRA frame.
    pub fn spawn(opts: PlayerOptions, make: impl FnMut(Vec<u8>, u32, u32) -> Option<T> + Send + 'static) -> Player<T> {
        let make: MakeFrame<T> = Box::new(make);
        let queue = opts.queue.max(MIN_QUEUE);
        let (tx, rx) = sync_channel(queue);
        // Room for every frame that can be in flight at once.
        let (recycle, recycle_rx) = sync_channel(queue + PICTURE_QUEUE + MIN_QUEUE);
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(PlayerStats::default());
        let path = opts.path.clone();
        let (s, st) = (stop.clone(), stats.clone());
        let spawned = std::thread::Builder::new().name("mandara-video".into()).spawn(move || {
            if let Err(e) = run(opts, make, &tx, &recycle_rx, &s, &st) {
                let _ = tx.try_send(PlayerEvent::Error(e));
            }
        });
        if let Err(e) = spawned {
            tracing::error!("video thread: {e}");
        }
        Player { rx, recycle, stop, stats, path }
    }
}

impl<T> Player<T> {
    pub fn try_recv(&self) -> Option<PlayerEvent<T>> {
        self.rx.try_recv().ok()
    }

    /// Hands a frame the UI no longer paints back to the converter thread
    /// to free; frees it here when the thread is gone or behind.
    pub fn recycle(&self, image: T) {
        let _ = self.recycle.try_send(image);
    }
}

impl<T> Drop for Player<T> {
    fn drop(&mut self) {
        // The thread notices on its next send and exits; queued frames are
        // freed with the channel.
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Sends, blocking while the ring is full; `false` means the player was
/// dropped (its receiver is gone, which also wakes a blocked send).
fn send<T>(tx: &SyncSender<T>, stop: &AtomicBool, ev: T) -> bool {
    !stop.load(Ordering::Relaxed) && tx.send(ev).is_ok()
}

/// Whether frames of `w × h` should be converted at half resolution.
pub fn use_half(w: u32, h: u32, target: (u32, u32), lite: bool) -> bool {
    let big = target.0 > 0 && target.1 > 0 && w / 2 >= target.0 * 9 / 10 && h / 2 >= target.1 * 9 / 10;
    big || (lite && w > 1920)
}

/// Decode thread → its consumer.
enum Decoded {
    Picture { pic: Picture, pts: Duration, busy: Duration },
    End,
}

/// Two stages so decoding the next frame overlaps converting this one:
/// a decode thread (demux + rav1d, which runs its own worker pool) feeds
/// pictures through a 2-slot channel to this thread, which converts them
/// to BGRA frames for the UI queue.
fn run<T>(opts: PlayerOptions, mut make: MakeFrame<T>, tx: &SyncSender<PlayerEvent<T>>, recycle: &Receiver<T>, stop: &AtomicBool, stats: &PlayerStats) -> Result<(), String> {
    let name = opts.path.display().to_string();
    let demux = Demuxer::open(&opts.path).map_err(|e| format!("{name}: {e}"))?;
    let info = demux.info();
    if !send(tx, stop, PlayerEvent::Info(info.clone())) {
        return Ok(());
    }
    let (ptx, prx) = sync_channel::<Decoded>(PICTURE_QUEUE);
    std::thread::scope(|scope| {
        let settings = DecoderSettings { threads: decoder_threads(), lite: opts.lite, ..DecoderSettings::default() };
        let (play_for, name_ref) = (opts.play_for, &name);
        let decoder = scope.spawn(move || decode_loop(demux, info, settings, play_for, name_ref, stop, |d| send(&ptx, stop, d)));
        let mut half: Option<bool> = None;
        let mut ok = true;
        while let Ok(msg) = prx.recv() {
            match msg {
                Decoded::Picture { pic, pts, busy } => {
                    // Free what the UI finished with before allocating anew.
                    recycle.try_iter().for_each(drop);
                    let t = Instant::now();
                    let h = *half.get_or_insert_with(|| use_half(pic.width() as u32, pic.height() as u32, opts.target, opts.lite));
                    let Some((bgra, w, hgt)) = pic.to_bgra(h) else { continue };
                    drop(pic);
                    let Some(image) = make(bgra, w as u32, hgt as u32) else { continue };
                    stats.frames.fetch_add(1, Ordering::Relaxed);
                    stats.busy_us.fetch_add((busy + t.elapsed()).as_micros() as u64, Ordering::Relaxed);
                    if !send(tx, stop, PlayerEvent::Frame(VideoFrame { pts, image, width: w as u32, height: hgt as u32 })) {
                        ok = false;
                        break;
                    }
                }
                Decoded::End => {
                    send(tx, stop, PlayerEvent::End);
                    break;
                }
            }
        }
        if !ok {
            stop.store(true, Ordering::SeqCst);
        }
        drop(prx);
        // An error from the decoder ends the loop above (channel closed).
        decoder.join().map_err(|_| format!("{name}: decoder thread crashed"))?
    })
}

/// Demuxes and decodes `demux` in a loop, handing every picture (with its
/// looping timestamp) to `emit`; `emit` returning `false` stops it.
fn decode_loop(
    mut demux: Demuxer,
    info: ClipInfo,
    settings: DecoderSettings,
    play_for: Option<Duration>,
    name: &str,
    stop: &AtomicBool,
    mut emit: impl FnMut(Decoded) -> bool,
) -> Result<(), String> {
    let nominal = (info.fps > 0.0).then(|| Duration::from_secs_f32(1.0 / info.fps));
    // Created here: the context must stay on the thread that uses it.
    let mut dec = Av1Decoder::new(settings)?;
    if !info.codec_config.is_empty() {
        // Only sequence headers: never yields pictures.
        let _ = dec.decode(&info.codec_config, Duration::ZERO);
    }
    let mut offset = Duration::ZERO;
    let mut first: Option<Duration> = None;
    let mut last = Duration::ZERO;
    // Frames in the current pass (to estimate the frame step when the
    // container has no nominal rate).
    let mut pass_frames = 0u32;
    let mut forward = |pics: Vec<Picture>, busy: Duration, offset: Duration, first: &mut Option<Duration>, last: &mut Duration| -> bool {
        let n = pics.len().max(1) as u32;
        for pic in pics {
            let raw = pic.pts();
            let base = *first.get_or_insert(raw);
            *last = (*last).max(raw.saturating_sub(base));
            if !emit(Decoded::Picture { pts: raw.saturating_sub(base) + offset, pic, busy: busy / n }) {
                return false;
            }
        }
        true
    };
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let packet = match demux.next_packet() {
            Ok(p) => p,
            // A truncated tail ends the clip like EOF.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => None,
            Err(e) => return Err(format!("{name}: {e}")),
        };
        let at_end = packet.is_none();
        let t = Instant::now();
        let pics = match &packet {
            Some(p) => dec.decode(&p.data, p.pts)?,
            None => dec.drain()?,
        };
        let busy = t.elapsed();
        pass_frames += pics.len() as u32;
        if !forward(pics, busy, offset, &mut first, &mut last) {
            return Ok(());
        }
        if !at_end {
            continue;
        }
        if first.is_none() {
            return Err(format!("{name}: no decodable frames"));
        }
        let step = nominal.unwrap_or_else(|| if pass_frames > 1 { last / (pass_frames - 1) } else { FALLBACK_FRAME_STEP });
        let clip = last + step;
        pass_frames = 0;
        let again = match play_for {
            None => true,
            Some(min) => offset + clip < min,
        };
        if !again {
            emit(Decoded::End);
            return Ok(());
        }
        offset += clip;
        demux.rewind().map_err(|e| format!("{name}: {e}"))?;
    }
}

// ── Picture stream (no conversion) ───────────────────────────────────────

/// Decoded pictures queued ahead of a [`FrameStream`] consumer. Two keep
/// the decoder one frame ahead while the consumer converts the current
/// one; each is one YUV picture (≈ 3 MiB at 1080p, 8-bit 4:2:0).
pub const STREAM_QUEUE: usize = 2;

#[derive(Clone, Debug)]
pub struct StreamOptions {
    pub path: PathBuf,
    /// AV1 worker threads (frame and tile threads together).
    pub threads: usize,
    /// Frames decoded in parallel (0 = rav1d's automatic choice). Each one
    /// in flight costs a full picture of memory.
    pub frame_delay: u32,
    /// Lite decoding (see [`DecoderSettings::lite`]).
    pub lite: bool,
    /// Pictures queued ahead of the consumer.
    pub queue: usize,
}

/// One decoded picture with its looping presentation time.
#[derive(Clone)]
pub struct StreamFrame {
    /// From the start of playback; keeps counting across loops.
    pub pts: Duration,
    pub picture: Arc<Picture>,
}

pub enum StreamEvent {
    Info(ClipInfo),
    Frame(StreamFrame),
    Error(String),
}

/// A clip decoded on its own thread, looping forever, into shared YUV
/// pictures. Reading nothing parks the decoder (blocked on the full queue:
/// no CPU, no wake-ups); dropping the stream ends the thread.
pub struct FrameStream {
    rx: Receiver<StreamEvent>,
    stop: Arc<AtomicBool>,
    /// The consumer found the queue empty and wants `notify` called once
    /// the next picture is queued.
    hungry: Arc<AtomicBool>,
    pub path: PathBuf,
}

impl FrameStream {
    /// Starts decoding; `notify` runs on the decode thread after a picture
    /// is queued for a consumer that asked with [`FrameStream::wake_on_next`].
    pub fn spawn(opts: StreamOptions, notify: impl Fn() + Send + Sync + 'static) -> FrameStream {
        let (tx, rx) = sync_channel(opts.queue.max(1));
        let stop = Arc::new(AtomicBool::new(false));
        let hungry = Arc::new(AtomicBool::new(false));
        let path = opts.path.clone();
        let (s, h) = (stop.clone(), hungry.clone());
        let spawned = std::thread::Builder::new().name("mandara-wp-video".into()).spawn(move || {
            let name = opts.path.display().to_string();
            let result = (|| {
                let demux = Demuxer::open(&opts.path).map_err(|e| format!("{name}: {e}"))?;
                let info = demux.info();
                if !send(&tx, &s, StreamEvent::Info(info.clone())) {
                    return Ok(());
                }
                let settings = DecoderSettings { threads: opts.threads.max(1), lite: opts.lite, frame_delay: opts.frame_delay };
                decode_loop(demux, info, settings, None, &name, &s, |d| match d {
                    Decoded::Picture { pic, pts, .. } => {
                        let ok = send(&tx, &s, StreamEvent::Frame(StreamFrame { pts, picture: Arc::new(pic) }));
                        if ok && h.swap(false, Ordering::SeqCst) {
                            notify();
                        }
                        ok
                    }
                    Decoded::End => true,
                })
            })();
            if let Err(e) = result {
                let _ = tx.try_send(StreamEvent::Error(e));
                if h.swap(false, Ordering::SeqCst) {
                    notify();
                }
            }
        });
        if let Err(e) = spawned {
            tracing::error!("wallpaper video thread: {e}");
        }
        FrameStream { rx, stop, hungry, path }
    }

    pub fn try_recv(&self) -> Option<StreamEvent> {
        self.rx.try_recv().ok()
    }

    /// Asks for `notify` once the next picture is queued, then looks again:
    /// returns what is queued now (no wake-up needed then), avoiding a lost
    /// wake-up between an empty [`FrameStream::try_recv`] and the request.
    pub fn wake_on_next(&self) -> Option<StreamEvent> {
        self.hungry.store(true, Ordering::SeqCst);
        let ev = self.rx.try_recv().ok();
        if ev.is_some() {
            self.hungry.store(false, Ordering::SeqCst);
        }
        ev
    }
}

impl Drop for FrameStream {
    fn drop(&mut self) {
        // A decoder blocked on the full queue wakes when the receiver is
        // dropped with the stream; one mid-decode sees the flag.
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_resolution_policy() {
        // 4K clip on a 1080p output: half.
        assert!(use_half(3840, 2160, (1920, 1080), false));
        // 4K clip on a 4K output: full, unless lite.
        assert!(!use_half(3840, 2160, (3840, 2160), false));
        assert!(use_half(3840, 2160, (3840, 2160), true));
        // 1080p clip never halves.
        assert!(!use_half(1920, 1080, (1280, 720), true));
    }
}
