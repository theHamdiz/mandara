//! The hadith of the day (or the hour): one hadith per period, the same on
//! every surface, from the collections chosen for it (Bukhari and Muslim
//! by default, so it is always sahih). The pool is every hadith whose
//! words are a quoted saying of a readable length; a shuffled order walks
//! the whole pool before anything repeats (Jannah's rotation:
//! [`crate::jannah::rotation`]). Pure: the date and the pool decide.

use super::collections::Collection;
use super::data::Library;

pub use crate::jannah::rotation::{Rotate, next_boundary, period};

/// The order's seed.
const SEED: u64 = 0x6861_6469_7468_0001;
/// The words of a daily hadith run between these many characters: long
/// enough to stand alone, short enough for a card.
pub const MIN_WORDS: usize = 40;
pub const MAX_WORDS: usize = 600;
/// The dataset's quotation mark: a quoted saying.
const QUOTE: char = '"';

/// The hadith of `lib` that may be the hadith of the day.
pub fn eligible(lib: &Library) -> Vec<u32> {
    lib.hadith
        .iter()
        .enumerate()
        .filter(|(_, h)| h.arabic.contains(QUOTE) && (MIN_WORDS..=MAX_WORDS).contains(&h.words().chars().count()))
        .map(|(i, _)| i as u32)
        .collect()
}

/// The pool over several collections, in the library's order.
pub fn pool(libs: &[(Collection, &Library)]) -> Vec<(Collection, u32)> {
    let mut libs: Vec<&(Collection, &Library)> = libs.iter().collect();
    libs.sort_by_key(|(c, _)| *c);
    libs.into_iter().flat_map(|(c, l)| eligible(l).into_iter().map(move |i| (*c, i))).collect()
}

/// The place in a pool of `n` for `slot` (a period).
pub fn pick(n: usize, slot: i64) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let len = n as i64;
    let order = crate::jannah::rotation::order(n, SEED, slot.div_euclid(len));
    order.get(slot.rem_euclid(len) as usize).copied()
}

/// What a cached "today" was picked from: the collections and the
/// meaning's language (a change picks again).
pub fn pool_key(collections: &[Collection], lang: Option<&str>) -> String {
    let mut ids: Vec<&str> = collections.iter().map(|c| c.id()).collect();
    ids.sort_unstable();
    format!("{}|{}", ids.join(","), lang.unwrap_or("ara"))
}

#[cfg(test)]
mod tests {
    use super::super::data::tests::sample;
    use super::super::data::{Library, parse};
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn a_round_shows_every_hadith_once() {
        let n = 37;
        let start = 739_000 * n as i64;
        let seen: HashSet<usize> = (start..start + n as i64).filter_map(|s| pick(n, s)).collect();
        assert_eq!(seen.len(), n);
        assert_eq!(pick(n, 5), pick(n, 5));
        assert_eq!(pick(0, 5), None);
        assert!(pick(n, -3).is_some());
    }

    #[test]
    fn the_pool_keeps_quoted_sayings_of_a_readable_length() {
        let c = Collection::Nawawi;
        let lib = Library::build(c, parse(&sample(c, 42, "ara")).unwrap(), None).unwrap();
        // The samples' words are short: none qualifies.
        assert!(eligible(&lib).is_empty());
        assert!(pool(&[(c, &lib)]).is_empty());
        // Two long enough quoted sayings, one unquoted, one far too long.
        let mut raw = parse(&sample(c, 42, "ara")).unwrap();
        let words = "كَلِمَةٌ طَيِّبَةٌ ".repeat(4);
        raw.hadiths[3].text = format!("حَدَّثَنَا فُلاَنٌ، قَالَ \" {words} \"");
        raw.hadiths[9].text = format!("حَدَّثَنَا فُلاَنٌ، قَالَ \" {words} \"");
        raw.hadiths[10].text = format!("{words}{words}");
        raw.hadiths[11].text = format!("قَالَ \" {} \"", words.repeat(20));
        let lib = Library::build(c, raw, None).unwrap();
        assert_eq!(eligible(&lib), vec![3, 9]);
        assert_eq!(pool(&[(c, &lib)]), vec![(c, 3), (c, 9)]);
        assert_eq!(pool_key(&[Collection::Muslim, Collection::Bukhari], Some("eng")), "bukhari,muslim|eng");
        assert_eq!(pool_key(&[Collection::Bukhari], None), "bukhari|ara");
    }
}
