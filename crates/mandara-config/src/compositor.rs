//! Compositor-managed configuration and its non-destructive emitters.
//!
//! Mandara never rewrites the user's own compositor config. Rules, keybinds
//! and live border colors go to `~/.config/mandara/generated/{mango.conf,
//! niri.kdl, hyprland.conf}`, and the user's main file only ever gains one
//! include line (installed on request, verified at startup):
//!
//! * MangoWM — `source_optional=~/.config/mandara/generated/mango.conf`
//! * Niri — `include optional=true "<abs path>/niri.kdl"` (26.04+; plain
//!   `include` on 25.11, the first version with includes)
//! * Hyprland — `source = ~/.config/mandara/generated/hyprland.conf`, or on
//!   Lua Hyprland (0.57+ only reads `hyprland.lua`)
//!   `pcall(dofile, "<abs path>/hyprland.lua")`
//!
//! Keybinds that collide with binds already in the user's config are
//! skipped and reported instead of silently overriding them.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    Mango,
    Niri,
    Hyprland,
}

impl Target {
    pub const ALL: [Target; 3] = [Target::Mango, Target::Niri, Target::Hyprland];

    /// Whether Hyprland reads the Lua config: always from 0.57 (hyprlang
    /// support was removed), and on 0.55–0.56 when `hyprland.lua` exists
    /// (`hyprland.conf` is then ignored).
    pub fn hyprland_lua() -> bool {
        installed_version(Target::Hyprland).is_some_and(hyprland_reads_lua) || hyprland_lua_config_exists()
    }

    pub fn generated_file(self) -> PathBuf {
        paths::generated_dir().join(match self {
            Target::Mango => "mango.conf",
            Target::Niri => "niri.kdl",
            Target::Hyprland if Self::hyprland_lua() => "hyprland.lua",
            Target::Hyprland => "hyprland.conf",
        })
    }

    /// The user's primary config file.
    pub fn user_config(self) -> PathBuf {
        let config = dirs::config_dir().unwrap_or_else(|| PathBuf::from("~/.config"));
        match self {
            Target::Mango => config.join("mango/config.conf"),
            Target::Niri => config.join("niri/config.kdl"),
            Target::Hyprland if Self::hyprland_lua() => config.join("hypr/hyprland.lua"),
            Target::Hyprland => config.join("hypr/hyprland.conf"),
        }
    }

    /// The single line Mandara adds to the user's config.
    pub fn include_line(self) -> String {
        match self {
            Target::Mango => "source_optional=~/.config/mandara/generated/mango.conf".into(),
            // An absolute path works from niri 25.11 (`~/` needs 26.04).
            Target::Niri => niri_include_line(&self.generated_file().display().to_string(), installed_version(Target::Niri)),
            Target::Hyprland if Self::hyprland_lua() => hyprland_lua_include(&self.generated_file().display().to_string()),
            Target::Hyprland => "source = ~/.config/mandara/generated/hyprland.conf".into(),
        }
    }
}

/// Niri 26.04 added `include optional=true` (a missing drop-in then only
/// logs a warning instead of failing the whole config).
pub const NIRI_OPTIONAL_INCLUDE: (u32, u32) = (26, 4);
/// Hyprland 0.57 removed hyprlang: only `hyprland.lua` is read.
pub const HYPRLAND_LUA_ONLY: (u32, u32) = (0, 57);

/// Whether `~/.config/hypr/hyprland.lua` exists.
pub fn hyprland_lua_config_exists() -> bool {
    dirs::config_dir().is_some_and(|c| c.join("hypr/hyprland.lua").exists())
}

/// Whether this Hyprland version reads only the Lua config.
pub fn hyprland_reads_lua(version: (u32, u32)) -> bool {
    version >= HYPRLAND_LUA_ONLY
}

/// Niri's include line for `path`. Niri includes are positional (later
/// settings win), so the line goes at the end of the user's config.
pub fn niri_include_line(path: &str, version: Option<(u32, u32)>) -> String {
    if version.is_some_and(|v| v >= NIRI_OPTIONAL_INCLUDE) { format!("include optional=true {}", kdl_str(path)) } else { format!("include {}", kdl_str(path)) }
}

/// Hyprland Lua include. `dofile` exists in every Lua Hyprland; `pcall`
/// keeps a missing drop-in from stopping the rest of the user's config.
pub fn hyprland_lua_include(path: &str) -> String {
    format!("pcall(dofile, {})", lua_str(path))
}

/// `(major, minor)` in a version banner: `niri 26.04 (8ed0da4)`,
/// `Hyprland 0.57.0 built from branch …`, `v0.57.0`.
pub fn parse_version(text: &str) -> Option<(u32, u32)> {
    text.split_whitespace().find_map(|w| {
        let mut parts = w.trim_start_matches('v').split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.trim_end_matches(|c: char| !c.is_ascii_digit()).parse().ok()?;
        Some((major, minor))
    })
}

/// Version of the compositor installed on `PATH` (`niri --version`,
/// `Hyprland --version`), probed once per process. Lets `mandara setup`
/// pick the right dialect even when it runs outside that compositor.
pub fn installed_version(target: Target) -> Option<(u32, u32)> {
    static CACHE: [std::sync::OnceLock<Option<(u32, u32)>>; 3] = [const { std::sync::OnceLock::new() }; 3];
    let (slot, program) = match target {
        Target::Mango => return None,
        Target::Niri => (&CACHE[1], "niri"),
        Target::Hyprland => (&CACHE[2], "Hyprland"),
    };
    *slot.get_or_init(|| {
        let out = std::process::Command::new(program).arg("--version").stdin(std::process::Stdio::null()).output().ok()?;
        parse_version(&String::from_utf8_lossy(&out.stdout))
    })
}

/// Per-window rule (fields map to each compositor's equivalent; options a
/// compositor lacks are omitted for it).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowRule {
    /// Regex on the app id / class.
    pub app_id: Option<String>,
    /// Regex on the title.
    pub title: Option<String>,
    pub floating: Option<bool>,
    /// MangoWM: visible on every tag.
    pub global: Option<bool>,
    /// MangoWM: always above.
    pub overlay: Option<bool>,
    pub unfocused_opacity: Option<f32>,
    /// Opacity while focused (MangoWM `focused_opacity`, Hyprland's first
    /// `opacity` value, Niri `is-active=true`).
    pub focused_opacity: Option<f32>,
    pub no_blur: Option<bool>,
    pub no_shadow: Option<bool>,
    pub confine_pointer: Option<bool>,
    pub force_tiled_state: Option<bool>,
}

/// MangoWM per-tag defaults.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TagRule {
    pub id: u32,
    pub monitor: Option<String>,
    /// `tile`, `scroller`, `grid`, `monocle`, `deck`, …
    pub layout: Option<String>,
    pub scroller_default_proportion: Option<f32>,
    /// Master area share for tiling layouts (`master_factor`, `0.05..=0.95`).
    pub master_factor: Option<f32>,
    /// Windows in the master area (`master_count`).
    pub master_count: Option<u32>,
}

/// Window-rule options and the compositors whose drop-in honours them
/// (`[MangoWM, Niri, Hyprland]`). Studio greys out the rest.
pub const RULE_OPTIONS: &[(&str, [bool; 3])] = &[
    ("floating", [true, true, true]),
    ("global", [true, false, true]),
    ("overlay", [true, false, false]),
    ("focused_opacity", [true, true, true]),
    ("unfocused_opacity", [true, true, true]),
    ("no_blur", [true, false, true]),
    ("no_shadow", [true, true, true]),
    ("confine_pointer", [true, false, false]),
    ("force_tiled_state", [true, false, false]),
];

/// Whether `target`'s emitter writes window-rule option `option`.
pub fn rule_option_supported(target: Target, option: &str) -> bool {
    let i = match target {
        Target::Mango => 0,
        Target::Niri => 1,
        Target::Hyprland => 2,
    };
    RULE_OPTIONS.iter().any(|(name, s)| *name == option && s[i])
}

/// MangoWM SceneFX fine-tuning (`None` keeps the compositor's value).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MangoFx {
    /// `blur_params_radius`
    pub blur_radius: Option<u32>,
    /// `blur_params_num_passes`
    pub blur_passes: Option<u32>,
    /// `shadows_size`
    pub shadows_size: Option<u32>,
    /// `shadows_blur`
    pub shadows_blur: Option<u32>,
}

/// One Niri output (`output "NAME" { scale …; position x=… y=…; }`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputRule {
    /// Connector name (`HDMI-A-1`, `eDP-1`).
    pub name: String,
    pub scale: Option<f32>,
    /// Logical position; both must be set to be written.
    pub x: Option<i32>,
    pub y: Option<i32>,
}

/// Niri scrollable-tiling options.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NiriOptions {
    /// New columns' width as a proportion of the output (`0.1..=1`).
    pub default_column_width: Option<f32>,
    /// Widths `switch-preset-column-width` cycles through (proportions).
    pub preset_column_widths: Vec<f32>,
    /// `never`, `always` or `on-overflow`.
    pub center_focused_column: Option<String>,
    /// Center a lone column on its output.
    pub always_center_single_column: Option<bool>,
    pub outputs: Vec<OutputRule>,
}

/// Hyprland decoration extras.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HyprlandOptions {
    /// `decoration:blur:size`
    pub blur_size: Option<u32>,
    /// `decoration:blur:passes`
    pub blur_passes: Option<u32>,
    /// `decoration:shadow:range`
    pub shadow_range: Option<u32>,
    /// `decoration:dim_inactive`
    pub dim_inactive: Option<bool>,
    /// `decoration:dim_strength`, `0..=1`
    pub dim_strength: Option<f32>,
    /// `general:layout`: `dwindle` or `master`.
    pub layout: Option<String>,
    /// `dwindle:preserve_split`: a split keeps its direction when resized.
    pub dwindle_preserve_split: Option<bool>,
    /// `dwindle:smart_split`: split toward the pointer's side of the window.
    pub dwindle_smart_split: Option<bool>,
    /// `dwindle:force_split`: 0 follows the pointer, 1 left / top, 2 right / bottom.
    pub dwindle_force_split: Option<u32>,
    /// `master:orientation`: `left`, `right`, `top`, `bottom` or `center`.
    pub master_orientation: Option<String>,
    /// `master:mfact`, `0..=1`.
    pub master_mfact: Option<f32>,
    /// `master:new_on_top`: new windows enter the master area.
    pub master_new_on_top: Option<bool>,
    /// `general:resize_on_border`: drag window borders to resize.
    pub resize_on_border: Option<bool>,
    /// `general:extend_border_grab_area`, px.
    pub border_grab_area: Option<u32>,
    /// `group:groupbar:enabled`.
    pub groupbar: Option<bool>,
    /// `group:groupbar:render_titles`.
    pub groupbar_titles: Option<bool>,
    /// `group:groupbar:gradients`.
    pub groupbar_gradients: Option<bool>,
    /// `group:groupbar:height`, px.
    pub groupbar_height: Option<u32>,
}

/// Hyprland `master:orientation` values.
pub const HYPR_MASTER_ORIENTATIONS: [&str; 5] = ["left", "right", "top", "bottom", "center"];
/// Hyprland `dwindle:force_split` upper bound (2 = right / bottom).
const HYPR_FORCE_SPLIT_MAX: u32 = 2;

impl HyprlandOptions {
    /// `(section, key, value)` of the layout, border and group options.
    fn layout_entries(&self) -> Vec<(&'static str, &'static str, String)> {
        let mut out = Vec::new();
        if let Some(l) = self.layout.as_deref().filter(|l| matches!(*l, "dwindle" | "master")) {
            out.push(("general", "layout", l.to_string()));
        }
        if let Some(v) = self.resize_on_border {
            out.push(("general", "resize_on_border", v.to_string()));
        }
        if let Some(v) = self.border_grab_area {
            out.push(("general", "extend_border_grab_area", v.to_string()));
        }
        if let Some(v) = self.dwindle_preserve_split {
            out.push(("dwindle", "preserve_split", v.to_string()));
        }
        if let Some(v) = self.dwindle_smart_split {
            out.push(("dwindle", "smart_split", v.to_string()));
        }
        if let Some(v) = self.dwindle_force_split {
            out.push(("dwindle", "force_split", v.min(HYPR_FORCE_SPLIT_MAX).to_string()));
        }
        if let Some(o) = self.master_orientation.as_deref().filter(|o| HYPR_MASTER_ORIENTATIONS.contains(o)) {
            out.push(("master", "orientation", o.to_string()));
        }
        if let Some(v) = self.master_mfact {
            out.push(("master", "mfact", format!("{:.2}", v.clamp(0.0, 1.0))));
        }
        if let Some(v) = self.master_new_on_top {
            out.push(("master", "new_on_top", v.to_string()));
        }
        if let Some(v) = self.groupbar {
            out.push(("groupbar", "enabled", v.to_string()));
        }
        if let Some(v) = self.groupbar_titles {
            out.push(("groupbar", "render_titles", v.to_string()));
        }
        if let Some(v) = self.groupbar_gradients {
            out.push(("groupbar", "gradients", v.to_string()));
        }
        if let Some(v) = self.groupbar_height {
            out.push(("groupbar", "height", v.to_string()));
        }
        out
    }
}

/// What a keybind does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum BindAction {
    /// A Mandara command, run as `mandara msg <words>`.
    Mandara(String),
    /// Spawn a shell command.
    Spawn(String),
}

impl BindAction {
    /// Shell command for this action; `bin` is the Mandara executable.
    pub fn shell_command_with(&self, bin: &str) -> String {
        match self {
            BindAction::Mandara(words) => format!("{bin} msg {words}"),
            BindAction::Spawn(cmd) => cmd.clone(),
        }
    }

    pub fn shell_command(&self) -> String {
        self.shell_command_with("mandara")
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keybind {
    /// Modifiers: `super`, `ctrl`, `alt`, `shift`.
    pub mods: Vec<String>,
    /// Key name (`space`, `n`, `Print`, `Delete`, …).
    pub key: String,
    pub action: BindAction,
}

impl Keybind {
    pub fn new(mods: &[&str], key: &str, action: BindAction) -> Self {
        Self { mods: mods.iter().map(|m| m.to_string()).collect(), key: key.into(), action }
    }

    /// Normalized identity used for conflict detection: sorted lowercase
    /// modifiers + lowercase key.
    pub fn chord(&self) -> String {
        chord(self.mods.iter().map(String::as_str), &self.keysym())
    }

    pub fn display(&self) -> String {
        let mut parts: Vec<String> = self.mods.iter().map(|m| capitalize(m)).collect();
        parts.push(self.key.clone());
        parts.join("+")
    }

    /// Why this bind can't be written, if it can't: no key yet (a row
    /// added in Studio whose capture was cancelled), or a typing key with
    /// no Super / Ctrl / Alt.
    pub fn problem(&self) -> Option<&'static str> {
        if self.key.trim().is_empty() {
            return Some("no key yet");
        }
        let has_command_mod = self.mods.iter().filter_map(|m| normalize_mod(m)).any(|m| m != "shift");
        (!has_command_mod && !bare_key_allowed(&self.key)).then_some("needs Super, Ctrl or Alt")
    }

    /// The key as a keysym name (punctuation spelled out).
    pub fn keysym(&self) -> String {
        keysym_for(self.key.trim())
    }

    /// Modifiers in `target`'s spelling, or `None` when one of them has no
    /// spelling there (MangoWM has no Mod5).
    fn target_mods(&self, target: Target) -> Option<Vec<&'static str>> {
        let set: BTreeSet<&str> = self.mods.iter().filter_map(|m| normalize_mod(m)).collect();
        // Fixed order (Super, Ctrl, Alt, Shift, …) whatever the config says.
        MOD_NAMES
            .iter()
            .filter(|(n, ..)| set.contains(n))
            .map(|&(_, mango, niri, hypr)| {
                let name = match target {
                    Target::Mango => mango,
                    Target::Niri => niri,
                    Target::Hyprland => hypr,
                };
                (!name.is_empty()).then_some(name)
            })
            .collect()
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn normalize_mod(m: &str) -> Option<&'static str> {
    match m.trim().to_ascii_lowercase().as_str() {
        "super" | "mod4" | "logo" | "mod" | "win" => Some("super"),
        "ctrl" | "control" => Some("ctrl"),
        "alt" | "mod1" => Some("alt"),
        "shift" => Some("shift"),
        // Rarer modifiers keep their identity, so `Mod5+x` never reads as
        // a bare `x` in conflict detection.
        "hyper" | "mod3" => Some("hyper"),
        "mod5" | "altgr" | "iso_level3_shift" => Some("mod5"),
        _ => None,
    }
}

/// The modifiers a Studio-captured chord may carry, as each compositor
/// spells them: `(normalized, MangoWM, Niri, Hyprland)`.
const MOD_NAMES: &[(&str, &str, &str, &str)] = &[("super", "SUPER", "Mod", "SUPER"), ("ctrl", "CTRL", "Ctrl", "CTRL"), ("alt", "ALT", "Alt", "ALT"), ("shift", "SHIFT", "Shift", "SHIFT"), ("hyper", "HYPER", "Mod3", "MOD3"), ("mod5", "", "ISO_Level3_Shift", "MOD5")];

/// A key typed as a character (`,`) or a GPUI key name (`pageup`) → its
/// xkb keysym name (`comma`, `Prior`), the spelling every compositor's
/// bind syntax accepts. Punctuation in particular can't stay a character:
/// `,` splits MangoWM / Hyprland bind lines and isn't a KDL identifier.
pub fn keysym_for(key: &str) -> String {
    let named = match key {
        "," => "comma",
        "." => "period",
        "/" => "slash",
        "\\" => "backslash",
        "-" => "minus",
        "=" => "equal",
        "+" => "plus",
        "[" => "bracketleft",
        "]" => "bracketright",
        ";" => "semicolon",
        "'" => "apostrophe",
        "`" => "grave",
        "<" => "less",
        ">" => "greater",
        "enter" => "Return",
        "escape" => "Escape",
        "backspace" => "BackSpace",
        "delete" => "Delete",
        "insert" => "Insert",
        "tab" => "Tab",
        "space" | " " => "space",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "home" => "Home",
        "end" => "End",
        "pageup" => "Prior",
        "pagedown" => "Next",
        "printscreen" | "print" => "Print",
        "pause" => "Pause",
        "menu" => "Menu",
        k if is_function_key(k) => return k.to_uppercase(),
        k if k.len() > 4 && k[..4].eq_ignore_ascii_case("xf86") => return k.to_string(),
        k if k.chars().count() == 1 => return k.to_lowercase(),
        k => return k.to_string(),
    };
    named.to_string()
}

fn is_function_key(k: &str) -> bool {
    k.len() > 1 && k[..1].eq_ignore_ascii_case("f") && k[1..].chars().all(|c| c.is_ascii_digit())
}

/// Keys that may be bound without Super / Ctrl / Alt: function keys,
/// Print, Pause and the XF86 media / hardware keys. Anything else bound
/// bare (or with only Shift) would stop that key typing in every app.
pub fn bare_key_allowed(key: &str) -> bool {
    let k = key.trim();
    is_function_key(k) || k.eq_ignore_ascii_case("print") || k.eq_ignore_ascii_case("pause") || (k.len() > 4 && k[..4].eq_ignore_ascii_case("xf86"))
}

pub fn chord<'a>(mods: impl Iterator<Item = &'a str>, key: &str) -> String {
    let set: BTreeSet<&str> = mods.filter_map(normalize_mod).collect();
    let mut s = set.into_iter().collect::<Vec<_>>().join("+");
    if !s.is_empty() {
        s.push('+');
    }
    s.push_str(&key.trim().to_ascii_lowercase());
    s
}

/// The `[compositor]` config section.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompositorConfig {
    /// Write generated drop-in files.
    pub manage: bool,
    pub window_rules: Vec<WindowRule>,
    pub tag_rules: Vec<TagRule>,
    pub keybinds: Vec<Keybind>,
    /// Window border width; `None` leaves the compositor's own setting.
    pub border_width: Option<u32>,
    /// Window look (gaps, rounding, blur, shadows…). Every field is
    /// optional: `None` leaves the user's own compositor setting alone.
    pub look: CompositorLook,
    /// MangoWM SceneFX tuning.
    pub mango_blur: bool,
    pub mango_blur_optimized: bool,
    pub mango_sync_obj: bool,
    /// MangoWM blur / shadow parameters.
    pub mango_fx: MangoFx,
    /// Niri-only layout and output options.
    pub niri: NiriOptions,
    /// Hyprland-only decoration options.
    pub hyprland: HyprlandOptions,
    /// Start Mandara with the compositor from the drop-in (skipped when
    /// your own config already starts it).
    pub autostart: bool,
    /// Saved display settings (Studio → Displays) written as monitor
    /// lines for every compositor (`crate::outputs`).
    pub outputs: Vec<crate::outputs::OutputConfig>,
    /// Keyboard, mouse, touchpad and cursor (Studio → Input).
    pub input: crate::input::InputConfig,
    /// Turn the compositor's own hot corner off (MangoWM `enable_hotarea`,
    /// Niri `gestures { hot-corners { off; } }`). Set automatically while
    /// Mandara's hot corners are active (see
    /// [`crate::ShellConfig::effective_compositor`]).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub disable_builtin_hot_corners: bool,
}

impl Default for CompositorConfig {
    fn default() -> Self {
        Self {
            manage: true,
            window_rules: vec![
                WindowRule { app_id: Some("^org\\.mandara\\.studio$".into()), floating: Some(true), ..Default::default() },
            ],
            tag_rules: Vec::new(),
            keybinds: default_keybinds(),
            border_width: None,
            look: CompositorLook::default(),
            mango_blur: true,
            mango_blur_optimized: true,
            mango_sync_obj: true,
            mango_fx: MangoFx::default(),
            niri: NiriOptions::default(),
            hyprland: HyprlandOptions::default(),
            autostart: false,
            outputs: Vec::new(),
            input: crate::input::InputConfig::default(),
            disable_builtin_hot_corners: false,
        }
    }
}

pub fn default_keybinds() -> Vec<Keybind> {
    use BindAction::Mandara as M;
    vec![
        Keybind::new(&["super"], "space", M("launcher toggle".into())),
        Keybind::new(&["super", "alt"], "space", M("launcher toggle".into())),
        Keybind::new(&["super"], "b", M("sidebar toggle".into())),
        Keybind::new(&["super", "shift"], "e", M("session menu toggle".into())),
        Keybind::new(&["ctrl", "alt"], "Delete", M("session menu toggle".into())),
        Keybind::new(&["super", "shift"], "s", M("screenshot region".into())),
        Keybind::new(&["super", "shift"], "v", M("panel toggle clipboard".into())),
        Keybind::new(&["super", "shift"], "t", M("theme mode toggle".into())),
        Keybind::new(&["ctrl", "shift"], "w", M("wallpaper picker toggle".into())),
        // Alt-Tab window switcher (releasing Alt switches).
        Keybind::new(&["alt"], "Tab", M("switcher next".into())),
        Keybind::new(&["alt", "shift"], "Tab", M("switcher prev".into())),
        // The mute key silences everything (virtual sinks such as
        // EasyEffects' included); skipped when your config binds it already.
        Keybind::new(&[], "XF86AudioMute", M("audio mute-all toggle".into())),
        // Volume keys move by Audio → "Volume step" and stop at the
        // 100 % / 150 % ceiling; skipped when your config binds them.
        Keybind::new(&[], "XF86AudioRaiseVolume", M("audio volume up".into())),
        Keybind::new(&[], "XF86AudioLowerVolume", M("audio volume down".into())),
        // Laptop brightness keys (logind, no root; DDC/CI monitors follow
        // when set to); skipped when your config binds them.
        Keybind::new(&[], "XF86MonBrightnessUp", M("brightness up".into())),
        Keybind::new(&[], "XF86MonBrightnessDown", M("brightness down".into())),
    ]
}

/// Window appearance written to the compositor drop-in. `None` = keep the
/// compositor's own value (Mandara's drop-in is sourced last, so it only
/// emits what the user actually changed in Studio).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompositorLook {
    /// Gap between windows, logical px.
    pub gaps_inner: Option<u32>,
    /// Gap between windows and the screen edges, logical px.
    pub gaps_outer: Option<u32>,
    /// Window corner radius, logical px.
    pub rounding: Option<u32>,
    pub blur: Option<bool>,
    pub shadows: Option<bool>,
    pub animations: Option<bool>,
    /// Opacity of the focused / unfocused windows, `0.1..=1`.
    pub active_opacity: Option<f32>,
    pub inactive_opacity: Option<f32>,
    /// Window gaps follow the bar's edge spacing (resolved by
    /// [`crate::ShellConfig::effective_compositor`] before emitting).
    pub gaps_follow_bar: bool,
}

impl CompositorLook {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Palette colors pushed to compositor borders (`#rrggbb` / `#rrggbbaa`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BorderColors {
    pub active: String,
    pub active_end: String,
    pub inactive: String,
    pub urgent: String,
}

/// Everything an emitter needs.
#[derive(Clone, Debug, PartialEq)]
pub struct ManagedConfig<'a> {
    pub config: &'a CompositorConfig,
    pub borders: Option<BorderColors>,
    /// Chords already bound in the user's own config (see
    /// [`existing_chords`]); colliding Mandara binds are skipped.
    pub existing_chords: BTreeSet<String>,
    /// Absolute path of the `mandara` executable used in keybinds.
    pub mandara_bin: String,
    /// Running compositor version `(major, minor)`, when known.
    pub version: Option<(u32, u32)>,
    /// The user's own config already launches Mandara (see
    /// [`user_autostarts`]), so the drop-in mustn't start it again.
    pub user_autostarts: bool,
}

impl ManagedConfig<'_> {
    fn autostart(&self) -> bool {
        self.config.autostart && !self.user_autostarts
    }
}

/// Whether the user's own compositor config already starts Mandara.
pub fn user_autostarts(target: Target) -> bool {
    // The main file and every file it sources / includes (never Mandara's
    // own drop-in).
    let text: String = crate::conflicts::config_files(target).iter().filter_map(|f| std::fs::read_to_string(f).ok()).collect::<Vec<_>>().join("\n");
    starts_mandara(&text)
}

/// Whether config text has an autostart line launching `mandara` (a bind
/// running `mandara msg …` is not one).
pub fn starts_mandara(text: &str) -> bool {
    text.lines().map(str::trim).filter(|l| !l.starts_with('#') && !l.starts_with("//") && !l.starts_with("--")).any(|l| {
        let starter = l.starts_with("exec-once") || l.starts_with("exec_once") || l.starts_with("spawn-at-startup") || l.starts_with("spawn-sh-at-startup") || l.contains("hl.exec_cmd(") || l.starts_with("exec=") || l.starts_with("exec ");
        starter && l.split(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '=' || c == ',' || c == '(').any(|w| w == "mandara" || w.ends_with("/mandara"))
    })
}

/// Emitter output.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Emitted {
    pub text: String,
    /// Human-readable notes about skipped binds.
    pub skipped: Vec<String>,
}

const HEADER: &str = "Generated by Mandara Shell — edit ~/.config/mandara/config.toml or use Mandara Studio; this file is overwritten.";

fn hex_bare(c: &str) -> String {
    let h = c.trim_start_matches('#');
    if h.len() == 6 { format!("{h}ff") } else { h.to_string() }
}

/// A bind ready to write: its modifiers in the target's spelling and its
/// keysym.
struct ReadyBind<'a> {
    bind: &'a Keybind,
    mods: Vec<&'static str>,
    key: String,
}

fn split_binds<'a>(cfg: &'a ManagedConfig, target: Target) -> (Vec<ReadyBind<'a>>, Vec<String>) {
    let mut seen = BTreeSet::new();
    let mut keep = Vec::new();
    let mut skipped = Vec::new();
    for bind in &cfg.config.keybinds {
        let chord = bind.chord();
        let mods = bind.target_mods(target);
        if let Some(problem) = bind.problem() {
            skipped.push(format!("{} → `{}` skipped: {problem}", bind.display(), bind.action.shell_command()));
        } else if mods.is_none() {
            skipped.push(format!("{} skipped: a modifier this compositor can't bind", bind.display()));
        } else if cfg.existing_chords.contains(&chord) {
            skipped.push(format!("{} → `{}` skipped: already bound in your config", bind.display(), bind.action.shell_command()));
        } else if !seen.insert(chord) {
            skipped.push(format!("{} skipped: duplicate Mandara bind", bind.display()));
        } else {
            keep.push(ReadyBind { bind, mods: mods.unwrap_or_default(), key: bind.keysym() });
        }
    }
    (keep, skipped)
}

fn b(v: bool) -> u8 {
    v as u8
}

/// Layer-shell namespaces Mandara animates itself: every compositor gets
/// rules turning off its own animation and shadow for them (the lock
/// surface is handled separately because it keeps the compositor's blur).
pub const SHELL_SURFACES: &[&str] = &["mandara-bar", "mandara-exclusion", "mandara-launcher", "mandara-switcher", "mandara-dock", "mandara-wallpaper", "mandara-osd", "mandara-notifications", "mandara-installer", "mandara-desktop", "mandara-desktop-menu", "mandara-desktop-edit"];

/// `~/.config/mandara/generated/mango.conf`
pub fn emit_mango(cfg: &ManagedConfig) -> Emitted {
    let c = cfg.config;
    let mut s = String::new();
    let _ = writeln!(s, "# {HEADER}\n");
    let _ = writeln!(s, "# Shell surfaces: no compositor animation or shadow (Mandara animates itself).");
    for ns in SHELL_SURFACES {
        let _ = writeln!(s, "layer_rule=no_animation:1,no_shadow:1,no_blur:1,layer_name:{ns}");
    }
    // The lock screen and dialogs on its surface keep the compositor's blur.
    let _ = writeln!(s, "layer_rule=no_animation:1,no_shadow:1,no_blur:0,layer_name:mandara-lock\n");
    if cfg.autostart() {
        // MangoWM's keyword is `exec_once` (`exec-once` is rejected).
        let _ = writeln!(s, "exec_once={}\n", cfg.mandara_bin);
    }
    let _ = writeln!(s, "# SceneFX");
    let blur = c.look.blur.unwrap_or(c.mango_blur);
    let _ = writeln!(s, "blur={}\nblur_optimized={}\nsync_obj_enable={}", b(blur), b(c.mango_blur_optimized), b(c.mango_sync_obj));
    let fx = &c.mango_fx;
    for (key, v) in [("blur_params_radius", fx.blur_radius), ("blur_params_num_passes", fx.blur_passes), ("shadows_size", fx.shadows_size), ("shadows_blur", fx.shadows_blur)] {
        if let Some(v) = v {
            let _ = writeln!(s, "{key}={v}");
        }
    }
    let _ = writeln!(s);
    if c.disable_builtin_hot_corners {
        let _ = writeln!(s, "# Mandara's hot corners replace MangoWM's hot area (both would toggle the overview)\nenable_hotarea=0\n");
    }
    s.push_str(&crate::input::emit_mango(&c.input));
    let l = &c.look;
    if !l.is_empty() {
        let _ = writeln!(s, "# Window look (Studio → Compositor)");
        if let Some(g) = l.gaps_inner {
            let _ = writeln!(s, "gap_inner_horizontal={g}\ngap_inner_vertical={g}");
        }
        if let Some(g) = l.gaps_outer {
            let _ = writeln!(s, "gap_outer_horizontal={g}\ngap_outer_vertical={g}");
        }
        if let Some(r) = l.rounding {
            let _ = writeln!(s, "border_radius={r}");
        }
        if let Some(v) = l.shadows {
            let _ = writeln!(s, "shadows={}", b(v));
        }
        if let Some(v) = l.animations {
            let _ = writeln!(s, "animations={}", b(v));
        }
        if let Some(o) = l.active_opacity {
            let _ = writeln!(s, "focused_opacity={:.2}", o.clamp(0.1, 1.0));
        }
        if let Some(o) = l.inactive_opacity {
            let _ = writeln!(s, "unfocused_opacity={:.2}", o.clamp(0.1, 1.0));
        }
        let _ = writeln!(s);
    }
    // The width is its own setting: it applies without colour sync too.
    if let Some(w) = c.border_width {
        let _ = writeln!(s, "# Window border width (Studio → Compositor)\nborder_px={w}\n");
    }
    if let Some(bc) = &cfg.borders {
        let _ = writeln!(s, "# Live border colors from the active Mandara palette");
        let _ = writeln!(s, "focus_color=0x{}", hex_bare(&bc.active));
        let _ = writeln!(s, "border_color=0x{}", hex_bare(&bc.inactive));
        let _ = writeln!(s, "urgent_color=0x{}\n", hex_bare(&bc.urgent));
    }
    if !c.window_rules.is_empty() {
        let _ = writeln!(s, "# Window rules");
    }
    for r in &c.window_rules {
        let mut props = Vec::new();
        let flag = |props: &mut Vec<String>, k: &str, v: Option<bool>| {
            if let Some(v) = v {
                props.push(format!("{k}:{}", b(v)));
            }
        };
        flag(&mut props, "is_floating", r.floating);
        flag(&mut props, "is_global", r.global);
        flag(&mut props, "is_overlay", r.overlay);
        flag(&mut props, "no_blur", r.no_blur);
        flag(&mut props, "no_shadow", r.no_shadow);
        flag(&mut props, "confine_pointer", r.confine_pointer);
        flag(&mut props, "force_tiled_state", r.force_tiled_state);
        if let Some(o) = r.unfocused_opacity {
            props.push(format!("unfocused_opacity:{:.2}", o.clamp(0.1, 1.0)));
        }
        if let Some(o) = r.focused_opacity {
            props.push(format!("focused_opacity:{:.2}", o.clamp(0.1, 1.0)));
        }
        if let Some(a) = &r.app_id {
            props.push(format!("app_id:{}", rule_value(a)));
        }
        if let Some(t) = &r.title {
            props.push(format!("title:{}", rule_value(t)));
        }
        if props.iter().any(|p| p.starts_with("app_id:") || p.starts_with("title:")) {
            let _ = writeln!(s, "window_rule={}", props.join(","));
        }
    }
    for t in &c.tag_rules {
        let mut props = vec![format!("id:{}", t.id)];
        if let Some(m) = &t.monitor {
            props.push(format!("monitor_name:{m}"));
        }
        if let Some(l) = &t.layout {
            props.push(format!("layout_name:{l}"));
        }
        if let Some(p) = t.scroller_default_proportion {
            props.push(format!("scroller_default_proportion:{:.2}", p.clamp(0.1, 1.0)));
        }
        if let Some(f) = t.master_factor {
            props.push(format!("master_factor:{:.2}", f.clamp(0.05, 0.95)));
        }
        if let Some(n) = t.master_count {
            props.push(format!("master_count:{}", n.max(1)));
        }
        let _ = writeln!(s, "tag_rule={}", props.join(","));
    }
    s.push_str(&crate::outputs::emit_outputs(Target::Mango, &c.outputs, false));
    let (binds, skipped) = split_binds(cfg, Target::Mango);
    if !binds.is_empty() {
        let _ = writeln!(s, "\n# Mandara keybinds");
    }
    for k in binds {
        let mods = if k.mods.is_empty() { "NONE".to_string() } else { k.mods.join("+") };
        let _ = writeln!(s, "bind={mods},{},spawn_shell,{}", k.key, k.bind.action.shell_command_with(&cfg.mandara_bin));
    }
    for note in &skipped {
        let _ = writeln!(s, "# {note}");
    }
    Emitted { text: s, skipped }
}

/// A column proportion for niri (`0.1..=1`, up to 5 decimals).
fn proportion(w: f32) -> String {
    let v = format!("{:.5}", w.clamp(0.1, 1.0));
    let v = v.trim_end_matches('0');
    if v.ends_with('.') { format!("{v}0") } else { v.to_string() }
}

/// niri's default `layout { gaps }`.
const NIRI_DEFAULT_GAPS: u32 = 16;

fn kdl_str(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A window class or title for a `key:value,…` (MangoWM) or `a, b`
/// (Hyprland) rule line. Comes from other apps (a picked window's title), so
/// it can't add config lines (control characters dropped) or split the rule
/// (a comma becomes `.`, the regex wildcard, so it still matches).
fn rule_value(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).map(|c| if c == ',' { '.' } else { c }).collect()
}

/// `~/.config/mandara/generated/niri.kdl`, in the KDL v1 dialect Niri's
/// config uses (`open-floating true`); validated with `kdl`'s v1 parser.
pub fn emit_niri(cfg: &ManagedConfig) -> Emitted {
    let c = cfg.config;
    let mut s = String::new();
    let _ = writeln!(s, "// {HEADER}\n");
    for ns in SHELL_SURFACES {
        let _ = writeln!(s, "layer-rule {{\n    match namespace={}\n    shadow {{ off; }}\n}}", kdl_str(&format!("^{ns}$")));
    }
    let _ = writeln!(s, "layer-rule {{\n    match namespace=\"^mandara-wallpaper$\"\n    place-within-backdrop true\n}}");
    if cfg.autostart() {
        let _ = writeln!(s, "spawn-at-startup {}", kdl_str(&cfg.mandara_bin));
    }
    let l = &c.look;
    // The wallpaper sits in the backdrop (above): workspaces must be
    // see-through or their solid background hides it outside the Overview.
    let mut layout = String::from("    background-color \"transparent\"\n");
    let width = c.border_width.map(|w| format!("        width {w}\n")).unwrap_or_default();
    if let Some(bc) = &cfg.borders {
        let _ = write!(
            layout,
            "    focus-ring {{\n{width}        active-gradient from={} to={} angle=45\n        inactive-color {}\n        urgent-color {}\n    }}\n",
            kdl_str(&bc.active),
            kdl_str(&bc.active_end),
            kdl_str(&bc.inactive),
            kdl_str(&bc.urgent)
        );
    } else if !width.is_empty() {
        // The width is its own setting: it applies without colour sync too.
        let _ = write!(layout, "    focus-ring {{\n{width}    }}\n");
    }
    // niri has one gaps value (between windows and to the edges); struts
    // add (or, negative, remove) the difference at the screen edges.
    if let Some(g) = l.gaps_inner {
        let _ = writeln!(layout, "    gaps {g}");
    }
    if let Some(outer) = l.gaps_outer {
        let d = outer as i64 - l.gaps_inner.unwrap_or(NIRI_DEFAULT_GAPS) as i64;
        let _ = writeln!(layout, "    struts {{\n        left {d}\n        right {d}\n        top {d}\n        bottom {d}\n    }}");
    }
    if let Some(on) = l.shadows {
        let _ = writeln!(layout, "    shadow {{ {}; }}", if on { "on" } else { "off" });
    }
    let n = &c.niri;
    if let Some(w) = n.default_column_width {
        let _ = writeln!(layout, "    default-column-width {{ proportion {}; }}", proportion(w));
    }
    let presets: Vec<String> = n.preset_column_widths.iter().map(|w| format!("        proportion {}", proportion(*w))).collect();
    if !presets.is_empty() {
        let _ = writeln!(layout, "    preset-column-widths {{\n{}\n    }}", presets.join("\n"));
    }
    if let Some(mode) = n.center_focused_column.as_deref().filter(|m| matches!(*m, "never" | "always" | "on-overflow")) {
        let _ = writeln!(layout, "    center-focused-column {}", kdl_str(mode));
    }
    if n.always_center_single_column == Some(true) {
        let _ = writeln!(layout, "    always-center-single-column");
    }
    if !layout.is_empty() {
        let _ = write!(s, "\nlayout {{\n{layout}}}\n");
    }
    // Animations: only `off` exists as a switch (on = leave the user's).
    if l.animations == Some(false) {
        let _ = writeln!(s, "\nanimations {{ off; }}");
    }
    // Niri's top-left hot corner opens its overview; Mandara's corners
    // replace it (`hot-corners` predates `include`, so any niri that reads
    // this file knows it).
    if c.disable_builtin_hot_corners {
        let _ = writeln!(s, "\ngestures {{\n    hot-corners {{\n        off\n    }}\n}}");
    }
    s.push_str(&crate::input::emit_niri(&c.input));
    // Blur arrived in niri 26.04; older versions reject the node.
    if let Some(on) = l.blur {
        if cfg.version.is_some_and(|v| v >= (26, 4)) {
            if on {
                let _ = writeln!(s, "\nwindow-rule {{\n    background-effect {{ blur true; }}\n}}");
            } else {
                let _ = writeln!(s, "\nblur {{ off; }}");
            }
        } else {
            let _ = writeln!(s, "\n// blur needs niri 26.04+ (running version unknown or older)");
        }
    }
    if let Some(r) = l.rounding {
        let _ = writeln!(s, "\nwindow-rule {{\n    geometry-corner-radius {r}\n    clip-to-geometry true\n}}");
    }
    if let Some(o) = l.active_opacity {
        let _ = writeln!(s, "\nwindow-rule {{\n    match is-active=true\n    opacity {:.2}\n}}", o.clamp(0.1, 1.0));
    }
    if let Some(o) = l.inactive_opacity {
        let _ = writeln!(s, "\nwindow-rule {{\n    match is-active=false\n    opacity {:.2}\n}}", o.clamp(0.1, 1.0));
    }
    // Studio → Displays supersedes the older Niri-only output list (Niri
    // rejects two blocks for one output).
    let displays = crate::outputs::emit_outputs(Target::Niri, &c.outputs, false);
    let superseded = |name: &str| c.outputs.iter().any(|d| d.has_settings() && d.name.trim() == name.trim());
    s.push_str(&displays);
    for o in n.outputs.iter().filter(|o| !o.name.trim().is_empty() && !superseded(&o.name)) {
        let mut body = String::new();
        if let Some(scale) = o.scale {
            let _ = writeln!(body, "    scale {:.2}", scale.clamp(0.25, 10.0));
        }
        if let (Some(x), Some(y)) = (o.x, o.y) {
            let _ = writeln!(body, "    position x={x} y={y}");
        }
        if !body.is_empty() {
            let _ = write!(s, "\noutput {} {{\n{body}}}\n", kdl_str(o.name.trim()));
        }
    }
    for r in &c.window_rules {
        let mut matcher = String::new();
        if let Some(a) = &r.app_id {
            let _ = write!(matcher, " app-id={}", kdl_str(a));
        }
        if let Some(t) = &r.title {
            let _ = write!(matcher, " title={}", kdl_str(t));
        }
        if matcher.is_empty() {
            continue;
        }
        let mut body = String::new();
        if let Some(f) = r.floating {
            let _ = writeln!(body, "    open-floating {f}");
        }
        if r.no_shadow == Some(true) {
            let _ = writeln!(body, "    shadow {{ off; }}");
        }
        if !body.is_empty() {
            let _ = write!(s, "\nwindow-rule {{\n    match{matcher}\n{body}}}\n");
        }
        // Niri has one opacity per rule: focused / unfocused match on
        // `is-active` (same window, so both matchers AND together).
        for (active, o) in [(true, r.focused_opacity), (false, r.unfocused_opacity)] {
            if let Some(o) = o {
                let _ = write!(s, "\nwindow-rule {{\n    match{matcher} is-active={active}\n    opacity {:.2}\n}}\n", o.clamp(0.1, 1.0));
            }
        }
    }
    let (binds, skipped) = split_binds(cfg, Target::Niri);
    if !binds.is_empty() {
        let _ = writeln!(s, "\nbinds {{");
        for k in binds {
            let key = if k.key.len() == 1 { k.key.to_ascii_uppercase() } else { capitalize(&k.key) };
            let chord = if k.mods.is_empty() { key } else { format!("{}+{key}", k.mods.join("+")) };
            // A bare identifier unless KDL needs quotes (a lone digit key).
            let chord = if chord.starts_with(|c: char| c.is_ascii_digit()) { kdl_str(&chord) } else { chord };
            let argv: Vec<String> = match &k.bind.action {
                BindAction::Mandara(words) => {
                    [cfg.mandara_bin.as_str(), "msg"].iter().map(|w| kdl_str(w)).chain(words.split_whitespace().map(kdl_str)).collect()
                }
                BindAction::Spawn(cmd) => vec![kdl_str("sh"), kdl_str("-c"), kdl_str(cmd)],
            };
            let _ = writeln!(s, "    {chord} {{ spawn {}; }}", argv.join(" "));
        }
        let _ = writeln!(s, "}}");
    }
    for note in &skipped {
        let _ = writeln!(s, "// {note}");
    }
    Emitted { text: s, skipped }
}

/// The Hyprland drop-in in the format the user's Hyprland reads: Lua for
/// 0.55+ (`hyprland.lua`), hyprlang for older setups.
pub fn emit_hyprland(cfg: &ManagedConfig) -> Emitted {
    if Target::hyprland_lua() { emit_hyprland_lua(cfg) } else { emit_hyprland_conf(cfg) }
}

fn rgba(hex: &str) -> String {
    format!("rgba({})", hex_bare(hex))
}

/// `~/.config/mandara/generated/hyprland.conf` (hyprlang, Hyprland ≤ 0.54
/// syntax: `windowrule = match:…, effect on`).
pub fn emit_hyprland_conf(cfg: &ManagedConfig) -> Emitted {
    let c = cfg.config;
    let l = &c.look;
    let mut s = String::new();
    let _ = writeln!(s, "# {HEADER}\n");
    if cfg.autostart() {
        let _ = writeln!(s, "exec-once = {}\n", cfg.mandara_bin);
    }
    for ns in SHELL_SURFACES {
        let _ = writeln!(s, "layerrule = match:namespace ^({ns})$, no_anim on");
    }
    let _ = writeln!(s, "layerrule = match:namespace ^(mandara-lock)$, blur on");
    let mut general = String::new();
    if let Some(g) = l.gaps_inner {
        let _ = writeln!(general, "    gaps_in = {g}");
    }
    if let Some(g) = l.gaps_outer {
        let _ = writeln!(general, "    gaps_out = {g}");
    }
    if let Some(w) = c.border_width {
        let _ = writeln!(general, "    border_size = {w}");
    }
    if let Some(bc) = &cfg.borders {
        let _ = writeln!(general, "    col.active_border = {} {} 45deg\n    col.inactive_border = {}", rgba(&bc.active), rgba(&bc.active_end), rgba(&bc.inactive));
    }
    let layout = c.hyprland.layout_entries();
    for (_, key, value) in layout.iter().filter(|(sec, ..)| *sec == "general") {
        let _ = writeln!(general, "    {key} = {value}");
    }
    if !general.is_empty() {
        let _ = write!(s, "\ngeneral {{\n{general}}}\n");
    }
    for section in ["dwindle", "master"] {
        let body: String = layout.iter().filter(|(sec, ..)| *sec == section).map(|(_, k, v)| format!("    {k} = {v}\n")).collect();
        if !body.is_empty() {
            let _ = write!(s, "\n{section} {{\n{body}}}\n");
        }
    }
    let groupbar: String = layout.iter().filter(|(sec, ..)| *sec == "groupbar").map(|(_, k, v)| format!("        {k} = {v}\n")).collect();
    if !groupbar.is_empty() {
        let _ = write!(s, "\ngroup {{\n    groupbar {{\n{groupbar}    }}\n}}\n");
    }
    s.push_str(&crate::input::emit_hyprland_conf(&c.input));
    let mut deco = String::new();
    if let Some(r) = l.rounding {
        let _ = writeln!(deco, "    rounding = {r}");
    }
    if let Some(o) = l.active_opacity {
        let _ = writeln!(deco, "    active_opacity = {:.2}", o.clamp(0.1, 1.0));
    }
    if let Some(o) = l.inactive_opacity {
        let _ = writeln!(deco, "    inactive_opacity = {:.2}", o.clamp(0.1, 1.0));
    }
    let h = &c.hyprland;
    let mut blur = String::new();
    if let Some(v) = l.blur {
        let _ = writeln!(blur, "        enabled = {v}");
    }
    if let Some(v) = h.blur_size {
        let _ = writeln!(blur, "        size = {v}");
    }
    if let Some(v) = h.blur_passes {
        let _ = writeln!(blur, "        passes = {}", v.clamp(1, 8));
    }
    if !blur.is_empty() {
        let _ = writeln!(deco, "    blur {{\n{blur}    }}");
    }
    let mut shadow = String::new();
    if let Some(v) = l.shadows {
        let _ = writeln!(shadow, "        enabled = {v}");
    }
    if let Some(v) = h.shadow_range {
        let _ = writeln!(shadow, "        range = {v}");
    }
    if !shadow.is_empty() {
        let _ = writeln!(deco, "    shadow {{\n{shadow}    }}");
    }
    if let Some(v) = h.dim_inactive {
        let _ = writeln!(deco, "    dim_inactive = {v}");
    }
    if let Some(v) = h.dim_strength {
        let _ = writeln!(deco, "    dim_strength = {:.2}", v.clamp(0.0, 1.0));
    }
    if !deco.is_empty() {
        let _ = write!(s, "\ndecoration {{\n{deco}}}\n");
    }
    if let Some(v) = l.animations {
        let _ = writeln!(s, "\nanimations {{\n    enabled = {v}\n}}");
    }
    for r in &c.window_rules {
        let mut matchers = Vec::new();
        if let Some(a) = &r.app_id {
            matchers.push(format!("match:class {}", rule_value(a)));
        }
        if let Some(t) = &r.title {
            matchers.push(format!("match:title {}", rule_value(t)));
        }
        if matchers.is_empty() {
            continue;
        }
        let m = matchers.join(", ");
        let mut effects = Vec::new();
        if r.floating == Some(true) {
            effects.push("float on".to_string());
        }
        if r.global == Some(true) {
            effects.push("pin on".to_string());
        }
        if r.focused_opacity.is_some() || r.unfocused_opacity.is_some() {
            let (a, i) = (r.focused_opacity.unwrap_or(1.0).clamp(0.1, 1.0), r.unfocused_opacity.unwrap_or(1.0).clamp(0.1, 1.0));
            effects.push(format!("opacity {a:.2} {i:.2}"));
        }
        if r.no_blur == Some(true) {
            effects.push("no_blur on".to_string());
        }
        if r.no_shadow == Some(true) {
            effects.push("no_shadow on".to_string());
        }
        for e in effects {
            let _ = writeln!(s, "windowrule = {m}, {e}");
        }
    }
    s.push_str(&crate::outputs::emit_outputs(Target::Hyprland, &c.outputs, false));
    let (binds, skipped) = split_binds(cfg, Target::Hyprland);
    if !binds.is_empty() {
        let _ = writeln!(s);
    }
    for k in binds {
        let _ = writeln!(s, "bind = {}, {}, exec, {}", k.mods.join(" "), k.key, k.bind.action.shell_command_with(&cfg.mandara_bin));
    }
    for note in &skipped {
        let _ = writeln!(s, "# {note}");
    }
    Emitted { text: s, skipped }
}

fn lua_str(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `~/.config/mandara/generated/hyprland.lua` (Hyprland 0.55+ Lua config).
/// Options use `hl.config` (documented); rule and bind calls are wrapped in
/// `pcall` so an API difference can never break the user's config.
pub fn emit_hyprland_lua(cfg: &ManagedConfig) -> Emitted {
    let c = cfg.config;
    let l = &c.look;
    let mut s = String::new();
    let _ = writeln!(s, "-- {HEADER}\n");
    if cfg.autostart() {
        let _ = writeln!(s, "pcall(hl.on, \"hyprland.start\", function() hl.exec_cmd({}) end)\n", lua_str(&cfg.mandara_bin));
    }
    let mut general: Vec<String> = Vec::new();
    if let Some(g) = l.gaps_inner {
        general.push(format!("gaps_in = {g}"));
    }
    if let Some(g) = l.gaps_outer {
        general.push(format!("gaps_out = {g}"));
    }
    if let Some(w) = c.border_width {
        general.push(format!("border_size = {w}"));
    }
    if let Some(bc) = &cfg.borders {
        general.push(format!(
            "col = {{ active_border = {{ colors = {{ {}, {} }}, angle = 45 }}, inactive_border = {} }}",
            lua_str(&rgba(&bc.active)),
            lua_str(&rgba(&bc.active_end)),
            lua_str(&rgba(&bc.inactive))
        ));
    }
    let layout = c.hyprland.layout_entries();
    let lua_value = |v: &str| if v == "true" || v == "false" || v.parse::<f64>().is_ok() { v.to_string() } else { lua_str(v) };
    let lua_fields = |section: &str| layout.iter().filter(|(sec, ..)| *sec == section).map(|(_, k, v)| format!("{k} = {}", lua_value(v))).collect::<Vec<_>>();
    general.extend(lua_fields("general"));
    let mut decoration: Vec<String> = Vec::new();
    if let Some(r) = l.rounding {
        decoration.push(format!("rounding = {r}"));
    }
    if let Some(o) = l.active_opacity {
        decoration.push(format!("active_opacity = {:.2}", o.clamp(0.1, 1.0)));
    }
    if let Some(o) = l.inactive_opacity {
        decoration.push(format!("inactive_opacity = {:.2}", o.clamp(0.1, 1.0)));
    }
    let h = &c.hyprland;
    let blur: Vec<String> = [l.blur.map(|v| format!("enabled = {v}")), h.blur_size.map(|v| format!("size = {v}")), h.blur_passes.map(|v| format!("passes = {}", v.clamp(1, 8)))].into_iter().flatten().collect();
    if !blur.is_empty() {
        decoration.push(format!("blur = {{ {} }}", blur.join(", ")));
    }
    let shadow: Vec<String> = [l.shadows.map(|v| format!("enabled = {v}")), h.shadow_range.map(|v| format!("range = {v}"))].into_iter().flatten().collect();
    if !shadow.is_empty() {
        decoration.push(format!("shadow = {{ {} }}", shadow.join(", ")));
    }
    if let Some(v) = h.dim_inactive {
        decoration.push(format!("dim_inactive = {v}"));
    }
    if let Some(v) = h.dim_strength {
        decoration.push(format!("dim_strength = {:.2}", v.clamp(0.0, 1.0)));
    }
    let mut sections: Vec<String> = Vec::new();
    if !general.is_empty() {
        sections.push(format!("    general = {{ {} }},", general.join(", ")));
    }
    if !decoration.is_empty() {
        sections.push(format!("    decoration = {{ {} }},", decoration.join(", ")));
    }
    if let Some(v) = l.animations {
        sections.push(format!("    animations = {{ enabled = {v} }},"));
    }
    for section in ["dwindle", "master"] {
        let fields = lua_fields(section);
        if !fields.is_empty() {
            sections.push(format!("    {section} = {{ {} }},", fields.join(", ")));
        }
    }
    let groupbar = lua_fields("groupbar");
    if !groupbar.is_empty() {
        sections.push(format!("    group = {{ groupbar = {{ {} }} }},", groupbar.join(", ")));
    }
    let (input_sections, input_env) = crate::input::hyprland_lua_sections(&c.input);
    sections.extend(input_sections);
    if !sections.is_empty() {
        let _ = writeln!(s, "hl.config({{\n{}\n}})\n", sections.join("\n"));
    }
    for line in input_env {
        let _ = writeln!(s, "{line}");
    }
    for ns in SHELL_SURFACES {
        let _ = writeln!(s, "pcall(hl.layer_rule, {{ match = {{ namespace = {} }}, no_anim = true }})", lua_str(&format!("^({ns})$")));
    }
    for r in &c.window_rules {
        let mut matcher = Vec::new();
        if let Some(a) = &r.app_id {
            matcher.push(format!("class = {}", lua_str(a)));
        }
        if let Some(t) = &r.title {
            matcher.push(format!("title = {}", lua_str(t)));
        }
        if matcher.is_empty() {
            continue;
        }
        let mut effects = Vec::new();
        if r.floating == Some(true) {
            effects.push("float = true".to_string());
        }
        if r.global == Some(true) {
            effects.push("pin = true".to_string());
        }
        if r.focused_opacity.is_some() || r.unfocused_opacity.is_some() {
            // Same effect as hyprlang's `opacity <active> <inactive>`.
            let (a, i) = (r.focused_opacity.unwrap_or(1.0).clamp(0.1, 1.0), r.unfocused_opacity.unwrap_or(1.0).clamp(0.1, 1.0));
            effects.push(format!("opacity = {}", lua_str(&format!("{a:.2} {i:.2}"))));
        }
        if r.no_blur == Some(true) {
            effects.push("no_blur = true".to_string());
        }
        if r.no_shadow == Some(true) {
            effects.push("no_shadow = true".to_string());
        }
        if !effects.is_empty() {
            let _ = writeln!(s, "pcall(hl.window_rule, {{ match = {{ {} }}, {} }})", matcher.join(", "), effects.join(", "));
        }
    }
    s.push_str(&crate::outputs::emit_outputs(Target::Hyprland, &c.outputs, true));
    let (binds, skipped) = split_binds(cfg, Target::Hyprland);
    if !binds.is_empty() {
        let _ = writeln!(s);
    }
    for k in binds {
        let chord = k.mods.iter().copied().chain(std::iter::once(k.key.as_str())).collect::<Vec<_>>().join(" + ");
        let _ = writeln!(s, "pcall(hl.bind, {}, hl.dsp.exec_cmd({}))", lua_str(&chord), lua_str(&k.bind.action.shell_command_with(&cfg.mandara_bin)));
    }
    for note in &skipped {
        let _ = writeln!(s, "-- {note}");
    }
    Emitted { text: s, skipped }
}

pub fn emit(target: Target, cfg: &ManagedConfig) -> Emitted {
    match target {
        Target::Mango => emit_mango(cfg),
        Target::Niri => emit_niri(cfg),
        Target::Hyprland => emit_hyprland(cfg),
    }
}

/// The file an include line pulls in, as written: MangoWM / Hyprland
/// `source[_optional] = path`, Niri `include [optional=true] "path"`,
/// Hyprland Lua `require("mod")` / `dofile("path")` (also through `pcall`;
/// a module name `a.b` is the file `a/b.lua` next to the config).
pub(crate) fn include_path(target: Target, line: &str) -> Option<String> {
    let line = line.trim();
    match target {
        Target::Niri => {
            let rest = line.strip_prefix("include")?.trim_start();
            // `include "x"` / `include optional=true "x"`: the last string.
            let end = rest.rfind('"')?;
            let start = rest[..end].rfind('"')?;
            Some(rest[start + 1..end].replace("\\\\", "\\").replace("\\\"", "\""))
        }
        Target::Mango | Target::Hyprland if !line.contains("hl.") && !line.contains("require") && !line.contains("dofile") => {
            line.strip_prefix("source_optional").or_else(|| line.strip_prefix("source")).and_then(|r| r.trim_start().strip_prefix('=')).map(|p| p.trim().to_string())
        }
        Target::Mango => None,
        Target::Hyprland => {
            let (call, at) = ["require", "dofile"].iter().find_map(|f| line.find(f).map(|i| (*f, i + f.len())))?;
            let rest = &line[at..];
            // Only a single literal: `os.getenv("HOME") .. "/x"` can't be resolved.
            let quote = rest.find(['"', '\''])?;
            let q = rest.as_bytes()[quote] as char;
            let lit_end = rest[quote + 1..].find(q)? + quote + 1;
            let before_ok = rest[..quote].chars().all(|c| c.is_whitespace() || c == '(' || c == ',');
            let after_ok = rest[lit_end + 1..].trim_start().starts_with([')', ',']);
            if !before_ok || !after_ok {
                return None;
            }
            let lit = &rest[quote + 1..lit_end];
            let explicit = lit.starts_with(['/', '~', '.']) || lit.ends_with(".lua");
            Some(if call == "require" && !explicit { format!("{}.lua", lit.replace('.', "/")) } else { lit.to_string() })
        }
    }
}

/// Expands `source=`/`include` paths relative to `base`.
pub(crate) fn resolve_include(base: &Path, raw: &str) -> PathBuf {
    let raw = raw.trim().trim_matches('"');
    let p = paths::expand_tilde(raw);
    if p.is_absolute() { p } else { base.parent().unwrap_or(Path::new(".")).join(p) }
}

/// Chords bound in the user's own config, following `source=` / `include`
/// (but never Mandara's generated file).
pub fn existing_chords(target: Target) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut stack = vec![target.user_config()];
    let mut visited = BTreeSet::new();
    let generated = target.generated_file();
    while let Some(file) = stack.pop() {
        // Never count a Mandara drop-in (ours, or another copy's) as the
        // user's own binds.
        if !visited.insert(file.clone()) || file == generated || is_generated(&file) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        out.extend(parse_chords(target, &text));
        for line in text.lines().filter(|l| !l.trim_start().starts_with(['#', '/']) && !l.trim_start().starts_with("--")) {
            if let Some(path) = include_path(target, line) {
                stack.push(resolve_include(&file, &path));
            }
        }
    }
    out
}

/// A file Mandara generates (`…/mandara/generated/…`).
pub fn is_generated(path: &Path) -> bool {
    path.to_string_lossy().contains("/mandara/generated/")
}

/// Hyprland's modifier parsing: it looks for each modifier's name anywhere
/// in the field, so `SUPER_SHIFT`, `SUPERSHIFT` and `SUPER SHIFT` all mean
/// the same.
fn hypr_mods(field: &str) -> Vec<&'static str> {
    let upper = field.to_ascii_uppercase();
    let names: &[(&[&str], &'static str)] = &[(&["SUPER", "WIN", "LOGO", "MOD4"], "super"), (&["CTRL", "CONTROL"], "ctrl"), (&["ALT", "MOD1"], "alt"), (&["SHIFT"], "shift"), (&["MOD3"], "hyper"), (&["MOD5"], "mod5")];
    names.iter().filter(|(aliases, _)| aliases.iter().any(|a| upper.contains(a))).map(|&(_, m)| m).collect()
}

/// Parses keybind chords from one config file's text.
pub fn parse_chords(target: Target, text: &str) -> Vec<String> {
    chords_by_line(target, text).into_iter().map(|(_, c)| c).collect()
}

/// Hyprland binds with their 0-based line: hyprlang `bind… = MODS, key, …`
/// (with `$var` substitution) and Lua `hl.bind("SUPER + Q", …)` (string
/// literals and `local x = "…"` variables joined with `..`; binds built in
/// loops can't be resolved and are left out). MangoWM: `bind[lr]=` lines;
/// Niri: the first line of each child of `binds {}`.
pub fn chords_by_line(target: Target, text: &str) -> Vec<(usize, String)> {
    match target {
        Target::Mango => mango_chords(text),
        Target::Niri => niri_chords(text),
        Target::Hyprland => hyprland_chords(text),
    }
}

fn mango_chords(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        let Some(rest) = ["bind=", "bindl=", "bindr=", "bindlr="].iter().find_map(|p| line.strip_prefix(p)) else { continue };
        let mut parts = rest.splitn(3, ',');
        if let (Some(mods), Some(key)) = (parts.next(), parts.next()) {
            out.push((i, chord(mods.split('+'), &keysym_for(key.trim()))));
        }
    }
    out
}

fn niri_chords(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_binds = false;
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.starts_with("//") {
            continue;
        }
        if depth == 0 && line.starts_with("binds") {
            in_binds = true;
        } else if in_binds
            && depth == 1
            && let Some(first) = line.split_whitespace().next().filter(|w| !w.starts_with('}'))
        {
            let mut pieces: Vec<&str> = first.trim_matches('"').split('+').collect();
            let key = pieces.pop().unwrap_or_default();
            out.push((i, chord(pieces.into_iter(), &keysym_for(key))));
        }
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        if depth <= 0 {
            in_binds = false;
            depth = 0;
        }
    }
    out
}

fn hyprland_chords(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    // `$mainMod = SUPER` (hyprlang) and `local mainMod = "SUPER"` (Lua).
    let mut vars: Vec<(String, String)> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let lua_line = raw.trim_start().starts_with("local ") || raw.contains("hl.");
        let line = if lua_line { raw.split("--").next().unwrap_or_default().trim() } else { raw.split('#').next().unwrap_or_default().trim() };
        if let Some((name, value)) = line.strip_prefix('$').and_then(|r| r.split_once('=')) {
            vars.push((format!("${}", name.trim()), value.trim().to_string()));
            // Longest first, so `$mod` never eats `$modShift`.
            vars.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
            continue;
        }
        if let Some((name, value)) = line.strip_prefix("local ").and_then(|r| r.split_once('=')) {
            if let Some(v) = lua_concat(value.trim(), &vars) {
                vars.push((name.trim().to_string(), v));
            }
            continue;
        }
        if let Some(at) = line.find("hl.bind(") {
            let args = &line[at + "hl.bind(".len()..];
            let Some(first) = lua_first_arg(args) else { continue };
            let Some(combo) = lua_concat(first, &vars) else { continue };
            let mut parts: Vec<&str> = combo.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
            let Some(key) = parts.pop() else { continue };
            out.push((i, chord(hypr_mods(&parts.join(" ")).into_iter(), &keysym_for(key))));
            continue;
        }
        let Some(rest) = line.strip_prefix("bind").and_then(|r| r.split_once('=')).map(|(_, r)| r) else { continue };
        let mut rest = rest.to_string();
        for (name, value) in vars.iter().filter(|(n, _)| n.starts_with('$')) {
            rest = rest.replace(name.as_str(), value);
        }
        let mut parts = rest.splitn(3, ',');
        if let (Some(mods), Some(key)) = (parts.next(), parts.next()) {
            out.push((i, chord(hypr_mods(mods).into_iter(), &keysym_for(key.trim()))));
        }
    }
    out
}

/// The first argument of a Lua call (up to its top-level comma or `)`).
fn lua_first_arg(args: &str) -> Option<&str> {
    let (mut depth, mut quote) = (0i32, None::<char>);
    for (i, c) in args.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(' | '{') => depth += 1,
            (None, ')' | '}') if depth == 0 => return Some(args[..i].trim()),
            (None, ')' | '}') => depth -= 1,
            (None, ',') if depth == 0 => return Some(args[..i].trim()),
            _ => {}
        }
    }
    None
}

/// Value of a Lua expression made of string literals and known variables
/// joined with `..` (`mainMod .. " + Q"`); `None` for anything else.
fn lua_concat(expr: &str, vars: &[(String, String)]) -> Option<String> {
    let mut out = String::new();
    for piece in expr.split("..").map(str::trim) {
        let quoted = piece.len() >= 2 && ((piece.starts_with('"') && piece.ends_with('"')) || (piece.starts_with('\'') && piece.ends_with('\'')));
        if quoted {
            out.push_str(&piece[1..piece.len() - 1]);
        } else {
            out.push_str(&vars.iter().find(|(n, _)| n == piece)?.1);
        }
    }
    Some(out)
}

/// Writes the generated drop-in atomically; returns its path.
pub fn write_generated(target: Target, emitted: &Emitted) -> Result<PathBuf> {
    let path = target.generated_file();
    crate::store::write_atomic(&path, emitted.text.as_bytes())?;
    Ok(path)
}

/// Whether the user's config already includes the generated file.
pub fn include_installed(target: Target) -> bool {
    include_installed_at(target, &target.user_config())
}

fn include_installed_at(target: Target, config: &Path) -> bool {
    let needle = target.generated_file();
    let needle_name = needle.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
    std::fs::read_to_string(config)
        .map(|t| t.lines().any(|l| !l.trim_start().starts_with(['#', '/']) && !l.trim_start().starts_with("--") && l.contains("mandara/generated/") && l.contains(&needle_name)))
        .unwrap_or(false)
}

/// Appends the include line to the user's config if missing. Nothing else in
/// the file is touched; a backup is written first.
///
/// The compositor then checks the result (`mango -p`, `niri validate`,
/// `Hyprland --verify-config`, when installed): if it rejects the config —
/// e.g. a Niri too old for `include` — the original is put back and an error
/// explains why.
pub fn install_include(target: Target) -> Result<bool> {
    let path = target.user_config();
    let before = std::fs::read_to_string(&path).ok();
    let added = install_include_at(target, &path)?;
    if added {
        if let crate::conflicts::Validation::Invalid { tool, report } = crate::conflicts::validate(target) {
            if let Some(original) = before {
                crate::store::write_atomic(&path, original.as_bytes())?;
            }
            anyhow::bail!("{tool} rejects {} with Mandara's include line, so it was not added:\n{report}", path.display());
        }
    }
    Ok(added)
}

fn install_include_at(target: Target, path: &Path) -> Result<bool> {
    if include_installed_at(target, path) {
        return Ok(false);
    }
    let original = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let backup = path.with_extension(format!("{}.mandara-backup", path.extension().and_then(|e| e.to_str()).unwrap_or("conf")));
    std::fs::write(&backup, &original).with_context(|| format!("writing backup {}", backup.display()))?;
    let comment = match target {
        Target::Niri => "// Mandara Shell (generated rules, binds and colors)",
        Target::Hyprland if Target::hyprland_lua() => "-- Mandara Shell (generated rules, binds and colors)",
        _ => "# Mandara Shell (generated rules, binds and colors)",
    };
    let sep = if original.ends_with('\n') { "" } else { "\n" };
    let updated = format!("{original}{sep}\n{comment}\n{}\n", target.include_line());
    crate::store::write_atomic(path, updated.as_bytes())?;
    Ok(true)
}

#[cfg(test)]
mod look_tests {
    use super::*;

    fn managed(config: &CompositorConfig) -> ManagedConfig<'_> {
        ManagedConfig {
            config,
            borders: Some(BorderColors { active: "#c8793c".into(), active_end: "#e0a060".into(), inactive: "#2a2420".into(), urgent: "#ff4f4f".into() }),
            existing_chords: Default::default(),
            mandara_bin: "mandara".into(),
            version: None,
            user_autostarts: false,
        }
    }

    #[test]
    fn mango_accepts_the_autostart_line() {
        if std::process::Command::new("mango").arg("-v").output().is_err() {
            return; // MangoWM isn't installed here.
        }
        let c = CompositorConfig { autostart: true, ..Default::default() };
        let mut m = managed(&c);
        m.mandara_bin = "/home/u/.local/bin/mandara".into();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mango.conf");
        std::fs::write(&file, emit_mango(&m).text).unwrap();
        let out = std::process::Command::new("mango").arg("-p").arg("-c").arg(&file).output().unwrap();
        let log = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        assert!(!log.contains("ERROR"), "{log}");
        assert!(emit_mango(&m).text.contains("exec_once=/home/u/.local/bin/mandara"));
    }

    #[test]
    fn autostart_lines_per_compositor() {
        let cfg = CompositorConfig { autostart: true, ..Default::default() };
        let m = managed(&cfg);
        assert!(emit_mango(&m).text.contains("exec_once=mandara"));
        assert!(emit_niri(&m).text.contains("spawn-at-startup \"mandara\""));
        assert!(emit_hyprland_conf(&m).text.contains("exec-once = mandara"));
        assert!(emit_hyprland_lua(&m).text.contains("hl.exec_cmd(\"mandara\")"));
        let already = ManagedConfig { user_autostarts: true, ..managed(&cfg) };
        assert!(!emit_mango(&already).text.contains("exec-once"));
        let off = CompositorConfig::default();
        assert!(!emit_niri(&managed(&off)).text.contains("spawn-at-startup"));
    }

    #[test]
    fn mandara_hot_corners_switch_the_compositors_off() {
        let off = CompositorConfig::default();
        assert!(!emit_mango(&managed(&off)).text.contains("enable_hotarea"));
        assert!(!emit_niri(&managed(&off)).text.contains("hot-corners"));
        let on = CompositorConfig { disable_builtin_hot_corners: true, ..CompositorConfig::default() };
        assert!(emit_mango(&managed(&on)).text.lines().any(|l| l == "enable_hotarea=0"));
        let niri = emit_niri(&managed(&on)).text;
        assert!(niri.contains("hot-corners"));
        // Still valid KDL v1 (Niri's dialect).
        assert!(kdl::KdlDocument::parse_v1(&niri).is_ok(), "{niri}");
        // Hyprland has no built-in hot corner: nothing to switch off.
        assert_eq!(emit_hyprland(&managed(&on)).text, emit_hyprland(&managed(&off)).text);
        // The flag is derived, never written to config.toml when off.
        assert!(!toml::to_string(&off).unwrap().contains("disable_builtin_hot_corners"));
    }

    fn full_look() -> CompositorConfig {
        CompositorConfig {
            look: CompositorLook {
                gaps_inner: Some(12),
                gaps_outer: Some(10),
                rounding: Some(14),
                blur: Some(true),
                shadows: Some(false),
                animations: Some(true),
                active_opacity: Some(1.0),
                inactive_opacity: Some(0.9),
                gaps_follow_bar: false,
            },
            ..CompositorConfig::default()
        }
    }

    #[test]
    fn unset_look_emits_nothing() {
        let c = CompositorConfig::default();
        let text = emit_mango(&managed(&c)).text;
        assert!(!text.contains("gap_inner"), "keeps the user's own gaps");
        assert!(!text.contains("border_radius"));
    }

    #[test]
    fn mango_look_keys() {
        let c = full_look();
        let text = emit_mango(&managed(&c)).text;
        for line in ["gap_inner_horizontal=12", "gap_outer_vertical=10", "border_radius=14", "shadows=0", "unfocused_opacity=0.90"] {
            assert!(text.contains(line), "missing {line}");
        }
    }

    #[test]
    fn niri_look_is_valid_kdl_with_struts_and_rules() {
        let c = full_look();
        let mut m = managed(&c);
        let old = emit_niri(&m).text;
        assert!(old.contains("blur needs niri 26.04"), "blur skipped when version unknown");
        m.version = Some((26, 4));
        let text = emit_niri(&m).text;
        kdl::KdlDocument::parse_v1(&text).unwrap_or_else(|e| panic!("niri drop-in is not valid KDL v1: {e:?}\n{text}"));
        for needle in ["gaps 12", "left -2", "shadow { off; }", "geometry-corner-radius 14", "match is-active=false", "background-effect { blur true; }"] {
            assert!(text.contains(needle), "missing {needle}\n{text}");
        }
    }

    #[test]
    fn hyprlang_uses_current_rule_syntax() {
        let c = full_look();
        let text = emit_hyprland_conf(&managed(&c)).text;
        assert!(!text.contains("windowrulev2"), "windowrulev2 is obsolete");
        for needle in ["gaps_in = 12", "gaps_out = 10", "rounding = 14", "inactive_opacity = 0.90", "enabled = false", "windowrule = match:class"] {
            assert!(text.contains(needle), "missing {needle}\n{text}");
        }
    }

    #[test]
    fn hyprland_lua_shape() {
        let c = full_look();
        let text = emit_hyprland_lua(&managed(&c)).text;
        assert!(text.contains("hl.config({"));
        assert!(text.contains("gaps_in = 12") && text.contains("rounding = 14"));
        assert!(text.contains("pcall(hl.window_rule"));
        let (open, close) = (text.matches('{').count(), text.matches('}').count());
        assert_eq!(open, close, "balanced braces\n{text}");
        assert_eq!(text.matches('(').count(), text.matches(')').count(), "balanced parens");
    }

    /// Everything the Studio compositor pages can set.
    fn studio_full() -> CompositorConfig {
        let mut c = full_look();
        c.window_rules.push(WindowRule {
            app_id: Some("^pavucontrol$".into()),
            title: Some("Volume".into()),
            floating: Some(true),
            global: Some(true),
            overlay: Some(false),
            focused_opacity: Some(0.95),
            unfocused_opacity: Some(0.8),
            no_blur: Some(true),
            no_shadow: Some(true),
            confine_pointer: Some(false),
            force_tiled_state: Some(true),
        });
        c.tag_rules = vec![
            TagRule { id: 1, layout: Some("scroller".into()), scroller_default_proportion: Some(0.8), ..Default::default() },
            TagRule { id: 2, monitor: Some("HDMI-A-1".into()), layout: Some("tile".into()), master_factor: Some(0.6), master_count: Some(2), ..Default::default() },
        ];
        c.mango_fx = MangoFx { blur_radius: Some(6), blur_passes: Some(2), shadows_size: Some(12), shadows_blur: Some(15) };
        c.niri = NiriOptions {
            default_column_width: Some(0.5),
            preset_column_widths: vec![1.0 / 3.0, 0.5, 2.0 / 3.0],
            center_focused_column: Some("on-overflow".into()),
            always_center_single_column: Some(true),
            outputs: vec![OutputRule { name: "eDP-1".into(), scale: Some(1.5), x: Some(0), y: Some(0) }, OutputRule { name: "  ".into(), scale: Some(2.0), ..Default::default() }],
        };
        c.hyprland = HyprlandOptions { blur_size: Some(8), blur_passes: Some(3), shadow_range: Some(20), dim_inactive: Some(true), dim_strength: Some(0.1), ..Default::default() };
        c
    }

    #[test]
    fn studio_options_reach_every_emitter() {
        let c = studio_full();
        let mango = emit_mango(&managed(&c)).text;
        for needle in [
            "blur_params_radius=6",
            "blur_params_num_passes=2",
            "shadows_size=12",
            "shadows_blur=15",
            "focused_opacity:0.95",
            "unfocused_opacity:0.80",
            "tag_rule=id:1,layout_name:scroller,scroller_default_proportion:0.80",
            "tag_rule=id:2,monitor_name:HDMI-A-1,layout_name:tile,master_factor:0.60,master_count:2",
        ] {
            assert!(mango.contains(needle), "mango missing {needle}\n{mango}");
        }
        let niri = emit_niri(&managed(&c)).text;
        kdl::KdlDocument::parse_v1(&niri).unwrap_or_else(|e| panic!("niri drop-in is not valid KDL v1: {e:?}\n{niri}"));
        for needle in [
            "default-column-width { proportion 0.5; }",
            "proportion 0.33333",
            "proportion 0.66667",
            "center-focused-column \"on-overflow\"",
            "always-center-single-column",
            "output \"eDP-1\" {\n    scale 1.50\n    position x=0 y=0\n}",
            "is-active=true\n    opacity 0.95",
            "is-active=false\n    opacity 0.80",
        ] {
            assert!(niri.contains(needle), "niri missing {needle}\n{niri}");
        }
        assert_eq!(niri.matches("\noutput ").count(), 1, "blank output names are skipped");
        let hypr = emit_hyprland_conf(&managed(&c)).text;
        for needle in ["size = 8", "passes = 3", "range = 20", "dim_inactive = true", "dim_strength = 0.10", "opacity 0.95 0.80", "pin on", "no_blur on"] {
            assert!(hypr.contains(needle), "hyprland missing {needle}\n{hypr}");
        }
        assert_eq!(hypr.matches('{').count(), hypr.matches('}').count(), "balanced hyprlang blocks");
        let lua = emit_hyprland_lua(&managed(&c)).text;
        assert!(lua.contains("blur = { enabled = true, size = 8, passes = 3 }") && lua.contains("dim_strength = 0.10"), "{lua}");
        assert_eq!(lua.matches('{').count(), lua.matches('}').count(), "balanced Lua tables");
    }

    /// The support table Studio shows matches what each emitter writes.
    #[test]
    fn rule_support_table_matches_emitters() {
        let base = WindowRule { app_id: Some("^probe$".into()), ..Default::default() };
        let with = |name: &str| {
            let mut r = base.clone();
            match name {
                "floating" => r.floating = Some(true),
                "global" => r.global = Some(true),
                "overlay" => r.overlay = Some(true),
                "focused_opacity" => r.focused_opacity = Some(0.9),
                "unfocused_opacity" => r.unfocused_opacity = Some(0.9),
                "no_blur" => r.no_blur = Some(true),
                "no_shadow" => r.no_shadow = Some(true),
                "confine_pointer" => r.confine_pointer = Some(true),
                "force_tiled_state" => r.force_tiled_state = Some(true),
                other => panic!("untested option {other}"),
            }
            r
        };
        let render = |target: Target, rule: WindowRule| {
            let c = CompositorConfig { window_rules: vec![rule], keybinds: Vec::new(), ..Default::default() };
            match target {
                Target::Hyprland => emit_hyprland_conf(&managed(&c)).text,
                t => emit(t, &managed(&c)).text,
            }
        };
        for (name, _) in RULE_OPTIONS {
            for target in Target::ALL {
                let changed = render(target, with(name)) != render(target, base.clone());
                assert_eq!(changed, rule_option_supported(target, name), "{target:?} / {name}");
            }
        }
    }

    /// The Lua emitter (Hyprland 0.55+) honours the same rule options as
    /// hyprlang, so the support table holds for both formats.
    #[test]
    fn hyprland_lua_rules_match_the_support_table() {
        let base = WindowRule { app_id: Some("^probe$".into()), ..Default::default() };
        let render = |rule: WindowRule| emit_hyprland_lua(&managed(&CompositorConfig { window_rules: vec![rule], keybinds: Vec::new(), ..Default::default() })).text;
        let plain = render(base.clone());
        for (name, support) in RULE_OPTIONS {
            let mut r = base.clone();
            match *name {
                "floating" => r.floating = Some(true),
                "global" => r.global = Some(true),
                "overlay" => r.overlay = Some(true),
                "focused_opacity" => r.focused_opacity = Some(0.9),
                "unfocused_opacity" => r.unfocused_opacity = Some(0.9),
                "no_blur" => r.no_blur = Some(true),
                "no_shadow" => r.no_shadow = Some(true),
                "confine_pointer" => r.confine_pointer = Some(true),
                "force_tiled_state" => r.force_tiled_state = Some(true),
                other => panic!("untested option {other}"),
            }
            assert_eq!(render(r) != plain, support[2], "Lua / {name}");
        }
        let mut both = base;
        both.focused_opacity = Some(0.95);
        both.unfocused_opacity = Some(0.8);
        assert!(render(both).contains("opacity = \"0.95 0.80\""));
    }

    /// The border width is its own setting: it reaches every compositor
    /// with and without palette border colours.
    #[test]
    fn border_width_applies_without_color_sync() {
        let c = CompositorConfig { border_width: Some(3), keybinds: Vec::new(), ..Default::default() };
        let plain = ManagedConfig { borders: None, ..managed(&c) };
        let mango = emit_mango(&plain).text;
        assert!(mango.contains("border_px=3") && !mango.contains("focus_color"), "{mango}");
        let niri = emit_niri(&plain).text;
        kdl::KdlDocument::parse_v1(&niri).unwrap_or_else(|e| panic!("{e:?}\n{niri}"));
        assert!(niri.contains("focus-ring {\n        width 3\n    }") && !niri.contains("active-gradient"), "{niri}");
        assert!(emit_hyprland_conf(&plain).text.contains("border_size = 3"));
        assert!(emit_hyprland_lua(&plain).text.contains("border_size = 3"));
        // With colours, still one width line per file.
        let synced = managed(&c);
        assert_eq!(emit_mango(&synced).text.matches("border_px=3").count(), 1);
        let niri = emit_niri(&synced).text;
        assert_eq!(niri.matches("width 3").count(), 1);
        assert!(niri.contains("active-gradient"));
        // Unset leaves the user's own width alone everywhere.
        let unset = CompositorConfig { keybinds: Vec::new(), ..Default::default() };
        for target in Target::ALL {
            let text = emit(target, &ManagedConfig { borders: None, ..managed(&unset) }).text;
            assert!(!text.contains("border_px") && !text.contains("border_size") && !text.contains("focus-ring"), "{target:?}\n{text}");
        }
    }

    /// MangoWM's checker accepts every Studio option (tag rules, SceneFX
    /// parameters, focused opacity).
    #[test]
    fn mango_accepts_studio_options() {
        if std::process::Command::new("mango").arg("-v").output().is_err() {
            return;
        }
        let c = studio_full();
        let path = std::env::temp_dir().join(format!("mandara-mango-studio-{}.conf", std::process::id()));
        std::fs::write(&path, emit_mango(&managed(&c)).text).unwrap();
        let out = std::process::Command::new("mango").arg("-p").arg("-c").arg(&path).output().unwrap();
        let report = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        std::fs::remove_file(&path).ok();
        assert!(!report.contains("ERROR"), "mango -p rejected the drop-in:\n{report}");
    }

    /// When MangoWM is installed, its own checker must accept the file.
    #[test]
    fn mango_accepts_generated_file() {
        if std::process::Command::new("mango").arg("-v").output().is_err() {
            return;
        }
        let c = full_look();
        let path = std::env::temp_dir().join(format!("mandara-mango-check-{}.conf", std::process::id()));
        std::fs::write(&path, emit_mango(&managed(&c)).text).unwrap();
        let out = std::process::Command::new("mango").arg("-p").arg("-c").arg(&path).output().unwrap();
        let report = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        std::fs::remove_file(&path).ok();
        assert!(!report.contains("ERROR"), "mango -p rejected the drop-in:\n{report}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn managed<'a>(cfg: &'a CompositorConfig, existing: &[&str]) -> ManagedConfig<'a> {
        ManagedConfig {
            config: cfg,
            borders: Some(BorderColors { active: "#5eead4".into(), active_end: "#93c5fd".into(), inactive: "#1f2937".into(), urgent: "#f87171".into() }),
            existing_chords: existing.iter().map(|s| s.to_string()).collect(),
            mandara_bin: "mandara".into(),
            version: None,
            user_autostarts: false,
        }
    }

    #[test]
    fn chords_normalize() {
        assert_eq!(chord(["SUPER", "SHIFT"].into_iter(), "S"), "shift+super+s");
        assert_eq!(chord(["Mod"].into_iter(), "Space"), "super+space");
        assert_eq!(chord(["NONE"].into_iter(), "Print"), "print");
    }

    #[test]
    fn parses_existing_binds_per_compositor() {
        let mango = "bind=SUPER,space,spawn,fuzzel\nbind=CTRL+ALT,Delete,quit\n# bind=SUPER,x,nope\n";
        assert_eq!(parse_chords(Target::Mango, mango), vec!["super+space", "alt+ctrl+delete"]);
        let hypr = "bind = SUPER, Q, exec, kitty\nbindl = , XF86AudioMute, exec, x\n";
        assert_eq!(parse_chords(Target::Hyprland, hypr), vec!["super+q", "xf86audiomute"]);
        let niri = "binds {\n    Mod+T { spawn \"alacritty\"; }\n    Mod+Shift+E { quit; }\n}\nlayout { gaps 8; }\n";
        assert_eq!(parse_chords(Target::Niri, niri), vec!["super+t", "shift+super+e"]);
    }

    #[test]
    fn a_window_title_cannot_add_config_lines() {
        // Issue #10: a picked window's title is chosen by another app.
        let mut cfg = CompositorConfig::default();
        cfg.window_rules = vec![WindowRule { app_id: Some("^evil$".into()), title: Some("x\nexec = rm -rf ~\r\n, float on".into()), floating: Some(true), ..Default::default() }];
        for (name, text) in [("mango", emit_mango(&managed(&cfg, &[])).text), ("hyprland", emit_hyprland(&managed(&cfg, &[])).text), ("niri", emit_niri(&managed(&cfg, &[])).text)] {
            assert!(!text.lines().any(|l| l.trim_start().starts_with("exec")), "{name}: a title started a config line:\n{text}");
        }
        let mango = emit_mango(&managed(&cfg, &[])).text;
        let rule = mango.lines().find(|l| l.contains("app_id:^evil$")).unwrap();
        assert!(rule.contains("title:xexec = rm -rf ~. float on"), "commas can't split the rule: {rule}");
        assert_eq!(rule_value("a,b\u{7}c"), "a.bc");
        assert_eq!(kdl_str("a\nb\""), "\"a\\nb\\\"\"");
    }

    #[test]
    fn mango_output_skips_conflicts_and_uses_real_keys() {
        let cfg = CompositorConfig::default();
        let out = emit_mango(&managed(&cfg, &["super+space", "alt+ctrl+delete"]));
        assert!(out.text.contains("layer_rule=no_animation:1,no_shadow:1,no_blur:1,layer_name:mandara-bar"));
        assert!(out.text.contains("focus_color=0x5eead4ff"));
        assert!(!out.text.contains("border_px"), "width is left alone unless configured");
        let mut sized = CompositorConfig::default();
        sized.border_width = Some(3);
        assert!(emit_mango(&managed(&sized, &[])).text.contains("border_px=3"), "Mango's key is border_px");
        assert!(out.text.contains("window_rule=is_floating:1,app_id:^org\\.mandara\\.studio$"));
        assert!(out.text.contains("bind=SUPER+ALT,space,spawn_shell,mandara msg launcher toggle"));
        assert!(!out.text.contains("bind=SUPER,space,"));
        assert_eq!(out.skipped.len(), 2);
        assert!(out.text.contains("bind=NONE,XF86AudioMute,spawn_shell,mandara msg audio mute-all toggle"), "new installs: the mute key mutes all sound");
        assert!(out.text.contains("bind=NONE,XF86MonBrightnessUp,spawn_shell,mandara msg brightness up"), "laptop brightness keys work out of the box");
        assert!(out.text.contains("bind=NONE,XF86MonBrightnessDown,spawn_shell,mandara msg brightness down"));
        assert!(!emit_mango(&managed(&cfg, &["xf86monbrightnessup"])).text.contains("XF86MonBrightnessUp,spawn"), "your own brightness key wins");
        assert!(!emit_mango(&managed(&cfg, &["xf86audiomute"])).text.contains("XF86AudioMute,spawn"), "your own mute key wins");
    }

    #[test]
    fn niri_output_is_valid_kdl() {
        let mut cfg = CompositorConfig::default();
        cfg.window_rules.push(WindowRule { app_id: Some("pavucontrol".into()), floating: Some(true), unfocused_opacity: Some(0.9), ..Default::default() });
        let out = emit_niri(&managed(&cfg, &["super+t"]));
        let doc = kdl::KdlDocument::parse_v1(&out.text).unwrap_or_else(|e| panic!("invalid KDL v1: {e}\n{}", out.text));
        assert!(doc.nodes().iter().any(|n| n.name().value() == "binds"));
        assert!(out.text.contains("Mod+Space { spawn \"mandara\" \"msg\" \"launcher\" \"toggle\"; }"));
        assert!(out.text.contains("active-gradient from=\"#5eead4\""));
    }

    #[test]
    fn keysyms_and_bare_keys() {
        for (typed, sym) in [(",", "comma"), (".", "period"), ("/", "slash"), ("\\", "backslash"), ("-", "minus"), ("=", "equal"), ("[", "bracketleft"), ("]", "bracketright"), (";", "semicolon"), ("'", "apostrophe"), ("`", "grave"), ("pageup", "Prior"), ("pagedown", "Next"), ("enter", "Return"), ("f11", "F11"), ("A", "a"), ("XF86AudioMute", "XF86AudioMute"), ("space", "space")] {
            assert_eq!(keysym_for(typed), sym, "{typed}");
        }
        for ok in ["F1", "f12", "Print", "XF86AudioRaiseVolume", "Pause"] {
            assert!(bare_key_allowed(ok), "{ok}");
        }
        for typing in ["a", "1", "space", "comma", "Return", "f", "fx"] {
            assert!(!bare_key_allowed(typing), "{typing}");
        }
        let bind = |mods: &[&str], key: &str| Keybind::new(mods, key, BindAction::Spawn("true".into()));
        assert_eq!(bind(&["super"], "").problem(), Some("no key yet"));
        assert_eq!(bind(&[], "a").problem(), Some("needs Super, Ctrl or Alt"));
        assert_eq!(bind(&["shift"], "a").problem(), Some("needs Super, Ctrl or Alt"), "Shift+a types A");
        assert_eq!(bind(&[], "Print").problem(), None);
        assert_eq!(bind(&["ctrl"], "a").problem(), None);
        assert_eq!(bind(&["super"], ",").chord(), "super+comma");
        assert_eq!(chord(["mod5"].into_iter(), "x"), "mod5+x", "rare modifiers are not dropped");
    }

    /// Binds a Studio capture can leave behind never reach a drop-in as
    /// broken syntax: empty keys, bare letters, punctuation and
    /// out-of-order or aliased modifiers.
    #[test]
    fn unusable_binds_are_skipped_and_punctuation_is_spelled_out() {
        let spawn = |mods: &[&str], key: &str| Keybind::new(mods, key, BindAction::Spawn("notify-send hi".into()));
        let cfg = CompositorConfig { keybinds: vec![spawn(&["super"], ""), spawn(&[], "q"), spawn(&["shift", "win"], ","), spawn(&["super"], "1"), spawn(&[], "F9")], ..Default::default() };
        let mango = emit_mango(&managed(&cfg, &[]));
        assert!(mango.text.contains("bind=SUPER+SHIFT,comma,spawn_shell,notify-send hi"), "{}", mango.text);
        assert!(mango.text.contains("bind=NONE,F9,spawn_shell,"));
        assert!(!mango.text.lines().any(|l| l.starts_with("bind=") && (l.contains(",,") || l.starts_with("bind=NONE,q"))), "{}", mango.text);
        assert_eq!(mango.skipped.iter().filter(|n| n.contains("no key yet") || n.contains("needs Super")).count(), 2, "{:?}", mango.skipped);
        let niri = emit_niri(&managed(&cfg, &[]));
        kdl::KdlDocument::parse_v1(&niri.text).unwrap_or_else(|e| panic!("invalid KDL: {e:?}\n{}", niri.text));
        assert!(niri.text.contains("Mod+Shift+Comma {"), "{}", niri.text);
        assert!(niri.text.contains("Mod+1 {") && niri.text.contains("    F9 {"), "{}", niri.text);
        let hypr = emit_hyprland_conf(&managed(&cfg, &[]));
        assert!(hypr.text.contains("bind = SUPER SHIFT, comma, exec, notify-send hi"), "{}", hypr.text);
        assert!(hypr.text.contains("bind = , F9, exec,"), "{}", hypr.text);
        assert!(!hypr.text.contains("bind = , q,") && !hypr.text.contains("bind = SUPER, , "), "{}", hypr.text);
        let lua = emit_hyprland_lua(&managed(&cfg, &[]));
        assert!(lua.text.contains("\"SUPER + SHIFT + comma\""), "{}", lua.text);
        // MangoWM has no Mod5: such a bind is skipped there, kept elsewhere.
        let altgr = CompositorConfig { keybinds: vec![spawn(&["mod5"], "e")], ..Default::default() };
        assert!(emit_mango(&managed(&altgr, &[])).skipped.iter().any(|n| n.contains("can't bind")));
        assert!(emit_hyprland_conf(&managed(&altgr, &[])).text.contains("bind = MOD5, e, exec"));
    }

    /// Hyprland's `$mainMod` variables and `SUPER_SHIFT` spellings count as
    /// the user's binds, so Mandara never writes a duplicate.
    #[test]
    fn hyprland_conflicts_follow_variables_and_joined_modifiers() {
        let text = "$mainMod = SUPER\n$mainModShift = SUPER SHIFT # both\nbind = $mainMod, Q, exec, kitty\nbind = SUPER_SHIFT, E, exit\nbind = $mainModShift, comma, exec, x\nbindel = , XF86AudioRaiseVolume, exec, y\nbind = CTRLALT, Delete, exec, z\n";
        assert_eq!(parse_chords(Target::Hyprland, text), vec!["super+q", "shift+super+e", "shift+super+comma", "xf86audioraisevolume", "alt+ctrl+delete"]);
        let mango = "bind=SUPER,comma,focusmon,left\n";
        assert_eq!(parse_chords(Target::Mango, mango), vec!["super+comma"]);
        // A Studio bind on Super+, is recognised as that same chord.
        let cfg = CompositorConfig { keybinds: vec![Keybind::new(&["super"], ",", BindAction::Spawn("x".into()))], ..Default::default() };
        let out = emit_mango(&managed(&cfg, &["super+comma"]));
        assert!(out.skipped.iter().any(|n| n.contains("already bound")), "{:?}", out.skipped);
    }

    #[test]
    fn hyprland_output() {
        let cfg = CompositorConfig::default();
        let out = emit_hyprland_conf(&managed(&cfg, &[]));
        assert!(out.text.contains("layerrule = match:namespace ^(mandara-bar)$, no_anim on"));
        assert!(out.text.contains("col.active_border = rgba(5eead4ff) rgba(93c5fdff) 45deg"));
        assert!(out.text.contains("bind = SUPER, space, exec, mandara msg launcher toggle"));
        assert!(out.text.contains("windowrule = match:class ^org\\.mandara\\.studio$, float on"));
    }

    #[test]
    fn include_install_is_append_only_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.conf");
        std::fs::write(&file, "blur=1\nbind=SUPER,q,killclient").unwrap();
        assert!(install_include_at(Target::Mango, &file).unwrap());
        let after = std::fs::read_to_string(&file).unwrap();
        assert!(after.starts_with("blur=1\nbind=SUPER,q,killclient\n"), "user content must be preserved verbatim");
        assert!(after.trim_end().ends_with(&Target::Mango.include_line()));
        assert!(!install_include_at(Target::Mango, &file).unwrap(), "second install is a no-op");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), after);
        let backup = dir.path().join("config.conf.mandara-backup");
        assert_eq!(std::fs::read_to_string(backup).unwrap(), "blur=1\nbind=SUPER,q,killclient");
    }
}
