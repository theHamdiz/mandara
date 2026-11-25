//! The programs that can record the screen and what each supports: Mandara's
//! built-in AV1 encoder, GPU Screen Recorder and wf-recorder. Detection is a
//! `PATH` lookup (cheap, done on demand); GPU Screen Recorder's `--info`
//! (which codecs this graphics card encodes) runs once in the background and
//! is cached until its binary changes.
//!
//! Every "can't" here comes with a sentence saying why (`tk` text), so
//! Studio disables the choice with its reason and a recording that has to
//! change a setting says so.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use mandara_config::recorder::{RecordAudio, RecordCodec, RecordContainer, RecordQuality, RecorderBackend};
use mandara_i18n::tk;
use serde::Serialize;

/// Built-in encoder: frame-rate ceiling (real-time AV1 on the CPU).
pub use super::MAX_FPS as BUILTIN_MAX_FPS;

/// A `PATH` lookup is reused this long by [`Installed::cached`] (views
/// ask on every frame; a recorder installed meanwhile shows up after it).
const DETECT_TTL: Duration = Duration::from_secs(5);
static INSTALLED: Mutex<Option<(Instant, Installed)>> = Mutex::new(None);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    GpuScreenRecorder,
    WfRecorder,
    Builtin,
}

impl Backend {
    /// In order of preference for [`RecorderBackend::Auto`].
    pub const ALL: [Backend; 3] = [Backend::GpuScreenRecorder, Backend::WfRecorder, Backend::Builtin];

    /// The executable an external backend runs.
    pub fn program(self) -> Option<&'static str> {
        match self {
            Backend::GpuScreenRecorder => Some("gpu-screen-recorder"),
            Backend::WfRecorder => Some("wf-recorder"),
            Backend::Builtin => None,
        }
    }

    /// The name shown to people (`tk` text for the built-in one).
    pub fn label(self) -> &'static str {
        match self {
            Backend::GpuScreenRecorder => "GPU Screen Recorder",
            Backend::WfRecorder => "wf-recorder",
            Backend::Builtin => tk("Built-in encoder"),
        }
    }

    /// The config value naming this backend.
    pub fn setting(self) -> RecorderBackend {
        match self {
            Backend::GpuScreenRecorder => RecorderBackend::GpuScreenRecorder,
            Backend::WfRecorder => RecorderBackend::WfRecorder,
            Backend::Builtin => RecorderBackend::Builtin,
        }
    }

    pub fn from_setting(setting: RecorderBackend) -> Option<Backend> {
        match setting {
            RecorderBackend::Auto => None,
            RecorderBackend::GpuScreenRecorder => Some(Backend::GpuScreenRecorder),
            RecorderBackend::WfRecorder => Some(Backend::WfRecorder),
            RecorderBackend::Builtin => Some(Backend::Builtin),
        }
    }

    /// Why this backend can't be used: not installed.
    pub fn missing_reason(self) -> &'static str {
        match self {
            Backend::GpuScreenRecorder => tk("GPU Screen Recorder isn't installed"),
            Backend::WfRecorder => tk("wf-recorder isn't installed"),
            Backend::Builtin => tk("Always available"),
        }
    }
}

/// Which external recorders are on `PATH`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Installed {
    pub gpu_screen_recorder: Option<PathBuf>,
    pub wf_recorder: Option<PathBuf>,
}

impl Installed {
    /// Looks the programs up on `PATH` (a few `stat`s).
    pub fn detect() -> Installed {
        Installed { gpu_screen_recorder: which("gpu-screen-recorder"), wf_recorder: which("wf-recorder") }
    }

    /// [`Installed::detect`], reused for a few seconds.
    pub fn cached() -> Installed {
        if let Ok(c) = INSTALLED.lock()
            && let Some((at, i)) = c.as_ref()
            && at.elapsed() < DETECT_TTL
        {
            return i.clone();
        }
        let i = Installed::detect();
        if let Ok(mut c) = INSTALLED.lock() {
            *c = Some((Instant::now(), i.clone()));
        }
        i
    }

    pub fn has(&self, backend: Backend) -> bool {
        match backend {
            Backend::GpuScreenRecorder => self.gpu_screen_recorder.is_some(),
            Backend::WfRecorder => self.wf_recorder.is_some(),
            Backend::Builtin => true,
        }
    }

    /// The backend a setting means: the chosen one, or the best installed
    /// for `auto`. `Err` names why a chosen one can't run.
    pub fn resolve(&self, setting: RecorderBackend) -> Result<Backend, &'static str> {
        match Backend::from_setting(setting) {
            Some(b) if self.has(b) => Ok(b),
            Some(b) => Err(b.missing_reason()),
            None => Ok(Backend::ALL.into_iter().find(|b| self.has(*b)).unwrap_or(Backend::Builtin)),
        }
    }
}

/// An executable named `program` on `PATH`.
pub fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|p| executable(p))
}

/// Whether a candidate is an executable file (symlinks are followed).
pub(crate) fn executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// What GPU Screen Recorder reports about this machine (`--info`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct GsrInfo {
    /// Video codecs this graphics card encodes (`h264`, `hevc`, `av1`, …).
    pub codecs: Vec<String>,
    pub containers: Vec<String>,
}

impl GsrInfo {
    /// Parses `gpu-screen-recorder --info`: `section=name` lines, then one
    /// value per line (`key|value` in some sections).
    pub fn parse(text: &str) -> GsrInfo {
        let mut info = GsrInfo::default();
        let mut section = "";
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if let Some(name) = line.strip_prefix("section=") {
                section = name;
                continue;
            }
            let value = line.split('|').next().unwrap_or(line).to_string();
            match section {
                "video_codecs" => info.codecs.push(value),
                "containers" => info.containers.push(value),
                _ => {}
            }
        }
        info
    }

    pub fn encodes(&self, codec: RecordCodec) -> bool {
        self.codecs.iter().any(|c| c == codec_name(codec))
    }

    pub fn writes(&self, container: RecordContainer) -> bool {
        self.containers.is_empty() || self.containers.iter().any(|c| c == container.extension())
    }
}

/// The codec's name in GPU Screen Recorder's `-k` and `--info`.
pub fn codec_name(codec: RecordCodec) -> &'static str {
    match codec {
        RecordCodec::H264 => "h264",
        RecordCodec::Hevc => "hevc",
        RecordCodec::Av1 => "av1",
    }
}

/// The cached `--info` of the installed GPU Screen Recorder, keyed by its
/// path and modification time.
static GSR_INFO: Mutex<Option<GsrCache>> = Mutex::new(None);

struct GsrCache {
    path: PathBuf,
    modified: Option<SystemTime>,
    /// When `modified` was last compared with the binary (re-checked at
    /// most every [`DETECT_TTL`], views ask every frame).
    checked: Instant,
    info: Option<GsrInfo>,
}
/// Set while a probe runs, so only one does.
static PROBING: Mutex<bool> = Mutex::new(false);

fn stamp(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// GPU Screen Recorder's `--info` if it was read already for the binary
/// at `path`; otherwise starts reading it in the background and returns
/// `None` (callers fall back to the general capability table).
pub fn gsr_info(path: &Path) -> Option<GsrInfo> {
    match gsr_probe(path) {
        GsrProbe::Read(info) => Some(info),
        GsrProbe::Pending | GsrProbe::Failed => None,
    }
}

/// Where reading GPU Screen Recorder's `--info` stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GsrProbe {
    /// Not read yet for this binary (a read has been started).
    Pending,
    Read(GsrInfo),
    /// It ran but reported no codecs (failed, or no usable GPU).
    Failed,
}

/// [`gsr_info`] with "not read yet" and "could not be read" apart.
pub fn gsr_probe(path: &Path) -> GsrProbe {
    let known = |c: &GsrCache| c.info.clone().map_or(GsrProbe::Failed, GsrProbe::Read);
    if let Ok(mut cache) = GSR_INFO.lock()
        && let Some(c) = cache.as_mut()
        && c.path == path
    {
        if c.checked.elapsed() < DETECT_TTL {
            return known(c);
        }
        if c.modified == stamp(path) {
            c.checked = Instant::now();
            return known(c);
        }
    }
    probe_in_background(path.to_path_buf());
    GsrProbe::Pending
}

/// Reads `--info` now (blocking, a fraction of a second) and caches it.
pub fn gsr_info_now(path: &Path) -> Option<GsrInfo> {
    let info = std::process::Command::new(path)
        .arg("--info")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| GsrInfo::parse(&String::from_utf8_lossy(&o.stdout)))
        .filter(|i| !i.codecs.is_empty());
    if let Ok(mut c) = GSR_INFO.lock() {
        *c = Some(GsrCache { path: path.to_path_buf(), modified: stamp(path), checked: Instant::now(), info: info.clone() });
    }
    info
}

fn probe_in_background(path: PathBuf) {
    {
        let Ok(mut busy) = PROBING.lock() else { return };
        if *busy {
            return;
        }
        *busy = true;
    }
    let _ = std::thread::Builder::new().name("mandara-gsr-info".into()).spawn(move || {
        gsr_info_now(&path);
        if let Ok(mut busy) = PROBING.lock() {
            *busy = false;
        }
    });
}

/// What a backend can do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Caps {
    pub codecs: Vec<RecordCodec>,
    pub containers: Vec<RecordContainer>,
    pub max_fps: u32,
    /// Records sound at all.
    pub audio: bool,
    /// Records system sound and the microphone together.
    pub audio_both: bool,
    pub pause: bool,
    /// A custom bitrate (else quality presets only).
    pub bitrate: bool,
}

/// The capability table; `info` narrows GPU Screen Recorder's codecs to
/// what this graphics card encodes.
pub fn caps(backend: Backend, info: Option<&GsrInfo>) -> Caps {
    use RecordCodec::*;
    use RecordContainer::*;
    match backend {
        Backend::Builtin => Caps { codecs: vec![Av1], containers: vec![Webm], max_fps: BUILTIN_MAX_FPS, audio: false, audio_both: false, pause: true, bitrate: false },
        Backend::GpuScreenRecorder => Caps {
            codecs: [H264, Hevc, Av1].into_iter().filter(|c| info.is_none_or(|i| i.encodes(*c))).collect(),
            containers: [Mp4, Mkv, Webm].into_iter().filter(|c| info.is_none_or(|i| i.writes(*c))).collect(),
            max_fps: mandara_config::recorder::defaults::FPS_RANGE.1,
            audio: true,
            audio_both: true,
            pause: true,
            bitrate: true,
        },
        Backend::WfRecorder => Caps {
            codecs: vec![H264, Hevc, Av1],
            containers: vec![Mp4, Mkv, Webm],
            max_fps: mandara_config::recorder::defaults::FPS_RANGE.1,
            audio: true,
            audio_both: false,
            pause: false,
            bitrate: true,
        },
    }
}

/// The codec `backend` really records when `chosen` is asked for: the
/// chosen one if it can, else its first (the recording plan and Studio's
/// disabled choices agree through this).
pub fn codec_used(backend: Backend, chosen: RecordCodec, info: Option<&GsrInfo>) -> RecordCodec {
    let codecs = caps(backend, info).codecs;
    if codecs.contains(&chosen) { chosen } else { codecs.first().copied().unwrap_or(RecordCodec::H264) }
}

/// Why `backend` can't record `codec` (`tk` text).
pub fn codec_reason(backend: Backend, codec: RecordCodec, info: Option<&GsrInfo>) -> Option<&'static str> {
    if caps(backend, info).codecs.contains(&codec) {
        return None;
    }
    Some(match backend {
        Backend::Builtin => tk("The built-in encoder records AV1 only"),
        _ => tk("Your graphics card can't encode this with GPU Screen Recorder"),
    })
}

/// Why `backend` can't write `container` with `codec` (`tk` text).
pub fn container_reason(backend: Backend, container: RecordContainer, codec: RecordCodec, info: Option<&GsrInfo>) -> Option<&'static str> {
    if !caps(backend, info).containers.contains(&container) {
        return Some(match backend {
            Backend::Builtin => tk("The built-in encoder writes WebM only"),
            _ => tk("This recorder can't write that format here"),
        });
    }
    (container == RecordContainer::Webm && codec != RecordCodec::Av1).then_some(tk("WebM holds AV1 video only: pick AV1, or MP4 or MKV"))
}

/// Why `backend` can't record at `fps` (`tk` text).
pub fn fps_reason(backend: Backend, fps: u32) -> Option<&'static str> {
    (fps > caps(backend, None).max_fps).then_some(tk("The built-in encoder records at most 60 fps"))
}

/// Why `backend` can't record `audio` (`tk` text).
pub fn audio_reason(backend: Backend, audio: RecordAudio) -> Option<&'static str> {
    let c = caps(backend, None);
    match audio {
        RecordAudio::None => None,
        _ if !c.audio => Some(tk("The built-in encoder records no sound")),
        RecordAudio::Both if !c.audio_both => Some(tk("wf-recorder records one sound source at a time")),
        _ => None,
    }
}

/// Why `backend` can't use `quality` (`tk` text).
pub fn quality_reason(backend: Backend, quality: RecordQuality) -> Option<&'static str> {
    (quality == RecordQuality::Custom && !caps(backend, None).bitrate).then_some(tk("The built-in encoder has quality presets only"))
}

/// Why `backend` can't pause (`tk` text).
pub fn pause_reason(backend: Backend) -> Option<&'static str> {
    (!caps(backend, None).pause).then_some(tk("wf-recorder can't pause: stop, and start a new recording"))
}

/// One backend as Studio and `record backends` show it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BackendStatus {
    pub backend: Backend,
    pub installed: bool,
    pub path: Option<PathBuf>,
    pub caps: Caps,
    /// `--info` was read (codecs reflect this graphics card).
    pub probed: bool,
    /// `--info` ran but told nothing: every codec stays offered.
    pub probe_failed: bool,
}

/// Every backend with whether it is installed and what it supports (the
/// GPU Screen Recorder probe is started in the background if needed).
pub fn statuses(installed: &Installed) -> Vec<BackendStatus> {
    Backend::ALL
        .into_iter()
        .map(|backend| {
            let path = match backend {
                Backend::GpuScreenRecorder => installed.gpu_screen_recorder.clone(),
                Backend::WfRecorder => installed.wf_recorder.clone(),
                Backend::Builtin => None,
            };
            let probe = match (backend, path.as_deref()) {
                (Backend::GpuScreenRecorder, Some(p)) => gsr_probe(p),
                _ => GsrProbe::Pending,
            };
            let info = match &probe {
                GsrProbe::Read(info) => Some(info),
                _ => None,
            };
            BackendStatus { backend, installed: installed.has(backend), caps: caps(backend, info), probed: info.is_some(), probe_failed: probe == GsrProbe::Failed, path }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: &str = "section=system_info\ndisplay_server|wayland\nsection=gpu_info\nvendor|amd\nsection=video_codecs\nh264\nhevc\nhevc_hdr\nav1_10bit\nsection=containers\nmp4\nmkv\nwebm\nsection=capture_options\nDP-1|3840x2160\nregion\n";

    fn installed(gsr: bool, wf: bool) -> Installed {
        Installed { gpu_screen_recorder: gsr.then(|| "/usr/bin/gpu-screen-recorder".into()), wf_recorder: wf.then(|| "/usr/bin/wf-recorder".into()) }
    }

    #[test]
    fn auto_prefers_gpu_then_wf_then_builtin() {
        assert_eq!(installed(true, true).resolve(RecorderBackend::Auto), Ok(Backend::GpuScreenRecorder));
        assert_eq!(installed(false, true).resolve(RecorderBackend::Auto), Ok(Backend::WfRecorder));
        assert_eq!(installed(false, false).resolve(RecorderBackend::Auto), Ok(Backend::Builtin));
        assert_eq!(installed(false, false).resolve(RecorderBackend::Builtin), Ok(Backend::Builtin));
    }

    #[test]
    fn a_chosen_missing_backend_says_so() {
        assert_eq!(installed(false, true).resolve(RecorderBackend::GpuScreenRecorder), Err(Backend::GpuScreenRecorder.missing_reason()));
        assert_eq!(installed(true, false).resolve(RecorderBackend::WfRecorder), Err(Backend::WfRecorder.missing_reason()));
    }

    #[test]
    fn gsr_info_parses_codecs_and_containers() {
        let info = GsrInfo::parse(INFO);
        assert_eq!(info.codecs, ["h264", "hevc", "hevc_hdr", "av1_10bit"]);
        assert_eq!(info.containers, ["mp4", "mkv", "webm"]);
        assert!(info.encodes(RecordCodec::Hevc));
        assert!(!info.encodes(RecordCodec::Av1), "only 10-bit AV1 is not plain AV1");
        let c = caps(Backend::GpuScreenRecorder, Some(&info));
        assert_eq!(c.codecs, [RecordCodec::H264, RecordCodec::Hevc]);
        assert_eq!(codec_reason(Backend::GpuScreenRecorder, RecordCodec::Av1, Some(&info)), Some(tk("Your graphics card can't encode this with GPU Screen Recorder")));
        assert_eq!(codec_reason(Backend::GpuScreenRecorder, RecordCodec::Av1, None), None, "unknown hardware: offered");
    }

    /// An unsupported codec falls back to the backend's first one, and a
    /// WebM choice is judged against that codec (HEVC asked on a card
    /// without it records H.264, so WebM is not offered).
    #[test]
    fn codec_used_falls_back_like_the_plan() {
        let info = GsrInfo { codecs: vec!["h264".into(), "av1".into()], containers: vec![] };
        assert_eq!(codec_used(Backend::GpuScreenRecorder, RecordCodec::Hevc, Some(&info)), RecordCodec::H264);
        assert_eq!(codec_used(Backend::GpuScreenRecorder, RecordCodec::Av1, Some(&info)), RecordCodec::Av1);
        assert_eq!(codec_used(Backend::Builtin, RecordCodec::H264, None), RecordCodec::Av1);
        let used = codec_used(Backend::GpuScreenRecorder, RecordCodec::Hevc, Some(&info));
        assert!(container_reason(Backend::GpuScreenRecorder, RecordContainer::Webm, used, Some(&info)).is_some());
        use mandara_config::recorder::RecorderSettings;
        let s = RecorderSettings { codec: RecordCodec::Hevc, container: RecordContainer::Webm, ..RecorderSettings::default() };
        let p = super::super::plan::plan(&s, Backend::GpuScreenRecorder, Some(&info));
        assert_eq!((p.codec, p.container), (used, RecordContainer::Mp4), "the plan agrees");
    }

    /// A probe that ran and told nothing is "failed", not "checking"
    /// forever; one that read codecs narrows the table.
    #[test]
    fn probe_failure_is_told_apart_from_pending() {
        crate::test_process::isolated("recording::backend::tests::probe_fixture");
    }

    #[test]
    #[ignore = "run by the parent probe test in a private single-threaded process"]
    fn probe_fixture() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().unwrap();
        let dir = directory.path();
        let fake = |name: &str, body: &str| {
            let p = dir.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        let broken = fake("gsr-broken", "echo 'gsr error: no gpu' >&2; exit 1");
        assert_eq!(gsr_info_now(&broken), None);
        assert_eq!(gsr_probe(&broken), GsrProbe::Failed);
        let installed = Installed { gpu_screen_recorder: Some(broken), wf_recorder: None };
        let s = &statuses(&installed)[0];
        assert!(s.probe_failed && !s.probed, "{s:?}");
        let good = fake("gsr-good", &format!("printf '{}'", INFO.replace('\n', "\\n")));
        assert!(gsr_info_now(&good).is_some());
        assert!(matches!(gsr_probe(&good), GsrProbe::Read(_)));
        let installed = Installed { gpu_screen_recorder: Some(good), wf_recorder: None };
        let s = &statuses(&installed)[0];
        assert!(s.probed && !s.probe_failed);
        assert_eq!(s.caps.codecs, [RecordCodec::H264, RecordCodec::Hevc]);
    }

    #[test]
    fn capability_table() {
        let b = caps(Backend::Builtin, None);
        assert_eq!((b.codecs.as_slice(), b.containers.as_slice(), b.max_fps, b.audio, b.pause, b.bitrate), ([RecordCodec::Av1].as_slice(), [RecordContainer::Webm].as_slice(), 60, false, true, false));
        let g = caps(Backend::GpuScreenRecorder, None);
        assert!(g.audio && g.audio_both && g.pause && g.bitrate && g.max_fps >= 120);
        let w = caps(Backend::WfRecorder, None);
        assert!(w.audio && !w.audio_both && !w.pause && w.bitrate && w.max_fps >= 120);
    }

    #[test]
    fn reasons_for_unsupported_choices() {
        use Backend::*;
        assert!(codec_reason(Builtin, RecordCodec::H264, None).is_some());
        assert!(codec_reason(Builtin, RecordCodec::Av1, None).is_none());
        assert!(container_reason(Builtin, RecordContainer::Mp4, RecordCodec::Av1, None).is_some());
        assert!(container_reason(Builtin, RecordContainer::Webm, RecordCodec::Av1, None).is_none());
        for b in [GpuScreenRecorder, WfRecorder] {
            assert!(container_reason(b, RecordContainer::Webm, RecordCodec::H264, None).is_some(), "WebM needs AV1");
            assert!(container_reason(b, RecordContainer::Webm, RecordCodec::Av1, None).is_none());
            assert!(container_reason(b, RecordContainer::Mkv, RecordCodec::Hevc, None).is_none());
            assert!(fps_reason(b, 120).is_none());
            assert!(quality_reason(b, RecordQuality::Custom).is_none());
        }
        assert!(fps_reason(Builtin, 90).is_some() && fps_reason(Builtin, 60).is_none());
        assert!(audio_reason(Builtin, RecordAudio::Mic).is_some() && audio_reason(Builtin, RecordAudio::None).is_none());
        assert!(audio_reason(WfRecorder, RecordAudio::Both).is_some() && audio_reason(WfRecorder, RecordAudio::System).is_none());
        assert!(audio_reason(GpuScreenRecorder, RecordAudio::Both).is_none());
        assert!(quality_reason(Builtin, RecordQuality::Custom).is_some() && quality_reason(Builtin, RecordQuality::Low).is_none());
        assert!(pause_reason(WfRecorder).is_some() && pause_reason(GpuScreenRecorder).is_none() && pause_reason(Builtin).is_none());
    }

    #[test]
    fn which_finds_executables_only() {
        assert!(which("sh").is_some(), "a shell is on PATH");
        assert!(which("mandara-no-such-program").is_none());
    }
}
