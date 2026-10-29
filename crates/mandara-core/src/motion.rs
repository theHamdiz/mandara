//! Frame timing for spring-driven views.

use std::time::{Duration, Instant};

/// Measures the time between animation frames for a view.
///
/// GPUI delivers frames at the output's refresh rate when a view calls
/// `window.request_animation_frame()`; this clock turns those into `dt`
/// values for [`crate::SpringValue::step`]. After an idle period the first
/// frame uses a nominal 144 Hz step instead of the (huge) wall-clock gap.
///
/// Render timestamps jitter by a millisecond or two around the display's
/// vsync, and a single slow frame (first build of a panel, a font fallback)
/// would otherwise make every spring *jump*. [`FrameClock::tick_vsync`]
/// therefore quantizes each step to whole refresh intervals and caps it at
/// [`MAX_DT`]: motion advances in even steps and a hitch slows the
/// animation for one frame instead of teleporting it.
#[derive(Debug, Default, Clone)]
pub struct FrameClock {
    last: Option<Instant>,
    /// Exponential moving average of the frame interval, seconds.
    avg_interval: f32,
}

/// Nominal frame step used when resuming from idle (one 144 Hz frame).
pub const NOMINAL_DT: f32 = 1.0 / 144.0;
/// Upper bound for a single step, in seconds (≈ one dropped 60 Hz frame).
/// Larger gaps are stretched rather than skipped, so a hitch never makes a
/// spring visibly leap.
pub const MAX_DT: f32 = 1.0 / 40.0;
/// Weight of the newest sample in the frame-interval moving average.
const INTERVAL_EMA_WEIGHT: f32 = 0.1;
/// Steps within this fraction of a refresh-interval multiple snap onto it.
const VSYNC_SNAP_TOLERANCE: f32 = 0.35;

impl FrameClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the step for a frame rendered at `now`.
    pub fn tick(&mut self, now: Instant) -> f32 {
        let dt = match self.last {
            Some(last) => now.saturating_duration_since(last).as_secs_f32().clamp(0.0, MAX_DT),
            None => NOMINAL_DT,
        };
        self.last = Some(now);
        if dt > 0.0 {
            self.avg_interval =
                if self.avg_interval == 0.0 { dt } else { self.avg_interval + (dt - self.avg_interval) * INTERVAL_EMA_WEIGHT };
        }
        dt
    }

    /// Like [`Self::tick`], but snaps the step to whole multiples of the
    /// display's refresh `interval` (from `Window::refresh_interval`) when
    /// it is close to one, removing render-timestamp jitter. Falls back to
    /// [`Self::tick`] when the interval is unknown.
    pub fn tick_vsync(&mut self, now: Instant, interval: Option<Duration>) -> f32 {
        let resumed = self.last.is_none();
        let raw = self.tick(now);
        let Some(interval) = interval.map(|d| d.as_secs_f32()).filter(|i| *i > 0.0) else { return raw };
        if resumed {
            // First frame after idle: exactly one refresh interval.
            return interval.min(MAX_DT);
        }
        let frames = raw / interval;
        let whole = frames.round().max(1.0);
        if (frames - whole).abs() <= VSYNC_SNAP_TOLERANCE { (whole * interval).min(MAX_DT) } else { raw }
    }

    /// Marks the animation as idle so the next tick restarts cleanly.
    pub fn idle(&mut self) {
        self.last = None;
    }

    /// Whether the clock is mid-animation (ticked since the last idle).
    pub fn is_running(&self) -> bool {
        self.last.is_some()
    }

    /// Smoothed frames per second while animating (diagnostics HUD).
    pub fn fps(&self) -> f32 {
        if self.avg_interval > 0.0 { 1.0 / self.avg_interval } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_tick_is_nominal_and_gaps_are_clamped() {
        let mut clock = FrameClock::new();
        let t0 = Instant::now();
        assert_eq!(clock.tick(t0), NOMINAL_DT);
        let dt = clock.tick(t0 + Duration::from_millis(7));
        assert!((dt - 0.007).abs() < 1e-4);
        assert_eq!(clock.tick(t0 + Duration::from_secs(5)), MAX_DT);
        clock.idle();
        assert_eq!(clock.tick(t0 + Duration::from_secs(10)), NOMINAL_DT);
    }

    #[test]
    fn vsync_ticks_are_quantized() {
        let mut clock = FrameClock::new();
        let hz60 = Some(Duration::from_micros(16_667));
        let t0 = Instant::now();
        let first = clock.tick_vsync(t0, hz60);
        assert!((first - 0.016_667).abs() < 1e-4);
        // A frame rendered 1.8 ms late still advances exactly one interval.
        let dt = clock.tick_vsync(t0 + Duration::from_micros(18_500), hz60);
        assert!((dt - 0.016_667).abs() < 1e-4, "{dt}");
        // A long hitch is capped, not skipped.
        let dt = clock.tick_vsync(t0 + Duration::from_millis(200), hz60);
        assert_eq!(dt, MAX_DT);
    }
}
