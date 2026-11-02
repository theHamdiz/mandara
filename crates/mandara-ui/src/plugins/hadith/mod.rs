//! Hadith: a library of the hadith collections and a daily companion.
//! Whole collections (Sahih al-Bukhari and Sahih Muslim by default; the
//! four Sunan, the Muwatta and three forties on request) are downloaded
//! from the public-domain hadith-api dataset into
//! `~/.local/share/mandara/hadith/` when the plugin is first on, verified
//! and swapped in atomically (`mandara_services::hadith`). Nothing of the
//! text ships in the binary.
//!
//! Surfaces: a slim manuscript glyph in the bar (optionally with today's
//! reference); a panel (today's hadith under a gilded headpiece, the
//! collections with their download state, "continue where you left off",
//! a books → hadith browser, the reader, search, bookmarks); a sidebar
//! card; desktop widgets (`hadith`: today's hadith in the quote design,
//! `collection`: reading progress through a collection).
//!
//! "Today" is a pure function of the date and the chosen collections; its
//! card is cached in `~/.local/state/mandara/hadith.json` with bookmarks
//! and reading progress, so small surfaces never load a collection.
//! Nothing ticks while idle: one timer waits for the next rotation;
//! downloads, loading, search and file writes run off the UI thread.
//!
//! Settings (`[plugins.settings.hadith]`): `collections` (downloaded on
//! their own), `show_translation`, `arabic_size` (`small` / `medium` /
//! `large`), `hide_chain`, `rotate` (`daily` / `hourly`),
//! `daily_collections`, `bar_label` (`glyph` / `reference`).
//!
//! Commands: `hadith collections|status|today|bookmarks`, `hadith
//! download [collection|all]`, `hadith remove <collection|all>`, `hadith
//! show <ref>`, `hadith search <q> [--collection c] [--grade g]`,
//! `hadith bookmark add|remove <ref>`.

pub mod art;
mod panel;
mod reader;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Local, NaiveDateTime};
use gpui::{AnyElement, App, AppContext as _, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, Task, Window, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui;
use mandara_i18n::{Lang, t, tf, tk, tr};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, DesktopWidgetType, Output, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::hadith::collections::{self, Collection, info};
use mandara_services::hadith::data::Library;
use mandara_services::hadith::rotation::{self, Rotate};
use mandara_services::hadith::search::Query;
use mandara_services::hadith::state::{Card, State, Today};
use mandara_services::hadith::store::{self, Loaded, Progress, Status};
use mandara_services::hadith::{GradeClass, refs};
use mandara_theme::{Color, MandaraPalette};
use serde_json::{Value, json};

use super::fn_view;
use super::jannah::{ArabicSize, arabic_ui, gold};
use super::panel_fx::{ViewSet, readable};
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::{card, text, tokens};
use crate::state::config;

pub const ID: &str = "hadith";

// ── Sizes & timing ───────────────────────────────────────────────────────

/// Bar chip: glyph size, gap and the reference's widest run.
const BAR_ICON_PX: f32 = 14.0;
const BAR_GAP_PX: f32 = 6.0;
const BAR_LABEL_MAX_PX: f32 = 120.0;
/// Sidebar card: glyph, Arabic size and clamped lines.
const SIDEBAR_ICON_PX: f32 = 15.0;
const SIDEBAR_ARABIC_PX: f32 = 19.0;
const SIDEBAR_LINES: usize = 3;
/// Arabic line height (em): room for tashkeel above and below.
pub(crate) const ARABIC_LINE_EM: f32 = super::jannah::ARABIC_LINE_EM;
/// The rotation timer lands this far past the boundary.
const TICK_MARGIN: Duration = Duration::from_millis(250);
/// Results a `hadith search` returns at most.
const COMMAND_RESULTS: usize = 50;
/// Characters of the words a search result carries.
const RESULT_EXCERPT: usize = 240;

// ── Settings ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    /// Downloaded on their own.
    pub collections: Vec<Collection>,
    pub translation: bool,
    pub arabic_size: ArabicSize,
    /// The reader starts with the chain folded away.
    pub hide_chain: bool,
    pub rotate: Rotate,
    /// Where the hadith of the day comes from.
    pub daily: Vec<Collection>,
    /// Today's reference next to the glyph.
    pub bar_reference: bool,
}

fn ids(list: &[Collection]) -> Vec<String> {
    list.iter().map(|c| c.id().to_string()).collect()
}

pub(crate) fn settings(cx: &App) -> Settings {
    let cfg = config(cx).read(cx).snapshot();
    let s = &cfg.plugins;
    let list = |key: &str| collections::parse_list(&s.setting(ID, key, ids(&collections::defaults())));
    let daily = list("daily_collections");
    Settings {
        collections: list("collections"),
        translation: s.setting(ID, "show_translation", true),
        arabic_size: match s.setting(ID, "arabic_size", String::from("medium")).as_str() {
            "small" => ArabicSize::Small,
            "large" => ArabicSize::Large,
            _ => ArabicSize::Medium,
        },
        hide_chain: s.setting(ID, "hide_chain", true),
        rotate: Rotate::parse(&s.setting(ID, "rotate", String::from("daily"))),
        daily: if daily.is_empty() { collections::defaults() } else { daily },
        bar_reference: s.setting(ID, "bar_label", String::from("glyph")) == "reference",
    }
}

/// The interface language's code (`en`, `ar`).
pub(crate) fn ui_code() -> &'static str {
    mandara_i18n::language().code()
}

/// Whether meanings show under the Arabic.
pub(crate) fn wants_meaning(s: &Settings) -> bool {
    s.translation && !arabic_ui()
}

/// A collection's name in the interface language (Arabic in Arabic).
pub(crate) fn title(c: Collection) -> String {
    if arabic_ui() { info(c).arabic.to_string() } else { t(info(c).title).to_string() }
}

/// How a hadith is cited in the interface language: `Sahih al-Bukhari
/// 774b`, `صحيح البخاري ٧٧٤b`.
pub(crate) fn citation(c: Collection, reference: &str) -> String {
    if arabic_ui() { format!("{} {}", info(c).arabic, super::panel_fx::arabic_digits(reference)) } else { refs::citation(t(info(c).title), reference) }
}

/// The short reference on chips: `Bukhari 774b`.
pub(crate) fn short_ref(c: Collection, reference: &str) -> String {
    if arabic_ui() { citation(c, reference) } else { refs::short(c, reference) }
}

/// A grade family's colour from the theme: sahih emerald, hasan teal,
/// da'if amber.
fn grade_tone(class: GradeClass, palette: &MandaraPalette) -> Color {
    match class {
        GradeClass::Sahih => palette.success_emerald,
        GradeClass::Hasan => palette.success_emerald.mix(palette.primary, TEAL_SHARE).with_hue(TEAL_HUE_DEG),
        GradeClass::Daif => palette.warning_amber,
        GradeClass::Other => palette.on_surface_variant,
    }
}

/// A grade family's colour, readable on the panel's surface.
pub(crate) fn grade_color(class: GradeClass, palette: &MandaraPalette) -> Color {
    readable(grade_tone(class, palette), palette)
}

/// Teal: the emerald turned toward blue-green.
const TEAL_HUE_DEG: f32 = 195.0;
const TEAL_SHARE: f32 = 0.2;

/// A grade family's name (a catalog key).
pub(crate) fn grade_label(class: GradeClass) -> &'static str {
    match class {
        GradeClass::Sahih => tk("Sahih"),
        GradeClass::Hasan => tk("Hasan"),
        GradeClass::Daif => tk("Da'if"),
        GradeClass::Other => tk("Graded"),
    }
}

// ── The shared model ─────────────────────────────────────────────────────

pub(crate) struct Model {
    pub state: State,
    /// Bumped per change; the newest write wins.
    generation: u64,
    /// Every view showing the plugin (bar chips, sidebar cards, panels).
    pub views: ViewSet,
    /// The rotation timer and when it is due.
    timer: Option<(NaiveDateTime, Task<()>)>,
    /// What is on disk, for the language it was read for.
    pub statuses: Vec<Status>,
    statuses_lang: Option<Lang>,
    statuses_busy: bool,
    /// Downloads: waiting, under way, and the last error of each.
    pub queue: Vec<Collection>,
    pub current: Option<Progress>,
    running: bool,
    pub errors: BTreeMap<Collection, String>,
    /// Collections being opened, and why one couldn't be.
    pub loading: BTreeSet<Collection>,
    pub load_errors: BTreeMap<Collection, String>,
    /// "Today" is being picked; the slot and pool last tried.
    today_busy: bool,
    today_tried: Option<(i64, String)>,
    /// The collections the last automatic download looked at.
    auto_for: Option<Vec<Collection>>,
    /// A hadith a command asked the panel to open.
    pub open_request: Option<(Collection, String)>,
}

pub(crate) type Shared = Rc<RefCell<Model>>;

struct GlobalHadith(Shared);

impl Global for GlobalHadith {}

/// The model, loaded on first use (the state file is small).
pub(crate) fn model(cx: &mut App) -> Shared {
    if let Some(g) = cx.try_global::<GlobalHadith>() {
        return g.0.clone();
    }
    let shared: Shared = Rc::new(RefCell::new(Model {
        state: State::load(&mandara_services::hadith::state::path()),
        generation: 0,
        views: ViewSet::default(),
        timer: None,
        statuses: Vec::new(),
        statuses_lang: None,
        statuses_busy: false,
        queue: Vec::new(),
        current: None,
        running: false,
        errors: BTreeMap::new(),
        loading: BTreeSet::new(),
        load_errors: BTreeMap::new(),
        today_busy: false,
        today_tried: None,
        auto_for: None,
        open_request: None,
    }));
    cx.set_global(GlobalHadith(shared.clone()));
    refresh_statuses(&shared, cx);
    ensure_timer(&shared, cx);
    shared
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
            if let Err(e) = state.save(&mandara_services::hadith::state::path()) {
                tracing::warn!("hadith: saving state: {e}");
            }
            *last = generation;
        }
    })
    .detach();
}

/// Every view redraws.
pub(crate) fn notify(shared: &Shared, cx: &mut App) {
    let views = shared.borrow().views.clone();
    views.notify(cx);
}

/// Every view and the desktop widgets redraw.
pub(crate) fn changed(shared: &Shared, cx: &mut App) {
    notify(shared, cx);
    crate::backdrop::refresh(cx);
}

fn enabled(cx: &App) -> bool {
    let cfg = config(cx).read(cx).snapshot();
    crate::desktop_plugins::enabled(ID, &cfg, cx)
}

// ── What is on disk ──────────────────────────────────────────────────────

/// Re-reads what is on disk (off the UI thread), then downloads what is
/// wanted and missing and picks today.
pub(crate) fn refresh_statuses(shared: &Shared, cx: &mut App) {
    {
        let mut m = shared.borrow_mut();
        if m.statuses_busy {
            return;
        }
        m.statuses_busy = true;
    }
    let lang = mandara_i18n::language();
    let task = cx.background_spawn(async move { store::statuses(lang.code()) });
    let shared = shared.clone();
    cx.spawn(async move |cx| {
        let statuses = task.await;
        cx.update(|cx| {
            {
                let mut m = shared.borrow_mut();
                if m.statuses_lang != Some(lang) {
                    store::retain_language(lang.code());
                    m.load_errors.clear();
                }
                m.statuses = statuses;
                m.statuses_lang = Some(lang);
                m.statuses_busy = false;
                m.today_tried = None;
                m.auto_for = None;
            }
            ensure_today(&shared, cx);
            changed(&shared, cx);
        });
    })
    .detach();
}

/// Refreshes the statuses when the interface language changed since.
pub(crate) fn check_language(shared: &Shared, cx: &mut App) {
    let stale = {
        let m = shared.borrow();
        m.statuses_lang.is_some() && m.statuses_lang != Some(mandara_i18n::language())
    };
    if stale {
        refresh_statuses(shared, cx);
    }
}

pub(crate) fn status(m: &Model, c: Collection) -> Option<&Status> {
    m.statuses.iter().find(|s| s.collection == c)
}

/// The Arabic of `c` is on disk.
pub(crate) fn downloaded(m: &Model, c: Collection) -> bool {
    status(m, c).is_some_and(|s| s.arabic)
}

/// Collections on disk, in the library's order.
pub(crate) fn on_disk(m: &Model) -> Vec<Collection> {
    Collection::ALL.into_iter().filter(|c| downloaded(m, *c)).collect()
}

// ── Downloads ────────────────────────────────────────────────────────────

/// Downloads the chosen collections that are missing (once per change of
/// the choice), unless removed by you, failed, or offline.
fn auto_download(shared: &Shared, cx: &mut App) {
    let s = settings(cx);
    let want: Vec<Collection> = {
        let mut m = shared.borrow_mut();
        if m.statuses_lang.is_none() || m.auto_for.as_ref() == Some(&s.collections) {
            return;
        }
        m.auto_for = Some(s.collections.clone());
        s.collections.iter().copied().filter(|c| !m.state.declined(*c) && !m.errors.contains_key(c) && status(&m, *c).is_some_and(|st| !st.complete())).collect()
    };
    if want.is_empty() || mandara_services::offline::is_offline() || !enabled(cx) {
        return;
    }
    queue(shared, &want, cx);
}

/// Queues `list` for download (skipping what is queued or under way);
/// the collections added.
pub(crate) fn queue(shared: &Shared, list: &[Collection], cx: &mut App) -> Vec<Collection> {
    let mut added = Vec::new();
    {
        let mut m = shared.borrow_mut();
        for c in list {
            if m.queue.contains(c) || m.current.as_ref().is_some_and(|p| p.collection == *c) {
                continue;
            }
            m.errors.remove(c);
            m.load_errors.remove(c);
            m.state.wanted(*c);
            m.queue.push(*c);
            added.push(*c);
        }
    }
    if !added.is_empty() {
        save(shared, cx);
        run_queue(shared, cx);
        changed(shared, cx);
    }
    added
}

/// Works through the queue, one collection at a time, off the UI thread;
/// progress reaches the views as it arrives.
fn run_queue(shared: &Shared, cx: &mut App) {
    {
        let mut m = shared.borrow_mut();
        if m.running {
            return;
        }
        m.running = true;
    }
    let shared = shared.clone();
    cx.spawn(async move |cx| {
        loop {
            let next = cx.update(|_| {
                let mut m = shared.borrow_mut();
                if m.queue.is_empty() {
                    return None;
                }
                let c = m.queue.remove(0);
                let lang = ui_code();
                let total = collections::download_size(c, collections::translation(c, lang).map(|(l, _)| l));
                m.current = Some(Progress { collection: c, step: collections::edition(c, "ara"), done: 0, total });
                Some((c, lang))
            });
            let Some((c, lang)) = next else { break };
            cx.update(|cx| notify(&shared, cx));
            let (tx, rx) = async_channel::unbounded::<Progress>();
            let task = cx.background_executor().spawn(async move {
                store::download(c, lang, &|p| {
                    let _ = tx.send_blocking(p);
                })
            });
            while let Ok(p) = rx.recv().await {
                cx.update(|cx| {
                    shared.borrow_mut().current = Some(p);
                    notify(&shared, cx);
                });
            }
            let result = task.await;
            cx.update(|cx| {
                {
                    let mut m = shared.borrow_mut();
                    m.current = None;
                    if let Err(e) = result {
                        tracing::warn!("hadith: downloading {}: {e}", c.id());
                        m.errors.insert(c, store::explain(&e));
                    }
                }
                refresh_statuses(&shared, cx);
                changed(&shared, cx);
            });
        }
        cx.update(|_| shared.borrow_mut().running = false);
    })
    .detach();
}

/// Removes `list` from disk (off the UI thread) and remembers you removed
/// them; `done` gets the bytes freed.
pub(crate) fn remove(shared: &Shared, list: Vec<Collection>, cx: &mut App, done: impl FnOnce(Result<u64, String>, &mut App) + 'static) {
    {
        let mut m = shared.borrow_mut();
        m.queue.retain(|c| !list.contains(c));
        for c in &list {
            m.state.removed(*c);
            m.errors.remove(c);
        }
    }
    save(shared, cx);
    let work = list.clone();
    let task = cx.background_spawn(async move { work.into_iter().try_fold(0u64, |freed, c| store::remove(c).map(|n| freed + n)) });
    let shared = shared.clone();
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            refresh_statuses(&shared, cx);
            changed(&shared, cx);
            done(result, cx);
        });
    })
    .detach();
}

// ── Libraries ────────────────────────────────────────────────────────────

/// `c`, loaded for the interface language; starts loading it off the UI
/// thread when it isn't (the views are told when it is ready).
pub(crate) fn library(shared: &Shared, c: Collection, cx: &mut App) -> Option<Arc<Loaded>> {
    let lang = ui_code();
    if let Some(hit) = store::cached(c, lang) {
        return Some(hit);
    }
    {
        let mut m = shared.borrow_mut();
        if m.loading.contains(&c) || m.load_errors.contains_key(&c) || !downloaded(&m, c) {
            return None;
        }
        m.loading.insert(c);
    }
    let task = cx.background_spawn(async move { store::load(c, lang).map(|_| ()) });
    let shared = shared.clone();
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            {
                let mut m = shared.borrow_mut();
                m.loading.remove(&c);
                if let Err(e) = result {
                    m.load_errors.insert(c, e);
                }
            }
            notify(&shared, cx);
        });
    })
    .detach();
    None
}

// ── Today ────────────────────────────────────────────────────────────────

/// Today's card for `daily` collections (blocking: loads what isn't
/// loaded, without keeping it).
fn compute_today(daily: &[Collection], lang: &str, slot: i64) -> Option<Card> {
    let libs: Vec<(Collection, Arc<Library>)> = daily
        .iter()
        .filter_map(|c| store::cached(*c, lang).map(|l| l.library.clone()).or_else(|| store::read(*c, lang).ok().map(Arc::new)).map(|l| (*c, l)))
        .collect();
    let borrowed: Vec<(Collection, &Library)> = libs.iter().map(|(c, l)| (*c, l.as_ref())).collect();
    let pool = rotation::pool(&borrowed);
    let (c, index) = pool[rotation::pick(pool.len(), slot)?];
    let lib = libs.iter().find(|(x, _)| *x == c)?;
    Card::of(&lib.1, index)
}

/// The current slot and what today's pool is.
fn today_key(s: &Settings) -> (i64, String) {
    (rotation::period(Local::now().naive_local(), s.rotate), rotation::pool_key(&s.daily, collections::dataset_lang(ui_code())))
}

/// Picks today's hadith off the UI thread when the cached one is for
/// another period or pool (and something to pick from is downloaded).
pub(crate) fn ensure_today(shared: &Shared, cx: &mut App) {
    check_language(shared, cx);
    auto_download(shared, cx);
    let s = settings(cx);
    let (slot, pool) = today_key(&s);
    let daily: Vec<Collection> = {
        let mut m = shared.borrow_mut();
        let fresh = m.state.today.as_ref().is_some_and(|t| t.slot == slot && t.pool == pool);
        let tried = m.today_tried.as_ref() == Some(&(slot, pool.clone()));
        if fresh || tried || m.today_busy || m.statuses_lang.is_none() {
            return;
        }
        let daily: Vec<Collection> = s.daily.iter().copied().filter(|c| downloaded(&m, *c)).collect();
        if daily.is_empty() {
            return;
        }
        m.today_busy = true;
        m.today_tried = Some((slot, pool.clone()));
        daily
    };
    let lang = ui_code();
    let task = cx.background_spawn(async move { compute_today(&daily, lang, slot) });
    let shared = shared.clone();
    cx.spawn(async move |cx| {
        let card = task.await;
        cx.update(|cx| {
            {
                let mut m = shared.borrow_mut();
                m.today_busy = false;
                if let Some(card) = card {
                    m.state.today = Some(Today { slot, pool, card });
                }
            }
            save(&shared, cx);
            ensure_timer(&shared, cx);
            changed(&shared, cx);
        });
    })
    .detach();
}

/// Today's card (the cached one while a new one is picked).
pub(crate) fn today(shared: &Shared, cx: &mut App) -> Option<Card> {
    ensure_today(shared, cx);
    shared.borrow().state.today.as_ref().map(|t| t.card.clone())
}

/// Keeps one wake-up scheduled for the next rotation (nothing while the
/// plugin is off).
pub(crate) fn ensure_timer(shared: &Shared, cx: &mut App) {
    if !enabled(cx) {
        shared.borrow_mut().timer = None;
        return;
    }
    let s = settings(cx);
    let now = Local::now().naive_local();
    let due = rotation::next_boundary(now, s.rotate);
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
            ensure_today(&shared, cx);
            changed(&shared, cx);
            ensure_timer(&shared, cx);
        });
    });
    shared.borrow_mut().timer = Some((due, task));
}

// ── Reading ──────────────────────────────────────────────────────────────

/// Remembers reading hadith `index` of `c` (its place and progress).
pub(crate) fn visit(shared: &Shared, c: Collection, index: u32, key: &str, cx: &mut App) {
    let fresh = {
        let mut m = shared.borrow_mut();
        let already = m.state.last.as_ref().is_some_and(|p| p.key == key) && m.state.is_read(c, index);
        if !already {
            m.state.visit(c, index, key, Local::now().timestamp());
        }
        !already
    };
    if fresh {
        save(shared, cx);
        crate::backdrop::refresh(cx);
    }
}

/// Bookmarks or un-bookmarks `key`; whether it is bookmarked now.
pub(crate) fn toggle_bookmark(shared: &Shared, key: &str, cx: &mut App) -> bool {
    let on = shared.borrow_mut().state.toggle_bookmark(key, Local::now().timestamp());
    save(shared, cx);
    changed(shared, cx);
    on
}

/// Opens the panel.
pub(crate) fn open_panel(cx: &mut App) {
    crate::state::shell(cx).update(cx, |s, cx| s.set_panel(Some(ID.into()), cx));
}

/// Opens the panel's reader on `key` (`bukhari:1`).
pub(crate) fn open_key(shared: &Shared, key: &str, cx: &mut App) {
    if let Some((c, r)) = refs::parse(key) {
        shared.borrow_mut().open_request = Some((c, r));
        open_panel(cx);
        notify(shared, cx);
    }
}

/// The text to paste: the words and the citation, with the meaning when
/// asked.
pub(crate) fn copy_text(c: Collection, reference: &str, url: &str, words: &str, meaning: Option<&str>) -> String {
    let cite = citation(c, reference);
    match meaning.filter(|m| !m.trim().is_empty()) {
        Some(m) => format!("{words}\n\n{}\n\n— {cite} · {url}", m.trim()),
        None => format!("{words}\n\n— {cite}"),
    }
}

// ── Desktop widgets ──────────────────────────────────────────────────────

/// What the desktop widgets show.
pub fn widget_info(cx: &mut App) -> Option<crate::backdrop::widgets::HadithInfo> {
    use crate::backdrop::widgets::{HadithCard, HadithInfo, HadithProgress};
    let shared = model(cx);
    ensure_timer(&shared, cx);
    let card = today(&shared, cx);
    let s = settings(cx);
    let palette = mandara_core::theme_entity(cx).read(cx).target().clone();
    let rgb = |class: GradeClass| {
        let [r, g, b, _] = grade_tone(class, &palette).to_rgba8();
        (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
    };
    let m = shared.borrow();
    let today = card.map(|c| HadithCard {
        words: c.words.clone(),
        meaning: c.meaning.clone().filter(|_| wants_meaning(&s)),
        english: c.english,
        narrator: c.narrator.clone().filter(|_| wants_meaning(&s)),
        reference: refs::citation(t(info(c.collection).title), &c.reference),
        reference_ar: format!("{} {}", info(c.collection).arabic, super::panel_fx::arabic_digits(&c.reference)),
        grade: c.grade.as_ref().map(|g| (g.class, g.grade.clone(), rgb(g.class))),
    });
    let progress = Collection::ALL
        .into_iter()
        .map(|c| HadithProgress {
            collection: c,
            title: info(c).title.to_string(),
            arabic: info(c).arabic.to_string(),
            read: m.state.read_count(c),
            total: info(c).hadith,
            downloaded: downloaded(&m, c),
            last: m.state.places.get(c.id()).and_then(|p| refs::parse(&p.key)).map(|(c, r)| refs::citation(t(info(c).short), &r)),
        })
        .collect();
    Some(HadithInfo { today, progress })
}

// ── Bar chip ─────────────────────────────────────────────────────────────

fn bar_view(palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let shared = model(cx);
    shared.borrow().views.track(window);
    ensure_timer(&shared, cx);
    let card = today(&shared, cx);
    let s = settings(cx);
    let size = crate::style::bar_icon(BAR_ICON_PX);
    // A download under way tints the glyph gold.
    let busy = shared.borrow().current.is_some();
    let ink = if busy { gold(palette) } else { palette.on_surface_variant };
    let label = card.filter(|_| s.bar_reference).map(|c| short_ref(c.collection, &c.reference));
    div()
        .flex()
        .items_center()
        .gap(px(BAR_GAP_PX))
        .font_features(tabular_nums())
        .child(icon("hadith", size, ink))
        .when_some(label, |el, l| el.child(div().max_w(px(BAR_LABEL_MAX_PX)).truncate().text_color(palette.on_surface_variant.hsla()).child(l)))
        .into_any_element()
}

// ── Sidebar card ─────────────────────────────────────────────────────────

fn sidebar_view(palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let shared = model(cx);
    shared.borrow().views.track(window);
    ensure_timer(&shared, cx);
    let today_card = today(&shared, cx);
    let s = settings(cx);
    let f = fonts(cx);
    let accent = gold(palette);
    let link = readable(palette.primary, palette);
    let (last, nothing) = {
        let m = shared.borrow();
        (m.state.last.clone(), m.statuses_lang.is_some() && on_disk(&m).is_empty())
    };
    let head = div()
        .flex()
        .items_center()
        .gap(px(tokens::SM))
        .child(icon("hadith", SIDEBAR_ICON_PX, accent))
        .child(div().flex_1().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(palette.on_surface_variant.hsla()).child(t("Hadith of the day")))
        .when_some(today_card.as_ref(), |el, c| el.child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).font_features(tabular_nums()).text_color(accent.hsla()).child(short_ref(c.collection, &c.reference))));
    let body = today_card.map(|c| {
        div()
            .flex()
            .flex_col()
            .gap(px(tokens::XS))
            .child(div().font_family(f.hadith.clone()).text_size(px(SIDEBAR_ARABIC_PX)).line_height(gpui::relative(ARABIC_LINE_EM)).text_right().line_clamp(SIDEBAR_LINES).child(crate::bidi::paragraph(c.words.clone())))
            .when_some(c.meaning.clone().filter(|_| wants_meaning(&s)), |el, m| {
                el.child(div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).line_clamp(SIDEBAR_LINES).child(crate::bidi::paragraph(m)))
            })
            .child(
                div()
                    .id("hadith-sidebar-more")
                    .flex()
                    .cursor_pointer()
                    .text_size(px(text::LABEL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(link.hsla())
                    .child(t("Read more"))
                    .on_click({
                        let (shared, key) = (shared.clone(), c.key.clone());
                        move |_, _, cx| open_key(&shared, &key, cx)
                    }),
            )
    });
    let hint = (nothing && body.is_none()).then(|| {
        div()
            .id("hadith-sidebar-get")
            .cursor_pointer()
            .text_size(px(text::BODY_S))
            .text_color(palette.on_surface_variant.hsla())
            .child(t("Download the library in the Hadith panel to read a hadith a day"))
            .on_click(|_, _, cx| open_panel(cx))
    });
    let resume = last.and_then(|p| refs::parse(&p.key)).map(|(c, r)| {
        let key = refs::key(c, &r);
        div()
            .id("hadith-sidebar-continue")
            .flex()
            .items_center()
            .gap(px(tokens::SM))
            .pt(px(tokens::XS))
            .cursor_pointer()
            .child(icon("bookmark", SIDEBAR_ICON_PX, accent))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(t("Continue where you left off")))
                    .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).font_features(tabular_nums()).truncate().child(citation(c, &r))),
            )
            .child(icon(crate::bidi::dir_icon("chevron-right"), SIDEBAR_ICON_PX, palette.on_surface_variant))
            .on_click({
                let shared = shared.clone();
                move |_, _, cx| open_key(&shared, &key, cx)
            })
    });
    card(palette).flex().flex_col().gap(px(tokens::SM)).child(head).when_some(body, |el, b| el.child(b)).when_some(hint, |el, h| el.child(h)).when_some(resume, |el, r| el.child(r)).into_any_element()
}

// ── Commands ─────────────────────────────────────────────────────────────

/// Runs `work` off the UI thread, then `then` on it (with the model), and
/// answers with the result.
fn job<T: Send + 'static>(cx: &mut App, work: impl FnOnce() -> Result<T, String> + Send + 'static, then: impl FnOnce(T, &mut App) -> Value + 'static) -> Output {
    let (tx, rx) = async_channel::bounded(1);
    let task = cx.background_spawn(async move { work() });
    cx.spawn(async move |cx| {
        let result = match task.await {
            Ok(v) => Ok(cx.update(|cx| then(v, cx))),
            Err(e) => Err(e),
        };
        let _ = tx.send(result.map(|v| v.to_string())).await;
    })
    .detach();
    Output::Pending(rx)
}

fn class_json(class: Option<GradeClass>) -> Value {
    class.map_or(Value::Null, |c| json!(c.id()))
}

/// A hadith as JSON.
fn hadith_json(lib: &Library, index: u32) -> Option<Value> {
    let h = lib.get(index)?;
    let c = lib.collection;
    let book = lib.book_of(index)?;
    let (before, after) = h.chain();
    Some(json!({
        "key": lib.key(index),
        "collection": c.id(),
        "collection_title": t(info(c).title),
        "collection_arabic": info(c).arabic,
        "reference": h.reference,
        "citation": citation(c, &h.reference),
        "url": lib.url(index),
        "book": { "number": book.number, "name": book.name },
        "in_book": h.in_book,
        "arabic": h.arabic,
        "words": h.words(),
        "chain": before,
        "after_words": after,
        "meaning": h.meaning,
        "meaning_language": lib.meaning.as_ref().map(|m| m.lang.clone()),
        "english_fallback": lib.meaning.as_ref().is_some_and(|m| m.fallback),
        "translator": lib.meaning.as_ref().and_then(|m| m.translator.clone()),
        "narrator": h.narrator,
        "grade": class_json(h.class(c)),
        "grades": h.grades.iter().map(|g| json!({ "grader": g.grader, "grade": g.grade, "class": g.class.id() })).collect::<Vec<_>>(),
    }))
}

/// A card as JSON (today's).
fn card_json(c: &Card) -> Value {
    json!({
        "key": c.key,
        "collection": c.collection.id(),
        "reference": c.reference,
        "citation": citation(c.collection, &c.reference),
        "url": c.url,
        "book": { "number": c.book, "name": c.book_name },
        "arabic": c.arabic,
        "words": c.words,
        "meaning": c.meaning_full,
        "english_fallback": c.english,
        "narrator": c.narrator,
        "grade": c.grade.as_ref().map(|g| json!({ "class": g.class.id(), "grade": g.grade, "grader": g.grader })),
    })
}

fn collection_json(c: Collection, st: Option<&Status>, s: &Settings, state: &State) -> Value {
    let i = info(c);
    json!({
        "id": c.id(),
        "title": t(i.title),
        "arabic": i.arabic,
        "hadith": i.hadith,
        "dataset_entries": i.entries,
        "graded": i.graded,
        "sahih": i.sahih,
        "chosen": s.collections.contains(&c),
        "daily": s.daily.contains(&c),
        "downloaded": st.is_some_and(|x| x.arabic),
        "translation": st.and_then(|x| x.translation.as_ref()).map(|(lang, have)| json!({ "language": lang, "downloaded": have })),
        "bytes_on_disk": st.map_or(0, |x| x.bytes),
        "download_bytes": collections::download_size(c, collections::translation(c, ui_code()).map(|(l, _)| l)),
        "read": state.read_count(c),
        "removed_by_you": state.declined(c),
    })
}

/// `collection`, `all`, or (with none) the chosen collections.
fn which(arg: Option<&str>, cx: &App) -> Result<Vec<Collection>, String> {
    match arg.map(str::trim) {
        None | Some("") => {
            let s = settings(cx);
            Ok(if s.collections.is_empty() { collections::defaults() } else { s.collections })
        }
        Some("all") => Ok(Collection::ALL.to_vec()),
        Some(name) => collections::alias(name).map(|c| vec![c]).ok_or_else(|| tf("No collection {name}; see `hadith collections`", &[("name", &name)])),
    }
}

fn progress_json(m: &Model) -> Value {
    json!({
        "current": m.current.as_ref().map(|p| json!({ "collection": p.collection.id(), "step": p.step, "bytes": p.done, "total_bytes": p.total })),
        "queued": m.queue.iter().map(|c| c.id()).collect::<Vec<_>>(),
        "errors": m.errors.iter().map(|(c, e)| (c.id().to_string(), json!(e))).collect::<serde_json::Map<String, Value>>(),
    })
}

/// A parsed reference that is downloaded, or why not.
fn wanted_ref(input: &str, cx: &mut App) -> Result<(Collection, String), String> {
    let (c, r) = refs::parse(input).ok_or_else(|| tf("Can't read {reference}: try `bukhari 1` or `muslim 2699a`", &[("reference", &input)]))?;
    let shared = model(cx);
    if !downloaded(&shared.borrow(), c) {
        return Err(tf("{collection} isn't downloaded: run `hadith download {id}`", &[("collection", &tr(info(c).title)), ("id", &c.id())]));
    }
    Ok((c, r))
}

/// Finds `reference` in `c` (loading it when needed; blocking).
fn find(c: Collection, reference: &str, lang: &str) -> Result<(Arc<Loaded>, u32), String> {
    let loaded = store::load(c, lang)?;
    let index = loaded.library.find(reference).ok_or_else(|| tf("No hadith {reference} in {collection}", &[("reference", &reference), ("collection", &tr(info(c).title))]))?;
    Ok((loaded, index))
}

fn register_commands(r: &mut PluginRegistry) {
    r.command(
        ID,
        CommandSpec::new_output("hadith collections", tk("Every collection: download state, size, progress and whether it is chosen"), |_, cx| {
            let lang = ui_code();
            Ok(job(cx, move || Ok(store::statuses(lang)), |statuses, cx| {
                let shared = model(cx);
                let s = settings(cx);
                let m = shared.borrow();
                json!(Collection::ALL.iter().map(|c| collection_json(*c, statuses.iter().find(|x| x.collection == *c), &s, &m.state)).collect::<Vec<_>>())
            }))
        }),
    );
    r.command(
        ID,
        CommandSpec::new("hadith download", tk("Download a collection (Arabic and the meaning in your language), all of them, or (with none) your chosen ones; returns at once, see `hadith status`"), |a, cx| {
            let list = which(a.get(0), cx)?;
            mandara_services::offline::check().map_err(|e| store::explain(&e))?;
            let shared = model(cx);
            let added = queue(&shared, &list, cx);
            let m = shared.borrow();
            Ok(json!({ "queued": added.iter().map(|c| c.id()).collect::<Vec<_>>(), "progress": progress_json(&m) }))
        })
        .arg(ArgSpec::optional("collection", ArgKind::String, tk("a collection id (bukhari, muslim, abudawud, tirmidhi, nasai, ibnmajah, malik, nawawi, qudsi, dehlawi) or all")))
        .example("hadith download")
        .example("hadith download abudawud")
        .example("hadith download all"),
    );
    r.command(
        ID,
        CommandSpec::new_output("hadith remove", tk("Delete a collection's files (or all of them); it isn't downloaded again on its own"), |a, cx| {
            let list = which(Some(a.str(0)?), cx)?;
            let shared = model(cx);
            let (tx, rx) = async_channel::bounded(1);
            let names: Vec<&'static str> = list.iter().map(|c| c.id()).collect();
            remove(&shared, list, cx, move |result, _| {
                let _ = tx.send_blocking(result.map(|freed| json!({ "removed": names, "freed_bytes": freed }).to_string()));
            });
            Ok(Output::Pending(rx))
        })
        .arg(ArgSpec::required("collection", ArgKind::String, tk("a collection id or all")))
        .example("hadith remove abudawud")
        .example("hadith remove all"),
    );
    r.command(
        ID,
        CommandSpec::new_output("hadith status", tk("What is downloaded, what is downloading and how far, and any errors"), |_, cx| {
            let lang = ui_code();
            Ok(job(cx, move || Ok(json!(store::statuses(lang))), |v: Value, cx| {
                let shared = model(cx);
                refresh_statuses(&shared, cx);
                let m = shared.borrow();
                json!({
                    "folder": store::dir(),
                    "source": store::HOME,
                    "offline": mandara_services::offline::is_offline(),
                    "collections": v,
                    "downloads": progress_json(&m),
                })
            }))
        }),
    );
    r.command(
        ID,
        CommandSpec::new_output("hadith today", tk("The hadith of the day (or the hour), the same as every surface shows"), |_, cx| {
            let shared = model(cx);
            ensure_today(&shared, cx);
            let s = settings(cx);
            let (slot, pool) = today_key(&s);
            if let Some(t) = shared.borrow().state.today.as_ref().filter(|t| t.slot == slot && t.pool == pool) {
                return Ok(Output::Value(card_json(&t.card)));
            }
            let daily: Vec<Collection> = s.daily.iter().copied().filter(|c| downloaded(&shared.borrow(), *c)).collect();
            if daily.is_empty() {
                return Err(t("Nothing to pick from yet: download Sahih al-Bukhari or Sahih Muslim (`hadith download`)").into());
            }
            let lang = ui_code();
            Ok(job(cx, move || compute_today(&daily, lang, slot).map(|c| card_json(&c)).ok_or_else(|| t("The hadith of the day couldn't be picked").to_string()), |v, _| v))
        }),
    );
    r.command(
        ID,
        CommandSpec::new_output("hadith show", tk("Open a hadith in the panel's reader and return it"), |a, cx| {
            let (c, reference) = wanted_ref(&a.rest(0), cx)?;
            let lang = ui_code();
            Ok(job(
                cx,
                move || {
                    let (loaded, index) = find(c, &reference, lang)?;
                    hadith_json(&loaded.library, index).ok_or_else(|| t("Not found").to_string())
                },
                |mut v, cx| {
                    let shared = model(cx);
                    if let Some(key) = v["key"].as_str().map(str::to_string) {
                        v["bookmarked"] = json!(shared.borrow().state.is_bookmarked(&key));
                        open_key(&shared, &key, cx);
                    }
                    v
                },
            ))
        })
        .arg(ArgSpec::required("ref", ArgKind::String, tk("a reference: bukhari 1, muslim 2699a, abudawud 4031")))
        .example("hadith show bukhari 1")
        .example("hadith show muslim 2699a")
        .example("hadith show abudawud 4031"),
    );
    r.command(
        ID,
        CommandSpec::new_output("hadith search", tk("Search the downloaded collections: Arabic without diacritics, the meaning or a reference; filter by collection and grade"), |a, cx| {
            let q = Query::parse(&a.0)?;
            if q.text.trim().is_empty() {
                return Err(t("Search for something: words in Arabic or in your language").into());
            }
            let shared = model(cx);
            let list: Vec<Collection> = on_disk(&shared.borrow()).into_iter().filter(|c| q.wants(*c)).collect();
            if list.is_empty() {
                return Err(t("None of those collections is downloaded").into());
            }
            let lang = ui_code();
            Ok(job(
                cx,
                move || {
                    let mut results = Vec::new();
                    let mut total = 0;
                    for c in list {
                        let loaded = store::load(c, lang)?;
                        let hits = mandara_services::hadith::search::search(&loaded.index(), &q, usize::MAX);
                        total += hits.len();
                        let lib = &loaded.library;
                        for i in hits.into_iter().take(COMMAND_RESULTS.saturating_sub(results.len())) {
                            let Some(h) = lib.get(i) else { continue };
                            results.push(json!({
                                "key": lib.key(i),
                                "citation": citation(c, &h.reference),
                                "url": lib.url(i),
                                "words": h.words().chars().take(RESULT_EXCERPT).collect::<String>(),
                                "meaning": h.meaning_body().map(|m| m.chars().take(RESULT_EXCERPT).collect::<String>()),
                                "grade": class_json(h.class(c)),
                            }));
                        }
                    }
                    Ok(json!({ "query": q.text, "grade": q.grade.map(|g| g.id()), "matches": total, "results": results }))
                },
                |v, _| v,
            ))
        })
        .arg(ArgSpec::required("query", ArgKind::String, tk("words, then optionally --collection <id> and --grade sahih|hasan|daif")))
        .example("hadith search الأعمال بالنيات")
        .example("hadith search mercy --collection bukhari")
        .example("hadith search الصلاة --grade sahih"),
    );
    r.command(
        ID,
        CommandSpec::new_output("hadith bookmark", tk("Bookmark a hadith, or remove its bookmark"), |a, cx| {
            let add = a.str(0)? == "add";
            let input = a.rest(1);
            let shared = model(cx);
            // A bookmark of a collection no longer downloaded can still go.
            if !add && let Some((c, r)) = refs::parse(&input).filter(|(c, _)| !downloaded(&shared.borrow(), *c)) {
                let key = refs::key(c, &r);
                let removed = shared.borrow_mut().state.unbookmark(&key);
                save(&shared, cx);
                changed(&shared, cx);
                return Ok(Output::Value(json!({ "key": key, "bookmarked": false, "changed": removed })));
            }
            let (c, reference) = wanted_ref(&input, cx)?;
            let lang = ui_code();
            Ok(job(
                cx,
                move || {
                    let (loaded, index) = find(c, &reference, lang)?;
                    Ok(json!(loaded.library.key(index)))
                },
                move |v, cx| {
                    let key = v.as_str().unwrap_or_default().to_string();
                    let shared = model(cx);
                    let now = Local::now().timestamp();
                    let changed_now = if add { shared.borrow_mut().state.bookmark(&key, now) } else { shared.borrow_mut().state.unbookmark(&key) };
                    save(&shared, cx);
                    changed(&shared, cx);
                    json!({ "key": key, "bookmarked": add, "changed": changed_now })
                },
            ))
        })
        .arg(ArgSpec::required("action", ArgKind::Enum, tk("add or remove")).choices(&["add", "remove"]))
        .arg(ArgSpec::required("ref", ArgKind::String, tk("a reference: bukhari 1, muslim 2699a")))
        .example("hadith bookmark add bukhari 1")
        .example("hadith bookmark remove muslim 2699a"),
    );
    r.command(
        ID,
        CommandSpec::new_output("hadith bookmarks", tk("Your bookmarked hadith, newest first"), |_, cx| {
            let shared = model(cx);
            let (marks, have) = {
                let m = shared.borrow();
                (m.state.bookmarks.clone(), on_disk(&m))
            };
            let lang = ui_code();
            Ok(job(
                cx,
                move || {
                    let out: Vec<Value> = marks
                        .iter()
                        .map(|b| {
                            let found = refs::parse(&b.key).filter(|(c, _)| have.contains(c)).and_then(|(c, r)| find(c, &r, lang).ok()).and_then(|(l, i)| hadith_json(&l.library, i));
                            match found {
                                Some(mut v) => {
                                    v["bookmarked_at"] = json!(b.at);
                                    v
                                }
                                None => json!({ "key": b.key, "bookmarked_at": b.at, "available": false }),
                            }
                        })
                        .collect();
                    Ok(json!(out))
                },
                |v, _| v,
            ))
        }),
    );
}

// ── Registration ─────────────────────────────────────────────────────────

/// Plate radius of the hadith widget, reference px.
const WIDGET_PLATE_RADIUS_PX: f32 = 28.0;

pub fn register(r: &mut PluginRegistry) {
    r.register(
        PluginManifest::builtin(ID, tk("Hadith"), tk("A library of the hadith collections, downloaded on demand, with a hadith of the day, a reader, search and bookmarks (opt-in)"), Some(Zone::Right)).opt_in(),
        |window, cx, panel_cx| {
            let shared = model(cx);
            ensure_timer(&shared, cx);
            let bar = fn_view(None, bar_view, window, cx);
            let sidebar = fn_view(None, sidebar_view, window, cx);
            let panel = panel::build(panel_cx, window, cx);
            PluginInstance { bar: Some(bar), panel: Some(panel), sidebar: Some(sidebar), on_click: None, on_secondary_click: None }
        },
    );
    // Registered after the plugin (registering clears a plugin's kinds).
    use mandara_config::desktop_widgets::{DesktopWidget, HadithLayout, HadithWidget, Look, Plate, WidgetKind};
    let one = |w: HadithWidget| vec![DesktopWidget { kind: WidgetKind::Hadith(w), ..DesktopWidget::default() }];
    r.desktop_widget(
        DesktopWidgetType::new(ID, "hadith", tk("Hadith of the day"), tk("Today's hadith in Amiri with its meaning and reference"), "hadith", move || {
            one(HadithWidget { look: Look { plate: Plate::Glass, plate_radius: WIDGET_PLATE_RADIUS_PX, ..Look::default() }, ..HadithWidget::default() })
        })
        .in_menu(),
    );
    r.desktop_widget(DesktopWidgetType::new(ID, "collection", tk("Reading progress"), tk("How far you have read through a hadith collection"), "hadith", move || {
        one(HadithWidget { layout: HadithLayout::Collection, look: Look { plate: Plate::Glass, plate_radius: WIDGET_PLATE_RADIUS_PX, ..Look::default() }, ..HadithWidget::default() })
    }));
    register_commands(r);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_carry_the_citation() {
        let plain = copy_text(Collection::Bukhari, "1", "https://sunnah.com/bukhari:1", "كَلاَمٌ", None);
        assert!(plain.starts_with("كَلاَمٌ") && plain.contains("— ") && !plain.contains("sunnah.com"));
        let full = copy_text(Collection::Bukhari, "1", "https://sunnah.com/bukhari:1", "كَلاَمٌ", Some(" words "));
        assert!(full.contains("\n\nwords\n\n") && full.ends_with("https://sunnah.com/bukhari:1"));
    }

    #[test]
    fn grades_have_their_colours_and_names() {
        let p = mandara_theme::engine::default_palette(true);
        assert_ne!(grade_color(GradeClass::Sahih, &p), grade_color(GradeClass::Daif, &p));
        assert_ne!(grade_color(GradeClass::Sahih, &p), grade_color(GradeClass::Hasan, &p));
        assert_eq!(grade_label(GradeClass::Daif), "Da'if");
    }
}
