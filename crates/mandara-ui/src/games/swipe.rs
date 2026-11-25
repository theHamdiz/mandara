//! Turns a drag (mouse) or a swipe (finger) into one move: once the
//! pointer has travelled far enough from where it went down, mostly along
//! one axis, the gesture fires once and then waits for release.

use super::tiles::board::Dir;

/// Travel (logical px) before a drag counts as a move.
pub const SWIPE_PX: f32 = 28.0;
/// The main axis must be this much longer than the other one.
const AXIS_RATIO: f32 = 1.3;

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Swipe {
    start: Option<(f32, f32)>,
    fired: bool,
}

impl Swipe {
    pub fn press(&mut self, x: f32, y: f32) {
        self.start = Some((x, y));
        self.fired = false;
    }

    pub fn active(&self) -> bool {
        self.start.is_some()
    }

    /// The move this position completes, once per gesture.
    pub fn moved(&mut self, x: f32, y: f32, threshold: f32) -> Option<Dir> {
        let (sx, sy) = self.start?;
        if self.fired {
            return None;
        }
        let (dx, dy) = (x - sx, y - sy);
        let (ax, ay) = (dx.abs(), dy.abs());
        let dir = if ax >= threshold && ax >= ay * AXIS_RATIO {
            if dx > 0.0 { Dir::Right } else { Dir::Left }
        } else if ay >= threshold && ay >= ax * AXIS_RATIO {
            if dy > 0.0 { Dir::Down } else { Dir::Up }
        } else {
            return None;
        };
        self.fired = true;
        Some(dir)
    }

    pub fn release(&mut self) {
        self.start = None;
        self.fired = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_enough_drag_fires_once_along_its_axis() {
        let mut s = Swipe::default();
        assert_eq!(s.moved(100.0, 0.0, SWIPE_PX), None, "nothing before a press");
        s.press(50.0, 50.0);
        assert_eq!(s.moved(60.0, 52.0, SWIPE_PX), None, "too short");
        assert_eq!(s.moved(90.0, 55.0, SWIPE_PX), Some(Dir::Right));
        assert_eq!(s.moved(150.0, 55.0, SWIPE_PX), None, "once per gesture");
        s.release();
        s.press(50.0, 50.0);
        assert_eq!(s.moved(48.0, 10.0, SWIPE_PX), Some(Dir::Up));
        s.press(50.0, 50.0);
        assert_eq!(s.moved(20.0, 52.0, SWIPE_PX), Some(Dir::Left));
        s.press(50.0, 50.0);
        assert_eq!(s.moved(52.0, 99.0, SWIPE_PX), Some(Dir::Down));
    }

    #[test]
    fn a_diagonal_drag_is_ambiguous_and_ignored() {
        let mut s = Swipe::default();
        s.press(0.0, 0.0);
        assert_eq!(s.moved(40.0, 38.0, SWIPE_PX), None);
        assert!(s.active());
    }
}
