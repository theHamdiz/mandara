//! Short-lived native shortcut scans and reversible, field-scoped saves.

mod cosmic;
mod kwin;
mod native;
mod transaction;

use std::path::PathBuf;

use anyhow::{Result, ensure};
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
    let scan = keybinds::scan_from(target, &root);
    Ok(Session {
        target,
        root,
        scan,
        primary_mod_is_alt: native.primary_mod_is_alt,
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
    ensure!(
        session
            .scan
            .sources
            .iter()
            .find(|source| source.file == id.file)
            .and_then(|source| source.bindings.get(id.index))
            .is_some_and(|row| row.read_only.is_none()),
        "{}",
        t("Computed shortcuts must be edited in their source configuration.")
    );
    ensure!(
        session
            .scan
            .sources
            .iter()
            .any(|source| source.binding_id(id.index).as_ref() == Some(id)),
        "{}",
        t(
            "The compositor shortcuts changed while they were being checked. Refresh the list and try again."
        )
    );
    let edit = keybinds::prepare(session.target, id, values)?;
    if !edit.changed() {
        return Ok(Saved { undo: None });
    }
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
