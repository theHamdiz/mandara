//! Dispatches desktop writes and returns their actual persistence completion.

use super::draft::SaveError;
use crate::productivity::Productivity;
use gpui::Context;
use mandara_i18n::{t, tr};

#[derive(Clone, Copy)]
pub(super) enum Target {
    Task { id: Option<u64>, list: u64 },
    Note { id: Option<u64>, topic: u64 },
}

impl Target {
    /// A damaged to-do book does not prevent editing the independent Markdown store.
    pub(super) fn ready(self, model: &Productivity) -> bool {
        if self.multiline() { model.notes_ready } else { model.ready }
    }
    /// Notes preserve Markdown newlines; tasks use a single-line field.
    pub(super) fn multiline(self) -> bool { matches!(self, Self::Note { .. }) }

    /// Resolves only this target from the current shared read model.
    pub(super) fn text(self, p: &Productivity) -> Option<&str> {
        match self {
            Self::Task { id: Some(id), .. } => p.book.todos.iter().find(|task| task.id == id).map(|task| task.text.as_str()),
            Self::Note { id: Some(id), .. } => p.notes.note(id).map(|note| note.text.as_str()),
            _ => None,
        }
    }
}

pub(super) struct Completion {
    pub target: Target,
    pub result: Result<(), SaveError<String>>,
}

/// Content can commit before a rename/index fails, so receipts retain its id.
struct NoteAttempt {
    id: Option<u64>,
    result: Result<(), SaveError<String>>,
}

/// The receipt retains the target id even if later filename/index work fails.
pub(super) fn write(
    model: &mut Productivity,
    cx: &mut Context<Productivity>,
    mut target: Target,
    text: String,
    original: Option<String>,
    retry: bool,
    root: Option<&std::path::Path>,
) -> Result<async_channel::Receiver<Completion>, SaveError<String>> {
    if !target.ready(model) { return Err(SaveError::Store(mandara_i18n::t("Loading…").to_owned())); }
    if root.is_some_and(|root| root != model.notes.root) { return Err(SaveError::Changed); }
    let (reply, result) = async_channel::bounded(1);
    match &mut target {
        Target::Task { id, list } => {
            let reservation = model.reserve_todos().map_err(SaveError::Store)?;
            let changed = if let Some(id) = *id {
                model.edit_lists(cx, |book| book.edit_task(id, &text)).map_err(|error| SaveError::Store(tr(error.message())))
            } else {
                model.add_todo(&text, *list, cx).map(|(created, _)| { *id = Some(created); }).ok_or_else(|| SaveError::Store(t("A task needs text after its list name.").into()))
            };
            if let Err(error) = changed { model.cancel_todos(reservation); return Err(error); }
            let pending = model.commit_todos(reservation, cx);
            mandara_services::runtime::spawn("desktop-task-save", async move {
                let result = match pending.recv().await {
                    Ok(result) => result.map_err(SaveError::Store),
                    Err(error) => Err(SaveError::Store(error.to_string())),
                };
                let _ = reply.send(Completion { target, result }).await;
            });
        }
        Target::Note { id, topic } => {
            let (id, topic) = (*id, *topic);
            let pending = model.edit_notes(cx, move |notes| {
                Ok(write_note(notes, id, topic, &text, original.as_deref(), retry))
            }).map_err(SaveError::Store)?;
            mandara_services::runtime::spawn("desktop-note-save", async move {
                let done = match pending.recv().await {
                    Ok(done) => match done.value {
                        Ok(attempt) => Completion {
                            target: Target::Note { id: attempt.id, topic },
                            result: attempt.result.and_then(|()| done.index_error.map_or(Ok(()), |error| Err(SaveError::Store(error)))),
                        },
                        Err(error) => Completion { target, result: Err(SaveError::Store(error)) },
                    },
                    Err(error) => Completion { target, result: Err(SaveError::Store(error.to_string())) },
                };
                let _ = reply.send(done).await;
            });
        }
    }
    Ok(result)
}

/// Refreshes the target before comparing text; retries must not skip final metadata work.
fn write_note(notes: &mut crate::notes_store::NoteStore, id: Option<u64>, topic: u64, text: &str, original: Option<&str>, retry: bool) -> NoteAttempt {
    let mut attempt = NoteAttempt { id, result: Ok(()) };
    attempt.result = (|| {
        if let Some(id) = id {
            if notes.note(id).is_some() {
                notes.refresh_note(id).map_err(|error| SaveError::Store(format!("{error:#}")))?;
            }
            match notes.note(id) {
                None if retry && text.trim().is_empty() => return Ok(()),
                None => return Err(SaveError::Removed),
                Some(note) if Some(note.text.as_str()) != original && note.text != text => return Err(SaveError::Changed),
                _ => {}
            }
            if text.trim().is_empty() {
                notes.delete(id).map_err(|error| SaveError::Store(format!("{error:#}")))?;
            } else {
                notes.write(id, text).map_err(|error| SaveError::Store(format!("{error:#}")))?;
                notes.settle(id).map_err(|error| SaveError::Store(format!("{error:#}")))?;
            }
        } else {
            attempt.id = Some(notes.create(topic, text).map_err(|error| SaveError::Store(format!("{error:#}")))?);
        }
        Ok(())
    })();
    attempt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes_store::{DEFAULT_TOPIC, INDEX_FILE, NoteStore};

    #[test]
    fn partial_index_failure_keeps_created_id_for_retry_without_duplicate_files() {
        let root = tempfile::tempdir().unwrap();
        let mut notes = NoteStore::open(root.path());
        std::fs::remove_file(root.path().join(INDEX_FILE)).unwrap();
        std::fs::create_dir(root.path().join(INDEX_FILE)).unwrap();
        let first = write_note(&mut notes, None, DEFAULT_TOPIC, "# New note\nKeep this", None, false);
        let id = first.id.expect("committed content has an id even when the index fails");
        assert!(first.result.is_ok());
        assert!(notes.index_result().is_err());
        assert_eq!(notes.notes.len(), 1);
        std::fs::remove_dir(root.path().join(INDEX_FILE)).unwrap();
        let retry = write_note(&mut notes, Some(id), DEFAULT_TOPIC, "# New note\nKeep this", None, true);
        assert!(retry.result.is_ok());
        notes.persist_index().unwrap();
        assert_eq!(notes.notes.len(), 1);
        assert_eq!(NoteStore::open(root.path()).note(id).unwrap().text, "# New note\nKeep this");
    }

    #[test]
    fn worker_rechecks_conflicts_and_does_not_replace_newer_text() {
        let root = tempfile::tempdir().unwrap();
        let mut notes = NoteStore::open(root.path());
        let id = notes.create(DEFAULT_TOPIC, "Original").unwrap();
        notes.write(id, "Changed elsewhere").unwrap();
        let done = write_note(&mut notes, Some(id), DEFAULT_TOPIC, "My draft", Some("Original"), false);
        assert_eq!(done.result, Err(SaveError::Changed));
        assert_eq!(notes.note(id).unwrap().text, "Changed elsewhere");
    }

    #[test]
    fn external_file_edits_are_checked_before_the_desktop_write() {
        let root = tempfile::tempdir().unwrap();
        let mut notes = NoteStore::open(root.path());
        let id = notes.create(DEFAULT_TOPIC, "Original").unwrap();
        let path = root.path().join(notes.note(id).unwrap().path.as_ref().unwrap());
        std::fs::write(&path, "External editor").unwrap();
        let done = write_note(&mut notes, Some(id), DEFAULT_TOPIC, "My draft", Some("Original"), false);
        assert_eq!(done.result, Err(SaveError::Changed));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "External editor");
    }

    #[test]
    fn a_failed_rename_retries_even_when_the_body_already_matches() {
        let root = tempfile::tempdir().unwrap();
        let mut notes = NoteStore::open(root.path());
        let id = notes.create(DEFAULT_TOPIC, "Original title").unwrap();
        let old_path = notes.note(id).unwrap().path.clone().unwrap();
        notes.topics.iter_mut().find(|topic| topic.id == DEFAULT_TOPIC).unwrap().dir = "blocked".into();
        std::fs::write(root.path().join("blocked"), b"not a directory").unwrap();
        let first = write_note(&mut notes, Some(id), DEFAULT_TOPIC, "Changed title\nNew body", Some("Original title"), false);
        assert!(matches!(first.result, Err(SaveError::Store(_))));
        assert_eq!(notes.note(id).unwrap().text, "Changed title\nNew body");
        assert_eq!(notes.note(id).unwrap().path.as_deref(), Some(old_path.as_str()));
        std::fs::remove_file(root.path().join("blocked")).unwrap();
        notes.topics.iter_mut().find(|topic| topic.id == DEFAULT_TOPIC).unwrap().dir.clear();
        let retry = write_note(&mut notes, Some(id), DEFAULT_TOPIC, "Changed title\nNew body", Some("Original title"), true);
        assert_eq!(retry.result, Ok(()));
        assert_eq!(notes.note(id).unwrap().path.as_deref(), Some("Changed title.md"));
        assert!(!root.path().join(old_path).exists());
        assert_eq!(notes.notes.len(), 1);
    }

    #[test]
    fn a_committed_delete_can_retry_only_the_failed_index() {
        let root = tempfile::tempdir().unwrap();
        let mut notes = NoteStore::open(root.path());
        let id = notes.create(DEFAULT_TOPIC, "Delete me").unwrap();
        std::fs::remove_file(root.path().join(INDEX_FILE)).unwrap();
        std::fs::create_dir(root.path().join(INDEX_FILE)).unwrap();
        let first = write_note(&mut notes, Some(id), DEFAULT_TOPIC, "", Some("Delete me"), false);
        assert_eq!(first.result, Ok(()));
        assert!(notes.index_result().is_err());
        assert!(notes.note(id).is_none());
        std::fs::remove_dir(root.path().join(INDEX_FILE)).unwrap();
        let retry = write_note(&mut notes, Some(id), DEFAULT_TOPIC, "", Some("Delete me"), true);
        assert_eq!(retry.result, Ok(()));
        notes.persist_index().unwrap();
        assert!(NoteStore::open(root.path()).notes.is_empty());
    }
}
