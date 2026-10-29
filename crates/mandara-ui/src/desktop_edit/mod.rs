//! Desktop edit mode: arrange the desktop widgets on the desktop itself.
//!
//! Entered from the desktop menu, `mandara msg desktop edit` or Studio
//! ([`enter`]). While it is on, every output gets a transparent layer
//! surface on the Top layer (`mandara-desktop-edit`, full input, the
//! keyboard on the output it was entered from) showing each enabled
//! widget's outline where it is painted, with a name chip and a corner
//! handle:
//!
//! * drag a widget to move it; it snaps to the screen edges and centre
//!   lines (with a guide; Shift drags freely) and stays inside the area
//!   the renderer keeps it in (clear of the bar and the screen frame). On
//!   release it re-anchors to the nearest of the nine anchors, so it keeps
//!   its place on other resolutions;
//! * drag the corner handle to scale it (about the opposite corner; the
//!   depth clock about its centre);
//! * right-click a widget for Duplicate, Hide, Settings in Studio and
//!   Remove; right-click empty space to add a widget there;
//! * the floating toolbar adds a widget, undoes the last edit, opens Studio
//!   and leaves (so do Esc and Done; Ctrl+Z undoes, Delete removes).
//!
//! Every edit goes through [`update_config`] (the backdrop repaints at
//! once; the depth clock's `[wallpaper]` keys are kept in step with
//! [`dw::sync_depth_clock`]). Outlines come from the renderer, which
//! reports where it painted each widget while edit mode is on
//! ([`crate::backdrop::OutputBoxes`]); cards are placed by the shell, so
//! their boxes are computed here with the card host's own geometry.
//!
//! Nothing stays resident: the surfaces, the model (outlines, undo
//! history) and the renderer's reports exist only while editing.

pub mod geom;

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::layer_shell::{Anchor as LayerAnchor, KeyboardInteractivity, Layer};
use gpui::{
    AnyElement, App, AppContext as _, Context, CursorStyle, Entity, FocusHandle, FontWeight, Global, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, WindowHandle, div, prelude::FluentBuilder as _, px,
};
use mandara_config::ShellConfig;
use mandara_config::desktop_cards::{self as dc, CALENDAR, FRAMED_BARS, MIRRORED_BARS, NOW_PLAYING, WAVE};
use mandara_config::desktop_widgets::{self as dw, Anchor, ClockWidget, DesktopWidget, DesktopWidgetsConfig, Insets, MonitorWidget, Placement, PlaneVisualizer, Preset, WidgetKind};
use mandara_core::{FrameClock, SpringConfig, SpringValue, SurfaceRole, ToGpui};
use mandara_i18n::{t, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use mandara_theme::MandaraPalette;
use serde_json::json;

use self::geom::{Area, Box4, Guide};
use crate::backdrop::OutputBoxes;
use crate::bidi::Bidi as _;
use crate::desktop_menu::{self as menu, Action, Entry, NewWidget, OnHover, OnRow};
use crate::icons::icon;
use crate::kit::text;
use crate::plugins::panel_fx::{elevation, readable, rim};
use crate::state::{config, update_config};

// ── Look ─────────────────────────────────────────────────────────────────

/// Outline stroke (idle / selected), corner radius, and the least size an
/// outline is drawn at (a hairline rule stays grabbable).
const OUTLINE_W: f32 = 1.5;
const SELECTED_W: f32 = 2.0;
const OUTLINE_RADIUS: f32 = 10.0;
const MIN_OUTLINE: f32 = 16.0;
/// Outline alphas (idle / hovered) and the selected widget's fill.
const OUTLINE_ALPHA: f32 = 0.5;
const HOVER_ALPHA: f32 = 0.85;
const SELECTED_FILL: f32 = 0.05;
/// Name chips: height, padding, gap above the outline, glyph.
const CHIP_H: f32 = 22.0;
const CHIP_PAD: f32 = 8.0;
const CHIP_GAP: f32 = 6.0;
const CHIP_GLYPH: f32 = 12.0;
/// The chip's remove button and its hover wash.
const CHIP_REMOVE: f32 = 18.0;
const CHIP_REMOVE_HOVER_ALPHA: f32 = 0.28;
/// The resize handle: knob diameter and hit area.
const HANDLE: f32 = 14.0;
const HANDLE_HIT: f32 = 26.0;
/// Snap guides: stroke and alpha.
const GUIDE_W: f32 = 1.0;
const GUIDE_ALPHA: f32 = 0.75;
/// The toolbar: height, padding, gap from the bottom of the safe area,
/// button height / padding, divider height.
const BAR_H: f32 = 48.0;
const BAR_PAD: f32 = 7.0;
const BAR_MARGIN: f32 = 20.0;
const BTN_H: f32 = 34.0;
const BTN_PAD: f32 = 13.0;
const BTN_GLYPH: f32 = 15.0;
const DIVIDER_H: f32 = 20.0;
const DIVIDER_ALPHA: f32 = 0.1;
const BTN_HOVER: f32 = 0.08;
const DISABLED_ALPHA: f32 = 0.4;
/// Pointer travel before a press becomes a drag.
const DRAG_SLOP: f32 = 3.0;
/// A dropped outline stays where it was dropped until the renderer
/// reports the widget there (at most this long).
const HELD_FOR: Duration = Duration::from_millis(600);
/// A reported box this close to the held one is the drop arriving.
const HELD_MATCH_PX: f32 = 2.0;
/// Undo steps kept (the history goes when edit mode ends).
const UNDO_DEPTH: usize = 32;
/// A duplicate lands this much (fractions of the display) from the
/// original.
const DUPLICATE_OFFSET: f32 = 0.02;
/// Fade of the edit chrome in.
const APPEAR_SPRING: SpringConfig = SpringConfig::new(420.0, 36.0, 1.0);
/// Shell IPC request sent by a process without the desktop (Studio).
const IPC_REQUEST: &str = "desktop edit\n";

// ── Model ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq)]
enum Kind {
    /// Painted on the wallpaper.
    Plane,
    DepthClock,
    Card { strip: bool },
}

/// One widget as edit mode shows it on an output.
#[derive(Clone, Debug)]
struct Item {
    id: String,
    label: SharedString,
    icon: &'static str,
    bounds: Box4,
    outer: Box4,
    kind: Kind,
    /// Where it may go.
    area: Area,
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Mode {
    Move,
    Scale,
}

#[derive(Clone, Debug)]
struct Drag {
    item: Item,
    output: String,
    size: (f32, f32),
    mode: Mode,
    from: (f32, f32),
    place: Placement,
    moved: bool,
    preview: Box4,
    outer: Box4,
    /// Shown while scaling.
    scale: f32,
}

#[derive(Clone, Debug, PartialEq)]
enum PopupKind {
    Widget(String),
    /// Add a widget at a point (`None`: the display's centre).
    Add(Option<(f32, f32)>),
}

#[derive(Clone, Debug)]
struct Popup {
    output: String,
    at: (f32, f32),
    kind: PopupKind,
    hover: Option<usize>,
    /// Opens above `at` (from the toolbar).
    up: bool,
}

#[derive(Clone, Default)]
struct Model {
    boxes: Vec<OutputBoxes>,
    selected: Option<String>,
    hover: Option<String>,
    undo: Rc<Vec<DesktopWidgetsConfig>>,
    drag: Option<Drag>,
    held: Option<(String, Box4, Instant)>,
    popup: Option<Popup>,
    guides: Vec<(String, Guide)>,
    /// Where the widget being dragged is right now. The renderer shows it
    /// there ([`live_place`]) and the config is written once, on release:
    /// a config update re-renders every view that reads the config, and
    /// doing that per pointer event held dragging to a few frames a second.
    live: Option<(String, Placement)>,
}

struct Edit {
    model: Entity<Model>,
    windows: Vec<(String, WindowHandle<EditView>)>,
}

impl Global for Edit {}

/// Edit mode is on in this process.
pub fn active(cx: &App) -> bool {
    cx.has_global::<Edit>()
}

/// The placement of the wallpaper widget being dragged, ahead of the
/// config (see `Model::live`).
pub(crate) fn live_place(cx: &App) -> Option<(String, Placement)> {
    cx.try_global::<Edit>().and_then(|e| e.model.read(cx).live.clone())
}

fn model(cx: &App) -> Option<Entity<Model>> {
    cx.try_global::<Edit>().map(|e| e.model.clone())
}

fn outputs(cx: &App) -> Vec<mandara_core::OutputGeometry> {
    cx.try_global::<crate::overlays::Outputs>().map(|o| o.0.clone()).filter(|o| !o.is_empty()).unwrap_or_else(|| mandara_core::query_outputs().unwrap_or_default())
}

/// Logical size of `output` (the renderer's report, else xdg-output).
fn output_size(output: &str, m: &Model, cx: &App) -> (f32, f32) {
    m.boxes.iter().find(|b| b.output == output).map(|b| b.size).unwrap_or_else(|| crate::plugins::dock::output_size(output, cx))
}

fn safe_of(output: &str, m: &Model, cfg: &ShellConfig) -> Insets {
    m.boxes.iter().find(|b| b.output == output).map(|b| b.safe).unwrap_or_else(|| crate::backdrop::widgets::safe_insets(cfg, cfg.bar_on_output(output)))
}

fn card_label(card: &str) -> &'static str {
    match card {
        NOW_PLAYING => t("Now playing"),
        CALENDAR => t("Calendar"),
        WAVE => t("Wave"),
        FRAMED_BARS => t("Framed bars"),
        MIRRORED_BARS => t("Mirrored bars"),
        _ => t("Card"),
    }
}

fn label_of(w: &DesktopWidget) -> (SharedString, &'static str) {
    match &w.kind {
        WidgetKind::Clock(c) if c.face.depth_style().is_some() => (t("Depth clock").into(), "clock"),
        WidgetKind::Clock(_) => (NewWidget::Clock.label().into(), "clock"),
        WidgetKind::Monitor(_) => (NewWidget::Monitor.label().into(), "gauge"),
        WidgetKind::Decoration(_) => (NewWidget::Decoration.label().into(), "leaf"),
        WidgetKind::Visualizer(_) => (NewWidget::Visualizer.label().into(), "waves"),
        WidgetKind::Card(c) => (card_label(&c.card).into(), if c.card == CALENDAR { "calendar" } else if c.card == NOW_PLAYING { "music" } else { "waves" }),
    }
}

/// The widgets shown on `output`, in paint order (cards last: they sit
/// above the wallpaper).
fn items(output: &str, cfg: &ShellConfig, m: &Model, cx: &App) -> Vec<Item> {
    let size = output_size(output, m, cx);
    let safe = Area::of(size, safe_of(output, m, cfg));
    let list = &cfg.desktop.widgets.items;
    let index = |id: &str| list.iter().position(|w| w.id == id).unwrap_or(usize::MAX);
    let mut out: Vec<(usize, Item)> = Vec::new();
    if let Some(b) = m.boxes.iter().find(|b| b.output == output) {
        for g in &b.widgets {
            let Some(w) = list.iter().find(|w| w.id == g.id && w.enabled) else { continue };
            let (label, icon) = label_of(w);
            out.push((index(&g.id), Item { id: g.id.clone(), label, icon, bounds: g.bounds, outer: g.outer, kind: Kind::Plane, area: safe }));
        }
        if let (Some(clock), Some((w, _))) = (b.clock, cfg.desktop.widgets.depth_clock()) {
            let (label, icon) = label_of(w);
            out.push((index(&w.id), Item { id: w.id.clone(), label, icon, bounds: clock, outer: clock, kind: Kind::DepthClock, area: safe }));
        }
    }
    let rtl = crate::bidi::rtl();
    let mut cards = Vec::new();
    for (w, c) in dc::enabled_cards(&cfg.desktop.widgets) {
        if !w.place.shows_on(&[output]) {
            continue;
        }
        let a = crate::cards::usable_area(cfg, output, size);
        let (ax, ay, aw, ah) = (f32::from(a.origin.x), f32::from(a.origin.y), f32::from(a.size.width), f32::from(a.size.height));
        let card = crate::cards::card_size(&c.card, w, (aw, size.1));
        if card.0 < 1.0 || card.1 < 1.0 {
            continue;
        }
        let r = crate::cards::surface_rect(&c.card, &w.place, a, card, rtl);
        let strip = matches!(c.card.as_str(), WAVE | MIRRORED_BARS);
        let pad = if strip { 0.0 } else { crate::cards::SHADOW_PAD_PX };
        let bounds = [f32::from(r.origin.x) + pad, f32::from(r.origin.y) + pad, f32::from(r.size.width) - 2.0 * pad, f32::from(r.size.height) - 2.0 * pad];
        let (label, icon) = label_of(w);
        cards.push((index(&w.id), Item { id: w.id.clone(), label, icon, bounds, outer: bounds, kind: Kind::Card { strip }, area: Area::rect((ax, ay), (aw, ah), (size.0 / 2.0, size.1 / 2.0)) }));
    }
    out.sort_by_key(|(i, _)| *i);
    cards.sort_by_key(|(i, _)| *i);
    let now = Instant::now();
    out.into_iter()
        .chain(cards)
        .map(|(_, mut it)| {
            match (&m.drag, &m.held) {
                (Some(d), _) if d.item.id == it.id && d.output == output => it.bounds = d.preview,
                // Cards are placed from the config at once; painted widgets
                // wait for the renderer.
                (_, Some((id, b, at))) if *id == it.id && now.duration_since(*at) < HELD_FOR && matches!(it.kind, Kind::Plane | Kind::DepthClock) => {
                    it.outer = remap(it.outer, it.bounds, *b);
                    it.bounds = *b;
                }
                _ => {}
            }
            it
        })
        .collect()
}

/// `b` moved and scaled the way `from` became `to`.
fn remap(b: Box4, from: Box4, to: Box4) -> Box4 {
    let kx = to[2] / from[2].max(f32::EPSILON);
    let ky = to[3] / from[3].max(f32::EPSILON);
    [to[0] + (b[0] - from[0]) * kx, to[1] + (b[1] - from[1]) * ky, b[2] * kx, b[3] * ky]
}

// ── Edits ────────────────────────────────────────────────────────────────

/// Applies `f` to widget `id` (migrating an old config first) and keeps
/// the depth clock's `[wallpaper]` keys in step — as Studio does.
fn write(id: &str, f: impl FnOnce(&mut DesktopWidget) + 'static, cx: &mut App) {
    let id = id.to_string();
    update_config(cx, move |c| {
        dw::migrate_depth_clock(c);
        if let Some(w) = c.desktop.widgets.get_mut(&id) {
            f(w);
        }
        dw::sync_depth_clock(c);
    });
}

/// Remembers the widgets as they are, for Undo.
fn push_undo(cx: &mut App) {
    let Some(m) = model(cx) else { return };
    let snapshot = config(cx).read(cx).get().desktop.widgets.clone();
    m.update(cx, |m, cx| {
        let undo = Rc::make_mut(&mut m.undo);
        if undo.last() != Some(&snapshot) {
            undo.push(snapshot);
            if undo.len() > UNDO_DEPTH {
                undo.remove(0);
            }
            cx.notify();
        }
    });
}

fn undo(cx: &mut App) {
    let Some(m) = model(cx) else { return };
    let Some(snapshot) = m.update(cx, |m, cx| {
        cx.notify();
        m.held = None;
        Rc::make_mut(&mut m.undo).pop()
    }) else {
        return;
    };
    update_config(cx, move |c| {
        c.desktop.widgets = snapshot;
        dw::sync_depth_clock(c);
    });
}

fn new_widget(kind: NewWidget) -> DesktopWidget {
    let plane = |k: WidgetKind| DesktopWidget { kind: k, ..DesktopWidget::default() };
    match kind {
        NewWidget::Clock => plane(WidgetKind::Clock(ClockWidget::default())),
        NewWidget::Monitor => plane(WidgetKind::Monitor(MonitorWidget::default())),
        // The Alexandria line (an empty decoration would be invisible).
        NewWidget::Decoration => Preset::AlexandriaLine.widgets().into_iter().next().unwrap_or_else(|| plane(WidgetKind::Decoration(Default::default()))),
        NewWidget::Visualizer => plane(WidgetKind::Visualizer(PlaneVisualizer::default())),
        NewWidget::NowPlaying => dc::new_card(NOW_PLAYING, Anchor::Center),
        NewWidget::Calendar => dc::new_card(CALENDAR, Anchor::Center),
    }
}

/// Adds a `kind` widget on `output` with its anchor point at `at`
/// (logical px; `None` = the centre), entering edit mode with it selected.
pub fn add_at(kind: NewWidget, output: &str, at: Option<(f32, f32)>, cx: &mut App) {
    enter_on(Some(output), cx);
    let Some(m) = model(cx) else { return };
    let cfg = config(cx).read(cx).snapshot();
    let size = output_size(output, m.read(cx), cx);
    let mut w = new_widget(kind);
    let card = matches!(w.kind, WidgetKind::Card(_));
    let (anchor, x, y) = match at {
        Some(p) if card => {
            let a = crate::cards::usable_area(&cfg, output, size);
            let (ax, ay) = (f32::from(a.origin.x), f32::from(a.origin.y));
            let (anchor, x, y) = geom::place_at((p.0 - ax, p.1 - ay), (f32::from(a.size.width), f32::from(a.size.height)));
            let stored = geom::card_stored_place(&Placement { anchor, x, y, ..w.place.clone() }, crate::bidi::rtl());
            (stored.anchor, stored.x, stored.y)
        }
        Some(p) => geom::place_at(p, size),
        None => (Anchor::Center, 0.0, 0.0),
    };
    (w.place.anchor, w.place.x, w.place.y) = (anchor, x, y);
    // On a multi-display desk it goes where it was asked for.
    if outputs(cx).len() > 1 {
        w.place.displays = vec![output.to_string()];
    }
    push_undo(cx);
    let added = Rc::new(std::cell::RefCell::new(None));
    let out = added.clone();
    update_config(cx, move |c| {
        dw::migrate_depth_clock(c);
        *out.borrow_mut() = c.desktop.widgets.add_new(w);
        dw::sync_depth_clock(c);
    });
    let id = added.take();
    m.update(cx, |m, cx| {
        m.selected = id;
        m.popup = None;
        cx.notify();
    });
}

fn remove(id: &str, cx: &mut App) {
    push_undo(cx);
    let id = id.to_string();
    let gone = id.clone();
    update_config(cx, move |c| {
        dw::migrate_depth_clock(c);
        c.desktop.widgets.items.retain(|w| w.id != gone);
        dw::sync_depth_clock(c);
    });
    if let Some(m) = model(cx) {
        m.update(cx, |m, cx| {
            if m.selected.as_deref() == Some(id.as_str()) {
                m.selected = None;
            }
            cx.notify();
        });
    }
}

fn duplicate(id: &str, cx: &mut App) {
    let Some(mut copy) = config(cx).read(cx).get().desktop.widgets.get(id).cloned() else { return };
    push_undo(cx);
    copy.place.x = (copy.place.x + DUPLICATE_OFFSET).clamp(dw::ranges::OFFSET.0, dw::ranges::OFFSET.1);
    copy.place.y = (copy.place.y + DUPLICATE_OFFSET).clamp(dw::ranges::OFFSET.0, dw::ranges::OFFSET.1);
    let added = Rc::new(std::cell::RefCell::new(None));
    let out = added.clone();
    update_config(cx, move |c| {
        dw::migrate_depth_clock(c);
        *out.borrow_mut() = c.desktop.widgets.add(copy);
        dw::sync_depth_clock(c);
    });
    if let (Some(m), Some(id)) = (model(cx), added.take()) {
        m.update(cx, |m, cx| {
            m.selected = Some(id);
            cx.notify();
        });
    }
}

fn hide(id: &str, cx: &mut App) {
    push_undo(cx);
    write(id, |w| w.enabled = false, cx);
}

/// The depth clock can't be duplicated (only the first depth face shows).
fn can_duplicate(id: &str, cfg: &ShellConfig) -> bool {
    cfg.desktop.widgets.get(id).is_some_and(|w| !matches!(&w.kind, WidgetKind::Clock(c) if c.face.depth_style().is_some()))
}

fn run_popup(action: Action, popup: &Popup, cx: &mut App) {
    if let Some(m) = model(cx) {
        m.update(cx, |m, cx| {
            m.popup = None;
            cx.notify();
        });
    }
    match action {
        Action::Add(kind) => {
            let at = match popup.kind {
                PopupKind::Add(at) => at,
                PopupKind::Widget(_) => None,
            };
            add_at(kind, &popup.output, at, cx);
        }
        Action::Remove(id) => remove(&id, cx),
        Action::Duplicate(id) => duplicate(&id, cx),
        Action::Hide(id) => hide(&id, cx),
        Action::WidgetSettings => crate::state::open_studio_at(Some("desktop-widgets".into()), cx),
        _ => {}
    }
}

fn popup_entries(kind: &PopupKind, cfg: &ShellConfig) -> Vec<Entry> {
    match kind {
        PopupKind::Add(_) => menu::add_entries(),
        PopupKind::Widget(id) => vec![
            menu::entry(0, "copy", t("Duplicate"), Action::Duplicate(id.clone())).enabled(can_duplicate(id, cfg)),
            menu::entry(0, "eye-off", t("Hide"), Action::Hide(id.clone())),
            menu::entry(1, "sliders", t("Settings in Studio"), Action::WidgetSettings),
            menu::entry(2, "trash", t("Remove"), Action::Remove(id.clone())).danger(),
        ],
    }
}

// ── Gestures ─────────────────────────────────────────────────────────────

fn start(item: Item, output: &str, mode: Mode, at: (f32, f32), cx: &mut App) {
    let Some(m) = model(cx) else { return };
    let place = config(cx).read(cx).get().desktop.widgets.get(&item.id).map(|w| w.place.clone()).unwrap_or_default();
    let size = output_size(output, m.read(cx), cx);
    let scale = place.scale();
    m.update(cx, |m, cx| {
        m.selected = Some(item.id.clone());
        m.popup = None;
        m.held = None;
        m.drag = Some(Drag { preview: item.bounds, outer: item.outer, item, output: output.to_string(), size, mode, from: at, place, moved: false, scale });
        cx.notify();
    });
}

/// The point a scale drag keeps still.
fn fixed_point(it: &Item, rtl: bool) -> (f32, f32) {
    let b = it.bounds;
    match it.kind {
        Kind::DepthClock => (b[0] + b[2] / 2.0, b[1] + b[3] / 2.0),
        _ if rtl => (b[0] + b[2], b[1]),
        _ => (b[0], b[1]),
    }
}

/// Card placement for a box at `outer` (screen px) in its area.
fn card_offsets(place: &Placement, area: &Area, outer: Box4, rtl: bool) -> Placement {
    let mut sp = geom::card_screen_place(place, rtl);
    let local = [outer[0] - area.x0, outer[1] - area.y0, outer[2], outer[3]];
    (sp.x, sp.y) = geom::offsets(&sp, area.size(), local);
    geom::card_stored_place(&sp, rtl)
}

fn on_move(output: &str, at: (f32, f32), free: bool, cx: &mut App) {
    let Some(m) = model(cx) else { return };
    let Some(mut d) = m.read(cx).drag.clone().filter(|d| d.output == output) else { return };
    let (dx, dy) = (at.0 - d.from.0, at.1 - d.from.1);
    if !d.moved && dx.hypot(dy) < DRAG_SLOP {
        return;
    }
    if !d.moved {
        push_undo(cx);
        d.moved = true;
    }
    let rtl = crate::bidi::rtl();
    let it = d.item.clone();
    let mut guides = Vec::new();
    let mut place = d.place.clone();
    match d.mode {
        Mode::Move => match it.kind {
            Kind::Plane => {
                let mv = geom::drag(it.bounds, it.outer, (dx, dy), &it.area, free);
                (place.x, place.y) = geom::offsets(&d.place, d.size, mv.outer);
                (d.preview, d.outer, guides) = (mv.bounds, mv.outer, mv.guides);
            }
            Kind::DepthClock => {
                let mv = geom::drag(it.bounds, it.bounds, (dx, dy), &it.area, free);
                let (fx, fy) = d.place.anchor.fraction();
                place.x = (d.place.x + (mv.bounds[0] - it.bounds[0]) / d.size.0.max(1.0)).clamp(-fx, 1.0 - fx);
                place.y = (d.place.y + (mv.bounds[1] - it.bounds[1]) / d.size.1.max(1.0)).clamp(-fy, 1.0 - fy);
                (d.preview, d.outer, guides) = (mv.bounds, mv.bounds, mv.guides);
            }
            Kind::Card { strip: false } => {
                let mv = geom::drag(it.bounds, it.bounds, (dx, dy), &it.area, free);
                place = card_offsets(&d.place, &it.area, mv.bounds, rtl);
                (d.preview, d.outer, guides) = (mv.bounds, mv.bounds, mv.guides);
            }
            Kind::Card { strip: true } => {
                let mv = geom::drag(it.bounds, it.bounds, (0.0, dy), &it.area, true);
                place.anchor = if mv.bounds[1] + mv.bounds[3] / 2.0 < (it.area.y0 + it.area.y1) / 2.0 { Anchor::Top } else { Anchor::Bottom };
                (place.x, place.y) = (0.0, 0.0);
                (d.preview, d.outer) = (mv.bounds, mv.bounds);
            }
        },
        Mode::Scale => {
            let fixed = fixed_point(&it, rtl);
            let (s, f) = geom::scale_by(d.place.scale(), fixed, d.from, at);
            place.scale = s;
            d.scale = s;
            d.preview = geom::scaled(it.bounds, fixed, f);
            d.outer = geom::scaled(it.outer, fixed, f);
            match it.kind {
                Kind::Plane => (place.x, place.y) = geom::offsets(&d.place, d.size, d.outer),
                Kind::Card { strip: false } => {
                    let p = card_offsets(&d.place, &it.area, d.outer, rtl);
                    (place.x, place.y) = (p.x, p.y);
                }
                _ => {}
            }
        }
    }
    let id = it.id.clone();
    let place = Placement { displays: d.place.displays.clone(), depth: d.place.depth, opacity: d.place.opacity, ..place };
    // Wallpaper widgets move in the renderer only until release; cards are
    // their own surfaces, placed from the config.
    let live = matches!(it.kind, Kind::Plane | Kind::DepthClock);
    if !live {
        let card = place.clone();
        write(&id, move |w| w.place = card, cx);
    }
    let output = output.to_string();
    m.update(cx, |m, cx| {
        m.guides = guides.into_iter().map(|g| (output.clone(), g)).collect();
        m.drag = Some(d);
        if live {
            m.live = Some((id, place));
        }
        cx.notify();
    });
    if live {
        crate::backdrop::refresh(cx);
    }
}

fn on_up(cx: &mut App) {
    let Some(m) = model(cx) else { return };
    let Some(d) = m.update(cx, |m, cx| {
        m.guides.clear();
        cx.notify();
        m.drag.take()
    }) else {
        return;
    };
    if !d.moved {
        return;
    }
    let rtl = crate::bidi::rtl();
    let it = &d.item;
    let mut held = d.preview;
    // The dragged wallpaper widget's place, written now in one update.
    let live = m.update(cx, |m, _| m.live.take()).filter(|(id, _)| *id == it.id).map(|(_, p)| p);
    if let Some(p) = live.clone() {
        let reanchor = d.mode == Mode::Move && it.kind == Kind::Plane;
        let (size, outer) = (d.size, d.outer);
        write(&it.id, move |w| w.place = if reanchor { geom::reanchor(&p, size, outer) } else { p }, cx);
    }
    if d.mode == Mode::Move {
        match it.kind {
            Kind::Plane => {}
            Kind::Card { strip: false } => {
                let (area, outer) = (it.area, d.outer);
                write(
                    &it.id,
                    move |w| {
                        let sp = geom::card_screen_place(&w.place, rtl);
                        let local = [outer[0] - area.x0, outer[1] - area.y0, outer[2], outer[3]];
                        w.place = geom::card_stored_place(&geom::reanchor(&sp, area.size(), local), rtl);
                    },
                    cx,
                );
            }
            Kind::Card { strip: true } => {
                let top = held[1] + held[3] / 2.0 < (it.area.y0 + it.area.y1) / 2.0;
                held[1] = if top { it.area.y0 } else { it.area.y1 - held[3] };
            }
            Kind::DepthClock => {}
        }
    }
    let id = it.id.clone();
    m.update(cx, |m, cx| {
        m.held = Some((id, held, Instant::now()));
        cx.notify();
    });
    // Let a held outline that never matched go back to the report.
    cx.spawn(async move |cx| {
        cx.background_executor().timer(HELD_FOR).await;
        cx.update(|cx| {
            if let Some(m) = model(cx) {
                m.update(cx, |_, cx| cx.notify());
            }
        });
    })
    .detach();
}

/// The renderer's report of where the widgets are painted.
pub(crate) fn on_boxes(boxes: Vec<OutputBoxes>, cx: &mut App) {
    let Some(m) = model(cx) else { return };
    m.update(cx, |m, cx| {
        if let Some((id, held, _)) = &m.held {
            let arrived = boxes.iter().flat_map(|b| b.widgets.iter().map(|g| (g.id.as_str(), g.bounds)).chain(b.clock.map(|c| ("", c)))).any(|(gid, b)| {
                (gid == id || gid.is_empty()) && (0..4).all(|i| (b[i] - held[i]).abs() <= HELD_MATCH_PX)
            });
            if arrived {
                m.held = None;
            }
        }
        m.boxes = boxes;
        cx.notify();
    });
}

// ── Entering and leaving ─────────────────────────────────────────────────

/// Enters edit mode (on the output under the keyboard's focus). From a
/// process without the desktop (Studio), asks the shell to.
pub fn enter(cx: &mut App) {
    enter_on(None, cx);
}

/// Enters edit mode with the keyboard on `output`.
pub fn enter_on(output: Option<&str>, cx: &mut App) {
    if cx.has_global::<crate::state::ConfigMirror>() {
        // Studio: the shell owns the desktop.
        cx.background_executor()
            .spawn(async move {
                use std::io::{Read as _, Write as _};
                let sent = std::os::unix::net::UnixStream::connect(mandara_config::paths::ipc_socket()).and_then(|mut s| {
                    s.write_all(IPC_REQUEST.as_bytes())?;
                    let mut reply = Vec::new();
                    s.read_to_end(&mut reply).map(|_| ())
                });
                if let Err(err) = sent {
                    tracing::warn!("desktop edit: the shell did not answer: {err}");
                }
            })
            .detach();
        return;
    }
    if active(cx) {
        return;
    }
    crate::desktop_menu::close(cx);
    let model = cx.new(|_| Model::default());
    cx.set_global(Edit { model: model.clone(), windows: Vec::new() });
    let outs = outputs(cx);
    let keyboard_on = output.map(str::to_string).or_else(|| crate::hub::hub(cx).read(cx).focused.as_ref().map(|c| c.monitor.clone())).filter(|o| outs.iter().any(|g| g.name == *o)).or_else(|| outs.first().map(|g| g.name.clone()));
    let mut windows = Vec::new();
    for g in &outs {
        let keyboard = if Some(&g.name) == keyboard_on.as_ref() { KeyboardInteractivity::Exclusive } else { KeyboardInteractivity::OnDemand };
        let options = crate::plugins::dock::layer_options(
            SurfaceRole::DesktopEdit,
            Layer::Top,
            LayerAnchor::TOP | LayerAnchor::BOTTOM | LayerAnchor::LEFT | LayerAnchor::RIGHT,
            (g.width as f32, g.height as f32),
            Some(menu::IGNORE_ZONES),
            keyboard,
            crate::plugins::dock::display_for(&g.name, cx),
        );
        let (name, m, focus_here) = (g.name.clone(), model.clone(), keyboard == KeyboardInteractivity::Exclusive);
        let opened = cx.open_window(options, move |window, cx| {
            cx.new(|cx| {
                let focus = cx.focus_handle();
                if focus_here {
                    focus.focus(window, cx);
                }
                let sub = cx.observe(&m, |_, _, cx| cx.notify());
                EditView { output: name, model: m, focus, appear: SpringValue::new(0.0, APPEAR_SPRING), clock: FrameClock::new(), _sub: sub }
            })
        });
        match opened {
            Ok(h) => windows.push((g.name.clone(), h)),
            Err(err) => tracing::warn!("desktop edit on {}: {err:#}", g.name),
        }
    }
    if cx.has_global::<Edit>() {
        cx.global_mut::<Edit>().windows = windows;
    }
    // The renderer starts reporting; hidden widgets and cards show.
    crate::backdrop::refresh(cx);
    crate::cards::refresh(cx);
}

/// Leaves edit mode: the surfaces, outlines and undo history go.
pub fn exit(cx: &mut App) {
    let Some(edit) = cx.try_global::<Edit>().map(|e| e.windows.clone()) else { return };
    cx.remove_global::<Edit>();
    for (_, h) in edit {
        let _ = h.update(cx, |_, window, _| window.remove_window());
    }
    crate::backdrop::refresh(cx);
    crate::cards::refresh(cx);
}

// ── The surface ──────────────────────────────────────────────────────────

struct EditView {
    output: String,
    model: Entity<Model>,
    focus: FocusHandle,
    appear: SpringValue<f32>,
    clock: FrameClock,
    _sub: Subscription,
}

fn pos(e: gpui::Point<gpui::Pixels>) -> (f32, f32) {
    (f32::from(e.x), f32::from(e.y))
}

impl EditView {
    fn key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let m = self.model.clone();
        let k = e.keystroke.key.as_str();
        let ctrl = e.keystroke.modifiers.control || e.keystroke.modifiers.platform;
        match k {
            "escape" if m.read(cx).popup.is_some() => m.update(cx, |m, cx| {
                m.popup = None;
                cx.notify();
            }),
            "escape" => cx.defer(exit),
            "z" if ctrl => undo(cx),
            "delete" | "backspace" => {
                if let Some(id) = m.read(cx).selected.clone() {
                    remove(&id, cx);
                }
            }
            _ => {}
        }
    }

    fn open_popup(&mut self, kind: PopupKind, at: (f32, f32), up: bool, cx: &mut Context<Self>) {
        let output = self.output.clone();
        self.model.update(cx, |m, cx| {
            if let PopupKind::Widget(id) = &kind {
                m.selected = Some(id.clone());
            }
            m.popup = Some(Popup { output, at, kind, hover: None, up });
            cx.notify();
        });
    }

    fn outline(&self, it: &Item, m: &Model, size: (f32, f32), palette: &MandaraPalette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let rtl = crate::bidi::rtl();
        let accent = readable(palette.primary, palette);
        let selected = m.selected.as_deref() == Some(it.id.as_str());
        let hovered = m.hover.as_deref() == Some(it.id.as_str());
        let dragging = m.drag.as_ref().filter(|d| d.item.id == it.id);
        let [x, y, w, h] = it.bounds;
        let (w2, h2) = (w.max(MIN_OUTLINE), h.max(MIN_OUTLINE));
        let (x, y) = (x - (w2 - w) / 2.0, y - (h2 - h) / 2.0);
        let line = if selected { accent } else { palette.on_surface.with_alpha(if hovered { HOVER_ALPHA } else { OUTLINE_ALPHA }) };
        let grab = it.clone();
        let menu_id = it.id.clone();
        let hover_id = it.id.clone();
        let body = div()
            .id(SharedString::from(format!("edit-box-{}", it.id)))
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(w2))
            .h(px(h2))
            .rounded(px(crate::style::radius(OUTLINE_RADIUS)))
            .border(px(if selected { SELECTED_W } else { OUTLINE_W }))
            .border_color(line.hsla())
            .when(selected, |el| el.bg(accent.with_alpha(SELECTED_FILL).hsla()))
            .cursor(if dragging.is_some() { CursorStyle::ClosedHand } else { CursorStyle::OpenHand })
            .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                let id = hover_id.clone();
                let on = *h;
                this.model.update(cx, |m, cx| {
                    if on {
                        m.hover = Some(id);
                    } else if m.hover.as_deref() == Some(id.as_str()) {
                        m.hover = None;
                    }
                    cx.notify();
                });
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                start(grab.clone(), &this.output, Mode::Move, pos(e.position), cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.open_popup(PopupKind::Widget(menu_id.clone()), pos(e.position), false, cx);
            }));

        // Name chip above the outline's start corner (inside when there is
        // no room above), with the scale while it is being resized.
        let scale_note = dragging.filter(|d| d.mode == Mode::Scale).map(|d| format!("{:.0}%", d.scale * 100.0));
        let chip_top = if y - CHIP_GAP - CHIP_H >= 0.0 { y - CHIP_GAP - CHIP_H } else { y + CHIP_GAP };
        let chip_bg = if selected { accent } else { menu::glass(palette) };
        let chip_fg = if selected { palette.on_primary } else { palette.on_surface };
        let grab2 = it.clone();
        let chip = div()
            .id(SharedString::from(format!("edit-chip-{}", it.id)))
            .absolute()
            .top(px(chip_top))
            .when(!rtl, |el| el.left(px(x)))
            .when(rtl, |el| el.right(px(size.0 - (x + w2))))
            .h(px(CHIP_H))
            .px(px(CHIP_PAD))
            .rounded(px(CHIP_H / 2.0))
            .bg(chip_bg.hsla())
            .shadow(rim(palette))
            .row()
            .items_center()
            .gap(px(text::LABEL / 2.0))
            .cursor(CursorStyle::OpenHand)
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                start(grab2.clone(), &this.output, Mode::Move, pos(e.position), cx);
            }))
            .child(icon(it.icon, CHIP_GLYPH, chip_fg))
            .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::MEDIUM).text_color(chip_fg.hsla()).child(it.label.clone()))
            .children(scale_note.map(|s| div().text_size(px(text::LABEL)).text_color(chip_fg.with_alpha(menu_hint_alpha()).hsla()).font_features(crate::fonts::tabular_nums()).child(s)))
            // One click removes it (Undo brings it back).
            .child({
                let gone = it.id.clone();
                div()
                    .id(SharedString::from(format!("edit-remove-{}", it.id)))
                    .size(px(CHIP_REMOVE))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(palette.urgent_coral.with_alpha(CHIP_REMOVE_HOVER_ALPHA).hsla()))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        remove(&gone, cx);
                    })
                    .child(icon("close", CHIP_GLYPH, chip_fg))
            });

        // The resize knob on the bottom-end corner.
        let resizable = it.kind != (Kind::Card { strip: true });
        let handle = (resizable && (selected || hovered)).then(|| {
            let (hx, hy) = (if rtl { x } else { x + w2 }, y + h2);
            let grab3 = it.clone();
            div()
                .id(SharedString::from(format!("edit-handle-{}", it.id)))
                .absolute()
                .left(px(hx - HANDLE_HIT / 2.0))
                .top(px(hy - HANDLE_HIT / 2.0))
                .size(px(HANDLE_HIT))
                .flex()
                .items_center()
                .justify_center()
                .cursor(if rtl { CursorStyle::ResizeUpRightDownLeft } else { CursorStyle::ResizeUpLeftDownRight })
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    start(grab3.clone(), &this.output, Mode::Scale, pos(e.position), cx);
                }))
                .child(div().size(px(HANDLE)).rounded_full().bg(palette.surface_base.hsla()).border(px(SELECTED_W)).border_color(accent.hsla()).shadow(elevation(palette, 1.0)))
        });
        let mut out = vec![body.into_any_element(), chip.into_any_element()];
        out.extend(handle.map(IntoElement::into_any_element));
        out
    }

    fn toolbar(&self, m: &Model, size: (f32, f32), safe: Insets, palette: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let accent = readable(palette.primary, palette);
        let can_undo = !m.undo.is_empty();
        let button = |id: &'static str, glyph: &'static str, label: &'static str, enabled: bool| {
            div()
                .id(id)
                .h(px(BTN_H))
                .px(px(BTN_PAD))
                .rounded(px(BTN_H / 2.0))
                .row()
                .items_center()
                .gap(px(BAR_PAD))
                .text_size(px(text::BODY))
                .when(enabled, |el| el.cursor_pointer().hover(|s| s.bg(palette.on_surface.with_alpha(BTN_HOVER).hsla())))
                .when(!enabled, |el| el.opacity(DISABLED_ALPHA))
                .child(icon(glyph, BTN_GLYPH, palette.on_surface_variant))
                .child(label)
        };
        let divider = || div().w(px(1.0)).h(px(DIVIDER_H)).bg(palette.on_surface.with_alpha(DIVIDER_ALPHA).hsla());
        let bar = div()
            .id("edit-toolbar")
            .occlude()
            .h(px(BAR_H))
            .p(px(BAR_PAD))
            .rounded(px(BAR_H / 2.0))
            .bg(menu::glass(palette).hsla())
            .shadow({
                let mut s = elevation(palette, 1.0);
                s.extend(rim(palette));
                s
            })
            .row()
            .items_center()
            .gap(px(BAR_PAD / 2.0))
            .child(div().px(px(BTN_PAD)).row().items_center().gap(px(BAR_PAD)).child(icon("pencil", BTN_GLYPH, accent)).child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(t("Edit desktop"))))
            .child(divider())
            .child(button("edit-add", "plus", t("Add widget"), true).on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                let at = pos(e.position());
                this.open_popup(PopupKind::Add(None), at, true, cx);
            })))
            .child(button("edit-undo", "undo", t("Undo"), can_undo).when(can_undo, |el| el.on_click(cx.listener(|_, _, _, cx| undo(cx)))))
            .child(button("edit-studio", "external", t("Open in Studio"), true).on_click(cx.listener(|_, _, _, cx| crate::state::open_studio_at(Some("desktop-widgets".into()), cx))))
            .child(
                div()
                    .id("edit-done")
                    .h(px(BTN_H))
                    .px(px(BTN_PAD))
                    .rounded(px(BTN_H / 2.0))
                    .bg(accent.hsla())
                    .text_color(palette.on_primary.hsla())
                    .row()
                    .items_center()
                    .gap(px(BAR_PAD))
                    .cursor_pointer()
                    .text_size(px(text::BODY))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(icon("check", BTN_GLYPH, palette.on_primary))
                    .child(t("Done"))
                    .on_click(cx.listener(|_, _, _, cx| cx.defer(exit))),
            );
        div().absolute().left_0().w(px(size.0)).top(px(size.1 - safe.bottom - BAR_MARGIN - BAR_H)).flex().justify_center().child(bar).into_any_element()
    }
}

fn menu_hint_alpha() -> f32 {
    crate::kit::alpha::SECONDARY_TEXT
}

impl Render for EditView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = mandara_core::frame_palette(window, cx);
        let f = crate::fonts::fonts(cx);
        let reduce = mandara_core::spring::motion_profile().reduce;
        self.appear.set_instant(reduce);
        self.appear.set_target(1.0);
        if self.appear.step(self.clock.tick(Instant::now())) {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        let p = self.appear.value().clamp(0.0, 1.0);
        let cfg = config(cx).read(cx).snapshot();
        let m = self.model.read(cx).clone();
        let size = output_size(&self.output, &m, cx);
        let safe = safe_of(&self.output, &m, &cfg);
        let items = items(&self.output, &cfg, &m, cx);
        let accent = readable(palette.primary, &palette);
        let guides: Vec<Guide> = m.guides.iter().filter(|(o, _)| *o == self.output).map(|(_, g)| *g).collect();
        let popup = m.popup.clone().filter(|p| p.output == self.output);
        let outlines: Vec<AnyElement> = items.iter().flat_map(|it| self.outline(it, &m, size, &palette, cx)).collect();
        let toolbar = self.toolbar(&m, size, safe, &palette, cx);
        let guide_els = guides.into_iter().map(|g| match g {
            Guide::Vertical(x) => div().absolute().top_0().left(px(x - GUIDE_W / 2.0)).w(px(GUIDE_W)).h(px(size.1)).bg(accent.with_alpha(GUIDE_ALPHA).hsla()),
            Guide::Horizontal(y) => div().absolute().left_0().top(px(y - GUIDE_W / 2.0)).h(px(GUIDE_W)).w(px(size.0)).bg(accent.with_alpha(GUIDE_ALPHA).hsla()),
        });
        let popup_el = popup.map(|pp| {
            let entries = popup_entries(&pp.kind, &cfg);
            let h = menu::height(&entries);
            let w = menu::MENU_W;
            let at = if pp.up { (pp.at.0 - w / 2.0, pp.at.1) } else { pp.at };
            let ((x, y), _) = menu::place(at, (w, h), size, crate::bidi::rtl() && !pp.up, pp.up);
            let pick: OnRow<Self> = Rc::new(move |this, i, _, cx| {
                let Some(pp) = this.model.read(cx).popup.clone() else { return };
                let cfg = config(cx).read(cx).snapshot();
                if let Some(e) = popup_entries(&pp.kind, &cfg).get(i).filter(|e| e.enabled) {
                    let action = e.action.clone();
                    cx.defer(move |cx| run_popup(action, &pp, cx));
                }
            });
            let hover: OnHover<Self> = Rc::new(|this, i, cx| {
                this.model.update(cx, |m, cx| {
                    if let Some(p) = m.popup.as_mut()
                        && p.hover != Some(i)
                    {
                        p.hover = Some(i);
                        cx.notify();
                    }
                });
            });
            menu::card("edit-popup-row", &entries, pp.hover, w, 1.0, &palette, cx, pick, hover).id("edit-popup").occlude().absolute().left(px(x)).top(px(y))
        });
        let output = self.output.clone();
        div()
            .id("desktop-edit-root")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .font_family(f.ui.clone())
            .text_color(palette.on_surface.hsla())
            .opacity(p)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.key(e, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.focus.focus(window, cx);
                this.model.update(cx, |m, cx| {
                    m.popup = None;
                    m.selected = None;
                    cx.notify();
                });
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                let at = pos(e.position);
                this.open_popup(PopupKind::Add(Some(at)), at, false, cx);
            }))
            .on_mouse_move(cx.listener(move |_, e: &MouseMoveEvent, _, cx| {
                if e.pressed_button == Some(MouseButton::Left) {
                    on_move(&output, pos(e.position), e.modifiers.shift, cx);
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|_, _: &MouseUpEvent, _, cx| on_up(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|_, _: &MouseUpEvent, _, cx| on_up(cx)))
            .children(guide_els)
            .children(outlines)
            .child(toolbar)
            .children(popup_el)
    }
}

// ── Commands ─────────────────────────────────────────────────────────────

pub fn register(r: &mut PluginRegistry) {
    let core = "core";
    r.command(
        core,
        CommandSpec::new("desktop edit", tk("Edit the desktop: move, resize, add and remove desktop widgets on the desktop itself (off / toggle leave)"), |a, cx| {
            let on = match a.get(0).unwrap_or("on") {
                "off" => false,
                "toggle" => !active(cx),
                _ => true,
            };
            if on { enter(cx) } else { exit(cx) }
            Ok(json!({ "editing": on }))
        })
        .arg(ArgSpec::optional("action", ArgKind::Enum, tk("what to do")).choices(&["on", "off", "toggle"]))
        .example("desktop edit")
        .example("desktop edit off"),
    );
    r.command(
        core,
        CommandSpec::new("desktop widgets", tk("Show or hide every desktop widget at once (none is switched off)"), |a, cx| {
            let hidden = match a.str(0)? {
                "hide" => true,
                "show" => false,
                _ => !config(cx).read(cx).get().desktop.widgets.hidden,
            };
            update_config(cx, move |c| c.desktop.widgets.hidden = hidden);
            Ok(json!({ "hidden": hidden }))
        })
        .arg(ArgSpec::required("action", ArgKind::Enum, tk("what to do")).choices(&["show", "hide", "toggle"]))
        .example("desktop widgets hide"),
    );
    r.command(
        core,
        CommandSpec::new("desktop menu", tk("Open the desktop's right-click menu at a point of a display (logical px)"), |a, cx| {
            let output = a.str(0)?.to_string();
            let x: f32 = a.str(1)?.parse().map_err(|_| t("Expected a number").to_string())?;
            let y: f32 = a.str(2)?.parse().map_err(|_| t("Expected a number").to_string())?;
            crate::desktop_menu::open(&output, (x, y), cx);
            Ok(json!({ "output": output, "x": x, "y": y }))
        })
        .arg(ArgSpec::required("output", ArgKind::String, tk("Connector name, e.g. DP-1 (see compositor monitors)")))
        .arg(ArgSpec::required("x", ArgKind::Number, tk("logical px from the left")))
        .arg(ArgSpec::required("y", ArgKind::Number, tk("logical px from the top")))
        .example("desktop menu DP-1 800 500"),
    );
}
