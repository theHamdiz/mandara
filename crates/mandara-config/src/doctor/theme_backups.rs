//! Bounded retention for the native theme guard's timestamped copies.

use anyhow::{Context as _, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Palette changes can be frequent. Keep recent recovery points without
/// letting automatic copies grow indefinitely; the ledger's original stays.
pub(super) const MAX_BACKUPS: usize = 20;

pub(super) fn retain(path: &Path, keep: usize) -> Result<()> {
    let parent = path.parent().context("configuration has no parent")?;
    let prefix = format!(
        "{}.mandara-backup-",
        path.file_name()
            .context("configuration has no filename")?
            .to_string_lossy()
    );
    let mut newest: BTreeMap<(u64, u32, u32), PathBuf> = BTreeMap::new();
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(suffix) = name.to_str().and_then(|name| name.strip_prefix(&prefix)) else {
            continue;
        };
        let mut parts = suffix.split('-');
        let (Some(seconds), Some(nanos), Some(pid), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if nanos.len() != 9
            || !nanos.bytes().all(|byte| byte.is_ascii_digit())
            || !canonical_number(seconds)
            || !canonical_number(pid)
        {
            continue;
        }
        let (Ok(seconds), Ok(nanos), Ok(pid)) = (
            seconds.parse::<u64>(),
            nanos.parse::<u32>(),
            pid.parse::<u32>(),
        ) else {
            continue;
        };
        if nanos >= 1_000_000_000 || !entry.file_type()?.is_file() {
            continue;
        }
        newest.insert((seconds, nanos, pid), entry.path());
        if newest.len() > keep
            && let Some((_, oldest)) = newest.pop_first()
        {
            fs::remove_file(&oldest)
                .with_context(|| format!("removing old theme backup {}", oldest.display()))?;
        }
    }
    Ok(())
}

fn canonical_number(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retention_keeps_newest_versions_originals_and_unrelated_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        for index in 0..8 {
            fs::write(
                root.path()
                    .join(format!("config.mandara-backup-{index}-000000000-1")),
                index.to_string(),
            )
            .unwrap();
        }
        for name in [
            "config.mandara-backup",
            "config.mandara-backup-notes",
            "other.mandara-backup-1-000000000-1",
        ] {
            fs::write(root.path().join(name), "keep").unwrap();
        }
        let link = root.path().join("config.mandara-backup-9-000000000-1");
        std::os::unix::fs::symlink("personal", &link).unwrap();
        retain(&path, 2).unwrap();
        assert!(
            root.path()
                .join("config.mandara-backup-7-000000000-1")
                .exists()
        );
        assert!(
            root.path()
                .join("config.mandara-backup-6-000000000-1")
                .exists()
        );
        assert!(
            !root
                .path()
                .join("config.mandara-backup-5-000000000-1")
                .exists()
        );
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 6);
        assert_eq!(fs::read_link(link).unwrap(), Path::new("personal"));
    }
}
