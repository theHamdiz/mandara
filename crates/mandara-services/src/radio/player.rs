//! The radio player. A session (one station, from play to pause or stop)
//! runs on two threads that exist only while it plays:
//!
//! * `mandara-radio-net` connects (following redirects and playlists),
//!   strips the ICY metadata, fills the jitter buffer and reconnects with
//!   backoff when the stream drops;
//! * `mandara-radio-out` decodes from the buffer and feeds the sound
//!   server through [`super::output`].
//!
//! Pausing or stopping bumps the session's generation and closes its
//! buffer: both threads notice within a quarter second, close their
//! sockets and end, and every buffer they made is freed with them. Nothing
//! polls between sessions. Changes are reported through `on_change` (the
//! owner reads [`Player::with`]); live readings (output level, buffer
//! fill) are atomics.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mandara_i18n::{t, tf};
use serde::Serialize;

use super::backoff::{self, Backoff};
use super::decode::{self, Source};
use super::http;
use super::icy;
use super::output::{BYTES_PER_FRAME, MAX_WRITE_FRAMES, Output, Renderer};
use super::playlist::{self, Playlist};
use super::ring::{self, Ring};
use super::station::Codec;
use crate::quran::player::Resample;

/// No audio for this long counts as a dropped stream.
pub const STALL: Duration = Duration::from_secs(12);
/// Body bytes read per network call.
const READ_CHUNK: usize = 16 * 1024;
/// Bytes looked at to recognise a codec the server didn't name.
const SNIFF_BYTES: usize = 2048;
/// Playlist files nested in playlist files, at most.
const MAX_PLAYLIST_DEPTH: usize = 2;
/// A connection that delivered audio this long counts as having played
/// (later drops retry instead of giving up as a wrong address would).
const PLAYED_AFTER: Duration = Duration::from_secs(2);
/// How often the reconnect wait looks at the stop flag.
const RETRY_POLL: Duration = Duration::from_millis(100);
/// Share of the buffer filled before playback starts or resumes.
const REFILL_SHARE: f32 = 0.5;
/// The sleep timer fades out over this long before stopping.
pub const SLEEP_FADE: Duration = Duration::from_secs(20);
/// The buffer settings' bounds and default (seconds).
pub const BUFFER_RANGE: (f32, f32) = (2.0, 10.0);
pub const DEFAULT_BUFFER_S: f32 = 4.0;
/// Default volume for new installs.
pub const DEFAULT_VOLUME: f32 = 0.7;
/// Decoded frames held between packets and writes (an AAC/Vorbis packet
/// is at most 8192 frames; resampling may add a few).
const PENDING_FRAMES: usize = 8192 * 2 + 64;

/// Where the player stands.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Stopped,
    /// Opening the station.
    Connecting,
    /// Connected, filling the buffer (also after an underrun).
    Buffering,
    Playing,
    Paused,
    /// The stream dropped; trying again.
    Reconnecting,
}

impl Status {
    /// A session is open (playing or about to).
    pub fn active(self) -> bool {
        matches!(self, Status::Connecting | Status::Buffering | Status::Playing | Status::Reconnecting)
    }
}

/// The station a session plays.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Tune {
    pub id: String,
    pub name: String,
    pub url: String,
    /// The listed bitrate (sizes the buffer before the server says).
    pub kbps: Option<u32>,
}

/// How sessions behave (from the plugin's settings).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Options {
    pub buffer_seconds: f32,
    pub auto_reconnect: bool,
    pub attempts: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self { buffer_seconds: DEFAULT_BUFFER_S, auto_reconnect: true, attempts: backoff::DEFAULT_ATTEMPTS }
    }
}

/// What the stream is, once connected.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct StreamInfo {
    pub codec: Codec,
    pub kbps: Option<u32>,
    pub rate: u32,
    pub channels: u8,
    /// The server's own station name and genre (`icy-name`, `icy-genre`).
    pub server_name: Option<String>,
    pub genre: Option<String>,
}

/// What the player is doing.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Snapshot {
    pub status: Status,
    pub tune: Option<Tune>,
    /// Now playing (the stream's ICY title).
    pub title: Option<String>,
    pub info: StreamInfo,
    pub error: Option<String>,
    /// Reconnection attempts since the stream dropped.
    pub attempt: u32,
}

/// Requests from outside the shell (media keys, other apps' controls) the
/// owner answers: they need its station list.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Next,
    Previous,
    Raise,
}

struct Ctl {
    snap: Snapshot,
    options: Options,
    ring: Option<Arc<Ring>>,
    mpris: Option<super::mpris::Server>,
    /// Expose sessions over MPRIS.
    mpris_enabled: bool,
}

pub(super) struct Inner {
    ctl: Mutex<Ctl>,
    generation: AtomicU64,
    volume: AtomicU32,
    muted: AtomicBool,
    level: AtomicU32,
    /// Unix ms when the sleep timer stops playback (0 = off) and its length.
    sleep_at: AtomicU64,
    sleep_len: AtomicU64,
    on_change: Box<dyn Fn() + Send + Sync>,
    on_request: Box<dyn Fn(Request) + Send + Sync>,
}

/// The radio player (cheap to clone; one per shell).
#[derive(Clone)]
pub struct Player {
    pub(super) inner: Arc<Inner>,
}

fn lock(m: &Mutex<Ctl>) -> MutexGuard<'_, Ctl> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or_default()
}

impl Player {
    pub fn new(on_change: impl Fn() + Send + Sync + 'static, on_request: impl Fn(Request) + Send + Sync + 'static) -> Self {
        let ctl = Ctl { snap: Snapshot::default(), options: Options::default(), ring: None, mpris: None, mpris_enabled: true };
        Self {
            inner: Arc::new(Inner {
                ctl: Mutex::new(ctl),
                generation: AtomicU64::new(0),
                volume: AtomicU32::new(DEFAULT_VOLUME.to_bits()),
                muted: AtomicBool::new(false),
                level: AtomicU32::new(0),
                sleep_at: AtomicU64::new(0),
                sleep_len: AtomicU64::new(0),
                on_change: Box::new(on_change),
                on_request: Box::new(on_request),
            }),
        }
    }

    pub(super) fn from_weak(weak: &Weak<Inner>) -> Option<Self> {
        weak.upgrade().map(|inner| Self { inner })
    }

    pub(super) fn downgrade(&self) -> Weak<Inner> {
        Arc::downgrade(&self.inner)
    }

    /// Reads the state without copying it.
    pub fn with<R>(&self, f: impl FnOnce(&Snapshot) -> R) -> R {
        f(&lock(&self.inner.ctl).snap)
    }

    pub fn snapshot(&self) -> Snapshot {
        self.with(Snapshot::clone)
    }

    pub fn status(&self) -> Status {
        self.with(|s| s.status)
    }

    /// The session's buffer, 0–1 (`None` between sessions).
    pub fn buffer_fill(&self) -> Option<f32> {
        lock(&self.inner.ctl).ring.as_ref().map(|r| r.fill())
    }

    /// The buffer is refilling (start or underrun).
    pub fn buffering(&self) -> bool {
        lock(&self.inner.ctl).ring.as_ref().is_some_and(|r| r.starved())
    }

    /// The output level, 0–1 (for meters).
    pub fn level(&self) -> f32 {
        f32::from_bits(self.inner.level.load(Ordering::Relaxed))
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.inner.volume.load(Ordering::Relaxed))
    }

    pub fn muted(&self) -> bool {
        self.inner.muted.load(Ordering::Relaxed)
    }

    pub fn set_volume(&self, v: f32) {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { DEFAULT_VOLUME };
        if (self.volume() - v).abs() > f32::EPSILON {
            self.inner.volume.store(v.to_bits(), Ordering::Relaxed);
            self.inner.changed();
        }
    }

    pub fn set_muted(&self, muted: bool) {
        if self.inner.muted.swap(muted, Ordering::Relaxed) != muted {
            self.inner.changed();
        }
    }

    /// Settings for the next session (and reconnects of the current one).
    pub fn set_options(&self, options: Options, mpris: bool) {
        let drop_server = {
            let mut c = lock(&self.inner.ctl);
            c.options = options;
            c.mpris_enabled = mpris;
            if mpris { None } else { c.mpris.take() }
        };
        drop(drop_server);
        if mpris && self.status() != Status::Stopped {
            self.ensure_mpris();
        }
    }

    /// Plays `tune` (replacing whatever played).
    pub fn play(&self, tune: Tune) {
        let generation = {
            let mut c = lock(&self.inner.ctl);
            if let Some(r) = c.ring.take() {
                r.close();
            }
            let g = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
            c.snap = Snapshot { status: Status::Connecting, tune: Some(tune), ..Snapshot::default() };
            g
        };
        self.inner.level.store(0, Ordering::Relaxed);
        self.ensure_mpris();
        self.inner.changed();
        self.spawn(generation);
    }

    /// Pauses: the session ends (live radio can't hold its place), the
    /// station stays chosen; whether it was playing.
    pub fn pause(&self) -> bool {
        {
            let mut c = lock(&self.inner.ctl);
            if !c.snap.status.active() {
                return false;
            }
            self.inner.generation.fetch_add(1, Ordering::SeqCst);
            if let Some(r) = c.ring.take() {
                r.close();
            }
            c.snap.status = Status::Paused;
            c.snap.attempt = 0;
        }
        self.cancel_sleep();
        self.inner.level.store(0, Ordering::Relaxed);
        self.inner.changed();
        true
    }

    /// Plays the paused (or last stopped-with-error) station again.
    pub fn resume(&self) -> bool {
        let tune = self.with(|s| (matches!(s.status, Status::Paused | Status::Stopped)).then(|| s.tune.clone()).flatten());
        match tune {
            Some(t) => {
                self.play(t);
                true
            }
            None => false,
        }
    }

    /// Play or pause; whether it plays now.
    pub fn toggle(&self) -> bool {
        if self.status().active() {
            self.pause();
            false
        } else {
            self.resume()
        }
    }

    /// Stops and forgets the station; releases everything, MPRIS included.
    pub fn stop(&self) {
        let (was, server) = {
            let mut c = lock(&self.inner.ctl);
            self.inner.generation.fetch_add(1, Ordering::SeqCst);
            if let Some(r) = c.ring.take() {
                r.close();
            }
            let was = c.snap.status;
            c.snap = Snapshot::default();
            (was, c.mpris.take())
        };
        drop(server);
        self.cancel_sleep();
        self.inner.level.store(0, Ordering::Relaxed);
        if was != Status::Stopped {
            self.inner.changed();
        }
    }

    /// Stops after `after`, fading out over its last [`SLEEP_FADE`].
    pub fn set_sleep(&self, after: Duration) {
        self.inner.sleep_len.store(after.as_millis() as u64, Ordering::Relaxed);
        self.inner.sleep_at.store(now_ms() + after.as_millis() as u64, Ordering::Relaxed);
        self.inner.changed();
    }

    pub fn cancel_sleep(&self) {
        if self.inner.sleep_at.swap(0, Ordering::Relaxed) != 0 {
            self.inner.sleep_len.store(0, Ordering::Relaxed);
            self.inner.changed();
        }
    }

    /// Remaining sleep time and the timer's full length.
    pub fn sleep_remaining(&self) -> Option<(Duration, Duration)> {
        self.inner.sleep_remaining()
    }

    /// Forwards a request from MPRIS to the owner.
    pub(super) fn request(&self, r: Request) {
        (self.inner.on_request)(r);
    }

    fn ensure_mpris(&self) {
        let mut c = lock(&self.inner.ctl);
        if c.mpris_enabled && c.mpris.is_none() {
            c.mpris = Some(super::mpris::Server::start(self));
        }
    }

    fn spawn(&self, generation: u64) {
        let inner = self.inner.clone();
        let spawned = std::thread::Builder::new().name("mandara-radio-net".into()).spawn(move || net(&inner, generation));
        if let Err(e) = spawned {
            self.inner.fail(generation, e.to_string());
        }
    }
}

impl Inner {
    fn current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == generation
    }

    fn changed(&self) {
        (self.on_change)();
        if let Some(s) = lock(&self.ctl).mpris.as_ref() {
            s.ping();
        }
    }

    fn sleep_remaining(&self) -> Option<(Duration, Duration)> {
        let at = self.sleep_at.load(Ordering::Relaxed);
        (at != 0).then(|| (Duration::from_millis(at.saturating_sub(now_ms())), Duration::from_millis(self.sleep_len.load(Ordering::Relaxed))))
    }

    /// The gain the output should glide to now.
    fn target_gain(&self) -> f32 {
        if self.muted.load(Ordering::Relaxed) {
            return 0.0;
        }
        let v = f32::from_bits(self.volume.load(Ordering::Relaxed));
        // A perceptual curve: the slider's middle sounds like half.
        let v = v * v;
        match self.sleep_remaining() {
            Some((left, _)) if left < SLEEP_FADE => v * (left.as_secs_f32() / SLEEP_FADE.as_secs_f32()),
            _ => v,
        }
    }

    /// Updates the session's snapshot (when still current) and reports it.
    fn update(&self, generation: u64, f: impl FnOnce(&mut Snapshot)) {
        let changed = {
            let mut c = lock(&self.ctl);
            if !self.current(generation) {
                return;
            }
            let before = c.snap.clone();
            f(&mut c.snap);
            c.snap != before
        };
        if changed {
            self.changed();
        }
    }

    /// Ends the session `generation` with an error (when still current).
    fn fail(&self, generation: u64, why: String) {
        {
            let mut c = lock(&self.ctl);
            if !self.current(generation) {
                return;
            }
            self.generation.fetch_add(1, Ordering::SeqCst);
            if let Some(r) = c.ring.take() {
                r.close();
            }
            c.snap.status = Status::Stopped;
            c.snap.error = Some(why.clone());
            c.snap.attempt = 0;
        }
        self.level.store(0, Ordering::Relaxed);
        tracing::warn!("radio: stopped: {why}");
        self.changed();
    }

    /// The sleep timer ran out.
    fn sleep_done(&self, generation: u64) {
        {
            let mut c = lock(&self.ctl);
            if !self.current(generation) {
                return;
            }
            self.generation.fetch_add(1, Ordering::SeqCst);
            if let Some(r) = c.ring.take() {
                r.close();
            }
            c.snap.status = Status::Paused;
            c.snap.attempt = 0;
        }
        self.sleep_at.store(0, Ordering::Relaxed);
        self.sleep_len.store(0, Ordering::Relaxed);
        self.level.store(0, Ordering::Relaxed);
        self.changed();
    }
}

/// A connection failure: worded for people, and whether trying again
/// could help.
struct Failure {
    message: String,
    retry: bool,
}

impl Failure {
    fn retry(message: String) -> Self {
        Self { message, retry: true }
    }

    fn fatal(message: String) -> Self {
        Self { message, retry: false }
    }
}

/// Opens the audio behind `url`: follows playlists, refuses HLS, pages and
/// codecs that can't be decoded.
fn connect(url: &str, cancelled: &dyn Fn() -> bool, depth: usize) -> Result<(http::Response, Codec), Failure> {
    let mut resp = http::open(url, cancelled).map_err(|e| {
        let fatal = e == t("The station's address no longer works") || e == t("The station refused to play here (it may be limited to its own country)");
        if fatal { Failure::fatal(e) } else { Failure::retry(e) }
    })?;
    let ct = resp.head.content_type.clone();
    let path = resp.url.split_once("://").map_or("", |(_, rest)| rest.find('/').map_or("", |i| &rest[i..])).to_string();
    if playlist::is_playlist(ct.as_deref(), &path) {
        if depth >= MAX_PLAYLIST_DEPTH {
            return Err(Failure::fatal(t("The station's playlist points at another playlist").into()));
        }
        let body = resp.read_to_limit(playlist::MAX_PLAYLIST_BYTES, Instant::now() + http::RESPONSE_TIMEOUT, cancelled).map_err(Failure::retry)?;
        let base = resp.url.clone();
        drop(resp);
        return match playlist::parse(&String::from_utf8_lossy(&body), &base) {
            Playlist::Hls => Err(Failure::fatal(t("HLS streams aren't supported yet").into())),
            Playlist::Streams(entries) if entries.is_empty() => Err(Failure::fatal(t("The station's playlist is empty").into())),
            Playlist::Streams(entries) => {
                let mut last = None;
                for entry in entries.iter().take(4) {
                    match connect(entry, cancelled, depth + 1) {
                        Ok(ok) => return Ok(ok),
                        Err(f) if f.message.is_empty() => return Err(f),
                        Err(f) => last = Some(f),
                    }
                }
                Err(last.unwrap_or_else(|| Failure::fatal(t("The station's playlist is empty").into())))
            }
        };
    }
    if !decode::audio_like(ct.as_deref()) {
        return Err(Failure::fatal(t("This address is a web page, not an audio stream").into()));
    }
    let mut codec = decode::codec_of(ct.as_deref());
    if codec == Codec::Unknown {
        codec = decode::sniff(resp.peek(SNIFF_BYTES, Instant::now() + http::RESPONSE_TIMEOUT, cancelled));
    }
    if !codec.playable() {
        return Err(Failure::fatal(t("Opus streams aren't supported yet").into()));
    }
    Ok((resp, codec))
}

/// What a test connection found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Probe {
    /// The address that answered (after redirects and playlists).
    pub url: String,
    pub info: StreamInfo,
    /// Audio bytes received and frames decoded.
    pub bytes: usize,
    pub frames: usize,
}

/// Bytes a test connection reads before decoding.
const PROBE_BYTES: usize = 48 * 1024;
/// How long a test connection may take.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(12);

/// Connects to `url`, receives some audio and decodes it: proof the
/// station plays here (blocking; refused while offline). Everything it
/// opens is closed before it returns.
pub fn probe(url: &str) -> Result<Probe, String> {
    super::station::validate_url(url).map_err(|e| mandara_i18n::tr(e.message()))?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let late = || Instant::now() > deadline;
    let (mut resp, codec) = connect(url, &late, 0).map_err(|f| if f.message.is_empty() { t("The station didn't answer in time").to_string() } else { f.message })?;
    let ring = Arc::new(Ring::new(ring::MIN_CAPACITY * 2, PROBE_BYTES));
    let mut demux = icy::Demux::new(resp.head.metaint);
    let mut buf = vec![0u8; READ_CHUNK];
    let mut bytes = 0usize;
    while bytes < PROBE_BYTES && !late() {
        match resp.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => demux.feed(&buf[..n.min(PROBE_BYTES - bytes + 4096)], &mut |a| {
                let a = &a[..a.len().min(PROBE_BYTES.saturating_sub(bytes))];
                bytes += a.len();
                ring.write_all(a);
            }, &mut |_| {}),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break,
        }
    }
    let info = StreamInfo { codec, kbps: resp.head.kbps, server_name: resp.head.name.take(), genre: resp.head.genre.take(), ..StreamInfo::default() };
    let final_url = resp.url.clone();
    drop(resp);
    if bytes == 0 {
        return Err(t("The station sent no audio").into());
    }
    ring.end();
    let mut src = Source::open(&ring, codec)?;
    let mut frames = 0usize;
    while frames < 4096 {
        match src.next(&mut |_| frames += 1) {
            Ok(true) => {}
            Ok(false) => break,
            Err(e) => return Err(e),
        }
    }
    if frames == 0 {
        return Err(t("The station's audio couldn't be decoded").into());
    }
    let info = StreamInfo { codec: src.codec, rate: src.rate, channels: src.channels, ..info };
    Ok(Probe { url: final_url, info, bytes, frames })
}

/// The network thread of session `generation`.
fn net(inner: &Arc<Inner>, generation: u64) {
    let (url, listed_kbps, options) = {
        let c = lock(&inner.ctl);
        let Some(tune) = c.snap.tune.as_ref() else { return };
        (tune.url.clone(), tune.kbps, c.options)
    };
    let cancelled = || !inner.current(generation);
    let seed = (now_ms() as u32) ^ (generation as u32).wrapping_mul(2_654_435_761);
    let mut backoff = Backoff::new(options.attempts, seed);
    let mut ring: Option<Arc<Ring>> = None;
    let mut audio: Option<std::thread::JoinHandle<()>> = None;
    let mut buf = vec![0u8; READ_CHUNK];
    let mut ever_played = false;
    let mut demux = Box::new(icy::Demux::new(None));
    let mut title_block: Vec<u8> = Vec::new();
    'session: loop {
        if cancelled() {
            break;
        }
        let failure = match connect(&url, &cancelled, 0) {
            Err(f) if f.message.is_empty() => break,
            Err(f) => f,
            Ok((mut resp, codec)) => {
                let kbps = resp.head.kbps.or(listed_kbps);
                // The buffer: made once per session, sized from the bitrate.
                let r = ring.get_or_insert_with(|| {
                    let cap = ring::capacity_for(kbps, options.buffer_seconds);
                    Arc::new(Ring::new(cap, (cap as f32 * REFILL_SHARE) as usize))
                });
                {
                    let mut c = lock(&inner.ctl);
                    if !inner.current(generation) {
                        r.close();
                        break;
                    }
                    c.ring = Some(r.clone());
                    c.snap.status = Status::Buffering;
                    c.snap.error = None;
                    c.snap.info.codec = codec;
                    c.snap.info.kbps = kbps;
                    c.snap.info.server_name = resp.head.name.take();
                    c.snap.info.genre = resp.head.genre.take();
                }
                inner.changed();
                if audio.is_none() {
                    let (inner2, r2) = (inner.clone(), r.clone());
                    match std::thread::Builder::new().name("mandara-radio-out".into()).spawn(move || {
                        if let Err(e) = out(&inner2, generation, &r2) {
                            inner2.fail(generation, e);
                        }
                    }) {
                        Ok(h) => audio = Some(h),
                        Err(e) => {
                            inner.fail(generation, e.to_string());
                            break;
                        }
                    }
                }
                *demux = icy::Demux::new(resp.head.metaint);
                let connected = Instant::now();
                let mut last_data = Instant::now();
                let r = r.clone();
                let why = loop {
                    match resp.read(&mut buf) {
                        Ok(0) => break t("The station ended the stream").to_string(),
                        Ok(n) => {
                            last_data = Instant::now();
                            let mut alive = true;
                            demux.feed(
                                &buf[..n],
                                &mut |audio| alive &= r.write_all(audio),
                                &mut |meta| {
                                    title_block.clear();
                                    title_block.extend_from_slice(meta);
                                },
                            );
                            if !alive {
                                break 'session;
                            }
                            if !title_block.is_empty() {
                                let title = icy::stream_title(&title_block);
                                title_block.clear();
                                inner.update(generation, |s| s.title = title);
                            }
                            if !ever_played && connected.elapsed() > PLAYED_AFTER {
                                ever_played = true;
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            if cancelled() {
                                break 'session;
                            }
                            if last_data.elapsed() > STALL {
                                break t("The station stopped sending audio").to_string();
                            }
                        }
                        Err(_) => break t("The connection to the station dropped").to_string(),
                    }
                };
                if connected.elapsed() > backoff::HEALTHY {
                    backoff.reset();
                }
                r.end();
                Failure::retry(why)
            }
        };
        if cancelled() {
            break;
        }
        // Never played: a wrong address or format won't fix itself.
        if !failure.retry || !options.auto_reconnect || (!ever_played && backoff.attempts() >= 2) {
            inner.fail(generation, failure.message);
            break;
        }
        let Some(delay) = backoff.next_delay() else {
            inner.fail(generation, tf("Gave up after {n} tries: {why}", &[("n", &backoff.attempts()), ("why", &failure.message)]));
            break;
        };
        let attempt = backoff.attempts();
        inner.update(generation, |s| {
            s.status = Status::Reconnecting;
            s.attempt = attempt;
            s.error = Some(failure.message.clone());
        });
        let until = Instant::now() + delay;
        while Instant::now() < until {
            if cancelled() {
                break 'session;
            }
            std::thread::sleep(RETRY_POLL.min(until.saturating_duration_since(Instant::now())));
        }
    }
    let had_buffer = ring.is_some();
    if let Some(r) = ring.take() {
        r.close();
    }
    drop(buf);
    drop(demux);
    drop(title_block);
    if let Some(h) = audio {
        let _ = h.join();
    }
    // The session's buffers (jitter buffer, decoder, sound stream) are
    // gone: hand the freed pages back (coalesced, off this thread).
    if had_buffer {
        crate::memory::request_trim();
    }
}

/// The audio thread of session `generation`: decodes the buffer and plays
/// it until the session ends.
fn out(inner: &Arc<Inner>, generation: u64, ring: &Arc<Ring>) -> Result<(), String> {
    let mut output: Option<Output> = None;
    let mut renderer = Renderer::new(48_000);
    let mut pending: Vec<[f32; 2]> = Vec::with_capacity(PENDING_FRAMES);
    let mut bytes: Vec<u8> = Vec::with_capacity(MAX_WRITE_FRAMES * BYTES_PER_FRAME);
    let mut opened_once = false;
    loop {
        if !inner.current(generation) || ring.is_closed() {
            return Ok(());
        }
        let codec = lock(&inner.ctl).snap.info.codec;
        let mut src = match Source::open(ring, codec) {
            Ok(s) => s,
            Err(_) if !inner.current(generation) || ring.is_closed() => return Ok(()),
            Err(e) if !opened_once => return Err(e),
            Err(_) => {
                // A reconnect that went wrong: wait for the next one.
                ring.reopen();
                continue;
            }
        };
        opened_once = true;
        inner.update(generation, |s| {
            s.info.rate = src.rate;
            s.info.channels = src.channels;
            s.info.codec = src.codec;
        });
        let out = match &mut output {
            Some(o) => o,
            slot => {
                let o = slot.insert(Output::open(src.rate)?);
                renderer = Renderer::new(o.rate);
                o
            }
        };
        let mut resample = Resample::new(src.rate, out.rate);
        renderer.restart();
        pending.clear();
        let mut at = 0usize;
        let mut playing = false;
        'stream: loop {
            if !inner.current(generation) {
                return Ok(());
            }
            if let Some((left, _)) = inner.sleep_remaining()
                && left.is_zero()
            {
                inner.sleep_done(generation);
                return Ok(());
            }
            while out.owed >= BYTES_PER_FRAME {
                if at == pending.len() {
                    pending.clear();
                    at = 0;
                    let more = src.next(&mut |f| resample.push(f, &mut pending))?;
                    if !more {
                        break 'stream;
                    }
                    continue;
                }
                let n = (out.owed / BYTES_PER_FRAME).min(pending.len() - at).min(MAX_WRITE_FRAMES);
                let rms = renderer.render(&pending[at..at + n], inner.target_gain(), &mut bytes);
                out.write(&bytes)?;
                at += n;
                inner.level.store(rms.to_bits(), Ordering::Relaxed);
                if !playing {
                    playing = true;
                    inner.update(generation, |s| {
                        s.status = Status::Playing;
                        s.error = None;
                        s.attempt = 0;
                    });
                }
            }
            out.wait()?;
        }
        // This connection's audio ended (it dropped): the network thread
        // reconnects into the same buffer.
        drop(src);
        inner.level.store(0, Ordering::Relaxed);
        if !inner.current(generation) || ring.is_closed() {
            return Ok(());
        }
        ring.reopen();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn tune(url: &str) -> Tune {
        Tune { id: "test".into(), name: "Test".into(), url: url.into(), kbps: Some(128) }
    }

    fn player() -> (Player, Arc<AtomicUsize>) {
        let changes = Arc::new(AtomicUsize::new(0));
        let c = changes.clone();
        let p = Player::new(move || {
            c.fetch_add(1, Ordering::Relaxed);
        }, |_| {});
        p.set_options(Options::default(), false);
        (p, changes)
    }

    fn wait_until(mut f: impl FnMut() -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_stopped_player_says_nothing_and_holds_nothing() {
        let (p, changes) = player();
        assert_eq!(p.status(), Status::Stopped);
        assert!(!p.pause() && !p.resume() && !p.toggle());
        p.stop();
        assert_eq!(changes.load(Ordering::Relaxed), 0);
        assert_eq!(p.buffer_fill(), None);
        p.set_volume(0.3);
        assert!((p.volume() - 0.3).abs() < 1e-6);
        p.set_volume(f32::NAN);
        assert_eq!(p.volume(), DEFAULT_VOLUME);
        assert_eq!(changes.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn a_bad_address_fails_once_without_retrying_forever() {
        // Nothing listens on this port.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let (p, _) = player();
        p.play(tune(&format!("http://127.0.0.1:{port}/stream")));
        assert!(wait_until(|| p.status() == Status::Stopped), "{:?}", p.snapshot());
        let snap = p.snapshot();
        assert!(snap.error.is_some(), "{snap:?}");
        assert!(snap.tune.is_some(), "the station stays chosen to try again");
        assert_eq!(p.buffer_fill(), None, "the buffer went with the session");
    }

    #[test]
    fn a_web_page_is_refused() {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _ = s.read(&mut [0u8; 1024]);
            s.write_all(b"HTTP/1.0 200 OK\r\nContent-Type: text/html\r\n\r\n<html></html>").unwrap();
        });
        let (p, _) = player();
        p.play(tune(&format!("http://127.0.0.1:{port}/")));
        assert!(wait_until(|| p.status() == Status::Stopped));
        assert_eq!(p.snapshot().error.as_deref(), Some(t("This address is a web page, not an audio stream")));
        server.join().unwrap();
    }

    #[test]
    fn pausing_ends_the_session_and_stopping_forgets_it() {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _ = s.read(&mut [0u8; 1024]);
            s.write_all(b"ICY 200 OK\r\ncontent-type: audio/mpeg\r\nicy-metaint: 1000\r\n\r\n").unwrap();
            // Audio, then a title, then silence: the client must notice the
            // pause by itself and close.
            s.write_all(&[0u8; 1000]).unwrap();
            let meta = b"StreamTitle='Harbor Lights - Low Tide';";
            let mut block = vec![(meta.len().div_ceil(16)) as u8];
            block.extend_from_slice(meta);
            block.resize(1 + meta.len().div_ceil(16) * 16, 0);
            s.write_all(&block).unwrap();
            let mut sink = [0u8; 16];
            // Returns once the client closes the socket.
            let _ = s.read(&mut sink);
        });
        let (p, _) = player();
        p.play(tune(&format!("http://127.0.0.1:{port}/live")));
        assert!(wait_until(|| p.snapshot().title.is_some()), "{:?}", p.snapshot());
        assert_eq!(p.snapshot().title.as_deref(), Some("Harbor Lights - Low Tide"));
        assert!(p.buffer_fill().is_some());
        assert!(p.pause());
        assert_eq!(p.status(), Status::Paused);
        assert_eq!(p.buffer_fill(), None);
        server.join().unwrap();
        p.stop();
        assert_eq!(p.snapshot(), Snapshot::default());
    }
}
