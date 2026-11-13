//! Restore a moved original without replacing a newer destination.

use std::path::Path;
use std::{fs, io};

pub(super) fn no_clobber(backup: &Path, destination: &Path) -> io::Result<()> {
    // Sibling hard links preserve the original inode, including symlinks on
    // Linux and FreeBSD, so recovery recognizes a completed restore.
    fs::hard_link(backup, destination)?;
    fs::remove_file(backup)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt as _;
    #[test]
    fn restores_files_and_links_without_clobbering_a_new_destination() {
        let root = tempfile::tempdir().unwrap();
        for link in [false, true] {
            let original = root.path().join(if link { "link" } else { "file" });
            let backup = original.with_extension("backup");
            if link {
                std::os::unix::fs::symlink("personal", &backup).unwrap();
            } else {
                fs::write(&backup, "personal").unwrap();
            }
            let inode = fs::symlink_metadata(&backup).unwrap().ino();
            fs::write(&original, "new edit").unwrap();
            assert_eq!(
                no_clobber(&backup, &original).unwrap_err().kind(),
                io::ErrorKind::AlreadyExists
            );
            assert_eq!(fs::read_to_string(&original).unwrap(), "new edit");
            assert!(fs::symlink_metadata(&backup).is_ok());
            fs::remove_file(&original).unwrap();
            no_clobber(&backup, &original).unwrap();
            assert_eq!(fs::symlink_metadata(&original).unwrap().ino(), inode);
            if link {
                assert_eq!(fs::read_link(&original).unwrap(), Path::new("personal"));
            } else {
                assert_eq!(fs::read_to_string(&original).unwrap(), "personal");
            }
            assert!(fs::symlink_metadata(backup).is_err());
        }
    }
}
