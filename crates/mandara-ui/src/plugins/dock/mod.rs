//! The app dock (built-in `dock` plugin, opt-in), modelled on the macOS
//! Dock: pinned, running and recent apps, minimized windows, stacks and
//! the Trash on a chromatic-glass strip at the bottom, left or right edge
//! of each chosen display, as its own `mandara-dock` layer surface.
//!
//! * Icons magnify under the pointer (a Gaussian falloff driven through a
//!   spring per icon), running apps carry dots, and badges show unread
//!   notifications, progress or the window count.
//! * Click focuses the app (cycling its windows, or minimizing when it is
//!   already focused with one window) or launches it — the icon bounces
//!   until a window maps; urgent apps bounce for attention. Middle-click
//!   opens a new instance; right-click opens the macOS-style menu
//!   ([`menu`]): windows, actions, Options ▸ Keep in Dock / Open at Login /
//!   Show in Files, Show All Windows, Hide, Quit.
//! * Drag icons to reorder, drag a running app into the pinned apps to keep
//!   it, drag an icon away from the dock to remove it (poof). Drop files on
//!   an app to open them with it, on the Trash to trash them; installable
//!   apps dropped on the dock go to the app installer ([`install`]).
//! * Auto-hide: never, always (reveal at the edge) or smart (only while a
//!   window overlaps the dock or one is fullscreen on its output). Hidden,
//!   the input region shrinks to a thin strip along the screen edge so
//!   clicks fall through (the Caelestia input-masking law). While one of
//!   its popovers is open (a stack, the Applications grid, a menu, the app
//!   picker) the dock stays out under it ([`model::Reveal`]).

pub mod applications;
pub mod install;
pub mod menu;
pub mod model;
pub mod picker;
pub mod sheet;
pub mod stacks;

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, DispatchPhase, DragMoveEvent, Entity, ExternalPaths, FontWeight, Global, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions, canvas, div, point, prelude::FluentBuilder as _, px, size,
};
use mandara_config::workflow::{DockAlignment, DockAutoHide, DockConfig, DockIndicator, DockPosition};
use mandara_core::{FrameClock, SpringConfig, SpringValue, SurfaceRole, ToGpui};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginInstance, PluginManifest, PluginRegistry};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use crate::apps::App as DesktopApp;
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::text;
use crate::plugins::panel_fx::{elevation, glow, readable, rim, wash, WASH_SOFT};
use crate::state::{config, shell, update_config};
use crate::windows::Windows;

pub use model::{Item, Kind, drop_index, magnification, reorder};

pub const ID: &str = "dock";

/// Springs: icon scale, hide slide (scaled by `hide_speed`), dragged icon.
const SCALE_SPRING: SpringConfig = SpringConfig::new(520.0, 34.0, 1.0);
const HIDE_STIFFNESS: f32 = 300.0;
const HIDE_DAMPING: f32 = 28.0;
/// Pointer travel (logical px) before a press on an icon drags it.
const DRAG_THRESHOLD: f32 = 6.0;
/// An icon dragged this far past the dock's inner edge is removed on release.
const REMOVE_LIFT_PX: f32 = 40.0;
/// Room above magnified icons for the name tooltip and bounces, logical px.
const TOOLTIP_SPACE: f32 = 34.0;
const TOOLTIP_H: f32 = 24.0;
/// Tooltip width estimate (average glyph advance + padding), minimum width
/// and gap to the icon.
const TOOLTIP_CHAR_PX: f32 = 7.2;
const TOOLTIP_PAD_PX: f32 = 24.0;
const TOOLTIP_MIN_W: f32 = 48.0;
const TOOLTIP_MAX_W: f32 = 420.0;
const TOOLTIP_GAP: f32 = 6.0;
/// Thickness of the strip along the screen edge that reveals a hidden dock.
const REVEAL_STRIP_PX: f32 = 3.0;
/// Running indicator: dot size, bar size, offset from the dock's outer edge.
const DOT_PX: f32 = 4.0;
const DOT_GAP_PX: f32 = 3.0;
const BAR_W_PX: f32 = 10.0;
const BAR_W_FOCUSED_PX: f32 = 18.0;
const INDICATOR_INSET_FRAC: f32 = 0.32;
/// Most indicator dots per app.
const MAX_DOTS: usize = 3;
/// Badges: size, overlap with the icon's corner, padding, text.
const BADGE_PX: f32 = 18.0;
const BADGE_INSET_FRAC: f32 = 0.72;
const BADGE_PAD_PX: f32 = 5.0;
const BADGE_TEXT_PX: f32 = 10.0;
/// Progress bar on an icon: height, inset from the icon's sides and bottom.
const PROGRESS_H: f32 = 5.0;
const PROGRESS_INSET_FRAC: f32 = 0.14;
const PROGRESS_TRACK_ALPHA: f32 = 0.35;
/// Install placeholder: ring stroke, icon dim.
const RING_STROKE: f32 = 3.5;
const INSTALL_ICON_FRAC: f32 = 0.6;
const INSTALL_DIM: f32 = 0.55;
/// Failed install shake: amplitude (px), frequency (Hz) and duration.
const SHAKE_PX: f32 = 6.0;
const SHAKE_HZ: f32 = 9.0;
const SHAKE_S: f32 = 0.55;
/// Smallest icon size honoured, logical px.
const MIN_ICON_PX: f32 = 16.0;
/// The hide spring may overshoot this far; hidden docks slide this much
/// past their own thickness.
const HIDE_OVERSHOOT: f32 = 1.2;
const HIDE_EXTRA_PX: f32 = 2.0;
/// Alphas: separator, idle indicator; focused indicator glow `(alpha, blur)`.
const SEPARATOR_ALPHA: f32 = 0.18;
const INDICATOR_ALPHA: f32 = 0.55;
const FOCUS_GLOW: (f32, f32) = (0.5, 6.0);
/// Drop highlight on an icon and the "Drop to install" glow `(alpha, blur)`.
const DROP_ALPHA: f32 = 0.22;
const INSTALL_GLOW: (f32, f32) = (0.55, 18.0);
/// Separator: length as a fraction of the icon, thickness.
const SEPARATOR_FRAC: f32 = 0.6;
const SEPARATOR_PX: f32 = 1.0;
/// Glyph size inside the launcher / trash tiles, and their corner radius,
/// as fractions of the icon.
const GLYPH_FRAC: f32 = 0.5;
/// The media chip along a bottom dock, in icon slots (art, two text lines,
/// the play button).
const MEDIA_CHIP_SLOTS: f32 = 4.6;
/// Media chip: inner gap, title / artist text sizes, play button share of
/// the chip's height and its glyph share.
const MEDIA_GAP_PX: f32 = 8.0;
const MEDIA_TITLE_PX: f32 = text::BODY_S;
const MEDIA_ARTIST_PX: f32 = text::LABEL;
const MEDIA_PLAY_FRAC: f32 = 0.62;
const MEDIA_PLAY_GLYPH: f32 = 0.5;
pub(crate) const TILE_RADIUS_FRAC: f32 = 0.3;
/// Stack tiles: the newest file's size over the folder, folder alpha.
pub(crate) const STACK_TOP_FRAC: f32 = 0.62;
pub(crate) const STACK_FOLDER_ALPHA: f32 = 0.55;
/// Minimized-window tile: corner glyph size.
const MINIMIZED_GLYPH_FRAC: f32 = 0.34;
/// Dragged icon opacity, and while it is about to be removed.
const DRAG_ALPHA: f32 = 0.9;
const REMOVE_ALPHA: f32 = 0.5;
/// The "poof" of a removed icon: duration, puffs, spread and size.
const POOF_S: f32 = 0.42;
const POOF_PUFFS: usize = 6;
const POOF_SPREAD_PX: f32 = 26.0;
const POOF_PUFF_PX: f32 = 16.0;
/// A smart-hiding dock re-checks window geometry this often (windows move
/// without events).
const SMART_POLL: Duration = Duration::from_millis(400);
/// A launch bounce stops after this long without a window.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(12);
/// A just-opened dock (switched on, moved) is out this long whatever the
/// hiding mode, so turning it on always shows it.
const SHOW_ON_OPEN: Duration = Duration::from_secs(3);
/// How long `dock show` keeps a hidden dock out.
const SHOW_FOR: Duration = Duration::from_secs(3);
/// Apps remembered as recently closed.
const CLOSED_CAP: usize = 24;
/// Launcher usage rows read for recent apps.
const USAGE_ROWS: usize = 24;

// ── Shared state ─────────────────────────────────────────────────────────

/// State every dock shares (one per shell, not per display).
#[derive(Default)]
struct Shared {
    /// Unpinned while running: keep the icon until it quits.
    linger: Vec<String>,
    /// Apps that quit this session, oldest first.
    closed: Vec<String>,
    /// Running desktop ids / app ids last time (to notice quits).
    running: Vec<String>,
    /// Recent apps, newest first (closed apps, then launcher usage).
    recent: Vec<String>,
    /// Launch bounces by item key.
    launching: HashMap<String, Instant>,
    /// Install landings (a single hop) by item key.
    landing: HashMap<String, Instant>,
    /// Install jobs already pinned, and when failures were first seen.
    pinned_jobs: Vec<u64>,
    failed_since: HashMap<u64, Instant>,
    installs: Vec<model::InstallSlot>,
    /// Bumped whenever any of the above changes.
    generation: u64,
}
impl Global for Shared {}

fn shared(cx: &mut App) -> &mut Shared {
    cx.default_global::<Shared>()
}

/// Open dock popovers by output ("" = any): the dock there stays out
/// while one is open.
#[derive(Default)]
struct Popovers(Vec<(String, AnyWindowHandle)>);
impl Global for Popovers {}

/// A popover opened over `monitor`'s dock.
pub(crate) fn popover_opened(monitor: &str, handle: AnyWindowHandle, cx: &mut App) {
    cx.default_global::<Popovers>().0.push((monitor.to_string(), handle));
    notify_docks(cx);
}

/// A popover started closing (or went away): its dock may hide again.
pub(crate) fn popover_closed(handle: AnyWindowHandle, cx: &mut App) {
    let p = cx.default_global::<Popovers>();
    let before = p.0.len();
    p.0.retain(|(_, h)| h.window_id() != handle.window_id());
    if p.0.len() != before {
        notify_docks(cx);
    }
}

/// Whether a popover pins `monitor`'s dock out (popovers that vanished
/// without closing, e.g. with their output, don't count).
fn pinned_on(monitor: &str, cx: &mut App) -> bool {
    if cx.try_global::<Popovers>().is_none_or(|p| p.0.is_empty()) {
        return false;
    }
    let live = cx.windows();
    let p = cx.global_mut::<Popovers>();
    p.0.retain(|(_, h)| live.iter().any(|w| w.window_id() == h.window_id()));
    p.0.iter().any(|(m, _)| m == monitor || m.is_empty())
}

fn notify_docks(cx: &mut App) {
    each_dock(cx, |_, cx| cx.notify());
}

/// Gap between a popover and its dock icon, logical px.
const POPOVER_GAP: f32 = 12.0;

pub(crate) fn sheet_gap() -> f32 {
    POPOVER_GAP
}

/// Icon ids the docks show (a closing popover keeps their images cached).
pub(crate) fn dock_icon_ids(cx: &mut App) -> Vec<String> {
    let ids = std::cell::RefCell::new(Vec::new());
    each_dock(cx, |v, _| ids.borrow_mut().extend(v.list.iter().map(|i| i.icon_id.clone())));
    ids.into_inner()
}

/// Desktop ids installed from the dock this session.
pub(crate) fn landed_ids(cx: &App) -> Vec<String> {
    cx.try_global::<Shared>().map(|s| s.landing.keys().cloned().collect()).unwrap_or_default()
}

/// The monitor and output rectangle of dock item `key` (the first dock
/// showing it), for opening its popover from the command line.
fn item_anchor(key: &str, cx: &mut App) -> Option<(String, (f32, f32, f32, f32))> {
    let found = std::cell::RefCell::new(None);
    each_dock(cx, |v, cx| {
        if found.borrow().is_none()
            && let Some(r) = v.rect(key)
        {
            *found.borrow_mut() = Some((v.monitor.clone(), v.to_output(r, cx)));
        }
    });
    found.into_inner()
}

/// Keeps `key`'s icon while it runs although it was just unpinned.
pub(crate) fn linger(key: &str, cx: &mut App) {
    let s = shared(cx);
    if !s.linger.iter().any(|l| l == key) {
        s.linger.push(key.to_string());
        s.generation += 1;
    }
    refresh_all(cx);
}

/// Every dock rebuilds its slots on the next frame.
pub(crate) fn refresh_all(cx: &mut App) {
    each_dock(cx, |v, cx| {
        v.dirty = true;
        cx.notify();
    });
}

/// Launches `item` (or one of its desktop actions) and starts its bounce.
pub(crate) fn launch_item(item: &Item, action: Option<&crate::apps::DesktopAction>, cx: &mut App) {
    let snapshot = config(cx).read(cx).snapshot();
    let Some(app) = &item.desktop else {
        tracing::warn!("dock: no desktop entry for {}", item.key);
        return;
    };
    if let Err(err) = crate::apps::launch_with(app, action, &snapshot.launch_options()) {
        tracing::warn!("dock: {err}");
        return;
    }
    crate::launcher_ext::record_use(&format!("app:{}", app.id), &snapshot);
    if snapshot.dock.bounce_on_launch && item.windows.is_empty() {
        shared(cx).launching.insert(item.key.clone(), Instant::now());
        refresh_all(cx);
    }
}

/// Opens `files` with `item`'s app.
fn open_files_with(item: &Item, files: &[PathBuf], cx: &mut App) {
    let Some(app) = &item.desktop else { return };
    let action = crate::apps::DesktopAction { id: String::new(), name: app.name.clone(), exec: model::exec_with_files(&app.exec, files) };
    let options = config(cx).read(cx).get().launch_options();
    if let Err(err) = crate::apps::launch_with(app, Some(&action), &options) {
        tracing::warn!("dock: {err}");
    }
}

pub(crate) fn open_trash() {
    std::thread::spawn(|| {
        if let Err(err) = mandara_services::session::open_uri("trash:///") {
            tracing::warn!("trash: {err}");
        }
    });
}

/// Follows windows for recent apps, lingering icons and launch bounces
/// (once per shell).
fn track_windows(cx: &mut App) {
    let windows = crate::windows::init(cx);
    let apps: Rc<[DesktopApp]> = crate::apps::discover().into();
    let update = move |cx: &mut App| {
        let list = crate::windows::init(cx).read(cx).list().to_vec();
        let now: Vec<String> = {
            let mut ids: Vec<String> = list.iter().filter(|w| !crate::windows::is_shell(&w.app_id)).map(|w| crate::windows::desktop_entry_for(&w.app_id, &apps).map(|a| a.id.clone()).unwrap_or_else(|| w.app_id.clone())).collect();
            ids.sort();
            ids.dedup();
            ids
        };
        let usage = crate::launcher_ext::usage_list(&config(cx).read(cx).snapshot(), USAGE_ROWS);
        let s = shared(cx);
        let before = std::mem::replace(&mut s.running, now.clone());
        model::note_closed(&mut s.closed, &before, &now, CLOSED_CAP);
        s.recent = model::recent_ids(&s.closed, &usage);
        let running = |k: &str| list.iter().any(|w| crate::windows::same_app(k, &w.app_id)) || now.iter().any(|n| n.eq_ignore_ascii_case(k));
        s.linger.retain(|k| running(k));
        s.launching.retain(|k, t| !running(k) && t.elapsed() < LAUNCH_TIMEOUT);
        s.generation += 1;
        refresh_all(cx);
    };
    update(cx);
    cx.observe(&windows, move |_, cx| update(cx)).detach();
}

/// Follows the installer's queue: placeholders, pins on success.
pub(crate) fn follow_installs(cx: &mut App) {
    let Some(bridge) = install::bridge(cx) else { return };
    let sync: install::Callback = Rc::new(|cx: &mut App| {
        let jobs = install::jobs(cx);
        let s = shared(cx);
        let done = install::finished(&jobs, &s.pinned_jobs);
        for (id, _) in &done {
            s.pinned_jobs.push(*id);
        }
        let now = Instant::now();
        for j in &jobs {
            if matches!(j.state, install::JobState::Failed(_)) {
                s.failed_since.entry(j.id).or_insert(now);
            }
        }
        let failed_since = s.failed_since.clone();
        s.installs = install::slots(&jobs).into_iter().filter(|j| j.failed.is_none() || failed_since.get(&j.id).is_none_or(|t| t.elapsed() < install::FAILED_FOR)).collect();
        for (_, desktop) in &done {
            s.landing.insert(desktop.clone(), now);
        }
        s.generation += 1;
        if !done.is_empty() {
            update_config(cx, move |c| {
                for (_, d) in &done {
                    c.dock.pin(d);
                }
            });
        }
        refresh_all(cx);
    });
    (bridge.observe)(cx, sync.clone());
    sync(cx);
}

// ── Geometry ─────────────────────────────────────────────────────────────

/// Cross-axis size of the surface: margin, dock, magnified overflow and
/// tooltip / bounce room.
fn surface_thickness(cfg: &DockConfig) -> f32 {
    let dock = cfg.icon_size + cfg.padding * 2.0;
    let grow = if cfg.magnification { cfg.icon_size * (cfg.magnify - 1.0).max(0.0) } else { 0.0 };
    let bounce = cfg.icon_size * model::HOP_FRAC;
    (cfg.margin + dock + grow.max(bounce) + TOOLTIP_SPACE).ceil()
}

/// `origin` of a `size`-px box moved inside the surface, `room` px clear
/// of the surface's inner edge (the dragged icon and its poof live in the
/// dock's own surface).
fn clamp_into(pos: DockPosition, surface: (f32, f32), origin: (f32, f32), size: f32, room: f32) -> (f32, f32) {
    let (mut x, mut y) = origin;
    match pos {
        DockPosition::Bottom => y = y.clamp(room, (surface.1 - size).max(room)),
        DockPosition::Left => x = x.clamp(0.0, (surface.0 - size - room).max(0.0)),
        DockPosition::Right => x = x.clamp(room, (surface.0 - size).max(room)),
    }
    (x.clamp(0.0, (surface.0 - size).max(0.0)), y.clamp(0.0, (surface.1 - size).max(0.0)))
}

fn axis_centre(pos: DockPosition, r: (f32, f32, f32, f32)) -> f32 {
    match pos {
        DockPosition::Bottom => r.0 + r.2 / 2.0,
        _ => r.1 + r.3 / 2.0,
    }
}

// ── View ─────────────────────────────────────────────────────────────────

struct Drag {
    key: String,
    start: (f32, f32),
    at: (f32, f32),
    active: bool,
    /// Far enough from the dock to be removed on release.
    remove: bool,
    /// Apps and stacks move; other slots only take the click.
    movable: bool,
}

/// A removed icon's poof: when, where (surface coords).
struct Poof(Instant, (f32, f32));

pub struct DockView {
    monitor: String,
    /// The output's position in the global layout (smart hide).
    origin: (f32, f32),
    windows: Entity<Windows>,
    apps: Rc<[DesktopApp]>,
    /// The sanitized config and the slots built from it (rebuilt when
    /// windows, config, notifications or the shared state change — not on
    /// animation frames).
    cfg: DockConfig,
    list: Rc<Vec<Item>>,
    badges: HashMap<String, model::Badge>,
    urgent: Vec<String>,
    dirty: bool,
    generation: u64,
    scales: HashMap<String, SpringValue<f32>>,
    hide: SpringValue<f32>,
    clock: FrameClock,
    /// Pointer position along the dock axis (surface coords), when inside.
    pointer: Option<f32>,
    hovered: Option<String>,
    /// Out regardless of hiding until this instant (`dock show`, just opened).
    shown_until: Option<Instant>,
    /// Pointer reveal, delays and popover pinning.
    reveal: model::Reveal,
    reveal_task: Option<gpui::Task<()>>,
    hide_task: Option<gpui::Task<()>>,
    drag: Option<Drag>,
    poof: Option<Poof>,
    /// A dragged file is an installable app (the "Drop to install" glow).
    install_hover: bool,
    /// Item rectangles last frame (surface coords) by key.
    rects: Vec<(String, (f32, f32, f32, f32))>,
    region: Vec<(i32, i32, i32, i32)>,
    /// Launcher customizations seen (a change rescans the apps).
    cfg_launcher_overrides: usize,
    /// The glass rectangle last frame (surface coords), and the surface.
    glass: (f32, f32, f32, f32),
    surface: (f32, f32),
    /// The media chip's album art, decoded at the dock's icon size.
    media_cover: Rc<std::cell::RefCell<crate::plugins::media::CoverSlot>>,
}

impl DockView {
    fn new(monitor: String, origin: (f32, f32), reveal: bool, cx: &mut Context<Self>) -> Self {
        let windows = crate::windows::init(cx);
        let mark = |this: &mut Self, cx: &mut Context<Self>| {
            this.dirty = true;
            cx.notify();
        };
        cx.observe(&windows, move |this, _, cx| mark(this, cx)).detach();
        cx.observe(&config(cx), |this: &mut Self, store, cx| {
            let next = store.read(cx).get();
            // Rescan apps only when it can matter: launcher customizations,
            // or a pin of an app not seen yet (just installed).
            let unknown = next.dock.pinned.iter().any(|p| !this.apps.iter().any(|a| a.id.eq_ignore_ascii_case(p)));
            if unknown || next.launcher.overrides.len() != this.cfg_launcher_overrides {
                this.apps = crate::apps::discover().into();
                this.cfg_launcher_overrides = next.launcher.overrides.len();
            }
            this.dirty = true;
            cx.notify();
        })
        .detach();
        cx.observe(&crate::hub::hub(cx), move |this, _, cx| mark(this, cx)).detach();
        cx.observe(&mandara_core::theme_entity(cx), |_, _, cx| cx.notify()).detach();
        // The launcher floats above the dock: keep the dock out meanwhile.
        cx.observe(&shell(cx), |_, _, cx| cx.notify()).detach();
        // Smart hide follows window geometry, which moves without events.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(SMART_POLL).await;
                let alive = this.update(cx, |this, cx| {
                    if this.cfg.auto_hide == DockAutoHide::Smart {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        let cfg = config(cx).read(cx).get();
        let overrides = cfg.launcher.overrides.len();
        let dock = cfg.dock_sanitized();
        Self {
            monitor,
            origin,
            windows,
            apps: crate::apps::discover().into(),
            cfg: dock,
            list: Rc::new(Vec::new()),
            badges: HashMap::new(),
            urgent: Vec::new(),
            dirty: true,
            generation: 0,
            scales: HashMap::new(),
            hide: SpringValue::new(1.0, SpringConfig::new(HIDE_STIFFNESS, HIDE_DAMPING, 1.0)),
            clock: FrameClock::new(),
            pointer: None,
            hovered: None,
            shown_until: reveal.then(|| Instant::now() + SHOW_ON_OPEN),
            reveal: model::Reveal::default(),
            reveal_task: None,
            hide_task: None,
            drag: None,
            poof: None,
            install_hover: false,
            rects: Vec::new(),
            region: Vec::new(),
            cfg_launcher_overrides: overrides,
            glass: (0.0, 0.0, 0.0, 0.0),
            surface: (0.0, 0.0),
            media_cover: Rc::default(),
        }
    }

    /// Rebuilds the slots, badges and urgency when something changed.
    fn refresh(&mut self, cx: &mut App) {
        let generation = cx.try_global::<Shared>().map(|s| s.generation).unwrap_or(0);
        if !self.dirty && generation == self.generation {
            return;
        }
        self.dirty = false;
        self.generation = generation;
        self.cfg = config(cx).read(cx).get().dock_sanitized();
        let s = cx.default_global::<Shared>();
        let (linger, recent, installs) = (s.linger.clone(), s.recent.clone(), s.installs.clone());
        let windows = self.windows.read(cx).list();
        let media = crate::cards::MediaPresence::of(cx).active;
        let list = model::items(&model::Inputs { cfg: &self.cfg, windows, apps: &self.apps, monitor: &self.monitor, linger: &linger, recent: &recent, installing: &installs, media });
        let hub = crate::hub::hub(cx).read(cx);
        self.badges.clear();
        if self.cfg.notification_badges || self.cfg.progress_badges || self.cfg.bounce_on_attention {
            for it in list.iter().filter(|i| i.is_app()) {
                let desktop = it.desktop.as_ref().map(|d| d.id.as_str()).unwrap_or("");
                let ids: Vec<&str> = it.windows.iter().map(|w| w.app_id.as_str()).chain(std::iter::once(it.key.as_str())).collect();
                let b = model::badge(&hub.notifications.items, desktop, &it.name, &ids);
                if b != model::Badge::default() {
                    self.badges.insert(it.key.clone(), b);
                }
            }
        }
        self.urgent = model::urgent_apps(&hub.clients, &hub.navigation);
        self.list = Rc::new(list);
    }

    /// Something overlaps the dock (smart hide): a fullscreen window on this
    /// output, or a window whose geometry intersects the dock.
    fn obstructed(&self, dock: (f32, f32, f32, f32), cx: &App) -> bool {
        let list = self.windows.read(cx).list();
        if list.iter().any(|w| w.fullscreen && !w.minimized && !crate::windows::is_shell(&w.app_id) && (w.outputs.is_empty() || w.outputs.contains(&self.monitor))) {
            return true;
        }
        let hub = crate::hub::hub(cx).read(cx);
        let global = (dock.0 + self.origin.0, dock.1 + self.origin.1, dock.2, dock.3);
        hub.clients.iter().filter(|c| c.monitor == self.monitor || self.monitor.is_empty()).filter(|c| crate::switcher::client_visible(c, hub)).any(|c| model::obstructs(&c.app_id, c.geometry, global))
    }

    fn set_inside(&mut self, inside: bool, cx: &mut Context<Self>) {
        if !self.reveal.pointer(inside) {
            return;
        }
        if !inside {
            self.pointer = None;
            self.hovered = None;
        }
        self.sync_timers(cx);
        cx.notify();
    }

    /// Runs the show / hide delays [`model::Reveal`] has pending.
    fn sync_timers(&mut self, cx: &mut Context<Self>) {
        let (show_ms, hide_ms) = (self.cfg.show_delay_ms, self.cfg.hide_delay_ms);
        if !self.reveal.show_pending {
            self.reveal_task = None;
        } else if self.reveal_task.is_none() {
            self.reveal_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(show_ms)).await;
                let _ = this.update(cx, |this, cx| {
                    this.reveal.show_elapsed();
                    this.reveal_task = None;
                    cx.notify();
                });
            }));
        }
        if !self.reveal.hide_pending {
            self.hide_task = None;
        } else if self.hide_task.is_none() {
            self.hide_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(hide_ms)).await;
                let _ = this.update(cx, |this, cx| {
                    this.reveal.hide_elapsed();
                    this.hide_task = None;
                    cx.notify();
                });
            }));
        }
    }

    /// Follows this dock's popovers (pinned out while one is open) and the
    /// surface's own pointer enter / leave: GPUI only re-tests hover on
    /// motion inside a surface, so a pointer that left for a popover (or
    /// anywhere) would otherwise stay "inside" forever and the reveal strip
    /// would never schedule a reveal again.
    fn sync_reveal(&mut self, window: &Window, cx: &mut Context<Self>) {
        let pinned = pinned_on(&self.monitor, cx);
        if pinned != self.reveal.pinned {
            if pinned {
                self.reveal.pin();
            } else {
                self.reveal.unpin();
                // A full hide delay from now, not from an older leave.
                self.hide_task = None;
                self.pointer = None;
                self.hovered = None;
            }
            self.sync_timers(cx);
        }
        if self.reveal.inside && !window.is_window_hovered() && self.drag.is_none() {
            self.set_inside(false, cx);
        }
    }

    fn item(&self, key: &str) -> Option<Item> {
        self.list.iter().find(|i| i.key == key).cloned()
    }

    /// Output coordinates of a surface rect (menus and popovers cover the
    /// whole output).
    fn to_output(&self, r: (f32, f32, f32, f32), cx: &App) -> (f32, f32, f32, f32) {
        let (ow, oh) = output_size(&self.monitor, cx);
        let thick = surface_thickness(&self.cfg);
        match self.cfg.position {
            DockPosition::Bottom => (r.0, oh - thick + r.1, r.2, r.3),
            DockPosition::Left => r,
            DockPosition::Right => (ow - thick + r.0, r.1, r.2, r.3),
        }
    }

    fn rect(&self, key: &str) -> Option<(f32, f32, f32, f32)> {
        self.rects.iter().find(|(k, _)| k == key).map(|(_, r)| *r)
    }

    /// Left click: launcher / trash / stack / focus-cycle-minimize / launch.
    fn activate(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(item) = self.item(key) else { return };
        match item.kind {
            Kind::Launcher => {
                let s = shell(cx);
                let open = !s.read(cx).launcher_open;
                s.update(cx, |s, cx| s.set_launcher(open, cx));
            }
            Kind::Applications => {
                if let Some(r) = self.rect(key) {
                    let anchor = self.to_output(r, cx);
                    applications::toggle(&self.monitor, anchor, cx);
                }
            }
            Kind::Trash => open_trash(),
            Kind::Media => crate::plugins::media::send(cx, mandara_services::MediaCommand::PlayPause),
            Kind::Stack(i) => {
                if let Some(r) = self.rect(key) {
                    let anchor = self.to_output(r, cx);
                    stacks::toggle_popover(i, &self.monitor, anchor, cx);
                }
            }
            Kind::Minimized(id) => {
                self.windows.read(cx).send(mandara_services::toplevels::ToplevelCommand::SetMinimized(id, false));
                crate::windows::activate(id, cx);
            }
            Kind::Separator => {}
            Kind::Installing(id) => {
                // A failed install's placeholder goes away on click.
                shared(cx).installs.retain(|j| j.id != id || j.failed.is_none());
                shared(cx).generation += 1;
                cx.notify();
            }
            Kind::App => match item.windows.as_slice() {
                [] => launch_item(&item, None, cx),
                [only] if only.activated && !only.minimized => self.windows.read(cx).send(mandara_services::toplevels::ToplevelCommand::SetMinimized(only.id, true)),
                list => {
                    // Focused app: the next of its windows; else its latest.
                    let target = if item.focused() { list.get(1).unwrap_or(&list[0]) } else { &list[0] };
                    if target.minimized {
                        self.windows.read(cx).send(mandara_services::toplevels::ToplevelCommand::SetMinimized(target.id, false));
                    }
                    crate::windows::activate(target.id, cx);
                }
            },
        }
    }

    fn open_menu_for(&mut self, key: Option<&str>, force: bool, at: (f32, f32), cx: &mut Context<Self>) {
        let (item, space, rect) = match key.and_then(|k| self.item(k).map(|i| (i, self.rect(k)))) {
            Some((item, Some(rect))) if item.kind != Kind::Separator && !matches!(item.kind, Kind::Installing(_)) => (item, false, rect),
            _ => {
                // Anchored on the glass's inner edge, not the click.
                let at = match self.cfg.position {
                    DockPosition::Bottom => (at.0, self.glass.1),
                    DockPosition::Left => (self.glass.0 + self.glass.2, at.1),
                    DockPosition::Right => (self.glass.0, at.1),
                };
                let placeholder = Item { key: "@space".into(), icon_id: String::new(), name: t("Dock").into(), desktop: None, windows: Vec::new(), pinned: false, recent: false, kind: Kind::Separator };
                (placeholder, true, (at.0, at.1, 1.0, 1.0))
            }
        };
        let anchor = self.to_output(rect, cx);
        menu::open(item, space, force, self.monitor.clone(), anchor, cx);
    }

    fn on_down(&mut self, e: &MouseDownEvent, key: &str, cx: &mut Context<Self>) {
        let at = (f32::from(e.position.x), f32::from(e.position.y));
        match e.button {
            MouseButton::Left => {
                // A click is a press and a release on the same slot.
                let movable = self.item(key).is_some_and(|i| i.is_app() || matches!(i.kind, Kind::Stack(_)));
                self.drag = Some(Drag { key: key.to_string(), start: at, at, active: false, remove: false, movable });
            }
            MouseButton::Middle => {
                if let Some(item) = self.item(key).filter(|i| i.is_app()) {
                    launch_item(&item, None, cx);
                }
            }
            MouseButton::Right => self.open_menu_for(Some(key), e.modifiers.alt, at, cx),
            _ => {}
        }
    }

    fn on_move(&mut self, at: (f32, f32), surface: (f32, f32), cx: &mut Context<Self>) {
        let pos = self.cfg.position;
        self.pointer = Some(model::along(pos, at));
        let band = self.cfg.margin + self.cfg.icon_size + self.cfg.padding * 2.0;
        if let Some(d) = self.drag.as_mut() {
            d.at = at;
            if d.movable && !d.active && ((at.0 - d.start.0).powi(2) + (at.1 - d.start.1).powi(2)).sqrt() > DRAG_THRESHOLD {
                d.active = true;
            }
            let removable = self.list.iter().any(|i| i.key == d.key && (i.pinned || matches!(i.kind, Kind::Stack(_))));
            d.remove = d.active && removable && model::edge_distance(pos, surface, at) > band + REMOVE_LIFT_PX;
        }
        cx.notify();
    }

    fn on_up(&mut self, key: Option<&str>, cx: &mut Context<Self>) {
        match self.drag.take() {
            Some(d) if d.active => self.drop_drag(d, cx),
            Some(d) if key == Some(d.key.as_str()) => self.activate(&d.key, cx),
            Some(_) => {}
            // A release without a press here (the press began elsewhere).
            None => {}
        }
        cx.notify();
    }

    /// A dragged icon is released: removed (dragged away), reordered, or
    /// pinned (a running app dropped among the pinned ones).
    fn drop_drag(&mut self, d: Drag, cx: &mut Context<Self>) {
        let Some(item) = self.item(&d.key) else { return };
        if d.remove {
            let vp = self.surface;
            let (x, y) = clamp_into(self.cfg.position, vp, d.at, 0.0, POOF_SPREAD_PX + POOF_PUFF_PX);
            self.poof = (!mandara_core::spring::motion_profile().reduce).then(|| Poof(Instant::now(), (x, y)));
            match item.kind {
                Kind::Stack(i) => update_config(cx, move |c| {
                    if i < c.dock.stacks.len() {
                        c.dock.stacks.remove(i);
                    }
                }),
                _ => {
                    let id = item.pin_key();
                    if item.running() {
                        linger(&item.key, cx);
                    }
                    update_config(cx, move |c| {
                        c.dock.unpin(&id);
                    });
                }
            }
            return;
        }
        let pos = self.cfg.position;
        let along = model::along(pos, d.at);
        match item.kind {
            Kind::Stack(from) => {
                let centres: Vec<f32> = (0..self.cfg.stacks.len()).filter(|i| *i != from).filter_map(|i| self.rect(&format!("@stack-{i}")).map(|r| axis_centre(pos, r))).collect();
                let to = model::pin_drop_index(&centres, along);
                update_config(cx, move |c| {
                    if from < c.dock.stacks.len() {
                        let s = c.dock.stacks.remove(from);
                        c.dock.stacks.insert(to.min(c.dock.stacks.len()), s);
                    }
                });
            }
            _ => {
                let pinned: Vec<(String, f32)> = self.list.iter().filter(|i| i.pinned && i.key != d.key).filter_map(|i| self.rect(&i.key).map(|r| (i.key.clone(), axis_centre(pos, r)))).collect();
                let centres: Vec<f32> = pinned.iter().map(|(_, c)| *c).collect();
                // Dropped among (or next to) the pinned apps?
                let slack = self.cfg.icon_size;
                let within = match (centres.first(), centres.last()) {
                    (Some(a), Some(b)) => along >= a - slack && along <= b + slack,
                    _ => true,
                };
                if !within && !item.pinned {
                    return;
                }
                let to = model::pin_drop_index(&centres, along);
                let id = item.pin_key();
                let anchor = pinned.get(to).map(|(k, _)| k.clone());
                update_config(cx, move |c| {
                    // Insert before the pin that sat at `to` (indices shift
                    // once the dragged pin leaves its old place).
                    c.dock.unpin(&id);
                    let at = anchor.and_then(|k| c.dock.pin_index(&k)).unwrap_or(c.dock.pinned.len());
                    c.dock.pin_at(&id, at);
                });
            }
        }
    }

    fn on_drop_files(&mut self, key: Option<&str>, paths: &ExternalPaths, cx: &mut Context<Self>) {
        let files: Vec<PathBuf> = paths.paths().to_vec();
        self.install_hover = false;
        let installable: Vec<PathBuf> = files.iter().filter(|p| install::accepts(p, cx)).cloned().collect();
        let item = key.and_then(|k| self.item(k));
        match item.as_ref().map(|i| &i.kind) {
            Some(Kind::Trash) => {
                if let Err(err) = stacks::move_to_trash(&files) {
                    tracing::warn!("dock: {err}");
                }
            }
            Some(Kind::App) => open_files_with(item.as_ref().expect("item"), &files, cx),
            _ => {
                if !install::install(installable, cx) {
                    tracing::info!("dock: nothing to install in the dropped files");
                }
            }
        }
        cx.notify();
    }
}

impl Render for DockView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh(cx);
        self.sync_reveal(window, cx);
        let palette = mandara_core::frame_palette(window, cx);
        let f = fonts(cx);
        let cfg = &self.cfg;
        let reduce = mandara_core::spring::motion_profile().reduce;
        let vp = window.viewport_size();
        let surface = (f32::from(vp.width), f32::from(vp.height));
        let horizontal = cfg.position == DockPosition::Bottom;
        let extent = if horizontal { surface.0 } else { surface.1 };
        let base = cfg.icon_size.max(MIN_ICON_PX);
        // App icons decode once at the largest size magnification reaches
        // (crate::icon_raster): a size that springs never waits on a decode.
        let decode_px = base * if cfg.magnification { cfg.magnify.max(1.0) } else { 1.0 };
        let thickness = base + cfg.padding * 2.0;
        let list = self.list.clone();
        if !cx.has_active_drag() {
            self.install_hover = false;
        }

        // Magnification targets and slot lengths.
        let now = Instant::now();
        let dt = self.clock.tick(now);
        let mut moving = false;
        // The media chip is wider along a bottom dock (art, title, play).
        let media_len = if horizontal { base * MEDIA_CHIP_SLOTS } else { base };
        let slot = |kind: &Kind| match kind {
            Kind::Separator => cfg.spacing * 2.0 + SEPARATOR_PX,
            Kind::Media => media_len + cfg.spacing,
            _ => base + cfg.spacing,
        };
        let rest_len: f32 = (list.iter().map(|i| slot(&i.kind)).sum::<f32>() - cfg.spacing + cfg.padding * 2.0).max(thickness);
        let rest_start = match cfg.alignment {
            DockAlignment::Start => cfg.margin.max(cfg.padding),
            DockAlignment::Center => (extent - rest_len) / 2.0,
            DockAlignment::End => extent - rest_len - cfg.margin.max(cfg.padding),
        };
        // Pointer distance is measured against resting centres (so icons
        // don't chase the pointer as they grow).
        let mut cursor = rest_start + cfg.padding;
        let mut rest_centres = Vec::with_capacity(list.len());
        for it in list.iter() {
            let s = slot(&it.kind);
            rest_centres.push(cursor + (s - cfg.spacing) / 2.0);
            cursor += s;
        }
        let dragging = self.drag.as_ref().filter(|d| d.active).map(|d| d.key.clone());
        let mut scales = Vec::with_capacity(list.len());
        for (it, c) in list.iter().zip(&rest_centres) {
            let target = match (self.pointer, cfg.magnification, &it.kind) {
                (Some(p), true, k) if !matches!(k, Kind::Separator | Kind::Media) && dragging.is_none() => magnification((p - c) / base, cfg.magnify, cfg.magnify_spread),
                _ => 1.0,
            };
            let spring = self.scales.entry(it.key.clone()).or_insert_with(|| SpringValue::new(1.0, SCALE_SPRING));
            spring.set_instant(reduce);
            spring.set_target(target);
            moving |= spring.step(dt);
            scales.push(if it.kind == Kind::Separator { 1.0 } else { spring.value().max(1.0) });
        }
        if self.scales.len() > list.len() {
            self.scales.retain(|k, _| list.iter().any(|i| &i.key == k));
        }
        let len: f32 = (list.iter().zip(&scales).map(|(i, s)| if matches!(i.kind, Kind::Separator | Kind::Media) { slot(&i.kind) } else { base * s + cfg.spacing }).sum::<f32>() - cfg.spacing + cfg.padding * 2.0).max(thickness);
        // Grow symmetrically around the resting centre (start-aligned docks grow away from the start).
        let start = match cfg.alignment {
            DockAlignment::Start => rest_start,
            DockAlignment::Center => (extent - len) / 2.0,
            DockAlignment::End => rest_start + rest_len - len,
        };

        // Visibility.
        let dock_rect = model::place(cfg.position, surface, cfg.margin, rest_start, 0.0, rest_len, thickness);
        let held_out = self.shown_until.is_some_and(|t| now < t);
        let forced = held_out || dragging.is_some() || self.install_hover || stays_out_for_launcher(cfg, shell(cx).read(cx).launcher_open);
        let obstructed = cfg.auto_hide == DockAutoHide::Smart && self.obstructed(dock_rect, cx);
        let visible = self.reveal.visible(cfg.auto_hide, forced, obstructed);
        let speed = cfg.hide_speed;
        let hide_spring = SpringConfig::new(HIDE_STIFFNESS * speed * speed, HIDE_DAMPING * speed, 1.0);
        if self.hide.config() != hide_spring {
            self.hide.set_config(hide_spring);
        }
        self.hide.set_instant(reduce);
        self.hide.set_target(if visible { 0.0 } else { 1.0 });
        moving |= self.hide.step(dt);
        if held_out {
            window.request_animation_frame();
        }
        let hide = self.hide.value().clamp(0.0, HIDE_OVERSHOOT);
        let slide = hide * (thickness + cfg.margin + HIDE_EXTRA_PX);
        let shown = 1.0 - hide.clamp(0.0, 1.0);
        let bg = model::place(cfg.position, surface, cfg.margin, start, -slide, len, thickness);
        self.glass = bg;
        self.surface = surface;

        // Bounces (launch, attention, install landing) and the poof.
        let (launching, landing) = cx.try_global::<Shared>().map(|s| (s.launching.clone(), s.landing.clone())).unwrap_or_default();
        let mut bouncing = false;
        let hop = |it: &Item| -> f32 {
            if reduce || !it.is_app() {
                return 0.0;
            }
            if let Some(t0) = launching.get(&it.key).filter(|_| it.windows.is_empty()) {
                return model::launch_hop(t0.elapsed().as_secs_f32());
            }
            if let Some(t0) = landing.get(&it.pin_key()).filter(|t| t.elapsed().as_secs_f32() < model::LAUNCH_HOP_S) {
                return model::launch_hop(t0.elapsed().as_secs_f32());
            }
            let wants = cfg.bounce_on_attention && !it.focused() && (self.urgent.iter().any(|u| it.windows.iter().any(|w| &w.app_id == u)) || self.badges.get(&it.key).is_some_and(|b| b.critical));
            if wants { model::attention_hop(now.duration_since(self.clock_epoch()).as_secs_f32()) } else { 0.0 }
        };

        // Layout.

        let accent = readable(palette.primary, &palette);
        let mut children: Vec<gpui::AnyElement> = Vec::with_capacity(list.len() * 2 + 4);
        let mut rects = Vec::with_capacity(list.len());
        let mut along = start + cfg.padding;
        let mut max_cross = thickness;
        let launcher_icon = config(cx).read(cx).get().appearance.launcher_icon.clone();
        let trash_full = cfg.trash && stacks::trash_count(cx) > 0;
        let media_cover = self.media_cover.clone();
        for (it, s) in list.iter().zip(&scales) {
            if it.kind == Kind::Separator {
                let l = base * SEPARATOR_FRAC;
                let (x, y, w, h) = model::place(cfg.position, surface, cfg.margin, along + cfg.spacing, (thickness - l) / 2.0 - slide, SEPARATOR_PX, l);
                children.push(div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).rounded_full().bg(palette.on_surface.with_alpha(SEPARATOR_ALPHA).hsla()).into_any_element());
                along += slot(&it.kind);
                continue;
            }
            let size_px = base * s;
            let lift = hop(it) * base * model::HOP_FRAC;
            if lift > 0.0 || launching.contains_key(&it.key) {
                bouncing = true;
            }
            let failed = match it.kind {
                Kind::Installing(id) => cx.try_global::<Shared>().and_then(|s| s.installs.iter().find(|j| j.id == id).cloned()),
                _ => None,
            };
            let shake = failed.as_ref().filter(|j| j.failed.is_some()).and_then(|j| cx.try_global::<Shared>().and_then(|s| s.failed_since.get(&j.id).copied())).map(|t0| t0.elapsed().as_secs_f32()).filter(|t| *t < SHAKE_S && !reduce);
            if shake.is_some() {
                bouncing = true;
            }
            let mut a = along + shake.map(|t| (t * SHAKE_HZ * std::f32::consts::TAU).sin() * SHAKE_PX * (1.0 - t / SHAKE_S)).unwrap_or(0.0);
            let along_px = if it.kind == Kind::Media { media_len } else { size_px };
            let mut rect = model::place(cfg.position, surface, cfg.margin, a, cfg.padding - slide + lift, along_px, size_px);
            let mut removing = false;
            if dragging.as_deref() == Some(it.key.as_str())
                && let Some(d) = &self.drag
            {
                removing = d.remove;
                if d.remove {
                    // Follows the pointer, kept inside the surface (with room
                    // for its "Remove from Dock" label).
                    let (x, y) = clamp_into(cfg.position, surface, (d.at.0 - size_px / 2.0, d.at.1 - size_px / 2.0), size_px, TOOLTIP_H + TOOLTIP_GAP);
                    rect = (x, y, size_px, size_px);
                } else {
                    a = model::along(cfg.position, d.at) - size_px / 2.0;
                    rect = model::place(cfg.position, surface, cfg.margin, a, cfg.padding - slide, size_px, size_px);
                }
            }
            rects.push((it.key.clone(), rect));
            max_cross = max_cross.max(cfg.padding + size_px + lift);
            let running = it.running();
            let mut opacity = if it.is_app() && !running { cfg.inactive_opacity } else { 1.0 };
            if dragging.as_deref() == Some(it.key.as_str()) {
                opacity *= if removing { REMOVE_ALPHA } else { DRAG_ALPHA };
            }
            let tile: gpui::AnyElement = match &it.kind {
                // The launcher button wears the bar's launcher icon (Studio → Launcher).
                Kind::Launcher => launcher_tile(&launcher_icon, size_px, &palette, accent),
                Kind::Applications => applications::folder_tile(size_px, &palette),
                Kind::Trash => trash_tile(trash_full, size_px, &palette),
                Kind::Stack(i) => match cfg.stacks.get(*i) {
                    Some(st) => stacks::stack_tile(st, size_px, &palette, cx),
                    None => glyph_tile("folder", size_px, &palette, accent),
                },
                Kind::Minimized(_) => minimized_tile(&it.icon_id, size_px, decode_px, &palette),
                Kind::Installing(_) => install_tile(failed.as_ref(), size_px, &palette, accent),
                Kind::Media => media_chip(&media_cover, rect.2.max(rect.3), rect.2.min(rect.3), &palette, window, cx),
                _ => crate::plugins::workspaces::app_icon_at(&it.icon_id, size_px, decode_px, palette.on_surface, false, &palette),
            };
            let key_down = it.key.clone();
            let key_up = it.key.clone();
            let key_hover = it.key.clone();
            let key_drop = it.key.clone();
            let drop_target = matches!(it.kind, Kind::App | Kind::Trash | Kind::Applications);
            let drop_tint = accent.with_alpha(DROP_ALPHA).hsla();
            // The Applications folder glows for installable apps ("Drop to install").
            let install_glow = (it.kind == Kind::Applications).then(|| glow(accent.with_alpha(INSTALL_GLOW.0), INSTALL_GLOW.1, 0.0, 0.0));
            let tile_radius = crate::style::radius(size_px * TILE_RADIUS_FRAC);
            children.push(
                div()
                    .id(SharedString::from(format!("dock-{}", it.key)))
                    .absolute()
                    .left(px(rect.0))
                    .top(px(rect.1))
                    .w(px(rect.2))
                    .h(px(rect.3))
                    .rounded(px(tile_radius))
                    .opacity(opacity * shown.max(0.0))
                    .cursor_pointer()
                    .child(tile)
                    .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                        // GPUI re-tests hover on repaint against the last pointer position, stale once the pointer left the surface.
                        if *hovered && !window.is_window_hovered() {
                            return;
                        }
                        if *hovered {
                            this.hovered = Some(key_hover.clone());
                        } else if this.hovered.as_deref() == Some(key_hover.as_str()) {
                            this.hovered = None;
                        }
                        cx.notify();
                    }))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| this.on_down(e, &key_down, cx)))
                    .on_mouse_down(MouseButton::Middle, {
                        let key = it.key.clone();
                        cx.listener(move |this, e: &MouseDownEvent, _, cx| this.on_down(e, &key, cx))
                    })
                    .on_mouse_down(MouseButton::Right, {
                        let key = it.key.clone();
                        cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.on_down(e, &key, cx);
                        })
                    })
                    .on_mouse_up(MouseButton::Left, cx.listener(move |this, _: &MouseUpEvent, _, cx| this.on_up(Some(&key_up), cx)))
                    .when(drop_target, |el| {
                        el.drag_over::<ExternalPaths>(move |style, paths, _, cx| match &install_glow {
                            Some(g) if paths.paths().iter().any(|p| install::accepts(p, cx)) => style.bg(drop_tint).shadow(vec![g.clone()]),
                            _ => style.bg(drop_tint),
                        })
                            .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                                cx.stop_propagation();
                                this.on_drop_files(Some(&key_drop), paths, cx);
                            }))
                    })
                    .into_any_element(),
            );
            // Running indicator between the icon and the outer edge.
            if running && cfg.indicator != DockIndicator::None && !removing {
                let focused = it.focused();
                let tint = if focused { accent } else { palette.on_surface.with_alpha(INDICATOR_ALPHA) };
                let inset = cfg.padding * INDICATOR_INSET_FRAC;
                let centre = a + size_px / 2.0;
                let marks: Vec<(f32, f32)> = match cfg.indicator {
                    DockIndicator::Dots => {
                        let n = it.windows.len().min(MAX_DOTS);
                        let total = n as f32 * DOT_PX + (n as f32 - 1.0) * DOT_GAP_PX;
                        (0..n).map(|k| (centre - total / 2.0 + k as f32 * (DOT_PX + DOT_GAP_PX), DOT_PX)).collect()
                    }
                    DockIndicator::Dot => vec![(centre - DOT_PX / 2.0, DOT_PX)],
                    _ => {
                        let w = if focused { BAR_W_FOCUSED_PX } else { BAR_W_PX };
                        vec![(centre - w / 2.0, w)]
                    }
                };
                for (m, l) in marks {
                    let (x, y, w, h) = model::place(cfg.position, surface, cfg.margin, m, inset - DOT_PX / 2.0 - slide, l, DOT_PX);
                    children.push(
                        div()
                            .absolute()
                            .left(px(x))
                            .top(px(y))
                            .w(px(w))
                            .h(px(h))
                            .rounded_full()
                            .bg(tint.hsla())
                            .opacity(shown)
                            .when(focused, |el| el.shadow(vec![glow(accent.with_alpha(FOCUS_GLOW.0), FOCUS_GLOW.1, 0.0, 0.0)]))
                            .into_any_element(),
                    );
                }
            }
            // Badges: unread notifications, else the window count; progress.
            let badge = self.badges.get(&it.key);
            let unread = badge.map(|b| b.unread).filter(|n| cfg.notification_badges && *n > 0);
            let label = match unread {
                Some(n) => Some((model::badge_text(n), palette.urgent_coral)),
                None if cfg.badges && it.windows.len() >= 2 => Some((it.windows.len().to_string(), accent)),
                None => None,
            };
            if let Some((label, fill)) = label.filter(|_| !removing) {
                let (x, y) = (rect.0 + rect.2 - BADGE_PX * BADGE_INSET_FRAC, rect.1 - BADGE_PX * (1.0 - BADGE_INSET_FRAC));
                children.push(
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .min_w(px(BADGE_PX))
                        .h(px(BADGE_PX))
                        .px(px(BADGE_PAD_PX))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(fill.hsla())
                        .text_color(fill.best_on(Color::hex("#ffffff"), Color::hex("#101010")).hsla())
                        .text_size(px(BADGE_TEXT_PX))
                        .font_weight(FontWeight::BOLD)
                        .font_features(tabular_nums())
                        .opacity(shown)
                        .child(label)
                        .into_any_element(),
                );
            }
            if let Some(p) = badge.and_then(|b| b.progress).filter(|_| cfg.progress_badges && !removing) {
                let inset = rect.2 * PROGRESS_INSET_FRAC;
                let w = rect.2 - inset * 2.0;
                let (x, y) = (rect.0 + inset, rect.1 + rect.3 - inset - PROGRESS_H);
                children.push(
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .w(px(w))
                        .h(px(PROGRESS_H))
                        .rounded_full()
                        .bg(palette.on_surface.with_alpha(PROGRESS_TRACK_ALPHA).hsla())
                        .opacity(shown)
                        .child(div().h_full().w(px(w * f32::from(p) / 100.0)).rounded_full().bg(accent.hsla()))
                        .into_any_element(),
                );
            }
            along += base * s + cfg.spacing;
        }
        // The poof of a removed icon.
        if let Some(Poof(t0, (px0, py0))) = &self.poof {
            let t = t0.elapsed().as_secs_f32() / POOF_S;
            if t >= 1.0 {
                self.poof = None;
            } else {
                bouncing = true;
                let ease = 1.0 - (1.0 - t).powi(3);
                for k in 0..POOF_PUFFS {
                    let ang = k as f32 / POOF_PUFFS as f32 * std::f32::consts::TAU;
                    let r = POOF_SPREAD_PX * ease;
                    let s = POOF_PUFF_PX * (1.0 - t * 0.6);
                    children.push(
                        div()
                            .absolute()
                            .left(px(px0 + ang.cos() * r - s / 2.0))
                            .top(px(py0 + ang.sin() * r - s / 2.0))
                            .size(px(s))
                            .rounded_full()
                            .bg(palette.on_surface.with_alpha((1.0 - t) * INDICATOR_ALPHA).hsla())
                            .into_any_element(),
                    );
                }
            }
        }
        if moving || bouncing {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        // Tooltip over the hovered icon (or why an install failed, or that
        // a dragged icon is about to go).
        let removing = self.drag.as_ref().filter(|d| d.active && d.remove).map(|d| d.key.clone());
        let tip_key = removing.clone().or_else(|| self.hovered.clone().filter(|_| dragging.is_none() && shown > 0.5 && cfg.tooltips));
        if let Some(h) = tip_key
            && let Some((_, r)) = rects.iter().find(|(k, _)| *k == h)
            && let Some(item) = list.iter().find(|i| i.key == h)
        {
            let label: SharedString = if removing.is_some() {
                t("Remove from Dock").into()
            } else {
                match &item.kind {
                    Kind::Launcher => t("Apps").into(),
                    Kind::Applications => t("Applications").into(),
                    Kind::Trash => t("Trash").into(),
                    Kind::Media => media_label(cx),
                    Kind::Stack(i) => cfg.stacks.get(*i).map(stacks::stack_name).unwrap_or_default().into(),
                    Kind::Installing(id) => match cx.try_global::<Shared>().and_then(|s| s.installs.iter().find(|j| j.id == *id).cloned()) {
                        Some(j) => match j.failed {
                            Some(e) => tf("Couldn't install {app}: {error}", &[("app", &j.name), ("error", &e)]).into(),
                            None => tf("Installing {app}… {pct}%", &[("app", &j.name), ("pct", &((j.progress * 100.0).round() as u32))]).into(),
                        },
                        None => item.name.clone().into(),
                    },
                    _ => item.name.clone().into(),
                }
            };
            let est_w = (label.chars().count() as f32 * TOOLTIP_CHAR_PX + TOOLTIP_PAD_PX).clamp(TOOLTIP_MIN_W, TOOLTIP_MAX_W);
            let (x, y) = match cfg.position {
                DockPosition::Bottom => (r.0 + r.2 / 2.0 - est_w / 2.0, r.1 - TOOLTIP_H - TOOLTIP_GAP),
                DockPosition::Left => (r.0 + r.2 + TOOLTIP_GAP, r.1 + r.3 / 2.0 - TOOLTIP_H / 2.0),
                DockPosition::Right => (r.0 - est_w - TOOLTIP_GAP, r.1 + r.3 / 2.0 - TOOLTIP_H / 2.0),
            };
            children.push(
                div()
                    .absolute()
                    .left(px(x.clamp(2.0, (surface.0 - est_w - 2.0).max(2.0))))
                    .top(px(y.max(2.0)))
                    .w(px(est_w))
                    .h(px(TOOLTIP_H))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(crate::shell::frame_color(&palette).hsla())
                    .shadow(rim(&palette))
                    .text_size(px(text::BODY_S))
                    .font_weight(FontWeight::MEDIUM)
                    .truncate()
                    .child(label)
                    .into_any_element(),
            );
        }
        // "Drop to install": a label over the Applications folder (or the
        // dock's middle) while an installable app is dragged over the dock.
        if self.install_hover {
            let label = t("Drop to install");
            let est_w = (label.chars().count() as f32 * TOOLTIP_CHAR_PX + TOOLTIP_PAD_PX).clamp(TOOLTIP_MIN_W, TOOLTIP_MAX_W);
            let r = rects.iter().find(|(k, _)| k == "@applications").map(|(_, r)| *r).unwrap_or(bg);
            let (x, y) = match cfg.position {
                DockPosition::Bottom => (r.0 + r.2 / 2.0 - est_w / 2.0, r.1 - TOOLTIP_H - TOOLTIP_GAP),
                DockPosition::Left => (r.0 + r.2 + TOOLTIP_GAP, r.1 + r.3 / 2.0 - TOOLTIP_H / 2.0),
                DockPosition::Right => (r.0 - est_w - TOOLTIP_GAP, r.1 + r.3 / 2.0 - TOOLTIP_H / 2.0),
            };
            children.push(
                div()
                    .absolute()
                    .left(px(x.clamp(2.0, (surface.0 - est_w - 2.0).max(2.0))))
                    .top(px(y.max(2.0)))
                    .w(px(est_w))
                    .h(px(TOOLTIP_H))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(accent.hsla())
                    .text_color(accent.best_on(Color::hex("#ffffff"), Color::hex("#101010")).hsla())
                    .text_size(px(text::BODY_S))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(label)
                    .into_any_element(),
            );
        }
        self.rects = rects;

        // Input region: the dock (with magnified icons and the tooltip) when
        // shown, the whole surface while dragging, a thin strip flush with
        // the screen edge when hidden.
        let region = if self.drag.as_ref().is_some_and(|d| d.active) {
            (0.0, 0.0, surface.0, surface.1)
        } else if shown > 0.05 {
            model::place(cfg.position, surface, cfg.margin, start, 0.0, len, max_cross.max(thickness) + TOOLTIP_SPACE)
        } else {
            model::reveal_strip(cfg.position, surface, rest_start, rest_len, REVEAL_STRIP_PX)
        };
        let irect = (region.0.floor() as i32, region.1.floor() as i32, region.2.ceil() as i32, region.3.ceil() as i32);
        if self.region.first() != Some(&irect) {
            let bounds = [Bounds { origin: point(px(irect.0 as f32), px(irect.1 as f32)), size: size(px(irect.2.max(1) as f32), px(irect.3.max(1) as f32)) }];
            window.set_input_region(Some(&bounds));
            self.region = vec![irect];
        }

        let fill = crate::shell::frame_color(&palette).mix(palette.primary, cfg.tint).with_alpha(cfg.opacity);
        let mut shadow = Vec::new();
        if cfg.shadow {
            shadow.extend(elevation(&palette, 1.0));
        }
        if cfg.rim {
            shadow.extend(rim(&palette));
        }
        if self.install_hover {
            shadow.push(glow(accent.with_alpha(INSTALL_GLOW.0), INSTALL_GLOW.1, 0.0, 0.0));
        }
        let background = div()
            .id("dock-glass")
            .absolute()
            .left(px(bg.0))
            .top(px(bg.1))
            .w(px(bg.2))
            .h(px(bg.3))
            .rounded(px(crate::style::radius(cfg.radius.min(thickness / 2.0))))
            .bg(fill.hsla())
            .shadow(shadow)
            .opacity(shown)
            .on_mouse_down(MouseButton::Right, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                this.open_menu_for(None, false, (f32::from(e.position.x), f32::from(e.position.y)), cx);
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.on_drop_files(None, paths, cx)));
        // Drags keep tracking outside the icons' hitboxes (and past the
        // surface while the button is held): window-level listeners.
        let me = cx.entity().downgrade();
        let tracker = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let (mv, up) = (me.clone(), me.clone());
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let vp = window.viewport_size();
                    let surface = (f32::from(vp.width), f32::from(vp.height));
                    let _ = mv.update(cx, |this, cx| {
                        if this.drag.is_some() {
                            this.on_move((f32::from(e.position.x), f32::from(e.position.y)), surface, cx);
                        }
                    });
                });
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let _ = up.update(cx, |this, cx| {
                        if this.drag.as_ref().is_some_and(|d| d.active) {
                            this.on_up(None, cx);
                        } else if this.drag.is_some() {
                            // Released off the pressed icon: no click.
                            this.drag = None;
                        }
                    });
                });
            },
        )
        .absolute()
        .size_full();
        div()
            .id("dock-root")
            .size_full()
            .relative()
            .font_family(f.ui.clone())
            .text_color(palette.on_surface.hsla())
            .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                // GPUI re-tests hover on repaint against the last pointer position, stale once the pointer left the surface.
                if *hovered && !window.is_window_hovered() {
                    return;
                }
                this.set_inside(*hovered, cx);
            }))
            .on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, window, cx| {
                this.set_inside(true, cx);
                if this.drag.is_none() {
                    let vp = window.viewport_size();
                    this.on_move((f32::from(e.position.x), f32::from(e.position.y)), (f32::from(vp.width), f32::from(vp.height)), cx);
                }
            }))
            .on_drag_move::<ExternalPaths>(cx.listener(|this, e: &DragMoveEvent<ExternalPaths>, _, cx| {
                let installable = e.drag(cx).paths().iter().any(|p| install::accepts(p, cx));
                if installable != this.install_hover {
                    this.install_hover = installable;
                    cx.notify();
                }
            }))
            .child(tracker)
            .child(background)
            .children(children)
    }
}

impl DockView {
    /// A fixed origin for the attention bounce phase.
    fn clock_epoch(&self) -> Instant {
        *EPOCH.get_or_init(Instant::now)
    }
}

static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// The launcher button: the configured launcher icon on a soft tile.
fn launcher_tile(spec: &str, size_px: f32, palette: &MandaraPalette, tint: Color) -> gpui::AnyElement {
    div()
        .size(px(size_px))
        .rounded(px(crate::style::radius(size_px * TILE_RADIUS_FRAC)))
        .bg(wash(WASH_SOFT, palette).hsla())
        .flex()
        .items_center()
        .justify_center()
        .child(crate::icons::any_icon(spec, size_px * GLYPH_FRAC, tint))
        .into_any_element()
}

/// A rounded tile with a bundled glyph (Applications, fallbacks).
fn glyph_tile(name: &'static str, size_px: f32, palette: &MandaraPalette, tint: Color) -> gpui::AnyElement {
    div()
        .size(px(size_px))
        .rounded(px(crate::style::radius(size_px * TILE_RADIUS_FRAC)))
        .bg(wash(WASH_SOFT, palette).hsla())
        .flex()
        .items_center()
        .justify_center()
        .child(icon(name, size_px * GLYPH_FRAC, tint))
        .into_any_element()
}

/// The first of `names` the icon theme has, else bundled `glyph` on a
/// soft tile (themes differ: `folder-download` vs `folder-downloads`…).
pub fn themed(names: &[&str], glyph: &'static str, size_px: f32, palette: &MandaraPalette, tint: Color) -> gpui::AnyElement {
    themed_at(names, glyph, size_px, size_px, palette, tint)
}

/// [`themed`] decoded for `decode_px` (an animated icon's final size).
pub fn themed_at(names: &[&str], glyph: &'static str, size_px: f32, decode_px: f32, palette: &MandaraPalette, tint: Color) -> gpui::AnyElement {
    match names.iter().find(|n| crate::icon_theme::resolve(n, None, false).is_some()) {
        Some(n) => div().size(px(size_px)).flex().items_center().justify_center().child(crate::icons::any_icon_at(n, size_px, decode_px, tint)).into_any_element(),
        None => glyph_tile(glyph, size_px, palette, tint),
    }
}

/// The Trash: the theme's empty / full trash icon, else the glyph.
fn trash_tile(full: bool, size_px: f32, palette: &MandaraPalette) -> gpui::AnyElement {
    let names: &[&str] = if full { &["user-trash-full", "trash-full"] } else { &["user-trash", "trash-empty"] };
    themed(names, "trash", size_px, palette, if full { palette.on_surface } else { palette.on_surface_variant })
}

/// A minimized window: its app's icon with a corner mark.
fn minimized_tile(app_id: &str, size_px: f32, decode_px: f32, palette: &MandaraPalette) -> gpui::AnyElement {
    let g = size_px * MINIMIZED_GLYPH_FRAC;
    div()
        .size(px(size_px))
        .relative()
        .child(crate::plugins::workspaces::app_icon_at(app_id, size_px, decode_px, palette.on_surface, false, palette))
        .child(div().absolute().right(px(0.0)).bottom(px(0.0)).size(px(g)).rounded_full().bg(crate::shell::frame_color(palette).hsla()).flex().items_center().justify_center().child(icon("minus", g * GLYPH_FRAC * 1.4, palette.on_surface_variant)))
        .into_any_element()
}

/// An install in progress: the app's icon, dimmed, in a filling ring (or
/// an alert mark when it failed).
fn install_tile(job: Option<&model::InstallSlot>, size_px: f32, palette: &MandaraPalette, accent: Color) -> gpui::AnyElement {
    let inner = size_px * INSTALL_ICON_FRAC;
    let glyph: gpui::AnyElement = match job.map(|j| j.icon.as_str()).filter(|i| !i.is_empty()) {
        Some(spec) => crate::icons::any_icon(spec, inner, palette.on_surface),
        None => icon("download", inner * GLYPH_FRAC * 1.4, palette.on_surface_variant).into_any_element(),
    };
    let failed = job.is_some_and(|j| j.failed.is_some());
    let progress = job.map(|j| j.progress).unwrap_or(0.0);
    div()
        .size(px(size_px))
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .child(div().opacity(INSTALL_DIM).child(glyph))
        .child(div().absolute().inset_0().child(crate::icons::ring(if failed { 1.0 } else { progress }, size_px, RING_STROKE, if failed { palette.urgent_coral } else { accent }, palette.on_surface.with_alpha(PROGRESS_TRACK_ALPHA))))
        .when(failed, |el| el.child(div().absolute().child(icon("alert", inner * GLYPH_FRAC, palette.urgent_coral))))
        .into_any_element()
}

/// The now-playing chip: album art, title / artist and a play / pause
/// button (a square of art over a play glyph in a side dock). Clicking the
/// chip toggles playback.
fn media_chip(cover: &Rc<std::cell::RefCell<crate::plugins::media::CoverSlot>>, w: f32, h: f32, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> gpui::AnyElement {
    use crate::plugins::media::{ArtTheme, CoverSlot, art};
    let state = crate::hub::hub(cx).read(cx).media.clone().unwrap_or_default();
    let image = CoverSlot::get(cover, state.art_path.as_deref(), h, window, cx);
    let m = ArtTheme::of(&state, palette);
    let radius = crate::style::radius(h * TILE_RADIUS_FRAC);
    let playing = state.status == mandara_services::PlaybackStatus::Playing;
    let glyph = if playing { "pause" } else { "play" };
    if w <= h + 1.0 {
        return div()
            .relative()
            .size(px(h))
            .child(art(image, h, radius, &m))
            .child(div().absolute().inset_0().flex().items_center().justify_center().child(icon(glyph, h * MEDIA_PLAY_GLYPH, palette.on_surface)))
            .into_any_element();
    }
    let play = h * MEDIA_PLAY_FRAC;
    div()
        .w(px(w))
        .h(px(h))
        .flex()
        .items_center()
        .gap(px(MEDIA_GAP_PX))
        .child(art(image, h, radius, &m))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(MEDIA_TITLE_PX)).font_weight(gpui::FontWeight::SEMIBOLD).truncate().child(SharedString::from(state.title.clone())))
                .child(div().text_size(px(MEDIA_ARTIST_PX)).text_color(m.a.hsla()).truncate().child(SharedString::from(state.artist.clone()))),
        )
        .child(
            div()
                .size(px(play))
                .flex_none()
                .rounded_full()
                .bg(mandara_core::oklab_gradient(crate::plugins::panel_fx::ANGLE_DIAGONAL_DEG, m.a, m.b))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(glyph, play * MEDIA_PLAY_GLYPH, palette.on_primary)),
        )
        .into_any_element()
}

/// The media chip's tooltip: what's playing.
fn media_label(cx: &App) -> SharedString {
    match crate::hub::hub(cx).read(cx).media.as_ref() {
        Some(m) if !m.artist.is_empty() => format!("{} — {}", m.title, m.artist).into(),
        Some(m) => m.title.clone().into(),
        None => SharedString::default(),
    }
}

// ── Surfaces ─────────────────────────────────────────────────────────────

pub(crate) fn layer_options(role: SurfaceRole, layer: Layer, anchor: Anchor, size_px: (f32, f32), exclusive: Option<f32>, keyboard: KeyboardInteractivity, display: Option<gpui::DisplayId>) -> WindowOptions {
    WindowOptions {
        titlebar: None,
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(0.0), px(0.0)), size: size(px(size_px.0), px(size_px.1)) })),
        focus: false,
        show: true,
        kind: WindowKind::LayerShell(LayerShellOptions {
            namespace: role.namespace().to_string(),
            layer,
            anchor,
            exclusive_zone: exclusive.map(px),
            exclusive_edge: None,
            margin: Some((px(0.0), px(0.0), px(0.0), px(0.0))),
            keyboard_interactivity: keyboard,
        }),
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: display,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some(mandara_core::SHELL_APP_ID.to_string()),
        inactive_frame_interval: None,
        ..Default::default()
    }
}

/// `(index, geometry)` of every output.
fn outputs(cx: &App) -> Vec<mandara_core::OutputGeometry> {
    cx.try_global::<crate::overlays::Outputs>().map(|o| o.0.clone()).filter(|o| !o.is_empty()).unwrap_or_else(|| mandara_core::query_outputs().unwrap_or_default())
}

pub(crate) fn output_size(monitor: &str, cx: &App) -> (f32, f32) {
    let all = outputs(cx);
    all.iter().find(|o| o.name == monitor).or(all.first()).map(|o| (o.width as f32, o.height as f32)).unwrap_or_else(|| crate::overlays::primary_output_size(cx))
}

pub(crate) fn display_for(monitor: &str, cx: &App) -> Option<gpui::DisplayId> {
    crate::outputs::display_for(monitor, cx)
}

/// What a dock surface was opened with (a change reopens it).
#[derive(Clone, PartialEq)]
struct SurfaceKey {
    position: DockPosition,
    overlay: bool,
    exclusive: Option<i32>,
    thickness: i32,
}

#[derive(Default)]
struct Docks(Vec<(String, SurfaceKey, WindowHandle<DockView>)>);
impl Global for Docks {}

fn alive<V: 'static>(h: &WindowHandle<V>, cx: &App) -> bool {
    cx.windows().iter().any(|w: &AnyWindowHandle| w.window_id() == h.window_id())
}

/// Whether the dock stays shown because the launcher is open above it
/// (the launcher's bottom drawer floats over a bottom dock; see
/// `launcher_place::for_config`).
pub fn stays_out_for_launcher(cfg: &DockConfig, launcher_open: bool) -> bool {
    launcher_open && cfg.position == DockPosition::Bottom
}

/// Opens / closes / reopens dock surfaces to match the config. A dock that
/// opens is shown for a moment whatever its hiding mode, so switching it
/// on (Studio, Plugins, `mandara msg dock on`) always visibly works.
fn reconcile(cx: &mut App) {
    let cfg = config(cx).read(cx).snapshot();
    let enabled = cx.global::<PluginRegistry>().is_enabled(ID, &cfg.plugins);
    let d = cfg.dock_sanitized();
    let thickness = surface_thickness(&d);
    let exclusive = (d.reserve_space && d.auto_hide == DockAutoHide::Never).then(|| d.band().round() as i32);
    let key = SurfaceKey { position: d.position, overlay: d.overlay, exclusive, thickness: thickness as i32 };
    let all = outputs(cx);
    let names: Vec<String> = all.iter().map(|o| o.name.clone()).collect();
    let mut docks = std::mem::take(&mut cx.default_global::<Docks>().0);
    docks.retain(|(_, k, h)| {
        let keep = alive(h, cx) && *k == key;
        if !keep && alive(h, cx) {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
        keep
    });
    let open: Vec<String> = docks.iter().map(|(n, ..)| n.clone()).collect();
    let (to_open, to_close) = model::plan_surfaces(&cfg, enabled, &names, &open);
    docks.retain(|(name, _, h)| {
        let close = to_close.contains(name);
        if close {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
        !close
    });
    for name in to_open {
        let Some(o) = all.iter().find(|o| o.name == name) else { continue };
        let (anchor, sz) = match d.position {
            DockPosition::Bottom => (Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT, (o.width as f32, thickness)),
            DockPosition::Left => (Anchor::LEFT | Anchor::TOP | Anchor::BOTTOM, (thickness, o.height as f32)),
            DockPosition::Right => (Anchor::RIGHT | Anchor::TOP | Anchor::BOTTOM, (thickness, o.height as f32)),
        };
        let layer = if d.overlay { Layer::Overlay } else { Layer::Top };
        let display = crate::outputs::display_for(&o.name, cx);
        // A zone of 0 keeps clear of other surfaces' zones (a bottom bar).
        let options = layer_options(SurfaceRole::Dock, layer, anchor, sz, Some(exclusive.map(|z| z as f32).unwrap_or(0.0)), KeyboardInteractivity::None, display);
        let n = o.name.clone();
        let origin = (o.x as f32, o.y as f32);
        match cx.open_window(options, move |_, cx| cx.new(|cx| DockView::new(n, origin, true, cx))) {
            Ok(h) => {
                tracing::info!("dock: shown on {}", o.name);
                docks.push((o.name.clone(), key.clone(), h));
            }
            Err(err) => tracing::error!("dock surface: {err:#}"),
        }
    }
    cx.set_global(Docks(docks));
}

/// Starts the dock (when enabled) and follows the config.
pub fn init(cx: &mut App) {
    // Clean up pins written by older builds (once).
    if config(cx).read(cx).get().dock.clone().normalize_pins() {
        update_config(cx, |c| {
            c.dock.normalize_pins();
        });
    }
    track_windows(cx);
    install::wire_installer(cx);
    reconcile(cx);
    let mut last = config(cx).read(cx).snapshot();
    cx.observe(&config(cx), move |store, cx| {
        let next = store.read(cx).snapshot();
        if next.dock != last.dock || next.plugins != last.plugins || next.displays != last.displays {
            last = next;
            reconcile(cx);
        }
    })
    .detach();
}

fn each_dock(cx: &mut App, f: impl Fn(&mut DockView, &mut Context<DockView>)) -> usize {
    let docks: Vec<WindowHandle<DockView>> = cx.try_global::<Docks>().map(|d| d.0.iter().map(|(_, _, h)| *h).collect()).unwrap_or_default();
    let mut n = 0;
    for h in docks {
        if h.update(cx, |v, _, cx| f(v, cx)).is_ok() {
            n += 1;
        }
    }
    n
}

/// Resolves an app named on the command line to a desktop id: a pinned
/// app's id or name, an installed app's exact id or name, then (only with
/// `fuzzy`, for pinning) the best name match. Unpinning never guesses.
fn resolve_app(name: &str, pinned: &[String], fuzzy: bool) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(t("Name an app (desktop id or name)").to_string());
    }
    let apps = crate::apps::discover();
    let id = mandara_config::dock::pin_id(name);
    let by_name = |n: &str| apps.iter().find(|x| x.name.eq_ignore_ascii_case(n)).map(|x| x.id.clone());
    if let Some(p) = pinned.iter().find(|p| p.eq_ignore_ascii_case(&id) || by_name(name).is_some_and(|i| i.eq_ignore_ascii_case(p))) {
        return Ok(p.clone());
    }
    if let Some(x) = apps.iter().find(|x| x.id.eq_ignore_ascii_case(&id)) {
        return Ok(x.id.clone());
    }
    if let Some(i) = by_name(name) {
        return Ok(i);
    }
    if fuzzy && let Some(&i) = picker::rank_apps(&apps, name).first() {
        return Ok(apps[i].id.clone());
    }
    Ok(id)
}

fn status_json(cx: &mut App) -> serde_json::Value {
    let snapshot = config(cx).read(cx).snapshot();
    let cfg = snapshot.dock_sanitized();
    let windows = crate::windows::init(cx);
    let apps = crate::apps::discover();
    let s = cx.default_global::<Shared>();
    let (linger, recent, installs) = (s.linger.clone(), s.recent.clone(), s.installs.clone());
    let media = crate::cards::MediaPresence::of(cx).active;
    let list = model::items(&model::Inputs { cfg: &cfg, windows: windows.read(cx).list(), apps: &apps, monitor: "", linger: &linger, recent: &recent, installing: &installs, media });
    let enabled = cx.global::<PluginRegistry>().is_enabled(ID, &snapshot.plugins);
    json!({
        "enabled": enabled,
        "surfaces": cx.try_global::<Docks>().map(|d| d.0.iter().map(|(n, ..)| n.clone()).collect::<Vec<_>>()).unwrap_or_default(),
        "auto_hide": cfg.auto_hide,
        "pinned": cfg.pinned,
        "items": list.iter().filter(|i| i.is_app()).map(|i| json!({ "app": i.key, "name": i.name, "pinned": i.pinned, "recent": i.recent, "windows": i.windows.iter().map(|w| w.id).collect::<Vec<_>>() })).collect::<Vec<_>>(),
        "stacks": cfg.stacks.iter().map(|st| json!({ "path": st.path, "view": st.view, "sort": st.sort })).collect::<Vec<_>>(),
        "trash": cfg.trash,
    })
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Dock"), tk("A macOS-style dock: pinned, running and recent apps, stacks and the Trash, with magnification and auto-hide"), None).opt_in(), |_, _, _| PluginInstance::empty());
    r.command(
        ID,
        CommandSpec::new("dock", tk("Turn the dock on or off, or reveal a hidden one for a few seconds"), |a, cx| {
            let action = a.str(0)?;
            match action {
                "on" | "off" => {
                    let on = action == "on";
                    let how = if on { crate::plugins::lifecycle::Action::Enable } else { crate::plugins::lifecycle::Action::Disable };
                    crate::plugins::lifecycle::apply(ID, how, cx)?;
                    Ok(json!({ "enabled": on }))
                }
                _ => {
                    let until = Instant::now() + SHOW_FOR;
                    let n = each_dock(cx, move |v, cx| {
                        v.shown_until = Some(until);
                        cx.notify();
                    });
                    Ok(json!({ "revealed": n }))
                }
            }
        })
        .arg(ArgSpec::required("action", ArgKind::Enum, tk("on, off or show")).choices(&["on", "off", "show"]))
        // Turns the dock plugin itself on.
        .works_when_off(),
    );
    for (path, summary, how) in [("dock pin", tk("Keep an app in the dock (desktop id or name)"), 0u8), ("dock unpin", tk("Remove an app from the dock"), 1), ("dock toggle-pin", tk("Keep in Dock on / off for an app"), 2)] {
        r.command(
            ID,
            CommandSpec::new(path, summary, move |a, cx| {
                let pinned = config(cx).read(cx).get().dock.pinned.clone();
                let id = resolve_app(&a.rest(0), &pinned, how == 0)?;
                let out = id.clone();
                let mut pinned = false;
                update_config(cx, |c| {
                    pinned = match how {
                        0 => {
                            c.dock.pin(&id);
                            true
                        }
                        1 => {
                            c.dock.unpin(&id);
                            false
                        }
                        _ => c.dock.toggle_pin(&id),
                    };
                });
                if !pinned {
                    linger(&id, cx);
                }
                Ok(json!({ "app": out, "pinned": pinned, "dock": config(cx).read(cx).get().dock.pinned }))
            })
            .arg(ArgSpec::required("app", ArgKind::String, tk("desktop id or app name")))
            .example("dock pin firefox")
            // Pins are config: set them up before turning the dock on.
            .works_when_off(),
        );
    }
    r.command(
        ID,
        CommandSpec::new("dock move", tk("Move a pinned app to a position (0 = first)"), |a, cx| {
            let pinned = config(cx).read(cx).get().dock.pinned.clone();
            let id = resolve_app(a.str(0)?, &pinned, false)?;
            let to = a.int(1)?.max(0) as usize;
            let mut moved = false;
            update_config(cx, |c| moved = c.dock.is_pinned(&id) && c.dock.pin_at(&id, to));
            Ok(json!({ "app": id, "moved": moved, "dock": config(cx).read(cx).get().dock.pinned }))
        })
        .arg(ArgSpec::required("app", ArgKind::String, tk("desktop id or app name")))
        .arg(ArgSpec::required("index", ArgKind::Integer, tk("new position, 0 = first")))
        .works_when_off(),
    );
    r.command(
        ID,
        CommandSpec::new("dock add", tk("Open the app picker over the dock to add apps by searching"), |_, cx| {
            let monitor = cx.try_global::<Docks>().and_then(|d| d.0.first().map(|(n, ..)| n.clone())).unwrap_or_default();
            picker::open(&monitor, cx);
            Ok(json!({ "open": true }))
        }),
    );
    r.command(
        ID,
        CommandSpec::new("dock stack", tk("Stacks on the dock's right side: list, add or remove a folder"), |a, cx| {
            let action = a.str(0)?;
            let folder = a.rest(1);
            let mut changed = false;
            match action {
                "add" => {
                    if folder.trim().is_empty() {
                        return Err(t("Name a folder, e.g. ~/Documents").to_string());
                    }
                    update_config(cx, |c| changed = c.dock.add_stack(&folder));
                }
                "remove" => update_config(cx, |c| changed = c.dock.remove_stack(&folder)),
                "close" => return Ok(json!({ "closed": stacks::close_popover(cx) })),
                "open" => {
                    let cfg = config(cx).read(cx).get().dock.clone();
                    let i = stacks::find_stack(&cfg.stacks, &folder).ok_or_else(|| tf("No stack named “{name}” (see `mandara msg dock stack list`)", &[("name", &folder)]))?;
                    let Some((monitor, anchor)) = item_anchor(&format!("@stack-{i}"), cx) else {
                        return Err(t("The dock isn't showing").to_string());
                    };
                    if !stacks::popover_open(i, cx) {
                        stacks::open_popover(i, &monitor, anchor, cx);
                    }
                    return Ok(json!({ "open": cfg.stacks[i].path, "index": i, "monitor": monitor }));
                }
                _ => {}
            }
            let cfg = config(cx).read(cx).get().dock.clone();
            Ok(json!({ "changed": changed, "stacks": cfg.stacks.iter().map(|s| json!({ "path": s.path, "view": s.view, "sort": s.sort, "display": s.display })).collect::<Vec<_>>() }))
        })
        .arg(ArgSpec::required("action", ArgKind::Enum, tk("list, add, remove, open or close")).choices(&["list", "add", "remove", "open", "close"]))
        .arg(ArgSpec::optional("folder", ArgKind::String, tk("folder path, name or index")))
        .example("dock stack add ~/Documents")
        .example("dock stack open Downloads")
        .works_when_off(),
    );
    r.command(
        ID,
        CommandSpec::new("dock applications", tk("The Applications folder: open or close its grid of apps"), |a, cx| {
            let action = a.get(0).unwrap_or("open");
            if action == "close" {
                return Ok(json!({ "closed": applications::close(cx) }));
            }
            if !config(cx).read(cx).get().dock.applications.enabled {
                return Err(t("The Applications folder is off (Studio → Dock → Applications folder)").to_string());
            }
            let Some((monitor, anchor)) = item_anchor("@applications", cx) else {
                return Err(t("The dock isn't showing").to_string());
            };
            if action == "toggle" {
                applications::toggle(&monitor, anchor, cx);
            } else if !applications::is_open(cx) {
                applications::open(&monitor, anchor, cx);
            }
            Ok(json!({ "open": applications::is_open(cx), "monitor": monitor }))
        })
        .arg(ArgSpec::optional("action", ArgKind::Enum, tk("open, close or toggle")).choices(&["open", "close", "toggle"]))
        .example("dock applications open"),
    );
    r.command(
        ID,
        CommandSpec::new("dock trash", tk("The Trash: count its items, or empty it (asks for --yes)"), |a, cx| {
            let action = a.get(0).unwrap_or("status");
            if action == "empty" {
                if a.get(1) != Some("--yes") {
                    return Err(tf("Emptying the Trash deletes {n} items for good; run `mandara msg dock trash empty --yes` to confirm", &[("n", &stacks::trash_count(cx))]));
                }
                let n = stacks::empty_trash_confirmed(true, cx)?.unwrap_or(0);
                refresh_all(cx);
                return Ok(json!({ "emptied": n }));
            }
            Ok(json!({ "items": stacks::trash_count(cx), "path": stacks::trash_dir() }))
        })
        .arg(ArgSpec::optional("action", ArgKind::Enum, tk("status or empty")).choices(&["status", "empty"]))
        .arg(ArgSpec::optional("confirm", ArgKind::String, tk("--yes to really empty it")))
        .works_when_off(),
    );
    r.command(
        ID,
        CommandSpec::new("dock status", tk("Dock items: pinned, running and recent apps with their windows, stacks"), |_, cx| Ok(status_json(cx)))
            // Diagnoses a dock that doesn't show.
            .works_when_off(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bottom_dock_stays_out_under_the_launcher() {
        let bottom = DockConfig::default();
        assert_eq!(bottom.position, DockPosition::Bottom);
        assert!(stays_out_for_launcher(&bottom, true));
        assert!(!stays_out_for_launcher(&bottom, false));
        let side = DockConfig { position: DockPosition::Left, ..DockConfig::default() };
        assert!(!stays_out_for_launcher(&side, true), "a side dock isn't under the drawer");
    }

    #[test]
    fn config_and_registry_agree_on_the_dock() {
        // `ShellConfig::dock_enabled` (used where no registry exists, e.g.
        // Studio) must match the plugin registry's opt-in rule.
        let mut registry = PluginRegistry::default();
        register(&mut registry);
        let mut cfg = mandara_config::ShellConfig::default();
        for (enabled, disabled) in [(false, false), (true, false), (true, true), (false, true)] {
            cfg.plugins.enabled.retain(|p| p != ID);
            cfg.plugins.disabled.retain(|p| p != ID);
            if enabled {
                cfg.plugins.enabled.push(ID.into());
            }
            if disabled {
                cfg.plugins.disabled.push(ID.into());
            }
            assert_eq!(registry.is_enabled(ID, &cfg.plugins), cfg.dock_enabled(), "enabled={enabled} disabled={disabled}");
        }
        assert_eq!(ID, mandara_config::workflow::DOCK_PLUGIN_ID);
    }

    #[test]
    fn dock_commands_that_must_work_while_off() {
        let mut registry = PluginRegistry::default();
        register(&mut registry);
        for path in ["dock", "dock status", "dock pin", "dock unpin", "dock toggle-pin", "dock stack", "dock move"] {
            let spec = registry.commands.all().iter().find(|s| s.path == path).unwrap_or_else(|| panic!("{path}"));
            assert!(spec.works_when_off, "{path}");
        }
    }

    #[test]
    fn dragged_icons_and_poofs_stay_inside_the_surface() {
        let s = (2560.0, 126.0);
        assert_eq!(clamp_into(DockPosition::Bottom, s, (100.0, -80.0), 48.0, 30.0), (100.0, 30.0), "dragged above the surface: pinned under its label");
        assert_eq!(clamp_into(DockPosition::Bottom, s, (3000.0, 50.0), 48.0, 30.0), (2512.0, 50.0));
        assert_eq!(clamp_into(DockPosition::Left, (126.0, 1440.0), (400.0, 10.0), 48.0, 30.0), (48.0, 10.0));
        assert_eq!(clamp_into(DockPosition::Right, (126.0, 1440.0), (-50.0, 10.0), 48.0, 30.0), (30.0, 10.0));
    }

    #[test]
    fn the_surface_has_room_for_magnified_and_bouncing_icons() {
        let d = DockConfig::default();
        let band = d.icon_size + d.padding * 2.0 + d.margin;
        assert!(surface_thickness(&d) >= band + d.icon_size * (d.magnify - 1.0) + TOOLTIP_SPACE - 1.0);
        let still = DockConfig { magnification: false, ..d.clone() };
        assert!(surface_thickness(&still) >= band + d.icon_size * model::HOP_FRAC, "bounces fit without magnification");
        // Dragging an icon away crosses the remove line inside the surface.
        assert!(surface_thickness(&d) > band + REMOVE_LIFT_PX);
    }
}
