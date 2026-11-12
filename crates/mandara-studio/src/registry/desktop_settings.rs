//! Registry entries for the bar's behaviour, wallpaper variants and
//! slideshow, interface style, date / time / calendar / location /
//! weather, the lock screen's look, and hot / screen corners.

use mandara_config::desktop::defaults::{SKETCH_DRAW_RANGE, SKETCH_LINE_RANGE, SLIDESHOW_INTERVAL_RANGE, SLIDESHOW_TRANSITION_RANGE, VIDEO_FPS_RANGE};
use mandara_i18n::tk;

use super::{Control, Section, Setting, s};

/// `bar.right_click`.
const MENU_TILE_SURFACE: &[(&str, &str)] = &[("flat", tk("Flat")), ("3d", tk("3D")), ("glass", tk("Glass"))];
const BAR_RIGHT_CLICK: &[(&str, &str)] = &[("main-menu", tk("The bar's main menu")), ("desktop-menu", tk("The desktop's menu"))];
/// `panels.menu_rail`: names beside the main menu's icons, or icons only.
const MENU_RAIL: &[(&str, &str)] = &[("compact", tk("Compact")), ("full", tk("Full"))];
/// `panels.menu_rail_height`: as tall as its buttons, or as the panel.
const MENU_RAIL_HEIGHT: &[(&str, &str)] = &[("hug", tk("Fit the buttons")), ("stretch", tk("Panel height"))];
/// `bar.scroll`.
const BAR_SCROLL: &[(&str, &str)] = &[("none", tk("Nothing")), ("workspaces", tk("Switch workspace")), ("volume", tk("Change volume"))];
const AUTO_HIDE: &[(&str, &str)] = &[("never", tk("Never")), ("always", tk("Always")), ("overlap", tk("When a window covers it")), ("fullscreen", tk("Fullscreen only"))];
/// `style.slider_handle`: the upright line between a slider's fill and its
/// track.
const SLIDER_HANDLE: &[(&str, &str)] = &[("line", tk("Line")), ("none", tk("None"))];
/// `style.slider_design`, in `SliderDesign::ALL`'s order (Themes →
/// Components draws each as a live preview).
pub(crate) const SLIDER_DESIGN: &[(&str, &str)] = &[("classic", tk("Classic")), ("continuous", tk("Continuous")), ("pill", tk("Pill")), ("wavy", tk("Wavy")), ("segmented", tk("Segmented"))];
/// The other component designs, each in its design's `ALL` order
/// (`mandara_config::components`).
pub(crate) const BUTTON_DESIGN: &[(&str, &str)] = &[("classic", tk("Classic")), ("square", tk("Square")), ("expressive", tk("Expressive")), ("cut", tk("Cut")), ("elevated", tk("Elevated"))];
pub(crate) const TOGGLE_DESIGN: &[(&str, &str)] = &[("classic", tk("Classic")), ("material", tk("Material")), ("expressive", tk("Expressive")), ("round", tk("Round"))];
pub(crate) const INPUT_DESIGN: &[(&str, &str)] = &[("classic", tk("Classic")), ("filled", tk("Filled")), ("outlined", tk("Outlined")), ("pill", tk("Pill"))];
pub(crate) const CHIP_DESIGN: &[(&str, &str)] = &[("classic", tk("Classic")), ("material", tk("Material")), ("connected", tk("Connected"))];
pub(crate) const PROGRESS_DESIGN: &[(&str, &str)] = &[("classic", tk("Classic")), ("material", tk("Material")), ("wavy", tk("Wavy")), ("segmented", tk("Segmented"))];
pub(crate) const SURFACE_CORNERS: &[(&str, &str)] = &[("rounded", tk("Rounded")), ("asymmetric", tk("Asymmetric")), ("square", tk("Square")), ("squircle", tk("Squircle")), ("cut", tk("Cut"))];
pub(crate) const CARD_STYLE: &[(&str, &str)] = &[("tonal", tk("Tonal")), ("raised", tk("Raised")), ("outlined", tk("Outlined"))];
/// Themes' group of component designs (its own section on the page).
pub(crate) const COMPONENTS: &str = tk("Components");
const FILL: &[(&str, &str)] = &[("cover", tk("Fill")), ("contain", tk("Fit")), ("stretch", tk("Stretch")), ("center", tk("Center")), ("tile", tk("Tile"))];
/// `WallpaperTransition::ALL`'s order, each with how it looks.
pub(crate) const TRANSITION: &[(&str, &str)] = &[
    ("fade", tk("Fade")),
    ("wipe", tk("Wipe")),
    ("grow", tk("Grow")),
    ("ripple", tk("Ripple: a wave spreads from the pointer or the centre")),
    ("tide", tk("Tide: a wave rises from the bottom and washes the new image in")),
    ("tear", tk("Tear: the old image rips along a jagged seam and peels away")),
    ("glitch", tk("Glitch: slices shear sideways with a colour split, then settle")),
    ("shatter", tk("Shatter: the old image breaks into shards that fall away")),
    ("dissolve", tk("Dissolve: the old image burns away along a glowing edge")),
    ("sketch", tk("Sketch: the outline is drawn first, then the image fills in")),
    ("none", tk("None")),
];
/// The Sketch transition's group (Wallpaper → Fit & effects, and the
/// Motion page's Wallpaper change card).
pub(crate) const SKETCH_GROUP: &str = tk("Sketch transition");
const SKETCH_INK: &[(&str, &str)] = &[("accent", tk("Theme accent")), ("ink", tk("Ink black")), ("white", tk("White")), ("wallpaper", tk("The wallpaper's own colour"))];
const SKETCH_ORDER: &[(&str, &str)] = &[("subject", tk("Subject first, then the background")), ("top-down", tk("Top to bottom")), ("center", tk("From the centre")), ("pointer", tk("From the pointer"))];
const SKETCH_FILL: &[(&str, &str)] = &[("fade", tk("Fade in")), ("bleed", tk("Watercolour bleed from the lines")), ("wipe", tk("Wipe along the strokes"))];
/// Steps of the Sketch sliders.
const SKETCH_LINE_STEP: f64 = 0.25;
const SKETCH_DRAW_STEP: f64 = 0.05;

/// Why a Sketch option doesn't apply under `cfg` right now.
pub(super) fn sketch_locked(cfg: &mandara_config::ShellConfig) -> Option<&'static str> {
    (cfg.wallpaper.transition != mandara_config::desktop::WallpaperTransition::Sketch).then_some(tk("For the Sketch transition"))
}
pub(crate) const ORIGINS: &[(&str, &str)] = &[("center", tk("The centre")), ("pointer", tk("The pointer"))];
const ORDER: &[(&str, &str)] = &[("shuffle", tk("Shuffle")), ("random", tk("Random")), ("alphabetical", tk("By name")), ("newest", tk("Newest first"))];
const SLIDESHOW_SOURCE: &[(&str, &str)] = &[("folder", tk("A folder")), ("playlist", tk("Playlist"))];
const SLIDESHOW_TRANSITION: &[(&str, &str)] = &[("desktop", tk("Same as the desktop")), ("fade", tk("Fade")), ("wipe", tk("Wipe")), ("grow", tk("Grow")), ("none", tk("None"))];
/// Step of the slideshow transition's length slider, ms.
const SLIDESHOW_TRANSITION_STEP_MS: f64 = 50.0;

/// Why a slideshow setting doesn't apply under `cfg` right now.
pub(super) fn slideshow_locked(path: &str, cfg: &mandara_config::ShellConfig) -> Option<&'static str> {
    use mandara_config::desktop::{SlideshowSource, SlideshowTransition};
    let s = &cfg.wallpaper.slideshow;
    match path {
        "/wallpaper/slideshow/from" | "/wallpaper/slideshow/until" if !s.hours => Some(tk("Turn on Only at certain times first")),
        "/wallpaper/slideshow/transition_ms" if s.transition == SlideshowTransition::Desktop => Some(tk("Follows the desktop's transition (Fit & effects)")),
        "/wallpaper/slideshow/directory" | "/wallpaper/slideshow/subfolders" | "/wallpaper/slideshow/light_directory" | "/wallpaper/slideshow/dark_directory" if s.source == SlideshowSource::Playlist => {
            Some(tk("The playlist is used instead of a folder"))
        }
        _ => None,
    }
}
const VIDEO_PAUSE: &[(&str, &str)] = &[("covered", tk("When windows cover it")), ("fullscreen", tk("Fullscreen only")), ("never", tk("Never"))];
const LOCATION_MODE: &[(&str, &str)] = &[("manual", tk("Manual")), ("geoclue", tk("Location service (GeoClue)")), ("ip", tk("IP address lookup"))];
const WIND: &[(&str, &str)] = &[("auto", tk("Follow units")), ("meters-per-second", "m/s"), ("knots", tk("Knots"))];
const LOCK_CLOCK: &[(&str, &str)] = &[("classic", tk("Classic")), ("stacked", tk("Stacked")), ("compact", tk("Compact"))];
const LOCK_BG: &[(&str, &str)] = &[("wallpaper", tk("Wallpaper")), ("image", tk("Image")), ("color", tk("Solid color"))];
const LOCK_SYMBOLS: &[(&str, &str)] = &[("shapes", tk("Shapes")), ("dots", tk("Dots"))];
const LOCK_DESIGN: &[(&str, &str)] = &[("mandara", tk("Mandara")), ("material-you", tk("Material You"))];
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
/// `datetime.weekend`.
const WEEKEND: &[(&str, &str)] = &[
    ("auto", tk("Automatic")),
    ("saturday-sunday", tk("Saturday and Sunday")),
    ("friday-saturday", tk("Friday and Saturday")),
    ("friday", tk("Friday")),
    ("sunday", tk("Sunday")),
];

/// Every W5 setting, appended to the registry.
pub(super) fn entries() -> Vec<Setting> {
    use Control::*;
    use Section::*;
    let slider = |min: f64, max: f64, step: f64, unit: &'static str| Slider { min, max, step, unit };
    vec![
        // Bar behaviour
        s(Bars, tk("Behaviour"), "/bar/auto_hide/mode", tk("Auto-hide"), tk("Slide the bar away; touch its edge to bring it back"), Choice(AUTO_HIDE)),
        s(Bars, tk("Behaviour"), "/bar/auto_hide/delay_ms", tk("Hide delay"), tk("How long the bar stays after the pointer leaves"), slider(0.0, 3000.0, 50.0, " ms")),
        s(Bars, tk("Behaviour"), "/bar/auto_hide/reveal_on_workspace_switch", tk("Peek on workspace switch"), tk("Show the hidden bar briefly when you change workspace or tag"), Toggle),
        s(Bars, tk("Behaviour"), "/bar/click_through", tk("Click through empty space"), tk("Clicks between bar items reach the window below (no main menu on right-click there)"), Toggle),
        s(Bars, tk("Behaviour"), "/bar/right_click", tk("Right-click on empty bar space"), tk("What opens when you right-click the bar between and around its items"), Choice(BAR_RIGHT_CLICK)),
        s(
            Bars,
            tk("Behaviour"),
            "/bar/right_click_gaps",
            tk("Main menu in the bar's gaps"),
            tk("Right-clicking between a split bar's pieces opens the main menu; the rest of the desktop, and Main menu → Apps & tools → Desktop menu, still open the desktop's menu"),
            Toggle,
        ),
        s(Bars, tk("Behaviour"), "/bar/middle_click/action", tk("Middle-click on empty bar space"), tk("What a middle click between and around the bar's items does"), Choice(CORNER)),
        s(Bars, tk("Behaviour"), "/bar/middle_click/command", tk("Middle-click command"), tk("Run with sh -c when the action is Command"), Text { placeholder: "ghostty" }),
        s(Bars, tk("Behaviour"), "/bar/scroll", tk("Scroll on empty bar space"), tk("Scrolling between and around the bar's items switches workspace or changes the volume"), Choice(BAR_SCROLL)),
        s(Bars, tk("Main menu"), "/panels/menu_translucent", tk("Glass menu"), tk("The main menu is see-through glass with solid cards on it"), Toggle),
        s(Bars, tk("Main menu"), "/panels/menu_opacity", tk("Glass opacity"), tk("How see-through the main menu's glass is"), Control::Slider { min: 0.2, max: 1.0, step: 0.01, unit: "" }),
        s(Bars, tk("Main menu"), "/panels/menu_rail", tk("Rail"), tk("Every entry's name beside its icon, or a narrow icon-only column"), Choice(MENU_RAIL)),
        s(Bars, tk("Main menu"), "/panels/menu_rail_height", tk("Rail height"), tk("As tall as its buttons, or as tall as the panel beside it with the buttons spread evenly over it"), Choice(MENU_RAIL_HEIGHT)),
        s(Bars, tk("Main menu"), "/panels/menu_patterns", tk("Tile patterns"), tk("Home cards and action tiles in the main menu wear faint geometric patterns"), Toggle),
        s(Bars, tk("Main menu"), "/panels/menu_calligraphy", tk("Kufic calligraphy"), tk("The app and tool tiles wear a Kufic word instead of their geometric pattern"), Toggle),
        s(Bars, tk("Main menu"), "/panels/menu_tile_surface", tk("Tile surface"), tk("Flat fills (the default), raised 3D tiles with a glass rim, or see-through glass"), Choice(MENU_TILE_SURFACE)),
        s(Bars, tk("Main menu"), "/panels/menu_tile_shadow", tk("Tile shadow"), tk("How strong the shadow under Home cards and action tiles is (0 for none)"), Control::Slider { min: 0.0, max: 1.0, step: 0.05, unit: "" }),
        s(Bars, tk("Behaviour"), "/bar/over_fullscreen", tk("Show over fullscreen windows"), tk("Keeps the bar and its panels above fullscreen apps; applies after a shell restart"), Toggle),
        // Wallpaper
        s(Wallpaper, tk("Desktop"), "/wallpaper/fill", tk("Fill mode"), tk("How images that don't match the screen are placed"), Choice(FILL)),
        s(Wallpaper, tk("Desktop"), "/wallpaper/fill_color", tk("Fill color"), tk("Behind Fit and Center (hex)"), Text { placeholder: "#000000" }),
        s(Wallpaper, tk("Desktop"), "/wallpaper/transition", tk("Transition"), tk("How a new wallpaper replaces the old one"), Choice(TRANSITION)),
        s(Wallpaper, tk("Desktop"), "/wallpaper/transition_origin", tk("Transition origin"), tk("Where a Ripple opens and a Shatter bursts from"), Choice(ORIGINS)),
        s(Wallpaper, SKETCH_GROUP, "/wallpaper/sketch/ink", tk("Line colour"), tk("What the Sketch transition draws its lines in"), Choice(SKETCH_INK)),
        s(Wallpaper, SKETCH_GROUP, "/wallpaper/sketch/line_width", tk("Line thickness"), tk("How thick the outline's strokes are"), slider(SKETCH_LINE_RANGE.0 as f64, SKETCH_LINE_RANGE.1 as f64, SKETCH_LINE_STEP, " px")),
        s(Wallpaper, SKETCH_GROUP, "/wallpaper/sketch/glow", tk("Glow"), tk("A soft halo under the lines"), Toggle),
        s(Wallpaper, SKETCH_GROUP, "/wallpaper/sketch/draw_order", tk("Drawing order"), tk("Which lines are drawn first"), Choice(SKETCH_ORDER)),
        s(Wallpaper, SKETCH_GROUP, "/wallpaper/sketch/draw_share", tk("Drawing vs filling"), tk("Share of the transition spent drawing the lines; the rest fills the image in"), slider(SKETCH_DRAW_RANGE.0 as f64, SKETCH_DRAW_RANGE.1 as f64, SKETCH_DRAW_STEP, "")),
        s(Wallpaper, SKETCH_GROUP, "/wallpaper/sketch/fill_style", tk("Fill style"), tk("How the image arrives once its lines are drawn"), Choice(SKETCH_FILL)),
        s(Wallpaper, SKETCH_GROUP, "/wallpaper/sketch/keep_outline", tk("Keep the outline"), tk("Leave a faint copy of the lines over the wallpaper afterwards"), Toggle),
        s(Wallpaper, tk("Desktop"), "/wallpaper/share_with_login", tk("Show on the login screen"), tk("Mandara Login shows your wallpaper when your account is picked (a copy goes to /var/lib/mandara-login)"), Toggle),
        s(Wallpaper, tk("Light & dark"), "/wallpaper/variants/light", tk("Light wallpaper"), tk("Shown while the theme is light; empty uses the main wallpaper"), OptionalText { placeholder: "~/Pictures/Wallpapers/day.jpg" }),
        s(Wallpaper, tk("Light & dark"), "/wallpaper/variants/dark", tk("Dark wallpaper"), tk("Shown while the theme is dark; per-display wallpapers: mandara msg wallpaper output"), OptionalText { placeholder: "~/Pictures/Wallpapers/night.jpg" }),
        // Slideshow (Studio → Wallpaper → Slideshow: the first group is its
        // header card, the others its folds).
        s(Wallpaper, tk("Slideshow"), "/wallpaper/slideshow/enabled", tk("Slideshow"), tk("Change the wallpaper on a timer"), Toggle),
        s(Wallpaper, tk("Slideshow"), "/wallpaper/slideshow/interval_minutes", tk("Every"), tk("Time between wallpapers, in minutes"), Number { min: f64::from(SLIDESHOW_INTERVAL_RANGE.0), max: f64::from(SLIDESHOW_INTERVAL_RANGE.1) }),
        s(Wallpaper, tk("Slideshow"), "/wallpaper/slideshow/order", tk("Order"), tk("Shuffle shows every wallpaper once before any repeats"), Choice(ORDER)),
        s(Wallpaper, tk("Slideshow"), "/wallpaper/slideshow/enabled", tk("Next wallpaper"), tk("Skip to the next image now"), Command { button: tk("Next"), args: &["wallpaper", "next"] }),
        s(Wallpaper, tk("Source"), "/wallpaper/slideshow/source", tk("Wallpapers from"), tk("A folder, or the wallpapers you picked into the playlist"), Choice(SLIDESHOW_SOURCE)),
        s(Wallpaper, tk("Source"), "/wallpaper/slideshow/directory", tk("Folder"), tk("Empty uses your wallpaper folder"), Text { placeholder: "~/Pictures/Wallpapers" }),
        s(Wallpaper, tk("Source"), "/wallpaper/slideshow/subfolders", tk("Include all sub-folders"), tk("Look into every folder inside it (otherwise one level, like the picker)"), Toggle),
        s(Wallpaper, tk("Source"), "/wallpaper/slideshow/light_directory", tk("Light theme folder"), tk("Used instead while the theme is light; empty uses the folder above"), Text { placeholder: "~/Pictures/Wallpapers/Day" }),
        s(Wallpaper, tk("Source"), "/wallpaper/slideshow/dark_directory", tk("Dark theme folder"), tk("Used instead while the theme is dark; empty uses the folder above"), Text { placeholder: "~/Pictures/Wallpapers/Night" }),
        s(Wallpaper, tk("When to change"), "/wallpaper/slideshow/on_start", tk("New wallpaper at login"), tk("Show the next wallpaper each time Mandara starts"), Toggle),
        s(Wallpaper, tk("When to change"), "/wallpaper/slideshow/pause_on_fullscreen", tk("Pause during fullscreen"), tk("Keep the wallpaper while a fullscreen app is open"), Toggle),
        s(Wallpaper, tk("When to change"), "/wallpaper/slideshow/pause_on_battery", tk("Pause on battery"), tk("Keep the wallpaper while running on battery"), Toggle),
        s(Wallpaper, tk("When to change"), "/wallpaper/slideshow/pause_when_locked", tk("Pause while locked"), tk("No changes while the screen is locked or off"), Toggle),
        s(Wallpaper, tk("When to change"), "/wallpaper/slideshow/hours", tk("Only at certain times"), tk("Change wallpapers only between the two times below"), Toggle),
        s(Wallpaper, tk("When to change"), "/wallpaper/slideshow/from", tk("From"), tk("Changes start at this time"), Time),
        s(Wallpaper, tk("When to change"), "/wallpaper/slideshow/until", tk("Until"), tk("…and stop at this time (it may be after midnight)"), Time),
        s(Wallpaper, tk("Slideshow transition"), "/wallpaper/slideshow/transition", tk("Transition"), tk("How a slideshow change replaces the wallpaper"), Choice(SLIDESHOW_TRANSITION)),
        s(Wallpaper, tk("Slideshow transition"), "/wallpaper/slideshow/transition_ms", tk("Length"), tk("How long a slideshow change takes"), slider(SLIDESHOW_TRANSITION_RANGE.0 as f64, SLIDESHOW_TRANSITION_RANGE.1 as f64, SLIDESHOW_TRANSITION_STEP_MS, " ms")),
        s(Wallpaper, tk("Displays"), "/wallpaper/slideshow/per_display", tk("Different wallpaper on each display"), tk("Every display gets its own wallpaper from the slideshow; off, they all change together"), Toggle),
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
        s(Themes, COMPONENTS, "/style/slider_design", tk("Slider design"), tk("How the volume, brightness and mixer sliders look; thin sliders keep the classic look"), Choice(SLIDER_DESIGN)),
        s(Themes, COMPONENTS, "/style/slider_handle", tk("Slider handle"), tk("The upright line between a slider's fill and its track; None shows the fill alone"), Choice(SLIDER_HANDLE)),
        s(Themes, COMPONENTS, "/style/button_design", tk("Button design"), tk("The shape and motion of buttons in panels, the sidebar, the main menu and Studio"), Choice(BUTTON_DESIGN)),
        s(Themes, COMPONENTS, "/style/toggle_design", tk("Toggle design"), tk("How switches, check boxes and radio buttons look and move"), Choice(TOGGLE_DESIGN)),
        s(Themes, COMPONENTS, "/style/input_design", tk("Text field design"), tk("How text fields and search boxes are drawn"), Choice(INPUT_DESIGN)),
        s(Themes, COMPONENTS, "/style/chip_design", tk("Chip design"), tk("How chips and choices between options look"), Choice(CHIP_DESIGN)),
        s(Themes, COMPONENTS, "/style/progress_design", tk("Progress design"), tk("How meters and progress bars are drawn"), Choice(PROGRESS_DESIGN)),
        s(Themes, COMPONENTS, "/style/surface_corners", tk("Card corners"), tk("The corner family of cards; Corner roundness still sets their size"), Choice(SURFACE_CORNERS)),
        s(Themes, COMPONENTS, "/style/card_style", tk("Card style"), tk("How cards stand off their panel: tonal, raised on a shadow or outlined"), Choice(CARD_STYLE)),
        s(Themes, COMPONENTS, "/looks/keep_components", tk("Keep my component designs when changing looks"), tk("Off: picking a look also brings its component designs, which you can change afterwards"), Toggle),
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
        s(DateTime, tk("Calendar"), "/datetime/weekend", tk("Weekend"), tk("Days the calendar sets apart; automatic picks Friday and Saturday for a Saturday-first week or an Arabic interface"), Choice(WEEKEND)),
        s(DateTime, tk("Calendar"), "/datetime/hijri_dates", tk("Hijri dates"), tk("Islamic day numbers under each day and the Hijri month in the clock panel's calendar (Prayer times' adjustment applies)"), Toggle),
        s(DateTime, tk("Calendar"), "/datetime/eastern_numerals", tk("Eastern Arabic numerals"), tk("٠١٢٣ instead of 0123 in the clock panel and its calendar"), Toggle),
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
        s(LockScreen, tk("Design"), "/lock/design", tk("Design"), tk("Mandara's glass, or Material You: colours from your wallpaper, a big stacked clock and a tonal card with a scalloped avatar"), Choice(LOCK_DESIGN)),
        s(LockScreen, tk("Clock"), "/lock/clock_style", tk("Clock style"), tk("One light line, stacked hours over minutes, or a small clock"), Choice(LOCK_CLOCK)),
        s(LockScreen, tk("Clock"), "/lock/show_date", tk("Date"), tk("The date under the clock"), Toggle),
        s(LockScreen, tk("Background"), "/lock/background", tk("Background"), tk("Your wallpaper, an image of your choice or a solid color"), Choice(LOCK_BG)),
        s(LockScreen, tk("Background"), "/lock/image", tk("Image"), tk("Used with the Image background"), OptionalText { placeholder: "~/Pictures/lock.jpg" }),
        s(LockScreen, tk("Background"), "/lock/blur", tk("Blur"), tk("Softens the background"), slider(0.0, 64.0, 1.0, " px")),
        s(LockScreen, tk("Background"), "/lock/dim", tk("Dim"), tk("Darkens the background so text stays readable"), slider(0.0, 0.9, 0.05, "")),
        s(LockScreen, tk("Background"), "/lock/blank_other_outputs", tk("Blank other displays"), tk("Only the main display shows the lock screen; others stay black (OLED friendly)"), Toggle),
        s(LockScreen, tk("Password"), "/lock/password_symbols", tk("Password symbols"), tk("Each character you type appears as a different small shape, or as a plain dot"), Choice(LOCK_SYMBOLS)),
        s(LockScreen, tk("Show"), "/lock/show_avatar", tk("Profile picture"), tk("Your avatar above the password field"), Toggle),
        s(LockScreen, tk("Show"), "/lock/show_media", tk("Now playing"), tk("The track that is playing"), Toggle),
        s(LockScreen, tk("Show"), "/lock/show_notifications", tk("Notification count"), tk("How many notifications are waiting (never their content)"), Toggle),
        s(LockScreen, tk("Show"), "/lock/show_weather", tk("Weather"), tk("Current conditions under the date"), Toggle),
        s(LockScreen, tk("Show"), "/lock/show_battery", tk("Battery"), tk("Charge level on laptops"), Toggle),
        s(LockScreen, tk("Preview"), "/lock/clock_style", tk("Preview the lock screen"), tk("Shows it for a few seconds without locking"), Command { button: tk("Preview"), args: &["lock", "--demo"] }),
        // Hot corners & screen corners
        s(Bars, tk("Hot corners"), "/hot_corners/enabled", tk("Hot corners"), tk("Rest the pointer in a screen corner to run an action; replaces your compositor's own hot corner"), Toggle),
        s(Bars, tk("Hot corners"), "/hot_corners/top_left/action", tk("Top left"), "", Choice(CORNER)),
        s(Bars, tk("Hot corners"), "/hot_corners/top_right/action", tk("Top right"), "", Choice(CORNER)),
        s(Bars, tk("Hot corners"), "/hot_corners/bottom_left/action", tk("Bottom left"), "", Choice(CORNER)),
        s(Bars, tk("Hot corners"), "/hot_corners/bottom_right/action", tk("Bottom right"), "", Choice(CORNER)),
        s(Bars, tk("Hot corners"), "/hot_corners/top_left/command", tk("Top left command"), tk("Run with sh -c when the action is Command"), Text { placeholder: "ghostty" }),
        s(Bars, tk("Hot corners"), "/hot_corners/top_right/command", tk("Top right command"), tk("Run with sh -c when the action is Command"), Text { placeholder: "" }),
        s(Bars, tk("Hot corners"), "/hot_corners/bottom_left/command", tk("Bottom left command"), tk("Run with sh -c when the action is Command"), Text { placeholder: "" }),
        s(Bars, tk("Hot corners"), "/hot_corners/bottom_right/command", tk("Bottom right command"), tk("Run with sh -c when the action is Command"), Text { placeholder: "" }),
        s(Bars, tk("Hot corners"), "/hot_corners/delay_ms", tk("Delay"), tk("How long the pointer must rest in the corner"), slider(0.0, 1000.0, 10.0, " ms")),
        s(Bars, tk("Hot corners"), "/hot_corners/size", tk("Trigger size"), tk("How big the hot spot in each corner is"), slider(1.0, 24.0, 1.0, " px")),
        s(Bars, tk("Hot corners"), "/hot_corners/over_fullscreen", tk("Over fullscreen apps"), tk("Also fire while a fullscreen app is shown (on MangoWM the bar must show over fullscreen windows too)"), Toggle),
        s(Bars, tk("Screen corners"), "/screen_corners/enabled", tk("Rounded screen corners"), tk("Black rounded corners drawn over the desktop"), Toggle),
        s(Bars, tk("Screen corners"), "/screen_corners/radius", tk("Radius"), "", slider(4.0, 40.0, 1.0, " px")),
        s(Bars, tk("Screen corners"), "/screen_corners/top_left", tk("Top left"), "", Toggle),
        s(Bars, tk("Screen corners"), "/screen_corners/top_right", tk("Top right"), "", Toggle),
        s(Bars, tk("Screen corners"), "/screen_corners/bottom_left", tk("Bottom left"), "", Toggle),
        s(Bars, tk("Screen corners"), "/screen_corners/bottom_right", tk("Bottom right"), "", Toggle),
        s(Bars, tk("Screen corners"), "/screen_corners/hide_between_displays", tk("Hide corners between displays"), "", Toggle),
        // App themes (cross-app sync)
        s(Themes, tk("App themes"), "/integrations/kitty", "kitty", tk("~/.config/kitty/themes/mandara.conf (pick it with kitten themes or include it)"), Toggle),
        s(Themes, tk("App themes"), "/integrations/foot", "foot", tk("~/.cache/mandara/themes/foot.ini (include= it from foot.ini)"), Toggle),
        s(Themes, tk("App themes"), "/integrations/alacritty", "Alacritty", tk("~/.cache/mandara/themes/alacritty.toml (add it to general.import)"), Toggle),
        s(Themes, tk("App themes"), "/integrations/wezterm", "WezTerm", tk("A “Mandara” color scheme in ~/.config/wezterm/colors"), Toggle),
        s(Themes, tk("App themes"), "/integrations/helix", "Helix", tk("theme = \"mandara\""), Toggle),
        s(Themes, tk("App themes"), "/integrations/neovim", "Neovim", tk(":colorscheme mandara"), Toggle),
        s(Themes, tk("App themes"), "/integrations/vesktop", tk("Vesktop / Vencord"), tk("Enable “mandara” under Themes"), Toggle),
        s(Themes, tk("App themes"), "/integrations/kde", tk("KDE / Plasma"), tk("A “Mandara” color scheme in ~/.local/share/color-schemes"), Toggle),
        s(Themes, tk("App themes"), "/integrations/cava", "Cava", tk("Theme files and activation steps: docs/app-themes.md"), Toggle),
        s(Themes, tk("App themes"), "/integrations/tmux", "tmux", tk("Theme files and activation steps: docs/app-themes.md"), Toggle),
        s(Themes, tk("App themes"), "/integrations/zellij", "Zellij", tk("Theme files and activation steps: docs/app-themes.md"), Toggle),
        s(Themes, tk("App themes"), "/integrations/spicetify", "Spicetify", tk("Theme files and activation steps: docs/app-themes.md"), Toggle),
        s(Themes, tk("App themes"), "/integrations/firefox", tk("Firefox / Zen"), tk("Pywal-style colors.json and userChrome.css in ~/.cache/mandara/themes/firefox (see the README)"), Toggle),
    ]
}
