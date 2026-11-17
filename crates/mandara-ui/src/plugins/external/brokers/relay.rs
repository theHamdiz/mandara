//! Studio forwards typed live-wallpaper requests; the shell owns every renderer.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, Global};
use mandara_plugins::Output;
use mandara_plugins::live_wallpaper::{Action, Request as LiveRequest};
use mandara_plugins::wasm::Request;
use serde_json::Value;

use super::{deliver_raw, live, live_access, loaded, project_of};

/// Installed by the Studio bootstrap. Only the typed operation crosses this boundary.
#[derive(Clone)]
pub struct ShellRelay(pub Arc<dyn Fn(&LiveRequest) -> Result<Value, String> + Send + Sync>);
impl Global for ShellRelay {}

/// The IPC bridge cannot fan out unbounded connection threads.
const MAX_RUNNING: usize = 2;
const MAX_WAITING: usize = 32;

struct Job { request: LiveRequest, id: u64 }

thread_local! {
    static QUEUE: RefCell<super::queue::Queue<Job, MAX_RUNNING, MAX_WAITING>> = const { RefCell::new(super::queue::Queue::new()) };
}

fn request(plugin: &str, generation: u64, r: &Request, resolve: impl FnOnce(u64) -> Result<(PathBuf, PathBuf), String>) -> Result<LiveRequest, String> {
    let action = match r.operation.as_str() {
        "status" => Action::Status {},
        "stop" => Action::Stop { output: live::output(&r.payload)? },
        "render" => {
            let handle = r.payload.get("project").and_then(Value::as_u64).ok_or("`project` must be a project handle")?;
            let (project, root) = resolve(handle)?;
            Action::Render { project, root, output: live::output(&r.payload)? }
        }
        _ => return Err("unknown live wallpaper operation".into()),
    };
    Ok(LiveRequest { plugin: plugin.to_string(), generation, action })
}

fn wanted(job: &Job, cx: &App) -> bool {
    let plugin = &job.request.plugin;
    let enabled = cx.global::<mandara_plugins::PluginRegistry>().is_enabled(plugin, &crate::state::config(cx).read(cx).get().plugins);
    live_access(plugin, cx).is_some_and(|access| job.request.authorize(&access, enabled).is_ok())
        && loaded(plugin).is_some_and(|p| p.borrow().as_ref().is_some_and(|p| p.is_open(job.id)))
}

pub(super) fn forward(plugin: &str, r: &Request, cx: &mut App) -> Option<Result<Value, String>> {
    if !cx.has_global::<ShellRelay>() { return Some(Err("Studio is not connected to the shell".into())); }
    let Some(access) = live_access(plugin, cx) else { return Some(Err("the plugin is not available".into())); };
    let request = match request(plugin, access.generation, r, |handle| project_of(plugin, handle, cx)) { Ok(request) => request, Err(error) => return Some(Err(error)) };
    if QUEUE.with(|q| q.borrow_mut().push(Job { request, id: r.id }).is_err()) {
        return Some(Err("the shell request queue is full".into()));
    }
    pump(cx);
    None
}

fn pump(cx: &mut App) {
    loop {
        let job = QUEUE.with(|q| q.borrow_mut().next_job());
        let Some(job) = job else { return };
        if !wanted(&job, cx) { QUEUE.with(|q| q.borrow_mut().complete()); continue; }
        let relay = cx.global::<ShellRelay>().clone();
        cx.spawn(async move |cx| {
            let (job, result) = mandara_services::runtime::unblock(move || {
                let result = (relay.0)(&job.request).map(|value| value.to_string());
                (job, result)
            }).await;
            cx.update(|cx| {
                QUEUE.with(|q| q.borrow_mut().complete());
                if wanted(&job, cx) { deliver_raw(&job.request.plugin, job.id, result, cx); }
                pump(cx);
            });
        }).detach();
    }
}

pub(super) fn stop(plugin: &str, ids: Option<&[u64]>) {
    QUEUE.with(|q| q.borrow_mut().retain(|job| job.request.plugin != plugin || ids.is_some_and(|ids| !ids.contains(&job.id))));
}

/// `plugins live <request>` is an IPC endpoint, never a guest command permission.
pub fn command(json: &str, cx: &mut App) -> Result<Output, String> {
    live::remote(LiveRequest::parse(json)?, cx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_plugins::extension::Capability;
    use serde_json::json;

    #[test]
    fn studio_resolves_projects_from_handles_and_ignores_guest_paths_and_programs() {
        let r = Request { id: 7, capability: Capability::WallpaperLive, operation: "render".into(), payload: json!({"project":42,"root":"/etc","program":"guest-command","output":"DP-1"}).as_object().unwrap().clone() };
        let request = request("workshop", 7, &r, |handle| {
            assert_eq!(handle, 42);
            Ok(("/approved/123".into(), "/approved".into()))
        }).unwrap();
        assert_eq!(request.action, Action::Render { root: "/approved".into(), project: "/approved/123".into(), output: Some("DP-1".into()) });
        assert!(!serde_json::to_string(&request).unwrap().contains("guest-command"));
    }

    #[test]
    fn missing_handles_and_invalid_display_payloads_never_reach_the_transport() {
        for payload in [json!({"project":"/path"}), json!({"project":1,"output":5})] {
            let r = Request { id: 1, capability: Capability::WallpaperLive, operation: "render".into(), payload: payload.as_object().unwrap().clone() };
            assert!(request("workshop", 7, &r, |_| Ok(("/approved/123".into(), "/approved".into()))).is_err());
        }
    }
}
