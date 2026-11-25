//! Read-only notes shared with views while the worker owns filesystem state.

use std::path::PathBuf;
use std::sync::Arc;
use super::{Note, Topic};

/// A failed read makes this path read-only; its last valid body anchors recovery.
pub struct ReadError {
    pub path: String,
    pub message: String,
    pub retained: Option<Arc<Note>>,
}

/// Cloning a snapshot shares note bodies; only the small topic table is copied.
#[derive(Clone)]
pub struct NotesSnapshot {
    pub root: PathBuf,
    pub topics: Vec<Topic>,
    pub notes: Vec<Arc<Note>>,
    /// Per-file scan errors are visible without disabling unrelated notes.
    pub read_errors: Option<Arc<[ReadError]>>,
}

impl NotesSnapshot {
    /// Error lookup uses relative paths, independently of note ordering or IDs.
    pub fn read_error(&self, path: &str) -> Option<&ReadError> {
        self.read_errors.as_ref()?.iter().find(|error| error.path == path)
    }
    /// Empty until the initial background scan completes.
    pub fn empty(root: PathBuf) -> Self {
        Self { root, topics: vec![Topic::default()], notes: Vec::new(), read_errors: None }
    }
    /// Looks up stable topic metadata from the last completed scan/write.
    pub fn topic(&self, id: u64) -> Option<&Topic> {
        self.topics.iter().find(|t| t.id == id)
    }

    /// Returns a borrowed body; views need no store ownership or filesystem access.
    pub fn note(&self, id: u64) -> Option<&Note> {
        self.notes.iter().find(|n| n.id == id).map(Arc::as_ref)
    }

    /// The topic `key` names: id or name (case-insensitive, spaces and
    /// dashes ignored as for `#topic`).
    pub fn find_topic(&self, key: &str) -> Option<u64> {
        let key = key.trim().trim_start_matches('#');
        if let Ok(id) = key.parse::<u64>()
            && self.topic(id).is_some()
        {
            return Some(id);
        }
        let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '_').flat_map(char::to_lowercase).collect::<String>();
        let k = squash(key);
        (!k.is_empty()).then(|| self.topics.iter().find(|t| squash(&t.name) == k).map(|t| t.id)).flatten()
    }


    /// Notes never written to a file and still empty (abandoned drafts),
    /// except `keep`: the one just opened to type into.
    pub fn empty_drafts(&self, keep: Option<u64>) -> Vec<u64> {
        self.notes.iter().filter(|n| n.text.trim().is_empty() && n.path.is_none() && Some(n.id) != keep).map(|n| n.id).collect()
    }


    /// `(topic, count)` for the topic switcher.
    pub fn count(&self, topic: Option<u64>) -> usize {
        self.notes.iter().filter(|n| topic.is_none_or(|t| n.topic == t) && !n.text.trim().is_empty()).count()
    }

    /// Notes matching `query` (fuzzy over title and body), best first.
    pub fn search(&self, query: &str) -> Vec<u64> {
        use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
        use nucleo_matcher::{Config, Matcher, Utf32Str};
        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut buf = Vec::new();
        let mut hits: Vec<(u32, u64)> = self
            .notes
            .iter()
            .filter_map(|n| {
                let hay = format!("{} {}", n.title(), n.text);
                pattern.score(Utf32Str::new(&hay, &mut buf), &mut matcher).map(|s| (s, n.id))
            })
            .collect();
        hits.sort_by_key(|(s, id)| (std::cmp::Reverse(*s), *id));
        hits.into_iter().map(|(_, id)| id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes_store::{DEFAULT_TOPIC, NoteStore};

    #[test]
    fn snapshots_share_unchanged_bodies_and_preserve_the_previous_changed_entry() {
        let root = tempfile::tempdir().unwrap();
        let mut store = NoteStore::open(root.path());
        let changed = store.create(DEFAULT_TOPIC, "Old body").unwrap();
        let kept = store.create(DEFAULT_TOPIC, "A different note").unwrap();
        let before = store.snapshot();
        let old_body = before.note(changed).unwrap().text.as_ptr();
        store.write(changed, "Replacement body").unwrap();
        let after = store.snapshot();
        let old_kept = before.notes.iter().find(|note| note.id == kept).unwrap();
        let new_kept = after.notes.iter().find(|note| note.id == kept).unwrap();
        assert!(Arc::ptr_eq(old_kept, new_kept));
        assert_eq!(before.note(changed).unwrap().text, "Old body");
        assert_eq!(before.note(changed).unwrap().text.as_ptr(), old_body);
        assert_eq!(after.note(changed).unwrap().text, "Replacement body");
        let old_wire = serde_json::to_value(before.note(kept).unwrap()).unwrap();
        assert_eq!(old_wire, serde_json::to_value(new_kept.as_ref()).unwrap());
    }
}
