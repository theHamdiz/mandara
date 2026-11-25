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
//! sound. On FreeBSD without a sound server the clip goes straight to the
//! OSS device (`freebsd::dsp`), opened for that one sound.
//!
//! One worker thread exists only while sounds are being played. It keeps
//! the [`CACHE_SLOTS`] most recently played sounds ready (at most
//! [`CACHE_BYTES`] of PCM: in the server's cache, or decoded in-process for
//! OSS), removes what falls out, and when idle for [`IDLE_CLOSE`] (or when
//! the theme changes, [`release`]) removes them all, closes its connection
//! and ends.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::CString;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use mandara_config::alerts::SoundsConfig;
use mandara_i18n::tk;
use pulseaudio::protocol::{self, Command};

use crate::audio::Pulse;

/// Longest clip decoded (login sounds can run a few seconds).
const MAX_CLIP: Duration = Duration::from_secs(12);
/// The worker removes its sounds, closes its connection and ends after this
/// long without sounds.
pub const IDLE_CLOSE: Duration = Duration::from_secs(30);
/// The same event replays at most this often (volume key repeat).
const MIN_REPEAT: Duration = Duration::from_millis(90);
/// Events remembered for [`MIN_REPEAT`] before old ones are dropped.
const REPEAT_KEYS: usize = 32;
/// Requests queued for the worker; more are dropped (sounds are cues, not data).
const QUEUE: usize = 8;
/// Sounds kept ready between plays, and the PCM they may hold.
pub const CACHE_SLOTS: usize = 12;
pub const CACHE_BYTES: usize = 4 << 20;
/// Bars in a sound's level outline (Studio's preview waveform).
pub const PEAKS: usize = 24;
/// Upload chunk size, bytes.
const UPLOAD_CHUNK: usize = 64 * 1024;
/// PulseAudio's 100 % volume.
const VOLUME_NORM: u32 = 0x10000;
/// Every `media.role=event` stream shares one stored level on the server
/// (the mixer's "System sounds", WirePlumber's `media.role:Notification`).
/// When that is muted, as desktops and mixers often leave it, every shell
/// sound played silently. Mandara's sounds have their own switch and
/// volume, so the server is asked not to restore that shared level onto
/// them.
const NO_RESTORE: (&std::ffi::CStr, &std::ffi::CStr) = (c"state.restore-props", c"false");
/// Theme every lookup falls back to.
pub const FALLBACK_THEME: &str = "freedesktop";
pub const EXTENSIONS: [&str; 3] = ["oga", "ogg", "wav"];
/// `$XDG_DATA_DIRS` when unset (the XDG default; FreeBSD's ports install
/// sound themes under `/usr/local/share/sounds`).
pub const DEFAULT_DATA_DIRS: &str = "/usr/local/share:/usr/share";
/// `index.theme` keys.
const INHERITS: &str = "Inherits=";
const DIRECTORIES: &str = "Directories=";
const OUTPUT_PROFILE: &str = "OutputProfile=";
const STEREO: &str = "stereo";

/// Something the shell makes a sound for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SoundEvent {
    VolumeChange,
    BrightnessChange,
    NotificationLow,
    Notification,
    NotificationCritical,
    PowerPlug,
    PowerUnplug,
    Screenshot,
    Login,
    Lock,
    Unlock,
    DeviceAdded,
    DeviceRemoved,
    TrashEmpty,
    Complete,
    BatteryLow,
    BatteryCritical,
    /// A theme sound name or a file path (notification `sound-*` hints).
    Named(String),
}

impl SoundEvent {
    /// The events with their own settings, in Studio order.
    pub const CONFIGURABLE: [SoundEvent; 13] = [
        SoundEvent::Notification,
        SoundEvent::NotificationCritical,
        SoundEvent::NotificationLow,
        SoundEvent::VolumeChange,
        SoundEvent::BrightnessChange,
        SoundEvent::Screenshot,
        SoundEvent::PowerPlug,
        SoundEvent::BatteryLow,
        SoundEvent::DeviceAdded,
        SoundEvent::TrashEmpty,
        SoundEvent::Complete,
        SoundEvent::Login,
        SoundEvent::Lock,
    ];
    /// Every named event (the configurable ones and their partners).
    pub const ALL: [SoundEvent; 17] = [
        SoundEvent::Notification,
        SoundEvent::NotificationCritical,
        SoundEvent::NotificationLow,
        SoundEvent::VolumeChange,
        SoundEvent::BrightnessChange,
        SoundEvent::Screenshot,
        SoundEvent::PowerPlug,
        SoundEvent::PowerUnplug,
        SoundEvent::BatteryLow,
        SoundEvent::BatteryCritical,
        SoundEvent::DeviceAdded,
        SoundEvent::DeviceRemoved,
        SoundEvent::TrashEmpty,
        SoundEvent::Complete,
        SoundEvent::Login,
        SoundEvent::Lock,
        SoundEvent::Unlock,
    ];

    /// Key in `[sounds.files]` and on the command line.
    pub fn key(&self) -> &str {
        match self {
            SoundEvent::VolumeChange => "volume-change",
            SoundEvent::BrightnessChange => "brightness-change",
            SoundEvent::NotificationLow => "notification-low",
            SoundEvent::Notification => "notification",
            SoundEvent::NotificationCritical => "notification-critical",
            SoundEvent::PowerPlug => "power-plug",
            SoundEvent::PowerUnplug => "power-unplug",
            SoundEvent::Screenshot => "screenshot",
            SoundEvent::Login => "login",
            SoundEvent::Lock => "lock",
            SoundEvent::Unlock => "unlock",
            SoundEvent::DeviceAdded => "device-added",
            SoundEvent::DeviceRemoved => "device-removed",
            SoundEvent::TrashEmpty => "trash-empty",
            SoundEvent::Complete => "complete",
            SoundEvent::BatteryLow => "battery-low",
            SoundEvent::BatteryCritical => "battery-critical",
            SoundEvent::Named(n) => n,
        }
    }

    pub fn parse(key: &str) -> Self {
        Self::ALL.into_iter().find(|e| e.key() == key).unwrap_or_else(|| SoundEvent::Named(key.to_string()))
    }

    /// What Studio calls it (`tk` text; display with `tr`).
    pub fn label(&self) -> &'static str {
        match self {
            SoundEvent::VolumeChange => tk("Volume"),
            SoundEvent::BrightnessChange => tk("Brightness"),
            SoundEvent::NotificationLow => tk("Low urgency"),
            SoundEvent::Notification => tk("Notification"),
            SoundEvent::NotificationCritical => tk("Critical"),
            SoundEvent::PowerPlug => tk("Charger"),
            SoundEvent::PowerUnplug => tk("Charger unplugged"),
            SoundEvent::Screenshot => tk("Screenshot"),
            SoundEvent::Login => tk("Login"),
            SoundEvent::Lock => tk("Lock and unlock"),
            SoundEvent::Unlock => tk("Unlock"),
            SoundEvent::DeviceAdded => tk("Devices"),
            SoundEvent::DeviceRemoved => tk("Device removed"),
            SoundEvent::TrashEmpty => tk("Empty Trash"),
            SoundEvent::Complete => tk("Finished"),
            SoundEvent::BatteryLow => tk("Low battery"),
            SoundEvent::BatteryCritical => tk("Critical battery"),
            SoundEvent::Named(_) => tk("Sound"),
        }
    }

    /// Sound-naming-spec names, most specific first.
    pub fn names(&self) -> Vec<&str> {
        match self {
            SoundEvent::VolumeChange => vec!["audio-volume-change"],
            // No spec name: a theme may add one, else the volume tick.
            SoundEvent::BrightnessChange => vec!["brightness-change", "audio-volume-change"],
            SoundEvent::NotificationLow => vec!["message-lowpriority", "message"],
            SoundEvent::Notification => vec!["message-new-instant", "message"],
            SoundEvent::NotificationCritical => vec!["dialog-warning", "message-highlight", "bell"],
            SoundEvent::PowerPlug => vec!["power-plug", "device-added"],
            SoundEvent::PowerUnplug => vec!["power-unplug", "device-removed"],
            SoundEvent::Screenshot => vec!["screen-capture", "camera-shutter"],
            SoundEvent::Login => vec!["desktop-login", "service-login"],
            SoundEvent::Lock => vec!["desktop-screen-lock", "service-logout"],
            SoundEvent::Unlock => vec!["desktop-screen-unlock", "service-login"],
            SoundEvent::DeviceAdded => vec!["device-added"],
            SoundEvent::DeviceRemoved => vec!["device-removed"],
            SoundEvent::TrashEmpty => vec!["trash-empty"],
            SoundEvent::Complete => vec!["complete", "completion-success", "dialog-information"],
            SoundEvent::BatteryLow => vec!["battery-low", "battery-caution", "dialog-warning"],
            SoundEvent::BatteryCritical => vec!["battery-caution", "battery-low", "dialog-warning"],
            SoundEvent::Named(n) => vec![n.as_str()],
        }
    }

    /// Whether the user switched this event on (`[sounds]` toggles).
    pub fn enabled_in(&self, cfg: &SoundsConfig) -> bool {
        match self {
            SoundEvent::VolumeChange => cfg.volume_change,
            SoundEvent::BrightnessChange => cfg.brightness_change,
            SoundEvent::NotificationLow => cfg.notification_low,
            SoundEvent::Notification | SoundEvent::Named(_) => cfg.notification,
            SoundEvent::NotificationCritical => cfg.notification_critical,
            SoundEvent::PowerPlug | SoundEvent::PowerUnplug | SoundEvent::BatteryLow | SoundEvent::BatteryCritical => cfg.charger,
            SoundEvent::Screenshot => cfg.screenshot,
            SoundEvent::Login => cfg.login,
            SoundEvent::Lock | SoundEvent::Unlock => cfg.lock,
            SoundEvent::DeviceAdded | SoundEvent::DeviceRemoved => cfg.devices,
            SoundEvent::TrashEmpty => cfg.trash,
            SoundEvent::Complete => cfg.complete,
        }
    }

    /// Switches this event's toggle (shared toggles switch their partners
    /// too: the charger and battery, lock and unlock, both device events).
    pub fn set_in(&self, cfg: &mut SoundsConfig, on: bool) {
        let toggle = match self {
            SoundEvent::VolumeChange => &mut cfg.volume_change,
            SoundEvent::BrightnessChange => &mut cfg.brightness_change,
            SoundEvent::NotificationLow => &mut cfg.notification_low,
            SoundEvent::Notification | SoundEvent::Named(_) => &mut cfg.notification,
            SoundEvent::NotificationCritical => &mut cfg.notification_critical,
            SoundEvent::PowerPlug | SoundEvent::PowerUnplug | SoundEvent::BatteryLow | SoundEvent::BatteryCritical => &mut cfg.charger,
            SoundEvent::Screenshot => &mut cfg.screenshot,
            SoundEvent::Login => &mut cfg.login,
            SoundEvent::Lock | SoundEvent::Unlock => &mut cfg.lock,
            SoundEvent::DeviceAdded | SoundEvent::DeviceRemoved => &mut cfg.devices,
            SoundEvent::TrashEmpty => &mut cfg.trash,
            SoundEvent::Complete => &mut cfg.complete,
        };
        *toggle = on;
    }

    /// Whether this sound comes unasked (the system or an app made it), so
    /// do not disturb silences it. Feedback for something you just did
    /// (volume and brightness ticks, a screenshot, emptying the Trash) and
    /// battery alerts still play. Notifications are decided by the
    /// notification service (it already knows about do not disturb).
    pub fn interrupts(&self) -> bool {
        !matches!(
            self,
            SoundEvent::VolumeChange
                | SoundEvent::BrightnessChange
                | SoundEvent::Screenshot
                | SoundEvent::TrashEmpty
                | SoundEvent::BatteryLow
                | SoundEvent::BatteryCritical
                | SoundEvent::Notification
                | SoundEvent::NotificationLow
                | SoundEvent::NotificationCritical
                | SoundEvent::Named(_)
        )
    }

    /// Whether the event should sound now under `cfg`: sounds on, the event
    /// on, and not silenced by do not disturb (`dnd`).
    pub fn allowed(&self, cfg: &SoundsConfig, dnd: bool) -> bool {
        cfg.enabled && cfg.volume > 0.0 && self.enabled_in(cfg) && !(dnd && cfg.quiet_in_dnd && self.interrupts())
    }
}

// ── Theme lookup ─────────────────────────────────────────────────────────

/// `$XDG_DATA_HOME/sounds` then each `$XDG_DATA_DIRS/sounds`.
pub fn sound_roots() -> Vec<PathBuf> {
    roots_for(dirs::data_dir().as_deref(), std::env::var("XDG_DATA_DIRS").ok().as_deref())
}

/// The sound folders for a data home and an `$XDG_DATA_DIRS` value (unset
/// or empty: [`DEFAULT_DATA_DIRS`], which covers FreeBSD's
/// `/usr/local/share` and Linux's `/usr/share`). No duplicates.
pub fn roots_for(data_home: Option<&Path>, data_dirs: Option<&str>) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = data_home.map(|h| h.join("sounds")).into_iter().collect();
    let dirs = data_dirs.filter(|s| !s.trim().is_empty()).unwrap_or(DEFAULT_DATA_DIRS);
    for root in dirs.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join("sounds")) {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
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
                    // Comma-separated by the spec; some themes use spaces.
                    subdirs.extend(v.split(|c: char| c == ',' || c.is_whitespace()).map(str::to_string).filter(|s| !s.is_empty()));
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

    /// PCM bytes (16-bit samples).
    pub fn bytes(&self) -> usize {
        self.samples.len() * std::mem::size_of::<i16>()
    }

    /// The level outline: the loudest sample in each of [`PEAKS`] equal
    /// stretches, scaled so the loudest stretch reads 255 (quiet sounds
    /// keep their shape).
    pub fn peaks(&self) -> [u8; PEAKS] {
        let mut raw = [0u16; PEAKS];
        let n = self.samples.len();
        if n == 0 {
            return [0; PEAKS];
        }
        for (i, s) in self.samples.iter().enumerate() {
            let bin = i * PEAKS / n;
            raw[bin] = raw[bin].max(s.unsigned_abs());
        }
        let top = f32::from(raw.iter().copied().max().unwrap_or(0).max(1));
        raw.map(|v| (f32::from(v) / top * f32::from(u8::MAX)).round() as u8)
    }

    /// Mono stays mono; anything wider folds down to stereo (even
    /// channels left, odd channels right).
    fn into_stereo_at_most(mut self) -> Self {
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
    Ok(clip.into_stereo_at_most())
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

/// What a play did: the sound's length and level outline (Studio draws it
/// while the preview plays).
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Played {
    pub duration_ms: u32,
    pub peaks: [u8; PEAKS],
}

impl Played {
    fn of(clip: &Clip) -> Self {
        Self { duration_ms: u32::try_from(clip.duration().as_millis()).unwrap_or(u32::MAX), peaks: clip.peaks() }
    }
}

/// Told how a queued sound went (previews wait for it).
pub type Reply = Box<dyn FnOnce(Result<Played, String>) + Send>;

enum Job {
    Play { path: PathBuf, volume: f32, event: String, reply: Option<Reply> },
    /// Remove every sound kept ready (the theme changed).
    Release,
}

/// The most recently used entries, bounded by count and by bytes; the
/// least recently used fall out first.
struct Lru<T> {
    entries: VecDeque<(String, usize, T)>,
    bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

impl<T> Lru<T> {
    fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self { entries: VecDeque::with_capacity(max_entries), bytes: 0, max_entries, max_bytes }
    }

    fn get(&mut self, key: &str) -> Option<&T> {
        let i = self.entries.iter().position(|(k, ..)| k == key)?;
        let entry = self.entries.remove(i)?;
        self.entries.push_back(entry);
        self.entries.back().map(|(_, _, v)| v)
    }

    fn remove(&mut self, key: &str) -> Option<T> {
        let i = self.entries.iter().position(|(k, ..)| k == key)?;
        let (_, bytes, v) = self.entries.remove(i)?;
        self.bytes -= bytes;
        Some(v)
    }

    /// Adds `value` (holding `bytes`) as the most recent entry and returns
    /// the keys that fell out. An entry larger than the byte budget is kept
    /// alone.
    fn insert(&mut self, key: String, bytes: usize, value: T) -> Vec<String> {
        let _ = self.remove(&key);
        let mut evicted = Vec::new();
        while !self.entries.is_empty() && (self.entries.len() >= self.max_entries || self.bytes + bytes > self.max_bytes) {
            if let Some((k, b, _)) = self.entries.pop_front() {
                self.bytes -= b;
                evicted.push(k);
            }
        }
        self.bytes += bytes;
        self.entries.push_back((key, bytes, value));
        evicted
    }

    /// Empties it, capacity included; returns the keys.
    fn clear(&mut self) -> Vec<String> {
        let keys = self.entries.drain(..).map(|(k, ..)| k).collect();
        self.entries.shrink_to_fit();
        self.bytes = 0;
        keys
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// A sound kept ready: its outline, and on OSS its decoded PCM (the
/// server holds it otherwise).
struct Ready {
    played: Played,
    #[cfg(target_os = "freebsd")]
    pcm: Option<Clip>,
}

/// Where sounds go.
enum Output {
    Pulse(Pulse),
    #[cfg(target_os = "freebsd")]
    Oss,
}

struct Worker {
    out: Option<Output>,
    ready: Lru<Ready>,
    failed: HashSet<PathBuf>,
    /// OSS scratch (bytes for the device), reused between sounds.
    #[cfg(target_os = "freebsd")]
    scratch: Vec<u8>,
}

fn worker() -> &'static Mutex<Option<std::sync::mpsc::SyncSender<Job>>> {
    static WORKER: OnceLock<Mutex<Option<std::sync::mpsc::SyncSender<Job>>>> = OnceLock::new();
    WORKER.get_or_init(|| Mutex::new(None))
}

/// A cache name that changes when the file or the volume baked into it
/// does.
fn sample_name(path: &Path, volume: f32) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    volume_permille(volume).hash(&mut h);
    if let Ok(meta) = std::fs::metadata(path) {
        meta.len().hash(&mut h);
        meta.modified().ok().hash(&mut h);
    }
    format!("mandara-{:016x}", h.finish())
}

/// `volume` (0–1) in thousandths: the cache tells volumes apart this finely.
fn volume_permille(volume: f32) -> u16 {
    (volume.clamp(0.0, 1.0) * 1000.0).round() as u16
}

/// Writes `samples` scaled by `volume` (0–1) into `out` as little-endian
/// bytes; `out` is cleared first and reused.
pub(crate) fn scaled_le(samples: &[i16], volume: f32, out: &mut Vec<u8>) {
    let gain = volume.clamp(0.0, 1.0);
    out.clear();
    out.reserve(std::mem::size_of_val(samples));
    for s in samples {
        let v = (f32::from(*s) * gain).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
}

/// Uploads `clip` at `volume` (0–1, baked into the samples) into the
/// server's sample cache as `name`.
fn upload(p: &mut Pulse, name: &str, clip: &Clip, volume: f32) -> Result<(), String> {
    let length = u32::try_from(clip.bytes()).map_err(|_| "sound too long".to_string())?;
    let mut props = protocol::Props::new();
    props.set(protocol::Prop::MediaRole, CString::from(c"event"));
    // pipewire-pulse names cache entries from the properties (`event.id`,
    // else `media.name`), not from the request's name field.
    let cname = CString::new(name).map_err(|e| e.to_string())?;
    props.set(protocol::Prop::EventId, cname.clone());
    props.set(protocol::Prop::MediaName, cname.clone());
    let params = protocol::UploadStreamParams {
        media_name: Some(cname),
        sample_spec: protocol::SampleSpec { format: protocol::SampleFormat::S16Le, channels: clip.channels, sample_rate: clip.rate },
        channel_map: if clip.channels == 1 { protocol::ChannelMap::mono() } else { protocol::ChannelMap::stereo() },
        length,
        props,
    };
    let reply: protocol::CreateUploadStreamReply = p.request(Command::CreateUploadStream(params)).map_err(|e| format!("create upload: {e}"))?;
    // Converted (and scaled, as on the OSS path) chunk by chunk: no second
    // full-size copy of the clip.
    let mut chunk = Vec::with_capacity(UPLOAD_CHUNK);
    for samples in clip.samples.chunks(UPLOAD_CHUNK / std::mem::size_of::<i16>()) {
        scaled_le(samples, volume, &mut chunk);
        p.write_data(reply.channel, &chunk)?;
    }
    p.ack(Command::FinishUploadStream(reply.channel)).map_err(|e| format!("finish upload: {e}"))
}

/// The properties of one play: an event sound from Mandara that keeps its
/// own level (see [`NO_RESTORE`]).
fn play_props(event: &str) -> Result<protocol::Props, String> {
    let mut props = protocol::Props::new();
    props.set(protocol::Prop::MediaRole, CString::from(c"event"));
    props.set(protocol::Prop::EventId, CString::new(event).map_err(|e| e.to_string())?);
    props.set(protocol::Prop::ApplicationName, CString::from(c"Mandara Shell"));
    let (key, value) = NO_RESTORE;
    props.set_bytes(key, value.to_bytes_with_nul());
    Ok(props)
}

/// Plays cached sample `name` at full level (its volume is in its samples:
/// PipeWire ignores the request's volume, PulseAudio would apply it twice).
fn play_sample(p: &mut Pulse, name: &str, event: &str) -> Result<(), String> {
    let props = play_props(event)?;
    let params = protocol::PlaySampleParams {
        sink_index: None,
        sink_name: None,
        volume: VOLUME_NORM,
        name: CString::new(name).map_err(|e| e.to_string())?,
        props,
    };
    // The reply carries the new sink input's index.
    p.request::<protocol::LookupReply>(Command::PlaySample(params)).map(|_| ())
}

fn remove_sample(p: &mut Pulse, name: &str) {
    if let Ok(name) = CString::new(name)
        && let Err(err) = p.ack(Command::RemoveSample(name))
    {
        tracing::debug!("sounds: removing a cached sound: {err}");
    }
}

impl Worker {
    fn new() -> Self {
        Self {
            out: None,
            ready: Lru::new(CACHE_SLOTS, CACHE_BYTES),
            failed: HashSet::new(),
            #[cfg(target_os = "freebsd")]
            scratch: Vec::new(),
        }
    }

    /// The output, connecting first if needed: the sound server, else (on
    /// FreeBSD) the OSS device.
    fn output(&mut self) -> Result<&mut Output, String> {
        if self.out.is_none() {
            let out = match Pulse::connect("Mandara Shell (sounds)") {
                Ok(p) => Output::Pulse(p),
                #[cfg(target_os = "freebsd")]
                Err(_) if Path::new(crate::freebsd::dsp::DEVICE).exists() => Output::Oss,
                Err(err) => return Err(err),
            };
            // A new connection: the PCM kept for another output is no use.
            self.drop_ready();
            self.out = Some(out);
        }
        self.out.as_mut().ok_or_else(|| "no sound output".to_string())
    }

    /// Forgets what is ready, removing it from the server.
    fn drop_ready(&mut self) {
        let keys = self.ready.clear();
        if let Some(Output::Pulse(p)) = self.out.as_mut() {
            for k in &keys {
                remove_sample(p, k);
            }
        }
    }

    fn play(&mut self, path: &Path, volume: f32, event: &str) -> Result<Played, String> {
        if self.failed.contains(path) {
            return Err(format!("{}: can't be played", path.display()));
        }
        let name = sample_name(path, volume);
        // One retry with a fresh connection (the server may have restarted,
        // which also empties its sample cache).
        let mut last = String::new();
        for _ in 0..2 {
            let played = match self.ready.get(&name) {
                Some(r) => r.played,
                None => {
                    let clip = match decode(path) {
                        Ok(clip) => clip,
                        Err(err) => {
                            self.failed.insert(path.to_path_buf());
                            return Err(err);
                        }
                    };
                    let played = Played::of(&clip);
                    let bytes = clip.bytes();
                    let ready = match self.output()? {
                        Output::Pulse(p) => {
                            if let Err(err) = upload(p, &name, &clip, volume) {
                                last = err;
                                self.out = None;
                                continue;
                            }
                            // The server holds the sound now.
                            drop(clip);
                            Ready {
                                played,
                                #[cfg(target_os = "freebsd")]
                                pcm: None,
                            }
                        }
                        #[cfg(target_os = "freebsd")]
                        Output::Oss => Ready { played, pcm: Some(clip) },
                    };
                    for gone in self.ready.insert(name.clone(), bytes, ready) {
                        if let Some(Output::Pulse(p)) = self.out.as_mut() {
                            remove_sample(p, &gone);
                        }
                    }
                    played
                }
            };
            let result = match self.output()? {
                Output::Pulse(p) => play_sample(p, &name, event),
                #[cfg(target_os = "freebsd")]
                Output::Oss => match self.ready.get(&name).and_then(|r| r.pcm.as_ref()) {
                    Some(clip) => crate::freebsd::dsp::play(&clip.samples, clip.channels, clip.rate, volume, &mut self.scratch),
                    None => Err("not decoded".into()),
                },
            };
            match result {
                Ok(()) => return Ok(played),
                Err(err) => {
                    tracing::debug!("sounds: playing {}: {err}", path.display());
                    last = err;
                    self.ready.remove(&name);
                    self.out = None;
                }
            }
        }
        Err(last)
    }

    /// Removes everything kept ready, disconnects and gives the decode
    /// buffers back to the system.
    fn close(&mut self) {
        self.drop_ready();
        self.out = None;
        self.failed = HashSet::new();
        #[cfg(target_os = "freebsd")]
        {
            self.scratch = Vec::new();
        }
        crate::memory::request_trim();
    }
}

/// The worker: one connection and the sounds kept ready, until idle.
fn run_worker(rx: std::sync::mpsc::Receiver<Job>) {
    let mut w = Worker::new();
    loop {
        let job = match rx.recv_timeout(IDLE_CLOSE) {
            Ok(job) => job,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Idle: give everything back and end the thread (the next
                // sound starts a new one).
                let mut slot = worker().lock().unwrap_or_else(|e| e.into_inner());
                if let Ok(job) = rx.try_recv() {
                    drop(slot);
                    run_job(&mut w, job);
                    continue;
                }
                *slot = None;
                drop(slot);
                w.close();
                return;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                w.close();
                return;
            }
        };
        run_job(&mut w, job);
    }
}

fn run_job(w: &mut Worker, job: Job) {
    match job {
        Job::Play { path, volume, event, reply } => {
            let result = w.play(&path, volume, &event);
            if let Err(err) = &result {
                // Sounds are rare; a failed one is worth a line in the log.
                tracing::warn!("sounds: {event} can't play ({}): {err}", path.display());
            }
            if let Some(reply) = reply {
                reply(result);
            }
        }
        Job::Release => {
            if w.ready.len() > 0 {
                w.drop_ready();
                crate::memory::request_trim();
            }
        }
    }
}

fn last_played() -> &'static Mutex<HashMap<String, Instant>> {
    static LAST: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    LAST.get_or_init(Default::default)
}

/// Whether `event` played too recently to play again; remembers it.
fn throttled(event: &str, now: Instant) -> bool {
    let mut last = last_played().lock().unwrap_or_else(|e| e.into_inner());
    if last.get(event).is_some_and(|t| now.duration_since(*t) < MIN_REPEAT) {
        return true;
    }
    if last.len() >= REPEAT_KEYS {
        last.retain(|_, t| now.duration_since(*t) < MIN_REPEAT);
    }
    last.insert(event.to_string(), now);
    false
}

/// Hands `job` to the worker, starting one if none runs. `Err` gives the
/// job back when the queue is full.
fn send(job: Job) -> Result<(), Job> {
    let mut w = worker().lock().unwrap_or_else(|e| e.into_inner());
    let job = match w.as_ref() {
        Some(tx) => match tx.try_send(job) {
            Ok(()) => return Ok(()),
            Err(std::sync::mpsc::TrySendError::Full(job)) => return Err(job),
            // The worker went idle and ended: start a new one.
            Err(std::sync::mpsc::TrySendError::Disconnected(job)) => job,
        },
        None => job,
    };
    let (tx, rx) = std::sync::mpsc::sync_channel(QUEUE);
    if let Err(err) = tx.try_send(job) {
        return Err(match err {
            std::sync::mpsc::TrySendError::Full(j) | std::sync::mpsc::TrySendError::Disconnected(j) => j,
        });
    }
    if let Err(err) = std::thread::Builder::new().name("mandara-sounds".into()).spawn(move || run_worker(rx)) {
        tracing::warn!("sounds: no sound worker: {err}");
        return Ok(());
    }
    *w = Some(tx);
    Ok(())
}

/// Queues `path` (an event's file) at `volume` (0–1). Returns `false`
/// when it was throttled or the queue is full.
pub fn play_file(path: PathBuf, volume: f32, event: &str) -> bool {
    play_file_then(path, volume, event, None)
}

/// [`play_file`], telling `reply` how it went (also when it was throttled
/// or dropped).
pub fn play_file_then(path: PathBuf, volume: f32, event: &str, reply: Option<Reply>) -> bool {
    if throttled(event, Instant::now()) {
        if let Some(reply) = reply {
            reply(Err("played a moment ago".into()));
        }
        return false;
    }
    match send(Job::Play { path, volume, event: event.to_string(), reply }) {
        Ok(()) => true,
        Err(Job::Play { reply: Some(reply), .. }) => {
            reply(Err("too many sounds at once".into()));
            false
        }
        Err(_) => false,
    }
}

/// Removes the sounds kept ready (the theme changed). Nothing to do when no
/// worker runs: it keeps nothing once it ends.
pub fn release() {
    let w = worker().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(tx) = w.as_ref() {
        let _ = tx.try_send(Job::Release);
    }
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

/// Plays `event` from `theme` (installed or not applied) at `volume`,
/// telling `reply` how it went; the file, or `None` when the theme has no
/// sound for it. Studio's gallery previews.
pub fn preview_theme(event: &SoundEvent, theme: &str, volume: f32, reply: Option<Reply>) -> Option<PathBuf> {
    let path = lookup_in(&sound_roots(), theme, &event.names())?;
    // Throttled per theme: previewing two themes back to back plays both.
    play_file_then(path.clone(), volume, &format!("{theme}/{}", event.key()), reply);
    Some(path)
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

    /// The volume is baked into a cached sound, so another volume is another
    /// cache entry (PipeWire ignores the volume of a play request).
    #[test]
    fn cached_sounds_differ_by_volume() {
        let p = Path::new("/nonexistent/bell.oga");
        assert_eq!(sample_name(p, 0.5), sample_name(p, 0.5));
        assert_ne!(sample_name(p, 0.5), sample_name(p, 0.3));
        assert_eq!(sample_name(p, 1.0), sample_name(p, 7.0), "clamped");
        let mut chunk = Vec::new();
        scaled_le(&[1000, -1000], 0.3, &mut chunk);
        assert_eq!(chunk, [300i16.to_le_bytes(), (-300i16).to_le_bytes()].concat());
    }

    /// A muted "System sounds" level on the server silenced every shell
    /// sound: plays ask the server not to restore it.
    #[test]
    fn plays_keep_their_own_level() {
        let props = play_props("notification").unwrap();
        assert_eq!(props.get_bytes(c"state.restore-props"), Some(&b"false\0"[..]));
        assert_eq!(props.get(protocol::Prop::MediaRole), Some(&b"event\0"[..]), "still an event sound for routing");
        assert_eq!(props.get(protocol::Prop::EventId), Some(&b"notification\0"[..]));
    }

    /// The decode → PCM path yields audible samples (not silence) and a
    /// level outline that follows them.
    #[test]
    fn a_tone_decodes_to_audible_pcm() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tone.wav");
        let rate = 48_000u32;
        let spec = hound::WavSpec { channels: 2, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        // 0.25 s of 440 Hz at half scale, fading out.
        let n = rate / 4;
        for i in 0..n {
            let t = i as f32 / rate as f32;
            let fade = 1.0 - i as f32 / n as f32;
            let s = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5 * fade * f32::from(i16::MAX);
            w.write_sample(s as i16).unwrap();
            w.write_sample(s as i16).unwrap();
        }
        w.finalize().unwrap();
        let clip = decode(&path).unwrap();
        assert_eq!((clip.channels, clip.rate), (2, rate));
        let rms = (clip.samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / clip.samples.len() as f64).sqrt();
        assert!(rms > 3000.0, "audible, rms {rms}");
        let played = Played::of(&clip);
        assert_eq!(played.duration_ms, 250);
        assert_eq!(played.peaks[0], u8::MAX, "loudest at the start");
        assert!(played.peaks[PEAKS - 1] < played.peaks[0] / 4, "fades out: {:?}", played.peaks);
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
        // Volume 0: silent samples.
        upload(&mut p, name, &clip, 0.0).unwrap();
        play_sample(&mut p, name, "test").unwrap();
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

    #[test]
    fn sub_folders_listed_with_spaces_or_commas_are_searched() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        for (theme, sep) in [("spaced", " "), ("commas", ",")] {
            let dir = root.join(theme);
            std::fs::create_dir_all(dir.join("stereo/alert")).unwrap();
            std::fs::create_dir_all(dir.join("stereo/action")).unwrap();
            std::fs::write(dir.join("index.theme"), format!("[Sound Theme]\nDirectories=stereo/alert{sep}stereo/action\n[stereo/alert]\nOutputProfile=stereo\n")).unwrap();
            std::fs::write(dir.join("stereo/action/bell.oga"), b"x").unwrap();
            assert!(lookup_in(std::slice::from_ref(&root), theme, &["bell"]).is_some_and(|p| p.ends_with("stereo/action/bell.oga")), "{theme}");
        }
    }

    #[test]
    fn every_event_has_a_key_label_and_name() {
        for e in SoundEvent::ALL {
            assert_eq!(SoundEvent::parse(e.key()), e);
            assert!(!e.names().is_empty() && !e.label().is_empty());
            // Its toggle switches it and only what shares that toggle.
            let mut cfg = SoundsConfig::default();
            for on in [false, true] {
                e.set_in(&mut cfg, on);
                assert_eq!(e.enabled_in(&cfg), on, "{e:?}");
            }
        }
        let mut cfg = SoundsConfig::default();
        SoundEvent::Lock.set_in(&mut cfg, false);
        assert!(!SoundEvent::Unlock.enabled_in(&cfg) && SoundEvent::Login.enabled_in(&cfg));
        assert!(SoundEvent::CONFIGURABLE.iter().all(|e| SoundEvent::ALL.contains(e)));
        let mut keys: Vec<&str> = SoundEvent::ALL.iter().map(SoundEvent::key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), SoundEvent::ALL.len());
    }

    #[test]
    fn do_not_disturb_silences_only_what_comes_unasked() {
        let cfg = SoundsConfig { enabled: true, ..Default::default() };
        assert!(SoundEvent::DeviceAdded.allowed(&cfg, false));
        assert!(!SoundEvent::DeviceAdded.allowed(&cfg, true), "a drive plugged in waits for do not disturb");
        assert!(!SoundEvent::Lock.allowed(&cfg, true) && !SoundEvent::Login.allowed(&cfg, true) && !SoundEvent::PowerPlug.allowed(&cfg, true));
        for e in [SoundEvent::VolumeChange, SoundEvent::Screenshot, SoundEvent::TrashEmpty, SoundEvent::BatteryLow, SoundEvent::BatteryCritical] {
            assert!(e.allowed(&cfg, true), "{e:?} answers you or warns: it plays");
        }
        let loud = SoundsConfig { quiet_in_dnd: false, ..cfg.clone() };
        assert!(SoundEvent::DeviceAdded.allowed(&loud, true));
        assert!(!SoundEvent::BrightnessChange.allowed(&cfg, false), "brightness ticks start off");
        assert!(!SoundEvent::VolumeChange.allowed(&SoundsConfig { volume: 0.0, ..cfg.clone() }, false));
        assert!(!SoundEvent::VolumeChange.allowed(&SoundsConfig::default(), false), "sounds start off");
    }

    #[test]
    fn roots_cover_freebsd_and_linux_system_folders() {
        let home = Path::new("/home/u/.local/share");
        let roots = roots_for(Some(home), None);
        assert_eq!(roots, vec![PathBuf::from("/home/u/.local/share/sounds"), PathBuf::from("/usr/local/share/sounds"), PathBuf::from("/usr/share/sounds")]);
        assert_eq!(roots_for(Some(home), Some("  ")), roots, "empty means unset");
        let custom = roots_for(None, Some("/opt/share::/usr/local/share:/opt/share"));
        assert_eq!(custom, vec![PathBuf::from("/opt/share/sounds"), PathBuf::from("/usr/local/share/sounds")]);
    }

    #[test]
    fn the_ready_cache_is_bounded_by_count_and_bytes() {
        let mut lru: Lru<u8> = Lru::new(3, 100);
        assert!(lru.insert("a".into(), 10, 1).is_empty());
        assert!(lru.insert("b".into(), 10, 2).is_empty());
        assert!(lru.insert("c".into(), 10, 3).is_empty());
        assert_eq!(lru.get("a"), Some(&1), "a is now the most recent");
        assert_eq!(lru.insert("d".into(), 10, 4), vec!["b".to_string()], "the least recently used falls out");
        assert_eq!(lru.insert("e".into(), 90, 5), vec!["c".to_string(), "a".to_string()], "bytes count too");
        assert_eq!((lru.len(), lru.bytes), (2, 100));
        assert_eq!(lru.insert("huge".into(), 500, 6), vec!["d".to_string(), "e".to_string()], "one oversized sound is kept alone");
        assert_eq!(lru.insert("huge".into(), 5, 7), Vec::<String>::new(), "replacing a key frees its old bytes");
        assert_eq!((lru.len(), lru.bytes), (1, 5));
        assert_eq!(lru.clear(), vec!["huge".to_string()]);
        assert_eq!((lru.len(), lru.bytes, lru.entries.capacity()), (0, 0, 0), "cleared with its capacity");
        let shipped: Lru<Ready> = Lru::new(CACHE_SLOTS, CACHE_BYTES);
        assert!(shipped.entries.capacity() >= CACHE_SLOTS);
        // A sound kept ready on the server costs its key and outline here.
        assert!(std::mem::size_of::<Ready>() <= 96, "{}", std::mem::size_of::<Ready>());
    }

    #[test]
    fn peaks_outline_the_loudest_part() {
        let mut samples = vec![0i16; 2400];
        samples[1200] = 8000;
        samples[100] = -4000;
        let clip = Clip { rate: 48_000, channels: 1, samples };
        let p = clip.peaks();
        assert_eq!(p[12], u8::MAX);
        assert_eq!(p[1], 128);
        assert_eq!(p[23], 0);
        assert_eq!(Clip { rate: 1, channels: 1, samples: Vec::new() }.peaks(), [0; PEAKS]);
        let played = Played::of(&clip);
        assert_eq!(played.duration_ms, 50);
    }

    #[test]
    fn previews_always_hear_back() {
        // A missing file fails while decoding, before any sound server is
        // contacted; a second request at once is throttled.
        let (tx, rx) = std::sync::mpsc::channel();
        let tx2 = tx.clone();
        let path = PathBuf::from("/nonexistent/mandara-test.oga");
        play_file_then(path.clone(), 0.0, "test-preview", Some(Box::new(move |r| tx.send(r).unwrap())));
        play_file_then(path, 0.0, "test-preview", Some(Box::new(move |r| tx2.send(r).unwrap())));
        let mut got: Vec<Result<Played, String>> = (0..2).map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap()).collect();
        got.sort_by_key(|r| r.as_ref().err().cloned());
        assert!(got.iter().all(Result::is_err));
        assert!(got.iter().any(|r| r.as_ref().err().is_some_and(|e| e == "played a moment ago")));
    }
}
