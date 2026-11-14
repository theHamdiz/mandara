//! One native override key, backed up before publication and guarded against stale edits.

use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result, ensure};
use mandara_i18n::t;
use mandara_theme::templates::{FileStamp, WriteLock};

use super::{Capability, Session};
use super::native::MAX_BYTES;

pub(super) fn read(path: &Path) -> Result<String> {
    let mut text = String::new();
    match File::open(path) {
        Ok(file) => { ensure!(file.metadata()?.is_file(), "{}", t("The Flatpak permission data is invalid or too large.")); file.take(MAX_BYTES + 1).read_to_string(&mut text)?; }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && std::fs::symlink_metadata(path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
        Err(error) => return Err(error.into()),
    }
    ensure!(text.len() as u64 <= MAX_BYTES, "{}", t("The Flatpak permission data is invalid or too large."));
    Ok(text)
}

#[derive(Clone, Debug)]
pub struct Undo {
    path: PathBuf,
    target: PathBuf,
    before: String,
    original: bool,
    written: FileStamp,
    pub backup: Option<PathBuf>,
    pub persistence_error: Option<String>,
}
impl Undo {
    /// Restore only while the exact file published by this transaction remains current.
    pub fn restore(&self) -> Result<()> {
        let lock = WriteLock::new(&self.path)?;
        ensure!(lock.target() == self.target, "{}", t("The app override moved. Refresh before using Undo."));
        lock.check_stamp(Some(self.written))?;
        if self.original {
            mandara_config::store::write_atomic_checked(&lock, Some(self.written), self.before.as_bytes())?;
        } else {
            std::fs::remove_file(lock.target())?;
            File::open(lock.target().parent().context(t("The Flatpak permission data is invalid or too large."))?)?.sync_all()?;
        }
        Ok(())
    }
}

fn backup(path: &Path, body: &str) -> Result<PathBuf> {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let path = path.with_file_name(format!("{}.mandara-{stamp}.bak", path.file_name().context(t("The Flatpak permission data is invalid or too large."))?.to_string_lossy()));
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
    file.write_all(body.as_bytes())?;
    file.sync_all()?;
    Ok(path)
}

/// Replace only the selected Context key, leaving comments and other groups byte-for-byte.
pub(super) fn merge(before: &str, generated: &str, key: &str) -> Result<String> {
    let mut in_context = false;
    let mut value = None;
    for line in generated.lines() {
        if line.trim().starts_with('[') { in_context = line.trim() == "[Context]"; }
        if in_context && let Some((name, _)) = line.split_once('=') && name.trim() == key { value = Some(line); }
    }
    let replacement = value.context(t("The Flatpak permission data is invalid or too large."))?;
    let mut result = String::with_capacity(before.len() + replacement.len());
    let mut inside = false;
    let mut seen_context = false;
    let mut replaced = false;
    for line in before.split_inclusive('\n') {
        if line.trim().starts_with('[') {
            if inside && !replaced { result.push_str(replacement); result.push('\n'); replaced = true; }
            inside = line.trim() == "[Context]";
            if inside { ensure!(!seen_context, "{}", t("The Flatpak permission data is invalid or too large.")); seen_context = true; }
        }
        if inside && line.split_once('=').is_some_and(|(name, _)| name.trim() == key) {
            ensure!(!replaced, "{}", t("The Flatpak permission data is invalid or too large."));
            result.push_str(replacement); result.push('\n'); replaced = true;
        } else { result.push_str(line); }
    }
    if !replaced {
        if !result.is_empty() && !result.ends_with('\n') { result.push('\n'); }
        if !seen_context { result.push_str("[Context]\n"); }
        result.push_str(replacement); result.push('\n');
    }
    ensure!(result.len() as u64 <= MAX_BYTES, "{}", t("The Flatpak permission data is invalid or too large."));
    Ok(result)
}

pub(super) fn apply(session: &Session, capability: &Capability, allowed: bool, cancel: &AtomicBool, revalidate: impl FnOnce() -> Result<()>) -> Result<Undo> {
    let lock = WriteLock::new(&session.path)?;
    ensure!(lock.target() == session.target, "{}", t("The app override moved. Refresh before saving."));
    lock.check_stamp(session.stamp)?;
    ensure!(read(lock.target())? == session.before, "{}", t("The app permissions changed. Refresh before saving."));
    revalidate()?;
    let scratch = tempfile::tempdir()?;
    let overrides = scratch.path().join("flatpak/overrides");
    std::fs::create_dir_all(&overrides)?;
    let generated = overrides.join(&session.app.id);
    std::fs::write(&generated, &session.before)?;
    let mut command = Command::new("flatpak");
    command.env_clear().env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", scratch.path()).env("XDG_DATA_HOME", scratch.path())
        .env("XDG_CONFIG_HOME", scratch.path().join("config"))
        .env("FLATPAK_USER_DIR", scratch.path().join("flatpak"))
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
        .env("DBUS_SYSTEM_BUS_ADDRESS", "unix:path=/nonexistent")
        .args(["override", "--user"]).arg(format!("{}={}", capability.key.flag(allowed), capability.value)).arg(&session.app.id);
    super::native::run(command, cancel)?;
    let next = merge(&session.before, &read(&generated)?, capability.key.name())?;
    ensure!(!cancel.load(Ordering::Acquire), "{}", t("Cancelled"));
    let backup = session.stamp.map(|_| backup(lock.target(), &session.before)).transpose()?;
    lock.check_stamp(session.stamp)?;
    finish(session, backup, mandara_config::store::write_atomic_checked(&lock, session.stamp, next.as_bytes()))
}

fn finish(session: &Session, backup: Option<PathBuf>, result: Result<FileStamp>) -> Result<Undo> {
    let (written, persistence_error) = match result {
        Ok(written) => (written, None),
        Err(error) => match mandara_config::store::published_stamp(&error) {
            Some(written) => (written, Some(format!("{error:#}"))),
            None => return Err(error),
        },
    };
    Ok(Undo { path: session.path.clone(), target: session.target.clone(), before: session.before.clone(), original: session.stamp.is_some(), written, backup, persistence_error })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_failed_effective_permission_check_cannot_publish_an_override() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("override");
        let before = "[Context]\nshared=network;\n";
        std::fs::write(&path, before).unwrap();
        let permissions = crate::app_install::flatpak::parse_context(before).unwrap();
        let session = Session { app: super::super::App { id: "org.example.Test".into(), name: "Test".into(), installation: "user".into() }, capabilities: super::super::model::capabilities(&permissions, &permissions), permissions, metadata: before.into(), path: path.clone(), target: path.clone(), before: before.into(), stamp: FileStamp::at(&path).unwrap() };
        let capability = session.capabilities.iter().find(|row| row.value == "network").unwrap();
        let called = std::cell::Cell::new(false);
        let result = apply(&session, capability, false, &AtomicBool::new(false), || {
            called.set(true);
            anyhow::bail!("injected effective-grant change")
        });
        assert!(called.get());
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), before);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().filter_map(Result::ok).filter(|entry| entry.file_name().to_string_lossy().ends_with(".bak")).count(), 0);
    }
    #[test]
    fn published_but_unsynced_permissions_keep_a_warning_and_a_working_undo() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("override");
        let before = "[Context]\nshared=network;\n";
        std::fs::write(&path, before).unwrap();
        let permissions = crate::app_install::flatpak::parse_context(before).unwrap();
        let session = Session { app: super::super::App { id: "org.example.Test".into(), name: "Test".into(), installation: "user".into() }, capabilities: super::super::model::capabilities(&permissions, &permissions), permissions, metadata: before.into(), path: path.clone(), target: path.clone(), before: before.into(), stamp: FileStamp::at(&path).unwrap() };
        std::fs::write(&path, "[Context]\nshared=!network;\n").unwrap();
        let stamp = FileStamp::at(&path).unwrap().unwrap();
        let error = mandara_config::store::PublishedWriteError::new(&path, stamp, std::io::Error::other("injected directory sync failure"));
        let undo = finish(&session, None, Err(error.into())).unwrap();
        assert!(undo.persistence_error.as_ref().unwrap().contains("injected directory sync failure"));
        undo.restore().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }
}
