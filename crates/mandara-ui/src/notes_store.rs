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

fn unix(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn rel(path: &Path) -> String {
    path.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// Every note and topic under one root folder.
pub struct NoteStore {
    pub root: PathBuf,
    pub topics: Vec<Topic>,
    /// Most recently edited first.
    pub notes: Vec<Note>,
    ids: BTreeMap<String, u64>,
    next_id: u64,
}

impl NoteStore {
    /// Opens (creating) `root` and scans it.
    pub fn open(root: &Path) -> Self {
        let index: Index = std::fs::read_to_string(root.join(INDEX_FILE)).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default();
        let mut s = Self { root: root.to_path_buf(), topics: index.topics, notes: Vec::new(), ids: index.ids, next_id: index.next_id.max(1) };
        if let Err(err) = std::fs::create_dir_all(root) {
            tracing::warn!("notes folder {}: {err:#}", root.display());
        }
        s.scan();
        s
    }

    fn save_index(&self) {
        let index = Index { topics: self.topics.clone(), ids: self.ids.clone(), next_id: self.next_id };
        let Ok(text) = toml::to_string(&index) else { return };
        if let Err(err) = mandara_config::write_atomic(&self.root.join(INDEX_FILE), text.as_bytes()) {
            tracing::warn!("saving the notes index: {err:#}");
        }
    }

    fn fresh_id(&mut self) -> u64 {
        let id = self.next_id.max(self.ids.values().max().map_or(1, |m| m + 1)).max(self.notes.iter().map(|n| n.id + 1).max().unwrap_or(1));
        self.next_id = id + 1;
        id
    }

    /// Re-reads the folder: topics for every folder, notes for every `.md`
    /// file. Notes in `keep` (unsaved edits) keep their in-memory text.
    pub fn rescan(&mut self, keep: &[u64]) {
        let kept: Vec<Note> = self.notes.iter().filter(|n| keep.contains(&n.id) || n.path.is_none()).cloned().collect();
        self.scan();
        for k in kept {
            match self.notes.iter_mut().find(|n| n.id == k.id) {
                Some(n) => n.text = k.text,
                None if k.path.is_none() => self.notes.insert(0, k),
                None => {}
            }
        }
    }

    fn scan(&mut self) {
        if !self.topics.iter().any(|t| t.id == DEFAULT_TOPIC) {
            self.topics.insert(0, Topic::default());
        }
        if let Some(t) = self.topics.iter_mut().find(|t| t.id == DEFAULT_TOPIC) {
            t.dir.clear();
        }
        // Folders: topics (new ones join with the next swatch).
        let mut dirs: Vec<String> = std::fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| !n.starts_with('.'))
            .collect();
        dirs.sort();
        self.topics.retain(|t| t.id == DEFAULT_TOPIC || dirs.contains(&t.dir));
        for d in dirs {
            if !self.topics.iter().any(|t| t.dir == d) {
                let id = self.topics.iter().map(|t| t.id).max().unwrap_or(DEFAULT_TOPIC) + 1;
                let color = format!("hue:{}", SWATCH_HUES[self.topics.len() % SWATCH_HUES.len()]);
                self.topics.push(Topic { id, name: d.clone(), dir: d, color, icon: None });
            }
        }
        for t in &mut self.topics {
            if !ListColor::valid(&t.color) {
                t.color = crate::todo_lists::DEFAULT_LIST_COLOR.into();
            }
        }
        // Notes.
        let mut notes = Vec::new();
        for t in &self.topics {
            let dir = self.root.join(&t.dir);
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') || path.extension().and_then(|x| x.to_str()) != Some(EXTENSION) || !e.file_type().is_ok_and(|t| t.is_file()) {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                let updated = e.metadata().and_then(|m| m.modified()).map(unix).unwrap_or(0);
                let rel_path = rel(path.strip_prefix(&self.root).unwrap_or(&path));
                notes.push((rel_path, t.id, text, updated));
            }
        }
        let mut out = Vec::with_capacity(notes.len());
        for (path, topic, text, updated) in notes {
            let id = match self.ids.get(&path) {
                Some(id) => *id,
                None => {
                    let id = self.fresh_id();
                    self.ids.insert(path.clone(), id);
                    id
                }
            };
            out.push(Note { id, topic, path: Some(path), text, updated });
        }
        self.ids.retain(|p, _| out.iter().any(|n| n.path.as_deref() == Some(p.as_str())));
        out.sort_by_key(|n| std::cmp::Reverse((n.updated, n.id)));
        self.notes = out;
        self.save_index();
    }

    pub fn topic(&self, id: u64) -> Option<&Topic> {
        self.topics.iter().find(|t| t.id == id)
    }

    pub fn note(&self, id: u64) -> Option<&Note> {
        self.notes.iter().find(|n| n.id == id)
    }

    /// The topic `key` names: id or name (case-insensitive, spaces and
    /// dashes ignored as for `#topic`).
    pub fn find_topic(&self, key: &str) -> Option<u64> {
        let key = key.trim().trim_start_matches('#');
        if let Ok(id) = key.parse::<u64>()
            && self.topic(id).is_some()
        {
            return Some(id);
        }
        let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '_').flat_map(char::to_lowercase).collect::<String>();
        let k = squash(key);
        (!k.is_empty()).then(|| self.topics.iter().find(|t| squash(&t.name) == k).map(|t| t.id)).flatten()
    }

    /// A file path for `stem` in `topic`'s folder that no file uses.
    fn free_path(&self, topic: u64, stem: &str, except: Option<&str>) -> String {
        let dir = self.topic(topic).map(|t| t.dir.clone()).unwrap_or_default();
        let stem = { let s = safe_name(stem); if s.is_empty() { UNTITLED.to_string() } else { s } };
        let join = |name: &str| if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") };
        let taken = |p: &str| Some(p) != except && (self.root.join(p).exists() || self.notes.iter().any(|n| n.path.as_deref() == Some(p)));
        let first = join(&format!("{stem}.{EXTENSION}"));
        if !taken(&first) {
            return first;
        }
        (2..).map(|n| join(&format!("{stem} {n}.{EXTENSION}"))).find(|p| !taken(p)).unwrap_or(first)
    }

    /// A new note (written once it has text). Returns its id.
    /// Notes never written to a file and still empty (abandoned drafts),
    /// except `keep`: the one just opened to type into.
    pub fn empty_drafts(&self, keep: Option<u64>) -> Vec<u64> {
        self.notes.iter().filter(|n| n.text.trim().is_empty() && n.path.is_none() && Some(n.id) != keep).map(|n| n.id).collect()
    }

    pub fn create(&mut self, topic: u64, text: &str) -> Result<u64> {
        let topic = if self.topic(topic).is_some() { topic } else { DEFAULT_TOPIC };
        let id = self.fresh_id();
        self.notes.insert(0, Note { id, topic, path: None, text: String::new(), updated: unix(SystemTime::now()) });
        if !text.trim().is_empty() {
            self.write(id, text)?;
        }
        self.save_index();
        Ok(id)
    }

    /// Writes a note's text to its file (named after its title the first
    /// time).
    pub fn write(&mut self, id: u64, text: &str) -> Result<()> {
        let pos = self.notes.iter().position(|n| n.id == id).context("no such note")?;
        let path = match self.notes[pos].path.clone() {
            Some(p) => p,
            None => {
                let stem = crate::markdown::title_line(text).unwrap_or_else(|| UNTITLED.into());
                let p = self.free_path(self.notes[pos].topic, &stem, None);
                self.ids.insert(p.clone(), id);
                self.notes[pos].path = Some(p.clone());
                p
            }
        };
        let full = self.root.join(&path);
        if let Some(dir) = full.parent() {
            std::fs::create_dir_all(dir)?;
        }
        mandara_config::write_atomic(&full, text.as_bytes())?;
        let n = &mut self.notes[pos];
        n.text = text.to_string();
        n.updated = unix(SystemTime::now());
        self.save_index();
        Ok(())
    }

    /// Editing ended: an emptied note is deleted, and the file is renamed
    /// to follow the title. Returns whether the note still exists.
    pub fn settle(&mut self, id: u64) -> Result<bool> {
        let Some(n) = self.note(id).cloned() else { return Ok(false) };
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

    fn rename_file(&mut self, id: u64, old: &str, new: &str) -> Result<()> {
        let to = self.root.join(new);
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::rename(self.root.join(old), &to)?;
        self.ids.remove(old);
        self.ids.insert(new.to_string(), id);
        if let Some(n) = self.notes.iter_mut().find(|n| n.id == id) {
            n.path = Some(new.to_string());
        }
        self.save_index();
        Ok(())
    }

    pub fn delete(&mut self, id: u64) -> Result<bool> {
        let Some(pos) = self.notes.iter().position(|n| n.id == id) else { return Ok(false) };
        let n = self.notes.remove(pos);
        if let Some(p) = &n.path {
            match std::fs::remove_file(self.root.join(p)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            self.ids.remove(p);
        }
        self.save_index();
        Ok(true)
    }

    /// Moves a note (its file) to another topic.
    pub fn move_note(&mut self, id: u64, topic: u64) -> Result<()> {
        self.topic(topic).context("no such topic")?;
        let n = self.note(id).cloned().context("no such note")?;
        if n.topic == topic {
            return Ok(());
        }
        if let Some(old) = &n.path {
            let name = Path::new(old).file_stem().and_then(|s| s.to_str()).unwrap_or(UNTITLED).to_string();
            let new = self.free_path(topic, &name, None);
            self.rename_file(id, old, &new)?;
        }
        if let Some(n) = self.notes.iter_mut().find(|n| n.id == id) {
            n.topic = topic;
        }
        Ok(())
    }

    pub fn add_topic(&mut self, name: &str, color: Option<&str>, icon: Option<&str>) -> Result<u64> {
        let dir = safe_name(name);
        anyhow::ensure!(!dir.is_empty(), "a topic needs a name");
        anyhow::ensure!(color.is_none_or(ListColor::valid), "colour must be a tone or hue:<0-359>");
        if let Some(t) = self.topics.iter().find(|t| t.dir == dir) {
            return Ok(t.id);
        }
        std::fs::create_dir_all(self.root.join(&dir))?;
        let id = self.topics.iter().map(|t| t.id).max().unwrap_or(DEFAULT_TOPIC) + 1;
        let color = color.map_or_else(|| format!("hue:{}", SWATCH_HUES[self.topics.len() % SWATCH_HUES.len()]), str::to_string);
        self.topics.push(Topic { id, name: name.split_whitespace().collect::<Vec<_>>().join(" "), dir, color, icon: icon.filter(|i| !i.trim().is_empty()).map(str::to_string) });
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
        let clean = name.split_whitespace().collect::<Vec<_>>().join(" ");
        anyhow::ensure!(!safe_name(&clean).is_empty(), "a topic needs a name");
        let t = self.topic(id).cloned().context("no such topic")?;
        if id != DEFAULT_TOPIC {
            let dir = safe_name(&clean);
            if dir != t.dir {
                anyhow::ensure!(!self.root.join(&dir).exists(), "a folder called {dir} already exists");
                std::fs::rename(self.root.join(&t.dir), self.root.join(&dir))?;
                let prefix = format!("{}/", t.dir);
                let moved: Vec<(String, u64)> = self.ids.iter().filter(|(p, _)| p.starts_with(&prefix)).map(|(p, i)| (p.clone(), *i)).collect();
                for (p, i) in moved {
                    let np = format!("{dir}/{}", &p[prefix.len()..]);
                    self.ids.remove(&p);
                    self.ids.insert(np.clone(), i);
                    if let Some(n) = self.notes.iter_mut().find(|n| n.id == i) {
                        n.path = Some(np);
                    }
                }
                if let Some(t) = self.topics.iter_mut().find(|t| t.id == id) {
                    t.dir = dir;
                }
            }
        }
        if let Some(t) = self.topics.iter_mut().find(|t| t.id == id) {
            t.name = clean;
        }
        self.save_index();
        Ok(())
    }

    pub fn recolor_topic(&mut self, id: u64, color: &str) -> Result<()> {
        anyhow::ensure!(ListColor::valid(color), "colour must be a tone or hue:<0-359>");
        self.topics.iter_mut().find(|t| t.id == id).context("no such topic")?.color = color.to_string();
        self.save_index();
        Ok(())
    }

    pub fn move_topic(&mut self, id: u64, to: usize) -> Result<usize> {
        let from = self.topics.iter().position(|t| t.id == id).context("no such topic")?;
        let t = self.topics.remove(from);
        let to = to.min(self.topics.len());
        self.topics.insert(to, t);
        self.save_index();
        Ok(to)
    }

    /// Deletes a topic; its notes move to the default topic or are deleted.
    /// The folder goes only once empty (other files in it are kept).
    pub fn remove_topic(&mut self, id: u64, notes: OnDelete) -> Result<usize> {
        anyhow::ensure!(id != DEFAULT_TOPIC, "the default topic can be renamed but not deleted");
        let t = self.topic(id).cloned().context("no such topic")?;
        let ids: Vec<u64> = self.notes.iter().filter(|n| n.topic == id).map(|n| n.id).collect();
        for n in &ids {
            match notes {
                OnDelete::MoveToInbox => self.move_note(*n, DEFAULT_TOPIC)?,
                OnDelete::DeleteTasks => {
                    self.delete(*n)?;
                }
            }
        }
        let _ = std::fs::remove_dir(self.root.join(&t.dir));
        self.topics.retain(|t| t.id != id);
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
                if let Ok(f) = std::fs::File::options().write(true).open(self.root.join(&p)) {
                    let _ = f.set_modified(when);
                }
                if let Some(n) = self.notes.iter_mut().find(|n| n.id == id) {
                    n.updated = old.updated;
                }
            }
            written += 1;
        }
        self.notes.sort_by_key(|n| std::cmp::Reverse((n.updated, n.id)));
        Ok(written)
    }

    /// Copies a topic's notes (or all, `None`) into `dir` as `.md` files.
    pub fn export(&self, topic: Option<u64>, dir: &Path) -> Result<usize> {
        std::fs::create_dir_all(dir)?;
        let mut n = 0;
        let mut used: Vec<String> = Vec::new();
        for note in self.notes.iter().filter(|x| topic.is_none_or(|t| x.topic == t) && !x.text.trim().is_empty()) {
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

    /// `(topic, count)` for the topic switcher.
    pub fn count(&self, topic: Option<u64>) -> usize {
        self.notes.iter().filter(|n| topic.is_none_or(|t| n.topic == t) && !n.text.trim().is_empty()).count()
    }

    /// Notes matching `query` (fuzzy over title and body), best first.
    pub fn search(&self, query: &str) -> Vec<u64> {
        use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
        use nucleo_matcher::{Config, Matcher, Utf32Str};
        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut buf = Vec::new();
        let mut hits: Vec<(u32, u64)> = self
            .notes
            .iter()
            .filter_map(|n| {
                let hay = format!("{} {}", n.title(), n.text);
                pattern.score(Utf32Str::new(&hay, &mut buf), &mut matcher).map(|s| (s, n.id))
            })
            .collect();
        hits.sort_by_key(|(s, id)| (std::cmp::Reverse(*s), *id));
        hits.into_iter().map(|(_, id)| id).collect()
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
