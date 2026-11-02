//! The library on disk, in `$XDG_DATA_HOME/mandara/hadith/`: each
//! collection's Arabic edition (`ara-<id>.json`) and one translation
//! (`<lang>-<id>.json`), exactly as the dataset serves them, plus the
//! dataset's `editions.json` (who translated what).
//!
//! A download reads each file into memory, checks it ([`super::data`]:
//! shape, counts, numbering, a translation that lines up) and only then
//! writes it in place atomically, so an interrupted or broken download
//! never leaves a bad file. Everything here blocks: call it off the UI
//! thread. Offline mode stops downloads before they start.
//!
//! Loaded libraries (and their search indexes) are shared process-wide,
//! one per collection and language, built on first use.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mandara_i18n::{t, tf, tr};
use parking_lot::Mutex;
use serde::Serialize;

use super::collections::{self, Collection, info};
use super::data::{self, Library, MeaningInfo, RawEdition};
use super::search::{self, Entry};

/// Where the dataset is served from (jsDelivr's mirror of the GitHub
/// repository, pinned to major version 1).
pub const BASE: &str = "https://cdn.jsdelivr.net/gh/fawazahmed0/hadith-api@1";
/// The dataset's home page (credited in the panel).
pub const HOME: &str = "https://github.com/fawazahmed0/hadith-api";
/// Connect and body timeouts.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const BODY_TIMEOUT: Duration = Duration::from_secs(180);
/// Largest file accepted.
const MAX_BYTES: u64 = 64 << 20;
/// Read buffer.
const CHUNK: usize = 64 * 1024;
/// The translators' list.
const EDITIONS: &str = "editions.json";
/// Progress is reported at most every this many bytes.
const PROGRESS_STEP: u64 = 256 * 1024;

/// `$XDG_DATA_HOME/mandara/hadith`.
pub fn dir() -> PathBuf {
    mandara_config::paths::data_dir().join("hadith")
}

/// The file of an edition (`ara`, `eng`…) of `c`.
pub fn file(c: Collection, lang: &str) -> PathBuf {
    dir().join(format!("{}.json", collections::edition(c, lang)))
}

/// What is on disk for a collection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Status {
    pub collection: Collection,
    /// The Arabic is downloaded.
    pub arabic: bool,
    /// The translation wanted for the interface language, and whether it
    /// is downloaded.
    pub translation: Option<(String, bool)>,
    /// Bytes on disk (every file of the collection).
    pub bytes: u64,
}

impl Status {
    /// Downloaded with everything the interface language wants.
    pub fn complete(&self) -> bool {
        self.arabic && self.translation.as_ref().is_none_or(|(_, have)| *have)
    }
}

/// Every file of `c` in the folder (its Arabic and any translations).
fn files_of(c: Collection) -> Vec<PathBuf> {
    let suffix = format!("-{}.json", c.id());
    std::fs::read_dir(dir())
        .map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(&suffix) && n.len() == suffix.len() + 3)).collect())
        .unwrap_or_default()
}

/// What is on disk for `c`, for interface language `ui` (a few `stat`s).
pub fn status(c: Collection, ui: &str) -> Status {
    let present = |lang: &str| std::fs::metadata(file(c, lang)).is_ok_and(|m| m.len() > 0);
    let translation = collections::translation(c, ui).map(|(lang, _)| (lang.to_string(), present(lang)));
    let bytes = files_of(c).iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
    Status { collection: c, arabic: present("ara"), translation, bytes }
}

/// Every collection's status.
pub fn statuses(ui: &str) -> Vec<Status> {
    Collection::ALL.into_iter().map(|c| status(c, ui)).collect()
}

/// What a download is doing.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Progress {
    pub collection: Collection,
    /// The edition being fetched (`ara-bukhari`), or `verify` while the
    /// files are checked.
    pub step: String,
    /// Bytes of the collection received, and about how many it has.
    pub done: u64,
    pub total: u64,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_connect(Some(CONNECT_TIMEOUT)).timeout_recv_body(Some(BODY_TIMEOUT)).build().into()
}

/// Fetches `url` into memory, reporting bytes as they arrive.
fn fetch(agent: &ureq::Agent, url: &str, mut on_bytes: impl FnMut(u64)) -> Result<String, String> {
    let mut response = agent
        .get(url)
        .header("User-Agent", concat!("mandara-shell/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| tf("Downloading {file}: {error}", &[("file", &url.rsplit('/').next().unwrap_or(url)), ("error", &e)]))?;
    let mut reader = response.body_mut().with_config().limit(MAX_BYTES).reader();
    let mut body = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    let mut reported = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| tf("Downloading {file}: {error}", &[("file", &url.rsplit('/').next().unwrap_or(url)), ("error", &e)]))?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
        let got = body.len() as u64;
        if got - reported >= PROGRESS_STEP {
            reported = got;
            on_bytes(got);
        }
    }
    on_bytes(body.len() as u64);
    String::from_utf8(body).map_err(|e| tf("Unexpected hadith file: {error}", &[("error", &e)]))
}

fn edition_url(edition: &str) -> String {
    format!("{BASE}/editions/{edition}.min.json")
}

fn write(path: &std::path::Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    mandara_config::store::write_atomic(path, text.as_bytes()).map_err(|e| format!("{}: {e:#}", path.display()))
}

/// Downloads `c` (its Arabic and the translation for interface language
/// `ui`), skipping files already there; each file is checked before it is
/// written. Blocking; honours offline mode.
pub fn download(c: Collection, ui: &str, progress: &dyn Fn(Progress)) -> Result<(), String> {
    crate::offline::check()?;
    let agent = agent();
    let translation = collections::translation(c, ui).map(|(lang, _)| lang);
    let total = collections::download_size(c, translation);
    let report = |step: &str, done: u64| progress(Progress { collection: c, step: step.to_string(), done: done.min(total), total });
    // The translators' names (small; refreshed with every download).
    if let Ok(text) = fetch(&agent, &format!("{BASE}/{EDITIONS}"), |_| {})
        && serde_json::from_str::<serde_json::Value>(&text).is_ok_and(|v| v.get(c.id()).is_some())
    {
        let _ = write(&dir().join(EDITIONS), &text);
    }
    let mut done = 0u64;
    // The Arabic first: a translation is checked against it.
    let ara_path = file(c, "ara");
    let arabic: RawEdition = match std::fs::read_to_string(&ara_path).ok().and_then(|t| data::parse(&t).ok()).filter(|r| data::verify_arabic(c, r).is_ok()) {
        Some(raw) => raw,
        None => {
            let name = collections::edition(c, "ara");
            let text = fetch(&agent, &edition_url(&name), |n| report(&name, n))?;
            report("verify", info(c).arabic_bytes);
            let raw = data::parse(&text)?;
            data::verify_arabic(c, &raw)?;
            write(&ara_path, &text)?;
            raw
        }
    };
    done += info(c).arabic_bytes;
    if let Some(lang) = translation {
        let path = file(c, lang);
        let fine = std::fs::read_to_string(&path).ok().and_then(|t| data::parse(&t).ok()).is_some_and(|r| data::verify(c, &r).and_then(|_| data::verify_pair(&arabic, &r)).is_ok());
        if !fine {
            let name = collections::edition(c, lang);
            let text = fetch(&agent, &edition_url(&name), |n| report(&name, done + n))?;
            report("verify", total);
            let raw = data::parse(&text)?;
            data::verify(c, &raw).and_then(|_| data::verify_pair(&arabic, &raw))?;
            write(&path, &text)?;
        }
    }
    report("done", total);
    forget(c);
    Ok(())
}

/// Removes every file of `c`; the bytes freed.
pub fn remove(c: Collection) -> Result<u64, String> {
    let mut freed = 0;
    for path in files_of(c) {
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        match std::fs::remove_file(&path) {
            Ok(()) => freed += len,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    forget(c);
    Ok(freed)
}

/// The translator of an edition, from the saved `editions.json`.
fn translator(c: Collection, lang: &str) -> Option<String> {
    let text = std::fs::read_to_string(dir().join(EDITIONS)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let name = collections::edition(c, lang);
    let author = v.get(c.id())?.get("collection")?.as_array()?.iter().find(|e| e.get("name").and_then(|n| n.as_str()) == Some(name.as_str()))?.get("author")?.as_str()?.trim().to_string();
    (!author.is_empty() && !author.eq_ignore_ascii_case("unknown")).then_some(author)
}

/// Reads and builds `c` from disk (with the translation for `ui` when it
/// is there). Blocking, not cached.
pub fn read(c: Collection, ui: &str) -> Result<Library, String> {
    let text = std::fs::read_to_string(file(c, "ara")).map_err(|_| tf("{collection} isn't downloaded", &[("collection", &tr(info(c).title))]))?;
    let arabic = data::parse(&text)?;
    drop(text);
    let translation = collections::translation(c, ui).and_then(|(lang, fallback)| {
        let raw = data::parse(&std::fs::read_to_string(file(c, lang)).ok()?).ok()?;
        Some((raw, MeaningInfo { lang: lang.to_string(), translator: translator(c, lang), fallback }))
    });
    // A translation that doesn't line up is left out, not fatal.
    match Library::build(c, arabic, translation) {
        Ok(lib) => Ok(lib),
        Err(e) if collections::translation(c, ui).is_some() => {
            tracing::warn!("hadith: {}: {e}; reading the Arabic only", c.id());
            let arabic = data::parse(&std::fs::read_to_string(file(c, "ara")).map_err(|e| e.to_string())?)?;
            Library::build(c, arabic, None)
        }
        Err(e) => Err(e),
    }
}

/// A loaded collection and its search index (built on first search).
pub struct Loaded {
    pub library: Arc<Library>,
    pub index: Mutex<Option<Arc<Vec<Entry>>>>,
}

impl Loaded {
    /// The search index, built now when missing (blocking).
    pub fn index(&self) -> Arc<Vec<Entry>> {
        let mut slot = self.index.lock();
        slot.get_or_insert_with(|| Arc::new(search::index(&self.library))).clone()
    }
}

type Cache = HashMap<(Collection, Option<&'static str>), Arc<Loaded>>;

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

fn cache_key(c: Collection, ui: &str) -> (Collection, Option<&'static str>) {
    (c, collections::translation(c, ui).map(|(lang, _)| lang))
}

/// The loaded `c` for `ui`, when it is (never blocks).
pub fn cached(c: Collection, ui: &str) -> Option<Arc<Loaded>> {
    CACHE.lock().as_ref()?.get(&cache_key(c, ui)).cloned()
}

/// The loaded `c` for `ui`, reading it now when needed (blocking).
pub fn load(c: Collection, ui: &str) -> Result<Arc<Loaded>, String> {
    if let Some(hit) = cached(c, ui) {
        return Ok(hit);
    }
    let lib = read(c, ui)?;
    let loaded = Arc::new(Loaded { library: Arc::new(lib), index: Mutex::new(None) });
    CACHE.lock().get_or_insert_with(HashMap::new).insert(cache_key(c, ui), loaded.clone());
    Ok(loaded)
}

/// Drops `c` from memory (after a download or a removal).
pub fn forget(c: Collection) {
    if let Some(map) = CACHE.lock().as_mut() {
        map.retain(|(k, _), _| *k != c);
    }
}

/// Drops whatever was loaded for another interface language.
pub fn retain_language(ui: &str) {
    if let Some(map) = CACHE.lock().as_mut() {
        map.retain(|(c, lang), _| collections::translation(*c, ui).map(|(l, _)| l) == *lang);
    }
}

/// A short line for the Maintenance page: how many collections are on
/// disk and their size.
pub fn summary() -> (usize, u64) {
    let all: Vec<Status> = Collection::ALL.into_iter().map(|c| status(c, "ar")).collect();
    (all.iter().filter(|s| s.arabic).count(), all.iter().map(|s| s.bytes).sum())
}

/// Removes every collection; the bytes freed.
pub fn remove_all() -> Result<u64, String> {
    let mut freed = 0;
    for c in Collection::ALL {
        freed += remove(c)?;
    }
    let editions = dir().join(EDITIONS);
    if let Ok(m) = std::fs::metadata(&editions) {
        freed += m.len();
        let _ = std::fs::remove_file(&editions);
    }
    let _ = std::fs::remove_dir(dir());
    Ok(freed)
}

/// The message a failed download shows (offline mode named plainly).
pub fn explain(error: &str) -> String {
    if error == crate::offline::blocked() { t("Offline mode is on: nothing is downloaded").to_string() } else { error.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_live_in_the_data_folder() {
        assert!(file(Collection::Bukhari, "ara").ends_with("hadith/ara-bukhari.json"));
        assert!(file(Collection::Muslim, "eng").ends_with("hadith/eng-muslim.json"));
        assert_eq!(edition_url("ara-bukhari"), "https://cdn.jsdelivr.net/gh/fawazahmed0/hadith-api@1/editions/ara-bukhari.min.json");
        let s = Status { collection: Collection::Bukhari, arabic: true, translation: Some(("eng".into(), false)), bytes: 1 };
        assert!(!s.complete());
        assert!(Status { translation: None, ..s.clone() }.complete());
    }
}
