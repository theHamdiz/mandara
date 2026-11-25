//! Sound themes (the opt-in `sound-themes` plugin): the curated download
//! catalog ([`catalog`]), installing your own or catalog themes and
//! removing them ([`install`]), setting the theme for apps ([`desktop`]),
//! and the installed themes with what each one sounds for ([`installed`],
//! [`coverage`]). Playing them is [`crate::sounds`].
//!
//! Nothing here runs anything from a theme archive: catalog archives are
//! verified against a pinned SHA-256, everything is unpacked with pure-Rust
//! readers that refuse unsafe paths, and only sound files, `index.theme`
//! and licence files are kept.

pub mod catalog;
pub mod desktop;
pub mod install;

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::sounds::{self, SoundEvent};

/// An installed theme, wherever it lives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ThemeInfo {
    /// The folder's name (what apps and `[sounds] theme` use).
    pub name: String,
    /// `Name=` from its `index.theme`, else the folder's name.
    pub title: String,
    pub comment: String,
    pub dir: PathBuf,
    /// In your own data folder (else a system folder).
    pub user: bool,
}

/// `Name=` and `Comment=` of an `index.theme` (the `[Sound Theme]` group,
/// unlocalized).
fn index_names(dir: &Path) -> (Option<String>, Option<String>) {
    let Ok(text) = std::fs::read_to_string(dir.join("index.theme")) else { return (None, None) };
    let get = |key: &str| desktop::ini_get(&text, "Sound Theme", key).filter(|v| !v.is_empty());
    (get("Name"), get("Comment"))
}

/// Every installed theme in `roots` (the first folder of a name wins, as in
/// lookups), by name.
pub fn installed_in(roots: &[PathBuf], user_root: Option<&Path>) -> Vec<ThemeInfo> {
    let mut out: Vec<ThemeInfo> = Vec::new();
    for root in roots {
        for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let dir = e.path();
            if name.starts_with('.') || out.iter().any(|t| t.name == name) || !(dir.join("index.theme").is_file() || dir.join("stereo").is_dir()) {
                continue;
            }
            let (title, comment) = index_names(&dir);
            out.push(ThemeInfo { title: title.unwrap_or_else(|| name.clone()), comment: comment.unwrap_or_default(), user: user_root.is_some_and(|u| root == u), name, dir });
        }
    }
    out.sort_by_key(|t| t.name.to_lowercase());
    out
}

/// Every installed theme (your data folder and the system's).
pub fn installed() -> Vec<ThemeInfo> {
    let user = dirs::data_dir().map(|d| d.join("sounds"));
    installed_in(&sounds::sound_roots(), user.as_deref())
}

/// The file each of `events` plays in `theme` (with its inherited themes
/// and the freedesktop fallback), `None` where it is silent.
pub fn coverage(roots: &[PathBuf], theme: &str, events: &[SoundEvent]) -> Vec<Option<PathBuf>> {
    events.iter().map(|e| sounds::lookup_in(roots, theme, &e.names())).collect()
}

/// Sets `theme` for apps (`None`: back to what they had before), recording
/// the earlier values in the install manifest.
pub fn apply_desktop(theme: Option<&str>, env: &install::Env) -> Result<desktop::Applied, String> {
    let mut manifest = install::Manifest::load(&env.manifest);
    let before = manifest.desktop.clone();
    let result = desktop::apply(theme, &desktop::DesktopEnv::current(), &desktop::Gsettings, &mut manifest.desktop);
    if manifest.desktop != before {
        manifest.save(&env.manifest)?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme(root: &Path, name: &str, index: Option<&str>, files: &[&str]) {
        let dir = root.join(name).join("stereo");
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(index) = index {
            std::fs::write(root.join(name).join("index.theme"), index).unwrap();
        }
        for f in files {
            std::fs::write(dir.join(f), b"OggS").unwrap();
        }
    }

    #[test]
    fn lists_user_and_system_themes_once() {
        let tmp = tempfile::tempdir().unwrap();
        let user = tmp.path().join("home/sounds");
        let usr_local = tmp.path().join("usr/local/share/sounds");
        let usr = tmp.path().join("usr/share/sounds");
        theme(&user, "ocean", Some("[Sound Theme]\nName=Ocean\nComment=Plasma\n"), &["bell.oga"]);
        theme(&usr_local, "ocean", Some("[Sound Theme]\nName=Old Ocean\n"), &[]);
        theme(&usr_local, "freedesktop", None, &["bell.oga"]);
        theme(&usr, "deepin", Some("[Sound Theme]\nName=Deepin\n"), &[]);
        std::fs::create_dir_all(usr.join("not-a-theme")).unwrap();
        let roots = vec![user.clone(), usr_local, usr];
        let list = installed_in(&roots, Some(&user));
        let names: Vec<_> = list.iter().map(|t| (t.name.as_str(), t.title.as_str(), t.user)).collect();
        assert_eq!(names, vec![("deepin", "Deepin", false), ("freedesktop", "freedesktop", false), ("ocean", "Ocean", true)]);
        assert_eq!(list[2].comment, "Plasma");
    }

    #[test]
    fn coverage_follows_inheritance_and_the_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        theme(&root, "freedesktop", None, &["device-added.oga", "message.oga"]);
        theme(&root, "mine", Some("[Sound Theme]\nInherits=freedesktop\n"), &["dialog-warning.wav"]);
        let roots = vec![root.clone()];
        let got = coverage(&roots, "mine", &[SoundEvent::NotificationCritical, SoundEvent::Notification, SoundEvent::PowerPlug, SoundEvent::TrashEmpty]);
        let rel: Vec<Option<String>> = got.iter().map(|p| p.as_ref().map(|p| p.strip_prefix(&root).unwrap().to_string_lossy().into_owned())).collect();
        assert_eq!(rel, vec![Some("mine/stereo/dialog-warning.wav".into()), Some("freedesktop/stereo/message.oga".into()), Some("freedesktop/stereo/device-added.oga".into()), None]);
    }
}
