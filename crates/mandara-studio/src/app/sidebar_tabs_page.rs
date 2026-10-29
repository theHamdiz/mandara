//! Studio → Sidebars & hover → Tabs: the sidebar's tabs and the widgets on
//! each. Tabs: select, rename in place, pick an icon, drag to reorder,
//! delete (asks once). Widgets of the selected tab: drag to reorder, drop
//! on another tab to move them there, × to take them off, and a searchable
//! picker of every widget (plugin art, description; off or removed
//! plugins greyed with "Turn on"). Every edit writes the config, so an open
//! sidebar follows at once.

use std::rc::Rc;

use gpui::{
    AnyElement, AppContext as _, ClickEvent, Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px,
};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{PluginOrigin, PluginRegistry};
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::{any_icon, icon};
use mandara_ui::sidebar_tabs::{SIDEBAR_WIDGETS, TAB_ICONS, tab_label, turn_on};
use mandara_ui::state::update_config;

use super::designer::{DragChip, Lane, place};
use super::{StudioView, plugin_tile};
use crate::text_field::TextField;
use crate::tokens::*;

/// Editor state kept on [`StudioView`].
#[derive(Default)]
pub struct TabsEditor {
    /// Index of the tab being edited.
    selected: usize,
    /// The icon grid is open for the selected tab.
    icons_open: bool,
    /// Tab id whose delete button was pressed once.
    confirm_delete: Option<String>,
    search: Option<Entity<TextField>>,
    query: String,
}

/// What's dragged when a tab is reordered.
#[derive(Clone)]
struct DragTab {
    index: usize,
    label: SharedString,
    icon: String,
    bg: Color,
    fg: Color,
}

impl Render for DragTab {
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
            .child(any_icon(&self.icon, ICON_14_PX, self.fg))
            .child(self.label.clone())
    }
}

/// Widget rows shown in the picker before the list scrolls.
const PICKER_MAX_H_PX: f32 = DIM_320_PX;
/// Tab list column width on wide windows.
const TAB_LIST_W_PX: f32 = DIM_260_PX;

impl StudioView {
    fn widget_search(&mut self, cx: &mut Context<Self>) -> Entity<TextField> {
        if let Some(f) = &self.sidebar_tabs.search {
            return f.clone();
        }
        let view = cx.entity().downgrade();
        let field = cx.new(|cx| {
            TextField::new(
                String::new(),
                t("Search widgets"),
                true,
                Rc::new(move |value, _, cx| {
                    let _ = view.update(cx, |v, cx| {
                        v.sidebar_tabs.query = value;
                        cx.notify();
                    });
                }),
                cx,
            )
        });
        self.sidebar_tabs.search = Some(field.clone());
        field
    }

    fn tab_row(&mut self, cfg: &ShellConfig, i: usize, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let tab = &cfg.sidebar.tabs[i];
        let selected = self.sidebar_tabs.selected == i;
        let confirming = self.sidebar_tabs.confirm_delete.as_deref() == Some(tab.id.as_str());
        let tab_id = tab.id.clone();
        let name_field = self.list_field(
            format!("sidebar-tab-name-{}", tab.id),
            &tab_label(tab),
            tk("Tab name"),
            Rc::new(move |c, v| {
                if let Some(i) = c.sidebar.tabs.iter().position(|t| t.id == tab_id) {
                    let _ = c.sidebar.rename_tab(i, &v);
                }
            }),
            cx,
        );
        let drag = DragTab { index: i, label: tab_label(tab).into(), icon: tab.icon.clone(), bg: p.primary, fg: p.on_primary };
        let del_id = tab.id.clone();
        let can_delete = cfg.sidebar.tabs.len() > 1;
        div()
            .id(("studio-tab", i))
            .p(px(SPACE_8_PX))
            .rounded(px(RADIUS_14_PX))
            .row()
            .items_center()
            .gap(px(SPACE_8_PX))
            .cursor_pointer()
            .bg(if selected { p.primary.with_alpha(ALPHA_12).hsla() } else { p.surface_base.hsla() })
            .border_1()
            .border_color(if selected { p.primary.with_alpha(ALPHA_60).hsla() } else { p.on_surface.with_alpha(ALPHA_6).hsla() })
            .child(icon("grid", ICON_12_PX, p.on_surface_variant.with_alpha(ALPHA_60)).into_any_element())
            .child(
                div()
                    .size(px(DIM_30_PX))
                    .flex_none()
                    .rounded(px(RADIUS_10_PX))
                    .row()
                    .items_center()
                    .justify_center()
                    .bg(p.surface_container_high.hsla())
                    .child(any_icon(&tab.icon, ICON_15_PX, if selected { p.primary } else { p.on_surface })),
            )
            .child(div().flex_1().min_w(px(0.0)).child(name_field))
            .child(
                div()
                    .px(px(SPACE_6_PX))
                    .h(px(DIM_20_PX))
                    .rounded_full()
                    .row()
                    .items_center()
                    .bg(p.surface_container_high.hsla())
                    .text_size(px(TEXT_11_PX))
                    .font_features(mandara_ui::fonts::tabular_nums())
                    .child(tab.cards.len().to_string()),
            )
            .when(can_delete, |el| {
                el.child(
                    div()
                        .id(("studio-tab-delete", i))
                        .h(px(DIM_24_PX))
                        .px(px(if confirming { SPACE_8_PX } else { SPACE_4_PX }))
                        .rounded_full()
                        .row()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_size(px(TEXT_11_5_PX))
                        .text_color(p.urgent_coral.hsla())
                        .when(confirming, |el| el.bg(p.urgent_coral.with_alpha(ALPHA_16).hsla()).child(t("Delete?")))
                        .when(!confirming, |el| el.hover(|s| s.bg(p.urgent_coral.with_alpha(ALPHA_16).hsla())).child(icon("trash", ICON_13_PX, p.on_surface_variant)))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            if this.sidebar_tabs.confirm_delete.as_deref() == Some(del_id.as_str()) {
                                let id = del_id.clone();
                                update_config(cx, move |c| {
                                    if let Some(i) = c.sidebar.tabs.iter().position(|t| t.id == id) {
                                        let _ = c.sidebar.remove_tab(i);
                                    }
                                });
                                this.sidebar_tabs.confirm_delete = None;
                                this.sidebar_tabs.selected = this.sidebar_tabs.selected.saturating_sub(1);
                            } else {
                                this.sidebar_tabs.confirm_delete = Some(del_id.clone());
                            }
                            cx.notify();
                        })),
                )
            })
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.sidebar_tabs.selected = i;
                this.sidebar_tabs.icons_open = false;
                this.sidebar_tabs.confirm_delete = None;
                cx.notify();
            }))
            .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
            .drag_over::<DragTab>(|s, _, _, _| s.border_color(gpui::rgb(0x6aa0ff)).border_2())
            .drag_over::<DragChip>(|s, _, _, _| s.bg(gpui::rgba(0x6aa0ff22)).border_color(gpui::rgba(0x6aa0ffaa)))
            .on_drop(cx.listener(move |this, d: &DragTab, _, cx| {
                let from = d.index;
                update_config(cx, move |c| {
                    let _ = c.sidebar.move_tab(from, i);
                });
                this.sidebar_tabs.selected = i;
                cx.notify();
            }))
            // A widget dropped on a tab moves (or, from the picker, goes) there.
            .on_drop(move |chip: &DragChip, _, cx| place(&chip.id, chip.from, Lane::Tab(i), None, cx))
            .into_any_element()
    }

    /// The picker row of one widget.
    fn widget_row(&self, m: &mandara_plugins::PluginManifest, tab: usize, on_tab: bool, enabled: bool, scale: f32, p: &MandaraPalette) -> AnyElement {
        let id = m.id.clone();
        let (label, color) = if on_tab {
            (t("Added"), p.on_surface_variant)
        } else if enabled {
            (t("Add"), p.primary)
        } else {
            (t("Turn on"), p.primary)
        };
        let drag = DragChip::new(m.id.clone(), Lane::Tray, m.display_name().into(), super::plugin_icon(&m.id), p.primary, p.on_primary);
        div()
            .id(SharedString::from(format!("widget-pick-{}", m.id)))
            .p(px(SPACE_8_PX))
            .rounded(px(RADIUS_12_PX))
            .row()
            .items_center()
            .gap(px(SPACE_10_PX))
            .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_5).hsla()))
            .when(!on_tab, |el| el.cursor_grab().on_drag(drag, |chip, _, _, cx| cx.new(|_| chip.clone())))
            .child(plugin_tile(m, DIM_34_PX, enabled, scale, p))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .row()
                    .flex_col()
                    .when(!enabled, |el| el.opacity(ALPHA_60))
                    .child(div().text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).truncate().child(m.display_name()))
                    .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).truncate().child(m.display_description())),
            )
            .child(
                div()
                    .id(SharedString::from(format!("widget-add-{}", m.id)))
                    .h(px(DIM_28_PX))
                    .px(px(SPACE_12_PX))
                    .rounded_full()
                    .row()
                    .items_center()
                    .text_size(px(TEXT_12_PX))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color.hsla())
                    .when(!on_tab, |el| {
                        el.cursor_pointer().bg(p.primary.with_alpha(ALPHA_12).hsla()).hover(|s| s.bg(p.primary.with_alpha(ALPHA_20).hsla())).on_click(move |_, _, cx| {
                            let id = id.clone();
                            update_config(cx, move |c| {
                                if c.sidebar.add_card(tab, &id, None).is_ok() {
                                    turn_on(c, &id);
                                }
                            });
                        })
                    })
                    .child(label),
            )
            .into_any_element()
    }

    /// Studio → Sidebars & hover → Tabs.
    pub(super) fn sidebar_tabs_editor(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let tabs = &cfg.sidebar.tabs;
        self.sidebar_tabs.selected = self.sidebar_tabs.selected.min(tabs.len().saturating_sub(1));
        let sel = self.sidebar_tabs.selected;
        let rows: Vec<AnyElement> = (0..tabs.len()).map(|i| self.tab_row(cfg, i, p, cx)).collect();
        let add_tab = div()
            .id("studio-tab-add")
            .h(px(DIM_36_PX))
            .rounded(px(RADIUS_12_PX))
            .row()
            .items_center()
            .justify_center()
            .gap(px(SPACE_6_PX))
            .cursor_pointer()
            .border_1()
            .border_color(p.on_surface.with_alpha(ALPHA_10).hsla())
            .text_size(px(TEXT_12_5_PX))
            .font_weight(FontWeight::MEDIUM)
            .text_color(p.primary.hsla())
            .hover(|s| s.bg(p.primary.with_alpha(ALPHA_8).hsla()))
            .child(icon("plus", ICON_14_PX, p.primary))
            .child(t("New tab"))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                let count = mandara_ui::state::config(cx).read(cx).snapshot().sidebar.tabs.len();
                update_config(cx, |c| {
                    let name = c.sidebar.free_tab_name(t("New tab"));
                    let _ = c.sidebar.add_tab(&name, None);
                });
                this.sidebar_tabs.selected = count;
                cx.notify();
            }));
        let builtin_note = cfg.sidebar.notifications_tab.then(|| {
            div()
                .p(px(SPACE_8_PX))
                .row()
                .items_center()
                .gap(px(SPACE_8_PX))
                .text_size(px(TEXT_11_5_PX))
                .text_color(p.on_surface_variant.hsla())
                .child(icon("bell", ICON_13_PX, p.on_surface_variant))
                .child(div().flex_1().min_w(px(0.0)).child(mandara_ui::bidi::paragraph(t("Notifications is a built-in tab after yours (switch it under Behaviour)"))))
        });
        let list = div()
            .when(!self.stack_rows, |el| el.w(px(TAB_LIST_W_PX)).flex_none())
            .row()
            .flex_col()
            .gap(px(SPACE_6_PX))
            .children(rows)
            .child(add_tab)
            .children(builtin_note);

        // The selected tab: icon, widgets, picker.
        let tab = &tabs[sel];
        let icons_open = self.sidebar_tabs.icons_open;
        let icon_button = div()
            .id("studio-tab-icon")
            .size(px(DIM_40_PX))
            .flex_none()
            .rounded(px(RADIUS_12_PX))
            .row()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(if icons_open { p.primary.with_alpha(ALPHA_16).hsla() } else { p.surface_container_high.hsla() })
            .hover(|s| s.bg(p.primary.with_alpha(ALPHA_10).hsla()))
            .child(any_icon(&tab.icon, ICON_22_PX, p.primary))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.sidebar_tabs.icons_open = !this.sidebar_tabs.icons_open;
                cx.notify();
            }));
        let head = div()
            .row()
            .items_center()
            .gap(px(SPACE_10_PX))
            .child(icon_button)
            .child(
                div()
                    .row()
                    .flex_col()
                    .child(div().text_size(px(TEXT_14_PX)).font_weight(FontWeight::SEMIBOLD).child(tf("Widgets on “{tab}”", &[("tab", &tab_label(tab))])))
                    .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Top to bottom. Drag to reorder, onto another tab to move, × to take off")))),
            );
        let icon_grid = icons_open.then(|| {
            let current = tab.icon.clone();
            let tiles = TAB_ICONS.iter().enumerate().map(|(n, name)| {
                let active = *name == current;
                div()
                    .id(("studio-tab-icon-pick", n))
                    .size(px(DIM_36_PX))
                    .rounded(px(RADIUS_10_PX))
                    .row()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(if active { p.primary.with_alpha(ALPHA_16).hsla() } else { p.surface_container_high.hsla() })
                    .border_2()
                    .border_color(if active { p.primary.hsla() } else { gpui::transparent_black() })
                    .hover(|s| s.bg(p.primary.with_alpha(ALPHA_10).hsla()))
                    .child(icon(name, ICON_16_PX, if active { p.primary } else { p.on_surface }))
                    .on_click(move |_, _, cx| {
                        update_config(cx, move |c| {
                            let _ = c.sidebar.set_tab_icon(sel, name);
                        })
                    })
            });
            div().p(px(SPACE_10_PX)).rounded(px(RADIUS_14_PX)).bg(p.surface_base.with_alpha(ALPHA_55).hsla()).row().flex_wrap().gap(px(SPACE_6_PX)).children(tiles)
        });
        let lane = self.lane(Lane::Tab(sel), tk("On this tab"), &tab.cards, true, p, cx);

        // Picker: every widget, filtered by the search.
        let search = self.widget_search(cx);
        let query = self.sidebar_tabs.query.trim().to_lowercase();
        let scale = window.scale_factor();
        let registry = cx.global::<PluginRegistry>();
        let mut widgets: Vec<(mandara_plugins::PluginManifest, bool)> = registry
            .manifests()
            .filter(|m| SIDEBAR_WIDGETS.contains(&m.id.as_str()) || matches!(m.origin, PluginOrigin::ThirdParty { .. }))
            .filter(|m| query.is_empty() || m.display_name().to_lowercase().contains(&query) || m.display_description().to_lowercase().contains(&query) || m.id.contains(&query))
            .map(|m| (m.clone(), registry.is_enabled(&m.id, &cfg.plugins) && !cfg.plugins.is_removed(&m.id)))
            .collect();
        widgets.sort_by_key(|(m, _)| m.display_name().to_lowercase());
        let picks: Vec<AnyElement> = widgets.iter().map(|(m, on)| self.widget_row(m, sel, tab.cards.contains(&m.id), *on, scale, p)).collect();
        let empty = picks.is_empty();
        let picker = div()
            .p(px(SPACE_10_PX))
            .rounded(px(RADIUS_16_PX))
            .bg(p.on_surface.with_alpha(ALPHA_3).hsla())
            .border_1()
            .border_color(p.on_surface.with_alpha(ALPHA_6).hsla())
            .row()
            .flex_col()
            .gap(px(SPACE_6_PX))
            .child(div().text_size(px(TEXT_10_5_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.with_alpha(ALPHA_80).hsla()).child(t("Add a widget")))
            .child(div().child(search))
            .child(div().id("studio-widget-picker").max_h(px(PICKER_MAX_H_PX)).overflow_y_scroll().row().flex_col().gap(px(SPACE_3_PX)).children(picks))
            .when(empty, |el| el.child(div().p(px(SPACE_8_PX)).text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(tf("No widgets match “{query}”", &[("query", &query)]))));
        let detail = div().flex_1().min_w(px(0.0)).row().flex_col().gap(px(SPACE_10_PX)).child(head).children(icon_grid).child(lane).child(picker);
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_12_PX))
            .child(
                div()
                    .px(px(SPACE_6_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_3_PX))
                    .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t("Sidebar tabs")))
                    .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Make tabs, name them, give them an icon and put any widget on any of them. The open sidebar follows at once.")))),
            )
            .child(div().row().when(self.stack_rows, |el| el.flex_col()).gap(px(SPACE_12_PX)).items_start().child(list).child(detail))
            .into_any_element()
    }
}
