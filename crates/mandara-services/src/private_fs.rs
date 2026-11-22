//! Files that hold what the user copied or was notified about: readable by
//! the user only (files `0600`, directories `0700`), whatever the umask.

use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::Path;

/// Creates `dir` (and missing parents) as `0700` and tightens `dir` itself
/// if an older version created it with the umask's default.
pub fn create_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    if std::fs::metadata(dir)?.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Writes `bytes` to a new `0600` file at `path`, replacing it.
pub fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    file.write_all(bytes)
}

/// Writes `dir/name` atomically (a `0600` temp file, then rename) inside a
/// `0700` `dir`. The rename also replaces a file an older version left
/// world-readable.
pub fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    create_dir(dir)?;
    let tmp = dir.join(format!("{name}.tmp"));
    write(&tmp, bytes)?;
    std::fs::rename(&tmp, dir.join(name)).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_and_dirs_are_private_even_when_replacing_a_readable_file() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("history");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let old = dir.join("history.json");
        std::fs::write(&old, b"old").unwrap();
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_atomic(&dir, "history.json", b"new").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(std::fs::read(&old).unwrap(), b"new");
        assert_eq!(mode(&old), 0o600);
        assert_eq!(mode(&dir), 0o700);
        assert!(!dir.join("history.json.tmp").exists());
        let nested = root.path().join("a/b");
        create_dir(&nested).unwrap();
        assert_eq!(mode(&nested), 0o700);
    }
}
