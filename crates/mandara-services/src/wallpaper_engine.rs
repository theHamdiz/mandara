//! Discovery of locally installed Wallpaper Engine items and shared assets.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::desktop_feeds::{steam_libraries, steam_roots};

const WORKSHOP: &str = "steamapps/workshop/content/431960";
const ASSETS: &str = "steamapps/common/wallpaper_engine/assets";
/// Avoid loading unexpectedly large previews from Workshop content.
const MAX_PREVIEW_BYTES: u64 = 8 * 1024 * 1024;
/// Workshop metadata is small. Bound reads before parsing untrusted files.
const MAX_PROJECT_BYTES: u64 = 1024 * 1024;

#[derive(Clone)]
pub struct Project {
    pub id: u64,
    pub title: String,
    pub directory: PathBuf,
    pub preview: Option<PathBuf>,
}

#[derive(Default)]
pub struct Library {
    pub projects: BTreeMap<u64, Project>,
    pub assets: Option<PathBuf>,
}

#[derive(Deserialize)]
struct Metadata {
    title: String,
    preview: Option<PathBuf>,
    #[serde(rename = "type")]
    kind: String,
}

pub fn discover(folder: &str) -> Result<Library, PathBuf> {
    let automatic = steam_roots().flat_map(|root| steam_libraries(&root));
    if folder.trim().is_empty() {
        return Ok(scan(automatic, None));
    }
    let (root, assets) = manual_location(mandara_config::paths::expand_tilde(folder.trim()))?;
    Ok(scan(root.into_iter().chain(automatic), assets))
}

fn manual_location(path: PathBuf) -> Result<(Option<PathBuf>, Option<PathBuf>), PathBuf> {
    if !path.is_absolute() {
        return Err(path);
    }
    // Inspect the supplied layout before resolving symlinks: an install may
    // be linked out of steamapps/common while its Workshop items stay in Steam.
    if path.join("steamapps").is_dir() {
        return Ok((Some(path), None));
    }
    if path.ends_with("steamapps") && path.is_dir() {
        return Ok((path.parent().map(Path::to_path_buf), None));
    }
    let assets = path.join("assets");
    if assets.is_dir() {
        let root = path.parent().and_then(Path::parent).filter(|p| p.ends_with("steamapps"))
            .and_then(Path::parent).map(Path::to_path_buf);
        return Ok((root, Some(assets)));
    }
    Err(path)
}

fn scan(libraries: impl IntoIterator<Item = PathBuf>, assets: Option<PathBuf>) -> Library {
    let mut result = Library { assets, ..Library::default() };
    let mut seen = HashSet::new();
    for root in libraries {
        if !root.is_absolute() {
            continue;
        }
        let Ok(real_root) = root.canonicalize() else { continue };
        if !seen.insert(real_root) {
            continue;
        }
        if result.assets.is_none() {
            result.assets = Some(root.join(ASSETS)).filter(|p| p.is_dir());
        }
        let Ok(entries) = fs::read_dir(root.join(WORKSHOP)) else {
            continue;
        };
        for entry in entries.flatten() {
            let Some(id) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<u64>().ok())
            else {
                continue;
            };
            if let Some(item) = project(id, &entry.path()) {
                result.projects.entry(id).or_insert(item);
            }
        }
    }
    result
}

fn project(id: u64, path: &Path) -> Option<Project> {
    use std::io::Read as _;
    let directory = path.canonicalize().ok()?;
    let manifest = directory.join("project.json");
    if !manifest.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(manifest)
        .ok()?
        .take(MAX_PROJECT_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_PROJECT_BYTES {
        return None;
    }
    let data: Metadata = serde_json::from_slice(&bytes).ok()?;
    // Web projects can run scripts and are outside the scene/video picker.
    if !data.kind.eq_ignore_ascii_case("scene") && !data.kind.eq_ignore_ascii_case("video") {
        return None;
    }
    let preview = data
        .preview
        .and_then(|name| directory.join(name).canonicalize().ok())
        .filter(|p| p.starts_with(&directory))
        .filter(|p| {
            p.extension().is_some_and(|ext| {
                ["jpg", "jpeg", "png", "webp", "gif"]
                    .iter()
                    .any(|allowed| ext.eq_ignore_ascii_case(allowed))
            })
        })
        .filter(|p| fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() <= MAX_PREVIEW_BYTES));
    Some(Project {
        id,
        title: data.title,
        directory,
        preview,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_library_takes_priority_and_accepts_the_install_folder() {
        let default = tempfile::tempdir().unwrap();
        let disk = tempfile::tempdir().unwrap();
        for (root, title) in [(default.path(), "Default"), (disk.path(), "Other disk")] {
            let item = root.join(WORKSHOP).join("123");
            fs::create_dir_all(&item).unwrap();
            fs::write(item.join("project.json"), format!(r#"{{"type":"Scene","title":"{title}"}}"#)).unwrap();
            fs::create_dir_all(root.join(ASSETS)).unwrap();
        }
        let install = disk.path().join(ASSETS).parent().unwrap().to_path_buf();
        for folder in [disk.path().to_path_buf(), disk.path().join("steamapps"), install] {
            let (root, assets) = manual_location(folder).unwrap();
            let library = scan(root.into_iter().chain([default.path().to_path_buf()]), assets);
            assert_eq!(library.projects[&123].title, "Other disk");
            assert_eq!(library.assets, Some(disk.path().join(ASSETS)));
        }
        assert_eq!(manual_location(disk.path().join("missing")), Err(disk.path().join("missing")));
        let standalone = tempfile::tempdir().unwrap();
        let assets = standalone.path().join("assets");
        fs::create_dir(&assets).unwrap();
        assert_eq!(manual_location(standalone.path().to_path_buf()), Ok((None, Some(assets))));
    }

    #[test]
    fn finds_scenes_in_secondary_library_and_keeps_previews_inside_the_project() {
        let main = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let item = other.path().join(WORKSHOP).join("123");
        fs::create_dir_all(&item).unwrap();
        fs::create_dir_all(main.path().join(ASSETS)).unwrap();
        fs::write(item.parent().unwrap().join("outside.jpg"), b"preview").unwrap();
        fs::write(
            item.join("project.json"),
            r#"{"type":"Scene","title":"Test","preview":"../outside.jpg"}"#,
        )
        .unwrap();
        let library = scan([main.path().into(), other.path().into()], None);
        assert!(library.assets.is_some());
        assert_eq!(library.projects[&123].title, "Test");
        assert!(library.projects[&123].preview.is_none());

        fs::write(item.join("preview.GIF"), b"GIF89a").unwrap();
        fs::write(
            item.join("project.json"),
            r#"{"type":"Scene","title":"Test","preview":"preview.GIF"}"#,
        )
        .unwrap();
        assert_eq!(project(123, &item).unwrap().preview, Some(item.join("preview.GIF").canonicalize().unwrap()));
    }
}
