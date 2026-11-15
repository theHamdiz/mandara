//! Launcher search providers and personalization:
//!
//! * **Providers** (each with a prefix and an "also in plain searches"
//!   switch): emoji (bundled table with English and Arabic keywords), open
//!   windows, clipboard history, wallpapers and files (recent files plus a
//!   small in-memory index of your home folders).
//! * **Usage**: launches are counted with exponential decay (frecency) in
//!   `~/.local/state/mandara/launcher-usage.toml`; ranking and the
//!   empty-query "Recent" view read it.
//! * **App customization**: pinned order, hidden apps, per-app names and
//!   icons, desktop actions as results, and packaging badges.

mod usage_store;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use mandara_config::workflow::{LauncherProviders, ProviderConfig};
use mandara_config::{LauncherConfig, ShellConfig};
use mandara_i18n::tk;
use serde::{Deserialize, Serialize};

use crate::apps::{App, DesktopAction};

/// Results kept per provider query.
pub const PROVIDER_LIMIT: usize = 40;
/// Provider hits mixed into a plain search, per provider.
const GLOBAL_LIMIT: usize = 4;
/// Characters of a clipboard entry shown as a launcher result.
const CLIPBOARD_LABEL_CHARS: usize = 90;
/// Seconds per day (usage decay is configured in days).
const SECS_PER_DAY: f64 = 86_400.0;
/// Usage scores below this are forgotten when saving.
const FORGET_BELOW: f64 = 0.02;
/// Keep recent ranking data bounded even when many different files are launched.
const MAX_USAGE_ENTRIES: usize = 1024;
/// A ranking identity is a compact label; oversized identities are not recorded.
const MAX_USAGE_KEY_BYTES: usize = 512;
/// Frecency is compared as an integer of this resolution.
const FRECENCY_SCALE: f64 = 1000.0;
/// File index: folders walked, depth, size cap and refresh period.
const FILE_INDEX_DEPTH: usize = 4;
const FILE_INDEX_MAX: usize = 25_000;
const FILE_INDEX_TTL: Duration = Duration::from_secs(600);
/// Recently used files read from `recently-used.xbel`.
const RECENT_FILES_MAX: usize = 200;

// ── Hits ─────────────────────────────────────────────────────────────────

/// What a provider result shows on its left.
#[derive(Clone, Debug, PartialEq)]
pub enum Lead {
    /// A character drawn large (emoji).
    Glyph(String),
    /// A window's app icon.
    AppIcon(String),
    /// An image thumbnail (wallpapers, copied images).
    Image(PathBuf),
    /// A bundled Mandara icon.
    Icon(&'static str),
    /// A square picture with its own shape (a plugin's image), drawn whole
    /// at the tile's size.
    Picture(PathBuf),
}

/// What activating a provider result does.
#[derive(Clone, Debug, PartialEq)]
#[expect(clippy::large_enum_variant, reason = "launcher results are rebuilt per keystroke; boxing adds an allocation per hit (speed over memory)")]
pub enum HitAction {
    /// Copy text (and paste it when auto-paste is on).
    Copy(String),
    /// A documented `mandara msg` command line.
    Command(Vec<String>),
    /// Open a file or folder with its default app.
    Open(PathBuf),
    /// Run an app's desktop action.
    AppAction(App, DesktopAction),
}

/// One provider result.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// Provider id (`emoji`, `windows`, …): its usage key prefix too.
    pub provider: &'static str,
    /// Stable key within the provider (usage tracking).
    pub key: String,
    pub label: String,
    pub detail: String,
    pub lead: Lead,
    /// Kind badge (`tk` text).
    pub badge: &'static str,
    /// Extra search words.
    pub keywords: String,
    pub action: HitAction,
}

impl Hit {
    pub fn usage_key(&self) -> String {
        format!("{}:{}", self.provider, self.key)
    }

    pub fn haystack(&self) -> String {
        format!("{} {} {}", self.label, self.detail, self.keywords)
    }
}

// ── Prefix routing ───────────────────────────────────────────────────────

/// The provider a query is addressed to by its prefix, and the rest of the
/// query. Longest prefix wins (so `::` could be set apart from `:`).
pub fn route<'q>(query: &'q str, providers: &LauncherProviders) -> Option<(&'static str, &'q str)> {
    let q = query.trim_start();
    providers
        .all()
        .into_iter()
        .filter(|(_, p)| p.enabled && !p.prefix.is_empty() && q.starts_with(p.prefix.as_str()))
        .max_by_key(|(_, p)| p.prefix.len())
        .map(|(id, p)| (id, &q[p.prefix.len()..]))
}

/// Providers mixed into plain searches.
pub fn globals(providers: &LauncherProviders) -> impl Iterator<Item = &'static str> + '_ {
    providers.all().into_iter().filter(|(_, p)| p.enabled && p.global).map(|(id, _)| id)
}

/// Per-provider cap for plain searches.
pub fn global_limit() -> usize {
    GLOBAL_LIMIT
}

/// A provider's config by id.
pub fn provider<'a>(providers: &'a LauncherProviders, id: &str) -> Option<&'a ProviderConfig> {
    providers.all().into_iter().find(|(p, _)| *p == id).map(|(_, c)| c)
}

// ── Emoji ────────────────────────────────────────────────────────────────

/// One emoji with its search words.
#[derive(Clone, Debug)]
pub struct Emoji {
    pub glyph: &'static str,
    pub name: &'static str,
    pub keywords: &'static str,
    pub arabic: &'static str,
}

/// The bundled table (`data/emoji.tsv`), parsed once.
pub fn emoji_table() -> &'static [Emoji] {
    static TABLE: OnceLock<Vec<Emoji>> = OnceLock::new();
    TABLE.get_or_init(|| {
        include_str!("data/emoji.tsv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .filter_map(|l| {
                let mut cols = l.split('\t');
                Some(Emoji { glyph: cols.next()?, name: cols.next()?, keywords: cols.next().unwrap_or(""), arabic: cols.next().unwrap_or("") })
            })
            .collect()
    })
}

/// Score of `words` for typed `q` (higher is better; 0 = no match):
/// whole-word hits beat word prefixes beat substrings.
fn word_score(text: &str, q: &str) -> u32 {
    let mut best = 0;
    for w in text.split_whitespace() {
        let s = if w == q {
            4
        } else if w.starts_with(q) {
            3
        } else if w.contains(q) {
            1
        } else {
            0
        };
        best = best.max(s);
    }
    best
}

/// Emoji matching every typed word (English name / keywords or Arabic),
/// best first; an empty query lists the table in order.
pub fn search_emoji(query: &str, limit: usize) -> Vec<&'static Emoji> {
    let q = query.trim().to_lowercase();
    let table = emoji_table();
    if q.is_empty() {
        return table.iter().take(limit).collect();
    }
    let words: Vec<&str> = q.split_whitespace().collect();
    let mut scored: Vec<(u32, usize, &Emoji)> = table
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let mut total = 0;
            for w in &words {
                // The name counts double: "cat" finds the cat before "category".
                let s = (word_score(e.name, w) * 2).max(word_score(e.keywords, w)).max(word_score(e.arabic, w) * 2);
                if s == 0 {
                    return None;
                }
                total += s;
            }
            // Curated (popular) emoji win ties.
            Some((total + u32::from(!e.keywords.is_empty()), i, e))
        })
        .collect();
    // Best score first; then the exact name, then shorter (more specific)
    // names, then table order.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then((b.2.name == q).cmp(&(a.2.name == q))).then(a.2.name.len().cmp(&b.2.name.len())).then(a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|(_, _, e)| e).collect()
}

/// `grinning face` → `Grinning face`.
fn sentence_case(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

pub fn emoji_hits(query: &str, limit: usize) -> Vec<Hit> {
    search_emoji(query, limit)
        .into_iter()
        .map(|e| Hit {
            provider: "emoji",
            key: e.glyph.to_string(),
            label: sentence_case(e.name),
            detail: if e.arabic.is_empty() { e.keywords.to_string() } else { e.arabic.to_string() },
            lead: Lead::Glyph(e.glyph.to_string()),
            badge: tk("Emoji"),
            keywords: String::new(),
            action: HitAction::Copy(e.glyph.to_string()),
        })
        .collect()
}

// ── Windows / clipboard / wallpapers ─────────────────────────────────────

/// Open windows (most recently used first) matching every typed word in
/// title or app name.
pub fn window_hits(query: &str, windows: &[mandara_services::toplevels::Toplevel], limit: usize, app_name: impl Fn(&str) -> String) -> Vec<Hit> {
    let q = query.trim().to_lowercase();
    windows
        .iter()
        .filter(|w| !crate::windows::is_shell(&w.app_id))
        .filter_map(|w| {
            let app = app_name(&w.app_id);
            let text = format!("{} {} {}", w.title, app, w.app_id).to_lowercase();
            (q.is_empty() || q.split_whitespace().all(|t| text.contains(t))).then(|| Hit {
                provider: "windows",
                key: w.app_id.clone(),
                label: if w.title.is_empty() { app.clone() } else { w.title.clone() },
                detail: if w.minimized { mandara_i18n::tf("{app} · minimized", &[("app", &app)]) } else { app },
                lead: Lead::AppIcon(w.app_id.clone()),
                badge: tk("Window"),
                keywords: String::new(),
                action: HitAction::Command(vec!["window".into(), "focus".into(), w.id.to_string()]),
            })
        })
        .take(limit)
        .collect()
}

/// Clipboard entries matching every typed word (text, kind, language, host,
/// app or collection name; "image", "code", "color" … find a kind).
pub fn clipboard_hits(query: &str, entries: &[mandara_services::ClipEntry], collections: &[mandara_services::clipboard::ClipCollection], limit: usize) -> Vec<Hit> {
    use mandara_services::clipboard::ClipContent;
    entries
        .iter()
        .filter(|e| e.matches(query, collections))
        .take(limit)
        .map(|e| {
            let kind = e.class.kind();
            let (lead, label) = match &e.content {
                ClipContent::Image { path, mime } => (Lead::Image(path.clone()), mandara_i18n::tf("Image ({mime})", &[("mime", mime)])),
                _ => (Lead::Icon(if e.pinned { "pin" } else { crate::plugins::clipboard::clip_kind_icon(kind) }), e.preview(CLIPBOARD_LABEL_CHARS)),
            };
            let app = if e.app_id.is_empty() { String::new() } else { crate::plugins::window::app_name(&e.app_id) };
            let kind_name = mandara_i18n::tr(crate::plugins::clipboard::clip_kind_label(kind));
            Hit {
                provider: "clipboard",
                key: e.id.to_string(),
                label,
                detail: if app.is_empty() { kind_name.to_string() } else { format!("{kind_name} · {app}") },
                lead,
                badge: tk("Clipboard"),
                keywords: String::new(),
                action: HitAction::Command(vec!["clipboard".into(), "copy".into(), e.id.to_string()]),
            }
        })
        .collect()
}

pub fn wallpaper_hits(query: &str, images: &[PathBuf], limit: usize) -> Vec<Hit> {
    let q = query.trim().to_lowercase();
    images
        .iter()
        .filter(|p| q.is_empty() || p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains(&q)))
        .take(limit)
        .map(|p| {
            let name = p.file_stem().map(|n| n.to_string_lossy().replace(['_', '-'], " ")).unwrap_or_default();
            Hit {
                provider: "wallpapers",
                key: p.display().to_string(),
                label: name,
                detail: p.parent().map(|d| d.display().to_string()).unwrap_or_default(),
                lead: Lead::Image(p.clone()),
                badge: tk("Wallpaper"),
                keywords: String::new(),
                action: HitAction::Command(vec!["wallpaper".into(), "set".into(), p.display().to_string()]),
            }
        })
        .collect()
}

// ── Files ────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
pub struct FileEntry {
    path: PathBuf,
    directory: bool,
}

/// One stalled key leaves room for a replacement without creating an
/// unbounded number of threads if several mounts are unavailable.
const MAX_SNAPSHOT_READS: usize = 2;

pub(super) struct Snapshot<V> {
    ready: async_channel::Receiver<()>,
    value: OnceLock<Arc<V>>,
}

impl<V> Snapshot<V> {
    fn new() -> (async_channel::Sender<()>, Arc<Self>) {
        let (done, ready) = async_channel::bounded(1);
        (done, Arc::new(Self { ready, value: OnceLock::new() }))
    }

    pub(super) fn ready(&self) -> async_channel::Receiver<()> {
        self.ready.clone()
    }

    /// A closed signal with no value means the request was cancelled or
    /// its worker stopped. The value belongs to this request, not the cache.
    pub(super) fn value(&self) -> Option<Arc<V>> {
        self.value.get().cloned()
    }
}

struct SnapshotJob<K, V> {
    key: K,
    done: async_channel::Sender<()>,
    snapshot: Arc<Snapshot<V>>,
}

impl<K: Clone, V> Clone for SnapshotJob<K, V> {
    fn clone(&self) -> Self { Self { key: self.key.clone(), done: self.done.clone(), snapshot: self.snapshot.clone() } }
}

/// Bounded blocking readers, one queued request per caller kind, and keyed last-good values.
pub(super) struct BackgroundSnapshot<K, V> {
    values: std::collections::VecDeque<(K, Arc<V>)>,
    views: usize,
    running: Vec<(K, Arc<Snapshot<V>>)>,
    queued_view: Option<SnapshotJob<K, V>>,
    queued_command: Option<SnapshotJob<K, V>>,
}

impl<K: 'static, V: 'static> gpui::Global for BackgroundSnapshot<K, V> {}

impl<K, V> Default for BackgroundSnapshot<K, V> {
    fn default() -> Self {
        Self { values: Default::default(), views: 0, running: Vec::new(), queued_view: None, queued_command: None }
    }
}

impl<K: Clone + PartialEq + Send + 'static, V: Send + Sync + 'static> BackgroundSnapshot<K, V> {
    pub(super) fn retain_view(cx: &mut gpui::App) {
        cx.default_global::<Self>().views += 1;
    }

    pub(super) fn release_view(cx: &mut gpui::App) {
        if cx.has_global::<Self>() { cx.global_mut::<Self>().leave(); }
    }

    fn leave(&mut self) {
        self.views = self.views.saturating_sub(1);
        if self.views == 0 {
            self.values = Default::default();
            self.queued_view = None;
            if self.running.is_empty() { self.running.shrink_to_fit(); }
        }
    }

    fn complete(&mut self, key: K, snapshot: &Arc<Snapshot<V>>, value: Option<Arc<V>>) {
        self.running.retain(|(_, running)| !Arc::ptr_eq(running, snapshot));
        if let Some(value) = value {
            let _ = snapshot.value.set(value.clone());
            // Commands own their result. Hidden views do not restore a cache.
            if self.views > 0 {
                self.values.retain(|(loaded, _)| loaded != &key);
                self.values.push_back((key, value));
                if self.values.len() > MAX_SNAPSHOT_READS { self.values.pop_front(); }
            }
        }
        if self.views == 0 && self.running.is_empty() { self.running.shrink_to_fit(); }
    }

    pub(super) fn get(cx: &gpui::App, key: &K) -> Option<Arc<V>> {
        cx.try_global::<Self>()?.values.iter().find_map(|(loaded, value)| (loaded == key).then(|| value.clone()))
    }

    pub(super) fn refresh(cx: &mut gpui::App, key: K, load: fn(K) -> V) -> Arc<Snapshot<V>> {
        let snapshot = cx.default_global::<Self>().request(key, false);
        Self::start_next(cx, load);
        snapshot
    }

    pub(super) fn refresh_for_view(cx: &mut gpui::App, key: K, load: fn(K) -> V) {
        cx.default_global::<Self>().request(key, true);
        Self::start_next(cx, load);
    }

    fn request(&mut self, key: K, for_view: bool) -> Arc<Snapshot<V>> {
        if let Some((_, snapshot)) = self.running.iter().find(|(running, _)| running == &key) {
            return snapshot.clone();
        }
        if let Some(job) = self.queued_view.as_ref().filter(|job| job.key == key) {
            let snapshot = job.snapshot.clone();
            if !for_view { self.queued_command = Some(job.clone()); }
            return snapshot;
        }
        if let Some(job) = self.queued_command.as_ref().filter(|job| job.key == key) {
            let snapshot = job.snapshot.clone();
            if for_view {
                self.queued_view = Some(job.clone());
            }
            return snapshot;
        }
        let slot = if for_view {
            &mut self.queued_view
        } else {
            &mut self.queued_command
        };
        let (done, snapshot) = Snapshot::new();
        *slot = Some(SnapshotJob { key, done, snapshot: snapshot.clone() });
        snapshot
    }

    fn next(&mut self) -> Option<SnapshotJob<K, V>> {
        if self.running.len() >= MAX_SNAPSHOT_READS {
            return None;
        }
        // IPC must not replace the refresh that the open launcher is waiting for.
        if let Some(job) = self.queued_view.take() {
            if self.queued_command.as_ref().is_some_and(|command| Arc::ptr_eq(&command.snapshot, &job.snapshot)) { self.queued_command = None; }
            Some(job)
        } else { self.queued_command.take() }
    }

    fn start_next(cx: &mut gpui::App, load: fn(K) -> V) {
        let Some(job) = cx.global_mut::<Self>().next() else { return };
        let SnapshotJob { key, done, snapshot } = job;
        cx.global_mut::<Self>().running.push((key.clone(), snapshot.clone()));
        let (result, receive) = async_channel::bounded(1);
        let input = key.clone();
        let _ = std::thread::Builder::new().name("mandara-launcher-data".into()).spawn(move || {
            let _ = result.try_send(Arc::new(load(input)));
        });
        cx.spawn(async move |cx| {
            let result = receive.recv().await;
            cx.update(|cx| {
                cx.global_mut::<Self>().complete(key, &snapshot, result.ok());
                Self::start_next(cx, load);
            });
            done.close();
        }).detach();
    }
}

pub(super) type RecentFiles = BackgroundSnapshot<(), Vec<FileEntry>>;

pub(super) fn read_recent_files(_: ()) -> Vec<FileEntry> {
    let Some(path) = dirs::data_dir().map(|d| d.join("recently-used.xbel")) else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    // Metadata can block on a network or FUSE mount. Both the existence
    // check and the folder badge come from this background snapshot.
    parse_xbel(&text).into_iter().rev().filter_map(|path| {
        let directory = path.metadata().ok()?.is_dir();
        Some(FileEntry { path, directory })
    }).take(RECENT_FILES_MAX).collect()
}

/// `href="file:///…"` attributes of an XBEL bookmark list, percent-decoded.
pub fn parse_xbel(text: &str) -> Vec<PathBuf> {
    text.match_indices("href=\"file://")
        .filter_map(|(i, m)| {
            let rest = &text[i + m.len()..];
            let end = rest.find('"')?;
            Some(PathBuf::from(percent_decode(&rest[..end])))
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

struct FileIndex {
    built: Instant,
    loading: bool,
    files: Vec<FileEntry>,
}

fn file_index() -> &'static Mutex<Option<FileIndex>> {
    static INDEX: OnceLock<Mutex<Option<FileIndex>>> = OnceLock::new();
    INDEX.get_or_init(|| Mutex::new(None))
}

/// Folders indexed for file search: home's XDG folders.
fn indexed_roots() -> Vec<PathBuf> {
    [dirs::document_dir(), dirs::download_dir(), dirs::desktop_dir(), dirs::picture_dir(), dirs::video_dir(), dirs::audio_dir()].into_iter().flatten().filter(|d| d.is_dir()).collect()
}

/// Walks `roots` breadth-first (hidden entries skipped) up to `depth`
/// levels and `max` paths.
fn walk(roots: &[PathBuf], depth: usize, max: usize) -> Vec<FileEntry> {
    let mut out = Vec::new();
    let mut level: Vec<PathBuf> = roots.to_vec();
    for _ in 0..=depth {
        let mut next = Vec::new();
        for dir in level {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                if e.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                let path = e.path();
                let kind = e.file_type().ok();
                let real_directory = kind.is_some_and(|kind| kind.is_dir());
                if real_directory {
                    next.push(path.clone());
                }
                let directory = real_directory || (kind.is_none_or(|kind| kind.is_symlink()) && path.is_dir());
                out.push(FileEntry { path, directory });
                if out.len() >= max {
                    return out;
                }
            }
        }
        level = next;
    }
    out
}

/// Starts (or refreshes, when stale) the file index on a background thread.
pub fn warm_file_index() {
    {
        let Ok(mut index) = file_index().lock() else { return };
        if index.as_ref().is_some_and(|index| index.loading || index.built.elapsed() <= FILE_INDEX_TTL) {
            return;
        }
        let files = index.take().map(|index| index.files).unwrap_or_default();
        *index = Some(FileIndex { built: Instant::now(), loading: true, files });
    }
    let started = std::thread::Builder::new().name("mandara-file-index".into()).spawn(|| {
        let files = walk(&indexed_roots(), FILE_INDEX_DEPTH, FILE_INDEX_MAX);
        if let Ok(mut index) = file_index().lock() {
            *index = Some(FileIndex { built: Instant::now(), loading: false, files });
        }
    });
    if started.is_err()
        && let Ok(mut index) = file_index().lock()
        && let Some(index) = index.as_mut()
    {
        index.loading = false;
    }
}

fn file_hit(file: &FileEntry) -> Hit {
    let p = &file.path;
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string());
    let home = dirs::home_dir().unwrap_or_default();
    let shown = p.parent().map(|d| d.strip_prefix(&home).map(|r| format!("~/{}", r.display())).unwrap_or_else(|_| d.display().to_string())).unwrap_or_default();
    Hit {
        provider: "files",
        key: p.display().to_string(),
        label: name,
        detail: shown,
        lead: Lead::Icon(if file.directory { "folder" } else { "note" }),
        badge: if file.directory { tk("Folder") } else { tk("File") },
        keywords: String::new(),
        action: HitAction::Open(p.to_path_buf()),
    }
}

/// Recent files first, then indexed ones, matching every typed word in the
/// file name (a query starting with `/` or `~/` matches whole paths).
pub fn file_hits(query: &str, recent: &[FileEntry], limit: usize) -> Vec<Hit> {
    let q = query.trim().to_lowercase();
    let by_path = q.starts_with('/') || q.starts_with("~/");
    let q = if let Some(rest) = q.strip_prefix("~/") { format!("{}/{rest}", dirs::home_dir().unwrap_or_default().display()).to_lowercase() } else { q };
    let matches = |p: &Path| {
        let hay = if by_path { p.display().to_string().to_lowercase() } else { p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default() };
        q.is_empty() || q.split_whitespace().all(|w| hay.contains(w))
    };
    let mut out: Vec<Hit> = recent.iter().filter(|file| matches(&file.path)).take(limit).map(file_hit).collect();
    if !q.is_empty() && out.len() < limit
        && let Ok(i) = file_index().lock()
            && let Some(index) = i.as_ref()
        {
            for file in index.files.iter().filter(|file| matches(&file.path)) {
                if out.len() >= limit {
                    break;
                }
                if !recent.iter().any(|recent| recent.path == file.path) {
                    out.push(file_hit(file));
                }
            }
        }
    out
}

// ── App customization ────────────────────────────────────────────────────

/// Applies hidden apps and name / icon overrides to discovered apps.
pub fn customize(apps: Vec<App>, cfg: &LauncherConfig) -> Vec<App> { customize_for_theme(apps, cfg, None) }

pub(crate) fn customize_for_theme(apps: Vec<App>, cfg: &LauncherConfig, theme: Option<&str>) -> Vec<App> {
    apps.into_iter()
        .filter(|a| !cfg.hidden.iter().any(|h| h.eq_ignore_ascii_case(&a.id)))
        .map(|mut a| {
            if let Some(o) = cfg.overrides.get(&a.id) {
                if !o.name.trim().is_empty() {
                    a.name = o.name.trim().to_string();
                }
                if !o.icon.trim().is_empty() {
                    let spec = o.icon.trim();
                    a.icon = if spec.starts_with('/') || spec.starts_with('~') { Some(mandara_config::paths::expand_tilde(spec)) } else { crate::icon_theme::resolve(spec, theme, false).map(|icon| icon.path) }.or(a.icon);
                }
            }
            a
        })
        .collect()
}

/// Desktop actions of every app as results (`Firefox — New Private Window`).
pub fn action_hits(apps: &[App]) -> Vec<Hit> {
    apps.iter()
        .flat_map(|a| {
            a.actions.iter().map(move |act| Hit {
                provider: "app-action",
                key: format!("{}/{}", a.id, act.id),
                label: format!("{} — {}", a.name, act.name),
                detail: a.comment.clone(),
                lead: Lead::AppIcon(a.id.clone()),
                badge: tk("App action"),
                keywords: a.keywords.clone(),
                action: HitAction::AppAction(a.clone(), act.clone()),
            })
        })
        .collect()
}

/// Empty-query order: pinned apps (in their order), then the most
/// frecent ones, then everything else alphabetically. Returns the apps and
/// how many lead as pinned / recent.
pub fn browse_order(apps: &[App], pinned: &[String], usage: &UsageSnapshot, recent_count: usize) -> (Vec<App>, usize, usize) {
    let mut out: Vec<App> = pinned.iter().filter_map(|id| apps.iter().find(|a| a.id.eq_ignore_ascii_case(id))).cloned().collect();
    let n_pinned = out.len();
    let mut recent: Vec<(i64, &App)> = apps
        .iter()
        .filter(|a| !out.iter().any(|p| p.id == a.id))
        .filter_map(|a| {
            let f = usage.frecency(&format!("app:{}", a.id));
            (f > 0).then_some((f, a))
        })
        .collect();
    recent.sort_by_key(|a| std::cmp::Reverse(a.0));
    out.extend(recent.into_iter().take(recent_count).map(|(_, a)| a.clone()));
    let n_recent = out.len() - n_pinned;
    let rest: Vec<App> = apps.iter().filter(|a| !out.iter().any(|o| o.id == a.id)).cloned().collect();
    out.extend(rest);
    (out, n_pinned, n_recent)
}

// ── Usage (frecency) ─────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageEntry {
    /// Decayed launch count as of `last`.
    pub score: f64,
    /// Unix seconds of the last launch.
    pub last: i64,
}

/// Launch counts with exponential decay.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub entries: BTreeMap<String, UsageEntry>,
}

fn decay(score: f64, elapsed_s: i64, half_life_days: f32) -> f64 {
    let half_life = (f64::from(half_life_days.max(0.1))) * SECS_PER_DAY;
    score * 0.5f64.powf(elapsed_s.max(0) as f64 / half_life)
}

impl Usage {
    /// Counts one launch of `key` at `now`.
    pub fn record(&mut self, key: &str, now: i64, half_life_days: f32) {
        if key.len() > MAX_USAGE_KEY_BYTES { return; }
        let e = self.entries.entry(key.to_string()).or_default();
        e.score = decay(e.score, now - e.last, half_life_days) + 1.0;
        e.last = now;
    }

    /// The decayed score of `key` at `now`.
    pub fn score(&self, key: &str, now: i64, half_life_days: f32) -> f64 {
        self.entries.get(key).map_or(0.0, |e| decay(e.score, now - e.last, half_life_days))
    }

    /// Drops entries that have decayed to nothing.
    pub fn prune(&mut self, now: i64, half_life_days: f32) {
        self.entries.retain(|_, e| decay(e.score, now - e.last, half_life_days) >= FORGET_BELOW);
        self.bound();
    }

    fn bound(&mut self) {
        self.entries.retain(|key, _| key.len() <= MAX_USAGE_KEY_BYTES);
        if self.entries.len() > MAX_USAGE_ENTRIES {
            let mut entries: Vec<_> = std::mem::take(&mut self.entries).into_iter().collect();
            entries.select_nth_unstable_by(MAX_USAGE_ENTRIES, |a, b| b.1.last.cmp(&a.1.last).then_with(|| b.0.cmp(&a.0)));
            entries.truncate(MAX_USAGE_ENTRIES);
            self.entries = entries.into_iter().collect();
        }
    }
}

/// Integer frecency scores frozen at one instant (cheap to sort by).
#[derive(Clone, Debug, Default)]
pub struct UsageSnapshot(pub BTreeMap<String, i64>);

impl UsageSnapshot {
    pub fn frecency(&self, key: &str) -> i64 {
        self.0.get(key).copied().unwrap_or(0)
    }
}

fn usage_path() -> PathBuf {
    dirs::state_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/state")).join("mandara").join("launcher-usage.toml")
}

/// The first history read is shared by every launcher and command.
pub(super) fn usage_ready() -> async_channel::Receiver<()> { usage_store::ready() }

#[derive(Default)]
pub(crate) struct UsageRevision(u64);
impl gpui::Global for UsageRevision {}
struct UsageLoading { _task: gpui::Task<()> }
impl gpui::Global for UsageLoading {}

pub(crate) fn warm_usage(cx: &mut gpui::App) {
    let ready = usage_ready();
    if ready.is_closed() || cx.has_global::<UsageLoading>() { return; }
    let task = cx.spawn(async move |cx| {
        let _ = ready.recv().await;
        cx.update(|cx| {
            cx.remove_global::<UsageLoading>();
            let revision = cx.default_global::<UsageRevision>();
            revision.0 = revision.0.wrapping_add(1);
        });
    });
    cx.set_global(UsageLoading { _task: task });
}

fn now() -> i64 {
    chrono::Local::now().timestamp()
}

/// Records a launch (when tracking is on) and saves off the UI thread.
pub fn record_use(key: &str, cfg: &ShellConfig) {
    let u = &cfg.launcher.usage;
    if !u.track || key.is_empty() {
        return;
    }
    usage_store::record(key, now(), u.half_life_days);
}

/// Frecency of every key right now (empty when sorting by usage is off).
pub fn usage_snapshot(cfg: &ShellConfig) -> UsageSnapshot {
    let u = &cfg.launcher.usage;
    if !u.sort {
        return UsageSnapshot::default();
    }
    let t = now();
    usage_store::read(|usage| UsageSnapshot(usage.entries.keys().map(|k| (k.clone(), (usage.score(k, t, u.half_life_days) * FRECENCY_SCALE) as i64)).collect()))
}

/// Forgets every recorded launch. Returns how many entries were dropped.
pub fn reset_usage() -> usize { usage_store::reset() }

/// The most used keys with their scores (for `launcher usage list`).
pub fn usage_list(cfg: &ShellConfig, limit: usize) -> Vec<(String, f64)> {
    usage_list_with_half(cfg.launcher.usage.half_life_days, limit)
}

fn usage_list_with_half(h: f32, limit: usize) -> Vec<(String, f64)> {
    let t = now();
    let mut rows: Vec<(String, f64)> = usage_store::read(|u| u.entries.keys().map(|k| (k.clone(), u.score(k, t, h))).collect());
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    rows.truncate(limit);
    rows
}

/// [`usage_list`] read fresh from disk (for other processes, e.g. Studio).
pub fn usage_list_from_disk(cfg: &ShellConfig, limit: usize) -> Vec<(String, f64)> {
    let (t, h) = (now(), cfg.launcher.usage.half_life_days);
    let usage: Usage = usage_store::read_file(&usage_path());
    let mut rows: Vec<(String, f64)> = usage.entries.keys().map(|k| (k.clone(), usage.score(k, t, h))).collect();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    rows.truncate(limit);
    rows
}

// ── Remembered query ─────────────────────────────────────────────────────

fn last_query_slot() -> &'static Mutex<String> {
    static SLOT: OnceLock<Mutex<String>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(String::new()))
}

pub fn remember_query(q: &str) {
    if let Ok(mut s) = last_query_slot().lock() {
        *s = q.to_string();
    }
}

pub fn last_query() -> String {
    last_query_slot().lock().map(|s| s.clone()).unwrap_or_default()
}

// ── Commands ─────────────────────────────────────────────────────────────

/// The desktop id of an installed app matching `name` (id or display name,
/// any case).
fn find_app_id(name: &str) -> Result<String, String> {
    let apps = crate::apps::discover();
    let n = name.trim();
    apps.iter()
        .find(|a| a.id.eq_ignore_ascii_case(n))
        .or_else(|| apps.iter().find(|a| a.name.eq_ignore_ascii_case(n)))
        .map(|a| a.id.clone())
        .ok_or_else(|| mandara_i18n::tf("No installed app {name} (use its desktop id, e.g. org.gnome.Nautilus)", &[("name", &format!("{n:?}"))]))
}

/// `launcher pin|unpin|hide|unhide|rename|icon|usage|emoji …`.
pub fn register(r: &mut mandara_plugins::PluginRegistry) {
    use mandara_plugins::{ArgKind, ArgSpec, CommandSpec};
    use serde_json::json;
    const OWNER: &str = crate::plugins::session::LAUNCHER_ID;
    // Pinned / hidden lists: add or remove a desktop id.
    for (path, summary, hidden, add) in [
        ("launcher pin", tk("Pin an app to the top of the launcher"), false, true),
        ("launcher unpin", tk("Unpin an app"), false, false),
        ("launcher hide", tk("Never list an app in the launcher"), true, true),
        ("launcher unhide", tk("List a hidden app again"), true, false),
    ] {
        r.command(
            OWNER,
            CommandSpec::new(path, summary, move |a, cx| {
                // Unhiding / unpinning takes the id as stored (the app may be hidden from discovery).
                let id = if add { find_app_id(&a.rest(0))? } else { a.rest(0).trim().to_string() };
                let mut changed = false;
                let out = id.clone();
                crate::state::update_config(cx, |c| {
                    let list = if hidden { &mut c.launcher.hidden } else { &mut c.launcher.pinned };
                    let at = list.iter().position(|x| x.eq_ignore_ascii_case(&id));
                    match (add, at) {
                        (true, None) => {
                            list.push(id);
                            changed = true;
                        }
                        (false, Some(i)) => {
                            list.remove(i);
                            changed = true;
                        }
                        _ => {}
                    }
                });
                let list = { let c = crate::state::config(cx).read(cx).get(); if hidden { c.launcher.hidden.clone() } else { c.launcher.pinned.clone() } };
                let mut reply = json!({ "app": out, "changed": changed });
                reply[if hidden { "hidden" } else { "pinned" }] = json!(list);
                Ok(reply)
            })
            .arg(ArgSpec::required("app", ArgKind::String, tk("desktop id or app name"))),
        );
    }
    r.command(
        OWNER,
        CommandSpec::new("launcher rename", tk("Show an app under your own name (empty name restores it)"), |a, cx| {
            let id = find_app_id(a.str(0)?)?;
            let name = a.rest(1);
            crate::state::update_config(cx, |c| {
                let o = c.launcher.overrides.entry(id.clone()).or_default();
                o.name = name.trim().to_string();
                if o.name.is_empty() && o.icon.is_empty() {
                    c.launcher.overrides.remove(&id);
                }
            });
            Ok(json!({ "app": id, "name": name.trim() }))
        })
        .arg(ArgSpec::required("app", ArgKind::String, tk("desktop id or app name")))
        .arg(ArgSpec::optional("name", ArgKind::String, tk("the new name")))
        .example("launcher rename org.gnome.Nautilus Files"),
    );
    r.command(
        OWNER,
        CommandSpec::new("launcher icon", tk("Give an app your own icon (theme icon name or image path; empty restores it)"), |a, cx| {
            let id = find_app_id(a.str(0)?)?;
            let icon = a.rest(1);
            crate::state::update_config(cx, |c| {
                let o = c.launcher.overrides.entry(id.clone()).or_default();
                o.icon = icon.trim().to_string();
                if o.name.is_empty() && o.icon.is_empty() {
                    c.launcher.overrides.remove(&id);
                }
            });
            Ok(json!({ "app": id, "icon": icon.trim() }))
        })
        .arg(ArgSpec::required("app", ArgKind::String, tk("desktop id or app name")))
        .arg(ArgSpec::optional("icon", ArgKind::String, tk("icon name or image path")))
        .example("launcher icon firefox ~/Pictures/fox.png"),
    );
    r.command(
        OWNER,
        CommandSpec::new_output("launcher usage list", tk("Most launched results with their frecency scores"), |_, cx| {
            let half = crate::state::config(cx).read(cx).get().launcher.usage.half_life_days;
            let ready = usage_ready();
            let (send, receive) = async_channel::bounded(1);
            cx.spawn(async move |cx| {
                let timer = cx.background_executor().timer(crate::overlays::LAUNCHER_DATA_TIMEOUT);
                if !crate::overlays::wait_for_catalogs(vec![ready], timer).await {
                    let _ = send.try_send(Err(mandara_i18n::t("The service did not reply").to_string()));
                    return;
                }
                if send.is_closed() { return; }
                cx.update(|_| {
                    let rows: Vec<serde_json::Value> = usage_list_with_half(half, PROVIDER_LIMIT).into_iter().map(|(k, s)| json!({ "key": k, "score": (s * 100.0).round() / 100.0 })).collect();
                    let _ = send.try_send(Ok(json!(rows).to_string()));
                });
            }).detach();
            Ok(receive.into())
        }),
    );
    r.command(OWNER, CommandSpec::new_output("launcher usage reset", tk("Forget every recorded launch (ranking starts over)"), |_, cx| {
        let ready = usage_ready();
        let (send, receive) = async_channel::bounded(1);
        cx.spawn(async move |cx| {
            let timer = cx.background_executor().timer(crate::overlays::LAUNCHER_DATA_TIMEOUT);
            if !crate::overlays::wait_for_catalogs(vec![ready], timer).await {
                let _ = send.try_send(Err(mandara_i18n::t("The service did not reply").to_string()));
                return;
            }
            if send.is_closed() { return; }
            cx.update(|_| { let _ = send.try_send(Ok(json!({ "forgotten": reset_usage() }).to_string())); });
        }).detach();
        Ok(receive.into())
    }));
    r.command(
        OWNER,
        CommandSpec::new("launcher emoji", tk("Find emoji by English or Arabic name"), |a, _| {
            let rows: Vec<serde_json::Value> = search_emoji(&a.rest(0), PROVIDER_LIMIT).into_iter().map(|e| json!({ "emoji": e.glyph, "name": e.name, "arabic": e.arabic })).collect();
            Ok(json!(rows))
        })
        .arg(ArgSpec::required("query", ArgKind::String, tk("words to look for")))
        .example("launcher emoji heart")
        .example("launcher emoji قمر"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_ipc_survives_shared_view_retirement_and_replacement() {
        for replace in [false, true] {
            let mut catalogs = BackgroundSnapshot::<u32, ()> { views: 1, ..Default::default() };
            let (_first, first) = Snapshot::new();
            let (_second, second) = Snapshot::new();
            catalogs.running.extend([(1, first), (2, second)]);
            let view = catalogs.request(3, true);
            let command = catalogs.request(3, false);
            assert!(Arc::ptr_eq(&view, &command));
            if replace { catalogs.request(4, true); }
            catalogs.leave();
            assert!(!command.ready().is_closed());
            catalogs.running.clear();
            let job = catalogs.next().unwrap();
            assert_eq!(job.key, 3);
            catalogs.complete(job.key, &job.snapshot, Some(Arc::new(())));
            assert!(command.value().is_some());
        }
    }

    #[test]
    fn hidden_launchers_release_catalogs_without_losing_pending_command_results() {
        let mut catalogs = BackgroundSnapshot::<u32, String> { views: 2, ..Default::default() };
        let (_done, snapshot) = Snapshot::new();
        catalogs.running.push((1, snapshot.clone()));
        catalogs.complete(1, &snapshot, Some(Arc::new("cached".into())));
        catalogs.leave();
        assert_eq!(catalogs.values.len(), 1, "another output still uses the cache");
        let (_done, pending) = Snapshot::new();
        catalogs.running.push((2, pending.clone()));
        let queued = catalogs.request(3, true);
        catalogs.leave();
        assert!(catalogs.values.is_empty() && catalogs.queued_view.is_none());
        assert!(queued.ready().is_closed());
        catalogs.complete(2, &pending, Some(Arc::new("command result".into())));
        assert_eq!(pending.value().as_deref().map(String::as_str), Some("command result"));
        assert!(catalogs.values.is_empty() && catalogs.running.capacity() == 0);
    }

    #[test]
    fn snapshot_keeps_its_value_after_completion_and_distinguishes_cancellation() {
        let (done, snapshot) = Snapshot::new();
        let second_waiter = snapshot.clone();
        assert!(!second_waiter.ready().is_closed());
        let _ = snapshot.value.set(Arc::new("first result"));
        done.close();
        assert!(second_waiter.ready().is_closed());
        assert_eq!(second_waiter.value().as_deref(), Some(&"first result"));

        let (cancel, snapshot) = Snapshot::<String>::new();
        drop(cancel);
        assert!(snapshot.ready().is_closed());
        assert!(snapshot.value().is_none());
    }

    #[test]
    fn ipc_refreshes_cannot_displace_the_open_launchers_queued_catalog() {
        let mut catalogs = BackgroundSnapshot::<_, ()>::default();
        let (_first, first) = Snapshot::new();
        let (_second, second) = Snapshot::new();
        catalogs.running.extend([(1, first), (2, second)]);

        let shared = catalogs.request(3, false);
        let view = catalogs.request(3, true);
        let old_ipc = catalogs.request(4, false);
        let ipc = catalogs.request(5, false);
        assert!(Arc::ptr_eq(&view, &shared));
        assert!(old_ipc.ready().is_closed());
        assert!(!view.ready().is_closed());
        assert!(!ipc.ready().is_closed());
        assert!(catalogs.next().is_none());

        catalogs.running.pop();
        let next = catalogs.next().expect("the view keeps its place");
        assert_eq!(next.key, 3);
        assert!(Arc::ptr_eq(&view, &next.snapshot));
        assert_eq!(catalogs.next().map(|job| job.key), Some(5));
    }

    #[test]
    fn routes_by_longest_prefix() {
        let mut p = LauncherProviders::default();
        assert_eq!(route(":smile", &p), Some(("emoji", "smile")));
        assert_eq!(route("  @term", &p), Some(("windows", "term")));
        assert_eq!(route("firefox", &p), None);
        p.emoji.enabled = false;
        assert_eq!(route(":smile", &p), None);
        p.files.prefix = "::".into();
        p.emoji.enabled = true;
        assert_eq!(route("::notes", &p), Some(("files", "notes")));
        assert!(globals(&LauncherProviders::default()).any(|g| g == "windows"));
    }

    #[test]
    fn emoji_table_and_search() {
        let t = emoji_table();
        assert!(t.len() > 1000, "{} emoji", t.len());
        assert!(t.iter().all(|e| !e.glyph.is_empty() && !e.name.is_empty()));
        assert_eq!(search_emoji("cat face", 1)[0].name, "cat face");
        // Arabic keywords find emoji too.
        assert!(search_emoji("قطة", 10).iter().any(|e| e.name.contains("cat")));
        assert!(search_emoji("قلب", 10).iter().any(|e| e.name.contains("heart")));
        assert!(search_emoji("zzqqx", 5).is_empty());
        let hits = emoji_hits("thumbs up", 3);
        assert_eq!(hits[0].action, HitAction::Copy("👍".into()));
    }

    #[test]
    fn frecency_decays_and_prunes() {
        let mut u = Usage::default();
        u.record("app:zed", 0, 1.0);
        u.record("app:zed", 0, 1.0);
        assert!((u.score("app:zed", 0, 1.0) - 2.0).abs() < 1e-9);
        // One half-life later the weight halved.
        assert!((u.score("app:zed", 86_400, 1.0) - 1.0).abs() < 1e-9);
        u.record("app:fox", 86_400, 1.0);
        assert!(u.score("app:fox", 86_400, 1.0) == u.score("app:zed", 86_400, 1.0));
        u.prune(86_400 * 30, 1.0);
        assert!(u.entries.is_empty());
    }

    #[test]
    fn browse_order_puts_pinned_then_recent() {
        let app = |id: &str| App { id: id.into(), name: id.into(), ..Default::default() };
        let apps = vec![app("a"), app("b"), app("c"), app("d")];
        let usage = UsageSnapshot([("app:c".to_string(), 5), ("app:a".to_string(), 9)].into_iter().collect());
        let (order, pinned, recent) = browse_order(&apps, &["d".into(), "missing".into()], &usage, 1);
        assert_eq!(order.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["d", "a", "b", "c"]);
        assert_eq!((pinned, recent), (1, 1));
    }

    #[test]
    fn customization_hides_and_renames() {
        let mut cfg = LauncherConfig { hidden: vec!["B".into()], ..Default::default() };
        cfg.overrides.insert("a".into(), mandara_config::workflow::AppOverride { name: "Alpha".into(), icon: String::new() });
        let apps = vec![App { id: "a".into(), name: "a".into(), ..Default::default() }, App { id: "b".into(), ..Default::default() }];
        let out = customize(apps, &cfg);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "Alpha");
    }

    #[test]
    fn xbel_and_files() {
        let xbel = r#"<bookmark href="file:///home/u/My%20Doc.txt" added="x"/><bookmark href="file:///tmp/a.png"/>"#;
        assert_eq!(parse_xbel(xbel), [PathBuf::from("/home/u/My Doc.txt"), PathBuf::from("/tmp/a.png")]);
        let recent = [
            FileEntry { path: "/tmp/report final.pdf".into(), directory: false },
            FileEntry { path: "/tmp/archive folder".into(), directory: true },
        ];
        let hits = file_hits("report", &recent, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].action, HitAction::Open(PathBuf::from("/tmp/report final.pdf")));
        assert_eq!(file_hits("archive", &recent, 5)[0].lead, Lead::Icon("folder"));
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        std::fs::create_dir_all(dir.join("sub/.hidden")).unwrap();
        std::fs::write(dir.join("sub/f.txt"), "").unwrap();
        let found = walk(std::slice::from_ref(&dir), 3, 100);
        assert!(found.iter().any(|file| file.path.ends_with("sub/f.txt") && !file.directory));
        assert!(found.iter().any(|file| file.path.ends_with("sub") && file.directory));
        assert!(!found.iter().any(|file| file.path.to_string_lossy().contains(".hidden")));
    }
}
