//! Jannah: a gentle daily reminder of Paradise. The bundled content
//! ([`data`]: Qur'an verses and hadith, verbatim, and [`paths`]: deeds and
//! manners that lead there) is read through one [`catalog`]; [`recent`]
//! draws a fresh reminder at random, never one shown lately, [`rotation`]
//! holds the periods and today's step, [`meanings`] fetches and caches
//! Qur'an translations (never bundled) and holds the hadith meanings,
//! [`search`] folds Arabic for diacritic-insensitive search and [`state`]
//! keeps saved reminders and done days. Plain Rust, no UI: the plugin
//! lives in `mandara-ui`.

pub mod catalog;
pub mod data;
pub mod meanings;
pub mod paths;
pub mod recent;
pub mod rotation;
pub mod search;
pub mod state;

pub use data::{Collection, Hadith, HadithText, Surah, Theme, Verse};
pub use paths::{PATHS, Path, PathGroup};
