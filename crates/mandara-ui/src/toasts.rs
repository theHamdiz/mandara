//! Notification popups on their own `mandara-notifications` layer surface,
//! opened while popups exist and destroyed afterwards.
//!
//! The surface sits in the corner or edge `notifications.position` names
//! (on the overlay layer when `over_fullscreen`, so it shows above
//! fullscreen windows), on the output `[displays.surfaces.notifications]`
//! allows (the focused one when it may). Toasts spring in from their edge,
//! close the gap smoothly when one leaves, pause their countdown while
//! hovered and draw a thin countdown line. Only the toasts take input: the
//! rest of the surface passes clicks through (`set_input_region`).
//!
//! Joined (`notifications.joined`), the stack hangs from the bar (or its
//! bare screen edge) as one drawer in the bar's colour, with concave flares
//! where it meets it ([`crate::toast_drawer`]); the surface then reaches
//! over the bar's reserved band so the drawer can touch the bar, and
//! follows the bar while it auto-hides ([`publish_bar_slide`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use gpui::{
    AnyElement, App, AppContext as _, BoxShadow, Context, DispatchPhase, Entity, Global, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement,
    Render, StatefulInteractiveElement as _, Styled, Task, Window, WindowHandle, canvas, div, point, prelude::FluentBuilder as _, px,
};
use mandara_config::alerts::{NotificationsConfig, ToastPosition};
use mandara_config::outputs::SurfaceKind;
use mandara_config::surfaces::ranges;
use mandara_config::{BarPosition, ShellConfig};
use mandara_core::{Edge, FrameClock, InputMaskRegistry, LayerSurfaceSpec, SpringConfig, SpringValue, SurfaceRole, ToGpui, edge_drawer_snapped, oklab_gradient};
use mandara_services::{Notification, NotificationCommand};
use smallvec::SmallVec;

use crate::plugins::notifications::{CardStyle, notification_card_styled};
use crate::plugins::panel_fx::{partner, readable};
use crate::state::config;
use crate::style::ClipRounded as _;
use crate::swipe::Swipe;
use crate::toast_drawer::{self, Drawer};

/// Distance between the toasts and the screen edges, logical px.
const EDGE_GAP_PX: f32 = 14.0;
/// Gap between stacked toasts.
const STACK_GAP_PX: f32 = 10.0;
/// Room around the stack for shadows.
const SHADOW_ROOM_PX: f32 = 16.0;
/// Height reserved per toast when sizing the surface (the tallest card:
/// title, two body lines, progress and an action row), before scaling.
const TOAST_MAX_H_PX: f32 = 168.0;
/// Height assumed for a toast before its first layout.
const TOAST_GUESS_H_PX: f32 = 84.0;
/// The surface never exceeds this share of the output's height.
const MAX_SURFACE_SHARE: f32 = 0.85;
/// Fallback output height before outputs are known.
const FALLBACK_OUTPUT_H: f32 = 1080.0;
/// Entrance / exit travel from the edge.
const SLIDE_PX: f32 = 44.0;
const TOAST_RADIUS_PX: f32 = 16.0;
const SHADOW_ALPHA: f32 = 0.22;
const SHADOW_Y_PX: f32 = 3.0;
const SHADOW_BLUR_PX: f32 = 10.0;
/// Countdown line.
const PROGRESS_H_PX: f32 = 2.5;
const PROGRESS_INSET_PX: f32 = 14.0;
const PROGRESS_ALPHA: f32 = 0.7;
const ANGLE_RIGHT_DEG: f32 = 90.0;
/// Presence below which a leaving toast is dropped.
const GONE: f32 = 0.02;
/// Height changes below this don't move the stack.
const MEASURE_EPSILON_PX: f32 = 0.5;
/// The surface counts as configured once within this many px of its size.
const SIZE_TOLERANCE_PX: f32 = 2.0;
/// Slack added to expiry wake-ups so the toast is due when we wake.
const WAKE_SLACK: Duration = Duration::from_millis(30);
const PRESENCE_SPRING: SpringConfig = SpringConfig::new(320.0, 26.0, 1.0);
const SLOT_SPRING: SpringConfig = SpringConfig::new(300.0, 28.0, 1.0);
/// Swiping a toast away: the pointer travels this far before the toast
/// follows it, and past this share of its width the toast flies off (hidden,
/// still in the notification list); a shorter swipe springs back.
const SWIPE_DISMISS_SHARE: f32 = 0.35;
/// How much a toast fades over a swipe as long as its width.
const SWIPE_FADE: f32 = 0.7;
/// Joined: room between the drawer's edges and the popups in it (and its
/// corners' extra radius, so popup and drawer corners stay concentric),
/// before scaling.
const DRAWER_PAD_PX: f32 = 8.0;
/// A bar at least this opaque lets the drawer overlap it by a pixel to hide
/// the seam (a translucent one would show the overlap darker).
const OPAQUE_ALPHA: f32 = 0.999;
/// Joined drawers shorter than this (px) are not drawn.
const MIN_DRAWER_PX: f32 = 0.5;
/// Output width assumed before outputs are known.
const FALLBACK_OUTPUT_W: f32 = 1920.0;

/// How far each output's bar has slid toward its edge (auto-hide), px. The
/// bar publishes it only while popups are joined, and only when it changes,
/// so the joined drawer slides with the bar.
#[derive(Default)]
pub struct BarSlides(SmallVec<[(String, f32); 2]>);
impl Global for BarSlides {}

/// Records how far `output`'s bar has slid (`slid` px; 0 = fully out).
pub fn publish_bar_slide(output: &str, slid: f32, cx: &mut App) {
    let current = cx.try_global::<BarSlides>().and_then(|b| b.0.iter().find(|(o, _)| o == output).map(|(_, s)| *s));
    if current == Some(slid) || (current.is_none() && slid == 0.0) {
        return;
    }
    let slides = cx.default_global::<BarSlides>();
    match slides.0.iter_mut().find(|(o, _)| o == output) {
        Some(entry) => entry.1 = slid,
        None => slides.0.push((output.to_string(), slid)),
    }
}

/// How far the bar on `output` has slid (0 when unknown: fully out).
fn bar_slid(output: Option<&str>, cx: &App) -> f32 {
    let Some(slides) = cx.try_global::<BarSlides>() else { return 0.0 };
    output.and_then(|o| slides.0.iter().find(|(name, _)| name == o)).map_or(0.0, |(_, s)| *s)
}

/// The output popups show on.
#[derive(Clone, Debug, PartialEq)]
struct PopupOutput {
    name: String,
    /// Logical width.
    width: f32,
    /// Logical height.
    height: f32,
    /// Its place in the output list (the dock's "first display" counts by
    /// it).
    index: usize,
}

/// The output popups show on: the one they're assigned to, else the
/// focused window's (the compositor's pick).
fn popup_output(cx: &App) -> Option<PopupOutput> {
    let outputs = &cx.try_global::<crate::overlays::Outputs>()?.0;
    let displays = &config(cx).read(cx).get().displays;
    let o = crate::outputs::pick_output(outputs, focused_window_monitor(cx).as_deref(), displays, SurfaceKind::Notifications)?;
    let index = outputs.iter().position(|g| g.name == o.name).unwrap_or(0);
    Some(PopupOutput { name: o.name.clone(), width: o.width as f32, height: o.height as f32, index })
}

/// Whether a bottom dock shows on `output` (on every output it may show
/// on while the popups' output is unknown).
fn dock_holds_bottom(cfg: &ShellConfig, output: Option<&PopupOutput>) -> bool {
    cfg.dock_bottom_clearance().is_some() && output.is_none_or(|o| cfg.dock_on_output(&o.name, o.index))
}

/// The display a popup surface opens on: joined popups take their shape
/// from one output's bar, so they open on that output; others go where
/// the compositor (or the notifications' screen choice) puts them.
fn surface_display<T>(joined: bool, on_popup_output: impl FnOnce() -> Option<T>, otherwise: impl FnOnce() -> Option<T>) -> Option<T> {
    if joined { on_popup_output().or_else(otherwise) } else { otherwise() }
}

/// The joined drawer on `output` with the bar slid `slid` px (`None`:
/// joining is off, or there is nothing to join there).
fn joined_drawer(cfg: &ShellConfig, output: Option<&PopupOutput>, slid: f32) -> Option<Drawer> {
    let ncfg = &cfg.notifications;
    if !ncfg.joined {
        return None;
    }
    let scale = ncfg.scale.clamp(crate::plugins::notifications::TOAST_SCALE.0, crate::plugins::notifications::TOAST_SCALE.1);
    let bar = crate::bar::layout::Metrics::of(cfg, crate::style::radius_scale());
    // A side bar's (or a pinned flush sidebar's) fillet where it meets the
    // popups' edge.
    let fillet = |right: bool| {
        let side_bar = if cfg.side_bar_on(right) { cfg.side_bars.flare_radius() } else { 0.0 };
        let sidebar = if crate::shell::sidebar_reserves_space(cfg, false) && cfg.sidebar_on_right() == right { bar.flare } else { 0.0 };
        side_bar.max(sidebar)
    };
    toast_drawer::solve(&toast_drawer::Inputs {
        position: ncfg.position,
        bar,
        bar_here: output.is_none_or(|o| cfg.bar_on_output(&o.name)),
        bar_slid: slid,
        screen_w: output.map_or(FALLBACK_OUTPUT_W, |o| o.width),
        min_margin: EDGE_GAP_PX,
        dock_bottom: dock_holds_bottom(cfg, output),
        // The narrowest card Studio offers (or a narrower one set by hand).
        min_width: (ncfg.width.min(mandara_config::alerts::defaults::TOAST_MIN_WIDTH_PX) + DRAWER_PAD_PX * 2.0) * scale,
        side_fillets: (fillet(false), fillet(true)),
    })
}

/// The focused window's monitor: cheap enough for every hub update (the
/// surface key is rebuilt on each one while toasts are up).
fn focused_window_monitor(cx: &App) -> Option<String> {
    crate::hub::hub(cx).read(cx).focused.as_ref().map(|c| c.monitor.clone())
}

/// What the surface was opened with; a change reopens it.
#[derive(Clone, Debug, PartialEq)]
struct SurfaceKey {
    position: ToastPosition,
    overlay: bool,
    size: (f32, f32),
    display: Option<gpui::DisplayId>,
    /// Joined: how far the surface reaches back over the bar's reserved
    /// band on its edge (a negative margin), so the drawer can touch the
    /// bar. 0 otherwise.
    pull: f32,
    /// How many popups show at once: the setting, or fewer when the
    /// surface's height cap can't hold that many (a short display). The
    /// rest wait their turn, their countdown not yet started.
    cards: usize,
}

/// How many cards of `card_h` (gap included) fit in a stack capped at
/// `cap`, at least one and at most `max_visible`.
fn cards_that_fit(max_visible: usize, card_h: f32, cap: f32) -> usize {
    let fit = ((cap - EDGE_GAP_PX * 2.0) / card_h.max(1.0)).floor().max(1.0) as usize;
    fit.min(max_visible.max(1))
}

impl SurfaceKey {
    /// Size the visible stack against its own output, including joined-bar geometry.
    fn of(shell: &ShellConfig, output: Option<&PopupOutput>, cx: &App) -> Self {
        let cfg = &shell.notifications;
        let scale = cfg.scale.clamp(crate::plugins::notifications::TOAST_SCALE.0, crate::plugins::notifications::TOAST_SCALE.1);
        let card_w = cfg.width * scale;
        let joined = joined_drawer(shell, output, 0.0);
        // Capped by the popups' own display, joined or not; the first
        // output only when it isn't known.
        let first_h = || cx.try_global::<crate::overlays::Outputs>().and_then(|o| o.0.first().map(|g| g.height as f32)).unwrap_or(FALLBACK_OUTPUT_H);
        let output_h = output.map_or_else(first_h, |o| o.height);
        let card_h = TOAST_MAX_H_PX * scale + STACK_GAP_PX;
        let cards = cards_that_fit(cfg.max_visible, card_h, output_h * MAX_SURFACE_SHARE);
        let stack_h = cards as f32 * card_h + EDGE_GAP_PX * 2.0;
        let display = surface_display(joined.is_some(), || output.and_then(|o| crate::outputs::display_for(&o.name, cx)), || crate::outputs::transient_display(SurfaceKind::Notifications, focused_window_monitor, cx),
        );
        let (w, h, pull) = match joined {
            Some(d) => {
                // From the screen edge: over the bar's band (or the screen
                // frame's strip) to the drawer's far end.
                let edge = if cfg.position.is_bottom() { BarPosition::Bottom } else { BarPosition::Top };
                let monitor = output.map_or("", |o| o.name.as_str());
                let pull = crate::shell::ExclusionView::zone_for(edge, shell, monitor);
                let m = crate::bar::layout::Metrics::of(shell, crate::style::radius_scale());
                let reach = pull.max(m.bar_far_edge() + m.screen_frame);
                let w = toast_drawer::surface_width(&d, card_w + DRAWER_PAD_PX * scale * 2.0);
                (w, (stack_h + reach).min(output_h * MAX_SURFACE_SHARE + reach), pull)
            }
            None => (card_w + (EDGE_GAP_PX + SHADOW_ROOM_PX) * 2.0, stack_h.min(output_h * MAX_SURFACE_SHARE), 0.0),
        };
        Self { position: cfg.position, overlay: cfg.over_fullscreen, size: (w.round(), h.round()), display, pull: pull.round(), cards }
    }

    fn spec(&self) -> LayerSurfaceSpec {
        let vertical = if self.position.is_bottom() { Anchor::BOTTOM } else { Anchor::TOP };
        let horizontal = match self.position.column() {
            -1 => Anchor::LEFT,
            1 => Anchor::RIGHT,
            _ => Anchor::empty(),
        };
        LayerSurfaceSpec {
            role: SurfaceRole::Notifications,
            layer: if self.overlay { Layer::Overlay } else { Layer::Top },
            anchor: vertical | horizontal,
            size: gpui::size(px(self.size.0), px(self.size.1)),
            // Joined: reach back over the bar's band (the other edges keep
            // clear of whatever reserves them).
            margin: if self.position.is_bottom() { (px(0.0), px(0.0), px(-self.pull), px(0.0)) } else { (px(-self.pull), px(0.0), px(0.0), px(0.0)) },
            // Stay clear of the bar's reserved band.
            exclusive_zone: Some(px(0.0)),
            keyboard: KeyboardInteractivity::None,
            display: self.display,
        }
    }
}

/// The toast surface host (a global).
struct Toasts {
    window: Option<WindowHandle<ToastView>>,
    key: Option<SurfaceKey>,
    /// Last visibility command accepted by the service queue; its state echo can arrive later.
    list_requested: Option<bool>,
    /// The output the popups show on (joined popups shape themselves after
    /// its bar).
    output: Option<PopupOutput>,
}

#[derive(Clone)]
struct GlobalToasts(Entity<Toasts>);
impl Global for GlobalToasts {}

/// Opens / reopens / leaves the surface to match the popups and settings.
fn sync(cx: &mut App) {
    let Some(host) = cx.try_global::<GlobalToasts>().map(|g| g.0.clone()) else { return };
    let any_popup = crate::hub::hub(cx).read(cx).notifications.items.iter().any(|n| n.popup);
    let cfg = config(cx).read(cx).snapshot();
    let alive = host.read(cx).window.as_ref().is_some_and(|h| cx.windows().iter().any(|w| w.window_id() == h.window_id()));
    if !alive {
        host.update(cx, |t, _| {
            t.window = None;
            t.key = None;
        });
    }
    if !any_popup {
        // An open surface lets its last toasts leave and then closes itself.
        return;
    }
    let output = popup_output(cx);
    let key = SurfaceKey::of(&cfg, output.as_ref(), cx);
    host.update(cx, |t, _| t.output = output);
    let current = host.read(cx).key.clone();
    if alive && current.as_ref() == Some(&key) {
        return;
    }
    if let Some(h) = host.update(cx, |t, _| t.window.take()) {
        let _ = h.update(cx, |_, window, _| window.remove_window());
    }
    let spec = key.spec();
    match cx.open_window(spec.window_options(), |_, cx| cx.new(ToastView::new)) {
        Ok(handle) => host.update(cx, |t, _| {
            t.window = Some(handle);
            t.key = Some(key);
        }),
        Err(err) => tracing::warn!("notification popups: {err:#}"),
    }
}

/// Whether a notification list is on screen: the bar's notifications panel,
/// or the sidebar (opened or hover-revealed) on its Notifications tab.
fn list_on_screen(cx: &App) -> bool {
    let s = crate::state::shell(cx).read(cx);
    let sidebar_tab = (s.sidebar_open || s.sidebar_hover) && config(cx).read(cx).get().sidebar.notifications_tab && crate::sidebar_tabs::notifications_selected(cx);
    s.panel.as_deref() == Some(crate::plugins::notifications::ID) || sidebar_tab
}

/// Records accepted requests rather than delayed service echoes, so a quick
/// open followed by a close queues both transitions in order.
fn queue_list_visibility(last: &mut Option<bool>, open: bool, enqueue: impl FnOnce(bool) -> bool) {
    if *last != Some(open) && enqueue(open) {
        *last = Some(open);
    }
}

/// Tells the notification service whether its list is on screen, so the
/// popups hand over to it (and new ones go straight into it) while it is.
pub fn sync_list(cx: &mut App) {
    let open = list_on_screen(cx);
    let Some(host) = cx.try_global::<GlobalToasts>().map(|g| g.0.clone()) else { return };
    if host.read(cx).list_requested == Some(open) { return; }
    let Some(service) = crate::hub::hub(cx).read(cx).notification_service.clone() else { return };
    host.update(cx, |host, _| queue_list_visibility(&mut host.list_requested, open, |open| service.try_send(NotificationCommand::ListOpen(open)).is_ok()));
}

/// Installs the popup surface; it follows the hub and the config.
pub fn init(cx: &mut App) {
    let host = cx.new(|_| Toasts { window: None, key: None, list_requested: None, output: None });
    cx.set_global(GlobalToasts(host));
    let hub = crate::hub::hub(cx);
    let mut last: Vec<u32> = Vec::new();
    cx.observe(&hub, move |hub, cx| {
        // A service update also retries a visibility write if its queue was full.
        sync_list(cx);
        let popups: Vec<u32> = hub.read(cx).notifications.items.iter().filter(|n| n.popup).map(|n| n.id).collect();
        if popups != last {
            last = popups;
            sync(cx);
        }
    })
    .detach();
    cx.observe(&crate::state::shell(cx), move |_, cx| {
        sync_list(cx);
        // Joined popups follow the bar's reserved band (`bar hide`).
        if config(cx).read(cx).get().notifications.joined {
            sync(cx);
        }
    })
    .detach();
    let mut last_cfg: Option<NotificationsConfig> = None;
    cx.observe(&config(cx), move |cfg, cx| {
        let next = cfg.read(cx).get().notifications.clone();
        // Joined popups also follow the bar's shape and edge.
        if last_cfg.as_ref() != Some(&next) || next.joined {
            last_cfg = Some(next);
            sync(cx);
        }
    })
    .detach();
    sync_list(cx);
    sync(cx);
}

/// One toast on screen (or leaving it).
struct Item {
    n: Notification,
    first_shown: Instant,
    /// Time spent hovered (the countdown pauses).
    paused: Duration,
    hovered_since: Option<Instant>,
    presence: SpringValue<f32>,
    /// Offset from the stack's anchored edge.
    slot: SpringValue<f32>,
    /// Sideways offset while (or after) it is swiped.
    swipe: Swipe,
    leaving: bool,
    expired: bool,
}

impl Item {
    fn new(n: Notification, now: Instant, reduce: bool) -> Self {
        let mut presence = SpringValue::new(0.0, PRESENCE_SPRING);
        presence.set_instant(reduce);
        presence.set_target(1.0);
        let mut slot = SpringValue::new(0.0, SLOT_SPRING);
        slot.set_instant(reduce);
        let mut swipe = Swipe::default();
        swipe.motion.set_instant(reduce);
        Item { n, first_shown: now, paused: Duration::ZERO, hovered_since: None, presence, slot, swipe, leaving: false, expired: false }
    }

    /// The pointer left: the countdown runs again.
    fn resume(&mut self, now: Instant) {
        if let Some(since) = self.hovered_since.take() {
            self.paused += now.saturating_duration_since(since);
        }
    }

    /// The pointer moved to `x` with the button down: past the slop the toast
    /// follows it 1:1. Returns whether anything changed.
    fn drag_to(&mut self, x: f32, y: f32) -> bool {
        self.swipe.drag_to(x, y)
    }

    /// The button came up. A swipe past `width × SWIPE_DISMISS_SHARE` flies
    /// off (returns `Some(true)`: hide it), a shorter one springs back
    /// (`Some(false)`); a plain click returns `None` and stays a click.
    fn release(&mut self, width: f32) -> Option<bool> {
        self.swipe.release(width * SWIPE_DISMISS_SHARE, width + SHADOW_ROOM_PX * 2.0)
    }
    /// Countdown progress `0..=1` at `now` (`None` for sticky toasts).
    fn elapsed_fraction(&self, now: Instant) -> Option<f32> {
        if self.n.display_ms == 0 {
            return None;
        }
        let paused = self.paused + self.hovered_since.map_or(Duration::ZERO, |h| now.saturating_duration_since(h));
        let elapsed = now.saturating_duration_since(self.first_shown).saturating_sub(paused);
        Some((elapsed.as_secs_f32() / Duration::from_millis(u64::from(self.n.display_ms)).as_secs_f32()).clamp(0.0, 1.0))
    }

    fn due(&self, now: Instant) -> Option<Instant> {
        if self.n.display_ms == 0 || self.hovered_since.is_some() || self.leaving {
            return None;
        }
        let elapsed = now.saturating_duration_since(self.first_shown).saturating_sub(self.paused);
        Some(now + Duration::from_millis(u64::from(self.n.display_ms)).saturating_sub(elapsed))
    }
}

pub struct ToastView {
    items: Vec<Item>,
    heights: Rc<RefCell<HashMap<u32, f32>>>,
    clock: FrameClock,
    mask: InputMaskRegistry,
    /// The single pending wake-up for the next expiry, and its due time.
    timer: Option<(Instant, Task<()>)>,
}

impl ToastView {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.observe(&crate::hub::hub(cx), |_, _, cx| cx.notify()).detach();
        cx.observe(&config(cx), |_, _, cx| cx.notify()).detach();
        cx.observe(&mandara_core::theme_entity(cx), |_, _, cx| cx.notify()).detach();
        // The bar sliding (auto-hide) moves a joined drawer with it.
        cx.observe_global::<BarSlides>(|_, cx| cx.notify()).detach();
        Self { items: Vec::new(), heights: Rc::default(), clock: FrameClock::new(), mask: InputMaskRegistry::new(), timer: None }
    }

    /// Merges the hub's popups into the on-screen list (newest first).
    fn reconcile(&mut self, popups: &[Notification], reduce: bool, now: Instant) {
        for item in &mut self.items {
            match popups.iter().find(|n| n.id == item.n.id) {
                Some(n) => {
                    item.n = n.clone();
                    if item.leaving {
                        // Came back (e.g. replaced while leaving).
                        item.leaving = false;
                        item.expired = false;
                        item.presence.set_target(1.0);
                    }
                }
                None if !item.leaving => {
                    item.leaving = true;
                    item.presence.set_target(0.0);
                }
                None => {}
            }
        }
        for n in popups.iter().rev() {
            if !self.items.iter().any(|i| i.n.id == n.id) {
                self.items.insert(0, Item::new(n.clone(), now, reduce));
            }
        }
        // Newest first, leaving ones keep their place.
        let order = |id: u32| popups.iter().position(|n| n.id == id).unwrap_or(usize::MAX);
        self.items.sort_by_key(|i| if i.leaving { usize::MAX } else { order(i.n.id) });
    }
}

/// Popups have their own window, so they establish the panel font here.
fn popup_root(fonts: crate::fonts::Fonts) -> gpui::Div {
    div().size_full().relative().font_family(fonts.panels)
}

impl Render for ToastView {
    /// Reconcile notification lifetimes and animate the visible stack using the configured popup typography.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cfg = config(cx).read(cx).snapshot();
        let _motion = mandara_core::spring::MotionScope::new(mandara_config::motion::surface_speed(cfg.motion.surfaces.notifications));
        let ncfg = &cfg.notifications;
        let reduce = mandara_core::spring::motion_profile().reduce;
        let now = Instant::now();
        // As many as the surface was sized for (fewer on a short display).
        let shown = cx.try_global::<GlobalToasts>().and_then(|g| g.0.read(cx).key.as_ref().map(|k| k.cards)).unwrap_or(ncfg.max_visible.max(1));
        let popups: Vec<Notification> = crate::hub::hub(cx).read(cx).notifications.items.iter().filter(|n| n.popup).take(shown).cloned().collect();
        self.reconcile(&popups, reduce, now);

        // The pointer is gone without a hover-out (clicking a toast raises
        // its app's window): the countdown runs again (#24).
        if !window.is_window_hovered() {
            for item in self.items.iter_mut().filter(|i| !i.swipe.pressed()) {
                item.resume(now);
            }
        }

        // Expire due toasts (once each).
        for item in &mut self.items {
            if !item.leaving && !item.expired && item.elapsed_fraction(now).is_some_and(|f| f >= 1.0) {
                item.expired = true;
                crate::plugins::notifications::send(cx, NotificationCommand::Expire(item.n.id));
            }
        }

        // Stack: target offsets from measured heights.
        let style = CardStyle::toast(ncfg);
        let scale = style.scale;
        // Joined: the drawer on the popups' output, following the bar.
        let joined = if ncfg.joined {
            let output = cx.try_global::<GlobalToasts>().and_then(|g| g.0.read(cx).output.clone());
            let slid = bar_slid(output.as_ref().map(|o| o.name.as_str()), cx);
            joined_drawer(&cfg, output.as_ref(), slid)
        } else {
            None
        };
        let pad = if joined.is_some() { DRAWER_PAD_PX * scale } else { 0.0 };
        // (A drawer narrower than the popups, on a small notch, narrows them.)
        let w = match &joined {
            Some(d) => ((ncfg.width * scale + pad * 2.0).min(d.max_width) - pad * 2.0).max(0.0),
            None => ncfg.width * scale,
        };
        let vp = window.viewport_size();
        let (sw, sh) = (f32::from(vp.width), f32::from(vp.height));
        // Until the compositor configures the requested size, draw nothing
        // (positions depend on it).
        let expected = cx.try_global::<GlobalToasts>().and_then(|g| g.0.read(cx).key.as_ref().map(|k| k.size));
        if expected.is_some_and(|(w, h)| (w - sw).abs() > SIZE_TOLERANCE_PX || (h - sh).abs() > SIZE_TOLERANCE_PX) {
            window.request_animation_frame();
            return div().size_full().child(self.mask.commit()).into_any_element();
        }
        let heights = self.heights.borrow().clone();
        let mut offset = 0.0;
        for item in &mut self.items {
            item.slot.set_instant(reduce);
            item.presence.set_instant(reduce);
            item.swipe.motion.set_instant(reduce);
            if !item.leaving {
                if item.slot.value() == 0.0 && item.presence.value() == 0.0 {
                    item.slot.snap_to(offset);
                } else {
                    item.slot.set_target(offset);
                }
                offset += heights.get(&item.n.id).copied().unwrap_or(TOAST_GUESS_H_PX * scale) + STACK_GAP_PX;
            }
        }
        let dt = self.clock.tick(now);
        let mut animating = false;
        for item in &mut self.items {
            animating |= item.presence.step(dt);
            animating |= item.slot.step(dt);
            animating |= item.swipe.motion.step(dt);
        }
        self.items.retain(|i| !(i.leaving && i.presence.value() <= GONE && !i.presence.is_animating()));
        let any_timed = ncfg.progress_bar && self.items.iter().any(|i| !i.leaving && i.n.display_ms > 0);
        if animating || any_timed {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        if self.items.is_empty() && popups.is_empty() {
            // Nothing left: close the surface.
            if let Some(host) = cx.try_global::<GlobalToasts>().map(|g| g.0.clone()) {
                host.update(cx, |t, _| {
                    t.window = None;
                    t.key = None;
                });
            }
            window.remove_window();
            return div().into_any_element();
        }

        let palette = mandara_core::frame_palette(window, cx);
        let frame = crate::shell::frame_color(&palette);
        let fill = match &joined {
            // The bar's own material (tint, opacity), so the join is seamless.
            Some(_) => {
                let tinted = if cfg.bar.tint > 0.0 { frame.mix(palette.primary, ranges::BAR_TINT.clamp(cfg.bar.tint, 0.0)) } else { frame };
                tinted.with_alpha(frame.alpha() * ranges::OPACITY.clamp(cfg.bar.opacity, 1.0) * ncfg.opacity.clamp(0.0, 1.0)).hsla()
            }
            None => frame.with_alpha(frame.alpha() * ncfg.opacity.clamp(0.0, 1.0)).hsla(),
        };
        let accent = readable(palette.primary, &palette);
        let bottom = ncfg.position.is_bottom();
        let column = ncfg.position.column();
        let x0 = match column {
            -1 => EDGE_GAP_PX + SHADOW_ROOM_PX,
            1 => sw - w - EDGE_GAP_PX - SHADOW_ROOM_PX,
            _ => (sw - w) / 2.0,
        };
        // Joined: how far the drawer reaches from its line right now, and
        // where it starts.
        let drawer = joined.map(|d| {
            let reach = toast_drawer::extent(self.items.iter().map(|i| (i.slot.value(), heights.get(&i.n.id).copied().unwrap_or(TOAST_GUESS_H_PX * scale), i.presence.value())), pad);
            let dw = w + pad * 2.0;
            let (x, y) = toast_drawer::body_origin(&d, column, bottom, (sw, sh), (dw, reach));
            (d, x, y, dw, reach)
        });
        let mut root = popup_root(crate::fonts::fonts(cx));
        let mut toasts: Vec<AnyElement> = Vec::with_capacity(self.items.len());
        let this = cx.entity().downgrade();
        for item in &self.items {
            let id = item.n.id;
            let p = item.presence.value().clamp(0.0, 1.0);
            let travel = (1.0 - p) * SLIDE_PX;
            let h = heights.get(&id).copied().unwrap_or(TOAST_GUESS_H_PX * scale);
            let (left, top) = match drawer {
                // Inside the drawer: they come out of the bar's edge.
                Some((_, _, _, _, reach)) => (pad, if bottom { reach - pad - item.slot.value() - h + travel } else { pad + item.slot.value() - travel }),
                None => {
                    let (dx, dy) = match column {
                        -1 => (-travel, 0.0),
                        1 => (travel, 0.0),
                        _ => (0.0, if bottom { travel } else { -travel }),
                    };
                    let y = if bottom { sh - EDGE_GAP_PX - item.slot.value() - h } else { EDGE_GAP_PX + item.slot.value() };
                    (x0 + dx, y + dy)
                }
            };
            let measured = self.heights.clone();
            let entity = cx.entity().clone();
            let progress = ncfg.progress_bar.then(|| item.elapsed_fraction(now)).flatten();
            let card = notification_card_styled(&item.n, &palette, &style);
            let hover_view = this.clone();
            let press_view = this.clone();
            let move_view = this.clone();
            let swipe = item.swipe.motion.value();
            let swipe_fade = 1.0 - (swipe.abs() / w).min(1.0) * SWIPE_FADE;
            toasts.push(
                div()
                    .id(("toast", id as usize))
                    .absolute()
                    .left(px(left + swipe))
                    .top(px(top))
                    .w(px(w))
                    .opacity(p * swipe_fade)
                    // A press may become a swipe (followed by the listeners below).
                    .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, _, cx| {
                        let x = f32::from(e.position.x);
                        let _ = press_view.update(cx, |v, cx| {
                            if let Some(item) = v.items.iter_mut().find(|i| i.n.id == id && !i.leaving) {
                                item.swipe.press(x, f32::from(e.position.y));
                                cx.notify();
                            }
                        });
                    })
                    .occlude()
                    // The card's hover fill, image and progress bar stay inside the corners.
                    .clip_rounded()
                    .rounded(px(crate::style::radius(TOAST_RADIUS_PX * scale)))
                    // Joined, the drawer is the backdrop.
                    .when(drawer.is_none(), |el| {
                        el.bg(fill).shadow(vec![BoxShadow {
                            color: gpui::black().opacity(SHADOW_ALPHA),
                            offset: point(px(0.0), px(SHADOW_Y_PX)),
                            blur_radius: px(SHADOW_BLUR_PX),
                            spread_radius: px(0.0),
                            inset: false,
                        }])
                    })
                    .when(ncfg.pause_on_hover, |el| {
                        el.on_hover(move |hovered: &bool, window, cx| {
                            // GPUI re-tests hover on repaint against the last
                            // pointer position, stale once the pointer left.
                            if *hovered && !window.is_window_hovered() {
                                return;
                            }
                            let hovered = *hovered;
                            let _ = hover_view.update(cx, |v, cx| {
                                let now = Instant::now();
                                if let Some(item) = v.items.iter_mut().find(|i| i.n.id == id) {
                                    match (hovered, item.hovered_since) {
                                        (true, None) => item.hovered_since = Some(now),
                                        (false, Some(since)) => {
                                            item.paused += now.saturating_duration_since(since);
                                            item.hovered_since = None;
                                        }
                                        _ => {}
                                    }
                                }
                                cx.notify();
                            });
                        })
                        // A popup that arrived under that stale position
                        // already counts as hovered for GPUI, so the real
                        // hover sends no event: the pointer moving over it
                        // starts the pause instead.
                        .on_mouse_move(move |_: &MouseMoveEvent, window, cx| {
                            if !window.is_window_hovered() {
                                return;
                            }
                            let _ = move_view.update(cx, |v, cx| {
                                if let Some(item) = v.items.iter_mut().find(|i| i.n.id == id && i.hovered_since.is_none()) {
                                    item.hovered_since = Some(Instant::now());
                                    cx.notify();
                                }
                            });
                        })
                    })
                    .child(card)
                    .when_some(progress, |el, f| {
                        el.child(
                            div()
                                .absolute()
                                .bottom(px(0.0))
                                .left(px(PROGRESS_INSET_PX * scale))
                                .right(px(PROGRESS_INSET_PX * scale))
                                .h(px(PROGRESS_H_PX))
                                .child(div().h_full().w(gpui::relative(1.0 - f)).rounded_full().bg(oklab_gradient(ANGLE_RIGHT_DEG, accent.with_alpha(PROGRESS_ALPHA), partner(accent).with_alpha(PROGRESS_ALPHA)))),
                        )
                    })
                    // Measure the laid-out height for the stack.
                    .child(
                        canvas(
                            move |bounds, window, _| {
                                let hh = f32::from(bounds.size.height);
                                let old = measured.borrow().get(&id).copied();
                                if old.is_none_or(|o| (o - hh).abs() > MEASURE_EPSILON_PX) {
                                    measured.borrow_mut().insert(id, hh);
                                    let entity = entity.clone();
                                    window.on_next_frame(move |_, cx| entity.update(cx, |_, cx| cx.notify()));
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
                    .when(!item.leaving, |el| el.child(self.mask.region()))
                    .into_any_element(),
            );
        }
        root = match drawer {
            Some((d, x, y, dw, reach)) if reach > MIN_DRAWER_PX => {
                // Overlap an opaque bar by a pixel: no seam.
                let seam = if d.on_bar && cfg.bar.opacity >= OPAQUE_ALPHA && !cfg.bar.frame_floats() { 1.0 } else { 0.0 };
                let radius = crate::style::radius(TOAST_RADIUS_PX * scale) + pad;
                let (edge, shape_y) = if bottom { (Edge::Bottom, y) } else { (Edge::Top, y - seam) };
                let shape = edge_drawer_snapped(edge, x, shape_y, dw, reach + seam, radius, d.flare, fill, window.scale_factor());
                // The popups are clipped to the drawer, so they come out
                // of the bar's edge and swipe out of the drawer's side.
                let body = div().absolute().left(px(x)).top(px(y)).w(px(dw)).h(px(reach)).clip_rounded();
                let body = if bottom { body.rounded_t(px(radius)) } else { body.rounded_b(px(radius)) };
                root.child(shape).child(body.child(self.mask.region()).children(toasts))
            }
            Some(_) => root,
            None => root.children(toasts),
        };
        // The button is down on a toast: follow the pointer wherever it goes
        // (the surface keeps it until release) and settle the swipe then.
        if let Some(id) = self.items.iter().find(|i| i.swipe.pressed()).map(|i| i.n.id) {
            let (follow, settle) = (this.clone(), this.clone());
            root = root.child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                            if phase != DispatchPhase::Bubble {
                                return;
                            }
                            let x = f32::from(e.position.x);
                            let _ = follow.update(cx, |v, cx| {
                                if v.items.iter_mut().find(|i| i.n.id == id).is_some_and(|i| i.drag_to(x, f32::from(e.position.y))) {
                                    cx.notify();
                                }
                            });
                        });
                        window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                            if phase != DispatchPhase::Capture || e.button != MouseButton::Left {
                                return;
                            }
                            let swiped = settle
                                .update(cx, |v, cx| {
                                    let item = v.items.iter_mut().find(|i| i.n.id == id);
                                    let outcome = item.and_then(|i| {
                                        let away = i.release(w);
                                        i.expired |= away == Some(true);
                                        away
                                    });
                                    if outcome == Some(true) {
                                        crate::plugins::notifications::send(cx, NotificationCommand::Expire(id));
                                    }
                                    cx.notify();
                                    outcome.is_some()
                                })
                                .unwrap_or(false);
                            // A swipe is not a click on the toast.
                            if swiped {
                                cx.stop_propagation();
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            );
        }
        self.heights.borrow_mut().retain(|id, _| self.items.iter().any(|i| i.n.id == *id));

        // Wake once for the next expiry (re-armed only when it changes).
        let next = self.items.iter().filter_map(|i| i.due(now)).min();
        match next {
            Some(due) if self.timer.as_ref().is_none_or(|(at, _)| *at != due) => {
                let wait = due.saturating_duration_since(now) + WAKE_SLACK;
                let task = cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    let _ = this.update(cx, |this, cx| {
                        this.timer = None;
                        cx.notify();
                    });
                });
                self.timer = Some((due, task));
            }
            Some(_) => {}
            None => self.timer = None,
        }
        root.child(self.mask.commit()).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_the_list_before_its_open_echo_queues_both_transitions() {
        let (sender, receiver) = async_channel::bounded(2);
        let mut requested = Some(false);
        queue_list_visibility(&mut requested, true, |open| sender.try_send(open).is_ok());
        // The actor has not consumed the open request or published an echo.
        queue_list_visibility(&mut requested, false, |open| sender.try_send(open).is_ok());
        assert!(receiver.try_recv().unwrap());
        assert!(!receiver.try_recv().unwrap());
        queue_list_visibility(&mut requested, false, |_| panic!("the queued close must be deduplicated"));
    }

    #[test]
    fn a_failed_visibility_write_stays_retryable() {
        let (sender, receiver) = async_channel::bounded(1);
        let mut requested = Some(false);
        queue_list_visibility(&mut requested, true, |open| sender.try_send(open).is_ok());
        queue_list_visibility(&mut requested, false, |open| sender.try_send(open).is_ok());
        assert_eq!(requested, Some(true));
        assert!(receiver.try_recv().unwrap());
        queue_list_visibility(&mut requested, false, |open| sender.try_send(open).is_ok());
        assert!(!receiver.try_recv().unwrap());
        sender.close();
        queue_list_visibility(&mut requested, true, |open| sender.try_send(open).is_ok());
        assert_eq!(requested, Some(false));
    }

    /// Child weights and tabular numerals must preserve the configured popup font family.
    #[test]
    fn popup_text_inherits_the_configured_panel_font_at_every_weight() {
        use gpui::Refineable as _;

        let fonts = crate::fonts::Fonts {
            ui: "UI font".into(), display: "Display font".into(), arabic: "Arabic font".into(),
            mono: "Mono font".into(), bar: "Bar font".into(), panels: "Popup font".into(),
            studio: "Studio font".into(), widgets: "Widget font".into(),
            scripture: "Scripture font".into(), hadith: "Hadith font".into(), emoji: None,
        };
        let mut root = popup_root(fonts);
        for weight in [gpui::FontWeight::NORMAL, gpui::FontWeight::MEDIUM, gpui::FontWeight::SEMIBOLD] {
            let child = gpui::StyleRefinement::default().font_weight(weight).font_features(crate::fonts::tabular_nums());
            let mut inherited = gpui::TextStyle::default();
            inherited.refine(&root.style().text);
            inherited.refine(&child.text);
            assert_eq!(inherited.font_family.as_ref(), "Popup font");
            assert_eq!(inherited.font_weight, weight);
            assert_eq!(inherited.font_features, crate::fonts::tabular_nums());
        }
    }

    fn item(display_ms: u32, shown: Instant) -> Item {
        Item::new(Notification { id: 1, display_ms, ..Default::default() }, shown, true)
    }

    /// #24: clicking a toast raises its app, the pointer leaves without a
    /// hover-out, and the countdown must run again.
    #[test]
    fn a_pause_ends_when_the_pointer_is_gone() {
        let t0 = Instant::now();
        let mut i = item(4000, t0);
        i.hovered_since = Some(t0 + Duration::from_secs(1));
        let t2 = t0 + Duration::from_secs(2);
        i.resume(t2);
        assert!(i.hovered_since.is_none());
        assert_eq!(i.due(t2), Some(t2 + Duration::from_secs(3)), "the second hovered doesn't count");
    }

    #[test]
    fn a_swipe_past_a_third_hides_and_a_short_one_springs_back() {
        let width = 360.0;
        let mut i = item(4000, Instant::now());
        // A click stays a click.
        i.swipe.press(100.0, 0.0);
        assert!(!i.drag_to(100.0 + crate::swipe::SLOP_PX / 2.0, 0.0));
        assert_eq!(i.release(width), None);
        // A short swipe springs back.
        i.swipe.press(100.0, 0.0);
        assert!(i.drag_to(160.0, 0.0));
        assert_eq!(i.swipe.motion.value(), 60.0, "follows the pointer 1:1");
        assert_eq!(i.release(width), Some(false));
        assert_eq!(i.swipe.motion.target(), 0.0);
        // Past the threshold, either way, it flies off that way.
        i.swipe.motion.snap_to(0.0);
        i.swipe.press(100.0, 0.0);
        assert!(i.drag_to(100.0 - width * 0.5, 0.0));
        assert_eq!(i.release(width), Some(true));
        assert!(i.swipe.motion.target() < -width);
    }

    #[test]
    fn countdown_pauses_while_hovered() {
        let t0 = Instant::now();
        let mut i = item(4000, t0);
        let t1 = t0 + Duration::from_secs(1);
        assert!((i.elapsed_fraction(t1).unwrap() - 0.25).abs() < 1e-3);
        i.hovered_since = Some(t1);
        let t2 = t0 + Duration::from_secs(3);
        assert!((i.elapsed_fraction(t2).unwrap() - 0.25).abs() < 1e-3, "paused while hovered");
        assert!(i.due(t2).is_none());
        i.paused += t2 - t1;
        i.hovered_since = None;
        assert_eq!(i.due(t2), Some(t2 + Duration::from_secs(3)));
        // Sticky toasts never count down.
        assert!(item(0, t0).elapsed_fraction(t2).is_none());
    }

    fn output(name: &str, index: usize) -> PopupOutput {
        PopupOutput { name: name.into(), width: 2560.0, height: 1440.0, index }
    }

    #[test]
    fn only_a_dock_on_the_popups_output_holds_their_bottom_edge() {
        use mandara_config::outputs::{ScreenChoice, ScreenSet};
        use mandara_config::workflow::{DOCK_PLUGIN_ID, DockPosition};
        let mut cfg = ShellConfig::default();
        cfg.plugins.enabled.push(DOCK_PLUGIN_ID.into());
        cfg.dock.position = DockPosition::Bottom;
        cfg.displays.surfaces.dock = ScreenSet { show_on: ScreenChoice::Selected, outputs: vec!["DP-1".into()] };
        assert!(dock_holds_bottom(&cfg, Some(&output("DP-1", 0))));
        assert!(!dock_holds_bottom(&cfg, Some(&output("DP-2", 1))), "the dock is on the other display");
        assert!(dock_holds_bottom(&cfg, None), "output unknown: assume the dock is there");
        cfg.dock.position = DockPosition::Left;
        assert!(!dock_holds_bottom(&cfg, Some(&output("DP-1", 0))));
        // Joined bottom popups on DP-2 hang from its free bottom edge.
        cfg.dock.position = DockPosition::Bottom;
        cfg.notifications.joined = true;
        cfg.notifications.position = ToastPosition::BottomRight;
        assert!(joined_drawer(&cfg, Some(&output("DP-2", 1)), 0.0).is_some());
        assert!(joined_drawer(&cfg, Some(&output("DP-1", 0)), 0.0).is_none());
    }

    #[test]
    fn joined_popups_open_on_the_output_they_are_shaped_for() {
        assert_eq!(surface_display(true, || Some(2), || None), Some(2));
        assert_eq!(surface_display(true, || None, || Some(1)), Some(1), "that output has no display yet");
        assert_eq!(surface_display(false, || Some(2), || None::<u32>), None, "cards: the compositor's pick");
    }

    #[test]
    fn a_narrow_output_keeps_joined_popups_as_cards() {
        let mut cfg = ShellConfig::default();
        cfg.notifications.joined = true;
        cfg.bar.layout = mandara_config::BarLayout::Notch;
        let narrow = PopupOutput { width: 320.0, ..output("DP-1", 0) };
        assert!(joined_drawer(&cfg, Some(&narrow), 0.0).is_none());
        assert!(joined_drawer(&cfg, Some(&output("DP-1", 0)), 0.0).is_some());
    }

    #[test]
    fn surface_specs_follow_position() {
        let key = SurfaceKey { position: ToastPosition::BottomLeft, overlay: true, size: (400.0, 600.0), display: None, pull: 0.0, cards: 3 };
        let spec = key.spec();
        assert_eq!(spec.anchor, Anchor::BOTTOM | Anchor::LEFT);
        assert_eq!(spec.layer, Layer::Overlay);
        let centre = SurfaceKey { position: ToastPosition::TopCenter, overlay: false, ..key };
        assert_eq!(centre.spec().anchor, Anchor::TOP);
        assert_eq!(centre.spec().layer, Layer::Top);
        // Joined: the surface reaches back over the bar's band on its edge.
        let joined = SurfaceKey { position: ToastPosition::TopRight, pull: 36.0, ..centre.clone() };
        assert_eq!(joined.spec().margin.0, px(-36.0));
        let bottom = SurfaceKey { position: ToastPosition::BottomCenter, ..joined };
        assert_eq!(bottom.spec().margin.2, px(-36.0));
        assert_eq!(bottom.spec().margin.0, px(0.0));
    }

    #[test]
    fn a_short_display_shows_only_the_popups_that_fit() {
        let card = 100.0;
        // Room for everything: the setting wins.
        assert_eq!(cards_that_fit(3, card, 2000.0), 3);
        // A short display: only what fits the cap, never more.
        let cap = EDGE_GAP_PX * 2.0 + card * 2.0 + 10.0;
        assert_eq!(cards_that_fit(5, card, cap), 2);
        // Always at least one, and a zero setting still shows one.
        assert_eq!(cards_that_fit(4, card, 10.0), 1);
        assert_eq!(cards_that_fit(0, card, 2000.0), 1);
    }
}
