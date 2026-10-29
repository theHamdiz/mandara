//! Calendar events for the clock panel and reminders (`[calendar]`).
//!
//! Sources (ICS subscriptions and local files / folders) are loaded off the
//! UI thread by [`mandara_services::calendar`], refreshed every
//! `refresh_minutes` and whenever the source list changes. A light ticker
//! posts a notification when an event's reminder time passes.

use std::time::Duration;

use chrono::{DateTime, Local};
use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use mandara_config::desktop::{CalendarConfig, CalendarSource};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PluginRegistry};
use mandara_services::calendar::{self, Event, Occurrence};
use serde_json::json;

use crate::state::{config, update_config};

/// Reminder times when the bar's custom time pattern is invalid.
const REMINDER_TIME_FALLBACK: &str = "%H:%M";
/// How often reminders are checked.
const REMINDER_TICK: Duration = Duration::from_secs(20);
/// Shortest refresh honoured, minutes.
const MIN_REFRESH_MIN: u32 = 5;
/// Seconds per minute.
const S_PER_MIN: u64 = 60;
/// Reminders whose time passed longer ago than this (asleep, shell
/// restarted) are not posted late.
const STALE_REMINDER: chrono::Duration = chrono::Duration::minutes(5);
/// Days scanned ahead for reminders (longest alarm lead honoured).
const REMINDER_HORIZON_DAYS: i64 = 8;
/// How often `calendar refresh` checks whether the reload finished.
const RELOAD_POLL: Duration = Duration::from_millis(100);
/// Reply channel capacity of pending commands.
const REPLY_QUEUE: usize = 1;

/// Loaded events, tagged with their source index.
#[derive(Default)]
pub struct CalendarStore {
    events: Vec<(usize, Event)>,
    /// Per-source load errors, for Studio / `calendar sources`.
    pub errors: Vec<(usize, String)>,
    /// Bumped on every reload (views cache occurrences against it).
    pub generation: u64,
    loaded: Option<Vec<CalendarSource>>,
    loading: bool,
    last_check: Option<DateTime<Local>>,
    /// The `refresh_minutes` the running refresh timer waits.
    refresh_every: u32,
    _refresh: Option<Task<()>>,
    _reminders: Option<Task<()>>,
}

struct GlobalCalendar(Entity<CalendarStore>);
impl Global for GlobalCalendar {}

/// The calendar store, if [`init`] ran (shell process).
pub fn store(cx: &App) -> Option<Entity<CalendarStore>> {
    cx.try_global::<GlobalCalendar>().map(|g| g.0.clone())
}

impl CalendarStore {
    /// Occurrences overlapping `[from, to)` of the enabled calendars.
    pub fn between(&self, from: DateTime<Local>, to: DateTime<Local>) -> Vec<Occurrence> {
        calendar::occurrences(&self.events, from, to)
    }

    pub fn has_sources(&self) -> bool {
        self.loaded.as_ref().is_some_and(|s| !s.is_empty())
    }

    /// Reloads every enabled source in the background.
    fn reload(&mut self, cfg: &CalendarConfig, cx: &mut Context<Self>) {
        let sources: Vec<CalendarSource> = if cfg.enabled { cfg.sources.iter().filter(|s| s.enabled).cloned().collect() } else { Vec::new() };
        self.loaded = Some(sources.clone());
        self.loading = true;
        let task = cx.background_executor().spawn(async move {
            let mut events = Vec::new();
            let mut errors = Vec::new();
            for (i, s) in sources.iter().enumerate() {
                // Offline mode: the last cached copy only, no request.
                let offline = calendar::is_remote(&s.location) && mandara_services::offline::is_offline();
                let text = if offline { calendar::load_cached(&s.location).ok_or_else(mandara_services::offline::blocked) } else { calendar::load(&s.location) };
                match text {
                    Ok(text) => events.extend(calendar::parse_ics(&text).into_iter().map(|e| (i, e))),
                    Err(err) => errors.push((i, err)),
                }
            }
            (events, errors)
        });
        cx.spawn(async move |this, cx| {
            let (events, errors) = task.await;
            let _ = this.update(cx, |this, cx| {
                for (i, err) in &errors {
                    tracing::warn!("calendar source {i}: {err}");
                }
                this.events = events;
                this.errors = errors;
                this.loading = false;
                this.generation += 1;
                cx.notify();
            });
        })
        .detach();
    }

    /// (Re)starts the periodic refresh, every `minutes` (at least
    /// [`MIN_REFRESH_MIN`]).
    fn start_refresh(&mut self, minutes: u32, cx: &mut Context<Self>) {
        self.refresh_every = minutes;
        let wait = Duration::from_secs(u64::from(minutes.max(MIN_REFRESH_MIN)) * S_PER_MIN);
        self._refresh = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(wait).await;
            let Ok(()) = this.update(cx, |this, cx| {
                let cfg = config(cx).read(cx).snapshot();
                this.reload(&cfg.calendar, cx);
            }) else {
                break;
            };
        }));
    }

    /// Posts the reminders that came due since the last check.
    fn check_reminders(&mut self, cx: &mut Context<Self>) {
        let cfg = config(cx).read(cx).snapshot();
        let now = Local::now();
        let after = self.last_check.unwrap_or(now).max(now - STALE_REMINDER);
        self.last_check = Some(now);
        if !cfg.calendar.enabled || !cfg.calendar.reminders || self.events.is_empty() {
            return;
        }
        let window = self.between(now - STALE_REMINDER, now + chrono::Duration::days(REMINDER_HORIZON_DAYS));
        let lead = Duration::from_secs(u64::from(cfg.calendar.reminder_minutes) * S_PER_MIN);
        for (occ, lead) in calendar::due_reminders(&window, after, now, lead, cfg.calendar.honour_event_alarms) {
            let minutes = lead.as_secs() / S_PER_MIN;
            let when = if minutes == 0 { t("Starting now").to_string() } else { tf("In {minutes} min", &[("minutes", &minutes)]) };
            // A bad custom pattern (`%Q`) would panic in `format`; this
            // falls back to 24-hour time and uses the date language.
            let time = crate::widgets::clock::format_with(&occ.start, &cfg.datetime.bar_time_pattern(cfg.general.clock_24h, false), REMINDER_TIME_FALLBACK);
            let body = if occ.location.is_empty() { format!("{when} · {time}") } else { format!("{when} · {time} · {}", occ.location) };
            crate::plugins::notifications::send(
                cx,
                mandara_services::NotificationCommand::Post { app: t("Calendar").into(), icon: "calendar".into(), summary: occ.summary.clone(), body },
            );
        }
    }
}

/// Starts the calendar store: loads sources, follows config changes,
/// refreshes periodically and posts reminders (shell process).
pub fn init(cx: &mut App) {
    let entity = cx.new(|cx: &mut Context<CalendarStore>| {
        let mut store = CalendarStore::default();
        let cfg = config(cx).read(cx).snapshot();
        store.reload(&cfg.calendar, cx);
        store.start_refresh(cfg.calendar.refresh_minutes, cx);
        store._reminders = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(REMINDER_TICK).await;
            if this.update(cx, |this, cx| this.check_reminders(cx)).is_err() {
                break;
            }
        }));
        store
    });
    // Source edits reload at once; a new refresh interval restarts the
    // timer (it used to wait out the old interval, up to 12 hours).
    let weak = entity.downgrade();
    cx.observe(&config(cx), move |store, cx| {
        let cfg = store.read(cx).snapshot();
        let Some(entity) = weak.upgrade() else { return };
        if entity.read(cx).refresh_every != cfg.calendar.refresh_minutes {
            entity.update(cx, |s, cx| s.start_refresh(cfg.calendar.refresh_minutes, cx));
        }
        let wanted: Vec<CalendarSource> = if cfg.calendar.enabled { cfg.calendar.sources.iter().filter(|s| s.enabled).cloned().collect() } else { Vec::new() };
        if entity.read(cx).loaded.as_ref() != Some(&wanted) {
            entity.update(cx, |s, cx| s.reload(&cfg.calendar, cx));
        }
    })
    .detach();
    cx.set_global(GlobalCalendar(entity));
}

fn occurrence_list(days: i64, cx: &App) -> Result<serde_json::Value, String> {
    let store = store(cx).ok_or("the calendar isn't running")?;
    let now = Local::now();
    let list: Vec<serde_json::Value> = store.read(cx).between(now, now + chrono::Duration::days(days)).iter().map(Occurrence::to_json).collect();
    Ok(json!(list))
}

/// Source names for display: the configured name, else the file / host.
pub fn source_label(s: &CalendarSource) -> String {
    if !s.name.trim().is_empty() {
        return s.name.clone();
    }
    let loc = s.location.trim().trim_end_matches('/');
    loc.rsplit('/').next().unwrap_or(loc).to_string()
}

pub fn register(r: &mut PluginRegistry) {
    let owner = crate::plugins::clock::ID;
    r.command(
        owner,
        CommandSpec::new("calendar", tk("Upcoming calendar events as JSON"), |a, cx| {
            let days = a.get(0).map(str::parse).transpose().map_err(|_| t("Days must be a whole number").to_string())?.unwrap_or_else(|| i64::from(config(cx).read(cx).get().calendar.agenda_days));
            occurrence_list(days, cx)
        })
        .arg(ArgSpec::optional("days", ArgKind::Integer, tk("How many days ahead (default: the agenda length)")))
        .example("calendar")
        .example("calendar 1"),
    );
    r.command(
        owner,
        CommandSpec::new("calendar sources", tk("Configured calendars and their load errors"), |_, cx| {
            let cfg = config(cx).read(cx).snapshot();
            let errors = store(cx).map(|s| s.read(cx).errors.clone()).unwrap_or_default();
            let enabled: Vec<&CalendarSource> = cfg.calendar.sources.iter().filter(|s| s.enabled).collect();
            Ok(json!(cfg
                .calendar
                .sources
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let loaded_index = enabled.iter().position(|e| std::ptr::eq(*e, s));
                    let error = loaded_index.and_then(|li| errors.iter().find(|(e, _)| *e == li).map(|(_, m)| m.clone()));
                    json!({ "index": i, "name": source_label(s), "location": s.location, "enabled": s.enabled, "color": s.color, "error": error })
                })
                .collect::<Vec<_>>()))
        })
        .example("calendar sources"),
    );
    r.command(
        owner,
        CommandSpec::new("calendar add", tk("Subscribe to an ICS calendar (https / webcal URL, .ics file or folder)"), |a, cx| {
            let location = a.str(0)?.to_string();
            let name = a.rest(1);
            let entry = CalendarSource { name, location, color: String::new(), enabled: true };
            let label = source_label(&entry);
            update_config(cx, move |c| {
                if !c.calendar.sources.iter().any(|s| s.location == entry.location) {
                    c.calendar.sources.push(entry);
                }
            });
            Ok(json!({ "added": label }))
        })
        .arg(ArgSpec::required("location", ArgKind::String, tk("URL or path")))
        .arg(ArgSpec::optional("name", ArgKind::String, tk("Display name")))
        .example("calendar add webcal://example.com/holidays.ics Holidays")
        .example("calendar add ~/Documents/work.ics Work"),
    );
    r.command(
        owner,
        CommandSpec::new("calendar remove", tk("Remove a calendar by index or name"), |a, cx| {
            let key = a.rest(0);
            let sources = config(cx).read(cx).get().calendar.sources.clone();
            let index = key.parse::<usize>().ok().filter(|i| *i < sources.len()).or_else(|| sources.iter().position(|s| source_label(s).eq_ignore_ascii_case(&key) || s.location == key));
            let index = index.ok_or_else(|| tf("No calendar “{name}”", &[("name", &key)]))?;
            update_config(cx, move |c| {
                if index < c.calendar.sources.len() {
                    c.calendar.sources.remove(index);
                }
            });
            Ok(json!({ "removed": index }))
        })
        .arg(ArgSpec::required("calendar", ArgKind::String, tk("Index (see calendar sources) or name")))
        .example("calendar remove 0"),
    );
    r.command(
        owner,
        CommandSpec::new_output("calendar refresh", tk("Fetch every calendar again now"), |_, cx| {
            let store = store(cx).ok_or("the calendar isn't running")?;
            let cfg = config(cx).read(cx).snapshot();
            let before = store.read(cx).generation;
            store.update(cx, |s, cx| s.reload(&cfg.calendar, cx));
            let (tx, rx) = async_channel::bounded(REPLY_QUEUE);
            cx.spawn(async move |cx| {
                // Reply once the reload lands.
                loop {
                    cx.background_executor().timer(RELOAD_POLL).await;
                    let done = cx.update(|cx| store.read(cx).generation != before);
                    if done {
                        let n = cx.update(|cx| store.read(cx).events.len());
                        let _ = tx.send(Ok(json!({ "events": n }).to_string())).await;
                        break;
                    }
                }
            })
            .detach();
            Ok(Output::Pending(rx))
        })
        .example("calendar refresh"),
    );
}
