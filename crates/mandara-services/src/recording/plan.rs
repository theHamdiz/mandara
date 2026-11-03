//! From settings to a command line: [`plan`] fits the recorder settings to
//! what the chosen backend supports (each change noted with its reason),
//! and [`gsr_args`] / [`wf_args`] build the argument vectors the external
//! recorders run with (never a shell string). Pure, so every combination is
//! unit-tested.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use mandara_config::recorder::{RecordAudio, RecordCodec, RecordContainer, RecordQuality, RecordResolution, RecorderSettings};
use serde::Serialize;

use super::backend::{self, Backend, GsrInfo};

/// Built-in encoder quantizers per quality preset (0–255, lower is
/// better): low, medium, high. High is the recorder's long-standing value.
const BUILTIN_QUANTIZERS: [u8; 3] = [160, 135, super::RECORD_QUANTIZER];
/// wf-recorder constant-quality factors per preset (low, medium, high) for
/// x264 / x265 (0–51) and SVT-AV1 (0–63); lower is better.
const X264_CRF: [u8; 3] = [28, 23, 18];
const X265_CRF: [u8; 3] = [30, 26, 22];
const SVTAV1_CRF: [u8; 3] = [42, 35, 28];
/// wf-recorder encoder speed: fast enough for real-time capture.
const X26X_PRESET: &str = "veryfast";
const SVTAV1_PRESET: &str = "10";

/// The settings a recording really uses on its backend.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Plan {
    pub backend: Backend,
    pub codec: RecordCodec,
    pub container: RecordContainer,
    pub fps: u32,
    pub quality: RecordQuality,
    pub bitrate_kbps: u32,
    pub resolution: RecordResolution,
    /// The built-in encoder's width limit, px.
    pub max_width: u32,
    pub audio: RecordAudio,
    /// Device names; empty = the default device.
    pub audio_output: String,
    pub audio_input: String,
    /// Settings changed to fit the backend, with why (`tk` text).
    pub notes: Vec<&'static str>,
}

impl Plan {
    /// `H.264 · MP4 · 60 fps`.
    pub fn summary(&self) -> String {
        let codec = match self.codec {
            RecordCodec::H264 => "H.264",
            RecordCodec::Hevc => "HEVC",
            RecordCodec::Av1 => "AV1",
        };
        let container = match self.container {
            RecordContainer::Mp4 => "MP4",
            RecordContainer::Mkv => "MKV",
            RecordContainer::Webm => "WebM",
        };
        format!("{codec} · {container} · {} fps", self.fps)
    }

    /// The built-in encoder's quantizer for this quality.
    pub fn quantizer(&self) -> u8 {
        BUILTIN_QUANTIZERS[preset(self.quality)]
    }
}

/// Index of a quality preset in the per-preset tables (custom: high).
fn preset(q: RecordQuality) -> usize {
    match q {
        RecordQuality::Low => 0,
        RecordQuality::Medium => 1,
        RecordQuality::High | RecordQuality::Custom => 2,
    }
}

/// Fits `s` to `backend`: an unsupported codec, container, frame rate,
/// quality or sound source becomes the closest supported one, and the
/// reason goes into [`Plan::notes`].
pub fn plan(s: &RecorderSettings, backend: Backend, info: Option<&GsrInfo>) -> Plan {
    let caps = backend::caps(backend, info);
    let mut notes = Vec::new();
    let mut note = |reason: Option<&'static str>| {
        if let Some(r) = reason
            && !notes.contains(&r)
        {
            notes.push(r);
        }
    };
    let codec = if caps.codecs.contains(&s.codec) {
        s.codec
    } else {
        note(backend::codec_reason(backend, s.codec, info));
        caps.codecs.first().copied().unwrap_or(RecordCodec::H264)
    };
    let container = match backend::container_reason(backend, s.container, codec, info) {
        None => s.container,
        reason => {
            note(reason);
            [RecordContainer::Mp4, RecordContainer::Mkv, RecordContainer::Webm]
                .into_iter()
                .find(|c| backend::container_reason(backend, *c, codec, info).is_none())
                .unwrap_or(RecordContainer::Mkv)
        }
    };
    let fps = s.fps();
    note(backend::fps_reason(backend, fps));
    let fps = fps.min(caps.max_fps);
    let quality = match backend::quality_reason(backend, s.quality) {
        None => s.quality,
        reason => {
            note(reason);
            RecordQuality::High
        }
    };
    let audio = match backend::audio_reason(backend, s.audio) {
        None => s.audio,
        reason => {
            note(reason);
            // One source at a time: the microphone (narration) wins.
            if caps.audio { RecordAudio::Mic } else { RecordAudio::None }
        }
    };
    Plan {
        backend,
        codec,
        container,
        fps,
        quality,
        bitrate_kbps: s.bitrate_kbps(),
        resolution: s.resolution,
        max_width: s.width(),
        audio,
        audio_output: s.audio_output.trim().to_string(),
        audio_input: s.audio_input.trim().to_string(),
        notes,
    }
}

/// What to record, in logical layout coordinates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Target {
    /// A whole output by name.
    Output { name: String },
    /// An area of `output` (whose top-left is at `origin` in the layout);
    /// `rect` is relative to the output.
    Region { output: String, origin: (i32, i32), rect: (i32, i32, i32, i32) },
}

impl Target {
    pub fn output(&self) -> &str {
        match self {
            Target::Output { name } => name,
            Target::Region { output, .. } => output,
        }
    }

    /// The area relative to its output (the built-in encoder's region).
    pub fn local_rect(&self) -> Option<(i32, i32, i32, i32)> {
        match self {
            Target::Output { .. } => None,
            Target::Region { rect, .. } => Some(*rect),
        }
    }

    /// The area in layout coordinates, size at least 2 × 2 and even.
    pub fn global_rect(&self) -> Option<(i32, i32, i32, i32)> {
        match self {
            Target::Output { .. } => None,
            Target::Region { origin, rect, .. } => Some((origin.0 + rect.0, origin.1 + rect.1, (rect.2 & !1).max(2), (rect.3 & !1).max(2))),
        }
    }
}

/// Default output's monitor in the sound server (wf-recorder, when the
/// default output's name could not be read).
const DEFAULT_MONITOR: &str = "@DEFAULT_MONITOR@";

/// `-a` sources for GPU Screen Recorder: system and microphone mixed into
/// one track.
fn gsr_audio(plan: &Plan) -> Option<String> {
    let system = plan.audio.system().then(|| if plan.audio_output.is_empty() { "default_output".to_string() } else { format!("device:{}.monitor", plan.audio_output) });
    let mic = plan.audio.mic().then(|| if plan.audio_input.is_empty() { "default_input".to_string() } else { format!("device:{}", plan.audio_input) });
    let sources: Vec<String> = system.into_iter().chain(mic).collect();
    (!sources.is_empty()).then(|| sources.join("|"))
}

fn push(args: &mut Vec<OsString>, items: &[&str]) {
    args.extend(items.iter().map(OsString::from));
}

/// GPU Screen Recorder's arguments for `plan`, `target` and the output file.
pub fn gsr_args(plan: &Plan, target: &Target, path: &Path) -> Vec<OsString> {
    let mut a: Vec<OsString> = Vec::new();
    match target.global_rect() {
        // A monitor by name, never "screen" (which means the first one).
        None => push(&mut a, &["-w", target.output()]),
        Some((x, y, w, h)) => push(&mut a, &["-w", "region", "-region", &format!("{w}x{h}+{x}+{y}")]),
    }
    push(&mut a, &["-c", plan.container.extension(), "-k", backend::codec_name(plan.codec), "-f", &plan.fps.to_string()]);
    match plan.quality {
        RecordQuality::Custom => push(&mut a, &["-bm", "cbr", "-q", &plan.bitrate_kbps.to_string()]),
        q => push(&mut a, &["-q", ["medium", "high", "very_high"][preset(q)]]),
    }
    if let Some((w, h)) = plan.resolution.bounds() {
        push(&mut a, &["-s", &format!("{w}x{h}")]);
    }
    if let Some(audio) = gsr_audio(plan) {
        push(&mut a, &["-a", &audio]);
    }
    push(&mut a, &["-cursor", "yes", "-v", "no", "-o"]);
    a.push(path.as_os_str().to_owned());
    a
}

/// wf-recorder's encoder (FFmpeg name) for a codec.
fn wf_encoder(codec: RecordCodec) -> &'static str {
    match codec {
        RecordCodec::H264 => "libx264",
        RecordCodec::Hevc => "libx265",
        RecordCodec::Av1 => "libsvtav1",
    }
}

/// wf-recorder's FFmpeg muxer for a container.
fn wf_muxer(container: RecordContainer) -> &'static str {
    match container {
        RecordContainer::Mp4 => "mp4",
        RecordContainer::Mkv => "matroska",
        RecordContainer::Webm => "webm",
    }
}

/// wf-recorder's arguments. `default_output` is the default output
/// device's name (for its monitor) when the system sound is recorded from
/// the default device.
pub fn wf_args(plan: &Plan, target: &Target, path: &Path, default_output: Option<&str>) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["-f".into(), path.as_os_str().to_owned()];
    match target.global_rect() {
        None => push(&mut a, &["-o", target.output()]),
        Some((x, y, w, h)) => push(&mut a, &["-g", &format!("{x},{y} {w}x{h}")]),
    }
    push(&mut a, &["-c", wf_encoder(plan.codec), "-m", wf_muxer(plan.container), "-r", &plan.fps.to_string()]);
    let (crf, speed) = match plan.codec {
        RecordCodec::H264 => (X264_CRF, X26X_PRESET),
        RecordCodec::Hevc => (X265_CRF, X26X_PRESET),
        RecordCodec::Av1 => (SVTAV1_CRF, SVTAV1_PRESET),
    };
    push(&mut a, &["-p", &format!("preset={speed}")]);
    match plan.quality {
        RecordQuality::Custom => push(&mut a, &["-p", &format!("b={}k", plan.bitrate_kbps)]),
        q => push(&mut a, &["-p", &format!("crf={}", crf[preset(q)])]),
    }
    if let Some((w, h)) = plan.resolution.bounds() {
        // Fit within the box, never upscale, even sides.
        push(&mut a, &["-F", &format!("scale=w='min({w},iw)':h='min({h},ih)':force_original_aspect_ratio=decrease:force_divisible_by=2")]);
    }
    // One source: the microphone, or what the output plays.
    match plan.audio {
        RecordAudio::None => {}
        RecordAudio::Mic | RecordAudio::Both if plan.audio_input.is_empty() => push(&mut a, &["--audio"]),
        RecordAudio::Mic | RecordAudio::Both => a.push(format!("--audio={}", plan.audio_input).into()),
        RecordAudio::System => {
            let device = if plan.audio_output.is_empty() { default_output.map(|d| format!("{d}.monitor")).unwrap_or_else(|| DEFAULT_MONITOR.into()) } else { format!("{}.monitor", plan.audio_output) };
            a.push(format!("--audio={device}").into());
        }
    }
    push(&mut a, &["-y"]);
    a
}

/// A recording's file: `template` (strftime) in `dir`, never an existing
/// file, with the container's extension.
pub fn output_path(dir: &Path, template: &str, container: RecordContainer, now: chrono::DateTime<chrono::Local>) -> PathBuf {
    crate::screenshot::unique_path(dir, &file_stem(template, now), container.extension())
}

/// A file name from a `strftime` pattern; an invalid or empty one falls
/// back to the default, and path separators become dashes.
pub fn file_stem(pattern: &str, now: chrono::DateTime<chrono::Local>) -> String {
    use chrono::format::{Item, StrftimeItems};
    let valid = |p: &str| !p.trim().is_empty() && !StrftimeItems::new(p).any(|i| matches!(i, Item::Error));
    let pattern = if valid(pattern) { pattern } else { mandara_config::recorder::defaults::FILENAME };
    let stem = now.format(pattern).to_string().replace(['/', '\\', '\0'], "-");
    let stem = stem.trim().trim_start_matches('.').to_string();
    if stem.is_empty() { "Recording".into() } else { stem }
}

/// The recordings folder: the setting (`~` allowed), else Videos/Recordings.
pub fn output_dir(setting: &str) -> PathBuf {
    let s = setting.trim();
    if s.is_empty() {
        dirs::video_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Videos")).join("Recordings")
    } else {
        mandara_config::paths::expand_tilde(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    fn strings(a: &[OsString]) -> Vec<String> {
        a.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    fn settings(f: impl FnOnce(&mut RecorderSettings)) -> RecorderSettings {
        let mut s = RecorderSettings::default();
        f(&mut s);
        s
    }

    fn screen() -> Target {
        Target::Output { name: "DP-1".into() }
    }

    fn region() -> Target {
        Target::Region { output: "DP-2".into(), origin: (2560, 0), rect: (100, 50, 801, 600) }
    }

    fn has(args: &[String], pair: &[&str]) -> bool {
        args.windows(pair.len()).any(|w| w == pair)
    }

    #[test]
    fn plan_keeps_supported_choices() {
        let s = settings(|s| {
            s.codec = RecordCodec::Hevc;
            s.container = RecordContainer::Mkv;
            s.fps = 120;
            s.audio = RecordAudio::Both;
            s.quality = RecordQuality::Custom;
        });
        let p = plan(&s, Backend::GpuScreenRecorder, None);
        assert_eq!((p.codec, p.container, p.fps, p.audio, p.quality), (RecordCodec::Hevc, RecordContainer::Mkv, 120, RecordAudio::Both, RecordQuality::Custom));
        assert!(p.notes.is_empty());
        assert_eq!(p.summary(), "HEVC · MKV · 120 fps");
    }

    #[test]
    fn plan_fits_the_builtin_encoder_and_says_why() {
        let s = settings(|s| {
            s.fps = 120;
            s.audio = RecordAudio::System;
            s.quality = RecordQuality::Custom;
        });
        let p = plan(&s, Backend::Builtin, None);
        assert_eq!((p.codec, p.container, p.fps, p.audio, p.quality), (RecordCodec::Av1, RecordContainer::Webm, 60, RecordAudio::None, RecordQuality::High));
        assert_eq!(p.notes.len(), 5, "{:?}", p.notes);
        assert_eq!(p.quantizer(), crate::recording::RECORD_QUANTIZER);
    }

    #[test]
    fn plan_moves_webm_off_non_av1_and_both_to_mic_on_wf() {
        let s = settings(|s| {
            s.container = RecordContainer::Webm;
            s.audio = RecordAudio::Both;
        });
        let p = plan(&s, Backend::WfRecorder, None);
        assert_eq!((p.codec, p.container, p.audio), (RecordCodec::H264, RecordContainer::Mp4, RecordAudio::Mic));
        assert_eq!(p.notes.len(), 2);
        let p = plan(&settings(|s| s.codec = RecordCodec::Av1), Backend::WfRecorder, None);
        assert!(p.notes.is_empty());
    }

    #[test]
    fn plan_drops_codecs_the_card_cannot_encode() {
        let info = GsrInfo { codecs: vec!["h264".into(), "hevc".into()], containers: vec![] };
        let p = plan(&settings(|s| s.codec = RecordCodec::Av1), Backend::GpuScreenRecorder, Some(&info));
        assert_eq!(p.codec, RecordCodec::H264);
        assert_eq!(p.notes.len(), 1);
    }

    #[test]
    fn regions_are_global_and_even() {
        assert_eq!(region().global_rect(), Some((2660, 50, 800, 600)));
        assert_eq!(region().local_rect(), Some((100, 50, 801, 600)));
        assert_eq!(screen().global_rect(), None);
        assert_eq!(region().output(), "DP-2");
    }

    #[test]
    fn gsr_screen_defaults() {
        let p = plan(&RecorderSettings::default(), Backend::GpuScreenRecorder, None);
        let a = strings(&gsr_args(&p, &screen(), Path::new("/v/out.mp4")));
        assert_eq!(a, ["-w", "DP-1", "-c", "mp4", "-k", "h264", "-f", "30", "-q", "very_high", "-cursor", "yes", "-v", "no", "-o", "/v/out.mp4"]);
    }

    #[test]
    fn gsr_every_source_codec_container_and_audio() {
        for codec in [RecordCodec::H264, RecordCodec::Hevc, RecordCodec::Av1] {
            for container in [RecordContainer::Mp4, RecordContainer::Mkv, RecordContainer::Webm] {
                for audio in [RecordAudio::None, RecordAudio::System, RecordAudio::Mic, RecordAudio::Both] {
                    for target in [screen(), region()] {
                        let s = settings(|s| {
                            s.codec = codec;
                            s.container = container;
                            s.audio = audio;
                        });
                        let p = plan(&s, Backend::GpuScreenRecorder, None);
                        let a = strings(&gsr_args(&p, &target, Path::new("/v/x")));
                        assert!(has(&a, &["-k", backend::codec_name(codec)]), "{a:?}");
                        assert!(has(&a, &["-c", p.container.extension()]), "{a:?}");
                        if container == RecordContainer::Webm && codec != RecordCodec::Av1 {
                            assert_ne!(p.container, RecordContainer::Webm);
                        }
                        let expected = match audio {
                            RecordAudio::None => None,
                            RecordAudio::System => Some("default_output"),
                            RecordAudio::Mic => Some("default_input"),
                            RecordAudio::Both => Some("default_output|default_input"),
                        };
                        match expected {
                            Some(src) => assert!(has(&a, &["-a", src]), "{a:?}"),
                            None => assert!(!a.contains(&"-a".to_string())),
                        }
                        match &target {
                            Target::Output { .. } => assert!(has(&a, &["-w", "DP-1"])),
                            Target::Region { .. } => assert!(has(&a, &["-w", "region", "-region", "800x600+2660+50"]), "{a:?}"),
                        }
                        assert_eq!(a.last().map(String::as_str), Some("/v/x"));
                    }
                }
            }
        }
    }

    #[test]
    fn gsr_quality_resolution_and_devices() {
        for (q, expected) in [(RecordQuality::Low, "medium"), (RecordQuality::Medium, "high"), (RecordQuality::High, "very_high")] {
            let a = strings(&gsr_args(&plan(&settings(|s| s.quality = q), Backend::GpuScreenRecorder, None), &screen(), Path::new("/x")));
            assert!(has(&a, &["-q", expected]) && !a.contains(&"-bm".to_string()), "{a:?}");
        }
        let s = settings(|s| {
            s.quality = RecordQuality::Custom;
            s.bitrate_kbps = 20_000;
            s.resolution = RecordResolution::P1440;
            s.fps = 90;
            s.audio = RecordAudio::Both;
            s.audio_output = "alsa_output.usb".into();
            s.audio_input = "alsa_input.mic".into();
        });
        let a = strings(&gsr_args(&plan(&s, Backend::GpuScreenRecorder, None), &screen(), Path::new("/x")));
        assert!(has(&a, &["-bm", "cbr", "-q", "20000"]), "{a:?}");
        assert!(has(&a, &["-s", "2560x1440"]) && has(&a, &["-f", "90"]));
        assert!(has(&a, &["-a", "device:alsa_output.usb.monitor|device:alsa_input.mic"]), "{a:?}");
    }

    #[test]
    fn wf_screen_defaults() {
        let p = plan(&RecorderSettings::default(), Backend::WfRecorder, None);
        let a = strings(&wf_args(&p, &screen(), Path::new("/v/out.mp4"), None));
        assert_eq!(a, ["-f", "/v/out.mp4", "-o", "DP-1", "-c", "libx264", "-m", "mp4", "-r", "30", "-p", "preset=veryfast", "-p", "crf=18", "-y"]);
    }

    #[test]
    fn wf_every_source_codec_container_and_audio() {
        for codec in [RecordCodec::H264, RecordCodec::Hevc, RecordCodec::Av1] {
            for container in [RecordContainer::Mp4, RecordContainer::Mkv, RecordContainer::Webm] {
                for audio in [RecordAudio::None, RecordAudio::System, RecordAudio::Mic, RecordAudio::Both] {
                    for target in [screen(), region()] {
                        let s = settings(|s| {
                            s.codec = codec;
                            s.container = container;
                            s.audio = audio;
                        });
                        let p = plan(&s, Backend::WfRecorder, None);
                        let a = strings(&wf_args(&p, &target, Path::new("/v/x"), Some("alsa_output.pci")));
                        assert!(has(&a, &["-c", wf_encoder(codec)]), "{a:?}");
                        assert!(has(&a, &["-m", wf_muxer(p.container)]), "{a:?}");
                        match &target {
                            Target::Output { .. } => assert!(has(&a, &["-o", "DP-1"])),
                            Target::Region { .. } => assert!(has(&a, &["-g", "2660,50 800x600"]), "{a:?}"),
                        }
                        let audio_args: Vec<&String> = a.iter().filter(|x| x.starts_with("--audio")).collect();
                        match audio {
                            RecordAudio::None => assert!(audio_args.is_empty()),
                            RecordAudio::System => assert_eq!(audio_args, ["--audio=alsa_output.pci.monitor"]),
                            RecordAudio::Mic | RecordAudio::Both => assert_eq!(audio_args, ["--audio"], "one source: the mic"),
                        }
                        assert_eq!(a.last().map(String::as_str), Some("-y"));
                    }
                }
            }
        }
    }

    #[test]
    fn wf_quality_resolution_and_devices() {
        let crf = |codec, q| {
            let a = strings(&wf_args(&plan(&settings(|s| {
                s.codec = codec;
                s.quality = q;
            }), Backend::WfRecorder, None), &screen(), Path::new("/x"), None));
            a.iter().find_map(|x| x.strip_prefix("crf=").map(str::to_string)).unwrap()
        };
        assert_eq!(crf(RecordCodec::H264, RecordQuality::Low), "28");
        assert_eq!(crf(RecordCodec::Hevc, RecordQuality::Medium), "26");
        assert_eq!(crf(RecordCodec::Av1, RecordQuality::High), "28");
        let s = settings(|s| {
            s.quality = RecordQuality::Custom;
            s.bitrate_kbps = 8_000;
            s.resolution = RecordResolution::P1080;
            s.audio = RecordAudio::System;
        });
        let a = strings(&wf_args(&plan(&s, Backend::WfRecorder, None), &screen(), Path::new("/x"), None));
        assert!(has(&a, &["-p", "b=8000k"]), "{a:?}");
        assert!(has(&a, &["-F", "scale=w='min(1920,iw)':h='min(1080,ih)':force_original_aspect_ratio=decrease:force_divisible_by=2"]), "{a:?}");
        assert!(a.contains(&"--audio=@DEFAULT_MONITOR@".to_string()), "unknown default output: the server's alias");
        let s = settings(|s| {
            s.audio = RecordAudio::Mic;
            s.audio_input = "alsa_input.usb".into();
        });
        let a = strings(&wf_args(&plan(&s, Backend::WfRecorder, None), &screen(), Path::new("/x"), None));
        assert!(a.contains(&"--audio=alsa_input.usb".to_string()));
    }

    #[test]
    fn file_names_from_templates() {
        let now = chrono::Local.with_ymd_and_hms(2026, 11, 3, 14, 5, 9).unwrap();
        assert_eq!(file_stem("Recording_%Y-%m-%d_%H-%M-%S", now), "Recording_2026-11-03_14-05-09");
        assert_eq!(file_stem("clips/%H%M", now), "clips-1405", "no folders from a name");
        assert_eq!(file_stem("", now), "Recording_2026-11-03_14-05-09");
        assert_eq!(file_stem("%Q bad", now), "Recording_2026-11-03_14-05-09", "an invalid pattern falls back");
        assert_eq!(file_stem("...", now), "Recording");
        let dir = std::env::temp_dir().join(format!("mandara-rec-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = output_path(&dir, "take", RecordContainer::Mkv, now);
        assert_eq!(first, dir.join("take.mkv"));
        std::fs::write(&first, b"x").unwrap();
        assert_eq!(output_path(&dir, "take", RecordContainer::Mkv, now), dir.join("take-2.mkv"), "never overwrites");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folders() {
        assert!(output_dir("").ends_with("Recordings"));
        assert_eq!(output_dir(" /tmp/clips "), PathBuf::from("/tmp/clips"));
    }
}
