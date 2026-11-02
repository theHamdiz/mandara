//! One collection, parsed and checked: the dataset's edition files
//! (`ara-<id>.min.json` and a translation) become a [`Library`] of books
//! and hadith with their sunnah.com references, grades, the words set apart
//! from the chain, the meaning and (from an English meaning) the narrator.
//!
//! Checks before anything is used: the JSON's shape, the number of entries
//! (the dataset's `info.json` count), strictly rising numbers, the Arabic
//! present for about as many hadith as expected, and a translation that
//! lines up with the Arabic number for number.

use std::collections::{BTreeMap, HashMap};

use mandara_i18n::tf;
use serde::Deserialize;

use super::chain;
use super::collections::{Collection, info};
use super::grade::{self, Grade, GradeClass};
use super::refs;

// ── The dataset's files ──────────────────────────────────────────────────

/// An edition file (`editions/<edition>.min.json`).
#[derive(Debug, Deserialize)]
pub struct RawEdition {
    pub metadata: RawMeta,
    pub hadiths: Vec<RawHadith>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawMeta {
    #[serde(default)]
    pub name: String,
    /// Book number → English name (`"1": "Revelation"`).
    #[serde(default)]
    pub sections: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct RawHadith {
    pub hadithnumber: f64,
    /// Muslim: Abdul Baqi's number with its narration (`"2699.01"`).
    #[serde(default)]
    pub arabicnumber: Option<serde_json::Value>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub grades: Vec<RawGrade>,
    #[serde(default)]
    pub reference: RawRef,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawRef {
    #[serde(default)]
    pub book: f64,
    #[serde(default)]
    pub hadith: f64,
}

#[derive(Debug, Deserialize)]
pub struct RawGrade {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub grade: String,
}

/// Parses an edition file.
pub fn parse(json: &str) -> Result<RawEdition, String> {
    serde_json::from_str(json).map_err(|e| tf("Unexpected hadith file: {error}", &[("error", &e)]))
}

/// The Arabic kept may fall short of the expected count by this share
/// (the dataset may fill in a gap) before the file is refused.
const KEPT_SLACK: f64 = 0.02;

/// Checks an edition of `c`: its entry count and rising numbers.
pub fn verify(c: Collection, raw: &RawEdition) -> Result<(), String> {
    let want = info(c).entries;
    if raw.hadiths.len() != want {
        return Err(tf("{edition} has {got} entries, expected {expected}", &[("edition", &raw.metadata.name), ("got", &raw.hadiths.len()), ("expected", &want)]));
    }
    if raw.hadiths.windows(2).any(|w| w[1].hadithnumber <= w[0].hadithnumber) {
        return Err(tf("{edition} is out of order", &[("edition", &raw.metadata.name)]));
    }
    Ok(())
}

/// Checks the Arabic edition: [`verify`], and enough hadith with text.
pub fn verify_arabic(c: Collection, raw: &RawEdition) -> Result<(), String> {
    verify(c, raw)?;
    let kept = raw.hadiths.iter().filter(|h| !h.text.trim().is_empty()).count();
    let want = info(c).hadith;
    if (kept as f64) < want as f64 * (1.0 - KEPT_SLACK) {
        return Err(tf("{edition} has Arabic for {got} hadith, expected {expected}", &[("edition", &raw.metadata.name), ("got", &kept), ("expected", &want)]));
    }
    Ok(())
}

/// Checks that a translation lines up with the Arabic, number for number.
pub fn verify_pair(arabic: &RawEdition, translation: &RawEdition) -> Result<(), String> {
    let aligned = arabic.hadiths.len() == translation.hadiths.len() && arabic.hadiths.iter().zip(&translation.hadiths).all(|(a, t)| a.hadithnumber == t.hadithnumber);
    if aligned { Ok(()) } else { Err(tf("{edition} doesn't line up with the Arabic", &[("edition", &translation.metadata.name)])) }
}

// ── The library ──────────────────────────────────────────────────────────

/// A collection's book (kitab), with its hadith in the collection's order.
#[derive(Debug, Clone)]
pub struct Book {
    pub number: u32,
    /// The dataset's English name (empty for an unnamed book).
    pub name: String,
    /// Indices into [`Library::hadith`].
    pub hadith: Vec<u32>,
}

/// One hadith.
#[derive(Debug, Clone)]
pub struct Hadith {
    /// The dataset's number (its order).
    pub number: f64,
    /// The sunnah.com reference (`774b`, `2699a`, `i3`).
    pub reference: String,
    /// Index into [`Library::books`].
    pub book: u32,
    /// The in-book number, when the dataset gives one.
    pub in_book: Option<u32>,
    /// The Arabic with its chain, verbatim.
    pub arabic: String,
    /// Byte range of the words (without the chain) in `arabic`.
    pub words: (u32, u32),
    pub grades: Vec<Grade>,
    /// The meaning in the downloaded translation, verbatim.
    pub meaning: Option<String>,
    /// The narrator, read from an English meaning (`Narrated Abu
    /// Huraira:`), and how many bytes of the meaning that opening takes.
    pub narrator: Option<String>,
    pub narrator_len: u32,
}

impl Hadith {
    /// The words without the chain.
    pub fn words(&self) -> &str {
        &self.arabic[self.words.0 as usize..self.words.1 as usize]
    }

    /// The chain before the words and what follows them (empty when the
    /// text has no separable chain).
    pub fn chain(&self) -> (&str, &str) {
        (self.arabic[..self.words.0 as usize].trim(), self.arabic[self.words.1 as usize..].trim())
    }

    /// The meaning without an opening "Narrated X:" (the narrator is shown
    /// on its own).
    pub fn meaning_body(&self) -> Option<&str> {
        self.meaning.as_deref().map(|m| m[(self.narrator_len as usize).min(m.len())..].trim_start())
    }

    /// The grade the hadith is badged and filtered by.
    pub fn lead_grade(&self) -> Option<&Grade> {
        grade::lead(&self.grades)
    }

    /// The lead grade's family; Bukhari and Muslim are sahih throughout.
    pub fn class(&self, c: Collection) -> Option<GradeClass> {
        if info(c).sahih { Some(GradeClass::Sahih) } else { self.lead_grade().map(|g| g.class) }
    }
}

/// The meaning the library was built with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeaningInfo {
    /// The dataset's language code (`eng`, `fra`).
    pub lang: String,
    /// The translator, when the dataset names one.
    pub translator: Option<String>,
    /// English standing in for the interface language.
    pub fallback: bool,
}

/// A collection, ready to read.
#[derive(Debug)]
pub struct Library {
    pub collection: Collection,
    pub books: Vec<Book>,
    pub hadith: Vec<Hadith>,
    pub meaning: Option<MeaningInfo>,
    by_reference: HashMap<String, u32>,
}

/// The Arabic number as text (`"2699.01"`, `8`).
fn number_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Every entry's sunnah.com reference (see [`refs`]), in order.
pub fn references(c: Collection, hadiths: &[RawHadith]) -> Vec<String> {
    // Numbers whose narrations the dataset writes with two decimals.
    let wide: std::collections::HashSet<i64> = hadiths.iter().filter(|h| h.hadithnumber.to_string().split_once('.').is_some_and(|(_, f)| f.len() == 2)).map(|h| h.hadithnumber.trunc() as i64).collect();
    let numbered = |raw: &RawHadith| match c {
        Collection::Muslim => raw.arabicnumber.as_ref().and_then(number_text).and_then(|n| refs::from_muslim(&n)),
        _ => Some(refs::from_number_in(raw.hadithnumber, wide.contains(&(raw.hadithnumber.trunc() as i64)))),
    };
    // What the dataset numbers itself: an inferred letter must not take one.
    let taken: std::collections::HashSet<String> = hadiths.iter().filter_map(numbered).collect();
    let mut previous = String::new();
    hadiths
        .iter()
        .map(|raw| {
            let (book, in_book) = (raw.reference.book as u32, raw.reference.hadith as u32);
            let reference = match numbered(raw) {
                Some(r) => Some(r),
                None if c == Collection::Muslim && book == 0 && in_book > 0 => Some(format!("{}{in_book}", refs::INTRO)),
                None => refs::next_letter(&previous).filter(|r| !taken.contains(r)),
            }
            .unwrap_or_else(|| refs::unnumbered(raw.hadithnumber));
            previous.clone_from(&reference);
            reference
        })
        .collect()
}

impl Library {
    /// Builds the library from checked editions (`translation`: the
    /// edition and what it is).
    pub fn build(c: Collection, arabic: RawEdition, translation: Option<(RawEdition, MeaningInfo)>) -> Result<Self, String> {
        verify_arabic(c, &arabic)?;
        let (mut meanings, meaning) = match translation {
            Some((t, m)) => {
                verify(c, &t).and_then(|_| verify_pair(&arabic, &t))?;
                (Some(t.hadiths.into_iter().map(|h| h.text)), Some(m))
            }
            None => (None, None),
        };
        let english = meaning.as_ref().is_some_and(|m| m.lang == "eng");
        let mut books: Vec<Book> = Vec::new();
        let mut book_index: HashMap<u32, u32> = HashMap::new();
        let mut hadith = Vec::with_capacity(info(c).hadith);
        let mut book = 0u32;
        let references = references(c, &arabic.hadiths);
        for (raw, reference) in arabic.hadiths.into_iter().zip(references) {
            let text = meanings.as_mut().and_then(Iterator::next).filter(|t| !t.trim().is_empty());
            let (ref_book, ref_hadith) = (raw.reference.book as u32, raw.reference.hadith as u32);
            // An entry without an in-book place belongs to the book before it.
            if ref_hadith > 0 {
                book = ref_book;
            }
            if raw.text.trim().is_empty() {
                continue;
            }
            let at = *book_index.entry(book).or_insert_with(|| {
                let name = arabic.metadata.sections.get(&book.to_string()).cloned().unwrap_or_default();
                books.push(Book { number: book, name: name.trim().to_string(), hadith: Vec::new() });
                (books.len() - 1) as u32
            });
            let index = hadith.len() as u32;
            books[at as usize].hadith.push(index);
            let (s, e) = chain::split(&raw.text);
            let (narrator, narrator_len) = text.as_deref().filter(|_| english).and_then(narrator).map_or((None, 0), |(n, len)| (Some(n), len as u32));
            hadith.push(Hadith {
                number: raw.hadithnumber,
                reference,
                book: at,
                in_book: (ref_hadith > 0).then_some(ref_hadith),
                words: (s as u32, e as u32),
                arabic: raw.text,
                grades: raw.grades.iter().filter(|g| !g.grade.trim().is_empty()).map(|g| Grade::new(&g.name, &g.grade)).collect(),
                meaning: text,
                narrator,
                narrator_len,
            });
        }
        let mut by_reference = HashMap::with_capacity(hadith.len());
        for (i, h) in hadith.iter().enumerate() {
            by_reference.entry(h.reference.clone()).or_insert(i as u32);
        }
        Ok(Library { collection: c, books, hadith, meaning, by_reference })
    }

    pub fn get(&self, index: u32) -> Option<&Hadith> {
        self.hadith.get(index as usize)
    }

    /// The hadith with `reference` (`774b`), or a number's first narration
    /// (`2699` → `2699a`).
    pub fn find(&self, reference: &str) -> Option<u32> {
        let r = reference.trim().to_lowercase();
        self.by_reference.get(&r).copied().or_else(|| {
            if !r.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            self.hadith.iter().position(|h| h.reference.strip_prefix(r.as_str()).is_some_and(|rest| rest.len() == 1 && rest.chars().all(|c| c.is_ascii_lowercase()))).map(|i| i as u32)
        })
    }

    pub fn book_of(&self, index: u32) -> Option<&Book> {
        self.books.get(self.get(index)?.book as usize)
    }

    /// Where a hadith sits in its book: `(position, book length)`.
    pub fn place(&self, index: u32) -> Option<(usize, usize)> {
        let b = self.book_of(index)?;
        Some((b.hadith.iter().position(|i| *i == index)?, b.hadith.len()))
    }

    /// The hadith `step` places after (or before) `index` in its book.
    pub fn step(&self, index: u32, step: isize) -> Option<u32> {
        let b = self.book_of(index)?;
        let at = b.hadith.iter().position(|i| *i == index)?;
        b.hadith.get(at.checked_add_signed(step)?).copied()
    }

    /// `bukhari:774b`.
    pub fn key(&self, index: u32) -> Option<String> {
        Some(refs::key(self.collection, &self.get(index)?.reference))
    }

    /// The hadith on sunnah.com.
    pub fn url(&self, index: u32) -> Option<String> {
        let h = self.get(index)?;
        Some(refs::url(self.collection, &h.reference, self.books.get(h.book as usize).map_or(0, |b| b.number)))
    }
}

// ── The narrator ─────────────────────────────────────────────────────────

/// Longest narrator name read (characters).
const MAX_NAME: usize = 80;
/// A sentence-start name is at most this many words.
const MAX_NAME_WORDS: usize = 6;

/// Lower-case words a name may hold (`Abdullah bin Umar`).
const NAME_PARTICLES: [&str; 8] = ["bin", "ibn", "b.", "bint", "al", "ibn-", "and", "of"];

/// Whether `s` reads like a person's name: every word capitalised (or
/// starting with a mark, `` `Umar ``), or a particle.
fn looks_like_name(s: &str) -> bool {
    s.split_whitespace().all(|w| NAME_PARTICLES.contains(&w) || w.chars().next().is_some_and(|c| c.is_uppercase() || !c.is_alphabetic()))
}

/// The narrator an English meaning opens with, and how many bytes of it
/// may be hidden when the narrator is shown apart (only a plain
/// "Narrated X:" opening; other openings stay).
pub fn narrator(meaning: &str) -> Option<(String, usize)> {
    let m = meaning.trim_start();
    let lead = meaning.len() - m.len();
    let clean = |s: &str| -> Option<String> {
        let s = s.trim().trim_end_matches([',', ':', ';', '.']).trim();
        (!s.is_empty() && s.chars().count() <= MAX_NAME && looks_like_name(s)).then(|| s.to_string())
    };
    if let Some(rest) = m.strip_prefix("Narrated ") {
        let colon = rest.find(':')?;
        let name = clean(&rest[..colon])?;
        return Some((name, lead + "Narrated ".len() + colon + 1));
    }
    const OPENINGS: [&str; 4] = ["It is narrated on the authority of ", "On the authority of ", "It was narrated from ", "It was narrated that "];
    const ENDS: [&str; 8] = [" (", ",", " who ", " that ", ":", " said", " reported", " narrated"];
    for opening in OPENINGS {
        if let Some(rest) = m.strip_prefix(opening) {
            let end = ENDS.iter().filter_map(|e| rest.find(e)).min().unwrap_or(rest.len());
            return clean(&rest[..end]).map(|n| (n, 0));
        }
    }
    for verb in [" reported", " narrated"] {
        if let Some(i) = m.find(verb) {
            let name = &m[..i];
            if name.split_whitespace().count() <= MAX_NAME_WORDS && !name.contains([':', '.', '"']) {
                return clean(name).map(|n| (n, 0));
            }
        }
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A made-up edition in the dataset's shape (no hadith text typed).
    pub(crate) fn sample(c: Collection, n: usize, lang: &str) -> String {
        let hadiths: Vec<serde_json::Value> = (0..n)
            .map(|i| {
                let book = 1 + i / 4;
                let text = if lang == "ara" { format!("حَدَّثَنَا فُلاَنٌ، قَالَ \" كَلاَمٌ طَيِّبٌ رَقْمُ {i} \"") } else { format!("Narrated Someone {i}: words {i}") };
                serde_json::json!({ "hadithnumber": i + 1, "arabicnumber": i + 1, "text": text, "grades": [{ "name": "Al-Albani", "grade": "Hasan" }], "reference": { "book": book, "hadith": i % 4 + 1 } })
            })
            .collect();
        serde_json::json!({ "metadata": { "name": c.id(), "sections": { "0": "", "1": "First", "2": "Second" } }, "hadiths": hadiths }).to_string()
    }

    #[test]
    fn an_edition_builds_a_library() {
        let c = Collection::Nawawi;
        let ara = parse(&sample(c, 42, "ara")).unwrap();
        let eng = parse(&sample(c, 42, "eng")).unwrap();
        let lib = Library::build(c, ara, Some((eng, MeaningInfo { lang: "eng".into(), translator: None, fallback: false }))).unwrap();
        assert_eq!(lib.hadith.len(), 42);
        assert_eq!(lib.books.len(), 11);
        assert_eq!((lib.books[0].number, lib.books[0].name.as_str(), lib.books[0].hadith.len()), (1, "First", 4));
        let h = &lib.hadith[5];
        assert_eq!(h.reference, "6");
        assert_eq!(h.words(), "كَلاَمٌ طَيِّبٌ رَقْمُ 5");
        assert_eq!(h.narrator.as_deref(), Some("Someone 5"));
        assert_eq!(h.meaning_body(), Some("words 5"));
        assert_eq!(h.class(c), Some(GradeClass::Hasan));
        assert_eq!(lib.find("6"), Some(5));
        assert_eq!(lib.place(5), Some((1, 4)));
        assert_eq!(lib.step(5, 1), Some(6));
        assert_eq!(lib.step(4, -1), None, "the first of its book");
        assert_eq!(lib.key(5).as_deref(), Some("nawawi:6"));
        assert_eq!(lib.url(5).as_deref(), Some("https://sunnah.com/nawawi40:6"));
    }

    #[test]
    fn broken_files_are_refused() {
        let c = Collection::Nawawi;
        assert!(parse("{}").is_err());
        // Wrong count.
        assert!(verify(c, &parse(&sample(c, 41, "ara")).unwrap()).is_err());
        // A translation of another length doesn't line up.
        let ara = parse(&sample(c, 42, "ara")).unwrap();
        let mut eng = parse(&sample(c, 42, "eng")).unwrap();
        eng.hadiths[3].hadithnumber = 99.0;
        assert!(verify_pair(&ara, &eng).is_err());
        // Missing Arabic.
        let mut bare = parse(&sample(c, 42, "ara")).unwrap();
        bare.hadiths.iter_mut().for_each(|h| h.text.clear());
        assert!(Library::build(c, bare, None).is_err());
    }

    #[test]
    fn muslim_references_follow_abdul_baqi() {
        let raw = |n: f64, an: serde_json::Value, book: u32, inb: u32| RawHadith { hadithnumber: n, arabicnumber: Some(an), text: "نَصٌّ".into(), grades: Vec::new(), reference: RawRef { book: f64::from(book), hadith: f64::from(inb) } };
        let mut hs = vec![
            raw(1.0, serde_json::Value::Null, 0, 1),
            raw(2.0, "546.01".into(), 5, 59),
            raw(3.0, serde_json::Value::Null, 0, 0),
            raw(4.0, "547".into(), 5, 61),
        ];
        // Pad to the expected count with empty entries.
        for i in hs.len()..info(Collection::Muslim).entries {
            hs.push(RawHadith { hadithnumber: 10.0 + i as f64, arabicnumber: None, text: String::new(), grades: Vec::new(), reference: RawRef::default() });
        }
        let refs = references(Collection::Muslim, &hs);
        assert_eq!(&refs[..4], ["i1", "546a", "546b", "547"]);
        // An inferred letter the dataset gives another narration is not used.
        let clash = [raw(1.0, "9.01".into(), 1, 1), raw(2.0, serde_json::Value::Null, 0, 0), raw(3.0, "9.02".into(), 1, 3)];
        assert_eq!(references(Collection::Muslim, &clash), ["9a", "n2", "9b"]);
        // Everywhere else the dataset's number is the reference.
        assert_eq!(references(Collection::Bukhari, &[raw(774.2, serde_json::Value::Null, 10, 169)]), ["774b"]);
        let tenth = [raw(3604.0, serde_json::Value::Null, 48, 1), raw(3604.09, serde_json::Value::Null, 48, 2), raw(3604.1, serde_json::Value::Null, 48, 3)];
        assert_eq!(references(Collection::Tirmidhi, &tenth), ["3604", "3604i", "3604j"]);
        // Too little Arabic for a real Muslim: refused.
        let ed = RawEdition { metadata: RawMeta::default(), hadiths: hs };
        assert!(verify_arabic(Collection::Muslim, &ed).is_err());
    }

    #[test]
    fn narrators_are_read_from_english() {
        assert_eq!(narrator("Narrated Abu Huraira: The Prophet said"), Some(("Abu Huraira".into(), "Narrated Abu Huraira:".len())));
        assert_eq!(narrator("Abu Huraira reported Allah's Messenger as saying"), Some(("Abu Huraira".into(), 0)));
        assert_eq!(narrator("On the authority of Abu Hurayrah (may Allah be pleased with him), who said"), Some(("Abu Hurayrah".into(), 0)));
        assert_eq!(narrator("It was narrated from Abu Hurairah that the Prophet said"), Some(("Abu Hurairah".into(), 0)));
        assert_eq!(narrator("Ibn `Umar narrated that the Prophet said"), Some(("Ibn `Umar".into(), 0)));
        assert_eq!(narrator("The report is not like witnessing."), None);
        assert_eq!(narrator("This hadith has been narrated on the authority of Abu Huraira through another chain"), None);
    }
}
