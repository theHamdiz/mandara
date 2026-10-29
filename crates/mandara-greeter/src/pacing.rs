//! Frame-pacing statistics for the login screen, with the same definitions
//! as the shell's `diagnostics frames`: the interval between two renders
//! that are part of one continuous animation (gaps longer than
//! [`CONTINUOUS`] are idle time, not slow frames), the CPU time spent
//! building the view tree, and "late" frames — intervals longer than
//! [`LATE_FACTOR`] × the median interval.
//!
//! The demo logs a summary every few seconds and on exit; set
//! `MANDARA_FRAME_STATS=1` to log it from the Studio preview too.

use std::time::{Duration, Instant};

/// Environment variable that turns pacing logs on outside the demo.
pub const STATS_ENV: &str = "MANDARA_FRAME_STATS";
/// Most frames kept (≈ 57 s at 144 Hz); older ones are dropped in bulk.
pub const CAPACITY: usize = 8192;
/// A render gap longer than this means the view was idle.
pub const CONTINUOUS: Duration = Duration::from_millis(100);
/// An interval longer than this many medians is a late (missed) frame.
pub const LATE_FACTOR: f32 = 1.5;
/// Milliseconds per second (for reporting).
const MS_PER_S: f32 = 1000.0;
/// Late frames whose start times are listed in the summary.
pub const LATE_LISTED: usize = 8;
/// Quantiles reported.
const P50: f32 = 0.5;
const P95: f32 = 0.95;
const P99: f32 = 0.99;

/// Whether pacing should be logged from a non-demo view.
pub fn enabled() -> bool {
    std::env::var_os(STATS_ENV).is_some_and(|v| v != "0")
}

/// Summary of the frames recorded so far.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PacingSummary {
    pub frames: usize,
    pub interval_p50_ms: f32,
    pub interval_p95_ms: f32,
    pub interval_p99_ms: f32,
    pub interval_max_ms: f32,
    pub build_p99_ms: f32,
    pub late: usize,
    /// Seconds after the first frame at which the first late frames ended.
    pub late_at_s: [Option<f32>; LATE_LISTED],
}

impl std::fmt::Display for PacingSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} frames, interval p50 {:.2} ms / p95 {:.2} / p99 {:.2} / max {:.2}, {} late (> {LATE_FACTOR}× median), build p99 {:.2} ms",
            self.frames, self.interval_p50_ms, self.interval_p95_ms, self.interval_p99_ms, self.interval_max_ms, self.late, self.build_p99_ms
        )?;
        for (i, at) in self.late_at_s.iter().flatten().enumerate() {
            write!(f, "{}{at:.2}s", if i == 0 { "; late at " } else { ", " })?;
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct Pacing {
    first_start: Option<Instant>,
    last_start: Option<Instant>,
    /// `(seconds since the first frame, interval ms)`.
    intervals_ms: Vec<(f32, f32)>,
    build_ms: Vec<f32>,
}

/// `q`-quantile of an ascending slice (nearest rank).
fn quantile(sorted: &[f32], q: f32) -> f32 {
    match sorted.len() {
        0 => 0.0,
        n => sorted[((n - 1) as f32 * q).round() as usize],
    }
}

impl Pacing {
    pub fn new() -> Self {
        Self { first_start: None, last_start: None, intervals_ms: Vec::with_capacity(CAPACITY), build_ms: Vec::with_capacity(CAPACITY) }
    }

    /// Records a render that started at `start` and took `build`.
    pub fn record(&mut self, start: Instant, build: Duration) {
        if self.intervals_ms.len() >= CAPACITY {
            self.intervals_ms.drain(..CAPACITY / 2);
        }
        if self.build_ms.len() >= CAPACITY {
            self.build_ms.drain(..CAPACITY / 2);
        }
        let first = *self.first_start.get_or_insert(start);
        if let Some(prev) = self.last_start.replace(start) {
            let gap = start.saturating_duration_since(prev);
            if gap <= CONTINUOUS {
                self.intervals_ms.push((start.saturating_duration_since(first).as_secs_f32(), gap.as_secs_f32() * MS_PER_S));
            }
        }
        self.build_ms.push(build.as_secs_f32() * MS_PER_S);
    }

    pub fn summary(&self) -> PacingSummary {
        let mut iv: Vec<f32> = self.intervals_ms.iter().map(|(_, ms)| *ms).collect();
        iv.sort_by(f32::total_cmp);
        let mut build = self.build_ms.clone();
        build.sort_by(f32::total_cmp);
        let p50 = quantile(&iv, P50);
        let mut late_at_s = [None; LATE_LISTED];
        for (slot, (at, _)) in late_at_s.iter_mut().zip(self.intervals_ms.iter().filter(|(_, ms)| *ms > p50 * LATE_FACTOR)) {
            *slot = Some(*at);
        }
        PacingSummary {
            frames: self.build_ms.len(),
            interval_p50_ms: p50,
            interval_p95_ms: quantile(&iv, P95),
            interval_p99_ms: quantile(&iv, P99),
            interval_max_ms: iv.last().copied().unwrap_or(0.0),
            build_p99_ms: quantile(&build, P99),
            late: iv.iter().filter(|i| **i > p50 * LATE_FACTOR).count(),
            late_at_s,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_late_frames_and_ignores_idle_gaps() {
        let mut p = Pacing::new();
        let t0 = Instant::now();
        let step = Duration::from_micros(6944);
        let mut t = t0;
        for i in 0..100 {
            // One 3× frame; one idle gap.
            t += if i == 50 { step * 3 } else if i == 70 { Duration::from_millis(500) } else { step };
            p.record(t, Duration::from_millis(1));
        }
        let s = p.summary();
        assert_eq!(s.frames, 100);
        assert_eq!(s.late, 1);
        assert!((s.interval_p50_ms - 6.944).abs() < 0.01);
    }
}
