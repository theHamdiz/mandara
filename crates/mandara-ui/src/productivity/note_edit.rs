//! A note draft is either a whole-text edit or a set of checkbox intentions.

use std::collections::BTreeMap;
use std::sync::Arc;
use crate::notes_store::{Note, NoteStore};

/// Start byte → (end byte, desired checked state), coalesced per marker.
pub(super) type TaskPatches = BTreeMap<usize, (usize, bool)>;

pub(super) struct PendingNote {
    pub note: Arc<Note>,
    pub generation: u64,
    pub tasks: Option<TaskPatches>,
}

impl PendingNote {
    /// The exact generation this receipt may acknowledge.
    pub fn token(&self) -> (u64, u64) { (self.note.id, self.generation) }

    /// Marker edits preserve unrelated text appended before the intent executes.
    pub fn apply(&self, store: &mut NoteStore) -> anyhow::Result<()> {
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
        } else if !(self.note.text.trim().is_empty() && self.note.path.is_none())
            && store.note(self.note.id).is_none_or(|current| current.text != self.note.text || current.path.is_none()) {
            store.write(self.note.id, &self.note.text)?;
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
        let mut pending = PendingNote { note: original, generation: 1, tasks: Some(BTreeMap::from([(2, (5, true)), (14, (17, true))])) };
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
        let pending = PendingNote { note: original, generation: 1, tasks: Some(BTreeMap::from([(2, (5, true))])) };
        assert!(pending.apply(&mut store).is_err());
        assert_eq!(store.note(id).unwrap().text, "A rewritten note");
    }
}
