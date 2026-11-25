//! Restore one owned file while retaining interrupted work for a retry.

use super::{Ledger, SyncReport, exists, remove_empty};
use std::io;
use std::path::Path;

impl Ledger {
    pub(super) fn restore_file(&mut self, key: &str, changed: &mut SyncReport) -> io::Result<()> {
        self.restore_file_with(key, changed, super::super::file_io::sync_parent)
    }

    fn restore_file_with(
        &mut self,
        key: &str,
        changed: &mut SyncReport,
        sync_parent: impl FnOnce(&Path) -> io::Result<()>,
    ) -> io::Result<()> {
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
                && super::super::fingerprint::matches(path, &entry.hash)?;
            if unchanged {
                std::fs::remove_file(path)?;
                changed.files.push(path.into());
            }
            let mut changed_path = unchanged;
            if let Some(backup) = &entry.backup
                && !exists(path)
                && exists(backup)
            {
                super::super::restore::no_clobber(backup, path)?;
                changed_path = true;
                changed.files.push(path.into());
            }
            if changed_path {
                sync_parent(path)?;
            }
            remove_empty(&entry.created_dirs);
            Ok::<_, io::Error>(())
        })();
        if let Err(error) = result {
            self.files.insert(key.into(), entry);
            return Err(error);
        }
        self.finish_file()
    }
}

#[cfg(test)]
mod tests;
