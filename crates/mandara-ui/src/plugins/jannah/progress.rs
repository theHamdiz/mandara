//! Progress labels change with saved state, the calendar day and the week layout.

use chrono::NaiveDate;
use mandara_services::jannah::state::State;

#[derive(Copy, Clone)]
pub(super) struct Progress {
    pub done: bool,
    pub week: [(NaiveDate, bool); 7],
}

#[derive(Copy, Clone)]
pub(super) struct DayProgress {
    pub done: bool,
    pub reminded: bool,
}

/// Day flags and week layout have separate keys so a timer cannot evict a view's week.
#[derive(Default)]
pub(super) struct ProgressCache {
    day: Option<((u64, NaiveDate), DayProgress)>,
    week: Option<((u64, NaiveDate, u32), [(NaiveDate, bool); 7])>,
}

impl ProgressCache {
    /// The model bumps `generation` whenever its saved state changes.
    pub fn day(&mut self, state: &State, generation: u64, date: NaiveDate) -> DayProgress {
        let key = (generation, date);
        if let Some((previous, progress)) = self.day
            && previous == key
        {
            return progress;
        }
        let progress = DayProgress {
            done: state.done_on(date),
            reminded: state.reminded_on(date),
        };
        self.day = Some((key, progress));
        progress
    }

    /// Week-start edits rebuild only the week, leaving daily reminder lookups warm.
    pub fn get(
        &mut self,
        state: &State,
        generation: u64,
        date: NaiveDate,
        week_start: u32,
    ) -> Progress {
        let day = self.day(state, generation, date);
        let key = (generation, date, week_start);
        let week = match self.week {
            Some((previous, week)) if previous == key => week,
            _ => {
                let week = state.week(date, week_start);
                self.week = Some((key, week));
                week
            }
        };
        Progress {
            done: day.done,
            week,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Saved progress, midnight and week-start edits each invalidate the snapshot.
    #[test]
    fn progress_tracks_saved_state_and_calendar_boundaries() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        let mut state = State::default();
        let mut cache = ProgressCache::default();
        let first = cache.get(&state, 0, date, 0);
        assert!(!first.done && !cache.day(&state, 0, date).reminded);
        assert_eq!(first.week, state.week(date, 0));
        state.mark_done(date);
        state.set_reminded(date);
        let changed = cache.get(&state, 1, date, 0);
        assert!(changed.done && cache.day(&state, 1, date).reminded);
        assert_eq!(changed.week, state.week(date, 0));
        let next = date.succ_opt().unwrap();
        let tomorrow = cache.get(&state, 1, next, 0);
        assert!(!tomorrow.done && !cache.day(&state, 1, next).reminded);
        assert_eq!(tomorrow.week, state.week(next, 0));
        let saturday = cache.get(&state, 1, next, 5);
        assert_eq!(saturday.week, state.week(next, 5));
        assert_ne!(saturday.week[0].0, tomorrow.week[0].0);
    }
}
