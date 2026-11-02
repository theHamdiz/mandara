//! The collections the hadith-api dataset offers (checked against its
//! `editions.json` and `info.json` on 2026-11-02): their names, how many
//! entries each edition has, how many carry Arabic text (the hadith the
//! library keeps), which translations exist, and the sizes of the files.

use mandara_i18n::tk;

pub use mandara_config::widget_kinds::HadithCollection as Collection;

/// What the library knows about a collection before downloading it.
#[derive(Debug)]
pub struct Info {
    /// The full English name, as cited (a catalog key).
    pub title: &'static str,
    /// The short English name used in references (`Bukhari 1`).
    pub short: &'static str,
    /// The Arabic name.
    pub arabic: &'static str,
    /// Entries in every edition (`info.json`'s count, the same in each).
    pub entries: usize,
    /// Entries with Arabic text: the hadith the library keeps.
    pub hadith: usize,
    /// Downloaded unless switched off in Studio.
    pub default_on: bool,
    /// The dataset graded it (the four Sunan and the Muwatta).
    pub graded: bool,
    /// Every hadith is sahih by consensus (Bukhari, Muslim).
    pub sahih: bool,
    /// Translation editions the dataset has (`eng`, `fra`, …).
    pub langs: &'static [&'static str],
    /// Sizes of the Arabic and English editions, bytes.
    pub arabic_bytes: u64,
    pub english_bytes: u64,
}

const ALL_LANGS: &[&str] = &["eng", "fra", "ind", "rus", "tur"];
const NO_RUSSIAN: &[&str] = &["eng", "fra", "ind", "tur"];

/// Every collection's facts, in [`Collection::ALL`]'s order.
static INFOS: [Info; 10] = [
    Info { title: tk("Sahih al-Bukhari"), short: tk("Bukhari"), arabic: "صحيح البخاري", entries: 7589, hadith: 7580, default_on: true, graded: false, sahih: true, langs: ALL_LANGS, arabic_bytes: 9_409_647, english_bytes: 4_821_289 },
    Info { title: tk("Sahih Muslim"), short: tk("Muslim"), arabic: "صحيح مسلم", entries: 7563, hadith: 7360, default_on: true, graded: false, sahih: true, langs: ALL_LANGS, arabic_bytes: 8_329_204, english_bytes: 3_940_216 },
    Info { title: tk("Sunan Abu Dawud"), short: tk("Abu Dawud"), arabic: "سنن أبي داود", entries: 5274, hadith: 5272, default_on: false, graded: true, sahih: false, langs: ALL_LANGS, arabic_bytes: 6_623_115, english_bytes: 3_589_001 },
    Info { title: tk("Jami' at-Tirmidhi"), short: tk("Tirmidhi"), arabic: "جامع الترمذي", entries: 3998, hadith: 3924, default_on: false, graded: true, sahih: false, langs: &["eng", "ind", "tur"], arabic_bytes: 6_583_667, english_bytes: 2_587_658 },
    Info { title: tk("Sunan an-Nasa'i"), short: tk("Nasa'i"), arabic: "سنن النسائي", entries: 5765, hadith: 5679, default_on: false, graded: true, sahih: false, langs: NO_RUSSIAN, arabic_bytes: 6_447_219, english_bytes: 3_328_851 },
    Info { title: tk("Sunan Ibn Majah"), short: tk("Ibn Majah"), arabic: "سنن ابن ماجه", entries: 4343, hadith: 4338, default_on: false, graded: true, sahih: false, langs: NO_RUSSIAN, arabic_bytes: 4_971_752, english_bytes: 2_629_738 },
    Info { title: tk("Muwatta Malik"), short: tk("Malik"), arabic: "موطأ مالك", entries: 1858, hadith: 1829, default_on: false, graded: true, sahih: false, langs: NO_RUSSIAN, arabic_bytes: 2_008_602, english_bytes: 1_473_225 },
    Info { title: tk("An-Nawawi's Forty"), short: tk("Nawawi"), arabic: "الأربعون النووية", entries: 42, hadith: 42, default_on: false, graded: false, sahih: false, langs: &["eng", "fra", "tur"], arabic_bytes: 47_261, english_bytes: 33_930 },
    Info { title: tk("Forty Hadith Qudsi"), short: tk("Qudsi"), arabic: "الأربعون القدسية", entries: 40, hadith: 40, default_on: false, graded: false, sahih: false, langs: &["eng", "fra"], arabic_bytes: 53_273, english_bytes: 39_414 },
    Info { title: tk("Shah Waliullah's Forty"), short: tk("Dehlawi"), arabic: "أربعون الدهلوي", entries: 40, hadith: 40, default_on: false, graded: false, sahih: false, langs: &["eng", "fra"], arabic_bytes: 7_183, english_bytes: 6_106 },
];

/// A collection's facts.
pub fn info(c: Collection) -> &'static Info {
    &INFOS[c as usize]
}

/// The collections downloaded unless switched off.
pub fn defaults() -> Vec<Collection> {
    Collection::ALL.into_iter().filter(|c| info(*c).default_on).collect()
}

/// Reads a list of collection ids (unknown ones skipped, duplicates kept
/// once, in the library's order).
pub fn parse_list(ids: &[String]) -> Vec<Collection> {
    Collection::ALL.into_iter().filter(|c| ids.iter().any(|id| alias(id.trim()) == Some(*c))).collect()
}

/// A collection by its id or a common name (`abu-dawud`, `ibn majah`,
/// `nawawi40`, `shahwaliullah40`), ignoring case.
pub fn alias(name: &str) -> Option<Collection> {
    let key: String = name.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    Some(match key.as_str() {
        "bukhari" | "sahihbukhari" | "sahihalbukhari" => Collection::Bukhari,
        "muslim" | "sahihmuslim" => Collection::Muslim,
        "abudawud" | "abudaud" | "dawud" | "sunanabudawud" => Collection::Abudawud,
        "tirmidhi" | "tirmizi" | "jamiattirmidhi" => Collection::Tirmidhi,
        "nasai" | "nasa" | "sunannasai" | "sunanannasai" => Collection::Nasai,
        "ibnmajah" | "majah" | "sunanibnmajah" => Collection::Ibnmajah,
        "malik" | "muwatta" | "muwattamalik" => Collection::Malik,
        "nawawi" | "nawawi40" | "forty" => Collection::Nawawi,
        "qudsi" | "qudsi40" => Collection::Qudsi,
        "dehlawi" | "shahwaliullah" | "shahwaliullah40" => Collection::Dehlawi,
        _ => return None,
    })
}

/// The dataset's language code for an interface language (`en` → `eng`);
/// `None` in Arabic (the library is Arabic first: no translation).
pub fn dataset_lang(ui: &str) -> Option<&'static str> {
    Some(match ui {
        "ar" => return None,
        "fr" => "fra",
        "id" => "ind",
        "ru" => "rus",
        "tr" => "tur",
        _ => "eng",
    })
}

/// The translation edition a collection is downloaded with for an
/// interface language: `(dataset language, is the English fallback)`.
/// English stands in when the dataset has no edition in that language.
pub fn translation(c: Collection, ui: &str) -> Option<(&'static str, bool)> {
    let wanted = dataset_lang(ui)?;
    let lang = if info(c).langs.contains(&wanted) { wanted } else { "eng" };
    Some((lang, lang == "eng" && ui != "en"))
}

/// The edition's name in the dataset (`ara-bukhari`, `eng-muslim`).
pub fn edition(c: Collection, lang: &str) -> String {
    format!("{lang}-{}", c.id())
}

/// About how many bytes a collection downloads with `translation`.
pub fn download_size(c: Collection, translation: Option<&str>) -> u64 {
    let i = info(c);
    // Other languages run about as long as the English.
    i.arabic_bytes + translation.map_or(0, |_| i.english_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collections_ids_and_aliases() {
        assert_eq!(Collection::ALL.len(), 10);
        for c in Collection::ALL {
            assert_eq!(alias(c.id()), Some(c));
            assert_eq!(Collection::from_id(c.id()), Some(c));
            let i = info(c);
            assert!(i.hadith <= i.entries && i.hadith > 0 && i.langs.contains(&"eng"));
            assert!(!(i.sahih && i.graded));
            // The table is in the enum's order.
            assert_eq!(alias(i.short), Some(c), "{}", i.short);
        }
        assert_eq!(alias("Abu Dawud"), Some(Collection::Abudawud));
        assert_eq!(alias("ibn-majah"), Some(Collection::Ibnmajah));
        assert_eq!(alias("Nasa'i"), Some(Collection::Nasai));
        assert_eq!(alias("nope"), None);
        assert_eq!(defaults(), vec![Collection::Bukhari, Collection::Muslim]);
        assert_eq!(parse_list(&["muslim".into(), " bukhari".into(), "x".into(), "muslim".into()]), vec![Collection::Bukhari, Collection::Muslim]);
    }

    #[test]
    fn translations_follow_the_interface() {
        assert_eq!(translation(Collection::Bukhari, "en"), Some(("eng", false)));
        assert_eq!(translation(Collection::Bukhari, "ru"), Some(("rus", false)));
        // No Russian Tirmidhi: English, labelled as such.
        assert_eq!(translation(Collection::Tirmidhi, "ru"), Some(("eng", true)));
        assert_eq!(translation(Collection::Tirmidhi, "fr"), Some(("eng", true)));
        assert_eq!(translation(Collection::Muslim, "de"), Some(("eng", true)));
        assert_eq!(translation(Collection::Muslim, "ar"), None);
        assert_eq!(edition(Collection::Abudawud, "ara"), "ara-abudawud");
        assert!(download_size(Collection::Bukhari, Some("eng")) > download_size(Collection::Bukhari, None));
    }
}
