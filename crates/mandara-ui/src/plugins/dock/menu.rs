//! The dock icon menu, laid out like macOS's:
//!
//! * running app — its windows (click to focus) · New Window and the
//!   app's desktop actions · Options ▸ · Show All Windows, Hide · Quit
//!   (Force Quit once a quit didn't take, or with Alt held);
//! * pinned, not running — Open · Options ▸ · Remove from Dock;
//! * Options ▸ — Keep in Dock ✓, Open at Login ✓ (XDG autostart entry
//!   `~/.config/autostart/<id>.desktop`), Show in Files, and Uninstall…
//!   for apps Mandara installed.
//!
//! Submenus open in place (with a back row) so the menu never leaves the
//! screen on any dock edge. Right-click on empty dock space opens the dock
//! menu instead: Add app…, the hiding / magnification switches and Dock
//! settings.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::layer_shell::KeyboardInteractivity;
use gpui::{App, AppContext as _, Context, FocusHandle, FontWeight, Global, InteractiveElement, IntoElement, KeyDownEvent, MouseDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, WindowHandle, div, prelude::FluentBuilder as _, px};
use mandara_config::workflow::{DockAutoHide, DockPosition};
use mandara_core::{SpringConfig, ToGpui};
use mandara_i18n::{t, tf};
use mandara_services::toplevels::ToplevelCommand;

use super::model::{Item, Kind};
use super::sheet::{self, Sheet};
use crate::icons::icon;
use crate::kit::text;
use crate::plugins::panel_fx::{elevation, readable, rim};
use crate::state::{config, update_config};

const MENU_W: f32 = 300.0;
const MENU_ROW_H: f32 = 32.0;
const MENU_PAD: f32 = 7.0;
const MENU_HEADER_H: f32 = 40.0;
const MENU_SEPARATOR_H: f32 = 9.0;
const MENU_RADIUS: f32 = 16.0;
const MENU_GAP: f32 = 10.0;
const ROW_PAD: f32 = 10.0;
const ROW_RADIUS: f32 = 9.0;
const ROW_GAP: f32 = 10.0;
const GLYPH_PX: f32 = 15.0;
const HEADER_ICON_PX: f32 = 26.0;
const HEADER_PAD: f32 = 6.0;
const DIVIDER_ALPHA: f32 = 0.08;
const HOVER_ALPHA: f32 = 0.07;
const DISABLED_ALPHA: f32 = 0.45;
const MENU_TITLE_CHARS: usize = 34;
const OPEN_SPRING: SpringConfig = SpringConfig::new(460.0, 32.0, 1.0);
/// After Quit, the menu offers Force Quit for this long.
const FORCE_QUIT_AFTER: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    Focus(u64),
    Open,
    NewWindow,
    Action(crate::apps::DesktopAction),
    Options,
    Back,
    KeepInDock,
    OpenAtLogin,
    ShowInFiles,
    Uninstall,
    ShowAll,
    Hide,
    Quit,
    ForceQuit,
    RemoveFromDock,
    // Stacks / trash / minimized / dock space.
    OpenStack,
    RemoveStack,
    OpenTrash,
    EmptyTrash,
    Restore(u64),
    AddApp,
    ToggleAutoHide,
    ToggleMagnification,
    DockSettings,
}

/// One row: label, glyph, action, checkmark (`Some` for toggles), a
/// separator above it, danger styling.
#[derive(Clone, Debug)]
pub struct Row {
    pub label: SharedString,
    pub glyph: &'static str,
    pub action: MenuAction,
    pub check: Option<bool>,
    pub sep: bool,
    pub danger: bool,
}

fn row(label: impl Into<SharedString>, glyph: &'static str, action: MenuAction) -> Row {
    Row { label: label.into(), glyph, action, check: None, sep: false, danger: false }
}

impl Row {
    fn sep(mut self) -> Self {
        self.sep = true;
        self
    }
    fn check(mut self, on: bool) -> Self {
        self.check = Some(on);
        self
    }
    fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

/// What the menu knows about the app beyond the dock item.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub open_at_login: bool,
    pub installed_by_mandara: bool,
    /// A Quit was sent recently and the app still runs.
    pub quit_pending: bool,
    /// Alt held while opening: offer Force Quit right away.
    pub force: bool,
    pub trash_items: usize,
}

/// The rows of `item`'s menu on `page` (`false` = main, `true` = Options).
pub fn rows(item: &Item, options: bool, facts: &Facts) -> Vec<Row> {
    let mut out = Vec::new();
    match item.kind {
        Kind::Stack(_) => {
            out.push(row(t("Open"), "folder", MenuAction::OpenStack));
            out.push(row(t("Open in Files"), "external", MenuAction::ShowInFiles));
            out.push(row(t("Remove from Dock"), "pin-off", MenuAction::RemoveStack).sep());
            return out;
        }
        Kind::Trash => {
            out.push(row(t("Open"), "trash", MenuAction::OpenTrash));
            let mut empty = row(t("Empty Trash…"), "trash", MenuAction::EmptyTrash).sep().danger();
            if facts.trash_items == 0 {
                empty.danger = false;
            }
            out.push(empty);
            return out;
        }
        Kind::Minimized(id) => {
            out.push(row(t("Restore"), "maximize", MenuAction::Restore(id)));
            out.push(row(t("Close"), "close", MenuAction::Quit).danger());
            return out;
        }
        Kind::Launcher | Kind::Applications => {
            out.push(row(t("Open"), "grid", MenuAction::Open));
            out.push(row(t("Add app…"), "plus", MenuAction::AddApp));
            return out;
        }
        Kind::Separator | Kind::Installing(_) | Kind::Media => return dock_rows(&mandara_config::workflow::DockConfig::default()),
        Kind::App => {}
    }
    if options {
        out.push(row(t("Back"), "chevron-left", MenuAction::Back));
        out.push(row(t("Keep in Dock"), "pin", MenuAction::KeepInDock).check(item.pinned).sep());
        if item.desktop.is_some() {
            out.push(row(t("Open at Login"), "zap", MenuAction::OpenAtLogin).check(facts.open_at_login));
            out.push(row(t("Show in Files"), "folder", MenuAction::ShowInFiles));
        }
        if facts.installed_by_mandara {
            out.push(row(t("Uninstall…"), "trash", MenuAction::Uninstall).sep().danger());
        }
        return out;
    }
    for w in &item.windows {
        let title = if w.title.is_empty() { item.name.clone() } else { crate::kit::ellipsize(&w.title, MENU_TITLE_CHARS) };
        out.push(row(title, if w.activated { "check" } else if w.minimized { "minus" } else { "window" }, MenuAction::Focus(w.id)));
    }
    let mut sep = !out.is_empty();
    if item.running() {
        if item.desktop.is_some() {
            let mut r = row(t("New Window"), "plus", MenuAction::NewWindow);
            r.sep = sep;
            out.push(r);
            for a in item.desktop.as_ref().map(|d| d.actions.clone()).unwrap_or_default().into_iter().filter(|a| !is_new_window(a)) {
                out.push(row(a.name.clone(), "chevron-right", MenuAction::Action(a)));
            }
            sep = true;
        }
    } else {
        out.push(row(t("Open"), "play", MenuAction::Open));
        for a in item.desktop.as_ref().map(|d| d.actions.clone()).unwrap_or_default() {
            out.push(row(a.name.clone(), "chevron-right", MenuAction::Action(a)));
        }
        sep = true;
    }
    let mut opts = row(t("Options"), "sliders", MenuAction::Options);
    opts.sep = sep;
    out.push(opts);
    if item.running() {
        out.push(row(t("Show All Windows"), "tiles", MenuAction::ShowAll).sep());
        out.push(row(t("Hide"), "eye-off", MenuAction::Hide));
        out.push(if facts.quit_pending || facts.force { row(t("Force Quit"), "close", MenuAction::ForceQuit).danger() } else { row(t("Quit"), "close", MenuAction::Quit) });
    } else if item.pinned {
        out.push(row(t("Remove from Dock"), "pin-off", MenuAction::RemoveFromDock).sep());
    } else if item.recent {
        out.push(row(t("Keep in Dock"), "pin", MenuAction::KeepInDock).sep());
    }
    out
}

/// A desktop action that duplicates the built-in "New Window" row.
fn is_new_window(a: &crate::apps::DesktopAction) -> bool {
    let name = a.name.trim();
    a.id.eq_ignore_ascii_case("new-window") || name.eq_ignore_ascii_case("New Window") || name == t("New Window")
}

/// The menu of empty dock space.
pub fn dock_rows(cfg: &mandara_config::workflow::DockConfig) -> Vec<Row> {
    vec![
        row(t("Add app…"), "plus", MenuAction::AddApp),
        row(t("Automatically hide and show the Dock"), "eye-off", MenuAction::ToggleAutoHide).check(cfg.auto_hide != DockAutoHide::Never).sep(),
        row(t("Magnification"), "maximize", MenuAction::ToggleMagnification).check(cfg.magnification),
        row(t("Dock settings…"), "sliders", MenuAction::DockSettings).sep(),
    ]
}

// ── Actions ──────────────────────────────────────────────────────────────

/// Quit requests by app key, for Force Quit.
#[derive(Default)]
pub struct QuitRequests(pub HashMap<String, Instant>);
impl Global for QuitRequests {}

fn quit_pending(key: &str, cx: &App) -> bool {
    cx.try_global::<QuitRequests>().and_then(|q| q.0.get(key)).is_some_and(|t| t.elapsed() < FORCE_QUIT_AFTER)
}

/// The `.desktop` file of desktop id `id`.
pub fn desktop_file(id: &str) -> Option<PathBuf> {
    mandara_config::apps::application_dirs().into_iter().map(|d| d.join(format!("{id}.desktop"))).find(|p| p.is_file())
}

/// Where "Show in Files" goes: the program's folder for apps run from a
/// path (AppImages, tarballs), else the `.desktop` file's folder.
pub fn show_in_files_target(exec: &str, desktop: Option<PathBuf>) -> Option<PathBuf> {
    let program = crate::apps::parse_exec(exec).into_iter().next().map(PathBuf::from);
    match program {
        Some(p) if p.is_absolute() && !p.starts_with("/usr") && !p.starts_with("/bin") => p.parent().map(PathBuf::from),
        _ => desktop.and_then(|d| d.parent().map(PathBuf::from)),
    }
}

/// Autostart id of desktop id `id`.
fn autostart_id(id: &str) -> String {
    format!("{}.desktop", mandara_config::dock::pin_id(id))
}

pub fn open_at_login(id: &str) -> bool {
    let want = autostart_id(id);
    mandara_config::apps::autostart_entries().iter().any(|e| e.id == want && e.enabled)
}

/// Turns "Open at Login" on or off for desktop id `id`.
pub fn set_open_at_login(id: &str, on: bool) -> Result<(), String> {
    use mandara_config::apps::{AutostartNew, add_autostart, autostart_entries, set_autostart_enabled};
    let want = autostart_id(id);
    let exists = autostart_entries().iter().any(|e| e.id == want);
    let r = if exists { set_autostart_enabled(&want, on) } else if on { add_autostart(&AutostartNew::App(want)).map(|_| ()) } else { Ok(()) };
    r.map_err(|e| format!("{e:#}"))
}

/// Process ids whose command name or executable matches `program`
/// (`/proc`; only this user's processes, never the shell itself).
pub fn pids_of(program: &str) -> Vec<u32> {
    use std::os::unix::fs::MetadataExt as _;
    let base = std::path::Path::new(program).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if base.is_empty() {
        return Vec::new();
    }
    let me = std::process::id();
    let uid = rustix::process::getuid().as_raw();
    let comm_name: String = base.chars().take(15).collect();
    let Ok(read) = std::fs::read_dir("/proc") else { return Vec::new() };
    read.flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()).map(|pid| (pid, e.path())))
        .filter(|(pid, path)| *pid != me && std::fs::metadata(path).is_ok_and(|m| m.uid() == uid))
        .filter(|(_, path)| {
            let comm = std::fs::read_to_string(path.join("comm")).unwrap_or_default();
            let exe = std::fs::read_link(path.join("exe")).ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_default();
            comm.trim() == comm_name || exe == base
        })
        .map(|(pid, _)| pid)
        .collect()
}

/// Force-quits `item`: SIGKILL to its processes (by program name), or
/// `flatpak kill` for Flatpak apps.
fn force_quit(item: &Item) {
    let Some(app) = item.desktop.clone() else {
        tracing::warn!("dock: can't find {}'s program to force quit", item.name);
        return;
    };
    std::thread::spawn(move || {
        let argv = crate::apps::parse_exec(&app.exec);
        if argv.first().is_some_and(|p| p.ends_with("flatpak")) {
            let _ = std::process::Command::new("flatpak").args(["kill", &app.id]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
            return;
        }
        let program = argv.iter().find(|a| !a.contains('=') && *a != "env").cloned().unwrap_or_default();
        for pid in pids_of(&program) {
            if let Err(err) = mandara_services::processes::kill(pid, true) {
                tracing::warn!("dock force quit: {err}");
            }
        }
    });
}

pub struct DockMenu {
    item: Item,
    monitor: String,
    anchor: (f32, f32, f32, f32),
    position: DockPosition,
    facts: Facts,
    options: bool,
    focus: FocusHandle,
    sheet: Sheet,
    /// Empty dock space (the dock menu, not an app's).
    space: bool,
    /// "Empty Trash…" was clicked once: the next click empties.
    confirming: bool,
}

impl DockMenu {
    fn rows(&self, cx: &App) -> Vec<Row> {
        if self.space {
            return dock_rows(&config(cx).read(cx).get().dock);
        }
        rows(&self.item, self.options, &self.facts)
    }

    fn run(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        let item = self.item.clone();
        let windows = crate::windows::init(cx);
        match action {
            MenuAction::Options => {
                self.options = true;
                cx.notify();
                return;
            }
            MenuAction::Back => {
                self.options = false;
                cx.notify();
                return;
            }
            MenuAction::Focus(id) => {
                windows.read(cx).send(ToplevelCommand::SetMinimized(id, false));
                crate::windows::activate(id, cx);
            }
            MenuAction::Open => match item.kind {
                Kind::Launcher | Kind::Applications => {
                    let s = crate::state::shell(cx);
                    s.update(cx, |s, cx| s.set_launcher(true, cx));
                }
                _ => super::launch_item(&item, None, cx),
            },
            MenuAction::NewWindow => super::launch_item(&item, None, cx),
            MenuAction::Action(a) => super::launch_item(&item, Some(&a), cx),
            MenuAction::KeepInDock => {
                let id = item.pin_key();
                let running = item.running();
                update_config(cx, move |c| {
                    c.dock.toggle_pin(&id);
                });
                if item.pinned && running {
                    super::linger(&item.key, cx);
                }
            }
            MenuAction::RemoveFromDock => {
                let id = item.pin_key();
                update_config(cx, move |c| {
                    c.dock.unpin(&id);
                });
            }
            MenuAction::OpenAtLogin => {
                let id = item.pin_key();
                let on = !self.facts.open_at_login;
                std::thread::spawn(move || {
                    if let Err(err) = set_open_at_login(&id, on) {
                        tracing::warn!("dock: open at login: {err}");
                    }
                });
            }
            MenuAction::ShowInFiles => {
                let target = match item.kind {
                    Kind::Stack(i) => config(cx).read(cx).get().dock.stacks.get(i).map(super::stacks::stack_dir),
                    _ => item.desktop.as_ref().and_then(|d| show_in_files_target(&d.exec, desktop_file(&d.id))),
                };
                if let Some(dir) = target {
                    super::stacks::open_path(&dir);
                }
            }
            MenuAction::Uninstall => crate::state::open_studio_at(Some("applications/installed".into()), cx),
            MenuAction::ShowAll => {
                for w in item.windows.iter().rev() {
                    windows.read(cx).send(ToplevelCommand::SetMinimized(w.id, false));
                    crate::windows::activate(w.id, cx);
                }
            }
            MenuAction::Hide => {
                for w in &item.windows {
                    windows.read(cx).send(ToplevelCommand::SetMinimized(w.id, true));
                }
            }
            MenuAction::Quit => {
                for w in &item.windows {
                    crate::windows::close(w.id, cx);
                }
                cx.default_global::<QuitRequests>().0.insert(item.key.clone(), Instant::now());
            }
            MenuAction::ForceQuit => {
                force_quit(&item);
                cx.default_global::<QuitRequests>().0.remove(&item.key);
            }
            MenuAction::OpenStack => {
                if let Kind::Stack(i) = item.kind {
                    super::stacks::open_popover(i, &self.monitor, self.anchor, cx);
                }
            }
            MenuAction::RemoveStack => {
                if let Kind::Stack(i) = item.kind {
                    update_config(cx, move |c| {
                        if i < c.dock.stacks.len() {
                            c.dock.stacks.remove(i);
                        }
                    });
                }
            }
            MenuAction::OpenTrash => super::open_trash(),
            MenuAction::EmptyTrash => {
                // Two clicks: the first arms (the row turns into the
                // confirmation), the second empties.
                match super::stacks::empty_trash_confirmed(false, cx) {
                    Ok(None) => {
                        self.facts.trash_items = super::stacks::trash_count(cx);
                        self.confirming = true;
                        cx.notify();
                        return;
                    }
                    Ok(Some(n)) => tracing::info!("dock: emptied the Trash ({n} items)"),
                    Err(err) => tracing::warn!("dock: {err}"),
                }
            }
            MenuAction::Restore(id) => {
                windows.read(cx).send(ToplevelCommand::SetMinimized(id, false));
                crate::windows::activate(id, cx);
            }
            MenuAction::AddApp => super::picker::open(&self.monitor, cx),
            MenuAction::ToggleAutoHide => update_config(cx, |c| c.dock.auto_hide = if c.dock.auto_hide == DockAutoHide::Never { DockAutoHide::Always } else { DockAutoHide::Never }),
            MenuAction::ToggleMagnification => update_config(cx, |c| c.dock.magnification = !c.dock.magnification),
            MenuAction::DockSettings => crate::state::open_studio_at(Some("dock".into()), cx),
        }
        let _ = window;
        self.sheet.close();
        cx.notify();
    }
}

impl DockMenu {
    fn render_rows(&self, rows: Vec<Row>, palette: &mandara_theme::MandaraPalette, cx: &mut Context<Self>) -> gpui::Div {
        let accent = readable(palette.primary, palette);
        let mut list = div().flex().flex_col();
        for (i, r) in rows.into_iter().enumerate() {
            if r.sep {
                list = list.child(div().h(px(MENU_SEPARATOR_H)).flex().items_center().child(div().w_full().h(px(1.0)).bg(palette.on_surface.with_alpha(DIVIDER_ALPHA).hsla())));
            }
            let confirm = self.confirming && r.action == MenuAction::EmptyTrash;
            let label: SharedString = if confirm { tf("Empty Trash? {n} items are deleted forever — click again", &[("n", &self.facts.trash_items)]).into() } else { r.label.clone() };
            let tint = if r.danger || confirm { palette.urgent_coral } else { palette.on_surface_variant };
            let disabled = r.action == MenuAction::EmptyTrash && self.facts.trash_items == 0;
            let action = r.action.clone();
            let submenu = r.action == MenuAction::Options;
            list = list.child(
                div()
                    .id(("dock-menu-row", i))
                    .h(px(MENU_ROW_H))
                    .px(px(ROW_PAD))
                    .rounded(px(crate::style::radius(ROW_RADIUS)))
                    .flex()
                    .items_center()
                    .gap(px(ROW_GAP))
                    .when(!disabled, |el| el.cursor_pointer().hover(|s| s.bg(palette.on_surface.with_alpha(HOVER_ALPHA).hsla())))
                    .when(disabled, |el| el.opacity(DISABLED_ALPHA))
                    .child(icon(r.glyph, GLYPH_PX, tint))
                    .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).when(r.danger || confirm, |el| el.text_color(palette.urgent_coral.hsla())).child(label))
                    .when_some(r.check, |el, on| el.child(div().size(px(GLYPH_PX)).when(on, |el| el.child(icon("check", GLYPH_PX, accent)))))
                    .when(submenu, |el| el.child(icon("chevron-right", GLYPH_PX, palette.on_surface_variant)))
                    .when(!disabled, |el| el.on_click(cx.listener(move |this, _, window, cx| this.run(action.clone(), window, cx)))),
            );
        }
        list
    }
}

impl Render for DockMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = mandara_core::frame_palette(window, cx);
        let f = crate::fonts::fonts(cx);
        let vp = window.viewport_size();
        let (sw, sh) = (f32::from(vp.width), f32::from(vp.height));
        let rows = self.rows(cx);
        let separators = rows.iter().filter(|r| r.sep).count() as f32;
        let header = !self.space && matches!(self.item.kind, Kind::App | Kind::Stack(_) | Kind::Trash | Kind::Minimized(_));
        let h = MENU_PAD * 2.0 + if header { MENU_HEADER_H } else { 0.0 } + rows.len() as f32 * MENU_ROW_H + separators * MENU_SEPARATOR_H;
        let (ax, ay, aw, ah) = self.anchor;
        let (x, y) = match self.position {
            DockPosition::Bottom => (ax + aw / 2.0 - MENU_W / 2.0, ay - MENU_GAP - h),
            DockPosition::Left => (ax + aw + MENU_GAP, ay + ah / 2.0 - h / 2.0),
            DockPosition::Right => (ax - MENU_GAP - MENU_W, ay + ah / 2.0 - h / 2.0),
        };
        let (x, y) = (x.clamp(MENU_GAP, (sw - MENU_W - MENU_GAP).max(MENU_GAP)), y.clamp(MENU_GAP, (sh - h - MENU_GAP).max(MENU_GAP)));
        let p = self.sheet.step(false, window, cx).clamp(0.0, 1.0);
        let accent = readable(palette.primary, &palette);
        let list = self.render_rows(rows, &palette, cx);
        let mut shadow = elevation(&palette, p);
        shadow.extend(rim(&palette));
        let title: SharedString = match self.item.kind {
            Kind::Trash => t("Trash").into(),
            Kind::Stack(i) => config(cx).read(cx).get().dock.stacks.get(i).map(super::stacks::stack_name).unwrap_or_default().into(),
            _ => self.item.name.clone().into(),
        };
        let header_glyph: gpui::AnyElement = match self.item.kind {
            Kind::Trash => icon("trash", HEADER_ICON_PX, palette.on_surface_variant).into_any_element(),
            Kind::Stack(_) => icon("folder", HEADER_ICON_PX, accent).into_any_element(),
            _ => crate::plugins::workspaces::app_icon(&self.item.icon_id, HEADER_ICON_PX, palette.on_surface, false, &palette),
        };
        let count = match self.item.kind {
            Kind::App if self.item.running() => Some(tf("{n} open", &[("n", &self.item.windows.len())])),
            Kind::Trash => Some(tf("{n} items", &[("n", &self.facts.trash_items)])),
            _ => None,
        };
        div()
            .id("dock-menu-root")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .font_family(f.ui.clone())
            .text_color(palette.on_surface.hsla())
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    if this.options {
                        this.options = false;
                    } else {
                        this.sheet.close();
                    }
                    cx.notify();
                }
            }))
            // Outside clicks hit this catcher; the card occludes it (see
            // `crate::dismiss`: the card slides in, so no fixed rect).
            .child(div().id("dock-menu-outside").absolute().inset_0().on_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.sheet.close();
                cx.notify();
            })))
            .child(
                div()
                    .id("dock-menu-card")
                    .occlude()
                    .absolute()
                    .left(px(x))
                    .top(px(y + (1.0 - p) * MENU_GAP))
                    .w(px(MENU_W))
                    .h(px(h))
                    .opacity(p)
                    .rounded(px(crate::style::radius(MENU_RADIUS)))
                    .bg(crate::shell::frame_color(&palette).hsla())
                    .shadow(shadow)
                    .p(px(MENU_PAD))
                    .flex()
                    .flex_col()
                    .when(header, |el| {
                        el.child(
                            div()
                                .h(px(MENU_HEADER_H))
                                .px(px(HEADER_PAD))
                                .flex()
                                .items_center()
                                .gap(px(ROW_GAP))
                                .child(header_glyph)
                                .child(div().flex_1().min_w(px(0.0)).truncate().font_weight(FontWeight::SEMIBOLD).child(title))
                                .when_some(count, |el, c| el.child(div().text_size(px(text::LABEL)).text_color(accent.hsla()).font_features(crate::fonts::tabular_nums()).child(c))),
                        )
                    })
                    .child(list),
            )
    }
}

#[derive(Default)]
struct Menu(Option<WindowHandle<DockMenu>>);
impl Global for Menu {}

/// Opens `item`'s menu (or the dock menu when `space`) over `anchor` in
/// output coordinates.
pub fn open(item: Item, space: bool, force: bool, monitor: String, anchor: (f32, f32, f32, f32), cx: &mut App) {
    let cfg = config(cx).read(cx).get().dock.clone();
    if let Some(h) = cx.try_global::<Menu>().and_then(|m| m.0) {
        let _ = h.update(cx, |_, window, _| window.remove_window());
        super::popover_closed(h.into(), cx);
    }
    let desktop_id = item.desktop.as_ref().map(|d| d.id.clone());
    let facts = Facts {
        open_at_login: desktop_id.as_deref().is_some_and(open_at_login),
        installed_by_mandara: desktop_id.as_deref().is_some_and(|id| super::install::installed_by_mandara(id, cx)),
        quit_pending: quit_pending(&item.key, cx) && item.running(),
        force,
        trash_items: if item.kind == Kind::Trash { super::stacks::trash_count(cx) } else { 0 },
    };
    let options = sheet::popover_options(&monitor, KeyboardInteractivity::OnDemand, cx);
    let on = monitor.clone();
    let handle = cx.open_window(options, move |window, cx| {
        cx.new(|cx| {
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            DockMenu { item, monitor, anchor, position: cfg.position, facts, options: false, focus, sheet: Sheet::new(OPEN_SPRING), space, confirming: false }
        })
    });
    match handle {
        Ok(h) => {
            cx.set_global(Menu(Some(h)));
            super::popover_opened(&on, h.into(), cx);
        }
        Err(err) => tracing::warn!("dock menu: {err:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_services::toplevels::Toplevel;

    fn item(pinned: bool, windows: usize) -> Item {
        let desktop = crate::apps::App { id: "org.demo.beta".into(), name: "Beta".into(), exec: "beta %U".into(), actions: vec![crate::apps::DesktopAction { id: "private".into(), name: "New Private Window".into(), exec: "beta --private".into() }], ..Default::default() };
        Item {
            key: "org.demo.beta".into(),
            icon_id: "org.demo.beta".into(),
            name: "Beta".into(),
            desktop: Some(desktop),
            windows: (0..windows).map(|i| Toplevel { id: i as u64 + 1, app_id: "org.demo.beta".into(), title: format!("Tab {i}"), ..Default::default() }).collect(),
            pinned,
            recent: false,
            kind: Kind::App,
        }
    }

    fn actions(rows: &[Row]) -> Vec<MenuAction> {
        rows.iter().map(|r| r.action.clone()).collect()
    }

    #[test]
    fn a_running_app_menu_is_laid_out_like_macos() {
        let r = rows(&item(false, 2), false, &Facts::default());
        let a = actions(&r);
        assert_eq!(a[..2], [MenuAction::Focus(1), MenuAction::Focus(2)]);
        assert_eq!(a[2], MenuAction::NewWindow);
        assert!(r[2].sep, "a separator under the window list");
        assert!(matches!(a[3], MenuAction::Action(_)));
        assert_eq!(a[4..], [MenuAction::Options, MenuAction::ShowAll, MenuAction::Hide, MenuAction::Quit]);
        let pending = rows(&item(false, 1), false, &Facts { quit_pending: true, ..Default::default() });
        assert_eq!(pending.last().unwrap().action, MenuAction::ForceQuit, "Quit didn't take: Force Quit");
    }

    #[test]
    fn keep_in_dock_is_a_checkmark_toggle_in_options() {
        let unpinned = rows(&item(false, 1), true, &Facts::default());
        let keep = unpinned.iter().find(|r| r.action == MenuAction::KeepInDock).unwrap();
        assert_eq!(keep.check, Some(false));
        let pinned = rows(&item(true, 1), true, &Facts { open_at_login: true, installed_by_mandara: true, ..Default::default() });
        assert_eq!(pinned.iter().find(|r| r.action == MenuAction::KeepInDock).unwrap().check, Some(true));
        assert_eq!(pinned.iter().find(|r| r.action == MenuAction::OpenAtLogin).unwrap().check, Some(true));
        assert!(pinned.iter().any(|r| r.action == MenuAction::Uninstall), "only for apps Mandara installed");
        assert!(!unpinned.iter().any(|r| r.action == MenuAction::Uninstall));
    }

    #[test]
    fn a_pinned_app_that_is_not_running_offers_open_and_remove() {
        let a = actions(&rows(&item(true, 0), false, &Facts::default()));
        assert_eq!(a[0], MenuAction::Open);
        assert_eq!(a.last(), Some(&MenuAction::RemoveFromDock));
        assert!(!a.contains(&MenuAction::Quit));
    }

    #[test]
    fn show_in_files_prefers_the_program_folder() {
        assert_eq!(show_in_files_target("/opt/Thing/thing.AppImage --x", None), Some(PathBuf::from("/opt/Thing")));
        assert_eq!(show_in_files_target("firefox %u", Some(PathBuf::from("/usr/share/applications/firefox.desktop"))), Some(PathBuf::from("/usr/share/applications")));
        assert_eq!(show_in_files_target("/usr/bin/foo", None), None);
    }
}
