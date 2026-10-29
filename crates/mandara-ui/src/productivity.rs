//! Shared state for the productivity plugins: to-dos and notes (persisted
//! to `~/.local/share/mandara/productivity.json`) and the Pomodoro timer.
//! One entity backs every bar item, panel, sidebar card and command, so a
//! change made anywhere shows up everywhere at once.

use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};

use crate::state::config;
use crate::notes_store::{LegacyNote, NoteStore};
use crate::todo_lists::{Filter, ListError, TodoBook, TodoList, parse_quick_add};

/// Notes / to-dos are written this long after the last edit (disk debounce).
const SAVE_DEBOUNCE: Duration = Duration::from_millis(300);
/// The countdown ticks on whole seconds.
const SECOND: Duration = Duration::from_secs(1);
/// Shortest timer wait near the end of a phase.
const MIN_TICK: Duration = Duration::from_millis(10);

pub const POMODORO_ID: &str = "pomodoro";
/// The notes plugin and its storage-folder setting.
pub const NOTES_ID: &str = "notes";
pub const NOTES_FOLDER_KEY: &str = "storage_folder";
/// External changes to the notes folder settle this long before a rescan.
const NOTES_WATCH_SETTLE: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Todo {
    pub id: u64,
    pub text: String,
    pub done: bool,
    /// Unix seconds.
    pub created: i64,
    /// The list it belongs to (`todo_lists::INBOX_ID` in older files).
    #[serde(default)]
    pub list: u64,
}

/// `productivity.json`. Every field defaults, so older files load.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Saved {
    pub(crate) lists: Vec<TodoList>,
    pub(crate) todos: Vec<Todo>,
    /// Notes from before they became Markdown files: read, moved into the
    /// notes folder once, and only written back if that failed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) notes: Vec<LegacyNote>,
    /// The list the to-do card and panel show.
    pub(crate) filter: Filter,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Focus,
    ShortBreak,
    LongBreak,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Focus => t("Focus"),
            Phase::ShortBreak => t("Short break"),
            Phase::LongBreak => t("Long break"),
        }
    }
}

/// Pomodoro durations and rules from `[plugins.settings.pomodoro]`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PomodoroRules {
    pub focus: Duration,
    pub short_break: Duration,
    pub long_break: Duration,
    /// A long break follows every N focus sessions.
    pub long_every: u32,
    /// Start the next phase automatically when one ends.
    pub auto_continue: bool,
}

impl PomodoroRules {
    pub fn from_config(cx: &App) -> Self {
        let cfg = config(cx).read(cx).snapshot();
        let s = &cfg.plugins;
        let mins = |key: &str, default: f64| Duration::from_secs_f64(s.setting(POMODORO_ID, key, default).clamp(1.0, 240.0) * 60.0);
        Self {
            focus: mins("focus_minutes", 25.0),
            short_break: mins("short_break_minutes", 5.0),
            long_break: mins("long_break_minutes", 15.0),
            long_every: s.setting(POMODORO_ID, "long_break_every", 4u32).max(1),
            auto_continue: s.setting(POMODORO_ID, "auto_continue", true),
        }
    }

    pub fn length(&self, phase: Phase) -> Duration {
        match phase {
            Phase::Focus => self.focus,
            Phase::ShortBreak => self.short_break,
            Phase::LongBreak => self.long_break,
        }
    }

    /// The phase after `phase`, given focus sessions completed so far.
    pub fn next(&self, phase: Phase, completed: u32) -> Phase {
        match phase {
            Phase::Focus if completed > 0 && completed.is_multiple_of(self.long_every) => Phase::LongBreak,
            Phase::Focus => Phase::ShortBreak,
            _ => Phase::Focus,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pomodoro {
    pub phase: Phase,
    /// Set while running.
    pub ends_at: Option<Instant>,
    /// Time left while paused (or the full length before starting).
    pub paused_left: Duration,
    pub length: Duration,
    /// Focus sessions completed today (this run of the shell).
    pub completed: u32,
}

impl Pomodoro {
    pub fn running(&self) -> bool {
        self.ends_at.is_some()
    }

    pub fn left(&self) -> Duration {
        match self.ends_at {
            Some(end) => end.saturating_duration_since(Instant::now()),
            None => self.paused_left,
        }
    }

    /// Elapsed fraction of the current phase, `0..=1`.
    pub fn progress(&self) -> f32 {
        if self.length.is_zero() {
            return 0.0;
        }
        1.0 - self.left().as_secs_f32() / self.length.as_secs_f32()
    }

    /// `mm:ss` of the time left.
    pub fn clock(&self) -> String {
        let s = self.left().as_secs_f32().ceil() as u64;
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

pub struct Productivity {
    /// Every to-do list and task.
    pub book: TodoBook,
    /// The list the to-do card and panel show (remembered).
    pub filter: Filter,
    /// Notes: Markdown files in topic folders (see [`crate::notes_store`]).
    pub notes: NoteStore,
    /// Notes typed but not yet written to their files.
    notes_dirty: Vec<u64>,
    notes_task: Option<Task<()>>,
    /// Legacy notes that could not be written out (kept in the JSON).
    legacy_notes: Vec<LegacyNote>,
    /// Watches the notes folder so external edits show up live.
    notes_watch: Option<(notify::RecommendedWatcher, Task<()>)>,
    /// The topic the notes card and panel show (`None` = all).
    pub notes_topic: Option<u64>,
    pub pomodoro: Pomodoro,
    next_id: u64,
    save_task: Option<Task<()>>,
    tick_task: Option<Task<()>>,
}

fn store_path() -> std::path::PathBuf {
    mandara_config::paths::data_dir().join("productivity.json")
}

fn now_unix() -> i64 {
    chrono::Local::now().timestamp()
}

impl Productivity {
    /// Loads everything; the flag asks for a save (notes just moved out of
    /// the JSON into files).
    fn load(rules: PomodoroRules, notes_root: std::path::PathBuf) -> (Self, bool) {
        let saved: Saved = std::fs::read(store_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let next_id = saved.todos.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        let mut notes = NoteStore::open(&notes_root);
        let had_legacy = !saved.notes.is_empty();
        let legacy_notes = match notes.migrate(&saved.notes) {
            Ok(0) => Vec::new(),
            Ok(n) => {
                tracing::info!("moved {n} notes into {}", notes.root.display());
                Vec::new()
            }
            Err(err) => {
                tracing::error!("moving notes into {}: {err:#} (kept in productivity.json)", notes.root.display());
                saved.notes
            }
        };
        let book = TodoBook::new(saved.lists, saved.todos);
        let filter = match saved.filter {
            Filter::List(id) if book.list(id).is_none() => Filter::All,
            f => f,
        };
        let resave = had_legacy && legacy_notes.is_empty();
        let this = Self {
            book,
            filter,
            notes,
            notes_dirty: Vec::new(),
            notes_task: None,
            legacy_notes,
            notes_watch: None,
            notes_topic: None,
            pomodoro: Pomodoro { phase: Phase::Focus, ends_at: None, paused_left: rules.focus, length: rules.focus, completed: 0 },
            next_id,
            save_task: None,
            tick_task: None,
        };
        (this, resave)
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id - 1
    }

    /// Persists to disk shortly after the last change (atomic write).
    fn save(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        let json = serde_json::to_vec_pretty(&Saved { lists: self.book.lists.clone(), todos: self.book.todos.clone(), notes: self.legacy_notes.clone(), filter: self.filter });
        self.save_task = Some(cx.spawn(async move |_, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            let Ok(json) = json else { return };
            let path = store_path();
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(err) = mandara_config::write_atomic(&path, &json) {
                tracing::error!("saving notes / to-dos: {err:#}");
            }
        }));
    }

    // ── To-dos ──────────────────────────────────────────────────────────

    /// Adds a task to `list`; a leading `#Topic` in `text` picks (or
    /// makes) that list instead. Returns the task id and its list.
    pub fn add_todo(&mut self, text: &str, list: u64, cx: &mut Context<Self>) -> Option<(u64, u64)> {
        let (topic, body) = parse_quick_add(text);
        if body.is_empty() {
            return None;
        }
        let list = match topic {
            Some(topic) => self.book.list_for_topic(topic),
            None => list,
        };
        let id = self.id();
        self.book.add(id, body, list, now_unix())?;
        self.save(cx);
        Some((id, self.book.todos[0].list))
    }

    pub fn set_done(&mut self, id: u64, done: bool, cx: &mut Context<Self>) -> bool {
        let Some(t) = self.book.todos.iter_mut().find(|t| t.id == id) else { return false };
        t.done = done;
        self.save(cx);
        true
    }

    pub fn remove_todo(&mut self, id: u64, cx: &mut Context<Self>) -> bool {
        let before = self.book.todos.len();
        self.book.todos.retain(|t| t.id != id);
        let removed = before != self.book.todos.len();
        if removed {
            self.save(cx);
        }
        removed
    }

    /// Deletes the completed tasks the filter shows.
    pub fn clear_done(&mut self, filter: Filter, cx: &mut Context<Self>) -> usize {
        let before = self.book.todos.len();
        self.book.todos.retain(|t| !(t.done && filter.shows(t)));
        self.save(cx);
        before - self.book.todos.len()
    }

    pub fn open_todos(&self, filter: Filter) -> usize {
        self.book.counts(filter).0
    }

    /// Runs a list edit and saves when it succeeds.
    pub fn edit_lists<T>(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut TodoBook) -> Result<T, ListError>) -> Result<T, ListError> {
        let out = f(&mut self.book)?;
        if let Filter::List(id) = self.filter
            && self.book.list(id).is_none()
        {
            self.filter = Filter::All;
        }
        self.save(cx);
        Ok(out)
    }

    pub fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        if self.filter != filter {
            self.filter = filter;
            self.save(cx);
        }
    }

    // ── Notes ───────────────────────────────────────────────────────────

    /// Notes changed: redraw, and write typed text shortly after the last
    /// keystroke.
    fn notes_changed(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        if self.notes_dirty.is_empty() {
            return;
        }
        self.notes_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            let _ = this.update(cx, |p, _| p.flush_notes());
        }));
    }

    /// Writes every typed-but-unsaved note now.
    pub fn flush_notes(&mut self) {
        for id in std::mem::take(&mut self.notes_dirty) {
            let Some(text) = self.notes.note(id).map(|n| n.text.clone()) else { continue };
            if text.trim().is_empty() && self.notes.note(id).is_some_and(|n| n.path.is_none()) {
                continue;
            }
            if let Err(err) = self.notes.write(id, &text) {
                tracing::error!("saving note {id}: {err:#}");
            }
        }
    }

    /// Adds a note to `topic`; a leading `#Topic` picks (or makes) that
    /// topic instead. Returns the note id.
    pub fn add_note(&mut self, text: &str, topic: u64, cx: &mut Context<Self>) -> anyhow::Result<u64> {
        let (tag, body) = parse_quick_add(text);
        let topic = match tag {
            Some(tag) => self.notes.topic_for(tag),
            None => topic,
        };
        let id = self.notes.create(topic, body)?;
        cx.notify();
        Ok(id)
    }

    /// Replaces a note's text and writes it at once; an emptied note is
    /// deleted.
    pub fn set_note(&mut self, id: u64, text: &str, cx: &mut Context<Self>) -> bool {
        if self.notes.note(id).is_none() {
            return false;
        }
        let ok = if text.trim().is_empty() { self.notes.delete(id).map(|_| ()) } else { self.notes.write(id, text) };
        if let Err(err) = ok {
            tracing::error!("saving note {id}: {err:#}");
        }
        cx.notify();
        true
    }

    /// Live edit from the editor: text in place, written after a pause.
    pub fn edit_note(&mut self, id: u64, text: &str, cx: &mut Context<Self>) {
        let Some(n) = self.notes.notes.iter_mut().find(|n| n.id == id) else { return };
        if n.text != text {
            n.text = text.to_string();
            n.updated = now_unix();
            if !self.notes_dirty.contains(&id) {
                self.notes_dirty.push(id);
            }
            self.notes_changed(cx);
        }
    }

    /// Editing of `id` ended: write it, rename its file after its title,
    /// drop it when empty.
    pub fn settle_note(&mut self, id: u64, cx: &mut Context<Self>) {
        self.flush_notes();
        if let Err(err) = self.notes.settle(id) {
            tracing::warn!("tidying note {id}: {err:#}");
        }
        cx.notify();
    }

    /// Drops notes left empty (e.g. a new note closed without typing).
    pub fn prune_empty(&mut self, cx: &mut Context<Self>) {
        let empty: Vec<u64> = self.notes.notes.iter().filter(|n| n.text.trim().is_empty() && n.path.is_none()).map(|n| n.id).collect();
        for id in &empty {
            let _ = self.notes.delete(*id);
        }
        if !empty.is_empty() {
            cx.notify();
        }
    }

    pub fn remove_note(&mut self, id: u64, cx: &mut Context<Self>) -> bool {
        self.notes_dirty.retain(|d| *d != id);
        let removed = self.notes.delete(id).unwrap_or_else(|err| {
            tracing::error!("deleting note {id}: {err:#}");
            false
        });
        cx.notify();
        removed
    }

    /// Runs a store edit (topics, moves) and redraws.
    pub fn edit_notes<T>(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut NoteStore) -> anyhow::Result<T>) -> anyhow::Result<T> {
        self.flush_notes();
        let out = f(&mut self.notes);
        if let Some(t) = self.notes_topic
            && self.notes.topic(t).is_none()
        {
            self.notes_topic = None;
        }
        cx.notify();
        out
    }

    pub fn set_notes_topic(&mut self, topic: Option<u64>, cx: &mut Context<Self>) {
        self.notes_topic = topic;
        cx.notify();
    }

    /// Re-reads the notes folder (external edits), keeping unsaved text.
    fn rescan_notes(&mut self, cx: &mut Context<Self>) {
        let keep = self.notes_dirty.clone();
        self.notes.rescan(&keep);
        cx.notify();
    }

    /// Follows the `storage_folder` setting and (re)starts the watcher.
    fn ensure_notes_root(&mut self, root: std::path::PathBuf, cx: &mut Context<Self>) {
        if self.notes.root != root {
            self.flush_notes();
            self.notes = NoteStore::open(&root);
            self.notes_topic = None;
            self.notes_watch = None;
            cx.notify();
        }
        if self.notes_watch.is_some() {
            return;
        }
        use notify::Watcher as _;
        let (tx, rx) = async_channel::bounded::<()>(1);
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if res.is_ok_and(|e| !e.kind.is_access()) {
                let _ = tx.try_send(());
            }
        });
        let mut watcher = match watcher {
            Ok(w) => w,
            Err(err) => return tracing::warn!("watching the notes folder: {err:#}"),
        };
        if let Err(err) = watcher.watch(&root, notify::RecursiveMode::Recursive) {
            return tracing::warn!("watching {}: {err:#}", root.display());
        }
        let task = cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                cx.background_executor().timer(NOTES_WATCH_SETTLE).await;
                while rx.try_recv().is_ok() {}
                if this.update(cx, |p, cx| p.rescan_notes(cx)).is_err() {
                    break;
                }
            }
        });
        self.notes_watch = Some((watcher, task));
    }

    // ── Pomodoro ────────────────────────────────────────────────────────

    pub fn start(&mut self, cx: &mut Context<Self>) {
        if self.pomodoro.running() {
            return;
        }
        let left = if self.pomodoro.paused_left.is_zero() { self.pomodoro.length } else { self.pomodoro.paused_left };
        self.pomodoro.ends_at = Some(Instant::now() + left);
        self.ensure_ticking(cx);
        cx.notify();
    }

    pub fn pause(&mut self) {
        if let Some(end) = self.pomodoro.ends_at.take() {
            self.pomodoro.paused_left = end.saturating_duration_since(Instant::now());
        }
        self.tick_task = None;
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.pomodoro.running() {
            self.pause();
            cx.notify();
        } else {
            self.start(cx);
        }
    }

    /// Back to the start of a fresh focus session.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        let rules = PomodoroRules::from_config(cx);
        self.pause();
        self.pomodoro = Pomodoro { phase: Phase::Focus, ends_at: None, paused_left: rules.focus, length: rules.focus, completed: self.pomodoro.completed };
        cx.notify();
    }

    /// Ends the current phase now (counting a finished focus session).
    pub fn skip(&mut self, cx: &mut Context<Self>) {
        self.advance(false, cx);
    }

    fn advance(&mut self, finished: bool, cx: &mut Context<Self>) {
        let rules = PomodoroRules::from_config(cx);
        let was_running = self.pomodoro.running();
        let ended = self.pomodoro.phase;
        if ended == Phase::Focus {
            self.pomodoro.completed += 1;
        }
        let next = rules.next(ended, self.pomodoro.completed);
        let length = rules.length(next);
        self.pomodoro.phase = next;
        self.pomodoro.length = length;
        self.pomodoro.paused_left = length;
        self.pomodoro.ends_at = None;
        self.tick_task = None;
        if finished {
            let (summary, body) = match next {
                Phase::Focus => (t("Break's over").to_string(), tf("Time to focus for {minutes} minutes.", &[("minutes", &(length.as_secs() / 60))])),
                _ => (
                    tf("Focus session {count} done", &[("count", &self.pomodoro.completed)]),
                    if next == Phase::LongBreak {
                        tf("Take a long break — {minutes} minutes.", &[("minutes", &(length.as_secs() / 60))])
                    } else {
                        tf("Take a short break — {minutes} minutes.", &[("minutes", &(length.as_secs() / 60))])
                    },
                ),
            };
            crate::plugins::notifications::send(
                cx,
                mandara_services::NotificationCommand::Post { app: t("Pomodoro").into(), icon: "timer".into(), summary, body },
            );
        }
        if (finished && rules.auto_continue) || (!finished && was_running) {
            self.start(cx);
        }
        cx.notify();
    }

    /// One task per running timer: wakes each second to refresh readouts
    /// and fires the phase change on time.
    fn ensure_ticking(&mut self, cx: &mut Context<Self>) {
        if self.tick_task.is_some() {
            return;
        }
        self.tick_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let wait = this
                    .read_with(cx, |p, _| {
                        let left = p.pomodoro.left();
                        // Land on whole-second boundaries so mm:ss flips crisply.
                        let frac = Duration::from_nanos((left.as_nanos() % 1_000_000_000) as u64);
                        if frac.is_zero() { SECOND } else { frac }.min(left.max(MIN_TICK))
                    })
                    .unwrap_or(SECOND);
                cx.background_executor().timer(wait).await;
                let done = this
                    .update(cx, |p, cx| {
                        if !p.pomodoro.running() {
                            return true;
                        }
                        if p.pomodoro.left().is_zero() {
                            p.tick_task = None;
                            p.advance(true, cx);
                            return true;
                        }
                        cx.notify();
                        false
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
        }));
    }
}

#[derive(Clone)]
pub struct GlobalProductivity(pub Entity<Productivity>);
impl Global for GlobalProductivity {}

/// The notes folder the config asks for (`storage_folder`, else the
/// default under the data dir).
pub fn notes_root(cfg: &mandara_config::ShellConfig) -> std::path::PathBuf {
    let folder: String = cfg.plugins.setting(NOTES_ID, NOTES_FOLDER_KEY, String::new());
    if folder.trim().is_empty() { crate::notes_store::default_root() } else { mandara_config::paths::expand_tilde(folder.trim()) }
}

pub fn init(cx: &mut App) -> Entity<Productivity> {
    let rules = PomodoroRules::from_config(cx);
    let root = notes_root(&config(cx).read(cx).snapshot());
    let entity = cx.new(|cx| {
        let (mut p, resave) = Productivity::load(rules, root.clone());
        p.ensure_notes_root(root, cx);
        if resave {
            p.save(cx);
        }
        cx.observe(&config(cx), |p: &mut Productivity, store, cx| {
            let root = notes_root(&store.read(cx).snapshot());
            p.ensure_notes_root(root, cx);
        })
        .detach();
        p
    });
    cx.set_global(GlobalProductivity(entity.clone()));
    entity
}

pub fn productivity(cx: &App) -> Entity<Productivity> {
    cx.global::<GlobalProductivity>().0.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> PomodoroRules {
        PomodoroRules {
            focus: Duration::from_secs(1500),
            short_break: Duration::from_secs(300),
            long_break: Duration::from_secs(900),
            long_every: 4,
            auto_continue: true,
        }
    }

    #[test]
    fn long_break_every_fourth_session() {
        let r = rules();
        assert_eq!(r.next(Phase::Focus, 1), Phase::ShortBreak);
        assert_eq!(r.next(Phase::Focus, 3), Phase::ShortBreak);
        assert_eq!(r.next(Phase::Focus, 4), Phase::LongBreak);
        assert_eq!(r.next(Phase::ShortBreak, 4), Phase::Focus);
        assert_eq!(r.next(Phase::LongBreak, 8), Phase::Focus);
    }

    #[test]
    fn clock_and_progress() {
        let p = Pomodoro { phase: Phase::Focus, ends_at: None, paused_left: Duration::from_secs(605), length: Duration::from_secs(1500), completed: 0 };
        assert_eq!(p.clock(), "10:05");
        assert!((p.progress() - (1.0 - 605.0 / 1500.0)).abs() < 1e-4);
    }

    #[test]
    fn note_title_and_preview() {
        use crate::notes_store::Note;
        let n = Note { id: 1, topic: 0, path: None, text: "\n  ## Groceries \nmilk\n\n- **eggs**".into(), updated: 0 };
        assert_eq!(n.title(), "Groceries");
        assert_eq!(n.preview(), "milk eggs");
        assert_eq!(Note { id: 2, topic: 0, path: None, text: "   ".into(), updated: 0 }.title(), "Untitled note");
    }
}
