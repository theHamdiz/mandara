//! A note draft is either a whole-text edit or a set of checkbox intentions.

use std::collections::BTreeMap;
use std::sync::Arc;
use crate::notes_store::{Note, NoteStore, body_hash};

/// Start byte → (end byte, desired checked state), coalesced per marker.
pub(super) type TaskPatches = BTreeMap<usize, (usize, bool)>;

pub(super) struct PendingNote {
    pub note: Arc<Note>,
    pub generation: u64,
    pub tasks: Option<TaskPatches>,
    /// [`body_hash`] of the file body a whole-text draft replaces: what the
    /// panel showed when the edit began. `None` skips the check (a draft
    /// with no file yet).
    pub base: Option<u64>,
}

/// Whole-text drafts the worker would not write because their file changed
/// elsewhere (or disappeared) since the edit began. The other drafts of the
/// same batch are still written; the panel keeps these as conflicts.
#[derive(Debug)]
pub(super) struct BodyConflicts(pub Vec<u64>);

impl std::fmt::Display for BodyConflicts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(mandara_i18n::t("This item changed elsewhere. Your text is still here."))
    }
}

impl std::error::Error for BodyConflicts {}

impl PendingNote {
    /// The exact generation this receipt may acknowledge.
    pub fn token(&self) -> (u64, u64) { (self.note.id, self.generation) }

    /// Whether writing this whole-text draft replaces nothing unseen. It
    /// re-reads the file (on the worker) and accepts the draft's base, the
    /// store's own last write of this note (so coalesced drafts queued
    /// behind an earlier write, or after a failed one, still save in order)
    /// or the draft itself. `false` means another program changed or
    /// removed the file. It never locks the file: a changed file only
    /// stops this one write.
    fn base_holds(&self, store: &mut NoteStore) -> anyhow::Result<bool> {
        let id = self.note.id;
        let Some(base) = self.base else { return Ok(true) };
        // A note a rescan removed is a conflict too, so the edit queued
        // behind this draft isn't failed by a later "no such note".
        let Some(current) = store.note(id) else { return Ok(false) };
        if current.path.is_none() { return Ok(true); }
        if !store.refresh_note(id)? { return Ok(false); }
        Ok(store.note(id).is_some_and(|current| current.text == self.note.text || body_hash(&current.text) == base || store.wrote(id, &current.text)))
    }

    /// Marker edits preserve unrelated text appended before the intent
    /// executes; whole-text drafts first check their base
    /// ([`BodyConflicts`] when the file changed elsewhere).
    pub fn apply(&self, store: &mut NoteStore) -> anyhow::Result<()> {
        store.ensure_writable(self.note.id)?;
        if let Some(tasks) = &self.tasks {
            let current = store.note(self.note.id).ok_or_else(|| anyhow::anyhow!("no such note"))?;
            let mut text = current.text.clone();
            for (&start, &(end, checked)) in tasks {
                let marker = text.get(start..end);
                anyhow::ensure!(matches!(marker, Some("[ ]" | "[x]" | "[X]")), "{}", mandara_i18n::t("This item changed elsewhere. Your text is still here."));
                if marker != Some(if checked { "[x]" } else { "[ ]" }) {
                    text.replace_range(start..end, if checked { "[x]" } else { "[ ]" });
                }
            }
            if text != current.text { store.write(self.note.id, &text)?; }
        } else if !(self.note.text.trim().is_empty() && self.note.path.is_none()) {
            if !self.base_holds(store)? {
                return Err(BodyConflicts(vec![self.note.id]).into());
            }
            if store.note(self.note.id).is_none() {
                // Its file was removed elsewhere and you edited the kept
                // draft again: the draft recreates the note.
                store.restore(self.note.id, self.note.topic, &self.note.text)?;
            } else if store.note(self.note.id).is_some_and(|current| current.text != self.note.text || current.path.is_none()) {
                store.write(self.note.id, &self.note.text)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes_store::DEFAULT_TOPIC;

    #[test]
    fn a_checkbox_patch_keeps_an_earlier_append_and_latest_marker_intent() {
        let root = tempfile::tempdir().unwrap();
        let mut store = NoteStore::open(root.path());
        let id = store.create(DEFAULT_TOPIC, "- [ ] First\n- [ ] Second").unwrap();
        let original = store.snapshot().notes[0].clone();
        // The UI prepared the click while an append was still queued.
        store.write(id, "- [ ] First\n- [ ] Second\nAppended elsewhere").unwrap();
        let mut pending = PendingNote { note: original, generation: 1, base: None, tasks: Some(BTreeMap::from([(2, (5, true)), (14, (17, true))])) };
        pending.apply(&mut store).unwrap();
        assert_eq!(store.note(id).unwrap().text, "- [x] First\n- [x] Second\nAppended elsewhere");
        pending.tasks.as_mut().unwrap().insert(2, (5, false));
        pending.apply(&mut store).unwrap();
        assert_eq!(store.note(id).unwrap().text, "- [ ] First\n- [x] Second\nAppended elsewhere");
    }

    #[test]
    fn a_changed_marker_is_reported_without_replacing_the_newer_body() {
        let root = tempfile::tempdir().unwrap();
        let mut store = NoteStore::open(root.path());
        let id = store.create(DEFAULT_TOPIC, "- [ ] Task").unwrap();
        let original = store.snapshot().notes[0].clone();
        store.write(id, "A rewritten note").unwrap();
        let pending = PendingNote { note: original, generation: 1, base: None, tasks: Some(BTreeMap::from([(2, (5, true))])) };
        assert!(pending.apply(&mut store).is_err());
        assert_eq!(store.note(id).unwrap().text, "A rewritten note");
    }
}
