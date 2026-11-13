//! Short-lived native shortcut scans and reversible, field-scoped saves.

mod native;
mod transaction;

use std::path::PathBuf;

use anyhow::{Result, ensure};
use mandara_config::{
    compositor::{Keybind, Target},
    keybinds::{self, BindingId, Scan, UndoEdit},
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
}

#[derive(serde::Serialize)]
pub struct Saved {
    pub undo: Option<UndoEdit>,
}

pub fn load() -> Result<Session> {
    let target = mandara_compositor::detected()
        .map(mandara_compositor::CompositorKind::target)
        .ok_or_else(|| anyhow::anyhow!("{}", t("No supported compositor is running.")))?;
    let root = mandara_compositor::native_config::running(target)?.unwrap_or_else(|| keybinds::default_root(target));
    let scan = keybinds::scan_from(target, &root);
    Ok(Session { target, root, scan })
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
    transaction::apply(&session, &edit, &preferences_before, preferences, &backend)?;
    Ok(Saved { undo: edit.undo() })
}

pub fn undo(edit: &UndoEdit, preferences: &dyn Fn() -> Result<Preferences>) -> Result<Saved> {
    save(&edit.id, &edit.values, preferences)
}
