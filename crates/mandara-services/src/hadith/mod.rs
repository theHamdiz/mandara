//! The Hadith library: whole collections of the public-domain (Unlicense)
//! hadith-api dataset, downloaded on demand, never bundled. Plain Rust,
//! no UI (the plugin lives in `mandara-ui`).
//!
//! * [`collections`]: the ten collections the dataset offers, their names,
//!   counts (checked against the dataset's `info.json`) and which
//!   translations each has;
//! * [`data`]: parses an edition, checks it and builds a [`data::Library`]
//!   (books, hadith, references, grades, the meaning);
//! * [`refs`]: sunnah.com references and links, and reading what people
//!   type (`bukhari 1`, `muslim 2699a`);
//! * [`chain`]: where a hadith's chain of narrators ends and its words
//!   begin; [`grade`]: the graders' words sorted into sahih / hasan /
//!   da'if;
//! * [`store`]: download (verified, swapped in atomically), remove, status
//!   and the loaded libraries; [`search`]: diacritic-insensitive search;
//!   [`rotation`]: the hadith of the day; [`state`]: bookmarks, reading
//!   progress and today's card.

pub mod chain;
pub mod collections;
pub mod data;
pub mod grade;
pub mod refs;
pub mod rotation;
pub mod search;
pub mod state;
pub mod store;

pub use collections::Collection;
pub use data::{Book, Hadith, Library};
pub use grade::{Grade, GradeClass};
