//! Studio → Bar / Sidebars: the drag-and-drop layout designer and bar
//! presets. Plugins are chips: drag them between the bar's left / center /
//! right lanes and the sidebar, reorder them, drop them on the tray to
//! remove them, or start from a preset. Every drop writes the config, so the
//! live shell follows instantly.

use mandara_ui::bidi::Bidi as _;
use gpui::{AnyElement, App, AppContext as _, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px};
use mandara_config::{BarLayout, BarPosition, ShellConfig};
use mandara_core::ToGpui;
use mandara_i18n::{t, tk, tr};
use mandara_plugins::PluginRegistry;
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::{StudioView, plugin_icon};
use crate::tokens::*;

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
    Tray,
}

impl Lane {
    fn list(self, c: &mut ShellConfig) -> Option<&mut Vec<String>> {
        match self {
            Lane::Left => Some(&mut c.bar.left),
            Lane::Center => Some(&mut c.bar.center),
            Lane::Right => Some(&mut c.bar.right),
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
    update_config(cx, |c| {
        // Index in the target before removal (same-lane reorders shift).
        let mut index = index;
        if from == to {
            if let (Some(list), Some(i)) = (to.list(c), index.as_mut()) {
                if let Some(old) = list.iter().position(|x| x == id) {
                    if old < *i {
                        *i -= 1;
                    }
                }
            }
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
    });
}

/// Sidebar-capable built-ins (third-party plugins all can).
pub(super) const SIDEBAR_CAPABLE: &[&str] = mandara_ui::sidebar_tabs::SIDEBAR_WIDGETS;
/// Built-ins without a bar item.
pub(super) const NO_BAR: &[&str] = &["main-menu", "compositor"];

/// Minimum height of a widget lane (vertical bars stack their chips).
const LANE_MIN_H_PX: f32 = 46.0;
const LANE_MIN_H_VERTICAL_PX: f32 = 120.0;
/// Widths of the widget pills in a preset's tiny bar drawing.
const MOCK_PILL_A_PX: f32 = 18.0;
const MOCK_PILL_B_PX: f32 = 14.0;
const MOCK_PILL_C_PX: f32 = 22.0;
const MOCK_PILL_SMALL_PX: f32 = 10.0;

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

fn apply_preset(p: &'static Preset, cx: &mut App) {
    let known: Vec<String> = cx.global::<PluginRegistry>().manifests().map(|m| m.id.clone()).collect();
    let pick = |ids: &[&str]| ids.iter().filter(|i| known.iter().any(|k| k == *i)).map(|s| s.to_string()).collect::<Vec<_>>();
    let (left, center, right) = (pick(p.left), pick(p.center), pick(p.right));
    update_config(cx, |c| {
        c.bar.layout = p.layout;
        c.bar.position = p.position;
        c.bar.height = p.height;
        c.bar.left = left;
        c.bar.center = center;
        c.bar.right = right;
    });
}

impl StudioView {
    fn chip(&self, id: &str, lane: Lane, index: usize, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let name = cx.global::<PluginRegistry>().manifests().find(|m| m.id == id).map(|m| m.display_name()).unwrap_or_else(|| id.to_string());
        let ic = plugin_icon(id);
        let enabled = {
            let cfg = mandara_ui::state::config(cx).read(cx).snapshot();
            cx.global::<PluginRegistry>().is_enabled(id, &cfg.plugins)
        };
        let drag = DragChip { id: id.to_string(), from: lane, label: name.clone().into(), icon: ic, bg: p.primary, fg: p.on_primary };
        let (id_rm, id_drop) = (id.to_string(), id.to_string());
        div()
            .id(SharedString::from(format!("chip-{lane:?}-{id}")))
            .group("chip")
            .h(px(DIM_30_PX))
            .pl(px(SPACE_10_PX))
            .pr(px(if lane == Lane::Tray { SPACE_12_PX } else { SPACE_6_PX }))
            .rounded_full()
            .row()
            .flex_none()
            .items_center()
            .gap(px(SPACE_6_PX))
            .cursor_grab()
            .bg(if lane == Lane::Tray { p.surface_container_high.hsla() } else { p.surface_base.hsla() })
            .border_1()
            .border_color(p.on_surface.with_alpha(ALPHA_8).hsla())
            .text_size(px(TEXT_12_PX))
            .font_weight(FontWeight::MEDIUM)
            .hover(|s| s.border_color(p.primary.with_alpha(ALPHA_60).hsla()))
            .when(!enabled, |el| el.opacity(ALPHA_60))
            .child(icon(ic, ICON_13_PX, p.on_surface_variant))
            .child(name)
            .when(!enabled, |el| el.child(div().px(px(SPACE_5_PX)).rounded(px(RADIUS_5_PX)).bg(p.on_surface.with_alpha(ALPHA_8).hsla()).text_size(px(TEXT_10_PX)).child(t("off"))))
            .when(lane != Lane::Tray, |el| {
                el.child(
                    div()
                        .id(SharedString::from(format!("chip-rm-{lane:?}-{id}")))
                        .size(px(DIM_18_PX))
                        .rounded_full()
                        .row()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .invisible()
                        .group_hover("chip", |s| s.visible())
                        .hover(|s| s.bg(p.urgent_coral.with_alpha(ALPHA_16).hsla()))
                        .child(icon("close", ICON_10_PX, p.on_surface_variant))
                        .on_click(move |_, _, cx| place(&id_rm, lane, Lane::Tray, None, cx)),
                )
            })
            .on_drag(drag, |chip, _, _, cx| cx.new(|_| chip.clone()))
            // Dropping on a chip inserts before it.
            .drag_over::<DragChip>(move |s, _, _, _| s.border_color(gpui::rgb(0x6aa0ff)).border_2())
            .on_drop(move |chip: &DragChip, _, cx| {
                if chip.id != id_drop {
                    place(&chip.id, chip.from, lane, Some(index), cx);
                }
            })
            .into_any_element()
    }

    pub(super) fn lane(&self, lane: Lane, title: &'static str, ids: &[String], vertical: bool, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let chips: Vec<AnyElement> = ids.iter().enumerate().map(|(i, id)| self.chip(id, lane, i, p, cx)).collect();
        let empty = chips.is_empty();
        div()
            .id(SharedString::from(format!("lane-{lane:?}")))
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(if vertical { LANE_MIN_H_VERTICAL_PX } else { LANE_MIN_H_PX }))
            .p(px(SPACE_8_PX))
            .rounded(px(RADIUS_16_PX))
            .border_1()
            .border_color(p.on_surface.with_alpha(ALPHA_6).hsla())
            .bg(p.on_surface.with_alpha(ALPHA_3).hsla())
            // Bar zones mirror the bar itself: physical left to right in
            // every language; the plugin tray reads in reading order.
            .when(lane == Lane::Tray, |el| el.row())
            .when(lane != Lane::Tray, |el| el.ltr_row())
            .when(vertical, |el| el.flex_col())
            .when(!vertical, |el| el.flex_wrap().items_center())
            .gap(px(SPACE_6_PX))
            .drag_over::<DragChip>(|s, _, _, _| s.bg(gpui::rgba(0x6aa0ff22)).border_color(gpui::rgba(0x6aa0ffaa)))
            .on_drop(move |chip: &DragChip, _, cx| place(&chip.id, chip.from, lane, None, cx))
            .child(div().w_full().text_size(px(TEXT_10_5_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.with_alpha(ALPHA_80).hsla()).child(mandara_ui::bidi::paragraph(tr(title))))
            .children(chips)
            .when(empty, |el| el.child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.with_alpha(ALPHA_60).hsla()).child(t("Drop plugins here"))))
            .into_any_element()
    }

    /// The bar designer with presets (Studio → Bar).
    pub(super) fn bar_designer(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let registry = cx.global::<PluginRegistry>();
        let placed: Vec<&String> = cfg.bar.left.iter().chain(&cfg.bar.center).chain(&cfg.bar.right).collect();
        let mut tray: Vec<String> = registry
            .manifests()
            .filter(|m| !NO_BAR.contains(&m.id.as_str()) && !placed.contains(&&m.id) && !cfg.plugins.is_removed(&m.id))
            .map(|m| m.id.clone())
            .collect();
        tray.sort();

        // Presets.
        let mut presets: Vec<AnyElement> = Vec::with_capacity(PRESETS.len());
        for (i, preset) in PRESETS.iter().enumerate() {
            let active = cfg.bar.layout == preset.layout && cfg.bar.position == preset.position && cfg.bar.center.iter().map(String::as_str).eq(preset.center.iter().copied());
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
                    BarLayout::Micro => div().h(px(DIM_10_PX)).row().justify_center().child(div().w(px(DIM_40_PX)).h(px(DIM_10_PX)).rounded_full().bg(p.surface_container_high.hsla())),
                    BarLayout::MinimalEdge => div().h(px(DIM_12_PX)).rounded(px(RADIUS_4_PX)).bg(p.surface_container_high.hsla()).px(px(SPACE_4_PX)).ltr_row().items_center().justify_between().child(pill(MOCK_PILL_A_PX)).child(pill(MOCK_PILL_B_PX)).child(pill(MOCK_PILL_C_PX)),
                    BarLayout::FloatingPills => div().h(px(DIM_12_PX)).ltr_row().items_center().justify_between().child(pill(MOCK_PILL_A_PX)).child(pill(MOCK_PILL_B_PX)).child(pill(MOCK_PILL_C_PX)),
                    BarLayout::ContiguousIsland => div().h(px(DIM_12_PX)).row().justify_center().child(div().h(px(DIM_12_PX)).px(px(SPACE_4_PX)).rounded_full().bg(p.surface_container_high.hsla()).row().items_center().gap(px(SPACE_4_PX)).child(pill(MOCK_PILL_SMALL_PX)).child(pill(MOCK_PILL_B_PX)).child(pill(MOCK_PILL_SMALL_PX))),
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
                    .on_click(move |_, _, cx| apply_preset(preset, cx))
                    .into_any_element(),
            );
        }
        let presets = super::grid::fit(self.page.column_w, BAR_PRESET_GRID).balanced(presets.len()).render(presets);

        let bar = div()
            .rounded(px(RADIUS_20_PX))
            .p(px(SPACE_10_PX))
            .bg(p.surface_container.hsla())
            // Left / Center / Right are screen sides, not reading order.
            .ltr_row()
            .when(self.stack_rows, |el| el.flex_col())
            .gap(px(SPACE_8_PX))
            .child(self.lane(Lane::Left, tk("Left"), &cfg.bar.left, false, p, cx))
            .child(self.lane(Lane::Center, tk("Center"), &cfg.bar.center, false, p, cx))
            .child(self.lane(Lane::Right, tk("Right"), &cfg.bar.right, false, p, cx));
        let tray_el = self.lane(Lane::Tray, tk("All plugins — drag onto the bar (turns it on), or drop here to remove"), &tray, false, p, cx);
        let header = |title: &'static str, sub: &'static str| {
            div()
                .px(px(SPACE_6_PX))
                .row()
                .flex_col()
                .gap(px(2.0))
                .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t(title)))
                .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(t(sub)))
        };
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_12_PX))
            .child(header("Presets", "One click sets the style, position, height and arrangement"))
            .child(presets)
            .child(header("Layout designer", "Drag plugins between zones, reorder them, or drop them on the tray to remove"))
            .child(bar)
            .child(tray_el)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_reference_real_plugins_and_layouts() {
        for p in PRESETS {
            assert!(!p.center.is_empty() || !p.left.is_empty());
            assert!(p.height >= 24.0 && p.height <= 40.0);
        }
    }
}
