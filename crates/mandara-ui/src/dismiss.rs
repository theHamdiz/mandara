//! One dismissal policy for every overlay and panel.
//!
//! Three rules, learned the hard way on MangoWM:
//!
//! - **Focus-out settles.** Clicking a layer surface makes the compositor
//!   send a keyboard leave immediately followed by an enter, *before* the
//!   press itself. A deactivation only means "the user went elsewhere" once
//!   it has lasted [`DEACTIVATE_SETTLE`]; keyboard state that arrives inside
//!   that window (a modifier "release" on re-entry) is an artifact.
//! - **Outside clicks are hit-tested by the element tree.** A full-surface
//!   catcher behind the content dismisses, and the content `.occlude()`s it.
//!   Never compare a press against hand-computed rectangles: springs, scale-in
//!   animations, edge flares and fractional-scale snapping all draw outside
//!   them. Where a rectangle is unavoidable (hover-leave timers), test it with
//!   [`Rect::contains`] and slack on both axes.
//! - **The opener decides the gesture.** Only a compositor keybind can be a
//!   held-modifier gesture; anything opened from inside the shell (launcher,
//!   hot corner, bar click) must not wait for a modifier release it will
//!   never see. Commands record where they came from with [`with_origin`].
//! - **The panel moving is not the user leaving.** When a panel resizes
//!   under a resting pointer, the leave and the next press outside are
//!   judged against where it rested before ([`ResizeShadow`]).

use std::time::{Duration, Instant};

use gpui::{App, Global, InteractiveElement};

/// How long a surface must stay without keyboard focus (or how long after a
/// focus change keyboard state is distrusted) before it counts as real.
pub const DEACTIVATE_SETTLE: Duration = Duration::from_millis(140);

/// Focus changes this soon after a press on Mandara are consequences of the
/// press (a raised player window, a slow leave/enter pair), never a click
/// elsewhere.
pub const PRESS_GRACE: Duration = Duration::from_millis(1500);

/// Whether losing keyboard focus at `now` may dismiss: not while a press on
/// the surface is still within [`PRESS_GRACE`].
pub fn focus_loss_dismisses(last_press: Option<Instant>, now: Instant) -> bool {
    last_press.is_none_or(|t| now.saturating_duration_since(t) >= PRESS_GRACE)
}

/// Where the command being executed came from.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Origin {
    /// From inside the shell: the launcher, a hot corner, a bar or dock click.
    #[default]
    InProcess,
    /// Over IPC (`mandara msg …`): a compositor keybind or a script.
    External,
}

#[derive(Default)]
struct CurrentOrigin(Origin);
impl Global for CurrentOrigin {}

/// Runs `f` with [`origin`] reporting `origin` (restored afterwards).
pub fn with_origin<R>(origin: Origin, cx: &mut App, f: impl FnOnce(&mut App) -> R) -> R {
    let before = cx.default_global::<CurrentOrigin>().0;
    cx.default_global::<CurrentOrigin>().0 = origin;
    let out = f(cx);
    cx.default_global::<CurrentOrigin>().0 = before;
    out
}

/// Where the command being executed came from.
pub fn origin(cx: &App) -> Origin {
    cx.try_global::<CurrentOrigin>().map(|o| o.0).unwrap_or_default()
}

/// Marks an element as inside its overlay: presses of *any* button on it
/// stop there and never reach a dismissing handler on an ancestor (stopping
/// only the left button lets a right-click on a card close the overlay).
pub trait KeepInside: InteractiveElement + Sized {
    fn keep_inside(self) -> Self {
        self.on_any_mouse_down(|_, _, cx| cx.stop_propagation())
    }
}
impl<E: InteractiveElement> KeepInside for E {}

/// An axis-aligned rectangle in logical pixels.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Whether `(px, py)` lies inside, grown by `slack` on every side.
    pub fn contains(&self, px: f32, py: f32, slack: f32) -> bool {
        px >= self.x - slack && px <= self.x + self.w + slack && py >= self.y - slack && py <= self.y + self.h + slack
    }
}

/// Slack around [`ResizeShadow`]'s rectangle: springs overshoot and the
/// rect is snapped to the device grid, so a resting pointer on its edge
/// still counts as inside.
pub const SHADOW_SLACK_PX: f32 = 4.0;

/// Where an open panel rested before the shell resized it, kept while the
/// pointer stays inside that area.
///
/// A panel whose content changes size (lyrics found or not, a list
/// refreshing) shrinks or moves out from under a pointer that never moved.
/// Neither the hover-leave nor a press on the click-outside catcher that
/// follows may count as the user leaving: the panel moved, not the user.
/// The shadow ends as soon as the pointer is seen outside it (a real move
/// away) or back over the panel.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct ResizeShadow {
    rect: Option<Rect>,
    /// A leave arrived inside the shadow: it becomes real once the pointer
    /// moves out of it.
    leave_pending: bool,
}

impl ResizeShadow {
    /// The panel's resting rect changed from `before` to `after`. Only the
    /// part of `before` that `after` no longer covers matters, but the
    /// whole of `before` is kept: inside `after` the panel itself is hit.
    pub fn resized(&mut self, before: Rect, after: Rect) {
        if before != after {
            self.rect = Some(before);
        }
    }

    /// Forgets the shadow and any deferred leave (the panel opened,
    /// switched, closed, or the pointer is over it again).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Whether a pointer at `(x, y)` that is no longer over the panel is
    /// only there because the panel moved away from it.
    pub fn holds(&self, x: f32, y: f32) -> bool {
        self.rect.is_some_and(|r| r.contains(x, y, SHADOW_SLACK_PX))
    }

    /// The pointer stopped hovering the panel at `(x, y)`: whether that
    /// counts as leaving now. Inside the shadow it doesn't; it is deferred
    /// until the pointer moves out ([`Self::pointer_moved`]).
    pub fn leave_counts(&mut self, x: f32, y: f32) -> bool {
        if self.holds(x, y) {
            self.leave_pending = true;
            return false;
        }
        true
    }

    /// The pointer moved to `(x, y)` outside the panel. Returns `true` when
    /// a deferred leave becomes real: the pointer left where the panel was.
    pub fn pointer_moved(&mut self, x: f32, y: f32) -> bool {
        if self.holds(x, y) {
            return false;
        }
        self.rect = None;
        std::mem::take(&mut self.leave_pending)
    }

    /// A press on the click-outside catcher at `(x, y)`: `true` when it
    /// must not dismiss — it landed where the panel just was, aimed at what
    /// moved away, so it counts as a press on the panel. The shadow is
    /// spent: a second, deliberate click there does dismiss.
    pub fn absorbs_press(&mut self, x: f32, y: f32) -> bool {
        let held = self.holds(x, y);
        if held {
            self.clear();
        }
        held
    }
}

/// Remembers the last keyboard-focus change of a surface so state that
/// arrives with it can be told apart from what the user did.
#[derive(Copy, Clone, Debug, Default)]
pub struct FocusSettle {
    changed_at: Option<Instant>,
}

impl FocusSettle {
    /// The surface gained or lost keyboard focus.
    pub fn changed(&mut self, now: Instant) {
        self.changed_at = Some(now);
    }

    /// Time left until a focus change at most [`DEACTIVATE_SETTLE`] old has
    /// settled, or `None` when there is nothing to wait for.
    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        let since = now.saturating_duration_since(self.changed_at?);
        (since < DEACTIVATE_SETTLE).then(|| DEACTIVATE_SETTLE - since)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_contains_with_slack() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert!(r.contains(10.0, 20.0, 0.0));
        assert!(r.contains(110.0, 70.0, 0.0));
        assert!(!r.contains(9.0, 30.0, 0.0));
        assert!(r.contains(4.0, 30.0, 6.0), "slack grows the left edge");
        assert!(r.contains(50.0, 76.0, 6.0), "slack grows the bottom edge");
        assert!(!r.contains(50.0, 77.0, 6.0));
    }

    #[test]
    fn presses_hold_off_focus_loss() {
        let t0 = Instant::now();
        assert!(focus_loss_dismisses(None, t0));
        assert!(!focus_loss_dismisses(Some(t0), t0 + PRESS_GRACE / 2), "a click inside just happened");
        assert!(focus_loss_dismisses(Some(t0), t0 + PRESS_GRACE));
    }

    #[test]
    fn resize_under_a_resting_pointer_is_not_leaving() {
        // The media panel was tall (lyrics loading) and the pointer rests on
        // its play button; the lookup fails and the panel shrinks above it.
        let tall = Rect::new(800.0, 40.0, 420.0, 560.0);
        let short = Rect::new(800.0, 40.0, 420.0, 440.0);
        let play = (1010.0, 560.0);
        assert!(!short.contains(play.0, play.1, 0.0), "the pointer is outside the shrunk panel");
        let mut s = ResizeShadow::default();
        assert!(!s.holds(play.0, play.1), "no resize, no shadow: a leave is a leave");
        let mut copy = s;
        assert!(copy.leave_counts(play.0, play.1), "no resize, no shadow: a leave is a leave");
        s.resized(tall, short);
        assert!(!s.leave_counts(play.0, play.1), "the hover-leave caused by our own resize is ignored");
        assert!(!s.pointer_moved(play.0 + 2.0, play.1 + 1.0), "jitter inside the old area keeps the panel");
        assert!(s.holds(play.0, play.1));
        assert!(s.pointer_moved(play.0, 900.0), "moving out of where the panel was is the real leave");
        assert!(!s.holds(play.0, play.1), "and the shadow is gone for good");
        assert!(!s.pointer_moved(play.0, 950.0), "the leave is reported once");
        // Without a deferred leave, moving out just ends the shadow.
        let mut quiet = ResizeShadow::default();
        quiet.resized(tall, short);
        assert!(!quiet.pointer_moved(play.0, 900.0));
        assert!(quiet.leave_counts(play.0, play.1), "the shadow is gone: a later leave is real");
    }

    #[test]
    fn press_where_the_panel_just_was_does_not_dismiss_once() {
        let mut s = ResizeShadow::default();
        s.resized(Rect::new(0.0, 0.0, 100.0, 300.0), Rect::new(0.0, 0.0, 100.0, 200.0));
        assert!(!s.absorbs_press(50.0, 500.0), "a press far away still dismisses");
        assert!(!s.leave_counts(50.0, 250.0));
        assert!(s.absorbs_press(50.0, 250.0), "the first press in the vacated area is absorbed");
        assert!(!s.absorbs_press(50.0, 250.0), "a second, deliberate click dismisses");
        assert!(!s.pointer_moved(50.0, 900.0), "an absorbed press used the panel: drifting away later doesn't close it");
        let mut same = ResizeShadow::default();
        same.resized(Rect::new(0.0, 0.0, 10.0, 10.0), Rect::new(0.0, 0.0, 10.0, 10.0));
        assert_eq!(same, ResizeShadow::default(), "an unchanged rect casts no shadow");
    }

    #[test]
    fn focus_settle_window() {
        let t0 = Instant::now();
        let mut s = FocusSettle::default();
        assert_eq!(s.remaining(t0), None, "no change yet");
        s.changed(t0);
        assert_eq!(s.remaining(t0), Some(DEACTIVATE_SETTLE));
        assert!(s.remaining(t0 + DEACTIVATE_SETTLE / 2).is_some());
        assert_eq!(s.remaining(t0 + DEACTIVATE_SETTLE), None);
    }
}
