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
    /// Last logical window size; compositor positions stay compositor-owned.
    pub window_size: Option<[f32; 2]>,
    /// Ids of collapsed sidebar groups ([`NavGroup::id`]).
    pub collapsed: BTreeSet<String>,
    /// The Applications page's last tab (`installed`, `defaults`, …).
    pub apps_tab: String,
    /// Themes → Looks and Designed themes as the compact list instead of
    /// the large preview cards (previews are the default). Replaces the
    /// `looks_previews` / `themes_previews` keys of 0.26, which almost
    /// every state file saved as off: they are ignored, so everyone starts
    /// with previews.
    pub looks_compact: bool,
    pub themes_compact: bool,
    /// Page sections you opened (`true`) or closed (`false`), by
    /// `Section/group`; others use their default (the first open).
    pub sections: BTreeMap<String, bool>,
}

impl gpui::Global for UiState {}

/// Reject corrupt geometry instead of passing NaNs or enormous surfaces to GPUI.
pub(crate) fn valid_size([width, height]: [f32; 2]) -> Option<[f32; 2]> {
    const MAX_LOGICAL_SIZE: f32 = 16_384.0;
    (width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0 && width <= MAX_LOGICAL_SIZE && height <= MAX_LOGICAL_SIZE).then_some([width, height])
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
    state.window_size = state.window_size.and_then(valid_size);
    state
}

pub fn load() -> UiState {
    load_from(&path())
}

pub fn save_to(path: &Path, state: &UiState) -> anyhow::Result<()> {
    let text = toml::to_string(state)?;
    mandara_config::store::write_atomic(path, text.as_bytes())
}

#[derive(Default)]
struct Writes {
    pending: Option<UiState>,
    running: bool,
}

static WRITES: (std::sync::Mutex<Writes>, std::sync::Condvar) = (std::sync::Mutex::new(Writes { pending: None, running: false }), std::sync::Condvar::new());

fn drain(writes: &(std::sync::Mutex<Writes>, std::sync::Condvar), mut write: impl FnMut(&UiState)) {
    loop {
        let state = {
            let mut pending = writes.0.lock().unwrap_or_else(|e| e.into_inner());
            match pending.pending.take() {
                Some(state) => state,
                None => {
                    pending.running = false;
                    writes.1.notify_all();
                    break;
                }
            }
        };
        // The writer owns no shared state while writing this snapshot.
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| write(&state))).is_err() {
            tracing::error!("Studio UI state writer panicked");
        }
    }
}

/// One writer and one replaceable snapshot, including during a resize drag.
pub fn save(state: &UiState) {
    let mut writes = WRITES.0.lock().unwrap_or_else(|e| e.into_inner());
    writes.pending = Some(state.clone());
    if writes.running { return; }
    writes.running = true;
    drop(writes);
    mandara_services::runtime::spawn("studio-ui-state", async {
        // Coalesce a resize burst before starting disk work; no timer survives a save.
        const SAVE_DELAY: std::time::Duration = std::time::Duration::from_millis(200);
        mandara_services::runtime::sleep(SAVE_DELAY).await;
        mandara_services::runtime::unblock(|| drain(&WRITES, |state| {
            if let Err(err) = save_to(&path(), state) {
                tracing::warn!("saving Studio UI state: {err:#}");
            }
        })).await;
    });
}

/// Called after GPUI stops, so the final resize is durable before process exit.
pub fn flush() {
    let mut writes = WRITES.0.lock().unwrap_or_else(|e| e.into_inner());
    while writes.running {
        writes = WRITES.1.wait(writes).unwrap_or_else(|e| e.into_inner());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panicking_snapshot_does_not_strand_the_writer_or_its_waiters() {
        let writes = (std::sync::Mutex::new(Writes { pending: Some(UiState::default()), running: true }), std::sync::Condvar::new());
        drain(&writes, |_| panic!("failed snapshot"));
        assert!(!writes.0.lock().unwrap().running);
        writes.0.lock().unwrap().pending = Some(UiState::default());
        writes.0.lock().unwrap().running = true;
        let mut count = 0;
        drain(&writes, |_| count += 1);
        assert_eq!(count, 1);
        assert!(!writes.0.lock().unwrap().running);
    }

    #[test]
    fn slow_disk_coalesces_resize_bursts_and_finishes_with_the_latest_complete_state() {
        use std::sync::{Arc, Condvar, Mutex, mpsc};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("studio-ui.toml");
        let first = UiState { window_size: Some([800.0, 600.0]), ..Default::default() };
        let writes = Arc::new((Mutex::new(Writes { pending: Some(first), running: true }), Condvar::new()));
        let (started_tx, started_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let worker = {
            let writes = writes.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                let mut count = 0;
                drain(&writes, |state| {
                    save_to(&path, state).unwrap();
                    count += 1;
                    if count == 1 { started_tx.send(()).unwrap(); resume_rx.recv().unwrap(); }
                });
                count
            })
        };
        started_rx.recv().unwrap();
        for width in 900..1000 {
            let mut state = UiState { window_size: Some([width as f32, 700.0]), ..Default::default() };
            state.apps_tab = "defaults".into();
            writes.0.lock().unwrap().pending = Some(state);
        }
        resume_tx.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), 2, "one active and one latest snapshot");
        assert!(!writes.0.lock().unwrap().running);
        let state = load_from(&path);
        assert_eq!(state.window_size, Some([999.0, 700.0]));
        assert_eq!(state.apps_tab, "defaults");
    }

    #[test]
    fn window_size_round_trips_and_rejects_invalid_geometry() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("studio-ui.toml");
        let state = UiState { window_size: Some([1320.0, 900.0]), ..Default::default() };
        save_to(&file, &state).unwrap();
        assert_eq!(load_from(&file), state);
        for size in [[f32::NAN, 500.0], [500.0, f32::INFINITY], [0.0, 500.0], [-1.0, 500.0], [f32::MAX, 500.0]] {
            assert_eq!(valid_size(size), None);
        }
        std::fs::write(&file, "window_size = [0.0, 500.0]\napps_tab = 'defaults'\n").unwrap();
        let state = load_from(&file);
        assert_eq!(state.window_size, None);
        assert_eq!(state.apps_tab, "defaults");
    }

    /// A state file from 0.26 (previews off) opens with previews on, and
    /// the compact choice round-trips.
    #[test]
    fn previews_are_the_default_even_for_an_old_state_file() {
        let dir = std::env::temp_dir().join(format!("mandara-ui-state-previews-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("studio-ui.toml");
        std::fs::write(&file, "looks_previews = false\nthemes_previews = false\napps_tab = \"defaults\"\n").unwrap();
        let old = load_from(&file);
        assert!(!old.looks_compact && !old.themes_compact, "previews on");
        assert_eq!(old.apps_tab, "defaults", "the rest of the file still loads");
        let mut state = old;
        state.looks_compact = true;
        save_to(&file, &state).unwrap();
        let back = load_from(&file);
        assert!(back.looks_compact && !back.themes_compact);
        assert!(!std::fs::read_to_string(&file).unwrap().contains("looks_previews"), "the old key is gone once saved");
        let _ = std::fs::remove_dir_all(&dir);
    }

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
