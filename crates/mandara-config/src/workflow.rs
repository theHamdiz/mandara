//! Everyday-workflow settings: the launcher's search providers, usage
//! ranking and app customizations, the dock (`[dock]`), the Alt-Tab window
//! switcher (`[switcher]`), screen time (`[screen_time]`), clipboard history
//! (`[clipboard]`) and the screenshot tool (`[plugins.settings.screenshot]`).
//!
//! Every struct is `#[serde(default)]` so older configs keep loading;
//! defaults live in [`defaults`] with their units in the name.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Default values of the workflow settings, in one place.
pub mod defaults {
    // ── Launcher ──
    /// Launcher width, logical px.
    pub const LAUNCHER_WIDTH_PX: f32 = 600.0;
    /// Bounds of the launcher width slider, logical px.
    pub const LAUNCHER_WIDTH_RANGE_PX: (f32, f32) = (440.0, 960.0);
    /// Tallest an attached launcher grows, logical px (fits the default
    /// rows with room to spare).
    pub const LAUNCHER_HEIGHT_PX: f32 = 560.0;
    /// Floating card in the middle of the screen: larger than the drawer.
    pub const LAUNCHER_CENTER_WIDTH_PX: f32 = 720.0;
    pub const LAUNCHER_CENTER_HEIGHT_PX: f32 = 640.0;
    /// Bounds of the center card's width and of both height sliders, px.
    pub const LAUNCHER_CENTER_WIDTH_RANGE_PX: (f32, f32) = (480.0, 1280.0);
    pub const LAUNCHER_HEIGHT_RANGE_PX: (f32, f32) = (240.0, 1000.0);
    /// Most app tiles per row in the grid layout (fewer when narrower).
    pub const LAUNCHER_GRID_COLUMNS: u32 = 6;
    /// App icon size in the grid layout, and its bounds, logical px.
    pub const LAUNCHER_GRID_ICON_PX: f32 = 48.0;
    pub const LAUNCHER_GRID_ICON_RANGE_PX: (f32, f32) = (32.0, 96.0);
    /// The Beacon style's search bar width and its bounds, logical px.
    pub const LAUNCHER_BEACON_WIDTH_PX: f32 = 680.0;
    pub const LAUNCHER_BEACON_WIDTH_RANGE_PX: (f32, f32) = (480.0, 960.0);
    /// Where the Beacon bar's top sits, a share of the screen height, and
    /// its bounds.
    pub const LAUNCHER_BEACON_TOP: f32 = 0.24;
    pub const LAUNCHER_BEACON_TOP_RANGE: (f32, f32) = (0.08, 0.45);
    /// Usage counts halve every this many days (frecency).
    pub const USAGE_HALF_LIFE_DAYS: f32 = 14.0;
    /// Recently used apps listed when the query is empty.
    pub const LAUNCHER_RECENT_COUNT: u32 = 6;
    /// Scrim behind the launcher (0 = none).
    pub const LAUNCHER_DIM: f32 = 0.0;
    /// Provider prefixes.
    pub const PREFIX_EMOJI: &str = ":";
    pub const PREFIX_WINDOWS: &str = "@";
    pub const PREFIX_CLIPBOARD: &str = "#";
    pub const PREFIX_WALLPAPERS: &str = "*";
    pub const PREFIX_FILES: &str = "~";
    pub const PREFIX_ACTIONS: &str = ">";

    // ── Window switcher ──
    /// Width of one window preview, logical px.
    pub const SWITCHER_THUMB_W_PX: f32 = 236.0;
    /// Previews per row before the carousel wraps.
    pub const SWITCHER_MAX_COLUMNS: u32 = 6;

    // ── Screen time ──
    /// Days of history kept.
    pub const SCREEN_TIME_RETENTION_DAYS: u32 = 90;
    /// Minutes without input after which time stops counting.
    pub const SCREEN_TIME_IDLE_MIN: u32 = 5;

    // ── Clipboard ──
    pub const CLIPBOARD_MAX_ENTRIES: usize = 200;
    pub const CLIPBOARD_MAX_PINNED: usize = 25;
    /// Pinned entries listed at the top of the clipboard panel.
    pub const CLIPBOARD_PINNED_SHOWN: usize = 4;
    /// Entries kept in collections (they survive clears like pinned ones).
    pub const CLIPBOARD_MAX_COLLECTED: usize = 500;
    /// Lines of a code, CSS or Markdown entry shown in its panel row (0 =
    /// one-line rows), and the bounds of that setting.
    pub const CLIPBOARD_PREVIEW_LINES: usize = 3;
    pub const CLIPBOARD_PREVIEW_LINES_RANGE: (usize, usize) = (0, 8);
    /// Largest text entry kept, KiB.
    pub const CLIPBOARD_MAX_TEXT_KB: u32 = 256;
    /// Largest image entry kept, MiB.
    pub const CLIPBOARD_MAX_IMAGE_MB: u32 = 32;
    /// Apps whose copies are never recorded (password managers).
    pub const CLIPBOARD_EXCLUDED_APPS: &[&str] = &["org.keepassxc.KeePassXC", "keepassxc", "1password", "Bitwarden", "com.bitwarden.desktop", "org.gnome.World.Secrets"];

    // ── Screenshot ──
    /// `strftime` pattern of saved screenshot names (no extension).
    pub const SCREENSHOT_FILENAME: &str = "Screenshot_%Y-%m-%d_%H-%M-%S";
    /// JPEG quality (WebP is lossless).
    pub const SCREENSHOT_QUALITY: u8 = 90;
    /// Scrim over the frozen screen while selecting.
    pub const SCREENSHOT_DIM: f32 = 0.45;
    /// Selection outline width, logical px.
    pub const SCREENSHOT_BORDER_PX: f32 = 2.0;
    /// Size of the capture toolbar (and its hint strip), × its design size.
    pub const SCREENSHOT_TOOLBAR_SCALE: f32 = 1.0;
    /// Bounds of [`SCREENSHOT_TOOLBAR_SCALE`].
    pub const SCREENSHOT_TOOLBAR_SCALE_RANGE: (f32, f32) = (0.75, 1.5);
    /// "Capture in N s": seconds to open a panel before the frame is taken.
    pub const SCREENSHOT_COUNTDOWN_S: u64 = 3;
    /// Bounds of [`SCREENSHOT_COUNTDOWN_S`].
    pub const SCREENSHOT_COUNTDOWN_RANGE_S: (u64, u64) = (1, 10);
}

// ── Launcher ─────────────────────────────────────────────────────────────

/// One launcher search provider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub enabled: bool,
    /// Typing this first searches only this provider (`:smile`).
    pub prefix: String,
    /// Also mixed into plain (unprefixed) searches.
    pub global: bool,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self { enabled: true, prefix: String::new(), global: false }
    }
}

fn provider(prefix: &str, global: bool) -> ProviderConfig {
    ProviderConfig { enabled: true, prefix: prefix.into(), global }
}

/// The launcher's search providers besides apps, `/` slash commands and
/// `=` arithmetic.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LauncherProviders {
    /// Emoji by English or Arabic name; Enter copies (or pastes).
    pub emoji: ProviderConfig,
    /// Open windows; Enter focuses.
    pub windows: ProviderConfig,
    /// Clipboard history; Enter copies (or pastes).
    pub clipboard: ProviderConfig,
    /// Images in the wallpaper folder; Enter sets the wallpaper.
    pub wallpapers: ProviderConfig,
    /// Recent files, then file names (plocate when installed).
    pub files: ProviderConfig,
    /// Actions, plugins, commands, Studio sections and skills.
    pub actions: ProviderConfig,
}

impl Default for LauncherProviders {
    fn default() -> Self {
        Self {
            emoji: provider(defaults::PREFIX_EMOJI, false),
            windows: provider(defaults::PREFIX_WINDOWS, true),
            clipboard: provider(defaults::PREFIX_CLIPBOARD, false),
            wallpapers: provider(defaults::PREFIX_WALLPAPERS, false),
            files: provider(defaults::PREFIX_FILES, false),
            actions: provider(defaults::PREFIX_ACTIONS, true),
        }
    }
}

impl LauncherProviders {
    /// `(id, provider)` pairs, in display order.
    pub fn all(&self) -> [(&'static str, &ProviderConfig); 6] {
        [("emoji", &self.emoji), ("windows", &self.windows), ("clipboard", &self.clipboard), ("wallpapers", &self.wallpapers), ("files", &self.files), ("actions", &self.actions)]
    }
}

/// Usage tracking: what you open often (and lately) ranks higher.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LauncherUsage {
    /// Count launches in `~/.local/state/mandara/launcher-usage.toml`.
    pub track: bool,
    /// Rank results by frecency within each match tier.
    pub sort: bool,
    /// A launch's weight halves after this many days.
    pub half_life_days: f32,
    /// Recently used apps shown under the pinned ones on an empty query.
    pub recent_count: u32,
}

impl Default for LauncherUsage {
    fn default() -> Self {
        Self { track: true, sort: true, half_life_days: defaults::USAGE_HALF_LIFE_DAYS, recent_count: defaults::LAUNCHER_RECENT_COUNT }
    }
}

/// Where the launcher appears: floating in the middle of the screen, or
/// attached to the top / bottom edge (the bar's, when the bar is there) at
/// its left, middle or right. Left and right are physical: they stay put in
/// right-to-left languages.
///
/// Older configs said `bottom` / `top` (a drawer in the middle of that
/// edge): they load as `bottom-center` / `top-center`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LauncherPosition {
    /// A floating glass card in the middle of the screen.
    Center,
    TopLeft,
    /// A drawer pouring down from the top edge.
    #[serde(alias = "top")]
    TopCenter,
    TopRight,
    BottomLeft,
    /// A drawer rising out of the bottom edge (Caelestia-style).
    #[default]
    #[serde(alias = "bottom")]
    BottomCenter,
    BottomRight,
}

/// The screen edge an attached launcher grows from.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LauncherEdge {
    Top,
    Bottom,
}

/// Where along its edge an attached launcher sits (physical sides).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LauncherAlign {
    Left,
    Center,
    Right,
}

impl LauncherPosition {
    /// Every placement, in the order Studio and `launcher placement` list them.
    pub const ALL: [LauncherPosition; 7] = [
        LauncherPosition::TopLeft,
        LauncherPosition::TopCenter,
        LauncherPosition::TopRight,
        LauncherPosition::Center,
        LauncherPosition::BottomLeft,
        LauncherPosition::BottomCenter,
        LauncherPosition::BottomRight,
    ];

    /// [`Self::id`] of each of [`Self::ALL`], in order.
    pub const IDS: [&'static str; 7] = ["top-left", "top-center", "top-right", "center", "bottom-left", "bottom-center", "bottom-right"];

    /// The config / command spelling (`top-left`, `center`, …).
    pub fn id(self) -> &'static str {
        match self {
            LauncherPosition::Center => "center",
            LauncherPosition::TopLeft => "top-left",
            LauncherPosition::TopCenter => "top-center",
            LauncherPosition::TopRight => "top-right",
            LauncherPosition::BottomLeft => "bottom-left",
            LauncherPosition::BottomCenter => "bottom-center",
            LauncherPosition::BottomRight => "bottom-right",
        }
    }

    /// Parses [`Self::id`], plus the older `top` / `bottom` and the
    /// `middle` / `centre` spellings.
    pub fn from_id(id: &str) -> Option<Self> {
        let id = id.trim().to_ascii_lowercase().replace(['_', ' '], "-").replace("centre", "center").replace("middle", "center");
        match id.as_str() {
            "top" => Some(LauncherPosition::TopCenter),
            "bottom" => Some(LauncherPosition::BottomCenter),
            _ => Self::ALL.into_iter().find(|p| p.id() == id),
        }
    }

    /// The edge it is attached to (`None`: floating in the middle).
    pub fn edge(self) -> Option<LauncherEdge> {
        match self {
            LauncherPosition::Center => None,
            LauncherPosition::TopLeft | LauncherPosition::TopCenter | LauncherPosition::TopRight => Some(LauncherEdge::Top),
            LauncherPosition::BottomLeft | LauncherPosition::BottomCenter | LauncherPosition::BottomRight => Some(LauncherEdge::Bottom),
        }
    }

    pub fn align(self) -> LauncherAlign {
        match self {
            LauncherPosition::TopLeft | LauncherPosition::BottomLeft => LauncherAlign::Left,
            LauncherPosition::TopRight | LauncherPosition::BottomRight => LauncherAlign::Right,
            _ => LauncherAlign::Center,
        }
    }

    pub fn is_center(self) -> bool {
        self == LauncherPosition::Center
    }
}

/// Where the search field sits relative to the results.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SearchPosition {
    /// Next to the edge the launcher comes from (bottom for the bottom drawer).
    #[default]
    Auto,
    Top,
    Bottom,
}

/// How results are laid out.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LauncherLayout {
    #[default]
    List,
    /// App tiles in a grid while browsing and while typing plain words
    /// (prefixed searches, `/` and `=` stay a list).
    Grid,
}

/// The launcher's overall shape.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LauncherStyle {
    /// One surface holding the search field and the results, placed by
    /// [`LauncherLook::position`].
    #[default]
    Classic,
    /// A centred glass search bar in the upper part of the screen with
    /// mode buttons beside it; results open in their own glass card under
    /// it once you type (the placement doesn't apply).
    Beacon,
}

impl LauncherStyle {
    pub fn id(self) -> &'static str {
        match self {
            LauncherStyle::Classic => "classic",
            LauncherStyle::Beacon => "beacon",
        }
    }
}

/// The launcher's size, placement and behaviour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LauncherLook {
    /// Width while attached to an edge, logical px.
    pub width: f32,
    /// Tallest the attached launcher grows, logical px (fewer rows fit
    /// when this is lower than `max_results` rows).
    pub height: f32,
    /// Size of the floating card (`position = "center"`), logical px.
    pub center_width: f32,
    pub center_height: f32,
    pub position: LauncherPosition,
    /// Attached at the bar's edge, in the zone holding the bar's launcher
    /// button: grow out of the button instead of the zone's edge / middle.
    pub align_to_button: bool,
    pub search_position: SearchPosition,
    /// Classic (one surface) or Beacon (a centred search bar).
    pub style: LauncherStyle,
    pub layout: LauncherLayout,
    /// Most tiles per row in the grid (fewer fit a narrower launcher).
    pub grid_columns: u32,
    /// App icon size in the grid, logical px.
    pub grid_icon_size: f32,
    /// A row of category chips (All, Pinned, Internet, Office…) above the
    /// grid.
    pub grid_categories: bool,
    /// Results under headings by kind (Top hit, Applications, Actions,
    /// Files…).
    pub group_results: bool,
    /// Keyboard hints under the results (↑↓ move, Enter opens…).
    pub hints: bool,
    /// The Beacon search bar's width, logical px.
    pub beacon_width: f32,
    /// The Beacon bar's top, a share of the screen height (0.24: about a
    /// quarter of the way down).
    pub beacon_top: f32,
    /// Shorter rows without the second line.
    pub compact: bool,
    /// Darken the screen behind the launcher (0–0.7).
    pub dim: f32,
    /// Keyboard only: hovering never moves the selection.
    pub ignore_mouse: bool,
    /// Reopen with the last query.
    pub remember_query: bool,
    /// List desktop actions ("New Window", "New Private Window") as results.
    pub app_actions: bool,
    /// Small badges for Flatpak, Snap, AppImage and your own (~/.local) apps.
    pub origin_badges: bool,
}

impl Default for LauncherLook {
    fn default() -> Self {
        Self {
            width: defaults::LAUNCHER_WIDTH_PX,
            height: defaults::LAUNCHER_HEIGHT_PX,
            center_width: defaults::LAUNCHER_CENTER_WIDTH_PX,
            center_height: defaults::LAUNCHER_CENTER_HEIGHT_PX,
            position: LauncherPosition::BottomCenter,
            align_to_button: true,
            search_position: SearchPosition::Auto,
            style: LauncherStyle::Classic,
            layout: LauncherLayout::List,
            grid_columns: defaults::LAUNCHER_GRID_COLUMNS,
            grid_icon_size: defaults::LAUNCHER_GRID_ICON_PX,
            grid_categories: true,
            group_results: false,
            hints: false,
            beacon_width: defaults::LAUNCHER_BEACON_WIDTH_PX,
            beacon_top: defaults::LAUNCHER_BEACON_TOP,
            compact: false,
            dim: defaults::LAUNCHER_DIM,
            ignore_mouse: false,
            remember_query: false,
            app_actions: true,
            origin_badges: true,
        }
    }
}

impl LauncherLook {
    /// Width and tallest height for the current placement, clamped to the
    /// Studio slider bounds (the center card has its own size).
    pub fn size(&self) -> (f32, f32) {
        if self.position.is_center() {
            self.size_for_center()
        } else {
            (clamp_or(self.width, defaults::LAUNCHER_WIDTH_RANGE_PX, defaults::LAUNCHER_WIDTH_PX), clamp_or(self.height, defaults::LAUNCHER_HEIGHT_RANGE_PX, defaults::LAUNCHER_HEIGHT_PX))
        }
    }

    /// The floating card's width and tallest height, clamped (the Beacon
    /// style's results card grows up to this height too).
    pub fn size_for_center(&self) -> (f32, f32) {
        (
            clamp_or(self.center_width, defaults::LAUNCHER_CENTER_WIDTH_RANGE_PX, defaults::LAUNCHER_CENTER_WIDTH_PX),
            clamp_or(self.center_height, defaults::LAUNCHER_HEIGHT_RANGE_PX, defaults::LAUNCHER_CENTER_HEIGHT_PX),
        )
    }

    /// The grid's icon size, clamped to its range.
    pub fn grid_icon(&self) -> f32 {
        clamp_or(self.grid_icon_size, defaults::LAUNCHER_GRID_ICON_RANGE_PX, defaults::LAUNCHER_GRID_ICON_PX)
    }

    /// The Beacon bar's width and top (share of the screen height),
    /// clamped to their ranges.
    pub fn beacon(&self) -> (f32, f32) {
        (
            clamp_or(self.beacon_width, defaults::LAUNCHER_BEACON_WIDTH_RANGE_PX, defaults::LAUNCHER_BEACON_WIDTH_PX),
            clamp_or(self.beacon_top, defaults::LAUNCHER_BEACON_TOP_RANGE, defaults::LAUNCHER_BEACON_TOP),
        )
    }
}

/// `v` clamped to `(lo, hi)`, or `d` when it isn't a number.
fn clamp_or(v: f32, (lo, hi): (f32, f32), d: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { d }
}

// ── Launcher looks ───────────────────────────────────────────────────────

/// One ready-made launcher look: a named set of [`LauncherLook`] values
/// applied together (Studio → Launcher → Launcher looks, `launcher look
/// <id>`). Every value can still be changed on its own afterwards; the
/// look then shows as customized.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LauncherLookPreset {
    pub id: &'static str,
    /// Display name and one-line description (i18n keys).
    pub name: &'static str,
    pub blurb: &'static str,
    pub style: LauncherStyle,
    pub layout: LauncherLayout,
    pub position: LauncherPosition,
    pub search_position: SearchPosition,
    pub compact: bool,
    pub group_results: bool,
    pub hints: bool,
    pub grid_columns: u32,
    pub grid_icon_size: f32,
    pub grid_categories: bool,
    pub center_width: f32,
    pub center_height: f32,
    pub dim: f32,
}

/// The shipped launcher looks; the first (Classic) is the default.
pub const LAUNCHER_LOOKS: [LauncherLookPreset; 3] = [
    LauncherLookPreset {
        id: "classic",
        name: mandara_i18n::tk("Classic"),
        blurb: mandara_i18n::tk("The list launcher: a drawer rising from the bottom edge, search next to the edge"),
        style: LauncherStyle::Classic,
        layout: LauncherLayout::List,
        position: LauncherPosition::BottomCenter,
        search_position: SearchPosition::Auto,
        compact: false,
        group_results: false,
        hints: false,
        grid_columns: defaults::LAUNCHER_GRID_COLUMNS,
        grid_icon_size: defaults::LAUNCHER_GRID_ICON_PX,
        grid_categories: true,
        center_width: defaults::LAUNCHER_CENTER_WIDTH_PX,
        center_height: defaults::LAUNCHER_CENTER_HEIGHT_PX,
        dim: defaults::LAUNCHER_DIM,
    },
    LauncherLookPreset {
        id: "grid",
        name: mandara_i18n::tk("Grid"),
        blurb: mandara_i18n::tk("Every app as a large icon in a card in the middle of the screen, with category chips"),
        style: LauncherStyle::Classic,
        layout: LauncherLayout::Grid,
        position: LauncherPosition::Center,
        search_position: SearchPosition::Top,
        compact: false,
        group_results: false,
        hints: false,
        grid_columns: 8,
        grid_icon_size: 56.0,
        grid_categories: true,
        center_width: 880.0,
        center_height: 660.0,
        dim: 0.2,
    },
    LauncherLookPreset {
        id: "beacon",
        name: mandara_i18n::tk("Beacon"),
        blurb: mandara_i18n::tk("A centred glass search bar in the style of macOS Spotlight; grouped results open under it as you type"),
        style: LauncherStyle::Beacon,
        layout: LauncherLayout::List,
        position: LauncherPosition::Center,
        search_position: SearchPosition::Top,
        compact: false,
        group_results: true,
        hints: true,
        grid_columns: defaults::LAUNCHER_GRID_COLUMNS,
        grid_icon_size: defaults::LAUNCHER_GRID_ICON_PX,
        grid_categories: true,
        center_width: defaults::LAUNCHER_CENTER_WIDTH_PX,
        center_height: defaults::LAUNCHER_CENTER_HEIGHT_PX,
        dim: defaults::LAUNCHER_DIM,
    },
];

impl LauncherLookPreset {
    /// The look with this id (case and spacing forgiven).
    pub fn find(id: &str) -> Option<&'static LauncherLookPreset> {
        let id = id.trim().to_ascii_lowercase();
        LAUNCHER_LOOKS.iter().find(|l| l.id == id)
    }

    /// Writes this look's values into `look` (sizes, providers and
    /// behaviour settings it doesn't name are kept).
    pub fn apply(&self, look: &mut LauncherLook) {
        look.style = self.style;
        look.layout = self.layout;
        look.position = self.position;
        look.search_position = self.search_position;
        look.compact = self.compact;
        look.group_results = self.group_results;
        look.hints = self.hints;
        look.grid_columns = self.grid_columns;
        look.grid_icon_size = self.grid_icon_size;
        look.grid_categories = self.grid_categories;
        look.center_width = self.center_width;
        look.center_height = self.center_height;
        look.dim = self.dim;
    }

    /// Whether `look` has exactly this look's values.
    pub fn matches(&self, look: &LauncherLook) -> bool {
        let mut applied = look.clone();
        self.apply(&mut applied);
        applied == *look
    }

    /// The look `look` matches, if any (`None`: customized).
    pub fn active(look: &LauncherLook) -> Option<&'static LauncherLookPreset> {
        LAUNCHER_LOOKS.iter().find(|l| l.matches(look))
    }
}

/// How apps are started.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LaunchOptions {
    /// Prefix for every launch, e.g. `uwsm app --` or `app2unit --`.
    pub wrapper: String,
    /// Move each launched app into its own systemd user scope
    /// (`app-mandara-<app>-<pid>.scope`), so its resources are tracked and
    /// it survives a shell restart cleanly.
    pub systemd_scope: bool,
    /// Terminal for `Terminal=true` apps (`ghostty -e`); empty uses
    /// `$TERMINAL`, then the first installed of a few well-known ones.
    pub terminal: String,
    /// Paste right away after copying from the launcher (calculator,
    /// emoji, clipboard history).
    pub auto_paste: bool,
}

/// Your own name / icon for an app, keyed by desktop id.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppOverride {
    /// Shown instead of the desktop entry's `Name` (empty keeps it).
    pub name: String,
    /// Icon spec: theme icon name or image path (empty keeps it).
    pub icon: String,
}

// ── Dock ─────────────────────────────────────────────────────────────────

/// The dock moved to [`crate::dock`]; re-exported here for older paths.
pub use crate::dock::{DOCK_PLUGIN_ID, DockAlignment, DockAutoHide, DockConfig, DockDisplays, DockIndicator, DockPosition, DockStack, StackDisplay, StackSort, StackView};

// ── Window switcher ──────────────────────────────────────────────────────

/// Look of the Alt-Tab switcher.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SwitcherStyle {
    /// Large live previews in a row.
    #[default]
    Carousel,
    /// App icons only, in a slim strip.
    Compact,
}

/// The Alt-Tab window switcher.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SwitcherConfig {
    pub enabled: bool,
    pub style: SwitcherStyle,
    /// Live window previews (ext-image-copy-capture); app icons otherwise.
    pub previews: bool,
    /// Window titles under the previews.
    pub captions: bool,
    /// "3 windows" counter in the header.
    pub show_count: bool,
    /// Only windows on the focused workspace / tags.
    pub current_workspace: bool,
    /// Windows of every output (off: the focused output only).
    pub all_outputs: bool,
    pub include_minimized: bool,
    /// One entry per app (its most recent window).
    pub group_by_app: bool,
    /// Releasing the modifier (Alt) switches; off, Enter or a click does.
    pub release_commits: bool,
    pub thumbnail_width: f32,
    pub max_columns: u32,
}

impl Default for SwitcherConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            style: SwitcherStyle::Carousel,
            previews: true,
            captions: true,
            show_count: true,
            current_workspace: false,
            all_outputs: true,
            include_minimized: true,
            group_by_app: false,
            release_commits: true,
            thumbnail_width: defaults::SWITCHER_THUMB_W_PX,
            max_columns: defaults::SWITCHER_MAX_COLUMNS,
        }
    }
}

// ── Screen time ──────────────────────────────────────────────────────────

/// Screen time / digital wellbeing (the built-in `screen-time` plugin).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenTimeConfig {
    /// Record which app has focus, per day. Stays on this computer
    /// (`~/.local/state/mandara/screen-time/`).
    pub track: bool,
    pub retention_days: u32,
    /// Stop counting after this many minutes without input.
    pub idle_minutes: u32,
    /// App ids never recorded.
    pub exclude: Vec<String>,
    /// Remind after this much total screen time a day (minutes, 0 = off).
    pub daily_limit_min: u32,
    /// Per-app daily reminders (app id → minutes).
    pub app_limits: BTreeMap<String, u32>,
}

impl Default for ScreenTimeConfig {
    fn default() -> Self {
        Self {
            track: true,
            retention_days: defaults::SCREEN_TIME_RETENTION_DAYS,
            idle_minutes: defaults::SCREEN_TIME_IDLE_MIN,
            exclude: Vec::new(),
            daily_limit_min: 0,
            app_limits: BTreeMap::new(),
        }
    }
}

// ── Clipboard ────────────────────────────────────────────────────────────

/// What clicking a clipboard-history entry does.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClipClick {
    /// Put it on the clipboard (right-click pastes).
    #[default]
    Copy,
    /// Put it on the clipboard and paste into the focused window
    /// (right-click only copies).
    Paste,
}

/// Clipboard history (`[clipboard]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipboardConfig {
    /// Record what you copy.
    pub enabled: bool,
    pub max_entries: usize,
    /// Pinned entries survive clears and never fall off the end.
    pub max_pinned: usize,
    /// Pinned entries listed above the history in the panel.
    pub pinned_shown: usize,
    pub max_text_kb: u32,
    /// Record copied images.
    pub images: bool,
    pub max_image_mb: u32,
    /// Keep history across restarts (`~/.cache/mandara/clipboard/`).
    pub persist: bool,
    /// When the app you copied from quits, Mandara keeps offering its last
    /// copy (so it can still be pasted).
    pub keep_from_closed_apps: bool,
    pub clear_on_start: bool,
    pub clear_on_lock: bool,
    /// Forget unpinned entries older than this (hours, 0 = never).
    pub auto_clear_hours: u32,
    /// Skip copies marked secret by password managers.
    pub exclude_password_managers: bool,
    /// Never record copies made while one of these apps has focus.
    pub exclude_apps: Vec<String>,
    /// Ask once more before clearing everything.
    pub confirm_clear: bool,
    pub click_action: ClipClick,
    /// Command for the edit button on images; `{path}` is the image file
    /// (e.g. `swappy -f {path}` or `satty --filename {path}`).
    pub image_command: String,
    /// Entries kept in collections (oldest dropped beyond it).
    pub max_collected: usize,
    /// Filter chips by kind (text, links, code, colours, …) in the panel.
    pub kind_chips: bool,
    /// Lines of code, CSS and Markdown previewed in a panel row (0 = one
    /// line, like plain text).
    pub preview_lines: usize,
}

impl ClipboardConfig {
    /// [`Self::preview_lines`] clamped to its range.
    pub fn preview_lines(&self) -> usize {
        let (lo, hi) = defaults::CLIPBOARD_PREVIEW_LINES_RANGE;
        self.preview_lines.clamp(lo, hi)
    }
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_entries: defaults::CLIPBOARD_MAX_ENTRIES,
            max_pinned: defaults::CLIPBOARD_MAX_PINNED,
            pinned_shown: defaults::CLIPBOARD_PINNED_SHOWN,
            max_text_kb: defaults::CLIPBOARD_MAX_TEXT_KB,
            images: true,
            max_image_mb: defaults::CLIPBOARD_MAX_IMAGE_MB,
            persist: true,
            keep_from_closed_apps: false,
            clear_on_start: false,
            clear_on_lock: false,
            auto_clear_hours: 0,
            exclude_password_managers: true,
            exclude_apps: defaults::CLIPBOARD_EXCLUDED_APPS.iter().map(|s| s.to_string()).collect(),
            confirm_clear: true,
            click_action: ClipClick::Copy,
            image_command: String::new(),
            max_collected: defaults::CLIPBOARD_MAX_COLLECTED,
            kind_chips: true,
            preview_lines: defaults::CLIPBOARD_PREVIEW_LINES,
        }
    }
}

// ── Screenshot ───────────────────────────────────────────────────────────

/// File format of saved screenshots.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShotFormat {
    #[default]
    Png,
    Jpeg,
    /// Lossless WebP.
    Webp,
}

impl ShotFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ShotFormat::Png => "png",
            ShotFormat::Jpeg => "jpg",
            ShotFormat::Webp => "webp",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            ShotFormat::Png => "image/png",
            ShotFormat::Jpeg => "image/jpeg",
            ShotFormat::Webp => "image/webp",
        }
    }
}

/// Screen edge of the screenshot / recording toolbar.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolbarPosition {
    #[default]
    Top,
    Bottom,
}

/// The screenshot plugin's settings, read from
/// `[plugins.settings.screenshot]` (Studio → Plugins → Screenshot).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenshotSettings {
    /// Enter / Capture copies the image.
    pub copy: bool,
    /// Enter / Capture saves the image.
    pub save: bool,
    pub ocr_language: String,
    /// Seconds to wait before a capture.
    pub delay: u64,
    /// Seconds "Capture in N s" (toolbar, `--delay` without a number)
    /// waits, so a panel can be opened first (1–10).
    pub countdown: u64,
    /// Folder for saved screenshots (`~` allowed); empty = Pictures/Screenshots.
    pub directory: String,
    /// `strftime` file name pattern, without extension.
    pub filename: String,
    pub format: ShotFormat,
    /// JPEG quality, 1–100.
    pub quality: u8,
    /// Include the pointer in screen / window captures.
    pub cursor: bool,
    /// Open the annotation editor after screen / window captures too.
    pub edit_after: bool,
    /// Releasing a region drag captures at once (skips annotating; off:
    /// Enter, Space or the toolbar captures).
    pub capture_on_release: bool,
    /// Start a new region selection where the last one was.
    pub remember_region: bool,
    /// Scrim over the frozen screen while selecting (0–0.8).
    pub dim: f32,
    /// Selection outline width, logical px.
    pub border: f32,
    /// Size badge (`1280 × 720`) next to the selection.
    pub show_size: bool,
    /// Keyboard hint strip under (or over) the toolbar.
    pub show_hints: bool,
    /// Screen edge the capture toolbar sits on.
    pub toolbar_position: ToolbarPosition,
    /// Toolbar size, × its design size (0.75–1.5).
    pub toolbar_scale: f32,
    /// Pipe each capture (PNG on stdin) to this command, e.g. `satty -f -`.
    pub pipe_command: String,
    /// Shutter sound (Studio → Sounds controls the theme and volume).
    pub sound: bool,
    /// A notification after window / screen captures.
    pub notify: bool,
}

impl Default for ScreenshotSettings {
    fn default() -> Self {
        Self {
            copy: true,
            save: true,
            ocr_language: "auto".into(),
            delay: 0,
            countdown: defaults::SCREENSHOT_COUNTDOWN_S,
            directory: String::new(),
            filename: defaults::SCREENSHOT_FILENAME.into(),
            format: ShotFormat::Png,
            quality: defaults::SCREENSHOT_QUALITY,
            cursor: false,
            edit_after: false,
            capture_on_release: false,
            remember_region: false,
            dim: defaults::SCREENSHOT_DIM,
            border: defaults::SCREENSHOT_BORDER_PX,
            show_size: true,
            show_hints: true,
            toolbar_position: ToolbarPosition::Top,
            toolbar_scale: defaults::SCREENSHOT_TOOLBAR_SCALE,
            pipe_command: String::new(),
            sound: true,
            notify: true,
        }
    }
}

impl ScreenshotSettings {
    /// The settings stored in `[plugins.settings.screenshot]`; keys that
    /// are missing or invalid fall back to their defaults one by one.
    /// [`Self::toolbar_scale`] clamped to its range.
    pub fn toolbar_scale(&self) -> f32 {
        let (lo, hi) = defaults::SCREENSHOT_TOOLBAR_SCALE_RANGE;
        if self.toolbar_scale.is_finite() { self.toolbar_scale.clamp(lo, hi) } else { defaults::SCREENSHOT_TOOLBAR_SCALE }
    }

    /// [`Self::countdown`] clamped to its range.
    pub fn countdown_secs(&self) -> u64 {
        let (lo, hi) = defaults::SCREENSHOT_COUNTDOWN_RANGE_S;
        self.countdown.clamp(lo, hi)
    }

    /// The other screen edge for the capture toolbar.
    pub fn flipped_position(&self) -> ToolbarPosition {
        match self.toolbar_position {
            ToolbarPosition::Top => ToolbarPosition::Bottom,
            ToolbarPosition::Bottom => ToolbarPosition::Top,
        }
    }

    pub fn from_plugins(plugins: &crate::PluginsConfig) -> Self {
        let Some(table) = plugins.settings.get("screenshot") else { return Self::default() };
        // Whole-table parse first (fast path), then key by key so one bad
        // value never resets the others.
        if let Ok(s) = toml::Value::Table(table.clone()).try_into::<Self>() {
            return s;
        }
        let mut out = toml::Table::try_from(Self::default()).unwrap_or_default();
        for (k, v) in table {
            let mut trial = out.clone();
            trial.insert(k.clone(), v.clone());
            if toml::Value::Table(trial.clone()).try_into::<Self>().is_ok() {
                out = trial;
            }
        }
        toml::Value::Table(out).try_into().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ShellConfig;

    #[test]
    fn defaults_round_trip_and_old_configs_load() {
        let cfg = ShellConfig::default();
        let text = toml::to_string(&cfg).unwrap();
        let back: ShellConfig = toml::from_str(&text).unwrap();
        assert_eq!(back, cfg);
        // A config from before these sections still loads with defaults.
        let old: ShellConfig = toml::from_str("[launcher]\nmax_results = 9\n").unwrap();
        assert_eq!(old.launcher.max_results, 9);
        assert_eq!(old.launcher.providers.emoji.prefix, ":");
        assert_eq!(old.dock, DockConfig::default());
        assert_eq!(old.clipboard.max_entries, defaults::CLIPBOARD_MAX_ENTRIES);
        assert!(old.switcher.release_commits);
        assert_eq!(old.clipboard.pinned_shown, defaults::CLIPBOARD_PINNED_SHOWN);
        assert!(old.clipboard.kind_chips);
        assert_eq!(old.clipboard.max_collected, defaults::CLIPBOARD_MAX_COLLECTED);
        let wide: ShellConfig = toml::from_str("[clipboard]\npreview_lines = 99\n").unwrap();
        assert_eq!(wide.clipboard.preview_lines(), defaults::CLIPBOARD_PREVIEW_LINES_RANGE.1);
    }

    #[test]
    fn older_launcher_positions_migrate_to_placements() {
        for (old, new) in [("bottom", LauncherPosition::BottomCenter), ("top", LauncherPosition::TopCenter), ("center", LauncherPosition::Center)] {
            let cfg: ShellConfig = toml::from_str(&format!("[launcher.look]\nposition = \"{old}\"\nwidth = 700.0\n")).unwrap();
            assert_eq!(cfg.launcher.look.position, new, "{old}");
            assert_eq!(cfg.launcher.look.width, 700.0, "the old width stays the attached width");
            assert_eq!(cfg.launcher.look.center_width, defaults::LAUNCHER_CENTER_WIDTH_PX);
            assert!(cfg.launcher.look.align_to_button);
            // Saved back in the new spelling, and it round-trips.
            let text = toml::to_string(&cfg).unwrap();
            assert!(text.contains(&format!("position = \"{}\"", new.id())), "{text}");
            assert_eq!(toml::from_str::<ShellConfig>(&text).unwrap().launcher.look.position, new);
        }
        assert_eq!(LauncherLook::default().position, LauncherPosition::BottomCenter, "the shipped drawer is unchanged");
        assert_eq!(LauncherPosition::ALL.map(|p| p.id()), LauncherPosition::IDS);
        for p in LauncherPosition::ALL {
            assert_eq!(LauncherPosition::from_id(p.id()), Some(p));
            let v: LauncherPosition = serde_json::from_value(serde_json::json!(p.id())).unwrap();
            assert_eq!(v, p);
        }
        assert_eq!(LauncherPosition::from_id("Bottom"), Some(LauncherPosition::BottomCenter));
        assert_eq!(LauncherPosition::from_id("top_middle"), Some(LauncherPosition::TopCenter));
        assert_eq!(LauncherPosition::from_id("left"), None);
        assert_eq!(LauncherPosition::TopRight.edge(), Some(LauncherEdge::Top));
        assert_eq!(LauncherPosition::BottomLeft.align(), LauncherAlign::Left);
        assert_eq!(LauncherPosition::Center.edge(), None);
    }

    #[test]
    fn launcher_size_follows_the_placement_and_clamps() {
        let mut look = LauncherLook::default();
        assert_eq!(look.size(), (defaults::LAUNCHER_WIDTH_PX, defaults::LAUNCHER_HEIGHT_PX));
        look.position = LauncherPosition::Center;
        assert_eq!(look.size(), (defaults::LAUNCHER_CENTER_WIDTH_PX, defaults::LAUNCHER_CENTER_HEIGHT_PX));
        assert!(look.size().0 > LauncherLook::default().size().0, "the center card is larger");
        look.center_width = 9999.0;
        look.center_height = f32::NAN;
        assert_eq!(look.size(), (defaults::LAUNCHER_CENTER_WIDTH_RANGE_PX.1, defaults::LAUNCHER_CENTER_HEIGHT_PX));
    }

    #[test]
    fn classic_is_the_default_look_and_every_look_round_trips() {
        let default = LauncherLook::default();
        assert_eq!(LauncherLookPreset::active(&default).map(|l| l.id), Some("classic"), "today's launcher is Classic");
        let ids: Vec<&str> = LAUNCHER_LOOKS.iter().map(|l| l.id).collect();
        assert_eq!(ids, ["classic", "grid", "beacon"]);
        for preset in &LAUNCHER_LOOKS {
            let mut look = default.clone();
            look.width = 700.0;
            look.ignore_mouse = true;
            preset.apply(&mut look);
            assert_eq!(LauncherLookPreset::active(&look).map(|l| l.id), Some(preset.id), "only {} matches itself", preset.id);
            assert!(look.width == 700.0 && look.ignore_mouse, "a look keeps settings it doesn't name");
            // Every value a look sets is inside its Studio range.
            assert_eq!(look.grid_icon(), preset.grid_icon_size);
            assert_eq!(look.size().0, if look.position.is_center() { preset.center_width } else { 700.0 });
            // Tweaking one value afterwards makes it a customized look.
            look.compact = !look.compact;
            assert!(LauncherLookPreset::active(&look).is_none());
            assert_eq!(LauncherLookPreset::find(&format!(" {} ", preset.id.to_uppercase())).map(|l| l.id), Some(preset.id));
        }
        assert!(LauncherLookPreset::find("spotlight").is_none());
    }

    #[test]
    fn older_launcher_configs_load_as_classic_and_new_values_clamp() {
        let cfg: ShellConfig = toml::from_str("[launcher.look]\nlayout = \"grid\"\ngrid_columns = 7\n").unwrap();
        assert_eq!(cfg.launcher.look.style, LauncherStyle::Classic);
        assert_eq!(cfg.launcher.look.grid_icon_size, defaults::LAUNCHER_GRID_ICON_PX);
        let cfg: ShellConfig = toml::from_str("[launcher.look]\nstyle = \"beacon\"\nbeacon_width = 5000.0\nbeacon_top = -1.0\ngrid_icon_size = 4.0\n").unwrap();
        let look = &cfg.launcher.look;
        assert_eq!(look.style, LauncherStyle::Beacon);
        assert_eq!(look.beacon(), (defaults::LAUNCHER_BEACON_WIDTH_RANGE_PX.1, defaults::LAUNCHER_BEACON_TOP_RANGE.0));
        assert_eq!(look.grid_icon(), defaults::LAUNCHER_GRID_ICON_RANGE_PX.0);
        let text = toml::to_string(&cfg).unwrap();
        assert_eq!(toml::from_str::<ShellConfig>(&text).unwrap().launcher.look, cfg.launcher.look);
    }

    #[test]
    fn screenshot_toolbar_settings_parse_and_clamp() {
        let cfg: ShellConfig = toml::from_str("[plugins.settings.screenshot]\ntoolbar_position = \"bottom\"\ntoolbar_scale = 9.0\n").unwrap();
        let shot = ScreenshotSettings::from_plugins(&cfg.plugins);
        assert_eq!(shot.toolbar_position, ToolbarPosition::Bottom);
        assert_eq!(shot.toolbar_scale(), defaults::SCREENSHOT_TOOLBAR_SCALE_RANGE.1);
        assert_eq!(ScreenshotSettings::default().toolbar_position, ToolbarPosition::Top);
        assert_eq!(ScreenshotSettings::default().toolbar_scale(), 1.0);
        assert_eq!(shot.flipped_position(), ToolbarPosition::Top);
    }

    #[test]
    fn screenshot_countdown_defaults_and_clamps() {
        assert_eq!(ScreenshotSettings::default().countdown_secs(), defaults::SCREENSHOT_COUNTDOWN_S);
        let cfg: ShellConfig = toml::from_str("[plugins.settings.screenshot]\ncountdown = 60\n").unwrap();
        assert_eq!(ScreenshotSettings::from_plugins(&cfg.plugins).countdown_secs(), defaults::SCREENSHOT_COUNTDOWN_RANGE_S.1);
        let cfg: ShellConfig = toml::from_str("[plugins.settings.screenshot]\ncountdown = 0\n").unwrap();
        assert_eq!(ScreenshotSettings::from_plugins(&cfg.plugins).countdown_secs(), defaults::SCREENSHOT_COUNTDOWN_RANGE_S.0);
    }

    #[test]
    fn provider_prefixes_are_distinct() {
        let p = LauncherProviders::default();
        let mut seen: Vec<&str> = p.all().iter().map(|(_, c)| c.prefix.as_str()).collect();
        seen.extend(["/", "="]);
        let n = seen.len();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), n, "prefixes collide");
    }

    #[test]
    fn screenshot_settings_tolerate_bad_values() {
        let mut plugins = crate::PluginsConfig::default();
        let table: toml::Table = toml::from_str("copy = false\nformat = \"jpeg\"\nquality = \"high\"\ndelay = 3\n").unwrap();
        plugins.settings.insert("screenshot".into(), table);
        let s = ScreenshotSettings::from_plugins(&plugins);
        assert!(!s.copy);
        assert_eq!(s.format, ShotFormat::Jpeg);
        assert_eq!(s.quality, defaults::SCREENSHOT_QUALITY, "bad value falls back");
        assert_eq!(s.delay, 3);
        assert_eq!(ScreenshotSettings::from_plugins(&crate::PluginsConfig::default()), ScreenshotSettings::default());
    }
}
