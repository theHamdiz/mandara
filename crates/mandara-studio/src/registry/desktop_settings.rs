//! Registry entries for the bar's behaviour, wallpaper variants and
//! slideshow, interface style, date / time / calendar / location /
//! weather, the lock screen's look, and hot / screen corners.

use mandara_config::desktop::defaults::VIDEO_FPS_RANGE;
use mandara_i18n::tk;

use super::{Control, Section, Setting, s};

const AUTO_HIDE: &[(&str, &str)] = &[("never", tk("Never")), ("always", tk("Always")), ("overlap", tk("When a window covers it")), ("fullscreen", tk("Fullscreen only"))];
const FILL: &[(&str, &str)] = &[("cover", tk("Fill")), ("contain", tk("Fit")), ("stretch", tk("Stretch")), ("center", tk("Center")), ("tile", tk("Tile"))];
const TRANSITION: &[(&str, &str)] = &[("fade", tk("Fade")), ("wipe", tk("Wipe")), ("grow", tk("Grow")), ("none", tk("None"))];
const ORDER: &[(&str, &str)] = &[("random", tk("Random")), ("alphabetical", tk("Alphabetical"))];
const VIDEO_PAUSE: &[(&str, &str)] = &[("covered", tk("When windows cover it")), ("fullscreen", tk("Fullscreen only")), ("never", tk("Never"))];
const LOCATION_MODE: &[(&str, &str)] = &[("manual", tk("Manual")), ("geoclue", tk("Location service (GeoClue)")), ("ip", tk("IP address lookup"))];
const WIND: &[(&str, &str)] = &[("auto", tk("Follow units")), ("meters-per-second", "m/s"), ("knots", tk("Knots"))];
const LOCK_CLOCK: &[(&str, &str)] = &[("classic", tk("Classic")), ("stacked", tk("Stacked")), ("compact", tk("Compact"))];
const LOCK_BG: &[(&str, &str)] = &[("wallpaper", tk("Wallpaper")), ("image", tk("Image")), ("color", tk("Solid color"))];
const CORNER: &[(&str, &str)] = &[
    ("none", tk("Nothing")),
    ("launcher", tk("Launcher")),
    ("overview", tk("Overview")),
    ("sidebar", tk("Sidebar")),
    ("session", tk("Power menu")),
    ("notifications", tk("Notifications")),
    ("wallpapers", tk("Wallpapers")),
    ("lock", tk("Lock screen")),
    ("command", tk("Command")),
];
const WEIGHTS: &[(&str, &str)] = &[("300", tk("Light")), ("400", tk("Regular")), ("500", tk("Medium")), ("600", tk("Semibold"))];

/// Every W5 setting, appended to the registry.
pub(super) fn entries() -> Vec<Setting> {
    use Control::*;
    use Section::*;
    let slider = |min: f64, max: f64, step: f64, unit: &'static str| Slider { min, max, step, unit };
    vec![
        // Bar behaviour
        s(Bar, tk("Behaviour"), "/bar/auto_hide/mode", tk("Auto-hide"), tk("Slide the bar away; touch its edge to bring it back"), Choice(AUTO_HIDE)),
        s(Bar, tk("Behaviour"), "/bar/auto_hide/delay_ms", tk("Hide delay"), tk("How long the bar stays after the pointer leaves"), slider(0.0, 3000.0, 50.0, " ms")),
        s(Bar, tk("Behaviour"), "/bar/auto_hide/reveal_on_workspace_switch", tk("Peek on workspace switch"), tk("Show the hidden bar briefly when you change workspace or tag"), Toggle),
        s(Bar, tk("Behaviour"), "/bar/click_through", tk("Click through empty space"), tk("Clicks between bar items reach the window below (no main menu on right-click there)"), Toggle),
        s(Bar, tk("Behaviour"), "/bar/over_fullscreen", tk("Show over fullscreen windows"), tk("Keeps the bar and its panels above fullscreen apps; applies after a shell restart"), Toggle),
        // Wallpaper
        s(Wallpaper, tk("Desktop"), "/wallpaper/fill", tk("Fill mode"), tk("How images that don't match the screen are placed"), Choice(FILL)),
        s(Wallpaper, tk("Desktop"), "/wallpaper/fill_color", tk("Fill color"), tk("Behind Fit and Center (hex)"), Text { placeholder: "#000000" }),
        s(Wallpaper, tk("Desktop"), "/wallpaper/transition", tk("Transition"), tk("How a new wallpaper replaces the old one"), Choice(TRANSITION)),
        s(Wallpaper, tk("Light & dark"), "/wallpaper/variants/light", tk("Light wallpaper"), tk("Shown while the theme is light; empty uses the main wallpaper"), OptionalText { placeholder: "~/Pictures/Wallpapers/day.jpg" }),
        s(Wallpaper, tk("Light & dark"), "/wallpaper/variants/dark", tk("Dark wallpaper"), tk("Shown while the theme is dark; per-display wallpapers: mandara msg wallpaper output"), OptionalText { placeholder: "~/Pictures/Wallpapers/night.jpg" }),
        s(Wallpaper, tk("Slideshow"), "/wallpaper/slideshow/enabled", tk("Slideshow"), tk("Change the wallpaper on a timer"), Toggle),
        s(Wallpaper, tk("Slideshow"), "/wallpaper/slideshow/interval_minutes", tk("Every"), tk("Time between wallpapers"), slider(1.0, 240.0, 1.0, " min")),
        s(Wallpaper, tk("Slideshow"), "/wallpaper/slideshow/order", tk("Order"), tk("Random never repeats the current image"), Choice(ORDER)),
        s(Wallpaper, tk("Slideshow"), "/wallpaper/slideshow/directory", tk("Folder"), tk("Empty uses the picker folder"), Text { placeholder: "~/Pictures/Wallpapers" }),
        s(Wallpaper, tk("Slideshow"), "/wallpaper/slideshow/pause_on_fullscreen", tk("Pause during fullscreen"), tk("Keep the wallpaper while a fullscreen app is open"), Toggle),
        s(Wallpaper, tk("Slideshow"), "/wallpaper/slideshow/enabled", tk("Next wallpaper"), tk("Skip to the next image now"), Command { button: tk("Next"), args: &["wallpaper", "next"] }),
        // Video wallpapers (AV1 .webm / .mkv / .ivf)
        s(Wallpaper, tk("Video"), "/wallpaper/video/pause_when", tk("Pause behind windows"), tk("A video wallpaper keeps its current frame while windows hide the desktop"), Choice(VIDEO_PAUSE)),
        s(Wallpaper, tk("Video"), "/wallpaper/video/pause_on_battery", tk("Pause on battery"), tk("Keep the current frame while running on battery"), Toggle),
        s(Wallpaper, tk("Video"), "/wallpaper/video/max_fps", tk("Frame rate limit"), tk("Highest frame rate a video wallpaper plays at; lower uses less power"), slider(f64::from(VIDEO_FPS_RANGE.0), f64::from(VIDEO_FPS_RANGE.1), 1.0, " fps")),
        s(Wallpaper, tk("Video"), "/wallpaper/video/still_frame", tk("First frame only"), tk("Show video wallpapers as a still image: no decoding, no extra power"), Toggle),
        // Interface style
        s(Themes, tk("Style"), "/style/radius_scale", tk("Corner roundness"), tk("Scales the radii of cards and tiles"), slider(0.0, 2.0, 0.05, "×")),
        s(Themes, tk("Style"), "/style/shadows", tk("Shadows"), tk("Soft elevation under panels and popups"), Toggle),
        s(Themes, tk("Style"), "/style/borders", tk("Panel edges"), tk("A hairline around floating panels"), Toggle),
        s(Themes, tk("Style"), "/style/backdrop_dim", tk("Dim behind panels"), tk("Darkens the desktop while a panel or the sidebar is open"), slider(0.0, 0.6, 0.02, "")),
        s(Themes, tk("Accessibility"), "/theme/adjustments/high_contrast", tk("High contrast"), tk("Pushes text and surfaces apart to WCAG AAA (7:1)"), Toggle),
        s(Typography, tk("Size"), "/typography/weight", tk("Text weight"), tk("Weight of body text across the shell"), Choice(WEIGHTS)),
        s(Motion, tk("Springs"), "/motion/speed", tk("Animation speed"), tk("Plays every animation faster or slower with the same feel"), slider(0.25, 3.0, 0.05, "×")),
        // Date & time
        s(DateTime, tk("Clock"), "/general/clock_24h", tk("24-hour clock"), tk("Off shows AM / PM"), Toggle),
        s(DateTime, tk("Clock"), "/datetime/leading_zero_12h", tk("Leading zero"), tk("12-hour clock: 09:05 instead of 9:05"), Toggle),
        s(DateTime, tk("Clock"), "/bar/show_seconds", tk("Show seconds"), tk("Seconds in the bar clock"), Toggle),
        s(DateTime, tk("Clock"), "/datetime/bar_show_date", tk("Date in the bar"), tk("The date next to the time"), Toggle),
        s(DateTime, tk("Formats"), "/datetime/bar_time_format", tk("Bar time format"), tk("strftime pattern, e.g. %H:%M or %-I:%M %p; empty follows the switches above"), Text { placeholder: "%H:%M" }),
        s(DateTime, tk("Formats"), "/datetime/bar_date_format", tk("Bar date format"), tk("e.g. %a %-d %b · %d/%m/%Y · %A"), Text { placeholder: "%a %-d %b" }),
        s(DateTime, tk("Formats"), "/datetime/lock_time_format", tk("Lock screen time"), tk("strftime pattern; empty follows the 24-hour switch"), Text { placeholder: "%H:%M" }),
        s(DateTime, tk("Formats"), "/datetime/lock_date_format", tk("Lock screen date"), tk("strftime pattern; empty uses the long date"), Text { placeholder: "%A, %-d %B" }),
        s(DateTime, tk("Formats"), "/datetime/time_locale", tk("Date language"), tk("Month and weekday names: auto follows the interface, or a locale such as en_GB, ar_EG, de_DE"), Text { placeholder: "auto" }),
        s(DateTime, tk("Calendar"), "/general/week_start", tk("Week starts on"), tk("First day in calendars"), Choice(super::WEEK)),
        s(DateTime, tk("Calendar"), "/datetime/week_numbers", tk("Week numbers"), tk("ISO week numbers beside each calendar row"), Toggle),
        s(DateTime, tk("Calendar"), "/calendar/enabled", tk("Show events"), tk("Events from your calendars in the clock panel. Add one with mandara msg calendar add <url or .ics> [name]"), Toggle),
        s(DateTime, tk("Calendar"), "/calendar/agenda_days", tk("Agenda length"), tk("Upcoming days listed under the month"), slider(1.0, 31.0, 1.0, " d")),
        s(DateTime, tk("Calendar"), "/calendar/hide_past", tk("Hide finished events"), tk("Today's events disappear once they end"), Toggle),
        s(DateTime, tk("Calendar"), "/calendar/refresh_minutes", tk("Refresh every"), tk("How often subscriptions are downloaded again"), slider(5.0, 720.0, 5.0, " min")),
        s(DateTime, tk("Calendar"), "/calendar/enabled", tk("Refresh calendars"), tk("Download every subscription again now"), Command { button: tk("Refresh"), args: &["calendar", "refresh"] }),
        s(DateTime, tk("Reminders"), "/calendar/reminders", tk("Event reminders"), tk("A notification before each event"), Toggle),
        s(DateTime, tk("Reminders"), "/calendar/reminder_minutes", tk("Remind me"), tk("Minutes before events without their own alarm"), slider(0.0, 120.0, 5.0, " min")),
        s(DateTime, tk("Reminders"), "/calendar/honour_event_alarms", tk("Use the events' own alarms"), tk("When an event defines alarms, remind at those times instead"), Toggle),
        // Location
        s(DateTime, tk("Location"), "/location/shared", tk("Use one location everywhere"), tk("Weather and prayer times always follow this location, even when they have their own"), Toggle),
        s(DateTime, tk("Location"), "/location/mode", tk("Find my location with"), tk("Nothing is looked up until you press Locate me (the location service also refreshes at startup)"), Choice(LOCATION_MODE)),
        s(DateTime, tk("Location"), "/location/mode", tk("Locate me"), tk("One lookup with the method above; IP lookups send your address to ipapi.co"), Command { button: tk("Locate me"), args: &["location", "locate"] }),
        s(DateTime, tk("Location"), "/location/name", tk("Place"), tk("Shown in widgets; Find looks its coordinates up (Open-Meteo)"), Text { placeholder: "El-Mandara" }),
        s(DateTime, tk("Location"), "/location/name", tk("Find coordinates"), tk("Look up the place name above"), Command { button: tk("Find"), args: &["location", "geocode"] }),
        s(DateTime, tk("Location"), "/location/latitude", tk("Latitude"), tk("Degrees north"), Number { min: -90.0, max: 90.0 }),
        s(DateTime, tk("Location"), "/location/longitude", tk("Longitude"), tk("Degrees east"), Number { min: -180.0, max: 180.0 }),
        // Weather
        s(DateTime, tk("Weather"), "/weather/enabled", tk("Weather"), tk("Fetch forecasts from Open-Meteo (off: no weather requests at all)"), Toggle),
        s(DateTime, tk("Weather"), "/weather/refresh_minutes", tk("Refresh every"), tk("Time between forecast updates"), slider(5.0, 180.0, 5.0, " min")),
        s(DateTime, tk("Weather"), "/weather/wind_unit", tk("Wind speed"), tk("Override the unit for wind"), Choice(WIND)),
        s(DateTime, tk("Weather"), "/weather/show_high_low_in_bar", tk("High / low in the bar"), tk("Today's range next to the temperature"), Toggle),
        s(DateTime, tk("Weather"), "/weather/show_location_in_bar", tk("Place in the bar"), tk("The place name next to the temperature"), Toggle),
        // Lock screen
        s(LockScreen, tk("Clock"), "/lock/clock_style", tk("Clock style"), tk("One light line, stacked hours over minutes, or a small clock"), Choice(LOCK_CLOCK)),
        s(LockScreen, tk("Clock"), "/lock/show_date", tk("Date"), tk("The date under the clock"), Toggle),
        s(LockScreen, tk("Background"), "/lock/background", tk("Background"), tk("Your wallpaper, an image of your choice or a solid color"), Choice(LOCK_BG)),
        s(LockScreen, tk("Background"), "/lock/image", tk("Image"), tk("Used with the Image background"), OptionalText { placeholder: "~/Pictures/lock.jpg" }),
        s(LockScreen, tk("Background"), "/lock/blur", tk("Blur"), tk("Softens the background"), slider(0.0, 64.0, 1.0, " px")),
        s(LockScreen, tk("Background"), "/lock/dim", tk("Dim"), tk("Darkens the background so text stays readable"), slider(0.0, 0.9, 0.05, "")),
        s(LockScreen, tk("Background"), "/lock/blank_other_outputs", tk("Blank other displays"), tk("Only the main display shows the lock screen; others stay black (OLED friendly)"), Toggle),
        s(LockScreen, tk("Show"), "/lock/show_avatar", tk("Profile picture"), tk("Your avatar above the password field"), Toggle),
        s(LockScreen, tk("Show"), "/lock/show_media", tk("Now playing"), tk("The track that is playing"), Toggle),
        s(LockScreen, tk("Show"), "/lock/show_notifications", tk("Notification count"), tk("How many notifications are waiting (never their content)"), Toggle),
        s(LockScreen, tk("Show"), "/lock/show_weather", tk("Weather"), tk("Current conditions under the date"), Toggle),
        s(LockScreen, tk("Show"), "/lock/show_battery", tk("Battery"), tk("Charge level on laptops"), Toggle),
        s(LockScreen, tk("Preview"), "/lock/clock_style", tk("Preview the lock screen"), tk("Shows it for a few seconds without locking"), Command { button: tk("Preview"), args: &["lock", "--demo"] }),
        // Hot corners & screen corners
        s(Sidebar, tk("Hot corners"), "/hot_corners/enabled", tk("Hot corners"), tk("Rest the pointer in a screen corner to run an action; replaces your compositor's own hot corner"), Toggle),
        s(Sidebar, tk("Hot corners"), "/hot_corners/top_left/action", tk("Top left"), "", Choice(CORNER)),
        s(Sidebar, tk("Hot corners"), "/hot_corners/top_right/action", tk("Top right"), "", Choice(CORNER)),
        s(Sidebar, tk("Hot corners"), "/hot_corners/bottom_left/action", tk("Bottom left"), "", Choice(CORNER)),
        s(Sidebar, tk("Hot corners"), "/hot_corners/bottom_right/action", tk("Bottom right"), "", Choice(CORNER)),
        s(Sidebar, tk("Hot corners"), "/hot_corners/top_left/command", tk("Top left command"), tk("Run with sh -c when the action is Command"), Text { placeholder: "ghostty" }),
        s(Sidebar, tk("Hot corners"), "/hot_corners/top_right/command", tk("Top right command"), tk("Run with sh -c when the action is Command"), Text { placeholder: "" }),
        s(Sidebar, tk("Hot corners"), "/hot_corners/bottom_left/command", tk("Bottom left command"), tk("Run with sh -c when the action is Command"), Text { placeholder: "" }),
        s(Sidebar, tk("Hot corners"), "/hot_corners/bottom_right/command", tk("Bottom right command"), tk("Run with sh -c when the action is Command"), Text { placeholder: "" }),
        s(Sidebar, tk("Hot corners"), "/hot_corners/delay_ms", tk("Delay"), tk("How long the pointer must rest in the corner"), slider(0.0, 1000.0, 10.0, " ms")),
        s(Sidebar, tk("Hot corners"), "/hot_corners/size", tk("Trigger size"), tk("How big the hot spot in each corner is"), slider(1.0, 24.0, 1.0, " px")),
        s(Sidebar, tk("Hot corners"), "/hot_corners/over_fullscreen", tk("Over fullscreen apps"), tk("Also fire while a fullscreen app is shown (on MangoWM the bar must show over fullscreen windows too)"), Toggle),
        s(Sidebar, tk("Screen corners"), "/screen_corners/enabled", tk("Rounded screen corners"), tk("Black rounded corners drawn over the desktop"), Toggle),
        s(Sidebar, tk("Screen corners"), "/screen_corners/radius", tk("Radius"), "", slider(4.0, 40.0, 1.0, " px")),
        // App themes (cross-app sync)
        s(Themes, tk("App themes"), "/integrations/kitty", "kitty", tk("~/.config/kitty/themes/mandara.conf (pick it with kitten themes or include it)"), Toggle),
        s(Themes, tk("App themes"), "/integrations/foot", "foot", tk("~/.cache/mandara/themes/foot.ini (include= it from foot.ini)"), Toggle),
        s(Themes, tk("App themes"), "/integrations/alacritty", "Alacritty", tk("~/.cache/mandara/themes/alacritty.toml (add it to general.import)"), Toggle),
        s(Themes, tk("App themes"), "/integrations/wezterm", "WezTerm", tk("A “Mandara” color scheme in ~/.config/wezterm/colors"), Toggle),
        s(Themes, tk("App themes"), "/integrations/helix", "Helix", tk("theme = \"mandara\""), Toggle),
        s(Themes, tk("App themes"), "/integrations/neovim", "Neovim", tk(":colorscheme mandara"), Toggle),
        s(Themes, tk("App themes"), "/integrations/vesktop", tk("Vesktop / Vencord"), tk("Enable “mandara” under Themes"), Toggle),
        s(Themes, tk("App themes"), "/integrations/kde", tk("KDE / Plasma"), tk("A “Mandara” color scheme in ~/.local/share/color-schemes"), Toggle),
        s(Themes, tk("App themes"), "/integrations/firefox", tk("Firefox / Zen"), tk("Pywal-style colors.json and userChrome.css in ~/.cache/mandara/themes/firefox (see the README)"), Toggle),
    ]
}
