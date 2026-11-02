//! References as sunnah.com gives them, built from the dataset's numbers,
//! and the links to sunnah.com (patterns checked by opening hadith of
//! every collection on 2026-11-02 and comparing their text):
//!
//! * Bukhari, the four Sunan: the number, a second narration under the
//!   same number lettered (`774.2` → `774b`): `sunnah.com/bukhari:774b`.
//! * Muslim: the dataset's `arabicnumber` (Abdul Baqi's numbering) with
//!   its narration letter (`2699.01` → `2699a`, `52.10` → `52j`); a
//!   narration the dataset left unnumbered takes the next letter after the
//!   one before it (`546b`), as sunnah.com does; the Introduction's
//!   narrations are `i1`, `i2`… and link to its page.
//! * The three forties: `nawawi40:<n>`, `qudsi40:<n>`,
//!   `shahwaliullah40:<n>`.
//! * Muwatta Malik: sunnah.com numbers its hadith differently, so a hadith
//!   keeps the dataset's number and links to its book's page.

use super::collections::{Collection, alias, info};

/// Narration letters (`a` = the first).
const LETTERS: &[u8; 26] = b"abcdefghijklmnopqrstuvwxyz";
/// The Introduction's references start with this.
pub const INTRO: &str = "i";
/// References without a sunnah.com number start with this.
pub const UNNUMBERED: &str = "n";

/// The letter of narration `k` (1 = `a`), when there is one.
fn letter(k: u32) -> Option<char> {
    LETTERS.get(usize::try_from(k).ok()?.checked_sub(1)?).map(|b| char::from(*b))
}

/// A dataset number written as a reference: `774` → `774`, `774.2` →
/// `774b`, `3604.02` → `3604b` (the decimals count the narration).
pub fn from_number(n: f64) -> String {
    from_number_in(n, false)
}

/// [`from_number`] for a number whose narrations run past nine, so the
/// dataset writes them with two decimals (`3604.02` … `3604.1` = the
/// tenth: `3604j`).
pub fn from_number_in(n: f64, two_decimals: bool) -> String {
    let text = n.to_string();
    let (base, frac) = text.split_once('.').unwrap_or((text.as_str(), ""));
    let frac = if two_decimals && frac.len() == 1 { format!("{frac}0") } else { frac.to_string() };
    match frac.parse::<u32>().ok().filter(|k| *k > 1).and_then(letter) {
        Some(l) => format!("{base}{l}"),
        None => base.to_string(),
    }
}

/// The reference of an entry sunnah.com has no number for: the dataset's
/// running number (`n4968`), linked to its book.
pub fn unnumbered(n: f64) -> String {
    format!("{UNNUMBERED}{}", from_number(n))
}

/// Muslim's `arabicnumber` as a reference: `2699.01` → `2699a`, `8` → `8`.
pub fn from_muslim(arabic_number: &str) -> Option<String> {
    let s = arabic_number.trim();
    match s.split_once('.') {
        Some((base, frac)) => {
            let base: u32 = base.parse().ok()?;
            let k: u32 = frac.parse().ok()?;
            Some(format!("{base}{}", letter(k)?))
        }
        None => s.parse::<u32>().ok().map(|n| n.to_string()),
    }
}

/// The reference after `previous` for an unnumbered Muslim narration:
/// `546a` → `546b`, `546` → `546b`.
pub fn next_letter(previous: &str) -> Option<String> {
    let digits: String = previous.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let k = previous[digits.len()..].chars().next().and_then(|c| LETTERS.iter().position(|b| char::from(*b) == c)).map_or(1, |i| i + 1);
    Some(format!("{digits}{}", letter(u32::try_from(k).ok()? + 1)?))
}

/// Where a hadith is read on sunnah.com.
pub fn url(c: Collection, reference: &str, book: u32) -> String {
    let base = "https://sunnah.com";
    match c {
        Collection::Muslim if reference.starts_with(INTRO) => format!("{base}/muslim/introduction"),
        _ if reference.starts_with(UNNUMBERED) => format!("{base}/{}/{book}", c.id()),
        Collection::Malik if book == 0 => format!("{base}/malik"),
        Collection::Malik => format!("{base}/malik/{book}"),
        Collection::Nawawi => format!("{base}/nawawi40:{reference}"),
        Collection::Qudsi => format!("{base}/qudsi40:{reference}"),
        Collection::Dehlawi => format!("{base}/shahwaliullah40:{reference}"),
        _ => format!("{base}/{}:{reference}", c.id()),
    }
}

/// The key a hadith is stored under: `bukhari:774b`.
pub fn key(c: Collection, reference: &str) -> String {
    format!("{}:{reference}", c.id())
}

/// What someone typed: `bukhari 1`, `muslim 2699a`, `Muslim:2699A`,
/// `abu dawud 4031`, `bukhari-774b`, `muslim intro 3`. Returns the
/// collection and the reference (lower-case, no spaces), not checked
/// against the library.
pub fn parse(input: &str) -> Option<(Collection, String)> {
    let s = input.trim().to_lowercase();
    // The collection's name may itself hold spaces (`abu dawud`): try the
    // longest prefix of words that names one.
    let words: Vec<&str> = s.split([' ', ':', '-', '#', '/']).filter(|w| !w.is_empty()).collect();
    for n in (1..words.len()).rev() {
        let Some(c) = alias(&words[..n].join(" ")) else { continue };
        let mut rest: String = words[n..].concat();
        if let Some(num) = rest.strip_prefix("introduction").or_else(|| rest.strip_prefix("intro")) {
            rest = format!("{INTRO}{num}");
        }
        let prefixed = |p: &str| rest.len() > p.len() && rest.starts_with(p) && rest[p.len()..].chars().all(|ch| ch.is_ascii_digit());
        let intro = prefixed(INTRO) || prefixed(UNNUMBERED);
        let numbered = rest.chars().next().is_some_and(|ch| ch.is_ascii_digit()) && rest.chars().all(|ch| ch.is_ascii_alphanumeric());
        let valid = intro || numbered;
        return valid.then_some((c, rest));
    }
    None
}

/// `Sahih al-Bukhari 774b` (`Sahih Muslim, Introduction 3`), in English
/// (the caller translates the name).
pub fn citation(title: &str, reference: &str) -> String {
    match reference.strip_prefix(INTRO) {
        Some(n) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => format!("{title}, Introduction {n}"),
        _ => format!("{title} {reference}"),
    }
}

/// The short reference shown on chips: `Bukhari 774b`.
pub fn short(c: Collection, reference: &str) -> String {
    citation(mandara_i18n::t(info(c).short), reference)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dataset_numbers_become_references() {
        assert_eq!(from_number(1.0), "1");
        assert_eq!(from_number(774.2), "774b");
        assert_eq!(from_number(1390.3), "1390c");
        assert_eq!(from_number(3604.02), "3604b");
        assert_eq!(from_number(3604.09), "3604i");
        assert_eq!(from_number_in(3604.1, true), "3604j");
        assert_eq!(from_number_in(3604.0, true), "3604");
        assert_eq!(unnumbered(4968.0), "n4968");
        assert_eq!(from_muslim("2699.01").as_deref(), Some("2699a"));
        assert_eq!(from_muslim("2699.02").as_deref(), Some("2699b"));
        assert_eq!(from_muslim("52.10").as_deref(), Some("52j"));
        assert_eq!(from_muslim("910").as_deref(), Some("910"));
        assert_eq!(from_muslim("x"), None);
        assert_eq!(next_letter("546a").as_deref(), Some("546b"));
        assert_eq!(next_letter("1479d").as_deref(), Some("1479e"));
        assert_eq!(next_letter("884").as_deref(), Some("884b"));
        assert_eq!(next_letter("i3"), None);
    }

    #[test]
    fn links_follow_sunnah_com() {
        assert_eq!(url(Collection::Bukhari, "774b", 10), "https://sunnah.com/bukhari:774b");
        assert_eq!(url(Collection::Muslim, "2699a", 48), "https://sunnah.com/muslim:2699a");
        assert_eq!(url(Collection::Muslim, "i3", 0), "https://sunnah.com/muslim/introduction");
        assert_eq!(url(Collection::Abudawud, "4031", 34), "https://sunnah.com/abudawud:4031");
        assert_eq!(url(Collection::Ibnmajah, "3930b", 36), "https://sunnah.com/ibnmajah:3930b");
        assert_eq!(url(Collection::Nawawi, "1", 1), "https://sunnah.com/nawawi40:1");
        assert_eq!(url(Collection::Qudsi, "40", 1), "https://sunnah.com/qudsi40:40");
        assert_eq!(url(Collection::Dehlawi, "14", 1), "https://sunnah.com/shahwaliullah40:14");
        assert_eq!(url(Collection::Malik, "1000", 21), "https://sunnah.com/malik/21");
        assert_eq!(url(Collection::Muslim, "n4968", 33), "https://sunnah.com/muslim/33");
        assert_eq!(key(Collection::Bukhari, "1"), "bukhari:1");
    }

    #[test]
    fn typed_references_are_read() {
        assert_eq!(parse("bukhari 1"), Some((Collection::Bukhari, "1".into())));
        assert_eq!(parse("Muslim:2699A"), Some((Collection::Muslim, "2699a".into())));
        assert_eq!(parse("abudawud 4031"), Some((Collection::Abudawud, "4031".into())));
        assert_eq!(parse("abu dawud 4031"), Some((Collection::Abudawud, "4031".into())));
        assert_eq!(parse("bukhari-774b"), Some((Collection::Bukhari, "774b".into())));
        assert_eq!(parse("muslim intro 3"), Some((Collection::Muslim, "i3".into())));
        assert_eq!(parse("muslim n4968"), Some((Collection::Muslim, "n4968".into())));
        assert_eq!(parse("ibn majah 3930 b"), Some((Collection::Ibnmajah, "3930b".into())));
        assert_eq!(parse("bukhari"), None);
        assert_eq!(parse("nowhere 1"), None);
        assert_eq!(parse("bukhari one"), None);
        assert_eq!(citation("Sahih Muslim", "i3"), "Sahih Muslim, Introduction 3");
        assert_eq!(citation("Sahih al-Bukhari", "774b"), "Sahih al-Bukhari 774b");
    }
}
