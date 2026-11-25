//! Undo a new override only after claiming and checking the actual file.

use anyhow::Result;
use mandara_theme::templates::{FileStamp, WriteLock};

pub(super) fn remove(lock: &WriteLock, expected: FileStamp) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::remove(lock, expected, || {}, |_| {})
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (lock, expected);
        anyhow::bail!(mandara_i18n::t(
            "App permissions are available for Flatpak apps on Linux."
        ))
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use anyhow::Context as _;
    use mandara_i18n::{t, tf};
    use rustix::fs::{AtFlags, Mode, OFlags, mkdirat, open, openat, statat, unlinkat};
    use std::{
        ffi::OsStr,
        fs::File,
        os::{fd::AsRawFd as _, unix::fs::MetadataExt as _},
        path::Path,
    };

    /// A private recovery directory uses the same 128-bit collision space as cleanup.
    const RANDOM_BYTES: usize = 16;
    const OPEN: OFlags = OFlags::RDONLY
        .union(OFlags::CLOEXEC)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::NONBLOCK);
    const RETAINED: &str = "override";

    pub(super) fn remove(
        lock: &WriteLock,
        expected: FileStamp,
        before_claim: impl FnOnce(),
        after_claim: impl FnOnce(&Path),
    ) -> Result<()> {
        let target = lock.target();
        let parent = target
            .parent()
            .context(t("The Flatpak permission data is invalid or too large."))?;
        let filename = target
            .file_name()
            .context(t("The Flatpak permission data is invalid or too large."))?;
        let directory = File::from(open(parent, OPEN | OFlags::DIRECTORY, Mode::empty())?);
        let mut random = [0; RANDOM_BYTES];
        getrandom::fill(&mut random)?;
        let name = format!(".mandara-undo-{:032x}", u128::from_ne_bytes(random));
        mkdirat(&directory, name.as_str(), Mode::RWXU)?;
        let recovery = File::from(openat(
            &directory,
            name.as_str(),
            OPEN | OFlags::DIRECTORY,
            Mode::empty(),
        )?);
        let recovery_path = || {
            std::fs::read_link(format!("/proc/self/fd/{}", recovery.as_raw_fd()))
                .unwrap_or_else(|_| parent.join(&name))
                .join(RETAINED)
        };
        let result = (|| -> Result<()> {
            before_claim();
            crate::file_claim::rename(&directory, filename, &recovery, OsStr::new(RETAINED))?;
            after_claim(&recovery_path());
            recovery.sync_all()?;
            directory.sync_all()?;
            let actual = openat(&recovery, RETAINED, OPEN, Mode::empty())
                .map(File::from)
                .map_err(std::io::Error::from)
                .and_then(|file| FileStamp::of(&file));
            if !actual.is_ok_and(|stamp| stamp == expected) || !same_directory(&directory, parent) {
                crate::file_claim::rename(&recovery, OsStr::new(RETAINED), &directory, filename)?;
                directory.sync_all()?;
                anyhow::bail!(t("The app permissions changed. Refresh before saving."));
            }
            unlinkat(&recovery, RETAINED, AtFlags::empty())?;
            recovery.sync_all()?;
            directory.sync_all()?;
            anyhow::ensure!(
                same_directory(&directory, parent)
                    && statat(&directory, filename, AtFlags::SYMLINK_NOFOLLOW)
                        .is_err_and(|error| error == rustix::io::Errno::NOENT),
                t("The app permissions changed. Refresh before saving.")
            );
            Ok(())
        })();
        if !statat(&recovery, RETAINED, AtFlags::SYMLINK_NOFOLLOW)
            .is_err_and(|error| error == rustix::io::Errno::NOENT)
        {
            return result
                .and_then(|()| {
                    anyhow::bail!(t("The app permissions changed. Refresh before saving."))
                })
                .with_context(|| {
                    tf(
                        "Undo could not finish. The retained file is available at {path}.",
                        &[("path", &recovery_path().display())],
                    )
                });
        }
        let cleanup = unlinkat(&directory, name.as_str(), AtFlags::REMOVEDIR);
        result?;
        cleanup?;
        directory.sync_all()?;
        Ok(())
    }

    fn same_directory(directory: &File, path: &Path) -> bool {
        directory
            .metadata()
            .and_then(|held| {
                std::fs::metadata(path)
                    .map(|current| held.dev() == current.dev() && held.ino() == current.ino())
            })
            .unwrap_or(false)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn undo_preserves_replacements_before_claim_and_while_restoring() {
            for replace_again in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let target = root.path().join("override");
                std::fs::write(&target, b"published").unwrap();
                let lock = WriteLock::new(&target).unwrap();
                let expected = FileStamp::at(&target).unwrap().unwrap();
                let result = remove(
                    &lock,
                    expected,
                    || {
                        let replacement = root.path().join("external");
                        std::fs::write(&replacement, b"external revision").unwrap();
                        std::fs::rename(replacement, &target).unwrap();
                    },
                    |_| {
                        if replace_again {
                            std::fs::write(&target, b"newer external revision").unwrap();
                        }
                    },
                );
                assert!(result.is_err());
                assert_eq!(
                    std::fs::read(&target).unwrap(),
                    if replace_again {
                        &b"newer external revision"[..]
                    } else {
                        &b"external revision"[..]
                    }
                );
                let recovery = std::fs::read_dir(root.path())
                    .unwrap()
                    .filter_map(|entry| entry.ok())
                    .find(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with(".mandara-undo-")
                    });
                if replace_again {
                    let path = recovery.unwrap().path().join(RETAINED);
                    assert_eq!(std::fs::read(&path).unwrap(), b"external revision");
                    assert!(format!("{:#}", result.unwrap_err()).contains(path.to_str().unwrap()));
                } else {
                    assert!(recovery.is_none());
                }
            }
        }

        #[test]
        fn undo_removes_only_its_owned_file_and_keeps_aliases_and_late_edits() {
            for late_edit in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let target = root.path().join("target");
                let alias = root.path().join("alias");
                std::fs::write(&target, b"published").unwrap();
                std::os::unix::fs::symlink(&target, &alias).unwrap();
                let lock = WriteLock::new(&alias).unwrap();
                let expected = FileStamp::at(&target).unwrap().unwrap();
                let result = remove(
                    &lock,
                    expected,
                    || {},
                    |_| {
                        if late_edit {
                            std::fs::write(&target, b"late replacement").unwrap();
                        }
                    },
                );
                assert!(alias.is_symlink());
                if late_edit {
                    assert!(result.is_err());
                    assert_eq!(std::fs::read(&target).unwrap(), b"late replacement");
                } else {
                    result.unwrap();
                    assert!(!target.exists());
                }
            }
        }

        #[test]
        fn undo_preserves_captured_edits_special_files_and_moved_directories() {
            for kind in 0..3 {
                let root = tempfile::tempdir().unwrap();
                let parent = root.path().join("overrides");
                std::fs::create_dir(&parent).unwrap();
                let target = parent.join("app");
                std::fs::write(&target, b"published").unwrap();
                let lock = WriteLock::new(&target).unwrap();
                let expected = FileStamp::at(&target).unwrap().unwrap();
                let result = remove(
                    &lock,
                    expected,
                    || {
                        if kind == 1 {
                            std::fs::remove_file(&target).unwrap();
                            rustix::fs::mkfifoat(rustix::fs::CWD, &target, Mode::RWXU).unwrap();
                        }
                    },
                    |captured| match kind {
                        0 => std::fs::write(captured, b"edited after capture").unwrap(),
                        2 => {
                            std::fs::rename(&parent, root.path().join("moved")).unwrap();
                            std::fs::create_dir(&parent).unwrap();
                            std::fs::write(&target, b"replacement directory").unwrap();
                        }
                        _ => {}
                    },
                );
                assert!(result.is_err());
                match kind {
                    0 => assert_eq!(std::fs::read(&target).unwrap(), b"edited after capture"),
                    1 => {
                        use std::os::unix::fs::FileTypeExt as _;
                        assert!(
                            std::fs::symlink_metadata(&target)
                                .unwrap()
                                .file_type()
                                .is_fifo()
                        );
                    }
                    2 => {
                        assert_eq!(std::fs::read(&target).unwrap(), b"replacement directory");
                        assert_eq!(
                            std::fs::read(root.path().join("moved/app")).unwrap(),
                            b"published"
                        );
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
}
