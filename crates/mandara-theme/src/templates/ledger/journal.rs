//! Durable ownership intent survives interruption before the next checkpoint.

use super::{BlockEntry, FileEntry, Ledger, exists};
use serde::{Deserialize, Serialize};
use std::io;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SlotStamp {
    // Opaque IDs keep all native bits while fitting TOML's signed integers.
    device: i64,
    inode: i64,
    length: u64,
    seconds: i64,
    nanos: i64,
}
impl SlotStamp {
    fn at(path: &Path) -> io::Result<Option<Self>> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => Ok(Some(Self {
                device: metadata.dev() as i64,
                inode: metadata.ino() as i64,
                length: metadata.len(),
                seconds: metadata.mtime(),
                nanos: metadata.mtime_nsec(),
            })),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation")]
pub(super) enum Pending {
    Apply {
        path: PathBuf,
        next: FileEntry,
        previous: Option<FileEntry>,
        original: Option<SlotStamp>,
        moved: Option<PathBuf>,
    },
    Block {
        key: String,
        next: BlockEntry,
        previous: Option<BlockEntry>,
        original: Option<SlotStamp>,
    },
    Restore {
        path: PathBuf,
        entry: FileEntry,
        backup_stamp: Option<SlotStamp>,
    },
}

impl Pending {
    pub(super) fn owner(&self) -> &str {
        match self {
            Self::Apply { next, .. } => &next.app,
            Self::Block { next, .. } => &next.app,
            Self::Restore { entry, .. } => &entry.app,
        }
    }
}

pub(super) fn contains_block<'a>(body: &str, lines: impl Iterator<Item = &'a str> + Clone) -> bool {
    let count = lines.clone().count();
    if count == 0 {
        return false;
    }
    let mut source = body.lines();
    loop {
        if source
            .clone()
            .take(count)
            .map(str::trim)
            .eq(lines.clone().map(str::trim))
        {
            return true;
        }
        if source.next().is_none() {
            return false;
        }
    }
}

impl Ledger {
    pub(super) fn begin_file(
        &mut self,
        key: &str,
        next: &FileEntry,
        moved: Option<&Path>,
    ) -> io::Result<()> {
        if self.record.is_none() {
            return Ok(());
        }
        self.pending = Some(Box::new(Pending::Apply {
            path: key.into(),
            next: next.clone(),
            previous: self.files.get(key).cloned(),
            original: SlotStamp::at(Path::new(key))?,
            moved: moved.map(Path::to_path_buf),
        }));
        self.dirty = true;
        self.checkpoint()
    }

    pub(super) fn begin_block(&mut self, key: &str, next: &BlockEntry) -> io::Result<()> {
        if self.record.is_none() {
            return Ok(());
        }
        let path = next.path.as_deref().unwrap_or_else(|| Path::new(key));
        self.pending = Some(Box::new(Pending::Block {
            key: key.into(),
            next: next.clone(),
            previous: self.blocks.get(key).cloned(),
            original: SlotStamp::at(path)?,
        }));
        self.dirty = true;
        self.checkpoint()
    }

    pub(super) fn begin_restore(&mut self, path: &Path, entry: &FileEntry) -> io::Result<()> {
        if self.record.is_none() {
            return Ok(());
        }
        let backup_stamp = entry
            .backup
            .as_deref()
            .map(SlotStamp::at)
            .transpose()?
            .flatten();
        self.pending = Some(Box::new(Pending::Restore {
            path: path.into(),
            entry: entry.clone(),
            backup_stamp,
        }));
        self.dirty = true;
        self.checkpoint()
    }

    pub(super) fn finish_file(&mut self) -> io::Result<()> {
        let Some(pending) = self.pending.take() else {
            return Ok(());
        };
        self.dirty = true;
        if let Err(error) = self.checkpoint() {
            self.pending = Some(pending);
            self.dirty = true;
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn settle(&mut self) -> io::Result<()> {
        let Some(pending) = &self.pending else {
            return Ok(());
        };
        if let Pending::Block {
            key,
            next,
            previous,
            original,
        } = pending.as_ref()
        {
            let path = next.path.as_deref().unwrap_or_else(|| Path::new(key));
            let mut entry = if SlotStamp::at(path)? == *original {
                if original.is_none() && next.created {
                    Some(BlockEntry {
                        lines: Vec::new(),
                        ..next.clone()
                    })
                } else {
                    previous.clone()
                }
            } else {
                let body = super::super::file_io::read(path)?;
                if contains_block(&body, next.lines.iter().map(String::as_str)) {
                    Some(next.clone())
                } else if let Some(previous) = previous
                    && contains_block(&body, previous.lines.iter().map(String::as_str))
                {
                    Some(previous.clone())
                } else {
                    let mut recovered = next.clone();
                    if let Some(previous) = previous {
                        recovered.lines.extend(
                            previous
                                .lines
                                .iter()
                                .filter(|line| !next.lines.contains(line))
                                .cloned(),
                        );
                    }
                    recovered.lines.retain(|line| {
                        !line.trim().is_empty()
                            && !line.trim_start().starts_with('[')
                            && body.lines().any(|actual| actual.trim() == line.trim())
                    });
                    Some(recovered)
                }
            };
            if entry
                .as_ref()
                .is_some_and(|entry| entry.lines.is_empty() && !entry.created)
            {
                entry = None;
            }
            let key = key.clone();
            if let Some(entry) = entry {
                self.blocks.insert(key, entry);
            } else {
                self.blocks.remove(&key);
            }
            return self.finish_file();
        }
        let (path, entry) = match pending.as_ref() {
            Pending::Apply {
                path,
                next,
                previous,
                original,
                moved,
            } => {
                let current = SlotStamp::at(path)?;
                let untouched = &current == original;
                let entry = match moved {
                    Some(backup) if exists(backup) => Some(next.clone()),
                    Some(_) if untouched => previous.clone(),
                    Some(_) => {
                        return Err(io::Error::other(
                            "an interrupted theme write is missing its original backup",
                        ));
                    }
                    None if untouched && original.is_some() => previous.clone(),
                    None if previous
                        .as_ref()
                        .map(|entry| super::super::fingerprint::matches(path, &entry.hash))
                        .transpose()?
                        .unwrap_or(false) =>
                    {
                        previous.clone()
                    }
                    None => Some(next.clone()),
                };
                (path.clone(), entry)
            }
            Pending::Block { .. } => unreachable!("block recovery returned above"),
            Pending::Restore {
                path,
                entry,
                backup_stamp,
            } => {
                let current = SlotStamp::at(path)?;
                let restored = match &entry.backup {
                    Some(_) if backup_stamp.is_some() && &current == backup_stamp => true,
                    Some(backup) if exists(backup) => false,
                    Some(_) => {
                        return Err(io::Error::other(
                            "an interrupted theme restore is missing its original backup",
                        ));
                    }
                    None => current.is_none(),
                };
                if restored {
                    super::super::file_io::sync_existing_parent(path)?;
                    super::remove_empty(&entry.created_dirs);
                }
                (path.clone(), (!restored).then(|| entry.clone()))
            }
        };
        let key = super::key(&path);
        if let Some(entry) = entry {
            self.files.insert(key, entry);
        } else {
            self.files.remove(&key);
        }
        self.finish_file()
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod block_tests;

#[cfg(test)]
mod creation_tests;

#[cfg(test)]
mod crash_tests;
