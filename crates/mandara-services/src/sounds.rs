//! Shell event sounds, the way the freedesktop sound theme spec intends:
//! a sound name ("audio-volume-change", "message-new-instant", …) is looked
//! up in the theme (`$XDG_DATA_HOME/sounds`, `$XDG_DATA_DIRS/sounds`), its
//! `Inherits=` chain and finally `freedesktop`, trying ever shorter names
//! ("message-new-instant" → "message-new" → "message"); a `.disabled` file
//! silences it. Files decode in pure Rust (`lewton` for Ogg Vorbis, `hound`
//! for WAV).
//!
//! Playback goes through the PulseAudio native protocol's sample cache
//! (PipeWire's pipewire-pulse implements it): each file is uploaded once,
//! then every play is one tiny `PLAY_SAMPLE` request the server mixes
//! itself, tagged `media.role=event` so policies treat it as an event
//! sound. One worker thread exists only while sounds are being played and
//! closes its connection after a short idle period.

use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use mandara_config::alerts::SoundsConfig;
use pulseaudio::protocol::{self, Command};

use crate::audio::Pulse;

/// Longest clip decoded (login sounds can run a few seconds).
const MAX_CLIP: Duration = Duration::from_secs(12);
/// The worker closes its connection after this long without sounds.
const IDLE_CLOSE: Duration = Duration::from_secs(30);
/// The same event replays at most this often (volume key repeat).
const MIN_REPEAT: Duration = Duration::from_millis(90);
/// Requests queued for the worker; more are dropped (sounds are cues, not data).
const QUEUE: usize = 8;
/// Upload chunk size, bytes.
const UPLOAD_CHUNK: usize = 64 * 1024;
/// PulseAudio's 100 % volume.
const VOLUME_NORM: f32 = 0x10000 as f32;
/// Theme every lookup falls back to.
const FALLBACK_THEME: &str = "freedesktop";
const EXTENSIONS: [&str; 3] = ["oga", "ogg", "wav"];
/// `index.theme` keys.
const INHERITS: &str = "Inherits=";
const DIRECTORIES: &str = "Directories=";
const OUTPUT_PROFILE: &str = "OutputProfile=";
const STEREO: &str = "stereo";

/// Something the shell makes a sound for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SoundEvent {
    VolumeChange,
    NotificationLow,
    Notification,
    NotificationCritical,
    PowerPlug,
    PowerUnplug,
    Screenshot,
    Login,
    BatteryLow,
    BatteryCritical,
    /// A theme sound name or a file path (notification `sound-*` hints).
    Named(String),
}

impl SoundEvent {
    /// The events with their own settings, in Studio order.
    pub const CONFIGURABLE: [SoundEvent; 8] = [
        SoundEvent::VolumeChange,
        SoundEvent::NotificationLow,
        SoundEvent::Notification,
        SoundEvent::NotificationCritical,
        SoundEvent::PowerPlug,
        SoundEvent::Screenshot,
        SoundEvent::Login,
        SoundEvent::BatteryLow,
    ];

    /// Key in `[sounds.files]` and on the command line.
    pub fn key(&self) -> &str {
        match self {
            SoundEvent::VolumeChange => "volume-change",
            SoundEvent::NotificationLow => "notification-low",
            SoundEvent::Notification => "notification",
            SoundEvent::NotificationCritical => "notification-critical",
            SoundEvent::PowerPlug => "power-plug",
            SoundEvent::PowerUnplug => "power-unplug",
            SoundEvent::Screenshot => "screenshot",
            SoundEvent::Login => "login",
            SoundEvent::BatteryLow => "battery-low",
            SoundEvent::BatteryCritical => "battery-critical",
            SoundEvent::Named(n) => n,
        }
    }

    pub fn parse(key: &str) -> Self {
        Self::CONFIGURABLE
            .into_iter()
            .chain([SoundEvent::PowerUnplug, SoundEvent::BatteryCritical])
            .find(|e| e.key() == key)
            .unwrap_or_else(|| SoundEvent::Named(key.to_string()))
    }

    /// Sound-naming-spec names, most specific first.
    pub fn names(&self) -> Vec<&str> {
        match self {
            SoundEvent::VolumeChange => vec!["audio-volume-change"],
            SoundEvent::NotificationLow => vec!["message-lowpriority", "message"],
            SoundEvent::Notification => vec!["message-new-instant", "message"],
            SoundEvent::NotificationCritical => vec!["dialog-warning", "message-highlight", "bell"],
            SoundEvent::PowerPlug => vec!["power-plug", "device-added"],
            SoundEvent::PowerUnplug => vec!["power-unplug", "device-removed"],
            SoundEvent::Screenshot => vec!["screen-capture", "camera-shutter"],
            SoundEvent::Login => vec!["desktop-login", "service-login"],
            SoundEvent::BatteryLow => vec!["battery-low", "battery-caution", "dialog-warning"],
            SoundEvent::BatteryCritical => vec!["battery-caution", "battery-low", "dialog-warning"],
            SoundEvent::Named(n) => vec![n.as_str()],
        }
    }

    /// Whether the user switched this event on (`[sounds]` toggles).
    pub fn enabled_in(&self, cfg: &SoundsConfig) -> bool {
        match self {
            SoundEvent::VolumeChange => cfg.volume_change,
            SoundEvent::NotificationLow => cfg.notification_low,
            SoundEvent::Notification | SoundEvent::Named(_) => cfg.notification,
            SoundEvent::NotificationCritical => cfg.notification_critical,
            SoundEvent::PowerPlug | SoundEvent::PowerUnplug | SoundEvent::BatteryLow | SoundEvent::BatteryCritical => cfg.charger,
            SoundEvent::Screenshot => cfg.screenshot,
            SoundEvent::Login => cfg.login,
        }
    }
}

// ── Theme lookup ─────────────────────────────────────────────────────────

/// `$XDG_DATA_HOME/sounds` then each `$XDG_DATA_DIRS/sounds`.
pub fn sound_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::data_dir() {
        roots.push(home.join("sounds"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    roots.extend(data_dirs.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join("sounds")));
    roots
}

/// GTK's configured sound theme, else `freedesktop`.
pub fn system_theme() -> String {
    let from_gtk = ["gtk-4.0", "gtk-3.0"].iter().find_map(|v| {
        let text = std::fs::read_to_string(dirs::config_dir()?.join(v).join("settings.ini")).ok()?;
        text.lines().find_map(|l| l.trim().strip_prefix("gtk-sound-theme-name")?.trim_start().strip_prefix('=').map(|v| v.trim().to_string())).filter(|v| !v.is_empty())
    });
    from_gtk.unwrap_or_else(|| FALLBACK_THEME.into())
}

/// Every installed theme name (folders with sounds in them).
pub fn themes() -> Vec<String> {
    let mut out: Vec<String> = sound_roots()
        .iter()
        .filter_map(|r| std::fs::read_dir(r).ok())
        .flatten()
        .flatten()
        .filter(|e| e.path().join("index.theme").exists() || e.path().join(STEREO).is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// A theme's sub-directories for stereo output and its parents.
fn theme_info(roots: &[PathBuf], theme: &str) -> (Vec<PathBuf>, Vec<String>) {
    let mut dirs = Vec::new();
    let mut inherits = Vec::new();
    for root in roots {
        let base = root.join(theme);
        if !base.is_dir() {
            continue;
        }
        let mut subdirs: Vec<String> = Vec::new();
        if let Ok(index) = std::fs::read_to_string(base.join("index.theme")) {
            let mut section = String::new();
            let mut profiles: HashMap<String, String> = HashMap::new();
            for line in index.lines().map(str::trim) {
                if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                    section = name.to_string();
                } else if let Some(v) = line.strip_prefix(INHERITS) {
                    inherits.extend(v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
                } else if let Some(v) = line.strip_prefix(DIRECTORIES) {
                    subdirs.extend(v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
                } else if let Some(v) = line.strip_prefix(OUTPUT_PROFILE) {
                    profiles.insert(section.clone(), v.trim().to_string());
                }
            }
            subdirs.retain(|d| profiles.get(d).is_none_or(|p| p == STEREO));
        }
        if !subdirs.iter().any(|d| d == STEREO) {
            subdirs.push(STEREO.into());
        }
        dirs.extend(subdirs.iter().map(|d| base.join(d)));
        dirs.push(base);
    }
    (dirs, inherits)
}

/// The theme and everything it inherits, then `freedesktop`; no repeats.
fn theme_chain(roots: &[PathBuf], theme: &str) -> Vec<PathBuf> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue = vec![theme.to_string()];
    let mut dirs = Vec::new();
    let mut i = 0;
    while i < queue.len() {
        let name = queue[i].clone();
        i += 1;
        if !seen.insert(name.clone()) {
            continue;
        }
        let (d, parents) = theme_info(roots, &name);
        dirs.extend(d);
        queue.extend(parents);
        if i == queue.len() && !seen.contains(FALLBACK_THEME) {
            queue.push(FALLBACK_THEME.into());
        }
    }
    dirs
}

/// `a-b-c`, `a-b`, `a`.
fn fallbacks(name: &str) -> impl Iterator<Item = &str> {
    std::iter::successors(Some(name), |n| n.rfind('-').map(|i| &n[..i]))
}

/// Finds the file for the first of `names` the theme (or its ancestors)
/// provides. A `.disabled` file stops the search: the sound is off.
pub fn lookup_in(roots: &[PathBuf], theme: &str, names: &[&str]) -> Option<PathBuf> {
    let dirs = theme_chain(roots, theme);
    for name in names {
        for candidate in fallbacks(name) {
            for dir in &dirs {
                if dir.join(format!("{candidate}.disabled")).exists() {
                    return None;
                }
                if let Some(p) = EXTENSIONS.iter().map(|ext| dir.join(format!("{candidate}.{ext}"))).find(|p| p.is_file()) {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// The file `event` plays under `cfg`: your own file, else the theme's.
pub fn resolve(event: &SoundEvent, cfg: &SoundsConfig) -> Option<PathBuf> {
    if let Some(own) = cfg.files.get(event.key()).map(|p| expand(p)).filter(|p| p.is_file()) {
        return Some(own);
    }
    if let SoundEvent::Named(n) = event
        && n.starts_with('/')
    {
        return Some(PathBuf::from(n)).filter(|p| p.is_file());
    }
    let theme = if cfg.theme.trim().is_empty() { system_theme() } else { cfg.theme.trim().to_string() };
    lookup_in(&sound_roots(), &theme, &event.names())
}

fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(p)),
        None => PathBuf::from(p),
    }
}

// ── Decoding ─────────────────────────────────────────────────────────────

/// Decoded PCM, interleaved signed 16-bit.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub rate: u32,
    pub channels: u8,
    pub samples: Vec<i16>,
}

impl Clip {
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.samples.len() as f64 / f64::from(self.channels.max(1)) / f64::from(self.rate.max(1)))
    }

    /// Mono stays mono; anything wider folds down to stereo (even
    /// channels left, odd channels right).
    fn to_stereo_at_most(mut self) -> Self {
        let ch = usize::from(self.channels);
        if ch <= 2 {
            return self;
        }
        let mut out = Vec::with_capacity(self.samples.len() / ch * 2);
        for frame in self.samples.chunks_exact(ch) {
            let (mut l, mut r, mut nl, mut nr) = (0i32, 0i32, 0i32, 0i32);
            for (i, s) in frame.iter().enumerate() {
                if i % 2 == 0 {
                    l += i32::from(*s);
                    nl += 1;
                } else {
                    r += i32::from(*s);
                    nr += 1;
                }
            }
            out.push((l / nl.max(1)) as i16);
            out.push((r / nr.max(1)) as i16);
        }
        self.samples = out;
        self.channels = 2;
        self
    }
}

/// Decodes an Ogg Vorbis (`.oga` / `.ogg`) or WAV file.
pub fn decode(path: &Path) -> Result<Clip, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    let clip = if ext == "wav" { decode_wav(path)? } else { decode_vorbis(path)? };
    if clip.samples.is_empty() || clip.channels == 0 || clip.rate == 0 {
        return Err(format!("{}: no audio", path.display()));
    }
    Ok(clip.to_stereo_at_most())
}

fn decode_vorbis(path: &Path) -> Result<Clip, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut reader = lewton::inside_ogg::OggStreamReader::new(BufReader::new(file)).map_err(|e| format!("{}: {e}", path.display()))?;
    let channels = reader.ident_hdr.audio_channels;
    let rate = reader.ident_hdr.audio_sample_rate;
    let cap = (MAX_CLIP.as_secs_f64() * f64::from(rate) * f64::from(channels)) as usize;
    let mut samples = Vec::new();
    while let Some(packet) = reader.read_dec_packet_itl().map_err(|e| format!("{}: {e}", path.display()))? {
        samples.extend_from_slice(&packet);
        if samples.len() >= cap {
            samples.truncate(cap);
            break;
        }
    }
    Ok(Clip { rate, channels, samples })
}

fn decode_wav(path: &Path) -> Result<Clip, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let spec = reader.spec();
    let channels = u8::try_from(spec.channels).map_err(|_| "too many channels".to_string())?;
    let cap = (MAX_CLIP.as_secs_f64() * f64::from(spec.sample_rate) * f64::from(spec.channels)) as usize;
    /// 16-bit full scale for float WAVs.
    const I16_SCALE: f32 = i16::MAX as f32;
    const I16_BITS: u16 = 16;
    let samples: Vec<i16> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().take(cap).map(|s| s.map(|v| (v.clamp(-1.0, 1.0) * I16_SCALE) as i16)).collect::<Result<_, _>>(),
        hound::SampleFormat::Int if spec.bits_per_sample <= I16_BITS => {
            let shift = I16_BITS - spec.bits_per_sample;
            reader.samples::<i16>().take(cap).map(|s| s.map(|v| v << shift)).collect::<Result<_, _>>()
        }
        hound::SampleFormat::Int => {
            let shift = spec.bits_per_sample - I16_BITS;
            reader.samples::<i32>().take(cap).map(|s| s.map(|v| (v >> shift) as i16)).collect::<Result<_, _>>()
        }
    }
    .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Clip { rate: spec.sample_rate, channels, samples })
}

// ── Playback ─────────────────────────────────────────────────────────────

struct Job {
    path: PathBuf,
    volume: f32,
    event: String,
}

fn worker() -> &'static Mutex<Option<std::sync::mpsc::SyncSender<Job>>> {
    static WORKER: OnceLock<Mutex<Option<std::sync::mpsc::SyncSender<Job>>>> = OnceLock::new();
    WORKER.get_or_init(|| Mutex::new(None))
}

/// A cache name that changes when the file does.
fn sample_name(path: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    if let Ok(meta) = std::fs::metadata(path) {
        meta.len().hash(&mut h);
        meta.modified().ok().hash(&mut h);
    }
    format!("mandara-{:016x}", h.finish())
}

/// Uploads `clip` into the server's sample cache as `name`.
fn upload(p: &mut Pulse, name: &str, clip: &Clip) -> Result<(), String> {
    let bytes: Vec<u8> = clip.samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let length = u32::try_from(bytes.len()).map_err(|_| "sound too long".to_string())?;
    let mut props = protocol::Props::new();
    props.set(protocol::Prop::MediaRole, CString::from(c"event"));
    // pipewire-pulse names cache entries from the properties (`event.id`,
    // else `media.name`), not from the request's name field.
    let cname = CString::new(name).map_err(|e| e.to_string())?;
    props.set(protocol::Prop::EventId, cname.clone());
    props.set(protocol::Prop::MediaName, cname);
    let params = protocol::UploadStreamParams {
        media_name: Some(CString::new(name).map_err(|e| e.to_string())?),
        sample_spec: protocol::SampleSpec { format: protocol::SampleFormat::S16Le, channels: clip.channels, sample_rate: clip.rate },
        channel_map: if clip.channels == 1 { protocol::ChannelMap::mono() } else { protocol::ChannelMap::stereo() },
        length,
        props,
    };
    let reply: protocol::CreateUploadStreamReply = p.request(Command::CreateUploadStream(params)).map_err(|e| format!("create upload: {e}"))?;
    for chunk in bytes.chunks(UPLOAD_CHUNK) {
        p.write_data(reply.channel, chunk)?;
    }
    p.ack(Command::FinishUploadStream(reply.channel)).map_err(|e| format!("finish upload: {e}"))
}

fn play_sample(p: &mut Pulse, name: &str, volume: f32, event: &str) -> Result<(), String> {
    let mut props = protocol::Props::new();
    props.set(protocol::Prop::MediaRole, CString::from(c"event"));
    props.set(protocol::Prop::EventId, CString::new(event).map_err(|e| e.to_string())?);
    props.set(protocol::Prop::ApplicationName, CString::from(c"Mandara Shell"));
    let params = protocol::PlaySampleParams {
        sink_index: None,
        sink_name: None,
        volume: (volume.clamp(0.0, 1.0) * VOLUME_NORM).round() as u32,
        name: CString::new(name).map_err(|e| e.to_string())?,
        props,
    };
    // The reply carries the new sink input's index.
    p.request::<protocol::LookupReply>(Command::PlaySample(params)).map(|_| ())
}

/// The worker: one connection, uploads each file once, closes when idle.
fn run_worker(rx: std::sync::mpsc::Receiver<Job>) {
    let mut conn: Option<Pulse> = None;
    let mut uploaded: HashSet<String> = HashSet::new();
    let mut failed: HashSet<PathBuf> = HashSet::new();
    loop {
        let job = match rx.recv_timeout(IDLE_CLOSE) {
            Ok(job) => job,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Idle: drop the connection and end the thread (the next
                // sound starts a new one).
                let mut w = worker().lock().unwrap_or_else(|e| e.into_inner());
                if let Ok(job) = rx.try_recv() {
                    drop(w);
                    play_job(&mut conn, &mut uploaded, &mut failed, job);
                    continue;
                }
                *w = None;
                return;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        };
        play_job(&mut conn, &mut uploaded, &mut failed, job);
    }
}

fn play_job(conn: &mut Option<Pulse>, uploaded: &mut HashSet<String>, failed: &mut HashSet<PathBuf>, job: Job) {
    if failed.contains(&job.path) {
        return;
    }
    let name = sample_name(&job.path);
    // One retry with a fresh connection (the server may have restarted,
    // which also empties its sample cache).
    for attempt in 0..2 {
        if conn.is_none() {
            match Pulse::connect("Mandara Shell (sounds)") {
                Ok(p) => *conn = Some(p),
                Err(err) => {
                    tracing::debug!("sounds: {err}");
                    return;
                }
            }
            uploaded.clear();
        }
        let Some(p) = conn.as_mut() else { return };
        if !uploaded.contains(&name) {
            match decode(&job.path) {
                Ok(clip) => {
                    if let Err(err) = upload(p, &name, &clip) {
                        tracing::warn!("sounds: uploading {}: {err}", job.path.display());
                        *conn = None;
                        continue;
                    }
                    uploaded.insert(name.clone());
                }
                Err(err) => {
                    tracing::warn!("sounds: {err}");
                    failed.insert(job.path.clone());
                    return;
                }
            }
        }
        match play_sample(p, &name, job.volume, &job.event) {
            Ok(()) => return,
            Err(err) => {
                tracing::debug!("sounds: playing {} (attempt {attempt}): {err}", job.path.display());
                *conn = None;
            }
        }
    }
}

fn last_played() -> &'static Mutex<HashMap<String, Instant>> {
    static LAST: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    LAST.get_or_init(Default::default)
}

/// Queues `path` (an event's file) at `volume` (0–1). Returns `false`
/// when it was throttled or the queue is full.
pub fn play_file(path: PathBuf, volume: f32, event: &str) -> bool {
    {
        let mut last = last_played().lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if last.get(event).is_some_and(|t| now.duration_since(*t) < MIN_REPEAT) {
            return false;
        }
        last.insert(event.to_string(), now);
    }
    let job = Job { path, volume, event: event.to_string() };
    let mut w = worker().lock().unwrap_or_else(|e| e.into_inner());
    let job = match w.as_ref() {
        Some(tx) => match tx.try_send(job) {
            Ok(()) => return true,
            Err(std::sync::mpsc::TrySendError::Full(_)) => return false,
            // The worker went idle and ended: start a new one.
            Err(std::sync::mpsc::TrySendError::Disconnected(job)) => job,
        },
        None => job,
    };
    let (tx, rx) = std::sync::mpsc::sync_channel(QUEUE);
    if tx.try_send(job).is_err() {
        return false;
    }
    if std::thread::Builder::new().name("mandara-sounds".into()).spawn(move || run_worker(rx)).is_err() {
        return false;
    }
    *w = Some(tx);
    true
}

/// Plays `event` if sounds and that event are enabled. Returns what was
/// queued (the file), for the command line.
pub fn play(event: &SoundEvent, cfg: &SoundsConfig) -> Option<PathBuf> {
    if !cfg.enabled || !event.enabled_in(cfg) || cfg.volume <= 0.0 {
        return None;
    }
    preview(event, cfg)
}

/// Plays `event` regardless of the toggles (Studio's preview buttons).
pub fn preview(event: &SoundEvent, cfg: &SoundsConfig) -> Option<PathBuf> {
    let path = resolve(event, cfg)?;
    play_file(path.clone(), cfg.volume, event.key()).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme(root: &Path, name: &str, index: &str, files: &[&str]) {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("stereo")).unwrap();
        if !index.is_empty() {
            std::fs::write(dir.join("index.theme"), index).unwrap();
        }
        for f in files {
            std::fs::write(dir.join("stereo").join(f), b"x").unwrap();
        }
    }

    #[test]
    fn lookup_follows_inheritance_fallbacks_and_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        theme(&root, "freedesktop", "", &["message.oga", "bell.oga", "audio-volume-change.oga"]);
        theme(&root, "ocean", "[Sound Theme]\nInherits=freedesktop\nDirectories=stereo\n[stereo]\nOutputProfile=stereo\n", &["message-new-instant.oga"]);
        theme(&root, "mine", "[Sound Theme]\nInherits=ocean\n", &["bell.disabled", "dialog-warning.wav"]);
        let roots = vec![root.clone()];
        let found = |t: &str, names: &[&str]| lookup_in(&roots, t, names).map(|p| p.strip_prefix(&root).unwrap().to_string_lossy().into_owned());
        assert_eq!(found("mine", &["message-new-instant"]).as_deref(), Some("ocean/stereo/message-new-instant.oga"));
        // Shorter names, then the freedesktop fallback.
        assert_eq!(found("ocean", &["message-lowpriority"]).as_deref(), Some("freedesktop/stereo/message.oga"));
        assert_eq!(found("mine", &["dialog-warning"]).as_deref(), Some("mine/stereo/dialog-warning.wav"));
        // `.disabled` silences, even though a parent has the sound.
        assert_eq!(found("mine", &["bell"]), None);
        // Unknown themes still reach freedesktop.
        assert_eq!(found("nope", &["audio-volume-change"]).as_deref(), Some("freedesktop/stereo/audio-volume-change.oga"));
        assert_eq!(fallbacks("a-b-c").collect::<Vec<_>>(), vec!["a-b-c", "a-b", "a"]);
    }

    #[test]
    fn events_round_trip_and_follow_toggles() {
        for e in SoundEvent::CONFIGURABLE {
            assert_eq!(SoundEvent::parse(e.key()), e);
            assert!(!e.names().is_empty());
        }
        assert_eq!(SoundEvent::parse("bell"), SoundEvent::Named("bell".into()));
        let mut cfg = SoundsConfig::default();
        assert!(play(&SoundEvent::Notification, &cfg).is_none(), "off by default");
        cfg.enabled = true;
        cfg.notification = false;
        assert!(!SoundEvent::Notification.enabled_in(&cfg));
        assert!(SoundEvent::NotificationCritical.enabled_in(&cfg));
    }

    #[test]
    fn wav_decodes_and_downmixes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.wav");
        let spec = hound::WavSpec { channels: 4, sample_rate: 8000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..800 {
            for s in [100i16, 200, 300, 400] {
                w.write_sample(s).unwrap();
            }
        }
        w.finalize().unwrap();
        let clip = decode(&path).unwrap();
        assert_eq!((clip.channels, clip.rate), (2, 8000));
        assert_eq!(&clip.samples[..2], &[200, 300]);
        assert!((clip.duration().as_secs_f32() - 0.1).abs() < 1e-3);
    }

    /// Uploads a real theme sound into the server's sample cache, plays it
    /// at volume 0 (silent) and removes it: the whole protocol path.
    /// `cargo test -p mandara-services sample_cache -- --ignored`
    #[test]
    #[ignore = "needs a running sound server"]
    fn sample_cache_round_trip_silently() {
        let path = Path::new("/usr/share/sounds/freedesktop/stereo/audio-volume-change.oga");
        let clip = decode(path).unwrap();
        let mut p = Pulse::connect("Mandara sounds test").unwrap();
        let name = "mandara-test-sample";
        upload(&mut p, name, &clip).unwrap();
        play_sample(&mut p, name, 0.0, "test").unwrap();
        p.ack(Command::RemoveSample(CString::new(name).unwrap())).unwrap();
    }

    #[test]
    fn system_vorbis_sound_decodes_if_installed() {
        let path = Path::new("/usr/share/sounds/freedesktop/stereo/audio-volume-change.oga");
        if !path.exists() {
            return;
        }
        let clip = decode(path).unwrap();
        assert!(clip.rate > 0 && (1..=2).contains(&clip.channels));
        assert!(clip.duration() > Duration::ZERO && clip.duration() < MAX_CLIP);
    }
}
