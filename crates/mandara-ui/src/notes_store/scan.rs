//! Prepare a complete folder scan before replacing the shared read snapshot.

use std::sync::Arc;
use anyhow::{Context as _, Result};
use super::{DEFAULT_TOPIC, EXTENSION, Note, NoteStore, Topic, rel, unix};
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
        for topic in &topics {
            let dir = self.view.root.join(&topic.dir);
            for entry in std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
                let entry = entry?;
                let path = entry.path();
                if entry.file_name().to_string_lossy().starts_with('.') || path.extension().and_then(|value| value.to_str()) != Some(EXTENSION) || !entry.file_type()?.is_file() { continue; }
                let text = match std::fs::read_to_string(&path) {
                    Ok(text) => text,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
                };
                let updated = entry.metadata().and_then(|metadata| metadata.modified()).map(unix).unwrap_or(0);
                let path = rel(path.strip_prefix(&self.view.root).unwrap_or(&path));
                match self.view.notes.iter().find(|note| note.path.as_deref() == Some(path.as_str()) && note.topic == topic.id && note.text == text && note.updated == updated) {
                    Some(note) => read.push(ScannedNote::Reused(note.clone())),
                    None => read.push(ScannedNote::Read { path, topic: topic.id, text, updated }),
                }
            }
        }
        // All fallible reads completed. No partial scan can erase remembered files.
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
        self.ids.retain(|path, _| notes.iter().any(|note| note.path.as_deref() == Some(path.as_str())));
        notes.sort_by_key(|note| std::cmp::Reverse((note.updated, note.id)));
        self.view.topics = topics;
        self.view.notes = notes;
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
