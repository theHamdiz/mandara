//! Radio plugin: internet radio with curated, categorised stations, your
//! own stations and favourites, a now-playing marquee in the bar, a panel
//! with a large now-playing card, transport, volume, sleep timer and a
//! station browser, a sidebar card, and the radio-browser.info directory
//! on request.
//!
//! Playback lives in [`mandara_services::radio`]: its threads exist only
//! while a station plays, and it serves an MPRIS player so media keys, the
//! media panel and the lock screen follow it. This module holds the
//! model (made on first use, never at startup unless "resume on login" is
//! on), the settings (`[plugins.settings.radio]`) and the commands.
//! Switching the plugin off stops playback and drops the model.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gpui::{App, AppContext as _, Global, Task};
use mandara_i18n::{Lang, t, tf, tk, tr};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::radio::{self, Category, Library, Player, Request, Status, directory, library::EditError, player, station};
use serde_json::{Value, json};

use super::panel_fx::ViewSet;
use super::settings_cache::{ConfigSettings, SettingsCache};
use super::fn_view;
use crate::state::config;

pub(crate) mod art;
mod bar;
mod panel;

pub const ID: &str = "radio";

/// Changed favourites, stations and volumes are written this long after
/// the last change (slider drags coalesce).
const SAVE_DELAY: Duration = Duration::from_millis(800);
/// Directory results are kept this long for the same query, then freed.
const DIRECTORY_TTL: Duration = Duration::from_secs(600);
/// Player requests (MPRIS next / previous / raise) queued for the UI.
const REQUESTS: usize = 4;
const PERCENT: f64 = 100.0;
const SECS_PER_MIN: u64 = 60;
/// Sleep timer choices offered (minutes).
pub(crate) const SLEEP_CHOICES: [u64; 5] = [15, 30, 45, 60, 90];

// ── Settings ─────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum BarStyle {
    /// Station art, live dot and the now-playing marquee.
    Full,
    /// Station art and live dot.
    Compact,
    /// The radio glyph alone.
    Icon,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ArtStyle {
    Vivid,
    Soft,
    /// Follows the theme's accent instead of the station's colour.
    Theme,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    /// The play button starts your first favourite (else the last one
    /// played) when no station was chosen yet.
    pub usual_favourite: bool,
    pub resume_on_login: bool,
    pub options: player::Options,
    pub per_station_volume: bool,
    pub notify: bool,
    pub mpris: bool,
    pub bar_style: BarStyle,
    /// Marquee speed in px/s (0: no scrolling).
    pub marquee_px_s: f32,
    pub bar_width: f32,
    pub art: ArtStyle,
    pub meter: bool,
    pub details: bool,
    /// Shelves shown, by [`Category::index`].
    pub categories: [bool; station::CATEGORIES],
    pub directory: bool,
    pub directory_limit: usize,
    pub sleep_minutes: u64,
}

/// Marquee speeds (px/s) by setting value.
const MARQUEE_SPEEDS: [(&str, f32); 4] = [("off", 0.0), ("slow", 18.0), ("normal", 30.0), ("fast", 48.0)];
/// The bar width setting's bounds and default (px).
pub(crate) const BAR_WIDTH_RANGE: (f32, f32) = (80.0, 320.0);
const DEFAULT_BAR_WIDTH: f32 = 180.0;

impl ConfigSettings for Settings {
    type Scope = ();

    fn resolve(cfg: &mandara_config::ShellConfig, _: Lang, (): ()) -> Self {
        let s = &cfg.plugins;
        let mut categories = [true; station::CATEGORIES];
        if let Some(list) = s.settings.get(ID).and_then(|t| t.get("categories")).and_then(|v| v.as_array()) {
            categories = [false; station::CATEGORIES];
            for c in list.iter().filter_map(|v| v.as_str()).filter_map(Category::parse) {
                categories[c.index()] = true;
            }
            // Stations you added always have a shelf.
            categories[Category::Other.index()] = true;
        }
        let attempts: u32 = s.setting(ID, "reconnect_attempts", radio::backoff::DEFAULT_ATTEMPTS);
        let buffer: f32 = s.setting(ID, "buffer_seconds", player::DEFAULT_BUFFER_S);
        Settings {
            usual_favourite: s.setting_str(ID, "usual") == Some("favourite"),
            resume_on_login: s.setting(ID, "resume_on_login", false),
            options: player::Options {
                buffer_seconds: buffer.clamp(player::BUFFER_RANGE.0, player::BUFFER_RANGE.1),
                auto_reconnect: s.setting(ID, "auto_reconnect", true),
                attempts: attempts.clamp(radio::backoff::ATTEMPTS_RANGE.0, radio::backoff::ATTEMPTS_RANGE.1),
            },
            per_station_volume: s.setting(ID, "per_station_volume", true),
            notify: s.setting(ID, "notify_now_playing", false),
            mpris: s.setting(ID, "media_controls", true),
            bar_style: match s.setting_str(ID, "bar_style").unwrap_or("full") {
                "compact" => BarStyle::Compact,
                "icon" => BarStyle::Icon,
                _ => BarStyle::Full,
            },
            marquee_px_s: {
                let key = s.setting_str(ID, "marquee_speed").unwrap_or("normal");
                MARQUEE_SPEEDS.iter().find(|(k, _)| *k == key).map_or(MARQUEE_SPEEDS[2].1, |(_, v)| *v)
            },
            bar_width: s.setting(ID, "bar_width", DEFAULT_BAR_WIDTH).clamp(BAR_WIDTH_RANGE.0, BAR_WIDTH_RANGE.1),
            art: match s.setting_str(ID, "art_style").unwrap_or("vivid") {
                "soft" => ArtStyle::Soft,
                "theme" => ArtStyle::Theme,
                _ => ArtStyle::Vivid,
            },
            meter: s.setting(ID, "show_meter", true),
            details: s.setting(ID, "show_details", true),
            categories,
            directory: s.setting(ID, "directory", false),
            directory_limit: s.setting(ID, "directory_limit", directory::DEFAULT_RESULTS).clamp(1, directory::MAX_RESULTS),
            sleep_minutes: s.setting(ID, "sleep_minutes", SLEEP_CHOICES[1]).clamp(1, 24 * SECS_PER_MIN),
        }
    }
}

thread_local! {
    static SETTINGS: RefCell<SettingsCache<Settings>> = const { RefCell::new(SettingsCache::new()) };
    /// "Resume on login" ran (once per shell, not per bar).
    static RESUMED: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn settings(cx: &App) -> Rc<Settings> {
    let config = config(cx).read(cx).snapshot();
    SETTINGS.with(|c| c.borrow_mut().get(&config, mandara_i18n::language(), ()))
}

// ── Model ────────────────────────────────────────────────────────────────

/// Which list the panel shows.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Tab {
    #[default]
    Favourites,
    Stations,
    Add,
}

/// The station editor (adding by address or changing one of yours).
#[derive(Default)]
pub(crate) struct Editor {
    /// The station being changed (`None`: a new one).
    pub editing: Option<String>,
    pub category: Option<Category>,
    /// A test connection is running (its generation).
    pub testing: Option<u64>,
    pub generation: u64,
    /// The last test: what it found, or why it failed.
    pub result: Option<Result<String, String>>,
}

/// Directory search state (only while the directory is on).
#[derive(Default)]
pub(crate) struct Directory {
    pub query: String,
    pub results: Vec<directory::Found>,
    pub busy: bool,
    pub error: Option<String>,
    pub generation: u64,
    /// Frees the results after [`DIRECTORY_TTL`].
    expiry: Option<Task<()>>,
}

pub(crate) struct Model {
    pub player: Player,
    /// Your stations, once read from disk (off the UI thread); taken out
    /// while a change runs.
    pub lib: Option<Library>,
    lib_loaded: bool,
    /// Changes asked for before the library was read.
    pending: Vec<Box<dyn FnOnce(&mut Library, &Player, &mut App)>>,
    pub views: ViewSet,
    save_timer: Option<Task<()>>,
    pub tab: Tab,
    pub category: Option<Category>,
    pub manage: bool,
    pub editor: Editor,
    pub directory: Directory,
    /// The title last announced (no repeats).
    announced: Option<String>,
    /// The station the library last recorded as played.
    recorded: Option<String>,
}

pub(crate) type Shared = Rc<RefCell<Model>>;

struct GlobalRadio(Shared);

impl Global for GlobalRadio {}

/// The model, if something made it already (never creates it).
pub(crate) fn existing(cx: &App) -> Option<Shared> {
    cx.try_global::<GlobalRadio>().map(|g| g.0.clone())
}

/// The model, made on first use: the player (no threads yet) and a
/// background read of the library.
pub(crate) fn model(cx: &mut App) -> Shared {
    if let Some(m) = existing(cx) {
        return m;
    }
    let (changed_tx, changed_rx) = async_channel::bounded::<()>(1);
    let (request_tx, request_rx) = async_channel::bounded::<Request>(REQUESTS);
    let player = Player::new(
        move || {
            let _ = changed_tx.try_send(());
        },
        move |r| {
            let _ = request_tx.try_send(r);
        },
    );
    let s = settings(cx);
    player.set_options(s.options, s.mpris);
    let shared: Shared = Rc::new(RefCell::new(Model {
        player,
        lib: None,
        lib_loaded: false,
        pending: Vec::new(),
        views: ViewSet::default(),
        save_timer: None,
        tab: Tab::default(),
        category: None,
        manage: false,
        editor: Editor::default(),
        directory: Directory::default(),
        announced: None,
        recorded: None,
    }));
    cx.set_global(GlobalRadio(shared.clone()));
    let weak = Rc::downgrade(&shared);
    let load = cx.background_spawn(async move { Library::load(&Library::path()) });
    cx.spawn(async move |cx| {
        let lib = load.await;
        let Some(shared) = weak.upgrade() else { return };
        cx.update(|cx| loaded(&shared, lib, cx));
    })
    .detach();
    // The player's reports, as they come (coalesced; nothing polls).
    let weak = Rc::downgrade(&shared);
    cx.spawn(async move |cx| {
        while changed_rx.recv().await.is_ok() {
            let Some(shared) = weak.upgrade() else { break };
            cx.update(|cx| on_player(&shared, cx));
        }
    })
    .detach();
    let weak = Rc::downgrade(&shared);
    cx.spawn(async move |cx| {
        while let Ok(r) = request_rx.recv().await {
            let Some(shared) = weak.upgrade() else { break };
            cx.update(|cx| on_request(&shared, r, cx));
        }
    })
    .detach();
    shared
}

fn loaded(shared: &Shared, mut lib: Library, cx: &mut App) {
    let pending = {
        let mut m = shared.borrow_mut();
        m.player.set_volume(lib.volume);
        m.player.set_muted(lib.muted);
        std::mem::take(&mut m.pending)
    };
    let player = shared.borrow().player.clone();
    let dirty = !pending.is_empty();
    for f in pending {
        f(&mut lib, &player, cx);
    }
    {
        let mut m = shared.borrow_mut();
        m.lib = Some(lib);
        m.lib_loaded = true;
    }
    if dirty {
        schedule_save(shared, cx);
    }
    changed(shared, cx);
}

/// Runs `f` with the library now, or once it has been read (or, when
/// called from inside another change, right after it).
fn with_lib(cx: &mut App, f: impl FnOnce(&mut Library, &Player, &mut App) + 'static) {
    let shared = model(cx);
    let (lib, player) = {
        let mut m = shared.borrow_mut();
        if !m.lib_loaded {
            m.pending.push(Box::new(f));
            return;
        }
        match m.lib.take() {
            Some(lib) => (lib, m.player.clone()),
            None => {
                drop(m);
                cx.defer(move |cx| with_lib(cx, f));
                return;
            }
        }
    };
    let mut lib = lib;
    f(&mut lib, &player, cx);
    shared.borrow_mut().lib = Some(lib);
    schedule_save(&shared, cx);
    changed(&shared, cx);
}

/// Every view showing the radio redraws.
pub(crate) fn changed(shared: &Shared, cx: &mut App) {
    let views = shared.borrow().views.clone();
    views.notify(cx);
}

/// Save generations, process-wide like [`WRITTEN`]: a model made after
/// Radio is switched off and on again keeps counting up, so its saves win.
static SAVE_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// The newest generation written to disk.
static WRITTEN: std::sync::Mutex<u64> = std::sync::Mutex::new(0);

fn next_save_generation() -> u64 {
    SAVE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

/// Writes `lib` to `path` unless a newer save already landed (off the UI thread).
fn write_library(lib: &Library, generation: u64, path: &std::path::Path) {
    let mut last = WRITTEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if generation > *last {
        if let Err(e) = lib.save(path) {
            tracing::warn!("radio: saving stations: {e}");
        }
        *last = generation;
    }
}

/// Writes the library off the UI thread [`SAVE_DELAY`] after the last
/// change (the newest change wins).
fn schedule_save(shared: &Shared, cx: &mut App) {
    let weak = Rc::downgrade(shared);
    let timer = cx.spawn(async move |cx| {
        cx.background_executor().timer(SAVE_DELAY).await;
        let Some(shared) = weak.upgrade() else { return };
        let lib = {
            let mut m = shared.borrow_mut();
            m.save_timer = None;
            m.lib.clone()
        };
        let Some(lib) = lib else { return };
        let generation = next_save_generation();
        cx.background_spawn(async move { write_library(&lib, generation, &Library::path()) }).detach();
    });
    shared.borrow_mut().save_timer = Some(timer);
}

fn on_player(shared: &Shared, cx: &mut App) {
    let s = settings(cx);
    let (player, title, id, status) = {
        let m = shared.borrow();
        let (title, id, status) = m.player.with(|snap| (snap.title.clone(), snap.tune.as_ref().map(|t| t.id.clone()), snap.status));
        (m.player.clone(), title, id, status)
    };
    // The library follows what played and volume changes made elsewhere
    // (media keys, other apps' controls).
    let record = {
        let m = shared.borrow();
        let played = status == Status::Playing && id.is_some() && m.recorded != id;
        let volume = m.lib.as_ref().is_some_and(|lib| {
            let want = match id.as_deref() {
                Some(id) if s.per_station_volume => lib.volume_of(id),
                _ => lib.volume,
            };
            (want - player.volume()).abs() > f32::EPSILON || lib.muted != player.muted()
        });
        (played, volume)
    };
    if record.0 || record.1 {
        if let Some(lib) = shared.borrow_mut().lib.as_mut() {
            if record.0
                && let Some(id) = id.as_deref()
                && lib.station(id).is_some()
            {
                lib.played(id);
            }
            if record.1 {
                lib.set_volume(id.as_deref(), player.volume(), s.per_station_volume);
                lib.muted = player.muted();
            }
        }
        if record.0 {
            shared.borrow_mut().recorded = id.clone();
        }
        schedule_save(shared, cx);
    }
    if status == Status::Stopped {
        shared.borrow_mut().recorded = None;
    }
    // A now-playing notification for each new title (when asked for).
    if s.notify && status == Status::Playing && title.is_some() && shared.borrow().announced != title {
        shared.borrow_mut().announced = title.clone();
        let station = player.with(|snap| snap.tune.as_ref().map(|t| t.name.clone())).unwrap_or_default();
        if let Some(title) = title {
            super::notifications::send(cx, mandara_services::notifications::NotificationCommand::Post { app: t("Radio").into(), icon: "radio".into(), summary: title, body: station });
        }
    }
    changed(shared, cx);
}

fn on_request(shared: &Shared, r: Request, cx: &mut App) {
    match r {
        Request::Next => step(shared, 1, cx),
        Request::Previous => step(shared, -1, cx),
        Request::Raise => {
            let commands = cx.global::<PluginRegistry>().commands.clone();
            let words = ["panel".to_string(), "open".to_string(), ID.to_string()];
            if let Err(e) = commands.execute(&words, cx) {
                tracing::warn!("radio: raise: {e}");
            }
        }
    }
}

// ── Actions ──────────────────────────────────────────────────────────────

/// Plays station `id` (curated, yours, or a directory result).
pub(crate) fn play(id: &str, cx: &mut App) {
    let id = id.to_string();
    let s = settings(cx);
    with_lib(cx, move |lib, player, _| {
        let Some(st) = lib.station(&id) else { return };
        let volume = if s.per_station_volume { lib.volume_of(&id) } else { lib.volume };
        player.set_options(s.options, s.mpris);
        player.set_volume(volume);
        player.play(st.tune());
    });
}

/// Plays a directory result without adding it.
pub(crate) fn play_found(found: &directory::Found, cx: &mut App) {
    let shared = model(cx);
    let s = settings(cx);
    let m = shared.borrow();
    m.player.set_options(s.options, s.mpris);
    m.player.play(player::Tune { id: format!("dir-{}", found.uuid), name: found.name.clone(), url: found.url.clone(), kbps: (found.kbps > 0).then_some(found.kbps) });
}

/// The station the play button starts when none was chosen: the last
/// one played or the first favourite (as set), else the other, else the
/// first curated station.
fn first_choice(lib: &Library, s: &Settings) -> Option<String> {
    let (last, favourite) = (lib.last.as_deref(), lib.favourites.first().map(String::as_str));
    let (a, b) = if s.usual_favourite { (favourite, last) } else { (last, favourite) };
    a.or(b).or(radio::CURATED.first().map(|c| c.id)).map(str::to_string)
}

/// Play or pause; starts the usual station when nothing was chosen.
pub(crate) fn toggle(cx: &mut App) {
    let shared = model(cx);
    let player = shared.borrow().player.clone();
    if player.status().active() {
        player.pause();
        return;
    }
    if player.with(|s| s.tune.is_some()) {
        let s = settings(cx);
        player.set_options(s.options, s.mpris);
        player.resume();
        return;
    }
    let s = settings(cx);
    with_lib(cx, move |lib, _, cx| {
        if let Some(id) = first_choice(lib, &s) {
            // Run after this borrow ends.
            cx.defer(move |cx| play(&id, cx));
        }
    });
}

/// The stations next / previous walk: your favourites, else every shown
/// station.
fn walk_list(lib: &Library, s: &Settings) -> Vec<String> {
    if !lib.favourites.is_empty() {
        return lib.favourites.clone();
    }
    lib.all().filter(|st| s.categories[st.category().index()]).map(|st| st.id().to_string()).collect()
}

/// Moves `delta` stations through the walk list.
fn step(shared: &Shared, delta: isize, cx: &mut App) {
    let s = settings(cx);
    let current = shared.borrow().player.with(|snap| snap.tune.as_ref().map(|t| t.id.clone()));
    with_lib(cx, move |lib, _, cx| {
        let list = walk_list(lib, &s);
        if list.is_empty() {
            return;
        }
        let at = current.and_then(|c| list.iter().position(|id| *id == c));
        let next = match at {
            Some(i) => (i as isize + delta).rem_euclid(list.len() as isize) as usize,
            None => 0,
        };
        let id = list[next].clone();
        cx.defer(move |cx| play(&id, cx));
    });
}

pub(crate) fn next(cx: &mut App) {
    let shared = model(cx);
    step(&shared, 1, cx);
}

pub(crate) fn previous(cx: &mut App) {
    let shared = model(cx);
    step(&shared, -1, cx);
}

/// Sets the volume (and remembers it for the station when asked to).
pub(crate) fn set_volume(v: f32, cx: &mut App) {
    let s = settings(cx);
    with_lib(cx, move |lib, player, _| {
        let id = player.with(|snap| snap.tune.as_ref().map(|t| t.id.clone()));
        player.set_muted(false);
        player.set_volume(v);
        lib.set_volume(id.as_deref(), v, s.per_station_volume);
        lib.muted = false;
    });
}

pub(crate) fn toggle_mute(cx: &mut App) {
    with_lib(cx, |lib, player, _| {
        let muted = !player.muted();
        player.set_muted(muted);
        lib.muted = muted;
    });
}

/// Adds or removes a favourite (a directory result playing now becomes a
/// station of yours first); `on_error` hears why it couldn't.
pub(crate) fn toggle_favourite(id: &str, cx: &mut App, on_error: impl FnOnce(String, &mut App) + 'static) {
    let id = id.to_string();
    with_lib(cx, move |lib, player, cx| {
        let target = if id.starts_with("dir-") {
            let Some(tune) = player.with(|snap| snap.tune.clone()).filter(|t| t.id == id) else { return };
            match lib.add_custom(&tune.name, &tune.url, Category::Other, tune.kbps, station::Codec::Unknown) {
                Ok(new) => new,
                Err(EditError::Exists) => lib.all().find(|s| s.url() == tune.url).map(|s| s.id().to_string()).unwrap_or_default(),
                Err(e) => return on_error(tr(e.message()), cx),
            }
        } else {
            id
        };
        if let Err(e) = lib.toggle_favourite(&target) {
            on_error(tr(e.message()), cx);
        }
    });
}

pub(crate) fn move_favourite(id: &str, delta: isize, cx: &mut App) {
    let id = id.to_string();
    with_lib(cx, move |lib, _, _| {
        lib.move_favourite(&id, delta);
    });
}

pub(crate) fn remove_station(id: &str, cx: &mut App) {
    let id = id.to_string();
    with_lib(cx, move |lib, _, _| {
        lib.remove_custom(&id);
    });
}

pub(crate) fn set_sleep(minutes: Option<u64>, cx: &mut App) {
    let shared = model(cx);
    let player = shared.borrow().player.clone();
    match minutes {
        Some(m) if m > 0 => player.set_sleep(Duration::from_secs(m * SECS_PER_MIN)),
        _ => player.cancel_sleep(),
    }
}

/// Tests `url` off the UI thread and, when it plays, adds it as yours (or
/// updates the station being edited). The reply says what it found.
pub(crate) fn test_and_save(name: String, url: String, category: Category, editing: Option<String>, cx: &mut App) -> async_channel::Receiver<Result<String, String>> {
    let (tx, rx) = async_channel::bounded(1);
    if let Err(e) = station::validate_url(&url) {
        let _ = tx.try_send(Err(tr(e.message())));
        return rx;
    }
    let shared = model(cx);
    let generation = {
        let mut m = shared.borrow_mut();
        m.editor.generation += 1;
        m.editor.testing = Some(m.editor.generation);
        m.editor.result = None;
        m.editor.generation
    };
    changed(&shared, cx);
    let weak = Rc::downgrade(&shared);
    cx.spawn(async move |cx| {
        let probe_url = url.clone();
        let probed = mandara_services::runtime::unblock(move || radio::probe(&probe_url)).await;
        let Some(shared) = weak.upgrade() else { return };
        cx.update(|cx| {
            if shared.borrow().editor.testing != Some(generation) {
                return;
            }
            let reply = match probed {
                Err(e) => Err(e),
                Ok(p) => {
                    let summary = probe_summary(&p);
                    let name = if name.trim().is_empty() { p.info.server_name.clone().unwrap_or_else(|| url.clone()) } else { name };
                    let outcome = Rc::new(RefCell::new(Ok(summary)));
                    let o = outcome.clone();
                    let kbps = p.info.kbps;
                    let codec = p.info.codec;
                    let new_station = editing.is_none();
                    with_lib(cx, move |lib, _, _| {
                        let r = match &editing {
                            Some(id) => lib.edit_custom(id, &name, &url, category).map(|_| {
                                if let Some(c) = lib.custom.iter_mut().find(|c| &c.id == id) {
                                    c.kbps = kbps;
                                    c.codec = codec;
                                }
                            }),
                            None => lib.add_custom(&name, &url, category, kbps, codec).map(|id| {
                                let _ = lib.toggle_favourite(&id);
                            }),
                        };
                        match r {
                            // A new station is saved: the form is ready for the next.
                            Ok(()) if new_station => panel::fill_editor(String::new(), String::new()),
                            Ok(()) => {}
                            Err(e) => *o.borrow_mut() = Err(tr(e.message())),
                        }
                    });
                    outcome.borrow().clone()
                }
            };
            {
                let mut m = shared.borrow_mut();
                m.editor.testing = None;
                m.editor.result = Some(reply.clone());
            }
            let _ = tx.try_send(reply);
            changed(&shared, cx);
        });
    })
    .detach();
    rx
}

fn probe_summary(p: &radio::Probe) -> String {
    let mut parts = Vec::new();
    if !p.info.codec.label().is_empty() {
        parts.push(p.info.codec.label().to_string());
    }
    if let Some(k) = p.info.kbps {
        parts.push(tf("{n} kbit/s", &[("n", &k)]));
    }
    if p.info.rate > 0 {
        parts.push(tf("{n} kHz", &[("n", &format!("{:.1}", f64::from(p.info.rate) / 1000.0))]));
    }
    tf("It plays: {details}", &[("details", &parts.join(" · "))])
}

/// Searches the directory (when it is on), keeping one search in flight
/// and freeing the results after a while.
pub(crate) fn search_directory(query: &str, cx: &mut App) {
    let s = settings(cx);
    if !s.directory {
        return;
    }
    let query = query.trim().to_string();
    let shared = model(cx);
    let generation = {
        let mut m = shared.borrow_mut();
        let d = &mut m.directory;
        if d.query == query && (d.busy || !d.results.is_empty()) {
            return;
        }
        d.generation += 1;
        d.query = query.clone();
        d.busy = !query.is_empty();
        d.error = None;
        d.results = Vec::new();
        d.expiry = None;
        d.generation
    };
    changed(&shared, cx);
    if query.is_empty() {
        return;
    }
    let limit = s.directory_limit;
    let weak = Rc::downgrade(&shared);
    cx.spawn(async move |cx| {
        let q = query.clone();
        let found = mandara_services::runtime::unblock(move || directory::search(&q, limit)).await;
        let Some(shared) = weak.upgrade() else { return };
        cx.update(|cx| {
            {
                let mut m = shared.borrow_mut();
                let d = &mut m.directory;
                if d.generation != generation {
                    return;
                }
                d.busy = false;
                match found {
                    Ok(list) => d.results = list,
                    Err(e) => d.error = Some(e),
                }
            }
            let weak = Rc::downgrade(&shared);
            let expiry = cx.spawn(async move |cx| {
                cx.background_executor().timer(DIRECTORY_TTL).await;
                let Some(shared) = weak.upgrade() else { return };
                cx.update(|cx| {
                    let mut m = shared.borrow_mut();
                    if m.directory.generation == generation {
                        m.directory = Directory::default();
                    }
                    drop(m);
                    changed(&shared, cx);
                });
            });
            shared.borrow_mut().directory.expiry = Some(expiry);
            changed(&shared, cx);
        });
    })
    .detach();
}

/// Stops playback and frees everything because the plugin was switched
/// off (nothing when it never ran).
pub fn stop(cx: &mut App) {
    SETTINGS.with(|c| c.borrow_mut().clear());
    art::clear();
    bar::clear();
    if cx.has_global::<GlobalRadio>() {
        let GlobalRadio(shared) = cx.remove_global::<GlobalRadio>();
        let mut m = shared.borrow_mut();
        m.player.stop();
        // A change still waiting for its delayed save is written now.
        if m.save_timer.take().is_some()
            && let Some(lib) = m.lib.clone()
        {
            let generation = next_save_generation();
            cx.background_spawn(async move { write_library(&lib, generation, &Library::path()) }).detach();
        }
    }
}

/// Starts the last station at login when "resume on login" is on (once).
fn resume_on_login(cx: &mut App) {
    if RESUMED.with(|r| r.replace(true)) {
        return;
    }
    let s = settings(cx);
    if !s.resume_on_login {
        return;
    }
    with_lib(cx, move |lib, _, cx| {
        if let Some(id) = first_choice(lib, &s) {
            cx.defer(move |cx| play(&id, cx));
        }
    });
}

/// The shelf of station `id` (curated, yours once read, else Other).
pub(crate) fn station_category(id: &str, cx: &App) -> Category {
    if let Some(c) = station::curated(id) {
        return c.category;
    }
    existing(cx).and_then(|m| m.borrow().lib.as_ref().and_then(|lib| lib.station(id).map(|s| s.category()))).unwrap_or(Category::Other)
}

// ── Launcher ─────────────────────────────────────────────────────────────

/// Launcher entries for the stations: `(id, name, shelf, search words)`.
/// The curated ones always; yours once the library has been read.
pub(crate) fn launcher_stations(cx: &App) -> Vec<(String, String, &'static str, String)> {
    let mut out: Vec<_> = radio::CURATED.iter().map(|c| (c.id.to_string(), c.name.to_string(), c.category.label(), format!("radio station fm {} {} {}", c.category.key(), c.country, c.tags))).collect();
    if let Some(shared) = existing(cx)
        && let Some(lib) = shared.borrow().lib.as_ref()
    {
        out.extend(lib.custom.iter().map(|c| (c.id.clone(), c.name.clone(), c.category.label(), format!("radio station fm {}", c.category.key()))));
    }
    out
}

// ── Commands ─────────────────────────────────────────────────────────────

fn status_json(cx: &mut App) -> Value {
    let Some(shared) = existing(cx) else {
        return json!({ "status": "stopped", "station": null, "title": null });
    };
    let m = shared.borrow();
    let p = &m.player;
    let fill = p.buffer_fill();
    p.with(|s| {
        json!({
            "status": s.status,
            "station": s.tune.as_ref().map(|t| json!({ "id": t.id, "name": t.name, "url": t.url })),
            "title": s.title,
            "codec": s.info.codec.label(),
            "kbps": s.info.kbps,
            "sample_rate": s.info.rate,
            "buffer": fill.map(|f| (f64::from(f) * PERCENT).round()),
            "volume": (f64::from(p.volume()) * PERCENT).round(),
            "muted": p.muted(),
            "error": s.error,
            "reconnect_attempt": s.attempt,
            "sleep_remaining_s": p.sleep_remaining().map(|(left, _)| left.as_secs()),
        })
    })
}

/// A station by id, or the first whose name contains `query`.
fn find_station(lib: &Library, query: &str) -> Option<String> {
    if lib.station(query).is_some() {
        return Some(query.to_string());
    }
    let q = query.to_lowercase();
    let words: Vec<&str> = q.split_whitespace().collect();
    lib.all().find(|s| s.matches(&words, "")).map(|s| s.id().to_string())
}

/// Answers a command with the library: at once when it has been read,
/// else once it is (the UI thread never reads files).
fn lib_command(cx: &mut App, f: impl FnOnce(&mut Library, &Player, &mut App) -> Result<Value, String> + 'static) -> Result<Output, String> {
    let (tx, rx) = async_channel::bounded::<Result<String, String>>(1);
    let now: Rc<RefCell<Option<Result<Value, String>>>> = Rc::default();
    let inline = Rc::new(Cell::new(true));
    let (slot, during) = (now.clone(), inline.clone());
    with_lib(cx, move |lib, player, cx| {
        let r = f(lib, player, cx);
        if during.get() {
            *slot.borrow_mut() = Some(r);
        } else {
            let _ = tx.try_send(r.map(|v| v.to_string()));
        }
    });
    inline.set(false);
    let r = now.borrow_mut().take();
    match r {
        Some(r) => r.map(Output::Value),
        None => Ok(Output::Pending(rx)),
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Radio"), tk("Internet radio: curated stations by genre, your own stations and favourites, with live now-playing"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        resume_on_login(cx);
        let bar = fn_view(None, bar::render, window, cx);
        let (view, size_cell) = panel::View::new(panel_cx.clone());
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        let sidebar = fn_view(None, panel::sidebar, window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), sidebar: Some(sidebar), on_click: None, on_secondary_click: Some(Rc::new(|_, cx| toggle(cx))) }
    });
    r.command(ID, CommandSpec::new("radio status", tk("What plays: station, title, format, buffer, volume"), |_, cx| Ok(status_json(cx))));
    r.command(
        ID,
        CommandSpec::new_output("radio play", tk("Play a station (by id or name), or the usual one"), |a, cx| {
            let Some(q) = a.str(0).ok().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string) else {
                if !model(cx).borrow().player.status().active() {
                    toggle(cx);
                }
                return Ok(Output::Value(status_json(cx)));
            };
            lib_command(cx, move |lib, _, cx| {
                let id = find_station(lib, &q).ok_or_else(|| tf("No station matches {query}", &[("query", &q)]))?;
                let name = lib.station(&id).map(|s| s.name().to_string());
                let reply = json!({ "playing": id, "name": name });
                cx.defer(move |cx| play(&id, cx));
                Ok(reply)
            })
        })
        .arg(ArgSpec::optional("station", ArgKind::String, tk("station id or name")))
        .example("radio play fip-jazz"),
    );
    for (path, summary, action) in [
        ("radio pause", tk("Pause the radio"), 0u8),
        ("radio toggle", tk("Play or pause the radio"), 1),
        ("radio stop", tk("Stop the radio and release everything"), 2),
        ("radio next", tk("Next favourite station"), 3),
        ("radio previous", tk("Previous favourite station"), 4),
    ] {
        r.command(ID, CommandSpec::new(path, summary, move |_, cx| {
            match action {
                0 => {
                    if let Some(m) = existing(cx) {
                        m.borrow().player.pause();
                    }
                }
                1 => toggle(cx),
                2 => {
                    if let Some(m) = existing(cx) {
                        m.borrow().player.stop();
                    }
                }
                3 => next(cx),
                _ => previous(cx),
            }
            Ok(status_json(cx))
        }));
    }
    r.command(
        ID,
        CommandSpec::new("radio volume", tk("Set the radio's volume (0–100)"), |a, cx| {
            let v = (a.number(0)? / PERCENT).clamp(0.0, 1.0) as f32;
            set_volume(v, cx);
            Ok(status_json(cx))
        })
        .arg(ArgSpec::required("percent", ArgKind::Number, tk("0–100")))
        .example("radio volume 60"),
    );
    r.command(ID, CommandSpec::new("radio mute", tk("Mute or unmute the radio"), |_, cx| {
        toggle_mute(cx);
        Ok(status_json(cx))
    }));
    r.command(
        ID,
        CommandSpec::new("radio sleep", tk("Stop the radio after N minutes (fading out), or `off`"), |a, cx| {
            match a.str(0)? {
                "off" | "0" => set_sleep(None, cx),
                mins => set_sleep(Some(mins.parse::<u64>().map_err(|_| t("Expected minutes or off").to_string())?.max(1)), cx),
            }
            Ok(status_json(cx))
        })
        .arg(ArgSpec::required("minutes", ArgKind::String, tk("minutes, or off")))
        .example("radio sleep 30"),
    );
    r.command(
        ID,
        CommandSpec::new_output("radio stations", tk("List stations, all or of one shelf"), |a, cx| {
            let shelf = a.str(0).ok().map(|c| Category::parse(c).ok_or_else(|| tf("Unknown shelf {name}", &[("name", &c)]))).transpose()?;
            lib_command(cx, move |lib, _, _| {
                Ok(Value::Array(
                    lib.all()
                        .filter(|s| shelf.is_none_or(|c| s.category() == c))
                        .map(|s| json!({ "id": s.id(), "name": s.name(), "shelf": s.category().key(), "url": s.url(), "favourite": lib.is_favourite(s.id()), "yours": s.is_custom() }))
                        .collect(),
                ))
            })
        })
        .arg(ArgSpec::optional("shelf", ArgKind::String, tk("news, talk, pop, rock, jazz, classical, electronic, chill, ambient, world, arabic, quran, kids or other")))
        .example("radio stations jazz"),
    );
    r.command(
        ID,
        CommandSpec::new_output("radio favourite", tk("Add or remove a favourite station"), |a, cx| {
            let q = a.str(0)?.to_string();
            lib_command(cx, move |lib, _, _| {
                let id = find_station(lib, &q).ok_or_else(|| tf("No station matches {query}", &[("query", &q)]))?;
                let on = lib.toggle_favourite(&id).map_err(|e| tr(e.message()))?;
                Ok(json!({ "station": id, "favourite": on }))
            })
        })
        .arg(ArgSpec::required("station", ArgKind::String, tk("station id or name")))
        .example("radio favourite fip-jazz"),
    );
    r.command(
        ID,
        CommandSpec::new_output("radio add", tk("Test a stream address and add it as your station"), |a, cx| {
            let url = a.str(0)?.to_string();
            let name = a.str(1).map(str::to_string).unwrap_or_default();
            Ok(Output::Pending(test_and_save(name, url, Category::Other, None, cx)))
        })
        .arg(ArgSpec::required("url", ArgKind::String, tk("http(s) stream or playlist address")))
        .arg(ArgSpec::optional("name", ArgKind::String, tk("station name (else the server's)")))
        .example("radio add https://ice1.somafm.com/groovesalad-128-mp3 \"Groove Salad\""),
    );
    r.command(
        ID,
        CommandSpec::new_output("radio remove", tk("Remove one of your stations"), |a, cx| {
            let q = a.str(0)?.to_string();
            lib_command(cx, move |lib, _, _| {
                let id = find_station(lib, &q).ok_or_else(|| tf("No station matches {query}", &[("query", &q)]))?;
                if !lib.remove_custom(&id) {
                    return Err(t("Only stations you added can be removed").into());
                }
                Ok(json!({ "removed": id }))
            })
        })
        .arg(ArgSpec::required("station", ArgKind::String, tk("station id or name"))),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(table: &str) -> mandara_config::ShellConfig {
        toml::from_str(&format!("[plugins.settings.radio]\n{table}")).unwrap()
    }

    #[test]
    fn settings_round_trip_with_defaults_and_bounds() {
        let d = Settings::resolve(&cfg(""), Lang::En, ());
        assert_eq!(d.options, player::Options::default());
        assert!(d.per_station_volume && d.mpris && !d.notify && !d.directory && !d.resume_on_login);
        assert_eq!((d.bar_style, d.art), (BarStyle::Full, ArtStyle::Vivid));
        assert!(d.categories.iter().all(|c| *c));
        assert_eq!(d.sleep_minutes, SLEEP_CHOICES[1]);
        let s = Settings::resolve(
            &cfg("buffer_seconds = 60\nreconnect_attempts = 0\nauto_reconnect = false\nbar_style = \"compact\"\nmarquee_speed = \"off\"\nbar_width = 9000\nart_style = \"theme\"\ncategories = [\"jazz\", \"quran\", \"nope\"]\ndirectory = true\ndirectory_limit = 500\nnotify_now_playing = true\nmedia_controls = false\nusual = \"favourite\"\nresume_on_login = true\nper_station_volume = false\nsleep_minutes = 45\n"),
            Lang::En,
            (),
        );
        assert_eq!(s.options, player::Options { buffer_seconds: player::BUFFER_RANGE.1, auto_reconnect: false, attempts: radio::backoff::ATTEMPTS_RANGE.0 });
        assert_eq!((s.bar_style, s.marquee_px_s, s.bar_width, s.art), (BarStyle::Compact, 0.0, BAR_WIDTH_RANGE.1, ArtStyle::Theme));
        let shown: Vec<Category> = Category::ALL.into_iter().filter(|c| s.categories[c.index()]).collect();
        assert_eq!(shown, [Category::Jazz, Category::Quran, Category::Other], "your stations always have a shelf");
        assert!(s.directory && s.notify && !s.mpris && s.resume_on_login && !s.per_station_volume);
        assert_eq!(s.directory_limit, directory::MAX_RESULTS);
        assert!(s.usual_favourite);
        assert_eq!(s.sleep_minutes, 45);
        // Every marquee choice resolves.
        for (key, speed) in MARQUEE_SPEEDS {
            assert_eq!(Settings::resolve(&cfg(&format!("marquee_speed = \"{key}\"")), Lang::En, ()).marquee_px_s, speed);
        }
    }

    #[test]
    fn the_usual_station_and_the_walk() {
        let s = Settings::resolve(&cfg(""), Lang::En, ());
        let mut lib = Library::default();
        assert_eq!(first_choice(&lib, &s).as_deref(), Some(radio::CURATED[0].id), "a first run plays something");
        lib.toggle_favourite("fip-jazz").unwrap();
        assert_eq!(first_choice(&lib, &s).as_deref(), Some("fip-jazz"));
        lib.played("radio-paradise");
        assert_eq!(first_choice(&lib, &s).as_deref(), Some("radio-paradise"), "the last one played wins");
        let favourite_first = Settings { usual_favourite: true, ..s.clone() };
        assert_eq!(first_choice(&lib, &favourite_first).as_deref(), Some("fip-jazz"));
        assert_eq!(walk_list(&lib, &s), ["fip-jazz"]);
        let only_jazz = Settings::resolve(&cfg("categories = [\"jazz\"]"), Lang::En, ());
        let walk = walk_list(&Library::default(), &only_jazz);
        assert!(!walk.is_empty() && walk.iter().all(|id| Library::default().station(id).unwrap().category() == Category::Jazz));
        assert_eq!(find_station(&lib, "fip-jazz").as_deref(), Some("fip-jazz"));
        assert_eq!(find_station(&lib, "groove salad").as_deref(), Some("soma-groove-salad"));
        assert_eq!(find_station(&lib, "zzzz-nothing"), None);
    }

    /// Save generations are process-wide: after Radio is switched off and
    /// on again (a new model), the next save is still newer than every
    /// write so far, and a stale write never replaces a newer one.
    #[test]
    fn saves_after_a_restart_still_land() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("radio.toml");
        let mut lib = Library::default();
        for _ in 0..3 {
            write_library(&lib, next_save_generation(), &path);
        }
        let stale = next_save_generation();
        lib.toggle_favourite("fip-jazz").unwrap();
        let fresh = next_save_generation();
        assert!(fresh > *WRITTEN.lock().unwrap(), "a later model's first save is newer than every write");
        write_library(&lib, fresh, &path);
        write_library(&Library::default(), stale, &path);
        assert_eq!(Library::load(&path), lib, "the newest change stays on disk");
    }
}
