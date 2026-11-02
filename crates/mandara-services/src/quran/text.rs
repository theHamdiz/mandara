//! The Qur'an as the plugin keeps it: the Tanzil Uthmani text (verbatim)
//! with the surah metadata, parsed from api.alquran.cloud's
//! `/v1/quran/quran-uthmani` and `/v1/meta` answers, verified (114 surahs,
//! 6236 ayahs, every surah's count, consecutive numbering, 30 ajza') and
//! stored as one compact JSON file. Meanings ([`Translation`]) come from
//! the same API's `/v1/quran/<edition>`.
//!
//! The API writes the basmala in front of the first ayah of every surah
//! but the first (where it is the first ayah) and the ninth (which has
//! none). The mushaf sets it as an unnumbered heading, so it is split off
//! into [`Quran::bismillah`], letter for letter as the text has it.
//!
//! References: [`Ref`] is `surah:ayah`; "global" numbers run 1..=6236
//! across the whole Qur'an (the recitation files are named by them).

use std::ops::RangeInclusive;

use mandara_i18n::tf;
use serde::{Deserialize, Serialize};

use crate::jannah::search::fold;

/// Surahs, ayahs and ajza' in the Qur'an.
pub const SURAHS: u16 = 114;
pub const AYAHS: u16 = 6236;
pub const JUZS: u8 = 30;
/// The edition the text comes from (Tanzil's Uthmani script).
pub const TEXT_EDITION: &str = "quran-uthmani";
/// Bumped when the stored layout changes (older files are downloaded again).
pub const FORMAT: u32 = 1;
/// Words of the basmala written in front of an ayah.
const BASMALA_WORDS: usize = 4;
/// The basmala, folded (diacritics and alifs dropped, see `fold`).
const BASMALA_FOLDED: &str = "بسم لله لرحمن لرحيم";
/// The surah whose first ayah is the basmala, and the one without it.
const OPENING: u16 = 1;
const REPENTANCE: u16 = 9;
/// The byte-order mark the API leaves in front of the first ayah.
const BOM: char = '\u{FEFF}';

/// A place in the Qur'an: `surah:ayah`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Ref {
    pub surah: u16,
    pub ayah: u16,
}

impl Ref {
    pub const fn new(surah: u16, ayah: u16) -> Self {
        Self { surah, ayah }
    }
}

impl std::fmt::Display for Ref {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.surah, self.ayah)
    }
}

/// Where a surah was revealed.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Revelation {
    Makki,
    Madani,
}

impl Revelation {
    pub fn id(self) -> &'static str {
        match self {
            Revelation::Makki => "makki",
            Revelation::Madani => "madani",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Surah {
    pub number: u16,
    /// The Arabic name as the API writes it, with its diacritics
    /// (`سُورَةُ البَقَرَةِ`).
    pub name: String,
    /// The transliteration (`Al-Baqara`).
    pub transliteration: String,
    /// The English meaning of the name (`The Cow`).
    pub meaning: String,
    pub revelation: Revelation,
    pub ayahs: u16,
    /// The global number of its first ayah.
    pub first: u16,
}

impl Surah {
    /// The Arabic name without the word "surah" (`البَقَرَةِ`).
    pub fn short_name(&self) -> &str {
        let name = self.name.trim();
        match name.split_once(' ') {
            Some((head, rest)) if fold(head) == fold("سورة") => rest.trim(),
            _ => name,
        }
    }

    /// The global numbers of its ayahs.
    pub fn globals(&self) -> RangeInclusive<u16> {
        self.first..=self.first + self.ayahs - 1
    }

    /// Whether the basmala heads it (all but the first and the ninth).
    pub fn has_bismillah(&self) -> bool {
        self.number != OPENING && self.number != REPENTANCE
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ayah {
    /// The Tanzil Uthmani text, verbatim (a first ayah without the basmala
    /// the API puts in front of it).
    pub text: String,
    pub juz: u8,
    pub page: u16,
    pub hizb_quarter: u8,
    /// A verse of prostration.
    pub sajda: bool,
}

/// The whole Qur'an.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Quran {
    pub format: u32,
    pub edition: String,
    /// The basmala as the text writes it (the opening's first ayah).
    pub bismillah: String,
    pub surahs: Vec<Surah>,
    /// Every ayah in order (global number − 1).
    pub ayahs: Vec<Ayah>,
    /// Where each juz begins.
    pub juz_starts: Vec<Ref>,
}

/// What a reference names: a whole surah, one ayah, or a run of ayahs
/// (global numbers, inclusive).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    Surah(u16),
    Ayah(Ref),
    Range(u16, u16),
}

impl Quran {
    pub fn surah(&self, n: u16) -> Option<&Surah> {
        self.surahs.get(usize::from(n).checked_sub(1)?).filter(|s| s.number == n)
    }

    /// The global number of `r`.
    pub fn global(&self, r: Ref) -> Option<u16> {
        let s = self.surah(r.surah)?;
        (1..=s.ayahs).contains(&r.ayah).then(|| s.first + r.ayah - 1)
    }

    /// The reference of global number `g`.
    pub fn at(&self, g: u16) -> Option<Ref> {
        if !(1..=AYAHS).contains(&g) {
            return None;
        }
        let i = self.surahs.partition_point(|s| s.first <= g).checked_sub(1)?;
        let s = &self.surahs[i];
        Some(Ref::new(s.number, g - s.first + 1))
    }

    /// The ayah with global number `g`.
    pub fn ayah_at(&self, g: u16) -> Option<&Ayah> {
        self.ayahs.get(usize::from(g).checked_sub(1)?)
    }

    pub fn ayah(&self, r: Ref) -> Option<&Ayah> {
        self.ayah_at(self.global(r)?)
    }

    /// Where juz `n` (1–30) begins.
    pub fn juz_start(&self, n: u8) -> Option<Ref> {
        self.juz_starts.get(usize::from(n).checked_sub(1)?).copied()
    }

    /// The surah a name or number names: `2`, `al-baqara`, `Baqara`,
    /// `البقرة`, `سورة البقرة`, `The Cow` (diacritics, case, dashes and the
    /// article don't matter).
    pub fn find_surah(&self, name: &str) -> Option<u16> {
        let name = name.trim();
        if let Ok(n) = name.parse::<u16>() {
            return self.surah(n).map(|s| s.number);
        }
        let want = name_key(name);
        if want.is_empty() {
            return None;
        }
        let keys = |s: &Surah| [name_key(&s.transliteration), name_key(s.short_name()), name_key(&s.meaning)];
        let sounds = skeleton(&want);
        self.surahs
            .iter()
            .find(|s| keys(s).contains(&want))
            // Spelled another way (`yasin` for Yaseen, `fatiha` for
            // Faatiha): the closest of the names that sound alike.
            .or_else(|| {
                let alike = self.surahs.iter().map(|s| (s, name_key(&s.transliteration))).filter(|(_, k)| sounds.len() >= 2 && skeleton(k) == sounds);
                alike.min_by_key(|(_, k)| distance(k, &want)).map(|(s, _)| s)
            })
            .or_else(|| self.surahs.iter().find(|s| keys(s).iter().any(|k| k.starts_with(&want))))
            .map(|s| s.number)
    }

    /// Parses `2`, `2:255`, `2:255-260`, `2:255-3:5`, `2:255-` (to the
    /// surah's end), `al-baqara`, `al-baqara:255` or `البقرة 255`.
    pub fn select(&self, text: &str) -> Result<Selection, String> {
        let text = text.trim();
        let bad = || tf("No surah or ayah {reference} (try 2, 2:255, 2:255-260 or al-baqara)", &[("reference", &text)]);
        let (head, tail) = match text.rsplit_once('-').filter(|(a, _)| a.contains(':')) {
            Some((a, b)) => (a.trim(), Some(b.trim())),
            None => (text, None),
        };
        let start = self.reference(head).map_err(|()| bad())?;
        let Some(tail) = tail else {
            return Ok(match start {
                Some(r) => Selection::Ayah(r),
                None => Selection::Surah(self.find_surah(head).ok_or_else(bad)?),
            });
        };
        let start = start.ok_or_else(bad)?;
        let first = self.global(start).ok_or_else(bad)?;
        let last = if tail.is_empty() {
            let s = self.surah(start.surah).ok_or_else(bad)?;
            s.first + s.ayahs - 1
        } else if let Ok(n) = tail.parse::<u16>() {
            self.global(Ref::new(start.surah, n)).ok_or_else(bad)?
        } else {
            self.reference(tail).ok().flatten().and_then(|r| self.global(r)).ok_or_else(bad)?
        };
        if last < first {
            return Err(bad());
        }
        Ok(Selection::Range(first, last))
    }

    /// `Some(ref)` for `surah:ayah` / `name:ayah` / `name ayah`, `None` for a
    /// bare surah, `Err` when the surah is unknown or the ayah out of range.
    fn reference(&self, text: &str) -> Result<Option<Ref>, ()> {
        let (surah, ayah) = match text.rsplit_once([':', ' ']) {
            Some((s, a)) if a.trim().parse::<u16>().is_ok() && !s.trim().is_empty() => (s.trim(), a.trim().parse::<u16>().ok()),
            _ => (text, None),
        };
        let n = self.find_surah(surah).ok_or(())?;
        match ayah {
            None => Ok(None),
            Some(a) => {
                let r = Ref::new(n, a);
                self.global(r).map(|_| Some(r)).ok_or(())
            }
        }
    }

    /// The global numbers a selection covers.
    pub fn globals(&self, sel: Selection) -> RangeInclusive<u16> {
        match sel {
            Selection::Surah(n) => self.surah(n).map(Surah::globals).unwrap_or_else(nothing),
            Selection::Ayah(r) => self.global(r).map(|g| g..=g).unwrap_or_else(nothing),
            Selection::Range(a, b) => a..=b,
        }
    }

    /// Checks what a loaded file must hold.
    pub fn verify(&self) -> Result<(), String> {
        let total: usize = self.surahs.iter().map(|s| usize::from(s.ayahs)).sum();
        if self.format != FORMAT || self.surahs.len() != usize::from(SURAHS) || self.ayahs.len() != usize::from(AYAHS) || total != usize::from(AYAHS) {
            return Err(tf("The Qur'an text is incomplete: {surahs} surahs and {ayahs} ayahs", &[("surahs", &self.surahs.len()), ("ayahs", &self.ayahs.len())]));
        }
        let mut next = 1;
        for (i, s) in self.surahs.iter().enumerate() {
            if usize::from(s.number) != i + 1 || s.first != next || s.ayahs == 0 {
                return Err(tf("Surah {n} is out of order", &[("n", &(i + 1))]));
            }
            next += s.ayahs;
        }
        if self.juz_starts.len() != usize::from(JUZS) || self.juz_starts.iter().any(|r| self.global(*r).is_none()) || self.ayahs.iter().any(|a| a.text.trim().is_empty() || !(1..=JUZS).contains(&a.juz)) {
            return Err(tf("The Qur'an text is damaged", &[]));
        }
        Ok(())
    }
}

/// A range of no ayahs (an exhausted one).
fn nothing() -> RangeInclusive<u16> {
    let mut r = 1..=1;
    r.next();
    r
}

/// A name folded for matching: diacritics, case, the article (`al-`,
/// `ال`), "surah" and everything but letters and digits dropped.
fn name_key(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().filter(|w| !matches!(fold(w).as_str(), "سوره" | "surah" | "sura" | "")).collect();
    let joined = fold(&words.join(" "));
    let mut s: String = joined.chars().filter(|c| c.is_alphanumeric()).collect();
    for article in ["al", "an", "ar", "as", "at", "ad", "adh", "ash", "az", "aal", "the"] {
        if joined.starts_with(&format!("{article}-")) || joined.starts_with(&format!("{article} ")) {
            s = s[article.len()..].to_string();
            break;
        }
    }
    // `fold` drops every alif: the Arabic article is left as a lone lam.
    let article = words.first().is_some_and(|w| w.starts_with(['ا', 'ٱ']));
    match s.strip_prefix('ل') {
        Some(rest) if article && rest.chars().count() >= 2 => rest.to_string(),
        _ => s,
    }
}

/// A Latin name's consonants with doubled letters merged (`yaseen` →
/// `ysn`, `faatiha` → `fth`): transliterations differ in their vowels.
fn skeleton(key: &str) -> String {
    let mut out = String::new();
    for c in key.chars().filter(|c| c.is_ascii_alphabetic() && !matches!(c, 'a' | 'e' | 'i' | 'o' | 'u')) {
        if !out.ends_with(c) {
            out.push(c);
        }
    }
    out
}

/// Edit distance between two short names.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

// ── Parsing the API ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Envelope<T> {
    code: u16,
    data: T,
}

#[derive(Deserialize)]
struct EditionBody {
    surahs: Vec<SurahBody>,
    edition: EditionHead,
}

#[derive(Deserialize)]
struct EditionHead {
    identifier: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    name: String,
    #[serde(rename = "englishName", default)]
    english_name: String,
    #[serde(default)]
    direction: Option<String>,
}

#[derive(Deserialize)]
struct SurahBody {
    number: u16,
    ayahs: Vec<AyahBody>,
}

#[derive(Deserialize)]
struct AyahBody {
    number: u16,
    #[serde(rename = "numberInSurah")]
    number_in_surah: u16,
    text: String,
    #[serde(default)]
    juz: u8,
    #[serde(default)]
    page: u16,
    #[serde(rename = "hizbQuarter", default)]
    hizb_quarter: u8,
    /// `false`, or an object for a verse of prostration.
    #[serde(default)]
    sajda: serde_json::Value,
}

#[derive(Deserialize)]
struct MetaBody {
    ayahs: Count,
    surahs: SurahRefs,
    juzs: Refs,
}

#[derive(Deserialize)]
struct Count {
    count: u16,
}

#[derive(Deserialize)]
struct SurahRefs {
    count: u16,
    references: Vec<SurahMeta>,
}

#[derive(Deserialize)]
struct SurahMeta {
    number: u16,
    name: String,
    #[serde(rename = "englishName")]
    english_name: String,
    #[serde(rename = "englishNameTranslation")]
    english_name_translation: String,
    #[serde(rename = "numberOfAyahs")]
    number_of_ayahs: u16,
    #[serde(rename = "revelationType")]
    revelation_type: String,
}

#[derive(Deserialize)]
struct Refs {
    count: u16,
    references: Vec<RefBody>,
}

#[derive(Deserialize)]
struct RefBody {
    surah: u16,
    ayah: u16,
}

fn unexpected(e: impl std::fmt::Display) -> String {
    tf("Unexpected answer from the Qur'an service: {error}", &[("error", &e)])
}

/// The surah metadata (`/v1/meta`), checked: 114 surahs whose counts add
/// up to 6236 ayahs, and 30 ajza'.
pub fn parse_meta(json: &str) -> Result<(Vec<Surah>, Vec<Ref>), String> {
    let env: Envelope<MetaBody> = serde_json::from_str(json).map_err(unexpected)?;
    if env.code != 200 {
        return Err(unexpected(env.code));
    }
    let m = env.data;
    if m.surahs.count != SURAHS || m.surahs.references.len() != usize::from(SURAHS) || m.ayahs.count != AYAHS || m.juzs.count != u16::from(JUZS) || m.juzs.references.len() != usize::from(JUZS) {
        return Err(tf("The Qur'an service's index is incomplete", &[]));
    }
    let mut surahs = Vec::with_capacity(usize::from(SURAHS));
    let mut first = 1u16;
    for (i, s) in m.surahs.references.into_iter().enumerate() {
        if usize::from(s.number) != i + 1 || s.number_of_ayahs == 0 {
            return Err(tf("The Qur'an service's index is out of order", &[]));
        }
        let revelation = if s.revelation_type.eq_ignore_ascii_case("medinan") { Revelation::Madani } else { Revelation::Makki };
        surahs.push(Surah { number: s.number, name: s.name.trim().to_string(), transliteration: s.english_name, meaning: s.english_name_translation, revelation, ayahs: s.number_of_ayahs, first });
        first = first.checked_add(s.number_of_ayahs).ok_or_else(|| tf("The Qur'an service's index is out of order", &[]))?;
    }
    if first != AYAHS + 1 {
        return Err(tf("The Qur'an service's index is incomplete", &[]));
    }
    let juz = m.juzs.references.into_iter().map(|r| Ref::new(r.surah, r.ayah)).collect();
    Ok((surahs, juz))
}

/// Checks an edition's body against the surah index: every surah, every
/// ayah numbered in order (in the surah and globally), no empty text.
fn check_edition(body: &EditionBody, surahs: &[Surah]) -> Result<(), String> {
    if body.surahs.len() != surahs.len() {
        return Err(tf("Expected {want} surahs, got {got}", &[("want", &surahs.len()), ("got", &body.surahs.len())]));
    }
    for (s, meta) in body.surahs.iter().zip(surahs) {
        if s.number != meta.number || s.ayahs.len() != usize::from(meta.ayahs) {
            return Err(tf("Surah {n} has {got} ayahs, expected {want}", &[("n", &meta.number), ("got", &s.ayahs.len()), ("want", &meta.ayahs)]));
        }
        for (i, a) in s.ayahs.iter().enumerate() {
            if usize::from(a.number_in_surah) != i + 1 || usize::from(a.number) != usize::from(meta.first) + i || a.text.trim().is_empty() {
                return Err(tf("Ayah {surah}:{ayah} is damaged", &[("surah", &meta.number), ("ayah", &(i + 1))]));
            }
        }
    }
    Ok(())
}

/// `text` without the basmala in front (when its first four words are it).
fn split_basmala(text: &str) -> Option<(&str, &str)> {
    let mut end = 0;
    for (n, (i, _)) in text.match_indices(' ').enumerate() {
        if n + 1 == BASMALA_WORDS {
            end = i;
            break;
        }
    }
    if end == 0 || fold(&text[..end]) != BASMALA_FOLDED {
        return None;
    }
    let rest = text[end..].trim_start();
    (!rest.is_empty()).then_some((&text[..end], rest))
}

/// Builds the Qur'an from the text (`/v1/quran/quran-uthmani`) and the
/// index ([`parse_meta`]), verified.
pub fn build(text_json: &str, surahs: Vec<Surah>, juz_starts: Vec<Ref>) -> Result<Quran, String> {
    let env: Envelope<EditionBody> = serde_json::from_str(text_json).map_err(unexpected)?;
    if env.code != 200 {
        return Err(unexpected(env.code));
    }
    let body = env.data;
    if body.edition.identifier != TEXT_EDITION {
        return Err(unexpected(&body.edition.identifier));
    }
    check_edition(&body, &surahs)?;
    let mut bismillah = String::new();
    let mut ayahs = Vec::with_capacity(usize::from(AYAHS));
    for (s, meta) in body.surahs.into_iter().zip(&surahs) {
        for a in s.ayahs {
            let raw = a.text.trim_start_matches(BOM).trim();
            let text = if a.number_in_surah == 1 && meta.number == OPENING {
                bismillah = raw.to_string();
                raw
            } else if a.number_in_surah == 1 && meta.has_bismillah() {
                split_basmala(raw).map_or(raw, |(_, rest)| rest)
            } else {
                raw
            };
            ayahs.push(Ayah { text: text.to_string(), juz: a.juz, page: a.page, hizb_quarter: a.hizb_quarter, sajda: !a.sajda.is_boolean() && !a.sajda.is_null() });
        }
    }
    let q = Quran { format: FORMAT, edition: TEXT_EDITION.into(), bismillah, surahs, ayahs, juz_starts };
    q.verify()?;
    Ok(q)
}

/// A meaning of the Qur'an in one language (one translation edition).
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Translation {
    /// `en.sahih`.
    pub edition: String,
    /// Who translated it.
    pub name: String,
    pub language: String,
    pub rtl: bool,
    /// Every ayah's meaning, in order (global number − 1).
    pub texts: Vec<String>,
}

impl Translation {
    pub fn at(&self, g: u16) -> Option<&str> {
        self.texts.get(usize::from(g).checked_sub(1)?).map(String::as_str)
    }

    pub fn verify(&self) -> Result<(), String> {
        if self.texts.len() != usize::from(AYAHS) || self.texts.iter().any(|t| t.trim().is_empty()) {
            return Err(tf("The meaning {edition} is incomplete", &[("edition", &self.edition)]));
        }
        Ok(())
    }
}

/// A translation edition (`/v1/quran/<edition>`), checked against the
/// index.
pub fn parse_translation(json: &str, edition: &str, surahs: &[Surah]) -> Result<Translation, String> {
    let env: Envelope<EditionBody> = serde_json::from_str(json).map_err(unexpected)?;
    if env.code != 200 {
        return Err(unexpected(env.code));
    }
    let body = env.data;
    if body.edition.identifier != edition {
        return Err(unexpected(&body.edition.identifier));
    }
    check_edition(&body, surahs)?;
    let head = &body.edition;
    let name = if head.english_name.trim().is_empty() { head.name.trim().to_string() } else { head.english_name.trim().to_string() };
    let rtl = head.direction.as_deref() == Some("rtl");
    let (language, name) = (head.language.clone(), name);
    let texts = body.surahs.into_iter().flat_map(|s| s.ayahs).map(|a| a.text.trim_start_matches(BOM).trim().to_string()).collect();
    let t = Translation { edition: edition.to_string(), name, language, rtl, texts };
    t.verify()?;
    Ok(t)
}

/// One translation edition the API offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditionInfo {
    pub identifier: String,
    pub language: String,
    /// The name in its own language.
    pub name: String,
    pub english_name: String,
    pub rtl: bool,
}

/// The translation editions (`/v1/edition?format=text&type=translation`).
pub fn parse_editions(json: &str) -> Result<Vec<EditionInfo>, String> {
    let env: Envelope<Vec<EditionHead>> = serde_json::from_str(json).map_err(unexpected)?;
    if env.code != 200 || env.data.is_empty() {
        return Err(unexpected(env.code));
    }
    Ok(env
        .data
        .into_iter()
        .map(|e| EditionInfo { rtl: e.direction.as_deref() == Some("rtl"), identifier: e.identifier, language: e.language, name: e.name.trim().to_string(), english_name: e.english_name.trim().to_string() })
        .collect())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tiny Qur'an in the API's shape: `counts[i]` ayahs in surah `i+1`
    /// (the real index is only checked by the full parser).
    pub(crate) fn sample() -> Quran {
        let counts = [7u16, 286, 200];
        let mut surahs = Vec::new();
        let mut first = 1;
        let names = [("سُورَةُ ٱلْفَاتِحَةِ", "Al-Faatiha", "The Opening"), ("سُورَةُ البَقَرَةِ", "Al-Baqara", "The Cow"), ("سُورَةُ آلِ عِمۡرَانَ", "Aal-i-Imraan", "The Family of Imraan")];
        for (i, n) in counts.iter().enumerate() {
            let (name, tr, meaning) = names[i];
            surahs.push(Surah { number: i as u16 + 1, name: name.into(), transliteration: tr.into(), meaning: meaning.into(), revelation: if i == 0 { Revelation::Makki } else { Revelation::Madani }, ayahs: *n, first });
            first += n;
        }
        let ayahs = (1..first).map(|g| Ayah { text: format!("كَلِمَةٌ {g}"), juz: 1, page: 1, hizb_quarter: 1, sajda: false }).collect();
        Quran { format: FORMAT, edition: TEXT_EDITION.into(), bismillah: "بِسْمِ ٱللَّهِ ٱلرَّحْمَٰنِ ٱلرَّحِيمِ".into(), surahs, ayahs, juz_starts: vec![Ref::new(1, 1)] }
    }

    /// An API answer for `counts` surahs (made-up words, real layout).
    fn edition_json(identifier: &str, counts: &[u16], basmala: bool) -> String {
        let mut g = 0u16;
        let surahs: Vec<String> = counts
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let ayahs: Vec<String> = (1..=*n)
                    .map(|a| {
                        g += 1;
                        let text = if basmala && a == 1 && i == 1 { format!("بِّسْمِ ٱللَّهِ ٱلرَّحْمَٰنِ ٱلرَّحِيمِ كَلِمَةٌ {g}") } else if a == 1 && i == 0 { "\u{FEFF}بِسْمِ ٱللَّهِ ٱلرَّحْمَٰنِ ٱلرَّحِيمِ".to_string() } else { format!("كَلِمَةٌ {g}") };
                        let sajda = if g == 3 { r#"{"id":1,"recommended":true,"obligatory":false}"# } else { "false" };
                        format!(r#"{{"number":{g},"text":"{text}","numberInSurah":{a},"juz":1,"manzil":1,"page":1,"ruku":1,"hizbQuarter":1,"sajda":{sajda}}}"#)
                    })
                    .collect();
                format!(r#"{{"number":{},"name":"x","englishName":"y","englishNameTranslation":"z","revelationType":"Meccan","ayahs":[{}]}}"#, i + 1, ayahs.join(","))
            })
            .collect();
        format!(r#"{{"code":200,"status":"OK","data":{{"surahs":[{}],"edition":{{"identifier":"{identifier}","language":"en","name":"N","englishName":"Saheeh International","format":"text","type":"translation","direction":"ltr"}}}}}}"#, surahs.join(","))
    }

    fn meta_surahs(counts: &[u16]) -> Vec<Surah> {
        let mut first = 1;
        counts
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let s = Surah { number: i as u16 + 1, name: "سورة".into(), transliteration: "T".into(), meaning: "M".into(), revelation: Revelation::Makki, ayahs: *n, first };
                first += n;
                s
            })
            .collect()
    }

    #[test]
    fn references_map_to_global_numbers_and_back() {
        let q = sample();
        assert_eq!(q.global(Ref::new(1, 1)), Some(1));
        assert_eq!(q.global(Ref::new(2, 1)), Some(8));
        assert_eq!(q.global(Ref::new(2, 255)), Some(262));
        assert_eq!(q.global(Ref::new(2, 287)), None);
        assert_eq!(q.global(Ref::new(9, 1)), None);
        for g in [1, 7, 8, 262, 293, 294, 493] {
            assert_eq!(q.at(g).and_then(|r| q.global(r)), Some(g), "{g}");
        }
        assert_eq!(q.at(0), None);
        assert_eq!(q.at(294), Some(Ref::new(3, 1)));
        assert!(q.surah(2).is_some_and(|s| s.has_bismillah()) && !q.surah(1).unwrap().has_bismillah());
        assert_eq!(q.surah(2).unwrap().short_name(), "البَقَرَةِ");
    }

    #[test]
    fn surahs_are_found_by_name_number_or_meaning() {
        let q = sample();
        for name in ["2", "al-baqara", "Al-Baqara", "baqara", "البقرة", "سورة البقرة", "البَقَرَةِ", "the cow", "Cow", "baq"] {
            assert_eq!(q.find_surah(name), Some(2), "{name}");
        }
        assert_eq!(q.find_surah("al-faatiha"), Some(1));
        assert_eq!(q.find_surah("الفاتحة"), Some(1));
        assert_eq!(q.find_surah("aal-i-imraan"), Some(3));
        // Other spellings of the same name.
        assert_eq!(q.find_surah("al-fatiha"), Some(1));
        assert_eq!(q.find_surah("Al Imran"), Some(3));
        assert_eq!((skeleton("yaseen"), skeleton("yasin")), ("ysn".into(), "ysn".into()));
        assert_eq!((distance("nas", "naas"), distance("nas", "nisaa"), distance("", "ab")), (1, 3, 2));
        assert_eq!(q.find_surah("115"), None);
        assert_eq!(q.find_surah("nowhere"), None);
        assert_eq!(q.find_surah("  "), None);
    }

    #[test]
    fn selections_parse() {
        let q = sample();
        assert_eq!(q.select("2"), Ok(Selection::Surah(2)));
        assert_eq!(q.select("al-baqara"), Ok(Selection::Surah(2)));
        assert_eq!(q.select("2:255"), Ok(Selection::Ayah(Ref::new(2, 255))));
        assert_eq!(q.select("baqara:255"), Ok(Selection::Ayah(Ref::new(2, 255))));
        assert_eq!(q.select("البقرة 255"), Ok(Selection::Ayah(Ref::new(2, 255))));
        assert_eq!(q.select("2:255-257"), Ok(Selection::Range(262, 264)));
        assert_eq!(q.select("2:285-3:2"), Ok(Selection::Range(292, 295)));
        assert_eq!(q.select("2:280-"), Ok(Selection::Range(287, 293)));
        assert_eq!(q.globals(Selection::Surah(1)), 1..=7);
        assert_eq!(q.globals(Selection::Ayah(Ref::new(2, 1))), 8..=8);
        for bad in ["2:0", "2:287", "2:255-250", "9", "nothing", "2:5-x", ""] {
            assert!(q.select(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_basmala_is_split_off_first_ayahs() {
        assert_eq!(split_basmala("بِسْمِ ٱللَّهِ ٱلرَّحْمَٰنِ ٱلرَّحِيمِ الٓمٓ"), Some(("بِسْمِ ٱللَّهِ ٱلرَّحْمَٰنِ ٱلرَّحِيمِ", "الٓمٓ")));
        // The variant with shadda first (Tanzil writes it so in two surahs).
        assert!(split_basmala("بِّسْمِ ٱللَّهِ ٱلرَّحْمَٰنِ ٱلرَّحِيمِ وَٱلتِّينِ").is_some());
        assert_eq!(split_basmala("بَرَآءَةٌۭ مِّنَ ٱللَّهِ وَرَسُولِهِۦٓ إِلَى ٱلَّذِينَ"), None);
        assert_eq!(split_basmala("بِسْمِ ٱللَّهِ ٱلرَّحْمَٰنِ ٱلرَّحِيمِ"), None, "the basmala alone is an ayah");
    }

    #[test]
    fn the_api_answers_parse_and_verify() {
        let counts = [7u16, 3];
        let surahs = meta_surahs(&counts);
        let json = edition_json(TEXT_EDITION, &counts, true);
        let env: Envelope<EditionBody> = serde_json::from_str(&json).unwrap();
        check_edition(&env.data, &surahs).unwrap();
        // Building needs the full Qur'an: a short one is refused.
        assert!(build(&json, surahs.clone(), vec![Ref::new(1, 1)]).is_err());
        // A count that disagrees with the index is caught.
        assert!(check_edition(&env.data, &meta_surahs(&[7, 4])).is_err());
        assert!(check_edition(&env.data, &meta_surahs(&[7])).is_err());
        // Wrong edition, wrong code, not JSON.
        assert!(parse_translation(&json, "en.sahih", &surahs).is_err());
        assert!(parse_translation(&json.replace("\"code\":200", "\"code\":404"), TEXT_EDITION, &surahs).is_err());
        assert!(parse_translation("<html>", TEXT_EDITION, &surahs).is_err());
    }

    #[test]
    fn the_whole_text_builds_with_its_basmala_split_off() {
        // The real shape: 114 surahs adding up to 6236 ayahs.
        let mut counts = vec![7u16, 286];
        let rest = AYAHS - 293;
        counts.extend((0..112).map(|i| rest / 112 + u16::from(i < rest % 112)));
        assert_eq!(counts.iter().sum::<u16>(), AYAHS);
        let surahs = meta_surahs(&counts);
        let json = edition_json(TEXT_EDITION, &counts, true);
        let juz: Vec<Ref> = (0..30).map(|i| Ref::new(1 + i, 1)).collect();
        let q = build(&json, surahs.clone(), juz).unwrap();
        assert_eq!(q.ayahs.len(), usize::from(AYAHS));
        assert_eq!(q.bismillah, "بِسْمِ ٱللَّهِ ٱلرَّحْمَٰنِ ٱلرَّحِيمِ", "the opening's first ayah, without the BOM");
        assert_eq!(q.ayah(Ref::new(1, 1)).unwrap().text, q.bismillah);
        assert_eq!(q.ayah(Ref::new(2, 1)).unwrap().text, "كَلِمَةٌ 8", "the basmala is a heading, not part of 2:1");
        assert!(q.ayah_at(3).unwrap().sajda && !q.ayah_at(4).unwrap().sajda);
        q.verify().unwrap();
        // The stored file round-trips.
        let back: Quran = serde_json::from_str(&serde_json::to_string(&q).unwrap()).unwrap();
        assert_eq!(back, q);
        // A meaning of the same shape.
        let tr = parse_translation(&edition_json("en.sahih", &counts, false), "en.sahih", &surahs).unwrap();
        assert_eq!((tr.texts.len(), tr.name.as_str(), tr.rtl), (usize::from(AYAHS), "Saheeh International", false));
        assert!(tr.at(1).is_some_and(|t| !t.starts_with('\u{FEFF}')) && tr.at(0).is_none() && tr.at(AYAHS + 1).is_none());
    }

    #[test]
    fn the_index_parses_and_is_checked() {
        let refs: Vec<String> = (1..=114).map(|n| format!(r#"{{"number":{n},"name":"سُورَةُ س","englishName":"S{n}","englishNameTranslation":"M","numberOfAyahs":{},"revelationType":"{}"}}"#, if n == 114 { AYAHS - 113 * 50 } else { 50 }, if n % 2 == 0 { "Medinan" } else { "Meccan" })).collect();
        let juz: Vec<String> = (1..=30).map(|n| format!(r#"{{"surah":{n},"ayah":1}}"#)).collect();
        let json = format!(r#"{{"code":200,"status":"OK","data":{{"ayahs":{{"count":6236}},"surahs":{{"count":114,"references":[{}]}},"juzs":{{"count":30,"references":[{}]}},"sajdas":{{"count":15,"references":[]}}}}}}"#, refs.join(","), juz.join(","));
        let (surahs, juz) = parse_meta(&json).unwrap();
        assert_eq!((surahs.len(), juz.len()), (114, 30));
        assert_eq!((surahs[1].first, surahs[1].revelation, surahs[0].revelation), (51, Revelation::Madani, Revelation::Makki));
        assert_eq!(surahs[113].first + surahs[113].ayahs - 1, AYAHS);
        assert!(parse_meta(&json.replace(r#""count":6236"#, r#""count":6000"#)).is_err());
        assert!(parse_meta(&json.replace(r#""numberOfAyahs":50,"#, r#""numberOfAyahs":51,"#)).is_err());
        assert!(parse_meta("{}").is_err());
    }

    #[test]
    fn editions_parse() {
        let json = r#"{"code":200,"status":"OK","data":[{"identifier":"en.sahih","language":"en","name":"Saheeh International","englishName":"Saheeh International","format":"text","type":"translation","direction":"ltr"},{"identifier":"ur.jalandhry","language":"ur","name":"جالندہری","englishName":"Fateh Muhammad Jalandhry","format":"text","type":"translation","direction":"rtl"}]}"#;
        let e = parse_editions(json).unwrap();
        assert_eq!(e.len(), 2);
        assert!(!e[0].rtl && e[1].rtl && e[1].language == "ur");
        assert!(parse_editions(r#"{"code":200,"data":[]}"#).is_err());
    }
}
