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

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use gpui::{
    App, AppContext as _, BoxShadow, Context, Entity, Global, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement as _, Styled, Task, Window, WindowHandle, canvas, div, point,
    prelude::FluentBuilder as _, px,
};
use mandara_config::alerts::{NotificationsConfig, ToastPosition};
use mandara_config::outputs::{SurfaceKind, assignment};
use mandara_core::{FrameClock, InputMaskRegistry, LayerSurfaceSpec, SpringConfig, SpringValue, SurfaceRole, ToGpui, oklab_gradient};
use mandara_services::{Notification, NotificationCommand};

use crate::plugins::notifications::{CardStyle, notification_card_styled};
use crate::plugins::panel_fx::{partner, readable};
use crate::state::config;
use crate::style::ClipRounded as _;

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

/// Which output a transient surface (toasts, OSD) opens on: `None` lets the
/// compositor pick (the focused output) when every output may show it or
/// the focused one is allowed; otherwise the first allowed output.
pub fn target_display(kind: SurfaceKind, cx: &App) -> Option<gpui::DisplayId> {
    let outputs = cx.try_global::<crate::overlays::Outputs>()?.0.clone();
    let cfg = config(cx).read(cx).snapshot();
    let allowed: Vec<usize> = outputs.iter().enumerate().filter(|(_, o)| assignment(&cfg.displays, kind, &o.name)).map(|(i, _)| i).collect();
    if allowed.len() == outputs.len() || allowed.is_empty() {
        return None;
    }
    let focused = crate::hub::hub(cx).read(cx).focused.as_ref().map(|c| c.monitor.clone());
    let pick = focused.and_then(|m| allowed.iter().copied().find(|&i| outputs[i].name == m)).or_else(|| allowed.first().copied())?;
    cx.displays().get(pick).map(|d| d.id())
}

/// What the surface was opened with; a change reopens it.
#[derive(Clone, Debug, PartialEq)]
struct SurfaceKey {
    position: ToastPosition,
    overlay: bool,
    size: (f32, f32),
    display: Option<gpui::DisplayId>,
}

impl SurfaceKey {
    fn of(cfg: &NotificationsConfig, cx: &App) -> Self {
        let scale = cfg.scale.clamp(crate::plugins::notifications::TOAST_SCALE.0, crate::plugins::notifications::TOAST_SCALE.1);
        let w = cfg.width * scale + (EDGE_GAP_PX + SHADOW_ROOM_PX) * 2.0;
        let output_h = cx.try_global::<crate::overlays::Outputs>().and_then(|o| o.0.first().map(|g| g.height as f32)).unwrap_or(FALLBACK_OUTPUT_H);
        let h = (cfg.max_visible.max(1) as f32 * (TOAST_MAX_H_PX * scale + STACK_GAP_PX) + EDGE_GAP_PX * 2.0).min(output_h * MAX_SURFACE_SHARE);
        Self { position: cfg.position, overlay: cfg.over_fullscreen, size: (w.round(), h.round()), display: target_display(SurfaceKind::Notifications, cx) }
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
            margin: (px(0.0), px(0.0), px(0.0), px(0.0)),
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
}

#[derive(Clone)]
struct GlobalToasts(Entity<Toasts>);
impl Global for GlobalToasts {}

/// Opens / reopens / leaves the surface to match the popups and settings.
fn sync(cx: &mut App) {
    let Some(host) = cx.try_global::<GlobalToasts>().map(|g| g.0.clone()) else { return };
    let any_popup = crate::hub::hub(cx).read(cx).notifications.items.iter().any(|n| n.popup);
    let cfg = config(cx).read(cx).get().notifications.clone();
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
    let key = SurfaceKey::of(&cfg, cx);
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

/// Tells the notification service whether its list is on screen, so the
/// popups hand over to it (and new ones go straight into it) while it is.
pub fn sync_list(cx: &mut App) {
    let open = list_on_screen(cx);
    if crate::hub::hub(cx).read(cx).notifications.list_open != open {
        crate::plugins::notifications::send(cx, NotificationCommand::ListOpen(open));
    }
}

/// Installs the popup surface; it follows the hub and the config.
pub fn init(cx: &mut App) {
    let host = cx.new(|_| Toasts { window: None, key: None });
    cx.set_global(GlobalToasts(host));
    let hub = crate::hub::hub(cx);
    let mut last: Vec<u32> = Vec::new();
    cx.observe(&hub, move |hub, cx| {
        let popups: Vec<u32> = hub.read(cx).notifications.items.iter().filter(|n| n.popup).map(|n| n.id).collect();
        if popups != last {
            last = popups;
            sync(cx);
        }
    })
    .detach();
    let mut list_was_open = false;
    cx.observe(&crate::state::shell(cx), move |_, cx| {
        let open = list_on_screen(cx);
        if open != list_was_open {
            list_was_open = open;
            sync_list(cx);
        }
    })
    .detach();
    let mut last_cfg: Option<NotificationsConfig> = None;
    cx.observe(&config(cx), move |cfg, cx| {
        let next = cfg.read(cx).get().notifications.clone();
        if last_cfg.as_ref() != Some(&next) {
            last_cfg = Some(next);
            sync(cx);
        }
    })
    .detach();
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
    leaving: bool,
    expired: bool,
}

impl Item {
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
                let mut presence = SpringValue::new(0.0, PRESENCE_SPRING);
                presence.set_instant(reduce);
                presence.set_target(1.0);
                let mut slot = SpringValue::new(0.0, SLOT_SPRING);
                slot.set_instant(reduce);
                self.items.insert(0, Item { n: n.clone(), first_shown: now, paused: Duration::ZERO, hovered_since: None, presence, slot, leaving: false, expired: false });
            }
        }
        // Newest first, leaving ones keep their place.
        let order = |id: u32| popups.iter().position(|n| n.id == id).unwrap_or(usize::MAX);
        self.items.sort_by_key(|i| if i.leaving { usize::MAX } else { order(i.n.id) });
    }
}

impl Render for ToastView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cfg = config(cx).read(cx).snapshot();
        let ncfg = &cfg.notifications;
        let reduce = cfg.motion.reduce_motion;
        let now = Instant::now();
        let popups: Vec<Notification> = crate::hub::hub(cx).read(cx).notifications.items.iter().filter(|n| n.popup).take(ncfg.max_visible.max(1)).cloned().collect();
        self.reconcile(&popups, reduce, now);

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
        let w = ncfg.width * scale;
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
        let fill = crate::shell::frame_color(&palette);
        let fill = fill.with_alpha(fill.alpha() * ncfg.opacity.clamp(0.0, 1.0)).hsla();
        let accent = readable(palette.primary, &palette);
        let bottom = ncfg.position.is_bottom();
        let column = ncfg.position.column();
        let x0 = match column {
            -1 => EDGE_GAP_PX + SHADOW_ROOM_PX,
            1 => sw - w - EDGE_GAP_PX - SHADOW_ROOM_PX,
            _ => (sw - w) / 2.0,
        };
        let mut root = div().size_full().relative();
        let this = cx.entity().downgrade();
        for item in &self.items {
            let id = item.n.id;
            let p = item.presence.value().clamp(0.0, 1.0);
            let travel = (1.0 - p) * SLIDE_PX;
            let (dx, dy) = match column {
                -1 => (-travel, 0.0),
                1 => (travel, 0.0),
                _ => (0.0, if bottom { travel } else { -travel }),
            };
            let h = heights.get(&id).copied().unwrap_or(TOAST_GUESS_H_PX * scale);
            let y = if bottom { sh - EDGE_GAP_PX - item.slot.value() - h } else { EDGE_GAP_PX + item.slot.value() };
            let measured = self.heights.clone();
            let entity = cx.entity().clone();
            let progress = ncfg.progress_bar.then(|| item.elapsed_fraction(now)).flatten();
            let card = notification_card_styled(&item.n, &palette, &style);
            let hover_view = this.clone();
            root = root.child(
                div()
                    .id(("toast", id as usize))
                    .absolute()
                    .left(px(x0 + dx))
                    .top(px(y + dy))
                    .w(px(w))
                    .opacity(p)
                    .occlude()
                    // The card's hover fill, image and progress bar stay inside the corners.
                    .clip_rounded()
                    .rounded(px(crate::style::radius(TOAST_RADIUS_PX * scale)))
                    .bg(fill)
                    .shadow(vec![BoxShadow {
                        color: gpui::black().opacity(SHADOW_ALPHA),
                        offset: point(px(0.0), px(SHADOW_Y_PX)),
                        blur_radius: px(SHADOW_BLUR_PX),
                        spread_radius: px(0.0),
                        inset: false,
                    }])
                    .when(ncfg.pause_on_hover, |el| {
                        el.on_hover(move |hovered: &bool, _, cx| {
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
                    .when(!item.leaving, |el| el.child(self.mask.region())),
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

    fn item(display_ms: u32, shown: Instant) -> Item {
        Item {
            n: Notification { id: 1, display_ms, ..Default::default() },
            first_shown: shown,
            paused: Duration::ZERO,
            hovered_since: None,
            presence: SpringValue::new(1.0, PRESENCE_SPRING),
            slot: SpringValue::new(0.0, SLOT_SPRING),
            leaving: false,
            expired: false,
        }
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

    #[test]
    fn surface_specs_follow_position() {
        let key = SurfaceKey { position: ToastPosition::BottomLeft, overlay: true, size: (400.0, 600.0), display: None };
        let spec = key.spec();
        assert_eq!(spec.anchor, Anchor::BOTTOM | Anchor::LEFT);
        assert_eq!(spec.layer, Layer::Overlay);
        let centre = SurfaceKey { position: ToastPosition::TopCenter, overlay: false, ..key };
        assert_eq!(centre.spec().anchor, Anchor::TOP);
        assert_eq!(centre.spec().layer, Layer::Top);
    }
}
