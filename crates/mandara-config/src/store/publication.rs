//! A published inode remains identifiable when its directory sync fails.

use mandara_theme::templates::FileStamp;
pub use mandara_theme::templates::PublishedWriteError;
use std::path::Path;

/// Recover the receipt without converting a persistence error into success.
pub fn published_stamp(error: &anyhow::Error) -> Option<FileStamp> {
    mandara_theme::templates::published_write(error.as_ref()).map(|receipt| receipt.stamp)
}

pub(super) fn sync_directory(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

/// A matching read can acknowledge a failed publication only after its sync.
pub(super) fn confirm(
    lock: &mandara_theme::templates::WriteLock,
    expected: &[u8],
    sync_directory: impl FnOnce(&Path) -> std::io::Result<()>,
) -> anyhow::Result<()> {
    let stamp = lock
        .check(expected)?
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?;
    let file = std::fs::File::open(lock.target())?;
    anyhow::ensure!(
        FileStamp::of(&file)? == stamp,
        "configuration changed while it was being edited; try again"
    );
    file.sync_all()?;
    sync_directory(
        lock.target()
            .parent()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?,
    )?;
    lock.check_stamp(Some(stamp))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    #[test]
    fn directory_sync_failures_retain_the_published_inode_receipt() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        std::fs::write(&path, "original").unwrap();
        let lock = mandara_theme::templates::WriteLock::new(&path).unwrap();
        let before = FileStamp::at(&path).unwrap();
        let error = super::super::write_atomic_checked_using(&lock, before, b"published", |_| {
            Err(io::Error::other("injected directory sync failure"))
        })
        .unwrap_err();
        let receipt = published_stamp(&error).unwrap();
        assert_eq!(FileStamp::at(&path).unwrap(), Some(receipt));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "published");
        assert!(format!("{error:#}").contains("injected directory sync failure"));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
        std::fs::write(&path, "newer personal edit").unwrap();
        assert!(lock.check_stamp(Some(receipt)).is_err());
    }

    fn fail_sync(_: &Path) -> io::Result<()> {
        Err(io::Error::other("injected directory sync failure"))
    }

    #[test]
    fn a_matching_flush_retries_sync_before_clearing_dirty_without_republishing() {
        use super::super::ConfigStore;
        use std::os::unix::fs::MetadataExt as _;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        let mut store = ConfigStore::open(&path).unwrap();
        store.update(|config| config.bar.height = 41.0);
        assert!(store.flush_using(|| Ok(()), fail_sync).is_err());
        let inode = std::fs::metadata(&path).unwrap().ino();
        assert!(store.is_dirty());
        assert!(
            store
                .flush_using(
                    || panic!("matching contents must not be rewritten"),
                    fail_sync
                )
                .is_err()
        );
        assert!(store.is_dirty());
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
        let synced = std::cell::Cell::new(false);
        store
            .flush_using(
                || panic!("matching contents must not be rewritten"),
                |directory| {
                    assert_eq!(directory, root.path());
                    sync_directory(directory)?;
                    synced.set(true);
                    Ok(())
                },
            )
            .unwrap();
        assert!(synced.get());
        assert!(!store.is_dirty());
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
    }

    #[test]
    fn a_discarded_background_save_cannot_be_acknowledged_by_an_unsynced_reload() {
        use super::super::ConfigStore;
        use std::os::unix::fs::MetadataExt as _;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        let mut store = ConfigStore::open(&path).unwrap();
        store.update(|config| config.bar.height = 41.0);
        let mut failed = store.save_request().unwrap();
        assert!(failed.store.flush_using(|| Ok(()), fail_sync).is_err());
        drop(failed);
        let inode = std::fs::metadata(&path).unwrap().ino();
        let mut reload = store.reload_request();
        assert!(reload.store.reload_using(fail_sync).is_err());
        assert!(reload.store.is_dirty());
        drop(reload);
        let saved = store.save_request().unwrap().write().unwrap();
        store.finish_save(saved).unwrap();
        assert!(!store.is_dirty());
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
        assert_eq!(ConfigStore::open(&path).unwrap().get().bar.height, 41.0);
    }

    #[test]
    fn clean_external_reloads_do_not_require_a_publication_sync() {
        use super::super::ConfigStore;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        let mut reader = ConfigStore::open(&path).unwrap();
        let mut writer = ConfigStore::open(&path).unwrap();
        writer.update(|config| config.bar.height = 41.0);
        writer.flush().unwrap();
        reader
            .reload_using(|_| panic!("a clean reader has no failed save to confirm"))
            .unwrap();
        assert_eq!(reader.get().bar.height, 41.0);
        assert!(!reader.is_dirty());
    }

    #[test]
    fn a_clean_reload_queued_before_a_failed_save_cannot_acknowledge_it() {
        use super::super::ConfigStore;
        use std::os::unix::fs::MetadataExt as _;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        let mut store = ConfigStore::open(&path).unwrap();
        let reload = store.reload_request();
        store.update(|config| config.bar.height = 41.0);
        let mut failed = store.save_request().unwrap();
        assert!(failed.store.flush_using(|| Ok(()), fail_sync).is_err());
        drop(failed);
        let inode = std::fs::metadata(&path).unwrap().ino();
        store.finish_reload(reload.read().unwrap()).unwrap();
        assert_eq!(store.get().bar.height, 41.0);
        assert!(store.is_dirty());
        let mut retry = store
            .save_request()
            .expect("the save still needs acknowledgement");
        assert!(
            retry
                .store
                .flush_using(|| panic!("do not republish"), fail_sync)
                .is_err()
        );
        assert!(retry.store.is_dirty());
        let saved = store.save_request().unwrap().write().unwrap();
        store.finish_save(saved).unwrap();
        assert!(!store.is_dirty());
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
    }

    #[test]
    fn even_a_confirmed_reload_does_not_acknowledge_a_later_failed_publication() {
        use super::super::ConfigStore;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        let mut store = ConfigStore::open(&path).unwrap();
        store.update(|config| config.bar.height = 41.0);
        let mut failed = store.save_request().unwrap();
        assert!(failed.store.flush_using(|| Ok(()), fail_sync).is_err());
        let reloaded = store.reload_request().read().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let lock = mandara_theme::templates::WriteLock::new(&path).unwrap();
        let before = FileStamp::at(&path).unwrap();
        assert!(
            super::super::write_atomic_checked_using(&lock, before, &bytes, fail_sync).is_err()
        );
        assert_ne!(FileStamp::at(&path).unwrap(), before);
        drop(lock);
        store.finish_reload(reloaded).unwrap();
        assert!(store.is_dirty());
        let saved = store.save_request().unwrap().write().unwrap();
        store.finish_save(saved).unwrap();
        assert!(!store.is_dirty());
    }
}
