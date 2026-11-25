//! Load each persisted book independently so one damaged file cannot hide the other.

use std::path::Path;
use super::{Saved, todo_writer};
use crate::notes_store::NoteStore;

pub(super) struct Loaded {
    pub todos: Result<Saved, String>,
    pub notes: NoteStore,
    pub migrated: bool,
}

/// Migration needs a valid JSON book; Markdown remains available after JSON errors.
pub(super) fn books(json: &Path, root: &Path) -> Loaded {
    let mut todos = todo_writer::load_saved(json).map_err(|error| format!("{error:#}"));
    let mut notes = NoteStore::open(root);
    let migrated = todos.as_mut().is_ok_and(|saved| {
        if saved.notes.is_empty() || notes.migrate(&saved.notes).is_err() { return false; }
        saved.notes.clear();
        true
    });
    Loaded { todos, notes, migrated }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes_store::DEFAULT_TOPIC;

    #[test]
    fn malformed_json_does_not_hide_or_overwrite_markdown_notes() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("notes");
        let mut notes = NoteStore::open(&folder);
        let id = notes.create(DEFAULT_TOPIC, "# Existing note\nKeep me").unwrap();
        drop(notes);
        let json = root.path().join("productivity.json");
        let damaged = b"{unfinished user data";
        std::fs::write(&json, damaged).unwrap();
        let mut loaded = books(&json, &folder);
        assert!(loaded.todos.is_err());
        assert!(!loaded.migrated);
        assert_eq!(loaded.notes.note(id).unwrap().text, "# Existing note\nKeep me");
        loaded.notes.write(id, "# Existing note\nStill editable").unwrap();
        assert_eq!(std::fs::read(json).unwrap(), damaged);
    }
}
