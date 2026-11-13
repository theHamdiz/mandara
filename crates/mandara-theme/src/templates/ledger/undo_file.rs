//! Restore one owned file while retaining interrupted work for a retry.

use super::{Ledger, SyncReport, exists, remove_empty};
use std::io;
use std::path::Path;

impl Ledger {
    pub(super) fn restore_file(&mut self, key: &str, changed: &mut SyncReport) -> io::Result<()> {
        let Some(entry) = self.files.get(key).cloned() else {
            return Ok(());
        };
        let path = Path::new(key);
        if entry.backup.as_ref().is_some_and(|backup| !exists(backup)) {
            changed.failed(path, "original backup is missing");
            return Ok(());
        }
        self.begin_restore(path, &entry)?;
        self.files.remove(key);
        self.dirty = true;
        let result = (|| {
            let unchanged = !std::fs::symlink_metadata(path)
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
                && super::super::fingerprint::matches(path, &entry.hash).unwrap_or(false);
            if unchanged {
                std::fs::remove_file(path)?;
                changed.files.push(path.into());
            }
            if let Some(backup) = &entry.backup
                && !exists(path)
                && exists(backup)
            {
                super::super::restore::no_clobber(backup, path)?;
                changed.files.push(path.into());
            }
            if unchanged {
                super::super::file_io::sync_parent(path)?;
            }
            remove_empty(&entry.created_dirs);
            Ok::<_, io::Error>(())
        })();
        if let Err(error) = result {
            changed.failed(path, error);
            self.files.insert(key.into(), entry);
        }
        self.finish_file()
    }
}
