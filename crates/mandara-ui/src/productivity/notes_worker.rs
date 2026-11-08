//! One ordered owner of the Markdown store, with shared read snapshots.

use async_channel::{Receiver, Sender};
use crate::notes_store::{NoteStore, NotesSnapshot};

/// Bounded intents keep a stalled disk from retaining an unbounded edit queue.
pub(super) const QUEUED_EDITS: usize = 32;

/// What one queued edit produced, sent back once it ran on the worker.
pub(super) struct Completion<T> {
    /// The store's view right after this edit (shared bodies, no copies).
    pub snapshot: NotesSnapshot,
    pub value: anyhow::Result<T>,
    /// Drafts kept back as body conflicts by the pending writes that ran
    /// ahead of this edit (the edit itself still ran).
    pub conflicts: Vec<u64>,
    /// Separate metadata failure lets an editor retain a newly committed id.
    pub index_error: Option<String>,
}

/// A successful body write can still carry an index failure and a usable id.
pub struct NoteReply<T> {
    /// The operation value remains available when content committed before metadata failed.
    pub value: Result<T, String>,
    /// Separate metadata failure lets an editor retain a newly committed id.
    pub index_error: Option<String>,
}

impl<T> NoteReply<T> {
    /// Ordinary callers get success only when both operation and metadata succeeded.
    pub fn result(self) -> Result<T, String> {
        let value = self.value?;
        self.index_error.map_or(Ok(value), Err)
    }
}

/// A type-erased queued edit, so one bounded queue carries every reply type.
trait Edit: Send {
    /// Runs on the worker's blocking thread with exclusive use of the store.
    fn run(self: Box<Self>, store: &mut NoteStore);
}

/// An edit for the notes folder `root`, with the channel its completion
/// goes to and the change signal a newly opened folder's watcher uses.
struct EditWithReply<F, T> {
    edit: F,
    reply: Sender<Completion<T>>,
    root: std::path::PathBuf,
    changed: Sender<()>,
}

impl<F, T> Edit for EditWithReply<F, T>
where
    F: FnOnce(&mut NoteStore) -> anyhow::Result<T> + Send,
    T: Send,
{
    /// Opens `root` first when the store holds another folder (flushing the
    /// old index; a failure replies without running the edit), then runs
    /// the edit and retries the index. The reply never blocks: its channel
    /// holds exactly one completion.
    fn run(self: Box<Self>, store: &mut NoteStore) {
        let Self { edit, reply, root, changed } = *self;
        if store.root != root {
            let replacement = (|| {
                store.prepare_leave()?;
                let mut replacement = NoteStore::open(&root);
                replacement.watch_changes(changed)?;
                replacement.index_result()?;
                Ok::<_, anyhow::Error>(replacement)
            })();
            match replacement {
                Ok(replacement) => *store = replacement,
                Err(error) => {
                    let _ = reply.try_send(Completion { snapshot: store.snapshot(), value: Err(error), conflicts: Vec::new(), index_error: None });
                    return;
                }
            }
        }
        let value = edit(store);
        let conflicts = std::mem::take(&mut store.deferred_conflicts);
        // A retry also repairs metadata after a previous partial body/rename write.
        let index_error = store.persist_index().err().map(|error| format!("{error:#}"));
        let _ = reply.try_send(Completion { snapshot: store.snapshot(), value, conflicts, index_error });
    }
}

/// The handle the panel model keeps: dropping it closes the queue, and the
/// worker task ends after the edits already queued.
pub(super) struct NotesWorker {
    edits: Sender<Box<dyn Edit>>,
}

impl NotesWorker {
    /// Owns the store on the shared runtime; blocking jobs are dispatched separately.
    pub fn start(store: NoteStore) -> Self {
        let (edits, incoming) = async_channel::bounded::<Box<dyn Edit>>(QUEUED_EDITS);
        mandara_services::runtime::spawn("notes-store", async move {
            let mut store = store;
            while let Ok(edit) = incoming.recv().await {
                store = mandara_services::runtime::unblock(move || {
                    edit.run(&mut store);
                    store
                }).await;
            }
        });
        Self { edits }
    }

    /// Enqueues a root-scoped intent, refusing excess work before retaining its payload.
    pub fn submit<T: Send + 'static>(
        &self,
        root: std::path::PathBuf,
        changed: Sender<()>,
        edit: impl FnOnce(&mut NoteStore) -> anyhow::Result<T> + Send + 'static,
    ) -> Result<Receiver<Completion<T>>, String> {
        let (reply, result) = async_channel::bounded(1);
        self.edits.try_send(Box::new(EditWithReply { edit, reply, root, changed }))
            .map_err(|error| match error {
                async_channel::TrySendError::Full(_) => mandara_i18n::t("Busy").to_owned(),
                async_channel::TrySendError::Closed(_) => mandara_i18n::t("Unavailable").to_owned(),
            })?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use crate::notes_store::{DEFAULT_TOPIC, INDEX_FILE};

    /// Slow CI still has a deadline; no held worker can strand the test suite.
    const TEST_TIMEOUT: Duration = Duration::from_secs(15);

    fn finish<T: Send + 'static>(pending: Receiver<Completion<T>>) -> Completion<T> {
        let (reply, result) = std::sync::mpsc::channel();
        mandara_services::runtime::spawn("notes-test-reply", async move {
            let _ = reply.send(pending.recv().await.unwrap());
        });
        result.recv_timeout(TEST_TIMEOUT).expect("bounded completion")
    }

    #[test]
    fn slow_io_does_not_run_on_the_caller_and_append_move_delete_stay_ordered() {
        let root = tempfile::tempdir().unwrap();
        let mut store = NoteStore::open(root.path());
        let id = store.create(DEFAULT_TOPIC, "Original").unwrap();
        let topic = store.add_topic("Work", None, None).unwrap();
        let worker = NotesWorker::start(store);
        let (changed, _) = async_channel::bounded(1);
        let (entered, waiting) = std::sync::mpsc::channel();
        let (release, held) = std::sync::mpsc::channel();
        let caller = std::thread::current().id();
        let first = worker.submit(root.path().into(), changed.clone(), move |store| {
            entered.send(std::thread::current().id()).unwrap();
            held.recv_timeout(TEST_TIMEOUT).unwrap();
            store.write(id, "Original\nFirst")
        }).unwrap();
        assert_ne!(waiting.recv_timeout(TEST_TIMEOUT).unwrap(), caller);
        let append = worker.submit(root.path().into(), changed.clone(), move |store| {
            let text = format!("{}\nSecond", store.note(id).unwrap().text);
            store.write(id, &text)
        }).unwrap();
        let moved = worker.submit(root.path().into(), changed.clone(), move |store| store.move_note(id, topic)).unwrap();
        let deleted = worker.submit(root.path().into(), changed, move |store| store.delete(id)).unwrap();
        assert!(append.try_recv().is_err(), "a queued mutation cannot pass a held write");
        release.send(()).unwrap();
        assert!(finish(first).value.is_ok());
        let after_append = finish(append);
        assert_eq!(after_append.snapshot.note(id).unwrap().text, "Original\nFirst\nSecond");
        let after_move = finish(moved);
        let path = after_move.snapshot.note(id).unwrap().path.as_ref().unwrap();
        assert!(path.starts_with("Work/"));
        let after_delete = finish(deleted);
        assert!(after_delete.value.unwrap());
        assert!(after_delete.snapshot.note(id).is_none());
        assert!(!root.path().join(path).exists());
    }

    #[test]
    fn stalled_writes_have_a_bounded_queue_and_partial_index_errors_keep_the_id() {
        let root = tempfile::tempdir().unwrap();
        let store = NoteStore::open(root.path());
        let worker = NotesWorker::start(store);
        let (changed, _) = async_channel::bounded(1);
        let (entered, waiting) = std::sync::mpsc::channel();
        let (release, held) = std::sync::mpsc::channel();
        let first = worker.submit(root.path().into(), changed.clone(), move |_| {
            entered.send(()).unwrap();
            held.recv_timeout(TEST_TIMEOUT).unwrap();
            Ok(())
        }).unwrap();
        waiting.recv_timeout(TEST_TIMEOUT).unwrap();
        let mut queued = Vec::new();
        for _ in 0..QUEUED_EDITS {
            queued.push(worker.submit(root.path().into(), changed.clone(), |_| Ok(())).unwrap());
        }
        assert!(worker.submit(root.path().into(), changed.clone(), |_| Ok(())).is_err());
        release.send(()).unwrap();
        assert!(finish(first).value.is_ok());
        for pending in queued { assert!(finish(pending).value.is_ok()); }
        std::fs::remove_file(root.path().join(INDEX_FILE)).unwrap();
        std::fs::create_dir(root.path().join(INDEX_FILE)).unwrap();
        let created = finish(worker.submit(root.path().into(), changed.clone(), |store| store.create(DEFAULT_TOPIC, "Retained content")).unwrap());
        let id = created.value.unwrap();
        assert!(created.index_error.is_some());
        assert_eq!(created.snapshot.note(id).unwrap().text, "Retained content");
        std::fs::remove_dir(root.path().join(INDEX_FILE)).unwrap();
        let retry = finish(worker.submit(root.path().into(), changed, move |store| store.settle(id)).unwrap());
        assert!(retry.value.unwrap());
        assert!(retry.index_error.is_none());
        assert_eq!(NoteStore::open(root.path()).notes.len(), 1);
    }

    #[test]
    fn identical_ids_in_different_roots_never_share_a_write() {
        let root = tempfile::tempdir().unwrap();
        let first_root = root.path().join("first");
        let second_root = root.path().join("second");
        let mut first_store = NoteStore::open(&first_root);
        let first_id = first_store.create(DEFAULT_TOPIC, "First folder").unwrap();
        let mut second_store = NoteStore::open(&second_root);
        let second_id = second_store.create(DEFAULT_TOPIC, "Second folder").unwrap();
        assert_eq!(first_id, second_id);
        let worker = NotesWorker::start(first_store);
        let (changed, _) = async_channel::bounded(1);
        let second = finish(worker.submit(second_root.clone(), changed.clone(), move |store| store.write(second_id, "Second changed")).unwrap());
        assert!(second.value.is_ok());
        let first = finish(worker.submit(first_root.clone(), changed, move |store| store.write(first_id, "First changed")).unwrap());
        assert!(first.value.is_ok());
        assert_eq!(NoteStore::open(&first_root).note(first_id).unwrap().text, "First changed");
        assert_eq!(NoteStore::open(&second_root).note(second_id).unwrap().text, "Second changed");
    }

    #[test]
    fn a_folder_switch_waits_for_failed_index_metadata_to_be_retried() {
        let root = tempfile::tempdir().unwrap();
        let first_root = root.path().join("first");
        let second_root = root.path().join("second");
        let mut store = NoteStore::open(&first_root);
        let id = store.create(DEFAULT_TOPIC, "Keep the id").unwrap();
        std::fs::remove_file(first_root.join(INDEX_FILE)).unwrap();
        std::fs::create_dir(first_root.join(INDEX_FILE)).unwrap();
        store.recolor_topic(DEFAULT_TOPIC, "warning").unwrap();
        assert!(store.index_result().is_err());
        let worker = NotesWorker::start(store);
        let (changed, _) = async_channel::bounded(1);
        let refused = finish(worker.submit(second_root.clone(), changed.clone(), |_| Ok(())).unwrap());
        assert!(refused.value.is_err());
        assert_eq!(refused.snapshot.root, first_root);
        assert!(!second_root.exists());
        std::fs::remove_dir(first_root.join(INDEX_FILE)).unwrap();
        let retried = finish(worker.submit(second_root.clone(), changed, |_| Ok(())).unwrap());
        assert!(retried.value.is_ok());
        assert_eq!(retried.snapshot.root, second_root);
        let reopened = NoteStore::open(&first_root);
        assert_eq!(reopened.note(id).unwrap().text, "Keep the id");
        assert_eq!(reopened.topic(DEFAULT_TOPIC).unwrap().color, "warning");
    }
}
