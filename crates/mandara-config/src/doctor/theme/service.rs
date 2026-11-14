//! A settings service owns live state and may serialize its file asynchronously.

use anyhow::{Context as _, Result, ensure};
use mandara_i18n::{t, tf};
use mandara_theme::templates::{FileStamp, WriteLock};
use std::{
    io::Read as _,
    time::{Duration, Instant},
};

/// Poll only during a user-requested save, until the native writer finishes.
const POLL: Duration = Duration::from_millis(20);
/// KGlobalAccel debounces writes for 500 ms; allow a bounded slow-disk margin.
const PERSIST_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_BYTES: u64 = 1024 * 1024;

pub trait ServiceEditor {
    fn persist_timeout(&self) -> Duration {
        PERSIST_TIMEOUT
    }
    fn preflight(&self) -> Result<()> {
        self.verify(false)
    }
    /// Check live values, availability, source revisions and caller preferences.
    fn verify(&self, after: bool) -> Result<()>;
    /// Recovery checks ownership independently of later preference edits.
    fn owns(&self, after: bool) -> Result<()> {
        self.verify(after)
    }
    /// Acknowledged requests must supersede pending serialization of older values.
    fn apply(&self, after: bool) -> Result<()>;
    /// A service may reorder or escape settings, but must not change other keys.
    fn equivalent(&self, body: &str, after: bool) -> Result<bool>;
}

fn read(lock: &WriteLock) -> Result<(String, FileStamp)> {
    lock.check_target()?;
    let file = crate::regular::open(lock.target())?;
    let stamp = FileStamp::of(&file)?;
    let mut body = String::new();
    file.take(MAX_BYTES + 1).read_to_string(&mut body)?;
    ensure!(
        body.len() as u64 <= MAX_BYTES,
        "shortcut settings exceed the file budget"
    );
    lock.check_stamp(Some(stamp))?;
    Ok((body, stamp))
}

fn persisted(
    lock: &WriteLock,
    previous: FileStamp,
    after: bool,
    editor: &dyn ServiceEditor,
    accept_current: bool,
) -> Result<FileStamp> {
    let deadline = Instant::now() + editor.persist_timeout();
    loop {
        let (body, stamp) = read(lock)?;
        if (accept_current || stamp != previous) && editor.equivalent(&body, after)? {
            editor.owns(after)?;
            return Ok(stamp);
        }
        ensure!(
            editor.equivalent(&body, !after)?,
            "{}",
            t("The shortcut service changed other settings while saving. The backup was kept.")
        );
        ensure!(
            Instant::now() < deadline,
            "{}",
            t("The shortcut service did not finish saving in time.")
        );
        std::thread::sleep(POLL);
    }
}

/// Back up before the first native request. Once its write is observed, restore
/// the selected field's exact surrounding bytes with the same conditional writer
/// used by the Config doctor. Foreign settings and retargeted links are rejected.
pub fn guarded_service_write(
    lock: &WriteLock,
    before: &str,
    after: &str,
    editor: &dyn ServiceEditor,
) -> Result<()> {
    let initial = lock
        .check(before.as_bytes())?
        .context("shortcut settings disappeared")?;
    editor.preflight()?;
    let backup = super::backup(lock, Some(initial))?;
    lock.check_stamp(Some(initial))?;
    let result: Result<()> = (|| {
        editor.apply(true)?;
        editor.verify(true)?;
        let native = persisted(lock, initial, true, editor, false)?;
        editor.verify(true)?;
        crate::store::write_atomic_checked(lock, Some(native), after.as_bytes())?;
        editor.verify(true)?;
        lock.check(after.as_bytes())?;
        Ok(())
    })();
    let Err(mut error) = result else {
        return Ok(());
    };
    // Restore only values still owned by this request. A later user's change
    // to the same action, another key or a symlink must survive the failure.
    let recovery = (|| -> Result<()> {
        let (body, stamp) = read(lock)?;
        if editor.owns(false).is_ok() && body == before {
            return Ok(());
        }
        editor.owns(true)?;
        ensure!(
            editor.equivalent(&body, true)? || editor.equivalent(&body, false)?,
            "shortcut settings changed outside this save"
        );
        editor.apply(false)?;
        editor.owns(false)?;
        let native = persisted(lock, stamp, false, editor, true)?;
        if lock.check(before.as_bytes()).is_err() {
            crate::store::write_atomic_checked(lock, Some(native), before.as_bytes())?;
        }
        Ok(())
    })();
    if let Err(recovery) = recovery {
        error = error.context(tf(
            "Could not restore the shortcut service: {error}",
            &[("error", &format!("{recovery:#}"))],
        ));
    }
    if let Some(backup) = backup {
        error = error.context(tf(
            "Shortcut backup: {file}",
            &[("file", &backup.display())],
        ));
    }
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, fs, path::PathBuf};

    struct Deferred {
        state: Cell<bool>,
        queued: Cell<Option<bool>>,
    }
    impl ServiceEditor for Deferred {
        fn persist_timeout(&self) -> Duration {
            Duration::ZERO
        }
        fn verify(&self, after: bool) -> Result<()> {
            ensure!(self.state.get() == after, "live state changed");
            Ok(())
        }
        fn apply(&self, after: bool) -> Result<()> {
            self.state.set(after);
            self.queued.set(Some(after));
            Ok(())
        }
        fn equivalent(&self, body: &str, after: bool) -> Result<bool> {
            Ok(body == if after { "value=new\n" } else { "value=old\n" })
        }
    }

    #[test]
    fn stalled_persistence_restores_live_values_and_supersedes_the_delayed_write() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("rc");
        fs::write(&file, "value=old\n").unwrap();
        let lock = WriteLock::new(&file).unwrap();
        let before = lock.check(b"value=old\n").unwrap();
        let service = Deferred {
            state: Cell::new(false),
            queued: Cell::new(None),
        };
        assert!(guarded_service_write(&lock, "value=old\n", "value=new\n", &service).is_err());
        assert!(!service.state.get());
        assert_eq!(service.queued.get(), Some(false));
        assert_eq!(lock.check(b"value=old\n").unwrap(), before);
        fs::write(
            &file,
            if service.queued.take().unwrap() {
                "value=new\n"
            } else {
                "value=old\n"
            },
        )
        .unwrap();
        assert_eq!(fs::read_to_string(file).unwrap(), "value=old\n");
    }

    struct Service {
        file: PathBuf,
        state: Cell<bool>,
        fail: bool,
        stale: bool,
    }
    impl ServiceEditor for Service {
        fn verify(&self, after: bool) -> Result<()> {
            ensure!(!(after && self.stale), "preferences changed");
            self.owns(after)
        }
        fn owns(&self, after: bool) -> Result<()> {
            ensure!(self.state.get() == after, "live state changed");
            Ok(())
        }
        fn apply(&self, after: bool) -> Result<()> {
            self.state.set(after);
            fs::write(
                &self.file,
                if after { "value=new\n" } else { "value=old\n" },
            )?;
            if after && self.fail {
                anyhow::bail!("native save failed");
            }
            Ok(())
        }
        fn equivalent(&self, body: &str, after: bool) -> Result<bool> {
            Ok(body
                .lines()
                .filter(|line| !line.starts_with('#'))
                .eq([if after { "value=new" } else { "value=old" }]))
        }
    }

    #[test]
    fn service_serialization_keeps_original_comments_symlinks_and_a_pre_request_backup() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("rc");
        let alias = directory.path().join("settings");
        fs::write(&file, "# personal\nvalue=old\n").unwrap();
        symlink(&file, &alias).unwrap();
        let lock = WriteLock::new(&alias).unwrap();
        let service = Service {
            file: file.clone(),
            state: Cell::new(false),
            fail: false,
            stale: false,
        };
        guarded_service_write(
            &lock,
            "# personal\nvalue=old\n",
            "# personal\nvalue=new\n",
            &service,
        )
        .unwrap();
        assert!(alias.is_symlink());
        assert_eq!(fs::read_to_string(file).unwrap(), "# personal\nvalue=new\n");
        assert!(
            fs::read_dir(directory.path())
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .contains("mandara-backup")
                    && fs::read_to_string(entry.path()).unwrap() == "# personal\nvalue=old\n")
        );
    }

    #[test]
    fn a_failed_native_call_restores_live_state_and_original_file_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("rc");
        let before = "# personal\nvalue=old\n";
        fs::write(&file, before).unwrap();
        let service = Service {
            file: file.clone(),
            state: Cell::new(false),
            fail: true,
            stale: false,
        };
        let lock = WriteLock::new(&file).unwrap();
        assert!(guarded_service_write(&lock, before, "# personal\nvalue=new\n", &service).is_err());
        assert!(!service.state.get());
        assert_eq!(fs::read_to_string(file).unwrap(), before);
    }

    #[test]
    fn changed_preferences_do_not_prevent_restoring_values_owned_by_a_failed_save() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("rc");
        let before = "# personal\nvalue=old\n";
        fs::write(&file, before).unwrap();
        let service = Service {
            file: file.clone(),
            state: Cell::new(false),
            fail: false,
            stale: true,
        };
        let lock = WriteLock::new(&file).unwrap();
        assert!(guarded_service_write(&lock, before, "# personal\nvalue=new\n", &service).is_err());
        assert!(!service.state.get());
        assert_eq!(fs::read_to_string(file).unwrap(), before);
    }
}
