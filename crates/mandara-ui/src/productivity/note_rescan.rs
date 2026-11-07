//! One retained folder reconciliation when the ordered worker is busy.

use std::path::Path;
use async_channel::{Receiver, Sender};
use super::notes_worker::{Completion, NotesWorker};

#[derive(Default)]
pub(super) struct NoteRescan {
    pending: bool,
}

impl NoteRescan {
    /// Repeated filesystem events need only one scan of the latest contents.
    pub fn request(&mut self) { self.pending = true; }

    /// A full queue leaves the request owned until a completion frees capacity.
    pub fn try_submit(
        &mut self,
        worker: &NotesWorker,
        root: &Path,
        changed: &Sender<()>,
    ) -> Result<Option<Receiver<Completion<()>>>, String> {
        if !self.pending { return Ok(None); }
        let incoming = worker.submit(root.to_owned(), changed.clone(), |store| {
            store.rescan(&[]);
            Ok(())
        })?;
        self.pending = false;
        Ok(Some(incoming))
    }
}
