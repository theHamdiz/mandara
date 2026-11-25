//! Internet radio: curated, categorised stations and your own, played in
//! pure Rust (MP3, AAC and Ogg Vorbis through `symphonia`) to the sound
//! server, with ICY now-playing titles, playlists, reconnects with backoff
//! and an MPRIS player for media keys and the media panel. Nothing runs
//! between sessions: see [`player`] for the threads and their lifetime.

pub mod backoff;
pub mod decode;
pub mod directory;
pub mod http;
pub mod icy;
pub mod library;
pub mod mpris;
pub mod output;
pub mod player;
pub mod playlist;
pub mod ring;
pub mod station;

pub use library::{Library, Station};
pub use player::{Options, Player, Probe, Request, Snapshot, Status, StreamInfo, Tune, probe};
pub use station::{CURATED, Category, Codec, Curated};
