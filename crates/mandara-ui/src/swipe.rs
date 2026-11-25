//! Pointer tracking shared by notification popups and history cards.

use mandara_core::{SpringConfig, SpringValue};

/// Match the existing popup gesture's tolerance for pointer jitter.
pub const SLOP_PX: f32 = 6.0;
/// A short return settles promptly without bouncing the card past its origin.
const RETURN_SPRING: SpringConfig = SpringConfig::new(420.0, 34.0, 1.0);

#[derive(Clone, Copy, PartialEq)]
enum Axis {
    Pending,
    Horizontal,
    Vertical,
}

struct Press {
    origin: (f32, f32),
    offset: f32,
    axis: Axis,
}

pub struct Swipe {
    pub motion: SpringValue<f32>,
    press: Option<Press>,
}

impl Default for Swipe {
    fn default() -> Self {
        Self {
            motion: SpringValue::new(0.0, RETURN_SPRING),
            press: None,
        }
    }
}

impl Swipe {
    /// Resume a returning card at its current offset, without a pointer jump.
    pub fn press(&mut self, x: f32, y: f32) {
        let offset = self.motion.value();
        self.motion.snap_to(offset);
        self.press = Some(Press {
            origin: (x, y),
            offset,
            axis: Axis::Pending,
        });
    }

    pub fn pressed(&self) -> bool {
        self.press.is_some()
    }

    /// Lock the gesture to its first intentional direction. Vertical movement
    /// remains available to the enclosing list and never dismisses a card.
    pub fn drag_to(&mut self, x: f32, y: f32) -> bool {
        let Some(press) = self.press.as_mut() else { return false };
        let (dx, dy) = (x - press.origin.0, y - press.origin.1);
        if !dx.is_finite() || !dy.is_finite() {
            return false;
        }
        if press.axis == Axis::Pending {
            if dx.abs().max(dy.abs()) < SLOP_PX {
                return false;
            }
            press.axis = if dx.abs() >= dy.abs() { Axis::Horizontal } else { Axis::Vertical };
        }
        if press.axis != Axis::Horizontal {
            return false;
        }
        self.motion.snap_to(press.offset + dx);
        true
    }

    /// `None` preserves a click; a consumed drag either returns or dismisses.
    /// The caller supplies its own threshold and off-screen distance.
    pub fn release(&mut self, threshold: f32, exit: f32) -> Option<bool> {
        let press = self.press.take()?;
        if press.axis == Axis::Pending {
            self.motion.set_target(0.0);
            return None;
        }
        let offset = self.motion.value();
        let away = press.axis == Axis::Horizontal && offset.abs() > threshold.max(SLOP_PX);
        self.motion.set_target(if away { offset.signum() * exit.max(threshold) } else { 0.0 });
        Some(away)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_and_short_drags_do_not_dismiss() {
        let mut swipe = Swipe::default();
        swipe.press(100.0, 20.0);
        assert!(!swipe.drag_to(102.0, 21.0));
        assert_eq!(swipe.release(100.0, 400.0), None);
        swipe.press(100.0, 20.0);
        assert!(swipe.drag_to(160.0, 20.0));
        assert_eq!(swipe.motion.value(), 60.0);
        assert_eq!(swipe.release(100.0, 400.0), Some(false));
        assert_eq!(swipe.motion.target(), 0.0);
    }

    #[test]
    fn both_directions_dismiss_and_vertical_drags_stay_vertical() {
        for direction in [-1.0, 1.0] {
            let mut swipe = Swipe::default();
            swipe.press(200.0, 20.0);
            swipe.drag_to(200.0 + direction * 150.0, 22.0);
            assert_eq!(swipe.release(100.0, 400.0), Some(true));
            assert_eq!(swipe.motion.target(), direction * 400.0);
        }
        let mut swipe = Swipe::default();
        swipe.press(0.0, 0.0);
        assert!(!swipe.drag_to(2.0, 20.0));
        assert!(!swipe.drag_to(200.0, 30.0));
        assert_eq!(swipe.release(100.0, 400.0), Some(false));
        assert_eq!(swipe.motion.target(), 0.0);
    }

    #[test]
    fn grabbing_a_returning_card_preserves_its_offset() {
        let mut swipe = Swipe::default();
        swipe.motion.snap_to(40.0);
        swipe.motion.set_target(0.0);
        swipe.press(200.0, 20.0);
        assert!(!swipe.motion.is_animating(), "a grabbed card waits for the pointer");
        assert!(swipe.drag_to(210.0, 20.0));
        assert_eq!(swipe.motion.value(), 50.0);
        swipe.release(100.0, 400.0);
        swipe.press(200.0, 20.0);
        assert!(!swipe.drag_to(202.0, 40.0));
        assert_eq!(swipe.release(100.0, 400.0), Some(false));
    }
}
