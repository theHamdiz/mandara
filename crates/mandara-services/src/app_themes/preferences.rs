//! Restore only the app preferences a template still owns.

use super::paths::Locations;
mod document;
pub use document::Format;
use document::Document as _;
#[cfg(test)]
use crate::icons::apply::ini_get;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Change {
    #[serde(default)]
    format: Format,
    path: PathBuf,
    section: String,
    key: String,
    before: Option<String>,
    after: String,
    created: bool,
}

#[derive(Serialize, Deserialize)]
struct Pending {
    owner: String,
    path: PathBuf,
    before: Vec<Change>,
    format: Format,
}

#[derive(Default, Serialize, Deserialize)]
struct Record {
    owners: BTreeMap<String, Vec<Change>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending: Option<Pending>,
}

impl Record {
    fn recover(&mut self) -> Result<bool> {
        let Some(pending) = self.pending.as_ref() else { return Ok(false) };
        let current = pending.format.parse(&text(&pending.path)?)?;
        let changes = self.owners.entry(pending.owner.clone()).or_default();
        // Atomic writers leave either version. Compare individual values so a
        // later unrelated user edit does not make recovery forget ownership.
        for before in &pending.before {
            if before.path != pending.path { continue; }
            if current.get(&before.section, &before.key)?.as_deref() == Some(&before.after) {
                if let Some(change) = changes.iter_mut().find(|change| change.path == before.path && change.section == before.section && change.key == before.key) {
                    *change = before.clone();
                } else {
                    changes.push(before.clone());
                }
            }
        }
        self.pending = None;
        Ok(true)
    }
}

fn record_path(locations: &Locations) -> PathBuf {
    locations
        .config
        .join("mandara/generated/app-theme-preferences.json")
}
fn load(path: &Path) -> Result<Record> {
    match super::apply::read_text(path) {
        Ok(text) => {
            let mut record: Record = serde_json::from_str(&text)?;
            if record.recover()? { store(path, &record)?; }
            Ok(record)
        },
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(Record::default())
        }
        Err(error) => Err(error),
    }
}
fn text(path: &Path) -> Result<String> {
    match super::apply::read_text(path) {
        Ok(text) => Ok(text),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(String::new())
        }
        Err(error) => Err(error),
    }
}
fn store(path: &Path, record: &Record) -> Result<()> {
    let bytes = serde_json::to_vec(record)?;
    ensure!(
        bytes.len() <= mandara_theme::templates::custom::MAX_TEMPLATE_BYTES,
        "app theme preference record is too large"
    );
    mandara_config::store::write_atomic(path, &bytes)
}

pub type Sections = BTreeMap<String, BTreeMap<String, String>>;

pub fn ini_keys(body: &str) -> Result<Sections> {
    let mut section = String::new();
    let mut keys = Sections::new();
    for line in body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with(['#', ';']))
    {
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            section = name.into();
        } else {
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("invalid INI setting: {line}"))?;
            ensure!(
                !section.is_empty() && !key.trim().is_empty(),
                "INI setting has no group or key"
            );
            keys.entry(section.clone())
                .or_default()
                .insert(key.trim().into(), value.trim().into());
        }
    }
    ensure!(!keys.is_empty(), "color scheme has no settings");
    Ok(keys)
}

fn settings(keys: &Sections) -> impl Iterator<Item = (&String, &String, &String)> {
    keys.iter()
        .flat_map(|(section, group)| group.iter().map(move |(key, value)| (section, key, value)))
}

/// The write-ahead record survives a failed application or a shell restart.
pub fn set(owner: &str, path: &Path, keys: &Sections, locations: &Locations) -> Result<()> {
    set_with(owner,path,keys,Format::Ini,locations,&|path,_,next|mandara_config::store::write_atomic(path,next.as_bytes()))
}

pub fn set_with(
    owner: &str,
    path: &Path,
    keys: &Sections,
    format: Format,
    locations: &Locations,
    write: &dyn Fn(&Path,&str,&str) -> Result<()>,
) -> Result<()> {
    let record_path = record_path(locations);
    let mut record = load(&record_path)?;
    let previous = record.owners.get(owner).cloned().unwrap_or_default();
    let created = !path.exists();
    let original = text(path)?;
    let mut next = format.parse(&original)?;
    let known_file = record
        .owners
        .values()
        .flatten()
        .any(|change| change.path == path);
    for (section, key, _) in settings(keys) {
        ensure!(
            !record.owners.iter().any(|(other, changes)| other != owner
                && changes.iter().any(|change| change.path == path
                    && change.section == *section
                    && change.key == *key)),
            "another template already manages {section}.{key}"
        );
    }
    let changes = record.owners.entry(owner.into()).or_default();
    let mut metadata_changed = false;
    for index in (0..changes.len()).rev() {
        let change = &changes[index];
        if change.path != path || keys.get(&change.section).is_some_and(|group|group.contains_key(&change.key)) {
            continue;
        }
        ensure!(change.format == format,"theme preference format changed");
        if next.get(&change.section,&change.key)?.as_deref() == Some(change.after.as_str()) {
            next.set(&change.section,&change.key,change.before.as_deref())?;
        }
        changes.remove(index);
        metadata_changed = true;
    }
    for (section, key, value) in settings(keys) {
        ensure!(
            !section.contains(['\n', '\r', ']'])
                && !key.contains(['\n', '\r', '='])
                && !value.contains(['\n', '\r']),
            "invalid INI preference"
        );
        let current = next.get(section, key)?;
        if let Some(change) = changes
            .iter_mut()
            .find(|change| change.path == path && change.section == *section && change.key == *key)
        {
            ensure!(change.format == format,"theme preference format changed");
            if current.as_deref() != Some(change.after.as_str()) {
                change.before = current;
                metadata_changed = true;
            }
            metadata_changed |= change.after != *value;
            change.after.clone_from(value);
        } else {
            metadata_changed = true;
            changes.push(Change {
                format,
                path: path.into(),
                section: section.clone(),
                key: key.clone(),
                before: current,
                after: value.clone(),
                created,
            });
        }
        next.set(section,key,Some(value))?;
    }
    let next = next.finish();
    if next == original {
        if metadata_changed {
            store(&record_path, &record)?;
        }
        return Ok(());
    }
    if !created && !known_file {
        std::fs::copy(path, mandara_theme::templates::backup_path(path))?;
    }
    record.pending = Some(Pending { owner: owner.into(), path: path.into(), before: previous, format });
    store(&record_path, &record)?;
    if let Err(error) = write(path,&original,&next) {
        // Keep the journal if recovery itself fails; another sync can retry.
        load(&record_path).map_err(|recovery| anyhow::anyhow!("{error:#}; recovering theme ownership: {recovery:#}"))?;
        return Err(error);
    }
    record.pending = None;
    store(&record_path, &record)?;
    Ok(())
}

pub fn undo(owner: &str, locations: &Locations) -> Result<Vec<PathBuf>> {
    undo_with(owner,locations,&|path,_,next|mandara_config::store::write_atomic(path,next.as_bytes()))
}

pub fn undo_with(
    owner: &str,
    locations: &Locations,
    write: &dyn Fn(&Path,&str,&str) -> Result<()>,
) -> Result<Vec<PathBuf>> {
    let record_path = record_path(locations);
    let mut record = load(&record_path)?;
    let Some(changes) = record.owners.get(owner) else {
        return Ok(Vec::new());
    };
    let mut files: BTreeMap<&Path, Vec<&Change>> = BTreeMap::new();
    for change in changes {
        files.entry(&change.path).or_default().push(change);
    }
    let mut changed = Vec::new();
    for (path, changes) in files {
        let original = text(path)?;
        let format = changes[0].format;
        ensure!(changes.iter().all(|change|change.format == format),"theme preference formats disagree");
        let mut next = format.parse(&original)?;
        for change in &changes {
            if next.get(&change.section,&change.key)?.as_deref() == Some(change.after.as_str()) {
                next.set(&change.section,&change.key,change.before.as_deref())?;
            }
        }
        let next = next.finish();
        if next != original {
            if changes.iter().all(|change| change.created)
                && next
                    .lines()
                    .all(|line| line.trim().is_empty() || line.trim().starts_with('['))
            {
                write(path,&original,&next)?;
                std::fs::remove_file(path)?;
            } else {
                write(path,&original,&next)?;
            }
            changed.push(path.to_path_buf());
        }
    }
    record.owners.remove(owner);
    store(&record_path, &record)?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_replacements_and_interrupted_writes_keep_the_original_undo_value() {
        for committed in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let locations = Locations { home:root.path().into(), config:root.path().join("config"), cache:root.path().join("cache"), data:root.path().join("data"), state:root.path().join("state") };
            let path = root.path().join("preferences");
            std::fs::write(&path, "[Theme]\nColor=Original\n").unwrap();
            let keys = |value: &str| ini_keys(&format!("[Theme]\nColor={value}\n")).unwrap();
            set("app", &path, &keys("A"), &locations).unwrap();
            assert!(set_with("app", &path, &keys("B"), Format::Ini, &locations, &|_,_,_| anyhow::bail!("write failed")).is_err());
            let record = load(&record_path(&locations)).unwrap();
            assert_eq!(record.owners["app"][0].after, "A");
            assert_eq!(record.owners["app"][0].before.as_deref(), Some("Original"));
            let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = set_with("app", &path, &keys("B"), Format::Ini, &locations, &|path,_,next| {
                    if committed { mandara_config::store::write_atomic(path, next.as_bytes())?; }
                    panic!("interrupted after journaling");
                });
            }));
            assert!(interrupted.is_err());
            // Recovery may run after an unrelated edit in the same file.
            use std::io::Write as _;
            writeln!(std::fs::OpenOptions::new().append(true).open(&path).unwrap(), "Unrelated=keep").unwrap();
            set("app", &path, &keys("C"), &locations).unwrap();
            undo("app", &locations).unwrap();
            let restored = std::fs::read_to_string(&path).unwrap();
            assert_eq!(ini_get(&restored, "Theme", "Color").as_deref(), Some("Original"));
            assert_eq!(ini_get(&restored, "Theme", "Unrelated").as_deref(), Some("keep"));
        }
    }

    #[test]
    fn theme_key_edits_preserve_symlinks_comments_and_later_user_color_changes() {
        for (format,original,section,key) in [
            (Format::Toml,"# personal\n[theme]\nborder-color = '#123456' # keep\nfont = 'Personal'\n","theme","border-color"),
            (Format::Xresources,"# personal\nwindow.active.border.color: #123456\nborder.width: 4\n","","window.active.border.color"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let locations = Locations { home:root.path().into(),config:root.path().join("config"),cache:root.path().join("cache"),data:root.path().join("data"),state:root.path().join("state") };
            let target = root.path().join("real");let path = root.path().join("config-file");
            std::fs::write(&target,original).unwrap();std::os::unix::fs::symlink(&target,&path).unwrap();
            let keys = [(section.into(),[(key.into(),"#abcdef".into())].into())].into();
            let write = |path:&Path,_:&str,next:&str|mandara_config::store::write_atomic(path,next.as_bytes());
            set_with("theme",&path,&keys,format,&locations,&write).unwrap();
            let applied = std::fs::read_to_string(&target).unwrap();
            assert!(applied.contains("# personal"));
            assert_eq!(format.parse(&applied).unwrap().get(section,key).unwrap().as_deref(),Some("#abcdef"));
            undo_with("theme",&locations,&write).unwrap();
            let restored = std::fs::read_to_string(&target).unwrap();
            assert_eq!(format.parse(&restored).unwrap().get(section,key).unwrap().as_deref(),Some("#123456"));
            assert!(restored.contains("# personal"));
            set_with("theme",&path,&keys,format,&locations,&write).unwrap();
            let edited = std::fs::read_to_string(&target).unwrap().replace("#abcdef","#fedcba");
            std::fs::write(&target,&edited).unwrap();
            undo_with("theme",&locations,&write).unwrap();
            assert_eq!(std::fs::read_to_string(&target).unwrap(),edited);
            assert_eq!(std::fs::read_link(&path).unwrap(),target);
        }
    }

    #[test]
    fn unchanged_values_still_record_ownership_before_later_updates() {
        let root = tempfile::tempdir().unwrap();
        let locations = Locations {
            home: root.path().into(),
            config: root.path().join("config"),
            data: root.path().join("data"),
            cache: root.path().join("cache"),
            state: root.path().join("state"),
        };
        let path = root.path().join("preferences");
        std::fs::write(&path, "[General]\nColorScheme=Original\n").unwrap();
        set(
            "one",
            &path,
            &ini_keys("[General]\nColorScheme=Original\n").unwrap(),
            &locations,
        )
        .unwrap();
        assert!(
            set(
                "two",
                &path,
                &ini_keys("[General]\nColorScheme=Other\n").unwrap(),
                &locations
            )
            .is_err()
        );
        set(
            "one",
            &path,
            &ini_keys("[General]\nColorScheme=Themed\n").unwrap(),
            &locations,
        )
        .unwrap();
        undo("one", &locations).unwrap();
        assert_eq!(
            ini_get(
                &std::fs::read_to_string(path).unwrap(),
                "General",
                "ColorScheme"
            )
            .as_deref(),
            Some("Original")
        );
    }

    #[test]
    fn updates_retire_omitted_keys_and_templates_cannot_take_each_others_preferences() {
        let root = tempfile::tempdir().unwrap();
        let locations = Locations {
            home: root.path().into(),
            config: root.path().join("config"),
            data: root.path().join("data"),
            cache: root.path().join("cache"),
            state: root.path().join("state"),
        };
        let path = root.path().join("kdeglobals");
        std::fs::write(&path, "[General]\nColorScheme=User\n[Icons]\nTheme=Keep\n").unwrap();
        set(
            "one",
            &path,
            &ini_keys("[General]\nColorScheme=Themed\n[Colors:Window]\nForegroundNormal=1,2,3\n")
                .unwrap(),
            &locations,
        )
        .unwrap();
        let original = std::fs::read_to_string(&path).unwrap();
        assert!(
            set(
                "two",
                &path,
                &ini_keys("[General]\nColorScheme=Other\n").unwrap(),
                &locations
            )
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        set(
            "one",
            &path,
            &ini_keys("[Colors:Window]\nForegroundNormal=4,5,6\n").unwrap(),
            &locations,
        )
        .unwrap();
        let updated = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            ini_get(&updated, "General", "ColorScheme").as_deref(),
            Some("User")
        );
        assert_eq!(ini_get(&updated, "Icons", "Theme").as_deref(), Some("Keep"));
        undo("one", &locations).unwrap();
        assert!(
            ini_get(
                &std::fs::read_to_string(path).unwrap(),
                "Colors:Window",
                "ForegroundNormal"
            )
            .is_none()
        );
    }

    #[test]
    fn disabling_restores_colors_without_removing_later_unrelated_preferences() {
        let root = tempfile::tempdir().unwrap();
        let locations = Locations {
            home: root.path().into(),
            config: root.path().join("config"),
            data: root.path().join("data"),
            cache: root.path().join("cache"),
            state: root.path().join("state"),
        };
        std::fs::create_dir_all(&locations.config).unwrap();
        let target = locations.config.join("real-kdeglobals");
        let path = locations.config.join("kdeglobals");
        std::fs::write(
            &target,
            "[General]\nColorScheme=User\n[Icons]\nTheme=Original\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        set(
            "editor",
            &path,
            &ini_keys("[General]\nColorScheme=Mandara\n[Colors:Window]\nBackgroundNormal=1,2,3\n")
                .unwrap(),
            &locations,
        )
        .unwrap();
        let body = std::fs::read_to_string(&path)
            .unwrap()
            .replace("Theme=Original", "Theme=New");
        std::fs::write(&target, body).unwrap();
        undo("editor", &locations).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            ini_get(&body, "General", "ColorScheme").as_deref(),
            Some("User")
        );
        assert_eq!(ini_get(&body, "Icons", "Theme").as_deref(), Some("New"));
        assert_eq!(ini_get(&body, "Colors:Window", "BackgroundNormal"), None);
        assert_eq!(std::fs::read_link(path).unwrap(), target);
    }
}
