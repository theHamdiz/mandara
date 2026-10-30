//! Studio → Dock: turn the dock on, a live preview, then the settings in
//! five groups — Layout, Behaviour, Apps (the apps kept in the dock with
//! the searchable "Add app…" picker, drag to reorder, remove), Stacks &
//! Trash (the folders on the right side) and Appearance — each with its
//! own reset.

use std::collections::HashSet;
use std::rc::Rc;

use gpui::{AnyElement, App, AppContext as _, ClickEvent, Context, Entity, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::workflow::{DockAutoHide, DockConfig, DockIndicator, StackDisplay, StackSort, StackView};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tr};
use mandara_plugins::PluginRegistry;
use mandara_theme::MandaraPalette;
use mandara_ui::apps::App as DesktopApp;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::plugins::dock::picker::{rank_apps, selection_changes};
use mandara_ui::state::update_config;

use super::StudioView;
use super::compositor_page::{button, card, chip, group_title, heading};
use crate::registry::workflow_settings::{DOCK_APPLICATIONS, DOCK_APPS, DOCK_BEHAVIOUR, DOCK_LAYOUT, DOCK_LOOK, DOCK_STACKS, STACK_DISPLAYS, STACK_SORTS, STACK_VIEWS};
use crate::registry::{self, Section};
use crate::text_field::TextField;
use crate::tokens::*;

/// Plugin ids.
const DOCK: &str = "dock";
/// Sample apps drawn in the preview when nothing is pinned.
const SAMPLE_APPS: &[&str] = &["org.gnome.Nautilus", "firefox", "com.mitchellh.ghostty", "dev.zed.Zed", "org.gnome.Settings"];
/// Preview: icons shown, which one is "under the pointer", and the
/// preview's scale relative to the real dock.
const PREVIEW_ICONS: usize = 7;
const PREVIEW_HOVER: usize = 2;
const PREVIEW_SCALE: f32 = 0.85;
/// Room above the preview strip for magnified icons.
const PREVIEW_HEADROOM_PX: f32 = 36.0;
/// Running dots in the preview.
const DOT_PX: f32 = 4.0;
const DOT_GAP_PX: f32 = 3.0;
const BAR_PX: f32 = 10.0;
const BAR_FOCUSED_PX: f32 = 18.0;
/// The preview's separator and right-side folder.
const PREVIEW_SEPARATOR_PX: f32 = 1.0;
/// Pinned-app rows.
const PIN_ICON_PX: f32 = 28.0;
const PIN_ROW_H_PX: f32 = 46.0;
const HANDLE_ICON_PX: f32 = 14.0;
/// Picker modal.
const PICKER_W_PX: f32 = 680.0;
const PICKER_H_PX: f32 = 560.0;
const PICK_TILE_W_PX: f32 = 116.0;
const PICK_TILE_H_PX: f32 = 104.0;
const PICK_ICON_PX: f32 = 44.0;
const PICK_CHECK_PX: f32 = 18.0;
const PICK_NAME_CHARS: usize = 16;
const PICK_MAX: usize = 150;
const SCRIM_ALPHA: f32 = 0.45;

// ── Picker state ─────────────────────────────────────────────────────────

/// The "Add app…" modal (one per Studio window).
#[derive(Default)]
struct DockPicker {
    open: bool,
    apps: Option<Rc<[DesktopApp]>>,
    query: String,
    selected: HashSet<String>,
    search: Option<Entity<TextField>>,
}
impl Global for DockPicker {}

/// A pinned app being dragged in the list.
#[derive(Clone)]
struct PinDrag {
    index: usize,
    id: String,
    name: String,
}

struct PinGhost(PinDrag, MandaraPalette);
impl Render for PinGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let p = &self.1;
        div()
            .h(px(PIN_ROW_H_PX))
            .px(px(SPACE_12_PX))
            .rounded(px(RADIUS_12_PX))
            .bg(p.surface_container_high.hsla())
            .text_color(p.on_surface.hsla())
            .row()
            .items_center()
            .gap(px(SPACE_10_PX))
            .child(mandara_ui::plugins::workspaces::app_icon(&self.0.id, PIN_ICON_PX, p.on_surface, false, p))
            .child(self.0.name.clone())
    }
}

/// An on/off card for a built-in plugin.
pub(super) fn plugin_switch(id: &'static str, title: &'static str, detail: &'static str, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<StudioView>) -> AnyElement {
    let enabled = cx.try_global::<PluginRegistry>().is_some_and(|r| r.is_enabled(id, &cfg.plugins));
    card(p)
        .child(
            div()
                .row()
                .items_center()
                .gap(px(SPACE_14_PX))
                .child(div().flex_1().min_w(px(0.0)).child(heading(title, detail, p)))
                .child(
                    div()
                        .id(SharedString::from(format!("plugin-switch-{id}")))
                        .flex_none()
                        .cursor_pointer()
                        .child(mandara_ui::plugins::network::switch(enabled, p))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.plugin_lifecycle(id, if enabled { mandara_ui::plugins::lifecycle::Action::Disable } else { mandara_ui::plugins::lifecycle::Action::Enable }, cx);
                            // A just-enabled dock shows itself for a moment
                            // in the shell, whatever its hiding mode.
                        })),
                ),
        )
        .into_any_element()
}

/// A static rendering of the dock with the current settings: pinned apps,
/// a separator and the right side (a stack, the Trash).
fn preview(d: &DockConfig, p: &MandaraPalette) -> AnyElement {
    let ids: Vec<String> = if d.pinned.is_empty() { SAMPLE_APPS.iter().map(|s| s.to_string()).collect() } else { d.pinned.clone() };
    let base = d.icon_size * PREVIEW_SCALE;
    let accent = mandara_ui::plugins::panel_fx::readable(p.primary, p);
    let dot = |focused: bool| div().size(px(DOT_PX)).rounded_full().bg(if focused { accent.hsla() } else { p.on_surface.with_alpha(ALPHA_55).hsla() });
    let mut icons: Vec<AnyElement> = ids
        .iter()
        .take(PREVIEW_ICONS)
        .enumerate()
        .map(|(i, id)| {
            let dist = i as f32 - PREVIEW_HOVER as f32;
            let scale = if d.magnification { mandara_ui::plugins::dock::magnification(dist, d.magnify, d.magnify_spread) } else { 1.0 };
            let size = base * scale;
            let running = i % 2 == 0;
            let marks = div()
                .h(px(DOT_PX))
                .row()
                .gap(px(DOT_GAP_PX))
                .when(running && d.indicator == DockIndicator::Dots, |el| el.child(dot(i == 0)).when(i == 2, |el| el.child(dot(false))))
                .when(running && d.indicator == DockIndicator::Dot, |el| el.child(dot(i == 0)))
                .when(running && d.indicator == DockIndicator::Bar, |el| {
                    el.child(div().w(px(if i == 0 { BAR_FOCUSED_PX } else { BAR_PX })).h(px(DOT_PX)).rounded_full().bg(if i == 0 { accent.hsla() } else { p.on_surface.with_alpha(ALPHA_55).hsla() }))
                });
            div()
                .row()
                .flex_col()
                .items_center()
                .justify_end()
                .gap(px(d.padding * 0.3))
                .opacity(if running { 1.0 } else { d.inactive_opacity })
                .child(mandara_ui::plugins::workspaces::app_icon(id, size, p.on_surface, false, p))
                .child(marks)
                .into_any_element()
        })
        .collect();
    let right = d.applications.enabled || !d.stacks.is_empty() || d.trash;
    if right {
        if d.separator {
            icons.push(div().w(px(PREVIEW_SEPARATOR_PX)).h(px(base * 0.6)).mb(px(base * 0.2)).bg(p.on_surface.with_alpha(ALPHA_20).hsla()).into_any_element());
        }
        if d.applications.enabled {
            icons.push(div().pb(px(DOT_PX + d.padding * 0.3)).child(mandara_ui::plugins::dock::applications::folder_tile(base, p)).into_any_element());
        }
        if !d.stacks.is_empty() {
            icons.push(div().pb(px(DOT_PX + d.padding * 0.3)).child(mandara_ui::plugins::dock::themed(&["folder-download", "folder-downloads", "folder"], "download", base, p, p.primary)).into_any_element());
        }
        if d.trash {
            icons.push(div().pb(px(DOT_PX + d.padding * 0.3)).child(mandara_ui::plugins::dock::themed(&["user-trash", "trash-empty"], "trash", base, p, p.on_surface_variant)).into_any_element());
        }
    }
    let mut shadow = Vec::new();
    if d.shadow {
        shadow.extend(mandara_ui::plugins::panel_fx::elevation(p, 1.0));
    }
    if d.rim {
        shadow.extend(mandara_ui::plugins::panel_fx::rim(p));
    }
    let thickness = base + d.padding * 2.0;
    let fill = mandara_ui::shell::frame_color(p).mix(p.primary, d.tint).with_alpha(d.opacity.clamp(0.0, 1.0));
    div()
        .h(px(thickness + PREVIEW_HEADROOM_PX + d.margin))
        .rounded(px(RADIUS_16_PX))
        .bg(mandara_core::oklab_gradient(mandara_ui::plugins::panel_fx::ANGLE_DIAGONAL_DEG, p.primary_container.with_alpha(ALPHA_30), p.tertiary.with_alpha(ALPHA_20)))
        .row()
        .items_end()
        .justify_center()
        .pb(px(d.margin))
        .child(
            div()
                .relative()
                .h(px(thickness))
                .px(px(d.padding))
                .row()
                .items_end()
                .gap(px(d.spacing))
                .pb(px(d.padding * 0.3))
                .child(div().absolute().inset_0().rounded(px(d.radius.min(thickness / 2.0))).bg(fill.hsla()).shadow(shadow))
                .children(icons),
        )
        .into_any_element()
}

fn picker(cx: &mut App) -> &mut DockPicker {
    cx.default_global::<DockPicker>()
}

/// Opens the "Add app…" modal: installed apps (as the launcher lists
/// them), pinned ones checked.
fn open_picker(window: &mut Window, cx: &mut Context<StudioView>) {
    let cfg = mandara_ui::state::config(cx).read(cx).get().clone();
    let apps: Rc<[DesktopApp]> = mandara_ui::launcher_ext::customize(mandara_ui::apps::discover(), &cfg.launcher).into();
    let selected = apps.iter().filter(|a| cfg.dock.is_pinned(&a.id)).map(|a| a.id.clone()).collect();
    let search = match picker(cx).search.clone() {
        Some(f) => f,
        None => {
            let studio = cx.entity().downgrade();
            let f = cx.new(|cx| {
                TextField::new(
                    String::new(),
                    t("Search installed apps"),
                    true,
                    Rc::new(move |value, _, cx| {
                        cx.default_global::<DockPicker>().query = value;
                        let _ = studio.update(cx, |_, cx| cx.notify());
                    }),
                    cx,
                )
            });
            picker(cx).search = Some(f.clone());
            f
        }
    };
    search.update(cx, |f, cx| {
        f.set("");
        f.focus(window, cx);
    });
    let st = picker(cx);
    st.apps = Some(apps);
    st.selected = selected;
    st.query.clear();
    st.open = true;
    cx.notify();
}

fn close_picker(apply: bool, cx: &mut Context<StudioView>) {
    if apply {
        let st = picker(cx);
        let (selected, apps) = (st.selected.clone(), st.apps.clone().unwrap_or_else(|| Rc::from(Vec::new())));
        let pinned = mandara_ui::state::config(cx).read(cx).get().dock.pinned.clone();
        let (pin, unpin) = selection_changes(&pinned, &selected, &apps);
        if !pin.is_empty() || !unpin.is_empty() {
            update_config(cx, move |c| {
                for id in &unpin {
                    c.dock.unpin(id);
                }
                for id in &pin {
                    c.dock.pin(id);
                }
            });
        }
    }
    picker(cx).open = false;
    cx.notify();
}

fn ipc(cx: &App, args: &[&str]) {
    if let Some(ipc) = cx.try_global::<crate::perf::ShellIpc>().cloned() {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        std::thread::spawn(move || {
            if let Err(err) = (ipc.0)(args) {
                tracing::warn!("dock: {err}");
            }
        });
    }
}

impl StudioView {
    pub(super) fn dock_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let toggle = plugin_switch(DOCK, t("Show the dock"), t("Pinned, running and recent apps, stacks and the Trash on a glass strip at the screen edge. Right-click an icon for its windows and Options; drag icons to reorder, or away from the dock to remove them."), cfg, p, cx);
        let enabled = cfg.dock_enabled();
        let hidden_note = (enabled && cfg.dock.auto_hide != DockAutoHide::Never).then(|| {
            let why = if cfg.dock.auto_hide == DockAutoHide::Smart { t("The dock hides while a window covers it. Touch the screen edge to bring it back.") } else { t("The dock hides until you touch the screen edge.") };
            card(p)
                .child(
                    div()
                        .row()
                        .items_center()
                        .gap(px(SPACE_12_PX))
                        .child(icon("eye-off", ICON_16_PX, p.primary))
                        .child(div().flex_1().min_w(px(0.0)).text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(why))
                        .child(button("dock-show-now", "eye", t("Show it now"), false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| ipc(cx, &["dock", "show"]))))
                        .child(button("dock-never-hide", "check", t("Keep it visible"), true, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| update_config(cx, |c| c.dock.auto_hide = DockAutoHide::Never)))),
                )
                .into_any_element()
        });
        let group = |name: &'static str| registry::in_section(Section::Dock).filter(move |s| s.group == name);
        let layout = self.rows(group(DOCK_LAYOUT), cfg, p, cx);
        let behaviour = self.rows(group(DOCK_BEHAVIOUR), cfg, p, cx);
        let apps_rows = self.rows(group(DOCK_APPS), cfg, p, cx);
        let stack_rows = self.rows(group(DOCK_STACKS), cfg, p, cx);
        let applications = self.rows(group(DOCK_APPLICATIONS), cfg, p, cx);
        let look = self.rows(group(DOCK_LOOK), cfg, p, cx);
        let displays = self.displays_link(mandara_config::outputs::SurfaceKind::Dock, cfg, p, cx);
        let pinned = self.pinned_card(cfg, p, cx);
        let stacks = self.stacks_card(cfg, p, cx);
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_26_PX))
            .child(toggle)
            .children(hidden_note)
            .child(card(p).child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t("Preview"))).child(preview(&cfg.dock, p)))
            .child(displays)
            .child(layout)
            .child(behaviour)
            .child(div().row().flex_col().gap(px(SPACE_10_PX)).child(group_title(tr(DOCK_APPS), p)).child(pinned))
            .child(apps_rows)
            .child(applications)
            .child(stacks)
            .child(stack_rows)
            .child(look)
            .into_any_element()
    }

    /// Apps kept in the dock: icon, name, drag handle, remove; "Add app…".
    fn pinned_card(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let apps = mandara_ui::apps::discover();
        let mut list = div().row().flex_col().gap(px(SPACE_4_PX));
        for (i, id) in cfg.dock.pinned.iter().enumerate() {
            let entry = apps.iter().find(|a| a.id.eq_ignore_ascii_case(id));
            let name = entry.map(|a| a.name.clone()).unwrap_or_else(|| id.clone());
            let icon_id = entry.and_then(|a| a.wm_class.clone()).unwrap_or_else(|| id.clone());
            let drag = PinDrag { index: i, id: icon_id.clone(), name: name.clone() };
            let palette = p.clone();
            let remove_id = id.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("dock-pin-{id}")))
                    .h(px(PIN_ROW_H_PX))
                    .px(px(SPACE_12_PX))
                    .rounded(px(RADIUS_12_PX))
                    .row()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .bg(p.surface_container_high.with_alpha(ALPHA_50).hsla())
                    .cursor_grab()
                    .on_drag(drag, move |d: &PinDrag, _, _, cx| {
                        let (d, palette) = (d.clone(), palette.clone());
                        cx.new(|_| PinGhost(d, palette))
                    })
                    .drag_over::<PinDrag>({
                        let tint = p.primary.with_alpha(ALPHA_16).hsla();
                        move |s, _, _, _| s.bg(tint)
                    })
                    .on_drop(cx.listener(move |_, d: &PinDrag, _, cx| {
                        let from = d.index;
                        update_config(cx, move |c| {
                            c.dock.move_pin(from, i);
                        });
                    }))
                    .child(icon("tiles", HANDLE_ICON_PX, p.on_surface_variant))
                    .child(mandara_ui::plugins::workspaces::app_icon(&icon_id, PIN_ICON_PX, p.on_surface, false, p))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .row()
                            .flex_col()
                            .child(div().truncate().text_size(px(TEXT_13_PX)).child(name))
                            .child(div().truncate().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).child(if entry.is_some() { id.clone() } else { tf("{id} · not installed", &[("id", id)]) })),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("dock-unpin-{id}")))
                            .size(px(DIM_30_PX))
                            .rounded_full()
                            .row()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(p.urgent_coral.with_alpha(ALPHA_15).hsla()))
                            .child(icon("close", ICON_14_PX, p.on_surface_variant))
                            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                let id = remove_id.clone();
                                update_config(cx, move |c| {
                                    c.dock.unpin(&id);
                                });
                            })),
                    ),
            );
        }
        if cfg.dock.pinned.is_empty() {
            list = list.child(div().p(px(SPACE_10_PX)).text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Nothing kept in the dock yet: add apps, or right-click a running app's icon → Options → Keep in Dock."))));
        }
        card(p)
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .child(div().flex_1().min_w(px(0.0)).child(heading(t("Kept in the Dock"), t("Drag to reorder. Running apps you keep from the dock's right-click menu show up here too."), p)))
                    .child(button("dock-add-app", "plus", t("Add app…"), true, p).on_click(cx.listener(|_, _: &ClickEvent, window, cx| open_picker(window, cx)))),
            )
            .child(list)
            .into_any_element()
    }

    /// Stacks: each folder with its layout, order and icon; add a folder.
    fn stacks_card(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let mut list = div().row().flex_col().gap(px(SPACE_10_PX));
        for (i, st) in cfg.dock.stacks.iter().enumerate() {
            let choices = |key: &'static str, opts: &'static [(&'static str, &'static str)], current: String, cx: &mut Context<Self>| {
                let mut row = div().row().flex_wrap().gap(px(SPACE_6_PX));
                for (value, label) in opts {
                    let v = value.to_string();
                    row = row.child(chip(SharedString::from(format!("stack-{i}-{key}-{value}")), tr(label), current == *value, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        let v = v.clone();
                        update_config(cx, move |c| {
                            if let Some(s) = c.dock.stacks.get_mut(i) {
                                let quoted = serde_json::Value::String(v.clone());
                                match key {
                                    "view" => s.view = serde_json::from_value::<StackView>(quoted).unwrap_or_default(),
                                    "sort" => s.sort = serde_json::from_value::<StackSort>(quoted).unwrap_or_default(),
                                    _ => s.display = serde_json::from_value::<StackDisplay>(quoted).unwrap_or_default(),
                                }
                            }
                        });
                    })));
                }
                row
            };
            let enum_str = |v: serde_json::Value| v.as_str().unwrap_or_default().to_string();
            let view = choices("view", STACK_VIEWS, enum_str(serde_json::to_value(st.view).unwrap_or_default()), cx);
            let sort = choices("sort", STACK_SORTS, enum_str(serde_json::to_value(st.sort).unwrap_or_default()), cx);
            let display = choices("display", STACK_DISPLAYS, enum_str(serde_json::to_value(st.display).unwrap_or_default()), cx);
            let label = |s: &'static str| div().w(px(DIM_90_PX)).flex_none().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tr(s)));
            list = list.child(
                div()
                    .p(px(SPACE_12_PX))
                    .rounded(px(RADIUS_14_PX))
                    .bg(p.surface_container_high.with_alpha(ALPHA_50).hsla())
                    .row()
                    .flex_col()
                    .gap(px(SPACE_8_PX))
                    .child(
                        div()
                            .row()
                            .items_center()
                            .gap(px(SPACE_10_PX))
                            .child(icon("folder", ICON_16_PX, p.primary))
                            .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).child(st.path.clone()))
                            .child(button(SharedString::from(format!("stack-remove-{i}")), "close", t("Remove"), false, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                update_config(cx, move |c| {
                                    if i < c.dock.stacks.len() {
                                        c.dock.stacks.remove(i);
                                    }
                                });
                            }))),
                    )
                    .child(div().row().items_center().gap(px(SPACE_10_PX)).child(label(mandara_i18n::tk("View as"))).child(view))
                    .child(div().row().items_center().gap(px(SPACE_10_PX)).child(label(mandara_i18n::tk("Sort by"))).child(sort))
                    .child(div().row().items_center().gap(px(SPACE_10_PX)).child(label(mandara_i18n::tk("Display as"))).child(display)),
            );
        }
        let add = self.add_field(
            "dock-stack-add",
            mandara_i18n::tk("Add a folder, e.g. ~/Documents — Enter"),
            Rc::new(|value, cx| {
                update_config(cx, move |c| {
                    c.dock.add_stack(&value);
                });
            }),
            cx,
        );
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(group_title(tr(DOCK_STACKS), p))
            .child(card(p).child(heading(t("Stacks"), t("Folders on the right side of the dock. Click one for its newest files; drag it along the dock to reorder or away to remove it."), p)).child(list).child(div().child(add)))
            .into_any_element()
    }

    /// The "Add app…" modal layer, while open.
    pub(super) fn dock_picker_layer(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let st = cx.try_global::<DockPicker>()?;
        if !st.open {
            return None;
        }
        let apps = st.apps.clone()?;
        let query = st.query.clone();
        let selected = st.selected.clone();
        let search = st.search.clone()?;
        let pinned = mandara_ui::state::config(cx).read(cx).get().dock.pinned.clone();
        let (pin, unpin) = selection_changes(&pinned, &selected, &apps);
        let changes = pin.len() + unpin.len();
        let results = rank_apps(&apps, &query);
        let mut grid = div().row().flex_wrap().gap(px(SPACE_8_PX));
        for &i in results.iter().take(PICK_MAX) {
            let a = &apps[i];
            let checked = selected.contains(&a.id);
            let id = a.id.clone();
            grid = grid.child(
                div()
                    .id(SharedString::from(format!("dock-pick-{}", a.id)))
                    .relative()
                    .w(px(PICK_TILE_W_PX))
                    .h(px(PICK_TILE_H_PX))
                    .rounded(px(RADIUS_14_PX))
                    .row()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(SPACE_6_PX))
                    .cursor_pointer()
                    .when(checked, |el| el.bg(p.primary.with_alpha(ALPHA_16).hsla()))
                    .hover(|s| s.bg(p.primary.with_alpha(ALPHA_10).hsla()))
                    .child(mandara_ui::plugins::workspaces::app_icon(a.wm_class.as_deref().unwrap_or(&a.id), PICK_ICON_PX, p.on_surface, false, p))
                    .child(div().w_full().px(px(SPACE_6_PX)).text_center().truncate().text_size(px(TEXT_11_5_PX)).child(mandara_ui::kit::ellipsize(&a.name, PICK_NAME_CHARS)))
                    .child(
                        div()
                            .absolute()
                            .top(px(SPACE_6_PX))
                            .right(px(SPACE_6_PX))
                            .size(px(PICK_CHECK_PX))
                            .rounded_full()
                            .row()
                            .items_center()
                            .justify_center()
                            .when(checked, |el| el.bg(p.primary.hsla()).child(icon("check", ICON_12_PX, p.on_primary)))
                            .when(!checked, |el| el.border_1().border_color(p.on_surface.with_alpha(ALPHA_25).hsla())),
                    )
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        let st = picker(cx);
                        if !st.selected.remove(&id) {
                            st.selected.insert(id.clone());
                        }
                        cx.notify();
                    })),
            );
        }
        let empty = results.is_empty();
        let apply_label = if changes == 0 { t("Done").to_string() } else { tf("Apply {n} changes", &[("n", &changes)]) };
        Some(
            div()
                .absolute()
                .inset_0()
                .row()
                .items_center()
                .justify_center()
                .child(div().id("dock-picker-scrim").absolute().inset_0().bg(gpui::black().opacity(SCRIM_ALPHA)).on_click(cx.listener(|_, _: &ClickEvent, _, cx| close_picker(false, cx))))
                .child(
                    div()
                        .id("dock-picker")
                        .occlude()
                        .w(px(PICKER_W_PX))
                        .h(px(PICKER_H_PX))
                        .rounded(px(RADIUS_22_PX))
                        .bg(p.surface_container.hsla())
                        .p(px(SPACE_18_PX))
                        .row()
                        .flex_col()
                        .gap(px(SPACE_12_PX))
                        .on_key_down(cx.listener(|_, e: &gpui::KeyDownEvent, _, cx| match e.keystroke.key.as_str() {
                            "escape" => close_picker(false, cx),
                            "enter" => close_picker(true, cx),
                            _ => {}
                        }))
                        .child(heading(t("Add apps to the Dock"), t("Search your installed apps; checked apps stay in the dock."), p))
                        .child(div().child(search))
                        .child({
                            let (scroll, bar) = mandara_ui::scrollbar::keyed("dock-picker-scroll");
                            let list = div()
                                .id("dock-picker-scroll")
                                .flex_1()
                                .min_h(px(0.0))
                                .overflow_y_scroll()
                                .track_scroll(&scroll)
                                .child(grid)
                                .when(empty, |el| el.child(div().p(px(SPACE_10_PX)).text_color(p.on_surface_variant.hsla()).child(tf("No apps match “{query}”", &[("query", &query)]))));
                            mandara_ui::scrollbar::with_scrollbar(list, &scroll, &bar, p).flex_1().min_h(px(0.0)).flex().flex_col()
                        })
                        .child(
                            div()
                                .row()
                                .items_center()
                                .justify_end()
                                .gap(px(SPACE_8_PX))
                                .child(button("dock-picker-cancel", "close", t("Cancel"), false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| close_picker(false, cx))))
                                .child(button("dock-picker-apply", "check", apply_label, true, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| close_picker(true, cx)))),
                        ),
                )
                .into_any_element(),
        )
    }
}
