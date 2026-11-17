//! Scoped file requests have bounded workers and one running operation per plugin.

use std::cell::RefCell;
use std::path::PathBuf;

use gpui::App;
use mandara_plugins::extension::Capability;
use mandara_plugins::wasm::{ImageOrigin, Request};
use mandara_services::plugin_files::{self, Folder, ScopedFiles as _};
use serde_json::{Value, json};

use super::{deliver, deliver_raw, granted_folder, issue, live_access, loaded};

/// A stalled mount in one plugin must leave a worker for another plugin.
const MAX_RUNNING: usize = 2;
/// The waiting queue cannot grow with bursts of guest requests.
const MAX_QUEUED: usize = 32;

enum Operation { Entries { relative: PathBuf, offset: usize, limit: usize }, Read { relative: PathBuf, offset: u64, limit: usize } }
struct Job { plugin: String, id: u64, root: PathBuf, operation: Operation }
enum Answer { Entries(plugin_files::Page), Read(String) }

thread_local! {
    static QUEUE: RefCell<super::queue::Queue<Job, MAX_RUNNING, MAX_QUEUED>> = const { RefCell::new(super::queue::Queue::new()) };
    static ACTIVE: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn number(payload: &serde_json::Map<String, Value>, key: &str, default: u64, max: u64) -> Result<u64, String> {
    match payload.get(key) {
        None => Ok(default),
        Some(value) => value.as_u64().filter(|n| *n <= max).ok_or_else(|| format!("`{key}` is out of range")),
    }
}

fn handle(plugin: &str, value: Option<&Value>, directory: bool) -> Result<(PathBuf, PathBuf), String> {
    let handle = value.and_then(Value::as_u64).ok_or("a host-issued file or directory handle is required")?;
    let entry = loaded(plugin).and_then(|p| p.borrow().as_ref().and_then(|p| p.image(handle).cloned())).ok_or("that handle is no longer available to this plugin")?;
    match entry.origin {
        ImageOrigin::Directory(root) if directory => Ok((root, entry.path)),
        ImageOrigin::File(root) if !directory => Ok((root, entry.path)),
        _ => Err("that handle is not the requested kind of file or directory".into()),
    }
}

fn job(plugin: &str, request: &Request, cx: &App) -> Result<Job, String> {
    let p = &request.payload;
    let (root, operation) = match request.operation.as_str() {
        "entries" => {
            if p.contains_key("folder") && p.contains_key("directory") { return Err("choose a folder or a directory handle".into()); }
            let (root, relative) = if p.contains_key("directory") { handle(plugin, p.get("directory"), true)? }
                else { (granted_folder(plugin, p, cx).ok_or("no such granted folder")?, PathBuf::new()) };
            let offset = number(p, "offset", 0, plugin_files::MAX_SCAN as u64)? as usize;
            let limit = number(p, "limit", plugin_files::MAX_PAGE as u64, plugin_files::MAX_PAGE as u64)? as usize;
            if limit == 0 { return Err("`limit` must be positive".into()); }
            (root, Operation::Entries { relative, offset, limit })
        }
        "read" => {
            let (root, relative) = handle(plugin, p.get("file"), false)?;
            let offset = number(p, "offset", 0, u64::MAX)?;
            let limit = number(p, "limit", plugin_files::MAX_READ_BYTES as u64, plugin_files::MAX_READ_BYTES as u64)? as usize;
            if limit == 0 { return Err("`limit` must be positive".into()); }
            (root, Operation::Read { relative, offset, limit })
        }
        _ => return Err("unknown file operation".into()),
    };
    Ok(Job { plugin: plugin.to_string(), id: request.id, root, operation })
}

pub(super) fn run(plugin: &str, request: &Request, cx: &mut App) -> Option<Result<Value, String>> {
    let job = match job(plugin, request, cx) { Ok(job) => job, Err(error) => return Some(Err(error)) };
    if QUEUE.with(|q| q.borrow_mut().push(job).is_err()) { return Some(Err("the file request queue is full".into())); }
    pump(cx);
    None
}

fn wanted(job: &Job, cx: &App) -> bool {
    live_access(&job.plugin, cx).is_some_and(|access| access.allows(&Capability::FilesRead) && access.allows_folder(&job.root))
        && loaded(&job.plugin).is_some_and(|p| p.borrow().as_ref().is_some_and(|p| p.is_open(job.id)))
}

fn work(job: &Job) -> Result<Answer, String> {
    let folder = Folder::open(&job.root)?;
    match &job.operation {
        Operation::Entries { relative, offset, limit } => folder.entries(relative, *offset, *limit).map(Answer::Entries),
        Operation::Read { relative, offset, limit } => {
            let r = folder.read(relative, *offset, *limit)?;
            Ok(Answer::Read(json!({"encoding":"base64","body":mandara_services::plugin_net::encode_bytes(&r.bytes),"size":r.size,"offset":r.offset,"next_offset":r.next_offset,"eof":r.eof}).to_string()))
        }
    }
}

fn page_reply(page: plugin_files::Page, mut issue: impl FnMut(PathBuf, plugin_files::Kind) -> Option<u64>) -> Result<Value, String> {
    let entries: Option<Vec<_>> = page.entries.into_iter().map(|entry| {
        let handle = issue(entry.relative, entry.kind)?;
        let kind = match entry.kind { plugin_files::Kind::File => "file", plugin_files::Kind::Directory => "directory" };
        Some(json!({"name":entry.name,"kind":kind,"handle":handle,"size":entry.size}))
    }).collect();
    let entries = entries.ok_or("no image handles are left")?;
    Ok(json!({"total":page.total,"offset":page.offset,"truncated":page.truncated,"items":entries}))
}

fn pump(cx: &mut App) {
    loop {
        let job = next_job();
        let Some(job) = job else { return };
        if !wanted(&job, cx) { QUEUE.with(|q| q.borrow_mut().complete()); continue; }
        ACTIVE.with(|active| active.borrow_mut().push(job.plugin.clone()));
        cx.spawn(async move |cx| {
            let (job, answer) = mandara_services::runtime::unblock(move || { let answer = work(&job); (job, answer) }).await;
            cx.update(|cx| {
                QUEUE.with(|q| q.borrow_mut().complete());
                ACTIVE.with(|active| {
                    let mut active = active.borrow_mut();
                    active.retain(|plugin| *plugin != job.plugin);
                    if active.is_empty() { active.shrink_to_fit(); }
                });
                if wanted(&job, cx) {
                    match answer {
                        Ok(Answer::Read(reply)) => deliver_raw(&job.plugin, job.id, Ok(reply), cx),
                        Ok(Answer::Entries(page)) => {
                            let reply = page_reply(page, |relative, kind| {
                                let origin = match kind {
                                    plugin_files::Kind::File => ImageOrigin::File(job.root.clone()),
                                    plugin_files::Kind::Directory => ImageOrigin::Directory(job.root.clone()),
                                };
                                issue(&job.plugin, relative, origin)
                            });
                            deliver(&job.plugin, job.id, reply, cx);
                        }
                        Err(error) => deliver(&job.plugin, job.id, Err(error), cx),
                    }
                }
                pump(cx);
            });
        }).detach();
    }
}

fn next_job() -> Option<Job> {
    ACTIVE.with(|active| QUEUE.with(|queue| queue.borrow_mut().next_job_where(|job| !active.borrow().contains(&job.plugin))))
}

pub(super) fn stop(plugin: &str, ids: Option<&[u64]>) {
    QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        q.retain(|job| job.plugin != plugin || ids.is_some_and(|ids| !ids.contains(&job.id)));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn one_plugin_cannot_fill_every_worker_or_block_another_plugins_queue() {
        let job = |plugin: &str| Job { plugin: plugin.into(), id: 1, root: PathBuf::new(), operation: Operation::Read { relative: "file".into(), offset: 0, limit: 1 } };
        QUEUE.with(|queue| { for plugin in ["stalled", "stalled", "other"] { assert!(queue.borrow_mut().push(job(plugin)).is_ok()); } });
        let first = next_job().unwrap();
        ACTIVE.with(|active| active.borrow_mut().push(first.plugin));
        assert_eq!(next_job().unwrap().plugin, "other");
        assert!(next_job().is_none());
        QUEUE.with(|queue| { let mut queue = queue.borrow_mut(); queue.complete(); queue.complete(); queue.retain(|_| false); });
        ACTIVE.with(|active| active.borrow_mut().clear());
    }

    #[test]
    fn a_page_never_silently_skips_an_entry_without_a_handle() {
        let page = || plugin_files::Page { total: 2, offset: 4, truncated: true, entries: vec![
            plugin_files::Entry { name: "one".into(), relative: "one".into(), kind: plugin_files::Kind::File, size: 1 },
            plugin_files::Entry { name: "two".into(), relative: "two".into(), kind: plugin_files::Kind::File, size: 2 },
        ] };
        let reply = page_reply(page(), |_, _| Some(10)).unwrap();
        assert_eq!(reply["items"].as_array().unwrap().len(), 2);
        assert_eq!(reply["offset"], 4);
        assert!(page_reply(page(), |path, _| (path == Path::new("one")).then_some(10)).is_err());
    }

    #[test]
    fn file_reply_keeps_arbitrary_bytes_and_never_returns_a_path() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("data"), [0, 255, 128]).unwrap();
        let job = Job { plugin: "reader".into(), id: 1, root: root.path().into(), operation: Operation::Read { relative: "data".into(), offset: 1, limit: 2 } };
        let Answer::Read(reply) = work(&job).unwrap() else { panic!("expected file bytes") };
        let reply: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["body"], "/4A=");
        assert_eq!(reply["next_offset"], 3);
        assert_eq!(reply["eof"], true);
        assert!(!reply.to_string().contains(root.path().to_str().unwrap()));
        assert!(number(&serde_json::from_value(json!({"limit":-1})).unwrap(), "limit", 1, 10).is_err());
        assert!(number(&serde_json::from_value(json!({"limit":"10"})).unwrap(), "limit", 1, 10).is_err());
    }

    #[test]
    fn cancellation_removes_only_the_selected_plugins_queued_requests() {
        let job = |plugin: &str, id| Job { plugin: plugin.into(), id, root: PathBuf::new(), operation: Operation::Entries { relative: Path::new("").into(), offset: 0, limit: 1 } };
        QUEUE.with(|q| { for job in [job("a", 1), job("a", 2), job("b", 3)] { assert!(q.borrow_mut().push(job).is_ok()); } });
        stop("a", Some(&[1]));
        assert_eq!(QUEUE.with(|q| q.borrow_mut().next_job().unwrap().id), 2);
        QUEUE.with(|q| q.borrow_mut().complete());
        stop("a", None);
        stop("b", None);
        assert!(QUEUE.with(|q| q.borrow_mut().next_job().is_none()));
    }
}
