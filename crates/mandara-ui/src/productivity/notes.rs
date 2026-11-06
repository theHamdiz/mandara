//! UI snapshots, dirty generations and ordered Markdown completion.

use std::sync::Arc;
use gpui::Context;
use crate::notes_store::{Note, NoteStore, NotesSnapshot};
use crate::todo_lists::parse_quick_add;
use crate::state::config;
use super::{Productivity, NoteReply, SAVE_DEBOUNCE, notes_worker, notes_root, now_unix};
use super::note_edit::{PendingNote, TaskPatches};

struct NotesCompletion {
    sequence: u64,
    snapshot: NotesSnapshot,
    acknowledged: Vec<(u64, u64)>,
    successful: bool,
    error: Option<String>,
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
        self.notes_sequence += 1;
        let sequence = self.notes_sequence;
        let (reply, result) = async_channel::bounded(1);
        cx.spawn(async move |this, cx| {
            let Ok(done) = incoming.recv().await else { return };
            let notes_worker::Completion { snapshot, value, index_error } = done;
            let successful = value.is_ok() && index_error.is_none();
            let _ = this.update(cx, |p, cx| {
                let error = value.as_ref().err().map(|error| format!("{error:#}")).or_else(|| index_error.clone());
                if p.apply_notes_completion(NotesCompletion { sequence, snapshot, acknowledged, successful, error }) { cx.notify(); }
                p.follow_notes_completion(cx);
            });
            let _ = reply.try_send(NoteReply { value: value.map_err(|error| format!("{error:#}")), index_error });
        }).detach();
        Ok(result)
    }

    /// Error status and snapshots share the same completion sequence boundary.
    fn apply_notes_completion(&mut self, update: NotesCompletion) -> bool {
        let NotesCompletion { sequence, mut snapshot, acknowledged, successful, error } = update;
        acknowledge(&mut self.notes_dirty, &mut self.notes_submitted, acknowledged, successful, |id| { self.notes_tasks.remove(&id); });
        if sequence < self.notes_applied { return false; }
        self.notes_error = error;
        if snapshot.root == self.notes.root || self.notes_dirty.is_empty() {
            for draft in self.notes.notes.iter().filter(|draft| self.notes_dirty.contains_key(&draft.id)) {
                if let Some(note) = snapshot.notes.iter_mut().find(|note| note.id == draft.id) {
                    if note.path == draft.path && note.topic == draft.topic { *note = draft.clone(); }
                    else { *note = Arc::new(Note { id: note.id, topic: note.topic, path: note.path.clone(), text: draft.text.clone(), updated: draft.updated }); }
                } else { snapshot.notes.push(draft.clone()); }
            }
            self.notes = snapshot;
            if self.notes_topic.is_some_and(|topic| self.notes.topic(topic).is_none()) { self.notes_topic = None; }
        }
        self.notes_applied = sequence;
        true
    }

    /// Drain a latest dirty model only after every already-submitted ordering barrier.
    fn follow_notes_completion(&mut self, cx: &mut Context<Self>) {
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
            .filter_map(|(&id, &generation)| self.notes.notes.iter().find(|note| note.id == id).map(|note| PendingNote { note: note.clone(), generation, tasks: self.notes_tasks.get(&id).cloned() }))
            .collect()
    }

    /// Enqueues the latest dirty bodies before the next ordered operation.
    /// Dirty generations stay retained until their own disk write succeeds.
    pub fn flush_notes(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
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
        self.queue_notes(cx, acknowledged, move |store| {
            write_pending(store, &pending)?;
            edit(store)
        })
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
        *note = Arc::new(Note { id, topic: note.topic, path: note.path.clone(), text: text.to_owned(), updated: now_unix() });
        self.notes_generation += 1;
        self.notes_dirty.insert(id, self.notes_generation);
        self.notes_tasks.remove(&id);
        self.notes_changed(cx);
    }

    /// A panel must not close a field that changed after its save request.
    pub fn note_pending(&self, id: u64) -> bool { self.notes_dirty.contains_key(&id) }

    /// Flushes before final filename/delete work and reports the real completion.
    pub fn settle_note(&mut self, id: u64, cx: &mut Context<Self>) -> Result<async_channel::Receiver<NoteReply<bool>>, String> {
        self.edit_notes(cx, move |store| store.settle(id))
    }

    /// Explicit deletion may discard this draft, while other dirty writes stay ordered.
    pub fn remove_note(&mut self, id: u64, cx: &mut Context<Self>) -> Result<async_channel::Receiver<NoteReply<bool>>, String> {
        let pending = self.pending_notes(Some(id), false);
        let mut acknowledged: Vec<_> = pending.iter().map(PendingNote::token).collect();
        let local_draft = self.notes_dirty.get(&id).copied();
        if let Some(generation) = local_draft { acknowledged.push((id, generation)); }
        self.queue_notes(cx, acknowledged, move |store| {
            write_pending(store, &pending)?;
            store.delete(id).map(|removed| removed || local_draft.is_some())
        })
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

    /// Reconciles external files only after accepted local drafts have reached disk.
    pub(super) fn rescan_notes(&mut self, cx: &mut Context<Self>) {
        let result = self.edit_notes(cx, |store| { store.rescan(&[]); Ok(()) });
        self.log_notes(result, cx);
    }

    /// Waits for pending bodies/metadata before opening the configured folder.
    pub(super) fn ensure_notes_root(&mut self, root: std::path::PathBuf, cx: &mut Context<Self>) {
        if self.notes.root == root || self.notes_switching || !self.ready { return; }
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
                    });
                }).detach();
            }
        }
    }

}

/// A structural edit must not pass a failed prerequisite body or index write.
fn write_pending(store: &mut NoteStore, pending: &[PendingNote]) -> anyhow::Result<()> {
    if store.scan_result().is_err() { store.rescan(&[]); store.scan_result()?; }
    for note in pending { note.apply(store)?; }
    store.persist_index()
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

    #[test]
    fn a_stale_completion_cannot_clear_a_newer_error_or_replace_its_snapshot() {
        let mut model = model(std::path::PathBuf::new());
        model.notes_applied = 2;
        model.notes_error = Some("newer index failure".into());
        let old = NotesSnapshot::empty(std::path::PathBuf::from("old"));
        assert!(!model.apply_notes_completion(NotesCompletion { sequence: 1, snapshot: old, acknowledged: Vec::new(), successful: true, error: None }));
        assert_eq!(model.notes_error.as_deref(), Some("newer index failure"));
        assert_eq!(model.notes.root, std::path::PathBuf::new());
        let current = NotesSnapshot::empty(std::path::PathBuf::from("current"));
        assert!(model.apply_notes_completion(NotesCompletion { sequence: 3, snapshot: current, acknowledged: Vec::new(), successful: true, error: None }));
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
