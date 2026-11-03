//! Language packs on demand: translations aren't compiled into Mandara, so
//! a language is downloaded the first time it's used (or chosen) and kept
//! next to the version it belongs to (`mandara_i18n::packs`).
//!
//! A pack always comes from **the running version's own release**
//! (`releases/tags/v<running>`, never `latest`): that release's
//! `SHA256SUMS` must carry a valid signature by Mandara's release key, its
//! `update.json` must match its signed line, and the pack must match the
//! hashes `update.json` lists for it (the `.zst` download, then the
//! decompressed `.mlang`). Finally the pack's own header must name the
//! running version and the language, or it isn't installed. Files are
//! written through a temporary file and a rename, so a process that maps
//! the old pack keeps reading it safely.
//!
//! - [`install_hooks`]: a language used without a pack (the shell's, a
//!   plugin's own, a desktop widget's) is downloaded in the background,
//!   then swapped in, and the UI re-renders ([`subscribe`]).
//! - [`selected`]: a language just chosen (Studio, `mandara msg`, the
//!   language plugin) is also checked against the published hash, so a
//!   republished pack replaces an outdated one.
//! - [`prepare_update`]: after the updater installed a new binary, the new
//!   version's packs for every language in use, so it starts translated.
//!
//! Offline mode blocks all of it; the interface then stays in English
//! until the pack can be downloaded ([`state`] says why, Studio shows it
//! with a Retry button).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use mandara_delta::{LocalePack, MANIFEST_FILE, Manifest};
use mandara_i18n::{Lang, packs, t, tf, tr};

use super::{AssetJson, REPO, ReleaseJson, api_base, asset_url, fetch, listed, signed_sums, unexpected};

/// Largest download or decompressed pack accepted (the biggest pack is a
/// few hundred KB; a hostile size can't fill memory).
const MAX_PACK_BYTES: u64 = 64 << 20;
/// Largest JSON answer read (release metadata, update.json, SHA256SUMS).
const MAX_JSON_BYTES: u64 = 4 << 20;
/// How long a release's published packs are reused before asking again.
const PUBLISHED_TTL: Duration = Duration::from_secs(60 * 60);

/// What's happening with a language's pack in this process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Looking at the release (nothing downloaded yet).
    Checking,
    /// Downloading: bytes so far and the download's size.
    Downloading { done: u64, total: u64 },
    /// Installed and loaded.
    Ready,
    /// Couldn't get it (translated reason); the interface stays in English.
    Failed(String),
}

impl State {
    pub fn busy(&self) -> bool {
        matches!(self, State::Checking | State::Downloading { .. })
    }
}

static STATES: Mutex<BTreeMap<u8, State>> = Mutex::new(BTreeMap::new());
/// Languages already compared with the published hash in this process.
static CHECKED: Mutex<BTreeSet<u8>> = Mutex::new(BTreeSet::new());
static LISTENERS: Mutex<Vec<async_channel::Sender<Lang>>> = Mutex::new(Vec::new());
/// The running release's published packs, with when they were read.
static PUBLISHED: Mutex<Option<(Instant, Published)>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The state of `lang`'s pack, when this process did anything about it.
pub fn state(lang: Lang) -> Option<State> {
    lock(&STATES).get(&(lang as u8)).cloned()
}

fn set_state(lang: Lang, state: State) {
    lock(&STATES).insert(lang as u8, state);
    notify(lang);
}

fn notify(lang: Lang) {
    // A nudge per change; a full channel already has one waiting.
    lock(&LISTENERS).retain(|tx| !tx.is_closed() && (tx.try_send(lang).is_ok() || tx.is_full()));
}

/// A receiver nudged whenever a pack's [`state`] changes or a pack is
/// swapped in: views re-render on it.
pub fn subscribe() -> async_channel::Receiver<Lang> {
    let (tx, rx) = async_channel::bounded(1);
    lock(&LISTENERS).push(tx);
    rx
}

/// Downloads missing packs as the process uses them. Call once, before
/// the first translated string (the shell and Studio do, at start-up).
pub fn install_hooks() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        packs::on_missing(|lang| request(lang, false));
        packs::on_loaded(notify);
    });
}

/// A language was just chosen: download it when it's missing, and when it
/// is present compare it with the published hash once per process.
pub fn selected(lang: Lang) {
    request(lang, false);
}

/// Tries again after a failure (Studio's Retry).
pub fn retry(lang: Lang) {
    lock(&CHECKED).remove(&(lang as u8));
    request(lang, true);
}

/// Starts the background work for `lang` unless it's English, already
/// running, or (when present and already compared) needs nothing.
fn request(lang: Lang, force: bool) {
    if lang == Lang::En || state(lang).is_some_and(|s| s.busy()) {
        return;
    }
    let present = packs::is_available(lang);
    if present && !force && !lock(&CHECKED).insert(lang as u8) {
        return;
    }
    if !present {
        set_state(lang, State::Checking);
    }
    std::thread::Builder::new()
        .name(format!("mandara-langpack-{}", lang.code()))
        .spawn(move || {
            let result = ensure(lang, present);
            match result {
                Ok(()) => set_state(lang, State::Ready),
                Err(e) if packs::is_available(lang) => {
                    // A pack is loaded; only the freshness check failed.
                    tracing::info!("{} language pack: {e}", lang.code());
                    set_state(lang, State::Ready);
                }
                Err(e) => {
                    tracing::warn!("{} language pack: {e}", lang.code());
                    set_state(lang, State::Failed(e));
                }
            }
        })
        .map(drop)
        .unwrap_or_else(|e| tracing::warn!("language pack thread: {e}"));
}

/// The download itself, serialized with other Mandara processes (the
/// shell and Studio may ask at once) by a lock file next to the pack.
fn ensure(lang: Lang, was_present: bool) -> Result<(), String> {
    crate::offline::check()?;
    let version = packs::running_version();
    let dir = packs::user_dir(version);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let lock_path = dir.join(format!(".{}.lock", lang.code()));
    let lock_file = std::fs::File::create(&lock_path).map_err(|e| format!("{}: {e}", lock_path.display()))?;
    // While another Mandara process (the shell, or Studio) downloads it,
    // show its progress here too: it writes the bytes so far next to the
    // lock.
    let progress_path = dir.join(format!(".{}.progress", lang.code()));
    loop {
        match lock_file.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => {
                if let Some((done, total)) = read_progress(&progress_path) {
                    set_state(lang, State::Downloading { done, total });
                }
                std::thread::sleep(PEER_POLL);
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(format!("{}: {e}", lock_path.display())),
        }
    }
    // Another process may have installed it while this one waited.
    if !was_present && packs::reload(lang) {
        return Ok(());
    }
    let published = published_cached(&api_base(), version)?;
    let entry = published.entry(lang)?;
    if let Some(path) = packs::find(lang, version).map(|(p, _)| p)
        && std::fs::read(&path).is_ok_and(|b| mandara_delta::sha256_hex(&b).eq_ignore_ascii_case(&entry.pack_sha256))
    {
        // Current: load it unless it's the one already mapped.
        if packs::loaded_from(lang).as_deref() != Some(path.as_path()) {
            packs::reload(lang);
        }
        return Ok(());
    }
    let result = download(&published, lang, &dir, &|done, total| {
        set_state(lang, State::Downloading { done, total });
        let _ = std::fs::write(&progress_path, format!("{done} {total}"));
    });
    let _ = std::fs::remove_file(&progress_path);
    result?;
    if packs::reload(lang) { Ok(()) } else { Err(tf("The {language} language pack was downloaded but couldn't be loaded", &[("language", &tr(lang.english_name()))])) }
}

/// How often a process waiting for another one's download reads its
/// progress.
const PEER_POLL: Duration = Duration::from_millis(100);

/// `done total` written by the process downloading a pack.
fn read_progress(path: &Path) -> Option<(u64, u64)> {
    let text = std::fs::read_to_string(path).ok()?;
    let (done, total) = text.trim().split_once(' ')?;
    Some((done.parse().ok()?, total.parse().ok()?))
}

/// What a release publishes for its language packs, its signature checked.
#[derive(Clone, Debug)]
pub struct Published {
    pub version: String,
    pub locales: BTreeMap<String, LocalePack>,
    sums: String,
    assets: Vec<AssetJson>,
}

impl Published {
    /// `lang`'s entry, or why there is none.
    pub fn entry(&self, lang: Lang) -> Result<&LocalePack, String> {
        self.locales.get(lang.code()).ok_or_else(|| tf("Mandara {version} has no {language} language pack to download", &[("version", &self.version), ("language", &tr(lang.english_name()))]))
    }
}

fn published_cached(api: &str, version: &str) -> Result<Published, String> {
    if let Some((at, p)) = lock(&PUBLISHED).as_ref()
        && p.version == version
        && at.elapsed() < PUBLISHED_TTL
    {
        return Ok(p.clone());
    }
    let p = published(api, version)?;
    *lock(&PUBLISHED) = Some((Instant::now(), p.clone()));
    Ok(p)
}

/// Reads `version`'s release from `api`: signed `SHA256SUMS`, then
/// `update.json` (which must match its signed line) and its `locales`.
pub fn published(api: &str, version: &str) -> Result<Published, String> {
    let version = version.trim_start_matches('v');
    let url = format!("{api}/repos/{REPO}/releases/tags/v{version}");
    let body = fetch::get(&url, MAX_JSON_BYTES, &|_| {}).map_err(|f| match f {
        fetch::Fetch::NotFound => tf("Mandara {version} has no published release to download languages from", &[("version", &version)]),
        other => other.into(),
    })?;
    let release: ReleaseJson = serde_json::from_slice(&body).map_err(unexpected)?;
    let sums = signed_sums(&release.assets, version)?;
    let (manifest_url, _) = asset_url(&release.assets, MANIFEST_FILE).ok_or_else(|| tf("Mandara {version} has no language packs to download", &[("version", &version)]))?;
    let bytes = fetch::get(&manifest_url, MAX_JSON_BYTES, &|_| {}).map_err(String::from)?;
    listed(&sums, MANIFEST_FILE, &bytes)?;
    let manifest = Manifest::parse(&bytes)?;
    if manifest.version.trim_start_matches('v') != version {
        return Err(unexpected(format!("update.json describes {}", manifest.version)));
    }
    if manifest.locales.is_empty() {
        return Err(tf("Mandara {version} has no language packs to download", &[("version", &version)]));
    }
    Ok(Published { version: version.to_string(), locales: manifest.locales, sums, assets: release.assets })
}

/// Downloads `lang`'s pack of `published` into `dir`, verifying the
/// download against the signed listing and update.json, the decompressed
/// pack against update.json, and its header against the version and
/// language. Progress is `(bytes, total)` of the download.
pub fn download(published: &Published, lang: Lang, dir: &Path, progress: &dyn Fn(u64, u64)) -> Result<PathBuf, String> {
    let entry = published.entry(lang)?;
    let failed = |file: &str| tf("{file} failed verification", &[("file", &file)]);
    // update.json is trusted through the signed listing; the pack's own
    // line in the listing must agree too.
    if mandara_delta::sha256sums_lookup(&published.sums, &entry.file).is_some_and(|s| !s.eq_ignore_ascii_case(&entry.sha256)) {
        return Err(tf("{file} doesn't match the signed checksums", &[("file", &entry.file)]));
    }
    let (url, _) = asset_url(&published.assets, &entry.file).ok_or_else(|| tf("Mandara {version} has no {language} language pack to download", &[("version", &published.version), ("language", &tr(lang.english_name()))]))?;
    let total = entry.size;
    progress(0, total);
    let zst = fetch::get(&url, entry.size.min(MAX_PACK_BYTES), &|done| progress(done, total)).map_err(String::from)?;
    if !mandara_delta::sha256_hex(&zst).eq_ignore_ascii_case(&entry.sha256) {
        return Err(failed(&entry.file));
    }
    let pack = mandara_delta::decompress(&zst, entry.pack_size.min(MAX_PACK_BYTES)).map_err(|e| format!("{}: {e}", entry.file))?;
    if pack.len() as u64 != entry.pack_size || !mandara_delta::sha256_hex(&pack).eq_ignore_ascii_case(&entry.pack_sha256) {
        return Err(failed(&entry.file));
    }
    packs::install(lang, &published.version, &pack, dir).map_err(|e| format!("{}: {e}", entry.file))
}

/// Every language Mandara speaks with `cfg`: the shell's, and every
/// `language` setting in it that names one (plugins' own languages,
/// desktop widgets), plus `extra` (the login screen's). English needs no
/// pack and is left out.
pub fn languages_in_use(cfg: &mandara_config::ShellConfig, extra: impl IntoIterator<Item = Lang>) -> Vec<Lang> {
    fn walk(v: &serde_json::Value, out: &mut BTreeSet<u8>) {
        match v {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    if k == "language"
                        && let Some(lang) = v.as_str().and_then(Lang::from_code)
                    {
                        out.insert(lang as u8);
                    }
                    walk(v, out);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    let mut set = BTreeSet::new();
    set.insert(cfg.general.locale.lang() as u8);
    if let Ok(v) = serde_json::to_value(cfg) {
        walk(&v, &mut set);
    }
    set.extend(extra.into_iter().map(|l| l as u8));
    Lang::ALL.into_iter().filter(|l| *l != Lang::En && set.contains(&(*l as u8))).collect()
}

/// The plugins' own languages (their `language` settings): cheap enough to
/// run on every config change.
pub fn plugin_languages(cfg: &mandara_config::ShellConfig) -> Vec<Lang> {
    let mut out: Vec<Lang> = cfg.plugins.settings.values().filter_map(|t| t.get("language")?.as_str().and_then(Lang::from_code)).filter(|l| *l != Lang::En).collect();
    out.sort_unstable_by_key(|l| *l as u8);
    out.dedup();
    out
}

/// What [`prepare_update`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpdateLanguages {
    pub installed: Vec<Lang>,
    /// Languages that will download when the new version starts instead.
    pub failed: Vec<(Lang, String)>,
}

/// After an update to `new_version` was installed: downloads its packs for
/// `langs` into its folder (from `api`), so it starts translated, then
/// removes pack folders older than the running version (which stays for a
/// rollback). Failures are reported, never fatal: the new version
/// downloads what's missing when it starts.
pub fn prepare_update_with(api: &str, new_version: &str, langs: &[Lang], progress: &dyn Fn(super::Progress)) -> UpdateLanguages {
    let mut report = UpdateLanguages::default();
    let new_version = new_version.trim_start_matches('v');
    let dir = packs::user_dir(new_version);
    let wanted: Vec<Lang> = langs.iter().copied().filter(|l| *l != Lang::En).collect::<BTreeSet<_>>().into_iter().collect();
    if !wanted.is_empty() {
        progress(super::Progress::Step(t("Downloading languages for the new version…").to_string()));
        match published(api, new_version) {
            Ok(p) => {
                for lang in wanted {
                    match download(&p, lang, &dir, &|done, total| progress(super::Progress::Bytes { done, total })) {
                        Ok(_) => report.installed.push(lang),
                        Err(e) => report.failed.push((lang, e)),
                    }
                }
            }
            Err(e) => report.failed.extend(wanted.into_iter().map(|l| (l, e.clone()))),
        }
    }
    let removed = packs::prune(&[new_version, packs::running_version()]);
    if !removed.is_empty() {
        tracing::info!("removed {} old language pack folders", removed.len());
    }
    report
}

/// [`prepare_update_with`] against GitHub (or `MANDARA_UPDATE_API`).
pub fn prepare_update(new_version: &str, langs: &[Lang], progress: &dyn Fn(super::Progress)) -> UpdateLanguages {
    prepare_update_with(&api_base(), new_version, langs, progress)
}

/// The size of a download in the state's words: "Downloading Arabic…
/// 180 KB".
pub fn describe(lang: Lang, state: &State) -> Option<String> {
    let language = tr(lang.english_name());
    match state {
        State::Checking => Some(tf("Downloading {language}…", &[("language", &language)])),
        State::Downloading { done, total } => {
            let size = if *total > 0 { format!("{} / {}", super::human_size(*done), super::human_size(*total)) } else { super::human_size(*done) };
            Some(tf("Downloading {language}… {size}", &[("language", &language), ("size", &size)]))
        }
        State::Failed(e) => Some(tf("Couldn't download {language}: {error}", &[("language", &language), ("error", e)])),
        State::Ready => None,
    }
}

/// Languages with a [`State`] this process recorded, for views listing
/// downloads in progress or failed.
pub fn active() -> Vec<(Lang, State)> {
    let states = lock(&STATES);
    Lang::ALL.into_iter().filter_map(|l| states.get(&(l as u8)).map(|s| (l, s.clone()))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_in_use_come_from_every_language_setting() {
        let mut cfg = mandara_config::ShellConfig::default();
        cfg.general.locale = mandara_config::Locale::from_lang(Lang::Ar);
        cfg.plugins.settings.entry("quran".into()).or_default().insert("language".into(), toml::Value::String("ur".into()));
        cfg.plugins.settings.entry("adhan".into()).or_default().insert("language".into(), toml::Value::String("follow".into()));
        assert_eq!(languages_in_use(&cfg, [Lang::Fr, Lang::En]), vec![Lang::Ar, Lang::Ur, Lang::Fr]);
        cfg.general.locale = mandara_config::Locale::from_lang(Lang::En);
        assert_eq!(languages_in_use(&cfg, []), vec![Lang::Ur]);
    }

    #[test]
    fn states_read_like_progress() {
        let s = describe(Lang::Ar, &State::Downloading { done: 180_000, total: 240_000 }).unwrap();
        assert!(s.contains("180 KB") && s.contains("240 KB"), "{s}");
        assert!(describe(Lang::Ar, &State::Ready).is_none());
        assert!(State::Checking.busy() && !State::Failed(String::new()).busy());
    }
}
