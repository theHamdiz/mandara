//! Meanings in the reader's language. Qur'an translations are copyrighted,
//! so none is bundled: the edition for the interface language is fetched
//! once from api.alquran.cloud (Tanzil's translations), only the verses in
//! the collection are kept, and they are cached in
//! `~/.cache/mandara/jannah/<edition>.json`. Hadith meanings come from the
//! dataset (English, Indonesian, Turkish, Russian, French) or, for German,
//! Spanish, Hindi, Chinese and Japanese, from AI renderings of the meaning
//! (`hadith-ai-<lang>.json`), which the interface always labels as such.
//!
//! Fetching is blocking: call [`fetch`] off the UI thread. Offline mode
//! blocks it (the caller shows the Arabic with a note).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use mandara_i18n::tf;
use serde::{Deserialize, Serialize};

use super::catalog::Item;
use super::data::{self, Hadith};

/// A Qur'an translation edition.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Edition {
    /// api.alquran.cloud / Tanzil identifier (`en.sahih`).
    pub id: &'static str,
    /// Who translated it, as shown under the meaning.
    pub translator: &'static str,
}

/// The edition per interface language (Arabic has none: Arabic only).
const EDITIONS: [(&str, Edition); 14] = [
    ("en", Edition { id: "en.sahih", translator: "Saheeh International" }),
    ("id", Edition { id: "id.indonesian", translator: "Kementerian Agama Republik Indonesia" }),
    ("tr", Edition { id: "tr.diyanet", translator: "Diyanet İşleri" }),
    ("ru", Edition { id: "ru.kuliev", translator: "Эльмир Кулиев" }),
    ("fr", Edition { id: "fr.hamidullah", translator: "Muhammad Hamidullah" }),
    ("de", Edition { id: "de.bubenheim", translator: "A. S. F. Bubenheim & N. Elyas" }),
    ("es", Edition { id: "es.cortes", translator: "Julio Cortés" }),
    ("zh", Edition { id: "zh.jian", translator: "马坚" }),
    ("ja", Edition { id: "ja.japanese", translator: "日本語訳 (Tanzil)" }),
    ("hi", Edition { id: "hi.hindi", translator: "सुहेल फ़ारूक़ ख़ान और सैफ़ुर्रहमान नदवी" }),
    ("fa", Edition { id: "fa.makarem", translator: "ناصر مکارم شیرازی" }),
    ("ur", Edition { id: "ur.jalandhry", translator: "فتح محمد جالندھری" }),
    ("bn", Edition { id: "bn.bengali", translator: "মুহিউদ্দীন খান" }),
    ("ko", Edition { id: "ko.korean", translator: "한국어 번역 (Tanzil)" }),
];

/// The Qur'an edition for a language code (`None` for Arabic and
/// languages without one).
pub fn edition(lang: &str) -> Option<Edition> {
    EDITIONS.iter().find(|(l, _)| *l == lang).map(|(_, e)| *e)
}

/// `(surah, ayah)` → meaning.
pub type Meanings = HashMap<(u16, u16), String>;

/// Whole-request timeout (the edition is about 1.6 MB).
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);
/// Largest edition accepted.
const MAX_BYTES: u64 = 16 << 20;
/// Edition endpoint.
const API: &str = "https://api.alquran.cloud/v1/quran/";

/// Where an edition's verses are cached.
pub fn cache_path(edition: &str) -> PathBuf {
    mandara_config::paths::cache_dir().join("jannah").join(format!("{edition}.json"))
}

/// The cache file.
#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheFile {
    edition: String,
    /// `"2:25"` → meaning.
    verses: BTreeMap<String, String>,
}

/// Every `(surah, ayah)` in the collection.
fn wanted() -> HashSet<(u16, u16)> {
    data::quran().verses.iter().map(|v| (v.surah, v.ayah)).collect()
}

/// The cached meanings of `edition`, when they cover every verse of the
/// collection (a grown collection fetches again).
pub fn load_cached(edition: &str) -> Option<Meanings> {
    let text = std::fs::read_to_string(cache_path(edition)).ok()?;
    let file: CacheFile = serde_json::from_str(&text).ok()?;
    if file.edition != edition {
        return None;
    }
    let map: Meanings = file
        .verses
        .into_iter()
        .filter_map(|(k, v)| {
            let (s, a) = k.split_once(':')?;
            Some(((s.parse().ok()?, a.parse().ok()?), v))
        })
        .collect();
    wanted().iter().all(|k| map.contains_key(k)).then_some(map)
}

/// The verses of `wanted` in an api.alquran.cloud `/v1/quran/<edition>`
/// response.
pub fn parse_edition(json: &str, wanted: &HashSet<(u16, u16)>) -> Result<Meanings, String> {
    #[derive(Deserialize)]
    struct Response {
        data: Body,
    }
    #[derive(Deserialize)]
    struct Body {
        surahs: Vec<SurahBody>,
    }
    #[derive(Deserialize)]
    struct SurahBody {
        number: u16,
        ayahs: Vec<Ayah>,
    }
    #[derive(Deserialize)]
    struct Ayah {
        #[serde(rename = "numberInSurah")]
        number_in_surah: u16,
        text: String,
    }
    let r: Response = serde_json::from_str(json).map_err(|e| tf("Unexpected answer from the Qur'an service: {error}", &[("error", &e)]))?;
    let mut out = Meanings::new();
    for s in r.data.surahs {
        for a in s.ayahs {
            if wanted.contains(&(s.number, a.number_in_surah)) {
                out.insert((s.number, a.number_in_surah), a.text.trim().to_string());
            }
        }
    }
    if out.is_empty() && !wanted.is_empty() {
        return Err(tf("The Qur'an service sent no verses", &[]));
    }
    Ok(out)
}

/// Downloads `edition`, keeps the collection's verses and caches them.
/// Blocking; honours offline mode.
pub fn fetch(edition: &str) -> Result<Meanings, String> {
    crate::offline::check()?;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(HTTP_TIMEOUT)).build().into();
    let mut response = agent
        .get(&format!("{API}{edition}"))
        .header("User-Agent", concat!("mandara-shell/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| tf("Request failed: {error}", &[("error", &e)]))?;
    let body = response.body_mut().with_config().limit(MAX_BYTES).read_to_string().map_err(|e| tf("Request failed: {error}", &[("error", &e)]))?;
    let map = parse_edition(&body, &wanted())?;
    store(edition, &map);
    Ok(map)
}

fn store(edition: &str, map: &Meanings) {
    let file = CacheFile { edition: edition.to_string(), verses: map.iter().map(|((s, a), v)| (format!("{s}:{a}"), v.clone())).collect() };
    let path = cache_path(edition);
    let written = path.parent().map(std::fs::create_dir_all).transpose().map_err(anyhow::Error::from).and_then(|_| {
        let json = serde_json::to_vec(&file)?;
        mandara_config::store::write_atomic(&path, &json)
    });
    if let Err(e) = written {
        tracing::warn!("jannah: caching {}: {e:#}", path.display());
    }
}

// ── Hadith ───────────────────────────────────────────────────────────────

/// Languages whose hadith meanings are AI renderings.
pub const AI_LANGS: [&str; 5] = ["de", "es", "hi", "zh", "ja"];

/// `hadith-ai-<lang>.json`.
#[derive(Debug, Default, Deserialize)]
pub struct AiRenderings {
    /// What the renderings are (shown in the About sheet).
    #[serde(default)]
    pub note: String,
    /// Hadith id → rendering.
    #[serde(default)]
    pub text: HashMap<String, String>,
}

fn ai_json(lang: &str) -> Option<&'static str> {
    Some(match lang {
        "de" => include_str!("../../data/jannah/hadith-ai-de.json"),
        "es" => include_str!("../../data/jannah/hadith-ai-es.json"),
        "hi" => include_str!("../../data/jannah/hadith-ai-hi.json"),
        "zh" => include_str!("../../data/jannah/hadith-ai-zh.json"),
        "ja" => include_str!("../../data/jannah/hadith-ai-ja.json"),
        _ => return None,
    })
}

/// The AI renderings for `lang`, parsed on first use.
pub fn ai_renderings(lang: &str) -> Option<&'static AiRenderings> {
    static CELLS: [OnceLock<AiRenderings>; 5] = [const { OnceLock::new() }; 5];
    let i = AI_LANGS.iter().position(|l| *l == lang)?;
    let json = ai_json(lang)?;
    Some(CELLS[i].get_or_init(|| {
        serde_json::from_str(json).unwrap_or_else(|e| {
            tracing::warn!("jannah: hadith-ai-{lang}.json: {e}");
            AiRenderings::default()
        })
    }))
}

/// A hadith's meaning in `lang`, and whether it is an AI rendering (in
/// German, Spanish, Hindi, Chinese and Japanese the AI rendering comes
/// first; elsewhere the dataset's translation).
pub fn hadith_meaning(h: &'static Hadith, lang: &str) -> Option<(&'static str, bool)> {
    if let Some(t) = ai_renderings(lang).and_then(|r| r.text.get(&h.id)).filter(|t| !t.trim().is_empty()) {
        return Some((t.as_str(), true));
    }
    h.text.get(lang).filter(|t| !t.trim().is_empty()).map(|t| (t, false))
}

/// What a card shows under the Arabic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meaning {
    pub text: String,
    /// Under the text: the translator, or the dataset's name.
    pub credit: String,
    /// An AI rendering of the meaning (always labelled).
    pub ai: bool,
}

/// An item's meaning in `lang` (`meanings` holds the fetched Qur'an
/// edition, when there is one).
pub fn meaning(item: &Item, lang: &str, meanings: Option<&Meanings>) -> Option<Meaning> {
    if let Some((s, a)) = item.verse_ref() {
        let e = edition(lang)?;
        let text = meanings?.get(&(s, a))?.clone();
        return Some(Meaning { text, credit: e.translator.to_string(), ai: false });
    }
    let h = item.hadith()?;
    let (text, ai) = hadith_meaning(h, lang)?;
    Some(Meaning { text: text.to_string(), credit: String::new(), ai })
}

// ── Excerpts ─────────────────────────────────────────────────────────────

/// Opening quotation marks: an excerpt starts with the quoted words.
const OPENERS: [char; 5] = ['"', '“', '«', '„', '「'];
/// Sentence ends.
const ENDS: [char; 6] = ['.', '!', '?', '。', '।', '؟'];
/// Shortest sentence kept on its own, and the longest excerpt.
const MIN_SENTENCE: usize = 24;
const MAX_EXCERPT: usize = 180;

/// The first sentence of a meaning, starting at the quoted words (a
/// hadith's "Narrated …:" is skipped), at most [`MAX_EXCERPT`] characters.
pub fn excerpt(text: &str) -> String {
    let start = text.find(OPENERS).map(|i| i + text[i..].chars().next().map_or(1, char::len_utf8)).unwrap_or(0);
    let body = text[start..].trim_start_matches(|c: char| c.is_whitespace() || OPENERS.contains(&c));
    let mut out = String::new();
    for (n, c) in body.chars().enumerate() {
        out.push(c);
        if ENDS.contains(&c) && n + 1 >= MIN_SENTENCE {
            break;
        }
        if n + 1 >= MAX_EXCERPT {
            let cut = out.rfind(' ').filter(|i| *i > MIN_SENTENCE).unwrap_or(out.len());
            out.truncate(cut);
            out.push('…');
            break;
        }
    }
    out.trim().trim_end_matches(['"', '”', '»', ',', ';', ':']).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::super::catalog::catalog;
    use super::*;

    #[test]
    fn every_interface_language_maps_to_an_existing_edition() {
        assert_eq!(edition("en").map(|e| e.id), Some("en.sahih"));
        assert_eq!(edition("ar"), None);
        for (lang, id) in [("id", "id.indonesian"), ("tr", "tr.diyanet"), ("ru", "ru.kuliev"), ("fr", "fr.hamidullah"), ("de", "de.bubenheim"), ("es", "es.cortes"), ("zh", "zh.jian"), ("ja", "ja.japanese"), ("hi", "hi.hindi"), ("fa", "fa.makarem"), ("ur", "ur.jalandhry"), ("bn", "bn.bengali"), ("ko", "ko.korean")] {
            assert_eq!(edition(lang).map(|e| e.id), Some(id));
        }
        for lang in mandara_i18n::Lang::ALL {
            assert_eq!(edition(lang.code()).is_none(), lang == mandara_i18n::Lang::Ar, "{lang:?}");
        }
        assert!(cache_path("en.sahih").ends_with("jannah/en.sahih.json"));
    }

    #[test]
    fn an_edition_response_keeps_only_the_collection() {
        let json = r#"{"code":200,"status":"OK","data":{"surahs":[{"number":2,"name":"x","ayahs":[{"number":31,"text":"a","numberInSurah":24},{"number":32,"text":" And give good tidings ","numberInSurah":25}]}],"edition":{"identifier":"en.sahih"}}}"#;
        let want: HashSet<(u16, u16)> = [(2, 25), (3, 133)].into();
        let m = parse_edition(json, &want).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[&(2, 25)], "And give good tidings");
        assert!(parse_edition("{}", &want).is_err());
        assert!(parse_edition(r#"{"data":{"surahs":[]}}"#, &want).is_err());
    }

    #[test]
    fn hadith_meanings_and_ai_renderings() {
        let c = catalog();
        let item = c.get(c.find("bukhari-6094").unwrap()).unwrap();
        let m = meaning(item, "en", None).unwrap();
        assert!(m.text.contains("Truthfulness") && !m.ai);
        assert!(meaning(item, "ar", None).is_none());
        // German has no dataset translation: only an AI rendering may fill it.
        assert!(meaning(item, "de", None).is_none_or(|m| m.ai));
        assert!(ai_renderings("de").is_some() && ai_renderings("fr").is_none());
        let verse = c.get(c.find("2:25").unwrap()).unwrap();
        assert!(meaning(verse, "en", None).is_none(), "not fetched yet");
        let fetched: Meanings = [((2, 25), "And give good tidings".to_string())].into();
        assert_eq!(meaning(verse, "en", Some(&fetched)).map(|m| m.credit), Some("Saheeh International".into()));
    }

    #[test]
    fn every_ai_rendering_covers_every_hadith() {
        for lang in AI_LANGS {
            let r = ai_renderings(lang).expect("an AI language");
            assert!(!r.note.is_empty(), "{lang}: the note says what it is");
            for h in super::super::data::all_hadith() {
                assert!(r.text.get(&h.id).is_some_and(|t| !t.trim().is_empty()), "{lang}: no rendering of {}", h.id);
            }
            assert_eq!(r.text.len(), super::super::data::all_hadith().len(), "{lang}: renderings of hadith that aren't in the data");
        }
    }

    #[test]
    fn excerpts_start_at_the_words() {
        let t = "Narrated `Abdullah:The Prophet (ﷺ) said, \"Truthfulness leads to righteousness, and righteousness leads to Paradise. And a man keeps on telling the truth";
        assert_eq!(excerpt(t), "Truthfulness leads to righteousness, and righteousness leads to Paradise.");
        assert_eq!(excerpt("Rapporté par X : « La véracité mène à la piété. Et »"), "La véracité mène à la piété.");
        let long = "word ".repeat(80);
        let e = excerpt(&long);
        assert!(e.ends_with('…') && e.chars().count() <= MAX_EXCERPT + 1);
        assert_eq!(excerpt("And give good tidings to those who believe."), "And give good tidings to those who believe.");
    }
}
