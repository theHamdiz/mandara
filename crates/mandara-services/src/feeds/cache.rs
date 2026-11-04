use std::collections::BTreeSet;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::{Feed, Source, MAX_ENTRIES};

/// Parsed headlines plus read flags need at most a few MiB. A cache is
/// never allowed to become an unbounded input on the next launch.
const MAX_CACHE_BYTES: usize = 4 * 1024 * 1024;
/// Refresh and read actions merge under the same lock, so one cannot
/// overwrite the other's read flags inside the shell process.
static WRITES: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub feed: Feed,
    pub read: BTreeSet<String>,
    pub refreshed: i64,
    /// A refresh can fail while the previous headlines remain readable.
    #[serde(skip)]
    pub problem: Option<String>,
}

impl Snapshot {
    pub fn unread(&self) -> usize { self.feed.entries.iter().filter(|entry| !self.read.contains(&entry.link)).count() }
}

pub struct Cache { root: PathBuf }

impl Default for Cache {
    fn default() -> Self { Self::at(mandara_config::paths::state_dir().join("feeds")) }
}

impl Cache {
    pub fn at(root: PathBuf) -> Self { Self { root } }

    fn path(&self, url: &str) -> PathBuf { self.root.join(format!("{:x}.json", Sha256::digest(url))) }

    pub fn load(&self, url: &str) -> Result<Option<Snapshot>, String> {
        let path = self.path(url);
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(failed(error)),
        };
        let mut bytes = Vec::new();
        file.take((MAX_CACHE_BYTES + 1) as u64).read_to_end(&mut bytes).map_err(failed)?;
        if bytes.len() > MAX_CACHE_BYTES { return Err(t("The saved feed is too large to read").into()); }
        let mut saved: Snapshot = serde_json::from_slice(&bytes).map_err(|_| t("The saved feed could not be read; refresh it to download a new copy").to_string())?;
        if saved.feed.url != url { return Err(t("The saved feed belongs to another address").into()); }
        saved.feed.title = super::parse::title(&saved.feed.title);
        let mut seen = std::collections::HashSet::new();
        saved.feed.entries = saved.feed.entries.into_iter().filter_map(super::parse::bounded_entry)
            .filter(|entry| seen.insert(entry.link.clone())).take(MAX_ENTRIES).collect();
        saved.read.retain(|link| saved.feed.entries.iter().any(|entry| &entry.link == link));
        Ok(Some(saved))
    }

    pub fn refresh(&self, source: &impl Source, url: &str, previous: Option<&Snapshot>) -> Result<Snapshot, String> {
        let saved = self.load(url);
        let previous = previous.filter(|previous| previous.feed.url == url).cloned().or_else(|| saved.as_ref().ok().cloned().flatten());
        let result = source.fetch(url, previous.as_ref().map(|snapshot| &snapshot.feed));
        let feed = match result {
            Ok(Some(feed)) => feed,
            Ok(None) => previous.as_ref().map(|snapshot| snapshot.feed.clone()).ok_or_else(|| t("The feed server returned no content").to_string())?,
            Err(problem) => return previous.map(|mut snapshot| { snapshot.problem = Some(problem.clone()); snapshot }).ok_or(problem),
        };
        if feed.url != url { return Err(t("The feed belongs to another address").into()); }
        let _guard = WRITES.lock();
        let mut read = previous.map(|snapshot| snapshot.read).unwrap_or_default();
        if let Ok(Some(current)) = self.load(url) { read.extend(current.read); }
        read.retain(|link| feed.entries.iter().any(|entry| &entry.link == link));
        let mut snapshot = Snapshot { feed, read, refreshed: chrono::Utc::now().timestamp(), problem: None };
        if let Err(error) = self.save(&snapshot) { snapshot.problem = Some(error); }
        Ok(snapshot)
    }

    pub fn mark_read(&self, url: &str, link: &str) -> Result<Snapshot, String> {
        let _guard = WRITES.lock();
        let mut snapshot = self.load(url)?.ok_or_else(|| t("Refresh this feed before marking it read").to_string())?;
        if !snapshot.feed.entries.iter().any(|entry| entry.link == link) { return Err(t("This article is no longer in the feed").into()); }
        snapshot.read.insert(link.into());
        self.save(&snapshot)?;
        Ok(snapshot)
    }

    fn save(&self, snapshot: &Snapshot) -> Result<(), String> {
        let bytes = serde_json::to_vec(snapshot).map_err(failed)?;
        if bytes.len() > MAX_CACHE_BYTES { return Err(t("The saved feed is too large to write").into()); }
        std::fs::create_dir_all(&self.root).map_err(failed)?;
        save_private(&self.path(&snapshot.feed.url), &bytes).map_err(failed)
    }
}

fn failed(error: impl std::fmt::Display) -> String {
    tf("Could not save or read feed history: {error}", &[("error", &error)]).to_string()
}

fn save_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().expect("feed cache has a parent"))?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feeds::parse;
    use crate::feeds::web_url;

    struct Online;
    impl Source for Online {
        fn fetch(&self, url: &str, _: Option<&Feed>) -> Result<Option<Feed>, String> {
            parse(url, br#"{"version":"https://jsonfeed.org/version/1.1","title":"News","items":[{"id":"one","url":"https://example.org/one","title":"First","content_text":"Story"}]}"#).map(Some)
        }
    }
    struct Offline;
    impl Source for Offline {
        fn fetch(&self, _: &str, _: Option<&Feed>) -> Result<Option<Feed>, String> { Err("offline fixture".into()) }
    }

    #[test]
    fn read_history_survives_refresh_and_an_offline_restart() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().into());
        let url = "https://example.org/feed?private=subscription";
        assert_eq!(cache.refresh(&Online, url, None).unwrap().unread(), 1);
        cache.mark_read(url, "https://example.org/one").unwrap();
        assert_eq!(cache.refresh(&Online, url, None).unwrap().unread(), 0);
        let snapshot = cache.refresh(&Offline, url, None).unwrap();
        assert_eq!(snapshot.unread(), 0);
        assert_eq!(snapshot.problem.as_deref(), Some("offline fixture"));
        assert!(!cache.path(url).to_string_lossy().contains("private"));
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(cache.path(url)).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn a_refresh_repairs_corrupt_history_and_does_not_mark_unknown_articles() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().into());
        let url = "https://example.org/feed";
        std::fs::write(cache.path(url), "broken").unwrap();
        assert!(cache.load(url).is_err());
        assert!(cache.refresh(&Offline, url, None).is_err());
        cache.refresh(&Online, url, None).unwrap();
        assert!(cache.load(url).unwrap().is_some());
        assert!(cache.mark_read(url, "https://unrelated.example").is_err());
    }

    #[test]
    fn saved_content_has_the_same_link_and_count_limits_as_a_download() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().into());
        let url = "https://example.org/feed";
        let mut snapshot = cache.refresh(&Online, url, None).unwrap();
        let entry = snapshot.feed.entries[0].clone();
        snapshot.feed.entries = (0..MAX_ENTRIES + 10).map(|index| super::super::Entry { link: format!("https://example.org/{index}"), ..entry.clone() }).collect();
        snapshot.feed.entries[0].link = "file:///etc/passwd".into();
        snapshot.read.insert("file:///etc/passwd".into());
        cache.save(&snapshot).unwrap();
        let saved = cache.load(url).unwrap().unwrap();
        assert_eq!(saved.feed.entries.len(), MAX_ENTRIES);
        assert!(saved.feed.entries.iter().all(|entry| web_url(&entry.link)));
        assert!(!saved.read.contains("file:///etc/passwd"));
    }
}
