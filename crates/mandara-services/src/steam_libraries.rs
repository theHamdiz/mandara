//! Steam's registered Workshop folders, found only when the person asks.

use std::io::Read as _;
use std::path::{Path, PathBuf};

/// Steam normally has a handful of libraries; malformed files cannot grow the scan indefinitely.
const MAX_LIBRARIES: usize = 64;
/// Library registration files are small, even on machines with many games.
const MAX_LIBRARY_FILE: u64 = 256 * 1024;
/// An approval picker presents a bounded set of candidate folders.
pub const MAX_FOLDERS: usize = 64;
/// Bound directory work independently of how many valid app-id folders it contains.
const MAX_CONTENT_ENTRIES: usize = 256;

/// A source of folder suggestions, not a grant or permission change.
pub trait FolderDiscovery {
    fn folders(&self) -> Vec<PathBuf>;
}

pub struct SteamWorkshop<'a> { pub home: &'a Path }

/// Native, compatibility-link and Flatpak installs may coexist.
fn roots(home: &Path) -> [PathBuf; 3] {
    [home.join(".local/share/Steam"), home.join(".steam/steam"), home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam")]
}

pub fn libraries(home: &Path) -> Vec<PathBuf> {
    let mut libraries = Vec::new();
    for root in roots(home) {
        let Ok(root) = root.canonicalize() else { continue };
        if !root.join("steamapps").is_dir() { continue; }
        if !libraries.contains(&root) { libraries.push(root.clone()); }
        for registration in ["steamapps/libraryfolders.vdf", "config/libraryfolders.vdf"] {
            let Ok(file) = std::fs::File::open(root.join(registration)) else { continue };
            let mut text = String::new();
            if file.take(MAX_LIBRARY_FILE + 1).read_to_string(&mut text).is_err() || text.len() as u64 > MAX_LIBRARY_FILE { continue; }
            for (key, value) in crate::desktop_feeds::kv_pairs(&text) {
                if libraries.len() >= MAX_LIBRARIES { break; }
                if !key.eq_ignore_ascii_case("path") && !( !key.is_empty() && key.bytes().all(|c| c.is_ascii_digit())) { continue; }
                let path = Path::new(&value);
                if !path.is_absolute() { continue; }
                let Ok(path) = path.canonicalize() else { continue };
                if path.join("steamapps").is_dir() && !libraries.contains(&path) { libraries.push(path); }
            }
        }
        if libraries.len() >= MAX_LIBRARIES { break; }
    }
    libraries
}

impl FolderDiscovery for SteamWorkshop<'_> {
    fn folders(&self) -> Vec<PathBuf> {
        let mut folders = Vec::new();
        for library in libraries(self.home) {
            let Ok(content) = library.join("steamapps/workshop/content").canonicalize() else { continue };
            let Ok(entries) = std::fs::read_dir(&content) else { continue };
            for entry in entries.flatten().take(MAX_CONTENT_ENTRIES) {
                if folders.len() >= MAX_FOLDERS { break; }
                if !entry.file_name().to_str().is_some_and(|name| name.parse::<u32>().is_ok_and(|id| id > 0)) { continue; }
                let Ok(path) = entry.path().canonicalize() else { continue };
                if path.starts_with(&content) && path.is_dir() && !folders.contains(&path) { folders.push(path); }
            }
        }
        folders.sort_unstable();
        folders
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workshop(root: &Path, id: &str) -> PathBuf {
        let path = root.join("steamapps/workshop/content").join(id);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn finds_registered_libraries_on_other_drives_without_granting_them() {
        let home = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let root = home.path().join(".local/share/Steam");
        let local = workshop(&root, "431960");
        let remote = workshop(other.path(), "431960");
        std::fs::create_dir(root.join("config")).unwrap();
        let registration = format!("\"libraryfolders\"\n{{\n\"0\"\n{{\n\"path\" \"{}\"\n}}\n}}\n", other.path().display());
        std::fs::write(root.join("config/libraryfolders.vdf"), registration).unwrap();
        let folders = SteamWorkshop { home: home.path() }.folders();
        assert_eq!(folders.len(), 2);
        assert!(folders.contains(&local) && folders.contains(&remote));
        assert!(!home.path().join(".config/mandara").exists());
    }

    #[test]
    fn combines_flatpak_and_native_installs_and_deduplicates_compatibility_links() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".local/share/Steam");
        let local = workshop(&root, "431960");
        let flatpak = workshop(&home.path().join(".var/app/com.valvesoftware.Steam/.local/share/Steam"), "123");
        std::fs::create_dir_all(home.path().join(".steam")).unwrap();
        std::os::unix::fs::symlink(&root, home.path().join(".steam/steam")).unwrap();
        let folders = SteamWorkshop { home: home.path() }.folders();
        assert_eq!(folders.len(), 2);
        assert!(folders.contains(&local) && folders.contains(&flatpak));
    }

    #[test]
    fn old_registration_and_escaped_paths_are_read_but_oversized_files_are_ignored() {
        let home = tempfile::tempdir().unwrap();
        let drive = tempfile::tempdir().unwrap();
        let root = home.path().join(".local/share/Steam");
        workshop(&root, "431960");
        let special = drive.path().join("with \\\"quotes");
        let expected = workshop(&special, "431960");
        let quoted = serde_json::to_string(special.to_str().unwrap()).unwrap();
        std::fs::write(root.join("steamapps/libraryfolders.vdf"), format!("\"1\" {quoted}\n")).unwrap();
        assert!(SteamWorkshop { home: home.path() }.folders().contains(&expected));
        std::fs::write(root.join("steamapps/libraryfolders.vdf"), "x".repeat(MAX_LIBRARY_FILE as usize + 1)).unwrap();
        assert_eq!(SteamWorkshop { home: home.path() }.folders().len(), 1);
    }

    #[test]
    fn suggestions_skip_links_outside_workshop_content_and_invalid_app_ids() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".local/share/Steam");
        let valid = workshop(&root, "431960");
        workshop(&root, "not-an-app-id");
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("steamapps/workshop/content/999")).unwrap();
        assert_eq!(SteamWorkshop { home: home.path() }.folders(), [valid]);
    }
}
