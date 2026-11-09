//! A play clock that only runs while someone can see the game: time banked
//! from earlier runs plus the current run.

use std::time::Instant;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct PlayClock {
    banked_ms: u64,
    since: Option<Instant>,
}

impl PlayClock {
    pub fn starting_at(ms: u64) -> Self {
        Self { banked_ms: ms, since: None }
    }

    pub fn running(&self) -> bool {
        self.since.is_some()
    }

    pub fn elapsed_ms(&self, now: Instant) -> u64 {
        self.banked_ms + self.since.map_or(0, |s| now.saturating_duration_since(s).as_millis() as u64)
    }

    pub fn resume(&mut self, now: Instant) {
        if self.since.is_none() {
            self.since = Some(now);
        }
    }

    pub fn pause(&mut self, now: Instant) {
        self.banked_ms = self.elapsed_ms(now);
        self.since = None;
    }

    /// Runs (or stops) as `on` says.
    pub fn set(&mut self, on: bool, now: Instant) {
        if on { self.resume(now) } else { self.pause(now) }
    }
}

/// `m:ss`, or `h:mm:ss` past an hour (tabular digits keep it steady).
pub fn format_ms(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_clock_banks_time_only_while_running() {
        let t0 = Instant::now();
        let mut c = PlayClock::starting_at(1_000);
        assert_eq!(c.elapsed_ms(t0 + Duration::from_secs(9)), 1_000);
        c.resume(t0);
        c.resume(t0 + Duration::from_secs(1));
        assert_eq!(c.elapsed_ms(t0 + Duration::from_secs(2)), 3_000);
        c.pause(t0 + Duration::from_secs(2));
        assert_eq!(c.elapsed_ms(t0 + Duration::from_secs(60)), 3_000);
        c.set(true, t0 + Duration::from_secs(60));
        assert!(c.running());
        assert_eq!(c.elapsed_ms(t0 + Duration::from_secs(61)), 4_000);
    }

    #[test]
    fn times_read_like_a_stopwatch() {
        assert_eq!(format_ms(0), "0:00");
        assert_eq!(format_ms(65_400), "1:05");
        assert_eq!(format_ms(3_725_000), "1:02:05");
    }
}
