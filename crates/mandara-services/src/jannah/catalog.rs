//! One list of every reminder (the verses, then the hadith) over the
//! bundled content, with stable keys (`quran-2:25`, `bukhari-3245`, the
//! same ids the paths cite), the text a card shows (a hadith's words
//! without its chain of narrators), references, citations and links.
//! Built once, on first use.

use std::sync::OnceLock;

use mandara_i18n::{t, tk};

use super::data::{self, Collection, Hadith, Theme, Verse};
use super::paths::{PATHS, Path, PathGroup};
use super::search::fold;

/// A reminder, by its place in the [`Catalog`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u32);

/// What an item is.
#[derive(Copy, Clone, Debug)]
pub enum Source {
    Verse(&'static Verse),
    Hadith(&'static Hadith),
}

/// One reminder.
#[derive(Debug)]
pub struct Item {
    pub id: ItemId,
    /// `quran-2:25` or `bukhari-3245`.
    pub key: String,
    pub source: Source,
    /// The Arabic a card shows: the verse, or the hadith's words without
    /// its chain of narrators (verbatim slices of the text).
    pub arabic: &'static str,
    /// [`Item::arabic`]'s length in characters (cards skip long ones).
    pub len: usize,
}

impl Item {
    pub fn is_verse(&self) -> bool {
        matches!(self.source, Source::Verse(_))
    }

    pub fn themes(&self) -> &'static [Theme] {
        match self.source {
            Source::Verse(v) => &v.themes,
            Source::Hadith(h) => &h.themes,
        }
    }

    /// The whole Arabic text (a hadith with its chain of narrators).
    pub fn full_arabic(&self) -> &'static str {
        match self.source {
            Source::Verse(v) => &v.text,
            Source::Hadith(h) => &h.arabic,
        }
    }

    /// `(surah, ayah)` of a verse.
    pub fn verse_ref(&self) -> Option<(u16, u16)> {
        match self.source {
            Source::Verse(v) => Some((v.surah, v.ayah)),
            Source::Hadith(_) => None,
        }
    }

    pub fn hadith(&self) -> Option<&'static Hadith> {
        match self.source {
            Source::Hadith(h) => Some(h),
            Source::Verse(_) => None,
        }
    }
}

/// Every reminder: the verses (by surah and ayah), then the hadith.
#[derive(Debug)]
pub struct Catalog {
    pub items: Vec<Item>,
    /// How many of `items` are verses (they come first).
    pub verses: usize,
}

impl Catalog {
    pub fn get(&self, id: ItemId) -> Option<&Item> {
        self.items.get(id.0 as usize)
    }

    /// The verses' ids.
    pub fn verse_ids(&self) -> impl Iterator<Item = ItemId> + '_ {
        self.items[..self.verses].iter().map(|i| i.id)
    }

    /// The hadith's ids.
    pub fn hadith_ids(&self) -> impl Iterator<Item = ItemId> + '_ {
        self.items[self.verses..].iter().map(|i| i.id)
    }

    /// The item a reference names: `2:25`, `quran-2:25`, `bukhari-3245`,
    /// `muslim-2699a` (also `bukhari:3245`, `Muslim 2699a`; a number
    /// without its narration letter finds the first narration).
    pub fn find(&self, reference: &str) -> Option<ItemId> {
        let r = reference.trim().to_lowercase();
        let r = r.strip_prefix("quran-").or_else(|| r.strip_prefix("quran:")).or_else(|| r.strip_prefix("quran ")).unwrap_or(&r);
        if let Some((s, a)) = r.split_once(':')
            && let (Ok(s), Ok(a)) = (s.trim().parse::<u32>(), a.trim().parse::<u32>())
        {
            return self.items[..self.verses].iter().find(|i| i.verse_ref().is_some_and(|(vs, va)| u32::from(vs) == s && u32::from(va) == a)).map(|i| i.id);
        }
        let key: String = r.split([':', ' ', '-']).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
        let hadith = &self.items[self.verses..];
        hadith
            .iter()
            .find(|i| i.key == key)
            .or_else(|| hadith.iter().find(|i| i.key.strip_prefix(key.as_str()).is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphabetic()))))
            .map(|i| i.id)
    }

    /// The item an evidence id cites.
    pub fn evidence(&self, id: &str) -> Option<ItemId> {
        self.find(id)
    }
}

/// The catalog, built on first use.
pub fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(build)
}

fn build() -> Catalog {
    let mut items = Vec::new();
    for v in &data::quran().verses {
        let id = ItemId(items.len() as u32);
        items.push(Item { id, key: format!("quran-{}:{}", v.surah, v.ayah), source: Source::Verse(v), arabic: v.text.trim(), len: v.text.chars().count() });
    }
    let verses = items.len();
    for h in data::all_hadith() {
        let id = ItemId(items.len() as u32);
        let arabic = matn(&h.arabic);
        items.push(Item { id, key: h.id.clone(), source: Source::Hadith(h), arabic, len: arabic.chars().count() });
    }
    Catalog { items, verses }
}

// ── The words of a hadith ────────────────────────────────────────────────

/// The quotation marks the dataset puts around the Prophet's words.
const QUOTE: char = '"';
/// Words that open a chain of narrators (compared without diacritics): a
/// second chain after the words (`… "‏ ‏.‏ وحدثني …`) is not part of them.
const CHAIN_OPENERS: [&str; 6] = ["حدثنا", "حدثني", "وحدثنا", "وحدثني", "أخبرنا", "وأخبرنا"];
/// Shorter quoted runs are a word or two inside the chain, not the saying.
const MIN_MATN_CHARS: usize = 12;
/// Trimmed off both ends of the words: spaces, direction marks, the
/// dataset's punctuation.
const MATN_TRIM: [char; 6] = [' ', '\u{200F}', '\u{200E}', '.', '،', '\u{2009}'];

/// A hadith's words without its chain of narrators: from the first
/// quotation mark to the last one before any second chain. The whole text
/// when it has no such quotation.
pub fn matn(arabic: &str) -> &str {
    let Some(first) = arabic.find(QUOTE) else { return arabic.trim() };
    let start = first + QUOTE.len_utf8();
    let rest = &arabic[start..];
    let openers: Vec<String> = CHAIN_OPENERS.iter().map(|w| fold(w)).collect();
    let chain = rest.split_inclusive(char::is_whitespace).scan(start, |at, word| {
        let here = *at;
        *at += word.len();
        Some((here, word))
    });
    let chain = chain.filter(|(_, w)| openers.contains(&fold(w))).map(|(i, _)| i).next();
    let end = arabic.match_indices(QUOTE).map(|(i, _)| i).rfind(|i| *i >= start && chain.is_none_or(|c| *i < c));
    match end {
        Some(end) => {
            let words = arabic[start..end].trim_matches(&MATN_TRIM[..]);
            if words.chars().count() >= MIN_MATN_CHARS { words } else { arabic.trim() }
        }
        None => arabic.trim(),
    }
}

// ── References ───────────────────────────────────────────────────────────

/// Code point of ARABIC-INDIC DIGIT ZERO (٠).
const ARABIC_INDIC_ZERO: u32 = 0x0660;
/// The ornate parentheses around a verse number (﴿٢٥﴾).
const AYAH_OPEN: char = '\u{FD3F}';
const AYAH_CLOSE: char = '\u{FD3E}';

/// ASCII digits as Arabic-Indic digits.
pub fn arabic_digits(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_digit() { char::from_u32(ARABIC_INDIC_ZERO + (c as u32 - '0' as u32)).unwrap_or(c) } else { c }).collect()
}

/// The ornate end-of-verse mark with its number (`﴿٢٥﴾`).
pub fn ayah_mark(ayah: u16) -> String {
    format!("{AYAH_OPEN}{}{AYAH_CLOSE}", arabic_digits(&ayah.to_string()))
}

/// A collection's name (a catalog key), and in Arabic.
pub fn collection_name(collection: Collection) -> (&'static str, &'static str) {
    match collection {
        Collection::Bukhari => (tk("Sahih al-Bukhari"), collection.arabic()),
        Collection::Muslim => (tk("Sahih Muslim"), collection.arabic()),
    }
}

/// A surah's `(transliterated, Arabic)` names.
pub fn surah_names(n: u16) -> (String, String) {
    data::quran().surahs.iter().find(|s| s.number == n).map(|s| (s.name.clone(), s.arabic.clone())).unwrap_or_else(|| (n.to_string(), arabic_digits(&n.to_string())))
}

/// How an item is cited: `Al-Baqarah 2:25` / `Sahih al-Bukhari 3245`, or
/// in Arabic (`البقرة ٢:٢٥`, `صحيح البخاري ٣٢٤٥`).
pub fn reference(item: &Item, arabic: bool) -> String {
    match item.source {
        Source::Verse(v) => {
            let (name, ar) = surah_names(v.surah);
            if arabic { format!("{ar} {}", arabic_digits(&format!("{}:{}", v.surah, v.ayah))) } else { format!("{name} {}:{}", v.surah, v.ayah) }
        }
        Source::Hadith(h) => {
            let (name, ar) = collection_name(h.collection);
            if arabic { format!("{ar} {}", arabic_digits(h.reference())) } else { format!("{} {}", t(name), h.reference()) }
        }
    }
}

/// Where to read an item online: quran.com for verses, the dataset's
/// sunnah.com address for hadith.
pub fn link(item: &Item) -> String {
    match item.source {
        Source::Verse(v) => format!("https://quran.com/{}/{}", v.surah, v.ayah),
        Source::Hadith(h) => h.url.clone(),
    }
}

/// A citation to paste: the reference and the link.
pub fn citation(item: &Item, arabic: bool) -> String {
    format!("{} · {}", reference(item, arabic), link(item))
}

/// The whole item to paste: the Arabic, its meaning (when there is one)
/// and the reference.
pub fn copy_text(item: &Item, meaning: Option<&str>, arabic: bool) -> String {
    let mark = item.verse_ref().map(|(_, a)| format!(" {}", ayah_mark(a))).unwrap_or_default();
    match meaning.filter(|m| !m.trim().is_empty()) {
        Some(m) => format!("{}{mark}\n\n{}\n\n— {}", item.arabic, m.trim(), reference(item, arabic)),
        None => format!("{}{mark}\n\n— {}", item.arabic, reference(item, arabic)),
    }
}

// ── Paths ────────────────────────────────────────────────────────────────

/// The path groups in display order.
pub const GROUPS: [PathGroup; 6] = [PathGroup::Worship, PathGroup::Character, PathGroup::Family, PathGroup::Others, PathGroup::Heart, PathGroup::Avoid];

/// A group's stable id and its heading (a catalog key).
pub fn group_label(g: &PathGroup) -> (&'static str, &'static str) {
    match g {
        PathGroup::Worship => ("worship", tk("Worship")),
        PathGroup::Character => ("character", tk("Character")),
        PathGroup::Family => ("family", tk("Family and kin")),
        PathGroup::Others => ("others", tk("People around you")),
        PathGroup::Heart => ("heart", tk("The heart")),
        PathGroup::Avoid => ("avoid", tk("Keep away from")),
    }
}

/// Whether `g` is the "keep away from" group.
pub fn is_avoid(g: &PathGroup) -> bool {
    matches!(g, PathGroup::Avoid)
}

/// Whether two groups are the same.
pub fn same_group(a: &PathGroup, b: &PathGroup) -> bool {
    group_label(a).0 == group_label(b).0
}

/// The path with `key`.
pub fn path(key: &str) -> Option<&'static Path> {
    PATHS.iter().find(|p| p.key == key)
}

/// The items a path cites, in its order (unknown ids skipped).
pub fn evidence(p: &Path) -> Vec<ItemId> {
    let c = catalog();
    p.evidence.iter().filter_map(|e| c.evidence(e)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_find_their_items() {
        let c = catalog();
        assert_eq!(c.items.len(), data::quran().verses.len() + data::all_hadith().len());
        let v = c.find("2:25").expect("2:25");
        assert_eq!(c.find("quran-2:25"), Some(v));
        assert_eq!(c.find(" Quran 2:25 "), Some(v));
        assert_eq!(c.get(v).map(|i| i.key.as_str()), Some("quran-2:25"));
        let h = c.find("bukhari-3245").expect("bukhari-3245");
        assert_eq!(c.find("Bukhari 3245"), Some(h));
        assert_eq!(c.find("bukhari:3245"), Some(h));
        assert!(c.find("2:9999").is_none() && c.find("muslim-0").is_none() && c.find("nonsense").is_none());
        // Suffixed sunnah.com references.
        let m = c.find("muslim-2699a").expect("muslim-2699a");
        assert_eq!(c.find("Muslim 2699a"), Some(m));
        assert_eq!(c.find("muslim:2699"), Some(m), "the first narration");
        assert_eq!(c.get(m).map(|i| i.key.as_str()), Some("muslim-2699a"));
        let h = c.get(m).unwrap();
        assert_eq!(reference(h, false), "Sahih Muslim 2699a");
        assert_eq!(link(h), h.hadith().unwrap().url);
        assert!(c.items[..c.verses].iter().all(Item::is_verse) && !c.items[c.verses..].iter().any(Item::is_verse));
    }

    #[test]
    fn a_hadith_card_shows_the_words_not_the_chain() {
        // Made-up words in the dataset's layout (no scripture typed here).
        let text = "حَدَّثَنَا فُلاَنٌ، عَنْ فُلاَنٍ قَالَ ‏ \"‏ كَلاَمٌ طَيِّبٌ جَمِيلٌ ‏\"‏‏.‏";
        assert_eq!(matn(text), "كَلاَمٌ طَيِّبٌ جَمِيلٌ");
        // A second chain after the words is left out.
        let two = format!("{text} وَحَدَّثَنِي فُلاَنٌ ‏\"‏ كَلاَمٌ آخَرُ هُنَا ‏\"‏");
        assert_eq!(matn(&two), "كَلاَمٌ طَيِّبٌ جَمِيلٌ");
        // No quotation: the whole text.
        assert_eq!(matn(" قَالَ كَذَا وَكَذَا "), "قَالَ كَذَا وَكَذَا");
        let c = catalog();
        let h = c.get(c.find("bukhari-6474").unwrap()).unwrap();
        assert!(!h.arabic.contains('"') && !fold(h.arabic).contains(&fold("حدثنا")) && h.full_arabic().contains(h.arabic), "{}", h.arabic);
        assert!(h.arabic.chars().count() * 2 < h.full_arabic().chars().count());
        assert!(h.full_arabic().split_whitespace().next().is_some_and(|w| fold(w) == fold("حدثنا")));
    }

    #[test]
    fn references_citations_and_links() {
        let c = catalog();
        let v = c.get(c.find("2:25").unwrap()).unwrap();
        assert_eq!(reference(v, false), "Al-Baqara 2:25");
        assert_eq!(reference(v, true), "البقرة ٢:٢٥");
        assert_eq!(link(v), "https://quran.com/2/25");
        assert_eq!(citation(v, false), "Al-Baqara 2:25 · https://quran.com/2/25");
        assert_eq!(ayah_mark(25), "\u{FD3F}٢٥\u{FD3E}");
        let h = c.get(c.find("bukhari-3245").unwrap()).unwrap();
        assert_eq!(reference(h, false), "Sahih al-Bukhari 3245");
        assert_eq!(reference(h, true), "صحيح البخاري ٣٢٤٥");
        assert_eq!(link(h), "https://sunnah.com/bukhari:3245");
        let copied = copy_text(v, Some("And give good tidings"), false);
        assert!(copied.starts_with(v.arabic) && copied.contains("﴿٢٥﴾") && copied.contains("And give good tidings") && copied.ends_with("— Al-Baqara 2:25"));
        assert!(!copy_text(h, None, true).contains("\n\n\n"));
    }

    #[test]
    fn paths_resolve_their_evidence() {
        for p in PATHS {
            assert_eq!(evidence(p).len(), p.evidence.len(), "{}", p.key);
        }
        assert!(GROUPS.iter().any(is_avoid));
        assert!(same_group(&PathGroup::Heart, &PathGroup::Heart) && !same_group(&PathGroup::Heart, &PathGroup::Avoid));
    }
}
