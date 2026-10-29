//! To-do lists ("topics"): every task belongs to one list; the built-in
//! Inbox (id 0) can be renamed but never deleted. Pure data and rules,
//! shared by the to-do plugin's views, its commands and the launcher's
//! `/todo #Topic text`; persisted inside `productivity.json` by
//! [`crate::productivity`].
//!
//! Files written before lists existed have no `lists` and no per-task
//! `list`: they load with every task in the Inbox ([`TodoBook::repair`]).

use serde::{Deserialize, Serialize};

use crate::productivity::Todo;

/// The Inbox: the default list, which always exists.
pub const INBOX_ID: u64 = 0;
/// Its English name (shown translated until the user renames it).
pub const INBOX_NAME: &str = "Inbox";
/// Longest list name kept (characters).
pub const MAX_LIST_NAME_CHARS: usize = 32;
/// Colour of a list made without one (a palette tone, see [`ListColor`]).
pub const DEFAULT_LIST_COLOR: &str = "primary";
/// Prefix of a quick-add topic: `#Work call the bank`.
const TOPIC_MARK: char = '#';

/// Palette tones a list can wear (theme-aware: they follow light / dark and
/// wallpaper colours).
pub const TONES: &[&str] = &["primary", "secondary", "tertiary", "success", "warning", "urgent"];
/// OKLCH hues offered as fixed swatches next to the tones (`hue:<deg>`,
/// drawn at the palette's own lightness / chroma).
pub const SWATCH_HUES: &[u16] = &[25, 70, 110, 160, 200, 250, 290, 330];

/// A list's colour: a palette tone name or an OKLCH hue.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ListColor<'a> {
    Tone(&'a str),
    Hue(u16),
}

impl<'a> ListColor<'a> {
    /// Parses `primary` … `urgent` or `hue:<0-359>` (unknown → primary).
    pub fn parse(s: &'a str) -> Self {
        if let Some(h) = s.strip_prefix("hue:").and_then(|h| h.trim().parse::<u16>().ok()) {
            return ListColor::Hue(h % 360);
        }
        ListColor::Tone(TONES.iter().find(|t| **t == s).copied().unwrap_or(DEFAULT_LIST_COLOR))
    }

    pub fn valid(s: &str) -> bool {
        TONES.contains(&s) || s.strip_prefix("hue:").is_some_and(|h| h.trim().parse::<u16>().is_ok_and(|h| h < 360))
    }
}

/// One list of tasks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TodoList {
    pub id: u64,
    pub name: String,
    /// See [`ListColor`].
    pub color: String,
    /// Optional Mandara icon name (a coloured dot otherwise).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// The list's completed tasks are folded open.
    pub show_done: bool,
}

impl Default for TodoList {
    fn default() -> Self {
        Self { id: INBOX_ID, name: INBOX_NAME.into(), color: DEFAULT_LIST_COLOR.into(), icon: None, show_done: false }
    }
}

impl TodoList {
    pub fn inbox() -> Self {
        Self::default()
    }

    /// Still called by its shipped name (shown translated).
    pub fn has_default_name(&self) -> bool {
        self.id == INBOX_ID && self.name == INBOX_NAME
    }
}

/// What happens to a deleted list's tasks.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OnDelete {
    MoveToInbox,
    DeleteTasks,
}

/// How tasks are ordered inside a list (open ones always above done ones).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SortOrder {
    #[default]
    Newest,
    Oldest,
    Alphabetical,
}

impl SortOrder {
    pub const CHOICES: &[&str] = &["newest", "oldest", "alphabetical"];

    pub fn parse(s: &str) -> Self {
        match s {
            "oldest" => SortOrder::Oldest,
            "alphabetical" => SortOrder::Alphabetical,
            _ => SortOrder::Newest,
        }
    }
}

/// Which tasks a surface shows.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Filter {
    #[default]
    All,
    List(u64),
}

/// Why a list edit was refused (i18n keys).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ListError {
    NoSuchList,
    NoSuchTask,
    EmptyName,
    InboxStays,
    BadColor,
}

impl ListError {
    pub fn message(self) -> &'static str {
        match self {
            ListError::NoSuchList => mandara_i18n::tk("No such list (use its name or id; see `todo lists`)"),
            ListError::NoSuchTask => mandara_i18n::tk("No such task (see `todo list`)"),
            ListError::EmptyName => mandara_i18n::tk("A list needs a name"),
            ListError::InboxStays => mandara_i18n::tk("The Inbox can be renamed but not deleted"),
            ListError::BadColor => mandara_i18n::tk("Colour must be a tone (primary, secondary, tertiary, success, warning, urgent) or hue:<0-359>"),
        }
    }
}

fn clean_name(name: &str) -> Option<String> {
    let flat = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let capped: String = flat.chars().take(MAX_LIST_NAME_CHARS).collect();
    (!capped.is_empty()).then_some(capped)
}

/// A name squashed for `#topic` matching: lowercase, no spaces, `-` or `_`
/// (`#side-project` and `#SideProject` both find "Side project").
fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '_').flat_map(char::to_lowercase).collect()
}

/// Splits a quick-add line into its `#Topic` (leading, one word) and the
/// task text: `"#Work call the bank"` → `(Some("Work"), "call the bank")`.
/// A lone `#` or a `#` later in the text is part of the task.
pub fn parse_quick_add(line: &str) -> (Option<&str>, &str) {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix(TOPIC_MARK) {
        let (topic, text) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        if !topic.is_empty() && !topic.starts_with(TOPIC_MARK) {
            return (Some(topic), text.trim());
        }
    }
    (None, line)
}

/// Every list and task.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TodoBook {
    pub lists: Vec<TodoList>,
    pub todos: Vec<Todo>,
}

impl TodoBook {
    pub fn new(lists: Vec<TodoList>, todos: Vec<Todo>) -> Self {
        let mut b = Self { lists, todos };
        b.repair();
        b
    }

    /// The Inbox exists (first), ids are unique, and every task points at
    /// a list that exists (strays go to the Inbox). Old files load here.
    pub fn repair(&mut self) {
        if !self.lists.iter().any(|l| l.id == INBOX_ID) {
            self.lists.insert(0, TodoList::inbox());
        }
        let mut seen = Vec::with_capacity(self.lists.len());
        self.lists.retain(|l| {
            let fresh = !seen.contains(&l.id);
            seen.push(l.id);
            fresh
        });
        for l in &mut self.lists {
            if let Some(n) = clean_name(&l.name) {
                l.name = n;
            } else {
                l.name = if l.id == INBOX_ID { INBOX_NAME.into() } else { format!("{TOPIC_MARK}{}", l.id) };
            }
            if !ListColor::valid(&l.color) {
                l.color = DEFAULT_LIST_COLOR.into();
            }
        }
        for t in &mut self.todos {
            if !seen.contains(&t.list) {
                t.list = INBOX_ID;
            }
        }
    }

    fn next_list_id(&self) -> u64 {
        self.lists.iter().map(|l| l.id).max().unwrap_or(INBOX_ID) + 1
    }

    pub fn list(&self, id: u64) -> Option<&TodoList> {
        self.lists.iter().find(|l| l.id == id)
    }

    fn list_mut(&mut self, id: u64) -> Result<&mut TodoList, ListError> {
        self.lists.iter_mut().find(|l| l.id == id).ok_or(ListError::NoSuchList)
    }

    /// The list `key` names: its id, or its name (case-insensitive, also
    /// matching without spaces / dashes).
    pub fn find_list(&self, key: &str) -> Option<u64> {
        let key = key.trim().trim_start_matches(TOPIC_MARK);
        if let Ok(id) = key.parse::<u64>()
            && self.list(id).is_some()
        {
            return Some(id);
        }
        let lower = key.to_lowercase();
        if let Some(l) = self.lists.iter().find(|l| l.name.to_lowercase() == lower) {
            return Some(l.id);
        }
        let squashed = squash(key);
        (!squashed.is_empty()).then(|| self.lists.iter().find(|l| squash(&l.name) == squashed).map(|l| l.id)).flatten()
    }

    pub fn add_list(&mut self, name: &str, color: Option<&str>, icon: Option<&str>) -> Result<u64, ListError> {
        let name = clean_name(name).ok_or(ListError::EmptyName)?;
        let color = match color {
            Some(c) if !ListColor::valid(c) => return Err(ListError::BadColor),
            Some(c) => c.to_string(),
            // New lists take the next swatch, so lists tell apart at once.
            None => format!("hue:{}", SWATCH_HUES[self.lists.len() % SWATCH_HUES.len()]),
        };
        let id = self.next_list_id();
        let icon = icon.map(str::trim).filter(|i| !i.is_empty()).map(str::to_string);
        self.lists.push(TodoList { id, name, color, icon, show_done: false });
        Ok(id)
    }

    /// The list for a `#topic`, made (with the topic as its name) when no
    /// list matches — quick-add never loses a task to a typo'd topic.
    pub fn list_for_topic(&mut self, topic: &str) -> u64 {
        match self.find_list(topic) {
            Some(id) => id,
            None => self.add_list(topic, None, None).unwrap_or(INBOX_ID),
        }
    }

    pub fn rename_list(&mut self, id: u64, name: &str) -> Result<(), ListError> {
        let name = clean_name(name).ok_or(ListError::EmptyName)?;
        self.list_mut(id)?.name = name;
        Ok(())
    }

    pub fn recolor_list(&mut self, id: u64, color: &str) -> Result<(), ListError> {
        if !ListColor::valid(color) {
            return Err(ListError::BadColor);
        }
        self.list_mut(id)?.color = color.to_string();
        Ok(())
    }

    pub fn set_list_icon(&mut self, id: u64, icon: Option<&str>) -> Result<(), ListError> {
        self.list_mut(id)?.icon = icon.map(str::trim).filter(|i| !i.is_empty()).map(str::to_string);
        Ok(())
    }

    /// Moves list `id` to position `to` (clamped).
    pub fn move_list(&mut self, id: u64, to: usize) -> Result<usize, ListError> {
        let from = self.lists.iter().position(|l| l.id == id).ok_or(ListError::NoSuchList)?;
        let l = self.lists.remove(from);
        let to = to.min(self.lists.len());
        self.lists.insert(to, l);
        Ok(to)
    }

    /// Deletes a list; its tasks move to the Inbox or are deleted with it.
    /// Returns how many tasks were moved or deleted.
    pub fn remove_list(&mut self, id: u64, tasks: OnDelete) -> Result<usize, ListError> {
        if id == INBOX_ID {
            return Err(ListError::InboxStays);
        }
        let pos = self.lists.iter().position(|l| l.id == id).ok_or(ListError::NoSuchList)?;
        self.lists.remove(pos);
        let affected = self.todos.iter().filter(|t| t.list == id).count();
        match tasks {
            OnDelete::MoveToInbox => self.todos.iter_mut().filter(|t| t.list == id).for_each(|t| t.list = INBOX_ID),
            OnDelete::DeleteTasks => self.todos.retain(|t| t.list != id),
        }
        Ok(affected)
    }

    pub fn move_task(&mut self, task: u64, list: u64) -> Result<(), ListError> {
        if self.list(list).is_none() {
            return Err(ListError::NoSuchList);
        }
        self.todos.iter_mut().find(|t| t.id == task).ok_or(ListError::NoSuchTask)?.list = list;
        Ok(())
    }

    /// Adds a task (open, newest at the top of its list).
    pub fn add(&mut self, id: u64, text: &str, list: u64, created: i64) -> Option<u64> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let list = if self.list(list).is_some() { list } else { INBOX_ID };
        self.todos.insert(0, Todo { id, text: text.to_string(), done: false, created, list });
        Some(id)
    }

    /// `(open, done)` tasks of one list, or of every list.
    pub fn counts(&self, filter: Filter) -> (usize, usize) {
        self.todos.iter().filter(|t| filter.shows(t)).fold((0, 0), |(o, d), t| if t.done { (o, d + 1) } else { (o + 1, d) })
    }

    /// A list's tasks in `order`, open ones first.
    pub fn tasks(&self, list: u64, order: SortOrder) -> Vec<&Todo> {
        let mut v: Vec<&Todo> = self.todos.iter().filter(|t| t.list == list).collect();
        // Stable: equal keys keep the stored (newest-first) order.
        match order {
            SortOrder::Newest => v.sort_by_key(|t| (t.done, std::cmp::Reverse(t.created))),
            SortOrder::Oldest => v.sort_by_key(|t| (t.done, t.created)),
            SortOrder::Alphabetical => v.sort_by_cached_key(|t| (t.done, t.text.to_lowercase())),
        }
        v
    }

    /// The filter a command or setting names (`all`, empty, or a list).
    pub fn filter_for(&self, key: &str) -> Option<Filter> {
        let key = key.trim();
        if key.is_empty() || key.eq_ignore_ascii_case("all") {
            return Some(Filter::All);
        }
        self.find_list(key).map(Filter::List)
    }
}

impl Filter {
    pub fn shows(self, t: &Todo) -> bool {
        match self {
            Filter::All => true,
            Filter::List(id) => t.list == id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> TodoBook {
        TodoBook::new(Vec::new(), Vec::new())
    }

    #[test]
    fn old_files_load_with_every_task_in_the_inbox() {
        // `productivity.json` as written before lists existed.
        let old = r#"{
          "todos": [
            { "id": 3, "text": "Call the landlord", "done": false, "created": 1700000300 },
            { "id": 2, "text": "Buy milk", "done": true, "created": 1700000200 }
          ],
          "notes": [ { "id": 1, "text": "Groceries", "updated": 1700000100 } ]
        }"#;
        let saved: crate::productivity::Saved = serde_json::from_str(old).unwrap();
        let b = TodoBook::new(saved.lists, saved.todos.clone());
        assert_eq!(b.lists, vec![TodoList::inbox()]);
        assert_eq!(b.todos.len(), 2, "no task lost");
        assert!(b.todos.iter().all(|t| t.list == INBOX_ID));
        assert_eq!(b.todos.iter().map(|t| (t.id, t.text.as_str(), t.done)).collect::<Vec<_>>(), [(3, "Call the landlord", false), (2, "Buy milk", true)]);
        assert_eq!(saved.notes.len(), 1);
        // Strays (a list deleted by hand) also land in the Inbox.
        let mut todos = saved.todos;
        todos[0].list = 42;
        assert_eq!(TodoBook::new(vec![TodoList::inbox()], todos).todos[0].list, INBOX_ID);
    }

    #[test]
    fn list_crud_and_order() {
        let mut b = book();
        let work = b.add_list("  Work  ", Some("tertiary"), Some("folder")).unwrap();
        let home = b.add_list("Side project", None, None).unwrap();
        assert!(b.list(home).unwrap().color.starts_with("hue:"), "new lists get a swatch");
        assert_ne!(work, home);
        assert_eq!(b.list(work).unwrap().name, "Work");
        assert_eq!(b.add_list(" ", None, None), Err(ListError::EmptyName));
        assert_eq!(b.add_list("x", Some("mauve"), None), Err(ListError::BadColor));
        assert_eq!(b.find_list("work"), Some(work));
        assert_eq!(b.find_list("#side-project"), Some(home));
        assert_eq!(b.find_list("SideProject"), Some(home));
        assert_eq!(b.find_list(&home.to_string()), Some(home));
        assert_eq!(b.find_list("nope"), None);
        b.rename_list(INBOX_ID, "Later").unwrap();
        assert!(!b.list(INBOX_ID).unwrap().has_default_name());
        b.recolor_list(work, "hue:200").unwrap();
        assert_eq!(ListColor::parse(&b.list(work).unwrap().color), ListColor::Hue(200));
        assert_eq!(b.recolor_list(work, "hue:400"), Err(ListError::BadColor));
        b.set_list_icon(work, None).unwrap();
        assert_eq!(b.move_list(home, 0), Ok(0));
        assert_eq!(b.lists.iter().map(|l| l.id).collect::<Vec<_>>(), [home, INBOX_ID, work]);
        assert_eq!(b.move_list(home, 99), Ok(2));
        assert_eq!(b.remove_list(INBOX_ID, OnDelete::DeleteTasks), Err(ListError::InboxStays));
    }

    #[test]
    fn deleting_a_list_moves_or_deletes_its_tasks() {
        let mut b = book();
        let work = b.add_list("Work", None, None).unwrap();
        b.add(10, "a", work, 1);
        b.add(11, "b", work, 2);
        b.add(12, "c", INBOX_ID, 3);
        assert_eq!(b.remove_list(work, OnDelete::MoveToInbox), Ok(2));
        assert_eq!(b.counts(Filter::List(INBOX_ID)), (3, 0));
        let work = b.add_list("Work", None, None).unwrap();
        b.move_task(10, work).unwrap();
        b.move_task(11, work).unwrap();
        assert_eq!(b.move_task(10, 999), Err(ListError::NoSuchList));
        assert_eq!(b.move_task(999, work), Err(ListError::NoSuchTask));
        assert_eq!(b.remove_list(work, OnDelete::DeleteTasks), Ok(2));
        assert_eq!(b.todos.iter().map(|t| t.id).collect::<Vec<_>>(), [12]);
        assert_eq!(b.remove_list(work, OnDelete::DeleteTasks), Err(ListError::NoSuchList));
    }

    #[test]
    fn quick_add_topics() {
        assert_eq!(parse_quick_add("#Work call the bank"), (Some("Work"), "call the bank"));
        assert_eq!(parse_quick_add("  #side-project   ship it "), (Some("side-project"), "ship it"));
        assert_eq!(parse_quick_add("buy milk #later"), (None, "buy milk #later"));
        assert_eq!(parse_quick_add("# heading"), (None, "# heading"));
        assert_eq!(parse_quick_add("##tag text"), (None, "##tag text"));
        assert_eq!(parse_quick_add("#Work"), (Some("Work"), ""));
        // A missing topic is created; an existing one (any spelling) reused.
        let mut b = book();
        let work = b.list_for_topic("Work");
        assert_eq!(b.list(work).unwrap().name, "Work");
        assert_eq!(b.list_for_topic("work"), work);
        assert_eq!(b.list_for_topic("Inbox"), INBOX_ID);
        assert_eq!(b.lists.len(), 2);
    }

    #[test]
    fn counts_and_sorting() {
        let mut b = book();
        let w = b.add_list("W", None, None).unwrap();
        b.add(1, "banana", w, 10);
        b.add(2, "apple", w, 30);
        b.add(3, "cherry", w, 20);
        b.add(4, "inbox task", INBOX_ID, 5);
        assert_eq!(b.add(5, "   ", w, 1), None);
        b.todos.iter_mut().find(|t| t.id == 2).unwrap().done = true;
        assert_eq!(b.counts(Filter::List(w)), (2, 1));
        assert_eq!(b.counts(Filter::All), (3, 1));
        let ids = |o| b.tasks(w, o).iter().map(|t| t.id).collect::<Vec<_>>();
        assert_eq!(ids(SortOrder::Newest), [3, 1, 2]);
        assert_eq!(ids(SortOrder::Oldest), [1, 3, 2]);
        assert_eq!(ids(SortOrder::Alphabetical), [1, 3, 2]);
        assert_eq!(b.filter_for("all"), Some(Filter::All));
        assert_eq!(b.filter_for("w"), Some(Filter::List(w)));
        assert_eq!(b.filter_for("zz"), None);
    }
}
