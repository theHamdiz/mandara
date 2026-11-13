//! Desktop widgets (`[desktop.widgets]`): things drawn on the wallpaper
//! layer — hero clocks, a system monitor column, static decorations and an
//! audio visualizer that can sit *behind* the wallpaper's subject — plus
//! interactive cards hosted on small layer surfaces.
//!
//! The model is shared by every kind of desktop widget so Studio's
//! "Desktop widgets" gallery can add, remove and arrange them in one place:
//!
//! * [`DesktopWidget`] — one placed widget: a stable `id`, `enabled`, a
//!   [`Placement`] (displays, anchor, offset, scale, opacity, depth) and its
//!   [`WidgetKind`] with the kind's own settings;
//! * [`DesktopWidgetsConfig::items`] — the list, in paint order (later on
//!   top), for every display; a widget lists the displays it shows on
//!   (empty = every display that shows the wallpaper).
//!
//! In TOML every widget is one `[[desktop.widgets.items]]` table whose
//! `kind` names the variant; the variant's fields sit beside the common
//! ones:
//!
//! ```toml
//! [[desktop.widgets.items]]
//! id = "clock-1"
//! kind = "clock"
//! face = "day-block"
//! [desktop.widgets.items.place]
//! anchor = "top-left"
//! x = 0.04
//! y = 0.08
//! ```
//!
//! Everything is off by default: an empty list costs nothing. The depth
//! clock that predates this model (`[wallpaper] desktop_clock`, its style,
//! position and date pill) is moved in as the first clock widget by
//! [`migrate_depth_clock`]; from then on the widget is the source and the
//! old `[wallpaper]` keys are kept in step for older readers
//! ([`sync_depth_clock`]).
//!
//! Kinds may be added; existing names and fields are never renamed (other
//! surfaces — e.g. the interactive cards — build on them).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::widget_style::ranges as style;
pub use crate::widget_kinds::*;
pub use crate::widget_style::{Chrome, PlateShape, ShadowKind};
use crate::{ClockStyle, ShellConfig};

/// Bounds and defaults of the widget settings, in one place.
pub mod ranges {
    /// Most widgets in the list (each paints into one or two planes).
    pub const MAX_WIDGETS: usize = 24;
    /// Widget scale (1 = designed for a 1080 px tall display).
    pub const SCALE: (f32, f32) = (0.25, 4.0);
    /// Offset from the anchor, as a fraction of the display's width / height.
    pub const OFFSET: (f32, f32) = (-1.0, 1.0);
    pub const OPACITY: (f32, f32) = (0.0, 1.0);
    /// Height every widget is designed for; sizes are reference pixels at it.
    pub const REFERENCE_HEIGHT_PX: f32 = 1080.0;
    /// System monitor refresh, seconds.
    pub const MONITOR_INTERVAL_S: (u32, u32) = (1, 60);
    pub const MONITOR_INTERVAL_DEFAULT_S: u32 = 2;
    /// System monitor column width, reference px.
    pub const MONITOR_WIDTH_PX: (f32, f32) = (160.0, 520.0);
    pub const MONITOR_WIDTH_DEFAULT_PX: f32 = 260.0;
    /// Visualizer frame cap, frames per second.
    pub const VISUALIZER_FPS: (u32, u32) = (15, 60);
    pub const VISUALIZER_FPS_DEFAULT: u32 = 30;
    /// Seconds of silence after which the visualizer freezes and stops
    /// listening.
    pub const VISUALIZER_SILENCE_S: (u32, u32) = (1, 30);
    pub const VISUALIZER_SILENCE_DEFAULT_S: u32 = 4;
    /// Visualizer bars / spikes.
    pub const VISUALIZER_BARS: (u32, u32) = (8, 96);
    pub const VISUALIZER_BARS_DEFAULT: u32 = 40;
    /// Visualizer length along its line (bars, wave) or radius (radial),
    /// reference px.
    pub const VISUALIZER_LENGTH_PX: (f32, f32) = (40.0, 2400.0);
    pub const VISUALIZER_LENGTH_DEFAULT_PX: f32 = 520.0;
    /// Tallest bar / wave swing, reference px.
    pub const VISUALIZER_HEIGHT_PX: (f32, f32) = (8.0, 600.0);
    pub const VISUALIZER_HEIGHT_DEFAULT_PX: f32 = 90.0;
    /// Gap between bars, fraction of a bar slot.
    pub const VISUALIZER_GAP: (f32, f32) = (0.0, 0.9);
    pub const VISUALIZER_GAP_DEFAULT: f32 = 0.35;
    /// Rotation, degrees.
    pub const ANGLE_DEG: (f32, f32) = (-180.0, 180.0);
    /// Decoration text size and shape extent, reference px.
    pub const TEXT_SIZE_PX: (f32, f32) = (6.0, 640.0);
    pub const TEXT_SIZE_DEFAULT_PX: f32 = 48.0;
    pub const SHAPE_SIZE_PX: (f32, f32) = (2.0, 2400.0);
    pub const SHAPE_SIZE_DEFAULT_PX: f32 = 180.0;
    pub const STROKE_PX: (f32, f32) = (0.5, 40.0);
    pub const STROKE_DEFAULT_PX: f32 = 2.0;
    /// Stripes / waves in a block.
    pub const REPEAT: (u32, u32) = (1, 40);
    pub const REPEAT_DEFAULT: u32 = 5;
    /// Letter spacing, em.
    pub const TRACKING_EM: (f32, f32) = (-0.2, 1.5);
    /// Font weight (CSS scale).
    pub const WEIGHT: (u16, u16) = (100, 900);
    pub const WEIGHT_DEFAULT: u16 = 400;
    /// Shadow / glow strength (1 = as designed).
    pub const EFFECT_STRENGTH: (f32, f32) = (0.1, 2.0);
    pub const EFFECT_STRENGTH_DEFAULT: f32 = 1.0;
    /// The plate behind a widget: opacity, corner radius and padding
    /// (reference px).
    pub const PLATE_OPACITY: (f32, f32) = (0.05, 1.0);
    pub const PLATE_OPACITY_DEFAULT: f32 = 0.55;
    pub const PLATE_RADIUS_PX: (f32, f32) = (0.0, 64.0);
    pub const PLATE_RADIUS_DEFAULT_PX: f32 = 24.0;
    pub const PLATE_PADDING_PX: (f32, f32) = (0.0, 96.0);
    pub const PLATE_PADDING_DEFAULT_PX: f32 = 24.0;
    /// World clock: most cities on one widget.
    pub const WORLD_CLOCK_CITIES: usize = 8;
    /// Weather: forecast days shown.
    pub const WEATHER_DAYS: (u32, u32) = (0, 7);
    pub const WEATHER_DAYS_DEFAULT: u32 = 4;
    /// Analog hands' width, reference px.
    pub const HAND_WIDTH_PX: (f32, f32) = (2.0, 24.0);
    pub const HAND_WIDTH_DEFAULT_PX: f32 = 5.0;
    /// Dot-matrix: brightness of the unlit dots.
    pub const UNLIT: (f32, f32) = (0.0, 0.5);
    pub const UNLIT_DEFAULT: f32 = 0.12;
    /// Stack face: gap between hours and minutes (fraction of a line).
    pub const LINE_GAP: (f32, f32) = (-0.4, 0.6);
    pub const LINE_GAP_DEFAULT: f32 = -0.18;
    /// HUD face: slant, degrees.
    pub const SLANT_DEG: (f32, f32) = (-30.0, 30.0);
    pub const SLANT_DEFAULT_DEG: f32 = 12.0;
    /// Numerals face: the giant numerals' strength.
    pub const NUMERALS: (f32, f32) = (0.05, 1.0);
    pub const NUMERALS_DEFAULT: f32 = 0.22;
    /// Template face: a new line's size, and most lines.
    pub const TEMPLATE_SIZE_DEFAULT_PX: f32 = 32.0;
    pub const TEMPLATE_LINES: usize = 6;
    /// Progress: ring / bar thickness, reference px.
    pub const PROGRESS_THICKNESS_PX: (f32, f32) = (2.0, 40.0);
    pub const PROGRESS_THICKNESS_DEFAULT_PX: f32 = 12.0;
    /// Monitor gauges: ring width (reference px), where the ring starts
    /// (degrees from the top; the gauges leave a gap at the bottom), and
    /// meter segments.
    pub const RING_WIDTH_PX: (f32, f32) = (1.0, 24.0);
    pub const RING_WIDTH_DEFAULT_PX: f32 = 6.5;
    pub const START_ANGLE_DEFAULT_DEG: f32 = -135.0;
    pub const SEGMENTS: (u32, u32) = (3, 40);
    pub const SEGMENTS_DEFAULT: u32 = 10;
}

/// The `[desktop.widgets]` defaults this build ships (see
/// [`crate::upgrade`]): since version 1 a widget's numerals, calendar and
/// font are its own settings (numerals and font follow the language and the
/// widgets font by default); an older list is upgraded to show exactly
/// what it showed before.
pub const WIDGETS_VERSION: u32 = 1;

// ── Shared model ─────────────────────────────────────────────────────────

/// `[desktop]`: the desktop (wallpaper layer) beyond the image itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopConfig {
    pub widgets: DesktopWidgetsConfig,
    /// `[desktop.menu]`: the right-click menu ([`crate::desktop_menu`]).
    pub menu: crate::desktop_menu::DesktopMenuConfig,
}

/// `[desktop.widgets]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopWidgetsConfig {
    /// The defaults this list was written for ([`WIDGETS_VERSION`]);
    /// missing in older files.
    #[serde(default)]
    pub version: u32,
    /// The `[wallpaper]` depth clock has been moved into [`Self::items`].
    pub migrated: bool,
    /// Every widget (the depth clock and the cards too) is hidden for now,
    /// without switching any off: the desktop menu's "Hide desktop widgets"
    /// (`mandara msg desktop widgets hide`). Edit mode shows them anyway.
    pub hidden: bool,
    /// Widgets in paint order (later ones on top).
    pub items: Vec<DesktopWidget>,
}

impl Default for DesktopWidgetsConfig {
    fn default() -> Self {
        Self { version: WIDGETS_VERSION, migrated: false, hidden: false, items: Vec::new() }
    }
}

/// One widget placed on the desktop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopWidget {
    /// Stable id (`clock-1`, `monitor-2`, …), unique in the list.
    pub id: String,
    pub enabled: bool,
    pub place: Placement,
    #[serde(flatten)]
    pub kind: WidgetKind,
}

impl Default for DesktopWidget {
    fn default() -> Self {
        Self { id: String::new(), enabled: true, place: Placement::default(), kind: WidgetKind::Clock(ClockWidget::default()) }
    }
}

/// Where a widget sits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Placement {
    /// Connector names (`DP-1`) it shows on; empty = every display.
    pub displays: Vec<String>,
    /// The point of the display — and of the widget's box — that are
    /// pinned together.
    pub anchor: Anchor,
    /// Offset from the anchor, fractions of the display's width / height.
    pub x: f32,
    pub y: f32,
    /// 1 = designed size on a 1080 px tall display (scales with height).
    pub scale: f32,
    pub opacity: f32,
    /// Under the wallpaper's subject (depth effect) or over it.
    pub depth: Depth,
    /// Turned about its centre, degrees clockwise.
    pub rotation: f32,
    /// Mirrored left to right / top to bottom.
    pub flip_x: bool,
    pub flip_y: bool,
}

impl Default for Placement {
    fn default() -> Self {
        Self { displays: Vec::new(), anchor: Anchor::Center, x: 0.0, y: 0.0, scale: 1.0, opacity: 1.0, depth: Depth::Behind, rotation: 0.0, flip_x: false, flip_y: false }
    }
}

impl Placement {
    /// Whether the widget shows on an output known by any of `names`.
    pub fn shows_on(&self, names: &[&str]) -> bool {
        self.displays.is_empty() || self.displays.iter().any(|d| names.iter().any(|n| n.eq_ignore_ascii_case(d)))
    }

    pub fn scale(&self) -> f32 {
        self.scale.clamp(ranges::SCALE.0, ranges::SCALE.1)
    }

    pub fn opacity(&self) -> f32 {
        self.opacity.clamp(ranges::OPACITY.0, ranges::OPACITY.1)
    }

    pub fn rotation(&self) -> f32 {
        crate::widget_style::ranges::ROTATION_DEG.clamp(self.rotation, 0.0)
    }

    /// Turned or mirrored (drawn through a transform).
    pub fn transformed(&self) -> bool {
        self.rotation() != 0.0 || self.flip_x || self.flip_y
    }

    /// Top-left of a `w × h` box on a `out_w × out_h` display (same units),
    /// kept fully on the display.
    pub fn origin(&self, out: (f32, f32), size: (f32, f32)) -> (f32, f32) {
        self.origin_in(out, size, Insets::default())
    }

    /// [`Self::origin`], kept inside the display less `safe` (the bar's
    /// exclusive zone, the screen frame): whatever the offsets, resolution
    /// or scale, the whole box stays visible and clear of the bar.
    pub fn origin_in(&self, (out_w, out_h): (f32, f32), (w, h): (f32, f32), safe: Insets) -> (f32, f32) {
        let (fx, fy) = self.anchor.fraction();
        let (ox, oy) = (self.x.clamp(ranges::OFFSET.0, ranges::OFFSET.1) * out_w, self.y.clamp(ranges::OFFSET.0, ranges::OFFSET.1) * out_h);
        let (x, y) = (fx * out_w + ox - fx * w, fy * out_h + oy - fy * h);
        (fit(x, w, safe.left, out_w - safe.right), fit(y, h, safe.top, out_h - safe.bottom))
    }

    /// The offset that puts a `w × h` box's top-left at `(left, top)`
    /// (dragging on Studio's preview), keeping the anchor. The box is kept
    /// on the display, as [`Self::origin`] shows it.
    pub fn offset_for(&self, (out_w, out_h): (f32, f32), (w, h): (f32, f32), (left, top): (f32, f32)) -> (f32, f32) {
        let (fx, fy) = self.anchor.fraction();
        let (left, top) = (fit(left, w, 0.0, out_w), fit(top, h, 0.0, out_h));
        let x = (left + fx * w - fx * out_w) / out_w.max(1.0);
        let y = (top + fy * h - fy * out_h) / out_h.max(1.0);
        (x.clamp(ranges::OFFSET.0, ranges::OFFSET.1), y.clamp(ranges::OFFSET.0, ranges::OFFSET.1))
    }
}

/// Start of a `len` long span placed at `at`, moved inside `lo..hi`
/// (centred on it when it can't fit).
fn fit(at: f32, len: f32, lo: f32, hi: f32) -> f32 {
    if len > hi - lo { lo + (hi - lo - len) / 2.0 } else { at.clamp(lo, hi - len) }
}

/// Room kept free along each edge of a display (same units as its size).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Insets {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Insets {
    pub fn scaled(self, k: f32) -> Self {
        Self { top: self.top * k, right: self.right * k, bottom: self.bottom * k, left: self.left * k }
    }
}

/// Nine anchor points.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    #[default]
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

/// Where a new widget goes when the centre is taken: the corners, then the
/// edges' middles.
pub const FREE_ANCHORS: [Anchor; 8] = [Anchor::TopLeft, Anchor::TopRight, Anchor::BottomLeft, Anchor::BottomRight, Anchor::Left, Anchor::Right, Anchor::Top, Anchor::Bottom];
/// Inset from the screen edges for a widget placed on a free anchor
/// (fractions of the output; the renderer's clamp keeps it clear of the bar).
const FREE_INSET: f32 = 0.04;

impl Anchor {
    pub const ALL: [Anchor; 9] = [Self::TopLeft, Self::Top, Self::TopRight, Self::Left, Self::Center, Self::Right, Self::BottomLeft, Self::Bottom, Self::BottomRight];

    /// Offsets ([`Placement::x`] / `y`) that pull a widget on this anchor
    /// [`FREE_INSET`] in from the edges it touches.
    pub fn inset(self) -> (f32, f32) {
        let (fx, fy) = self.fraction();
        let toward = |f: f32| if f < 0.5 { FREE_INSET } else if f > 0.5 { -FREE_INSET } else { 0.0 };
        (toward(fx), toward(fy))
    }

    /// `(fx, fy)`, each 0, ½ or 1.
    pub fn fraction(self) -> (f32, f32) {
        let i = Self::ALL.iter().position(|a| *a == self).unwrap_or(4);
        ((i % 3) as f32 * 0.5, (i / 3) as f32 * 0.5)
    }
}

/// Above or below the wallpaper's subject cut-out.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Depth {
    /// Between the wallpaper and its subject (needs a depth mask and
    /// Wallpaper → Fill = Cover; otherwise drawn like `front`).
    #[default]
    Behind,
    Front,
}

/// A widget's kind and its own settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum WidgetKind {
    Clock(ClockWidget),
    Monitor(MonitorWidget),
    Decoration(Decoration),
    Visualizer(PlaneVisualizer),
    Weather(WeatherWidget),
    WorldClock(WorldClockWidget),
    Countdown(CountdownWidget),
    Progress(ProgressWidget),
    Todo(TodoWidget),
    Note(NoteWidget),
    Pomodoro(PomodoroWidget),
    Quote(QuoteWidget),
    Notebook(NotebookWidget),
    Date(DateWidget),
    Sky(SkyWidget),
    Profile(ProfileWidget),
    Shortcuts(ShortcutsWidget),
    Toggles(TogglesWidget),
    Sliders(SlidersWidget),
    Palette(PaletteWidget),
    Pet(PetWidget),
    Bubbles(BubblesWidget),
    Photo(PhotoWidget),
    Sticker(StickerWidget),
    Media(MediaWidget),
    Fetch(FetchWidget),
    ScreenTime(ScreenTimeWidget),
    Prayer(PrayerWidget),
    Jannah(JannahWidget),
    Hadith(HadithWidget),
    Quran(QuranWidget),
    Search(SearchWidget),
    Glance(GlanceWidget),
    Github(GithubWidget),
    Currency(CurrencyWidget),
    AiUsage(AiUsageWidget),
    Games(GamesWidget),
    /// An interactive card (media, calendar, a plugin's widget, …) on its
    /// own small layer surface, drawn by the shell's card host: `card`
    /// names it, `options` holds its settings.
    Card(CardWidget),
}

impl WidgetKind {
    /// Whether this kind draws forecast data when it is enabled and visible.
    pub fn uses_weather(&self) -> bool {
        match self {
            Self::Weather(_) => true,
            Self::Clock(clock) => clock.shows_weather(),
            _ => false,
        }
    }

    /// Stable kind id (`clock`, `monitor`, …), as in TOML.
    pub fn id(&self) -> &'static str {
        match self {
            WidgetKind::Clock(_) => "clock",
            WidgetKind::Monitor(_) => "monitor",
            WidgetKind::Decoration(_) => "decoration",
            WidgetKind::Visualizer(_) => "visualizer",
            WidgetKind::Weather(_) => "weather",
            WidgetKind::WorldClock(_) => "world-clock",
            WidgetKind::Countdown(_) => "countdown",
            WidgetKind::Progress(_) => "progress",
            WidgetKind::Todo(_) => "todo",
                        WidgetKind::Note(_) => "note",
                        WidgetKind::Pomodoro(_) => "pomodoro",
                        WidgetKind::Quote(_) => "quote",
                        WidgetKind::Notebook(_) => "notebook",
            WidgetKind::Date(_) => "date",
            WidgetKind::Sky(_) => "sky",
            WidgetKind::Profile(_) => "profile",
            WidgetKind::Shortcuts(_) => "shortcuts",
            WidgetKind::Toggles(_) => "toggles",
            WidgetKind::Sliders(_) => "sliders",
            WidgetKind::Palette(_) => "palette",
            WidgetKind::Pet(_) => "pet",
            WidgetKind::Bubbles(_) => "bubbles",
            WidgetKind::Photo(_) => "photo",
            WidgetKind::Sticker(_) => "sticker",
            WidgetKind::Media(_) => "media",
            WidgetKind::Fetch(_) => "fetch",
            WidgetKind::ScreenTime(_) => "screen-time",
            WidgetKind::Prayer(_) => "prayer",
            WidgetKind::Jannah(_) => "jannah",
            WidgetKind::Hadith(_) => "hadith",
            WidgetKind::Quran(_) => "quran",
            WidgetKind::Search(_) => "search",
            WidgetKind::Glance(_) => "glance",
            WidgetKind::Github(_) => "github",
            WidgetKind::Currency(_) => "currency",
            WidgetKind::AiUsage(_) => "ai-usage",
            WidgetKind::Games(_) => "games",
                        WidgetKind::Card(_) => "card",
        }
    }

    /// Painted by the software wallpaper renderer (cards are not).
    pub fn on_wallpaper_plane(&self) -> bool {
        !matches!(self, WidgetKind::Card(_))
    }

    /// The painted look, for every kind that has one.
    pub fn look(&self) -> Option<&Look> {
        match self {
            WidgetKind::Clock(c) => Some(&c.look),
            WidgetKind::Monitor(m) => Some(&m.look),
            WidgetKind::Decoration(d) => Some(&d.look),
            WidgetKind::Visualizer(v) => Some(&v.look),
            WidgetKind::Weather(w) => Some(&w.look),
            WidgetKind::WorldClock(w) => Some(&w.look),
            WidgetKind::Countdown(c) => Some(&c.look),
            WidgetKind::Progress(p) => Some(&p.look),
            WidgetKind::Todo(x) => Some(&x.look),
            WidgetKind::Note(x) => Some(&x.look),
            WidgetKind::Pomodoro(x) => Some(&x.look),
            WidgetKind::Quote(x) => Some(&x.look),
            WidgetKind::Notebook(x) => Some(&x.look),
            WidgetKind::Date(x) => Some(&x.look),
            WidgetKind::Sky(x) => Some(&x.look),
            WidgetKind::Profile(x) => Some(&x.look),
            WidgetKind::Shortcuts(x) => Some(&x.look),
            WidgetKind::Toggles(x) => Some(&x.look),
            WidgetKind::Sliders(x) => Some(&x.look),
            WidgetKind::Palette(x) => Some(&x.look),
            WidgetKind::Pet(x) => Some(&x.look),
            WidgetKind::Bubbles(x) => Some(&x.look),
            WidgetKind::Photo(x) => Some(&x.look),
            WidgetKind::Sticker(x) => Some(&x.look),
            WidgetKind::Media(x) => Some(&x.look),
            WidgetKind::Fetch(x) => Some(&x.look),
            WidgetKind::ScreenTime(x) => Some(&x.look),
            WidgetKind::Prayer(x) => Some(&x.look),
            WidgetKind::Jannah(x) => Some(&x.look),
            WidgetKind::Hadith(x) => Some(&x.look),
            WidgetKind::Quran(x) => Some(&x.look),
            WidgetKind::Search(x) => Some(&x.look),
            WidgetKind::Glance(x) => Some(&x.look),
            WidgetKind::Github(x) => Some(&x.look),
            WidgetKind::Currency(x) => Some(&x.look),
            WidgetKind::AiUsage(x) => Some(&x.look),
            WidgetKind::Games(x) => Some(&x.look),
            WidgetKind::Card(_) => None,
        }
    }

    pub fn look_mut(&mut self) -> Option<&mut Look> {
        match self {
            WidgetKind::Clock(c) => Some(&mut c.look),
            WidgetKind::Monitor(m) => Some(&mut m.look),
            WidgetKind::Decoration(d) => Some(&mut d.look),
            WidgetKind::Visualizer(v) => Some(&mut v.look),
            WidgetKind::Weather(w) => Some(&mut w.look),
            WidgetKind::WorldClock(w) => Some(&mut w.look),
            WidgetKind::Countdown(c) => Some(&mut c.look),
            WidgetKind::Progress(p) => Some(&mut p.look),
            WidgetKind::Todo(x) => Some(&mut x.look),
            WidgetKind::Note(x) => Some(&mut x.look),
            WidgetKind::Pomodoro(x) => Some(&mut x.look),
            WidgetKind::Quote(x) => Some(&mut x.look),
            WidgetKind::Notebook(x) => Some(&mut x.look),
            WidgetKind::Date(x) => Some(&mut x.look),
            WidgetKind::Sky(x) => Some(&mut x.look),
            WidgetKind::Profile(x) => Some(&mut x.look),
            WidgetKind::Shortcuts(x) => Some(&mut x.look),
            WidgetKind::Toggles(x) => Some(&mut x.look),
            WidgetKind::Sliders(x) => Some(&mut x.look),
            WidgetKind::Palette(x) => Some(&mut x.look),
            WidgetKind::Pet(x) => Some(&mut x.look),
            WidgetKind::Bubbles(x) => Some(&mut x.look),
            WidgetKind::Photo(x) => Some(&mut x.look),
            WidgetKind::Sticker(x) => Some(&mut x.look),
            WidgetKind::Media(x) => Some(&mut x.look),
            WidgetKind::Fetch(x) => Some(&mut x.look),
            WidgetKind::ScreenTime(x) => Some(&mut x.look),
            WidgetKind::Prayer(x) => Some(&mut x.look),
            WidgetKind::Jannah(x) => Some(&mut x.look),
            WidgetKind::Hadith(x) => Some(&mut x.look),
            WidgetKind::Quran(x) => Some(&mut x.look),
            WidgetKind::Search(x) => Some(&mut x.look),
            WidgetKind::Glance(x) => Some(&mut x.look),
            WidgetKind::Github(x) => Some(&mut x.look),
            WidgetKind::Currency(x) => Some(&mut x.look),
            WidgetKind::AiUsage(x) => Some(&mut x.look),
            WidgetKind::Games(x) => Some(&mut x.look),
            WidgetKind::Card(_) => None,
        }
    }

    /// Digits, calendar and language, for every kind that writes them.
    pub fn locale(&self) -> Option<&TextOptions> {
        match self {
            WidgetKind::Clock(c) => Some(&c.locale),
            WidgetKind::Monitor(m) => Some(&m.locale),
            WidgetKind::Weather(w) => Some(&w.locale),
            WidgetKind::WorldClock(w) => Some(&w.locale),
            WidgetKind::Countdown(c) => Some(&c.locale),
            WidgetKind::Progress(p) => Some(&p.locale),
            WidgetKind::Todo(x) => Some(&x.locale),
            WidgetKind::Note(x) => Some(&x.locale),
            WidgetKind::Pomodoro(x) => Some(&x.locale),
            WidgetKind::Quote(x) => Some(&x.locale),
            WidgetKind::Notebook(x) => Some(&x.locale),
            WidgetKind::Date(x) => Some(&x.locale),
            WidgetKind::Sky(x) => Some(&x.locale),
            WidgetKind::Profile(x) => Some(&x.locale),
            WidgetKind::Shortcuts(x) => Some(&x.locale),
            WidgetKind::Toggles(x) => Some(&x.locale),
            WidgetKind::Sliders(x) => Some(&x.locale),
            WidgetKind::Palette(x) => Some(&x.locale),
            WidgetKind::Pet(x) => Some(&x.locale),
            WidgetKind::Media(x) => Some(&x.locale),
            WidgetKind::Fetch(x) => Some(&x.locale),
            WidgetKind::ScreenTime(x) => Some(&x.locale),
            WidgetKind::Prayer(x) => Some(&x.locale),
            WidgetKind::Jannah(x) => Some(&x.locale),
            WidgetKind::Hadith(x) => Some(&x.locale),
            WidgetKind::Quran(x) => Some(&x.locale),
            WidgetKind::Search(x) => Some(&x.locale),
            WidgetKind::Glance(x) => Some(&x.locale),
            WidgetKind::Github(x) => Some(&x.locale),
            WidgetKind::Currency(x) => Some(&x.locale),
            WidgetKind::AiUsage(x) => Some(&x.locale),
            WidgetKind::Games(x) => Some(&x.locale),
            WidgetKind::Decoration(_) | WidgetKind::Visualizer(_) | WidgetKind::Bubbles(_) | WidgetKind::Photo(_) | WidgetKind::Sticker(_) | WidgetKind::Card(_) => None,
        }
    }

    pub fn locale_mut(&mut self) -> Option<&mut TextOptions> {
        match self {
            WidgetKind::Clock(c) => Some(&mut c.locale),
            WidgetKind::Monitor(m) => Some(&mut m.locale),
            WidgetKind::Weather(w) => Some(&mut w.locale),
            WidgetKind::WorldClock(w) => Some(&mut w.locale),
            WidgetKind::Countdown(c) => Some(&mut c.locale),
            WidgetKind::Progress(p) => Some(&mut p.locale),
            WidgetKind::Todo(x) => Some(&mut x.locale),
            WidgetKind::Note(x) => Some(&mut x.locale),
            WidgetKind::Pomodoro(x) => Some(&mut x.locale),
            WidgetKind::Quote(x) => Some(&mut x.locale),
            WidgetKind::Notebook(x) => Some(&mut x.locale),
            WidgetKind::Date(x) => Some(&mut x.locale),
            WidgetKind::Sky(x) => Some(&mut x.locale),
            WidgetKind::Profile(x) => Some(&mut x.locale),
            WidgetKind::Shortcuts(x) => Some(&mut x.locale),
            WidgetKind::Toggles(x) => Some(&mut x.locale),
            WidgetKind::Sliders(x) => Some(&mut x.locale),
            WidgetKind::Palette(x) => Some(&mut x.locale),
            WidgetKind::Pet(x) => Some(&mut x.locale),
            WidgetKind::Media(x) => Some(&mut x.locale),
            WidgetKind::Fetch(x) => Some(&mut x.locale),
            WidgetKind::ScreenTime(x) => Some(&mut x.locale),
            WidgetKind::Prayer(x) => Some(&mut x.locale),
            WidgetKind::Jannah(x) => Some(&mut x.locale),
            WidgetKind::Hadith(x) => Some(&mut x.locale),
            WidgetKind::Quran(x) => Some(&mut x.locale),
            WidgetKind::Search(x) => Some(&mut x.locale),
            WidgetKind::Glance(x) => Some(&mut x.locale),
            WidgetKind::Github(x) => Some(&mut x.locale),
            WidgetKind::Currency(x) => Some(&mut x.locale),
            WidgetKind::AiUsage(x) => Some(&mut x.locale),
            WidgetKind::Games(x) => Some(&mut x.locale),
            WidgetKind::Decoration(_) | WidgetKind::Visualizer(_) | WidgetKind::Bubbles(_) | WidgetKind::Photo(_) | WidgetKind::Sticker(_) | WidgetKind::Card(_) => None,
        }
    }

    /// The first-party plugin that provides this kind (see
    /// [`crate::desktop_cards::card_plugin`] for cards).
    pub fn plugin(&self) -> &str {
        match self {
            WidgetKind::Clock(_) | WidgetKind::Progress(_) => plugin_ids::CLOCKS,
            WidgetKind::Monitor(_) => plugin_ids::MONITOR,
            WidgetKind::Decoration(_) => plugin_ids::DECORATIONS,
            WidgetKind::Visualizer(_) => plugin_ids::VISUALIZERS,
            WidgetKind::Weather(_) => plugin_ids::WEATHER,
            WidgetKind::WorldClock(_) => plugin_ids::WORLD_CLOCK,
            WidgetKind::Countdown(_) => plugin_ids::COUNTDOWN,
            WidgetKind::Todo(_) | WidgetKind::Note(_) | WidgetKind::Pomodoro(_) | WidgetKind::Quote(_) | WidgetKind::Notebook(_) => plugin_ids::PRODUCTIVITY,
            WidgetKind::Date(_) => plugin_ids::CALENDAR,
            WidgetKind::Sky(_) => plugin_ids::SKY,
            WidgetKind::Profile(_) | WidgetKind::Shortcuts(_) | WidgetKind::Toggles(_) | WidgetKind::Sliders(_) | WidgetKind::Palette(_) => plugin_ids::IDENTITY,
            WidgetKind::Pet(_) | WidgetKind::Bubbles(_) | WidgetKind::Photo(_) | WidgetKind::Sticker(_) => plugin_ids::FUN,
            WidgetKind::Media(_) => plugin_ids::NOW_PLAYING,
            WidgetKind::Fetch(_) | WidgetKind::ScreenTime(_) | WidgetKind::Prayer(_) | WidgetKind::Search(_) | WidgetKind::Glance(_) | WidgetKind::Github(_) | WidgetKind::Currency(_) | WidgetKind::AiUsage(_) | WidgetKind::Games(_) => plugin_ids::INFO,
            WidgetKind::Jannah(_) => plugin_ids::JANNAH,
            WidgetKind::Hadith(_) => plugin_ids::HADITH,
            WidgetKind::Quran(_) => plugin_ids::QURAN,
            WidgetKind::Card(c) => crate::desktop_cards::card_plugin(&c.card),
        }
    }
}

/// Ids of the first-party plugins that provide the desktop widgets. A
/// widget whose plugin is switched off isn't drawn (it stays in the list).
pub mod plugin_ids {
    pub const CLOCKS: &str = "desktop-clocks";
    pub const MONITOR: &str = "desktop-monitor";
    pub const DECORATIONS: &str = "desktop-decorations";
    pub const VISUALIZERS: &str = "desktop-visualizers";
    pub const NOW_PLAYING: &str = "desktop-now-playing";
    pub const CALENDAR: &str = "desktop-calendar";
    pub const WEATHER: &str = "desktop-weather";
    pub const WORLD_CLOCK: &str = "desktop-world-clock";
    pub const COUNTDOWN: &str = "desktop-countdown";
    pub const PRODUCTIVITY: &str = "desktop-productivity";
    pub const SKY: &str = "desktop-sky";
    pub const IDENTITY: &str = "desktop-identity";
    pub const FUN: &str = "desktop-fun";
    pub const INFO: &str = "desktop-info";
    /// Not a desktop widget plugin: the Jannah plugin offers its own
    /// widgets (so they are not in [`ALL`]).
    pub const JANNAH: &str = "jannah";
    /// Not a desktop widget plugin either: the Hadith plugin's widgets.
    pub const HADITH: &str = "hadith";
    /// The Qur'an plugin's own widgets (not in [`ALL`] either).
    pub const QURAN: &str = "quran";
    /// Every one of them.
    pub const ALL: [&str; 14] = [CLOCKS, MONITOR, DECORATIONS, VISUALIZERS, NOW_PLAYING, CALENDAR, WEATHER, WORLD_CLOCK, COUNTDOWN, PRODUCTIVITY, SKY, IDENTITY, FUN, INFO];
}

/// The current weather, painted (needs the weather plugin and a location).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WeatherWidget {
    pub layout: WeatherLayout,
    /// The place name.
    pub location: bool,
    /// Feels like, humidity and wind.
    pub details: bool,
    /// Days of forecast under it (0 = none).
    pub days: u32,
    /// Sentence layout: your own sentence per condition (clear, partly
    /// cloudy, cloudy, fog, drizzle, rain, snow, thunder; empty = the
    /// built-in one).
    pub sentences: Vec<String>,
    /// Disc layout: its diameter and a dotted ring round it.
    pub disc: f32,
    pub dotted_ring: bool,
    /// Hourly layout: hours shown and the area under the curve.
    pub hours: u32,
    pub fill: bool,
    pub look: Look,
    pub locale: TextOptions,
}

/// Weather widget layouts.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WeatherLayout {
    /// A big temperature beside the condition's icon, the condition and
    /// place under it.
    #[default]
    Hero,
    /// One line: icon, temperature, condition.
    Compact,
    /// The condition and a friendly sentence about it.
    Sentence,
    /// A round disc: the icon, a big temperature and the day's range.
    Disc,
    /// The next hours as a temperature curve with icons.
    Hourly,
    /// A compact forecast printed across a horizontal ticket.
    Ribbon,
    /// A thermometer filled from the current temperature and today's range.
    Thermometer,
    /// A petal dial showing measured relative humidity.
    HumidityRose,
}

impl Default for WeatherWidget {
    fn default() -> Self {
        Self {
            layout: WeatherLayout::Hero,
            location: true,
            details: true,
            days: ranges::WEATHER_DAYS_DEFAULT,
            sentences: Vec::new(),
            disc: crate::widget_kinds::ranges::DISC_DEFAULT_PX,
            dotted_ring: true,
            hours: crate::widget_kinds::ranges::HOURS_DEFAULT,
            fill: true,
            look: Look::default(),
            locale: TextOptions::default(),
        }
    }
}

impl WeatherWidget {
    pub fn days(&self) -> u32 {
        self.days.clamp(ranges::WEATHER_DAYS.0, ranges::WEATHER_DAYS.1)
    }
}

/// Times in other cities.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldClockWidget {
    /// Cities or IANA time zones (`Tokyo`, `America/New_York`); empty =
    /// the World clock plugin's cities.
    pub cities: Vec<String>,
    /// Hours ahead or behind here, and "tomorrow" / "yesterday".
    pub offsets: bool,
    /// Side by side instead of one under another.
    pub horizontal: bool,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for WorldClockWidget {
    fn default() -> Self {
        Self { cities: Vec::new(), offsets: true, horizontal: false, look: Look::default(), locale: TextOptions::default() }
    }
}

/// Days until (or since) a date.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CountdownWidget {
    /// What is coming (empty = "New Year" when counting to it).
    pub title: String,
    /// `YYYY-MM-DD` (empty = the next New Year's Day).
    pub date: String,
    /// The target date under the count.
    pub show_date: bool,
    /// After the date: days since it (otherwise the widget shows "today"
    /// on the day and nothing after).
    pub count_up: bool,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for CountdownWidget {
    fn default() -> Self {
        Self { title: String::new(), date: String::new(), show_date: true, count_up: true, look: Look::default(), locale: TextOptions::default() }
    }
}

/// An interactive card's settings (owned by the card host).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CardWidget {
    pub card: String,
    pub options: BTreeMap<String, toml::Value>,
}

// ── Look shared by the painted kinds ─────────────────────────────────────

/// A colour role.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tone {
    /// Light or dark, whichever reads best over the wallpaper behind it.
    #[default]
    Auto,
    /// The theme's text colour.
    OnSurface,
    /// The theme's accent.
    Accent,
    /// `custom_color` (`#rrggbb` or `oklch(L C H)`).
    Custom,
}

/// Font role from Typography.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FontRole {
    /// The layout's font roles; ordinary text follows Typography → Desktop widgets.
    #[default]
    Widgets,
    /// Display face (Alexandria by default).
    Display,
    /// Interface face (Inter).
    Ui,
    /// Arabic face (Cairo).
    Arabic,
    /// Monospace face (JetBrains Mono).
    Mono,
    /// A Qur'anic Naskh for scripture (Amiri Quran, then Amiri, Noto Naskh
    /// Arabic or the Arabic face).
    Scripture,
    /// A Naskh for hadith (Amiri, then Amiri Quran, Noto Naskh Arabic or
    /// the Arabic face).
    Hadith,
    /// The widget's own family ([`Look::font_family`]).
    Custom,
}

/// Which digits numbers are written with.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Numerals {
    /// The widget's language decides: Eastern Arabic in Arabic, else
    /// Western.
    #[default]
    Auto,
    /// 0123456789
    Western,
    /// ٠١٢٣٤٥٦٧٨٩
    EasternArabic,
}

impl Numerals {
    pub const ALL: [Numerals; 3] = [Numerals::Auto, Numerals::Western, Numerals::EasternArabic];

    /// Whether Eastern Arabic digits are used for text in `lang`.
    pub fn eastern(self, arabic_language: bool) -> bool {
        match self {
            Numerals::Auto => arabic_language,
            Numerals::Western => false,
            Numerals::EasternArabic => true,
        }
    }
}

/// Which calendar a widget's date is in.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Calendar {
    #[default]
    Gregorian,
    /// The Islamic (Hijri) date; the day begins at sunset (Maghrib).
    Hijri,
    /// Gregorian · Hijri.
    Both,
    /// No date (where the widget can go without one).
    None,
}

impl Calendar {
    pub const ALL: [Calendar; 4] = [Calendar::Gregorian, Calendar::Hijri, Calendar::Both, Calendar::None];

    pub fn gregorian(self) -> bool {
        matches!(self, Calendar::Gregorian | Calendar::Both)
    }

    pub fn hijri(self) -> bool {
        matches!(self, Calendar::Hijri | Calendar::Both)
    }
}

/// How lines of a widget line up.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Align {
    /// With the side of the screen the widget is anchored to.
    #[default]
    Auto,
    /// Where reading starts (left, or right in right-to-left text).
    Start,
    Center,
    End,
}

impl Align {
    pub const ALL: [Align; 4] = [Align::Auto, Align::Start, Align::Center, Align::End];

    /// 0 (left) … 1 (right) for a widget anchored at `anchor_x` (0, ½, 1)
    /// whose text runs right to left when `rtl`.
    pub fn fraction(self, anchor_x: f32, rtl: bool) -> f32 {
        match (self, rtl) {
            (Align::Auto, _) => anchor_x,
            (Align::Center, _) => HALF,
            (Align::Start, false) | (Align::End, true) => 0.0,
            (Align::Start, true) | (Align::End, false) => 1.0,
        }
    }
}

/// A plate behind the widget, so it reads over any wallpaper.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Plate {
    #[default]
    None,
    /// Translucent theme surface with a 1 px specular rim.
    Glass,
    /// The theme surface, opaque.
    Solid,
}

/// Soft shadow or glow behind text and shapes.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Effect {
    #[default]
    None,
    /// A soft dark (or light, over bright wallpapers) shadow.
    Shadow,
    /// A glow in the ink's own colour.
    Glow,
}

/// Colours and effect of a painted widget.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    pub tone: Tone,
    /// Secondary details (weekday, seconds, graphs).
    pub accent: Tone,
    pub custom_color: String,
    pub custom_accent: String,
    pub font: FontRole,
    /// The family when `font = "custom"` (any name; one that isn't
    /// installed falls back like the Typography families).
    pub font_family: String,
    pub effect: Effect,
    /// Shadow / glow strength, 1 = as designed.
    pub effect_strength: f32,
    /// How the lines line up.
    pub align: Align,
    pub plate: Plate,
    pub plate_opacity: f32,
    /// Corner radius and padding of the plate, reference px.
    pub plate_radius: f32,
    pub plate_padding: f32,
    /// The plate's outline shape, and its parameters: lobes (cookie,
    /// flower), lobe or tear depth, lean (skew) and corner cut (chamfer).
    pub plate_shape: PlateShape,
    pub plate_lobes: u32,
    pub plate_depth: f32,
    pub plate_skew: f32,
    pub plate_chamfer: f32,
    /// A line around the plate's shape (drawn even without a plate),
    /// reference px (0 = none), and its colour.
    pub outline_width: f32,
    pub outline_tone: Tone,
    pub outline_color: String,
    /// A shadow of the whole widget: soft or hard, its offset, blur,
    /// opacity and colour.
    pub shadow: ShadowKind,
    pub shadow_x: f32,
    pub shadow_y: f32,
    pub shadow_blur: f32,
    pub shadow_opacity: f32,
    pub shadow_tone: Tone,
    pub shadow_color: String,
    /// A header line above the content (empty = none), its size, and a
    /// rule under it.
    pub header: String,
    pub header_size: f32,
    pub header_rule: bool,
    /// A frame around the widget, its colour (tape) and the tape's angle.
    pub chrome: Chrome,
    pub chrome_tone: Tone,
    pub chrome_color: String,
    pub tape_angle: f32,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            tone: Tone::Auto,
            accent: Tone::Accent,
            custom_color: String::new(),
            custom_accent: String::new(),
            font: FontRole::Widgets,
            font_family: String::new(),
            effect: Effect::None,
            effect_strength: ranges::EFFECT_STRENGTH_DEFAULT,
            align: Align::Auto,
            plate: Plate::None,
            plate_opacity: ranges::PLATE_OPACITY_DEFAULT,
            plate_radius: ranges::PLATE_RADIUS_DEFAULT_PX,
            plate_padding: ranges::PLATE_PADDING_DEFAULT_PX,
            plate_shape: PlateShape::Rect,
            plate_lobes: style::LOBES_DEFAULT,
            plate_depth: style::DEPTH_DEFAULT,
            plate_skew: style::SKEW_DEFAULT_DEG,
            plate_chamfer: style::CHAMFER_DEFAULT_PX,
            outline_width: 0.0,
            outline_tone: Tone::Auto,
            outline_color: String::new(),
            shadow: ShadowKind::None,
            shadow_x: style::SHADOW_X_DEFAULT_PX,
            shadow_y: style::SHADOW_Y_DEFAULT_PX,
            shadow_blur: style::SHADOW_BLUR_DEFAULT_PX,
            shadow_opacity: style::SHADOW_OPACITY_DEFAULT,
            shadow_tone: Tone::Auto,
            shadow_color: String::new(),
            header: String::new(),
            header_size: style::HEADER_SIZE_DEFAULT_PX,
            header_rule: true,
            chrome: Chrome::None,
            chrome_tone: Tone::Accent,
            chrome_color: String::new(),
            tape_angle: style::TAPE_ANGLE_DEFAULT_DEG,
        }
    }
}

impl Look {
    pub fn effect_strength(&self) -> f32 {
        self.effect_strength.clamp(ranges::EFFECT_STRENGTH.0, ranges::EFFECT_STRENGTH.1)
    }

    pub fn plate_opacity(&self) -> f32 {
        self.plate_opacity.clamp(ranges::PLATE_OPACITY.0, ranges::PLATE_OPACITY.1)
    }

    pub fn plate_radius(&self) -> f32 {
        self.plate_radius.clamp(ranges::PLATE_RADIUS_PX.0, ranges::PLATE_RADIUS_PX.1)
    }

    pub fn plate_padding(&self) -> f32 {
        self.plate_padding.clamp(ranges::PLATE_PADDING_PX.0, ranges::PLATE_PADDING_PX.1)
    }

    pub fn outline_width(&self) -> f32 {
        style::OUTLINE_PX.clamp(self.outline_width, 0.0)
    }

    pub fn shadow_offset(&self) -> (f32, f32) {
        (style::SHADOW_OFFSET_PX.clamp(self.shadow_x, style::SHADOW_X_DEFAULT_PX), style::SHADOW_OFFSET_PX.clamp(self.shadow_y, style::SHADOW_Y_DEFAULT_PX))
    }

    pub fn shadow_blur(&self) -> f32 {
        style::SHADOW_BLUR_PX.clamp(self.shadow_blur, style::SHADOW_BLUR_DEFAULT_PX)
    }

    pub fn shadow_opacity(&self) -> f32 {
        style::SHADOW_OPACITY.clamp(self.shadow_opacity, style::SHADOW_OPACITY_DEFAULT)
    }

    pub fn header_size(&self) -> f32 {
        style::HEADER_SIZE_PX.clamp(self.header_size, style::HEADER_SIZE_DEFAULT_PX)
    }

    pub fn plate_depth(&self) -> f32 {
        style::DEPTH.clamp(self.plate_depth, style::DEPTH_DEFAULT)
    }

    pub fn plate_skew(&self) -> f32 {
        style::SKEW_DEG.clamp(self.plate_skew, style::SKEW_DEFAULT_DEG)
    }

    pub fn plate_chamfer(&self) -> f32 {
        style::CHAMFER_PX.clamp(self.plate_chamfer, style::CHAMFER_DEFAULT_PX)
    }

    pub fn tape_angle(&self) -> f32 {
        style::TAPE_ANGLE_DEG.clamp(self.tape_angle, style::TAPE_ANGLE_DEFAULT_DEG)
    }

    /// Something is drawn around or behind the content: a plate, an
    /// outline or a chrome (the automatic ink then reads against the
    /// plate).
    pub fn framed(&self) -> bool {
        self.plate != Plate::None || self.chrome != Chrome::None
    }
}

/// Language-dependent text of a widget: digits, calendar and the language
/// names and words are written in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextOptions {
    pub numerals: Numerals,
    pub calendar: Calendar,
    /// A language code (`en`, `ar`, …) for day and month names and words;
    /// empty = the interface language (or Studio's date language).
    pub language: String,
    /// The Hijri date in Arabic (its month names and digits), whatever the
    /// widget's language.
    pub hijri_in_arabic: bool,
}

impl Default for TextOptions {
    fn default() -> Self {
        Self { numerals: Numerals::Auto, calendar: Calendar::Gregorian, language: String::new(), hijri_in_arabic: true }
    }
}

// ── Clock ────────────────────────────────────────────────────────────────

/// Clock layouts. The first three are the depth clock that predates
/// desktop widgets (with its glow, date pill and audio ring); one of them
/// is drawn at a time, the first enabled.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClockFace {
    /// Depth clock: hours over minutes, large and bold.
    #[default]
    Stacked,
    /// Depth clock: one light `HH:MM` line.
    Classic,
    /// Depth clock: Eastern Arabic numerals in the Arabic face.
    Arabic,
    /// Giant hours over minutes with small seconds (or AM/PM) beside them.
    Superscript,
    /// One line: `12:35 AM — 09/04`.
    Headline,
    /// Weather and temperature, a slanted rule, the weekday in heavy type,
    /// the time in a slanted solid box and a small date line.
    DayBlock,
    /// Circular progress (minute of the hour or of the day) with the time
    /// inside and `Weekday · Month Day` below.
    Ring,
    /// A vertical column between thin rules: greeting, weekday, date, time.
    Greeting,
    /// A giant day numeral with the month's name across it.
    DayNumeral,
    /// `23:23` over `Sunday · August 23`, the weekday in the accent.
    Minimal,
    /// A dial with hands: hour ticks, optional numbers and a seconds hand,
    /// the date under it.
    Analog,
    /// Hours over minutes in huge numerals, the date under them (a card
    /// clock on a plate).
    Stack,
    /// The time centred in one big expressive shape.
    Shape,
    /// The time in lit dots of a dot-matrix display.
    DotMatrix,
    /// The time in square pixels, with a small date line.
    Pixel,
    /// Each digit in its own box, AM / PM chips and a week strip.
    Boxes,
    /// Heavy slanted time over a hard shadow, the part of the day, the
    /// weekday and the moon beside it.
    Hud,
    /// Lines you write yourself from the time and date (`{HH}:{mm}`,
    /// `{weekday}`, …), each with its own size.
    Template,
    /// Analog hands over the hour and minute as giant numerals.
    Numerals,
    /// Hour and minute as bodies orbiting on two rings.
    Orbit,
    /// Time written in words, with the exact time below.
    Word,
    /// Four mechanical split-flap digit cards.
    SplitFlap,
    /// Numerals inside outlined glass tubes.
    Nixie,
    /// Binary-coded decimal columns with their decimal readout.
    Binary,
}

impl ClockFace {
    pub const ALL: [ClockFace; 24] = [
        Self::Stacked,
        Self::Classic,
        Self::Arabic,
        Self::Superscript,
        Self::Headline,
        Self::DayBlock,
        Self::Ring,
        Self::Greeting,
        Self::DayNumeral,
        Self::Minimal,
        Self::Analog,
        Self::Stack,
        Self::Shape,
        Self::DotMatrix,
        Self::Pixel,
        Self::Boxes,
        Self::Hud,
        Self::Template,
        Self::Numerals,
        Self::Orbit,
        Self::Word,
        Self::SplitFlap,
        Self::Nixie,
        Self::Binary,
    ];

    /// The depth clock's own faces (drawn by its dedicated planes).
    pub fn depth_style(self) -> Option<ClockStyle> {
        match self {
            ClockFace::Stacked => Some(ClockStyle::Stacked),
            ClockFace::Classic => Some(ClockStyle::Classic),
            ClockFace::Arabic => Some(ClockStyle::Arabic),
            _ => None,
        }
    }

    pub fn from_depth_style(s: ClockStyle) -> Self {
        match s {
            ClockStyle::Stacked => ClockFace::Stacked,
            ClockStyle::Classic => ClockFace::Classic,
            ClockStyle::Arabic => ClockFace::Arabic,
        }
    }

    /// Stable id, as in TOML.
    pub fn id(self) -> &'static str {
        match self {
            ClockFace::Stacked => "stacked",
            ClockFace::Classic => "classic",
            ClockFace::Arabic => "arabic",
            ClockFace::Superscript => "superscript",
            ClockFace::Headline => "headline",
            ClockFace::DayBlock => "day-block",
            ClockFace::Ring => "ring",
            ClockFace::Greeting => "greeting",
            ClockFace::DayNumeral => "day-numeral",
            ClockFace::Minimal => "minimal",
            ClockFace::Analog => "analog",
            ClockFace::Stack => "stack",
            ClockFace::Shape => "shape",
            ClockFace::DotMatrix => "dot-matrix",
            ClockFace::Pixel => "pixel",
            ClockFace::Boxes => "boxes",
            ClockFace::Hud => "hud",
            ClockFace::Template => "template",
            ClockFace::Numerals => "numerals",
            ClockFace::Orbit => "orbit",
            ClockFace::Word => "word",
            ClockFace::SplitFlap => "split-flap",
            ClockFace::Nixie => "nixie",
            ClockFace::Binary => "binary",
        }
    }
}

/// What the ring face's arc measures.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RingProgress {
    /// Minutes of the current hour.
    #[default]
    Hour,
    /// Time of the day.
    Day,
}

/// A clock widget.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClockWidget {
    pub face: ClockFace,
    pub look: Look,
    /// Seconds (faces that show them; only that small plane redraws each
    /// second, and only while the desktop is visible).
    pub seconds: bool,
    /// Before [`WIDGETS_VERSION`] 1: Eastern Arabic numerals (read once by
    /// the upgrade into [`TextOptions::numerals`], never written).
    #[serde(rename = "eastern_numerals", skip_serializing)]
    pub legacy_eastern: bool,
    /// The date line (and, for the depth faces, the date pill).
    pub date: bool,
    /// Current weather where the face has room for it (needs the weather
    /// plugin and a location).
    pub weather: bool,
    /// Depth faces: the audio ring around the clock.
    pub audio_ring: bool,
    pub ring_progress: RingProgress,
    /// Analog face: the hour numbers around the dial.
    pub dial_numbers: bool,
    /// Digits, calendar and language.
    pub locale: TextOptions,
    /// Analog and numerals faces: the dial and the hands.
    pub analog: AnalogOptions,
    /// The faces' own shapes and sizes (shape, dots, boxes, slant, …).
    pub face_options: FaceOptions,
    /// Template face: its lines.
    pub lines: Vec<TemplateLine>,
    /// Greeting face: your own greeting for the morning, afternoon,
    /// evening and night (empty = the built-in one) and a line under it.
    pub phrases: Vec<String>,
    pub subtitle: String,
}

impl Default for ClockWidget {
    fn default() -> Self {
        Self {
            face: ClockFace::Minimal,
            look: Look::default(),
            seconds: false,
            legacy_eastern: false,
            date: true,
            weather: true,
            audio_ring: false,
            ring_progress: RingProgress::Hour,
            dial_numbers: true,
            locale: TextOptions::default(),
            analog: AnalogOptions::default(),
            face_options: FaceOptions::default(),
            lines: TemplateLine::defaults(),
            phrases: Vec::new(),
            subtitle: String::new(),
        }
    }
}

impl ClockWidget {
    /// Only the day-block face displays the optional weather caption.
    pub fn shows_weather(&self) -> bool {
        self.weather && self.face == ClockFace::DayBlock
    }

    /// A new clock with `face`: the depth faces keep their Gregorian ·
    /// Hijri date pill.
    pub fn with_face(face: ClockFace) -> Self {
        let mut c = Self { face, ..Self::default() };
        if face.depth_style().is_some() {
            c.locale.calendar = Calendar::Both;
        }
        c
    }

    /// Whether the clock writes Eastern Arabic digits when the widget's
    /// language is Arabic or not (`arabic_language`). The Arabic depth face
    /// always did unless told otherwise.
    pub fn eastern(&self, arabic_language: bool) -> bool {
        match (self.locale.numerals, self.face) {
            (Numerals::Auto, ClockFace::Arabic) => true,
            (n, _) => n.eastern(arabic_language),
        }
    }
}

/// Analog dial marks.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DialMarks {
    None,
    /// A dot at every hour.
    Dots,
    /// Hour ticks (numbers at the quarters with "hour numbers" on).
    #[default]
    Ticks,
    /// The twelve hour numbers.
    Numbers,
}

/// Analog hand shapes.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HandStyle {
    /// A rounded line.
    #[default]
    Classic,
    /// A wide filled blade.
    Fill,
    /// A blade drawn as an outline.
    Hollow,
}

/// How the analog clock shows the seconds.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecondsStyle {
    #[default]
    Hand,
    /// A dot running round the rim.
    Dot,
}

/// How the analog clock writes the date.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DateStyle {
    /// A line under the dial.
    #[default]
    Under,
    /// The day in a round bubble on the dial.
    Bubble,
    /// The day in a small box on the dial.
    Box,
}

/// The analog and numerals faces' dial and hands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalogOptions {
    /// The dial's outline (a scalloped cookie, a flower, …).
    pub dial: PlateShape,
    /// The dial filled with the plate's colour (else only its rim).
    pub dial_fill: bool,
    pub marks: DialMarks,
    pub hour_hand: HandStyle,
    /// Width of the minute hand (and the hour hand with it), reference px.
    pub hand_width: f32,
    pub seconds_style: SecondsStyle,
    pub date_style: DateStyle,
    /// Turns the dial's shape, degrees.
    pub dial_turn: f32,
}

impl Default for AnalogOptions {
    fn default() -> Self {
        Self {
            dial: PlateShape::Circle,
            dial_fill: false,
            marks: DialMarks::Ticks,
            hour_hand: HandStyle::Classic,
            hand_width: ranges::HAND_WIDTH_DEFAULT_PX,
            seconds_style: SecondsStyle::Hand,
            date_style: DateStyle::Under,
            dial_turn: 0.0,
        }
    }
}

impl AnalogOptions {
    pub fn hand_width(&self) -> f32 {
        self.hand_width.clamp(ranges::HAND_WIDTH_PX.0, ranges::HAND_WIDTH_PX.1)
    }
}

/// The other faces' own shapes and sizes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FaceOptions {
    /// Shape face: the shape the time sits in, and whether it turns a
    /// little every minute.
    pub shape: PlateShape,
    pub spin: bool,
    /// Dot-matrix face: square dots, and how bright the unlit ones are.
    pub square_dots: bool,
    pub unlit: f32,
    /// Stack face: the gap between hours and minutes, fraction of the
    /// digits' height.
    pub line_gap: f32,
    /// Boxes face: each box's shape, and the week strip under them.
    pub box_shape: PlateShape,
    pub week_strip: bool,
    /// HUD face: the slant (degrees), and the moon beside the time.
    pub slant: f32,
    pub moon: bool,
    /// Numerals face: how strong the giant numerals are.
    pub numerals: f32,
    /// Orbit face: the hour numbers around the rings.
    pub labels: bool,
    /// Template face: the first line beside the others, a rule between.
    pub divider: bool,
}

impl Default for FaceOptions {
    fn default() -> Self {
        Self {
            shape: PlateShape::Cookie,
            spin: false,
            square_dots: false,
            unlit: ranges::UNLIT_DEFAULT,
            line_gap: ranges::LINE_GAP_DEFAULT,
            box_shape: PlateShape::Rect,
            week_strip: true,
            slant: ranges::SLANT_DEFAULT_DEG,
            moon: true,
            numerals: ranges::NUMERALS_DEFAULT,
            labels: true,
            divider: false,
        }
    }
}

impl FaceOptions {
    pub fn unlit(&self) -> f32 {
        self.unlit.clamp(ranges::UNLIT.0, ranges::UNLIT.1)
    }
    pub fn line_gap(&self) -> f32 {
        self.line_gap.clamp(ranges::LINE_GAP.0, ranges::LINE_GAP.1)
    }
    pub fn slant(&self) -> f32 {
        self.slant.clamp(ranges::SLANT_DEG.0, ranges::SLANT_DEG.1)
    }
    pub fn numerals(&self) -> f32 {
        self.numerals.clamp(ranges::NUMERALS.0, ranges::NUMERALS.1)
    }
}

/// One line of the template face.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TemplateLine {
    /// Text with placeholders: `{HH}` `{H}` `{hh}` `{h}` `{mm}` `{ss}`
    /// `{ampm}` `{time}` `{weekday}` `{wd}` `{day}` `{dd}` `{month}` `{mon}`
    /// `{MM}` `{year}` `{greeting}` `{daypart}` `{hijri}`.
    pub text: String,
    /// Size, reference px.
    pub size: f32,
    pub weight: u16,
    pub opacity: f32,
    /// In the accent colour.
    pub accent: bool,
    /// Upper case.
    pub caps: bool,
}

impl Default for TemplateLine {
    fn default() -> Self {
        Self { text: String::new(), size: ranges::TEMPLATE_SIZE_DEFAULT_PX, weight: ranges::WEIGHT_DEFAULT, opacity: 1.0, accent: false, caps: false }
    }
}

impl TemplateLine {
    /// The lines a new template clock starts with.
    pub fn defaults() -> Vec<TemplateLine> {
        vec![
            TemplateLine { text: "{weekday}".into(), size: 30.0, weight: 300, opacity: 0.8, caps: true, ..Default::default() },
            TemplateLine { text: "{day} {month}".into(), size: 22.0, weight: 500, accent: true, ..Default::default() },
            TemplateLine { text: "{time}".into(), size: 120.0, weight: 200, ..Default::default() },
        ]
    }

    pub fn size(&self) -> f32 {
        self.size.clamp(ranges::TEXT_SIZE_PX.0, ranges::TEXT_SIZE_PX.1)
    }
    pub fn weight(&self) -> u16 {
        self.weight.clamp(ranges::WEIGHT.0, ranges::WEIGHT.1)
    }
    pub fn opacity(&self) -> f32 {
        self.opacity.clamp(ranges::OPACITY.0, ranges::OPACITY.1)
    }
}

// ── Progress ─────────────────────────────────────────────────────────────

/// The period a progress widget measures.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Period {
    Hour,
    #[default]
    Day,
    Week,
    Month,
    Year,
}

/// How a progress widget draws.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProgressStyle {
    #[default]
    Ring,
    /// A ring with a gentle wave.
    WavyRing,
    Bar,
    /// One dot per day (hour, for the day) in a grid, the past filled.
    Dots,
}

/// How much of today (this week, month or year) has passed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProgressWidget {
    pub period: Period,
    pub style: ProgressStyle,
    /// `62 % of today · 9 h 2 min left` under it.
    pub label: bool,
    /// Line width (rings, bars), reference px.
    pub thickness: f32,
    pub look: Look,
    pub locale: TextOptions,
}

impl Default for ProgressWidget {
    fn default() -> Self {
        Self { period: Period::Day, style: ProgressStyle::Ring, label: true, thickness: ranges::PROGRESS_THICKNESS_DEFAULT_PX, look: Look::default(), locale: TextOptions::default() }
    }
}

impl ProgressWidget {
    pub fn thickness(&self) -> f32 {
        self.thickness.clamp(ranges::PROGRESS_THICKNESS_PX.0, ranges::PROGRESS_THICKNESS_PX.1)
    }
}

// ── System monitor ───────────────────────────────────────────────────────

/// One reading of the system monitor column. Each is an option the user
/// switches on or off; the GPU readings show one line per GPU when there
/// are several, and none at all on a machine without one.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MonitorRow {
    Cpu,
    /// CPU load history graph.
    CpuGraph,
    /// GPU load.
    Gpu,
    /// Video memory used / total.
    Vram,
    Memory,
    /// Swap used / total (hidden without swap).
    Swap,
    GpuPower,
    CpuTemp,
    GpuTemp,
    Fan,
    Battery,
    /// Root filesystem used / total with a thin bar.
    Disk,
    Download,
    Upload,
    /// Download / upload history graph.
    NetGraph,
}

impl MonitorRow {
    /// Every reading, in display order.
    pub const ALL: [MonitorRow; 15] = [
        Self::Cpu,
        Self::CpuGraph,
        Self::Gpu,
        Self::Vram,
        Self::Memory,
        Self::Swap,
        Self::GpuPower,
        Self::CpuTemp,
        Self::GpuTemp,
        Self::Fan,
        Self::Battery,
        Self::Disk,
        Self::Download,
        Self::Upload,
        Self::NetGraph,
    ];

    /// The rows a new monitor (or one whose config names none) shows:
    /// every reading but VRAM and swap, which came later and stay opt-in
    /// so existing monitors look the same. The GPU rows only show where a
    /// GPU reports them.
    pub const DEFAULT: [MonitorRow; 13] = [
        Self::Cpu,
        Self::CpuGraph,
        Self::Gpu,
        Self::Memory,
        Self::GpuPower,
        Self::CpuTemp,
        Self::GpuTemp,
        Self::Fan,
        Self::Battery,
        Self::Disk,
        Self::Download,
        Self::Upload,
        Self::NetGraph,
    ];

    /// A reading of the GPU (locked where no GPU reports it).
    pub fn is_gpu(self) -> bool {
        matches!(self, Self::Gpu | Self::Vram | Self::GpuPower | Self::GpuTemp)
    }

    /// Switches `self` in `rows` on or off, keeping the display order.
    pub fn toggle_in(self, rows: &mut Vec<MonitorRow>) {
        if let Some(i) = rows.iter().position(|r| *r == self) {
            rows.remove(i);
        } else {
            rows.push(self);
            rows.sort_by_key(|r| Self::ALL.iter().position(|a| a == r));
        }
    }
}

/// A conky-style column of readings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorWidget {
    /// Rows shown, top to bottom.
    pub rows: Vec<MonitorRow>,
    /// Seconds between readings (sampling runs only while the widget shows).
    pub interval_s: u32,
    /// Column width, reference px.
    pub width: f32,
    /// Section title above the rows (empty = none).
    pub title: String,
    pub style: MonitorStyle,
    /// Gauges: ring width (reference px), a wavy ring, a ring of dots, and
    /// where the ring starts (degrees from the top).
    pub ring_width: f32,
    pub wavy: bool,
    pub dotted: bool,
    pub start_angle: f32,
    /// Meters: segments per meter.
    pub segments: u32,
    pub look: Look,
    /// Digits (and the language of the labels).
    pub locale: TextOptions,
}

/// How the system monitor shows its readings.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MonitorStyle {
    /// A column of labelled values, bars and graphs.
    #[default]
    Column,
    /// Ring gauges for the readings that are a share (CPU, GPU, memory,
    /// disk, battery), the others listed under them.
    Gauges,
    /// Segmented meters (like a battery's cells) for the shares.
    Meters,
    /// Tall bars side by side for the shares.
    Bars,
    /// Arc gauges with ticks for the temperatures and the fan.
    Thermals,
}

impl Default for MonitorWidget {
    fn default() -> Self {
        Self {
            rows: MonitorRow::DEFAULT.to_vec(),
            interval_s: ranges::MONITOR_INTERVAL_DEFAULT_S,
            width: ranges::MONITOR_WIDTH_DEFAULT_PX,
            title: String::new(),
            style: MonitorStyle::Column,
            ring_width: ranges::RING_WIDTH_DEFAULT_PX,
            wavy: false,
            dotted: false,
            start_angle: ranges::START_ANGLE_DEFAULT_DEG,
            segments: ranges::SEGMENTS_DEFAULT,
            look: Look::default(),
            locale: TextOptions::default(),
        }
    }
}

impl MonitorWidget {
    pub fn ring_width(&self) -> f32 {
        self.ring_width.clamp(ranges::RING_WIDTH_PX.0, ranges::RING_WIDTH_PX.1)
    }
    pub fn segments(&self) -> u32 {
        self.segments.clamp(ranges::SEGMENTS.0, ranges::SEGMENTS.1)
    }

    pub fn interval_s(&self) -> u32 {
        self.interval_s.clamp(ranges::MONITOR_INTERVAL_S.0, ranges::MONITOR_INTERVAL_S.1)
    }
}

// ── Decorations ──────────────────────────────────────────────────────────

/// What a decoration draws.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DecorShape {
    /// `text`, horizontal or vertical.
    #[default]
    Text,
    /// A filled circle.
    Disc,
    /// A circle outline.
    Circle,
    /// Parallel bars in a block.
    Stripes,
    /// Parallel wavy lines.
    Waves,
    /// One straight line.
    Rule,
    /// A rectangular frame.
    Frame,
    /// A strip of washi tape (`count` stripes, torn ends).
    Tape,
}

/// A static decoration (drawn once).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Decoration {
    pub shape: DecorShape,
    pub text: String,
    /// Text runs top to bottom, columns from right to left: CJK and kana
    /// stand upright one character under the other; joined and
    /// right-to-left scripts (Arabic, Hebrew, …) and Latin keep their
    /// shaped line, turned a quarter (see [`Self::sideways`]).
    pub vertical: bool,
    /// Vertical text: which way the turned runs face.
    pub sideways: Sideways,
    /// Vertical text: Latin letters and digits stand upright too, one under
    /// another, instead of turning with the line.
    pub upright_latin: bool,
    /// Text size, reference px.
    pub size: f32,
    pub weight: u16,
    /// Letter spacing, em.
    pub tracking: f32,
    /// Shape extent, reference px.
    pub width: f32,
    pub height: f32,
    /// Line width of outlines, rules, stripes and waves, reference px.
    pub stroke: f32,
    /// Stripes / waves in the block.
    pub count: u32,
    /// Rotation, degrees clockwise.
    pub rotation: f32,
    pub look: Look,
}

impl Default for Decoration {
    fn default() -> Self {
        Self {
            shape: DecorShape::Text,
            text: String::new(),
            vertical: false,
            sideways: Sideways::default(),
            upright_latin: false,
            size: ranges::TEXT_SIZE_DEFAULT_PX,
            weight: ranges::WEIGHT_DEFAULT,
            tracking: 0.0,
            width: ranges::SHAPE_SIZE_DEFAULT_PX,
            height: ranges::SHAPE_SIZE_DEFAULT_PX,
            stroke: ranges::STROKE_DEFAULT_PX,
            count: ranges::REPEAT_DEFAULT,
            rotation: 0.0,
            look: Look::default(),
        }
    }
}

/// How vertical text turns the runs that don't stand upright.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Sideways {
    /// The letters' baseline faces the middle of the screen: clockwise on
    /// the right half (and in the middle), counter-clockwise on the left.
    #[default]
    Auto,
    /// A quarter turn clockwise (Latin reads top to bottom).
    Clockwise,
    /// A quarter turn counter-clockwise (Latin reads bottom to top).
    CounterClockwise,
}

impl Sideways {
    /// Whether runs turn clockwise for a widget placed at `place`.
    pub fn clockwise(self, place: &Placement) -> bool {
        match self {
            Sideways::Clockwise => true,
            Sideways::CounterClockwise => false,
            Sideways::Auto => place.anchor.fraction().0 + place.x.clamp(ranges::OFFSET.0, ranges::OFFSET.1) >= HALF,
        }
    }
}

/// The middle of the display, as a fraction.
const HALF: f32 = 0.5;

// ── Plane visualizer ─────────────────────────────────────────────────────

/// Visualizer shapes.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VisualizerShape {
    /// Bars rising from a line (rotate it to follow something in the art).
    #[default]
    Bars,
    /// Bars growing both ways from the line.
    Mirrored,
    /// A filled wave strip.
    Wave,
    /// Spikes around a circle.
    Radial,
    /// Bars of stacked square segments, like an LED meter: the accent,
    /// then amber, then coral near the top.
    Led,
}

/// An audio visualizer painted on the wallpaper layer (it can sit behind
/// the subject). Listens only while music plays and the desktop shows;
/// freezes and stops listening after `silence_s` of silence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaneVisualizer {
    pub shape: VisualizerShape,
    pub bars: u32,
    /// Length of the line (bars, wave) or the inner radius (radial),
    /// reference px.
    pub length: f32,
    /// Tallest bar / swing, reference px.
    pub height: f32,
    /// Rotation of the line, degrees clockwise.
    pub angle: f32,
    /// Gap between bars, fraction of a slot.
    pub gap: f32,
    pub fps: u32,
    pub silence_s: u32,
    pub look: Look,
}

impl Default for PlaneVisualizer {
    fn default() -> Self {
        Self {
            shape: VisualizerShape::Bars,
            bars: ranges::VISUALIZER_BARS_DEFAULT,
            length: ranges::VISUALIZER_LENGTH_DEFAULT_PX,
            height: ranges::VISUALIZER_HEIGHT_DEFAULT_PX,
            angle: 0.0,
            gap: ranges::VISUALIZER_GAP_DEFAULT,
            fps: ranges::VISUALIZER_FPS_DEFAULT,
            silence_s: ranges::VISUALIZER_SILENCE_DEFAULT_S,
            look: Look { tone: Tone::Accent, ..Look::default() },
        }
    }
}

impl PlaneVisualizer {
    pub fn fps(&self) -> u32 {
        self.fps.clamp(ranges::VISUALIZER_FPS.0, ranges::VISUALIZER_FPS.1)
    }

    pub fn bars(&self) -> u32 {
        self.bars.clamp(ranges::VISUALIZER_BARS.0, ranges::VISUALIZER_BARS.1)
    }
}

// ── List helpers ─────────────────────────────────────────────────────────

impl DesktopWidgetsConfig {
    /// A new id for a widget of `kind` (`clock-3`): one more than the
    /// largest number in use.
    pub fn next_id(&self, kind: &str) -> String {
        let n = self.items.iter().filter_map(|w| w.id.strip_prefix(kind)?.strip_prefix('-')?.parse::<u32>().ok()).max().unwrap_or(0);
        format!("{kind}-{}", n + 1)
    }

    /// Adds `widget` (fresh id, at the top) unless the list is full;
    /// returns its id.
    pub fn add(&mut self, mut widget: DesktopWidget) -> Option<String> {
        if self.items.len() >= ranges::MAX_WIDGETS {
            return None;
        }
        widget.id = self.next_id(widget.kind.id());
        let id = widget.id.clone();
        self.items.push(widget);
        Some(id)
    }

    /// Adds a widget the user just created: in front of the wallpaper's
    /// subject, so it shows wherever it lands (behind a subject that
    /// covers the middle of the screen, a fresh widget was invisible until
    /// dragged clear), and — when it asks for the centre and something
    /// already sits there — on the first free anchor ([`FREE_ANCHORS`])
    /// rather than on top of it. Moving it behind is one switch away.
    pub fn add_new(&mut self, mut widget: DesktopWidget) -> Option<String> {
        widget.place.depth = Depth::Front;
        let centred = widget.place.anchor == Anchor::Center && widget.place.x == 0.0 && widget.place.y == 0.0;
        let card = matches!(widget.kind, WidgetKind::Card(_));
        if centred && !card {
            let taken: Vec<Anchor> = self.items.iter().filter(|w| w.enabled && !matches!(w.kind, WidgetKind::Card(_))).map(|w| w.place.anchor).collect();
            if taken.contains(&Anchor::Center)
                && let Some(free) = FREE_ANCHORS.into_iter().find(|a| !taken.contains(a))
            {
                widget.place.anchor = free;
                (widget.place.x, widget.place.y) = free.inset();
            }
        }
        self.add(widget)
    }

    /// Ids of widgets that repeat an earlier one exactly (same kind and
    /// settings), in list order: what "Remove duplicates" takes away.
    pub fn duplicates(&self) -> Vec<String> {
        self.items.iter().enumerate().filter(|(i, w)| self.items[..*i].iter().any(|e| e.kind == w.kind)).map(|(_, w)| w.id.clone()).collect()
    }

    /// Removes every widget that repeats an earlier one exactly; returns
    /// how many went.
    pub fn remove_duplicates(&mut self) -> usize {
        let gone = self.duplicates();
        self.items.retain(|w| !gone.contains(&w.id));
        gone.len()
    }

    /// The first widget with exactly this kind and settings.
    pub fn find_same(&self, kind: &WidgetKind) -> Option<&DesktopWidget> {
        self.items.iter().find(|w| &w.kind == kind)
    }

    pub fn get(&self, id: &str) -> Option<&DesktopWidget> {
        self.items.iter().find(|w| w.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut DesktopWidget> {
        self.items.iter_mut().find(|w| w.id == id)
    }

    /// Enabled widgets the wallpaper renderer paints (no cards), capped at
    /// [`ranges::MAX_WIDGETS`]. The depth clock faces are excluded (they
    /// have their own planes, see [`Self::depth_clock`]).
    pub fn painted(&self) -> impl Iterator<Item = &DesktopWidget> {
        self.items
            .iter()
            .take(ranges::MAX_WIDGETS)
            .filter(|w| w.enabled && w.kind.on_wallpaper_plane() && !matches!(&w.kind, WidgetKind::Clock(c) if c.face.depth_style().is_some()))
    }

    /// The depth clock: the first enabled clock with a depth face.
    pub fn depth_clock(&self) -> Option<(&DesktopWidget, &ClockWidget)> {
        self.items.iter().filter(|w| w.enabled).find_map(|w| match &w.kind {
            WidgetKind::Clock(c) if c.face.depth_style().is_some() => Some((w, c)),
            _ => None,
        })
    }

    /// Repairs hand edits: unique, non-empty ids.
    pub fn normalize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        for i in 0..self.items.len() {
            let id = self.items[i].id.trim().to_string();
            if id.is_empty() || !seen.insert(id.clone()) {
                let kind = self.items[i].kind.id();
                let mut n = i + 1;
                let fresh = loop {
                    let c = format!("{kind}-{n}");
                    if !seen.contains(&c) && !self.items.iter().any(|w| w.id == c) {
                        break c;
                    }
                    n += 1;
                };
                seen.insert(fresh.clone());
                self.items[i].id = fresh;
            } else {
                self.items[i].id = id;
            }
        }
    }
}

// ── Presets ──────────────────────────────────────────────────────────────

/// Decoration presets offered by Studio's gallery (original designs).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Preset {
    /// One Arabic calligraphic line: "الإسكندرية · المندرة".
    AlexandriaLine,
    /// A vertical poem block — placeholder text to replace with yours.
    VerticalPoem,
    /// A soft accent disc, a hairline rule and a stripe block.
    Geometric,
}

/// Text of [`Preset::VerticalPoem`] until you write your own ("your poem
/// here", in three columns).
pub const POEM_PLACEHOLDER: &str = "ここに\nあなたの\n詩を";
/// Text of [`Preset::AlexandriaLine`].
pub const ALEXANDRIA_LINE: &str = "الإسكندرية · المندرة";

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::AlexandriaLine, Preset::VerticalPoem, Preset::Geometric];

    /// The widgets the preset adds (ids are given by [`DesktopWidgetsConfig::add`]).
    pub fn widgets(self) -> Vec<DesktopWidget> {
        let place = |anchor: Anchor, x: f32, y: f32, opacity: f32| Placement { anchor, x, y, opacity, ..Placement::default() };
        let deco = |d: Decoration, place: Placement| DesktopWidget { place, kind: WidgetKind::Decoration(d), ..DesktopWidget::default() };
        match self {
            Preset::AlexandriaLine => vec![deco(
                Decoration { text: ALEXANDRIA_LINE.into(), size: 76.0, weight: 700, look: Look { font: FontRole::Arabic, effect: Effect::Shadow, ..Look::default() }, ..Decoration::default() },
                place(Anchor::Bottom, 0.0, -0.1, 0.92),
            )],
            Preset::VerticalPoem => vec![deco(
                Decoration { text: POEM_PLACEHOLDER.into(), vertical: true, size: 58.0, weight: 500, look: Look { tone: Tone::Accent, ..Look::default() }, ..Decoration::default() },
                place(Anchor::Right, -0.08, 0.0, 0.85),
            )],
            Preset::Geometric => vec![
                deco(Decoration { shape: DecorShape::Disc, width: 380.0, look: Look { tone: Tone::Accent, ..Look::default() }, ..Decoration::default() }, place(Anchor::TopRight, -0.12, 0.1, 0.32)),
                deco(Decoration { shape: DecorShape::Rule, width: 520.0, stroke: 1.5, ..Decoration::default() }, place(Anchor::Right, -0.06, 0.12, 0.6)),
                deco(Decoration { shape: DecorShape::Stripes, width: 200.0, height: 110.0, count: 6, ..Decoration::default() }, place(Anchor::BottomRight, -0.08, -0.12, 0.45)),
            ],
        }
    }
}

// ── The depth clock (predates widgets) ───────────────────────────────────

/// Position / scale differences below this are rounding, not edits.
const SYNC_EPSILON: f32 = 1e-5;

/// Id of the migrated depth clock.
pub const DEPTH_CLOCK_ID: &str = "depth-clock";

/// Moves the `[wallpaper]` depth clock into `[desktop.widgets]` as the
/// first clock widget (once; disabled when the clock was off, so it can be
/// switched back on in the gallery). Returns whether anything changed.
pub fn migrate_depth_clock(cfg: &mut ShellConfig) -> bool {
    let w = &mut cfg.desktop.widgets;
    if w.migrated {
        return false;
    }
    w.migrated = true;
    let wp = &cfg.wallpaper;
    let clock = DesktopWidget {
        id: DEPTH_CLOCK_ID.into(),
        enabled: wp.desktop_clock,
        place: Placement {
            anchor: Anchor::Center,
            x: wp.clock_x.clamp(0.0, 1.0) - 0.5,
            y: wp.clock_y.clamp(0.0, 1.0) - 0.5,
            scale: wp.clock_scale,
            ..Placement::default()
        },
        kind: WidgetKind::Clock(ClockWidget {
            date: wp.show_date,
            weather: wp.show_weather,
            audio_ring: wp.audio_ring,
            // As the old clock showed it: its own digits.
            locale: TextOptions { numerals: legacy_numerals(ClockFace::from_depth_style(wp.clock_style), false), ..ClockWidget::with_face(ClockFace::from_depth_style(wp.clock_style)).locale },
            ..ClockWidget::with_face(ClockFace::from_depth_style(wp.clock_style))
        }),
    };
    if !w.items.iter().any(|i| i.id == DEPTH_CLOCK_ID) {
        w.items.insert(0, clock);
    }
    true
}

/// The digits a clock showed before [`WIDGETS_VERSION`] 1: Eastern Arabic
/// on the Arabic depth face or when asked for, else Western, whatever the
/// language.
pub fn legacy_numerals(face: ClockFace, eastern: bool) -> Numerals {
    if eastern || face == ClockFace::Arabic { Numerals::EasternArabic } else { Numerals::Western }
}

/// Brings a list written before [`WIDGETS_VERSION`] 1 forward so it shows
/// exactly what it showed: digits the old way (the language never chose
/// them), the depth clock's Gregorian · Hijri pill, and the display face
/// as the widgets font (Typography → Desktop widgets falls back to it, so
/// nothing changes until a widgets font is chosen). Returns whether
/// anything changed.
pub fn upgrade(w: &mut DesktopWidgetsConfig) -> bool {
    if w.version >= WIDGETS_VERSION {
        return false;
    }
    for item in &mut w.items {
        match &mut item.kind {
            WidgetKind::Clock(c) => {
                c.locale.numerals = legacy_numerals(c.face, c.legacy_eastern);
                c.legacy_eastern = false;
                if c.face.depth_style().is_some() {
                    c.locale.calendar = Calendar::Both;
                }
            }
            WidgetKind::Monitor(m) => m.locale.numerals = Numerals::Western,
            _ => {}
        }
        if let Some(look) = item.kind.look_mut()
            && look.font == FontRole::Display
        {
            look.font = FontRole::Widgets;
        }
    }
    w.version = WIDGETS_VERSION;
    true
}

/// Everything a loaded config needs: the depth clock moved in (once),
/// widget ids repaired and the old `[wallpaper]` keys brought in step.
/// Run by [`crate::ConfigStore`] on every load; returns whether anything
/// changed.
pub fn migrate(cfg: &mut ShellConfig) -> bool {
    let moved = migrate_depth_clock(cfg);
    let before = cfg.desktop.widgets.items.iter().map(|w| w.id.clone()).collect::<Vec<_>>();
    cfg.desktop.widgets.normalize();
    let renamed = cfg.desktop.widgets.items.iter().map(|w| &w.id).ne(before.iter());
    sync_depth_clock(cfg) | moved | renamed
}

/// Writes the depth clock widget back into the `[wallpaper]` keys its
/// renderer (and older tools) read. No-op before migration. Returns
/// whether anything changed.
pub fn sync_depth_clock(cfg: &mut ShellConfig) -> bool {
    if !cfg.desktop.widgets.migrated {
        return false;
    }
    let before = cfg.wallpaper.clone();
    let wp = &mut cfg.wallpaper;
    match cfg.desktop.widgets.depth_clock() {
        Some((w, c)) => {
            let (fx, fy) = w.place.anchor.fraction();
            wp.desktop_clock = true;
            wp.clock_style = c.face.depth_style().unwrap_or_default();
            // Rounding noise (0.5 + (0.32 − 0.5)) is not a change.
            let set = |v: &mut f32, to: f32| {
                if (*v - to).abs() > SYNC_EPSILON {
                    *v = to;
                }
            };
            set(&mut wp.clock_x, (fx + w.place.x).clamp(0.0, 1.0));
            set(&mut wp.clock_y, (fy + w.place.y).clamp(0.0, 1.0));
            set(&mut wp.clock_scale, w.place.scale);
            wp.show_date = c.date;
            wp.show_weather = c.weather;
            wp.audio_ring = c.audio_ring;
        }
        None => wp.desktop_clock = false,
    }
    *wp != before
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widgets_round_trip_through_toml() {
        let mut cfg = DesktopWidgetsConfig::default();
        cfg.add(DesktopWidget { kind: WidgetKind::Clock(ClockWidget { face: ClockFace::DayBlock, ..Default::default() }), ..Default::default() });
        cfg.add(DesktopWidget { kind: WidgetKind::Monitor(MonitorWidget::default()), place: Placement { anchor: Anchor::TopRight, x: -0.02, ..Default::default() }, ..Default::default() });
        cfg.add(DesktopWidget { kind: WidgetKind::Decoration(Decoration { text: "المندرة".into(), vertical: true, ..Default::default() }), ..Default::default() });
        cfg.add(DesktopWidget { kind: WidgetKind::Visualizer(PlaneVisualizer { angle: -30.0, ..Default::default() }), ..Default::default() });
        cfg.add(DesktopWidget { kind: WidgetKind::Card(CardWidget { card: "media".into(), ..Default::default() }), ..Default::default() });
        let text = toml::to_string(&cfg).unwrap();
        assert!(text.contains("kind = \"day-block\"") || text.contains("face = \"day-block\""), "{text}");
        let back: DesktopWidgetsConfig = toml::from_str(&text).unwrap();
        assert_eq!(back, cfg);
        assert_eq!(cfg.items.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), ["clock-1", "monitor-1", "decoration-1", "visualizer-1", "card-1"]);
    }

    #[test]
    fn hand_written_widgets_fill_in_defaults() {
        let w: DesktopWidgetsConfig = toml::from_str("[[items]]\nkind = \"clock\"\nface = \"ring\"\n[items.place]\nanchor = \"bottom-left\"\n\n[[items]]\nkind = \"monitor\"\nrows = [\"cpu\", \"disk\"]\n").unwrap();
        let WidgetKind::Clock(c) = &w.items[0].kind else { panic!() };
        assert_eq!(c.face, ClockFace::Ring);
        assert!(w.items[0].enabled && w.items[0].place.anchor == Anchor::BottomLeft && w.items[0].place.scale == 1.0);
        let WidgetKind::Monitor(m) = &w.items[1].kind else { panic!() };
        assert_eq!(m.rows, [MonitorRow::Cpu, MonitorRow::Disk]);
        assert_eq!(m.interval_s(), ranges::MONITOR_INTERVAL_DEFAULT_S);
        let mut w = w;
        w.normalize();
        assert_eq!((w.items[0].id.as_str(), w.items[1].id.as_str()), ("clock-1", "monitor-2"));
    }

    #[test]
    fn monitor_options_keep_their_defaults_and_round_trip() {
        // A monitor saved before VRAM and swap existed shows what it did.
        let m = MonitorWidget::default();
        assert_eq!(m.rows, MonitorRow::DEFAULT);
        assert!(!m.rows.contains(&MonitorRow::Vram) && !m.rows.contains(&MonitorRow::Swap), "the new readings are opt-in");
        assert!([MonitorRow::Cpu, MonitorRow::Memory, MonitorRow::Gpu, MonitorRow::GpuTemp, MonitorRow::GpuPower, MonitorRow::CpuTemp, MonitorRow::Disk, MonitorRow::Download, MonitorRow::Upload, MonitorRow::Battery].iter().all(|r| m.rows.contains(r)));
        assert!(MonitorRow::DEFAULT.iter().all(|r| MonitorRow::ALL.contains(r)));
        let old: MonitorWidget = toml::from_str("style = \"gauges\"\n").unwrap();
        assert_eq!(old.rows, MonitorRow::DEFAULT, "no rows key: the old defaults");
        let listed: MonitorWidget = toml::from_str("rows = [\"cpu\", \"gpu-temp\"]\n").unwrap();
        assert_eq!(listed.rows, [MonitorRow::Cpu, MonitorRow::GpuTemp], "an old list loads unchanged");
        // Every option survives a save and load.
        let all = MonitorWidget { rows: MonitorRow::ALL.to_vec(), ..MonitorWidget::default() };
        let text = toml::to_string(&all).unwrap();
        assert!(text.contains("\"vram\"") && text.contains("\"swap\""));
        assert_eq!(toml::from_str::<MonitorWidget>(&text).unwrap(), all);
        let one = MonitorWidget { rows: vec![MonitorRow::Vram], ..MonitorWidget::default() };
        assert_eq!(toml::from_str::<MonitorWidget>(&toml::to_string(&one).unwrap()).unwrap(), one);
        // Toggling keeps the display order.
        let mut rows = vec![MonitorRow::Cpu, MonitorRow::Disk];
        MonitorRow::Vram.toggle_in(&mut rows);
        MonitorRow::Swap.toggle_in(&mut rows);
        assert_eq!(rows, [MonitorRow::Cpu, MonitorRow::Vram, MonitorRow::Swap, MonitorRow::Disk]);
        MonitorRow::Cpu.toggle_in(&mut rows);
        assert_eq!(rows, [MonitorRow::Vram, MonitorRow::Swap, MonitorRow::Disk]);
        assert_eq!(MonitorRow::ALL.iter().filter(|r| r.is_gpu()).count(), 4);
    }

    #[test]
    fn depth_clock_moves_in_as_the_first_clock() {
        let mut cfg: ShellConfig = toml::from_str("[wallpaper]\nclock_style = \"classic\"\nclock_x = 0.25\nclock_y = 0.7\nclock_scale = 1.5\nshow_weather = false\n").unwrap();
        // `ConfigStore` runs this on every load.
        assert!(migrate(&mut cfg));
        assert!(!migrate(&mut cfg), "once");
        assert!(cfg.desktop.widgets.migrated);
        let first = &cfg.desktop.widgets.items[0];
        assert_eq!(first.id, DEPTH_CLOCK_ID);
        assert!(first.enabled);
        let WidgetKind::Clock(c) = &first.kind else { panic!() };
        assert_eq!(c.face, ClockFace::Classic);
        assert!(c.date && !c.weather && c.audio_ring);
        assert!((first.place.x + 0.25).abs() < 1e-6 && (first.place.y - 0.2).abs() < 1e-6 && first.place.scale == 1.5);
        // The widget is the source now: edits flow back to the old keys.
        let (x, y) = first.place.origin((100.0, 100.0), (0.0, 0.0));
        assert!((x - 25.0).abs() < 1e-4 && (y - 70.0).abs() < 1e-4);
        cfg.desktop.widgets.items[0].place.x = 0.1;
        assert!(sync_depth_clock(&mut cfg));
        assert!((cfg.wallpaper.clock_x - 0.6).abs() < 1e-6);
        cfg.desktop.widgets.items[0].enabled = false;
        sync_depth_clock(&mut cfg);
        assert!(!cfg.wallpaper.desktop_clock, "no enabled depth clock");
        // A clock that was off migrates disabled; nothing else is added.
        let mut off: ShellConfig = toml::from_str("[wallpaper]\ndesktop_clock = false\n").unwrap();
        migrate(&mut off);
        assert_eq!(off.desktop.widgets.items.len(), 1);
        assert!(!off.desktop.widgets.items[0].enabled);
        assert!(!sync_depth_clock(&mut off), "still off");
        assert_eq!(off.desktop.widgets.painted().count(), 0, "new widgets are all off by default");
    }

    #[test]
    fn migration_keeps_existing_widgets_and_runs_once() {
        let mut cfg = ShellConfig::default();
        cfg.desktop.widgets.add(DesktopWidget { kind: WidgetKind::Monitor(MonitorWidget::default()), ..Default::default() });
        assert!(migrate_depth_clock(&mut cfg));
        assert_eq!(cfg.desktop.widgets.items.len(), 2);
        assert_eq!(cfg.desktop.widgets.items[0].id, DEPTH_CLOCK_ID);
        assert!(!migrate_depth_clock(&mut cfg));
        assert_eq!(cfg.desktop.widgets.items.len(), 2);
        // The depth faces have their own planes; the monitor is painted.
        assert_eq!(cfg.desktop.widgets.painted().map(|w| w.kind.id()).collect::<Vec<_>>(), ["monitor"]);
        assert_eq!(cfg.desktop.widgets.depth_clock().map(|(w, _)| w.id.as_str()), Some(DEPTH_CLOCK_ID));
    }

    #[test]
    fn placement_anchors_and_drag_offsets() {
        let p = Placement { anchor: Anchor::BottomRight, x: -0.1, y: -0.1, ..Default::default() };
        let (x, y) = p.origin((1000.0, 500.0), (200.0, 100.0));
        assert_eq!((x, y), (700.0, 350.0));
        let back = p.offset_for((1000.0, 500.0), (200.0, 100.0), (x, y));
        assert!((back.0 + 0.1).abs() < 1e-6 && (back.1 + 0.1).abs() < 1e-6);
        let c = Placement::default();
        assert_eq!(c.origin((1000.0, 500.0), (200.0, 100.0)), (400.0, 200.0), "centred");
        assert!(c.shows_on(&["DP-1"]));
        let only = Placement { displays: vec!["HDMI-A-1".into()], ..Default::default() };
        assert!(!only.shows_on(&["DP-1", "Dell U2720Q"]) && only.shows_on(&["hdmi-a-1"]));
    }

    #[test]
    fn placement_keeps_the_box_on_screen_and_clear_of_the_bar() {
        let bar = Insets { top: 40.0, ..Default::default() };
        // A centre-anchored box pushed up past the top edge (an old depth
        // clock offset) lands just under the bar, at any resolution.
        let up = Placement { anchor: Anchor::Top, y: -0.2, ..Default::default() };
        for (out, size) in [((1920.0, 1080.0), (400.0, 200.0)), ((3840.0, 2160.0), (800.0, 400.0))] {
            let (x, y) = up.origin_in(out, size, bar.scaled(out.1 / 1080.0));
            assert!((y - 40.0 * out.1 / 1080.0).abs() < 1e-3, "{out:?}: {y}");
            assert!(x >= 0.0 && x + size.0 <= out.0);
        }
        // Every anchor and extreme offset stays inside.
        for anchor in Anchor::ALL {
            for (dx, dy) in [(-1.0, -1.0), (1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (0.0, 0.0)] {
                let p = Placement { anchor, x: dx, y: dy, ..Default::default() };
                let (x, y) = p.origin_in((1920.0, 1080.0), (300.0, 120.0), bar);
                assert!(x >= 0.0 && x + 300.0 <= 1920.0 && y >= 40.0 && y + 120.0 <= 1080.0, "{anchor:?} {dx},{dy}: {x},{y}");
            }
        }
        // Too big to fit: centred on the free area.
        let (_, y) = Placement::default().origin_in((100.0, 100.0), (10.0, 140.0), bar);
        assert!((y - (40.0 + (60.0 - 140.0) / 2.0)).abs() < 1e-4);
        // Dragging past the edge saves the offset that is shown.
        let p = Placement { anchor: Anchor::TopLeft, ..Default::default() };
        let off = p.offset_for((1000.0, 500.0), (200.0, 100.0), (-50.0, 450.0));
        assert_eq!(Placement { x: off.0, y: off.1, ..p }.origin((1000.0, 500.0), (200.0, 100.0)), (0.0, 400.0));
    }

    #[test]
    fn new_widgets_show_in_front_and_avoid_a_taken_centre() {
        let mut ws = DesktopWidgetsConfig::default();
        let clock = || DesktopWidget { kind: WidgetKind::Clock(ClockWidget::default()), ..DesktopWidget::default() };
        let first = ws.add_new(clock()).unwrap();
        let first = ws.get(&first).unwrap();
        assert_eq!(first.place.depth, Depth::Front, "a fresh widget is never hidden behind the subject");
        assert_eq!(first.place.anchor, Anchor::Center);
        let second = ws.add_new(DesktopWidget { kind: WidgetKind::Monitor(MonitorWidget::default()), ..DesktopWidget::default() }).unwrap();
        let second = ws.get(&second).unwrap();
        assert_eq!(second.place.anchor, FREE_ANCHORS[0], "the centre is taken: the first free corner");
        assert_eq!((second.place.x, second.place.y), FREE_ANCHORS[0].inset());
    }

    #[test]
    fn duplicates_are_exact_repeats_and_go_in_one_step() {
        let mut ws = DesktopWidgetsConfig::default();
        let clock = || DesktopWidget { kind: WidgetKind::Clock(ClockWidget::default()), ..DesktopWidget::default() };
        let a = ws.add(clock()).unwrap();
        let b = ws.add(clock()).unwrap();
        let c = ws.add(DesktopWidget { kind: WidgetKind::Monitor(MonitorWidget::default()), ..DesktopWidget::default() }).unwrap();
        let d = ws.add(clock()).unwrap();
        assert_eq!(ws.duplicates(), vec![b.clone(), d.clone()]);
        assert_eq!(ws.remove_duplicates(), 2);
        assert_eq!(ws.items.iter().map(|w| w.id.clone()).collect::<Vec<_>>(), vec![a, c], "the first of each stays");
        assert!(ws.duplicates().is_empty());
    }
}
