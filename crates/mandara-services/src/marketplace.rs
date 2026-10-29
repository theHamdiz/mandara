//! The plugin marketplace: free, on GitHub. A plugin is a public repository
//! tagged with the `mandara-plugin` topic whose releases carry a
//! `<id>.mandara.tar.gz` asset (plugin.toml, plugin.wasm, optional
//! locales/, icon.png, cover.png). Studio searches the topic, shows the
//! results and installs a release into `~/.local/share/mandara/plugins/<id>`.
//! See PLUGINS.md for the publishing guide.
//!
//! Nothing but the topic decides whether a repository is listed: forks and
//! archived repositories are included, and a repository without a usable
//! release is still listed (installing it reports why it can't be). The
//! API base can be pointed elsewhere with `MANDARA_MARKETPLACE_API` (tests,
//! mirrors, GitHub Enterprise).

use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mandara_i18n::{t, tf};
use parking_lot::Mutex;
use serde::Deserialize;
use ureq::http::StatusCode;

pub const TOPIC: &str = "mandara-plugin";
pub const ASSET_SUFFIX: &str = ".mandara.tar.gz";
/// Largest archive accepted (and largest unpacked total).
pub const MAX_BYTES: u64 = 64 << 20;
/// Environment variable overriding the API base (no trailing slash needed).
pub const API_ENV: &str = "MANDARA_MARKETPLACE_API";
/// GitHub's REST API.
pub const DEFAULT_API: &str = "https://api.github.com";
/// Results per search page (GitHub's maximum).
pub const SEARCH_PER_PAGE: u32 = 100;
/// Search pages fetched at most per query. Anonymous search allows ten
/// requests a minute, so a full query stays well under it; beyond this the
/// page asks for a narrower search.
pub const SEARCH_MAX_PAGES: u32 = 3;
/// How long a search answer is reused (typing the same query again, page
/// switches, reopening Studio) before GitHub is asked again.
pub const SEARCH_CACHE_TTL: Duration = Duration::from_secs(60);
/// Distinct queries kept in the search cache.
const SEARCH_CACHE_ENTRIES: usize = 32;
/// Releases inspected when the latest one has no plugin archive (covers
/// pre-releases and a newer release without the asset).
const RELEASES_PER_PAGE: u32 = 20;
/// Largest JSON answer read from the API.
const MAX_JSON_BYTES: u64 = 4 << 20;
/// Whole-request timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Shortest wait reported after a rate limit.
const MIN_RETRY_SECS: u64 = 1;
/// Prefixes / suffixes authors put around the plugin id in repository names.
const REPO_AFFIXES: [&str; 2] = ["mandara-", "-mandara"];

/// A plugin listed in the marketplace.
#[derive(Clone, Debug, PartialEq, Deserialize, serde::Serialize)]
pub struct Listing {
    /// `owner/repo`.
    pub full_name: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub html_url: String,
    #[serde(default)]
    pub stargazers_count: u32,
    #[serde(default)]
    pub topics: Vec<String>,
    pub owner: Owner,
    #[serde(default)]
    pub updated_at: String,
    /// Read-only on GitHub: still installable, no longer maintained.
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub fork: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize, serde::Serialize)]
pub struct Owner {
    pub login: String,
    #[serde(default)]
    pub avatar_url: String,
}

/// One marketplace search.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Results {
    pub items: Vec<Listing>,
    /// Matching repositories on GitHub (may exceed `items`).
    pub total: u64,
}

impl Results {
    /// More repositories match than were fetched (narrow the search).
    pub fn truncated(&self) -> bool {
        (self.items.len() as u64) < self.total
    }
}

#[derive(Deserialize)]
struct Search {
    #[serde(default)]
    total_count: u64,
    items: Vec<Listing>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct Release {
    pub(crate) tag_name: String,
    #[serde(default)]
    draft: bool,
    /// The release page (plugin update notes).
    #[serde(default)]
    pub(crate) html_url: String,
    #[serde(default)]
    pub(crate) assets: Vec<Asset>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct Asset {
    pub(crate) name: String,
    pub(crate) browser_download_url: String,
    pub(crate) size: u64,
}

/// Why an install didn't happen.
#[derive(Clone, Debug, PartialEq)]
pub enum InstallError {
    /// The repository has no published release.
    NoRelease,
    /// Releases exist but none carries a `*.mandara.tar.gz` asset.
    NoAsset,
    /// The release failed the compliance check (`mandara_plugins::vet`);
    /// it is flagged and installs only when forced.
    Rejected { tag: String, class: Class, reason: String },
    /// Anything else (network, rate limit, bad archive…), translated.
    Failed(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoRelease => f.write_str(t("This plugin has no published release yet")),
            Self::NoAsset => f.write_str(&tf("The release has no {suffix} file", &[("suffix", &ASSET_SUFFIX)])),
            Self::Rejected { tag, reason, .. } => f.write_str(&tf("Version {version} isn't recommended: {reason}", &[("version", tag), ("reason", reason)])),
            Self::Failed(e) => f.write_str(e),
        }
    }
}

impl InstallError {
    /// A short label for a badge (the full text is [`Display`]).
    pub fn short(&self) -> String {
        match self {
            Self::NoRelease => t("No release yet").into(),
            Self::NoAsset => t("No plugin file yet").into(),
            Self::Rejected { class, .. } => class.label().into(),
            Self::Failed(e) => e.clone(),
        }
    }
}

// ── Compliance flags ─────────────────────────────────────────────────────

/// Why a release failed the compliance check.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    /// Impersonates a built-in, reaches outside the sandbox or acts unasked.
    Malicious,
    /// Shows nothing, or is the unchanged example or template.
    Placeholder,
    /// Doesn't load, crashes or runs past its time budget.
    Broken,
}

impl Class {
    pub fn label(self) -> &'static str {
        match self {
            Self::Malicious => t("Malicious"),
            Self::Placeholder => t("Placeholder"),
            Self::Broken => t("Broken"),
        }
    }
}

/// A failed check: its class and a translated reason.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, Deserialize)]
pub struct Finding {
    pub class: Class,
    pub reason: String,
}

impl Finding {
    pub fn new(class: Class, reason: impl Into<String>) -> Self {
        Self { class, reason: reason.into() }
    }
}

/// A repository's flagged release. Only that tag carries the warning: the
/// next release is checked afresh and clears or replaces it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, Deserialize)]
pub struct Flag {
    pub tag: String,
    pub class: Class,
    pub reason: String,
}

impl Flag {
    /// "This version (v1.2) isn't recommended: Broken: …".
    pub fn warning(&self) -> String {
        tf("This version ({version}) isn't recommended: {class}: {reason}", &[("version", &self.tag), ("class", &self.class.label()), ("reason", &self.reason)])
    }

    /// "Version v1.2 isn't recommended: …", for cards that already show the
    /// class as a badge.
    pub fn short_warning(&self) -> String {
        tf("Version {version} isn't recommended: {reason}", &[("version", &self.tag), ("reason", &self.reason)])
    }
}

/// The flag file, beside the installed plugins (a file, so discovery skips it).
const FLAGS_FILE: &str = ".flags.toml";

fn flags_path() -> PathBuf {
    install_dir().join(FLAGS_FILE)
}

fn flags_at(path: &Path) -> std::collections::BTreeMap<String, Flag> {
    std::fs::read_to_string(path).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
}

/// Every flagged release: repository → flag.
pub fn flags() -> std::collections::BTreeMap<String, Flag> {
    flags_at(&flags_path())
}

/// `repo`'s flag, if its flagged release is `tag`.
pub fn flag_for(repo: &str, tag: &str) -> Option<Flag> {
    flags().remove(repo).filter(|f| f.tag == tag)
}

/// Records the check of `repo` at `tag` in the flag file at `path`: a pass
/// clears the repository's flag, a failure replaces it with this release's.
fn record(path: &Path, repo: &str, tag: &str, verdict: &Result<(), Finding>) {
    let mut all = flags_at(path);
    let flag = verdict.as_ref().err().map(|f| Flag { tag: tag.to_string(), class: f.class, reason: f.reason.clone() });
    if all.get(repo) == flag.as_ref() {
        return;
    }
    match flag {
        Some(flag) => all.insert(repo.to_string(), flag),
        None => all.remove(repo),
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(path, toml::to_string(&all).unwrap_or_default()) {
        tracing::warn!("{}: {e}", path.display());
    }
}

/// How an install or update treats the compliance check.
pub struct Vetting<'a> {
    /// Checks an unpacked plugin folder of a repository
    /// (`mandara_plugins::vet::vet`, which needs the built-in ids).
    pub check: &'a dyn Fn(&Path, &str) -> Result<(), Finding>,
    /// Install a failing release anyway (its flag stays).
    pub force: bool,
}

impl Vetting<'_> {
    /// Checks `dir` (release `tag` of `repo`), records the verdict and
    /// refuses a failing release unless forced.
    pub fn admit(&self, repo: &str, tag: &str, dir: &Path) -> Result<(), InstallError> {
        self.admit_at(&flags_path(), repo, tag, dir)
    }

    fn admit_at(&self, flags: &Path, repo: &str, tag: &str, dir: &Path) -> Result<(), InstallError> {
        let verdict = (self.check)(dir, repo);
        record(flags, repo, tag, &verdict);
        match verdict {
            Err(f) if !self.force => Err(InstallError::Rejected { tag: tag.to_string(), class: f.class, reason: f.reason }),
            Err(f) => {
                tracing::warn!("{repo} {tag} installed anyway: {}", f.reason);
                Ok(())
            }
            Ok(()) => Ok(()),
        }
    }
}

impl From<String> for InstallError {
    fn from(e: String) -> Self {
        Self::Failed(e)
    }
}

/// A request that didn't produce a body.
enum Fetch {
    NotFound,
    Failed(String),
}

impl From<Fetch> for String {
    fn from(f: Fetch) -> Self {
        match f {
            Fetch::NotFound => tf("Couldn't reach GitHub: {error}", &[("error", &StatusCode::NOT_FOUND)]),
            Fetch::Failed(e) => e,
        }
    }
}

/// The API base: `MANDARA_MARKETPLACE_API` or GitHub's.
pub fn api_base() -> String {
    std::env::var(API_ENV).ok().map(|v| v.trim().trim_end_matches('/').to_string()).filter(|v| v.starts_with("https://") || v.starts_with("http://")).unwrap_or_else(|| DEFAULT_API.to_string())
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(REQUEST_TIMEOUT)).http_status_as_error(false).build().into()
}

fn header_u64(r: &ureq::http::Response<ureq::Body>, name: &str) -> Option<u64> {
    r.headers().get(name)?.to_str().ok()?.trim().parse().ok()
}

/// Seconds until GitHub accepts requests again, if `r` is a rate limit
/// (primary: 403/429 with `x-ratelimit-remaining: 0`; secondary:
/// `retry-after`).
pub(crate) fn rate_limit_wait(r: &ureq::http::Response<ureq::Body>) -> Option<u64> {
    let status = r.status();
    if status != StatusCode::FORBIDDEN && status != StatusCode::TOO_MANY_REQUESTS {
        return None;
    }
    if let Some(after) = header_u64(r, "retry-after") {
        return Some(after.max(MIN_RETRY_SECS));
    }
    let exhausted = header_u64(r, "x-ratelimit-remaining") == Some(0);
    if !exhausted && status == StatusCode::FORBIDDEN {
        return None;
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default();
    Some(header_u64(r, "x-ratelimit-reset").map_or(SEARCH_CACHE_TTL.as_secs(), |reset| reset.saturating_sub(now)).max(MIN_RETRY_SECS))
}

fn get(url: &str, limit: u64) -> Result<Vec<u8>, Fetch> {
    crate::offline::check().map_err(Fetch::Failed)?;
    let reach = |e: &dyn std::fmt::Display| Fetch::Failed(tf("Couldn't reach GitHub: {error}", &[("error", &e)]));
    let mut r = agent()
        .get(url)
        .header("User-Agent", concat!("mandara-shell/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| reach(&e))?;
    if let Some(seconds) = rate_limit_wait(&r) {
        return Err(Fetch::Failed(tf("GitHub's limit for anonymous requests was reached. Try again in {seconds} s.", &[("seconds", &seconds)])));
    }
    match r.status() {
        StatusCode::NOT_FOUND => return Err(Fetch::NotFound),
        s if !s.is_success() => return Err(reach(&s)),
        _ => {}
    }
    let mut out = Vec::new();
    r.body_mut().with_config().limit(limit).reader().read_to_end(&mut out).map_err(|e| reach(&e))?;
    Ok(out)
}

/// The search terms, reduced to safe words joined by `+`.
fn search_terms(query: &str) -> String {
    query.split_whitespace().map(|w| w.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect::<String>()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join("+")
}

/// Search URL for one page: the topic (forks included), an optional query,
/// most-starred first.
pub fn search_url(query: &str, page: u32) -> String {
    let q = search_terms(query);
    let filters = format!("topic:{TOPIC}+fork:true");
    let q = if q.is_empty() { filters } else { format!("{q}+{filters}") };
    format!("{}/search/repositories?q={q}&sort=stars&order=desc&per_page={SEARCH_PER_PAGE}&page={page}", api_base())
}

static SEARCH_CACHE: Mutex<Vec<(String, Instant, Results)>> = Mutex::new(Vec::new());

/// Plugins on the marketplace matching `query` (empty = all), up to
/// [`SEARCH_MAX_PAGES`] pages. Answers are cached for [`SEARCH_CACHE_TTL`].
pub fn search(query: &str) -> Result<Results, String> {
    crate::offline::check()?;
    let key = search_url(query, 1);
    if let Some((_, _, cached)) = SEARCH_CACHE.lock().iter().find(|(k, at, _)| *k == key && at.elapsed() < SEARCH_CACHE_TTL) {
        return Ok(cached.clone());
    }
    let mut results = Results::default();
    for page in 1..=SEARCH_MAX_PAGES {
        let body = get(&search_url(query, page), MAX_JSON_BYTES)?;
        let s: Search = serde_json::from_slice(&body).map_err(|e| tf("Unexpected answer from GitHub: {error}", &[("error", &e)]))?;
        results.total = s.total_count.max(results.total);
        let fetched = s.items.len();
        for item in s.items {
            if !results.items.iter().any(|l| l.full_name == item.full_name) {
                results.items.push(item);
            }
        }
        if fetched < SEARCH_PER_PAGE as usize || results.items.len() as u64 >= results.total {
            break;
        }
    }
    results.total = results.total.max(results.items.len() as u64);
    let mut cache = SEARCH_CACHE.lock();
    cache.retain(|(k, at, _)| *k != key && at.elapsed() < SEARCH_CACHE_TTL);
    if cache.len() >= SEARCH_CACHE_ENTRIES {
        cache.remove(0);
    }
    cache.push((key, Instant::now(), results.clone()));
    Ok(results)
}

/// Forgets cached searches (the next search asks GitHub again).
pub fn clear_cache() {
    SEARCH_CACHE.lock().clear();
}

/// Where marketplace plugins are installed.
pub fn install_dir() -> PathBuf {
    mandara_config::paths::data_dir().join("plugins")
}

/// Every installed marketplace plugin: repository → (folder, release tag),
/// read in one pass (for listing many results without a scan per row).
pub fn installed_all() -> std::collections::HashMap<String, (PathBuf, String)> {
    let Ok(entries) = std::fs::read_dir(install_dir()) else { return Default::default() };
    entries
        .flatten()
        .filter_map(|e| {
            let dir = e.path();
            let src = std::fs::read_to_string(dir.join(".source")).ok()?;
            let mut lines = src.lines();
            let repo = lines.next()?.to_string();
            let tag = lines.next().unwrap_or_default().to_string();
            Some((repo, (dir, tag)))
        })
        .collect()
}

/// The installed copy of `full_name`, if any (matched by its `.source`).
pub fn installed(full_name: &str) -> Option<(PathBuf, String)> {
    let entries = std::fs::read_dir(install_dir()).ok()?;
    entries.flatten().map(|e| e.path()).find_map(|dir| {
        let src = std::fs::read_to_string(dir.join(".source")).ok()?;
        let mut lines = src.lines();
        (lines.next()? == full_name).then(|| (dir.clone(), lines.next().unwrap_or_default().to_string()))
    })
}

#[derive(Deserialize)]
struct ManifestId {
    id: String,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Unpacks a plugin archive safely into a temporary folder (no absolute
/// paths, `..`, links or devices; size-capped) and returns it with the id.
pub(crate) fn unpack(bytes: &[u8], scratch: &Path) -> Result<(PathBuf, String), String> {
    let _ = std::fs::remove_dir_all(scratch);
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let mut total = 0u64;
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            continue;
        }
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        // Archives may wrap everything in one top folder; strip it below.
        if path.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
            return Err(t("The plugin archive contains unsafe paths").to_string());
        }
        total += entry.header().size().unwrap_or(0);
        if total > MAX_BYTES {
            return Err(t("The plugin archive is too large").to_string());
        }
        let dest = scratch.join(&path);
        if kind.is_dir() {
            std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            entry.unpack(&dest).map_err(|e| e.to_string())?;
        }
    }
    // plugin.toml at the root or inside exactly one top-level folder (other
    // folders without one, such as `__MACOSX`, are ignored).
    let root = if scratch.join("plugin.toml").is_file() {
        scratch.to_path_buf()
    } else {
        let dirs: Vec<PathBuf> = std::fs::read_dir(scratch).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|p| p.is_dir() && p.join("plugin.toml").is_file()).collect();
        match dirs.as_slice() {
            [one] => one.clone(),
            _ => return Err(t("The plugin archive has no plugin.toml").to_string()),
        }
    };
    let text = std::fs::read_to_string(root.join("plugin.toml")).map_err(|e| e.to_string())?;
    let m: ManifestId = toml::from_str(&text).map_err(|e| tf("Invalid plugin.toml: {error}", &[("error", &e)]))?;
    if !valid_id(&m.id) {
        return Err(tf("Invalid plugin id {id}", &[("id", &m.id)]));
    }
    Ok((root, m.id))
}

/// The release's plugin archive: any asset ending in [`ASSET_SUFFIX`]
/// (case-insensitive); with several, the one named after the repository.
fn plugin_asset<'a>(assets: &'a [Asset], full_name: &str) -> Option<&'a Asset> {
    let stem = |a: &Asset| {
        let lower = a.name.to_ascii_lowercase();
        lower.strip_suffix(ASSET_SUFFIX).map(str::to_string)
    };
    let matching: Vec<&Asset> = assets.iter().filter(|a| stem(a).is_some()).collect();
    let repo = full_name.rsplit('/').next().unwrap_or_default().to_ascii_lowercase();
    let mut wanted = vec![repo.clone()];
    wanted.extend(REPO_AFFIXES.iter().filter_map(|a| repo.strip_prefix(a).or_else(|| repo.strip_suffix(a)).map(str::to_string)));
    matching.iter().find(|a| stem(a).is_some_and(|s| wanted.contains(&s))).or(matching.first()).copied()
}

/// The newest release carrying a plugin archive: `releases/latest` first,
/// then the release list (pre-releases count; drafts don't).
pub(crate) fn find_release(full_name: &str) -> Result<(Release, Asset), InstallError> {
    let base = api_base();
    match get(&format!("{base}/repos/{full_name}/releases/latest"), MAX_JSON_BYTES) {
        Ok(body) => {
            let latest: Release = serde_json::from_slice(&body).map_err(|e| tf("Unexpected answer from GitHub: {error}", &[("error", &e)]))?;
            if let Some(asset) = plugin_asset(&latest.assets, full_name) {
                return Ok((latest.clone(), asset.clone()));
            }
        }
        Err(Fetch::NotFound) => {}
        Err(Fetch::Failed(e)) => return Err(InstallError::Failed(e)),
    }
    let body = get(&format!("{base}/repos/{full_name}/releases?per_page={RELEASES_PER_PAGE}"), MAX_JSON_BYTES).map_err(|f| match f {
        Fetch::NotFound => InstallError::NoRelease,
        Fetch::Failed(e) => InstallError::Failed(e),
    })?;
    let releases: Vec<Release> = serde_json::from_slice(&body).map_err(|e| tf("Unexpected answer from GitHub: {error}", &[("error", &e)]))?;
    let published: Vec<&Release> = releases.iter().filter(|r| !r.draft).collect();
    if published.is_empty() {
        return Err(InstallError::NoRelease);
    }
    published.iter().find_map(|r| plugin_asset(&r.assets, full_name).map(|a| ((*r).clone(), a.clone()))).ok_or(InstallError::NoAsset)
}

/// Installs (or updates) the newest release of `full_name` that carries a
/// plugin archive, after `vetting` admits it. Returns the plugin id and
/// version tag. Enabling it is the caller's job (Studio adds it to
/// `plugins.enabled`).
pub fn install(full_name: &str, vetting: &Vetting, progress: &dyn Fn(&str)) -> Result<(String, String), InstallError> {
    if !full_name.split('/').all(|p| !p.is_empty() && p.chars().all(|c| c.is_alphanumeric() || "-_.".contains(c))) || full_name.matches('/').count() != 1 {
        return Err(InstallError::Failed(tf("Invalid repository {name}", &[("name", &full_name)])));
    }
    progress(t("Finding the latest release…"));
    let (release, asset) = find_release(full_name)?;
    if asset.size > MAX_BYTES {
        return Err(InstallError::Failed(t("The plugin archive is too large").to_string()));
    }
    progress(t("Downloading…"));
    let bytes = get(&asset.browser_download_url, MAX_BYTES).map_err(String::from)?;
    progress(t("Installing…"));
    let scratch = mandara_config::paths::cache_dir().join("marketplace").join(full_name.replace('/', "__"));
    let (root, id) = unpack(&bytes, &scratch)?;
    if asset.name.to_ascii_lowercase() != format!("{id}{ASSET_SUFFIX}") {
        tracing::info!("{full_name}: asset {} holds plugin `{id}` (the id comes from plugin.toml)", asset.name);
    }
    progress(t("Checking the plugin…"));
    if let Err(e) = vetting.admit(full_name, &release.tag_name, &root) {
        let _ = std::fs::remove_dir_all(&scratch);
        return Err(e);
    }
    let dest = install_dir().join(&id);
    if let Some((existing, _)) = installed(full_name) {
        if existing != dest {
            let _ = std::fs::remove_dir_all(&existing);
        }
    }
    let staging = install_dir().join(format!(".{id}.new"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(install_dir()).map_err(|e| e.to_string())?;
    std::fs::rename(&root, &staging).or_else(|_| copy_tree(&root, &staging)).map_err(|e| e.to_string())?;
    std::fs::write(staging.join(".source"), format!("{full_name}\n{}\n", release.tag_name)).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&staging, &dest).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&scratch);
    Ok((id, release.tag_name))
}

pub(crate) fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let dest = to.join(e.file_name());
        if e.file_type()?.is_dir() { copy_tree(&e.path(), &dest)? } else { std::fs::copy(e.path(), dest).map(|_| ())? }
    }
    Ok(())
}

/// Removes an installed marketplace plugin.
pub fn uninstall(full_name: &str) -> Result<String, String> {
    let (dir, _) = installed(full_name).ok_or_else(|| t("Not installed").to_string())?;
    let id = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (path, data) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            builder.append_data(&mut h, path, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn unpacks_wrapped_archives_and_reads_the_id() {
        let dir = std::env::temp_dir().join(format!("mandara-mkt-{}", std::process::id()));
        let bytes = archive(&[("moon/plugin.toml", b"id = \"moon-phase\"\nname = \"Moon\"\n"), ("moon/plugin.wasm", b"\0asm"), ("moon/locales/ar.toml", b"\"Moon\" = \"\\u0642\\u0645\\u0631\"")]);
        let (root, id) = unpack(&bytes, &dir).unwrap();
        assert_eq!(id, "moon-phase");
        assert!(root.join("plugin.wasm").is_file() && root.join("locales/ar.toml").is_file());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn keeps_icon_and_cover_art() {
        let dir = std::env::temp_dir().join(format!("mandara-mkt-art-{}", std::process::id()));
        let png = b"\x89PNG\r\n\x1a\n";
        let bytes = archive(&[("moon/plugin.toml", b"id = \"moon-phase\"\nname = \"Moon\"\n"), ("moon/plugin.wasm", b"\0asm"), ("moon/icon.png", png), ("moon/cover.png", png)]);
        let (root, _) = unpack(&bytes, &dir).unwrap();
        assert_eq!(std::fs::read(root.join("icon.png")).unwrap(), png);
        assert_eq!(std::fs::read(root.join("cover.png")).unwrap(), png);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_traversal_and_bad_ids() {
        let dir = std::env::temp_dir().join(format!("mandara-mkt-bad-{}", std::process::id()));
        // tar::Builder refuses `..` itself, so craft the header by hand.
        let mut h = tar::Header::new_gnu();
        h.as_gnu_mut().unwrap().name[..12].copy_from_slice(b"../evil.toml");
        h.set_size(1);
        h.set_mode(0o644);
        h.set_cksum();
        let mut b = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        b.append(&h, &b"x"[..]).unwrap();
        let bytes = b.into_inner().unwrap().finish().unwrap();
        assert!(unpack(&bytes, &dir).is_err());
        let bad = archive(&[("plugin.toml", b"id = \"../../x\"\nname = \"x\"\n")]);
        assert!(unpack(&bad, &dir).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn search_url_uses_topic() {
        assert!(search_url("", 1).ends_with("/search/repositories?q=topic:mandara-plugin+fork:true&sort=stars&order=desc&per_page=100&page=1"));
        assert!(search_url("moon phase", 2).contains("q=moon+phase+topic:mandara-plugin+fork:true&"));
        assert!(search_url("moon", 2).ends_with("&page=2"));
        assert!(search_url("a&b=c", 1).contains("q=abc+topic"));
    }

    fn asset(name: &str) -> Asset {
        Asset { name: name.into(), browser_download_url: format!("https://example.invalid/{name}"), size: 1 }
    }

    #[test]
    fn picks_the_plugin_asset() {
        let assets = [asset("notes.txt"), asset("other.mandara.tar.gz"), asset("moon-phase.mandara.tar.gz")];
        assert_eq!(plugin_asset(&assets, "you/mandara-moon-phase").unwrap().name, "moon-phase.mandara.tar.gz");
        assert_eq!(plugin_asset(&assets, "you/moon-phase").unwrap().name, "moon-phase.mandara.tar.gz");
        // Any name works when there's one archive; the id comes from plugin.toml.
        assert_eq!(plugin_asset(&[asset("Weird.Mandara.tar.gz")], "you/x").unwrap().name, "Weird.Mandara.tar.gz");
        assert!(plugin_asset(&[asset("moon.tar.gz")], "you/moon").is_none());
    }

    #[test]
    fn flags_follow_the_checked_release() {
        let dir = std::env::temp_dir().join(format!("mandara-mkt-flags-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(FLAGS_FILE);
        let fail = |dir: &Path, _: &str| if dir.ends_with("bad") { Err(Finding::new(Class::Broken, "it traps")) } else { Ok(()) };
        let (strict, forced) = (Vetting { check: &fail, force: false }, Vetting { check: &fail, force: true });
        let repo = "you/mandara-x";
        // A failing release is refused and flagged for its tag.
        let err = strict.admit_at(&path, repo, "v1.2", Path::new("bad")).unwrap_err();
        assert_eq!(err, InstallError::Rejected { tag: "v1.2".into(), class: Class::Broken, reason: "it traps".into() });
        assert!(err.to_string().contains("v1.2") && err.to_string().contains("it traps"));
        assert_eq!(flags_at(&path)[repo].tag, "v1.2");
        // Forced, it installs and the warning stays on that version.
        assert_eq!(forced.admit_at(&path, repo, "v1.2", Path::new("bad")), Ok(()));
        assert!(flags_at(&path)[repo].warning().contains("v1.2"));
        // A newer release is checked afresh: failing moves the flag to it…
        assert!(strict.admit_at(&path, repo, "v1.3", Path::new("bad")).is_err());
        assert_eq!(flags_at(&path)[repo].tag, "v1.3");
        // …and passing clears it.
        assert_eq!(strict.admit_at(&path, repo, "v1.4", Path::new("good")), Ok(()));
        assert!(flags_at(&path).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ignores_stray_top_level_folders() {
        let dir = std::env::temp_dir().join(format!("mandara-mkt-stray-{}", std::process::id()));
        let bytes = archive(&[("__MACOSX/._x", b"x"), ("moon/plugin.toml", b"id = \"moon-phase\"\nname = \"Moon\"\n"), ("moon/plugin.wasm", b"\0asm")]);
        assert_eq!(unpack(&bytes, &dir).unwrap().1, "moon-phase");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
