//! The first real frame of a full-screen overlay (launcher, power menu,
//! wallpaper picker, password prompts).
//!
//! GPUI's first frames of a new layer surface use an integer-scale guess of
//! its size, so overlays draw nothing until the viewport takes the size of
//! the output they were opened for ([`FirstFrame`]). Such an overlay holds
//! the keyboard exclusively from the start, and Hyprland also sends it
//! *every* pointer event, on every display. One that never draws is an
//! invisible wall over the whole desktop (issue #55: the power menu waited
//! for the first output's size on a smaller second display). Two nets keep
//! that from ever happening again:
//!
//! - after [`FALLBACK_FRAMES`] frames of waiting, the overlay draws at the
//!   size it has, whatever output the compositor put it on;
//! - [`guard`] closes an overlay that still has not drawn after
//!   [`DEADLINE`] (no frames reach its output at all), so its input goes
//!   back to the desktop.

use std::time::Duration;

use gpui::{Context, Window};

/// Viewport sizes within this many logical px count as the expected one
/// (xdg-output rounds fractional-scale sizes to whole px).
pub const SLACK_PX: f32 = 2.0;
/// Frames an overlay waits for its expected size before drawing at the size
/// it has (a few frames cover GPUI's scale guess; more is an invisible
/// surface holding the keyboard).
pub const FALLBACK_FRAMES: u8 = 6;
/// An overlay that has not drawn by then is closed: a first frame normally
/// lands within a few dozen milliseconds; this leaves room for a busy
/// machine while never locking the desktop for long.
pub const DEADLINE: Duration = Duration::from_secs(2);

/// Whether `viewport` is `expected`, within [`SLACK_PX`].
pub fn same_size(viewport: (f32, f32), expected: (f32, f32)) -> bool {
    (viewport.0 - expected.0).abs() <= SLACK_PX && (viewport.1 - expected.1).abs() <= SLACK_PX
}

/// A window's viewport size, logical px.
pub fn viewport(window: &Window) -> (f32, f32) {
    let vp = window.viewport_size();
    (f32::from(vp.width), f32::from(vp.height))
}

/// Waits for an overlay's real size, for [`FALLBACK_FRAMES`] at most.
#[derive(Clone, Debug)]
pub struct FirstFrame {
    expected: (f32, f32),
    waited: u8,
    drawn: bool,
}

impl FirstFrame {
    /// An overlay opened for an output of logical size `expected`.
    pub fn new(expected: (f32, f32)) -> Self {
        Self { expected, waited: 0, drawn: false }
    }

    /// Whether `viewport` is the size the overlay was opened for.
    pub fn matches(&self, viewport: (f32, f32)) -> bool {
        same_size(viewport, self.expected)
    }

    /// Whether the overlay may draw a frame of `viewport` now. Counts a
    /// waiting frame when not; once it has drawn, it always may.
    pub fn ready(&mut self, viewport: (f32, f32)) -> bool {
        if !self.drawn && (self.matches(viewport) || self.waited >= FALLBACK_FRAMES) {
            self.drawn = true;
        }
        if !self.drawn {
            self.waited = self.waited.saturating_add(1);
        }
        self.drawn
    }

    /// [`Self::ready`] for `window`'s viewport.
    pub fn ready_in(&mut self, window: &Window) -> bool {
        self.ready(viewport(window))
    }

    /// Whether the overlay has drawn its first real frame.
    pub fn drawn(&self) -> bool {
        self.drawn
    }
}

/// Closes the overlay in `window` when it has not drawn its first real
/// frame by [`DEADLINE`]: `give_up` answers whatever it was opened for
/// (cancels the prompt, clears the shell state), then the surface goes, and
/// with it the exclusive keyboard (and, on Hyprland, the pointer).
pub fn guard<V: 'static>(what: &'static str, window: &mut Window, cx: &mut Context<V>, drawn: fn(&V) -> bool, give_up: fn(&mut V, &mut Context<V>)) {
    cx.spawn_in(window, async move |this, cx| {
        cx.background_executor().timer(DEADLINE).await;
        let _ = this.update_in(cx, |this, window, cx| {
            if !drawn(this) {
                let (w, h) = viewport(window);
                tracing::warn!("{what}: no frame drawn within {DEADLINE:?} (surface {w}x{h}); closing it so it can't hold the keyboard and pointer");
                give_up(this, cx);
                window.remove_window();
            }
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_expected_size_draws_at_once() {
        let mut f = FirstFrame::new((1536.0, 864.0));
        assert!(!f.drawn());
        assert!(f.ready((1536.0 + SLACK_PX, 864.0)), "within the slack");
        assert!(f.drawn());
    }

    #[test]
    fn another_size_draws_after_the_fallback_frames() {
        // The compositor put the overlay on an output of another size: a
        // few frames of waiting, never an invisible surface for good.
        let mut f = FirstFrame::new((2560.0, 1440.0));
        for _ in 0..FALLBACK_FRAMES {
            assert!(!f.ready((1536.0, 864.0)));
        }
        assert!(f.ready((1536.0, 864.0)));
        // Once drawn it keeps drawing, whatever the viewport does next.
        assert!(f.ready((800.0, 600.0)));
        assert!(f.drawn());
    }

    #[test]
    fn waiting_never_overflows() {
        let mut f = FirstFrame::new((10.0, 10.0));
        let drew = (0..=u16::from(u8::MAX)).filter(|_| f.ready((0.0, 0.0))).count();
        assert_eq!(drew, usize::from(u8::MAX) + 1 - usize::from(FALLBACK_FRAMES));
        assert!(f.drawn());
        assert_eq!(f.waited, FALLBACK_FRAMES, "counting stops once it draws");
    }
}
