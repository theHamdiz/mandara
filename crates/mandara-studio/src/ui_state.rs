//! Studio's own UI state (which sidebar groups are collapsed, the last
//! Applications tab), remembered
//! per user in `$XDG_STATE_HOME/mandara/studio-ui.toml`: never in
//! `config.toml`, so it isn't synced to the shell, exported with profiles
//! or shown as a setting.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::registry::NavGroup;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    /// Ids of collapsed sidebar groups ([`NavGroup::id`]).
    pub collapsed: BTreeSet<String>,
    /// The Applications page's last tab (`installed`, `defaults`, …).
    pub apps_tab: String,
}

impl UiState {
    pub fn is_collapsed(&self, group: NavGroup) -> bool {
        self.collapsed.contains(group.id())
    }

    /// Collapses or expands `group`; returns its new collapsed state.
    pub fn toggle(&mut self, group: NavGroup) -> bool {
        if !self.collapsed.remove(group.id()) {
            self.collapsed.insert(group.id().to_string());
        }
        self.is_collapsed(group)
    }

    pub fn set_collapsed(&mut self, group: NavGroup, collapsed: bool) {
        if collapsed {
            self.collapsed.insert(group.id().to_string());
        } else {
            self.collapsed.remove(group.id());
        }
    }
}

/// `$XDG_STATE_HOME/mandara/studio-ui.toml` (`~/.local/state/…`).
pub fn path() -> PathBuf {
    dirs::state_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/state")).join("mandara").join("studio-ui.toml")
}

/// The saved state; defaults when the file is missing or unreadable.
pub fn load_from(path: &Path) -> UiState {
    let mut state: UiState = std::fs::read_to_string(path).ok().and_then(|text| toml::from_str(&text).ok()).unwrap_or_default();
    // Forget groups that no longer exist.
    state.collapsed.retain(|id| NavGroup::parse(id).is_some());
    state
}

pub fn load() -> UiState {
    load_from(&path())
}

pub fn save_to(path: &Path, state: &UiState) -> anyhow::Result<()> {
    let text = toml::to_string(state)?;
    mandara_config::store::write_atomic(path, text.as_bytes())
}

/// Saves off the UI thread (a tiny file, but never block a frame on disk).
pub fn save(state: &UiState) {
    let state = state.clone();
    std::thread::spawn(move || {
        if let Err(err) = save_to(&path(), &state) {
            tracing::warn!("saving Studio UI state: {err:#}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapsed_groups_round_trip_and_unknown_ids_drop() {
        let dir = std::env::temp_dir().join(format!("mandara-studio-ui-{}", std::process::id()));
        let file = dir.join("studio-ui.toml");
        let mut state = UiState::default();
        assert!(state.toggle(NavGroup::System));
        state.set_collapsed(NavGroup::Extend, true);
        save_to(&file, &state).unwrap();
        assert_eq!(load_from(&file), state);
        assert!(!state.toggle(NavGroup::System));
        std::fs::write(&file, "collapsed = [\"desktop\", \"gone\"]\n").unwrap();
        let loaded = load_from(&file);
        assert!(loaded.is_collapsed(NavGroup::Desktop) && loaded.collapsed.len() == 1);
        assert_eq!(load_from(&dir.join("missing.toml")), UiState::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
