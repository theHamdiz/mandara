//! What app theming changed outside Mandara's own folders, so switching an
//! app off takes back exactly that and nothing else.
//!
//! Every write the theme sync makes outside `~/.cache/mandara` and
//! `~/.config/mandara` goes through this ledger:
//!
//! * **Owned theme files** (`~/.config/ghostty/themes/mandara`,
//!   `kitty/themes/mandara.conf`, …): a file or symlink already at that path
//!   that Mandara didn't write is moved to `<path>.mandara-backup` first.
//!   Switched off, the file is removed if it is still exactly what Mandara
//!   last wrote (a user's edits are kept), the backup goes back in its
//!   place, and folders Mandara created for it go again when empty.
//! * **Include / import lines** (`gtk.css`, `fuzzel.ini`): switched off,
//!   only Mandara's lines are removed; a file Mandara created that is then
//!   empty is deleted. The one-time `.mandara-backup` of the user's file is
//!   kept.
//! * **Kvantum activation**: `kvantum.kvconfig` is backed up before
//!   `kvantummanager --set Mandara`. Switched off, Mandara's link goes and
//!   the backup returns while Kvantum still selects Mandara.
//!
//! Anything without a ledger entry is never touched: files an older Mandara
//! wrote (0.1 kept no ledger) stay as they are until the app is switched on
//! and off again. The ledger itself lives in `~/.config/mandara/generated/`
//! ([`mandara_config`'s `paths::app_themes_ledger`]), beside the other
//! machine-written files, so clearing caches never forgets what to restore.

mod journal;
mod undo_file;

use std::collections::BTreeMap;
use super::SyncReport;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Text every template writes, so a file an older Mandara wrote without a
/// ledger is recognised as Mandara's own (and not backed up as the user's).
pub const MARKER: &str = "Mandara Shell";
/// Suffix of a backed-up user file.
pub const BACKUP_SUFFIX: &str = "mandara-backup";
/// Bound ownership metadata independently of an edited file's size.
const MAX_LEDGER_BYTES: u64 = 8 * 1024 * 1024;

/// 64-bit FNV-1a of `bytes`: enough to tell whether a file is still what
/// Mandara wrote (not a security boundary).
pub use super::fingerprint::fnv1a;

fn hash_hex(bytes: &[u8]) -> String {
    format!("{:016x}", fnv1a(bytes))
}

/// A theme file Mandara writes at a path it doesn't otherwise own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// The integration that wrote it (`ghostty`, `kitty`, …).
    pub app: String,
    /// FNV-1a of what Mandara last wrote, in hex.
    pub hash: String,
    /// Where the user's own file (or symlink) that sat here was moved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<PathBuf>,
    /// Folders created for it, deepest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created_dirs: Vec<PathBuf>,
}

/// Lines Mandara added to a user's config file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockEntry {
    pub app: String,
    /// Older entries used their map key as the path. Several extensions may
    /// now add independent imports to the same config file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    pub lines: Vec<String>,
    /// The file didn't exist before Mandara added the lines.
    #[serde(default)]
    pub created: bool,
    /// Folders created for it, deepest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created_dirs: Vec<PathBuf>,
}

/// Kvantum activation (`~/.config/Kvantum/Mandara` and the selection).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvantumEntry {
    /// Mandara's symlink and the theme folder it points at.
    pub link: PathBuf,
    pub target: PathBuf,
    /// Older records only tracked links Mandara created.
    #[serde(default = "legacy_owned_link")]
    pub link_created: bool,
    /// The user's `kvantum.kvconfig` before Mandara selected its theme.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kvconfig_backup: Option<PathBuf>,
    /// There was no `kvantum.kvconfig` before.
    #[serde(default)]
    pub kvconfig_created: bool,
}

fn legacy_owned_link() -> bool { true }

/// What the theme sync changed outside Mandara's own folders.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending: Option<Box<journal::Pending>>,
    #[serde(skip)]
    record: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, FileEntry>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub blocks: BTreeMap<String, BlockEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kvantum: Option<KvantumEntry>,
    /// Changed since loaded.
    #[serde(skip)]
    dirty: bool,
    #[serde(skip)]
    errors: BTreeMap<String, String>,
}

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn recorded_identity(path: &Path, errors: &mut BTreeMap<String, String>) -> std::io::Result<Option<PathBuf>> {
    match super::destination::identity(path) {
        Ok(path) => Ok(Some(path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            errors.insert(path.display().to_string(), format!("app theme ownership destination is unreadable: {error}"));
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// A free backup path for `path`: `<path>.mandara-backup`, else with a
/// timestamp (and a counter if that is taken too).
pub fn backup_path(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let plain = path.with_file_name(format!("{name}.{BACKUP_SUFFIX}"));
    if !exists(&plain) {
        return plain;
    }
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default();
    (0u32..)
        .map(|n| {
            path.with_file_name(if n == 0 { format!("{name}.{BACKUP_SUFFIX}-{secs}") } else { format!("{name}.{BACKUP_SUFFIX}-{secs}-{n}") })
        })
        .find(|p| !exists(p))
        .unwrap_or(plain)
}

/// The ancestors of `path` that don't exist yet, deepest first.
fn missing_dirs(path: &Path) -> Vec<PathBuf> {
    path.ancestors().skip(1).take_while(|d| !d.as_os_str().is_empty() && !exists(d)).map(Path::to_path_buf).collect()
}

/// Removes folders Mandara created (deepest first) while they are empty.
fn remove_empty(dirs: &[PathBuf]) {
    for dir in dirs {
        if std::fs::remove_dir(dir).is_err() {
            break;
        }
    }
}

impl Ledger {
    /// Reads the ledger at `path` (empty when missing or unreadable).
    pub fn load(path: &Path) -> Self {
        Self::read(path).unwrap_or_else(|error| {
            tracing::warn!("app theme ownership {}: {error}", path.display());
            Self::default()
        })
    }

    /// A damaged ownership record must be reported before any new write.
    pub fn read(path: &Path) -> std::io::Result<Self> {
        use std::io::Read as _;
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && std::fs::symlink_metadata(path).is_err() => {
                return Ok(Self { record: Some(path.into()), ..Self::default() });
            }
            Err(error) => return Err(error),
        };
        let mut text = String::new();
        file.take(MAX_LEDGER_BYTES + 1).read_to_string(&mut text)?;
        if text.len() as u64 > MAX_LEDGER_BYTES {
            return Err(std::io::Error::other(
                "app-theme ownership record is too large",
            ));
        }
        let mut ledger: Self = toml::from_str(&text).map_err(std::io::Error::other)?;
        ledger.record = Some(path.into());
        ledger.settle()?;
        Ok(ledger)
    }

    /// Writes the ledger if it changed (removing the file once empty).
    pub fn store(&mut self, path: &Path) -> std::io::Result<()> {
        self.record = Some(path.into());
        self.settle()?;
        self.checkpoint()
    }

    fn checkpoint(&mut self) -> std::io::Result<()> {
        let Some(path) = &self.record else { return Ok(()); };
        if !self.dirty { return Ok(()); }
        if self.is_empty() {
            if exists(path) {
                std::fs::remove_file(path)?;
                super::file_io::sync_parent(path)?;
            }
        } else {
            let body = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
            if body.len() as u64 > MAX_LEDGER_BYTES {
                return Err(std::io::Error::other("app-theme ownership record is too large"));
            }
            super::file_io::write(path, &body)?;
        }
        self.dirty = false;
        Ok(())
    }

    pub fn take_errors(&mut self) -> BTreeMap<String, String> {
        std::mem::take(&mut self.errors)
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_none() && self.files.is_empty() && self.blocks.is_empty() && self.kvantum.is_none()
    }

    /// Whether `app` has anything applied.
    pub fn has(&self, app: &str) -> bool {
        self.pending.as_ref().is_some_and(|pending| pending.owner() == app) || self.files.values().any(|f| f.app == app) || self.blocks.values().any(|b| b.app == app)
    }

    fn check_destination(&self, identity: &Path) -> std::io::Result<()> {
        if let Some(record) = &self.record && super::destination::identity(record)? == identity {
            return Err(std::io::Error::other("a theme cannot replace its ownership record"));
        }
        Ok(())
    }

    /// Writes `body` to `path` for `app`, backing up a file there that isn't
    /// Mandara's first.
    pub fn put_file(&mut self, app: &str, path: &Path, body: &str) -> std::io::Result<()> {
        let mut written = None;
        let result = self.put_file_with(app, path, body, |path, body| {
            written = super::file_io::write_recorded(path, body)?;
            Ok(())
        });
        result.map_err(|error| match written {
            Some(written) => std::io::Error::other(super::PublishedWriteError::new(&written.target, written.stamp, error)),
            None => error,
        })
    }

    fn put_file_with(&mut self, app: &str, path: &Path, body: &str, write: impl FnOnce(&Path, &str) -> std::io::Result<()>) -> std::io::Result<()> {
        self.settle()?;
        let new_dirs = missing_dirs(path);
        let _lock = super::WriteLock::new(path)?;
        let slot = super::destination::slot(path)?;
        let identity = super::destination::identity(path)?;
        self.check_destination(&identity)?;
        let mut k = key(&slot);
        for known in self.files.keys() {
            if recorded_identity(Path::new(known), &mut self.errors)?.as_ref() == Some(&identity) {
                k = known.clone();
                break;
            }
        }
        for (key, block) in &self.blocks {
            if recorded_identity(block.path.as_deref().unwrap_or_else(|| Path::new(key)), &mut self.errors)?.as_ref() == Some(&identity) {
                return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "a whole-file theme cannot replace managed import lines"));
            }
        }
        let owned_path = PathBuf::from(&k);
        let path = owned_path.as_path();
        if self.files.get(&k).is_some_and(|entry| entry.app != app) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{} is managed by another app template", path.display()),
            ));
        }
        if let Ok(metadata) = std::fs::symlink_metadata(path)
            && (!metadata.is_file() && !metadata.file_type().is_symlink()
                || metadata.file_type().is_symlink()
                    && std::fs::metadata(path).is_ok_and(|target| !target.is_file()))
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{} is not a regular file", path.display()),
            ));
        }
        let is_link = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
        let mut entry = self.files.get(&k).cloned().unwrap_or_else(|| FileEntry { app: app.to_string(), ..Default::default() });
        let known = self.files.contains_key(&k);
        let mut moved = None;
        if exists(path) {
            let ours_now = !is_link && if known {
                super::fingerprint::matches(path, &entry.hash)?
            } else {
                // Imported and user-authored templates never claim an original
                // merely because its contents happen to mention Mandara.
                !app.contains([':', '/']) && super::file_io::read(path).is_ok_and(|body| body.contains(MARKER))
            };
            if !ours_now {
                let backup = backup_path(path);
                // The first backup is the user's original; later ones (their
                // edits of Mandara's file) are kept beside it.
                if entry.backup.is_none() {
                    entry.backup = Some(backup.clone());
                }
                moved = Some(backup);
            }
        } else if !known {
            entry.created_dirs = new_dirs;
        }
        entry.app = app.to_string();
        entry.hash = hash_hex(body.as_bytes());
        if moved.is_none() && exists(path) && self.files.get(&k) == Some(&entry) {
            return Ok(());
        }
        self.begin_file(&k, &entry, moved.as_deref())?;
        if let Some(backup) = &moved {
            std::fs::rename(path, backup)?;
            if !is_link { std::fs::File::open(backup)?.sync_all()?; }
            super::file_io::sync_parent(backup)?;
        }
        if let Err(error) = write(path, body) {
            if let Some(backup) = moved && let Err(restore) = super::restore::no_clobber(&backup, path) {
                let failure = std::io::Error::other(format!("{error}; restoring {} failed: {restore}; the previous file is preserved at {}", path.display(), backup.display()));
                return Err(match super::published_write(&error) {
                    Some(written) => std::io::Error::other(super::PublishedWriteError::new(&written.target, written.stamp, failure)),
                    None => failure,
                });
            }
            return Err(error);
        }
        if self.files.get(&k) != Some(&entry) {
            self.files.insert(k, entry);
            self.dirty = true;
        }
        self.finish_file()?;
        Ok(())
    }

    /// Ensures `block` in `config` for `app` (see [`super::ensure_block`]).
    pub fn put_block(&mut self, app: &str, config: &Path, block: &[&str], placement: super::Placement) -> std::io::Result<bool> {
        self.settle()?;
        let new_dirs = missing_dirs(config);
        let _lock = super::WriteLock::new(config)?;
        let identity = super::destination::identity(config)?;
        self.check_destination(&identity)?;
        for known in self.files.keys() {
            if recorded_identity(Path::new(known), &mut self.errors)?.as_ref() == Some(&identity) {
                return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "managed import lines cannot edit a whole-file theme"));
            }
        }
        let config = identity.as_path();
        let created = !exists(config);
        let dirs = if created { new_dirs } else { Vec::new() };
        let k = self
            .blocks
            .iter()
            .find(|(key, entry)| {
                entry.app == app
                    && entry.path.as_deref().unwrap_or_else(|| Path::new(key)) == config
            })
            .map(|(key, _)| key.clone())
            .unwrap_or_else(|| {
                let path = key(config);
                if self.blocks.contains_key(&path) {
                    (0_u32..)
                        .map(|index| format!("{path}#mandara:{app}:{index}"))
                        .find(|key| !self.blocks.contains_key(key))
                        .unwrap_or(path)
                } else {
                    path
                }
            });
        let previous = self.blocks.get(&k).cloned();
        let before = super::file_io::read(config)?;
        if previous.is_none() && journal::contains_block(&before, block.iter().copied()) {
            return Ok(false);
        }
        if previous.as_ref().is_some_and(|entry| entry.lines.iter().map(String::as_str).eq(block.iter().copied()))
            && super::block_body(&before, block, placement) == before
        {
            return Ok(false);
        }
        let lines: Vec<String> = block.iter().map(|line| (*line).to_owned()).collect();
        drop(before);
        let entry = match &previous {
            Some(e) => BlockEntry { app: app.to_string(), lines, ..e.clone() },
            None => BlockEntry { app: app.to_string(), path: Some(config.into()), lines, created, created_dirs: dirs },
        };
        self.begin_block(&k, &entry)?;
        let mut changed = false;
        if let Some(previous) = &previous
            && !previous.lines.iter().map(String::as_str).eq(block.iter().copied())
        {
            let lines = previous.lines.iter().map(String::as_str).collect::<Vec<_>>();
            changed = super::remove_block(config, &lines)?;
        }
        changed |= super::ensure_block(config, block, placement)?;
        if !entry.lines.is_empty() || entry.created {
            if self.blocks.get(&k) != Some(&entry) { self.blocks.insert(k, entry); self.dirty = true; }
        } else if self.blocks.remove(&k).is_some() { self.dirty = true; }
        self.finish_file()?;
        Ok(changed)
    }

    /// Takes back everything `app` applied. Returns the paths changed.
    pub fn take_back(&mut self, app: &str) -> Vec<PathBuf> {
        self.take_back_except(app, &[])
    }

    /// Retire destinations removed by an edited or updated template.
    pub fn take_back_except(&mut self, app: &str, keep: &[PathBuf]) -> Vec<PathBuf> {
        self.take_back_except_checked(app, keep).into_files()
    }

    /// Restore an integration, retaining failed ownership records for a retry.
    pub fn take_back_checked(&mut self, app: &str) -> SyncReport {
        self.take_back_except_checked(app, &[])
    }

    pub fn take_back_except_checked(&mut self, app: &str, keep: &[PathBuf]) -> SyncReport {
        let mut changed = SyncReport::default();
        if let Err(error) = self.settle() {
            changed.failed(self.record.as_deref().unwrap_or_else(|| Path::new("app themes")), error);
            return changed;
        }
        let mut resolved = Vec::with_capacity(keep.len());
        for path in keep {
            match super::destination::identity(path) {
                Ok(path) => resolved.push(path),
                Err(error) => { changed.failed(path, error); return changed; }
            }
        }
        let kept = |path: &Path| super::destination::identity(path).is_ok_and(|path| resolved.contains(&path));
        let files: Vec<String> = self.files.iter().filter(|(k, e)| e.app == app && !kept(Path::new(k))).map(|(k, _)| k.clone()).collect();
        for k in files {
            if let Err(error) = self.restore_file(&k, &mut changed) {
                changed.failed(Path::new(&k), error);
                return changed;
            }
        }
        let blocks: Vec<String> = self.blocks.iter().filter(|(k, e)| {
                e.app == app
                    && !kept(e.path.as_deref().unwrap_or_else(|| Path::new(k)))
            }).map(|(k, _)| k.clone()).collect();
        for k in blocks {
            let Some(entry) = self.blocks.remove(&k) else { continue;
            };
            self.dirty = true;
            let path = entry.path.clone().unwrap_or_else(|| PathBuf::from(&k));
            let lines: Vec<&str> = entry.lines.iter().map(String::as_str).collect();
            match super::remove_block(&path, &lines) {
                Ok(true) => changed.files.push(path.clone()),
                Ok(false) => {}
                Err(err) => {
                    changed.failed(&path, err);
                    self.blocks.insert(k, entry);
                    continue;
                }
            }
            if entry.created
                && let Some(other) = self.blocks.iter_mut().find_map(|(key, other)| {
                    (other.path.as_deref().unwrap_or_else(|| Path::new(key)) == path)
                        .then_some(other)
                })
            {
                other.created = true;
                other.created_dirs.clone_from(&entry.created_dirs);
            }
            if entry.created && !exists(&path) {
                remove_empty(&entry.created_dirs);
            } else if entry.created && super::file_io::read(&path).is_ok_and(|t| t.trim().is_empty()) {
                match std::fs::remove_file(&path) {
                    Ok(()) => remove_empty(&entry.created_dirs),
                    Err(error) => { changed.failed(&path, error);self.blocks.insert(k,entry); }
                }
            }
        }
        changed
    }

    /// Records Kvantum activation before it happens (once): the link and a
    /// backup of `kvconfig`.
    pub fn record_kvantum(&mut self, link: &Path, target: &Path, kvconfig: &Path) -> std::io::Result<()> {
        self.record_kvantum_link(link, target, kvconfig, true)
    }

    pub(super) fn record_kvantum_link(&mut self, link: &Path, target: &Path, kvconfig: &Path, link_created: bool) -> std::io::Result<()> {
        self.settle()?;
        if let Some(entry) = &mut self.kvantum {
            if entry.link_created && entry.target != target { entry.target = target.into(); self.dirty = true; }
            return self.checkpoint();
        }
        let (kvconfig_backup, kvconfig_created) = if exists(kvconfig) {
            let backup = backup_path(kvconfig);
            std::fs::copy(kvconfig, &backup)?;
            std::fs::File::open(&backup)?.sync_all()?;
            super::file_io::sync_parent(&backup)?;
            (Some(backup), false)
        } else {
            (None, true)
        };
        self.kvantum = Some(KvantumEntry { link: link.to_path_buf(), target: target.to_path_buf(), link_created, kvconfig_backup, kvconfig_created });
        self.dirty = true;
        self.checkpoint()
    }

    /// Takes back Kvantum activation: Mandara's link goes and, while Kvantum
    /// still selects `theme`, the user's `kvconfig` returns.
    pub fn take_back_kvantum(&mut self, kvconfig: &Path, theme: &str) -> Vec<PathBuf> {
        self.take_back_kvantum_checked(kvconfig, theme).into_files()
    }

    pub fn take_back_kvantum_checked(&mut self, kvconfig: &Path, theme: &str) -> SyncReport {
        let Some(entry) = self.kvantum.take() else { return SyncReport::default(); };
        self.dirty = true;
        let mut changed = SyncReport::default();
        match super::kvantum_selection::restore(kvconfig, entry.kvconfig_backup.as_deref(), entry.kvconfig_created, theme) {
            Ok(true) => changed.files.push(kvconfig.into()),
            Ok(false) => {},
            Err(error) => changed.failed(kvconfig, error),
        }
        if changed.errors.is_empty() && entry.link_created && std::fs::read_link(&entry.link).is_ok_and(|target| target == entry.target) {
            match std::fs::remove_file(&entry.link) {
                Ok(()) => changed.files.push(entry.link.clone()),
                Err(error) => changed.failed(&entry.link, error),
            }
        }
        if !changed.errors.is_empty() { self.kvantum = Some(entry); }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_destinations_with_a_brand_mention_still_keep_the_original() {
        let root = tempfile::tempdir().unwrap(); let path = root.path().join("profile");
        let original = "My settings mention Mandara Shell but belong to me\n"; std::fs::write(&path, original).unwrap();
        let mut ledger = Ledger::default(); ledger.put_file("community:example/main", &path, "generated").unwrap();
        assert!(ledger.take_back_checked("community:example/main").errors.is_empty()); assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    }

    #[test]
    fn missing_kvantum_backups_keep_the_active_theme_link_for_retry() {
        let root = tempfile::tempdir().unwrap(); let link = root.path().join("Mandara"); let target = root.path().join("theme"); let config = root.path().join("kvantum.kvconfig");
        std::fs::create_dir(&target).unwrap(); std::fs::write(&config, "[General]\ntheme=Personal\n").unwrap();
        let mut ledger = Ledger::default(); ledger.record_kvantum(&link, &target, &config).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap(); std::fs::write(&config, "[General]\ntheme=Mandara\n").unwrap();
        std::fs::remove_file(ledger.kvantum.as_ref().unwrap().kvconfig_backup.as_ref().unwrap()).unwrap();
        assert!(!ledger.take_back_kvantum_checked(&config, "Mandara").errors.is_empty());
        assert_eq!(std::fs::read_link(link).unwrap(), target); assert!(ledger.kvantum.is_some());
    }

    #[test]
    fn failed_first_writes_keep_an_edit_that_reappears_at_the_destination() {
        let root = tempfile::tempdir().unwrap(); let path = root.path().join("theme"); std::fs::write(&path, "original").unwrap();
        let mut ledger = Ledger::default();
        let error = ledger.put_file_with("app", &path, "theme", |path, _| { std::fs::write(path, "newer edit")?; Err(std::io::Error::other("write conflict")) }).unwrap_err();
        assert!(error.to_string().contains("previous file is preserved")); assert_eq!(std::fs::read_to_string(&path).unwrap(), "newer edit");
        assert_eq!(std::fs::read_to_string(path.with_file_name("theme.mandara-backup")).unwrap(), "original");
    }

    #[test]
    fn a_damaged_destination_does_not_block_unrelated_file_or_import_writes() {
        let root = tempfile::tempdir().unwrap(); let broken = root.path().join("a-broken");
        let mut ledger = Ledger::default(); ledger.put_file("old", &broken, "theme").unwrap();
        std::fs::remove_file(&broken).unwrap(); std::os::unix::fs::symlink(root.path().join("missing"), &broken).unwrap();
        let valid = root.path().join("z-valid"); ledger.put_file("new", &valid, "new theme").unwrap();
        let imports = root.path().join("z-imports"); ledger.put_block("imports", &imports, &["import colors"], super::super::Placement::AfterImports).unwrap();
        assert_eq!(std::fs::read_to_string(valid).unwrap(), "new theme");
        assert!(std::fs::read_to_string(imports).unwrap().contains("import colors"));
        assert!(ledger.take_errors().contains_key(broken.to_str().unwrap()));
        assert!(ledger.has("old"));
    }

    #[test]
    fn failed_file_writes_keep_committed_ownership_and_return_moved_user_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("colors");
        std::fs::write(&path, "original").unwrap();
        let mut ledger = Ledger::default();
        let fail = |_: &Path, _: &str| Err(std::io::Error::other("failed write"));
        assert!(ledger.put_file_with("app", &path, "A", fail).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
        assert!(ledger.is_empty());
        ledger.put_file("app", &path, "A").unwrap();
        let committed = ledger.files.clone();
        assert!(ledger.put_file_with("app", &path, "B", fail).is_err());
        assert_eq!(ledger.files, committed);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "A");
        ledger.put_file("app", &path, "B").unwrap();
        assert!(ledger.take_back_checked("app").errors.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
    }

    #[test]
    fn dangling_record_links_are_rejected_without_replacing_ownership() {
        let root = tempfile::tempdir().unwrap();
        let record = root.path().join("ledger");
        let target = root.path().join("missing");
        std::os::unix::fs::symlink(&target, &record).unwrap();
        assert!(Ledger::read(&record).is_err());
        assert_eq!(std::fs::read_link(&record).unwrap(), target);
    }

    #[test]
    fn whole_files_and_import_blocks_cannot_take_each_others_destinations() {
        for file_first in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("gtk.css");
            std::fs::write(&path, "original\n").unwrap();
            let mut ledger = Ledger::default();
            if file_first {
                ledger.put_file("custom", &path, "themed").unwrap();
                assert!(ledger.put_block("gtk", &path, &["@import 'theme';"], super::super::Placement::AfterImports).is_err());
                ledger.take_back_checked("custom");
            } else {
                ledger.put_block("gtk", &path, &["@import 'theme';"], super::super::Placement::AfterImports).unwrap();
                assert!(ledger.put_file("custom", &path, "themed").is_err());
                ledger.take_back_checked("gtk");
            }
            assert_eq!(std::fs::read_to_string(path).unwrap(), "original\n");
        }
    }

    #[test]
    fn directory_and_file_aliases_share_ownership_and_keep_lists() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app");
        let path = app.join("colors");
        let alias = root.path().join("alias");
        let mut ledger = Ledger::default();
        ledger.put_file("one", &path, "first").unwrap();
        std::os::unix::fs::symlink(&app, &alias).unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        for other in [alias.join("colors"), app.join("missing/../colors"), link] {
            assert!(ledger.put_file("two", &other, "second").is_err());
            assert!(!ledger.put_block("two", &other, &["import"], super::super::Placement::AfterImports).unwrap_or(false));
        }
        ledger.put_file("one", &alias.join("colors"), "updated").unwrap();
        assert_eq!(ledger.files.len(), 1);
        assert!(ledger.take_back_except_checked("one", &[alias.join("colors")]).errors.is_empty());
        assert!(path.exists());
        assert!(ledger.take_back_checked("one").errors.is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn a_missing_original_backup_does_not_delete_the_only_remaining_copy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("colors");
        std::fs::write(&path,"original").unwrap();
        let mut ledger = Ledger::default();
        ledger.put_file("app",&path,"generated").unwrap();
        let backup = ledger.files.values().next().unwrap().backup.clone().unwrap();
        let saved = std::fs::read(&backup).unwrap();
        std::fs::remove_file(&backup).unwrap();
        ledger.take_back("app");
        assert_eq!(std::fs::read_to_string(&path).unwrap(),"generated");
        assert!(ledger.has("app"));
        std::fs::write(backup,saved).unwrap();
        ledger.take_back("app");
        assert_eq!(std::fs::read_to_string(path).unwrap(),"original");
        assert!(!ledger.has("app"));
    }

    #[test]
    fn changing_an_import_retires_the_old_line_and_invalid_records_are_reported() {
        let directory = tempfile::tempdir().unwrap();
        let css = directory.path().join("gtk.css");
        let record = directory.path().join("ledger.toml");
        let mut ledger = Ledger::default();
        ledger
            .put_block(
                "app",
                &css,
                &["@import 'old.css';"],
                super::super::Placement::AfterImports,
            )
            .unwrap();
        ledger
            .put_block(
                "app",
                &css,
                &["@import 'new.css';"],
                super::super::Placement::AfterImports,
            )
            .unwrap();
        let text = std::fs::read_to_string(&css).unwrap();
        assert!(text.contains("new.css") && !text.contains("old.css"));
        ledger.take_back("app");
        assert!(!css.exists());
        std::fs::write(&record, "not toml").unwrap();
        assert!(Ledger::read(&record).is_err());
    }

    #[test]
    fn independent_extensions_share_import_files_and_remove_only_their_own_lines() {
        let directory = tempfile::tempdir().unwrap();
        let css = directory.path().join("gtk/gtk.css");
        let record = directory.path().join("ledger.toml");
        let mut ledger = Ledger::default();
        ledger
            .put_block(
                "one",
                &css,
                &["@import 'one.css';"],
                super::super::Placement::AfterImports,
            )
            .unwrap();
        ledger
            .put_block(
                "two",
                &css,
                &["@import 'two.css';"],
                super::super::Placement::AfterImports,
            )
            .unwrap();
        ledger.store(&record).unwrap();
        let mut ledger = Ledger::load(&record);
        ledger.take_back("one");
        let text = std::fs::read_to_string(&css).unwrap();
        assert!(!text.contains("one.css") && text.contains("two.css"));
        ledger.take_back("two");
        assert!(!css.exists());
        assert!(ledger.is_empty());
    }

    #[test]
    fn templates_cannot_replace_directories_or_take_another_templates_destination() {
        let directory = tempfile::tempdir().unwrap();
        let mut ledger = Ledger::default();
        assert!(ledger.put_file("one", directory.path(), "colors").is_err());
        let path = directory.path().join("colors");
        ledger.put_file("one", &path, "first").unwrap();
        assert!(ledger.put_file("two", &path, "second").is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "first");
    }
}
