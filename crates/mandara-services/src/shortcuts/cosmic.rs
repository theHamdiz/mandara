//! Inherited bindings stay in the installed catalog; edits create user overrides.

use super::{Preferences, Saved, Session, Undo, transaction};
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
        let custom: std::collections::BTreeSet<_> = scan
            .sources
            .iter()
            .flat_map(|source| &source.bindings)
            .filter_map(|row| keybinds::cosmic::identity(row).ok())
            .collect();
        let inherited = keybinds::scan_from(Target::Cosmic, &defaults);
        scan.errors.extend(inherited.errors);
        for mut source in inherited.sources {
            for row in &mut source.bindings {
                row.scope = "defaults".into();
                if keybinds::cosmic::identity(row).is_ok_and(|key| custom.contains(&key)) {
                    row.read_only = Some(mandara_i18n::tk(
                        "A custom shortcut overrides this default. Edit the custom shortcut instead.",
                    ));
                }
            }
            if scan
                .sources
                .iter()
                .map(|source| source.bindings.len())
                .sum::<usize>()
                + source.bindings.len()
                > keybinds::MAX_BINDINGS
            {
                scan.errors.push((
                    source.file,
                    mandara_i18n::t("The compositor has too many shortcuts to edit at once.")
                        .into(),
                ));
                break;
            }
            scan.sources.push(source);
        }
    }
    super::permissions::mark(&mut scan, Some(&root));
    Session {
        environment: Default::default(),
        target: Target::Cosmic,
        root,
        scan,
        primary_mod_is_alt: false,
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
    let home = dirs::data_dir().into_iter();
    let directories =
        std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    catalog_directory(home.chain(std::env::split_paths(&directories)))
        .map(|path| path.join("defaults"))
        .filter(|path| path.is_file())
}

fn catalog_directory(directories: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    directories
        .into_iter()
        .map(|path| path.join("cosmic/com.system76.CosmicSettings.Shortcuts/v1"))
        .find(|path| path.is_dir())
}

pub(super) fn save(
    session: &Session,
    edit: &keybinds::PreparedEdit,
    initial: &Preferences,
    preferences: &dyn Fn() -> Result<Preferences>,
    backend: &dyn transaction::Backend,
) -> Result<Saved> {
    let path = &session.root;
    let new = keybinds::cosmic::identity(edit.binding())?;
    for source in &session.scan.sources {
        for (index, row) in source.bindings.iter().enumerate() {
            ensure!(
                source.file == edit.id.file && index == edit.id.index
                    || row.read_only.is_some()
                    || keybinds::cosmic::identity(row)? != new,
                "{}",
                mandara_i18n::t(
                    "Another compositor shortcut already uses this key. Edit that shortcut or choose a different key."
                )
            );
        }
    }
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
    fn installed_catalog_priority_is_chosen_by_directory_before_entry_lookup() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        let suffix = "cosmic/com.system76.CosmicSettings.Shortcuts/v1";
        std::fs::create_dir_all(first.join(suffix)).unwrap();
        std::fs::create_dir_all(second.join(suffix)).unwrap();
        std::fs::write(second.join(suffix).join("defaults"), "{}").unwrap();
        let selected = catalog_directory([first.clone(), second]).unwrap();
        assert_eq!(selected, first.join(suffix));
        assert!(!selected.join("defaults").exists());
    }
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

    #[test]
    fn default_edits_and_compact_undo_touch_only_user_overrides() {
        let directory = tempfile::tempdir().unwrap();
        let custom = directory.path().join("custom");
        let defaults = custom.with_file_name("defaults");
        let catalog = "{(modifiers:[Super],key:\"q\"):Close,(modifiers:[Alt],key:\"F4\"):Close}";
        let personal =
            "// personal\n{ (modifiers:[Alt],key:\"F12\"): Spawn(\"foot\") } // trailing }\n";
        std::fs::write(&defaults, catalog).unwrap();
        std::fs::write(&custom, personal).unwrap();
        let session = load(custom.clone());
        let source = session
            .scan
            .sources
            .iter()
            .find(|source| source.file == defaults)
            .unwrap();
        let id = source.binding_id(0).unwrap();
        let edit = keybinds::prepare(
            Target::Cosmic,
            &id,
            &["[Super]".into(), "\"w\"".into(), "Close".into()],
        )
        .unwrap();
        let initial = Preferences {
            target: Target::Cosmic,
            manage: false,
            keybinds: Vec::new(),
        };
        let preferences = || Ok(initial.clone());
        let saved = save(&session, &edit, &initial, &preferences, &Checker).unwrap();
        let Undo::Cosmic(inverse) = saved.undo.unwrap() else {
            panic!("default edit has a document undo")
        };
        assert!(serde_json::to_vec(&inverse).unwrap().len() < 1024);
        assert_eq!(std::fs::read_to_string(&defaults).unwrap(), catalog);
        assert_eq!(
            keybinds::scan_from(Target::Cosmic, &custom).sources[0]
                .bindings
                .len(),
            3
        );
        let session = load(custom.clone());
        undo(&session, &inverse, &initial, &preferences, &Checker).unwrap();
        assert_eq!(std::fs::read_to_string(&custom).unwrap(), personal);
        std::fs::write(&custom, format!("{personal}// later edit\n")).unwrap();
        assert!(
            undo(
                &load(custom.clone()),
                &inverse,
                &initial,
                &preferences,
                &Checker
            )
            .is_err()
        );
        assert!(
            std::fs::read_to_string(custom)
                .unwrap()
                .ends_with("// later edit\n")
        );
    }
}
