//! Inherited bindings stay in the installed catalog; edits create user overrides.

use super::{Preferences, Saved, Session, Undo, native::Native, transaction};
use anyhow::{Result, ensure};
use mandara_config::{
    compositor::Target,
    doctor::theme::{self, FileEdit},
    keybinds::{self, TextUndo},
};
use mandara_theme::templates::WriteLock;
use std::path::{Path, PathBuf};

pub(super) fn load(root: PathBuf) -> Session {
    let mut scan = if root.exists() {
        keybinds::scan_from(Target::Cosmic, &root)
    } else {
        Default::default()
    };
    if let Some(defaults) = defaults_path(&root) {
        let inherited = keybinds::scan_from(Target::Cosmic, &defaults);
        scan.errors.extend(inherited.errors);
        for mut source in inherited.sources {
            for row in &mut source.bindings {
                row.scope = "defaults".into();
                if keybinds::cosmic::identity(row).is_ok_and(|key| {
                    scan.sources
                        .iter()
                        .flat_map(|source| &source.bindings)
                        .any(|custom| {
                            keybinds::cosmic::identity(custom).is_ok_and(|custom| custom == key)
                        })
                }) {
                    row.read_only = Some(mandara_i18n::tk(
                        "A custom shortcut overrides this default. Edit the custom shortcut instead.",
                    ));
                }
            }
            scan.sources.push(source);
        }
    }
    Session {
        target: Target::Cosmic,
        root,
        scan,
    }
}

pub(super) struct Checker;
impl transaction::Backend for Checker {
    fn validate(&self, path: &Path, body: &str) -> Result<()> {
        keybinds::parse(Target::Cosmic, path, body)?;
        Ok(())
    }
    fn check(&self) -> theme::LiveCheck {
        Ok::<(), anyhow::Error>(()).into()
    }
    fn apply(&self) -> Result<()> {
        Ok(())
    }
    fn restore(&self) -> Result<()> {
        Ok(())
    }
    fn version(&self) -> Option<(u32, u32)> {
        None
    }
}

fn defaults_path(custom: &Path) -> Option<PathBuf> {
    let local = custom.with_file_name("defaults");
    if local.exists() {
        return Some(local);
    }
    let relative = "cosmic/com.system76.CosmicSettings.Shortcuts/v1/defaults";
    let home = dirs::data_dir().into_iter();
    let directories =
        std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    home.chain(std::env::split_paths(&directories))
        .map(|path| path.join(relative))
        .find(|path| path.exists())
}

pub(super) fn save(
    session: &Session,
    edit: &keybinds::PreparedEdit,
    initial: &Preferences,
    preferences: &dyn Fn() -> Result<Preferences>,
    backend: &Native<'_>,
) -> Result<Saved> {
    let path = &session.root;
    let original = if path.exists() {
        crate::app_themes::apply::read_text(path)?
    } else {
        String::new()
    };
    let next = keybinds::cosmic::override_default(
        if original.is_empty() { "{}" } else { &original },
        &edit.old_binding,
        edit.binding(),
    )?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    publish(
        session,
        path,
        None,
        &original,
        &next,
        initial,
        preferences,
        backend,
    )
}

pub(super) fn undo(
    session: &Session,
    undo: &TextUndo,
    initial: &Preferences,
    preferences: &dyn Fn() -> Result<Preferences>,
    backend: &dyn transaction::Backend,
) -> Result<Saved> {
    ensure!(
        undo.file == session.root,
        "COSMIC undo belongs to another configuration"
    );
    let original = crate::app_themes::apply::read_text(&undo.file)?;
    let next = undo.apply(&original)?;
    publish(
        session,
        &undo.file,
        Some(&undo.target),
        &original,
        &next,
        initial,
        preferences,
        backend,
    )
}

fn publish(
    session: &Session,
    path: &Path,
    expected: Option<&Path>,
    original: &str,
    next: &str,
    initial: &Preferences,
    preferences: &dyn Fn() -> Result<Preferences>,
    backend: &dyn transaction::Backend,
) -> Result<Saved> {
    let lock = WriteLock::new(path)?;
    ensure!(
        expected.is_none_or(|expected| expected == lock.target()),
        "COSMIC shortcut path changed"
    );
    // An absent custom map and an empty map have identical native meaning.
    // Undo leaves an empty valid map when it removes the first user override.
    let next = if next.is_empty() { "{}" } else { next };
    let guard = transaction::DocumentGuard {
        session,
        destination: lock.target(),
        initial,
        preferences,
        backend,
    };
    guard.fresh()?;
    theme::guarded_batch(
        &[FileEdit {
            lock: &lock,
            before: original,
            after: next,
            intermediate: None,
        }],
        &guard,
    )?;
    Ok(Saved {
        undo: Some(Undo::Cosmic(TextUndo::between(
            path.into(),
            lock.target().into(),
            original,
            next,
        ))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn user_defaults_load_without_a_custom_file_and_shadowed_defaults_stay_visible() {
        let directory = tempfile::tempdir().unwrap();
        let custom = directory.path().join("custom");
        std::fs::write(
            custom.with_file_name("defaults"),
            "{(modifiers:[Super],key:\"q\"):Close}",
        )
        .unwrap();
        let session = load(custom.clone());
        assert!(session.scan.errors.is_empty());
        assert_eq!(session.scan.sources.len(), 1);
        assert!(session.scan.sources[0].bindings[0].read_only.is_none());
        std::fs::write(&custom, "{(modifiers:[Super],key:\"q\"):Disable}").unwrap();
        let session = load(custom);
        assert_eq!(session.scan.sources.len(), 2);
        assert!(session.scan.sources[1].bindings[0].read_only.is_some());
    }
}
