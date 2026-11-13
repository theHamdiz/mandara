//! Explicit consent for a catalog revision's destinations and commands.

use super::{catalog::Installed, paths::Locations};
use anyhow::{Result, ensure};
use mandara_config::app_themes::AppThemesConfig;
use mandara_theme::templates::destination;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Review {
    pub id: String,
    pub revision: String,
    pub files: Vec<PathBuf>,
    pub commands: Vec<String>,
    pub actions: Vec<String>,
    pub fingerprint: String,
    pub shared_palette: bool,
}

struct Hash(Sha256);
impl std::io::Write for Hash {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Review {
    fn file(&mut self, path: PathBuf, follows_link: bool) -> Result<()> {
        let resolved = if follows_link {
            destination::identity(&path)?
        } else {
            destination::slot(&path)?
        };
        self.files.extend([path, resolved]);
        Ok(())
    }

    pub fn new(id: &str, package: &Installed, locations: &Locations) -> Result<Self> {
        let revision = package
            .root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        ensure!(
            revision.len() == 64 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "app template has no immutable revision"
        );
        let mut review = Self {
            id: id.into(),
            revision: revision.into(),
            files: Vec::new(),
            commands: Vec::new(),
            actions: Vec::new(),
            fingerprint: String::new(),
            shared_palette: false,
        };
        let assets = destination::identity(&package.root)?;
        for (name, entry) in &package.manifest.templates {
            let owner = format!("{id}/{name}");
            let adapted = (package.category == "compositor")
                .then(|| super::compositor::prepare(&owner, entry, locations))
                .transpose()?;
            let entry = adapted.as_ref().unwrap_or(entry);
            let base = entry
                .directory
                .as_ref()
                .map(|path| locations.resolve(&package.root, path))
                .transpose()?
                .unwrap_or_else(|| package.root.clone());
            if entry.content.is_none() {
                for input in std::iter::once(&entry.input_path)
                    .chain(entry.input_path_modes.values())
                    .filter(|path| !path.as_os_str().is_empty())
                {
                    let path = locations.resolve(&base, input)?;
                    let resolved = destination::identity(&path)?;
                    if !resolved.starts_with(&assets) {
                        review.files.extend([path, resolved]);
                    }
                }
            }
            for path in entry.output_path.iter() {
                review.file(locations.resolve(&base, path)?, false)?;
            }
            if package.category == "compositor" {
                for path in super::compositor::destinations(&owner, locations)? {
                    review.file(path, true)?;
                }
            }
            if let Some(extension) = &entry.gtk_extension {
                let version = match extension.as_str() {
                    "gtk3" => "gtk-3.0",
                    "gtk4" => "gtk-4.0",
                    _ => anyhow::bail!("unknown GTK extension"),
                };
                review.file(locations.config.join(version).join("gtk.css"), true)?;
                review.file(
                    locations
                        .config
                        .join(version)
                        .join(format!("mandara-{}.css", owner.replace([':', '/'], "-"))),
                    false,
                )?;
            }
            match entry.post_action.as_str() {
                "firefox-theme" => {
                    for path in [
                        locations
                            .home
                            .join(".mozilla/native-messaging-hosts/pywalfox.json"),
                        super::firefox::settings_path(locations),
                        locations.cache.join("wal/colors.json"),
                    ] {
                        review.file(path, false)?;
                    }
                    review.actions.push(entry.post_action.clone());
                }
                "kde-color-scheme" => {
                    review.file(locations.config.join("kdeglobals"), true)?;
                    review.actions.push(entry.post_action.clone());
                }
                "" => {}
                other => anyhow::bail!("unknown app theme action: {other}"),
            }
            for command in [
                &entry.input_path_dynamic,
                &entry.output_path_dynamic,
                &entry.pre_hook,
                &entry.post_hook,
                &entry.undo_hook,
            ] {
                if !command.is_empty() {
                    review.commands.push(command.clone());
                }
            }
        }
        review.files.sort();
        review.files.dedup();
        review.commands.sort();
        review.commands.dedup();
        review.actions.sort();
        review.actions.dedup();
        review.shared_palette = review
            .actions
            .iter()
            .any(|action| action == "firefox-theme");
        let mut hash = Hash(Sha256::new());
        serde_json::to_writer(
            &mut hash,
            &(
                &review.id,
                &review.revision,
                &review.files,
                &review.commands,
                &review.actions,
                &package.manifest,
                locations.environment(),
            ),
        )?;
        review.fingerprint = format!("{:x}", hash.0.finalize());
        Ok(review)
    }

    pub fn check(&self, config: &AppThemesConfig) -> Result<()> {
        ensure!(
            config.approved.get(&self.id) == Some(&self.fingerprint),
            mandara_i18n::t(
                "Review files and commands before enabling this template. Commands can change other files too. Updates need a new review."
            )
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{apply, manifest::Manifest};
    use super::*;
    use mandara_theme::templates::custom::{Paths, Template};
    use std::sync::atomic::AtomicBool;

    fn locations(root: &std::path::Path) -> Locations {
        Locations {
            home: root.into(),
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
            state: root.join("state"),
        }
    }
    fn package(root: PathBuf) -> Installed {
        Installed {
            root,
            manifest: Manifest {
                templates: [(
                    "startup".into(),
                    Template {
                        content: Some("reviewed content".into()),
                        output_path: Paths::One("~/.bashrc".into()),
                        ..Default::default()
                    },
                )]
                .into(),
                ..Default::default()
            },
            category: String::new(),
            lease: None,
        }
    }
    #[test]
    fn external_template_inputs_are_included_in_the_file_review() {
        let root = tempfile::tempdir().unwrap();
        let locations = locations(root.path());
        let mut package = package(root.path().join("a".repeat(64)));
        let entry = package.manifest.templates.get_mut("startup").unwrap();
        entry.content = None;
        entry.input_path = "$XDG_CONFIG_HOME/private-template".into();
        let review = Review::new("community:app", &package, &locations).unwrap();
        assert!(
            review
                .files
                .contains(&locations.config.join("private-template"))
        );
        let first = review.fingerprint;
        package
            .manifest
            .templates
            .get_mut("startup")
            .unwrap()
            .input_path = "$XDG_CONFIG_HOME/another-template".into();
        assert_ne!(
            Review::new("community:app", &package, &locations)
                .unwrap()
                .fingerprint,
            first
        );
    }

    #[test]
    fn approval_covers_the_revision_commands_and_resolved_destinations() {
        let root = tempfile::tempdir().unwrap();
        let locations = locations(root.path());
        let original = package(root.path().join("a".repeat(64)));
        let review = Review::new("community:app", &original, &locations).unwrap();
        assert!(review.files.contains(&locations.home.join(".bashrc")));
        let mut config = AppThemesConfig::default();
        assert!(review.check(&config).is_err());
        config
            .approved
            .insert(review.id.clone(), review.fingerprint.clone());
        review.check(&config).unwrap();
        let mut changed = original.clone();
        changed
            .manifest
            .templates
            .get_mut("startup")
            .unwrap()
            .post_hook = "change files".into();
        assert!(
            Review::new("community:app", &changed, &locations)
                .unwrap()
                .check(&config)
                .is_err()
        );
        let mut changed = original.clone();
        changed.root = root.path().join("b".repeat(64));
        assert!(
            Review::new("community:app", &changed, &locations)
                .unwrap()
                .check(&config)
                .is_err()
        );
        let mut directory = original.clone();
        directory
            .manifest
            .templates
            .get_mut("startup")
            .unwrap()
            .output_path = Paths::One("~/alias/theme".into());
        let first = root.path().join("first");
        let second = root.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&first, &alias).unwrap();
        let review = Review::new("community:app", &directory, &locations).unwrap();
        config
            .approved
            .insert(review.id.clone(), review.fingerprint.clone());
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(second, &alias).unwrap();
        assert!(
            Review::new("community:app", &directory, &locations)
                .unwrap()
                .check(&config)
                .is_err()
        );
    }

    #[test]
    fn a_catalog_cannot_write_startup_files_before_review_even_with_commands_off() {
        let root = tempfile::tempdir().unwrap();
        let locations = locations(root.path());
        let directory = locations.catalogs().join("community/app");
        let package = package(directory.join("a".repeat(64)));
        std::fs::create_dir_all(&package.root).unwrap();
        std::fs::write(
            directory.join("installed.json"),
            serde_json::to_vec(&package).unwrap(),
        )
        .unwrap();
        let startup = locations.home.join(".bashrc");
        let personal = locations.home.join("personal-shell-config");
        std::fs::write(&personal, "personal").unwrap();
        std::os::unix::fs::symlink(&personal, &startup).unwrap();
        let mut config = AppThemesConfig {
            enabled: vec!["community:app".into()],
            run_hooks: false,
            ..Default::default()
        };
        let theme = apply::Theme {
            palette: mandara_theme::engine::default_palette(true),
            other: mandara_theme::engine::default_palette(false),
            font_family: "Inter".into(),
            image: PathBuf::new(),
        };
        let cancel = AtomicBool::new(false);
        let report = apply::sync(&config, &theme, &locations, &cancel, &|| false).unwrap();
        assert!(report.errors["community:app"].contains("review"));
        assert_eq!(std::fs::read_to_string(&startup).unwrap(), "personal");
        let review = Review::new("community:app", &package, &locations).unwrap();
        config.approved.insert(review.id, review.fingerprint);
        assert!(
            apply::sync(&config, &theme, &locations, &cancel, &|| false)
                .unwrap()
                .errors
                .is_empty()
        );
        assert_eq!(
            std::fs::read_to_string(&startup).unwrap(),
            "reviewed content"
        );
        assert!(
            apply::sync(&config, &theme, &locations, &cancel, &|| false)
                .unwrap()
                .errors
                .is_empty()
        );
        assert_eq!(std::fs::read_to_string(&personal).unwrap(), "personal");
        config.enabled.clear();
        assert!(
            apply::sync(&config, &theme, &locations, &cancel, &|| false)
                .unwrap()
                .errors
                .is_empty()
        );
        assert_eq!(std::fs::read_to_string(startup).unwrap(), "personal");
    }
}
