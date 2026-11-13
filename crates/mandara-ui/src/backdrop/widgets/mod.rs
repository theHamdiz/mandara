//! Desktop widgets on the software wallpaper canvas (`[desktop.widgets]`,
//! [`mandara_config::desktop_widgets`]): hero clocks, the system monitor
//! column, static decorations and the plane visualizer.
//!
//! Each widget owns two planes of its output's canvas, sized to what it
//! draws: a *static* plane (repainted when its content key changes — the
//! minute for clocks, never for decorations) and a *live* plane for what
//! changes in between (seconds, readings, the visualizer). Widgets set
//! `behind` sit between the wallpaper and the depth subject, the others
//! above it.
//!
//! Nothing here runs without enabled widgets: an empty list adds no plane,
//! no wake-up and no sampler. Live work also waits for the desktop to be
//! visible on that output (no fullscreen / tiled window over it, not
//! locked): the system monitor samples only then, the visualizer listens
//! only while music plays and stops after its silence timeout.

mod cache_key;
pub mod actions;
pub mod clock_cards;
pub mod date_cards;
pub mod datecal;
pub mod draw;
pub mod empty;
pub mod faces;
pub mod feeds;
pub mod frame;
pub mod fun;
pub mod gallery;
pub mod hadith;
pub mod identity;
pub mod info;
pub mod jannah;
pub mod layout;
pub mod lingo;
pub mod media_view;
pub mod monitor_more;
pub mod passive;
pub mod productive;
pub mod progress;
pub mod quran;
pub mod sayings;
pub mod sky;
pub mod skyview;
pub mod sysmon;
pub mod texts;
pub mod visualizer;
pub mod weather_cards;
pub mod weather_more;

use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike as _, Local, Timelike as _};
use image::RgbImage;
use mandara_config::Units;
use mandara_config::desktop_widgets::{ClockFace, ClockWidget, Depth, DesktopWidget, Insets, MonitorRow, MonitorStyle, MonitorWidget, WidgetKind, ranges};
use mandara_core::shm_canvas::{Place, Planes, Rect, snap};
use mandara_i18n::tk;
use mandara_theme::Color;
use tiny_skia::Pixmap;

use self::draw::{Inks, Measurer, Style};
use self::layout::{ClockStrings, Composition, MonitorLine};
use self::lingo::{HijriDate, Lingo};
use crate::backdrop::paint::FontNames;
use crate::lock::paint::TextPainter;
use crate::widgets::clock::arabic_numerals;
use crate::widgets::system::format_bytes;

/// Plane index of widget `i`'s static plane (its live plane is the next).
/// The depth clock uses the planes below.
pub const PLANE_BASE: usize = super::video::FILL_PLANE + 1;
/// Slack after a second / minute boundary before redrawing, so the new
/// value is current.
const TICK_SLACK: Duration = Duration::from_millis(15);
const MS_PER_SECOND: i64 = 1000;
const MS_PER_MINUTE: i64 = 60_000;
/// Share of a widget the subject may cover before the widget is drawn in
/// front of it instead of behind.
const HIDDEN_BEHIND_COVERAGE: f32 = 0.6;
/// Poll period while a visualizer waits for sound (music playing).
const VIS_POLL: Duration = Duration::from_millis(250);
/// Retry period of the spectrum subscription.
const SPECTRUM_RETRY: Duration = Duration::from_secs(2);
/// Region around a widget sampled for its automatic ink, as a fraction of
/// the box (centre ± half).
const TONE_HALF: f32 = 0.5;
/// Hours where the greeting changes.
const MORNING_FROM: u32 = 5;
const AFTERNOON_FROM: u32 = 12;
const EVENING_FROM: u32 = 17;
const NIGHT_FROM: u32 = 21;
const SECONDS_PER_DAY: f32 = 86_400.0;
const SECONDS_PER_HOUR: f32 = 3_600.0;
/// Hours on a 12-hour clock.
const HALF_DAY: u32 = 12;

pub fn planes(i: usize) -> (usize, usize) {
    (PLANE_BASE + 2 * i, PLANE_BASE + 2 * i + 1)
}

pub(crate) fn key(parts: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

/// What the renderer needs from the shell for the widgets.
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetScene {
    /// Enabled widgets the canvas paints, in order.
    pub items: Vec<DesktopWidget>,
    pub on_surface: Color,
    /// `(icon, temperature)` of the current weather, when known.
    pub weather: Option<(&'static str, String)>,
    /// Outputs where the desktop is visible (live widgets run only there);
    /// filled only while a live widget exists.
    pub visible: Vec<String>,
    /// A player is playing (filled only while a visualizer exists).
    pub music: bool,
    pub units: Units,
    pub h24: bool,
    pub rtl: bool,
    /// The widget being dragged in edit mode: its planes move with the
    /// pointer as they are (a subsurface move, no repaint) and are painted
    /// once more where it lands.
    pub moving: Option<String>,
    /// The theme's surface (widget plates).
    pub surface: Color,
    /// Today's Hijri date (the day begins at Maghrib), while a widget may
    /// show it.
    pub hijri: Option<HijriDate>,
    /// The weather in full, while a weather widget shows.
    pub forecast: Option<Forecast>,
    /// The World clock plugin's cities (a world clock widget without its
    /// own list shows them).
    pub world_cities: Vec<String>,
    /// The week's first day, days after Monday.
    pub week_start: u32,
    /// What the newer widgets show, filled only while one needs it.
    pub data: WidgetData,
}

/// Data of the widget collection's kinds, from the shell's services.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WidgetData {
    /// Every to-do, open first, and the lists.
    pub todos: Vec<TodoItem>,
    pub lists: Vec<(u64, String)>,
    /// Notes, most recently edited first.
    pub notes: Vec<NoteItem>,
    pub pomodoro: Option<PomodoroState>,
    /// Each widget's turns of its "next" (photos, quotes, the pet).
    pub turns: std::collections::BTreeMap<String, u32>,
    /// Calendar events from today on, soonest first.
    pub events: Vec<EventItem>,
    /// The location (latitude, longitude) the sky widgets use.
    pub place: Option<(f64, f64)>,
    /// You (the profile card).
    pub me: Option<MeInfo>,
    /// Each shortcut widget's tiles.
    pub tiles: std::collections::BTreeMap<String, Vec<TileItem>>,
    /// Quick toggles and whether each is on, volume and brightness.
    pub toggles: Vec<(String, bool)>,
    pub levels: Vec<(String, f32)>,
    /// The theme's colours: role name (a catalog key) and `0xRRGGBB`.
    pub palette: Vec<(String, u32)>,
    /// Each photo frame's and sticker's picture.
    pub pictures: std::collections::BTreeMap<String, layout::Picture>,
    /// When each widget was last clicked for its "next" (Unix ms): the
    /// desk pet reacts for a moment.
    pub poked: std::collections::BTreeMap<String, i64>,
    /// What plays (while a player is loaded and a media widget shows).
    pub media: Option<MediaInfo>,
    pub fetch: Option<FetchInfo>,
    pub screen: Option<ScreenInfo>,
    pub prayer: Option<PrayerInfo>,
    /// Today's Jannah reminder and step (while a Jannah widget shows).
    pub jannah: Option<JannahInfo>,
    /// Today's hadith and the reading progress (while a Hadith widget
    /// shows).
    pub hadith: Option<HadithInfo>,
    /// The ayah of the day and your khatm (while a Qur'an widget shows).
    pub quran: Option<QuranInfo>,
    pub glance: Option<GlanceInfo>,
    /// Each GitHub, exchange rate and AI usage widget's feed, by widget id.
    pub github: std::collections::BTreeMap<String, Feed<Vec<mandara_services::desktop_feeds::ContributionDay>>,
    >,
    pub rates: std::collections::BTreeMap<String, Feed<mandara_services::desktop_feeds::Rates>>,
    pub ai_usage: std::collections::BTreeMap<String, Feed<std::collections::BTreeMap<chrono::NaiveDate, mandara_services::desktop_feeds::DayTokens,
            >,
        >,
    >,
    pub games: Feed<Vec<GameItem>>,
}

/// Something fetched for a widget: what came, why it didn't, or that it's
/// on its way.
#[derive(Clone, Debug, PartialEq)]
pub struct Feed<T> {
    pub value: Option<T>,
    pub error: Option<String>,
    pub loading: bool,
}

impl<T> Default for Feed<T> {
    fn default() -> Self {
        Self { value: None, error: None, loading: false }
    }
}

/// An installed game for the games shelf.
#[derive(Clone, Debug, PartialEq)]
pub struct GameItem {
    pub appid: u32,
    pub name: String,
    pub cover: Option<layout::Picture>,
}

/// The machine, for the fetch card.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FetchInfo {
    pub user: String,
    pub host: String,
    pub os: String,
    pub kernel: String,
    pub desktop: String,
    pub cpu: String,
    pub gpu: String,
    /// Memory used and total, bytes.
    pub memory: (u64, u64),
    pub uptime_s: u64,
    pub shell: String,
    pub screen: String,
    pub logo: Option<layout::Picture>,
}

/// Screen time: day names with their totals (today last, flagged) and
/// today's top apps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScreenInfo {
    pub days: Vec<(String, u64, bool)>,
    pub top: Vec<(String, u64)>,
}

/// Today's prayer times: name (catalog key), Arabic name, time; tomorrow's
/// Fajr and today's Hijri date.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrayerInfo {
    pub times: Vec<(&'static str, &'static str, chrono::NaiveTime)>,
    pub tomorrow_fajr: chrono::NaiveTime,
    /// Today's Hijri date (the day turns at Maghrib); the widget writes it
    /// in its own language, digits and calendar.
    pub hijri: Option<lingo::HijriDate>,
    /// The plugin's own language (`None`: the interface's).
    pub lang: Option<mandara_i18n::Lang>,
}

/// The Hadith plugin's widgets: today's hadith and the reading progress
/// through each collection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HadithInfo {
    pub today: Option<HadithCard>,
    pub progress: Vec<HadithProgress>,
    /// The plugin's own language (`None`: the interface's).
    pub lang: Option<mandara_i18n::Lang>,
}

/// The hadith of the day as the desktop shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HadithCard {
    /// The words without the chain, verbatim.
    pub words: String,
    /// The meaning (without "Narrated X:") when shown, and its narrator.
    pub meaning: Option<String>,
    /// English stands in for the interface language.
    pub english: bool,
    pub narrator: Option<String>,
    /// `Sahih al-Bukhari 1` and `صحيح البخاري ١`.
    pub reference: String,
    pub reference_ar: String,
    /// The lead grade: its family, its words (empty for the two Sahih
    /// collections) and its colour (`0xRRGGBB`).
    pub grade: Option<(mandara_services::hadith::GradeClass, String, u32)>,
    /// Its key (`bukhari:1`): a click opens it in the reader.
    pub open: String,
}

/// Reading progress through one collection.
#[derive(Clone, Debug, PartialEq)]
pub struct HadithProgress {
    pub collection: mandara_config::widget_kinds::HadithCollection,
    /// The English name (a catalog key) and the Arabic.
    pub title: String,
    pub arabic: String,
    pub read: usize,
    pub total: usize,
    pub downloaded: bool,
    /// Where you were (`Bukhari 52`).
    pub last: Option<String>,
}

/// Today's Jannah reminder (as the plugin picked it for the desktop) and
/// today's step.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JannahInfo {
    /// The Arabic (a hadith's words without its chain), verbatim.
    pub arabic: String,
    /// A verse's ornate number (`﴿٢٥﴾`); none for a hadith.
    pub ayah_mark: Option<String>,
    /// The meaning in the interface language, when shown and known.
    pub meaning: Option<String>,
    /// The meaning is an AI rendering (labelled as such).
    pub ai: bool,
    /// The reference (`Al-Baqara 2:25`) and in Arabic (`البقرة ٢:٢٥`).
    pub reference: String,
    pub reference_ar: String,
    /// The item's key (`quran-2:25`): a click opens it in the reader
    /// (empty: the panel).
    pub open: String,
    /// Today's step: its title and line (catalog keys) and its check.
    pub step_title: String,
    pub step_line: String,
    pub step_done: bool,
    /// This week's days, done or not, and which is today.
    pub week: [bool; 7],
    pub today: usize,
    /// The plugin's own language (`None`: the interface's).
    pub lang: Option<mandara_i18n::Lang>,
}

/// The Qur'an plugin's ayah of the day and where you are in your khatm.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuranInfo {
    /// The ayah of the day, verbatim, and its ornate number (`﴿٢٥٥﴾`).
    pub arabic: String,
    pub ayah_mark: String,
    /// Its meaning in the interface language, when downloaded.
    pub meaning: Option<String>,
    /// Its reference (`Al-Baqara 2:255`) and in Arabic (`البقرة ٢:٢٥٥`).
    pub reference: String,
    pub reference_ar: String,
    /// `2:255`: a click on the ayah opens it in the reader.
    pub open: String,
    /// This khatm's progress (0–1) and the khatms completed.
    pub progress: f32,
    pub khatms: u32,
    /// Where you read: the surah (Arabic and transliterated), the ayah,
    /// its ayah count and juz.
    pub surah_ar: String,
    pub surah: String,
    pub ayah: u16,
    pub ayahs: u16,
    pub juz: u8,
    /// The ayah being recited (`2:255`), while a recitation plays.
    pub reciting: Option<String>,
    /// The plugin's own language (`None`: the interface's).
    pub lang: Option<mandara_i18n::Lang>,
}

/// The one thing worth knowing now: an icon, a sentence and what a click
/// does.
#[derive(Clone, Debug, PartialEq)]
pub struct GlanceInfo {
    pub icon: &'static str,
    pub text: String,
    pub action: Option<actions::Action>,
}

/// The track a media widget shows.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaInfo {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    /// Position at `at_ms` (Unix ms), the playback rate and the length.
    pub position_ms: i64,
    pub at_ms: i64,
    pub rate: f64,
    pub length_ms: i64,
    /// A live stream: no progress line, and "Live" for the time.
    pub live: bool,
    pub art: Option<layout::Picture>,
    /// Synced lyric lines (ms, text), when a lyrics widget asked.
    pub lyrics: Vec<(i64, String)>,
    pub lyrics_loading: bool,
}

/// You, for the profile card.
#[derive(Clone, Debug, PartialEq)]
pub struct MeInfo {
    pub name: String,
    pub login: String,
    pub host: String,
    pub uptime_s: u64,
    pub avatar: Option<layout::Picture>,
    pub dnd: bool,
}

/// One launch tile.
#[derive(Clone, Debug, PartialEq)]
pub struct TileItem {
    pub label: String,
    pub action: actions::Action,
    /// The app's own icon, else a bundled glyph.
    pub icon: Option<layout::Picture>,
    pub glyph: &'static str,
}

/// A calendar event.
#[derive(Clone, Debug, PartialEq)]
pub struct EventItem {
    /// Its first and last day (local).
    pub day: chrono::NaiveDate,
    pub last_day: chrono::NaiveDate,
    /// Unix ms.
    pub start_ms: i64,
    pub end_ms: i64,
    pub all_day: bool,
    pub title: String,
    /// Its calendar's colour (`0xRRGGBB`).
    pub color: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TodoItem {
    pub id: u64,
    pub text: String,
    pub done: bool,
    pub list: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NoteItem {
    pub title: String,
    /// The text after the title (cut short).
    pub body: String,
}

/// The focus timer, as wall-clock times so the canvas counts down on its
/// own.
#[derive(Clone, Debug, PartialEq)]
pub struct PomodoroState {
    /// The phase (an English catalog key).
    pub phase: &'static str,
    /// When it ends (Unix ms) while running.
    pub ends_ms: Option<i64>,
    /// Time left while paused, ms.
    pub paused_ms: i64,
    pub length_ms: i64,
    pub running: bool,
    pub completed: u32,
    /// Focus sessions in a set (a long break follows).
    pub long_every: u32,
}

/// What the weather widget shows, from the weather plugin.
#[derive(Clone, Debug, PartialEq)]
pub struct Forecast {
    pub icon: &'static str,
    /// °C.
    pub temp: f32,
    pub feels_like: f32,
    pub humidity: u8,
    /// km/h.
    pub wind: f32,
    /// The condition (an English catalog key).
    pub condition: String,
    pub place: String,
    /// Coming days: date, icon, min and max °C.
    pub days: Vec<(chrono::NaiveDate, &'static str, f32, f32)>,
    /// The coming hours: hour of the day, icon, °C.
    pub hours: Vec<(u32, &'static str, f32)>,
    /// The condition as an index (clear, partly cloudy, cloudy, fog,
    /// drizzle, rain, snow, thunder) and whether the sun is up.
    pub kind: usize,
    pub is_day: bool,
    /// The forecast's date boundary; unknown zones follow the local clock.
    pub timezone: Option<mandara_services::world_clock::Tz>,
}

impl Forecast {
    /// Shared projection of the weather actor for desktop and public widgets.
    pub fn from_weather(w: &mandara_services::weather::Weather, place: &str) -> Self {
        Self {
            icon: crate::plugins::weather::condition_icon(w.condition, w.is_day),
            temp: w.temp, feels_like: w.feels_like, humidity: w.humidity, wind: w.wind,
            condition: mandara_i18n::untranslate(w.condition.label()), place: place.into(),
            days: w.days.iter().filter_map(|d| {
                    Some((chrono::NaiveDate::parse_from_str(&d.date, "%Y-%m-%d").ok()?, crate::plugins::weather::condition_icon(d.condition, true), d.min, d.max))
                }).collect(),
            hours: w.hours.iter().map(|h| {
                    (h.hour, crate::plugins::weather::condition_icon(h.condition, h.is_day), h.temp)
                }).collect(),
            kind: w.condition as usize, is_day: w.is_day, timezone: w.timezone.parse().ok(),
        }
    }
}

impl Default for WidgetScene {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            on_surface: Color::oklch(0.95, 0.0, 0.0),
            weather: None,
            visible: Vec::new(),
            music: false,
            units: Units::default(),
            h24: true,
            rtl: false,
            moving: None,
            surface: Color::oklch(0.22, 0.02, 250.0),
            hijri: None,
            forecast: None,
            world_cities: Vec::new(),
            week_start: 0,
            data: WidgetData::default(),
        }
    }
}

/// What changes a widget's pixels: its settings and size, not where it
/// sits (moving it moves its planes). The anchor stays in (it sets the
/// text alignment), and so does the way vertical text turns (it follows
/// the side of the screen).
pub fn content_key(w: &DesktopWidget) -> u64 {
    let place = &w.place;
    let turns = match &w.kind {
        WidgetKind::Decoration(decoration) => decoration.vertical && decoration.sideways.clockwise(place),
        WidgetKind::Notebook(_) => mandara_config::desktop_widgets::Sideways::Auto.clockwise(place),
        _ => false,
    };
    key((
        (&w.id,w.enabled,cache_key::debug(&w.kind),cache_key::debug(&place.anchor)),
        (place.scale.to_bits(),place.opacity.to_bits(),place.rotation.to_bits(),place.flip_x,place.flip_y,turns),
    ))
}

/// Physical px a placed box may drift from where it was painted before it
/// counts as moved.
const MOVE_EPSILON_PX: f32 = 0.01;

/// `r` moved by `(dx, dy)` rounded to the canvas grid (subsurfaces sit on
/// whole logical pixels).
fn shifted(r: Rect, (dx, dy): (f32, f32), scale120: u32) -> Rect {
    let g = mandara_core::shm_canvas::grid_step(scale120).max(1) as f32;
    let step = |d: f32| ((d / g).round() * g) as i32;
    Rect { x: r.x + step(dx), y: r.y + step(dy), ..r }
}

impl PomodoroState {
    /// A timer that hasn't started: 25 minutes of focus.
    pub fn idle() -> Self {
        Self { phase: tk("Focus"), ends_ms: None, paused_ms: IDLE_FOCUS_MS, length_ms: IDLE_FOCUS_MS, running: false, completed: 0, long_every: IDLE_LONG_EVERY }
    }
}

/// The focus timer's defaults (previews, before the store is read).
const IDLE_FOCUS_MS: i64 = 25 * 60 * 1000;
const IDLE_LONG_EVERY: u32 = 4;

impl WidgetScene {
    /// A scene for previews and Studio's gallery: `items` with example
    /// weather, today's Hijri date, the default world cities and the
    /// information widgets' examples ([`gallery::info`]; nothing is read).
    pub fn preview(items: Vec<DesktopWidget>) -> Self {
        let today = Local::now().date_naive();
        let mut scene = Self {
            items,
            weather: Some(("sun", "23°".into())),
            hijri: Some(lingo::hijri_of(today)),
            forecast: Some(texts::sample_forecast(today)),
            world_cities: mandara_services::world_clock::DEFAULT_CITIES.iter().map(|c| c.to_string()).collect(),
            rtl: mandara_i18n::language().is_rtl(),
            ..Self::default()
        };
        let items = std::mem::take(&mut scene.items);
        for w in &items {
            gallery::info::populate(&mut scene, w, None, 0);
        }
        scene.items = items;
        scene
    }

    /// Any widget below the depth subject (the depth mask is needed).
    pub fn any_behind(&self) -> bool {
        self.items.iter().any(|w| w.place.depth == Depth::Behind)
    }

    /// Any widget whose content changes between minutes.
    pub fn any_live(&self) -> bool {
        self.items.iter().any(|w| is_live(&w.kind))
    }

    pub fn any_visualizer(&self) -> bool {
        self.items.iter().any(|w| matches!(w.kind, WidgetKind::Visualizer(_)))
    }
}

fn is_live(k: &WidgetKind) -> bool {
    match k {
        WidgetKind::Clock(c) => ticks_seconds(c),
        WidgetKind::Monitor(_) | WidgetKind::Visualizer(_) | WidgetKind::Pomodoro(_) | WidgetKind::Pet(_) | WidgetKind::Bubbles(_) | WidgetKind::Media(_) => true,
        _ => false,
    }
}

/// Frames per second of an animated widget (the pet, bubbles) at `now`.
fn animation_fps(w: &DesktopWidget, scene: &WidgetScene, now: &DateTime<Local>) -> Option<u32> {
    match &w.kind {
        WidgetKind::Pet(_) => Some(if fun::asleep(now) && !scene.data.poked.contains_key(&w.id) { fun::PET_SLEEP_FPS } else { fun::PET_FPS }),
        WidgetKind::Bubbles(_) => Some(fun::BUBBLES_FPS),
        WidgetKind::Media(mw) if mw.layout == mandara_config::desktop_widgets::MediaLayout::Vinyl && mw.spin && scene.data.media.as_ref().is_some_and(|m| m.playing) =>
        {
            Some(media_view::VINYL_FPS)
        }
        _ => None,
    }
}

/// The toggles a widget shows (its own list, else four common ones).
pub fn toggle_ids(w: &mandara_config::desktop_widgets::TogglesWidget) -> Vec<String> {
    if w.toggles.is_empty() { ["wifi", "bluetooth", "dnd", "night-light"].map(String::from).to_vec() } else { w.toggles.clone() }
}


/// What else `w` shows from the scene (the forecast, the cities), hashed.
fn data_key(w: &DesktopWidget, scene: &WidgetScene) -> u64 {
    match &w.kind {
        WidgetKind::Weather(_) => cache_key::debug(&scene.forecast),
        WidgetKind::WorldClock(_) => key(&scene.world_cities),
        WidgetKind::Todo(_) => cache_key::debug(&(&scene.data.todos,&scene.data.lists)),
        WidgetKind::Note(_) | WidgetKind::Notebook(_) => cache_key::debug(&scene.data.notes),
        WidgetKind::Pomodoro(_) => cache_key::debug(&scene.data.pomodoro),
        WidgetKind::Quote(_) => key(scene.data.turns.get(&w.id)),
        WidgetKind::Date(_) => cache_key::debug(&scene.data.events),
        WidgetKind::Sky(_) => cache_key::debug(&scene.data.place),
        WidgetKind::Profile(_) => cache_key::debug(&scene.data.me),
        WidgetKind::Shortcuts(_) => cache_key::debug(&scene.data.tiles.get(&w.id)),
        WidgetKind::Toggles(_) => cache_key::debug(&scene.data.toggles),
        WidgetKind::Sliders(_) => cache_key::debug(&scene.data.levels),
        WidgetKind::Palette(_) => cache_key::debug(&scene.data.palette),
        WidgetKind::Photo(_) | WidgetKind::Sticker(_) => {
            cache_key::debug(&scene.data.pictures.get(&w.id))
        }
        WidgetKind::Fetch(_) => cache_key::debug(&(&scene.data.fetch,&scene.data.palette)),
        WidgetKind::ScreenTime(_) => cache_key::debug(&scene.data.screen),
        WidgetKind::Prayer(_) => cache_key::debug(&scene.data.prayer),
        WidgetKind::Jannah(_) => cache_key::debug(&scene.data.jannah),
        WidgetKind::Hadith(_) => cache_key::debug(&scene.data.hadith),
        WidgetKind::Quran(_) => cache_key::debug(&scene.data.quran),
        WidgetKind::Glance(_) => cache_key::debug(&(&scene.data.glance,&scene.weather)),
        WidgetKind::Github(_) => cache_key::debug(&scene.data.github.get(&w.id)),
        WidgetKind::Currency(_) => cache_key::debug(&scene.data.rates.get(&w.id)),
        WidgetKind::AiUsage(_) => cache_key::debug(&scene.data.ai_usage.get(&w.id)),
        WidgetKind::Games(_) => cache_key::debug(&scene.data.games),
        // The track, the art and the lyrics; not the moving position.
        WidgetKind::Media(_) => cache_key::debug(&scene.data.media.as_ref().map(|m| (&m.title, &m.artist, m.playing, &m.art, &m.lyrics, m.lyrics_loading, m.length_ms, m.live))),
        _ => 0,
    }
}

/// A clock face that shows seconds (its live plane redraws each second).
fn ticks_seconds(c: &ClockWidget) -> bool {
    c.seconds && matches!(c.face, ClockFace::Superscript | ClockFace::Analog)
}

/// One widget's planes on one output.
#[derive(Default)]
struct State {
    id: String,
    /// [`content_key`]: everything that changes the pixels (not where the
    /// widget sits).
    cfg: u64,
    static_key: Option<u64>,
    live_key: Option<u64>,
    rect: Rect,
    live_rect: Rect,
    /// Where the planes were painted: the placed box's top-left (physical
    /// px) and the two rectangles, before any [`Planes::move_plane`].
    painted_at: (f32, f32),
    painted_rects: (Rect, Rect),
    /// Where the plane content may reach (grows; the depth cut covers it).
    envelope: Rect,
    behind: bool,
    /// The box [`mandara_config::desktop_widgets::Placement::origin_in`]
    /// placed (physical `x, y, w, h`): edit mode moves it.
    outer: (f32, f32, f32, f32),
    vis: Option<(visualizer::Geometry, visualizer::Painter, f32)>,
    frame_at: Option<Instant>,
    inks: Option<Inks>,
    /// Where a click does something, as last painted.
    hits: Option<HitMap>,
}

/// A painted widget's clickable areas and how it was placed and turned.
#[derive(Clone, Debug)]
struct HitMap {
    hits: Vec<layout::Hit>,
    origin: (f32, f32),
    size: (f32, f32),
    rotation: f32,
    flip: (bool, bool),
}

impl HitMap {
    fn of(l: &Laid) -> Option<Self> {
        (!l.comp.hits.is_empty()).then(|| HitMap { hits: l.comp.hits.clone(), origin: l.origin, size: l.comp.size, rotation: l.rotation, flip: l.flip })
    }

    fn hit(&self, p: (f32, f32)) -> Option<&actions::Action> {
        let laid = Laid { comp: Composition { hits: self.hits.clone(), size: self.size, ..Composition::default() }, origin: self.origin, k: 1.0, rotation: self.rotation, flip: self.flip, outer: (0.0, 0.0, 0.0, 0.0), margin: 0.0 };
        let found = laid.hit(p)?.clone();
        self.hits.iter().map(|h| &h.action).find(|a| **a == found)
    }
}

/// The widgets of one output.
#[derive(Default)]
pub struct OutWidgets {
    states: Vec<State>,
}

impl OutWidgets {
    pub fn clear(&mut self) {
        self.states = Vec::new();
    }

    /// The current painted bounds, without building an edit-mode inventory.
    pub fn bounds(&self, id: &str) -> Option<Rect> {
        self.states.iter().find(|state| state.id == id).map(|state| state.rect.union(&state.live_rect)).filter(|rect| !rect.is_empty())
    }

    /// Each shown widget's id, painted bounds and placed box (physical
    /// px), for edit mode.
    pub fn geometry(&self) -> Vec<(String, Rect, (f32, f32, f32, f32))> {
        self.states
            .iter()
            .filter(|st| !st.id.is_empty())
            .map(|st| (st.id.clone(), st.rect.union(&st.live_rect), st.outer))
            .filter(|(_, r, _)| !r.is_empty())
            .collect()
    }

    /// The widget and action under `p` (physical px), the topmost widget
    /// first.
    pub fn hit(&self, p: (f32, f32)) -> Option<(String, actions::Action)> {
        self.states.iter().rev().filter(|st| !st.id.is_empty()).find_map(|st| st.hits.as_ref()?.hit(p).map(|a| (st.id.clone(), a.clone())))
    }

    /// Removes every widget plane of output `id` (buffers and surfaces)
    /// and forgets the widgets.
    pub fn remove_all(&mut self, host: &mut impl Planes, id: u32) {
        remove_unused(host, id, &Layers::default());
        self.states = Vec::new();
    }
}

/// Removes every widget plane of output `id` that `layers` doesn't stack:
/// the plane set then matches the shown widgets exactly, whatever changed
/// (a widget removed, disabled, moved to another display or index, or
/// turned into a kind that uses fewer planes), and removed planes keep no
/// buffer or surface.
fn remove_unused(host: &mut impl Planes, id: u32, layers: &Layers) {
    for i in 0..ranges::MAX_WIDGETS {
        let (a, b) = planes(i);
        for p in [a, b] {
            if !layers.behind.contains(&p) && !layers.front.contains(&p) && host.has_plane(id, p) {
                host.remove_plane(id, p);
            }
        }
    }
}

/// Renderer-wide widget services.
#[derive(Default)]
pub struct Shared {
    pub sampler: Option<sysmon::Sampler>,
    tap: Option<mandara_services::spectrum::Spectrum>,
    tap_tried: Option<Instant>,
    silent_since: Option<Instant>,
    /// Stopped listening after the silence timeout (until music restarts).
    silenced: bool,
    music_was: bool,
}

impl Shared {
    pub fn listening(&self) -> bool {
        self.tap.is_some()
    }
}

/// Planes the widgets of an output contribute to its stacking order and
/// the area the depth subject must cover.
#[derive(Default)]
pub struct Layers {
    pub behind: Vec<usize>,
    pub front: Vec<usize>,
    pub reach: Rect,
}

/// An output as the widgets see it.
pub struct OutputCx<'a> {
    pub id: u32,
    pub names: &'a [&'a str],
    pub logical: (f32, f32),
    pub physical: (u32, u32),
    pub scale120: u32,
    /// Small copy of the wallpaper for automatic ink.
    pub tone: Option<&'a RgbImage>,
    pub primary: Color,
    pub fonts: &'a FontNames,
    /// Depth effect available (a subject cut-out is drawn).
    pub depth: bool,
    /// The shown subject's coverage map (output fractions), once cut out.
    pub subject: Option<&'a image::GrayImage>,
    /// Edges widgets keep clear of (the bar, the screen frame), physical px.
    pub safe: Insets,
}

impl OutputCx<'_> {
    fn output_rect(&self) -> Rect {
        Rect::new(0, 0, self.physical.0, self.physical.1)
    }

    /// Physical px per reference px at `scale`.
    fn k(&self, scale: f32) -> f32 {
        self.physical.1 as f32 / ranges::REFERENCE_HEIGHT_PX * scale
    }
}

/// Gap kept between desktop widgets and the bar / screen edges, logical px.
const SAFE_MARGIN_PX: f32 = 8.0;

/// What the desktop clock and widgets keep clear of on an output, logical
/// px: the bar's exclusive zone and the screen frame (where the bar shows),
/// plus a small margin from every edge.
pub fn safe_insets(cfg: &mandara_config::ShellConfig, bar_here: bool) -> Insets {
    use mandara_config::BarPosition;
    let mut i = Insets::default();
    if bar_here {
        let m = crate::bar::layout::Metrics::of(cfg, 1.0);
        let frame = m.screen_frame.max(0.0);
        i = Insets { top: frame, right: frame, bottom: frame, left: frame };
        let zone = crate::bar::layout::exclusive_zone(&m);
        match cfg.bar.position {
            BarPosition::Top => i.top = zone,
            BarPosition::Bottom => i.bottom = zone,
            BarPosition::Left => i.left = zone,
            BarPosition::Right => i.right = zone,
        }
        // Pinned side bars (`[side_bars]`) keep their edges too.
        for right in [false, true] {
            if cfg.side_bar_on(right) && cfg.side_bars.side(right).pinned {
                let depth = crate::bar::layout::exclusive_zone(&crate::bar::side::metrics(cfg, right, 1.0));
                if right { i.right = i.right.max(depth) } else { i.left = i.left.max(depth) }
            }
        }
    }
    Insets { top: i.top + SAFE_MARGIN_PX, right: i.right + SAFE_MARGIN_PX, bottom: i.bottom + SAFE_MARGIN_PX, left: i.left + SAFE_MARGIN_PX }
}

fn shows(w: &DesktopWidget, o: &OutputCx<'_>) -> bool {
    w.place.shows_on(o.names)
}

// ── Strings ──────────────────────────────────────────────────────────────

/// The greeting for `hour` (an English catalog key).
pub fn greeting(hour: u32) -> &'static str {
    match hour {
        h if (MORNING_FROM..AFTERNOON_FROM).contains(&h) => tk("Good morning"),
        h if (AFTERNOON_FROM..EVENING_FROM).contains(&h) => tk("Good afternoon"),
        h if (EVENING_FROM..NIGHT_FROM).contains(&h) => tk("Good evening"),
        _ => tk("Good night"),
    }
}

/// Every string of a clock at `now`, in the clock's language, digits and
/// calendar (`hijri`: today's Hijri date).
#[allow(clippy::too_many_arguments)]
pub fn clock_strings(now: &DateTime<Local>, c: &ClockWidget, h24: bool, weather: Option<&(&'static str, String)>, hijri: Option<HijriDate>, ui_rtl: bool, week_start: u32) -> ClockStrings {
    let probe = Lingo::of(&c.locale, ui_rtl, None);
    let l = Lingo { eastern: c.eastern(probe.arabic()), ..probe };
    let (h, m, s) = (now.hour(), now.minute(), now.second());
    let h12 = match h % HALF_DAY {
        0 => HALF_DAY,
        v => v,
    };
    // Stacked hours balance better as two digits.
    let padded = matches!(c.face, ClockFace::Superscript | ClockFace::Stack | ClockFace::DotMatrix | ClockFace::Pixel | ClockFace::Boxes | ClockFace::Numerals);
    let hours = if h24 { format!("{h:02}") } else if padded { format!("{h12:02}") } else { h12.to_string() };
    let minutes = format!("{m:02}");
    let date = now.date_naive();
    let secs = now.num_seconds_from_midnight() as f32;
    // The Hijri calendar alone names its own day and month.
    let hijri_only = c.locale.calendar == mandara_config::desktop_widgets::Calendar::Hijri;
    let (day, month) = match hijri.filter(|_| hijri_only) {
        Some(hd) => (l.hijri_digits(hd.2), l.hijri_month(hd.1)),
        None => (l.digits(date.day().to_string()), l.date(&date, "%B")),
    };
    ClockStrings {
        time: l.digits(format!("{hours}:{minutes}")),
        hours: l.digits(hours),
        minutes: l.digits(minutes),
        seconds: l.digits(format!("{s:02}")),
        meridiem: (!h24).then(|| l.word(if h < HALF_DAY { tk("AM") } else { tk("PM") })),
        weekday: l.date(&date, "%A"),
        weekday_short: l.date(&date, "%a"),
        day,
        month,
        numeric_date: match hijri.filter(|_| c.locale.calendar.hijri()) {
            Some((_, hm, hd)) if hijri_only => {
                let s = format!("{hd:02}/{hm:02}");
                if l.hijri_in_arabic { arabic_numerals(&s) } else { l.digits(s) }
            }
            _ => lingo::dated(&l, &date, tk("%d/%m"), hijri, face_separator()),
        },
        day_month: lingo::dated(&l, &date, tk("%-d %B"), hijri, face_separator()),
        greeting: c.phrases.get(greeting_slot(h)).map(|p| p.trim()).filter(|p| !p.is_empty()).map_or_else(|| l.word(greeting(h)), str::to_string),
        weather: if c.weather { weather.map(|(i, temp)| (*i, l.digits(temp.clone()))) } else { None },
        hour_fraction: (m as f32 * 60.0 + s as f32) / SECONDS_PER_HOUR,
        day_fraction: secs / SECONDS_PER_DAY,
        rtl: l.rtl,
        hour: h,
        minute: m,
        second: s,
        hour_numbers: ANALOG_NUMBERS.map(|n| l.digits(n)),
        all_hours: std::array::from_fn(|i| {
            l.digits(if i == 0 { HALF_DAY } else { i as u32 }.to_string())
        }),
        meridiem_pair: [l.word(tk("AM")), l.word(tk("PM"))],
        year: l.digits(date.year().to_string()),
        month_short: l.date(&date, "%b"),
        month_number: l.digits(format!("{:02}", date.month())),
        day_part: l.word(day_part(h)),
        hijri: hijri.map(|hd| l.hijri_full(hd)).unwrap_or_default(),
        moon_phase: sky::moon_phase(now.timestamp()),
        week: week_of(&l, date, week_start),
    }
}

/// The greeting slot of `hour`: morning, afternoon, evening, night.
fn greeting_slot(hour: u32) -> usize {
    match hour {
        h if (MORNING_FROM..AFTERNOON_FROM).contains(&h) => 0,
        h if (AFTERNOON_FROM..EVENING_FROM).contains(&h) => 1,
        h if (EVENING_FROM..NIGHT_FROM).contains(&h) => 2,
        _ => 3,
    }
}

/// The part of the day of `hour` (an English catalog key).
pub fn day_part(hour: u32) -> &'static str {
    [tk("Morning"), tk("Afternoon"), tk("Evening"), tk("Night")][greeting_slot(hour)]
}

/// This week's short day names from its first day (`week_start` days
/// after Monday), and whether each is `today`.
fn week_of(l: &Lingo, today: chrono::NaiveDate, week_start: u32) -> Vec<(String, bool)> {
    let from_start = (today.weekday().num_days_from_monday() + 7 - week_start % 7) % 7;
    let first = today - chrono::Duration::days(i64::from(from_start));
    (0..7).map(|i| first + chrono::Duration::days(i)).map(|d| (l.date(&d, "%a"), d == today)).collect()
}

/// Where a decoration's lines line up by default: centred.
const DECORATION_ALIGN: f32 = 0.5;

/// The hours an analog dial numbers: the quarters.
const ANALOG_NUMBERS: [&str; 4] = ["12", "3", "6", "9"];

/// What joins two dates on one line.
fn face_separator() -> &'static str {
    layout::SEPARATOR
}

/// Full scale of the thermal arcs: degrees Celsius and fan RPM.
const TEMP_SCALE_C: f32 = 100.0;
/// The label plane's key covers this many GPUs' readings (four bits each),
/// then their count from bit [`GPU_COUNT_SHIFT`].
const KEYED_GPUS: usize = 8;
const GPU_MASK_BITS: usize = 4;
const GPU_COUNT_SHIFT: u32 = 56;
const GPU_COUNT_MAX: usize = 255;
const FAN_SCALE_RPM: f32 = 3000.0;

/// `6.1 / 31.2 GB`: both in the total's unit.
fn pair_bytes(used: u64, total: u64) -> String {
    let t = format_bytes(total);
    match t.split_once(' ') {
        Some((num, unit)) => {
            let scale = total as f64 / num.parse::<f64>().unwrap_or(1.0).max(f64::EPSILON);
            format!("{:.1} / {num} {unit}", used as f64 / scale)
        }
        None => format!("{} / {t}", format_bytes(used)),
    }
}

fn percent(v: f32) -> String {
    format!("{:.0}%", (v * 100.0).clamp(0.0, 999.0))
}

/// The monitor's rows for the current reading (rows without a reading are
/// left out once the sampler has had a full sample).
pub fn monitor_lines(w: &MonitorWidget, s: Option<&sysmon::Sampler>, units: Units, ui_rtl: bool) -> Vec<MonitorLine> {
    let l = Lingo::of(&w.locale, ui_rtl, None);
    let r = s.map(|s| &s.reading);
    let settled = s.is_some_and(|s| s.samples >= 2);
    let dash = || "—".to_string();
    // Gauges, meters and bars show memory as a share of the total.
    let gauges = w.style != MonitorStyle::Column;
    let share = |u: u64, tot: u64| u as f32 / tot.max(1) as f32;
    let mut out = Vec::new();
    for row in &w.rows {
        let row = *row;
        if row.is_gpu() {
            gpu_lines(row, r, &l, units, gauges, settled, &mut out);
            continue;
        }
        let line = |label: &str, value: Option<String>, share: Option<f32>| -> Option<MonitorLine> {
            if value.is_none() && settled {
                return None;
            }
            Some(MonitorLine { row: Some(row), label: l.word(label), value: l.digits(value.unwrap_or_else(dash)), share: share.filter(|_| gauges), ..Default::default() })
        };
        let rate = |v: Option<f64>| v.map(|b| format!("{}{}", format_bytes(b as u64), l.word(tk("/s"))));
        let made = match row {
            MonitorRow::Cpu => line(tk("CPU"), r.and_then(|r| r.cpu).map(percent), Some(r.and_then(|r| r.cpu).unwrap_or(0.0))),
            MonitorRow::Memory => {
                let mem = r.and_then(|r| r.memory);
                line(tk("Memory"), mem.map(|(u, tot)| {
                        if gauges { percent(share(u, tot)) } else { pair_bytes(u, tot)
                        }
                    }), Some(mem.map(|(u, tot)| share(u, tot)).unwrap_or(0.0))).map(|mut ln| {
                    ln.short = Some(l.word(tk("RAM")));
                    ln
                })
            }
            MonitorRow::Swap => {
                let swap = r.and_then(|r| r.swap);
                line(tk("Swap"), swap.map(|(u, tot)| {
                        if gauges { percent(share(u, tot)) } else { pair_bytes(u, tot)
                        }
                    }), Some(swap.map(|(u, tot)| share(u, tot)).unwrap_or(0.0)))
            }
            MonitorRow::CpuTemp => line(tk("CPU temp"), r.and_then(|r| r.cpu_temp).map(|c| units.format_degrees(c)), None).map(|mut ln| {
                ln.level = r.and_then(|r| r.cpu_temp).map(|c| c / TEMP_SCALE_C);
                ln.short = Some(l.word(tk("CPU")));
                ln
            }),
            // Handled above.
            MonitorRow::Gpu | MonitorRow::Vram | MonitorRow::GpuPower | MonitorRow::GpuTemp => None,
            MonitorRow::Fan => line(tk("Fan"), r.and_then(|r| r.fan_rpm).map(|v| format!("{v} {}", l.word(tk("RPM")))), None).map(|mut ln| {
                ln.level = r.and_then(|r| r.fan_rpm).map(|v| v as f32 / FAN_SCALE_RPM);
                ln
            }),
            MonitorRow::Battery => line(tk("Battery"), r.and_then(|r| r.battery).map(percent), Some(r.and_then(|r| r.battery).unwrap_or(0.0))).map(|mut ln| {
                ln.short = Some(l.word(tk("Batt.")));
                ln
            }),
            MonitorRow::Disk => {
                let disk = r.and_then(|r| r.disk);
                line(tk("Disk"), disk.map(|(u, tot)| {
                        if gauges { percent(share(u, tot)) } else { pair_bytes(u, tot)
                        }
                    }), Some(disk.map(|(u, tot)| share(u, tot)).unwrap_or(0.0))).map(|mut ln| {
                    ln.bar = (!gauges).then(|| disk.map(|(u, tot)| share(u, tot)).unwrap_or(0.0));
                    ln
                })
            }
            MonitorRow::Download => line(tk("Download"), rate(r.and_then(|r| r.down)), None),
            MonitorRow::Upload => line(tk("Upload"), rate(r.and_then(|r| r.up)), None),
            MonitorRow::CpuGraph => Some(MonitorLine { row: Some(row), graph: Some((s.map(|s| s.cpu_history.normalized(1.0)).unwrap_or_default(), None)), ..Default::default() }),
            MonitorRow::NetGraph => {
                let scale = s.map(|s| s.down_history.max().max(s.up_history.max())).unwrap_or(1.0).max(1.0);
                Some(MonitorLine { row: Some(row), graph: Some((s.map(|s| s.down_history.normalized(scale)).unwrap_or_default(), s.map(|s| s.up_history.normalized(scale)))), ..Default::default() })
            }
        };
        out.extend(made);
    }
    out
}

/// The lines of a GPU row: one per GPU that reports it (numbered when
/// there are several). Before the GPU sampler's first reading a single
/// placeholder holds the place; after it, a reading no GPU has shows no
/// line at all.
fn gpu_lines(row: MonitorRow, r: Option<&sysmon::Reading>, l: &Lingo, units: Units, gauges: bool, settled: bool, out: &mut Vec<MonitorLine>) {
    let gpus = r.map_or(&[][..], |r| &r.gpus[..]);
    let probed = r.is_some_and(|r| r.gpus_probed);
    if gpus.is_empty() {
        if !probed && !settled {
            out.extend(gpu_line(row, &sysmon::GpuReading::default(), None, l, units, gauges));
        }
        return;
    }
    let several = gpus.len() > 1;
    out.extend(gpus.iter().enumerate().filter_map(|(i, g)| gpu_line(row, g, several.then_some(i + 1), l, units, gauges)));
}

/// GPU `g`'s line for `row` (`n` numbers it among several), `None` when it
/// doesn't report that reading; an unread one (the placeholder) is a dash.
fn gpu_line(row: MonitorRow, g: &sysmon::GpuReading, n: Option<usize>, l: &Lingo, units: Units, gauges: bool) -> Option<MonitorLine> {
    let placeholder = *g == sysmon::GpuReading::default();
    let named = |single: &str, numbered: &str| match n {
        Some(n) => mandara_i18n::fill(&l.word(numbered), &[("n", &l.digits(n.to_string()))]),
        None => l.word(single),
    };
    let share = |(u, t): (u64, u64)| u as f32 / t.max(1) as f32;
    let (label, value, share, level, short) = match row {
        MonitorRow::Gpu => (named(tk("GPU"), tk("GPU {n}")), g.load.map(percent), Some(g.load.unwrap_or(0.0)), None, None),
        MonitorRow::Vram => (named(tk("VRAM"), tk("VRAM {n}")), g.vram.map(|v| {
                if gauges { percent(share(v)) } else { pair_bytes(v.0, v.1)
                }
            }), Some(g.vram.map(share).unwrap_or(0.0)), None, None),
        MonitorRow::GpuTemp => (named(tk("GPU temp"), tk("GPU {n} temp")), g.temp.map(|c| units.format_degrees(c)), None, g.temp.map(|c| c / TEMP_SCALE_C), Some(named(tk("GPU"), tk("GPU {n}")))),
        MonitorRow::GpuPower => (named(tk("GPU power"), tk("GPU {n} power")), g.power_w.map(|w| format!("{w:.0} W")), None, None, None),
        _ => return None,
    };
    if value.is_none() && !placeholder {
        return None;
    }
    Some(MonitorLine { row: Some(row), label, value: l.digits(value.unwrap_or_else(|| "—".to_string())), share: share.filter(|_| gauges), level, short, ..Default::default() })
}

// ── Layout of one widget ─────────────────────────────────────────────────

/// A widget laid out on an output: its composition, where its box goes
/// (physical, absolute) and how it is turned.
pub struct Laid {
    pub comp: Composition,
    pub origin: (f32, f32),
    pub k: f32,
    /// Degrees clockwise (the decoration's own turn plus the placement's).
    pub rotation: f32,
    /// Mirrored left to right / top to bottom.
    pub flip: (bool, bool),
    /// The placed (possibly rotated) outer box, `x, y, w, h`.
    pub outer: (f32, f32, f32, f32),
    /// Margin the look's effect and shadow need, physical px.
    pub margin: f32,
}

/// Lays out content and its shared frame at `now`.
pub fn lay_out(w: &DesktopWidget, o: &OutputCx<'_>, scene: &WidgetScene, sampler: Option<&sysmon::Sampler>, now: &DateTime<Local>, text: &mut TextPainter) -> Option<Laid> {
    let k = o.k(w.place.scale());
    let look = w.kind.look();
    let fonts = o.fonts.for_look(look);
    // A faith plugin's widget speaks the plugin's own language unless it has one.
    let plugin = match &w.kind {
        WidgetKind::Prayer(_) => scene.data.prayer.as_ref().and_then(|p| p.lang),
        WidgetKind::Jannah(_) => scene.data.jannah.as_ref().and_then(|j| j.lang),
        WidgetKind::Hadith(_) => scene.data.hadith.as_ref().and_then(|h| h.lang),
        WidgetKind::Quran(_) => scene.data.quran.as_ref().and_then(|q| q.lang),
        _ => None,
    };
    let rtl_of = |opts: Option<&mandara_config::desktop_widgets::TextOptions>| {
        opts.map_or(scene.rtl, |opts| {
            Lingo::of_plugin(opts, scene.rtl, plugin).rtl
        })
    };
    let rtl = rtl_of(w.kind.locale());
    let align = look.map_or(mandara_config::desktop_widgets::Align::Auto, |l| l.align).fraction(w.place.anchor.fraction().0, rtl);
    let mut m = Measurer { text, fonts: &fonts };
    let (comp, own_turn) = match &w.kind {
        WidgetKind::Clock(c) => {
            let s = clock_strings(now, c, scene.h24, scene.weather.as_ref(), scene.hijri, scene.rtl, scene.week_start);
            (layout::clock(c, &s, k, align, s.rtl, &mut m), 0.0)
        }
        WidgetKind::Monitor(mw) => {
            let lines = monitor_lines(mw, sampler, scene.units, scene.rtl);
            match mw.style {
                MonitorStyle::Column => (layout::monitor(mw, &lines, k, rtl, &mut m), 0.0),
                MonitorStyle::Gauges => (layout::monitor_gauges(mw, &lines, k, rtl, &mut m), 0.0),
                MonitorStyle::Meters => (monitor_more::meters(mw, &lines, k, rtl, &mut m), 0.0),
                MonitorStyle::Bars => (monitor_more::bars(mw, &lines, k, rtl, &mut m), 0.0),
                MonitorStyle::Thermals => (monitor_more::thermals(mw, &lines, k, rtl, &mut m), 0.0),
            }
        }
        WidgetKind::Decoration(d) => {
            // Lines of text are centred unless told otherwise.
            let align = if d.look.align == mandara_config::desktop_widgets::Align::Auto { DECORATION_ALIGN } else { align };
            (layout::decoration(d, k, d.sideways.clockwise(&w.place), align, &mut m), d.rotation.clamp(ranges::ANGLE_DEG.0, ranges::ANGLE_DEG.1))
        }
        WidgetKind::Weather(ww) => {
            use mandara_config::desktop_widgets::WeatherLayout;
            let s = texts::weather(ww, scene, now);
            let l = Lingo::of(&ww.locale, scene.rtl, None);
            let comp = match ww.layout {
                WeatherLayout::Sentence => {
                    let text = scene.forecast.as_ref().map(|f| weather_more::sentence(ww, f, &l)).unwrap_or_default();
                    Composition::of(weather_more::sentence_layout(ww, &s, &text, k, align, &mut m))
                }
                WeatherLayout::Disc => Composition::of(weather_more::disc_layout(ww, &s, &l, scene.units, k, &mut m)),
                WeatherLayout::Hourly => Composition::of(weather_more::hourly_layout(ww, scene.forecast.as_ref(), &l, scene.units, scene.h24, k, &mut m)),
                WeatherLayout::Ribbon | WeatherLayout::Thermometer | WeatherLayout::HumidityRose => Composition::of(weather_cards::layout(ww, &s, scene.forecast.as_ref(), &l, scene.units, k, &mut m)),
                _ => layout::weather_widget(ww, &s, k, align, &mut m),
            };
            (comp, 0.0)
        }
        WidgetKind::WorldClock(wc) => (layout::world_clock(wc, &texts::cities(wc, scene, now), k, rtl, &mut m), 0.0),
        WidgetKind::Countdown(c) => {
            let s = texts::countdown(c, scene, now);
            (layout::countdown(c, &s, k, align, &mut m), 0.0)
        }
        WidgetKind::Progress(p) => (progress::progress(p, now, &Lingo::of(&p.locale, scene.rtl, None), scene.week_start, k, align, &mut m), 0.0),
        WidgetKind::Todo(t) => (productive::todo(t, &scene.data.todos, &scene.data.lists, &Lingo::of(&t.locale, scene.rtl, None), k, &mut m), 0.0),
        WidgetKind::Note(n) => (productive::note(n, &scene.data.notes, &Lingo::of(&n.locale, scene.rtl, None), k, &mut m), 0.0),
        WidgetKind::Pomodoro(p) => {
            let state = scene.data.pomodoro.clone().unwrap_or_else(PomodoroState::idle);
            (productive::pomodoro(p, &state, now, &Lingo::of(&p.locale, scene.rtl, None), k, &mut m), 0.0)
        }
        WidgetKind::Quote(q) => (productive::quote(q, now, scene.data.turns.get(&w.id).copied().unwrap_or(0), &Lingo::of(&q.locale, scene.rtl, None), k, align, &mut m), 0.0),
        WidgetKind::Date(d) => (datecal::date_widget(d, &scene.data.events, now, &Lingo::of(&d.locale, scene.rtl, None), scene.hijri, scene.week_start, scene.h24, k, align, &mut m), 0.0),
        WidgetKind::Sky(sk) => (skyview::sky_widget(sk, scene.data.place, now, &Lingo::of(&sk.locale, scene.rtl, None), scene.h24, k, align, &mut m), 0.0),
        WidgetKind::Profile(p) => {
            let me = scene.data.me.clone().unwrap_or_else(|| MeInfo { name: String::new(), login: String::new(), host: String::new(), uptime_s: 0, avatar: None, dnd: false });
            (identity::profile(p, &me, &Lingo::of(&p.locale, scene.rtl, None), k, &mut m), 0.0)
        }
        WidgetKind::Shortcuts(sw) => (identity::shortcuts(sw, scene.data.tiles.get(&w.id).map(Vec::as_slice).unwrap_or_default(), &Lingo::of(&sw.locale, scene.rtl, None), k, &mut m), 0.0),
        WidgetKind::Toggles(tw) => {
            let states: Vec<(String, bool)> = toggle_ids(tw).into_iter().filter_map(|id| scene.data.toggles.iter().find(|(t, _)| *t == id).cloned()).collect();
            (identity::toggles(tw, &states, &Lingo::of(&tw.locale, scene.rtl, None), k, &mut m), 0.0)
        }
        WidgetKind::Sliders(sw) => {
            let wanted = if sw.levels.is_empty() { vec!["volume".to_string(), "brightness".to_string()] } else { sw.levels.clone() };
            let levels: Vec<(String, f32)> = wanted.into_iter().filter_map(|id| scene.data.levels.iter().find(|(t, _)| *t == id).cloned()).collect();
            (identity::sliders(sw, &levels, &Lingo::of(&sw.locale, scene.rtl, None), k, &mut m), 0.0)
        }
        WidgetKind::Palette(pw) => (identity::palette(pw, &scene.data.palette, &Lingo::of(&pw.locale, scene.rtl, None), k, &mut m), 0.0),
        WidgetKind::Pet(pw) => {
            let poked = scene.data.poked.get(&w.id).map(|at| now.timestamp_millis() - at).filter(|d| (0..fun::POKE_MS).contains(d));
            (fun::pet(pw, now, poked, scene.music, k, &mut m), 0.0)
        }
        WidgetKind::Bubbles(b) => (fun::bubbles(b, now, k), 0.0),
        WidgetKind::Photo(p) => (fun::photo(p, scene.data.pictures.get(&w.id), if p.folder.trim().is_empty() { "~/Pictures" } else { p.folder.trim() }, &Lingo::of(&Default::default(), scene.rtl, None), k, &mut m), 0.0),
        WidgetKind::Sticker(st) => (fun::sticker(st, scene.data.pictures.get(&w.id), &Lingo::of(&Default::default(), scene.rtl, None), k, &mut m), 0.0),
        WidgetKind::Media(mw) => (media_view::media(mw, scene.data.media.as_ref(), now, &Lingo::of(&mw.locale, scene.rtl, None), k, &mut m), 0.0),
        // The machine is read once in the background: nothing until then
        // (an empty `@` and zero uptime would read as broken).
        WidgetKind::Fetch(f) => (info::fetch(f, scene.data.fetch.as_ref()?, &scene.data.palette, &Lingo::of(&f.locale, scene.rtl, None), k, &mut m), 0.0),
        WidgetKind::ScreenTime(st) => (info::screen_time(st, scene.data.screen.as_ref(), &Lingo::of(&st.locale, scene.rtl, None), k, &mut m), 0.0),
        WidgetKind::Prayer(p) => {
            let l = Lingo::of_plugin(&p.locale, scene.rtl, plugin);
            match scene.data.prayer.as_ref() {
                Some(times) => (info::prayer(p, times, now, &l, scene.h24, k, &mut m), 0.0),
                // The Prayer times plugin is off: say so, with the way to turn it on.
                None => (empty::layout(&info::prayer_off(), p.look.font, &l, k, &mut m), 0.0),
            }
        }
        WidgetKind::Jannah(jw) => (jannah::jannah(jw, scene.data.jannah.as_ref()?, &Lingo::of_plugin(&jw.locale, scene.rtl, plugin), k, align, &mut m), 0.0),
        WidgetKind::Hadith(hw) => (hadith::hadith(hw, scene.data.hadith.as_ref()?, &Lingo::of_plugin(&hw.locale, scene.rtl, plugin), k, align, &mut m)?, 0.0),
        WidgetKind::Quran(qw) => (quran::quran(qw, scene.data.quran.as_ref()?, &Lingo::of_plugin(&qw.locale, scene.rtl, plugin), k, align, &mut m), 0.0),
        WidgetKind::Search(sw) => (info::search(sw, &Lingo::of(&sw.locale, scene.rtl, None), k, &mut m), 0.0),
        WidgetKind::Glance(g) => {
            let gi = scene.data.glance.clone().unwrap_or(GlanceInfo { icon: "clock", text: String::new(), action: None });
            (info::glance(g, &gi, scene.weather.as_ref(), now, &Lingo::of(&g.locale, scene.rtl, None), k, align, &mut m), 0.0)
        }
        WidgetKind::Github(gw) => (feeds::github(gw, &w.id, scene.data.github.get(&w.id).unwrap_or(&Feed::default()), &Lingo::of(&gw.locale, scene.rtl, None), k, &mut m), 0.0),
        WidgetKind::Currency(cw) => (feeds::currency(cw, &w.id, scene.data.rates.get(&w.id).unwrap_or(&Feed::default()), &Lingo::of(&cw.locale, scene.rtl, None), k, &mut m), 0.0),
        WidgetKind::AiUsage(aw) => (feeds::ai_usage(aw, &w.id, scene.data.ai_usage.get(&w.id).unwrap_or(&Feed::default()), &Lingo::of(&aw.locale, scene.rtl, None), k, &mut m), 0.0),
        WidgetKind::Games(gw) => (feeds::games(gw, &scene.data.games, &Lingo::of(&gw.locale, scene.rtl, None), k, &mut m), 0.0),
        WidgetKind::Notebook(n) => (productive::notebook(n, &scene.data.notes, &Lingo::of(&n.locale, scene.rtl, None), mandara_config::desktop_widgets::Sideways::Auto.clockwise(&w.place), k, &mut m), 0.0),
        WidgetKind::Visualizer(v) => {
            let geometry = visualizer::Geometry::new(v, k);
            (
                Composition::of(vec![
                    layout::Mark::new(
                        layout::MarkKind::Rect {
                            x: 0.0,
                            y: 0.0,
                            w: geometry.size.0,
                            h: geometry.size.1,
                            radius: 0.0,
                            width: 0.0,
                        },
                        layout::InkRole::Accent,
                    )
                    .dim(0.0)
                    .live(),
                ]),
                0.0,
            )
        }
        WidgetKind::Card(_) => return None,
    };
    let comp = match look {
        Some(l) => layout::with_plate(comp, l, k, align, &mut m),
        None => comp,
    };
    let rotation = own_turn + w.place.rotation();
    let flip = (w.place.flip_x, w.place.flip_y);
    let margin = draw::look_margin(look, k);
    let boxed = (comp.size.0 + 2.0 * margin, comp.size.1 + 2.0 * margin);
    let outer = if rotation != 0.0 { draw::rotated_size(boxed, rotation) } else { boxed };
    let (x, y) = w.place.origin_in((o.physical.0 as f32, o.physical.1 as f32), outer, o.safe);
    // The composition's origin inside the (possibly rotated) outer box.
    let origin = (x + (outer.0 - comp.size.0) / 2.0, y + (outer.1 - comp.size.1) / 2.0);
    Some(Laid { comp, origin, k, rotation, flip, outer: (x, y, outer.0, outer.1), margin })
}

fn look_of(k: &WidgetKind) -> Option<&mandara_config::desktop_widgets::Look> {
    k.look()
}

/// How `w`'s planes are painted: its fonts, colours, effect and shadow.
fn style_for<'a>(w: &'a DesktopWidget, fonts: &'a FontNames, inks: Inks, opacity: f32, k: f32) -> Style<'a> {
    let look = look_of(&w.kind);
    Style {
        fonts: fonts.for_look(look),
        inks,
        opacity,
        effect: look.map(|l| l.effect).unwrap_or_default(),
        strength: look.map_or(ranges::EFFECT_STRENGTH_DEFAULT, |l| l.effect_strength()),
        k,
        drop: look.and_then(|l| draw::Drop::of(l, k)),
    }
}

impl Laid {
    /// Turned or mirrored: the whole widget is drawn through a transform,
    /// into one plane (the live one when anything in it changes between
    /// minutes).
    pub fn transformed(&self) -> bool {
        self.rotation != 0.0 || self.flip.0 || self.flip.1
    }

    /// The plane rectangle holding the marks with `live == live`.
    pub fn rect(&self, live: bool, scale120: u32, output: Rect) -> Rect {
        let margin = self.margin;
        if self.transformed() {
            if live != self.comp.bounds(true).is_some() {
                return Rect::default();
            }
            let (w, h) = draw::rotated_size((self.comp.size.0 + 2.0 * margin, self.comp.size.1 + 2.0 * margin), self.rotation);
            let (cx, cy) = (self.origin.0 + self.comp.size.0 / 2.0, self.origin.1 + self.comp.size.1 / 2.0);
            return snap(Rect::from_edges(cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0), scale120).intersect(&output);
        }
        match self.comp.bounds(live) {
            Some((x0, y0, x1, y1)) => snap(Rect::from_edges(self.origin.0 + x0 - margin, self.origin.1 + y0 - margin, self.origin.0 + x1 + margin, self.origin.1 + y1 + margin), scale120).intersect(&output),
            None => Rect::default(),
        }
    }

    /// Paints into a plane at `rect`.
    pub fn paint(&self, pm: &mut tiny_skia::PixmapMut<'_>, text: &mut TextPainter, live: bool, rect: Rect, st: &Style) {
        if !self.transformed() {
            draw::paint(pm, text, &self.comp, Some(live), (rect.x as f32 - self.origin.0, rect.y as f32 - self.origin.1), st);
            return;
        }
        let margin = self.margin;
        let (w, h) = ((self.comp.size.0 + 2.0 * margin).ceil() as u32, (self.comp.size.1 + 2.0 * margin).ceil() as u32);
        let Some(mut tmp) = Pixmap::new(w.max(1), h.max(1)) else { return;
        };
        draw::paint(&mut tmp.as_mut(), text, &self.comp, None, (-margin, -margin), st);
        let c = (self.origin.0 + self.comp.size.0 / 2.0 - rect.x as f32, self.origin.1 + self.comp.size.1 / 2.0 - rect.y as f32);
        draw::draw_turned(pm, &tmp, self.rotation, self.flip, c, 1.0);
    }

    /// The action at `p` (physical px on the output), if the widget has
    /// one there.
    pub fn hit(&self, p: (f32, f32)) -> Option<&actions::Action> {
        let c = (self.origin.0 + self.comp.size.0 / 2.0, self.origin.1 + self.comp.size.1 / 2.0);
        let mut q = mandara_core::shape::rotate(p, c, -self.rotation);
        if self.flip.0 {
            q.0 = 2.0 * c.0 - q.0;
        }
        if self.flip.1 {
            q.1 = 2.0 * c.1 - q.1;
        }
        self.comp.hit((q.0 - self.origin.0, q.1 - self.origin.1))
    }
}

/// The widget's colours over the wallpaper under `rect`.
fn inks_for(w: &DesktopWidget, o: &OutputCx<'_>, scene: &WidgetScene, rect: Rect) -> Inks {
    let (pw, ph) = (o.physical.0.max(1) as f32, o.physical.1.max(1) as f32);
    let tone = o.tone.map(|t| {
        let c = ((rect.x as f32 + rect.w as f32 / 2.0) / pw, (rect.y as f32 + rect.h as f32 / 2.0) / ph);
        crate::desktop::region_tone(t, c, (rect.w as f32 / pw * TONE_HALF, rect.h as f32 / ph * TONE_HALF))
    });
    let default = mandara_config::desktop_widgets::Look::default();
    let look = look_of(&w.kind).unwrap_or(&default);
    // On a plate the automatic ink reads against the plate, not the
    // wallpaper (on a polaroid, against its paper).
    let tone = match (look.chrome, look.framed()) {
        (mandara_config::desktop_widgets::Chrome::Polaroid, _) => Some(draw::paper()),
        (_, true) => Some(scene.surface),
        _ => tone,
    };
    let auto = crate::desktop::ink_for(tone, o.primary);
    draw::inks(look, &auto, scene.on_surface, scene.surface)
}

// ── Static pass (from the clock layer, every wake) ───────────────────────

/// Paints every widget's static plane whose content changed (and, after a
/// layout change, its live plane), and returns the stacking / depth
/// information. Cheap when nothing changed: one hash per widget.
#[allow(clippy::too_many_arguments)]
pub fn step_static(host: &mut impl Planes, ws: &mut OutWidgets, o: &OutputCx<'_>, scene: &WidgetScene, shared: &Shared, text: &mut TextPainter, now: DateTime<Local>, wallpaper_key: u64) -> anyhow::Result<Layers> {
    let mut layers = Layers::default();
    let visible = o
        .names
        .iter()
        .any(|name| scene.visible.iter().any(|visible| visible == name));
    let mut released_scratch = false;
    // Forget widgets that went away or moved index (their planes are
    // removed at the end, with every other plane no shown widget stacks).
    for (i, st) in ws.states.iter_mut().enumerate() {
        if scene.items.get(i).is_none_or(|widget|!shows(widget,o)) {
            released_scratch |= st
                .vis
                .as_ref()
                .is_some_and(|(_, painter, _)| painter.cached_bytes() > 0);
            *st = State::default();
        }
    }
    ws.states.resize_with(scene.items.len().min(ranges::MAX_WIDGETS), State::default);
    let output = o.output_rect();
    let minute = crate::widgets::clock::local_stamp(&now, false);
    let avail = shared.sampler.as_ref().map(|s| availability(&s.reading, s.samples));
    for (i, w) in scene.items.iter().enumerate().take(ranges::MAX_WIDGETS).filter(|(_,widget)|shows(widget,o)) {
        let (p_static, p_live) = planes(i);
        let st = &mut ws.states[i];
        let cfg = content_key(w);
        if st.id != w.id || st.cfg != cfg {
            released_scratch |= st
                .vis
                .as_ref()
                .is_some_and(|(_, painter, _)| painter.cached_bytes() > 0);
            *st = State { id: w.id.clone(), cfg, ..State::default() };
        }
        let moving = scene.moving.as_deref() == Some(w.id.as_str());
        let opacity = w.place.opacity();
        if matches!(w.kind, WidgetKind::Visualizer(_)) && !visible {
            released_scratch |= st.vis.as_ref()
                .is_some_and(|(_, painter, _)| painter.cached_bytes() > 0);
                st.vis = None;
                st.static_key = None;
            push_layers(&mut layers, st,
                (!st.rect.is_empty()).then_some(p_static),
                (!st.live_rect.is_empty()).then_some(p_live));
            continue;
        }
        let tick = match &w.kind {
            WidgetKind::Clock(_) | WidgetKind::Weather(_) | WidgetKind::WorldClock(_) | WidgetKind::Countdown(_) | WidgetKind::Progress(_) | WidgetKind::Quote(_) | WidgetKind::Date(_) | WidgetKind::Sky(_) | WidgetKind::Prayer(_) | WidgetKind::Glance(_) => minute,
            _ => 0,
        };
        let skey = key((
            (cfg, tick, o.physical, o.scale120, wallpaper_key),
            (scene.on_surface.to_rgba8(), scene.surface.to_rgba8(), o.primary.to_rgba8(), o.fonts, &scene.weather, &scene.hijri),
            (scene.h24, scene.rtl, mandara_i18n::language(), if matches!(w.kind, WidgetKind::Monitor(_)) { avail } else { None }),
            (cache_key::debug(&scene.units),scene.week_start),
            data_key(w, scene),
        ));
        if st.static_key == Some(skey) {
            // Same pixels: has the widget moved?
            let size = (st.outer.2, st.outer.3);
            let at = w.place.origin_in((o.physical.0 as f32, o.physical.1 as f32), size, o.safe);
            let delta = (at.0 - st.painted_at.0, at.1 - st.painted_at.1);
            if delta.0.abs() > MOVE_EPSILON_PX || delta.1.abs() > MOVE_EPSILON_PX {
                let (r0, l0) = st.painted_rects;
                let (r1, l1) = (shifted(r0, delta, o.scale120), shifted(l0, delta, o.scale120));
                // While dragged, the planes slide as they are; once it lands
                // (or when moved from Studio) it is painted where it is.
                let slid = moving && (r1.is_empty() || host.move_plane(o.id, p_static, r1)) && (l1.is_empty() || host.move_plane(o.id, p_live, l1));
                if slid {
                    (st.rect, st.live_rect) = (r1, l1);
                    st.outer = (at.0, at.1, size.0, size.1);
                    st.envelope = st.envelope.union(&r1).union(&l1);
                } else {
                    st.static_key = None;
                }
            }
        }
        if st.static_key != Some(skey) {
            let Some(laid) = lay_out(w, o, scene, shared.sampler.as_ref(), &now, text) else { continue;
            };
            let rect = laid.rect(false, o.scale120, output);
            let live_rect = laid.rect(true, o.scale120, output);
            st.outer = laid.outer;
            st.hits = HitMap::of(&laid);
            st.painted_at = (laid.outer.0, laid.outer.1);
            st.painted_rects = (rect, live_rect);
            let inks = inks_for(w, o, scene, rect.union(&live_rect));
            let style = style_for(w, o.fonts, inks, opacity, laid.k);
            if let WidgetKind::Visualizer(v) = &w.kind {
                let geometry = visualizer::Geometry::new(v, laid.k).placed(&laid, live_rect);
                let mut painter = visualizer::Painter::default();
                painter.prepare(&laid, live_rect, text, &style);
                st.vis = Some((
                    geometry,
                    painter,
                    laid.k,
                ));
            }
            if rect.is_empty() {
                host.hide(o.id, p_static);
                st.static_key = Some(skey);
            } else if host.paint(o.id, p_static, Place::Rect(rect), None, false, true, |pm| {
                laid.paint(pm, text, false, rect, &style)
            })? {
                st.static_key = Some(skey);
            }
            st.rect = rect;
            // What changed the static part (the language, the data, the
            // look) may change the live part too.
            st.live_key = None;
            st.live_rect = live_rect;
            st.inks = Some(inks);
            let waiting_for_audio = matches!(w.kind, WidgetKind::Visualizer(_))
                && !(laid.transformed() && laid.comp.marks.iter().any(|mark| mark.opacity > 0.0));
            if live_rect.is_empty() || waiting_for_audio {
                host.hide(o.id, p_live);
            } else if st.live_key.is_none() {
                // Paint the live part now (the live pass keeps it current).
                if host.paint(o.id, p_live, Place::Rect(live_rect), None, true, true, |pm| laid.paint(pm, text, true, live_rect, &style))? {
                    st.live_key = Some(live_tick_key(w, scene, shared, &now));
                }
            }
            st.envelope = st.envelope.union(&rect).union(&live_rect);
        }
        st.behind = behind_subject(w, o, &st.envelope);
        let live = (!st.live_rect.is_empty()).then_some(p_live);
        push_layers(&mut layers, st, (!st.rect.is_empty()).then_some(p_static), live);
    }
    remove_unused(host, o.id, &layers);
    if released_scratch {
        mandara_services::memory::request_trim();
    }
    Ok(layers)
}

/// Whether `w` is drawn under the wallpaper's subject: set to `behind`,
/// with a subject to go behind — unless the subject would hide most of it
/// (then it goes in front, as the depth clock fades a subject that covers
/// its digits): a widget behind a landscape that fills the lower screen
/// was invisible, a visualizer there never seen to move.
fn behind_subject(w: &DesktopWidget, o: &OutputCx<'_>, area: &Rect) -> bool {
    if w.place.depth != Depth::Behind || !o.depth {
        return false;
    }
    let Some(map) = o.subject else { return true };
    if area.w == 0 || area.h == 0 {
        return true;
    }
    let (pw, ph) = (o.physical.0.max(1) as f32, o.physical.1.max(1) as f32);
    let fraction = (area.x as f32 / pw, area.y as f32 / ph, (area.x as f32 + area.w as f32) / pw, (area.y as f32 + area.h as f32) / ph);
    crate::depth::coverage(map, fraction) < HIDDEN_BEHIND_COVERAGE
}

fn push_layers(layers: &mut Layers, st: &State, a: Option<usize>, b: Option<usize>) {
    let list = if st.behind { &mut layers.behind } else { &mut layers.front };
    list.extend(a);
    list.extend(b);
    if st.behind {
        layers.reach = layers.reach.union(&st.envelope);
    }
}

/// Which monitor readings exist (keys the label plane).
fn availability(r: &sysmon::Reading, samples: u64) -> u64 {
    let bits = [r.cpu.is_some(), r.memory.is_some(), r.swap.is_some(), r.disk.is_some(), r.cpu_temp.is_some(), r.fan_rpm.is_some(), r.battery.is_some(), r.down.is_some(), samples >= 2, r.gpus_probed];
    let flags = bits.iter().enumerate().fold(0, |acc, (i, b)| acc | ((*b as u64) << i));
    // Each GPU's four readings (the lines and their labels), and how many.
    let gpus = r.gpus.iter().take(KEYED_GPUS).enumerate().fold(0u64, |acc, (i, g)| {
            acc | (u64::from(g.mask()) << (GPU_MASK_BITS * i))
        });
    flags | (gpus << bits.len()) | ((r.gpus.len().min(GPU_COUNT_MAX) as u64) << GPU_COUNT_SHIFT)
}

/// The live content's tick: the second (seconds clocks) or the sample.
fn live_tick_key(w: &DesktopWidget, scene: &WidgetScene, shared: &Shared, now: &DateTime<Local>) -> u64 {
    match &w.kind {
        WidgetKind::Monitor(_) => key(("sample", shared.sampler.as_ref().map(|s| s.generation))),
        // A paused timer doesn't change between scenes.
        WidgetKind::Pomodoro(_) if !scene.data.pomodoro.as_ref().is_some_and(|p| p.running) => {
            key(("paused", cache_key::debug(&scene.data.pomodoro)))
        }
        WidgetKind::Pet(_) | WidgetKind::Bubbles(_) | WidgetKind::Media(_) if animation_fps(w, scene, now).is_some() => {
            let fps = i64::from(animation_fps(w, scene, now).unwrap_or(1).max(1));
            key(("frame", now.timestamp_millis() * fps / MS_PER_SECOND))
        }
        // Paused: the position stays.
        WidgetKind::Media(_) if !scene.data.media.as_ref().is_some_and(|m| m.playing) => key(("paused", scene.data.media.as_ref().map(|m| m.position_ms))),
        _ => key(("second", now.timestamp())),
    }
}

// ── Live pass (seconds, readings, visualizer frames) ─────────────────────

/// What the sampler should read and how often, or `None` to drop it: only
/// while a monitor widget shows on an output whose desktop is visible.
pub fn sampler_wanted(scene: &WidgetScene, outputs: &[Vec<&str>]) -> Option<(sysmon::Wants, Duration)> {
    let mut wants = sysmon::Wants::default();
    let mut every: Option<u32> = None;
    for w in &scene.items {
        let WidgetKind::Monitor(m) = &w.kind else { continue;
        };
        let on_visible = outputs.iter().any(|names| {
            w.place.shows_on(names) && names.iter().any(|n| scene.visible.iter().any(|v| v == n))
        });
        if on_visible {
            wants.add(&m.rows);
            every = Some(every.map_or(m.interval_s(), |e| e.min(m.interval_s())));
        }
    }
    every.map(|s| (wants, Duration::from_secs(u64::from(s))))
}

/// Whether the visualizers should listen now. `silent_for` is how long the
/// spectrum has been quiet while listening.
pub fn should_listen(scene: &WidgetScene, outputs: &[Vec<&str>], silenced: bool) -> bool {
    scene.music
        && !silenced
        && scene.items.iter().any(|w| {
            matches!(w.kind, WidgetKind::Visualizer(_)) && outputs.iter().any(|names| {
                    w.place.shows_on(names) && names.iter().any(|n| scene.visible.iter().any(|v| v == n))
                })
        })
}

/// Shortest silence timeout of the visualizers.
fn silence_limit(scene: &WidgetScene) -> Duration {
    let s = scene.items.iter().filter_map(|w| match &w.kind {
        WidgetKind::Visualizer(v) => Some(v.silence_s.clamp(ranges::VISUALIZER_SILENCE_S.0, ranges::VISUALIZER_SILENCE_S.1)),
        _ => None,
    });
    Duration::from_secs(u64::from(s.min().unwrap_or(ranges::VISUALIZER_SILENCE_DEFAULT_S)))
}

/// Starts / stops the sampler and the spectrum subscription; samples when
/// due. `outputs` lists each output's names.
pub fn step_services(shared: &mut Shared, scene: &WidgetScene, outputs: &[Vec<&str>], now: Instant) {
    match sampler_wanted(scene, outputs) {
        Some((wants, every)) => {
            let s = shared.sampler.get_or_insert_with(sysmon::Sampler::new);
            if s.sampled_at.is_none_or(|t| now.saturating_duration_since(t) + TICK_SLACK >= every) {
                s.sample(wants, now);
            }
        }
        None => shared.sampler = None,
    }
    // Music starting again lifts a silence stop.
    if scene.music && !shared.music_was {
        shared.silenced = false;
    }
    shared.music_was = scene.music;
    if should_listen(scene, outputs, shared.silenced) {
        if shared.tap.is_none() && shared.tap_tried.is_none_or(|t| now.saturating_duration_since(t) >= SPECTRUM_RETRY) {
            shared.tap_tried = Some(now);
            shared.tap = mandara_services::spectrum::subscribe();
            shared.silent_since = None;
        }
        if let Some(tap) = &shared.tap {
            if tap.is_active() {
                shared.silent_since = None;
            } else if now.saturating_duration_since(*shared.silent_since.get_or_insert(now)) >= silence_limit(scene) {
                // Freeze: the planes keep their last frame; capture stops.
                shared.tap = None;
                shared.silenced = true;
                shared.silent_since = None;
            }
        }
    } else {
        shared.tap = None;
        shared.silent_since = None;
    }
}

/// Repaints the live planes that are due on output `o`. Returns whether
/// anything was painted (the caller commits).
#[allow(clippy::too_many_arguments)]
pub fn step_live(host: &mut impl Planes, ws: &mut OutWidgets, o: &OutputCx<'_>, scene: &WidgetScene, shared: &Shared, text: &mut TextPainter, now: DateTime<Local>, at: Instant) -> anyhow::Result<bool> {
    if !o.names.iter().any(|n| scene.visible.iter().any(|v| v == n)) {
        return Ok(false);
    }
    let mut painted = false;
    let bands = shared.tap.as_ref().filter(|t| t.is_active()).map(|t| t.bands());
    for (i, w) in scene.items.iter().enumerate().take(ranges::MAX_WIDGETS) {
        let Some(st) = ws.states.get_mut(i) else { break;
        };
        if st.id != w.id || st.live_rect.is_empty() || !shows(w, o) {
            continue;
        }
        let (_, p_live) = planes(i);
        let rect = st.live_rect;
        let opacity = w.place.opacity();
        match &w.kind {
            WidgetKind::Visualizer(v) => {
                let Some(bands) = bands.as_ref() else { continue;
                };
                let period = Duration::from_secs_f32(1.0 / v.fps() as f32);
                if st.frame_at.is_some_and(|t| at.saturating_duration_since(t) + TICK_SLACK < period) || !host.frame_ready(o.id) {
                    continue;
                }
                let Some(inks) = st.inks else { continue };
                let Some((g, painter, k)) = st.vis.as_mut() else { continue;
                };
                let style = style_for(w, o.fonts, inks, opacity, *k);
                let g = *g;
                if host.paint(o.id, p_live, Place::Rect(rect), None, true, true, |pm| {
                    painter.styled_frame(pm, &g, bands, &style)
                })? {
                    st.frame_at = Some(at);
                    painted = true;
                }
            }
            WidgetKind::Clock(_) | WidgetKind::Monitor(_) | WidgetKind::Pomodoro(_) | WidgetKind::Pet(_) | WidgetKind::Bubbles(_) | WidgetKind::Media(_) => {
                let k = live_tick_key(w, scene, shared, &now);
                if st.live_key == Some(k) {
                    continue;
                }
                // Animations wait for the compositor to take the last frame.
                if animation_fps(w, scene, &now).is_some() && !host.frame_ready(o.id) {
                    continue;
                }
                let Some(laid) = lay_out(w, o, scene, shared.sampler.as_ref(), &now, text) else { continue;
                };
                let Some(inks) = st.inks else { continue };
                let style = style_for(w, o.fonts, inks, opacity, laid.k);
                if host.paint(o.id, p_live, Place::Rect(rect), None, true, true, |pm| {
                    laid.paint(pm, text, true, rect, &style)
                })? {
                    st.live_key = Some(k);
                    painted = true;
                }
            }
            _ => {}
        }
    }
    Ok(painted)
}

/// When the widgets of an output next need the renderer.
pub fn next_wake(ws: &OutWidgets, o_names: &[&str], scene: &WidgetScene, shared: &Shared, now: DateTime<Local>, at: Instant) -> Option<Duration> {
    if !o_names.iter().any(|name|scene.visible.iter().any(|visible|visible == name)) {
        return None;
    }
    let mut wake: Option<Duration> = None;
    let mut soon = |d: Duration| wake = Some(wake.map_or(d, |w| w.min(d)));
    let ms = now.timestamp_millis();
    let until = |period: i64| {
        Duration::from_millis((period - ms.rem_euclid(period)).unsigned_abs()) + TICK_SLACK
    };
    let visible = o_names.iter().any(|n| scene.visible.iter().any(|v| v == n));
    for (i, w) in scene.items.iter().enumerate().take(ranges::MAX_WIDGETS) {
        if !w.enabled || !w.place.shows_on(o_names) {
            continue;
        }
        match &w.kind {
            WidgetKind::Clock(c) => {
                soon(until(MS_PER_MINUTE));
                if visible && ticks_seconds(c) {
                    soon(until(MS_PER_SECOND));
                }
            }
            WidgetKind::Weather(_) if w.enabled && visible => soon(until(MS_PER_MINUTE)),
            // Their times and the day count change with the minute.
            WidgetKind::WorldClock(_) | WidgetKind::Countdown(_) | WidgetKind::Progress(_) | WidgetKind::Quote(_) | WidgetKind::Date(_) | WidgetKind::Sky(_) | WidgetKind::Prayer(_) | WidgetKind::Glance(_) => soon(until(MS_PER_MINUTE)),
            WidgetKind::Pomodoro(_) if visible && scene.data.pomodoro.as_ref().is_some_and(|p| p.running) =>
            {
                soon(until(MS_PER_SECOND))
            }
            WidgetKind::Pet(_) | WidgetKind::Bubbles(_) | WidgetKind::Media(_) if visible && animation_fps(w, scene, &now).is_some() => {
                let fps = i64::from(animation_fps(w, scene, &now).unwrap_or(1).max(1));
                soon(until(MS_PER_SECOND / fps));
            }
            WidgetKind::Media(_) if visible && scene.data.media.as_ref().is_some_and(|m| m.playing) =>
            {
                soon(until(MS_PER_SECOND))
            }
            WidgetKind::Monitor(m) if visible => {
                let every = Duration::from_secs(u64::from(m.interval_s()));
                soon(shared.sampler.as_ref().and_then(|s| s.sampled_at).map_or(Duration::ZERO, |t| {
                            every.saturating_sub(at.saturating_duration_since(t))
                        }));
            }
            WidgetKind::Visualizer(v) if visible && scene.music && !shared.silenced => {
                if shared.tap.as_ref().is_some_and(|t| t.is_active()) {
                    let period = Duration::from_secs_f32(1.0 / v.fps() as f32);
                    let last = ws.states.get(i).and_then(|s| s.frame_at);
                    soon(last.map_or(Duration::ZERO, |t| {
                        period.saturating_sub(at.saturating_duration_since(t))
                    }));
                } else {
                    // Waiting for sound (or the silence timeout).
                    soon(VIS_POLL);
                }
            }
            _ => {}
        }
    }
    wake
}

// ── Offscreen (preview, Studio thumbnails) ───────────────────────────────

/// Paints `w` (static and live parts) over `canvas` (a full output at
/// `o.physical`), as the live canvas would show it.
#[allow(clippy::too_many_arguments)]
pub fn compose(canvas: &mut tiny_skia::PixmapMut<'_>, w: &DesktopWidget, o: &OutputCx<'_>, scene: &WidgetScene, sampler: Option<&sysmon::Sampler>, text: &mut TextPainter, now: &DateTime<Local>, bands: &[f32]) {
    let Some(laid) = lay_out(w, o, scene, sampler, now, text) else { return;
    };
    compose_laid(canvas, w, o, scene, &laid, text, bands);
}

/// Paint a laid-out widget into its fitted preview canvas.
fn compose_laid(canvas: &mut tiny_skia::PixmapMut<'_>, w: &DesktopWidget, o: &OutputCx<'_>, scene: &WidgetScene, laid: &Laid, text: &mut TextPainter,
    bands: &[f32],
) {
    let output = o.output_rect();
    let opacity = w.place.opacity();
    let (rect, live) = (laid.rect(false, o.scale120, output), laid.rect(true, o.scale120, output));
    let inks = inks_for(w, o, scene, rect.union(&live));
    let style = style_for(w, o.fonts, inks, opacity, laid.k);
    for (r, is_live) in [(rect, false), (live, true)] {
        if r.is_empty() {
            continue;
        }
        let Some(mut pm) = Pixmap::new(r.w, r.h) else { continue;
        };
        paint_preview(&mut pm, w, laid, text, is_live, r, &style, bands);
        crate::backdrop::paint::compose(canvas, &pm, r, 1.0);
    }
}

/// The same framed spectrum in Studio, transitions and the live desktop.
#[allow(clippy::too_many_arguments)]
fn paint_preview(
    pm: &mut Pixmap,
    widget: &DesktopWidget,
    laid: &Laid,
    text: &mut TextPainter,
    live: bool,
    rect: Rect,
    style: &Style,
    bands: &[f32],
) {
    if live && let WidgetKind::Visualizer(visualizer) = &widget.kind {
        let geometry = visualizer::Geometry::new(visualizer, laid.k).placed(laid, rect);
        let mut painter = visualizer::Painter::default();
        painter.prepare(laid, rect, text, style);
        painter.styled_frame(&mut pm.as_mut(), &geometry, bands, style);
    } else {
        laid.paint(&mut pm.as_mut(), text, live, rect, style);
    }
}

/// Paints every widget shown on `o` on one side of the depth subject
/// (`behind` it or in front), as the live canvas stacks them, over
/// `canvas`; returns the area the ones behind reach (the subject covers
/// it). A wallpaper crossfade bakes the incoming desktop with this.
#[allow(clippy::too_many_arguments)]
pub fn compose_side(canvas: &mut tiny_skia::PixmapMut<'_>, o: &OutputCx<'_>, scene: &WidgetScene, shared: &Shared, text: &mut TextPainter, now: &DateTime<Local>, behind: bool) -> Rect {
    let bands = shared.tap.as_ref().filter(|t| t.is_active()).map(|t| t.bands().to_vec());
    let mut reach = Rect::default();
    for w in scene.items.iter().take(ranges::MAX_WIDGETS).filter(|w| shows(w, o)) {
        let Some(area) = widget_rect(w, o, scene, text, now) else { continue;
        };
        if behind_subject(w, o, &area) != behind {
            continue;
        }
        // A silent visualizer paints no frame on the live canvas either.
        compose(canvas, w, o, scene, shared.sampler.as_ref(), text, now,
            bands.as_deref().unwrap_or_default());
        if behind {
            reach = reach.union(&area);
        }
    }
    reach
}

/// Where `w` draws on output `o` (physical px), for Studio's drag boxes.
pub fn widget_rect(w: &DesktopWidget, o: &OutputCx<'_>, scene: &WidgetScene, text: &mut TextPainter, now: &DateTime<Local>) -> Option<Rect> {
    let output = o.output_rect();
    let laid = lay_out(w, o, scene, None, now, text)?;
    Some(laid.rect(false, o.scale120, output).union(&laid.rect(true, o.scale120, output)))
}

/// A widget's id and box as fractions of a preview frame `[x, y, w, h]`.
pub type WidgetBox = (String, [f32; 4]);

/// One widget of a preview drawn on its own: its pixels (straight RGBA,
/// `rect`'s size) and, in frame pixels, the rectangle they cover and the
/// box its placement positions. Moving the widget moves the picture
/// (`rect` keeps its offset from the box), so a drag never repaints it.
pub struct PreviewLayer {
    pub rect: Rect,
    /// `x, y, w, h` of the placed box.
    pub outer: (f32, f32, f32, f32),
}

impl PreviewLayer {
    /// Where the picture goes when the widget is placed at `place` on a
    /// `frame` (frame px, top-left).
    pub fn at(&self, place: &mandara_config::desktop_widgets::Placement, frame: (f32, f32)) -> (f32, f32) {
        let (x, y) = place.origin(frame, (self.outer.2, self.outer.3));
        (x + self.rect.x as f32 - self.outer.0, y + self.rect.y as f32 - self.outer.1)
    }
}

/// `w` alone on a transparent `size` frame, cropped to what it draws (see
/// [`PreviewLayer`]).
pub fn preview_layer(w: &DesktopWidget, size: (u32, u32), fonts: &FontNames, primary: Color, text: &mut TextPainter) -> Option<(image::RgbaImage, PreviewLayer)> {
    let names: [&str; 0] = [];
    let o = OutputCx { id: 0, names: &names, logical: (size.0 as f32, size.1 as f32), physical: size, scale120: mandara_core::shm_canvas::SCALE_DENOMINATOR, tone: None, primary, fonts, depth: false, subject: None, safe: Insets::default() };
    let scene = WidgetScene::preview(vec![w.clone()]);
    let now = Local::now();
    let output = o.output_rect();
    let opacity = w.place.opacity();
    let (union, outer, parts): (Rect, (f32, f32, f32, f32), Vec<(Rect, Pixmap)>) = {
        let laid = lay_out(w, &o, &scene, None, &now, text)?;
        let (rect, live) = (laid.rect(false, o.scale120, output), laid.rect(true, o.scale120, output));
        let inks = inks_for(w, &o, &scene, rect.union(&live));
        let style = style_for(w, o.fonts, inks, opacity, laid.k);
        let mut parts = Vec::new();
        for (r, is_live) in [(rect, false), (live, true)] {
            if r.is_empty() {
                continue;
            }
            let mut pm = Pixmap::new(r.w, r.h)?;
            paint_preview(&mut pm,
                w,
                &laid, text, is_live, r, &style,
                &preview_bands(),
            );
            parts.push((r, pm));
        }
        (rect.union(&live), laid.outer, parts)
    };
    if union.is_empty() {
        return None;
    }
    let mut canvas = Pixmap::new(union.w, union.h)?;
    for (r, pm) in &parts {
        crate::backdrop::paint::compose(&mut canvas.as_mut(), pm, Rect::new(r.x - union.x, r.y - union.y, r.w, r.h), 1.0);
    }
    Some((straight(canvas)?, PreviewLayer { rect: union, outer }))
}

/// A widget preview (all of `items` on a transparent `size` frame, as
/// straight RGBA) and each widget's box as fractions of the frame.
pub fn preview(items: &[DesktopWidget], size: (u32, u32), fonts: &FontNames, primary: Color, text: &mut TextPainter) -> Option<(image::RgbaImage, Vec<WidgetBox>)> {
    let mut canvas = Pixmap::new(size.0.max(1), size.1.max(1))?;
    let names: [&str; 0] = [];
    let o = OutputCx { id: 0, names: &names, logical: (size.0 as f32, size.1 as f32), physical: size, scale120: mandara_core::shm_canvas::SCALE_DENOMINATOR, tone: None, primary, fonts, depth: false, subject: None, safe: Insets::default() };
    let scene = WidgetScene::preview(items.to_vec());
    let now = Local::now();
    let bands = preview_bands();
    let mut boxes = Vec::new();
    let (fw, fh) = (size.0.max(1) as f32, size.1.max(1) as f32);
    for w in items {
        compose(&mut canvas.as_mut(), w, &o, &scene, None, text, &now, &bands);
        if let Some(r) = widget_rect(w, &o, &scene, text, &now) {
            boxes.push((w.id.clone(), [r.x as f32 / fw, r.y as f32 / fh, r.w as f32 / fw, r.h as f32 / fh]));
        }
    }
    Some((straight(canvas)?, boxes))
}

/// Straight RGBA → a GPUI image (BGRA), for Studio.
pub fn render_image(mut rgba: image::RgbaImage) -> std::sync::Arc<gpui::RenderImage> {
    for px in rgba.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    std::sync::Arc::new(gpui::RenderImage::new(smallvec::smallvec![image::Frame::new(rgba)]))
}

fn straight(canvas: Pixmap) -> Option<image::RgbaImage> {
    let (w, h) = (canvas.width(), canvas.height());
    let mut img = image::RgbaImage::from_raw(w, h, canvas.take())?;
    for p in img.pixels_mut() {
        let a = p.0[3] as u32;
        if a > 0 && a < 255 {
            for channel in &mut p.0[..3] {
                *channel = ((u32::from(*channel) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Some(img)
}

/// Example spectrum for previews (a gentle musical slope).
pub fn preview_bands() -> [f32; mandara_services::spectrum::BANDS] {
    let mut b = [0.0; mandara_services::spectrum::BANDS];
    for (i, v) in b.iter_mut().enumerate() {
        let f = i as f32 / mandara_services::spectrum::BANDS as f32;
        *v = (0.85 - 0.55 * f + 0.18 * (f * 23.0).sin()).clamp(0.05, 1.0);
    }
    b
}

/// A thumbnail of `w` alone on a transparent `w × h` frame (Studio's
/// gallery), as straight RGBA.
pub fn thumbnail(w: &DesktopWidget, size: (u32, u32), fonts: &FontNames, primary: Color, text: &mut TextPainter) -> Option<image::RgbaImage> {
    let mut canvas = Pixmap::new(size.0.max(1), size.1.max(1))?;
    let mut sample = sysmon::Sampler::preview();
    if let WidgetKind::Monitor(m) = &w.kind {
        let mut wants = sysmon::Wants::default();
        wants.add(&m.rows);
        sample.sample(wants, Instant::now());
    }
    let names: [&str; 0] = [];
    let o = OutputCx { id: 0, names: &names, logical: (size.0 as f32, size.1 as f32), physical: size, scale120: mandara_core::shm_canvas::SCALE_DENOMINATOR, tone: None, primary, fonts, depth: false, subject: None, safe: Insets::default() };
    let scene = WidgetScene::preview(Vec::new());
    compose(&mut canvas.as_mut(), w, &o, &scene, Some(&sample), text, &Local::now(), &preview_bands());
    straight(canvas)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::desktop_widgets::{MonitorWidget, Placement, PlaneVisualizer};

    fn scene(items: Vec<DesktopWidget>) -> WidgetScene {
        WidgetScene { items, visible: vec!["DP-1".into()], music: true, ..Default::default() }
    }

    fn monitor_on(displays: &[&str]) -> DesktopWidget {
        DesktopWidget { id: "monitor-1".into(), place: Placement { displays: displays.iter().map(|d| d.to_string()).collect(), ..Default::default() }, kind: WidgetKind::Monitor(MonitorWidget { interval_s: 3, ..Default::default() }), ..Default::default() }
    }

    #[test]
    fn preview_pixels_keep_alpha_and_unpremultiply_each_color_channel() {
        let mut canvas = Pixmap::new(2, 1).unwrap();
        canvas.data_mut().copy_from_slice(&[64, 32, 16, 128, 0, 0, 0, 0]);
        assert_eq!(straight(canvas).unwrap().as_raw(), &[128, 64, 32, 128, 0, 0, 0, 0]);
    }

    #[test]
    fn covered_desktops_and_disabled_clocks_do_not_schedule_widget_timers() {
        let mut scene = scene(vec![DesktopWidget::default(),DesktopWidget { kind:WidgetKind::Date(Default::default()),..Default::default() }]);
        let widgets = OutWidgets::default();let shared = Shared::default();
        let now = Local::now();let at = Instant::now();
        assert!(next_wake(&widgets,&["DP-1"],&scene,&shared,now,at).is_some());
        scene.visible.clear();
        assert!(next_wake(&widgets,&["DP-1"],&scene,&shared,now,at).is_none());
        scene.visible.push("DP-1".into());
        for widget in &mut scene.items { widget.enabled = false; }
        assert!(next_wake(&widgets,&["DP-1"],&scene,&shared,now,at).is_none());
    }

    #[test]
    fn shared_calendar_and_unit_settings_invalidate_cached_widget_planes_immediately() {
        let fonts = FontNames::default();
        let names = ["DP-1"];
        let output = OutputCx { id:1,names:&names,logical:(1920.0,1080.0),physical:(1920,1080),scale120:120,tone:None,primary:Color::hex("#0088ff"),fonts:&fonts,depth:false,subject:None,safe:Insets::default() };
        let mut scene = scene(vec![DesktopWidget { id:"date".into(),kind:WidgetKind::Date(Default::default()),..Default::default() }]);
        let mut host = Recorder::default();
        let mut widgets = OutWidgets::default();
        let mut text = TextPainter::new();
        let shared = Shared::default();
        let now = Local::now();
        step_static(&mut host,&mut widgets,&output,&scene,&shared,&mut text,now,0).unwrap();
        let first = host.paints;
        step_static(&mut host,&mut widgets,&output,&scene,&shared,&mut text,now,0).unwrap();
        assert_eq!(host.paints,first);
        scene.week_start = 6;
        step_static(&mut host,&mut widgets,&output,&scene,&shared,&mut text,now,0).unwrap();
        assert!(host.paints > first);
        let second = host.paints;
        scene.units = Units::Imperial;
        step_static(&mut host,&mut widgets,&output,&scene,&shared,&mut text,now,0).unwrap();
        assert!(host.paints > second);
    }

    #[test]
    fn sampler_runs_only_with_a_visible_monitor() {
        let outs = vec![vec!["DP-1"], vec!["HDMI-A-1"]];
        assert!(sampler_wanted(&scene(vec![]), &outs).is_none(), "no widget, no sampler");
        let (wants, every) = sampler_wanted(&scene(vec![monitor_on(&[])]), &outs).expect("visible monitor");
        assert!(wants.cpu && wants.net && every == Duration::from_secs(3));
        assert!(sampler_wanted(&scene(vec![monitor_on(&["HDMI-A-1"])]), &outs).is_none(), "its display is covered");
        let mut off = scene(vec![monitor_on(&[])]);
        off.visible.clear();
        assert!(sampler_wanted(&off, &outs).is_none(), "fullscreen / locked: paused");
        // The renderer drops the sampler as soon as it is not wanted.
        let mut shared = Shared::default();
        step_services(&mut shared, &scene(vec![monitor_on(&[])]), &outs, Instant::now());
        assert!(shared.sampler.as_ref().is_some_and(|s| s.samples == 1));
        step_services(&mut shared, &scene(vec![monitor_on(&[])]), &outs, Instant::now());
        assert_eq!(shared.sampler.as_ref().map(|s| s.samples), Some(1), "not due yet");
        step_services(&mut shared, &off, &outs, Instant::now());
        assert!(shared.sampler.is_none());
    }

    #[test]
    fn visualizer_listens_only_while_music_plays_and_stops_when_silent() {
        let outs = vec![vec!["DP-1"]];
        let vis = DesktopWidget { kind: WidgetKind::Visualizer(PlaneVisualizer { silence_s: 2, ..Default::default() }), ..Default::default() };
        let s = scene(vec![vis.clone()]);
        assert!(should_listen(&s, &outs, false));
        assert!(!should_listen(&s, &outs, true), "silenced");
        assert!(!should_listen(&WidgetScene { music: false, ..s.clone() }, &outs, false), "paused player");
        assert!(!should_listen(&WidgetScene { visible: vec![], ..s.clone() }, &outs, false), "desktop covered");
        assert!(!should_listen(&scene(vec![]), &outs, false));
        assert_eq!(silence_limit(&s), Duration::from_secs(2));
        // Silence: with no sound server the tap never starts, and nothing
        // keeps listening once the music stops.
        let mut shared = Shared::default();
        let t0 = Instant::now();
        step_services(&mut shared, &WidgetScene { music: false, ..s.clone() }, &outs, t0);
        assert!(!shared.listening());
        // A silence stop holds until the music restarts.
        shared.silenced = true;
        shared.music_was = true;
        step_services(&mut shared, &s, &outs, t0);
        assert!(shared.silenced && !shared.listening());
        step_services(&mut shared, &WidgetScene { music: false, ..s.clone() }, &outs, t0);
        step_services(&mut shared, &s, &outs, t0);
        assert!(!shared.silenced, "music restarting lifts the stop");
    }

    #[test]
    fn greetings_follow_the_hour() {
        assert_eq!(greeting(6), "Good morning");
        assert_eq!(greeting(13), "Good afternoon");
        assert_eq!(greeting(18), "Good evening");
        assert_eq!(greeting(23), "Good night");
        assert_eq!(greeting(2), "Good night");
    }

    #[test]
    fn clock_strings_respect_hours_and_numerals() {
        use chrono::TimeZone as _;
        let now = Local.with_ymd_and_hms(2026, 8, 22, 0, 9, 5).unwrap();
        let c = ClockWidget { face: ClockFace::Superscript, ..Default::default() };
        let s = clock_strings(&now, &c, false, None, None, false, 0);
        assert_eq!((s.hours.as_str(), s.minutes.as_str(), s.time.as_str()), ("12", "09", "12:09"));
        assert_eq!(s.meridiem.as_deref(), Some("AM"));
        let h24 = clock_strings(&now, &ClockWidget { locale: mandara_config::desktop_widgets::TextOptions { numerals: mandara_config::desktop_widgets::Numerals::EasternArabic, ..Default::default() }, ..c.clone() }, true, Some(&("sun", "23°".into())), None, false, 0);
        assert_eq!(h24.time, "٠٠:٠٩");
        assert!(h24.meridiem.is_none());
        assert_eq!(h24.weather.as_ref().map(|w| w.1.as_str()), Some("٢٣°"));
        assert!((s.day_fraction - (9.0 * 60.0 + 5.0) / SECONDS_PER_DAY).abs() < 1e-6);
    }

    #[test]
    fn byte_pairs_share_a_unit() {
        assert_eq!(pair_bytes(6_100_000_000, 31_200_000_000), "6.1 / 31.2 GB");
        let lines = monitor_lines(&MonitorWidget::default(), None, Units::Metric, false);
        assert!(lines.iter().any(|l| l.label == "CPU" && l.value == "—"), "unknown until sampled");
    }

    /// A settled sampler holding `gpus` and every other reading.
    fn sampled(gpus: Vec<sysmon::GpuReading>) -> sysmon::Sampler {
        let mut s = sysmon::Sampler::preview();
        s.samples = 2;
        s.reading = sysmon::Reading {
            cpu: Some(0.25),
            memory: Some((4 << 30, 16 << 30)),
            swap: Some((1 << 30, 8 << 30)),
            disk: Some((100 << 30, 500 << 30)),
            cpu_temp: Some(55.0),
            gpus,
            gpus_probed: true,
            fan_rpm: Some(900),
            battery: Some(0.8),
            down: Some(1000.0),
            up: Some(10.0),
        };
        s
    }

    fn nvidia() -> sysmon::GpuReading {
        sysmon::GpuReading { load: Some(0.42), vram: Some((3 << 30, 12 << 30)), temp: Some(61.0), power_w: Some(140.0) }
    }

    fn rows_of(lines: &[MonitorLine]) -> Vec<MonitorRow> {
        lines.iter().filter_map(|l| l.row).collect()
    }

    #[test]
    fn every_reading_on_shows_each_once_and_one_alone_shows_only_it() {
        let all = MonitorWidget { rows: MonitorRow::ALL.to_vec(), ..MonitorWidget::default() };
        let s = sampled(vec![nvidia()]);
        let lines = monitor_lines(&all, Some(&s), Units::Metric, false);
        assert_eq!(rows_of(&lines), MonitorRow::ALL, "one line per reading, in order");
        let label = |r: MonitorRow| {
            lines.iter().find(|l| l.row == Some(r)).map(|l| (l.label.as_str(), l.value.as_str()))
        };
        assert_eq!(label(MonitorRow::Gpu), Some(("GPU", "42%")));
        assert!(label(MonitorRow::Vram).is_some_and(|(_, v)| v.contains('/')));
        assert_eq!(label(MonitorRow::GpuPower), Some(("GPU power", "140 W")));
        assert!(label(MonitorRow::Swap).is_some_and(|(l, v)| l == "Swap" && v.contains('/')));
        for one in MonitorRow::ALL {
            let w = MonitorWidget { rows: vec![one], ..MonitorWidget::default() };
            assert_eq!(rows_of(&monitor_lines(&w, Some(&s), Units::Metric, false)), [one], "{one:?} alone");
        }
        // Both lay out; every reading makes a taller column than one.
        let fonts = FontNames::default();
        let names = ["DP-1"];
        let o = OutputCx { id: 1, names: &names, logical: (1920.0, 1080.0), physical: (1920, 1080), scale120: mandara_core::shm_canvas::SCALE_DENOMINATOR, tone: None, primary: Color::oklch(0.6, 0.15, 200.0), fonts: &fonts, depth: false, subject: None, safe: Insets::default() };
        let mut text = TextPainter::new();
        let now = Local::now();
        let height = |m: MonitorWidget, text: &mut TextPainter| {
            let w = DesktopWidget { kind: WidgetKind::Monitor(m), ..Default::default() };
            lay_out(&w, &o, &scene(vec![w.clone()]), Some(&s), &now, text).expect("laid out").comp.size.1
        };
        let tall = height(all.clone(), &mut text);
        let short = height(MonitorWidget { rows: vec![MonitorRow::Vram], ..MonitorWidget::default() }, &mut text);
        assert!(tall > short * 4.0 && short > 0.0, "{tall} vs {short}");
        for style in [MonitorStyle::Gauges, MonitorStyle::Meters, MonitorStyle::Bars, MonitorStyle::Thermals] {
            assert!(height(MonitorWidget { style, ..all.clone() }, &mut text) > 0.0, "{style:?}");
        }
    }

    #[test]
    fn several_gpus_get_a_line_each_and_none_leaves_no_empty_row() {
        let w = MonitorWidget { rows: vec![MonitorRow::Gpu, MonitorRow::Vram, MonitorRow::GpuTemp, MonitorRow::GpuPower], ..MonitorWidget::default() };
        // An NVIDIA card and an APU that reports no power.
        let apu = sysmon::GpuReading { load: Some(0.05), vram: Some((1 << 30, 2 << 30)), temp: Some(48.0), power_w: None };
        let lines = monitor_lines(&w, Some(&sampled(vec![nvidia(), apu])), Units::Metric, false);
        let labels: Vec<&str> = lines.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, ["GPU 1", "GPU 2", "VRAM 1", "VRAM 2", "GPU 1 temp", "GPU 2 temp", "GPU 1 power"]);
        assert_eq!(lines[4].short.as_deref(), Some("GPU 1"));
        // No GPU: no GPU rows at all, not even a dash, once the GPUs were read.
        let none = monitor_lines(&MonitorWidget::default(), Some(&sampled(vec![])), Units::Metric, false);
        assert!(none.iter().all(|l| !l.row.is_some_and(MonitorRow::is_gpu)), "{none:?}");
        assert!(none.iter().any(|l| l.row == Some(MonitorRow::Cpu)));
        // Before the first GPU reading a dash holds the place.
        let mut early = sampled(vec![]);
        (early.reading.gpus_probed, early.samples) = (false, 1);
        let lines = monitor_lines(&w, Some(&early), Units::Metric, false);
        assert_eq!(lines.len(), 4);
        assert!(lines.iter().all(|l| l.value == "—"));
        // The label plane's key follows the GPU lines.
        let one = sampled(vec![nvidia()]);
        let two = sampled(vec![nvidia(), apu]);
        assert_ne!(availability(&one.reading, 2), availability(&two.reading, 2));
        assert_ne!(availability(&sampled(vec![apu]).reading, 2), availability(&one.reading, 2), "a reading missing changes the key");
    }

    /// A compositor stand-in: which planes exist, which are mapped, the
    /// bytes their buffers hold and the rect each was last painted at.
    #[derive(Default)]
    struct Recorder {
        exists: std::collections::BTreeSet<usize>,
        bytes: std::collections::BTreeMap<usize, u64>,
        rects: std::collections::BTreeMap<usize, Rect>,
        /// Paints and plane moves so far.
        paints: usize,
        moves: usize,
    }

    impl Planes for Recorder {
        fn paint(&mut self, _id: u32, index: usize, place: Place, _damage: Option<Rect>, _keep: bool, _clear: bool, paint: impl FnOnce(&mut tiny_skia::PixmapMut<'_>)) -> anyhow::Result<bool> {
            let Place::Rect(r) = place else { panic!("widgets paint plain rects") };
            let mut pm = Pixmap::new(r.w.max(1), r.h.max(1)).expect("plane pixmap");
            paint(&mut pm.as_mut());
            self.exists.insert(index);
            self.bytes.insert(index, u64::from(r.w.max(1)) * u64::from(r.h.max(1)) * 4);
            self.rects.insert(index, r);
            self.paints += 1;
            Ok(true)
        }

        fn hide(&mut self, _id: u32, index: usize) {
            self.bytes.remove(&index);
            self.rects.remove(&index);
        }

        fn remove_plane(&mut self, id: u32, index: usize) {
            self.hide(id, index);
            self.exists.remove(&index);
        }

        fn has_plane(&self, _id: u32, index: usize) -> bool {
            self.exists.contains(&index)
        }

        fn frame_ready(&self, _id: u32) -> bool {
            true
        }

        fn move_plane(&mut self, _id: u32, index: usize, to: Rect) -> bool {
            match self.rects.get_mut(&index) {
                Some(r) if r.w == to.w && r.h == to.h => {
                    *r = to;
                    self.moves += 1;
                    true
                }
                _ => false,
            }
        }
    }

    impl Recorder {
        fn mapped(&self) -> Vec<usize> {
            self.bytes.keys().copied().collect()
        }
    }

    fn shape(id: &str, shape: mandara_config::desktop_widgets::DecorShape, anchor: mandara_config::desktop_widgets::Anchor) -> DesktopWidget {
        use mandara_config::desktop_widgets::Decoration;
        DesktopWidget { id: id.into(), place: Placement { anchor, ..Default::default() }, kind: WidgetKind::Decoration(Decoration { shape, ..Default::default() }), ..Default::default() }
    }

    /// Dragging a widget in edit mode slides its planes (no repaint per
    /// pointer event); where it lands it is painted once more, exactly.
    #[test]
    fn weather_repaints_at_date_boundaries_without_a_clock_or_refresh() {
        use mandara_config::desktop_widgets::{WeatherLayout, WeatherWidget};
        let fonts = FontNames::default();
        let names = ["DP-1"];
        let output = OutputCx { id: 1, names: &names, logical: (1920.0, 1080.0), physical: (1920, 1080), scale120: mandara_core::shm_canvas::SCALE_DENOMINATOR, tone: None, primary: Color::oklch(0.6, 0.15, 200.0), fonts: &fonts, depth: false, subject: None, safe: Insets::default() };
        let widget = DesktopWidget { kind: WidgetKind::Weather(WeatherWidget { layout: WeatherLayout::Thermometer, ..Default::default() }), ..Default::default() };
        let mut scene = scene(vec![widget]);
        let before = DateTime::parse_from_rfc3339("2026-11-05T23:59:59Z").unwrap().with_timezone(&Local);
        let after = before + chrono::Duration::seconds(1);
        let mut forecast = texts::sample_forecast(before.with_timezone(&chrono::Utc).date_naive());
        forecast.timezone = Some("UTC".parse().unwrap());
        scene.forecast = Some(forecast);
        let mut text = TextPainter::new();
        let mut host = Recorder::default();
        let mut widgets = OutWidgets::default();
        let shared = Shared::default();
        step_static(&mut host, &mut widgets, &output, &scene, &shared, &mut text, before, 0).unwrap();
        let painted = host.paints;
        step_static(&mut host, &mut widgets, &output, &scene, &shared, &mut text, before, 0).unwrap();
        assert_eq!(host.paints, painted);
        step_static(&mut host, &mut widgets, &output, &scene, &shared, &mut text, after, 0).unwrap();
        assert!(host.paints > painted);
        let at = Instant::now();
        assert!(next_wake(&widgets, &names, &scene, &shared, before, at).is_some());
        scene.items[0].enabled = false;
        assert!(next_wake(&widgets, &names, &scene, &shared, before, at).is_none());
        scene.items[0].enabled = true;
        scene.visible.clear();
        assert!(next_wake(&widgets, &names, &scene, &shared, before, at).is_none());
    }

    #[test]
    fn a_dragged_widget_slides_and_is_painted_once_where_it_lands() {
        use mandara_config::desktop_widgets::{Anchor, DecorShape};
        let fonts = FontNames::default();
        let names = ["DP-1"];
        let o = OutputCx { id: 1, names: &names, logical: (1920.0, 1080.0), physical: (1920, 1080), scale120: mandara_core::shm_canvas::SCALE_DENOMINATOR, tone: None, primary: Color::oklch(0.6, 0.15, 200.0), fonts: &fonts, depth: false, subject: None, safe: Insets::default() };
        let mut text = TextPainter::new();
        let mut host = Recorder::default();
        let mut ws = OutWidgets::default();
        let shared = Shared::default();
        let now = Local::now();
        let mut disc = shape("decoration-1", DecorShape::Disc, Anchor::TopLeft);
        let (d0, _) = planes(0);
        step_static(&mut host, &mut ws, &o, &scene(vec![disc.clone()]), &shared, &mut text, now, 0).expect("step");
        let (painted, first) = (host.paints, host.rects[&d0]);
        let mut dragging = scene(vec![]);
        dragging.moving = Some(disc.id.clone());
        for step in 1..=10 {
            disc.place.x = step as f32 * 0.01;
            dragging.items = vec![disc.clone()];
            step_static(&mut host, &mut ws, &o, &dragging, &shared, &mut text, now, 0).expect("step");
        }
        assert_eq!(host.paints, painted, "no repaint while dragged");
        assert_eq!(host.rects[&d0].x - first.x, (0.1_f32 * 1920.0).round() as i32, "the plane followed");
        assert_eq!(host.moves, 10);
        // Released: painted once at the exact place.
        step_static(&mut host, &mut ws, &o, &scene(vec![disc.clone()]), &shared, &mut text, now, 0).expect("step");
        assert_eq!(host.paints, painted + 1);
        step_static(&mut host, &mut ws, &o, &scene(vec![disc.clone()]), &shared, &mut text, now, 0).expect("step");
        assert_eq!(host.paints, painted + 1, "and left alone after that");
    }

    #[test]
    fn hidden_visualizers_release_frame_scratch_and_rebuild_on_show() {
        use mandara_config::desktop_widgets::{Chrome, Look, Plate};
        let fonts = FontNames::default();
        let names = ["DP-1"];
        let output = OutputCx {
            id: 1,
            names: &names,
            logical: (1920.0, 1080.0),
            physical: (1920, 1080),
            scale120: 120,
            tone: None,
            primary: Color::hex("#0088ff"),
            fonts: &fonts,
            depth: false,
            subject: None,
            safe: Default::default(),
        };
        let widget = DesktopWidget {
            kind: WidgetKind::Visualizer(PlaneVisualizer {
                look: Look {
                    plate: Plate::Glass,
                    chrome: Chrome::Window,
                    header: "Spectrum".into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
            place: Placement {
                rotation: 20.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut scene = scene(vec![widget]);
        let mut widgets = OutWidgets::default();
        let mut host = Recorder::default();
        let mut text = TextPainter::new();
        let now = Local::now();
        step_static(
            &mut host,
            &mut widgets,
            &output,
            &scene,
            &Shared::default(),
            &mut text,
            now,
            0,
        )
        .unwrap();
        assert!(widgets.states[0].vis.as_ref().unwrap().1.cached_bytes() > 0);
        let painted = host.paints;
        scene.visible.clear();
        step_static(
            &mut host,
            &mut widgets,
            &output,
            &scene,
            &Shared::default(),
            &mut text,
            now,
            0,
        )
        .unwrap();
        assert!(widgets.states[0].vis.is_none());
        assert_eq!(host.paints, painted);
        scene.visible.push("DP-1".into());
        step_static(
            &mut host,
            &mut widgets,
            &output,
            &scene,
            &Shared::default(),
            &mut text,
            now,
            0,
        )
        .unwrap();
        assert!(widgets.states[0].vis.is_some());
        assert!(host.paints > painted);
    }

    #[test]
    fn plane_set_matches_the_enabled_widgets_after_every_change() {
        use mandara_config::desktop_widgets::{Anchor, DecorShape, DesktopWidgetsConfig};
        let fonts = FontNames::default();
        let names = ["DP-1"];
        let o = OutputCx { id: 1, names: &names, logical: (1920.0, 1080.0), physical: (1920, 1080), scale120: mandara_core::shm_canvas::SCALE_DENOMINATOR, tone: None, primary: Color::oklch(0.6, 0.15, 200.0), fonts: &fonts, depth: false, subject: None, safe: Insets::default() };
        let mut text = TextPainter::new();
        let mut host = Recorder::default();
        let mut ws = OutWidgets::default();
        let shared = Shared::default();
        let now = Local::now();
        let mut cfg = DesktopWidgetsConfig::default();
        // What the renderer is given: the enabled, painted widgets.
        let mut step = |cfg: &DesktopWidgetsConfig, host: &mut Recorder, ws: &mut OutWidgets| {
            let items: Vec<DesktopWidget> = cfg.painted().cloned().collect();
            step_static(host, ws, &o, &scene(items), &shared, &mut text, now, 0).expect("step");
        };
        let disc = shape("decoration-1", DecorShape::Disc, Anchor::TopLeft);
        let stripes = shape("decoration-2", DecorShape::Stripes, Anchor::BottomRight);
        let vis = DesktopWidget { id: "visualizer-1".into(), kind: WidgetKind::Visualizer(PlaneVisualizer::default()), ..Default::default() };
        let (d0, _) = planes(0);
        let (d1, l1) = planes(1);
        let (_, l2) = planes(2);

        // Add: each widget gets exactly the planes it draws.
        cfg.items = vec![disc.clone(), stripes.clone(), vis.clone()];
        step(&cfg, &mut host, &mut ws);
        // The visualizer's plane fills with its first frame (live pass).
        assert_eq!(host.mapped(), [d0, d1], "static planes painted");
        assert!(host.exists.iter().all(|p| [d0, d1].contains(p)));
        let disc_rect = host.rects[&d0];
        let stripes_rect = host.rects[&d1];

        // Remove: the stripes go, the visualizer moves up one index.
        cfg.items = vec![disc.clone(), vis.clone()];
        step(&cfg, &mut host, &mut ws);
        assert_eq!(host.mapped(), [d0], "the removed decoration's plane is gone");
        assert!(!host.has_plane(1, d1) && !host.bytes.contains_key(&d1), "and holds no buffer");
        assert!(!host.has_plane(1, l2), "nothing left at the visualizer's old index");

        // Disable: switched off widgets leave the canvas too.
        cfg.items = vec![disc.clone(), stripes.clone()];
        step(&cfg, &mut host, &mut ws);
        assert_eq!(host.mapped(), [d0, d1]);
        cfg.items[0].enabled = false;
        step(&cfg, &mut host, &mut ws);
        // The renderer is given the enabled widgets only: the stripes take
        // the first planes, the second pair goes.
        assert_eq!(host.mapped(), [d0], "the disabled disc's plane is gone");
        assert_eq!(host.rects[&d0], stripes_rect);
        assert!(!host.has_plane(1, d1));

        // Change kind: a decoration turned visualizer keeps only a live plane.
        cfg.items[0].enabled = true;
        cfg.items[1].kind = vis.kind.clone();
        step(&cfg, &mut host, &mut ws);
        assert_eq!(host.mapped(), [d0], "the decoration's static plane is gone");
        assert!(host.exists.iter().all(|p| *p == d0 || *p == l1));

        // Reorder: each plane shows the widget now at its index.
        cfg.items = vec![stripes.clone(), disc.clone()];
        step(&cfg, &mut host, &mut ws);
        assert_eq!(host.mapped(), [d0, d1]);
        assert_eq!((host.rects[&d0], host.rects[&d1]), (stripes_rect, disc_rect), "planes repainted for their new owners");
        assert!(!host.has_plane(1, l1));

        // Remove everything (a look with only the depth clock): no plane,
        // no buffer left.
        cfg.items.clear();
        step(&cfg, &mut host, &mut ws);
        assert!(host.exists.is_empty() && host.bytes.values().sum::<u64>() == 0, "{:?}", host.exists);
        assert!(ws.states.is_empty());
    }
}
