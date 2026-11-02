//! Grades as the dataset gives them (`Sahih`, `Hasan Sahih`, `Isnaad
//! Hasan`, `Daif`, `Mawdu`…, each with its grader), sorted into three
//! families for the badges' colours and the search filter. The words and
//! the grader's name are always shown as given.

use serde::{Deserialize, Serialize};

/// A grade's family.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GradeClass {
    Sahih,
    Hasan,
    /// Weak, and the words for worse (munkar, shadh, mawdu').
    Daif,
    /// Anything else (`Maqtu`, `-`): shown neutral.
    Other,
}

impl GradeClass {
    pub fn id(self) -> &'static str {
        match self {
            GradeClass::Sahih => "sahih",
            GradeClass::Hasan => "hasan",
            GradeClass::Daif => "daif",
            GradeClass::Other => "other",
        }
    }

    /// `sahih`, `hasan`, `daif` (also `da'if`, `weak`), ignoring case.
    pub fn parse(s: &str) -> Option<Self> {
        let k: String = s.chars().filter(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_lowercase();
        Some(match k.as_str() {
            "sahih" | "saheeh" | "authentic" => GradeClass::Sahih,
            "hasan" | "good" => GradeClass::Hasan,
            "daif" | "dhaif" | "weak" => GradeClass::Daif,
            _ => return None,
        })
    }
}

/// Words that put a grade in the weak family (lower-case).
const WEAK: [&str; 10] = ["daif", "da'if", "da`if", "dhaif", "weak", "munkar", "mawdu", "maudu", "shadh", "batil"];

/// The family of a grade's words.
pub fn classify(grade: &str) -> GradeClass {
    let g = grade.to_lowercase();
    if WEAK.iter().any(|w| g.contains(w)) {
        GradeClass::Daif
    } else if g.contains("sahih") {
        GradeClass::Sahih
    } else if g.contains("hasan") {
        GradeClass::Hasan
    } else {
        GradeClass::Other
    }
}

/// One grader's grade.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grade {
    pub grader: String,
    pub grade: String,
    pub class: GradeClass,
}

impl Grade {
    pub fn new(grader: &str, grade: &str) -> Self {
        Self { grader: grader.trim().to_string(), grade: grade.trim().to_string(), class: classify(grade) }
    }
}

/// The grader whose grade leads (and filters): the one most cited across
/// the four Sunan.
const LEAD_GRADER: &str = "Al-Albani";

/// The grade a hadith is badged and filtered by: Al-Albani's when given,
/// else the first grader's.
pub fn lead(grades: &[Grade]) -> Option<&Grade> {
    grades.iter().find(|g| g.grader.eq_ignore_ascii_case(LEAD_GRADER)).or_else(|| grades.first())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grades_sort_into_families() {
        for (g, c) in [
            ("Sahih", GradeClass::Sahih),
            ("Hasan Sahih", GradeClass::Sahih),
            ("Sahih Lighairihi", GradeClass::Sahih),
            ("Sahih - Agreed Upon", GradeClass::Sahih),
            ("Isnaad Hasan", GradeClass::Hasan),
            ("Hasan Lighairihi", GradeClass::Hasan),
            ("Daif", GradeClass::Daif),
            ("Daif Isnaad", GradeClass::Daif),
            ("Very Daif", GradeClass::Daif),
            ("Munkar", GradeClass::Daif),
            ("Mawdu", GradeClass::Daif),
            ("Shadh, Sahih", GradeClass::Daif),
            ("Mauquf Daif", GradeClass::Daif),
            ("Maqtu", GradeClass::Other),
            ("-", GradeClass::Other),
        ] {
            assert_eq!(classify(g), c, "{g}");
        }
        assert_eq!(GradeClass::parse("Da'if"), Some(GradeClass::Daif));
        assert_eq!(GradeClass::parse("SAHIH"), Some(GradeClass::Sahih));
        assert_eq!(GradeClass::parse("x"), None);
    }

    #[test]
    fn al_albani_leads() {
        let gs = vec![Grade::new("Abu Ghuddah", "Hasan"), Grade::new("Al-Albani", "Sahih"), Grade::new("Zubair Ali Zai", "Daif")];
        assert_eq!(lead(&gs).map(|g| g.class), Some(GradeClass::Sahih));
        assert_eq!(lead(&gs[..1]).map(|g| g.grader.as_str()), Some("Abu Ghuddah"));
        assert!(lead(&[]).is_none());
    }
}
