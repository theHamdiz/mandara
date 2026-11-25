//! Qur'an: a reader and companion for the whole Qur'an. The panel (Home
//! with "continue reading", the khatm ring, the ayah of the day and the
//! juz grid; the 114 surahs; the mushaf-like reader with study mode;
//! bookmarks; search), recitation of an ayah, a range or a whole surah
//! with the ayah being recited glowing in the reader, a bar chip (a book,
//! or a small wave and `surah:ayah` while reciting), a sidebar card and
//! desktop widgets (`ayah`, `progress`). Opt-in.
//!
//! Content comes from `mandara_services::quran`: the Tanzil Uthmani text
//! with its index and, outside Arabic, the meaning in the interface
//! language are downloaded on first use (about 3.8 MB from
//! api.alquran.cloud), verified and kept; recitations are fetched per ayah
//! and cached. Loading, downloads, search and audio all run off the UI
//! thread; nothing ticks while idle (the bar's wave animates only while a
//! recitation plays). Where you read, the khatm, bookmarks and the reader's
//! mode live in `~/.local/state/mandara/quran.json`.
//!
//! Settings (`[plugins.settings.quran]`): `reciter`, `translation` (an
//! edition id for your language; empty = the default), `arabic_size`
//! (`small` / `medium` / `large`).
//!
//! Commands: `quran surahs|read <surah[:ayah]>|today|search <q>|bookmark
//! add|remove <ref>|bookmarks|play <ref|surah>|pause|resume|stop|download|
//! status|remove|editions`.

pub mod art;
mod panel;
mod reader;
mod share;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Local;
use gpui::{AnyElement, App, AppContext as _, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Task, Window, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui;
use mandara_i18n::{Lang, t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, DesktopWidgetType, Output, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::quran::player::{self, Event, Player, Reciter, Snapshot, Status};
use mandara_services::quran::search::{self, Index};
use mandara_services::quran::state::{self, State};
use mandara_services::quran::store::{self, Progress};
use mandara_services::quran::{Quran, Ref, Selection, Translation, daily};
use mandara_theme::{Color, MandaraPalette};
use serde_json::{Value, json};

use super::panel_fx::{ViewSet, arabic_digits, partner, redraw_in};
use super::{fn_view, plugin_lang};
use crate::bidi::Bidi as _;
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::{card, text, tokens};
use crate::state::config;

use super::settings_cache::{ConfigSettings, SettingsCache};

pub const ID: &str = "quran";

// ── Sizes & timing ───────────────────────────────────────────────────────

/// Bar chip: glyph size, gap, the wave's bars and how it moves.
const BAR_ICON_PX: f32 = 14.0;
const BAR_GAP_PX: f32 = 6.0;
const WAVE_BARS: usize = 4;
const WAVE_BAR_W_PX: f32 = 2.5;
const WAVE_BAR_GAP_PX: f32 = 2.0;
const WAVE_MIN_PX: f32 = 3.0;
const WAVE_MAX_PX: f32 = 13.0;
/// How strongly the output level lifts the wave, and its floor.
const WAVE_LEVEL_GAIN: f32 = 6.0;
const WAVE_LEVEL_MIN: f32 = 0.25;
/// Slow, out-of-phase breathing (rad/s, per-bar step, phase step).
const WAVE_RATE: f32 = 5.2;
const WAVE_RATE_STEP: f32 = 0.9;
const WAVE_PHASE_STEP: f32 = 1.3;
/// The wave redraws this often while a recitation plays.
const CHIP_FRAME: Duration = Duration::from_millis(40);
/// Paused: the wave rests at this share of its height and alpha.
const PAUSED_SCALE: f32 = 0.45;
/// Sidebar card: icon, Arabic size, clamped lines, the ring.
const SIDEBAR_ICON_PX: f32 = 15.0;
const SIDEBAR_ARABIC_PX: f32 = 19.0;
const SIDEBAR_NAME_PX: f32 = 20.0;
const SIDEBAR_LINES: usize = 2;
const SIDEBAR_RING_PX: f32 = 40.0;
const SIDEBAR_RING_STROKE: f32 = 3.5;
/// Arabic line height (em): room for tashkeel above and below.
pub(crate) const ARABIC_LINE_EM: f32 = super::jannah::ARABIC_LINE_EM;
/// Reading positions are written at most this often.
const SAVE_DELAY: Duration = Duration::from_millis(1500);
/// Results a search returns at most.
pub(crate) const SEARCH_MAX: usize = 200;
const COMMAND_SEARCH_MAX: usize = 50;
/// The reader's glow fades from one ayah to the next over this long.
pub(crate) const GLOW_MOVE: Duration = Duration::from_millis(450);

// ── Settings ─────────────────────────────────────────────────────────────

/// How large the reader sets the Arabic.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ArabicSize {
    Small,
    Medium,
    Large,
}

impl ArabicSize {
    /// The reader's Arabic size, px.
    pub(crate) fn reader_px(self) -> f32 {
        match self {
            ArabicSize::Small => 22.0,
            ArabicSize::Medium => 26.0,
            ArabicSize::Large => 31.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    pub reciter: &'static Reciter,
    /// The chosen translation edition (empty: the language's default).
    pub translation: String,
    pub arabic_size: ArabicSize,
    /// The language the Qur'an plugin speaks (and shows the meaning in):
    /// its own, else Mandara's.
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
            reciter: player::reciter_or_default(s.setting_str(ID, "reciter").unwrap_or(player::DEFAULT_RECITER)),
            translation: s.setting(ID, "translation", String::new()),
            arabic_size: match s.setting_str(ID, "arabic_size").unwrap_or("medium") {
                "small" => ArabicSize::Small,
                "large" => ArabicSize::Large,
                _ => ArabicSize::Medium,
            },
            lang: language,
        }
    }
}

/// The Qur'an plugin's own language (`None`: Mandara's), from its
/// `language` setting.
pub(crate) fn own_lang(cx: &App) -> Option<Lang> {
    let config = config(cx).read(cx).snapshot();
    plugin_lang::own_language(config.plugins.setting_str(ID, "language").unwrap_or(plugin_lang::FOLLOW))
}

/// Whether the interface is Arabic (the text only, no translation).
pub(crate) fn arabic_ui() -> bool {
    mandara_i18n::language() == Lang::Ar
}

/// Numbers as the interface writes them (Eastern Arabic digits in Arabic).
pub(crate) fn digits(s: impl Into<String>) -> String {
    let s = s.into();
    if arabic_ui() { arabic_digits(&s) } else { s }
}

/// The meaning edition the settings and language ask for.
pub(crate) fn wanted_edition(cx: &App) -> Option<String> {
    let s = settings(cx);
    mandara_services::quran::meaning_edition(s.lang.code(), &s.translation)
}

/// The gold accent (the same as Jannah's).
pub(crate) fn gold(palette: &MandaraPalette) -> Color {
    super::jannah::gold(palette)
}

/// The ornate end-of-ayah mark with its number (`﴿٢٥٥﴾`).
pub(crate) fn ayah_mark(n: u16) -> String {
    mandara_services::jannah::catalog::ayah_mark(n)
}

// ── The shared model ─────────────────────────────────────────────────────

/// Everything loaded: the text, its meaning and what is built from them.
pub(crate) struct Book {
    pub q: Quran,
    pub meaning: Option<Translation>,
    pub index: Index,
    /// The ayahs of the day, in their order.
    pub daily: Vec<u16>,
    /// Each juz' opening word(s), for the juz grid.
    pub juz_words: Vec<String>,
}

impl Book {
    fn new(q: Quran, meaning: Option<Translation>) -> Self {
        let index = Index::build(&q, meaning.as_ref());
        let daily = daily::candidates(&q);
        let juz_words = (1..=mandara_services::quran::text::JUZS)
            .map(|j| {
                let text = q.juz_start(j).and_then(|r| q.ayah(r)).map(|a| a.text.as_str()).unwrap_or("");
                // The rub' el hizb mark (۞) and pause marks aren't words.
                text.split(' ').filter(|w| w.chars().any(char::is_alphabetic)).take(1).collect::<Vec<_>>().join(" ")
            })
            .collect();
        Self { q, meaning, index, daily, juz_words }
    }

    pub fn edition(&self) -> Option<&str> {
        self.meaning.as_ref().map(|m| m.edition.as_str())
    }

    /// The meaning of global ayah `g`, when downloaded.
    pub fn meaning_of(&self, g: u16) -> Option<&str> {
        self.meaning.as_ref()?.at(g)
    }

    /// Today's ayah (global number).
    pub fn today(&self) -> Option<u16> {
        self.ayah_of_day(0)
    }

    /// The ayah of the day `days` after today (before it when negative).
    pub fn ayah_of_day(&self, days: i64) -> Option<u16> {
        daily::pick(&self.daily, Local::now().date_naive() + chrono::Duration::days(days))
    }

    /// `Al-Baqara 2:255`, or `البقرة ٢:٢٥٥` in Arabic.
    pub fn reference(&self, g: u16) -> String {
        let Some(r) = self.q.at(g) else { return String::new() };
        let Some(s) = self.q.surah(r.surah) else { return r.to_string() };
        if arabic_ui() { format!("{} {}", s.short_name(), arabic_digits(&r.to_string())) } else { format!("{} {r}", s.transliteration) }
    }
}

/// Where the content stands.
#[derive(Clone)]
pub(crate) enum Data {
    /// Not looked at yet.
    Idle,
    /// Loading from disk or downloading (see [`Model::progress`]).
    Working,
    Ready(Arc<Book>),
    Failed(String),
    /// Offline mode is on and nothing is downloaded.
    Offline,
    /// Removed by you: downloaded again only when asked.
    Removed,
}

/// What the panel should show on its next render.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Open {
    /// The reader at a global ayah.
    Reader(u16),
}

pub(crate) struct Model {
    pub data: Data,
    /// The download's progress while one runs.
    pub progress: Option<Progress>,
    /// Why the meaning couldn't be loaded (the text still shows).
    pub meaning_error: Option<String>,
    /// A load is running.
    pending: bool,
    /// The edition the last load was for (`Some(None)`: Arabic only), so a
    /// failure isn't retried in a loop.
    tried: Option<Option<String>>,
    /// Offline mode was on when it was tried (try again once it's off).
    tried_offline: bool,
    pub state: State,
    generation: u64,
    save_timer: Option<Task<()>>,
    pub views: ViewSet,
    pub player: Player,
    /// The recitation as the player last reported it.
    pub playback: Snapshot,
    /// The last recitation error (shown once in the panel).
    pub play_error: Option<String>,
    /// The ayah recited before the current one and when it moved on (the
    /// reader's glow fades across).
    pub glow: Option<(Option<u16>, Instant)>,
    pub open_request: Option<Open>,
    /// `quran download` calls waiting for the running load.
    waiters: Vec<async_channel::Sender<()>>,
}

pub(crate) type Shared = Rc<RefCell<Model>>;

struct GlobalQuran(Shared);

impl Global for GlobalQuran {}

/// The model, made on first use (the state file is a few KB).
pub(crate) fn model(cx: &mut App) -> Shared {
    if let Some(g) = cx.try_global::<GlobalQuran>() {
        return g.0.clone();
    }
    let (tx, rx) = async_channel::unbounded::<Event>();
    let player = Player::new(move |ev| {
        let _ = tx.try_send(ev);
    });
    let shared: Shared = Rc::new(RefCell::new(Model {
        data: Data::Idle,
        progress: None,
        meaning_error: None,
        pending: false,
        tried: None,
        tried_offline: false,
        state: State::load(&state::path()),
        generation: 0,
        save_timer: None,
        views: ViewSet::default(),
        player,
        playback: Snapshot::default(),
        play_error: None,
        glow: None,
        open_request: None,
        waiters: Vec::new(),
    }));
    cx.set_global(GlobalQuran(shared.clone()));
    // The player's events, as they come (no polling).
    let weak = Rc::downgrade(&shared);
    cx.spawn(async move |cx| {
        while let Ok(ev) = rx.recv().await {
            let Some(shared) = weak.upgrade() else { break };
            cx.update(|cx| on_player(&shared, ev, cx));
        }
    })
    .detach();
    shared
}

/// Every view showing the plugin redraws (and the desktop widgets).
pub(crate) fn changed(shared: &Shared, cx: &mut App) {
    let views = shared.borrow().views.clone();
    views.notify(cx);
    crate::backdrop::refresh(cx);
}

/// The loaded book, if any.
pub(crate) fn book(shared: &Shared) -> Option<Arc<Book>> {
    match &shared.borrow().data {
        Data::Ready(b) => Some(b.clone()),
        _ => None,
    }
}

// ── State ────────────────────────────────────────────────────────────────

/// Writes the state off the UI thread (the newest change wins).
fn save_now(shared: &Shared, cx: &mut App) {
    static WRITTEN: std::sync::Mutex<u64> = std::sync::Mutex::new(0);
    let (state, generation) = {
        let mut m = shared.borrow_mut();
        m.generation += 1;
        m.save_timer = None;
        (m.state.clone(), m.generation)
    };
    // The desktop's progress widget follows the saved place.
    crate::backdrop::refresh(cx);
    cx.background_spawn(async move {
        let mut last = WRITTEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if generation > *last {
            if let Err(e) = state.save(&state::path()) {
                tracing::warn!("quran: saving state: {e}");
            }
            *last = generation;
        }
    })
    .detach();
}

/// Saves soon (reading positions change as you scroll).
fn save_soon(shared: &Shared, cx: &mut App) {
    if shared.borrow().save_timer.is_some() {
        return;
    }
    let weak = Rc::downgrade(shared);
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(SAVE_DELAY).await;
        cx.update(|cx| {
            if let Some(shared) = weak.upgrade() {
                save_now(&shared, cx);
            }
        });
    });
    shared.borrow_mut().save_timer = Some(task);
}

/// Reading (or listening) reached global ayah `g`.
pub(crate) fn read_at(shared: &Shared, g: u16, cx: &mut App) {
    // The khatm's notification speaks the plugin's language.
    let _lang = mandara_i18n::scoped(own_lang(cx));
    let completed = {
        let mut m = shared.borrow_mut();
        if m.state.last == Some(g) {
            return;
        }
        m.state.read_at(g, Local::now().timestamp())
    };
    if completed {
        let khatms = shared.borrow().state.khatms;
        super::notifications::send(
            cx,
            mandara_services::NotificationCommand::Post { app: t("Qur'an").into(), icon: "book".into(), summary: t("Khatm complete").into(), body: tf("May Allah accept it from you. That makes {n}.", &[("n", &khatms)]) },
        );
        save_now(shared, cx);
    } else {
        save_soon(shared, cx);
    }
    let views = shared.borrow().views.clone();
    views.notify(cx);
}

/// Bookmarks or un-bookmarks `r`; whether it is one now.
pub(crate) fn toggle_bookmark(shared: &Shared, r: Ref, cx: &mut App) -> bool {
    let on = shared.borrow_mut().state.toggle_bookmark(r, Local::now().timestamp());
    save_now(shared, cx);
    changed(shared, cx);
    on
}

/// Switches the reader between flowing Arabic and study mode.
pub(crate) fn set_study(shared: &Shared, study: bool, cx: &mut App) {
    shared.borrow_mut().state.study = study;
    save_now(shared, cx);
    changed(shared, cx);
}

// ── Loading & downloading ────────────────────────────────────────────────

/// Loads the content (from disk, else downloading it) when it isn't there
/// or the meaning's edition changed; never loops on a failure.
pub(crate) fn ensure(shared: &Shared, cx: &mut App) {
    let want = wanted_edition(cx);
    {
        let m = shared.borrow();
        if m.pending {
            return;
        }
        let tried = m.tried.as_ref() == Some(&want) && !(m.tried_offline && !mandara_services::offline::is_offline());
        match &m.data {
            Data::Ready(b) if b.edition() == want.as_deref() || tried => return,
            Data::Failed(_) if tried => return,
            Data::Offline if mandara_services::offline::is_offline() => return,
            Data::Removed => return,
            _ => {}
        }
    }
    load(shared, want, cx);
}

/// Asks again after a failure (or after a removal).
pub(crate) fn retry(shared: &Shared, cx: &mut App) {
    {
        let mut m = shared.borrow_mut();
        m.tried = None;
        if matches!(m.data, Data::Failed(_) | Data::Offline | Data::Removed) {
            m.data = Data::Idle;
        }
    }
    ensure(shared, cx);
    changed(shared, cx);
}

type Loaded = Result<(Book, Option<String>), String>;

/// Loads (and downloads what is missing) off the UI thread; progress
/// arrives over a channel as bytes do.
fn load(shared: &Shared, edition: Option<String>, cx: &mut App) {
    {
        let mut m = shared.borrow_mut();
        m.pending = true;
        m.tried = Some(edition.clone());
        m.tried_offline = mandara_services::offline::is_offline();
        m.progress = None;
        if !matches!(m.data, Data::Ready(_)) {
            m.data = Data::Working;
        }
    }
    let (tx, rx) = async_channel::unbounded::<Progress>();
    let task = cx.background_spawn(async move {
        let report = move |p: Progress| {
            let _ = tx.try_send(p);
        };
        let loaded: Loaded = match store::ensure(edition.as_deref(), &report) {
            Ok((q, meaning)) => Ok((Book::new(q, meaning), None)),
            // The meaning failed: the text alone, with the reason.
            Err(e) => match store::load_text() {
                Some(q) => Ok((Book::new(q, None), Some(e))),
                None => Err(e),
            },
        };
        loaded
    });
    let shared = shared.clone();
    cx.spawn(async move |cx| {
        while let Ok(p) = rx.recv().await {
            cx.update(|cx| {
                shared.borrow_mut().progress = Some(p);
                let views = shared.borrow().views.clone();
                views.notify(cx);
            });
        }
        let loaded = task.await;
        cx.update(|cx| {
            {
                let mut m = shared.borrow_mut();
                m.pending = false;
                m.progress = None;
                for w in m.waiters.drain(..) {
                    let _ = w.try_send(());
                }
                match loaded {
                    Ok((book, meaning_error)) => {
                        m.data = Data::Ready(Arc::new(book));
                        m.meaning_error = meaning_error;
                    }
                    Err(e) if mandara_services::offline::is_offline() => {
                        tracing::info!("quran: {e}");
                        m.data = Data::Offline;
                    }
                    Err(e) => {
                        tracing::warn!("quran: {e}");
                        m.data = Data::Failed(e);
                    }
                }
            }
            changed(&shared, cx);
        });
    })
    .detach();
}

/// Starts the download (or the load from disk) now, and resolves when it is
/// done: for `quran download`.
fn download_command(cx: &mut App) -> Output {
    let shared = model(cx);
    let (tx, rx) = async_channel::bounded::<Result<String, String>>(1);
    retry(&shared, cx);
    let (done_tx, done_rx) = async_channel::bounded::<()>(1);
    if shared.borrow().pending {
        shared.borrow_mut().waiters.push(done_tx);
    } else {
        let _ = done_tx.try_send(());
    }
    let weak = Rc::downgrade(&shared);
    cx.spawn(async move |cx| {
        let _ = done_rx.recv().await;
        let v = cx.update(|_| weak.upgrade().map(|s| status_json(&s)));
        let _ = tx.try_send(match v {
            Some(v) if matches!(v.get("state").and_then(Value::as_str), Some("failed" | "offline")) => Err(v.get("error").and_then(Value::as_str).map_or_else(mandara_services::offline::blocked, str::to_string)),
            Some(v) => Ok(v.to_string()),
            None => Err(t("The Qur'an plugin stopped").into()),
        });
    })
    .detach();
    Output::Pending(rx)
}

/// Removes every download (stopping a recitation first).
fn remove_command(cx: &mut App) -> Output {
    let shared = model(cx);
    shared.borrow().player.stop();
    {
        let mut m = shared.borrow_mut();
        m.data = Data::Removed;
        m.progress = None;
        m.tried = None;
    }
    changed(&shared, cx);
    let (tx, rx) = async_channel::bounded::<Result<String, String>>(1);
    cx.background_spawn(async move {
        let _ = tx.try_send(store::remove().map(|freed| json!({ "removed": true, "freed_bytes": freed }).to_string()));
    })
    .detach();
    Output::Pending(rx)
}

// ── Recitation ───────────────────────────────────────────────────────────

/// Recites `sel` with the chosen reciter.
pub(crate) fn play(shared: &Shared, sel: Selection, cx: &mut App) -> Result<Vec<u16>, String> {
    let book = book(shared).ok_or_else(|| t("The Qur'an isn't downloaded yet").to_string())?;
    let queue: Vec<u16> = book.q.globals(sel).collect();
    let first = *queue.first().ok_or_else(|| t("Nothing to recite").to_string())?;
    let options = settings(cx);
    let reciter = options.reciter;
    let index = mandara_services::quran::audio::AyahIndex::new(&book.q);
    if mandara_services::offline::is_offline() && !mandara_services::quran::audio::cached(reciter.id, first, &index) {
        return Err(t("Offline mode is on and this recitation isn't downloaded yet").into());
    }
    shared.borrow_mut().play_error = None;
    shared.borrow().player.recite(queue.clone(), reciter.id, index, options.lang);
    sync_playback(shared);
    Ok(queue)
}

/// Takes the player's state now (commands answer before its events land).
fn sync_playback(shared: &Shared) {
    let snap = shared.borrow().player.snapshot();
    shared.borrow_mut().playback = snap;
}

/// Recites from global ayah `g` to the end of its surah.
pub(crate) fn play_from(shared: &Shared, g: u16, cx: &mut App) -> Result<(), String> {
    let book = book(shared).ok_or_else(|| t("The Qur'an isn't downloaded yet").to_string())?;
    let r = book.q.at(g).ok_or_else(|| t("Nothing to recite").to_string())?;
    let end = book.q.surah(r.surah).map(|s| s.first + s.ayahs - 1).unwrap_or(g);
    play(shared, Selection::Range(g, end), cx).map(|_| ())
}

/// Pauses or resumes; whether anything changed.
pub(crate) fn toggle_pause(shared: &Shared) -> bool {
    let p = shared.borrow().player.clone();
    p.pause() || p.resume()
}

fn on_player(shared: &Shared, ev: Event, cx: &mut App) {
    let snap = shared.borrow().player.snapshot();
    let before = shared.borrow().playback.current();
    match &ev {
        Event::Error(e) => shared.borrow_mut().play_error = Some(e.clone()),
        Event::Ayah(g) => {
            shared.borrow_mut().glow = Some((before, Instant::now()));
            read_at(shared, *g, cx);
        }
        Event::Status(_) | Event::Finished | Event::Download(_) => {}
    }
    shared.borrow_mut().playback = snap;
    changed(shared, cx);
}

// ── Opening ──────────────────────────────────────────────────────────────

pub(crate) fn open_panel(cx: &mut App) {
    crate::state::shell(cx).update(cx, |s, cx| s.set_panel(Some(ID.into()), cx));
}

/// Opens the reader at global ayah `g`.
pub(crate) fn open_at(shared: &Shared, g: u16, cx: &mut App) {
    shared.borrow_mut().open_request = Some(Open::Reader(g));
    open_panel(cx);
    changed(shared, cx);
}

/// Opens the reader at the ayah `reference` names (`2:255`), e.g. from a
/// desktop widget.
pub fn open_reference(reference: &str, cx: &mut App) -> Result<(), String> {
    let (shared, book) = need_book(cx)?;
    let g = global_of(&book, reference)?;
    open_at(&shared, g, cx);
    Ok(())
}

/// Where "continue" goes: the last-read ayah (or the beginning).
pub(crate) fn continue_at(shared: &Shared) -> u16 {
    shared.borrow().state.last.filter(|g| *g < mandara_services::quran::text::AYAHS).unwrap_or(1)
}

/// Opens the reader where you left off (the desktop's progress widget).
pub fn continue_reading(cx: &mut App) {
    let shared = model(cx);
    ensure(&shared, cx);
    let g = continue_at(&shared);
    open_at(&shared, g, cx);
}

/// Opens a link in the browser (off the UI thread).
pub(crate) fn open_link(url: String) {
    std::thread::spawn(move || {
        if let Err(err) = mandara_services::session::open_uri(&url) {
            tracing::warn!("quran: opening {url}: {err}");
        }
    });
}

/// quran.com's page for an ayah.
pub(crate) fn link(r: Ref) -> String {
    format!("https://quran.com/{}/{}", r.surah, r.ayah)
}

/// An ayah to paste: the Arabic with its mark, its meaning when asked,
/// and the reference.
pub(crate) fn copy_text(book: &Book, g: u16, with_meaning: bool) -> String {
    let (Some(r), Some(a)) = (book.q.at(g), book.q.ayah_at(g)) else { return String::new() };
    let head = format!("{} {}", a.text, ayah_mark(r.ayah));
    match book.meaning_of(g).filter(|_| with_meaning) {
        Some(m) => format!("{head}\n\n{m}\n\n— {}", book.reference(g)),
        None => format!("{head}\n\n— {}", book.reference(g)),
    }
}

// ── Desktop widgets ──────────────────────────────────────────────────────

/// What the desktop widgets show (`None` until the content is loaded).
pub fn widget_info(cx: &mut App) -> Option<crate::backdrop::widgets::QuranInfo> {
    let own = own_lang(cx);
    let _lang = mandara_i18n::scoped(own);
    let shared = model(cx);
    ensure(&shared, cx);
    let book = book(&shared)?;
    let today = book.today()?;
    let r = book.q.at(today)?;
    let m = shared.borrow();
    let last = m.state.last.and_then(|g| book.q.at(g)).unwrap_or(Ref::new(1, 1));
    let surah = book.q.surah(last.surah)?;
    let playing = m.playback.current().filter(|_| matches!(m.playback.status, Status::Playing | Status::Loading));
    // Verbatim: the ayah of the day is picked to fit a card; the painter
    // folds a long one at a whole word and offers "Read more".
    let text = book.q.ayah_at(today)?.text.clone();
    Some(crate::backdrop::widgets::QuranInfo {
        arabic: text,
        ayah_mark: ayah_mark(r.ayah),
        meaning: book.meaning_of(today).filter(|_| !arabic_ui()).map(str::to_string),
        reference: {
            let s = book.q.surah(r.surah)?;
            format!("{} {r}", s.transliteration)
        },
        reference_ar: {
            let s = book.q.surah(r.surah)?;
            format!("{} {}", s.short_name(), arabic_digits(&r.to_string()))
        },
        open: r.to_string(),
        progress: m.state.progress(),
        khatms: m.state.khatms,
        surah_ar: surah.short_name().to_string(),
        surah: surah.transliteration.clone(),
        ayah: last.ayah,
        ayahs: surah.ayahs,
        juz: book.q.ayah(last).map(|a| a.juz).unwrap_or(1),
        reciting: playing.and_then(|g| book.q.at(g)).map(|r| r.to_string()),
        lang: own,
    })
}

// ── Bar chip ─────────────────────────────────────────────────────────────

thread_local! {
    /// When the wave started, for its phase.
    static WAVE_T0: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

fn bar_view(palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let shared = model(cx);
    shared.borrow().views.track(window);
    let (snap, level) = {
        let m = shared.borrow();
        (m.playback.clone(), m.player.level())
    };
    let size = crate::style::bar_icon(BAR_ICON_PX);
    let ink = palette.on_surface_variant;
    let current = snap.current().filter(|_| snap.status != Status::Stopped);
    let Some(g) = current else {
        WAVE_T0.with(|c| c.set(None));
        return icon("book", size, ink).into_any_element();
    };
    let accent = gold(palette);
    let playing = matches!(snap.status, Status::Playing | Status::Loading);
    let scale = if playing { 1.0 } else { PAUSED_SCALE };
    let t0 = WAVE_T0.with(|c| {
        let t = c.get().unwrap_or_else(Instant::now);
        c.set(Some(t));
        t
    });
    if playing {
        redraw_in(CHIP_FRAME, window, cx);
    }
    let tt = t0.elapsed().as_secs_f32();
    let lift = (level * WAVE_LEVEL_GAIN).clamp(WAVE_LEVEL_MIN, 1.0) * scale;
    let bars = (0..WAVE_BARS).map(|i| {
        let k = i as f32;
        let phase = if playing { tt * (WAVE_RATE + k * WAVE_RATE_STEP) + k * WAVE_PHASE_STEP } else { k * WAVE_PHASE_STEP };
        let h = WAVE_MIN_PX + (WAVE_MAX_PX - WAVE_MIN_PX) * lift * (0.55 + 0.45 * phase.sin());
        div().w(px(WAVE_BAR_W_PX)).h(px(h)).rounded_full().bg(accent.mix(partner(accent), k / WAVE_BARS as f32).with_alpha(if playing { 1.0 } else { PAUSED_SCALE + 0.2 }).hsla())
    });
    let label = book(&shared).and_then(|b| b.q.at(g)).map(|r| digits(r.to_string())).unwrap_or_default();
    div()
        .flex()
        .items_center()
        .gap(px(BAR_GAP_PX))
        .font_features(tabular_nums())
        .child(div().h(px(WAVE_MAX_PX)).flex().items_center().gap(px(WAVE_BAR_GAP_PX)).children(bars))
        .child(div().text_color(if playing { palette.on_surface.hsla() } else { ink.hsla() }).child(label))
        .into_any_element()
}

// ── Sidebar card ─────────────────────────────────────────────────────────

fn sidebar_view(palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let shared = model(cx);
    shared.borrow().views.track(window);
    ensure(&shared, cx);
    let f = fonts(cx);
    let accent = gold(palette);
    let muted = palette.on_surface_variant;
    let head = |trailing: Option<AnyElement>| {
        div()
            .flex()
            .items_center()
            .gap(px(tokens::SM))
            .child(icon("book", SIDEBAR_ICON_PX, accent))
            .child(div().flex_1().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(muted.hsla()).child(t("Qur'an")))
            .when_some(trailing, |el, x| el.child(x))
    };
    let Some(book) = book(&shared) else {
        let line = match &shared.borrow().data {
            Data::Working => t("Getting the Qur'an ready…"),
            Data::Failed(_) => t("The download didn't finish; open the panel to try again"),
            Data::Offline => t("Offline mode is on: the Qur'an downloads once you're online"),
            _ => t("Open the panel to download the Qur'an"),
        };
        return card(palette)
            .flex()
            .flex_col()
            .gap(px(tokens::SM))
            .child(head(None))
            .child(div().id("quran-sidebar-open").cursor_pointer().text_size(px(text::BODY_S)).text_color(muted.hsla()).child(line).on_click(|_, _, cx| open_panel(cx)))
            .into_any_element();
    };
    let (progress, last) = {
        let m = shared.borrow();
        (m.state.progress(), continue_at(&shared))
    };
    let ring = div()
        .relative()
        .size(px(SIDEBAR_RING_PX))
        .flex_none()
        .child(crate::icons::ring(progress, SIDEBAR_RING_PX, SIDEBAR_RING_STROKE, accent, accent.with_alpha(0.18)))
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(text::LABEL - 1.0))
                .font_weight(FontWeight::SEMIBOLD)
                .font_features(tabular_nums())
                .text_color(accent.hsla())
                .child(digits(format!("{}%", (progress * 100.0).floor() as u32))),
        )
        .into_any_element();
    let r = book.q.at(last).unwrap_or(Ref::new(1, 1));
    let surah = book.q.surah(r.surah);
    let resume = {
        let shared = shared.clone();
        div()
            .id("quran-sidebar-continue")
            .flex()
            .flex_col()
            .cursor_pointer()
            .child(div().text_size(px(text::LABEL)).text_color(muted.hsla()).child(t("Continue reading")))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(tokens::SM))
                    .child(div().font_family(f.scripture.clone()).text_size(px(SIDEBAR_NAME_PX)).text_color(accent.hsla()).child(surah.map(|s| s.short_name().to_string()).unwrap_or_default()))
                    .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).font_features(tabular_nums()).child(book.reference(last))),
            )
            .on_click(move |_, _, cx| open_at(&shared, last, cx))
    };
    let today = book.today().map(|g| {
        use super::reading::{Block, fold, link};
        let a = book.q.ayah_at(g).map(|a| a.text.clone()).unwrap_or_default();
        let mark = book.q.at(g).map(|r| ayah_mark(r.ayah)).unwrap_or_default();
        let tint = super::panel_fx::readable(palette.primary, palette);
        // Folded to a couple of lines with its meaning; Read more unfolds
        // the card in place, the reader is one link away.
        let arabic = Block::new(div().font_family(f.scripture.clone()).text_size(px(SIDEBAR_ARABIC_PX)).line_height(gpui::relative(ARABIC_LINE_EM)).text_right(), format!("{a}\u{00A0}{mark}"), SIDEBAR_LINES);
        let meaning = book.meaning_of(g).filter(|_| !arabic_ui()).map(|m| Block::new(div().text_size(px(text::BODY_S)).text_color(muted.hsla()), m.to_string(), SIDEBAR_LINES));
        let open = {
            let shared = shared.clone();
            link("quran-sidebar-open", t("Open in the reader"), tint, move |cx| open_at(&shared, g, cx))
        };
        let key = book.reference(g);
        div()
            .pt(px(tokens::SM))
            .flex()
            .flex_col()
            .gap(px(tokens::XS))
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(tokens::SM))
                    .child(div().flex_1().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(accent.hsla()).child(t("Ayah of the day")))
                    .child(div().text_size(px(text::LABEL)).text_color(muted.hsla()).font_features(tabular_nums()).child(key.clone())),
            )
            .child(fold("quran-sidebar-fold", &key, std::iter::once(arabic).chain(meaning), tokens::XS, tint, Some(open), window, cx))
    });
    card(palette).text_start().flex().flex_col().gap(px(tokens::SM)).child(head(Some(ring))).child(resume).when_some(today, |el, x| el.child(x)).into_any_element()
}

// ── Commands ─────────────────────────────────────────────────────────────

fn surah_json(book: &Book, n: u16) -> Value {
    let Some(s) = book.q.surah(n) else { return Value::Null };
    json!({
        "number": s.number,
        "name": s.name,
        "name_short": s.short_name(),
        "transliteration": s.transliteration,
        "meaning": s.meaning,
        "revelation": s.revelation.id(),
        "ayahs": s.ayahs,
        "first_global": s.first,
        "juz": book.q.ayah(Ref::new(n, 1)).map(|a| a.juz),
        "bismillah": s.has_bismillah(),
    })
}

fn ayah_json(shared: &Shared, book: &Book, g: u16) -> Value {
    let (Some(r), Some(a)) = (book.q.at(g), book.q.ayah_at(g)) else { return Value::Null };
    json!({
        "ref": r.to_string(),
        "surah": r.surah,
        "ayah": r.ayah,
        "global": g,
        "reference": book.reference(g),
        "arabic": a.text,
        "meaning": book.meaning_of(g),
        "meaning_edition": book.edition(),
        "juz": a.juz,
        "page": a.page,
        "sajda": a.sajda,
        "link": link(r),
        "bookmarked": shared.borrow().state.is_bookmarked(r),
    })
}

/// The loaded book, or why there is none (for commands).
fn need_book(cx: &mut App) -> Result<(Shared, Arc<Book>), String> {
    let shared = model(cx);
    ensure(&shared, cx);
    let b = book(&shared);
    match (b, &shared.borrow().data) {
        (Some(b), _) => Ok((shared.clone(), b)),
        (None, Data::Working | Data::Idle) => Err(t("The Qur'an is still downloading; try again in a moment (`quran status`)").into()),
        (None, Data::Offline) => Err(t("Offline mode is on and the Qur'an isn't downloaded yet").into()),
        (None, Data::Removed) => Err(t("The Qur'an was removed; run `quran download`").into()),
        (None, Data::Failed(e)) => Err(tf("The download failed: {error} (`quran download` tries again)", &[("error", e)])),
        (None, Data::Ready(_)) => Err(t("The Qur'an isn't loaded").into()),
    }
}

fn select(book: &Book, text: &str) -> Result<Selection, String> {
    book.q.select(text)
}

/// The ayah a reference names (a surah alone: its first ayah).
fn global_of(book: &Book, text: &str) -> Result<u16, String> {
    let sel = select(book, text)?;
    book.q.globals(sel).next().ok_or_else(|| tf("No surah or ayah {reference}", &[("reference", &text)]))
}

fn playback_json(shared: &Shared, book: Option<&Book>) -> Value {
    let m = shared.borrow();
    let s = &m.playback;
    let current = s.current();
    json!({
        "status": s.status,
        "reciter": (!s.reciter.is_empty()).then(|| s.reciter.clone()),
        "current": current.and_then(|g| book.and_then(|b| b.q.at(g))).map(|r| r.to_string()),
        "queue_length": s.queue.len(),
        "position": (!s.queue.is_empty()).then_some(s.index + 1),
        "download": s.download,
        "error": m.play_error,
    })
}

fn status_json(shared: &Shared) -> Value {
    let book = book(shared);
    let m = shared.borrow();
    let (state, error) = match &m.data {
        Data::Idle => ("idle", None),
        Data::Working => ("downloading", None),
        Data::Ready(_) => ("ready", None),
        Data::Failed(e) => ("failed", Some(e.clone())),
        Data::Offline => ("offline", None),
        Data::Removed => ("removed", None),
    };
    let progress = m.progress.map(|p| json!({ "stage": p.stage, "done_bytes": p.done, "total_bytes": p.total }));
    let last = m.state.last.and_then(|g| book.as_ref().and_then(|b| b.q.at(g))).map(|r| r.to_string());
    drop(m);
    json!({
        "state": state,
        "error": error,
        "progress": progress,
        "meaning_edition": book.as_ref().and_then(|b| b.edition().map(str::to_string)),
        "meaning_name": book.as_ref().and_then(|b| b.meaning.as_ref().map(|m| m.name.clone())),
        "meaning_error": shared.borrow().meaning_error,
        "source": "Tanzil Uthmani text via api.alquran.cloud",
        "last_read": last,
        "khatm_progress": shared.borrow().state.progress(),
        "khatms": shared.borrow().state.khatms,
        "bookmarks": shared.borrow().state.bookmarks.len(),
        "recitation": playback_json(shared, book.as_deref()),
    })
}

/// `quran status`: the model's state plus what is on disk (read off the
/// UI thread).
fn status_command(cx: &mut App) -> Output {
    let shared = model(cx);
    let mut v = status_json(&shared);
    let (tx, rx) = async_channel::bounded::<Result<String, String>>(1);
    cx.background_spawn(async move {
        let u = store::usage();
        v["disk"] = json!({ "text_bytes": u.text_bytes, "meanings": u.meanings, "audio_files": u.audio_files, "audio_bytes": u.audio_bytes, "total_bytes": u.total(), "folder": store::dir(), "audio_folder": store::cache_dir() });
        let _ = tx.try_send(Ok(v.to_string()));
    })
    .detach();
    Output::Pending(rx)
}

fn register_commands(r: &mut PluginRegistry) {
    // Every command answers in the plugin's own language.
    let mut cmd = |spec: CommandSpec| r.command(ID, plugin_lang::speaking(spec, own_lang));
    cmd(plugin_lang::language_command(ID, "quran language", "quran language fr"));
    cmd(CommandSpec::new("quran open", tk("Open the Qur'an panel"), |_, cx| {
        open_panel(cx);
        Ok(json!({"panel": ID}))
    }).example("quran open"));
    cmd(CommandSpec::new_output("quran copy-image", tk("Copy an ayah and its design as a complete PNG image"), |a, cx| share::command(&a.rest(0), cx))
        .arg(ArgSpec::required("ref", ArgKind::String, tk("an ayah, such as 2:255")))
        .example("quran copy-image 2:255"));
    cmd(
        CommandSpec::new("quran peek", tk("Look at the ayah of another day (days after today, or before it when negative) without moving your place"), |a, cx| {
            let days = if a.get(0).is_some() { a.int(0)? } else { 1 };
            let (shared, book) = need_book(cx)?;
            let g = book.ayah_of_day(days).ok_or_else(|| t("Nothing to show").to_string())?;
            let mut v = ayah_json(&shared, &book, g);
            v["days"] = json!(days);
            v["date"] = json!((Local::now().date_naive() + chrono::Duration::days(days)).format("%Y-%m-%d").to_string());
            v["place_moved"] = json!(false);
            Ok(v)
        })
        .arg(ArgSpec::optional("days", ArgKind::Integer, tk("how many days ahead (1 when left out; negative looks back)")))
        .example("quran peek")
        .example("quran peek 7"),
    );
    cmd(
        CommandSpec::new("quran surahs", tk("The 114 surahs: names, meaning, Makki or Madani and ayah counts"), |_, cx| {
            let (_, book) = need_book(cx)?;
            Ok(json!((1..=mandara_services::quran::text::SURAHS).map(|n| surah_json(&book, n)).collect::<Vec<_>>()))
        }),
    );
    cmd(
        CommandSpec::new("quran read", tk("Open the reader at a surah or ayah and return its text and meaning"), |a, cx| {
            let (shared, book) = need_book(cx)?;
            let sel = select(&book, &a.rest(0))?;
            let globals: Vec<u16> = book.q.globals(sel).collect();
            let first = *globals.first().ok_or_else(|| t("Nothing to read").to_string())?;
            open_at(&shared, first, cx);
            read_at(&shared, first, cx);
            let surah = book.q.at(first).map(|r| surah_json(&book, r.surah)).unwrap_or(Value::Null);
            Ok(json!({ "surah": surah, "ayahs": globals.iter().map(|g| ayah_json(&shared, &book, *g)).collect::<Vec<_>>() }))
        })
        .arg(ArgSpec::required("ref", ArgKind::String, tk("a surah (2, al-baqara) or an ayah (2:255), or a range (2:255-257)")))
        .example("quran read 2:255")
        .example("quran read al-kahf")
        .example("quran read 36"),
    );
    cmd(
        CommandSpec::new("quran today", tk("The ayah of the day with its meaning"), |_, cx| {
            let (shared, book) = need_book(cx)?;
            let g = book.today().ok_or_else(|| t("Nothing to show").to_string())?;
            Ok(ayah_json(&shared, &book, g))
        }),
    );
    cmd(
        CommandSpec::new_output("quran search", tk("Search the Arabic (without diacritics) and the meaning; every word must match"), |a, cx| {
            let (shared, book) = need_book(cx)?;
            let query = a.rest(0);
            let (tx, rx) = async_channel::bounded::<Result<String, String>>(1);
            let task = cx.background_spawn({
                let book = book.clone();
                async move { search::search(&book.index, &book.q, &query, COMMAND_SEARCH_MAX) }
            });
            cx.spawn(async move |cx| {
                let hits = task.await;
                let v = cx.update(|_| json!({ "total": hits.total, "shown": hits.ayahs.len(), "ayahs": hits.ayahs.iter().map(|g| ayah_json(&shared, &book, *g)).collect::<Vec<_>>() }));
                let _ = tx.try_send(Ok(v.to_string()));
            })
            .detach();
            Ok(Output::Pending(rx))
        })
        .arg(ArgSpec::required("query", ArgKind::String, tk("words in Arabic or in your language, or a reference")))
        .example("quran search الحي القيوم")
        .example("quran search patience"),
    );
    cmd(
        CommandSpec::new("quran bookmarks", tk("Your bookmarked ayahs, newest first"), |_, cx| {
            let (shared, book) = need_book(cx)?;
            let marks = shared.borrow().state.bookmarks.clone();
            Ok(json!(marks
                .iter()
                .filter_map(|b| {
                    let g = book.q.global(b.at_ref)?;
                    let mut v = ayah_json(&shared, &book, g);
                    v["bookmarked_at"] = json!(b.at);
                    Some(v)
                })
                .collect::<Vec<_>>()))
        }),
    );
    for (path, add) in [("quran bookmark add", true), ("quran bookmark remove", false)] {
        cmd(
            CommandSpec::new(path, if add { tk("Bookmark an ayah") } else { tk("Remove a bookmark") }, move |a, cx| {
                let (shared, book) = need_book(cx)?;
                let g = global_of(&book, &a.rest(0))?;
                let at = book.q.at(g).ok_or_else(|| t("Nothing to bookmark").to_string())?;
                let changed_now = {
                    let mut m = shared.borrow_mut();
                    if add { m.state.bookmark(at, Local::now().timestamp()) } else { m.state.unbookmark(at) }
                };
                if changed_now {
                    save_now(&shared, cx);
                    changed(&shared, cx);
                }
                Ok(json!({ "ref": at.to_string(), "bookmarked": add, "changed": changed_now }))
            })
            .arg(ArgSpec::required("ref", ArgKind::String, tk("an ayah: 2:255 or al-baqara:255")))
            .example(if add { "quran bookmark add 2:255" } else { "quran bookmark remove 2:255" }),
        );
    }
    cmd(
        CommandSpec::new("quran play", tk("Recite an ayah, a range or a whole surah (the reader follows along)"), |a, cx| {
            let (shared, book) = need_book(cx)?;
            let sel = select(&book, &a.rest(0))?;
            let queue = play(&shared, sel, cx)?;
            changed(&shared, cx);
            Ok(json!({ "status": "loading", "reciter": settings(cx).reciter.id, "first": book.q.at(queue[0]).map(|r| r.to_string()), "last": queue.last().and_then(|g| book.q.at(*g)).map(|r| r.to_string()), "ayahs": queue.len() }))
        })
        .arg(ArgSpec::required("ref", ArgKind::String, tk("2:255, 2:255-257, 2:255- (to the surah's end), 36 or yaseen")))
        .example("quran play 1")
        .example("quran play 2:255")
        .example("quran play 2:285-286"),
    );
    cmd(
        CommandSpec::new("quran pause", tk("Pause the recitation"), |_, cx| {
            let shared = model(cx);
            let paused = shared.borrow().player.pause();
            sync_playback(&shared);
            let mut v = playback_json(&shared, book(&shared).as_deref());
            v["changed"] = json!(paused);
            Ok(v)
        }),
    );
    cmd(
        CommandSpec::new("quran resume", tk("Resume a paused recitation where it stopped"), |_, cx| {
            let shared = model(cx);
            let resumed = shared.borrow().player.resume();
            sync_playback(&shared);
            let mut v = playback_json(&shared, book(&shared).as_deref());
            v["changed"] = json!(resumed);
            Ok(v)
        }),
    );
    cmd(
        CommandSpec::new("quran stop", tk("Stop the recitation"), |_, cx| {
            let shared = model(cx);
            shared.borrow().player.stop();
            sync_playback(&shared);
            Ok(playback_json(&shared, book(&shared).as_deref()))
        }),
    );
    cmd(CommandSpec::new_output("quran download", tk("Download the Qur'an and the meaning in your language now (or load them); waits until done"), |_, cx| Ok(download_command(cx))));
    cmd(CommandSpec::new_output("quran status", tk("What is downloaded and loaded, its size, the khatm and the recitation"), |_, cx| Ok(status_command(cx))));
    cmd(CommandSpec::new_output("quran remove", tk("Remove the downloaded text, meanings and cached recitations"), |_, cx| Ok(remove_command(cx))));
    cmd(
        CommandSpec::new_output("quran editions", tk("The translations available in your language (for Studio's translation setting)"), |_, cx| {
            let lang = mandara_i18n::language().code();
            let current = wanted_edition(cx);
            let (tx, rx) = async_channel::bounded::<Result<String, String>>(1);
            cx.background_spawn(async move {
                let result = store::editions().map(|list| {
                    let mine: Vec<Value> = list
                        .into_iter()
                        .filter(|e| e.language == lang)
                        .map(|e| json!({ "identifier": e.identifier, "name": e.name, "english_name": e.english_name, "current": current.as_deref() == Some(e.identifier.as_str()) }))
                        .collect();
                    json!({ "language": lang, "editions": mine }).to_string()
                });
                let _ = tx.try_send(result);
            })
            .detach();
            Ok(Output::Pending(rx))
        }),
    );
}

// ── Registration ─────────────────────────────────────────────────────────

/// Plate radius of the ayah widget, reference px.
const AYAH_PLATE_RADIUS_PX: f32 = 30.0;
const PROGRESS_PLATE_RADIUS_PX: f32 = 26.0;

pub fn register(r: &mut PluginRegistry) {
    r.register(
        PluginManifest::builtin(ID, tk("Qur'an"), tk("Read the whole Qur'an with its meaning, listen to it recited, keep your place and your khatm (opt-in)"), Some(Zone::Right)).opt_in(),
        |window, cx, panel_cx| {
            let shared = model(cx);
            // The content downloads when the plugin is first switched on.
            ensure(&shared, cx);
            let bar = fn_view(None, plugin_lang::in_language(own_lang, bar_view), window, cx);
            let sidebar = fn_view(None, plugin_lang::in_language(own_lang, sidebar_view), window, cx);
            let panel = panel::build(panel_cx, window, cx);
            PluginInstance {
                bar: Some(bar),
                panel: Some(panel),
                sidebar: Some(sidebar),
                on_click: None,
                // A right-click pauses or resumes a recitation.
                on_secondary_click: Some(Rc::new(|_, cx| {
                    let shared = model(cx);
                    if toggle_pause(&shared) {
                        changed(&shared, cx);
                    }
                })),
            }
        },
    );
    // The Qur'an text is the plugin's model (search, commands, panel); the card keeps only its view.
    r.sidebar_card(ID, mandara_plugins::CardRelease::Views);
    // Registered after the plugin (registering clears a plugin's kinds).
    use mandara_config::desktop_widgets::{DesktopWidget, Look, Plate, QuranLayout, QuranWidget, WidgetKind};
    let one = |w: QuranWidget| vec![DesktopWidget { kind: WidgetKind::Quran(w), ..DesktopWidget::default() }];
    r.desktop_widget(
        DesktopWidgetType::new(ID, "ayah", tk("Ayah of the day"), tk("Today's ayah in calligraphy under an arch, with its meaning"), "book", move || {
            one(QuranWidget { look: Look { plate: Plate::Glass, plate_radius: AYAH_PLATE_RADIUS_PX, ..Look::default() }, ..QuranWidget::default() })
        })
        .in_menu(),
    );
    r.desktop_widget(DesktopWidgetType::new(ID, "progress", tk("Khatm progress"), tk("Your khatm as a ring, with the surah you're reading and a click to continue"), "book", move || {
        one(QuranWidget { layout: QuranLayout::Progress, look: Look { plate: Plate::Glass, plate_radius: PROGRESS_PLATE_RADIUS_PX, ..Look::default() }, ..QuranWidget::default() })
    }));
    register_commands(r);
}

/// A surah's name for a heading: Arabic in Arabic, else the
/// transliteration.
pub(crate) fn surah_title(book: &Book, n: u16) -> SharedString {
    book.q.surah(n).map(|s| if arabic_ui() { s.short_name().to_string() } else { s.transliteration.clone() }).unwrap_or_default().into()
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
        assert!(first.translation.is_empty());
        config.plugins.settings.entry(ID.into()).or_default().insert("translation".into(), toml::Value::String("demo-edition".into()));
        let changed = cache.get(&Arc::new(config), Lang::Ar, ());
        assert!(!Rc::ptr_eq(&first, &changed));
        assert_eq!(changed.lang, Lang::Ar);
        let held = SETTINGS.with(|cache| cache.borrow_mut().get(&snapshot, Lang::En, ()));
        clear_settings();
        assert!(!Rc::ptr_eq(&held, &SETTINGS.with(|cache| cache.borrow_mut().get(&snapshot, Lang::En, ()))));
        clear_settings();
        assert_eq!(changed.translation, "demo-edition");
    }


    #[test]
    fn sizes_grow() {
        assert!(ArabicSize::Small.reader_px() < ArabicSize::Medium.reader_px() && ArabicSize::Medium.reader_px() < ArabicSize::Large.reader_px());
        assert!(WAVE_MIN_PX < WAVE_MAX_PX && (0.0..1.0).contains(&PAUSED_SCALE));
    }

    #[test]
    fn marks_and_digits() {
        assert_eq!(ayah_mark(255), "\u{FD3F}٢٥٥\u{FD3E}");
        assert_eq!(link(Ref::new(2, 255)), "https://quran.com/2/255");
    }
}
