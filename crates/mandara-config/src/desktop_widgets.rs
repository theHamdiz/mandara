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
}

// ── Shared model ─────────────────────────────────────────────────────────

/// `[desktop]`: the desktop (wallpaper layer) beyond the image itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopConfig {
    pub widgets: DesktopWidgetsConfig,
}

/// `[desktop.widgets]`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopWidgetsConfig {
    /// The `[wallpaper]` depth clock has been moved into [`Self::items`].
    pub migrated: bool,
    /// Every widget (the depth clock and the cards too) is hidden for now,
    /// without switching any off: the desktop menu's "Hide desktop widgets"
    /// (`mandara msg desktop widgets hide`). Edit mode shows them anyway.
    pub hidden: bool,
    /// Widgets in paint order (later ones on top).
    pub items: Vec<DesktopWidget>,
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
}

impl Default for Placement {
    fn default() -> Self {
        Self { displays: Vec::new(), anchor: Anchor::Center, x: 0.0, y: 0.0, scale: 1.0, opacity: 1.0, depth: Depth::Behind }
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
    /// An interactive card (media, calendar, …) on its own small layer
    /// surface, drawn by the shell's card host: `card` names it, `options`
    /// holds its settings.
    Card(CardWidget),
}

impl WidgetKind {
    /// Stable kind id (`clock`, `monitor`, …), as in TOML.
    pub fn id(&self) -> &'static str {
        match self {
            WidgetKind::Clock(_) => "clock",
            WidgetKind::Monitor(_) => "monitor",
            WidgetKind::Decoration(_) => "decoration",
            WidgetKind::Visualizer(_) => "visualizer",
            WidgetKind::Card(_) => "card",
        }
    }

    /// Painted by the software wallpaper renderer (cards are not).
    pub fn on_wallpaper_plane(&self) -> bool {
        !matches!(self, WidgetKind::Card(_))
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
    /// Display face (Alexandria by default).
    #[default]
    Display,
    /// Interface face (Inter).
    Ui,
    /// Arabic face (Cairo).
    Arabic,
    /// Monospace face (JetBrains Mono).
    Mono,
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
    pub effect: Effect,
}

impl Default for Look {
    fn default() -> Self {
        Self { tone: Tone::Auto, accent: Tone::Accent, custom_color: String::new(), custom_accent: String::new(), font: FontRole::Display, effect: Effect::None }
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
}

impl ClockFace {
    pub const ALL: [ClockFace; 10] = [
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
    /// Eastern Arabic numerals (٠١٢٣…).
    pub eastern_numerals: bool,
    /// The date line (and, for the depth faces, the Gregorian + Hijri pill).
    pub date: bool,
    /// Current weather where the face has room for it (needs the weather
    /// plugin and a location).
    pub weather: bool,
    /// Depth faces: the audio ring around the clock.
    pub audio_ring: bool,
    pub ring_progress: RingProgress,
}

impl Default for ClockWidget {
    fn default() -> Self {
        Self { face: ClockFace::Minimal, look: Look::default(), seconds: false, eastern_numerals: false, date: true, weather: true, audio_ring: false, ring_progress: RingProgress::Hour }
    }
}

// ── System monitor ───────────────────────────────────────────────────────

/// One reading of the system monitor column.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MonitorRow {
    Cpu,
    /// CPU load history graph.
    CpuGraph,
    Gpu,
    Memory,
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
    pub const ALL: [MonitorRow; 13] = [
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
    pub look: Look,
}

impl Default for MonitorWidget {
    fn default() -> Self {
        Self {
            rows: MonitorRow::ALL.to_vec(),
            interval_s: ranges::MONITOR_INTERVAL_DEFAULT_S,
            width: ranges::MONITOR_WIDTH_DEFAULT_PX,
            title: String::new(),
            look: Look { font: FontRole::Mono, ..Look::default() },
        }
    }
}

impl MonitorWidget {
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
            face: ClockFace::from_depth_style(wp.clock_style),
            date: wp.show_date,
            weather: wp.show_weather,
            audio_ring: wp.audio_ring,
            ..ClockWidget::default()
        }),
    };
    if !w.items.iter().any(|i| i.id == DEPTH_CLOCK_ID) {
        w.items.insert(0, clock);
    }
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
