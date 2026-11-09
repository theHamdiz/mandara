//! Each subscription's local history: its headlines (merged across
//! downloads, bounded by count and age), read and starred flags, the
//! server's validators, its backoff after failures, and a separate,
//! bounded store of cleaned-up article content read one article at a time.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::html::Article;
use super::{Feed, Source, MAX_ENTRIES};

/// Parsed headlines plus read flags need at most a few MiB. A cache is
/// never allowed to become an unbounded input on the next launch.
const MAX_CACHE_BYTES: usize = 4 * 1024 * 1024;
/// Article content kept per feed; older articles open in the browser.
pub const MAX_STORE_BYTES: usize = 2 * 1024 * 1024;
/// Starred articles stay beyond the item limit, up to this many per feed.
pub const MAX_STARRED: usize = 200;
/// Backoff after a failed refresh: doubling from five minutes to six hours.
const BACKOFF_BASE_SECS: i64 = 300;
const BACKOFF_MAX_SECS: i64 = 6 * 3600;
/// The longest wait a server's Retry-After may impose.
const RETRY_AFTER_MAX_SECS: u64 = 24 * 3600;
const DAY_SECS: i64 = 86_400;
/// Refresh and flag actions merge under the same lock, so one cannot
/// overwrite the other's flags inside the shell process.
static WRITES: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// How much history a feed keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Entries per feed (starred ones come on top).
    pub items: usize,
    /// Days an entry stays after it has left the feed.
    pub retention_days: u32,
}

impl Default for Limits {
    fn default() -> Self { Self { items: 50, retention_days: 14 } }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub feed: Feed,
    pub read: BTreeSet<String>,
    #[serde(default)]
    pub starred: BTreeSet<String>,
    pub refreshed: i64,
    /// Failed refreshes in a row, and when the next attempt is due.
    #[serde(default)]
    pub failures: u32,
    #[serde(default)]
    pub retry_at: i64,
    /// The last refresh's error, kept until a refresh succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Unread entries the last download added (for notifications).
    #[serde(skip)]
    pub fresh: usize,
}

/// A feed's numbers for the bar and the sidebar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub total: usize,
    pub unread: usize,
    pub starred: usize,
    pub failing: bool,
}

impl Snapshot {
    fn empty(url: &str) -> Self { Self { feed: Feed { url: url.into(), ..Feed::default() }, ..Self::default() } }

    pub fn unread(&self) -> usize { self.feed.entries.iter().filter(|entry| !self.read.contains(&entry.link)).count() }

    pub fn counts(&self) -> Counts {
        Counts { total: self.feed.entries.len(), unread: self.unread(), starred: self.starred.len(), failing: self.error.is_some() }
    }

    /// Whether a refresh is waiting out a failure.
    pub fn backing_off(&self, now: i64) -> bool { self.retry_at > now }

    /// Keep flags for entries that still exist.
    fn tidy_flags(&mut self) {
        let links: HashSet<&str> = self.feed.entries.iter().map(|entry| entry.link.as_str()).collect();
        self.read.retain(|link| links.contains(link.as_str()));
        self.starred.retain(|link| links.contains(link.as_str()));
    }
}

/// Seconds to wait after the `failures`-th failure in a row.
pub(super) fn backoff(failures: u32, retry_after: Option<u64>) -> i64 {
    if let Some(seconds) = retry_after { return seconds.clamp(60, RETRY_AFTER_MAX_SECS) as i64; }
    BACKOFF_BASE_SECS.saturating_mul(1 << failures.saturating_sub(1).min(10)).min(BACKOFF_MAX_SECS)
}

/// Merge a download into the history: refreshed entries keep when they
/// were first seen, entries that left the feed stay for the retention
/// period, starred ones always; newest first, `limits.items` at most.
/// Returns the article content to store, newest first.
pub(super) fn merge(base: Snapshot, mut feed: Feed, limits: Limits, now: i64) -> (Snapshot, Vec<(String, Article)>) {
    let articles: Vec<(String, Article)> = feed.entries.iter().map(|entry| entry.link.clone()).zip(std::mem::take(&mut feed.articles)).collect();
    // A feed's first download is not news: nothing to notify about.
    let first = base.feed.entries.is_empty();
    let mut old: HashMap<String, super::Entry> = base.feed.entries.into_iter().map(|entry| (entry.link.clone(), entry)).collect();
    let mut fresh = 0;
    for entry in &mut feed.entries {
        match old.remove(&entry.link) {
            Some(previous) => if previous.seen > 0 { entry.seen = previous.seen; },
            None => if !first && !base.read.contains(&entry.link) { fresh += 1; },
        }
    }
    let cutoff = now - i64::from(limits.retention_days) * DAY_SECS;
    let mut kept: Vec<super::Entry> = old.into_values().filter(|entry| entry.when() >= cutoff || base.starred.contains(&entry.link)).collect();
    feed.entries.append(&mut kept);
    feed.entries.sort_by_key(|entry| std::cmp::Reverse(entry.when()));
    let (mut count, mut starred) = (0, 0);
    feed.entries.retain(|entry| if base.starred.contains(&entry.link) && starred < MAX_STARRED { starred += 1; true } else { count += 1; count <= limits.items.clamp(1, MAX_ENTRIES) });
    if feed.icon.is_none() { feed.icon = base.feed.icon; }
    if feed.site.is_none() { feed.site = base.feed.site; }
    let mut snapshot = Snapshot { feed, read: base.read, starred: base.starred, refreshed: now, failures: 0, retry_at: 0, error: None, fresh };
    snapshot.tidy_flags();
    (snapshot, articles)
}

pub struct Cache { root: PathBuf }

impl Default for Cache {
    fn default() -> Self { Self::at(mandara_config::paths::state_dir().join("feeds")) }
}

impl Cache {
    pub fn at(root: PathBuf) -> Self { Self { root } }

    fn path(&self, url: &str) -> PathBuf { self.root.join(format!("{:x}.json", Sha256::digest(url))) }

    fn store_path(&self, url: &str) -> PathBuf { self.root.join(format!("{:x}.articles.json", Sha256::digest(url))) }

    /// Read saved headlines defensively; a corrupt cache behaves like an empty feed.
    pub fn load(&self, url: &str) -> Result<Option<Snapshot>, String> {
        let Some(bytes) = read_bounded(&self.path(url), MAX_CACHE_BYTES).map_err(failed)? else { return Ok(None) };
        let mut saved: Snapshot = serde_json::from_slice(&bytes).map_err(|_| t("The saved feed could not be read; refresh it to download a new copy").to_string())?;
        drop(bytes);
        if saved.feed.url != url { return Err(t("The saved feed belongs to another address").into()); }
        saved.feed.title = super::parse::title(&saved.feed.title);
        let mut seen = HashSet::new();
        saved.feed.entries = saved.feed.entries.into_iter().filter_map(super::parse::bounded_entry)
            .filter(|entry| seen.insert(entry.link.clone())).take(MAX_ENTRIES + MAX_STARRED).collect();
        // Older caches did not record when an entry appeared.
        for entry in &mut saved.feed.entries { if entry.seen == 0 { entry.seen = entry.published.unwrap_or(saved.refreshed); } }
        saved.feed.icon = saved.feed.icon.filter(|icon| super::web_url(icon));
        saved.tidy_flags();
        Ok(Some(saved))
    }

    /// Refresh one feed: skip it while it backs off (unless `force`d), keep
    /// the history on HTTP 304 or an error, merge a download otherwise.
    /// Errors are recorded in the snapshot, with the next retry time.
    pub fn refresh(&self, source: &impl Source, url: &str, previous: Option<&Snapshot>, limits: Limits, force: bool) -> Result<Snapshot, String> {
        let now = chrono::Utc::now().timestamp();
        let saved = self.load(url).ok().flatten();
        let previous = saved.or_else(|| previous.filter(|previous| previous.feed.url == url).cloned());
        if !force && let Some(previous) = &previous && previous.backing_off(now) { return Ok(previous.clone()); }
        let result = source.fetch(url, previous.as_ref().map(|snapshot| &snapshot.feed));
        let _guard = WRITES.lock();
        let mut base = previous.unwrap_or_else(|| Snapshot::empty(url));
        // Flags set while the download ran are on disk; they win.
        if let Ok(Some(current)) = self.load(url) {
            base.read = current.read;
            base.starred = current.starred;
        }
        let snapshot = match result {
            Ok(Some(feed)) => {
                if feed.url != url { return Err(t("The feed belongs to another address").into()); }
                let (snapshot, articles) = merge(base, feed, limits, now);
                if let Err(error) = self.store_articles(&snapshot, articles) { tracing::debug!("feed articles: {error}"); }
                snapshot
            }
            Ok(None) => Snapshot { refreshed: now, failures: 0, retry_at: 0, error: None, fresh: 0, ..base },
            Err(failure) => {
                let failures = base.failures.saturating_add(1);
                Snapshot { failures, retry_at: now + backoff(failures, failure.retry_after), error: Some(failure.message), fresh: 0, ..base }
            }
        };
        self.save(&snapshot)?;
        Ok(snapshot)
    }

    /// Change flags under the write lock, only for articles in the feed.
    fn update(&self, url: &str, change: impl FnOnce(&mut Snapshot) -> Result<(), String>) -> Result<Snapshot, String> {
        let _guard = WRITES.lock();
        let mut snapshot = self.load(url)?.ok_or_else(|| t("Refresh this feed before marking it read").to_string())?;
        change(&mut snapshot)?;
        self.save(&snapshot)?;
        Ok(snapshot)
    }

    /// Persist a read flag only for an article in the current feed.
    pub fn mark_read(&self, url: &str, link: &str) -> Result<Snapshot, String> { self.set_read(url, &[link], true) }

    /// Mark articles read or unread.
    pub fn set_read(&self, url: &str, links: &[&str], read: bool) -> Result<Snapshot, String> {
        self.update(url, |snapshot| {
            let mut any = false;
            for link in links {
                if !snapshot.feed.entries.iter().any(|entry| entry.link == *link) { continue; }
                any = true;
                if read { snapshot.read.insert((*link).to_owned()); } else { snapshot.read.remove(*link); }
            }
            if any { Ok(()) } else { Err(t("This article is no longer in the feed").into()) }
        })
    }

    /// Mark every article of the feed read (optionally only those up to `until`).
    pub fn mark_all_read(&self, url: &str) -> Result<Snapshot, String> {
        self.update(url, |snapshot| {
            snapshot.read.extend(snapshot.feed.entries.iter().map(|entry| entry.link.clone()));
            Ok(())
        })
    }

    /// Star (save for later) or unstar an article.
    pub fn set_starred(&self, url: &str, link: &str, starred: bool) -> Result<Snapshot, String> {
        self.update(url, |snapshot| {
            if !snapshot.feed.entries.iter().any(|entry| entry.link == link) { return Err(t("This article is no longer in the feed").into()); }
            if starred {
                if snapshot.starred.len() >= MAX_STARRED { return Err(t("This feed already has 200 saved articles").into()); }
                snapshot.starred.insert(link.to_owned());
            } else {
                snapshot.starred.remove(link);
            }
            Ok(())
        })
    }

    /// One article's cleaned-up content, when it is still stored.
    pub fn article(&self, url: &str, link: &str) -> Result<Option<Article>, String> {
        let Some(bytes) = read_bounded(&self.store_path(url), MAX_STORE_BYTES * 2).map_err(failed)? else { return Ok(None) };
        let mut store: Vec<(String, Article)> = serde_json::from_slice(&bytes).unwrap_or_default();
        drop(bytes);
        Ok(store.iter().position(|(saved, _)| saved == link).map(|at| store.swap_remove(at).1))
    }

    /// Keep content for the snapshot's entries, newest (and starred)
    /// first, within [`MAX_STORE_BYTES`].
    fn store_articles(&self, snapshot: &Snapshot, fresh: Vec<(String, Article)>) -> Result<(), String> {
        let path = self.store_path(&snapshot.feed.url);
        let mut old: HashMap<String, Article> = match read_bounded(&path, MAX_STORE_BYTES * 2).map_err(failed)? {
            Some(bytes) => serde_json::from_slice::<Vec<(String, Article)>>(&bytes).unwrap_or_default().into_iter().collect(),
            None => HashMap::new(),
        };
        let mut fresh: HashMap<String, Article> = fresh.into_iter().collect();
        let order = snapshot.feed.entries.iter().filter(|entry| snapshot.starred.contains(&entry.link))
            .chain(snapshot.feed.entries.iter().filter(|entry| !snapshot.starred.contains(&entry.link)));
        let mut kept = Vec::new();
        let mut weight = 0;
        for entry in order {
            let Some(article) = fresh.remove(&entry.link).or_else(|| old.remove(&entry.link)) else { continue };
            if article.blocks.is_empty() { continue; }
            weight += article.weight() + entry.link.len();
            if weight > MAX_STORE_BYTES { break; }
            kept.push((entry.link.clone(), article));
        }
        drop((old, fresh));
        let bytes = serde_json::to_vec(&kept).map_err(failed)?;
        std::fs::create_dir_all(&self.root).map_err(failed)?;
        save_private(&path, &bytes).map_err(failed)
    }

    /// Save the feed and its read history with owner-only permissions.
    fn save(&self, snapshot: &Snapshot) -> Result<(), String> {
        let bytes = serde_json::to_vec(snapshot).map_err(failed)?;
        if bytes.len() > MAX_CACHE_BYTES { return Err(t("The saved feed is too large to write").into()); }
        std::fs::create_dir_all(&self.root).map_err(failed)?;
        save_private(&self.path(&snapshot.feed.url), &bytes).map_err(failed)
    }
}

/// A file's bytes up to `limit` (`None` when it does not exist).
fn read_bounded(path: &Path, limit: usize) -> std::io::Result<Option<Vec<u8>>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > limit { return Err(std::io::Error::other(t("The saved feed is too large to read"))); }
    Ok(Some(bytes))
}

fn failed(error: impl std::fmt::Display) -> String {
    tf("Could not save or read feed history: {error}", &[("error", &error)]).to_string()
}

/// Flush a private temporary file before atomically replacing the cached feed.
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
    use crate::feeds::{Entry, Failure, parse_full, web_url};

    struct Online(&'static str);
    impl Source for Online {
        fn fetch(&self, url: &str, _: Option<&Feed>) -> Result<Option<Feed>, Failure> {
            let mut parsed = parse_full(url, self.0.as_bytes(), None)?;
            parsed.feed.articles = parsed.articles;
            Ok(Some(parsed.feed))
        }
    }
    const ONE: Online = Online(r#"{"version":"https://jsonfeed.org/version/1.1","title":"News","items":[{"id":"one","url":"https://example.org/one","title":"First","content_html":"<p>Story <b>one</b></p><script>x()</script>"}]}"#);
    const TWO: Online = Online(r#"{"version":"https://jsonfeed.org/version/1.1","title":"News","items":[{"id":"two","url":"https://example.org/two","title":"Second","content_text":"Later"}]}"#);
    struct Offline;
    impl Source for Offline {
        fn fetch(&self, _: &str, _: Option<&Feed>) -> Result<Option<Feed>, Failure> { Err("offline fixture".into()) }
    }
    struct Busy;
    impl Source for Busy {
        fn fetch(&self, _: &str, _: Option<&Feed>) -> Result<Option<Feed>, Failure> { Err(Failure { message: "busy".into(), retry_after: Some(900) }) }
    }

    #[test]
    fn read_history_survives_refresh_and_an_offline_restart() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().into());
        let url = "https://example.org/feed?private=subscription";
        assert_eq!(cache.refresh(&ONE, url, None, Limits::default(), true).unwrap().unread(), 1);
        cache.mark_read(url, "https://example.org/one").unwrap();
        assert_eq!(cache.refresh(&ONE, url, None, Limits::default(), true).unwrap().unread(), 0);
        let snapshot = cache.refresh(&Offline, url, None, Limits::default(), true).unwrap();
        assert_eq!(snapshot.unread(), 0);
        assert_eq!(snapshot.feed.entries.len(), 1, "a failure keeps the headlines");
        assert_eq!(snapshot.error.as_deref(), Some("offline fixture"));
        assert!(!cache.path(url).to_string_lossy().contains("private"));
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(cache.path(url)).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(cache.store_path(url)).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn read_unread_and_starred_flags_persist_and_starred_outlive_the_feed() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().into());
        let url = "https://example.org/feed";
        cache.refresh(&ONE, url, None, Limits::default(), true).unwrap();
        cache.set_starred(url, "https://example.org/one", true).unwrap();
        cache.mark_all_read(url).unwrap();
        assert_eq!(cache.set_read(url, &["https://example.org/one"], false).unwrap().unread(), 1);
        // The article left the feed long ago: only the star keeps it.
        let limits = Limits { items: 1, retention_days: 0 };
        let mut stale = cache.load(url).unwrap().unwrap();
        stale.feed.entries[0].seen = 1;
        stale.feed.entries[0].published = None;
        cache.save(&stale).unwrap();
        let snapshot = cache.refresh(&TWO, url, None, limits, true).unwrap();
        assert_eq!(snapshot.feed.entries.iter().map(|e| e.link.as_str()).collect::<Vec<_>>(), ["https://example.org/two", "https://example.org/one"]);
        assert!(snapshot.starred.contains("https://example.org/one"));
        assert_eq!(snapshot.fresh, 1);
        cache.set_starred(url, "https://example.org/one", false).unwrap();
        let snapshot = cache.refresh(&TWO, url, None, limits, true).unwrap();
        assert_eq!(snapshot.feed.entries.len(), 1, "unstarred history expires");
        assert_eq!(snapshot.fresh, 0);
    }

    #[test]
    fn history_within_retention_stays_after_leaving_the_feed() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().into());
        let url = "https://example.org/feed";
        assert_eq!(cache.refresh(&ONE, url, None, Limits::default(), true).unwrap().fresh, 0, "a first download is not news");
        let snapshot = cache.refresh(&TWO, url, None, Limits::default(), true).unwrap();
        assert_eq!(snapshot.feed.entries.len(), 2);
        assert_eq!(snapshot.unread(), 2);
        assert_eq!(snapshot.fresh, 1);
    }

    #[test]
    fn articles_are_stored_sanitized_and_read_one_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().into());
        let url = "https://example.org/feed";
        cache.refresh(&ONE, url, None, Limits::default(), true).unwrap();
        let article = cache.article(url, "https://example.org/one").unwrap().unwrap();
        assert_eq!(article.blocks.len(), 1);
        let stored = std::fs::read_to_string(cache.store_path(url)).unwrap();
        assert!(!stored.contains("script") && !stored.contains("x()"), "{stored}");
        assert!(cache.article(url, "https://example.org/missing").unwrap().is_none());
        assert!(cache.article("https://other.example/feed", "x").unwrap().is_none());
    }

    #[test]
    fn failures_back_off_and_honour_retry_after() {
        assert_eq!(backoff(1, None), 300);
        assert_eq!(backoff(2, None), 600);
        assert_eq!(backoff(30, None), BACKOFF_MAX_SECS);
        assert_eq!(backoff(1, Some(5)), 60);
        assert_eq!(backoff(1, Some(u64::MAX)), RETRY_AFTER_MAX_SECS as i64);
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().into());
        let url = "https://example.org/feed";
        let first = cache.refresh(&Busy, url, None, Limits::default(), false).unwrap();
        assert_eq!(first.failures, 1);
        assert!(first.retry_at > chrono::Utc::now().timestamp() + 800);
        // Backing off: an unforced refresh does not contact the server.
        let waiting = cache.refresh(&ONE, url, None, Limits::default(), false).unwrap();
        assert!(waiting.feed.entries.is_empty() && waiting.failures == 1);
        let second = cache.refresh(&Offline, url, None, Limits::default(), true).unwrap();
        assert_eq!(second.failures, 2);
        let ok = cache.refresh(&ONE, url, None, Limits::default(), true).unwrap();
        assert_eq!((ok.failures, ok.retry_at, ok.error.as_deref()), (0, 0, None));
    }

    #[test]
    fn a_refresh_repairs_corrupt_history_and_does_not_mark_unknown_articles() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().into());
        let url = "https://example.org/feed";
        std::fs::write(cache.path(url), "broken").unwrap();
        assert!(cache.load(url).is_err());
        assert!(cache.refresh(&Offline, url, None, Limits::default(), true).unwrap().feed.entries.is_empty());
        cache.refresh(&ONE, url, None, Limits::default(), true).unwrap();
        assert!(cache.load(url).unwrap().is_some());
        assert!(cache.mark_read(url, "https://unrelated.example").is_err());
    }

    #[test]
    fn saved_content_has_the_same_link_and_count_limits_as_a_download() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().into());
        let url = "https://example.org/feed";
        let mut snapshot = cache.refresh(&ONE, url, None, Limits::default(), true).unwrap();
        let entry = snapshot.feed.entries[0].clone();
        snapshot.feed.entries = (0..MAX_ENTRIES + MAX_STARRED + 10).map(|index| Entry { link: format!("https://example.org/{index}"), ..entry.clone() }).collect();
        snapshot.feed.entries[0].link = "file:///etc/passwd".into();
        snapshot.read.insert("file:///etc/passwd".into());
        cache.save(&snapshot).unwrap();
        let saved = cache.load(url).unwrap().unwrap();
        assert_eq!(saved.feed.entries.len(), MAX_ENTRIES + MAX_STARRED);
        assert!(saved.feed.entries.iter().all(|entry| web_url(&entry.link)));
        assert!(!saved.read.contains("file:///etc/passwd"));
    }

    /// Caches written before history, stars and backoff still load.
    #[test]
    fn caches_from_the_previous_format_migrate() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().into());
        let url = "https://example.org/feed";
        std::fs::create_dir_all(dir.path()).unwrap();
        let old = serde_json::json!({"feed":{"url":url,"title":"Old","entries":[{"id":"a","title":"Kept","link":"https://example.org/a","published":1793786400},{"id":"b","title":"Undated","link":"https://example.org/b","published":null}],"etag":"\"v1\"","modified":null},"read":["https://example.org/a","https://example.org/gone"],"refreshed":1793790000});
        std::fs::write(cache.path(url), serde_json::to_vec(&old).unwrap()).unwrap();
        let saved = cache.load(url).unwrap().unwrap();
        assert_eq!(saved.feed.entries.len(), 2);
        assert_eq!(saved.unread(), 1);
        assert_eq!(saved.feed.etag.as_deref(), Some("\"v1\""));
        assert_eq!(saved.feed.entries[1].seen, 1_793_790_000, "undated entries date from the old refresh");
        assert!(saved.starred.is_empty() && saved.failures == 0);
    }
}
