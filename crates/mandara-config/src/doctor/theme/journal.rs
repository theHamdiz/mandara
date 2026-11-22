//! A durable record of a shortcut transaction, so a crash between publishing
//! its files cannot leave a half-applied set behind.
//!
//! The journal sits beside the native file as `<name>.mandara-txn`. It is
//! written (atomically, with the directory synced) after the recovery copies
//! exist and before the first file is published, and it learns the hash of
//! every body *before* that body is published. A process that dies mid-batch
//! therefore leaves a journal whose hashes include whatever is on disk.
//! [`recover_interrupted`] restores each file that still holds one of those
//! bodies from its recovery copy; a file that holds anything else (the user's
//! own edit made since) is left alone.
//!
//! Finishing the transaction, by success or by a complete rollback, removes
//! the journal. A rollback that could not restore a file keeps it, so the next
//! transaction retries.

use anyhow::{Context as _, Result, bail, ensure};
use mandara_theme::templates::{FileStamp, WriteLock, fnv1a};
use serde::{Deserialize, Serialize};
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// Config files are read in full only up to this size; a larger file cannot
/// be one of Mandara's shortcut bodies.
const MAX_BYTES: u64 = 8 * 1024 * 1024;
const SUFFIX: &str = "mandara-txn";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    /// The config path as the transaction addressed it (a symlink stays one).
    path: PathBuf,
    /// The copy taken before the transaction; `None` when the file did not exist.
    backup: Option<PathBuf>,
    /// Hash of the file as it was; `None` when it did not exist.
    original: Option<String>,
    /// Hashes of every body the transaction may have published.
    published: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    files: Vec<Entry>,
}

pub(super) fn hash(bytes: &[u8]) -> String {
    format!("{:016x}", fnv1a(bytes))
}

/// Where the journal of a transaction whose first file is `native` lives.
pub fn journal_path(native: &Path) -> PathBuf {
    let name = native.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    native.with_file_name(format!("{name}.{SUFFIX}"))
}

/// An open journal for one running transaction.
pub(super) struct Journal {
    path: PathBuf,
    record: Record,
}

impl Journal {
    /// Records the files about to change, durably, before any is published.
    pub(super) fn begin(native: &Path, files: Vec<(PathBuf, Option<PathBuf>, Option<String>)>) -> Result<Self> {
        let journal = Self {
            path: journal_path(native),
            record: Record {
                files: files
                    .into_iter()
                    .map(|(path, backup, original)| Entry { path, backup, original, published: Vec::new() })
                    .collect(),
            },
        };
        journal.store()?;
        Ok(journal)
    }

    /// Makes `body` recoverable for file `index`; call before publishing it.
    pub(super) fn will_publish(&mut self, index: usize, body: &[u8]) -> Result<()> {
        let hash = hash(body);
        let entry = &mut self.record.files[index];
        if !entry.published.contains(&hash) {
            entry.published.push(hash);
            self.store()?;
        }
        Ok(())
    }

    fn store(&self) -> Result<()> {
        let text = toml::to_string(&self.record).context("encoding the shortcut transaction journal")?;
        crate::store::write_atomic(&self.path, text.as_bytes())
            .with_context(|| format!("writing the shortcut transaction journal {}", self.path.display()))
    }

    /// The transaction ended consistently: nothing is left to recover.
    pub(super) fn finish(self) -> Result<()> {
        remove(&self.path)
    }
}

fn remove(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("removing the shortcut transaction journal {}", path.display())),
    }
    let parent = path.parent().context("journal has no parent directory")?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

fn read_bounded(path: &Path) -> Result<Option<Vec<u8>>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_BYTES, "{} is too large", path.display());
    Ok(Some(bytes))
}

/// Finishes a transaction that was interrupted (the process died, or a
/// rollback could not complete) after it began publishing files. `native` is
/// the first file of that transaction. Returns whether anything was undone.
///
/// Call it before starting another transaction on the same files; it is
/// cheap when no journal exists.
pub fn recover_interrupted(native: &Path) -> Result<bool> {
    let path = journal_path(native);
    let Some(bytes) = read_bounded(&path)? else {
        return Ok(false);
    };
    let record: Record = std::str::from_utf8(&bytes)
        .map_err(anyhow::Error::from)
        .and_then(|text| toml::from_str(text).map_err(anyhow::Error::from))
        .with_context(|| format!("{} is not a valid shortcut transaction journal; check the files it names, then delete it", path.display()))?;
    ensure!(record.files.len() <= super::transaction::MAX_FILES, "{} lists too many files", path.display());
    let Some(first) = record.files.first() else {
        remove(&path)?;
        return Ok(false);
    };
    let (first_lock, second_lock) = WriteLock::pair(&first.path, record.files.get(1).map(|entry| entry.path.as_path()))?;
    let locks = [Some(first_lock), second_lock];
    let mut undone = false;
    let mut failure = None;
    for (entry, lock) in record.files.iter().zip(&locks).rev() {
        let lock = lock.as_ref().context("the transaction journal and its locks disagree")?;
        match restore(lock, entry) {
            Ok(restored) => undone |= restored,
            Err(error) => {
                failure.get_or_insert(error.context(format!("could not undo an interrupted shortcut change to {}", entry.path.display())));
            }
        }
    }
    if let Some(error) = failure {
        // Keep the journal: the next attempt retries what is left.
        return Err(error);
    }
    remove(&path)?;
    Ok(undone)
}

/// Puts `entry` back when it still holds a body this transaction published.
fn restore(lock: &WriteLock, entry: &Entry) -> Result<bool> {
    let current = FileStamp::at(lock.target())?;
    let Some(bytes) = read_bounded(lock.target())? else {
        return Ok(false);
    };
    let now = hash(&bytes);
    if entry.original.as_deref() == Some(now.as_str()) || !entry.published.contains(&now) {
        // Untouched, or edited by someone else since: not ours to revert.
        return Ok(false);
    }
    match (&entry.backup, &entry.original) {
        (Some(backup), Some(original)) => {
            let Some(saved) = read_bounded(backup)? else {
                bail!("its recovery copy {} is missing", backup.display());
            };
            ensure!(&hash(&saved) == original, "its recovery copy {} no longer matches the original", backup.display());
            crate::store::write_atomic_checked(lock, current, &saved)?;
        }
        (None, None) => {
            lock.check_stamp(current)?;
            std::fs::remove_file(lock.target())?;
            let parent = lock.target().parent().context("config has no parent directory")?;
            std::fs::File::open(parent)?.sync_all()?;
        }
        _ => bail!("the journal entry is inconsistent"),
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn begin(native: &Path, generated: &Path, backups: [Option<&Path>; 2]) -> Journal {
        let files = [native, generated]
            .into_iter()
            .zip(backups)
            .map(|(path, backup)| {
                (path.to_path_buf(), backup.map(Path::to_path_buf), fs::read(path).ok().map(|bytes| hash(&bytes)))
            })
            .collect();
        Journal::begin(native, files).unwrap()
    }

    #[test]
    fn a_half_applied_pair_is_restored_from_the_recovery_copies() {
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("native");
        let generated = dir.path().join("generated");
        let native_backup = dir.path().join("native.bak");
        let generated_backup = dir.path().join("generated.bak");
        for (path, body) in [(&native, "old native"), (&generated, "old generated"), (&native_backup, "old native"), (&generated_backup, "old generated")] {
            fs::write(path, body).unwrap();
        }
        let mut journal = begin(&native, &generated, [Some(&native_backup), Some(&generated_backup)]);
        // The first file was published, then the process died.
        journal.will_publish(1, b"bridge").unwrap();
        journal.will_publish(0, b"new native").unwrap();
        fs::write(&native, "new native").unwrap();
        drop(journal);
        assert!(journal_path(&native).exists());
        assert!(recover_interrupted(&native).unwrap());
        assert_eq!(fs::read_to_string(&native).unwrap(), "old native");
        assert_eq!(fs::read_to_string(&generated).unwrap(), "old generated");
        assert!(!journal_path(&native).exists());
        assert!(!recover_interrupted(&native).unwrap(), "nothing left to undo");
    }

    #[test]
    fn a_body_published_just_before_the_crash_is_recovered_too() {
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("native");
        let generated = dir.path().join("generated");
        let backup = dir.path().join("generated.bak");
        fs::write(&native, "old native").unwrap();
        fs::write(&generated, "old generated").unwrap();
        fs::write(&backup, "old generated").unwrap();
        let mut journal = begin(&native, &generated, [None, Some(&backup)]);
        journal.will_publish(1, b"new generated").unwrap();
        // The crash came after the rename, before `publish` returned.
        fs::write(&generated, "new generated").unwrap();
        drop(journal);
        assert!(recover_interrupted(&native).unwrap());
        assert_eq!(fs::read_to_string(&generated).unwrap(), "old generated");
    }

    #[test]
    fn a_file_the_user_edited_since_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("native");
        let backup = dir.path().join("native.bak");
        fs::write(&native, "old").unwrap();
        fs::write(&backup, "old").unwrap();
        let mut journal = begin(&native, &native.with_file_name("unused"), [Some(&backup), None]);
        journal.will_publish(0, b"new").unwrap();
        fs::write(&native, "new").unwrap();
        fs::write(&native, "the user's own edit").unwrap();
        drop(journal);
        assert!(!recover_interrupted(&native).unwrap());
        assert_eq!(fs::read_to_string(&native).unwrap(), "the user's own edit");
        assert!(!journal_path(&native).exists());
    }

    #[test]
    fn a_file_the_transaction_created_is_removed_again() {
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("native");
        let generated = dir.path().join("generated");
        fs::write(&native, "old native").unwrap();
        let mut journal = begin(&native, &generated, [None, None]);
        journal.will_publish(1, b"created").unwrap();
        fs::write(&generated, "created").unwrap();
        drop(journal);
        assert!(recover_interrupted(&native).unwrap());
        assert!(!generated.exists());
        assert_eq!(fs::read_to_string(&native).unwrap(), "old native");
    }

    #[test]
    fn a_missing_recovery_copy_keeps_the_journal_and_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("native");
        let backup = dir.path().join("native.bak");
        fs::write(&native, "old").unwrap();
        let mut journal = begin(&native, &native.with_file_name("unused"), [Some(&backup), None]);
        journal.will_publish(0, b"new").unwrap();
        fs::write(&native, "new").unwrap();
        drop(journal);
        let error = recover_interrupted(&native).unwrap_err();
        assert!(format!("{error:#}").contains("recovery copy"), "{error:#}");
        assert_eq!(fs::read_to_string(&native).unwrap(), "new");
        assert!(journal_path(&native).exists());
    }

    #[test]
    fn a_finished_transaction_leaves_no_journal() {
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("native");
        fs::write(&native, "old").unwrap();
        let mut journal = begin(&native, &native.with_file_name("unused"), [None, None]);
        journal.will_publish(0, b"new").unwrap();
        journal.finish().unwrap();
        assert!(!journal_path(&native).exists());
        assert!(!recover_interrupted(&native).unwrap());
    }
}
