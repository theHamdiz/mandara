//! Mandara patch (touchscreen input, theHamdiz/mandara#130): turns the
//! `wl_touch` stream into GPUI's raw [`TouchEvent`]s.
//!
//! GPUI core already recognizes taps (synthesized left press + release, so
//! `on_click` fires), pans (touch scrolling with momentum), long presses and
//! claimed touch drags from `PlatformInput::Touch`. Upstream's Wayland client
//! never asked the seat for `wl_touch`, so nothing fed it. This module is the
//! protocol-to-GPUI half, kept free of Wayland objects so it can be tested:
//!
//! * a Wayland touch id is only unique while the finger is down and the
//!   compositor reuses it for the next contact, while GPUI requires a
//!   [`TouchId`] that is never reused; every `down` gets a fresh one;
//! * `motion` and `up` carry no surface: a point stays with the window its
//!   `down` landed on, in that surface's coordinates;
//! * `up` carries no position: the point ends where it last was;
//! * motions are coalesced until the compositor's `frame`, so a fast
//!   digitizer produces one GPUI event per point per frame, flushed before
//!   that point's `up`;
//! * `cancel` (the compositor took the touch sequence, e.g. for a gesture)
//!   ends every point as [`TouchPhase::Cancelled`], which GPUI treats as "no
//!   tap", and so does losing the touch capability or the seat.
//!
//! The pointer's own state (focus, position, pressed button, hover) is never
//! touched: a tap is not a pointer enter, and later mouse use is unchanged.

use gpui::{Pixels, Point, TouchEvent, TouchId, TouchPhase, point, px};
use smallvec::SmallVec;

/// Touch points tracked without a heap allocation. Touchscreens report up
/// to ten contacts; beyond four this spills once and stays bounded by the
/// hardware.
const INLINE_POINTS: usize = 4;

/// The events one Wayland touch event produces, each for the window it
/// belongs to, ready for `handle_input` once the client state is released.
pub(crate) type TouchDispatches<W> = SmallVec<[(W, TouchEvent); INLINE_POINTS]>;

/// A `wl_touch` surface-local position in GPUI's logical window pixels.
///
/// Surface-local coordinates are logical (pre-scale) units, the space GPUI
/// lays windows out in, with the fractional part kept: at a 1.5× output a
/// touch at device pixel 151 arrives as 100.67 and must stay there, exactly
/// like `wl_pointer` positions. Layer-shell surfaces and xdg toplevels use
/// the same space.
pub(crate) fn touch_position(x: f64, y: f64) -> Point<Pixels> {
    point(px(x as f32), px(y as f32))
}

struct ActivePoint<W> {
    /// The compositor's id, valid from `down` to `up`/`cancel`.
    wayland_id: i32,
    /// GPUI's id for this contact, never reused.
    id: TouchId,
    window: W,
    position: Point<Pixels>,
    /// Moved since the last `frame` (a `Moved` event is due).
    moved: bool,
}

pub(crate) struct TouchPoints<W> {
    points: SmallVec<[ActivePoint<W>; INLINE_POINTS]>,
    last_id: u64,
}

impl<W: Clone> TouchPoints<W> {
    pub(crate) fn new() -> Self {
        Self {
            points: SmallVec::new(),
            last_id: 0,
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.points.len()
    }

    fn event(point: &ActivePoint<W>, phase: TouchPhase) -> (W, TouchEvent) {
        (
            point.window.clone(),
            TouchEvent {
                id: point.id,
                phase,
                position: point.position,
                predicted_position: None,
                force: None,
            },
        )
    }

    /// A finger touched `window` at `position` (`wl_touch.down`).
    pub(crate) fn down(
        &mut self,
        wayland_id: i32,
        window: W,
        position: Point<Pixels>,
        out: &mut TouchDispatches<W>,
    ) {
        // A compositor must end a point before reusing its id; if one never
        // did, end the stale contact instead of letting it linger.
        if let Some(index) = self.index(wayland_id) {
            let stale = self.points.swap_remove(index);
            out.push(Self::event(&stale, TouchPhase::Cancelled));
        }
        self.last_id = self.last_id.wrapping_add(1);
        let point = ActivePoint {
            wayland_id,
            id: TouchId(self.last_id),
            window,
            position,
            moved: false,
        };
        out.push(Self::event(&point, TouchPhase::Started));
        self.points.push(point);
    }

    /// A finger moved (`wl_touch.motion`); reported at the next `frame`.
    pub(crate) fn motion(&mut self, wayland_id: i32, position: Point<Pixels>) {
        if let Some(index) = self.index(wayland_id) {
            let point = &mut self.points[index];
            if point.position != position {
                point.position = position;
                point.moved = true;
            }
        }
    }

    /// The end of one hardware report (`wl_touch.frame`): one `Moved` per
    /// point that moved during it.
    pub(crate) fn frame(&mut self, out: &mut TouchDispatches<W>) {
        for point in &mut self.points {
            if std::mem::take(&mut point.moved) {
                out.push(Self::event(point, TouchPhase::Moved));
            }
        }
    }

    /// A finger lifted (`wl_touch.up`), where it last was.
    pub(crate) fn up(&mut self, wayland_id: i32, out: &mut TouchDispatches<W>) {
        let Some(index) = self.index(wayland_id) else {
            return;
        };
        let point = self.points.swap_remove(index);
        if point.moved {
            out.push(Self::event(&point, TouchPhase::Moved));
        }
        out.push(Self::event(&point, TouchPhase::Ended));
    }

    /// The compositor took the sequence (`wl_touch.cancel`), or touch input
    /// went away: every point ends without completing a tap.
    pub(crate) fn cancel(&mut self, out: &mut TouchDispatches<W>) {
        for point in self.points.drain(..) {
            out.push(Self::event(&point, TouchPhase::Cancelled));
        }
    }

    /// Drops the points on a window that is closing, without events.
    pub(crate) fn forget_window(&mut self, is_window: impl Fn(&W) -> bool) {
        self.points.retain(|point| !is_window(&point.window));
    }

    fn index(&self, wayland_id: i32) -> Option<usize> {
        self.points
            .iter()
            .position(|point| point.wayland_id == wayland_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows are plain names in these tests.
    type Points = TouchPoints<&'static str>;

    fn phases(
        out: &TouchDispatches<&'static str>,
    ) -> Vec<(&'static str, u64, TouchPhase, f32, f32)> {
        out.iter()
            .map(|(window, event)| {
                (
                    *window,
                    event.id.0,
                    event.phase,
                    f32::from(event.position.x),
                    f32::from(event.position.y),
                )
            })
            .collect()
    }

    fn take(
        out: &mut TouchDispatches<&'static str>,
    ) -> Vec<(&'static str, u64, TouchPhase, f32, f32)> {
        let events = phases(out);
        out.clear();
        events
    }

    #[test]
    fn tap_starts_and_ends_where_the_finger_lifted() {
        let mut points = Points::new();
        let mut out = TouchDispatches::new();
        points.down(0, "bar", touch_position(40.0, 12.0), &mut out);
        points.frame(&mut out);
        assert_eq!(
            take(&mut out),
            [("bar", 1, TouchPhase::Started, 40.0, 12.0)]
        );
        points.up(0, &mut out);
        points.frame(&mut out);
        assert_eq!(take(&mut out), [("bar", 1, TouchPhase::Ended, 40.0, 12.0)]);
        assert_eq!(points.len(), 0);
    }

    #[test]
    fn motions_are_coalesced_per_frame_and_flushed_before_up() {
        let mut points = Points::new();
        let mut out = TouchDispatches::new();
        points.down(3, "studio", touch_position(10.0, 10.0), &mut out);
        take(&mut out);
        points.motion(3, touch_position(12.0, 10.0));
        points.motion(3, touch_position(15.0, 11.0));
        assert!(out.is_empty(), "nothing before the frame");
        points.frame(&mut out);
        assert_eq!(
            take(&mut out),
            [("studio", 1, TouchPhase::Moved, 15.0, 11.0)]
        );
        // A frame without motion reports nothing.
        points.frame(&mut out);
        assert!(out.is_empty());
        // A motion and the lift in one frame: the move lands first, and the
        // release is at the final position.
        points.motion(3, touch_position(30.0, 11.0));
        points.up(3, &mut out);
        points.frame(&mut out);
        assert_eq!(
            take(&mut out),
            [
                ("studio", 1, TouchPhase::Moved, 30.0, 11.0),
                ("studio", 1, TouchPhase::Ended, 30.0, 11.0),
            ]
        );
    }

    #[test]
    fn a_motion_to_the_same_position_is_not_an_event() {
        let mut points = Points::new();
        let mut out = TouchDispatches::new();
        points.down(0, "bar", touch_position(5.0, 5.0), &mut out);
        take(&mut out);
        points.motion(0, touch_position(5.0, 5.0));
        points.frame(&mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn cancel_ends_every_point_without_ending_any_as_a_tap() {
        let mut points = Points::new();
        let mut out = TouchDispatches::new();
        points.down(0, "bar", touch_position(1.0, 2.0), &mut out);
        points.down(1, "panel", touch_position(3.0, 4.0), &mut out);
        take(&mut out);
        points.motion(1, touch_position(9.0, 4.0));
        points.cancel(&mut out);
        let events = take(&mut out);
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| e.2 == TouchPhase::Cancelled));
        assert!(events.contains(&("bar", 1, TouchPhase::Cancelled, 1.0, 2.0)));
        assert!(events.contains(&("panel", 2, TouchPhase::Cancelled, 9.0, 4.0)));
        // Nothing is left to end later.
        points.up(0, &mut out);
        points.up(1, &mut out);
        points.frame(&mut out);
        assert!(out.is_empty());
        assert_eq!(points.len(), 0);
    }

    #[test]
    fn points_keep_their_own_window_and_ids_are_never_reused() {
        let mut points = Points::new();
        let mut out = TouchDispatches::new();
        points.down(0, "bar", touch_position(1.0, 1.0), &mut out);
        points.down(1, "studio", touch_position(2.0, 2.0), &mut out);
        points.motion(0, touch_position(1.5, 1.0));
        points.motion(1, touch_position(2.5, 2.0));
        points.frame(&mut out);
        let events = take(&mut out);
        assert_eq!(
            events,
            [
                ("bar", 1, TouchPhase::Started, 1.0, 1.0),
                ("studio", 2, TouchPhase::Started, 2.0, 2.0),
                ("bar", 1, TouchPhase::Moved, 1.5, 1.0),
                ("studio", 2, TouchPhase::Moved, 2.5, 2.0),
            ]
        );
        points.up(0, &mut out);
        points.up(1, &mut out);
        take(&mut out);
        // The compositor reuses wayland id 0 for the next finger; GPUI gets
        // a new id so the recognizer can't mistake it for the old contact.
        points.down(0, "bar", touch_position(7.0, 7.0), &mut out);
        assert_eq!(take(&mut out), [("bar", 3, TouchPhase::Started, 7.0, 7.0)]);
    }

    #[test]
    fn unknown_ids_are_ignored() {
        let mut points = Points::new();
        let mut out = TouchDispatches::new();
        // A point that went down on a surface that isn't a GPUI window was
        // never tracked: its motion and lift do nothing.
        points.motion(9, touch_position(1.0, 1.0));
        points.up(9, &mut out);
        points.frame(&mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn a_reused_id_without_up_cancels_the_stale_contact() {
        let mut points = Points::new();
        let mut out = TouchDispatches::new();
        points.down(0, "bar", touch_position(1.0, 1.0), &mut out);
        take(&mut out);
        points.down(0, "bar", touch_position(50.0, 1.0), &mut out);
        assert_eq!(
            take(&mut out),
            [
                ("bar", 1, TouchPhase::Cancelled, 1.0, 1.0),
                ("bar", 2, TouchPhase::Started, 50.0, 1.0),
            ]
        );
        assert_eq!(points.len(), 1);
    }

    #[test]
    fn a_closing_window_forgets_its_points() {
        let mut points = Points::new();
        let mut out = TouchDispatches::new();
        points.down(0, "studio", touch_position(1.0, 1.0), &mut out);
        points.down(1, "bar", touch_position(2.0, 2.0), &mut out);
        take(&mut out);
        points.forget_window(|window| *window == "studio");
        points.motion(0, touch_position(3.0, 3.0));
        points.up(0, &mut out);
        assert!(out.is_empty(), "no event reaches a closed window");
        points.up(1, &mut out);
        assert_eq!(take(&mut out), [("bar", 2, TouchPhase::Ended, 2.0, 2.0)]);
    }

    /// End to end below Wayland: what [`TouchPoints`] emits, dispatched into
    /// a real GPUI window the way `dispatch_touch` does, clicks a button
    /// exactly when a tap lands on it.
    mod gpui_window {
        use super::super::*;
        use gpui::{
            AnyWindowHandle, AppContext as _, Context, InteractiveElement as _, IntoElement,
            ParentElement as _, PlatformInput, Render, StatefulInteractiveElement as _,
            Styled as _, TestAppContext, Window, div,
        };
        use std::{cell::RefCell, rc::Rc};

        struct Button {
            clicks: Rc<RefCell<Vec<(f32, f32)>>>,
        }

        impl Render for Button {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let clicks = self.clicks.clone();
                div().size_full().child(
                    div()
                        .id("button")
                        .absolute()
                        .left(px(100.))
                        .top(px(50.))
                        .w(px(40.))
                        .h(px(20.))
                        .on_click(move |event, _, _| {
                            let position = event.position();
                            clicks
                                .borrow_mut()
                                .push((f32::from(position.x), f32::from(position.y)));
                        }),
                )
            }
        }

        fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Rc<RefCell<Vec<(f32, f32)>>>) {
            let clicks = Rc::new(RefCell::new(Vec::new()));
            let window: AnyWindowHandle = cx
                .add_window({
                    let clicks = clicks.clone();
                    move |_, _| Button { clicks }
                })
                .into();
            cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
                .unwrap();
            (window, clicks)
        }

        /// What `dispatch_touch` does with a window's `handle_input`.
        fn dispatch(
            window: AnyWindowHandle,
            cx: &mut TestAppContext,
            out: &mut TouchDispatches<()>,
        ) {
            for ((), event) in out.drain(..) {
                cx.update_window(window, |_, window, cx| {
                    window.dispatch_event(PlatformInput::Touch(event), cx);
                })
                .unwrap();
            }
        }

        #[gpui::test]
        fn a_tap_on_a_button_clicks_it_where_the_finger_lifted(cx: &mut TestAppContext) {
            let (window, clicks) = open(cx);
            let mut points = TouchPoints::new();
            let mut out = TouchDispatches::new();
            points.down(0, (), touch_position(110.5, 60.25), &mut out);
            points.frame(&mut out);
            dispatch(window, cx, &mut out);
            assert!(
                clicks.borrow().is_empty(),
                "no click while the finger is down"
            );
            // A jitter within the touch slop is still a tap.
            points.motion(0, touch_position(112.0, 61.0));
            points.frame(&mut out);
            points.up(0, &mut out);
            points.frame(&mut out);
            dispatch(window, cx, &mut out);
            assert_eq!(clicks.borrow().as_slice(), [(112.0, 61.0)]);
        }

        #[gpui::test]
        fn a_tap_elsewhere_a_swipe_and_a_cancel_click_nothing(cx: &mut TestAppContext) {
            let (window, clicks) = open(cx);
            let mut points = TouchPoints::new();
            let mut out = TouchDispatches::new();
            // Beside the button.
            points.down(0, (), touch_position(20.0, 20.0), &mut out);
            points.up(0, &mut out);
            dispatch(window, cx, &mut out);
            // A swipe that starts on the button scrolls instead.
            points.down(0, (), touch_position(110.0, 60.0), &mut out);
            points.motion(0, touch_position(110.0, 120.0));
            points.frame(&mut out);
            points.up(0, &mut out);
            dispatch(window, cx, &mut out);
            // The compositor takes a touch on the button.
            points.down(1, (), touch_position(110.0, 60.0), &mut out);
            points.cancel(&mut out);
            dispatch(window, cx, &mut out);
            assert!(clicks.borrow().is_empty(), "{:?}", clicks.borrow());
            // The next real tap still works.
            points.down(1, (), touch_position(120.0, 55.0), &mut out);
            points.up(1, &mut out);
            dispatch(window, cx, &mut out);
            assert_eq!(clicks.borrow().as_slice(), [(120.0, 55.0)]);
        }
    }

    #[test]
    fn fractional_scale_positions_keep_their_fraction() {
        // At 1.5× a touch on device pixel 151 × 75 is surface-local
        // 100.666… × 50 (wl_fixed carries 1/256 px). GPUI windows are laid
        // out in those logical pixels, so the position must pass through
        // unrounded, at every scale, exactly like the pointer's.
        for scale in [1.0_f64, 1.25, 1.5, 2.0] {
            let (device_x, device_y) = (151.0_f64, 75.0_f64);
            let wl_fixed = |v: f64| (v * 256.0).round() / 256.0;
            let (x, y) = (wl_fixed(device_x / scale), wl_fixed(device_y / scale));
            let position = touch_position(x, y);
            assert!(
                (f64::from(f32::from(position.x)) - x).abs() < 1e-4,
                "x at {scale}"
            );
            assert!(
                (f64::from(f32::from(position.y)) - y).abs() < 1e-4,
                "y at {scale}"
            );
            // Back to device pixels within a 1/256 px wl_fixed step.
            assert!((f64::from(f32::from(position.x)) * scale - device_x).abs() <= scale / 256.0);
        }
        let mut points = Points::new();
        let mut out = TouchDispatches::new();
        points.down(0, "bar", touch_position(100.6640625, 50.25), &mut out);
        assert_eq!(
            take(&mut out),
            [("bar", 1, TouchPhase::Started, 100.6640625, 50.25)]
        );
    }
}
