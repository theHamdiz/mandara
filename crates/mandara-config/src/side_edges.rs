//! What each side edge of the screen holds: nothing, a side bar (a slim bar
//! of plugin items, `[side_bars]`) or the sidebar (the slide-out panel of
//! tabs and cards, `[sidebar]`). One model behind Studio's Sidebars picker,
//! so a choice for one edge never leaves the two fighting over it.
//!
//! The rules it keeps: an edge holds one of them; there is one sidebar, so
//! choosing it for an edge moves it there; a side bar never shares the main
//! bar's own edge (the sidebar may: it slides out beside a vertical bar);
//! and a bar preset's rail (the left side bar some presets join to the
//! bar) changes only with its preset.

use crate::bar_styles::BarStyle;
use crate::{BarPosition, ShellConfig, SidebarSide};

/// What a side edge holds.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum EdgeUse {
    Nothing,
    /// A slim bar of plugin items down the edge.
    SideBar,
    /// The slide-out panel with tabs and cards.
    Sidebar,
}

impl EdgeUse {
    /// Picker order.
    pub const ALL: [Self; 3] = [Self::Nothing, Self::SideBar, Self::Sidebar];

    /// Stable name for routes and commands.
    pub fn id(self) -> &'static str {
        match self {
            Self::Nothing => "nothing",
            Self::SideBar => "side-bar",
            Self::Sidebar => "sidebar",
        }
    }
}

/// Why an edge can't take a choice.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EdgeBlock {
    /// The main bar stands on this edge; a side bar can't share it.
    BarHere,
    /// This edge's side bar is the rail of the bar preset; it changes with
    /// the preset.
    Rail(BarStyle),
}

impl ShellConfig {
    /// What the left (`false`) or right edge holds as drawn. A sidebar a
    /// side bar has taken the edge from counts as the side bar.
    pub fn edge_use(&self, right: bool) -> EdgeUse {
        if self.side_bar_on(right) {
            EdgeUse::SideBar
        } else if self.sidebar.enabled && self.sidebar_on_right() == right {
            EdgeUse::Sidebar
        } else {
            EdgeUse::Nothing
        }
    }

    /// The bar preset whose rail is this edge's side bar: only the left
    /// one, while it shows.
    pub fn edge_rail(&self, right: bool) -> Option<BarStyle> {
        self.bar.style.filter(|style| !right && style.rail().is_some() && self.side_bar_on(false))
    }

    /// Whether the edge can take `choice` now, and if not, why.
    pub fn edge_allows(&self, right: bool, choice: EdgeUse) -> Result<(), EdgeBlock> {
        if let Some(style) = self.edge_rail(right) {
            return if choice == EdgeUse::SideBar { Ok(()) } else { Err(EdgeBlock::Rail(style)) };
        }
        let edge = if right { BarPosition::Right } else { BarPosition::Left };
        if choice == EdgeUse::SideBar && self.bar.position == edge {
            return Err(EdgeBlock::BarHere);
        }
        Ok(())
    }

    /// Gives the edge `choice` and only changes what that takes: the side
    /// bar or sidebar it replaces goes off, a sidebar on the other edge
    /// moves here, and the other edge keeps its side bar. A sidebar hidden
    /// behind a side bar is turned off, so nothing comes back unasked.
    /// Every other setting of either is kept.
    pub fn set_edge_use(&mut self, right: bool, choice: EdgeUse) -> Result<(), EdgeBlock> {
        self.edge_allows(right, choice)?;
        if self.sidebar.enabled && !self.sidebar_active() {
            self.sidebar.enabled = false;
        }
        let sidebar_here = self.sidebar.enabled && self.sidebar_on_right() == right;
        match choice {
            EdgeUse::Nothing => {
                self.side_bar_off(right);
                if sidebar_here {
                    self.sidebar.enabled = false;
                }
            }
            EdgeUse::SideBar => {
                if sidebar_here {
                    self.sidebar.enabled = false;
                }
                // A side bar the other edge doesn't show stays hidden, even
                // once the main bar leaves that edge.
                if !self.side_bar_on(!right) {
                    self.side_bars.side_mut(!right).enabled = false;
                }
                self.side_bars.enabled = true;
                self.side_bars.side_mut(right).enabled = true;
            }
            EdgeUse::Sidebar => {
                self.side_bar_off(right);
                self.sidebar.enabled = true;
                // Auto stays while it already lands here.
                if self.sidebar_on_right() != right {
                    self.sidebar.side = if right { SidebarSide::Right } else { SidebarSide::Left };
                }
            }
        }
        Ok(())
    }

    /// Turns this edge's side bar off; with neither showing, side bars go
    /// off.
    fn side_bar_off(&mut self, right: bool) {
        if self.side_bar_on(right) {
            self.side_bars.side_mut(right).enabled = false;
        }
        if !self.side_bar_on(!right) {
            self.side_bars.enabled = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Configurations covering every starting point: side bars off, one or
    /// both on, the sidebar on either edge or off (Auto included), hidden
    /// behind a side bar, and the main bar standing on a side edge.
    fn starts() -> Vec<ShellConfig> {
        let mut out = Vec::new();
        for side_bars in [false, true] {
            for (left, right) in [(false, false), (true, false), (false, true), (true, true)] {
                for sidebar in [None, Some(SidebarSide::Auto), Some(SidebarSide::Left), Some(SidebarSide::Right)] {
                    for position in [BarPosition::Top, BarPosition::Left, BarPosition::Right] {
                        let mut cfg = ShellConfig::default();
                        cfg.side_bars.enabled = side_bars;
                        cfg.side_bars.left.enabled = left;
                        cfg.side_bars.right.enabled = right;
                        cfg.sidebar.enabled = sidebar.is_some();
                        cfg.sidebar.side = sidebar.unwrap_or_default();
                        cfg.bar.position = position;
                        out.push(cfg);
                    }
                }
            }
        }
        out
    }

    #[test]
    fn every_allowed_choice_lands_and_leaves_no_conflict() {
        for start in starts() {
            for right in [false, true] {
                for choice in EdgeUse::ALL {
                    let mut cfg = start.clone();
                    let before_other = start.edge_use(!right);
                    let result = cfg.set_edge_use(right, choice);
                    assert_eq!(result, start.edge_allows(right, choice), "{choice:?} on right={right}");
                    if result.is_err() {
                        assert_eq!(cfg, start, "a refused choice changes nothing");
                        continue;
                    }
                    assert_eq!(cfg.edge_use(right), choice, "{choice:?} on right={right} from {:?}/{:?}", start.edge_use(false), start.edge_use(true));
                    // The other edge keeps what it showed, unless the one
                    // sidebar moved from there.
                    let other = cfg.edge_use(!right);
                    let moved = choice == EdgeUse::Sidebar && before_other == EdgeUse::Sidebar;
                    assert_eq!(other, if moved { EdgeUse::Nothing } else { before_other });
                    // No sidebar is left hidden behind a side bar.
                    assert!(!cfg.sidebar.enabled || cfg.sidebar_active());
                    assert_eq!(cfg.sidebar_active(), [false, true].into_iter().any(|r| cfg.edge_use(r) == EdgeUse::Sidebar));
                    // Side bars are on exactly while one shows, and no
                    // side bar waits hidden to reappear on its own.
                    assert_eq!(cfg.side_bars.enabled, [false, true].into_iter().any(|r| cfg.edge_use(r) == EdgeUse::SideBar));
                    if choice == EdgeUse::SideBar {
                        assert!(cfg.side_bars.side(!right).enabled == cfg.side_bar_on(!right));
                    }
                }
            }
        }
    }

    #[test]
    fn the_main_bars_edge_takes_the_sidebar_but_never_a_side_bar() {
        let mut cfg = ShellConfig::default();
        cfg.bar.position = BarPosition::Left;
        assert_eq!(cfg.set_edge_use(false, EdgeUse::SideBar), Err(EdgeBlock::BarHere));
        assert_eq!(cfg.set_edge_use(false, EdgeUse::Sidebar), Ok(()));
        assert_eq!(cfg.edge_use(false), EdgeUse::Sidebar);
        assert_eq!(cfg.set_edge_use(true, EdgeUse::SideBar), Ok(()));
        assert_eq!((cfg.edge_use(false), cfg.edge_use(true)), (EdgeUse::Sidebar, EdgeUse::SideBar));
    }

    #[test]
    fn a_presets_rail_holds_its_edge_until_the_preset_changes() {
        for style in [BarStyle::Expressive, BarStyle::RamlTram] {
            let mut cfg = ShellConfig::default();
            style.apply(&mut cfg);
            assert_eq!(cfg.edge_rail(false), Some(style));
            assert_eq!(cfg.edge_rail(true), None);
            assert_eq!(cfg.edge_use(false), EdgeUse::SideBar);
            for choice in [EdgeUse::Nothing, EdgeUse::Sidebar] {
                assert_eq!(cfg.set_edge_use(false, choice), Err(EdgeBlock::Rail(style)));
            }
            assert_eq!(cfg.edge_use(false), EdgeUse::SideBar);
            // The right edge stays free to choose.
            assert_eq!(cfg.set_edge_use(true, EdgeUse::Sidebar), Ok(()));
            assert_eq!(cfg.edge_use(true), EdgeUse::Sidebar);
            crate::bar_styles::clear(&mut cfg);
            assert_eq!(cfg.edge_rail(false), None);
        }
        // Rail stands the bar itself on the left edge: no rail side bar.
        let mut cfg = ShellConfig::default();
        BarStyle::Rail.apply(&mut cfg);
        assert_eq!(cfg.edge_rail(false), None);
        assert_eq!(cfg.edge_allows(false, EdgeUse::SideBar), Err(EdgeBlock::BarHere));
    }

    #[test]
    fn choosing_keeps_auto_placement_and_every_other_setting() {
        let mut cfg = ShellConfig::default();
        cfg.sidebar.width = 420.0;
        cfg.side_bars.width = 50.0;
        assert_eq!(cfg.edge_use(false), EdgeUse::Sidebar, "Auto is the left edge in English");
        cfg.set_edge_use(false, EdgeUse::Sidebar).unwrap();
        assert_eq!(cfg.sidebar.side, SidebarSide::Auto);
        cfg.set_edge_use(true, EdgeUse::Sidebar).unwrap();
        assert_eq!(cfg.sidebar.side, SidebarSide::Right);
        assert_eq!(cfg.edge_use(false), EdgeUse::Nothing, "the one sidebar moved");
        cfg.set_edge_use(true, EdgeUse::SideBar).unwrap();
        assert!(!cfg.sidebar.enabled && cfg.side_bar_on(true) && !cfg.side_bar_on(false));
        cfg.set_edge_use(true, EdgeUse::Nothing).unwrap();
        assert!(!cfg.side_bars.enabled, "no side bar left");
        assert_eq!((cfg.sidebar.width, cfg.side_bars.width), (420.0, 50.0));
    }

    #[test]
    fn ids_are_distinct() {
        let ids: std::collections::HashSet<_> = EdgeUse::ALL.into_iter().map(EdgeUse::id).collect();
        assert_eq!(ids.len(), EdgeUse::ALL.len());
    }
}
