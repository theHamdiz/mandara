//! The Applications folder: macOS's Applications stack on the right side
//! of the dock, opening a Launchpad-style grid of every installed app.
//!
//! * The apps are the launcher's list (hidden apps, renamed apps and
//!   custom icons applied), searched with the dock picker's fuzzy ranking
//!   ([`super::picker::rank_apps`]); the search field has focus on open.
//! * Order: A–Z, most used (the launcher's frecency) or by category (the
//!   `.desktop` `Categories=`, under captions); category chips filter.
//! * Click launches (and closes); arrows, Enter, Esc; right-click: Open,
//!   Keep in Dock ✓, Open at Login ✓, Show in Files, Uninstall… (Studio's
//!   uninstaller, with its confirmation).
//! * It is the install drop target: an installable file dragged over the
//!   folder's dock icon or over the open grid glows ("Drop to install")
//!   and dropping hands it to the app installer; apps installed this
//!   session (or in the last days) wear a "New" badge.
//!
//! The grid is virtualized (`uniform_list`): only visible rows lay out and
//! decode icons (at the size they are drawn, `crate::icon_raster`), and the
//! icons it decoded are evicted when it closes (except the ones the dock
//! itself shows), so opening it leaves nothing behind.

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui::layer_shell::KeyboardInteractivity;
use gpui::{
    AnyElement, App, AppContext as _, Context, DragMoveEvent, Entity, ExternalPaths, FocusHandle, FontWeight, Global, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Render, ScrollStrategy, SharedString, StatefulInteractiveElement, Styled,
    UniformListScrollHandle, Window, WindowHandle, div, linear_color_stop, linear_gradient, prelude::FluentBuilder as _, px, uniform_list,
};
use mandara_config::dock::{AppsOrder, AppsView};
use mandara_config::workflow::DockPosition;
use mandara_core::{SpringConfig, ToGpui};
use mandara_i18n::{t, tf};
use mandara_theme::{Color, MandaraPalette};

use super::model::{Item, Kind};
use super::sheet::{self, MenuRow, Sheet};
use crate::apps::App as DesktopApp;
use crate::icons::icon;
use crate::kit::text;
use crate::plugins::panel_fx::{WASH_SOFT, glow, readable, rim, wash};
use crate::state::{config, update_config};
use crate::text_field::TextField;

/// Panel geometry (logical px).
const PAD: f32 = 26.0;
const RADIUS: f32 = 28.0;
const GAP: f32 = 12.0;
const HEADER_H: f32 = 40.0;
const SEARCH_H: f32 = 40.0;
const SEARCH_W: f32 = 340.0;
const CHIPS_H: f32 = 30.0;
const CHIP_PAD: f32 = 12.0;
const CHIP_GAP: f32 = 6.0;
/// The panel at most this wide / tall, and this far from the screen edges.
const MAX_W: f32 = 1180.0;
const MAX_H: f32 = 820.0;
const EDGE: f32 = 24.0;
/// Grid tiles: room around the icon, name lines, caption above a category.
const TILE_PAD_X: f32 = 22.0;
const TILE_PAD_TOP: f32 = 12.0;
const NAME_GAP: f32 = 9.0;
const NAME_LINE_H: f32 = 15.0;
const NAME_LINES: usize = 2;
const TILE_PAD_BOTTOM: f32 = 10.0;
const TILE_RADIUS: f32 = 18.0;
const CAPTION_H: f32 = 34.0;
const MIN_COLUMNS: usize = 4;
const MAX_COLUMNS: usize = 8;
/// List rows.
const LIST_W: f32 = 620.0;
const LIST_ROW_H: f32 = 54.0;
const LIST_ICON: f32 = 36.0;
const LIST_ROWS: usize = 11;
/// "New" badge.
const BADGE_H: f32 = 18.0;
const BADGE_PAD: f32 = 7.0;
const BADGE_TEXT: f32 = 10.0;
/// Apps installed this recently count as new.
const NEW_FOR: Duration = Duration::from_secs(3 * 24 * 3600);
/// Launcher usage rows read for "Most used".
const USAGE_ROWS: usize = 400;
const HOVER_ALPHA: f32 = 0.07;
const SELECTED_ALPHA: f32 = 0.16;
const DROP_GLOW: (f32, f32) = (0.6, 26.0);
const DROP_SCRIM_ALPHA: f32 = 0.10;
const OPEN_SPRING: SpringConfig = SpringConfig::new(380.0, 30.0, 1.0);
/// The panel's content fades in from this openness.
const CONTENT_FROM: f32 = 0.3;
/// Folder tile drawing, fractions of the icon size: the back sheet with
/// its tab, the front sheet, the glyph.
const FOLDER_TAB: (f32, f32, f32, f32) = (0.10, 0.12, 0.36, 0.16);
const FOLDER_BACK: (f32, f32, f32, f32) = (0.06, 0.20, 0.88, 0.66);
const FOLDER_FRONT: (f32, f32, f32, f32) = (0.06, 0.30, 0.88, 0.58);
const FOLDER_RADIUS: f32 = 0.12;
const FOLDER_GLYPH: f32 = 0.34;
const FOLDER_BACK_MIX: f32 = 0.38;
const FOLDER_SHEEN: f32 = 0.10;

// ── Categories ───────────────────────────────────────────────────────────

/// Display groups of the freedesktop main categories, in order.
const GROUPS: &[(&str, &[&str])] = &[
    (mandara_i18n::tk("Internet"), &["Network", "WebBrowser", "Email", "Chat", "InstantMessaging"]),
    (mandara_i18n::tk("Office"), &["Office", "WordProcessor", "Spreadsheet", "Presentation"]),
    (mandara_i18n::tk("Development"), &["Development", "IDE", "TextEditor"]),
    (mandara_i18n::tk("Graphics"), &["Graphics", "Photography"]),
    (mandara_i18n::tk("Media"), &["AudioVideo", "Audio", "Video", "Music"]),
    (mandara_i18n::tk("Games"), &["Game"]),
    (mandara_i18n::tk("Education"), &["Education", "Science"]),
    (mandara_i18n::tk("Utilities"), &["Utility", "Accessories", "FileManager", "TerminalEmulator"]),
    (mandara_i18n::tk("System"), &["System", "Settings", "Monitor", "PackageManager"]),
];
const OTHER: &str = mandara_i18n::tk("Other");

/// The display group of `.desktop` `categories` (English key).
pub fn group_of(categories: &str) -> &'static str {
    let cats: Vec<&str> = categories.split(';').map(str::trim).filter(|c| !c.is_empty()).collect();
    GROUPS.iter().find(|(_, keys)| cats.iter().any(|c| keys.iter().any(|k| k.eq_ignore_ascii_case(c)))).map(|(name, _)| *name).unwrap_or(OTHER)
}

/// The apps to show, in order: matching `query` (ranked best first while
/// searching), in `group` (`None` = all), sorted by `order` otherwise.
/// `usage` maps desktop ids to frecency.
pub fn arrange(apps: &[DesktopApp], query: &str, group: Option<&str>, order: AppsOrder, usage: &HashMap<String, f64>) -> Vec<usize> {
    let mut out: Vec<usize> = super::picker::rank_apps(apps, query).into_iter().filter(|&i| group.is_none_or(|g| group_of(&apps[i].categories) == g)).collect();
    if !query.trim().is_empty() {
        return out;
    }
    let name = |i: usize| apps[i].name.to_lowercase();
    match order {
        AppsOrder::Name => out.sort_by_key(|&i| name(i)),
        AppsOrder::MostUsed => out.sort_by(|&a, &b| {
            let (ua, ub) = (usage.get(&apps[a].id).copied().unwrap_or(0.0), usage.get(&apps[b].id).copied().unwrap_or(0.0));
            ub.total_cmp(&ua).then_with(|| name(a).cmp(&name(b)))
        }),
        AppsOrder::Categories => {
            let rank = |i: usize| GROUPS.iter().position(|(g, _)| *g == group_of(&apps[i].categories)).unwrap_or(GROUPS.len());
            out.sort_by(|&a, &b| rank(a).cmp(&rank(b)).then_with(|| name(a).cmp(&name(b))));
        }
    }
    out
}

/// One row of the virtualized grid: a caption (first row of a category)
/// and its apps.
#[derive(Clone, Debug, PartialEq)]
pub struct GridRow {
    pub caption: Option<&'static str>,
    pub apps: Vec<usize>,
}

/// `order`ed app indices in rows of `cols`; with `sections`, each
/// category starts a row under its caption.
pub fn rows(order: &[usize], cols: usize, sections: Option<&dyn Fn(usize) -> &'static str>) -> Vec<GridRow> {
    let cols = cols.max(1);
    let mut out: Vec<GridRow> = Vec::new();
    let mut current: Option<&'static str> = None;
    for &i in order {
        let group = sections.map(|f| f(i));
        let new_section = group.is_some() && group != current;
        if new_section || out.last().is_none_or(|r| r.apps.len() >= cols) {
            out.push(GridRow { caption: if new_section { group } else { None }, apps: Vec::new() });
            current = group.or(current);
        }
        if let Some(r) = out.last_mut() {
            r.apps.push(i);
        }
    }
    out
}

// ── View ─────────────────────────────────────────────────────────────────

pub struct AppsGrid {
    anchor: (f32, f32, f32, f32),
    position: DockPosition,
    apps: Rc<[DesktopApp]>,
    usage: HashMap<String, f64>,
    new_ids: Vec<String>,
    query: String,
    group: Option<&'static str>,
    order: Vec<usize>,
    rows: Vec<GridRow>,
    cols: usize,
    search: Entity<TextField>,
    focus: FocusHandle,
    sheet: Sheet,
    scroll: UniformListScrollHandle,
    selected: Option<usize>,
    menu: Option<(usize, (f32, f32), bool)>,
    install_hover: bool,
    /// App ids whose icons were laid out (evicted on close).
    seen: Vec<String>,
    released: bool,
    /// The window's scale when last drawn (the icons' decode size).
    scale: f32,
}

impl AppsGrid {
    fn new(anchor: (f32, f32, f32, f32), window: &mut Window, cx: &mut Context<Self>) -> Self {
        let dock = config(cx).read(cx).get().dock_sanitized();
        let apps: Rc<[DesktopApp]> = super::picker::installed_apps(cx).into();
        let search = Self::search_field(window, cx);
        // New installs show up while the grid is open (they pin the dock).
        cx.observe(&config(cx), |this: &mut Self, _, cx| {
            let fresh: Rc<[DesktopApp]> = super::picker::installed_apps(cx).into();
            if fresh.len() != this.apps.len() {
                this.apps = fresh;
                this.new_ids = new_apps(&this.apps, cx);
                this.rebuild(cx);
            }
        })
        .detach();
        let mut this = Self {
            anchor,
            position: dock.position,
            usage: usage_map(cx),
            new_ids: new_apps(&apps, cx),
            apps,
            query: String::new(),
            group: None,
            order: Vec::new(),
            rows: Vec::new(),
            cols: MIN_COLUMNS,
            search,
            focus: cx.focus_handle(),
            sheet: Sheet::new(OPEN_SPRING),
            scroll: UniformListScrollHandle::new(),
            selected: None,
            menu: None,
            install_hover: false,
            seen: Vec::new(),
            released: false,
            scale: window.scale_factor(),
        };
        // A grid removed without its last frame (the close deadline) still
        // frees the icons it decoded.
        cx.on_release(|this: &mut Self, cx| this.forget_icons(cx)).detach();
        this.rebuild(cx);
        this
    }

    fn search_field(window: &mut Window, cx: &mut Context<Self>) -> Entity<TextField> {
        let this = cx.entity().downgrade();
        let field = cx.new(|cx| {
            TextField::new(
                String::new(),
                t("Search apps"),
                true,
                Rc::new(move |value, _, cx| {
                    let _ = this.update(cx, |g, cx| {
                        g.query = value;
                        g.selected = None;
                        g.rebuild(cx);
                    });
                }),
                cx,
            )
        });
        field.update(cx, |f, cx| f.focus(window, cx));
        field
    }

    fn settings(cx: &App) -> mandara_config::dock::DockApplications {
        config(cx).read(cx).get().dock_sanitized().applications
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let s = Self::settings(cx);
        self.order = arrange(&self.apps, &self.query, self.group, s.order, &self.usage);
        let apps = self.apps.clone();
        let sections = |i: usize| group_of(&apps[i].categories);
        let by_category = s.order == AppsOrder::Categories && self.query.trim().is_empty() && self.group.is_none();
        let cols = if s.view == AppsView::List { 1 } else { self.cols };
        self.rows = rows(&self.order, cols, if by_category { Some(&sections) } else { None });
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.sheet.close();
        cx.notify();
    }

    fn launch(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(item) = self.item(i, cx) {
            super::launch_item(&item, None, cx);
        }
        self.close(cx);
    }

    fn item(&self, i: usize, cx: &App) -> Option<Item> {
        let a = self.apps.get(i)?.clone();
        let pinned = config(cx).read(cx).get().dock.is_pinned(&a.id);
        Some(Item { key: a.id.clone(), icon_id: a.wm_class.clone().unwrap_or_else(|| a.id.clone()), name: a.name.clone(), desktop: Some(a), windows: Vec::new(), pinned, recent: false, kind: Kind::App })
    }

    fn menu_rows(&self, i: usize, cx: &App) -> Vec<MenuRow> {
        let id = self.apps.get(i).map(|a| a.id.clone()).unwrap_or_default();
        let pinned = config(cx).read(cx).get().dock.is_pinned(&id);
        let at_login = self.menu.map(|m| m.2).unwrap_or(false);
        vec![
            MenuRow::new(t("Open"), "play"),
            MenuRow::new(t("Keep in Dock"), "pin").check(pinned).sep(),
            MenuRow::new(t("Open at Login"), "zap").check(at_login),
            MenuRow::new(t("Show in Files"), "folder"),
            MenuRow::new(t("Uninstall…"), "trash").sep().danger(),
        ]
    }

    fn run_menu(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some((i, _, at_login)) = self.menu.take() else { return };
        let Some(app) = self.apps.get(i).cloned() else { return };
        match row {
            0 => return self.launch(i, cx),
            1 => {
                let id = app.id.clone();
                update_config(cx, move |c| {
                    c.dock.toggle_pin(&id);
                });
            }
            2 => {
                let id = app.id.clone();
                std::thread::spawn(move || {
                    if let Err(err) = super::menu::set_open_at_login(&id, !at_login) {
                        tracing::warn!("dock: open at login: {err}");
                    }
                });
            }
            3 => {
                if let Some(dir) = super::menu::show_in_files_target(&app.exec, super::menu::desktop_file(&app.id)) {
                    super::stacks::open_path(&dir);
                }
                return self.close(cx);
            }
            _ => {
                crate::state::open_studio_at(Some("applications/installed".into()), cx);
                return self.close(cx);
            }
        }
        cx.notify();
    }

    fn on_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = e.keystroke.key.as_str();
        if key == "escape" {
            if self.menu.take().is_some() {
            } else if !self.query.is_empty() || self.group.is_some() {
                self.query.clear();
                self.group = None;
                self.rebuild(cx);
                cx.stop_propagation();
                return;
            } else {
                return self.close(cx);
            }
            cx.notify();
            return;
        }
        let n = self.order.len();
        if n == 0 {
            return;
        }
        let pos = self.selected.and_then(|s| self.order.iter().position(|&i| i == s));
        let cols = self.rows.first().map(|r| r.apps.len()).unwrap_or(1).max(1) as isize;
        let browse = self.query.is_empty() || pos.is_some();
        let d: isize = match key {
            "down" => cols,
            "up" => -cols,
            "right" if browse => 1,
            "left" if browse => -1,
            "enter" => {
                let i = self.selected.unwrap_or(self.order[0]);
                return self.launch(i, cx);
            }
            _ => return,
        };
        cx.stop_propagation();
        let next = match pos {
            None => 0,
            Some(p) => (p as isize + d).clamp(0, n as isize - 1) as usize,
        };
        self.selected = Some(self.order[next]);
        if let Some(r) = self.rows.iter().position(|r| r.apps.contains(&self.order[next])) {
            self.scroll.scroll_to_item(r, ScrollStrategy::Center);
        }
        cx.notify();
    }

    fn render_rows(&mut self, range: Range<usize>, icon_px: f32, tile_w: f32, list: bool, palette: &MandaraPalette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let accent = readable(palette.primary, palette);
        let hover = palette.on_surface.with_alpha(HOVER_ALPHA).hsla();
        let on_accent = accent.best_on(Color::hex("#ffffff"), Color::hex("#101010"));
        let mut out = Vec::with_capacity(range.len());
        for r in range {
            let Some(row) = self.rows.get(r).cloned() else { continue };
            let mut el = div().flex().flex_col().w_full();
            if let Some(c) = row.caption {
                el = el.child(div().h(px(CAPTION_H)).flex().items_end().pb(px(CHIP_GAP)).px(px(CHIP_GAP)).text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(t(c)));
            } else if self.rows.iter().any(|r| r.caption.is_some()) {
                el = el.child(div().h(px(CAPTION_H)));
            }
            let mut line = div().flex().flex_row();
            for &i in &row.apps {
                let a = &self.apps[i];
                let icon_id = a.wm_class.clone().unwrap_or_else(|| a.id.clone());
                if !self.seen.contains(&icon_id) {
                    self.seen.push(icon_id.clone());
                }
                let selected = self.selected == Some(i);
                let is_new = self.new_ids.iter().any(|n| n.eq_ignore_ascii_case(&a.id));
                let badge = is_new.then(|| div().h(px(BADGE_H)).px(px(BADGE_PAD)).rounded_full().flex().items_center().bg(accent.hsla()).text_color(on_accent.hsla()).text_size(px(BADGE_TEXT)).font_weight(FontWeight::BOLD).child(t("New")));
                let app_icon = crate::plugins::workspaces::app_icon(&icon_id, icon_px, palette.on_surface, false, palette);
                let mut tile = div()
                    .id(SharedString::from(format!("apps-{}", a.id)))
                    .relative()
                    .rounded(px(crate::style::radius(TILE_RADIUS)))
                    .cursor_pointer()
                    .when(selected, |el| el.bg(accent.with_alpha(SELECTED_ALPHA).hsla()))
                    .hover(|s| s.bg(hover));
                if list {
                    tile = tile
                        .w_full()
                        .h(px(LIST_ROW_H))
                        .px(px(GAP))
                        .flex()
                        .items_center()
                        .gap(px(GAP))
                        .child(crate::plugins::workspaces::app_icon(&icon_id, LIST_ICON, palette.on_surface, false, palette))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .child(div().truncate().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child(a.name.clone()))
                                .when(!a.comment.is_empty(), |el| el.child(div().truncate().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(a.comment.clone()))),
                        )
                        .children(badge);
                } else {
                    tile = tile
                        .w(px(tile_w))
                        .flex()
                        .flex_col()
                        .items_center()
                        .pt(px(TILE_PAD_TOP))
                        .pb(px(TILE_PAD_BOTTOM))
                        .gap(px(NAME_GAP))
                        .child(app_icon)
                        .child(div().w_full().px(px(CHIP_GAP)).text_center().line_clamp(NAME_LINES).text_ellipsis().text_size(px(text::BODY_S)).line_height(px(NAME_LINE_H)).when(selected, |el| el.text_color(accent.hsla())).child(a.name.clone()))
                        .when_some(badge, |el, b| el.child(div().absolute().top(px(CHIP_GAP)).right(px(CHIP_GAP)).child(b)));
                }
                tile = tile.on_click(cx.listener(move |this, _, _, cx| this.launch(i, cx))).on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.selected = Some(i);
                    let id = this.apps.get(i).map(|a| a.id.clone()).unwrap_or_default();
                    this.menu = Some((i, (f32::from(e.position.x), f32::from(e.position.y)), super::menu::open_at_login(&id)));
                    cx.notify();
                }));
                line = line.child(tile);
            }
            out.push(el.child(line).into_any_element());
        }
        out
    }

    /// The raster icon files of the apps the grid laid out (minus the
    /// ones the dock itself shows) and the grid's two icon sizes.
    fn icon_paths(&mut self, cx: &mut App) -> (Vec<PathBuf>, [f32; 2]) {
        let keep = super::dock_icon_ids(cx);
        let grid_px = config(cx).read(cx).get().dock_sanitized().applications.icon_size;
        let paths = std::mem::take(&mut self.seen)
            .into_iter()
            .filter(|id| !keep.iter().any(|k| k.eq_ignore_ascii_case(id)))
            .filter_map(|id| crate::apps::icon_for_app_id(&id, false).filter(|r| !r.symbolic).map(|r| r.path))
            .collect();
        (paths, [grid_px, LIST_ICON])
    }

    /// Drops the decoded icons when the grid goes away without a window.
    fn forget_icons(&mut self, cx: &mut App) {
        let (paths, sizes) = self.icon_paths(cx);
        for path in &paths {
            for size in sizes {
                crate::icon_raster::forget_cached(path, size, self.scale, cx);
            }
        }
    }

    /// Frees what the grid decoded: icons nobody else shows, its atlas.
    fn release(&mut self, window: &mut Window, cx: &mut App) {
        if std::mem::replace(&mut self.released, true) {
            return;
        }
        // Decoded at the grid's (and list's) size: crate::icon_raster.
        let (paths, sizes) = self.icon_paths(cx);
        for path in &paths {
            for size in sizes {
                crate::icon_raster::forget(path, size, window, cx);
            }
        }
    }
}

impl Render for AppsGrid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = mandara_core::frame_palette(window, cx);
        let f = crate::fonts::fonts(cx);
        let vp = window.viewport_size();
        let screen = (f32::from(vp.width), f32::from(vp.height));
        if !cx.has_active_drag() {
            self.install_hover = false;
        }
        self.scale = window.scale_factor();
        let p = self.sheet.step(false, window, cx);
        if self.sheet.closing() && p <= 0.0 {
            self.release(window, cx);
        }
        let dock = config(cx).read(cx).get().dock_sanitized();
        let s = dock.applications.clone();
        let list = s.view == AppsView::List;
        let icon_px = s.icon_size;
        let tile_w = icon_px + TILE_PAD_X * 2.0;
        let row_h = if list { LIST_ROW_H } else { TILE_PAD_TOP + icon_px + NAME_GAP + NAME_LINE_H * NAME_LINES as f32 + TILE_PAD_BOTTOM } + if self.rows.iter().any(|r| r.caption.is_some()) { CAPTION_H } else { 0.0 };
        // Size: as many columns as fit, rows up to the screen.
        let room_w = (screen.0 - EDGE * 2.0).min(MAX_W) - PAD * 2.0;
        let cols = if list { 1 } else { ((room_w / tile_w).floor() as usize).clamp(MIN_COLUMNS, MAX_COLUMNS) };
        if !list && cols != self.cols {
            self.cols = cols;
            self.rebuild(cx);
        }
        let w = if list { LIST_W } else { cols as f32 * tile_w + PAD * 2.0 };
        let chrome = PAD * 2.0 + HEADER_H + GAP + CHIPS_H + GAP;
        let band = dock.band() + super::sheet_gap();
        let room_h = (screen.1 - band - EDGE * 2.0).min(MAX_H) - chrome;
        let visible_rows = if list { LIST_ROWS.min(self.rows.len().max(1)) } else { ((room_h / row_h).floor() as usize).max(1).min(self.rows.len().max(1)) };
        let body_h = (visible_rows as f32 * row_h).min(room_h.max(row_h));
        let h = chrome + body_h;
        let (x, y) = sheet::place_card(self.position, self.anchor, (w, h), screen, super::sheet_gap());
        let e = p.clamp(0.0, 1.0);
        let card = sheet::grow_from(self.anchor, (x, y, w, h), e);
        let content_alpha = ((p - CONTENT_FROM) / (1.0 - CONTENT_FROM)).clamp(0.0, 1.0);
        let accent = readable(palette.primary, &palette);
        let fill = sheet::glass(&palette, &dock);

        // Category chips (the groups present).
        let mut present: Vec<&'static str> = GROUPS.iter().map(|(g, _)| *g).filter(|g| self.apps.iter().any(|a| group_of(&a.categories) == *g)).collect();
        if self.apps.iter().any(|a| group_of(&a.categories) == OTHER) {
            present.push(OTHER);
        }
        let chip = |id: SharedString, label: SharedString, on: bool| {
            div()
                .id(id)
                .h(px(CHIPS_H))
                .px(px(CHIP_PAD))
                .rounded_full()
                .flex()
                .items_center()
                .flex_none()
                .cursor_pointer()
                .text_size(px(text::BODY_S))
                .when(on, |el| el.bg(accent.hsla()).text_color(accent.best_on(Color::hex("#ffffff"), Color::hex("#101010")).hsla()).font_weight(FontWeight::SEMIBOLD))
                .when(!on, |el| el.bg(wash(WASH_SOFT, &palette).hsla()).hover(|st| st.bg(palette.on_surface.with_alpha(HOVER_ALPHA).hsla())))
                .child(label)
        };
        let mut chips = div().id("apps-chips").h(px(CHIPS_H)).flex().flex_row().gap(px(CHIP_GAP)).overflow_x_scroll();
        chips = chips.child(chip("apps-chip-all".into(), t("All").into(), self.group.is_none()).on_click(cx.listener(|this, _, _, cx| {
            this.group = None;
            this.rebuild(cx);
        })));
        for g in present {
            chips = chips.child(chip(SharedString::from(format!("apps-chip-{g}")), t(g).into(), self.group == Some(g)).on_click(cx.listener(move |this, _, _, cx| {
                this.group = if this.group == Some(g) { None } else { Some(g) };
                this.selected = None;
                this.rebuild(cx);
            })));
        }

        let header = div()
            .h(px(HEADER_H))
            .flex()
            .items_center()
            .gap(px(GAP))
            .child(folder_tile(HEADER_H * FOLDER_HEADER_FRAC, &palette))
            .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(text::TITLE)).child(t("Applications")))
            .child(div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).font_features(crate::fonts::tabular_nums()).child(tf("{n} apps", &[("n", &self.order.len())])))
            .child(div().flex_1())
            .child(div().w(px(SEARCH_W)).h(px(SEARCH_H)).child(self.search.clone()));
        let count = self.rows.len();
        let grid = uniform_list(
            "apps-grid",
            count,
            cx.processor(move |this, range: Range<usize>, window, cx| {
                let palette = mandara_core::frame_palette(window, cx);
                this.render_rows(range, icon_px, tile_w, list, &palette, cx)
            }),
        )
        .track_scroll(&self.scroll)
        .h(px(body_h))
        .w_full();
        let empty = self.order.is_empty();
        let content = div()
            .absolute()
            .left(px(x - card.0))
            .top(px(y - card.1))
            .w(px(w))
            .h(px(h))
            .p(px(PAD))
            .opacity(content_alpha)
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(header)
            .child(chips)
            .child(grid)
            .when(empty, |el| el.child(div().absolute().left(px(PAD)).top(px(chrome - PAD)).text_color(palette.on_surface_variant.hsla()).child(tf("No apps match “{query}”", &[("query", &self.query)]))));
        let mut shadow = sheet::panel_shadow(&palette, p);
        if self.install_hover {
            shadow.push(glow(accent.with_alpha(DROP_GLOW.0), DROP_GLOW.1, 0.0, 0.0));
        }
        let radius = RADIUS + (self.anchor.2 * super::TILE_RADIUS_FRAC - RADIUS) * (1.0 - e);
        let mut root = div()
            .id("apps-root")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .font_family(f.ui.clone())
            .text_color(palette.on_surface.hsla())
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.on_key(e, cx)))
            .on_drag_move::<ExternalPaths>(cx.listener(|this, e: &DragMoveEvent<ExternalPaths>, _, cx| {
                let installable = e.drag(cx).paths().iter().any(|p| super::install::accepts(p, cx));
                if installable != this.install_hover {
                    this.install_hover = installable;
                    cx.notify();
                }
            }))
            .child(sheet::outside("apps-outside", cx, |this: &mut Self, cx| {
                if this.menu.take().is_some() {
                    cx.notify();
                } else {
                    this.close(cx);
                }
            }))
            .child(
                div()
                    .id("apps-card")
                    .occlude()
                    .absolute()
                    .left(px(card.0))
                    .top(px(card.1))
                    .w(px(card.2))
                    .h(px(card.3))
                    .opacity(p.clamp(0.0, 1.0).sqrt())
                    .rounded(px(crate::style::radius(radius)))
                    .overflow_hidden()
                    .bg(fill.hsla())
                    .shadow(shadow)
                    .on_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        if this.menu.take().is_some() {
                            cx.notify();
                        }
                    }))
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                        this.install_hover = false;
                        let installable: Vec<PathBuf> = paths.paths().iter().filter(|p| super::install::accepts(p, cx)).cloned().collect();
                        if !super::install::install(installable, cx) {
                            tracing::info!("dock: nothing to install in the dropped files");
                        }
                        cx.notify();
                    }))
                    .child(content)
                    .when(self.install_hover, |el| {
                        el.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(accent.with_alpha(DROP_SCRIM_ALPHA).hsla())
                                .text_size(px(text::TITLE))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(accent.hsla())
                                .child(t("Drop to install")),
                        )
                    }),
            );
        if let Some((i, at, _)) = self.menu {
            let on: sheet::MenuHandler<Self> = Rc::new(|this, row, _, cx| this.run_menu(row, cx));
            root = root.child(sheet::item_menu(self.menu_rows(i, cx), at, screen, &palette, fill, on, cx));
        }
        root
    }
}

/// Header folder size, a fraction of the header height.
const FOLDER_HEADER_FRAC: f32 = 0.8;

/// Desktop id → frecency from the launcher's usage.
fn usage_map(cx: &App) -> HashMap<String, f64> {
    let snapshot = config(cx).read(cx).snapshot();
    crate::launcher_ext::usage_list(&snapshot, USAGE_ROWS).into_iter().filter_map(|(k, s)| k.strip_prefix("app:").map(|id| (id.to_string(), s))).collect()
}

/// Apps installed this session (dock installs) or whose desktop file in
/// your own data folder (Mandara's installer, user Flatpaks) is newer
/// than [`NEW_FOR`]. System entries don't count: package updates touch
/// them all the time.
fn new_apps(apps: &[DesktopApp], cx: &App) -> Vec<String> {
    let landed = super::landed_ids(cx);
    let mine = dirs::data_dir().unwrap_or_default();
    apps.iter()
        .filter(|a| {
            landed.iter().any(|l| l.eq_ignore_ascii_case(&a.id))
                || super::menu::desktop_file(&a.id).filter(|p| !mine.as_os_str().is_empty() && p.starts_with(&mine)).and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|age| age < NEW_FOR)
        })
        .map(|a| a.id.clone())
        .collect()
}

// ── Dock tile ────────────────────────────────────────────────────────────

/// The Applications folder icon: a theme-tinted folder (back sheet with
/// its tab, a lighter gradient front) wearing the apps glyph.
pub fn folder_tile(size: f32, palette: &MandaraPalette) -> AnyElement {
    let accent = palette.accent_gradient_start;
    let back = accent.mix(palette.surface_dim, FOLDER_BACK_MIX);
    let front_top = palette.accent_gradient_end.lighten(FOLDER_SHEEN);
    let glyph = accent.best_on(Color::hex("#ffffff"), Color::hex("#101010"));
    let r = crate::style::radius(size * FOLDER_RADIUS);
    let rect = |(x, y, w, h): (f32, f32, f32, f32)| div().absolute().left(px(size * x)).top(px(size * y)).w(px(size * w)).h(px(size * h));
    div()
        .size(px(size))
        .flex_none()
        .relative()
        .child(rect(FOLDER_TAB).rounded(px(r)).bg(back.hsla()))
        .child(rect(FOLDER_BACK).rounded(px(r)).bg(back.hsla()))
        .child(
            rect(FOLDER_FRONT)
                .rounded(px(r))
                .bg(linear_gradient(180.0, linear_color_stop(front_top.hsla(), 0.0), linear_color_stop(accent.hsla(), 1.0)))
                .shadow(rim(palette))
                .flex()
                .items_center()
                .justify_center()
                .child(icon("grid", size * FOLDER_GLYPH, glyph)),
        )
        .into_any_element()
}

// ── Opening ──────────────────────────────────────────────────────────────

#[derive(Default)]
struct Grid(Option<WindowHandle<AppsGrid>>);
impl Global for Grid {}

/// Whether the Applications grid is open (and not closing).
pub fn is_open(cx: &mut App) -> bool {
    cx.try_global::<Grid>().and_then(|g| g.0).and_then(|h| h.read_with(cx, |v, _| !v.sheet.closing()).ok()).unwrap_or(false)
}

/// Closes the grid. Returns whether it was open.
pub fn close(cx: &mut App) -> bool {
    let Some(h) = cx.try_global::<Grid>().and_then(|g| g.0) else { return false };
    h.update(cx, |v, _, cx| {
        let was = !v.sheet.closing();
        v.close(cx);
        was
    })
    .unwrap_or(false)
}

/// Opens the grid over `anchor` (the folder's dock icon, output
/// coordinates), or closes it when open.
pub fn toggle(monitor: &str, anchor: (f32, f32, f32, f32), cx: &mut App) {
    if !close(cx) {
        open(monitor, anchor, cx);
    }
}

/// Opens the grid over `anchor` on `monitor`.
pub fn open(monitor: &str, anchor: (f32, f32, f32, f32), cx: &mut App) {
    if let Some(h) = cx.try_global::<Grid>().and_then(|g| g.0) {
        let _ = h.update(cx, |_, window, _| window.remove_window());
        super::popover_closed(h.into(), cx);
    }
    let options = sheet::popover_options(monitor, KeyboardInteractivity::Exclusive, cx);
    match cx.open_window(options, move |window, cx| cx.new(|cx| AppsGrid::new(anchor, window, cx))) {
        Ok(h) => {
            cx.set_global(Grid(Some(h)));
            super::popover_opened(monitor, h.into(), cx);
        }
        Err(err) => tracing::warn!("dock applications: {err:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str, categories: &str) -> DesktopApp {
        DesktopApp { id: id.into(), name: name.into(), categories: categories.into(), ..Default::default() }
    }

    #[test]
    fn categories_group_like_a_launchpad() {
        assert_eq!(group_of("Network;WebBrowser;"), "Internet");
        assert_eq!(group_of("GTK;Development;IDE"), "Development");
        assert_eq!(group_of("AudioVideo;Player"), "Media");
        assert_eq!(group_of(""), OTHER);
        assert_eq!(group_of("X-Unknown"), OTHER);
    }

    #[test]
    fn apps_sort_by_name_use_or_category_and_search_ranks() {
        let apps = vec![app("zed", "Zed", "Development;"), app("firefox", "Firefox", "Network;"), app("gimp", "GIMP", "Graphics;"), app("btop", "btop", "System;")];
        let names = |v: Vec<usize>| v.into_iter().map(|i| apps[i].id.clone()).collect::<Vec<_>>();
        let usage: HashMap<String, f64> = [("gimp".to_string(), 9.0), ("zed".to_string(), 3.0)].into();
        assert_eq!(names(arrange(&apps, "", None, AppsOrder::Name, &usage)), ["btop", "firefox", "gimp", "zed"]);
        assert_eq!(names(arrange(&apps, "", None, AppsOrder::MostUsed, &usage)), ["gimp", "zed", "btop", "firefox"]);
        assert_eq!(names(arrange(&apps, "", None, AppsOrder::Categories, &usage)), ["firefox", "zed", "gimp", "btop"]);
        assert_eq!(names(arrange(&apps, "", Some("Graphics"), AppsOrder::Name, &usage)), ["gimp"]);
        assert_eq!(names(arrange(&apps, "fire", None, AppsOrder::MostUsed, &usage)), ["firefox"], "a search ranks, whatever the order");
    }

    #[test]
    fn grid_rows_wrap_and_start_each_category_under_its_caption() {
        let plain = rows(&[0, 1, 2, 3, 4], 2, None);
        assert_eq!(plain.iter().map(|r| r.apps.clone()).collect::<Vec<_>>(), vec![vec![0, 1], vec![2, 3], vec![4]]);
        assert!(plain.iter().all(|r| r.caption.is_none()));
        let group = |i: usize| if i < 3 { "Internet" } else { "Games" };
        let sections = rows(&[0, 1, 2, 3, 4], 2, Some(&group));
        assert_eq!(sections.iter().map(|r| (r.caption, r.apps.clone())).collect::<Vec<_>>(), vec![(Some("Internet"), vec![0, 1]), (None, vec![2]), (Some("Games"), vec![3, 4])]);
        assert!(rows(&[], 4, None).is_empty());
    }
}
