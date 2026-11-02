//! Jannah: a gentle, constant reminder of Paradise, its description and
//! the deeds and manners that lead to it. The content (Qur'an verses and
//! hadith, copied verbatim) lives in `data/jannah/`; see its README.

pub mod data;
pub mod paths;

pub use data::{Collection, Hadith, HadithText, Item, Surah, Theme, Verse};
pub use paths::{Path, PathGroup, PATHS};
