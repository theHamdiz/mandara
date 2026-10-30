//! App installer plugin (`app-installer`): install apps by dropping files
//! on Mandara or picking them — AppImages, Flatpak bundles / refs,
//! portable archives and native packages — through
//! [`mandara_services::app_install`], with a chromatic-glass confirmation
//! card and a progress pill on the `mandara-installer` layer surface
//! ([`crate::installer_overlay`]).
//!
//! # Public API (for the dock, Studio, the launcher)
//!
//! * [`accepts`]`(path) -> bool` — cheap magic-byte sniff (reads one
//!   page; nothing decompressed or run). Use it to decide whether a drag
//!   gets the "Drop to install" glow.
//! * [`install`]`(paths, origin, cx)` — queue files for installation
//!   ([`InstallOrigin::Dock`] / `Studio` / `Command` / `Launcher`). Each
//!   file becomes a [`Job`]: inspected in the background, then confirmed
//!   on the card (or installed straight away when the plugin's settings
//!   allow), installed, integrated. Does nothing but post an error when
//!   the plugin is switched off.
//! * [`queue`]`(cx) -> Entity<InstallQueue>` — observe it for progress:
//!   [`InstallQueue::jobs`] lists [`Job`]s with `id`, `app_id` /
//!   `desktop_id` (known once inspected / installed), `name`, `icon` (an
//!   image path), [`Job::progress`] (0..1), `phase`
//!   ([`Phase`]: inspect → confirm → install → integrate → done / failed
//!   / cancelled) and `result`. Finished jobs stay listed for
//!   [`JOB_LINGER`] so a dock can play its own finish animation.
//! * [`crate::installer_overlay::drop_zone`] — wraps any element so an
//!   installable file dragged over it glows "Drop to install" and dropping
//!   installs it (respects reduce motion).
//!
//! Commands: `apps install <path…> [--yes] [--no-path-shim] [--dry-run]`,
//! `apps jobs`, `apps jobs cancel <id>`; `apps installed --by-mandara`
//! lists what this plugin installed. Settings
//! (`[plugins.settings.app-installer]`, Studio → Plugins → App installer):
//! `install_dir`, `path_shims`, `confirm_rootless`, `flatpak_scope`,
//! `allow_native`, `after_install`, `notify`.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Entity, Global};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PluginManifest, PluginRegistry};
use mandara_services::app_install::{self as engine, Choices, InstallEnv, InstallSettings, Outcome, Phase, Plan, Progress};
use mandara_services::app_uninstall::SystemRunner;
use serde_json::{Value, json};

pub use mandara_services::app_install::Phase as JobPhase;

use crate::state::config;

pub const ID: &str = "app-installer";

/// Finished jobs stay in the queue this long (dock rings, the pill).
pub const JOB_LINGER: Duration = Duration::from_secs(8);
/// Failed jobs stay until closed, or this long.
const FAILED_LINGER: Duration = Duration::from_secs(60);
/// Preview files older than this are stale (a crash or a hidden job left
/// them); the first install of a session sweeps them.
const PREVIEW_MAX_AGE: Duration = Duration::from_secs(60 * 60);
/// The stale-preview sweep ran this session.
static PREVIEWS_PRUNED: AtomicBool = AtomicBool::new(false);
/// Progress updates smaller than this aren't forwarded to the UI.
const PROGRESS_STEP: f32 = 0.004;
/// `apps install` flags.
const FLAG_YES: &str = "--yes";
const FLAG_NO_SHIM: &str = "--no-path-shim";
const FLAG_DRY_RUN: &str = "--dry-run";
const INSTALL_FLAGS: &[&str] = &[FLAG_YES, FLAG_NO_SHIM, FLAG_DRY_RUN];

/// Where an install was started.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallOrigin {
    Dock,
    Studio,
    Command,
    Launcher,
}

/// How a job runs (from `apps install` flags).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct JobOptions {
    /// Skip the confirmation (blocked plans still fail).
    pub yes: bool,
    pub no_path_shim: bool,
    pub dry_run: bool,
}

/// One file being installed.
#[derive(Clone)]
pub struct Job {
    pub id: u64,
    pub origin: InstallOrigin,
    pub path: PathBuf,
    /// The file name until the app's own name is known.
    pub name: String,
    pub app_id: Option<String>,
    /// Desktop id (without `.desktop`) once installed.
    pub desktop_id: Option<String>,
    /// The app's icon (an image file).
    pub icon: Option<PathBuf>,
    /// `AppImage`, `Flatpak`, `Portable`, `Native (Pacman)`.
    pub badge: String,
    pub phase: Phase,
    /// 0..1 (`None`: no total known yet — draw an indeterminate ring).
    pub fraction: Option<f32>,
    pub plan: Option<Rc<Plan>>,
    pub choices: Choices,
    pub result: Option<Result<Outcome, String>>,
    pub options: JobOptions,
    /// The pill was hidden: completion is announced by a notification.
    pub background: bool,
    /// Files queued together share a batch.
    pub batch: u64,
    /// Waiting on the queue sheet (several files dropped at once).
    pub batched: bool,
    /// "Install all" before this one was inspected: install once planned.
    pub install_when_ready: bool,
    /// Pin the installed app to the dock (on by default from the dock).
    pub keep_in_dock: bool,
    /// When installing started (the progress pill's elapsed time).
    pub started_at: Option<Instant>,
    pub(crate) cancel: Arc<AtomicBool>,
    pub(crate) finished_at: Option<Instant>,
}

impl Job {
    /// 0..1 progress for rings (indeterminate counts as 0).
    pub fn progress(&self) -> f32 {
        match self.phase {
            Phase::Done => 1.0,
            _ => self.fraction.unwrap_or(0.0),
        }
    }

    pub fn finished(&self) -> bool {
        self.phase.finished()
    }

    /// Time spent installing so far (or in all, once finished).
    pub fn elapsed(&self) -> Duration {
        match (self.started_at, self.finished_at) {
            (Some(s), Some(f)) => f.saturating_duration_since(s),
            (Some(s), None) => s.elapsed(),
            _ => Duration::ZERO,
        }
    }

    /// Whether a decision or inspection is still ahead of it.
    fn pending(&self) -> bool {
        matches!(self.phase, Phase::Inspect | Phase::Confirm)
    }

    fn json(&self) -> Value {
        json!({
            "id": self.id,
            "origin": self.origin,
            "file": self.path,
            "name": self.name,
            "app_id": self.app_id,
            "desktop_id": self.desktop_id,
            "format": self.badge,
            "phase": self.phase,
            "progress": self.progress(),
            "error": self.result.as_ref().and_then(|r| r.as_ref().err()),
            "outcome": self.result.as_ref().and_then(|r| r.as_ref().ok()),
            "plan": self.plan.as_deref().map(plan_json),
        })
    }
}

fn plan_json(p: &Plan) -> Value {
    json!({
        "app": p.inspection,
        "format": p.badge(),
        "rootless": p.rootless,
        "needs_password": p.needs_password,
        "target": p.target,
        "desktop_id": p.desktop_id,
        "path_command": p.shim,
        "command": p.command,
        "simulation": p.native.as_ref().map(|n| &n.simulation),
        "duplicate": p.duplicate,
        "blockers": p.blockers,
        "warnings": p.warnings,
        "steps": p.steps,
        "needs_confirmation": p.confirm,
    })
}

/// Every install job, newest last.
#[derive(Default)]
pub struct InstallQueue {
    jobs: Vec<Job>,
    next_id: u64,
    next_batch: u64,
}

/// What the installer surface shows.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Sheet {
    /// The queue sheet for files dropped together (a batch id).
    Queue(u64),
    /// One job's confirmation, or its failure.
    Card(u64),
    /// The progress / finished pill.
    Pill(u64),
    Nothing,
}

/// The surface's state machine: a batch waiting on the queue sheet comes
/// first, then the first job needing a decision (or showing a failure),
/// then the newest running job, else the newest finished one.
pub fn sheet(jobs: &[Job]) -> Sheet {
    if let Some(j) = jobs.iter().find(|j| j.batched && !j.background && j.pending()) {
        return Sheet::Queue(j.batch);
    }
    if let Some(j) = jobs.iter().find(|j| !j.background && (j.phase == Phase::Confirm || j.phase == Phase::Failed)) {
        return Sheet::Card(j.id);
    }
    let visible: Vec<&Job> = jobs.iter().filter(|j| !j.background && j.phase != Phase::Confirm).collect();
    visible.iter().rev().find(|j| !j.finished()).or(visible.last()).map_or(Sheet::Nothing, |j| Sheet::Pill(j.id))
}

impl InstallQueue {
    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    pub fn job(&self, id: u64) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }

    fn job_mut(&mut self, id: u64) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    /// The first job waiting for a decision.
    pub fn confirming(&self) -> Option<&Job> {
        self.jobs.iter().find(|j| j.phase == Phase::Confirm || (j.phase == Phase::Failed && !j.background && j.plan.is_some()))
    }

    /// What the installer surface shows ([`sheet`]).
    pub fn sheet(&self) -> Sheet {
        sheet(&self.jobs)
    }

    /// A batch's jobs, in drop order.
    pub fn batch(&self, batch: u64) -> impl Iterator<Item = &Job> {
        self.jobs.iter().filter(move |j| j.batch == batch)
    }

    /// Jobs the pill shows (not hidden, not waiting for a decision).
    pub fn visible(&self) -> impl Iterator<Item = &Job> {
        self.jobs.iter().filter(|j| !j.background && j.phase != Phase::Confirm)
    }
}

struct GlobalQueue(Entity<InstallQueue>);
impl Global for GlobalQueue {}

/// The shared install queue (created on first use).
pub fn queue(cx: &mut App) -> Entity<InstallQueue> {
    if let Some(q) = cx.try_global::<GlobalQueue>() {
        return q.0.clone();
    }
    let q = cx.new(|_| InstallQueue::default());
    cx.set_global(GlobalQueue(q.clone()));
    q
}

/// The queue, when one exists (read-only callers).
pub fn queue_ref(cx: &App) -> Option<Entity<InstallQueue>> {
    cx.try_global::<GlobalQueue>().map(|q| q.0.clone())
}

/// Cheap: whether `path` is a file the installer takes (magic bytes).
pub fn accepts(path: &Path) -> bool {
    engine::accepts(path)
}

/// Queues `paths` for installation (see the module docs).
pub fn install(paths: Vec<PathBuf>, origin: InstallOrigin, cx: &mut App) {
    start(paths, origin, JobOptions::default(), None, cx);
}

fn settings(cx: &App) -> InstallSettings {
    InstallSettings::from_plugins(&config(cx).read(cx).get().plugins)
}

fn plugin_on(cx: &App) -> bool {
    let cfg = config(cx).read(cx).get();
    cx.try_global::<PluginRegistry>().is_none_or(|r| r.is_enabled(ID, &cfg.plugins))
}

/// Starts jobs; returns their ids. `reply` gets one JSON answer (CLI): the
/// queued job ids at once, or — for dry runs — the plans.
fn start(paths: Vec<PathBuf>, origin: InstallOrigin, options: JobOptions, reply: Option<async_channel::Sender<Result<String, String>>>, cx: &mut App) -> Vec<u64> {
    if !plugin_on(cx) {
        let msg = t("The App Installer plugin is off: turn it on in Studio → Plugins").to_string();
        post(t("App installer"), msg.clone(), String::new(), cx);
        if let Some(r) = reply {
            let _ = r.try_send(Err(msg));
        }
        return Vec::new();
    }
    if options.dry_run {
        dry_run(paths, reply, cx);
        return Vec::new();
    }
    // First use this session: sweep stale previews, off the UI thread.
    if !PREVIEWS_PRUNED.swap(true, Ordering::Relaxed) {
        let settings = settings(cx);
        cx.background_spawn(async move {
            let gone = engine::prune_previews(&InstallEnv::current(&settings), PREVIEW_MAX_AGE);
            if gone > 0 {
                tracing::debug!("installer: removed {gone} stale previews");
            }
        })
        .detach();
    }
    let q = queue(cx);
    let mut ids = Vec::new();
    let batched = paths.len() > 1 && !options.yes;
    let batch = q.update(cx, |q, _| {
        q.next_batch += 1;
        q.next_batch
    });
    for path in paths {
        let id = q.update(cx, |q, cx| {
            q.next_id += 1;
            let id = q.next_id;
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            q.jobs.push(Job {
                id,
                origin,
                path: path.clone(),
                name,
                app_id: None,
                desktop_id: None,
                icon: None,
                badge: String::new(),
                phase: Phase::Inspect,
                fraction: None,
                plan: None,
                choices: Choices::default(),
                result: None,
                options,
                background: false,
                batch,
                batched,
                install_when_ready: false,
                keep_in_dock: origin == InstallOrigin::Dock,
                started_at: None,
                cancel: Arc::new(AtomicBool::new(false)),
                finished_at: None,
            });
            cx.notify();
            id
        });
        ids.push(id);
        inspect(id, path, cx);
    }
    // Answer at once so the CLI never waits on a confirmation.
    if let Some(r) = reply {
        let jobs: Vec<Value> = ids.iter().filter_map(|id| q.read(cx).job(*id)).map(|j| json!({ "id": j.id, "file": j.path })).collect();
        let _ = r.try_send(Ok(serde_json::to_string_pretty(&json!({ "queued": jobs, "next": "mandara msg apps jobs" })).unwrap_or_default()));
    }
    crate::installer_overlay::ensure_open(cx);
    ids
}

/// `--dry-run`: the plans, nothing queued or installed.
fn dry_run(paths: Vec<PathBuf>, reply: Option<async_channel::Sender<Result<String, String>>>, cx: &mut App) {
    let settings = settings(cx);
    let task = cx.background_spawn(async move {
        let env = InstallEnv::current(&settings);
        let plans: Vec<Value> = paths
            .iter()
            .map(|p| match engine::plan(p, &settings, &env, &SystemRunner) {
                Ok(plan) => {
                    let choices = Choices { dry_run: true, ..plan.default_choices.clone() };
                    let outcome = engine::execute(&plan, &choices, &env, &SystemRunner, &|_| {}, &AtomicBool::new(false));
                    json!({ "file": p, "plan": plan_json(&plan), "would": outcome.ok() })
                }
                Err(e) => json!({ "file": p, "error": e }),
            })
            .collect();
        serde_json::to_string_pretty(&Value::Array(plans)).unwrap_or_default()
    });
    cx.spawn(async move |_| {
        let text = task.await;
        if let Some(r) = reply {
            let _ = r.send(Ok(text)).await;
        }
    })
    .detach();
}

fn inspect(id: u64, path: PathBuf, cx: &mut App) {
    let settings = settings(cx);
    let task = cx.background_spawn(async move {
        let env = InstallEnv::current(&settings);
        engine::plan(&path, &settings, &env, &SystemRunner)
    });
    cx.spawn(async move |cx| {
        let result = task.await;
        let _ = cx.update(|cx| on_planned(id, result, cx));
    })
    .detach();
}

fn on_planned(id: u64, result: Result<Plan, String>, cx: &mut App) {
    let q = queue(cx);
    let Some(job) = q.read(cx).job(id).cloned().filter(|j| !j.cancel.load(Ordering::Relaxed)) else {
        // Cancelled (or gone) while inspecting: its previews aren't needed.
        if let Ok(plan) = &result {
            delete_previews(preview_paths(&plan.inspection), cx);
        }
        return;
    };
    let plan = match result {
        Ok(p) => p,
        Err(e) => return finish(id, Err(e), cx),
    };
    let blocked = plan.blockers.first().cloned();
    // "Install all" counts as the confirmation; a batch otherwise waits.
    let auto = job.options.yes || job.install_when_ready || (!plan.confirm && !job.batched);
    let plan = Rc::new(plan);
    q.update(cx, |q, cx| {
        if let Some(j) = q.job_mut(id) {
            j.name = plan.inspection.name.clone();
            j.app_id = Some(plan.inspection.id.clone());
            j.desktop_id = plan.desktop_id.clone();
            j.icon = plan.inspection.icon_file.clone().or_else(|| plan.desktop_id.as_deref().and_then(|d| crate::apps::find_icon(d)));
            j.badge = plan.badge();
            j.choices = Choices { path_shim: plan.default_choices.path_shim && !j.options.no_path_shim && plan.shim.as_ref().is_some_and(|s| s.conflict.is_none()), dry_run: j.options.dry_run, ..plan.default_choices.clone() };
            j.plan = Some(plan.clone());
            j.phase = Phase::Confirm;
        }
        cx.notify();
    });
    match blocked {
        // A blocked file from the CLI fails; on screen the card explains.
        Some(why) if job.options.yes => finish(id, Err(why), cx),
        Some(_) => {}
        None if auto => run(id, cx),
        None => {}
    }
    crate::installer_overlay::ensure_open(cx);
}

/// Sets a job's choices from the card.
pub fn set_choices(id: u64, f: impl FnOnce(&mut Choices), cx: &mut App) {
    queue(cx).update(cx, |q, cx| {
        if let Some(j) = q.job_mut(id) {
            f(&mut j.choices);
        }
        cx.notify();
    });
}

/// The card's Install button.
pub fn confirm(id: u64, cx: &mut App) {
    run(id, cx);
}

/// The queue sheet's "Install all": every ready file in the batch installs
/// (files still being inspected install once ready); files that can't be
/// installed then explain themselves one by one.
pub fn install_all(batch: u64, cx: &mut App) {
    let q = queue(cx);
    let ready: Vec<u64> = q.update(cx, |q, cx| {
        let mut ready = Vec::new();
        for j in q.jobs.iter_mut().filter(|j| j.batch == batch) {
            j.batched = false;
            match j.phase {
                Phase::Inspect => j.install_when_ready = true,
                Phase::Confirm if j.plan.as_ref().is_some_and(|p| p.can_install()) => ready.push(j.id),
                _ => {}
            }
        }
        cx.notify();
        ready
    });
    for id in ready {
        run(id, cx);
    }
}

/// The queue sheet's "Review each": one confirmation card per file.
pub fn review_each(batch: u64, cx: &mut App) {
    queue(cx).update(cx, |q, cx| {
        for j in q.jobs.iter_mut().filter(|j| j.batch == batch) {
            j.batched = false;
        }
        cx.notify();
    });
}

/// The queue sheet's "Cancel": every file of the batch not yet installing.
pub fn cancel_batch(batch: u64, cx: &mut App) {
    let ids: Vec<u64> = queue(cx).read(cx).batch(batch).filter(|j| j.pending()).map(|j| j.id).collect();
    for id in ids {
        cancel(id, cx);
    }
}

/// Takes one file out of the queue sheet to its own card.
pub fn review_one(id: u64, cx: &mut App) {
    queue(cx).update(cx, |q, cx| {
        if let Some(j) = q.job_mut(id) {
            j.batched = false;
        }
        cx.notify();
    });
}

/// The finished card's "Keep in Dock" switch (applied at once).
pub fn set_keep_in_dock(id: u64, on: bool, cx: &mut App) {
    let desktop = queue(cx).update(cx, |q, cx| {
        let j = q.job_mut(id)?;
        j.keep_in_dock = on;
        cx.notify();
        j.desktop_id.clone().filter(|_| j.phase == Phase::Done)
    });
    if let Some(d) = desktop {
        if on { pin_to_dock(&d, cx) } else { unpin_from_dock(&d, cx) }
    }
}

/// Queues a failed job's file again (the failure card's "Try again").
pub fn retry(id: u64, cx: &mut App) {
    let Some(job) = queue(cx).read(cx).job(id).cloned() else { return };
    dismiss(id, cx);
    start(vec![job.path], job.origin, JobOptions { yes: false, ..job.options }, None, cx);
}

/// Cancels a job: before it runs it just goes; while copying it stops at
/// the next chunk and rolls back. Package managers can't be interrupted
/// once polkit said yes.
pub fn cancel(id: u64, cx: &mut App) {
    let q = queue(cx);
    let Some(job) = q.read(cx).job(id).cloned() else { return };
    job.cancel.store(true, Ordering::Relaxed);
    if matches!(job.phase, Phase::Inspect | Phase::Confirm) || job.finished() {
        if !job.finished() {
            finish(id, Err(t("Cancelled").to_string()), cx);
        }
        dismiss(id, cx);
    }
}

/// Removes a finished job (the card's Close).
pub fn dismiss(id: u64, cx: &mut App) {
    let gone = queue(cx).update(cx, |q, cx| {
        let gone: Vec<PathBuf> = q.jobs.iter().filter(|j| j.id == id && j.finished()).filter_map(|j| j.plan.as_ref()).flat_map(|p| preview_paths(&p.inspection)).collect();
        q.jobs.retain(|j| j.id != id || !j.finished());
        cx.notify();
        // Another job of the same app may still show the same files.
        let kept: Vec<PathBuf> = q.jobs.iter().filter_map(|j| j.plan.as_ref()).flat_map(|p| preview_paths(&p.inspection)).collect();
        gone.into_iter().filter(|p| !kept.contains(p)).collect::<Vec<_>>()
    });
    delete_previews(gone, cx);
}

/// An inspection's preview files (icon and screenshots).
fn preview_paths(i: &engine::Inspection) -> Vec<PathBuf> {
    i.icon_file.iter().chain(&i.screenshots).cloned().collect()
}

/// Deletes preview files off the UI thread (only inside the preview folder).
fn delete_previews(paths: Vec<PathBuf>, cx: &mut App) {
    if paths.is_empty() {
        return;
    }
    let settings = settings(cx);
    cx.background_spawn(async move { engine::remove_previews(&paths, &InstallEnv::current(&settings)) }).detach();
}

/// Hides the pill; completion arrives as a notification.
pub fn send_to_background(id: u64, cx: &mut App) {
    queue(cx).update(cx, |q, cx| {
        if let Some(j) = q.job_mut(id) {
            j.background = true;
        }
        cx.notify();
    });
}

enum Msg {
    Progress(Progress),
    Finished(Result<Outcome, String>),
}

fn run(id: u64, cx: &mut App) {
    let q = queue(cx);
    let Some(job) = q.read(cx).job(id).cloned() else { return };
    let Some(plan) = job.plan.clone().filter(|_| job.phase == Phase::Confirm) else { return };
    let settings = settings(cx);
    q.update(cx, |q, cx| {
        if let Some(j) = q.job_mut(id) {
            j.phase = Phase::Install;
            j.fraction = None;
            j.batched = false;
            j.started_at = Some(Instant::now());
        }
        cx.notify();
    });
    let (tx, rx) = async_channel::unbounded::<Msg>();
    let plan_owned: Plan = (*plan).clone();
    let choices = job.choices.clone();
    let cancel = job.cancel.clone();
    let spawned = std::thread::Builder::new().name("mandara-install".into()).spawn(move || {
        let env = InstallEnv::current(&settings);
        let last = std::cell::Cell::new((Phase::Inspect, -1.0f32));
        let progress = |p: Progress| {
            let f = p.fraction().unwrap_or(-1.0);
            let (phase, prev) = last.get();
            if phase != p.phase || (f - prev).abs() >= PROGRESS_STEP || f >= 1.0 {
                last.set((p.phase, f));
                let _ = tx.send_blocking(Msg::Progress(p));
            }
        };
        let result = engine::execute(&plan_owned, &choices, &env, &SystemRunner, &progress, &cancel);
        let _ = tx.send_blocking(Msg::Finished(result));
    });
    if let Err(e) = spawned {
        return finish(id, Err(e.to_string()), cx);
    }
    cx.spawn(async move |cx| {
        while let Ok(msg) = rx.recv().await {
            let done = matches!(msg, Msg::Finished(_));
            let _ = cx.update(|cx| match msg {
                Msg::Progress(p) => queue(cx).update(cx, |q, cx| {
                    if let Some(j) = q.job_mut(id) {
                        j.phase = p.phase;
                        j.fraction = p.fraction();
                    }
                    cx.notify();
                }),
                Msg::Finished(r) => finish(id, r, cx),
            });
            if done {
                break;
            }
        }
    })
    .detach();
}

fn finish(id: u64, result: Result<Outcome, String>, cx: &mut App) {
    let q = queue(cx);
    let cancelled = result.as_ref().err().is_some_and(|e| e == t("Cancelled"));
    let phase = match &result {
        Ok(_) => Phase::Done,
        Err(_) if cancelled => Phase::Cancelled,
        Err(_) => Phase::Failed,
    };
    let Some(job) = q.update(cx, |q, cx| {
        let j = q.job_mut(id)?;
        j.phase = phase;
        j.fraction = Some(if phase == Phase::Done { 1.0 } else { j.fraction.unwrap_or(0.0) });
        if let Ok(o) = &result {
            j.desktop_id = o.desktop_id.clone().or(j.desktop_id.clone());
            j.app_id = Some(o.id.clone());
            j.name = o.name.clone();
        }
        j.result = Some(result.clone());
        j.finished_at = Some(Instant::now());
        cx.notify();
        Some(j.clone())
    }) else {
        return;
    };
    if phase == Phase::Done
        && job.keep_in_dock
        && let Some(d) = &job.desktop_id
    {
        pin_to_dock(d, cx);
    }
    let s = settings(cx);
    if s.notify && !cancelled && (job.background || job.origin == InstallOrigin::Command) {
        match &result {
            Ok(o) => post(t("App installer"), tf("{app} is installed", &[("app", &o.name)]), t("Find it in your apps").to_string(), cx),
            Err(e) => post(t("App installer"), tf("Couldn't install {app}", &[("app", &job.name)]), e.clone(), cx),
        }
    }
    // Finished jobs leave the queue after a while (failed ones on screen
    // wait to be closed).
    let linger = if phase == Phase::Failed { FAILED_LINGER } else { JOB_LINGER };
    cx.spawn(async move |cx| {
        cx.background_executor().timer(linger).await;
        let _ = cx.update(|cx| dismiss(id, cx));
    })
    .detach();
    crate::installer_overlay::ensure_open(cx);
}

fn post(app: &str, summary: String, body: String, cx: &mut App) {
    super::notifications::send(cx, mandara_services::NotificationCommand::Post { app: app.into(), icon: "download".into(), summary, body });
}

/// Opens an installed app by desktop id.
pub fn open_app(desktop_id: &str, cx: &mut App) -> Result<(), String> {
    let options = config(cx).read(cx).get().launcher.launch.clone();
    let app = crate::apps::discover().into_iter().find(|a| a.id == desktop_id || a.id.trim_end_matches(".desktop") == desktop_id).ok_or_else(|| tf("No installed app {id}", &[("id", &desktop_id)]))?;
    crate::apps::launch_with(&app, None, &options)
}

/// Pins an app to the dock.
pub fn pin_to_dock(desktop_id: &str, cx: &mut App) {
    let id = desktop_id.trim_end_matches(".desktop").to_string();
    crate::state::update_config(cx, move |c| {
        if !c.dock.pinned.iter().any(|p| p == &id) {
            c.dock.pinned.push(id);
        }
    });
}

/// Takes an app off the dock.
pub fn unpin_from_dock(desktop_id: &str, cx: &mut App) {
    let id = desktop_id.trim_end_matches(".desktop").to_string();
    crate::state::update_config(cx, move |c| c.dock.pinned.retain(|p| p != &id));
}

/// `[paths…, flags…]` → paths and options.
fn parse_install_args(words: &[String]) -> Result<(Vec<PathBuf>, JobOptions), String> {
    let mut options = JobOptions::default();
    let mut paths = Vec::new();
    for w in words {
        match w.as_str() {
            FLAG_YES => options.yes = true,
            FLAG_NO_SHIM => options.no_path_shim = true,
            FLAG_DRY_RUN => options.dry_run = true,
            f if f.starts_with("--") => return Err(tf("Unknown option {option}; expected {options}", &[("option", &f), ("options", &INSTALL_FLAGS.join(", "))])),
            p => paths.push(PathBuf::from(p)),
        }
    }
    if paths.is_empty() {
        return Err(t("Name at least one file to install").to_string());
    }
    if let Some(rel) = paths.iter().find(|p| !p.is_absolute()) {
        return Err(tf("Use an absolute path: {path}", &[("path", &rel.display())]));
    }
    if let Some(missing) = paths.iter().find(|p| !p.is_file()) {
        return Err(tf("No such file: {path}", &[("path", &missing.display())]));
    }
    // `MANDARA_INSTALL_DRY_RUN` keeps the on-screen flow and only stops
    // execution (the engine checks it).
    Ok((paths, options))
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("App installer"), tk("Install apps by dropping AppImages, Flatpaks, archives or packages on the dock or Studio"), None).opt_in(), |_, cx, _| {
        queue(cx);
        mandara_plugins::PluginInstance::empty()
    });
    r.command(
        ID,
        CommandSpec::new_output("apps install", tk("Install apps from files (AppImage, Flatpak, .tar.gz/.zip, .deb/.rpm/…): shows the confirmation on screen unless --yes; --dry-run prints the plan"), |a, cx| {
            let (paths, options) = parse_install_args(&a.0)?;
            let (tx, rx) = async_channel::bounded(1);
            start(paths, InstallOrigin::Command, options, Some(tx), cx);
            Ok(Output::Pending(rx))
        })
        .arg(ArgSpec::required("paths", ArgKind::String, tk("absolute file paths, then --yes (skip the confirmation), --no-path-shim, --dry-run")))
        .example("apps install /home/me/Downloads/Obsidian-1.6.7.AppImage")
        .example("apps install /home/me/Downloads/app.deb --dry-run"),
    );
    r.command(
        ID,
        CommandSpec::new("apps jobs", tk("App installer jobs with their phase and progress"), |_, cx| {
            let q = queue(cx);
            Ok(Value::Array(q.read(cx).jobs().iter().map(Job::json).collect()))
        }),
    );
    r.command(
        ID,
        CommandSpec::new("apps jobs cancel", tk("Cancel an install job (rootless installs roll back)"), |a, cx| {
            let id = u64::try_from(a.int(0)?).map_err(|e| e.to_string())?;
            if queue(cx).read(cx).job(id).is_none() {
                return Err(tf("No install job {id}", &[("id", &id)]));
            }
            cancel(id, cx);
            Ok(json!({ "cancelled": id }))
        })
        .arg(ArgSpec::required("id", ArgKind::Integer, tk("job id from apps jobs"))),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: u64, batch: u64, batched: bool, phase: Phase) -> Job {
        Job {
            id,
            origin: InstallOrigin::Dock,
            path: PathBuf::from(format!("/f/{id}.AppImage")),
            name: String::new(),
            app_id: None,
            desktop_id: None,
            icon: None,
            badge: String::new(),
            phase,
            fraction: None,
            plan: None,
            choices: Choices::default(),
            result: None,
            options: JobOptions::default(),
            background: false,
            batch,
            batched,
            install_when_ready: false,
            keep_in_dock: true,
            started_at: None,
            cancel: Arc::new(AtomicBool::new(false)),
            finished_at: None,
        }
    }

    #[test]
    fn the_queue_sheet_leads_then_cards_then_the_pill() {
        assert_eq!(sheet(&[]), Sheet::Nothing);
        // Three files dropped together: the queue sheet, while any is pending.
        let mut jobs = vec![job(1, 1, true, Phase::Inspect), job(2, 1, true, Phase::Confirm), job(3, 1, true, Phase::Confirm)];
        assert_eq!(sheet(&jobs), Sheet::Queue(1));
        // "Review each" releases them: one card at a time, in order.
        for j in &mut jobs {
            j.batched = false;
        }
        assert_eq!(sheet(&jobs), Sheet::Card(2));
        // "Install all": running ones show the pill; a blocked one its card.
        jobs[0].phase = Phase::Install;
        jobs[1].phase = Phase::Install;
        assert_eq!(sheet(&jobs), Sheet::Card(3));
        jobs[2].phase = Phase::Cancelled;
        assert_eq!(sheet(&jobs), Sheet::Pill(2), "the newest running job");
        jobs[1].phase = Phase::Done;
        jobs[0].phase = Phase::Done;
        assert_eq!(sheet(&jobs), Sheet::Pill(3), "else the newest finished");
        // A failure on screen asks to be read; hidden jobs never show.
        jobs[0].phase = Phase::Failed;
        assert_eq!(sheet(&jobs), Sheet::Card(1));
        for j in &mut jobs {
            j.background = true;
        }
        assert_eq!(sheet(&jobs), Sheet::Nothing);
        // A single file never uses the queue sheet.
        assert_eq!(sheet(&[job(9, 2, false, Phase::Confirm)]), Sheet::Card(9));
    }

    #[test]
    fn elapsed_stops_when_finished() {
        let mut j = job(1, 1, false, Phase::Done);
        assert_eq!(j.elapsed(), Duration::ZERO);
        let t = Instant::now();
        j.started_at = Some(t);
        j.finished_at = Some(t + Duration::from_secs(3));
        assert_eq!(j.elapsed(), Duration::from_secs(3));
    }

    #[test]
    fn install_args() {
        let dir = std::env::temp_dir();
        let file = dir.join(format!("mandara-installer-arg-{}.AppImage", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let words = vec![file.to_string_lossy().to_string(), FLAG_YES.into(), FLAG_NO_SHIM.into()];
        let (paths, o) = parse_install_args(&words).unwrap();
        assert_eq!(paths, vec![file.clone()]);
        assert!(o.yes && o.no_path_shim);
        assert!(parse_install_args(&["--bogus".into()]).is_err());
        assert!(parse_install_args(&["relative.AppImage".into()]).is_err());
        assert!(parse_install_args(&[]).is_err());
        std::fs::remove_file(file).unwrap();
    }
}
