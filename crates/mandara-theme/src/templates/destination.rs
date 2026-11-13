//! Compare app-theme destinations through directory and symlink aliases.

use std::io;
use std::path::{Component, Path, PathBuf};

/// Resolve existing prefixes, then normalize missing suffixes without creating
/// directories. Resolve each component before `..`, as the filesystem does.
pub fn identity(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.into()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut result = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            component => {
                result.push(component.as_os_str());
                match result.canonicalize() {
                    Ok(resolved) => result = resolved,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        match std::fs::symlink_metadata(&result) {
                            Ok(_) => return Err(error),
                            Err(missing) if missing.kind() == io::ErrorKind::NotFound => {}
                            Err(error) => return Err(error),
                        }
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }
    Ok(result)
}

/// Whole-file ownership backs up the final symlink itself; parent directory
/// aliases still address the same slot.
pub fn slot(path: &Path) -> io::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "app theme target has no filename",
        )
    })?;
    Ok(identity(path.parent().unwrap_or_else(|| Path::new(".")))?.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_include_symlinks_and_missing_suffixes() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("app");
        std::fs::create_dir(&directory).unwrap();
        std::os::unix::fs::symlink(&directory, root.path().join("alias")).unwrap();
        let expected = directory.join("colors");
        assert_eq!(
            identity(&root.path().join("alias/missing/../colors")).unwrap(),
            expected
        );
        std::fs::write(&expected, "theme").unwrap();
        let link = root.path().join("colors-link");
        std::os::unix::fs::symlink(&expected, &link).unwrap();
        assert_eq!(identity(&link).unwrap(), expected);
        assert_eq!(slot(&link).unwrap(), link);
    }
}
