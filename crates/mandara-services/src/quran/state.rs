//! What the Qur'an plugin remembers, in `~/.local/state/mandara/quran.json`:
//! where you last read, how far this khatm has come, the khatms completed,
//! your bookmarks (newest first) and the reader's mode. Small, and written
//! whole (atomically) off the UI thread.
//!
//! A khatm moves forward only by reading on: a position counts when it is
//! at most [`ADVANCE_WINDOW`] ayahs past the furthest one so far, so
//! opening the last surah to look something up doesn't complete it.
//! Reaching the last ayah completes it and the next khatm starts.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::text::{AYAHS, Ref};

/// How far past the furthest ayah a position may be and still count.
pub const ADVANCE_WINDOW: u16 = 50;
/// Most bookmarks kept.
const MAX_BOOKMARKS: usize = 1000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    #[serde(rename = "ref")]
    pub at_ref: Ref,
    /// When it was added (Unix seconds).
    pub at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// Where you last read (global number), and when.
    pub last: Option<u16>,
    pub last_at: i64,
    /// The furthest ayah (global number) of this khatm.
    pub furthest: u16,
    /// Khatms completed.
    pub khatms: u32,
    /// Newest first.
    pub bookmarks: Vec<Bookmark>,
    /// The reader shows each ayah with its meaning (else flowing Arabic).
    pub study: bool,
}

/// `~/.local/state/mandara/quran.json`.
pub fn path() -> PathBuf {
    mandara_config::paths::state_dir().join("quran.json")
}

impl State {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        mandara_config::store::write_atomic(path, &json).map_err(|e| format!("{}: {e:#}", path.display()))
    }

    /// Reading reached global ayah `g` at `now`: the last-read place moves
    /// there and the khatm forward when `g` follows on. Returns whether a
    /// khatm was just completed.
    pub fn read_at(&mut self, g: u16, now: i64) -> bool {
        if !(1..=AYAHS).contains(&g) {
            return false;
        }
        self.last = Some(g);
        self.last_at = now;
        if g > self.furthest && g <= self.furthest.saturating_add(ADVANCE_WINDOW) {
            self.furthest = g;
            if g == AYAHS {
                self.khatms += 1;
                self.furthest = 0;
                return true;
            }
        }
        false
    }

    /// This khatm's progress, 0–1.
    pub fn progress(&self) -> f32 {
        f32::from(self.furthest) / f32::from(AYAHS)
    }

    pub fn is_bookmarked(&self, r: Ref) -> bool {
        self.bookmarks.iter().any(|b| b.at_ref == r)
    }

    /// Adds a bookmark (first in the list); `false` when it already is one.
    pub fn bookmark(&mut self, r: Ref, now: i64) -> bool {
        if self.is_bookmarked(r) {
            return false;
        }
        self.bookmarks.insert(0, Bookmark { at_ref: r, at: now });
        self.bookmarks.truncate(MAX_BOOKMARKS);
        true
    }

    /// Removes a bookmark; `false` when there was none.
    pub fn unbookmark(&mut self, r: Ref) -> bool {
        let before = self.bookmarks.len();
        self.bookmarks.retain(|b| b.at_ref != r);
        self.bookmarks.len() != before
    }

    /// Adds or removes a bookmark; whether it is one now.
    pub fn toggle_bookmark(&mut self, r: Ref, now: i64) -> bool {
        if self.unbookmark(r) {
            return false;
        }
        self.bookmark(r, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_khatm_moves_forward_by_reading_on() {
        let mut s = State::default();
        assert!(!s.read_at(1, 10));
        assert_eq!((s.last, s.furthest), (Some(1), 1));
        // Reading on, a page at a time.
        for g in (20..=400).step_by(20) {
            s.read_at(g, 11);
        }
        assert_eq!(s.furthest, 400);
        // A look at the last surah moves the place, not the khatm.
        s.read_at(6230, 12);
        assert_eq!((s.last, s.furthest), (Some(6230), 400));
        // Going back doesn't undo it.
        s.read_at(100, 13);
        assert_eq!(s.furthest, 400);
        assert!((s.progress() - 400.0 / 6236.0).abs() < 1e-6);
        assert!(!s.read_at(0, 14) && !s.read_at(AYAHS + 1, 14));
        assert_eq!(s.last, Some(100));
    }

    #[test]
    fn reaching_the_end_completes_a_khatm() {
        let mut s = State { furthest: AYAHS - 10, ..State::default() };
        assert!(s.read_at(AYAHS, 5));
        assert_eq!((s.khatms, s.furthest, s.last), (1, 0, Some(AYAHS)));
        assert_eq!(s.progress(), 0.0);
        // Reading the end again doesn't count twice.
        assert!(!s.read_at(AYAHS, 6));
        assert_eq!(s.khatms, 1);
    }

    #[test]
    fn bookmarks_are_newest_first_and_unique() {
        let mut s = State::default();
        assert!(s.bookmark(Ref::new(2, 255), 1));
        assert!(s.bookmark(Ref::new(36, 1), 2));
        assert!(!s.bookmark(Ref::new(2, 255), 3));
        assert_eq!(s.bookmarks.iter().map(|b| b.at_ref).collect::<Vec<_>>(), [Ref::new(36, 1), Ref::new(2, 255)]);
        assert!(!s.toggle_bookmark(Ref::new(36, 1), 4));
        assert!(s.toggle_bookmark(Ref::new(36, 1), 5));
        assert!(s.unbookmark(Ref::new(2, 255)) && !s.unbookmark(Ref::new(2, 255)));
    }

    #[test]
    fn the_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("quran.json");
        assert_eq!(State::load(&path), State::default());
        let mut s = State { study: true, khatms: 2, ..State::default() };
        s.read_at(30, 9);
        s.bookmark(Ref::new(1, 5), 7);
        s.save(&path).unwrap();
        assert_eq!(State::load(&path), s);
        std::fs::write(&path, r#"{"last": 12, "future": 1}"#).unwrap();
        assert_eq!(State::load(&path).last, Some(12));
        std::fs::write(&path, "nope").unwrap();
        assert_eq!(State::load(&path), State::default());
    }
}
