//! Every app Mandara installed, with every file it wrote:
//! `~/.local/share/mandara/apps/registry.toml`. The uninstaller reads it
//! to remove exactly those files (to the Trash) and nothing else.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::detect::{Family, Format};

const REGISTRY_FILE: &str = "registry.toml";

/// Serializes registry writes within the process.
static WRITE_LOCK: Mutex<()> = Mutex::new(());
/// The last registry read for badges: (path, mtime, contents).
static CACHE: Mutex<Option<(PathBuf, Option<SystemTime>, Registry)>> = Mutex::new(None);

/// One installed app.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    pub family: Option<Family>,
    pub format: Option<Format>,
    /// The file it was installed from (name only) and its SHA-256.
    #[serde(default)]
    pub source_file: String,
    #[serde(default)]
    pub sha256: String,
    /// Seconds since the epoch.
    #[serde(default)]
    pub installed_at: u64,
    /// `mandara-obsidian` (no `.desktop`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop_file: Option<PathBuf>,
    /// The app's own folder (rootless installs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<PathBuf>,
    /// Every path Mandara wrote: the folder, the entry, icons, the shim.
    #[serde(default)]
    pub paths: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shim: Option<PathBuf>,
    /// Flatpak app id / native package name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flatpak: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flatpak_user: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default, rename = "app")]
    pub apps: Vec<Entry>,
}

/// `$XDG_DATA_HOME/mandara/apps`.
pub fn apps_root(data_home: &Path) -> PathBuf {
    data_home.join("mandara").join("apps")
}

pub fn path(data_home: &Path) -> PathBuf {
    apps_root(data_home).join(REGISTRY_FILE)
}

/// The registry (empty when missing or unreadable).
pub fn load(data_home: &Path) -> Registry {
    std::fs::read_to_string(path(data_home)).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
}

/// Writes atomically (temp file + rename).
fn save(data_home: &Path, reg: &Registry) -> Result<(), String> {
    let file = path(data_home);
    let dir = file.parent().ok_or("no registry folder")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let text = toml::to_string_pretty(reg).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(".{REGISTRY_FILE}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &file).map_err(|e| e.to_string())
}

/// Applies `f` to the registry under the write lock and saves it.
pub fn update(data_home: &Path, f: impl FnOnce(&mut Registry)) -> Result<(), String> {
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut reg = load(data_home);
    f(&mut reg);
    save(data_home, &reg)
}

/// Adds `entry`, replacing one with the same id.
pub fn record(data_home: &Path, entry: Entry) -> Result<(), String> {
    update(data_home, |reg| {
        reg.apps.retain(|e| e.id != entry.id);
        reg.apps.push(entry);
        reg.apps.sort_by_key(|a| a.name.to_lowercase());
    })
}

/// Reads through a cache keyed on the file's mtime (cheap enough to ask
/// for every app in a list).
pub fn cached(data_home: &Path) -> Registry {
    let file = path(data_home);
    let mtime = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((p, m, reg)) = cache.as_ref()
        && *p == file
        && *m == mtime
    {
        return reg.clone();
    }
    let reg = load(data_home);
    *cache = Some((file, mtime, reg.clone()));
    reg
}

/// The entry that installed desktop file `desktop` (by path or desktop id).
pub fn by_desktop(data_home: &Path, desktop: &Path) -> Option<Entry> {
    let stem = desktop.file_stem().map(|s| s.to_string_lossy().to_string());
    cached(data_home).apps.into_iter().find(|e| e.desktop_file.as_deref() == Some(desktop) || (e.desktop_id.is_some() && e.desktop_id == stem && e.desktop_file.as_ref().is_some_and(|f| f.file_name() == desktop.file_name())))
}

/// The entry of Flatpak app `id`.
pub fn by_flatpak(data_home: &Path, id: &str) -> Option<Entry> {
    cached(data_home).apps.into_iter().find(|e| e.flatpak.as_deref() == Some(id))
}

/// Drops the entry that owns `desktop` (after an uninstall), or the one of
/// Flatpak / package `name`.
pub fn forget(data_home: &Path, desktop: &Path, name: &str) -> Result<bool, String> {
    let by_name = || cached(data_home).apps.into_iter().find(|e| e.flatpak.as_deref() == Some(name) || e.package.as_deref() == Some(name));
    let owned = by_desktop(data_home, desktop).or_else(|| (!name.is_empty()).then(by_name).flatten());
    let Some(entry) = owned else { return Ok(false) };
    update(data_home, |reg| reg.apps.retain(|e| e.id != entry.id))?;
    Ok(true)
}

/// Apps installed by Mandara.
pub fn installed(data_home: &Path) -> Vec<Entry> {
    load(data_home).apps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_finds_and_forgets() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        let desktop = data.join("applications/mandara-hello.desktop");
        let entry = Entry { id: "hello".into(), name: "Hello".into(), family: Some(Family::AppImage), desktop_id: Some("mandara-hello".into()), desktop_file: Some(desktop.clone()), ..Default::default() };
        record(data, entry.clone()).unwrap();
        record(data, Entry { version: "2".into(), ..entry.clone() }).unwrap();
        assert_eq!(installed(data).len(), 1, "same id replaces");
        assert_eq!(by_desktop(data, &desktop).unwrap().version, "2");
        assert!(forget(data, &desktop, "").unwrap());
        assert!(installed(data).is_empty());
        assert!(!forget(data, &desktop, "").unwrap());
    }
}
