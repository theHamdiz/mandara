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
}

impl BarStyle {
    pub const ALL: [BarStyle; 4] = [BarStyle::Expressive, BarStyle::TileDock, BarStyle::Rail, BarStyle::Segmented];

    /// The id used by `bar style` and in the config.
    pub fn id(self) -> &'static str {
        match self {
            BarStyle::Expressive => "expressive",
            BarStyle::TileDock => "tile-dock",
            BarStyle::Rail => "rail",
            BarStyle::Segmented => "segmented",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.id() == id)
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
        }
    }

    /// The left side bar's groups (top, middle, bottom) the style joins to
    /// the bar, when a preset replaces items.
    pub const fn rail(self) -> Option<Zones> {
        match self {
            BarStyle::Expressive => Some([&["launcher"], &["clock", "workspaces"], &["audio", "network", "bluetooth"]]),
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
        bar.floating_frame = false;
        bar.style = Some(self);
        if self == BarStyle::Expressive {
            let sb = &mut cfg.side_bars;
            sb.enabled = true;
            sb.joined = true;
            sb.chips = SideBarChips::Pills;
            sb.icon_shape = BarIconShape::Cookie;
            sb.width = RAIL_WIDTH_PX;
            sb.chip_size = RAIL_CHIP_PX;
            sb.chip_spacing = RAIL_SPACING_PX;
            sb.chip_fill = RAIL_FILL;
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

/// The rail Expressive joins to its bar: width, item size and spacing,
/// and the strength of its items' wash.
const RAIL_WIDTH_PX: f32 = 42.0;
const RAIL_CHIP_PX: f32 = 32.0;
const RAIL_SPACING_PX: f32 = 6.0;
const RAIL_FILL: f32 = 1.6;

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
    if style == BarStyle::Expressive {
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
        }
        // A bar shaped by hand (no style recorded) is left alone.
        let mut hand = ShellConfig::default();
        hand.bar.item_shape = BarItemShape::Tile;
        let kept = hand.clone();
        clear(&mut hand);
        assert_eq!(hand, kept);
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
