//! The job runner: one generation at a time on its own thread, with
//! progress and cancel shared through atomics so the UI only reads them.
//! Stills render and encode in one engine call (no partial progress, and
//! a cancel lands when the file is written: it is then removed); video
//! loops report every frame and stop at the next one.
//!
//! No GUI types: the plugin's entity wraps a [`JobRunner`] and turns
//! [`Finished`] results into wallpapers, the tests drive it directly.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use mandara_i18n::t;
use mandara_wallgen::{Generated, OutputOptions, Recipe, VideoOptions};
use serde_json::{Value, json};

/// Progress is shared as permille (atomics hold integers).
const PERMILLE: f32 = 1000.0;
/// Seconds are reported with this many decimals.
const SECONDS_ROUNDING: f64 = 100.0;

/// What to make.
#[derive(Clone, Debug, PartialEq)]
pub enum Output {
    Still(OutputOptions),
    /// A seamless loop written to the folder.
    Video { options: VideoOptions, dir: PathBuf },
}

/// Who asked: decides nothing in the runner, reported with the result.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Studio, the CLI or the launcher.
    Manual,
    /// The wallpaper picker's Generate button.
    Picker,
    Schedule,
}

impl Origin {
    pub fn id(self) -> &'static str {
        match self {
            Origin::Manual => "manual",
            Origin::Picker => "picker",
            Origin::Schedule => "schedule",
        }
    }
}

/// One generation.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub recipe: Recipe,
    pub output: Output,
    /// Set the result as the wallpaper.
    pub apply: bool,
    /// Prune the folder to the newest N generated files afterwards.
    pub keep: Option<usize>,
    pub origin: Origin,
}

impl Request {
    pub fn is_video(&self) -> bool {
        matches!(self.output, Output::Video { .. })
    }

    /// The output folder.
    pub fn dir(&self) -> &std::path::Path {
        match &self.output {
            Output::Still(o) => &o.dir,
            Output::Video { dir, .. } => dir,
        }
    }
}

/// A written wallpaper.
#[derive(Clone, Debug, PartialEq)]
pub struct Done {
    pub file: Generated,
    pub recipe: Recipe,
    pub video: bool,
    /// Wall-clock time the job took.
    pub seconds: f64,
    /// Files the keep-N limit removed.
    pub pruned: Vec<PathBuf>,
}

impl Done {
    pub fn json(&self) -> Value {
        json!({
            "path": self.file.path,
            "recipe_path": self.file.recipe_path,
            "kind": if self.video { "video" } else { "image" },
            "width": self.file.width,
            "height": self.file.height,
            "bytes": self.file.bytes,
            "seconds": (self.seconds * SECONDS_ROUNDING).round() / SECONDS_ROUNDING,
            "style": self.recipe.style.id(),
            "seed": format!("{:016x}", self.recipe.seed),
            "pruned": self.pruned,
        })
    }
}

/// Runs `req` on the calling thread.
pub fn run(req: &Request, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<Done, String> {
    let started = Instant::now();
    let file = match &req.output {
        Output::Still(options) => {
            progress(0.0);
            let file = mandara_wallgen::save(&req.recipe, options)?;
            if cancel.load(Ordering::Relaxed) {
                let _ = std::fs::remove_file(&file.path);
                let _ = std::fs::remove_file(&file.recipe_path);
                return Err(t("Cancelled").into());
            }
            progress(1.0);
            file
        }
        Output::Video { options, dir } => mandara_wallgen::render_video(&req.recipe, options, dir, progress, cancel)?,
    };
    let pruned = match req.keep {
        // The new file is the newest, so it always survives.
        Some(keep) if keep > 0 => mandara_wallgen::prune(req.dir(), keep).unwrap_or_else(|e| {
            tracing::warn!("wallpaper generator: pruning: {e}");
            Vec::new()
        }),
        _ => Vec::new(),
    };
    Ok(Done { file, recipe: req.recipe.clone(), video: req.is_video(), seconds: started.elapsed().as_secs_f64(), pruned })
}

/// A job's result.
#[derive(Clone, Debug)]
pub struct Finished {
    pub id: u64,
    pub request: Request,
    pub result: Result<Done, String>,
}

/// The job in flight.
#[derive(Clone)]
struct Running {
    id: u64,
    request: Request,
    started: Instant,
    started_at: SystemTime,
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct State {
    running: Option<Running>,
    last: Option<Finished>,
    next_id: u64,
}

/// A snapshot of the runner for status displays.
#[derive(Clone, Debug)]
pub struct Status {
    /// `(id, request, progress 0–1, seconds running, cancel requested)`.
    pub running: Option<(u64, Request, f32, f64, bool)>,
    pub started_at: Option<SystemTime>,
    pub last: Option<Finished>,
}

impl Status {
    pub fn json(&self) -> Value {
        let running = self.running.as_ref().map(|(id, req, progress, secs, cancelling)| {
            json!({
                "id": id,
                "kind": if req.is_video() { "video" } else { "image" },
                "origin": req.origin.id(),
                // Stills render in one step: no meaningful fraction.
                "progress": if req.is_video() { Value::from((f64::from(*progress) * f64::from(PERMILLE)).round() / f64::from(PERMILLE)) } else { Value::Null },
                "seconds": (secs * SECONDS_ROUNDING).round() / SECONDS_ROUNDING,
                "cancelling": cancelling,
                "style": req.recipe.style.id(),
            })
        });
        let last = self.last.as_ref().map(|f| match &f.result {
            Ok(done) => {
                let mut v = done.json();
                v["id"] = json!(f.id);
                v["ok"] = json!(true);
                v
            }
            Err(e) => json!({ "id": f.id, "ok": false, "error": e }),
        });
        json!({ "running": running, "last": last })
    }
}

/// Runs one job at a time on a background thread.
#[derive(Clone, Default)]
pub struct JobRunner {
    state: Arc<Mutex<State>>,
}

impl JobRunner {
    /// Starts `request` unless a job is running (the error says so).
    /// `on_done` runs on the job thread when it ends.
    pub fn start(&self, request: Request, on_done: impl FnOnce(Finished) + Send + 'static) -> Result<u64, String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        if state.running.is_some() {
            return Err(t("A wallpaper is already being generated").into());
        }
        state.next_id += 1;
        let id = state.next_id;
        let running = Running { id, request: request.clone(), started: Instant::now(), started_at: SystemTime::now(), progress: Arc::default(), cancel: Arc::default() };
        let (progress, cancel) = (running.progress.clone(), running.cancel.clone());
        state.running = Some(running);
        drop(state);
        let shared = self.state.clone();
        let spawned = std::thread::Builder::new().name("mandara-wallgen".into()).spawn(move || {
            let report = |f: f32| progress.store((f.clamp(0.0, 1.0) * PERMILLE) as u32, Ordering::Relaxed);
            let result = run(&request, &report, &cancel);
            let finished = Finished { id, request, result };
            if let Ok(mut s) = shared.lock() {
                s.running = None;
                s.last = Some(finished.clone());
            }
            on_done(finished);
        });
        if let Err(e) = spawned {
            if let Ok(mut s) = self.state.lock() {
                s.running = None;
            }
            return Err(e.to_string());
        }
        Ok(id)
    }

    /// Asks the running job to stop; `false` when nothing runs.
    pub fn cancel(&self) -> bool {
        let state = self.state.lock();
        match state.ok().and_then(|s| s.running.clone()) {
            Some(r) => {
                r.cancel.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    pub fn busy(&self) -> bool {
        self.state.lock().is_ok_and(|s| s.running.is_some())
    }

    pub fn status(&self) -> Status {
        let Ok(s) = self.state.lock() else { return Status { running: None, started_at: None, last: None } };
        Status {
            running: s.running.as_ref().map(|r| (r.id, r.request.clone(), r.progress.load(Ordering::Relaxed) as f32 / PERMILLE, r.started.elapsed().as_secs_f64(), r.cancel.load(Ordering::Relaxed))),
            started_at: s.running.as_ref().map(|r| r.started_at),
            last: s.last.clone(),
        }
    }
}
