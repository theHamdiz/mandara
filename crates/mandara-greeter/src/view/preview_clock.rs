//! Clock-boundary wakes for a visible, otherwise stationary preview.

use std::time::Duration;

/// Keep an existing wake at the same cadence; changing seconds replaces it.
pub(super) fn plan(millis: i64, seconds: bool, pending: Option<bool>) -> Option<Duration> {
    if pending == Some(seconds) { return None; }
    /// Clock boundaries use wall time, not time since the last interaction.
    const MILLIS_PER_SECOND: i64 = 1000;
    const SECONDS_PER_MINUTE: i64 = 60;
    let period = MILLIS_PER_SECOND * if seconds { 1 } else { SECONDS_PER_MINUTE };
    Some(Duration::from_millis((period - millis.rem_euclid(period)) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wakes_follow_the_next_displayed_clock_boundary() {
        assert_eq!(plan(59_900, false, None), Some(Duration::from_millis(100)));
        assert_eq!(plan(60_000, false, None), Some(Duration::from_secs(60)));
        assert_eq!(plan(1_250, true, None), Some(Duration::from_millis(750)));
    }

    #[test]
    fn enabling_seconds_replaces_a_pending_minute_wake() {
        assert_eq!(plan(10_500, false, None), Some(Duration::from_millis(49_500)));
        assert_eq!(plan(10_500, true, Some(false)), Some(Duration::from_millis(500)));
        assert_eq!(plan(10_500, true, Some(true)), None);
        assert_eq!(plan(10_500, false, Some(true)), Some(Duration::from_millis(49_500)));
        assert_eq!(plan(10_500, false, Some(false)), None);
    }
}
