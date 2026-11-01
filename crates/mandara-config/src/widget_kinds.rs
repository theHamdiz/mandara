//! Settings of the desktop widget kinds added with the widget collection:
//! productivity (to-do list, sticky note, focus timer, quote, notebook
//! tabs) and the rest, each a [`crate::desktop_widgets::WidgetKind`]
//! variant with the shared [`Look`] and, where it writes words or numbers,
//! [`TextOptions`].

use serde::{Deserialize, Serialize};

use crate::desktop_widgets::{Calendar, Look, PlateShape, TextOptions};

/// Bounds and defaults (reference px at a 1080 px tall display unless
/// noted).
pub mod ranges {
    /// Rows a list shows at most.
    pub const ROWS: (u32, u32) = (1, 16);
    pub const TODO_ROWS_DEFAULT: u32 = 7;
    pub const NOTE_LINES_DEFAULT: u32 = 8;
    /// Width of a list or note, reference px.
    pub const WIDTH_PX: (f32, f32) = (160.0, 900.0);
    pub const TODO_WIDTH_DEFAULT_PX: f32 = 340.0;
    pub const NOTE_WIDTH_DEFAULT_PX: f32 = 300.0;
    pub const QUOTE_WIDTH_DEFAULT_PX: f32 = 520.0;
    /// Text size of lists, notes and quotes, reference px.
    pub const TEXT_PX: (f32, f32) = (10.0, 64.0);
    pub const TODO_TEXT_DEFAULT_PX: f32 = 19.0;
    pub const NOTE_TEXT_DEFAULT_PX: f32 = 19.0;
    pub const QUOTE_TEXT_DEFAULT_PX: f32 = 30.0;
    /// Notebook tabs shown.
    pub const TABS: (u32, u32) = (1, 10);
    pub const TABS_DEFAULT: u32 = 5;
    /// The focus timer's ring diameter, reference px.
    pub const TIMER_SIZE_PX: (f32, f32) = (120.0, 520.0);
    pub const TIMER_SIZE_DEFAULT_PX: f32 = 240.0;
    /// Agenda rows.
    pub const AGENDA_ROWS: (u32, u32) = (1, 8);
    pub const AGENDA_ROWS_DEFAULT: u32 = 4;
    /// Sky widgets' size, reference px, and the sun arc's height (of its
    /// width).
    pub const SKY_SIZE_PX: (f32, f32) = (120.0, 900.0);
    pub const SKY_SIZE_DEFAULT_PX: f32 = 420.0;
    pub const ARC_HEIGHT: (f32, f32) = (0.15, 0.6);
    pub const ARC_HEIGHT_DEFAULT: f32 = 0.38;
    /// Hourly weather: hours shown.
    pub const HOURS: (u32, u32) = (4, 24);
    pub const HOURS_DEFAULT: u32 = 12;
    /// Weather disc diameter, reference px.
    pub const DISC_PX: (f32, f32) = (120.0, 480.0);
    pub const DISC_DEFAULT_PX: f32 = 220.0;
    /// Tile columns.
    pub const COLUMNS: (u32, u32) = (1, 8);
    pub const COLUMNS_DEFAULT: u32 = 4;
    pub const TOGGLE_COLUMNS_DEFAULT: u32 = 2;
    pub const PALETTE_COLUMNS_DEFAULT: u32 = 4;
    /// Sliders' length and thickness, reference px.
    pub const SLIDER_LENGTH_PX: (f32, f32) = (80.0, 600.0);
    pub const SLIDER_LENGTH_DEFAULT_PX: f32 = 180.0;
    pub const SLIDER_THICKNESS_PX: (f32, f32) = (12.0, 90.0);
    pub const SLIDER_THICKNESS_DEFAULT_PX: f32 = 44.0;
    /// The pet's size, reference px.
    pub const PET_SIZE_PX: (f32, f32) = (60.0, 400.0);
    pub const PET_SIZE_DEFAULT_PX: f32 = 140.0;
    /// Bubbles: how many, their size range, rise speed (reference px per
    /// second) and the area they float in.
    pub const BUBBLES: (u32, u32) = (3, 40);
    pub const BUBBLES_DEFAULT: u32 = 14;
    pub const BUBBLE_PX: (f32, f32) = (8.0, 200.0);
    pub const BUBBLE_MIN_DEFAULT_PX: f32 = 18.0;
    pub const BUBBLE_MAX_DEFAULT_PX: f32 = 70.0;
    pub const BUBBLE_SPEED: (f32, f32) = (5.0, 120.0);
    pub const BUBBLE_SPEED_DEFAULT: f32 = 24.0;
    pub const BUBBLES_WIDTH_DEFAULT_PX: f32 = 520.0;
    pub const BUBBLES_HEIGHT_DEFAULT_PX: f32 = 700.0;
    pub const AREA_PX: (f32, f32) = (100.0, 2400.0);
    /// Photo frame: minutes per picture and its size.
    pub const PHOTO_MINUTES: (u32, u32) = (1, 1440);
    pub const PHOTO_MINUTES_DEFAULT: u32 = 30;
    pub const PHOTO_W_DEFAULT_PX: f32 = 320.0;
    pub const PHOTO_H_DEFAULT_PX: f32 = 240.0;
    /// Sticker: size and its cut-out border, reference px.
    pub const STICKER_SIZE_DEFAULT_PX: f32 = 220.0;
    pub const STICKER_BORDER_PX: (f32, f32) = (0.0, 20.0);
    pub const STICKER_BORDER_DEFAULT_PX: f32 = 6.0;
    /// Media widgets: the cover's size, the record's grooves, the halftone
    /// dot pitch and the lyric lines.
    pub const MEDIA_SIZE_PX: (f32, f32) = (80.0, 600.0);
    pub const MEDIA_SIZE_DEFAULT_PX: f32 = 220.0;
    pub const GROOVES: (u32, u32) = (0, 40);
    pub const GROOVES_DEFAULT: u32 = 14;
    pub const DOT_PITCH_PX: (f32, f32) = (2.0, 16.0);
    pub const DOT_PITCH_DEFAULT_PX: f32 = 5.0;
    pub const LYRIC_LINES: (u32, u32) = (1, 9);
    pub const LYRIC_LINES_DEFAULT: u32 = 5;
    /// Screen time: days of bars and apps listed.
    pub const SCREEN_DAYS: (u32, u32) = (1, 14);
    pub const SCREEN_DAYS_DEFAULT: u32 = 7;
    pub const SCREEN_APPS: (u32, u32) = (0, 8);
    pub const SCREEN_APPS_DEFAULT: u32 = 3;
    /// The search field's width, reference px.
    pub const SEARCH_W_DEFAULT_PX: f32 = 520.0;
    /// GitHub: weeks of the calendar.
    pub const WEEKS: (u32, u32) = (4, 53);
    pub const WEEKS_DEFAULT: u32 = 20;
    /// Currency: decimals shown.
    pub const DECIMALS: (u32, u32) = (0, 6);
    pub const DECIMALS_DEFAULT: u32 = 2;
    /// AI usage: days of bars, and the budget's range (millions of tokens).
    pub const AI_DAYS: (u32, u32) = (1, 30);
    pub const AI_DAYS_DEFAULT: u32 = 7;
    pub const AI_BUDGET_M: (f32, f32) = (0.0, 2_000.0);
    /// Games: covers on the shelf.
    pub const GAMES: (u32, u32) = (1, 12);
    pub const GAMES_DEFAULT: u32 = 5;
}

/// A to-do checklist from the To-do plugin's lists; a click ticks a task.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TodoWidget {
    /// A list's name (empty = every list).
    pub list: String,
    /// Most rows shown.
    pub rows: u32,
    /// Ticked tasks stay in the list, struck through.
    pub show_done: bool,
    pub checkbox: PlateShape,
    /// Rules under every row, like lined paper.
    pub ruled: bool,
    /// The list's name and the open count above it.
    pub title: bool,
    /// A last row that opens the To-do panel to add a task.
    pub add_row: bool,
    pub width: f32,
    pub text_size: f32,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for TodoWidget {
    fn default() -> Self {
        Self {
            list: String::new(),
            rows: ranges::TODO_ROWS_DEFAULT,
            show_done: true,
            checkbox: PlateShape::Squircle,
            ruled: false,
            title: true,
            add_row: true,
            width: ranges::TODO_WIDTH_DEFAULT_PX,
            text_size: ranges::TODO_TEXT_DEFAULT_PX,
            look: Look::default(),
            locale: TextOptions::default(),
        }
    }
}

/// The paper a sticky note is written on.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NoteSheet {
    /// The widget's plate (or nothing).
    Theme,
    /// A pastel of the theme's accent.
    #[default]
    Pastel,
    /// White paper.
    Paper,
    /// `sheet_color`.
    Custom,
}

/// Lines on a note.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NoteRuling {
    #[default]
    Plain,
    Ruled,
    Grid,
    Dots,
}

/// A note from the Notes plugin on the desktop; a click opens it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NoteWidget {
    /// The note's title (empty = the most recently edited).
    pub note: String,
    pub lines: u32,
    pub sheet: NoteSheet,
    pub sheet_color: String,
    pub ruling: NoteRuling,
    /// The word count in a corner.
    pub word_count: bool,
    pub width: f32,
    pub text_size: f32,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for NoteWidget {
    fn default() -> Self {
        Self {
            note: String::new(),
            lines: ranges::NOTE_LINES_DEFAULT,
            sheet: NoteSheet::Pastel,
            sheet_color: String::new(),
            ruling: NoteRuling::Ruled,
            word_count: false,
            width: ranges::NOTE_WIDTH_DEFAULT_PX,
            text_size: ranges::NOTE_TEXT_DEFAULT_PX,
            look: Look::default(),
            locale: TextOptions::default(),
        }
    }
}

/// How the focus timer draws.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TimerStyle {
    #[default]
    Ring,
    /// The time over a row of dots, one per focus session of the set.
    Dots,
}

/// The Pomodoro focus timer; a click starts or pauses it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PomodoroWidget {
    pub style: TimerStyle,
    /// Skip and reset buttons under it.
    pub controls: bool,
    pub size: f32,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for PomodoroWidget {
    fn default() -> Self {
        Self { style: TimerStyle::Ring, controls: true, size: ranges::TIMER_SIZE_DEFAULT_PX, look: Look::default(), locale: TextOptions::default() }
    }
}

/// How often the quote changes.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QuoteEvery {
    #[default]
    Day,
    Hour,
    /// Only when clicked.
    Click,
}

/// A quote card: your own quotes, or a few built-in sayings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuoteWidget {
    /// One per entry, `text — author` (empty = the built-in sayings).
    pub quotes: Vec<String>,
    pub every: QuoteEvery,
    /// Large opening quotation mark.
    pub mark: bool,
    pub width: f32,
    pub text_size: f32,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for QuoteWidget {
    fn default() -> Self {
        Self { quotes: Vec::new(), every: QuoteEvery::Day, mark: true, width: ranges::QUOTE_WIDTH_DEFAULT_PX, text_size: ranges::QUOTE_TEXT_DEFAULT_PX, look: Look::default(), locale: TextOptions::default() }
    }
}

/// Vertical notebook tabs: your recent notes as tabs on a screen edge; a
/// click opens one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotebookWidget {
    pub tabs: u32,
    /// The tabs' shape.
    pub tab_shape: PlateShape,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for NotebookWidget {
    fn default() -> Self {
        Self { tabs: ranges::TABS_DEFAULT, tab_shape: PlateShape::Rect, look: Look::default(), locale: TextOptions::default() }
    }
}

/// `v` inside `(lo, hi)`, or `fallback` when it isn't a number.
pub fn within(v: f32, (lo, hi): (f32, f32), fallback: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { fallback }
}

/// A count inside `(lo, hi)`.
pub fn count(v: u32, (lo, hi): (u32, u32)) -> u32 {
    v.clamp(lo, hi)
}

// ── Calendar ─────────────────────────────────────────────────────────────

/// How a date widget lays out.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DateLayout {
    /// The month as a grid, today marked by a shape.
    #[default]
    Month,
    /// This week on one row, today in a pill.
    Week,
    /// The next events, each with a date chip.
    Agenda,
    /// A big tile: the weekday, the day's numeral, the month.
    Tile,
}

/// A painted calendar: a month grid, a week strip, what's coming up or a
/// big date tile, with events from Studio → Calendar's sources.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DateWidget {
    pub layout: DateLayout,
    /// The shape that marks today (and the agenda's date chips).
    pub marker: PlateShape,
    /// ISO week numbers in front of the month's rows.
    pub week_numbers: bool,
    /// A dot under days with events.
    pub event_dots: bool,
    /// Agenda rows.
    pub rows: u32,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for DateWidget {
    fn default() -> Self {
        Self { layout: DateLayout::Month, marker: PlateShape::Circle, week_numbers: false, event_dots: true, rows: ranges::AGENDA_ROWS_DEFAULT, look: Look::default(), locale: TextOptions::default() }
    }
}

// ── Sky ──────────────────────────────────────────────────────────────────

/// What a sky widget shows.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkyLayout {
    /// The sun on its path from sunrise to sunset (the moon at night).
    #[default]
    SunArc,
    /// Tonight's moon: its phase and how much is lit.
    Moon,
    /// The Earth with day and night where they really are, your place and
    /// the moon over it.
    Globe,
}

/// The sky over your location, worked out from the time (no network).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkyWidget {
    pub layout: SkyLayout,
    /// Size (the arc's width, the moon's or the globe's diameter),
    /// reference px.
    pub size: f32,
    /// Sun arc: dashed path, hour ticks, its height (fraction of the
    /// width).
    pub dashed: bool,
    pub ticks: bool,
    pub arc_height: f32,
    /// Times and names under it.
    pub labels: bool,
    /// Moon: the dark side faintly lit and a soft terminator.
    pub shaded: bool,
    /// Globe: the moon over the place it stands above.
    pub moon: bool,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for SkyWidget {
    fn default() -> Self {
        Self { layout: SkyLayout::SunArc, size: ranges::SKY_SIZE_DEFAULT_PX, dashed: true, ticks: false, arc_height: ranges::ARC_HEIGHT_DEFAULT, labels: true, shaded: true, moon: true, look: Look::default(), locale: TextOptions::default() }
    }
}

// ── Identity ─────────────────────────────────────────────────────────────

/// A profile card: your picture, name, `user@host`, uptime and a status.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileWidget {
    pub avatar: PlateShape,
    /// `user@host` under the name.
    pub login: bool,
    /// How long the computer has been up.
    pub uptime: bool,
    /// A status pill: your own words (empty = "Online", or "Do not
    /// disturb" while it's on).
    pub status: String,
    pub show_status: bool,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for ProfileWidget {
    fn default() -> Self {
        Self { avatar: PlateShape::Circle, login: true, uptime: true, status: String::new(), show_status: true, look: Look::default(), locale: TextOptions::default() }
    }
}

/// How shortcut tiles are laid out.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TileLayout {
    #[default]
    Grid,
    /// One under another, the name beside the icon.
    List,
}

/// Launch tiles: apps (desktop ids), folders or web addresses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutsWidget {
    /// `firefox.desktop`, `~/Documents`, `https://…`; empty = your dock's
    /// apps (or, for folders, your home folders).
    pub items: Vec<String>,
    /// Folders (opened in the file manager) rather than apps.
    pub folders: bool,
    pub layout: TileLayout,
    pub columns: u32,
    pub tile: PlateShape,
    /// Each tile in its own pastel.
    pub colorful: bool,
    pub labels: bool,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for ShortcutsWidget {
    fn default() -> Self {
        Self { items: Vec::new(), folders: false, layout: TileLayout::Grid, columns: ranges::COLUMNS_DEFAULT, tile: PlateShape::Squircle, colorful: true, labels: true, look: Look::default(), locale: TextOptions::default() }
    }
}

/// Quick toggles on the desktop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TogglesWidget {
    /// `wifi`, `bluetooth`, `dnd`, `night-light`, `keep-awake`, `dark`,
    /// `mute` (empty = Wi-Fi, Bluetooth, do not disturb, night light).
    pub toggles: Vec<String>,
    /// The shape while off, and the one it turns into when on.
    pub shape_off: PlateShape,
    pub shape_on: PlateShape,
    pub columns: u32,
    pub labels: bool,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for TogglesWidget {
    fn default() -> Self {
        Self { toggles: Vec::new(), shape_off: PlateShape::Circle, shape_on: PlateShape::Squircle, columns: ranges::TOGGLE_COLUMNS_DEFAULT, labels: true, look: Look::default(), locale: TextOptions::default() }
    }
}

/// Volume and brightness as sliders you click or scroll.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SlidersWidget {
    /// `volume`, `brightness` (empty = both).
    pub levels: Vec<String>,
    /// Upright bars (else lying down).
    pub vertical: bool,
    pub length: f32,
    pub thickness: f32,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for SlidersWidget {
    fn default() -> Self {
        Self { levels: Vec::new(), vertical: true, length: ranges::SLIDER_LENGTH_DEFAULT_PX, thickness: ranges::SLIDER_THICKNESS_DEFAULT_PX, look: Look::default(), locale: TextOptions::default() }
    }
}

/// The theme's colours as swatches; a click copies one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaletteWidget {
    pub swatch: PlateShape,
    /// The roles' names and hex codes under the swatches.
    pub labels: bool,
    pub columns: u32,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for PaletteWidget {
    fn default() -> Self {
        Self { swatch: PlateShape::Circle, labels: true, columns: ranges::PALETTE_COLUMNS_DEFAULT, look: Look::default(), locale: TextOptions::default() }
    }
}

// ── Fun ──────────────────────────────────────────────────────────────────

/// A little cat on the desktop that breathes, blinks, sleeps at night,
/// bobs along to music and purrs when you click it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PetWidget {
    pub size: f32,
    /// Its name, shown when you click it (empty = none).
    pub name: String,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for PetWidget {
    fn default() -> Self {
        Self { size: ranges::PET_SIZE_DEFAULT_PX, name: String::new(), look: Look::default(), locale: TextOptions::default() }
    }
}

/// Soap bubbles drifting up while the desktop shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BubblesWidget {
    pub count: u32,
    /// Smallest and largest bubble, reference px.
    pub min_size: f32,
    pub max_size: f32,
    /// Rise speed, reference px per second.
    pub speed: f32,
    pub width: f32,
    pub height: f32,
    pub look: Look,
}

impl Default for BubblesWidget {
    fn default() -> Self {
        Self {
            count: ranges::BUBBLES_DEFAULT,
            min_size: ranges::BUBBLE_MIN_DEFAULT_PX,
            max_size: ranges::BUBBLE_MAX_DEFAULT_PX,
            speed: ranges::BUBBLE_SPEED_DEFAULT,
            width: ranges::BUBBLES_WIDTH_DEFAULT_PX,
            height: ranges::BUBBLES_HEIGHT_DEFAULT_PX,
            look: Look::default(),
        }
    }
}

/// Pictures from a folder, one at a time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PhotoWidget {
    /// A folder of pictures (empty = ~/Pictures).
    pub folder: String,
    /// Minutes each picture stays (a click shows the next).
    pub minutes: u32,
    pub crop: PlateShape,
    pub width: f32,
    pub height: f32,
    pub look: Look,
}

impl Default for PhotoWidget {
    fn default() -> Self {
        Self { folder: String::new(), minutes: ranges::PHOTO_MINUTES_DEFAULT, crop: PlateShape::Rect, width: ranges::PHOTO_W_DEFAULT_PX, height: ranges::PHOTO_H_DEFAULT_PX, look: Look::default() }
    }
}

/// Any picture as a sticker, cut out with an outline round it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StickerWidget {
    pub image: String,
    pub size: f32,
    /// The white cut-out border round its shape, reference px.
    pub border: f32,
    pub look: Look,
}

impl Default for StickerWidget {
    fn default() -> Self {
        Self { image: String::new(), size: ranges::STICKER_SIZE_DEFAULT_PX, border: ranges::STICKER_BORDER_DEFAULT_PX, look: Look::default() }
    }
}

// ── Media ────────────────────────────────────────────────────────────────

/// How a media widget shows what plays.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MediaLayout {
    /// The cover on a turning record, the track beside it.
    #[default]
    Vinyl,
    /// A pocket player: a body with an LCD and buttons.
    Device,
    /// The cover cut to a shape that changes with every track.
    Shaped,
    /// A tall pill: the cover on top, the track written down its side.
    SidePill,
    /// The cover as a dot screen (or one-bit dither).
    Halftone,
    /// Synced lyrics, the current line lit.
    Lyrics,
}

/// What plays, painted on the desktop (only while a player is loaded).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaWidget {
    pub layout: MediaLayout,
    /// The cover's (record's, pill's) size, reference px.
    pub size: f32,
    /// Vinyl: the record turns while music plays; its grooves.
    pub spin: bool,
    pub grooves: u32,
    /// Shaped: the shapes it picks from, one per track.
    pub shapes: Vec<PlateShape>,
    /// Halftone: the dot pitch (reference px) and one-bit dither instead
    /// of dots.
    pub dot_pitch: f32,
    pub dither: bool,
    /// Lyrics: lines shown.
    pub lines: u32,
    /// Play / pause, previous and next buttons.
    pub controls: bool,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for MediaWidget {
    fn default() -> Self {
        Self {
            layout: MediaLayout::Vinyl,
            size: ranges::MEDIA_SIZE_DEFAULT_PX,
            spin: true,
            grooves: ranges::GROOVES_DEFAULT,
            shapes: vec![PlateShape::Cookie, PlateShape::Flower, PlateShape::Squircle, PlateShape::Pentagon, PlateShape::Circle],
            dot_pitch: ranges::DOT_PITCH_DEFAULT_PX,
            dither: false,
            lines: ranges::LYRIC_LINES_DEFAULT,
            controls: true,
            look: Look::default(),
            locale: TextOptions::default(),
        }
    }
}

// ── Information ──────────────────────────────────────────────────────────

/// How the machine card lays out.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FetchLayout {
    /// Like a terminal fetch: `user@host`, then `key: value` lines.
    #[default]
    Terminal,
    /// Rows with an icon each.
    Rows,
    /// Tiles in a grid.
    Grid,
}

/// Your machine at a glance: system, kernel, compositor, processor,
/// graphics, memory, uptime, shell, screen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FetchWidget {
    pub layout: FetchLayout,
    /// The distribution's logo.
    pub logo: bool,
    /// A row of the theme's colours under it.
    pub colors: bool,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for FetchWidget {
    fn default() -> Self {
        Self { layout: FetchLayout::Terminal, logo: true, colors: true, look: Look::default(), locale: TextOptions::default() }
    }
}

/// Today's screen time from the Screen time plugin: the total, the last
/// days as bars and the top apps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenTimeWidget {
    /// Days of bars (1 = today only).
    pub days: u32,
    pub apps: u32,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for ScreenTimeWidget {
    fn default() -> Self {
        Self { days: ranges::SCREEN_DAYS_DEFAULT, apps: ranges::SCREEN_APPS_DEFAULT, look: Look::default(), locale: TextOptions::default() }
    }
}

/// How the prayer times widget lays out.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrayerLayout {
    /// The next prayer and the time to it inside a ring of the day.
    #[default]
    Ring,
    /// Every time of the day, the next one lit.
    List,
}

/// The next prayer with a countdown, from the Prayer times plugin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrayerWidget {
    pub layout: PrayerLayout,
    /// Prayer names in Arabic whatever the widget's language.
    pub arabic_names: bool,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for PrayerWidget {
    fn default() -> Self {
        // A prayer widget dates the day by the Hijri calendar.
        Self { layout: PrayerLayout::Ring, arabic_names: false, look: Look::default(), locale: TextOptions { calendar: Calendar::Hijri, ..TextOptions::default() } }
    }
}

/// A search field on the desktop: a click opens the launcher.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchWidget {
    pub width: f32,
    pub shape: PlateShape,
    /// Your own words in the field (empty = "Search apps, files and more").
    pub placeholder: String,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for SearchWidget {
    fn default() -> Self {
        Self { width: ranges::SEARCH_W_DEFAULT_PX, shape: PlateShape::Rect, placeholder: String::new(), look: Look::default(), locale: TextOptions::default() }
    }
}

/// At a glance: the date, the weather and the one thing worth knowing now
/// (an event soon, the next prayer, a low battery, unread notifications,
/// what plays).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlanceWidget {
    pub weather: bool,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for GlanceWidget {
    fn default() -> Self {
        Self { weather: true, look: Look::default(), locale: TextOptions::default() }
    }
}

// ── Online and local feeds ───────────────────────────────────────────────

/// GitHub's public contribution calendar for a user (asks github.com
/// only when `online` is on; cached).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GithubWidget {
    pub user: String,
    /// Fetch from github.com (off until you switch it on).
    pub online: bool,
    pub weeks: u32,
    pub cell: PlateShape,
    /// The total and the current streak under the grid.
    pub summary: bool,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for GithubWidget {
    fn default() -> Self {
        Self { user: String::new(), online: false, weeks: ranges::WEEKS_DEFAULT, cell: PlateShape::Rect, summary: true, look: Look::default(), locale: TextOptions::default() }
    }
}

/// Exchange rates (asks the rates service only when `online` is on;
/// cached for the day).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CurrencyWidget {
    /// The currency the others are priced in (`USD`).
    pub base: String,
    /// Currencies shown (`EGP`, `EUR`, …).
    pub pairs: Vec<String>,
    pub decimals: u32,
    pub online: bool,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for CurrencyWidget {
    fn default() -> Self {
        Self { base: "USD".into(), pairs: vec!["EUR".into(), "GBP".into(), "EGP".into()], decimals: ranges::DECIMALS_DEFAULT, online: false, look: Look::default(), locale: TextOptions::default() }
    }
}

/// Claude Code's token use from its local session logs (read only when
/// `read_logs` is on).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiUsageWidget {
    pub read_logs: bool,
    pub days: u32,
    /// A weekly budget in millions of tokens for a meter (0 = none).
    pub budget_millions: f32,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for AiUsageWidget {
    fn default() -> Self {
        Self { read_logs: false, days: ranges::AI_DAYS_DEFAULT, budget_millions: 0.0, look: Look::default(), locale: TextOptions::default() }
    }
}

/// Your installed Steam games as a shelf of covers; a click starts one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GamesWidget {
    pub count: u32,
    pub cover: PlateShape,
    /// The names under the covers.
    pub names: bool,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for GamesWidget {
    fn default() -> Self {
        Self { count: ranges::GAMES_DEFAULT, cover: PlateShape::Rect, names: true, look: Look::default(), locale: TextOptions::default() }
    }
}
