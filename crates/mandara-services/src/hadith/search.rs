//! Search over the downloaded collections: the Arabic without its
//! diacritics and letter variants (Jannah's folding,
//! [`crate::jannah::search::fold`]: a plainly typed الجنة finds
//! الْجَنَّةَ), the meaning case-insensitively, and the reference. Every
//! word of the query must match. Filters narrow by collection and by the
//! lead grade's family. Indexes are built per collection, off the UI
//! thread, and kept with the loaded collection.

use super::collections::Collection;
use super::data::Library;
use super::grade::GradeClass;
use crate::jannah::search::fold;

/// One searchable hadith.
#[derive(Clone, Debug)]
pub struct Entry {
    pub index: u32,
    /// Folded Arabic (the whole text), meaning and reference.
    pub arabic: String,
    pub meaning: String,
    pub reference: String,
    /// The lead grade's family (sahih for Bukhari and Muslim).
    pub class: Option<GradeClass>,
}

/// The index of a library.
pub fn index(lib: &Library) -> Vec<Entry> {
    let short = super::collections::info(lib.collection).short;
    lib.hadith
        .iter()
        .enumerate()
        .map(|(i, h)| Entry {
            index: i as u32,
            arabic: fold(&h.arabic),
            meaning: h.meaning.as_deref().map(fold).unwrap_or_default(),
            reference: fold(&format!("{short} {} {}", lib.collection.id(), h.reference)),
            class: h.class(lib.collection),
        })
        .collect()
}

/// What to look for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    /// Only these collections (`None`: all searched).
    pub collections: Option<Vec<Collection>>,
    pub grade: Option<GradeClass>,
}

impl Query {
    /// Reads `words [--collection c] [--grade g]` (also `-c`, `-g`,
    /// `--collection=c`); unknown values are errors.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut q = Query::default();
        let mut words = Vec::new();
        let mut it = args.iter().flat_map(|a| match a.split_once('=') {
            Some((k, v)) if k.starts_with("--") => vec![k.to_string(), v.to_string()],
            _ => vec![a.clone()],
        });
        while let Some(a) = it.next() {
            match a.as_str() {
                "--collection" | "-c" => {
                    let v = it.next().ok_or_else(|| mandara_i18n::t("--collection needs a collection").to_string())?;
                    let c = super::collections::alias(&v).ok_or_else(|| mandara_i18n::tf("No collection {name}", &[("name", &v)]))?;
                    q.collections.get_or_insert_with(Vec::new).push(c);
                }
                "--grade" | "-g" => {
                    let v = it.next().ok_or_else(|| mandara_i18n::t("--grade needs sahih, hasan or daif").to_string())?;
                    q.grade = Some(GradeClass::parse(&v).ok_or_else(|| mandara_i18n::tf("Unknown grade {grade}: use sahih, hasan or daif", &[("grade", &v)]))?);
                }
                _ => words.push(a),
            }
        }
        q.text = words.join(" ");
        Ok(q)
    }

    /// Whether `c` is searched.
    pub fn wants(&self, c: Collection) -> bool {
        self.collections.as_ref().is_none_or(|l| l.contains(&c))
    }
}

/// The entries of one collection's index every word of `query` matches,
/// in order, at most `max`.
pub fn search(index: &[Entry], query: &Query, max: usize) -> Vec<u32> {
    let q = fold(&query.text);
    let words: Vec<&str> = q.split(' ').filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return Vec::new();
    }
    index
        .iter()
        .filter(|e| query.grade.is_none_or(|g| e.class == Some(g)))
        .filter(|e| words.iter().all(|w| e.arabic.contains(w) || e.meaning.contains(w) || e.reference.contains(w)))
        .map(|e| e.index)
        .take(max)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::data::tests::sample;
    use super::super::data::{Library, MeaningInfo, parse};
    use super::*;

    fn lib() -> Library {
        let c = Collection::Nawawi;
        Library::build(c, parse(&sample(c, 42, "ara")).unwrap(), Some((parse(&sample(c, 42, "eng")).unwrap(), MeaningInfo { lang: "eng".into(), translator: None, fallback: false }))).unwrap()
    }

    #[test]
    fn arabic_meaning_and_reference_match() {
        let l = lib();
        let idx = index(&l);
        // Typed without diacritics.
        assert_eq!(search(&idx, &Query { text: "كلام طيب".into(), ..Query::default() }, 100).len(), 42);
        assert!(search(&idx, &Query { text: "رقم 7".into(), ..Query::default() }, 100).contains(&7));
        assert_eq!(search(&idx, &Query { text: "SOMEONE 12:".into(), ..Query::default() }, 100), vec![12]);
        assert!(search(&idx, &Query { text: "nawawi 40".into(), ..Query::default() }, 100).contains(&39));
        assert!(search(&idx, &Query { text: "zzz".into(), ..Query::default() }, 100).is_empty());
        assert_eq!(search(&idx, &Query { text: "كلام".into(), ..Query::default() }, 5).len(), 5);
        assert!(search(&idx, &Query { text: "  ".into(), ..Query::default() }, 5).is_empty());
        // Every sample is graded hasan by Al-Albani.
        assert!(search(&idx, &Query { text: "كلام".into(), grade: Some(GradeClass::Sahih), ..Query::default() }, 5).is_empty());
        assert_eq!(search(&idx, &Query { text: "كلام".into(), grade: Some(GradeClass::Hasan), ..Query::default() }, 5).len(), 5);
    }

    #[test]
    fn queries_read_their_filters() {
        let args = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        let q = Query::parse(&args("الصلاة --collection bukhari -c abu-dawud --grade sahih")).unwrap();
        assert_eq!(q.text, "الصلاة");
        assert_eq!(q.collections, Some(vec![Collection::Bukhari, Collection::Abudawud]));
        assert_eq!(q.grade, Some(GradeClass::Sahih));
        assert!(q.wants(Collection::Bukhari) && !q.wants(Collection::Muslim));
        let q = Query::parse(&args("mercy --grade=hasan")).unwrap();
        assert_eq!((q.text.as_str(), q.grade), ("mercy", Some(GradeClass::Hasan)));
        assert!(Query::parse(&args("x --grade great")).is_err());
        assert!(Query::parse(&args("x --collection")).is_err());
        assert!(Query::parse(&args("x --collection nowhere")).is_err());
    }
}
