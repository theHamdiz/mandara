//! Validate a native shortcut edit and its generated companion as one operation.

use anyhow::{Context as _, Result, ensure};
use mandara_i18n::{t, tf};
use mandara_theme::templates::{FileStamp, WriteLock};
use std::{borrow::Cow, path::Path};

use super::LiveCheck;

/// A shortcut transaction changes the native source and, at most, one drop-in.
const MAX_FILES: usize = 2;

pub struct FileEdit<'a> {
    pub lock: &'a WriteLock,
    pub before: &'a str,
    pub after: &'a str,
    /// A drop-in can first omit both old and new chords while the native file
    /// changes, then restore the shortcuts that no longer conflict.
    pub intermediate: Option<&'a str>,
}

pub trait BatchValidator {
    /// Parse a candidate without executing user configuration code.
    fn validate(&self, path: &Path, body: &str) -> Result<()>;
    fn before(&self) -> LiveCheck;
    /// Check the complete config after every file has been published.
    fn after(&self) -> LiveCheck;
    /// A failed native reload is a failure even if the config has fewer diagnostics.
    fn apply_live(&self) -> Result<()> {
        Ok(())
    }
    fn restore_live(&self) -> Result<()> {
        Ok(())
    }
}

fn changed() -> String {
    t("The compositor shortcuts changed while they were being checked. Refresh the list and try again.").into()
}

pub fn guarded_batch(edits: &[FileEdit<'_>], validator: &dyn BatchValidator) -> Result<()> {
    guarded_batch_finalizing(edits, validator, None)
}

/// A companion can be calculated after the native source is published, so
/// include order, variables and physical-key aliases use the compositor's
/// current configuration. Its backup and write fence are still taken first.
pub fn guarded_batch_finalizing(
    edits: &[FileEdit<'_>],
    validator: &dyn BatchValidator,
    finalize: Option<&dyn Fn(usize) -> Result<Option<String>>>,
) -> Result<()> {
    guarded_batch_using(edits, validator, finalize, |lock, before, body| {
        crate::store::write_atomic_checked(lock, before, body.as_bytes())
    })
}

fn guarded_batch_using(
    edits: &[FileEdit<'_>],
    validator: &dyn BatchValidator,
    finalize: Option<&dyn Fn(usize) -> Result<Option<String>>>,
    publish: impl Fn(&WriteLock, Option<FileStamp>, &str) -> Result<FileStamp>,
) -> Result<()> {
    ensure!(
        !edits.is_empty() && edits.len() <= MAX_FILES,
        "invalid shortcut transaction size"
    );
    let mut original = [None; MAX_FILES];
    let mut written = [None; MAX_FILES];
    let mut recovery = [None, None];
    let mut current = [Cow::Borrowed(""), Cow::Borrowed("")];
    for (index, edit) in edits.iter().enumerate() {
        ensure!(
            !edits[..index]
                .iter()
                .any(|previous| previous.lock.target() == edit.lock.target()),
            "shortcut files refer to the same destination"
        );
        original[index] = edit.lock.check(edit.before.as_bytes()).context(changed())?;
        current[index] = Cow::Borrowed(edit.before);
        validator.validate(edit.lock.path(), edit.after)?;
        if let Some(body) = edit.intermediate {
            validator.validate(edit.lock.path(), body)?;
        }
    }
    if finalize.is_none()
        && edits.iter().all(|edit| {
            edit.before == edit.after && edit.intermediate.is_none_or(|body| body == edit.before)
        })
    {
        return Ok(());
    }
    let baseline = validator.before();
    ensure!(
        baseline.ran,
        "{}",
        t("The compositor's shortcut checker could not run. No files were changed.")
    );
    for (index, edit) in edits.iter().enumerate() {
        edit.lock.check_stamp(original[index]).context(changed())?;
        if finalize.is_some()
            || edit.before != edit.after
            || edit.intermediate.is_some_and(|body| body != edit.before)
        {
            recovery[index] = super::backup(edit.lock, original[index])?;
        }
    }

    let result = (|| -> Result<()> {
        for intermediate in [true, false] {
            for (index, edit) in edits.iter().enumerate() {
                let body = if intermediate {
                    let Some(body) = edit.intermediate else {
                        continue;
                    };
                    Cow::Borrowed(body)
                } else {
                    match finalize
                        .map(|finalize| finalize(index))
                        .transpose()?
                        .flatten()
                    {
                        Some(body) => {
                            validator.validate(edit.lock.path(), &body)?;
                            Cow::Owned(body)
                        }
                        None => Cow::Borrowed(edit.after),
                    }
                };
                if body == current[index] {
                    continue;
                }
                let previous = written[index].or(original[index]);
                match publish(edit.lock, previous, &body) {
                    Ok(stamp) => {
                        written[index] = Some(stamp);
                        current[index] = body;
                    }
                    Err(error) => {
                        if let Some(stamp) = crate::store::published_stamp(&error) {
                            written[index] = Some(stamp);
                        }
                        return Err(error);
                    }
                }
            }
        }
        verify(edits, &original, &written, &current)?;
        let live = validator.after();
        verify(edits, &original, &written, &current)?;
        if super::super::comparison::is_worse(&baseline, &live) {
            return Err(live
                .result
                .err()
                .unwrap_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidData).into())
                .context(t("The compositor rejected these shortcuts.")));
        }
        validator.apply_live()?;
        verify(edits, &original, &written, &current)?;
        Ok(())
    })();
    let Err(mut error) = result else {
        return Ok(());
    };
    let mut restored = true;
    for (index, edit) in edits.iter().enumerate().rev() {
        let Some(stamp) = written[index] else {
            continue;
        };
        if let Err(restore) = super::restore_written(edit.lock, original[index], stamp, edit.before)
        {
            restored = false;
            let backup = recovery[index]
                .as_deref()
                .map_or_else(String::new, |path| path.display().to_string());
            error = error.context(tf(
                "Could not restore {file}: {error}. Backup: {backup}",
                &[
                    ("file", &edit.lock.path().display()),
                    ("error", &format!("{restore:#}")),
                    ("backup", &backup),
                ],
            ));
        }
    }
    if restored
        && written.iter().any(Option::is_some)
        && let Err(restore) = validator.restore_live()
    {
        error = error.context(tf(
            "The files were restored, but the compositor could not reload them: {error}",
            &[("error", &format!("{restore:#}"))],
        ));
    }
    Err(error)
}

fn verify(
    edits: &[FileEdit<'_>],
    original: &[Option<FileStamp>; MAX_FILES],
    written: &[Option<FileStamp>; MAX_FILES],
    current: &[Cow<'_, str>; MAX_FILES],
) -> Result<()> {
    for (index, edit) in edits.iter().enumerate() {
        let expected = written[index].or(original[index]);
        edit.lock.check_stamp(expected).context(changed())?;
        let body = &current[index];
        ensure!(
            edit.lock.check(body.as_bytes()).context(changed())? == expected,
            "{}",
            changed()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        fs,
    };

    struct Check<'a> {
        native: &'a Path,
        generated: &'a Path,
        reject: bool,
        live_failure: bool,
        foreign: bool,
        restored: Cell<bool>,
    }
    impl BatchValidator for Check<'_> {
        fn validate(&self, _: &Path, _: &str) -> Result<()> {
            Ok(())
        }
        fn before(&self) -> LiveCheck {
            Ok(()).into()
        }
        fn after(&self) -> LiveCheck {
            assert_eq!(fs::read_to_string(self.native).unwrap(), "new native");
            assert_eq!(fs::read_to_string(self.generated).unwrap(), "new generated");
            if self.foreign {
                fs::write(self.native, "external edit").unwrap();
            }
            if self.reject {
                Err(anyhow::anyhow!("native rejection")).into()
            } else {
                Ok(()).into()
            }
        }
        fn apply_live(&self) -> Result<()> {
            ensure!(!self.live_failure, "reload failed");
            Ok(())
        }
        fn restore_live(&self) -> Result<()> {
            self.restored.set(true);
            Ok(())
        }
    }

    #[test]
    fn paired_changes_publish_the_bridge_before_native_and_final_bindings() {
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("native");
        let generated = dir.path().join("generated");
        fs::write(&native, "old native").unwrap();
        fs::write(&generated, "old generated").unwrap();
        let (first, second) = WriteLock::pair(&native, Some(&generated)).unwrap();
        let second = second.unwrap();
        let check = Check {
            native: &native,
            generated: &generated,
            reject: false,
            live_failure: false,
            foreign: false,
            restored: Cell::new(false),
        };
        let edits = [
            FileEdit {
                lock: &first,
                before: "old native",
                after: "new native",
                intermediate: None,
            },
            FileEdit {
                lock: &second,
                before: "old generated",
                after: "new generated",
                intermediate: Some("bridge"),
            },
        ];
        let states = RefCell::new(Vec::new());
        guarded_batch_using(&edits, &check, None, |lock, stamp, body| {
            let result = crate::store::write_atomic_checked(lock, stamp, body.as_bytes())?;
            states.borrow_mut().push((
                fs::read_to_string(&native).unwrap(),
                fs::read_to_string(&generated).unwrap(),
            ));
            Ok(result)
        })
        .unwrap();
        assert_eq!(
            *states.borrow(),
            [
                ("old native".into(), "bridge".into()),
                ("new native".into(), "bridge".into()),
                ("new native".into(), "new generated".into())
            ]
        );
        assert!(!check.restored.get());
    }

    #[test]
    fn native_rejection_and_failed_reload_restore_both_files() {
        for (reject, live_failure) in [(true, false), (false, true)] {
            let dir = tempfile::tempdir().unwrap();
            let native = dir.path().join("native");
            let generated = dir.path().join("generated");
            fs::write(&native, "old native").unwrap();
            fs::write(&generated, "old generated").unwrap();
            let (first, second) = WriteLock::pair(&native, Some(&generated)).unwrap();
            let second = second.unwrap();
            let check = Check {
                native: &native,
                generated: &generated,
                reject,
                live_failure,
                foreign: false,
                restored: Cell::new(false),
            };
            let edits = [
                FileEdit {
                    lock: &first,
                    before: "old native",
                    after: "new native",
                    intermediate: None,
                },
                FileEdit {
                    lock: &second,
                    before: "old generated",
                    after: "new generated",
                    intermediate: None,
                },
            ];
            assert!(guarded_batch(&edits, &check).is_err());
            assert_eq!(fs::read_to_string(&native).unwrap(), "old native");
            assert_eq!(fs::read_to_string(&generated).unwrap(), "old generated");
            assert!(check.restored.get());
        }
    }

    #[test]
    fn post_publish_failure_in_either_file_rolls_back_every_published_version() {
        for fail_at in [1, 2, 3] {
            let dir = tempfile::tempdir().unwrap();
            let native = dir.path().join("native");
            let generated = dir.path().join("generated");
            fs::write(&native, "old native").unwrap();
            fs::write(&generated, "old generated").unwrap();
            let (first, second) = WriteLock::pair(&native, Some(&generated)).unwrap();
            let second = second.unwrap();
            let check = Check {
                native: &native,
                generated: &generated,
                reject: false,
                live_failure: false,
                foreign: false,
                restored: Cell::new(false),
            };
            let edits = [
                FileEdit {
                    lock: &first,
                    before: "old native",
                    after: "new native",
                    intermediate: None,
                },
                FileEdit {
                    lock: &second,
                    before: "old generated",
                    after: "new generated",
                    intermediate: Some("bridge"),
                },
            ];
            let count = Cell::new(0);
            assert!(
                guarded_batch_using(&edits, &check, None, |lock, previous, body| {
                    let stamp =
                        crate::store::write_atomic_checked(lock, previous, body.as_bytes())?;
                    count.set(count.get() + 1);
                    if count.get() == fail_at {
                        return Err(mandara_theme::templates::PublishedWriteError::new(
                            lock.target(),
                            stamp,
                            std::io::Error::other("directory sync failed"),
                        )
                        .into());
                    }
                    Ok(stamp)
                })
                .is_err()
            );
            assert_eq!(fs::read_to_string(&native).unwrap(), "old native");
            assert_eq!(fs::read_to_string(&generated).unwrap(), "old generated");
        }
    }

    #[test]
    fn final_companion_uses_the_new_native_source_and_failure_restores_both() {
        for reject in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let native = root.path().join("native");
            let generated = root.path().join("generated");
            fs::write(&native, "old native").unwrap();
            fs::write(&generated, "old generated").unwrap();
            let (first, second) = WriteLock::pair(&native, Some(&generated)).unwrap();
            let check = Check {
                native: &native,
                generated: &generated,
                reject,
                live_failure: false,
                foreign: false,
                restored: Cell::new(false),
            };
            let edits = [
                FileEdit {
                    lock: &first,
                    before: "old native",
                    after: "new native",
                    intermediate: None,
                },
                FileEdit {
                    lock: second.as_ref().unwrap(),
                    before: "old generated",
                    after: "bridge",
                    intermediate: Some("bridge"),
                },
            ];
            let finalized = Cell::new(false);
            let result = guarded_batch_finalizing(
                &edits,
                &check,
                Some(&|index| {
                    if index == 0 {
                        return Ok(None);
                    }
                    assert_eq!(fs::read_to_string(&native).unwrap(), "new native");
                    assert_eq!(fs::read_to_string(&generated).unwrap(), "bridge");
                    finalized.set(true);
                    Ok(Some("new generated".into()))
                }),
            );
            assert_eq!(result.is_err(), reject);
            assert!(finalized.get());
            assert_eq!(
                fs::read_to_string(&native).unwrap(),
                if reject { "old native" } else { "new native" }
            );
            assert_eq!(
                fs::read_to_string(&generated).unwrap(),
                if reject {
                    "old generated"
                } else {
                    "new generated"
                }
            );
        }
    }

    #[test]
    fn concurrent_external_edit_is_kept_while_the_companion_is_restored() {
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("native");
        let generated = dir.path().join("generated");
        fs::write(&native, "old native").unwrap();
        fs::write(&generated, "old generated").unwrap();
        let (first, second) = WriteLock::pair(&native, Some(&generated)).unwrap();
        let second = second.unwrap();
        let check = Check {
            native: &native,
            generated: &generated,
            reject: false,
            live_failure: false,
            foreign: true,
            restored: Cell::new(false),
        };
        let edits = [
            FileEdit {
                lock: &first,
                before: "old native",
                after: "new native",
                intermediate: None,
            },
            FileEdit {
                lock: &second,
                before: "old generated",
                after: "new generated",
                intermediate: None,
            },
        ];
        let error = guarded_batch(&edits, &check).unwrap_err();
        assert!(format!("{error:#}").contains("Backup:"));
        assert_eq!(fs::read_to_string(&native).unwrap(), "external edit");
        assert_eq!(fs::read_to_string(&generated).unwrap(), "old generated");
        assert!(!check.restored.get());
    }
}
