//! Screen recording state for the shell: one recording at a time, started
//! from the snipping tool, the screenshot panel or `mandara msg record …`,
//! stopped from the bar (red dot + timer), the panel or a command.

use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use mandara_i18n::{t, tf};
use mandara_services::recording::{self, RecordOptions, Recording, RecordingResult};

use crate::state::config;

/// The recording timer counts whole seconds.
const ELAPSED_TICK: Duration = Duration::from_secs(1);

pub struct Recorder {
    pub active: Option<Recording>,
    /// Set while starting or finishing (encoder flush), for the UI.
    pub busy: bool,
    ticker: Option<Task<()>>,
}

impl Recorder {
    pub fn elapsed(&self) -> Option<Duration> {
        self.active.as_ref().map(|r| r.started.elapsed())
    }

    /// `m:ss` of the running recording.
    pub fn clock(&self) -> Option<String> {
        self.elapsed().map(|d| format!("{}:{:02}", d.as_secs() / 60, d.as_secs() % 60))
    }

    /// Wakes observers each second while recording (the bar timer).
    fn tick(&mut self, cx: &mut Context<Self>) {
        self.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(ELAPSED_TICK).await;
                let alive = this
                    .update(cx, |r, cx| {
                        cx.notify();
                        r.active.is_some()
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        }));
    }
}

#[derive(Clone)]
pub struct GlobalRecorder(pub Entity<Recorder>);
impl Global for GlobalRecorder {}

pub fn init(cx: &mut App) -> Entity<Recorder> {
    let entity = cx.new(|_| Recorder { active: None, busy: false, ticker: None });
    cx.set_global(GlobalRecorder(entity.clone()));
    entity
}

pub fn recorder(cx: &App) -> Option<Entity<Recorder>> {
    cx.try_global::<GlobalRecorder>().map(|g| g.0.clone())
}

pub fn is_recording(cx: &App) -> bool {
    recorder(cx).is_some_and(|r| r.read(cx).active.is_some())
}

/// Where a new recording goes: the configured folder (`~` allowed), else
/// `~/Videos/Recordings`.
fn recording_path(folder: &str) -> std::path::PathBuf {
    let default = recording::default_path();
    if folder.trim().is_empty() {
        return default;
    }
    let dir = mandara_config::paths::expand_tilde(folder.trim());
    let stem = default.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Recording".into());
    mandara_services::screenshot::unique_path(&dir, &stem, "webm")
}

/// Starts recording `output` (empty = focused / first) and an optional
/// logical region of it, after `delay` (lets overlays fade out first).
pub fn start(output: String, region: Option<(i32, i32, i32, i32)>, delay: Duration, cx: &mut App) -> Result<(), String> {
    let Some(rec) = recorder(cx) else { return Err(t("recorder not initialised").into()) };
    if rec.read(cx).active.is_some() || rec.read(cx).busy {
        return Err(t("already recording").into());
    }
    let cfg = config(cx).read(cx).snapshot();
    let id = crate::plugins::screenshot::ID;
    let output = if output.is_empty() { crate::hub::hub(cx).read(cx).focused.as_ref().map(|c| c.monitor.clone()).unwrap_or_default() } else { output };
    let opts = RecordOptions {
        output,
        region,
        fps: cfg.plugins.setting(id, "record_fps", 30u32),
        max_width: cfg.plugins.setting(id, "record_width", 1600u32),
        path: recording_path(&cfg.plugins.setting(id, "record_directory", String::new())),
    };
    rec.update(cx, |r, cx| {
        r.busy = true;
        cx.notify();
    });
    cx.spawn(async move |cx| {
        cx.background_executor().timer(delay).await;
        let started = cx.background_executor().spawn(async move { recording::start(opts) }).await;
        cx.update(|cx| {
            let Some(rec) = recorder(cx) else { return };
            match started {
                Ok(recording) => rec.update(cx, |r, cx| {
                    r.busy = false;
                    r.active = Some(recording);
                    r.tick(cx);
                    cx.notify();
                }),
                Err(err) => {
                    rec.update(cx, |r, cx| {
                        r.busy = false;
                        cx.notify();
                    });
                    notify(Err(err), cx);
                }
            }
        });
    })
    .detach();
    Ok(())
}

/// Stops the recording; the result arrives on the returned channel (and
/// as a notification).
pub fn stop(cx: &mut App) -> Result<async_channel::Receiver<Result<String, String>>, String> {
    let Some(rec) = recorder(cx) else { return Err(t("recorder not initialised").into()) };
    let Some(recording) = rec.update(cx, |r, cx| {
        let taken = r.active.take();
        r.busy = taken.is_some();
        r.ticker = None;
        cx.notify();
        taken
    }) else {
        return Err(t("not recording").into());
    };
    let (tx, rx) = async_channel::bounded(1);
    cx.spawn(async move |cx| {
        let result = cx.background_executor().spawn(async move { recording.stop() }).await;
        let text = result.as_ref().map(describe).map_err(Clone::clone);
        cx.update(|cx| {
            if let Some(rec) = recorder(cx) {
                rec.update(cx, |r, cx| {
                    r.busy = false;
                    cx.notify();
                });
            }
            notify(result.map(|r| describe(&r)), cx);
        });
        let _ = tx.send(text).await;
    })
    .detach();
    Ok(rx)
}

pub fn describe(r: &RecordingResult) -> String {
    let secs = r.duration.as_secs();
    tf(
        "{duration} · {width}×{height} · {size} MB saved to {path}",
        &[
            ("duration", &format!("{}:{:02}", secs / 60, secs % 60)),
            ("width", &r.width),
            ("height", &r.height),
            ("size", &format!("{:.1}", r.bytes as f64 / 1_048_576.0)),
            ("path", &r.path.display()),
        ],
    )
}

fn notify(result: Result<String, String>, cx: &mut App) {
    let (summary, body) = match result {
        Ok(text) => (t("Recording saved").to_string(), text),
        Err(err) => (t("Recording failed").to_string(), err),
    };
    crate::plugins::notifications::send(cx, mandara_services::NotificationCommand::Post { app: t("Screen recorder").into(), icon: "camera".into(), summary, body });
}

/// For callers that want the start time too.
pub fn started(cx: &App) -> Option<Instant> {
    recorder(cx).and_then(|r| r.read(cx).active.as_ref().map(|a| a.started))
}
