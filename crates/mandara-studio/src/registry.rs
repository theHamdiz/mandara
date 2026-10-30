//! The declarative settings registry: every user-facing option as data
//! (section, group, config path, control, ranges, search text). One generic
//! engine reads and writes `ShellConfig` through JSON pointers, so adding a
//! setting is one entry — no bespoke UI code.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use mandara_config::ShellConfig;
use mandara_config::surfaces::ranges;
use mandara_i18n::{t, tf, tk, tr};
use serde_json::Value;

pub mod alert_settings;
mod desktop_settings;
#[cfg(test)]
mod look_tests;
pub mod surface_settings;
pub mod system_settings;
mod tuning_settings;
pub mod workflow_settings;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    /// The overview Studio opens on: live shell status, quick settings and
    /// a card per navigation group.
    Home,
    /// Looks, designed and wallpaper themes, light / dark, palette tuning,
    /// style, icons and app themes: every color-and-shape choice on one page.
    Themes,
    Bar,
    Sidebar,
    Launcher,
    /// The app dock (W4).
    Dock,
    /// Alt-Tab window switcher and screen time (W4).
    WindowSwitcher,
    /// Popups, rules, do not disturb, history (W3).
    Notifications,
    /// On-screen display kinds and look (W3).
    Osd,
    Wallpaper,
    /// Clocks, system monitor, decorations, visualizers and cards on the
    /// desktop: add, arrange and style them.
    DesktopWidgets,
    Displays,
    Audio,
    /// Wi-Fi, VPN, hotspot, saved networks, Bluetooth devices (W6).
    Network,
    LoginScreen,
    /// Lock screen look (W5).
    LockScreen,
    /// Idle stages, lock behaviour, battery alerts, the session menu and
    /// event hooks (W1).
    Power,
    /// Clock formats, calendars, location and weather (W5).
    DateTime,
    Users,
    /// Keyboard, mouse, touchpad and cursor (W6).
    Input,
    Shortcuts,
    Motion,
    /// Default apps, autostart, preferred apps (W6).
    Applications,
    /// Microphone / camera / screen-share indicators (W3).
    Privacy,
    General,
    Typography,
    Plugins,
    Compositor,
    Performance,
    /// Settings profiles, export / import, recent changes (W6).
    Profiles,
    Maintenance,
    About,
}

impl Section {
    pub const ALL: [Section; 32] = [
        Section::Home,
        Section::Themes,
        Section::Bar,
        Section::Sidebar,
        Section::Launcher,
        Section::Dock,
        Section::WindowSwitcher,
        Section::Notifications,
        Section::Osd,
        Section::Wallpaper,
        Section::DesktopWidgets,
        Section::Displays,
        Section::Audio,
        Section::Network,
        Section::LoginScreen,
        Section::LockScreen,
        Section::Power,
        Section::DateTime,
        Section::Users,
        Section::Input,
        Section::Shortcuts,
        Section::Motion,
        Section::Applications,
        Section::Privacy,
        Section::General,
        Section::Typography,
        Section::Plugins,
        Section::Compositor,
        Section::Performance,
        Section::Profiles,
        Section::Maintenance,
        Section::About,
    ];

    /// English title: matches command-line section names and keys element
    /// ids. Display [`Section::label`] instead.
    pub fn title(self) -> &'static str {
        match self {
            Section::Home => tk("Home"),
            Section::Themes => tk("Themes"),
            Section::Bar => tk("Bar"),
            Section::Sidebar => tk("Sidebars & hover"),
            Section::Launcher => tk("Launcher"),
            Section::Dock => tk("Dock"),
            Section::WindowSwitcher => tk("Window switcher"),
            Section::Notifications => tk("Notifications"),
            Section::Osd => tk("On-screen displays"),
            Section::Privacy => tk("Privacy"),
            Section::Wallpaper => tk("Wallpaper"),
            Section::DesktopWidgets => tk("Desktop widgets"),
            Section::Displays => tk("Displays"),
            Section::Audio => tk("Audio & media"),
            Section::Network => tk("Network & Bluetooth"),
            Section::Input => tk("Input"),
            Section::Applications => tk("Applications"),
            Section::Profiles => tk("Profiles & backup"),
            Section::Power => tk("Power & idle"),
            Section::LoginScreen => tk("Login screen"),
            Section::LockScreen => tk("Lock appearance"),
            Section::DateTime => tk("Date, time & location"),
            Section::Users => tk("Users"),
            Section::Shortcuts => tk("Keyboard shortcuts"),
            Section::Motion => tk("Motion"),
            Section::General => tk("General"),
            Section::Typography => tk("Typography"),
            Section::Plugins => tk("Plugins"),
            Section::Compositor => tk("Compositor"),
            Section::Performance => tk("Performance"),
            Section::Maintenance => tk("Maintenance"),
            Section::About => tk("About"),
        }
    }

    /// The title in the current language.
    pub fn label(self) -> String {
        tr(self.title())
    }

    /// Sidebar icon; the launcher's Studio entries use the same one
    /// (`mandara_ui::overlays::STUDIO_SECTIONS`, checked by a test).
    pub fn icon(self) -> &'static str {
        match self {
            Section::Home => "grid",
            Section::Themes => "droplet",
            Section::Wallpaper => "image",
            Section::DesktopWidgets => "clock",
            Section::Typography => "text-scan",
            Section::Motion => "waves",
            Section::Bar => "window",
            Section::Sidebar => "sidebar",
            Section::Dock => "box",
            Section::Launcher => "search",
            Section::WindowSwitcher => "copy",
            Section::Notifications => "bell",
            Section::Osd => "sliders",
            Section::Displays => "monitor",
            Section::Audio => "volume-high",
            Section::Network => "wifi-4",
            Section::Power => "power",
            Section::Input => "pointer",
            Section::Shortcuts => "keyboard",
            Section::DateTime => "calendar",
            Section::Applications => "package",
            Section::Users => "user",
            Section::LoginScreen => "key",
            Section::LockScreen => "lock",
            Section::Privacy => "shield",
            Section::Plugins => "zap",
            Section::Compositor => "tiles",
            Section::General => "globe",
            Section::Profiles => "download",
            Section::Performance => "gauge",
            Section::Maintenance => "refresh",
            Section::About => "note",
        }
    }

    /// The sidebar group this section sits in (`None` for Home, pinned
    /// above the groups).
    pub fn group(self) -> Option<NavGroup> {
        NavGroup::ALL.into_iter().find(|g| g.sections().contains(&self))
    }
}

/// The sidebar's information architecture: sections grouped by what they
/// shape, in reading order. Home sits above the groups.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum NavGroup {
    /// How everything looks and moves.
    LookAndFeel,
    /// The shell's own surfaces.
    Desktop,
    /// Hardware, connectivity and the session.
    System,
    /// Who can get in, and what apps can see.
    Security,
    /// Plugins and the compositor.
    Extend,
    /// Studio and the shell themselves.
    Mandara,
}

impl NavGroup {
    pub const ALL: [NavGroup; 6] = [NavGroup::LookAndFeel, NavGroup::Desktop, NavGroup::System, NavGroup::Security, NavGroup::Extend, NavGroup::Mandara];

    /// Stable id (persisted collapse state, element ids).
    pub fn id(self) -> &'static str {
        match self {
            NavGroup::LookAndFeel => "look-and-feel",
            NavGroup::Desktop => "desktop",
            NavGroup::System => "system",
            NavGroup::Security => "security",
            NavGroup::Extend => "extend",
            NavGroup::Mandara => "mandara",
        }
    }

    /// English title; display it with `tr`.
    pub fn title(self) -> &'static str {
        match self {
            NavGroup::LookAndFeel => tk("Look & feel"),
            NavGroup::Desktop => tk("Desktop"),
            NavGroup::System => tk("System"),
            NavGroup::Security => tk("Security & accounts"),
            NavGroup::Extend => tk("Extend"),
            NavGroup::Mandara => tk("Mandara"),
        }
    }

    /// One line for the group's card on Home (`tk` text).
    pub fn blurb(self) -> &'static str {
        match self {
            NavGroup::LookAndFeel => tk("Colors, wallpaper, type and motion"),
            NavGroup::Desktop => tk("Bar, dock, launcher and popups"),
            NavGroup::System => tk("Screens, sound, network, power and input"),
            NavGroup::Security => tk("People, sign-in, lock and privacy"),
            NavGroup::Extend => tk("Plugins and your compositor"),
            NavGroup::Mandara => tk("Language, backups, health and version"),
        }
    }

    /// Icon of the group's card on Home.
    pub fn icon(self) -> &'static str {
        match self {
            NavGroup::LookAndFeel => "droplet",
            NavGroup::Desktop => "window",
            NavGroup::System => "cpu",
            NavGroup::Security => "shield",
            NavGroup::Extend => "zap",
            NavGroup::Mandara => "heart",
        }
    }

    pub fn sections(self) -> &'static [Section] {
        use Section::*;
        match self {
            NavGroup::LookAndFeel => &[Themes, Wallpaper, DesktopWidgets, Typography, Motion],
            NavGroup::Desktop => &[Bar, Sidebar, Dock, Launcher, WindowSwitcher, Notifications, Osd],
            NavGroup::System => &[Displays, Audio, Network, Power, Input, Shortcuts, DateTime, Applications],
            NavGroup::Security => &[Users, LoginScreen, LockScreen, Privacy],
            NavGroup::Extend => &[Plugins, Compositor],
            NavGroup::Mandara => &[General, Profiles, Performance, Maintenance, About],
        }
    }

    /// Parses [`NavGroup::id`].
    pub fn parse(id: &str) -> Option<NavGroup> {
        NavGroup::ALL.into_iter().find(|g| g.id() == id)
    }
}

/// Sections in sidebar order: Home, then each group's.
pub fn nav_order() -> impl Iterator<Item = Section> {
    std::iter::once(Section::Home).chain(NavGroup::ALL.into_iter().flat_map(|g| g.sections().iter().copied()))
}

#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Toggle,
    Slider { min: f64, max: f64, step: f64, unit: &'static str },
    /// `(json value, label)`; values are strings unless they parse as JSON.
    Choice(&'static [(&'static str, &'static str)]),
    Text { placeholder: &'static str },
    /// Text that maps empty → `null`.
    OptionalText { placeholder: &'static str },
    Number { min: f64, max: f64 },
    /// An icon: bundled name, `distro`, icon-theme name or image path,
    /// chosen from a visual picker.
    Icon,
    /// A daily time `HH:MM`, edited with hour / minute steppers.
    Time,
    /// A number that may be unset (`null` = "keep my compositor's value").
    OptionalSlider { min: f64, max: f64, step: f64, unit: &'static str, default: f64 },
    /// A button running a shell command over IPC (`button` is `tk()` text).
    Command { button: &'static str, args: &'static [&'static str] },
    /// A list of strings, edited as comma-separated text.
    List { placeholder: &'static str },
}

#[derive(Clone, Debug)]
pub struct Setting {
    pub section: Section,
    pub group: &'static str,
    /// JSON pointer into the serialized config (`/bar/height`).
    pub path: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub control: Control,
    /// Value used when the key is absent (plugin settings tables).
    pub default: Option<Value>,
}

impl Setting {
    /// Matches the English text and its translation.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        let english = [self.label, self.help, self.group, self.section.title()];
        english.iter().chain(std::iter::once(&self.path)).any(|s| s.to_lowercase().contains(&q))
            || (mandara_i18n::language() != mandara_i18n::Lang::En && english.iter().any(|s| !s.is_empty() && tr(s).to_lowercase().contains(&q)))
    }
}

fn s(section: Section, group: &'static str, path: &'static str, label: &'static str, help: &'static str, control: Control) -> Setting {
    Setting { section, group, path, label, help, control, default: None }
}

fn sd(section: Section, group: &'static str, path: &'static str, label: &'static str, help: &'static str, control: Control, default: Value) -> Setting {
    Setting { section, group, path, label, help, control, default: Some(default) }
}

/// Choice labels are `tk()`-marked English; display them with `tr()`.
const LAYOUTS: &[(&str, &str)] = &[("minimal-edge", tk("Edge frame")), ("floating-pills", tk("Glass pills")), ("contiguous-island", tk("Island")), ("hairline", tk("Hairline")), ("micro", tk("Micro pill"))];
/// Tri-state for optional booleans: unset keeps the compositor's value.
const TRI: &[(&str, &str)] = &[("null", tk("Your config")), ("true", tk("On")), ("false", tk("Off"))];

/// A slider over a named config [`Span`](mandara_config::surfaces::Span).
pub(crate) fn span(r: mandara_config::surfaces::Span, unit: &'static str) -> Control {
    Control::Slider { min: f64::from(r.min), max: f64::from(r.max), step: f64::from(r.step), unit }
}

fn optional(min: f64, max: f64, step: f64, unit: &'static str, default: f64) -> Control {
    Control::OptionalSlider { min, max, step, unit, default }
}

const ICON_STYLES: &[(&str, &str)] = &[("monochrome", tk("Monochrome")), ("color", tk("Color"))];
/// Language names stay in their own script (`tr` leaves them as they are).
const LOCALES: &[(&str, &str)] = &[
    ("auto", tk("System")),
    ("english", "English"),
    ("arabic", "العربية"),
    ("spanish", "Español"),
    ("french", "Français"),
    ("german", "Deutsch"),
    ("turkish", "Türkçe"),
    ("russian", "Русский"),
    ("hindi", "हिन्दी"),
    ("chinese", "简体中文"),
    ("japanese", "日本語"),
];
const UNITS: &[(&str, &str)] = &[("metric", tk("Metric (°C, km/h)")), ("imperial", tk("Imperial (°F, mph)"))];
const WEEK: &[(&str, &str)] = &[("monday", tk("Monday")), ("sunday", tk("Sunday")), ("saturday", tk("Saturday"))];
const MODES: &[(&str, &str)] = &[("dark", tk("Dark")), ("light", tk("Light"))];
const METHODS: &[(&str, &str)] = &[
    ("egyptian", tk("Egyptian")),
    ("muslim-world-league", tk("MWL")),
    ("umm-al-qura", tk("Umm al-Qura")),
    ("isna", tk("ISNA")),
    ("karachi", tk("Karachi")),
];
const SLEEP_MODES: &[(&str, &str)] = &[("suspend", tk("Suspend")), ("hibernate", tk("Hibernate")), ("suspend-then-hibernate", tk("Suspend, then hibernate")), ("hybrid-sleep", tk("Hybrid"))];
const ALERT_STYLES: &[(&str, &str)] = &[("both", tk("Notification + OSD")), ("notification", tk("Notification")), ("osd", tk("OSD")), ("off", tk("Off"))];
const CRITICAL_ACTIONS: &[(&str, &str)] = &[("nothing", tk("Nothing")), ("suspend", tk("Sleep")), ("hibernate", tk("Hibernate")), ("shutdown", tk("Shut down"))];
const PROFILES: &[(&str, &str)] = &[("null", tk("Leave it")), ("power-saver", tk("Power saver")), ("balanced", tk("Balanced")), ("performance", tk("Performance"))];
const MENU_STYLES: &[(&str, &str)] = &[("dock", tk("Edge dock")), ("grid", tk("Grid"))];
const MENU_DEFAULTS: &[(&str, &str)] = &[("lock", tk("Lock")), ("suspend", tk("Sleep")), ("logout", tk("Log out")), ("reboot", tk("Restart")), ("shutdown", tk("Shut down"))];
const SCHOOLS: &[(&str, &str)] = &[("shafii", tk("Shafi'i")), ("hanafi", tk("Hanafi"))];
const BOOL_SCHEMES: &[(&str, &str)] = &[("tonal-spot", tk("Tonal spot")), ("vibrant", tk("Vibrant")), ("expressive", tk("Expressive")), ("fidelity", tk("Fidelity")), ("fruit-salad", tk("Fruit salad")), ("monochrome", tk("Monochrome"))];

/// Every registered setting, built once (the table is static data).
pub fn settings() -> &'static [Setting] {
    static ALL: OnceLock<Vec<Setting>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut all = build();
        all.extend(surface_settings::entries());
        all.extend(desktop_settings::entries());
        all.extend(alert_settings::entries());
        all.extend(system_settings::entries());
        all.extend(workflow_settings::entries());
        all.extend(tuning_settings::entries());
        all
    })
}

/// The settings of one section, in registry order.
pub fn in_section(section: Section) -> impl Iterator<Item = &'static Setting> {
    settings().iter().filter(move |s| s.section == section)
}

/// The opt-in plugin a setting belongs to: its rows show only while that
/// plugin is on (Wallhaven's search options, the cursor settings).
pub fn required_plugin(path: &str) -> Option<&'static str> {
    if path.starts_with("/wallpaper/wallhaven") {
        Some(mandara_config::upgrade::WALLHAVEN_ID)
    } else if path.starts_with("/compositor/input/cursor/") {
        Some(mandara_config::upgrade::CURSORS_ID)
    } else {
        None
    }
}

/// Why `setting` can't be changed under `cfg` right now (`tk` text shown in
/// place of its control), e.g. the launcher's hover edge while the dock
/// owns the bottom edge (`ShellConfig::launcher_hover_enabled`).
pub fn locked(setting: &Setting, cfg: &ShellConfig) -> Option<&'static str> {
    match setting.path {
        "/edges/launcher_bottom" if cfg.dock_enabled() => Some(tk("Click only while the dock is on")),
        path if path.starts_with("/compositor/input/cursor/") => system_settings::cursor_locked(path, running_compositor()),
        path => surface_settings::locked(path, cfg),
    }
}

/// The compositor Studio runs under, detected once (it can't change while
/// Studio is open).
pub fn running_compositor() -> Option<mandara_config::compositor::Target> {
    use mandara_compositor::CompositorKind;
    use mandara_config::compositor::Target;
    static RUNNING: OnceLock<Option<Target>> = OnceLock::new();
    *RUNNING.get_or_init(|| {
        mandara_compositor::detect().map(|k| match k {
            CompositorKind::MangoWM => Target::Mango,
            CompositorKind::Niri => Target::Niri,
            CompositorKind::Hyprland => Target::Hyprland,
        })
    })
}

thread_local! {
    /// The default config's JSON tree (reset buttons, "changed" checks).
    static DEFAULTS: Rc<Value> = Rc::new(serde_json::to_value(ShellConfig::default()).unwrap_or(Value::Null));
}

/// `setting`'s value in the default config.
pub fn default_value(setting: &Setting) -> Value {
    DEFAULTS.with(|tree| tree.pointer(setting.path).cloned().filter(|v| !v.is_null() || setting.default.is_none()).or_else(|| setting.default.clone()).unwrap_or(Value::Null))
}

/// The setting at a config path.
pub fn by_path(path: &str) -> Option<&'static Setting> {
    settings().iter().find(|s| s.path == path)
}

thread_local! {
    /// The JSON tree of the config being rendered, so reading every control
    /// of a page serializes the config once instead of once per control.
    static JSON: RefCell<Option<(Arc<ShellConfig>, Rc<Value>)>> = const { RefCell::new(None) };
}

/// Caches `cfg`'s JSON tree for [`get`] / [`set`]; call once per frame with
/// the snapshot being rendered. Holding the `Arc` keeps the cache key alive,
/// so a new config can never reuse the address of a cached one.
pub fn prime(cfg: &Arc<ShellConfig>) {
    JSON.with(|j| {
        let mut j = j.borrow_mut();
        if !j.as_ref().is_some_and(|(cached, _)| Arc::ptr_eq(cached, cfg)) {
            let tree = serde_json::to_value(&**cfg).unwrap_or(Value::Null);
            *j = Some((cfg.clone(), Rc::new(tree)));
        }
    });
}

/// `cfg` as JSON: the primed tree when `cfg` is the primed snapshot.
fn json_of(cfg: &ShellConfig) -> Rc<Value> {
    let hit = JSON.with(|j| j.borrow().as_ref().filter(|(cached, _)| std::ptr::eq(&**cached, cfg)).map(|(_, tree)| tree.clone()));
    hit.unwrap_or_else(|| Rc::new(serde_json::to_value(cfg).unwrap_or(Value::Null)))
}

fn build() -> Vec<Setting> {
    use Control::*;
    use Section::*;
    let slider = |min: f64, max: f64, step: f64, unit: &'static str| Slider { min, max, step, unit };
    vec![
        // Themes
        s(Themes, tk("Mode"), "/theme/mode", tk("Appearance"), tk("Light or dark surfaces for every theme"), Choice(MODES)),
        s(Themes, tk("Mode"), "/theme/auto", tk("Switch automatically"), tk("Follow the desktop's light/dark preference, or switch at set times every day"), Choice(const { &[("manual", tk("Off")), ("system", tk("Follow system")), ("schedule", tk("Scheduled"))] })),
        s(Themes, tk("Mode"), "/theme/light_at", tk("Light from"), tk("Scheduled: switch to light at this time"), Time),
        s(Themes, tk("Mode"), "/theme/dark_at", tk("Dark from"), tk("Scheduled: switch to dark at this time"), Time),
        s(Themes, tk("Tuning"), "/theme/adjustments/chroma_scale", tk("Accent vividness"), tk("Scales the chroma of every accent"), slider(0.2, 1.8, 0.05, "×")),
        s(Themes, tk("Tuning"), "/theme/adjustments/hue_shift", tk("Hue shift"), tk("Rotates every hue of the palette"), slider(-180.0, 180.0, 1.0, "°")),
        s(Themes, tk("Tuning"), "/theme/adjustments/lightness_shift", tk("Lightness"), tk("Shifts every token's lightness"), slider(-0.15, 0.15, 0.005, "")),
        s(Themes, tk("Tuning"), "/theme/adjustments/rim_intensity", tk("Specular rim"), tk("Strength of glass edge highlights"), slider(0.0, 2.0, 0.05, "×")),
        s(Themes, tk("Tuning"), "/theme/transition_ms", tk("Theme crossfade"), tk("Duration of the OKLCH palette transition"), slider(0.0, 1500.0, 25.0, " ms")),
        s(Themes, tk("Icons"), "/appearance/bar_icons", tk("Bar app icons"), tk("Monochrome symbolic icons tint with the theme"), Choice(ICON_STYLES)),
        s(Themes, tk("Icons"), "/appearance/icon_theme", tk("Icon theme"), tk("Leave empty to follow GTK's icon theme"), OptionalText { placeholder: "Tela-circle-red-dark" }),
        // No default: with a designed theme no scheme chip is lit, and
        // picking one switches to the wallpaper's colors.
        s(Themes, tk("Wallpaper colors"), "/theme/source/variant", tk("Wallpaper scheme"), tk("Material You variant used for wallpaper themes"), Choice(BOOL_SCHEMES)),
        // Bar
        s(Bar, tk("Layout"), "/bar/layout", tk("Style"), tk("Edge frame, floating glass pills, one island, a thin hairline or a tiny micro pill"), Choice(LAYOUTS)),
        s(Bar, tk("Layout"), "/bar/position", tk("Position"), tk("Any screen edge; panels open toward the middle (sideways from a side bar)"), Choice(const { &[("top", tk("Top")), ("bottom", tk("Bottom")), ("left", tk("Left")), ("right", tk("Right"))] })),
        s(Bar, tk("Layout"), "/bar/height", tk("Height"), tk("Bar thickness"), span(ranges::BAR_HEIGHT, " px")),
        s(Bar, tk("Layout"), "/bar/edge_gap", tk("Edge gap"), tk("Distance from the screen edge (floating styles and the floating frame)"), span(ranges::BAR_EDGE_GAP, " px")),
        s(Bar, tk("Layout"), "/bar/side_gap", tk("Side inset"), tk("Horizontal inset of the outer groups"), span(ranges::BAR_SIDE_GAP, " px")),
        s(Bar, tk("Shape"), "/bar/corner_radius", tk("Bar radius"), tk("Glass pills and island: their corners. Edge frame: the hover chips, and the frame's ends when it floats"), span(ranges::BAR_PILL_RADIUS, " px")),
        s(Bar, tk("Shape"), "/bar/flare_radius", tk("Flare radius"), tk("Concave curve where the edge frame meets the screen sides, panels and the sidebar"), span(ranges::BAR_FLARE_RADIUS, " px")),
        s(Bar, surface_settings::APPEARANCE, "/bar/opacity", tk("Background opacity"), tk("Lower it to let your wallpaper and windows show through the bar"), span(ranges::OPACITY, "")),
        s(Bar, surface_settings::PANELS, "/bar/panel_radius", tk("Panel radius"), tk("Corner radius of dropdown panels"), span(ranges::PANEL_RADIUS, " px")),
        s(Bar, tk("Clock"), "/bar/show_seconds", tk("Show seconds"), tk("Seconds in the bar clock"), Toggle),
        // Sidebar
        s(Sidebar, tk("Behaviour"), "/sidebar/enabled", tk("Enable sidebar"), tk("The edge drawer with quick settings and cards"), Toggle),
        s(Sidebar, tk("Behaviour"), "/sidebar/side", tk("Side"), tk("Which edge it lives on; the power dock takes the other. Auto follows the locale (right for Arabic)"), Choice(const { &[("auto", tk("Auto")), ("left", tk("Left")), ("right", tk("Right"))] })),
        s(Sidebar, tk("Behaviour"), "/sidebar/notifications_tab", tk("Notifications tab"), tk("Notifications in their own tab after yours, instead of a card"), Toggle),
        s(Sidebar, tk("Tabs"), "/sidebar/always_first_tab", tk("Always open on the first tab"), tk("Every opening shows the first tab instead of the one you used last"), Toggle),
        s(Sidebar, tk("Tabs"), "/sidebar/add_tab_button", tk("New-tab button"), tk("A + at the end of the tab strip makes a tab in place"), Toggle),
        s(Sidebar, tk("Open on hover"), "/sidebar/hover_reveal", tk("Sidebar"), tk("Open when the pointer touches the sidebar's edge"), Toggle),
        s(Sidebar, tk("Open on hover"), "/edges/session_right", tk("Power dock"), tk("The edge opposite the sidebar · MangoWM and Hyprland"), Toggle),
        s(Sidebar, tk("Open on hover"), "/edges/launcher_bottom", tk("Launcher"), tk("Rest the pointer on the bottom edge"), Toggle),
        s(Sidebar, tk("Open on hover"), "/edges/dwell_ms", tk("Dwell time"), tk("How long the pointer must rest on an edge first"), slider(0.0, 800.0, 10.0, " ms")),
        s(Sidebar, tk("Behaviour"), "/sidebar/mode", tk("Windows"), tk("Float over windows (smoothest), push them aside while the sidebar is open, or keep it pinned open with its space reserved"), Choice(const { &[("float", tk("Float")), ("push", tk("Push")), ("pinned", tk("Pinned"))] })),
        s(Sidebar, tk("Layout"), "/sidebar/width", tk("Width"), tk("Sidebar width"), span(ranges::SIDEBAR_WIDTH, " px")),
        // Launcher
        s(Launcher, tk("Button"), "/appearance/launcher_icon", tk("Button icon"), tk("Bundled, your distro's logo, any icon from your theme, or your own image"), Icon),
        s(Launcher, tk("Launcher"), "/launcher/enabled", tk("Enable launcher"), tk("Apps, settings, commands, “/” slash commands and skills, “=” calculator; off disables the keybind, edge and menu entry"), Toggle),
        s(Launcher, tk("Launcher"), "/launcher/max_results", tk("Visible results"), tk("Rows shown before the list scrolls"), slider(3.0, 12.0, 1.0, "")),
        s(Launcher, tk("Launcher"), "/appearance/launcher_icons", tk("App icons"), tk("How app icons are drawn"), Choice(ICON_STYLES)),
        // Wallpaper
        s(Wallpaper, tk("Desktop"), "/wallpaper/enabled", tk("Draw the wallpaper"), tk("Mandara paints the desktop background (replaces swww / awww / swaybg)"), Toggle),
        s(Wallpaper, tk("Desktop"), "/wallpaper/crossfade_ms", tk("Crossfade"), tk("Blend time when the wallpaper changes"), slider(0.0, 2000.0, 50.0, " ms")),
        // The depth clock itself (style, place, date pill, audio ring) is a
        // desktop widget: Wallpaper → Desktop widgets.
        s(Wallpaper, tk("Depth clock"), "/wallpaper/depth_effect", tk("Depth effect"), tk("Draw the wallpaper's foreground subject over the clock (with the Fill mode)"), Toggle),
        s(Wallpaper, tk("Depth clock"), "/wallpaper/depth_mask", tk("Subject mask"), tk("Automatic finds the subject itself; your own masks go in ~/.local/share/mandara/depth_masks/<wallpaper name>.png"), Choice(const { &[("auto", tk("Automatic")), ("custom", tk("My masks only")), ("off", tk("Off"))] })),
        s(Wallpaper, tk("Depth clock"), "/wallpaper/path", tk("Regenerate depth mask"), tk("Find the current wallpaper's subject again (skipped when it isn't confident)"), Command { button: tk("Regenerate"), args: &["wallpaper", "depth-mask", "regenerate"] }),
        s(Wallpaper, tk("Picker"), "/wallpaper/picker", tk("Enable wallpaper picker"), tk("The carousel opened from the bar, the main menu or `mandara msg wallpaper picker toggle`"), Toggle),
        s(Wallpaper, tk("Picker"), "/wallpaper/picker_style", tk("Layout"), tk("Slices: slanted strips around a big preview (Omarchy-style). Carousel: floating cards"), Choice(const { &[("slices", tk("Slices")), ("carousel", tk("Carousel"))] })),
        s(Wallpaper, tk("Picker"), "/wallpaper/picker_size", tk("Tile size"), tk("Scales every tile"), slider(0.6, 1.6, 0.05, "×")),
        s(Wallpaper, tk("Picker"), "/wallpaper/picker_focus", tk("Focused tile size"), tk("Carousel: how much bigger the middle wallpaper is"), slider(1.5, 4.0, 0.1, "×")),
        s(Wallpaper, tk("Picker"), "/wallpaper/picker_names", tk("Show names"), tk("The focused wallpaper's name under the picker"), Toggle),
        s(Wallpaper, tk("Picker"), "/wallpaper/directory", tk("Folder"), tk("Images here (and one level of sub-folders) appear in the picker"), Text { placeholder: "~/Pictures/Wallpapers" }),
        s(Wallpaper, tk("Wallhaven"), "/wallpaper/wallhaven_query", tk("Default search"), tk("Leave empty for the toplist, e.g. \"nature 4k\""), Text { placeholder: "toplist" }),
        s(Wallpaper, tk("Wallhaven"), "/wallpaper/wallhaven_sorting", tk("Sort by"), tk("Order of results without a search"), Choice(const { &[("toplist", tk("Top")), ("date_added", tk("Newest")), ("views", tk("Most viewed")), ("favorites", tk("Favorites")), ("random", tk("Random"))] })),
        s(Wallpaper, tk("Picker"), "/wallpaper/match_colors", tk("Match colors to wallpaper"), tk("Re-derive the theme (Material You) from each wallpaper you pick"), Toggle),
        // Displays & night light (the page adds the arrangement, profiles and brightness)
        s(Displays, tk("Schedule & colour"), "/night_light/enabled", tk("Night light"), tk("Warm the screens in the evening (wlr-gamma-control: MangoWM, Hyprland, Sway; Niri can't)"), Toggle),
        s(Displays, tk("Schedule & colour"), "/night_light/schedule", tk("Schedule"), tk("Sunset to sunrise at your location (Date, time & location, else the weather location), fixed times, or always while on"), Choice(const { &[("sun", tk("Sunset to sunrise")), ("fixed", tk("Fixed times")), ("always", tk("Always"))] })),
        s(Displays, tk("Schedule & colour"), "/night_light/night_temperature", tk("Night temperature"), tk("Lower is warmer; 6500 K is untinted"), slider(1000.0, 6500.0, 100.0, " K")),
        s(Displays, tk("Schedule & colour"), "/night_light/day_temperature", tk("Day temperature"), tk("Usually 6500 K (untinted)"), slider(4000.0, 6500.0, 100.0, " K")),
        s(Displays, tk("Schedule & colour"), "/night_light/start", tk("Warm from"), tk("Fixed schedule: start of the night"), Time),
        s(Displays, tk("Schedule & colour"), "/night_light/end", tk("Until"), tk("Fixed schedule: end of the night"), Time),
        s(Displays, tk("Schedule & colour"), "/night_light/transition_minutes", tk("Transition"), tk("How long the fade after each change takes"), slider(0.0, 180.0, 5.0, " min")),
        s(Displays, tk("Schedule & colour"), "/night_light/gamma", tk("Gamma"), tk("Adjusts mid-tones at night (1 = unchanged)"), slider(0.5, 1.5, 0.05, "")),
        s(Displays, tk("Schedule & colour"), "/night_light/brightness", tk("Night dimming"), tk("Darkens the picture at night beyond the backlight's lowest step (1 = unchanged)"), slider(0.4, 1.0, 0.05, "×")),
        s(Displays, tk("Exceptions"), "/night_light/pause_fullscreen", tk("Pause for fullscreen windows"), tk("Games and videos keep their true colours"), Toggle),
        s(Displays, tk("Arrangement"), "/displays/revert_seconds", tk("Revert after"), tk("Changed display settings come back unless you press Keep within this time"), slider(5.0, 60.0, 1.0, " s")),
        s(Displays, tk("Arrangement"), "/displays/auto_profiles", tk("Apply profiles automatically"), tk("When the monitors of a saved profile are connected, switch to it"), Toggle),
        s(Displays, tk("External brightness"), "/displays/brightness/ddc", tk("Control external monitors"), tk("Brightness over DDC/CI through /dev/i2c-* (needs the i2c group and DDC/CI on in the monitor's menu)"), Toggle),
        s(Displays, tk("External brightness"), "/displays/brightness/sync_all", tk("Brightness keys change every screen"), tk("The laptop panel and external monitors move together"), Toggle),
        s(Displays, tk("External brightness"), "/displays/brightness/min_percent", tk("Minimum brightness"), tk("Never dim a screen below this"), slider(1.0, 50.0, 1.0, "%")),
        // Motion
        s(Motion, tk("Springs"), "/motion/stiffness", tk("Stiffness"), tk("Higher is snappier"), slider(60.0, 900.0, 5.0, "")),
        s(Motion, tk("Springs"), "/motion/damping", tk("Damping"), tk("Higher settles with less bounce"), slider(4.0, 60.0, 0.5, "")),
        s(Motion, tk("Springs"), "/motion/mass", tk("Mass"), tk("Heavier springs move more slowly"), slider(0.3, 3.0, 0.05, "")),
        s(Motion, tk("Accessibility"), "/motion/reduce_motion", tk("Reduce motion"), tk("Apply changes instantly without springs"), Toggle),
        // General
        s(General, tk("Region"), "/general/locale", tk("Language"), tk("Shell text language"), Choice(LOCALES)),
        s(General, tk("Region"), "/general/clock_24h", tk("24-hour clock"), tk("Off shows AM / PM"), Toggle),
        s(General, tk("Region"), "/general/week_start", tk("Week starts on"), tk("First day in calendars"), Choice(WEEK)),
        s(General, tk("Region"), "/general/units", tk("Units"), tk("Temperatures and wind speed in weather and system panels"), Choice(UNITS)),
        s(General, tk("Reliability"), "/general/auto_restart", tk("Restart automatically after a crash"), tk("Keeps Mandara running if anything ever takes it down. Turn off to let it stay stopped after a crash or kill"), Toggle),
        s(General, tk("Security"), "/general/polkit_agent", tk("Authentication dialogs"), tk("Mandara asks for your password when an app needs administrator rights (polkit). Skipped if another agent is already running"), Toggle),
        s(General, tk("Security"), "/general/auth_dialog_width", tk("Password dialog width"), tk("Width of the administrator-password and keyring-unlock dialogs"), slider(360.0, 640.0, 8.0, " px")),
        s(General, tk("Security"), "/general/lock_screen", tk("Mandara lock screen"), tk("Lock requests from the session menu, idle daemons and loginctl lock-session open Mandara's lock screen"), Toggle),
        s(General, tk("Keyrings & passwords"), "/general/keyring_prompt", tk("GNOME Keyring prompts"), tk("Mandara asks for your keyring password when an app needs a saved secret (gnome-keyring / Secret Service). Skipped while another prompter, such as GNOME Shell, is running"), Toggle),
        s(General, tk("Keyrings & passwords"), "/general/keepassxc_prompt", tk("KeePassXC unlock dialog"), tk("Unlock KeePassXC databases from quick settings, the launcher or `mandara msg keepassxc unlock`. The password is sent to KeePassXC over the session bus in clear text, which only programs running as you can read (gnome-keyring's prompts are encrypted)"), Toggle),
        s(General, tk("Keyrings & passwords"), "/general/keepassxc_ask_at_login", tk("Ask for KeePassXC at login"), tk("Once per login, when KeePassXC is running with a locked database, Mandara's dialog asks for its password"), Toggle),
        // Updates (bespoke card on the About page: updates_card.rs).
        s(About, tk("Updates"), "/updates/self_check", tk("Check for updates automatically"), tk("Looks for new Mandara releases in the background and tells you once per version. Updates install as small binary patches, with the full download as the fallback"), Toggle),
        s(About, tk("Updates"), "/updates/plugins", tk("Include marketplace plugins"), tk("Also checks the plugins you installed from the marketplace (Mandara's own plugins update with the shell)"), Toggle),
        s(About, tk("Updates"), "/updates/check_interval_hours", tk("Check every"), tk("Hours between automatic checks"), slider(1.0, 72.0, 1.0, " h")),
        s(Privacy, tk("Network"), "/general/offline", tk("Offline mode"), tk("Mandara makes no network requests of its own: weather, lyrics, album art, Wallhaven, the plugin marketplace, public IP, update news and downloads stay off"), Toggle),
        // Power & idle (bespoke page: power_page.rs shows the timeouts and lists).
        s(Power, tk("When idle"), "/idle/fade_s", tk("Fade before"), tk("The screen fades out this long before it locks or turns off; any input cancels"), slider(0.0, 30.0, 1.0, " s")),
        s(Power, tk("When idle"), "/idle/suspend_mode", tk("Sleep means"), tk("What the idle stage, the critical battery action and the session menu's Sleep do"), Choice(SLEEP_MODES)),
        s(Power, tk("When idle"), "/idle/locked_screen_off_s", tk("Screens off while locked"), tk("Turn the screens off after this much idle time on the lock screen, even while a video keeps the session awake (0 = never)"), slider(0.0, 600.0, 10.0, " s")),
        s(Power, tk("When idle"), "/idle/respect_media", tk("Stay awake while media plays"), tk("Music or video playing over MPRIS holds every idle stage back"), Toggle),
        s(Power, tk("Locking"), "/idle/lock_before_sleep", tk("Lock before sleeping"), tk("Closing the lid, the session menu or systemctl suspend lock the screen first; the computer waits until the lock is up"), Toggle),
        s(Power, tk("Locking"), "/idle/lock_on_lid_close", tk("Lock when the lid closes"), tk("Even when closing the lid doesn't suspend (docked, external monitor)"), Toggle),
        s(Power, tk("Locking"), "/idle/lock_at_startup", tk("Lock at login"), tk("Lock once when the session starts (useful with autologin); shell restarts don't lock"), Toggle),
        s(Power, tk("Locking"), "/idle/lock_grace_s", tk("Unlock grace period"), tk("After an automatic lock, moving the mouse or pressing a key within this time unlocks without a password (0 = always ask)"), slider(0.0, 60.0, 1.0, " s")),
        s(Power, tk("Battery alerts"), "/power/low_percent", tk("Low battery at"), tk("Warn once when the charge drops to this while discharging"), slider(5.0, 50.0, 1.0, "%")),
        s(Power, tk("Battery alerts"), "/power/low_alert", tk("Low battery alert"), tk("How the warning appears"), Choice(ALERT_STYLES)),
        s(Power, tk("Battery alerts"), "/power/critical_percent", tk("Critical battery at"), tk("A second, stronger warning"), slider(2.0, 25.0, 1.0, "%")),
        s(Power, tk("Battery alerts"), "/power/critical_alert", tk("Critical battery alert"), tk("How the critical warning appears"), Choice(ALERT_STYLES)),
        s(Power, tk("Battery alerts"), "/power/critical_action", tk("When nearly empty"), tk("Runs after a warning when the charge reaches the action level; plugging in cancels"), Choice(CRITICAL_ACTIONS)),
        s(Power, tk("Battery alerts"), "/power/action_percent", tk("Action level"), tk("Charge at which the action above runs"), slider(1.0, 15.0, 1.0, "%")),
        s(Power, tk("Battery alerts"), "/power/action_delay_s", tk("Warning before the action"), tk("Time to plug in after the warning appears"), slider(10.0, 300.0, 10.0, " s")),
        s(Power, tk("Battery alerts"), "/power/peripheral_alerts", tk("Peripheral batteries"), tk("Warn when a mouse, keyboard or headset runs low"), Toggle),
        s(Power, tk("Battery alerts"), "/power/peripheral_low_percent", tk("Peripheral low at"), tk("Threshold for peripherals (per-device overrides in config.toml under [power.device_thresholds])"), slider(5.0, 50.0, 1.0, "%")),
        s(Power, tk("Battery alerts"), "/power/notify_full", tk("Tell me when charged"), tk("A notice when the battery is full or reaches its charge limit"), Toggle),
        s(Power, tk("Battery alerts"), "/power/critical_pulse", tk("Pulse the battery when critical"), tk("The bar's battery chip breathes coral while the charge is critical"), Toggle),
        s(Power, tk("Power profiles"), "/power/profile_ac", tk("On mains power"), tk("Switch to this profile when the charger is plugged in"), Choice(PROFILES)),
        s(Power, tk("Power profiles"), "/power/profile_battery", tk("On battery"), tk("Switch to this profile when running on battery"), Choice(PROFILES)),
        s(Power, tk("Power profiles"), "/power/saver_at_low", tk("Power saver at low battery"), tk("Switch to power-saver at the low threshold and back when charging"), Toggle),
        s(Power, tk("Session menu"), "/session_menu/style", tk("Style"), tk("A dock on the screen edge or a centered grid"), Choice(MENU_STYLES)),
        s(Power, tk("Session menu"), "/session_menu/grid_columns", tk("Grid columns"), tk("Tiles per row in the grid style"), slider(2.0, 6.0, 1.0, "")),
        s(Power, tk("Session menu"), "/session_menu/hold_to_confirm", tk("Hold to confirm"), tk("Log out, restart and shut down need a press-and-hold (mouse, Enter or a number key)"), Toggle),
        s(Power, tk("Session menu"), "/session_menu/hold_ms", tk("Hold time"), tk("How long to hold"), slider(300.0, 3000.0, 100.0, " ms")),
        s(Power, tk("Session menu"), "/session_menu/default_action", tk("Selected on open"), tk("The entry Enter runs right after opening the menu"), Choice(MENU_DEFAULTS)),
        s(Power, tk("Session menu"), "/session_menu/show_shortcuts", tk("Number-key hints"), tk("Badges 1–9; pressing a number runs that entry"), Toggle),
        s(Power, tk("Hooks"), "/hooks/enabled", tk("Run event hooks"), tk("Commands attached to shell events (below) run; off pauses them all"), Toggle),
        s(Power, tk("Hooks"), "/hooks/timeout_s", tk("Hook time limit"), tk("A hook still running after this is stopped (rules can set their own)"), slider(1.0, 300.0, 1.0, " s")),
        // Typography (font family placeholders are names, not text)
        s(Typography, tk("Families"), "/typography/ui_font", tk("Interface font"), tk("Used for most text"), Text { placeholder: "Inter" }),
        s(Typography, tk("Families"), "/typography/display_font", tk("Display font"), tk("Large clocks and headings"), Text { placeholder: "Alexandria" }),
        s(Typography, tk("Families"), "/typography/arabic_font", tk("Arabic font"), tk("Arabic script"), Text { placeholder: "Cairo" }),
        s(Typography, tk("Families"), "/typography/mono_font", tk("Monospace font"), tk("Code and numbers"), Text { placeholder: "JetBrains Mono" }),
        s(Typography, tk("Size"), "/typography/scale", tk("Text scale"), tk("Scales text in the bar, the launcher and the wallpaper picker"), slider(0.75, 1.5, 0.05, "×")),
        // Plugins (per-plugin settings tables)
        // Groups are the plugins' English names (matched against manifests).
        sd(Plugins, tk("Prayer times"), "/plugins/settings/prayer-times/location_name", tk("Place name"), tk("Shown with the prayer times; empty uses the coordinates"), Text { placeholder: "El-Mandara" }, Value::from("")),
        sd(Plugins, tk("Prayer times"), "/plugins/settings/prayer-times/latitude", tk("Latitude"), tk("Default: El-Mandara, Alexandria"), Number { min: -90.0, max: 90.0 }, Value::from(31.284)),
        sd(Plugins, tk("Prayer times"), "/plugins/settings/prayer-times/longitude", tk("Longitude"), tk("Degrees east"), Number { min: -180.0, max: 180.0 }, Value::from(30.017)),
        sd(Plugins, tk("Prayer times"), "/plugins/settings/prayer-times/method", tk("Method"), tk("Twilight angles"), Choice(METHODS), Value::from("egyptian")),
        sd(Plugins, tk("Prayer times"), "/plugins/settings/prayer-times/asr", tk("Asr school"), tk("Shadow length rule"), Choice(SCHOOLS), Value::from("shafii")),
        sd(Plugins, tk("Prayer times"), "/plugins/settings/prayer-times/hijri_offset", tk("Hijri date adjustment"), tk("Days to add when your local moon sighting differs from the calculation"), slider(-2.0, 2.0, 1.0, " d"), Value::from(0)),
        sd(Plugins, tk("System tray"), "/plugins/settings/tray/show_passive", tk("Show idle items"), tk("Keep passive (idle) tray icons in the bar; they're always listed in the tray panel"), Toggle, Value::from(false)),
        sd(Plugins, tk("Screenshot"), "/plugins/settings/screenshot/copy", tk("Copy to clipboard"), tk("Enter / Capture copies the image"), Toggle, Value::from(true)),
        sd(Plugins, tk("Screenshot"), "/plugins/settings/screenshot/save", tk("Save to disk"), tk("~/Pictures/Screenshots · off = clipboard only"), Toggle, Value::from(true)),
        sd(Plugins, tk("Screenshot"), "/plugins/settings/screenshot/ocr_language", tk("Text recognition"), tk("Copy text (Ctrl+T) reads this script; Auto reads both"), Choice(const { &[("auto", tk("Auto (Latin + Arabic)")), ("latin", tk("Latin · English, Español, Français")), ("arabic", "العربية")] }), Value::from("auto")),
        sd(Plugins, tk("Screenshot"), "/plugins/settings/screenshot/delay", tk("Timer"), tk("Seconds to wait before a capture"), slider(0.0, 10.0, 1.0, " s"), Value::from(0)),
        sd(Plugins, tk("Screenshot"), "/plugins/settings/screenshot/countdown", tk("Countdown"), tk("Seconds “Capture in” waits so you can open a panel first (toolbar button, T, --delay)"), slider(1.0, 10.0, 1.0, " s"), Value::from(3)),
        sd(Plugins, tk("Weather"), "/plugins/settings/weather/location_name", tk("Place name"), tk("Shown on the weather card"), Text { placeholder: "El-Mandara" }, Value::from("El-Mandara")),
        sd(Plugins, tk("Weather"), "/plugins/settings/weather/latitude", tk("Latitude"), tk("Degrees north"), Number { min: -90.0, max: 90.0 }, Value::from(31.284)),
        sd(Plugins, tk("Weather"), "/plugins/settings/weather/longitude", tk("Longitude"), tk("Degrees east"), Number { min: -180.0, max: 180.0 }, Value::from(30.017)),
        sd(Plugins, tk("Weather"), "/plugins/settings/weather/animate", tk("Animated sky"), tk("Moving clouds, rain and stars (off with reduced motion)"), Toggle, Value::from(true)),
        sd(Plugins, tk("App installer"), "/plugins/settings/app-installer/install_dir", tk("Install location"), tk("Folder for AppImages and portable apps; empty uses ~/.local/share/mandara/apps"), Text { placeholder: "~/.local/share/mandara/apps" }, Value::from("")),
        sd(Plugins, tk("App installer"), "/plugins/settings/app-installer/path_shims", tk("Add to PATH by default"), tk("Start with “Add to PATH” switched on: a command in ~/.local/bin (never replacing an existing file)"), Toggle, Value::from(false)),
        sd(Plugins, tk("App installer"), "/plugins/settings/app-installer/confirm_rootless", tk("Confirm before installing"), tk("Show the confirmation card for AppImages, Flatpaks and portable apps too (native packages always ask)"), Toggle, Value::from(true)),
        sd(Plugins, tk("App installer"), "/plugins/settings/app-installer/flatpak_scope", tk("Flatpak installs"), tk("For you only needs no password; for everyone, Flatpak asks for it"), Choice(const { &[("user", tk("For me (--user)")), ("system", tk("For everyone (--system)"))] }), Value::from("user")),
        sd(Plugins, tk("App installer"), "/plugins/settings/app-installer/allow_native", tk("Allow native packages"), tk(".deb, .rpm, .pkg.tar.zst, .apk and .xbps through your package manager, always after its own preview and your password"), Toggle, Value::from(true)),
        sd(Plugins, tk("App installer"), "/plugins/settings/app-installer/after_install", tk("After installing"), tk("What happens to the file you installed from"), Choice(const { &[("keep", tk("Keep the file")), ("trash", tk("Move it to the Trash"))] }), Value::from("keep")),
        sd(Plugins, tk("App installer"), "/plugins/settings/app-installer/notify", tk("Notify when done"), tk("A notification when an install finishes in the background or from the command line"), Toggle, Value::from(true)),
        sd(Plugins, tk("Media"), "/plugins/settings/media/lyrics", tk("Synced lyrics"), tk("Time-synced lyrics from lrclib.net in the media panel (fetched only while it is open)"), Toggle, Value::from(true)),
        sd(Plugins, tk("Media"), "/plugins/settings/media/lyrics_rows", tk("Lyrics rows"), tk("Lines of lyrics shown around the one being sung"), slider(3.0, 9.0, 2.0, ""), Value::from(5)),
        sd(Plugins, tk("Pomodoro"), "/plugins/settings/pomodoro/focus_minutes", tk("Focus"), tk("Length of a focus session"), slider(5.0, 120.0, 5.0, " min"), Value::from(25)),
        sd(Plugins, tk("Pomodoro"), "/plugins/settings/pomodoro/short_break_minutes", tk("Short break"), tk("Between focus sessions"), slider(1.0, 30.0, 1.0, " min"), Value::from(5)),
        sd(Plugins, tk("Pomodoro"), "/plugins/settings/pomodoro/long_break_minutes", tk("Long break"), tk("After several sessions"), slider(5.0, 60.0, 5.0, " min"), Value::from(15)),
        sd(Plugins, tk("Pomodoro"), "/plugins/settings/pomodoro/long_break_every", tk("Long break every"), tk("Sessions before a long break"), slider(2.0, 8.0, 1.0, ""), Value::from(4)),
        sd(Plugins, tk("Pomodoro"), "/plugins/settings/pomodoro/auto_continue", tk("Auto-continue"), tk("Start the next phase automatically"), Toggle, Value::from(true)),
        sd(Plugins, tk("Drives"), "/plugins/settings/drives/notify_on_plug", tk("Announce new drives"), tk("A notification and a pulse on the chip when a drive is plugged in"), Toggle, Value::from(true)),
        sd(Plugins, tk("Drives"), "/plugins/settings/drives/show_system", tk("Show system partitions"), tk("Also list boot, EFI and other internal system partitions"), Toggle, Value::from(false)),
        sd(Plugins, tk("Containers"), "/plugins/settings/containers/engine", tk("Engine"), tk("Which container engine the panel talks to"), Choice(const { &[("auto", tk("Automatic")), ("docker", "Docker"), ("podman", "Podman")] }), Value::from("auto")),
        sd(Plugins, tk("Containers"), "/plugins/settings/containers/show_stopped", tk("Show stopped containers"), tk("List exited and created containers below the running ones"), Toggle, Value::from(true)),
        sd(Plugins, tk("Containers"), "/plugins/settings/containers/refresh_s", tk("Refresh every"), tk("How often the container list updates while the panel is closed (the open panel refreshes every 2 s)"), slider(5.0, 600.0, 5.0, " s"), Value::from(30)),
        sd(Plugins, tk("Containers"), "/plugins/settings/containers/log_lines", tk("Log lines"), tk("How many recent log lines the logs view loads"), slider(50.0, 1000.0, 50.0, ""), Value::from(200)),
        sd(Plugins, tk("Services"), "/plugins/settings/services/default_scope", tk("Default view"), tk("Which units the panel shows when it opens"), Choice(const { &[("system", tk("System")), ("user", tk("User"))] }), Value::from("system")),
        sd(Plugins, tk("Services"), "/plugins/settings/services/alert_failed", tk("Failed-unit alert"), tk("Show a badge on the chip when units have failed"), Toggle, Value::from(true)),
        sd(Plugins, tk("Services"), "/plugins/settings/services/journal_lines", tk("Journal lines"), tk("Recent log lines shown for a selected unit"), slider(10.0, 200.0, 10.0, ""), Value::from(40)),
        sd(Plugins, tk("Power"), "/plugins/settings/power/show_peripherals", tk("Peripheral batteries"), tk("List mice, keyboards and headsets that report a battery"), Toggle, Value::from(true)),
        sd(Plugins, tk("Power"), "/plugins/settings/power/history_hours", tk("Charge history"), tk("Time span of the battery charge sparkline"), slider(1.0, 48.0, 1.0, " h"), Value::from(12)),
        sd(Plugins, tk("Updates"), "/plugins/settings/updates/interval_hours", tk("Check every"), tk("How often to look for updates while the chip is in the bar"), slider(1.0, 24.0, 1.0, " h"), Value::from(3)),
        sd(Plugins, tk("Updates"), "/plugins/settings/updates/aur", tk("AUR packages"), tk("Also check AUR packages with paru or yay"), Toggle, Value::from(true)),
        sd(Plugins, tk("Updates"), "/plugins/settings/updates/flatpak", "Flatpak", tk("Also check Flatpak apps and runtimes"), Toggle, Value::from(true)),
        sd(Plugins, tk("Updates"), "/plugins/settings/updates/notify", tk("Notify about updates"), tk("A notification when new updates appear"), Toggle, Value::from(true)),
        sd(Plugins, tk("Updates"), "/plugins/settings/updates/arch_news", tk("Arch news"), tk("Latest archlinux.org news headline (fetched only while the panel is open)"), Toggle, Value::from(true)),
        sd(Plugins, tk("Updates"), "/plugins/settings/updates/news_count", tk("News headlines"), tk("How many of the latest Arch news posts the panel lists"), slider(1.0, 5.0, 1.0, ""), Value::from(1)),
        sd(Plugins, tk("Updates"), "/plugins/settings/updates/first_check_delay_s", tk("First check after"), tk("Seconds after login before the first update check"), slider(0.0, 600.0, 5.0, " s"), Value::from(20)),
        sd(Plugins, tk("Updates"), "/plugins/settings/updates/terminal", tk("Terminal"), tk("Runs “Update now”; empty detects ghostty, kitty, foot, alacritty or wezterm"), Text { placeholder: "ghostty" }, Value::from("")),
        sd(Plugins, tk("World clock"), "/plugins/settings/world-clock/cities", tk("Cities"), tk("Comma-separated city names (English or Arabic) or time zones like Europe/Lisbon"), Text { placeholder: "Alexandria, London, New York, Tokyo" }, Value::from("Alexandria, London, New York, Tokyo")),
        sd(Plugins, tk("World clock"), "/plugins/settings/world-clock/clock", tk("Time format"), tk("12- or 24-hour times; Follow the shell uses General → 24-hour clock"), Choice(const { &[("shell", tk("Follow the shell")), ("24", tk("24-hour")), ("12", tk("12-hour"))] }), Value::from("shell")),
        sd(Plugins, tk("World clock"), "/plugins/settings/world-clock/work_start", tk("Working day starts"), tk("Highlighted in each city's local time on the meeting planner"), slider(0.0, 23.0, 1.0, ":00"), Value::from(9)),
        sd(Plugins, tk("World clock"), "/plugins/settings/world-clock/work_end", tk("Working day ends"), tk("Highlighted in each city's local time on the meeting planner"), slider(0.0, 23.0, 1.0, ":00"), Value::from(17)),
        sd(Plugins, tk("World clock"), "/plugins/settings/world-clock/bar_cities", tk("Cities in the bar"), tk("How many of the first cities the bar chip shows (0 = just an icon)"), slider(0.0, 4.0, 1.0, ""), Value::from(2)),
        sd(Plugins, tk("Network speed"), "/plugins/settings/net-speed/unit", tk("Units"), tk("Rates in bytes (MB/s) or bits (Mb/s)"), Choice(const { &[("bytes", tk("Bytes · MB/s")), ("bits", tk("Bits · Mb/s"))] }), Value::from("bytes")),
        sd(Plugins, tk("Network speed"), "/plugins/settings/net-speed/bar_upload", tk("Upload in the bar"), tk("Show the upload rate next to download in the bar chip"), Toggle, Value::from(true)),
        sd(Plugins, tk("Network speed"), "/plugins/settings/net-speed/public_ip_url", tk("Public IP service"), tk("Queried once when the panel opens; must answer with the address as plain text"), Text { placeholder: "https://api.ipify.org" }, Value::from("https://api.ipify.org")),
        sd(Plugins, tk("Network speed"), "/plugins/settings/net-speed/speed_test_mb", tk("Speed test size"), tk("Downloaded from speed.cloudflare.com only when you press Run test"), slider(5.0, 200.0, 5.0, " MB"), Value::from(25)),
        sd(Plugins, tk("Authenticator"), "/plugins/settings/authenticator/clear_seconds", tk("Clear copied codes after"), tk("Wipes the clipboard if it still holds the code (0 = never)"), slider(0.0, 120.0, 5.0, " s"), Value::from(30)),
        sd(Plugins, tk("Color picker"), "/plugins/settings/color-picker/copy_on_pick", tk("Copy picked colors"), tk("Put the picked color on the clipboard right away"), Toggle, Value::from(true)),
        sd(Plugins, tk("Color picker"), "/plugins/settings/color-picker/copy_format", tk("Copy as"), tk("Format copied when a color is picked"), Choice(const { &[("hex", "HEX"), ("rgb", "RGB"), ("hsl", "HSL"), ("oklch", "OKLCH")] }), Value::from("hex")),
        sd(Plugins, tk("Color picker"), "/plugins/settings/color-picker/open_after_pick", tk("Open the panel after picking"), tk("Show formats, ramp and contrast for the new color"), Toggle, Value::from(true)),
        sd(Plugins, tk("Soundscapes"), "/plugins/settings/soundscapes/bar_timer", tk("Sleep timer in the bar"), tk("Show the minutes left next to the playing indicator"), Toggle, Value::from(true)),
        // Compositor
        s(Compositor, tk("Integration"), "/compositor/manage", tk("Manage compositor drop-in"), tk("Write rules, binds and colors to ~/.config/mandara/generated"), Toggle),
        s(Compositor, tk("Integration"), "/integrations/compositor_borders", tk("Sync border colors"), tk("Window borders follow the theme"), Toggle),
        s(Compositor, tk("Window look"), "/compositor/look/gaps_inner", tk("Gaps between windows"), tk("Unset keeps your compositor's value"), optional(0.0, 48.0, 1.0, " px", 10.0)),
        s(Compositor, tk("Window look"), "/compositor/look/gaps_outer", tk("Gaps to screen edges"), tk("Unset keeps your compositor's value"), optional(0.0, 64.0, 1.0, " px", 10.0)),
        s(Compositor, tk("Window look"), "/compositor/border_width", tk("Border width"), tk("Window border / focus ring thickness"), optional(0.0, 10.0, 1.0, " px", 2.0)),
        s(Compositor, tk("Window look"), "/compositor/look/rounding", tk("Corner radius"), tk("Rounded window corners"), optional(0.0, 32.0, 1.0, " px", 12.0)),
        s(Compositor, tk("Window look"), "/compositor/look/active_opacity", tk("Focused window opacity"), "", optional(0.3, 1.0, 0.01, "", 1.0)),
        s(Compositor, tk("Window look"), "/compositor/look/inactive_opacity", tk("Unfocused window opacity"), "", optional(0.3, 1.0, 0.01, "", 0.92)),
        s(Compositor, tk("Effects"), "/compositor/look/blur", tk("Blur"), tk("Background blur behind translucent windows (MangoWM, Hyprland, Niri 26.04+)"), Choice(TRI)),
        s(Compositor, tk("Effects"), "/compositor/look/shadows", tk("Shadows"), tk("Drop shadows under windows"), Choice(TRI)),
        s(Compositor, tk("Effects"), "/compositor/look/animations", tk("Animations"), tk("Compositor window animations"), Choice(TRI)),
        s(Compositor, tk("MangoWM"), "/compositor/mango_blur", tk("SceneFX blur"), tk("Background blur"), Toggle),
        s(Compositor, tk("MangoWM"), "/compositor/mango_blur_optimized", tk("Optimized blur"), tk("Faster blur on static backgrounds"), Toggle),
        // The help is the config key itself.
        s(Compositor, tk("MangoWM"), "/compositor/mango_sync_obj", tk("Explicit sync"), "sync_obj_enable", Toggle),
        s(Compositor, tk("MangoWM"), "/compositor/mango_fx/blur_radius", tk("Blur radius"), "blur_params_radius", optional(1.0, 20.0, 1.0, " px", 5.0)),
        s(Compositor, tk("MangoWM"), "/compositor/mango_fx/blur_passes", tk("Blur passes"), "blur_params_num_passes", optional(1.0, 6.0, 1.0, "", 2.0)),
        s(Compositor, tk("MangoWM"), "/compositor/mango_fx/shadows_size", tk("Shadow size"), "shadows_size", optional(0.0, 60.0, 1.0, " px", 10.0)),
        s(Compositor, tk("MangoWM"), "/compositor/mango_fx/shadows_blur", tk("Shadow softness"), "shadows_blur", optional(0.0, 60.0, 1.0, " px", 15.0)),
        s(Compositor, tk("Niri"), "/compositor/niri/default_column_width", tk("Default column width"), tk("New columns' share of the screen width"), optional(0.1, 1.0, 0.05, "", 0.5)),
        s(Compositor, tk("Niri"), "/compositor/niri/center_focused_column", tk("Center focused column"), tk("Keep the focused column in the middle of the screen"), Choice(const { &[("null", tk("Your config")), ("never", tk("Never")), ("on-overflow", tk("When it overflows")), ("always", tk("Always"))] })),
        s(Compositor, tk("Niri"), "/compositor/niri/always_center_single_column", tk("Center a lone column"), tk("A single column sits in the middle instead of at the left edge"), Choice(const { &[("null", tk("Your config")), ("true", tk("On"))] })),
        s(Compositor, tk("Hyprland"), "/compositor/hyprland/blur_size", tk("Blur size"), "decoration:blur:size", optional(1.0, 20.0, 1.0, " px", 8.0)),
        s(Compositor, tk("Hyprland"), "/compositor/hyprland/blur_passes", tk("Blur passes"), "decoration:blur:passes", optional(1.0, 6.0, 1.0, "", 2.0)),
        s(Compositor, tk("Hyprland"), "/compositor/hyprland/shadow_range", tk("Shadow range"), "decoration:shadow:range", optional(0.0, 60.0, 1.0, " px", 20.0)),
        s(Compositor, tk("Hyprland"), "/compositor/hyprland/dim_inactive", tk("Dim unfocused windows"), "decoration:dim_inactive", Choice(TRI)),
        s(Compositor, tk("Hyprland"), "/compositor/hyprland/dim_strength", tk("Dim strength"), "decoration:dim_strength", optional(0.0, 1.0, 0.05, "", 0.1)),
        // Audio & media
        sd(Audio, tk("Visualizer"), "/plugins/settings/media/visualizer_style", tk("Visualizer style"), tk("How the media panel draws the live audio spectrum"), Choice(const { &[("bars", tk("Mirrored bars")), ("wave", tk("Liquid wave")), ("ring", tk("Album ring"))] }), Value::from("bars")),
        sd(Audio, tk("Visualizer"), "/plugins/settings/media/visualizer_bars", tk("Bars"), tk("How many bars or dots the spectrum uses"), slider(8.0, 64.0, 2.0, ""), Value::from(32)),
        sd(Audio, tk("Visualizer"), "/plugins/settings/media/visualizer_smoothing", tk("Smoothing"), tk("Higher values fall back more slowly for a calmer motion"), slider(0.0, 0.95, 0.05, ""), Value::from(0.35)),
        // App themes (cross-app sync)
        s(Themes, tk("App themes"), "/integrations/ghostty", "Ghostty", tk("Terminal colors follow the theme"), Toggle),
        s(Themes, tk("App themes"), "/integrations/gtk4", tk("GTK 3 / 4 & libadwaita"), tk("Adds one import line to gtk.css (backed up first)"), Toggle),
        s(Themes, tk("App themes"), "/integrations/qt6", tk("Qt 5 / 6 (qt6ct, qt5ct)"), tk("A “mandara” color scheme for qt6ct / qt5ct"), Toggle),
        s(Themes, tk("App themes"), "/integrations/kvantum", "Kvantum", tk("A “Mandara” Kvantum theme in ~/.cache/mandara/themes/kvantum"), Toggle),
        s(Themes, tk("App themes"), "/integrations/kvantum_apply", tk("Use the Kvantum theme"), tk("Links it into ~/.config/Kvantum and runs kvantummanager --set Mandara"), Toggle),
        s(Themes, tk("App themes"), "/integrations/zed", "Zed", tk("A “Mandara” editor theme"), Toggle),
        s(Themes, tk("App themes"), "/integrations/fuzzel", "Fuzzel", tk("Launcher colors"), Toggle),
        s(Themes, tk("App themes"), "/integrations/btop", "btop", tk("A “mandara” btop theme"), Toggle),
    ]
}

/// Reads a setting's current value.
pub fn get(cfg: &ShellConfig, setting: &Setting) -> Value {
    let root = json_of(cfg);
    root.pointer(setting.path).cloned().filter(|v| !v.is_null() || setting.default.is_none()).or_else(|| setting.default.clone()).unwrap_or(Value::Null)
}

/// Writes `value` at the setting's path (creating intermediate objects) and
/// validates by deserializing the whole config.
pub fn set(cfg: &ShellConfig, setting: &Setting, value: Value) -> Result<ShellConfig, String> {
    let value = integral(value);
    let mut root = Value::clone(&json_of(cfg));
    if setting.path == WALLPAPER_SCHEME_PATH && !value.is_null() {
        use_wallpaper_colors(&mut root)?;
    }
    if HEX_COLOR_PATHS.contains(&setting.path)
        && let Some(text) = value.as_str()
        && mandara_theme::Color::from_hex(text.trim()).is_err()
    {
        return Err(tf("{value} isn't a color: use hex like #1e1e2e", &[("value", &text)]));
    }
    let mut cursor = &mut root;
    let mut parts = setting.path.trim_start_matches('/').split('/').peekable();
    while let Some(part) = parts.next() {
        if !cursor.is_object() {
            *cursor = Value::Object(Default::default());
        }
        let Value::Object(map) = cursor else { break };
        if parts.peek().is_none() {
            map.insert(part.to_string(), value);
            break;
        }
        cursor = map.entry(part.to_string()).or_insert_with(|| Value::Object(Default::default()));
    }
    if setting.path == "/general/units" {
        // An explicit choice retires the legacy weather-only Fahrenheit flag
        // (which otherwise still reads as Imperial).
        if let Some(weather) = root.pointer_mut("/plugins/settings/weather").and_then(Value::as_object_mut) {
            weather.remove("fahrenheit");
        }
    }
    serde_json::from_value::<ShellConfig>(root).map_err(|e| tf("invalid value for {setting}: {error}", &[("setting", &tr(setting.label)), ("error", &e)]))
}

/// Text settings holding a `#rrggbb` color (anything else would silently
/// fall back to black).
const HEX_COLOR_PATHS: &[&str] = &["/wallpaper/fill_color"];

/// The Material You scheme of wallpaper themes.
pub const WALLPAPER_SCHEME_PATH: &str = "/theme/source/variant";

/// Picking a wallpaper scheme while a designed theme is active switches to
/// colors from the wallpaper (remembering the theme, as the Themes page's
/// wallpaper card does); a preset has no scheme to change, so the choice
/// would otherwise be dropped silently.
fn use_wallpaper_colors(root: &mut Value) -> Result<(), String> {
    let kind = root.pointer("/theme/source/kind").and_then(Value::as_str).unwrap_or_default().to_string();
    if kind != "preset" {
        return Ok(());
    }
    let Some(wallpaper) = root.pointer("/wallpaper/path").filter(|p| p.is_string()).cloned() else {
        return Err(t("Pick a wallpaper first: wallpaper schemes take their colors from it").to_string());
    };
    let preset = root.pointer("/theme/source/preset").cloned().unwrap_or(Value::Null);
    if let Some(theme) = root.pointer_mut("/theme").and_then(Value::as_object_mut) {
        theme.insert("last_preset".into(), preset);
        theme.insert("source".into(), serde_json::json!({ "kind": "wallpaper", "path": wallpaper, "variant": Value::Null, "seed": 0 }));
    }
    if let Some(w) = root.pointer_mut("/wallpaper").and_then(Value::as_object_mut) {
        w.insert("match_colors".into(), Value::Bool(true));
    }
    Ok(())
}

/// Sliders produce `f64`s; a whole number becomes an integer so `u32`
/// fields accept it (float fields accept integers too).
pub fn integral(value: Value) -> Value {
    match value.as_f64() {
        Some(f) if value.is_f64() && f.fract() == 0.0 && f.abs() < 9.0e15 => Value::from(f as i64),
        _ => value,
    }
}

/// Converts a choice string to JSON (numbers / booleans stay typed).
pub fn choice_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::from(raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_path_resolves_and_round_trips() {
        let cfg = ShellConfig::default();
        for setting in settings() {
            let v = get(&cfg, setting);
            // Writing the current value back must always be valid.
            let back = set(&cfg, setting, v.clone()).unwrap_or_else(|e| panic!("{}: {e}", setting.path));
            assert_eq!(get(&back, setting), v, "{}", setting.path);
        }
    }

    #[test]
    fn writes_take_effect_and_invalid_values_are_rejected() {
        let cfg = ShellConfig::default();
        let all = settings();
        let height = all.iter().find(|s| s.path == "/bar/height").unwrap();
        let next = set(&cfg, height, Value::from(28.0)).unwrap();
        assert_eq!(next.bar.height, 28.0);
        let layout = all.iter().find(|s| s.path == "/bar/layout").unwrap();
        assert!(set(&cfg, layout, Value::from("sideways")).is_err());
        let lat = all.iter().find(|s| s.path.ends_with("prayer-times/latitude")).unwrap();
        let gaps = by_path("/compositor/look/gaps_inner").unwrap();
        let with_gaps = set(&cfg, gaps, Value::from(12.0)).unwrap();
        assert_eq!(with_gaps.compositor.look.gaps_inner, Some(12));
        let with_lat = set(&cfg, lat, Value::from(30.04)).unwrap();
        assert_eq!(with_lat.plugins.setting("prayer-times", "latitude", 0.0), 30.04);
    }

    #[test]
    fn choices_are_valid_values() {
        let cfg = ShellConfig::default();
        for setting in settings() {
            if let Control::Choice(choices) = &setting.control {
                if setting.path.starts_with("/theme/source") {
                    continue; // only valid for wallpaper sources
                }
                for (raw, _) in choices.iter() {
                    assert!(set(&cfg, setting, choice_value(raw)).is_ok(), "{} = {raw}", setting.path);
                }
            }
        }
    }

    #[test]
    fn units_choice_retires_legacy_fahrenheit() {
        let mut cfg = ShellConfig::default();
        cfg.plugins.settings.entry("weather".into()).or_default().insert("fahrenheit".into(), toml::Value::Boolean(true));
        assert_eq!(cfg.units(), mandara_config::Units::Imperial);
        let units = by_path("/general/units").unwrap();
        assert_eq!(get(&cfg, units), Value::from("metric"));
        let metric = set(&cfg, units, Value::from("metric")).unwrap();
        assert_eq!(metric.units(), mandara_config::Units::Metric);
        let imperial = set(&cfg, units, Value::from("imperial")).unwrap();
        assert_eq!(imperial.general.units, mandara_config::Units::Imperial);
        assert!(!settings().iter().any(|s| s.path.ends_with("fahrenheit")));
    }

    #[test]
    fn primed_json_matches_a_fresh_read() {
        let mut next = ShellConfig::default();
        next.bar.height = 31.0;
        let primed = Arc::new(ShellConfig::default());
        prime(&primed);
        let height = by_path("/bar/height").unwrap();
        // A different config is never served the primed tree.
        assert_eq!(get(&next, height), Value::from(31.0));
        assert_eq!(get(&primed, height), get(&ShellConfig::default(), height));
    }

    #[test]
    fn every_section_but_home_sits_in_exactly_one_group() {
        for section in Section::ALL {
            let groups = NavGroup::ALL.iter().filter(|g| g.sections().contains(&section)).count();
            assert_eq!(groups, usize::from(section != Section::Home), "{:?} is in {groups} groups", section.title());
        }
        assert_eq!(nav_order().count(), Section::ALL.len());
        for g in NavGroup::ALL {
            assert_eq!(NavGroup::parse(g.id()), Some(g));
        }
        assert_eq!(Section::Home.group(), None);
        assert_eq!(Section::Dock.group(), Some(NavGroup::Desktop));
    }

    #[test]
    fn search_matches_labels_and_sections() {
        let all = settings();
        assert!(all.iter().any(|s| s.matches("vivid")));
        assert!(all.iter().filter(|s| s.matches("prayer")).count() >= 4);
    }
}
