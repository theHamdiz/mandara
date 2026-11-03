//! One recording on whichever backend the settings pick: [`start`] resolves
//! the backend, fits the settings to it ([`plan`](super::plan::plan)),
//! names the file and starts the encoder or the external recorder;
//! [`Session`] pauses, resumes and stops it the same way for all of them.
//! Starting and stopping block (process start, file finalization): call
//! them off the UI thread.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use mandara_config::recorder::RecorderSettings;
use mandara_i18n::{t, tf, tr};

use super::backend::{self, Backend, Installed};
use super::external::{self, External};
use super::plan::{self, Plan, Target};
use super::{RecordOptions, Recording};

/// An external recorder that exits within this long refused its options
/// (or could not capture): that is a failed start, not a recording.
const START_GRACE: Duration = Duration::from_millis(700);

/// Elapsed recording time, without the time spent paused.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Clock {
    started: Instant,
    paused_total: Duration,
    paused_at: Option<Instant>,
}

impl Clock {
    pub fn new(now: Instant) -> Clock {
        Clock { started: now, paused_total: Duration::ZERO, paused_at: None }
    }

    pub fn elapsed(&self, now: Instant) -> Duration {
        let paused_now = self.paused_at.map_or(Duration::ZERO, |at| now.saturating_duration_since(at));
        now.saturating_duration_since(self.started).saturating_sub(self.paused_total + paused_now)
    }

    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// Returns whether it changed.
    pub fn pause(&mut self, now: Instant) -> bool {
        if self.paused_at.is_some() {
            return false;
        }
        self.paused_at = Some(now);
        true
    }

    /// Returns whether it changed.
    pub fn resume(&mut self, now: Instant) -> bool {
        let Some(at) = self.paused_at.take() else { return false };
        self.paused_total += now.saturating_duration_since(at);
        true
    }
}

/// `m:ss`, or `h:mm:ss` from an hour on.
pub fn format_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

enum Engine {
    Builtin(Recording),
    External(External),
}

/// A running recording.
pub struct Session {
    engine: Engine,
    pub plan: Plan,
    pub clock: Clock,
    pub path: PathBuf,
    /// Closes when the recording ends by itself or after [`Session::stop`].
    pub ended: async_channel::Receiver<()>,
}

/// A saved recording.
#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub path: PathBuf,
    pub duration: Duration,
    pub bytes: u64,
    /// Frame size when known (the built-in encoder reports it).
    pub size: Option<(u32, u32)>,
}

/// Starts recording `target` with `settings`. `default_output` resolves
/// the default output device's name (wf-recorder records its monitor).
pub fn start(settings: &RecorderSettings, target: Target, now: chrono::DateTime<chrono::Local>, default_output: impl FnOnce() -> Option<String>) -> Result<Session, String> {
    let installed = Installed::detect();
    let backend = installed
        .resolve(settings.backend)
        .map_err(|reason| tf("{reason}. Install it, or choose another recorder in Studio → Plugins → Screenshot", &[("reason", &tr(reason))]))?;
    let info = match &installed.gpu_screen_recorder {
        Some(path) if backend == Backend::GpuScreenRecorder => backend::gsr_info(path).or_else(|| backend::gsr_info_now(path)),
        _ => None,
    };
    let plan = plan::plan(settings, backend, info.as_ref());
    for note in &plan.notes {
        tracing::info!(target: "mandara::recorder", "{}: {}", backend.label(), note);
    }
    let dir = plan::output_dir(&settings.directory);
    let path = plan::output_path(&dir, &settings.filename, plan.container, now);
    let started = Instant::now();
    let (engine, ended) = match backend {
        Backend::Builtin => {
            let (bw, bh) = plan.resolution.bounds().unwrap_or((u32::MAX, u32::MAX));
            let opts = RecordOptions { max_width: plan.max_width.min(bw), max_height: bh, quantizer: plan.quantizer(), ..RecordOptions::new(target.output().to_string(), target.local_rect(), plan.fps, plan.max_width, path.clone()) };
            let rec = super::start(opts)?;
            let ended = rec.ended.clone();
            (Engine::Builtin(rec), ended)
        }
        Backend::GpuScreenRecorder | Backend::WfRecorder => {
            let program = match backend {
                Backend::GpuScreenRecorder => installed.gpu_screen_recorder.clone(),
                _ => installed.wf_recorder.clone(),
            }
            .ok_or_else(|| tr(backend.missing_reason()))?;
            let args = match backend {
                Backend::GpuScreenRecorder => plan::gsr_args(&plan, &target, &path),
                _ => {
                    let default = (plan.audio == mandara_config::recorder::RecordAudio::System && plan.audio_output.is_empty()).then(default_output).flatten();
                    plan::wf_args(&plan, &target, &path, default.as_deref())
                }
            };
            let (tx, ended) = async_channel::bounded::<()>(1);
            let ext = external::spawn(&program, args, path.clone(), tx)?;
            if let Some(exit) = ext.wait_exit(START_GRACE) {
                let _ = std::fs::remove_file(&path);
                return Err(failure(backend, &exit));
            }
            (Engine::External(ext), ended)
        }
    };
    Ok(Session { engine, plan, clock: Clock::new(started), path, ended })
}

/// A recorder that ended unexpectedly, in words.
fn failure(backend: Backend, exit: &external::Exit) -> String {
    match exit.reason() {
        Some(reason) => tf("{recorder} stopped: {reason}", &[("recorder", &tr(backend.label())), ("reason", &reason)]),
        None => tf("{recorder} stopped unexpectedly", &[("recorder", &tr(backend.label()))]),
    }
}

impl Session {
    pub fn backend(&self) -> Backend {
        self.plan.backend
    }

    /// Why this recording can't pause (`tk` text), or `None`.
    pub fn pause_reason(&self) -> Option<&'static str> {
        backend::pause_reason(self.backend())
    }

    pub fn is_paused(&self) -> bool {
        self.clock.is_paused()
    }

    pub fn elapsed(&self) -> Duration {
        self.clock.elapsed(Instant::now())
    }

    /// Pauses or resumes. `Ok(false)`: it already was.
    pub fn set_paused(&mut self, paused: bool) -> Result<bool, String> {
        if let Some(reason) = self.pause_reason() {
            return Err(tr(reason).to_string());
        }
        if paused == self.clock.is_paused() {
            return Ok(false);
        }
        match &self.engine {
            Engine::Builtin(rec) => rec.set_paused(paused),
            Engine::External(ext) => ext.toggle_pause()?,
        }
        let now = Instant::now();
        if paused { self.clock.pause(now) } else { self.clock.resume(now) };
        Ok(true)
    }

    /// Stops and finalizes the file. Blocking.
    pub fn stop(self) -> Result<Saved, String> {
        let duration = self.clock.elapsed(Instant::now());
        match self.engine {
            Engine::Builtin(rec) => rec.stop().map(|r| Saved { path: r.path, duration: r.duration, bytes: r.bytes, size: Some((r.width, r.height)) }),
            Engine::External(ext) => {
                let backend = self.plan.backend;
                let exit = ext.stop();
                let bytes = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
                if bytes == 0 {
                    let _ = std::fs::remove_file(&self.path);
                    return Err(match exit.reason() {
                        Some(_) => failure(backend, &exit),
                        None => t("The recorder saved nothing").to_string(),
                    });
                }
                Ok(Saved { path: self.path, duration, bytes, size: None })
            }
        }
    }

    /// After [`Session::ended`] closed without a stop: what went wrong.
    /// Joins the encoder (blocking) for the built-in one.
    pub fn failed(self) -> String {
        let backend = self.plan.backend;
        match self.engine {
            Engine::Builtin(rec) => match rec.stop() {
                Err(e) => e,
                Ok(_) => t("The recording ended by itself").to_string(),
            },
            Engine::External(ext) => {
                let exit = ext.exit().unwrap_or(external::Exit { code: None, stderr: String::new() });
                failure(backend, &exit)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEP: Duration = Duration::from_secs(10);

    #[test]
    fn clock_leaves_out_paused_time() {
        let t0 = Instant::now();
        let mut c = Clock::new(t0);
        assert_eq!(c.elapsed(t0 + STEP), STEP);
        assert!(c.pause(t0 + STEP));
        assert!(!c.pause(t0 + STEP * 2), "already paused");
        assert!(c.is_paused());
        assert_eq!(c.elapsed(t0 + STEP * 3), STEP, "frozen while paused");
        assert!(c.resume(t0 + STEP * 3));
        assert!(!c.resume(t0 + STEP * 3));
        assert_eq!(c.elapsed(t0 + STEP * 4), STEP * 2);
        assert_eq!(c.elapsed(t0), Duration::ZERO, "never negative");
    }

    #[test]
    fn elapsed_formatting() {
        assert_eq!(format_elapsed(Duration::ZERO), "0:00");
        assert_eq!(format_elapsed(Duration::from_secs(65)), "1:05");
        assert_eq!(format_elapsed(Duration::from_secs(600)), "10:00");
        assert_eq!(format_elapsed(Duration::from_secs(3_725)), "1:02:05");
    }
}
