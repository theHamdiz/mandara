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
//! A style that keeps your items still makes them fit it, and remembers
//! what it did (`bar.style_items`) so [`clear`] gives it back: a style with
//! a rail takes the items the rail shows off the bar (each shows once: the
//! workspaces run down the rail), and a style made of particular items
//! (System monitor's meters) places those and switches their plugins on.
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
    /// A full-width bottom taskbar: the launcher and your pinned and
    /// running apps centred, the status cluster and a stacked time and
    /// date on the right, on a soft translucent surface.
    Taskbar,
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

/// A zone of the bar (start, middle, end) or of the left rail (top,
/// middle, bottom).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ItemZone {
    Start,
    Middle,
    End,
    RailTop,
    RailMiddle,
    RailBottom,
    /// The right side bar's zones.
    RightTop,
    RightMiddle,
    RightBottom,
}

impl ItemZone {
    pub const BAR: [ItemZone; 3] = [ItemZone::Start, ItemZone::Middle, ItemZone::End];
    pub const RAIL: [ItemZone; 3] = [ItemZone::RailTop, ItemZone::RailMiddle, ItemZone::RailBottom];
    pub const RIGHT: [ItemZone; 3] = [ItemZone::RightTop, ItemZone::RightMiddle, ItemZone::RightBottom];
    /// Every zone an item can sit in, bar first.
    pub const ALL: [ItemZone; 9] = [
        ItemZone::Start,
        ItemZone::Middle,
        ItemZone::End,
        ItemZone::RailTop,
        ItemZone::RailMiddle,
        ItemZone::RailBottom,
        ItemZone::RightTop,
        ItemZone::RightMiddle,
        ItemZone::RightBottom,
    ];

    pub fn list(self, cfg: &mut ShellConfig) -> &mut Vec<String> {
        match self {
            ItemZone::Start => &mut cfg.bar.left,
            ItemZone::Middle => &mut cfg.bar.center,
            ItemZone::End => &mut cfg.bar.right,
            ItemZone::RailTop => &mut cfg.side_bars.left.top,
            ItemZone::RailMiddle => &mut cfg.side_bars.left.middle,
            ItemZone::RailBottom => &mut cfg.side_bars.left.bottom,
            ItemZone::RightTop => &mut cfg.side_bars.right.top,
            ItemZone::RightMiddle => &mut cfg.side_bars.right.middle,
            ItemZone::RightBottom => &mut cfg.side_bars.right.bottom,
        }
    }

    pub fn get(self, cfg: &ShellConfig) -> &Vec<String> {
        match self {
            ItemZone::Start => &cfg.bar.left,
            ItemZone::Middle => &cfg.bar.center,
            ItemZone::End => &cfg.bar.right,
            ItemZone::RailTop => &cfg.side_bars.left.top,
            ItemZone::RailMiddle => &cfg.side_bars.left.middle,
            ItemZone::RailBottom => &cfg.side_bars.left.bottom,
            ItemZone::RightTop => &cfg.side_bars.right.top,
            ItemZone::RightMiddle => &cfg.side_bars.right.middle,
            ItemZone::RightBottom => &cfg.side_bars.right.bottom,
        }
    }

    /// The bar zone in the same place along its bar (top / start, middle,
    /// bottom / end): where an item goes when its side bar stops showing.
    pub fn on_bar(self) -> ItemZone {
        match self {
            ItemZone::Start | ItemZone::RailTop | ItemZone::RightTop => ItemZone::Start,
            ItemZone::Middle | ItemZone::RailMiddle | ItemZone::RightMiddle => ItemZone::Middle,
            ItemZone::End | ItemZone::RailBottom | ItemZone::RightBottom => ItemZone::End,
        }
    }

    /// Whether the zone shows on `cfg` (the bar always does; a side bar's
    /// zones while that side bar is on).
    pub fn shows(self, cfg: &ShellConfig) -> bool {
        match self {
            ItemZone::Start | ItemZone::Middle | ItemZone::End => true,
            ItemZone::RailTop | ItemZone::RailMiddle | ItemZone::RailBottom => cfg.side_bar_on(false),
            ItemZone::RightTop | ItemZone::RightMiddle | ItemZone::RightBottom => cfg.side_bar_on(true),
        }
    }
}

/// One item a style moved: where it was, or where it was put.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemPlace {
    pub id: String,
    pub zone: ItemZone,
    pub index: usize,
}

/// What a style did to your items when it kept them (`[bar.style_items]`),
/// so the next preset, a Look or none puts them back ([`clear`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StyleItems {
    /// The style has fitted your items to itself (set by [`BarStyle::apply`],
    /// and by [`upgrade`] for a style picked before it did).
    pub settled: bool,
    /// Items taken off the bar because the style's rail shows them.
    pub taken: Vec<ItemPlace>,
    /// Items it placed: its signature, the workspaces on its rail, an item
    /// of its own where the rail freed room.
    pub added: Vec<ItemPlace>,
    /// Plugins it switched on, or took out of `plugins.disabled`.
    pub enabled: Vec<String>,
    pub undisabled: Vec<String>,
}

impl StyleItems {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The workspaces plugin, which a style's rail holds.
const WORKSPACES: &str = "workspaces";
/// The launcher, which a style with a rail seats in the corner.
const LAUNCHER: &str = "launcher";
/// The taskbar item: pinned and running apps in the bar.
pub const TASKBAR: &str = "taskbar";

/// Whether `id` is on the bar.
fn on_bar(cfg: &ShellConfig, id: &str) -> bool {
    [&cfg.bar.left, &cfg.bar.center, &cfg.bar.right].into_iter().any(|z| z.iter().any(|x| x == id))
}

/// Whether `id` is on the left rail.
fn on_rail(cfg: &ShellConfig, id: &str) -> bool {
    cfg.side_bars.left.zones().into_iter().any(|z| z.iter().any(|x| x == id))
}

/// Whether `id` is still on the bar or a side bar once the style's own
/// placements are gone, or a desktop widget needs its plugin: you put it
/// there yourself, so it keeps its plugin. (A sidebar card doesn't count:
/// it was there, switched off, before.)
pub(crate) fn placed_by_you(cfg: &ShellConfig, id: &str) -> bool {
    placed(cfg, id)
}

fn placed(cfg: &ShellConfig, id: &str) -> bool {
    on_bar(cfg, id) || [&cfg.side_bars.left, &cfg.side_bars.right].into_iter().any(|s| s.zones().into_iter().any(|z| z.iter().any(|x| x == id))) || cfg.desktop.widgets.items.iter().any(|w| w.kind.plugin() == id)
}

/// Puts `id` into `zone` at `index` (at most its end) for the style, its
/// plugin switched on, and records both. A plugin you removed stays away.
fn add(cfg: &mut ShellConfig, items: &mut StyleItems, id: &str, zone: ItemZone, index: usize) {
    if cfg.plugins.is_removed(id) {
        return;
    }
    let list = zone.list(cfg);
    let index = index.min(list.len());
    list.insert(index, id.to_string());
    items.added.push(ItemPlace { id: id.to_string(), zone, index });
    if cfg.plugins.disabled.iter().any(|d| d == id) {
        cfg.plugins.disabled.retain(|d| d != id);
        items.undisabled.push(id.to_string());
    }
    if !cfg.plugins.enabled.iter().any(|e| e == id) {
        cfg.plugins.enabled.push(id.to_string());
        items.enabled.push(id.to_string());
    }
}

impl BarStyle {
    /// Fits the bar's items (kept, not replaced) to the style: its rail
    /// holds the workspaces and the bar no longer shows what the rail
    /// does; where that freed room at the bar's start, the first of the
    /// preset's own start items that is on neither comes in (`fill`); its
    /// signature items come in too.
    fn fit_items(self, cfg: &mut ShellConfig, fill: bool) -> StyleItems {
        let mut items = StyleItems { settled: true, ..StyleItems::default() };
        if let Some(rail) = self.rail() {
            if !on_rail(cfg, WORKSPACES) {
                let (zone, index) = ItemZone::RAIL.into_iter().zip(rail).find_map(|(z, ids)| ids.iter().position(|id| *id == WORKSPACES).map(|i| (z, i))).unwrap_or((ItemZone::RailTop, 0));
                add(cfg, &mut items, WORKSPACES, zone, index);
            }
            for zone in ItemZone::BAR {
                let before = std::mem::take(zone.list(cfg));
                let mut kept = Vec::with_capacity(before.len());
                for (index, id) in before.into_iter().enumerate() {
                    if on_rail(cfg, &id) {
                        items.taken.push(ItemPlace { id, zone, index });
                    } else {
                        kept.push(id);
                    }
                }
                *zone.list(cfg) = kept;
            }
            let freed = items.taken.iter().find(|t| t.zone == ItemZone::Start).map(|t| t.index);
            if let Some(index) = freed.filter(|_| fill) {
                // Not the launcher: it belongs in the corner, first, and a
                // bar you keep without one stays without one.
                let own = self.zones()[0].iter().find(|id| **id != LAUNCHER && !on_bar(cfg, id) && !on_rail(cfg, id) && !cfg.plugins.is_removed(id));
                if let Some(id) = own {
                    add(cfg, &mut items, id, ItemZone::Start, index);
                }
            }
        }
        for ((zone, ids), own) in ItemZone::BAR.into_iter().zip(self.signature()).zip(self.zones()) {
            for id in ids {
                if !on_bar(cfg, id) && !on_rail(cfg, id) {
                    add(cfg, &mut items, id, zone, own.iter().position(|x| x == id).unwrap_or(usize::MAX));
                }
            }
        }
        items
    }
}

/// Brings a config whose style was picked before styles fitted the items
/// they keep up to date: what its rail shows leaves the bar (the
/// workspaces showed twice) and its signature items come (System monitor
/// came without its meters). Returns whether anything changed.
pub fn upgrade(cfg: &mut ShellConfig) -> bool {
    let Some(style) = cfg.bar.style else { return false };
    if cfg.bar.style_items.settled {
        return false;
    }
    // The rail only fits the items while it shows.
    if style.rail().is_some() && !cfg.side_bar_on(false) {
        return false;
    }
    cfg.bar.style_items = style.fit_items(cfg, false);
    true
}

impl BarStyle {
    pub const ALL: [BarStyle; 9] = [
        BarStyle::Expressive,
        BarStyle::TileDock,
        BarStyle::Rail,
        BarStyle::Segmented,
        BarStyle::SystemMonitor,
        BarStyle::StanleyBridge,
        BarStyle::PharosBeam,
        BarStyle::RamlTram,
        BarStyle::Taskbar,
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
            BarStyle::Taskbar => "taskbar",
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

    /// The bar's items when a preset replaces them. A style with a rail
    /// starts its bar with the launcher: it sits in the corner where the
    /// bar meets the rail.
    pub const fn zones(self) -> Zones {
        match self {
            BarStyle::Expressive => [&["launcher", "screenshot", "clipboard", "color-picker", "wallpaper", "keybinds"], &["weather", "media", "drives", "pomodoro"], &["tray", "notifications", "session"]],
            BarStyle::TileDock => [&["launcher", "screenshot", "clipboard", "color-picker"], &["workspaces", "media"], &["clock", "audio", "notifications", "session"]],
            BarStyle::Rail => [&["launcher", "workspaces"], &["clock"], &["audio", "network", "notifications", "session"]],
            BarStyle::Segmented => [&["launcher", "workspaces", "active-window"], &["clock", "media"], &["network", "bluetooth", "audio", "notifications", "session"]],
            BarStyle::SystemMonitor => [&["launcher", "workspaces", "processes"], &["mini-stats", "resources", "net-speed"], &["drives", "clock", "audio", "session"]],
            BarStyle::StanleyBridge => [&["launcher", "workspaces"], &["media", "clock"], &["audio", "network", "bluetooth", "notifications", "session"]],
            BarStyle::PharosBeam => [&["launcher", "workspaces"], &["clock", "media"], &["weather", "audio", "notifications", "session"]],
            BarStyle::RamlTram => [&["launcher", "active-window"], &["clock", "media"], &["weather", "tray", "bluetooth", "notifications"]],
            // The launcher opens the centred group, your apps follow it.
            BarStyle::Taskbar => [&["workspaces", "weather"], &["launcher", TASKBAR], &["tray", "network", "audio", "power", "clock", "notifications"]],
        }
    }

    /// The left side bar's groups (top, middle, bottom) the style joins to
    /// the bar, when a preset replaces items. The workspaces live here and
    /// nowhere else.
    pub const fn rail(self) -> Option<Zones> {
        match self {
            BarStyle::Expressive => Some([&[], &["clock", "workspaces"], &["audio", "network", "bluetooth"]]),
            BarStyle::RamlTram => Some([&["workspaces"], &[], &["network", "audio", "session"]]),
            _ => None,
        }
    }

    /// The items a style is made of: they come with it even when your
    /// items stay (in the zone the preset gives them), their plugins
    /// switched on, and go with it again. System monitor is its stat
    /// blocks, meters and rates.
    pub const fn signature(self) -> Zones {
        match self {
            BarStyle::SystemMonitor => [&["processes"], &["mini-stats", "resources", "net-speed"], &["drives"]],
            BarStyle::Taskbar => [&[], &[TASKBAR], &[]],
            _ => [&[], &[], &[]],
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
            // A soft translucent strip: hover-light squircles, no rim.
            BarStyle::Taskbar => Look {
                layout: BarLayout::MinimalEdge,
                position: BarPosition::Bottom,
                height: 48.0,
                item_shape: BarItemShape::Squircle,
                capsule_fill: BarCapsuleFill::Soft,
                capsule_strength: 0.7,
                item_inset: 6.0,
                chip_padding: 10.0,
                chip_spacing: 4.0,
                corner_radius: 8.0,
                flare_radius: 8.0,
                opacity: 0.86,
                tint: 0.05,
                rim_intensity: 0.35,
                icon_scale: 1.15,
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
        self.apply_look(cfg);
        cfg.bar.style_items = self.fit_items(cfg, true);
    }

    /// Applies the style's look like [`Self::apply`] but leaves the items
    /// to the caller (a Look, or a preset that replaces the items, lays
    /// them out itself: [`crate::bar_items::lay_out`]).
    pub fn apply_look(self, cfg: &mut ShellConfig) {
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
        cfg.bar.style_items = StyleItems { settled: true, ..StyleItems::default() };
    }

    /// The style's own items as a layout: its bar zones and its rail.
    pub fn items_layout(self) -> crate::bar_items::Layout {
        crate::bar_items::Layout::from_ids(self.zones(), self.rail())
    }

    /// Switches on the plugins of the style's signature items that `placed`
    /// holds, recorded so [`clear`] switches them off again.
    pub fn switch_on_signature(self, cfg: &mut ShellConfig, placed: &[String]) {
        let ids: Vec<&str> = self.signature().into_iter().flatten().copied().filter(|id| placed.iter().any(|p| p == id)).collect();
        switch_on(cfg, &ids);
    }

    /// Switches on the plugins of every item in `placed` (a preset that
    /// replaces the items), recorded like [`Self::switch_on_signature`].
    pub fn switch_on_placed(self, cfg: &mut ShellConfig, placed: &[String]) {
        let ids: Vec<&str> = placed.iter().map(String::as_str).collect();
        switch_on(cfg, &ids);
    }
}

/// Switches `ids` on for the style, recorded in `bar.style_items`.
fn switch_on(cfg: &mut ShellConfig, ids: &[&str]) {
    for id in ids.iter().copied() {
        if cfg.plugins.disabled.iter().any(|d| d == id) {
            cfg.plugins.disabled.retain(|d| d != id);
            cfg.bar.style_items.undisabled.push(id.to_string());
        }
        if !cfg.plugins.enabled.iter().any(|e| e == id) {
            cfg.plugins.enabled.push(id.to_string());
            cfg.bar.style_items.enabled.push(id.to_string());
        }
    }
}

impl BarStyle {
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
            BarStyle::Taskbar => SideLook { item_shape: BarItemShape::Squircle, icon_shape: BarIconShape::Match, icon_fill: BarIconFill::Match, item_inset: 4.0 },
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

/// Undoes what the style did to your items ([`StyleItems`]): what it
/// added leaves, what it took off the bar comes back where it was, and the
/// plugins it switched on go off again unless you placed them since.
fn give_back_items(cfg: &mut ShellConfig) {
    let items = std::mem::take(&mut cfg.bar.style_items);
    for a in &items.added {
        let list = a.zone.list(cfg);
        if let Some(i) = list.iter().position(|x| *x == a.id) {
            list.remove(i);
        }
    }
    // In the order they were taken (zone by zone, by index), so each lands
    // where it was.
    for t in &items.taken {
        if !on_bar(cfg, &t.id) {
            let list = t.zone.list(cfg);
            list.insert(t.index.min(list.len()), t.id.clone());
        }
    }
    // An item a Look or preset laid out goes with it: it doesn't keep the
    // plugin on (yours do).
    let kept = |cfg: &ShellConfig, id: &str| placed(cfg, id) && (cfg.bar.yours.iter().any(|y| y == id) || !cfg.bar.layout_items.as_ref().is_some_and(|l| l.iter().any(|p| p.id == id)) || cfg.desktop.widgets.items.iter().any(|w| w.kind.plugin() == id));
    for id in &items.enabled {
        if !kept(cfg, id) {
            cfg.plugins.enabled.retain(|e| e != id);
        }
    }
    for id in &items.undisabled {
        if !kept(cfg, id) && !cfg.plugins.enabled.iter().any(|e| e == id) && !cfg.plugins.disabled.iter().any(|d| d == id) {
            cfg.plugins.disabled.push(id.clone());
        }
    }
}

/// Puts back every field the bar's recorded style changed (to its
/// default) and forgets the style. Nothing happens without one, so a bar
/// you shaped yourself is never touched.
pub fn clear(cfg: &mut ShellConfig) {
    let Some(style) = cfg.bar.style.take() else { return };
    give_back_items(cfg);
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

    /// Every item the bar and the shown left rail place, in order.
    fn shown(cfg: &ShellConfig) -> Vec<String> {
        let rail = cfg.side_bar_on(false).then(|| cfg.side_bars.left.zones()).into_iter().flatten();
        [&cfg.bar.left, &cfg.bar.center, &cfg.bar.right].into_iter().chain(rail).flatten().cloned().collect()
    }

    fn twice(list: &[String]) -> Vec<&String> {
        list.iter().enumerate().filter(|(i, id)| list[..*i].contains(id)).map(|(_, id)| id).collect()
    }

    fn ids(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_style_places_an_item_twice() {
        for style in BarStyle::ALL {
            // Its own items: the bar's zones and the rail's groups.
            let rail = style.rail().unwrap_or_default();
            let own: Vec<String> = style.zones().into_iter().chain(rail).flatten().map(|s| s.to_string()).collect();
            assert!(twice(&own).is_empty(), "{style:?} places {:?} twice", twice(&own));
            assert!(style.signature().into_iter().flatten().all(|id| own.iter().any(|o| o == id)), "{style:?}: its signature is part of its items");
            // Kept items: the shipped bar, and one that already has the
            // rail's items and the signature (nothing to add).
            let mut busy = ShellConfig::default();
            busy.bar.right.extend(ids(&["processes", "mini-stats", "net-speed", "drives", "wallpaper"]));
            for start in [ShellConfig::default(), busy] {
                let mut cfg = start.clone();
                style.apply(&mut cfg);
                let list = shown(&cfg);
                assert!(twice(&list).is_empty(), "{style:?} keeping your items shows {:?} twice", twice(&list));
                // Once more over itself: still once each.
                style.apply(&mut cfg);
                assert!(twice(&shown(&cfg)).is_empty(), "{style:?} picked again");
            }
        }
    }

    #[test]
    fn a_rail_style_moves_the_workspaces_to_its_rail_and_gives_them_back() {
        for style in [BarStyle::Expressive, BarStyle::RamlTram] {
            let before = ShellConfig::default();
            let mut cfg = before.clone();
            style.apply(&mut cfg);
            assert!(on_rail(&cfg, WORKSPACES) && !on_bar(&cfg, WORKSPACES), "{style:?}: the workspaces run down the rail only");
            assert!(!on_bar(&cfg, "screenshot"), "{style:?}: nor does anything else the rail shows stay on the bar");
            assert_eq!(cfg.bar.left.first().map(String::as_str), Some("launcher"), "{style:?}: the launcher stays first, in the corner");
            // The freed room: the first of the preset's own start items
            // that is on neither bar, recorded with its plugin.
            let fill = style.zones()[0].iter().find(|id| **id != LAUNCHER && !on_bar(&before, id) && !on_rail(&before, id)).copied();
            assert_eq!(cfg.bar.style_items.added.iter().find(|a| a.zone == ItemZone::Start).map(|a| a.id.as_str()), fill, "{style:?}");
            let text = toml::to_string(&cfg).expect("serializes");
            let mut back: ShellConfig = toml::from_str(&text).expect("parses");
            assert_eq!(back.bar, cfg.bar, "{style:?}: the record survives a reload");
            // Leaving it gives every item back where it was.
            clear(&mut back);
            assert_eq!((&back.bar.left, &back.bar.center, &back.bar.right), (&before.bar.left, &before.bar.center, &before.bar.right), "{style:?}");
            assert_eq!(back.plugins, before.plugins, "{style:?}: the plugins it switched on go off again");
            assert!(back.bar.style_items.is_empty());
        }
        // A bar you keep without a launcher stays without one.
        let mut bare = ShellConfig::default();
        bare.bar.left.retain(|id| id != LAUNCHER);
        BarStyle::Expressive.apply(&mut bare);
        assert!(!on_bar(&bare, LAUNCHER) && !on_rail(&bare, LAUNCHER));
        // A rail without the workspaces gets them where the style puts them.
        let mut cfg = ShellConfig::default();
        cfg.side_bars.left.top = ids(&["clock"]);
        BarStyle::RamlTram.apply(&mut cfg);
        assert_eq!(cfg.side_bars.left.top, ["workspaces", "clock"]);
        assert!(!on_bar(&cfg, WORKSPACES) && !on_bar(&cfg, "clock"));
        clear(&mut cfg);
        assert_eq!(cfg.side_bars.left.top, ["clock"]);
        assert_eq!(cfg.bar.left, ShellConfig::default().bar.left);
    }

    #[test]
    fn system_monitor_brings_its_meters_and_switches_them_on() {
        let before = ShellConfig::default();
        let mut cfg = before.clone();
        BarStyle::SystemMonitor.apply(&mut cfg);
        for (zone, ids) in [&cfg.bar.left, &cfg.bar.center, &cfg.bar.right].into_iter().zip(BarStyle::SystemMonitor.signature()) {
            for id in ids {
                // One you have already stays where you put it (the shipped
                // bar's resources, on its end).
                assert!(zone.iter().any(|x| x == id) || on_bar(&before, id), "{id} in its zone");
                assert_eq!(shown(&cfg).iter().filter(|x| *x == id).count(), 1, "{id} once");
            }
        }
        // The opt-in meters are switched on; nothing else is.
        let switched: Vec<&str> = cfg.plugins.enabled.iter().filter(|id| !before.plugins.enabled.contains(id)).map(String::as_str).collect();
        let meters: Vec<&str> = ["processes", "mini-stats", "resources", "net-speed", "drives"].into_iter().filter(|id| !on_bar(&before, id) && !before.plugins.enabled.iter().any(|e| e == id)).collect();
        assert_eq!(switched, meters);
        assert!(cfg.bar.center.iter().position(|x| x == "mini-stats") < cfg.bar.center.iter().position(|x| x == "net-speed"), "in the preset's order");
        // Picking another style takes them away again.
        BarStyle::PharosBeam.apply(&mut cfg);
        for id in BarStyle::SystemMonitor.signature().into_iter().flatten() {
            assert_eq!(on_bar(&cfg, id), on_bar(&before, id), "{id}");
        }
        assert_eq!(cfg.plugins, before.plugins);
        // A meter you switched off comes on with it and goes off after; one
        // you removed stays away; one you placed since stays on.
        let mut mine = ShellConfig::default();
        mine.bar.right.retain(|id| id != "resources");
        mine.plugins.disabled.push("resources".into());
        mine.plugins.removed.push("drives".into());
        let kept = mine.clone();
        BarStyle::SystemMonitor.apply(&mut mine);
        assert!(on_bar(&mine, "resources") && !mine.plugins.disabled.iter().any(|d| d == "resources"));
        assert!(!on_bar(&mine, "drives") && !mine.plugins.enabled.iter().any(|e| e == "drives"));
        mine.side_bars.right.top.push("net-speed".into());
        mine.sidebar.tabs[0].cards.push("mini-stats".into());
        clear(&mut mine);
        assert!(mine.plugins.disabled.iter().any(|d| d == "resources"), "switched off again");
        assert!(mine.plugins.enabled.iter().any(|e| e == "net-speed"), "the side bar you put it on keeps it on");
        assert!(!mine.plugins.enabled.iter().any(|e| e == "mini-stats"), "a sidebar card doesn't");
        assert_eq!((&mine.bar.left, &mine.bar.center, &mine.bar.right), (&kept.bar.left, &kept.bar.center, &kept.bar.right));
    }

    #[test]
    fn a_style_picked_before_is_fitted_once_on_upgrade() {
        // As picking a rail style left a config: the workspaces on the bar
        // and on the rail, nothing recorded.
        let mut old = ShellConfig::default();
        BarStyle::Expressive.apply(&mut old);
        give_back_items(&mut old);
        assert!(on_bar(&old, WORKSPACES) && on_rail(&old, WORKSPACES) && !old.bar.style_items.settled);
        let text = toml::to_string(&old).expect("serializes");
        assert!(!text.contains("style_items"), "nothing recorded yet");
        let mut cfg: ShellConfig = toml::from_str(&text).expect("parses");
        assert!(upgrade(&mut cfg));
        assert!(twice(&shown(&cfg)).is_empty() && on_rail(&cfg, WORKSPACES) && !on_bar(&cfg, WORKSPACES));
        assert!(cfg.bar.style_items.added.is_empty(), "no new item on upgrade");
        assert!(!upgrade(&mut cfg), "once");
        // The workspaces you put back on the bar yourself afterwards stay.
        cfg.bar.left.insert(1, WORKSPACES.into());
        assert!(!upgrade(&mut cfg));
        assert!(on_bar(&cfg, WORKSPACES));
        // Leaving the style puts the bar back as it was.
        let mut fitted: ShellConfig = toml::from_str(&text).expect("parses");
        crate::upgrade::upgrade(&mut fitted);
        clear(&mut fitted);
        assert_eq!(fitted.bar.left, ShellConfig::default().bar.left);
        // System monitor picked before gets its meters.
        let mut monitor = ShellConfig::default();
        BarStyle::SystemMonitor.apply(&mut monitor);
        give_back_items(&mut monitor);
        assert!(!on_bar(&monitor, "net-speed"));
        assert!(upgrade(&mut monitor));
        assert!(on_bar(&monitor, "net-speed") && monitor.plugins.enabled.iter().any(|e| e == "net-speed"));
        // No style, or a rail switched off: nothing to do.
        assert!(!upgrade(&mut ShellConfig::default()));
        let mut off = ShellConfig::default();
        BarStyle::RamlTram.apply(&mut off);
        give_back_items(&mut off);
        off.side_bars.left.enabled = false;
        assert!(!upgrade(&mut off));
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
