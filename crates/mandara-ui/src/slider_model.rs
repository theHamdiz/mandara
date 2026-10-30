//! The model behind every slider in the shell ([`crate::kit::slider`],
//! [`crate::plugins::panel_fx::big_slider`]): pointer capture, the value
//! you are dragging, and the writes that carry it to whatever owns the real
//! value (the sound server, a backlight, a local mixer).
//!
//! - **Your value while you drag.** From the press until the release the
//!   slider draws the pointer's value, never the owner's, so the echo of an
//!   older write (which a busy sound server delivers late) can't pull the
//!   fill back under the pointer. After the release it keeps your value
//!   until every write has finished and [`SETTLE`] has passed, then
//!   reconciles to the owner's real value. Changes made elsewhere (volume
//!   keys, pavucontrol, "Mute all") show at once whenever you aren't
//!   dragging.
//! - **One write in flight.** A drag produces a value per pointer event,
//!   far faster than a server answers. Each slider sends one write at a
//!   time; values that arrive meanwhile replace each other, and the latest
//!   goes out when the write in flight completes. Nothing queues up, so the
//!   server never falls behind and the last position always lands.
//! - **Capture.** Once pressed, the slider follows the pointer anywhere in
//!   the window (above, below or past its ends) until the button is
//!   released; a press on the track jumps straight to that value.
//! - **Geometry.** [`Track`] maps a pointer position to a value with the
//!   track's insets, so the drawn handle sits under the pointer. Positions
//!   are logical pixels, the space GPUI reports both bounds and pointer
//!   events in, so the mapping is the same at any output scale.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AnyWindowHandle, App, AppContext as _, DispatchPhase, ElementId, EntityId, InteractiveElement as _, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Styled as _, Window, canvas};

/// After the release, the slider keeps your value this long after its last
/// write finished: long enough for the owner to report the final value back
/// (the sound server re-reads the device after each write, slower on a busy
/// machine), short enough that a change made elsewhere shows soon after.
pub const SETTLE: Duration = Duration::from_millis(500);
/// A write that hasn't completed after this long is given up on (an owner
/// that never answers must not freeze the slider).
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(3);
/// Values closer than this are the same position: nothing new is written.
pub const SAME_VALUE: f32 = 1e-4;
/// The shortest travel a track is treated as having, in logical px (a
/// zero-width track before its first layout must not divide by zero).
const MIN_TRAVEL_PX: f32 = 1.0;
/// Extra wait after the hold expires before redrawing, so the deadline has
/// surely passed when the view re-renders.
const RECONCILE_MARGIN: Duration = Duration::from_millis(20);

// ── Geometry ─────────────────────────────────────────────────────────────

/// Where a track's values sit: value 0 is `start_inset` logical px in from
/// the reading-start edge, value 1 is `end_inset` px in from the far edge
/// (where the handle's centre is drawn at those values).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Track {
    pub start_inset: f32,
    pub end_inset: f32,
}

impl Track {
    /// Values span the whole width (a knob centred on the value).
    pub const EDGE_TO_EDGE: Self = Self { start_inset: 0.0, end_inset: 0.0 };

    /// The distance the handle travels from value 0 to value 1.
    pub fn travel(&self, width: f32) -> f32 {
        (width - self.start_inset - self.end_inset).max(MIN_TRAVEL_PX)
    }

    /// The value (`0..=1`) under a pointer at `x`, for a track at `origin`
    /// that is `width` wide; positions past either end clamp. Mirrored in
    /// right-to-left layouts (value 0 at the right).
    pub fn value_at(&self, x: f32, origin: f32, width: f32, rtl: bool) -> f32 {
        let from_start = if rtl { origin + width - x } else { x - origin };
        ((from_start - self.start_inset) / self.travel(width)).clamp(0.0, 1.0)
    }

    /// Where the handle's centre is drawn for `value` (the inverse of
    /// [`Self::value_at`]).
    pub fn position_of(&self, value: f32, origin: f32, width: f32, rtl: bool) -> f32 {
        let from_start = self.start_inset + value.clamp(0.0, 1.0) * self.travel(width);
        if rtl { origin + width - from_start } else { origin + from_start }
    }
}

// ── Drag and write model (pure) ──────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Phase {
    #[default]
    Idle,
    Dragging,
    /// Released at this instant; holding your value until it settles.
    Released(Instant),
}

/// One slider's drag state and write coalescing.
#[derive(Clone, Debug, Default)]
pub struct LiveValue {
    phase: Phase,
    /// The value you set (shown while [`Self::holding`]).
    value: f32,
    /// The value of the latest write handed out.
    sent: Option<f32>,
    /// When the write in flight was handed out.
    in_flight: Option<Instant>,
    /// The newest value that arrived while a write was in flight.
    pending: Option<f32>,
    /// When the last write completed.
    last_done: Option<Instant>,
}

fn same(a: f32, b: Option<f32>) -> bool {
    b.is_some_and(|b| (a - b).abs() < SAME_VALUE)
}

impl LiveValue {
    /// The button went down on the track at `value`: the drag starts there.
    /// Returns a value to write now, if any.
    pub fn press(&mut self, value: f32, now: Instant) -> Option<f32> {
        self.expire_write(now);
        if self.in_flight.is_none() {
            // A fresh drag always writes its first value: the owner may
            // have changed since the last one.
            self.sent = None;
        }
        self.phase = Phase::Dragging;
        self.offer(value, now)
    }

    /// The pointer moved to `value` (anywhere in the window) with the left
    /// button `held`. A move without the button ends a drag whose release
    /// was lost. Returns a value to write now, if any.
    pub fn pointer_moved(&mut self, value: f32, held: bool, now: Instant) -> Option<f32> {
        if self.phase != Phase::Dragging {
            return None;
        }
        if !held {
            self.release(now);
            return None;
        }
        self.offer(value, now)
    }

    /// The button was released: hold your value until it settles.
    pub fn release(&mut self, now: Instant) {
        if self.phase == Phase::Dragging {
            self.phase = Phase::Released(now);
        }
    }

    /// The write in flight completed. Returns the next value to write (the
    /// newest that arrived meanwhile), if any.
    pub fn write_done(&mut self, now: Instant) -> Option<f32> {
        self.last_done = Some(now);
        match self.pending.take() {
            Some(next) => {
                self.sent = Some(next);
                self.in_flight = Some(now);
                Some(next)
            }
            None => {
                self.in_flight = None;
                None
            }
        }
    }

    /// Whether you are dragging.
    pub fn dragging(&self) -> bool {
        self.phase == Phase::Dragging
    }

    /// Whether your value is shown instead of the owner's.
    pub fn holding(&self, now: Instant) -> bool {
        match self.phase {
            Phase::Idle => false,
            Phase::Dragging => true,
            Phase::Released(at) => self.writing(now) || now.saturating_duration_since(self.last_done.map_or(at, |d| d.max(at))) < SETTLE,
        }
    }

    /// The value to draw, given the owner's `server` value.
    pub fn shown(&self, server: f32, now: Instant) -> f32 {
        if self.holding(now) { self.value } else { server }
    }

    /// Your value while it is held.
    pub fn held(&self, now: Instant) -> Option<f32> {
        self.holding(now).then_some(self.value)
    }

    /// How long until the hold ends on its own (released, nothing left to
    /// write); `None` while dragging, writing or already idle.
    pub fn hold_ends_in(&self, now: Instant) -> Option<Duration> {
        let Phase::Released(at) = self.phase else { return None };
        if self.writing(now) {
            return None;
        }
        let quiet = self.last_done.map_or(at, |d| d.max(at));
        Some(SETTLE.saturating_sub(now.saturating_duration_since(quiet)))
    }

    /// Nothing held and nothing being written: the state can be dropped.
    pub fn finished(&self, now: Instant) -> bool {
        !self.holding(now) && !self.writing(now)
    }

    fn writing(&self, now: Instant) -> bool {
        self.in_flight.is_some_and(|t| now.saturating_duration_since(t) < WRITE_TIMEOUT)
    }

    fn expire_write(&mut self, now: Instant) {
        if self.in_flight.is_some() && !self.writing(now) {
            self.in_flight = None;
            self.pending = None;
        }
    }

    /// Takes `value` as yours: written now when nothing is in flight,
    /// else kept as the next write (replacing an older one).
    fn offer(&mut self, value: f32, now: Instant) -> Option<f32> {
        self.value = value;
        self.expire_write(now);
        if self.in_flight.is_some() {
            // Back at the value in flight: nothing more to write after it.
            self.pending = (!same(value, self.sent)).then_some(value);
            return None;
        }
        if same(value, self.sent) {
            return None;
        }
        self.sent = Some(value);
        self.in_flight = Some(now);
        Some(value)
    }
}

// ── Shell glue ───────────────────────────────────────────────────────────

/// A started write: `None` when it already finished (a local value set
/// synchronously), else a channel that yields once the owner has applied it
/// (a service request's reply).
pub type Write = Option<async_channel::Receiver<Result<String, String>>>;

/// Writes a slider value (`0..=1`) to its owner.
pub type Writer = Rc<dyn Fn(f32, &mut Window, &mut App) -> Write>;

/// What a slider's `on_change` may return: a [`Write`], or nothing for a
/// value applied at once (a local setting, a Studio draft).
pub trait IntoWrite {
    fn into_write(self) -> Write;
}

impl IntoWrite for () {
    fn into_write(self) -> Write {
        None
    }
}

impl IntoWrite for Write {
    fn into_write(self) -> Write {
        self
    }
}

struct Slot {
    model: LiveValue,
    writer: Writer,
    /// The view that draws the slider (re-rendered as the value changes).
    view: Option<EntityId>,
    window: AnyWindowHandle,
}

thread_local! {
    /// Sliders being dragged or settling, by key (the slider's element id).
    static SLOTS: RefCell<HashMap<ElementId, Slot>> = RefCell::new(HashMap::new());
}

/// Your value for the slider `key` while it is held (dragging or settling).
pub fn held(key: &ElementId) -> Option<f32> {
    SLOTS.with(|s| s.borrow().get(key).and_then(|slot| slot.model.held(Instant::now())))
}

/// The value to draw for the slider `key`: yours while held, else `server`.
pub fn shown(key: &ElementId, server: f32) -> f32 {
    held(key).unwrap_or(server)
}

fn dragging(key: &ElementId) -> bool {
    SLOTS.with(|s| s.borrow().get(key).is_some_and(|slot| slot.model.dragging()))
}

fn notify(view: Option<EntityId>, window: &mut Window, cx: &mut App) {
    match view {
        Some(view) => cx.notify(view),
        None => window.refresh(),
    }
}

/// Hands `value` to the slot's writer and follows the write to completion.
fn write(key: &ElementId, value: f32, window: &mut Window, cx: &mut App) {
    let Some((writer, handle)) = SLOTS.with(|s| s.borrow().get(key).map(|slot| (slot.writer.clone(), slot.window))) else { return };
    match writer(value, window, cx) {
        None => write_done(key, window, cx),
        Some(reply) => {
            let key = key.clone();
            cx.spawn(async move |cx| {
                // A dropped reply (service gone) also ends the write.
                let _ = reply.recv().await;
                if cx.update_window(handle, |_, window, cx| write_done(&key, window, cx)).is_err() {
                    SLOTS.with(|s| s.borrow_mut().remove(&key));
                }
            })
            .detach();
        }
    }
}

fn write_done(key: &ElementId, window: &mut Window, cx: &mut App) {
    let Some((next, view)) = SLOTS.with(|s| s.borrow_mut().get_mut(key).map(|slot| (slot.model.write_done(Instant::now()), slot.view))) else { return };
    match next {
        Some(value) => write(key, value, window, cx),
        None => {
            notify(view, window, cx);
            schedule_reconcile(key, cx);
        }
    }
}

/// Redraws the slider once its hold has expired (so it shows the owner's
/// value again) and drops its state.
fn schedule_reconcile(key: &ElementId, cx: &mut App) {
    let Some(delay) = SLOTS.with(|s| s.borrow().get(key).and_then(|slot| slot.model.hold_ends_in(Instant::now()))) else { return };
    let key = key.clone();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(delay + RECONCILE_MARGIN).await;
        cx.update(|cx| {
            let view = SLOTS.with(|s| {
                let mut s = s.borrow_mut();
                let slot = s.get(&key)?;
                let finished = slot.model.finished(Instant::now());
                let view = slot.view;
                if finished {
                    s.remove(&key);
                }
                finished.then_some(view).flatten()
            });
            if let Some(view) = view {
                cx.notify(view);
            }
        });
    })
    .detach();
}

fn press(key: &ElementId, value: f32, view: Option<EntityId>, writer: Writer, window: &mut Window, cx: &mut App) {
    let now = Instant::now();
    let to_write = SLOTS.with(|s| {
        let mut s = s.borrow_mut();
        s.retain(|_, slot| !slot.model.finished(now));
        let slot = s.entry(key.clone()).or_insert_with(|| Slot { model: LiveValue::default(), writer: writer.clone(), view, window: window.window_handle() });
        slot.writer = writer;
        slot.view = view.or(slot.view);
        slot.window = window.window_handle();
        slot.model.press(value, now)
    });
    notify(view, window, cx);
    if let Some(v) = to_write {
        write(key, v, window, cx);
    }
}

fn pointer_moved(key: &ElementId, value: f32, held: bool, writer: Writer, window: &mut Window, cx: &mut App) {
    let now = Instant::now();
    let Some((to_write, view, released)) = SLOTS.with(|s| {
        let mut s = s.borrow_mut();
        let slot = s.get_mut(key)?;
        slot.writer = writer;
        let to_write = slot.model.pointer_moved(value, held, now);
        Some((to_write, slot.view, !slot.model.dragging()))
    }) else {
        return;
    };
    notify(view, window, cx);
    if let Some(v) = to_write {
        write(key, v, window, cx);
    }
    if released {
        schedule_reconcile(key, cx);
    }
}

fn release(key: &ElementId, window: &mut Window, cx: &mut App) {
    let view = SLOTS.with(|s| {
        let mut s = s.borrow_mut();
        let slot = s.get_mut(key)?;
        slot.model.release(Instant::now());
        Some(slot.view)
    });
    if let Some(view) = view {
        notify(view, window, cx);
        schedule_reconcile(key, cx);
    }
}

/// Makes `el` (the slider's root, positioned `relative`) a slider: a press
/// jumps to the value under the pointer and starts a drag that follows the
/// pointer anywhere in the window until release, with the drag model above.
/// `key` must be the element's id; `on_change` writes a value (`0..=1`).
pub fn bind<R: IntoWrite>(el: gpui::Stateful<gpui::Div>, key: ElementId, track: Track, on_change: impl Fn(f32, &mut Window, &mut App) -> R + 'static) -> gpui::Stateful<gpui::Div> {
    let rtl = crate::bidi::rtl();
    let writer: Writer = Rc::new(move |v, window, cx| on_change(v, window, cx).into_write());
    let bounds: Rc<Cell<gpui::Bounds<gpui::Pixels>>> = Rc::default();
    let view: Rc<Cell<Option<EntityId>>> = Rc::default();
    let value_at = move |x: gpui::Pixels, b: gpui::Bounds<gpui::Pixels>| track.value_at(f32::from(x), f32::from(b.origin.x), f32::from(b.size.width), rtl);
    let (paint_key, paint_writer, paint_bounds, paint_view) = (key.clone(), writer.clone(), bounds.clone(), view.clone());
    el.child(
        canvas(
            move |b, _, _| paint_bounds.set(b),
            move |b, _, window, _| {
                paint_view.set(Some(window.current_view()));
                // Window-wide listeners: the drag keeps following the pointer
                // after it leaves the track.
                let (move_key, up_key) = (paint_key.clone(), paint_key);
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && dragging(&move_key) {
                        pointer_moved(&move_key, value_at(e.position.x, b), e.pressed_button == Some(MouseButton::Left), paint_writer.clone(), window, cx);
                    }
                });
                window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && e.button == MouseButton::Left && dragging(&up_key) {
                        release(&up_key, window, cx);
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full(),
    )
    .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, window, cx| {
        press(&key, value_at(e.position.x, bounds.get()), view.get(), writer.clone(), window, cx);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-5;

    #[test]
    fn pointer_maps_to_the_value_under_it_with_insets() {
        // A 300 px track at x = 100 whose handle centre runs 32 px … 292 px.
        let t = Track { start_inset: 32.0, end_inset: 8.0 };
        assert!((t.value_at(132.0, 100.0, 300.0, false)).abs() < EPS, "value 0 at the start inset");
        assert!((t.value_at(392.0, 100.0, 300.0, false) - 1.0).abs() < EPS, "value 1 at the end inset");
        assert!((t.value_at(262.0, 100.0, 300.0, false) - 0.5).abs() < EPS);
        assert_eq!(t.value_at(110.0, 100.0, 300.0, false), 0.0, "inside the start cap clamps to 0");
        assert_eq!(t.value_at(399.0, 100.0, 300.0, false), 1.0, "past the end clamps to 1");
        // Right to left: mirrored.
        assert!((t.value_at(368.0, 100.0, 300.0, true)).abs() < EPS);
        assert!((t.value_at(238.0, 100.0, 300.0, true) - 0.5).abs() < EPS);
        // Round trip: the handle is drawn exactly where the pointer is.
        for rtl in [false, true] {
            for i in 0..=100 {
                let v = i as f32 / 100.0;
                let x = t.position_of(v, 100.0, 300.0, rtl);
                assert!((t.value_at(x, 100.0, 300.0, rtl) - v).abs() < EPS, "round trip at {v} (rtl {rtl})");
            }
        }
        // No layout yet: no division by zero.
        assert_eq!(Track::EDGE_TO_EDGE.value_at(5.0, 0.0, 0.0, false), 1.0);
    }

    #[test]
    fn mapping_is_the_same_at_every_output_scale() {
        // The compositor delivers positions in device pixels; GPUI hands
        // bounds and pointer events to the slider in logical pixels (device
        // / scale). The same physical gesture must pick the same value at
        // 1x, 1.25x and 1.5x, with the track's padding scaled alike.
        let t = Track { start_inset: 32.0, end_inset: 8.0 };
        let (origin, width) = (1336.0_f32, 328.0_f32); // logical
        for frac in [0.0_f32, 0.1, 0.37, 0.5, 0.83, 1.0] {
            let logical_x = t.position_of(frac, origin, width, false);
            for scale in [1.0_f32, 1.25, 1.5] {
                let (dev_x, dev_origin, dev_width) = (logical_x * scale, origin * scale, width * scale);
                let v = t.value_at(dev_x / scale, dev_origin / scale, dev_width / scale, false);
                assert!((v - frac).abs() < 1e-4, "{frac} at scale {scale}: got {v}");
            }
        }
        // Half a device pixel of pointer jitter moves the value by less
        // than one displayed percent at every scale.
        for scale in [1.0_f32, 1.25, 1.5] {
            let a = t.value_at(1500.0, origin, width, false);
            let b = t.value_at(1500.0 + 0.5 / scale, origin, width, false);
            assert!((b - a).abs() < 0.01, "scale {scale}");
        }
    }

    #[test]
    fn echoes_are_ignored_while_dragging_and_settling() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut m = LiveValue::default();
        assert_eq!(m.shown(0.3, t0), 0.3, "idle: the owner's value");
        assert_eq!(m.press(0.6, t0), Some(0.6), "the press writes at once");
        assert_eq!(m.shown(0.3, ms(1)), 0.6, "a stale echo doesn't move the slider");
        m.pointer_moved(0.7, true, ms(10));
        assert_eq!(m.shown(0.6, ms(11)), 0.7, "the echo of the first write is ignored too");
        m.release(ms(20));
        assert!(m.holding(ms(21)), "released with writes outstanding: still held");
        assert_eq!(m.write_done(ms(100)), Some(0.7), "the newest value goes out next");
        assert_eq!(m.write_done(ms(200)), None);
        assert_eq!(m.shown(0.6, ms(200) + SETTLE / 2), 0.7, "settling after the last write");
        assert_eq!(m.hold_ends_in(ms(200)), Some(SETTLE));
        assert_eq!(m.shown(0.7, ms(200) + SETTLE), 0.7, "reconciled to the owner's value");
        assert_eq!(m.shown(0.25, ms(200) + SETTLE), 0.25, "a later change made elsewhere shows");
        assert!(m.finished(ms(200) + SETTLE));
    }

    #[test]
    fn bursts_keep_one_write_in_flight_and_the_last_value_wins() {
        let t0 = Instant::now();
        let mut m = LiveValue::default();
        let mut written = vec![m.press(0.10, t0).unwrap()];
        for i in 1..=200 {
            assert_eq!(m.pointer_moved(0.10 + i as f32 * 0.004, true, t0), None, "a write is in flight");
        }
        m.release(t0);
        while let Some(v) = m.write_done(t0) {
            written.push(v);
        }
        assert_eq!(written.len(), 2, "200 pointer events: two writes");
        assert!((written[1] - 0.9).abs() < EPS, "the last position lands");
        // Unchanged positions write nothing; returning to the value in
        // flight cancels the pending one.
        let mut m = LiveValue::default();
        assert_eq!(m.press(0.5, t0), Some(0.5));
        assert_eq!(m.pointer_moved(0.6, true, t0), None);
        assert_eq!(m.pointer_moved(0.5, true, t0), None);
        assert_eq!(m.write_done(t0), None, "back where the write in flight goes: nothing more");
        assert_eq!(m.pointer_moved(0.5, true, t0), None, "same position: nothing to write");
        assert_eq!(m.pointer_moved(0.55, true, t0), Some(0.55));
    }

    #[test]
    fn a_lost_write_doesnt_freeze_the_slider() {
        let t0 = Instant::now();
        let mut m = LiveValue::default();
        m.press(0.4, t0);
        m.release(t0);
        assert!(m.holding(t0 + WRITE_TIMEOUT / 2));
        assert!(!m.holding(t0 + WRITE_TIMEOUT + SETTLE), "never answered: the owner's value again");
        assert_eq!(m.press(0.8, t0 + WRITE_TIMEOUT + SETTLE), Some(0.8), "and the next drag writes");
    }

    #[test]
    fn capture_follows_the_pointer_outside_the_track_until_release() {
        let t = Track { start_inset: 32.0, end_inset: 8.0 };
        let (origin, width) = (100.0, 300.0);
        let t0 = Instant::now();
        let mut m = LiveValue::default();
        // Moves before a press do nothing.
        assert_eq!(m.pointer_moved(0.5, true, t0), None);
        assert!(!m.dragging());
        m.press(t.value_at(262.0, origin, width, false), t0);
        m.write_done(t0);
        // The pointer leaves the thin track (its height doesn't matter) and
        // goes past its end: the drag continues, clamped.
        assert_eq!(m.pointer_moved(t.value_at(600.0, origin, width, false), true, t0), Some(1.0));
        m.write_done(t0);
        assert_eq!(m.pointer_moved(t.value_at(-50.0, origin, width, false), true, t0), Some(0.0));
        assert!(m.dragging());
        // A move without the button (a release delivered elsewhere) ends it.
        assert_eq!(m.pointer_moved(0.7, false, t0), None);
        assert!(!m.dragging());
        assert_eq!(m.pointer_moved(0.7, true, t0), None, "no longer captured");
    }

    /// A slow sound server: each write takes `LATENCY` to apply, and its
    /// change event reaches the shell `ECHO` later still. The pointer drags
    /// up, then back down (the reported back-and-forth). The fill must sit
    /// under the pointer the whole time (never pulled back by a stale echo,
    /// never a step behind), at most one write may be in flight, and the
    /// last position must reach the server.
    #[test]
    fn a_slow_server_never_pulls_the_slider_back() {
        const LATENCY: u64 = 90;
        const ECHO: u64 = 140;
        const POINTER_EVERY: u64 = 4; // 250 Hz mouse
        const STEPS: u64 = 150;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut m = LiveValue::default();
        let mut server = 0.2_f32; // what the shell last heard
        let mut applying: Option<(f32, u64)> = None; // value, done at
        let mut echoes: Vec<(f32, u64)> = Vec::new(); // value, arrives at
        let mut writes = 0;
        // Up to 0.8 over the first half, back down to 0.35.
        let target = |step: u64| {
            let half = STEPS / 2;
            if step <= half { 0.2 + 0.6 * step as f32 / half as f32 } else { 0.8 - 0.45 * (step - half) as f32 / half as f32 }
        };
        let mut pointer = target(0);
        let end = STEPS * POINTER_EVERY;
        for now in 0..end + 2000 {
            // Server progress.
            if let Some((v, done)) = applying
                && now >= done
            {
                echoes.push((v, now + ECHO));
                applying = m.write_done(at(now)).map(|next| (next, now + LATENCY));
            }
            echoes.retain(|&(v, t)| {
                if now >= t {
                    server = v;
                    false
                } else {
                    true
                }
            });
            // Pointer.
            if now % POINTER_EVERY == 0 && now <= end {
                let step = now / POINTER_EVERY;
                pointer = target(step);
                let w = if step == 0 { m.press(pointer, at(now)) } else { m.pointer_moved(pointer, true, at(now)) };
                if let Some(v) = w {
                    assert!(applying.is_none(), "one write in flight");
                    applying = Some((v, now + LATENCY));
                    writes += 1;
                }
            }
            if now == end {
                m.release(at(now));
            }
            let drawn = m.shown(server, at(now));
            if now <= end {
                assert!((drawn - pointer).abs() < EPS, "drawn {drawn} but the pointer is at {pointer} ({now} ms; server says {server})");
            }
        }
        assert!(writes < STEPS / 4, "{writes} writes for {STEPS} pointer events: coalesced");
        assert!((server - target(STEPS)).abs() < EPS, "the last position reached the server");
        assert!((m.shown(server, at(end + 2000)) - target(STEPS)).abs() < EPS, "and is drawn after reconciling");
        assert!(m.finished(at(end + 2000)));
    }
}
