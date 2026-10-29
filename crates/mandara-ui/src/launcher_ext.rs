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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
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
/// Seconds per day (usage decay is configured in days).
const SECS_PER_DAY: f64 = 86_400.0;
/// Usage scores below this are forgotten when saving.
const FORGET_BELOW: f64 = 0.02;
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
}

/// What activating a provider result does.
#[derive(Clone, Debug, PartialEq)]
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
pub fn window_hits(query: &str, windows: &[mandara_services::toplevels::Toplevel], limit: usize) -> Vec<Hit> {
    let q = query.trim().to_lowercase();
    windows
        .iter()
        .filter(|w| !crate::windows::is_shell(&w.app_id))
        .filter_map(|w| {
            let app = crate::plugins::window::app_name(&w.app_id);
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

pub fn clipboard_hits(query: &str, entries: &[mandara_services::ClipEntry], limit: usize) -> Vec<Hit> {
    use mandara_services::clipboard::ClipContent;
    let q = query.trim().to_lowercase();
    entries
        .iter()
        .filter(|e| match &e.content {
            ClipContent::Text { text } => q.is_empty() || text.to_lowercase().contains(&q),
            ClipContent::Image { .. } => q.is_empty() || "image picture".contains(q.as_str()),
        })
        .take(limit)
        .map(|e| {
            let (lead, label) = match &e.content {
                ClipContent::Image { path, mime } => (Lead::Image(path.clone()), mandara_i18n::tf("Image ({mime})", &[("mime", mime)])),
                ClipContent::Text { .. } => (Lead::Icon(if e.pinned { "pin" } else { "clipboard" }), e.preview(90)),
            };
            Hit {
                provider: "clipboard",
                key: e.id.to_string(),
                label,
                detail: if e.app_id.is_empty() { String::new() } else { crate::plugins::window::app_name(&e.app_id) },
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

/// Recently used files from `~/.local/share/recently-used.xbel`, newest last
/// in the file → newest first here.
pub fn recent_files() -> Vec<PathBuf> {
    let Some(path) = dirs::data_dir().map(|d| d.join("recently-used.xbel")) else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    parse_xbel(&text).into_iter().rev().filter(|p| p.exists()).take(RECENT_FILES_MAX).collect()
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
    files: Vec<PathBuf>,
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
pub fn walk(roots: &[PathBuf], depth: usize, max: usize) -> Vec<PathBuf> {
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
                let p = e.path();
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    next.push(p.clone());
                }
                out.push(p);
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
    let stale = file_index().lock().map(|i| i.as_ref().is_none_or(|i| i.built.elapsed() > FILE_INDEX_TTL)).unwrap_or(false);
    if !stale {
        return;
    }
    // Mark as fresh at once so concurrent opens don't start a second walk.
    if let Ok(mut i) = file_index().lock() {
        let files = i.take().map(|i| i.files).unwrap_or_default();
        *i = Some(FileIndex { built: Instant::now(), files });
    }
    let _ = std::thread::Builder::new().name("mandara-file-index".into()).spawn(|| {
        let files = walk(&indexed_roots(), FILE_INDEX_DEPTH, FILE_INDEX_MAX);
        if let Ok(mut i) = file_index().lock() {
            *i = Some(FileIndex { built: Instant::now(), files });
        }
    });
}

fn file_hit(p: &Path) -> Hit {
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string());
    let home = dirs::home_dir().unwrap_or_default();
    let shown = p.parent().map(|d| d.strip_prefix(&home).map(|r| format!("~/{}", r.display())).unwrap_or_else(|_| d.display().to_string())).unwrap_or_default();
    Hit {
        provider: "files",
        key: p.display().to_string(),
        label: name,
        detail: shown,
        lead: Lead::Icon(if p.is_dir() { "folder" } else { "note" }),
        badge: if p.is_dir() { tk("Folder") } else { tk("File") },
        keywords: String::new(),
        action: HitAction::Open(p.to_path_buf()),
    }
}

/// Recent files first, then indexed ones, matching every typed word in the
/// file name (a query starting with `/` or `~/` matches whole paths).
pub fn file_hits(query: &str, recent: &[PathBuf], limit: usize) -> Vec<Hit> {
    let q = query.trim().to_lowercase();
    let by_path = q.starts_with('/') || q.starts_with("~/");
    let q = if let Some(rest) = q.strip_prefix("~/") { format!("{}/{rest}", dirs::home_dir().unwrap_or_default().display()).to_lowercase() } else { q };
    let matches = |p: &Path| {
        let hay = if by_path { p.display().to_string().to_lowercase() } else { p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default() };
        q.is_empty() || q.split_whitespace().all(|w| hay.contains(w))
    };
    let mut out: Vec<Hit> = recent.iter().filter(|p| matches(p)).take(limit).map(|p| file_hit(p)).collect();
    if !q.is_empty() && out.len() < limit {
        if let Ok(i) = file_index().lock()
            && let Some(index) = i.as_ref()
        {
            for p in index.files.iter().filter(|p| matches(p)) {
                if out.len() >= limit {
                    break;
                }
                if !recent.contains(p) {
                    out.push(file_hit(p));
                }
            }
        }
    }
    out
}

// ── App customization ────────────────────────────────────────────────────

/// Applies hidden apps and name / icon overrides to discovered apps.
pub fn customize(apps: Vec<App>, cfg: &LauncherConfig) -> Vec<App> {
    apps.into_iter()
        .filter(|a| !cfg.hidden.iter().any(|h| h.eq_ignore_ascii_case(&a.id)))
        .map(|mut a| {
            if let Some(o) = cfg.overrides.get(&a.id) {
                if !o.name.trim().is_empty() {
                    a.name = o.name.trim().to_string();
                }
                if !o.icon.trim().is_empty() {
                    let spec = o.icon.trim();
                    a.icon = if spec.starts_with('/') || spec.starts_with('~') { Some(mandara_config::paths::expand_tilde(spec)) } else { crate::apps::find_icon(spec) }.or(a.icon);
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
    recent.sort_by(|a, b| b.0.cmp(&a.0));
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

fn usage_store() -> &'static Mutex<Option<Usage>> {
    static STORE: OnceLock<Mutex<Option<Usage>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(None))
}

fn with_usage<T>(f: impl FnOnce(&mut Usage) -> T) -> T {
    let mut guard = usage_store().lock().unwrap_or_else(|e| e.into_inner());
    let usage = guard.get_or_insert_with(|| std::fs::read_to_string(usage_path()).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default());
    f(usage)
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
    let half_life = u.half_life_days;
    let snapshot = with_usage(|usage| {
        usage.record(key, now(), half_life);
        usage.prune(now(), half_life);
        usage.clone()
    });
    std::thread::spawn(move || {
        if let Ok(text) = toml::to_string(&snapshot) {
            let path = usage_path();
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = mandara_config::write_atomic(&path, text.as_bytes());
        }
    });
}

/// Frecency of every key right now (empty when sorting by usage is off).
pub fn usage_snapshot(cfg: &ShellConfig) -> UsageSnapshot {
    let u = &cfg.launcher.usage;
    if !u.sort {
        return UsageSnapshot::default();
    }
    let t = now();
    with_usage(|usage| UsageSnapshot(usage.entries.keys().map(|k| (k.clone(), (usage.score(k, t, u.half_life_days) * FRECENCY_SCALE) as i64)).collect()))
}

/// Forgets every recorded launch. Returns how many entries were dropped.
pub fn reset_usage() -> usize {
    let n = with_usage(|usage| std::mem::take(&mut usage.entries).len());
    let _ = std::fs::remove_file(usage_path());
    n
}

/// The most used keys with their scores (for `launcher usage list`).
pub fn usage_list(cfg: &ShellConfig, limit: usize) -> Vec<(String, f64)> {
    let (t, h) = (now(), cfg.launcher.usage.half_life_days);
    let mut rows: Vec<(String, f64)> = with_usage(|u| u.entries.keys().map(|k| (k.clone(), u.score(k, t, h))).collect());
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    rows.truncate(limit);
    rows
}

/// [`usage_list`] read fresh from disk (for other processes, e.g. Studio).
pub fn usage_list_from_disk(cfg: &ShellConfig, limit: usize) -> Vec<(String, f64)> {
    let (t, h) = (now(), cfg.launcher.usage.half_life_days);
    let usage: Usage = std::fs::read_to_string(usage_path()).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default();
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
        CommandSpec::new("launcher usage list", tk("Most launched results with their frecency scores"), |_, cx| {
            let cfg = crate::state::config(cx).read(cx).snapshot();
            let rows: Vec<serde_json::Value> = usage_list(&cfg, PROVIDER_LIMIT).into_iter().map(|(k, s)| json!({ "key": k, "score": (s * 100.0).round() / 100.0 })).collect();
            Ok(json!(rows))
        }),
    );
    r.command(OWNER, CommandSpec::new("launcher usage reset", tk("Forget every recorded launch (ranking starts over)"), |_, _| Ok(json!({ "forgotten": reset_usage() }))));
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
        let mut cfg = LauncherConfig::default();
        cfg.hidden = vec!["B".into()];
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
        let recent = vec![PathBuf::from("/tmp/report final.pdf"), PathBuf::from("/tmp/x.txt")];
        let hits = file_hits("report", &recent, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].action, HitAction::Open(PathBuf::from("/tmp/report final.pdf")));
        let dir = std::env::temp_dir().join(format!("mandara-walk-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub/.hidden")).unwrap();
        std::fs::write(dir.join("sub/f.txt"), "").unwrap();
        let found = walk(std::slice::from_ref(&dir), 3, 100);
        assert!(found.iter().any(|p| p.ends_with("sub/f.txt")));
        assert!(!found.iter().any(|p| p.to_string_lossy().contains(".hidden")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
