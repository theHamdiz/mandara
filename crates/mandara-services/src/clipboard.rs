//! Clipboard history, natively over Wayland: an ext-data-control or wlr-data-control
//! watcher reads every new selection in-process, entries (text and images)
//! persist under `~/.cache/mandara/clipboard/`, restores are served with
//! `wl-clipboard-rs`, and auto-paste types Ctrl+V through a
//! `zwp_virtual_keyboard_v1`. Entries flagged as secrets by password
//! managers are never recorded.
//!
//! What is kept follows [`ClipboardPolicy`] (Studio → Plugins → Clipboard):
//! size limits, pinned entries that survive clears, apps whose copies are
//! never recorded, age-based clearing, and re-offering the last copy when
//! the app it came from quits.
//!
//! Every entry is classified when it is copied ([`kind`]: text, link,
//! Markdown, code, CSS, colour, image or screenshot, files) and the class is
//! saved with it. Entries can be grouped into named [`ClipCollection`]s
//! (saved next to the history in `collections.json`); collected entries
//! survive clears like pinned ones.

mod color;
pub mod kind;
mod lang;
mod watch;

pub use kind::{ClipClass, Kind};

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::mpsc;
use std::time::Duration;

use mandara_config::workflow::ClipboardConfig;

use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{zwp_virtual_keyboard_manager_v1, zwp_virtual_keyboard_v1};

use crate::actor::{Envelope, ServiceHandle, reply};

/// Bytes per KiB / MiB (the policy's size limits are given in those).
const KIB: usize = 1024;
const MIB: usize = 1024 * 1024;
/// How often entries older than the auto-clear age are dropped.
const PRUNE_EVERY: Duration = Duration::from_secs(60);
/// Seconds per hour.
const SECS_PER_HOUR: i64 = 3600;
/// Hard ceiling of any selection read, whatever the policy says.
const READ_CEILING: usize = 256 * MIB;
/// A selection source that hasn't finished sending by then is given up on.
const READ_TIMEOUT: Duration = Duration::from_secs(3);
/// Chunk size for reading a selection out of its pipe.
const READ_CHUNK: usize = 64 * 1024;
/// Time for the launcher / panel to release keyboard focus before Ctrl+V.
const PASTE_DELAY: Duration = Duration::from_millis(180);
/// States buffered for the UI, commands from the UI / CLI, and selections
/// queued by the watcher.
const UPDATE_QUEUE: usize = 8;
const COMMAND_QUEUE: usize = 32;
const EVENT_QUEUE: usize = 16;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ClipContent {
    Text { text: String },
    Image { path: PathBuf, mime: String },
    /// Files copied in a file manager (`text/uri-list`): `file://` URIs.
    Files { uris: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipEntry {
    pub id: u64,
    pub content: ClipContent,
    /// Unix seconds.
    pub copied_at: i64,
    /// Pinned entries survive clears and never fall off the end.
    #[serde(default)]
    pub pinned: bool,
    /// App focused when it was copied (best effort; empty if unknown).
    #[serde(default)]
    pub app_id: String,
    /// What it holds, decided when it was copied.
    #[serde(default)]
    pub class: ClipClass,
    /// Ids of the [`ClipCollection`]s it belongs to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collections: Vec<u32>,
}

/// A named group of entries (Work, Palette, Snippets…).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipCollection {
    pub id: u32,
    pub name: String,
    /// Unix seconds.
    #[serde(default)]
    pub created: i64,
}

/// Longest collection name kept (characters).
pub const COLLECTION_NAME_MAX: usize = 40;

/// What the history keeps (from `[clipboard]`).
#[derive(Clone, Debug, PartialEq)]
pub struct ClipboardPolicy {
    pub enabled: bool,
    pub max_entries: usize,
    pub max_pinned: usize,
    /// Unpinned entries kept because they are in a collection.
    pub max_collected: usize,
    pub max_text: usize,
    pub images: bool,
    pub max_image: usize,
    pub persist: bool,
    pub keep_from_closed_apps: bool,
    pub exclude_password_managers: bool,
    /// Lowercase app ids whose copies are skipped.
    pub excluded_apps: Vec<String>,
    /// Unpinned entries older than this are dropped.
    pub max_age: Option<Duration>,
}

impl Default for ClipboardPolicy {
    fn default() -> Self {
        Self::from(&ClipboardConfig::default())
    }
}

impl From<&ClipboardConfig> for ClipboardPolicy {
    fn from(c: &ClipboardConfig) -> Self {
        Self {
            enabled: c.enabled,
            max_entries: c.max_entries.max(1),
            max_pinned: c.max_pinned,
            max_collected: c.max_collected,
            max_text: c.max_text_kb as usize * KIB,
            images: c.images,
            max_image: c.max_image_mb as usize * MIB,
            persist: c.persist,
            keep_from_closed_apps: c.keep_from_closed_apps,
            exclude_password_managers: c.exclude_password_managers,
            excluded_apps: c.exclude_apps.iter().map(|a| a.trim().to_lowercase()).filter(|a| !a.is_empty()).collect(),
            max_age: (c.auto_clear_hours > 0).then(|| Duration::from_secs(u64::from(c.auto_clear_hours) * SECS_PER_HOUR as u64)),
        }
    }
}

impl ClipboardPolicy {
    /// Whether copies made while `app_id` has focus are skipped.
    pub fn excludes(&self, app_id: &str) -> bool {
        let app = app_id.trim().to_lowercase();
        !app.is_empty() && self.excluded_apps.iter().any(|x| app == *x || app.contains(x.as_str()))
    }
}

/// Policy plus the focused app, shared with the selection watcher thread.
#[derive(Default)]
struct Shared {
    policy: ClipboardPolicy,
    focused_app: String,
}

impl ClipEntry {
    pub fn preview(&self, max: usize) -> String {
        let clip = |flat: String| if flat.chars().count() > max { format!("{}…", flat.chars().take(max).collect::<String>()) } else { flat };
        match &self.content {
            ClipContent::Text { text } => {
                // Fold whitespace over a bounded head only: a huge copy's
                // title never needs more than `max` characters.
                let head: String = text.chars().take(max.saturating_mul(PREVIEW_HEAD_FACTOR).max(max + 1)).collect();
                clip(head.split_whitespace().collect::<Vec<_>>().join(" "))
            }
            ClipContent::Image { mime, .. } => tf("Image ({mime})", &[("mime", mime)]),
            ClipContent::Files { .. } => clip(self.file_paths().iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).collect::<Vec<_>>().join(", ")),
        }
    }

    /// Local paths of a files entry (empty for anything else).
    pub fn file_paths(&self) -> Vec<PathBuf> {
        match &self.content {
            ClipContent::Files { uris } => uris.iter().map(|u| kind::uri_path(u)).collect(),
            _ => Vec::new(),
        }
    }

    /// Kept through clears and limits: pinned or in a collection.
    pub fn kept(&self) -> bool {
        self.pinned || !self.collections.is_empty()
    }

    /// Whether every word of `query` is found (case-insensitively) in the
    /// entry: its text, file paths, kind and language / host / notation,
    /// the app it came from or the names of its collections.
    pub fn matches(&self, query: &str, collections: &[ClipCollection]) -> bool {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return true;
        }
        let keywords = self.class.keywords();
        let names: Vec<&str> = collections.iter().filter(|c| self.collections.contains(&c.id)).map(|c| c.name.as_str()).collect();
        let paths: Vec<String> = self.file_paths().iter().map(|p| p.to_string_lossy().into_owned()).collect();
        words.iter().all(|w| {
            let content = match &self.content {
                ClipContent::Text { text } => contains_ci(text, w),
                ClipContent::Image { mime, .. } => contains_ci(mime, w),
                ClipContent::Files { .. } => paths.iter().any(|p| contains_ci(p, w)),
            };
            content || contains_ci(&keywords, w) || contains_ci(&self.app_id, w) || names.iter().any(|n| contains_ci(n, w))
        })
    }
}

/// How many characters of a text's head the one-line preview folds, per
/// character shown (whitespace runs collapse, so a little more is read).
const PREVIEW_HEAD_FACTOR: usize = 4;

/// Case-insensitive substring test; `needle` is already lower-case. ASCII
/// needles compare in place, without lower-casing the haystack.
fn contains_ci(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.is_ascii() {
        let (h, n) = (hay.as_bytes(), needle.as_bytes());
        return h.len() >= n.len() && h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n));
    }
    hay.to_lowercase().contains(needle)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ClipboardState {
    pub available: bool,
    /// Newest first, shared by the actor, panels and launcher snapshots.
    pub entries: Arc<Vec<ClipEntry>>,
    /// In the order they were created.
    pub collections: Vec<ClipCollection>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClipboardCommand {
    /// Put an entry back on the clipboard; optionally paste it (Ctrl+V).
    Restore { id: u64, paste: bool },
    Delete(u64),
    /// Forget the history; pinned entries stay unless `pinned`.
    Clear { pinned: bool },
    Pin { id: u64, pinned: bool },
    /// A new, empty collection (replies `{"id", "name"}`).
    CreateCollection(String),
    RenameCollection { id: u32, name: String },
    /// Removes the collection; its entries stay in the history.
    DeleteCollection(u32),
    /// Adds an entry to a collection (or takes it out).
    Collect { id: u64, collection: u32, add: bool },
    /// New settings (applied to the history at once).
    Configure(ClipboardConfig),
    /// The focused window's app id (for per-app exclusion).
    FocusedApp(String),
}

pub type ClipboardService = ServiceHandle<ClipboardState, ClipboardCommand>;

fn store_dir() -> PathBuf {
    mandara_config::paths::cache_dir().join("clipboard")
}

fn hash(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// Whether the offer is flagged as a secret by a password manager.
pub fn is_secret(types: &[&str]) -> bool {
    types.iter().any(|t| t.contains("passwordManagerHint") || *t == "x-kde-passwordManagerHint")
}

/// File-manager copies: a URI list, or GNOME's `copy\n<uris>` form.
const URI_LIST: &str = "text/uri-list";
const GNOME_FILES: &str = "x-special/gnome-copied-files";

/// Decides what (if anything) to record from the offered MIME types:
/// `image/png`, another `image`, `files` (a URI list, read and checked
/// before falling back to its text) or `text`.
pub fn classify(types: &[&str]) -> Option<&'static str> {
    if is_secret(types) {
        return None;
    }
    if types.contains(&"image/png") {
        Some("image/png")
    } else if types.iter().any(|t| t.starts_with("image/")) {
        Some("image")
    } else if types.iter().any(|t| *t == URI_LIST || *t == GNOME_FILES) {
        Some("files")
    } else if types.iter().any(|t| t.starts_with("text/") || *t == "UTF8_STRING" || *t == "STRING") {
        Some("text")
    } else {
        None
    }
}

const HISTORY_FILE: &str = "history.json";
const COLLECTIONS_FILE: &str = "collections.json";

/// The saved history in `dir`, every entry (re)classified: entries from
/// before classification get their kind, and text follows the current rules.
fn load(dir: &Path) -> Vec<ClipEntry> {
    let mut entries: Vec<ClipEntry> = std::fs::read_to_string(dir.join(HISTORY_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    for e in &mut entries {
        e.class = match &e.content {
            ClipContent::Text { text } => kind::classify_text(text),
            ClipContent::Files { uris } => ClipClass::Files { count: uris.len() },
            ClipContent::Image { .. } if matches!(e.class, ClipClass::Image { .. }) => continue,
            ClipContent::Image { path, .. } => kind::classify_image_file(path, &e.app_id),
        };
    }
    entries
}

/// Writes `bytes` to `dir/name` atomically (temp file + rename).
fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) {
    if std::fs::create_dir_all(dir).is_ok() {
        let tmp = dir.join(format!("{name}.tmp"));
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(tmp, dir.join(name));
        }
    }
}

fn save(dir: &Path, entries: &[ClipEntry]) {
    if let Ok(json) = serde_json::to_vec(entries) {
        write_atomic(dir, HISTORY_FILE, &json);
    }
}

/// Collections are small and the user's own organization: saved even when
/// the history itself is kept in memory only.
fn load_collections(dir: &Path) -> Vec<ClipCollection> {
    std::fs::read_to_string(dir.join(COLLECTIONS_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_collections(dir: &Path, collections: &[ClipCollection]) {
    if let Ok(json) = serde_json::to_vec_pretty(collections) {
        write_atomic(dir, COLLECTIONS_FILE, &json);
    }
}

/// Forgets memberships of collections that no longer exist.
fn reconcile(entries: &mut [ClipEntry], collections: &[ClipCollection]) {
    for e in entries {
        e.collections.retain(|id| collections.iter().any(|c| c.id == *id));
    }
}

/// Picks the MIME type to read for a classification.
fn pick_mime<'a>(kind: &str, types: &[&'a str]) -> Option<&'a str> {
    match kind {
        "text" => ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "STRING", "TEXT"]
            .iter()
            .find_map(|want| types.iter().find(|t| *t == want).copied())
            .or_else(|| types.iter().find(|t| t.starts_with("text/") && **t != URI_LIST).copied()),
        "files" => types.iter().find(|t| **t == URI_LIST).or_else(|| types.iter().find(|t| **t == GNOME_FILES)).copied(),
        "image/png" => types.iter().find(|t| **t == "image/png").copied(),
        _ => types.iter().find(|t| t.starts_with("image/")).copied(),
    }
}

/// The `file://` URIs of a URI list or GNOME copy (`copy\n…` / `cut\n…`).
fn read_uris(bytes: &[u8]) -> Option<Vec<String>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let text = text.strip_prefix("copy\n").or_else(|| text.strip_prefix("cut\n")).unwrap_or(text);
    kind::file_uris(text)
}

/// Builds an entry from the offered types under `policy`, reading the
/// chosen type with `read` (which returns the raw bytes) and classifying
/// what it got.
fn capture(types: &[&str], policy: &ClipboardPolicy, app_id: &str, mut read: impl FnMut(&str) -> Option<Vec<u8>>) -> Option<ClipEntry> {
    if !policy.enabled || policy.excludes(app_id) {
        return None;
    }
    let open: Vec<&str> = if policy.exclude_password_managers {
        types.to_vec()
    } else {
        // Without the password-manager filter, classify as if the hint were absent.
        types.iter().copied().filter(|t| !is_secret(&[t])).collect()
    };
    let mut kind = classify(&open)?;
    if kind.starts_with("image") && !policy.images {
        return None;
    }
    let mut content = None;
    if kind == "files" {
        // A URI list of web links (browsers offer one) is just text.
        content = pick_mime(kind, types).and_then(&mut read).and_then(|b| read_uris(&b)).map(|uris| ClipContent::Files { uris });
        if content.is_none() {
            kind = "text";
        }
    }
    let content = match content {
        Some(c) => c,
        None => {
            let mime = pick_mime(kind, types)?;
            let bytes = read(mime)?;
            if kind == "text" {
                if bytes.is_empty() || bytes.len() > policy.max_text {
                    return None;
                }
                let text = String::from_utf8(bytes).ok()?;
                if text.trim().is_empty() {
                    return None;
                }
                ClipContent::Text { text }
            } else {
                if bytes.is_empty() || bytes.len() > policy.max_image {
                    return None;
                }
                let ext = mime.trim_start_matches("image/").split(';').next().unwrap_or("img");
                let dir = store_dir().join("images");
                std::fs::create_dir_all(&dir).ok()?;
                let path = dir.join(format!("{:016x}.{ext}", hash(&bytes)));
                if !path.exists() {
                    std::fs::write(&path, &bytes).ok()?;
                }
                let class = kind::classify_image(&bytes, app_id);
                return Some(entry(ClipContent::Image { path, mime: mime.to_string() }, class, app_id));
            }
        }
    };
    let class = match &content {
        ClipContent::Text { text } => kind::classify_text(text),
        ClipContent::Files { uris } => ClipClass::Files { count: uris.len() },
        ClipContent::Image { path, .. } => kind::classify_image_file(path, app_id),
    };
    Some(entry(content, class, app_id))
}

/// A new entry, keyed by its content.
fn entry(content: ClipContent, class: ClipClass, app_id: &str) -> ClipEntry {
    let id = match &content {
        ClipContent::Text { text } => hash(text.as_bytes()),
        ClipContent::Image { path, .. } => hash(path.to_string_lossy().as_bytes()),
        ClipContent::Files { uris } => hash(uris.join("\n").as_bytes()),
    };
    ClipEntry { id, content, copied_at: chrono::Local::now().timestamp(), pinned: false, app_id: app_id.to_string(), class, collections: Vec::new() }
}

/// Inserts newest-first, de-duplicating by id (a re-copied entry keeps its
/// pin and collections) and enforcing the policy's limits.
pub fn push(entries: &mut Vec<ClipEntry>, mut entry: ClipEntry, policy: &ClipboardPolicy) {
    if let Some(old) = entries.iter().find(|e| e.id == entry.id) {
        entry.pinned |= old.pinned;
        for c in &old.collections {
            if !entry.collections.contains(c) {
                entry.collections.push(*c);
            }
        }
    }
    entries.retain(|e| e.id != entry.id);
    entries.insert(0, entry);
    enforce_limits(entries, policy);
}

/// Caps pinned entries at `max_pinned`, other collected ones at
/// `max_collected` and the rest at `max_entries` (the oldest go first),
/// and drops entries that are neither pinned nor collected past the max age.
pub fn enforce_limits(entries: &mut Vec<ClipEntry>, policy: &ClipboardPolicy) {
    let now = chrono::Local::now().timestamp();
    let max_age = policy.max_age.map(|d| d.as_secs() as i64);
    let (mut pinned, mut collected, mut loose) = (0usize, 0usize, 0usize);
    entries.retain(|e| {
        if e.pinned {
            pinned += 1;
            pinned <= policy.max_pinned
        } else if !e.collections.is_empty() {
            collected += 1;
            collected <= policy.max_collected
        } else {
            loose += 1;
            loose <= policy.max_entries && !past_age(e, now, max_age)
        }
    });
}

/// Only loose entries age out; pinned and collected entries remain available.
fn past_age(entry: &ClipEntry, now: i64, max_age: Option<i64>) -> bool {
    !entry.kept() && max_age.is_some_and(|age| now.saturating_sub(entry.copied_at) > age)
}

/// Leave the shared allocation intact when a timer has nothing to expire.
fn prune_shared(entries: &mut Arc<Vec<ClipEntry>>, policy: &ClipboardPolicy, now: i64) -> bool {
    let max_age = policy.max_age.map(|d| d.as_secs() as i64);
    if !entries.iter().any(|entry| past_age(entry, now, max_age)) {
        return false;
    }
    Arc::make_mut(entries).retain(|entry| !past_age(entry, now, max_age));
    true
}

/// Trims a collection name to its limit; `None` when nothing is left.
fn collection_name(name: &str) -> Option<String> {
    let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(COLLECTION_NAME_MAX).collect();
    (!name.is_empty()).then_some(name)
}

/// Applies a collection command to the collections and entries; returns the
/// reply (JSON for a created or renamed collection).
fn edit_collections(command: &ClipboardCommand, collections: &mut Vec<ClipCollection>, entries: &mut [ClipEntry]) -> Result<String, String> {
    let missing = |id: u32| tf("No clipboard collection {id}", &[("id", &id)]);
    let taken = |name: &str, except: Option<u32>| collections.iter().any(|c| Some(c.id) != except && c.name.eq_ignore_ascii_case(name));
    match command {
        ClipboardCommand::CreateCollection(name) => {
            let name = collection_name(name).ok_or_else(|| t("A collection needs a name").to_string())?;
            if taken(&name, None) {
                return Err(tf("There is already a collection called {name}", &[("name", &name)]));
            }
            let id = collections.iter().map(|c| c.id).max().unwrap_or(0) + 1;
            collections.push(ClipCollection { id, name: name.clone(), created: chrono::Local::now().timestamp() });
            Ok(serde_json::json!({ "id": id, "name": name }).to_string())
        }
        ClipboardCommand::RenameCollection { id, name } => {
            let name = collection_name(name).ok_or_else(|| t("A collection needs a name").to_string())?;
            if taken(&name, Some(*id)) {
                return Err(tf("There is already a collection called {name}", &[("name", &name)]));
            }
            let c = collections.iter_mut().find(|c| c.id == *id).ok_or_else(|| missing(*id))?;
            c.name = name.clone();
            Ok(serde_json::json!({ "id": id, "name": name }).to_string())
        }
        ClipboardCommand::DeleteCollection(id) => {
            let before = collections.len();
            collections.retain(|c| c.id != *id);
            if before == collections.len() {
                return Err(missing(*id));
            }
            for e in entries.iter_mut() {
                e.collections.retain(|c| c != id);
            }
            Ok("deleted".into())
        }
        ClipboardCommand::Collect { id, collection, add } => {
            if !collections.iter().any(|c| c.id == *collection) {
                return Err(missing(*collection));
            }
            let e = entries.iter_mut().find(|e| e.id == *id).ok_or_else(|| tf("No clipboard entry {id}", &[("id", id)]))?;
            let had = e.collections.contains(collection);
            if *add && !had {
                e.collections.push(*collection);
            } else if !*add {
                e.collections.retain(|c| c != collection);
            }
            Ok(if *add { "added".into() } else { "removed".into() })
        }
        _ => Err("not a collection command".into()),
    }
}

/// Removes image files no entry refers to any more.
fn sweep_images(entries: &[ClipEntry]) {
    let Ok(dir) = std::fs::read_dir(store_dir().join("images")) else { return };
    let keep: std::collections::HashSet<PathBuf> = entries
        .iter()
        .filter_map(|e| match &e.content {
            ClipContent::Image { path, .. } => Some(path.clone()),
            ClipContent::Text { .. } | ClipContent::Files { .. } => None,
        })
        .collect();
    for f in dir.flatten() {
        if !keep.contains(&f.path()) {
            let _ = std::fs::remove_file(f.path());
        }
    }
}

/// Takes ownership of the clipboard with `bytes`, serving it from a
/// background thread until another client replaces the selection.
pub fn serve(bytes: Vec<u8>, mime: Option<String>) -> Result<(), String> {
    serve_all(vec![(bytes, mime)])
}

/// Like [`serve`], offering the same data in several MIME types at once
/// (copied files: a URI list, GNOME's form and plain paths).
pub fn serve_all(sources: Vec<(Vec<u8>, Option<String>)>) -> Result<(), String> {
    use wl_clipboard_rs::copy::{MimeSource, MimeType, Options, Source};
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("mandara-clipboard-serve".into())
        .spawn(move || {
            let mut opts = Options::new();
            opts.foreground(true);
            let sources: Vec<MimeSource> = sources
                .into_iter()
                .map(|(bytes, mime)| MimeSource {
                    source: Source::Bytes(bytes.into_boxed_slice()),
                    mime_type: match mime {
                        Some(m) => MimeType::Specific(m),
                        None => MimeType::Text,
                    },
                })
                .collect();
            // Preparing takes the selection; serving blocks until another
            // app replaces it. Report success / failure right after taking it.
            match opts.prepare_copy_multi(sources) {
                Ok(prepared) => {
                    let _ = tx.send(Ok(()));
                    if let Err(err) = prepared.serve() {
                        tracing::warn!("clipboard serve failed: {err}");
                    }
                }
                Err(err) => {
                    let _ = tx.send(Err(tf("Could not set the clipboard: {error}", &[("error", &err)])));
                }
            }
        })
        .map_err(|e| e.to_string())?;
    rx.recv().map_err(|_| t("The clipboard thread failed").to_string())?
}

fn restore(entry: &ClipEntry, paste: bool) -> Result<String, String> {
    match &entry.content {
        ClipContent::Text { text } => serve(text.clone().into_bytes(), None)?,
        ClipContent::Image { path, mime } => serve(std::fs::read(path).map_err(|e| e.to_string())?, Some(mime.clone()))?,
        ClipContent::Files { uris } => {
            let paths = entry.file_paths().iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n");
            serve_all(vec![
                (uris.join("\r\n").into_bytes(), Some(URI_LIST.into())),
                (format!("copy\n{}", uris.join("\n")).into_bytes(), Some(GNOME_FILES.into())),
                (paths.into_bytes(), Some("text/plain;charset=utf-8".into())),
            ])?
        }
    }
    if paste {
        // Give the launcher/panel time to release keyboard focus.
        std::thread::sleep(PASTE_DELAY);
        keyboard::ctrl_v()?;
    }
    Ok(if paste { "pasted".into() } else { "copied".into() })
}

/// Synthetic key presses over `zwp_virtual_keyboard_v1`.
pub mod keyboard {
    use super::*;

    type VkManager = zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
    type Vk = zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;

    /// evdev key codes.
    const KEY_LEFTCTRL: u32 = 29;
    const KEY_V: u32 = 47;
    /// `Control` modifier mask in the standard `complete` xkb types.
    const MOD_CTRL: u32 = 1 << 2;
    const KEYMAP: &str = "xkb_keymap {\n  xkb_keycodes { include \"evdev+aliases(qwerty)\" };\n  xkb_types { include \"complete\" };\n  xkb_compat { include \"complete\" };\n  xkb_symbols { include \"pc+us+inet(evdev)\" };\n};\n";

    struct State;
    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
        fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
    }
    impl Dispatch<wl_seat::WlSeat, ()> for State {
        fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }
    impl Dispatch<VkManager, ()> for State {
        fn event(_: &mut Self, _: &VkManager, _: zwp_virtual_keyboard_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }
    impl Dispatch<Vk, ()> for State {
        fn event(_: &mut Self, _: &Vk, _: zwp_virtual_keyboard_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }

    fn keymap_fd() -> Result<(std::os::fd::OwnedFd, u32), String> {
        let fd = rustix::fs::memfd_create("mandara-keymap", rustix::fs::MemfdFlags::CLOEXEC).map_err(|e| e.to_string())?;
        let mut file = std::fs::File::from(fd);
        let mut bytes = KEYMAP.as_bytes().to_vec();
        bytes.push(0);
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        Ok((file.into(), bytes.len() as u32))
    }

    /// Types Ctrl+V into the focused window.
    pub fn ctrl_v() -> Result<(), String> {
        chord(&["ctrl"], KEY_V)
    }

    /// evdev code for a modifier name.
    fn modifier(name: &str) -> Option<(u32, u32)> {
        // (evdev key, xkb modifier mask in the standard keymap)
        Some(match name {
            "shift" => (42, 1 << 0),
            "ctrl" => (KEY_LEFTCTRL, MOD_CTRL),
            "alt" => (56, 1 << 3),
            "super" => (125, 1 << 6),
            _ => return None,
        })
    }

    /// evdev code for a plain key name (letters, digits, a few named keys).
    pub fn key_code(name: &str) -> Option<u32> {
        const ROW1: &str = "qwertyuiop";
        const ROW2: &str = "asdfghjkl";
        const ROW3: &str = "zxcvbnm";
        let n = name.to_ascii_lowercase();
        if n.len() == 1 {
            let c = n.chars().next()?;
            if let Some(i) = ROW1.find(c) {
                return Some(16 + i as u32);
            }
            if let Some(i) = ROW2.find(c) {
                return Some(30 + i as u32);
            }
            if let Some(i) = ROW3.find(c) {
                return Some(44 + i as u32);
            }
            if let Some(d) = c.to_digit(10) {
                return Some(if d == 0 { 11 } else { 1 + d });
            }
        }
        Some(match n.as_str() {
            "space" => 57,
            "enter" | "return" => 28,
            "escape" | "esc" => 1,
            "tab" => 15,
            "backspace" => 14,
            "delete" => 111,
            "print" => 99,
            _ => return None,
        })
    }

    /// Presses `mods` + `key` (evdev code) on a virtual keyboard.
    pub fn chord(mods: &[&str], key: u32) -> Result<(), String> {
        chord_held(mods, key, std::time::Duration::ZERO)
    }

    /// Like [`chord`], but keeps `mods` held for `hold` after tapping `key`
    /// (a hand holding Alt after Alt+Tab).
    pub fn chord_held(mods: &[&str], key: u32, hold: std::time::Duration) -> Result<(), String> {
        let mods: Vec<(u32, u32)> = mods.iter().map(|m| modifier(m).ok_or(format!("unknown modifier {m:?}"))).collect::<Result<_, _>>()?;
        let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=8, ()).map_err(|e| format!("wl_seat: {e}"))?;
        let manager: VkManager = globals.bind(&qh, 1..=1, ()).map_err(|e| format!("zwp_virtual_keyboard_manager_v1: {e}"))?;
        let kb = manager.create_virtual_keyboard(&seat, &qh, ());
        let (fd, size) = keymap_fd()?;
        kb.keymap(1 /* XKB_V1 */, fd.as_fd(), size);
        let mut state = State;
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
        let mut t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u32).unwrap_or(0);
        let mut mask = 0;
        for (code, m) in &mods {
            kb.key(t, *code, 1);
            mask |= m;
            kb.modifiers(mask, 0, 0, 0);
            t += 1;
        }
        kb.key(t, key, 1);
        kb.key(t + 1, key, 0);
        t += 2;
        if !hold.is_zero() {
            queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
            std::thread::sleep(hold);
            t += hold.as_millis() as u32;
        }
        for (code, _) in mods.iter().rev() {
            kb.key(t, *code, 0);
            t += 1;
        }
        kb.modifiers(0, 0, 0, 0);
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
        kb.destroy();
        let _ = conn.flush();
        Ok(())
    }
}

pub fn spawn_clipboard_service() -> ClipboardService {
    spawn_clipboard_service_with(&ClipboardConfig::default())
}

/// Starts the clipboard history under `config` (later changes arrive as
/// [`ClipboardCommand::Configure`]).
pub fn spawn_clipboard_service_with(config: &ClipboardConfig) -> ClipboardService {
    let (tx, updates) = async_channel::bounded(UPDATE_QUEUE);
    let (commands, rx) = async_channel::bounded::<Envelope<ClipboardCommand>>(COMMAND_QUEUE);
    let (events_tx, events) = async_channel::bounded::<watch::Event>(EVENT_QUEUE);
    let start_policy = ClipboardPolicy::from(config);
    let clear_on_start = config.clear_on_start;
    let shared = Arc::new(Mutex::new(Shared { policy: start_policy.clone(), focused_app: String::new() }));
    let stop_watch = watch::start(events_tx, shared.clone());
    crate::runtime::spawn("clipboard", async move {
        let _stop_watch = stop_watch;
        use futures::FutureExt as _;
        let mut policy = start_policy;
        let mut available = false;
        let persist = policy.persist;
        let (mut entries, mut collections) = crate::runtime::unblock(move || (if persist { load(&store_dir()) } else { Vec::new() }, load_collections(&store_dir()))).await;
        reconcile(&mut entries, &collections);
        if clear_on_start {
            entries.retain(ClipEntry::kept);
        }
        enforce_limits(&mut entries, &policy);
        let mut entries = Arc::new(entries);
        {
            let (snapshot, persist) = (entries.clone(), policy.persist);
            crate::runtime::unblock(move || {
                if persist { save(&store_dir(), &snapshot) } else { forget_disk() }
                sweep_images(&snapshot);
            })
            .await;
        }
        let _ = tx.send(ClipboardState { available, entries: entries.clone(), collections: collections.clone() }).await;
        let mut events = events;
        // Keeps a never-firing replacement channel open once the watcher ends.
        let mut _parked: Option<async_channel::Sender<watch::Event>> = None;
        let mut prune = std::pin::pin!(crate::runtime::sleep(PRUNE_EVERY).fuse());
        loop {
            let mut changed = false;
            futures::select! {
                cmd = rx.recv().fuse() => {
                    let Ok(Envelope { command, reply: r }) = cmd else { return };
                    let result = match command {
                        ClipboardCommand::Restore { id, paste } => match entries.iter().find(|e| e.id == id).cloned() {
                            Some(entry) => crate::runtime::unblock(move || restore(&entry, paste)).await,
                            None => Err(tf("No clipboard entry {id}", &[("id", &id)])),
                        },
                        ClipboardCommand::Delete(id) => {
                            let before = entries.len();
                            if entries.iter().any(|e| e.id == id) {
                                Arc::make_mut(&mut entries).retain(|e| e.id != id);
                            }
                            changed = before != entries.len();
                            if changed { Ok("deleted".into()) } else { Err(tf("No clipboard entry {id}", &[("id", &id)])) }
                        }
                        ClipboardCommand::Clear { pinned } => {
                            if entries.iter().any(|e| !e.kept() || pinned) {
                                Arc::make_mut(&mut entries).retain(|e| e.kept() && !pinned);
                            }
                            changed = true;
                            Ok("cleared".into())
                        }
                        command @ (ClipboardCommand::CreateCollection(_) | ClipboardCommand::RenameCollection { .. } | ClipboardCommand::DeleteCollection(_) | ClipboardCommand::Collect { .. }) => {
                            match edit_collections(&command, &mut collections, Arc::<Vec<ClipEntry>>::make_mut(&mut entries).as_mut_slice()) {
                                Ok(reply) => {
                                    enforce_limits(Arc::make_mut(&mut entries), &policy);
                                    let snapshot = collections.clone();
                                    crate::runtime::unblock(move || save_collections(&store_dir(), &snapshot)).await;
                                    changed = true;
                                    Ok(reply)
                                }
                                Err(err) => Err(err),
                            }
                        }
                        ClipboardCommand::Pin { id, pinned } => match entries.iter().position(|e| e.id == id) {
                            Some(index) => {
                                Arc::make_mut(&mut entries)[index].pinned = pinned;
                                enforce_limits(Arc::make_mut(&mut entries), &policy);
                                changed = true;
                                Ok(if pinned { "pinned".into() } else { "unpinned".into() })
                            }
                            None => Err(tf("No clipboard entry {id}", &[("id", &id)])),
                        },
                        ClipboardCommand::Configure(config) => {
                            let next = ClipboardPolicy::from(&config);
                            changed = next != policy;
                            if let Ok(mut s) = shared.lock() {
                                s.policy = next.clone();
                            }
                            policy = next;
                            if changed { enforce_limits(Arc::make_mut(&mut entries), &policy); }
                            Ok("configured".into())
                        }
                        ClipboardCommand::FocusedApp(app) => {
                            if let Ok(mut s) = shared.lock() {
                                s.focused_app = app;
                            }
                            Ok("ok".into())
                        }
                    };
                    reply(&r, result);
                }
                ev = events.recv().fuse() => match ev {
                    Ok(watch::Event::Entry(entry)) => {
                        push(Arc::make_mut(&mut entries), entry, &policy);
                        changed = true;
                    }
                    Ok(watch::Event::Ready(ok)) => {
                        changed = available != ok;
                        available = ok;
                    }
                    Ok(watch::Event::Cleared) => {
                        // The app owning the selection quit: offer its last
                        // copy ourselves so it can still be pasted.
                        if policy.keep_from_closed_apps && let Some(last) = entries.first().cloned()
                            && let Err(err) = crate::runtime::unblock(move || restore(&last, false)).await {
                                tracing::debug!("clipboard: keeping the last copy failed: {err}");
                            }
                    }
                    // The watcher ended: keep serving commands (a closed
                    // channel would resolve immediately, so park a fresh one).
                    Err(_) => {
                        let (parked, never) = async_channel::bounded(1);
                        _parked = Some(parked);
                        events = never;
                    }
                },
                _ = prune.as_mut() => {
                    prune.set(crate::runtime::sleep(PRUNE_EVERY).fuse());
                    changed = prune_shared(&mut entries, &policy, chrono::Local::now().timestamp());
                }
            }
            if changed {
                let (snapshot, persist) = (entries.clone(), policy.persist);
                crate::runtime::unblock(move || {
                    if persist { save(&store_dir(), &snapshot) } else { forget_disk() }
                    sweep_images(&snapshot);
                })
                .await;
                if tx.send(ClipboardState { available, entries: entries.clone(), collections: collections.clone() }).await.is_err() {
                    return;
                }
            }
        }
    });
    ServiceHandle::new(updates, commands)
}

/// Deletes the saved history file (history kept in memory only).
fn forget_disk() {
    let _ = std::fs::remove_file(store_dir().join(HISTORY_FILE));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> ClipEntry {
        entry(ClipContent::Text { text: t.into() }, kind::classify_text(t), "")
    }

    #[test]
    fn history_snapshots_share_payloads_and_mutations_preserve_old_values() {
        let payload = "text".repeat(64 * 1024);
        let mut history = Arc::new(vec![text(&payload)]);
        let state = ClipboardState { available: true, entries: history.clone(), collections: Vec::new() };
        let launcher = state.clone();
        assert!(Arc::ptr_eq(&history, &state.entries));
        assert!(Arc::ptr_eq(&state.entries, &launcher.entries));
        let ClipContent::Text { text: actor_text } = &history[0].content else { panic!("text entry") };
        let ClipContent::Text { text: launcher_text } = &launcher.entries[0].content else { panic!("text entry") };
        assert_eq!(actor_text.as_ptr(), launcher_text.as_ptr());
        Arc::make_mut(&mut history)[0].pinned = true;
        assert!(!Arc::ptr_eq(&history, &state.entries));
        assert!(history[0].pinned);
        assert!(!state.entries[0].pinned);
        assert!(!launcher.entries[0].pinned);
    }

    #[test]
    fn shared_history_keeps_the_existing_json_shape() {
        let entries = vec![text("clipboard text")];
        let state = ClipboardState { available: true, entries: Arc::new(entries.clone()), collections: Vec::new() };
        assert_eq!(serde_json::to_value(&state).unwrap(), serde_json::json!({
            "available": true, "entries": entries, "collections": []
        }));
        assert_eq!(serde_json::to_value(&state.entries).unwrap(), serde_json::to_value(&*state.entries).unwrap());
    }

    #[test]
    fn idle_pruning_does_not_copy_a_shared_history() {
        let mut history = Arc::new(vec![text("recent")]);
        let snapshot = history.clone();
        let now = history[0].copied_at;
        let mut policy = ClipboardPolicy::default();
        assert!(!prune_shared(&mut history, &policy, now));
        policy.max_age = Some(Duration::from_secs(60));
        assert!(!prune_shared(&mut history, &policy, now));
        assert!(Arc::ptr_eq(&history, &snapshot));
        assert!(prune_shared(&mut history, &policy, now + 61));
        assert!(history.is_empty());
        assert_eq!(snapshot.len(), 1);
    }

    #[test]
    fn classifies_and_skips_secrets() {
        assert_eq!(classify(&["text/plain;charset=utf-8", "UTF8_STRING"]), Some("text"));
        assert_eq!(classify(&["image/png", "text/html"]), Some("image/png"));
        assert_eq!(classify(&["text/plain", "x-kde-passwordManagerHint"]), None);
        assert_eq!(classify(&["application/x-thing"]), None);
        assert_eq!(classify(&["text/uri-list", "x-special/gnome-copied-files", "text/plain"]), Some("files"));
    }

    #[test]
    fn captures_classify_what_was_copied() {
        let p = ClipboardPolicy::default();
        let e = capture(&["text/plain"], &p, "", |_| Some(b"#cba6f7".to_vec())).unwrap();
        assert!(matches!(e.class, ClipClass::Color { .. }), "{:?}", e.class);
        let e = capture(&["text/plain"], &p, "", |_| Some(b"fn main() {\n    println!(\"hi\");\n}".to_vec())).unwrap();
        assert_eq!(e.class, ClipClass::Code { lang: "rust".into() });
        // A file manager's copy: files, offered back as a URI list.
        let list = b"file:///home/demo/Pictures/harbour.png\r\nfile:///home/demo/notes.md\r\n".to_vec();
        let e = capture(&["text/uri-list", "text/plain"], &p, "", |mime| (mime == "text/uri-list").then(|| list.clone())).unwrap();
        assert_eq!(e.class, ClipClass::Files { count: 2 });
        assert_eq!(e.preview(40), "harbour.png, notes.md");
        assert_eq!(e.file_paths()[1], PathBuf::from("/home/demo/notes.md"));
        // GNOME's form.
        let e = capture(&["x-special/gnome-copied-files"], &p, "", |_| Some(b"copy\nfile:///tmp/a.txt".to_vec())).unwrap();
        assert_eq!(e.content, ClipContent::Files { uris: vec!["file:///tmp/a.txt".into()] });
        // A browser's URI list of web links falls back to its text: a link.
        let e = capture(&["text/uri-list", "text/plain"], &p, "", |_| Some(b"https://mandara.dev/".to_vec())).unwrap();
        assert_eq!(e.class, ClipClass::Link { host: "mandara.dev".into() });
    }

    #[test]
    fn search_matches_text_kind_and_collections() {
        let work = ClipCollection { id: 1, name: "Work notes".into(), created: 0 };
        let mut e = text("SELECT id FROM users;");
        e.collections.push(1);
        e.app_id = "dev.zed.Zed".into();
        let all = [work];
        for q in ["", "select", "USERS", "code", "sql", "work", "zed", "id users"] {
            assert!(e.matches(q, &all), "{q:?}");
        }
        for q in ["python", "image", "id nope", "personal"] {
            assert!(!e.matches(q, &all), "{q:?}");
        }
        assert!(text("oklch(70% 0.15 250)").matches("color", &[]));
        assert!(text("Ünïcode Straße").matches("straße", &[]));
    }

    #[test]
    fn collections_create_rename_collect_and_delete() {
        let (mut cols, mut entries) = (Vec::new(), vec![text("a"), text("b")]);
        let (a, b) = (entries[0].id, entries[1].id);
        let reply = edit_collections(&ClipboardCommand::CreateCollection("  Work  ".into()), &mut cols, &mut entries).unwrap();
        assert_eq!(reply, r#"{"id":1,"name":"Work"}"#);
        edit_collections(&ClipboardCommand::CreateCollection("Palette".into()), &mut cols, &mut entries).unwrap();
        assert!(edit_collections(&ClipboardCommand::CreateCollection("work".into()), &mut cols, &mut entries).is_err(), "names are unique");
        assert!(edit_collections(&ClipboardCommand::CreateCollection("   ".into()), &mut cols, &mut entries).is_err(), "names aren't empty");
        assert!(edit_collections(&ClipboardCommand::RenameCollection { id: 2, name: "Work".into() }, &mut cols, &mut entries).is_err());
        edit_collections(&ClipboardCommand::RenameCollection { id: 2, name: "Colours".into() }, &mut cols, &mut entries).unwrap();
        assert_eq!(cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Work", "Colours"]);
        let long = "x".repeat(COLLECTION_NAME_MAX * 2);
        edit_collections(&ClipboardCommand::RenameCollection { id: 2, name: long }, &mut cols, &mut entries).unwrap();
        assert_eq!(cols[1].name.chars().count(), COLLECTION_NAME_MAX);
        for (id, c) in [(a, 1), (b, 1), (b, 2)] {
            edit_collections(&ClipboardCommand::Collect { id, collection: c, add: true }, &mut cols, &mut entries).unwrap();
        }
        // Adding twice is harmless; unknown collections and entries fail.
        edit_collections(&ClipboardCommand::Collect { id: a, collection: 1, add: true }, &mut cols, &mut entries).unwrap();
        assert_eq!(entries[0].collections, [1]);
        assert!(edit_collections(&ClipboardCommand::Collect { id: a, collection: 9, add: true }, &mut cols, &mut entries).is_err());
        assert!(edit_collections(&ClipboardCommand::Collect { id: 7, collection: 1, add: true }, &mut cols, &mut entries).is_err());
        edit_collections(&ClipboardCommand::Collect { id: b, collection: 1, add: false }, &mut cols, &mut entries).unwrap();
        assert_eq!(entries[1].collections, [2]);
        // Deleting a collection keeps its entries, minus the membership.
        edit_collections(&ClipboardCommand::DeleteCollection(2), &mut cols, &mut entries).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries[1].collections.is_empty());
        assert!(edit_collections(&ClipboardCommand::DeleteCollection(2), &mut cols, &mut entries).is_err());
        // New ids never reuse a live one.
        let reply = edit_collections(&ClipboardCommand::CreateCollection("Snippets".into()), &mut cols, &mut entries).unwrap();
        assert_eq!(reply, r#"{"id":2,"name":"Snippets"}"#);
    }

    #[test]
    fn collected_entries_survive_limits_and_clears() {
        let policy = ClipboardPolicy { max_entries: 1, max_collected: 2, max_age: Some(Duration::from_secs(3600)), ..ClipboardPolicy::default() };
        let mut list = Vec::new();
        for t in ["a", "b", "c"] {
            push(&mut list, text(t), &policy);
            list[0].collections.push(1);
        }
        let mut old = text("old");
        old.copied_at -= 2 * 3600;
        old.collections.push(1);
        list.push(old);
        push(&mut list, text("loose"), &policy);
        // Two collected kept (the oldest collected beyond the cap goes), plus one loose.
        assert_eq!(list.iter().map(|e| e.preview(9)).collect::<Vec<_>>(), ["loose", "c", "b"]);
        // A re-copy keeps the membership.
        push(&mut list, text("c"), &policy);
        assert_eq!(list[0].collections, [1]);
        list.retain(ClipEntry::kept);
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn collections_and_memberships_persist() {
        let dir = std::env::temp_dir().join(format!("mandara-clip-test-{}-{}", std::process::id(), chrono::Local::now().timestamp_nanos_opt().unwrap_or_default()));
        let (mut cols, mut entries) = (Vec::new(), vec![text("#cba6f7"), text("https://mandara.dev"), text("plain")]);
        for name in ["Palette", "Links", "Scratch"] {
            edit_collections(&ClipboardCommand::CreateCollection(name.into()), &mut cols, &mut entries).unwrap();
        }
        let (colour, link) = (entries[0].id, entries[1].id);
        edit_collections(&ClipboardCommand::Collect { id: colour, collection: 1, add: true }, &mut cols, &mut entries).unwrap();
        edit_collections(&ClipboardCommand::Collect { id: link, collection: 2, add: true }, &mut cols, &mut entries).unwrap();
        edit_collections(&ClipboardCommand::Collect { id: link, collection: 3, add: true }, &mut cols, &mut entries).unwrap();
        save(&dir, &entries);
        save_collections(&dir, &cols);
        // A restart: the same collections and memberships, classes intact.
        let (mut loaded, loaded_cols) = (load(&dir), load_collections(&dir));
        assert_eq!(loaded_cols, cols);
        assert_eq!(loaded, entries);
        assert!(matches!(loaded[0].class, ClipClass::Color { .. }));
        // A collection deleted while the history wasn't saved: memberships
        // to it are dropped on load.
        let fewer: Vec<ClipCollection> = cols.iter().filter(|c| c.id != 3).cloned().collect();
        save_collections(&dir, &fewer);
        reconcile(&mut loaded, &load_collections(&dir));
        assert_eq!(loaded[1].collections, [2]);
        // An old history without classes or collections loads and is classified.
        let legacy = r#"[{"id":7,"content":{"kind":"text","text":"margin: 0 auto;"},"copied_at":0,"pinned":true}]"#;
        std::fs::write(dir.join(HISTORY_FILE), legacy).unwrap();
        let old = load(&dir);
        assert_eq!((old[0].class.clone(), old[0].pinned, old[0].collections.is_empty()), (ClipClass::Css, true, true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn push_dedupes_newest_first() {
        let mut list = vec![text("a"), text("b")];
        push(&mut list, text("b"), &ClipboardPolicy::default());
        assert_eq!(list[0].preview(10), "b");
        assert_eq!(list.len(), 2);
        let long = text("lorem   ipsum\n dolor sit amet");
        assert_eq!(long.preview(11), "lorem ipsum…");
    }

    #[test]
    fn limits_keep_pins_and_drop_old_entries() {
        let policy = ClipboardPolicy { max_entries: 2, max_pinned: 1, max_age: Some(Duration::from_secs(3600)), ..ClipboardPolicy::default() };
        let mut list = Vec::new();
        for t in ["a", "b", "c"] {
            push(&mut list, text(t), &policy);
        }
        assert_eq!(list.iter().map(|e| e.preview(5)).collect::<Vec<_>>(), ["c", "b"]);
        list[1].pinned = true;
        let mut old = text("ancient");
        old.copied_at -= 2 * 3600;
        list.push(old);
        push(&mut list, text("d"), &policy);
        // "b" is pinned (kept beyond the two loose ones), "ancient" aged out.
        assert_eq!(list.iter().map(|e| e.preview(9)).collect::<Vec<_>>(), ["d", "c", "b"]);
        // Re-copying a pinned entry keeps its pin.
        push(&mut list, text("b"), &policy);
        assert!(list[0].pinned);
    }

    #[test]
    fn policy_from_config() {
        let c = ClipboardConfig { exclude_apps: vec!["KeePassXC".into(), " ".into()], max_text_kb: 1, auto_clear_hours: 2, ..Default::default() };
        let p = ClipboardPolicy::from(&c);
        assert!(p.excludes("org.keepassxc.KeePassXC") && !p.excludes("firefox") && !p.excludes(""));
        assert_eq!(p.max_text, 1024);
        assert_eq!(p.max_age, Some(Duration::from_secs(7200)));
        // Disabled or excluded: nothing is captured (and nothing is read).
        let read = |_: &str| -> Option<Vec<u8>> { panic!("must not read") };
        assert!(capture(&["text/plain"], &ClipboardPolicy { enabled: false, ..p.clone() }, "", read).is_none());
        assert!(capture(&["text/plain"], &p, "org.keepassxc.KeePassXC", read).is_none());
        assert!(capture(&["image/png"], &ClipboardPolicy { images: false, ..p.clone() }, "", read).is_none());
        let e = capture(&["text/plain"], &p, "zed", |_| Some(b"hi".to_vec())).unwrap();
        assert_eq!((e.app_id.as_str(), e.preview(5).as_str()), ("zed", "hi"));
        assert!(capture(&["text/plain"], &p, "zed", |_| Some(vec![b'x'; 2048])).is_none(), "over the text limit");
    }
}
