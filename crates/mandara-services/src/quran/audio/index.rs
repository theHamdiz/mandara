//! The small part of the loaded text needed to address audio files.

use super::super::text::{Quran, Ref};

/// Each surah's first global ayah number and ayah count: all the audio
/// fetcher keeps of the loaded text.
#[derive(Clone, Debug)]
pub struct AyahIndex {
    chapters: Box<[(u16, u16)]>,
}

impl AyahIndex {
    /// Copies the surah boundaries out of the loaded text.
    pub fn new(quran: &Quran) -> Self {
        Self { chapters: quran.surahs.iter().map(|s| (s.first, s.ayahs)).collect() }
    }

    /// The surah/ayah of global ayah `global` and that surah's ayah count
    /// (`None` outside the text).
    pub fn locate(&self, global: u16) -> Option<(Ref, u16)> {
        let i = self.chapters.partition_point(|s| s.0 <= global).checked_sub(1)?;
        let (first, count) = self.chapters[i];
        let ayah = global.checked_sub(first)?.checked_add(1)?;
        (ayah <= count).then(|| (Ref::new(i as u16 + 1, ayah), count))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_index_keeps_chapter_boundaries_and_rejects_invalid_numbers() {
        let index = AyahIndex { chapters: Box::new([(1, 7), (8, 286), (294, 200)]) };
        for (global, r, count) in [(1, Ref::new(1, 1), 7), (7, Ref::new(1, 7), 7), (8, Ref::new(2, 1), 286), (262, Ref::new(2, 255), 286), (493, Ref::new(3, 200), 200)] {
            assert_eq!(index.locate(global), Some((r, count)));
        }
        assert!(index.locate(0).is_none());
        assert!(index.locate(494).is_none());
        assert!(AyahIndex { chapters: Box::new([]) }.locate(1).is_none());
    }
}
