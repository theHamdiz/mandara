//! The adhan (call to prayer) for the Prayer times plugin. Nothing of it
//! ships: the recording is downloaded once, when you turn the adhan on,
//! into `~/.local/share/mandara/prayer/adhan/adhan.mp3` (written to a
//! `.part` file first, checked to be MP3, then renamed), and kept until you
//! remove it. Playback goes through the Qur'an recitation player
//! ([`crate::quran::player`]): one playback stream ("Mandara adhan") on
//! the sound server at the system volume, stoppable at any moment.
//!
//! The recording: "Beautiful adhan" by Adam-synagda, released into the
//! public domain (CC0 1.0) on Wikimedia Commons, fetched as Commons' own
//! MP3 rendering of the original Ogg file.

use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::time::Duration;

use mandara_i18n::tf;

use crate::quran::player::{self, Player, Source};

/// Commons' MP3 rendering of the recording.
pub const URL: &str = "https://upload.wikimedia.org/wikipedia/commons/transcoded/b/b0/Beautiful_adhan.ogg/Beautiful_adhan.ogg.mp3";
/// Where the recording and its licence are described.
pub const PAGE: &str = "https://commons.wikimedia.org/wiki/File:Beautiful_adhan.ogg";
/// The licence and who recorded it, as shown in the panel and docs.
pub const LICENCE: &str = "CC0 1.0";
pub const AUTHOR: &str = "Adam-synagda";
/// About how large the download is (for the progress before the server
/// says).
pub const EXPECTED_BYTES: u64 = 1_781_329;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const BODY_TIMEOUT: Duration = Duration::from_secs(120);
/// Smaller isn't the recording (an error page); larger isn't either.
const MIN_BYTES: u64 = 64 << 10;
const MAX_BYTES: u64 = 24 << 20;
const CHUNK: usize = 64 << 10;
/// Wikimedia asks for a descriptive User-Agent.
const USER_AGENT: &str = concat!("mandara-shell/", env!("CARGO_PKG_VERSION"), " (https://github.com/theHamdiz/mandara)");

/// `~/.local/share/mandara/prayer/adhan/`.
pub fn dir() -> PathBuf {
    mandara_config::paths::data_dir().join("prayer").join("adhan")
}

/// The downloaded recording.
pub fn path() -> PathBuf {
    dir().join("adhan.mp3")
}

/// The recording's size when it is downloaded.
pub fn downloaded() -> Option<u64> {
    std::fs::metadata(path()).ok().map(|m| m.len()).filter(|n| *n >= MIN_BYTES)
}

/// Downloads the recording (blocking; refused while offline), reporting
/// `(bytes, total)` as they arrive; the file's path.
pub fn download(progress: &dyn Fn(u64, u64)) -> Result<PathBuf, String> {
    crate::offline::check()?;
    let failed = |e: &dyn std::fmt::Display| tf("Downloading the adhan: {error}", &[("error", e)]);
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_connect(Some(CONNECT_TIMEOUT)).timeout_recv_body(Some(BODY_TIMEOUT)).build().into();
    let mut response = agent.get(URL).header("User-Agent", USER_AGENT).call().map_err(|e| failed(&e))?;
    let total = response.body().content_length().unwrap_or(EXPECTED_BYTES);
    let mut reader = response.body_mut().with_config().limit(MAX_BYTES).reader();
    let mut body = Vec::with_capacity(usize::try_from(total).unwrap_or_default());
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = reader.read(&mut buf).map_err(|e| failed(&e))?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
        progress(body.len() as u64, total.max(body.len() as u64));
    }
    if (body.len() as u64) < MIN_BYTES || !player::looks_like_mp3(&body) {
        return Err(failed(&"not an MP3 file"));
    }
    let path = path();
    std::fs::create_dir_all(dir()).map_err(|e| failed(&e))?;
    let part = path.with_extension(format!("mp3.{}.part", std::process::id()));
    let written = std::fs::File::create(&part).and_then(|mut f| f.write_all(&body).and_then(|_| f.sync_all())).and_then(|_| std::fs::rename(&part, &path));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&part);
        return Err(failed(&e));
    }
    Ok(path)
}

/// Deletes the recording; the bytes freed.
pub fn remove() -> Result<u64, String> {
    let freed = downloaded().unwrap_or_default();
    match std::fs::remove_file(path()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", path().display())),
    }
    let _ = std::fs::remove_dir(dir());
    Ok(freed)
}

/// The player's file: the downloaded recording (never fetched here, so a
/// prayer never waits on the network).
fn file(_voice: &str, _item: u16) -> Result<PathBuf, String> {
    downloaded().map(|_| path()).ok_or_else(|| mandara_i18n::t("The adhan isn't downloaded").to_string())
}

/// How the adhan plays: the downloaded file on its own stream.
struct Downloaded;

impl crate::quran::audio::AudioSource for Downloaded {
    fn prepare(&self, request: &crate::quran::audio::Request<'_>, _: &mut crate::quran::audio::FetchCache) -> Result<crate::quran::audio::Prepared, String> {
        request.observer.check()?;
        file(request.voice, request.global).map(crate::quran::audio::Prepared::file)
    }
}

pub const SOURCE: Source = Source { files: &Downloaded, client: "Mandara adhan", media: c"Adhan", app_id: c"org.mandara.prayer" };

/// A player for the adhan (events as the recitation player's).
pub fn player(on_event: impl Fn(player::Event) + Send + Sync + 'static) -> Player {
    Player::with_source(SOURCE, on_event)
}

/// Plays the adhan on `p` (from its start).
pub fn play(p: &Player) {
    p.play(vec![1], "adhan");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recording_lives_under_the_data_dir_and_is_never_fetched_to_play() {
        assert!(path().ends_with("prayer/adhan/adhan.mp3"));
        assert!(URL.starts_with("https://upload.wikimedia.org/") && URL.ends_with(".mp3"));
        // Playing never downloads: without the file the source refuses.
        if downloaded().is_none() {
            assert!(file("adhan", 1).is_err());
        }
    }
}
