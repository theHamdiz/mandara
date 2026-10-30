//! The per-output shell surface: one full-screen transparent layer that hosts
//! the bar, every plugin panel and the sidebar.
//!
//! * The bar's zones are runs of plugin slots; each slot is a soft chip whose
//!   content is the plugin's bar view, measured at its intrinsic width.
//! * Clicking a slot pours that plugin's panel out of the bar (edge frame) or
//!   morphs it out of the pill (floating layouts) on an expressive spring.
//! * The sidebar slides in from its edge (left, or right for RTL) on hover
//!   or command; the session dock uses the opposite edge.
//! * Every frame the interactive rects are pushed to the compositor as the
//!   input region, so all transparent pixels click through.

use crate::bidi::Bidi as _;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, AnyView, App, AppContext as _, Bounds, ClickEvent, Context, Entity, EntityId, FocusHandle, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Pixels, Render, StatefulInteractiveElement, StyleRefinement, Styled, Task,
    Window, canvas, div, point, prelude::FluentBuilder as _, px, size,
};
use mandara_config::surfaces::{BarSeparator, SidebarHeader, TabPosition, ranges};
use mandara_config::{BarLayout, BarPosition, ConfigStore, ShellConfig};
use mandara_core::{
    Edge, FlareCenter, FrameClock, InputMaskRegistry, SpringConfig, SpringValue, ToGpui, concave_flare, edge_drawer_snapped,
};
use mandara_plugins::{ClickHandler, PanelCx, PanelSpec, PluginRegistry};
use mandara_theme::{Color, MandaraPalette};

use crate::bar::layout::{self, BarGeometry, Metrics, PILL_INSET};
use crate::fonts::fonts;
use crate::kit::{reveal, tokens};
use crate::state::{ShellState, config, shell};

/// Opaque fill of the frame, panels and sidebar: a deep tonal surface that
/// reads crisply over anything (no see-through patchiness).
pub fn frame_color(palette: &MandaraPalette) -> Color {
    if palette.is_dark {
        palette.surface_dim.mix(palette.surface_base, FRAME_DARK_MIX)
    } else {
        palette.surface_base
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One plugin placed in a bar zone.
struct Slot {
    id: String,
    /// The plugin factory generation this instance was built from.
    generation: u64,
    /// The plugin's bar view wrapped in a cached [`ChipHost`], so the bar
    /// item only rebuilds when the plugin itself notifies — not on every
    /// shell frame (hover springs, panel morphs, other chips).
    host: Option<Entity<ChipHost>>,
    panel: Option<PanelSpec>,
    panel_cx: PanelCx,
    on_click: Option<ClickHandler>,
    on_secondary_click: Option<ClickHandler>,
    measured: Rc<Cell<f32>>,
    measured_cross: Rc<Cell<f32>>,
    /// Built for a side bar (its chip host stacks content in a column).
    vertical: bool,
    hover: SpringValue<f32>,
    hovered: bool,
}

/// Which plugins are placed, for change detection.
/// (bar zones, sidebar tabs' cards, disabled, notifications tab, enabled
/// opt-ins, registry generation).
type Placement = ([Vec<String>; 3], Vec<(String, Vec<String>)>, Vec<String>, bool, Vec<String>, u64);

pub struct ShellView {
    config: Entity<ConfigStore>,
    shell: Entity<ShellState>,
    monitor: String,
    mask: InputMaskRegistry,
    focus: FocusHandle,
    clock: FrameClock,
    surface: (f32, f32),

    zones: [Vec<Slot>; 3],
    /// The sidebar's tabs: cached card lists, selection, menus.
    tabs: crate::sidebar_tabs::SidebarTabs,
    /// The notifications list shown in its own sidebar tab, when enabled.
    sidebar_notifications: Option<AnyView>,
    /// The main menu panel (right-click on empty bar space).
    menu: Option<(PanelSpec, PanelCx)>,
    /// The detached panel's plugin id and instance (see [`DETACHED_SLOT`]).
    detached: Option<(String, PanelSpec, PanelCx)>,
    placement: Option<Placement>,
    /// The slots were built for a side bar.
    vertical_slots: bool,

    /// This frame's resolved bar / panel geometry settings.
    metrics: Metrics,
    /// The panel / sidebar springs use a custom (untuned) feel.
    panel_custom: bool,
    sidebar_custom: bool,
    groups: [SpringValue<Bounds<Pixels>>; 3],
    pill_alpha: SpringValue<f32>,
    pill_radius: SpringValue<Pixels>,
    backdrop: SpringValue<Bounds<Pixels>>,
    applied: Option<LayoutKey>,

    panel: SpringValue<Bounds<Pixels>>,
    panel_slot: Option<(usize, usize)>,
    panel_visible: bool,
    panel_entered: bool,
    /// A press landed inside the open panel: it now stays until a click
    /// outside, Esc or its bar button — never on pointer drift.
    panel_committed: bool,
    /// Where the open panel rested before its content resized it: a leave
    /// or an outside press inside it was caused by the panel moving.
    panel_shadow: crate::dismiss::ResizeShadow,
    sidebar: SpringValue<f32>,
    leave_task: Option<Task<()>>,
    edge_task: Option<Task<()>>,
    menu_anchor: f32,
    /// Last mouse press anywhere on this surface. Focus/activation changes
    /// shortly after one (e.g. pressing play makes the player window emit
    /// focus events) must never dismiss the panel being used.
    last_press: Option<Instant>,
    /// Pending confirmation that keyboard focus really left the surface.
    deactivate_task: Option<Task<()>>,
    /// Progress handle shared with the sidebar cards' staggered reveal.
    sidebar_cx: PanelCx,
    /// Bar auto-hide / per-output visibility, and this frame's bar shift.
    autohide: crate::bar::autohide::AutoHide,
    bar_shift: f32,
    /// Hot corners (and rounded screen corners).
    corners: crate::corners::HotCorners,
}

/// Everything the bar's pill geometry ([`layout::compute`]) depends on: the
/// pills re-target whenever any of it changes (a Studio slider included).
#[derive(Clone, Debug, PartialEq)]
struct LayoutKey {
    widths: [f32; 3],
    surface: (u32, u32),
    /// Style, position, height, gaps, radii (after the global roundness),
    /// chip spacing, alignment.
    metrics: Metrics,
}

fn layout_key(widths: [f32; 3], metrics: Metrics, surface: (f32, f32)) -> LayoutKey {
    LayoutKey { widths: widths.map(|w| w.round()), surface: (surface.0 as u32, surface.1 as u32), metrics }
}

/// Blend of `surface_dim` toward `surface_base` for the dark frame fill.
const FRAME_DARK_MIX: f32 = 0.55;
/// Corner radius of the floating pills before the first layout, in px.
const INITIAL_PILL_RADIUS_PX: f32 = 12.0;
/// Element-id stride separating the slots of the three bar zones.
const SLOT_ID_STRIDE: usize = 100;
/// Width of the virtual chip that main-menu / detached panels morph out of.
const ANCHOR_CHIP_W_PX: f32 = 40.0;
/// Base UI text size before the typography scale, in px.
const BASE_TEXT_PX: f32 = 12.5;
/// Bar opacity at or above which fills count as opaque (overlap seams).
const OPAQUE_ALPHA: f32 = 0.999;
/// Specular rim highlight / shade alphas of the floating glass pills.
const RIM_LIGHT_ALPHA: f32 = 0.12;
const RIM_SHADOW_ALPHA: f32 = 0.3;
/// Spring value below which a fading piece is treated as fully hidden.
const HIDDEN_EPSILON: f32 = 0.002;
/// Pill alpha below which the floating pill glass is skipped.
const PILL_VISIBLE_EPSILON: f32 = 0.001;
/// Largest sidebar spring overshoot honoured when drawing.
const SIDEBAR_OVERSHOOT_MAX: f32 = 1.2;
/// Chip radius used before any chip has been measured, in px.
const FALLBACK_CHIP_RADIUS_PX: f32 = 10.0;
/// Bar separators: dot diameter, line width and its share of the chip
/// height, and their ink alpha.
const SEPARATOR_DOT_PX: f32 = 3.0;
const SEPARATOR_LINE_W_PX: f32 = 1.0;
const SEPARATOR_LINE_FRAC: f32 = 0.5;
const SEPARATOR_ALPHA: f32 = 0.22;
/// Alpha of the hairline bar's accent line.
const ACCENT_LINE_ALPHA: f32 = 0.7;
/// Compact sidebar header: button / tab height.
const SIDEBAR_COMPACT_BUTTON_PX: f32 = 24.0;
/// State-layer alphas of bar chips (active panel / hover).
const ACTIVE_CHIP_TINT_ALPHA: f32 = 0.16;
const HOVER_CHIP_TINT_ALPHA: f32 = 0.08;
/// Lightness boost of an active chip's text.
const ACTIVE_TEXT_LIGHTEN: f32 = 0.06;
/// Where unmeasured bar items are laid out (off-screen left), in px.
const OFFSCREEN_X_PX: f32 = -10_000.0;
/// Width changes smaller than this don't re-layout the bar, in px.
const MEASURE_EPSILON_PX: f32 = 0.5;
/// Morph progress after which a panel counts as settled ("entered").
const PANEL_SETTLED_PROGRESS: f32 = 0.92;
/// Horizontal overhang of the hover bridge between a pill and its panel.
const BRIDGE_OVERHANG_PX: f32 = 8.0;
/// Panel content fades in over this morph-progress window.
const CONTENT_FADE_FROM: f32 = 0.25;
const CONTENT_FADE_TO: f32 = 0.7;
/// Sidebar tab strip metrics, in px.
const SIDEBAR_TAB_H_PX: f32 = 30.0;
const SIDEBAR_TAB_STRIP_PAD_PX: f32 = 3.0;
const SIDEBAR_PAD_PX: f32 = 14.0;
const SIDEBAR_ICON_BUTTON_PX: f32 = 30.0;
const PIN_TINT_ALPHA: f32 = 0.14;
/// Fraction of the free height left clear above/below the sidebar trigger.
const SIDEBAR_TRIGGER_INSET: f32 = 0.1;
/// Launcher / session edge triggers: start and length along their edge as
/// fractions of the surface.
const EDGE_TRIGGER_START: f32 = 0.3;
const EDGE_TRIGGER_SPAN: f32 = 0.4;
/// Exclusive-zone changes smaller than this are ignored, in px.
const ZONE_EPSILON_PX: f32 = 0.5;

/// Hosts one plugin's bar view as its own (cached) GPUI view and records
/// the content's intrinsic width. GPUI re-renders a cached view only when it
/// or a descendant notified, so the 10–20 bar items no longer rebuild on
/// every shell frame; the measurement canvas re-runs exactly when the
/// content may have changed size.
pub struct ChipHost {
    view: AnyView,
    /// The content's extent along the bar (width; height in a side bar).
    measured: Rc<Cell<f32>>,
    /// Its extent across the bar (a side bar hides content wider than it).
    measured_cross: Rc<Cell<f32>>,
    /// Laid out for a side bar: content stacks in a column.
    vertical: bool,
    shell: EntityId,
}

impl Render for ChipHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let measured = self.measured.clone();
        let measured_cross = self.measured_cross.clone();
        let shell = self.shell;
        let vertical = self.vertical;
        div().size_full().relative().child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .when(!vertical, |el| el.h_full().flex().items_center())
                .when(vertical, |el| el.flex().flex_col().items_center())
                .whitespace_nowrap()
                .child(self.view.clone())
                .child(
                    canvas(
                        move |bounds, window, _| {
                            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                            let (along, across) = if vertical { (h, w) } else { (w, h) };
                            let moved = (measured.get() - along).abs() > MEASURE_EPSILON_PX || (vertical && (measured_cross.get() - across).abs() > MEASURE_EPSILON_PX);
                            if moved {
                                measured.set(along);
                                measured_cross.set(across);
                                window.on_next_frame(move |_, cx| cx.notify(shell));
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                ),
        )
    }
}

/// Width of screen-edge hover strips. Wide enough that the last pixels of
/// travel toward the edge produce motion events inside the strip (an edge-
/// clamped pointer sends no further motion after entering).
const EDGE_STRIP: f32 = 4.0;
// Shared focus-out settle (see `crate::dismiss`).
use crate::dismiss::DEACTIVATE_SETTLE;

/// Pseudo slot address of the main menu panel.
const MENU_SLOT: (usize, usize) = (usize::MAX, 0);
/// A plugin panel opened while the plugin isn't placed in the bar (from
/// the launcher or `panel open`): anchored at the bar's center.
const DETACHED_SLOT: (usize, usize) = (usize::MAX - 1, 0);

use mandara_config::surfaces::defaults as surface_defaults;
const PANEL_SPRING: SpringConfig = SpringConfig::new(surface_defaults::PANEL_SPRING_STIFFNESS, surface_defaults::PANEL_SPRING_DAMPING, 1.0);
const SIDEBAR_SPRING: SpringConfig = SpringConfig::new(surface_defaults::SIDEBAR_SPRING_STIFFNESS, surface_defaults::SIDEBAR_SPRING_DAMPING, 1.0);

/// The spring a panel / sidebar runs on: its shipped spring (tuned by
/// `[motion]`) or the user's own stiffness / damping, taken as is.
pub(crate) fn spring_for(shipped: SpringConfig, o: &mandara_config::surfaces::SpringOverride) -> (SpringConfig, bool) {
    match o.custom() {
        Some((stiffness, damping)) => (SpringConfig::new(stiffness, damping, shipped.mass), true),
        None => (shipped, false),
    }
}

/// Steps `spring` under the global profile, or — for a custom spring —
/// under a neutral feel that keeps only reduced motion (and the global
/// speed, applied inside the step).
pub(crate) fn step_spring<T: mandara_core::spring::Springable>(spring: &mut SpringValue<T>, custom: bool, dt: f32) -> bool {
    if custom {
        let global = mandara_core::spring::motion_profile();
        spring.step_in(dt, &mandara_core::spring::MotionProfile { feel: SpringConfig::default(), reduce: global.reduce })
    } else {
        spring.step(dt)
    }
}

impl ShellView {
    pub fn new(monitor: String, surface: (f32, f32), window: &mut Window, cx: &mut Context<Self>) -> Self {
        let config = config(cx);
        let shell = shell(cx);
        cx.observe(&config, |this: &mut Self, _, cx| {
            this.placement = None; // re-check plugin placement
            cx.notify();
        })
        .detach();
        cx.observe(&mandara_core::theme_entity(cx), |_, _, cx| cx.notify()).detach();
        cx.observe_in(&shell, window, |this: &mut Self, shell, window, cx| {
            if shell.read(cx).panel.is_some() || shell.read(cx).sidebar_open {
                this.focus.focus(window, cx);
            }
            cx.notify();
        })
        .detach();
        // Clicking any other window takes keyboard focus away from the shell
        // surface: collapse whatever is open. Clicking *inside* the surface
        // makes MangoWM send a keyboard leave immediately followed by an enter
        // (before the press itself is delivered), so a deactivation only
        // counts once it has lasted a moment.
        cx.observe_window_activation(window, |this: &mut Self, window, cx| {
            tracing::debug!(target: "mandara::dismiss", "surface active = {}", window.is_window_active());
            if window.is_window_active() {
                this.deactivate_task = None;
                return;
            }
            this.deactivate_task = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(DEACTIVATE_SETTLE).await;
                let _ = this.update_in(cx, |this, window, cx| {
                    this.deactivate_task = None;
                    if !window.is_window_active() {
                        this.dismiss("focus moved to another window", cx);
                    }
                });
            }));
        })
        .detach();
        // Backup for panels opened by keybind/IPC (never focused): when a
        // *window* gains focus, dismiss. Clicking a Mandara surface makes the
        // compositor report "no client focused" — that transition must never
        // dismiss, or clicks inside panels would close them.
        let hub = crate::hub::hub(cx);
        // The last *window* that had focus. "No client focused" (our own
        // surface took focus) is never stored, so focus returning to the
        // same window after a click on Mandara is not a change.
        let mut last_window = hub.read(cx).focused.as_ref().map(|c| c.id);
        // (Notification popups live on their own surface: `crate::toasts`.)
        cx.observe(&hub, move |this: &mut Self, hub, cx| {
            let Some(focus) = hub.read(cx).focused.as_ref().map(|c| c.id) else { return };
            tracing::debug!(target: "mandara::dismiss", "focused client {focus} (last window {last_window:?})");
            if Some(focus) == last_window {
                return;
            }
            last_window = Some(focus);
            this.dismiss("another window focused", cx);
        })
        .detach();
        let zero = Bounds::default();
        let spring = SpringConfig::default();
        let monitor_for_autohide = monitor.clone();
        let monitor_for_corners = monitor.clone();
        Self {
            config,
            shell,
            monitor,
            mask: InputMaskRegistry::new(),
            focus: cx.focus_handle(),
            clock: FrameClock::new(),
            surface,
            zones: [Vec::new(), Vec::new(), Vec::new()],
            sidebar_notifications: None,
            tabs: crate::sidebar_tabs::SidebarTabs::new(),
            menu: None,
            detached: None,
            placement: None,
            vertical_slots: false,
            metrics: Metrics::default_for(BarLayout::default(), BarPosition::default()),
            panel_custom: false,
            sidebar_custom: false,
            groups: [SpringValue::new(zero, spring), SpringValue::new(zero, spring), SpringValue::new(zero, spring)],
            pill_alpha: SpringValue::new(0.0, spring),
            pill_radius: SpringValue::new(px(INITIAL_PILL_RADIUS_PX), spring),
            backdrop: SpringValue::new(zero, spring),
            applied: None,
            panel: SpringValue::new(zero, PANEL_SPRING),
            panel_slot: None,
            panel_visible: false,
            panel_entered: false,
            panel_committed: false,
            panel_shadow: Default::default(),
            sidebar: SpringValue::new(0.0, SIDEBAR_SPRING),
            leave_task: None,
            edge_task: None,
            last_press: None,
            deactivate_task: None,
            menu_anchor: 0.0,
            sidebar_cx: PanelCx::new(),
            autohide: crate::bar::autohide::AutoHide::new(monitor_for_autohide, |v: &mut ShellView| &mut v.autohide, cx),
            bar_shift: 0.0,
            corners: crate::corners::HotCorners::new(monitor_for_corners, |v: &mut ShellView| &mut v.corners, cx),
        }
    }

    /// Instantiates plugins for the configured placement (only when it
    /// changed, so live views keep their state across unrelated edits).
    fn ensure_slots(&mut self, cfg: &ShellConfig, window: &mut Window, cx: &mut Context<Self>) {
        let generation = cx.global::<PluginRegistry>().generation;
        // Fast path (every frame): compare in place, without cloning the
        // placement lists.
        let vertical = cfg.bar.position.is_vertical();
        if let Some((zones, cards, disabled, notif, enabled, g)) = &self.placement
            && *g == generation
            && self.vertical_slots == vertical
            && *notif == cfg.sidebar.notifications_tab
            && zones[0] == cfg.bar.left
            && zones[1] == cfg.bar.center
            && zones[2] == cfg.bar.right
            && self.tabs.matches(&cfg.sidebar.tabs, cards)
            && *disabled == cfg.plugins.disabled
            && *enabled == cfg.plugins.enabled
        {
            return;
        }
        let placement: Placement = (
            [cfg.bar.left.clone(), cfg.bar.center.clone(), cfg.bar.right.clone()],
            crate::sidebar_tabs::SidebarTabs::key(&cfg.sidebar.tabs),
            cfg.plugins.disabled.clone(),
            cfg.sidebar.notifications_tab,
            cfg.plugins.enabled.clone(),
            generation,
        );
        let registry_ids: Vec<(String, bool)> = {
            let registry = cx.global::<PluginRegistry>();
            registry.manifests().map(|m| (m.id.clone(), registry.is_enabled(&m.id, &cfg.plugins))).collect()
        };
        let enabled = |id: &str| registry_ids.iter().any(|(i, e)| i == id && *e);
        let base_cx = PanelCx::for_monitor(&self.monitor);
        // Live instances are reused when a plugin stays placed.
        let mut pool: Vec<Slot> = self.zones.iter_mut().flat_map(std::mem::take).collect();
        for (zi, ids) in placement.0.iter().enumerate() {
            let mut next = Vec::new();
            for id in ids.iter().filter(|id| enabled(id)) {
                let Some((build, generation)) = cx.global::<PluginRegistry>().get(id).map(|f| (f.build.clone(), f.generation)) else { continue };
                // Reuse the live instance unless the plugin was hot-reloaded.
                if let Some(pos) = pool.iter().position(|s| &s.id == id && s.generation == generation && s.vertical == vertical) {
                    next.push(pool.remove(pos));
                    continue;
                }
                let panel_cx = base_cx.fork();
                // A plugin whose builder panics is skipped, not fatal.
                let Some(instance) = mandara_plugins::guard(&format!("plugin `{id}` startup"), || build(window, cx, panel_cx.clone())) else { continue };
                let measured = Rc::new(Cell::new(0.0));
                let measured_cross = Rc::new(Cell::new(0.0));
                let shell_id = cx.entity_id();
                let host = instance.bar.map(|view| cx.new(|_| ChipHost { view, measured: measured.clone(), measured_cross: measured_cross.clone(), vertical, shell: shell_id }));
                next.push(Slot {
                    id: id.clone(),
                    generation,
                    host,
                    panel: instance.panel,
                    panel_cx,
                    on_click: instance.on_click,
                    on_secondary_click: instance.on_secondary_click,
                    measured,
                    measured_cross,
                    vertical,
                    hover: SpringValue::new(0.0, SpringConfig::SNAPPY),
                    hovered: false,
                });
            }
            self.zones[zi] = next;
        }
        let notif_id = crate::plugins::notifications::ID;
        let notif_tab = placement.3 && enabled(notif_id);
        self.sidebar_notifications = notif_tab
            .then(|| {
                let build = cx.global::<PluginRegistry>().get(notif_id).map(|f| f.build.clone())?;
                mandara_plugins::guard("notifications tab startup", || build(window, cx, base_cx.fork()))?.sidebar
            })
            .flatten();
        self.tabs.rebuild(&cfg.sidebar.tabs, &enabled, notif_tab.then_some(notif_id), &base_cx, window, cx);
        if self.menu.is_none() {
            let build = cx.global::<PluginRegistry>().get(crate::plugins::main_menu::ID).map(|f| f.build.clone());
            if let Some(build) = build {
                let menu_cx = base_cx.fork();
                self.menu = mandara_plugins::guard("main menu startup", || build(window, cx, menu_cx.clone())).and_then(|i| i.panel).map(|p| (p, menu_cx));
            }
        }
        // A detached panel's plugin may have been reloaded or removed.
        if self.panel_slot != Some(DETACHED_SLOT) {
            self.detached = None;
        }
        self.placement = Some(placement);
        self.vertical_slots = vertical;
        self.applied = None;
    }

    fn zone_widths(&self) -> [f32; 3] {
        let m = self.metrics;
        let w = |zone: &[Slot]| layout::zone_width(&zone.iter().map(|s| self.slot_length(s)).collect::<smallvec::SmallVec<[f32; 16]>>(), &m);
        [w(&self.zones[0]), w(&self.zones[1]), w(&self.zones[2])]
    }

    /// A slot's content length along the bar; 0 (hidden) for content wider
    /// than a side bar (plugins without a compact column form).
    fn slot_length(&self, slot: &Slot) -> f32 {
        let m = &self.metrics;
        if m.vertical && slot.measured_cross.get() > m.height - m.chip_inset() * 2.0 + MEASURE_EPSILON_PX {
            return 0.0;
        }
        slot.measured.get()
    }

    fn update_targets(&mut self, cfg: &ShellConfig) {
        let widths = self.zone_widths();
        if widths.iter().all(|w| *w <= 0.0) || self.surface.0 < 1.0 {
            return;
        }
        let pos = cfg.bar.position;
        let key = layout_key(widths, self.metrics, self.surface);
        if self.applied.as_ref() == Some(&key) {
            return;
        }
        // Switching edges jumps rather than flying the pills across the screen.
        let first = self.applied.as_ref().is_none_or(|k| k.metrics.position != pos);
        // Geometry is computed for a top bar, then mirrored for a bottom one
        // (transposed for a side bar).
        let (along, _) = layout::bar_axes(&self.metrics, self.surface);
        let geo: BarGeometry = layout::compute(&self.metrics, along, widths);
        let surface = self.surface;
        for (s, t) in self.groups.iter_mut().zip(geo.pills) {
            s.set_target(layout::orient(pos, t, surface));
        }
        self.backdrop.set_target(layout::orient(pos, geo.backdrop, surface));
        self.pill_radius.set_target(geo.pill_radius);
        self.pill_alpha.set_target(if geo.separate_pills { 1.0 } else { 0.0 });
        if first {
            for s in &mut self.groups {
                s.snap_to_target();
            }
            self.backdrop.snap_to_target();
            self.pill_radius.snap_to_target();
            self.pill_alpha.snap_to_target();
        }
        self.applied = Some(key);
    }

    /// Chip rects of every visible slot in a zone, from the zone's animated
    /// bounds.
    fn slot_rects(&self, zone: usize) -> Vec<Option<Bounds<Pixels>>> {
        let m = &self.metrics;
        let g = self.groups[zone].value();
        if m.vertical {
            // Solved in bar space (as a top edge frame), then transposed.
            let (along, _) = layout::bar_axes(m, self.surface);
            let gb = layout::unorient(m.position, g, self.surface);
            let inset = m.chip_inset();
            let (y, h) = (f32::from(m.frame(along).origin.y) + inset, m.height - inset * 2.0);
            let mut x = f32::from(gb.origin.x);
            return self.zones[zone]
                .iter()
                .map(|slot| {
                    let w = self.slot_length(slot);
                    if w < 1.0 {
                        return None;
                    }
                    let chip_w = w + m.chip_padding * 2.0;
                    let r = Bounds { origin: point(px(x), px(y)), size: size(px(chip_w), px(h)) };
                    x += chip_w + m.chip_gap;
                    Some(layout::orient(m.position, r, self.surface))
                })
                .collect();
        }
        let floating = !m.attached();
        let inset = if floating { PILL_INSET } else { 0.0 };
        let (y, h) = if floating {
            (f32::from(g.origin.y) + PILL_INSET + self.bar_shift, f32::from(g.size.height) - PILL_INSET * 2.0)
        } else {
            // The frame's bar-side edge: the band's start (top bar) or its
            // end less the frame (bottom bar).
            let (band_y, band_h) = layout::reserved_band(m, self.surface.1);
            let frame_y = if m.position == BarPosition::Bottom { band_y + band_h - m.height - if m.floating_frame { m.edge_gap } else { 0.0 } } else { f32::from(m.frame(self.surface.0).origin.y) };
            (frame_y + m.chip_inset() + self.bar_shift, m.height - m.chip_inset() * 2.0)
        };
        let mut x = f32::from(g.origin.x) + inset;
        self.zones[zone]
            .iter()
            .map(|slot| {
                let w = slot.measured.get();
                if w < 1.0 {
                    return None;
                }
                let chip_w = w + m.chip_padding * 2.0;
                let r = Bounds { origin: point(px(x), px(y)), size: size(px(chip_w), px(h)) };
                x += chip_w + m.chip_gap;
                Some(r)
            })
            .collect()
    }

    /// Instantiates the plugin of a requested panel that isn't in the bar.
    fn ensure_detached(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.shell.read(cx).panel.clone() else { return };
        if id == crate::plugins::main_menu::ID || self.find_slot(&id).is_some() || self.detached.as_ref().is_some_and(|(d, _, _)| *d == id) {
            return;
        }
        // A different detached panel is requested while one is shown (or
        // still animating closed): replace it in place. The drawer then
        // morphs from the old panel's rect to the new one, like switching
        // between bar panels. (Waiting for the old one to finish closing
        // left nothing to re-render once it settled, so the new panel never
        // appeared.)
        let enabled = {
            let cfg = crate::state::config(cx).read(cx).snapshot();
            cx.global::<PluginRegistry>().is_enabled(&id, &cfg.plugins)
        };
        let Some(build) = cx.global::<PluginRegistry>().get(&id).map(|f| f.build.clone()).filter(|_| enabled) else { return };
        let panel_cx = PanelCx::for_monitor(&self.monitor);
        let pcx = panel_cx.clone();
        if let Some(panel) = mandara_plugins::guard(&format!("plugin `{id}` panel"), || build(window, cx, pcx)).and_then(|i| i.panel) {
            self.detached = Some((id, panel, panel_cx));
        }
    }

    fn find_slot(&self, id: &str) -> Option<(usize, usize)> {
        self.zones.iter().enumerate().find_map(|(z, slots)| slots.iter().position(|s| s.id == id).map(|i| (z, i)))
    }

    fn panel_spec(&self, (z, i): (usize, usize)) -> Option<&PanelSpec> {
        if z == MENU_SLOT.0 {
            self.menu.as_ref().map(|(p, _)| p)
        } else if z == DETACHED_SLOT.0 {
            self.detached.as_ref().map(|(_, p, _)| p)
        } else {
            self.zones.get(z)?.get(i)?.panel.as_ref()
        }
    }

    /// Collapsed and expanded rects of a panel, in surface coordinates. The
    /// geometry is solved top-oriented and mirrored for a bottom bar, so the
    /// panel springs upward from it.
    fn panel_rects(&self, cfg: &ShellConfig, (z, i): (usize, usize)) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
        let m = &self.metrics;
        let (pos, surface) = (m.position, self.surface);
        let (along, away) = layout::bar_axes(m, surface);
        // The originating chip, top-oriented (bar space for a side bar).
        let chip = if z == MENU_SLOT.0 || z == DETACHED_SLOT.0 {
            let x = if z == MENU_SLOT.0 { self.menu_anchor } else { along / 2.0 };
            Bounds {
                origin: point(px(x - ANCHOR_CHIP_W_PX / 2.0), px(f32::from(m.frame(along).origin.y) + m.chip_inset())),
                size: size(px(ANCHOR_CHIP_W_PX), px(m.height - m.chip_inset() * 2.0)),
            }
        } else {
            layout::unorient(pos, self.slot_rects(z).get(i).copied().flatten()?, surface)
        };
        let target = self.panel_spec((z, i))?.size.get();
        // Beside a side bar the screen frame takes the far edge.
        let free_h = away - layout::exclusive_zone(m) - m.screen_frame;
        let expanded = layout::dashboard_rect(m, chip, target, along, free_h);
        let collapsed = layout::start_rect(m, cfg.panels.animation, chip, expanded);
        Some((layout::orient(pos, collapsed, surface), layout::orient(pos, expanded, surface)))
    }

    fn update_panel(&mut self, cfg: &ShellConfig, cx: &App) {
        let state = self.shell.read(cx);
        let wanted = match state.panel.as_deref() {
            Some(id) if id == crate::plugins::main_menu::ID && self.menu.is_some() => {
                // (Along the bar: x, or y beside a side bar.)
                self.menu_anchor = state.panel_anchor.unwrap_or(layout::bar_axes(&self.metrics, self.surface).0 / 2.0);
                Some(MENU_SLOT)
            }
            Some(id) => self.find_slot(id).filter(|(z, i)| self.zones[*z][*i].panel.is_some()).or_else(|| {
                self.detached.as_ref().filter(|(d, _, _)| d == id).map(|_| DETACHED_SLOT)
            }),
            None => None,
        };
        match wanted {
            Some(slot) => {
                let Some((collapsed, expanded)) = self.panel_rects(cfg, slot) else { return };
                if !self.panel_visible {
                    self.panel.snap_to(collapsed);
                    self.panel_visible = true;
                    self.panel_entered = false;
                    self.panel_committed = false;
                    self.panel_shadow.clear();
                    // A close scheduled for the previous panel must never
                    // hit the one just opened.
                    self.leave_task = None;
                } else if self.panel_slot != Some(slot) {
                    self.panel_entered = false;
                    self.panel_committed = false;
                    self.panel_shadow.clear();
                    self.leave_task = None;
                } else if self.panel.target() != expanded && !self.panel.is_animating() {
                    // The resting panel's content changed size: remember
                    // where it rested, so it shrinking away from a resting
                    // pointer never reads as the user leaving it.
                    self.panel_shadow.resized(rect_of(self.panel.target()), rect_of(expanded));
                }
                self.panel_slot = Some(slot);
                self.panel.set_target(expanded);
            }
            None if self.panel_visible => {
                // Closing: forget hover/commit state and any pending close,
                // so reopening (even the same panel) starts clean.
                self.panel_entered = false;
                self.panel_committed = false;
                self.panel_shadow.clear();
                self.leave_task = None;
                if let Some(slot) = self.panel_slot
                    && let Some((collapsed, _)) = self.panel_rects(cfg, slot)
                {
                    self.panel.set_target(collapsed);
                }
            }
            None => {}
        }
    }

    fn panel_progress(&self, cfg: &ShellConfig) -> f32 {
        match (self.panel_visible, self.panel_slot) {
            (true, Some(slot)) => self
                .panel_rects(cfg, slot)
                .map(|(c, e)| layout::morph_progress_on(&self.metrics, self.panel.value(), c, e))
                .unwrap_or(0.0),
            _ => 0.0,
        }
    }

    /// Advances every spring; returns whether another frame is needed.
    fn step(&mut self, dt: f32, panel_open: bool) -> bool {
        let mut animating = false;
        for s in &mut self.groups {
            animating |= s.step(dt);
        }
        for zone in &mut self.zones {
            for slot in zone {
                slot.hover.set_target(if slot.hovered { 1.0 } else { 0.0 });
                animating |= slot.hover.step(dt);
            }
        }
        animating |= self.backdrop.step(dt);
        animating |= self.pill_radius.step(dt);
        animating |= self.pill_alpha.step(dt);
        animating |= step_spring(&mut self.sidebar, self.sidebar_custom, dt);
        animating |= self.tabs.step(dt);
        animating |= self.autohide.step(dt);
        if self.panel_visible {
            let moving = step_spring(&mut self.panel, self.panel_custom, dt);
            animating |= moving;
            if !moving && !panel_open {
                self.panel_visible = false;
                self.panel_slot = None;
            }
        }
        animating
    }

    /// Closes the panel / sidebar shortly after the pointer leaves, unless it
    /// comes back.
    fn schedule_leave(&mut self, what: Leave, cx: &mut Context<Self>) {
        // How long the pointer may be away before a hover-opened panel or
        // sidebar closes (`panels.leave_delay_ms`).
        let delay = crate::state::config(cx).read(cx).snapshot().panels.leave_delay();
        self.leave_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |this, cx| {
                tracing::info!(target: "mandara::dismiss", "pointer left {}", match what { Leave::Panel => "panel", Leave::Sidebar => "sidebar" });
                this.shell.update(cx, |s, cx| match what {
                    Leave::Panel => s.set_panel(None, cx),
                    Leave::Sidebar => s.set_sidebar_hover(false, cx),
                });
            });
        }));
    }

    fn cancel_leave(&mut self) {
        self.leave_task = None;
    }

    /// Fires an edge action after the configured dwell, unless the pointer
    /// leaves the trigger first.
    fn arm_edge(&mut self, which: EdgeAction, dwell: Duration, cx: &mut Context<Self>) {
        self.edge_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(dwell).await;
            let _ = this.update(cx, |this, cx| {
                // The dock may have been switched on during the dwell.
                let launcher_hover = config(cx).read(cx).get().launcher_hover_enabled();
                this.shell.update(cx, |s, cx| match which {
                    EdgeAction::Launcher if !launcher_hover => {}
                    EdgeAction::Launcher => s.set_launcher(true, cx),
                    EdgeAction::Session => s.set_session(true, cx),
                });
            });
        }));
    }

    /// A press on the click-outside catcher: closes everything, unless it
    /// landed where the open panel rested a moment ago and the pointer has
    /// not moved away since (the panel shrank out from under it).
    fn press_outside(&mut self, e: &gpui::MouseDownEvent, cx: &mut Context<Self>) {
        if self.panel_shadow.absorbs_press(f32::from(e.position.x), f32::from(e.position.y)) {
            tracing::info!(target: "mandara::dismiss", "press ignored: the panel resized under the pointer");
            // Aimed at the panel: it now stays like after a press on it.
            self.panel_committed = true;
            self.cancel_leave();
            return;
        }
        self.close_all("click outside", cx);
    }

    /// Closes the panel and the unpinned sidebar unconditionally (explicit
    /// click outside).
    fn close_all(&mut self, reason: &str, cx: &mut Context<Self>) {
        tracing::info!(target: "mandara::dismiss", "close all: {reason}");
        self.shell.update(cx, |s, cx| {
            s.set_panel(None, cx);
            s.set_sidebar(false, cx);
        });
    }

    /// Collapses the open panel and the (unpinned) sidebar — ignored while
    /// the pointer is over Mandara's own UI.
    fn dismiss(&mut self, reason: &str, cx: &mut Context<Self>) {
        // A window that takes focus right after a press on Mandara (pressing
        // play raises the player, a notification action opens its app) is a
        // consequence of using the panel, not a click elsewhere.
        if !crate::dismiss::focus_loss_dismisses(self.last_press, Instant::now()) {
            tracing::info!(target: "mandara::dismiss", "ignored ({reason}) within press grace");
            return;
        }
        let state = self.shell.read(cx);
        if state.panel.is_some() || state.sidebar_visible() {
            tracing::info!(target: "mandara::dismiss", "dismiss: {reason}");
            self.shell.update(cx, |s, cx| {
                s.set_panel(None, cx);
                s.set_sidebar(false, cx);
            });
        }
    }
}

#[derive(Copy, Clone)]
enum EdgeAction {
    Launcher,
    Session,
}

#[derive(Copy, Clone)]
enum Leave {
    Panel,
    Sidebar,
}

/// Rounds the auto-hide shift to device pixels (no blurry bar edges).
fn px_snap_shift(shift: f32, scale: f32) -> f32 {
    mandara_core::snap(shift, scale)
}

/// A bounds as a [`crate::dismiss::Rect`] (logical pixels).
fn rect_of(b: Bounds<Pixels>) -> crate::dismiss::Rect {
    crate::dismiss::Rect::new(f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height))
}

fn place(b: Bounds<Pixels>) -> gpui::Div {
    div().absolute().left(b.origin.x).top(b.origin.y).w(b.size.width).h(b.size.height)
}

impl Render for ShellView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Frame timing for Studio → Performance (`diagnostics frames`).
        let _frame = crate::plugins::diagnostics::frame(cx.entity_id().as_u64());
        let now = Instant::now();
        let cfg = self.config.read(cx).snapshot();
        let palette = mandara_core::frame_palette(window, cx);
        let f = fonts(cx);
        let viewport = window.viewport_size();
        if f32::from(viewport.width) > 1.0 {
            let s = (f32::from(viewport.width), f32::from(viewport.height));
            if s != self.surface {
                tracing::info!("shell surface {}×{} at render scale {}", s.0, s.1, window.scale_factor());
                self.surface = s;
                self.applied = None;
            }
        }
        // Resolved once per frame: geometry, radii (global roundness) and
        // the panel / sidebar springs follow Studio edits immediately.
        self.metrics = Metrics::of(&cfg, crate::style::radius_scale());
        let m = self.metrics;
        let (panel_spring, panel_custom) = spring_for(PANEL_SPRING, &cfg.panels.spring);
        self.panel.set_config(panel_spring);
        self.panel_custom = panel_custom;
        let (sidebar_spring, sidebar_custom) = spring_for(SIDEBAR_SPRING, &cfg.sidebar.look.spring);
        self.sidebar.set_config(sidebar_spring);
        self.sidebar_custom = sidebar_custom;
        self.ensure_slots(&cfg, window, cx);
        self.ensure_detached(window, cx);
        self.update_targets(&cfg);
        self.update_panel(&cfg, cx);
        // An attached launcher can grow out of its bar button: say where
        // that rests on this output (only while the launcher is open).
        if self.shell.read(cx).launcher_open {
            // (A side bar's button is not on the launcher's edge.)
            let chip = self.find_slot(crate::plugins::session::LAUNCHER_ID).and_then(|(z, i)| self.slot_rects(z).get(i).copied().flatten()).filter(|_| !m.vertical);
            crate::launcher_place::publish_button(&self.monitor, chip.map(|b| (f32::from(b.origin.x), f32::from(b.size.width))), cx);
        }
        let shell_state = self.shell.read(cx);
        let panel_open = shell_state.panel.is_some();
        let sidebar_target = if cfg.sidebar.enabled && (cfg.sidebar.pinned() || shell_state.sidebar_visible()) { 1.0 } else { 0.0 };
        self.sidebar.set_target(sidebar_target);
        self.autohide.update(&cfg, &self.monitor, panel_open || sidebar_target > 0.0);
        let dt = self.clock.tick_vsync(now, window.refresh_interval(cx));
        if self.step(dt, panel_open) {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }

        let attached = m.attached();
        let bar_h = mandara_core::snap(m.height, window.scale_factor());
        let bottom = cfg.bar.position == BarPosition::Bottom;
        let vertical = m.vertical;
        let right_bar = cfg.bar.position == BarPosition::Right;
        // The band the bar reserves; the sidebar fills the rest of the
        // screen's height (below a top bar, above a bottom one; inside the
        // screen frame beside a side bar).
        let (band_y, band_h) = layout::reserved_band(&m, self.surface.1);
        let top = if bottom { 0.0 } else { band_h } + m.screen_frame;
        let free_h = self.surface.1 - band_h - m.screen_frame * 2.0;
        // Auto-hide slides every bar piece off its edge.
        self.bar_shift = px_snap_shift(self.autohide.shift(&cfg, band_h), window.scale_factor());
        let shift = self.bar_shift;
        let shifted = |b: Bounds<Pixels>| Bounds { origin: point(b.origin.x, b.origin.y + px(shift)), size: b.size };
        let click_through = cfg.bar.click_through;
        let fill = frame_color(&palette);
        // The bar's own surfaces (strip, screen flares, pills, island) honour
        // `bar.opacity`; panels and the sidebar keep the full frame color.
        let bar_fill = if cfg.bar.tint > 0.0 { fill.mix(palette.primary, ranges::BAR_TINT.clamp(cfg.bar.tint, 0.0)) } else { fill };
        let bar_fill_h = bar_fill.with_alpha(fill.alpha() * ranges::OPACITY.clamp(cfg.bar.opacity, 1.0)).hsla();
        // Drop-down panels / side panels carry their own opacity.
        let panel_alpha = ranges::OPACITY.clamp(cfg.panels.opacity, 1.0);
        let side_alpha = ranges::OPACITY.clamp(cfg.sidebar.look.opacity, 1.0);
        let panel_radius = crate::style::panel_radius(&cfg.bar);
        let side_look = &cfg.sidebar.look;
        let side_radius = crate::style::side_panel_radius(side_look);
        let side_inset = side_look.inset_with(&cfg.bar);
        // Glass rim and soft shadow of the bar's floating shapes.
        let rim_k = ranges::INTENSITY.clamp(cfg.bar.rim_intensity, 1.0) * if crate::style::borders() { 1.0 } else { 0.0 };
        let bar_rim = mandara_core::glass_rim_width(
            palette.specular_rim_light.with_alpha(RIM_LIGHT_ALPHA * rim_k),
            palette.specular_rim_shadow.with_alpha(RIM_SHADOW_ALPHA * rim_k),
            ranges::BAR_RIM_WIDTH.clamp(cfg.bar.rim_width, mandara_config::surfaces::defaults::BAR_RIM_WIDTH_PX),
        );
        let bar_shadow = if cfg.bar.shadow { crate::plugins::panel_fx::elevation_scaled(&palette, 1.0, cfg.bar.shadow_strength) } else { Vec::new() };
        let bar_depth = |rim: &[gpui::BoxShadow]| -> Vec<gpui::BoxShadow> { bar_shadow.iter().chain(rim).cloned().collect() };
        // Frame pieces overlap the bar by a pixel to hide seams; translucent
        // fills would double up there, so they butt edge to edge instead.
        let seam = if cfg.bar.opacity >= OPAQUE_ALPHA && !m.floating_frame { 1.0 } else { 0.0 };
        // Every frame edge sits on the device pixel grid (no AA seams).
        let scale = window.scale_factor();
        let px_snap = |v: f32| mandara_core::snap(v, scale);
        let pill_alpha = self.pill_alpha.value().clamp(0.0, 1.0);
        let pill_radius = self.pill_radius.value();
        let progress = self.panel_progress(&cfg);
        for (z, zone) in self.zones.iter().enumerate() {
            for (i, slot) in zone.iter().enumerate() {
                slot.panel_cx.set_progress(if self.panel_slot == Some((z, i)) { progress } else { 0.0 });
            }
        }
        if let Some((_, menu_cx)) = &self.menu {
            menu_cx.set_progress(if self.panel_slot == Some(MENU_SLOT) { progress } else { 0.0 });
        }
        if let Some((_, _, d_cx)) = &self.detached {
            d_cx.set_progress(if self.panel_slot == Some(DETACHED_SLOT) { progress } else { 0.0 });
        }
        let sidebar_p = self.sidebar.value().clamp(0.0, SIDEBAR_OVERSHOOT_MAX);
        let sidebar_w = ranges::SIDEBAR_WIDTH.clamp(cfg.sidebar.width, mandara_config::defaults::SIDEBAR_WIDTH_PX);
        let sidebar_right = cfg.sidebar_on_right();
        // A floating sidebar travels its margin further so it hides fully.
        let travel = sidebar_w + side_inset;
        // A side bar on the sidebar's edge: the sidebar slides out beside it.
        let bar_on_sidebar_edge = vertical && right_bar == sidebar_right;
        let beside_bar = if bar_on_sidebar_edge { layout::exclusive_zone(&m) } else { 0.0 };
        let sidebar_x = mandara_core::snap(
            if sidebar_right { self.surface.0 - beside_bar - travel * sidebar_p.min(1.0) } else { beside_bar + side_inset - travel * (1.0 - sidebar_p.min(1.0)) },
            window.scale_factor(),
        );
        // Attached to the edge frame (flares, shared fill) unless it floats
        // or the frame itself floats (or the bar is a side bar).
        let side_attached = attached && !side_look.floats_with(&cfg.bar) && !m.floating_frame && !vertical;

        let mut root = div()
            .id("mandara-shell")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .font_family(f.ui.clone())
            // Right-to-left languages align text blocks to the right.
            .text_start()
            .text_size(px(BASE_TEXT_PX * cfg.typography.scale))
            .font_weight(crate::style::body_weight())
            .text_color(palette.on_surface.hsla())
            // Capture phase: sees every press on Mandara's UI, even over
            // occluding panels.
            .capture_any_mouse_down(cx.listener(|this, _: &gpui::MouseDownEvent, _, _| {
                tracing::debug!(target: "mandara::dismiss", "press");
                this.last_press = Some(Instant::now());
            }))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                let sidebar_up = this.shell.read(cx).sidebar_visible() || crate::state::config(cx).read(cx).get().sidebar.pinned();
                if sidebar_up && this.shell.read(cx).panel.is_none() {
                    let cfg = crate::state::config(cx).read(cx).snapshot();
                    let notifications = this.sidebar_notifications.is_some();
                    if this.tabs.handle_key(e, &cfg.sidebar, notifications, cx) {
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }
                }
                if e.keystroke.key == "escape" {
                    this.shell.update(cx, |s, cx| {
                        s.set_panel(None, cx);
                        s.set_sidebar(false, cx);
                    });
                }
            }));

        // ── Click-outside catcher ────────────────────────────────────────
        // While a panel, the main menu or an unpinned sidebar is open, an
        // invisible full-surface layer joins the input region (beneath the
        // UI), so the first click anywhere else closes it — the reliable
        // popup-menu behaviour, independent of compositor focus hints.
        //
        // This includes a sidebar revealed by hover: without the catcher the
        // pointer leaves the shell surface straight onto a window, GPUI never
        // sees a hover-out, and a click on the already-focused window gives
        // no focus change either — the sidebar would stay open.
        let catcher_active = panel_open || (cfg.sidebar.enabled && shell_state.sidebar_visible() && !cfg.sidebar.pinned());
        if catcher_active {
            root = root.child(
                div()
                    .id("outside-catcher")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    // `style.backdrop_dim`: the desktop dims as the panel / sidebar opens.
                    .when(cfg.style.backdrop_dim > 0.0, |el| {
                        let open = progress.max(if cfg.sidebar.pinned() { 0.0 } else { sidebar_p.min(1.0) });
                        el.bg(gpui::black().opacity(cfg.style.backdrop_dim.clamp(0.0, 1.0) * open))
                    })
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, e: &gpui::MouseDownEvent, _, cx| this.press_outside(e, cx)))
                    .on_mouse_down(gpui::MouseButton::Right, cx.listener(|this, e: &gpui::MouseDownEvent, _, cx| this.press_outside(e, cx)))
                    .on_mouse_down(gpui::MouseButton::Middle, cx.listener(|this, e: &gpui::MouseDownEvent, _, cx| this.press_outside(e, cx)))
                    // A pointer the panel shrank away from: its leave counts
                    // once it really moves out of where the panel was.
                    .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _, cx| {
                        let (x, y) = (f32::from(e.position.x), f32::from(e.position.y));
                        if this.panel_shadow.pointer_moved(x, y) && !this.panel_committed && this.leave_task.is_none() {
                            this.schedule_leave(Leave::Panel, cx);
                        }
                    }))
                    .child(self.mask.region()),
            );
        }

        // ── Frame: bar strip, screen flares, panel drawer, sidebar ────────
        if attached && m.floating_frame {
            // A floating frame: one rounded slab clear of the screen edges.
            let frame = layout::orient(m.position, m.frame(layout::bar_axes(&m, self.surface).0), self.surface);
            root = root.child(
                place(shifted(frame))
                    .id("bar-backdrop")
                    .rounded(px(m.frame_radius()))
                    .bg(bar_fill_h)
                    .shadow(bar_depth(&bar_rim))
                    .on_mouse_down(gpui::MouseButton::Right, cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                        // The main menu opens at the press, along the bar.
                        let x = f32::from(if vertical { e.position.y } else { e.position.x });
                        this.shell.update(cx, |s, cx| {
                            s.panel_anchor = Some(x);
                            s.toggle_panel(crate::plugins::main_menu::ID, cx);
                        });
                    }))
                    .when(!click_through, |el| el.child(self.mask.region())),
            );
        } else if attached && vertical {
            // A side strip flush with its edge; flares curve into the top
            // and bottom edges (or into the screen frame's opening).
            let strip = layout::reserved_rect(&m, self.surface);
            let (sx, sw_) = (px_snap(f32::from(strip.origin.x)), px_snap(f32::from(strip.size.width)));
            let r = px_snap(m.flare);
            root = root.child(div().absolute().left(px(sx)).top_0().w(px(sw_)).h(px(self.surface.1)).bg(bar_fill_h));
            if let Some((strips, inner)) = layout::screen_frame(&m, self.surface) {
                for b in strips {
                    root = root.child(place(b).bg(bar_fill_h));
                }
                let (ix, iy) = (px_snap(f32::from(inner.origin.x)), px_snap(f32::from(inner.origin.y)));
                let (ix1, iy1) = (px_snap(f32::from(inner.origin.x + inner.size.width)), px_snap(f32::from(inner.origin.y + inner.size.height)));
                root = root
                    .child(concave_flare(ix, iy, r, FlareCenter::BottomRight, bar_fill_h))
                    .child(concave_flare(ix1 - r, iy, r, FlareCenter::BottomLeft, bar_fill_h))
                    .child(concave_flare(ix, iy1 - r, r, FlareCenter::TopRight, bar_fill_h))
                    .child(concave_flare(ix1 - r, iy1 - r, r, FlareCenter::TopLeft, bar_fill_h));
            } else {
                let (fx, top_c, bottom_c) = if right_bar { (sx - r, FlareCenter::BottomLeft, FlareCenter::TopLeft) } else { (sx + sw_, FlareCenter::BottomRight, FlareCenter::TopRight) };
                root = root.child(concave_flare(fx, 0.0, r, top_c, bar_fill_h)).child(concave_flare(fx, self.surface.1 - r, r, bottom_c, bar_fill_h));
            }
            root = root.child(
                place(self.backdrop.value())
                    .id("bar-backdrop")
                    .on_mouse_down(gpui::MouseButton::Right, cx.listener(|this, e: &gpui::MouseDownEvent, _, cx| {
                        let y = f32::from(e.position.y);
                        this.shell.update(cx, |s, cx| {
                            s.panel_anchor = Some(y);
                            s.toggle_panel(crate::plugins::main_menu::ID, cx);
                        });
                    }))
                    .when(!click_through, |el| el.child(self.mask.region())),
            );
        } else if attached {
            let w = self.surface.0;
            let r = px_snap(m.flare);
            // Screen flares curve away from the bar toward the middle of the
            // screen: below a top strip, above a bottom one.
            let strip_y = if bottom { px_snap(band_y + band_h - bar_h) } else { 0.0 } + shift;
            let (flare_y, left_c, right_c) = if bottom {
                (strip_y - r, FlareCenter::TopRight, FlareCenter::TopLeft)
            } else {
                (bar_h + shift, FlareCenter::BottomRight, FlareCenter::BottomLeft)
            };
            root = root
                .child(div().absolute().top(px(strip_y)).left_0().w(px(w)).h(px(bar_h)).bg(bar_fill_h))
                .child(concave_flare(0.0, flare_y, r, left_c, bar_fill_h))
                .child(concave_flare(w - r, flare_y, r, right_c, bar_fill_h))
                .child(
                    place(shifted(self.backdrop.value()))
                        .id("bar-backdrop")
                        .on_mouse_down(gpui::MouseButton::Right, cx.listener(|this, e: &gpui::MouseDownEvent, _, cx| {
                            let x = f32::from(e.position.x);
                            this.shell.update(cx, |s, cx| {
                                s.panel_anchor = Some(x);
                                s.toggle_panel(crate::plugins::main_menu::ID, cx);
                            });
                        }))
                        .when(!click_through, |el| el.child(self.mask.region())),
                );
            // Hairline: a 1 px accent line along the bar's inner edge (over
            // the strip).
            if m.accent_line {
                let line_y = if bottom { strip_y } else { strip_y + bar_h - 1.0 };
                root = root.child(div().absolute().top(px(line_y)).left_0().w(px(w)).h(px(1.0)).bg(palette.primary.with_alpha(ACCENT_LINE_ALPHA).hsla()));
            }
        } else if pill_alpha < OPAQUE_ALPHA {
            root = root.child(
                place(shifted(self.backdrop.value()))
                    .opacity(1.0 - pill_alpha)
                    .rounded(pill_radius)
                    .bg(bar_fill_h)
                    .shadow(bar_depth(&bar_rim))
                    .when(!click_through, |el| el.child(self.mask.region())),
            );
        }
        // The sidebar's box: the free height beside the bar, inset all
        // round while floating.
        let (side_top, side_h) = (top + side_inset, (free_h - side_inset * 2.0).max(0.0));
        if sidebar_p > HIDDEN_EPSILON {
            let h = side_h;
            let side_fill = if side_attached { bar_fill_h } else { fill.with_alpha(fill.alpha() * side_alpha).hsla() };
            let seam = if side_attached { seam } else { 0.0 };
            let floating_side = side_look.floats_with(&cfg.bar);
            root = root
                .child(
                    div()
                        .absolute()
                        .left(px(sidebar_x))
                        // Overlap the bar by a pixel on the side it touches.
                        .top(px(if bottom { side_top } else { side_top - seam }))
                        .w(px(sidebar_w))
                        .h(px(h + seam))
                        .bg(side_fill)
                        .when(floating_side, |el| el.rounded(px(side_radius)).shadow(crate::plugins::panel_fx::elevation_scaled(&palette, sidebar_p.min(1.0), cfg.panels.shadow_strength)))
                        .when(!side_attached && !floating_side && !sidebar_right, |el| el.rounded_r(px(side_radius)))
                        .when(!side_attached && !floating_side && sidebar_right, |el| el.rounded_l(px(side_radius))),
                )
                .when(side_attached, |el| {
                    let r = px_snap(m.flare * sidebar_p.min(1.0));
                    // Where the sidebar meets the bar: under a top bar, or
                    // just above a bottom one.
                    let fy = if bottom { px_snap(top + h) - r } else { px_snap(top) };
                    el.child(if sidebar_right {
                        concave_flare(px_snap(sidebar_x) - r, fy, r, if bottom { FlareCenter::TopLeft } else { FlareCenter::BottomLeft }, bar_fill_h)
                    } else {
                        concave_flare(px_snap(sidebar_x + sidebar_w), fy, r, if bottom { FlareCenter::TopRight } else { FlareCenter::BottomRight }, bar_fill_h)
                    })
                });
        }
        // Fade & slide panels fade in with their travel.
        let panel_fade = if cfg.panels.animation == mandara_config::surfaces::PanelAnimation::Slide { progress } else { 1.0 };
        if self.panel_visible {
            let b = self.panel.value();
            let (x, y, w, h) = (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height));
            let drawer_fill = bar_fill.with_alpha(fill.alpha() * ranges::OPACITY.clamp(cfg.bar.opacity, 1.0) * panel_alpha).hsla();
            let panel_fill = fill.with_alpha(fill.alpha() * panel_alpha).hsla();
            let drawer = if attached && vertical {
                // Grows sideways out of the side bar.
                let edge = if right_bar { Edge::Right } else { Edge::Left };
                let (dx, dw) = if right_bar { (x, w + seam) } else { (x - seam, w + seam) };
                edge_drawer_snapped(edge, dx, y, dw, h, panel_radius, m.flare, drawer_fill, scale)
            } else if attached && bottom {
                // Rises from the bar: attached along its bottom edge.
                edge_drawer_snapped(Edge::Bottom, x, y, w, h + seam, panel_radius, m.flare, drawer_fill, scale)
            } else if attached {
                edge_drawer_snapped(Edge::Top, x, y - seam, w, h + seam, panel_radius, m.flare, drawer_fill, scale)
            } else {
                place(b)
                    .rounded(px(f32::from(pill_radius) + (panel_radius - f32::from(pill_radius)) * progress))
                    .bg(panel_fill)
                    // One even hairline edge + soft elevation (the old
                    // top-highlight / bottom-shade rim faded along the
                    // corners and looked patchy over busy wallpapers).
                    .border_1()
                    .border_color(crate::plugins::panel_fx::surface_edge(&palette, cfg.theme.adjustments.rim_intensity).hsla())
                    .shadow(crate::plugins::panel_fx::elevation_scaled(&palette, progress, cfg.panels.shadow_strength))
                    .into_any_element()
            };
            root = root.child(if panel_fade < 1.0 { div().absolute().size_full().opacity(panel_fade).child(drawer).into_any_element() } else { drawer });
        }

        // ── Floating pill glass per zone ──────────────────────────────────
        if pill_alpha > PILL_VISIBLE_EPSILON {
            for g in &self.groups {
                let b = g.value();
                if f32::from(b.size.width) < 1.0 {
                    continue;
                }
                root = root.child(
                    place(shifted(b))
                        .opacity(pill_alpha)
                        .rounded(pill_radius)
                        .bg(bar_fill_h)
                        .shadow(bar_depth(&bar_rim))
                        .when(!click_through, |el| el.child(self.mask.region())),
                );
            }
        }

        // ── Slot chips + content ──────────────────────────────────────────
        let active_slot = if panel_open { self.panel_slot } else { None };
        let bar_text_px = BASE_TEXT_PX * cfg.typography.scale * cfg.bar.text_scale();
        for z in 0..self.zones.len() {
            let rects = self.slot_rects(z);
            // Edge-frame chips take the bar radius; chips inside a pill or
            // the island stay concentric with it.
            let chip_radius = rects
                .iter()
                .flatten()
                .next()
                .map(|r| {
                    let half = f32::from(r.size.height.min(r.size.width)) / 2.0;
                    if attached { m.radius.min(half) } else { crate::style::concentric(f32::from(pill_radius), PILL_INSET).min(half) }
                })
                .unwrap_or(FALLBACK_CHIP_RADIUS_PX);
            // Separator marks centred in the gaps between visible chips.
            if cfg.bar.separator != BarSeparator::None {
                let mut prev: Option<Bounds<Pixels>> = None;
                for r in rects.iter().flatten() {
                    if let Some(p) = prev {
                        // Centred in the gap along the bar (x, or y beside a side bar).
                        let (gap_mid, mid_y, w, h) = if vertical {
                            let mid_x = f32::from(r.origin.x) + f32::from(r.size.width) / 2.0;
                            let gap_y = (f32::from(p.origin.y + p.size.height) + f32::from(r.origin.y)) / 2.0;
                            let (w, h) = match cfg.bar.separator {
                                BarSeparator::Dot => (SEPARATOR_DOT_PX, SEPARATOR_DOT_PX),
                                _ => (f32::from(r.size.width) * SEPARATOR_LINE_FRAC, SEPARATOR_LINE_W_PX),
                            };
                            (mid_x, gap_y, w, h)
                        } else {
                            let gap_mid = (f32::from(p.origin.x + p.size.width) + f32::from(r.origin.x)) / 2.0;
                            let mid_y = f32::from(r.origin.y) + f32::from(r.size.height) / 2.0;
                            let (w, h) = match cfg.bar.separator {
                                BarSeparator::Dot => (SEPARATOR_DOT_PX, SEPARATOR_DOT_PX),
                                _ => (SEPARATOR_LINE_W_PX, f32::from(r.size.height) * SEPARATOR_LINE_FRAC),
                            };
                            (gap_mid, mid_y, w, h)
                        };
                        root = root.child(
                            div()
                                .absolute()
                                .left(px(px_snap(gap_mid - w / 2.0)))
                                .top(px(px_snap(mid_y - h / 2.0)))
                                .w(px(w))
                                .h(px(h))
                                .rounded_full()
                                .bg(palette.on_surface.with_alpha(SEPARATOR_ALPHA).hsla()),
                        );
                    }
                    prev = Some(*r);
                }
            }
            for (i, slot) in self.zones[z].iter().enumerate() {
                let lift = slot.hover.value().clamp(0.0, 1.0);
                let active = active_slot == Some((z, i));
                if let Some(r) = rects[i] {
                    let tint = if active { palette.primary.with_alpha(ACTIVE_CHIP_TINT_ALPHA) } else { palette.on_surface.with_alpha(HOVER_CHIP_TINT_ALPHA * lift) };
                    let has_action = slot.panel.is_some() || slot.on_click.is_some();
                    let id = slot.id.clone();
                    let id2 = id.clone();
                    root = root.child(
                        place(r)
                            .id(("slot", z * SLOT_ID_STRIDE + i))
                            // Presses on a chip are never "outside": the
                            // chip toggles its own panel.
                            .occlude()
                            .rounded(px(chip_radius))
                            // Click-through bars: only the chips take input.
                            .when(click_through, |el| el.child(self.mask.region()))
                            .bg(tint.hsla())
                            .when(has_action, |el| el.cursor_pointer())
                            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                                // GPUI re-tests hover on repaint against the last pointer position, stale once the pointer left the surface.
                                if *hovered && !window.is_window_hovered() {
                                    return;
                                }
                                if let Some(slot) = this.zones[z].get_mut(i) {
                                    slot.hovered = *hovered;
                                }
                                // Back on the panel's own chip: keep it open.
                                if *hovered && this.panel_slot == Some((z, i)) {
                                    this.cancel_leave();
                                }
                                cx.notify();
                            }))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                let Some(slot) = this.zones[z].get(i) else { return };
                                // A plugin's own click handler wins (it may
                                // toggle its panel itself); otherwise the
                                // chip toggles the panel.
                                if let Some(handler) = slot.on_click.clone() {
                                    mandara_plugins::guard(&format!("plugin `{id}` click"), || handler(window, cx));
                                } else if slot.panel.is_some() {
                                    this.shell.update(cx, |s, cx| s.toggle_panel(&id, cx));
                                }
                            }))
                            .on_mouse_down(gpui::MouseButton::Right, cx.listener(move |this, _: &gpui::MouseDownEvent, window, cx| {
                                // A plugin's chip owns its right-click; never
                                // fall through to the bar's main menu.
                                cx.stop_propagation();
                                if let Some(handler) = this.zones[z].get(i).and_then(|s| s.on_secondary_click.clone()) {
                                    mandara_plugins::guard(&format!("plugin `{id2}` right-click"), || handler(window, cx));
                                }
                            })),
                    );
                }
                // Content lays out at its intrinsic width inside a cached
                // host spanning the rest of the surface; the host records
                // that width (see [`ChipHost`]). Its bounds only change when
                // the chip moves, so an idle chip is replayed, not rebuilt.
                let Some(host) = slot.host.clone() else { continue };
                let (x, y, h) = match rects[i] {
                    // A side bar's chip: content centred across, padded along.
                    Some(r) if vertical => (
                        f32::from(r.origin.x) + (f32::from(r.size.width) - slot.measured_cross.get()) / 2.0,
                        f32::from(r.origin.y) + m.chip_padding,
                        f32::from(r.size.height) - m.chip_padding * 2.0,
                    ),
                    Some(r) => (f32::from(r.origin.x) + m.chip_padding, f32::from(r.origin.y), f32::from(r.size.height)),
                    // Hidden/unmeasured: render offscreen-left so it can be measured.
                    None => (OFFSCREEN_X_PX, 0.0, bar_h),
                };
                root = root.child(
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .w(px(self.surface.0))
                        .h(px(h))
                        .text_size(px(bar_text_px))
                        .when(active, |el| el.text_color(palette.primary.lighten(ACTIVE_TEXT_LIGHTEN).hsla()))
                        .child(AnyView::from(host).cached(StyleRefinement::default().size_full())),
                );
            }
        }

        // ── Panel content ─────────────────────────────────────────────────
        if let (true, Some(slot)) = (self.panel_visible, self.panel_slot)
            && let Some(spec) = self.panel_spec(slot)
        {
            let b = self.panel.value();
            let target = spec.size.get();
            // The panel only counts as "entered" once its morph settles: a
            // floating panel starts under the pointer (on the pill) and then
            // slides away from it, which must not read as leaving.
            let settled = progress > PANEL_SETTLED_PROGRESS;
            // Floating layouts leave a gap between the pill and its panel:
            // bridge it so crossing it never counts as leaving.
            if !attached && slot.0 < self.zones.len() {
                if let Some(Some(chip)) = self.slot_rects(slot.0).get(slot.1).copied() {
                    let (cy0, cy1) = (f32::from(chip.origin.y), f32::from(chip.origin.y + chip.size.height));
                    let (py0, py1) = (f32::from(b.origin.y), f32::from(b.origin.y + b.size.height));
                    let (y0, y1) = if py0 >= cy1 { (cy1, py0) } else { (py1, cy0) };
                    if y1 > y0 {
                        let x0 = f32::from(chip.origin.x) - BRIDGE_OVERHANG_PX;
                        let bridge = Bounds { origin: point(px(x0), px(y0)), size: size(chip.size.width + px(BRIDGE_OVERHANG_PX * 2.0), px(y1 - y0)) };
                        root = root.child(
                            place(bridge)
                                .id("panel-bridge")
                                .on_hover(cx.listener(|this, hovered: &bool, window, _| {
                                    // GPUI re-tests hover on repaint against the last pointer position, stale once the pointer left the surface.
                                    if *hovered && !window.is_window_hovered() {
                                        return;
                                    }
                                    if *hovered {
                                        this.cancel_leave();
                                    }
                                }))
                                .child(self.mask.region()),
                        );
                    }
                }
            }
            // Content: the plugin's natural size, widened by the width
            // scale, held inside the (height-capped) panel and its padding.
            let pad = m.panel_pad;
            // Capped by the expanded panel (never by the animating rect, so
            // the content doesn't re-layout every frame of the morph).
            let expanded = self.panel_rects(&cfg, slot).map(|(_, e)| e.size).unwrap_or(b.size);
            let content_w = (target.width * m.panel_width_scale).min((expanded.width - px(pad * 2.0)).max(px(0.0)));
            let content_h = target.height.min((expanded.height - px(pad * 2.0)).max(px(0.0)));
            // Pinned to the bar-side edge so the morph reveals it from the
            // bar: its top (top bar), bottom (bottom bar) or side (side bar).
            let content_top = if vertical { (b.size.height - content_h) / 2.0 } else if bottom { b.size.height - content_h - px(pad) } else { px(pad) };
            let content_left = if !vertical { (b.size.width - content_w) / 2.0 } else if right_bar { b.size.width - content_w - px(pad) } else { px(pad) };
            root = root.child(
                place(b)
                    .id("panel")
                    .occlude()
                    .overflow_hidden()
                    .when(panel_fade < 1.0, |el| el.opacity(panel_fade))
                    .when(attached && vertical && !right_bar, |el| el.rounded_r(px(panel_radius)))
                    .when(attached && vertical && right_bar, |el| el.rounded_l(px(panel_radius)))
                    .when(attached && !vertical && !bottom, |el| el.rounded_b(px(panel_radius)))
                    .when(attached && !vertical && bottom, |el| el.rounded_t(px(panel_radius)))
                    .when(!attached, |el| el.rounded(px(panel_radius)))
                    .track_focus(&self.focus)
                    .capture_any_mouse_down(cx.listener(|this, _: &gpui::MouseDownEvent, _, _| {
                        this.panel_committed = true;
                        this.cancel_leave();
                    }))
                    .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                        // GPUI re-tests hover on repaint against the last pointer position, stale once the pointer left the surface.
                        if *hovered && !window.is_window_hovered() {
                            return;
                        }
                        if *hovered {
                            if settled {
                                this.panel_entered = true;
                            }
                            this.panel_shadow.clear();
                            this.cancel_leave();
                        } else if this.panel_entered && !this.panel_committed {
                            let pointer = window.mouse_position();
                            if this.panel_shadow.leave_counts(f32::from(pointer.x), f32::from(pointer.y)) {
                                this.schedule_leave(Leave::Panel, cx);
                            } else {
                                tracing::debug!(target: "mandara::dismiss", "leave deferred: the panel resized under the pointer");
                            }
                        }
                    }))
                    .on_mouse_move(cx.listener(move |this, _: &gpui::MouseMoveEvent, _, _| {
                        if settled {
                            this.panel_entered = true;
                        }
                        this.panel_shadow.clear();
                    }))
                    .child(
                        div()
                            .absolute()
                            // Content is pinned to the bar-side edge so the
                            // morph reveals it from the bar (mirrored for a
                            // bottom bar: anchored to the panel's bottom).
                            .top(content_top)
                            .left(content_left)
                            .w(content_w)
                            .h(content_h)
                            .opacity(smoothstep(CONTENT_FADE_FROM, CONTENT_FADE_TO, progress))
                            // Once the morph has settled the content is a
                            // cached view: it rebuilds only when the plugin
                            // notifies, not when a bar chip or hover spring
                            // re-renders the shell. While morphing it renders
                            // every frame (its reveal follows the progress).
                            .child(if progress >= 1.0 {
                                spec.view.clone().cached(StyleRefinement::default().size_full()).into_any_element()
                            } else {
                                spec.view.clone().into_any_element()
                            }),
                    )
                    // The plugin sizes itself while it renders, after this
                    // view read its size: when that changed, lay the panel
                    // out again on the very next frame instead of whenever
                    // something else happens to redraw the shell (a hover,
                    // a press), which left a stale tall panel around the
                    // shrunk content until the user touched it.
                    .child({
                        let size_cell = spec.size.clone();
                        canvas(
                            move |_, window, _| {
                                if size_cell.get() != target {
                                    window.request_animation_frame();
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_0()
                    })
                    .child(self.mask.region()),
            );
        }

        // ── Sidebar content + hover trigger ───────────────────────────────
        if sidebar_p <= HIDDEN_EPSILON {
            // The next opening picks its tab (the last one, or the first).
            self.tabs.on_hidden();
        }
        if sidebar_p > HIDDEN_EPSILON {
            let h = side_h;
            let compact = side_look.header == SidebarHeader::Compact;
            let (tab_h, icon_btn) = if compact { (SIDEBAR_COMPACT_BUTTON_PX, SIDEBAR_COMPACT_BUTTON_PX) } else { (SIDEBAR_TAB_H_PX, SIDEBAR_ICON_BUTTON_PX) };
            let card_gap = ranges::CARD_SPACING.clamp(side_look.card_spacing, mandara_config::surfaces::defaults::CARD_SPACING_PX);
            let sidebar_cx = &self.sidebar_cx;
            sidebar_cx.set_progress(sidebar_p.min(1.0));
            let notifications = self.sidebar_notifications.is_some();
            let sel = self.tabs.resolve(&cfg.sidebar, notifications, cx);
            let cards: Vec<AnyElement> = match (&sel, &self.sidebar_notifications) {
                (crate::sidebar_tabs::Sel::Notifications, Some(view)) => vec![view.clone().into_any_element()],
                _ => self.tabs.cards(&sel).iter().enumerate().map(|(i, card)| reveal(sidebar_cx, i, card.clone())).collect(),
            };
            let cards = if cards.is_empty() { vec![crate::sidebar_tabs::empty_tab(&palette)] } else { cards };
            // The switched-in tab slides from its side and fades in.
            let (slide, fade) = self.tabs.transition();
            let cards = div().relative().left(px(slide)).opacity(fade).flex().flex_col().gap(px(card_gap)).children(cards);
            let scroll_handle = self.tabs.scroll(&sel);
            let scroll_bar = self.tabs.scrollbar();
            let unread = if notifications { crate::hub::hub(cx).read(cx).notifications.unread() } else { 0 };
            // The header row: the strip, then the pin and close buttons.
            let strip_w = sidebar_w - 2.0 * SIDEBAR_PAD_PX - 2.0 * (icon_btn + tokens::XS);
            let tabs = self.tabs.strip(&cfg, &sel, notifications, unread, tab_h, strip_w, &palette, |v: &mut ShellView| &mut v.tabs, window, cx);
            let menu_top = match side_look.tab_position {
                TabPosition::Top => SIDEBAR_PAD_PX + tab_h + SIDEBAR_TAB_STRIP_PAD_PX * 2.0,
                TabPosition::Bottom => SIDEBAR_PAD_PX,
            };
            let tab_menu = self.tabs.menu_overlay(&cfg, menu_top, &palette, |v: &mut ShellView| &mut v.tabs, cx);
            root = root.child(
                div()
                    .id("sidebar")
                    .absolute()
                    .left(px(sidebar_x))
                    .top(px(side_top))
                    .w(px(sidebar_w))
                    .h(px(h))
                    .occlude()
                    // Interacting with a hover-revealed sidebar commits to it:
                    // from then on only a click outside or the close button
                    // hides it, never the pointer drifting off or content
                    // shrinking under it (e.g. clearing notifications).
                    .capture_any_mouse_down(cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                        this.cancel_leave();
                        if !this.shell.read(cx).sidebar_open {
                            this.shell.update(cx, |s, cx| s.set_sidebar(true, cx));
                        }
                    }))
                    .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                        // GPUI re-tests hover on repaint against the last pointer position, stale once the pointer left the surface.
                        if *hovered && !window.is_window_hovered() {
                            return;
                        }
                        if *hovered {
                            this.cancel_leave();
                        } else if !this.shell.read(cx).sidebar_open && !crate::state::config(cx).read(cx).get().sidebar.pinned() {
                            this.schedule_leave(Leave::Sidebar, cx);
                        }
                    }))
                    .child({
                        let pinned = cfg.sidebar.pinned();
                        let header = div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(tokens::XS))
                            .child(tabs)
                            .child(crate::kit::icon_button(
                                "sidebar-pin",
                                if pinned { "pin" } else { "pin-off" },
                                icon_btn,
                                if pinned { palette.primary } else { palette.on_surface_variant },
                                pinned.then_some(palette.primary.with_alpha(PIN_TINT_ALPHA)),
                                move |_, cx| crate::state::update_config(cx, |c| c.sidebar.set_pinned(!pinned)),
                            ))
                            .child(crate::kit::icon_button("sidebar-close", "close", icon_btn, palette.on_surface_variant, None, |_, cx| {
                                crate::state::update_config(cx, |c| c.sidebar.set_pinned(false));
                                let shell = crate::state::shell(cx);
                                shell.update(cx, |s, cx| s.set_sidebar(false, cx));
                            }));
                        let scroll = div().id("sidebar-scroll").overflow_y_scroll().track_scroll(&scroll_handle).p(px(SIDEBAR_PAD_PX)).flex().flex_col().gap(px(card_gap));
                        let with_bar = |scroll| crate::scrollbar::with_scrollbar(scroll, &scroll_handle, &scroll_bar, &palette);
                        match side_look.tab_position {
                            // The header scrolls with the cards (the shipped layout).
                            TabPosition::Top => with_bar(scroll.size_full().child(header).child(cards)).size_full().into_any_element(),
                            // Pinned under the scrolling cards.
                            TabPosition::Bottom => div()
                                .size_full()
                                .flex()
                                .flex_col()
                                .child(with_bar(scroll.flex_1().min_h(px(0.0)).child(cards)).flex_1().min_h(px(0.0)).flex().flex_col())
                                .child(div().px(px(SIDEBAR_PAD_PX)).pb(px(SIDEBAR_PAD_PX)).child(header))
                                .into_any_element(),
                        }
                    })
                    .children(tab_menu)
                    .child(self.mask.region()),
            );
        }
        // A floating sidebar leaves a gap at the screen edge: the pointer
        // resting there counts as on the sidebar. Else it closed the
        // hover-opened sidebar, the edge trigger reopened it under the
        // pointer, and the two took turns for as long as it stayed.
        if sidebar_p > HIDDEN_EPSILON && side_inset > 0.0 && !bar_on_sidebar_edge {
            let gap_x = if sidebar_right { (sidebar_x + sidebar_w).max(0.0) } else { 0.0 };
            let gap_w = if sidebar_right { (self.surface.0 - gap_x).max(0.0) } else { sidebar_x.max(0.0) };
            root = root.child(
                div()
                    .id("sidebar-edge-gap")
                    .absolute()
                    .left(px(gap_x))
                    .top(px(side_top))
                    .w(px(gap_w))
                    .h(px(side_h))
                    .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                        if *hovered && !window.is_window_hovered() {
                            return;
                        }
                        if *hovered {
                            this.cancel_leave();
                        } else if !this.shell.read(cx).sidebar_open && !crate::state::config(cx).read(cx).get().sidebar.pinned() {
                            this.schedule_leave(Leave::Sidebar, cx);
                        }
                    }))
                    .child(self.mask.region()),
            );
        }
        // (A side bar on the sidebar's edge owns that edge: open the
        // sidebar from the bar or a keybind instead.)
        if cfg.sidebar.enabled && cfg.sidebar.hover_reveal && sidebar_p < 0.5 && !bar_on_sidebar_edge {
            let h = free_h;
            root = root.child(
                div()
                    .id("sidebar-trigger")
                    .absolute()
                    .left(px(if sidebar_right { self.surface.0 - EDGE_STRIP } else { 0.0 }))
                    .top(px(top + h * SIDEBAR_TRIGGER_INSET))
                    .w(px(EDGE_STRIP))
                    .h(px(h * (1.0 - SIDEBAR_TRIGGER_INSET * 2.0)))
                    .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                        // GPUI re-tests hover on repaint against the last pointer position, stale once the pointer left the surface.
                        if *hovered && !window.is_window_hovered() {
                            return;
                        }
                        if *hovered {
                            this.cancel_leave();
                            this.shell.update(cx, |s, cx| s.set_sidebar_hover(true, cx));
                        }
                    }))
                    // Real motion on the edge: the arrival GPUI's hover state
                    // misses after a stale repaint.
                    .on_mouse_move(cx.listener(|this, _: &gpui::MouseMoveEvent, window, cx| {
                        if window.is_window_hovered() {
                            this.cancel_leave();
                            this.shell.update(cx, |s, cx| s.set_sidebar_hover(true, cx));
                        }
                    }))
                    .child(self.mask.region()),
            );
        }

        // ── Bar reveal strip / hover band, hot corners, screen corners ────
        let surface = self.surface;
        root = root.children(self.autohide.elements(&cfg, &self.monitor, surface, (band_y, band_h), EDGE_STRIP, &self.mask, |v: &mut ShellView| &mut v.autohide, cx));
        root = root.children(self.corners.elements(&cfg, surface, &self.mask, |v: &mut ShellView| &mut v.corners, cx));

        // ── Edge triggers: launcher (bottom) and session dock (right) ─────
        let dwell = Duration::from_millis(cfg.edges.dwell_ms);
        let (edge_start, edge_span) = (EDGE_TRIGGER_START, EDGE_TRIGGER_SPAN);
        let (sw, sh) = self.surface;
        // The dock owns the bottom edge while it's on: the launcher is then
        // click-only (`ShellConfig::launcher_hover_enabled`).
        if cfg.launcher_hover_enabled() {
            root = root.child(
                div()
                    .id("edge-bottom")
                    .absolute()
                    .left(px(sw * edge_start))
                    // A bottom bar owns the bottom edge: the trigger moves
                    // to the top edge instead.
                    .top(px(if bottom { 0.0 } else { sh - EDGE_STRIP }))
                    .w(px(sw * edge_span))
                    .h(px(EDGE_STRIP))
                    .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                        // GPUI re-tests hover on repaint against the last pointer position, stale once the pointer left the surface.
                        if *hovered && !window.is_window_hovered() {
                            return;
                        }
                        if *hovered {
                            this.arm_edge(EdgeAction::Launcher, dwell, cx);
                        } else {
                            this.edge_task = None;
                        }
                    }))
                    .on_mouse_move(cx.listener(move |this, _: &gpui::MouseMoveEvent, _, cx| {
                        if this.edge_task.is_none() {
                            this.arm_edge(EdgeAction::Launcher, dwell, cx);
                        }
                    }))
                    .child(self.mask.region()),
            );
        }
        let niri = crate::hub::hub(cx).read(cx).compositor.as_ref().is_some_and(|c| c.kind() == mandara_compositor::CompositorKind::Niri);
        // A side bar on the session dock's edge owns that edge.
        let bar_on_session_edge = vertical && right_bar != sidebar_right;
        if cfg.edges.session_right && !niri && !bar_on_session_edge {
            // The session dock takes the edge opposite the sidebar.
            root = root.child(
                div()
                    .id("edge-session")
                    .absolute()
                    .left(px(if sidebar_right { 0.0 } else { sw - EDGE_STRIP }))
                    .top(px(sh * edge_start))
                    .w(px(EDGE_STRIP))
                    .h(px(sh * edge_span))
                    .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                        // GPUI re-tests hover on repaint against the last pointer position, stale once the pointer left the surface.
                        if *hovered && !window.is_window_hovered() {
                            return;
                        }
                        if *hovered {
                            this.arm_edge(EdgeAction::Session, dwell, cx);
                        } else {
                            this.edge_task = None;
                        }
                    }))
                    .on_mouse_move(cx.listener(move |this, _: &gpui::MouseMoveEvent, _, cx| {
                        if this.edge_task.is_none() {
                            this.arm_edge(EdgeAction::Session, dwell, cx);
                        }
                    }))
                    .child(self.mask.region()),
            );
        }

        root.child(self.mask.commit())
    }
}

/// Opens the invisible exclusive-zone strips of one output *on demand*.
///
/// Every GPUI window carries a fixed cost (its own swapchain, pipelines,
/// atlases and instance buffers; shader compilation when created), so
/// instead of four strips
/// per output (bar top + bottom, sidebar left + right) only the ones that
/// currently reserve space are created: the bar's edge at start, the
/// sidebar's edge once its mode reserves space, the other bar edge
/// only if the bar moves there. A created strip is kept (it reserves 0 px
/// while unused) so toggling never re-creates surfaces mid-animation.
///
/// In Float mode (the default) the sidebar floats over windows and reserves
/// nothing: reserving on every open makes the compositor re-tile all
/// windows twice per open / close. Push mode asks for exactly that, so its
/// strip is created as soon as the mode is chosen (never on the first open:
/// a new GPUI window mid-slide costs a swapchain and pipelines).
pub fn open_exclusion_strips(display: Option<gpui::DisplayId>, output: (f32, f32), monitor: String, cx: &mut App) {
    use gpui::layer_shell::Anchor;
    use mandara_core::LayerSurfaceSpec;
    // [bar top, bar bottom, sidebar left, sidebar right]
    // Slots: bar top, bottom, left, right; sidebar left, right.
    let opened: Rc<Cell<[bool; 6]>> = Rc::default();
    let ensure = move |cx: &mut App| {
        let cfg = config(cx).read(cx).snapshot();
        let sidebar_reserves = sidebar_may_reserve(&cfg);
        let mut state = opened.get();
        for (i, position) in BarPosition::ALL.into_iter().enumerate() {
            let zone = ExclusionView::zone_for(position, &cfg, &monitor);
            if state[i] || zone <= 0.0 {
                continue;
            }
            state[i] = true;
            let (anchor, length) = match position {
                BarPosition::Top => (Anchor::TOP | Anchor::LEFT | Anchor::RIGHT, output.0),
                BarPosition::Bottom => (Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT, output.0),
                BarPosition::Left => (Anchor::LEFT | Anchor::TOP | Anchor::BOTTOM, output.1),
                BarPosition::Right => (Anchor::RIGHT | Anchor::TOP | Anchor::BOTTOM, output.1),
            };
            let strip = LayerSurfaceSpec::exclusion_strip(anchor, px(length), px(zone), display);
            if let Err(err) = cx.open_window(strip.window_options(), { let monitor = monitor.clone(); move |_, cx| cx.new(|cx| ExclusionView::new(position, cx).on_monitor(monitor)) }) {
                tracing::error!("exclusion strip: {err:#}");
            }
        }
        for (i, right) in [false, true].into_iter().enumerate() {
            let slot = BarPosition::ALL.len() + i;
            if state[slot] || !sidebar_reserves || cfg.sidebar_on_right() != right {
                continue;
            }
            state[slot] = true;
            let edge = if right { Anchor::RIGHT } else { Anchor::LEFT };
            let strip = LayerSurfaceSpec::exclusion_strip(Anchor::TOP | Anchor::BOTTOM | edge, px(output.1), px(0.0), display);
            if let Err(err) = cx.open_window(strip.window_options(), move |_, cx| cx.new(|cx| SidebarExclusionView::new(right, cx))) {
                tracing::error!("sidebar exclusion strip: {err:#}");
            }
        }
        opened.set(state);
    };
    let ensure = Rc::new(ensure);
    ensure(cx);
    let on_config = ensure.clone();
    cx.observe(&config(cx), move |_, cx| on_config(cx)).detach();
}

/// Whether the sidebar's mode ever reserves its width (Push or Pinned): its
/// exclusion strip is created up front then, never mid-slide.
pub fn sidebar_may_reserve(cfg: &ShellConfig) -> bool {
    cfg.sidebar.enabled && cfg.sidebar.mode.reserves_space()
}

/// Whether the sidebar reserves its width right now: always when pinned,
/// while open in Push mode, never in Float mode (it floats as an overlay).
pub fn sidebar_reserves_space(cfg: &ShellConfig, open: bool) -> bool {
    sidebar_may_reserve(cfg) && (cfg.sidebar.pinned() || open)
}

/// Invisible strip on one screen edge reserving the sidebar's width while
/// it is pinned (or open in Push mode), so the compositor tiles windows
/// beside it. One strip lives on each edge; only the one on the sidebar's
/// side reserves.
pub struct SidebarExclusionView {
    config: Entity<ConfigStore>,
    shell: Entity<ShellState>,
    applied: f32,
    right: bool,
}

impl SidebarExclusionView {
    pub fn new(right: bool, cx: &mut Context<Self>) -> Self {
        let config = config(cx);
        let shell = crate::state::shell(cx);
        cx.observe(&config, |_, _, cx| cx.notify()).detach();
        // Push mode follows the sidebar opening and closing.
        cx.observe(&shell, |_, _, cx| cx.notify()).detach();
        Self { config, shell, applied: -1.0, right }
    }
}

impl Render for SidebarExclusionView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cfg = self.config.read(cx).snapshot();
        let open = self.shell.read(cx).sidebar_visible();
        let zone = if sidebar_reserves_space(&cfg, open) && cfg.sidebar_on_right() == self.right { cfg.sidebar.width + cfg.sidebar.look.inset_with(&cfg.bar) } else { 0.0 };
        if (zone - self.applied).abs() > ZONE_EPSILON_PX {
            window.set_exclusive_zone(px(zone));
            window.set_input_region(Some(&[]));
            self.applied = zone;
        }
        div().size_full()
    }
}

/// Invisible strip that only reserves the bar's exclusive zone. One strip
/// lives on the top edge and one on the bottom; only the one on the bar's
/// configured edge reserves, so the position can switch live.
pub struct ExclusionView {
    config: Entity<ConfigStore>,
    applied: f32,
    position: BarPosition,
    /// The output's connector name (per-output bars / auto-hide).
    monitor: String,
}

impl ExclusionView {
    pub fn new(position: BarPosition, cx: &mut Context<Self>) -> Self {
        let config = config(cx);
        cx.observe(&config, |_, _, cx| cx.notify()).detach();
        Self { config, applied: -1.0, position, monitor: String::new() }
    }

    pub fn on_monitor(mut self, monitor: String) -> Self {
        self.monitor = monitor;
        self
    }

    /// The zone this strip should reserve for `cfg` on `monitor`: none when
    /// the bar is hidden there or floats over windows (auto-hide).
    pub fn zone_for(position: BarPosition, cfg: &ShellConfig, monitor: &str) -> f32 {
        if cfg.bar.position == position && crate::bar::autohide::reserves_space(cfg, monitor) { layout::exclusive_zone(&Metrics::of(cfg, crate::style::radius_scale())) } else { 0.0 }
    }
}

impl Render for ExclusionView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cfg = self.config.read(cx).snapshot();
        let zone = Self::zone_for(self.position, &cfg, &self.monitor);
        if (zone - self.applied).abs() > ZONE_EPSILON_PX {
            window.set_exclusive_zone(px(zone));
            window.set_input_region(Some(&[]));
            self.applied = zone;
        }
        div().size_full()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_shape_sliders_re_target_the_pills() {
        let cfg = ShellConfig::default();
        let surface = (2560.0, 1440.0);
        let widths = [120.0, 300.0, 240.0];
        let key = |c: &ShellConfig, scale: f32, w: [f32; 3]| layout_key(w, Metrics::of(c, scale), surface);
        let base = key(&cfg, 1.0, widths);
        assert_eq!(base, key(&cfg, 1.0, [120.2, 299.9, 240.0]), "sub-pixel width jitter keeps the layout");
        assert_ne!(key(&cfg, 0.5, widths), base, "the global roundness re-shapes the bar");
        let edits: [fn(&mut ShellConfig); 14] = [
            |c| c.bar.edge_gap += 2.0,
            |c| c.bar.side_gap += 2.0,
            |c| c.bar.corner_radius -= 2.0,
            |c| c.bar.flare_radius += 2.0,
            |c| c.bar.height += 2.0,
            |c| c.bar.position = if c.bar.position == BarPosition::Top { BarPosition::Bottom } else { BarPosition::Top },
            |c| c.bar.layout = if c.bar.layout == BarLayout::MinimalEdge { BarLayout::FloatingPills } else { BarLayout::MinimalEdge },
            |c| c.bar.chip_padding += 2.0,
            |c| c.bar.chip_spacing += 2.0,
            |c| c.bar.separator = mandara_config::surfaces::BarSeparator::Dot,
            |c| c.bar.center_align = mandara_config::surfaces::BarCenterAlign::Between,
            |c| c.bar.floating_frame = true,
            |c| c.panels.gap += 2.0,
            |c| c.panels.anchor = mandara_config::surfaces::PanelAnchor::Center,
        ];
        for (i, edit) in edits.iter().enumerate() {
            let mut changed = cfg.clone();
            edit(&mut changed);
            assert_ne!(key(&changed, 1.0, widths), base, "edit {i} must re-layout the bar");
        }
    }

    #[test]
    fn custom_springs_replace_the_shipped_feel() {
        let mut o = mandara_config::surfaces::SpringOverride::default();
        assert_eq!(spring_for(PANEL_SPRING, &o), (PANEL_SPRING, false));
        o.custom = true;
        o.stiffness = 500.0;
        o.damping = 30.0;
        let (s, custom) = spring_for(PANEL_SPRING, &o);
        assert!(custom);
        assert_eq!((s.stiffness, s.damping), (500.0, 30.0));
    }
}
