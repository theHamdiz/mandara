//! Retain a known publication when a later persistence step fails.

use super::FileStamp;
use std::{
    fmt, io,
    path::{Path, PathBuf},
};

/// The inode came from the temporary file, rather than a later path lookup.
#[derive(Clone, Debug)]
pub struct PublishedWrite {
    pub target: PathBuf,
    pub stamp: FileStamp,
}

/// Replacement succeeded, but the caller must still handle the reported error.
#[derive(Debug)]
pub struct PublishedWriteError {
    receipt: PublishedWrite,
    source: io::Error,
}
impl PublishedWriteError {
    pub fn new(target: &Path, stamp: FileStamp, source: io::Error) -> Self {
        Self {
            receipt: PublishedWrite {
                target: target.into(),
                stamp,
            },
            source,
        }
    }
}
impl fmt::Display for PublishedWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} was published but persistence could not be confirmed: {}",
            self.receipt.target.display(),
            self.source
        )
    }
}
impl std::error::Error for PublishedWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// IO errors can wrap this receipt without exposing it through `source`.
pub fn published_write<'a>(
    error: &'a (dyn std::error::Error + 'static),
) -> Option<&'a PublishedWrite> {
    if let Some(error) = error.downcast_ref::<PublishedWriteError>() {
        return Some(&error.receipt);
    }
    if let Some(error) = error.downcast_ref::<io::Error>()
        && let Some(inner) = error.get_ref()
        && let Some(receipt) = published_write(inner)
    {
        return Some(receipt);
    }
    error.source().and_then(published_write)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipts_survive_io_wrappers_and_context_with_their_original_target() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("theme");
        let receipt = super::super::file_io::write_recorded(&path, "published")
            .unwrap()
            .unwrap();
        let error = anyhow::Error::new(io::Error::other(PublishedWriteError::new(
            &receipt.target,
            receipt.stamp,
            io::Error::other("directory sync failed"),
        )))
        .context("saving a theme");
        let recovered = published_write(error.as_ref()).unwrap();
        assert_eq!(recovered.target, path);
        assert_eq!(FileStamp::at(&path).unwrap(), Some(recovered.stamp));
        assert!(published_write(&io::Error::other("nothing was published")).is_none());
    }
}
