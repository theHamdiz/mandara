//! Displays, night light and per-output surface assignment: the
//! `[displays]` and `[night_light]` config sections, plus the monitor
//! lines Mandara persists to the compositor drop-ins
//! (`[[compositor.outputs]]` → MangoWM `monitor_rule=`, Niri `output {}`,
//! Hyprland `monitor =`).
//!
//! The shell surfaces (bar, OSD, notifications, wallpaper) ask
//! [`assignment`] whether they belong on an output.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::compositor::{Target, include_path, is_generated, resolve_include};

/// Default values, in one place.
pub mod defaults {
    /// Colour temperature of untinted daylight (sRGB's D65 white), kelvin.
    pub const NEUTRAL_TEMPERATURE_K: u32 = 6500;
    /// Default night temperature: warm but still colour-faithful.
    pub const NIGHT_TEMPERATURE_K: u32 = 4000;
    /// Warmest / coolest temperature Studio and the CLI accept.
    pub const MIN_TEMPERATURE_K: u32 = 1000;
    pub const MAX_TEMPERATURE_K: u32 = 10000;
    /// Fixed schedule: warm from 20:00 until 07:00.
    pub const NIGHT_START: &str = "20:00";
    pub const NIGHT_END: &str = "07:00";
    /// Fade between day and night temperatures, minutes.
    pub const TRANSITION_MINUTES: u32 = 30;
    /// Longest fade Studio offers, minutes.
    pub const MAX_TRANSITION_MINUTES: u32 = 180;
    /// Neutral gamma exponent and brightness multiplier of the ramp.
    pub const NEUTRAL_GAMMA: f32 = 1.0;
    pub const NEUTRAL_BRIGHTNESS: f32 = 1.0;
    /// "Test" display changes revert after this many seconds unless kept.
    pub const REVERT_SECONDS: u32 = 15;
    /// Shortest / longest revert countdown accepted.
    pub const MIN_REVERT_SECONDS: u32 = 5;
    pub const MAX_REVERT_SECONDS: u32 = 60;
    /// External monitors are never dimmed below this percentage over DDC/CI.
    pub const MIN_BRIGHTNESS_PERCENT: u32 = 1;
    /// The highest brightness floor accepted (Studio's slider maximum): a
    /// floor above the ceiling would make every clamp panic.
    pub const MAX_BRIGHTNESS_FLOOR_PERCENT: u32 = 50;
}

// ── Night light ─────────────────────────────────────────────────────────────

/// When the night light warms the screens.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NightSchedule {
    /// From sunset to sunrise at the configured location.
    #[default]
    Sun,
    /// From `start` to `end` every day (local time).
    Fixed,
    /// Always warm while enabled.
    Always,
}

/// `[night_light]`: screen colour temperature through
/// `wlr-gamma-control-unstable-v1` (MangoWM, Hyprland, Sway; Niri lacks it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NightLightConfig {
    pub enabled: bool,
    pub schedule: NightSchedule,
    /// Kelvin at night / during the day (6500 = untinted).
    pub night_temperature: u32,
    pub day_temperature: u32,
    /// Fixed schedule bounds, `HH:MM` local time.
    pub start: String,
    pub end: String,
    /// Fade length after each boundary, minutes (0 = switch at once).
    pub transition_minutes: u32,
    /// Extra gamma exponent applied to the ramp (1 = unchanged).
    pub gamma: f32,
    /// Software brightness multiplier at night (1 = unchanged).
    pub brightness: f32,
    /// Location for the sun schedule; unset = the weather / prayer-times
    /// location.
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    /// Pause while the focused window is fullscreen (games, video).
    pub pause_fullscreen: bool,
    /// App ids that pause the night light while focused (colour work).
    pub excluded_apps: Vec<String>,
}

impl Default for NightLightConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            schedule: NightSchedule::Sun,
            night_temperature: defaults::NIGHT_TEMPERATURE_K,
            day_temperature: defaults::NEUTRAL_TEMPERATURE_K,
            start: defaults::NIGHT_START.into(),
            end: defaults::NIGHT_END.into(),
            transition_minutes: defaults::TRANSITION_MINUTES,
            gamma: defaults::NEUTRAL_GAMMA,
            brightness: defaults::NEUTRAL_BRIGHTNESS,
            latitude: None,
            longitude: None,
            pause_fullscreen: false,
            excluded_apps: Vec::new(),
        }
    }
}

impl NightLightConfig {
    /// Whether focusing `app_id` pauses the night light (case-insensitive).
    pub fn excludes(&self, app_id: &str) -> bool {
        !app_id.is_empty() && self.excluded_apps.iter().any(|a| a.eq_ignore_ascii_case(app_id))
    }
}

// ── Outputs ─────────────────────────────────────────────────────────────────

/// Output rotation / flip, in `wl_output.transform` order (rotations are
/// counter-clockwise, as in the protocol).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transform {
    #[default]
    Normal,
    #[serde(rename = "90")]
    Rotate90,
    #[serde(rename = "180")]
    Rotate180,
    #[serde(rename = "270")]
    Rotate270,
    Flipped,
    #[serde(rename = "flipped-90")]
    Flipped90,
    #[serde(rename = "flipped-180")]
    Flipped180,
    #[serde(rename = "flipped-270")]
    Flipped270,
}

impl Transform {
    pub const ALL: [Transform; 8] = [
        Transform::Normal,
        Transform::Rotate90,
        Transform::Rotate180,
        Transform::Rotate270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ];

    /// The `wl_output.transform` value (also MangoWM `rr` and Hyprland's
    /// `transform`).
    pub fn wl(self) -> u32 {
        Self::ALL.iter().position(|t| *t == self).unwrap_or_default() as u32
    }

    pub fn from_wl(v: u32) -> Self {
        Self::ALL.get(v as usize).copied().unwrap_or_default()
    }

    /// Niri's / the CLI's name (`normal`, `90`, `flipped-270`…).
    pub fn name(self) -> &'static str {
        match self {
            Transform::Normal => "normal",
            Transform::Rotate90 => "90",
            Transform::Rotate180 => "180",
            Transform::Rotate270 => "270",
            Transform::Flipped => "flipped",
            Transform::Flipped90 => "flipped-90",
            Transform::Flipped180 => "flipped-180",
            Transform::Flipped270 => "flipped-270",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.name().eq_ignore_ascii_case(s.trim()))
    }

    /// Quarter turns swap width and height.
    pub fn swaps_axes(self) -> bool {
        self.wl() % 2 == 1
    }
}

/// Variable refresh rate.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VrrMode {
    #[default]
    Off,
    On,
    /// Only while a fullscreen window shows (Hyprland `vrr, 2`, Niri
    /// `on-demand`; elsewhere the same as on).
    Fullscreen,
}

/// The settings of one output, applied live over
/// `wlr-output-management` and persisted to the compositor drop-ins.
/// Unset fields keep the compositor's value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    /// Connector name (`HDMI-A-1`, `eDP-1`).
    pub name: String,
    /// EDID identity, used to recognise the monitor on another port.
    pub make: String,
    pub model: String,
    pub serial: String,
    pub enabled: bool,
    /// Mode in hardware pixels and millihertz.
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub refresh_mhz: Option<u32>,
    pub scale: Option<f32>,
    pub transform: Option<Transform>,
    /// Top-left corner in the global logical space.
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub vrr: Option<VrrMode>,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            make: String::new(),
            model: String::new(),
            serial: String::new(),
            enabled: true,
            width: None,
            height: None,
            refresh_mhz: None,
            scale: None,
            transform: None,
            x: None,
            y: None,
            vrr: None,
        }
    }
}

/// Millihertz per hertz.
pub const MHZ_PER_HZ: f64 = 1000.0;
/// Smallest / largest output scale written anywhere.
pub const MIN_SCALE: f32 = 0.25;
pub const MAX_SCALE: f32 = 10.0;

impl OutputConfig {
    /// The monitor's identity: `make model serial` when the EDID has a
    /// serial (stable across ports), otherwise the connector name.
    pub fn identity(&self) -> String {
        identity(&self.name, &self.make, &self.model, &self.serial)
    }

    /// `"3840x2160@60.000"` when a full mode is set.
    pub fn mode_string(&self) -> Option<String> {
        let (w, h) = (self.width?, self.height?);
        Some(match self.refresh_mhz {
            Some(r) if r > 0 => format!("{w}x{h}@{:.3}", r as f64 / MHZ_PER_HZ),
            _ => format!("{w}x{h}"),
        })
    }

    fn scale_clamped(&self) -> Option<f32> {
        self.scale.filter(|s| *s > 0.0).map(|s| s.clamp(MIN_SCALE, MAX_SCALE))
    }

    /// Whether anything besides the identity is set (otherwise nothing is
    /// written for it).
    pub fn has_settings(&self) -> bool {
        !self.enabled || self.mode_string().is_some() || self.scale_clamped().is_some() || self.transform.is_some() || (self.x.is_some() && self.y.is_some()) || self.vrr.is_some()
    }
}

/// See [`OutputConfig::identity`].
pub fn identity(name: &str, make: &str, model: &str, serial: &str) -> String {
    if serial.trim().is_empty() || make.trim().is_empty() { name.to_string() } else { format!("{} {} {}", make.trim(), model.trim(), serial.trim()) }
}

/// A saved arrangement, applied automatically when exactly its monitors
/// are connected.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayProfile {
    pub name: String,
    pub outputs: Vec<OutputConfig>,
}

impl DisplayProfile {
    /// The set of monitor identities this profile was saved with.
    pub fn identities(&self) -> BTreeSet<String> {
        self.outputs.iter().map(OutputConfig::identity).collect()
    }

    /// Whether the connected monitors (by [`identity`]) are exactly this
    /// profile's.
    pub fn matches(&self, connected: &BTreeSet<String>) -> bool {
        !self.outputs.is_empty() && self.identities() == *connected
    }
}

/// Brightness of external monitors over DDC/CI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExternalBrightness {
    /// Control external monitors over DDC/CI (`/dev/i2c-*`, i2c group).
    pub ddc: bool,
    /// Brightness keys change every monitor together (laptop panel and
    /// external ones); desktops without a backlight always use DDC/CI.
    pub sync_all: bool,
    /// Never dim below this percentage.
    pub min_percent: u32,
}

impl Default for ExternalBrightness {
    fn default() -> Self {
        Self { ddc: true, sync_all: false, min_percent: defaults::MIN_BRIGHTNESS_PERCENT }
    }
}

impl ExternalBrightness {
    /// `min_percent` limited to what Studio offers, so a hand-edited
    /// `min_percent = 150` can't exceed 100 % (where `clamp` panics).
    pub fn floor_percent(&self) -> u32 {
        self.min_percent.min(defaults::MAX_BRIGHTNESS_FLOOR_PERCENT)
    }
}

/// A shell surface that can be limited to some outputs.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceKind {
    Bar,
    Osd,
    Notifications,
    Wallpaper,
    /// The app dock (the built-in `dock` plugin).
    Dock,
}

impl SurfaceKind {
    pub const ALL: [SurfaceKind; 5] = [SurfaceKind::Bar, SurfaceKind::Dock, SurfaceKind::Osd, SurfaceKind::Notifications, SurfaceKind::Wallpaper];

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "bar" => Some(Self::Bar),
            "osd" => Some(Self::Osd),
            "notifications" => Some(Self::Notifications),
            "wallpaper" => Some(Self::Wallpaper),
            "dock" => Some(Self::Dock),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Bar => "bar",
            Self::Osd => "osd",
            Self::Notifications => "notifications",
            Self::Wallpaper => "wallpaper",
            Self::Dock => "dock",
        }
    }
}

/// Which outputs show a surface.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScreenChoice {
    #[default]
    All,
    /// Only `displays.primary` (every output while it's unset).
    Primary,
    /// Only the listed outputs (every output while the list is empty).
    Selected,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenSet {
    pub show_on: ScreenChoice,
    /// Connector names (or `make model serial`) for [`ScreenChoice::Selected`].
    pub outputs: Vec<String>,
}

/// Per-surface output assignment.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SurfaceAssignment {
    pub bar: ScreenSet,
    pub osd: ScreenSet,
    pub notifications: ScreenSet,
    pub wallpaper: ScreenSet,
    pub dock: ScreenSet,
}

impl SurfaceAssignment {
    pub fn get(&self, kind: SurfaceKind) -> &ScreenSet {
        match kind {
            SurfaceKind::Bar => &self.bar,
            SurfaceKind::Osd => &self.osd,
            SurfaceKind::Notifications => &self.notifications,
            SurfaceKind::Wallpaper => &self.wallpaper,
            SurfaceKind::Dock => &self.dock,
        }
    }

    pub fn get_mut(&mut self, kind: SurfaceKind) -> &mut ScreenSet {
        match kind {
            SurfaceKind::Bar => &mut self.bar,
            SurfaceKind::Osd => &mut self.osd,
            SurfaceKind::Notifications => &mut self.notifications,
            SurfaceKind::Wallpaper => &mut self.wallpaper,
            SurfaceKind::Dock => &mut self.dock,
        }
    }
}

/// `[displays]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplaysConfig {
    /// Seconds a tested change waits for "Keep" before reverting.
    pub revert_seconds: u32,
    /// Apply the matching saved profile when monitors are plugged in.
    pub auto_profiles: bool,
    pub profiles: Vec<DisplayProfile>,
    /// The main output (connector name) for "primary only" surfaces.
    pub primary: Option<String>,
    /// Which outputs show each shell surface.
    pub surfaces: SurfaceAssignment,
    pub brightness: ExternalBrightness,
}

impl DisplaysConfig {
    /// `revert_seconds` within the accepted range: `0` would revert a
    /// tested change before anyone could press Keep.
    pub fn revert_secs(&self) -> u32 {
        self.revert_seconds.clamp(defaults::MIN_REVERT_SECONDS, defaults::MAX_REVERT_SECONDS)
    }
}

impl Default for DisplaysConfig {
    fn default() -> Self {
        Self {
            revert_seconds: defaults::REVERT_SECONDS,
            auto_profiles: true,
            profiles: Vec::new(),
            primary: None,
            surfaces: SurfaceAssignment::default(),
            brightness: ExternalBrightness::default(),
        }
    }
}

impl DisplaysConfig {
    /// The profile whose monitors are exactly `connected`.
    pub fn matching_profile(&self, connected: &BTreeSet<String>) -> Option<&DisplayProfile> {
        self.profiles.iter().find(|p| p.matches(connected))
    }
}

fn same_output(entry: &str, output: &str) -> bool {
    entry.trim().eq_ignore_ascii_case(output.trim())
}

/// Whether surface `kind` belongs on `output` (a connector name or `make
/// model serial`). A choice that would leave the surface nowhere (no
/// primary, empty list) shows it everywhere.
pub fn assignment(cfg: &DisplaysConfig, kind: SurfaceKind, output: &str) -> bool {
    let set = cfg.surfaces.get(kind);
    match set.show_on {
        ScreenChoice::All => true,
        ScreenChoice::Primary => cfg.primary.as_deref().filter(|p| !p.trim().is_empty()).is_none_or(|p| same_output(p, output)),
        ScreenChoice::Selected => set.outputs.is_empty() || set.outputs.iter().any(|o| same_output(o, output)),
    }
}

/// [`assignment`] for an output known by both its connector name and its
/// `make model serial` identity.
pub fn assignment_any(cfg: &DisplaysConfig, kind: SurfaceKind, names: &[&str]) -> bool {
    names.iter().any(|n| assignment(cfg, kind, n))
}

// ── Compositor drop-ins ─────────────────────────────────────────────────────

/// Hyprland's VRR values (`0` off, `1` on, `2` fullscreen only).
fn hypr_vrr(v: VrrMode) -> u8 {
    match v {
        VrrMode::Off => 0,
        VrrMode::On => 1,
        VrrMode::Fullscreen => 2,
    }
}

fn hz(mhz: u32) -> String {
    format!("{:.3}", mhz as f64 / MHZ_PER_HZ)
}

/// Escapes regex metacharacters (MangoWM matches names as PCRE2).
fn regex_exact(s: &str) -> String {
    let mut out = String::from("^");
    for c in s.chars() {
        if "\\^$.|?*+()[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('$');
    out
}

/// MangoWM `monitor_rule=` lines (0.18 key:value syntax, checked with
/// `mango -p`).
pub fn mango_lines(outputs: &[OutputConfig]) -> String {
    let mut s = String::new();
    for o in outputs.iter().filter(|o| !o.name.trim().is_empty() && o.has_settings()) {
        let mut props = vec![format!("name:{}", regex_exact(o.name.trim()))];
        if !o.enabled {
            props.push("disable:1".into());
        } else {
            if let (Some(w), Some(h)) = (o.width, o.height) {
                props.push(format!("width:{w}"));
                props.push(format!("height:{h}"));
                if let Some(r) = o.refresh_mhz.filter(|r| *r > 0) {
                    props.push(format!("refresh:{}", hz(r)));
                }
            }
            if let (Some(x), Some(y)) = (o.x, o.y) {
                props.push(format!("x:{x}"));
                props.push(format!("y:{y}"));
            }
            if let Some(scale) = o.scale_clamped() {
                props.push(format!("scale:{scale:.2}"));
            }
            if let Some(t) = o.transform {
                props.push(format!("rr:{}", t.wl()));
            }
            if let Some(v) = o.vrr {
                props.push(format!("vrr:{}", u8::from(v != VrrMode::Off)));
            }
        }
        let _ = writeln!(s, "monitor_rule={}", props.join(","));
    }
    s
}

fn kdl_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Niri `output "NAME" { … }` blocks (KDL v1).
pub fn niri_blocks(outputs: &[OutputConfig]) -> String {
    let mut s = String::new();
    for o in outputs.iter().filter(|o| !o.name.trim().is_empty() && o.has_settings()) {
        let mut body = String::new();
        if !o.enabled {
            let _ = writeln!(body, "    off");
        } else {
            if let Some(mode) = o.mode_string() {
                let _ = writeln!(body, "    mode {}", kdl_str(&mode));
            }
            if let Some(scale) = o.scale_clamped() {
                let _ = writeln!(body, "    scale {scale:.2}");
            }
            if let Some(t) = o.transform {
                let _ = writeln!(body, "    transform {}", kdl_str(t.name()));
            }
            if let (Some(x), Some(y)) = (o.x, o.y) {
                let _ = writeln!(body, "    position x={x} y={y}");
            }
            match o.vrr {
                Some(VrrMode::On) => {
                    let _ = writeln!(body, "    variable-refresh-rate");
                }
                Some(VrrMode::Fullscreen) => {
                    let _ = writeln!(body, "    variable-refresh-rate on-demand=true");
                }
                _ => {}
            }
        }
        let _ = write!(s, "\noutput {} {{\n{body}}}\n", kdl_str(o.name.trim()));
    }
    s
}

/// Hyprland (hyprlang) `monitor =` lines.
pub fn hyprland_conf_lines(outputs: &[OutputConfig]) -> String {
    let mut s = String::new();
    for o in outputs.iter().filter(|o| !o.name.trim().is_empty() && o.has_settings()) {
        let name = o.name.trim();
        if !o.enabled {
            let _ = writeln!(s, "monitor = {name}, disable");
            continue;
        }
        let mode = o.mode_string().unwrap_or_else(|| "preferred".into());
        let pos = match (o.x, o.y) {
            (Some(x), Some(y)) => format!("{x}x{y}"),
            _ => "auto".into(),
        };
        let scale = o.scale_clamped().map(|v| format!("{v:.2}")).unwrap_or_else(|| "auto".into());
        let mut line = format!("monitor = {name}, {mode}, {pos}, {scale}");
        if let Some(t) = o.transform {
            let _ = write!(line, ", transform, {}", t.wl());
        }
        if let Some(v) = o.vrr {
            let _ = write!(line, ", vrr, {}", hypr_vrr(v));
        }
        let _ = writeln!(s, "{line}");
    }
    s
}

fn lua_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Hyprland 0.55+ Lua monitor calls, wrapped in `pcall` so an API change
/// never breaks the user's config.
pub fn hyprland_lua_lines(outputs: &[OutputConfig]) -> String {
    let mut s = String::new();
    for o in outputs.iter().filter(|o| !o.name.trim().is_empty() && o.has_settings()) {
        let mut fields = vec![format!("output = {}", lua_str(o.name.trim()))];
        if !o.enabled {
            fields.push("disabled = true".into());
        } else {
            fields.push(format!("mode = {}", lua_str(&o.mode_string().unwrap_or_else(|| "preferred".into()))));
            fields.push(format!(
                "position = {}",
                lua_str(&match (o.x, o.y) {
                    (Some(x), Some(y)) => format!("{x}x{y}"),
                    _ => "auto".into(),
                })
            ));
            if let Some(scale) = o.scale_clamped() {
                fields.push(format!("scale = {scale:.2}"));
            }
            if let Some(t) = o.transform {
                fields.push(format!("transform = {}", t.wl()));
            }
            if let Some(v) = o.vrr {
                fields.push(format!("vrr = {}", hypr_vrr(v)));
            }
        }
        let _ = writeln!(s, "pcall(hl.monitor, {{ {} }})", fields.join(", "));
    }
    s
}

/// The monitor lines for `target`'s drop-in, with a heading comment;
/// empty when nothing is persisted.
pub fn emit_outputs(target: Target, outputs: &[OutputConfig], hyprland_lua: bool) -> String {
    let (comment, body) = match target {
        Target::Mango => ("#", mango_lines(outputs)),
        Target::Niri => ("//", niri_blocks(outputs)),
        Target::Hyprland if hyprland_lua => ("--", hyprland_lua_lines(outputs)),
        Target::Hyprland => ("#", hyprland_conf_lines(outputs)),
    };
    if body.trim().is_empty() {
        return String::new();
    }
    format!("\n{comment} Displays (Studio → Displays)\n{body}")
}

/// Output names that the user's own compositor config (and the files it
/// includes, never Mandara's drop-in) already configures. Studio warns
/// about these: the compositor may prefer the user's line over the drop-in.
pub fn user_output_rules(target: Target) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut stack = vec![target.user_config()];
    let mut visited: BTreeSet<PathBuf> = BTreeSet::new();
    while let Some(file) = stack.pop() {
        if !visited.insert(file.clone()) || is_generated(&file) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        out.extend(parse_output_rules(target, &text));
        for line in text.lines().filter(|l| !l.trim_start().starts_with(['#', '/']) && !l.trim_start().starts_with("--")) {
            if let Some(path) = include_path(target, line) {
                stack.push(resolve_include(&file, &path));
            }
        }
    }
    out
}

/// Output names configured in one config file's text.
pub fn parse_output_rules(target: Target, text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#') && !l.starts_with("//")) {
        let name = match target {
            Target::Mango => line
                .strip_prefix("monitor_rule=")
                .or_else(|| line.strip_prefix("monitorrule="))
                .and_then(|rest| rest.split(',').find_map(|p| p.trim().strip_prefix("name:")).or_else(|| rest.split(',').next()))
                .map(|n| n.trim_start_matches('^').trim_end_matches('$').replace('\\', "")),
            Target::Hyprland => line.strip_prefix("monitor").and_then(|r| r.trim_start().strip_prefix('=')).and_then(|r| r.split(',').next()).map(|n| n.trim().to_string()),
            Target::Niri => line.strip_prefix("output ").and_then(|r| r.trim().strip_prefix('"')).and_then(|r| r.split('"').next()).map(str::to_string),
        };
        if let Some(n) = name.filter(|n| !n.is_empty()) {
            out.push(n);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dell() -> OutputConfig {
        OutputConfig {
            name: "DP-1".into(),
            make: "Dell Inc.".into(),
            model: "U2720Q".into(),
            serial: "ABC123".into(),
            width: Some(3840),
            height: Some(2160),
            refresh_mhz: Some(59_997),
            scale: Some(1.5),
            transform: Some(Transform::Rotate90),
            x: Some(2560),
            y: Some(0),
            vrr: Some(VrrMode::On),
            ..Default::default()
        }
    }

    fn laptop_off() -> OutputConfig {
        OutputConfig { name: "eDP-1".into(), enabled: false, ..Default::default() }
    }

    #[test]
    fn transforms_round_trip() {
        for t in Transform::ALL {
            assert_eq!(Transform::from_wl(t.wl()), t);
            assert_eq!(Transform::parse(t.name()), Some(t));
        }
        assert!(Transform::Rotate270.swaps_axes() && !Transform::Flipped180.swaps_axes());
        let toml = toml::to_string(&OutputConfig { transform: Some(Transform::Flipped90), ..Default::default() }).unwrap();
        assert!(toml.contains("transform = \"flipped-90\""), "{toml}");
    }

    #[test]
    fn identity_prefers_edid_serial() {
        assert_eq!(dell().identity(), "Dell Inc. U2720Q ABC123");
        assert_eq!(OutputConfig { name: "HDMI-A-1".into(), make: "Acme".into(), ..Default::default() }.identity(), "HDMI-A-1");
        assert_eq!(dell().mode_string().as_deref(), Some("3840x2160@59.997"));
    }

    #[test]
    fn profiles_match_exact_monitor_sets() {
        let p = DisplayProfile { name: "Desk".into(), outputs: vec![dell(), laptop_off()] };
        let both: BTreeSet<String> = ["Dell Inc. U2720Q ABC123".to_string(), "eDP-1".to_string()].into();
        assert!(p.matches(&both));
        let laptop: BTreeSet<String> = ["eDP-1".to_string()].into();
        assert!(!p.matches(&laptop));
        let cfg = DisplaysConfig { profiles: vec![p], ..Default::default() };
        assert_eq!(cfg.matching_profile(&both).map(|p| p.name.as_str()), Some("Desk"));
        assert!(cfg.matching_profile(&laptop).is_none());
    }

    #[test]
    fn surface_assignment() {
        let mut cfg = DisplaysConfig::default();
        assert!(assignment(&cfg, SurfaceKind::Bar, "HDMI-A-1"));
        cfg.surfaces.bar.show_on = ScreenChoice::Primary;
        assert!(assignment(&cfg, SurfaceKind::Bar, "HDMI-A-1"), "no primary yet: everywhere");
        cfg.primary = Some("eDP-1".into());
        assert!(!assignment(&cfg, SurfaceKind::Bar, "HDMI-A-1"));
        assert!(assignment(&cfg, SurfaceKind::Bar, "edp-1"));
        cfg.surfaces.osd.show_on = ScreenChoice::Selected;
        assert!(assignment(&cfg, SurfaceKind::Osd, "DP-2"), "empty list: everywhere");
        cfg.surfaces.osd.outputs = vec!["DP-2".into()];
        assert!(assignment(&cfg, SurfaceKind::Osd, "DP-2") && !assignment(&cfg, SurfaceKind::Osd, "eDP-1"));
        assert!(assignment_any(&cfg, SurfaceKind::Bar, &["HDMI-A-1", "eDP-1"]));
        assert!(assignment(&cfg, SurfaceKind::Wallpaper, "anything"));
        for kind in SurfaceKind::ALL {
            assert_eq!(SurfaceKind::parse(kind.id()), Some(kind));
        }
    }

    #[test]
    fn mango_monitor_rules() {
        let text = mango_lines(&[dell(), laptop_off(), OutputConfig { name: "HDMI-A-2".into(), ..Default::default() }]);
        assert!(text.contains("monitor_rule=name:^DP-1$,width:3840,height:2160,refresh:59.997,x:2560,y:0,scale:1.50,rr:1,vrr:1"), "{text}");
        assert!(text.contains("monitor_rule=name:^eDP-1$,disable:1"));
        assert!(!text.contains("HDMI-A-2"), "nothing set, nothing written");
    }

    /// MangoWM's own checker accepts the monitor rules.
    #[test]
    fn mango_accepts_monitor_rules() {
        if std::process::Command::new("mango").arg("-v").output().is_err() {
            return;
        }
        let path = std::env::temp_dir().join(format!("mandara-mango-monitors-{}.conf", std::process::id()));
        std::fs::write(&path, mango_lines(&[dell(), laptop_off()])).unwrap();
        let out = std::process::Command::new("mango").arg("-p").arg("-c").arg(&path).output().unwrap();
        let report = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        std::fs::remove_file(&path).ok();
        assert!(!report.contains("ERROR"), "mango -p rejected the monitor rules:\n{report}");
    }

    #[test]
    fn niri_outputs_are_valid_kdl() {
        let fs = OutputConfig { name: "HDMI-A-1".into(), vrr: Some(VrrMode::Fullscreen), ..Default::default() };
        let text = niri_blocks(&[dell(), laptop_off(), fs]);
        kdl::KdlDocument::parse_v1(&text).unwrap_or_else(|e| panic!("invalid KDL v1: {e:?}\n{text}"));
        for needle in ["output \"DP-1\"", "mode \"3840x2160@59.997\"", "scale 1.50", "transform \"90\"", "position x=2560 y=0", "variable-refresh-rate\n", "output \"eDP-1\" {\n    off\n}", "on-demand=true"] {
            assert!(text.contains(needle), "missing {needle:?}\n{text}");
        }
    }

    #[test]
    fn hyprland_monitor_lines() {
        let text = hyprland_conf_lines(&[dell(), laptop_off(), OutputConfig { name: "HDMI-A-1".into(), scale: Some(2.0), ..Default::default() }]);
        assert!(text.contains("monitor = DP-1, 3840x2160@59.997, 2560x0, 1.50, transform, 1, vrr, 1"), "{text}");
        assert!(text.contains("monitor = eDP-1, disable"));
        assert!(text.contains("monitor = HDMI-A-1, preferred, auto, 2.00"));
        let lua = hyprland_lua_lines(&[dell()]);
        assert!(lua.starts_with("pcall(hl.monitor, { output = \"DP-1\", mode = \"3840x2160@59.997\""), "{lua}");
        assert_eq!(emit_outputs(Target::Mango, &[], false), "");
        assert!(emit_outputs(Target::Niri, &[dell()], false).starts_with("\n// Displays"));
    }

    #[test]
    fn drop_ins_carry_saved_outputs() {
        use crate::compositor::{CompositorConfig, ManagedConfig, OutputRule, emit_hyprland_conf, emit_mango, emit_niri};
        let mut c = CompositorConfig { outputs: vec![dell()], ..Default::default() };
        c.niri.outputs = vec![OutputRule { name: "DP-1".into(), scale: Some(2.0), ..Default::default() }, OutputRule { name: "HDMI-A-9".into(), scale: Some(2.0), ..Default::default() }];
        let m = ManagedConfig { config: &c, borders: None, existing_chords: Default::default(), mandara_bin: "mandara".into(), version: None, user_autostarts: false };
        assert!(emit_mango(&m).text.contains("monitor_rule=name:^DP-1$"));
        assert!(emit_hyprland_conf(&m).text.contains("monitor = DP-1, 3840x2160@59.997"));
        let niri = emit_niri(&m).text;
        kdl::KdlDocument::parse_v1(&niri).unwrap_or_else(|e| panic!("invalid KDL v1: {e:?}\n{niri}"));
        assert_eq!(niri.matches("output \"DP-1\"").count(), 1, "Displays supersedes the Niri-only entry:\n{niri}");
        assert!(niri.contains("output \"HDMI-A-9\""));
    }

    #[test]
    fn parses_users_own_output_rules() {
        let mango = "# x\nmonitor_rule=name:^HDMI-A-2$,width:3840,height:2160\nmonitorrule=eDP-1,0.55,1,tile\n";
        assert_eq!(parse_output_rules(Target::Mango, mango), vec!["HDMI-A-2", "eDP-1"]);
        assert_eq!(parse_output_rules(Target::Hyprland, "monitor = DP-1, preferred, auto, 1\nmonitor=,preferred,auto,1\n"), vec!["DP-1"]);
        assert_eq!(parse_output_rules(Target::Niri, "output \"eDP-1\" {\n    scale 2\n}\n"), vec!["eDP-1"]);
    }

    #[test]
    fn display_limits_are_clamped_on_read() {
        let mut d = DisplaysConfig::default();
        assert_eq!(d.revert_secs(), defaults::REVERT_SECONDS);
        d.revert_seconds = 0;
        assert_eq!(d.revert_secs(), defaults::MIN_REVERT_SECONDS, "0 would revert before Keep");
        d.revert_seconds = 3600;
        assert_eq!(d.revert_secs(), defaults::MAX_REVERT_SECONDS);
        d.brightness.min_percent = 150;
        assert_eq!(d.brightness.floor_percent(), defaults::MAX_BRIGHTNESS_FLOOR_PERCENT);
        d.brightness.min_percent = 7;
        assert_eq!(d.brightness.floor_percent(), 7);
        let loaded: DisplaysConfig = toml::from_str("revert_seconds = 0\n[brightness]\nmin_percent = 400\n").unwrap();
        assert!(loaded.revert_secs() >= defaults::MIN_REVERT_SECONDS && loaded.brightness.floor_percent() <= defaults::MAX_BRIGHTNESS_FLOOR_PERCENT);
    }

    #[test]
    fn night_light_defaults_and_exclusions() {
        let mut n = NightLightConfig::default();
        assert!(!n.enabled && n.night_temperature < n.day_temperature);
        n.excluded_apps.push("org.gimp.GIMP".into());
        assert!(n.excludes("org.gimp.gimp") && !n.excludes("") && !n.excludes("firefox"));
        let cfg: NightLightConfig = toml::from_str("enabled = true\nschedule = \"fixed\"\n").unwrap();
        assert_eq!(cfg.schedule, NightSchedule::Fixed);
        assert_eq!(cfg.night_temperature, defaults::NIGHT_TEMPERATURE_K);
    }
}
