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
//!
//! Its look is `[plugins.settings.taskbar]` ([`TaskbarSettings`]): the
//! buttons straight on the bar or on a tray of the dock's glass (classic or
//! liquid, with grain), the running mark, how the focused app stands out,
//! names beside the icons, one button per app or per window, window counts
//! and coral for apps asking for attention. Without the table it looks as
//! it always did.

use std::cell::RefCell;
use std::sync::{Arc, Weak};

use gpui::{FontWeight, InteractiveElement as _, IntoElement, MouseButton, ParentElement, SharedString, StatefulInteractiveElement as _, Styled, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::taskbar::{FocusStyle, TaskbarIndicator, TaskbarLabels, TaskbarMaterial, TaskbarSettings};
use mandara_core::ToGpui;
use mandara_plugins::{PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::toplevels::{Toplevel, ToplevelCommand};
use mandara_theme::{Color, MandaraPalette};

use super::view::domain_view_observing;
use crate::apps::App as DesktopApp;
use crate::bidi::Bidi as _;
use crate::hub::{Domains, hub};
use crate::kit::text;

pub const ID: &str = mandara_config::bar_styles::TASKBAR;

/// The button round an icon, the running mark under it, logical px.
const BUTTON_PAD_PX: f32 = 5.0;
const MARK_W_PX: f32 = 6.0;
const MARK_W_FOCUSED_PX: f32 = 14.0;
const MARK_H_PX: f32 = 2.5;
/// The mark's gap to the button's bottom edge.
const MARK_INSET_PX: f32 = 1.0;
/// Dots: size and gap; Glow: its light `(alpha focused, alpha running,
/// blur)`, small, as MangoWM blurs alpha into halos.
const DOT_PX: f32 = 3.0;
const DOT_GAP_PX: f32 = 2.0;
const GLOW: (f32, f32, f32) = (0.7, 0.35, 4.0);
/// Most indicator dots per app.
const MAX_DOTS: usize = 3;
/// A labelled button: the gap between icon and name, and the room after it.
const LABEL_GAP_PX: f32 = 6.0;
const LABEL_END_PX: f32 = 9.0;
/// The window count: its size, inset from the button's corner and text.
const COUNT_PX: f32 = 12.0;
const COUNT_INSET_PX: f32 = 1.0;
const COUNT_TEXT_PX: f32 = 8.5;
const COUNT_PAD_PX: f32 = 3.0;
/// The glass tray's padding round the buttons (taken from the buttons'
/// own padding, so the bar's height never changes).
const TRAY_PAD_PX: f32 = 2.0;
/// Most buttons shown (the rest are left out, like a full taskbar).
const MAX_SLOTS: usize = 24;
/// State layers: hover, pressed, the focused tint and accent wash, and an
/// urgent app's coral wash.
const HOVER_ALPHA: f32 = 0.10;
const PRESS_ALPHA: f32 = 0.16;
const FOCUSED_ALPHA: f32 = 0.16;
const ACCENT_ALPHA: f32 = 0.18;
const URGENT_ALPHA: f32 = 0.16;
/// The Raised focus: a glass key `(alpha dark, alpha light)` on a tight
/// contact shadow.
const RAISED_ALPHA: (f32, f32) = (0.13, 0.92);
const RAISED_SHADOW: (f32, f32, f32) = (0.22, 1.0, 2.0);

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
    /// The window's title, when the button is one window of an app.
    pub title: Option<String>,
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
        out.push(Slot { key: p.clone(), icon, pinned: true, windows: mine.iter().map(|w| w.id).collect(), running: !mine.is_empty(), focused: mine.iter().any(|w| w.activated), title: None });
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
        out.push(Slot { key: w.app_id.clone(), icon: w.app_id.clone(), pinned: false, windows: vec![w.id], running: true, focused: w.activated, title: None });
    }
    // No window list: the compositor's clients say what runs.
    if windows.is_empty() {
        for (app_id, focused) in clients.iter().filter(|(a, _)| !a.is_empty() && !crate::windows::is_shell(a)) {
            if let Some(s) = out.iter_mut().find(|s| crate::windows::same_app(&s.key, app_id) || apps.iter().any(|a| a.id.eq_ignore_ascii_case(&s.key) && super::dock::model::belongs(app_id, &s.key, Some(a)))) {
                s.running = true;
                s.focused |= *focused;
            } else if out.len() < MAX_SLOTS {
                out.push(Slot { key: app_id.clone(), icon: app_id.clone(), pinned: false, windows: Vec::new(), running: true, focused: *focused, title: None });
            }
        }
    }
    out
}

/// One button per window (`group_windows = false`): an app's windows split
/// into buttons of their own, oldest first, so they keep their places as
/// focus moves; apps without windows keep their single button.
pub fn ungroup(slots: Vec<Slot>, windows: &[Toplevel]) -> Vec<Slot> {
    let mut out = Vec::with_capacity(slots.len());
    for slot in slots {
        if slot.windows.len() < 2 {
            out.push(slot);
            continue;
        }
        let mut ids = slot.windows.clone();
        ids.sort_unstable();
        for (i, id) in ids.into_iter().enumerate() {
            if out.len() >= MAX_SLOTS {
                break;
            }
            let w = windows.iter().find(|w| w.id == id);
            out.push(Slot { windows: vec![id], focused: w.is_some_and(|w| w.activated), title: w.map(|w| w.title.clone()).filter(|t| !t.is_empty()), pinned: slot.pinned && i == 0, ..slot.clone() });
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

/// Whether `slot` shows its name under `labels`.
pub fn labelled(slot: &Slot, labels: TaskbarLabels) -> bool {
    match labels {
        TaskbarLabels::None => false,
        TaskbarLabels::Focused => slot.focused,
        TaskbarLabels::Running => slot.running,
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

/// The taskbar's settings, parsed once per config snapshot (the snapshot
/// held weakly, so an old config is never kept alive for it).
#[derive(Default)]
struct Settings(RefCell<Option<(Weak<ShellConfig>, Arc<TaskbarSettings>)>>);

impl Settings {
    fn of(&self, cfg: &Arc<ShellConfig>) -> Arc<TaskbarSettings> {
        let mut cached = self.0.borrow_mut();
        if let Some((owner, s)) = cached.as_ref()
            && owner.upgrade().is_some_and(|o| Arc::ptr_eq(&o, cfg))
        {
            return s.clone();
        }
        let s = Arc::new(TaskbarSettings::from_plugins(&cfg.plugins));
        *cached = Some((Arc::downgrade(cfg), s.clone()));
        s
    }
}

/// The running mark of a button `side` px tall whose icon centre sits
/// `centre` px from its start.
fn indicator(s: &TaskbarSettings, slot: &Slot, urgent: bool, centre: f32, palette: &MandaraPalette) -> Option<gpui::AnyElement> {
    if !slot.running || s.indicator == TaskbarIndicator::None {
        return None;
    }
    let tint = if urgent {
        palette.urgent_coral
    } else if slot.focused {
        palette.primary
    } else {
        palette.on_surface_variant
    };
    let mark = |w: f32, h: f32| div().flex_none().w(px(w)).h(px(h)).rounded_full().bg(tint.hsla());
    let row = match s.indicator {
        TaskbarIndicator::Line => mark(if slot.focused { MARK_W_FOCUSED_PX } else { MARK_W_PX }, MARK_H_PX).into_any_element(),
        TaskbarIndicator::Dot => mark(DOT_PX, DOT_PX).into_any_element(),
        TaskbarIndicator::Dots => div().flex_none().flex().gap(px(DOT_GAP_PX)).children((0..slot.windows.len().clamp(1, MAX_DOTS)).map(|_| mark(DOT_PX, DOT_PX))).into_any_element(),
        TaskbarIndicator::Glow => {
            let alpha = if slot.focused || urgent { GLOW.0 } else { GLOW.1 };
            mark(if slot.focused { MARK_W_FOCUSED_PX } else { MARK_W_PX }, MARK_H_PX).shadow(vec![crate::plugins::panel_fx::glow(tint.with_alpha(alpha), GLOW.2, 0.0, 0.0)]).into_any_element()
        }
        TaskbarIndicator::None => return None,
    };
    // Centred under the icon (the mark's own width is unknown here: a
    // zero-width centring box does it), from the button's reading start.
    let at = div().absolute().bottom(px(MARK_INSET_PX));
    let at = at.inset_start(px(centre));
    Some(at.w(px(0.0)).flex().justify_center().child(row).into_any_element())
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, mandara_i18n::tk("Taskbar"), mandara_i18n::tk("Your pinned and running apps in the bar"), Some(Zone::Center)).opt_in(), |window, cx, _| {
        let windows = crate::windows::init(cx);
        let catalog = crate::apps::changes::init(cx);
        let follow = windows.clone();
        let settings = Settings::default();
        let bar = domain_view_observing(
            Domains::CLIENTS.union(Domains::FOCUS).union(Domains::NAVIGATION),
            None,
            move |cx| Some(cx.observe(&follow, |_, _, cx| cx.notify())),
            move |palette, window, cx| {
                let cx_ref: &gpui::App = cx;
                let list = windows.read(cx_ref).list();
                let apps = catalog.read(cx_ref).apps.clone();
                let cfg = crate::state::config(cx_ref).read(cx_ref).snapshot();
                let s = settings.of(&cfg);
                let hub = hub(cx_ref).read(cx_ref);
                let clients: Vec<(String, bool)> = if list.is_empty() { hub.clients.iter().map(|c| (c.app_id.clone(), c.focused)).collect() } else { Vec::new() };
                let urgent_ids = if s.urgent { super::dock::model::urgent_apps(&hub.clients, &hub.navigation) } else { Vec::new() };
                let mut slots = slots(&cfg.dock.pinned, list, &clients, &apps);
                if !s.group_windows {
                    slots = ungroup(slots, list);
                }
                let monochrome = cfg.appearance.bar_icons == mandara_config::IconStyle::Monochrome;
                let icon = crate::style::bar_icon(s.icon_size);
                // On a tray the buttons give up its padding: the bar keeps its height.
                let tray = s.material != TaskbarMaterial::None;
                let pad = if tray { BUTTON_PAD_PX - TRAY_PAD_PX } else { BUTTON_PAD_PX };
                let side = icon + pad * 2.0;
                let tray_h = side + if tray { TRAY_PAD_PX * 2.0 } else { 0.0 };
                let tray_r = radius(cfg.bar.item_shape, tray_h);
                let radius = if tray { crate::style::concentric(tray_r, TRAY_PAD_PX) } else { radius(cfg.bar.item_shape, side) };
                let accent = crate::plugins::panel_fx::readable(palette.primary, palette);
                let raised_alpha = if palette.is_dark { RAISED_ALPHA.0 } else { RAISED_ALPHA.1 };
                let raised_fill = if palette.is_dark { palette.on_surface.with_alpha(raised_alpha) } else { Color::oklch(1.0, 0.0, 0.0).with_alpha(raised_alpha) };
                let mut raised_shadow = crate::plugins::panel_fx::rim(palette);
                raised_shadow.push(gpui::BoxShadow { color: crate::plugins::panel_fx::shadow_ink(palette).with_alpha(RAISED_SHADOW.0).hsla(), offset: gpui::point(px(0.0), px(RAISED_SHADOW.1)), blur_radius: px(RAISED_SHADOW.2), spread_radius: px(0.0), inset: false });
                let buttons = slots.into_iter().enumerate().map(|(i, slot)| {
                    let urgent = !slot.focused && urgent_ids.iter().any(|u| crate::windows::same_app(&slot.key, u) || crate::windows::same_app(&slot.icon, u));
                    let label: Option<SharedString> = labelled(&slot, s.labels).then(|| {
                        slot.title.clone().or_else(|| apps.iter().find(|a| a.id.eq_ignore_ascii_case(&slot.key)).or_else(|| crate::windows::desktop_entry_for(&slot.key, &apps)).map(|a| a.name.clone())).unwrap_or_else(|| slot.key.clone()).into()
                    });
                    let count = (s.window_count && slot.windows.len() >= 2).then_some(slot.windows.len());
                    let labelled = label.is_some();
                    let (key, middle_key) = (slot.key.clone(), slot.key.clone());
                    let windows_click = windows.clone();
                    let mark = indicator(&s, &slot, urgent, if labelled { pad + icon / 2.0 } else { side / 2.0 }, palette);
                    let focus_fill = match s.focus_style {
                        _ if !slot.focused => None,
                        FocusStyle::Tint => Some(palette.on_surface.with_alpha(FOCUSED_ALPHA)),
                        FocusStyle::Raised => Some(raised_fill),
                        FocusStyle::Accent => Some(accent.with_alpha(ACCENT_ALPHA)),
                        FocusStyle::None => None,
                    };
                    div()
                        .id(("taskbar-slot", i))
                        .relative()
                        .h(px(side))
                        .min_w(px(side))
                        .flex_none()
                        .row()
                        .items_center()
                        .rounded(px(radius))
                        .when(label.is_none(), |el| el.w(px(side)).justify_center())
                        .when(label.is_some(), |el| el.max_w(px(side + s.label_width)).pad_start(px(pad)).pad_end(px(LABEL_END_PX)).gap(px(LABEL_GAP_PX)))
                        .when_some(focus_fill, |el, c| el.bg(c.hsla()))
                        .when(slot.focused && s.focus_style == FocusStyle::Raised, |el| el.shadow(raised_shadow.clone()))
                        .when(urgent, |el| el.bg(palette.urgent_coral.with_alpha(URGENT_ALPHA).hsla()))
                        .hover(|st| st.bg(palette.on_surface.with_alpha(HOVER_ALPHA).hsla()))
                        .active(|st| st.bg(palette.on_surface.with_alpha(PRESS_ALPHA).hsla()))
                        .opacity(if slot.running || !slot.pinned { 1.0 } else { cfg.dock.inactive_opacity.clamp(0.4, 1.0) })
                        .child(div().flex_none().child(super::workspaces::app_icon(&slot.icon, icon, palette.on_surface, monochrome, palette)))
                        .when_some(label, |el, l| {
                            el.child(div().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).font_weight(if slot.focused { FontWeight::MEDIUM } else { FontWeight::NORMAL }).text_color(if slot.focused { palette.on_surface.hsla() } else { palette.on_surface_variant.hsla() }).child(l))
                        })
                        .children(mark)
                        .when_some(count, |el, n| {
                            // Beside a name it follows the name; on an icon it sits in the corner.
                            let badge = if labelled { div().flex_none() } else { div().absolute().top(px(COUNT_INSET_PX)).inset_end(px(COUNT_INSET_PX)) };
                            el.child(
                                badge
                                    .min_w(px(COUNT_PX))
                                    .h(px(COUNT_PX))
                                    .px(px(COUNT_PAD_PX))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(accent.hsla())
                                    .text_color(palette.on_primary.hsla())
                                    .text_size(px(COUNT_TEXT_PX))
                                    .font_weight(FontWeight::BOLD)
                                    .font_features(crate::fonts::tabular_nums())
                                    .child(SharedString::from(n.to_string())),
                            )
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
                });
                let row = div().row().items_center().gap(px(s.spacing));
                if !tray {
                    return row.children(buttons).into_any_element();
                }
                // A tray of the dock's glass (classic or liquid) under the buttons.
                let d = &cfg.dock;
                let material = crate::components::materials::Material {
                    kind: if s.material == TaskbarMaterial::Liquid { mandara_config::dock::DockMaterial::Liquid } else { mandara_config::dock::DockMaterial::Classic },
                    opacity: d.opacity.clamp(0.0, 1.0),
                    tint: d.tint,
                    gloss: d.gloss,
                    refraction: d.refraction,
                    rim: true,
                    // The bar casts its own shadow.
                    shadow: false,
                    grain: s.grain(),
                };
                let plate = crate::components::materials::dress(row.relative().p(px(TRAY_PAD_PX)), &material, tray_h, tray_r, palette, window.scale_factor(), Some(window.window_handle()), cx);
                plate.children(buttons).into_any_element()
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

    /// One button per window: stable (oldest first), each with its own
    /// title, focus and click; pinned apps without windows stay one button.
    #[test]
    fn ungrouped_windows_get_buttons_of_their_own() {
        let windows = [win(9, "foot", false), win(3, "foot", true), win(5, "zed", false)];
        let grouped = slots(&["firefox".to_string()], &windows, &[], &[]);
        let s = ungroup(grouped, &windows);
        let keys: Vec<(&str, &[u64])> = s.iter().map(|s| (s.key.as_str(), s.windows.as_slice())).collect();
        assert_eq!(keys, [("firefox", &[][..]), ("foot", &[3][..]), ("foot", &[9][..]), ("zed", &[5][..])]);
        assert!(s[1].focused && !s[2].focused, "only the focused window's button");
        assert_eq!(s[2].title.as_deref(), Some("foot 9"));
        assert_eq!(click(&s[1], &windows), Click::Minimize(3));
        assert_eq!(click(&s[2], &windows), Click::Focus(9));
    }

    #[test]
    fn labels_follow_the_setting() {
        let windows = [win(1, "foot", true), win(2, "zed", false)];
        let s = slots(&["firefox".to_string()], &windows, &[], &[]);
        let shown = |l: TaskbarLabels| s.iter().filter(|x| labelled(x, l)).count();
        assert_eq!((shown(TaskbarLabels::None), shown(TaskbarLabels::Focused), shown(TaskbarLabels::Running)), (0, 1, 2));
    }
}
