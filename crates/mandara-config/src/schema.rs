//! The `config.toml` schema. Every section is `#[serde(default)]` so partial
//! or older config files keep loading.

use std::collections::BTreeMap;
use std::path::PathBuf;

use mandara_theme::{PaletteAdjustments, ThemeSource};
use serde::{Deserialize, Serialize};

use crate::surfaces;


/// °F per °C and °F at 0 °C.
const FAHRENHEIT_PER_CELSIUS: f32 = 9.0 / 5.0;
const FAHRENHEIT_AT_ZERO_CELSIUS: f32 = 32.0;
/// Kilometres in a statute mile.
const KM_PER_MILE: f32 = 1.609_344;

/// Default values of the config schema, in one place.
pub mod defaults {
    /// Pointer dwell on a screen edge before its overlay opens, ms.
    pub const EDGE_DWELL_MS: u64 = 140;
    /// Left sidebar width, logical px.
    pub const SIDEBAR_WIDTH_PX: f32 = 392.0;
    /// Palette cross-fade when the theme changes, ms (CLAUDE.md: 450 ms).
    pub const THEME_TRANSITION_MS: u64 = 450;
    /// Bar geometry, logical px: height (slim, ~30 px), gaps to the screen
    /// edge and sides, pill corner radius, the flare joining a panel to the
    /// bar, and panel corner radius.
    pub const BAR_HEIGHT_PX: f32 = 30.0;
    pub const BAR_EDGE_GAP_PX: f32 = 6.0;
    pub const BAR_SIDE_GAP_PX: f32 = 8.0;
    pub const BAR_CORNER_RADIUS_PX: f32 = 15.0;
    pub const BAR_FLARE_RADIUS_PX: f32 = 18.0;
    pub const PANEL_RADIUS_PX: f32 = 26.0;
    /// Width of the polkit and keyring password dialogs, logical px.
    pub const AUTH_DIALOG_WIDTH_PX: f32 = 440.0;
    /// Bounds of [`AUTH_DIALOG_WIDTH_PX`].
    pub const AUTH_DIALOG_WIDTH_RANGE: (f32, f32) = (360.0, 640.0);
    /// Spring physics (CLAUDE.md defaults).
    pub const SPRING_STIFFNESS: f32 = 280.0;
    pub const SPRING_DAMPING: f32 = 24.0;
    pub const SPRING_MASS: f32 = 1.0;
    /// Wallpaper picker: magnification of the focused thumbnail.
    pub const WALLPAPER_PICKER_FOCUS: f32 = 2.6;
    /// Desktop clock position as fractions of the screen.
    pub const DESKTOP_CLOCK_X: f32 = 0.5;
    pub const DESKTOP_CLOCK_Y: f32 = 0.32;
    /// Wallpaper cross-fade, ms.
    pub const WALLPAPER_CROSSFADE_MS: u64 = 700;
    /// Launcher results shown.
    pub const LAUNCHER_MAX_RESULTS: usize = 7;
    /// Regular body text weight (CSS scale).
    pub const FONT_WEIGHT_REGULAR: u16 = 400;
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellConfig {
    pub general: GeneralConfig,
    pub appearance: AppearanceConfig,
    pub theme: ThemeConfig,
    pub bar: BarConfig,
    pub motion: MotionConfig,
    pub typography: TypographyConfig,
    pub wallpaper: WallpaperConfig,
    pub launcher: LauncherConfig,
    /// The app dock (`crate::workflow`).
    pub dock: crate::workflow::DockConfig,
    /// The Alt-Tab window switcher.
    pub switcher: crate::workflow::SwitcherConfig,
    /// Screen time / digital wellbeing.
    pub screen_time: crate::workflow::ScreenTimeConfig,
    /// Clipboard history.
    pub clipboard: crate::workflow::ClipboardConfig,
    pub integrations: IntegrationsConfig,
    pub plugins: PluginsConfig,
    pub sidebar: SidebarConfig,
    /// Drop-down panels opened from the bar (`crate::surfaces`).
    pub panels: crate::surfaces::PanelsConfig,
    pub edges: EdgeTriggers,
    pub compositor: crate::compositor::CompositorConfig,
    /// Screen colour temperature (`crate::outputs`).
    pub night_light: crate::outputs::NightLightConfig,
    /// Display profiles, surface placement per output, external brightness.
    pub displays: crate::outputs::DisplaysConfig,
    /// Idle stages and lock behaviour (`crate::power`).
    pub idle: crate::power::IdleConfig,
    /// Battery alerts and automatic power profiles.
    pub power: crate::power::PowerConfig,
    /// The power / session menu.
    pub session_menu: crate::power::SessionMenuConfig,
    /// Commands run on shell events.
    pub hooks: crate::power::HooksConfig,
    /// Clock / date formats (`crate::desktop`).
    pub datetime: crate::desktop::DateTimeConfig,
    /// ICS calendar subscriptions and reminders.
    pub calendar: crate::desktop::CalendarConfig,
    /// The location shared by weather, prayer times and schedules.
    pub location: crate::desktop::LocationConfig,
    pub weather: crate::desktop::WeatherConfig,
    /// The lock screen's look.
    pub lock: crate::desktop::LockConfig,
    pub hot_corners: crate::desktop::HotCornersConfig,
    pub screen_corners: crate::desktop::ScreenCornersConfig,
    /// Interface style: radii, shadows, borders.
    pub style: crate::desktop::StyleConfig,
    /// Notification popups, history, rules and quiet modes (`crate::alerts`).
    pub notifications: crate::alerts::NotificationsConfig,
    /// On-screen display kinds and look.
    pub osd: crate::alerts::OsdConfig,
    /// Volume step, over-amplification, device names, per-app outputs.
    pub audio: crate::alerts::AudioConfig,
    /// Media player preferences.
    pub media: crate::alerts::MediaConfig,
    /// Shell event sounds.
    pub sounds: crate::alerts::SoundsConfig,
    /// Microphone / camera / screen-share indicators.
    pub privacy: crate::alerts::PrivacyConfig,
    /// Preferred terminal / file manager / browser (`crate::apps`).
    pub apps: crate::apps::AppsConfig,
    /// VPN and hotspot preferences (`crate::connectivity`).
    pub network: crate::connectivity::NetworkConfig,
    /// Pairing agent, auto-connect, adapter.
    pub bluetooth: crate::connectivity::BluetoothConfig,
    /// Mandara's own updates and marketplace plugin updates.
    pub updates: UpdatesConfig,
    /// Pacing of the GPU audio visualizer cards (`crate::desktop_cards`).
    pub visualizers: crate::desktop_cards::VisualizersConfig,
    /// The one-click looks gallery (`crate::looks`).
    pub looks: crate::looks::LooksState,
    /// Widgets painted on the wallpaper layer (`crate::desktop_widgets`).
    pub desktop: crate::desktop_widgets::DesktopConfig,
}

impl ShellConfig {
    /// `true` when the sidebar sits on the right edge (and the session dock
    /// therefore on the left).
    pub fn sidebar_on_right(&self) -> bool {
        match self.sidebar.side {
            SidebarSide::Left => false,
            SidebarSide::Right => true,
            SidebarSide::Auto => self.general.locale.is_arabic(),
        }
    }

    /// The dock's settings as drawn: sanitized, and moved to the bottom
    /// edge while a side bar holds its edge (the two can't share one).
    pub fn dock_sanitized(&self) -> crate::dock::DockConfig {
        use crate::dock::DockPosition;
        let mut d = self.dock.sanitized();
        let taken = match self.bar.position {
            BarPosition::Left => d.position == DockPosition::Left,
            BarPosition::Right => d.position == DockPosition::Right,
            _ => false,
        };
        if taken {
            d.position = DockPosition::Bottom;
        }
        d
    }

    /// The compositor settings as emitted: `look.gaps_follow_bar` resolved
    /// to the bar's side gap, so windows keep the same spacing from the
    /// screen edges and each other as the bar keeps from the screen.
    ///
    /// Mandara's hot corners also switch the compositor's own hot corner
    /// off while any corner is active (see
    /// [`crate::desktop::HotCornersConfig::any_active`]).
    pub fn effective_compositor(&self) -> std::borrow::Cow<'_, crate::compositor::CompositorConfig> {
        let hot_corners = self.hot_corners.any_active();
        // Cursor themes are a plugin: switched off, Mandara leaves the
        // cursor configuration entirely to the compositor and the user.
        let cursor_off = !self.cursors_enabled() && self.compositor.input.cursor != crate::input::CursorInput::default();
        if !self.compositor.look.gaps_follow_bar && !hot_corners && !cursor_off {
            return std::borrow::Cow::Borrowed(&self.compositor);
        }
        let mut c = self.compositor.clone();
        if cursor_off {
            c.input.cursor = crate::input::CursorInput::default();
        }
        if self.compositor.look.gaps_follow_bar {
            let gap = self.bar.side_gap.max(0.0).round() as u32;
            c.look.gaps_outer = Some(gap);
            c.look.gaps_inner = Some(gap);
        }
        c.disable_builtin_hot_corners |= hot_corners;
        std::borrow::Cow::Owned(c)
    }

    /// Whether the opt-in cursor themes plugin is on (its cursor settings
    /// are emitted and applied only then).
    pub fn cursors_enabled(&self) -> bool {
        self.plugins.opted_in(crate::upgrade::CURSORS_ID)
    }

    /// Whether the opt-in Wallhaven plugin is on (online wallpapers in the
    /// picker; no Wallhaven request is made otherwise).
    pub fn wallhaven_enabled(&self) -> bool {
        self.plugins.opted_in(crate::upgrade::WALLHAVEN_ID)
    }

    /// The layout list behind `lane`.
    pub fn layout_lane(&self, lane: LayoutLane) -> &Vec<String> {
        match lane {
            LayoutLane::BarLeft => &self.bar.left,
            LayoutLane::BarCenter => &self.bar.center,
            LayoutLane::BarRight => &self.bar.right,
            LayoutLane::Sidebar => self.sidebar.tabs.first().map_or(&NO_CARDS, |t| &t.cards),
        }
    }

    pub fn layout_lane_mut(&mut self, lane: LayoutLane) -> &mut Vec<String> {
        match lane {
            LayoutLane::BarLeft => &mut self.bar.left,
            LayoutLane::BarCenter => &mut self.bar.center,
            LayoutLane::BarRight => &mut self.bar.right,
            LayoutLane::Sidebar => {
                if self.sidebar.tabs.is_empty() {
                    self.sidebar.normalize_tabs();
                }
                &mut self.sidebar.tabs[0].cards
            }
        }
    }

    /// The list a slot points into (a sidebar slot names its tab; a tab
    /// that no longer exists falls back to the last one).
    fn slot_list_mut(&mut self, slot: &LayoutSlot) -> &mut Vec<String> {
        if slot.lane != LayoutLane::Sidebar {
            return self.layout_lane_mut(slot.lane);
        }
        if self.sidebar.tabs.is_empty() {
            self.sidebar.normalize_tabs();
        }
        let tab = slot.tab.min(self.sidebar.tabs.len() - 1);
        &mut self.sidebar.tabs[tab].cards
    }

    /// Every place `id` sits in the bar and sidebar layouts (every sidebar
    /// tab that shows it).
    pub fn plugin_places(&self, id: &str) -> Vec<LayoutSlot> {
        let bar = LayoutLane::ALL
            .iter()
            .filter(|&&lane| lane != LayoutLane::Sidebar)
            .filter_map(|&lane| self.layout_lane(lane).iter().position(|p| p == id).map(|index| LayoutSlot::new(lane, index)));
        let tabs = self.sidebar.tabs.iter().enumerate().filter_map(|(tab, t)| {
            t.cards.iter().position(|p| p == id).map(|index| LayoutSlot { tab, ..LayoutSlot::new(LayoutLane::Sidebar, index) })
        });
        bar.chain(tabs).collect()
    }

    /// Takes `id` out of every layout list, returning where it was.
    pub fn take_plugin_places(&mut self, id: &str) -> Vec<LayoutSlot> {
        let places = self.plugin_places(id);
        for lane in LayoutLane::ALL {
            if lane != LayoutLane::Sidebar {
                self.layout_lane_mut(lane).retain(|p| p != id);
            }
        }
        for tab in &mut self.sidebar.tabs {
            tab.cards.retain(|p| p != id);
        }
        places
    }

    /// Puts `id` back at `places` (indices clamped to the list's current
    /// length; lists that already hold it are left alone).
    pub fn put_plugin_places(&mut self, id: &str, places: &[LayoutSlot]) {
        for slot in places {
            let list = self.slot_list_mut(slot);
            if !list.iter().any(|p| p == id) {
                list.insert(slot.index.min(list.len()), id.to_string());
            }
        }
    }

    /// Where `id` sits in the default layout (empty for plugins the
    /// default bar and sidebar don't show).
    pub fn default_plugin_places(id: &str) -> Vec<LayoutSlot> {
        ShellConfig::default().plugin_places(id)
    }

    /// Whether the app dock (the opt-in `dock` plugin) is switched on.
    pub fn dock_enabled(&self) -> bool {
        let id = crate::workflow::DOCK_PLUGIN_ID;
        !self.plugins.disabled.iter().any(|p| p == id) && !self.plugins.is_removed(id) && self.plugins.enabled.iter().any(|p| p == id)
    }

    /// Whether resting the pointer on the bottom edge opens the launcher.
    /// While the dock is on, that edge belongs to the dock, so the launcher
    /// becomes click-only (the dock's launcher button, the bar, keybinds).
    pub fn launcher_hover_enabled(&self) -> bool {
        self.launcher.enabled && self.edges.launcher_bottom && !self.dock_enabled()
    }

    /// Room a launcher drawer on the bottom edge keeps free for the dock
    /// (its band plus the same gap above it), or `None` when no dock sits on
    /// the bottom edge.
    pub fn dock_bottom_clearance(&self) -> Option<f32> {
        (self.dock_enabled() && self.dock.position == crate::workflow::DockPosition::Bottom).then(|| self.dock.band() + self.dock.margin)
    }

    /// Whether the bar shows on `output`: `[displays.surfaces.bar]`, or the
    /// legacy `bar.monitors` list while the assignment is still at its
    /// default.
    pub fn bar_on_output(&self, output: &str) -> bool {
        use crate::outputs::{ScreenSet, SurfaceKind, assignment};
        if self.displays.surfaces.bar == ScreenSet::default() {
            let mut legacy = legacy_list(&self.bar.monitors).peekable();
            if legacy.peek().is_some() {
                return legacy.any(|m| m.eq_ignore_ascii_case(output.trim()));
            }
        }
        assignment(&self.displays, SurfaceKind::Bar, output)
    }

    /// Whether the dock shows on `output` (`index` in output order):
    /// `[displays.surfaces.dock]`, or the dock's own `displays` / `monitors`
    /// while the assignment is still at its default.
    pub fn dock_on_output(&self, output: &str, index: usize) -> bool {
        use crate::outputs::{ScreenSet, SurfaceKind, assignment};
        if self.displays.surfaces.dock == ScreenSet::default() {
            return self.dock.shows_on(output, index);
        }
        assignment(&self.displays, SurfaceKind::Dock, output)
    }

    /// Moves per-surface output settings that predate `[displays.surfaces]`
    /// into it: `bar.monitors` and the dock's listed displays (and its
    /// "first display" once a main display is set). A surface whose
    /// assignment was already chosen keeps it. Returns whether anything
    /// moved.
    pub fn migrate_legacy(&mut self) -> bool {
        use crate::outputs::{ScreenChoice, ScreenSet};
        use crate::workflow::DockDisplays;
        let mut moved = false;
        let listed: Vec<String> = legacy_list(&self.bar.monitors).map(str::to_string).collect();
        if !listed.is_empty() && self.displays.surfaces.bar == ScreenSet::default() {
            self.displays.surfaces.bar = ScreenSet { show_on: ScreenChoice::Selected, outputs: listed };
            moved = true;
        }
        if !self.bar.monitors.is_empty() && self.displays.surfaces.bar != ScreenSet::default() {
            self.bar.monitors.clear();
            moved = true;
        }
        if self.displays.surfaces.dock == ScreenSet::default() {
            let dock = &mut self.dock;
            let set = match dock.displays {
                DockDisplays::Listed if !dock.monitors.is_empty() => Some(ScreenSet { show_on: ScreenChoice::Selected, outputs: dock.monitors.clone() }),
                // "First display" only maps onto "main display" once one is set.
                DockDisplays::Primary if self.displays.primary.is_some() => Some(ScreenSet { show_on: ScreenChoice::Primary, outputs: Vec::new() }),
                _ => None,
            };
            if let Some(set) = set {
                self.displays.surfaces.dock = set;
                dock.displays = DockDisplays::All;
                dock.monitors.clear();
                moved = true;
            }
        }
        moved
    }

    /// The launch options in effect: the launcher's own terminal, else the
    /// preferred terminal from `[apps]` (a desktop id resolves to its
    /// program).
    pub fn launch_options(&self) -> crate::workflow::LaunchOptions {
        let mut options = self.launcher.launch.clone();
        if options.terminal.trim().is_empty()
            && let Some(term) = crate::apps::resolve_command(&self.apps.terminal)
        {
            options.terminal = term;
        }
        options
    }

    /// The measurement system in effect. Older configs carried a weather-only
    /// `[plugins.settings.weather] fahrenheit = true`; with the global setting
    /// still at its default that legacy flag is honoured as Imperial.
    pub fn units(&self) -> Units {
        match self.general.units {
            Units::Metric if self.plugins.setting("weather", "fahrenheit", false) => Units::Imperial,
            units => units,
        }
    }
}

/// Entries of a comma-separated output list, trimmed, empties dropped.
fn legacy_list(text: &str) -> impl Iterator<Item = &str> {
    text.split(',').map(str::trim).filter(|m| !m.is_empty())
}

/// Screen-edge hover triggers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EdgeTriggers {
    /// Open the launcher when the pointer rests on the bottom edge.
    pub launcher_bottom: bool,
    /// Open the power/session dock from its edge — the one opposite the
    /// sidebar (MangoWM and Hyprland; Niri scrolls with screen edges).
    pub session_right: bool,
    /// Pointer dwell before an edge triggers, in milliseconds.
    pub dwell_ms: u64,
}

impl Default for EdgeTriggers {
    fn default() -> Self {
        Self { launcher_bottom: true, session_right: true, dwell_ms: defaults::EDGE_DWELL_MS }
    }
}

/// Which screen edge the sidebar lives on. The session dock always takes
/// the opposite edge.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SidebarSide {
    /// Left for left-to-right locales, right for Arabic (RTL).
    #[default]
    Auto,
    Left,
    Right,
}

/// How the sidebar shares the screen with windows.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SidebarMode {
    /// Floats over windows while open and reserves nothing, so opening it
    /// never makes the compositor re-tile (the smoothest).
    #[default]
    Float,
    /// Pushes windows aside while open and gives the space back on close.
    Push,
    /// Always open, with its width reserved.
    Pinned,
}

impl SidebarMode {
    /// Whether an open sidebar reserves its width as an exclusive zone.
    pub fn reserves_space(self) -> bool {
        matches!(self, Self::Push | Self::Pinned)
    }
}

/// The auto-hiding sidebar drawer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, from = "SidebarConfigFile")]
pub struct SidebarConfig {
    pub enabled: bool,
    pub side: SidebarSide,
    /// Reveal when the pointer touches the sidebar's screen edge.
    pub hover_reveal: bool,
    pub width: f32,
    /// Float over windows, push them aside while open, or stay pinned.
    pub mode: SidebarMode,
    /// The mode the sidebar's pin button returns to when unpinning.
    pub unpinned_mode: SidebarMode,
    /// The tabs of plugin cards, in strip order (see `crate::sidebar`).
    /// Older configs' single `cards = [...]` list reads as the first tab.
    pub tabs: Vec<crate::sidebar::SidebarTab>,
    /// Notifications get their own built-in sidebar tab (after the user's
    /// tabs) instead of a card.
    pub notifications_tab: bool,
    /// Every opening shows the first tab instead of the one used last.
    pub always_first_tab: bool,
    /// The "+" at the end of the tab strip that makes a new tab in place.
    pub add_tab_button: bool,
    /// Shape, spacing and motion (shared by the power dock).
    pub look: crate::surfaces::SidebarLook,
}

impl SidebarConfig {
    /// Kept open with its space reserved.
    pub fn pinned(&self) -> bool {
        self.mode == SidebarMode::Pinned
    }

    /// Pins, or unpins back to the mode it had before pinning.
    pub fn set_pinned(&mut self, pinned: bool) {
        match (pinned, self.pinned()) {
            (true, false) => {
                self.unpinned_mode = self.mode;
                self.mode = SidebarMode::Pinned;
            }
            (false, true) => self.mode = if self.unpinned_mode == SidebarMode::Pinned { SidebarMode::Float } else { self.unpinned_mode },
            _ => {}
        }
    }
}

/// `[sidebar]` as written on disk: also reads the old `pinned = true`
/// (before the three modes) as [`SidebarMode::Pinned`].
#[derive(Deserialize)]
#[serde(default)]
struct SidebarConfigFile {
    enabled: bool,
    side: SidebarSide,
    hover_reveal: bool,
    width: f32,
    mode: Option<SidebarMode>,
    unpinned_mode: SidebarMode,
    /// Pre-mode configs: `true` meant pinned.
    pinned: Option<bool>,
    /// Pre-tab configs: the one card list (becomes the Overview tab).
    cards: Option<Vec<String>>,
    tabs: Option<Vec<crate::sidebar::SidebarTab>>,
    notifications_tab: bool,
    always_first_tab: bool,
    add_tab_button: bool,
    look: crate::surfaces::SidebarLook,
}

impl Default for SidebarConfigFile {
    fn default() -> Self {
        let d = SidebarConfig::default();
        Self {
            enabled: d.enabled,
            side: d.side,
            hover_reveal: d.hover_reveal,
            width: d.width,
            mode: None,
            unpinned_mode: d.unpinned_mode,
            pinned: None,
            cards: None,
            tabs: None,
            notifications_tab: d.notifications_tab,
            always_first_tab: d.always_first_tab,
            add_tab_button: d.add_tab_button,
            look: d.look,
        }
    }
}

impl From<SidebarConfigFile> for SidebarConfig {
    fn from(f: SidebarConfigFile) -> Self {
        let mode = f.mode.unwrap_or(if f.pinned == Some(true) { SidebarMode::Pinned } else { SidebarMode::Float });
        let tabs = match (f.tabs, f.cards) {
            (Some(tabs), _) if !tabs.is_empty() => tabs,
            (_, Some(cards)) => vec![crate::sidebar::SidebarTab::overview(cards)],
            _ => SidebarConfig::default().tabs,
        };
        let mut c = Self {
            enabled: f.enabled,
            side: f.side,
            hover_reveal: f.hover_reveal,
            width: f.width,
            mode,
            unpinned_mode: f.unpinned_mode,
            tabs,
            notifications_tab: f.notifications_tab,
            always_first_tab: f.always_first_tab,
            add_tab_button: f.add_tab_button,
            look: f.look,
        };
        c.normalize_tabs();
        c
    }
}

impl Default for SidebarConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            side: SidebarSide::Auto,
            hover_reveal: true,
            width: defaults::SIDEBAR_WIDTH_PX,
            mode: SidebarMode::Float,
            unpinned_mode: SidebarMode::Float,
            tabs: vec![crate::sidebar::SidebarTab::overview(ids(&["quick-settings", "weather", "media", "audio", "resources"]))],
            notifications_tab: true,
            always_first_tab: false,
            add_tab_button: true,
            look: crate::surfaces::SidebarLook::default(),
        }
    }
}

/// How app icons are drawn.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IconStyle {
    /// Monochrome symbolic icons tinted with the palette (theme permitting).
    #[default]
    Monochrome,
    /// The icon theme's full-color icons.
    Color,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceConfig {
    /// Icon theme name; `None` follows GTK's `gtk-icon-theme-name`.
    pub icon_theme: Option<String>,
    pub bar_icons: IconStyle,
    pub launcher_icons: IconStyle,
    /// The launcher button's icon: a bundled Mandara icon name, `distro`
    /// (the distribution logo), an icon-theme name, or an image path.
    pub launcher_icon: String,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self { icon_theme: None, bar_icons: IconStyle::Monochrome, launcher_icons: IconStyle::Color, launcher_icon: "grid".into() }
    }
}

/// Language for shell text.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Locale {
    /// Follow `LC_ALL` / `LC_MESSAGES` / `LANG`; English when unsupported.
    #[default]
    Auto,
    English,
    Arabic,
    Spanish,
    French,
    Chinese,
    Turkish,
    Japanese,
    German,
    Russian,
    Hindi,
}

impl Locale {
    /// The interface language (`Auto` resolved from the environment).
    pub fn lang(self) -> mandara_i18n::Lang {
        use mandara_i18n::Lang;
        match self {
            Locale::Auto => Lang::from_env(),
            Locale::English => Lang::En,
            Locale::Arabic => Lang::Ar,
            Locale::Spanish => Lang::Es,
            Locale::French => Lang::Fr,
            Locale::Chinese => Lang::Zh,
            Locale::Turkish => Lang::Tr,
            Locale::Japanese => Lang::Ja,
            Locale::German => Lang::De,
            Locale::Russian => Lang::Ru,
            Locale::Hindi => Lang::Hi,
        }
    }

    pub fn from_lang(lang: mandara_i18n::Lang) -> Locale {
        use mandara_i18n::Lang;
        match lang {
            Lang::En => Locale::English,
            Lang::Ar => Locale::Arabic,
            Lang::Es => Locale::Spanish,
            Lang::Fr => Locale::French,
            Lang::Zh => Locale::Chinese,
            Lang::Tr => Locale::Turkish,
            Lang::Ja => Locale::Japanese,
            Lang::De => Locale::German,
            Lang::Ru => Locale::Russian,
            Lang::Hi => Locale::Hindi,
        }
    }

    /// Right-to-left interface (Arabic).
    pub fn is_arabic(self) -> bool {
        self.lang().is_rtl()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub locale: Locale,
    /// 24-hour clock (`false` → 12-hour with AM/PM).
    pub clock_24h: bool,
    /// First day of the week in calendars (`monday` / `sunday` / `saturday`).
    pub week_start: WeekStart,
    /// The supervisor restarts the shell after a crash. Off: a crash (or
    /// `kill`) ends Mandara for good.
    pub auto_restart: bool,
    /// Measurement system for temperatures and speeds (weather, sensors).
    pub units: Units,
    /// On-screen displays for volume, microphone, brightness, keyboard
    /// layout and Caps Lock changes.
    pub osd: bool,
    /// Where on-screen displays appear.
    pub osd_position: OsdPosition,
    /// Mandara's polkit authentication agent answers admin-password prompts
    /// (only registered when no other agent is running).
    pub polkit_agent: bool,
    /// Mandara answers gnome-keyring's unlock / new-keyring password prompts
    /// (the Secret Service system prompter); skipped while another
    /// prompter such as GNOME Shell owns the name.
    pub keyring_prompt: bool,
    /// Width of the polkit and keyring password dialogs (logical px).
    pub auth_dialog_width: f32,
    /// Mandara's KeePassXC unlock dialog (quick settings, launcher,
    /// `mandara msg keepassxc unlock`). The password is handed to
    /// KeePassXC over the session bus in clear text (only programs running
    /// as you can read it).
    pub keepassxc_prompt: bool,
    /// After login, ask for the KeePassXC password when KeePassXC is
    /// running with a locked database.
    pub keepassxc_ask_at_login: bool,
    /// Logind lock requests (`loginctl lock-session`, idle daemons, the
    /// session menu's Lock) open Mandara's lock screen.
    pub lock_screen: bool,
    /// Offline mode: Mandara makes no network requests of its own (weather,
    /// lyrics, Wallhaven, marketplace, public IP, update news…).
    pub offline: bool,
}

impl GeneralConfig {
    /// [`Self::auth_dialog_width`] clamped to its range.
    pub fn auth_dialog_width(&self) -> f32 {
        let (lo, hi) = defaults::AUTH_DIALOG_WIDTH_RANGE;
        if self.auth_dialog_width.is_finite() { self.auth_dialog_width.clamp(lo, hi) } else { defaults::AUTH_DIALOG_WIDTH_PX }
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            locale: Locale::Auto,
            clock_24h: true,
            week_start: WeekStart::Monday,
            auto_restart: true,
            units: Units::Metric,
            osd: true,
            osd_position: OsdPosition::Bottom,
            polkit_agent: true,
            keyring_prompt: true,
            auth_dialog_width: defaults::AUTH_DIALOG_WIDTH_PX,
            keepassxc_prompt: true,
            keepassxc_ask_at_login: false,
            lock_screen: true,
            offline: false,
        }
    }
}

/// Mandara's own updates (`mandara update`, Studio → About): binary
/// patches first, the full download as the fallback.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdatesConfig {
    /// Check for new Mandara releases in the background and announce each
    /// new version once (a notification; quiet modes apply).
    pub self_check: bool,
    /// Hours between automatic checks (at least one).
    pub check_interval_hours: u32,
    /// Also check installed marketplace plugins.
    pub plugins: bool,
}

impl UpdatesConfig {
    /// Default hours between automatic checks.
    pub const DEFAULT_INTERVAL_HOURS: u32 = 6;
    const SECS_PER_HOUR: u64 = 3600;

    /// The interval between automatic checks.
    pub fn interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.check_interval_hours.max(1)) * Self::SECS_PER_HOUR)
    }
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self { self_check: true, check_interval_hours: Self::DEFAULT_INTERVAL_HOURS, plugins: true }
    }
}

/// Screen position of on-screen displays (always clear of the bar).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OsdPosition {
    #[default]
    Bottom,
    Top,
    Center,
    /// Vertical pill on the left / right edge.
    Left,
    Right,
}

impl OsdPosition {
    /// Left / right OSDs are drawn as vertical pills.
    pub fn is_vertical(self) -> bool {
        matches!(self, OsdPosition::Left | OsdPosition::Right)
    }
}

/// Measurement system: Metric (°C, km/h) or Imperial (°F, mph).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Units {
    #[default]
    Metric,
    Imperial,
}

impl Units {
    pub fn is_imperial(self) -> bool {
        self == Units::Imperial
    }

    /// A Celsius reading in this system: `(value, "°C" | "°F")`.
    pub fn temperature(self, celsius: f32) -> (f32, &'static str) {
        match self {
            Units::Metric => (celsius, "°C"),
            Units::Imperial => (celsius * FAHRENHEIT_PER_CELSIUS + FAHRENHEIT_AT_ZERO_CELSIUS, "°F"),
        }
    }

    /// A rounded temperature with its unit (`23°C`, `73°F`).
    pub fn format_temperature(self, celsius: f32) -> String {
        let (v, unit) = self.temperature(celsius);
        format!("{}{unit}", v.round() as i32)
    }

    /// A rounded temperature with a bare degree sign (`23°`), for compact
    /// readouts where the unit is clear from context.
    pub fn format_degrees(self, celsius: f32) -> String {
        format!("{}°", self.temperature(celsius).0.round() as i32)
    }

    /// A km/h speed in this system: `(value, "km/h" | "mph")`.
    pub fn speed(self, kmh: f32) -> (f32, &'static str) {
        match self {
            Units::Metric => (kmh, "km/h"),
            Units::Imperial => (kmh / KM_PER_MILE, "mph"),
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WeekStart {
    #[default]
    Monday,
    Sunday,
    Saturday,
}

/// Plugin enablement and per-plugin settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginsConfig {
    /// The built-in default set this table was written for (see
    /// [`crate::upgrade`]). Missing in a file written before the field
    /// existed (0.1: almost every built-in on), so the upgrade can keep
    /// those plugins on; a fresh config carries the current version.
    #[serde(default)]
    pub defaults_version: u32,
    /// Third-party (or default-off) plugins the user switched on.
    pub enabled: Vec<String>,
    /// Plugins the user switched off (including shipped ones).
    pub disabled: Vec<String>,
    /// Built-in plugins the user removed ("uninstalled"): off, taken out of
    /// every layout and listed under "Removed Mandara plugins" until
    /// restored. Built-ins are compiled in, so this is how they go away.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    /// Where each removed plugin sat, so restoring puts it back there.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub removed_places: BTreeMap<String, Vec<LayoutSlot>>,
    /// Free-form settings tables keyed by plugin id, e.g.
    /// `[plugins.settings.prayer-times] latitude = 31.2`.
    pub settings: BTreeMap<String, toml::Table>,
}

/// A layout list that holds plugin ids.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LayoutLane {
    BarLeft,
    BarCenter,
    BarRight,
    Sidebar,
}

impl LayoutLane {
    pub const ALL: [LayoutLane; 4] = [LayoutLane::BarLeft, LayoutLane::BarCenter, LayoutLane::BarRight, LayoutLane::Sidebar];
}

/// One position of a plugin in a layout list.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutSlot {
    pub lane: LayoutLane,
    pub index: usize,
    /// The sidebar tab (index into `sidebar.tabs`) of a sidebar slot.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tab: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// The sidebar lane of a config without tabs (never the case once loaded).
static NO_CARDS: Vec<String> = Vec::new();

impl LayoutSlot {
    /// A slot in a bar lane, or in the first sidebar tab.
    pub const fn new(lane: LayoutLane, index: usize) -> Self {
        Self { lane, index, tab: 0 }
    }
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            defaults_version: crate::upgrade::PLUGIN_DEFAULTS_VERSION,
            enabled: Vec::new(),
            disabled: Vec::new(),
            removed: Vec::new(),
            removed_places: BTreeMap::new(),
            settings: BTreeMap::new(),
        }
    }
}

impl PluginsConfig {
    /// Whether the opt-in plugin `id` is switched on: listed as enabled and
    /// neither disabled nor removed. Only meaningful for plugins that ship
    /// switched off (for the rest, ask the plugin registry), and usable
    /// where the registry isn't, such as the compositor emitters.
    pub fn opted_in(&self, id: &str) -> bool {
        self.enabled.iter().any(|e| e == id) && !self.disabled.iter().any(|d| d == id) && !self.is_removed(id)
    }

    /// Whether `id` was removed (see [`PluginsConfig::removed`]).
    pub fn is_removed(&self, id: &str) -> bool {
        self.removed.iter().any(|r| r == id)
    }

    /// Reads a typed setting for `plugin`, falling back to `default`.
    pub fn setting<T: serde::de::DeserializeOwned>(&self, plugin: &str, key: &str, default: T) -> T {
        self.settings
            .get(plugin)
            .and_then(|t| t.get(key))
            .and_then(|v| v.clone().try_into().ok())
            .unwrap_or(default)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    /// Light / dark appearance for preset themes (wallpaper sources carry
    /// their own `dark` flag).
    pub mode: ThemeMode,
    pub source: ThemeSource,
    pub adjustments: PaletteAdjustments,
    /// Crossfade length when the palette changes, in milliseconds.
    pub transition_ms: u64,
    /// The catalog theme in use before switching to wallpaper colors, so
    /// turning "match colors" off restores it.
    pub last_preset: Option<String>,
    /// Who decides `mode`: you, the desktop's preference, or the clock.
    pub auto: ThemeAuto,
    /// Daily schedule (`HH:MM`, local time) for [`ThemeAuto::Schedule`].
    pub light_at: String,
    pub dark_at: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            mode: ThemeMode::Dark,
            source: ThemeSource::default(),
            adjustments: PaletteAdjustments::default(),
            transition_ms: defaults::THEME_TRANSITION_MS,
            last_preset: None,
            auto: ThemeAuto::Manual,
            light_at: "07:00".into(),
            dark_at: "19:00".into(),
        }
    }
}

/// Automatic light/dark switching. Automatic changes happen only when the
/// desired mode changes, so a manual toggle in between is kept until then.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeAuto {
    #[default]
    Manual,
    /// Follow the desktop's color-scheme preference (XDG portal).
    System,
    /// Light from `light_at` until `dark_at`, every day.
    Schedule,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

impl ThemeMode {
    pub fn is_dark(self) -> bool {
        matches!(self, ThemeMode::Dark)
    }

    pub fn toggled(self) -> Self {
        match self {
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Dark,
        }
    }
}

/// Visual arrangement of the top bar.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BarLayout {
    /// A slim full-width frame flush with the screen edge; panels pour out of
    /// it with concave flares (the default).
    #[default]
    MinimalEdge,
    /// Separate chromatic-glass pills floating under the screen edge.
    FloatingPills,
    /// One contiguous island that morphs in place.
    ContiguousIsland,
    /// A thin full-width strip (`bar.compact_height`, ~24 px) dense with
    /// mini stats, optionally underlined by a 1 px accent line.
    Hairline,
    /// A single tiny floating pill at the middle of the edge.
    Micro,
}

impl BarLayout {
    pub const ALL: [BarLayout; 5] = [BarLayout::MinimalEdge, BarLayout::FloatingPills, BarLayout::ContiguousIsland, BarLayout::Hairline, BarLayout::Micro];

    pub fn display_name(self) -> &'static str {
        match self {
            BarLayout::MinimalEdge => "Edge Frame",
            BarLayout::FloatingPills => "Glass Pills",
            BarLayout::ContiguousIsland => "Island",
            BarLayout::Hairline => "Hairline",
            BarLayout::Micro => "Micro Pill",
        }
    }

    /// Whether panels hang from the bar (vs. morphing out of a floating pill).
    pub fn is_attached(self) -> bool {
        matches!(self, BarLayout::MinimalEdge | BarLayout::Hairline)
    }

    /// Styles drawn at `bar.compact_height` instead of `bar.height`.
    pub fn is_compact(self) -> bool {
        matches!(self, BarLayout::Hairline | BarLayout::Micro)
    }

    pub fn next(self) -> BarLayout {
        let i = BarLayout::ALL.iter().position(|l| *l == self).unwrap_or(0);
        BarLayout::ALL[(i + 1) % BarLayout::ALL.len()]
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BarPosition {
    #[default]
    Top,
    Bottom,
    /// A slim vertical bar along the left edge (`bar.vertical_width` wide;
    /// drawn as the edge frame whatever `bar.layout` says).
    Left,
    Right,
}

impl BarPosition {
    pub const ALL: [BarPosition; 4] = [BarPosition::Top, BarPosition::Bottom, BarPosition::Left, BarPosition::Right];

    /// A side bar (left / right edge).
    pub fn is_vertical(self) -> bool {
        matches!(self, BarPosition::Left | BarPosition::Right)
    }

    pub fn display_name(self) -> &'static str {
        match self {
            BarPosition::Top => "Top",
            BarPosition::Bottom => "Bottom",
            BarPosition::Left => "Left",
            BarPosition::Right => "Right",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BarConfig {
    pub layout: BarLayout,
    pub position: BarPosition,
    /// Bar height in logical pixels. Mandara favours slim bars.
    pub height: f32,
    /// Gap between the screen edge and the pills.
    pub edge_gap: f32,
    /// Horizontal inset of the outermost pills.
    pub side_gap: f32,
    pub corner_radius: f32,
    /// Concave flare radius where the edge frame meets the screen sides and
    /// where panels meet the bar.
    pub flare_radius: f32,
    /// Bottom corner radius of panels / the expanded dashboard.
    pub panel_radius: f32,
    pub show_seconds: bool,
    /// Plugin ids per zone, in order.
    /// Opacity of the bar's own background (strip, pills, island), 0–1.
    pub opacity: f32,
    pub left: Vec<String>,
    pub center: Vec<String>,
    pub right: Vec<String>,
    /// Slide the bar away (always / while windows overlap / fullscreen).
    pub auto_hide: crate::desktop::BarAutoHide,
    /// Clicks on empty bar space reach the windows below (no main menu on
    /// right-click there).
    pub click_through: bool,
    /// Keep the bar above fullscreen windows (overlay layer; applies after
    /// a restart).
    pub over_fullscreen: bool,
    /// Legacy: outputs (connector names, comma-separated) that showed the
    /// bar. Superseded by `[displays.surfaces.bar]`; still honoured while
    /// that is at its default ([`ShellConfig::bar_on_output`]) and moved
    /// there on load ([`ShellConfig::migrate_legacy`]).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub monitors: String,
    /// Horizontal padding inside each chip, around the plugin's content.
    pub chip_padding: f32,
    /// Gap between neighbouring chips of a zone.
    pub chip_spacing: f32,
    /// Share of the accent mixed into the bar fill (0 = neutral surface).
    pub tint: f32,
    /// Width of the specular glass rim (pills, island, floating frame).
    pub rim_width: f32,
    /// Strength of the rim's highlight and shade.
    pub rim_intensity: f32,
    /// A soft shadow under floating bar styles.
    pub shadow: bool,
    pub shadow_strength: f32,
    /// Size of bar text relative to the interface text size.
    pub text_scale: f32,
    /// Size of bar icons relative to their designed size.
    pub icon_scale: f32,
    /// Where the center zone sits.
    pub center_align: crate::surfaces::BarCenterAlign,
    /// What separates neighbouring chips.
    pub separator: crate::surfaces::BarSeparator,
    /// Edge frame only: float the frame clear of the screen edges (edge and
    /// side gaps) with rounded corners instead of spanning the edge.
    pub floating_frame: bool,
    /// Height of the compact styles (hairline, micro pill).
    pub compact_height: f32,
    /// Hairline: a 1 px accent line along the bar's inner edge.
    pub accent_line: bool,
    /// Width of a left / right bar.
    pub vertical_width: f32,
    /// A thin border around the whole display, joined to a side bar
    /// (reserves nothing beyond the bar itself).
    pub screen_frame: bool,
    pub frame_thickness: f32,
}

impl BarConfig {
    /// Corner radius of the bar's own shape for its style, before the
    /// global roundness: the pills or the island, the hover chips of the
    /// edge frame (capped at half their height), the floating frame.
    pub fn shape_radius(&self) -> f32 {
        surfaces::ranges::BAR_PILL_RADIUS.clamp(self.corner_radius, defaults::BAR_CORNER_RADIUS_PX)
    }

    /// Whether the edge frame floats clear of the screen edges.
    pub fn frame_floats(&self) -> bool {
        self.layout.is_attached() && self.floating_frame
    }

    pub fn chip_padding(&self) -> f32 {
        surfaces::ranges::BAR_CHIP_PADDING.clamp(self.chip_padding, surfaces::defaults::BAR_CHIP_PADDING_PX)
    }

    pub fn chip_spacing(&self) -> f32 {
        surfaces::ranges::BAR_CHIP_SPACING.clamp(self.chip_spacing, surfaces::defaults::BAR_CHIP_SPACING_PX)
    }

    pub fn text_scale(&self) -> f32 {
        surfaces::ranges::BAR_TEXT_SCALE.clamp(self.text_scale, 1.0)
    }

    pub fn icon_scale(&self) -> f32 {
        surfaces::ranges::BAR_ICON_SCALE.clamp(self.icon_scale, 1.0)
    }

    /// The style actually drawn: a side bar is always the edge frame.
    pub fn effective_layout(&self) -> BarLayout {
        if self.position.is_vertical() { BarLayout::MinimalEdge } else { self.layout }
    }

    pub fn compact_height(&self) -> f32 {
        surfaces::ranges::BAR_COMPACT_HEIGHT.clamp(self.compact_height, surfaces::defaults::BAR_COMPACT_HEIGHT_PX)
    }

    pub fn vertical_width(&self) -> f32 {
        surfaces::ranges::BAR_VERTICAL_WIDTH.clamp(self.vertical_width, surfaces::defaults::BAR_VERTICAL_WIDTH_PX)
    }

    /// Thickness of the screen frame (0 when it is off or the bar is not
    /// on a side).
    pub fn frame_thickness(&self) -> f32 {
        if self.screen_frame && self.position.is_vertical() {
            surfaces::ranges::SCREEN_FRAME.clamp(self.frame_thickness, surfaces::defaults::SCREEN_FRAME_PX)
        } else {
            0.0
        }
    }
}

impl Default for BarConfig {
    fn default() -> Self {
        Self {
            layout: BarLayout::default(),
            position: BarPosition::default(),
            height: defaults::BAR_HEIGHT_PX,
            edge_gap: defaults::BAR_EDGE_GAP_PX,
            side_gap: defaults::BAR_SIDE_GAP_PX,
            corner_radius: defaults::BAR_CORNER_RADIUS_PX,
            flare_radius: defaults::BAR_FLARE_RADIUS_PX,
            panel_radius: defaults::PANEL_RADIUS_PX,
            show_seconds: false,
            opacity: 1.0,
            left: ids(&["launcher", "workspaces", "active-window"]),
            center: ids(&["clock", "media"]),
            right: ids(&["weather", "resources", "clipboard", "screenshot", "tray", "network", "bluetooth", "audio", "notifications", "session"]),
            auto_hide: crate::desktop::BarAutoHide::default(),
            click_through: false,
            over_fullscreen: false,
            monitors: String::new(),
            chip_padding: surfaces::defaults::BAR_CHIP_PADDING_PX,
            chip_spacing: surfaces::defaults::BAR_CHIP_SPACING_PX,
            tint: 0.0,
            rim_width: surfaces::defaults::BAR_RIM_WIDTH_PX,
            rim_intensity: 1.0,
            shadow: false,
            shadow_strength: surfaces::defaults::BAR_SHADOW_STRENGTH,
            text_scale: 1.0,
            icon_scale: 1.0,
            center_align: crate::surfaces::BarCenterAlign::Screen,
            separator: crate::surfaces::BarSeparator::None,
            floating_frame: false,
            compact_height: surfaces::defaults::BAR_COMPACT_HEIGHT_PX,
            accent_line: true,
            vertical_width: surfaces::defaults::BAR_VERTICAL_WIDTH_PX,
            screen_frame: false,
            frame_thickness: surfaces::defaults::SCREEN_FRAME_PX,
        }
    }
}

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Damped-harmonic-oscillator parameters shared by every spring.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MotionConfig {
    pub stiffness: f32,
    pub damping: f32,
    pub mass: f32,
    /// Disables springs entirely (accessibility: reduced motion).
    pub reduce_motion: bool,
    /// Animation speed multiplier for every spring (1 = normal, 2 = twice
    /// as fast).
    pub speed: f32,
}

impl Default for MotionConfig {
    fn default() -> Self {
        Self { stiffness: defaults::SPRING_STIFFNESS, damping: defaults::SPRING_DAMPING, mass: defaults::SPRING_MASS, reduce_motion: false, speed: 1.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TypographyConfig {
    pub ui_font: String,
    pub display_font: String,
    pub arabic_font: String,
    pub mono_font: String,
    /// Global UI text scale.
    pub scale: f32,
    /// Weight of body text (CSS scale, 100–900; 400 = regular).
    pub weight: u16,
}

impl Default for TypographyConfig {
    fn default() -> Self {
        Self {
            ui_font: "Inter".into(),
            display_font: "Alexandria".into(),
            arabic_font: "Cairo".into(),
            mono_font: "JetBrains Mono".into(),
            scale: 1.0,
            weight: defaults::FONT_WEIGHT_REGULAR,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WallpaperConfig {
    /// Mandara draws the wallpaper itself (a background layer per output).
    pub enabled: bool,
    /// The wallpaper picker overlay is available (command, menu, bar item).
    pub picker: bool,
    /// Folder the picker browses (`~` allowed).
    pub directory: String,
    /// Re-derive the theme from each wallpaper you pick.
    pub match_colors: bool,
    /// How the picker lays out wallpapers.
    pub picker_style: PickerStyle,
    /// Tile size multiplier (0.6–1.6).
    pub picker_size: f32,
    /// Carousel: how many times wider the focused tile is than the others.
    pub picker_focus: f32,
    /// Show the focused wallpaper's name.
    pub picker_names: bool,
    pub path: Option<PathBuf>,
    pub depth_effect: bool,
    /// Desktop clock anchor as a fraction of the output size.
    pub clock_x: f32,
    pub clock_y: f32,
    pub clock_scale: f32,
    /// Typographic desktop clock drawn on the wallpaper (depth layer 2).
    pub desktop_clock: bool,
    pub clock_style: ClockStyle,
    /// Gregorian + Hijri date pill under the desktop clock.
    pub show_date: bool,
    /// Compact weather summary next to the date.
    pub show_weather: bool,
    /// Subtle ring around the clock reacting to the music playing.
    pub audio_ring: bool,
    /// Foreground subject mask drawn over the clock (depth layer 3).
    pub depth_mask: DepthMask,
    pub crossfade_ms: u64,
    /// Before 0.2: browse Wallhaven in the picker. Wallhaven is now the
    /// opt-in `wallhaven` plugin; [`crate::upgrade`] turns a `true` here
    /// into that plugin being on, then drops the key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wallhaven: Option<bool>,
    /// Default Wallhaven search; empty = the toplist.
    pub wallhaven_query: String,
    /// `toplist`, `date_added`, `views`, `favorites`, `random` or `relevance`.
    pub wallhaven_sorting: String,
    /// How the image fills the output, and the colour of any bands.
    pub fill: crate::desktop::WallpaperFill,
    pub fill_color: String,
    /// How a new wallpaper replaces the old one.
    pub transition: crate::desktop::WallpaperTransition,
    /// Light / dark and per-output wallpapers.
    pub variants: crate::desktop::WallpaperVariants,
    pub slideshow: crate::desktop::WallpaperSlideshow,
    /// Video wallpaper playback (pausing, frame rate).
    pub video: crate::desktop::WallpaperVideo,
}

impl Default for WallpaperConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            picker: true,
            // Your Pictures folder's Wallpapers (`~/Imagens/…` in Portuguese).
            directory: crate::paths::default_wallpaper_dir_display(),
            match_colors: false,
            picker_style: PickerStyle::Slices,
            picker_size: 1.0,
            picker_focus: defaults::WALLPAPER_PICKER_FOCUS,
            picker_names: true,
            path: None,
            depth_effect: true,
            clock_x: defaults::DESKTOP_CLOCK_X,
            clock_y: defaults::DESKTOP_CLOCK_Y,
            clock_scale: 1.0,
            desktop_clock: true,
            clock_style: ClockStyle::default(),
            show_date: true,
            show_weather: true,
            audio_ring: true,
            depth_mask: DepthMask::default(),
            crossfade_ms: defaults::WALLPAPER_CROSSFADE_MS,
            wallhaven: None,
            wallhaven_query: String::new(),
            wallhaven_sorting: "toplist".into(),
            fill: crate::desktop::WallpaperFill::default(),
            fill_color: "#000000".into(),
            transition: crate::desktop::WallpaperTransition::default(),
            variants: crate::desktop::WallpaperVariants::default(),
            slideshow: crate::desktop::WallpaperSlideshow::default(),
            video: crate::desktop::WallpaperVideo::default(),
        }
    }
}

/// Desktop clock typography.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClockStyle {
    /// Hours over minutes, large and bold (sits well behind a subject).
    #[default]
    Stacked,
    /// One light `HH:MM` line.
    Classic,
    /// Eastern Arabic numerals in the Arabic face.
    Arabic,
}

/// Where the foreground subject mask comes from.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DepthMask {
    /// Your mask when there is one, else one generated from the wallpaper.
    #[default]
    Auto,
    /// Only masks you provide (`~/.local/share/mandara/depth_masks/<stem>.png`).
    Custom,
    Off,
}

/// Wallpaper picker layout.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PickerStyle {
    /// Slanted slices around a large preview (Omarchy-style).
    #[default]
    Slices,
    /// Floating cards; the focused one large in the middle.
    Carousel,
}

/// The app launcher overlay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LauncherConfig {
    pub enabled: bool,
    /// Result rows shown at once.
    pub max_results: usize,
    /// `/trigger query` shortcuts typed in the launcher.
    pub commands: Vec<SlashCommand>,
    /// Named multi-step routines (each step a `mandara msg` command).
    pub skills: Vec<Skill>,
    /// Emoji, windows, clipboard, wallpapers, files and actions providers.
    pub providers: crate::workflow::LauncherProviders,
    /// Usage tracking and frecency ranking.
    pub usage: crate::workflow::LauncherUsage,
    /// Desktop ids listed first, in this order, when nothing is typed.
    pub pinned: Vec<String>,
    /// Desktop ids never listed.
    pub hidden: Vec<String>,
    /// Your own name / icon per desktop id.
    pub overrides: BTreeMap<String, crate::workflow::AppOverride>,
    /// Size, placement and behaviour.
    pub look: crate::workflow::LauncherLook,
    /// How apps are started (wrapper, systemd scopes, terminal).
    pub launch: crate::workflow::LaunchOptions,
}

impl Default for LauncherConfig {
    fn default() -> Self {
        let cmd = |trigger: &str, title: &str, action: &str, icon: &str| SlashCommand {
            trigger: trigger.into(),
            title: title.into(),
            description: String::new(),
            action: action.into(),
            icon: icon.into(),
        };
        Self {
            enabled: true,
            max_results: defaults::LAUNCHER_MAX_RESULTS,
            commands: vec![
                cmd("g", "Search the web", "url:https://duckduckgo.com/?q={query}", "search"),
                cmd("yt", "Search YouTube", "url:https://www.youtube.com/results?search_query={query}", "play"),
                cmd("wiki", "Search Wikipedia", "url:https://en.wikipedia.org/w/index.php?search={query}", "note"),
                cmd("gh", "Search GitHub", "url:https://github.com/search?q={query}", "grid"),
                cmd("todo", "Add a to-do", "mandara:todo add {query}", "list-check"),
                cmd("note", "New note", "mandara:notes add {query}", "note"),
            ],
            skills: vec![
                Skill {
                    name: "Focus".into(),
                    description: "Do not disturb on and a Pomodoro started".into(),
                    steps: vec!["notifications dnd on".into(), "pomodoro start".into()],
                    icon: "timer".into(),
                },
                Skill {
                    name: "Wind down".into(),
                    description: "Do not disturb off, timer reset, dark appearance".into(),
                    steps: vec!["notifications dnd off".into(), "pomodoro reset".into(), "theme mode dark".into()],
                    icon: "moon".into(),
                },
            ],
            providers: Default::default(),
            usage: Default::default(),
            pinned: Vec::new(),
            hidden: Vec::new(),
            overrides: BTreeMap::new(),
            look: Default::default(),
            launch: Default::default(),
        }
    }
}

/// A launcher slash command: typing `/trigger words…` runs `action` with
/// `{query}` replaced by the words.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SlashCommand {
    pub trigger: String,
    pub title: String,
    pub description: String,
    /// `url:<template>` (opens in the browser), `mandara:<command words>`
    /// or `exec:<shell command>`.
    pub action: String,
    /// Bundled icon name.
    pub icon: String,
}

/// A launcher skill: a named routine of Mandara commands run in order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// Each a `mandara msg` command line, e.g. `theme mode dark`.
    pub steps: Vec<String>,
    pub icon: String,
}

/// Which external apps receive generated theme files. Every one is off
/// until the user turns it on: Mandara writes nothing outside its own
/// folders (`~/.config/mandara`, `~/.cache/mandara`) unless asked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IntegrationsConfig {
    /// The defaults this table was written for (see [`crate::upgrade`]).
    /// Missing in a file written before the field existed (0.1, when most
    /// apps were themed by default); a fresh config carries the current
    /// version.
    #[serde(default)]
    pub version: u32,
    pub ghostty: bool,
    pub gtk4: bool,
    pub qt6: bool,
    pub zed: bool,
    pub fuzzel: bool,
    pub btop: bool,
    pub compositor_borders: bool,
    /// Kvantum theme files in `~/.cache/mandara/themes/kvantum/Mandara/`.
    pub kvantum: bool,
    /// Opt-in: link the Kvantum theme into `~/.config/Kvantum` and select it.
    pub kvantum_apply: bool,
    /// Terminal / editor / app colour files (written to
    /// `~/.cache/mandara/themes/` and each app's own themes folder; the
    /// app's main config only gains one include line where it supports it).
    pub kitty: bool,
    pub foot: bool,
    pub alacritty: bool,
    pub wezterm: bool,
    pub helix: bool,
    pub neovim: bool,
    /// Pywalfox-compatible colours for Firefox / Zen (`~/.cache/wal/colors.json`
    /// is never touched; see the README).
    pub firefox: bool,
    /// Vesktop / Vencord QuickCSS theme file.
    pub vesktop: bool,
    /// KDE / Plasma colour scheme (`~/.local/share/color-schemes/Mandara.colors`).
    pub kde: bool,
}

impl Default for IntegrationsConfig {
    fn default() -> Self {
        Self::all(false)
    }
}

impl IntegrationsConfig {
    /// Every integration on or off (the current defaults version).
    pub fn all(on: bool) -> Self {
        Self {
            version: crate::upgrade::INTEGRATIONS_VERSION,
            ghostty: on,
            gtk4: on,
            qt6: on,
            zed: on,
            fuzzel: on,
            btop: on,
            compositor_borders: on,
            kvantum: on,
            kvantum_apply: on,
            kitty: on,
            foot: on,
            alacritty: on,
            wezterm: on,
            helix: on,
            neovim: on,
            firefox: on,
            vesktop: on,
            kde: on,
        }
    }

    /// Whether any app outside Mandara is themed (compositor borders live in
    /// Mandara's own drop-in, so they don't count).
    pub fn any_app(&self) -> bool {
        let &Self { version: _, compositor_borders: _, ghostty, gtk4, qt6, zed, fuzzel, btop, kvantum, kvantum_apply, kitty, foot, alacritty, wezterm, helix, neovim, firefox, vesktop, kde } = self;
        [ghostty, gtk4, qt6, zed, fuzzel, btop, kvantum, kvantum_apply, kitty, foot, alacritty, wezterm, helix, neovim, firefox, vesktop, kde].into_iter().any(|on| on)
    }
}

#[cfg(test)]
mod units_tests {
    use super::*;

    #[test]
    fn temperature_and_speed_conversions() {
        assert_eq!(Units::Metric.temperature(21.5), (21.5, "°C"));
        let (f, unit) = Units::Imperial.temperature(100.0);
        assert!((f - 212.0).abs() < 1e-4 && unit == "°F");
        assert_eq!(Units::Imperial.format_temperature(-40.0), "-40°F");
        assert_eq!(Units::Metric.format_temperature(22.6), "23°C");
        assert_eq!(Units::Imperial.format_degrees(0.0), "32°");
        let (mph, unit) = Units::Imperial.speed(16.09344);
        assert!((mph - 10.0).abs() < 1e-3 && unit == "mph");
        assert_eq!(Units::Metric.speed(12.0), (12.0, "km/h"));
    }

    #[test]
    fn units_load_and_legacy_fahrenheit_migrates() {
        // Existing configs without the key still load (Metric).
        let cfg: ShellConfig = toml::from_str("[general]\nclock_24h = false\n").unwrap();
        assert_eq!(cfg.general.units, Units::Metric);
        assert_eq!(cfg.units(), Units::Metric);
        let cfg: ShellConfig = toml::from_str("[general]\nunits = \"imperial\"\n").unwrap();
        assert_eq!(cfg.units(), Units::Imperial);
        // The old weather-only toggle still means Imperial.
        let legacy: ShellConfig = toml::from_str("[plugins.settings.weather]\nfahrenheit = true\n").unwrap();
        assert_eq!(legacy.general.units, Units::Metric);
        assert_eq!(legacy.units(), Units::Imperial);
        let round = toml::to_string(&ShellConfig::default()).unwrap();
        assert!(round.contains("units = \"metric\""));
    }

    #[test]
    fn legacy_bar_monitors_are_honoured_then_migrated() {
        use crate::outputs::{ScreenChoice, SurfaceKind};
        let mut cfg: ShellConfig = toml::from_str("[bar]\nmonitors = \" eDP-1, HDMI-A-1\"\n").unwrap();
        assert!(cfg.bar_on_output("eDP-1") && cfg.bar_on_output("hdmi-a-1") && !cfg.bar_on_output("DP-1"));
        assert!(cfg.migrate_legacy());
        assert!(cfg.bar.monitors.is_empty());
        assert_eq!(cfg.displays.surfaces.get(SurfaceKind::Bar).show_on, ScreenChoice::Selected);
        assert_eq!(cfg.displays.surfaces.bar.outputs, ["eDP-1", "HDMI-A-1"]);
        assert!(cfg.bar_on_output("eDP-1") && !cfg.bar_on_output("DP-1"));
        assert!(!cfg.migrate_legacy(), "a second pass changes nothing");
        assert!(!toml::to_string(&cfg).unwrap().contains("monitors = \""), "the legacy key isn't written back");
        // An explicit assignment wins over the old list.
        let mut cfg: ShellConfig = toml::from_str("[bar]\nmonitors = \"eDP-1\"\n[displays.surfaces.bar]\nshow_on = \"selected\"\noutputs = [\"DP-1\"]\n").unwrap();
        assert!(cfg.bar_on_output("DP-1") && !cfg.bar_on_output("eDP-1"));
        cfg.migrate_legacy();
        assert_eq!(cfg.displays.surfaces.bar.outputs, ["DP-1"]);
        assert!(cfg.bar.monitors.is_empty());
    }

    #[test]
    fn dock_displays_fall_back_and_migrate() {
        use crate::outputs::ScreenChoice;
        use crate::workflow::DockDisplays;
        let mut cfg = ShellConfig::default();
        cfg.dock.displays = DockDisplays::Primary;
        assert!(cfg.dock_on_output("DP-1", 0) && !cfg.dock_on_output("DP-2", 1), "legacy: first output");
        assert!(!cfg.migrate_legacy(), "no main display yet: keep the legacy choice");
        cfg.dock.displays = DockDisplays::Listed;
        cfg.dock.monitors = vec!["HDMI-A-1".into()];
        assert!(cfg.migrate_legacy());
        assert_eq!(cfg.displays.surfaces.dock.show_on, ScreenChoice::Selected);
        assert_eq!(cfg.dock.displays, DockDisplays::All);
        assert!(cfg.dock_on_output("HDMI-A-1", 1) && !cfg.dock_on_output("DP-1", 0));
    }

    #[test]
    fn launcher_terminal_falls_back_to_the_preferred_one() {
        let mut cfg = ShellConfig::default();
        cfg.apps.terminal = "mandara-test-term".into();
        assert_eq!(cfg.launch_options().terminal, "mandara-test-term");
        cfg.launcher.launch.terminal = "foot".into();
        assert_eq!(cfg.launch_options().terminal, "foot");
        cfg.launcher.launch.terminal.clear();
        cfg.apps.terminal.clear();
        assert!(cfg.launch_options().terminal.is_empty(), "unset everywhere: $TERMINAL / detection decide");
    }
}

#[cfg(test)]
mod desktop_wiring_tests {
    use super::*;
    use crate::desktop::{Corner, CornerAction};
    use crate::workflow::{DOCK_PLUGIN_ID, DockPosition};

    fn with_dock(on: bool) -> ShellConfig {
        let mut c = ShellConfig::default();
        c.plugins.enabled.retain(|p| p != DOCK_PLUGIN_ID);
        c.plugins.disabled.retain(|p| p != DOCK_PLUGIN_ID);
        if on {
            c.plugins.enabled.push(DOCK_PLUGIN_ID.into());
        }
        c
    }

    #[test]
    fn dock_is_opt_in_and_a_disable_wins() {
        assert!(!with_dock(false).dock_enabled());
        let mut c = with_dock(true);
        assert!(c.dock_enabled());
        c.plugins.disabled.push(DOCK_PLUGIN_ID.into());
        assert!(!c.dock_enabled(), "disabled overrides enabled");
    }

    #[test]
    fn the_dock_makes_the_launcher_click_only() {
        let mut c = with_dock(false);
        c.edges.launcher_bottom = true;
        assert!(c.launcher_hover_enabled());
        c.launcher.enabled = false;
        assert!(!c.launcher_hover_enabled(), "no launcher, no edge");
        c.launcher.enabled = true;
        c.edges.launcher_bottom = false;
        assert!(!c.launcher_hover_enabled());
        let mut d = with_dock(true);
        d.edges.launcher_bottom = true;
        assert!(!d.launcher_hover_enabled(), "the dock owns the bottom edge");
        // Turning the dock off brings the user's own choice back.
        assert!(with_dock(false).launcher_hover_enabled() == ShellConfig::default().edges.launcher_bottom);
    }

    #[test]
    fn launcher_keeps_clear_of_a_bottom_dock() {
        assert_eq!(with_dock(false).dock_bottom_clearance(), None);
        let mut c = with_dock(true);
        let expected = c.dock.margin * 2.0 + c.dock.icon_size + c.dock.padding * 2.0;
        assert_eq!(c.dock_bottom_clearance(), Some(expected));
        assert_eq!(c.dock.band(), c.dock.margin + c.dock.icon_size + c.dock.padding * 2.0);
        c.dock.position = DockPosition::Left;
        assert_eq!(c.dock_bottom_clearance(), None, "a side dock leaves the bottom edge free");
    }

    #[test]
    fn active_hot_corners_turn_the_compositors_corner_off() {
        let mut c = ShellConfig::default();
        c.compositor.look.gaps_follow_bar = false;
        assert!(!c.effective_compositor().disable_builtin_hot_corners);
        assert!(matches!(c.effective_compositor(), std::borrow::Cow::Borrowed(_)));
        c.hot_corners.enabled = true;
        assert!(c.effective_compositor().disable_builtin_hot_corners);
        // Every corner set to nothing: the compositor keeps its own.
        for corner in [&mut c.hot_corners.top_left, &mut c.hot_corners.top_right, &mut c.hot_corners.bottom_left, &mut c.hot_corners.bottom_right] {
            *corner = Corner { action: CornerAction::None, command: String::new() };
        }
        assert!(!c.effective_compositor().disable_builtin_hot_corners);
        // Gaps that follow the bar still resolve alongside.
        c.hot_corners.top_left.action = CornerAction::Launcher;
        c.compositor.look.gaps_follow_bar = true;
        let eff = c.effective_compositor();
        assert!(eff.disable_builtin_hot_corners);
        assert_eq!(eff.look.gaps_outer, Some(c.bar.side_gap.round() as u32));
    }
}

#[cfg(test)]
mod sidebar_mode_tests {
    use super::*;

    fn parse(sidebar: &str) -> SidebarConfig {
        toml::from_str::<ShellConfig>(&format!("[sidebar]\n{sidebar}")).unwrap().sidebar
    }

    #[test]
    fn old_pinned_flag_still_loads() {
        assert_eq!(parse("pinned = true").mode, SidebarMode::Pinned);
        assert_eq!(parse("pinned = false").mode, SidebarMode::Float);
        assert_eq!(parse("width = 400.0").mode, SidebarMode::Float);
        // An explicit mode wins over a leftover flag.
        assert_eq!(parse("pinned = true\nmode = \"push\"").mode, SidebarMode::Push);
    }

    #[test]
    fn modes_round_trip_and_only_float_never_reserves() {
        for mode in [SidebarMode::Float, SidebarMode::Push, SidebarMode::Pinned] {
            let mut cfg = ShellConfig::default();
            cfg.sidebar.mode = mode;
            let back: ShellConfig = toml::from_str(&toml::to_string(&cfg).unwrap()).unwrap();
            assert_eq!(back.sidebar.mode, mode);
            // Studio edits go through JSON.
            let json: ShellConfig = serde_json::from_value(serde_json::to_value(&cfg).unwrap()).unwrap();
            assert_eq!(json.sidebar.mode, mode);
            assert_eq!(mode.reserves_space(), mode != SidebarMode::Float);
        }
    }

    #[test]
    fn pin_button_returns_to_the_previous_mode() {
        let mut s = SidebarConfig { mode: SidebarMode::Push, ..SidebarConfig::default() };
        s.set_pinned(true);
        assert!(s.pinned());
        s.set_pinned(false);
        assert_eq!(s.mode, SidebarMode::Push);
        let mut f = SidebarConfig::default();
        f.set_pinned(true);
        f.set_pinned(false);
        assert_eq!(f.mode, SidebarMode::Float);
        // Unpinning a config that was pinned from Studio (no remembered mode).
        let mut p = SidebarConfig { mode: SidebarMode::Pinned, unpinned_mode: SidebarMode::Pinned, ..SidebarConfig::default() };
        p.set_pinned(false);
        assert_eq!(p.mode, SidebarMode::Float);
    }
}

#[cfg(test)]
mod plugin_layout_tests {
    use super::*;

    #[test]
    fn removed_fields_are_backwards_compatible() {
        // A config written before `removed` existed still loads.
        let old: PluginsConfig = toml::from_str("enabled = [\"dock\"]\ndisabled = [\"weather\"]\n").unwrap();
        assert!(old.removed.is_empty() && old.removed_places.is_empty());
        // And an empty list stays out of the file.
        let written = toml::to_string(&old).unwrap();
        assert!(!written.contains("removed"), "{written}");
        // Round trip with a removal.
        let mut c = old.clone();
        c.removed.push("weather".into());
        c.removed_places.insert("weather".into(), vec![LayoutSlot::new(LayoutLane::BarRight, 0)]);
        let back: PluginsConfig = toml::from_str(&toml::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
        assert!(back.is_removed("weather"));
    }

    #[test]
    fn places_are_taken_and_put_back() {
        let mut c = ShellConfig::default();
        let before = c.clone();
        let places = c.take_plugin_places("weather");
        assert_eq!(places, ShellConfig::default_plugin_places("weather"));
        assert!(places.iter().any(|s| s.lane == LayoutLane::BarRight) && places.iter().any(|s| s.lane == LayoutLane::Sidebar));
        assert!(c.plugin_places("weather").is_empty());
        c.put_plugin_places("weather", &places);
        assert_eq!(c.bar.right, before.bar.right);
        assert_eq!(c.sidebar.tabs, before.sidebar.tabs);
        // Putting back twice never duplicates; indices past the end append.
        c.put_plugin_places("weather", &places);
        assert_eq!(c.bar.right, before.bar.right);
        c.put_plugin_places("x", &[LayoutSlot::new(LayoutLane::BarLeft, usize::MAX)]);
        assert_eq!(c.bar.left.last().map(String::as_str), Some("x"));
    }

    #[test]
    fn prayer_times_is_not_in_the_default_layout() {
        assert!(ShellConfig::default_plugin_places("prayer-times").is_empty());
        let d = PluginsConfig::default();
        assert!(!d.enabled.iter().any(|p| p == "prayer-times"));
    }
}
