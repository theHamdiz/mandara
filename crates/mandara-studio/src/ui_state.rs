//! Studio's own UI state (which sidebar groups are collapsed, the last
//! Applications tab), remembered
//! per user in `$XDG_STATE_HOME/mandara/studio-ui.toml`: never in
//! `config.toml`, so it isn't synced to the shell, exported with profiles
//! or shown as a setting.

use std::collections::{BTreeMap, BTreeSet};
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
    /// Themes → Looks and Designed themes as large preview cards instead of
    /// the compact list.
    pub looks_previews: bool,
    pub themes_previews: bool,
    /// Page sections you opened (`true`) or closed (`false`), by
    /// `Section/group`; others use their default (the first open).
    pub sections: BTreeMap<String, bool>,
}

impl UiState {
    /// New tab keys fall back to the expansion saved for their former tab
    /// (the Side bars and Sidebar tabs merged into Sidebars), then for their
    /// former page. An explicit new key wins. Reading a legacy key does not
    /// rewrite it, and missing entries keep the caller's default expansion
    /// behavior.
    pub(crate) fn fold_state(&self, key: &str) -> Option<bool> {
        self.sections.get(key).copied().or_else(|| {
            let (tab, group) = key.strip_prefix("Bars/")?.split_once('/')?;
            let (former_tab, former_page) = crate::registry::bars::BarsTab::parse(tab)?.former_keys(group);
            former_tab.and_then(|k| self.sections.get(&k).copied()).or_else(|| self.sections.get(&former_page).copied())
        })
    }

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
    fn bars_folds_keep_former_pages_distinct_and_new_keys_take_precedence() {
        let mut state = UiState::default();
        state.sections.insert("Bar/Layout".into(), false);
        state.sections.insert("Sidebar/Layout".into(), true);
        assert_eq!(state.fold_state("Bars/bar/Layout"), Some(false));
        assert_eq!(state.fold_state("Bars/sidebars/Layout"), Some(true), "the sidebar's Layout, not the bar's");
        state.sections.insert("Bars/sidebars/Layout".into(), false);
        assert_eq!(state.fold_state("Bars/sidebars/Layout"), Some(false));
        assert_eq!(state.fold_state("Bars/edges/Hot corners"), None);
        assert_eq!(state.fold_state("Themes/Mode"), None);
    }

    #[test]
    fn folds_saved_on_the_two_former_tabs_carry_over_to_sidebars() {
        use crate::registry::side_bar_settings::{SIDE_BAR_ITEMS, SIDE_BAR_SHAPE};
        let mut state = UiState::default();
        // Saved on the former tabs, and on the pages before them.
        state.sections.insert(format!("Bars/side-bars/{SIDE_BAR_SHAPE}"), true);
        state.sections.insert(format!("Bar/{SIDE_BAR_SHAPE}"), false);
        state.sections.insert(format!("Bar/{SIDE_BAR_ITEMS}"), true);
        state.sections.insert("Bars/sidebar/Tabs".into(), true);
        state.sections.insert("Sidebar/Tabs".into(), false);
        state.sections.insert("Sidebar/Motion".into(), true);
        state.sections.insert("Bar/Motion".into(), false);
        assert_eq!(state.fold_state(&format!("Bars/sidebars/{SIDE_BAR_SHAPE}")), Some(true), "the former tab beats the former page");
        assert_eq!(state.fold_state(&format!("Bars/sidebars/{SIDE_BAR_ITEMS}")), Some(true), "side bar groups were on the Bar page");
        assert_eq!(state.fold_state("Bars/sidebars/Tabs"), Some(true));
        assert_eq!(state.fold_state("Bars/sidebars/Motion"), Some(true), "panel groups were on the Sidebar page");
        assert_eq!(state.fold_state("Bars/sidebars/Behaviour"), None);
        // Keys under the former tab slugs resolve the same way.
        assert_eq!(state.fold_state(&format!("Bars/side-bars/{SIDE_BAR_ITEMS}")), Some(true));
    }

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
