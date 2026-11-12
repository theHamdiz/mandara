//! A quick back-and-forth of the pointer ("shake"), detected from the
//! pointer events a surface already receives: no timer, no polling, no
//! allocation. [`ShakeDetector::feed`] is a pure step over one sample.
//!
//! A shake is at least `strokes` horizontal strokes in alternating
//! directions, each at least `travel` logical pixels long and mostly
//! horizontal, all within `window`. Small wobbles under the dead zone never
//! reverse a stroke, a pause longer than `window` starts over, and after a
//! shake the detector rests for `cooldown` so one gesture fires once.

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

/// The process-wide shake level ([`ShakeSensitivity::to_u8`]); `Off` until
/// the shell sets it (Stash on, shaking not switched off).
static LEVEL: AtomicU8 = AtomicU8::new(0);

/// Sets the shake level every surface detects with.
pub fn set_level(level: ShakeSensitivity) {
    LEVEL.store(level.to_u8(), Ordering::Relaxed);
}

/// The thresholds in force, or `None` while shaking is off.
pub fn current_tuning() -> Option<ShakeTuning> {
    ShakeSensitivity::from_u8(LEVEL.load(Ordering::Relaxed)).tuning()
}

/// How readily a shake is recognised.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShakeSensitivity {
    /// Never.
    Off,
    /// Short strokes are enough.
    Gentle,
    /// The default.
    #[default]
    Normal,
    /// Long, deliberate strokes only.
    Firm,
}

impl ShakeSensitivity {
    pub const ALL: [ShakeSensitivity; 4] = [ShakeSensitivity::Off, ShakeSensitivity::Gentle, ShakeSensitivity::Normal, ShakeSensitivity::Firm];

    pub fn id(self) -> &'static str {
        match self {
            ShakeSensitivity::Off => "off",
            ShakeSensitivity::Gentle => "gentle",
            ShakeSensitivity::Normal => "normal",
            ShakeSensitivity::Firm => "firm",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.id().eq_ignore_ascii_case(s.trim()))
    }

    /// The thresholds, or `None` when shaking is off.
    pub fn tuning(self) -> Option<ShakeTuning> {
        let (travel, strokes, window_ms) = match self {
            ShakeSensitivity::Off => return None,
            ShakeSensitivity::Gentle => (28.0, 4, 650),
            ShakeSensitivity::Normal => (44.0, 4, 550),
            ShakeSensitivity::Firm => (70.0, 5, 500),
        };
        Some(ShakeTuning { travel, strokes, window: Duration::from_millis(window_ms), ..ShakeTuning::default() })
    }

    /// Stored as a small integer where an atomic carries it across threads.
    pub fn to_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(v: u8) -> Self {
        Self::ALL.get(usize::from(v)).copied().unwrap_or(ShakeSensitivity::Off)
    }
}

/// The thresholds of one sensitivity.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ShakeTuning {
    /// Shortest stroke, logical px along x.
    pub travel: f32,
    /// Strokes in alternating directions (3 reversals = 4 strokes).
    pub strokes: u8,
    /// All strokes within this long.
    pub window: Duration,
    /// Motion back against a stroke shorter than this is a wobble.
    pub dead_zone: f32,
    /// A stroke whose vertical extent passes this share of its length is
    /// not horizontal.
    pub max_slope: f32,
    /// Rest after a shake.
    pub cooldown: Duration,
}

impl Default for ShakeTuning {
    fn default() -> Self {
        Self { travel: 44.0, strokes: 4, window: Duration::from_millis(550), dead_zone: 6.0, max_slope: 0.7, cooldown: Duration::from_millis(900) }
    }
}

/// Most stroke ends remembered (more than any tuning asks for).
const MAX_STROKES: usize = 6;

/// The stroke being drawn.
#[derive(Copy, Clone, Debug)]
struct Stroke {
    /// +1 right, -1 left, 0 not yet known.
    dir: i8,
    start_x: f32,
    /// Furthest x reached in `dir`.
    peak_x: f32,
    min_y: f32,
    max_y: f32,
}

/// Follows one pointer on one surface: under 160 bytes, no heap.
#[derive(Clone, Debug, Default)]
pub struct ShakeDetector {
    stroke: Option<Stroke>,
    /// End times of the counted strokes, oldest first.
    ends: [Option<Instant>; MAX_STROKES],
    count: usize,
    last: Option<Instant>,
    resting_until: Option<Instant>,
}

impl ShakeDetector {
    pub const fn new() -> Self {
        Self { stroke: None, ends: [None; MAX_STROKES], count: 0, last: None, resting_until: None }
    }

    /// Forgets everything (the pointer left, or moved to another surface).
    pub fn reset(&mut self) {
        let resting = self.resting_until;
        *self = Self::new();
        self.resting_until = resting;
    }

    /// One pointer position at `now`. Returns `true` exactly once per shake.
    pub fn feed(&mut self, x: f32, y: f32, now: Instant, tuning: &ShakeTuning) -> bool {
        if self.step(x, y, now, tuning) {
            return true;
        }
        // The stroke in progress completes the shake as soon as it is long
        // enough (no need to reverse once more).
        if self.resting_until.is_none() && self.count + 1 >= usize::from(tuning.strokes) && self.current_long_enough(tuning) {
            return self.fire(now, tuning);
        }
        false
    }

    fn step(&mut self, x: f32, y: f32, now: Instant, tuning: &ShakeTuning) -> bool {
        if !x.is_finite() || !y.is_finite() {
            return false;
        }
        if let Some(until) = self.resting_until {
            if now < until {
                self.last = Some(now);
                return false;
            }
            self.resting_until = None;
        }
        // A pause longer than the whole gesture starts over.
        if self.last.is_some_and(|t| now.saturating_duration_since(t) > tuning.window) {
            self.stroke = None;
            self.count = 0;
        }
        self.last = Some(now);
        self.drop_stale(now, tuning.window);
        let Some(mut s) = self.stroke else {
            self.stroke = Some(Stroke { dir: 0, start_x: x, peak_x: x, min_y: y, max_y: y });
            return false;
        };
        s.min_y = s.min_y.min(y);
        s.max_y = s.max_y.max(y);
        if s.dir == 0 {
            let dx = x - s.start_x;
            if dx.abs() >= tuning.dead_zone {
                s.dir = if dx > 0.0 { 1 } else { -1 };
                s.peak_x = x;
            }
            self.stroke = Some(s);
            return false;
        }
        let forward = f32::from(s.dir) * (x - s.peak_x);
        if forward >= 0.0 {
            s.peak_x = x;
            self.stroke = Some(s);
            return false;
        }
        if -forward < tuning.dead_zone {
            self.stroke = Some(s);
            return false;
        }
        // Reversed: the stroke that just ended counts if it was long and flat.
        let length = (s.peak_x - s.start_x).abs();
        let rise = s.max_y - s.min_y;
        if length >= tuning.travel && rise <= length * tuning.max_slope {
            self.push_end(now);
        } else {
            // A short or steep stroke breaks the rhythm.
            self.count = 0;
        }
        self.stroke = Some(Stroke { dir: -s.dir, start_x: s.peak_x, peak_x: x, min_y: y, max_y: y });
        false
    }

    fn current_long_enough(&self, tuning: &ShakeTuning) -> bool {
        self.stroke.is_some_and(|s| s.dir != 0 && (s.peak_x - s.start_x).abs() >= tuning.travel && s.max_y - s.min_y <= (s.peak_x - s.start_x).abs() * tuning.max_slope)
    }

    fn fire(&mut self, now: Instant, tuning: &ShakeTuning) -> bool {
        *self = Self::new();
        self.resting_until = Some(now + tuning.cooldown);
        true
    }

    fn push_end(&mut self, now: Instant) {
        if self.count == MAX_STROKES {
            self.ends.copy_within(1.., 0);
            self.count -= 1;
        }
        self.ends[self.count] = Some(now);
        self.count += 1;
    }

    /// Forgets stroke ends older than the window.
    fn drop_stale(&mut self, now: Instant, window: Duration) {
        let stale = self.ends[..self.count].iter().take_while(|t| t.is_some_and(|t| now.saturating_duration_since(t) > window)).count();
        if stale > 0 {
            self.ends.copy_within(stale..self.count, 0);
            self.count -= stale;
            for slot in &mut self.ends[self.count..] {
                *slot = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds a polyline of x positions (y fixed) sampled every `step_ms`,
    /// `per_leg` samples between corners; returns how often it fired.
    fn run(det: &mut ShakeDetector, corners: &[f32], y_wobble: f32, step_ms: u64, per_leg: usize, tuning: &ShakeTuning) -> usize {
        let t0 = Instant::now();
        let mut fired = 0;
        let mut k = 0u64;
        for pair in corners.windows(2) {
            for i in 0..per_leg {
                let f = i as f32 / per_leg as f32;
                let x = pair[0] + (pair[1] - pair[0]) * f;
                let y = 400.0 + if k.is_multiple_of(2) { y_wobble } else { -y_wobble };
                if det.feed(x, y, t0 + Duration::from_millis(k * step_ms), tuning) {
                    fired += 1;
                }
                k += 1;
            }
        }
        fired
    }

    fn normal() -> ShakeTuning {
        ShakeSensitivity::Normal.tuning().unwrap()
    }

    #[test]
    fn a_quick_back_and_forth_fires_once() {
        let mut d = ShakeDetector::new();
        // Four strokes of 120 px, 8 samples each at 8 ms: 256 ms in all.
        assert_eq!(run(&mut d, &[100.0, 220.0, 100.0, 220.0, 100.0, 220.0], 2.0, 8, 8, &normal()), 1);
    }

    #[test]
    fn slow_strokes_do_not_fire() {
        let mut d = ShakeDetector::new();
        // The same strokes at 40 ms per sample: 1.6 s.
        assert_eq!(run(&mut d, &[100.0, 220.0, 100.0, 220.0, 100.0], 2.0, 40, 10, &normal()), 0);
    }

    #[test]
    fn a_straight_move_or_one_reversal_never_fires() {
        let mut d = ShakeDetector::new();
        assert_eq!(run(&mut d, &[0.0, 900.0], 0.0, 4, 60, &normal()), 0);
        let mut d = ShakeDetector::new();
        assert_eq!(run(&mut d, &[100.0, 300.0, 100.0], 0.0, 6, 10, &normal()), 0);
    }

    #[test]
    fn small_jitter_is_not_a_shake() {
        let mut d = ShakeDetector::new();
        // ±10 px trembling, many times.
        let corners: Vec<f32> = (0..30).map(|i| if i % 2 == 0 { 500.0 } else { 510.0 }).collect();
        assert_eq!(run(&mut d, &corners, 1.0, 5, 4, &normal()), 0);
    }

    #[test]
    fn vertical_scrubbing_is_not_a_shake() {
        let mut d = ShakeDetector::new();
        let t0 = Instant::now();
        let tuning = normal();
        let mut fired = false;
        // Diagonal zig-zag that is mostly vertical: 50 px across, 200 px down.
        for k in 0..60u64 {
            let leg = (k / 8) as f32;
            let f = (k % 8) as f32 / 8.0;
            let x = if (leg as u64).is_multiple_of(2) { 100.0 + 50.0 * f } else { 150.0 - 50.0 * f };
            let y = 200.0 * f + leg * 3.0;
            fired |= d.feed(x, y, t0 + Duration::from_millis(k * 8), &tuning);
        }
        assert!(!fired);
    }

    #[test]
    fn sensitivity_changes_what_counts() {
        // 36 px strokes: enough for gentle, too short for normal and firm.
        let corners = [100.0, 136.0, 100.0, 136.0, 100.0, 136.0];
        for (sensitivity, expect) in [(ShakeSensitivity::Gentle, 1), (ShakeSensitivity::Normal, 0), (ShakeSensitivity::Firm, 0)] {
            let mut d = ShakeDetector::new();
            assert_eq!(run(&mut d, &corners, 0.0, 8, 6, &sensitivity.tuning().unwrap()), expect, "{sensitivity:?}");
        }
        assert!(ShakeSensitivity::Off.tuning().is_none());
    }

    #[test]
    fn cooldown_makes_one_long_shake_fire_once_then_rearms() {
        let tuning = normal();
        let mut d = ShakeDetector::new();
        // Twelve strokes in one go: one fire (the rest falls in the cooldown).
        let corners: Vec<f32> = (0..13).map(|i| if i % 2 == 0 { 100.0 } else { 220.0 }).collect();
        assert_eq!(run(&mut d, &corners, 0.0, 6, 6, &tuning), 1);
        // A second gesture after the cooldown fires again.
        let t1 = Instant::now() + Duration::from_secs(5);
        let mut fired = 0;
        for (k, x) in (0..40u64).zip([100.0f32, 220.0, 100.0, 220.0, 100.0].windows(2).flat_map(|w| (0..8).map(move |i| w[0] + (w[1] - w[0]) * i as f32 / 8.0))) {
            if d.feed(x, 300.0, t1 + Duration::from_millis(k * 8), &tuning) {
                fired += 1;
            }
        }
        assert_eq!(fired, 1);
    }

    #[test]
    fn a_pause_starts_over() {
        let tuning = normal();
        let mut d = ShakeDetector::new();
        let t0 = Instant::now();
        let mut fired = false;
        // Two strokes, a 1 s pause, two more strokes: never four in a row.
        let mut k = 0u64;
        for (gap, corners) in [(0u64, [100.0f32, 220.0, 100.0]), (1000, [100.0, 220.0, 100.0])] {
            for w in corners.windows(2) {
                for i in 0..8 {
                    let x = w[0] + (w[1] - w[0]) * i as f32 / 8.0;
                    fired |= d.feed(x, 300.0, t0 + Duration::from_millis(gap + k * 8), &tuning);
                    k += 1;
                }
            }
        }
        assert!(!fired);
    }

    #[test]
    fn non_finite_samples_are_ignored_and_the_detector_is_small() {
        let mut d = ShakeDetector::new();
        assert!(!d.feed(f32::NAN, 0.0, Instant::now(), &normal()));
        assert!(std::mem::size_of::<ShakeDetector>() <= 160, "{}", std::mem::size_of::<ShakeDetector>());
        for s in ShakeSensitivity::ALL {
            assert_eq!(ShakeSensitivity::from_u8(s.to_u8()), s);
            assert_eq!(ShakeSensitivity::parse(s.id()), Some(s));
        }
    }
}
