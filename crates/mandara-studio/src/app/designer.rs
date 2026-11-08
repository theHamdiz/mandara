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
    items: [Vec<String>; 3],
}

impl PresetSelection {
    fn remember(&mut self, index: usize, config: &ShellConfig) {
        let bar = &config.bar;
        self.chosen = Some(ChosenPreset { index, layout: bar.layout, position: bar.position, items: [bar.left.clone(), bar.center.clone(), bar.right.clone()] });
    }

    fn active(&mut self, config: &ShellConfig) -> Option<usize> {
        let bar = &config.bar;
        if let Some(chosen) = &self.chosen {
            if chosen.layout == bar.layout && chosen.position == bar.position && chosen.items.iter().zip([&bar.left, &bar.center, &bar.right]).all(|(saved, current)| saved == current) {
                return Some(chosen.index);
            }
            self.chosen = None;
        }
        PRESETS.iter().position(|preset| bar.layout == preset.layout && bar.position == preset.position && bar.center.iter().map(String::as_str).eq(preset.center.iter().copied()))
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

/// Sidebar-capable built-ins (third-party plugins all can).
pub(super) const SIDEBAR_CAPABLE: &[&str] = mandara_ui::sidebar_tabs::SIDEBAR_WIDGETS;
/// Built-ins without a bar item.
const NO_BAR: &[&str] = &["main-menu", "compositor", "quick-settings", "wallhaven", "cursors", "dock", "app-installer"];

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
/// Widths of the widget pills in a preset's tiny bar drawing.
const MOCK_PILL_A_PX: f32 = 18.0;
const MOCK_PILL_B_PX: f32 = 14.0;
const MOCK_PILL_C_PX: f32 = 22.0;

struct Preset {
    name: &'static str,
    blurb: &'static str,
    layout: BarLayout,
    position: BarPosition,
    height: f32,
    left: &'static [&'static str],
    center: &'static [&'static str],
    right: &'static [&'static str],
}

const PRESETS: &[Preset] = &[
    Preset {
        name: tk("System monitor"),
        blurb: tk("CPU, memory, GPUs, temperatures, disks and uptime"),
        layout: BarLayout::MinimalEdge,
        position: BarPosition::Top,
        height: 30.0,
        left: &["launcher", "workspaces"],
        center: &["mini-stats", "processes"],
        right: &["resources", "net-speed", "drives", "clock", "audio", "session"],
    },
    Preset {
        name: tk("Classic"),
        blurb: tk("Edge-to-edge frame, everything at hand"),
        layout: BarLayout::MinimalEdge,
        position: BarPosition::Top,
        height: 30.0,
        left: &["launcher", "workspaces", "active-window"],
        center: &["clock", "media"],
        right: &["weather", "resources", "clipboard", "screenshot", "network", "bluetooth", "audio", "notifications", "session"],
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
    },
    Preset {
        name: tk("Productive"),
        blurb: tk("Tasks, focus timer and system load"),
        layout: BarLayout::FloatingPills,
        position: BarPosition::Top,
        height: 30.0,
        left: &["launcher", "workspaces", "active-window"],
        center: &["clock", "pomodoro"],
        right: &["todo", "notes", "resources", "audio", "notifications", "session"],
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
    },
];

fn apply_preset(p: &'static Preset, replace_items: bool, cx: &mut App) {
    let known: Vec<String> = cx.global::<PluginRegistry>().manifests().map(|m| m.id.clone()).collect();
    update_config(cx, |c| apply_preset_in(c, p, replace_items, &known));
}

fn apply_preset_in(c: &mut ShellConfig, p: &Preset, replace_items: bool, known: &[String]) {
    let pick = |ids: &[&str]| ids.iter().filter(|i| known.iter().any(|k| k == *i)).map(|s| s.to_string()).collect::<Vec<_>>();
    let (left, center, right) = (pick(p.left), pick(p.center), pick(p.right));
    c.bar.layout = p.layout;
    c.bar.position = p.position;
    c.bar.height = p.height;
    if replace_items {
        c.bar.left.clear();
        c.bar.center.clear();
        c.bar.right.clear();
        for (ids, to) in [(left, Lane::Left), (center, Lane::Center), (right, Lane::Right)] {
            for id in ids { place_in(c, &id, Lane::Tray, to, None); }
        }
    }
    // The notch look: grouped capsules and a soft shadow under it.
    if p.layout == BarLayout::Notch {
        c.bar.capsules = true;
        c.bar.shadow = true;
    }
}

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
            let pill = |w: f32| div().w(px(w)).h(px(DIM_8_PX)).rounded_full().bg(p.primary.with_alpha(ALPHA_75).hsla());
            let mock = div()
                .h(px(DIM_44_PX))
                .rounded(px(RADIUS_10_PX))
                .bg(p.surface_base.hsla())
                .p(px(SPACE_6_PX))
                .row()
                .flex_col()
                .when(preset.position == BarPosition::Bottom, |el| el.justify_end())
                .when(preset.position.is_vertical(), |el| el.ltr_row())
                .child(match preset.layout {
                    // A side bar: a slim strip down the left edge.
                    _ if preset.position.is_vertical() => div().w(px(DIM_10_PX)).h_full().rounded(px(RADIUS_4_PX)).bg(p.surface_container_high.hsla()).py(px(SPACE_3_PX)).row().flex_col().items_center().justify_between().child(div().size(px(DIM_6_PX)).rounded_full().bg(p.primary.with_alpha(ALPHA_75).hsla())).child(div().size(px(DIM_6_PX)).rounded_full().bg(p.primary.with_alpha(ALPHA_75).hsla())),
                    BarLayout::Hairline => div().h(px(DIM_6_PX)).rounded(px(RADIUS_1_5_PX)).bg(p.surface_container_high.hsla()).border_b_1().border_color(p.primary.hsla()),
                    BarLayout::Micro | BarLayout::ContiguousIsland => div().row().justify_center().child(previews::centered(preset, p)),
                    BarLayout::MinimalEdge => div().h(px(DIM_12_PX)).rounded(px(RADIUS_4_PX)).bg(p.surface_container_high.hsla()).px(px(SPACE_4_PX)).ltr_row().items_center().justify_between().child(pill(MOCK_PILL_A_PX)).child(pill(MOCK_PILL_B_PX)).child(pill(MOCK_PILL_C_PX)),
                    BarLayout::Notch => div().h(px(DIM_12_PX)).mt(px(-SPACE_6_PX)).mx(px(SPACE_10_PX)).rounded_b(px(RADIUS_6_PX)).bg(p.surface_container_high.hsla()).px(px(SPACE_4_PX)).ltr_row().items_center().justify_between().child(pill(MOCK_PILL_A_PX)).child(pill(MOCK_PILL_B_PX)).child(pill(MOCK_PILL_C_PX)),
                    BarLayout::FloatingPills => div().h(px(DIM_12_PX)).ltr_row().items_center().justify_between().child(pill(MOCK_PILL_A_PX)).child(pill(MOCK_PILL_B_PX)).child(pill(MOCK_PILL_C_PX)),
                });
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
        for (layout, position) in [(BarLayout::Hairline, config.bar.position), (config.bar.layout, BarPosition::Bottom)] {
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
        let preset = &PRESETS[0];
        config.bar.layout = preset.layout;
        config.bar.position = preset.position;
        config.bar.center = preset.center.iter().map(|id| (*id).into()).collect();
        let mut selection = PresetSelection::default();
        assert_eq!(selection.active(&config), Some(0));
    }

    #[test]
    fn presets_reference_real_plugins_and_layouts() {
        for p in PRESETS {
            assert!(!p.center.is_empty() || !p.left.is_empty());
            assert!(p.height >= 24.0 && p.height <= 40.0);
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
}
