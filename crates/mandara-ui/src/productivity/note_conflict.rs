//! Retained drafts compare recovered files with their last readable body.

use std::sync::Arc;
use crate::notes_store::{Note, NotesSnapshot};
use super::note_edit::TaskPatches;

pub(super) struct RetainedDraft {
    pub generation: u64,
    pub note: Arc<Note>,
    pub tasks: Option<TaskPatches>,
    pub original: Option<Arc<Note>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Recovery { Waiting, Retry, Conflict }

impl RetainedDraft {
    /// Retry only an unchanged recovered file; external replacements stay conflicts.
    pub fn recovery(&self, snapshot: &NotesSnapshot) -> Recovery {
        let Some(original) = &self.original else { return Recovery::Conflict };
        if original.path.as_deref().is_some_and(|path| snapshot.read_error(path).is_some()) { return Recovery::Waiting; }
        match snapshot.note(original.id) {
            Some(current) if current.text == original.text && current.path == original.path && current.topic == original.topic => Recovery::Retry,
            _ => Recovery::Conflict,
        }
    }
}
