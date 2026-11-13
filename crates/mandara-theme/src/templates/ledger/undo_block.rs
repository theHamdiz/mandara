//! Retire an owned import only after its removal is durable.

use super::{Ledger, SyncReport, exists, remove_empty};
use std::io;
use std::path::Path;

impl Ledger {
    pub(super) fn restore_block(&mut self, key: String, changed: &mut SyncReport) {
        self.restore_block_with(key, changed, super::super::file_io::sync_recovered);
    }

    fn restore_block_with(
        &mut self,
        key: String,
        changed: &mut SyncReport,
        sync_removed: impl FnOnce(&Path) -> io::Result<()>,
    ) {
        let Some(entry) = self.blocks.remove(&key) else {
            return;
        };
        self.dirty = true;
        let path = entry.path.as_deref().unwrap_or_else(|| Path::new(&key));
        let result = (|| {
            let lines: Vec<&str> = entry.lines.iter().map(String::as_str).collect();
            let removed = super::super::remove_block(path, &lines)?;
            let mut needs_sync = !removed;
            if entry.created {
                if !exists(path) {
                    remove_empty(&entry.created_dirs);
                    needs_sync = true;
                } else if super::super::file_io::read(path)?.trim().is_empty() {
                    std::fs::remove_file(path)?;
                    remove_empty(&entry.created_dirs);
                    needs_sync = true;
                }
            }
            if needs_sync {
                sync_removed(path)?;
            }
            Ok::<_, io::Error>(removed)
        })();
        let removed = match result {
            Ok(removed) => removed,
            Err(error) => {
                changed.failed(path, error);
                self.blocks.insert(key, entry);
                return;
            }
        };
        if removed {
            changed.files.push(path.into());
        }
        if entry.created
            && let Some(other) = self.blocks.iter_mut().find_map(|(key, other)| {
                (other.path.as_deref().unwrap_or_else(|| Path::new(key)) == path).then_some(other)
            })
        {
            other.created = true;
            other.created_dirs.clone_from(&entry.created_dirs);
        }
    }
}

#[cfg(test)]
mod tests;
