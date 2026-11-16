//! The fullscreen library's usage order, folders and drag targets.

use std::collections::{HashMap, HashSet};

use gpui::{AnyElement, AppContext, ClickEvent, Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, px};
use mandara_config::workflow::LauncherStyle;
use mandara_core::ToGpui;
use mandara_i18n::t;
use mandara_theme::{Color, MandaraPalette};

use super::{Item, LauncherView, launcher_looks::BeaconMode};

const DROP_EDGE: f32 = 0.23;
const DRAG_HEIGHT: f32 = 32.0;
pub(super) const FOLDER_BACK_H: f32 = DRAG_HEIGHT;
const DRAG_PAD: f32 = 12.0;

type Layout = Vec<Vec<String>>;

#[derive(Clone)]
pub(super) enum DropAt {
    Beside(String, bool),
    Group(String),
    Out(String),
}

fn layout(configured: &[Vec<String>], ranked: impl IntoIterator<Item = String>) -> Layout {
    let mut seen = HashSet::new();
    let mut order: Layout = configured.iter().filter_map(|tile| {
        let tile: Vec<_> = tile.iter().filter(|id| seen.insert((*id).clone())).cloned().collect();
        (!tile.is_empty()).then_some(tile)
    }).collect();
    order.extend(ranked.into_iter().filter(|id| seen.insert(id.clone())).map(|id| vec![id]));
    order
}

fn locate(order: &Layout, id: &str) -> Option<(usize, usize)> {
    order.iter().enumerate().find_map(|(tile, apps)| apps.iter().position(|app| app == id).map(|app| (tile, app)))
}

fn move_app(order: &mut Layout, app: &str, inside: bool, at: DropAt) -> bool {
    let target = match &at { DropAt::Beside(id, _) | DropAt::Group(id) | DropAt::Out(id) => id };
    let (Some((from, member)), Some((to, _))) = (locate(order, app), locate(order, target)) else { return false };
    if from == to {
        return match at {
            DropAt::Beside(target, after) if inside && app != target => {
                let moved = order[from].remove(member);
                let (_, position) = locate(order, &target).expect("another member of the same folder");
                order[from].insert(position + usize::from(after), moved);
                true
            }
            DropAt::Out(_) if inside && order[from].len() > 1 => {
                let moved = order[from].remove(member);
                order.insert(from + 1, vec![moved]);
                true
            }
            _ => false,
        };
    }
    let moved = if inside && order[from].len() > 1 {
        vec![order[from].remove(member)]
    } else {
        order.remove(from)
    };
    let to = locate(order, target).expect("different source and destination tiles").0;
    match at {
        DropAt::Beside(_, after) => order.insert(to + usize::from(after), moved),
        DropAt::Group(_) => order[to].extend(moved),
        DropAt::Out(_) => order.insert(to + 1, moved),
    }
    true
}

#[derive(Clone)]
pub(super) struct DragApp {
    pub(super) id: String,
    title: SharedString,
    bg: Color,
    fg: Color,
}

impl Render for DragApp {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().h(px(DRAG_HEIGHT)).px(px(DRAG_PAD)).rounded_full().flex().items_center()
            .bg(self.bg.hsla()).text_color(self.fg.hsla()).child(self.title.clone())
    }
}

impl LauncherView {
    pub(super) fn organizes_fullscreen(&self) -> bool {
        self.look.style == LauncherStyle::Fullscreen && self.query.trim().is_empty()
            && matches!(self.mode, None | Some(BeaconMode::Apps))
    }

    fn ranked_fullscreen_ids(&self) -> Vec<String> {
        let mut apps: Vec<_> = self.ctx.catalog.apps.iter().collect();
        apps.sort_by_cached_key(|app| std::cmp::Reverse(self.ctx.usage.frecency(&format!("app:{}", app.id))));
        apps.into_iter().map(|app| app.id.clone()).collect()
    }

    pub(super) fn arrange_fullscreen(&mut self, results: &mut Vec<Item>) {
        self.fullscreen_groups.clear();
        if !self.organizes_fullscreen() {
            self.fullscreen_folder = None;
            return;
        }
        let order = layout(&self.look.fullscreen_order, self.ranked_fullscreen_ids());
        let mut apps: HashMap<_, _> = std::mem::take(results).into_iter().filter_map(|item| match item {
            Item::App(app) => Some((app.id.clone(), app)),
            _ => None,
        }).collect();
        if let Some(folder) = self.fullscreen_folder.as_deref() {
            if let Some((index, _)) = locate(&order, folder) {
                let members: Vec<_> = order[index].iter().filter_map(|id| apps.get(id).cloned()).collect();
                if members.len() > 1 {
                    results.extend(members.into_iter().map(Item::App));
                    return;
                }
            }
            self.fullscreen_folder = None;
        }
        for tile in order {
            let members: Vec<_> = tile.iter().filter_map(|id| apps.remove(id)).collect();
            if let Some(app) = members.first() {
                results.push(Item::App(app.clone()));
                if members.len() > 1 {
                    self.fullscreen_groups.insert(results.len() - 1, members);
                }
            }
        }
    }

    pub(super) fn leave_fullscreen_folder(&mut self, cx: &mut Context<Self>) {
        let folder = self.fullscreen_folder.take();
        self.refilter(cx);
        self.select_fullscreen_app(folder.as_deref());
        cx.notify();
    }

    fn select_fullscreen_app(&mut self, id: Option<&str>) {
        if let Some(index) = self.results.iter().enumerate().position(|(index, item)| {
            matches!(item, Item::App(app) if Some(app.id.as_str()) == id)
                || self.fullscreen_groups.get(&index).is_some_and(|apps| apps.iter().any(|app| Some(app.id.as_str()) == id))
        }) {
            self.selected = index;
        }
    }

    pub(super) fn place_fullscreen_app(&mut self, drag: &DragApp, at: DropAt, cx: &mut Context<Self>) {
        if !self.organizes_fullscreen() {
            return;
        }
        let mut order = layout(&self.look.fullscreen_order, self.ranked_fullscreen_ids());
        let inside = self.fullscreen_folder.is_some();
        let out = matches!(at, DropAt::Out(_));
        if !move_app(&mut order, &drag.id, inside, at) {
            return;
        }
        if out {
            self.fullscreen_folder = None;
        }
        self.look.fullscreen_order.clone_from(&order);
        crate::state::update_config(cx, move |cfg| cfg.launcher.look.fullscreen_order = order);
        self.refilter(cx);
        self.select_fullscreen_app(Some(&drag.id));
        self.wheel_hold = true;
        cx.notify();
    }

    pub(super) fn drop_on_fullscreen_page(&mut self, drag: &DragApp, page: usize, size: usize, cx: &mut Context<Self>) {
        let source = self.results.iter().position(|item| matches!(item, Item::App(app) if app.id == drag.id));
        if let (Some(source), Some(Item::App(target))) = (source, self.results.get(page * size)) {
            let at = DropAt::Beside(target.id.clone(), source < page * size);
            self.place_fullscreen_app(drag, at, cx);
        }
    }

    pub(super) fn fullscreen_drop_target(&self, target: gpui::Div, at: DropAt, palette: &MandaraPalette, cx: &mut Context<Self>) -> gpui::Div {
        let highlight = palette.primary.with_alpha(0.28).hsla();
        target.drag_over::<DragApp>(move |style, _, _, _| style.bg(highlight))
            .on_drop(cx.listener(move |this, drag: &DragApp, _, cx| {
                this.place_fullscreen_app(drag, at.clone(), cx);
            }))
    }

    pub(super) fn draggable_fullscreen_tile(&self, tile: gpui::Stateful<gpui::Div>, id: String, title: String, width: f32, palette: &MandaraPalette, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let inside = self.fullscreen_folder.is_some();
        let edge = width * DROP_EDGE;
        let rtl = crate::bidi::rtl();
        let band = || div().absolute().top_0().bottom_0();
        let drop_left = self.fullscreen_drop_target(band().left_0().w(px(edge)), DropAt::Beside(id.clone(), rtl), palette, cx);
        let drop_right = self.fullscreen_drop_target(band().right_0().w(px(edge)), DropAt::Beside(id.clone(), !rtl), palette, cx);
        let center = if inside { DropAt::Beside(id.clone(), false) } else { DropAt::Group(id.clone()) };
        let drop_center = self.fullscreen_drop_target(band().left(px(edge)).right(px(edge)).rounded_full(), center, palette, cx);
        let drag = DragApp { id, title: title.into(), bg: palette.primary, fg: palette.on_primary };
        tile.on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
            .child(drop_left).child(drop_center).child(drop_right)
    }

    pub(super) fn fullscreen_folder_back(&self, tab: usize, palette: &MandaraPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.fullscreen_folder.as_ref()?.clone();
        let button = self.fullscreen_drop_target(div(), DropAt::Out(id), palette, cx);
        Some(button.id("fullscreen-folder-back").h(px(FOLDER_BACK_H)).px(px(DRAG_PAD)).rounded_full().self_start()
            .bg(palette.surface_container_high.hsla())
            .role(gpui::Role::Button).focusable().tab_index(tab as isize).cursor_pointer()
            .flex().items_center().child(t("Back"))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.leave_fullscreen_folder(cx)))
            .into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullscreen_folders_reorder_and_dissolve_without_losing_apps() {
        let mut order = layout(&[], ["a", "b", "c", "d"].map(String::from));
        assert!(move_app(&mut order, "a", false, DropAt::Group("b".into())));
        assert_eq!(order, [["b", "a"].as_slice(), &["c"], &["d"]]);
        assert!(move_app(&mut order, "b", false, DropAt::Beside("d".into(), true)));
        assert!(move_app(&mut order, "a", true, DropAt::Beside("b".into(), false)));
        assert!(move_app(&mut order, "a", true, DropAt::Out("b".into())));
        assert_eq!(order, [["c"].as_slice(), &["d"], &["b"], &["a"]]);
        assert!(!move_app(&mut order, "a", false, DropAt::Group("missing".into())));
    }

    #[test]
    fn fullscreen_layout_keeps_saved_apps_and_appends_new_ones() {
        let saved = vec![vec!["missing".into(), "b".into()], vec!["b".into()]];
        assert_eq!(layout(&saved, ["a", "b"].map(String::from)), [["missing", "b"].as_slice(), &["a"]]);
    }
}
