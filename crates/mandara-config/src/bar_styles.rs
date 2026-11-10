//! Bar style presets: one-click bars that look nothing like the shipped
//! strip (Studio → Bars → Presets, `mandara msg bar style`).
//!
//! A style sets the bar's layout, edge and height like the other presets,
//! and also its *look*: the shape and colour of its items (tiles, Material
//! 3 Expressive shapes, connected groups), their inset and spacing, the
//! screen frame, and for *Expressive* a rail joined to the bar. The bar
//! remembers the style (`bar.style`), so picking any other preset, a Look,
//! or `bar style none` first puts every field the style touched back to its
//! default ([`clear`]).
//!
//! Each style is built from the shared bar settings, so every part of it
//! stays adjustable in Studio afterwards.

use serde::{Deserialize, Serialize};

use crate::side_bars::SideBarChips;
use crate::surfaces::{BarCapsuleFill, BarIconFill, BarIconShape, BarItemShape};
use crate::{BarConfig, BarLayout, BarPosition, ShellConfig};

/// A bar style preset.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BarStyle {
    /// A tall bottom bar inside a frame round the whole display, joined
    /// to a rail on the left: round accent buttons, tonal tiles for the
    /// widgets, scalloped badges down the rail.
    Expressive,
    /// One floating island at the bottom with large squircle icon tiles.
    TileDock,
    /// A wide rail on the left edge in a screen frame, with flower-shaped
    /// buttons and tiles.
    Rail,
    /// No bar at all: each group is a row of connected tiles floating
    /// under the top edge.
    Segmented,
    /// A dense status line along the bottom edge: every reading on a
    /// square-cornered stat block, in the monospace font, meters and
    /// rates in the middle.
    SystemMonitor,
    /// Tile dock split into three floating docks along the bottom (the
    /// arches of a bridge), with tertiary squircle buttons.
    StanleyBridge,
    /// Expressive's tonal tiles and scalloped accent buttons in a narrow
    /// lantern hanging from the top edge.
    PharosBeam,
    /// A top bar joined to a rail down the left edge (a tram line running
    /// across and down the screen), with flower buttons on both.
    RamlTram,
}

/// Plugin ids of a bar's three zones (start, middle, end; top to bottom on
/// a side bar).
pub type Zones = [&'static [&'static str]; 3];

/// Settings a style shares with the other styles' defaults.
struct Look {
    layout: BarLayout,
    position: BarPosition,
    height: f32,
    item_shape: BarItemShape,
    icon_shape: BarIconShape,
    icon_fill: BarIconFill,
    capsule_fill: BarCapsuleFill,
    capsule_strength: f32,
    item_inset: f32,
    chip_padding: f32,
    chip_spacing: f32,
    corner_radius: f32,
    flare_radius: f32,
    edge_gap: f32,
    opacity: f32,
    tint: f32,
    rim_intensity: f32,
    shadow: bool,
    icon_scale: f32,
    screen_frame: bool,
    frame_thickness: f32,
    vertical_width: f32,
    notch_width: f32,
    /// The bar's text in the monospace font (`typography.bar_font`).
    mono: bool,
}

/// A rail a style joins to its bar: the left side bar's width, item size,
/// spacing and wash, and the shape of its icon items.
struct RailLook {
    width: f32,
    chip_size: f32,
    chip_spacing: f32,
    chip_fill: f32,
    icon_shape: BarIconShape,
}

impl BarStyle {
    pub const ALL: [BarStyle; 8] = [
        BarStyle::Expressive,
        BarStyle::TileDock,
        BarStyle::Rail,
        BarStyle::Segmented,
        BarStyle::SystemMonitor,
        BarStyle::StanleyBridge,
        BarStyle::PharosBeam,
        BarStyle::RamlTram,
    ];

    /// The styles a side bar can take on its own (`side-bar style`, the
    /// side bars' Style chips).
    pub const SIDE: [BarStyle; 4] = [BarStyle::Expressive, BarStyle::TileDock, BarStyle::Rail, BarStyle::Segmented];

    /// The id used by `bar style` and in the config.
    pub fn id(self) -> &'static str {
        match self {
            BarStyle::Expressive => "expressive",
            BarStyle::TileDock => "tile-dock",
            BarStyle::Rail => "rail",
            BarStyle::Segmented => "segmented",
            BarStyle::SystemMonitor => "system-monitor",
            BarStyle::StanleyBridge => "stanley-bridge",
            BarStyle::PharosBeam => "pharos-beam",
            BarStyle::RamlTram => "raml-tram",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.id() == id)
    }

    /// A style a side bar can take on its own ([`Self::SIDE`]).
    pub fn from_side_id(id: &str) -> Option<Self> {
        Self::SIDE.into_iter().find(|s| s.id() == id)
    }

    pub fn layout(self) -> BarLayout {
        self.look().layout
    }

    pub fn position(self) -> BarPosition {
        self.look().position
    }

    pub fn height(self) -> f32 {
        self.look().height
    }

    /// The bar's items when a preset replaces them.
    pub const fn zones(self) -> Zones {
        match self {
            BarStyle::Expressive => [&["screenshot", "clipboard", "color-picker", "wallpaper", "keybinds"], &["weather", "media", "drives", "pomodoro"], &["tray", "notifications", "session"]],
            BarStyle::TileDock => [&["launcher", "screenshot", "clipboard", "color-picker"], &["workspaces", "media"], &["clock", "audio", "notifications", "session"]],
            BarStyle::Rail => [&["launcher", "workspaces"], &["clock"], &["audio", "network", "notifications", "session"]],
            BarStyle::Segmented => [&["launcher", "workspaces", "active-window"], &["clock", "media"], &["network", "bluetooth", "audio", "notifications", "session"]],
            BarStyle::SystemMonitor => [&["launcher", "workspaces", "processes"], &["mini-stats", "resources", "net-speed"], &["drives", "clock", "audio", "session"]],
            BarStyle::StanleyBridge => [&["launcher", "workspaces"], &["media", "clock"], &["audio", "network", "bluetooth", "notifications", "session"]],
            BarStyle::PharosBeam => [&["launcher", "workspaces"], &["clock", "media"], &["weather", "audio", "notifications", "session"]],
            BarStyle::RamlTram => [&["active-window"], &["clock", "media"], &["weather", "tray", "bluetooth", "notifications"]],
        }
    }

    /// The left side bar's groups (top, middle, bottom) the style joins to
    /// the bar, when a preset replaces items.
    pub const fn rail(self) -> Option<Zones> {
        match self {
            BarStyle::Expressive => Some([&["launcher"], &["clock", "workspaces"], &["audio", "network", "bluetooth"]]),
            BarStyle::RamlTram => Some([&["launcher"], &["workspaces"], &["network", "audio", "session"]]),
            _ => None,
        }
    }

    /// The rail's own look, for the styles that join one ([`Self::rail`]).
    fn rail_look(self) -> Option<RailLook> {
        match self {
            BarStyle::Expressive => Some(RailLook { width: 42.0, chip_size: 32.0, chip_spacing: 6.0, chip_fill: 1.6, icon_shape: BarIconShape::Cookie }),
            BarStyle::RamlTram => Some(RailLook { width: 46.0, chip_size: 34.0, chip_spacing: 8.0, chip_fill: 1.4, icon_shape: BarIconShape::Flower }),
            _ => None,
        }
    }

    fn look(self) -> Look {
        let base = BarConfig::default();
        let plain = Look {
            layout: base.layout,
            position: base.position,
            height: base.height,
            item_shape: base.item_shape,
            icon_shape: base.icon_shape,
            icon_fill: base.icon_fill,
            capsule_fill: base.capsule_fill,
            capsule_strength: base.capsule_strength,
            item_inset: base.item_inset,
            chip_padding: base.chip_padding,
            chip_spacing: base.chip_spacing,
            corner_radius: base.corner_radius,
            flare_radius: base.flare_radius,
            edge_gap: base.edge_gap,
            opacity: base.opacity,
            tint: base.tint,
            rim_intensity: base.rim_intensity,
            shadow: base.shadow,
            icon_scale: base.icon_scale,
            screen_frame: base.screen_frame,
            frame_thickness: base.frame_thickness,
            vertical_width: base.vertical_width,
            notch_width: base.notch_width,
            mono: false,
        };
        match self {
            BarStyle::Expressive => Look {
                layout: BarLayout::MinimalEdge,
                position: BarPosition::Bottom,
                height: 52.0,
                item_shape: BarItemShape::Tile,
                icon_shape: BarIconShape::Circle,
                icon_fill: BarIconFill::Accent,
                capsule_fill: BarCapsuleFill::Container,
                capsule_strength: 1.2,
                item_inset: 7.0,
                chip_padding: 12.0,
                chip_spacing: 12.0,
                corner_radius: 12.0,
                flare_radius: 24.0,
                screen_frame: true,
                frame_thickness: 7.0,
                // A tonal frame: a little of the accent in the bar's fill.
                tint: 0.12,
                icon_scale: 1.1,
                ..plain
            },
            BarStyle::TileDock => Look {
                layout: BarLayout::ContiguousIsland,
                position: BarPosition::Bottom,
                height: 56.0,
                item_shape: BarItemShape::Squircle,
                icon_shape: BarIconShape::Squircle,
                icon_fill: BarIconFill::Accent,
                capsule_fill: BarCapsuleFill::Container,
                capsule_strength: 1.2,
                item_inset: 7.0,
                chip_padding: 12.0,
                chip_spacing: 6.0,
                corner_radius: 22.0,
                edge_gap: 10.0,
                shadow: true,
                icon_scale: 1.3,
                ..plain
            },
            BarStyle::Rail => Look {
                layout: BarLayout::MinimalEdge,
                position: BarPosition::Left,
                item_shape: BarItemShape::Tile,
                icon_shape: BarIconShape::Flower,
                icon_fill: BarIconFill::Tertiary,
                capsule_fill: BarCapsuleFill::Container,
                capsule_strength: 1.2,
                item_inset: 6.0,
                chip_padding: 8.0,
                chip_spacing: 8.0,
                corner_radius: 14.0,
                flare_radius: 22.0,
                screen_frame: true,
                frame_thickness: 8.0,
                vertical_width: 56.0,
                icon_scale: 1.15,
                ..plain
            },
            BarStyle::Segmented => Look {
                layout: BarLayout::FloatingPills,
                position: BarPosition::Top,
                height: 36.0,
                item_shape: BarItemShape::Connected,
                capsule_fill: BarCapsuleFill::Container,
                capsule_strength: 1.2,
                chip_padding: 12.0,
                chip_spacing: 2.0,
                edge_gap: 6.0,
                opacity: 0.0,
                rim_intensity: 0.0,
                ..plain
            },
            // Stat blocks: square-cornered container tiles almost as tall
            // as the strip, packed tight, numbers in the monospace font.
            BarStyle::SystemMonitor => Look {
                layout: BarLayout::MinimalEdge,
                position: BarPosition::Bottom,
                height: 30.0,
                item_shape: BarItemShape::Tile,
                capsule_fill: BarCapsuleFill::Container,
                capsule_strength: 1.3,
                item_inset: 4.0,
                chip_padding: 9.0,
                chip_spacing: 3.0,
                corner_radius: 5.0,
                flare_radius: 10.0,
                mono: true,
                ..plain
            },
            BarStyle::StanleyBridge => Look {
                layout: BarLayout::FloatingPills,
                position: BarPosition::Bottom,
                height: 48.0,
                item_shape: BarItemShape::Squircle,
                icon_shape: BarIconShape::Squircle,
                icon_fill: BarIconFill::Tertiary,
                capsule_fill: BarCapsuleFill::Container,
                capsule_strength: 1.2,
                item_inset: 6.0,
                chip_padding: 11.0,
                chip_spacing: 6.0,
                corner_radius: 18.0,
                edge_gap: 10.0,
                shadow: true,
                icon_scale: 1.2,
                ..plain
            },
            BarStyle::PharosBeam => Look {
                layout: BarLayout::Notch,
                position: BarPosition::Top,
                height: 44.0,
                item_shape: BarItemShape::Tile,
                icon_shape: BarIconShape::Burst,
                icon_fill: BarIconFill::Accent,
                capsule_fill: BarCapsuleFill::Container,
                capsule_strength: 1.2,
                item_inset: 5.0,
                chip_padding: 10.0,
                chip_spacing: 6.0,
                corner_radius: 12.0,
                flare_radius: 20.0,
                tint: 0.12,
                shadow: true,
                icon_scale: 1.05,
                notch_width: 0.5,
                ..plain
            },
            // Icon items stay round: their padding keeps them within the
            // icon-sized share of the items' thickness.
            BarStyle::RamlTram => Look {
                layout: BarLayout::MinimalEdge,
                position: BarPosition::Top,
                height: 38.0,
                item_shape: BarItemShape::Tile,
                icon_shape: BarIconShape::Flower,
                icon_fill: BarIconFill::Tertiary,
                capsule_fill: BarCapsuleFill::Container,
                capsule_strength: 1.2,
                item_inset: 5.0,
                chip_padding: 7.0,
                chip_spacing: 6.0,
                corner_radius: 12.0,
                flare_radius: 22.0,
                tint: 0.06,
                icon_scale: 1.05,
                ..plain
            },
        }
    }

    /// Applies the style's layout, edge, height and look to `cfg` (its
    /// items stay; see [`Self::zones`]). A style set before is cleared
    /// first, so nothing of it lingers.
    pub fn apply(self, cfg: &mut ShellConfig) {
        clear(cfg);
        let l = self.look();
        let bar = &mut cfg.bar;
        bar.layout = l.layout;
        bar.position = l.position;
        bar.height = l.height;
        bar.item_capsules = true;
        bar.capsules = false;
        bar.item_shape = l.item_shape;
        bar.icon_shape = l.icon_shape;
        bar.icon_fill = l.icon_fill;
        bar.capsule_fill = l.capsule_fill;
        bar.capsule_strength = l.capsule_strength;
        bar.capsule_outline = false;
        bar.item_inset = l.item_inset;
        bar.chip_padding = l.chip_padding;
        bar.chip_spacing = l.chip_spacing;
        bar.corner_radius = l.corner_radius;
        bar.flare_radius = l.flare_radius;
        bar.edge_gap = l.edge_gap;
        bar.opacity = l.opacity;
        bar.tint = l.tint;
        bar.rim_intensity = l.rim_intensity;
        bar.shadow = l.shadow;
        bar.icon_scale = l.icon_scale;
        bar.screen_frame = l.screen_frame;
        bar.frame_thickness = l.frame_thickness;
        bar.vertical_width = l.vertical_width;
        bar.notch_width = l.notch_width;
        bar.floating_frame = false;
        bar.style = Some(self);
        if l.mono {
            cfg.typography.bar_font.clone_from(&cfg.typography.mono_font);
        }
        if let Some(rail) = self.rail_look() {
            let sb = &mut cfg.side_bars;
            sb.enabled = true;
            sb.joined = true;
            sb.chips = SideBarChips::Pills;
            sb.icon_shape = rail.icon_shape;
            sb.width = rail.width;
            sb.chip_size = rail.chip_size;
            sb.chip_spacing = rail.chip_spacing;
            sb.chip_fill = rail.chip_fill;
            sb.left.enabled = true;
            sb.left.pinned = true;
            sb.right.enabled = false;
        }
    }

    /// Whether `cfg`'s bar still looks as this style set it (layout, edge
    /// and item look; sizes may have been tuned since).
    pub fn matches(self, cfg: &ShellConfig) -> bool {
        let l = self.look();
        let bar = &cfg.bar;
        bar.layout == l.layout && bar.position == l.position && bar.item_capsules && bar.item_shape == l.item_shape && bar.icon_shape == l.icon_shape && bar.icon_fill == l.icon_fill
    }
}

/// One side bar's look in a style: item shapes, colour and inset.
struct SideLook {
    item_shape: BarItemShape,
    icon_shape: BarIconShape,
    icon_fill: BarIconFill,
    item_inset: f32,
}

impl BarStyle {
    fn side_look(self) -> SideLook {
        match self {
            BarStyle::Expressive => SideLook { item_shape: BarItemShape::Tile, icon_shape: BarIconShape::Cookie, icon_fill: BarIconFill::Accent, item_inset: 4.0 },
            BarStyle::TileDock => SideLook { item_shape: BarItemShape::Squircle, icon_shape: BarIconShape::Squircle, icon_fill: BarIconFill::Accent, item_inset: 3.0 },
            BarStyle::Rail => SideLook { item_shape: BarItemShape::Tile, icon_shape: BarIconShape::Flower, icon_fill: BarIconFill::Tertiary, item_inset: 4.0 },
            // Connected groups: icons stay in the group's shape.
            BarStyle::Segmented => SideLook { item_shape: BarItemShape::Connected, icon_shape: BarIconShape::Match, icon_fill: BarIconFill::Match, item_inset: 5.0 },
            // Not offered for a side bar alone (see `SIDE`), but each has
            // its own item look if a config names it.
            BarStyle::SystemMonitor => SideLook { item_shape: BarItemShape::Tile, icon_shape: BarIconShape::Match, icon_fill: BarIconFill::Match, item_inset: 3.0 },
            BarStyle::StanleyBridge => SideLook { item_shape: BarItemShape::Squircle, icon_shape: BarIconShape::Squircle, icon_fill: BarIconFill::Tertiary, item_inset: 3.0 },
            BarStyle::PharosBeam => SideLook { item_shape: BarItemShape::Tile, icon_shape: BarIconShape::Burst, icon_fill: BarIconFill::Accent, item_inset: 4.0 },
            BarStyle::RamlTram => SideLook { item_shape: BarItemShape::Tile, icon_shape: BarIconShape::Flower, icon_fill: BarIconFill::Tertiary, item_inset: 5.0 },
        }
    }

    /// Gives the left (`false`) or right side bar alone this style's item
    /// look: pill items in the style's shapes, colour and inset. A style
    /// it had before is cleared first; the other bars are untouched.
    pub fn apply_to_side(self, cfg: &mut ShellConfig, right: bool) {
        clear_side(cfg, right);
        let l = self.side_look();
        let side = cfg.side_bars.side_mut(right);
        side.chips = Some(SideBarChips::Pills);
        side.item_shape = Some(l.item_shape);
        side.icon_shape = Some(l.icon_shape);
        side.icon_fill = Some(l.icon_fill);
        side.item_inset = Some(l.item_inset);
        side.style = Some(self);
    }

    /// Whether that side bar still has this style's item look.
    pub fn side_matches(self, cfg: &ShellConfig, right: bool) -> bool {
        let l = self.side_look();
        let side = cfg.side_bars.side(right);
        side.chips == Some(SideBarChips::Pills) && side.item_shape == Some(l.item_shape) && side.icon_shape == Some(l.icon_shape) && side.icon_fill == Some(l.icon_fill)
    }
}

/// Clears what the style recorded on the left (`false`) or right side bar
/// set: the bar follows the shared side bar look again. Nothing happens
/// without a recorded style, so a side bar shaped by hand is kept.
pub fn clear_side(cfg: &mut ShellConfig, right: bool) {
    let side = cfg.side_bars.side_mut(right);
    if side.style.take().is_none() {
        return;
    }
    side.chips = None;
    side.item_shape = None;
    side.icon_shape = None;
    side.icon_fill = None;
    side.item_inset = None;
}

/// Puts back every field the bar's recorded style changed (to its
/// default) and forgets the style. Nothing happens without one, so a bar
/// you shaped yourself is never touched.
pub fn clear(cfg: &mut ShellConfig) {
    let Some(style) = cfg.bar.style.take() else { return };
    let d = BarConfig::default();
    let bar = &mut cfg.bar;
    bar.item_capsules = d.item_capsules;
    bar.item_shape = d.item_shape;
    bar.icon_shape = d.icon_shape;
    bar.icon_fill = d.icon_fill;
    bar.capsule_fill = d.capsule_fill;
    bar.capsule_strength = d.capsule_strength;
    bar.item_inset = d.item_inset;
    bar.chip_padding = d.chip_padding;
    bar.chip_spacing = d.chip_spacing;
    bar.corner_radius = d.corner_radius;
    bar.flare_radius = d.flare_radius;
    bar.edge_gap = d.edge_gap;
    bar.opacity = d.opacity;
    bar.tint = d.tint;
    bar.rim_intensity = d.rim_intensity;
    bar.shadow = d.shadow;
    bar.icon_scale = d.icon_scale;
    bar.screen_frame = d.screen_frame;
    bar.frame_thickness = d.frame_thickness;
    bar.vertical_width = d.vertical_width;
    bar.notch_width = d.notch_width;
    if style.look().mono {
        cfg.typography.bar_font = crate::TypographyConfig::default().bar_font;
    }
    if style.rail_look().is_some() {
        let d = crate::side_bars::SideBarsConfig::default();
        let sb = &mut cfg.side_bars;
        sb.enabled = d.enabled;
        sb.chips = d.chips;
        sb.icon_shape = d.icon_shape;
        sb.width = d.width;
        sb.chip_size = d.chip_size;
        sb.chip_spacing = d.chip_spacing;
        sb.chip_fill = d.chip_fill;
        sb.right.enabled = d.right.enabled;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_every_style_sets_its_look() {
        for style in BarStyle::ALL {
            assert_eq!(BarStyle::from_id(style.id()), Some(style));
            let mut cfg = ShellConfig::default();
            style.apply(&mut cfg);
            assert!(style.matches(&cfg), "{style:?}");
            assert_eq!(cfg.bar.style, Some(style));
            assert!(crate::surfaces::ranges::BAR_HEIGHT.clamp(style.height(), 0.0) == style.height(), "{style:?}: its height is in range");
            // No other style claims the same bar.
            for other in BarStyle::ALL.into_iter().filter(|o| *o != style) {
                assert!(!other.matches(&cfg), "{style:?} is not {other:?}");
            }
            assert!(style.zones().iter().any(|z| !z.is_empty()));
        }
        assert_eq!(BarStyle::from_id("nope"), None);
    }

    #[test]
    fn a_style_survives_a_save_and_reload() {
        for style in BarStyle::ALL {
            let mut cfg = ShellConfig::default();
            style.apply(&mut cfg);
            let text = toml::to_string(&cfg).expect("serializes");
            let value: toml::Value = toml::from_str(&text).expect("parses");
            assert_eq!(value["bar"]["style"].as_str(), Some(style.id()), "{style:?}");
            let back: ShellConfig = toml::from_str(&text).expect("parses");
            assert_eq!(back.bar, cfg.bar, "{style:?}: the bar round-trips");
            assert_eq!(back.side_bars, cfg.side_bars);
            assert!(style.matches(&back));
        }
        // A config without the new keys reads as the shipped bar.
        let plain: ShellConfig = toml::from_str("[bar]\nheight = 30\n").expect("parses");
        assert_eq!(plain.bar.style, None);
        assert_eq!(plain.bar.item_shape, BarItemShape::Capsule);
        assert_eq!(plain.bar.icon_shape, BarIconShape::Match);
        let value: toml::Value = toml::from_str(&toml::to_string(&ShellConfig::default()).expect("serializes")).expect("parses");
        assert!(value["bar"].get("style").is_none(), "no style key until one is picked");
    }

    #[test]
    fn clearing_puts_back_what_the_style_changed_and_nothing_else() {
        let mut cfg = ShellConfig::default();
        cfg.bar.text_scale = 1.2;
        cfg.bar.left = vec!["clock".into()];
        let before = cfg.clone();
        for style in BarStyle::ALL {
            let mut c = before.clone();
            style.apply(&mut c);
            clear(&mut c);
            assert_eq!(c.bar.style, None);
            // Layout, edge, height and items are the next preset's to set.
            c.bar.layout = before.bar.layout;
            c.bar.position = before.bar.position;
            c.bar.height = before.bar.height;
            c.side_bars.joined = before.side_bars.joined;
            c.side_bars.left = before.side_bars.left.clone();
            c.side_bars.right = before.side_bars.right.clone();
            assert_eq!(c.bar, before.bar, "{style:?}");
            assert_eq!(c.side_bars, before.side_bars, "{style:?}");
            assert_eq!(c.typography, before.typography, "{style:?}: the bar font comes back");
        }
        // A bar shaped by hand (no style recorded) is left alone.
        let mut hand = ShellConfig::default();
        hand.bar.item_shape = BarItemShape::Tile;
        let kept = hand.clone();
        clear(&mut hand);
        assert_eq!(hand, kept);
    }

    #[test]
    fn a_side_bar_takes_a_style_of_its_own_and_gives_it_back() {
        for style in BarStyle::SIDE {
            let mut cfg = ShellConfig::default();
            BarStyle::TileDock.apply(&mut cfg);
            let main = cfg.bar.clone();
            let before = cfg.side_bars.clone();
            style.apply_to_side(&mut cfg, true);
            assert!(style.side_matches(&cfg, true) && !style.side_matches(&cfg, false), "{style:?}: the right bar alone");
            assert_eq!(cfg.side_bars.left, before.left, "the other bar is untouched");
            assert_eq!(cfg.bar, main, "so is the main bar and its own style");
            let items = cfg.side_bars.items(true, &cfg.bar);
            assert_eq!(items.chips, SideBarChips::Pills);
            assert!(items.chip_size <= cfg.side_bars.width() && items.chip_size >= crate::side_bars::ranges::CHIP_SIZE.min);
            // It survives a save and reload.
            let back: ShellConfig = toml::from_str(&toml::to_string(&cfg).expect("serializes")).expect("parses");
            assert_eq!(back.side_bars, cfg.side_bars, "{style:?}");
            // Another style replaces it whole; none puts the shared look back.
            let other = BarStyle::SIDE.into_iter().find(|o| *o != style).unwrap();
            other.apply_to_side(&mut cfg, true);
            assert!(other.side_matches(&cfg, true) && !style.side_matches(&cfg, true));
            clear_side(&mut cfg, true);
            assert_eq!(cfg.side_bars, before, "{style:?}: back to the shared look");
            // Clearing the main bar's style leaves a side bar's own alone.
            style.apply_to_side(&mut cfg, false);
            clear(&mut cfg);
            assert!(style.side_matches(&cfg, false));
        }
        // A side bar shaped by hand (no style recorded) is never cleared.
        let mut hand = ShellConfig::default();
        hand.side_bars.left.item_shape = Some(BarItemShape::Tile);
        let kept = hand.clone();
        clear_side(&mut hand, false);
        assert_eq!(hand, kept);
        // Unset, a side bar follows the shared look and the bar's shapes.
        let mut cfg = ShellConfig::default();
        cfg.bar.item_shape = BarItemShape::Squircle;
        cfg.side_bars.icon_shape = BarIconShape::Burst;
        let items = cfg.side_bars.items(false, &cfg.bar);
        assert_eq!((items.item_shape, items.icon_shape, items.chips, items.chip_size), (BarItemShape::Squircle, BarIconShape::Burst, cfg.side_bars.chips, cfg.side_bars.chip_size()));
        // An old config without the new keys reads unchanged.
        let old: ShellConfig = toml::from_str("[side_bars.left]\nenabled = true\n").expect("parses");
        assert_eq!(old.side_bars.left.style, None);
        assert_eq!(old.side_bars.left.item_shape, None);
    }

    #[test]
    fn the_new_styles_set_their_own_signature() {
        // System monitor: a slim bottom status line in the monospace font.
        let mut cfg = ShellConfig::default();
        cfg.typography.mono_font = "Iosevka".into();
        BarStyle::SystemMonitor.apply(&mut cfg);
        assert_eq!((cfg.bar.layout, cfg.bar.position, cfg.bar.item_shape), (BarLayout::MinimalEdge, BarPosition::Bottom, BarItemShape::Tile));
        assert_eq!(cfg.typography.bar_font, "Iosevka");
        assert!(!cfg.side_bars.enabled && !cfg.bar.screen_frame);
        // Pharos Beam: a narrower notch than the shipped one.
        BarStyle::PharosBeam.apply(&mut cfg);
        assert_eq!(cfg.typography.bar_font, "", "the monospace bar went with System monitor");
        assert_eq!(cfg.bar.layout, BarLayout::Notch);
        assert!(cfg.bar.notch_width() < BarConfig::default().notch_width());
        // Raml Tram: a top bar joined to a flower rail on the left.
        BarStyle::RamlTram.apply(&mut cfg);
        assert_eq!(cfg.bar.notch_width, BarConfig::default().notch_width);
        assert_eq!(cfg.bar.position, BarPosition::Top);
        assert!(cfg.side_bar_on(false) && !cfg.side_bar_on(true) && cfg.side_bars_join());
        assert_eq!(cfg.side_bars.icon_shape, BarIconShape::Flower);
        assert_eq!(cfg.side_bars.chips, SideBarChips::Pills);
        assert!(BarStyle::RamlTram.rail().is_some());
        // Stanley Bridge: three floating docks, the rail gone.
        BarStyle::StanleyBridge.apply(&mut cfg);
        assert!(!cfg.side_bars.enabled);
        assert_eq!((cfg.bar.layout, cfg.bar.position, cfg.bar.icon_fill), (BarLayout::FloatingPills, BarPosition::Bottom, BarIconFill::Tertiary));
        // A bar font you chose yourself is never cleared without a style.
        let mut hand = ShellConfig::default();
        hand.typography.bar_font = "Cairo".into();
        clear(&mut hand);
        assert_eq!(hand.typography.bar_font, "Cairo");
        // Only the first four take a side bar on their own.
        for style in BarStyle::ALL {
            assert_eq!(BarStyle::from_side_id(style.id()).is_some(), BarStyle::SIDE.contains(&style));
        }
    }

    #[test]
    fn switching_styles_leaves_nothing_of_the_last_one() {
        let mut cfg = ShellConfig::default();
        BarStyle::Expressive.apply(&mut cfg);
        assert!(cfg.side_bars.enabled && cfg.bar.screen_frame);
        BarStyle::Segmented.apply(&mut cfg);
        assert!(!cfg.side_bars.enabled, "Expressive's rail went with it");
        assert!(!cfg.bar.screen_frame);
        assert_eq!(cfg.side_bars.icon_shape, BarIconShape::Match);
        assert!(BarStyle::Segmented.matches(&cfg));
    }
}
