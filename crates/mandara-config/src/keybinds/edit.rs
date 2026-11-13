//! Re-read the exact scanned source before preparing a native field edit.

use anyhow::{Context as _, Result, ensure};
use mandara_i18n::t;

use super::{
    Binding, BindingId, Encoding,
    scan::{fingerprint, read},
};
use crate::compositor::Target;

pub struct PreparedEdit {
    pub id: BindingId,
    pub original: String,
    next: Option<String>,
    pub old_binding: Binding,
    new_binding: Option<Binding>,
}

/// Undo holds only the edited row's values and the new source identity.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct UndoEdit {
    pub id: BindingId,
    pub values: Vec<String>,
}

fn stale() -> String {
    t("The compositor shortcuts changed while they were being checked. Refresh the list and try again.").into()
}

pub fn prepare(target: Target, id: &BindingId, values: &[String]) -> Result<PreparedEdit> {
    ensure!(
        std::fs::canonicalize(&id.file).context(stale())? == id.target,
        "{}",
        stale()
    );
    let original = read(&id.file)?;
    ensure!(fingerprint(&original) == id.fingerprint, "{}", stale());
    let rows = super::parse(target, &id.target, &original)?;
    let count = rows.len();
    let old_binding = rows.into_iter().nth(id.index).context(stale())?;
    let next = match super::patch(&original, &old_binding, values)? {
        std::borrow::Cow::Borrowed(_) => None,
        std::borrow::Cow::Owned(next) => Some(next),
    };
    let new_binding = if let Some(next) = next.as_deref() {
        let rows = super::parse(target, &id.target, next)?;
        ensure!(
            rows.len() == count,
            "{}",
            t("These values would change the structure of the shortcut configuration.")
        );
        let row = rows.into_iter().nth(id.index).context(stale())?;
        ensure!(
            row.read_only.is_none() && row.fields.len() == old_binding.fields.len(),
            "{}",
            t("These values would change the structure of the shortcut configuration.")
        );
        for ((before, after), value) in old_binding.fields.iter().zip(&row.fields).zip(values) {
            let expected = match before.encoding {
                Encoding::TomlString
                | Encoding::KdlString
                | Encoding::XmlAttribute
                | Encoding::XmlText
                | Encoding::ShellWord => value.as_str(),
                _ => value.trim(),
            };
            ensure!(
                before.label == after.label && before.name == after.name && after.value == expected,
                "{}",
                t("The compositor did not read these shortcut values as intended.")
            );
        }
        super::formats::format(target, &id.target, next).validate_edit(&old_binding, &row)?;
        Some(row)
    } else {
        None
    };
    Ok(PreparedEdit {
        id: id.clone(),
        original,
        next,
        old_binding,
        new_binding,
    })
}

impl PreparedEdit {
    pub fn changed(&self) -> bool {
        self.next.is_some()
    }
    pub fn body(&self) -> &str {
        self.next.as_deref().unwrap_or(&self.original)
    }
    pub fn binding(&self) -> &Binding {
        self.new_binding.as_ref().unwrap_or(&self.old_binding)
    }

    pub fn undo(&self) -> Option<UndoEdit> {
        self.next.as_ref()?;
        Some(UndoEdit {
            id: BindingId {
                fingerprint: fingerprint(self.body()),
                ..self.id.clone()
            },
            values: self
                .old_binding
                .fields
                .iter()
                .map(|field| field.value.clone())
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    #[test]
    fn edited_cosmic_actions_are_validated_while_unchanged_future_actions_survive() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("custom");
        fs::write(&path, "{(modifiers: [Super], key: \"q\"): FutureAction(4)}").unwrap();
        let source = super::super::scan_from(Target::Cosmic, &path);
        let id = source.sources[0].binding_id(0).unwrap();
        assert!(
            prepare(
                Target::Cosmic,
                &id,
                &["[Super]".into(), "\"w\"".into(), "FutureAction(4)".into()]
            )
            .is_ok()
        );
        assert!(
            prepare(
                Target::Cosmic,
                &id,
                &["[Super]".into(), "\"w\"".into(), "Clsoe".into()]
            )
            .is_err()
        );
        assert!(
            prepare(
                Target::Cosmic,
                &id,
                &["[Super]".into(), "\"w\"".into(), "System(Terminal)".into()]
            )
            .is_ok()
        );
    }

    #[test]
    fn aqueous_system_action_identifiers_cannot_be_renamed_into_ignored_options() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("wm.toml");
        fs::write(&path, "[keybinds]\nspawn_terminal='Super+T'\n").unwrap();
        let source = super::super::scan_from(Target::Aqueous, &path);
        let id = source.sources[0].binding_id(0).unwrap();
        assert!(
            prepare(
                Target::Aqueous,
                &id,
                &["spawn_terminal".into(), "Super+Return".into()]
            )
            .is_ok()
        );
        assert!(
            prepare(
                Target::Aqueous,
                &id,
                &["invented_action".into(), "Super+Return".into()]
            )
            .is_err()
        );
    }

    #[test]
    fn preparing_an_edit_does_not_write_and_undo_targets_only_that_saved_revision() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        let original = "# keep\n[keybinds]\n'Super+T' = 'spawn foot'\n[other]\nvalue=4\n";
        fs::write(&path, original).unwrap();
        let scan = super::super::scan_from(Target::Umbriel, &path);
        let source = &scan.sources[0];
        let id = source.binding_id(0).unwrap();
        let prepared = prepare(
            Target::Umbriel,
            &id,
            &["Super+T".into(), "spawn kitty".into()],
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        let undo = prepared.undo().unwrap();
        assert!(prepare(Target::Umbriel, &undo.id, &undo.values).is_err());
        fs::write(&path, prepared.body()).unwrap();
        let restored = prepare(Target::Umbriel, &undo.id, &undo.values).unwrap();
        assert_eq!(restored.binding().fields[1].value, "spawn foot");
        assert_eq!(
            restored.body(),
            original.replace("'spawn foot'", "\"spawn foot\"")
        );
        fs::write(&path, "[keybinds]\n'Super+X' = 'my new shortcut'\n").unwrap();
        assert!(prepare(Target::Umbriel, &undo.id, &undo.values).is_err());
    }

    #[test]
    fn a_retargeted_symlink_is_stale_even_when_its_new_destination_has_identical_bytes() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first.toml");
        let second = root.path().join("second.toml");
        let alias = root.path().join("config.toml");
        for path in [&first, &second] {
            fs::write(path, "[keybinds]\n'Super+T'='spawn foot'\n").unwrap();
        }
        symlink(&first, &alias).unwrap();
        let source = super::super::scan_from(Target::Umbriel, &alias);
        let id = source.sources[0].binding_id(0).unwrap();
        fs::remove_file(&alias).unwrap();
        symlink(&second, &alias).unwrap();
        assert!(
            prepare(
                Target::Umbriel,
                &id,
                &["Super+T".into(), "spawn kitty".into()]
            )
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(&first).unwrap(),
            fs::read_to_string(&second).unwrap()
        );
    }

    #[test]
    fn unchanged_values_do_not_allocate_a_second_config_or_an_undo_record() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        fs::write(&path, "[keybinds]\n'Super+T'='spawn foot'\n").unwrap();
        let source = super::super::scan_from(Target::Umbriel, &path);
        let id = source.sources[0].binding_id(0).unwrap();
        let prepared = prepare(
            Target::Umbriel,
            &id,
            &["Super+T".into(), "spawn foot".into()],
        )
        .unwrap();
        assert!(!prepared.changed());
        assert!(prepared.next.is_none());
        assert!(prepared.undo().is_none());
        assert_eq!(prepared.body().as_ptr(), prepared.original.as_ptr());
    }
}
