//! The app dock (`[dock]`, the built-in opt-in `dock` plugin): placement,
//! pinned apps and their order, recent apps, stacks, badges, hiding and
//! the glass it sits on.
//!
//! Re-exported from [`crate::workflow`] (its historical home), so
//! `mandara_config::workflow::DockConfig` keeps working. Every numeric
//! value has a named [`Span`] in [`ranges`]: Studio's sliders and the
//! shell's clamps ([`DockConfig::sanitized`]) read the same bounds. Old
//! configs load unchanged (`#[serde(default)]`); pinned ids written by
//! older builds are cleaned up by [`DockConfig::normalize_pins`].

use serde::{Deserialize, Serialize};

use crate::surfaces::Span;

/// Plugin id of the app dock (opt-in: on only when listed in
/// `plugins.enabled`).
pub const DOCK_PLUGIN_ID: &str = "dock";

/// Default values of the dock settings.
pub mod defaults {
    pub const DOCK_ICON_PX: f32 = 48.0;
    pub const DOCK_SPACING_PX: f32 = 6.0;
    pub const DOCK_PADDING_PX: f32 = 8.0;
    pub const DOCK_RADIUS_PX: f32 = 22.0;
    /// Gap between the dock and the screen edge, logical px.
    pub const DOCK_MARGIN_PX: f32 = 8.0;
    pub const DOCK_OPACITY: f32 = 1.0;
    /// Accent mixed into the glass (0 = the plain frame colour).
    pub const DOCK_TINT: f32 = 0.0;
    /// Peak scale of the icon under the pointer.
    pub const DOCK_MAGNIFY: f32 = 1.55;
    /// Icons on each side of the pointer that grow with it.
    pub const DOCK_MAGNIFY_SPREAD: f32 = 2.2;
    /// Opacity of pinned apps that aren't running.
    pub const DOCK_INACTIVE_OPACITY: f32 = 0.82;
    /// Pointer dwell before a hidden dock shows / leave delay before it hides, ms.
    pub const DOCK_SHOW_DELAY_MS: u64 = 120;
    pub const DOCK_HIDE_DELAY_MS: u64 = 450;
    /// Speed of the hide / show slide (1 = the default spring).
    pub const DOCK_HIDE_SPEED: f32 = 1.0;
    /// Recent apps shown after the running ones (macOS shows three).
    pub const DOCK_RECENT_COUNT: u32 = 3;
    /// Files listed in a stack's popover.
    pub const DOCK_STACK_ITEMS: u32 = 15;
    /// Most files a fan shows; "Automatic" fans folders with at most this
    /// many files and opens bigger ones as a grid (like macOS).
    pub const DOCK_FAN_ITEMS: u32 = 12;
    /// Icon size in the Applications grid, logical px.
    pub const DOCK_APPS_ICON_PX: f32 = 72.0;
    /// The stack every new dock has.
    pub const DOCK_DOWNLOADS: &str = "~/Downloads";
}

/// Named bounds of every numeric dock setting (logical px unless noted).
pub mod ranges {
    use super::Span;

    pub const ICON_SIZE: Span = Span::new(24.0, 128.0, 1.0);
    pub const SPACING: Span = Span::new(0.0, 24.0, 1.0);
    pub const PADDING: Span = Span::new(2.0, 24.0, 1.0);
    pub const RADIUS: Span = Span::new(0.0, 64.0, 1.0);
    pub const MARGIN: Span = Span::new(0.0, 40.0, 1.0);
    /// Glass opacity, 0–1.
    pub const OPACITY: Span = Span::new(0.0, 1.0, 0.05);
    /// Accent tint of the glass, 0–0.5.
    pub const TINT: Span = Span::new(0.0, 0.5, 0.01);
    pub const INACTIVE_OPACITY: Span = Span::new(0.3, 1.0, 0.05);
    /// Peak magnification, × the icon size.
    pub const MAGNIFY: Span = Span::new(1.0, 2.5, 0.05);
    /// Neighbours that grow along, in icons.
    pub const MAGNIFY_SPREAD: Span = Span::new(0.6, 4.0, 0.1);
    pub const SHOW_DELAY_MS: Span = Span::new(0.0, 1000.0, 10.0);
    pub const HIDE_DELAY_MS: Span = Span::new(0.0, 2000.0, 50.0);
    /// Hide / show animation speed, × the default.
    pub const HIDE_SPEED: Span = Span::new(0.25, 3.0, 0.05);
    /// Most apps (0 = no limit).
    pub const MAX_APPS: Span = Span::new(0.0, 30.0, 1.0);
    pub const RECENT_COUNT: Span = Span::new(0.0, 10.0, 1.0);
    pub const STACK_ITEMS: Span = Span::new(4.0, 40.0, 1.0);
    /// Files in a fan (a taller fan leaves the screen).
    pub const FAN_ITEMS: Span = Span::new(4.0, 20.0, 1.0);
    /// Icon size in the Applications grid.
    pub const APPS_ICON_SIZE: Span = Span::new(48.0, 128.0, 4.0);
}

/// Screen edge the dock sits on.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DockPosition {
    #[default]
    Bottom,
    Left,
    Right,
}

/// Where along its edge the dock sits.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DockAlignment {
    Start,
    #[default]
    Center,
    End,
}

/// When the dock hides.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DockAutoHide {
    /// Always visible (macOS's default).
    #[default]
    Never,
    /// Hidden until the pointer touches its edge ("Automatically hide and
    /// show the Dock").
    Always,
    /// Hidden only while a window overlaps it (or one is fullscreen).
    Smart,
}

/// Running-app indicator under each icon.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DockIndicator {
    /// One dot per window (up to three).
    #[default]
    Dots,
    /// A short bar that widens on the focused app.
    Bar,
    /// One dot per app, like macOS.
    Dot,
    None,
}

/// Which displays show the dock.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DockDisplays {
    #[default]
    All,
    /// The first output only.
    Primary,
    /// Only the outputs listed in `monitors`.
    Listed,
}

/// How a stack opens.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StackView {
    /// Fan for a few items, grid for more (macOS "Automatic").
    #[default]
    Auto,
    Fan,
    Grid,
    List,
}

/// Order of a stack's files.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StackSort {
    /// Newest first, by when the file appeared in the folder.
    #[default]
    DateAdded,
    DateModified,
    Name,
    Kind,
}

/// What a stack's dock icon shows.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StackDisplay {
    /// The newest file on top of a folder (macOS "Stack").
    #[default]
    Stack,
    /// The folder icon.
    Folder,
}

/// How the Applications folder opens.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppsView {
    /// Large icons on a Launchpad-style grid.
    #[default]
    Grid,
    /// Rows with a small icon and the app's description.
    List,
}

/// Order of the Applications folder.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppsOrder {
    /// A–Z by name.
    #[default]
    Name,
    /// Most used first (the launcher's frecency), then A–Z.
    MostUsed,
    /// Grouped under their `.desktop` category, A–Z inside.
    Categories,
}

/// The Applications folder on the right side of the dock (`[dock.applications]`):
/// a Launchpad-style grid of every installed app, and the drop target that
/// installs apps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DockApplications {
    pub enabled: bool,
    pub view: AppsView,
    pub order: AppsOrder,
    /// Icon size in the grid, logical px.
    pub icon_size: f32,
}

impl Default for DockApplications {
    fn default() -> Self {
        Self { enabled: true, view: AppsView::Grid, order: AppsOrder::Name, icon_size: defaults::DOCK_APPS_ICON_PX }
    }
}

/// A folder on the right side of the dock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DockStack {
    /// Folder path (`~` expands to your home).
    pub path: String,
    pub view: StackView,
    pub sort: StackSort,
    pub display: StackDisplay,
}

impl Default for DockStack {
    fn default() -> Self {
        Self { path: defaults::DOCK_DOWNLOADS.into(), view: StackView::default(), sort: StackSort::default(), display: StackDisplay::default() }
    }
}

impl DockStack {
    pub fn folder(path: &str) -> Self {
        Self { path: path.trim().to_string(), ..Self::default() }
    }

    /// The folder with `~` expanded against `home`.
    pub fn resolve(&self, home: &std::path::Path) -> std::path::PathBuf {
        let p = self.path.trim();
        match p.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None if p == "~" => home.to_path_buf(),
            None => std::path::PathBuf::from(p),
        }
    }
}

/// The app dock (the built-in `dock` plugin).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DockConfig {
    // ── Layout ──
    pub position: DockPosition,
    pub alignment: DockAlignment,
    pub displays: DockDisplays,
    /// Output names for [`DockDisplays::Listed`].
    pub monitors: Vec<String>,
    /// Only windows on the dock's own output count as running.
    pub isolate_displays: bool,

    // ── Apps ──
    /// Pinned apps (desktop ids without `.desktop`), in order.
    pub pinned: Vec<String>,
    pub show_running: bool,
    /// Recently used apps that aren't pinned or running, after the running
    /// ones ("Show suggested and recent apps in Dock").
    pub show_recent: bool,
    pub recent_count: u32,
    /// Hairlines between the apps, the recent apps and the right side
    /// (stacks, minimized windows, trash).
    pub separator: bool,
    /// Window-count badge on apps with two or more windows.
    pub badges: bool,
    /// Unread-notification count on the app's icon.
    pub notification_badges: bool,
    /// A progress ring from notifications that report progress.
    pub progress_badges: bool,
    pub indicator: DockIndicator,
    /// Most apps shown (0 = no limit).
    pub max_apps: u32,
    /// A launcher button at the dock's start (or end).
    pub launcher_button: bool,
    pub launcher_at_end: bool,
    /// The Applications folder on the right side (a Launchpad-style grid
    /// of your apps; drop apps on it to install them). Replaces the
    /// `applications_slot` switch of 0.1, which every config saved as off
    /// before the folder existed, so old configs get it on.
    pub applications: DockApplications,
    /// A trash entry at the end (drop files to trash them; right-click to
    /// empty it).
    pub trash: bool,
    /// Minimized windows stay in their app's icon; off puts each one on
    /// the right side of the dock.
    pub minimize_to_app: bool,
    /// A now-playing chip (album art, title, play / pause) at the dock's
    /// end while a media player is active.
    pub media_chip: bool,

    // ── Behaviour ──
    pub auto_hide: DockAutoHide,
    /// Reserve its space so windows tile around it (only when never hiding).
    pub reserve_space: bool,
    /// Above fullscreen windows (overlay layer).
    pub overlay: bool,
    pub show_delay_ms: u64,
    pub hide_delay_ms: u64,
    /// Speed of the hide / show slide.
    pub hide_speed: f32,
    /// Icons bounce while their app starts ("Animate opening applications").
    pub bounce_on_launch: bool,
    /// Icons bounce when their app wants attention (an urgent window or a
    /// critical notification).
    pub bounce_on_attention: bool,
    /// App names above the icons on hover.
    pub tooltips: bool,
    /// Icons grow under the pointer.
    pub magnification: bool,
    pub magnify: f32,
    pub magnify_spread: f32,

    // ── Stacks ──
    pub stacks: Vec<DockStack>,
    /// Files listed when a stack opens.
    pub stack_items: u32,
    /// Most files a fan shows (Automatic fans folders this small).
    pub fan_items: u32,
    /// File names beside fan items and under grid icons.
    pub stack_names: bool,

    // ── Appearance ──
    pub icon_size: f32,
    pub spacing: f32,
    pub padding: f32,
    pub radius: f32,
    pub margin: f32,
    /// Background opacity (0–1).
    pub opacity: f32,
    /// Accent tint of the glass (0–0.5).
    pub tint: f32,
    /// 1 px specular rim on the glass.
    pub rim: bool,
    pub shadow: bool,
    /// Opacity of pinned apps that aren't running.
    pub inactive_opacity: f32,
}

impl Default for DockConfig {
    fn default() -> Self {
        Self {
            position: DockPosition::Bottom,
            alignment: DockAlignment::Center,
            displays: DockDisplays::All,
            monitors: Vec::new(),
            isolate_displays: false,
            pinned: ["org.gnome.Nautilus", "firefox", "com.mitchellh.ghostty", "dev.zed.Zed"].iter().map(|s| s.to_string()).collect(),
            show_running: true,
            show_recent: true,
            recent_count: defaults::DOCK_RECENT_COUNT,
            separator: true,
            badges: true,
            notification_badges: true,
            progress_badges: true,
            indicator: DockIndicator::Dots,
            max_apps: 0,
            launcher_button: true,
            launcher_at_end: false,
            applications: DockApplications::default(),
            trash: true,
            minimize_to_app: true,
            media_chip: false,
            auto_hide: DockAutoHide::Never,
            reserve_space: true,
            overlay: false,
            show_delay_ms: defaults::DOCK_SHOW_DELAY_MS,
            hide_delay_ms: defaults::DOCK_HIDE_DELAY_MS,
            hide_speed: defaults::DOCK_HIDE_SPEED,
            bounce_on_launch: true,
            bounce_on_attention: true,
            tooltips: true,
            magnification: true,
            magnify: defaults::DOCK_MAGNIFY,
            magnify_spread: defaults::DOCK_MAGNIFY_SPREAD,
            stacks: vec![DockStack::default()],
            stack_items: defaults::DOCK_STACK_ITEMS,
            fan_items: defaults::DOCK_FAN_ITEMS,
            stack_names: true,
            icon_size: defaults::DOCK_ICON_PX,
            spacing: defaults::DOCK_SPACING_PX,
            padding: defaults::DOCK_PADDING_PX,
            radius: defaults::DOCK_RADIUS_PX,
            margin: defaults::DOCK_MARGIN_PX,
            opacity: defaults::DOCK_OPACITY,
            tint: defaults::DOCK_TINT,
            rim: true,
            shadow: true,
            inactive_opacity: defaults::DOCK_INACTIVE_OPACITY,
        }
    }
}

/// `id` as the dock stores it: trimmed, without `.desktop`.
pub fn pin_id(id: &str) -> String {
    let id = id.trim();
    id.strip_suffix(".desktop").unwrap_or(id).to_string()
}

impl DockConfig {
    /// The dock's resting footprint from its screen edge: margin, icons and
    /// padding (also the exclusive zone it reserves), logical px.
    pub fn band(&self) -> f32 {
        self.margin + self.icon_size + self.padding * 2.0
    }

    /// Whether the dock appears on output `name` (`index` in output order).
    pub fn shows_on(&self, name: &str, index: usize) -> bool {
        match self.displays {
            DockDisplays::All => true,
            DockDisplays::Primary => index == 0,
            DockDisplays::Listed => self.monitors.iter().any(|m| m == name),
        }
    }

    /// Every numeric value inside its [`ranges`] span (hand-edited configs
    /// never break the layout).
    pub fn sanitized(&self) -> Self {
        let d = Self::default();
        let ms = |v: u64, r: Span| (v as f32).clamp(r.min, r.max) as u64;
        let count = |v: u32, r: Span| (v as f32).clamp(r.min, r.max) as u32;
        Self {
            icon_size: ranges::ICON_SIZE.clamp(self.icon_size, d.icon_size),
            spacing: ranges::SPACING.clamp(self.spacing, d.spacing),
            padding: ranges::PADDING.clamp(self.padding, d.padding),
            radius: ranges::RADIUS.clamp(self.radius, d.radius),
            margin: ranges::MARGIN.clamp(self.margin, d.margin),
            opacity: ranges::OPACITY.clamp(self.opacity, d.opacity),
            tint: ranges::TINT.clamp(self.tint, d.tint),
            inactive_opacity: ranges::INACTIVE_OPACITY.clamp(self.inactive_opacity, d.inactive_opacity),
            magnify: ranges::MAGNIFY.clamp(self.magnify, d.magnify),
            magnify_spread: ranges::MAGNIFY_SPREAD.clamp(self.magnify_spread, d.magnify_spread),
            hide_speed: ranges::HIDE_SPEED.clamp(self.hide_speed, d.hide_speed),
            show_delay_ms: ms(self.show_delay_ms, ranges::SHOW_DELAY_MS),
            hide_delay_ms: ms(self.hide_delay_ms, ranges::HIDE_DELAY_MS),
            max_apps: count(self.max_apps, ranges::MAX_APPS),
            recent_count: count(self.recent_count, ranges::RECENT_COUNT),
            stack_items: count(self.stack_items, ranges::STACK_ITEMS),
            fan_items: count(self.fan_items, ranges::FAN_ITEMS),
            applications: DockApplications { icon_size: ranges::APPS_ICON_SIZE.clamp(self.applications.icon_size, d.applications.icon_size), ..self.applications.clone() },
            ..self.clone()
        }
    }

    // ── Pins ──

    /// Index of pinned app `id` (case-insensitive, `.desktop` optional).
    pub fn pin_index(&self, id: &str) -> Option<usize> {
        let id = pin_id(id);
        self.pinned.iter().position(|p| p.eq_ignore_ascii_case(&id))
    }

    pub fn is_pinned(&self, id: &str) -> bool {
        self.pin_index(id).is_some()
    }

    /// Pins `id` at the end (a pinned app stays where it is). Returns
    /// whether it was added.
    pub fn pin(&mut self, id: &str) -> bool {
        !self.is_pinned(id) && self.pin_at(id, usize::MAX)
    }

    /// Pins `id` at `index` (clamped), or moves it there when already
    /// pinned. Returns whether anything changed.
    pub fn pin_at(&mut self, id: &str, index: usize) -> bool {
        let id = pin_id(id);
        if id.is_empty() {
            return false;
        }
        let before = self.pinned.clone();
        if let Some(i) = self.pin_index(&id) {
            self.pinned.remove(i);
        }
        let at = index.min(self.pinned.len());
        self.pinned.insert(at, id);
        self.pinned != before
    }

    /// Unpins `id`. Returns whether it was pinned.
    pub fn unpin(&mut self, id: &str) -> bool {
        match self.pin_index(id) {
            Some(i) => {
                self.pinned.remove(i);
                true
            }
            None => false,
        }
    }

    /// "Keep in Dock": pins or unpins `id`. Returns whether it is pinned now.
    pub fn toggle_pin(&mut self, id: &str) -> bool {
        if self.unpin(id) {
            false
        } else {
            self.pin(id)
        }
    }

    /// Moves the pin at `from` to `to` (both clamped).
    pub fn move_pin(&mut self, from: usize, to: usize) -> bool {
        if from >= self.pinned.len() || self.pinned.is_empty() {
            return false;
        }
        let to = to.min(self.pinned.len() - 1);
        if from == to {
            return false;
        }
        let v = self.pinned.remove(from);
        self.pinned.insert(to, v);
        true
    }

    /// Cleans pins written by older builds: trims them, drops `.desktop`
    /// suffixes, empty entries and duplicates (first wins, any case), and
    /// drops stacks without a folder. Returns whether anything changed.
    pub fn normalize_pins(&mut self) -> bool {
        let before = (self.pinned.clone(), self.stacks.len());
        let mut out: Vec<String> = Vec::with_capacity(self.pinned.len());
        for p in &self.pinned {
            let id = pin_id(p);
            if !id.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(&id)) {
                out.push(id);
            }
        }
        self.pinned = out;
        self.stacks.retain(|s| !s.path.trim().is_empty());
        (self.pinned.clone(), self.stacks.len()) != before
    }

    // ── Stacks ──

    /// Adds a stack for `folder` (no duplicates). Returns whether it was added.
    pub fn add_stack(&mut self, folder: &str) -> bool {
        let folder = folder.trim().trim_end_matches('/');
        if folder.is_empty() || self.stacks.iter().any(|s| s.path.trim().trim_end_matches('/') == folder) {
            return false;
        }
        self.stacks.push(DockStack::folder(folder));
        true
    }

    /// Removes the stack for `folder` (or at that index, when it parses as
    /// one). Returns whether one was removed.
    pub fn remove_stack(&mut self, folder: &str) -> bool {
        let folder = folder.trim().trim_end_matches('/');
        let before = self.stacks.len();
        match self.stacks.iter().position(|s| s.path.trim().trim_end_matches('/') == folder) {
            Some(i) => {
                self.stacks.remove(i);
            }
            None => {
                if let Ok(i) = folder.parse::<usize>()
                    && i < self.stacks.len()
                {
                    self.stacks.remove(i);
                }
            }
        }
        self.stacks.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_unpin_toggle() {
        let mut d = DockConfig { pinned: vec!["firefox".into()], ..DockConfig::default() };
        assert!(d.pin("org.demo.Alpha.desktop"));
        assert_eq!(d.pinned, ["firefox", "org.demo.Alpha"]);
        assert!(!d.pin("ORG.DEMO.ALPHA"), "already pinned (any case)");
        assert!(d.is_pinned("org.demo.alpha.desktop"));
        assert!(!d.toggle_pin("firefox"), "toggle unpins a pinned app");
        assert_eq!(d.pinned, ["org.demo.Alpha"]);
        assert!(d.toggle_pin("firefox"), "and pins it again");
        assert_eq!(d.pinned, ["org.demo.Alpha", "firefox"]);
        assert!(d.unpin("firefox") && !d.unpin("firefox"));
        assert!(!d.pin("  "), "blank ids are refused");
    }

    #[test]
    fn pin_order_and_moves() {
        let mut d = DockConfig { pinned: vec!["a".into(), "b".into(), "c".into()], ..DockConfig::default() };
        assert!(d.pin_at("x", 1));
        assert_eq!(d.pinned, ["a", "x", "b", "c"]);
        assert!(d.pin_at("c", 0), "pinning a pinned app moves it");
        assert_eq!(d.pinned, ["c", "a", "x", "b"]);
        assert!(d.move_pin(0, 99));
        assert_eq!(d.pinned, ["a", "x", "b", "c"]);
        assert!(!d.move_pin(9, 0) && !d.move_pin(1, 1));
    }

    #[test]
    fn old_pinned_lists_migrate() {
        // A pre-0.2 dock: ids with `.desktop`, blanks and duplicates, no
        // stacks key at all.
        let old: DockConfig = toml::from_str("pinned = [\"firefox.desktop\", \" org.gnome.Nautilus \", \"\", \"Firefox\"]\nauto_hide = \"smart\"\n").unwrap();
        assert_eq!(old.auto_hide, DockAutoHide::Smart, "explicit choices survive");
        assert_eq!(old.stacks, vec![DockStack::default()], "new keys take their defaults");
        let mut m = old.clone();
        assert!(m.normalize_pins());
        assert_eq!(m.pinned, ["firefox", "org.gnome.Nautilus"]);
        assert!(!m.normalize_pins(), "idempotent");
        let text = toml::to_string(&m).unwrap();
        assert_eq!(toml::from_str::<DockConfig>(&text).unwrap(), m);
    }

    #[test]
    fn the_applications_folder_comes_on_over_the_old_switch() {
        // 0.1 saved `applications_slot = false` into every config.
        let old: DockConfig = toml::from_str("applications_slot = false\nstack_items = 9\n").unwrap();
        assert!(old.applications.enabled, "the folder is on by default");
        assert_eq!((old.applications.view, old.applications.order), (AppsView::Grid, AppsOrder::Name));
        assert_eq!(old.fan_items, defaults::DOCK_FAN_ITEMS);
        let off: DockConfig = toml::from_str("[applications]\nenabled = false\norder = \"most-used\"\n").unwrap();
        assert!(!off.applications.enabled);
        assert_eq!(off.applications.order, AppsOrder::MostUsed);
        let wild = DockConfig { fan_items: 500, applications: DockApplications { icon_size: 4.0, ..Default::default() }, ..DockConfig::default() }.sanitized();
        assert_eq!(wild.fan_items, ranges::FAN_ITEMS.max as u32);
        assert_eq!(wild.applications.icon_size, ranges::APPS_ICON_SIZE.min);
    }

    #[test]
    fn sanitized_clamps_to_ranges() {
        let wild = DockConfig { icon_size: 900.0, opacity: f32::NAN, magnify: 0.2, recent_count: 99, hide_delay_ms: 60_000, ..DockConfig::default() };
        let s = wild.sanitized();
        assert_eq!(s.icon_size, ranges::ICON_SIZE.max);
        assert_eq!(s.opacity, defaults::DOCK_OPACITY);
        assert_eq!(s.magnify, ranges::MAGNIFY.min);
        assert_eq!(s.recent_count, ranges::RECENT_COUNT.max as u32);
        assert_eq!(s.hide_delay_ms, ranges::HIDE_DELAY_MS.max as u64);
        assert_eq!(DockConfig::default().sanitized(), DockConfig::default(), "defaults sit inside their ranges");
    }

    #[test]
    fn stacks_add_remove_and_resolve() {
        let mut d = DockConfig::default();
        assert!(!d.add_stack("~/Downloads/"), "no duplicates");
        assert!(d.add_stack("~/Documents"));
        assert_eq!(d.stacks.len(), 2);
        assert_eq!(d.stacks[1].resolve(std::path::Path::new("/home/u")), std::path::PathBuf::from("/home/u/Documents"));
        assert_eq!(DockStack::folder("/srv/x").resolve(std::path::Path::new("/home/u")), std::path::PathBuf::from("/srv/x"));
        assert!(d.remove_stack("0"));
        assert_eq!(d.stacks[0].path, "~/Documents");
        assert!(d.remove_stack("~/Documents") && d.stacks.is_empty());
    }

    #[test]
    fn display_filter() {
        let mut d = DockConfig::default();
        assert!(d.shows_on("DP-1", 3));
        d.displays = DockDisplays::Primary;
        assert!(d.shows_on("DP-1", 0) && !d.shows_on("DP-2", 1));
        d.displays = DockDisplays::Listed;
        d.monitors = vec!["HDMI-A-1".into()];
        assert!(d.shows_on("HDMI-A-1", 1) && !d.shows_on("DP-1", 0));
    }
}
