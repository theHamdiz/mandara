//! Jannah: a constant, gentle reminder of Paradise as the final home: what
//! it is, how to get there, what to do and what to avoid. A reminder from
//! the Qur'an and the hadith, a fresh one at random each time the panel or
//! the sidebar opens (never the one just shown or one shown lately, and
//! the same on every surface open at once): a slim arch glyph in the bar
//! (optionally with today's step), a panel (the reminder under an arch
//! with its meaning and reference, today's step and this week's dots, and
//! Paths, Qur'an, Hadith and Saved), a sidebar card and desktop widgets
//! (`verse`, `quote`, `step`), which keep theirs for a day (or an hour).
//! Opt-in.
//!
//! Content comes from `mandara_services::jannah` (bundled verbatim; Qur'an
//! meanings are fetched once per language from api.alquran.cloud and
//! cached, never while offline). The picking rules live in
//! [`mandara_services::jannah::recent`]; the shell only tells them when a
//! surface opens. Nothing ticks while idle: one timer waits for the
//! widgets' next period (or the morning reminder), and the bar's glint
//! animates only for the moment the item changes. Saved items and done
//! days live in `~/.local/state/mandara/jannah.json`, what was shown
//! lately in `jannah-recent.json` beside it; both are read and written off
//! the UI thread (the latter coalesced).
//!
//! Settings (`[plugins.settings.jannah]`): `rotate` (`daily` / `hourly`),
//! `sources` (`both` / `quran` / `hadith`), `show_translation`,
//! `arabic_size` (`small` / `medium` / `large`), `bar_label` (`glyph` /
//! `step`), `morning_reminder`.
//!
//! Commands: `jannah today|next|prev|peek [n]|show <ref>|search
//! <query>|paths|path <key>|step|step done|step peek [days]|saved|save
//! <ref>|unsave <ref>`.

pub mod art;
pub(crate) mod panel;
mod share;

#[cfg(test)]
pub(super) use share::longest_export_document;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDate, NaiveDateTime, Timelike as _};
use gpui::{AnyElement, App, AppContext as _, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Task, Window, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui;
use mandara_i18n::{Lang, t, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, DesktopWidgetType, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::jannah::catalog::{self, Item, ItemId, catalog};
use mandara_services::jannah::meanings::{self, Meaning, Meanings};
use mandara_services::jannah::paths::Path;
use mandara_services::jannah::recent::{self, Memory, Pool, Reminders};
use mandara_services::jannah::rotation::{self, Rotate, Sources};
use mandara_services::jannah::search::{self, Entry};
use mandara_services::jannah::state::{self, State};
use mandara_theme::{Color, MandaraPalette};
use serde_json::{Value, json};

use super::panel_fx::{ViewSet, readable};
use super::{fn_view, plugin_lang};
use crate::bidi::Bidi as _;
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::{card, text, tokens};
use crate::state::config;

use super::settings_cache::{ConfigSettings, SettingsCache};
mod progress;
use progress::{DayProgress, Progress, ProgressCache};

pub const ID: &str = "jannah";

// ── Sizes & timing ───────────────────────────────────────────────────────

/// Longest Arabic (characters) a desktop widget shows: it makes its own
/// pick from the items that fit (the sidebar folds the panel's instead).
const WIDGET_LIMIT: usize = 320;
/// Bar chip: glyph size, gap, and the step title's widest run.
const BAR_ICON_PX: f32 = 14.0;
const BAR_GAP_PX: f32 = 6.0;
const BAR_STEP_MAX_PX: f32 = 180.0;
/// The bar's gold glint: how long it sweeps, the band's width and its
/// strongest alpha.
const GLINT: Duration = Duration::from_millis(1100);
const GLINT_BAND_PX: f32 = 6.0;
const GLINT_ALPHA: f32 = 0.85;
/// Sidebar card: glyph, Arabic size, clamped lines, the check.
const SIDEBAR_ICON_PX: f32 = 15.0;
const SIDEBAR_ARABIC_PX: f32 = 19.0;
const SIDEBAR_LINES: usize = 3;
const SIDEBAR_CHECK_PX: f32 = 26.0;
/// Arabic line height (em): room for tashkeel above and below.
pub(crate) const ARABIC_LINE_EM: f32 = 1.95;
/// Ticks land this far past the boundary they wait for.
const TICK_MARGIN: Duration = Duration::from_millis(250);
/// What was shown is written this long after the last pick (a burst of
/// opens writes once).
const MEMORY_SAVE_DELAY: Duration = Duration::from_millis(1500);
/// The morning reminder goes out from this hour until noon.
const MORNING_HOUR: u32 = 7;
const NOON: u32 = 12;
/// Results a `jannah search` returns at most.
const SEARCH_MAX: usize = 50;

// ── Settings ─────────────────────────────────────────────────────────────

/// How large the Arabic is set.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ArabicSize {
    Small,
    Medium,
    Large,
}

impl ArabicSize {
    /// The hero's Arabic size, px.
    pub(crate) fn hero_px(self) -> f32 {
        match self {
            ArabicSize::Small => 21.0,
            ArabicSize::Medium => 25.0,
            ArabicSize::Large => 30.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    pub rotate: Rotate,
    pub sources: Sources,
    pub translation: bool,
    pub arabic_size: ArabicSize,
    pub bar_step: bool,
    pub morning: bool,
    /// The language Jannah speaks: its own, else Mandara's.
    pub lang: Lang,
}

thread_local! {
    static SETTINGS: RefCell<SettingsCache<Settings>> = const { RefCell::new(SettingsCache::new()) };
}

/// Release only resolved configuration when the plugin is switched off.
pub(super) fn clear_settings() {
    SETTINGS.with(|cache| cache.borrow_mut().clear());
}

pub(crate) fn settings(cx: &App) -> Rc<Settings> {
    let config = config(cx).read(cx).snapshot();
    let language = plugin_lang::resolved(plugin_lang::own_language(config.plugins.setting_str(ID, "language").unwrap_or(plugin_lang::FOLLOW)));
    SETTINGS.with(|cache| cache.borrow_mut().get(&config, language, ()))
}

impl ConfigSettings for Settings {
    type Scope = ();

    fn resolve(cfg: &mandara_config::ShellConfig, language: Lang, (): ()) -> Self {
        let s = &cfg.plugins;
        Settings {
            rotate: Rotate::parse(s.setting_str(ID, "rotate").unwrap_or("daily")),
            sources: Sources::parse(s.setting_str(ID, "sources").unwrap_or("both")),
            translation: s.setting(ID, "show_translation", true),
            arabic_size: match s.setting_str(ID, "arabic_size").unwrap_or("medium") {
                "small" => ArabicSize::Small,
                "large" => ArabicSize::Large,
                _ => ArabicSize::Medium,
            },
            bar_step: s.setting_str(ID, "bar_label").unwrap_or("glyph") == "step",
            morning: s.setting(ID, "morning_reminder", false),
            lang: language,
        }
    }
}

/// Jannah's own language (`None`: Mandara's), from its `language` setting.
pub(crate) fn own_lang(cx: &App) -> Option<Lang> {
    let config = config(cx).read(cx).snapshot();
    plugin_lang::own_language(config.plugins.setting_str(ID, "language").unwrap_or(plugin_lang::FOLLOW))
}

/// Whether the interface is Arabic (scripture only, no translation).
pub(crate) fn arabic_ui() -> bool {
    mandara_i18n::language() == Lang::Ar
}

/// Whether meanings show under the Arabic (never in Arabic).
pub(crate) fn wants_meaning(s: &Settings) -> bool {
    s.translation && s.lang != Lang::Ar
}

// ── The shared model ─────────────────────────────────────────────────────

/// Where the Qur'an meanings of the interface language stand.
#[derive(Clone)]
pub(crate) enum Fetch {
    Idle,
    Loading,
    Ready(Arc<Meanings>),
    Failed(String),
    /// Offline mode is on: Arabic only.
    Offline,
}

pub(crate) struct Model {
    pub state: State,
    /// Bumped per change; the newest write wins.
    generation: u64,
    progress: RefCell<ProgressCache>,
    pub edition: Option<&'static str>,
    pub fetch: Fetch,
    /// Every view showing Jannah (bar chips, sidebar cards, panels).
    pub views: ViewSet,
    /// The pending wake-up and when it is due.
    timer: Option<(NaiveDateTime, Task<()>)>,
    /// The search index for `(language, meanings loaded)`.
    pub index: Option<((Lang, bool), Arc<Vec<Entry>>)>,
    /// An item a command asked the panel to open in the reader.
    pub open_request: Option<ItemId>,
    /// What the panel and sidebar show, what was shown lately and the
    /// desktop widgets' pick.
    reminders: Reminders,
    /// The remembered picks were read from disk (until then nothing is
    /// written, and the widgets wait).
    memory_loaded: bool,
    /// A pick happened before they were read: write once they are.
    memory_dirty: bool,
    /// The pending coalesced write of what was shown.
    memory_save: Option<Task<()>>,
    /// Bumped per pick; the newest write wins.
    memory_generation: u64,
}

impl Model {
    fn new(state: State) -> Self {
        Model {
            state,
            generation: 0,
            progress: RefCell::default(),
            edition: None,
            fetch: Fetch::Idle,
            views: ViewSet::default(),
            timer: None,
            index: None,
            open_request: None,
            reminders: Reminders::default(),
            memory_loaded: false,
            memory_dirty: false,
            memory_save: None,
            memory_generation: 0,
        }
    }

    fn day_progress(&self, date: NaiveDate) -> DayProgress {
        self.progress.borrow_mut().day(&self.state, self.generation, date)
    }

    fn progress(&self, date: NaiveDate, week_start: u32) -> Progress {
        self.progress.borrow_mut().get(&self.state, self.generation, date, week_start)
    }
}

pub(crate) type Shared = Rc<RefCell<Model>>;

struct GlobalJannah(Shared);

impl Global for GlobalJannah {}

/// The model, loaded on first use (the state file is a few KB).
pub(crate) fn model(cx: &mut App) -> Shared {
    if let Some(g) = cx.try_global::<GlobalJannah>() {
        return g.0.clone();
    }
    let shared: Shared = Rc::new(RefCell::new(Model::new(State::load(&state::path()))));
    cx.set_global(GlobalJannah(shared.clone()));
    load_memory(&shared, cx);
    follow_surfaces(&shared, cx);
    ensure_timer(&shared, cx);
    shared
}

/// Reads what was shown lately off the UI thread (building the catalog
/// there too), then lets the widgets pick and the writes go out.
fn load_memory(shared: &Shared, cx: &mut App) {
    let task = cx.background_spawn(async move { Memory::load(&recent::path(), catalog()) });
    let weak = Rc::downgrade(shared);
    cx.spawn(async move |cx| {
        let loaded = task.await;
        cx.update(|cx| {
            let Some(shared) = weak.upgrade() else { return };
            let dirty = {
                let mut m = shared.borrow_mut();
                m.reminders.merge(loaded);
                m.memory_loaded = true;
                std::mem::take(&mut m.memory_dirty)
            };
            if dirty {
                remember(&shared, cx);
            }
            changed(&shared, cx);
        });
    })
    .detach();
}

/// Tells the picking rules when the panel or the sidebar opens, so the
/// next look draws a fresh reminder (the sidebar's card follows the
/// panel's while both are open). The shell's state is installed before
/// any plugin runs (`init_state`, in the shell and in Studio), and the
/// model is built only once, so there is no later chance to register.
fn follow_surfaces(shared: &Shared, cx: &mut App) {
    let state = crate::state::shell(cx);
    let weak = Rc::downgrade(shared);
    cx.observe(&state, move |state, cx| {
        let Some(shared) = weak.upgrade() else { return };
        let (panel, sidebar) = {
            let s = state.read(cx);
            (s.panel.as_deref() == Some(ID), s.sidebar_visible())
        };
        let opened = shared.borrow_mut().reminders.surfaces(panel, sidebar, Instant::now());
        if opened {
            let views = shared.borrow().views.clone();
            views.notify(cx);
        }
    })
    .detach();
}

/// Writes what was shown lately, once a burst of picks settles, off the UI
/// thread (the newest write wins).
fn remember(shared: &Shared, cx: &mut App) {
    {
        let mut m = shared.borrow_mut();
        m.memory_generation += 1;
        if !m.memory_loaded {
            m.memory_dirty = true;
            return;
        }
        if m.memory_save.is_some() {
            return;
        }
    }
    static WRITTEN: std::sync::Mutex<u64> = std::sync::Mutex::new(0);
    let weak = Rc::downgrade(shared);
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(MEMORY_SAVE_DELAY).await;
        cx.update(|cx| {
            let Some(shared) = weak.upgrade() else { return };
            let (memory, generation) = {
                let mut m = shared.borrow_mut();
                m.memory_save = None;
                (m.reminders.memory.clone(), m.memory_generation)
            };
            cx.background_spawn(async move {
                let mut last = WRITTEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if generation > *last {
                    if let Err(e) = memory.save(&recent::path(), catalog()) {
                        tracing::warn!("jannah: saving what was shown: {e}");
                    }
                    *last = generation;
                }
            })
            .detach();
        });
    });
    shared.borrow_mut().memory_save = Some(task);
}

/// Writes the state off the UI thread (the newest change wins).
fn save(shared: &Shared, cx: &mut App) {
    static WRITTEN: std::sync::Mutex<u64> = std::sync::Mutex::new(0);
    let (state, generation) = {
        let mut m = shared.borrow_mut();
        m.generation += 1;
        (m.state.clone(), m.generation)
    };
    cx.background_spawn(async move {
        let mut last = WRITTEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if generation > *last {
            if let Err(e) = state.save(&state::path()) {
                tracing::warn!("jannah: saving state: {e}");
            }
            *last = generation;
        }
    })
    .detach();
}

/// After a change: every view and the desktop widgets redraw.
fn changed(shared: &Shared, cx: &mut App) {
    let views = shared.borrow().views.clone();
    views.notify(cx);
    crate::backdrop::refresh(cx);
}

// ── Today ────────────────────────────────────────────────────────────────

/// The items a surface showing at most `limit` characters picks from.
fn pool(s: &Settings, limit: Option<usize>) -> Pool<'static> {
    Pool::new(catalog(), s.sources, limit)
}

/// After a pick: write it down and redraw every surface (after this
/// render: the one drawing may be one of them).
fn picked(shared: &Shared, cx: &mut App) {
    remember(shared, cx);
    let views = shared.borrow().views.clone();
    cx.defer(move |cx| views.notify(cx));
}

/// The reminder the panel and sidebar show: the current one, or a fresh
/// one at random when a surface just opened (or none was shown yet).
pub(crate) fn current(shared: &Shared, s: &Settings, cx: &mut App) -> Option<ItemId> {
    let (id, new) = shared.borrow_mut().reminders.current(&pool(s, None), &mut recent::os_draw);
    if new {
        picked(shared, cx);
    }
    id
}

/// `item`'s fixed number in the pool the reminders are drawn from and the
/// pool's size (`(104, 230)`).
pub(crate) fn number(s: &Settings, item: ItemId) -> Option<(usize, usize)> {
    pool(s, None).number(item)
}

/// The reminder on screen, without drawing one (the bar's glint).
fn shown(shared: &Shared) -> Option<ItemId> {
    shared.borrow().reminders.shown()
}

/// Whether Previous has a reminder to go back to.
pub(crate) fn has_prev(shared: &Shared) -> bool {
    shared.borrow().reminders.has_prev()
}

/// Next on every surface: forward again over what Previous stepped back
/// through, then a fresh reminder.
pub(crate) fn next(shared: &Shared, cx: &mut App) {
    let s = settings(cx);
    shared.borrow_mut().reminders.next(&pool(&s, None), &mut recent::os_draw);
    remember(shared, cx);
    changed(shared, cx);
}

/// Previous on every surface: the reminder shown before; whether there was
/// one.
pub(crate) fn prev(shared: &Shared, cx: &mut App) -> bool {
    let moved = shared.borrow_mut().reminders.prev().is_some();
    if moved {
        changed(shared, cx);
    }
    moved
}

/// A look only: the reminder shown `-by` before this one, or (`by > 0`)
/// another the next pick could bring.
fn look(shared: &Shared, s: &Settings, by: i64) -> Option<ItemId> {
    shared.borrow().reminders.look(&pool(s, None), by, &mut recent::os_draw)
}

/// The desktop widgets' reminder for this period (`None` until what was
/// shown lately is read).
fn widget_item(shared: &Shared, s: &Settings, now: NaiveDateTime, cx: &mut App) -> Option<ItemId> {
    if !shared.borrow().memory_loaded {
        return None;
    }
    let (id, new) = shared.borrow_mut().reminders.widget(&pool(s, Some(WIDGET_LIMIT)), rotation::period(now, s.rotate), &mut recent::os_draw);
    if new {
        remember(shared, cx);
    }
    id
}

/// Today's step.
pub(crate) fn today_step() -> Option<&'static Path> {
    step_in(0)
}

/// The step of the day `days` after today (before it when negative).
pub(crate) fn step_in(days: i64) -> Option<&'static Path> {
    rotation::step(Local::now().date_naive() + chrono::Duration::days(days))
}

/// Marks today's step done (a no-op when it already is); whether it was
/// newly marked.
pub fn mark_step_done(cx: &mut App) -> bool {
    let shared = model(cx);
    let added = shared.borrow_mut().state.mark_done(Local::now().date_naive());
    if added {
        save(&shared, cx);
        changed(&shared, cx);
    }
    added
}

/// Saves or removes an item; whether it is saved now.
pub(crate) fn toggle_saved(shared: &Shared, key: &str, cx: &mut App) -> bool {
    let saved = shared.borrow_mut().state.toggle_saved(key, Local::now().timestamp());
    save(shared, cx);
    changed(shared, cx);
    saved
}

// ── Meanings ─────────────────────────────────────────────────────────────

/// Starts loading the Qur'an meanings of the interface language when they
/// are wanted and not there yet (the cache, else the network unless
/// offline), off the UI thread.
pub(crate) fn ensure_meanings(shared: &Shared, cx: &mut App) {
    let s = settings(cx);
    if !wants_meaning(&s) {
        return;
    }
    let Some(e) = meanings::edition(s.lang.code()) else { return };
    {
        let mut m = shared.borrow_mut();
        if m.edition != Some(e.id) {
            m.edition = Some(e.id);
            m.fetch = Fetch::Idle;
            m.index = None;
        }
        // Offline mode switched off since: try again.
        if matches!(m.fetch, Fetch::Offline) && !mandara_services::offline::is_offline() {
            m.fetch = Fetch::Idle;
        }
        if !matches!(m.fetch, Fetch::Idle) {
            return;
        }
        m.fetch = Fetch::Loading;
    }
    let id = e.id;
    let task = cx.background_spawn(async move {
        if let Some(cached) = meanings::load_cached(id) {
            return Ok(cached);
        }
        if mandara_services::offline::is_offline() {
            return Err(None);
        }
        meanings::fetch(id).map_err(Some)
    });
    let shared = shared.clone();
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            {
                let mut m = shared.borrow_mut();
                // The language may have changed meanwhile.
                if m.edition != Some(id) {
                    return;
                }
                m.fetch = match result {
                    Ok(map) => Fetch::Ready(Arc::new(map)),
                    Err(None) => Fetch::Offline,
                    Err(Some(why)) => {
                        tracing::warn!("jannah: meanings {id}: {why}");
                        Fetch::Failed(why)
                    }
                };
                m.index = None;
            }
            changed(&shared, cx);
        });
    })
    .detach();
}

/// Lets a failed fetch try again.
pub(crate) fn retry_meanings(shared: &Shared, cx: &mut App) {
    shared.borrow_mut().fetch = Fetch::Idle;
    ensure_meanings(shared, cx);
    changed(shared, cx);
}

/// An item's meaning in the interface language (Qur'an: when fetched;
/// hadith: the dataset or an AI rendering, else the English with its
/// language named).
pub(crate) fn meaning_of(m: &Model, item: &Item) -> Option<Meaning> {
    let lang = mandara_i18n::language();
    if lang == Lang::Ar {
        return None;
    }
    let ready = match &m.fetch {
        Fetch::Ready(map) => Some(map.as_ref()),
        _ => None,
    };
    meanings::meaning(item, lang.code(), ready).or_else(|| {
        let h = item.hadith()?;
        let (text, _) = meanings::hadith_meaning(h, Lang::En.code())?;
        Some(Meaning { text: text.to_string(), credit: t("English").to_string(), ai: false })
    })
}

/// The search index for the interface language, built off the UI thread
/// when missing; `None` until it is ready (the caller is notified).
pub(crate) fn search_index(shared: &Shared, cx: &mut App) -> Option<Arc<Vec<Entry>>> {
    let lang = mandara_i18n::language();
    let (key, meanings) = {
        let m = shared.borrow();
        let meanings = match &m.fetch {
            Fetch::Ready(map) => Some(map.clone()),
            _ => None,
        };
        let key = (lang, meanings.is_some());
        if let Some((k, idx)) = &m.index
            && *k == key
        {
            return Some(idx.clone());
        }
        (key, meanings)
    };
    static BUILDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if BUILDING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return None;
    }
    let task = cx.background_spawn(async move { build_index(lang, meanings.as_deref()) });
    let shared = shared.clone();
    cx.spawn(async move |cx| {
        let idx = Arc::new(task.await);
        BUILDING.store(false, std::sync::atomic::Ordering::Release);
        cx.update(|cx| {
            shared.borrow_mut().index = Some((key, idx));
            let views = shared.borrow().views.clone();
            views.notify(cx);
        });
    })
    .detach();
    None
}

/// Every item, folded for search in `lang`.
fn build_index(lang: Lang, map: Option<&Meanings>) -> Vec<Entry> {
    let c = catalog();
    let ids: Vec<ItemId> = c.items.iter().map(|i| i.id).collect();
    let arabic = lang == Lang::Ar;
    search::index(
        c,
        &ids,
        |id| {
            let item = c.get(id)?;
            if arabic {
                return None;
            }
            meanings::meaning(item, lang.code(), map).map(|m| m.text)
        },
        |id| c.get(id).map(|i| format!("{} {} {}", catalog::reference(i, false), catalog::reference(i, true), i.key)).unwrap_or_default(),
    )
}

// ── Timer ────────────────────────────────────────────────────────────────

/// When the next wake-up is due: the next rotation, or the morning
/// reminder's hour when it is on and still to come.
fn next_due(m: &Model, s: &Settings, now: NaiveDateTime) -> NaiveDateTime {
    let boundary = rotation::next_boundary(now, s.rotate);
    if !s.morning {
        return boundary;
    }
    let today = now.date();
    let morning = |d: NaiveDate| d.and_hms_opt(MORNING_HOUR, 0, 0).unwrap_or(now);
    let next_morning = if now.hour() < MORNING_HOUR { morning(today) } else { morning(today + chrono::Duration::days(1)) };
    // Already inside the morning window and not reminded: now (soon).
    if (MORNING_HOUR..NOON).contains(&now.hour()) && !m.day_progress(today).reminded {
        return now;
    }
    boundary.min(next_morning)
}

/// Keeps exactly one wake-up scheduled, for the right moment (settings may
/// have changed). Nothing runs while the plugin is off.
pub(crate) fn ensure_timer(shared: &Shared, cx: &mut App) {
    let cfg = config(cx).read(cx).snapshot();
    if !crate::desktop_plugins::enabled(ID, &cfg, cx) {
        shared.borrow_mut().timer = None;
        return;
    }
    let s = settings(cx);
    let now = Local::now().naive_local();
    let due = next_due(&shared.borrow(), &s, now);
    if shared.borrow().timer.as_ref().is_some_and(|(d, _)| *d == due) {
        return;
    }
    let wait = (due - now).to_std().unwrap_or(Duration::ZERO) + TICK_MARGIN;
    let weak = Rc::downgrade(shared);
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(wait).await;
        cx.update(|cx| {
            let Some(shared) = weak.upgrade() else { return };
            shared.borrow_mut().timer = None;
            remind(&shared, cx);
            changed(&shared, cx);
            ensure_timer(&shared, cx);
        });
    });
    shared.borrow_mut().timer = Some((due, task));
}

/// Sends the morning reminder once a morning, when it is on.
fn remind(shared: &Shared, cx: &mut App) {
    let s = settings(cx);
    let _lang = mandara_i18n::scoped(Some(s.lang));
    let now = Local::now().naive_local();
    if !s.morning || !(MORNING_HOUR..NOON).contains(&now.hour()) || shared.borrow().day_progress(now.date()).reminded {
        return;
    }
    // A fresh reminder, which the panel and sidebar then show (and won't
    // bring back soon).
    let id = shared.borrow_mut().reminders.next(&pool(&s, None), &mut recent::os_draw);
    remember(shared, cx);
    let Some(item) = id.and_then(|id| catalog().get(id)) else { return };
    let arabic = arabic_ui();
    let body = match meaning_of(&shared.borrow(), item).filter(|_| wants_meaning(&s)) {
        Some(m) => format!("{}\n— {}", meanings::excerpt(&m.text), catalog::reference(item, arabic)),
        None => format!("{}\n— {}", item.arabic, catalog::reference(item, arabic)),
    };
    super::notifications::send(
        cx,
        mandara_services::NotificationCommand::Post { app: t("Jannah").into(), icon: "arch".into(), summary: t("Today's reminder").into(), body },
    );
    shared.borrow_mut().state.set_reminded(now.date());
    save(shared, cx);
}

// ── Desktop widgets ──────────────────────────────────────────────────────

/// What the desktop widgets show this period.
pub fn widget_info(cx: &mut App) -> Option<crate::backdrop::widgets::JannahInfo> {
    let shared = model(cx);
    ensure_timer(&shared, cx);
    let s = settings(cx);
    let own = own_lang(cx);
    let _lang = mandara_i18n::scoped(own);
    if wants_meaning(&s) {
        ensure_meanings(&shared, cx);
    }
    let item = catalog().get(widget_item(&shared, &s, Local::now().naive_local(), cx)?)?;
    let m = shared.borrow();
    let meaning = meaning_of(&m, item).filter(|_| wants_meaning(&s));
    let date = Local::now().date_naive();
    let progress = m.progress(date, crate::plugins::clock::week_start_days(cx));
    let week = progress.week;
    let step = today_step();
    Some(crate::backdrop::widgets::JannahInfo {
        arabic: item.arabic.to_string(),
        ayah_mark: item.verse_ref().map(|(_, a)| catalog::ayah_mark(a)),
        ai: meaning.as_ref().is_some_and(|m| m.ai),
        meaning: meaning.map(|m| m.text),
        reference: catalog::reference(item, false),
        reference_ar: catalog::reference(item, true),
        open: item.key.to_string(),
        step_title: step.map(|p| p.title.to_string()).unwrap_or_default(),
        step_line: step.map(|p| p.line.to_string()).unwrap_or_default(),
        step_done: progress.done,
        week: week.map(|(_, done)| done),
        today: week.iter().position(|(d, _)| *d == date).unwrap_or_default(),
        lang: own,
    })
}

// ── Bar chip ─────────────────────────────────────────────────────────────

/// The bar glint: the item last shown and when the sweep began.
#[derive(Default)]
struct Glint {
    last: Option<ItemId>,
    since: Option<Instant>,
}

/// The gold accent: the theme's warm amber, readable on the surface.
pub(crate) fn gold(palette: &MandaraPalette) -> Color {
    readable(palette.warning_amber, palette)
}

fn bar_view(glint: Rc<RefCell<Glint>>) -> impl Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement {
    move |palette, window, cx| {
        let shared = model(cx);
        shared.borrow().views.track(window);
        ensure_timer(&shared, cx);
        let s = settings(cx);
        let current = shown(&shared);
        let size = crate::style::bar_icon(BAR_ICON_PX);
        let mut g = glint.borrow_mut();
        if g.last.is_some() && g.last != current && !mandara_core::spring::motion_profile().reduce {
            g.since = Some(Instant::now());
        }
        g.last = current;
        let sweep = g.since.map(|t| t.elapsed().as_secs_f32() / GLINT.as_secs_f32()).filter(|p| *p < 1.0);
        if sweep.is_none() {
            g.since = None;
        }
        drop(g);
        let ink = palette.on_surface_variant;
        let mut glyph = div().relative().size(px(size)).flex_none().child(icon("arch", size, ink));
        if let Some(p) = sweep {
            window.request_animation_frame();
            // A narrow window of gold arch slides across, brightest midway.
            let band = GLINT_BAND_PX;
            let x = -band + (size + band) * p;
            let alpha = (std::f32::consts::PI * p).sin() * GLINT_ALPHA;
            glyph = glyph.child(
                div()
                    .absolute()
                    .top_0()
                    .left(px(x))
                    .w(px(band))
                    .h(px(size))
                    .overflow_hidden()
                    .child(div().absolute().top_0().left(px(-x)).child(icon("arch", size, gold(palette).with_alpha(alpha)))),
            );
        }
        let step = s.bar_step.then(today_step).flatten();
        let done = shared.borrow().day_progress(Local::now().date_naive()).done;
        div()
            .flex()
            .items_center()
            .gap(px(BAR_GAP_PX))
            .font_features(tabular_nums())
            .child(glyph)
            .when_some(step, |el, p| {
                el.child(
                    div()
                        .max_w(px(BAR_STEP_MAX_PX))
                        .truncate()
                        .text_color(if done { gold(palette).hsla() } else { palette.on_surface_variant.hsla() })
                        .child(t(p.title)),
                )
            })
            .into_any_element()
    }
}

// ── Sidebar card ─────────────────────────────────────────────────────────

fn open_panel(cx: &mut App) {
    crate::state::shell(cx).update(cx, |s, cx| s.set_panel(Some(ID.into()), cx));
}

/// Opens the panel's reader on `id` (every word, a hadith's chain too).
pub(crate) fn open_item(id: ItemId, cx: &mut App) {
    let shared = model(cx);
    ensure_meanings(&shared, cx);
    {
        let mut m = shared.borrow_mut();
        m.open_request = Some(id);
        // The panel opens for this item: what the sidebar shows stays.
        m.reminders.hold(Instant::now());
    }
    open_panel(cx);
    changed(&shared, cx);
}

/// Opens the reader on the item `reference` names (`2:25`,
/// `bukhari-3245`), e.g. from a desktop widget.
pub fn open_reference(reference: &str, cx: &mut App) -> Result<(), String> {
    open_item(find(reference)?.id, cx);
    Ok(())
}

fn sidebar_view(palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let shared = model(cx);
    shared.borrow().views.track(window);
    ensure_timer(&shared, cx);
    let s = settings(cx);
    if wants_meaning(&s) {
        ensure_meanings(&shared, cx);
    }
    let f = fonts(cx);
    let accent = gold(palette);
    let arabic = arabic_ui();
    // The panel's own reminder (a fresh one when the sidebar opens): folded
    // here, never swapped for a shorter one.
    let item = current(&shared, &s, cx).and_then(|id| catalog().get(id));
    let date = Local::now().date_naive();
    let (meaning, done) = {
        let m = shared.borrow();
        (item.and_then(|i| meaning_of(&m, i)).filter(|_| wants_meaning(&s)), m.day_progress(date).done)
    };
    let head = div()
        .flex()
        .items_center()
        .gap(px(tokens::SM))
        .child(icon("arch", SIDEBAR_ICON_PX, accent))
        .child(div().flex_1().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(palette.on_surface_variant.hsla()).child(t("Jannah")))
        .when_some(item, |el, i| el.child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(accent.hsla()).child(catalog::reference(i, arabic))));
    // Folded to a few lines; Read more unfolds the card in place, and the
    // panel's reader (with a hadith's chain) is one link away.
    let body = item.map(|i| {
        use super::reading::{Block, fold, link};
        let tint = readable(palette.primary, palette);
        let arabic = Block::new(div().font_family(f.scripture.clone()).text_size(px(SIDEBAR_ARABIC_PX)).line_height(gpui::relative(ARABIC_LINE_EM)).text_right(), panel::words(i), SIDEBAR_LINES);
        let meaning = meaning.map(|m| Block::new(div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()), m.text, SIDEBAR_LINES));
        let id = i.id;
        let open = link("jannah-sidebar-open", t("Open in the reader"), tint, move |cx| open_item(id, cx));
        fold("jannah-sidebar-fold", &i.key, std::iter::once(arabic).chain(meaning), tokens::XS, tint, Some(open), window, cx)
    });
    let step = today_step().map(|p| {
        div()
            .id("jannah-sidebar-step")
            .flex()
            .items_center()
            .gap(px(tokens::SM))
            .pt(px(tokens::SM))
            .cursor_pointer()
            .child(panel::check(done, SIDEBAR_CHECK_PX, palette))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(if done { t("Today's step · done") } else { t("Today's step") }))
                    .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).truncate().child(t(p.title))),
            )
            .on_click(|_, _, cx| {
                mark_step_done(cx);
            })
    });
    card(palette).text_start().flex().flex_col().gap(px(tokens::SM)).child(head).when_some(body, |el, b| el.child(b)).when_some(step, |el, s| el.child(s)).into_any_element()
}

// ── Commands ─────────────────────────────────────────────────────────────

/// An item as JSON (its meaning when known).
fn item_json(shared: &Shared, item: &Item) -> Value {
    let m = shared.borrow();
    let meaning = meaning_of(&m, item);
    json!({
        "key": item.key,
        "kind": if item.is_verse() { "verse" } else { "hadith" },
        "reference": catalog::reference(item, false),
        "reference_ar": catalog::reference(item, true),
        "arabic": item.arabic,
        "arabic_full": item.full_arabic(),
        "meaning": meaning.as_ref().map(|m| m.text.clone()),
        "meaning_credit": meaning.as_ref().map(|m| m.credit.clone()).filter(|c| !c.is_empty()),
        "ai_rendering": meaning.as_ref().is_some_and(|m| m.ai),
        "themes": item.themes().iter().map(|th| th.id()).collect::<Vec<_>>(),
        "link": catalog::link(item),
        "citation": catalog::citation(item, false),
        "saved": m.state.is_saved(&item.key),
        "meaning_status": match (&m.fetch, item.is_verse() && meaning.is_none()) {
            (_, false) => Value::Null,
            (Fetch::Loading | Fetch::Idle, true) => json!("loading"),
            (Fetch::Offline, true) => json!("offline"),
            (Fetch::Failed(why), true) => json!({ "failed": why }),
            (Fetch::Ready(_), true) => json!("unavailable"),
        },
    })
}

fn path_json(p: &'static Path) -> Value {
    let (group, label) = catalog::group_label(&p.group);
    json!({ "key": p.key, "group": group, "group_label": t(label), "title": t(p.title), "line": t(p.line), "evidence": p.evidence })
}

fn find(reference: &str) -> Result<&'static Item, String> {
    catalog().find(reference).and_then(|id| catalog().get(id)).ok_or_else(|| mandara_i18n::tf("No verse or hadith {reference} in Jannah", &[("reference", &reference)]))
}

fn today_json(cx: &mut App) -> Result<Value, String> {
    let shared = model(cx);
    ensure_meanings(&shared, cx);
    let s = settings(cx);
    let item = current(&shared, &s, cx).and_then(|id| catalog().get(id)).ok_or_else(|| t("Nothing to show").to_string())?;
    let can_go_back = has_prev(&shared);
    let (n, of) = number(&s, item.id).unzip();
    Ok(json!({ "item": item_json(&shared, item), "number": n, "of": of, "step": today_step().map(path_json), "rotate": if s.rotate == Rotate::Hourly { "hourly" } else { "daily" }, "has_previous": can_go_back }))
}

fn step_json(cx: &mut App) -> Value {
    let shared = model(cx);
    let date = Local::now().date_naive();
    let m = shared.borrow();
    let progress = m.progress(date, crate::plugins::clock::week_start_days(cx));
    let week = progress.week;
    json!({
        "step": today_step().map(path_json),
        "done_today": progress.done,
        "week": week.iter().map(|(d, done)| json!({ "date": d.format("%Y-%m-%d").to_string(), "done": done })).collect::<Vec<_>>(),
    })
}

fn register_commands(r: &mut PluginRegistry) {
    // Every command answers in Jannah's own language.
    let mut add = |spec: CommandSpec| r.command(ID, plugin_lang::speaking(spec, own_lang));
    add(plugin_lang::language_command(ID, "jannah language", "jannah language ar"));
    add(CommandSpec::new("jannah open", tk("Open the Jannah panel"), |_, cx| {
        open_panel(cx);
        Ok(json!({"panel": ID}))
    }).example("jannah open"));
    add(CommandSpec::new_output("jannah copy-image", tk("Copy a reminder and its design as a complete PNG image"), |a, cx| share::command(&a.rest(0), cx))
        .arg(ArgSpec::required("ref", ArgKind::String, tk("a verse or hadith reference")))
        .example("jannah copy-image 89:30"));
    add(
        CommandSpec::new("jannah peek", tk("Look at another reminder without changing the one shown (negative: the ones shown before)"), |a, cx| {
            let by = if a.get(0).is_some() { a.int(0)? } else { 1 };
            let shared = model(cx);
            ensure_meanings(&shared, cx);
            let s = settings(cx);
            let item = look(&shared, &s, by).and_then(|id| catalog().get(id)).ok_or_else(|| t("Nothing to show").to_string())?;
            Ok(json!({ "peek": by, "item": item_json(&shared, item), "shown_moved": false }))
        })
        .arg(ArgSpec::optional("n", ArgKind::Integer, tk("1 when left out: another at random; negative: that many back through what was shown")))
        .example("jannah peek")
        .example("jannah peek -1"),
    );
    add(
        CommandSpec::new("jannah step peek", tk("Look at the step of a day after today (before it when negative) without marking anything"), |a, _| {
            let days = if a.get(0).is_some() { a.int(0)? } else { 1 };
            let date = Local::now().date_naive() + chrono::Duration::days(days);
            Ok(json!({ "days": days, "date": date.format("%Y-%m-%d").to_string(), "step": step_in(days).map(path_json), "marked": false }))
        })
        .arg(ArgSpec::optional("days", ArgKind::Integer, tk("how many days ahead (1 when left out; negative looks back)")))
        .example("jannah step peek")
        .example("jannah step peek 2"),
    );
    add(CommandSpec::new("jannah today", tk("The verse or hadith on screen now and today's step"), |_, cx| today_json(cx)));
    add(
        CommandSpec::new("jannah next", tk("Show a fresh reminder at random (every surface follows)"), |_, cx| {
            let shared = model(cx);
            next(&shared, cx);
            today_json(cx)
        }),
    );
    add(
        CommandSpec::new("jannah prev", tk("Go back to the reminder shown before"), |_, cx| {
            let shared = model(cx);
            let moved = prev(&shared, cx);
            let mut v = today_json(cx)?;
            v["moved"] = json!(moved);
            Ok(v)
        }),
    );
    add(
        CommandSpec::new("jannah show", tk("Open a verse or hadith in the Jannah panel and return it"), |a, cx| {
            let item = find(&a.rest(0))?;
            open_item(item.id, cx);
            Ok(item_json(&model(cx), item))
        })
        .arg(ArgSpec::required("ref", ArgKind::String, tk("2:25, quran-2:25 or bukhari-3245")))
        .example("jannah show 2:25")
        .example("jannah show bukhari-3245"),
    );
    add(
        CommandSpec::new("jannah search", tk("Search the verses and hadith (Arabic without diacritics, or the meaning)"), |a, cx| {
            let shared = model(cx);
            let meanings = match &shared.borrow().fetch {
                Fetch::Ready(m) => Some(m.clone()),
                _ => None,
            };
            let idx = build_index(mandara_i18n::language(), meanings.as_deref());
            let hits = search::search(&idx, &a.rest(0));
            Ok(json!(hits.iter().take(SEARCH_MAX).filter_map(|id| catalog().get(*id)).map(|i| item_json(&shared, i)).collect::<Vec<_>>()))
        })
        .arg(ArgSpec::required("query", ArgKind::String, tk("words in Arabic or in your language")))
        .example("jannah search الجنة")
        .example("jannah search rivers"),
    );
    add(
        CommandSpec::new("jannah paths", tk("The paths to Jannah, by group"), |_, _| {
            Ok(json!(catalog::GROUPS
                .iter()
                .map(|g| {
                    let (id, label) = catalog::group_label(g);
                    let paths: Vec<Value> = mandara_services::jannah::paths::PATHS.iter().filter(|p| catalog::same_group(&p.group, g)).map(path_json).collect();
                    json!({ "group": id, "label": t(label), "paths": paths })
                })
                .collect::<Vec<_>>()))
        }),
    );
    add(
        CommandSpec::new("jannah path", tk("One path with its evidence"), |a, cx| {
            let key = a.str(0)?;
            let p = catalog::path(key).ok_or_else(|| mandara_i18n::tf("No path {key}", &[("key", &key)]))?;
            let shared = model(cx);
            let mut v = path_json(p);
            v["evidence"] = json!(catalog::evidence(p).iter().filter_map(|id| catalog().get(*id)).map(|i| item_json(&shared, i)).collect::<Vec<_>>());
            Ok(v)
        })
        .arg(ArgSpec::required("key", ArgKind::String, tk("a path key from `jannah paths`")))
        .example("jannah path truthfulness"),
    );
    add(CommandSpec::new("jannah step", tk("Today's step and this week's done days"), |_, cx| Ok(step_json(cx))));
    add(
        CommandSpec::new("jannah step done", tk("Mark today's step done"), |_, cx| {
            let newly = mark_step_done(cx);
            let mut v = step_json(cx);
            v["newly_done"] = json!(newly);
            Ok(v)
        }),
    );
    add(
        CommandSpec::new("jannah saved", tk("Your saved verses and hadith, newest first"), |_, cx| {
            let shared = model(cx);
            let saved = shared.borrow().state.saved.clone();
            Ok(json!(saved
                .iter()
                .filter_map(|s| {
                    let item = catalog().find(&s.key).and_then(|id| catalog().get(id))?;
                    let mut v = item_json(&shared, item);
                    v["saved_at"] = json!(s.at);
                    Some(v)
                })
                .collect::<Vec<_>>()))
        }),
    );
    add(
        CommandSpec::new("jannah save", tk("Save a verse or hadith"), |a, cx| {
            let item = find(&a.rest(0))?;
            let shared = model(cx);
            let added = shared.borrow_mut().state.save_item(&item.key, Local::now().timestamp());
            if added {
                save(&shared, cx);
                changed(&shared, cx);
            }
            Ok(json!({ "key": item.key, "saved": true, "newly_saved": added }))
        })
        .arg(ArgSpec::required("ref", ArgKind::String, tk("2:25, quran-2:25 or bukhari-3245")))
        .example("jannah save 2:25"),
    );
    add(
        CommandSpec::new("jannah unsave", tk("Remove a saved verse or hadith"), |a, cx| {
            let item = find(&a.rest(0))?;
            let shared = model(cx);
            let removed = shared.borrow_mut().state.unsave(&item.key);
            if removed {
                save(&shared, cx);
                changed(&shared, cx);
            }
            Ok(json!({ "key": item.key, "saved": false, "removed": removed }))
        })
        .arg(ArgSpec::required("ref", ArgKind::String, tk("2:25, quran-2:25 or bukhari-3245")))
        .example("jannah unsave bukhari-3245"),
    );
}

// ── Registration ─────────────────────────────────────────────────────────

pub fn register(r: &mut PluginRegistry) {
    r.register(
        PluginManifest::builtin(ID, tk("Jannah"), tk("A gentle daily reminder of Paradise: a verse or hadith, its meaning and one good step for today (opt-in)"), Some(Zone::Right)).opt_in(),
        |window, cx, panel_cx| {
            let shared = model(cx);
            ensure_timer(&shared, cx);
            let bar = fn_view(None, plugin_lang::in_language(own_lang, bar_view(Rc::default())), window, cx);
            let sidebar = fn_view(None, plugin_lang::in_language(own_lang, sidebar_view), window, cx);
            let panel = panel::build(panel_cx, window, cx);
            PluginInstance {
                bar: Some(bar),
                panel: Some(panel),
                sidebar: Some(sidebar),
                on_click: None,
                // A right-click marks today's step done.
                on_secondary_click: Some(Rc::new(|_, cx| {
                    mark_step_done(cx);
                })),
            }
        },
    );
    // Registered after the plugin (registering clears a plugin's kinds).
    use mandara_config::desktop_widgets::{DesktopWidget, JannahLayout, JannahWidget, Look, Plate, WidgetKind};
    let one = |w: JannahWidget| vec![DesktopWidget { kind: WidgetKind::Jannah(w), ..DesktopWidget::default() }];
    r.desktop_widget(
        DesktopWidgetType::new(ID, "verse", tk("Jannah verse"), tk("Today's verse or hadith in calligraphy under an arch, with its meaning"), "arch", move || {
            one(JannahWidget { look: Look { plate: Plate::Glass, plate_radius: VERSE_PLATE_RADIUS_PX, ..Look::default() }, ..JannahWidget::default() })
        })
        .in_menu(),
    );
    r.desktop_widget(DesktopWidgetType::new(ID, "quote", tk("Jannah quote"), tk("Today's verse or hadith in the quote card's style"), "arch", move || {
        one(JannahWidget { layout: JannahLayout::Quote, ..JannahWidget::default() })
    }));
    r.desktop_widget(DesktopWidgetType::new(ID, "step", tk("Today's step"), tk("One good deed for today, with its check and your week"), "arch", move || {
        one(JannahWidget { layout: JannahLayout::Step, look: Look { plate: Plate::Glass, plate_radius: STEP_PLATE_RADIUS_PX, ..Look::default() }, ..JannahWidget::default() })
    }));
    register_commands(r);
}

/// Plate radii of the new widgets, reference px.
const VERSE_PLATE_RADIUS_PX: f32 = 30.0;
const STEP_PLATE_RADIUS_PX: f32 = 24.0;

/// Formats an item's reference chip text: `Al-Baqara 2:25 · البقرة`.
pub(crate) fn chip_text(item: &Item) -> SharedString {
    if arabic_ui() {
        return catalog::reference(item, true).into();
    }
    match item.verse_ref() {
        Some((s, _)) => format!("{} · {}", catalog::reference(item, false), catalog::surah_names(s).1).into(),
        None => match item.hadith() {
            Some(h) => format!("{} · {}", catalog::reference(item, false), h.collection.arabic()).into(),
            None => catalog::reference(item, false).into(),
        },
    }
}

/// Opens a link in the browser (off the UI thread).
pub(crate) fn open_link(url: String) {
    std::thread::spawn(move || {
        if let Err(err) = mandara_services::session::open_uri(&url) {
            tracing::warn!("jannah: opening {url}: {err}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Warm views reuse resolved settings; language and config edits still take effect.
    #[test]
    fn settings_cache_tracks_config_and_plugin_language_without_loading_data() {
        let mut config = mandara_config::ShellConfig::default();
        let mut cache = SettingsCache::<Settings>::new();
        let snapshot = Arc::new(config.clone());
        let first = cache.get(&snapshot, Lang::En, ());
        assert!(Rc::ptr_eq(&first, &cache.get(&snapshot, Lang::En, ())));
        assert_eq!(first.rotate, Rotate::Daily);
        config.plugins.settings.entry(ID.into()).or_default().insert("rotate".into(), toml::Value::String("hourly".into()));
        let changed = cache.get(&Arc::new(config), Lang::Ar, ());
        assert!(!Rc::ptr_eq(&first, &changed));
        assert_eq!(changed.lang, Lang::Ar);
        let held = SETTINGS.with(|cache| cache.borrow_mut().get(&snapshot, Lang::En, ()));
        clear_settings();
        assert!(!Rc::ptr_eq(&held, &SETTINGS.with(|cache| cache.borrow_mut().get(&snapshot, Lang::En, ()))));
        clear_settings();
        assert_eq!(changed.rotate, Rotate::Hourly);
    }


    fn s() -> Settings {
        Settings { rotate: Rotate::Daily, sources: Sources::Both, translation: true, arabic_size: ArabicSize::Medium, bar_step: false, morning: true, lang: Lang::En }
    }

    fn m() -> Model {
        Model::new(State::default())
    }

    #[test]
    fn the_timer_waits_for_the_rotation_or_the_morning() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        let mut model = m();
        // Before dawn: the morning reminder comes first.
        assert_eq!(next_due(&model, &s(), d.and_hms_opt(5, 0, 0).unwrap()), d.and_hms_opt(MORNING_HOUR, 0, 0).unwrap());
        // In the morning, not yet reminded: now.
        let nine = d.and_hms_opt(9, 0, 0).unwrap();
        assert_eq!(next_due(&model, &s(), nine), nine);
        model.state.set_reminded(d);
        model.generation += 1; // Saving a state change advances the model's revision.
        assert_eq!(next_due(&model, &s(), nine), (d + chrono::Duration::days(1)).and_hms_opt(0, 0, 0).unwrap());
        // Hourly, no reminder: the next hour.
        let hourly = Settings { rotate: Rotate::Hourly, morning: false, ..s() };
        assert_eq!(next_due(&model, &hourly, nine), d.and_hms_opt(10, 0, 0).unwrap());
    }

    #[test]
    fn the_widget_pool_holds_only_what_fits() {
        let small = pool(&s(), Some(WIDGET_LIMIT));
        assert!(!small.is_empty());
        assert!(small.iter().all(|id| catalog().get(id).is_some_and(|i| i.len <= WIDGET_LIMIT)));
        assert_eq!(pool(&s(), None).len(), catalog().items.len());
        let quran = pool(&Settings { sources: Sources::Quran, ..s() }, None);
        assert!(quran.iter().all(|id| catalog().get(id).is_some_and(|i| i.is_verse())));
    }

    #[test]
    fn hadith_meanings_fall_back_to_english() {
        let model = m();
        let c = catalog();
        let h = c.get(c.find("bukhari-6094").unwrap()).unwrap();
        let meaning = meaning_of(&model, h);
        // English, Turkish, … have the dataset's text; others the English.
        assert!(meaning.is_some_and(|m| !m.text.is_empty()) || arabic_ui());
    }
}
