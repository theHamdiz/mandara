//! Search over the whole Qur'an: the Arabic without its diacritics (the
//! same folding as Jannah's search: tashkeel, Qur'anic marks, tatweel and
//! alifs dropped, letter variants folded, so a plainly typed word finds
//! the Uthmani spelling) and the downloaded meaning, case-insensitively.
//! Every word of the query must match, in the Arabic or the meaning. A
//! reference (`2:255`) finds that ayah. Built and searched off the UI
//! thread (folding the whole text takes a few milliseconds).

use crate::jannah::search::fold;

use super::text::{Quran, Translation};

/// Every ayah, folded (index = global number − 1).
#[derive(Debug, Default)]
pub struct Index {
    arabic: Vec<String>,
    meaning: Vec<String>,
}

impl Index {
    pub fn build(q: &Quran, meaning: Option<&Translation>) -> Self {
        let arabic = q.ayahs.iter().map(|a| fold(&a.text)).collect();
        let meaning = match meaning {
            Some(m) => m.texts.iter().map(|t| fold(t)).collect(),
            None => Vec::new(),
        };
        Self { arabic, meaning }
    }

    pub fn len(&self) -> usize {
        self.arabic.len()
    }

    pub fn is_empty(&self) -> bool {
        self.arabic.is_empty()
    }
}

/// What a search found: the first `limit` ayahs (global numbers, in
/// order) and how many matched in all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hits {
    pub ayahs: Vec<u16>,
    pub total: usize,
}

/// The ayahs every word of `query` matches.
pub fn search(index: &Index, q: &Quran, query: &str, limit: usize) -> Hits {
    let query = query.trim();
    if query.is_empty() {
        return Hits::default();
    }
    // A reference goes straight to its ayah.
    if query.contains(':')
        && let Ok(super::text::Selection::Ayah(r)) = q.select(query)
        && let Some(g) = q.global(r)
    {
        return Hits { ayahs: vec![g], total: 1 };
    }
    let folded = fold(query);
    let words: Vec<&str> = folded.split(' ').filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return Hits::default();
    }
    let mut hits = Hits::default();
    for (i, arabic) in index.arabic.iter().enumerate() {
        let meaning = index.meaning.get(i).map(String::as_str).unwrap_or("");
        if words.iter().all(|w| arabic.contains(w) || meaning.contains(w)) {
            hits.total += 1;
            if hits.ayahs.len() < limit {
                hits.ayahs.push(i as u16 + 1);
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::super::text::tests::sample;
    use super::*;

    #[test]
    fn plain_arabic_finds_the_uthmani_spelling() {
        let mut q = sample();
        q.ayahs[261].text = "ٱللَّهُ لَآ إِلَٰهَ إِلَّا هُوَ ٱلْحَىُّ ٱلْقَيُّومُ".into();
        q.ayahs[30].text = "جَنَّٰتٍۢ تَجْرِى مِن تَحْتِهَا ٱلْأَنْهَٰرُ".into();
        let idx = Index::build(&q, None);
        assert_eq!(idx.len(), q.ayahs.len());
        assert_eq!(search(&idx, &q, "الحي القيوم", 50).ayahs, vec![262]);
        assert_eq!(search(&idx, &q, "لا اله الا هو", 50).ayahs, vec![262]);
        assert_eq!(search(&idx, &q, "جنات تجري", 50).ayahs, vec![31]);
        assert!(search(&idx, &q, "القيوم جنات", 50).ayahs.is_empty(), "every word must match one ayah");
        assert_eq!(search(&idx, &q, "   ", 50), Hits::default());
    }

    #[test]
    fn meanings_and_references_are_searched() {
        let q = sample();
        let mut texts: Vec<String> = (1..=q.ayahs.len()).map(|g| format!("meaning {g}")).collect();
        texts[261] = "Allah - there is no deity except Him, the Ever-Living, the Sustainer".into();
        let t = Translation { edition: "en.sahih".into(), name: "S".into(), language: "en".into(), rtl: false, texts };
        let idx = Index::build(&q, Some(&t));
        assert_eq!(search(&idx, &q, "ever-living SUSTAINER", 50).ayahs, vec![262]);
        assert_eq!(search(&idx, &q, "2:255", 50).ayahs, vec![262]);
        // Everything matches "meaning": the first ten, and the true total.
        let all = search(&idx, &q, "meaning", 10);
        assert_eq!((all.ayahs.len(), all.total), (10, q.ayahs.len() - 1));
        assert_eq!(all.ayahs[0], 1);
    }
}
