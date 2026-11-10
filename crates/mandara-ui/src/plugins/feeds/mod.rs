//! Feeds: a reader for RSS, Atom and JSON Feed subscriptions. The bar
//! shows the unread count from the local history; the panel holds a
//! sidebar of folders and feeds, one timeline of every article, search,
//! read, unread and starred flags, and a clean reader view.
//!
//! Lifecycles: the bar reads counts from disk once (off the UI thread).
//! The panel loads the history when it opens, refreshes feeds that are
//! due while it is visible, and releases the history, the article and
//! every decoded picture shortly after it closes. Nothing downloads
//! while nothing shows the feeds, unless you turned on background
//! refresh: then one coalesced refresh runs at a low, fixed interval.

mod card;
mod images;
mod library;
mod panel;
mod reader;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, Task, px, size};
use mandara_config::ShellConfig;
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::feeds::{Cache, Counts, HttpSource, Limits, Snapshot, Subscription};

use super::extras::Flash;
use super::panel_fx::ViewSet;
use super::settings_cache::{ConfigSettings, SettingsCache};
pub use library::host;
use library::{Filter, Item, Library, Scope};

pub const ID: &str = "feeds";
/// Subscriptions kept at most (a reading list, not a crawler).
pub const MAX_SUBSCRIPTIONS: usize = 64;
/// How old a feed may get while the panel is open before it refreshes.
pub const DEFAULT_REFRESH_SECS: i64 = 300;
pub const MIN_REFRESH_SECS: i64 = 60;
pub const MAX_REFRESH_SECS: i64 = 3600;
/// Background refresh (opt-in): how often, in minutes.
pub const DEFAULT_BACKGROUND_MIN: i64 = 60;
pub const MIN_BACKGROUND_MIN: i64 = 15;
pub const MAX_BACKGROUND_MIN: i64 = 360;
/// Articles kept per feed (starred ones come on top).
pub const DEFAULT_ITEMS: i64 = 50;
pub const MIN_ITEMS: i64 = 10;
pub const MAX_ITEMS: i64 = 200;
/// Days an article stays after it left its feed.
pub const DEFAULT_RETENTION_DAYS: i64 = 14;
pub const MIN_RETENTION_DAYS: i64 = 1;
pub const MAX_RETENTION_DAYS: i64 = 90;
/// Downloads of one refresh running at once (on the shared blocking pool).
const REFRESH_LANES: usize = 3;
/// New-article notifications come at most this often.
const NOTIFY_GAP: Duration = Duration::from_secs(30 * 60);
/// The panel's history and pictures go this long after it closes (the
/// close animation never shows them disappearing).
const RELEASE_AFTER_CLOSE: Duration = Duration::from_millis(1500);
/// The first background refresh waits this long after start-up.
const BACKGROUND_FIRST_WAIT: Duration = Duration::from_secs(120);
/// The panel's starting size; it is a two-pane reader.
const PANEL_W: f32 = 760.0;
const PANEL_H: f32 = 580.0;
const SECS_PER_MIN: u64 = 60;

/// Which pictures load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pictures {
    /// None: monograms instead of feed icons, no thumbnails.
    Off,
    /// Feed icons and the thumbnails feeds declare (the default).
    Thumbnails,
    /// Also the images inside articles (the reader asks their servers).
    All,
}

/// How the bar item looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarStyle { Badge, Count, Icon }

/// Feeds' settings, resolved once per configuration.
pub struct Settings {
    pub subs: Rc<[Subscription]>,
    /// A fingerprint of `subs` (addresses, names, folders, colours): what
    /// the sidebar card's cached rows depend on.
    pub subs_key: u64,
    pub refresh: Duration,
    pub background: Option<Duration>,
    pub limits: Limits,
    pub pictures: Pictures,
    pub notify: bool,
    pub mark_read_on_open: bool,
    pub open_in_browser: bool,
    pub bar: BarStyle,
    /// The sidebar card's folder (empty: every feed).
    pub card_folder: Rc<str>,
}

/// The subscriptions in configured order: the `feeds` list, or the older
/// space-separated `subscriptions` text. Duplicates and non-web
/// addresses are skipped; at most [`MAX_SUBSCRIPTIONS`].
pub fn subscriptions(cfg: &ShellConfig) -> Vec<Subscription> {
    let table = cfg.plugins.settings.get(ID);
    let mut out: Vec<Subscription> = Vec::new();
    let mut push = |sub: Subscription| {
        let url = sub.url.trim();
        if out.len() < MAX_SUBSCRIPTIONS && mandara_services::feeds::web_url(url) && !out.iter().any(|seen| seen.url == url) {
            out.push(Subscription { url: url.to_owned(), ..sub });
        }
    };
    match table.and_then(|table| table.get("feeds")).and_then(toml::Value::as_array) {
        Some(list) => list.iter().filter_map(|value| value.clone().try_into::<Subscription>().ok()).for_each(&mut push),
        None => table.and_then(|table| table.get("subscriptions")).and_then(toml::Value::as_str).unwrap_or_default()
            .split_whitespace().for_each(|url| push(Subscription { url: url.into(), ..Subscription::default() })),
    }
    out
}

/// Write `list` as the `feeds` setting (and drop the older text form).
pub fn set_subscriptions(cfg: &mut ShellConfig, list: &[Subscription]) {
    let table = cfg.plugins.settings.entry(ID.into()).or_default();
    table.remove("subscriptions");
    let values = list.iter().take(MAX_SUBSCRIPTIONS).filter_map(|sub| toml::Value::try_from(sub).ok()).collect();
    table.insert("feeds".into(), toml::Value::Array(values));
}

fn choice<'a>(cfg: &'a ShellConfig, key: &str) -> &'a str { cfg.plugins.setting_str(ID, key).unwrap_or_default() }

/// Resolve every setting (Studio and commands use this uncached).
pub fn settings(cfg: &ShellConfig) -> Settings {
    let p = &cfg.plugins;
    let int = |key: &str, default: i64, min: i64, max: i64| p.setting(ID, key, default).clamp(min, max);
    let subs: Rc<[Subscription]> = subscriptions(cfg).into();
    Settings {
        subs_key: fingerprint(&subs),
        subs,
        refresh: Duration::from_secs(int("refresh_seconds", DEFAULT_REFRESH_SECS, MIN_REFRESH_SECS, MAX_REFRESH_SECS) as u64),
        background: p.setting(ID, "background_refresh", false).then(|| Duration::from_secs(int("background_minutes", DEFAULT_BACKGROUND_MIN, MIN_BACKGROUND_MIN, MAX_BACKGROUND_MIN) as u64 * SECS_PER_MIN)),
        limits: Limits { items: int("items_per_feed", DEFAULT_ITEMS, MIN_ITEMS, MAX_ITEMS) as usize, retention_days: int("retention_days", DEFAULT_RETENTION_DAYS, MIN_RETENTION_DAYS, MAX_RETENTION_DAYS) as u32 },
        pictures: match choice(cfg, "images") { "off" => Pictures::Off, "all" => Pictures::All, _ => Pictures::Thumbnails },
        notify: p.setting(ID, "notify", false),
        mark_read_on_open: p.setting(ID, "mark_read_on_open", true),
        open_in_browser: choice(cfg, "open_in") == "browser",
        bar: match choice(cfg, "bar_style") { "count" => BarStyle::Count, "icon" => BarStyle::Icon, _ => BarStyle::Badge },
        card_folder: choice(cfg, "card_folder").trim().into(),
    }
}

/// A cheap fingerprint of the subscriptions, taken once per configuration.
fn fingerprint(subs: &[Subscription]) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for sub in subs { (&sub.url, &sub.title, &sub.folder, &sub.color).hash(&mut hasher); }
    hasher.finish()
}

impl ConfigSettings for Settings {
    type Scope = ();
    fn resolve(config: &ShellConfig, _: mandara_i18n::Lang, (): ()) -> Self { settings(config) }
}

thread_local! {
    static SETTINGS: RefCell<SettingsCache<Settings>> = const { RefCell::new(SettingsCache::new()) };
    static HUB: RefCell<Option<Rc<RefCell<Hub>>>> = const { RefCell::new(None) };
}

fn cached_settings(cx: &App) -> Rc<Settings> {
    let config = crate::state::config(cx).read(cx).snapshot();
    SETTINGS.with(|cache| cache.borrow_mut().get(&config, mandara_i18n::language(), ()))
}

/// An article the reader shows.
pub(super) struct Reading {
    pub url: String,
    pub link: String,
    /// `None` while its content loads from disk; `Some(None)` when none is stored.
    pub article: Option<Option<Rc<reader::Doc>>>,
}

/// What the panel shows besides the data.
#[derive(Default)]
pub(super) struct Ui {
    pub scope: Scope,
    pub filter: Filter,
    /// The keyboard selection: an index into the list on screen.
    pub selected: Option<usize>,
    pub reading: Option<Reading>,
    pub list_scroll: gpui::UniformListScrollHandle,
    pub reader_scroll: gpui::ScrollHandle,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Load { #[default] Idle, Loading, Loaded }

/// Everything Feeds holds while it is on.
#[derive(Default)]
pub(super) struct Hub {
    /// Bumped when the panel's data is released; older loads are rejected.
    generation: u64,
    /// Per-feed numbers for the bar (small; kept while the plugin is on).
    counts: HashMap<String, Counts>,
    counts_load: Load,
    /// The open panel's history.
    pub lib: Option<Library>,
    lib_load: Load,
    /// A refresh in flight: feeds done and total.
    pub refreshing: Option<(usize, usize)>,
    /// When the panel last checked which feeds are due.
    due_checked: Option<Instant>,
    pub ui: Ui,
    pub views: ViewSet,
    pub flash: Flash,
    pub images: images::Shared,
    background: Option<(Duration, Task<()>)>,
    release: Option<Task<()>>,
    notified: Option<Instant>,
    /// New unread articles since the last notification, and where.
    fresh: (usize, Vec<String>),
    /// Bumped whenever a snapshot changes (the sidebar card follows it).
    rev: u64,
    /// An article the sidebar card asked the panel to open.
    pending_open: Option<(String, String)>,
    card: card::CardData,
}

pub(super) fn hub() -> Rc<RefCell<Hub>> {
    HUB.with(|slot| slot.borrow_mut().get_or_insert_with(Rc::default).clone())
}

/// Drop everything Feeds holds (the plugin was switched off). Never
/// allocates a hub just to clear it.
pub(super) fn stop(cx: &mut App) {
    SETTINGS.with(|cache| cache.borrow_mut().clear());
    if let Some(hub) = HUB.with(|slot| slot.borrow_mut().take()) {
        let images = hub.borrow().images.clone();
        let had = images.borrow_mut().clear(|image| cx.drop_image(image, None));
        drop(hub);
        if had { mandara_services::memory::request_trim(); }
    }
}

/// The unread total for the bar, loading the counts once.
fn unread_total(cx: &mut App) -> (usize, bool) {
    let hub = hub();
    let settings = cached_settings(cx);
    let start = {
        let mut h = hub.borrow_mut();
        let start = h.counts_load == Load::Idle;
        if start { h.counts_load = Load::Loading; }
        start
    };
    if start {
        let urls: Vec<String> = settings.subs.iter().map(|sub| sub.url.clone()).collect();
        let hub = hub.clone();
        cx.spawn(async move |cx| {
            let counts = mandara_services::runtime::unblock(move || {
                let cache = Cache::default();
                urls.into_iter().filter_map(|url| { let counts = cache.load(&url).ok().flatten()?.counts(); Some((url, counts)) }).collect::<Vec<_>>()
            }).await;
            let views = {
                let mut h = hub.borrow_mut();
                h.counts_load = Load::Loaded;
                for (url, counts) in counts { h.counts.entry(url).or_insert(counts); }
                h.views.clone()
            };
            views.notify_async(cx);
        }).detach();
    }
    let h = hub.borrow();
    if let Some(lib) = &h.lib {
        return (lib.unread_in(&Scope::All), lib.feeds.iter().any(|row| row.error().is_some()));
    }
    let subscribed = |url: &String| settings.subs.iter().any(|sub| &sub.url == url);
    let total = h.counts.iter().filter(|(url, _)| subscribed(url)).map(|(_, counts)| counts.unread).sum();
    (total, h.counts.iter().any(|(url, counts)| subscribed(url) && counts.failing))
}

/// The panel became visible: cancel a pending release, load the history
/// once and refresh what is due.
fn opened(cx: &mut App) {
    let hub = hub();
    let settings = cached_settings(cx);
    let load = {
        let mut h = hub.borrow_mut();
        h.release = None;
        if h.lib.as_mut().is_some_and(|lib| lib.sync(&settings.subs)) { h.due_checked = None; }
        let load = h.lib.is_none() && h.lib_load != Load::Loading;
        if load { h.lib_load = Load::Loading; }
        load
    };
    if load {
        let generation = hub.borrow().generation;
        let subs = settings.subs.clone();
        let urls: Vec<String> = subs.iter().map(|sub| sub.url.clone()).collect();
        let hub = hub.clone();
        cx.spawn(async move |cx| {
            let snapshots = mandara_services::runtime::unblock(move || {
                let cache = Cache::default();
                urls.into_iter().filter_map(|url| cache.load(&url).ok().flatten()).collect::<Vec<_>>()
            }).await;
            cx.update(|cx| {
                let views = {
                    let mut h = hub.borrow_mut();
                    if h.generation != generation { return; }
                    let mut lib = Library::default();
                    lib.sync(&subs);
                    for snapshot in snapshots {
                        h.counts.insert(snapshot.feed.url.clone(), snapshot.counts());
                        lib.set(Arc::new(snapshot));
                    }
                    h.lib = Some(lib);
                    h.lib_load = Load::Loaded;
                    h.due_checked = None;
                    h.views.clone()
                };
                open_pending(cx);
                refresh_due(cx);
                views.notify(cx);
            });
        }).detach();
    } else {
        open_pending(cx);
        refresh_due(cx);
    }
}

/// Show the article the sidebar card asked for, once the history is loaded.
fn open_pending(cx: &mut App) {
    let hub = hub();
    let item = {
        let mut h = hub.borrow_mut();
        if h.lib.is_none() { return; }
        let Some((url, link)) = h.pending_open.take() else { return };
        h.lib.as_ref().and_then(|lib| lib.find(&url, &link))
    };
    if let Some(item) = item { open(item, cx); }
}

/// Refresh the feeds whose history is older than the refresh interval
/// (and that are not backing off), at most once per interval.
fn refresh_due(cx: &mut App) {
    let settings = cached_settings(cx);
    let hub = hub();
    let due: Vec<String> = {
        let mut h = hub.borrow_mut();
        if h.refreshing.is_some() || h.due_checked.is_some_and(|at| at.elapsed() < settings.refresh) { return; }
        let Some(lib) = &h.lib else { return };
        let now = chrono::Utc::now().timestamp();
        let age = settings.refresh.as_secs() as i64;
        let due = lib.feeds.iter().filter(|row| row.snapshot.as_ref().is_none_or(|snapshot| now - snapshot.refreshed >= age && !snapshot.backing_off(now))).map(|row| row.sub.url.clone()).collect();
        h.due_checked = Some(Instant::now());
        due
    };
    if !due.is_empty() { refresh(due, false, cx); }
}

/// Refresh every subscription now (the Refresh button, `feeds refresh`).
pub(super) fn refresh_all(cx: &mut App) {
    let settings = cached_settings(cx);
    refresh(settings.subs.iter().map(|sub| sub.url.clone()).collect(), true, cx);
}

/// One coalesced refresh: a few lanes share the list, each running one
/// download at a time on the shared blocking pool; results land as each
/// feed finishes. A second request while one runs is dropped.
fn refresh(urls: Vec<String>, force: bool, cx: &mut App) {
    if urls.is_empty() || !super::lifecycle::is_on(ID, cx) { return; }
    let hub = hub();
    let limits = cached_settings(cx).limits;
    {
        let mut h = hub.borrow_mut();
        if h.refreshing.is_some() { return; }
        h.refreshing = Some((0, urls.len()));
    }
    hub.borrow().views.clone().notify(cx);
    let queue = Rc::new(RefCell::new(std::collections::VecDeque::from(urls)));
    let lanes = Rc::new(Cell::new(REFRESH_LANES));
    for _ in 0..REFRESH_LANES {
        let (queue, lanes, hub) = (queue.clone(), lanes.clone(), hub.clone());
        cx.spawn(async move |cx| {
            loop {
                // Switching Feeds off ends the refresh after the current download.
                if !HUB.with(|slot| slot.borrow().as_ref().is_some_and(|current| Rc::ptr_eq(current, &hub))) { break; }
                let Some(url) = queue.borrow_mut().pop_front() else { break };
                let address = url.clone();
                let result = mandara_services::runtime::unblock(move || Cache::default().refresh(&HttpSource::default(), &address, None, limits, force)).await;
                cx.update(|cx| absorb(&hub, url, result, cx));
            }
            lanes.set(lanes.get() - 1);
            if lanes.get() == 0 { cx.update(|cx| finished(&hub, cx)); }
        }).detach();
    }
}

/// One feed's refresh landed: update the counts, the open library and
/// the notification tally.
fn absorb(hub: &Rc<RefCell<Hub>>, url: String, result: Result<Snapshot, String>, cx: &mut App) {
    let views = {
        let mut h = hub.borrow_mut();
        if let Some((done, _)) = &mut h.refreshing { *done += 1; }
        match result {
            Ok(snapshot) => {
                h.rev = h.rev.wrapping_add(1);
                h.counts.insert(url, snapshot.counts());
                if snapshot.fresh > 0 {
                    h.fresh.0 += snapshot.fresh;
                    let name = if snapshot.feed.title.is_empty() { host(&snapshot.feed.url).to_owned() } else { snapshot.feed.title.clone() };
                    if !h.fresh.1.contains(&name) { h.fresh.1.push(name); }
                }
                if let Some(lib) = &mut h.lib { lib.set(Arc::new(snapshot)); }
            }
            Err(error) => tracing::debug!("feeds: {url}: {error}"),
        }
        h.views.clone()
    };
    views.notify(cx);
}

/// The refresh is done: notify about new articles when that is on, the
/// panel is not already showing them and the last notice is old enough.
fn finished(hub: &Rc<RefCell<Hub>>, cx: &mut App) {
    let notify = cached_settings(cx).notify;
    let visible = crate::state::shell(cx).read(cx).panel.as_deref() == Some(ID);
    let notice = {
        let mut h = hub.borrow_mut();
        h.refreshing = None;
        let due = h.notified.is_none_or(|at| at.elapsed() >= NOTIFY_GAP);
        let notice = (notify && !visible && due && h.fresh.0 > 0).then(|| {
            let (count, names) = std::mem::take(&mut h.fresh);
            (count, names)
        });
        if visible { h.fresh = (0, Vec::new()); }
        if notice.is_some() { h.notified = Some(Instant::now()); }
        notice
    };
    if let Some((count, names)) = notice {
        let summary = tf("{count} new articles", &[("count", &count)]);
        let body = match names.as_slice() {
            [one] => one.clone(),
            [first, rest @ ..] => tf("{feed} and {more} more feeds", &[("feed", first), ("more", &rest.len())]),
            [] => String::new(),
        };
        super::notifications::send(cx, mandara_services::NotificationCommand::Post { app: t("Feeds").into(), icon: "rss".into(), summary, body });
    }
    hub.borrow().views.clone().notify(cx);
}

/// Keep the opt-in background refresh in step with its setting: one
/// timer, a low fixed interval, nothing when it is off.
fn follow_background(cx: &mut App) {
    let every = cached_settings(cx).background;
    let hub = hub();
    let current = hub.borrow().background.as_ref().map(|(every, _)| *every);
    if current == every { return; }
    let Some(every) = every else { hub.borrow_mut().background = None; return };
    let weak = Rc::downgrade(&hub);
    let task = cx.spawn(async move |cx| {
        let mut wait = BACKGROUND_FIRST_WAIT.min(every);
        loop {
            cx.background_executor().timer(wait).await;
            wait = every;
            if weak.upgrade().is_none() { return; }
            cx.update(|cx| {
                let settings = cached_settings(cx);
                let now = chrono::Utc::now().timestamp();
                // Coalesced with what is due: fresh feeds and backing-off ones wait.
                let urls: Vec<String> = settings.subs.iter().map(|sub| sub.url.clone()).collect();
                let Some(hub) = weak.upgrade() else { return };
                let due = {
                    let h = hub.borrow();
                    match &h.lib {
                        Some(lib) => lib.feeds.iter().filter(|row| row.snapshot.as_ref().is_none_or(|s| now - s.refreshed >= every.as_secs() as i64 / 2 && !s.backing_off(now))).map(|row| row.sub.url.clone()).collect(),
                        None => urls,
                    }
                };
                refresh(due, false, cx);
            });
        }
    });
    hub.borrow_mut().background = Some((every, task));
}

/// Follows the shell once per process (not once per build: sidebar cards
/// are built every time the sidebar opens): a closed panel releases its
/// history after its close animation.
fn follow_shell(cx: &mut App) {
    thread_local! { static FOLLOWING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
    if FOLLOWING.with(|f| f.replace(true)) { return; }
    cx.observe(&crate::state::shell(cx), |shell, cx| {
        // A floating Feeds panel is open too, though it isn't the shell's panel.
        if !panel_open(shell.read(cx).panel.as_deref(), crate::floating::is_floating(ID, cx)) { closed(cx); }
    }).detach();
}

/// The sidebar closed: the card's rows go (never building a hub for it).
fn release_card(_: &mut App) {
    if let Some(hub) = HUB.with(|slot| slot.borrow().clone()) {
        hub.borrow_mut().card.release();
    }
}

/// Whether the Feeds panel shows: as the shell's panel or floating.
fn panel_open(shell_panel: Option<&str>, floating: bool) -> bool {
    floating || shell_panel == Some(ID)
}

/// The panel closed: release its history, article and pictures after the
/// close animation. Counts stay for the bar.
fn closed(cx: &mut App) {
    let hub = hub();
    {
        let h = hub.borrow();
        if h.release.is_some() || (h.lib.is_none() && h.ui.reading.is_none() && h.images.borrow().is_empty()) { return; }
    }
    let weak = Rc::downgrade(&hub);
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(RELEASE_AFTER_CLOSE).await;
        let Some(hub) = weak.upgrade() else { return };
        cx.update(|cx| release(&hub, cx));
    });
    hub.borrow_mut().release = Some(task);
}

fn release(hub: &Rc<RefCell<Hub>>, cx: &mut App) {
    let images = {
        let mut h = hub.borrow_mut();
        h.release = None;
        h.generation = h.generation.wrapping_add(1);
        h.lib = None;
        h.lib_load = Load::Idle;
        h.due_checked = None;
        h.ui.reading = None;
        h.ui.selected = None;
        h.images.clone()
    };
    images.borrow_mut().clear(|image| cx.drop_image(image, None));
    mandara_services::memory::request_trim();
}

// ── Actions shared by the panel and commands ─────────────────────────────

/// Persist flags off the UI thread; the saved snapshot replaces the local one.
fn persist(url: String, cx: &mut App, change: impl FnOnce(&Cache, &str) -> Result<Snapshot, String> + Send + 'static) {
    let hub = hub();
    cx.spawn(async move |cx| {
        let address = url.clone();
        let result = mandara_services::runtime::unblock(move || change(&Cache::default(), &address)).await;
        cx.update(|cx| {
            let views = {
                let mut h = hub.borrow_mut();
                match result {
                    Ok(snapshot) => {
                        h.rev = h.rev.wrapping_add(1);
                        h.counts.insert(url, snapshot.counts());
                        if let Some(lib) = &mut h.lib { lib.set(Arc::new(snapshot)); }
                    }
                    Err(error) => h.flash.show(false, error),
                }
                h.views.clone()
            };
            views.notify(cx);
        });
    }).detach();
}

/// Mark one article read or unread.
pub(super) fn set_read(item: Item, read: bool, cx: &mut App) {
    let Some((url, link)) = with_entry(item, |row, entry| (row.sub.url.clone(), entry.link.clone())) else { return };
    if let Some(lib) = &mut hub().borrow_mut().lib { lib.flag(item, Some(read), None); }
    persist(url, cx, move |cache, url| cache.set_read(url, &[&link], read));
}

/// Star or unstar one article.
pub(super) fn set_starred(item: Item, starred: bool, cx: &mut App) {
    let Some((url, link)) = with_entry(item, |row, entry| (row.sub.url.clone(), entry.link.clone())) else { return };
    if let Some(lib) = &mut hub().borrow_mut().lib { lib.flag(item, None, Some(starred)); }
    persist(url, cx, move |cache, url| cache.set_starred(url, &link, starred));
}

/// Mark everything in the selected scope read.
pub(super) fn mark_scope_read(cx: &mut App) {
    let touched = {
        let hub = hub();
        let mut h = hub.borrow_mut();
        let scope = h.ui.scope.clone();
        match &mut h.lib { Some(lib) => lib.mark_scope_read(&scope), None => Vec::new() }
    };
    for (url, links) in touched {
        persist(url, cx, move |cache, url| cache.set_read(url, &links.iter().map(String::as_str).collect::<Vec<_>>(), true));
    }
}

fn with_entry<T>(item: Item, f: impl FnOnce(&library::FeedRow, &mandara_services::feeds::Entry) -> T) -> Option<T> {
    let hub = hub();
    let h = hub.borrow();
    let (row, entry) = h.lib.as_ref()?.entry(item)?;
    Some(f(row, entry))
}

/// Open an article in the browser (only web addresses are handed over).
pub(super) fn open_in_browser(link: &str) -> Result<(), String> {
    if !mandara_services::feeds::web_url(link) { return Err(t("This article has no web address").into()); }
    mandara_services::session::open_uri(link).map_err(|error| error.to_string())
}

/// Show an article: in the reader (loading its stored content off the
/// UI thread) or the browser, as configured.
pub(super) fn open(item: Item, cx: &mut App) {
    let settings = cached_settings(cx);
    let Some((url, link, read)) = with_entry(item, |row, entry| (row.sub.url.clone(), entry.link.clone(), row.snapshot.as_ref().is_some_and(|s| s.read.contains(&entry.link)))) else { return };
    if settings.open_in_browser {
        match open_in_browser(&link) {
            Ok(()) => if !read { set_read(item, true, cx); },
            Err(error) => hub().borrow().flash.show(false, error),
        }
        return;
    }
    if settings.mark_read_on_open && !read { set_read(item, true, cx); }
    let hub = hub();
    {
        let mut h = hub.borrow_mut();
        h.ui.reading = Some(Reading { url: url.clone(), link: link.clone(), article: None });
        h.ui.reader_scroll = gpui::ScrollHandle::new();
    }
    let generation = hub.borrow().generation;
    hub.borrow().views.clone().notify(cx);
    cx.spawn(async move |cx| {
        let (address, article_link) = (url.clone(), link.clone());
        let article = mandara_services::runtime::unblock(move || Cache::default().article(&address, &article_link).ok().flatten().map(|article| reader::Doc::from_article(&article))).await;
        cx.update(|cx| {
            let views = {
                let mut h = hub.borrow_mut();
                if h.generation != generation { return; }
                match &mut h.ui.reading {
                    Some(reading) if reading.url == url && reading.link == link => reading.article = Some(article.map(Rc::new)),
                    _ => return,
                }
                h.views.clone()
            };
            views.notify(cx);
        });
    }).detach();
}

// ── Registration ─────────────────────────────────────────────────────────

fn snapshot_json(snapshot: &Snapshot) -> serde_json::Value {
    serde_json::json!({
        "url": snapshot.feed.url, "title": snapshot.feed.title, "refreshed": snapshot.refreshed,
        "unread": snapshot.unread(), "saved": snapshot.starred.len(), "error": snapshot.error, "retry_at": (snapshot.retry_at > 0).then_some(snapshot.retry_at),
        "entries": snapshot.feed.entries.iter().map(|entry| serde_json::json!({
            "title": entry.title, "link": entry.link, "published": entry.published, "author": entry.author, "summary": entry.summary,
            "read": snapshot.read.contains(&entry.link), "saved": snapshot.starred.contains(&entry.link),
        })).collect::<Vec<_>>(),
    })
}

/// Run blocking work for a command, then finish on the UI thread.
fn command(work: impl FnOnce() -> Result<serde_json::Value, String> + Send + 'static, then: impl FnOnce(&serde_json::Value, &mut App) + 'static, cx: &mut App) -> Output {
    let (tx, rx) = async_channel::bounded(1);
    cx.spawn(async move |cx| {
        let result = mandara_services::runtime::unblock(work).await;
        if let Ok(value) = &result { cx.update(|cx| then(value, cx)); }
        let _ = tx.send(result.map(|value| serde_json::to_string_pretty(&value).unwrap_or_default())).await;
    }).detach();
    Output::Pending(rx)
}

/// After a command changed a feed's flags on disk, reload it if the panel is open.
fn reload(url: String, cx: &mut App) { persist(url, cx, |cache, url| cache.load(url)?.ok_or_else(|| t("Refresh this feed before marking it read").to_string())); }

/// Register commands and lazy views without reading feeds during startup.
pub fn register(registry: &mut PluginRegistry) {
    registry.register(PluginManifest::builtin(ID, tk("Feeds"), tk("A reader for RSS, Atom and JSON feeds: one timeline, folders, saved articles, search and a clean reader view"), Some(Zone::Right)).opt_in(), |window, cx, context| {
        follow_background(cx);
        follow_shell(cx);
        let sidebar = super::model_view(None, card::render, window, cx);
        if context.is_card() {
            return PluginInstance { sidebar: Some(sidebar), ..PluginInstance::empty() };
        }
        let bar = super::model_view(None, panel::bar, window, cx);
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(PANEL_H))));
        let view = panel::View::new(context.clone(), cx);
        let panel = super::model_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), sidebar: Some(sidebar), ..PluginInstance::empty() }
    });
    // The card's rows live in the hub: they go when the sidebar closes.
    registry.sidebar_card(ID, mandara_plugins::CardRelease::shared(release_card));
    registry.command(ID, CommandSpec::new("feeds list", tk("Your subscriptions with their folders and unread counts"), |_, cx| {
        let counts = hub().borrow().counts.clone();
        let list: Vec<serde_json::Value> = cached_settings(cx).subs.iter().map(|sub| {
            let counts = counts.get(&sub.url).copied().unwrap_or_default();
            serde_json::json!({ "url": sub.url, "title": sub.title, "folder": sub.folder, "color": sub.color, "unread": counts.unread, "saved": counts.starred, "failing": counts.failing })
        }).collect();
        Ok(serde_json::Value::Array(list))
    }));
    registry.command(ID, CommandSpec::new_output("feeds refresh", tk("Refresh a feed now and return its articles, or every feed when no address is given"), |args, cx| {
        let Some(url) = args.get(0).map(str::to_owned) else {
            refresh_all(cx);
            return Ok(Output::Value(serde_json::json!({ "refreshing": cached_settings(cx).subs.len() })));
        };
        let limits = cached_settings(cx).limits;
        Ok(command(move || Cache::default().refresh(&HttpSource::default(), &url, None, limits, true).map(|snapshot| snapshot_json(&snapshot)), |value, cx| {
            if let Some(url) = value["url"].as_str() { reload(url.to_owned(), cx); }
        }, cx))
    }).arg(ArgSpec::optional("url", ArgKind::String, tk("RSS, Atom or JSON feed address"))).example("feeds refresh https://example.org/feed.xml"));
    registry.command(ID, CommandSpec::new_output("feeds discover", tk("Find the feeds a website offers"), |args, cx| {
        let url = args.str(0)?.to_owned();
        Ok(command(move || mandara_services::feeds::discover(&url).map(|found| serde_json::to_value(found).unwrap_or_default()), |_, _| {}, cx))
    }).arg(ArgSpec::required("url", ArgKind::String, tk("A website or feed address"))).example("feeds discover example.org").works_when_off());
    registry.command(ID, CommandSpec::new_output("feeds subscribe", tk("Subscribe to a feed, or to the feed a website offers (turns Feeds on)"), |args, cx| {
        let url = args.str(0)?.to_owned();
        let folder = args.rest(1);
        Ok(command(move || {
            let found = mandara_services::feeds::discover(&url)?;
            let first = found.into_iter().next().ok_or_else(|| t("No feed found at this address").to_string())?;
            Ok(serde_json::json!({ "url": first.url, "title": first.title }))
        }, move |value, cx| {
            let Some(url) = value["url"].as_str().map(str::to_owned) else { return };
            let title = value["title"].as_str().unwrap_or_default().to_owned();
            crate::state::update_config(cx, |c| {
                let mut list = subscriptions(c);
                if !list.iter().any(|sub| sub.url == url) { list.push(Subscription { url, title, folder: folder.trim().to_owned(), color: String::new() }); }
                set_subscriptions(c, &list);
            });
            hub().borrow_mut().due_checked = None;
            // Choosing to subscribe is choosing Feeds.
            if !super::lifecycle::is_on(ID, cx) && let Err(error) = super::lifecycle::apply(ID, super::lifecycle::Action::Enable, cx) { tracing::warn!("feeds: {error}"); }
        }, cx))
    }).arg(ArgSpec::required("url", ArgKind::String, tk("A website or feed address"))).arg(ArgSpec::optional("folder", ArgKind::String, tk("Folder to put it in"))).example("feeds subscribe https://blog.rust-lang.org").works_when_off());
    registry.command(ID, CommandSpec::new("feeds unsubscribe", tk("Remove a subscription (its saved history stays until you subscribe again)"), |args, cx| {
        let url = args.str(0)?.to_owned();
        let before = subscriptions(&crate::state::config(cx).read(cx).snapshot()).len();
        crate::state::update_config(cx, |c| { let list: Vec<Subscription> = subscriptions(c).into_iter().filter(|sub| sub.url != url).collect(); set_subscriptions(c, &list); });
        Ok(serde_json::json!({ "removed": before > subscriptions(&crate::state::config(cx).read(cx).snapshot()).len() }))
    }).arg(ArgSpec::required("url", ArgKind::String, tk("The feed address"))));
    registry.command(ID, CommandSpec::new_output("feeds items", tk("Saved articles of one feed or every feed, newest first, with their read and saved flags"), |args, cx| {
        let urls: Vec<String> = match args.get(0) { Some(url) => vec![url.to_owned()], None => cached_settings(cx).subs.iter().map(|sub| sub.url.clone()).collect() };
        Ok(command(move || {
            let cache = Cache::default();
            Ok(serde_json::Value::Array(urls.iter().filter_map(|url| cache.load(url).ok().flatten()).map(|snapshot| snapshot_json(&snapshot)).collect()))
        }, |_, _| {}, cx))
    }).arg(ArgSpec::optional("url", ArgKind::String, tk("The feed address"))));
    let flag = |path: &'static str, summary: &'static str, apply: fn(&Cache, &str, &str) -> Result<Snapshot, String>| {
        CommandSpec::new_output(path, summary, move |args, cx| {
            let (url, link) = (args.str(0)?.to_owned(), args.str(1)?.to_owned());
            Ok(command(move || apply(&Cache::default(), &url, &link).map(|snapshot| snapshot_json(&snapshot)), |value, cx| {
                if let Some(url) = value["url"].as_str() { reload(url.to_owned(), cx); }
            }, cx))
        }).arg(ArgSpec::required("url", ArgKind::String, tk("The feed address"))).arg(ArgSpec::required("link", ArgKind::String, tk("The article's link")))
    };
    registry.command(ID, flag("feeds read", tk("Mark an article read"), |cache, url, link| cache.set_read(url, &[link], true)));
    registry.command(ID, flag("feeds unread", tk("Mark an article unread"), |cache, url, link| cache.set_read(url, &[link], false)));
    registry.command(ID, flag("feeds save", tk("Save an article to read it later"), |cache, url, link| cache.set_starred(url, link, true)));
    registry.command(ID, flag("feeds unsave", tk("Remove an article from your saved ones"), |cache, url, link| cache.set_starred(url, link, false)));
    registry.command(ID, CommandSpec::new_output("feeds read-all", tk("Mark every article of a feed read, or of every feed when no address is given"), |args, cx| {
        let urls: Vec<String> = match args.get(0) { Some(url) => vec![url.to_owned()], None => cached_settings(cx).subs.iter().map(|sub| sub.url.clone()).collect() };
        Ok(command(move || {
            let cache = Cache::default();
            let done = urls.iter().filter(|url| cache.mark_all_read(url).is_ok()).count();
            Ok(serde_json::json!({ "feeds": done, "urls": urls }))
        }, |value, cx| {
            for url in value["urls"].as_array().into_iter().flatten().filter_map(serde_json::Value::as_str) { reload(url.to_owned(), cx); }
        }, cx))
    }).arg(ArgSpec::optional("url", ArgKind::String, tk("The feed address"))));
    registry.command(ID, CommandSpec::new_output("feeds import", tk("Add the subscriptions of an OPML file (from another feed reader)"), |args, cx| {
        let path = expand_home(&args.rest(0));
        Ok(command(move || {
            let text = read_opml(&path)?;
            serde_json::to_value(mandara_services::feeds::opml::import(&text)?).map_err(|error| error.to_string())
        }, |value, cx| {
            let Ok(found) = serde_json::from_value::<Vec<Subscription>>(value.clone()) else { return };
            crate::state::update_config(cx, |c| {
                let mut list = subscriptions(c);
                for sub in found { if !list.iter().any(|seen| seen.url == sub.url) { list.push(sub); } }
                set_subscriptions(c, &list);
            });
        }, cx))
    }).arg(ArgSpec::required("path", ArgKind::String, tk("The OPML file"))).example("feeds import ~/Downloads/subscriptions.opml").works_when_off());
    registry.command(ID, CommandSpec::new_output("feeds export", tk("Save your subscriptions as an OPML file"), |args, cx| {
        let path = expand_home(&args.rest(0));
        let subs: Vec<Subscription> = cached_settings(cx).subs.to_vec();
        Ok(command(move || {
            std::fs::write(&path, mandara_services::feeds::opml::export(&subs)).map_err(|error| error.to_string())?;
            Ok(serde_json::json!({ "path": path, "feeds": subs.len() }))
        }, |_, _| {}, cx))
    }).arg(ArgSpec::required("path", ArgKind::String, tk("The OPML file"))).example("feeds export ~/feeds.opml").works_when_off());
}

/// A typed file path with `~` for your home folder.
pub fn expand_home(path: &str) -> std::path::PathBuf {
    let path = path.trim();
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None if path == "~" => dirs::home_dir().unwrap_or_default(),
        None => std::path::PathBuf::from(path),
    }
}

/// An OPML file's text, bounded.
pub fn read_opml(path: &std::path::Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|error| error.to_string())?;
    if meta.len() as usize > mandara_services::feeds::opml::MAX_IMPORT_BYTES { return Err(t("This OPML file is too large").into()); }
    std::fs::read_to_string(path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(entries: &[(&str, toml::Value)]) -> ShellConfig {
        let mut config = ShellConfig::default();
        let table = config.plugins.settings.entry(ID.into()).or_default();
        for (key, value) in entries { table.insert((*key).into(), value.clone()); }
        config
    }

    /// Lists from before folders and titles keep working unchanged.
    #[test]
    fn older_space_separated_subscriptions_still_load() {
        let cfg = with(&[("subscriptions", toml::Value::String("https://one.example/feed\nhttps://two.example/feed https://one.example/feed file:///etc/passwd".into()))]);
        assert_eq!(subscriptions(&cfg).iter().map(|s| s.url.as_str()).collect::<Vec<_>>(), ["https://one.example/feed", "https://two.example/feed"]);
        assert!(subscriptions(&with(&[("subscriptions", toml::Value::Integer(3))])).is_empty());
    }

    #[test]
    fn the_feeds_list_round_trips_and_replaces_the_older_text() {
        let mut cfg = with(&[("subscriptions", toml::Value::String("https://old.example/feed".into()))]);
        let list = vec![
            Subscription { url: "https://a.example/feed".into(), title: "A".into(), folder: "Tech".into(), color: "#ff8800".into() },
            Subscription { url: "https://b.example/feed".into(), ..Subscription::default() },
        ];
        set_subscriptions(&mut cfg, &list);
        assert!(cfg.plugins.settings[ID].get("subscriptions").is_none());
        assert_eq!(subscriptions(&cfg), list);
        let text = toml::to_string(&cfg.plugins.settings[ID]).unwrap();
        assert!(text.contains("[[feeds]]") && !text.contains("title = \"\""), "{text}");
        let many: Vec<Subscription> = (0..MAX_SUBSCRIPTIONS + 5).map(|n| Subscription { url: format!("https://{n}.example/feed"), ..Subscription::default() }).collect();
        set_subscriptions(&mut cfg, &many);
        assert_eq!(subscriptions(&cfg).len(), MAX_SUBSCRIPTIONS);
    }

    /// The card's rows follow subscribing, unsubscribing and moving or
    /// renaming a feed even when no snapshot changed.
    #[test]
    fn the_subscriptions_fingerprint_follows_every_change_the_card_shows() {
        let sub = |url: &str, folder: &str| Subscription { url: url.into(), folder: folder.into(), ..Subscription::default() };
        let base = [sub("https://a.example/feed", ""), sub("https://b.example/feed", "News")];
        let key = fingerprint(&base);
        assert_eq!(key, fingerprint(&base.clone()), "stable");
        assert_ne!(key, fingerprint(&base[..1]), "unsubscribed");
        assert_ne!(key, fingerprint(&[base[0].clone(), base[1].clone(), sub("https://c.example/feed", "")]), "subscribed");
        assert_ne!(key, fingerprint(&[base[0].clone(), sub("https://b.example/feed", "Tech")]), "moved to another folder");
        assert_ne!(key, fingerprint(&[base[0].clone(), Subscription { title: "Bee".into(), ..base[1].clone() }]), "renamed");
        let mut cfg = ShellConfig::default();
        set_subscriptions(&mut cfg, &base);
        assert_eq!(settings(&cfg).subs_key, key, "resolved with the settings");
    }

    #[test]
    fn a_floating_panel_counts_as_open() {
        assert!(panel_open(Some(ID), false));
        assert!(panel_open(None, true), "floating, not the shell's panel");
        assert!(panel_open(Some("weather"), true));
        assert!(!panel_open(Some("weather"), false));
        assert!(!panel_open(None, false));
    }

    #[test]
    fn settings_clamp_and_default() {
        let cfg = with(&[("refresh_seconds", toml::Value::Integer(0)), ("items_per_feed", toml::Value::Integer(100_000)), ("images", toml::Value::String("all".into())), ("bar_style", toml::Value::String("count".into())), ("background_refresh", toml::Value::Boolean(true)), ("background_minutes", toml::Value::Integer(1))]);
        let s = settings(&cfg);
        assert_eq!(s.refresh, Duration::from_secs(MIN_REFRESH_SECS as u64));
        assert_eq!(s.limits.items, MAX_ITEMS as usize);
        assert_eq!(s.pictures, Pictures::All);
        assert_eq!(s.bar, BarStyle::Count);
        assert_eq!(s.background, Some(Duration::from_secs(MIN_BACKGROUND_MIN as u64 * SECS_PER_MIN)), "background refresh is never frequent");
        let d = settings(&ShellConfig::default());
        assert_eq!((d.pictures, d.bar, d.background, d.notify, d.mark_read_on_open, d.open_in_browser), (Pictures::Thumbnails, BarStyle::Badge, None, false, true, false));
    }

    #[test]
    fn stopping_never_constructs_a_hub_and_drops_the_existing_one() {
        HUB.with(|slot| slot.borrow_mut().take());
        let hub = hub();
        let weak = Rc::downgrade(&hub);
        drop(hub);
        HUB.with(|slot| slot.borrow_mut().take());
        assert!(weak.upgrade().is_none());
        assert!(HUB.with(|slot| slot.borrow().is_none()));
    }
}
