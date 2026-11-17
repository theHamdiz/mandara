//! One renderer preparation runs at a time; superseded selections never reach the filesystem.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::App;
use mandara_plugins::{Output, live_wallpaper::{Action, Request}};

use super::{PENDING, WallpaperKey, authorize_request, pending, prepare, settle, start, watch};
use super::super::queue::Queue;

/// A burst of selections waits in bounded storage behind the one file/renderer lookup.
const MAX_WAITING: usize = 32;

struct Job {
    request: Request,
    local: Option<(u64, u64)>,
    outputs: Vec<String>,
    pending: Rc<pending::Start>,
    reply: async_channel::Sender<Result<String, String>>,
}

thread_local! {
    static QUEUE: RefCell<Queue<Job, 1, MAX_WAITING>> = const { RefCell::new(Queue::new()) };
}

pub(super) fn enqueue(request: Request, local: Option<(u64, u64)>, outputs: Vec<String>, cx: &mut App) -> Result<Output, String> {
    if !QUEUE.with(|q| q.borrow().has_room()) { return Err("the live wallpaper request queue is full".into()); }
    let Action::Render { root, .. } = &request.action else { return Err("a render request is required".into()) };
    let wallpaper = WallpaperKey::of(crate::state::config(cx).read(cx).get());
    let pending = PENDING.with(|p| p.borrow_mut().begin(&request.plugin, root, request.generation, &outputs, wallpaper));
    let (reply, result) = async_channel::bounded(1);
    let job = Job { request, local, outputs, pending, reply };
    if let Some(job) = QUEUE.with(|q| q.borrow_mut().push(job).err()) {
        cancel(&job, cx);
        return Err("the live wallpaper request queue is full".into());
    }
    watch(cx);
    pump(cx);
    Ok(result.into())
}

fn wanted(job: &Job, cx: &App) -> bool {
    !job.reply.is_closed() && PENDING.with(|p| p.borrow().owns(&job.pending))
        && job.local.is_none_or(|(_, id)| super::super::loaded(&job.request.plugin).is_some_and(|p| p.borrow().as_ref().is_some_and(|p| p.is_open(id))))
        && authorize_request(&job.request, cx).is_ok()
}

fn cancel(job: &Job, cx: &mut App) {
    PENDING.with(|p| p.borrow_mut().cancel(&job.pending));
    let _ = job.reply.try_send(Err("live wallpaper request superseded or cancelled".into()));
    settle(cx);
}

fn pump(cx: &mut App) {
    loop {
        let job = QUEUE.with(|q| q.borrow_mut().next_job());
        let Some(job) = job else { return };
        if !wanted(&job, cx) {
            QUEUE.with(|q| q.borrow_mut().complete());
            cancel(&job, cx);
            continue;
        }
        let Action::Render { project, root, .. } = &job.request.action else {
            QUEUE.with(|q| q.borrow_mut().complete());
            cancel(&job, cx);
            continue;
        };
        let (project, root) = (project.clone(), root.clone());
        cx.spawn(async move |cx| {
            let prepared = mandara_services::runtime::unblock(move || prepare(&project, &root)).await;
            cx.update(|cx| {
                QUEUE.with(|q| q.borrow_mut().complete());
                let open = wanted(&job, cx);
                let wallpaper = WallpaperKey::of(crate::state::config(cx).read(cx).get());
                let outputs = PENDING.with(|p| p.borrow_mut().complete(&job.pending, job.outputs, &wallpaper, open));
                let answer = if outputs.is_empty() { Err("live wallpaper request superseded or cancelled".into()) }
                    else { prepared.and_then(|prepared| start(&job.request.plugin, job.request.generation, job.local.map(|(handle, _)| handle), prepared, &outputs, cx)) };
                if answer.is_err() { settle(cx); }
                let _ = job.reply.try_send(answer.map(|value| value.to_string()));
                pump(cx);
            });
        }).detach();
        return;
    }
}

pub(super) fn stop(plugin: &str, only: Option<&str>) {
    QUEUE.with(|q| q.borrow_mut().retain(|job| {
        let keep = job.request.plugin != plugin || only.is_some_and(|only| job.outputs.iter().any(|output| output != only));
        if !keep { let _ = job.reply.try_send(Err("live wallpaper request cancelled".into())); }
        keep
    }));
}

pub(super) fn clear() {
    QUEUE.with(|q| q.borrow_mut().retain(|job| { let _ = job.reply.try_send(Err("live wallpaper request cancelled".into())); false }));
}

pub(super) fn prune(cx: &App) -> bool {
    let mut changed = false;
    QUEUE.with(|q| q.borrow_mut().retain(|job| {
        let keep = wanted(job, cx);
        if !keep {
            PENDING.with(|pending| pending.borrow_mut().cancel(&job.pending));
            let _ = job.reply.try_send(Err("live wallpaper request superseded or cancelled".into()));
            changed = true;
        }
        keep
    }));
    changed
}
