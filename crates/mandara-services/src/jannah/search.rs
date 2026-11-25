//! Search over the reminders: Arabic without its diacritics (tashkeel,
//! Qur'anic marks, tatweel) and with its letter variants folded (ى → ي,
//! ة → ه, ؤ → و, ئ → ي), and the meaning in the reader's language,
//! case-insensitively. Every alif (written, hamzated, wasla or the
//! Uthmani dagger alif) and lone hamza is dropped, so the Uthmani جَنَّٰتٍ
//! and a typed جنات meet, as do هَٰذَا and هذا. Every word of the query must
//! match. The index is built and searched off the UI thread.

use super::catalog::{Catalog, ItemId};

/// Arabic marks dropped before comparing: tashkeel and Qur'anic
/// annotation marks, the superscript (dagger) alif, tatweel.
fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0610..=0x061A | 0x064B..=0x065F | 0x06D6..=0x06ED | 0x0640 | 0x08D3..=0x08FF)
}

/// Alifs (written or not) and the lone hamza: spelled differently in the
/// Uthmani text and in what people type, so left out of both.
fn is_alif(c: char) -> bool {
    matches!(c, 'ا' | 'أ' | 'إ' | 'آ' | 'ٱ' | 'ٲ' | 'ٳ' | '\u{0670}' | 'ء')
}

/// One letter folded to its plain form.
fn fold_letter(c: char) -> char {
    match c {
        'ى' | 'ئ' | 'ی' => 'ي',
        'ة' => 'ه',
        'ؤ' => 'و',
        'ک' => 'ك',
        _ => c,
    }
}

/// `text` folded for comparison: Arabic marks dropped and letters folded,
/// everything lower-cased, runs of spaces made one.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = true;
    for c in text.chars() {
        if is_mark(c) || is_alif(c) || c == '\u{200F}' || c == '\u{200E}' {
            continue;
        }
        if c.is_whitespace() || matches!(c, '،' | '؛' | '"' | '“' | '”' | '«' | '»') {
            if !space {
                out.push(' ');
                space = true;
            }
            continue;
        }
        space = false;
        out.extend(fold_letter(c).to_lowercase());
    }
    out.trim_end().to_string()
}

/// One searchable reminder.
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: ItemId,
    /// The folded Arabic (the whole text) and meaning.
    pub arabic: String,
    pub meaning: String,
    /// The folded reference (`al-baqara 2:25`, `sahih al-bukhari 3245`).
    pub reference: String,
}

/// The index over `items` (`meaning` gives each one's meaning in the
/// reader's language, `reference` its citation).
pub fn index(cat: &Catalog, items: &[ItemId], meaning: impl Fn(ItemId) -> Option<String>, reference: impl Fn(ItemId) -> String) -> Vec<Entry> {
    items
        .iter()
        .filter_map(|id| {
            let item = cat.get(*id)?;
            Some(Entry { id: *id, arabic: fold(item.full_arabic()), meaning: meaning(*id).map(|m| fold(&m)).unwrap_or_default(), reference: fold(&reference(*id)) })
        })
        .collect()
}

/// The entries every word of `query` matches (in index order).
pub fn search(index: &[Entry], query: &str) -> Vec<ItemId> {
    let q = fold(query);
    let words: Vec<&str> = q.split(' ').filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return index.iter().map(|e| e.id).collect();
    }
    index.iter().filter(|e| words.iter().all(|w| e.arabic.contains(w) || e.meaning.contains(w) || e.reference.contains(w))).map(|e| e.id).collect()
}

#[cfg(test)]
mod tests {
    use super::super::catalog::catalog;
    use super::*;

    #[test]
    fn arabic_folds_without_diacritics() {
        // Uthmani spelling with the dagger alif and small marks.
        assert_eq!(fold("جَنَّٰتٍۢ تَجْرِى"), fold("جنات تجري"));
        assert_eq!(fold("جنات تجري"), "جنت تجري");
        assert_eq!(fold("ٱلْجَنَّةَ"), fold("الجنة"));
        assert_eq!(fold("أَحْسَنُوا۟ ٱلْحُسْنَىٰ"), fold("احسنوا الحسنى"));
        assert_eq!(fold("هَٰذَا"), fold("هذا"));
        assert_eq!(fold("  Paradise,\u{200F}  Gardens "), "paradise, gardens");
        assert_eq!(fold("مُؤْمِنٌ"), "مومن");
    }

    #[test]
    fn search_matches_arabic_and_meanings() {
        let c = catalog();
        let all: Vec<ItemId> = c.items.iter().map(|i| i.id).collect();
        let six = c.find("bukhari-6094").unwrap();
        let idx = index(c, &all, |id| (id == six).then(|| "Truthfulness leads to righteousness".to_string()), |id| c.get(id).map(|i| i.key.clone()).unwrap_or_default());
        // Typed plainly, without diacritics.
        let hits = search(&idx, "الجنة");
        assert!(hits.len() >= 3, "{}", hits.len());
        let v = c.find("2:25").unwrap();
        assert!(search(&idx, "جنات تجري").contains(&v));
        assert_eq!(search(&idx, "TRUTHFULNESS righteousness"), vec![six]);
        assert!(search(&idx, "truthfulness nowhere").is_empty());
        assert_eq!(search(&idx, "bukhari-6094"), vec![six]);
        assert_eq!(search(&idx, "  ").len(), all.len());
    }
}
