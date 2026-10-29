//! Scheduling helpers for "a new wallpaper every …": pure functions of the
//! clock, so the caller owns the timer.

use std::time::{Duration, SystemTime};

/// When the next wallpaper is due.
///
/// * Never generated: due now.
/// * Otherwise one `interval` after the last one; if that moment has
///   already passed (the machine slept, the shell was off), due now, never
///   a burst of catch-up runs.
/// * A `last` in the future (the clock went backwards) counts as now, so a
///   clock change can't postpone the schedule by more than one interval.
pub fn next_due(last: Option<SystemTime>, interval: Duration, now: SystemTime) -> SystemTime {
    let Some(last) = last else { return now };
    let last = last.min(now);
    let due = last.checked_add(interval).unwrap_or(now);
    due.max(now)
}

/// Whether a wallpaper is due at `now`.
pub fn is_due(last: Option<SystemTime>, interval: Duration, now: SystemTime) -> bool {
    next_due(last, interval, now) <= now
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_times() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let hour = Duration::from_secs(3600);
        assert_eq!(next_due(None, hour, t0), t0);
        assert!(is_due(None, hour, t0));
        // Generated 10 minutes ago: due 50 minutes from now.
        let ten = Duration::from_secs(600);
        assert_eq!(next_due(Some(t0 - ten), hour, t0), t0 + hour - ten);
        assert!(!is_due(Some(t0 - ten), hour, t0));
        // Overdue by days: due now, once.
        assert_eq!(next_due(Some(t0 - hour * 50), hour, t0), t0);
        // Exactly one interval ago: due now.
        assert!(is_due(Some(t0 - hour), hour, t0));
        // Clock went backwards: treated as generated now.
        assert_eq!(next_due(Some(t0 + hour * 5), hour, t0), t0 + hour);
    }
}
