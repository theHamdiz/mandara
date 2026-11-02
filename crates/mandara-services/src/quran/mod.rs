//! The Qur'an plugin's services, plain Rust with no UI (the plugin lives
//! in `mandara-ui`): [`text`] parses and verifies the Tanzil Uthmani text,
//! its index and the meanings; [`store`] downloads them on first use and
//! keeps them (nothing ships in the binary); [`search`] folds the Arabic
//! for diacritic-insensitive search; [`state`] remembers where you read,
//! the khatm and bookmarks; [`daily`] picks the ayah of the day; and
//! [`player`] recites per-ayah audio through the sound server.

pub mod daily;
pub mod player;
pub mod search;
pub mod state;
pub mod store;
pub mod text;

pub use text::{Ayah, Quran, Ref, Revelation, Selection, Surah, Translation};

/// The meaning edition for an interface language: `chosen` (an edition id
/// from Studio) when it is in that language, else the language's default
/// (the edition Jannah uses). `None` for Arabic: Arabic only.
pub fn meaning_edition(lang: &str, chosen: &str) -> Option<String> {
    let default = crate::jannah::meanings::edition(lang)?;
    let chosen = chosen.trim();
    let ok = !chosen.is_empty() && chosen.split_once('.').is_some_and(|(l, rest)| l == lang && !rest.is_empty()) && store::sanitize(chosen) == chosen;
    Some(if ok { chosen.to_string() } else { default.id.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_meaning_follows_the_language_and_the_choice() {
        assert_eq!(meaning_edition("en", ""), Some("en.sahih".into()));
        assert_eq!(meaning_edition("en", "en.yusufali"), Some("en.yusufali".into()));
        // Another language's edition, or nonsense, falls back.
        assert_eq!(meaning_edition("en", "fr.hamidullah"), Some("en.sahih".into()));
        assert_eq!(meaning_edition("en", "en."), Some("en.sahih".into()));
        assert_eq!(meaning_edition("en", "en/../x"), Some("en.sahih".into()));
        assert_eq!(meaning_edition("de", " de.aburida "), Some("de.aburida".into()));
        assert_eq!(meaning_edition("ar", "ar.muyassar"), None, "Arabic: no translation");
    }
}
