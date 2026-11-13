//! Restore generated fragments after a rejected include or reload.

use anyhow::{Result, ensure};
use mandara_theme::templates::{FileStamp, WriteLock};
use std::path::{Path, PathBuf};

pub(super) enum Snapshot {
    Absent,
    File(Vec<u8>),
    Link {
        target: PathBuf,
        resolved: PathBuf,
        bytes: Vec<u8>,
    },
}
impl Snapshot {
    pub fn capture(path: &Path) -> Result<Self> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => Ok(Self::Link {
                target: std::fs::read_link(path)?,
                resolved: path.canonicalize()?,
                bytes: crate::app_themes::apply::read_text(path)?.into_bytes(),
            }),
            Ok(_) => Ok(Self::File(
                crate::app_themes::apply::read_text(path)?.into_bytes(),
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::Absent),
            Err(error) => Err(error.into()),
        }
    }
    pub fn text(&self) -> Option<&str> {
        match self { Self::Absent => None, Self::File(bytes) | Self::Link { bytes, .. } => std::str::from_utf8(bytes).ok() }
    }

    pub fn restore(&self, path: &Path, written: Option<&Written>) -> Result<()> {
        let Some(written) = written else {
            return Ok(());
        };
        let lock = WriteLock::new(path)?;
        ensure!(
            lock.target() == written.target && std::fs::read_link(path).ok() == written.link,
            "rollback conflict: compositor configuration changed; newer settings were kept"
        );
        lock.check_stamp(Some(written.stamp))?;
        match self {
            Self::File(bytes) => mandara_config::store::write_atomic(path, bytes),
            Self::Absent => match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            },
            Self::Link {
                target,
                resolved,
                bytes,
            } => {
                if written.target == *resolved {
                    mandara_config::store::write_atomic(resolved, bytes)?;
                }
                if std::fs::read_link(path).is_ok_and(|current| current == *target) {
                    return Ok(());
                }
                match std::fs::remove_file(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
                std::os::unix::fs::symlink(target, path)?;
                Ok(())
            }
        }
    }
}
/// Receipt for a known write, checked before restoring captured bytes.
pub(super) struct Written {
    target: PathBuf,
    link: Option<PathBuf>,
    stamp: FileStamp,
}
impl Written {
    pub fn capture(path: &Path, expected: &str) -> Result<Self> {
        let lock = WriteLock::new(path)?;
        let stamp = lock
            .check(expected.as_bytes())?
            .ok_or_else(|| anyhow::anyhow!("written compositor fragment is missing"))?;
        Ok(Self {
            target: lock.target().into(),
            link: std::fs::read_link(path).ok(),
            stamp,
        })
    }
    pub fn atomic(lock: &WriteLock, bytes: &[u8]) -> Result<Self> {
        let stamp = mandara_config::store::write_atomic_locked(lock, bytes)?;
        Ok(Self {
            target: lock.target().into(),
            link: std::fs::read_link(lock.path()).ok(),
            stamp,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rollback_keeps_newer_bytes_replaced_inodes_and_retargeted_links() {
        let root = tempfile::tempdir().unwrap();
        for kind in ["file", "link", "absent"] {
            let path = root.path().join(kind);
            let target = root.path().join(format!("{kind}-target"));
            if kind != "absent" {
                std::fs::write(&target, "original").unwrap();
                if kind == "link" {
                    std::os::unix::fs::symlink(&target, &path).unwrap();
                } else {
                    std::fs::write(&path, "original").unwrap();
                }
            }
            let snapshot = Snapshot::capture(&path).unwrap();
            mandara_config::store::write_atomic(&path, b"rejected").unwrap();
            let written = Written::capture(&path, "rejected").unwrap();
            std::fs::write(&path, "newer personal edit").unwrap();
            assert!(snapshot.restore(&path, Some(&written)).is_err());
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "newer personal edit"
            );
            if kind == "link" {
                assert_eq!(std::fs::read_link(path).unwrap(), target);
            }
        }
    }

    #[test]
    fn rollback_restores_contents_written_through_an_unchanged_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("link");
        std::fs::write(&target, "original").unwrap();
        std::os::unix::fs::symlink("target", &link).unwrap();
        let snapshot = Snapshot::capture(&link).unwrap();
        mandara_config::store::write_atomic(&link, b"rejected").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "rejected");
        let written = Written::capture(&link, "rejected").unwrap();
        snapshot.restore(&link, Some(&written)).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
        assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("target"));
    }

    #[test]
    fn rollback_restores_absent_files_and_the_original_symlink_without_touching_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fragment");
        let absent = Snapshot::capture(&path).unwrap();
        std::fs::write(&path, "new").unwrap();
        let written = Written::capture(&path, "new").unwrap();
        absent.restore(&path, Some(&written)).unwrap();
        assert!(!path.exists());
        let target = dir.path().join("original");
        std::fs::write(&target, "user").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let original = Snapshot::capture(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "rejected").unwrap();
        let written = Written::capture(&path, "rejected").unwrap();
        std::fs::write(&target, "newer target edit").unwrap();
        original.restore(&path, Some(&written)).unwrap();
        assert_eq!(std::fs::read_link(&path).unwrap(), target);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "newer target edit");
    }
}
