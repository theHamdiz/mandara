//! Desktop-wide settings that sit beside the bar: date / time formats,
//! calendar subscriptions, the shared location, weather options, the lock
//! screen's look, hot corners, rounded screen corners and interface style.
//!
//! Every struct is `#[serde(default)]` so older configs keep loading.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Default values of this module, in one place.
pub mod defaults {
    /// Calendar subscriptions are re-fetched this often, minutes.
    pub const CALENDAR_REFRESH_MIN: u32 = 30;
    /// Default reminder lead time before an event, minutes.
    pub const REMINDER_LEAD_MIN: u32 = 10;
    /// Weather refresh interval, minutes.
    pub const WEATHER_REFRESH_MIN: u32 = 30;
    /// El-Mandara, Alexandria (the shell's namesake and default location).
    pub const LATITUDE: f64 = 31.284;
    pub const LONGITUDE: f64 = 30.017;
    pub const PLACE: &str = "El-Mandara";
    /// Lock screen background blur radius and dim, logical px / 0–1: light
    /// enough that your wallpaper stays recognisable.
    pub const LOCK_BLUR_PX: f32 = 10.0;
    pub const LOCK_DIM: f32 = 0.18;
    /// The defaults before version 1 of `[lock] look_version`.
    pub const LOCK_BLUR_PX_V0: f32 = 28.0;
    pub const LOCK_DIM_V0: f32 = 0.35;
    /// Lock screen battery reading turns coral at or below this, percent.
    pub const LOCK_LOW_BATTERY_PERCENT: u8 = 15;
    /// Bounds of `lock.low_battery_percent`.
    pub const LOCK_LOW_BATTERY_RANGE: (u8, u8) = (1, 50);
    /// Hot corner dwell before it fires, ms.
    pub const HOT_CORNER_DELAY_MS: u64 = 120;
    /// Hot corner hit square, logical px.
    pub const HOT_CORNER_SIZE_PX: f32 = 3.0;
    /// Rounded screen corner radius, logical px.
    pub const SCREEN_CORNER_RADIUS_PX: f32 = 14.0;
    /// Bar auto-hide: delay before hiding and the reveal strip, ms / px.
    pub const BAR_HIDE_DELAY_MS: u64 = 600;
    pub const BAR_REVEAL_STRIP_PX: f32 = 3.0;
    /// Wallpaper slideshow interval, minutes.
    pub const SLIDESHOW_MIN: u32 = 30;
    /// Bounds of `wallpaper.slideshow.interval_minutes`: a minute to a week.
    pub const SLIDESHOW_INTERVAL_RANGE: (u32, u32) = (1, 7 * 24 * 60);
    /// The one-click intervals Studio offers, minutes.
    pub const SLIDESHOW_PRESETS: [u32; 8] = [5, 15, 30, 60, 3 * 60, 6 * 60, 12 * 60, 24 * 60];
    /// Default "only between" window of the slideshow.
    pub const SLIDESHOW_FROM: &str = "08:00";
    pub const SLIDESHOW_UNTIL: &str = "22:00";
    /// The slideshow's own transition length, and its bounds, ms.
    pub const SLIDESHOW_TRANSITION_MS: u64 = 1200;
    pub const SLIDESHOW_TRANSITION_RANGE: (u64, u64) = (100, 5000);
    /// How deep "include sub-folders" looks below the slideshow folder.
    pub const SLIDESHOW_FOLDER_DEPTH: u8 = 8;
    /// Video wallpapers: highest frame rate drawn, and its bounds. 30 fps
    /// keeps motion smooth at a fraction of a 60 fps clip's decode cost.
    pub const VIDEO_MAX_FPS: u32 = 30;
    pub const VIDEO_FPS_RANGE: (u32, u32) = (1, 60);
}

// ── Date & time ──────────────────────────────────────────────────────────

/// Clock and date formatting (`[datetime]`). Empty format strings mean
/// "automatic" (derived from `general.clock_24h` and `bar.show_seconds`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DateTimeConfig {
    /// strftime pattern for the bar clock, e.g. `%H:%M` or `%-I:%M %p`.
    pub bar_time_format: String,
    /// strftime pattern for the bar date, e.g. `%a %-d %b`.
    pub bar_date_format: String,
    /// Show the date next to the time in the bar.
    pub bar_show_date: bool,
    /// strftime pattern for the lock screen clock.
    pub lock_time_format: String,
    /// strftime pattern for the lock screen date line.
    pub lock_date_format: String,
    /// 12-hour clocks: `09:05 PM` instead of `9:05 PM`.
    pub leading_zero_12h: bool,
    /// ISO week numbers in front of every calendar row.
    pub week_numbers: bool,
    /// Hijri (Islamic) day numbers under each calendar day, the Hijri month
    /// in the calendar's title and today's Hijri date in the clock panel.
    pub hijri_dates: bool,
    /// Eastern Arabic numerals (٠١٢٣…) in the clock panel and its calendar.
    pub eastern_numerals: bool,
    /// Which days the calendar marks as the weekend.
    pub weekend: Weekend,
    /// Month / weekday names: `auto` follows the interface language, or a
    /// POSIX locale such as `en_GB`, `ar_EG`, `de_DE`.
    pub time_locale: String,
}

impl Default for DateTimeConfig {
    fn default() -> Self {
        Self {
            bar_time_format: String::new(),
            bar_date_format: String::new(),
            bar_show_date: true,
            lock_time_format: String::new(),
            lock_date_format: String::new(),
            leading_zero_12h: false,
            week_numbers: false,
            hijri_dates: false,
            eastern_numerals: false,
            weekend: Weekend::Auto,
            time_locale: "auto".into(),
        }
    }
}

impl DateTimeConfig {
    /// The bar's time pattern: the custom one, else from the 24-hour /
    /// seconds switches.
    pub fn bar_time_pattern(&self, clock_24h: bool, seconds: bool) -> String {
        if !self.bar_time_format.trim().is_empty() {
            return self.bar_time_format.clone();
        }
        self.automatic_time(clock_24h, seconds)
    }

    /// The lock screen's time pattern (never seconds unless custom).
    pub fn lock_time_pattern(&self, clock_24h: bool) -> String {
        if !self.lock_time_format.trim().is_empty() {
            return self.lock_time_format.clone();
        }
        // The big lock clock drops the AM/PM marker; the date says enough.
        self.big_clock_pattern(clock_24h).into()
    }

    /// Hours and minutes for big clocks (lock screen, clock panel): the
    /// 24-hour and leading-zero switches, no seconds or AM/PM marker.
    pub fn big_clock_pattern(&self, clock_24h: bool) -> &'static str {
        if clock_24h {
            "%H:%M"
        } else if self.leading_zero_12h {
            "%I:%M"
        } else {
            "%-I:%M"
        }
    }

    /// The pattern implied by the 24-hour / seconds / leading-zero switches.
    pub fn automatic_time(&self, clock_24h: bool, seconds: bool) -> String {
        let hour = if clock_24h { "%H" } else if self.leading_zero_12h { "%I" } else { "%-I" };
        let secs = if seconds { ":%S" } else { "" };
        let ampm = if clock_24h { "" } else { " %p" };
        format!("{hour}:%M{secs}{ampm}")
    }
}

/// The calendar's weekend days (`datetime.weekend`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Weekend {
    /// Friday and Saturday when the week starts on Saturday or the
    /// interface is in Arabic, else Saturday and Sunday.
    #[default]
    Auto,
    SaturdaySunday,
    FridaySaturday,
    Friday,
    Sunday,
}

impl Weekend {
    /// Weekend flags indexed by days from Monday (`0` = Monday, `6` =
    /// Sunday). `saturday_first`: the week starts on Saturday; `arabic`:
    /// the interface language is Arabic (both only matter for `Auto`).
    pub fn days_from_monday(self, saturday_first: bool, arabic: bool) -> [bool; 7] {
        const FRIDAY: usize = 4;
        const SATURDAY: usize = 5;
        const SUNDAY: usize = 6;
        let mut days = [false; 7];
        let on: &[usize] = match self {
            Self::Auto if saturday_first || arabic => &[FRIDAY, SATURDAY],
            Self::Auto | Self::SaturdaySunday => &[SATURDAY, SUNDAY],
            Self::FridaySaturday => &[FRIDAY, SATURDAY],
            Self::Friday => &[FRIDAY],
            Self::Sunday => &[SUNDAY],
        };
        for d in on {
            days[*d] = true;
        }
        days
    }
}

// ── Calendar ─────────────────────────────────────────────────────────────

/// One calendar: an `https://` / `webcal://` ICS subscription or a local
/// `.ics` file (`~` allowed).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CalendarSource {
    pub name: String,
    /// URL or file path.
    pub location: String,
    /// `#rrggbb` dot colour; empty uses the theme accent.
    pub color: String,
    pub enabled: bool,
}

/// Calendar events in the clock panel, with reminders (`[calendar]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CalendarConfig {
    pub enabled: bool,
    pub sources: Vec<CalendarSource>,
    /// Subscriptions are fetched again after this many minutes.
    pub refresh_minutes: u32,
    /// A notification before each event starts.
    pub reminders: bool,
    /// Lead time for events that define no alarm of their own, minutes.
    pub reminder_minutes: u32,
    /// Use the events' own VALARM triggers when they have them.
    pub honour_event_alarms: bool,
    /// Hide events that already ended today.
    pub hide_past: bool,
    /// How many upcoming days the agenda lists.
    pub agenda_days: u32,
}

impl Default for CalendarConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sources: Vec::new(),
            refresh_minutes: defaults::CALENDAR_REFRESH_MIN,
            reminders: true,
            reminder_minutes: defaults::REMINDER_LEAD_MIN,
            honour_event_alarms: true,
            hide_past: true,
            agenda_days: 7,
        }
    }
}

// ── Location & weather ───────────────────────────────────────────────────

/// How the shared location is found.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LocationMode {
    /// The coordinates below, as typed (or found by a city search).
    #[default]
    Manual,
    /// GeoClue2 (the desktop location service), refreshed at startup.
    Geoclue,
    /// One IP-based lookup, only when you press “Locate me”.
    Ip,
}

/// One location shared by weather, prayer times and schedules
/// (`[location]`). Plugins keep honouring their own coordinates when the
/// shared location is off.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocationConfig {
    /// Weather and prayer times use this location (instead of their own).
    pub shared: bool,
    pub mode: LocationMode,
    pub latitude: f64,
    pub longitude: f64,
    /// Place name shown in widgets.
    pub name: String,
    /// The place's IANA time zone (`Africa/Cairo`), from the last place
    /// search or IP lookup; empty when unknown (then the weather
    /// forecast's is used). Studio warns when the system clock disagrees.
    pub timezone: String,
}

impl Default for LocationConfig {
    fn default() -> Self {
        Self { shared: false, mode: LocationMode::Manual, latitude: defaults::LATITUDE, longitude: defaults::LONGITUDE, name: defaults::PLACE.into(), timezone: String::new() }
    }
}

/// Wind speed unit override.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindUnit {
    /// km/h or mph from `general.units`.
    #[default]
    Auto,
    MetersPerSecond,
    Knots,
}

/// Weather service options (`[weather]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WeatherConfig {
    /// Fetch weather at all (off = no network requests for weather).
    pub enabled: bool,
    pub refresh_minutes: u32,
    pub wind_unit: WindUnit,
    /// Place name next to the temperature in the bar.
    pub show_location_in_bar: bool,
    /// Today's high / low in the bar chip.
    pub show_high_low_in_bar: bool,
}

impl Default for WeatherConfig {
    fn default() -> Self {
        Self { enabled: true, refresh_minutes: defaults::WEATHER_REFRESH_MIN, wind_unit: WindUnit::Auto, show_location_in_bar: false, show_high_low_in_bar: false }
    }
}

/// Metres per second and knots per km/h.
pub const MS_PER_KMH: f32 = 1.0 / 3.6;
pub const KNOTS_PER_KMH: f32 = 0.539_957;

impl WindUnit {
    /// A km/h speed in this unit, or `None` for [`WindUnit::Auto`].
    pub fn convert(self, kmh: f32) -> Option<(f32, &'static str)> {
        match self {
            WindUnit::Auto => None,
            WindUnit::MetersPerSecond => Some((kmh * MS_PER_KMH, "m/s")),
            WindUnit::Knots => Some((kmh * KNOTS_PER_KMH, "kn")),
        }
    }
}

// ── Lock screen ──────────────────────────────────────────────────────────

/// Lock screen clock typography.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LockClockStyle {
    /// One large light line.
    #[default]
    Classic,
    /// Hours over minutes, bold.
    Stacked,
    /// A small clock above the password field.
    Compact,
}

/// What the lock screen shows behind its card.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LockBackground {
    /// The current wallpaper, blurred.
    #[default]
    Wallpaper,
    /// An image of your choice (`lock.image`).
    Image,
    /// A solid colour from the theme.
    Color,
}

/// The lock screen's look (`[lock]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LockConfig {
    /// The look defaults this table was written for (see
    /// [`crate::upgrade`]); missing in older files.
    #[serde(default)]
    pub look_version: u32,
    pub clock_style: LockClockStyle,
    pub background: LockBackground,
    pub image: Option<PathBuf>,
    /// Background blur radius, logical px (0 = sharp).
    pub blur: f32,
    /// Darkens the background, 0–1.
    pub dim: f32,
    pub show_avatar: bool,
    pub show_date: bool,
    /// Now-playing line with the track title.
    pub show_media: bool,
    /// Count of notifications that arrived while locked / unread.
    pub show_notifications: bool,
    /// Current weather under the date.
    pub show_weather: bool,
    /// Battery percentage in the corner (laptops).
    pub show_battery: bool,
    /// Outputs other than the focused one stay solid black (OLED care).
    pub blank_other_outputs: bool,
    /// The battery reading turns coral at or below this charge, percent.
    pub low_battery_percent: u8,
}

impl LockConfig {
    /// [`LockConfig::low_battery_percent`], clamped.
    pub fn low_battery(&self) -> u8 {
        self.low_battery_percent.clamp(defaults::LOCK_LOW_BATTERY_RANGE.0, defaults::LOCK_LOW_BATTERY_RANGE.1)
    }
}

impl Default for LockConfig {
    fn default() -> Self {
        Self {
            look_version: crate::upgrade::LOCK_LOOK_VERSION,
            clock_style: LockClockStyle::Classic,
            background: LockBackground::Wallpaper,
            image: None,
            blur: defaults::LOCK_BLUR_PX,
            dim: defaults::LOCK_DIM,
            show_avatar: true,
            show_date: true,
            show_media: true,
            show_notifications: true,
            show_weather: false,
            show_battery: true,
            blank_other_outputs: false,
            low_battery_percent: defaults::LOCK_LOW_BATTERY_PERCENT,
        }
    }
}

// ── Hot corners & screen corners ─────────────────────────────────────────

/// What a hot corner does.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CornerAction {
    #[default]
    None,
    Launcher,
    /// The compositor's overview (Niri / Hyprland plugin / Mango).
    Overview,
    Sidebar,
    /// Power / session dock.
    Session,
    /// Toggle notifications sidebar tab / center.
    Notifications,
    /// Wallpaper picker.
    Wallpapers,
    /// Lock the session.
    Lock,
    /// Run `command` for this corner.
    Command,
}

/// One corner's binding.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Corner {
    pub action: CornerAction,
    /// Shell command for [`CornerAction::Command`] (run with `sh -c`).
    pub command: String,
}

impl Corner {
    pub fn is_active(&self) -> bool {
        match self.action {
            CornerAction::None => false,
            CornerAction::Command => !self.command.trim().is_empty(),
            _ => true,
        }
    }
}

/// Hot corners (`[hot_corners]`): rest the pointer in a screen corner.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotCornersConfig {
    pub enabled: bool,
    pub top_left: Corner,
    pub top_right: Corner,
    pub bottom_left: Corner,
    pub bottom_right: Corner,
    /// Dwell before a corner fires, ms (guards against fly-bys).
    pub delay_ms: u64,
    /// Hit square, logical px.
    pub size: f32,
    /// Also fire while a fullscreen window is shown on that display (games
    /// and videos push the pointer into corners, so this is off by default;
    /// MangoWM also needs the bar's "Show over fullscreen windows").
    pub over_fullscreen: bool,
}

impl Default for HotCornersConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            top_left: Corner { action: CornerAction::Overview, command: String::new() },
            top_right: Corner::default(),
            bottom_left: Corner::default(),
            bottom_right: Corner::default(),
            delay_ms: defaults::HOT_CORNER_DELAY_MS,
            size: defaults::HOT_CORNER_SIZE_PX,
            over_fullscreen: false,
        }
    }
}

/// Screen corners in order: top-left, top-right, bottom-left, bottom-right.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScreenCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl ScreenCorner {
    pub const ALL: [ScreenCorner; 4] = [ScreenCorner::TopLeft, ScreenCorner::TopRight, ScreenCorner::BottomLeft, ScreenCorner::BottomRight];

    /// Stable id (`top-left`, …) used by the command line.
    pub fn id(self) -> &'static str {
        match self {
            ScreenCorner::TopLeft => "top-left",
            ScreenCorner::TopRight => "top-right",
            ScreenCorner::BottomLeft => "bottom-left",
            ScreenCorner::BottomRight => "bottom-right",
        }
    }

    /// Parses [`ScreenCorner::id`] (also accepts `top_left` / `topleft`).
    pub fn parse(id: &str) -> Option<ScreenCorner> {
        let norm: String = id.trim().to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphabetic()).collect();
        ScreenCorner::ALL.into_iter().find(|c| c.id().replace('-', "") == norm)
    }

    /// Whether the corner is on the right / bottom edge.
    pub fn is_right(self) -> bool {
        matches!(self, ScreenCorner::TopRight | ScreenCorner::BottomRight)
    }

    pub fn is_bottom(self) -> bool {
        matches!(self, ScreenCorner::BottomLeft | ScreenCorner::BottomRight)
    }
}

impl HotCornersConfig {
    /// Whether any corner would fire: hot corners are on and at least one
    /// corner has an action. Mandara then also turns the compositor's own
    /// hot corner off (MangoWM's hot area, Niri's overview corner), which
    /// would otherwise fire alongside it — two overview toggles cancel out.
    pub fn any_active(&self) -> bool {
        self.enabled && ScreenCorner::ALL.iter().any(|c| self.corner(*c).is_active())
    }

    pub fn corner(&self, which: ScreenCorner) -> &Corner {
        match which {
            ScreenCorner::TopLeft => &self.top_left,
            ScreenCorner::TopRight => &self.top_right,
            ScreenCorner::BottomLeft => &self.bottom_left,
            ScreenCorner::BottomRight => &self.bottom_right,
        }
    }
}

/// Rounded black screen corners drawn over the desktop (`[screen_corners]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenCornersConfig {
    pub enabled: bool,
    pub radius: f32,
    pub top_left: bool,
    pub top_right: bool,
    pub bottom_left: bool,
    pub bottom_right: bool,
    /// Leave the shared edges of adjacent displays unobstructed.
    pub hide_between_displays: bool,
}

impl Default for ScreenCornersConfig {
    fn default() -> Self {
        Self { enabled: true, radius: defaults::SCREEN_CORNER_RADIUS_PX, top_left: true, top_right: true, bottom_left: true, bottom_right: true, hide_between_displays: false }
    }
}

impl ScreenCornersConfig {
    pub fn corner_enabled(&self, corner: ScreenCorner) -> bool {
        self.enabled && match corner {
            ScreenCorner::TopLeft => self.top_left,
            ScreenCorner::TopRight => self.top_right,
            ScreenCorner::BottomLeft => self.bottom_left,
            ScreenCorner::BottomRight => self.bottom_right,
        }
    }
}

// ── Interface style ──────────────────────────────────────────────────────

/// Shell-wide interface style (`[style]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StyleConfig {
    /// Multiplies corner radii of cards, chips, panels and the bar.
    pub radius_scale: f32,
    /// Soft shadows under panels, toasts and popups.
    pub shadows: bool,
    /// Hairline borders around panels and popups.
    pub borders: bool,
    /// Dims the desktop behind open panels / the launcher, 0–1.
    pub backdrop_dim: f32,
    /// The upright handle between every slider's fill and its track.
    pub slider_handle: SliderHandle,
}

impl Default for StyleConfig {
    fn default() -> Self {
        Self { radius_scale: 1.0, shadows: true, borders: true, backdrop_dim: 0.0, slider_handle: SliderHandle::Line }
    }
}

/// What marks the end of a thick slider's fill (`style.slider_handle`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SliderHandle {
    /// An upright line past the fill, a small gap on each side, and a stop
    /// dot at the end of the track.
    #[default]
    Line,
    /// Nothing: the fill's rounded end alone.
    None,
}

// ── Bar extras ───────────────────────────────────────────────────────────

/// When the bar hides itself.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutoHideMode {
    /// Always visible (reserves its space).
    #[default]
    Never,
    /// Hidden until the pointer touches its edge.
    Always,
    /// Hidden only while a window covers the bar's area (tiled or
    /// fullscreen on the active workspace / tags).
    Overlap,
    /// Hidden only while a fullscreen window is shown.
    Fullscreen,
}

/// Bar auto-hide (`[bar.auto_hide]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BarAutoHide {
    pub mode: AutoHideMode,
    /// Delay after the pointer leaves before the bar slides away, ms.
    pub delay_ms: u64,
    /// Show the bar briefly when the workspace / tag changes.
    pub reveal_on_workspace_switch: bool,
}

impl Default for BarAutoHide {
    fn default() -> Self {
        Self { mode: AutoHideMode::Never, delay_ms: defaults::BAR_HIDE_DELAY_MS, reveal_on_workspace_switch: true }
    }
}

// ── Wallpaper extras ─────────────────────────────────────────────────────

/// How a wallpaper fills its output.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WallpaperFill {
    /// Scale to cover the output, cropping the overflow.
    #[default]
    Cover,
    /// Scale to fit inside, bands in the fill colour.
    Contain,
    /// Stretch to the output's aspect.
    Stretch,
    /// Original size, centred.
    Center,
    /// Original size, repeated.
    Tile,
}

/// How one wallpaper replaces another.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WallpaperTransition {
    #[default]
    Fade,
    /// The new image sweeps in from the left (from the right in RTL).
    Wipe,
    /// The new image grows out of the centre.
    Grow,
    /// Cut without animation.
    None,
}

/// Slideshow order.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SlideshowOrder {
    /// Any other wallpaper each time (repeats can come soon).
    Random,
    /// Every wallpaper once in a shuffled order, then a new shuffle.
    #[default]
    Shuffle,
    /// By file name.
    Alphabetical,
    /// The most recently added or changed files first.
    Newest,
}

/// Which wallpapers the slideshow cycles through.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SlideshowSource {
    /// A folder: [`WallpaperSlideshow::directory`], or the wallpaper folder
    /// when that is empty.
    #[default]
    Folder,
    /// The wallpapers picked into [`WallpaperSlideshow::playlist`].
    Playlist,
}

/// How a slideshow change replaces the wallpaper.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SlideshowTransition {
    /// The desktop's own transition and length (`wallpaper.transition`).
    #[default]
    Desktop,
    Fade,
    Wipe,
    Grow,
    None,
}

impl SlideshowTransition {
    /// The transition to draw, or `None` to follow the desktop's.
    pub fn own(self) -> Option<WallpaperTransition> {
        match self {
            Self::Desktop => None,
            Self::Fade => Some(WallpaperTransition::Fade),
            Self::Wipe => Some(WallpaperTransition::Wipe),
            Self::Grow => Some(WallpaperTransition::Grow),
            Self::None => Some(WallpaperTransition::None),
        }
    }
}

/// Wallpaper slideshow (`[wallpaper.slideshow]`). Every field added after
/// 0.4 defaults to the behaviour before it, so older configs keep working.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WallpaperSlideshow {
    pub enabled: bool,
    /// Time between wallpapers ([`defaults::SLIDESHOW_INTERVAL_RANGE`]).
    pub interval_minutes: u32,
    pub order: SlideshowOrder,
    pub source: SlideshowSource,
    /// Folder to cycle through; empty uses the wallpaper folder.
    pub directory: String,
    /// Look into every sub-folder (else the folder and one level below,
    /// like the picker).
    pub subfolders: bool,
    /// Wallpapers picked for [`SlideshowSource::Playlist`], in order.
    pub playlist: Vec<PathBuf>,
    /// Folders used instead while the theme is light / dark (empty: the
    /// source above).
    pub light_directory: String,
    pub dark_directory: String,
    /// Show the next wallpaper each time the shell starts (login).
    pub on_start: bool,
    /// Don't change wallpapers while a fullscreen window is open.
    pub pause_on_fullscreen: bool,
    /// …while running on battery.
    pub pause_on_battery: bool,
    /// …while the session is locked or the screens are off.
    pub pause_when_locked: bool,
    /// Change only between [`Self::from`] and [`Self::until`] (`HH:MM`,
    /// local time; the window may cross midnight).
    pub hours: bool,
    pub from: String,
    pub until: String,
    pub transition: SlideshowTransition,
    /// Length of [`Self::transition`] when it isn't the desktop's, ms.
    pub transition_ms: u64,
    /// Every display gets its own wallpaper from the slideshow (else they
    /// all change to the same one).
    pub per_display: bool,
}

impl Default for WallpaperSlideshow {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_minutes: defaults::SLIDESHOW_MIN,
            order: SlideshowOrder::default(),
            source: SlideshowSource::default(),
            directory: String::new(),
            subfolders: false,
            playlist: Vec::new(),
            light_directory: String::new(),
            dark_directory: String::new(),
            on_start: false,
            pause_on_fullscreen: true,
            pause_on_battery: false,
            pause_when_locked: true,
            hours: false,
            from: defaults::SLIDESHOW_FROM.into(),
            until: defaults::SLIDESHOW_UNTIL.into(),
            transition: SlideshowTransition::default(),
            transition_ms: defaults::SLIDESHOW_TRANSITION_MS,
            per_display: false,
        }
    }
}

impl WallpaperSlideshow {
    /// [`Self::interval_minutes`] within its range.
    pub fn interval(&self) -> u32 {
        self.interval_minutes.clamp(defaults::SLIDESHOW_INTERVAL_RANGE.0, defaults::SLIDESHOW_INTERVAL_RANGE.1)
    }

    /// [`Self::transition_ms`] within its range.
    pub fn transition_len(&self) -> u64 {
        self.transition_ms.clamp(defaults::SLIDESHOW_TRANSITION_RANGE.0, defaults::SLIDESHOW_TRANSITION_RANGE.1)
    }

    /// The folder typed for the current appearance (light / dark folder,
    /// else the main one; empty = the wallpaper folder).
    pub fn folder_for(&self, dark: bool) -> &str {
        let themed = if dark { &self.dark_directory } else { &self.light_directory };
        if themed.trim().is_empty() { self.directory.trim() } else { themed.trim() }
    }

    /// Whether the slideshow may change at `minute` of the day (minutes
    /// since local midnight): always without [`Self::hours`], else inside
    /// the window (which may cross midnight; equal ends mean all day).
    pub fn in_hours(&self, minute: u32) -> bool {
        match (self.hours, parse_hhmm(&self.from), parse_hhmm(&self.until)) {
            (true, Some(from), Some(until)) => within(from, until, minute % MINUTES_PER_DAY),
            _ => true,
        }
    }

    /// Minutes from `minute` until the window opens (0 when inside it).
    pub fn minutes_until_open(&self, minute: u32) -> u32 {
        if self.in_hours(minute) {
            return 0;
        }
        let from = parse_hhmm(&self.from).unwrap_or(0);
        (from + MINUTES_PER_DAY - minute % MINUTES_PER_DAY) % MINUTES_PER_DAY
    }
}

/// Minutes in a day.
const MINUTES_PER_DAY: u32 = 24 * 60;

/// `HH:MM` → minutes since midnight.
pub fn parse_hhmm(text: &str) -> Option<u32> {
    let (h, m) = text.trim().split_once(':')?;
    let (h, m) = (h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Whether `minute` is in `[from, until)`, wrapping past midnight.
fn within(from: u32, until: u32, minute: u32) -> bool {
    match from.cmp(&until) {
        std::cmp::Ordering::Equal => true,
        std::cmp::Ordering::Less => (from..until).contains(&minute),
        std::cmp::Ordering::Greater => minute >= from || minute < until,
    }
}

/// A slideshow interval typed by a person: `45` (minutes), `45m`, `2h`,
/// `1h30m`, `1d`. `None` for anything else or zero.
pub fn parse_interval(text: &str) -> Option<u32> {
    let t = text.trim().to_ascii_lowercase();
    if let Ok(m) = t.parse::<u32>() {
        return (m > 0).then_some(m);
    }
    let (mut total, mut digits) = (0u32, String::new());
    for c in t.chars().filter(|c| !c.is_whitespace()) {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        let n: u32 = digits.parse().ok()?;
        digits.clear();
        total = total.checked_add(match c {
            'm' => n,
            'h' => n.checked_mul(60)?,
            'd' => n.checked_mul(MINUTES_PER_DAY)?,
            _ => return None,
        })?;
    }
    (digits.is_empty() && total > 0).then_some(total)
}

/// When a video wallpaper stops for windows in front of it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VideoPause {
    /// A tiled or fullscreen window covers the output's visible workspace.
    #[default]
    Covered,
    /// Only a fullscreen window does.
    Fullscreen,
    /// Keep playing behind windows (still stops while locked, with the
    /// screens off, and whenever the compositor stops drawing the desktop).
    Never,
}

/// Video wallpapers (`[wallpaper.video]`): AV1 `.webm`, `.mkv` and `.ivf`
/// clips play as the desktop background, looping.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WallpaperVideo {
    /// Stop playing (keep the current frame) while on battery.
    pub pause_on_battery: bool,
    /// Stop playing while windows cover the desktop.
    pub pause_when: VideoPause,
    /// Highest frame rate drawn ([`defaults::VIDEO_FPS_RANGE`]).
    pub max_fps: u32,
    /// Show the clip's first frame only: no decoding, a still's cost.
    pub still_frame: bool,
}

impl Default for WallpaperVideo {
    fn default() -> Self {
        Self { pause_on_battery: true, pause_when: VideoPause::Covered, max_fps: defaults::VIDEO_MAX_FPS, still_frame: false }
    }
}

impl WallpaperVideo {
    /// [`Self::max_fps`] within its range.
    pub fn fps_cap(&self) -> u32 {
        self.max_fps.clamp(defaults::VIDEO_FPS_RANGE.0, defaults::VIDEO_FPS_RANGE.1)
    }
}

/// Wallpaper chosen per output and per appearance.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WallpaperVariants {
    /// Used while the theme is light / dark (falls back to `path`).
    pub light: Option<PathBuf>,
    pub dark: Option<PathBuf>,
    /// Per-output overrides, keyed by connector name (`DP-1`, `eDP-1`).
    pub monitors: BTreeMap<String, PathBuf>,
}

/// The wallpaper output `monitor` shows: its own override, then the
/// light / dark variant for the current appearance, then the main one.
/// Paths typed in Studio may start with `~/`; they come back expanded.
pub fn effective_wallpaper(path: Option<&PathBuf>, variants: &WallpaperVariants, monitor: Option<&str>, dark: bool) -> Option<PathBuf> {
    let themed = if dark { variants.dark.as_ref() } else { variants.light.as_ref() };
    let chosen = monitor.and_then(|m| variants.monitors.get(m)).or(themed.filter(|p| !p.as_os_str().is_empty())).or(path)?;
    Some(crate::paths::expand_tilde(&chosen.to_string_lossy()))
}

/// Next slideshow image after `current` in `images` (sorted): the
/// following one alphabetically, or a pseudo-random different one using
/// `seed`.
pub fn next_slide(images: &[PathBuf], current: Option<&PathBuf>, order: SlideshowOrder, seed: u64) -> Option<PathBuf> {
    if images.is_empty() {
        return None;
    }
    let at = current.and_then(|c| images.iter().position(|p| p == c));
    let next = match order {
        // `images` comes newest first for [`SlideshowOrder::Newest`].
        SlideshowOrder::Alphabetical | SlideshowOrder::Newest => at.map_or(0, |i| (i + 1) % images.len()),
        // A shuffle without its [`ShuffleDeck`] picks like Random.
        SlideshowOrder::Random | SlideshowOrder::Shuffle => {
            if images.len() == 1 {
                0
            } else {
                // Any image but the current one, uniformly from the seed.
                let pick = usize::try_from(seed % (images.len() as u64 - 1)).unwrap_or(0);
                match at {
                    Some(i) if pick >= i => pick + 1,
                    _ => pick,
                }
            }
        }
    };
    images.get(next).cloned()
}

/// [`SlideshowOrder::Shuffle`]: every wallpaper once in a shuffled order,
/// then a fresh shuffle (never starting with the one on screen). A change
/// to the set of wallpapers starts a new round.
#[derive(Clone, Debug, Default)]
pub struct ShuffleDeck {
    /// Still to show this round; drawn from the end.
    left: Vec<PathBuf>,
    /// Fingerprint of the wallpapers the round was dealt from.
    of: u64,
}

impl ShuffleDeck {
    /// Wallpapers left in this round.
    pub fn remaining(&self) -> usize {
        self.left.len()
    }

    /// The next wallpaper of the round (`seed` shuffles a new one).
    pub fn draw(&mut self, images: &[PathBuf], current: Option<&PathBuf>, seed: u64) -> Option<PathBuf> {
        use std::hash::{Hash, Hasher};
        if images.is_empty() {
            self.left.clear();
            return None;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        images.hash(&mut h);
        let of = h.finish();
        if of != self.of || self.left.is_empty() {
            self.of = of;
            self.left = images.to_vec();
            // Fisher–Yates with a xorshift stream from `seed`.
            let mut state = seed | 1;
            for i in (1..self.left.len()).rev() {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let j = usize::try_from(state % (i as u64 + 1)).unwrap_or(0);
                self.left.swap(i, j);
            }
            // A new round never opens with the wallpaper already shown.
            if self.left.len() > 1 && self.left.last() == current {
                let last = self.left.len() - 1;
                self.left.swap(0, last);
            }
        }
        self.left.pop()
    }
}

/// Wallpapers newest first (by modification time, then name), for
/// [`SlideshowOrder::Newest`].
pub fn newest_first(images: &mut [PathBuf]) {
    images.sort_by_cached_key(|p| (std::cmp::Reverse(std::fs::metadata(p).and_then(|m| m.modified()).ok()), p.file_name().map(|n| n.to_string_lossy().to_lowercase())));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_corners_are_rounded_by_default_and_a_saved_choice_stays() {
        let fresh: crate::ShellConfig = toml::from_str("").unwrap();
        assert!(fresh.screen_corners.enabled, "a new config rounds the screen corners");
        let saved: crate::ShellConfig = toml::from_str("[screen_corners]\nenabled = false").unwrap();
        assert!(!saved.screen_corners.enabled, "turning them off is kept");
        assert!(ScreenCorner::ALL.into_iter().all(|corner| fresh.screen_corners.corner_enabled(corner)));
        let saved: crate::ShellConfig = toml::from_str("[screen_corners]\ntop_right = false\nbottom_left = false\nhide_between_displays = true").unwrap();
        assert!(saved.screen_corners.corner_enabled(ScreenCorner::TopLeft));
        assert!(!saved.screen_corners.corner_enabled(ScreenCorner::TopRight));
        assert!(!saved.screen_corners.corner_enabled(ScreenCorner::BottomLeft));
        assert!(saved.screen_corners.corner_enabled(ScreenCorner::BottomRight));
        assert!(saved.screen_corners.hide_between_displays);
    }

    #[test]
    fn automatic_time_patterns() {
        let d = DateTimeConfig::default();
        assert_eq!(d.bar_time_pattern(true, false), "%H:%M");
        assert_eq!(d.bar_time_pattern(true, true), "%H:%M:%S");
        assert_eq!(d.bar_time_pattern(false, false), "%-I:%M %p");
        let zero = DateTimeConfig { leading_zero_12h: true, ..Default::default() };
        assert_eq!(zero.bar_time_pattern(false, true), "%I:%M:%S %p");
        let custom = DateTimeConfig { bar_time_format: "%H.%M".into(), ..Default::default() };
        assert_eq!(custom.bar_time_pattern(false, true), "%H.%M");
        assert_eq!(d.lock_time_pattern(false), "%-I:%M");
    }

    #[test]
    fn weekend_days_follow_the_choice_and_the_region() {
        let on = |w: Weekend, sat: bool, ar: bool| w.days_from_monday(sat, ar).iter().enumerate().filter(|(_, d)| **d).map(|(i, _)| i).collect::<Vec<_>>();
        assert_eq!(on(Weekend::Auto, false, false), [5, 6], "Saturday and Sunday by default");
        assert_eq!(on(Weekend::Auto, true, false), [4, 5], "a Saturday-first week rests on Friday and Saturday");
        assert_eq!(on(Weekend::Auto, false, true), [4, 5], "so does an Arabic interface");
        assert_eq!(on(Weekend::SaturdaySunday, true, true), [5, 6], "an explicit choice wins");
        assert_eq!(on(Weekend::Friday, false, false), [4]);
        assert_eq!(on(Weekend::Sunday, false, false), [6]);
        let d = DateTimeConfig::default();
        assert!(!d.hijri_dates && !d.eastern_numerals && d.weekend == Weekend::Auto, "existing setups look as before");
        let parsed: DateTimeConfig = toml::from_str("weekend = \"friday-saturday\"\nhijri_dates = true").unwrap();
        assert_eq!(parsed.weekend, Weekend::FridaySaturday);
        assert!(parsed.hijri_dates);
    }

    #[test]
    fn wallpaper_resolution_order() {
        let main = PathBuf::from("/w/main.jpg");
        let mut v = WallpaperVariants { dark: Some("/w/dark.jpg".into()), ..Default::default() };
        assert_eq!(effective_wallpaper(Some(&main), &v, Some("DP-1"), true), Some("/w/dark.jpg".into()));
        assert_eq!(effective_wallpaper(Some(&main), &v, Some("DP-1"), false), Some(main.clone()), "no light variant → main");
        v.monitors.insert("DP-1".into(), "/w/side.png".into());
        assert_eq!(effective_wallpaper(Some(&main), &v, Some("DP-1"), true), Some("/w/side.png".into()));
        assert_eq!(effective_wallpaper(None, &WallpaperVariants::default(), None, true), None);
    }

    #[test]
    fn typed_wallpaper_paths_expand_the_home_folder() {
        let home = crate::paths::expand_tilde("~");
        let v = WallpaperVariants { light: Some("~/Pictures/day.jpg".into()), dark: Some("".into()), ..Default::default() };
        let main = PathBuf::from("/w/main.jpg");
        assert_eq!(effective_wallpaper(Some(&main), &v, None, false), Some(home.join("Pictures/day.jpg")));
        assert_eq!(effective_wallpaper(Some(&main), &v, None, true), Some(main.clone()), "an empty variant falls back to the main wallpaper");
        let mut per_output = WallpaperVariants::default();
        per_output.monitors.insert("HDMI-A-1".into(), "~/side.png".into());
        assert_eq!(effective_wallpaper(Some(&main), &per_output, Some("HDMI-A-1"), true), Some(home.join("side.png")));
        assert_eq!(effective_wallpaper(Some(&main), &per_output, Some("eDP-1"), true), Some(main));
    }

    #[test]
    fn slideshow_never_repeats_the_current_image() {
        let imgs: Vec<PathBuf> = ["a", "b", "c", "d"].iter().map(PathBuf::from).collect();
        assert_eq!(next_slide(&imgs, Some(&imgs[3]), SlideshowOrder::Alphabetical, 0), Some(imgs[0].clone()));
        assert_eq!(next_slide(&imgs, None, SlideshowOrder::Alphabetical, 0), Some(imgs[0].clone()));
        for seed in 0..50 {
            let n = next_slide(&imgs, Some(&imgs[1]), SlideshowOrder::Random, seed).unwrap();
            assert_ne!(n, imgs[1]);
        }
        assert_eq!(next_slide(&imgs[..1], Some(&imgs[0]), SlideshowOrder::Random, 7), Some(imgs[0].clone()));
        assert_eq!(next_slide(&[], None, SlideshowOrder::Random, 7), None);
    }

    #[test]
    fn shuffle_shows_every_wallpaper_once_per_round() {
        let imgs: Vec<PathBuf> = ["a", "b", "c", "d", "e"].iter().map(PathBuf::from).collect();
        for seed in 0..20 {
            let mut deck = ShuffleDeck::default();
            let mut current = Some(imgs[2].clone());
            let mut seen = Vec::new();
            for _ in 0..imgs.len() {
                let next = deck.draw(&imgs, current.as_ref(), seed).unwrap();
                assert!(!seen.contains(&next), "seed {seed}: {next:?} twice in one round");
                seen.push(next.clone());
                current = Some(next);
            }
            assert_ne!(seen[0], imgs[2], "a round never opens with the wallpaper on screen");
            assert_eq!(deck.remaining(), 0);
            // The next round deals again and doesn't repeat the last one.
            assert_ne!(deck.draw(&imgs, current.as_ref(), seed + 1), current);
        }
        // A changed folder starts a new round from it.
        let mut deck = ShuffleDeck::default();
        deck.draw(&imgs, None, 3);
        let fewer = &imgs[..2];
        assert!(fewer.contains(&deck.draw(fewer, None, 3).unwrap()));
        assert_eq!(deck.draw(&[], None, 3), None);
    }

    #[test]
    fn slideshow_hours_and_intervals() {
        let mut s = WallpaperSlideshow::default();
        assert!(s.in_hours(3 * 60), "no window: always");
        s.hours = true;
        assert!(s.in_hours(8 * 60) && s.in_hours(21 * 60 + 59) && !s.in_hours(22 * 60) && !s.in_hours(7 * 60));
        assert_eq!(s.minutes_until_open(7 * 60 + 30), 30);
        assert_eq!(s.minutes_until_open(23 * 60), 9 * 60);
        assert_eq!(s.minutes_until_open(9 * 60), 0);
        // A window across midnight.
        s.from = "22:00".into();
        s.until = "06:30".into();
        assert!(s.in_hours(23 * 60) && s.in_hours(60) && !s.in_hours(12 * 60));
        // Unreadable times never stop the slideshow.
        s.from = "late".into();
        assert!(s.in_hours(12 * 60));
        assert_eq!(parse_interval("45"), Some(45));
        assert_eq!(parse_interval("2h"), Some(120));
        assert_eq!(parse_interval("1h 30m"), Some(90));
        assert_eq!(parse_interval("1d"), Some(1440));
        assert_eq!(parse_interval("0"), None);
        assert_eq!(parse_interval("soon"), None);
        assert_eq!(parse_interval("5"), Some(5));
        assert_eq!(parse_interval("3x"), None);
        assert_eq!(WallpaperSlideshow { interval_minutes: 0, ..Default::default() }.interval(), defaults::SLIDESHOW_INTERVAL_RANGE.0);
    }

    #[test]
    fn slideshow_folders_follow_the_appearance() {
        let s = WallpaperSlideshow { directory: "~/W".into(), dark_directory: "~/Night".into(), ..Default::default() };
        assert_eq!(s.folder_for(true), "~/Night");
        assert_eq!(s.folder_for(false), "~/W", "no light folder: the main one");
        assert_eq!(WallpaperSlideshow::default().folder_for(true), "", "empty: the wallpaper folder");
    }

    /// A `[wallpaper.slideshow]` table as 0.4 wrote it keeps its meaning:
    /// the same folder, order, interval and pause, and nothing new turned on.
    #[test]
    fn slideshow_tables_from_before_keep_working() {
        let old = "enabled = true\ninterval_minutes = 45\norder = \"random\"\ndirectory = \"~/Pictures/Walls\"\npause_on_fullscreen = false\n";
        let s: WallpaperSlideshow = toml::from_str(old).unwrap();
        assert!(s.enabled && !s.pause_on_fullscreen);
        assert_eq!((s.interval(), s.order, s.source, s.folder_for(true)), (45, SlideshowOrder::Random, SlideshowSource::Folder, "~/Pictures/Walls"));
        assert!(!s.on_start && !s.per_display && !s.hours && !s.subfolders && !s.pause_on_battery);
        assert_eq!(s.transition.own(), None, "changes keep the desktop's transition");
        let alpha: WallpaperSlideshow = toml::from_str("order = \"alphabetical\"").unwrap();
        assert_eq!(alpha.order, SlideshowOrder::Alphabetical);
    }

    #[test]
    fn video_wallpaper_settings_default_and_clamp() {
        let v = WallpaperVideo::default();
        assert!(v.pause_on_battery && !v.still_frame);
        assert_eq!(v.pause_when, VideoPause::Covered);
        assert_eq!(v.fps_cap(), defaults::VIDEO_MAX_FPS);
        assert_eq!(WallpaperVideo { max_fps: 0, ..v.clone() }.fps_cap(), defaults::VIDEO_FPS_RANGE.0);
        assert_eq!(WallpaperVideo { max_fps: 500, ..v }.fps_cap(), defaults::VIDEO_FPS_RANGE.1);
        // Older configs without the table load with the defaults.
        let w: crate::WallpaperConfig = toml::from_str("enabled = true").unwrap();
        assert_eq!(w.video, WallpaperVideo::default());
        let w: crate::WallpaperConfig = toml::from_str("[video]\npause_when = \"fullscreen\"\nmax_fps = 24").unwrap();
        assert_eq!((w.video.pause_when, w.video.max_fps, w.video.pause_on_battery), (VideoPause::Fullscreen, 24, true));
    }

    #[test]
    fn wind_units_convert() {
        let (v, u) = WindUnit::MetersPerSecond.convert(36.0).unwrap();
        assert!((v - 10.0).abs() < 1e-4 && u == "m/s");
        assert!(WindUnit::Auto.convert(10.0).is_none());
    }

    #[test]
    fn corners_need_an_action() {
        let c = HotCornersConfig::default();
        assert!(c.corner(ScreenCorner::TopLeft).is_active());
        assert!(!c.corner(ScreenCorner::BottomRight).is_active());
        assert!(!Corner { action: CornerAction::Command, command: " ".into() }.is_active());
    }

    #[test]
    fn any_active_needs_the_switch_and_an_action() {
        let mut c = HotCornersConfig::default();
        assert!(!c.any_active(), "off by default");
        c.enabled = true;
        assert!(c.any_active(), "the default top-left overview corner");
        c.top_left.action = CornerAction::None;
        assert!(!c.any_active());
        c.bottom_right = Corner { action: CornerAction::Command, command: "true".into() };
        assert!(c.any_active());
    }

    #[test]
    fn corner_ids_round_trip() {
        for c in ScreenCorner::ALL {
            assert_eq!(ScreenCorner::parse(c.id()), Some(c));
        }
        assert_eq!(ScreenCorner::parse("Top_Right"), Some(ScreenCorner::TopRight));
        assert_eq!(ScreenCorner::parse("middle"), None);
        assert!(ScreenCorner::BottomRight.is_right() && ScreenCorner::BottomRight.is_bottom());
        assert!(!ScreenCorner::TopLeft.is_right() && !ScreenCorner::TopLeft.is_bottom());
    }

    #[test]
    fn hot_corners_parse_without_new_keys() {
        // Configs written before `over_fullscreen` existed keep loading.
        let c: HotCornersConfig = toml::from_str("enabled = true\ndelay_ms = 200\n[top_left]\naction = \"launcher\"\n").unwrap();
        assert!(c.enabled && !c.over_fullscreen);
        assert_eq!(c.delay_ms, 200);
        assert_eq!(c.top_left.action, CornerAction::Launcher);
    }
}
