//! UI snapshots, dirty generations and ordered Markdown completion.

use std::sync::Arc;
use gpui::Context;
use crate::notes_store::{Note, NoteStore, NotesSnapshot, body_hash};
use crate::todo_lists::parse_quick_add;
use crate::state::config;
use super::{Productivity, NoteReply, SAVE_DEBOUNCE, notes_worker, notes_root, now_unix};
use super::note_edit::{BodyConflicts, PendingNote, TaskPatches};
use super::note_conflict::{Recovery, RetainedDraft};

struct NotesCompletion {
    sequence: u64,
    snapshot: NotesSnapshot,
    acknowledged: Vec<(u64, u64)>,
    successful: bool,
    error: Option<String>,
    /// Drafts the worker kept back because their file changed elsewhere
    /// ([`BodyConflicts`]); the batch's other bodies were written.
    conflicts: Vec<u64>,
}

impl Productivity {
    /// Debounces live typing; an explicit operation can flush sooner.
    fn notes_changed(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        self.notes_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            let _ = this.update(cx, |p, cx| {
                if let Err(error) = p.flush_notes(cx) { tracing::warn!("saving notes: {error}"); }
            });
        }));
    }

    /// Applies the shared snapshot before replying and rejects stale completion order.
    fn queue_notes<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        acknowledged: Vec<(u64, u64)>,
        edit: impl FnOnce(&mut NoteStore) -> anyhow::Result<T> + Send + 'static,
    ) -> Result<async_channel::Receiver<NoteReply<T>>, String> {
        let worker = self.notes_worker.as_ref().ok_or_else(|| mandara_i18n::t("Loading…").to_owned())?;
        let incoming = worker.submit(self.notes.root.clone(), self.notes_changes.clone(), edit)?;
        Ok(self.observe_notes(cx, acknowledged, incoming))
    }

    /// All accepted intents use the same snapshot, ordering and reply boundary.
    fn observe_notes<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        acknowledged: Vec<(u64, u64)>,
        incoming: async_channel::Receiver<notes_worker::Completion<T>>,
    ) -> async_channel::Receiver<NoteReply<T>> {
        self.notes_sequence += 1;
        let sequence = self.notes_sequence;
        let (reply, result) = async_channel::bounded(1);
        cx.spawn(async move |this, cx| {
            let Ok(done) = incoming.recv().await else { return };
            let notes_worker::Completion { snapshot, value, conflicts: deferred, index_error } = done;
            // Body conflicts leave the batch's other writes done; they are
            // shown as retained conflicts, not as a failed save.
            let failed_on_conflict = value.as_ref().err().and_then(|error| error.downcast_ref::<BodyConflicts>());
            let mut conflicts = failed_on_conflict.map(|c| c.0.clone()).unwrap_or_default();
            conflicts.extend(deferred);
            let successful = (value.is_ok() || failed_on_conflict.is_some()) && index_error.is_none();
            let _ = this.update(cx, |p, cx| {
                let error = value.as_ref().err().filter(|error| !error.is::<BodyConflicts>()).map(|error| format!("{error:#}")).or_else(|| index_error.clone());
                if p.apply_notes_completion(NotesCompletion { sequence, snapshot, acknowledged, successful, error, conflicts }) { cx.notify(); }
                p.follow_notes_completion(cx);
            });
            let _ = reply.try_send(NoteReply { value: value.map_err(|error| format!("{error:#}")), index_error });
        }).detach();
        result
    }

    /// Error status and snapshots share the same completion sequence boundary.
    fn apply_notes_completion(&mut self, update: NotesCompletion) -> bool {
        let NotesCompletion { sequence, mut snapshot, mut acknowledged, successful, error, conflicts } = update;
        if !conflicts.is_empty() {
            // Each conflict belongs to the draft generation this save sent.
            let attempts: Vec<(u64, u64)> = acknowledged.iter().filter(|(id, _)| conflicts.contains(id)).copied().collect();
            acknowledged.retain(|(id, _)| !conflicts.contains(id));
            self.retain_body_conflicts(&attempts, &snapshot);
        }
        if successful && sequence >= self.notes_applied {
            for &(id, generation) in &acknowledged {
                self.notes_conflicts.retain(|(root, note), draft| root != &self.notes.root || *note != id || draft.generation != generation);
            }
        }
        acknowledge(&mut self.notes_dirty, &mut self.notes_submitted, acknowledged, successful, |id| { self.notes_tasks.remove(&id); self.notes_base.remove(&id); });
        if sequence < self.notes_applied { return false; }
        self.retain_conflicts(&snapshot);
        self.notes_error = error;
        if snapshot.root == self.notes.root || self.notes_dirty.is_empty() {
            let recovered: Vec<_> = self.notes_conflicts.iter().filter(|((root, _), draft)| root == &snapshot.root && draft.original.is_some())
                .filter_map(|((_, id), draft)| { let recovery = draft.recovery(&snapshot); (recovery != Recovery::Waiting).then_some((*id, recovery)) }).collect();
            let drafts = self.notes.notes.iter().filter(|draft| self.notes_dirty.contains_key(&draft.id))
                .chain(self.notes_conflicts.iter().filter(|((root, _), _)| root == &snapshot.root).map(|(_, draft)| &draft.note));
            for draft in drafts {
                if let Some(note) = snapshot.notes.iter_mut().find(|note| note.id == draft.id) {
                    if note.path == draft.path && note.topic == draft.topic { *note = draft.clone(); }
                    else { *note = Arc::new(Note { id: note.id, topic: note.topic, path: note.path.clone(), text: draft.text.clone(), updated: draft.updated }); }
                } else { snapshot.notes.push(draft.clone()); }
            }
            // Ids belong to their folder; nothing is dirty when it changes.
            if snapshot.root != self.notes.root { self.notes_base.clear(); }
            self.notes = snapshot;
            for (id, recovery) in recovered {
                let key = (self.notes.root.clone(), id);
                match recovery {
                    Recovery::Retry => {
                        if let Some(draft) = self.notes_conflicts.remove(&key) {
                            self.notes_dirty.insert(id, draft.generation);
                            if let Some(tasks) = draft.tasks { self.notes_tasks.insert(id, tasks); }
                        }
                    }
                    Recovery::Conflict => { if let Some(draft) = self.notes_conflicts.get_mut(&key) { draft.original = None; } }
                    Recovery::Waiting => {}
                }
            }
            if self.notes_topic.is_some_and(|topic| self.notes.topic(topic).is_none()) { self.notes_topic = None; }
        }
        self.notes_applied = sequence;
        true
    }

    /// A stale checkbox or removed file needs user attention, not repeated prerequisite writes.
    fn retain_conflicts(&mut self, snapshot: &NotesSnapshot) {
        if snapshot.root != self.notes.root { return; }
        let conflicts: Vec<_> = self.notes_dirty.iter().filter_map(|(&id, &generation)| {
            let draft = self.notes.notes.iter().find(|note| note.id == id)?;
            let current = snapshot.note(id);
            let removed = current.is_none() && draft.path.is_some();
            let changed_markers = self.notes_tasks.get(&id).is_some_and(|patches| {
                patches.iter().any(|(&start, &(end, _))| !matches!(current.and_then(|note| note.text.get(start..end)), Some("[ ]" | "[x]" | "[X]")))
            });
            let unreadable = draft.path.as_deref().and_then(|path| snapshot.read_error(path));
            (removed || changed_markers || unreadable.is_some()).then(|| (id, generation, draft.clone(), unreadable.and_then(|error| error.retained.clone())))
        }).collect();
        for (id, generation, note, original) in conflicts {
            self.notes_dirty.remove(&id);
            self.notes_submitted.remove(&id);
            let tasks = self.notes_tasks.remove(&id);
            self.rebase_conflict(id, snapshot);
            self.notes_conflicts.insert((self.notes.root.clone(), id), RetainedDraft { generation, note, tasks, original });
        }
    }

    /// Whole-text drafts whose file changed elsewhere stay visible as
    /// conflicts instead of overwriting it, and stop being write
    /// prerequisites for the other notes.
    fn retain_body_conflicts(&mut self, attempts: &[(u64, u64)], snapshot: &NotesSnapshot) {
        if snapshot.root != self.notes.root { return; }
        for &(id, attempt) in attempts {
            // A newer draft typed since this save stays dirty with its own
            // base; its own save is checked again, never rebased to this
            // older snapshot.
            if self.notes_dirty.get(&id) != Some(&attempt) { continue; }
            let Some(generation) = self.notes_dirty.remove(&id) else { continue };
            self.notes_submitted.remove(&id);
            self.notes_tasks.remove(&id);
            self.rebase_conflict(id, snapshot);
            let Some(note) = self.notes.notes.iter().find(|note| note.id == id).cloned() else { continue };
            self.notes_conflicts.insert((self.notes.root.clone(), id), RetainedDraft { generation, note, tasks: None, original: None });
        }
    }

    /// A conflict's draft replaces the body found elsewhere once you edit
    /// it again, so a conflict never locks the note; a further outside
    /// change before that edit is a new conflict.
    fn rebase_conflict(&mut self, id: u64, snapshot: &NotesSnapshot) {
        match snapshot.note(id) {
            Some(current) => { self.notes_base.insert(id, body_hash(&current.text)); }
            None => { self.notes_base.remove(&id); }
        }
    }

    /// Conflicts remain visible after unrelated operations or leaving and returning to a folder.
    pub(crate) fn has_note_conflicts(&self) -> bool { self.notes_conflicts.keys().any(|(root, _)| root == &self.notes.root) }

    /// Drain a latest dirty model only after every already-submitted ordering barrier.
    fn follow_notes_completion(&mut self, cx: &mut Context<Self>) {
        self.retry_notes_rescan(cx);
        let barrier = self.notes_applied != self.notes_sequence;
        if self.notes_flush.should_flush(self.notes_error.is_none(), barrier, !self.notes_dirty.is_empty()) {
            if let Err(error) = self.flush_notes(cx) { tracing::warn!("saving notes: {error}"); }
        } else if !barrier && self.notes_dirty.is_empty() && !self.notes_switching {
            let desired = notes_root(&config(cx).read(cx).snapshot());
            if desired != self.notes.root { self.ensure_notes_root(desired, cx); }
        }
    }

    /// Shares current dirty bodies and preserves their original edit kind.
    fn pending_notes(&self, except: Option<u64>, unsubmitted_only: bool) -> Vec<PendingNote> {
        self.notes_dirty.iter()
            .filter(|&(&id, &generation)| except != Some(id) && (!unsubmitted_only || self.notes_submitted.get(&id) != Some(&generation)))
            .filter_map(|(&id, &generation)| self.notes.notes.iter().find(|note| note.id == id).map(|note| PendingNote { note: note.clone(), generation, tasks: self.notes_tasks.get(&id).cloned(), base: self.notes_base.get(&id).copied() }))
            .collect()
    }

    /// Enqueues the latest dirty bodies before the next ordered operation.
    /// Dirty generations stay retained until their own disk write succeeds.
    pub fn flush_notes(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        self.retry_notes_rescan(cx);
        if self.notes_flush.running() { return Ok(()); }
        let pending = self.pending_notes(None, true);
        if pending.is_empty() { return Ok(()); }
        let acknowledged: Vec<_> = pending.iter().map(PendingNote::token).collect();
        let submitted = acknowledged.clone();
        let result = self.queue_notes(cx, acknowledged, move |store| write_pending(store, &pending))?;
        self.notes_submitted.extend(submitted);
        self.notes_flush.begin();
        cx.spawn(async move |this, cx| {
            let result = match result.recv().await { Ok(done) => done.result(), Err(error) => Err(error.to_string()) };
            let _ = this.update(cx, |p, cx| {
                p.notes_flush.finish();
                if let Err(error) = result { tracing::warn!("saving notes: {error}"); }
                p.follow_notes_completion(cx);
            });
        }).detach();
        Ok(())
    }

    /// A completion reflects filesystem and index results, including partial writes.
    pub fn edit_notes<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut NoteStore) -> anyhow::Result<T> + Send + 'static,
    ) -> Result<async_channel::Receiver<NoteReply<T>>, String> {
        let pending = self.pending_notes(None, false);
        let acknowledged = pending.iter().map(PendingNote::token).collect();
        self.queue_notes(cx, acknowledged, move |store| pending_then(store, &pending, edit))
    }

    /// Resolves quick-add topics on the worker, returning the committed note id.
    pub fn add_note(&mut self, text: &str, topic: u64, cx: &mut Context<Self>) -> Result<async_channel::Receiver<NoteReply<u64>>, String> {
        let text = text.to_owned();
        self.edit_notes(cx, move |store| {
            let (tag, body) = parse_quick_add(&text);
            let topic = match tag {
                Some(tag) => match store.find_topic(tag) { Some(id) => id, None => store.add_topic(tag, None, None)? },
                None => topic,
            };
            store.create(topic, body)
        })
    }

    /// Checkbox intentions apply to the worker's current body, preserving earlier appends.
    pub fn toggle_note_task(&mut self, id: u64, range: std::ops::Range<usize>, cx: &mut Context<Self>) {
        let next = self.notes.note(id).and_then(|note| crate::markdown::toggle_task(&note.text, range.clone()));
        let Some(next) = next else { return };
        let checked = next.get(range.clone()) == Some("[x]");
        let tasks = self.take_task_patches(id);
        self.edit_note(id, &next, cx);
        if let Some(mut tasks) = tasks {
            tasks.insert(range.start, (range.end, checked));
            self.notes_tasks.insert(id, tasks);
        }
        if let Err(error) = self.flush_notes(cx) { tracing::warn!("saving note checkbox: {error}"); }
    }

    /// A pending whole-body edit cannot be reduced to a checkbox-only intention.
    fn take_task_patches(&mut self, id: u64) -> Option<TaskPatches> {
        if self.notes_dirty.contains_key(&id) && !self.notes_tasks.contains_key(&id) { None }
        else { Some(self.notes_tasks.remove(&id).unwrap_or_default()) }
    }

    /// Live drafts share all unchanged entries with the worker's read snapshot.
    pub fn edit_note(&mut self, id: u64, text: &str, cx: &mut Context<Self>) {
        let Some(note) = self.notes.notes.iter_mut().find(|note| note.id == id) else { return };
        if note.text == text { return; }
        self.notes_conflicts.retain(|(root, note), _| root != &self.notes.root || *note != id);
        // The body this edit run replaces: what the panel showed when it
        // began (a conflict already set the body found elsewhere).
        if note.path.is_some() && !self.notes_base.contains_key(&id) { self.notes_base.insert(id, body_hash(&note.text)); }
        *note = Arc::new(Note { id, topic: note.topic, path: note.path.clone(), text: text.to_owned(), updated: now_unix() });
        self.notes_generation += 1;
        self.notes_dirty.insert(id, self.notes_generation);
        self.notes_tasks.remove(&id);
        self.notes_changed(cx);
    }

    /// A panel must not close a field that changed after its save request.
    pub fn note_pending(&self, id: u64) -> bool { self.notes_dirty.contains_key(&id) || self.notes_conflicts.keys().any(|(root, note)| root == &self.notes.root && *note == id) }

    /// Flushes before final filename/delete work and reports the real completion.
    pub fn settle_note(&mut self, id: u64, cx: &mut Context<Self>) -> Result<async_channel::Receiver<NoteReply<bool>>, String> {
        self.edit_notes(cx, move |store| store.settle(id))
    }

    /// Explicit deletion may discard this draft, while other dirty writes stay ordered.
    pub fn remove_note(&mut self, id: u64, cx: &mut Context<Self>) -> Result<async_channel::Receiver<NoteReply<bool>>, String> {
        let pending = self.pending_notes(Some(id), false);
        let mut acknowledged: Vec<_> = pending.iter().map(PendingNote::token).collect();
        let local_draft = self.notes_dirty.get(&id).copied().or_else(|| self.notes_conflicts.iter().find(|((root, note), _)| root == &self.notes.root && *note == id).map(|(_, draft)| draft.generation));
        if let Some(generation) = local_draft { acknowledged.push((id, generation)); }
        self.queue_notes(cx, acknowledged, move |store| pending_then(store, &pending, |store| store.delete(id).map(|removed| removed || local_draft.is_some())))
    }

    /// Detached UI actions still observe and report their eventual filesystem result.
    fn log_notes<T: Send + 'static>(&self, result: Result<async_channel::Receiver<NoteReply<T>>, String>, cx: &mut Context<Self>) {
        match result {
            Err(error) => tracing::warn!("notes: {error}"),
            Ok(result) => cx.spawn(async move |_, _| {
                if let Ok(done) = result.recv().await
                    && let Err(error) = done.result() { tracing::warn!("notes: {error}"); }
            }).detach(),
        }
    }

    /// Changes only the visible topic; stored Markdown and folders are untouched.
    pub fn set_notes_topic(&mut self, topic: Option<u64>, cx: &mut Context<Self>) {
        self.notes_topic = topic;
        cx.notify();
    }

    /// Rescans remain available when a failed file has a retained local draft.
    pub(super) fn rescan_notes(&mut self, cx: &mut Context<Self>) {
        self.notes_rescan.request();
        self.retry_notes_rescan(cx);
    }

    /// Reuse worker completions rather than timers to retry a full queue.
    fn retry_notes_rescan(&mut self, cx: &mut Context<Self>) {
        if self.notes_switching { return; }
        let Some(worker) = self.notes_worker.as_ref() else { return };
        let result = match self.notes_rescan.try_submit(worker, &self.notes.root, &self.notes_changes) {
            Ok(Some(incoming)) => Ok(self.observe_notes(cx, Vec::new(), incoming)),
            Ok(None) => return,
            Err(error) => Err(error),
        };
        self.log_notes(result, cx);
    }

    /// Waits for pending bodies/metadata before opening the configured folder.
    pub(super) fn ensure_notes_root(&mut self, root: std::path::PathBuf, cx: &mut Context<Self>) {
        if self.notes.root == root || self.notes_switching || !self.notes_ready { return; }
        if !self.notes_dirty.is_empty() {
            if let Err(error) = self.flush_notes(cx) { tracing::warn!("saving notes before changing folder: {error}"); }
            return;
        }
        if self.notes_applied != self.notes_sequence { return; }
        let changed = self.notes_changes.clone();
        let result = self.queue_notes(cx, Vec::new(), move |store| {
            store.prepare_leave()?;
            let mut replacement = NoteStore::open(&root);
            replacement.watch_changes(changed)?;
            replacement.index_result()?;
            *store = replacement;
            Ok(())
        });
        match result {
            Err(error) => tracing::warn!("changing notes folder: {error}"),
            Ok(result) => {
                self.notes_switching = true;
                cx.spawn(async move |this, cx| {
                    let result = result.recv().await;
                    let _ = this.update(cx, |p, cx| {
                        p.notes_switching = false;
                        if let Ok(done) = result {
                            match done.result() {
                                Err(error) => tracing::warn!("changing notes folder: {error}"),
                                Ok(()) if p.notes_dirty.is_empty() => {
                                    let desired = notes_root(&config(cx).read(cx).snapshot());
                                    p.ensure_notes_root(desired, cx);
                                }
                                Ok(()) => {}
                            }
                        }
                        p.retry_notes_rescan(cx);
                    });
                }).detach();
            }
        }
    }

}

/// A structural edit must not pass a failed prerequisite body or index write.
/// A draft whose file changed elsewhere does not stop the others: they are
/// written, and [`BodyConflicts`] names the drafts kept back.
fn write_pending(store: &mut NoteStore, pending: &[PendingNote]) -> anyhow::Result<()> {
    if store.scan_result().is_err() { store.rescan(&[]); store.scan_result()?; }
    let mut conflicts = Vec::new();
    for note in pending {
        match note.apply(store) {
            Err(error) if error.is::<BodyConflicts>() => conflicts.push(note.note.id),
            result => result?,
        }
    }
    store.persist_index()?;
    if conflicts.is_empty() { Ok(()) } else { Err(BodyConflicts(conflicts).into()) }
}

/// Writes the pending drafts, then runs `edit`. A draft whose file changed
/// elsewhere is kept back for its own note (handed to the completion through
/// the store) and never cancels this different edit; other write failures do.
fn pending_then<T>(store: &mut NoteStore, pending: &[PendingNote], edit: impl FnOnce(&mut NoteStore) -> anyhow::Result<T>) -> anyhow::Result<T> {
    if let Err(error) = write_pending(store, pending) {
        match error.downcast::<BodyConflicts>() {
            Ok(BodyConflicts(ids)) => store.deferred_conflicts.extend(ids),
            Err(error) => return Err(error),
        }
    }
    edit(store)
}

/// Only the matching successful generation clears a draft or its marker intentions.
fn acknowledge(
    dirty: &mut std::collections::BTreeMap<u64, u64>,
    submitted: &mut std::collections::BTreeMap<u64, u64>,
    tokens: Vec<(u64, u64)>,
    successful: bool,
    mut cleared: impl FnMut(u64),
) {
    for (id, generation) in tokens {
        if submitted.get(&id) == Some(&generation) { submitted.remove(&id); }
        if successful && dirty.get(&id) == Some(&generation) { dirty.remove(&id); cleared(id); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use crate::notes_store::DEFAULT_TOPIC;
    use super::super::PomodoroRules;

    fn model(root: std::path::PathBuf) -> Productivity {
        let (changed, _) = async_channel::bounded(1);
        Productivity::empty(PomodoroRules { focus: std::time::Duration::ZERO, short_break: std::time::Duration::ZERO, long_break: std::time::Duration::ZERO, long_every: 4, auto_continue: false }, root, changed)
    }

    fn unreadable_draft(checkbox: bool) -> (tempfile::TempDir, NoteStore, Productivity, u64, std::path::PathBuf, String) {
        let root = tempfile::tempdir().unwrap();
        let mut store = NoteStore::open(root.path());
        let id = store.create(DEFAULT_TOPIC, "- [ ] Original task").unwrap();
        let path = root.path().join(store.note(id).unwrap().path.as_ref().unwrap());
        let mut model = model(root.path().into());
        model.notes = store.snapshot();
        let mut draft = (*model.notes.notes[0]).clone();
        let desired = if checkbox { "- [x] Original task" } else { "My unsaved Markdown\nKeep every line" }.to_owned();
        draft.text = desired.clone();
        model.notes.notes[0] = Arc::new(draft);
        model.notes_dirty.insert(id, 3);
        if checkbox { model.notes_tasks.insert(id, BTreeMap::from([(2, (5, true))])); }
        std::fs::write(&path, [0xff, 0, 0xfe]).unwrap();
        store.rescan(&[]);
        assert!(write_pending(&mut store, &model.pending_notes(None, false)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), [0xff, 0, 0xfe]);
        model.apply_notes_completion(NotesCompletion { sequence: 1, snapshot: store.snapshot(), acknowledged: Vec::new(), successful: true, error: None, conflicts: Vec::new() });
        assert!(model.notes_dirty.is_empty());
        assert!(model.note_pending(id));
        assert!(write_pending(&mut store, &model.pending_notes(None, false)).is_ok());
        store.create(DEFAULT_TOPIC, "An unrelated note remains writable").unwrap();
        (root, store, model, id, path, desired)
    }

    #[test]
    fn unchanged_file_recovery_reconciles_and_retries_bodies_and_checkbox_intents() {
        for checkbox in [false, true] {
            let (_root, mut store, mut model, id, path, desired) = unreadable_draft(checkbox);
            std::fs::write(&path, "- [ ] Original task").unwrap();
            store.rescan(&[]);
            model.apply_notes_completion(NotesCompletion { sequence: 2, snapshot: store.snapshot(), acknowledged: Vec::new(), successful: true, error: None, conflicts: Vec::new() });
            assert!(!model.has_note_conflicts());
            assert_eq!(model.notes_dirty.get(&id), Some(&3));
            let pending = model.pending_notes(None, false);
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].tasks.is_some(), checkbox);
            write_pending(&mut store, &pending).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), desired);
            model.apply_notes_completion(NotesCompletion { sequence: 3, snapshot: store.snapshot(), acknowledged: vec![(id, 3)], successful: true, error: None, conflicts: Vec::new() });
            assert!(!model.note_pending(id));
        }
    }

    #[test]
    fn changed_file_recovery_keeps_both_external_content_and_the_unsaved_draft() {
        let (_root, mut store, mut model, id, path, desired) = unreadable_draft(false);
        let external = "# Recovered externally\nDo not replace this";
        std::fs::write(&path, external).unwrap();
        store.rescan(&[]);
        model.apply_notes_completion(NotesCompletion { sequence: 2, snapshot: store.snapshot(), acknowledged: Vec::new(), successful: true, error: None, conflicts: Vec::new() });
        assert!(model.has_note_conflicts());
        assert!(model.notes_dirty.is_empty());
        assert_eq!(model.notes.note(id).unwrap().text, desired);
        write_pending(&mut store, &model.pending_notes(None, false)).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), external);
        assert_eq!(store.note(id).unwrap().text, external);
        store.create(DEFAULT_TOPIC, "Another independent note").unwrap();
    }

    #[test]
    fn a_full_worker_queue_retains_one_recovery_scan_and_rechecks_the_original_body() {
        /// A held worker must not strand a failed test or the shared blocking pool.
        const WORKER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

        fn finish<T: Send + 'static>(pending: async_channel::Receiver<notes_worker::Completion<T>>) -> notes_worker::Completion<T> {
            let (reply, result) = std::sync::mpsc::channel();
            mandara_services::runtime::spawn("notes-rescan-test", async move {
                let _ = reply.send(pending.recv().await.unwrap());
            });
            result.recv_timeout(WORKER_TIMEOUT).expect("bounded worker completion")
        }

        for changed_externally in [false, true] {
            let (root, store, mut model, id, path, desired) = unreadable_draft(false);
            let worker = notes_worker::NotesWorker::start(store);
            let (changed, _) = async_channel::bounded(1);
            let (entered, waiting) = std::sync::mpsc::channel();
            let (release, held) = std::sync::mpsc::channel();
            let first = worker.submit(root.path().into(), changed.clone(), move |_| {
                entered.send(()).unwrap();
                held.recv_timeout(WORKER_TIMEOUT).unwrap();
                Ok(())
            }).unwrap();
            waiting.recv_timeout(WORKER_TIMEOUT).unwrap();
            let queued: Vec<_> = (0..notes_worker::QUEUED_EDITS)
                .map(|_| worker.submit(root.path().into(), changed.clone(), |_| Ok(())).unwrap()).collect();
            for _ in 0..4 {
                model.notes_rescan.request();
                assert!(model.notes_rescan.try_submit(&worker, root.path(), &changed).is_err());
            }
            let recovered = if changed_externally { "Changed while unreadable" } else { "- [ ] Original task" };
            std::fs::write(&path, recovered).unwrap();
            assert!(model.has_note_conflicts(), "readability alone must not clear the retained draft");
            assert!(model.pending_notes(None, false).is_empty());
            release.send(()).unwrap();
            assert!(finish(first).value.is_ok());
            for pending in queued { assert!(finish(pending).value.is_ok()); }

            let retry = model.notes_rescan.try_submit(&worker, root.path(), &changed).unwrap().expect("the missed event remains requested");
            assert!(model.notes_rescan.try_submit(&worker, root.path(), &changed).unwrap().is_none(), "events coalesce into one accepted scan");
            let scanned = finish(retry);
            assert!(scanned.value.is_ok());
            assert!(scanned.index_error.is_none());
            assert!(scanned.snapshot.read_errors.is_none());
            assert_eq!(scanned.snapshot.note(id).unwrap().text, recovered);
            model.apply_notes_completion(NotesCompletion { sequence: 2, snapshot: scanned.snapshot, acknowledged: Vec::new(), successful: true, error: None, conflicts: Vec::new() });
            assert_eq!(model.notes.note(id).unwrap().text, desired);
            assert_eq!(model.has_note_conflicts(), changed_externally);
            let pending = model.pending_notes(None, false);
            assert_eq!(pending.len(), usize::from(!changed_externally));
            let saved = finish(worker.submit(root.path().into(), changed, move |store| write_pending(store, &pending)).unwrap());
            assert!(saved.value.is_ok());
            assert_eq!(std::fs::read_to_string(path).unwrap(), if changed_externally { recovered } else { &desired });
        }
    }

    #[test]
    fn a_conflicting_patch_stays_recoverable_without_blocking_other_notes_or_folders() {
        let root = tempfile::tempdir().unwrap();
        let mut store = NoteStore::open(root.path());
        let id = store.create(DEFAULT_TOPIC, "- [ ] Original task").unwrap();
        let mut model = model(root.path().into());
        model.notes = store.snapshot();
        let mut draft = (*model.notes.notes[0]).clone();
        draft.text = "- [x] Original task".into();
        model.notes.notes[0] = Arc::new(draft);
        model.notes_dirty.insert(id, 1);
        model.notes_tasks.insert(id, BTreeMap::from([(2, (5, true))]));
        store.write(id, "Rewritten externally").unwrap();
        assert!(write_pending(&mut store, &model.pending_notes(None, false)).is_err());
        model.apply_notes_completion(NotesCompletion { sequence: 1, snapshot: store.snapshot(), acknowledged: vec![(id, 1)], successful: false, error: Some("changed marker".into()), conflicts: Vec::new() });
        assert!(model.notes_dirty.is_empty());
        assert!(model.note_pending(id));
        assert_eq!(model.notes.note(id).unwrap().text, "- [x] Original task");
        assert!(write_pending(&mut store, &model.pending_notes(None, false)).is_ok());
        let added = store.create(DEFAULT_TOPIC, "An unrelated note").unwrap();
        model.apply_notes_completion(NotesCompletion { sequence: 2, snapshot: store.snapshot(), acknowledged: Vec::new(), successful: true, error: None, conflicts: Vec::new() });
        assert!(model.notes.note(added).is_some());
        let other = root.path().join("other");
        model.apply_notes_completion(NotesCompletion { sequence: 3, snapshot: NotesSnapshot::empty(other.clone()), acknowledged: Vec::new(), successful: true, error: None, conflicts: Vec::new() });
        assert_eq!(model.notes.root, other);
        assert!(!model.has_note_conflicts());
        model.apply_notes_completion(NotesCompletion { sequence: 4, snapshot: store.snapshot(), acknowledged: Vec::new(), successful: true, error: None, conflicts: Vec::new() });
        assert!(model.has_note_conflicts());
        assert_eq!(model.notes.note(id).unwrap().text, "- [x] Original task");
        assert_eq!(store.note(id).unwrap().text, "Rewritten externally");
        assert!(!model.apply_notes_completion(NotesCompletion { sequence: 2, snapshot: store.snapshot(), acknowledged: vec![(id, 1)], successful: true, error: None, conflicts: Vec::new() }));
        assert!(model.has_note_conflicts(), "an older successful write cannot clear a newer quarantined conflict");
        assert_eq!(model.notes.note(id).unwrap().text, "- [x] Original task");
    }

    #[test]
    fn a_stale_completion_cannot_clear_a_newer_error_or_replace_its_snapshot() {
        let mut model = model(std::path::PathBuf::new());
        model.notes_applied = 2;
        model.notes_error = Some("newer index failure".into());
        let old = NotesSnapshot::empty(std::path::PathBuf::from("old"));
        assert!(!model.apply_notes_completion(NotesCompletion { sequence: 1, snapshot: old, acknowledged: Vec::new(), successful: true, error: None, conflicts: Vec::new() }));
        assert_eq!(model.notes_error.as_deref(), Some("newer index failure"));
        assert_eq!(model.notes.root, std::path::PathBuf::new());
        let current = NotesSnapshot::empty(std::path::PathBuf::from("current"));
        assert!(model.apply_notes_completion(NotesCompletion { sequence: 3, snapshot: current, acknowledged: Vec::new(), successful: true, error: None, conflicts: Vec::new() }));
        assert!(model.notes_error.is_none());
        assert_eq!(model.notes.root, std::path::PathBuf::from("current"));
    }

    #[test]
    fn a_panel_transition_stays_pending_until_its_latest_generation_is_acknowledged() {
        let mut model = model(std::path::PathBuf::new());
        model.notes_dirty.insert(7, 2);
        assert!(model.note_pending(7));
        acknowledge(&mut model.notes_dirty, &mut model.notes_submitted, vec![(7, 1)], true, |_| {});
        assert!(model.note_pending(7));
        acknowledge(&mut model.notes_dirty, &mut model.notes_submitted, vec![(7, 2)], true, |_| {});
        assert!(!model.note_pending(7));
    }

    #[test]
    fn a_checkbox_click_cannot_replace_a_pending_whole_body_edit_with_only_a_patch() {
        let mut model = model(std::path::PathBuf::new());
        model.notes_dirty.insert(1, 1);
        assert!(model.take_task_patches(1).is_none());
        assert!(model.take_task_patches(2).unwrap().is_empty());
        model.notes_tasks.insert(1, BTreeMap::from([(2, (5, true))]));
        assert_eq!(model.take_task_patches(1).unwrap(), BTreeMap::from([(2, (5, true))]));
    }

    #[test]
    fn explicit_delete_excludes_only_its_own_failed_draft_prerequisite() {
        let root = tempfile::tempdir().unwrap();
        let mut store = NoteStore::open(root.path());
        let deleted = store.create(DEFAULT_TOPIC, "Already removed").unwrap();
        let other = store.create(DEFAULT_TOPIC, "Other original").unwrap();
        let mut model = model(root.path().into());
        model.notes = store.snapshot();
        model.notes_dirty.insert(deleted, 1);
        model.notes_dirty.insert(other, 2);
        store.delete(deleted).unwrap();
        let pending = model.pending_notes(Some(deleted), false);
        assert_eq!(pending.iter().map(|pending| pending.note.id).collect::<Vec<_>>(), vec![other]);
        assert!(write_pending(&mut store, &pending).is_ok());
        assert!(!store.delete(deleted).unwrap());
        assert_eq!(store.note(other).unwrap().text, "Other original");
    }

    /// Types `text` into note `id` as [`Productivity::edit_note`] does
    /// (without a window): the first edit of a run remembers the body it
    /// replaces.
    fn type_body(model: &mut Productivity, id: u64, text: &str) {
        let pos = model.notes.notes.iter().position(|note| note.id == id).unwrap();
        let note = model.notes.notes[pos].clone();
        model.notes_conflicts.retain(|(_, note), _| *note != id);
        model.notes_base.entry(id).or_insert_with(|| body_hash(&note.text));
        model.notes.notes[pos] = Arc::new(Note { id, topic: note.topic, path: note.path.clone(), text: text.into(), updated: note.updated });
        model.notes_generation += 1;
        model.notes_dirty.insert(id, model.notes_generation);
    }

    /// Runs the model's pending bodies as the worker would and applies the
    /// completion as `observe_notes` does.
    fn save(model: &mut Productivity, store: &mut NoteStore, sequence: u64) -> Option<Vec<u64>> {
        let pending = model.pending_notes(None, false);
        let acknowledged = pending.iter().map(PendingNote::token).collect();
        let result = write_pending(store, &pending);
        let conflicts = result.as_ref().err().and_then(|error| error.downcast_ref::<BodyConflicts>()).map(|c| c.0.clone());
        assert!(result.is_ok() || conflicts.is_some(), "only conflicts fail here: {result:?}");
        model.apply_notes_completion(NotesCompletion { sequence, snapshot: store.snapshot(), acknowledged, successful: true, error: None, conflicts: conflicts.clone().unwrap_or_default() });
        conflicts
    }

    /// An ordinary panel body save checks the file against the body the
    /// edit began from: a change made in another editor is kept, the draft
    /// stays visible as a conflict, the batch's other notes still save, and
    /// editing again replaces the outside body deliberately (#111).
    #[test]
    fn a_body_save_keeps_an_outside_edit_without_blocking_other_notes_or_locking_the_note() {
        let root = tempfile::tempdir().unwrap();
        let mut store = NoteStore::open(root.path());
        let id = store.create(DEFAULT_TOPIC, "# Plan\nOriginal").unwrap();
        let other = store.create(DEFAULT_TOPIC, "# Other\nOriginal").unwrap();
        let path = root.path().join(store.note(id).unwrap().path.as_ref().unwrap());
        let mut model = model(root.path().into());
        model.notes = store.snapshot();
        type_body(&mut model, id, "# Plan\nMy draft");
        type_body(&mut model, other, "# Other\nSaved alongside");
        // Another editor saves while the draft is still unsaved.
        std::fs::write(&path, "# Plan\nWritten in another editor").unwrap();
        assert_eq!(save(&mut model, &mut store, 1), Some(vec![id]));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Plan\nWritten in another editor", "the outside edit is not replaced");
        assert_eq!(store.note(other).unwrap().text, "# Other\nSaved alongside");
        assert!(!model.notes_dirty.contains_key(&other), "the other note's write was acknowledged");
        assert!(model.has_note_conflicts() && model.note_pending(id));
        assert_eq!(model.notes.note(id).unwrap().text, "# Plan\nMy draft", "the draft is still shown");
        assert!(model.notes_error.is_none(), "shown as a conflict, not a failed save");
        assert!(model.pending_notes(None, false).is_empty(), "a conflict is not a write prerequisite");

        // The other editor is never locked out; editing again here replaces
        // what it wrote, on purpose.
        std::fs::write(&path, "# Plan\nWritten in another editor").unwrap();
        type_body(&mut model, id, "# Plan\nMy draft, after reading the warning");
        assert_eq!(save(&mut model, &mut store, 2), None);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Plan\nMy draft, after reading the warning");
        assert!(!model.has_note_conflicts() && !model.note_pending(id));
        assert!(model.notes_base.is_empty(), "acknowledged drafts forget their base");
    }

    /// A conflict from an older save attempt never takes over the newer draft
    /// typed since: it stays dirty, with its own base, for its own save.
    #[test]
    fn a_stale_conflict_leaves_the_newer_draft_dirty() {
        let root = tempfile::tempdir().unwrap();
        let mut store = NoteStore::open(root.path());
        let id = store.create(DEFAULT_TOPIC, "# Plan\nOne").unwrap();
        let mut model = model(root.path().into());
        model.notes = store.snapshot();
        model.notes_dirty.insert(id, 2);
        model.apply_notes_completion(NotesCompletion { sequence: 1, snapshot: store.snapshot(), acknowledged: vec![(id, 1)], successful: true, error: None, conflicts: vec![id] });
        assert_eq!(model.notes_dirty.get(&id), Some(&2), "the newer draft is still waiting to be saved");
        assert!(!model.has_note_conflicts(), "the older attempt's conflict is not applied to it");
        // The same generation's conflict is retained as before.
        model.apply_notes_completion(NotesCompletion { sequence: 2, snapshot: store.snapshot(), acknowledged: vec![(id, 2)], successful: true, error: None, conflicts: vec![id] });
        assert!(model.has_note_conflicts() && !model.notes_dirty.contains_key(&id));
    }

    /// A draft that conflicts with an outside edit is kept back for its own
    /// note, and the different edit queued behind it still runs (#132 review).
    #[test]
    fn a_body_conflict_does_not_cancel_the_edit_queued_behind_it() {
        let root = tempfile::tempdir().unwrap();
        let mut store = NoteStore::open(root.path());
        let id = store.create(DEFAULT_TOPIC, "# Log\nOne").unwrap();
        let path = root.path().join(store.note(id).unwrap().path.as_ref().unwrap());
        let mut model = model(root.path().into());
        model.notes = store.snapshot();
        let draft = PendingNote { note: Arc::new(Note { text: "# Log\nMine".into(), ..(*model.notes.notes[0]).clone() }), generation: 1, tasks: None, base: Some(body_hash("# Log\nOne")) };
        std::fs::write(&path, "# Log\nChanged outside").unwrap();
        let added = pending_then(&mut store, &[draft], |store| store.create(DEFAULT_TOPIC, "# Other\nNew")).expect("the queued edit runs");
        assert!(store.note(added).is_some(), "the other note was created");
        assert_eq!(std::mem::take(&mut store.deferred_conflicts), [id], "the conflicting draft is kept back for its own note");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Log\nChanged outside", "the outside edit is untouched");
        // With nothing pending, the edit just runs.
        assert!(pending_then(&mut store, &[], |_| anyhow::Ok(())).is_ok());
    }

    /// Coalesced drafts queued behind the note's own earlier write, or
    /// after a failed one, save in order; an outside edit between them is
    /// still caught.
    #[test]
    fn queued_body_drafts_follow_their_own_writes_but_not_outside_ones() {
        let root = tempfile::tempdir().unwrap();
        let mut store = NoteStore::open(root.path());
        let id = store.create(DEFAULT_TOPIC, "# Log\nOne").unwrap();
        let path = root.path().join(store.note(id).unwrap().path.as_ref().unwrap());
        let mut model = model(root.path().into());
        model.notes = store.snapshot();
        let base = body_hash("# Log\nOne");
        let draft = |text: &str, generation| PendingNote { note: Arc::new(Note { text: text.into(), ..(*model.notes.notes[0]).clone() }), generation, tasks: None, base: Some(base) };
        // Two flushes of one edit run: the second sees the first's write.
        write_pending(&mut store, &[draft("# Log\nOne\nTwo", 1)]).unwrap();
        write_pending(&mut store, &[draft("# Log\nOne\nTwo\nThree", 2)]).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Log\nOne\nTwo\nThree");
        // Re-sending the same generation (a structural edit's prerequisite) is a no-op.
        write_pending(&mut store, &[draft("# Log\nOne\nTwo\nThree", 2)]).unwrap();
        // An outside edit after the store's own write is caught.
        std::fs::write(&path, "# Log\nRewritten outside").unwrap();
        let error = write_pending(&mut store, &[draft("# Log\nFour", 3)]).unwrap_err();
        assert_eq!(error.downcast_ref::<BodyConflicts>().map(|c| c.0.as_slice()), Some([id].as_slice()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Log\nRewritten outside");
        // A file removed elsewhere is a conflict too, never recreated silently.
        std::fs::remove_file(&path).unwrap();
        assert!(write_pending(&mut store, &[draft("# Log\nFive", 4)]).unwrap_err().is::<BodyConflicts>());
        assert!(!path.exists());
        // Without a base (a note with no file yet), nothing is compared.
        let fresh = store.create(DEFAULT_TOPIC, "").unwrap();
        let unsaved = PendingNote { note: Arc::new(Note { id: fresh, topic: DEFAULT_TOPIC, path: None, text: "# New\nBody".into(), updated: 0 }), generation: 5, tasks: None, base: None };
        write_pending(&mut store, &[unsaved]).unwrap();
        assert_eq!(store.note(fresh).unwrap().text, "# New\nBody");
    }

    #[test]
    fn a_delete_or_write_receipt_never_clears_a_newer_draft_generation() {
        let mut dirty = BTreeMap::from([(7, 2), (8, 1)]);
        let mut submitted = BTreeMap::from([(7, 1), (8, 1)]);
        acknowledge(&mut dirty, &mut submitted, vec![(7, 1), (8, 1)], true, |_| {});
        assert_eq!(dirty, BTreeMap::from([(7, 2)]));
        assert!(submitted.is_empty());
        acknowledge(&mut dirty, &mut submitted, vec![(7, 2)], false, |_| {});
        assert_eq!(dirty.get(&7), Some(&2));
    }
}
