//! The Jannah content: every Qur'an verse and every hadith of Sahih
//! al-Bukhari and Sahih Muslim that mentions Paradise, copied verbatim from
//! their sources (provenance and selection rules: `data/jannah/README.md`).
//!
//! Both files are compiled in and parsed once, on first use.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

const QURAN_JSON: &str = include_str!("../../data/jannah/quran.json");
const HADITH_JSON: &str = include_str!("../../data/jannah/hadith.json");

/// The fixed set of themes every verse and hadith is tagged with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// What Jannah is like: gardens, palaces, shade, rivers, food, garments.
    Description,
    /// Actions that lead there.
    Deeds,
    /// What keeps a person from it.
    Barriers,
    /// Who enters, who enters first, the traits of its people.
    People,
    /// Allah's mercy, hope, forgiveness.
    Mercy,
    /// Degrees, gates, al-Firdaws.
    Levels,
    /// Seeing Allah.
    Vision,
    /// Forever: no death, no sorrow.
    Eternity,
    /// Parents, children, spouses.
    Family,
    /// The garden of Adam.
    Adam,
}

impl Theme {
    pub const ALL: [Theme; 10] = [
        Theme::Description,
        Theme::Deeds,
        Theme::Barriers,
        Theme::People,
        Theme::Mercy,
        Theme::Levels,
        Theme::Vision,
        Theme::Eternity,
        Theme::Family,
        Theme::Adam,
    ];

    /// The id used in the JSON files.
    pub fn id(self) -> &'static str {
        match self {
            Theme::Description => "description",
            Theme::Deeds => "deeds",
            Theme::Barriers => "barriers",
            Theme::People => "people",
            Theme::Mercy => "mercy",
            Theme::Levels => "levels",
            Theme::Vision => "vision",
            Theme::Eternity => "eternity",
            Theme::Family => "family",
            Theme::Adam => "adam",
        }
    }
}

// ------------------------------------------------------------------ Qur'an

/// `quran.json`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quran {
    pub source: QuranSource,
    /// All 114 surahs, in order.
    pub surahs: Vec<Surah>,
    /// Sorted by surah, then ayah.
    pub verses: Vec<Verse>,
}

/// Attribution for the Qur'an text (Tanzil requires it to be shown).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuranSource {
    pub text: String,
    pub publisher: String,
    pub url: String,
    pub license: String,
    pub retrieved_from: String,
    pub surah_metadata: String,
    pub retrieved: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Surah {
    pub number: u16,
    /// Arabic name without diacritics, e.g. `البقرة`.
    pub arabic: String,
    /// English transliteration as the source spells it, e.g. `Al-Baqara`.
    pub name: String,
    /// English meaning, e.g. `The Cow`.
    pub meaning: String,
    /// Number of ayahs in the surah.
    pub ayahs: u16,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verse {
    pub surah: u16,
    pub ayah: u16,
    /// Tanzil Uthmani text, verbatim.
    pub text: String,
    pub themes: Vec<Theme>,
}

impl Verse {
    /// `quran-2:25`, the id used by path evidence.
    pub fn id(&self) -> String {
        format!("quran-{}:{}", self.surah, self.ayah)
    }

    /// The verse on quran.com.
    pub fn url(&self) -> String {
        format!("https://quran.com/{}/{}", self.surah, self.ayah)
    }
}

// ------------------------------------------------------------------ hadith

/// `hadith.json`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HadithFile {
    pub source: HadithSource,
    /// Sorted by collection, then number, then the sunnah.com letter.
    pub hadith: Vec<Hadith>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HadithSource {
    pub dataset: String,
    pub url: String,
    pub license: String,
    pub retrieved_from: String,
    pub retrieved: String,
    /// Per language: a translator name, or per collection names.
    pub translators: HashMap<String, serde_json::Value>,
    pub numbering: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Collection {
    Bukhari,
    Muslim,
}

impl Collection {
    /// The collection's name in English, for citations.
    pub fn title(self) -> &'static str {
        match self {
            Collection::Bukhari => "Sahih al-Bukhari",
            Collection::Muslim => "Sahih Muslim",
        }
    }

    /// The collection's name in Arabic.
    pub fn arabic(self) -> &'static str {
        match self {
            Collection::Bukhari => "صحيح البخاري",
            Collection::Muslim => "صحيح مسلم",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Collection::Bukhari => "bukhari",
            Collection::Muslim => "muslim",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hadith {
    /// `bukhari-3245`, `muslim-2699a`: the collection and the sunnah.com
    /// reference (the standard number plus sunnah.com's narration letter).
    pub id: String,
    pub collection: Collection,
    /// The standard number: Bukhari as numbered in Fath al-Bari, Muslim as
    /// numbered by Muhammad Fuad Abdul Baqi.
    pub number: u32,
    /// The book (kitab) number within the collection, as sunnah.com numbers it.
    pub book: u32,
    /// The hadith on sunnah.com (checked to open the same hadith).
    pub url: String,
    /// Arabic text with its isnad, verbatim from the dataset.
    pub arabic: String,
    /// The dataset's translations, verbatim; a language is absent when the
    /// dataset has none or its text does not match the Arabic.
    pub text: HadithText,
    pub themes: Vec<Theme>,
}

/// Translations by interface language code.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HadithText {
    pub en: Option<String>,
    pub id: Option<String>,
    pub tr: Option<String>,
    pub ru: Option<String>,
    pub fr: Option<String>,
}

impl HadithText {
    /// The translation for a language code (`en`, `id`, `tr`, `ru`, `fr`).
    pub fn get(&self, lang: &str) -> Option<&str> {
        match lang {
            "en" => self.en.as_deref(),
            "id" => self.id.as_deref(),
            "tr" => self.tr.as_deref(),
            "ru" => self.ru.as_deref(),
            "fr" => self.fr.as_deref(),
            _ => None,
        }
    }
}

impl Hadith {
    /// The sunnah.com reference: `3245`, `774b`, `2699a`.
    pub fn reference(&self) -> &str {
        self.id.split_once('-').map_or(self.id.as_str(), |(_, r)| r)
    }

    /// `Sahih al-Bukhari 3245`, `Sahih Muslim 2699a`.
    pub fn citation(&self) -> String {
        format!("{} {}", self.collection.title(), self.reference())
    }
}

// ------------------------------------------------------------------ lookups

/// A verse or a hadith.
#[derive(Debug, Clone, Copy)]
pub enum Item {
    Verse(&'static Verse),
    Hadith(&'static Hadith),
}

impl Item {
    /// `quran-2:25` or the hadith id.
    pub fn id(&self) -> String {
        match self {
            Item::Verse(v) => v.id(),
            Item::Hadith(h) => h.id.clone(),
        }
    }

    pub fn arabic(&self) -> &'static str {
        match self {
            Item::Verse(v) => &v.text,
            Item::Hadith(h) => &h.arabic,
        }
    }

    pub fn themes(&self) -> &'static [Theme] {
        match self {
            Item::Verse(v) => &v.themes,
            Item::Hadith(h) => &h.themes,
        }
    }
}

/// The parsed `quran.json` (parsed once, on first use).
pub fn quran() -> &'static Quran {
    static QURAN: OnceLock<Quran> = OnceLock::new();
    QURAN.get_or_init(|| serde_json::from_str(QURAN_JSON).expect("bundled quran.json parses"))
}

/// The parsed `hadith.json` (parsed once, on first use).
pub fn hadith_file() -> &'static HadithFile {
    static HADITH: OnceLock<HadithFile> = OnceLock::new();
    HADITH.get_or_init(|| serde_json::from_str(HADITH_JSON).expect("bundled hadith.json parses"))
}

/// Every verse, sorted by surah then ayah.
pub fn verses() -> &'static [Verse] {
    &quran().verses
}

/// Every hadith, sorted by collection then number.
pub fn all_hadith() -> &'static [Hadith] {
    &hadith_file().hadith
}

/// Surah `n` (1..=114).
pub fn surah(n: u16) -> Option<&'static Surah> {
    quran().surahs.get(usize::from(n).checked_sub(1)?).filter(|s| s.number == n)
}

/// The verse `surah:ayah`, when it is part of the set.
pub fn verse(surah: u16, ayah: u16) -> Option<&'static Verse> {
    let v = verses();
    v.binary_search_by(|x| (x.surah, x.ayah).cmp(&(surah, ayah))).ok().map(|i| &v[i])
}

/// The hadith with this id (`bukhari-3245`, `muslim-2699a`).
pub fn hadith(id: &str) -> Option<&'static Hadith> {
    static INDEX: OnceLock<HashMap<&'static str, usize>> = OnceLock::new();
    let index = INDEX.get_or_init(|| all_hadith().iter().enumerate().map(|(i, h)| (h.id.as_str(), i)).collect());
    index.get(id).map(|&i| &all_hadith()[i])
}

/// A verse (`quran-2:25`) or a hadith by id.
pub fn lookup(id: &str) -> Option<Item> {
    if let Some(r) = id.strip_prefix("quran-") {
        let (s, a) = r.split_once(':')?;
        return verse(s.parse().ok()?, a.parse().ok()?).map(Item::Verse);
    }
    hadith(id).map(Item::Hadith)
}

/// Every verse, then every hadith, tagged with `theme`.
pub fn by_theme(theme: Theme) -> impl Iterator<Item = Item> {
    let verses = verses().iter().filter(move |v| v.themes.contains(&theme)).map(Item::Verse);
    let hadith = all_hadith().iter().filter(move |h| h.themes.contains(&theme)).map(Item::Hadith);
    verses.chain(hadith)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jannah::paths::{PathGroup, PATHS};
    use std::collections::HashSet;

    const FATHA: char = '\u{064E}';
    const SHADDA: char = '\u{0651}';

    fn is_mark(c: char) -> bool {
        matches!(c, '\u{064B}'..='\u{065F}' | '\u{0670}' | '\u{06D6}'..='\u{06ED}')
    }

    fn strip(text: &str) -> String {
        text.chars().filter(|&c| !is_mark(c)).map(|c| if c == 'ٱ' { 'ا' } else { c }).collect()
    }

    /// A janna-family word whose jīm carries a fatha: ج + fatha, ن + shadda,
    /// an optional alif, then ة or ت (جَنَّة، الجَنَّة، جَنَّٰت، جَنَّتَان…).
    fn has_fatha_janna(text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        (0..chars.len()).any(|i| {
            if chars[i] != 'ج' {
                return false;
            }
            let mut j = i + 1;
            let mut fatha = false;
            while j < chars.len() && is_mark(chars[j]) {
                fatha |= chars[j] == FATHA;
                j += 1;
            }
            if !fatha || chars.get(j) != Some(&'ن') {
                return false;
            }
            j += 1;
            let mut shadda = false;
            while j < chars.len() && is_mark(chars[j]) {
                shadda |= chars[j] == SHADDA;
                j += 1;
            }
            if !shadda {
                return false;
            }
            if chars.get(j) == Some(&'ا') {
                j += 1;
                while j < chars.len() && is_mark(chars[j]) {
                    j += 1;
                }
            }
            matches!(chars.get(j), Some('ة' | 'ت'))
        })
    }

    #[test]
    fn jannah_files_parse_with_sane_counts() {
        let q = quran();
        assert_eq!(q.surahs.len(), 114);
        assert!((100..=160).contains(&q.verses.len()), "{} verses", q.verses.len());
        let h = all_hadith();
        assert!((500..=700).contains(&h.len()), "{} hadith", h.len());
        for c in [Collection::Bukhari, Collection::Muslim] {
            let n = h.iter().filter(|x| x.collection == c).count();
            assert!((200..=400).contains(&n), "{n} hadith in {c:?}");
        }
        assert!(!q.source.url.is_empty() && !hadith_file().source.license.is_empty());
    }

    #[test]
    fn jannah_surahs_are_complete() {
        let q = quran();
        for (i, s) in q.surahs.iter().enumerate() {
            assert_eq!(usize::from(s.number), i + 1);
            assert!(!s.arabic.is_empty() && !s.name.is_empty() && !s.meaning.is_empty());
            assert!(s.ayahs > 0);
        }
        assert_eq!(q.surahs.iter().map(|s| u32::from(s.ayahs)).sum::<u32>(), 6236);
        assert_eq!(surah(2).map(|s| s.arabic.as_str()), Some("البقرة"));
        assert!(surah(0).is_none() && surah(115).is_none());
    }

    #[test]
    fn jannah_verses_are_unique_sorted_and_in_range() {
        let v = verses();
        for w in v.windows(2) {
            assert!((w[0].surah, w[0].ayah) < (w[1].surah, w[1].ayah), "{} then {}", w[0].id(), w[1].id());
        }
        for x in v {
            let s = surah(x.surah).expect("surah exists");
            assert!(x.ayah >= 1 && x.ayah <= s.ayahs, "{} out of range", x.id());
            assert!(!x.text.trim().is_empty(), "{} has no text", x.id());
            assert!(!x.themes.is_empty(), "{} has no theme", x.id());
            let mut t = x.themes.clone();
            t.dedup();
            assert_eq!(t.len(), x.themes.len(), "{} repeats a theme", x.id());
        }
        assert!(verse(2, 25).is_some());
    }

    #[test]
    fn jannah_every_verse_names_paradise() {
        const NAMES: [&str; 4] = ["فردوس", "دار السلم", "دار المقامة", "دار المتقين"];
        for x in verses() {
            let plain = strip(&x.text);
            assert!(
                has_fatha_janna(&x.text) || NAMES.iter().any(|n| plain.contains(n)),
                "{} mentions neither janna nor a name of Paradise",
                x.id()
            );
        }
    }

    #[test]
    fn jannah_excluded_verses_are_absent() {
        // جِنَّة (jinn, madness), جُنَّة (shield), أَجِنَّة (embryos) and
        // earthly gardens: see data/jannah/README.md.
        const EXCLUDED: &[(u16, u16)] = &[
            (114, 6), (7, 184), (11, 119), (32, 13), (37, 158), (23, 25), (23, 70), (34, 8), (34, 46),
            (53, 32), (58, 16), (63, 2),
            (2, 265), (2, 266), (6, 99), (6, 141), (13, 4), (17, 91), (18, 32), (18, 33), (18, 35),
            (18, 39), (18, 40), (23, 19), (25, 8), (25, 10), (26, 57), (26, 134), (26, 147),
            (34, 15), (34, 16), (36, 34), (44, 25), (50, 9), (68, 17), (71, 12), (78, 16),
        ];
        for &(s, a) in EXCLUDED {
            assert!(verse(s, a).is_none(), "{s}:{a} must not be in the set");
        }
    }

    #[test]
    fn jannah_hadith_are_unique_and_complete() {
        let mut ids = HashSet::new();
        for h in all_hadith() {
            assert!(ids.insert(h.id.as_str()), "duplicate id {}", h.id);
            assert!(h.id.starts_with(&format!("{}-{}", h.collection.id(), h.number)), "{} vs number {}", h.id, h.number);
            let tail = &h.reference()[h.number.to_string().len()..];
            assert!(tail.len() <= 1 && tail.chars().all(|c| c.is_ascii_lowercase()), "{}", h.id);
            assert!(h.url.starts_with(&format!("https://sunnah.com/{}", h.collection.id())), "{}", h.url);
            assert!(h.book > 0, "{} has no book", h.id);
            assert!(!h.arabic.trim().is_empty(), "{} has no Arabic", h.id);
            assert!(!h.themes.is_empty(), "{} has no theme", h.id);
            let en = h.text.en.as_deref().unwrap_or_default();
            assert!(!en.trim().is_empty(), "{} has no English", h.id);
            for lang in ["en", "id", "tr", "ru", "fr"] {
                if let Some(t) = h.text.get(lang) {
                    assert!(!t.trim().is_empty(), "{} has an empty {lang} text", h.id);
                }
            }
            assert_eq!(hadith(&h.id).map(|x| x.id.as_str()), Some(h.id.as_str()));
        }
        assert_eq!(hadith("bukhari-3245").map(Hadith::citation).as_deref(), Some("Sahih al-Bukhari 3245"));
        assert_eq!(hadith("muslim-2699a").map(Hadith::citation).as_deref(), Some("Sahih Muslim 2699a"));
    }

    #[test]
    fn jannah_every_hadith_mentions_jannah() {
        for h in all_hadith() {
            assert!(
                has_fatha_janna(&h.arabic) || strip(&h.arabic).contains("الفردوس"),
                "{} has no janna (fatha) form nor الفردوس",
                h.id
            );
        }
    }

    #[test]
    fn jannah_janna_matcher_tells_the_vowels_apart() {
        assert!(has_fatha_janna("جَنَّٰتٍۢ تَجْرِى"));
        assert!(has_fatha_janna("ٱلْجَنَّةَ"));
        assert!(has_fatha_janna("رَوْضَاتِ ٱلْجَنَّاتِ"));
        assert!(has_fatha_janna("جَنَّتَانِ"));
        assert!(!has_fatha_janna("مِنَ ٱلْجِنَّةِ وَٱلنَّاسِ"));
        assert!(!has_fatha_janna("الصِّيَامُ جُنَّةٌ"));
        assert!(!has_fatha_janna("وَجْنَتَاهُ"));
    }

    #[test]
    fn jannah_every_theme_is_used() {
        for t in Theme::ALL {
            assert!(by_theme(t).next().is_some(), "no item tagged {}", t.id());
        }
        assert!(by_theme(Theme::Adam).all(|i| i.themes().contains(&Theme::Adam)));
    }

    #[test]
    fn jannah_paths_are_backed_by_the_data() {
        let mut keys = HashSet::new();
        for p in PATHS {
            assert!(keys.insert(p.key), "duplicate path {}", p.key);
            assert!(!p.title.trim().is_empty() && !p.line.trim().is_empty(), "{} lacks text", p.key);
            assert!(!p.evidence.is_empty(), "{} has no evidence", p.key);
            for id in p.evidence {
                assert!(lookup(id).is_some(), "{}: evidence {id} is not in the data", p.key);
            }
        }
        assert!((30..=40).contains(&PATHS.len()), "{} paths", PATHS.len());
        for g in PathGroup::ALL {
            assert!(PATHS.iter().any(|p| p.group == g), "no path in {g:?}");
        }
        assert_eq!(lookup("quran-2:25").map(|i| i.id()).as_deref(), Some("quran-2:25"));
        assert!(lookup("quran-114:6").is_none() && lookup("bukhari-0").is_none());
    }
}
