//! The inputs that require rebuilding plugin slots, rather than repainting them.

use mandara_config::ShellConfig;

use crate::sidebar_tabs::SidebarTabs;

/// Plugin membership and factory generation captured when shell slots are built.
/// Appearance and dimensions are excluded so those edits reuse existing views.
pub(super) struct Placement {
    pub zones: [Vec<String>; 3],
    pub sides: [Vec<String>; 6],
    pub notifications_tab: bool,
    cards: Vec<(String, Vec<String>)>,
    disabled: Vec<String>,
    enabled: Vec<String>,
    removed: Vec<String>,
    generation: u64,
    vertical: bool,
}

/// Disabled side bars instantiate no plugins, regardless of their saved lists.
fn side_groups(cfg: &ShellConfig) -> [&[String]; 6] {
    let on = [cfg.side_bar_on(false), cfg.side_bar_on(true)];
    std::array::from_fn(|k| {
        if on[k / 3] {
            cfg.side_bars.side(k / 3 == 1).zones()[k % 3].as_slice()
        } else {
            &[]
        }
    })
}

impl Placement {
    /// Captures effective placements, omitting plugins on disabled side bars.
    /// `generation` identifies the registry factories used to build the slots.
    pub fn new(cfg: &ShellConfig, generation: u64) -> Self {
        Self {
            zones: [
                cfg.bar.left.clone(),
                cfg.bar.center.clone(),
                cfg.bar.right.clone(),
            ],
            sides: side_groups(cfg).map(<[String]>::to_vec),
            notifications_tab: cfg.sidebar.notifications_tab,
            cards: SidebarTabs::key(&cfg.sidebar.tabs),
            disabled: cfg.plugins.disabled.clone(),
            enabled: cfg.plugins.enabled.clone(),
            removed: cfg.plugins.removed.clone(),
            generation,
            vertical: cfg.bar.position.is_vertical(),
        }
    }

    /// Whether existing slots can be reused for this configuration and registry.
    /// Compares membership and sidebar cards in place; matching frames clone nothing.
    /// A match preserves view ownership, not geometry: callers still lay out
    /// the retained slots using the current dimensions and appearance.
    pub fn matches(&self, cfg: &ShellConfig, generation: u64, tabs: &SidebarTabs) -> bool {
        self.generation == generation
            && self.vertical == cfg.bar.position.is_vertical()
            && self.notifications_tab == cfg.sidebar.notifications_tab
            && self.zones[0] == cfg.bar.left
            && self.zones[1] == cfg.bar.center
            && self.zones[2] == cfg.bar.right
            && self
                .sides
                .iter()
                .zip(side_groups(cfg))
                .all(|(cached, live)| cached == live)
            && tabs.matches(&cfg.sidebar.tabs, &self.cards)
            && self.disabled == cfg.plugins.disabled
            && self.enabled == cfg.plugins.enabled
            && self.removed == cfg.plugins.removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::{BarLayout, BarPosition};

    const GENERATION: u64 = 1;

    fn matches(key: &Placement, cfg: &ShellConfig) -> bool {
        key.matches(cfg, GENERATION, &SidebarTabs::new())
    }

    #[test]
    fn appearance_and_geometry_edits_reuse_the_live_slots() {
        let cfg = ShellConfig::default();
        let key = Placement::new(&cfg, GENERATION);
        let edits: [fn(&mut ShellConfig); 6] = [
            |c| c.theme.mode = c.theme.mode.toggled(),
            |c| c.bar.height += 1.0,
            |c| c.sidebar.width += 1.0,
            |c| c.panels.width_scale += 0.1,
            |c| c.appearance.launcher_icon = "grid".into(),
            |c| c.bar.position = BarPosition::Bottom,
        ];
        for edit in edits {
            let mut changed = cfg.clone();
            edit(&mut changed);
            assert!(matches(&key, &changed));
        }
        for layout in [
            BarLayout::MinimalEdge,
            BarLayout::FloatingPills,
            BarLayout::ContiguousIsland,
            BarLayout::Hairline,
            BarLayout::Micro,
            BarLayout::Notch,
        ] {
            let mut changed = cfg.clone();
            changed.bar.layout = layout;
            assert!(
                matches(&key, &changed),
                "{layout:?}: shape changes repaint the same plugin views"
            );
        }
    }

    #[test]
    fn axis_and_factory_changes_rebuild_slots() {
        let cfg = ShellConfig::default();
        let key = Placement::new(&cfg, GENERATION);
        assert!(!key.matches(&cfg, GENERATION + 1, &SidebarTabs::new()));
        for position in [BarPosition::Left, BarPosition::Right] {
            let mut changed = cfg.clone();
            changed.bar.position = position;
            assert!(!matches(&key, &changed));
        }
    }

    #[test]
    fn geometry_retargets_without_rebuilding_membership_on_either_output() {
        use crate::bar::layout::Metrics;
        let cfg = ShellConfig::default();
        let slots = Placement::new(&cfg, GENERATION);
        let widths = [120.0, 300.0, 240.0];
        for surface in [(1920.0, 1080.0), (2560.0, 1440.0)] {
            let before = super::super::layout_key(widths, Metrics::of(&cfg, 1.0), surface);
            let edits: [fn(&mut ShellConfig); 3] = [
                |c| c.bar.height += 1.0,
                |c| c.bar.position = BarPosition::Bottom,
                |c| c.bar.layout = BarLayout::Notch,
            ];
            for edit in edits {
                let mut changed = cfg.clone();
                edit(&mut changed);
                assert!(matches(&slots, &changed));
                assert_ne!(
                    super::super::layout_key(widths, Metrics::of(&changed, 1.0), surface),
                    before
                );
            }
            assert_ne!(
                super::super::layout_key(
                    widths,
                    Metrics::of(&cfg, 1.0),
                    (surface.0 + 1.0, surface.1)
                ),
                before
            );
            assert!(matches(&slots, &cfg));
        }
    }

    #[test]
    fn every_plugin_switch_including_unplaced_removals_rebuilds_slots() {
        let cfg = ShellConfig::default();
        let key = Placement::new(&cfg, GENERATION);
        let edits: [fn(&mut ShellConfig); 4] = [
            |c| c.plugins.disabled.push("notifications".into()),
            |c| c.plugins.enabled.push("notes".into()),
            |c| c.plugins.removed.push("notifications".into()),
            |c| c.sidebar.notifications_tab = !c.sidebar.notifications_tab,
        ];
        for edit in edits {
            let mut changed = cfg.clone();
            edit(&mut changed);
            assert!(!matches(&key, &changed));
        }
    }

    #[test]
    fn all_bar_and_sidebar_lists_follow_their_live_configuration() {
        let cfg = ShellConfig::default();
        let key = Placement::new(&cfg, GENERATION);
        let edits: [fn(&mut ShellConfig); 5] = [
            |c| c.bar.left.push("notes".into()),
            |c| c.bar.center.push("notes".into()),
            |c| c.bar.right.push("notes".into()),
            |c| c.sidebar.tabs[0].cards.push("notes".into()),
            |c| c.sidebar.tabs[0].id.push_str("-new"),
        ];
        for edit in edits {
            let mut changed = cfg.clone();
            edit(&mut changed);
            assert!(!matches(&key, &changed));
        }
        let mut renamed = cfg;
        renamed.sidebar.tabs[0].name.push_str(" renamed");
        renamed.sidebar.tabs[0].icon = "note".into();
        assert!(
            matches(&key, &renamed),
            "tab labels and icons are read directly when drawing"
        );
        let mut reordered = renamed;
        let mut other = reordered.sidebar.tabs[0].clone();
        other.id = "other".into();
        reordered.sidebar.tabs.push(other);
        assert!(!matches(&key, &reordered));
        let ordered = Placement::new(&reordered, GENERATION);
        reordered.sidebar.tabs.reverse();
        assert!(!matches(&ordered, &reordered));
    }

    #[test]
    fn side_groups_rebuild_only_while_their_side_is_active() {
        let mut cfg = ShellConfig::default();
        cfg.side_bars.enabled = true;
        cfg.side_bars.left.enabled = true;
        cfg.side_bars.right.enabled = true;
        let key = Placement::new(&cfg, GENERATION);
        for index in 0..6 {
            let mut changed = cfg.clone();
            let side = if index < 3 {
                &mut changed.side_bars.left
            } else {
                &mut changed.side_bars.right
            };
            match index % 3 {
                0 => &mut side.top,
                1 => &mut side.middle,
                _ => &mut side.bottom,
            }
            .push("notes".into());
            assert!(!matches(&key, &changed), "group {index}");
        }
        cfg.side_bars.left.enabled = false;
        assert!(!matches(&key, &cfg));
        let disabled = Placement::new(&cfg, GENERATION);
        cfg.side_bars.left.top.push("notes".into());
        assert!(matches(&disabled, &cfg));
        cfg.side_bars.left.enabled = true;
        assert!(!matches(&disabled, &cfg));
        cfg.side_bars.enabled = false;
        let disabled = Placement::new(&cfg, GENERATION);
        cfg.side_bars.right.bottom.push("notes".into());
        assert!(matches(&disabled, &cfg));
    }
}
