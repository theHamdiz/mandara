//! Prepare a complete folder scan before replacing the shared read snapshot.

use std::sync::Arc;
use anyhow::{Context as _, Result};
use super::{DEFAULT_TOPIC, EXTENSION, Note, NoteStore, ReadError, Topic, rel, unix};
use crate::todo_lists::{ListColor, SWATCH_HUES};

enum ScannedNote {
    Reused(Arc<Note>),
    Read { path: String, topic: u64, text: String, updated: i64 },
}

impl NoteStore {
    pub(super) fn scan(&mut self) -> Result<()> {
        let mut topics = self.view.topics.clone();
        if !topics.iter().any(|topic| topic.id == DEFAULT_TOPIC) { topics.insert(0, Topic::default()); }
        if let Some(topic) = topics.iter_mut().find(|topic| topic.id == DEFAULT_TOPIC) { topic.dir.clear(); }
        let mut dirs = Vec::new();
        for entry in std::fs::read_dir(&self.view.root).with_context(|| format!("reading {}", self.view.root.display()))? {
            let entry = entry?;
            if entry.file_type()?.is_dir()
                && let Ok(name) = entry.file_name().into_string()
                && !name.starts_with('.') { dirs.push(name); }
        }
        dirs.sort();
        topics.retain(|topic| topic.id == DEFAULT_TOPIC || dirs.contains(&topic.dir));
        for dir in dirs {
            if !topics.iter().any(|topic| topic.dir == dir) {
                let id = topics.iter().map(|topic| topic.id).max().unwrap_or(DEFAULT_TOPIC) + 1;
                let color = format!("hue:{}", SWATCH_HUES[topics.len() % SWATCH_HUES.len()]);
                topics.push(Topic { id, name: dir.clone(), dir, color, icon: None });
            }
        }
        for topic in &mut topics {
            if !ListColor::valid(&topic.color) { topic.color = crate::todo_lists::DEFAULT_LIST_COLOR.into(); }
        }
        let mut read = Vec::new();
        let mut failed = Vec::new();
        let mut warnings = Vec::new();
        for topic in &topics {
            let dir = self.view.root.join(&topic.dir);
            for entry in std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
                let entry = entry?;
                let path = entry.path();
                if entry.file_name().to_string_lossy().starts_with('.') || path.extension().and_then(|value| value.to_str()) != Some(EXTENSION) || !entry.file_type()?.is_file() { continue; }
                let relative = rel(path.strip_prefix(&self.view.root).unwrap_or(&path));
                let text = match std::fs::read_to_string(&path) {
                    Ok(text) => text,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        let warning = format!("reading {}: {error}", path.display());
                        tracing::warn!("notes: {warning}");
                        let retained = self.view.notes.iter().find(|note| note.path.as_deref() == Some(relative.as_str())).cloned();
                        warnings.push(ReadError { path: relative.clone(), message: warning, retained: retained.clone() });
                        failed.push(relative.clone());
                        if let Some(note) = retained {
                            read.push(ScannedNote::Reused(note));
                        }
                        continue;
                    }
                };
                let updated = entry.metadata().and_then(|metadata| metadata.modified()).map(unix).unwrap_or(0);
                let path = relative;
                match self.view.notes.iter().find(|note| note.path.as_deref() == Some(path.as_str()) && note.topic == topic.id && note.text == text && note.updated == updated) {
                    Some(note) => read.push(ScannedNote::Reused(note.clone())),
                    None => read.push(ScannedNote::Read { path, topic: topic.id, text, updated }),
                }
            }
        }
        // Directory traversal completed; failed files keep their previous identity.
        let mut notes = Vec::with_capacity(read.len());
        for entry in read {
            let (path, topic, text, updated) = match entry {
                ScannedNote::Reused(note) => { notes.push(note); continue; }
                ScannedNote::Read { path, topic, text, updated } => (path, topic, text, updated),
            };
            let id = match self.ids.get(&path) {
                Some(id) => *id,
                None => {
                    let id = self.fresh_id();
                    self.ids.insert(path.clone(), id);
                    id
                }
            };
            notes.push(Arc::new(Note { id, topic, path: Some(path), text, updated }));
        }
        self.ids.retain(|path, _| failed.contains(path) || notes.iter().any(|note| note.path.as_deref() == Some(path.as_str())));
        notes.sort_by_key(|note| std::cmp::Reverse((note.updated, note.id)));
        self.view.topics = topics;
        self.view.notes = notes;
        self.view.read_errors = (!warnings.is_empty()).then(|| Arc::from(warnings));
        self.scan_error = None;
        self.save_index();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes_store::INDEX_FILE;

    #[test]
    fn one_invalid_file_preserves_its_snapshot_and_id_while_other_notes_remain_editable() {
        let root = tempfile::tempdir().unwrap();
        let mut store = NoteStore::open(root.path());
        let bad = store.create(DEFAULT_TOPIC, "# Damaged\nKeep this snapshot").unwrap();
        let good = store.create(DEFAULT_TOPIC, "# Good\nOriginal").unwrap();
        let path = root.path().join(store.note(bad).unwrap().path.as_ref().unwrap());
        let before = store.snapshot().notes.into_iter().find(|note| note.id == bad).unwrap();
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        store.rescan(&[]);
        assert!(store.scan_result().is_ok());
        assert!(store.read_errors.as_ref().is_some_and(|errors| errors.len() == 1));
        assert!(Arc::ptr_eq(&before, store.notes.iter().find(|note| note.id == bad).unwrap()));
        assert!(store.write(bad, "Stale snapshot").is_err());
        assert!(store.settle(bad).is_err());
        assert!(store.delete(bad).is_err());
        let topic = store.add_topic("Other", None, None).unwrap();
        assert!(store.move_note(bad, topic).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), [0xff, 0xfe], "failed reads must leave the original bytes untouched");
        store.write(good, "# Good\nChanged").unwrap();
        let added = store.create(DEFAULT_TOPIC, "Another note").unwrap();
        assert_eq!(store.note(good).unwrap().text, "# Good\nChanged");
        assert!(store.note(added).is_some());
        assert!(store.prepare_leave().is_ok());
        drop(store);
        let mut reopened = NoteStore::open(root.path());
        assert!(reopened.ids.values().any(|id| *id == bad));
        std::fs::write(path, "# Damaged\nRecovered").unwrap();
        reopened.rescan(&[]);
        assert_eq!(reopened.note(bad).unwrap().text, "# Damaged\nRecovered");
        assert!(reopened.read_errors.is_none());
    }

    #[test]
    fn a_successful_rescan_reuses_unchanged_note_bodies() {
        let root = tempfile::tempdir().unwrap();
        let mut store = NoteStore::open(root.path());
        let id = store.create(DEFAULT_TOPIC, "Keep this body").unwrap();
        store.rescan(&[]);
        let before = store.snapshot();
        store.rescan(&[]);
        assert!(store.index_result().is_ok());
        let after = store.notes.iter().find(|note| note.id == id).unwrap();
        assert!(Arc::ptr_eq(&before.notes[0], after));
    }

    #[test]
    fn a_failed_folder_scan_keeps_the_previous_snapshot_and_index_until_recovery() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("notes");
        let saved = root.path().join("kept");
        let mut store = NoteStore::open(&path);
        let id = store.create(DEFAULT_TOPIC, "Keep this body").unwrap();
        let before = store.snapshot();
        let index = std::fs::read(path.join(INDEX_FILE)).unwrap();
        std::fs::rename(&path, &saved).unwrap();
        std::fs::write(&path, b"not a directory").unwrap();
        store.rescan(&[]);
        assert!(store.scan_result().is_err());
        assert!(Arc::ptr_eq(&before.notes[0], &store.notes[0]));
        assert_eq!(std::fs::read(saved.join(INDEX_FILE)).unwrap(), index);
        assert!(store.prepare_leave().is_ok(), "an unchanged unreadable root does not trap the folder setting");
        std::fs::remove_file(&path).unwrap();
        std::fs::rename(saved, &path).unwrap();
        store.rescan(&[]);
        assert!(store.index_result().is_ok());
        assert_eq!(store.note(id).unwrap().text, "Keep this body");
        assert_eq!(std::fs::read(path.join(INDEX_FILE)).unwrap(), index);
    }
}
