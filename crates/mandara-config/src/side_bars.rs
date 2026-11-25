//! Side bars (`[side_bars]`): a slim left and right bar of plugin chips
//! (icons, pills, islands; never widget cards). Off by default. While on,
//! a side bar takes its edge from the widget sidebar (whose settings are
//! kept and come back when side bars are off), and by default the top bar
//! and both side bars join into one frame.

use serde::{Deserialize, Serialize};

use crate::surfaces::{BarCenterAlign, Span};
use crate::{BarLayout, BarPosition, ShellConfig};

/// Named bounds of the side bar settings (logical px unless noted).
pub mod ranges {
    use super::Span;

    pub const WIDTH: Span = Span::new(28.0, 72.0, 1.0);
    /// Gap to the screen edge and the top bar (separate side bars), and the
    /// inset of the first and last items from the bar's ends.
    pub const EDGE_GAP: Span = Span::new(0.0, 32.0, 1.0);
    /// An item's extent across the bar (capped at the bar's width).
    pub const CHIP_SIZE: Span = Span::new(16.0, 72.0, 1.0);
    /// Space inside each item along the bar, around its content.
    pub const CHIP_PADDING: Span = Span::new(0.0, 20.0, 1.0);
    pub const CHIP_SPACING: Span = Span::new(0.0, 24.0, 1.0);
    pub const CORNER_RADIUS: Span = Span::new(0.0, 36.0, 1.0);
    pub const FLARE_RADIUS: Span = Span::new(0.0, 40.0, 1.0);
    /// Share of the accent mixed into the fill (0–1).
    pub const TINT: Span = Span::new(0.0, 0.6, 0.01);
    /// Strength of the pill / island wash behind items (multiplier).
    pub const CHIP_FILL: Span = Span::new(0.0, 3.0, 0.05);
    /// Pointer dwell on the screen edge before a hidden side bar slides
    /// out, and the wait after the pointer leaves before it slides away, ms.
    pub const REVEAL_DELAY_MS: Span = Span::new(0.0, 1500.0, 10.0);
    pub const HIDE_DELAY_MS: Span = Span::new(0.0, 3000.0, 20.0);
    /// One bar's item inset from its long edges.
    pub const ITEM_INSET: Span = Span::new(0.0, 20.0, 1.0);
}

/// Defaults of the side bar settings.
pub mod defaults {
    pub const WIDTH_PX: f32 = 40.0;
    pub const EDGE_GAP_PX: f32 = 6.0;
    pub const CHIP_SIZE_PX: f32 = 34.0;
    pub const CHIP_PADDING_PX: f32 = 7.0;
    pub const CHIP_SPACING_PX: f32 = 4.0;
    pub const CORNER_RADIUS_PX: f32 = 16.0;
    pub const FLARE_RADIUS_PX: f32 = 18.0;
    pub const CHIP_FILL: f32 = 1.0;
    pub const REVEAL_DELAY_MS: u32 = 150;
    pub const HIDE_DELAY_MS: u32 = 600;
}

/// The shape of separate (not joined) side bars.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SideBarShape {
    /// One glass pill per group (top, middle, bottom).
    #[default]
    Pills,
    /// The three groups packed into one island.
    Island,
    /// One rounded slab along the whole edge; panels pour out of it.
    Slab,
}

/// How the items of a side bar are drawn.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SideBarChips {
    /// Bare icons that light up under the pointer.
    #[default]
    Icons,
    /// Every item on a pill of its own.
    Pills,
    /// Each group's items on one shared island.
    Island,
}

/// The colour a side bar's items light up and wash with: the theme's
/// accent or one of the palette's other roles.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SideBarAccent {
    /// The theme's accent (`primary`).
    #[default]
    Theme,
    Secondary,
    Tertiary,
    Emerald,
    Amber,
    Coral,
}

/// One side bar: whether it shows, whether it stays, its colour and the
/// plugins of its three groups.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SideBarConfig {
    pub enabled: bool,
    /// Always shown, with its width reserved so windows tile beside it;
    /// unpinned it slides away and comes back when the pointer touches its
    /// screen edge.
    pub pinned: bool,
    pub accent: SideBarAccent,
    /// Plugin ids per group, in order (top to bottom).
    pub top: Vec<String>,
    pub middle: Vec<String>,
    pub bottom: Vec<String>,
    /// This bar's own item look; unset, it follows the side bars' shared
    /// one (`chips`, `icon_shape`) and the bar's (`item_shape`,
    /// `icon_fill`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chips: Option<SideBarChips>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_shape: Option<crate::surfaces::BarItemShape>,
    /// `match` here draws icon items like the other items.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_shape: Option<crate::surfaces::BarIconShape>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_fill: Option<crate::surfaces::BarIconFill>,
    /// Space between the items and the bar's long edges (sets the items'
    /// size across the bar).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_inset: Option<f32>,
    /// The bar style preset that shaped this bar (`side-bar style`);
    /// another one, or none, clears what it set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<crate::bar_styles::BarStyle>,
}

/// How one side bar draws its items, resolved: its own look over the
/// shared one ([`SideBarsConfig::items`]).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SideItems {
    pub chips: SideBarChips,
    pub item_shape: crate::surfaces::BarItemShape,
    pub icon_shape: crate::surfaces::BarIconShape,
    pub icon_fill: crate::surfaces::BarIconFill,
    /// The items' extent across the bar.
    pub chip_size: f32,
}

impl SideBarConfig {
    /// The three groups, top to bottom.
    pub fn zones(&self) -> [&Vec<String>; 3] {
        [&self.top, &self.middle, &self.bottom]
    }

    pub fn zone_mut(&mut self, zone: SideZone) -> &mut Vec<String> {
        match zone {
            SideZone::Top => &mut self.top,
            SideZone::Middle => &mut self.middle,
            SideZone::Bottom => &mut self.bottom,
        }
    }

    fn with(top: &[&str], middle: &[&str], bottom: &[&str]) -> Self {
        let ids = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
        Self { enabled: true, pinned: true, accent: SideBarAccent::Theme, top: ids(top), middle: ids(middle), bottom: ids(bottom), chips: None, item_shape: None, icon_shape: None, icon_fill: None, item_inset: None, style: None }
    }
}

impl Default for SideBarConfig {
    fn default() -> Self {
        Self::with(&[], &[], &[])
    }
}

/// A group of a side bar.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SideZone {
    Top,
    Middle,
    Bottom,
}

impl SideZone {
    pub const ALL: [SideZone; 3] = [SideZone::Top, SideZone::Middle, SideZone::Bottom];
}

/// `[side_bars]`: both side bars and their shared look.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SideBarsConfig {
    /// The master switch (off by default).
    pub enabled: bool,
    /// The top bar and the side bars join into one frame (the top bar is
    /// drawn as the edge frame while they do; its own style comes back
    /// when they're separate).
    pub joined: bool,
    /// The shape of separate side bars.
    pub shape: SideBarShape,
    pub width: f32,
    pub edge_gap: f32,
    pub chip_size: f32,
    pub chip_padding: f32,
    pub chip_spacing: f32,
    pub chips: SideBarChips,
    /// The shape of icon-sized pill items (`match`: the bar's
    /// `icon_shape`).
    pub icon_shape: crate::surfaces::BarIconShape,
    /// Strength of the pill / island wash behind items.
    pub chip_fill: f32,
    /// Where the middle group sits.
    pub align: BarCenterAlign,
    pub corner_radius: f32,
    pub flare_radius: f32,
    /// Separate side bars: their background opacity, accent tint, frosted
    /// wallpaper glass, specular rim and shadow. Joined ones share the top
    /// bar's material so the frame reads as one piece.
    pub opacity: f32,
    pub tint: f32,
    pub glass: bool,
    pub rim: bool,
    pub rim_intensity: f32,
    pub shadow: bool,
    pub reveal_delay_ms: u32,
    pub hide_delay_ms: u32,
    pub left: SideBarConfig,
    pub right: SideBarConfig,
}

impl Default for SideBarsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            joined: true,
            shape: SideBarShape::Pills,
            width: defaults::WIDTH_PX,
            edge_gap: defaults::EDGE_GAP_PX,
            chip_size: defaults::CHIP_SIZE_PX,
            chip_padding: defaults::CHIP_PADDING_PX,
            chip_spacing: defaults::CHIP_SPACING_PX,
            chips: SideBarChips::Icons,
            icon_shape: crate::surfaces::BarIconShape::Match,
            chip_fill: defaults::CHIP_FILL,
            align: BarCenterAlign::Screen,
            corner_radius: defaults::CORNER_RADIUS_PX,
            flare_radius: defaults::FLARE_RADIUS_PX,
            opacity: 1.0,
            tint: 0.0,
            glass: false,
            rim: true,
            rim_intensity: 1.0,
            shadow: true,
            reveal_delay_ms: defaults::REVEAL_DELAY_MS,
            hide_delay_ms: defaults::HIDE_DELAY_MS,
            left: SideBarConfig::with(&["workspaces"], &[], &["wallpaper", "screenshot"]),
            right: SideBarConfig::with(&["clock"], &["power"], &["notifications"]),
        }
    }
}

impl SideBarsConfig {
    /// The left (`false`) or right side bar's own settings.
    pub fn side(&self, right: bool) -> &SideBarConfig {
        if right { &self.right } else { &self.left }
    }

    pub fn side_mut(&mut self, right: bool) -> &mut SideBarConfig {
        if right { &mut self.right } else { &mut self.left }
    }

    pub fn width(&self) -> f32 {
        ranges::WIDTH.clamp(self.width, defaults::WIDTH_PX)
    }

    pub fn edge_gap(&self) -> f32 {
        ranges::EDGE_GAP.clamp(self.edge_gap, defaults::EDGE_GAP_PX)
    }

    /// An item's extent across the bar: at most the bar's width.
    pub fn chip_size(&self) -> f32 {
        ranges::CHIP_SIZE.clamp(self.chip_size, defaults::CHIP_SIZE_PX).min(self.width())
    }

    pub fn chip_padding(&self) -> f32 {
        ranges::CHIP_PADDING.clamp(self.chip_padding, defaults::CHIP_PADDING_PX)
    }

    pub fn chip_spacing(&self) -> f32 {
        ranges::CHIP_SPACING.clamp(self.chip_spacing, defaults::CHIP_SPACING_PX)
    }

    /// How the left (`false`) or right side bar draws its items: its own
    /// look where it has one, else the shared side bar settings and the
    /// bar's item shape and icon colour.
    pub fn items(&self, right: bool, bar: &crate::BarConfig) -> SideItems {
        let own = self.side(right);
        let chip_size = match own.item_inset {
            Some(inset) => (self.width() - 2.0 * ranges::ITEM_INSET.clamp(inset, 0.0)).max(ranges::CHIP_SIZE.min).min(self.width()),
            None => self.chip_size(),
        };
        SideItems {
            chips: own.chips.unwrap_or(self.chips),
            item_shape: own.item_shape.unwrap_or(bar.item_shape),
            icon_shape: own.icon_shape.unwrap_or_else(|| self.icon_shape(bar)),
            icon_fill: own.icon_fill.unwrap_or(bar.icon_fill),
            chip_size,
        }
    }

    /// The icon shape the side bars draw: their own, or the bar's.
    pub fn icon_shape(&self, bar: &crate::BarConfig) -> crate::surfaces::BarIconShape {
        if self.icon_shape == crate::surfaces::BarIconShape::Match { bar.icon_shape } else { self.icon_shape }
    }

    pub fn chip_fill(&self) -> f32 {
        ranges::CHIP_FILL.clamp(self.chip_fill, defaults::CHIP_FILL)
    }

    pub fn corner_radius(&self) -> f32 {
        ranges::CORNER_RADIUS.clamp(self.corner_radius, defaults::CORNER_RADIUS_PX)
    }

    pub fn flare_radius(&self) -> f32 {
        ranges::FLARE_RADIUS.clamp(self.flare_radius, defaults::FLARE_RADIUS_PX)
    }

    pub fn opacity(&self) -> f32 {
        crate::surfaces::ranges::OPACITY.clamp(self.opacity, 1.0)
    }

    pub fn tint(&self) -> f32 {
        ranges::TINT.clamp(self.tint, 0.0)
    }

    pub fn rim_intensity(&self) -> f32 {
        if self.rim { crate::surfaces::ranges::INTENSITY.clamp(self.rim_intensity, 1.0) } else { 0.0 }
    }

    pub fn reveal_delay(&self) -> std::time::Duration {
        std::time::Duration::from_millis(ranges::REVEAL_DELAY_MS.clamp(self.reveal_delay_ms as f32, defaults::REVEAL_DELAY_MS as f32) as u64)
    }

    pub fn hide_delay(&self) -> std::time::Duration {
        std::time::Duration::from_millis(ranges::HIDE_DELAY_MS.clamp(self.hide_delay_ms as f32, defaults::HIDE_DELAY_MS as f32) as u64)
    }

    /// The bar style a side bar is drawn as: the edge frame when joined (or
    /// a slab, which floats as a frame), else the separate shape.
    pub fn layout(&self, joined: bool) -> BarLayout {
        match (joined, self.shape) {
            (true, _) | (false, SideBarShape::Slab) => BarLayout::MinimalEdge,
            (false, SideBarShape::Pills) => BarLayout::FloatingPills,
            (false, SideBarShape::Island) => BarLayout::ContiguousIsland,
        }
    }
}

impl ShellConfig {
    /// Whether the left (`false`) or right side bar shows: side bars on,
    /// that side on, and the main bar not on that edge itself.
    pub fn side_bar_on(&self, right: bool) -> bool {
        let edge = if right { BarPosition::Right } else { BarPosition::Left };
        self.side_bars.enabled && self.side_bars.side(right).enabled && self.bar.position != edge
    }

    /// Whether the side bars and a top / bottom bar join into one frame.
    pub fn side_bars_join(&self) -> bool {
        self.side_bars.joined && !self.bar.position.is_vertical() && (self.side_bar_on(false) || self.side_bar_on(true))
    }

    /// The bar style drawn: the edge frame while side bars join it, else
    /// the bar's own ([`crate::BarConfig::effective_layout`]). The bar's
    /// setting is kept and comes back when they don't.
    pub fn bar_layout(&self) -> BarLayout {
        if self.side_bars_join() { BarLayout::MinimalEdge } else { self.bar.effective_layout() }
    }

    /// Whether the widget sidebar is in use: on, and its edge not taken by a
    /// side bar (its settings are kept for when side bars are off).
    pub fn sidebar_active(&self) -> bool {
        self.sidebar.enabled && !self.side_bar_on(self.sidebar_on_right())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_bars_are_opt_in_and_old_configs_load() {
        let cfg: ShellConfig = toml::from_str("[bar]\nheight = 30\n").unwrap();
        assert!(!cfg.side_bars.enabled);
        assert!(!cfg.side_bar_on(false) && !cfg.side_bar_on(true));
        assert!(cfg.sidebar_active());
        assert_eq!(cfg.bar_layout(), cfg.bar.layout);
        let partial: ShellConfig = toml::from_str("[side_bars]\nenabled = true\n[side_bars.right]\nenabled = false\n").unwrap();
        assert!(partial.side_bar_on(false) && !partial.side_bar_on(true));
        assert!(partial.side_bars.joined, "joined by default");
        assert!(!partial.side_bars.left.top.is_empty(), "a missing side keeps its shipped plugins");
    }

    #[test]
    fn side_bars_take_their_edge_and_keep_the_sidebar_settings() {
        let mut cfg = ShellConfig::default();
        cfg.sidebar.side = crate::SidebarSide::Left;
        cfg.bar.layout = BarLayout::FloatingPills;
        cfg.side_bars.enabled = true;
        assert!(!cfg.sidebar_active(), "the left side bar replaces the widget sidebar");
        assert!(cfg.sidebar.enabled, "its setting is untouched");
        assert_eq!(cfg.bar_layout(), BarLayout::MinimalEdge, "joined: the top bar is the edge frame");
        assert_eq!(cfg.bar.layout, BarLayout::FloatingPills, "its own style is kept");
        cfg.side_bars.left.enabled = false;
        assert!(cfg.sidebar_active(), "only the side with a side bar gives way");
        cfg.side_bars.joined = false;
        assert_eq!(cfg.bar_layout(), BarLayout::FloatingPills);
        // A side bar never sits on the main bar's own edge.
        cfg.bar.position = BarPosition::Right;
        assert!(!cfg.side_bar_on(true));
        cfg.side_bars.joined = true;
        assert!(!cfg.side_bars_join(), "only a top or bottom bar joins");
    }

    #[test]
    fn sizes_are_clamped() {
        let mut s = SideBarsConfig { width: 30.0, chip_size: 60.0, ..SideBarsConfig::default() };
        assert_eq!(s.chip_size(), 30.0, "an item never outgrows its bar");
        s.width = f32::NAN;
        assert_eq!(s.width(), defaults::WIDTH_PX);
        s.rim = false;
        assert_eq!(s.rim_intensity(), 0.0);
        assert_eq!(s.layout(true), BarLayout::MinimalEdge);
        s.shape = SideBarShape::Island;
        assert_eq!(s.layout(false), BarLayout::ContiguousIsland);
    }
}
