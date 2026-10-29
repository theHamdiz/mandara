//! The dock's side of the drag-and-drop app installer
//! (`plugins::app_installer`, a separate first-party plugin).
//!
//! The dock talks to the installer through [`InstallBridge`], a small
//! global set by [`wire_installer`] from `plugins::app_installer`'s public
//! API (`accepts`, `install(paths, InstallOrigin::Dock, cx)`, `queue`), so
//! the dock's logic stays testable without it. The bridge carries:
//! * `accepts(path)` — whether a dragged file is an installable app
//!   (the "Drop to install" glow, and the drop on the dock /
//!   Applications slot);
//! * `install(paths, cx)` — starts installs (origin: the dock);
//! * `jobs(cx)` — the queue as [`InstallSlot`]s (progress ring
//!   placeholders, failure shake + tooltip);
//! * `installed_by_mandara(desktop_id)` — offers "Uninstall…" in the
//!   icon's Options;
//! * `observe(cx, callback)` — calls back whenever the queue changes.
//!
//! Finished installs from the dock are pinned here (with a landing
//! bounce); failures shake their placeholder and show the error as its
//! tooltip until it is dismissed (click) or [`FAILED_FOR`] passes.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui::{App, Global};

use super::model::InstallSlot;

/// How long a failed install's placeholder stays (shaking once, then
/// showing its error) before it goes away by itself.
pub const FAILED_FOR: Duration = Duration::from_secs(8);

pub type Callback = Rc<dyn Fn(&mut App)>;
pub type InstallFn = Rc<dyn Fn(Vec<PathBuf>, &mut App)>;
pub type JobsFn = Rc<dyn Fn(&App) -> Vec<InstallJob>>;
pub type ObserveFn = Rc<dyn Fn(&mut App, Callback)>;

/// The app installer, as the dock sees it.
#[derive(Clone)]
pub struct InstallBridge {
    pub accepts: Rc<dyn Fn(&Path) -> bool>,
    pub install: InstallFn,
    pub jobs: JobsFn,
    pub installed_by_mandara: Rc<dyn Fn(&str) -> bool>,
    pub observe: ObserveFn,
}
impl Global for InstallBridge {}

/// One install, as the dock draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct InstallJob {
    pub id: u64,
    /// Known once the package is unpacked.
    pub desktop_id: Option<String>,
    pub name: String,
    pub icon: String,
    /// 0–1.
    pub progress: f32,
    pub state: JobState,
    /// Started from the dock (finished ones get pinned).
    pub from_dock: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum JobState {
    Running,
    Done,
    Failed(String),
}

/// Installs the dock shows: running ones and recent failures.
pub fn slots(jobs: &[InstallJob]) -> Vec<InstallSlot> {
    jobs.iter()
        .filter(|j| j.from_dock && j.state != JobState::Done)
        .map(|j| InstallSlot { id: j.id, name: j.name.clone(), icon: j.icon.clone(), progress: j.progress.clamp(0.0, 1.0), failed: match &j.state { JobState::Failed(e) => Some(e.clone()), _ => None } })
        .collect()
}

/// Desktop ids of dock installs that finished since `seen` (job ids
/// already handled), to pin.
pub fn finished(jobs: &[InstallJob], seen: &[u64]) -> Vec<(u64, String)> {
    jobs.iter().filter(|j| j.from_dock && j.state == JobState::Done && !seen.contains(&j.id)).filter_map(|j| j.desktop_id.clone().map(|d| (j.id, d))).collect()
}

/// Sets the bridge and starts following its queue.
pub fn set_bridge(bridge: InstallBridge, cx: &mut App) {
    cx.set_global(bridge);
    super::follow_installs(cx);
}

/// The installer's job, as the dock draws it.
fn job_view(j: &crate::plugins::app_installer::Job) -> InstallJob {
    use mandara_services::app_install::Phase;
    let state = match j.phase {
        Phase::Done | Phase::Cancelled => JobState::Done,
        Phase::Failed => JobState::Failed(j.result.as_ref().and_then(|r| r.as_ref().err().cloned()).unwrap_or_else(|| mandara_i18n::t("Failed").to_string())),
        _ => JobState::Running,
    };
    InstallJob {
        id: j.id,
        // A cancelled job never pins anything.
        desktop_id: if j.phase == Phase::Done { j.desktop_id.clone() } else { None },
        name: j.name.clone(),
        icon: j.icon.as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
        progress: j.progress(),
        state,
        from_dock: j.origin == crate::plugins::app_installer::InstallOrigin::Dock,
    }
}

/// Whether Mandara's installer installed desktop id `id` (AppImage,
/// portable, Flatpak or package it recorded).
fn recorded(id: &str) -> bool {
    let data = dirs::data_dir().unwrap_or_default();
    let id = mandara_config::dock::pin_id(id);
    mandara_services::app_install::registry::installed(&data).iter().any(|e| e.desktop_id.as_deref().is_some_and(|d| d.eq_ignore_ascii_case(&id)) || e.flatpak.as_deref().is_some_and(|f| f.eq_ignore_ascii_case(&id)))
}

/// Connects the dock to the app installer plugin.
pub fn wire_installer(cx: &mut App) {
    use crate::plugins::app_installer as ai;
    let bridge = InstallBridge {
        accepts: Rc::new(ai::accepts),
        install: Rc::new(|paths, cx| ai::install(paths, ai::InstallOrigin::Dock, cx)),
        jobs: Rc::new(|cx| ai::queue_ref(cx).map(|q| q.read(cx).jobs().iter().map(job_view).collect()).unwrap_or_default()),
        installed_by_mandara: Rc::new(recorded),
        observe: Rc::new(|cx, callback| {
            let q = ai::queue(cx);
            cx.observe(&q, move |_, cx| callback(cx)).detach();
        }),
    };
    set_bridge(bridge, cx);
}

pub fn bridge(cx: &App) -> Option<InstallBridge> {
    cx.try_global::<InstallBridge>().cloned()
}

pub fn accepts(path: &Path, cx: &App) -> bool {
    bridge(cx).is_some_and(|b| (b.accepts)(path))
}

pub fn installed_by_mandara(desktop_id: &str, cx: &App) -> bool {
    bridge(cx).is_some_and(|b| (b.installed_by_mandara)(desktop_id))
}

pub fn jobs(cx: &App) -> Vec<InstallJob> {
    bridge(cx).map(|b| (b.jobs)(cx)).unwrap_or_default()
}

/// Hands installable `paths` to the installer. Returns whether it took
/// them.
pub fn install(paths: Vec<PathBuf>, cx: &mut App) -> bool {
    match bridge(cx) {
        Some(b) if !paths.is_empty() => {
            (b.install)(paths, cx);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: u64, state: JobState, from_dock: bool) -> InstallJob {
        InstallJob { id, desktop_id: Some(format!("app{id}")), name: format!("App {id}"), icon: String::new(), progress: 1.4, state, from_dock }
    }

    #[test]
    fn running_and_failed_dock_installs_get_slots_and_finished_ones_get_pinned() {
        let jobs = vec![job(1, JobState::Running, true), job(2, JobState::Done, true), job(3, JobState::Failed("bad".into()), true), job(4, JobState::Running, false)];
        let s = slots(&jobs);
        assert_eq!(s.iter().map(|s| s.id).collect::<Vec<_>>(), [1, 3]);
        assert_eq!(s[0].progress, 1.0, "clamped");
        assert_eq!(s[1].failed.as_deref(), Some("bad"));
        assert_eq!(finished(&jobs, &[]), [(2, "app2".to_string())]);
        assert!(finished(&jobs, &[2]).is_empty(), "pinned once");
    }
}
