//! Live audio spectrum of what's playing, for visualizers: records the
//! default output's monitor through the PulseAudio protocol (PipeWire's
//! pipewire-pulse speaks it too), runs a Hann-windowed FFT and folds it into
//! log-spaced bands smoothed for display. Pure Rust (`pulseaudio`,
//! `rustfft`).
//!
//! Capture runs only while at least one [`Spectrum`] handle is alive: the
//! first `subscribe()` starts it, dropping the last handle stops it.

use std::ffi::CString;
use std::io::{BufReader, Read};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use pulseaudio::protocol::{self, Command};
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

/// Number of bands delivered.
pub const BANDS: usize = 32;
const RATE: u32 = 48_000;
const FFT: usize = 2048;
const LOW_HZ: f32 = 40.0;
const HIGH_HZ: f32 = 16_000.0;
/// Bytes per `Float32Le` mono sample.
const SAMPLE_BYTES: usize = 4;
/// Capture fragments per second requested from the server (~10 ms each).
const FRAGMENTS_PER_S: u32 = 100;
/// Socket read timeout: how often a quiet capture re-checks subscribers.
const READ_TIMEOUT: Duration = Duration::from_millis(500);
/// Minimum time between analyses (~60 per second is plenty for display).
const ANALYSIS_EVERY: Duration = Duration::from_millis(16);
/// Most recent samples scanned for audible sound, and the peak counting as sound.
const PEAK_WINDOW: usize = 1024;
const AUDIBLE_PEAK: f32 = 0.003;
/// "Recently audible" window of [`Spectrum::is_active`].
const ACTIVE_WINDOW: Duration = Duration::from_secs(1);
/// Automatic gain: the ceiling's floor and its per-analysis decay.
const GAIN_FLOOR: f32 = 0.35;
const GAIN_DECAY: f32 = 0.997;
/// Smoothing toward a louder (attack) or quieter (release) level.
const ATTACK: f32 = 0.6;
const RELEASE: f32 = 0.18;
/// Hann window: `0.5 − 0.5·cos(2πi/N)`.
const HANN_HALF: f32 = 0.5;
/// Folding: dB range mapped to 0..=1 (−70 … −10 dBFS) and the treble lift
/// added across the bands.
const DB_FLOOR: f32 = -70.0;
const DB_RANGE: f32 = 60.0;
const TREBLE_LIFT: f32 = 3.0;
/// Amplitude → dB, and the smallest amplitude considered (avoids log(0)).
const DB_PER_DECADE: f32 = 20.0;
const MIN_AMPLITUDE: f32 = 1e-9;
/// Hann window coherent gain (normalizes peak amplitudes).
const HANN_COHERENT_GAIN: f32 = 0.5;
/// Pulse request tags of the setup sequence.
const TAG_AUTH: u32 = 0;
const TAG_CLIENT_NAME: u32 = 1;
const TAG_SERVER_INFO: u32 = 2;
const TAG_RECORD: u32 = 3;

struct Shared {
    bands: Mutex<[f32; BANDS]>,
    subscribers: AtomicUsize,
    running: AtomicBool,
    /// When audible sound was last seen.
    last_sound: Mutex<Option<Instant>>,
}

fn shared() -> &'static Arc<Shared> {
    static S: OnceLock<Arc<Shared>> = OnceLock::new();
    S.get_or_init(|| Arc::new(Shared { bands: Mutex::new([0.0; BANDS]), subscribers: AtomicUsize::new(0), running: AtomicBool::new(false), last_sound: Mutex::new(None) }))
}

/// A subscription to the spectrum; capture stops when the last is dropped.
pub struct Spectrum(Arc<Shared>);

impl Spectrum {
    /// Band levels, `0..=1`, low to high frequency.
    pub fn bands(&self) -> [f32; BANDS] {
        *self.0.bands.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Sound was heard in the last second (silence / paused → `false`).
    pub fn is_active(&self) -> bool {
        self.0.last_sound.lock().unwrap_or_else(|e| e.into_inner()).is_some_and(|t| t.elapsed() < ACTIVE_WINDOW)
    }

    /// Capture is connected.
    pub fn is_running(&self) -> bool {
        self.0.running.load(Ordering::Relaxed)
    }
}

impl Clone for Spectrum {
    fn clone(&self) -> Self {
        self.0.subscribers.fetch_add(1, Ordering::SeqCst);
        Spectrum(self.0.clone())
    }
}

impl Drop for Spectrum {
    fn drop(&mut self) {
        self.0.subscribers.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Subscribes, starting capture if needed. `None` without a sound server.
pub fn subscribe() -> Option<Spectrum> {
    pulseaudio::socket_path_from_env()?;
    let s = shared().clone();
    s.subscribers.fetch_add(1, Ordering::SeqCst);
    if !s.running.swap(true, Ordering::SeqCst) {
        let worker = s.clone();
        let spawned = std::thread::Builder::new().name("mandara-spectrum".into()).spawn(move || {
            if let Err(err) = capture(&worker) {
                tracing::debug!("spectrum capture stopped: {err}");
            }
            *worker.bands.lock().unwrap_or_else(|e| e.into_inner()) = [0.0; BANDS];
            worker.running.store(false, Ordering::SeqCst);
        });
        if spawned.is_err() {
            s.running.store(false, Ordering::SeqCst);
        }
    }
    Some(Spectrum(s))
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn capture(s: &Shared) -> Result<(), String> {
    let path = pulseaudio::socket_path_from_env().ok_or("no sound server")?;
    let stream = UnixStream::connect(path).map_err(err)?;
    stream.set_read_timeout(Some(READ_TIMEOUT)).map_err(err)?;
    let mut sock = BufReader::new(stream);
    let cookie = crate::audio::pulse_cookie();
    let auth = protocol::AuthParams { version: protocol::MAX_VERSION, supports_shm: false, supports_memfd: false, cookie };
    protocol::write_command_message(sock.get_mut(), TAG_AUTH, &Command::Auth(auth), protocol::MAX_VERSION).map_err(err)?;
    let (_, reply) = protocol::read_reply_message::<protocol::AuthReply>(&mut sock, protocol::MAX_VERSION).map_err(|e| format!("auth: {e}"))?;
    let version = protocol::MAX_VERSION.min(reply.version);
    let mut props = protocol::Props::new();
    props.set(protocol::Prop::ApplicationName, CString::from(c"Mandara visualizer"));
    protocol::write_command_message(sock.get_mut(), TAG_CLIENT_NAME, &Command::SetClientName(props), version).map_err(err)?;
    protocol::read_reply_message::<protocol::SetClientNameReply>(&mut sock, version).map_err(|e| format!("client name: {e}"))?;

    // The default sink's monitor = what's playing.
    protocol::write_command_message(sock.get_mut(), TAG_SERVER_INFO, &Command::GetServerInfo, version).map_err(err)?;
    let (_, info) = protocol::read_reply_message::<protocol::ServerInfo>(&mut sock, version).map_err(|e| format!("server info: {e}"))?;
    let sink = info.default_sink_name.ok_or("no default output")?;
    let monitor_name = format!("{}.monitor", sink.to_string_lossy());
    let monitor = CString::new(monitor_name.clone()).map_err(err)?;

    let mut stream_props = protocol::Props::new();
    stream_props.set(protocol::Prop::MediaName, CString::from(c"Spectrum"));
    let params = protocol::RecordStreamParams {
        source_name: Some(monitor),
        sample_spec: protocol::SampleSpec { format: protocol::SampleFormat::Float32Le, channels: 1, sample_rate: RATE },
        channel_map: protocol::ChannelMap::mono(),
        buffer_attr: protocol::stream::BufferAttr {
            max_length: u32::MAX,
            target_length: u32::MAX,
            pre_buffering: u32::MAX,
            minimum_request_length: u32::MAX,
            // ~10 ms fragments.
            fragment_size: (RATE / FRAGMENTS_PER_S) * SAMPLE_BYTES as u32,
        },
        flags: protocol::stream::StreamFlags { adjust_latency: true, ..Default::default() },
        props: stream_props,
        ..Default::default()
    };
    protocol::write_command_message(sock.get_mut(), TAG_RECORD, &Command::CreateRecordStream(params), version).map_err(err)?;
    let (_, rec) = protocol::read_reply_message::<protocol::CreateRecordStreamReply>(&mut sock, version).map_err(|e| format!("record {}: {e}", monitor_name))?;
    if rec.sample_spec.format != protocol::SampleFormat::Float32Le {
        return Err(format!("unexpected sample format {:?}", rec.sample_spec.format));
    }

    // Everything the loop touches is allocated here, once: the planner's
    // FFT, its scratch, the sample window and the capture buffers.
    let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT);
    let window = hann_window();
    let edges = band_edges(rec.sample_spec.sample_rate as f32);
    let mut ring: Vec<f32> = vec![0.0; FFT];
    let mut buf: Vec<u8> = Vec::new();
    let mut spectrum = vec![Complex::new(0.0f32, 0.0); FFT];
    let mut fft_scratch = vec![Complex::new(0.0f32, 0.0); fft.get_inplace_scratch_len()];
    let mut smooth = [0.0f32; BANDS];
    // Automatic gain: quiet content (speech, low volume) still fills the
    // range; the ceiling follows peaks up fast and relaxes slowly.
    let mut ceiling = GAIN_FLOOR;
    let mut last_fft = Instant::now();

    while s.subscribers.load(Ordering::SeqCst) > 0 {
        let desc = match protocol::read_descriptor(&mut sock) {
            Ok(d) => d,
            Err(e) if is_timeout(&e) => continue,
            Err(e) => return Err(err(e)),
        };
        if desc.channel == u32::MAX {
            // Server command (e.g. stream moved/killed): read and ignore.
            let (_, msg) = protocol::Command::read_tag_prefixed(&mut sock, version).map_err(err)?;
            if matches!(msg, Command::RecordStreamKilled(_)) {
                return Err("stream killed".into());
            }
            continue;
        }
        buf.resize(desc.length as usize, 0);
        sock.read_exact(&mut buf).map_err(err)?;
        push_samples(&mut ring, &buf);
        if last_fft.elapsed() < ANALYSIS_EVERY {
            continue;
        }
        last_fft = Instant::now();
        let peak = ring[FFT - PEAK_WINDOW..].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        if peak > AUDIBLE_PEAK {
            *s.last_sound.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        }
        for ((c, x), w) in spectrum.iter_mut().zip(&ring).zip(&window) {
            *c = Complex::new(x * w, 0.0);
        }
        fft.process_with_scratch(&mut spectrum, &mut fft_scratch);
        let mut levels = fold(&spectrum, &edges);
        let loudest = levels.iter().copied().fold(0.0f32, f32::max);
        ceiling = if loudest > ceiling { loudest } else { (ceiling * GAIN_DECAY).max(GAIN_FLOOR) };
        for l in &mut levels {
            *l = (*l / ceiling).clamp(0.0, 1.0);
        }
        for (s, l) in smooth.iter_mut().zip(levels) {
            // Fast attack, slow release.
            *s += (l - *s) * if l > *s { ATTACK } else { RELEASE };
        }
        *s.bands.lock().unwrap_or_else(|e| e.into_inner()) = smooth;
    }
    Ok(())
}

/// The Hann window over one FFT frame.
fn hann_window() -> Vec<f32> {
    (0..FFT).map(|i| HANN_HALF - HANN_HALF * (std::f32::consts::TAU * i as f32 / FFT as f32).cos()).collect()
}

/// Appends little-endian `f32` samples to the sliding analysis window:
/// one block move per fragment (not one shift per sample) and no
/// allocation.
fn push_samples(ring: &mut [f32], bytes: &[u8]) {
    let n = bytes.len() / SAMPLE_BYTES;
    // Only the newest `ring.len()` samples matter.
    let skip = n.saturating_sub(ring.len());
    let keep = n - skip;
    let len = ring.len();
    ring.copy_within(keep.., 0);
    for (dst, c) in ring[len - keep..].iter_mut().zip(bytes.chunks_exact(SAMPLE_BYTES).skip(skip)) {
        *dst = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
    }
}

fn is_timeout(e: &protocol::ProtocolError) -> bool {
    matches!(e, protocol::ProtocolError::Io(io) if matches!(io.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
}

/// FFT bin ranges for log-spaced bands between LOW_HZ and HIGH_HZ.
fn band_edges(rate: f32) -> Vec<(usize, usize)> {
    let bin_hz = rate / FFT as f32;
    (0..BANDS)
        .map(|b| {
            let f0 = LOW_HZ * (HIGH_HZ / LOW_HZ).powf(b as f32 / BANDS as f32);
            let f1 = LOW_HZ * (HIGH_HZ / LOW_HZ).powf((b + 1) as f32 / BANDS as f32);
            let lo = ((f0 / bin_hz) as usize).max(1);
            let hi = ((f1 / bin_hz) as usize).max(lo + 1).min(FFT / 2);
            (lo, hi)
        })
        .collect()
}

/// Band levels `0..=1` from an FFT (dB mapped from −70..−10 dBFS, with a
/// gentle treble lift so high bands aren't always flat).
fn fold(spectrum: &[Complex<f32>], edges: &[(usize, usize)]) -> [f32; BANDS] {
    let mut out = [0.0f32; BANDS];
    // One-sided spectrum (×2) over the window's coherent gain.
    let norm = 2.0 / (FFT as f32 * HANN_COHERENT_GAIN);
    for (b, (lo, hi)) in edges.iter().enumerate() {
        let peak = spectrum[*lo..*hi].iter().map(|c| c.norm() * norm).fold(0.0f32, f32::max);
        let tilt = 1.0 + TREBLE_LIFT * b as f32 / BANDS as f32;
        let db = DB_PER_DECADE * (peak * tilt).max(MIN_AMPLITUDE).log10();
        out[b] = ((db - DB_FLOOR) / DB_RANGE).clamp(0.0, 1.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_lights_its_band() {
        let edges = band_edges(RATE as f32);
        let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT);
        let freq = 1000.0;
        let mut buf: Vec<Complex<f32>> = (0..FFT)
            .map(|i| {
                let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FFT as f32).cos();
                Complex::new(0.5 * (std::f32::consts::TAU * freq * i as f32 / RATE as f32).sin() * w, 0.0)
            })
            .collect();
        fft.process(&mut buf);
        let levels = fold(&buf, &edges);
        let loudest = levels.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i).unwrap();
        let (lo, hi) = edges[loudest];
        let bin_hz = RATE as f32 / FFT as f32;
        assert!((lo as f32 * bin_hz..=hi as f32 * bin_hz + bin_hz).contains(&freq), "band {loudest} {lo}..{hi}");
        assert!(levels[loudest] > 0.6);
        assert!(levels[0] < 0.3, "low band quiet: {}", levels[0]);
    }

    #[test]
    fn samples_slide_in_at_the_end() {
        let mut ring = vec![0.0f32; FFT];
        let bytes: Vec<u8> = [1.0f32, 2.0, 3.0].iter().flat_map(|x| x.to_le_bytes()).collect();
        push_samples(&mut ring, &bytes);
        assert_eq!(&ring[FFT - 3..], &[1.0, 2.0, 3.0]);
        push_samples(&mut ring, &4.0f32.to_le_bytes());
        assert_eq!(&ring[FFT - 4..], &[1.0, 2.0, 3.0, 4.0]);
        // More than a frame: only the newest samples stay.
        let many: Vec<u8> = (0..FFT + 5).flat_map(|i| (i as f32).to_le_bytes()).collect();
        push_samples(&mut ring, &many);
        assert_eq!(ring[0], 5.0);
        assert_eq!(ring[FFT - 1], (FFT + 4) as f32);
    }

    #[test]
    fn edges_are_increasing_and_in_range() {
        let e = band_edges(RATE as f32);
        assert_eq!(e.len(), BANDS);
        assert!(e.windows(2).all(|w| w[0].0 <= w[1].0));
        assert!(e.iter().all(|(lo, hi)| lo < hi && *hi <= FFT / 2));
    }
}
