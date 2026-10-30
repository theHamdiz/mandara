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
    /// Lock screen background blur radius and dim, logical px / 0–1.
    pub const LOCK_BLUR_PX: f32 = 28.0;
    pub const LOCK_DIM: f32 = 0.35;
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
}

impl Default for ScreenCornersConfig {
    fn default() -> Self {
        Self { enabled: false, radius: defaults::SCREEN_CORNER_RADIUS_PX }
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
}

impl Default for StyleConfig {
    fn default() -> Self {
        Self { radius_scale: 1.0, shadows: true, borders: true, backdrop_dim: 0.0 }
    }
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
    #[default]
    Random,
    Alphabetical,
}

/// Wallpaper slideshow (`[wallpaper.slideshow]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WallpaperSlideshow {
    pub enabled: bool,
    pub interval_minutes: u32,
    pub order: SlideshowOrder,
    /// Folder to cycle through; empty uses the picker folder.
    pub directory: String,
    /// Don't change wallpapers while a fullscreen window is open.
    pub pause_on_fullscreen: bool,
}

impl Default for WallpaperSlideshow {
    fn default() -> Self {
        Self { enabled: false, interval_minutes: defaults::SLIDESHOW_MIN, order: SlideshowOrder::Random, directory: String::new(), pause_on_fullscreen: true }
    }
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
        SlideshowOrder::Alphabetical => at.map_or(0, |i| (i + 1) % images.len()),
        SlideshowOrder::Random => {
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

#[cfg(test)]
mod tests {
    use super::*;

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
