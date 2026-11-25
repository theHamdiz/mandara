//! Motion for the component designs: Material 3 Expressive's springs,
//! played at the shell's animation speed and snapped to their target with
//! reduced motion, and the one control the pointer is holding down.
//!
//! The springs are GPUI's stateful spring elements: their state lives in
//! the element's own frame state (keyed by the element's id under its
//! parent's), so it is dropped with the element and a control drawn for the
//! first time starts at its target (opening a panel never animates its
//! switches; flipping one does).

use std::cell::RefCell;

use gpui::{AnimationExt as _, App, ElementId, InteractiveElement, IntoElement, MouseButton, SpringAnimation, SpringAnimationElement, SpringConfig, SpringPlayback, Window};

/// Material 3 Expressive's motion scheme springs: damping ratio and
/// stiffness (`ExpressiveMotionTokens`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Feel {
    /// Small moves that may overshoot a little: a press morph, a thumb.
    FastSpatial,
    /// Larger moves: a thumb crossing its track.
    DefaultSpatial,
    /// Colour and opacity, never overshooting.
    FastEffects,
}

impl Feel {
    /// `(damping ratio, stiffness)`.
    pub const fn tokens(self) -> (f32, f32) {
        match self {
            Feel::FastSpatial => (0.6, 800.0),
            Feel::DefaultSpatial => (0.8, 380.0),
            Feel::FastEffects => (1.0, 3800.0),
        }
    }

    /// The spring at the animation speed `speed` (2 plays it twice as
    /// fast: stiffness by `speed²`, the same damping ratio).
    pub fn config(self, speed: f32) -> SpringConfig {
        let (ratio, stiffness) = self.tokens();
        let s = if speed.is_finite() && speed > 0.0 { speed } else { 1.0 };
        let k = stiffness * s * s;
        SpringConfig::new(k, 2.0 * ratio * k.sqrt(), 1.0)
    }
}

/// Settles within this much of its target (the values are fractions).
const EPSILON: f32 = 0.002;

/// `el` drawn at a spring heading for `target`, keyed `id` under its
/// parent: `draw` gets the spring's value (past the target while it
/// overshoots). Reduced motion draws the target at once.
pub fn spring<E: IntoElement + 'static>(el: E, id: impl Into<ElementId>, target: f32, feel: Feel, draw: impl FnOnce(E, f32) -> E + 'static) -> SpringAnimationElement<E> {
    let profile = mandara_core::spring::motion_profile();
    let playback = if profile.reduce { SpringPlayback::Completed } else { SpringPlayback::Running };
    el.with_spring(id, SpringAnimation::new(feel.config(mandara_core::spring::time_scale())).to(target).with_epsilon(EPSILON).playback(playback), draw)
}

thread_local! {
    /// The control the left button is holding down (one pointer, one at a
    /// time).
    static HELD: RefCell<Option<ElementId>> = const { RefCell::new(None) };
}

/// Whether `id` is the control being held down.
pub fn held(id: &ElementId) -> bool {
    HELD.with(|h| h.borrow().as_ref() == Some(id))
}

fn set_held(id: Option<ElementId>, window: &mut Window) {
    HELD.with(|h| *h.borrow_mut() = id);
    window.refresh();
}

/// Tracks presses of `el` (id `id`) for [`held`]: down holds it, the
/// release (anywhere) lets go. Never stops the event, so the control's own
/// click still fires.
pub fn track_press<E: InteractiveElement>(el: E, id: ElementId) -> E {
    let up = id.clone();
    el.on_mouse_down(MouseButton::Left, move |_, window: &mut Window, _: &mut App| set_held(Some(id.clone()), window))
        .on_mouse_up(MouseButton::Left, move |_, window: &mut Window, _: &mut App| {
            if held(&up) {
                set_held(None, window);
            }
        })
        .on_mouse_up_out(MouseButton::Left, |_, window: &mut Window, _: &mut App| {
            if HELD.with(|h| h.borrow().is_some()) {
                set_held(None, window);
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn springs_keep_their_damping_ratio_at_every_speed() {
        for feel in [Feel::FastSpatial, Feel::DefaultSpatial, Feel::FastEffects] {
            let (ratio, stiffness) = feel.tokens();
            for speed in [0.25, 1.0, 3.0] {
                let c = feel.config(speed);
                let (w0, zeta) = c.canonical();
                assert!((zeta - ratio).abs() < 1e-4, "{feel:?} at {speed}×: ratio {zeta}");
                assert!((w0 - stiffness.sqrt() * speed).abs() < 1e-2, "{feel:?} at {speed}×: plays {speed} times as fast");
            }
            assert_eq!(feel.config(f32::NAN), feel.config(1.0), "a broken speed plays at the normal one");
        }
        assert!(Feel::FastEffects.tokens().0 >= 1.0, "effects never overshoot");
    }
}
