//! One transient gesture per notification list, released when the surface changes.

use gpui::{
    AnyElement, App, AppContext as _, Context, DispatchPhase, Entity, InteractiveElement as _, IntoElement as _, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, Styled as _, WeakEntity, Window, canvas, div, px,
};
use mandara_core::FrameClock;
use mandara_services::NotificationCommand;
use std::time::Instant;

use crate::swipe::Swipe;

struct Active {
    id: u32,
    swipe: Swipe,
}

pub(super) struct Controller {
    active: Option<Active>,
    clock: FrameClock,
}

impl Controller {
    /// Own one gesture per list and cancel it when its surface or setting changes.
    pub(super) fn new(cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            cx.observe(&crate::state::shell(cx), |this: &mut Self, _, cx| {
                if this.clear() {
                    cx.notify();
                }
            })
            .detach();
            cx.observe(&crate::state::config(cx), |this: &mut Self, cfg, cx| {
                if !cfg.read(cx).get().notifications.history_swipe && this.clear() {
                    cx.notify();
                }
            })
            .detach();
            Self {
                active: None,
                clock: FrameClock::new(),
            }
        })
    }

    pub(super) fn clear(&mut self) -> bool {
        self.clock.idle();
        self.active.take().is_some()
    }

    pub(super) fn id(&self) -> Option<u32> {
        self.active.as_ref().map(|a| a.id)
    }

    /// A frame retains only the selected id and offset, never the history.
    pub(super) fn frame(owner: &Entity<Self>, alive: bool, window: &mut Window, cx: &mut App) -> Frame {
        let _motion = mandara_core::spring::MotionScope::new(mandara_config::motion::surface_speed(crate::state::config(cx).read(cx).get().motion.surfaces.notifications));
        let weak = owner.downgrade();
        owner.update(cx, |this, cx| {
            let enabled = crate::state::config(cx).read(cx).get().notifications.history_swipe;
            if !enabled || !alive {
                this.clear();
            }
            let mut id = None;
            let mut offset = 0.0;
            let mut pressed = false;
            if let Some(active) = &mut this.active {
                pressed = active.swipe.pressed();
                let moving = active.swipe.motion.step(this.clock.tick_vsync(Instant::now(), window.refresh_interval(cx)));
                if moving {
                    window.request_animation_frame();
                }
                if !pressed && !moving {
                    this.clear();
                } else {
                    id = Some(active.id);
                    offset = active.swipe.motion.value();
                }
            } else {
                this.clock.idle();
            }
            Frame {
                owner: weak,
                id,
                offset,
                pressed,
                enabled,
            }
        })
    }

    /// Start or regrab the selected card without retaining the history snapshot.
    fn press(&mut self, id: u32, event: &MouseDownEvent, cx: &mut Context<Self>) {
        if event.button != MouseButton::Left || !crate::state::config(cx).read(cx).get().notifications.history_swipe {
            return;
        }
        if self.active.as_ref().is_none_or(|a| a.id != id) {
            self.active = Some(Active { id, swipe: Swipe::default() });
        }
        if let Some(active) = &mut self.active {
            active.swipe.press(event.position.x.into(), event.position.y.into());
        }
        cx.notify();
    }
}

pub(super) struct Frame {
    owner: WeakEntity<Controller>,
    id: Option<u32>,
    offset: f32,
    pressed: bool,
    enabled: bool,
}

impl Frame {
    /// Offset only the active card and leave disabled lists without gesture handlers.
    pub(super) fn card(&self, id: u32, body: AnyElement) -> AnyElement {
        if !self.enabled {
            return body;
        }
        let owner = self.owner.clone();
        let offset = if self.id == Some(id) { self.offset } else { 0.0 };
        div()
            .id(("notification-swipe", id))
            .w_full()
            .relative()
            .capture_any_mouse_down(move |event, _, cx| {
                let _ = owner.update(cx, |s, cx| s.press(id, event, cx));
            })
            .child(div().w_full().relative().left(px(offset)).child(body))
            .into_any_element()
    }

    /// Pointer grabs keep working beyond the card; release consumes a drag so
    /// it cannot also activate a notification or one of its buttons.
    pub(super) fn listeners(&self) -> Option<AnyElement> {
        if !self.pressed {
            return None;
        }
        let (follow, release) = (self.owner.clone(), self.owner.clone());
        Some(
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }
                        let _ = follow.update(cx, |s, cx| {
                            if event.pressed_button != Some(MouseButton::Left) {
                                if s.clear() {
                                    cx.notify();
                                }
                                return;
                            }
                            if let Some(active) = &mut s.active
                                && active.swipe.drag_to(event.position.x.into(), event.position.y.into())
                            {
                                cx.notify();
                            }
                        });
                    });
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                        if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
                            return;
                        }
                        let consumed = release
                            .update(cx, |s, cx| {
                                let threshold = crate::state::config(cx).read(cx).get().notifications.history_swipe_distance();
                                let outcome = s
                                    .active
                                    .as_mut()
                                    .and_then(|a| a.swipe.release(threshold, window.viewport_size().width.into()).map(|away| (a.id, away)));
                                if let Some((id, true)) = outcome {
                                    super::send(cx, NotificationCommand::Dismiss(id));
                                }
                                cx.notify();
                                outcome.is_some()
                            })
                            .unwrap_or(false);
                        if consumed {
                            cx.stop_propagation();
                        }
                    });
                },
            )
            .absolute()
            .size_full()
            .into_any_element(),
        )
    }
}
