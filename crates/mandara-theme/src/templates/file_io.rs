//! Bounded reads and atomic writes for app preferences and generated themes.

use std::io::{Read as _, Write as _};
use std::path::Path;

/// Larger than a rendered template; user configs can contain unrelated settings.
const MAX_CONFIG_BYTES: u64 = 8 * 1024 * 1024;

pub(super) fn read(path: &Path) -> std::io::Result<String> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && std::fs::symlink_metadata(path).is_err() => return Ok(String::new()),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() { return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput,"app theme target is not a regular file")); }
    let mut body = String::new();
    std::fs::File::open(path)?.take(MAX_CONFIG_BYTES+1).read_to_string(&mut body)?;
    if body.len() as u64 > MAX_CONFIG_BYTES { return Err(std::io::Error::new(std::io::ErrorKind::InvalidData,"app theme config is too large")); }
    Ok(body)
}

pub(super) fn write(path: &Path, body: &str) -> std::io::Result<()> {
    write_recorded(path, body).map(|_| ())
}

pub(super) fn write_recorded(path: &Path,body: &str) -> std::io::Result<Option<super::PublishedWrite>> {
    let lock = super::WriteLock::new(path)?;
    let before = super::FileStamp::at(lock.target())?;
    let target = if std::fs::symlink_metadata(path).is_ok_and(|meta|meta.file_type().is_symlink()) { std::fs::canonicalize(path)? } else { path.into() };
    if target.exists() && read(&target)? == body {
        std::fs::File::open(&target)?.sync_all()?;
        sync_parent(&target)?;
        return Ok(None);
    }
    let parent = target.parent().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput,"app theme target has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::Builder::new().prefix(".mandara-theme-").tempfile_in(parent)?;
    if let Ok(metadata) = std::fs::metadata(&target) { temporary.as_file().set_permissions(metadata.permissions())?; }
    temporary.write_all(body.as_bytes())?;
    temporary.as_file().sync_all()?;
    let stamp = super::FileStamp::of(temporary.as_file())?;
    lock.check_stamp(before)?;
    temporary.persist(&target).map_err(|error|error.error)?;
    sync_parent(&target).map_err(|error| std::io::Error::other(super::PublishedWriteError::new(&target, stamp, error)))?;
    Ok(Some(super::PublishedWrite { target, stamp }))
}

pub(super) fn sync_parent(path: &Path) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| std::io::Error::other("app theme target has no parent"))?;
    std::fs::File::open(parent)?.sync_all()
}


/// Persist a visible recovered file or link before finalizing its ownership.
pub(super) fn sync_recovered(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => std::fs::File::open(path)?.sync_all()?,
        Ok(metadata) if metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(std::io::ErrorKind::InvalidInput.into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    sync_existing_parent(path)
}

/// Recovery can finish after a created directory was already removed.
pub(super) fn sync_existing_parent(path: &Path) -> std::io::Result<()> {
    let mut parent = path.parent();
    while let Some(directory) = parent {
        match std::fs::File::open(directory) {
            Ok(file) => {
                if !file.metadata()?.is_dir() {
                    return Err(std::io::ErrorKind::NotADirectory.into());
                }
                return file.sync_all();
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                parent = directory.parent();
            }
            Err(error) => return Err(error),
        }
    }
    Err(std::io::ErrorKind::NotFound.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atomic_updates_keep_symlinks_permissions_and_unrelated_files() {
        use std::os::unix::fs::{PermissionsExt as _,symlink};
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real");
        let link = dir.path().join("link");
        std::fs::write(&target,"original").unwrap();
        std::fs::set_permissions(&target,std::fs::Permissions::from_mode(0o640)).unwrap();
        symlink(&target,&link).unwrap();
        write(&link,"updated").unwrap();
        assert_eq!(read(&link).unwrap(),"updated");
        assert_eq!(std::fs::read_link(&link).unwrap(),target);
        assert_eq!(std::fs::metadata(&link).unwrap().permissions().mode() & 0o777,0o640);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(),2);
        assert!(read(dir.path()).is_err());
        let empty = dir.path().join("empty");
        write(&empty, "").unwrap();
        assert!(empty.is_file());
        let broken = dir.path().join("broken");
        symlink(dir.path().join("missing"),&broken).unwrap();
        assert!(write(&broken,"new").is_err());
        assert!(std::fs::symlink_metadata(broken).unwrap().file_type().is_symlink());
    }
    #[test]
    fn recovery_sync_accepts_removed_directories_but_rejects_invalid_parents() {
        let root = tempfile::tempdir().unwrap();
        sync_existing_parent(&root.path().join("removed/nested/theme")).unwrap();
        let directory = root.path().join("app");
        std::fs::create_dir(&directory).unwrap();
        sync_existing_parent(&directory.join("theme")).unwrap();
        let file = root.path().join("file");
        std::fs::write(&file, "personal").unwrap();
        assert!(sync_existing_parent(&file.join("theme")).is_err());
        assert_eq!(std::fs::read_to_string(file).unwrap(), "personal");
    }

    #[test]
    fn recovery_sync_handles_files_missing_paths_and_unfollowed_links() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("theme");
        std::fs::write(&path, "palette").unwrap();
        sync_recovered(&path).unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink("missing-personal-target", &link).unwrap();
        sync_recovered(&link).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("missing-personal-target"));
        sync_recovered(&root.path().join("missing/nested/theme")).unwrap();
        assert!(sync_recovered(root.path()).is_err());
    }

}
