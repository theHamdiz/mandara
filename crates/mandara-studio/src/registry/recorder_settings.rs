//! The screen recorder's settings (Studio → Plugins → Screenshot): which
//! recorder, what it records, the video and sound, and where files go.
//! Choices the recorder in use can't do are disabled with the reason
//! ([`choice_locked`]); settings that don't apply right now are locked
//! ([`locked`]).

use mandara_config::ShellConfig;
use mandara_config::recorder::{RecordAudio, RecordContainer, RecordQuality, RecorderBackend, RecorderSettings, defaults};
use mandara_i18n::tk;
use mandara_services::recording::backend::{self, Backend, GsrInfo, Installed};
use serde_json::Value;

use super::{Control, Section, Setting, sd};

pub const RECORDER: &str = tk("Screen recording");
pub const RECORDER_VIDEO: &str = tk("Recording video");
pub const RECORDER_SOUND: &str = tk("Recording sound");
pub const RECORDER_FILES: &str = tk("Recording files");

const P: &str = "/plugins/settings/screenshot/";

const BACKENDS: &[(&str, &str)] = &[("auto", tk("Automatic")), ("gpu-screen-recorder", "GPU Screen Recorder"), ("wf-recorder", "wf-recorder"), ("builtin", tk("Built-in encoder"))];
const SOURCES: &[(&str, &str)] = &[("screen", tk("Screen")), ("region", tk("Region")), ("window", tk("Window"))];
const RESOLUTIONS: &[(&str, &str)] = &[("native", tk("Native")), ("1080p", "1080p"), ("1440p", "1440p"), ("4k", "4K")];
const FPS: &[(&str, &str)] = &[("24", "24"), ("30", "30"), ("60", "60"), ("90", "90"), ("120", "120")];
const CODECS: &[(&str, &str)] = &[("h264", "H.264"), ("hevc", "HEVC"), ("av1", "AV1")];
const QUALITIES: &[(&str, &str)] = &[("low", tk("Low")), ("medium", tk("Medium")), ("high", tk("High")), ("custom", tk("Custom bitrate"))];
const CONTAINERS: &[(&str, &str)] = &[("mp4", "MP4"), ("mkv", "MKV"), ("webm", "WebM")];
const AUDIO: &[(&str, &str)] = &[("none", tk("None")), ("system", tk("System sound")), ("mic", tk("Microphone")), ("both", tk("Both"))];
const COUNTDOWNS: &[(&str, &str)] = &[("0", tk("Off")), ("3", "3 s"), ("5", "5 s"), ("10", "10 s")];
/// Slider step of the custom bitrate, kbit/s.
const BITRATE_STEP_KBPS: f64 = 500.0;
/// Slider step of the built-in encoder's width limit, px.
const WIDTH_STEP_PX: f64 = 80.0;

pub fn entries() -> Vec<Setting> {
    use Control::*;
    use Section::Plugins;
    let s = |group: &'static str, key: &'static str, label: &'static str, help: &'static str, control: Control, default: Value| -> Setting {
        let path: &'static str = Box::leak(format!("{P}{key}").into_boxed_str());
        sd(Plugins, group, path, label, help, control, default)
    };
    let d = RecorderSettings::default();
    let (bmin, bmax) = defaults::BITRATE_RANGE_KBPS;
    let (wmin, wmax) = defaults::WIDTH_RANGE_PX;
    vec![
        s(RECORDER, "record_backend", tk("Recorder"), tk("Automatic picks GPU Screen Recorder, then wf-recorder, then the built-in encoder (AV1, no sound)"), Choice(BACKENDS), Value::from("auto")),
        s(RECORDER, "record_backends", tk("Installed recorders"), tk("What each recorder can do on this computer"), RecorderBackends, Value::Null),
        s(RECORDER, "record_source", tk("Default source"), tk("What the record shortcut and `record start` capture"), Choice(SOURCES), Value::from("screen")),
        s(RECORDER, "record_monitor", tk("Display"), tk("The screen recorded; Focused display follows the focused window"), Display, Value::from("")),
        s(RECORDER, "record_countdown", tk("Countdown"), tk("Seconds before recording starts (Esc cancels)"), Choice(COUNTDOWNS), Value::from(0)),
        s(RECORDER_VIDEO, "record_resolution", tk("Resolution"), tk("Larger screens are scaled down to fit; smaller ones keep their size"), Choice(RESOLUTIONS), Value::from("native")),
        s(RECORDER_VIDEO, "record_fps", tk("Frame rate"), tk("Frames per second"), Choice(FPS), Value::from(d.fps)),
        s(RECORDER_VIDEO, "record_codec", tk("Codec"), tk("H.264 plays everywhere; HEVC and AV1 are smaller at the same quality"), Choice(CODECS), Value::from("h264")),
        s(RECORDER_VIDEO, "record_quality", tk("Quality"), tk("Higher is sharper and larger"), Choice(QUALITIES), Value::from("high")),
        s(RECORDER_VIDEO, "record_bitrate", tk("Bitrate"), tk("Video bitrate of the Custom quality"), Slider { min: f64::from(bmin), max: f64::from(bmax), step: BITRATE_STEP_KBPS, unit: " kbit/s" }, Value::from(d.bitrate_kbps)),
        s(RECORDER_VIDEO, "record_container", tk("Format"), tk("MKV survives a crash; MP4 plays everywhere; WebM holds AV1"), Choice(CONTAINERS), Value::from("mp4")),
        s(RECORDER_VIDEO, "record_width", tk("Built-in encoder width"), tk("The built-in encoder scales recordings down to at most this width"), Slider { min: f64::from(wmin), max: f64::from(wmax), step: WIDTH_STEP_PX, unit: " px" }, Value::from(d.width)),
        s(RECORDER_SOUND, "record_audio", tk("Sound"), tk("System sound, the microphone or both (mixed into one track)"), Choice(AUDIO), Value::from("none")),
        s(RECORDER_SOUND, "record_audio_output", tk("Output device"), tk("Whose sound is recorded as system sound"), Device { input: false }, Value::from("")),
        s(RECORDER_SOUND, "record_audio_input", tk("Microphone"), tk("The input recorded as the microphone"), Device { input: true }, Value::from("")),
        s(RECORDER_FILES, "record_directory", tk("Folder"), tk("Where screen recordings go (empty: Videos/Recordings)"), Text { placeholder: "~/Videos/Recordings" }, Value::from("")),
        s(RECORDER_FILES, "record_filename", tk("File name"), tk("strftime pattern, e.g. Recording_%Y-%m-%d_%H-%M-%S"), Text { placeholder: defaults::FILENAME }, Value::from(defaults::FILENAME)),
        s(RECORDER_FILES, "record_open_folder", tk("Open the folder afterwards"), tk("Shows the new recording in your file manager"), Toggle, Value::from(false)),
    ]
}

fn settings(cfg: &ShellConfig) -> RecorderSettings {
    RecorderSettings::from_plugins(&cfg.plugins)
}

/// The backend the settings mean on this computer (a chosen one even when
/// it is missing) and GPU Screen Recorder's codec report when known.
fn effective(s: &RecorderSettings) -> (Backend, Option<GsrInfo>) {
    let installed = Installed::cached();
    let backend = installed.resolve(s.backend).unwrap_or_else(|_| Backend::from_setting(s.backend).unwrap_or(Backend::Builtin));
    let info = match (&installed.gpu_screen_recorder, backend) {
        (Some(path), Backend::GpuScreenRecorder) => backend::gsr_info(path),
        _ => None,
    };
    (backend, info)
}

/// Why a recorder setting doesn't apply right now.
pub fn locked(path: &str, cfg: &ShellConfig) -> Option<&'static str> {
    let key = path.strip_prefix(P)?;
    let s = settings(cfg);
    let (backend, _) = effective(&s);
    match key {
        "record_bitrate" if backend == Backend::Builtin => Some(tk("The built-in encoder has quality presets only")),
        "record_bitrate" if s.quality != RecordQuality::Custom => Some(tk("For the Custom quality")),
        "record_width" if backend != Backend::Builtin => Some(tk("Only the built-in encoder uses this; the others follow Resolution")),
        "record_audio_output" | "record_audio_input" if backend == Backend::Builtin => Some(tk("The built-in encoder records no sound")),
        "record_audio_output" if !s.audio.system() => Some(tk("Pick System sound or Both under Sound first")),
        "record_audio_input" if !s.audio.mic() => Some(tk("Pick Microphone or Both under Sound first")),
        _ => None,
    }
}

/// A choice's value as the settings type it stands for.
fn parse<T: serde::de::DeserializeOwned>(raw: &str) -> Option<T> {
    serde_json::from_value(super::choice_value(raw)).ok()
}

/// Why one choice of a recorder setting can't be picked with the recorder
/// in use (it is shown disabled with this reason).
pub fn choice_locked(path: &str, raw: &str, cfg: &ShellConfig) -> Option<&'static str> {
    let key = path.strip_prefix(P)?;
    let s = settings(cfg);
    if key == "record_backend" {
        let chosen = parse::<RecorderBackend>(raw)?;
        let b = Backend::from_setting(chosen)?;
        return (!Installed::cached().has(b)).then(|| b.missing_reason());
    }
    let (backend, info) = effective(&s);
    match key {
        "record_codec" => backend::codec_reason(backend, parse(raw)?, info.as_ref()),
        "record_container" => {
            let container: RecordContainer = parse(raw)?;
            // The codec this backend really uses (as the recording plan does).
            backend::container_reason(backend, container, backend::codec_used(backend, s.codec, info.as_ref()), info.as_ref())
        }
        "record_fps" => backend::fps_reason(backend, raw.parse().ok()?),
        "record_quality" => backend::quality_reason(backend, parse(raw)?),
        "record_audio" => {
            let audio: RecordAudio = parse(raw)?;
            backend::audio_reason(backend, audio)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(pairs: &[(&str, toml::Value)]) -> ShellConfig {
        let mut cfg = ShellConfig::default();
        let table = cfg.plugins.settings.entry("screenshot".into()).or_default();
        for (k, v) in pairs {
            table.insert((*k).into(), v.clone());
        }
        cfg
    }

    #[test]
    fn every_choice_value_parses_into_the_settings() {
        for s in entries() {
            let Control::Choice(choices) = s.control else { continue };
            let key = s.path.strip_prefix(P).unwrap();
            for (raw, _) in choices {
                let value: toml::Value = serde_json::from_value(super::super::choice_value(raw)).unwrap();
                let cfg = cfg_with(&[(key, value.clone())]);
                let back = toml::Value::try_from(settings(&cfg)).unwrap();
                assert_eq!(back.get(key), Some(&value), "{key} = {raw} doesn't reach RecorderSettings");
            }
        }
    }

    #[test]
    fn builtin_disables_what_it_cannot_do() {
        let cfg = cfg_with(&[("record_backend", toml::Value::from("builtin"))]);
        let path = |k: &str| format!("{P}{k}");
        assert!(choice_locked(&path("record_codec"), "h264", &cfg).is_some());
        assert!(choice_locked(&path("record_codec"), "av1", &cfg).is_none());
        assert!(choice_locked(&path("record_container"), "mp4", &cfg).is_some());
        assert!(choice_locked(&path("record_container"), "webm", &cfg).is_none());
        assert!(choice_locked(&path("record_fps"), "120", &cfg).is_some());
        assert!(choice_locked(&path("record_fps"), "60", &cfg).is_none());
        assert!(choice_locked(&path("record_audio"), "mic", &cfg).is_some());
        assert!(choice_locked(&path("record_audio"), "none", &cfg).is_none());
        assert!(choice_locked(&path("record_quality"), "custom", &cfg).is_some());
        assert!(locked(&path("record_width"), &cfg).is_none(), "the built-in width applies");
        assert!(locked(&path("record_audio_input"), &cfg).is_some());
        assert_eq!(choice_locked(&path("record_backend"), "builtin", &cfg), None, "always available");
        assert_eq!(choice_locked(&path("record_backend"), "auto", &cfg), None);
    }

    #[test]
    fn settings_that_wait_for_another() {
        let path = |k: &str| format!("{P}{k}");
        let cfg = cfg_with(&[("record_backend", toml::Value::from("gpu-screen-recorder")), ("record_audio", toml::Value::from("system"))]);
        assert_eq!(locked(&path("record_bitrate"), &cfg), Some(tk("For the Custom quality")));
        assert!(locked(&path("record_width"), &cfg).is_some(), "only the built-in encoder");
        assert!(locked(&path("record_audio_output"), &cfg).is_none());
        assert_eq!(locked(&path("record_audio_input"), &cfg), Some(tk("Pick Microphone or Both under Sound first")));
        let cfg = cfg_with(&[("record_backend", toml::Value::from("gpu-screen-recorder")), ("record_quality", toml::Value::from("custom")), ("record_codec", toml::Value::from("h264"))]);
        assert!(locked(&path("record_bitrate"), &cfg).is_none());
        assert!(choice_locked(&path("record_container"), "webm", &cfg).is_some(), "WebM needs AV1");
        assert!(locked("/bar/height", &cfg).is_none());
    }
}
