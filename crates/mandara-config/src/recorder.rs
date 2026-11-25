//! The screen recorder's settings, kept in the screenshot plugin's table
//! (`[plugins.settings.screenshot]`, keys starting with `record_`) next to
//! the capture options, so Studio shows them on the Screenshot plugin's
//! settings. Every key is optional: a missing or invalid one falls back to
//! its default alone, so older configs keep loading.

use serde::{Deserialize, Serialize};

/// Default values and bounds of the recorder settings.
pub mod defaults {
    /// Frames per second (the encoder's default and the slider's start).
    pub const FPS: u32 = 30;
    /// The frame rates offered (Studio, the panel), lowest first.
    pub const FPS_CHOICES: [u32; 5] = [24, 30, 60, 90, 120];
    /// Bounds of a frame rate typed into the config by hand.
    pub const FPS_RANGE: (u32, u32) = (5, 240);
    /// Built-in encoder: frames are scaled down to at most this width, px.
    pub const WIDTH_PX: u32 = 1600;
    pub const WIDTH_RANGE_PX: (u32, u32) = (640, 3840);
    /// Custom quality: the video bitrate, kbit/s, and its bounds.
    pub const BITRATE_KBPS: u32 = 12_000;
    pub const BITRATE_RANGE_KBPS: (u32, u32) = (1_000, 150_000);
    /// The countdown choices before a recording starts, seconds (0 = off).
    pub const COUNTDOWN_CHOICES_S: [u64; 4] = [0, 3, 5, 10];
    /// Longest countdown a hand-edited config may ask for, seconds.
    pub const COUNTDOWN_MAX_S: u64 = 60;
    /// `strftime` pattern of recording names (no extension).
    pub const FILENAME: &str = "Recording_%Y-%m-%d_%H-%M-%S";
}

/// Which program records.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecorderBackend {
    /// The best one installed: GPU Screen Recorder, then wf-recorder, then
    /// the built-in encoder.
    #[default]
    Auto,
    /// Mandara's own AV1 / WebM encoder (always available, no audio).
    Builtin,
    GpuScreenRecorder,
    WfRecorder,
}

/// What a recording started without an explicit source captures (the
/// `record start` / `record toggle` commands and shortcuts).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordSource {
    /// The display with the focused window (or the chosen display).
    #[default]
    Screen,
    /// Pick an area first.
    Region,
    /// The focused window's area.
    Window,
}

/// The largest frame size a recording is scaled down to (never up).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordResolution {
    #[default]
    #[serde(rename = "native")]
    Native,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "1440p")]
    P1440,
    #[serde(rename = "4k")]
    P2160,
}

impl RecordResolution {
    /// The bounding box, px; `None` keeps the source size.
    pub fn bounds(self) -> Option<(u32, u32)> {
        match self {
            RecordResolution::Native => None,
            RecordResolution::P1080 => Some((1920, 1080)),
            RecordResolution::P1440 => Some((2560, 1440)),
            RecordResolution::P2160 => Some((3840, 2160)),
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordCodec {
    #[default]
    H264,
    Hevc,
    Av1,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordQuality {
    Low,
    Medium,
    #[default]
    High,
    /// A fixed bitrate ([`RecorderSettings::bitrate_kbps`]).
    Custom,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordContainer {
    #[default]
    Mp4,
    Mkv,
    Webm,
}

impl RecordContainer {
    pub fn extension(self) -> &'static str {
        match self {
            RecordContainer::Mp4 => "mp4",
            RecordContainer::Mkv => "mkv",
            RecordContainer::Webm => "webm",
        }
    }
}

/// Which sound goes into a recording.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordAudio {
    #[default]
    None,
    /// What the computer plays (the output device's monitor).
    System,
    /// The microphone (the input device).
    Mic,
    /// Both, mixed into one track.
    Both,
}

impl RecordAudio {
    pub fn system(self) -> bool {
        matches!(self, RecordAudio::System | RecordAudio::Both)
    }

    pub fn mic(self) -> bool {
        matches!(self, RecordAudio::Mic | RecordAudio::Both)
    }
}

/// The recorder's settings (`record_*` keys of
/// `[plugins.settings.screenshot]`, Studio → Plugins → Screenshot).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecorderSettings {
    #[serde(rename = "record_backend")]
    pub backend: RecorderBackend,
    /// What `record start` and the shortcuts capture.
    #[serde(rename = "record_source")]
    pub source: RecordSource,
    /// Output name to record (`DP-1`); empty = the display with the
    /// focused window.
    #[serde(rename = "record_monitor")]
    pub monitor: String,
    #[serde(rename = "record_resolution")]
    pub resolution: RecordResolution,
    #[serde(rename = "record_fps")]
    pub fps: u32,
    /// The built-in encoder's width limit, px.
    #[serde(rename = "record_width")]
    pub width: u32,
    #[serde(rename = "record_codec")]
    pub codec: RecordCodec,
    #[serde(rename = "record_quality")]
    pub quality: RecordQuality,
    /// Video bitrate for [`RecordQuality::Custom`], kbit/s.
    #[serde(rename = "record_bitrate")]
    pub bitrate_kbps: u32,
    #[serde(rename = "record_container")]
    pub container: RecordContainer,
    #[serde(rename = "record_audio")]
    pub audio: RecordAudio,
    /// Output device whose sound is recorded (a server name); empty = the
    /// default output.
    #[serde(rename = "record_audio_output")]
    pub audio_output: String,
    /// Input device recorded as the microphone; empty = the default input.
    #[serde(rename = "record_audio_input")]
    pub audio_input: String,
    /// Seconds counted down before recording starts (0 = at once).
    #[serde(rename = "record_countdown")]
    pub countdown: u64,
    /// Folder (`~` allowed); empty = Videos/Recordings.
    #[serde(rename = "record_directory")]
    pub directory: String,
    /// `strftime` file name pattern, without extension.
    #[serde(rename = "record_filename")]
    pub filename: String,
    /// Open the folder in the file manager when a recording is saved.
    #[serde(rename = "record_open_folder")]
    pub open_folder: bool,
}

impl Default for RecorderSettings {
    fn default() -> Self {
        Self {
            backend: RecorderBackend::Auto,
            source: RecordSource::Screen,
            monitor: String::new(),
            resolution: RecordResolution::Native,
            fps: defaults::FPS,
            width: defaults::WIDTH_PX,
            codec: RecordCodec::H264,
            quality: RecordQuality::High,
            bitrate_kbps: defaults::BITRATE_KBPS,
            container: RecordContainer::Mp4,
            audio: RecordAudio::None,
            audio_output: String::new(),
            audio_input: String::new(),
            countdown: 0,
            directory: String::new(),
            filename: defaults::FILENAME.into(),
            open_folder: false,
        }
    }
}

impl RecorderSettings {
    /// The `record_*` keys of the screenshot plugin's table; keys that are
    /// missing or invalid fall back to their defaults one by one.
    pub fn from_plugins(plugins: &crate::PluginsConfig) -> Self {
        let Some(table) = plugins.settings.get("screenshot") else { return Self::default() };
        let mut out = toml::Table::try_from(Self::default()).unwrap_or_default();
        for (k, v) in table.iter().filter(|(k, _)| k.starts_with("record_")) {
            let mut trial = out.clone();
            trial.insert(k.clone(), v.clone());
            if toml::Value::Table(trial.clone()).try_into::<Self>().is_ok() {
                out = trial;
            }
        }
        toml::Value::Table(out).try_into().unwrap_or_default()
    }

    /// [`Self::fps`] within its bounds.
    pub fn fps(&self) -> u32 {
        self.fps.clamp(defaults::FPS_RANGE.0, defaults::FPS_RANGE.1)
    }

    /// [`Self::width`] within its bounds.
    pub fn width(&self) -> u32 {
        self.width.clamp(defaults::WIDTH_RANGE_PX.0, defaults::WIDTH_RANGE_PX.1)
    }

    /// [`Self::bitrate_kbps`] within its bounds.
    pub fn bitrate_kbps(&self) -> u32 {
        self.bitrate_kbps.clamp(defaults::BITRATE_RANGE_KBPS.0, defaults::BITRATE_RANGE_KBPS.1)
    }

    /// [`Self::countdown`] within its bounds.
    pub fn countdown_secs(&self) -> u64 {
        self.countdown.min(defaults::COUNTDOWN_MAX_S)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ShellConfig;

    fn with(table: &str) -> RecorderSettings {
        let mut cfg = ShellConfig::default();
        cfg.plugins.settings.insert("screenshot".into(), toml::from_str(table).unwrap());
        RecorderSettings::from_plugins(&cfg.plugins)
    }

    #[test]
    fn missing_table_and_keys_give_defaults() {
        assert_eq!(RecorderSettings::from_plugins(&ShellConfig::default().plugins), RecorderSettings::default());
        assert_eq!(with("copy = false\ndirectory = \"~/x\""), RecorderSettings::default(), "screenshot keys are not the recorder's");
    }

    #[test]
    fn old_keys_still_read() {
        let s = with("record_fps = 25\nrecord_width = 1280\nrecord_directory = \"~/Clips\"");
        assert_eq!((s.fps, s.width, s.directory.as_str()), (25, 1280, "~/Clips"));
    }

    #[test]
    fn one_bad_key_keeps_the_others() {
        let s = with("record_codec = \"av1\"\nrecord_container = \"avi\"\nrecord_audio = \"both\"\nrecord_resolution = \"1440p\"\nrecord_backend = \"wf-recorder\"");
        assert_eq!(s.codec, RecordCodec::Av1);
        assert_eq!(s.container, RecordContainer::Mp4, "an unknown container falls back alone");
        assert_eq!(s.audio, RecordAudio::Both);
        assert_eq!(s.resolution, RecordResolution::P1440);
        assert_eq!(s.backend, RecorderBackend::WfRecorder);
    }

    #[test]
    fn values_are_clamped() {
        let s = with("record_fps = 1000\nrecord_bitrate = 1\nrecord_countdown = 999\nrecord_width = 10");
        assert_eq!(s.fps(), defaults::FPS_RANGE.1);
        assert_eq!(s.bitrate_kbps(), defaults::BITRATE_RANGE_KBPS.0);
        assert_eq!(s.countdown_secs(), defaults::COUNTDOWN_MAX_S);
        assert_eq!(s.width(), defaults::WIDTH_RANGE_PX.0);
    }

    #[test]
    fn resolution_boxes_and_audio_roles() {
        assert_eq!(RecordResolution::Native.bounds(), None);
        assert_eq!(RecordResolution::P2160.bounds(), Some((3840, 2160)));
        assert!(RecordAudio::Both.system() && RecordAudio::Both.mic());
        assert!(!RecordAudio::None.system() && !RecordAudio::None.mic());
        assert_eq!(RecordContainer::Mkv.extension(), "mkv");
    }
}
