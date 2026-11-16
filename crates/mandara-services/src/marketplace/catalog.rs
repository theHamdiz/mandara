//! Small released manifests for store presentation, cached independently of searches.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Listing, Results};

mod manifest;
mod budget;
pub(super) use budget::observe;

/// A manifest is text, not the plugin archive. Keep both remote and saved input bounded.
const MANIFEST_BYTES: u64 = 64 << 10;
const CACHE_BYTES: u64 = 8 << 20;
/// At most the search API's three pages; old entries leave first.
const CACHE_ENTRIES: usize = (super::SEARCH_PER_PAGE * super::SEARCH_MAX_PAGES) as usize;

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct Text {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Metadata {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub i18n: BTreeMap<String, Text>,
}

impl Metadata {
    pub fn text(&self, language: &str) -> (&str, &str) {
        let text = self.i18n.get(language);
        (
            text.map(|t| t.name.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(&self.name),
            text.map(|t| t.description.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(&self.description),
        )
    }
}

#[derive(Deserialize, Serialize)]
struct Entry {
    api: String,
    repo: String,
    repository_id: Option<u64>,
    publisher_id: Option<u64>,
    revision: String,
    metadata: Option<Metadata>,
    #[serde(skip)]
    bytes: usize,
}

impl Entry {
    fn encoded_len(&self) -> usize {
        serde_json::to_vec(self).map_or(CACHE_BYTES as usize, |bytes| bytes.len() + 1)
    }
    fn matches(&self, api: &str, listing: &Listing) -> bool {
        self.api == api
            && self.repo == listing.full_name
            && self.repository_id == listing.id
            && self.publisher_id == listing.owner.id
            && self.revision == listing.updated_at
    }
}

fn load(path: &Path) -> Vec<Entry> {
    use std::io::Read as _;
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut bytes = Vec::new();
    if file.take(CACHE_BYTES + 1).read_to_end(&mut bytes).is_err()
        || bytes.len() as u64 > CACHE_BYTES
    {
        return Vec::new();
    }
    let mut entries = serde_json::from_slice::<Vec<Entry>>(&bytes)
        .ok()
        .filter(|entries| entries.len() <= CACHE_ENTRIES)
        .unwrap_or_default();
    for entry in &mut entries {
        entry.bytes = entry.encoded_len();
    }
    entries
}

/// Small persistent scan position: failed lookups must not monopolize every allowance.
const RESUME_BYTES: u64 = 4 << 10;

#[derive(Default, Deserialize, Serialize)]
struct Resume {
    api: String,
    after: Option<String>,
}

impl Resume {
    fn load(path: &Path) -> Self {
        use std::io::Read as _;
        let Ok(file) = std::fs::File::open(path) else { return Self::default(); };
        let mut bytes = Vec::new();
        if file.take(RESUME_BYTES + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > RESUME_BYTES { return Self::default(); }
        serde_json::from_slice::<Self>(&bytes).ok()
            .filter(|cursor| cursor.after.as_ref().is_none_or(|repo| super::moderation::policy::valid_repository(repo)))
            .unwrap_or_default()
    }
    fn for_api(&mut self, api: &str) -> &mut Option<String> {
        if self.api != api { self.api = api.to_owned(); self.after = None; }
        &mut self.after
    }
    fn save(&self, path: &Path) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(self)?;
        anyhow::ensure!(bytes.len() as u64 <= RESUME_BYTES, "plugin catalog resume record is too large");
        mandara_config::store::write_atomic(path, &bytes)
    }
}

struct Cache {
    entries: Vec<Entry>,
    resume: Resume,
}

fn fetch(repo: &str) -> Result<Option<Metadata>, String> {
    if !super::moderation::policy::valid_repository(repo) {
        return Err(format!("invalid plugin repository: {repo}"));
    }
    let (release, archive) = super::find_release_with(repo, |url, limit| {
        budget::take().map_err(super::Fetch::Failed)?;
        super::get_media_timeout(url, limit, "application/vnd.github+json", budget::TIMEOUT)
    }).map_err(|error| error.to_string())?;
    manifest::fetch(&super::api_base(), repo, &release.tag_name)
        .or_else(|error| {
            if !budget::available() { return Err(error); }
            manifest::from_archive(repo, &archive.browser_download_url)
        })
        .map(Some)
}

fn enrich_after(
    results: &mut Results,
    entries: &mut Vec<Entry>,
    api: &str,
    mut fetch: impl FnMut(&str) -> Result<Option<Metadata>, String>,
    keep_going: impl Fn() -> bool,
    mut publish: impl FnMut(&Results),
    after: &mut Option<String>,
) -> bool {
    for listing in &mut results.items {
        if let Some(entry) = entries.iter().find(|e| e.matches(api, listing)) {
            listing.plugin = entry.metadata.clone();
        }
    }
    // Cached text and repository fallbacks are visible before the first HTTP request.
    publish(results);
    let mut changed = false;
    let count = results.items.len();
    let start = after.as_ref().and_then(|repo| results.items.iter().position(|l| l.full_name == *repo)).map_or(0, |index| (index + 1) % count);
    for offset in 0..count {
        let listing = &mut results.items[(start + offset) % count];
        if entries.iter().any(|e| e.matches(api, listing)) { continue; }
        if !keep_going() { break; }
        if !super::moderation::policy::valid_repository(&listing.full_name) { continue; }
        *after = Some(listing.full_name.clone());
        let metadata = match fetch(&listing.full_name) {
            Ok(metadata) => metadata,
            Err(error) => {
                tracing::debug!(repository = %listing.full_name, "plugin store metadata unavailable: {error}");
                continue;
            }
        };
        entries.retain(|e| e.api != api || e.repo != listing.full_name);
        let mut entry = Entry {
            api: api.into(),
            repo: listing.full_name.clone(),
            repository_id: listing.id,
            publisher_id: listing.owner.id,
            revision: listing.updated_at.clone(),
            metadata: metadata.clone(),
            bytes: 0,
        };
        entry.bytes = entry.encoded_len();
        entries.push(entry);
        let mut used: usize = entries.iter().map(|e| e.bytes).sum();
        while entries.len() > CACHE_ENTRIES || used > CACHE_BYTES as usize - 2 {
            used -= entries.remove(0).bytes;
        }
        listing.plugin = metadata;
        changed = true;
        publish(results);
    }
    changed
}

/// Called by the store's sole blocking worker, after moderation admits the listings.
pub fn enrich(results: &mut Results, keep_going: impl Fn() -> bool, publish: impl FnMut(&Results)) {
    static CACHE: parking_lot::Mutex<Option<Cache>> = parking_lot::Mutex::new(None);
    let dir = mandara_config::paths::cache_dir();
    let path = dir.join("marketplace-catalog.json");
    let resume_path = dir.join("marketplace-catalog-resume.json");
    let mut cache = CACHE.lock();
    let Cache { entries, resume } = cache.get_or_insert_with(|| Cache { entries: load(&path), resume: Resume::load(&resume_path) });
    let api = super::api_base();
    let before = resume.after.clone();
    let after = resume.for_api(&api);
    // One pass is optional work, with short individual requests and no hidden polling.
    const PASS_TIME: std::time::Duration = std::time::Duration::from_secs(20);
    let started = std::time::Instant::now();
    if enrich_after(results, entries, &api, fetch, || keep_going() && budget::available() && started.elapsed() < PASS_TIME, publish, after) {
        match serde_json::to_vec(entries)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| mandara_config::store::write_atomic(&path, &bytes))
        {
            Ok(()) => {}
            Err(error) => tracing::warn!("saving plugin catalog: {error:#}"),
        }
    }
    if resume.after != before && let Err(error) = resume.save(&resume_path) {
        tracing::warn!("saving plugin catalog scan position: {error:#}");
    }
}

#[cfg(test)]
fn enrich_stream(results: &mut Results, entries: &mut Vec<Entry>, api: &str,
    fetch: impl FnMut(&str) -> Result<Option<Metadata>, String>, keep_going: impl Fn() -> bool,
    publish: impl FnMut(&Results)) -> bool {
    enrich_after(results, entries, api, fetch, keep_going, publish, &mut None)
}

#[cfg(test)]
fn enrich_with(results: &mut Results, entries: &mut Vec<Entry>, api: &str,
    fetch: impl FnMut(&str) -> Result<Option<Metadata>, String>, keep_going: impl Fn() -> bool) -> bool {
    enrich_stream(results, entries, api, fetch, keep_going, |_| {})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_repositories_do_not_starve_later_releases_after_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resume.json");
        let mut entries = Vec::new();
        let mut cursor = Resume::default();
        let mut items: Vec<_> = (0..10).map(|n| listing(&format!("author/empty-{n}"), "a")).collect();
        items.push(listing("author/released", "a"));
        let metadata: Metadata = toml::from_str("id='released'\nname='Released Picker'").unwrap();
        let mut results = Results { items, total: 11 };
        let left = std::cell::Cell::new(20);
        enrich_after(&mut results, &mut entries, "api", |_| {
            left.set(left.get() - 2);
            Err("no release".into())
        }, || left.get() > 0, |_| {}, cursor.for_api("api"));
        assert_eq!(cursor.after.as_deref(), Some("author/empty-9"));
        cursor.save(&path).unwrap();
        let mut reopened = Resume::load(&path);
        let mut calls = Vec::new();
        left.set(20);
        enrich_after(&mut results, &mut entries, "api", |repo| {
            calls.push(repo.to_owned());
            left.set(left.get() - 2);
            if repo == "author/released" { Ok(Some(metadata.clone())) } else { Err("no release".into()) }
        }, || left.get() > 0, |_| {}, reopened.for_api("api"));
        assert_eq!(calls.first().map(String::as_str), Some("author/released"));
        assert!(results.items.last().unwrap().plugin.is_some());
        assert_eq!(reopened.for_api("different-api"), &None);
        std::fs::write(&path, vec![b'x'; RESUME_BYTES as usize + 1]).unwrap();
        assert!(Resume::load(&path).after.is_none());
    }

    #[test]
    fn repository_cards_are_published_before_fetching_and_new_metadata_streams_in() {
        let metadata: Metadata = toml::from_str("id='picker'\nname='Real Picker'").unwrap();
        let mut results = Results { items: vec![listing("a/one", "a"), listing("a/two", "a")], total: 2 };
        let fetched = std::cell::Cell::new(0);
        let publications = std::cell::RefCell::new(Vec::new());
        enrich_stream(&mut results, &mut Vec::new(), "api", |_| {
            assert!(!publications.borrow().is_empty(), "repository cards must precede optional network work");
            fetched.set(fetched.get() + 1);
            Ok(Some(metadata.clone()))
        }, || true, |results| {
            publications.borrow_mut().push((fetched.get(), results.items.iter().filter(|l| l.plugin.is_some()).count()));
        });
        assert_eq!(*publications.borrow(), [(0, 0), (1, 1), (2, 2)]);
        let mut cached = Vec::new();
        enrich_with(&mut results, &mut cached, "api", |_| Ok(Some(metadata.clone())), || true);
        enrich_stream(&mut results, &mut cached, "api", |_| panic!("cached metadata must not refetch"), || false, |results| assert!(results.items.iter().all(|l| l.plugin.is_some())));
    }

    #[test]
    fn manifest_is_read_from_the_release_ref_as_bounded_raw_toml() {
        use std::io::{BufRead as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                let n = reader.read_line(&mut line).unwrap();
                headers.push_str(&line);
                if n == 0 || line == "\r\n" {
                    break;
                }
            }
            let body = "id='gif-picker'\nname='GIF Picker'\ndescription='Search and copy GIF links.'\npermissions=['clipboard.write']\n";
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            headers
        });
        let metadata = manifest::fetch(&api, "author/one", "plugin/v0.1+build").unwrap();
        assert_eq!(metadata.name, "GIF Picker");
        let headers = worker.join().unwrap();
        assert!(
            headers.starts_with(
                "GET /repos/author/one/contents/plugin.toml?ref=plugin%2Fv0.1%2Bbuild "
            ),
            "{headers}"
        );
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("accept: application/vnd.github.raw+json")
        );
    }

    #[test]
    fn leaving_the_store_stops_before_fetching_another_entry() {
        let mut entries = Vec::new();
        let mut results = Results {
            items: vec![listing("author/one", "a"), listing("author/two", "a")],
            total: 2,
        };
        let calls = std::cell::Cell::new(0);
        enrich_with(
            &mut results,
            &mut entries,
            "api",
            |_| {
                calls.set(calls.get() + 1);
                Ok(None)
            },
            || calls.get() == 0,
        );
        assert_eq!(calls.get(), 1);
        assert_eq!(entries.len(), 1);
    }

    fn listing(repo: &str, revision: &str) -> Listing {
        serde_json::from_value(serde_json::json!({"id": 1, "full_name": repo, "name": "mandara-repo", "html_url": "https://example.invalid", "owner": {"id": 2, "login": "author"}, "updated_at": revision})).unwrap()
    }

    #[test]
    fn catalog_reuses_unchanged_entries_and_fetches_only_new_or_changed_sources() {
        let mut cache = Vec::new();
        let mut calls = Vec::new();
        let mut results = Results {
            items: vec![listing("author/one", "a")],
            total: 1,
        };
        let metadata: Metadata = toml::from_str("id='picker'\nname='GIF Picker'\ndescription='Search and copy GIF links.'\n[i18n.pt]\nname='Seletor de GIFs'\n").unwrap();
        let mut fetch = |repo: &str| {
            calls.push(repo.to_owned());
            Ok(Some(metadata.clone()))
        };
        assert!(enrich_with(
            &mut results,
            &mut cache,
            "api",
            &mut fetch,
            || true
        ));
        assert!(!enrich_with(
            &mut results,
            &mut cache,
            "api",
            &mut fetch,
            || true
        ));
        results.items.push(listing("author/two", "b"));
        assert!(enrich_with(
            &mut results,
            &mut cache,
            "api",
            &mut fetch,
            || true
        ));
        results.items[0].owner.id = Some(3);
        assert!(enrich_with(
            &mut results,
            &mut cache,
            "api",
            &mut fetch,
            || true
        ));
        assert_eq!(calls, ["author/one", "author/two", "author/one"]);
        assert_eq!(
            results.items[0].plugin.as_ref().unwrap().text("pt"),
            ("Seletor de GIFs", "Search and copy GIF links.")
        );
        assert_eq!(
            results.items[0].plugin.as_ref().unwrap().text("ar"),
            ("GIF Picker", "Search and copy GIF links.")
        );
    }

    #[test]
    fn cache_round_trips_and_ignores_oversized_input() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("catalog.json");
        let mut entries = Vec::new();
        let mut results = Results {
            items: vec![listing("author/one", "a")],
            total: 1,
        };
        enrich_with(&mut results, &mut entries, "api", |_| Ok(None), || true);
        std::fs::write(&file, serde_json::to_vec(&entries).unwrap()).unwrap();
        assert_eq!(load(&file).len(), 1);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&file)
            .unwrap()
            .set_len(CACHE_BYTES + 1)
            .unwrap();
        assert!(load(&file).is_empty());
    }

    #[test]
    fn transient_errors_are_retried_instead_of_becoming_permanent_missing_metadata() {
        let mut entries = Vec::new();
        let mut results = Results {
            items: vec![listing("author/one", "a")],
            total: 1,
        };
        assert!(!enrich_with(
            &mut results,
            &mut entries,
            "api",
            |_| Err("temporary failure".into()),
            || true
        ));
        assert!(entries.is_empty());
        let metadata = Metadata {
            id: "picker".into(),
            name: "Picker".into(),
            description: String::new(),
            i18n: BTreeMap::new(),
        };
        assert!(enrich_with(
            &mut results,
            &mut entries,
            "api",
            |_| Ok(Some(metadata.clone())),
            || true
        ));
        assert_eq!(results.items[0].plugin, Some(metadata));
    }

    #[test]
    fn large_catalogs_keep_the_saved_cache_within_its_read_budget() {
        let text = Text {
            name: "Picker".into(),
            description: "x".repeat(4096),
        };
        let metadata = Metadata {
            id: "picker".into(),
            name: "Picker".into(),
            description: String::new(),
            i18n: (0..16)
                .map(|i| (format!("lang-{i}"), text.clone()))
                .collect(),
        };
        let mut results = Results {
            items: (0..140)
                .map(|i| listing(&format!("author/picker-{i}"), "a"))
                .collect(),
            total: 140,
        };
        let mut entries = Vec::new();
        enrich_with(
            &mut results,
            &mut entries,
            "api",
            |_| Ok(Some(metadata.clone())),
            || true,
        );
        let bytes = serde_json::to_vec(&entries).unwrap();
        assert!(bytes.len() as u64 <= CACHE_BYTES);
        assert!(
            entries.len() < results.items.len(),
            "evict older entries before writing an unreadable cache"
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(load(&path).len(), entries.len());
    }
}
