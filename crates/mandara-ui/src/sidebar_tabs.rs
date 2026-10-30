//! The sidebar's tabs: the user's tabs of plugin cards
//! (`mandara_config::sidebar`) plus the built-in Notifications tab.
//!
//! * One live instance per plugin backs every tab that shows it (only the
//!   visible tab renders, so a card appears once per frame), and instances
//!   survive layout edits — Studio drags never reset a card's state.
//! * Card lists are cached per tab and rebuilt only when the layout changes;
//!   only the visible tab's cards render and tick.
//! * The last tab is remembered in `$XDG_STATE_HOME/mandara/sidebar.toml`
//!   (unless "Always open on the first tab"), and each tab keeps its own
//!   scroll position.
//! * The strip: right-click a tab for Rename / Change icon / Move / Delete;
//!   "+" makes a tab in place; Ctrl+Tab, Ctrl+Shift+Tab and Ctrl+1…9 switch.
//!   Tabs past the strip's width scroll sideways (wheel, touchpad,
//!   click-drag) under edge fades, the switched-to tab scrolls into view on
//!   a spring, and "+" stays pinned after them. Idle tabs shrink to their
//!   icon only when every label wouldn't fit.
//! * Switching slides the cards in from the side of the new tab (mirrored
//!   in right-to-left languages) on a spring — instant with reduced motion.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, AnyView, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, ScrollHandle, StatefulInteractiveElement, Styled, WeakEntity, Window, div, prelude::FluentBuilder as _,
    px,
};
use mandara_config::sidebar::{NOTIFICATIONS_TAB, OpenTab, SidebarState, SidebarTab};
use mandara_config::{ShellConfig, SidebarConfig};
use mandara_core::{SpringConfig, SpringValue, ToGpui};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PanelCx, PluginRegistry};
use mandara_theme::MandaraPalette;
use serde_json::json;

use crate::bidi::Bidi as _;
use crate::icons::{any_icon, icon};
use crate::kit::{text, tokens};
use crate::state::{config, shell, update_config};
use crate::text_field::TextField;

/// Built-in plugins that have a sidebar card (third-party plugins all may).
pub const SIDEBAR_WIDGETS: &[&str] = &["quick-settings", "weather", "notifications", "media", "pomodoro", "todo", "notes", "audio", "resources", "prayer-times"];
/// Icons offered for a tab (Mandara's own set).
pub const TAB_ICONS: &[&str] = &[
    "grid", "tiles", "bell", "calendar", "list-check", "note", "music", "headphones", "cpu", "monitor", "cloud-sun", "coffee", "heart",
    "bolt", "zap", "globe", "folder", "terminal", "clock", "timer", "leaf", "flame", "user", "sliders", "image", "package", "server",
    "shield", "key", "box", "pencil", "waves", "crescent", "sun", "moon",
];

/// How far switched-in cards travel, logical px.
const SLIDE_PX: f32 = 28.0;
/// Below this progress the outgoing tab is considered gone.
const SETTLED: f32 = 0.999;
const TAB_SPRING: SpringConfig = SpringConfig::GENTLE;
// Strip.
const TAB_PAD_X_PX: f32 = 12.0;
const TAB_COMPACT_PAD_X_PX: f32 = 9.0;
const TAB_ICON_PX: f32 = 15.0;
const TAB_GAP_PX: f32 = 2.0;
const STRIP_PAD_PX: f32 = 3.0;
const TRACK_ALPHA: f32 = 0.05;
const HOVER_ALPHA: f32 = 0.06;
const BADGE_PX: f32 = 17.0;
const BADGE_PAD_PX: f32 = 5.0;
const BADGE_TEXT_PX: f32 = 10.0;
const BADGE_MAX: usize = 99;
const RENAME_W_PX: f32 = 120.0;
/// Edge fades over tabs scrolled out of the strip.
const STRIP_FADE_PX: f32 = 22.0;
/// A left-to-right gradient (CSS degrees).
const FADE_ANGLE_DEG: f32 = 90.0;
/// Room kept beside a tab scrolled into view (its neighbour peeks out).
const SCROLL_MARGIN_PX: f32 = 18.0;
/// Scroll differences below this are none.
const SCROLL_EPSILON_PX: f32 = 0.5;
/// A press moving further than this pans the strip instead of clicking.
const PAN_THRESHOLD_PX: f32 = 4.0;
const STRIP_SPRING: SpringConfig = SpringConfig::new(300.0, 30.0, 1.0);
/// Spare width a label row needs before idle tabs shrink to icons
/// (rounding of measured text).
const LABEL_SLACK_PX: f32 = 2.0;
// Menu.
const MENU_PAD_PX: f32 = 12.0;
const MENU_RADIUS_PX: f32 = 16.0;
const MENU_ITEM_H_PX: f32 = 32.0;
const MENU_ITEM_PAD_X_PX: f32 = 10.0;
const MENU_ITEM_RADIUS_PX: f32 = 9.0;
const MENU_ICON_PX: f32 = 14.0;
const ICON_TILE_PX: f32 = 34.0;
const ICON_TILE_GLYPH_PX: f32 = 17.0;
const SCRIM_ALPHA: f32 = 0.18;
// Empty tab.
const EMPTY_ICON_PX: f32 = 28.0;
const EMPTY_PAD_Y_PX: f32 = 28.0;
const EMPTY_BUTTON_H_PX: f32 = 30.0;
const EMPTY_BUTTON_PAD_X_PX: f32 = 14.0;
const SELECTED_ALPHA: f32 = 0.14;

/// A tab-menu action on the owning view.
type MenuAction<V> = Box<dyn Fn(&mut V, &mut Window, &mut Context<V>)>;
/// An edit of the tab at an index.
type TabEdit = Box<dyn Fn(&mut SidebarConfig, usize)>;
type SharedTabEdit = Rc<dyn Fn(&mut SidebarConfig, usize)>;

/// Which tab is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sel {
    /// A user tab, by id.
    Tab(String),
    Notifications,
}

/// The tab menu (right-click on a tab).
#[derive(Clone, Debug, PartialEq, Eq)]
enum TabMenu {
    Actions(String),
    Icons(String),
    Confirm(String),
}

/// `sidebar tab open` requests: a generation each shell view acts on once.
#[derive(Default)]
struct OpenRequest {
    generation: u64,
    key: String,
}
impl gpui::Global for OpenRequest {}

/// The last tab, mirrored from the state file (read once).
struct LastTab(String);
impl gpui::Global for LastTab {}

fn last_tab(cx: &mut App) -> String {
    if !cx.has_global::<LastTab>() {
        cx.set_global(LastTab(SidebarState::load().last_tab));
    }
    cx.global::<LastTab>().0.clone()
}

/// Whether the sidebar's selected tab is the Notifications tab.
pub fn notifications_selected(cx: &App) -> bool {
    cx.try_global::<LastTab>().is_some_and(|l| l.0 == NOTIFICATIONS_TAB)
}

/// Remembers `id` as the last tab (written off the UI thread).
fn remember(id: &str, cx: &mut App) {
    if cx.try_global::<LastTab>().is_some_and(|l| l.0 == id) {
        return;
    }
    cx.set_global(LastTab(id.to_string()));
    crate::toasts::sync_list(cx);
    let state = SidebarState { last_tab: id.to_string() };
    cx.background_executor()
        .spawn(async move {
            if let Err(err) = state.save() {
                tracing::warn!("saving the sidebar state: {err:#}");
            }
        })
        .detach();
}

/// A tab's name as shown (the shipped "Overview" follows the language).
pub fn tab_label(tab: &SidebarTab) -> String {
    if tab.has_default_name() { mandara_i18n::t("Overview").to_string() } else { tab.name.clone() }
}

/// Splits `--flag value` out of command words.
pub fn take_flag(words: &[String], flag: &str) -> (Vec<String>, Option<String>) {
    let mut out = Vec::with_capacity(words.len());
    let mut value = None;
    let mut it = words.iter();
    while let Some(w) = it.next() {
        if w == flag {
            value = it.next().cloned();
        } else if let Some(v) = w.strip_prefix(flag).and_then(|r| r.strip_prefix('=')) {
            value = Some(v.to_string());
        } else {
            out.push(w.clone());
        }
    }
    (out, value)
}

/// Every tab of the strip in order: the user's, then Notifications.
fn order(cfg: &SidebarConfig, notifications: bool) -> Vec<Sel> {
    let mut v: Vec<Sel> = cfg.tabs.iter().map(|t| Sel::Tab(t.id.clone())).collect();
    if notifications {
        v.push(Sel::Notifications);
    }
    v
}

/// The sidebar's tab state, owned by each shell view.
pub struct SidebarTabs {
    sel: Sel,
    /// Built cards per tab, in strip order (shared instances).
    cards: Vec<(String, Vec<AnyView>)>,
    /// Live card instances by plugin id, with their factory generation.
    pool: HashMap<String, (u64, AnyView)>,
    scroll: HashMap<String, ScrollHandle>,
    /// The cards' overlay scroll bar (one per sidebar, whichever tab shows).
    bar: std::rc::Rc<crate::scrollbar::ScrollbarState>,
    anim: SpringValue<f32>,
    /// -1 / +1: the side new cards slide in from.
    dir: f32,
    menu: Option<TabMenu>,
    renaming: Option<(String, Entity<TextField>)>,
    /// The next visible frame picks the opening tab.
    pending_open: bool,
    seen_request: u64,
    /// The strip's sideways scroll (made when the strip first shows).
    strip_scroll: Option<ScrollHandle>,
    /// Scroll-into-view of the active tab, logical px from the left.
    strip_pos: SpringValue<f32>,
    strip_animating: bool,
    /// Scroll the active tab into view after this many frames (animated?).
    reveal: Option<(bool, u8)>,
    /// Click-drag panning: pointer x and scroll position at the press.
    pan: Option<(f32, f32)>,
    /// The current press panned the strip (its click doesn't switch).
    panned: bool,
    /// Measured label widths and the key they were measured for.
    label_widths: (u64, Vec<f32>),
}

impl Default for SidebarTabs {
    fn default() -> Self {
        Self::new()
    }
}

impl SidebarTabs {
    pub fn new() -> Self {
        let mut anim = SpringValue::new(1.0, TAB_SPRING);
        anim.snap_to(1.0);
        Self {
            sel: Sel::Tab(String::new()),
            cards: Vec::new(),
            pool: HashMap::new(),
            scroll: HashMap::new(),
            bar: crate::scrollbar::ScrollbarState::new(),
            anim,
            dir: 1.0,
            menu: None,
            renaming: None,
            pending_open: true,
            seen_request: 0,
            strip_scroll: None,
            strip_pos: SpringValue::new(0.0, STRIP_SPRING),
            strip_animating: false,
            reveal: None,
            pan: None,
            panned: false,
            label_widths: (0, Vec::new()),
        }
    }

    /// Whether the cached cards still match `tabs` (checked every frame,
    /// without cloning).
    pub fn matches(&self, tabs: &[SidebarTab], cached: &[(String, Vec<String>)]) -> bool {
        cached.len() == tabs.len() && cached.iter().zip(tabs).all(|((id, cards), t)| *id == t.id && *cards == t.cards)
    }

    /// The layout key cached alongside the cards.
    pub fn key(tabs: &[SidebarTab]) -> Vec<(String, Vec<String>)> {
        tabs.iter().map(|t| (t.id.clone(), t.cards.clone())).collect()
    }

    /// Rebuilds the per-tab card lists, reusing live instances. `enabled`
    /// says whether a plugin is on; `skip` is left out (the notifications
    /// card while it has its own tab).
    pub fn rebuild(&mut self, tabs: &[SidebarTab], enabled: &dyn Fn(&str) -> bool, skip: Option<&str>, base_cx: &PanelCx, window: &mut Window, cx: &mut App) {
        let mut pool = std::mem::take(&mut self.pool);
        let mut next_pool = HashMap::new();
        self.cards = tabs
            .iter()
            .map(|tab| {
                let cards = tab
                    .cards
                    .iter()
                    .filter(|id| enabled(id) && skip != Some(id.as_str()))
                    .filter_map(|id| {
                        if let Some((_, view)) = next_pool.get(id) {
                            return Some(AnyView::clone(view));
                        }
                        let (build, generation) = cx.global::<PluginRegistry>().get(id).map(|f| (f.build.clone(), f.generation))?;
                        let view = match pool.remove(id) {
                            Some((g, view)) if g == generation => view,
                            _ => mandara_plugins::guard(&format!("plugin `{id}` startup"), || build(window, cx, base_cx.fork()))?.sidebar?,
                        };
                        next_pool.insert(id.clone(), (generation, view.clone()));
                        Some(view)
                    })
                    .collect();
                (tab.id.clone(), cards)
            })
            .collect();
        self.pool = next_pool;
        self.scroll.retain(|id, _| tabs.iter().any(|t| &t.id == id));
    }

    /// The sidebar finished hiding: the next opening picks its tab again,
    /// and every tab starts from its top (a list left scrolled opened with
    /// its first cards out of sight).
    pub fn on_hidden(&mut self) {
        self.pending_open = true;
        for list in self.scroll.values() {
            list.set_offset(gpui::point(px(0.0), px(0.0)));
        }
        self.menu = None;
        self.renaming = None;
        self.pan = None;
        self.strip_animating = false;
    }

    /// Advances the switch animation and the strip's scroll-into-view.
    pub fn step(&mut self, dt: f32) -> bool {
        let mut moving = self.anim.step(dt);
        if self.strip_animating {
            let more = self.strip_pos.step(dt);
            if let Some(h) = &self.strip_scroll {
                h.set_offset(gpui::point(px(-self.strip_pos.value()), px(0.0)));
            }
            self.strip_animating = more;
            moving |= more;
        }
        moving
    }

    /// The tab to show this frame (resolving openings, `sidebar tab open`
    /// requests and tabs deleted under it).
    pub fn resolve(&mut self, cfg: &SidebarConfig, notifications: bool, cx: &mut App) -> Sel {
        if let Some(req) = cx.try_global::<OpenRequest>()
            && req.generation != self.seen_request
        {
            self.seen_request = req.generation;
            let key = req.key.clone();
            if key == NOTIFICATIONS_TAB && notifications {
                self.select(Sel::Notifications, cfg, notifications, cx);
            } else if let Some(i) = cfg.tab_index(&key) {
                self.select(Sel::Tab(cfg.tabs[i].id.clone()), cfg, notifications, cx);
            }
            self.pending_open = false;
        }
        if self.pending_open {
            self.pending_open = false;
            let last = last_tab(cx);
            self.sel = match cfg.opening_tab(&last, notifications) {
                OpenTab::Notifications => Sel::Notifications,
                OpenTab::Tab(i) => Sel::Tab(cfg.tabs.get(i).map(|t| t.id.clone()).unwrap_or_default()),
            };
            self.anim.snap_to(1.0);
            // Opening shows the tab at once (right-to-left: from the end).
            self.reveal = Some((false, 1));
        }
        let valid = match &self.sel {
            Sel::Notifications => notifications,
            Sel::Tab(id) => cfg.tabs.iter().any(|t| &t.id == id),
        };
        if !valid {
            self.sel = Sel::Tab(cfg.tabs.first().map(|t| t.id.clone()).unwrap_or_default());
        }
        self.sel.clone()
    }

    /// Switches tab (animated, remembered).
    fn select(&mut self, to: Sel, cfg: &SidebarConfig, notifications: bool, cx: &mut App) {
        if to == self.sel {
            return;
        }
        let tabs = order(cfg, notifications);
        let from = tabs.iter().position(|s| *s == self.sel).unwrap_or(0);
        let next = tabs.iter().position(|s| *s == to).unwrap_or(0);
        self.dir = if next >= from { 1.0 } else { -1.0 };
        self.reveal = Some((true, 1));
        self.anim.snap_to(0.0);
        self.anim.set_target(1.0);
        self.menu = None;
        if to == Sel::Notifications {
            crate::plugins::notifications::send(cx, mandara_services::NotificationCommand::MarkAllRead);
        }
        remember(
            match &to {
                Sel::Notifications => NOTIFICATIONS_TAB,
                Sel::Tab(id) => id,
            },
            cx,
        );
        self.sel = to;
    }

    /// Ctrl+Tab / Ctrl+Shift+Tab / Ctrl+1…9 inside the sidebar, and Escape
    /// for an open tab menu. Returns whether the key was used.
    pub fn handle_key(&mut self, e: &KeyDownEvent, cfg: &SidebarConfig, notifications: bool, cx: &mut App) -> bool {
        let k = &e.keystroke;
        if k.key == "escape" && (self.menu.is_some() || self.renaming.is_some()) {
            self.menu = None;
            self.renaming = None;
            return true;
        }
        if !k.modifiers.control || k.modifiers.alt {
            return false;
        }
        let tabs = order(cfg, notifications);
        let at = tabs.iter().position(|s| *s == self.sel).unwrap_or(0);
        let to = if k.key == "tab" {
            let n = tabs.len();
            if k.modifiers.shift { (at + n - 1) % n } else { (at + 1) % n }
        } else if let Some(d) = k.key.parse::<usize>().ok().filter(|d| (1..=9).contains(d)) {
            if d > tabs.len() {
                return true;
            }
            d - 1
        } else {
            return false;
        };
        self.select(tabs[to].clone(), cfg, notifications, cx);
        true
    }

    /// The visible tab's cards (empty for Notifications).
    pub fn cards(&self, sel: &Sel) -> &[AnyView] {
        match sel {
            Sel::Tab(id) => self.cards.iter().find(|(t, _)| t == id).map_or(&[], |(_, c)| c.as_slice()),
            Sel::Notifications => &[],
        }
    }

    /// The scroll position of `sel`'s card list.
    pub fn scroll(&mut self, sel: &Sel) -> ScrollHandle {
        let key = match sel {
            Sel::Tab(id) => id.as_str(),
            Sel::Notifications => NOTIFICATIONS_TAB,
        };
        self.scroll.entry(key.to_string()).or_default().clone()
    }

    /// The cards' overlay scroll bar state.
    pub fn scrollbar(&self) -> std::rc::Rc<crate::scrollbar::ScrollbarState> {
        self.bar.clone()
    }

    /// `(x offset, opacity)` of the switching-in cards.
    pub fn transition(&self) -> (f32, f32) {
        let p = self.anim.value().clamp(0.0, 1.0);
        if p >= SETTLED {
            return (0.0, 1.0);
        }
        let side = if crate::bidi::rtl() { -self.dir } else { self.dir };
        ((1.0 - p) * SLIDE_PX * side, p)
    }

    /// The tab strip: the tabs scroll sideways inside their track (wheel,
    /// touchpad, click-drag), with edge fades where more tabs hide, and "+"
    /// stays pinned after the track. `row_w` is the width the strip may
    /// use (the sidebar's header minus its buttons). `get` reaches this
    /// state inside the owning view.
    #[allow(clippy::too_many_arguments)]
    pub fn strip<V: 'static>(
        &mut self,
        cfg: &ShellConfig,
        sel: &Sel,
        notifications: bool,
        unread: usize,
        tab_h: f32,
        row_w: f32,
        palette: &MandaraPalette,
        get: fn(&mut V) -> &mut SidebarTabs,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> AnyElement {
        let sc = &cfg.sidebar;
        let rtl = crate::bidi::rtl();
        let total = sc.tabs.len() + usize::from(notifications);
        let mut labels: Vec<String> = sc.tabs.iter().map(tab_label).collect();
        if notifications {
            labels.push(mandara_i18n::t("Notifications").to_string());
        }
        let family = crate::fonts::fonts(cx).ui.clone();
        let widths = self.label_widths(&labels, &family, window);
        let badge_at = notifications.then_some(sc.tabs.len()).filter(|_| unread > 0);
        let active_ix = order(sc, notifications).iter().position(|s| s == sel);
        let full_w = strip_width(widths, badge_at, active_ix);
        let avail = strip_viewport(row_w, tab_h, sc.add_tab_button);
        let compact = total > 1 && full_w > avail;
        let mut items: Vec<AnyElement> = Vec::with_capacity(total);
        let tab_button = |key: Sel, ic: &str, label: String, badge: usize, ix: usize, cx: &mut Context<V>| -> AnyElement {
            let active = *sel == key;
            let show_label = active || !compact;
            let (fg, bg) = if active { (palette.on_primary, Some(palette.primary)) } else { (palette.on_surface_variant, None) };
            let menu_key = key.clone();
            div()
                .id(("sidebar-tab", ix))
                .h(px(tab_h))
                .px(px(if show_label { TAB_PAD_X_PX } else { TAB_COMPACT_PAD_X_PX }))
                .flex_none()
                .rounded_full()
                .row()
                .items_center()
                .gap(px(tokens::XS + TAB_GAP_PX))
                .cursor_pointer()
                .text_size(px(text::BODY_S))
                .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                .when_some(bg, |el, bg| el.bg(bg.hsla()).text_color(fg.hsla()))
                .when(!active, |el| el.text_color(fg.hsla()).hover(|s| s.bg(palette.on_surface.with_alpha(HOVER_ALPHA).hsla())))
                .child(any_icon(ic, TAB_ICON_PX, fg))
                .when(show_label, |el| el.child(label))
                .when(badge > 0 && !active, |el| {
                    el.child(
                        div()
                            .min_w(px(BADGE_PX))
                            .h(px(BADGE_PX))
                            .px(px(BADGE_PAD_PX))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(palette.primary.hsla())
                            .text_color(palette.on_primary.hsla())
                            .text_size(px(BADGE_TEXT_PX))
                            .font_weight(FontWeight::BOLD)
                            .font_features(crate::fonts::tabular_nums())
                            .child(badge.min(BADGE_MAX).to_string()),
                    )
                })
                .on_click(cx.listener(move |v, _: &gpui::ClickEvent, _, cx| {
                    // The press panned the strip: not a tab switch.
                    if std::mem::take(&mut get(v).panned) {
                        return;
                    }
                    let cfg = config(cx).read(cx).snapshot();
                    get(v).select(key.clone(), &cfg.sidebar, notifications, cx);
                    cx.notify();
                }))
                .when_some(match menu_key { Sel::Tab(id) => Some(id), Sel::Notifications => None }, |el, id| {
                    el.on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |v, _: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let t = get(v);
                            t.renaming = None;
                            t.menu = Some(TabMenu::Actions(id.clone()));
                            cx.notify();
                        }),
                    )
                })
                .into_any_element()
        };
        let mut labels = labels.into_iter();
        for (ix, tab) in sc.tabs.iter().enumerate() {
            let label = labels.next().unwrap_or_default();
            if let Some((id, field)) = &self.renaming
                && *id == tab.id
            {
                items.push(div().id(("sidebar-tab-rename", ix)).w(px(RENAME_W_PX)).flex_none().child(field.clone()).into_any_element());
                continue;
            }
            items.push(tab_button(Sel::Tab(tab.id.clone()), &tab.icon, label, 0, ix, cx));
        }
        if notifications {
            items.push(tab_button(Sel::Notifications, "bell", labels.next().unwrap_or_default(), unread, sc.tabs.len(), cx));
        }
        // The scroll container stays left to right (a reversed flex row
        // would overflow past its start, out of reach): right-to-left
        // languages get the tabs in reverse, scrolled to the right end.
        if rtl {
            items.reverse();
        }
        let visual_active = active_ix.map(|i| if rtl { total.saturating_sub(1).saturating_sub(i) } else { i });
        let handle = self.strip_scroll.get_or_insert_with(ScrollHandle::new).clone();
        let view = handle.bounds();
        let view_w = f32::from(view.size.width);
        let max = f32::from(handle.max_offset().x).max(0.0);
        let pos = (-f32::from(handle.offset().x)).clamp(0.0, max);
        // Bring the active tab into view one layout after it changed (its
        // label and its neighbours' widths change with it).
        if let Some((animate, wait)) = self.reveal {
            if wait > 0 {
                self.reveal = Some((animate, wait - 1));
                window.request_animation_frame();
            } else {
                self.reveal = None;
                if let Some(item) = visual_active.and_then(|i| handle.bounds_for_item(i)).filter(|_| view_w > 0.0) {
                    let start = f32::from(item.left() - view.left());
                    let end = f32::from(item.right() - view.left());
                    let target = scroll_into_view(start, end, view_w, pos, max, SCROLL_MARGIN_PX);
                    if (target - pos).abs() > SCROLL_EPSILON_PX {
                        if animate {
                            self.strip_pos.snap_to(pos);
                            self.strip_pos.set_target(target);
                            self.strip_animating = true;
                            window.request_animation_frame();
                        } else {
                            handle.set_offset(gpui::point(px(-target), px(0.0)));
                            window.request_animation_frame();
                        }
                    }
                }
            }
        }
        let overflows = max > SCROLL_EPSILON_PX;
        let edge = crate::shell::frame_color(palette).mix(palette.on_surface, TRACK_ALPHA);
        let fade = |left: bool| {
            let (from, to) = if left { (edge, edge.with_alpha(0.0)) } else { (edge.with_alpha(0.0), edge) };
            div()
                .absolute()
                .top(px(STRIP_PAD_PX))
                .bottom(px(STRIP_PAD_PX))
                .w(px(STRIP_FADE_PX))
                .when(left, |el| el.left(px(STRIP_PAD_PX)))
                .when(!left, |el| el.right(px(STRIP_PAD_PX)))
                .bg(gpui::linear_gradient(FADE_ANGLE_DEG, gpui::linear_color_stop(from.hsla(), 0.0), gpui::linear_color_stop(to.hsla(), 1.0)))
        };
        let scroller = div()
            .id("sidebar-tabs")
            .min_w(px(0.0))
            .ltr_row()
            .items_center()
            .gap(px(TAB_GAP_PX))
            .overflow_x_scroll()
            .track_scroll(&handle)
            .children(items);
        let pan_handle = handle.clone();
        let track = div()
            .id("sidebar-tabs-track")
            .relative()
            .min_w(px(0.0))
            .p(px(STRIP_PAD_PX))
            .rounded_full()
            .bg(palette.on_surface.with_alpha(TRACK_ALPHA).hsla())
            .flex()
            .child(scroller)
            .when(overflows && pos > SCROLL_EPSILON_PX, |el| el.child(fade(true)))
            .when(overflows && pos < max - SCROLL_EPSILON_PX, |el| el.child(fade(false)))
            // The tabs' own scroll maps the wheel sideways; while they
            // overflow, the wheel stays here instead of also scrolling the
            // cards under the header.
            .on_scroll(cx.listener(move |v, _: &gpui::ScrollEvent, _, cx| {
                get(v).strip_animating = false;
                if overflows {
                    cx.stop_propagation();
                }
            }))
            .when(overflows && self.renaming.is_none(), |el| {
                el.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |v, e: &gpui::MouseDownEvent, _, _| {
                        let t = get(v);
                        t.strip_animating = false;
                        t.panned = false;
                        t.pan = Some((f32::from(e.position.x), pos));
                    }),
                )
                .on_mouse_move(cx.listener(move |v, e: &gpui::MouseMoveEvent, _, cx| {
                    let t = get(v);
                    let Some((x0, from)) = t.pan else { return };
                    if e.pressed_button != Some(MouseButton::Left) {
                        t.pan = None;
                        return;
                    }
                    let dx = f32::from(e.position.x) - x0;
                    if !t.panned && dx.abs() < PAN_THRESHOLD_PX {
                        return;
                    }
                    t.panned = true;
                    let to = (from - dx).clamp(0.0, max);
                    pan_handle.set_offset(gpui::point(px(-to), px(0.0)));
                    cx.notify();
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(move |v, _: &gpui::MouseUpEvent, _, _| get(v).pan = None))
                .on_mouse_up_out(MouseButton::Left, cx.listener(move |v, _: &gpui::MouseUpEvent, _, _| {
                    let t = get(v);
                    t.pan = None;
                    t.panned = false;
                }))
            });
        div()
            .flex_1()
            .min_w(px(0.0))
            .row()
            .items_center()
            .gap(px(tokens::XS))
            .child(track)
            .when(sc.add_tab_button, |el| {
                el.child(crate::kit::icon_button("sidebar-tab-add", "plus", tab_h, palette.on_surface_variant, None, {
                    let view = cx.entity().downgrade();
                    move |window, cx| {
                        let _ = view.update(cx, |v, cx| {
                            get(v).create_tab(get, window, cx);
                            cx.notify();
                        });
                    }
                }))
            })
            .into_any_element()
    }

    /// Each label's width at the strip's size (bold, so a label never
    /// outgrows its estimate), measured again only when a label, the
    /// language or the font changes.
    fn label_widths(&mut self, labels: &[String], family: &gpui::SharedString, window: &mut Window) -> &[f32] {
        use std::hash::{Hash as _, Hasher as _};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        labels.hash(&mut h);
        family.hash(&mut h);
        let key = h.finish();
        if self.label_widths.0 != key || self.label_widths.1.len() != labels.len() {
            let font = gpui::Font { weight: FontWeight::SEMIBOLD, ..gpui::font(family.clone()) };
            let system = window.text_system();
            self.label_widths.1 = labels
                .iter()
                .map(|l| {
                    let run = gpui::TextRun { len: l.len(), font: font.clone(), color: gpui::black(), background_color: None, underline: None, strikethrough: None };
                    f32::from(system.layout_line(l, px(text::BODY_S), &[run], None).width)
                })
                .collect();
            self.label_widths.0 = key;
        }
        &self.label_widths.1
    }

    /// "+": a new tab named "New tab" (numbered when taken), shown at once
    /// with its name field focused.
    fn create_tab<V: 'static>(&mut self, get: fn(&mut V) -> &mut SidebarTabs, window: &mut Window, cx: &mut Context<V>) {
        let cfg = config(cx).read(cx).snapshot();
        let mut sidebar = cfg.sidebar.clone();
        let name = sidebar.free_tab_name(mandara_i18n::t("New tab"));
        let Ok(i) = sidebar.add_tab(&name, None) else { return };
        let id = sidebar.tabs[i].id.clone();
        let new_name = name.clone();
        update_config(cx, move |c| {
            let _ = c.sidebar.add_tab(&new_name, None);
        });
        self.select(Sel::Tab(id.clone()), &sidebar, cfg.sidebar.notifications_tab, cx);
        self.start_rename(id, &name, get, window, cx);
    }

    fn start_rename<V: 'static>(&mut self, id: String, name: &str, get: fn(&mut V) -> &mut SidebarTabs, window: &mut Window, cx: &mut Context<V>) {
        let view: WeakEntity<V> = cx.entity().downgrade();
        let tab_id = id.clone();
        let field = cx.new(|cx| {
            TextField::new(
                name.to_string(),
                mandara_i18n::t("Tab name"),
                false,
                Rc::new(move |value: String, _: &mut Window, cx: &mut App| {
                    let key = tab_id.clone();
                    if !value.trim().is_empty() {
                        update_config(cx, move |c| {
                            if let Some(i) = c.sidebar.tabs.iter().position(|t| t.id == key) {
                                let _ = c.sidebar.rename_tab(i, &value);
                            }
                        });
                    }
                    let _ = view.update(cx, |v, cx| {
                        get(v).renaming = None;
                        cx.notify();
                    });
                }),
                cx,
            )
        });
        field.update(cx, |f, cx| f.focus(window, cx));
        self.menu = None;
        self.renaming = Some((id, field));
    }

    /// The open tab menu over the sidebar (with a scrim that closes it),
    /// or nothing.
    pub fn menu_overlay<V: 'static>(&mut self, cfg: &ShellConfig, top: f32, palette: &MandaraPalette, get: fn(&mut V) -> &mut SidebarTabs, cx: &mut Context<V>) -> Option<AnyElement> {
        let menu = self.menu.clone()?;
        let sc = &cfg.sidebar;
        let id = match &menu {
            TabMenu::Actions(id) | TabMenu::Icons(id) | TabMenu::Confirm(id) => id.clone(),
        };
        let Some(ix) = sc.tabs.iter().position(|t| t.id == id) else {
            self.menu = None;
            return None;
        };
        let tab = &sc.tabs[ix];
        let item = |eid: &'static str, ic: &'static str, label: &'static str, color: mandara_theme::Color, on: MenuAction<V>, cx: &mut Context<V>| {
            div()
                .id(eid)
                .h(px(MENU_ITEM_H_PX))
                .px(px(MENU_ITEM_PAD_X_PX))
                .rounded(px(crate::style::radius(MENU_ITEM_RADIUS_PX)))
                .row()
                .items_center()
                .gap(px(tokens::SM))
                .cursor_pointer()
                .text_size(px(text::BODY_S))
                .text_color(color.hsla())
                .hover(|s| s.bg(palette.on_surface.with_alpha(HOVER_ALPHA).hsla()))
                .child(icon(ic, MENU_ICON_PX, color))
                .child(mandara_i18n::t(label))
                .on_click(cx.listener(move |v, _: &gpui::ClickEvent, window, cx| {
                    on(v, window, cx);
                    cx.notify();
                }))
                .into_any_element()
        };
        let edit = move |f: TabEdit| -> MenuAction<V> {
            let f: SharedTabEdit = f.into();
            let id = id.clone();
            Box::new(move |v, _, cx| {
                let (f, id) = (f.clone(), id.clone());
                update_config(cx, move |c| {
                    if let Some(i) = c.sidebar.tabs.iter().position(|t| t.id == id) {
                        f(&mut c.sidebar, i);
                    }
                });
                get(v).menu = None;
            })
        };
        let title = div().px(px(MENU_ITEM_PAD_X_PX)).pb(px(tokens::XS)).text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(tab_label(tab));
        let body: Vec<AnyElement> = match menu {
            TabMenu::Actions(_) => {
                let (rid, rname) = (tab.id.clone(), tab.name.clone());
                let iid = tab.id.clone();
                let cid = tab.id.clone();
                let mut v: Vec<AnyElement> = vec![
                    item("tab-menu-rename", "pencil", mandara_i18n::tk("Rename"), palette.on_surface, Box::new(move |v, window, cx| get(v).start_rename(rid.clone(), &rname, get, window, cx)), cx),
                    item("tab-menu-icon", "image", mandara_i18n::tk("Change icon"), palette.on_surface, Box::new(move |v, _, _| get(v).menu = Some(TabMenu::Icons(iid.clone()))), cx),
                ];
                if ix > 0 {
                    v.push(item("tab-menu-left", "chevron-left", mandara_i18n::tk("Move left"), palette.on_surface, edit(Box::new(|s, i| { let _ = s.move_tab(i, i - 1); })), cx));
                }
                if ix + 1 < sc.tabs.len() {
                    v.push(item("tab-menu-right", "chevron-right", mandara_i18n::tk("Move right"), palette.on_surface, edit(Box::new(|s, i| { let _ = s.move_tab(i, i + 1); })), cx));
                }
                if sc.tabs.len() > 1 {
                    v.push(item("tab-menu-delete", "trash", mandara_i18n::tk("Delete…"), palette.urgent_coral, Box::new(move |v, _, _| get(v).menu = Some(TabMenu::Confirm(cid.clone()))), cx));
                }
                v
            }
            TabMenu::Icons(_) => {
                let current = tab.icon.clone();
                let tiles = TAB_ICONS.iter().enumerate().map(|(n, name)| {
                    let active = *name == current;
                    let set = edit(Box::new(move |s, i| { let _ = s.set_tab_icon(i, name); }));
                    div()
                        .id(("tab-icon", n))
                        .size(px(ICON_TILE_PX))
                        .rounded(px(crate::style::radius(MENU_ITEM_RADIUS_PX)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .when(active, |el| el.bg(palette.primary.with_alpha(SELECTED_ALPHA).hsla()))
                        .hover(|s| s.bg(palette.on_surface.with_alpha(HOVER_ALPHA).hsla()))
                        .child(icon(name, ICON_TILE_GLYPH_PX, if active { palette.primary } else { palette.on_surface }))
                        .on_click(cx.listener(move |v, _: &gpui::ClickEvent, window, cx| {
                            set(v, window, cx);
                            cx.notify();
                        }))
                        .into_any_element()
                });
                vec![div().row().flex_wrap().gap(px(tokens::XS)).children(tiles).into_any_element()]
            }
            TabMenu::Confirm(_) => {
                let cancel_id = tab.id.clone();
                vec![
                    div().px(px(MENU_ITEM_PAD_X_PX)).pb(px(tokens::XS)).text_size(px(text::BODY_S)).child(mandara_i18n::tf("Delete the tab “{name}”? Its widgets stay available in Studio.", &[("name", &tab_label(tab))])).into_any_element(),
                    item("tab-menu-confirm", "trash", mandara_i18n::tk("Delete tab"), palette.urgent_coral, edit(Box::new(|s, i| { let _ = s.remove_tab(i); })), cx),
                    item("tab-menu-cancel", "close", mandara_i18n::tk("Cancel"), palette.on_surface, Box::new(move |v, _, _| get(v).menu = Some(TabMenu::Actions(cancel_id.clone()))), cx),
                ]
            }
        };
        let scrim = div()
            .id("tab-menu-scrim")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(palette.surface_dim.with_alpha(SCRIM_ALPHA).hsla())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |v, _: &gpui::MouseDownEvent, _, cx| {
                    get(v).menu = None;
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |v, _: &gpui::MouseDownEvent, _, cx| {
                    get(v).menu = None;
                    cx.notify();
                }),
            );
        let card = div()
            .id("tab-menu")
            .absolute()
            .top(px(top))
            .left(px(MENU_PAD_PX))
            .right(px(MENU_PAD_PX))
            .p(px(MENU_PAD_PX))
            .rounded(px(crate::style::radius(MENU_RADIUS_PX)))
            .bg(palette.surface_container_high.hsla())
            .shadow_lg()
            .occlude()
            .flex()
            .flex_col()
            .child(title)
            .children(body);
        Some(div().absolute().top_0().left_0().size_full().child(scrim).child(card).into_any_element())
    }
}

/// Width of the strip's tabs with every label shown: `labels` are the
/// measured label widths in strip order; the tab at `badge_at` carries an
/// unread badge unless it is `active`.
pub(crate) fn strip_width(labels: &[f32], badge_at: Option<usize>, active: Option<usize>) -> f32 {
    let tabs: f32 = labels
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let badge = if badge_at == Some(i) && active != Some(i) { BADGE_PX + tokens::XS + TAB_GAP_PX } else { 0.0 };
            TAB_PAD_X_PX * 2.0 + TAB_ICON_PX + tokens::XS + TAB_GAP_PX + w + badge
        })
        .sum();
    tabs + TAB_GAP_PX * labels.len().saturating_sub(1) as f32 + LABEL_SLACK_PX
}

/// The tabs' viewport inside a strip row `row_w` wide (minus "+" and the
/// track's padding).
pub(crate) fn strip_viewport(row_w: f32, tab_h: f32, add_button: bool) -> f32 {
    let plus = if add_button { tab_h + tokens::XS } else { 0.0 };
    (row_w - plus - STRIP_PAD_PX * 2.0).max(0.0)
}

/// The scroll position (0 ..= `max`, from the left) that shows the span
/// `start..end` of the content in a viewport `view` wide, scrolled to
/// `current`: unchanged when it is already in view, else the least
/// movement that shows it with `margin` beside it (a span wider than the
/// viewport aligns its start).
pub(crate) fn scroll_into_view(start: f32, end: f32, view: f32, current: f32, max: f32, margin: f32) -> f32 {
    let margin = margin.min(((view - (end - start)) / 2.0).max(0.0));
    let target = if start - margin < current || end - start > view {
        start - margin
    } else if end + margin > current + view {
        end + margin - view
    } else {
        current
    };
    target.clamp(0.0, max.max(0.0))
}

/// The body of a tab without cards: what to do next.
pub fn empty_tab(palette: &MandaraPalette) -> AnyElement {
    div()
        .py(px(EMPTY_PAD_Y_PX))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(tokens::SM))
        .text_center()
        .child(icon("tiles", EMPTY_ICON_PX, palette.on_surface_variant))
        .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child(mandara_i18n::t("This tab is empty")))
        .child(div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(mandara_i18n::t("Add any widget to it in Studio → Sidebars & hover.")))
        .child(
            div()
                .id("sidebar-empty-studio")
                .h(px(EMPTY_BUTTON_H_PX))
                .px(px(EMPTY_BUTTON_PAD_X_PX))
                .rounded_full()
                .flex()
                .items_center()
                .cursor_pointer()
                .bg(palette.primary.hsla())
                .text_color(palette.on_primary.hsla())
                .text_size(px(text::BODY_S))
                .font_weight(FontWeight::SEMIBOLD)
                .child(mandara_i18n::t("Add widgets"))
                .on_click(|_, _, cx| crate::state::open_studio_at(Some("sidebar".into()), cx)),
        )
        .into_any_element()
}

// ── Commands ──────────────────────────────────────────────────────────────

fn tab_json(t: &SidebarTab) -> serde_json::Value {
    json!({ "id": t.id, "name": t.name, "icon": t.icon, "widgets": t.cards })
}

fn tab_of(cfg: &SidebarConfig, key: &str) -> Result<usize, String> {
    cfg.tab_index(key).ok_or_else(|| mandara_i18n::t(mandara_config::sidebar::TabError::NoSuchTab.message()).to_string())
}

/// Runs a tab edit on the live config; the reply is the edited tab list.
fn edit(cx: &mut App, f: impl FnOnce(&mut SidebarConfig) -> Result<(), mandara_config::sidebar::TabError>) -> Result<serde_json::Value, String> {
    let mut sidebar = config(cx).read(cx).snapshot().sidebar.clone();
    f(&mut sidebar).map_err(|e| e.to_string())?;
    let tabs = sidebar.tabs.clone();
    update_config(cx, move |c| c.sidebar.tabs = tabs);
    Ok(json!(sidebar.tabs.iter().map(tab_json).collect::<Vec<_>>()))
}

/// Whether `id` can sit in a sidebar tab.
pub fn has_sidebar_card(id: &str, cx: &App) -> bool {
    let registry = cx.global::<PluginRegistry>();
    registry.manifests().any(|m| m.id == id && (SIDEBAR_WIDGETS.contains(&id) || matches!(m.origin, mandara_plugins::PluginOrigin::ThirdParty { .. })))
}

/// Placing a widget turns its plugin on (and brings back a removed one).
pub fn turn_on(c: &mut ShellConfig, id: &str) {
    c.plugins.disabled.retain(|x| x != id);
    c.plugins.removed.retain(|x| x != id);
    c.plugins.removed_places.remove(id);
    if !c.plugins.enabled.iter().any(|x| x == id) {
        c.plugins.enabled.push(id.to_string());
    }
}

fn position(a: &mandara_plugins::Args, i: usize) -> Result<Option<usize>, String> {
    Ok(match a.get(i) {
        Some(_) => Some(usize::try_from(a.int(i)?.max(1) - 1).unwrap_or(0)),
        None => None,
    })
}

pub fn register(r: &mut PluginRegistry) {
    let core = "core";
    const TAB_ARG: &str = mandara_i18n::tk("tab name (one word), id or 1-based position");
    r.command(
        core,
        CommandSpec::new("sidebar tab list", mandara_i18n::tk("The sidebar's tabs and their widgets"), |_, cx| {
            let cfg = config(cx).read(cx).snapshot();
            Ok(json!({ "tabs": cfg.sidebar.tabs.iter().map(tab_json).collect::<Vec<_>>(), "notifications_tab": cfg.sidebar.notifications_tab, "always_first_tab": cfg.sidebar.always_first_tab }))
        }),
    );
    r.command(
        core,
        CommandSpec::new("sidebar tab add", mandara_i18n::tk("Add a sidebar tab (--icon name)"), |a, cx| {
            let (words, icon) = take_flag(&a.0, "--icon");
            let name = words.join(" ");
            edit(cx, |s| s.add_tab(&name, icon.as_deref()).map(drop))
        })
        .arg(ArgSpec::required("name", ArgKind::String, mandara_i18n::tk("tab name (rest of the line)")))
        .example("sidebar tab add Focus --icon timer"),
    );
    r.command(
        core,
        CommandSpec::new("sidebar tab rename", mandara_i18n::tk("Rename a sidebar tab"), |a, cx| {
            let i = tab_of(&config(cx).read(cx).snapshot().sidebar, a.str(0)?)?;
            let name = a.rest(1);
            edit(cx, |s| s.rename_tab(i, &name))
        })
        .arg(ArgSpec::required("tab", ArgKind::String, TAB_ARG))
        .arg(ArgSpec::required("name", ArgKind::String, mandara_i18n::tk("new name (rest of the line)")))
        .example("sidebar tab rename 2 Deep work"),
    );
    r.command(
        core,
        CommandSpec::new("sidebar tab icon", mandara_i18n::tk("Change a sidebar tab's icon"), |a, cx| {
            let i = tab_of(&config(cx).read(cx).snapshot().sidebar, a.str(0)?)?;
            let icon = a.str(1)?.to_string();
            edit(cx, |s| s.set_tab_icon(i, &icon))
        })
        .arg(ArgSpec::required("tab", ArgKind::String, TAB_ARG))
        .arg(ArgSpec::required("icon", ArgKind::String, mandara_i18n::tk("a Mandara or icon-theme icon name")))
        .example("sidebar tab icon Focus timer"),
    );
    r.command(
        core,
        CommandSpec::new("sidebar tab remove", mandara_i18n::tk("Delete a sidebar tab (the last one stays)"), |a, cx| {
            let i = tab_of(&config(cx).read(cx).snapshot().sidebar, a.str(0)?)?;
            edit(cx, |s| s.remove_tab(i).map(drop))
        })
        .arg(ArgSpec::required("tab", ArgKind::String, TAB_ARG))
        .example("sidebar tab remove Focus"),
    );
    r.command(
        core,
        CommandSpec::new("sidebar tab move", mandara_i18n::tk("Move a sidebar tab to a 1-based position"), |a, cx| {
            let i = tab_of(&config(cx).read(cx).snapshot().sidebar, a.str(0)?)?;
            let to = position(a, 1)?.unwrap_or(0);
            edit(cx, |s| s.move_tab(i, to).map(drop))
        })
        .arg(ArgSpec::required("tab", ArgKind::String, TAB_ARG))
        .arg(ArgSpec::required("position", ArgKind::Integer, mandara_i18n::tk("1 = first")))
        .example("sidebar tab move Focus 1"),
    );
    r.command(
        core,
        CommandSpec::new("sidebar tab open", mandara_i18n::tk("Open the sidebar on a tab"), |a, cx| {
            let key = a.rest(0);
            let cfg = config(cx).read(cx).snapshot();
            let target = if key.eq_ignore_ascii_case(NOTIFICATIONS_TAB) && cfg.sidebar.notifications_tab {
                NOTIFICATIONS_TAB.to_string()
            } else {
                cfg.sidebar.tabs[tab_of(&cfg.sidebar, &key)?].id.clone()
            };
            let generation = cx.try_global::<OpenRequest>().map_or(0, |r| r.generation) + 1;
            cx.set_global(OpenRequest { generation, key: target.clone() });
            shell(cx).update(cx, |s, cx| s.set_sidebar(true, cx));
            Ok(json!({ "sidebar": true, "tab": target }))
        })
        .arg(ArgSpec::required("tab", ArgKind::String, mandara_i18n::tk("tab name, id, 1-based position or notifications")))
        .example("sidebar tab open 2")
        .example("sidebar tab open notifications"),
    );
    r.command(
        core,
        CommandSpec::new("sidebar tab widget add", mandara_i18n::tk("Put a plugin's card on a tab (turns the plugin on)"), |a, cx| {
            let i = tab_of(&config(cx).read(cx).snapshot().sidebar, a.str(0)?)?;
            let plugin = a.str(1)?.to_string();
            if !has_sidebar_card(&plugin, cx) {
                return Err(mandara_i18n::tf("No plugin with a sidebar card is called {id}", &[("id", &plugin)]));
            }
            let at = position(a, 2)?;
            let reply = edit(cx, |s| s.add_card(i, &plugin, at))?;
            update_config(cx, move |c| turn_on(c, &plugin));
            Ok(reply)
        })
        .arg(ArgSpec::required("tab", ArgKind::String, TAB_ARG))
        .arg(ArgSpec::required("plugin", ArgKind::String, mandara_i18n::tk("plugin id (see `plugins list`)")))
        .arg(ArgSpec::optional("position", ArgKind::Integer, mandara_i18n::tk("1 = top (default: bottom)")))
        .example("sidebar tab widget add Focus pomodoro"),
    );
    r.command(
        core,
        CommandSpec::new("sidebar tab widget remove", mandara_i18n::tk("Take a plugin's card off a tab"), |a, cx| {
            let i = tab_of(&config(cx).read(cx).snapshot().sidebar, a.str(0)?)?;
            let plugin = a.str(1)?.to_string();
            edit(cx, |s| s.remove_card(i, &plugin))
        })
        .arg(ArgSpec::required("tab", ArgKind::String, TAB_ARG))
        .arg(ArgSpec::required("plugin", ArgKind::String, mandara_i18n::tk("plugin id")))
        .example("sidebar tab widget remove Overview weather"),
    );
    r.command(
        core,
        CommandSpec::new("sidebar tab widget move", mandara_i18n::tk("Move a card to another tab or position"), |a, cx| {
            let cfg = config(cx).read(cx).snapshot();
            let plugin = a.str(0)?.to_string();
            let from = tab_of(&cfg.sidebar, a.str(1)?)?;
            let to = tab_of(&cfg.sidebar, a.str(2)?)?;
            let at = position(a, 3)?;
            edit(cx, |s| s.move_card(from, to, &plugin, at))
        })
        .arg(ArgSpec::required("plugin", ArgKind::String, mandara_i18n::tk("plugin id")))
        .arg(ArgSpec::required("from", ArgKind::String, TAB_ARG))
        .arg(ArgSpec::required("to", ArgKind::String, TAB_ARG))
        .arg(ArgSpec::optional("position", ArgKind::Integer, mandara_i18n::tk("1 = top (default: bottom)")))
        .example("sidebar tab widget move media Overview Focus 1"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_are_split_from_words() {
        let w = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        assert_eq!(take_flag(&w("Deep work --icon timer"), "--icon"), (w("Deep work"), Some("timer".into())));
        assert_eq!(take_flag(&w("--icon=leaf Garden"), "--icon"), (w("Garden"), Some("leaf".into())));
        assert_eq!(take_flag(&w("Plain"), "--icon"), (w("Plain"), None));
    }

    #[test]
    fn tab_icons_are_bundled() {
        for name in TAB_ICONS {
            assert!(crate::icons::ICON_NAMES.contains(name), "{name} is not in Mandara's icon set");
        }
        assert!(TAB_ICONS.contains(&mandara_config::sidebar::DEFAULT_TAB_ICON));
        assert!(TAB_ICONS.contains(&mandara_config::sidebar::OVERVIEW_ICON));
    }

    #[test]
    fn scroll_into_view_moves_least() {
        // Viewport 200 wide over 600 of content.
        let (view, max, m) = (200.0, 400.0, 10.0);
        // Already visible: stays.
        assert_eq!(scroll_into_view(50.0, 120.0, view, 0.0, max, m), 0.0);
        // Past the right edge: just enough to show it plus the margin.
        assert_eq!(scroll_into_view(250.0, 300.0, view, 0.0, max, m), 110.0);
        // Before the left edge: its start, minus the margin.
        assert_eq!(scroll_into_view(40.0, 90.0, view, 100.0, max, m), 30.0);
        // Clamped to the content.
        assert_eq!(scroll_into_view(560.0, 600.0, view, 0.0, max, m), max);
        assert_eq!(scroll_into_view(0.0, 40.0, view, 300.0, max, m), 0.0);
        // Wider than the viewport: its start aligns.
        assert_eq!(scroll_into_view(100.0, 400.0, view, 0.0, max, m), 100.0);
        // No overflow: never scrolls.
        assert_eq!(scroll_into_view(150.0, 190.0, view, 0.0, 0.0, m), 0.0);
    }

    #[test]
    fn compacting_follows_the_width() {
        let labels = [52.0, 74.0];
        let full = strip_width(&labels, None, Some(0));
        let one = TAB_PAD_X_PX * 2.0 + TAB_ICON_PX + tokens::XS + TAB_GAP_PX;
        assert_eq!(full, one * 2.0 + 52.0 + 74.0 + TAB_GAP_PX + LABEL_SLACK_PX);
        // An unread badge on an idle tab widens it; on the active one it hides.
        assert!(strip_width(&labels, Some(1), Some(0)) > full);
        assert_eq!(strip_width(&labels, Some(0), Some(0)), full);
        // Wide sidebars keep every label, narrow ones compact.
        assert!(full <= strip_viewport(full + 60.0, 30.0, true));
        assert!(full > strip_viewport(full, 30.0, true));
        assert_eq!(strip_viewport(10.0, 30.0, true), 0.0);
        // Ten tabs never fit the default sidebar with labels.
        let row = mandara_config::defaults::SIDEBAR_WIDTH_PX - 2.0 * (30.0 + tokens::XS) - 28.0;
        assert!(strip_width(&[60.0; 10], None, Some(0)) > strip_viewport(row, 30.0, true));
    }

    #[test]
    fn strip_order_ends_with_notifications() {
        let mut c = SidebarConfig::default();
        c.add_tab("Work", None).unwrap();
        assert_eq!(order(&c, true), vec![Sel::Tab("overview".into()), Sel::Tab("work".into()), Sel::Notifications]);
        assert_eq!(order(&c, false).len(), 2);
    }
}
