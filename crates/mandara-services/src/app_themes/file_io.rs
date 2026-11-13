//! Bounded template reads and conservative ownership-record discovery.

use anyhow::{Context as _, Result, ensure};
use mandara_theme::templates::custom::MAX_TEMPLATE_BYTES;
use std::io::Read as _;
use std::path::Path;

pub(crate) fn read_text(path: &Path) -> Result<String> {
    ensure!(
        std::fs::metadata(path)?.is_file(),
        "template input is not a regular file"
    );
    let file = std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let mut text = String::new();
    file.take(MAX_TEMPLATE_BYTES as u64 + 1)
        .read_to_string(&mut text)?;
    ensure!(
        text.len() <= MAX_TEMPLATE_BYTES,
        "template input is too large"
    );
    Ok(text)
}

pub(super) fn present(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// A missing symlink target is an unreadable record, never empty ownership.
pub(super) fn absent(path: &Path, error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        && matches!(std::fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_dangling_record_is_present_and_cannot_be_treated_as_empty() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("record");
        let target = root.path().join("missing");
        assert!(!present(&path).unwrap());
        assert!(absent(&path, &read_text(&path).unwrap_err()));
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(present(&path).unwrap());
        assert!(!absent(&path, &read_text(&path).unwrap_err()));
        assert_eq!(std::fs::read_link(path).unwrap(), target);
    }
}
