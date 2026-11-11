//! Studio → Bars: the drag-and-drop layout designer and bar
//! presets. The bar's left / center / right zones (and the side bars'
//! groups while they're on) are columns of items stacked in order: drag
//! them to reorder or move them, × takes one off, a click opens a menu
//! (move up / down, move to another zone, remove). Under them, the plugin
//! pool ([`super::plugin_pool`]) finds and adds plugins. Or start from a
//! preset. Every edit writes the config, so the live shell follows
//! instantly.

use mandara_ui::bidi::Bidi as _;
use gpui::{AnyElement, App, AppContext as _, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px};
use mandara_config::bar_styles::BarStyle;
use mandara_config::side_bars::SideZone;
use mandara_config::{BarLayout, BarPosition, ShellConfig};
use mandara_core::ToGpui;
use mandara_i18n::{t, tk, tr};
use mandara_plugins::PluginRegistry;
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::plugin_pool::{Menu, MenuFor, zone_title};
use super::{StudioView, plugin_icon, plugin_tile};
use crate::tokens::*;

mod previews;

/// A style-only apply keeps its identity even when several presets share
/// a layout. Manual item, layout or position edits retire that selection.
#[derive(Default)]
pub(super) struct PresetSelection {
    chosen: Option<ChosenPreset>,
}

struct ChosenPreset {
    index: usize,
    layout: BarLayout,
    position: BarPosition,
    style: Option<BarStyle>,
    items: [Vec<String>; 3],
}

impl PresetSelection {
    fn remember(&mut self, index: usize, config: &ShellConfig) {
        let bar = &config.bar;
        self.chosen = Some(ChosenPreset { index, layout: bar.layout, position: bar.position, style: bar.style, items: [bar.left.clone(), bar.center.clone(), bar.right.clone()] });
    }

    fn active(&mut self, config: &ShellConfig) -> Option<usize> {
        let bar = &config.bar;
        if let Some(chosen) = &self.chosen {
            if chosen.layout == bar.layout && chosen.position == bar.position && chosen.style == bar.style && chosen.items.iter().zip([&bar.left, &bar.center, &bar.right]).all(|(saved, current)| saved == current) {
                return Some(chosen.index);
            }
            self.chosen = None;
        }
        PRESETS.iter().position(|preset| {
            let look = match preset.style {
                Some(style) => bar.style == Some(style) && style.matches(config),
                None => bar.style.is_none(),
            };
            look && bar.layout == preset.layout && bar.position == preset.position && bar.center.iter().map(String::as_str).eq(preset.center.iter().copied())
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Lane {
    Left,
    Center,
    Right,
    /// The first sidebar tab ("Add to sidebar"; "Remove from sidebar"
    /// takes the card off every tab).
    Sidebar,
    /// One sidebar tab, by index.
    Tab(usize),
    /// A group of the left (`false`) or right side bar.
    Side(bool, SideZone),
    Tray,
}

impl Lane {
    pub(crate) fn list(self, c: &mut ShellConfig) -> Option<&mut Vec<String>> {
        match self {
            Lane::Left => Some(&mut c.bar.left),
            Lane::Center => Some(&mut c.bar.center),
            Lane::Right => Some(&mut c.bar.right),
            Lane::Side(right, zone) => Some(c.side_bars.side_mut(right).zone_mut(zone)),
            Lane::Sidebar => c.sidebar.tabs.first_mut().map(|t| &mut t.cards),
            Lane::Tab(i) => c.sidebar.tabs.get_mut(i).map(|t| &mut t.cards),
            Lane::Tray => None,
        }
    }
}

/// What's being dragged.
#[derive(Clone, Debug)]
pub struct DragChip {
    pub id: String,
    pub from: Lane,
    label: SharedString,
    icon: &'static str,
    bg: Color,
    fg: Color,
}

impl DragChip {
    pub fn new(id: String, from: Lane, label: SharedString, icon: &'static str, bg: Color, fg: Color) -> Self {
        Self { id, from, label, icon, bg, fg }
    }
}

impl Render for DragChip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(DIM_32_PX))
            .px(px(SPACE_12_PX))
            .rounded_full()
            .row()
            .items_center()
            .gap(px(SPACE_7_PX))
            .bg(self.bg.hsla())
            .text_color(self.fg.hsla())
            .text_size(px(TEXT_12_5_PX))
            .font_weight(FontWeight::MEDIUM)
            .shadow_lg()
            .child(icon(self.icon, ICON_14_PX, self.fg))
            .child(self.label.clone())
    }
}

/// Moves `id` from `from` to `to` at `index` (None = end).
pub(super) fn place(id: &str, from: Lane, to: Lane, index: Option<usize>, cx: &mut App) {
    update_config(cx, |c| place_in(c, id, from, to, index));
}

/// [`place`] on a config: takes `id` out of `from` (out of every sidebar
/// tab when the sidebar's card goes to the tray) and inserts it into `to`
/// at `index` (None = end), turning the plugin on (and bringing back a
/// removed one). Dropping on the tray only takes it out.
pub(crate) fn place_in(c: &mut ShellConfig, id: &str, from: Lane, to: Lane, index: Option<usize>) {
    if matches!(to, Lane::Left | Lane::Center | Lane::Right | Lane::Side(..)) && !bar_capable(id) {
        return;
    }
    // Index in the target before removal (same-lane reorders shift).
    let mut index = index;
    if from == to
        && let (Some(list), Some(i)) = (to.list(c), index.as_mut())
        && let Some(old) = list.iter().position(|x| x == id)
        && old < *i
    {
        *i -= 1;
    }
    if from == Lane::Sidebar && to == Lane::Tray {
        for tab in &mut c.sidebar.tabs {
            tab.cards.retain(|x| x != id);
        }
    }
    if let Some(list) = from.list(c) {
        list.retain(|x| x != id);
    }
    if let Some(list) = to.list(c) {
        list.retain(|x| x != id);
        let i = index.unwrap_or(list.len()).min(list.len());
        list.insert(i, id.to_string());
        // Placing a plugin turns it on (and brings back a removed one).
        c.plugins.disabled.retain(|x| x != id);
        c.plugins.removed.retain(|x| x != id);
        c.plugins.removed_places.remove(id);
        if !c.plugins.enabled.iter().any(|x| x == id) {
            c.plugins.enabled.push(id.to_string());
        }
    }
}

/// Built-ins without a bar item.
const NO_BAR: &[&str] = &["main-menu", "compositor", "quick-settings", "wallhaven", "cursors", "icon-themes", "dock", "app-installer"];

/// Desktop-only families and service plugins have no bar view. A missing
/// default zone alone does not imply that: optional bar items use it too.
pub(super) fn bar_capable(id: &str) -> bool {
    !NO_BAR.contains(&id) && !mandara_config::desktop_widgets::plugin_ids::ALL.contains(&id)
}

/// Minimum height of a zone (a sidebar tab's list is taller).
const LANE_MIN_H_PX: f32 = 96.0;
const LANE_MIN_H_TALL_PX: f32 = 120.0;
/// A zone item's height and artwork.
const ZONE_ITEM_H_PX: f32 = DIM_34_PX;
const ZONE_TILE_PX: f32 = DIM_20_PX;
/// The bar's three zones sit side by side while each gets this much.
const ZONE_MIN_W_PX: f32 = DIM_200_PX;
/// A preset's tiny bar drawing marks each item of a zone with a dot this
/// big, this far apart (a busy zone draws longer).
const MOCK_ITEM_PX: f32 = 4.0;
const MOCK_ITEM_GAP_PX: f32 = 2.0;

struct Preset {
    name: &'static str,
    blurb: &'static str,
    layout: BarLayout,
    position: BarPosition,
    height: f32,
    left: &'static [&'static str],
    center: &'static [&'static str],
    right: &'static [&'static str],
    /// A plain preset groups each zone on a soft capsule of its own
    /// (`bar.capsules`); the other plain presets turn them off, so none
    /// lingers from the last one.
    capsules: bool,
    /// A bar style preset: it sets the bar's look too (and Expressive's
    /// rail), and any other preset puts that look back.
    style: Option<BarStyle>,
}

/// A style preset's card: its layout, edge and height (kept equal to the
/// style's, see the tests) and the style's own items.
const fn styled(name: &'static str, blurb: &'static str, style: BarStyle, layout: BarLayout, position: BarPosition, height: f32) -> Preset {
    let [left, center, right] = style.zones();
    Preset { name, blurb, layout, position, height, left, center, right, capsules: false, style: Some(style) }
}

const PRESETS: &[Preset] = &[
    styled(tk("System monitor"), tk("A dense status line along the bottom: stat blocks, live meters and rates in monospace"), BarStyle::SystemMonitor, BarLayout::MinimalEdge, BarPosition::Bottom, 30.0),
    Preset {
        name: tk("Classic"),
        blurb: tk("Edge-to-edge frame, everything at hand"),
        layout: BarLayout::MinimalEdge,
        position: BarPosition::Top,
        height: 30.0,
        left: &["launcher", "workspaces", "active-window"],
        center: &["clock", "media"],
        right: &["weather", "resources", "clipboard", "screenshot", "network", "bluetooth", "audio", "notifications", "session"],
        capsules: false,
        style: None,
    },
    Preset {
        name: tk("Notch"),
        blurb: tk("Hangs from the top edge with curved shoulders"),
        layout: BarLayout::Notch,
        position: BarPosition::Top,
        height: 30.0,
        left: &["launcher", "workspaces", "active-window"],
        center: &["clock", "media"],
        right: &["weather", "tray", "network", "bluetooth", "audio", "notifications", "session"],
        capsules: true,
        style: None,
    },
    Preset {
        name: tk("Glass pills"),
        blurb: tk("Floating chromatic-glass islands"),
        layout: BarLayout::FloatingPills,
        position: BarPosition::Top,
        height: 32.0,
        left: &["launcher", "workspaces"],
        center: &["clock", "media"],
        right: &["network", "bluetooth", "audio", "notifications", "session"],
        capsules: false,
        style: None,
    },
    Preset {
        name: tk("Dynamic island"),
        blurb: tk("One centered island that morphs"),
        layout: BarLayout::ContiguousIsland,
        position: BarPosition::Top,
        height: 32.0,
        left: &["launcher", "workspaces"],
        center: &["clock"],
        right: &["media", "audio", "notifications", "session"],
        capsules: false,
        style: None,
    },
    Preset {
        name: tk("Minimal"),
        blurb: tk("Just the essentials, nothing else"),
        layout: BarLayout::MinimalEdge,
        position: BarPosition::Top,
        height: 26.0,
        left: &["launcher", "workspaces"],
        center: &["clock"],
        right: &["audio", "notifications", "session"],
        capsules: false,
        style: None,
    },
    Preset {
        // A work desk along the bottom edge: the focus zone (the timer and
        // the window you're in) in the middle, tasks and notes on the left,
        // the system's load with the clock on the right, each zone on a
        // capsule of its own.
        name: tk("Productive"),
        blurb: tk("A work desk along the bottom: focus timer in the middle, tasks, notes and system load in zones of their own"),
        layout: BarLayout::MinimalEdge,
        position: BarPosition::Bottom,
        height: 34.0,
        left: &["launcher", "workspaces", "todo", "notes"],
        center: &["pomodoro", "active-window"],
        right: &["resources", "clock", "audio", "notifications", "session"],
        capsules: true,
        style: None,
    },
    Preset {
        name: tk("Hairline"),
        blurb: tk("A thin strip dense with live stats"),
        layout: BarLayout::Hairline,
        position: BarPosition::Top,
        height: 30.0,
        left: &["workspaces", "mini-stats"],
        center: &["clock", "media"],
        right: &["tray", "network", "audio", "notifications", "session"],
        capsules: false,
        style: None,
    },
    Preset {
        name: tk("Micro pill"),
        blurb: tk("One tiny pill at the top"),
        layout: BarLayout::Micro,
        position: BarPosition::Top,
        height: 30.0,
        left: &[],
        center: &["workspaces", "clock", "audio", "session"],
        right: &[],
        capsules: false,
        style: None,
    },
    Preset {
        name: tk("Side bar"),
        blurb: tk("A slim bar on the left edge; panels open sideways"),
        layout: BarLayout::MinimalEdge,
        position: BarPosition::Left,
        height: 30.0,
        left: &["launcher", "workspaces"],
        center: &["clock"],
        right: &["tray", "audio", "network", "notifications", "session"],
        capsules: false,
        style: None,
    },
    Preset {
        name: tk("Dock"),
        blurb: tk("A bottom bar, panels spring upward"),
        layout: BarLayout::FloatingPills,
        position: BarPosition::Bottom,
        height: 34.0,
        left: &["launcher", "workspaces"],
        center: &["media"],
        right: &["clock", "network", "audio", "notifications", "session"],
        capsules: false,
        style: None,
    },
    styled(tk("Expressive"), tk("Round buttons, tonal tiles and a scalloped rail inside a frame round the screen"), BarStyle::Expressive, BarLayout::MinimalEdge, BarPosition::Bottom, 52.0),
    styled(tk("Tile dock"), tk("A floating island of large squircle tiles at the bottom"), BarStyle::TileDock, BarLayout::ContiguousIsland, BarPosition::Bottom, 56.0),
    styled(tk("Rail"), tk("A wide rail of flower buttons and tiles inside a screen frame"), BarStyle::Rail, BarLayout::MinimalEdge, BarPosition::Left, 30.0),
    styled(tk("Segmented"), tk("No bar, just rows of connected tiles under the top edge"), BarStyle::Segmented, BarLayout::FloatingPills, BarPosition::Top, 36.0),
    styled(tk("Stanley Bridge"), tk("Three floating docks of squircle tiles spanning the bottom like a bridge's arches"), BarStyle::StanleyBridge, BarLayout::FloatingPills, BarPosition::Bottom, 48.0),
    styled(tk("Pharos Beam"), tk("A lantern of scalloped accent buttons and tonal tiles hanging from the top edge"), BarStyle::PharosBeam, BarLayout::Notch, BarPosition::Top, 44.0),
    styled(tk("Raml Tram"), tk("One line of flower stops running along the top and down the left edge"), BarStyle::RamlTram, BarLayout::MinimalEdge, BarPosition::Top, 38.0),
];

fn apply_preset(p: &'static Preset, replace_items: bool, cx: &mut App) {
    let known: Vec<String> = cx.global::<PluginRegistry>().manifests().map(|m| m.id.clone()).collect();
    update_config(cx, |c| apply_preset_in(c, p, replace_items, &known));
}

fn apply_preset_in(c: &mut ShellConfig, p: &Preset, replace_items: bool, known: &[String]) {
    let pick = |ids: &[&str]| ids.iter().filter(|i| known.iter().any(|k| k == *i)).map(|s| s.to_string()).collect::<Vec<_>>();
    let (left, center, right) = (pick(p.left), pick(p.center), pick(p.right));
    match p.style {
        // A style sets the layout, edge and height with its look.
        Some(style) => style.apply(c),
        None => {
            // The last style's look goes; this preset's bar comes.
            mandara_config::bar_styles::clear(c);
            c.bar.layout = p.layout;
            c.bar.position = p.position;
            c.bar.height = p.height;
            c.bar.capsules = p.capsules;
            // A capsule on every item hides the zone capsules, so a preset
            // built on zones turns them off.
            if p.capsules {
                c.bar.item_capsules = false;
            }
        }
    }
    if replace_items {
        c.bar.left.clear();
        c.bar.center.clear();
        c.bar.right.clear();
        for (ids, to) in [(left, Lane::Left), (center, Lane::Center), (right, Lane::Right)] {
            for id in ids { place_in(c, &id, Lane::Tray, to, None); }
        }
        // A style's rail (Expressive, Raml Tram): the left side bar's groups.
        if let Some(rail) = p.style.and_then(BarStyle::rail) {
            for (zone, ids) in SideZone::ALL.into_iter().zip(rail) {
                c.side_bars.left.zone_mut(zone).clear();
                for id in pick(ids) { place_in(c, &id, Lane::Tray, Lane::Side(false, zone), None); }
            }
        }
    }
    // The notch look: a soft shadow under its grouped capsules (a style
    // brings its own look).
    if p.layout == BarLayout::Notch && p.style.is_none() {
        c.bar.shadow = true;
    }
}

/// One side bar's own style: a chip per bar style (its item look, for this
/// bar alone) and *Shared* (the side bars' shared look). Another style or
/// Shared clears what the last one set; a bar shaped by hand shows none.
fn side_style_row(cfg: &ShellConfig, right: bool, p: &MandaraPalette) -> AnyElement {
    let current = cfg.side_bars.side(right).style;
    let side = if right { "right" } else { "left" };
    let chip = |style: Option<BarStyle>, label: &'static str| {
        super::compositor_page::chip(SharedString::from(format!("side-style-{side}-{}", style.map_or("shared", BarStyle::id))), tr(label), current == style, p).on_click(move |_, _, cx| {
            update_config(cx, |c| match style {
                Some(style) => style.apply_to_side(c, right),
                None => mandara_config::bar_styles::clear_side(c, right),
            });
        })
    };
    div()
        .row()
        .flex_wrap()
        .items_center()
        .gap(px(SPACE_6_PX))
        .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("Style")))
        .child(chip(None, tk("Shared")))
        .children(BarStyle::SIDE.into_iter().zip(SIDE_STYLE_NAMES).map(|(style, name)| chip(Some(style), name)))
        .into_any_element()
}

/// The style chips' names, in [`BarStyle::SIDE`] order.
const SIDE_STYLE_NAMES: [&str; 4] = [tk("Expressive"), tk("Tile dock"), tk("Rail"), tk("Segmented")];

impl StudioView {
    /// One item of a zone: grip, artwork, name, "off" while the plugin is
    /// off, and × to take it off. Drag to reorder or move it (dropping on
    /// an item inserts before it); click for the plugin menu.
    fn zone_item(&self, id: &str, lane: Lane, index: usize, len: usize, scale: f32, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let registry = cx.global::<PluginRegistry>();
        let enabled = {
            let cfg = mandara_ui::state::config(cx).read(cx).snapshot();
            registry.is_enabled(id, &cfg.plugins)
        };
        let manifest = registry.manifest(id);
        let name = manifest.map_or_else(|| id.to_string(), |m| m.display_name());
        let ic = plugin_icon(id);
        let drag = DragChip { id: id.to_string(), from: lane, label: name.clone().into(), icon: ic, bg: p.primary, fg: p.on_primary };
        let (id_rm, id_drop, id_menu, name_menu) = (id.to_string(), id.to_string(), id.to_string(), name.clone());
        let tile = match manifest {
            Some(m) => plugin_tile(m, ZONE_TILE_PX, enabled, scale, p),
            None => div().size(px(ZONE_TILE_PX)).flex_none().row().items_center().justify_center().child(icon(ic, ICON_13_PX, p.on_surface_variant)).into_any_element(),
        };
        div()
            .id(SharedString::from(format!("zone-{lane:?}-{id}")))
            .group("zone-item")
            .h(px(ZONE_ITEM_H_PX))
            .flex_none()
            .w_full()
            .pad_start(px(SPACE_6_PX))
            .pad_end(px(SPACE_4_PX))
            .rounded(px(RADIUS_10_PX))
            .row()
            .items_center()
            .gap(px(SPACE_8_PX))
            .cursor_grab()
            .bg(p.surface_base.hsla())
            .border_1()
            .border_color(p.on_surface.with_alpha(ALPHA_8).hsla())
            .shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
            .hover(|s| s.border_color(p.primary.with_alpha(ALPHA_60).hsla()))
            .child(div().flex_none().opacity(ALPHA_50).group_hover("zone-item", |s| s.opacity(1.0)).child(icon("grid", ICON_11_PX, p.on_surface_variant)))
            .child(tile)
            .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(TEXT_12_5_PX)).font_weight(FontWeight::MEDIUM).when(!enabled, |el| el.text_color(p.on_surface_variant.hsla())).child(name))
            .when(!enabled, |el| el.child(div().flex_none().px(px(SPACE_5_PX)).rounded(px(RADIUS_5_PX)).bg(p.on_surface.with_alpha(ALPHA_8).hsla()).text_size(px(TEXT_10_PX)).text_color(p.on_surface_variant.hsla()).child(t("Off"))))
            .child(
                div()
                    .id(SharedString::from(format!("zone-rm-{lane:?}-{id}")))
                    .size(px(DIM_22_PX))
                    .flex_none()
                    .rounded_full()
                    .row()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .opacity(ALPHA_45)
                    .group_hover("zone-item", |s| s.opacity(1.0))
                    .hover(|s| s.bg(p.urgent_coral.with_alpha(ALPHA_16).hsla()))
                    .child(icon("close", ICON_10_PX, p.on_surface_variant))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        place(&id_rm, lane, Lane::Tray, None, cx);
                    }),
            )
            // The plugin menu belongs to the bar's designer (a sidebar tab's
            // items keep drag and ×).
            .when(matches!(lane, Lane::Left | Lane::Center | Lane::Right | Lane::Side(..)), |el| {
                el.on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                    let at = e.position();
                    this.open_plugin_menu(Menu { id: id_menu.clone(), name: name_menu.clone(), from: lane, kind: MenuFor::Zone { index, len }, at: (f32::from(at.x), f32::from(at.y)) }, cx);
                }))
            })
            .on_drag(drag, |chip, _, _, cx| cx.new(|_| chip.clone()))
            // Dropping on an item inserts before it.
            .drag_over::<DragChip>(move |s, _, _, _| s.border_color(gpui::rgb(0x6aa0ff)).border_2())
            .on_drop(move |chip: &DragChip, _, cx| {
                if chip.id != id_drop {
                    place(&chip.id, chip.from, lane, Some(index), cx);
                }
            })
            .into_any_element()
    }

    /// A zone (a bar zone, a side bar group, a sidebar tab): its title and
    /// count, then its items stacked top to bottom in order. Dropping on
    /// the zone's free space appends.
    pub(super) fn lane(&self, lane: Lane, title: &'static str, ids: &[String], tall: bool, scale: f32, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let len = ids.len();
        let items: Vec<AnyElement> = ids.iter().enumerate().map(|(i, id)| self.zone_item(id, lane, i, len, scale, p, cx)).collect();
        div()
            .id(SharedString::from(format!("lane-{lane:?}")))
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(if tall { LANE_MIN_H_TALL_PX } else { LANE_MIN_H_PX }))
            .p(px(SPACE_8_PX))
            .pb(px(SPACE_12_PX))
            .rounded(px(RADIUS_16_PX))
            .border_1()
            .border_color(p.on_surface.with_alpha(ALPHA_6).hsla())
            .bg(p.on_surface.with_alpha(ALPHA_3).hsla())
            .row()
            .flex_col()
            .gap(px(SPACE_4_PX))
            .drag_over::<DragChip>(|s, _, _, _| s.bg(gpui::rgba(0x6aa0ff22)).border_color(gpui::rgba(0x6aa0ffaa)))
            .on_drop(move |chip: &DragChip, _, cx| place(&chip.id, chip.from, lane, None, cx))
            .child(
                div()
                    .px(px(SPACE_4_PX))
                    .pb(px(SPACE_4_PX))
                    .row()
                    .items_center()
                    .gap(px(SPACE_6_PX))
                    .text_size(px(TEXT_11_PX))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.on_surface_variant.with_alpha(ALPHA_80).hsla())
                    .child(div().min_w(px(0.0)).truncate().child(tr(title)))
                    .child(div().flex_none().text_color(p.on_surface_variant.with_alpha(ALPHA_55).hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(len.to_string())),
            )
            .children(items)
            .when(len == 0, |el| {
                el.child(
                    div()
                        .flex_1()
                        .min_h(px(ZONE_ITEM_H_PX))
                        .rounded(px(RADIUS_10_PX))
                        .border_1()
                        .border_color(p.on_surface.with_alpha(ALPHA_10).hsla())
                        .row()
                        .items_center()
                        .justify_center()
                        .text_size(px(TEXT_11_5_PX))
                        .text_color(p.on_surface_variant.with_alpha(ALPHA_60).hsla())
                        .child(t("Drop plugins here")),
                )
            })
            .into_any_element()
    }

    /// The side bars' layout designer (Studio → Bars → Side bars).
    pub(super) fn side_bars_designer(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if !cfg.side_bars.enabled { return div().into_any_element(); }
        let scale = window.scale_factor();
        let mut side = |right: bool, title: &'static str| {
            let settings = cfg.side_bars.side(right);
            div().flex_1().min_w(px(0.0)).row().flex_col().gap(px(SPACE_6_PX))
                .when(!settings.enabled, |el| el.opacity(ALPHA_60))
                .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(tr(title)))
                .child(side_style_row(cfg, right, p))
                .child(self.lane(Lane::Side(right, SideZone::Top), tk("Top"), &settings.top, false, scale, p, cx))
                .child(self.lane(Lane::Side(right, SideZone::Middle), tk("Middle"), &settings.middle, false, scale, p, cx))
                .child(self.lane(Lane::Side(right, SideZone::Bottom), tk("Bottom"), &settings.bottom, false, scale, p, cx))
        };
        let (left, right) = (side(false, tk("Left side bar")), side(true, tk("Right side bar")));
        let lanes = div().ltr_row().when(self.stack_rows, |el| el.flex_col()).gap(px(SPACE_12_PX)).child(left).child(right);
        let pool = self.plugin_pool(cfg, p, window, cx);
        let menu = self.plugin_menu(cfg, p, window, cx);
        div().row().flex_col().gap(px(SPACE_12_PX)).child(lanes).child(pool).children(menu).into_any_element()
    }

    pub(super) fn bar_designer(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let scale = window.scale_factor();
        let vertical = cfg.bar.position.is_vertical();
        let replace_items = self.designer.replace_items;
        let selected = self.designer.preset_selection.active(cfg);

        // Presets.
        let mut presets: Vec<AnyElement> = Vec::with_capacity(PRESETS.len());
        for (i, preset) in PRESETS.iter().enumerate() {
            let active = selected == Some(i);
            // A tiny drawing of the preset's bar (physical sides: never mirrored).
            // A zone: a dot per item, on a glass pill of its own when the
            // groups float, or on its soft capsule inside the bar
            // (`bar.capsules`: Notch, Productive).
            let pill = |items: &[&str], floating: bool| {
                div()
                    .flex_none()
                    .ltr_row()
                    .items_center()
                    .gap(px(MOCK_ITEM_GAP_PX))
                    .when(floating, |el| el.h(px(DIM_10_PX)).px(px(MOCK_ITEM_GAP_PX * 1.5)).rounded_full().bg(p.surface_container_high.hsla()))
                    .when(!floating && preset.capsules, |el| el.h(px(DIM_8_PX)).px(px(MOCK_ITEM_GAP_PX * 1.5)).rounded_full().bg(p.on_surface.with_alpha(ALPHA_12).hsla()))
                    .children(items.iter().map(|_| div().flex_none().size(px(MOCK_ITEM_PX)).rounded_full().bg(p.primary.with_alpha(ALPHA_75).hsla())))
            };
            let mock = div()
                .h(px(DIM_44_PX))
                .rounded(px(RADIUS_10_PX))
                .bg(p.surface_base.hsla())
                .p(px(SPACE_6_PX))
                .row()
                .flex_col()
                .when(preset.position == BarPosition::Bottom, |el| el.justify_end())
                .when(preset.position.is_vertical(), |el| el.ltr_row())
                .child(if let Some(style) = preset.style { previews::styled(style, p) } else { match preset.layout {
                    // A side bar: a slim strip down the left edge.
                    _ if preset.position.is_vertical() => div().w(px(DIM_10_PX)).h_full().rounded(px(RADIUS_4_PX)).bg(p.surface_container_high.hsla()).py(px(SPACE_3_PX)).row().flex_col().items_center().justify_between().child(div().size(px(DIM_6_PX)).rounded_full().bg(p.primary.with_alpha(ALPHA_75).hsla())).child(div().size(px(DIM_6_PX)).rounded_full().bg(p.primary.with_alpha(ALPHA_75).hsla())),
                    BarLayout::Hairline => div().h(px(DIM_6_PX)).rounded(px(RADIUS_1_5_PX)).bg(p.surface_container_high.hsla()).border_b_1().border_color(p.primary.hsla()),
                    BarLayout::Micro | BarLayout::ContiguousIsland => div().row().justify_center().child(previews::centered(preset, p)),
                    BarLayout::MinimalEdge => div().h(px(DIM_12_PX)).rounded(px(RADIUS_4_PX)).bg(p.surface_container_high.hsla()).px(px(SPACE_4_PX)).ltr_row().items_center().justify_between().child(pill(preset.left, false)).child(pill(preset.center, false)).child(pill(preset.right, false)),
                    BarLayout::Notch => div().h(px(DIM_12_PX)).mt(px(-SPACE_6_PX)).mx(px(SPACE_10_PX)).rounded_b(px(RADIUS_6_PX)).bg(p.surface_container_high.hsla()).px(px(SPACE_4_PX)).ltr_row().items_center().justify_between().child(pill(preset.left, false)).child(pill(preset.center, false)).child(pill(preset.right, false)),
                    BarLayout::FloatingPills => div().h(px(DIM_12_PX)).ltr_row().items_center().justify_between().child(pill(preset.left, true)).child(pill(preset.center, true)).child(pill(preset.right, true)),
                } });
            presets.push(
                div()
                    .id(("preset", i))
                    .flex_1()
                    .rounded(px(RADIUS_16_PX))
                    .p(px(SPACE_10_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_8_PX))
                    .cursor_pointer()
                    .bg(if active { p.primary.with_alpha(ALPHA_12).hsla() } else { p.surface_container.hsla() })
                    .border_2()
                    .border_color(if active { p.primary.hsla() } else { gpui::transparent_black() })
                    .hover(|s| s.bg(p.primary.with_alpha(ALPHA_8).hsla()))
                    .child(mock)
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(TEXT_13_PX)).child(mandara_ui::bidi::paragraph(tr(preset.name))))
                    .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tr(preset.blurb))))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        apply_preset(preset, replace_items, cx);
                        let config = mandara_ui::state::config(cx).read(cx).snapshot();
                        this.designer.preset_selection.remember(i, &config);
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        let presets = super::grid::fit(self.page.column_w, BAR_PRESET_GRID).balanced(presets.len()).render(presets);

        // The bar's zones as columns, items stacked in order. Left / Center
        // / Right are screen sides, not reading order (Top / Middle /
        // Bottom on a bar standing on a side edge); they stack on narrow
        // pages.
        let side_by_side = self.page.column_w - 2.0 * SPACE_10_PX - 2.0 * SPACE_8_PX >= 3.0 * ZONE_MIN_W_PX;
        let bar = div()
            .rounded(px(RADIUS_20_PX))
            .p(px(SPACE_10_PX))
            .bg(p.surface_container.hsla())
            .ltr_row()
            .when(!side_by_side, |el| el.flex_col())
            .gap(px(SPACE_8_PX))
            .child(self.lane(Lane::Left, zone_title(Lane::Left, vertical), &cfg.bar.left, false, scale, p, cx))
            .child(self.lane(Lane::Center, zone_title(Lane::Center, vertical), &cfg.bar.center, false, scale, p, cx))
            .child(self.lane(Lane::Right, zone_title(Lane::Right, vertical), &cfg.bar.right, false, scale, p, cx));
        let bar = bar.into_any_element();
        let pool = self.plugin_pool(cfg, p, window, cx);
        let menu = self.plugin_menu(cfg, p, window, cx);
        let header = |title: &'static str, sub: &'static str| {
            div()
                .px(px(SPACE_6_PX))
                .row()
                .flex_col()
                .gap(px(SPACE_3_PX))
                .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t(title)))
                .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t(sub))))
        };
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_12_PX))
            .child(header(tk("Presets"), tk("Changes the bar's style, position and height. Your items stay unless you choose to replace them.")))
            .child(super::compositor_page::chip("bar-preset-replace-items", t("Replace bar items"), replace_items, p).on_click(cx.listener(|this, _, _, cx| {
                this.designer.replace_items = !this.designer.replace_items;
                cx.notify();
            })))
            .child(presets)
            .child(header(tk("Layout designer"), tk("Drag items to reorder them or move them between zones; click one for more")))
            .child(bar)
            .child(pool)
            .children(menu)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_only_preset_selection_survives_custom_items_but_not_manual_edits() {
        let index = PRESETS.iter().position(|p| p.name == "System monitor").unwrap();
        let mut config = ShellConfig::default();
        config.bar.center = vec!["media".into()];
        apply_preset_in(&mut config, &PRESETS[index], false, &[]);
        let mut selection = PresetSelection::default();
        selection.remember(index, &config);
        assert_eq!(selection.active(&config), Some(index));
        config.bar.height += 1.0;
        assert_eq!(selection.active(&config), Some(index));
        for lane in [Lane::Left, Lane::Center, Lane::Right] {
            let mut edited = config.clone();
            lane.list(&mut edited).unwrap().push("weather".into());
            selection.remember(index, &config);
            selection.active(&edited);
            assert!(selection.chosen.is_none());
        }
        let other_edge = if config.bar.position == BarPosition::Bottom { BarPosition::Top } else { BarPosition::Bottom };
        for (layout, position) in [(BarLayout::Hairline, config.bar.position), (config.bar.layout, other_edge)] {
            let mut edited = config.clone();
            edited.bar.layout = layout;
            edited.bar.position = position;
            selection.remember(index, &config);
            selection.active(&edited);
            assert!(selection.chosen.is_none());
        }
    }

    #[test]
    fn an_existing_exact_preset_remains_recognizable_without_a_click() {
        let mut config = ShellConfig::default();
        let index = PRESETS.iter().position(|p| p.style.is_none()).unwrap();
        let preset = &PRESETS[index];
        config.bar.layout = preset.layout;
        config.bar.position = preset.position;
        config.bar.center = preset.center.iter().map(|id| (*id).into()).collect();
        let mut selection = PresetSelection::default();
        assert_eq!(selection.active(&config), Some(index));
    }

    #[test]
    fn presets_reference_real_plugins_and_layouts() {
        for p in PRESETS {
            assert!(!p.center.is_empty() || !p.left.is_empty());
            let heights = mandara_config::surfaces::ranges::BAR_HEIGHT;
            assert!(p.height >= heights.min && p.height <= heights.max);
        }
    }

    fn bar() -> ShellConfig {
        let mut c = ShellConfig::default();
        c.bar.left = vec!["launcher".into(), "workspaces".into(), "active-window".into()];
        c.bar.center = vec!["clock".into()];
        c.bar.right = vec!["audio".into(), "session".into()];
        c
    }

    #[test]
    fn reordering_in_a_zone_lands_before_the_drop_target() {
        let mut c = bar();
        // Dragged down onto the last item: it goes before it.
        place_in(&mut c, "launcher", Lane::Left, Lane::Left, Some(2));
        assert_eq!(c.bar.left, ["workspaces", "launcher", "active-window"]);
        // Dragged up onto the first item.
        place_in(&mut c, "active-window", Lane::Left, Lane::Left, Some(0));
        assert_eq!(c.bar.left, ["active-window", "workspaces", "launcher"]);
        // Dropped on the zone's free space: the end.
        place_in(&mut c, "active-window", Lane::Left, Lane::Left, None);
        assert_eq!(c.bar.left, ["workspaces", "launcher", "active-window"]);
    }

    #[test]
    fn moving_between_zones_and_off_the_bar() {
        let mut c = bar();
        place_in(&mut c, "clock", Lane::Center, Lane::Right, Some(1));
        assert!(c.bar.center.is_empty());
        assert_eq!(c.bar.right, ["audio", "clock", "session"]);
        place_in(&mut c, "clock", Lane::Right, Lane::Tray, None);
        assert_eq!(c.bar.right, ["audio", "session"]);
    }

    #[test]
    fn placing_a_plugin_turns_it_on() {
        let mut c = bar();
        c.plugins.disabled.push("weather".into());
        c.plugins.removed.push("weather".into());
        place_in(&mut c, "weather", Lane::Tray, Lane::Center, None);
        assert_eq!(c.bar.center, ["clock", "weather"]);
        assert!(!c.plugins.disabled.iter().any(|x| x == "weather"));
        assert!(!c.plugins.is_removed("weather"));
        assert!(c.plugins.enabled.iter().any(|x| x == "weather"));
    }

    #[test]
    fn a_style_preset_keeps_existing_items_and_desktop_widgets() {
        let mut c = bar();
        c.desktop.widgets.items.push(mandara_config::desktop_widgets::DesktopWidget::default());
        let before = c.clone();
        let notch = PRESETS.iter().find(|p| p.layout == BarLayout::Notch).unwrap();
        let known = ["clock".to_string(), "launcher".to_string()];
        apply_preset_in(&mut c, notch, false, &known);
        assert_eq!(c.bar.layout, BarLayout::Notch);
        assert_eq!((&c.bar.left, &c.bar.center, &c.bar.right), (&before.bar.left, &before.bar.center, &before.bar.right));
        assert_eq!(c.desktop, before.desktop);
        assert_eq!(c.plugins, before.plugins);
        apply_preset_in(&mut c, notch, true, &known);
        assert_ne!(c.bar.left, before.bar.left);
        assert_eq!(c.desktop, before.desktop);
        assert!(c.plugins.enabled.iter().any(|id| id == "launcher"));
    }

    #[test]
    fn desktop_only_widgets_cannot_be_added_to_a_bar() {
        for id in mandara_config::desktop_widgets::plugin_ids::ALL.into_iter().chain(NO_BAR.iter().copied()) {
            let mut c = bar();
            let before = c.clone();
            place_in(&mut c, id, Lane::Tray, Lane::Center, None);
            assert_eq!(c, before, "{id}");
            place_in(&mut c, id, Lane::Tray, Lane::Side(false, SideZone::Middle), None);
            assert_eq!(c, before, "{id}");
        }
        assert!(bar_capable("power"));
        assert!(bar_capable("processes"));
        assert!(bar_capable("third-party-example"));
    }

    #[test]
    fn style_presets_show_their_style_and_put_it_back_when_you_move_on() {
        let styles: Vec<&Preset> = PRESETS.iter().filter(|p| p.style.is_some()).collect();
        assert_eq!(styles.len(), BarStyle::ALL.len(), "every style has a card");
        for p in &styles {
            let style = p.style.unwrap();
            // The card draws what the style sets.
            assert_eq!((p.layout, p.position, p.height), (style.layout(), style.position(), style.height()), "{}", p.name);
        }
        let known: Vec<String> = ["launcher", "clock", "workspaces", "audio", "network", "bluetooth", "weather", "media", "screenshot", "clipboard", "color-picker", "tray", "notifications", "session", "drives", "pomodoro", "wallpaper", "keybinds"].map(String::from).to_vec();
        let expressive = styles.iter().find(|p| p.style == Some(BarStyle::Expressive)).unwrap();
        let mut c = bar();
        apply_preset_in(&mut c, expressive, true, &known);
        assert!(BarStyle::Expressive.matches(&c) && c.side_bars.enabled && c.bar.screen_frame);
        assert_eq!(c.side_bars.left.middle, ["clock", "workspaces"], "its rail is filled");
        assert!(c.bar.center.iter().any(|id| id == "weather"));
        let mut selection = PresetSelection::default();
        let index = PRESETS.iter().position(|p| std::ptr::eq(p, *expressive)).unwrap();
        assert_eq!(selection.active(&c), Some(index), "recognised without a click");
        // Any other preset takes the style's look away, keeping your items.
        let classic = PRESETS.iter().find(|p| p.name == "Classic").unwrap();
        let items = c.bar.center.clone();
        apply_preset_in(&mut c, classic, false, &known);
        assert_eq!(c.bar.style, None);
        assert!(!c.side_bars.enabled && !c.bar.screen_frame && !c.bar.item_capsules);
        assert_eq!(c.bar.center, items);
        assert_eq!(selection.active(&c), None, "Classic's own items differ");
        // Every style preset: picked over a bar shaped by hand, then left
        // for Classic, gives back all it set and keeps the hand-made rest.
        // Every plugin a preset or its rail places.
        let mut every: Vec<String> = Vec::new();
        for p in PRESETS {
            let rail = p.style.and_then(BarStyle::rail).unwrap_or_default();
            for id in [p.left, p.center, p.right].into_iter().chain(rail).flatten() {
                if !every.iter().any(|k| k == id) {
                    every.push((*id).to_string());
                }
            }
        }
        let known = every;
        for p in &styles {
            let style = p.style.unwrap();
            let mut hand = bar();
            hand.bar.text_scale = 1.15;
            hand.bar.show_seconds = true;
            apply_preset_in(&mut hand, p, true, &known);
            assert!(style.matches(&hand), "{}", p.name);
            assert_eq!((hand.bar.layout, hand.bar.position, hand.bar.height), (p.layout, p.position, p.height), "{}", p.name);
            let index = PRESETS.iter().position(|q| std::ptr::eq(q, *p)).unwrap();
            assert_eq!(PresetSelection::default().active(&hand), Some(index), "{} is recognised", p.name);
            apply_preset_in(&mut hand, classic, false, &known);
            let d = ShellConfig::default();
            assert_eq!(hand.bar.style, None, "{}", p.name);
            assert_eq!((hand.bar.item_shape, hand.bar.icon_shape, hand.bar.icon_fill, hand.bar.item_capsules), (d.bar.item_shape, d.bar.icon_shape, d.bar.icon_fill, d.bar.item_capsules), "{}", p.name);
            assert_eq!((hand.bar.notch_width, hand.bar.screen_frame, hand.side_bars.enabled), (d.bar.notch_width, d.bar.screen_frame, d.side_bars.enabled), "{}", p.name);
            assert_eq!(hand.typography.bar_font, d.typography.bar_font, "{}", p.name);
            assert_eq!((hand.bar.text_scale, hand.bar.show_seconds), (1.15, true), "{}: your own settings stay", p.name);
            assert_eq!((hand.bar.layout, hand.bar.position, hand.bar.height), (classic.layout, classic.position, classic.height));
        }
    }

    #[test]
    fn every_preset_shows_each_item_once_and_switched_on() {
        let mut registry = mandara_plugins::PluginRegistry::default();
        mandara_ui::plugins::register_builtin(&mut registry);
        let known: Vec<String> = registry.manifests().map(|m| m.id.clone()).collect();
        for p in PRESETS {
            // Replacing your items, and keeping them (Studio's default and
            // `bar style`), over the shipped bar.
            for replace in [true, false] {
                let mut c = ShellConfig::default();
                apply_preset_in(&mut c, p, replace, &known);
                let rail = c.side_bar_on(false).then(|| c.side_bars.left.zones()).into_iter().flatten();
                let shown: Vec<&String> = [&c.bar.left, &c.bar.center, &c.bar.right].into_iter().chain(rail).flatten().collect();
                for (i, id) in shown.iter().enumerate() {
                    assert!(!shown[..i].contains(id), "{} (replace {replace}) shows {id} twice", p.name);
                    assert!(registry.is_enabled(id, &c.plugins), "{} (replace {replace}): {id} is placed but its plugin is off", p.name);
                }
                // A style's own items all come: System monitor's meters too.
                if let Some(style) = p.style {
                    for id in style.signature().into_iter().flatten() {
                        assert!(shown.iter().any(|s| s == id), "{} (replace {replace}): {id}", p.name);
                    }
                }
            }
        }
        // The rail presets: the workspaces once, on the rail, and the
        // launcher first on the bar, in the corner.
        for style in [BarStyle::Expressive, BarStyle::RamlTram] {
            let p = PRESETS.iter().find(|p| p.style == Some(style)).unwrap();
            for replace in [true, false] {
                let mut c = ShellConfig::default();
                apply_preset_in(&mut c, p, replace, &known);
                assert!(c.side_bars.left.zones().iter().any(|z| z.iter().any(|id| id == "workspaces")), "{} (replace {replace})", p.name);
                assert!(![&c.bar.left, &c.bar.center, &c.bar.right].iter().any(|z| z.iter().any(|id| id == "workspaces")), "{} (replace {replace})", p.name);
                assert_eq!(c.bar.left.first().map(String::as_str), Some("launcher"), "{} (replace {replace})", p.name);
            }
        }
    }

    /// What a preset makes the bar look like: its layout, edge and height,
    /// its style, the items' shapes and colour, the bar font and the rail.
    fn visible(c: &ShellConfig) -> (BarLayout, BarPosition, u32, Option<BarStyle>, String) {
        let b = &c.bar;
        let shapes = format!("{:?} {:?} {:?} {} {} {} {} {}", b.item_shape, b.icon_shape, b.icon_fill, b.item_capsules, b.capsules, c.typography.bar_font, c.side_bars_join(), c.bar_layout() == BarLayout::Notch);
        (b.layout, b.position, b.height.to_bits(), b.style, shapes)
    }

    #[test]
    fn no_two_presets_look_alike() {
        let mut seen: Vec<(&str, (BarLayout, BarPosition, u32, Option<BarStyle>, String))> = Vec::new();
        for p in PRESETS {
            let mut c = ShellConfig::default();
            apply_preset_in(&mut c, p, true, &[]);
            let look = visible(&c);
            if let Some((other, _)) = seen.iter().find(|(_, l)| *l == look) {
                panic!("{} looks just like {other}: {look:?}", p.name);
            }
            seen.push((p.name, look));
        }
        // The look-alike this guards against: System monitor is not Classic
        // with other items.
        let named = |name: &str| seen.iter().find(|(n, _)| *n == name).map(|(_, l)| l.clone()).unwrap();
        let (monitor, classic) = (named("System monitor"), named("Classic"));
        assert_ne!((monitor.0, monitor.1), (classic.0, classic.1), "a different edge or layout");
        assert_ne!(monitor.4, classic.4, "a different shape language");
        let names: Vec<&str> = PRESETS.iter().map(|p| p.name).collect();
        for name in ["Stanley Bridge", "Pharos Beam", "Raml Tram"] {
            assert!(names.contains(&name), "{name} has a card");
        }
    }

    #[test]
    fn every_preset_card_has_its_own_drawing() {
        // Plain presets draw their layout and edge; style presets draw
        // their style. No two cards share a drawing.
        let mut drawings: Vec<(BarLayout, BarPosition, Option<BarStyle>, bool, [usize; 3])> = Vec::new();
        for p in PRESETS {
            let key = match p.style {
                Some(style) => (BarLayout::MinimalEdge, BarPosition::Top, Some(style), false, [0; 3]),
                // Plain presets with the same layout and edge draw a dot
                // per item (Classic and Minimal), on zone capsules when
                // they have them.
                None => (p.layout, p.position, None, p.capsules, [p.left.len(), p.center.len(), p.right.len()]),
            };
            assert!(!drawings.contains(&key), "{}: its card looks like another", p.name);
            drawings.push(key);
        }
    }

    #[test]
    fn no_two_presets_are_the_same_bar_with_other_items() {
        // Glass pills and Productive were once the same floating pills at
        // the top with a few items swapped. Two presets may share a bar
        // (layout, style, edge and zone capsules) only when one is the
        // other with fewer items (Minimal is Classic trimmed down).
        let bar = |p: &Preset| (p.layout, p.style, p.position, p.capsules);
        let trimmed = |small: &Preset, big: &Preset| [(small.left, big.left), (small.center, big.center), (small.right, big.right)].iter().all(|(s, b)| s.iter().all(|id| b.contains(id)));
        for (i, a) in PRESETS.iter().enumerate() {
            for b in &PRESETS[i + 1..] {
                assert_ne!((bar(a), [a.left, a.center, a.right]), (bar(b), [b.left, b.center, b.right]), "{} and {} are the same preset", a.name, b.name);
                if bar(a) == bar(b) {
                    assert!(trimmed(a, b) || trimmed(b, a), "{} and {} are the same bar with other items", a.name, b.name);
                }
                // Studio recognises the chosen preset by its bar and its
                // middle zone, so no two may share both.
                assert!(bar(a) != bar(b) || a.center != b.center, "{} and {} light up together", a.name, b.name);
            }
        }
        // Productive is a bar of its own: not Glass pills, not Classic,
        // not the System monitor strip along the same edge.
        let named = |name: &str| PRESETS.iter().find(|p| p.name == name).unwrap();
        let productive = named("Productive");
        for other in PRESETS.iter().filter(|p| p.name != "Productive") {
            assert_ne!(bar(productive), bar(other), "Productive's bar is {}'s", other.name);
        }
        assert_ne!((productive.layout, productive.position), (named("Glass pills").layout, named("Glass pills").position));
        assert!(productive.center.contains(&"pomodoro"), "the focus timer sits in the middle");
    }

    #[test]
    fn zone_capsules_come_and_go_with_their_presets() {
        let known: Vec<String> = PRESETS.iter().flat_map(|p| [p.left, p.center, p.right]).flatten().map(|id| (*id).to_string()).collect();
        let named = |name: &str| PRESETS.iter().find(|p| p.name == name).unwrap();
        let mut c = bar();
        apply_preset_in(&mut c, named("Productive"), true, &known);
        assert!(c.bar.capsules);
        assert_eq!((c.bar.layout, c.bar.position, c.bar.height), (BarLayout::MinimalEdge, BarPosition::Bottom, 34.0));
        assert_eq!(c.bar.center, ["pomodoro", "active-window"]);
        let index = PRESETS.iter().position(|p| p.name == "Productive").unwrap();
        assert_eq!(PresetSelection::default().active(&c), Some(index), "recognised without a click");
        // Leaving for a preset without them takes them away.
        for name in ["Glass pills", "Classic"] {
            let mut moved = c.clone();
            apply_preset_in(&mut moved, named(name), false, &known);
            assert!(!moved.bar.capsules, "{name}");
        }
        // The Notch keeps its own, with its shadow.
        apply_preset_in(&mut c, named("Notch"), false, &known);
        assert!(c.bar.capsules && c.bar.shadow);
    }

    #[test]
    fn zone_capsule_presets_turn_item_capsules_off() {
        let known: Vec<String> = PRESETS.iter().flat_map(|p| [p.left, p.center, p.right]).flatten().map(|id| (*id).to_string()).collect();
        let zoned: Vec<&Preset> = PRESETS.iter().filter(|p| p.capsules).collect();
        assert!(zoned.len() >= 2, "Productive and the Notch use zone capsules");
        for p in zoned {
            let mut c = bar();
            c.bar.style = None;
            c.bar.item_capsules = true;
            apply_preset_in(&mut c, p, false, &known);
            assert!(c.bar.capsules && !c.bar.item_capsules, "{}: its zone capsules show", p.name);
        }
        // A preset without zone capsules leaves the item capsules alone.
        for p in PRESETS.iter().filter(|p| !p.capsules && p.style.is_none()) {
            let mut c = bar();
            c.bar.item_capsules = true;
            apply_preset_in(&mut c, p, false, &known);
            assert!(c.bar.item_capsules, "{}", p.name);
        }
    }
}
