//! Registry entries for the launcher's providers, ranking, look and
//! launching; the Dock; the Window switcher and Screen time; clipboard
//! history and the screenshot / recording tools (W4). The bespoke parts
//! (app names and icons, per-app time limits, the dock preview) live in
//! `app/{launcher,dock,switcher}_page.rs`.

use mandara_i18n::tk;
use serde_json::Value;

use mandara_config::dock::ranges as dock_ranges;
use mandara_config::workflow::defaults as launcher;

/// Step of the launcher size sliders, px.
const LAUNCHER_SIZE_STEP_PX: f64 = 10.0;

use super::{Control, Section, Setting, s, sd, span};

/// Group names (the bespoke pages pick rows by them).
pub const PROVIDERS: &str = tk("Search providers");
pub const RANKING: &str = tk("Ranking");
pub const LOOK: &str = tk("Size & placement");
pub const LAUNCHING: &str = tk("Starting apps");
pub const APPS: &str = tk("Pinned & hidden apps");
pub const DOCK_LAYOUT: &str = tk("Layout");
pub const DOCK_BEHAVIOUR: &str = tk("Behaviour");
pub const DOCK_APPS: &str = tk("Apps");
pub const DOCK_STACKS: &str = tk("Stacks & Trash");
pub const DOCK_APPLICATIONS: &str = tk("Applications folder");
pub const DOCK_LOOK: &str = tk("Appearance");
pub const SWITCHER: &str = tk("Window switcher");
pub const SWITCHER_LIST: &str = tk("Which windows");
pub const SCREEN_TIME: &str = tk("Screen time");
pub const CLIPBOARD: &str = tk("Clipboard");
pub const SCREENSHOT: &str = tk("Screenshot");
pub const RECORDING: &str = tk("Screen recording");

/// Launcher placements (Studio → Launcher draws them as a screen diagram).
pub const POSITIONS: &[(&str, &str)] = &[
    ("top-left", tk("Top left")),
    ("top-center", tk("Top middle")),
    ("top-right", tk("Top right")),
    ("center", tk("Center of the screen")),
    ("bottom-left", tk("Bottom left")),
    ("bottom-center", tk("Bottom middle")),
    ("bottom-right", tk("Bottom right")),
];
const SEARCH_POSITIONS: &[(&str, &str)] = &[("auto", tk("Next to the edge")), ("top", tk("Top")), ("bottom", tk("Bottom"))];
const LAYOUTS: &[(&str, &str)] = &[("list", tk("List")), ("grid", tk("Grid"))];
const DOCK_POSITIONS: &[(&str, &str)] = &[("bottom", tk("Bottom")), ("left", tk("Left")), ("right", tk("Right"))];
const ALIGNMENTS: &[(&str, &str)] = &[("start", tk("Start")), ("center", tk("Center")), ("end", tk("End"))];
const INDICATORS: &[(&str, &str)] = &[("dot", tk("One dot")), ("dots", tk("A dot per window")), ("bar", tk("Bar")), ("none", tk("None"))];
const HIDING: &[(&str, &str)] = &[("never", tk("Off")), ("always", tk("On")), ("smart", tk("When a window overlaps"))];
/// Stack layouts and orders (Studio → Dock → Stacks).
pub const STACK_VIEWS: &[(&str, &str)] = &[("auto", tk("Automatic")), ("fan", tk("Fan")), ("grid", tk("Grid")), ("list", tk("List"))];
pub const STACK_SORTS: &[(&str, &str)] = &[("date-added", tk("Date added")), ("date-modified", tk("Date modified")), ("name", tk("Name")), ("kind", tk("Kind"))];
pub const STACK_DISPLAYS: &[(&str, &str)] = &[("stack", tk("Stack")), ("folder", tk("Folder"))];
/// The Applications folder's layouts and orders (Studio → Dock).
const APPS_VIEWS: &[(&str, &str)] = &[("grid", tk("Grid")), ("list", tk("List"))];
const APPS_ORDERS: &[(&str, &str)] = &[("name", tk("A–Z")), ("most-used", tk("Most used")), ("categories", tk("Categories"))];
const STYLES: &[(&str, &str)] = &[("carousel", tk("Live previews")), ("compact", tk("Icons only"))];
const CLICKS: &[(&str, &str)] = &[("copy", tk("Copy (right-click pastes)")), ("paste", tk("Paste (right-click copies)"))];
const FORMATS: &[(&str, &str)] = &[("png", "PNG"), ("jpeg", "JPEG"), ("webp", tk("WebP (lossless)"))];

pub fn entries() -> Vec<Setting> {
    use Control::*;
    use Section::*;
    let slider = |min: f64, max: f64, step: f64, unit: &'static str| Slider { min, max, step, unit };
    // Launcher sizes: their config bounds, in 10 px steps.
    let px_range = |(min, max): (f32, f32)| slider(f64::from(min), f64::from(max), LAUNCHER_SIZE_STEP_PX, " px");
    let shot = |path: &'static str, label: &'static str, help: &'static str, control: Control, default: Value| sd(Plugins, SCREENSHOT, path, label, help, control, default);
    vec![
        // Launcher: providers.
        s(Launcher, PROVIDERS, "/launcher/providers/emoji/enabled", tk("Emoji"), tk("Find emoji by English or Arabic name; Enter copies"), Toggle),
        s(Launcher, PROVIDERS, "/launcher/providers/emoji/prefix", tk("Emoji prefix"), tk("Typed first to search only emoji"), Text { placeholder: ":" }),
        s(Launcher, PROVIDERS, "/launcher/providers/emoji/global", tk("Emoji in plain searches"), tk("Mix matching emoji into every search"), Toggle),
        s(Launcher, PROVIDERS, "/launcher/providers/windows/enabled", tk("Open windows"), tk("Find a window by title or app; Enter focuses it"), Toggle),
        s(Launcher, PROVIDERS, "/launcher/providers/windows/prefix", tk("Windows prefix"), tk("Typed first to search only open windows"), Text { placeholder: "@" }),
        s(Launcher, PROVIDERS, "/launcher/providers/windows/global", tk("Windows in plain searches"), tk("Mix matching windows into every search"), Toggle),
        s(Launcher, PROVIDERS, "/launcher/providers/clipboard/enabled", tk("Clipboard history"), tk("Search what you copied; Enter copies it again"), Toggle),
        s(Launcher, PROVIDERS, "/launcher/providers/clipboard/prefix", tk("Clipboard prefix"), tk("Typed first to search only the clipboard history"), Text { placeholder: "#" }),
        s(Launcher, PROVIDERS, "/launcher/providers/clipboard/global", tk("Clipboard in plain searches"), tk("Mix matching copies into every search"), Toggle),
        s(Launcher, PROVIDERS, "/launcher/providers/wallpapers/enabled", tk("Wallpapers"), tk("Images in your wallpaper folder; Enter sets one"), Toggle),
        s(Launcher, PROVIDERS, "/launcher/providers/wallpapers/prefix", tk("Wallpapers prefix"), tk("Typed first to search only wallpapers"), Text { placeholder: "*" }),
        s(Launcher, PROVIDERS, "/launcher/providers/wallpapers/global", tk("Wallpapers in plain searches"), tk("Mix matching wallpapers into every search"), Toggle),
        s(Launcher, PROVIDERS, "/launcher/providers/files/enabled", tk("Files"), tk("Recent files, then names in Documents, Downloads, Pictures… (indexed in memory)"), Toggle),
        s(Launcher, PROVIDERS, "/launcher/providers/files/prefix", tk("Files prefix"), tk("Typed first to search only files; ~/ paths match whole paths"), Text { placeholder: "~" }),
        s(Launcher, PROVIDERS, "/launcher/providers/files/global", tk("Files in plain searches"), tk("Mix matching files into every search"), Toggle),
        s(Launcher, PROVIDERS, "/launcher/providers/actions/enabled", tk("Actions & settings"), tk("Themes, session actions, plugins, Studio pages, commands and skills"), Toggle),
        s(Launcher, PROVIDERS, "/launcher/providers/actions/prefix", tk("Actions prefix"), tk("Typed first to list only actions and commands"), Text { placeholder: ">" }),
        s(Launcher, PROVIDERS, "/launcher/providers/actions/global", tk("Actions in plain searches"), tk("Off: plain searches list apps (and the providers above) only"), Toggle),
        // Launcher: ranking.
        s(Launcher, RANKING, "/launcher/usage/track", tk("Remember what you open"), tk("Counted on this computer only (~/.local/state/mandara)"), Toggle),
        s(Launcher, RANKING, "/launcher/usage/sort", tk("Rank by use"), tk("Frequently and recently used results come first"), Toggle),
        s(Launcher, RANKING, "/launcher/usage/half_life_days", tk("Usage fades after"), tk("A launch counts half as much after this many days"), slider(1.0, 90.0, 1.0, " d")),
        s(Launcher, RANKING, "/launcher/usage/recent_count", tk("Recent apps"), tk("Recently used apps listed under the pinned ones before you type"), slider(0.0, 12.0, 1.0, "")),
        s(Launcher, RANKING, "/launcher/usage/reset", tk("Forget usage"), tk("Start ranking from scratch"), Command { button: tk("Reset"), args: &["launcher", "usage", "reset"] }),
        // Launcher: look.
        s(Launcher, LOOK, "/launcher/look/position", tk("Placement"), tk("Float in the middle of the screen, or attach to the top or bottom edge (the bar's, when it's there) at the left, middle or right"), Choice(POSITIONS)),
        s(Launcher, LOOK, "/launcher/look/align_to_button", tk("Align to the launcher button"), tk("Attached at the bar's edge, in the zone holding the bar's launcher button: grow out of the button"), Toggle),
        s(Launcher, LOOK, "/launcher/look/search_position", tk("Search field"), tk("Above or below the results"), Choice(SEARCH_POSITIONS)),
        s(Launcher, LOOK, "/launcher/look/width", tk("Width when attached"), tk("Launcher width at a screen edge"), px_range(launcher::LAUNCHER_WIDTH_RANGE_PX)),
        s(Launcher, LOOK, "/launcher/look/height", tk("Height when attached"), tk("Tallest it grows; fewer rows show when this is low"), px_range(launcher::LAUNCHER_HEIGHT_RANGE_PX)),
        s(Launcher, LOOK, "/launcher/look/center_width", tk("Width in the center"), tk("Size of the floating card in the middle of the screen"), px_range(launcher::LAUNCHER_CENTER_WIDTH_RANGE_PX)),
        s(Launcher, LOOK, "/launcher/look/center_height", tk("Height in the center"), tk("Tallest the floating card grows"), px_range(launcher::LAUNCHER_HEIGHT_RANGE_PX)),
        s(Launcher, tk("Results"), "/launcher/look/layout", tk("Layout"), tk("Grid shows app tiles while nothing is typed"), Choice(LAYOUTS)),
        s(Launcher, tk("Results"), "/launcher/look/grid_columns", tk("Grid columns"), tk("App tiles per row"), slider(3.0, 10.0, 1.0, "")),
        s(Launcher, tk("Results"), "/launcher/look/compact", tk("Compact rows"), tk("One line per result"), Toggle),
        s(Launcher, tk("Launcher"), "/launcher/look/dim", tk("Dim the screen"), tk("Darken everything behind the launcher (a bar it hangs from stays bright)"), slider(0.0, 0.7, 0.05, "")),
        s(Launcher, tk("Launcher"), "/launcher/look/ignore_mouse", tk("Keyboard only"), tk("Hovering never moves the selection"), Toggle),
        s(Launcher, tk("Results"), "/launcher/look/remember_query", tk("Remember the last search"), tk("Reopen with what you typed last (selected, so typing replaces it)"), Toggle),
        s(Launcher, tk("Results"), "/launcher/look/app_actions", tk("App actions"), tk("List desktop actions such as New Private Window"), Toggle),
        s(Launcher, tk("Results"), "/launcher/look/origin_badges", tk("Packaging badges"), tk("Mark Flatpak, Snap, AppImage and your own apps"), Toggle),
        // Launcher: starting apps.
        s(Launcher, LAUNCHING, "/launcher/launch/wrapper", tk("Launch prefix"), tk("Put before every app's command, e.g. uwsm app -- or app2unit --"), Text { placeholder: "uwsm app --" }),
        s(Launcher, LAUNCHING, "/launcher/launch/systemd_scope", tk("Own systemd scope per app"), tk("Each app runs in app-mandara-<app>.scope: tracked resources, survives shell restarts"), Toggle),
        s(Launcher, LAUNCHING, "/launcher/launch/terminal", tk("Terminal"), tk("For terminal apps; empty uses your preferred terminal (Applications), then $TERMINAL, then the first installed one"), Text { placeholder: "ghostty -e" }),
        s(Launcher, LAUNCHING, "/launcher/launch/auto_paste", tk("Paste after copying"), tk("Emoji, calculator results and clipboard entries are pasted at once"), Toggle),
        s(Launcher, APPS, "/launcher/pinned", tk("Pinned apps"), tk("Desktop ids listed first, in this order (Ctrl+P in the launcher pins too)"), List { placeholder: "org.gnome.Nautilus, firefox" }),
        s(Launcher, APPS, "/launcher/hidden", tk("Hidden apps"), tk("Desktop ids never listed"), List { placeholder: "org.gnome.Tour" }),
        // Dock (Studio → Dock lays these out by group, with the pinned
        // apps and stacks editors in between).
        s(Dock, DOCK_LAYOUT, "/dock/position", tk("Position on screen"), tk("Screen edge the dock sits on (a side bar's edge is taken: the dock moves to the bottom there)"), Choice(DOCK_POSITIONS)),
        s(Dock, DOCK_LAYOUT, "/dock/media_chip", tk("Now-playing chip"), tk("Album art, title and play / pause at the dock's end while music plays"), Toggle),
        s(Dock, DOCK_LAYOUT, "/dock/alignment", tk("Alignment"), tk("Where along the edge"), Choice(ALIGNMENTS)),
        s(Dock, DOCK_LAYOUT, "/dock/icon_size", tk("Size"), tk("Resting icon size"), span(dock_ranges::ICON_SIZE, " px")),
        s(Dock, DOCK_LAYOUT, "/dock/magnification", tk("Magnification"), tk("Icons swell as the pointer passes (spring-animated)"), Toggle),
        s(Dock, DOCK_LAYOUT, "/dock/magnify", tk("Magnified size"), tk("Peak size of the icon under the pointer"), span(dock_ranges::MAGNIFY, "×")),
        s(Dock, DOCK_LAYOUT, "/dock/magnify_spread", tk("Magnification spread"), tk("How many neighbours grow along"), span(dock_ranges::MAGNIFY_SPREAD, "")),
        s(Dock, DOCK_LAYOUT, "/dock/isolate_displays", tk("Only this display's windows"), tk("Running apps count only windows on the dock's own display"), Toggle),
        s(Dock, DOCK_BEHAVIOUR, "/dock/auto_hide", tk("Automatically hide and show the Dock"), tk("Hidden docks come back when the pointer touches their screen edge"), Choice(HIDING)),
        s(Dock, DOCK_BEHAVIOUR, "/dock/show_delay_ms", tk("Reveal delay"), tk("Pointer rest on the edge before a hidden dock shows"), span(dock_ranges::SHOW_DELAY_MS, " ms")),
        s(Dock, DOCK_BEHAVIOUR, "/dock/hide_delay_ms", tk("Hide delay"), tk("Grace period after the pointer leaves"), span(dock_ranges::HIDE_DELAY_MS, " ms")),
        s(Dock, DOCK_BEHAVIOUR, "/dock/hide_speed", tk("Hide and show speed"), tk("How fast the dock slides in and out"), span(dock_ranges::HIDE_SPEED, "×")),
        s(Dock, DOCK_BEHAVIOUR, "/dock/reserve_space", tk("Keep windows clear of the dock"), tk("Windows tile around a dock that never hides"), Toggle),
        s(Dock, DOCK_BEHAVIOUR, "/dock/overlay", tk("Above fullscreen windows"), tk("Put the dock on the overlay layer"), Toggle),
        s(Dock, DOCK_BEHAVIOUR, "/dock/bounce_on_launch", tk("Animate opening applications"), tk("Icons bounce until the app's first window appears"), Toggle),
        s(Dock, DOCK_BEHAVIOUR, "/dock/bounce_on_attention", tk("Bounce for attention"), tk("Apps with an urgent window or a critical notification bounce"), Toggle),
        s(Dock, DOCK_BEHAVIOUR, "/dock/minimize_to_app", tk("Minimize windows into application icon"), tk("Off: each minimized window gets its own icon on the right side"), Toggle),
        s(Dock, DOCK_BEHAVIOUR, "/dock/tooltips", tk("Show names on hover"), tk("A label above the icon under the pointer"), Toggle),
        s(Dock, DOCK_APPS, "/dock/show_running", tk("Show running apps"), tk("Also show apps that aren't kept in the dock while they run"), Toggle),
        s(Dock, DOCK_APPS, "/dock/indicator", tk("Show indicators for open applications"), tk("Under each running app"), Choice(INDICATORS)),
        s(Dock, DOCK_APPS, "/dock/show_recent", tk("Show suggested and recent apps in Dock"), tk("Apps you used lately, after the running ones"), Toggle),
        s(Dock, DOCK_APPS, "/dock/recent_count", tk("Recent apps"), tk("How many recent apps"), span(dock_ranges::RECENT_COUNT, "")),
        s(Dock, DOCK_APPS, "/dock/notification_badges", tk("Notification badges"), tk("Unread notification count on the app's icon"), Toggle),
        s(Dock, DOCK_APPS, "/dock/progress_badges", tk("Progress bars"), tk("Progress reported by an app's notifications, on its icon"), Toggle),
        s(Dock, DOCK_APPS, "/dock/badges", tk("Window count"), tk("A badge on apps with two or more windows (when no notifications are unread)"), Toggle),
        s(Dock, DOCK_APPS, "/dock/max_apps", tk("Most apps"), tk("0 shows every app"), span(dock_ranges::MAX_APPS, "")),
        s(Dock, DOCK_APPS, "/dock/launcher_button", tk("Launcher button"), tk("Opens the launcher"), Toggle),
        s(Dock, DOCK_APPS, "/dock/launcher_at_end", tk("Launcher button at the end"), tk("Off: at the start"), Toggle),
        s(Dock, DOCK_APPLICATIONS, "/dock/applications/enabled", tk("Applications folder"), tk("Your apps in a Launchpad-style grid, on the right of the dock; drop an app on it to install it"), Toggle),
        s(Dock, DOCK_APPLICATIONS, "/dock/applications/view", tk("View as"), tk("Large icons, or rows with each app's description"), Choice(APPS_VIEWS)),
        s(Dock, DOCK_APPLICATIONS, "/dock/applications/order", tk("Order"), tk("A–Z, most used first, or grouped by category"), Choice(APPS_ORDERS)),
        s(Dock, DOCK_APPLICATIONS, "/dock/applications/icon_size", tk("Icon size"), tk("Size of the app icons in the grid"), span(dock_ranges::APPS_ICON_SIZE, " px")),
        s(Dock, DOCK_STACKS, "/dock/trash", tk("Trash"), tk("Drop files on it to trash them; right-click to empty it"), Toggle),
        s(Dock, DOCK_STACKS, "/dock/stack_items", tk("Files in a stack"), tk("How many of the newest files a stack shows"), span(dock_ranges::STACK_ITEMS, "")),
        s(Dock, DOCK_STACKS, "/dock/fan_items", tk("Files in a fan"), tk("A fan shows this many of the newest files; its top row opens the folder in your file manager"), span(dock_ranges::FAN_ITEMS, "")),
        s(Dock, DOCK_STACKS, "/dock/stack_names", tk("Show file names"), tk("Names beside fan items and under grid icons"), Toggle),
        s(Dock, DOCK_LOOK, "/dock/opacity", tk("Glass opacity"), tk("0 hides the glass, leaving the icons"), span(dock_ranges::OPACITY, "")),
        s(Dock, DOCK_LOOK, "/dock/tint", tk("Accent tint"), tk("Mixes the theme accent into the glass"), span(dock_ranges::TINT, "")),
        s(Dock, DOCK_LOOK, "/dock/radius", tk("Corner radius"), tk("Capped at a full pill; follows Themes → Corner roundness"), span(dock_ranges::RADIUS, " px")),
        s(Dock, DOCK_LOOK, "/dock/padding", tk("Padding"), tk("Room around the icons"), span(dock_ranges::PADDING, " px")),
        s(Dock, DOCK_LOOK, "/dock/spacing", tk("Icon spacing"), tk("Gap between icons"), span(dock_ranges::SPACING, " px")),
        s(Dock, DOCK_LOOK, "/dock/margin", tk("Distance from the edge"), tk("Gap between the dock and the screen edge"), span(dock_ranges::MARGIN, " px")),
        s(Dock, DOCK_LOOK, "/dock/shadow", tk("Shadow"), tk("Soft elevation under the dock"), Toggle),
        s(Dock, DOCK_LOOK, "/dock/rim", tk("Specular rim"), tk("1 px glass edge highlight"), Toggle),
        s(Dock, DOCK_LOOK, "/dock/separator", tk("Separators"), tk("Hairlines between apps, recent apps and the right side"), Toggle),
        s(Dock, DOCK_LOOK, "/dock/inactive_opacity", tk("Not-running opacity"), tk("Apps that aren't running fade to this"), span(dock_ranges::INACTIVE_OPACITY, "")),
        // Window switcher.
        s(WindowSwitcher, SWITCHER, "/switcher/enabled", tk("Enable the window switcher"), tk("Alt+Tab runs `mandara msg switcher next`, Alt+Shift+Tab `switcher prev`"), Toggle),
        s(WindowSwitcher, SWITCHER, "/switcher/bind", tk("Keyboard shortcut"), tk("Add Alt+Tab and Alt+Shift+Tab to Mandara's compositor drop-in (skipped when your config already uses them)"), Command { button: tk("Bind Alt+Tab"), args: &["switcher", "bind"] }),
        s(WindowSwitcher, SWITCHER, "/switcher/style", tk("Style"), tk("Live window previews or app icons"), Choice(STYLES)),
        s(WindowSwitcher, SWITCHER, "/switcher/previews", tk("Live previews"), tk("Copies each window's contents (ext-image-copy-capture); app icons otherwise"), Toggle),
        s(WindowSwitcher, SWITCHER, "/switcher/captions", tk("Titles"), tk("Window titles under the previews"), Toggle),
        s(WindowSwitcher, SWITCHER, "/switcher/show_count", tk("Window count"), tk("How many windows are listed"), Toggle),
        s(WindowSwitcher, SWITCHER, "/switcher/release_commits", tk("Switch on release"), tk("Letting go of Alt switches; off, Enter or a click does"), Toggle),
        s(WindowSwitcher, SWITCHER, "/switcher/thumbnail_width", tk("Preview size"), tk("Width of each preview"), slider(140.0, 420.0, 4.0, " px")),
        s(WindowSwitcher, SWITCHER, "/switcher/max_columns", tk("Previews per row"), tk("More windows wrap onto further rows"), slider(2.0, 10.0, 1.0, "")),
        s(WindowSwitcher, SWITCHER, "/switcher/demo", tk("Try it"), tk("Shows the switcher for three seconds without taking the keyboard"), Command { button: tk("Preview"), args: &["switcher", "demo"] }),
        s(WindowSwitcher, SWITCHER_LIST, "/switcher/current_workspace", tk("This workspace only"), tk("Only windows on the focused workspace or tags"), Toggle),
        s(WindowSwitcher, SWITCHER_LIST, "/switcher/all_outputs", tk("All displays"), tk("Off: only windows on the focused display"), Toggle),
        s(WindowSwitcher, SWITCHER_LIST, "/switcher/include_minimized", tk("Minimized windows"), tk("List minimized windows too (switching restores them)"), Toggle),
        s(WindowSwitcher, SWITCHER_LIST, "/switcher/group_by_app", tk("One entry per app"), tk("Only each app's most recent window"), Toggle),
        // Screen time.
        s(WindowSwitcher, SCREEN_TIME, "/screen_time/track", tk("Record screen time"), tk("Which app has focus, per day; needs the Screen time plugin. Stays on this computer"), Toggle),
        s(WindowSwitcher, SCREEN_TIME, "/screen_time/idle_minutes", tk("Stop counting when idle"), tk("Minutes without input after which time no longer counts"), slider(1.0, 30.0, 1.0, " min")),
        s(WindowSwitcher, SCREEN_TIME, "/screen_time/retention_days", tk("Keep history"), tk("Days of screen time kept"), slider(7.0, 365.0, 1.0, " d")),
        s(WindowSwitcher, SCREEN_TIME, "/screen_time/daily_limit_min", tk("Daily reminder"), tk("A notification after this much screen time in a day (0 = off)"), slider(0.0, 720.0, 15.0, " min")),
        s(WindowSwitcher, SCREEN_TIME, "/screen_time/exclude", tk("Never record"), tk("App ids left out of screen time"), List { placeholder: "org.keepassxc.KeePassXC" }),
        s(WindowSwitcher, SCREEN_TIME, "/screen_time/reset", tk("Forget today"), tk("Clears today's screen time"), Command { button: tk("Reset today"), args: &["screentime", "reset", "today"] }),
        // Clipboard history.
        s(Plugins, CLIPBOARD, "/clipboard/enabled", tk("Record history"), tk("Off pauses recording; the history stays"), Toggle),
        s(Plugins, CLIPBOARD, "/clipboard/max_entries", tk("History size"), tk("Entries kept (pinned ones don't count)"), slider(10.0, 1000.0, 10.0, "")),
        s(Plugins, CLIPBOARD, "/clipboard/max_pinned", tk("Pinned entries"), tk("Most pinned entries kept"), slider(0.0, 100.0, 1.0, "")),
        s(Plugins, CLIPBOARD, "/clipboard/pinned_shown", tk("Pinned rows shown"), tk("Pinned entries listed above the history in the panel"), slider(0.0, 12.0, 1.0, "")),
        s(Plugins, CLIPBOARD, "/clipboard/max_text_kb", tk("Largest text"), tk("Longer copies aren't recorded"), slider(4.0, 4096.0, 4.0, " KiB")),
        s(Plugins, CLIPBOARD, "/clipboard/images", tk("Record images"), tk("Copied images and screenshots"), Toggle),
        s(Plugins, CLIPBOARD, "/clipboard/max_image_mb", tk("Largest image"), tk("Bigger images aren't recorded"), slider(1.0, 128.0, 1.0, " MiB")),
        s(Plugins, CLIPBOARD, "/clipboard/persist", tk("Keep after restart"), tk("Saved in ~/.cache/mandara/clipboard; off keeps it in memory only"), Toggle),
        s(Plugins, CLIPBOARD, "/clipboard/keep_from_closed_apps", tk("Keep copies from closed apps"), tk("When the app you copied from quits, its last copy can still be pasted"), Toggle),
        s(Plugins, CLIPBOARD, "/clipboard/clear_on_start", tk("Clear at startup"), tk("Unpinned entries are forgotten when Mandara starts"), Toggle),
        s(Plugins, CLIPBOARD, "/clipboard/clear_on_lock", tk("Clear when locking"), tk("Unpinned entries are forgotten when the session locks"), Toggle),
        s(Plugins, CLIPBOARD, "/clipboard/auto_clear_hours", tk("Forget after"), tk("Unpinned entries older than this are dropped (0 = never)"), slider(0.0, 168.0, 1.0, " h")),
        s(Plugins, CLIPBOARD, "/clipboard/exclude_password_managers", tk("Skip passwords"), tk("Copies marked secret by password managers are never recorded"), Toggle),
        s(Plugins, CLIPBOARD, "/clipboard/exclude_apps", tk("Never record from"), tk("App ids whose copies are skipped while they have focus"), List { placeholder: "org.keepassxc.KeePassXC, 1password" }),
        s(Plugins, CLIPBOARD, "/clipboard/confirm_clear", tk("Confirm before clearing"), tk("“Clear all” asks once more"), Toggle),
        s(Plugins, CLIPBOARD, "/clipboard/click_action", tk("Click"), tk("What clicking an entry does"), Choice(CLICKS)),
        s(Plugins, CLIPBOARD, "/clipboard/image_command", tk("Image editor"), tk("Edit button on images; {path} is the image, e.g. swappy -f {path}"), Text { placeholder: "swappy -f {path}" }),
        // Screenshot (plugin settings table).
        shot("/plugins/settings/screenshot/directory", tk("Folder"), tk("Where screenshots are saved (empty: Pictures/Screenshots)"), Text { placeholder: "~/Pictures/Screenshots" }, Value::from("")),
        shot("/plugins/settings/screenshot/filename", tk("File name"), tk("strftime pattern, e.g. Screenshot_%Y-%m-%d_%H-%M-%S"), Text { placeholder: "Screenshot_%Y-%m-%d_%H-%M-%S" }, Value::from("Screenshot_%Y-%m-%d_%H-%M-%S")),
        shot("/plugins/settings/screenshot/format", tk("Format"), tk("Saved files; the clipboard always gets PNG"), Choice(FORMATS), Value::from("png")),
        shot("/plugins/settings/screenshot/quality", tk("JPEG quality"), tk("Higher is sharper and larger"), slider(40.0, 100.0, 1.0, ""), Value::from(90)),
        shot("/plugins/settings/screenshot/cursor", tk("Include the pointer"), tk("Paint the mouse pointer into captures"), Toggle, Value::from(false)),
        shot("/plugins/settings/screenshot/edit_after", tk("Edit window and screen captures"), tk("Open the annotation editor instead of saving at once"), Toggle, Value::from(false)),
        shot("/plugins/settings/screenshot/capture_on_release", tk("Capture on release"), tk("Letting go of a region drag captures (no annotating)"), Toggle, Value::from(false)),
        shot("/plugins/settings/screenshot/remember_region", tk("Remember the region"), tk("Start where the last region was"), Toggle, Value::from(false)),
        shot("/plugins/settings/screenshot/dim", tk("Dim while selecting"), tk("Darkness around the selection"), slider(0.0, 0.8, 0.05, ""), Value::from(0.45)),
        shot("/plugins/settings/screenshot/border", tk("Selection outline"), tk("Outline width"), slider(1.0, 6.0, 0.5, " px"), Value::from(2)),
        shot("/plugins/settings/screenshot/show_size", tk("Size badge"), tk("Pixel size next to the selection"), Toggle, Value::from(true)),
        shot("/plugins/settings/screenshot/show_hints", tk("Keyboard hints"), tk("The hint strip under the toolbar"), Toggle, Value::from(true)),
        shot("/plugins/settings/screenshot/toolbar_position", tk("Toolbar position"), tk("Screen edge the capture toolbar sits on"), Choice(const { &[("top", tk("Top")), ("bottom", tk("Bottom"))] }), Value::from("top")),
        shot("/plugins/settings/screenshot/toolbar_scale", tk("Toolbar size"), tk("Size of the capture toolbar and its hints"), slider(0.75, 1.5, 0.05, "×"), Value::from(1)),
        shot("/plugins/settings/screenshot/pipe_command", tk("Send to"), tk("Pipe each capture (PNG on stdin) to a command, e.g. satty -f -"), Text { placeholder: "satty -f -" }, Value::from("")),
        shot("/plugins/settings/screenshot/sound", tk("Shutter sound"), tk("Uses your sound theme (Studio → Audio & media → Sounds)"), Toggle, Value::from(true)),
        shot("/plugins/settings/screenshot/notify", tk("Notify"), tk("A notification after window and screen captures"), Toggle, Value::from(true)),
        sd(Plugins, SCREENSHOT, "/plugins/settings/screenshot/record_directory", tk("Recordings folder"), tk("Where screen recordings go (empty: Videos/Recordings)"), Text { placeholder: "~/Videos/Recordings" }, Value::from("")),
        sd(Plugins, SCREENSHOT, "/plugins/settings/screenshot/record_fps", tk("Recording frame rate"), tk("Frames per second of screen recordings"), slider(5.0, 60.0, 1.0, " fps"), Value::from(30)),
        sd(Plugins, SCREENSHOT, "/plugins/settings/screenshot/record_width", tk("Recording width"), tk("Recordings are scaled down to at most this width"), slider(640.0, 3840.0, 80.0, " px"), Value::from(1600)),
    ]
}
