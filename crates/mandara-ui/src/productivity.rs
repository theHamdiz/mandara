//! Shared state for the productivity plugins: to-dos and notes (persisted
//! to `~/.local/share/mandara/productivity.json`) and the Pomodoro timer.
//! One entity backs every bar item, panel, sidebar card and command, so a
//! change made anywhere shows up everywhere at once.

use std::time::{Duration, Instant};
use std::collections::BTreeMap;

mod notes_worker;
mod note_rescan;
mod note_edit;
mod note_conflict;
mod note_flush;
mod notes;
mod todo_writer;
mod load;
pub use notes_worker::NoteReply;
use notes_worker::NotesWorker;

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};

use crate::state::config;
use crate::notes_store::{LegacyNote, NotesSnapshot};
use crate::todo_lists::{Filter, ListError, TodoBook, TodoList, parse_quick_add};

/// Live edits share this debounce; explicit saves and command receipts flush sooner.
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

    /// A phase that hasn't started yet takes its length from `rules`, so a
    /// new focus or break length shows at once; a running or paused one
    /// keeps its time. Returns whether anything changed.
    pub fn follow(&mut self, rules: &PomodoroRules) -> bool {
        let fresh = !self.running() && self.paused_left == self.length;
        let length = rules.length(self.phase);
        if !fresh || length == self.length {
            return false;
        }
        self.length = length;
        self.paused_left = length;
        true
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
    pub notes: NotesSnapshot,
    /// Notes typed but not yet written to their files.
    notes_dirty: BTreeMap<u64, u64>,
    notes_generation: u64,
    notes_submitted: BTreeMap<u64, u64>,
    notes_tasks: BTreeMap<u64, note_edit::TaskPatches>,
    /// [`crate::notes_store::body_hash`] of the file body each typed draft
    /// replaces, from its first unsaved edit until its write is
    /// acknowledged; after a conflict, the body found elsewhere, so the next
    /// edit replaces that deliberately.
    notes_base: BTreeMap<u64, u64>,
    /// Conflicting drafts stay recoverable per folder without becoming write prerequisites.
    notes_conflicts: BTreeMap<(std::path::PathBuf, u64), note_conflict::RetainedDraft>,
    notes_flush: note_flush::NoteFlush,
    notes_rescan: note_rescan::NoteRescan,
    notes_sequence: u64,
    notes_applied: u64,
    notes_worker: Option<NotesWorker>,
    notes_changes: async_channel::Sender<()>,
    notes_switching: bool,
    pub(crate) ready: bool,
    pub(crate) notes_ready: bool,
    pub(crate) load_error: Option<String>,
    pub(crate) todo_error: Option<String>,
    pub(crate) notes_error: Option<String>,
    notes_task: Option<Task<()>>,
    /// Legacy notes that could not be written out (kept in the JSON).
    legacy_notes: Vec<LegacyNote>,
    /// Watches the notes folder so external edits show up live.
    notes_watch: Option<Task<()>>,
    /// The topic the notes card and panel show (`None` = all).
    pub notes_topic: Option<u64>,
    pub pomodoro: Pomodoro,
    next_id: u64,
    save_task: Option<Task<()>>,
    todo_writes: todo_writer::TodoWrites,
    tick_task: Option<Task<()>>,
}

/// `productivity.json` under the data dir: lists, tasks, the filter and
/// any legacy notes not yet written out.
fn store_path() -> std::path::PathBuf {
    mandara_config::paths::data_dir().join("productivity.json")
}

/// The current time in Unix seconds (edit timestamps).
fn now_unix() -> i64 {
    chrono::Local::now().timestamp()
}

impl Productivity {
    /// The model before anything is loaded: no tasks, the notes folder
    /// `root` not yet scanned and no notes worker, so edits wait for the load.
    fn empty(rules: PomodoroRules, root: std::path::PathBuf, changes: async_channel::Sender<()>) -> Self {
        Self {
            book: TodoBook::new(Vec::new(), Vec::new()), filter: Filter::All,
            notes: NotesSnapshot::empty(root), notes_dirty: BTreeMap::new(),
            notes_generation: 0, notes_submitted: BTreeMap::new(), notes_tasks: BTreeMap::new(), notes_base: BTreeMap::new(), notes_conflicts: BTreeMap::new(), notes_flush: note_flush::NoteFlush::default(), notes_rescan: note_rescan::NoteRescan::default(), notes_sequence: 0, notes_applied: 0,
            notes_worker: None, notes_changes: changes, notes_switching: false, ready: false, notes_ready: false, load_error: None, todo_error: None, notes_error: None,
            notes_task: None, legacy_notes: Vec::new(), notes_watch: None, notes_topic: None,
            pomodoro: Pomodoro { phase: Phase::Focus, ends_at: None, paused_left: rules.focus, length: rules.focus, completed: 0 },
            next_id: 1, save_task: None, todo_writes: todo_writer::TodoWrites::default(), tick_task: None,
        }
    }

    /// The next task id.
    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id - 1
    }

    // ── To-dos ──────────────────────────────────────────────────────────

    /// Adds a task to `list`; a leading `#Topic` in `text` picks (or
    /// makes) that list instead. Returns the task id and its list.
    pub fn add_todo(&mut self, text: &str, list: u64, cx: &mut Context<Self>) -> Option<(u64, u64)> {
        if !self.ready { return None; }
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
        if !self.ready { return false; }
        let Some(t) = self.book.todos.iter_mut().find(|t| t.id == id) else { return false };
        t.done = done;
        self.save(cx);
        true
    }

    pub fn remove_todo(&mut self, id: u64, cx: &mut Context<Self>) -> bool {
        if !self.ready { return false; }
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
        if !self.ready { return 0; }
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
        if !self.ready { return Err(ListError::NoSuchList); }
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
        if !self.ready { return; }
        if self.filter != filter {
            self.filter = filter;
            self.save(cx);
        }
    }

    // ── Notes ───────────────────────────────────────────────────────────

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

/// Creates the shared model and loads the to-dos and notes folder on the
/// blocking pool; the notes worker starts with the loaded store. Folder
/// changes are coalesced into one rescan per settle period, and config
/// edits move the notes folder and the timer lengths.
pub fn init(cx: &mut App) -> Entity<Productivity> {
    let rules = PomodoroRules::from_config(cx);
    let root = notes_root(&config(cx).read(cx).snapshot());
    let (changes, changed) = async_channel::bounded(1);
    let entity = cx.new(|cx: &mut Context<Productivity>| {
        let mut p = Productivity::empty(rules, root.clone(), changes.clone());
        cx.spawn(async move |this, cx| {
            let loaded = mandara_services::runtime::unblock(move || {
                let mut loaded = load::books(&store_path(), &root);
                if let Err(error) = loaded.notes.watch_changes(changes) { tracing::warn!("watching notes: {error:#}"); }
                loaded
            }).await;
            let _ = this.update(cx, |p, cx| {
                let load::Loaded { todos, notes, migrated } = loaded;
                match todos {
                    Ok(saved) => {
                        p.next_id = saved.todos.iter().map(|task| task.id).max().unwrap_or(0) + 1;
                        p.book = TodoBook::new(saved.lists, saved.todos);
                        p.filter = match saved.filter { Filter::List(id) if p.book.list(id).is_none() => Filter::All, filter => filter };
                        p.legacy_notes = saved.notes;
                        p.ready = true;
                    }
                    Err(error) => {
                        tracing::warn!("loading productivity data: {error}");
                        p.load_error = Some(error);
                    }
                }
                p.notes_error = notes.index_result().err().map(|error| format!("{error:#}"));
                p.notes = notes.snapshot();
                p.notes_worker = Some(NotesWorker::start(notes));
                p.notes_ready = true;
                p.ensure_notes_root(notes_root(&config(cx).read(cx).snapshot()), cx);
                if migrated { p.save(cx); }
                cx.notify();
            });
        }).detach();
        p.notes_watch = Some(cx.spawn(async move |this, cx| {
            while changed.recv().await.is_ok() {
                cx.background_executor().timer(NOTES_WATCH_SETTLE).await;
                while changed.try_recv().is_ok() {}
                if this.update(cx, |p, cx| p.rescan_notes(cx)).is_err() { break; }
            }
        }));
        cx.observe(&config(cx), |p: &mut Productivity, store, cx| {
            let root = notes_root(&store.read(cx).snapshot());
            p.ensure_notes_root(root, cx);
            // New Pomodoro lengths apply to a timer that hasn't started.
            if p.pomodoro.follow(&PomodoroRules::from_config(cx)) {
                cx.notify();
            }
        })
        .detach();
        p
    });
    cx.set_global(GlobalProductivity(entity.clone()));
    entity
}

/// The shared model ([`init`] must have run).
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
    fn an_idle_timer_follows_new_lengths_but_a_started_one_keeps_its_time() {
        let mins = |m: u64| Duration::from_secs(m * 60);
        let rules = |focus: u64, short: u64| PomodoroRules { focus: mins(focus), short_break: mins(short), long_break: mins(15), long_every: 4, auto_continue: true };
        // Issue #7: a 50-minute focus shows at once on a fresh timer.
        let mut p = Pomodoro { phase: Phase::Focus, ends_at: None, paused_left: mins(25), length: mins(25), completed: 0 };
        assert!(p.follow(&rules(50, 5)));
        assert_eq!((p.length, p.clock().as_str()), (mins(50), "50:00"));
        assert!(!p.follow(&rules(50, 5)), "nothing new");
        // Paused halfway: the session keeps its own time.
        p.paused_left = mins(20);
        assert!(!p.follow(&rules(30, 5)));
        assert_eq!(p.paused_left, mins(20));
        // A fresh break follows the break length.
        let mut b = Pomodoro { phase: Phase::ShortBreak, ends_at: None, paused_left: mins(5), length: mins(5), completed: 1 };
        assert!(b.follow(&rules(25, 10)));
        assert_eq!(b.length, mins(10));
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
