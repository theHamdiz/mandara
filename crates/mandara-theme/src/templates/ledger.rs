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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Text every template writes, so a file an older Mandara wrote without a
/// ledger is recognised as Mandara's own (and not backed up as the user's).
pub const MARKER: &str = "Mandara Shell";
/// Suffix of a backed-up user file.
pub const BACKUP_SUFFIX: &str = "mandara-backup";

/// 64-bit FNV-1a of `bytes`: enough to tell whether a file is still what
/// Mandara wrote (not a security boundary).
pub fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET, |h, b| (h ^ u64::from(*b)).wrapping_mul(PRIME))
}

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
    /// The user's `kvantum.kvconfig` before Mandara selected its theme.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kvconfig_backup: Option<PathBuf>,
    /// There was no `kvantum.kvconfig` before.
    #[serde(default)]
    pub kvconfig_created: bool,
}

/// What the theme sync changed outside Mandara's own folders.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, FileEntry>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub blocks: BTreeMap<String, BlockEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kvantum: Option<KvantumEntry>,
    /// Changed since loaded.
    #[serde(skip)]
    dirty: bool,
}

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
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
        .map(|n| path.with_file_name(if n == 0 { format!("{name}.{BACKUP_SUFFIX}-{secs}") } else { format!("{name}.{BACKUP_SUFFIX}-{secs}-{n}") }))
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
        std::fs::read_to_string(path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    /// Writes the ledger if it changed (removing the file once empty).
    pub fn store(&mut self, path: &Path) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        if self.is_empty() {
            if exists(path) {
                std::fs::remove_file(path)?;
            }
        } else {
            let body = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("mandara-tmp");
            std::fs::write(&tmp, body)?;
            std::fs::rename(tmp, path)?;
        }
        self.dirty = false;
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.blocks.is_empty() && self.kvantum.is_none()
    }

    /// Whether `app` has anything applied.
    pub fn has(&self, app: &str) -> bool {
        self.files.values().any(|f| f.app == app) || self.blocks.values().any(|b| b.app == app)
    }

    /// Writes `body` to `path` for `app`, backing up a file there that isn't
    /// Mandara's first.
    pub fn put_file(&mut self, app: &str, path: &Path, body: &str) -> std::io::Result<()> {
        let k = key(path);
        let current = std::fs::read(path).ok();
        let is_link = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
        let mut entry = self.files.get(&k).cloned().unwrap_or_else(|| FileEntry { app: app.to_string(), ..Default::default() });
        let known = self.files.contains_key(&k);
        if exists(path) {
            let ours_now = !is_link && current.as_deref().is_some_and(|c| if known { hash_hex(c) == entry.hash } else { String::from_utf8_lossy(c).contains(MARKER) });
            if !ours_now {
                let backup = backup_path(path);
                std::fs::rename(path, &backup)?;
                // The first backup is the user's original; later ones (their
                // edits of Mandara's file) are kept beside it.
                if entry.backup.is_none() {
                    entry.backup = Some(backup);
                }
            }
        } else if !known {
            entry.created_dirs = missing_dirs(path);
        }
        super::write(path, body)?;
        entry.app = app.to_string();
        entry.hash = hash_hex(body.as_bytes());
        if self.files.get(&k) != Some(&entry) {
            self.files.insert(k, entry);
            self.dirty = true;
        }
        Ok(())
    }

    /// Ensures `block` in `config` for `app` (see [`super::ensure_block`]).
    pub fn put_block(&mut self, app: &str, config: &Path, block: &[&str], placement: super::Placement) -> std::io::Result<bool> {
        let created = !exists(config);
        let dirs = if created { missing_dirs(config) } else { Vec::new() };
        let changed = super::ensure_block(config, block, placement)?;
        let k = key(config);
        let lines: Vec<String> = block.iter().map(|l| l.to_string()).collect();
        let entry = match self.blocks.get(&k) {
            Some(e) => BlockEntry { app: app.to_string(), lines, ..e.clone() },
            None => BlockEntry { app: app.to_string(), lines, created, created_dirs: dirs },
        };
        if self.blocks.get(&k) != Some(&entry) {
            self.blocks.insert(k, entry);
            self.dirty = true;
        }
        Ok(changed)
    }

    /// Takes back everything `app` applied. Returns the paths changed.
    pub fn take_back(&mut self, app: &str) -> Vec<PathBuf> {
        let mut changed = Vec::new();
        let files: Vec<String> = self.files.iter().filter(|(_, e)| e.app == app).map(|(k, _)| k.clone()).collect();
        for k in files {
            let Some(entry) = self.files.remove(&k) else { continue };
            self.dirty = true;
            let path = PathBuf::from(&k);
            let unchanged = !std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) && std::fs::read(&path).is_ok_and(|c| hash_hex(&c) == entry.hash);
            if unchanged {
                match std::fs::remove_file(&path) {
                    Ok(()) => changed.push(path.clone()),
                    Err(err) => tracing::warn!("theme take-back {}: {err}", path.display()),
                }
            }
            if let Some(backup) = &entry.backup
                && !exists(&path)
                && exists(backup)
            {
                match std::fs::rename(backup, &path) {
                    Ok(()) => changed.push(path.clone()),
                    Err(err) => tracing::warn!("theme restore {}: {err}", path.display()),
                }
            }
            remove_empty(&entry.created_dirs);
        }
        let blocks: Vec<String> = self.blocks.iter().filter(|(_, e)| e.app == app).map(|(k, _)| k.clone()).collect();
        for k in blocks {
            let Some(entry) = self.blocks.remove(&k) else { continue };
            self.dirty = true;
            let path = PathBuf::from(&k);
            let lines: Vec<&str> = entry.lines.iter().map(String::as_str).collect();
            match super::remove_block(&path, &lines) {
                Ok(true) => changed.push(path.clone()),
                Ok(false) => {}
                Err(err) => tracing::warn!("theme take-back {}: {err}", path.display()),
            }
            if entry.created && std::fs::read_to_string(&path).is_ok_and(|t| t.trim().is_empty()) && std::fs::remove_file(&path).is_ok() {
                remove_empty(&entry.created_dirs);
            }
        }
        changed
    }

    /// Records Kvantum activation before it happens (once): the link and a
    /// backup of `kvconfig`.
    pub fn record_kvantum(&mut self, link: &Path, target: &Path, kvconfig: &Path) -> std::io::Result<()> {
        if self.kvantum.is_some() {
            return Ok(());
        }
        let (kvconfig_backup, kvconfig_created) = if exists(kvconfig) {
            let backup = backup_path(kvconfig);
            std::fs::copy(kvconfig, &backup)?;
            (Some(backup), false)
        } else {
            (None, true)
        };
        self.kvantum = Some(KvantumEntry { link: link.to_path_buf(), target: target.to_path_buf(), kvconfig_backup, kvconfig_created });
        self.dirty = true;
        Ok(())
    }

    /// Takes back Kvantum activation: Mandara's link goes and, while Kvantum
    /// still selects `theme`, the user's `kvconfig` returns.
    pub fn take_back_kvantum(&mut self, kvconfig: &Path, theme: &str) -> Vec<PathBuf> {
        let Some(entry) = self.kvantum.take() else { return Vec::new() };
        self.dirty = true;
        let mut changed = Vec::new();
        if std::fs::read_link(&entry.link).is_ok_and(|t| t == entry.target) && std::fs::remove_file(&entry.link).is_ok() {
            changed.push(entry.link.clone());
        }
        let selected = std::fs::read_to_string(kvconfig).is_ok_and(|t| t.lines().any(|l| l.trim().replace(' ', "") == format!("theme={theme}")));
        if selected {
            let restored = match &entry.kvconfig_backup {
                Some(backup) if exists(backup) => std::fs::rename(backup, kvconfig).is_ok(),
                Some(_) => false,
                None => entry.kvconfig_created && std::fs::remove_file(kvconfig).is_ok(),
            };
            if restored {
                changed.push(kvconfig.to_path_buf());
            }
        }
        changed
    }
}
