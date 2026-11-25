//! When to try a dropped stream again: quickly at first, then less and
//! less often, up to a cap, and only a bounded number of times. A
//! connection that played for a while starts the count afresh.

use std::time::Duration;

/// The first retry.
pub const FIRST: Duration = Duration::from_millis(500);
/// The longest wait between attempts.
pub const CAP: Duration = Duration::from_secs(30);
/// Playing this long counts as a healthy connection (resets the count).
pub const HEALTHY: Duration = Duration::from_secs(30);
/// Attempts allowed by default before giving up.
pub const DEFAULT_ATTEMPTS: u32 = 8;
/// The attempts setting's bounds.
pub const ATTEMPTS_RANGE: (u32, u32) = (1, 30);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backoff {
    attempt: u32,
    max: u32,
    /// Per-session seed so several shells don't retry in lockstep.
    seed: u32,
}

impl Backoff {
    pub fn new(max_attempts: u32, seed: u32) -> Self {
        Self { attempt: 0, max: max_attempts.clamp(ATTEMPTS_RANGE.0, ATTEMPTS_RANGE.1), seed: seed | 1 }
    }

    /// Attempts made since the last healthy connection.
    pub fn attempts(&self) -> u32 {
        self.attempt
    }

    /// The wait before the next attempt, or `None` when the attempts are
    /// used up. Doubles from [`FIRST`] up to [`CAP`], with up to a quarter
    /// of jitter.
    pub fn next_delay(&mut self) -> Option<Duration> {
        if self.attempt >= self.max {
            return None;
        }
        let base = FIRST.saturating_mul(1u32 << self.attempt.min(16)).min(CAP);
        self.attempt += 1;
        // xorshift: deterministic jitter in [0, base / 4).
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        let jitter = base.mul_f64(f64::from(x % 1000) / 4000.0);
        Some((base + jitter).min(CAP))
    }

    /// A connection played long enough: start counting again.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_grow_to_the_cap_and_stop() {
        let mut b = Backoff::new(10, 7);
        let delays: Vec<Duration> = std::iter::from_fn(|| b.next_delay()).collect();
        assert_eq!(delays.len(), 10);
        assert!(delays[0] >= FIRST && delays[0] < FIRST.mul_f64(1.25));
        for w in delays.windows(2) {
            assert!(w[1] >= w[0].min(CAP.mul_f64(0.8)), "{delays:?}");
        }
        assert!(delays.iter().all(|d| *d <= CAP));
        assert_eq!(*delays.last().unwrap(), CAP.min(*delays.last().unwrap()));
        assert!(delays[7..].iter().all(|d| *d >= CAP.mul_f64(0.99)), "long outages wait the cap: {delays:?}");
        assert_eq!(b.next_delay(), None, "attempts are bounded");
        assert_eq!(b.attempts(), 10);
        b.reset();
        assert!(b.next_delay().unwrap() < Duration::from_secs(1), "a healthy connection starts over");
    }

    #[test]
    fn attempts_are_clamped_and_jitter_differs() {
        assert_eq!(std::iter::from_fn({
            let mut b = Backoff::new(0, 1);
            move || b.next_delay()
        })
        .count(), ATTEMPTS_RANGE.0 as usize);
        assert_eq!(std::iter::from_fn({
            let mut b = Backoff::new(1_000, 1);
            move || b.next_delay()
        })
        .count(), ATTEMPTS_RANGE.1 as usize);
        let a = Backoff::new(5, 1).next_delay();
        let b = Backoff::new(5, 99).next_delay();
        assert_ne!(a, b, "different seeds spread the first retry");
    }
}
