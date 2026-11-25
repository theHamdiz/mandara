//! Sidebar tabs: user-made tabs of plugin cards (`[[sidebar.tabs]]`), the
//! editing operations Studio, the shell and `mandara msg sidebar tab …`
//! share, and the remembered last tab
//! (`$XDG_STATE_HOME/mandara/sidebar.toml`, state — never the config).
//!
//! Configs written before tabs existed had one card list,
//! `[sidebar] cards = [...]`; it reads as the first tab, "Overview", and the
//! next save writes the `tabs` shape (see `SidebarConfigFile`).

use std::path::{Path, PathBuf};

use mandara_i18n::tk;
use serde::{Deserialize, Serialize};

use crate::SidebarConfig;

/// Id of the first tab of a migrated or default config.
pub const OVERVIEW_ID: &str = "overview";
/// Its English name (shown translated while the user hasn't renamed it).
pub const OVERVIEW_NAME: &str = "Overview";
pub const OVERVIEW_ICON: &str = "grid";
/// Icon of a new tab when none is picked.
pub const DEFAULT_TAB_ICON: &str = "tiles";
/// The built-in notifications tab, as remembered in the state file and
/// accepted by `sidebar tab open`. Never a user tab's id.
pub const NOTIFICATIONS_TAB: &str = "notifications";
/// Longest tab name kept (characters).
pub const MAX_TAB_NAME_CHARS: usize = 32;
/// Id stem when a name has no usable characters.
const ID_FALLBACK: &str = "tab";

/// One sidebar tab: a named, iconed list of plugin cards, top to bottom.
/// A plugin may sit in several tabs; every copy is the same live instance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarTab {
    /// Stable id (state file, commands); derived from the first name.
    pub id: String,
    pub name: String,
    /// A Mandara icon name or a theme icon (`mandara_ui::icons::any_icon`).
    pub icon: String,
    /// Plugin ids whose sidebar cards the tab shows.
    pub cards: Vec<String>,
}

impl Default for SidebarTab {
    fn default() -> Self {
        Self { id: String::new(), name: String::new(), icon: DEFAULT_TAB_ICON.into(), cards: Vec::new() }
    }
}

impl SidebarTab {
    /// The first tab of a default or migrated config.
    pub fn overview(cards: Vec<String>) -> Self {
        Self { id: OVERVIEW_ID.into(), name: OVERVIEW_NAME.into(), icon: OVERVIEW_ICON.into(), cards }
    }

    /// Whether the name is still the shipped "Overview" (shown translated).
    pub fn has_default_name(&self) -> bool {
        self.id == OVERVIEW_ID && self.name == OVERVIEW_NAME
    }
}

/// Why a tab edit was refused (messages are i18n keys: `mandara_i18n::tr`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TabError {
    NoSuchTab,
    EmptyName,
    LastTab,
    AlreadyThere,
    NotThere,
}

impl TabError {
    pub fn message(self) -> &'static str {
        match self {
            TabError::NoSuchTab => tk("No such sidebar tab (use its name, id or 1-based position)"),
            TabError::EmptyName => tk("A tab needs a name"),
            TabError::LastTab => tk("The sidebar keeps at least one tab"),
            TabError::AlreadyThere => tk("That widget is already on this tab"),
            TabError::NotThere => tk("That widget is not on this tab"),
        }
    }
}

impl std::fmt::Display for TabError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(mandara_i18n::t(self.message()))
    }
}

/// The tab a sidebar opens on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OpenTab {
    /// Index into [`SidebarConfig::tabs`].
    Tab(usize),
    Notifications,
}

/// `name` trimmed, single-line and capped; `None` when nothing is left.
fn clean_name(name: &str) -> Option<String> {
    let flat: String = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let capped: String = flat.chars().take(MAX_TAB_NAME_CHARS).collect();
    (!capped.is_empty()).then_some(capped)
}

/// Lowercase ASCII slug of `name` (`"Side project!"` → `side-project`).
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { ID_FALLBACK.into() } else { out }
}

impl SidebarConfig {
    /// The tab `key` names: its id, its 1-based position, or its name
    /// (case-insensitive).
    pub fn tab_index(&self, key: &str) -> Option<usize> {
        let key = key.trim();
        if let Some(i) = self.tabs.iter().position(|t| t.id == key) {
            return Some(i);
        }
        if let Ok(n) = key.parse::<usize>() {
            return (1..=self.tabs.len()).contains(&n).then(|| n - 1);
        }
        let lower = key.to_lowercase();
        self.tabs.iter().position(|t| t.name.to_lowercase() == lower)
    }

    fn tab_mut(&mut self, i: usize) -> Result<&mut SidebarTab, TabError> {
        self.tabs.get_mut(i).ok_or(TabError::NoSuchTab)
    }

    /// An id no tab uses yet, derived from `name`.
    fn free_id(&self, name: &str) -> String {
        let stem = slug(name);
        let taken = |id: &str| id == NOTIFICATIONS_TAB || self.tabs.iter().any(|t| t.id == id);
        if !taken(&stem) {
            return stem;
        }
        (2..).map(|n| format!("{stem}-{n}")).find(|id| !taken(id)).unwrap_or(stem)
    }

    /// `base`, or `base 2`, `base 3`… — the first name no tab has.
    pub fn free_tab_name(&self, base: &str) -> String {
        let taken = |n: &str| self.tabs.iter().any(|t| t.name.eq_ignore_ascii_case(n));
        if !taken(base) {
            return base.to_string();
        }
        (2..).map(|n| format!("{base} {n}")).find(|n| !taken(n)).unwrap_or_else(|| base.to_string())
    }

    /// Appends a tab; returns its index.
    pub fn add_tab(&mut self, name: &str, icon: Option<&str>) -> Result<usize, TabError> {
        let name = clean_name(name).ok_or(TabError::EmptyName)?;
        let id = self.free_id(&name);
        let icon = icon.map(str::trim).filter(|i| !i.is_empty()).unwrap_or(DEFAULT_TAB_ICON).to_string();
        self.tabs.push(SidebarTab { id, name, icon, cards: Vec::new() });
        Ok(self.tabs.len() - 1)
    }

    pub fn rename_tab(&mut self, i: usize, name: &str) -> Result<(), TabError> {
        let name = clean_name(name).ok_or(TabError::EmptyName)?;
        self.tab_mut(i)?.name = name;
        Ok(())
    }

    pub fn set_tab_icon(&mut self, i: usize, icon: &str) -> Result<(), TabError> {
        let icon = icon.trim();
        self.tab_mut(i)?.icon = if icon.is_empty() { DEFAULT_TAB_ICON.into() } else { icon.into() };
        Ok(())
    }

    /// Deletes a tab (its widgets stay on in any other tab that has them).
    pub fn remove_tab(&mut self, i: usize) -> Result<SidebarTab, TabError> {
        if i >= self.tabs.len() {
            return Err(TabError::NoSuchTab);
        }
        if self.tabs.len() == 1 {
            return Err(TabError::LastTab);
        }
        Ok(self.tabs.remove(i))
    }

    /// Moves tab `from` to position `to` (clamped).
    pub fn move_tab(&mut self, from: usize, to: usize) -> Result<usize, TabError> {
        if from >= self.tabs.len() {
            return Err(TabError::NoSuchTab);
        }
        let tab = self.tabs.remove(from);
        let to = to.min(self.tabs.len());
        self.tabs.insert(to, tab);
        Ok(to)
    }

    /// Puts `plugin` on tab `tab` at `index` (end when `None` or past it).
    pub fn add_card(&mut self, tab: usize, plugin: &str, index: Option<usize>) -> Result<(), TabError> {
        let t = self.tab_mut(tab)?;
        if t.cards.iter().any(|c| c == plugin) {
            return Err(TabError::AlreadyThere);
        }
        let i = index.unwrap_or(t.cards.len()).min(t.cards.len());
        t.cards.insert(i, plugin.to_string());
        Ok(())
    }

    pub fn remove_card(&mut self, tab: usize, plugin: &str) -> Result<(), TabError> {
        let t = self.tab_mut(tab)?;
        let before = t.cards.len();
        t.cards.retain(|c| c != plugin);
        if t.cards.len() == before { Err(TabError::NotThere) } else { Ok(()) }
    }

    /// Moves `plugin` from tab `from` to tab `to` at `index` (a reorder
    /// when both are the same tab). A copy already on `to` is replaced.
    pub fn move_card(&mut self, from: usize, to: usize, plugin: &str, index: Option<usize>) -> Result<(), TabError> {
        if to >= self.tabs.len() {
            return Err(TabError::NoSuchTab);
        }
        let old = self.tabs.get(from).ok_or(TabError::NoSuchTab)?.cards.iter().position(|c| c == plugin).ok_or(TabError::NotThere)?;
        let mut index = index;
        // Indices name the target as it is now: removing the card first
        // shifts the ones after it in a same-tab reorder.
        if from == to
            && let Some(i) = index.as_mut()
            && old < *i
        {
            *i -= 1;
        }
        self.tabs[from].cards.remove(old);
        self.tabs[to].cards.retain(|c| c != plugin);
        let cards = &mut self.tabs[to].cards;
        let i = index.unwrap_or(cards.len()).min(cards.len());
        cards.insert(i, plugin.to_string());
        Ok(())
    }

    /// Whether any tab shows `plugin`'s card.
    pub fn has_card(&self, plugin: &str) -> bool {
        self.tabs.iter().any(|t| t.cards.iter().any(|c| c == plugin))
    }

    /// Repairs a hand-edited list: at least one tab, unique non-empty ids
    /// (never the notifications tab's), named tabs, no card twice in a tab.
    pub fn normalize_tabs(&mut self) {
        if self.tabs.is_empty() {
            self.tabs.push(SidebarTab::overview(Vec::new()));
        }
        let tabs = std::mem::take(&mut self.tabs);
        for mut tab in tabs {
            tab.name = clean_name(&tab.name).unwrap_or_else(|| if tab.id.is_empty() { OVERVIEW_NAME.into() } else { tab.id.clone() });
            let id = slug(if tab.id.is_empty() { &tab.name } else { &tab.id });
            tab.id = if id == NOTIFICATIONS_TAB || self.tabs.iter().any(|t| t.id == id) { self.free_id(&id) } else { id };
            if tab.icon.trim().is_empty() {
                tab.icon = DEFAULT_TAB_ICON.into();
            }
            let mut seen = Vec::with_capacity(tab.cards.len());
            tab.cards.retain(|c| {
                let fresh = !seen.contains(c);
                seen.push(c.clone());
                fresh
            });
            self.tabs.push(tab);
        }
    }

    /// The tab an opening sidebar shows: the first one when
    /// `always_first_tab` is on, else the remembered `last` (a tab id or
    /// [`NOTIFICATIONS_TAB`]) while it still exists.
    pub fn opening_tab(&self, last: &str, notifications_tab: bool) -> OpenTab {
        if self.always_first_tab {
            return OpenTab::Tab(0);
        }
        if last == NOTIFICATIONS_TAB && notifications_tab {
            return OpenTab::Notifications;
        }
        OpenTab::Tab(self.tabs.iter().position(|t| t.id == last).unwrap_or(0))
    }
}

/// Per-user sidebar state kept across opens and restarts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarState {
    /// Id of the tab shown last (or [`NOTIFICATIONS_TAB`]).
    pub last_tab: String,
}

/// `$XDG_STATE_HOME/mandara/sidebar.toml` (`~/.local/state/…`).
pub fn state_path() -> PathBuf {
    crate::paths::state_dir().join("sidebar.toml")
}

impl SidebarState {
    /// The saved state at `path` (defaults when missing or unreadable).
    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::write_atomic(path, toml::to_string(self)?.as_bytes())
    }

    pub fn load() -> Self {
        Self::load_from(&state_path())
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&state_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ShellConfig;

    fn names(c: &SidebarConfig) -> Vec<&str> {
        c.tabs.iter().map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn old_cards_list_becomes_the_overview_tab_and_saves_as_tabs() {
        let old = "[sidebar]\nwidth = 380.0\ncards = [\"media\", \"todo\"]\nnotifications_tab = true\n";
        let cfg: ShellConfig = toml::from_str(old).unwrap();
        assert_eq!(cfg.sidebar.tabs, vec![SidebarTab::overview(vec!["media".into(), "todo".into()])]);
        let written = toml::to_string(&cfg).unwrap();
        assert!(written.contains("[[sidebar.tabs]]"), "{written}");
        let table: toml::Table = toml::from_str(&written).unwrap();
        assert!(table["sidebar"].get("cards").is_none(), "the old list is not written back: {written}");
        let again: ShellConfig = toml::from_str(&written).unwrap();
        assert_eq!(again.sidebar, cfg.sidebar);
        // No sidebar table at all: the shipped Overview.
        let fresh: ShellConfig = toml::from_str("").unwrap();
        assert_eq!(fresh.sidebar.tabs, SidebarConfig::default().tabs);
        assert_eq!(fresh.sidebar.tabs[0].id, OVERVIEW_ID);
        // `tabs` wins over a stale `cards` left in the file.
        let both: ShellConfig = toml::from_str("[sidebar]\ncards = [\"x\"]\n[[sidebar.tabs]]\nid = \"work\"\nname = \"Work\"\ncards = [\"todo\"]\n").unwrap();
        assert_eq!(names(&both.sidebar), ["Work"]);
    }

    #[test]
    fn tab_crud_and_ordering() {
        let mut c = SidebarConfig::default();
        let work = c.add_tab("  Work   stuff ", Some("folder")).unwrap();
        assert_eq!(c.tabs[work].id, "work-stuff");
        assert_eq!(c.tabs[work].name, "Work stuff");
        assert_eq!(c.tabs[work].icon, "folder");
        let dup = c.add_tab("Work stuff", None).unwrap();
        assert_eq!(c.tabs[dup].id, "work-stuff-2", "ids stay unique");
        assert_eq!(c.tabs[dup].icon, DEFAULT_TAB_ICON);
        assert_eq!(c.free_tab_name("Work stuff"), "Work stuff 2");
        assert_eq!(c.add_tab("   ", None), Err(TabError::EmptyName));
        // A tab is never called `notifications` internally.
        let n = c.add_tab("Notifications", None).unwrap();
        assert_eq!(c.tabs[n].id, "notifications-2");
        c.rename_tab(dup, "Side project").unwrap();
        assert_eq!(c.tab_index("side project"), Some(dup));
        assert_eq!(c.tab_index("2"), Some(1));
        assert_eq!(c.tab_index("work-stuff"), Some(work));
        assert_eq!(c.tab_index("0"), None);
        assert_eq!(c.move_tab(dup, 0), Ok(0));
        assert_eq!(names(&c), ["Side project", "Overview", "Work stuff", "Notifications"]);
        assert_eq!(c.move_tab(0, 99), Ok(3));
        assert_eq!(names(&c), ["Overview", "Work stuff", "Notifications", "Side project"]);
        c.remove_tab(2).unwrap();
        c.remove_tab(2).unwrap();
        c.remove_tab(1).unwrap();
        assert_eq!(c.remove_tab(0), Err(TabError::LastTab));
        assert_eq!(c.remove_tab(5), Err(TabError::NoSuchTab));
        let long = "x".repeat(MAX_TAB_NAME_CHARS * 2);
        let i = c.add_tab(&long, None).unwrap();
        assert_eq!(c.tabs[i].name.chars().count(), MAX_TAB_NAME_CHARS);
    }

    #[test]
    fn widgets_are_added_moved_and_removed() {
        let mut c = SidebarConfig::default();
        c.tabs[0].cards = vec!["a".into(), "b".into(), "c".into()];
        let t = c.add_tab("Focus", None).unwrap();
        c.add_card(t, "todo", None).unwrap();
        // The same widget may live in several tabs.
        c.add_card(t, "a", Some(0)).unwrap();
        assert_eq!(c.tabs[t].cards, ["a", "todo"]);
        assert_eq!(c.add_card(t, "a", None), Err(TabError::AlreadyThere));
        // Reorder within a tab: indices name the list before the move.
        c.move_card(0, 0, "a", Some(3)).unwrap();
        assert_eq!(c.tabs[0].cards, ["b", "c", "a"]);
        c.move_card(0, 0, "a", Some(0)).unwrap();
        assert_eq!(c.tabs[0].cards, ["a", "b", "c"]);
        // Move across tabs; a copy already there is not duplicated.
        c.move_card(0, t, "a", Some(1)).unwrap();
        assert_eq!(c.tabs[0].cards, ["b", "c"]);
        assert_eq!(c.tabs[t].cards, ["todo", "a"]);
        c.move_card(0, t, "b", None).unwrap();
        assert_eq!(c.tabs[t].cards, ["todo", "a", "b"]);
        assert_eq!(c.move_card(0, t, "zzz", None), Err(TabError::NotThere));
        c.remove_card(t, "todo").unwrap();
        assert_eq!(c.remove_card(t, "todo"), Err(TabError::NotThere));
        assert!(c.has_card("a") && !c.has_card("todo"));
    }

    #[test]
    fn plugin_places_span_every_tab() {
        let mut cfg = ShellConfig::default();
        let t = cfg.sidebar.add_tab("Music", None).unwrap();
        cfg.sidebar.add_card(t, "media", Some(0)).unwrap();
        let places = cfg.take_plugin_places("media");
        assert!(!cfg.sidebar.has_card("media"), "removing a plugin clears it from every tab");
        cfg.put_plugin_places("media", &places);
        assert_eq!(cfg.sidebar.tabs[t].cards, ["media"]);
        assert!(cfg.sidebar.tabs[0].cards.iter().any(|c| c == "media"));
        assert_eq!(cfg.sidebar.tabs[0].cards, SidebarConfig::default().tabs[0].cards);
    }

    #[test]
    fn normalize_repairs_hand_edits() {
        let mut c = SidebarConfig { tabs: Vec::new(), ..Default::default() };
        c.normalize_tabs();
        assert_eq!(c.tabs.len(), 1);
        c.tabs = vec![
            SidebarTab { id: "x".into(), name: "X".into(), icon: String::new(), cards: vec!["a".into(), "a".into()] },
            SidebarTab { id: "x".into(), name: "".into(), ..Default::default() },
            SidebarTab { id: NOTIFICATIONS_TAB.into(), name: "N".into(), ..Default::default() },
        ];
        c.normalize_tabs();
        assert_eq!(c.tabs[0].cards, ["a"]);
        assert_eq!(c.tabs[0].icon, DEFAULT_TAB_ICON);
        assert_ne!(c.tabs[1].id, "x");
        assert!(!c.tabs[1].name.is_empty());
        assert_ne!(c.tabs[2].id, NOTIFICATIONS_TAB);
    }

    #[test]
    fn last_tab_is_remembered_in_the_state_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mandara/sidebar.toml");
        assert_eq!(SidebarState::load_from(&path), SidebarState::default());
        let mut c = SidebarConfig::default();
        let t = c.add_tab("Work", None).unwrap();
        SidebarState { last_tab: c.tabs[t].id.clone() }.save_to(&path).unwrap();
        let state = SidebarState::load_from(&path);
        assert_eq!(c.opening_tab(&state.last_tab, true), OpenTab::Tab(t));
        assert_eq!(c.opening_tab(NOTIFICATIONS_TAB, true), OpenTab::Notifications);
        assert_eq!(c.opening_tab(NOTIFICATIONS_TAB, false), OpenTab::Tab(0), "tab turned off");
        assert_eq!(c.opening_tab("deleted", true), OpenTab::Tab(0));
        c.always_first_tab = true;
        assert_eq!(c.opening_tab(&state.last_tab, true), OpenTab::Tab(0));
        std::fs::write(&path, "not = [toml").unwrap();
        assert_eq!(SidebarState::load_from(&path), SidebarState::default());
    }
}
