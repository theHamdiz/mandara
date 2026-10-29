//! Screen recording, pure Rust end to end: frames come from a persistent
//! `zwlr_screencopy_v1` session (whole output or a region), are converted
//! to BT.709 YUV 4:2:0, encoded to AV1 with rav1e, and muxed into WebM by
//! the small writer below. Timestamps are the real capture times, so the
//! file stays in sync even when frames are dropped under load.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mandara_i18n::{t, tf};
use rav1e::prelude::*;

use crate::screencopy::FrameSource;

/// The encoder thread must report ready within this (failure detection).
const READY_TIMEOUT: Duration = Duration::from_secs(5);
/// Frame queue wait so the encoder loop notices a stop request promptly.
const FRAME_WAIT: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, PartialEq)]
pub struct RecordOptions {
    /// Output name (xdg-output), empty for the first output.
    pub output: String,
    /// Logical region relative to the output (`x, y, w, h`), or the whole output.
    pub region: Option<(i32, i32, i32, i32)>,
    pub fps: u32,
    /// Encoded frames are scaled down to at most this width.
    pub max_width: u32,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordingResult {
    pub path: PathBuf,
    pub duration: Duration,
    pub frames: u64,
    pub dropped: u64,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
}

/// A running recording; [`Recording::stop`] finishes and returns the file.
pub struct Recording {
    stop: Arc<AtomicBool>,
    frames: Arc<AtomicU64>,
    thread: Option<std::thread::JoinHandle<Result<RecordingResult, String>>>,
    pub started: Instant,
    pub path: PathBuf,
}

impl Recording {
    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    pub fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Stops capturing, flushes the encoder and finalizes the file.
    pub fn stop(mut self) -> Result<RecordingResult, String> {
        self.stop.store(true, Ordering::SeqCst);
        match self.thread.take() {
            Some(thread) => thread.join().map_err(|_| t("The recording thread crashed").to_string())?,
            None => Err(t("The recording already finished").into()),
        }
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// One frame ready for the encoder.
struct Yuv {
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    at: Duration,
}

/// Even dimensions no wider than `max_w`, keeping the aspect ratio.
pub fn encode_size(w: u32, h: u32, max_w: u32) -> (u32, u32) {
    let (w2, h2) = if w > max_w { (max_w, (h as u64 * max_w as u64 / w as u64) as u32) } else { (w, h) };
    ((w2 & !1).max(2), (h2 & !1).max(2))
}

/// Converts a raw screencopy frame (BGRx / RGBx byte order) to BT.709
/// limited-range YUV 4:2:0 at `dw × dh` (`dw`, `dh` even), averaging a 2×2
/// sample of each source box when shrinking. One pass, split into bands of
/// two luma rows across all cores.
#[allow(clippy::too_many_arguments)]
pub fn to_yuv420(data: &[u8], sw: u32, sh: u32, stride: u32, bgr: bool, y_invert: bool, dw: u32, dh: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (sw, sh, stride, dw, dh) = (sw as usize, sh as usize, stride as usize, dw as usize, dh as usize);
    let (cw, ch) = (dw / 2, dh / 2);
    let (ri, bi) = if bgr { (2, 0) } else { (0, 2) };
    let mut yp = vec![0u8; dw * dh];
    let mut up = vec![0u8; cw * ch];
    let mut vp = vec![0u8; cw * ch];
    // Source column offsets for each destination column (two taps).
    let cols: Vec<(usize, usize)> = (0..dw)
        .map(|x| {
            let a = x * sw / dw;
            let b = ((x + 1) * sw / dw).max(a + 1).min(sw);
            (a * 4, ((a + b) / 2).min(sw - 1) * 4)
        })
        .collect();
    let row_of = |y: usize| -> (usize, usize) {
        let a = y * sh / dh;
        let b = ((y + 1) * sh / dh).max(a + 1).min(sh);
        let m = ((a + b) / 2).min(sh - 1);
        let flip = |r: usize| if y_invert { sh - 1 - r } else { r };
        (flip(a) * stride, flip(m) * stride)
    };
    let px = |row: usize, col: usize| -> (u32, u32, u32) {
        let i = row + col;
        match data.get(i..i + 3) {
            Some(p) => (p[ri] as u32, p[1] as u32, p[bi] as u32),
            None => (0, 0, 0),
        }
    };
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 16);
    let band = ch.div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let ys = yp.chunks_mut(dw * 2 * band);
        let us = up.chunks_mut(cw * band);
        let vs = vp.chunks_mut(cw * band);
        for (bi_, ((ychunk, uchunk), vchunk)) in ys.zip(us).zip(vs).enumerate() {
            let (cols, row_of, px) = (&cols, &row_of, &px);
            scope.spawn(move || {
                let rows = uchunk.len() / cw.max(1);
                for r in 0..rows {
                    let cy = bi_ * band + r;
                    let mut acc = vec![(0i32, 0i32, 0i32); cw];
                    for dy in 0..2 {
                        let y = cy * 2 + dy;
                        let (ra, rb) = row_of(y);
                        let yrow = &mut ychunk[(r * 2 + dy) * dw..(r * 2 + dy + 1) * dw];
                        for (x, out) in yrow.iter_mut().enumerate() {
                            let (ca, cb) = cols[x];
                            let (p1, p2, p3, p4) = (px(ra, ca), px(ra, cb), px(rb, ca), px(rb, cb));
                            let rr = ((p1.0 + p2.0 + p3.0 + p4.0) / 4) as i32;
                            let gg = ((p1.1 + p2.1 + p3.1 + p4.1) / 4) as i32;
                            let bb = ((p1.2 + p2.2 + p3.2 + p4.2) / 4) as i32;
                            *out = (((47 * rr + 157 * gg + 16 * bb + 128) >> 8) + 16).clamp(16, 235) as u8;
                            let a = &mut acc[x / 2];
                            a.0 += rr;
                            a.1 += gg;
                            a.2 += bb;
                        }
                    }
                    for (x, (r_, g_, b_)) in acc.into_iter().enumerate() {
                        let (r_, g_, b_) = (r_ / 4, g_ / 4, b_ / 4);
                        uchunk[r * cw + x] = (((-26 * r_ - 87 * g_ + 112 * b_ + 128) >> 8) + 128).clamp(16, 240) as u8;
                        vchunk[r * cw + x] = (((112 * r_ - 102 * g_ - 10 * b_ + 128) >> 8) + 128).clamp(16, 240) as u8;
                    }
                }
            });
        }
    });
    (yp, up, vp)
}

/// Starts recording on two threads (capture → encode).
pub fn start(opts: RecordOptions) -> Result<Recording, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let frames = Arc::new(AtomicU64::new(0));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let (stop2, frames2, path) = (stop.clone(), frames.clone(), opts.path.clone());
    let thread = std::thread::Builder::new()
        .name("mandara-record".into())
        .spawn(move || run(opts, stop2, frames2, ready_tx))
        .map_err(|e| e.to_string())?;
    // Surface setup errors (no screencopy, bad region…) to the caller.
    match ready_rx.recv_timeout(READY_TIMEOUT) {
        Ok(Ok(())) => Ok(Recording { stop, frames, thread: Some(thread), started: Instant::now(), path }),
        Ok(Err(err)) => {
            let _ = thread.join();
            Err(err)
        }
        Err(_) => Err(t("The recording did not start in time").into()),
    }
}

fn run(opts: RecordOptions, stop: Arc<AtomicBool>, frames: Arc<AtomicU64>, ready: std::sync::mpsc::Sender<Result<(), String>>) -> Result<RecordingResult, String> {
    let setup = (|| -> Result<_, String> {
        let mut source = FrameSource::new(&opts.output, opts.region).map_err(|e| format!("{e:#}"))?.with_cursor(true);
        let (w, h) = {
            let first = source.next_frame().map_err(|e| format!("{e:#}"))?;
            (first.width, first.height)
        };
        Ok((source, w, h))
    })();
    let (mut source, sw, sh) = match setup {
        Ok(v) => v,
        Err(e) => {
            let _ = ready.send(Err(e.clone()));
            return Err(e);
        }
    };
    let (dw, dh) = encode_size(sw, sh, opts.max_width);
    let fps = opts.fps.clamp(5, 60);
    let mut sink = match VideoSink::create(&opts.path, &SinkOptions::new(dw, dh, fps)) {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(e.clone()));
            return Err(e);
        }
    };
    let _ = ready.send(Ok(()));

    // Capture thread: paced at `fps`, drops frames when the encoder lags.
    let (tx, rx) = std::sync::mpsc::sync_channel::<Yuv>(3);
    let dropped = Arc::new(AtomicU64::new(0));
    let (cap_stop, cap_dropped) = (stop.clone(), dropped.clone());
    let capture = std::thread::Builder::new().name("mandara-record-capture".into()).spawn(move || {
        let t0 = Instant::now();
        let interval = Duration::from_secs_f64(1.0 / fps as f64);
        let mut next = t0;
        while !cap_stop.load(Ordering::SeqCst) {
            let now = Instant::now();
            if now < next {
                std::thread::sleep(next - now);
            }
            next += interval;
            let Ok(f) = source.next_frame() else { break };
            let (y, u, v) = to_yuv420(f.data, f.width, f.height, f.stride, f.bgr, f.y_invert, dw, dh);
            match tx.try_send(Yuv { y, u, v, at: t0.elapsed() }) {
                Ok(()) => {}
                Err(std::sync::mpsc::TrySendError::Full(_)) => {
                    cap_dropped.fetch_add(1, Ordering::Relaxed);
                }
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => break,
            }
        }
    });

    loop {
        match rx.recv_timeout(FRAME_WAIT) {
            Ok(yuv) => {
                sink.push_yuv420(&yuv.y, &yuv.u, &yuv.v, yuv.at)?;
                frames.store(sink.frames(), Ordering::Relaxed);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if stop.load(Ordering::SeqCst) && capture.as_ref().is_ok_and(|c| c.is_finished()) {
            // Encode whatever the capture thread already queued.
            while let Ok(yuv) = rx.try_recv() {
                sink.push_yuv420(&yuv.y, &yuv.u, &yuv.v, yuv.at)?;
            }
            break;
        }
    }
    let duration = sink.last_at() + Duration::from_secs_f64(1.0 / fps as f64);
    let (frames_written, bytes) = sink.finish(duration)?;
    frames.store(frames_written, Ordering::Relaxed);
    Ok(RecordingResult { path: opts.path, duration, frames: frames_written, dropped: dropped.load(Ordering::Relaxed), width: dw, height: dh, bytes })
}

// ── AV1 + WebM frame sink ────────────────────────────────────────────────

/// rav1e's fastest speed preset (the recorder's real-time setting).
const FASTEST_SPEED: u8 = 10;
/// Recorder quantizers: base and floor (0–255, lower is better).
const RECORD_QUANTIZER: u8 = 110;
const RECORD_MIN_QUANTIZER: u8 = 40;
/// The recorder forces a key frame at least this many seconds apart…
const RECORD_KEY_SECONDS: u32 = 4;
/// Most encoder threads (rav1e parallelizes across tiles).
const MAX_ENCODER_THREADS: usize = 32;
/// BT.709 luma weights.
const KR: f32 = 0.2126;
const KB: f32 = 0.0722;
/// Limited-range 10-bit levels: black, luma span, chroma centre and span.
const Y10_BLACK: f32 = 64.0;
const Y10_SPAN: f32 = 876.0;
const C10_MID: f32 = 512.0;
const C10_SPAN: f32 = 896.0;

/// Encoder settings for a [`VideoSink`].
#[derive(Clone, Debug, PartialEq)]
pub struct SinkOptions {
    /// Frame size; both even.
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// rav1e speed preset: 0 (slowest, smallest) … 10 (fastest).
    pub speed: u8,
    /// Base and minimum quantizer, 0–255 (lower is better quality).
    pub quantizer: u8,
    pub min_quantizer: u8,
    /// Low-latency mode (no frame reordering): for live capture.
    pub low_latency: bool,
    /// Most and fewest frames between key frames.
    pub max_key_interval: u32,
    pub min_key_interval: u32,
    /// Encode 10-bit (less banding in dark gradients) instead of 8-bit.
    pub ten_bit: bool,
    pub threads: usize,
}

impl SinkOptions {
    /// The screen recorder's real-time settings.
    pub fn new(width: u32, height: u32, fps: u32) -> Self {
        SinkOptions {
            width,
            height,
            fps: fps.max(1),
            speed: FASTEST_SPEED,
            quantizer: RECORD_QUANTIZER,
            min_quantizer: RECORD_MIN_QUANTIZER,
            low_latency: true,
            max_key_interval: fps.max(1) * RECORD_KEY_SECONDS,
            min_key_interval: fps.max(1),
            ten_bit: false,
            threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(MAX_ENCODER_THREADS),
        }
    }
}

enum Encoder {
    Eight(Context<u8>),
    Ten(Context<u16>),
}

/// Encodes frames to AV1 with rav1e and muxes them into a WebM file: the
/// shared back end of the screen recorder and of generated video
/// wallpapers. Frames are pushed as BT.709 limited-range YUV 4:2:0 planes
/// or as packed RGB (converted here), each with its presentation time.
pub struct VideoSink {
    enc: Encoder,
    mux: WebmWriter,
    width: u32,
    height: u32,
    stamps: std::collections::HashMap<u64, Duration>,
    sent: u64,
    written: u64,
    last_at: Duration,
}

impl VideoSink {
    /// Opens `path` (creating its directory) and configures the encoder.
    pub fn create(path: &std::path::Path, opts: &SinkOptions) -> Result<VideoSink, String> {
        let (w, h) = ((opts.width & !1).max(2), (opts.height & !1).max(2));
        let fps = opts.fps.max(1) as u64;
        let enc = EncoderConfig {
            // rav1e parallelizes within a frame only across tiles.
            tiles: opts.threads.max(1),
            width: w as usize,
            height: h as usize,
            bit_depth: if opts.ten_bit { 10 } else { 8 },
            time_base: Rational::new(1, fps),
            speed_settings: SpeedSettings::from_preset(opts.speed.min(FASTEST_SPEED)),
            low_latency: opts.low_latency,
            quantizer: opts.quantizer as usize,
            min_quantizer: opts.min_quantizer,
            max_key_frame_interval: opts.max_key_interval.max(1) as u64,
            min_key_frame_interval: opts.min_key_interval.min(opts.max_key_interval).max(1) as u64,
            pixel_range: PixelRange::Limited,
            color_description: Some(ColorDescription {
                color_primaries: ColorPrimaries::BT709,
                transfer_characteristics: TransferCharacteristics::BT709,
                matrix_coefficients: MatrixCoefficients::BT709,
            }),
            ..EncoderConfig::with_speed_preset(opts.speed.min(FASTEST_SPEED))
        };
        let cfg = Config::new().with_encoder_config(enc).with_threads(opts.threads.max(1));
        let failed = |e: InvalidConfig| tf("Video encoder failed: {error}", &[("error", &e)]);
        let enc = if opts.ten_bit { Encoder::Ten(cfg.new_context().map_err(failed)?) } else { Encoder::Eight(cfg.new_context().map_err(failed)?) };
        let header = match &enc {
            Encoder::Eight(c) => c.container_sequence_header(),
            Encoder::Ten(c) => c.container_sequence_header(),
        };
        let mux = WebmWriter::create(path, w, h, &header).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(VideoSink { enc, mux, width: w, height: h, stamps: Default::default(), sent: 0, written: 0, last_at: Duration::ZERO })
    }

    /// Encoded frame size (even).
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Frames written to the file so far.
    pub fn frames(&self) -> u64 {
        self.written
    }

    /// Presentation time of the last written frame.
    pub fn last_at(&self) -> Duration {
        self.last_at
    }

    /// Pushes one frame of 8-bit planes (`y`: width × height, `u`/`v`:
    /// half size each way). A 10-bit sink scales them up.
    pub fn push_yuv420(&mut self, y: &[u8], u: &[u8], v: &[u8], at: Duration) -> Result<(), String> {
        let cw = (self.width / 2) as usize;
        self.stamps.insert(self.sent, at);
        self.sent += 1;
        let sent = match &mut self.enc {
            Encoder::Eight(ctx) => {
                let mut f = ctx.new_frame();
                f.planes[0].copy_from_raw_u8(y, self.width as usize, 1);
                f.planes[1].copy_from_raw_u8(u, cw, 1);
                f.planes[2].copy_from_raw_u8(v, cw, 1);
                ctx.send_frame(f).map_err(encoder_error)
            }
            Encoder::Ten(ctx) => {
                let up = |p: &[u8]| -> Vec<u8> { p.iter().flat_map(|&s| ((s as u16) << 2).to_le_bytes()).collect() };
                let mut f = ctx.new_frame();
                f.planes[0].copy_from_raw_u8(&up(y), self.width as usize * 2, 2);
                f.planes[1].copy_from_raw_u8(&up(u), cw * 2, 2);
                f.planes[2].copy_from_raw_u8(&up(v), cw * 2, 2);
                ctx.send_frame(f).map_err(encoder_error)
            }
        };
        sent?;
        self.drain().map(|_| ())
    }

    /// Pushes one frame of 10-bit samples (0–1023, limited range). An
    /// 8-bit sink rounds them down to 8 bits.
    pub fn push_yuv420_10(&mut self, y: &[u16], u: &[u16], v: &[u16], at: Duration) -> Result<(), String> {
        match &mut self.enc {
            Encoder::Eight(_) => {
                let down = |p: &[u16]| -> Vec<u8> { p.iter().map(|&s| ((s + 2) >> 2).min(255) as u8).collect() };
                self.push_yuv420(&down(y), &down(u), &down(v), at)
            }
            Encoder::Ten(ctx) => {
                let cw = (self.width / 2) as usize;
                let bytes = |p: &[u16]| -> Vec<u8> { p.iter().flat_map(|s| s.to_le_bytes()).collect() };
                let mut f = ctx.new_frame();
                f.planes[0].copy_from_raw_u8(&bytes(y), self.width as usize * 2, 2);
                f.planes[1].copy_from_raw_u8(&bytes(u), cw * 2, 2);
                f.planes[2].copy_from_raw_u8(&bytes(v), cw * 2, 2);
                self.stamps.insert(self.sent, at);
                self.sent += 1;
                ctx.send_frame(f).map_err(encoder_error)?;
                self.drain().map(|_| ())
            }
        }
    }

    /// Pushes a packed RGB frame (`width × height × 3` bytes, sRGB).
    pub fn push_rgb(&mut self, rgb: &[u8], at: Duration) -> Result<(), String> {
        let (w, h) = (self.width, self.height);
        let rgbx: Vec<u8> = rgb.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 0]).collect();
        let (y, u, v) = to_yuv420(&rgbx, w, h, w * 4, false, false, w, h);
        self.push_yuv420(&y, &u, &v, at)
    }

    /// Pushes a packed 16-bit RGB frame (`width × height × 3` samples,
    /// sRGB, 0–65535), converted at 10-bit precision.
    pub fn push_rgb16(&mut self, rgb: &[u16], at: Duration) -> Result<(), String> {
        let (y, u, v) = rgb16_to_yuv420_10(rgb, self.width as usize, self.height as usize);
        self.push_yuv420_10(&y, &u, &v, at)
    }

    /// Writes every finished packet; `Ok(true)` once the encoder is done.
    fn drain(&mut self) -> Result<bool, String> {
        loop {
            let packet = match &mut self.enc {
                Encoder::Eight(ctx) => ctx.receive_packet().map(|p| (p.data, p.input_frameno, p.frame_type)),
                Encoder::Ten(ctx) => ctx.receive_packet().map(|p| (p.data, p.input_frameno, p.frame_type)),
            };
            match packet {
                Ok((data, frameno, kind)) => {
                    let at = self.stamps.remove(&frameno).unwrap_or(self.last_at);
                    self.last_at = at;
                    self.mux.write_frame(&data, at, kind == FrameType::KEY).map_err(|e| e.to_string())?;
                    self.written += 1;
                }
                Err(EncoderStatus::Encoded) => continue,
                Err(EncoderStatus::NeedMoreData) => return Ok(false),
                Err(EncoderStatus::LimitReached) => return Ok(true),
                Err(e) => return Err(encoder_error(e)),
            }
        }
    }

    /// Flushes the encoder and finalizes the file with `duration`; returns
    /// the frames written and the file size.
    pub fn finish(mut self, duration: Duration) -> Result<(u64, u64), String> {
        match &mut self.enc {
            Encoder::Eight(ctx) => ctx.flush(),
            Encoder::Ten(ctx) => ctx.flush(),
        }
        while !self.drain()? {}
        let frames = self.written;
        let bytes = self.mux.finish(duration).map_err(|e| e.to_string())?;
        Ok((frames, bytes))
    }
}

fn encoder_error(e: EncoderStatus) -> String {
    tf("Video encoder failed: {error}", &[("error", &format!("{e:?}"))])
}

/// Packed 16-bit sRGB → BT.709 limited-range 10-bit YUV 4:2:0 (`w`, `h`
/// even; chroma is the 2 × 2 average).
pub fn rgb16_to_yuv420_10(rgb: &[u16], w: usize, h: usize) -> (Vec<u16>, Vec<u16>, Vec<u16>) {
    let (cw, ch) = (w / 2, h / 2);
    let mut y = vec![0u16; w * h];
    let mut u = vec![0u16; cw * ch];
    let mut v = vec![0u16; cw * ch];
    let kg = 1.0 - KR - KB;
    let px = |i: usize| -> (f32, f32, f32) {
        let p = rgb.get(i * 3..i * 3 + 3).unwrap_or(&[0, 0, 0]);
        (p[0] as f32 / 65535.0, p[1] as f32 / 65535.0, p[2] as f32 / 65535.0)
    };
    for cy in 0..ch {
        for cx in 0..cw {
            let (mut sr, mut sg, mut sb) = (0.0, 0.0, 0.0);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = (cy * 2 + dy) * w + cx * 2 + dx;
                let (r, g, b) = px(i);
                let luma = KR * r + kg * g + KB * b;
                y[i] = (Y10_BLACK + Y10_SPAN * luma).round().clamp(0.0, 1023.0) as u16;
                (sr, sg, sb) = (sr + r, sg + g, sb + b);
            }
            let (r, g, b) = (sr / 4.0, sg / 4.0, sb / 4.0);
            let luma = KR * r + kg * g + KB * b;
            let cb = (b - luma) / (2.0 * (1.0 - KB));
            let cr = (r - luma) / (2.0 * (1.0 - KR));
            u[cy * cw + cx] = (C10_MID + C10_SPAN * cb).round().clamp(0.0, 1023.0) as u16;
            v[cy * cw + cx] = (C10_MID + C10_SPAN * cr).round().clamp(0.0, 1023.0) as u16;
        }
    }
    (y, u, v)
}

// ── WebM (Matroska) writer ───────────────────────────────────────────────

/// EBML variable-length size.
fn vint(n: u64) -> Vec<u8> {
    for len in 1..=8u32 {
        if n < (1u64 << (7 * len)) - 1 {
            let mut out = vec![0u8; len as usize];
            for (i, b) in out.iter_mut().enumerate() {
                *b = (n >> (8 * (len as usize - 1 - i))) as u8;
            }
            out[0] |= 0x80 >> (len - 1);
            return out;
        }
    }
    vec![0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]
}

fn el(id: u32, body: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = id.to_be_bytes().into_iter().skip_while(|b| *b == 0).collect();
    out.extend(vint(body.len() as u64));
    out.extend_from_slice(body);
    out
}

fn uint(id: u32, v: u64) -> Vec<u8> {
    let bytes: Vec<u8> = v.to_be_bytes().into_iter().skip_while(|b| *b == 0).collect();
    el(id, if bytes.is_empty() { &[0] } else { &bytes })
}

/// Minimal streaming WebM muxer for one AV1 video track: clusters are
/// buffered and written with known sizes; the duration is patched in at
/// the end.
pub struct WebmWriter {
    file: BufWriter<File>,
    duration_at: u64,
    cluster: Vec<u8>,
    cluster_start: Option<Duration>,
}

const UNKNOWN_SIZE: [u8; 8] = [0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

impl WebmWriter {
    pub fn create(path: &std::path::Path, width: u32, height: u32, av1c: &[u8]) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = BufWriter::new(File::create(path)?);
        let header = [uint(0x4286, 1), uint(0x42F7, 1), uint(0x42F2, 4), uint(0x42F3, 8), el(0x4282, b"webm"), uint(0x4287, 4), uint(0x4285, 2)].concat();
        file.write_all(&el(0x1A45_DFA3, &header))?;
        // Segment of unknown size (streaming).
        file.write_all(&0x1853_8067u32.to_be_bytes())?;
        file.write_all(&UNKNOWN_SIZE)?;
        // Info, with a Duration placeholder we patch on finish.
        let app = concat!("Mandara Shell ", env!("CARGO_PKG_VERSION"));
        let info_body_head = [uint(0x2A_D7B1, 1_000_000), el(0x4D80, app.as_bytes()), el(0x5741, app.as_bytes())].concat();
        let duration = el(0x4489, &0f64.to_be_bytes());
        let info = el(0x1549_A966, &[info_body_head.clone(), duration.clone()].concat());
        let info_at = file.stream_position()?;
        // Offset of the 8 float bytes: element header + body head + Duration id/size.
        let info_header_len = (info.len() - info_body_head.len() - duration.len()) as u64;
        let duration_at = info_at + info_header_len + info_body_head.len() as u64 + (duration.len() - 8) as u64;
        file.write_all(&info)?;
        let video = el(0xE0, &[uint(0xB0, width as u64), uint(0xBA, height as u64)].concat());
        let track = el(0xAE, &[uint(0xD7, 1), uint(0x73C5, 1), uint(0x83, 1), el(0x86, b"V_AV1"), el(0x63A2, av1c), video].concat());
        file.write_all(&el(0x1654_AE6B, &track))?;
        Ok(Self { file, duration_at, cluster: Vec::new(), cluster_start: None })
    }

    fn flush_cluster(&mut self) -> std::io::Result<()> {
        if let Some(start) = self.cluster_start.take() {
            let body = [uint(0xE7, start.as_millis() as u64), std::mem::take(&mut self.cluster)].concat();
            self.file.write_all(&el(0x1F43_B675, &body))?;
        }
        Ok(())
    }

    /// Writes one temporal unit at `at` (from the recording start).
    pub fn write_frame(&mut self, data: &[u8], at: Duration, key: bool) -> std::io::Result<()> {
        let rel = |start: Duration| at.as_millis() as i64 - start.as_millis() as i64;
        if key || self.cluster_start.is_none_or(|s| rel(s) > 30_000) {
            self.flush_cluster()?;
            self.cluster_start = Some(at);
        }
        let start = self.cluster_start.unwrap_or(at);
        // Matroska's AV1 mapping drops temporal delimiter OBUs.
        let data = if data.len() >= 2 && data[0] == 0x12 && data[1] == 0x00 { &data[2..] } else { data };
        let mut block = vec![0x81];
        block.extend_from_slice(&(rel(start).clamp(i16::MIN as i64, i16::MAX as i64) as i16).to_be_bytes());
        block.push(if key { 0x80 } else { 0x00 });
        block.extend_from_slice(data);
        self.cluster.extend(el(0xA3, &block));
        Ok(())
    }

    /// Flushes and patches the duration; returns the file size.
    pub fn finish(mut self, duration: Duration) -> std::io::Result<u64> {
        self.flush_cluster()?;
        let end = self.file.stream_position()?;
        self.file.seek(SeekFrom::Start(self.duration_at))?;
        self.file.write_all(&(duration.as_secs_f64() * 1000.0).to_be_bytes())?;
        self.file.seek(SeekFrom::Start(end))?;
        self.file.flush()?;
        Ok(end)
    }
}

/// `~/Videos/Recordings/Recording_<time>.webm` (XDG videos dir).
pub fn default_path() -> PathBuf {
    let dir = dirs::video_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Videos")).join("Recordings");
    crate::screenshot::unique_path(&dir, &format!("Recording_{}", chrono::Local::now().format("%Y-%m-%d_%H-%M-%S")), "webm")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vint_sizes() {
        assert_eq!(vint(0), vec![0x80]);
        assert_eq!(vint(126), vec![0xFE]);
        assert_eq!(vint(127), vec![0x40, 0x7F]);
        assert_eq!(vint(300), vec![0x41, 0x2C]);
    }

    #[test]
    fn encode_size_is_even_and_bounded() {
        assert_eq!(encode_size(3840, 2160, 1920), (1920, 1080));
        assert_eq!(encode_size(801, 601, 1920), (800, 600));
        assert_eq!(encode_size(5000, 1, 1920), (1920, 2));
    }

    #[test]
    fn yuv_of_gray_is_neutral() {
        let data = vec![128u8; 4 * 4 * 4];
        let (y, u, v) = to_yuv420(&data, 4, 4, 16, true, false, 4, 4);
        assert!(y.iter().all(|&l| (l as i32 - 126).abs() <= 2), "{y:?}");
        assert!(u.iter().chain(&v).all(|&c| (c as i32 - 128).abs() <= 1));
    }

    #[test]
    fn encodes_and_muxes_a_short_clip() {
        let (w, h) = (64u32, 48u32);
        let enc = EncoderConfig { width: w as usize, height: h as usize, time_base: Rational::new(1, 30), ..EncoderConfig::with_speed_preset(10) };
        let mut ctx: Context<u8> = Config::new().with_encoder_config(enc).with_threads(2).new_context().unwrap();
        let path = std::env::temp_dir().join(format!("mandara-rec-{}.webm", std::process::id()));
        let mut mux = WebmWriter::create(&path, w, h, &ctx.container_sequence_header()).unwrap();
        for i in 0..10u8 {
            let data = vec![i * 20; (w * h * 4) as usize];
            let (y, u, v) = to_yuv420(&data, w, h, w * 4, true, false, w, h);
            let mut f = ctx.new_frame();
            f.planes[0].copy_from_raw_u8(&y, w as usize, 1);
            f.planes[1].copy_from_raw_u8(&u, (w / 2) as usize, 1);
            f.planes[2].copy_from_raw_u8(&v, (w / 2) as usize, 1);
            ctx.send_frame(f).unwrap();
        }
        ctx.flush();
        let mut n = 0;
        loop {
            match ctx.receive_packet() {
                Ok(p) => {
                    mux.write_frame(&p.data, Duration::from_millis(33 * p.input_frameno), p.frame_type == FrameType::KEY).unwrap();
                    n += 1;
                }
                Err(EncoderStatus::Encoded) => {}
                Err(EncoderStatus::LimitReached) => break,
                Err(e) => panic!("{e:?}"),
            }
        }
        let size = mux.finish(Duration::from_millis(330)).unwrap();
        assert_eq!(n, 10);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], &[0x1A, 0x45, 0xDF, 0xA3]);
        assert_eq!(bytes.len() as u64, size);
        assert!(bytes.windows(5).any(|w| w == b"V_AV1"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn sink_encodes_rgb_frames_at_8_and_10_bits() {
        let (w, h) = (64u32, 48u32);
        for ten_bit in [false, true] {
            let path = std::env::temp_dir().join(format!("mandara-sink-{}-{ten_bit}.webm", std::process::id()));
            let opts = SinkOptions { ten_bit, low_latency: false, threads: 2, ..SinkOptions::new(w, h, 24) };
            let mut sink = VideoSink::create(&path, &opts).unwrap();
            for i in 0..6u16 {
                let rgb16: Vec<u16> = (0..w * h * 3).map(|k| (k as u16).wrapping_mul(37).wrapping_add(i * 4000)).collect();
                sink.push_rgb16(&rgb16, Duration::from_millis(u64::from(i) * 42)).unwrap();
            }
            let rgb8 = vec![90u8; (w * h * 3) as usize];
            sink.push_rgb(&rgb8, Duration::from_millis(6 * 42)).unwrap();
            let (frames, size) = sink.finish(Duration::from_millis(7 * 42)).unwrap();
            assert_eq!(frames, 7, "ten_bit={ten_bit}");
            let bytes = std::fs::read(&path).unwrap();
            assert_eq!(bytes.len() as u64, size);
            assert!(bytes.windows(5).any(|w| w == b"V_AV1"));
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn rgb16_to_yuv_levels() {
        let white = vec![65535u16; 2 * 2 * 3];
        let (y, u, v) = rgb16_to_yuv420_10(&white, 2, 2);
        assert!(y.iter().all(|&l| l == 940), "{y:?}");
        assert_eq!((u[0], v[0]), (512, 512));
        let (y, ..) = rgb16_to_yuv420_10(&[0u16; 12], 2, 2);
        assert!(y.iter().all(|&l| l == 64));
    }
}
