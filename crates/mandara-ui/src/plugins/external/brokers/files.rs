//! Scoped file requests share one bounded blocking worker.

use std::cell::RefCell;
use std::path::PathBuf;

use gpui::App;
use mandara_plugins::extension::Capability;
use mandara_plugins::wasm::{ImageOrigin, Request};
use mandara_services::plugin_files::{self, Folder, ScopedFiles as _};
use serde_json::{Value, json};

use super::{deliver, deliver_raw, granted_folder, issue, live_access, loaded};

/// A single directory scan runs at a time, with bounded requests waiting behind it.
const MAX_QUEUED: usize = 32;

enum Operation { Entries { relative: PathBuf, offset: usize, limit: usize }, Read { relative: PathBuf, offset: u64, limit: usize } }
struct Job { plugin: String, id: u64, root: PathBuf, operation: Operation }
enum Answer { Entries(plugin_files::Page), Read(String) }

thread_local! {
    static QUEUE: RefCell<super::queue::Queue<Job, 1, MAX_QUEUED>> = const { RefCell::new(super::queue::Queue::new()) };
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

fn pump(cx: &mut App) {
    loop {
        let job = QUEUE.with(|q| q.borrow_mut().next_job());
        let Some(job) = job else { return };
        if !wanted(&job, cx) { QUEUE.with(|q| q.borrow_mut().complete()); continue; }
        cx.spawn(async move |cx| {
            let (job, answer) = mandara_services::runtime::unblock(move || { let answer = work(&job); (job, answer) }).await;
            cx.update(|cx| {
                QUEUE.with(|q| q.borrow_mut().complete());
                if wanted(&job, cx) {
                    match answer {
                        Ok(Answer::Read(reply)) => deliver_raw(&job.plugin, job.id, Ok(reply), cx),
                        Ok(Answer::Entries(page)) => {
                            let entries: Vec<_> = page.entries.into_iter().filter_map(|entry| {
                                let (kind, origin) = match entry.kind {
                                    plugin_files::Kind::File => ("file", ImageOrigin::File(job.root.clone())),
                                    plugin_files::Kind::Directory => ("directory", ImageOrigin::Directory(job.root.clone())),
                                };
                                let handle = issue(&job.plugin, entry.relative, origin)?;
                                Some(json!({"name":entry.name,"kind":kind,"handle":handle,"size":entry.size}))
                            }).collect();
                            deliver(&job.plugin, job.id, Ok(json!({"total":page.total,"offset":page.offset,"truncated":page.truncated,"items":entries})), cx);
                        }
                        Err(error) => deliver(&job.plugin, job.id, Err(error), cx),
                    }
                }
                pump(cx);
            });
        }).detach();
        return;
    }
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
