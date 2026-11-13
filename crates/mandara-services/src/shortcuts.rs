//! Short-lived native shortcut scans and reversible, field-scoped saves.

mod cosmic;
mod kwin;
mod native;
mod permissions;
mod transaction;

use std::path::PathBuf;

use anyhow::{Context as _, Result, ensure};
use mandara_config::{
    compositor::{Keybind, Target},
    keybinds::{self, BindingId, Scan, TextUndo, UndoEdit},
};
use mandara_i18n::t;

/// Only binding preferences cross the process boundary; no shell config is retained.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Preferences {
    pub target: Target,
    pub manage: bool,
    pub keybinds: Vec<Keybind>,
}

#[derive(Debug, serde::Serialize)]
pub struct Session {
    pub target: Target,
    pub root: PathBuf,
    pub scan: Scan,
    pub primary_mod_is_alt: bool,
    #[serde(skip)]
    pub environment: mandara_config::compositor::IncludeEnvironment,
}

impl Session {
    fn selected(&self, id: &BindingId) -> Result<&keybinds::Binding> {
        let row = self.scan.sources.iter()
            .find(|source| source.file == id.file && source.target == id.target && source.fingerprint == id.fingerprint)
            .and_then(|source| source.bindings.get(id.index))
            .ok_or_else(|| anyhow::anyhow!("{}", t("The compositor shortcuts changed while they were being checked. Refresh the list and try again.")))?;
        if let Some(reason) = row.read_only {
            anyhow::bail!("{}", t(reason));
        }
        Ok(row)
    }
}

#[derive(serde::Serialize)]
pub struct Saved {
    pub undo: Option<Undo>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum Undo {
    Native(UndoEdit),
    Cosmic(TextUndo),
}

pub fn load() -> Result<Session> {
    let target = mandara_compositor::detected()
        .map(mandara_compositor::CompositorKind::target)
        .ok_or_else(|| anyhow::anyhow!("{}", t("No supported compositor is running.")))?;
    let native = mandara_compositor::native_config::running(target)?;
    let root = native
        .file
        .unwrap_or_else(|| keybinds::default_root(target));
    if target == Target::Cosmic {
        return Ok(cosmic::load(root));
    }
    let environment = native.environment.unwrap_or_default();
    let mut scan = keybinds::scan_roots_in(
        target,
        std::iter::once(root.clone()).chain(native.additional_files),
        &environment,
    );
    if target == Target::Niri {
        let nested = mandara_compositor::niri::NiriDriver::from_env()
            .context(t(
                "The compositor connection is unavailable. No files were changed.",
            ))?
            .is_nested()?;
        keybinds::resolve_niri_mod(&mut scan, nested);
    }
    permissions::mark(&mut scan, None);
    Ok(Session {
        target,
        root,
        scan,
        primary_mod_is_alt: native.primary_mod_is_alt,
        environment,
    })
}

/// Called on one blocking worker. A caller keeps only one mutation in flight.
/// The preferences callback reads the live shell after acquiring the shared lock.
pub fn save(
    id: &BindingId,
    values: &[String],
    preferences: &dyn Fn() -> Result<Preferences>,
) -> Result<Saved> {
    let _lock = mandara_config::compositor_theme::lock()?;
    let session = load()?;
    ensure!(
        session.scan.errors.is_empty(),
        "{}",
        t("Resolve the shortcut configuration errors before saving.")
    );
    session.selected(id)?;
    let edit = keybinds::prepare(session.target, id, values)?;
    if !edit.changed() {
        return Ok(Saved { undo: None });
    }
    check_destination(&session, &edit)?;
    let preferences_before = preferences()?;
    ensure!(
        preferences_before.target == session.target,
        "{}",
        t("The running compositor changed. Refresh the list and try again.")
    );
    let backend = native::Native::new(&session, &edit)?;
    if session.target == Target::KWin {
        return kwin::save(&session, &edit, &preferences_before, preferences, &backend);
    }
    if session.target == Target::Cosmic && edit.id.file != session.root {
        return cosmic::save(&session, &edit, &preferences_before, preferences, &backend);
    }
    transaction::apply(&session, &edit, &preferences_before, preferences, &backend)?;
    Ok(Saved {
        undo: edit.undo().map(Undo::Native),
    })
}

fn check_destination(session: &Session, edit: &keybinds::PreparedEdit) -> Result<()> {
    if session.target != Target::River {
        return Ok(());
    }
    let destination = keybinds::river::identity(edit.binding());
    for source in &session.scan.sources {
        for (index, row) in source.bindings.iter().enumerate() {
            if source.target == edit.id.target && index == edit.id.index {
                continue;
            }
            ensure!(
                destination.is_none() || destination != keybinds::river::identity(row),
                "{}",
                t(
                    "Another compositor shortcut already uses this key. Edit that shortcut or choose a different key."
                )
            );
        }
    }
    Ok(())
}

pub fn undo(edit: &Undo, preferences: &dyn Fn() -> Result<Preferences>) -> Result<Saved> {
    match edit {
        Undo::Native(edit) => save(&edit.id, &edit.values, preferences),
        Undo::Cosmic(edit) => {
            let _lock = mandara_config::compositor_theme::lock()?;
            let session = load()?;
            let initial = preferences()?;
            ensure!(
                session.target == Target::Cosmic && initial.target == Target::Cosmic,
                "{}",
                t("The running compositor changed. Refresh the list and try again.")
            );
            ensure!(
                session.scan.errors.is_empty(),
                "{}",
                t("Resolve the shortcut configuration errors before saving.")
            );
            cosmic::undo(&session, edit, &initial, preferences, &cosmic::Checker)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn river_rejects_destination_conflicts_before_any_file_or_live_write() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("init");
        let original = "riverctl map -layout 2 normal Super Q close\nriverctl map -repeat normal Super W close\n";
        std::fs::write(&file, original).unwrap();
        let session = Session {
            target: Target::River,
            root: file.clone(),
            scan: keybinds::scan_from(Target::River, &file),
            primary_mod_is_alt: false,
            environment: Default::default(),
        };
        let source = &session.scan.sources[0];
        let mut values: Vec<_> = source.bindings[0]
            .fields
            .iter()
            .map(|field| field.value.clone())
            .collect();
        let key = source.bindings[0]
            .fields
            .iter()
            .position(|field| field.label == "Key")
            .unwrap();
        values[key] = "w".into();
        let edit =
            keybinds::prepare(Target::River, &source.binding_id(0).unwrap(), &values).unwrap();
        assert!(
            check_destination(&session, &edit)
                .unwrap_err()
                .to_string()
                .contains("already uses")
        );
        assert_eq!(std::fs::read_to_string(file).unwrap(), original);
    }

    #[test]
    fn stale_identity_precedes_lock_reasons_and_current_locks_explain_the_real_cause() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("config.conf");
        std::fs::write(&file, "bind=SUPER,Q,killclient\n").unwrap();
        let mut session = Session {
            environment: Default::default(),
            target: Target::Mango,
            root: file.clone(),
            scan: keybinds::scan_from(Target::Mango, &file),
            primary_mod_is_alt: false,
        };
        let id = session.scan.sources[0].binding_id(0).unwrap();
        session.environment = mandara_config::compositor::IncludeEnvironment::captured([(
            "TOKEN".into(),
            "private-environment-value".into(),
        )]);
        assert!(
            !serde_json::to_string(&session)
                .unwrap()
                .contains("private-environment-value")
        );
        assert!(!format!("{:?}", session.environment).contains("private-environment-value"));
        assert!(session.selected(&id).is_ok());
        let reason = mandara_i18n::tk("Not supported by this compositor");
        session.scan.sources[0].bindings[0].read_only = Some(reason);
        assert_eq!(session.selected(&id).unwrap_err().to_string(), t(reason));
        let mut stale = id;
        stale.fingerprint = stale.fingerprint.wrapping_add(1);
        assert!(
            session
                .selected(&stale)
                .unwrap_err()
                .to_string()
                .contains("changed")
        );
    }
}
