//! Select and restore Kvantum's one owned preference without replacing other settings.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// A local settings command should finish promptly; a stuck manager is reaped.
pub(super) const TIMEOUT: Duration = Duration::from_secs(10);
/// Poll only while the short-lived selection command is running.
const POLL: Duration = Duration::from_millis(10);

pub(super) fn select(mut command: Command, timeout: Duration) -> io::Result<()> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + timeout;
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break Ok(()),
            Ok(Some(status)) => {
                break Err(io::Error::other(format!(
                    "Kvantum theme selection failed ({status})"
                )));
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            Ok(None) => {
                break Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Kvantum theme selection timed out",
                ));
            }
            Err(error) => break Err(error),
        }
    };
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn theme(body: &str) -> Option<&str> {
    let mut general = false;
    let mut selected = None;
    for line in body.lines().map(str::trim) {
        if line.starts_with('[') {
            general = line == "[General]";
        } else if general
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "theme"
        {
            selected = Some(value.trim());
        }
    }
    selected
}

pub(super) fn restore(
    path: &Path,
    backup: Option<&Path>,
    created: bool,
    owned: &str,
) -> io::Result<bool> {
    if super::FileStamp::at(path)?.is_none() {
        return Ok(false);
    }
    let lock = super::WriteLock::new(path)?;
    let current = super::file_io::read(path)?;
    if theme(&current) != Some(owned) {
        return Ok(false);
    }
    if let Some(backup) = backup
        && super::FileStamp::at(backup)?.is_none()
    {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "original Kvantum selection backup is missing",
        ));
    }
    let original = backup
        .map(super::file_io::read)
        .transpose()?
        .unwrap_or_default();
    let previous = theme(&original);
    let mut general = false;
    let mut next = String::with_capacity(current.len());
    for line in current.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            general = trimmed == "[General]";
        }
        if general
            && trimmed
                .split_once('=')
                .is_some_and(|(key, _)| key.trim() == "theme")
        {
            if let Some(previous) = previous {
                next.push_str("theme=");
                next.push_str(previous);
                next.push('\n');
            }
        } else {
            next.push_str(line);
        }
    }
    lock.check(current.as_bytes())?;
    if created
        && next
            .lines()
            .all(|line| line.trim().is_empty() || line.trim() == "[General]")
    {
        std::fs::remove_file(lock.target())?;
    } else {
        super::file_io::write(path, &next)?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn undo_restores_only_the_selection_and_keeps_symlinks_and_later_settings() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("real");
        let path = root.path().join("config");
        let backup = root.path().join("backup");
        std::fs::write(&backup, "[General]\ntheme=Personal\nOther=Old\n").unwrap();
        std::fs::write(
            &target,
            "[General]\ntheme=Mandara\nOther=New\n[App]\nSetting=Keep\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(restore(&path, Some(&backup), false, "Mandara").unwrap());
        assert_eq!(std::fs::read_link(&path).unwrap(), target);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[General]\ntheme=Personal\nOther=New\n[App]\nSetting=Keep\n"
        );
        assert!(!restore(&path, Some(&backup), false, "Mandara").unwrap());
    }
    #[test]
    fn a_missing_backup_preserves_the_active_selection_for_a_later_retry() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        std::fs::write(&path, "[General]\ntheme=Mandara\n").unwrap();
        assert!(restore(&path, Some(&root.path().join("missing")), false, "Mandara").is_err());
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "[General]\ntheme=Mandara\n"
        );
    }

    #[test]
    fn a_hung_selector_is_killed_and_reaped() {
        let mut command = Command::new("sleep");
        command.arg("30");
        assert_eq!(
            select(command, Duration::ZERO).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
    }
}
