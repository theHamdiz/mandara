//! Resolve Flatpak filesystem aliases once on the inspection worker.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, ensure};

use super::{Capability, Constraint, Key};

/// Linux bounds a pathname independently of the native metadata output limit.
const MAX_PATH_BYTES: usize = 4096;
/// Flatpak's host-os grant exposes these system trees, sometimes through symlinks.
const SYSTEM_TREES: [&str; 5] = ["/usr", "/bin", "/sbin", "/lib", "/lib64"];
/// GLib's user-directory names, in a fixed order shared by parsing and lookup.
const USER_DIRS: [(&str, &str); 8] = [
    ("xdg-desktop", "XDG_DESKTOP_DIR"),
    ("xdg-documents", "XDG_DOCUMENTS_DIR"),
    ("xdg-download", "XDG_DOWNLOAD_DIR"),
    ("xdg-music", "XDG_MUSIC_DIR"),
    ("xdg-pictures", "XDG_PICTURES_DIR"),
    ("xdg-videos", "XDG_VIDEOS_DIR"),
    ("xdg-public-share", "XDG_PUBLICSHARE_DIR"),
    ("xdg-templates", "XDG_TEMPLATES_DIR"),
];

fn invalid() -> &'static str {
    mandara_i18n::t("The Flatpak permission data is invalid or too large.")
}

pub(super) struct Locations {
    home: PathBuf,
    user: [Option<PathBuf>; USER_DIRS.len()],
    config: PathBuf,
    data: PathBuf,
    cache: PathBuf,
    runtime: PathBuf,
}

impl Locations {
    pub fn load() -> Result<Self> {
        let home = dirs::home_dir().context(invalid())?;
        let config = mandara_config::paths::config_dir()
            .parent()
            .context(invalid())?
            .to_path_buf();
        let text = super::transaction::read(&config.join("user-dirs.dirs"))?;
        let mut locations = Self::in_home(&home, &text)?;
        locations.config = config;
        locations.data = mandara_config::paths::data_dir()
            .parent()
            .context(invalid())?
            .to_path_buf();
        locations.cache = mandara_config::paths::cache_dir()
            .parent()
            .context(invalid())?
            .to_path_buf();
        locations.runtime = dirs::runtime_dir().unwrap_or_else(|| locations.cache.clone());
        Ok(locations)
    }

    fn in_home(home: &Path, text: &str) -> Result<Self> {
        ensure!(text.len() as u64 <= super::native::MAX_BYTES, invalid());
        let mut user = std::array::from_fn(|_| None);
        user[0] = Some(home.join("Desktop"));
        // Match GLib: literal absolute paths or a leading $HOME; later entries
        // replace earlier ones. These files are data and are never sourced.
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let Some(index) = USER_DIRS
                .iter()
                .position(|(_, name)| *name == key.trim_matches([' ', '\t']))
            else {
                continue;
            };
            let Some(value) = value.trim_start_matches([' ', '\t']).strip_prefix('"') else {
                continue;
            };
            let Some(end) = value.rfind('"') else {
                continue;
            };
            let value = &value[..end];
            let path = if value == "$HOME" {
                home.to_path_buf()
            } else if let Some(rest) = value.strip_prefix("$HOME/") {
                join(home, rest)?
            } else if value.starts_with('/') {
                lexical(Path::new(value))?
            } else {
                continue;
            };
            user[index] = Some(path);
        }
        let home = lexical(home)?;
        for path in user.iter_mut().flatten() {
            *path = lexical(path)?;
        }
        Ok(Self {
            config: home.join(".config"),
            data: home.join(".local/share"),
            cache: home.join(".cache"),
            runtime: home.join(".cache"),
            home,
            user,
        })
    }

    fn resolve(&self, grant: &str) -> Result<Option<PathBuf>> {
        let location = location(grant)?;
        let (prefix, rest) = location.split_once('/').unwrap_or((&location, ""));
        let root = match prefix {
            "home" | "~" => Some(&self.home),
            "xdg-config" => Some(&self.config),
            "xdg-data" => Some(&self.data),
            "xdg-cache" => Some(&self.cache),
            "xdg-run" if !rest.is_empty() => Some(&self.runtime),
            _ => USER_DIRS
                .iter()
                .position(|(name, _)| *name == prefix)
                .and_then(|index| self.user[index].as_ref())
                .filter(|path| *path != &self.home),
        };
        let path = if location == "host-etc" {
            PathBuf::from("/etc")
        } else if let Some(root) = root {
            join(root, rest.trim_start_matches('/'))?
        } else if location.starts_with('/') {
            lexical(Path::new(&location))?
        } else {
            return Ok(None);
        };
        Ok(Some(path))
    }

    pub fn constrain(&self, rows: &mut [Capability], grants: &[String]) -> Result<()> {
        // At most the fixed set of editable filesystem locations is retained.
        // Each grant is resolved once, rather than once per settings row.
        let mut controls = Vec::with_capacity(USER_DIRS.len() + 7);
        for (index, row) in rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.key == Key::Filesystems && row.editable)
        {
            if let Some(path) = self.resolve(&row.value)? {
                controls.push((index, canonical(&path)?));
            }
            if row.value == "host-os" {
                for path in SYSTEM_TREES {
                    controls.push((index, canonical(Path::new(path))?));
                }
            }
        }
        for grant in grants {
            if location(grant)? == "host-os" {
                for root in SYSTEM_TREES {
                    let path = canonical(Path::new(root))?;
                    for (index, control) in &controls {
                        if rows[*index].value != "host-os" && control.starts_with(&path) {
                            rows[*index].editable = false;
                            rows[*index].constraint = Some(Constraint::Broader);
                        }
                    }
                }
                continue;
            }
            let Some(path) = self.resolve(grant)? else {
                continue;
            };
            let path = canonical(&path)?;
            let name = location(grant)?;
            for (index, control) in &controls {
                if name != rows[*index].value && control.starts_with(&path) {
                    rows[*index].editable = false;
                    rows[*index].constraint = Some(Constraint::Broader);
                }
            }
        }
        Ok(())
    }
}

fn canonical(path: &Path) -> Result<PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(path.to_path_buf())
        }
        Err(error) => Err(error.into()),
    }
}

fn join(root: &Path, rest: &str) -> Result<PathBuf> {
    ensure!(
        root.as_os_str()
            .len()
            .checked_add(rest.len() + 1)
            .is_some_and(|size| size <= MAX_PATH_BYTES),
        invalid()
    );
    lexical(&root.join(rest))
}

fn lexical(path: &Path) -> Result<PathBuf> {
    ensure!(
        path.is_absolute() && path.as_os_str().len() <= MAX_PATH_BYTES,
        invalid()
    );
    let mut result = PathBuf::new();
    for part in path.components() {
        ensure!(!matches!(part, Component::ParentDir), invalid());
        result.push(part);
    }
    Ok(result)
}

/// Native flags escape colons and backslashes; metadata adds GKeyFile escaping.
fn location(grant: &str) -> Result<String> {
    ensure!(grant.len() <= MAX_PATH_BYTES, invalid());
    let mut result = String::with_capacity(grant.len());
    let mut chars = grant.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            ':' => {
                ensure!(
                    matches!(chars.collect::<String>().as_str(), "ro" | "rw" | "create"),
                    invalid()
                );
                break;
            }
            '\\' => {
                let next = chars.next().context(invalid())?;
                // The native permission list escapes a flag escape once more.
                let next = if next == '\\' && chars.peek() == Some(&':') {
                    chars.next().context(invalid())?
                } else {
                    next
                };
                result.push(next);
            }
            _ => result.push(ch),
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_install::flatpak::parse_context;

    #[test]
    fn equivalent_and_parent_paths_lock_only_the_affected_controls() {
        let locations = Locations::in_home(Path::new("/home/alice"), "XDG_DOCUMENTS_DIR=\"$HOME/Writing\"\nXDG_MUSIC_DIR=\"/media/Audio\"\nXDG_PICTURES_DIR=\"$HOME\"\n").unwrap();
        for (grant, locked) in [
            ("/home/alice", "home"),
            ("~/Writing:ro", "xdg-documents"),
            ("/home/alice/Writing/.", "xdg-documents"),
            ("/media:rw", "xdg-music"),
            ("home", "xdg-documents"),
        ] {
            let permissions = parse_context(&format!(
                "[Context]\nfilesystems=home;xdg-documents;xdg-music;{grant};\n"
            ))
            .unwrap();
            let mut rows = super::super::model::capabilities(&permissions, &permissions);
            locations
                .constrain(&mut rows, &permissions.filesystems)
                .unwrap();
            let row = rows.iter().find(|row| row.value == locked).unwrap();
            assert!(!row.editable, "{grant}: {locked}");
            assert_eq!(row.constraint, Some(Constraint::Broader));
            assert!(
                rows.iter()
                    .find(|row| row.value == "network")
                    .unwrap()
                    .editable
            );
            if !grant.starts_with("/media") {
                assert!(
                    rows.iter()
                        .find(|row| row.value == "xdg-music")
                        .unwrap()
                        .editable
                );
            }
        }
        let permissions =
            parse_context("[Context]\nfilesystems=home;xdg-pictures;/unrelated;\n").unwrap();
        let mut rows = super::super::model::capabilities(&permissions, &permissions);
        locations
            .constrain(&mut rows, &permissions.filesystems)
            .unwrap();
        assert!(
            rows.iter()
                .find(|row| row.value == "home")
                .unwrap()
                .editable,
            "disabled XDG aliases never grant the home folder"
        );
        assert_eq!(locations.resolve("xdg-pictures").unwrap(), None);
        assert_eq!(
            locations.resolve("~/Music\\\\:live:ro").unwrap().unwrap(),
            Path::new("/home/alice/Music:live")
        );
        assert!(locations.resolve(&"x".repeat(MAX_PATH_BYTES + 1)).is_err());
        assert!(locations.resolve("~/../private").is_err());
    }

    #[test]
    fn user_directory_revisions_are_read_again_without_a_process_cache() {
        let home = Path::new("/home/alice");
        let before = Locations::in_home(home, "XDG_DOCUMENTS_DIR=\"$HOME/Old\"\n").unwrap();
        let after = Locations::in_home(
            home,
            "XDG_DOCUMENTS_DIR=\"$HOME/Old\"\nXDG_DOCUMENTS_DIR=\"$HOME/New\"\n",
        )
        .unwrap();
        assert_eq!(
            before.resolve("xdg-documents").unwrap().unwrap(),
            home.join("Old")
        );
        assert_eq!(
            after.resolve("xdg-documents").unwrap().unwrap(),
            home.join("New")
        );
        assert_eq!(
            after.resolve("xdg-desktop").unwrap().unwrap(),
            home.join("Desktop")
        );
        assert_eq!(after.resolve("xdg-download").unwrap(), None);
        let locations =
            Locations::in_home(home, "XDG_DOCUMENTS_DIR=\"/usr/share/Documents\"\n").unwrap();
        let permissions = parse_context("[Context]\nfilesystems=host-os;xdg-documents;\n").unwrap();
        let mut rows = super::super::model::capabilities(&permissions, &permissions);
        locations
            .constrain(&mut rows, &permissions.filesystems)
            .unwrap();
        assert_eq!(
            rows.iter()
                .find(|row| row.value == "xdg-documents")
                .unwrap()
                .constraint,
            Some(Constraint::Broader)
        );
    }

    #[test]
    fn existing_symlinks_are_resolved_once_before_comparing_grants() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir(&home).unwrap();
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&home, &alias).unwrap();
        let locations = Locations::in_home(&home, "").unwrap();
        let permissions = parse_context(&format!(
            "[Context]\nfilesystems=home;{};\n",
            alias.display()
        ))
        .unwrap();
        let mut rows = super::super::model::capabilities(&permissions, &permissions);
        locations
            .constrain(&mut rows, &permissions.filesystems)
            .unwrap();
        assert_eq!(
            rows.iter()
                .find(|row| row.value == "home")
                .unwrap()
                .constraint,
            Some(Constraint::Broader)
        );
    }
}
