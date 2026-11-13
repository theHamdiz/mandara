//! A published inode remains identifiable when its directory sync fails.

use mandara_theme::templates::FileStamp;
pub use mandara_theme::templates::PublishedWriteError;

/// Recover the receipt without converting a persistence error into success.
pub fn published_stamp(error: &anyhow::Error) -> Option<FileStamp> {
    mandara_theme::templates::published_write(error.as_ref()).map(|receipt| receipt.stamp)
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
}
