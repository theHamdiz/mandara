//! Recitation: per-ayah MP3s from cdn.islamic.network
//! (`/quran/audio/128/<reciter>/<global ayah>.mp3`), each downloaded once
//! into `~/.cache/mandara/quran/audio/<reciter>/` (written to `.part`
//! first and checked to be MP3), decoded in pure Rust (`symphonia`, MP3
//! only) and played through the shell's sound path: the PulseAudio native
//! protocol that PipeWire's `pipewire-pulse` speaks, as one playback
//! stream ("Mandara Qur'an") per recitation, the way Soundscapes plays.
//!
//! A recitation is a queue of ayahs (one, a range or a whole surah) played
//! back to back without gaps: while one plays, the next few are fetched.
//! Two threads exist only while it plays (audio and prefetch); pausing
//! ends them and remembers the place, resuming starts them again from it.
//! Offline mode plays what is cached and stops at the first ayah that
//! isn't. Every change is reported through the `on_event` callback
//! (status, the ayah now heard, errors); nothing polls.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use mandara_i18n::tf;
use pulseaudio::protocol::{self, Command};
use serde::Serialize;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// One reciter on the CDN (each one checked to serve the whole Qur'an).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Reciter {
    pub id: &'static str,
    pub name: &'static str,
    pub arabic: &'static str,
}

/// The reciters offered (verified on cdn.islamic.network at 128 kbps).
pub const RECITERS: [Reciter; 9] = [
    Reciter { id: "ar.alafasy", name: "Mishary Rashid Alafasy", arabic: "مشاري راشد العفاسي" },
    Reciter { id: "ar.husary", name: "Mahmoud Khalil Al-Husary", arabic: "محمود خليل الحصري" },
    Reciter { id: "ar.husarymujawwad", name: "Al-Husary (Mujawwad)", arabic: "محمود خليل الحصري (مجوّد)" },
    Reciter { id: "ar.minshawi", name: "Mohamed Siddiq Al-Minshawi", arabic: "محمد صديق المنشاوي" },
    Reciter { id: "ar.hudhaify", name: "Ali Al-Hudhaify", arabic: "علي بن عبد الرحمن الحذيفي" },
    Reciter { id: "ar.mahermuaiqly", name: "Maher Al-Muaiqly", arabic: "ماهر المعيقلي" },
    Reciter { id: "ar.shaatree", name: "Abu Bakr Ash-Shatri", arabic: "أبو بكر الشاطري" },
    Reciter { id: "ar.ahmedajamy", name: "Ahmed Al-Ajamy", arabic: "أحمد بن علي العجمي" },
    Reciter { id: "ar.muhammadayyoub", name: "Muhammad Ayyub", arabic: "محمد أيوب" },
];
pub const DEFAULT_RECITER: &str = "ar.alafasy";
/// The CDN's per-ayah files.
const CDN: &str = "https://cdn.islamic.network/quran/audio/128";

/// Network bounds for one file.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const BODY_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_FILE_BYTES: u64 = 24 << 20;
/// Smaller files aren't an ayah (an error page).
const MIN_FILE_BYTES: u64 = 1024;
/// Files fetched ahead of the one playing.
const PREFETCH: usize = 3;
/// How long the prefetcher sleeps between checks when it is ahead.
const PREFETCH_WAIT: Duration = Duration::from_millis(800);

/// Output: interleaved stereo f32.
const CHANNELS: usize = 2;
const BYTES_PER_FRAME: usize = CHANNELS * std::mem::size_of::<f32>();
/// Largest single write to the server.
const MAX_WRITE_FRAMES: usize = 8192;
/// The server-side buffer: small enough that pause feels immediate.
const TARGET_LATENCY: Duration = Duration::from_millis(150);
/// Socket read timeout: how often the audio thread checks for pause.
const READ_TIMEOUT: Duration = Duration::from_millis(100);
/// Fade-in when playback (re)starts, so resuming never clicks.
const FADE_IN: Duration = Duration::from_millis(25);
/// After the last ayah: let the buffer play out before closing.
const TAIL: Duration = Duration::from_millis(250);

pub fn reciter(id: &str) -> Option<&'static Reciter> {
    RECITERS.iter().find(|r| r.id == id)
}

/// The reciter `id`, or the default for an unknown one.
pub fn reciter_or_default(id: &str) -> &'static Reciter {
    reciter(id).unwrap_or(&RECITERS[0])
}

pub fn audio_url(reciter: &str, global: u16) -> String {
    format!("{CDN}/{}/{global}.mp3", super::store::sanitize(reciter))
}

pub fn audio_path(reciter: &str, global: u16) -> PathBuf {
    super::store::cache_dir().join("audio").join(super::store::sanitize(reciter)).join(format!("{global}.mp3"))
}

/// Whether `head` starts like an MP3 file (an ID3 tag or a frame sync).
pub fn looks_like_mp3(head: &[u8]) -> bool {
    head.starts_with(b"ID3") || (head.len() >= 2 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0)
}

/// The cached file of ayah `global`, downloaded first when missing
/// (blocking; refused while offline).
pub fn fetch_audio(reciter: &str, global: u16) -> Result<PathBuf, String> {
    let path = audio_path(reciter, global);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() >= MIN_FILE_BYTES) {
        return Ok(path);
    }
    crate::offline::check()?;
    let url = audio_url(reciter, global);
    let failed = |e: &dyn std::fmt::Display| tf("Downloading the recitation of ayah {n}: {error}", &[("n", &global), ("error", e)]);
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_connect(Some(CONNECT_TIMEOUT)).timeout_recv_body(Some(BODY_TIMEOUT)).build().into();
    let mut response = agent.get(&url).header("User-Agent", concat!("mandara-shell/", env!("CARGO_PKG_VERSION"))).call().map_err(|e| failed(&e))?;
    let mut body = Vec::new();
    response.body_mut().with_config().limit(MAX_FILE_BYTES).reader().read_to_end(&mut body).map_err(|e| failed(&e))?;
    if (body.len() as u64) < MIN_FILE_BYTES || !looks_like_mp3(&body) {
        return Err(failed(&"not an MP3 file"));
    }
    let dir = path.parent().ok_or_else(|| failed(&"no folder"))?;
    std::fs::create_dir_all(dir).map_err(|e| failed(&e))?;
    let part = path.with_extension(format!("mp3.{}.part", std::process::id()));
    let written = std::fs::File::create(&part).and_then(|mut f| f.write_all(&body).and_then(|_| f.sync_all())).and_then(|_| std::fs::rename(&part, &path));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&part);
        return Err(failed(&e));
    }
    Ok(path)
}

// ── Decoding ─────────────────────────────────────────────────────────────

/// One ayah's file, decoded packet by packet.
struct Clip {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    rate: u32,
    buf: Option<SampleBuffer<f32>>,
    /// The last packet's frames as stereo, and how many are used.
    frames: Vec<[f32; 2]>,
    used: usize,
}

impl Clip {
    fn open(path: &Path) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("mp3");
        let probed = symphonia::default::get_probe().format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default()).map_err(|e| format!("{}: {e}", path.display()))?;
        let format = probed.format;
        let track = format.default_track().ok_or_else(|| format!("{}: no audio", path.display()))?;
        let decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()).map_err(|e| format!("{}: {e}", path.display()))?;
        let rate = track.codec_params.sample_rate.unwrap_or(44_100);
        Ok(Self { track: track.id, format, decoder, rate, buf: None, frames: Vec::new(), used: 0 })
    }

    /// Decodes the next packet into `frames`; `false` at the end.
    fn refill(&mut self) -> bool {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(_) => return false,
            };
            if packet.track_id() != self.track {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(d) => d,
                // A damaged frame: skip it.
                Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                Err(_) => return false,
            };
            let spec = *decoded.spec();
            let channels = spec.channels.count().max(1);
            if self.buf.as_ref().is_none_or(|b| b.capacity() < decoded.capacity() * channels) {
                self.buf = Some(SampleBuffer::new(decoded.capacity() as u64, spec));
            }
            let Some(buf) = self.buf.as_mut() else { return false };
            buf.copy_interleaved_ref(decoded);
            self.frames.clear();
            self.frames.extend(buf.samples().chunks(channels).map(|f| match f {
                [m] => [*m, *m],
                [l, r, ..] => [*l, *r],
                [] => [0.0, 0.0],
            }));
            self.used = 0;
            if !self.frames.is_empty() {
                return true;
            }
        }
    }

    /// The next frame at the clip's own rate.
    fn next(&mut self) -> Option<[f32; 2]> {
        if self.used >= self.frames.len() && !self.refill() {
            return None;
        }
        let f = self.frames[self.used];
        self.used += 1;
        Some(f)
    }

    /// Skips `n` frames (resuming mid-ayah).
    fn skip(&mut self, mut n: u64) {
        while n > 0 {
            if self.used >= self.frames.len() && !self.refill() {
                return;
            }
            let take = (self.frames.len() - self.used).min(usize::try_from(n).unwrap_or(usize::MAX));
            self.used += take;
            n -= take as u64;
        }
    }
}

/// Linear resampling from a clip's rate to the stream's, frame by frame,
/// carrying its phase across calls (a no-op when the rates match).
#[derive(Debug)]
pub struct Resample {
    step: f64,
    /// Position of the next output frame between `prev` and the next
    /// input frame (0–1).
    t: f64,
    prev: Option<[f32; 2]>,
}

impl Resample {
    pub fn new(from: u32, to: u32) -> Self {
        Self { step: f64::from(from) / f64::from(to.max(1)), t: 0.0, prev: None }
    }

    pub fn passthrough(&self) -> bool {
        (self.step - 1.0).abs() < f64::EPSILON
    }

    /// Feeds one input frame; pushes the output frames it completes.
    pub fn push(&mut self, cur: [f32; 2], out: &mut Vec<[f32; 2]>) {
        if self.passthrough() {
            out.push(cur);
            return;
        }
        let Some(prev) = self.prev else {
            self.prev = Some(cur);
            return;
        };
        while self.t < 1.0 {
            let t = self.t as f32;
            out.push([prev[0] + (cur[0] - prev[0]) * t, prev[1] + (cur[1] - prev[1]) * t]);
            self.t += self.step;
        }
        self.t -= 1.0;
        self.prev = Some(cur);
    }
}

// ── The player ───────────────────────────────────────────────────────────

/// Where a recitation stands.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Stopped,
    /// Waiting for the first (or next) file.
    Loading,
    Playing,
    Paused,
}

/// What the player tells its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Status(Status),
    /// The ayah now being recited (global number).
    Ayah(u16),
    /// The queue played to its end.
    Finished,
    Error(String),
}

/// What is playing: the queue, where in it, and how.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub status: Status,
    pub queue: Vec<u16>,
    pub index: usize,
    pub reciter: String,
    pub error: Option<String>,
}

impl Snapshot {
    /// The ayah being recited (or paused at).
    pub fn current(&self) -> Option<u16> {
        (self.status != Status::Stopped).then(|| self.queue.get(self.index).copied()).flatten()
    }
}

struct Ctl {
    /// Bumped by every play / pause / resume / stop: a session whose
    /// generation is no longer current ends.
    generation: u64,
    snap: Snapshot,
    /// Source frames of the current ayah already sent (to resume from).
    sent: u64,
    /// The current clip's sample rate (to turn the latency into frames).
    rate: u32,
}

struct Inner {
    ctl: Mutex<Ctl>,
    /// Wakes the prefetcher (the queue moved on) and the audio thread (a
    /// file arrived).
    wake: Condvar,
    /// The output level (RMS of the last block, f32 bits).
    level: AtomicU32,
    on_event: Box<dyn Fn(Event) + Send + Sync>,
}

/// The recitation player (cheap to clone; one per shell).
#[derive(Clone)]
pub struct Player {
    inner: Arc<Inner>,
}

fn lock(m: &Mutex<Ctl>) -> MutexGuard<'_, Ctl> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Player {
    pub fn new(on_event: impl Fn(Event) + Send + Sync + 'static) -> Self {
        let ctl = Ctl { generation: 0, snap: Snapshot::default(), sent: 0, rate: 44_100 };
        Self { inner: Arc::new(Inner { ctl: Mutex::new(ctl), wake: Condvar::new(), level: AtomicU32::new(0), on_event: Box::new(on_event) }) }
    }

    pub fn snapshot(&self) -> Snapshot {
        lock(&self.inner.ctl).snap.clone()
    }

    /// The output level, 0–1 (for the bar's wave).
    pub fn level(&self) -> f32 {
        f32::from_bits(self.inner.level.load(Ordering::Relaxed))
    }

    /// Recites `queue` (global numbers) from its start with `reciter`.
    pub fn play(&self, queue: Vec<u16>, reciter: &str) {
        if queue.is_empty() {
            return;
        }
        let first = queue[0];
        let generation = {
            let mut c = lock(&self.inner.ctl);
            c.generation += 1;
            c.snap = Snapshot { status: Status::Loading, queue, index: 0, reciter: reciter.to_string(), error: None };
            c.sent = 0;
            c.generation
        };
        self.inner.wake.notify_all();
        (self.inner.on_event)(Event::Status(Status::Loading));
        (self.inner.on_event)(Event::Ayah(first));
        self.spawn(generation, 0);
    }

    /// Pauses (the threads end; the place is kept); whether it was playing.
    pub fn pause(&self) -> bool {
        {
            let mut c = lock(&self.inner.ctl);
            if !matches!(c.snap.status, Status::Playing | Status::Loading) {
                return false;
            }
            c.generation += 1;
            c.snap.status = Status::Paused;
        }
        self.inner.wake.notify_all();
        self.inner.level.store(0, Ordering::Relaxed);
        (self.inner.on_event)(Event::Status(Status::Paused));
        true
    }

    /// Resumes a paused recitation where it was; whether it was paused.
    pub fn resume(&self) -> bool {
        let (generation, skip) = {
            let mut c = lock(&self.inner.ctl);
            if c.snap.status != Status::Paused {
                return false;
            }
            c.generation += 1;
            c.snap.status = Status::Loading;
            // What was sent but still in the server's buffer wasn't heard.
            let unheard = (TARGET_LATENCY.as_secs_f64() * f64::from(c.rate)) as u64;
            (c.generation, c.sent.saturating_sub(unheard))
        };
        (self.inner.on_event)(Event::Status(Status::Loading));
        self.spawn(generation, skip);
        true
    }

    /// Stops and forgets the queue.
    pub fn stop(&self) {
        let was = {
            let mut c = lock(&self.inner.ctl);
            c.generation += 1;
            let was = c.snap.status;
            c.snap = Snapshot::default();
            was
        };
        self.inner.wake.notify_all();
        self.inner.level.store(0, Ordering::Relaxed);
        if was != Status::Stopped {
            (self.inner.on_event)(Event::Status(Status::Stopped));
        }
    }

    fn spawn(&self, generation: u64, skip: u64) {
        let ready: Arc<Mutex<Vec<Option<Result<PathBuf, String>>>>> = Arc::default();
        let (inner, r) = (self.inner.clone(), ready.clone());
        let fetch = std::thread::Builder::new().name("mandara-quran-fetch".into()).spawn(move || prefetch(&inner, generation, &r));
        let inner = self.inner.clone();
        let audio = std::thread::Builder::new().name("mandara-quran-recite".into()).spawn(move || {
            if let Err(e) = session(&inner, generation, skip, &ready) {
                inner.fail(generation, e);
            }
        });
        if let Err(e) = fetch.and(audio) {
            self.inner.fail(generation, e.to_string());
        }
    }
}

impl Inner {
    fn current(&self, generation: u64) -> bool {
        lock(&self.ctl).generation == generation
    }

    /// Ends the session `generation` with an error (when still current).
    fn fail(&self, generation: u64, why: String) {
        {
            let mut c = lock(&self.ctl);
            if c.generation != generation {
                return;
            }
            c.generation += 1;
            c.snap.status = Status::Stopped;
            c.snap.error = Some(why.clone());
        }
        self.wake.notify_all();
        self.level.store(0, Ordering::Relaxed);
        tracing::warn!("quran: recitation stopped: {why}");
        (self.on_event)(Event::Error(why));
        (self.on_event)(Event::Status(Status::Stopped));
    }

    fn set_status(&self, generation: u64, status: Status) {
        {
            let mut c = lock(&self.ctl);
            if c.generation != generation || c.snap.status == status {
                return;
            }
            c.snap.status = status;
        }
        (self.on_event)(Event::Status(status));
    }
}

/// Fetches the queue's files ahead of the one playing.
fn prefetch(inner: &Inner, generation: u64, ready: &Mutex<Vec<Option<Result<PathBuf, String>>>>) {
    loop {
        let (queue, index, reciter) = {
            let c = lock(&inner.ctl);
            if c.generation != generation {
                return;
            }
            (c.snap.queue.clone(), c.snap.index, c.snap.reciter.clone())
        };
        let have = ready.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len().max(index);
        if have >= queue.len() {
            return;
        }
        if have > index + PREFETCH {
            let c = lock(&inner.ctl);
            if c.generation == generation {
                let _ = inner.wake.wait_timeout(c, PREFETCH_WAIT);
            }
            continue;
        }
        let result = fetch_audio(&reciter, queue[have]);
        {
            let mut r = ready.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            r.resize(have, None);
            r.push(Some(result));
        }
        inner.wake.notify_all();
    }
}

/// The file of queue item `i` once fetched (`None` while it is still
/// coming; the session ended).
fn wait_file(inner: &Inner, generation: u64, i: usize, ready: &Mutex<Vec<Option<Result<PathBuf, String>>>>, block: bool) -> Option<Result<PathBuf, String>> {
    loop {
        if let Some(Some(r)) = ready.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(i) {
            return Some(r.clone());
        }
        let c = lock(&inner.ctl);
        if c.generation != generation || !block {
            return None;
        }
        let _ = inner.wake.wait_timeout(c, READ_TIMEOUT);
    }
}

/// Opens the stream on the sound server at the first clip's rate: the
/// connection, the stream's channel and the bytes it asks for first.
fn connect(rate: u32) -> Result<(crate::audio::Pulse, u32, usize), String> {
    let mut p = crate::audio::Pulse::connect("Mandara Qur'an")?;
    let mut props = protocol::Props::new();
    props.set(protocol::Prop::MediaName, std::ffi::CString::from(c"Qur'an recitation"));
    props.set(protocol::Prop::ApplicationId, std::ffi::CString::from(c"org.mandara.quran"));
    let target = (TARGET_LATENCY.as_secs_f32() * rate as f32) as u32 * BYTES_PER_FRAME as u32;
    let params = protocol::PlaybackStreamParams {
        sample_spec: protocol::SampleSpec { format: protocol::SampleFormat::Float32Le, channels: CHANNELS as u8, sample_rate: rate },
        channel_map: protocol::ChannelMap::stereo(),
        cvolume: Some(protocol::ChannelVolume::norm(CHANNELS as u8)),
        sink_name: Some(protocol::DEFAULT_SINK.to_owned()),
        buffer_attr: protocol::stream::BufferAttr { max_length: u32::MAX, target_length: target, pre_buffering: u32::MAX, minimum_request_length: u32::MAX, fragment_size: u32::MAX },
        flags: protocol::stream::StreamFlags { adjust_latency: true, ..Default::default() },
        props,
        ..Default::default()
    };
    let reply: protocol::CreatePlaybackStreamReply = p.request(Command::CreatePlaybackStream(params))?;
    if reply.sample_spec.format != protocol::SampleFormat::Float32Le || usize::from(reply.sample_spec.channels) != CHANNELS {
        return Err(format!("unexpected stream format {:?}", reply.sample_spec));
    }
    p.sock.get_ref().set_read_timeout(Some(READ_TIMEOUT)).map_err(|e| e.to_string())?;
    Ok((p, reply.channel, reply.requested_bytes as usize))
}

fn is_timeout(e: &protocol::ProtocolError) -> bool {
    matches!(e, protocol::ProtocolError::Io(io) if matches!(io.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
}

/// The audio thread: plays the queue from the current item (skipping
/// `skip` frames of it) until the end, a pause or a stop.
fn session(inner: &Inner, generation: u64, skip: u64, ready: &Mutex<Vec<Option<Result<PathBuf, String>>>>) -> Result<(), String> {
    let mut index = lock(&inner.ctl).snap.index;
    let Some(first) = wait_file(inner, generation, index, ready, true) else { return Ok(()) };
    let mut clip = Clip::open(&first?)?;
    clip.skip(skip);
    let rate = clip.rate;
    let mut sent = skip;
    {
        let mut c = lock(&inner.ctl);
        if c.generation != generation {
            return Ok(());
        }
        c.rate = rate;
        c.sent = sent;
    }
    let (mut p, channel, mut owed) = connect(rate)?;
    if !inner.current(generation) {
        return Ok(());
    }
    inner.set_status(generation, Status::Playing);
    let mut resample = Resample::new(clip.rate, rate);
    let mut clip = Some(clip);
    let mut fade = 0usize;
    let fade_len = (FADE_IN.as_secs_f32() * rate as f32) as usize;
    let mut frames: Vec<[f32; 2]> = Vec::with_capacity(MAX_WRITE_FRAMES + 4);
    let mut bytes: Vec<u8> = Vec::with_capacity(MAX_WRITE_FRAMES * BYTES_PER_FRAME + 64);
    let mut finished = false;
    loop {
        if !inner.current(generation) {
            return Ok(());
        }
        // Fill what the server asked for.
        while owed >= BYTES_PER_FRAME && !finished {
            let want = (owed / BYTES_PER_FRAME).min(MAX_WRITE_FRAMES);
            frames.clear();
            let mut waiting = false;
            while frames.len() < want {
                let Some(c) = clip.as_mut() else {
                    // The next ayah: is its file here?
                    let queue_len = lock(&inner.ctl).snap.queue.len();
                    if index + 1 >= queue_len {
                        finished = true;
                        break;
                    }
                    match wait_file(inner, generation, index + 1, ready, false) {
                        None => {
                            waiting = true;
                            break;
                        }
                        Some(Err(e)) => return Err(e),
                        Some(Ok(path)) => {
                            let next = Clip::open(&path)?;
                            resample = Resample::new(next.rate, rate);
                            index += 1;
                            sent = 0;
                            let g = {
                                let mut c = lock(&inner.ctl);
                                if c.generation != generation {
                                    return Ok(());
                                }
                                c.snap.index = index;
                                c.sent = 0;
                                c.rate = next.rate;
                                c.snap.queue.get(index).copied()
                            };
                            inner.wake.notify_all();
                            if let Some(g) = g {
                                (inner.on_event)(Event::Ayah(g));
                            }
                            clip = Some(next);
                            continue;
                        }
                    }
                };
                match c.next() {
                    Some(f) => {
                        sent += 1;
                        resample.push(f, &mut frames);
                    }
                    None => clip = None,
                }
            }
            if waiting && frames.is_empty() {
                inner.set_status(generation, Status::Loading);
                break;
            }
            inner.set_status(generation, Status::Playing);
            bytes.clear();
            let mut energy = 0.0f32;
            for f in &frames {
                let g = if fade < fade_len { fade as f32 / fade_len as f32 } else { 1.0 };
                fade += 1;
                for s in f {
                    let v = (s * g).clamp(-1.0, 1.0);
                    energy += v * v;
                    bytes.extend_from_slice(&v.to_le_bytes());
                }
            }
            if !frames.is_empty() {
                let rms = (energy / (frames.len() * CHANNELS) as f32).sqrt();
                inner.level.store(rms.to_bits(), Ordering::Relaxed);
                p.write_data(channel, &bytes)?;
                owed = owed.saturating_sub(bytes.len());
            }
            {
                let mut c = lock(&inner.ctl);
                if c.generation == generation {
                    c.sent = sent;
                }
            }
            if waiting {
                break;
            }
        }
        if finished {
            std::thread::sleep(TARGET_LATENCY + TAIL);
            let done = {
                let mut c = lock(&inner.ctl);
                if c.generation != generation {
                    return Ok(());
                }
                c.generation += 1;
                c.snap.status = Status::Stopped;
                true
            };
            inner.level.store(0, Ordering::Relaxed);
            if done {
                (inner.on_event)(Event::Finished);
                (inner.on_event)(Event::Status(Status::Stopped));
            }
            return Ok(());
        }
        match protocol::read_command_message(&mut p.sock, p.version) {
            Ok((_, Command::Request(protocol::Request { length, .. }))) => owed += length as usize,
            Ok((_, Command::PlaybackStreamKilled(_))) => return Err(mandara_i18n::t("The sound server closed the stream").into()),
            Ok((_, Command::Error(e))) => return Err(format!("{e:?}")),
            Ok(_) => {}
            // Nothing asked (or a file still coming): look again.
            Err(e) if is_timeout(&e) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reciters_and_files() {
        assert_eq!(reciter_or_default("nope").id, DEFAULT_RECITER);
        assert!(RECITERS.iter().all(|r| r.id.starts_with("ar.") && !r.name.is_empty() && !r.arabic.is_empty()));
        assert_eq!(audio_url("ar.alafasy", 262), "https://cdn.islamic.network/quran/audio/128/ar.alafasy/262.mp3");
        assert!(audio_path("ar.alafasy", 262).ends_with("quran/audio/ar.alafasy/262.mp3"));
        assert!(audio_path("../x", 1).ends_with("audio/x/1.mp3"));
        assert!(looks_like_mp3(b"ID3\x04\x00") && looks_like_mp3(&[0xFF, 0xFB, 0x90]));
        assert!(!looks_like_mp3(b"<?xml version") && !looks_like_mp3(&[0xFF]));
    }

    #[test]
    fn resampling_keeps_time_and_shape() {
        let input: Vec<[f32; 2]> = (0..2205).map(|i| [i as f32, -(i as f32)]).collect();
        // Same rate: untouched.
        let mut same = Resample::new(44_100, 44_100);
        let mut out = Vec::new();
        input.iter().for_each(|f| same.push(*f, &mut out));
        assert_eq!(out, input);
        // 22.05 kHz → 44.1 kHz: twice the frames, halfway values between.
        let mut up = Resample::new(22_050, 44_100);
        let mut out = Vec::new();
        input.iter().for_each(|f| up.push(*f, &mut out));
        assert!((out.len() as i64 - 2 * 2204).abs() <= 2, "{}", out.len());
        assert_eq!(out[0], [0.0, 0.0]);
        assert!((out[1][0] - 0.5).abs() < 1e-4 && (out[3][1] + 1.5).abs() < 1e-4);
        // Fed in pieces, the same result.
        let mut a = Resample::new(44_100, 48_000);
        let mut b = Resample::new(44_100, 48_000);
        let (mut whole, mut parts) = (Vec::new(), Vec::new());
        input.iter().for_each(|f| a.push(*f, &mut whole));
        for chunk in input.chunks(97) {
            chunk.iter().for_each(|f| b.push(*f, &mut parts));
        }
        assert_eq!(whole, parts);
        assert!((whole.len() as f64 - 2204.0 * 48_000.0 / 44_100.0).abs() < 2.0);
    }

    #[test]
    fn the_player_reports_its_state_without_a_sound_server() {
        let events: Arc<Mutex<Vec<Event>>> = Arc::default();
        let e = events.clone();
        let player = Player::new(move |ev| e.lock().unwrap().push(ev));
        assert_eq!(player.snapshot().status, Status::Stopped);
        assert!(!player.pause() && !player.resume());
        player.play(Vec::new(), DEFAULT_RECITER);
        assert_eq!(player.snapshot().status, Status::Stopped, "nothing to play");
        let snap = Snapshot { status: Status::Paused, queue: vec![8, 9], index: 1, reciter: DEFAULT_RECITER.into(), error: None };
        assert_eq!(snap.current(), Some(9));
        assert_eq!(Snapshot::default().current(), None);
        player.stop();
        assert!(events.lock().unwrap().is_empty(), "stopping a stopped player says nothing");
    }
}
