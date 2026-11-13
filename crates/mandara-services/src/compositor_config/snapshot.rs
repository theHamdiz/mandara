//! Restore generated fragments after a rejected include or reload.

use anyhow::Result;
use std::path::{Path,PathBuf};

pub(super) enum Snapshot { Absent,File(Vec<u8>),Link(PathBuf) }
impl Snapshot {
    pub fn capture(path: &Path) -> Result<Self> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => Ok(Self::Link(std::fs::read_link(path)?)),
            Ok(_) => Ok(Self::File(crate::app_themes::apply::read_text(path)?.into_bytes())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::Absent),
            Err(error) => Err(error.into()),
        }
    }
    pub fn restore(&self,path: &Path) -> Result<()> {
        match self {
            Self::File(bytes) => mandara_config::store::write_atomic(path,bytes),
            Self::Absent => match std::fs::remove_file(path) { Ok(()) => Ok(()),Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),Err(error) => Err(error.into()) },
            Self::Link(target) => {
                if std::fs::read_link(path).is_ok_and(|current|current == *target) { return Ok(()); }
                match std::fs::remove_file(path) { Ok(()) => {},Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},Err(error) => return Err(error.into()) }
                std::os::unix::fs::symlink(target,path)?;
                Ok(())
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rollback_restores_absent_files_and_the_original_symlink_without_touching_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fragment");
        let absent = Snapshot::capture(&path).unwrap();
        std::fs::write(&path,"new").unwrap();
        absent.restore(&path).unwrap();
        assert!(!path.exists());
        let target = dir.path().join("original");
        std::fs::write(&target,"user").unwrap();
        std::os::unix::fs::symlink(&target,&path).unwrap();
        let original = Snapshot::capture(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path,"rejected").unwrap();
        original.restore(&path).unwrap();
        assert_eq!(std::fs::read_link(&path).unwrap(),target);
        assert_eq!(std::fs::read_to_string(&path).unwrap(),"user");
    }
}
