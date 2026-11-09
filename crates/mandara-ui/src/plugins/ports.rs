//! Listening ports: which servers are listening on this computer and who
//! can reach them. The bar chip is a plug glyph with the number of
//! listening TCP ports. The panel lists TCP listeners and unconnected UDP
//! sockets with the port, protocol, bind scope (localhost only / all
//! interfaces / one address) and the owning process with its app icon,
//! plus a search box and an All / TCP / UDP filter. Each row opens
//! `http://localhost:PORT` (TCP), copies the address, or ends the process
//! (End arms first, then SIGTERM, like the process hunter).
//!
//! Data comes from `/proc/net/{tcp,tcp6,udp,udp6}` and `/proc/<pid>/fd`
//! ([`mandara_services::ports`]); other users' sockets show as a system
//! process without a PID. The panel samples every 2 s while it is open;
//! the chip re-counts every 15 s while it is on screen (a cheap read of the
//! four tables, no process scan). Nothing runs off screen. Commands:
//! `ports list`, `ports kill <port>`.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext as _, ElementId, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, Task, Window, div, prelude::FluentBuilder as _, px, size,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk, tr};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::ports::{self as svc, Listener, Proto, Scope};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::extras::{CONTROL_H, Flash, Segment, search_box, segmented, toast};
use super::model_view as fn_view;
use super::panel_fx::{
    ANGLE_DIAGONAL_DEG, ANGLE_DOWN_DEG, HEADER_H, PILL_H_PX, PanelShape, SMALL_ICON_PX, ViewSet, alpha, empty_state, glass_card, glow, header, partner, readable,
    redraw_in,
};
use crate::fonts::tabular_nums;
use crate::icons::icon;
use crate::kit::{icon_button, reveal, text, tokens};
use crate::text_field::TextField;

pub const ID: &str = "ports";

// ── Layout ───────────────────────────────────────────────────────────────

const PANEL_W: f32 = 540.0;
const PAD: f32 = 14.0;
const GAP: f32 = tokens::MD;
/// Summary tiles.
const STATS_H: f32 = 72.0;
const STAT_GAP_PX: f32 = 10.0;
const STAT_PAD_PX: f32 = 12.0;
const STAT_VALUE_PX: f32 = 24.0;
const STAT_VALUE_LINE_PX: f32 = 28.0;
const STAT_ICON_PX: f32 = 13.0;
const STAT_LABEL_GAP_PX: f32 = 6.0;
/// Rows.
const ROW_H: f32 = 58.0;
const ROW_GAP: f32 = 4.0;
const ROW_PAD_X_PX: f32 = 8.0;
const ROW_INNER_GAP_PX: f32 = 12.0;
const ROW_HOVER: (f32, f32) = (0.05, 0.04);
const ROW_ARMED: (f32, f32) = (0.1, 0.07);
const ROW_RADIUS_INSET_PX: f32 = 2.0;
const ROW_MIN_RADIUS_PX: f32 = 10.0;
/// The big port number on its tinted tile.
const PORT_W_PX: f32 = 78.0;
const PORT_H_PX: f32 = 44.0;
const PORT_TEXT_PX: f32 = 20.0;
const PORT_RADIUS_PX: f32 = 12.0;
const PORT_FILL: (f32, f32) = (0.16, 0.08);
/// App icon / glyph avatar.
const AVATAR_PX: f32 = 30.0;
const AVATAR_GLYPH_PX: f32 = 20.0;
const AVATAR_ICON_PX: f32 = 15.0;
const AVATAR_RADIUS_FRAC: f32 = 0.32;
const AVATAR_FILL: (f32, f32) = (0.06, 0.045);
const LINE_GAP_PX: f32 = 2.0;
const INLINE_GAP_PX: f32 = 6.0;
/// Protocol and scope badges.
const BADGE_H_PX: f32 = 16.0;
const BADGE_PAD_X_PX: f32 = 6.0;
const BADGE_TEXT_PX: f32 = 10.0;
const BADGE_FILL: (f32, f32) = (0.14, 0.1);
const SCOPE_DOT_PX: f32 = 6.0;
/// Row actions.
const ACTION_BTN_PX: f32 = 28.0;
const ACTIONS_GAP_PX: f32 = 4.0;
const END_PAD_X_PX: f32 = 11.0;
const END_TEXT_PX: f32 = text::LABEL + 0.5;
const END_SOFT: (f32, f32) = (0.16, 0.11);
const END_HOVER: (f32, f32) = (0.26, 0.18);
const ARMED_GLOW: (f32, f32, f32) = (0.3, 8.0, 1.0);
/// Rows visible before the list scrolls (the cut row hints at more).
const VISIBLE_ROWS: f32 = 6.5;
const LIST_MAX_H: f32 = VISIBLE_ROWS * (ROW_H + ROW_GAP);
const LIST_FADE_PX: f32 = 22.0;
const EMPTY_H: f32 = 170.0;
/// Bar chip.
const CHIP_ICON_PX: f32 = 15.0;
const CHIP_GAP_PX: f32 = 4.0;
const CHIP_TEXT_PX: f32 = 12.0;
const CHIP_IDLE_ALPHA: f32 = 0.7;
/// Size changes smaller than this don't resize the panel.
const SIZE_EPSILON_PX: f32 = 0.5;
/// `ink_on`: fills lighter than this get dark ink.
const INK_SWITCH_L: f32 = 0.64;
const DARK_INK: (f32, f32) = (0.2, 0.02);
const LIGHT_INK_L: f32 = 0.99;

// ── Timing ───────────────────────────────────────────────────────────────

/// Full sample (with processes) while the panel is open.
const PANEL_EVERY: Duration = Duration::from_secs(2);
/// Chip count while the chip is on screen.
const CHIP_EVERY: Duration = Duration::from_secs(15);
/// A sample this close to due counts as due (render timers jitter).
const SAMPLE_SLACK: Duration = Duration::from_millis(150);
/// The chip poller stops once no view rendered for this long past a cycle.
const IDLE_GRACE: Duration = Duration::from_secs(3);
/// The panel counts as open this long after its last render.
const PANEL_SEEN: Duration = Duration::from_secs(4);
/// An End press arms the button; a second press within this sends SIGTERM.
const ARM: Duration = Duration::from_secs(3);
const ARM_TICK: Duration = Duration::from_millis(500);
/// Process names whose app-icon lookup is remembered (bounded).
const ICON_CACHE_CAP: usize = 512;

// ── Model ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Filter {
    All,
    Tcp,
    Udp,
}

impl Filter {
    fn admits(self, p: Proto) -> bool {
        match self {
            Filter::All => true,
            Filter::Tcp => p == Proto::Tcp,
            Filter::Udp => p == Proto::Udp,
        }
    }
}

/// A row's identity (stable across samples).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RowKey(Proto, u16, Option<u32>);

impl RowKey {
    fn of(l: &Listener) -> Self {
        Self(l.proto, l.port, l.pid)
    }

    fn element(&self, prefix: &'static str) -> ElementId {
        let proto = if self.0 == Proto::Tcp { 0 } else { 1 };
        let n = (u64::from(self.2.unwrap_or(0)) << 20) | (u64::from(self.1) << 1) | proto;
        ElementId::NamedInteger(SharedString::new_static(prefix), n)
    }
}

/// Labels formatted once per sample.
struct Row {
    port: SharedString,
    name: SharedString,
    detail: SharedString,
    scope: SharedString,
}

struct Model {
    listeners: Vec<Listener>,
    rows: Vec<Row>,
    /// Process name → it has an app icon (looked up off the UI thread).
    icons: HashMap<String, bool>,
    /// Distinct listening TCP ports (the chip).
    tcp_count: Option<usize>,
    /// inode → PID from the last scan (speeds up the next one).
    hint: HashMap<u64, u32>,
    full_at: Option<Instant>,
    count_at: Option<Instant>,
    in_flight: bool,
    generation: u64,
    filter: Filter,
    /// (generation, query, filter) → indices of the shown rows.
    shown: Option<((u64, String, Filter), Vec<usize>)>,
    armed: HashMap<RowKey, Instant>,
    flash: Flash,
    seen: Option<Instant>,
    panel_seen: Option<Instant>,
    poller: Option<Task<()>>,
    close_watch: Rc<Cell<bool>>,
    views: ViewSet,
}

impl Model {
    fn new() -> Self {
        Self {
            listeners: Vec::new(),
            rows: Vec::new(),
            icons: HashMap::new(),
            tcp_count: None,
            hint: HashMap::new(),
            full_at: None,
            count_at: None,
            in_flight: false,
            generation: 0,
            filter: Filter::All,
            shown: None,
            armed: HashMap::new(),
            flash: Flash::default(),
            seen: None,
            panel_seen: None,
            poller: None,
            close_watch: Rc::default(),
            views: ViewSet::default(),
        }
    }

    fn absorb(&mut self, listeners: Vec<Listener>) {
        self.tcp_count = Some(listeners.iter().filter(|l| l.proto == Proto::Tcp).map(|l| l.port).collect::<HashSet<_>>().len());
        self.rows = listeners.iter().map(row_labels).collect();
        self.armed.retain(|k, at| at.elapsed() < ARM && listeners.iter().any(|l| RowKey::of(l) == *k));
        self.listeners = listeners;
        self.generation = self.generation.wrapping_add(1);
        self.full_at = Some(Instant::now());
        self.count_at = self.full_at;
    }

    /// Frees what only the panel shows (the panel closed).
    fn free_panel(&mut self) {
        self.listeners = Vec::new();
        self.rows = Vec::new();
        self.shown = None;
        self.armed = HashMap::new();
        self.full_at = None;
        self.panel_seen = None;
        self.generation = self.generation.wrapping_add(1);
    }

    fn armed(&self, key: &RowKey) -> bool {
        self.armed.get(key).is_some_and(|t| t.elapsed() < ARM)
    }

    /// Indices of the rows matching `query` and the filter (cached).
    fn shown(&mut self, query: &str) -> &[usize] {
        let key = (self.generation, query.to_string(), self.filter);
        if self.shown.as_ref().is_none_or(|(k, _)| *k != key) {
            let q = query.trim().to_lowercase();
            let idx = self.listeners.iter().enumerate().filter(|(_, l)| self.filter.admits(l.proto) && matches(l, &q)).map(|(i, _)| i).collect();
            self.shown = Some((key, idx));
        }
        self.shown.as_ref().map(|(_, v)| v.as_slice()).unwrap_or_default()
    }
}

/// A query matches the port, the process (name, PID or command) or an
/// address.
fn matches(l: &Listener, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    l.port.to_string().contains(q)
        || l.process.as_deref().is_some_and(|n| n.to_lowercase().contains(q))
        || l.pid.is_some_and(|p| p.to_string() == q)
        || l.command.as_deref().is_some_and(|c| c.to_lowercase().contains(q))
        || l.addresses.iter().any(|a| a.to_string().contains(q))
}

fn scope_label(l: &Listener) -> String {
    match l.scope {
        Scope::Localhost => t("Localhost only").into(),
        Scope::AllInterfaces => t("All interfaces").into(),
        Scope::Address => l.addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "),
    }
}

fn row_labels(l: &Listener) -> Row {
    let name = l.process.clone().unwrap_or_else(|| t("System process").into());
    let detail = match (l.pid, l.command.as_deref()) {
        (Some(pid), Some(cmd)) => tf("PID {pid} · {command}", &[("pid", &pid), ("command", &cmd)]),
        (Some(pid), None) => tf("PID {pid}", &[("pid", &pid)]),
        (None, _) => t("Owned by another user").into(),
    };
    Row { port: l.port.to_string().into(), name: name.into(), detail: detail.into(), scope: scope_label(l).into() }
}

type Shared = Rc<RefCell<Model>>;

thread_local! {
    static MODEL: RefCell<Weak<RefCell<Model>>> = const { RefCell::new(Weak::new()) };
}

/// The model shared by every chip and panel of this plugin.
fn model() -> Shared {
    if let Some(m) = MODEL.with(|w| w.borrow().upgrade()) {
        return m;
    }
    let m = Rc::new(RefCell::new(Model::new()));
    MODEL.with(|w| *w.borrow_mut() = Rc::downgrade(&m));
    m
}

/// One background sample's result.
enum Sample {
    Full { listeners: Vec<Listener>, hint: HashMap<u64, u32>, icons: Vec<(String, bool)> },
    Count(usize),
}

/// Starts a background sample when one is due: `full` (listeners with
/// their processes, for the panel) or just the TCP port count (the chip).
fn refresh(m: &Shared, full: bool, cx: &mut App) {
    let (hint, known) = {
        let mut mb = m.borrow_mut();
        let every = if full { PANEL_EVERY } else { CHIP_EVERY };
        let last = if full { mb.full_at } else { mb.count_at };
        if mb.in_flight || last.is_some_and(|t| t.elapsed() + SAMPLE_SLACK < every) {
            return;
        }
        mb.in_flight = true;
        if mb.icons.len() > ICON_CACHE_CAP {
            mb.icons.clear();
        }
        (if full { std::mem::take(&mut mb.hint) } else { HashMap::new() }, mb.icons.keys().cloned().collect::<HashSet<String>>())
    };
    let m = m.clone();
    cx.spawn(async move |cx| {
        let sample = cx
            .background_executor()
            .spawn(async move {
                if !full {
                    return Sample::Count(svc::tcp_port_count(&svc::sockets()));
                }
                let mut hint = hint;
                let listeners = svc::listeners(&mut hint);
                // Resolve new app icons here so rows never search an icon
                // theme on the UI thread (the resolver caches globally).
                let names: HashSet<&str> = listeners.iter().filter_map(|l| l.process.as_deref()).filter(|n| !known.contains(*n)).collect();
                let icons = names.into_iter().map(|n| (n.to_string(), crate::apps::icon_for_app_id(n, false).is_some())).collect();
                Sample::Full { listeners, hint, icons }
            })
            .await;
        let views = {
            let mut mb = m.borrow_mut();
            mb.in_flight = false;
            match sample {
                Sample::Full { listeners, hint, icons } => {
                    mb.hint = hint;
                    mb.icons.extend(icons);
                    // A panel that closed meanwhile doesn't keep the list.
                    if mb.panel_seen.is_some_and(|t| t.elapsed() < PANEL_SEEN) {
                        mb.absorb(listeners);
                    } else {
                        mb.tcp_count = Some(listeners.iter().filter(|l| l.proto == Proto::Tcp).map(|l| l.port).collect::<HashSet<_>>().len());
                        mb.count_at = Some(Instant::now());
                    }
                }
                Sample::Count(n) => {
                    mb.tcp_count = Some(n);
                    mb.count_at = Some(Instant::now());
                }
            }
            mb.views.clone()
        };
        views.notify_async(cx);
    })
    .detach();
}

/// Marks a view as on screen and keeps the chip's count poller running
/// while any is (it ends once none rendered for a cycle).
fn keep_alive(m: &Shared, window: &Window, cx: &mut App) {
    let mut mb = m.borrow_mut();
    mb.views.track(window);
    mb.seen = Some(Instant::now());
    if mb.poller.is_some() {
        return;
    }
    let weak = Rc::downgrade(m);
    mb.poller = Some(cx.spawn(async move |cx| {
        let mut slept = Duration::ZERO;
        loop {
            let Some(m) = weak.upgrade() else { return };
            // On screen = rendered since this poller went to sleep (each
            // wake notifies the views, so a visible chip always has).
            if m.borrow().seen.is_none_or(|t| t.elapsed() > slept + IDLE_GRACE) {
                break;
            }
            cx.update(|cx| {
                refresh(&m, false, cx);
                m.borrow().views.notify(cx);
            });
            drop(m);
            cx.background_executor().timer(CHIP_EVERY).await;
            slept = CHIP_EVERY;
        }
        if let Some(m) = weak.upgrade() {
            m.borrow_mut().poller = None;
        }
    }));
}

// ── Pieces ───────────────────────────────────────────────────────────────

fn ink_on(bg: Color) -> Color {
    if bg.lightness() > INK_SWITCH_L { Color::oklch(DARK_INK.0, DARK_INK.1, bg.hue()) } else { Color::oklch(LIGHT_INK_L, 0.0, 0.0) }
}

fn scope_color(scope: Scope, palette: &MandaraPalette) -> Color {
    readable(
        match scope {
            Scope::Localhost => palette.success_emerald,
            Scope::AllInterfaces => palette.warning_amber,
            Scope::Address => palette.tertiary,
        },
        palette,
    )
}

fn badge(label: impl Into<SharedString>, tint: Color, palette: &MandaraPalette) -> AnyElement {
    div()
        .flex_none()
        .h(px(BADGE_H_PX))
        .px(px(BADGE_PAD_X_PX))
        .rounded_full()
        .flex()
        .items_center()
        .bg(tint.with_alpha(alpha(BADGE_FILL, palette)).hsla())
        .text_size(px(BADGE_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tint.hsla())
        .child(label.into())
        .into_any_element()
}

fn avatar(l: &Listener, has_icon: bool, palette: &MandaraPalette) -> AnyElement {
    let inner: AnyElement = match l.process.as_deref() {
        Some(name) if has_icon => super::workspaces::app_icon(name, AVATAR_GLYPH_PX, palette.on_surface_variant, false, palette),
        Some(_) => icon("terminal", AVATAR_ICON_PX, palette.on_surface_variant).into_any_element(),
        None => icon("server", AVATAR_ICON_PX, palette.on_surface_variant).into_any_element(),
    };
    div()
        .size(px(AVATAR_PX))
        .flex_none()
        .rounded(px(crate::style::radius(AVATAR_PX * AVATAR_RADIUS_FRAC)))
        .bg(palette.on_surface.with_alpha(alpha(AVATAR_FILL, palette)).hsla())
        .flex()
        .items_center()
        .justify_center()
        .child(inner)
        .into_any_element()
}

fn port_tile(port: SharedString, proto: Proto, palette: &MandaraPalette) -> AnyElement {
    let tint = readable(if proto == Proto::Tcp { palette.primary } else { palette.tertiary }, palette);
    div()
        .w(px(PORT_W_PX))
        .h(px(PORT_H_PX))
        .flex_none()
        .rounded(px(crate::style::radius(PORT_RADIUS_PX)))
        .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, tint.with_alpha(alpha(PORT_FILL, palette)), partner(tint).with_alpha(alpha(PORT_FILL, palette) * 0.5)))
        .flex()
        .items_center()
        .justify_center()
        .font_features(tabular_nums())
        .text_size(px(PORT_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tint.hsla())
        .child(port)
        .into_any_element()
}

fn copy_text(s: &str) -> Result<(), String> {
    mandara_services::clipboard::serve(s.as_bytes().to_vec(), None)
}

/// Opens `url` with the default browser, off the UI thread.
fn open_url(url: String) {
    std::thread::spawn(move || {
        if let Err(err) = mandara_services::session::open_uri(&url) {
            tracing::warn!("ports: opening {url}: {err}");
        }
    });
}

/// The End → Confirm button of one row.
fn end_button(key: RowKey, name: SharedString, pid: u32, m: &Shared, palette: &MandaraPalette) -> AnyElement {
    let armed = m.borrow().armed(&key);
    let coral = palette.urgent_coral;
    let soft = readable(coral, palette);
    let ink = ink_on(coral);
    let (soft_fill, hover_fill) = (alpha(END_SOFT, palette), alpha(END_HOVER, palette));
    let (glow_alpha, glow_blur, glow_dy) = ARMED_GLOW;
    use crate::components::buttons::{Ink, designed};
    let m = m.clone();
    let id = key.element("ports-end");
    // Armed it is filled coral (the classic one glows too).
    let ink_of = if armed { Ink { bg: Some(coral), hover: coral, outline: None } } else { Ink { bg: Some(coral.with_alpha(soft_fill)), hover: coral.with_alpha(hover_fill), outline: None } };
    let el = div().id(id.clone()).h(px(PILL_H_PX)).px(px(END_PAD_X_PX)).flex_none();
    designed(el, id, PILL_H_PX, false, ink_of, Some(palette), |el| {
        el.rounded_full()
            .when(armed, |el| el.bg(coral.hsla()).shadow(vec![glow(coral.with_alpha(glow_alpha), glow_blur, 0.0, glow_dy)]))
            .when(!armed, |el| el.bg(coral.with_alpha(soft_fill).hsla()).hover(move |s| s.bg(coral.with_alpha(hover_fill).hsla())))
    })
        .flex()
        .items_center()
        .gap(px(ACTIONS_GAP_PX))
        .cursor_pointer()
        .text_size(px(END_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(if armed { ink.hsla() } else { soft.hsla() })
        .child(icon(if armed { "check" } else { "close" }, SMALL_ICON_PX, if armed { ink } else { soft }))
        .child(if armed { t("Confirm") } else { t("End") })
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            let views = {
                let mut mb = m.borrow_mut();
                if mb.armed(&key) {
                    mb.armed.remove(&key);
                    match mandara_services::processes::kill(pid, false) {
                        Ok(()) => mb.flash.show(true, tf("Asked to quit: {name} ({pid})", &[("name", &name), ("pid", &pid)])),
                        Err(e) => mb.flash.show(false, tr(&e)),
                    }
                    // Re-sample soon so the row disappears.
                    mb.full_at = None;
                } else {
                    mb.armed.insert(key.clone(), Instant::now());
                }
                mb.views.clone()
            };
            views.notify(cx);
        })
        .into_any_element()
}

fn row(i: usize, m: &Shared, palette: &MandaraPalette, radius: f32) -> AnyElement {
    let mb = m.borrow();
    let (l, labels) = (&mb.listeners[i], &mb.rows[i]);
    let key = RowKey::of(l);
    let armed = mb.armed(&key);
    let has_icon = l.process.as_deref().is_some_and(|n| mb.icons.get(n).copied().unwrap_or(false));
    let scope_tint = scope_color(l.scope, palette);
    let hover = palette.on_surface.with_alpha(alpha(ROW_HOVER, palette));
    let warn = palette.urgent_coral.with_alpha(alpha(ROW_ARMED, palette));
    let muted = palette.on_surface_variant;
    let flash = mb.flash.clone();

    let mut actions = div().flex().items_center().gap(px(ACTIONS_GAP_PX)).flex_none();
    if let Some(url) = l.url() {
        let flash = flash.clone();
        let views = mb.views.clone();
        actions = actions.child(
            div().invisible().group_hover("ports-row", |s| s.visible()).child(icon_button(key.element("ports-open"), "external", ACTION_BTN_PX, muted, None, move |_, cx| {
                open_url(url.clone());
                flash.show(true, tf("Opening {url}", &[("url", &url)]));
                views.notify(cx);
            })),
        );
    }
    {
        let address = l.address();
        let views = mb.views.clone();
        actions = actions.child(div().when(!armed, |el| el.invisible().group_hover("ports-row", |s| s.visible())).child(icon_button(
            key.element("ports-copy"),
            "copy",
            ACTION_BTN_PX,
            muted,
            None,
            move |_, cx| {
                match copy_text(&address) {
                    Ok(()) => flash.show(true, tf("Copied {address}", &[("address", &address)])),
                    Err(e) => flash.show(false, e),
                }
                views.notify(cx);
            },
        )));
    }
    if let Some(pid) = l.pid {
        let end = end_button(key.clone(), labels.name.clone(), pid, m, palette);
        actions = actions.child(div().when(!armed, |el| el.invisible().group_hover("ports-row", |s| s.visible())).child(end));
    }

    div()
        .id(key.element("ports-row"))
        .group("ports-row")
        .h(px(ROW_H))
        .flex_none()
        .px(px(ROW_PAD_X_PX))
        .rounded(px(radius))
        .flex()
        .items_center()
        .gap(px(ROW_INNER_GAP_PX))
        .when(armed, |el| el.bg(warn.hsla()))
        .when(!armed, |el| el.hover(move |s| s.bg(hover.hsla())))
        .child(port_tile(labels.port.clone(), l.proto, palette))
        .child(avatar(l, has_icon, palette))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(LINE_GAP_PX))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(INLINE_GAP_PX))
                        .min_w(px(0.0))
                        .child(div().min_w(px(0.0)).truncate().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child(labels.name.clone()))
                        .child(badge(l.proto.label(), readable(if l.proto == Proto::Tcp { palette.primary } else { palette.tertiary }, palette), palette)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(INLINE_GAP_PX))
                        .min_w(px(0.0))
                        .text_size(px(text::LABEL))
                        .text_color(muted.hsla())
                        .child(div().size(px(SCOPE_DOT_PX)).flex_none().rounded_full().bg(scope_tint.hsla()))
                        .child(div().flex_none().text_color(scope_tint.hsla()).font_weight(FontWeight::MEDIUM).child(labels.scope.clone()))
                        .child(div().flex_none().child("·"))
                        .child(div().min_w(px(0.0)).truncate().font_features(tabular_nums()).child(labels.detail.clone())),
                ),
        )
        .child(actions)
        .into_any_element()
}

fn stat(glyph: &'static str, label: &'static str, value: String, tint: Color, radius: f32, palette: &MandaraPalette) -> AnyElement {
    glass_card(palette)
        .rounded(px(radius))
        .flex_1()
        .min_w(px(0.0))
        .h(px(STATS_H))
        .p(px(STAT_PAD_PX))
        .flex()
        .flex_col()
        .justify_between()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(STAT_LABEL_GAP_PX))
                .child(icon(glyph, STAT_ICON_PX, tint))
                .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(label)),
        )
        .child(div().font_features(tabular_nums()).text_size(px(STAT_VALUE_PX)).line_height(px(STAT_VALUE_LINE_PX)).font_weight(FontWeight::MEDIUM).child(value))
        .into_any_element()
}

// ── Panel ────────────────────────────────────────────────────────────────

struct View {
    p: PanelCx,
    m: Shared,
    field: Entity<TextField>,
    size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>,
}

impl View {
    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let m = &self.m;
        keep_alive(m, window, cx);
        if self.p.progress() > 0.0 {
            m.borrow_mut().panel_seen = Some(Instant::now());
            refresh(m, true, cx);
            redraw_in(PANEL_EVERY, window, cx);
            let watch = m.borrow().close_watch.clone();
            if !watch.get() {
                let freed = m.clone();
                super::processes::on_panel_close(&self.p, &watch, cx, move |_| freed.borrow_mut().free_panel());
            }
        }
        let shape = PanelShape::of(cx);
        let card_r = shape.inner(PAD);
        let row_r = (card_r - ROW_RADIUS_INSET_PX).max(ROW_MIN_RADIUS_PX);
        let query = self.field.read(cx).value.clone();
        let flash = m.borrow().flash.current(window, cx);
        if !m.borrow().armed.is_empty() {
            redraw_in(ARM_TICK, window, cx);
        }

        let (sampled, total, tcp, udp, exposed, filter) = {
            let mb = m.borrow();
            let ls = &mb.listeners;
            (
                mb.full_at.is_some(),
                ls.len(),
                ls.iter().filter(|l| l.proto == Proto::Tcp).count(),
                ls.iter().filter(|l| l.proto == Proto::Udp).count(),
                ls.iter().filter(|l| l.scope != Scope::Localhost).count(),
                mb.filter,
            )
        };
        let shown: Vec<usize> = m.borrow_mut().shown(&query).to_vec();

        let accent = readable(palette.primary, palette);
        let subtitle = if !sampled {
            t("Reading sockets…").to_string()
        } else if query.trim().is_empty() {
            tf("{count} listening · {exposed} reachable from the network", &[("count", &total), ("exposed", &exposed)])
        } else {
            tf("{matches} of {total} match", &[("matches", &shown.len()), ("total", &total)])
        };
        let head = header("plug", accent, t("Listening ports"), subtitle, palette, None);

        let dash = |n: usize| if sampled { n.to_string() } else { "–".into() };
        let stats = div()
            .flex()
            .gap(px(STAT_GAP_PX))
            .child(stat("plug", t("TCP"), dash(tcp), readable(palette.primary, palette), card_r, palette))
            .child(stat("waves", t("UDP"), dash(udp), readable(palette.tertiary, palette), card_r, palette))
            .child(stat("globe", t("Open to network"), dash(exposed), readable(if exposed > 0 { palette.warning_amber } else { palette.success_emerald }, palette), card_r, palette));

        let seg = |label: &'static str, which: Filter| {
            let m = m.clone();
            Segment {
                label: tr(label).into(),
                icon: None,
                active: filter == which,
                on_click: Rc::new(move |_, cx| {
                    let views = {
                        let mut mb = m.borrow_mut();
                        mb.filter = which;
                        mb.views.clone()
                    };
                    views.notify(cx);
                }),
            }
        };
        let controls = div()
            .flex()
            .items_center()
            .gap(px(tokens::SM))
            .child(search_box("ports-clear", &self.field, &query, palette, cx))
            .child(segmented("ports-filter", vec![seg(tk("All"), Filter::All), seg(tk("TCP"), Filter::Tcp), seg(tk("UDP"), Filter::Udp)], palette));

        let rows: Vec<AnyElement> = shown.iter().map(|&i| row(i, m, palette, row_r)).collect();
        let list_h = if rows.is_empty() { EMPTY_H } else { (rows.len() as f32 * (ROW_H + ROW_GAP)).min(LIST_MAX_H) };
        let list: AnyElement = if !sampled {
            empty_state("plug", t("Reading sockets…"), t("Looking at /proc/net"), EMPTY_H, palette).into_any_element()
        } else if rows.is_empty() && total == 0 {
            empty_state("plug", t("Nothing is listening"), t("Start a dev server and it shows up here"), EMPTY_H, palette).into_any_element()
        } else if rows.is_empty() {
            empty_state("search", tf("No port matches \u{201c}{query}\u{201d}", &[("query", &query.trim())]), t("Try a port number or a process name"), EMPTY_H, palette).into_any_element()
        } else {
            let fill = crate::shell::frame_color(palette);
            let clipped = list_h >= LIST_MAX_H - SIZE_EPSILON_PX;
            div()
                .relative()
                .h(px(list_h))
                .child(div().id("ports-list").size_full().overflow_y_scroll().flex().flex_col().gap(px(ROW_GAP)).children(rows))
                .when(clipped, |el| el.child(div().absolute().left_0().right_0().bottom_0().h(px(LIST_FADE_PX)).bg(oklab_gradient(ANGLE_DOWN_DEG, fill.with_alpha(0.0), fill))))
                .into_any_element()
        };

        let height = PAD * 2.0 + HEADER_H + GAP + STATS_H + GAP + CONTROL_H + GAP + list_h;
        if (f32::from(self.size_cell.get().height) - height).abs() > SIZE_EPSILON_PX {
            self.size_cell.set(size(px(PANEL_W), px(height)));
        }
        let p = &self.p;
        div()
            .relative()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(reveal(p, 0, head))
            .child(reveal(p, 1, stats))
            .child(reveal(p, 2, controls))
            .child(reveal(p, 3, list))
            .when_some(flash, |el, (ok, msg)| el.child(toast(ok, msg, PAD, palette)))
            .into_any_element()
    }
}

fn chip(m: &Shared, palette: &MandaraPalette, window: &Window, cx: &mut App) -> AnyElement {
    keep_alive(m, window, cx);
    refresh(m, false, cx);
    let count = m.borrow().tcp_count;
    let busy = count.is_some_and(|n| n > 0);
    let tint = if busy { palette.on_surface } else { palette.on_surface.with_alpha(CHIP_IDLE_ALPHA) };
    div()
        .flex()
        .items_center()
        .gap(px(CHIP_GAP_PX))
        .child(icon("plug", crate::style::bar_icon(CHIP_ICON_PX), tint))
        .when_some(count, |el, n| el.child(div().font_features(tabular_nums()).text_size(px(CHIP_TEXT_PX)).font_weight(FontWeight::MEDIUM).text_color(tint.hsla()).child(n.to_string())))
        .into_any_element()
}

// ── Commands ─────────────────────────────────────────────────────────────

/// Runs `f` on its own thread; the CLI receives its JSON.
fn pending(f: impl FnOnce() -> Result<serde_json::Value, String> + Send + 'static) -> Output {
    let (tx, rx) = async_channel::bounded(1);
    let _ = std::thread::Builder::new().name("mandara-ports-cmd".into()).spawn(move || {
        let _ = tx.send_blocking(f().map(|v| serde_json::to_string_pretty(&v).unwrap_or_default()));
    });
    Output::Pending(rx)
}

/// Ends every process listening on `port` (SIGTERM).
fn kill_port(port: u16) -> Result<serde_json::Value, String> {
    let listeners = svc::listeners(&mut HashMap::new());
    let on_port: Vec<&Listener> = listeners.iter().filter(|l| l.port == port).collect();
    if on_port.is_empty() {
        return Err(tf("Nothing is listening on port {port}", &[("port", &port)]));
    }
    let mut pids: Vec<(u32, String)> = on_port.iter().filter_map(|l| Some((l.pid?, l.process.clone().unwrap_or_default()))).collect();
    pids.sort();
    pids.dedup();
    if pids.is_empty() {
        return Err(tf("Port {port} belongs to another user's process; end it with your system tools", &[("port", &port)]));
    }
    let mut ended = Vec::new();
    for (pid, name) in &pids {
        mandara_services::processes::kill(*pid, false).map_err(|e| tr(&e).to_string())?;
        ended.push(json!({ "pid": pid, "process": name }));
    }
    Ok(json!({ "port": port, "signal": "TERM", "ended": ended }))
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Listening ports"), tk("Which servers listen on which ports, who can reach them, and a quick way to end them"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let m = model();
        let views = m.borrow().views.clone();
        let field = cx.new(|cx| TextField::new(String::new(), t("Port or process…"), true, Rc::new(move |_, _, cx| views.notify(cx)), cx));
        let bar_model = m.clone();
        let bar = fn_view(None, move |palette, window, cx| chip(&bar_model, palette, window, cx), window, cx);
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(PAD * 2.0 + HEADER_H + GAP + STATS_H + GAP + CONTROL_H + GAP + EMPTY_H))));
        let view = View { p: panel_cx.clone(), m, field, size_cell: size_cell.clone() };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
    });
    r.command(
        ID,
        CommandSpec::new_output("ports list", tk("Listening TCP ports and unconnected UDP sockets with their process, scope and address"), |_, _| {
            Ok(pending(|| Ok(json!(svc::listeners(&mut HashMap::new())))))
        })
        .example("ports list"),
    );
    r.command(
        ID,
        CommandSpec::new_output("ports kill", tk("End the process listening on a port (SIGTERM)"), |a, _| {
            let port = u16::try_from(a.int(0)?).map_err(|_| t("expected a port between 1 and 65535").to_string())?;
            Ok(pending(move || kill_port(port)))
        })
        .arg(ArgSpec::required("port", ArgKind::Integer, tk("port number")))
        .example("ports kill 3000"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn listener(proto: Proto, port: u16, scope: Scope, pid: Option<u32>, name: Option<&str>) -> Listener {
        Listener {
            proto,
            port,
            scope,
            addresses: vec![IpAddr::from([127, 0, 0, 1])],
            uid: 1000,
            pid,
            process: name.map(str::to_string),
            command: name.map(|n| format!("{n} --serve")),
        }
    }

    #[test]
    fn search_matches_port_process_and_address() {
        let l = listener(Proto::Tcp, 5173, Scope::Localhost, Some(4242), Some("node"));
        for q in ["", "517", "node", "--serve", "4242", "127.0"] {
            assert!(matches(&l, q), "{q}");
        }
        assert!(!matches(&l, "python"));
        assert!(!matches(&l, "42"), "a PID matches only whole");
    }

    #[test]
    fn rows_are_cached_per_sample_query_and_filter() {
        let mut m = Model::new();
        m.panel_seen = Some(Instant::now());
        m.absorb(vec![listener(Proto::Tcp, 22, Scope::AllInterfaces, None, None), listener(Proto::Udp, 5353, Scope::AllInterfaces, None, None), listener(Proto::Tcp, 3000, Scope::Localhost, Some(7), Some("vite"))]);
        assert_eq!(m.tcp_count, Some(2));
        assert_eq!(m.shown("").len(), 3);
        m.filter = Filter::Udp;
        assert_eq!(m.shown(""), [1]);
        m.filter = Filter::All;
        assert_eq!(m.shown("vite"), [2]);
        assert_eq!(m.rows[0].name.as_ref(), "System process");
        m.free_panel();
        assert!(m.listeners.is_empty() && m.shown.is_none() && m.full_at.is_none());
        assert_eq!(m.tcp_count, Some(2), "the chip keeps its count");
    }

    #[test]
    fn row_keys_are_distinct() {
        let a = RowKey(Proto::Tcp, 3000, Some(1));
        let b = RowKey(Proto::Udp, 3000, Some(1));
        let c = RowKey(Proto::Tcp, 3000, None);
        assert_ne!(a.element("x"), b.element("x"));
        assert_ne!(a.element("x"), c.element("x"));
    }
}
