//! Taskbar plugin: your pinned and running apps as icons in the bar (the
//! Taskbar bar style centres them next to the launcher).
//!
//! Pinned apps are the dock's (`dock.pinned`, so pinning in either place
//! pins in both), then every other app with a window, in the order the
//! window list gives. The windows come from the shared window list
//! (`wlr-foreign-toplevel` / `ext-foreign-toplevel`); where the compositor
//! offers neither, the compositor driver's client list marks what runs, and
//! a click starts the app (which raises its window) instead of focusing it.
//! Click: start, focus, or — the focused app with one window — minimize;
//! middle click: a new window.

use gpui::{InteractiveElement as _, IntoElement, MouseButton, ParentElement, StatefulInteractiveElement as _, Styled, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui;
use mandara_plugins::{PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::toplevels::{Toplevel, ToplevelCommand};

use super::view::domain_view_observing;
use crate::apps::App as DesktopApp;
use crate::hub::{Domains, hub};

pub const ID: &str = mandara_config::bar_styles::TASKBAR;

/// An icon's size (before the bar's icon scale), the button round it, the
/// running mark under it and how far apart buttons sit, logical px.
const ICON_PX: f32 = 18.0;
const BUTTON_PAD_PX: f32 = 5.0;
const MARK_W_PX: f32 = 6.0;
const MARK_W_FOCUSED_PX: f32 = 14.0;
const MARK_H_PX: f32 = 2.5;
const GAP_PX: f32 = 2.0;
/// Most buttons shown (the rest are left out, like a full taskbar).
const MAX_SLOTS: usize = 24;
/// Hover and focused tints of a button.
const HOVER_ALPHA: f32 = 0.10;
const FOCUSED_ALPHA: f32 = 0.16;

/// One button: an app, pinned or running, and its windows.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    /// Desktop id (pinned) or window app id (running).
    pub key: String,
    /// The app id its icon is looked up by.
    pub icon: String,
    pub pinned: bool,
    /// Its windows, most recently used first (empty: not running, or no
    /// window list).
    pub windows: Vec<u64>,
    pub running: bool,
    pub focused: bool,
}

/// The buttons for `pinned` apps and running `windows` (`clients` from the
/// compositor when there is no window list).
pub fn slots(pinned: &[String], windows: &[Toplevel], clients: &[(String, bool)], apps: &[DesktopApp]) -> Vec<Slot> {
    let mut out: Vec<Slot> = Vec::with_capacity(pinned.len() + windows.len().min(MAX_SLOTS));
    for p in pinned.iter().filter(|p| p.as_str() != mandara_config::dock::LAUNCHER_PIN) {
        let entry = apps.iter().find(|a| a.id.eq_ignore_ascii_case(p));
        let mine: Vec<&Toplevel> = windows.iter().filter(|w| super::dock::model::belongs(&w.app_id, p, entry)).collect();
        // A pin of an app that isn't installed (and doesn't run) is left out.
        if entry.is_none() && mine.is_empty() && !apps.is_empty() {
            continue;
        }
        let icon = entry.and_then(|e| e.wm_class.clone()).filter(|_| mine.is_empty()).or_else(|| mine.first().map(|w| w.app_id.clone())).unwrap_or_else(|| p.clone());
        out.push(Slot { key: p.clone(), icon, pinned: true, windows: mine.iter().map(|w| w.id).collect(), running: !mine.is_empty(), focused: mine.iter().any(|w| w.activated) });
    }
    let claimed = |out: &[Slot], w: &Toplevel| out.iter().any(|s| s.windows.contains(&w.id) || (!s.pinned && crate::windows::same_app(&s.key, &w.app_id)));
    // Other running apps in window-creation order: stable while you
    // switch between them.
    let mut by_age: Vec<&Toplevel> = windows.iter().filter(|w| !crate::windows::is_shell(&w.app_id) && w.parent.is_none()).collect();
    by_age.sort_by_key(|w| w.id);
    for w in by_age {
        if out.len() >= MAX_SLOTS {
            break;
        }
        if let Some(s) = out.iter_mut().find(|s| !s.pinned && crate::windows::same_app(&s.key, &w.app_id)) {
            s.windows.push(w.id);
            s.focused |= w.activated;
            continue;
        }
        if claimed(&out, w) {
            continue;
        }
        out.push(Slot { key: w.app_id.clone(), icon: w.app_id.clone(), pinned: false, windows: vec![w.id], running: true, focused: w.activated });
    }
    // No window list: the compositor's clients say what runs.
    if windows.is_empty() {
        for (app_id, focused) in clients.iter().filter(|(a, _)| !a.is_empty() && !crate::windows::is_shell(a)) {
            if let Some(s) = out.iter_mut().find(|s| crate::windows::same_app(&s.key, app_id) || apps.iter().any(|a| a.id.eq_ignore_ascii_case(&s.key) && super::dock::model::belongs(app_id, &s.key, Some(a)))) {
                s.running = true;
                s.focused |= *focused;
            } else if out.len() < MAX_SLOTS {
                out.push(Slot { key: app_id.clone(), icon: app_id.clone(), pinned: false, windows: Vec::new(), running: true, focused: *focused });
            }
        }
    }
    out
}

/// What a click on `slot` does.
#[derive(Debug, PartialEq, Eq)]
pub enum Click {
    Launch,
    Focus(u64),
    Minimize(u64),
}

pub fn click(slot: &Slot, windows: &[Toplevel]) -> Click {
    match slot.windows.as_slice() {
        [] => Click::Launch,
        [only] if slot.focused && windows.iter().any(|w| w.id == *only && !w.minimized) => Click::Minimize(*only),
        // The focused app: its next window; else its latest.
        ids => Click::Focus(if slot.focused { ids.get(1).copied().unwrap_or(ids[0]) } else { ids[0] }),
    }
}

/// A button's corner radius in the bar's item shape.
fn radius(shape: mandara_config::surfaces::BarItemShape, side: f32) -> f32 {
    use mandara_config::surfaces::BarItemShape;
    side * match shape {
        BarItemShape::Capsule => 0.5,
        BarItemShape::Squircle | BarItemShape::Connected => 0.3,
        BarItemShape::Tile => 0.22,
    }
}

fn launch(key: &str, cx: &mut gpui::App) {
    let catalog = crate::apps::changes::init(cx);
    let apps = catalog.read(cx).apps.clone();
    let Some(app) = apps.iter().find(|a| a.id.eq_ignore_ascii_case(key)).or_else(|| crate::windows::desktop_entry_for(key, &apps)) else {
        tracing::warn!("taskbar: no desktop entry for {key}");
        return;
    };
    let options = crate::state::config(cx).read(cx).get().launch_options();
    if let Err(err) = crate::apps::launch_with(app, None, &options) {
        tracing::warn!("taskbar: {err}");
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, mandara_i18n::tk("Taskbar"), mandara_i18n::tk("Your pinned and running apps in the bar"), Some(Zone::Center)).opt_in(), |window, cx, _| {
        let windows = crate::windows::init(cx);
        let catalog = crate::apps::changes::init(cx);
        let follow = windows.clone();
        let bar = domain_view_observing(
            Domains::CLIENTS.union(Domains::FOCUS),
            None,
            move |cx| Some(cx.observe(&follow, |_, _, cx| cx.notify())),
            move |palette, _, cx| {
                let cx_ref: &gpui::App = cx;
                let list = windows.read(cx_ref).list();
                let apps = catalog.read(cx_ref).apps.clone();
                let cfg = crate::state::config(cx_ref).read(cx_ref).snapshot();
                let clients: Vec<(String, bool)> = if list.is_empty() { hub(cx_ref).read(cx_ref).clients.iter().map(|c| (c.app_id.clone(), c.focused)).collect() } else { Vec::new() };
                let slots = slots(&cfg.dock.pinned, list, &clients, &apps);
                let monochrome = cfg.appearance.bar_icons == mandara_config::IconStyle::Monochrome;
                let icon = crate::style::bar_icon(ICON_PX);
                let side = icon + BUTTON_PAD_PX * 2.0;
                let radius = radius(cfg.bar.item_shape, side);
                div()
                    .flex()
                    .items_center()
                    .gap(px(GAP_PX))
                    .children(slots.into_iter().enumerate().map(|(i, slot)| {
                        let mark_w = if slot.focused { MARK_W_FOCUSED_PX } else { MARK_W_PX };
                        let (key, middle_key) = (slot.key.clone(), slot.key.clone());
                        let windows_click = windows.clone();
                        div()
                            .id(("taskbar-slot", i))
                            .relative()
                            .size(px(side))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(radius))
                            .when(slot.focused, |el| el.bg(palette.on_surface.with_alpha(FOCUSED_ALPHA).hsla()))
                            .hover(|s| s.bg(palette.on_surface.with_alpha(HOVER_ALPHA).hsla()))
                            .opacity(if slot.running || !slot.pinned { 1.0 } else { cfg.dock.inactive_opacity.clamp(0.4, 1.0) })
                            .child(super::workspaces::app_icon(&slot.icon, icon, palette.on_surface, monochrome, palette))
                            .when(slot.running, |el| {
                                el.child(div().absolute().bottom(px(1.0)).left(px((side - mark_w) / 2.0)).w(px(mark_w)).h(px(MARK_H_PX)).rounded_full().bg(if slot.focused { palette.primary.hsla() } else { palette.on_surface_variant.hsla() }))
                            })
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                let w = windows_click.read(cx);
                                match click(&slot, w.list()) {
                                    Click::Launch => launch(&key, cx),
                                    Click::Focus(id) => {
                                        if w.list().iter().any(|t| t.id == id && t.minimized) {
                                            w.send(ToplevelCommand::SetMinimized(id, false));
                                        }
                                        crate::windows::activate(id, cx);
                                    }
                                    Click::Minimize(id) => w.send(ToplevelCommand::SetMinimized(id, true)),
                                }
                            })
                            .on_mouse_down(MouseButton::Middle, move |_, _, cx| {
                                cx.stop_propagation();
                                launch(&middle_key, cx);
                            })
                    }))
                    .into_any_element()
            },
            window,
            cx,
        );
        PluginInstance { bar: Some(bar), ..PluginInstance::empty() }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(id: u64, app: &str, activated: bool) -> Toplevel {
        Toplevel { id, app_id: app.into(), title: format!("{app} {id}"), activated, ..Default::default() }
    }

    #[test]
    fn pinned_apps_first_then_running_ones_once_each() {
        let pinned = vec!["@launcher".to_string(), "firefox".to_string(), "org.gnome.Nautilus".to_string()];
        let windows = [win(1, "foot", true), win(2, "firefox", false), win(3, "foot", false), win(4, mandara_core::SHELL_APP_ID, false)];
        let s = slots(&pinned, &windows, &[], &[]);
        let keys: Vec<&str> = s.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, ["firefox", "org.gnome.Nautilus", "foot"], "the launcher pin and the shell are not apps");
        assert!(s[0].running && !s[0].focused && s[0].windows == [2]);
        assert!(!s[1].running);
        assert!(s[2].focused && s[2].windows == [1, 3]);
    }

    #[test]
    fn clicks_start_focus_cycle_and_minimize() {
        let windows = [win(1, "foot", true), win(2, "foot", false), win(3, "zed", false)];
        let s = slots(&[], &windows, &[], &[]);
        assert_eq!(click(&s[0], &windows), Click::Focus(2), "the focused app: its next window");
        assert_eq!(click(&s[1], &windows), Click::Focus(3));
        let one = [win(5, "zed", true)];
        let s = slots(&[], &one, &[], &[]);
        assert_eq!(click(&s[0], &one), Click::Minimize(5));
        let pinned = slots(&["zed".to_string()], &[], &[], &[]);
        assert_eq!(click(&pinned[0], &[]), Click::Launch);
    }

    /// Without a window list the compositor's clients mark what runs.
    #[test]
    fn without_a_window_list_the_compositor_clients_show_what_runs() {
        let s = slots(&["firefox".to_string()], &[], &[("firefox".into(), true), ("foot".into(), false)], &[]);
        assert!(s[0].running && s[0].focused);
        assert_eq!(s[1].key, "foot");
        assert_eq!(click(&s[1], &[]), Click::Launch, "a click starts (and so raises) the app");
    }
}
