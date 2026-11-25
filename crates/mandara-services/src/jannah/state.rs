//! What Jannah remembers between sessions, in
//! `~/.local/state/mandara/jannah.json`: saved reminders (newest first),
//! the days a step was done and the last day the morning reminder went
//! out. Small and written whole (atomically) off the UI thread. (What was
//! shown lately lives in its own small file: [`super::recent`].)

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use chrono::{Datelike as _, Duration, NaiveDate};
use serde::{Deserialize, Serialize};

/// Done days older than this are forgotten (only this week is shown).
const KEEP_DONE_DAYS: i64 = 400;
/// Most saved reminders kept.
const MAX_SAVED: usize = 2000;
const DATE_FORMAT: &str = "%Y-%m-%d";
const DAYS_PER_WEEK: i64 = 7;

/// One saved reminder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    /// `quran-2:25` / `bukhari-3245`.
    pub key: String,
    /// When it was saved (Unix seconds).
    pub at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// Newest first.
    pub saved: Vec<Saved>,
    /// Days a step was done (`YYYY-MM-DD`).
    pub done: BTreeSet<String>,
    /// The last day the morning reminder was sent.
    pub reminded: Option<String>,
}

/// `~/.local/state/mandara/jannah.json`.
pub fn path() -> PathBuf {
    mandara_config::paths::state_dir().join("jannah.json")
}

fn day_key(d: NaiveDate) -> String {
    d.format(DATE_FORMAT).to_string()
}

impl State {
    /// The state at `path` (empty when missing or unreadable).
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    /// Writes the state to `path` (atomically).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        mandara_config::store::write_atomic(path, &json).map_err(|e| format!("{}: {e:#}", path.display()))
    }

    pub fn is_saved(&self, key: &str) -> bool {
        self.saved.iter().any(|s| s.key == key)
    }

    /// Saves `key` (first in the list); `false` when it already was.
    pub fn save_item(&mut self, key: &str, now: i64) -> bool {
        if self.is_saved(key) {
            return false;
        }
        self.saved.insert(0, Saved { key: key.to_string(), at: now });
        self.saved.truncate(MAX_SAVED);
        true
    }

    /// Removes `key`; `false` when it wasn't saved.
    pub fn unsave(&mut self, key: &str) -> bool {
        let before = self.saved.len();
        self.saved.retain(|s| s.key != key);
        self.saved.len() != before
    }

    /// Saves or removes `key`; whether it is saved now.
    pub fn toggle_saved(&mut self, key: &str, now: i64) -> bool {
        if !self.unsave(key) {
            self.save_item(key, now);
            return true;
        }
        false
    }

    pub fn done_on(&self, day: NaiveDate) -> bool {
        self.done.contains(&day_key(day))
    }

    /// Marks `day`'s step done (forgetting very old days); `false` when it
    /// already was.
    pub fn mark_done(&mut self, day: NaiveDate) -> bool {
        let added = self.done.insert(day_key(day));
        let oldest = day_key(day - Duration::days(KEEP_DONE_DAYS));
        self.done.retain(|d| *d >= oldest);
        added
    }

    /// The seven days of `day`'s week (starting `week_start` days after
    /// Monday) and whether a step was done on each.
    pub fn week(&self, day: NaiveDate, week_start: u32) -> [(NaiveDate, bool); 7] {
        let into = (i64::from(day.weekday().num_days_from_monday()) - i64::from(week_start)).rem_euclid(DAYS_PER_WEEK);
        let first = day - Duration::days(into);
        std::array::from_fn(|i| {
            let d = first + Duration::days(i as i64);
            (d, self.done_on(d))
        })
    }

    /// Whether the morning reminder already went out on `day`.
    pub fn reminded_on(&self, day: NaiveDate) -> bool {
        self.reminded.as_deref() == Some(day_key(day).as_str())
    }

    pub fn set_reminded(&mut self, day: NaiveDate) {
        self.reminded = Some(day_key(day));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, d).unwrap()
    }

    #[test]
    fn saved_items_are_newest_first_and_unique() {
        let mut s = State::default();
        assert!(s.save_item("quran-2:25", 10));
        assert!(s.save_item("bukhari-3245", 20));
        assert!(!s.save_item("quran-2:25", 30));
        assert_eq!(s.saved.iter().map(|x| x.key.as_str()).collect::<Vec<_>>(), ["bukhari-3245", "quran-2:25"]);
        assert!(!s.toggle_saved("bukhari-3245", 40));
        assert!(!s.is_saved("bukhari-3245"));
        assert!(s.toggle_saved("bukhari-3245", 50));
        assert!(s.unsave("quran-2:25") && !s.unsave("quran-2:25"));
    }

    #[test]
    fn done_days_and_the_week() {
        let mut s = State::default();
        // 2 Nov 2026 is a Monday.
        assert!(s.mark_done(day(2)) && !s.mark_done(day(2)));
        s.mark_done(day(4));
        let week = s.week(day(5), 0);
        assert_eq!(week[0], (day(2), true));
        assert_eq!(week.iter().filter(|(_, done)| *done).count(), 2);
        // A week starting on Saturday (5 days after Monday).
        let week = s.week(day(5), 5);
        assert_eq!(week[0].0, NaiveDate::from_ymd_opt(2026, 10, 31).unwrap());
        assert!(week[2].1 && week[4].1);
        // Very old days are forgotten.
        s.mark_done(day(2) + Duration::days(KEEP_DONE_DAYS + 3));
        assert!(!s.done_on(day(2)));
    }

    #[test]
    fn the_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("jannah.json");
        assert_eq!(State::load(&path), State::default());
        let mut s = State::default();
        s.save_item("muslim-2699", 7);
        s.mark_done(day(3));
        s.set_reminded(day(3));
        s.save(&path).unwrap();
        let back = State::load(&path);
        assert_eq!(back, s);
        assert!(back.reminded_on(day(3)) && !back.reminded_on(day(4)));
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(State::load(&path), State::default());
        // Unknown and missing fields are fine (`shift` is from before
        // reminders were picked at random).
        std::fs::write(&path, r#"{"shift": 2, "future": true, "reminded": "2026-11-03"}"#).unwrap();
        assert!(State::load(&path).reminded_on(day(3)));
    }
}
