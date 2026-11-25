//! Notes on disk: every note is a real Markdown file, every topic a folder.
//!
//! ```text
//! <root>/                      default topic ("Notes"): files at the top
//! <root>/Work/plan.md          topic "Work"
//! <root>/.mandara-notes.toml   index: topic order, colours, icons, note ids
//! ```
//!
//! `<root>` is `$XDG_DATA_HOME/mandara/notes` unless the notes plugin's
//! `storage_folder` setting points elsewhere (a Nextcloud folder, an
//! Obsidian vault…). Files stay the source of truth: the index only adds
//! what a folder can't hold (order, colour, icon, the numeric ids commands
//! use), so external edits, new files and new folders simply show up on the
//! next scan (the plugin watches `<root>` with `notify`), and deleting the
//! index loses nothing but colours. A file renamed outside Mandara gets a
//! new id. Hidden files and folders and non-`.md` files are ignored.
//!
//! A note's file is named after its title when it is first written and
//! renamed to follow the title when editing ends ([`NoteStore::settle`]).
//! Notes kept in `productivity.json` before this existed are written out as
//! files once ([`NoteStore::migrate`]) and only then dropped from the JSON.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod snapshot;
mod scan;
mod watch;
pub use snapshot::{NotesSnapshot, ReadError};
use std::time::SystemTime;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::todo_lists::{ListColor, OnDelete, SWATCH_HUES};

/// The default topic: the root folder itself; renamable, never deleted.
pub const DEFAULT_TOPIC: u64 = 0;
pub const DEFAULT_TOPIC_NAME: &str = "Notes";
pub const INDEX_FILE: &str = ".mandara-notes.toml";
pub const EXTENSION: &str = "md";
/// Longest topic / file name stem kept (characters).
const MAX_NAME_CHARS: usize = 60;
/// Stem of a note with no title yet.
const UNTITLED: &str = "Untitled";

/// A note body's fingerprint for conflict checks: the panel remembers
/// which disk body a draft replaces, and the store which body it last wrote,
/// as this instead of a second copy of the Markdown. It is only compared
/// within one process.
pub(crate) fn body_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// The default notes folder.
pub fn default_root() -> PathBuf {
    mandara_config::paths::data_dir().join("notes")
}

/// A topic (folder) of notes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Topic {
    pub id: u64,
    pub name: String,
    /// Folder under the root (`""` for the default topic).
    pub dir: String,
    /// A palette tone or `hue:<deg>` (as to-do lists).
    pub color: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

impl Default for Topic {
    fn default() -> Self {
        Self { id: DEFAULT_TOPIC, name: DEFAULT_TOPIC_NAME.into(), dir: String::new(), color: crate::todo_lists::DEFAULT_LIST_COLOR.into(), icon: None }
    }
}

impl Topic {
    /// The default topic still under its shipped English name (shown translated).
    pub fn has_default_name(&self) -> bool {
        self.id == DEFAULT_TOPIC && self.name == DEFAULT_TOPIC_NAME
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Index {
    topics: Vec<Topic>,
    /// Note path (relative, `/`-separated) → id.
    ids: BTreeMap<String, u64>,
    next_id: u64,
}

#[derive(Serialize)]
struct IndexRef<'a> {
    topics: &'a [Topic],
    ids: &'a BTreeMap<String, u64>,
    next_id: u64,
}

/// One note.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Note {
    pub id: u64,
    pub topic: u64,
    /// Relative path of its file (`None` until it has text).
    pub path: Option<String>,
    pub text: String,
    /// Unix seconds (the file's modification time).
    pub updated: i64,
}

impl Note {
    /// First line without Markdown markers.
    pub fn title(&self) -> String {
        crate::markdown::title_line(&self.text).unwrap_or_else(|| mandara_i18n::t("Untitled note").to_string())
    }

    /// The text after the title, flattened (no Markdown markers).
    pub fn preview(&self) -> String {
        let body: String = self.text.trim_start().lines().skip(1).collect::<Vec<_>>().join("\n");
        let mut s = String::new();
        crate::markdown::plain(&crate::markdown::parse(&body).blocks, &mut s);
        s
    }
}

/// A note kept in `productivity.json` before notes became files.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LegacyNote {
    pub id: u64,
    pub text: String,
    pub updated: i64,
}

/// `name` made safe as a single path component (keeps Arabic and other
/// scripts; drops separators, control characters and a leading dot).
pub fn safe_name(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c == '/' || c == '\\' || c.is_control() || c == ':' { ' ' } else { c }).collect();
    let flat = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = flat.trim_start_matches('.').trim();
    trimmed.chars().take(MAX_NAME_CHARS).collect::<String>().trim().to_string()
}

/// Seconds since the epoch (0 for times before it).
fn unix(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// A root-relative path with `/` separators, as the index stores it.
fn rel(path: &Path) -> String {
    path.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// Every note and topic under one root folder.
pub struct NoteStore {
    view: NotesSnapshot,
    ids: BTreeMap<String, u64>,
    next_id: u64,
    index_error: Option<String>,
    scan_error: Option<String>,
    index_dirty: bool,
    /// Drafts the last edit's pending writes kept back because their file
    /// changed elsewhere; the worker hands them to its completion, so a
    /// conflict never fails the different edit queued behind it.
    pub(crate) deferred_conflicts: Vec<u64>,
    watcher: Option<notify::RecommendedWatcher>,
    /// [`body_hash`] of the body this store last wrote, per note, so a
    /// queued draft can tell its own earlier write from an external edit.
    /// Entries leave with their note; a folder change replaces the store.
    written: BTreeMap<u64, u64>,
}

impl std::ops::Deref for NoteStore {
    type Target = NotesSnapshot;
    fn deref(&self) -> &Self::Target { &self.view }
}

impl std::ops::DerefMut for NoteStore {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.view }
}

impl NoteStore {
    /// A stale retained body must never replace bytes that the scan could not read.
    pub(crate) fn ensure_writable(&self, id: u64) -> Result<()> {
        if let Some(error) = self.note(id).and_then(|note| note.path.as_deref()).and_then(|path| self.read_error(path)) {
            anyhow::bail!("{}", error.message);
        }
        Ok(())
    }

    /// Folder mutations cannot bypass a failed child file's protection.
    fn ensure_topic_writable(&self, id: u64) -> Result<()> {
        let topic = self.topic(id).context("no such topic")?;
        if let Some(error) = self.read_errors.as_ref().and_then(|errors| errors.iter().find(|error| Path::new(&error.path).parent() == Some(Path::new(&topic.dir)))) {
            anyhow::bail!("{}", error.message);
        }
        Ok(())
    }
    /// Opens (creating) `root` and scans it.
    pub fn open(root: &Path) -> Self {
        let index: Index = std::fs::read_to_string(root.join(INDEX_FILE)).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default();
        let mut s = Self { view: NotesSnapshot { root: root.to_path_buf(), topics: index.topics, notes: Vec::new(), read_errors: None }, ids: index.ids, next_id: index.next_id.max(1), index_error: None, scan_error: None, index_dirty: false, deferred_conflicts: Vec::new(), watcher: None, written: BTreeMap::new() };
        if let Err(err) = std::fs::create_dir_all(root) {
            tracing::warn!("notes folder {}: {err:#}", root.display());
        }
        s.rescan(&[]);
        s
    }

    /// Marks the index changed and writes it now; a failure stays recorded
    /// (`index_dirty`, `index_error`) for the next [`NoteStore::persist_index`].
    fn save_index(&mut self) {
        self.index_dirty = true;
        let _ = self.persist_index();
    }

    /// Retries metadata after a content or rename operation partially succeeded.
    pub fn persist_index(&mut self) -> Result<()> {
        let result = self.scan_result().and_then(|()| self.write_index());
        self.index_error = result.as_ref().err().map(|error| format!("{error:#}"));
        if result.is_ok() { self.index_dirty = false; }
        result
    }

    /// Writes `.mandara-notes.toml` atomically, skipping the write when the
    /// file already holds the same bytes.
    fn write_index(&self) -> Result<()> {
        let index = IndexRef { topics: &self.view.topics, ids: &self.ids, next_id: self.next_id };
        let text = toml::to_string(&index)?;
        let path = self.view.root.join(INDEX_FILE);
        if std::fs::read(&path).is_ok_and(|current| current == text.as_bytes()) {
            return Ok(());
        }
        mandara_config::write_atomic(&path, text.as_bytes())?;
        Ok(())
    }

    /// Shares note entries without copying their bodies or touching the filesystem.
    pub fn snapshot(&self) -> NotesSnapshot { self.view.clone() }

    /// Refreshes only the note being committed before comparing an editor's draft.
    /// This detects intervening file edits; it does not lock external editors out.
    pub fn refresh_note(&mut self, id: u64) -> Result<bool> {
        let pos = self.view.notes.iter().position(|note| note.id == id).context("no such note")?;
        let note = &self.view.notes[pos];
        let Some(path) = &note.path else { return Ok(true) };
        match std::fs::read_to_string(self.view.root.join(path)) {
            Ok(text) => {
                if note.text != text {
                    self.view.notes[pos] = Arc::new(Note { id, topic: note.topic, path: note.path.clone(), text, updated: unix(SystemTime::now()) });
                }
                Ok(true)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.ids.remove(path);
                self.view.notes.remove(pos);
                self.written.remove(&id);
                Ok(false)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// An operation may have written Markdown before its index failed.
    pub fn index_result(&self) -> Result<()> {
        self.scan_result()?;
        match &self.index_error {
            Some(error) => anyhow::bail!("{error}"),
            None => Ok(()),
        }
    }

    /// A failed scan leaves the previous snapshot and index unchanged.
    pub fn scan_result(&self) -> Result<()> {
        match &self.scan_error { Some(error) => anyhow::bail!("{error}"), None => Ok(()) }
    }

    /// Changing folders may leave an unreadable, unchanged folder, never unsaved metadata.
    pub fn prepare_leave(&mut self) -> Result<()> {
        if self.index_dirty { self.persist_index()?; }
        Ok(())
    }

    /// A note id above every id in use or in the index.
    fn fresh_id(&mut self) -> u64 {
        let id = self.next_id.max(self.ids.values().max().map_or(1, |m| m + 1)).max(self.view.notes.iter().map(|n| n.id + 1).max().unwrap_or(1));
        self.next_id = id + 1;
        id
    }

    /// Re-reads the folder: topics for every folder, notes for every `.md`
    /// file. Notes in `keep` (unsaved edits) keep their in-memory text.
    pub fn rescan(&mut self, keep: &[u64]) {
        let kept: Vec<Arc<Note>> = self.view.notes.iter().filter(|n| keep.contains(&n.id) || n.path.is_none()).cloned().collect();
        if let Err(error) = self.scan() {
            self.scan_error = Some(format!("{error:#}"));
            return;
        }
        for k in kept {
            match self.view.notes.iter_mut().find(|n| n.id == k.id) {
                Some(n) => *n = k,
                None if k.path.is_none() => self.view.notes.insert(0, k),
                None => {}
            }
        }
        let notes = &self.view.notes;
        self.written.retain(|id, _| notes.iter().any(|note| note.id == *id));
    }

    /// A file path for `stem` in `topic`'s folder that no file uses.
    fn free_path(&self, topic: u64, stem: &str, except: Option<&str>) -> String {
        let dir = self.topic(topic).map(|t| t.dir.clone()).unwrap_or_default();
        let stem = { let s = safe_name(stem); if s.is_empty() { UNTITLED.to_string() } else { s } };
        let join = |name: &str| if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") };
        let taken = |p: &str| Some(p) != except && (self.view.root.join(p).exists() || self.view.notes.iter().any(|n| n.path.as_deref() == Some(p)));
        let first = join(&format!("{stem}.{EXTENSION}"));
        if !taken(&first) {
            return first;
        }
        (2..).map(|n| join(&format!("{stem} {n}.{EXTENSION}"))).find(|p| !taken(p)).unwrap_or(first)
    }

    /// Adds a note to `topic` (the default topic if it doesn't exist) and
    /// writes its file unless `text` is blank. A failed file write leaves no
    /// note behind, nor does a blank note whose index can't be saved; a
    /// written note keeps its id when only the index failed
    /// ([`NoteStore::index_result`] reports it).
    pub fn create(&mut self, topic: u64, text: &str) -> Result<u64> {
        let topic = if self.topic(topic).is_some() { topic } else { DEFAULT_TOPIC };
        let previous_next = self.next_id;
        let previous_dirty = self.index_dirty;
        let id = self.fresh_id();
        self.view.notes.insert(0, Arc::new(Note { id, topic, path: None, text: String::new(), updated: unix(SystemTime::now()) }));
        if !text.trim().is_empty()
            && let Err(error) = self.write(id, text)
        {
            self.view.notes.retain(|note| note.id != id);
            return Err(error);
        }
        self.save_index();
        if text.trim().is_empty()
            && let Err(error) = self.index_result()
        {
            self.view.notes.retain(|note| note.id != id);
            self.next_id = previous_next;
            self.index_dirty = previous_dirty;
            return Err(error);
        }
        Ok(id)
    }

    /// Re-creates note `id` from a kept draft after its file was removed
    /// elsewhere, under the same id so later saves of that draft land in it.
    pub fn restore(&mut self, id: u64, topic: u64, text: &str) -> Result<()> {
        anyhow::ensure!(self.note(id).is_none(), "note {id} still exists");
        let topic = if self.topic(topic).is_some() { topic } else { DEFAULT_TOPIC };
        self.view.notes.insert(0, Arc::new(Note { id, topic, path: None, text: String::new(), updated: unix(SystemTime::now()) }));
        self.next_id = self.next_id.max(id + 1);
        if let Err(error) = self.write(id, text) {
            self.view.notes.retain(|note| note.id != id);
            return Err(error);
        }
        self.save_index();
        Ok(())
    }

    /// Writes a note's text to its file (named after its title the first
    /// time).
    pub fn write(&mut self, id: u64, text: &str) -> Result<()> {
        self.ensure_writable(id)?;
        let pos = self.view.notes.iter().position(|n| n.id == id).context("no such note")?;
        let path = match self.view.notes[pos].path.clone() {
            Some(p) => p,
            None => {
                let stem = crate::markdown::title_line(text).unwrap_or_else(|| UNTITLED.into());
                self.free_path(self.view.notes[pos].topic, &stem, None)
            }
        };
        let full = self.view.root.join(&path);
        if let Some(dir) = full.parent() {
            std::fs::create_dir_all(dir)?;
        }
        mandara_config::write_atomic(&full, text.as_bytes())?;
        if self.view.notes[pos].path.is_none() {
            self.ids.insert(path.clone(), id);
        }
        let topic = self.view.notes[pos].topic;
        // Replace the changed entry directly: make_mut would copy its old body.
        self.view.notes[pos] = Arc::new(Note { id, topic, path: Some(path), text: text.to_owned(), updated: unix(SystemTime::now()) });
        self.written.insert(id, body_hash(text));
        self.save_index();
        Ok(())
    }

    /// Whether `text` is the body this store last wrote for note `id` (so
    /// finding it on disk means no other program changed the file since).
    pub(crate) fn wrote(&self, id: u64, text: &str) -> bool {
        self.written.get(&id).is_some_and(|&hash| hash == body_hash(text))
    }

    /// Editing ended: an emptied note is deleted, and the file is renamed
    /// to follow the title. Returns whether the note still exists.
    pub fn settle(&mut self, id: u64) -> Result<bool> {
        self.ensure_writable(id)?;
        let Some(n) = self.view.notes.iter().find(|note| note.id == id).cloned() else { return Ok(false) };
        if n.text.trim().is_empty() {
            self.delete(id)?;
            return Ok(false);
        }
        let Some(old) = n.path.clone() else { return Ok(true) };
        let Some(title) = crate::markdown::title_line(&n.text) else { return Ok(true) };
        let stem = Path::new(&old).file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
        let wanted = safe_name(&title);
        if wanted.is_empty() || stem == wanted || stem.strip_prefix(&wanted).is_some_and(|r| r.trim().parse::<u32>().is_ok()) {
            return Ok(true);
        }
        let new = self.free_path(n.topic, &wanted, Some(&old));
        self.rename_file(id, &old, &new)?;
        Ok(true)
    }

    /// Renames note `id`'s file from `old` to `new` (root-relative) and
    /// moves its id in the index with it.
    fn rename_file(&mut self, id: u64, old: &str, new: &str) -> Result<()> {
        self.ensure_writable(id)?;
        let to = self.view.root.join(new);
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::rename(self.view.root.join(old), &to)?;
        self.ids.remove(old);
        self.ids.insert(new.to_string(), id);
        if let Some(n) = self.view.notes.iter_mut().find(|n| n.id == id) {
            Arc::make_mut(n).path = Some(new.to_string());
        }
        self.save_index();
        Ok(())
    }

    /// Deletes a note and its file (already gone counts as deleted). Returns
    /// whether the note existed.
    pub fn delete(&mut self, id: u64) -> Result<bool> {
        self.ensure_writable(id)?;
        let Some(pos) = self.view.notes.iter().position(|n| n.id == id) else { return Ok(false) };
        let n = &self.view.notes[pos];
        if let Some(p) = &n.path {
            match std::fs::remove_file(self.view.root.join(p)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            self.ids.remove(p);
        }
        self.view.notes.remove(pos);
        self.written.remove(&id);
        self.save_index();
        Ok(true)
    }

    /// Moves a note (its file) to another topic.
    pub fn move_note(&mut self, id: u64, topic: u64) -> Result<()> {
        self.ensure_writable(id)?;
        self.topic(topic).context("no such topic")?;
        let n = self.view.notes.iter().find(|note| note.id == id).cloned().context("no such note")?;
        if n.topic == topic {
            return Ok(());
        }
        if let Some(old) = &n.path {
            let name = Path::new(old).file_stem().and_then(|s| s.to_str()).unwrap_or(UNTITLED).to_string();
            let new = self.free_path(topic, &name, None);
            self.rename_file(id, old, &new)?;
        }
        if let Some(n) = self.view.notes.iter_mut().find(|n| n.id == id) {
            Arc::make_mut(n).topic = topic;
        }
        Ok(())
    }

    /// Adds a topic and its folder, or returns the existing topic with that
    /// folder name. Without a colour, the next swatch hue is used.
    pub fn add_topic(&mut self, name: &str, color: Option<&str>, icon: Option<&str>) -> Result<u64> {
        let dir = safe_name(name);
        anyhow::ensure!(!dir.is_empty(), "a topic needs a name");
        anyhow::ensure!(color.is_none_or(ListColor::valid), "colour must be a tone or hue:<0-359>");
        if let Some(t) = self.view.topics.iter().find(|t| t.dir == dir) {
            return Ok(t.id);
        }
        std::fs::create_dir_all(self.view.root.join(&dir))?;
        let id = self.view.topics.iter().map(|t| t.id).max().unwrap_or(DEFAULT_TOPIC) + 1;
        let color = color.map_or_else(|| format!("hue:{}", SWATCH_HUES[self.view.topics.len() % SWATCH_HUES.len()]), str::to_string);
        self.view.topics.push(Topic { id, name: name.split_whitespace().collect::<Vec<_>>().join(" "), dir, color, icon: icon.filter(|i| !i.trim().is_empty()).map(str::to_string) });
        self.save_index();
        Ok(id)
    }

    /// The topic for a `#topic` quick add, made when missing.
    pub fn topic_for(&mut self, key: &str) -> u64 {
        match self.find_topic(key) {
            Some(id) => id,
            None => self.add_topic(key, None, None).unwrap_or(DEFAULT_TOPIC),
        }
    }

    /// Renames a topic (and its folder; the default topic keeps the root).
    pub fn rename_topic(&mut self, id: u64, name: &str) -> Result<()> {
        if id != DEFAULT_TOPIC { self.ensure_topic_writable(id)?; }
        let clean = name.split_whitespace().collect::<Vec<_>>().join(" ");
        anyhow::ensure!(!safe_name(&clean).is_empty(), "a topic needs a name");
        let t = self.topic(id).cloned().context("no such topic")?;
        if id != DEFAULT_TOPIC {
            let dir = safe_name(&clean);
            if dir != t.dir {
                anyhow::ensure!(!self.view.root.join(&dir).exists(), "a folder called {dir} already exists");
                std::fs::rename(self.view.root.join(&t.dir), self.view.root.join(&dir))?;
                let prefix = format!("{}/", t.dir);
                let moved: Vec<(String, u64)> = self.ids.iter().filter(|(p, _)| p.starts_with(&prefix)).map(|(p, i)| (p.clone(), *i)).collect();
                for (p, i) in moved {
                    let np = format!("{dir}/{}", &p[prefix.len()..]);
                    self.ids.remove(&p);
                    self.ids.insert(np.clone(), i);
                    if let Some(n) = self.view.notes.iter_mut().find(|n| n.id == i) {
                        Arc::make_mut(n).path = Some(np);
                    }
                }
                if let Some(t) = self.view.topics.iter_mut().find(|t| t.id == id) {
                    t.dir = dir;
                }
            }
        }
        if let Some(t) = self.view.topics.iter_mut().find(|t| t.id == id) {
            t.name = clean;
        }
        self.save_index();
        Ok(())
    }

    /// Sets a topic's colour (a tone or `hue:<0-359>`).
    pub fn recolor_topic(&mut self, id: u64, color: &str) -> Result<()> {
        anyhow::ensure!(ListColor::valid(color), "colour must be a tone or hue:<0-359>");
        self.view.topics.iter_mut().find(|t| t.id == id).context("no such topic")?.color = color.to_string();
        self.save_index();
        Ok(())
    }

    /// Moves a topic to position `to` (clamped) in the topic order; returns
    /// where it landed.
    pub fn move_topic(&mut self, id: u64, to: usize) -> Result<usize> {
        let from = self.view.topics.iter().position(|t| t.id == id).context("no such topic")?;
        let t = self.view.topics.remove(from);
        let to = to.min(self.view.topics.len());
        self.view.topics.insert(to, t);
        self.save_index();
        Ok(to)
    }

    /// Deletes a topic; its notes move to the default topic or are deleted.
    /// The folder goes only once empty (other files in it are kept).
    pub fn remove_topic(&mut self, id: u64, notes: OnDelete) -> Result<usize> {
        self.ensure_topic_writable(id)?;
        anyhow::ensure!(id != DEFAULT_TOPIC, "the default topic can be renamed but not deleted");
        let t = self.topic(id).cloned().context("no such topic")?;
        let ids: Vec<u64> = self.view.notes.iter().filter(|n| n.topic == id).map(|n| n.id).collect();
        for n in &ids {
            match notes {
                OnDelete::MoveToInbox => self.move_note(*n, DEFAULT_TOPIC)?,
                OnDelete::DeleteTasks => {
                    self.delete(*n)?;
                }
            }
        }
        let _ = std::fs::remove_dir(self.view.root.join(&t.dir));
        self.view.topics.retain(|t| t.id != id);
        self.save_index();
        Ok(ids.len())
    }

    /// Writes `notes` from `productivity.json` into the default topic.
    /// Returns how many were written; on error nothing is lost (the caller
    /// keeps the JSON copy).
    pub fn migrate(&mut self, notes: &[LegacyNote]) -> Result<usize> {
        let mut written = 0;
        for old in notes.iter().filter(|n| !n.text.trim().is_empty()) {
            let id = self.create(DEFAULT_TOPIC, &old.text)?;
            if let Some(p) = self.note(id).and_then(|n| n.path.clone()) {
                let when = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(old.updated.max(0) as u64);
                if let Ok(f) = std::fs::File::options().write(true).open(self.view.root.join(&p)) {
                    let _ = f.set_modified(when);
                }
                if let Some(n) = self.view.notes.iter_mut().find(|n| n.id == id) {
                    Arc::make_mut(n).updated = old.updated;
                }
            }
            written += 1;
        }
        self.view.notes.sort_by_key(|n| std::cmp::Reverse((n.updated, n.id)));
        Ok(written)
    }

    /// Copies a topic's notes (or all, `None`) into `dir` as `.md` files.
    pub fn export(&self, topic: Option<u64>, dir: &Path) -> Result<usize> {
        std::fs::create_dir_all(dir)?;
        let mut n = 0;
        let mut used: Vec<String> = Vec::new();
        for note in self.view.notes.iter().filter(|x| topic.is_none_or(|t| x.topic == t) && !x.text.trim().is_empty()) {
            let stem = safe_name(&note.title());
            let mut name = format!("{stem}.{EXTENSION}");
            let mut k = 2;
            while used.contains(&name) || dir.join(&name).exists() {
                name = format!("{stem} {k}.{EXTENSION}");
                k += 1;
            }
            mandara_config::write_atomic(&dir.join(&name), note.text.as_bytes())?;
            used.push(name);
            n += 1;
        }
        Ok(n)
    }


}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, NoteStore) {
        let dir = tempfile::tempdir().unwrap();
        let s = NoteStore::open(&dir.path().join("notes"));
        (dir, s)
    }

    #[test]
    fn borrowed_index_keeps_the_existing_toml_shape() {
        let (_dir, mut store) = store();
        store.create(DEFAULT_TOPIC, "One note").unwrap();
        let owned = Index { topics: store.topics.clone(), ids: store.ids.clone(), next_id: store.next_id };
        let borrowed = IndexRef { topics: &store.topics, ids: &store.ids, next_id: store.next_id };
        assert_eq!(toml::to_string(&borrowed).unwrap(), toml::to_string(&owned).unwrap());
    }

    #[test]
    fn a_failed_empty_draft_index_does_not_accumulate_invisible_notes() {
        let (_dir, mut store) = store();
        std::fs::remove_file(store.root.join(INDEX_FILE)).unwrap();
        std::fs::create_dir(store.root.join(INDEX_FILE)).unwrap();
        let next = store.next_id;
        for _ in 0..4 { assert!(store.create(DEFAULT_TOPIC, "").is_err()); }
        assert!(store.notes.is_empty());
        assert_eq!(store.next_id, next);
        assert!(store.prepare_leave().is_ok());
        std::fs::remove_dir(store.root.join(INDEX_FILE)).unwrap();
        let id = store.create(DEFAULT_TOPIC, "").unwrap();
        assert!(store.note(id).is_some());
    }

    #[test]
    fn failed_note_writes_leave_no_phantom_note_or_path() {
        let (_dir, mut store) = store();
        let kept = store.create(DEFAULT_TOPIC, "Keep this note").unwrap();
        let empty = store.create(DEFAULT_TOPIC, "").unwrap();
        store.topics.iter_mut().find(|topic| topic.id == DEFAULT_TOPIC).unwrap().dir = "blocked".into();
        std::fs::write(store.root.join("blocked"), b"not a directory").unwrap();
        let ids = store.ids.clone();
        assert!(store.write(empty, "Cannot save yet").is_err());
        assert!(store.note(empty).unwrap().path.is_none());
        assert_eq!(store.note(empty).unwrap().text, "");
        assert!(store.create(DEFAULT_TOPIC, "Cannot create yet").is_err());
        assert_eq!(store.notes.len(), 2);
        assert_eq!(store.note(kept).unwrap().text, "Keep this note");
        assert_eq!(store.ids, ids);
        std::fs::remove_file(store.root.join("blocked")).unwrap();
        store.write(empty, "Saved on retry").unwrap();
        assert_eq!(store.note(empty).unwrap().text, "Saved on retry");
    }

    #[test]
    fn failed_note_deletion_preserves_the_note_and_index_for_retry() {
        let (_dir, mut store) = store();
        let id = store.create(DEFAULT_TOPIC, "Keep until deleted").unwrap();
        let path = store.root.join(store.note(id).unwrap().path.as_ref().unwrap());
        let ids = store.ids.clone();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(store.delete(id).is_err());
        assert_eq!(store.note(id).unwrap().text, "Keep until deleted");
        assert_eq!(store.ids, ids);
        std::fs::remove_dir(&path).unwrap();
        assert!(store.delete(id).unwrap());
        assert!(store.note(id).is_none());
        assert!(!store.ids.values().any(|saved| *saved == id));
    }

    /// Issue #6: "+" creates an empty note and opens it; pruning abandoned
    /// drafts must not delete it before you've typed a word.
    #[test]
    fn a_new_empty_note_survives_while_open_and_goes_once_abandoned() {
        let (_dir, mut s) = store();
        let old = s.create(DEFAULT_TOPIC, "").unwrap();
        let kept = s.create(DEFAULT_TOPIC, "Groceries").unwrap();
        let new = s.create(DEFAULT_TOPIC, "").unwrap();
        // Opening the new one prunes the old draft only.
        assert_eq!(s.empty_drafts(Some(new)), vec![old]);
        // Closing it with nothing typed prunes it too; written notes stay.
        let mut all = s.empty_drafts(None);
        all.sort_unstable();
        let mut want = vec![old, new];
        want.sort_unstable();
        assert_eq!(all, want);
        assert!(!all.contains(&kept));
    }

    #[test]
    fn old_json_notes_become_files_in_the_default_topic() {
        // `productivity.json` as written before topics existed.
        let old = r#"{ "todos": [], "notes": [
            { "id": 4, "text": "Groceries\n- [ ] saffron\n- [x] dates", "updated": 1700000400 },
            { "id": 2, "text": "Trip to Siwa\nbook the bus", "updated": 1700000200 },
            { "id": 1, "text": "   ", "updated": 1700000100 }
        ] }"#;
        let saved: crate::productivity::Saved = serde_json::from_str(old).unwrap();
        let (_d, mut s) = store();
        assert_eq!(s.migrate(&saved.notes).unwrap(), 2, "empty notes are not kept");
        let reopened = NoteStore::open(&s.root);
        assert_eq!(reopened.notes.len(), 2);
        assert!(reopened.notes.iter().all(|n| n.topic == DEFAULT_TOPIC));
        assert_eq!(reopened.notes[0].text, "Groceries\n- [ ] saffron\n- [x] dates", "newest first, text intact");
        assert_eq!(reopened.notes[0].updated, 1700000400, "edit time kept");
        assert_eq!(reopened.notes[1].path.as_deref(), Some("Trip to Siwa.md"));
        assert!(s.root.join("Groceries.md").is_file());
    }

    #[test]
    fn topics_are_folders() {
        let (_d, mut s) = store();
        let work = s.add_topic("Work", Some("tertiary"), None).unwrap();
        assert!(s.root.join("Work").is_dir());
        let id = s.create(work, "# Plan\n\nship").unwrap();
        assert_eq!(s.note(id).unwrap().path.as_deref(), Some("Work/Plan.md"));
        s.rename_topic(work, "Day job").unwrap();
        assert!(s.root.join("Day job/Plan.md").is_file());
        assert_eq!(s.note(id).unwrap().path.as_deref(), Some("Day job/Plan.md"));
        s.recolor_topic(work, "hue:200").unwrap();
        assert!(s.recolor_topic(work, "teal").is_err());
        assert_eq!(s.find_topic("dayjob"), Some(work));
        assert_eq!(s.move_topic(work, 0).unwrap(), 0);
        // Order, colours and ids survive a reopen.
        let again = NoteStore::open(&s.root);
        assert_eq!(again.topics[0].id, work);
        assert_eq!(again.topics[0].color, "hue:200");
        assert_eq!(again.note(id).unwrap().text, "# Plan\n\nship");
        // An external folder and file show up.
        std::fs::create_dir_all(s.root.join("Ideas")).unwrap();
        std::fs::write(s.root.join("Ideas/x.md"), "x").unwrap();
        std::fs::write(s.root.join("Ideas/skip.txt"), "no").unwrap();
        s.rescan(&[]);
        let ideas = s.find_topic("Ideas").unwrap();
        assert_eq!(s.count(Some(ideas)), 1);
        assert!(s.remove_topic(DEFAULT_TOPIC, OnDelete::MoveToInbox).is_err());
    }

    #[test]
    fn deleting_a_topic_moves_or_deletes_its_notes() {
        let (_d, mut s) = store();
        let a = s.add_topic("A", None, None).unwrap();
        s.create(a, "one").unwrap();
        s.create(a, "two").unwrap();
        assert_eq!(s.remove_topic(a, OnDelete::MoveToInbox).unwrap(), 2);
        assert_eq!(s.count(Some(DEFAULT_TOPIC)), 2);
        assert!(!s.root.join("A").exists());
        let b = s.add_topic("B", None, None).unwrap();
        let n = s.create(b, "three").unwrap();
        std::fs::write(s.root.join("B/keep.txt"), "not a note").unwrap();
        assert_eq!(s.remove_topic(b, OnDelete::DeleteTasks).unwrap(), 1);
        assert!(s.note(n).is_none());
        assert!(s.root.join("B/keep.txt").is_file(), "other files are never touched");
    }

    #[test]
    fn files_follow_titles_and_moves() {
        let (_d, mut s) = store();
        let id = s.create(DEFAULT_TOPIC, "").unwrap();
        assert_eq!(s.note(id).unwrap().path, None, "nothing written while empty");
        s.write(id, "Draft").unwrap();
        s.write(id, "Final title\nbody").unwrap();
        assert!(s.settle(id).unwrap());
        assert_eq!(s.note(id).unwrap().path.as_deref(), Some("Final title.md"));
        assert!(!s.root.join("Draft.md").exists());
        let other = s.create(DEFAULT_TOPIC, "Final title").unwrap();
        assert_eq!(s.note(other).unwrap().path.as_deref(), Some("Final title 2.md"));
        assert!(s.settle(other).unwrap());
        assert_eq!(s.note(other).unwrap().path.as_deref(), Some("Final title 2.md"), "numbered twins stay put");
        let t = s.add_topic("T", None, None).unwrap();
        s.move_note(id, t).unwrap();
        assert!(s.root.join("T/Final title.md").is_file());
        s.write(id, "  ").unwrap();
        assert!(!s.settle(id).unwrap(), "an emptied note is deleted");
        assert!(!s.root.join("T/Final title.md").exists());
        assert_eq!(safe_name("../a/b:c\n"), "a b c");
        assert_eq!(safe_name("ملاحظات"), "ملاحظات");
    }

    #[test]
    fn search_and_export() {
        let (d, mut s) = store();
        let w = s.add_topic("Work", None, None).unwrap();
        s.create(w, "Quarterly report\nnumbers").unwrap();
        s.create(DEFAULT_TOPIC, "Recipes\nkoshary").unwrap();
        let hits = s.search("koshry");
        assert_eq!(hits.len(), 1, "fuzzy");
        assert_eq!(s.note(hits[0]).unwrap().title(), "Recipes");
        assert!(s.search("zzzz").is_empty());
        let out = d.path().join("export");
        assert_eq!(s.export(Some(w), &out).unwrap(), 1);
        assert!(out.join("Quarterly report.md").is_file());
        assert_eq!(s.export(None, &out).unwrap(), 2);
    }
}
