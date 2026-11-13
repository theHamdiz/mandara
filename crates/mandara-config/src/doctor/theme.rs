//! Reversible theme-key edits for compositors without generated includes.

use anyhow::{Context as _, Result, ensure};
use mandara_i18n::t;
use std::fs;
use std::io::Read as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

/// Compare a config in small chunks without allocating another copy of it.
const COMPARE_BYTES: usize = 4096;

pub trait Validator {
    /// Check the edited theme with its compositor in an isolated process.
    fn validate(&self, body: &str) -> Result<()>;

    /// Check the actual file after writing as well as its private candidate.
    /// Implementations may inspect live includes, but must never execute user
    /// startup commands or connect to the running compositor.
    fn validate_live(&self, path: &Path) -> Result<()> {
        const MAX_CONFIG_BYTES: u64 = 8 * 1024 * 1024;
        let mut body = String::new();
        match fs::File::open(path) {
            Ok(file) => {
                file.take(MAX_CONFIG_BYTES + 1).read_to_string(&mut body)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        ensure!(
            body.len() as u64 <= MAX_CONFIG_BYTES,
            "compositor configuration is too large"
        );
        self.validate(&body)
    }
}

fn destination(path: &Path) -> Result<PathBuf> {
    let target = crate::store::symlink_target(path);
    for parent in target.ancestors() {
        if let Ok(base) = parent.canonicalize() {
            let suffix = target.strip_prefix(parent)?;
            return Ok(if suffix.as_os_str().is_empty() {
                base
            } else {
                base.join(suffix)
            });
        }
    }
    Ok(target)
}

fn changed() -> String {
    t("The compositor configuration changed while its theme was being checked. Try again.").into()
}

/// The inode and modification time also detect replacement with identical text.
type Identity = (u64, u64, i64, i64);

fn unchanged(path: &Path, expected: &[u8]) -> Result<Option<Identity>> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ensure!(expected.is_empty(), changed());
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    ensure!(
        metadata.is_file(),
        t("The compositor configuration is not a regular file.")
    );
    let mut file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    let mut buffer = [0; COMPARE_BYTES];
    let mut offset = 0;
    loop {
        let count = match file.read(&mut buffer) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        if count == 0 {
            break;
        }
        ensure!(
            expected.get(offset..offset + count) == Some(&buffer[..count]),
            changed()
        );
        offset += count;
    }
    ensure!(offset == expected.len(), changed());
    Ok(Some((
        metadata.dev(),
        metadata.ino(),
        metadata.mtime(),
        metadata.mtime_nsec(),
    )))
}

/// The caller records ownership and backs up the original before this write.
pub fn guarded_write(
    path: &Path,
    expected: &str,
    next: &str,
    validator: &dyn Validator,
) -> Result<()> {
    let target = destination(path)?;
    let before = unchanged(&target, expected.as_bytes())?;
    let _baseline = validator.validate_live(path);
    // A compositor's file watcher must never see a rejected edit.
    validator.validate(next).context(t(
        "The compositor rejected the theme; previous settings were kept.",
    ))?;
    ensure!(destination(path)? == target, changed());
    ensure!(
        unchanged(&target, expected.as_bytes())? == before,
        changed()
    );
    crate::store::write_atomic(&target, next.as_bytes())?;
    let written = unchanged(&target, next.as_bytes())?;
    let live = validator.validate_live(path);
    ensure!(destination(path)? == target, changed());
    ensure!(unchanged(&target, next.as_bytes())? == written, changed());
    if let Err(error) = live {
        // A live include can change after candidate validation. Restore only
        // our own write, preserving an editor's concurrent replacement.
        ensure!(destination(path)? == target, changed());
        unchanged(&target, next.as_bytes())?;
        if before.is_some() {
            crate::store::write_atomic(&target, expected.as_bytes())?;
        } else {
            fs::remove_file(&target)?;
        }
        return Err(error).context(t(
            "The compositor rejected the theme; previous settings were kept.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Check<F>(F);
    impl<F: Fn() -> Result<()>> Validator for Check<F> {
        fn validate(&self, _: &str) -> Result<()> {
            (self.0)()
        }
    }

    #[test]
    fn a_failed_live_check_restores_existing_and_previously_absent_targets() {
        struct Live {
            checks: std::cell::Cell<usize>,
        }
        impl Validator for Live {
            fn validate(&self, _: &str) -> Result<()> {
                Ok(())
            }
            fn validate_live(&self, _: &Path) -> Result<()> {
                let checks = self.checks.get();
                self.checks.set(checks + 1);
                if checks > 0 {
                    anyhow::bail!("included file changed");
                }
                Ok(())
            }
        }
        let root = tempfile::tempdir().unwrap();
        for exists in [true, false] {
            let path = root.path().join(if exists { "existing" } else { "absent" });
            let expected = if exists { "personal" } else { "" };
            if exists {
                fs::write(&path, expected).unwrap();
            }
            let validator = Live {
                checks: std::cell::Cell::new(0),
            };
            assert!(guarded_write(&path, expected, "theme", &validator).is_err());
            assert_eq!(validator.checks.get(), 2);
            if exists {
                assert_eq!(fs::read_to_string(&path).unwrap(), expected);
            } else {
                assert!(!path.exists());
            }
        }
    }

    #[test]
    fn rejected_edits_keep_the_original_target_and_symlink() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("real");
        let link = root.path().join("config");
        fs::write(&target, "personal").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let reject = Check(|| anyhow::bail!("native rejection"));
        assert!(guarded_write(&link, "personal", "theme", &reject).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "personal");
        assert_eq!(fs::read_link(&link).unwrap(), target);
        let absent = root.path().join("absent");
        assert!(guarded_write(&absent, "", "theme", &reject).is_err());
        assert!(!absent.exists());
        guarded_write(&link, "personal", "theme", &Check(|| Ok(()))).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "theme");
        assert_eq!(fs::read_link(&link).unwrap(), target);
    }

    #[test]
    fn edits_made_during_native_validation_are_not_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        fs::write(&path, "personal").unwrap();
        let edit = Check(|| {
            fs::write(&path, "new personal edit")?;
            Ok(())
        });
        assert!(guarded_write(&path, "personal", "theme", &edit).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "new personal edit");
        let absent = root.path().join("absent");
        let create = Check(|| {
            fs::write(&absent, "new configuration")?;
            Ok(())
        });
        assert!(guarded_write(&absent, "", "theme", &create).is_err());
        assert_eq!(fs::read_to_string(&absent).unwrap(), "new configuration");
    }

    #[test]
    fn a_retargeted_symlink_keeps_both_files_even_when_their_text_matches() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old");
        let new = root.path().join("new");
        let link = root.path().join("config");
        fs::write(&old, "personal").unwrap();
        fs::write(&new, "personal").unwrap();
        std::os::unix::fs::symlink(&old, &link).unwrap();
        let retarget = Check(|| {
            fs::remove_file(&link)?;
            std::os::unix::fs::symlink(&new, &link)?;
            Ok(())
        });
        assert!(guarded_write(&link, "personal", "theme", &retarget).is_err());
        assert_eq!(fs::read_to_string(&old).unwrap(), "personal");
        assert_eq!(fs::read_to_string(&new).unwrap(), "personal");
        assert_eq!(fs::read_link(&link).unwrap(), new);
    }

    #[test]
    fn an_atomic_replacement_with_the_same_text_still_belongs_to_the_editor() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        let temporary = root.path().join("editor-save");
        fs::write(&path, "personal").unwrap();
        let replace = Check(|| {
            fs::write(&temporary, "personal")?;
            fs::rename(&temporary, &path)?;
            Ok(())
        });
        assert!(guarded_write(&path, "personal", "theme", &replace).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "personal");
    }
}
