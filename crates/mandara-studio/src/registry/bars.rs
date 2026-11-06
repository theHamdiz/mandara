//! Locations within Studio's Bars page, shared by routes, search and settings.

use mandara_i18n::tk;

/// A Bars destination shared by settings search, saved navigation and CLI routes.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum BarsTab {
    #[default]
    Bar,
    SideBars,
    Sidebar,
    Panels,
    Edges,
}

impl BarsTab {
    /// Display order; the default destination is the main bar.
    pub const ALL: [Self; 5] = [Self::Bar, Self::SideBars, Self::Sidebar, Self::Panels, Self::Edges];

    /// Stable route name, independent of the interface language.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Bar => "bar", Self::SideBars => "side-bars", Self::Sidebar => "sidebar",
            Self::Panels => "panels", Self::Edges => "edges",
        }
    }

    /// Translation key for the visible tab label; translate it at the call site.
    pub fn title(self) -> &'static str {
        match self {
            Self::Bar => tk("Bar"), Self::SideBars => tk("Side bars"), Self::Sidebar => tk("Sidebar"),
            Self::Panels => tk("Drop-down panels"), Self::Edges => tk("Edges & corners"),
        }
    }

    /// Name in the shell's shared icon set.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Bar => "window", Self::SideBars | Self::Sidebar => "sidebar",
            Self::Panels => "sliders", Self::Edges => "pointer",
        }
    }

    /// Accepts canonical slugs and legacy route aliases, ignoring ASCII case
    /// and surrounding whitespace. Unknown names have no destination.
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|tab| tab.slug() == name).or(match name.as_str() {
            "main" | "main-bar" => Some(Self::Bar),
            "side_bars" | "sides" => Some(Self::SideBars),
            "tabs" | "cards" | "drawer" => Some(Self::Sidebar),
            "menu" | "drop-down-panels" => Some(Self::Panels),
            "hover" | "corners" | "edges-and-corners" => Some(Self::Edges),
            _ => None,
        })
    }

    /// Maps a configuration JSON pointer to its tab without renaming settings.
    /// Shared paths such as panel radius and hover reveal follow their controls;
    /// paths outside Bars return `None`.
    pub fn for_path(path: &str) -> Option<Self> {
        if path.starts_with("/side_bars/") { Some(Self::SideBars) }
        else if path.starts_with("/panels/") || path == "/bar/panel_radius" { Some(Self::Panels) }
        else if path.starts_with("/edges/") || path.starts_with("/hot_corners/") || path.starts_with("/screen_corners/") || path == "/sidebar/hover_reveal" { Some(Self::Edges) }
        else if path.starts_with("/sidebar/") { Some(Self::Sidebar) }
        else if path.starts_with("/bar/") { Some(Self::Bar) }
        else { None }
    }

    /// Fold state was stored under the two former page names.
    pub fn former_page(self) -> &'static str {
        match self { Self::Sidebar | Self::Edges => "Sidebar", _ => "Bar" }
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
    fn route_names_round_trip_and_keep_sidebar_distinct_from_side_bars() {
        for tab in BarsTab::ALL { assert_eq!(BarsTab::parse(tab.slug()), Some(tab)); }
        assert_eq!(BarsTab::parse(" SIDE_BARS "), Some(BarsTab::SideBars));
        assert_eq!(BarsTab::parse("cards"), Some(BarsTab::Sidebar));
        assert_eq!(BarsTab::parse("hover"), Some(BarsTab::Edges));
        assert_eq!(BarsTab::parse("unknown"), None);
    }
}
