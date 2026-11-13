//! Reversible theme-key edits for compositors without generated includes.

mod rollback;

use super::comparison::CheckResult;
use anyhow::{Context as _, Result, ensure};
use mandara_i18n::t;
use mandara_theme::templates::{FileStamp, WriteLock};
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

/// Full doctor reports preserve warning/error counts; theme-only parsers have
/// a single pass/fail result. Both follow the same regression comparison.
pub struct LiveCheck {
    ran: bool,
    errors: usize,
    warnings: usize,
    result: Result<()>,
}
impl From<Result<()>> for LiveCheck {
    fn from(result: Result<()>) -> Self {
        Self {
            ran: true,
            errors: usize::from(result.is_err()),
            warnings: 0,
            result,
        }
    }
}
impl From<super::Report> for LiveCheck {
    fn from(report: super::Report) -> Self {
        let (errors, warnings) = report.counts();
        Self {
            ran: report.ran(),
            errors,
            warnings,
            result: if report.ran() && report.problems() == 0 {
                Ok(())
            } else {
                Err(anyhow::anyhow!("{}", report.raw))
            },
        }
    }
}
impl CheckResult for LiveCheck {
    fn ran(&self) -> bool {
        self.ran
    }
    fn counts(&self) -> (usize, usize) {
        (self.errors, self.warnings)
    }
}

pub trait Validator {
    /// Check the edited theme with its compositor in an isolated process.
    fn validate(&self, body: &str) -> Result<()>;

    fn check_live(&self, path: &Path) -> LiveCheck {
        self.validate_live(path).into()
    }

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

fn changed() -> String {
    t("The compositor configuration changed while its theme was being checked. Try again.").into()
}

/// Keep each pre-edit version, including later palette changes and undo. The
/// file stays a symlink; the backup stores its target's bytes and permissions.
fn backup(lock: &WriteLock, expected: Option<FileStamp>) -> Result<Option<PathBuf>> {
    backup_with(lock, expected, |path| {
        fs::File::open(path.parent().context("backup has no parent directory")?)?.sync_all()?;
        Ok(())
    })
}

fn backup_with(lock: &WriteLock, expected: Option<FileStamp>, sync_parent: impl FnOnce(&Path) -> Result<()>) -> Result<Option<PathBuf>> {
    lock.check_stamp(expected)?;
    if expected.is_none() {
        return Ok(None);
    }
    super::theme_backups::retain(lock.path(), super::theme_backups::MAX_BACKUPS - 1)?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    let path = lock.path();
    let name = path
        .file_name()
        .context("configuration has no filename")?
        .to_string_lossy();
    let backup = path.with_file_name(format!(
        "{name}.mandara-backup-{}-{:09}-{}",
        now.as_secs(),
        now.subsec_nanos(),
        std::process::id()
    ));
    let mut source = fs::File::open(lock.target())?;
    let mut saved = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&backup)?;
    saved.set_permissions(source.metadata()?.permissions())?;
    std::io::copy(&mut source, &mut saved)?;
    saved.flush()?;
    saved.sync_all()?;
    sync_parent(&backup)?;
    lock.check_stamp(expected)?;
    Ok(Some(backup))
}

/// The caller records ownership; this guard backs up each actual edit.
pub fn guarded_write(
    path: &Path,
    expected: &str,
    next: &str,
    validator: &dyn Validator,
) -> Result<()> {
    let lock = WriteLock::new(path)?;
    guarded_write_locked(&lock, expected, next, validator)
}

/// Reuse the preference writer's lock through journaling and native validation.
pub fn guarded_write_locked(
    lock: &WriteLock,
    expected: &str,
    next: &str,
    validator: &dyn Validator,
) -> Result<()> {
    let path = lock.path();
    let before = lock.check(expected.as_bytes()).context(changed())?;
    let baseline = validator.check_live(path);
    validator.validate(next).context(t(
        "The compositor rejected the theme; previous settings were kept.",
    ))?;
    lock.check_stamp(before).context(changed())?;
    if expected == next {
        return Ok(());
    }
    let recovery = backup(lock, before)?;
    let written = match crate::store::write_atomic_checked(lock, before, next.as_bytes()) {
        Ok(written) => written,
        Err(error) => {
            if let Some(written) = crate::store::published_stamp(&error)
                && let Err(restore) = restore_written(lock, before, written, expected)
            {
                return Err(rollback::failed(error, restore, recovery.as_deref()));
            }
            return Err(error);
        }
    };
    let live = validator.check_live(path);
    lock.check_stamp(Some(written)).context(changed())?;
    if super::comparison::is_worse(&baseline, &live) {
        let primary = live.result.err().unwrap_or_else(|| {
            std::io::Error::from(std::io::ErrorKind::InvalidData).into()
        });
        if let Err(restore) = restore_written(lock, before, written, expected) {
            return Err(rollback::failed(primary, restore, recovery.as_deref()));
        }
        return Err(primary.context(t(
            "The compositor rejected the theme; previous settings were kept.",
        )));
    }
    Ok(())
}

fn restore_written(lock: &WriteLock, before: Option<mandara_theme::templates::FileStamp>, written: mandara_theme::templates::FileStamp, expected: &str) -> Result<()> {
    // Rollback is another conditional write, with its own recoverable copy.
    lock.check_stamp(Some(written)).context(changed())?;
    let _ = backup(lock, Some(written))?;
    if before.is_some() {
        crate::store::write_atomic_checked(lock, Some(written), expected.as_bytes())?;
    } else {
        lock.check_stamp(Some(written))?;
        fs::remove_file(lock.target())?;
        if let Some(parent) = lock.target().parent() { fs::File::open(parent)?.sync_all()?; }
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
    fn repeated_native_edits_bound_recovery_copies_and_keep_the_original() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        fs::write(&path, "personal").unwrap();
        fs::write(
            root.path().join("config.mandara-backup"),
            "original undo copy",
        )
        .unwrap();
        let mut before = String::from("personal");
        for index in 0..super::super::theme_backups::MAX_BACKUPS + 4 {
            let next = format!("palette {index}");
            guarded_write(&path, &before, &next, &Check(|| Ok(()))).unwrap();
            before = next;
        }
        let count = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name.to_string_lossy().starts_with("config.mandara-backup-"))
            .count();
        assert_eq!(count, super::super::theme_backups::MAX_BACKUPS);
        assert_eq!(
            fs::read_to_string(root.path().join("config.mandara-backup")).unwrap(),
            "original undo copy"
        );
    }

    #[test]
    fn existing_live_problems_do_not_block_a_theme_but_new_ones_roll_it_back() {
        struct Counts {
            calls: std::cell::Cell<usize>,
            after: usize,
        }
        impl Validator for Counts {
            fn validate(&self, _: &str) -> Result<()> {
                Ok(())
            }
            fn check_live(&self, _: &Path) -> LiveCheck {
                let calls = self.calls.get();
                self.calls.set(calls + 1);
                LiveCheck {
                    ran: true,
                    errors: if calls == 0 { 1 } else { self.after },
                    warnings: 0,
                    result: Err(anyhow::anyhow!("unrelated invalid setting")),
                }
            }
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        fs::write(&path, "personal").unwrap();
        guarded_write(
            &path,
            "personal",
            "palette A",
            &Counts {
                calls: 0.into(),
                after: 1,
            },
        )
        .unwrap();
        assert!(
            guarded_write(
                &path,
                "palette A",
                "palette B",
                &Counts {
                    calls: 0.into(),
                    after: 2
                }
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "palette A");
        guarded_write(&path, "palette A", "personal", &Check(|| Ok(()))).unwrap();
        let backups: Vec<_> = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains("mandara-backup-")
            })
            .collect();
        assert_eq!(backups.len(), 4);
        assert!(
            backups
                .iter()
                .any(|path| fs::read_to_string(path).unwrap() == "palette B")
        );
    }

    #[test]
    fn a_new_edit_during_the_live_check_is_preserved_instead_of_rolled_back() {
        struct Edit<'a>(&'a Path);
        impl Validator for Edit<'_> {
            fn validate(&self, _: &str) -> Result<()> {
                Ok(())
            }
            fn validate_live(&self, _: &Path) -> Result<()> {
                if fs::read_to_string(self.0)? == "theme" {
                    fs::write(self.0, "editor's newer value")?;
                    anyhow::bail!("live include failed");
                }
                Ok(())
            }
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        fs::write(&path, "personal").unwrap();
        assert!(guarded_write(&path, "personal", "theme", &Edit(&path)).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "editor's newer value");
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
    #[test]
    fn failed_rollback_reports_the_live_diagnostic_and_pre_edit_backup() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().unwrap();
        let permissions = fs::metadata(root.path()).unwrap().permissions();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o555)).unwrap();
        let privileged = fs::File::create(root.path().join("permission-probe")).is_ok();
        fs::set_permissions(root.path(), permissions.clone()).unwrap();
        if privileged {
            return;
        }
        struct Rejected;
        impl Validator for Rejected {
            fn validate(&self, _: &str) -> Result<()> { Ok(()) }
            fn validate_live(&self, path: &Path) -> Result<()> {
                if fs::read_to_string(path)? == "theme" {
                    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o555))?;
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "native compositor diagnostic").into());
                }
                Ok(())
            }
        }
        let path = root.path().join("config");
        fs::write(&path, "personal").unwrap();
        let result = guarded_write(&path, "personal", "theme", &Rejected);
        fs::set_permissions(root.path(), permissions).unwrap();
        let error = result.unwrap_err();
        let recovery = fs::read_dir(root.path()).unwrap().map(|entry| entry.unwrap().path())
            .find(|path| path.file_name().unwrap().to_string_lossy().contains("mandara-backup-")).unwrap();
        assert_eq!(fs::read_to_string(&recovery).unwrap(), "personal");
        let message = format!("{error:#}");
        assert!(message.contains("native compositor diagnostic"), "{message}");
        assert!(message.contains("restoring the previous theme failed"), "{message}");
        assert!(message.contains(recovery.to_str().unwrap()), "{message}");
        assert_eq!(error.root_cause().downcast_ref::<std::io::Error>().unwrap().kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(fs::read_to_string(path).unwrap(), "theme");
    }

    #[test]
    fn backup_sync_uses_the_links_directory_and_precedes_any_config_edit() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("links");
        let target = root.path().join("targets");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&target).unwrap();
        let real = target.join("config");
        let link = source.join("config");
        fs::write(&real, "personal").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let lock = WriteLock::new(&link).unwrap();
        let before = FileStamp::at(&real).unwrap();
        let result = backup_with(&lock, before, |copy| {
            assert_eq!(copy.parent(), Some(source.as_path()));
            assert_eq!(fs::read_to_string(copy).unwrap(), "personal");
            Err(std::io::Error::other("injected backup directory sync failure").into())
        });
        assert!(result.is_err());
        assert_eq!(FileStamp::at(&real).unwrap(), before);
        assert_eq!(fs::read_to_string(&real).unwrap(), "personal");
        assert_eq!(fs::read_link(&link).unwrap(), real);
    }

}
