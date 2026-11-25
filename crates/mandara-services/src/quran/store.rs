//! Where the Qur'an plugin keeps its downloads, and the downloads
//! themselves. Nothing of the text ships in the binary: on first use the
//! index (`/v1/meta`, ~50 KB), the Tanzil Uthmani text
//! (`/v1/quran/quran-uthmani`, ~2.1 MB) and, outside Arabic, one meaning
//! (`/v1/quran/<edition>`, ~1.6 MB) are fetched from api.alquran.cloud,
//! verified ([`super::text`]) and only then written, each file atomically,
//! to `~/.local/share/mandara/quran/`. A failed or partial download never
//! replaces good files. The list of translation editions is cached there
//! too; recitation audio lives in `~/.cache/mandara/quran/audio/`.
//!
//! Everything here blocks: call it off the UI thread. Offline mode stops
//! every request ([`crate::offline`]).

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mandara_i18n::tf;
use serde::Serialize;

use super::text::{self, EditionInfo, Quran, Translation};

/// The API and its endpoints.
pub const API: &str = "https://api.alquran.cloud/v1";
const META: &str = "/meta";
const TEXT: &str = "/quran/quran-uthmani";
const EDITIONS: &str = "/edition?format=text&type=translation";
/// Timeouts (network safety bounds).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const BODY_TIMEOUT: Duration = Duration::from_secs(120);
/// Largest answer accepted.
const MAX_BYTES: u64 = 32 << 20;
/// Read buffer.
const CHUNK: usize = 64 * 1024;
/// Sizes the answers usually have (when the server sends no length), so
/// progress moves smoothly: the index, the text, a meaning.
pub const META_BYTES: u64 = 53_000;
pub const TEXT_BYTES: u64 = 2_110_000;
pub const MEANING_BYTES: u64 = 1_620_000;

/// `~/.local/share/mandara/quran`.
pub fn dir() -> PathBuf {
    mandara_config::paths::data_dir().join("quran")
}

/// The text with its index.
pub fn text_path() -> PathBuf {
    dir().join("quran.json")
}

/// A meaning edition.
pub fn meaning_path(edition: &str) -> PathBuf {
    dir().join(format!("meaning-{}.json", sanitize(edition)))
}

/// The cached list of translation editions.
pub fn editions_path() -> PathBuf {
    dir().join("editions.json")
}

/// `~/.cache/mandara/quran` (recitation audio).
pub fn cache_dir() -> PathBuf {
    mandara_config::paths::cache_dir().join("quran")
}

/// An edition or reciter identifier made safe for a file name.
pub fn sanitize(id: &str) -> String {
    id.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')).collect::<String>().trim_start_matches('.').to_string()
}

/// What is being fetched.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Index,
    Text,
    Meaning,
    Saving,
}

/// Download progress: the stage, bytes so far and the expected total of
/// the whole download.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub stage: Stage,
    pub done: u64,
    pub total: u64,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_connect(Some(CONNECT_TIMEOUT)).timeout_recv_body(Some(BODY_TIMEOUT)).build().into()
}

/// GETs `path` of the API as text, reporting bytes as they arrive.
fn get(path: &str, on_bytes: &mut dyn FnMut(u64)) -> Result<String, String> {
    crate::offline::check()?;
    let url = format!("{API}{path}");
    let failed = |e: &dyn std::fmt::Display| tf("Downloading {url}: {error}", &[("url", &url), ("error", e)]);
    let mut response = agent().get(&url).header("User-Agent", concat!("mandara-shell/", env!("CARGO_PKG_VERSION"))).call().map_err(|e| failed(&e))?;
    let mut reader = response.body_mut().with_config().limit(MAX_BYTES).reader();
    let mut body = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = reader.read(&mut buf).map_err(|e| failed(&e))?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
        on_bytes(body.len() as u64);
    }
    String::from_utf8(body).map_err(|e| failed(&e))
}

fn write(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let json = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    mandara_config::store::write_atomic(path, &json).map_err(|e| format!("{}: {e:#}", path.display()))
}

/// The stored text, verified (`None`: missing, damaged or of an older
/// layout).
pub fn load_text() -> Option<Quran> {
    let bytes = std::fs::read(text_path()).ok()?;
    let q: Quran = serde_json::from_slice(&bytes).ok()?;
    q.verify().ok()?;
    Some(q)
}

/// A stored meaning, verified.
pub fn load_meaning(edition: &str) -> Option<Translation> {
    let bytes = std::fs::read(meaning_path(edition)).ok()?;
    let t: Translation = serde_json::from_slice(&bytes).ok()?;
    (t.edition == edition && t.verify().is_ok()).then_some(t)
}

/// The text (stored, else downloaded) and, when `edition` is given, its
/// meaning (stored, else downloaded). `report` sees the progress of what
/// is downloaded.
pub fn ensure(edition: Option<&str>, report: &dyn Fn(Progress)) -> Result<(Quran, Option<Translation>), String> {
    let text = load_text();
    let meaning = edition.and_then(load_meaning);
    let need_text = text.is_none();
    let need_meaning = edition.is_some() && meaning.is_none();
    let total = if need_text { META_BYTES + TEXT_BYTES } else { 0 } + if need_meaning { MEANING_BYTES } else { 0 };
    let mut base = 0u64;
    let q = match text {
        Some(q) => q,
        None => {
            let meta = get(META, &mut |n| report(Progress { stage: Stage::Index, done: n.min(META_BYTES), total }))?;
            let (surahs, juz) = text::parse_meta(&meta)?;
            base += META_BYTES;
            let body = get(TEXT, &mut |n| report(Progress { stage: Stage::Text, done: base + n.min(TEXT_BYTES), total }))?;
            base += TEXT_BYTES;
            report(Progress { stage: Stage::Saving, done: base, total });
            let q = text::build(&body, surahs, juz)?;
            write(&text_path(), &q)?;
            q
        }
    };
    let meaning = match (edition, meaning) {
        (Some(_), Some(m)) => Some(m),
        (Some(e), None) => {
            let body = get(&format!("/quran/{}", sanitize(e)), &mut |n| report(Progress { stage: Stage::Meaning, done: base + n.min(MEANING_BYTES), total }))?;
            report(Progress { stage: Stage::Saving, done: total, total });
            let m = text::parse_translation(&body, e, &q.surahs)?;
            write(&meaning_path(e), &m)?;
            Some(m)
        }
        (None, _) => None,
    };
    if total > 0 {
        report(Progress { stage: Stage::Saving, done: total, total });
    }
    Ok((q, meaning))
}

/// The translation editions the API offers (cached; downloaded once).
pub fn editions() -> Result<Vec<EditionInfo>, String> {
    if let Some(list) = std::fs::read(editions_path()).ok().and_then(|b| serde_json::from_slice::<Vec<EditionInfo>>(&b).ok()).filter(|l| !l.is_empty()) {
        return Ok(list);
    }
    let body = get(EDITIONS, &mut |_| {})?;
    let list = text::parse_editions(&body)?;
    write(&editions_path(), &list)?;
    Ok(list)
}

/// What is on disk.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub text: bool,
    pub text_bytes: u64,
    /// Stored meanings: edition and size.
    pub meanings: Vec<(String, u64)>,
    /// Cached recitation files and their size.
    pub audio_files: u64,
    pub audio_bytes: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.text_bytes + self.meanings.iter().map(|(_, b)| b).sum::<u64>() + self.audio_bytes
    }
}

/// Files and bytes under `dir`, recursively.
fn walk(dir: &Path) -> (u64, u64) {
    let Ok(entries) = std::fs::read_dir(dir) else { return (0, 0) };
    entries.flatten().fold((0, 0), |(n, b), e| match e.file_type() {
        Ok(t) if t.is_dir() => {
            let (n2, b2) = walk(&e.path());
            (n + n2, b + b2)
        }
        Ok(_) => (n + 1, b + e.metadata().map(|m| m.len()).unwrap_or(0)),
        Err(_) => (n, b),
    })
}

/// What the plugin keeps on disk (stats only; the text isn't parsed).
pub fn usage() -> Usage {
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
    let text_bytes = size(&text_path());
    let mut meanings: Vec<(String, u64)> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let edition = name.strip_prefix("meaning-")?.strip_suffix(".json")?.to_string();
            Some((edition, e.metadata().ok()?.len()))
        })
        .collect();
    meanings.sort();
    let (audio_files, audio_bytes) = walk(&cache_dir().join("audio"));
    Usage { text: text_bytes.is_some(), text_bytes: text_bytes.unwrap_or(0), meanings, audio_files, audio_bytes }
}

/// The text's and meanings' size on disk when the text is downloaded (a
/// few `stat`s; recitations aren't counted).
pub fn installed_bytes() -> Option<u64> {
    std::fs::metadata(text_path()).ok()?;
    Some(std::fs::read_dir(dir()).ok()?.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum())
}

/// Removes every download (text, meanings, the edition list and cached
/// recitations); the bytes freed.
pub fn remove() -> Result<u64, String> {
    let freed = usage().total() + std::fs::metadata(editions_path()).map(|m| m.len()).unwrap_or(0);
    for d in [dir(), cache_dir()] {
        match std::fs::remove_dir_all(&d) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", d.display())),
        }
    }
    Ok(freed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_safe_file_names() {
        assert_eq!(sanitize("en.sahih"), "en.sahih");
        assert_eq!(sanitize("../../etc/passwd"), "etcpasswd");
        assert_eq!(sanitize("ar.alafasy-2"), "ar.alafasy-2");
        assert!(meaning_path("en.sahih").ends_with("quran/meaning-en.sahih.json"));
        assert!(text_path().ends_with("mandara/quran/quran.json"));
    }

    #[test]
    fn usage_counts_nested_files() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("a/b")).unwrap();
        std::fs::write(d.path().join("a/one.mp3"), [0u8; 10]).unwrap();
        std::fs::write(d.path().join("a/b/two.mp3"), [0u8; 5]).unwrap();
        assert_eq!(walk(d.path()), (2, 15));
        assert_eq!(walk(&d.path().join("missing")), (0, 0));
        let u = Usage { text: true, text_bytes: 3, meanings: vec![("en.sahih".into(), 4)], audio_files: 1, audio_bytes: 5 };
        assert_eq!(u.total(), 12);
    }
}
