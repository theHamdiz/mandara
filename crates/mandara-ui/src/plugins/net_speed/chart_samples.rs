//! Charts borrow the sampler's history instead of retaining a second copy.

use std::collections::VecDeque;

use mandara_services::net_speed::RateHistory;

/// Selects the live window without copying samples, including wrapped deques.
pub(super) fn chart_samples(
    history: Option<&RateHistory>,
    long: bool,
) -> Option<&VecDeque<(f64, f64)>> {
    history.map(|history| {
        if long {
            &history.buckets
        } else {
            &history.seconds
        }
    })
}

/// Pads missing older columns without materializing a contiguous sample buffer.
pub(super) fn chart_columns(
    samples: &VecDeque<(f64, f64)>,
    capacity: usize,
) -> impl Iterator<Item = Option<(f64, f64)>> + '_ {
    std::iter::repeat_n(None, capacity.saturating_sub(samples.len()))
        .chain(samples.iter().copied().map(Some))
        .take(capacity)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Window changes expose the original samples in chronological order.
    #[test]
    fn chart_window_borrows_wrapped_history_and_preserves_fractional_rates() {
        let mut history = RateHistory::default();
        // Fill the sampler's reserved window before crossing its ring boundary.
        history.seconds.resize(history.seconds.capacity(), (0.0, 0.0));
        while history.seconds.len() > 2 {
            history.seconds.pop_front();
        }
        history.seconds[0] = (3.75, 4.5);
        history.seconds[1] = (5.25, 6.75);
        history.seconds.push_back((7.5, 8.25));
        history.buckets.push_back((0.125, 0.0625));
        assert!(
            !history.seconds.as_slices().1.is_empty(),
            "fixture crosses the deque boundary"
        );
        let short = chart_samples(Some(&history), false).unwrap();
        assert!(std::ptr::eq(short, &history.seconds));
        assert_eq!(
            short.iter().copied().collect::<Vec<_>>(),
            [(3.75, 4.5), (5.25, 6.75), (7.5, 8.25)]
        );
        let long = chart_samples(Some(&history), true).unwrap();
        assert!(std::ptr::eq(long, &history.buckets));
        assert_eq!(long.front(), Some(&(0.125, 0.0625)));
        assert!(chart_samples(None, true).is_none());
        assert_eq!(
            chart_columns(short, 5).collect::<Vec<_>>(),
            [
                None,
                None,
                Some((3.75, 4.5)),
                Some((5.25, 6.75)),
                Some((7.5, 8.25))
            ]
        );
        assert_eq!(
            chart_columns(short, 2).collect::<Vec<_>>(),
            [Some((3.75, 4.5)), Some((5.25, 6.75))]
        );
        assert!(chart_columns(short, 0).next().is_none());
    }
}
