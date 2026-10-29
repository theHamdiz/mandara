//! Ambient soundscapes, synthesized in pure Rust (no audio files): sea
//! waves on the Corniche, rain, wind, café murmur, a crackling fireplace
//! and white / pink / brown noise, mixed with per-layer and master volume.
//!
//! The mix plays through the PulseAudio native protocol (PipeWire's
//! `pipewire-pulse` speaks it) as a playback stream named "Mandara
//! Soundscapes", so it shows up in every mixer. Audio is rendered on a
//! dedicated thread that exists only while playing (or fading out); the UI
//! hands parameters over through atomics, and the render path performs no
//! allocations and no locking. Gains glide per sample, so volume moves,
//! play / pause and the sleep timer's fade never click.

use std::ffi::CString;
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pulseaudio::protocol::{self, Command};
use serde::{Deserialize, Serialize};

/// Output sample rate and channel count.
pub const RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
const BYTES_PER_FRAME: usize = CHANNELS * std::mem::size_of::<f32>();
/// Frames rendered between parameter reads.
const BLOCK_FRAMES: usize = 512;
/// Largest single write (bytes); bigger server requests are split.
const MAX_WRITE_BYTES: usize = BLOCK_FRAMES * BYTES_PER_FRAME * 16;
/// Target server-side buffer (keeps volume changes responsive).
const TARGET_LATENCY: Duration = Duration::from_millis(120);
/// Socket read timeout while waiting for the server's requests.
const READ_TIMEOUT: Duration = Duration::from_millis(100);
/// Time constant of the per-sample gain glide.
const GAIN_GLIDE: Duration = Duration::from_millis(40);
/// Play / pause fade.
const PLAY_FADE: Duration = Duration::from_millis(600);
/// The sleep timer fades out over this long before stopping.
pub const SLEEP_FADE: Duration = Duration::from_secs(30);
/// Gains below this count as silent.
const SILENT: f32 = 1e-4;
/// State flushed to zero below this magnitude (no denormals).
const DENORMAL: f32 = 1e-20;
/// Overall headroom before the soft clipper.
const HEADROOM: f32 = 0.9;
/// Default master volume for new installs.
pub const DEFAULT_MASTER: f32 = 0.6;
/// Delay before persisting changed levels (coalesces slider drags).
const SAVE_DELAY: Duration = Duration::from_millis(800);

/// One ambient layer.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Layer {
    Sea,
    Rain,
    Wind,
    Cafe,
    Fire,
    White,
    Pink,
    Brown,
}

pub const LAYERS: usize = 8;

impl Layer {
    pub const ALL: [Layer; LAYERS] = [Layer::Sea, Layer::Rain, Layer::Wind, Layer::Cafe, Layer::Fire, Layer::White, Layer::Pink, Layer::Brown];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn key(self) -> &'static str {
        match self {
            Layer::Sea => "sea",
            Layer::Rain => "rain",
            Layer::Wind => "wind",
            Layer::Cafe => "cafe",
            Layer::Fire => "fire",
            Layer::White => "white",
            Layer::Pink => "pink",
            Layer::Brown => "brown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Layer::ALL.into_iter().find(|l| l.key().eq_ignore_ascii_case(s.trim()))
    }

    /// English label (`tk`-marked; display with `tr`).
    pub fn label(self) -> &'static str {
        use mandara_i18n::tk;
        match self {
            Layer::Sea => tk("Corniche waves"),
            Layer::Rain => tk("Rain"),
            Layer::Wind => tk("Wind"),
            Layer::Cafe => tk("Café murmur"),
            Layer::Fire => tk("Fireplace"),
            Layer::White => tk("White noise"),
            Layer::Pink => tk("Pink noise"),
            Layer::Brown => tk("Brown noise"),
        }
    }
}

/// A named mix.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Preset {
    pub key: &'static str,
    /// English label (`tk`-marked).
    pub label: &'static str,
    pub levels: [f32; LAYERS],
}

/// Levels are in [`Layer::ALL`] order: sea, rain, wind, café, fire, white,
/// pink, brown.
pub const PRESETS: &[Preset] = &[
    Preset { key: "corniche-night", label: mandara_i18n::tk("Corniche night"), levels: [0.85, 0.0, 0.22, 0.0, 0.0, 0.0, 0.0, 0.12] },
    Preset { key: "rainy-study", label: mandara_i18n::tk("Rainy study"), levels: [0.0, 0.75, 0.0, 0.28, 0.22, 0.0, 0.0, 0.1] },
    Preset { key: "deep-focus", label: mandara_i18n::tk("Deep focus"), levels: [0.0, 0.12, 0.0, 0.0, 0.0, 0.0, 0.32, 0.5] },
    Preset { key: "fireside", label: mandara_i18n::tk("Fireside"), levels: [0.0, 0.18, 0.25, 0.0, 0.8, 0.0, 0.0, 0.0] },
];

pub fn preset(key: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.key.eq_ignore_ascii_case(key.trim()))
}

// ── Parameter handoff ────────────────────────────────────────────────────

fn store_f32(a: &AtomicU32, v: f32) {
    a.store(v.to_bits(), Ordering::Relaxed);
}

fn load_f32(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or_default()
}

/// Lock-free parameters shared between the UI and the audio thread.
pub struct Params {
    levels: [AtomicU32; LAYERS],
    master: AtomicU32,
    playing: AtomicBool,
    /// Unix milliseconds when the sleep timer stops playback (0 = off).
    sleep_at: AtomicU64,
    /// Total sleep timer length in ms (for the countdown ring).
    sleep_len: AtomicU64,
    /// The audio thread is alive.
    running: AtomicBool,
    /// Bumped by the audio thread for UI animation (blocks rendered).
    blocks: AtomicU64,
    /// Smoothed output level `0..=1`, written by the audio thread.
    level: AtomicU32,
}

impl Params {
    fn new(levels: [f32; LAYERS], master: f32) -> Self {
        Self {
            levels: levels.map(|v| AtomicU32::new(v.clamp(0.0, 1.0).to_bits())),
            master: AtomicU32::new(master.clamp(0.0, 1.0).to_bits()),
            playing: AtomicBool::new(false),
            sleep_at: AtomicU64::new(0),
            sleep_len: AtomicU64::new(0),
            running: AtomicBool::new(false),
            blocks: AtomicU64::new(0),
            level: AtomicU32::new(0f32.to_bits()),
        }
    }

    pub fn level_of(&self, layer: Layer) -> f32 {
        load_f32(&self.levels[layer.index()])
    }

    pub fn master(&self) -> f32 {
        load_f32(&self.master)
    }

    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Output level for meters / animation.
    pub fn output_level(&self) -> f32 {
        load_f32(&self.level)
    }

    /// Remaining sleep time and the timer's full length.
    pub fn sleep_remaining(&self) -> Option<(Duration, Duration)> {
        let at = self.sleep_at.load(Ordering::Relaxed);
        if at == 0 {
            return None;
        }
        Some((Duration::from_millis(at.saturating_sub(now_ms())), Duration::from_millis(self.sleep_len.load(Ordering::Relaxed))))
    }

    fn levels(&self) -> [f32; LAYERS] {
        std::array::from_fn(|i| load_f32(&self.levels[i]))
    }
}

// ── DSP building blocks (allocation-free) ────────────────────────────────

/// xorshift32: fast, deterministic noise.
#[derive(Clone, Copy)]
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        Self(seed.max(1))
    }

    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f32 {
        (self.next() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Uniform in `[-1, 1)`.
    fn white(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

#[inline]
fn flush(x: f32) -> f32 {
    if x.abs() < DENORMAL { 0.0 } else { x }
}

/// One-pole smoothing coefficient for a cutoff in Hz.
#[inline]
fn one_pole(hz: f32) -> f32 {
    1.0 - (-std::f32::consts::TAU * hz / RATE as f32).exp()
}

/// Per-sample glide coefficient for a time constant.
fn glide(time: Duration) -> f32 {
    1.0 - (-1.0 / (time.as_secs_f32() * RATE as f32)).exp()
}

#[derive(Clone, Copy, Default)]
struct Lowpass {
    y: f32,
}

impl Lowpass {
    #[inline]
    fn run(&mut self, x: f32, a: f32) -> f32 {
        self.y = flush(self.y + a * (x - self.y));
        self.y
    }
}

#[derive(Clone, Copy, Default)]
struct Highpass {
    lp: Lowpass,
}

impl Highpass {
    #[inline]
    fn run(&mut self, x: f32, a: f32) -> f32 {
        x - self.lp.run(x, a)
    }
}

/// RBJ band-pass (constant 0 dB peak).
#[derive(Clone, Copy, Default)]
struct Bandpass {
    b0: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Bandpass {
    fn tune(&mut self, hz: f32, q: f32) {
        let w0 = std::f32::consts::TAU * hz.clamp(20.0, RATE as f32 * 0.45) / RATE as f32;
        let alpha = w0.sin() / (2.0 * q.max(0.1));
        let a0 = 1.0 + alpha;
        self.b0 = alpha / a0;
        self.b2 = -alpha / a0;
        self.a1 = -2.0 * w0.cos() / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = flush(y);
        self.y1
    }
}

/// Paul Kellet's pink noise filter.
#[derive(Clone, Copy, Default)]
struct Pink {
    b: [f32; 7],
}

impl Pink {
    #[inline]
    fn run(&mut self, w: f32) -> f32 {
        let b = &mut self.b;
        b[0] = flush(0.99886 * b[0] + w * 0.0555179);
        b[1] = flush(0.99332 * b[1] + w * 0.0750759);
        b[2] = flush(0.96900 * b[2] + w * 0.1538520);
        b[3] = flush(0.86650 * b[3] + w * 0.3104856);
        b[4] = flush(0.55000 * b[4] + w * 0.5329522);
        b[5] = flush(-0.7616 * b[5] - w * 0.0168980);
        let out = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + w * 0.5362;
        b[6] = w * 0.115926;
        out * 0.11
    }
}

/// Leaky-integrated (brown / red) noise.
#[derive(Clone, Copy, Default)]
struct Brown {
    y: f32,
}

impl Brown {
    #[inline]
    fn run(&mut self, w: f32) -> f32 {
        self.y = flush((self.y + 0.02 * w) / 1.02);
        self.y * 3.5
    }
}

/// A slow random control signal (low-passed noise, rescaled to `0..=1`).
#[derive(Clone, Copy)]
struct Drift {
    rng: Rng,
    target: f32,
    value: f32,
    countdown: u32,
    period: u32,
    rate: f32,
}

impl Drift {
    fn new(seed: u32, seconds: f32) -> Self {
        let period = (seconds * RATE as f32) as u32;
        Self { rng: Rng::new(seed), target: 0.5, value: 0.5, countdown: 0, period: period.max(1), rate: glide(Duration::from_secs_f32(seconds * 0.6)) }
    }

    #[inline]
    fn run(&mut self) -> f32 {
        if self.countdown == 0 {
            self.target = self.rng.unit();
            self.countdown = self.period / 2 + (self.rng.unit() * self.period as f32) as u32;
        }
        self.countdown -= 1;
        self.value += (self.target - self.value) * self.rate;
        self.value
    }
}

/// Short decaying noise bursts (rain drops, fire pops) in a fixed pool.
#[derive(Clone, Copy, Default)]
struct Burst {
    amp: f32,
    decay: f32,
    pan: f32,
    tone: Lowpass,
}

const BURSTS: usize = 24;

#[derive(Clone, Copy)]
struct Bursts {
    pool: [Burst; BURSTS],
    next: usize,
}

impl Bursts {
    fn new() -> Self {
        Self { pool: [Burst::default(); BURSTS], next: 0 }
    }

    fn trigger(&mut self, amp: f32, decay_ms: f32, pan: f32) {
        let decay = (-1.0 / (decay_ms * 0.001 * RATE as f32)).exp();
        self.pool[self.next] = Burst { amp, decay, pan, tone: Lowpass::default() };
        self.next = (self.next + 1) % BURSTS;
    }

    #[inline]
    fn run(&mut self, rng: &mut Rng, tone: f32) -> (f32, f32) {
        let (mut l, mut r) = (0.0, 0.0);
        for b in self.pool.iter_mut() {
            if b.amp < SILENT {
                continue;
            }
            let s = b.tone.run(rng.white(), tone) * b.amp;
            l += s * (1.0 - b.pan);
            r += s * (1.0 + b.pan);
            b.amp = flush(b.amp * b.decay);
        }
        (l, r)
    }
}

// ── Layers ───────────────────────────────────────────────────────────────

/// Mediterranean waves: low-passed noise whose cutoff and level follow a
/// swell envelope (slow rise, crash, long hissing recede), two staggered
/// wave trains for stereo width, over a soft brown-noise undertow.
#[derive(Clone, Copy)]
struct Sea {
    rng: Rng,
    phase: [f32; 2],
    step: [f32; 2],
    tone: [Lowpass; 2],
    hiss: [Highpass; 2],
    undertow: Brown,
}

impl Sea {
    const MIN_PERIOD: f32 = 7.0;
    const MAX_PERIOD: f32 = 12.5;

    fn new(seed: u32) -> Self {
        let mut rng = Rng::new(seed);
        let mut step = || 1.0 / (rng.range(Self::MIN_PERIOD, Self::MAX_PERIOD) * RATE as f32);
        let step = [step(), step()];
        Self { rng, phase: [0.0, 0.45], step, tone: [Lowpass::default(); 2], hiss: [Highpass::default(); 2], undertow: Brown::default() }
    }

    /// Swell envelope over one wave (`0..1` phase).
    fn swell(p: f32) -> f32 {
        const CREST: f32 = 0.38;
        if p < CREST {
            let t = p / CREST;
            t * t * (3.0 - 2.0 * t)
        } else {
            (-(p - CREST) * 3.6).exp()
        }
    }

    #[inline]
    fn run(&mut self) -> (f32, f32) {
        let under = self.undertow.run(self.rng.white()) * 0.25;
        let mut out = [0.0f32; 2];
        for ch in 0..2 {
            self.phase[ch] += self.step[ch];
            if self.phase[ch] >= 1.0 {
                self.phase[ch] -= 1.0;
                self.step[ch] = 1.0 / (self.rng.range(Self::MIN_PERIOD, Self::MAX_PERIOD) * RATE as f32);
            }
            let env = Self::swell(self.phase[ch]);
            let cutoff = 260.0 + env * 2600.0;
            let body = self.tone[ch].run(self.rng.white(), one_pole(cutoff));
            let foam = self.hiss[ch].run(self.rng.white(), one_pole(1800.0)) * env * env * 0.12;
            out[ch] = (body * (0.18 + 0.95 * env) + foam) * 1.6 + under;
        }
        (out[0], out[1])
    }
}

/// Rain: a steady band-limited hiss plus individual drops.
#[derive(Clone, Copy)]
struct Rain {
    rng: Rng,
    hiss_hp: [Highpass; 2],
    hiss_lp: [Lowpass; 2],
    drops: Bursts,
    density: Drift,
}

impl Rain {
    /// Average drops per second at full density.
    const DROPS_PER_SEC: f32 = 55.0;

    fn new(seed: u32) -> Self {
        Self { rng: Rng::new(seed), hiss_hp: [Highpass::default(); 2], hiss_lp: [Lowpass::default(); 2], drops: Bursts::new(), density: Drift::new(seed ^ 0x9e37, 6.0) }
    }

    #[inline]
    fn run(&mut self) -> (f32, f32) {
        let density = 0.55 + 0.45 * self.density.run();
        if self.rng.unit() < Self::DROPS_PER_SEC * density / RATE as f32 {
            let amp = self.rng.range(0.08, 0.5);
            let decay = self.rng.range(2.0, 9.0);
            let pan = self.rng.white() * 0.8;
            self.drops.trigger(amp, decay, pan);
        }
        let (dl, dr) = self.drops.run(&mut self.rng, one_pole(4200.0));
        let (hp, lp) = (one_pole(900.0), one_pole(7000.0));
        let mut hiss = [0.0f32; 2];
        for (ch, h) in hiss.iter_mut().enumerate() {
            *h = self.hiss_lp[ch].run(self.hiss_hp[ch].run(self.rng.white(), hp), lp) * 0.32 * density;
        }
        (hiss[0] + dl, hiss[1] + dr)
    }
}

/// Wind: band-passed noise whose centre and level drift slowly (gusts).
#[derive(Clone, Copy)]
struct Wind {
    rng: Rng,
    band: [Bandpass; 2],
    pitch: Drift,
    gust: Drift,
    retune: u32,
}

impl Wind {
    const RETUNE_EVERY: u32 = 64;

    fn new(seed: u32) -> Self {
        let mut band = [Bandpass::default(); 2];
        for b in &mut band {
            b.tune(400.0, 1.6);
        }
        Self { rng: Rng::new(seed), band, pitch: Drift::new(seed ^ 0x51ed, 4.5), gust: Drift::new(seed ^ 0x2c1b, 3.0), retune: 0 }
    }

    #[inline]
    fn run(&mut self) -> (f32, f32) {
        let pitch = self.pitch.run();
        let gust = self.gust.run();
        if self.retune == 0 {
            self.band[0].tune(180.0 + pitch * 700.0, 1.4 + gust);
            self.band[1].tune(200.0 + pitch * 760.0, 1.4 + gust);
            self.retune = Self::RETUNE_EVERY;
        }
        self.retune -= 1;
        let level = (0.25 + gust * gust * 1.1) * 1.9;
        (self.band[0].run(self.rng.white()) * level, self.band[1].run(self.rng.white()) * level)
    }
}

/// Café murmur: several band-passed "voices" with syllable-rate envelopes
/// over room tone, kept dark and distant, plus the odd cup clink.
#[derive(Clone, Copy)]
struct Cafe {
    rng: Rng,
    voices: [Voice; Cafe::VOICES],
    room: Brown,
    room_lp: Lowpass,
    clink_phase: f32,
    clink_step: f32,
    clink_amp: f32,
    clink_decay: f32,
    dark: [Lowpass; 2],
}

#[derive(Clone, Copy, Default)]
struct Voice {
    band: Bandpass,
    env: f32,
    target: f32,
    countdown: u32,
    pan: f32,
}

impl Cafe {
    const VOICES: usize = 7;
    const CLINKS_PER_SEC: f32 = 0.25;

    fn new(seed: u32) -> Self {
        let mut rng = Rng::new(seed);
        let voices = std::array::from_fn(|_| {
            let mut v = Voice { pan: rng.white() * 0.7, ..Default::default() };
            v.band.tune(rng.range(350.0, 2200.0), 4.0);
            v
        });
        Self { rng, voices, room: Brown::default(), room_lp: Lowpass::default(), clink_phase: 0.0, clink_step: 0.0, clink_amp: 0.0, clink_decay: 0.0, dark: [Lowpass::default(); 2] }
    }

    #[inline]
    fn run(&mut self) -> (f32, f32) {
        let (mut l, mut r) = (0.0f32, 0.0f32);
        let follow = one_pole(18.0);
        for v in self.voices.iter_mut() {
            if v.countdown == 0 {
                // A new syllable (or a pause), with a new formant.
                let talking = self.rng.unit() < 0.7;
                v.target = if talking { self.rng.range(0.35, 1.0) } else { 0.0 };
                v.countdown = (self.rng.range(0.09, 0.32) * RATE as f32) as u32;
                v.band.tune(self.rng.range(300.0, 2400.0), 5.0);
            }
            v.countdown -= 1;
            v.env = flush(v.env + (v.target - v.env) * follow);
            let s = v.band.run(self.rng.white()) * v.env;
            l += s * (1.0 - v.pan);
            r += s * (1.0 + v.pan);
        }
        if self.clink_amp < SILENT && self.rng.unit() < Self::CLINKS_PER_SEC / RATE as f32 {
            self.clink_amp = self.rng.range(0.04, 0.12);
            self.clink_step = std::f32::consts::TAU * self.rng.range(2200.0, 3800.0) / RATE as f32;
            self.clink_decay = (-1.0 / (0.09 * RATE as f32)).exp();
        }
        let clink = if self.clink_amp >= SILENT {
            self.clink_phase = (self.clink_phase + self.clink_step) % std::f32::consts::TAU;
            self.clink_amp = flush(self.clink_amp * self.clink_decay);
            self.clink_phase.sin() * self.clink_amp
        } else {
            0.0
        };
        let room = self.room_lp.run(self.room.run(self.rng.white()), one_pole(500.0)) * 0.35;
        let dark = one_pole(2600.0);
        (self.dark[0].run(l * 0.55, dark) + room + clink * 0.8, self.dark[1].run(r * 0.55, dark) + room + clink)
    }
}

/// Fireplace: a flickering low roar plus crackles and pops.
#[derive(Clone, Copy)]
struct Fire {
    rng: Rng,
    roar: Brown,
    roar_lp: Lowpass,
    flicker: Drift,
    pops: Bursts,
    pop_hp: [Highpass; 2],
    burst_left: u32,
    burst_gap: u32,
}

impl Fire {
    const CRACKLES_PER_SEC: f32 = 5.0;

    fn new(seed: u32) -> Self {
        Self {
            rng: Rng::new(seed),
            roar: Brown::default(),
            roar_lp: Lowpass::default(),
            flicker: Drift::new(seed ^ 0x7f4a, 0.8),
            pops: Bursts::new(),
            pop_hp: [Highpass::default(); 2],
            burst_left: 0,
            burst_gap: 0,
        }
    }

    #[inline]
    fn run(&mut self) -> (f32, f32) {
        // Crackles come in little clusters of 1–5 pops.
        if self.burst_left == 0 && self.rng.unit() < Self::CRACKLES_PER_SEC / RATE as f32 {
            self.burst_left = 1 + (self.rng.unit() * 5.0) as u32;
            self.burst_gap = 0;
        }
        if self.burst_left > 0 {
            if self.burst_gap == 0 {
                let amp = self.rng.range(0.2, 0.9);
                let decay = self.rng.range(0.4, 2.5);
                let pan = self.rng.white() * 0.6;
                self.pops.trigger(amp, decay, pan);
                self.burst_left -= 1;
                self.burst_gap = (self.rng.range(0.008, 0.06) * RATE as f32) as u32;
            } else {
                self.burst_gap -= 1;
            }
        }
        let (pl, pr) = self.pops.run(&mut self.rng, one_pole(9000.0));
        let hp = one_pole(700.0);
        let flicker = 0.55 + 0.45 * self.flicker.run();
        let roar = self.roar_lp.run(self.roar.run(self.rng.white()), one_pole(260.0)) * flicker * 1.4;
        (roar + self.pop_hp[0].run(pl, hp), roar + self.pop_hp[1].run(pr, hp))
    }
}

/// The whole mixer. `render` is allocation- and lock-free.
pub struct Engine {
    sea: Sea,
    rain: Rain,
    wind: Wind,
    cafe: Cafe,
    fire: Fire,
    white: Rng,
    pink: [Pink; 2],
    brown: [Brown; 2],
    noise_rng: Rng,
    gains: [f32; LAYERS],
    master: f32,
    play: f32,
    glide: f32,
    play_glide: f32,
    level: f32,
}

impl Engine {
    pub fn new(seed: u32) -> Self {
        Self {
            sea: Sea::new(seed ^ 0x1111),
            rain: Rain::new(seed ^ 0x2222),
            wind: Wind::new(seed ^ 0x3333),
            cafe: Cafe::new(seed ^ 0x4444),
            fire: Fire::new(seed ^ 0x5555),
            white: Rng::new(seed ^ 0x6666),
            pink: [Pink::default(); 2],
            brown: [Brown::default(); 2],
            noise_rng: Rng::new(seed ^ 0x7777),
            gains: [0.0; LAYERS],
            master: 0.0,
            play: 0.0,
            glide: glide(GAIN_GLIDE),
            play_glide: glide(PLAY_FADE / 4),
            level: 0.0,
        }
    }

    /// Whether everything has faded to silence.
    pub fn is_silent(&self) -> bool {
        self.play < SILENT
    }

    /// Fills interleaved stereo `out` (`frames × 2`), gliding gains toward
    /// `levels`, `master` and `play` (0 = paused, 1 = playing, or a sleep
    /// fade in between).
    pub fn render(&mut self, out: &mut [f32], levels: &[f32; LAYERS], master: f32, play: f32) {
        let mut sum_sq = 0.0f32;
        for frame in out.chunks_exact_mut(CHANNELS) {
            self.play = flush(self.play + (play - self.play) * self.play_glide);
            self.master = flush(self.master + (master - self.master) * self.glide);
            let (mut l, mut r) = (0.0f32, 0.0f32);
            for (i, layer) in Layer::ALL.iter().enumerate() {
                self.gains[i] = flush(self.gains[i] + (levels[i] - self.gains[i]) * self.glide);
                let g = self.gains[i];
                if g < SILENT && levels[i] < SILENT {
                    continue;
                }
                let (a, b) = match layer {
                    Layer::Sea => self.sea.run(),
                    Layer::Rain => self.rain.run(),
                    Layer::Wind => self.wind.run(),
                    Layer::Cafe => self.cafe.run(),
                    Layer::Fire => self.fire.run(),
                    Layer::White => (self.white.white() * 0.22, self.white.white() * 0.22),
                    Layer::Pink => (self.pink[0].run(self.noise_rng.white()), self.pink[1].run(self.noise_rng.white())),
                    Layer::Brown => (self.brown[0].run(self.noise_rng.white()), self.brown[1].run(self.noise_rng.white())),
                };
                // Perceptual (squared) volume curve.
                let k = g * g;
                l += a * k;
                r += b * k;
            }
            let k = self.master * self.master * self.play * HEADROOM;
            let (l, r) = (soft_clip(l * k), soft_clip(r * k));
            frame[0] = l;
            frame[1] = r;
            sum_sq += l * l + r * r;
        }
        let frames = (out.len() / CHANNELS).max(1) as f32;
        let rms = (sum_sq / (2.0 * frames)).sqrt();
        self.level += (rms - self.level) * 0.3;
    }

    pub fn level(&self) -> f32 {
        self.level
    }
}

/// Rational tanh approximation: transparent at normal levels, rounds off
/// peaks when many layers stack up.
#[inline]
fn soft_clip(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}

// ── Persistence ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Saved {
    master: f32,
    levels: std::collections::BTreeMap<String, f32>,
}

fn state_path() -> std::path::PathBuf {
    dirs::state_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/state")).join("mandara").join("soundscapes.toml")
}

fn load_saved() -> ([f32; LAYERS], f32) {
    let saved = std::fs::read_to_string(state_path()).ok().and_then(|t| toml::from_str::<Saved>(&t).ok());
    match saved {
        Some(s) => (std::array::from_fn(|i| s.levels.get(Layer::ALL[i].key()).copied().unwrap_or(0.0).clamp(0.0, 1.0)), s.master.clamp(0.0, 1.0)),
        None => (PRESETS[0].levels, DEFAULT_MASTER),
    }
}

fn save(levels: [f32; LAYERS], master: f32) {
    let saved = Saved { master, levels: Layer::ALL.iter().map(|l| (l.key().to_string(), levels[l.index()])).collect() };
    let path = state_path();
    if let (Some(dir), Ok(text)) = (path.parent(), toml::to_string(&saved)) {
        let _ = std::fs::create_dir_all(dir);
        let tmp = path.with_extension("toml.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }
}

// ── Handle ───────────────────────────────────────────────────────────────

/// The UI's handle: setters are lock-free; starting playback spawns the
/// audio thread (which exits once paused and faded out).
#[derive(Clone)]
pub struct Soundscapes {
    params: Arc<Params>,
    error: Arc<Mutex<Option<String>>>,
    save_pending: Arc<AtomicBool>,
}

impl Default for Soundscapes {
    fn default() -> Self {
        Self::new()
    }
}

impl Soundscapes {
    /// Loads the last levels (or the Corniche night preset).
    pub fn new() -> Self {
        let (levels, master) = load_saved();
        Self { params: Arc::new(Params::new(levels, master)), error: Arc::default(), save_pending: Arc::default() }
    }

    pub fn params(&self) -> &Params {
        &self.params
    }

    /// The last playback error (no sound server, stream killed…).
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn changed(&self) {
        if self.save_pending.swap(true, Ordering::SeqCst) {
            return;
        }
        let params = self.params.clone();
        let pending = self.save_pending.clone();
        let _ = std::thread::Builder::new().name("mandara-soundscapes-save".into()).spawn(move || {
            std::thread::sleep(SAVE_DELAY);
            pending.store(false, Ordering::SeqCst);
            save(params.levels(), params.master());
        });
    }

    pub fn set_level(&self, layer: Layer, value: f32) {
        store_f32(&self.params.levels[layer.index()], value.clamp(0.0, 1.0));
        self.changed();
    }

    pub fn set_master(&self, value: f32) {
        store_f32(&self.params.master, value.clamp(0.0, 1.0));
        self.changed();
    }

    pub fn apply_preset(&self, preset: &Preset) {
        for layer in Layer::ALL {
            store_f32(&self.params.levels[layer.index()], preset.levels[layer.index()]);
        }
        self.changed();
    }

    /// The preset whose levels match the current mix, if any.
    pub fn current_preset(&self) -> Option<&'static Preset> {
        let levels = self.params.levels();
        PRESETS.iter().find(|p| p.levels.iter().zip(levels).all(|(a, b)| (a - b).abs() < 0.01))
    }

    pub fn play(&self) -> Result<(), String> {
        pulseaudio::socket_path_from_env().ok_or_else(|| mandara_i18n::t("No sound server (PulseAudio / PipeWire) found").to_string())?;
        *self.error.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.params.playing.store(true, Ordering::SeqCst);
        if !self.params.running.swap(true, Ordering::SeqCst) {
            let params = self.params.clone();
            let error = self.error.clone();
            let spawned = std::thread::Builder::new().name("mandara-soundscapes".into()).spawn(move || {
                if let Err(e) = playback(&params) {
                    tracing::warn!("soundscapes playback stopped: {e}");
                    *error.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
                    params.playing.store(false, Ordering::SeqCst);
                }
                store_f32(&params.level, 0.0);
                params.running.store(false, Ordering::SeqCst);
            });
            if let Err(e) = spawned {
                self.params.running.store(false, Ordering::SeqCst);
                self.params.playing.store(false, Ordering::SeqCst);
                return Err(e.to_string());
            }
        }
        Ok(())
    }

    /// Fades out; the audio thread exits when silent.
    pub fn pause(&self) {
        self.params.playing.store(false, Ordering::SeqCst);
        self.cancel_sleep();
    }

    pub fn toggle(&self) -> Result<bool, String> {
        if self.params.is_playing() {
            self.pause();
            Ok(false)
        } else {
            self.play().map(|_| true)
        }
    }

    /// Stops after `after`, fading out over the last [`SLEEP_FADE`].
    pub fn set_sleep(&self, after: Duration) {
        self.params.sleep_len.store(after.as_millis() as u64, Ordering::SeqCst);
        self.params.sleep_at.store(now_ms() + after.as_millis() as u64, Ordering::SeqCst);
    }

    pub fn cancel_sleep(&self) {
        self.params.sleep_at.store(0, Ordering::SeqCst);
        self.params.sleep_len.store(0, Ordering::SeqCst);
    }
}

/// Gain applied by the sleep timer at `now_ms` (1 → 0 over the fade), and
/// whether the timer has run out.
pub fn sleep_gain(sleep_at: u64, now: u64) -> (f32, bool) {
    if sleep_at == 0 {
        return (1.0, false);
    }
    let left = sleep_at.saturating_sub(now) as f32 / MS_PER_S;
    let fade = SLEEP_FADE.as_secs_f32();
    ((left / fade).clamp(0.0, 1.0), now >= sleep_at)
}

/// Milliseconds per second (the sleep timer counts in ms).
const MS_PER_S: f32 = 1000.0;

fn perr(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn is_timeout(e: &protocol::ProtocolError) -> bool {
    matches!(e, protocol::ProtocolError::Io(io) if matches!(io.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
}

/// The audio thread: connects, creates the stream and serves the server's
/// requests until paused and silent.
fn playback(params: &Params) -> Result<(), String> {
    let path = pulseaudio::socket_path_from_env().ok_or("no sound server")?;
    let stream = UnixStream::connect(path).map_err(perr)?;
    stream.set_read_timeout(Some(READ_TIMEOUT)).map_err(perr)?;
    let mut sock = BufReader::new(stream);
    let cookie = crate::audio::pulse_cookie();
    let auth = protocol::AuthParams { version: protocol::MAX_VERSION, supports_shm: false, supports_memfd: false, cookie };
    protocol::write_command_message(sock.get_mut(), 0, &Command::Auth(auth), protocol::MAX_VERSION).map_err(perr)?;
    let (_, reply) = protocol::read_reply_message::<protocol::AuthReply>(&mut sock, protocol::MAX_VERSION).map_err(perr)?;
    let version = protocol::MAX_VERSION.min(reply.version);
    let mut props = protocol::Props::new();
    props.set(protocol::Prop::ApplicationName, CString::from(c"Mandara Soundscapes"));
    props.set(protocol::Prop::ApplicationId, CString::from(c"org.mandara.soundscapes"));
    props.set(protocol::Prop::ApplicationIconName, CString::from(c"weather-showers"));
    protocol::write_command_message(sock.get_mut(), 1, &Command::SetClientName(props), version).map_err(perr)?;
    protocol::read_reply_message::<protocol::SetClientNameReply>(&mut sock, version).map_err(perr)?;

    let mut stream_props = protocol::Props::new();
    stream_props.set(protocol::Prop::MediaName, CString::from(c"Ambient soundscape"));
    let target = (TARGET_LATENCY.as_secs_f32() * RATE as f32) as u32 * BYTES_PER_FRAME as u32;
    let params_msg = protocol::PlaybackStreamParams {
        sample_spec: protocol::SampleSpec { format: protocol::SampleFormat::Float32Le, channels: CHANNELS as u8, sample_rate: RATE },
        channel_map: protocol::ChannelMap::stereo(),
        cvolume: Some(protocol::ChannelVolume::norm(CHANNELS as u8)),
        sink_name: Some(protocol::DEFAULT_SINK.to_owned()),
        buffer_attr: protocol::stream::BufferAttr { max_length: u32::MAX, target_length: target, pre_buffering: u32::MAX, minimum_request_length: u32::MAX, fragment_size: u32::MAX },
        flags: protocol::stream::StreamFlags { adjust_latency: true, ..Default::default() },
        props: stream_props,
        ..Default::default()
    };
    protocol::write_command_message(sock.get_mut(), 2, &Command::CreatePlaybackStream(params_msg), version).map_err(perr)?;
    let (_, info) = protocol::read_reply_message::<protocol::CreatePlaybackStreamReply>(&mut sock, version).map_err(perr)?;
    if info.sample_spec.format != protocol::SampleFormat::Float32Le || info.sample_spec.channels as usize != CHANNELS {
        return Err(format!("unexpected stream format {:?}", info.sample_spec));
    }

    let seed = (now_ms() as u32) ^ std::process::id();
    let mut engine = Engine::new(seed);
    // Pre-allocated buffers: the render path never allocates.
    let mut samples = vec![0f32; MAX_WRITE_BYTES / std::mem::size_of::<f32>()];
    let mut bytes = vec![0u8; MAX_WRITE_BYTES];
    let mut serve = |want: usize, engine: &mut Engine, sock: &mut BufReader<UnixStream>| -> Result<(), String> {
        let mut left = want - want % BYTES_PER_FRAME;
        while left > 0 {
            let chunk = left.min(MAX_WRITE_BYTES);
            let frames = chunk / BYTES_PER_FRAME;
            for block in samples[..frames * CHANNELS].chunks_mut(BLOCK_FRAMES * CHANNELS) {
                let (sleep_fade, expired) = sleep_gain(params.sleep_at.load(Ordering::Relaxed), now_ms());
                if expired {
                    params.playing.store(false, Ordering::SeqCst);
                    params.sleep_at.store(0, Ordering::SeqCst);
                    params.sleep_len.store(0, Ordering::SeqCst);
                }
                let play = if params.playing.load(Ordering::Relaxed) { sleep_fade } else { 0.0 };
                engine.render(block, &params.levels(), params.master(), play);
            }
            for (dst, s) in bytes[..chunk].chunks_exact_mut(std::mem::size_of::<f32>()).zip(&samples[..frames * CHANNELS]) {
                dst.copy_from_slice(&s.to_le_bytes());
            }
            protocol::write_memblock(sock.get_mut(), info.channel, &bytes[..chunk], 0).map_err(perr)?;
            params.blocks.fetch_add(1, Ordering::Relaxed);
            store_f32(&params.level, engine.level());
            left -= chunk;
        }
        Ok(())
    };
    serve(info.requested_bytes as usize, &mut engine, &mut sock)?;
    loop {
        if !params.playing.load(Ordering::Relaxed) && engine.is_silent() {
            return Ok(());
        }
        match protocol::read_command_message(&mut sock, version) {
            Ok((_, Command::Request(protocol::Request { length, .. }))) => serve(length as usize, &mut engine, &mut sock)?,
            Ok((_, Command::PlaybackStreamKilled(_))) => return Err(mandara_i18n::t("The sound server closed the stream").into()),
            Ok((_, Command::Error(e))) => return Err(format!("{e:?}")),
            Ok(_) => {}
            Err(e) if is_timeout(&e) => {}
            Err(e) => return Err(perr(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_layer(layer: Layer, seconds: f32) -> Vec<f32> {
        let mut engine = Engine::new(0xC0FFEE);
        let mut levels = [0.0; LAYERS];
        levels[layer.index()] = 1.0;
        let frames = (seconds * RATE as f32) as usize;
        let mut out = vec![0f32; frames * CHANNELS];
        for block in out.chunks_mut(BLOCK_FRAMES * CHANNELS) {
            engine.render(block, &levels, 1.0, 1.0);
        }
        out
    }

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|x| x * x).sum::<f32>() / s.len().max(1) as f32).sqrt()
    }

    #[test]
    fn every_layer_is_audible_bounded_and_finite() {
        for layer in Layer::ALL {
            let out = render_layer(layer, 4.0);
            // Skip the fade-in.
            let settled = &out[RATE as usize * CHANNELS..];
            let level = rms(settled);
            assert!((0.015..=0.5).contains(&level), "{layer:?} rms {level}");
            assert!(settled.iter().all(|x| x.is_finite()), "{layer:?} produced NaN / inf");
            assert!(settled.iter().all(|x| x.abs() <= 1.0), "{layer:?} exceeds full scale");
            assert!(settled.iter().all(|x| *x == 0.0 || x.abs() > 1e-30), "{layer:?} produced denormals");
        }
    }

    #[test]
    fn layers_sound_different() {
        // Zero-crossing rate separates bright (white, rain) from dark (brown, sea).
        let zcr = |s: &[f32]| s.chunks_exact(2).map(|f| f[0]).collect::<Vec<_>>().windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        let white = zcr(&render_layer(Layer::White, 1.0));
        let brown = zcr(&render_layer(Layer::Brown, 1.0));
        let sea = zcr(&render_layer(Layer::Sea, 1.0));
        assert!(white > brown * 5, "white {white} vs brown {brown}");
        assert!(white > sea * 4, "white {white} vs sea {sea}");
    }

    #[test]
    fn gain_changes_are_click_free() {
        let mut engine = Engine::new(7);
        let mut levels = [0.0; LAYERS];
        levels[Layer::Pink.index()] = 1.0;
        let mut warm = vec![0f32; RATE as usize * CHANNELS];
        engine.render(&mut warm, &levels, 1.0, 1.0);
        // Slam the master from 1 to 0 and the layer off: the output must
        // glide, never jump by more than noise does on its own.
        let natural = warm.chunks_exact(2).map(|f| f[0]).collect::<Vec<_>>().windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        let mut out = vec![0f32; RATE as usize * 3 / 2 * CHANNELS];
        engine.render(&mut out, &[0.0; LAYERS], 0.0, 0.0);
        let mut prev = warm[warm.len() - 2];
        for f in out.chunks_exact(2) {
            assert!((f[0] - prev).abs() <= natural * 1.05 + 1e-6, "jump {} > {natural}", (f[0] - prev).abs());
            prev = f[0];
        }
        assert!(engine.is_silent(), "paused output fades to silence within the play fade");
        assert!(out[out.len() - 2].abs() < 1e-3);
    }

    #[test]
    fn silence_when_everything_is_off() {
        let mut engine = Engine::new(3);
        let mut out = vec![1f32; 4096];
        engine.render(&mut out, &[0.0; LAYERS], 1.0, 1.0);
        assert!(out.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn full_mix_stays_within_full_scale() {
        let mut engine = Engine::new(11);
        let mut out = vec![0f32; RATE as usize * 2 * CHANNELS];
        engine.render(&mut out, &[1.0; LAYERS], 1.0, 1.0);
        assert!(out.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
        assert!(rms(&out) > 0.05);
    }

    #[test]
    fn sleep_timer_fades_then_stops() {
        assert_eq!(sleep_gain(0, 5), (1.0, false));
        let at = 100_000;
        assert_eq!(sleep_gain(at, at - SLEEP_FADE.as_millis() as u64 * 2), (1.0, false));
        let (half, done) = sleep_gain(at, at - SLEEP_FADE.as_millis() as u64 / 2);
        assert!((half - 0.5).abs() < 1e-3 && !done);
        assert_eq!(sleep_gain(at, at + 1), (0.0, true));
    }

    #[test]
    fn presets_and_parsing() {
        assert_eq!(Layer::parse("CAFE"), Some(Layer::Cafe));
        assert_eq!(Layer::parse("lava"), None);
        assert!(preset("rainy-study").is_some());
        for p in PRESETS {
            assert!(p.levels.iter().all(|v| (0.0..=1.0).contains(v)));
            assert!(p.levels.iter().any(|v| *v > 0.0));
        }
        assert!(soft_clip(10.0) <= 1.0 && soft_clip(-10.0) >= -1.0);
        assert!((soft_clip(0.1) - 0.1).abs() < 0.001);
    }
}
