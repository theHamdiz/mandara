//! Locations within Studio's Bars page, shared by routes, search and settings.

use mandara_i18n::tk;

/// A Bars destination shared by settings search, saved navigation and CLI routes.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum BarsTab {
    #[default]
    Bar,
    /// What each side edge holds: side bars (slim bars of items) and the
    /// sidebar (the slide-out panel), formerly two tabs.
    Sidebars,
    Panels,
    Edges,
}

/// A section of the Sidebars tab: the side bars, or the sidebar panel.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SidebarsPart {
    SideBar,
    Panel,
}

impl SidebarsPart {
    pub const ALL: [Self; 2] = [Self::SideBar, Self::Panel];

    /// Stable route name (`bars/sidebars/<slug>`).
    pub fn slug(self) -> &'static str {
        match self {
            Self::SideBar => "side-bar",
            Self::Panel => "sidebar",
        }
    }

    /// Accepts the section slugs and the two former tab names, ignoring
    /// ASCII case and surrounding whitespace.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "side-bar" | "side-bars" | "side_bars" | "side_bar" | "sides" => Some(Self::SideBar),
            "sidebar" | "panel" | "tabs" | "cards" | "drawer" => Some(Self::Panel),
            _ => None,
        }
    }

    /// The section a configuration path belongs to; `None` outside it.
    pub fn for_path(path: &str) -> Option<Self> {
        if path.starts_with("/side_bars/") { Some(Self::SideBar) }
        else if path.starts_with("/sidebar/") && BarsTab::for_path(path) == Some(BarsTab::Sidebars) { Some(Self::Panel) }
        else { None }
    }

    /// The section a settings group of the Sidebars tab belongs to. The two
    /// sections' group names never overlap, so a fold key needs only the
    /// group to find its former tab.
    pub fn of_group(group: &str) -> Self {
        if super::side_bar_settings::GROUPS.contains(&group) { Self::SideBar } else { Self::Panel }
    }

    /// The tab slug and page name its folds were saved under before the
    /// two tabs merged.
    fn former(self) -> (&'static str, &'static str) {
        match self {
            Self::SideBar => ("side-bars", "Bar"),
            Self::Panel => ("sidebar", "Sidebar"),
        }
    }
}

impl BarsTab {
    /// Display order; the default destination is the main bar.
    pub const ALL: [Self; 4] = [Self::Bar, Self::Sidebars, Self::Panels, Self::Edges];

    /// Stable route name, independent of the interface language.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Bar => "bar", Self::Sidebars => "sidebars",
            Self::Panels => "panels", Self::Edges => "edges",
        }
    }

    /// Translation key for the visible tab label; translate it at the call site.
    pub fn title(self) -> &'static str {
        match self {
            Self::Bar => tk("Bar"), Self::Sidebars => tk("Sidebars"),
            Self::Panels => tk("Drop-down panels"), Self::Edges => tk("Edges & corners"),
        }
    }

    /// Name in the shell's shared icon set.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Bar => "window", Self::Sidebars => "sidebar",
            Self::Panels => "sliders", Self::Edges => "pointer",
        }
    }

    /// Accepts canonical slugs and legacy route aliases, ignoring ASCII case
    /// and surrounding whitespace. The former Side bars and Sidebar tabs
    /// (and their aliases) lead to Sidebars. Unknown names have no
    /// destination.
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|tab| tab.slug() == name).or(match name.as_str() {
            "main" | "main-bar" => Some(Self::Bar),
            "menu" | "drop-down-panels" => Some(Self::Panels),
            "hover" | "corners" | "edges-and-corners" => Some(Self::Edges),
            _ => SidebarsPart::parse(&name).map(|_| Self::Sidebars),
        })
    }

    /// Maps a configuration JSON pointer to its tab without renaming settings.
    /// Shared paths such as panel radius and hover reveal follow their controls;
    /// paths outside Bars return `None`.
    pub fn for_path(path: &str) -> Option<Self> {
        if path.starts_with("/side_bars/") { Some(Self::Sidebars) }
        else if path.starts_with("/panels/") || path == "/bar/panel_radius" { Some(Self::Panels) }
        else if path.starts_with("/edges/") || path.starts_with("/hot_corners/") || path.starts_with("/screen_corners/") || path == "/sidebar/hover_reveal" { Some(Self::Edges) }
        else if path.starts_with("/sidebar/") { Some(Self::Sidebars) }
        else if path.starts_with("/bar/") { Some(Self::Bar) }
        else { None }
    }

    /// Where a fold of this tab named `group` was saved before: the former
    /// tab's key (`Bars/<tab>/<group>`, only when it differs from this
    /// tab's), then the former page's (`<page>/<group>`), in that order.
    pub fn former_keys(self, group: &str) -> (Option<String>, String) {
        let (tab, page) = match self {
            Self::Sidebars => {
                let (tab, page) = SidebarsPart::of_group(group).former();
                (Some(tab), page)
            }
            Self::Edges => (None, "Sidebar"),
            Self::Bar | Self::Panels => (None, "Bar"),
        };
        (tab.map(|tab| format!("Bars/{tab}/{group}")), format!("{page}/{group}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{self, Section};

    #[test]
    fn every_bars_setting_has_one_tab_and_its_original_config_path() {
        let settings: Vec<_> = registry::in_section(Section::Bars).collect();
        let mut assigned = std::collections::HashSet::new();
        for tab in BarsTab::ALL {
            let paths: Vec<_> = settings.iter().filter(|s| BarsTab::for_path(s.path) == Some(tab)).map(|s| s.path).collect();
            assert!(!paths.is_empty(), "{tab:?}");
            for path in paths { assert!(assigned.insert(path), "{path}"); }
        }
        assert_eq!(assigned.len(), settings.len());
        assert_eq!(BarsTab::for_path("/bar/panel_radius"), Some(BarsTab::Panels));
        assert_eq!(BarsTab::for_path("/sidebar/hover_reveal"), Some(BarsTab::Edges));
        assert_eq!(BarsTab::for_path("/dock/enabled"), None);
    }

    #[test]
    fn every_sidebars_setting_is_in_exactly_one_section_and_groups_never_overlap() {
        let mut groups = [std::collections::HashSet::new(), std::collections::HashSet::new()];
        for s in registry::in_section(Section::Bars).filter(|s| BarsTab::for_path(s.path) == Some(BarsTab::Sidebars)) {
            let part = SidebarsPart::for_path(s.path).unwrap_or_else(|| panic!("{}", s.path));
            assert_eq!(SidebarsPart::of_group(s.group), part, "{} in {}", s.path, s.group);
            groups[part as usize].insert(s.group);
        }
        assert!(groups[0].is_disjoint(&groups[1]), "{groups:?}");
        assert!(!groups[0].is_empty() && !groups[1].is_empty());
        assert_eq!(SidebarsPart::for_path("/sidebar/hover_reveal"), None, "it stays on Edges");
    }

    #[test]
    fn route_names_round_trip_and_the_former_tabs_lead_to_sidebars() {
        for tab in BarsTab::ALL { assert_eq!(BarsTab::parse(tab.slug()), Some(tab)); }
        for part in SidebarsPart::ALL { assert_eq!(SidebarsPart::parse(part.slug()), Some(part)); }
        for former in ["side-bars", " SIDE_BARS ", "sides", "sidebar", "Sidebar", "cards", "tabs", "drawer"] {
            assert_eq!(BarsTab::parse(former), Some(BarsTab::Sidebars), "{former}");
        }
        assert_eq!(SidebarsPart::parse("side-bars"), Some(SidebarsPart::SideBar));
        assert_eq!(SidebarsPart::parse("sidebar"), Some(SidebarsPart::Panel));
        assert_eq!(SidebarsPart::parse("cards"), Some(SidebarsPart::Panel));
        assert_eq!(BarsTab::parse("hover"), Some(BarsTab::Edges));
        assert_eq!(BarsTab::parse("unknown"), None);
        assert_eq!(SidebarsPart::parse("sidebars"), None, "the tab itself names no section");
    }

    #[test]
    fn folds_find_their_former_tab_and_page() {
        use super::super::side_bar_settings::SIDE_BAR_SHAPE;
        assert_eq!(BarsTab::Sidebars.former_keys(SIDE_BAR_SHAPE), (Some("Bars/side-bars/Side bar shape".into()), "Bar/Side bar shape".into()));
        assert_eq!(BarsTab::Sidebars.former_keys("Layout"), (Some("Bars/sidebar/Layout".into()), "Sidebar/Layout".into()));
        assert_eq!(BarsTab::Edges.former_keys("Hot corners"), (None, "Sidebar/Hot corners".into()));
        assert_eq!(BarsTab::Bar.former_keys("Layout"), (None, "Bar/Layout".into()));
    }
}
