//! Screen recording state for the shell: one recording at a time on the
//! backend the settings pick (GPU Screen Recorder, wf-recorder or the
//! built-in AV1 encoder, `mandara_services::recording`). Started from the
//! screenshot panel, the snipping tool, `mandara msg record …` or a
//! shortcut, optionally after a countdown; paused, resumed and stopped from
//! the bar (red dot + timer), the panel or a command.
//!
//! The [`Recorder`] entity changes only when the state does (start, pause,
//! stop); the one-second [`RecorderClock`] ticks only while a countdown
//! runs or a recording is running unpaused, and only the recorder's own
//! bar chip and panel listen to it.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use mandara_config::recorder::{RecordSource, RecorderSettings};
use mandara_i18n::{t, tf, tr};
use mandara_services::AudioCommand;
use mandara_services::recording::history::{self, Recent};
use mandara_services::recording::plan::Target;
use mandara_services::recording::session::{self, Saved, Session};

use crate::state::config;

/// The recording timer counts whole seconds.
const ELAPSED_TICK: Duration = Duration::from_secs(1);
/// After the snipping overlay closes, before a countdown may start (the
/// snipping tool's countdown pill is reused and must be free again).
const AFTER_SNIP: Duration = Duration::from_millis(450);

/// Wakes the recorder's clock displays once a second while it runs.
pub struct RecorderClock;

/// A countdown before a recording.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Countdown {
    pub until: Instant,
    generation: u64,
}

pub struct Recorder {
    pub active: Option<Session>,
    pub counting: Option<Countdown>,
    /// Set while starting or finishing (the file being written).
    pub busy: bool,
    /// The microphone was muted from the recorder (unmuted again when the
    /// recording ends).
    pub mic_muted: bool,
    /// Recent recordings, read in the background when first shown.
    recent: Option<Vec<Recent>>,
    loading_recent: bool,
    generation: u64,
    ticker: Option<Task<()>>,
    pub clock: Entity<RecorderClock>,
}

impl Recorder {
    pub fn elapsed(&self) -> Option<Duration> {
        self.active.as_ref().map(Session::elapsed)
    }

    /// `m:ss` of the running recording.
    pub fn clock(&self) -> Option<String> {
        self.elapsed().map(session::format_elapsed)
    }

    pub fn paused(&self) -> bool {
        self.active.as_ref().is_some_and(Session::is_paused)
    }

    /// Whole seconds left on the countdown.
    pub fn countdown_left(&self) -> Option<u64> {
        self.counting.map(|c| c.until.saturating_duration_since(Instant::now()).as_secs_f64().ceil() as u64)
    }

    /// Ticks [`RecorderClock`] once a second while a countdown runs or a
    /// recording runs unpaused; stops by itself otherwise.
    fn tick(&mut self, cx: &mut Context<Self>) {
        let running = |r: &Recorder| r.counting.is_some() || r.active.as_ref().is_some_and(|s| !s.is_paused());
        if !running(self) {
            self.ticker = None;
            return;
        }
        let clock = self.clock.clone();
        self.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(ELAPSED_TICK).await;
                let alive = this.update(cx, |r, cx| {
                    clock.update(cx, |_, cx| cx.notify());
                    running(r)
                });
                if !alive.unwrap_or(false) {
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
    let clock = cx.new(|_| RecorderClock);
    let entity = cx.new(|_| Recorder { active: None, counting: None, busy: false, mic_muted: false, recent: None, loading_recent: false, generation: 0, ticker: None, clock });
    cx.set_global(GlobalRecorder(entity.clone()));
    entity
}

pub fn recorder(cx: &App) -> Option<Entity<Recorder>> {
    cx.try_global::<GlobalRecorder>().map(|g| g.0.clone())
}

pub fn is_recording(cx: &App) -> bool {
    recorder(cx).is_some_and(|r| r.read(cx).active.is_some())
}

/// Recording, counting down, starting or finishing.
pub fn is_engaged(cx: &App) -> bool {
    recorder(cx).is_some_and(|r| {
        let r = r.read(cx);
        r.active.is_some() || r.counting.is_some() || r.busy
    })
}

pub fn settings(cx: &App) -> RecorderSettings {
    RecorderSettings::from_plugins(&config(cx).read(cx).get().plugins)
}

/// What to record when starting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The setting's default source.
    Default,
    /// A display by name; `None` = the chosen display, else the one with
    /// the focused window.
    Screen(Option<String>),
    /// Pick an area with the snipping tool first.
    Region,
    /// The focused window's area.
    Window,
}

/// The output with the focused window, else the first one.
fn focused_output(cx: &App) -> String {
    crate::hub::hub(cx).read(cx).focused.as_ref().map(|c| c.monitor.clone()).filter(|m| !m.is_empty()).or_else(|| crate::outputs::geometries(cx).first().map(|g| g.name.clone())).unwrap_or_default()
}

/// The layout position of output `name`.
fn origin(name: &str, cx: &App) -> (i32, i32) {
    crate::outputs::geometries(cx).iter().find(|g| g.name == name).map_or((0, 0), |g| (g.x, g.y))
}

/// Where a source records, without starting anything.
fn target(source: &Source, s: &RecorderSettings, cx: &App) -> Result<Option<Target>, String> {
    let source = match source {
        Source::Default => match s.source {
            RecordSource::Screen => Source::Screen(None),
            RecordSource::Region => Source::Region,
            RecordSource::Window => Source::Window,
        },
        other => other.clone(),
    };
    match source {
        Source::Region => Ok(None),
        Source::Window => {
            let hub = crate::hub::hub(cx).read(cx);
            let c = hub.focused.as_ref().ok_or_else(|| t("No window has focus"))?;
            let (x, y, w, h) = c.geometry.ok_or_else(|| t("the compositor did not report a focused window geometry"))?;
            let output = if c.monitor.is_empty() { focused_output(cx) } else { c.monitor.clone() };
            let o = origin(&output, cx);
            Ok(Some(Target::Region { output, origin: o, rect: (x - o.0, y - o.1, w, h) }))
        }
        Source::Screen(name) => {
            let chosen = name.or_else(|| Some(s.monitor.trim().to_string()).filter(|m| !m.is_empty()));
            let name = match chosen {
                Some(n) => {
                    let outputs = crate::outputs::geometries(cx);
                    if !outputs.is_empty() && !outputs.iter().any(|g| g.name == n) {
                        return Err(tf("There is no display named {name}", &[("name", &n)]));
                    }
                    n
                }
                None => focused_output(cx),
            };
            Ok(Some(Target::Output { name }))
        }
        Source::Default => unreachable!("resolved above"),
    }
}

/// Starts recording `source`: after the countdown setting (if any), else
/// after `lead_in` (lets a panel or overlay get out of the frame).
pub fn start(source: Source, lead_in: Duration, cx: &mut App) -> Result<(), String> {
    let Some(rec) = recorder(cx) else { return Err(t("recorder not initialised").into()) };
    if is_engaged(cx) {
        return Err(t("already recording").into());
    }
    let s = settings(cx);
    match target(&source, &s, cx)? {
        None => {
            crate::snip::open_for_recording(cx);
            Ok(())
        }
        Some(target) => {
            begin(rec, target, s.countdown_secs(), lead_in, cx);
            Ok(())
        }
    }
}

/// The snipping tool picked an area (or a whole output) to record.
pub fn record_area(output: String, region: Option<(i32, i32, i32, i32)>, cx: &mut App) -> Result<(), String> {
    let Some(rec) = recorder(cx) else { return Err(t("recorder not initialised").into()) };
    if is_engaged(cx) {
        return Err(t("already recording").into());
    }
    let output = if output.is_empty() { focused_output(cx) } else { output };
    let target = match region {
        Some(rect) => Target::Region { origin: origin(&output, cx), output, rect },
        None => Target::Output { name: output },
    };
    let secs = settings(cx).countdown_secs();
    if secs == 0 {
        begin(rec, target, 0, AFTER_SNIP, cx);
    } else {
        // The overlay is closing; its countdown pill is free once it has.
        cx.spawn(async move |cx| {
            cx.background_executor().timer(AFTER_SNIP).await;
            cx.update(|cx| begin(rec, target, secs, Duration::ZERO, cx));
        })
        .detach();
    }
    Ok(())
}

/// Counts down `secs` (the snipping tool's countdown pill, Esc cancels),
/// then launches.
fn begin(rec: Entity<Recorder>, target: Target, secs: u64, lead_in: Duration, cx: &mut App) {
    if secs == 0 {
        launch(rec, target, lead_in, cx);
        return;
    }
    let generation = rec.update(cx, |r, cx| {
        r.generation += 1;
        r.counting = Some(Countdown { until: Instant::now() + Duration::from_secs(secs), generation: r.generation });
        r.tick(cx);
        cx.notify();
        r.generation
    });
    let (cancel_tx, cancelled) = async_channel::bounded::<Result<String, String>>(1);
    let rec2 = rec.clone();
    let job: crate::snip::Job = Box::new(move |cx| {
        let still = rec2.update(cx, |r, cx| {
            let ours = r.counting.is_some_and(|c| c.generation == generation);
            if ours {
                r.counting = None;
                cx.notify();
            }
            ours
        });
        if still {
            launch(rec2, target, Duration::ZERO, cx);
        }
    });
    crate::snip::begin(crate::snip::SnipMode::Screen, false, crate::snip::Preselect::None, secs, Some(job), Some(cancel_tx), cx);
    // Esc on the countdown (or another capture taking over) cancels it.
    cx.spawn(async move |cx| {
        if let Ok(Err(_)) = cancelled.recv().await {
            cx.update(|cx| {
                rec.update(cx, |r, cx| {
                    if r.counting.is_some_and(|c| c.generation == generation) {
                        r.counting = None;
                        r.tick(cx);
                        cx.notify();
                    }
                })
            });
        }
    })
    .detach();
}

/// The default output device's name (wf-recorder records its monitor).
fn default_output_name() -> Option<String> {
    mandara_services::audio::mixer().ok()?.outputs.into_iter().find(|d| d.is_default).map(|d| d.name)
}

fn launch(rec: Entity<Recorder>, target: Target, delay: Duration, cx: &mut App) {
    let s = settings(cx);
    rec.update(cx, |r, cx| {
        r.busy = true;
        cx.notify();
    });
    cx.spawn(async move |cx| {
        if !delay.is_zero() {
            cx.background_executor().timer(delay).await;
        }
        let started = cx.background_executor().spawn(async move { mandara_plugins::guard("screen recording", || session::start(&s, target, chrono::Local::now(), default_output_name)).unwrap_or_else(|| Err(t("internal error (logged)").into())) }).await;
        cx.update(|cx| match started {
            Ok(session) => {
                let ended = session.ended.clone();
                let path = session.path.clone();
                rec.update(cx, |r, cx| {
                    r.busy = false;
                    r.active = Some(session);
                    r.tick(cx);
                    cx.notify();
                });
                watch_end(rec.clone(), ended, path, cx);
            }
            Err(err) => {
                rec.update(cx, |r, cx| {
                    r.busy = false;
                    cx.notify();
                });
                notify(Err(err), cx);
            }
        });
    })
    .detach();
}

/// A recording that ends without being stopped (the recorder crashed, the
/// output went away, the disk filled up) is reported.
fn watch_end(rec: Entity<Recorder>, ended: async_channel::Receiver<()>, path: PathBuf, cx: &mut App) {
    cx.spawn(async move |cx| {
        let _ = ended.recv().await;
        let lost = cx.update(|cx| {
            rec.update(cx, |r, cx| {
                if r.active.as_ref().is_some_and(|s| s.path == path) {
                    let session = r.active.take();
                    r.tick(cx);
                    cx.notify();
                    session
                } else {
                    None
                }
            })
        });
        let Some(session) = lost else { return };
        let message = cx.background_executor().spawn(async move { session.failed() }).await;
        cx.update(|cx| {
            restore_mic(&rec, cx);
            notify(Err(message), cx);
        });
    })
    .detach();
}

/// Unmutes the microphone if the recorder muted it.
fn restore_mic(rec: &Entity<Recorder>, cx: &mut App) {
    if rec.update(cx, |r, _| std::mem::take(&mut r.mic_muted)) {
        let input = settings(cx).audio_input;
        send_audio(AudioCommand::SetInputMute(Some(input).filter(|i| !i.is_empty()), false), cx);
    }
}

fn send_audio(command: AudioCommand, cx: &mut App) {
    if let Some(service) = crate::hub::hub(cx).read(cx).audio_service.clone() {
        service.send(command);
    }
}

/// Stops the recording (or cancels the countdown); the result arrives on
/// the returned channel (and as a notification).
pub fn stop(cx: &mut App) -> Result<async_channel::Receiver<Result<String, String>>, String> {
    let Some(rec) = recorder(cx) else { return Err(t("recorder not initialised").into()) };
    let (tx, rx) = async_channel::bounded(1);
    if rec.read(cx).counting.is_some() {
        rec.update(cx, |r, cx| {
            r.counting = None;
            r.tick(cx);
            cx.notify();
        });
        crate::snip::close(cx);
        let _ = tx.try_send(Ok(t("countdown cancelled").to_string()));
        return Ok(rx);
    }
    let Some(session) = rec.update(cx, |r, cx| {
        let taken = r.active.take();
        r.busy = taken.is_some();
        r.tick(cx);
        cx.notify();
        taken
    }) else {
        return Err(t("not recording").into());
    };
    restore_mic(&rec, cx);
    let open_folder = settings(cx).open_folder;
    cx.spawn(async move |cx| {
        let result = cx
            .background_executor()
            .spawn(async move {
                let saved = session.stop()?;
                let at = chrono::Local::now().timestamp();
                if let Err(e) = history::add(&history::file(), Recent { path: saved.path.clone(), secs: saved.duration.as_secs_f64(), bytes: saved.bytes, at }) {
                    tracing::warn!("recordings history: {e}");
                }
                Ok::<Saved, String>(saved)
            })
            .await;
        let text = result.as_ref().map(describe).map_err(Clone::clone);
        cx.update(|cx| {
            rec.update(cx, |r, cx| {
                r.busy = false;
                r.recent = None;
                cx.notify();
            });
            if let (true, Ok(saved)) = (open_folder, &result) {
                reveal(&saved.path);
            }
            notify(text.clone(), cx);
        });
        let _ = tx.send(text).await;
    })
    .detach();
    Ok(rx)
}

/// Pauses (`Some(true)`), resumes (`Some(false)`) or toggles (`None`).
pub fn set_paused(paused: Option<bool>, cx: &mut App) -> Result<bool, String> {
    let Some(rec) = recorder(cx) else { return Err(t("recorder not initialised").into()) };
    rec.update(cx, |r, cx| {
        let Some(s) = r.active.as_mut() else { return Err(t("not recording").to_string()) };
        let want = paused.unwrap_or(!s.is_paused());
        s.set_paused(want)?;
        r.tick(cx);
        cx.notify();
        Ok(want)
    })
}

/// Mutes or unmutes the recorded microphone (`None` toggles).
pub fn set_mic_muted(muted: Option<bool>, cx: &mut App) -> Result<bool, String> {
    let Some(rec) = recorder(cx) else { return Err(t("recorder not initialised").into()) };
    let records_mic = rec.read(cx).active.as_ref().is_some_and(|s| s.plan.audio.mic());
    if !records_mic {
        return Err(t("This recording has no microphone").into());
    }
    let want = muted.unwrap_or(!rec.read(cx).mic_muted);
    rec.update(cx, |r, cx| {
        r.mic_muted = want;
        cx.notify();
    });
    let input = settings(cx).audio_input;
    send_audio(AudioCommand::SetInputMute(Some(input).filter(|i| !i.is_empty()), want), cx);
    Ok(want)
}

/// Stops a recording (or countdown) if one runs, else starts the default
/// source.
pub fn toggle(cx: &mut App) -> Result<Option<async_channel::Receiver<Result<String, String>>>, String> {
    if recorder(cx).is_some_and(|r| r.read(cx).active.is_some() || r.read(cx).counting.is_some()) {
        return stop(cx).map(Some);
    }
    start(Source::Default, Duration::ZERO, cx).map(|_| None)
}

/// Recent recordings (newest first); read in the background the first
/// time, `None` until then.
pub fn recent(cx: &mut App) -> Option<Vec<Recent>> {
    let rec = recorder(cx)?;
    if let Some(list) = rec.read(cx).recent.clone() {
        return Some(list);
    }
    if !rec.read(cx).loading_recent {
        rec.update(cx, |r, _| r.loading_recent = true);
        cx.spawn(async move |cx| {
            let list = cx.background_executor().spawn(async { history::list(&history::file()) }).await;
            cx.update(|cx| {
                rec.update(cx, |r, cx| {
                    r.recent = Some(list);
                    r.loading_recent = false;
                    cx.notify();
                })
            });
        })
        .detach();
    }
    None
}

/// Moves a recording to the Trash and forgets it.
pub fn trash(path: PathBuf, cx: &mut App) {
    let Some(rec) = recorder(cx) else { return };
    cx.spawn(async move |cx| {
        let p = path.clone();
        let result = cx
            .background_executor()
            .spawn(async move {
                crate::plugins::dock::stacks::move_to_trash(std::slice::from_ref(&p))?;
                history::forget(&history::file(), &p)
            })
            .await;
        cx.update(|cx| {
            rec.update(cx, |r, cx| {
                r.recent = None;
                cx.notify();
            });
            if let Err(err) = result {
                notify(Err(tf("Could not move {path} to the Trash: {error}", &[("path", &path.display()), ("error", &err)])), cx);
            }
        });
    })
    .detach();
}

/// Opens a recording with its default app.
pub fn open(path: &std::path::Path) {
    crate::plugins::dock::stacks::open_path(path);
}

/// Opens a recording's folder.
pub fn reveal(path: &std::path::Path) {
    if let Some(dir) = path.parent() {
        crate::plugins::dock::stacks::open_path(dir);
    }
}

/// `1:05 · 12.3 MB saved to …`.
pub fn describe(r: &Saved) -> String {
    let duration = session::format_elapsed(r.duration);
    let size = format!("{:.1}", r.bytes as f64 / 1_048_576.0);
    match r.size {
        Some((width, height)) => tf("{duration} · {width}×{height} · {size} MB saved to {path}", &[("duration", &duration), ("width", &width), ("height", &height), ("size", &size), ("path", &r.path.display())]),
        None => tf("{duration} · {size} MB saved to {path}", &[("duration", &duration), ("size", &size), ("path", &r.path.display())]),
    }
}

fn notify(result: Result<String, String>, cx: &mut App) {
    let (summary, body) = match result {
        Ok(text) => (t("Recording saved").to_string(), text),
        Err(err) => (t("Recording failed").to_string(), err),
    };
    crate::plugins::notifications::send(cx, mandara_services::NotificationCommand::Post { app: t("Screen recorder").into(), icon: "camera".into(), summary, body });
}

/// The status of the recorder as JSON (`record status`).
pub fn status(cx: &App) -> serde_json::Value {
    let Some(r) = recorder(cx) else { return serde_json::json!({ "recording": false }) };
    let r = r.read(cx);
    let s = r.active.as_ref();
    serde_json::json!({
        "recording": s.is_some(),
        "paused": r.paused(),
        "countdown": r.countdown_left(),
        "busy": r.busy,
        "elapsed": r.clock(),
        "path": s.map(|s| s.path.display().to_string()),
        "backend": s.map(|s| s.backend()),
        "settings": s.map(|s| s.plan.summary()),
        "mic_muted": r.mic_muted,
        "notes": s.map(|s| s.plan.notes.iter().map(|n| tr(n)).collect::<Vec<_>>()),
    })
}
