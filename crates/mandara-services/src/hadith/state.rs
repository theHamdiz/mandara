//! What the Hadith plugin remembers, in
//! `~/.local/state/mandara/hadith.json`: bookmarks (newest first), where
//! you were in each collection and last of all, which hadith you have
//! read (a bitset per collection, for the progress tile), the collections
//! you removed yourself (so they aren't downloaded again on their own),
//! and today's card (so the bar, the sidebar and the desktop show it
//! without loading a collection). Small; written whole and atomically,
//! off the UI thread.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::collections::Collection;
use super::grade::GradeClass;

/// Most bookmarks kept.
const MAX_BOOKMARKS: usize = 5000;
/// Bits per hex digit.
const NIBBLE: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    /// `bukhari:774b`.
    pub key: String,
    /// When it was added (Unix seconds).
    pub at: i64,
}

/// Where you were in a collection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub key: String,
    pub at: i64,
}

/// A grade on today's card.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardGrade {
    pub class: GradeClass,
    /// The grade's words and its grader, as given (empty for the two
    /// Sahih collections).
    pub grade: String,
    pub grader: String,
}

/// Everything a small surface shows of a hadith.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Card {
    pub key: String,
    pub collection: Collection,
    /// The sunnah.com reference (`774b`).
    pub reference: String,
    pub url: String,
    /// The words without the chain, and the whole Arabic.
    pub words: String,
    pub arabic: String,
    /// The meaning without an opening "Narrated X:", and verbatim.
    pub meaning: Option<String>,
    pub meaning_full: Option<String>,
    /// English standing in for the interface language.
    pub english: bool,
    pub narrator: Option<String>,
    pub grade: Option<CardGrade>,
    /// The book's number and English name.
    pub book: u32,
    pub book_name: String,
}

impl Card {
    /// The card of hadith `index` of `lib`.
    pub fn of(lib: &super::data::Library, index: u32) -> Option<Self> {
        let h = lib.get(index)?;
        let c = lib.collection;
        let book = lib.books.get(h.book as usize)?;
        let grade = match (super::collections::info(c).sahih, h.lead_grade()) {
            (true, _) => Some(CardGrade { class: GradeClass::Sahih, grade: String::new(), grader: String::new() }),
            (false, Some(g)) => Some(CardGrade { class: g.class, grade: g.grade.clone(), grader: g.grader.clone() }),
            (false, None) => None,
        };
        Some(Card {
            key: lib.key(index)?,
            collection: c,
            reference: h.reference.clone(),
            url: lib.url(index)?,
            words: h.words().to_string(),
            arabic: h.arabic.clone(),
            meaning: h.meaning_body().map(str::to_string),
            meaning_full: h.meaning.clone(),
            english: lib.meaning.as_ref().is_some_and(|m| m.fallback),
            narrator: h.narrator.clone(),
            grade,
            book: book.number,
            book_name: book.name.clone(),
        })
    }
}

/// Today's card and what it was picked for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Today {
    pub slot: i64,
    /// [`super::rotation::pool_key`].
    pub pool: String,
    pub card: Card,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// Newest first.
    pub bookmarks: Vec<Bookmark>,
    /// Collection id → where you were.
    pub places: BTreeMap<String, Place>,
    /// The last hadith you read anywhere.
    pub last: Option<Place>,
    /// Collection id → the hadith read (hex bitset over library indices).
    pub read: BTreeMap<String, String>,
    /// Collections you removed (not downloaded again on their own).
    pub declined: BTreeSet<String>,
    pub today: Option<Today>,
}

/// `~/.local/state/mandara/hadith.json`.
pub fn path() -> PathBuf {
    mandara_config::paths::state_dir().join("hadith.json")
}

/// Whether bit `i` is set in hex bitset `hex`.
fn bit(hex: &str, i: usize) -> bool {
    hex.as_bytes().get(i / NIBBLE).and_then(|b| char::from(*b).to_digit(16)).is_some_and(|d| d & (1 << (i % NIBBLE)) != 0)
}

/// `hex` with bit `i` set.
fn set_bit(hex: &mut String, i: usize) {
    let at = i / NIBBLE;
    while hex.len() <= at {
        hex.push('0');
    }
    let d = char::from(hex.as_bytes()[at]).to_digit(16).unwrap_or(0) | (1 << (i % NIBBLE));
    if let Some(c) = char::from_digit(d, 16) {
        hex.replace_range(at..=at, &c.to_string());
    }
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

    pub fn is_bookmarked(&self, key: &str) -> bool {
        self.bookmarks.iter().any(|b| b.key == key)
    }

    /// Bookmarks `key` (first); `false` when it already was.
    pub fn bookmark(&mut self, key: &str, now: i64) -> bool {
        if self.is_bookmarked(key) {
            return false;
        }
        self.bookmarks.insert(0, Bookmark { key: key.to_string(), at: now });
        self.bookmarks.truncate(MAX_BOOKMARKS);
        true
    }

    /// Removes `key`'s bookmark; `false` when there was none.
    pub fn unbookmark(&mut self, key: &str) -> bool {
        let before = self.bookmarks.len();
        self.bookmarks.retain(|b| b.key != key);
        self.bookmarks.len() != before
    }

    /// Bookmarks or un-bookmarks `key`; whether it is bookmarked now.
    pub fn toggle_bookmark(&mut self, key: &str, now: i64) -> bool {
        if self.unbookmark(key) {
            return false;
        }
        self.bookmark(key, now)
    }

    /// Remembers reading `key` (library `index` of `c`).
    pub fn visit(&mut self, c: Collection, index: u32, key: &str, now: i64) {
        let place = Place { key: key.to_string(), at: now };
        self.places.insert(c.id().to_string(), place.clone());
        self.last = Some(place);
        set_bit(self.read.entry(c.id().to_string()).or_default(), index as usize);
    }

    pub fn is_read(&self, c: Collection, index: u32) -> bool {
        self.read.get(c.id()).is_some_and(|h| bit(h, index as usize))
    }

    /// How many hadith of `c` were read.
    pub fn read_count(&self, c: Collection) -> usize {
        self.read.get(c.id()).map_or(0, |h| h.chars().filter_map(|d| d.to_digit(16)).map(|d| d.count_ones() as usize).sum())
    }

    /// Remembers that you removed `c` (its progress is kept).
    pub fn removed(&mut self, c: Collection) {
        self.declined.insert(c.id().to_string());
    }

    /// Whether you removed `c` yourself.
    pub fn declined(&self, c: Collection) -> bool {
        self.declined.contains(c.id())
    }

    /// You downloaded `c` again.
    pub fn wanted(&mut self, c: Collection) {
        self.declined.remove(c.id());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmarks_are_newest_first_and_unique() {
        let mut s = State::default();
        assert!(s.bookmark("bukhari:1", 10));
        assert!(s.bookmark("muslim:2699a", 20));
        assert!(!s.bookmark("bukhari:1", 30));
        assert_eq!(s.bookmarks.iter().map(|b| b.key.as_str()).collect::<Vec<_>>(), ["muslim:2699a", "bukhari:1"]);
        assert!(!s.toggle_bookmark("muslim:2699a", 40));
        assert!(s.toggle_bookmark("muslim:2699a", 50));
        assert!(s.unbookmark("bukhari:1") && !s.unbookmark("bukhari:1"));
    }

    #[test]
    fn reading_is_counted_per_collection() {
        let mut s = State::default();
        for i in [0, 3, 4, 3, 1000, 7579] {
            s.visit(Collection::Bukhari, i, &format!("bukhari:{}", i + 1), 5);
        }
        assert_eq!(s.read_count(Collection::Bukhari), 5);
        assert!(s.is_read(Collection::Bukhari, 1000) && !s.is_read(Collection::Bukhari, 999));
        assert_eq!(s.read_count(Collection::Muslim), 0);
        assert_eq!(s.places.get("bukhari").map(|p| p.key.as_str()), Some("bukhari:7580"));
        assert_eq!(s.last.as_ref().map(|p| p.key.as_str()), Some("bukhari:7580"));
        s.removed(Collection::Muslim);
        assert!(s.declined(Collection::Muslim));
        s.wanted(Collection::Muslim);
        assert!(!s.declined(Collection::Muslim));
    }

    #[test]
    fn the_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("hadith.json");
        assert_eq!(State::load(&path), State::default());
        let mut s = State::default();
        s.bookmark("bukhari:1", 7);
        s.visit(Collection::Muslim, 12, "muslim:8a", 9);
        s.today = Some(Today { slot: 3, pool: "bukhari,muslim|eng".into(), card: Card { key: "bukhari:1".into(), grade: Some(CardGrade { class: GradeClass::Sahih, grade: String::new(), grader: String::new() }), ..Card::default() } });
        s.save(&path).unwrap();
        assert_eq!(State::load(&path), s);
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(State::load(&path), State::default());
        std::fs::write(&path, r#"{"future": 1, "declined": ["malik"]}"#).unwrap();
        assert!(State::load(&path).declined(Collection::Malik));
    }
}
