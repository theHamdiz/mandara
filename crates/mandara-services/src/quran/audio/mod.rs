//! Addressing, caching and preparing the supported recitation sources.

mod index;
mod timing;
mod transport;

use std::io::Read as _;
use std::path::{Path, PathBuf};

use mandara_i18n::{t, tf};

use super::reciters::{Provider, reciter_or_default};
pub use timing::Interval;
use transport::{Http, Transport};

pub use index::AyahIndex;

/// The player cancels a download when its queue changes or is paused.
pub trait Observer {
    /// Whether the request is still wanted (checked between steps and while
    /// downloading).
    fn current(&self) -> bool;
    /// Download progress in bytes (`total` is 0 when unknown).
    fn progress(&self, done: u64, total: u64);

    /// `Err` ("Recitation cancelled") once the request is no longer current.
    fn check(&self) -> Result<(), String> {
        if self.current() { Ok(()) } else { Err(t("Recitation cancelled").into()) }
    }
}

/// A file is decoded in packets; timed providers also bound its playback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub path: PathBuf,
    pub interval: Option<Interval>,
}

impl Prepared {
    /// A whole file to play, with no interval.
    pub fn file(path: PathBuf) -> Self {
        Self { path, interval: None }
    }
}

/// Only the current chapter's small timing table stays in the fetch worker.
#[derive(Default)]
pub struct FetchCache {
    timing: Option<(u16, u16, Box<[Interval]>)>,
}

/// One ayah to prepare: the reciter, its global number (1..=6236), the
/// index that maps it to surah and ayah, and who to report to.
pub struct Request<'a> {
    pub voice: &'a str,
    pub global: u16,
    pub index: Option<&'a AyahIndex>,
    pub observer: &'a dyn Observer,
}

/// Audio queues can prepare verse files or a bounded part of a chapter.
pub trait AudioSource: Send + Sync {
    fn prepare(&self, request: &Request<'_>, cache: &mut FetchCache) -> Result<Prepared, String>;
}

/// The real sources: HTTPS downloads into the Qur'an cache folder.
pub struct Recitations;

impl AudioSource for Recitations {
    fn prepare(&self, request: &Request<'_>, cache: &mut FetchCache) -> Result<Prepared, String> {
        prepare(request, cache, &Http, &super::store::cache_dir())
    }
}

/// The request's surah/ayah and that surah's ayah count, from the index.
fn reference(request: &Request<'_>) -> Result<(super::text::Ref, u16), String> {
    request.index.and_then(|index| index.locate(request.global)).ok_or_else(|| t("The recitation needs the downloaded Qur'an index").to_string())
}

/// Resolves the reciter's file for the request under `root`, downloading
/// it (and, for chapter recitations, the surah's timing table) only when
/// no valid cached copy exists. Runs on the fetch worker; the observer can
/// cancel between every step.
fn prepare(request: &Request<'_>, cache: &mut FetchCache, transport: &dyn Transport, root: &Path) -> Result<Prepared, String> {
    request.observer.check()?;
    if !(1..=super::text::AYAHS).contains(&request.global) {
        return Err(t("The recitation needs the downloaded Qur'an index").into());
    }
    let who = reciter_or_default(request.voice);
    let dir = root.join("audio").join(who.id);
    let mut interval = None;
    let (url, path, limit) = match who.provider {
        Provider::GlobalAyah => (format!("https://cdn.islamic.network/quran/audio/128/{}/{n}.mp3", who.id, n = request.global), dir.join(format!("{}.mp3", request.global)), transport::MAX_AYAH),
        Provider::EveryAyah { directory } => {
            let (r, _) = reference(request)?;
            (format!("https://everyayah.com/data/{directory}/{:03}{:03}.mp3", r.surah, r.ayah), dir.join(format!("{}.mp3", request.global)), transport::MAX_AYAH)
        }
        Provider::TimedChapter { read, directory } => {
            let (r, count) = reference(request)?;
            if cache.timing.as_ref().is_none_or(|(old_read, surah, _)| *old_read != read || *surah != r.surah) {
                let path = dir.join(format!("surah-{:03}.timing.json", r.surah));
                let loaded = read_timing(&path, count);
                let rows = match loaded {
                    Some(rows) => rows,
                    None => {
                        request.observer.check()?;
                        let url = format!("https://www.mp3quran.net/api/v3/ayat_timing?surah={}&read={read}", r.surah);
                        let bytes = transport.metadata(&url, request.observer)?;
                        let rows = timing::parse(&bytes, count).map_err(|e| tf("The recitation timing could not be read: {error}", &[("error", &e)]))?;
                        request.observer.check()?;
                        mandara_config::store::write_atomic(&path, &bytes).map_err(|e| e.to_string())?;
                        rows
                    }
                };
                cache.timing = Some((read, r.surah, rows));
            }
            interval = cache.timing.as_ref().and_then(|(_, _, rows)| rows.get(usize::from(r.ayah - 1))).copied();
            if interval.is_none() {
                return Err(t("The recitation needs the downloaded Qur'an index").into());
            }
            (format!("{directory}{:03}.mp3", r.surah), dir.join(format!("surah-{:03}.mp3", r.surah)), transport::MAX_CHAPTER)
        }
    };
    if !transport::cached_audio(&path, limit) {
        transport.audio(&url, &path, limit, request.observer)?;
    }
    request.observer.check()?;
    Ok(Prepared { path, interval })
}

/// A cached timing table, if it is within `MAX_TIMING` bytes and parses
/// for `count` ayahs (otherwise it is fetched again).
fn read_timing(path: &Path, count: u16) -> Option<Box<[Interval]>> {
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > transport::MAX_TIMING {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(transport::MAX_TIMING + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > transport::MAX_TIMING {
        return None;
    }
    timing::parse(&bytes, count).ok()
}

/// A quick offline preflight. The fetch worker validates file contents and
/// timing before playback, without reading a timing table on the UI thread.
pub fn cached(voice: &str, global: u16, index: &AyahIndex) -> bool {
    cached_at(&super::store::cache_dir(), voice, global, index)
}

/// [`cached`] under `root`: the audio (and timing) files exist with a
/// plausible size; contents are checked later, on the worker.
fn cached_at(root: &Path, voice: &str, global: u16, index: &AyahIndex) -> bool {
    let who = reciter_or_default(voice);
    let dir = root.join("audio").join(who.id);
    match who.provider {
        Provider::GlobalAyah | Provider::EveryAyah { .. } => cached_size(&dir.join(format!("{global}.mp3")), transport::MIN_AUDIO, transport::MAX_AYAH),
        Provider::TimedChapter { .. } => {
            let Some((r, _)) = index.locate(global) else {
                return false;
            };
            let path = dir.join(format!("surah-{:03}.timing.json", r.surah));
            cached_size(&dir.join(format!("surah-{:03}.mp3", r.surah)), transport::MIN_AUDIO, transport::MAX_CHAPTER) && cached_size(&path, 1, transport::MAX_TIMING)
        }
    }
}

/// Whether `path` is a file of `min..=max` bytes.
fn cached_size(path: &Path, min: u64, max: u64) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && (min..=max).contains(&m.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Live;
    impl Observer for Live {
        fn current(&self) -> bool {
            true
        }
        fn progress(&self, _: u64, _: u64) {}
    }

    #[derive(Default)]
    struct Fake {
        urls: RefCell<Vec<String>>,
    }
    impl Transport for Fake {
        fn audio(&self, url: &str, path: &Path, _: u64, _: &dyn Observer) -> Result<(), String> {
            self.urls.borrow_mut().push(url.into());
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut bytes = vec![0; transport::MIN_AUDIO as usize];
            bytes[..3].copy_from_slice(b"ID3");
            std::fs::write(path, bytes).unwrap();
            Ok(())
        }
        fn metadata(&self, url: &str, _: &dyn Observer) -> Result<Vec<u8>, String> {
            self.urls.borrow_mut().push(url.into());
            let rows = (1..=7).map(|ayah| format!(r#"{{"ayah":{ayah},"start_time":{},"end_time":{}}}"#, ayah * 1000, (ayah + 1) * 1000)).collect::<Vec<_>>().join(",");
            Ok(format!("[{rows}]").into_bytes())
        }
    }

    #[test]
    fn sources_use_verified_verse_addresses_and_reuse_one_timed_chapter() {
        let root = tempfile::tempdir().unwrap();
        let fake = Fake::default();
        let q: super::super::text::Quran = serde_json::from_value(
            serde_json::json!({"format":1,"edition":"quran-uthmani","bismillah":"","surahs":[{"number":1,"name":"","transliteration":"","meaning":"","revelation":"makki","ayahs":7,"first":1}],"ayahs":[],"juz_starts":[]}),
        )
        .unwrap();
        let index = AyahIndex::new(&q);
        let mut cache = FetchCache::default();
        for (voice, n) in [("ar.husary", 2), ("ar.abdulsamad", 2), ("ar.yasseraldossari", 2), ("mp3quran.258", 1), ("mp3quran.258", 2)] {
            let p = prepare(&Request { voice, global: n, index: Some(&index), observer: &Live }, &mut cache, &fake, root.path()).unwrap();
            assert_eq!(p.interval.is_some(), voice == "mp3quran.258");
        }
        assert_eq!(
            *fake.urls.borrow(),
            [
                "https://cdn.islamic.network/quran/audio/128/ar.husary/2.mp3",
                "https://everyayah.com/data/Abdul_Basit_Murattal_192kbps/001002.mp3",
                "https://everyayah.com/data/Yasser_Ad-Dussary_128kbps/001002.mp3",
                "https://www.mp3quran.net/api/v3/ayat_timing?surah=1&read=258",
                "https://cdn.mp3quran.net/audio/abdulrasheed-sufi/r3/001.mp3"
            ]
        );
        assert!(prepare(&Request { voice: "mp3quran.258", global: 1, index: None, observer: &Live }, &mut cache, &fake, root.path()).is_err());
        assert!(cached_at(root.path(), "mp3quran.258", 1, &index));
        let timing = root.path().join("audio/mp3quran.258/surah-001.timing.json");
        std::fs::remove_file(&timing).unwrap();
        assert!(!cached_at(root.path(), "mp3quran.258", 1, &index));
        assert!(cached_at(root.path(), "ar.yasseraldossari", 2, &index));
    }

    #[test]
    fn broken_or_oversized_timing_cache_is_never_used() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("timing.json");
        std::fs::write(&path, include_bytes!("fixtures/hafs-258-001.json")).unwrap();
        assert_eq!(read_timing(&path, 7).unwrap().len(), 7);
        assert!(read_timing(&path, 6).is_none());
        std::fs::write(&path, b"<html>unavailable</html>").unwrap();
        assert!(read_timing(&path, 7).is_none());
        std::fs::File::create(&path).unwrap().set_len(transport::MAX_TIMING + 1).unwrap();
        assert!(read_timing(&path, 7).is_none());
    }
}
