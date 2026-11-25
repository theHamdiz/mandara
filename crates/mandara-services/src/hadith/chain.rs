//! Where a hadith's chain of narrators (isnad) ends and its words (matn)
//! begin, so the reader can set the words first and the chain quietly.
//! Nothing is changed or removed: the text is only split into three
//! verbatim slices (before, the words, after).
//!
//! * With the dataset's quotation marks, the words run from the first mark
//!   to the last one before any second chain (Jannah's rule,
//!   [`crate::jannah::catalog::matn`]); a saying whose closing mark is
//!   missing runs to the end (or to a second chain).
//! * Without them, the chain is the run of short transmission clauses at
//!   the start (`حدثنا فلان، عن فلان، …`), each opening with a
//!   transmission word; the first longer clause starts the words.
//! * Anything shorter than a few words is not split.

use crate::jannah::catalog::matn as quoted_matn;
use crate::jannah::search::fold;

/// The quotation mark the dataset puts around the words.
const QUOTE: char = '"';
/// Words that open a chain clause (compared folded: no diacritics or
/// alifs).
const TRANSMISSION: [&str; 16] = [
    "حدثنا", "حدثني", "وحدثنا", "وحدثني", "أخبرنا", "أخبرني", "وأخبرنا", "وأخبرني", "أنبأنا", "وأنبأنا", "عن", "وعن", "سمعت", "ح", "قالا", "قالوا",
];
/// The dash around a parenthetical in a chain (`- يعني ابن فلان -`).
const ASIDE: &str = "-";
/// "He said" before a transmission word (`قال حدثنا`).
const SAID: [&str; 3] = ["قال", "قالا", "قالوا"];
/// A chain clause has at most this many words.
const MAX_CLAUSE_WORDS: usize = 9;
/// Clause separators in the dataset's Arabic.
const SEPARATORS: [char; 2] = ['،', ','];
/// Shorter words are not set apart.
const MIN_WORDS_CHARS: usize = 12;
/// Trimmed off the words: spaces, direction marks, punctuation.
const TRIM: [char; 7] = [' ', '\u{200F}', '\u{200E}', '.', '،', '\u{2009}', ':'];

/// Byte range of the words in `arabic` (the whole text when it has no
/// separable chain).
pub fn split(arabic: &str) -> (usize, usize) {
    let whole = whole(arabic);
    let range = if arabic.contains(QUOTE) { quoted(arabic) } else { clauses(arabic) };
    match range {
        Some((s, e)) if arabic[s..e].chars().count() >= MIN_WORDS_CHARS => (s, e),
        _ => whole,
    }
}

/// The text without its surrounding spaces, as a range.
fn whole(arabic: &str) -> (usize, usize) {
    let start = arabic.len() - arabic.trim_start().len();
    (start, start + arabic.trim().len())
}

/// `inner`'s range inside `outer` (it is a slice of it).
fn offset(outer: &str, inner: &str) -> (usize, usize) {
    let start = inner.as_ptr() as usize - outer.as_ptr() as usize;
    (start, start + inner.len())
}

fn quoted(arabic: &str) -> Option<(usize, usize)> {
    let words = quoted_matn(arabic);
    let (s, e) = offset(arabic, words);
    if (s, e) != whole(arabic) {
        return Some((s, e));
    }
    // No closing mark: from the opening mark to the end, or to a second
    // chain after it.
    let start = arabic.find(QUOTE)? + QUOTE.len_utf8();
    let mut at = start;
    let mut end = arabic.len();
    for word in arabic[start..].split_inclusive(char::is_whitespace) {
        let folded = fold(word.trim_matches(|c: char| TRIM.contains(&c)));
        if at > start && (folded == fold("وحدثنا") || folded == fold("وحدثني") || folded == fold("حدثنا") || folded == fold("وأخبرنا")) {
            end = at;
            break;
        }
        at += word.len();
    }
    let slice = arabic[start..end].trim_matches(|c: char| TRIM.contains(&c) || c == QUOTE);
    (!slice.is_empty()).then(|| offset(arabic, slice))
}

fn clauses(arabic: &str) -> Option<(usize, usize)> {
    let openers: Vec<String> = TRANSMISSION.iter().map(|w| fold(w)).collect();
    let mut chain_end = None;
    let mut at = 0;
    for clause in arabic.split_inclusive(SEPARATORS) {
        // A parenthetical `- يعني ابن فلان -` before a clause is part of
        // the chain; so is a `قال` before a transmission word.
        let mut words: Vec<&str> = clause.split_whitespace().collect();
        if words.first() == Some(&ASIDE)
            && let Some(close) = words.iter().skip(1).position(|w| *w == ASIDE)
        {
            words.drain(..close + 2);
        }
        if words.first().is_some_and(|w| SAID.iter().any(|s| fold(w) == fold(s))) && words.get(1).is_some_and(|w| openers.contains(&fold(w))) {
            words.remove(0);
        }
        let opens = words.first().is_some_and(|w| openers.contains(&fold(w)));
        // The last clause (no separator after it) is never all chain.
        let closed = clause.ends_with(SEPARATORS);
        if !opens || words.len() > MAX_CLAUSE_WORDS || !closed {
            break;
        }
        at += clause.len();
        chain_end = Some(at);
    }
    let start = chain_end?;
    let rest = arabic[start..].trim_matches(|c: char| TRIM.contains(&c));
    (!rest.is_empty()).then(|| offset(arabic, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Made-up words in the dataset's layouts (no hadith text typed here).
    #[test]
    fn quoted_words_are_set_apart() {
        let text = "حَدَّثَنَا فُلاَنٌ، عَنْ فُلاَنٍ قَالَ ‏ \"‏ كَلاَمٌ طَيِّبٌ جَمِيلٌ ‏\"‏‏.‏";
        let (s, e) = split(text);
        assert_eq!(&text[s..e], "كَلاَمٌ طَيِّبٌ جَمِيلٌ");
        // An opening mark without its closing one runs to the end.
        let open = "حَدَّثَنَا فُلاَنٌ، قَالَ : \" كَلاَمٌ طَيِّبٌ جَمِيلٌ وَحَسَنٌ";
        let (s, e) = split(open);
        assert_eq!(&open[s..e], "كَلاَمٌ طَيِّبٌ جَمِيلٌ وَحَسَنٌ");
        // … or to a second chain.
        let two = "حَدَّثَنَا فُلاَنٌ، قَالَ \" كَلاَمٌ طَيِّبٌ جَمِيلٌ وَحَسَنٌ وَحَدَّثَنِي فُلاَنٌ بِمِثْلِهِ";
        let (s, e) = split(two);
        assert_eq!(&two[s..e], "كَلاَمٌ طَيِّبٌ جَمِيلٌ وَحَسَنٌ");
    }

    #[test]
    fn unquoted_chains_end_at_the_first_long_clause() {
        let text = "حَدَّثَنَا فُلاَنٌ، حَدَّثَنَا فُلاَنٌ، عَنْ فُلاَنِ بْنِ فُلاَنٍ، أَنَّهُ قَالَ جَاءَ رَجُلٌ إِلَى الْمَسْجِدِ فَسَأَلَ عَنْ أَمْرٍ";
        let (s, e) = split(text);
        assert_eq!(&text[s..e], "أَنَّهُ قَالَ جَاءَ رَجُلٌ إِلَى الْمَسْجِدِ فَسَأَلَ عَنْ أَمْرٍ");
        // Parentheticals and "he said" stay in the chain.
        let aside = "حَدَّثَنَا فُلاَنٌ، - يَعْنِي ابْنَ فُلاَنٍ - عَنْ فُلاَنٍ، قَالَ حَدَّثَنَا فُلاَنٌ، أَنَّ رَجُلاً كَانَ إِذَا ذَهَبَ إِلَى السُّوقِ أَبْعَدَ";
        let (s, e) = split(aside);
        assert_eq!(&aside[s..e], "أَنَّ رَجُلاً كَانَ إِذَا ذَهَبَ إِلَى السُّوقِ أَبْعَدَ");
        // No chain at the start: the whole text.
        let plain = " الْكَلاَمُ الطَّيِّبُ صَدَقَةٌ وَالْبِرُّ ";
        let (s, e) = split(plain);
        assert_eq!(&plain[s..e], plain.trim());
        // Too little left after the chain: not split.
        let short = "حَدَّثَنَا فُلاَنٌ، بِمِثْلِهِ";
        assert_eq!(split(short), whole(short));
        assert_eq!(split(""), (0, 0));
    }
}
