//! Compositor-managed configuration and its non-destructive emitters.
//!
//! Mandara never rewrites the user's own compositor config. Rules, keybinds
//! and live border colors go to `~/.config/mandara/generated/{mango.conf,
//! niri.kdl, hyprland.conf, umbriel.toml, sway.conf, river.conf}`, and the user's main file only
//! ever gains one include line (installed on request, verified at startup):
//!
//! * MangoWM — `source_optional=~/.config/mandara/generated/mango.conf`
//! * Niri — `include optional=true "<abs path>/niri.kdl"` (26.04+; plain
//!   `include` on 25.11, the first version with includes)
//! * Hyprland — `source = ~/.config/mandara/generated/hyprland.conf`, or on
//!   Lua Hyprland (0.57+ only reads `hyprland.lua`)
//!   `pcall(dofile, "<abs path>/hyprland.lua")`
//! * Umbriel — `"<abs path>/umbriel.toml"` in `[include.optional] files`
//!   (added to the list when the user already has one; see
//!   [`umbriel_add_include`])
//! * Sway — `include <abs path>/sway.conf` (sway reads its config in order,
//!   so the line goes at the end; see [`sway_include_line`])
//! * River — none: its config is an executable `init` program, which
//!   Mandara never edits. Mandara's settings are River commands it sends
//!   over `river-control` when the shell starts and whenever they change
//!   ([`river_commands`]; `river.conf` keeps a record of them). The one
//!   line a user adds is the one that starts Mandara from `init`
//!   ([`river_spawn_line`]).
//! * KWin, Aqueous, labwc and COSMIC — none ([`Target::self_configured`]):
//!   their settings stay in Plasma System Settings, Aqueous's own TOML
//!   files, labwc's `rc.xml` and COSMIC Settings.
//!
//! Keybinds that collide with binds already in the user's config are
//! skipped and reported instead of silently overriding them.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::{mango_keys, paths};

mod sway_read;
mod glob;
pub mod shortcuts;
pub(crate) use sway_read::binds as sway_config_binds;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    Mango,
    Niri,
    Hyprland,
    Umbriel,
    Sway,
    River,
    #[serde(rename = "kwin")]
    KWin,
    Aqueous,
    Labwc,
    Cosmic,
}

impl Target {
    pub const ALL: [Target; 10] = [Target::Mango, Target::Niri, Target::Hyprland, Target::Umbriel, Target::Sway, Target::River, Target::KWin, Target::Aqueous, Target::Labwc, Target::Cosmic];

    /// This compositor's column in the `[MangoWM, Niri, Hyprland, Umbriel,
    /// Sway, River, KWin, Aqueous, labwc, COSMIC]` support tables ([`RULE_OPTIONS`], `input::INPUT_OPTIONS`).
    pub const fn index(self) -> usize {
        match self {
            Target::Mango => 0,
            Target::Niri => 1,
            Target::Hyprland => 2,
            Target::Umbriel => 3,
            Target::Sway => 4,
            Target::River => 5,
            Target::KWin => 6,
            Target::Aqueous => 7,
            Target::Labwc => 8,
            Target::Cosmic => 9,
        }
    }

    /// Compositors whose settings stay in their own tools, so Mandara
    /// writes no drop-in for them: KWin (Plasma System Settings),
    /// Aqueous (its hot-reloaded TOML files in `~/.config/aqueous`, which
    /// document no include directive), labwc (`rc.xml` has no include
    /// Mandara's drop-in could use safely) and COSMIC (COSMIC Settings,
    /// stored in its own files under `~/.config/cosmic`). Mandara drives
    /// them over their native protocols only.
    pub const fn self_configured(self) -> bool {
        matches!(self, Target::KWin | Target::Aqueous | Target::Labwc | Target::Cosmic)
    }

    /// Where a [`Target::self_configured`] compositor keeps its settings
    /// (an i18n key, shown with `tr`), `None` for the others.
    pub const fn settings_elsewhere(self) -> Option<&'static str> {
        match self {
            Target::KWin => Some(KWIN_SETTINGS),
            Target::Aqueous => Some(AQUEOUS_SETTINGS),
            Target::Labwc => Some(LABWC_SETTINGS),
            Target::Cosmic => Some(COSMIC_SETTINGS),
            _ => None,
        }
    }

    /// The compositor a name on the command line means (`mango`, `niri`,
    /// `hyprland`, `umbriel`, `sway`, and the usual aliases).
    pub fn parse(name: &str) -> Option<Target> {
        match name.trim().to_ascii_lowercase().as_str() {
            "mango" | "mangowm" | "mangowc" => Some(Target::Mango),
            "niri" => Some(Target::Niri),
            "hyprland" | "hypr" => Some(Target::Hyprland),
            "umbriel" => Some(Target::Umbriel),
            "sway" | "swaywm" => Some(Target::Sway),
            "river" | "river-classic" => Some(Target::River),
            "kwin" | "kde" | "plasma" => Some(Target::KWin),
            "aqueous" => Some(Target::Aqueous),
            "labwc" => Some(Target::Labwc),
            "cosmic" | "cosmic-comp" => Some(Target::Cosmic),
            _ => None,
        }
    }

    /// The name [`Target::parse`] takes for this compositor, lowercase.
    pub const fn id(self) -> &'static str {
        match self {
            Target::Mango => "mango",
            Target::Niri => "niri",
            Target::Hyprland => "hyprland",
            Target::Umbriel => "umbriel",
            Target::Sway => "sway",
            Target::River => "river",
            Target::KWin => "kwin",
            Target::Aqueous => "aqueous",
            Target::Labwc => "labwc",
            Target::Cosmic => "cosmic",
        }
    }

    /// The compositor's name as people know it.
    pub const fn name(self) -> &'static str {
        match self {
            Target::Mango => "MangoWM",
            Target::Niri => "Niri",
            Target::Hyprland => "Hyprland",
            Target::Umbriel => "Umbriel",
            Target::Sway => "Sway",
            Target::River => "River",
            Target::KWin => "KWin",
            Target::Aqueous => "Aqueous",
            Target::Labwc => "labwc",
            Target::Cosmic => "COSMIC",
        }
    }

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
            Target::Umbriel => "umbriel.toml",
            Target::Sway => "sway.conf",
            // Not read by River: the commands Mandara sent, for the record
            // (and to take its own binds and rules back on the next sync).
            Target::River => "river.conf",
            Target::KWin => "kwin.conf",
            // Never written: Aqueous reads only its own files.
            Target::Aqueous => "aqueous.conf",
            Target::Labwc => "labwc.conf",
            Target::Cosmic => "cosmic.conf",
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
            // Umbriel reads `$XDG_CONFIG_HOME/umbriel/config.toml` first.
            Target::Umbriel => config.join("umbriel/config.toml"),
            Target::Sway => sway_user_config(&config, &paths::home()),
            // River runs `$XDG_CONFIG_HOME/river/init` at start-up.
            Target::River => config.join("river/init"),
            Target::KWin => config.join("kwinrc"),
            // Bindings, actions, workspaces and global policy (next to its
            // outputs, layout, input and rules files).
            Target::Aqueous => config.join("aqueous/wm.toml"),
            // labwc's main config (next to its autostart, environment and
            // menu files).
            Target::Labwc => config.join("labwc/rc.xml"),
            // COSMIC keeps its compositor settings as one RON file per
            // value in this directory.
            Target::Cosmic => config.join("cosmic/com.system76.CosmicComp/v1"),
        }
    }

    /// The single line Mandara adds to the user's config.
    pub fn include_line(self) -> String {
        match self {
            Target::Mango => "source_optional=~/.config/mandara/generated/mango.conf".into(),
            // An absolute path works from niri 25.11 (`~/` needs 26.04).
            Target::Niri => niri_include_line(&self.generated_file().display().to_string(), installed_version(Target::Niri)),
            Target::Hyprland if Self::hyprland_lua() => {
                hyprland_lua_include(&self.generated_file().display().to_string())
            }
            Target::Hyprland => "source = ~/.config/mandara/generated/hyprland.conf".into(),
            // A table rather than a line: shown as the two lines to add when
            // the config has no `[include.optional]` yet.
            Target::Umbriel => format!("[include.optional]\nfiles = [{}]", toml_str(&self.generated_file().display().to_string())),
            Target::Sway => sway_include_line(&self.generated_file().display().to_string()),
            Target::River => river_spawn_line(&paths::self_exe().display().to_string()),
            Target::KWin | Target::Aqueous | Target::Labwc | Target::Cosmic => String::new(),
        }
    }
}

/// [`Target::settings_elsewhere`] for KWin.
pub const KWIN_SETTINGS: &str = mandara_i18n::tk("KWin settings are managed in Plasma System Settings");
/// [`Target::settings_elsewhere`] for Aqueous.
pub const AQUEOUS_SETTINGS: &str = mandara_i18n::tk("Aqueous settings stay in its own files in ~/.config/aqueous");
/// [`Target::settings_elsewhere`] for labwc.
pub const LABWC_SETTINGS: &str = mandara_i18n::tk("labwc settings stay in ~/.config/labwc (rc.xml, autostart)");
/// [`Target::settings_elsewhere`] for COSMIC.
pub const COSMIC_SETTINGS: &str = mandara_i18n::tk("COSMIC settings are managed in COSMIC Settings");

/// The line that starts Mandara from River's `init` (which Mandara never
/// edits: the user adds it; `riverctl spawn` runs it through `/bin/sh -c`).
pub fn river_spawn_line(mandara: &str) -> String {
    format!("riverctl spawn {}", shell_quote(mandara))
}

/// `s` as one POSIX shell word (single quotes when it needs any).
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+=:,@%".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Shell words of one line: single and double quotes and backslashes
/// undone, a `#` comment dropped (enough for `riverctl …` lines).
pub fn shell_words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                let w = word.get_or_insert_with(String::new);
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    w.push(c);
                }
            }
            '"' => {
                let w = word.get_or_insert_with(String::new);
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => w.extend(chars.next()),
                        c => w.push(c),
                    }
                }
            }
            '\\' => word.get_or_insert_with(String::new).extend(chars.next()),
            '#' if word.is_none() => break,
            c if c.is_whitespace() => out.extend(word.take()),
            c => word.get_or_insert_with(String::new).push(c),
        }
    }
    out.extend(word);
    out
}

/// The config file sway reads, in sway's own search order
/// (`~/.sway/config`, `$XDG_CONFIG_HOME/sway/config`, `~/.i3/config`,
/// `$XDG_CONFIG_HOME/i3/config`); `$XDG_CONFIG_HOME/sway/config` when none
/// exists yet (sway then reads the system-wide [`SWAY_SYSTEM_CONFIG`],
/// which Mandara never edits).
pub fn sway_user_config(config_dir: &Path, home: &Path) -> PathBuf {
    let candidates = [home.join(".sway/config"), config_dir.join("sway/config"), home.join(".i3/config"), config_dir.join("i3/config")];
    candidates.iter().find(|p| p.is_file()).cloned().unwrap_or_else(|| config_dir.join("sway/config"))
}

/// The system-wide config sway falls back to. When the user has no config
/// of their own, `mandara setup` starts theirs as a copy of this one (what
/// sway's documentation asks for) before adding the include line.
pub const SWAY_SYSTEM_CONFIG: &str = "/etc/sway/config";

/// Sway's include line for `path`. Sway expands include paths like a shell
/// word (`wordexp`) and doesn't honour quotes, so spaces and shell
/// characters are escaped with a backslash (checked with `sway --validate`).
pub fn sway_include_line(path: &str) -> String {
    format!("include {}", sway_word(path))
}

/// `s` as one shell word for sway's `include` (backslash escapes).
fn sway_word(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).fold(String::new(), |mut out, c| {
        if !(c.is_ascii_alphanumeric() || "/._-+~@,:=%".contains(c)) {
            out.push('\\');
        }
        out.push(c);
        out
    })
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
/// `Hyprland --version`, `umbriel --version`), probed once per process.
/// Lets `mandara setup` pick the right dialect even when it runs outside
/// that compositor.
pub fn installed_version(target: Target) -> Option<(u32, u32)> {
    static CACHE: [std::sync::OnceLock<Option<(u32, u32)>>; Target::ALL.len()] = [const { std::sync::OnceLock::new() }; Target::ALL.len()];
    let program = match target {
        Target::Mango => return None,
        Target::Niri => "niri",
        Target::Hyprland => "Hyprland",
        Target::Umbriel => "umbriel",
        // `sway version 1.12`.
        Target::Sway => "sway",
        // `river -version`: `0.3.17` (handled below).
        Target::River => "river",
        Target::KWin => "kwin_wayland",
        // `aqueous -version` (its `main.zig`): `aqueous 0.4.0 +xwayland`.
        Target::Aqueous => "aqueous",
        // `labwc 0.20.2 (+xwayland …) wlroots-0.20.2`.
        Target::Labwc => "labwc",
        // `cosmic-comp 1.0.0 (git commit …)`.
        Target::Cosmic => "cosmic-comp",
    };
    let slot = &CACHE[target.index()];
    *slot.get_or_init(|| {
        let flag = if matches!(target, Target::River | Target::Aqueous) { "-version" } else { "--version" };
        let out = std::process::Command::new(program).arg(flag).stdin(std::process::Stdio::null()).output().ok()?;
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
/// (`[MangoWM, Niri, Hyprland, Umbriel, Sway, River, KWin, Aqueous, labwc, COSMIC]`). Studio greys out the rest.
/// Umbriel's `global` is its `default_pinned` (above other windows on every
/// workspace of the output, like Hyprland's `pin`); Sway's is `sticky` (a
/// floating window shown on every workspace of its output); River's is a
/// `tags` rule putting new windows on every tag. Sway and River have no
/// per-focus opacity, blur or shadows.
pub const RULE_OPTIONS: &[(&str, [bool; Target::ALL.len()])] = &[
    ("floating", [true, true, true, true, true, true, false, false, false, false]),
    ("global", [true, false, true, true, true, true, false, false, false, false]),
    ("overlay", [true, false, false, false, false, false, false, false, false, false]),
    ("focused_opacity", [true, true, true, true, false, false, false, false, false, false]),
    ("unfocused_opacity", [true, true, true, true, false, false, false, false, false, false]),
    ("no_blur", [true, false, true, true, false, false, false, false, false, false]),
    ("no_shadow", [true, true, true, true, false, false, false, false, false, false]),
    ("confine_pointer", [true, false, false, true, false, false, false, false, false, false]),
    ("force_tiled_state", [true, false, false, false, false, false, false, false, false, false]),
];

/// Whether `target`'s emitter writes window-rule option `option`.
pub fn rule_option_supported(target: Target, option: &str) -> bool {
    RULE_OPTIONS.iter().any(|(name, s)| *name == option && s[target.index()])
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
    #[serde(alias = "exec")]
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
            .filter(|(n, _)| set.contains(n))
            .map(|(_, names)| {
                let name = names[target.index()];
                (!name.is_empty()).then_some(name)
            })
            .collect()
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

pub(crate) fn normalize_mod(m: &str) -> Option<&'static str> {
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
/// spells them: `(normalized, [MangoWM, Niri, Hyprland, Umbriel, Sway, River, KWin, Aqueous, labwc, COSMIC])`.
/// Aqueous, labwc and COSMIC binds stay in their own settings, which
/// Mandara never writes: their columns only name the modifiers for display.
/// Umbriel documents Super, Ctrl, Alt and Shift only (its `Mod` follows
/// `general.mod_key`, so Mandara spells Super out). Sway takes xkb's
/// modifier names (`Mod4`, `Ctrl`, `Alt`, `Shift`, `Mod3`, `Mod5`) and
/// rejects `Hyper`; River takes `Super`, `Control`, `Alt`, `Shift`, `Mod3`
/// and `Mod5` (`riverctl(1)`).
const MOD_NAMES: &[(&str, [&str; Target::ALL.len()])] = &[
    ("super", ["SUPER", "Mod", "SUPER", "Super", "Mod4", "Super", "Meta", "Super", "Super", "Super"]),
    ("ctrl", ["CTRL", "Ctrl", "CTRL", "Ctrl", "Ctrl", "Control", "Ctrl", "Ctrl", "Ctrl", "Ctrl"]),
    ("alt", ["ALT", "Alt", "ALT", "Alt", "Alt", "Alt", "Alt", "Alt", "Alt", "Alt"]),
    ("shift", ["SHIFT", "Shift", "SHIFT", "Shift", "Shift", "Shift", "Shift", "Shift", "Shift", "Shift"]),
    ("hyper", ["HYPER", "Mod3", "MOD3", "", "Mod3", "Mod3", "Hyper", "", "", ""]),
    ("mod5", ["", "ISO_Level3_Shift", "MOD5", "", "Mod5", "Mod5", "Mod5", "", "", ""]),
];

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
    s.push_str(&mango_keys::canonical_keysym(key));
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
    /// The hardware-key defaults `keybinds` was written for (see
    /// [`crate::upgrade`]): missing in older files, so the media and
    /// brightness keys added since reach existing configs too.
    #[serde(default)]
    pub keybinds_version: u32,
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
    /// Config doctor: check the compositor's config at start-up (only when
    /// it or the compositor changed) and send one notification about new
    /// problems.
    pub doctor_notify: bool,
}

impl Default for CompositorConfig {
    fn default() -> Self {
        Self {
            manage: true,
            window_rules: vec![
                WindowRule { app_id: Some("^org\\.mandara\\.studio$".into()), floating: Some(true), ..Default::default() }],
            tag_rules: Vec::new(),
            keybinds: default_keybinds(),
            keybinds_version: crate::upgrade::KEYBIND_DEFAULTS_VERSION,
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
            doctor_notify: true,
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
        Keybind::new(&["alt"], "v", M("panel toggle clipboard".into())),
        Keybind::new(&[], "Print", M("screenshot screen".into())),
        Keybind::new(&["ctrl"], "Print", M("screenshot region".into())),
        Keybind::new(&["alt"], "Print", M("screenshot window".into())),
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
        // Media keys (laptop F-keys such as F4–F6 send these) drive the
        // current player; skipped when your config binds them.
        Keybind::new(&[], "XF86AudioPlay", M("media play-pause".into())),
        Keybind::new(&[], "XF86AudioPause", M("media play-pause".into())),
        Keybind::new(&[], "XF86AudioNext", M("media next".into())),
        Keybind::new(&[], "XF86AudioPrev", M("media previous".into())),
        Keybind::new(&[], "XF86AudioMicMute", M("audio mic mute".into())),
    ]
}

/// The hardware keys among [`default_keybinds`] (volume, brightness,
/// media): the ones an older config is given when it lacks them.
pub fn hardware_keybinds() -> Vec<Keybind> {
    default_keybinds().into_iter().filter(|b| b.mods.is_empty() && b.key.starts_with("XF86")).collect()
}

/// Defaults added after release, with the key-defaults version that
/// brought each: an older config is given the ones newer than its own
/// version whose chord it doesn't already use.
pub fn added_keybinds() -> Vec<(u32, Keybind)> {
    use BindAction::Mandara as M;
    vec![
        (2, Keybind::new(&["alt"], "v", M("panel toggle clipboard".into()))),
        (2, Keybind::new(&[], "Print", M("screenshot screen".into()))),
        (2, Keybind::new(&["ctrl"], "Print", M("screenshot region".into()))),
        (2, Keybind::new(&["alt"], "Print", M("screenshot window".into()))),
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
    /// Window open / close / move / workspace animations (Studio → Motion;
    /// only where [`crate::window_motion::supported`]).
    pub windows: crate::window_motion::WindowMotion,
}

impl CompositorLook {
    /// Window animations to write: none while animations are switched off.
    fn window_motion(&self) -> Option<&crate::window_motion::WindowMotion> {
        (self.animations != Some(false)).then_some(&self.windows)
    }
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
    if target == Target::KWin { return crate::kwin::autostart_installed(); }
    // Mandara writes nothing Aqueous, labwc or COSMIC reads, so it starts
    // nothing there (users add it to their own startup settings).
    if matches!(target, Target::Aqueous | Target::Labwc | Target::Cosmic) { return false; }
    // The main file and every file it sources / includes (never Mandara's
    // own drop-in).
    let texts: Vec<String> = crate::conflicts::config_files(target).iter().filter_map(|f| std::fs::read_to_string(f).ok()).collect();
    if target == Target::River {
        return texts.iter().any(|t| river_starts_mandara(t));
    }
    if target == Target::Umbriel {
        // `general.autostart` entries (an array, possibly over several
        // lines) rather than keyword lines.
        return texts.iter().any(|t| {
            crate::conflicts::umbriel_autostarts(t).iter().any(|(_, cmd)| {
                    cmd.split_whitespace().any(|w| w == "mandara" || w.ends_with("/mandara"))
                })
        });
    }
    starts_mandara(&texts.join("\n"))
}

/// Whether config text has an autostart line launching `mandara` (a bind
/// running `mandara msg …` is not one).
pub fn starts_mandara(text: &str) -> bool {
    text.lines().map(str::trim).filter(|l| !l.starts_with('#') && !l.starts_with("//") && !l.starts_with("--")).any(|l| {
        let starter = l.starts_with("exec-once") || l.starts_with("exec_once") || l.starts_with("spawn-at-startup") || l.starts_with("spawn-sh-at-startup") || l.contains("hl.exec_cmd(") || l.starts_with("exec=") || l.starts_with("exec ") || l.starts_with("exec_always ");
        starter && l.split(|c: char| {
                    c.is_whitespace() || c == '"' || c == '\'' || c == '=' || c == ',' || c == '('
                }).any(|w| w == "mandara" || w.ends_with("/mandara"))
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
        // As the compositor sees it: on MangoWM, two key names on one key
        // (Play and Pause) are one bind.
        let chord = target_chord(target, bind);
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

/// Shell surfaces whose rules were checked only by MangoWM's and Niri's
/// own checkers so far (`mango_itself_accepts_the_drop_in`,
/// `niri_itself_accepts_the_drop_in`): emitted for those two only, until
/// Hyprland's and Umbriel's checkers have accepted them too.
pub const MANGO_NIRI_SURFACES: &[&str] = &["mandara-floating"];

/// Shell surfaces whose rules only MangoWM's own checker has accepted so
/// far (`mango_itself_accepts_the_drop_in`): the wallpaper transition's
/// temporary surface. Without a rule the other compositors at most fade it
/// in and out, which shows nothing: its first and last frames are exactly
/// the desktop under it.
pub const MANGO_SURFACES: &[&str] = &["mandara-wallpaper-transition"];

/// `~/.config/mandara/generated/mango.conf`
pub fn emit_mango(cfg: &ManagedConfig) -> Emitted {
    let c = cfg.config;
    let mut s = String::new();
    let _ = writeln!(s, "# {HEADER}\n");
    let _ = writeln!(s, "# Shell surfaces: no compositor animation or shadow (Mandara animates itself).");
    for ns in SHELL_SURFACES.iter().chain(MANGO_NIRI_SURFACES).chain(MANGO_SURFACES) {
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
    if let Some(m) = l.window_motion() {
        let lines = crate::window_motion::mango(m);
        if !lines.is_empty() {
            let _ = writeln!(s, "{lines}");
        }
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
    let _ = writeln!(s, "\n# Mandara keybinds");
    for k in binds {
        let mods = if k.mods.is_empty() { "NONE".to_string() } else { k.mods.join("+") };
        let _ = writeln!(s, "bind={mods},{},spawn_shell,{}", k.key, k.bind.action.shell_command_with(&cfg.mandara_bin));
    }
    for note in &skipped {
        let _ = writeln!(s, "# {note}");
    }
    let _ = writeln!(s, "# End Mandara keybinds");
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
    for ns in SHELL_SURFACES.iter().chain(MANGO_NIRI_SURFACES) {
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
    if let Some(m) = l.window_motion() {
        s.push_str(&crate::window_motion::niri(m));
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
    let superseded = |name: &str| {
        c.outputs.iter().any(|d| d.has_settings() && d.name.trim() == name.trim())
    };
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
    let _ = writeln!(s, "\n// Mandara keybinds");
    if !binds.is_empty() {
        let _ = writeln!(s, "\nbinds {{");
        for k in binds {
            let key = if k.key.len() == 1 { k.key.to_ascii_uppercase() } else { capitalize(&k.key) };
            let chord = if k.mods.is_empty() { key } else { format!("{}+{key}", k.mods.join("+")) };
            // A bare identifier unless KDL needs quotes (a lone digit key).
            let chord = if chord.starts_with(|c: char| c.is_ascii_digit()) { kdl_str(&chord) } else { chord };
            let argv: Vec<String> = match &k.bind.action {
                BindAction::Mandara(words) => [cfg.mandara_bin.as_str(), "msg"].iter().map(|w| kdl_str(w)).chain(words.split_whitespace().map(kdl_str)).collect(),
                BindAction::Spawn(cmd) => vec![kdl_str("sh"), kdl_str("-c"), kdl_str(cmd)],
            };
            let _ = writeln!(s, "    {chord} {{ spawn {}; }}", argv.join(" "));
        }
        let _ = writeln!(s, "}}");
    }
    for note in &skipped {
        let _ = writeln!(s, "// {note}");
    }
    let _ = writeln!(s, "// End Mandara keybinds");
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
    // Window animations are written only in the Lua format: no Hyprland
    // that reads this file was available to check them with.
    if l.window_motion().is_some_and(|m| m.style != crate::window_motion::WindowStyle::Compositor) {
        let _ = writeln!(s, "\n# Window animations from Studio need Hyprland 0.55 or newer (Lua config); skipped here.");
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
    let _ = writeln!(s, "\n# Mandara keybinds");
    for k in binds {
        let _ = writeln!(s, "bind = {}, {}, exec, {}", k.mods.join(" "), k.key, k.bind.action.shell_command_with(&cfg.mandara_bin));
    }
    for note in &skipped {
        let _ = writeln!(s, "# {note}");
    }
    let _ = writeln!(s, "# End Mandara keybinds");
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
    let lua_value = |v: &str| {
        if v == "true" || v == "false" || v.parse::<f64>().is_ok() { v.to_string() } else { lua_str(v) }
    };
    let lua_fields = |section: &str| {
        layout.iter().filter(|(sec, ..)| *sec == section).map(|(_, k, v)| format!("{k} = {}", lua_value(v))).collect::<Vec<_>>()
    };
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
    if let Some(m) = l.window_motion() {
        s.push_str(&crate::window_motion::hyprland_lua(m));
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
    let _ = writeln!(s, "\n-- Mandara keybinds");
    for k in binds {
        let chord = k.mods.iter().copied().chain(std::iter::once(k.key.as_str())).collect::<Vec<_>>().join(" + ");
        let _ = writeln!(s, "pcall(hl.bind, {}, hl.dsp.exec_cmd({}))", lua_str(&chord), lua_str(&k.bind.action.shell_command_with(&cfg.mandara_bin)));
    }
    for note in &skipped {
        let _ = writeln!(s, "-- {note}");
    }
    let _ = writeln!(s, "-- End Mandara keybinds");
    Emitted { text: s, skipped }
}

/// A TOML basic string (`"…"`), escaped per the TOML spec. Values come from
/// other apps too (a picked window's title), so control characters are
/// escaped rather than written raw.
pub fn toml_str(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Umbriel's default `layout.gap` (logical px): the edge gap Studio's outer
/// gap is measured against when the inner gap is left alone.
const UMBRIEL_DEFAULT_GAP: u32 = 8;

/// Umbriel's hot-corner tables (`[hot_corners.<corner>]`).
const UMBRIEL_HOT_CORNERS: [&str; 4] = ["top_left", "top_right", "bottom_left", "bottom_right"];

/// A colour as Umbriel writes it (`#RRGGBBAA`).
fn umbriel_color(hex: &str) -> String {
    format!("#{}", hex_bare(hex).to_ascii_uppercase())
}

/// One `[[window_rule]]` (Umbriel), from `match.*` selectors and settings.
fn umbriel_window_rule(s: &mut String, matchers: &[(&str, String)], settings: &[(&str, String)]) {
    if settings.is_empty() {
        return;
    }
    let _ = writeln!(s, "\n[[window_rule]]");
    for (k, v) in matchers {
        let _ = writeln!(s, "match.{k} = {v}");
    }
    for (k, v) in settings {
        let _ = writeln!(s, "{k} = {v}");
    }
}

/// `~/.config/mandara/generated/umbriel.toml`, Umbriel's TOML dialect
/// (documented keys only; checked with `umbriel config validate` where
/// Umbriel is installed). Umbriel applies the including file *after* its
/// includes, so anything the user's own `config.toml` sets wins over this
/// drop-in; rule lists (`[[window_rule]]`, `[[layer_rule]]`) are collected
/// from both, and `[keybinds]` merge by chord.
pub fn emit_umbriel(cfg: &ManagedConfig) -> Emitted {
    let c = cfg.config;
    let l = &c.look;
    let mut s = String::new();
    let _ = writeln!(s, "# {HEADER}");
    let _ = writeln!(s, "# Umbriel applies your config.toml after this file: a value you set there wins over Mandara's.");
    if cfg.autostart() {
        // A plain array: replaced, not merged, when your config.toml sets
        // its own `general.autostart`.
        let _ = writeln!(s, "\n[general]\nautostart = [{}]", toml_str(&cfg.mandara_bin));
    }
    let mut layout = String::new();
    if let Some(g) = l.gaps_inner {
        let _ = writeln!(layout, "gap = {g}");
    }
    if !layout.is_empty() {
        let _ = write!(s, "\n# Window look (Studio → Compositor)\n[layout]\n{layout}");
    }
    // One gap value (between windows and to the edges); struts add (or,
    // negative, remove) the difference at the screen edges.
    if let Some(outer) = l.gaps_outer {
        let d = outer as i64 - l.gaps_inner.unwrap_or(UMBRIEL_DEFAULT_GAP) as i64;
        let _ = writeln!(s, "\n[layout.struts]\nleft = {d}\nright = {d}\ntop = {d}\nbottom = {d}");
    }
    let mut appearance = String::new();
    if let Some(w) = c.border_width {
        let _ = writeln!(appearance, "border_width = {w}");
    }
    if let Some(r) = l.rounding {
        let _ = writeln!(appearance, "corner_radius = {r}");
    }
    if !appearance.is_empty() {
        let _ = write!(s, "\n[appearance]\n{appearance}");
    }
    if let Some(v) = l.blur {
        let _ = writeln!(s, "\n[appearance.blur]\nenabled = {v}");
    }
    if let Some(v) = l.shadows {
        let _ = writeln!(s, "\n[appearance.shadow]\nenabled = {v}");
    }
    if let Some(v) = l.animations {
        let _ = writeln!(s, "\n[animation]\nenabled = {v}");
    }
    if let Some(bc) = &cfg.borders {
        // Umbriel borders take one colour each (no gradient, no urgent colour).
        let _ = writeln!(s, "\n# Live border colors from the active Mandara palette\n[colors.border]\nfocused = {}\nunfocused = {}", toml_str(&umbriel_color(&bc.active)), toml_str(&umbriel_color(&bc.inactive)));
    }
    if c.disable_builtin_hot_corners {
        let _ = writeln!(s, "\n# Mandara's hot corners replace Umbriel's");
        for corner in UMBRIEL_HOT_CORNERS {
            let _ = writeln!(s, "[hot_corners.{corner}]\nenabled = false");
        }
    }
    s.push_str(&crate::input::emit_umbriel(&c.input));
    s.push_str(&crate::outputs::emit_outputs(Target::Umbriel, &c.outputs, false));
    let (binds, skipped) = split_binds(cfg, Target::Umbriel);
    let _ = writeln!(s, "\n# Mandara keybinds");
    if !binds.is_empty() {
        let _ = writeln!(s, "[keybinds]");
    }
    for k in binds {
        let chord = k.mods.iter().copied().chain(std::iter::once(k.key.as_str())).collect::<Vec<_>>().join("+");
        let _ = writeln!(s, "{} = {}", toml_str(&chord), toml_str(&format!("spawn {}", k.bind.action.shell_command_with(&cfg.mandara_bin))));
    }
    let _ = writeln!(s, "# End Mandara keybinds");
    // Shell surfaces: Mandara draws its own glass (layer blur is off by
    // default; this states it). The lock surface keeps the compositor's blur.
    let namespaces: Vec<String> = SHELL_SURFACES.iter().map(|ns| toml_str(&format!("^{ns}$"))).collect();
    let _ = writeln!(s, "\n# Shell surfaces (Mandara draws its own glass)\n[[layer_rule]]\nmatch.namespace = [{}]\nblur = false", namespaces.join(", "));
    let _ = writeln!(s, "\n[[layer_rule]]\nmatch.namespace = {}\nblur = true", toml_str("^mandara-lock$"));
    if l.blur == Some(true) {
        // `appearance.blur.enabled` is only the master switch: windows opt in.
        umbriel_window_rule(&mut s, &[], &[("blur", "true".into())]);
    }
    for (focused, o) in [(true, l.active_opacity), (false, l.inactive_opacity)] {
        if let Some(o) = o {
            umbriel_window_rule(&mut s, &[("is_focused", focused.to_string())], &[("opacity", format!("{:.2}", o.clamp(0.1, 1.0)))]);
        }
    }
    for r in &c.window_rules {
        let mut matchers = Vec::new();
        if let Some(a) = &r.app_id {
            matchers.push(("app_id", toml_str(a)));
        }
        if let Some(t) = &r.title {
            matchers.push(("title", toml_str(t)));
        }
        if matchers.is_empty() {
            continue;
        }
        let mut settings = Vec::new();
        if let Some(v) = r.floating {
            settings.push(("default_floating", v.to_string()));
        }
        if let Some(v) = r.global {
            settings.push(("default_pinned", v.to_string()));
        }
        if r.no_blur == Some(true) {
            settings.push(("blur", "false".to_string()));
        }
        if r.no_shadow == Some(true) {
            settings.push(("shadow", "false".to_string()));
        }
        if let Some(v) = r.confine_pointer {
            settings.push(("confine_pointer", v.to_string()));
        }
        umbriel_window_rule(&mut s, &matchers, &settings);
        // One opacity per rule: focused / unfocused match on `is_focused`.
        for (focused, o) in [(true, r.focused_opacity), (false, r.unfocused_opacity)] {
            if let Some(o) = o {
                let mut m = matchers.clone();
                m.push(("is_focused", focused.to_string()));
                umbriel_window_rule(&mut s, &m, &[("opacity", format!("{:.2}", o.clamp(0.1, 1.0)))]);
            }
        }
    }
    for note in &skipped {
        let _ = writeln!(s, "# {note}");
    }
    Emitted { text: s, skipped }
}

/// Sway's default `gaps inner` (logical px): the edge gap Studio's outer gap
/// is measured against when the inner gap is left alone.
const SWAY_DEFAULT_GAP: u32 = 0;

/// A colour as sway writes it (`#rrggbbaa`).
fn sway_color(hex: &str) -> String {
    format!("#{}", hex_bare(hex))
}

/// Black or white title text, whichever contrasts more with `background`
/// (sway's `client.*` lines need a text colour; Mandara only syncs the
/// border colours).
fn sway_text_on(background: &str) -> &'static str {
    use mandara_theme::Color;
    let Ok(bg) = Color::from_hex(background) else { return SWAY_TEXT_LIGHT;
    };
    let (dark, light) = (Color::hex(SWAY_TEXT_DARK), Color::hex(SWAY_TEXT_LIGHT));
    if bg.contrast_ratio(dark) >= bg.contrast_ratio(light) { SWAY_TEXT_DARK } else { SWAY_TEXT_LIGHT }
}

/// Title text on light and dark title bars.
const SWAY_TEXT_DARK: &str = "#000000ff";
const SWAY_TEXT_LIGHT: &str = "#ffffffff";

/// A window class, app id or title as a sway criteria value: a quoted
/// regex. Sway has no escape for `"` inside one, so a quote becomes `.`
/// (the regex wildcard, so it still matches); control characters can't add
/// config lines.
fn sway_criteria_value(s: &str) -> String {
    let v: String = s.chars().filter(|c| !c.is_control()).map(|c| if c == '"' { '.' } else { c }).collect();
    format!("\"{v}\"")
}

/// The command a sway bind runs: `exec <command>`. Sway splits a binding's
/// command list at `;` and `,` outside quotes, so a command containing
/// either is double-quoted whole (sway passes what is inside, single
/// quotes and backslashes included, to the shell as it is). There is no
/// escape for a double quote inside: such a command can't be written
/// (`None`). Checked by running the commands in a headless sway.
fn sway_exec(cmd: &str) -> Option<String> {
    let cmd: String = cmd.chars().filter(|c| !c.is_control()).collect();
    match (cmd.contains([';', ',']), cmd.contains('"')) {
        (false, _) => Some(format!("exec {cmd}")),
        (true, false) => Some(format!("exec \"{cmd}\"")),
        (true, true) => None,
    }
}

/// `~/.config/mandara/generated/sway.conf`: sway's own commands only (from
/// `sway(5)`, `sway-input(5)` and `sway-output(5)`; checked with
/// `sway --validate`). Sway reads its config in order and the include sits
/// at the end of the user's file, so these lines apply on top of theirs.
/// Sway has no rounded corners, blur, shadows, animations, opacity rules or
/// hot corners: those Studio settings are left out (and locked in Studio).
pub fn emit_sway(cfg: &ManagedConfig) -> Emitted {
    let c = cfg.config;
    let l = &c.look;
    let mut s = String::new();
    let _ = writeln!(s, "# {HEADER}");
    if cfg.autostart() {
        // `exec` runs once at start-up (`exec_always` would start a second
        // shell on every reload).
        let _ = writeln!(s, "\nexec {}", cfg.mandara_bin);
    }
    let mut look = String::new();
    if let Some(g) = l.gaps_inner {
        let _ = writeln!(look, "gaps inner {g}");
    }
    // Sway's outer gap is added to the inner one at the screen edges.
    if let Some(outer) = l.gaps_outer {
        let d = outer as i64 - l.gaps_inner.unwrap_or(SWAY_DEFAULT_GAP) as i64;
        let _ = writeln!(look, "gaps outer {d}");
    }
    if !look.is_empty() {
        let _ = write!(s, "\n# Window look (Studio → Compositor)\n{look}");
    }
    if let Some(w) = c.border_width {
        // Pixel borders (no title bars), as on every other compositor.
        let _ = writeln!(s, "\n# Window border width (Studio → Compositor)\ndefault_border pixel {w}\ndefault_floating_border pixel {w}");
    }
    if let Some(bc) = &cfg.borders {
        // `client.<class> <border> <background> <text> <indicator> <child_border>`.
        let _ = writeln!(s, "\n# Live border colors from the active Mandara palette");
        let line = |class: &str, main: &str, indicator: &str| {
            format!("client.{class} {m} {m} {} {} {m}", sway_text_on(main), sway_color(indicator), m = sway_color(main))
        };
        let _ = writeln!(s, "{}", line("focused", &bc.active, &bc.active_end));
        let _ = writeln!(s, "{}", line("focused_inactive", &bc.inactive, &bc.inactive));
        let _ = writeln!(s, "{}", line("unfocused", &bc.inactive, &bc.inactive));
        let _ = writeln!(s, "{}", line("urgent", &bc.urgent, &bc.urgent));
    }
    s.push_str(&crate::input::emit_sway(&c.input));
    let mut rules = String::new();
    for r in &c.window_rules {
        let mut commands = Vec::new();
        if let Some(f) = r.floating {
            commands.push(format!("floating {}", if f { "enable" } else { "disable" }));
        }
        // Sticky only applies to floating windows (like Hyprland's pin).
        if let Some(g) = r.global {
            commands.push(format!("sticky {}", if g { "enable" } else { "disable" }));
        }
        if commands.is_empty() {
            continue;
        }
        let title = r.title.as_deref().map(|t| format!(" title={}", sway_criteria_value(t))).unwrap_or_default();
        // Wayland windows match on `app_id`, X11 ones on `class`.
        let criteria: Vec<String> = match &r.app_id {
            Some(a) => vec![format!("[app_id={}{title}]", sway_criteria_value(a)), format!("[class={}{title}]", sway_criteria_value(a))],
            None if !title.is_empty() => vec![format!("[{}]", title.trim_start())],
            None => continue,
        };
        for crit in criteria {
            let _ = writeln!(rules, "for_window {crit} {}", commands.join(", "));
        }
    }
    if !rules.is_empty() {
        let _ = write!(s, "\n# Window rules\n{rules}");
    }
    s.push_str(&crate::outputs::emit_outputs(Target::Sway, &c.outputs, false));
    let (binds, mut skipped) = split_binds(cfg, Target::Sway);
    let _ = writeln!(s, "\n# Mandara keybinds");
    for k in binds {
        let combo = k.mods.iter().copied().chain(std::iter::once(k.key.as_str())).collect::<Vec<_>>().join("+");
        let command = k.bind.action.shell_command_with(&cfg.mandara_bin);
        match sway_exec(&command) {
            Some(exec) => {
                let _ = writeln!(s, "bindsym {combo} {exec}");
            }
            None => skipped.push(format!("{} → `{command}` skipped: sway can't quote a command with both `\"` and `;` or `,`", k.bind.display())),
        }
    }
    for note in &skipped {
        let _ = writeln!(s, "# {note}");
    }
    let _ = writeln!(s, "# End Mandara keybinds");
    Emitted { text: s, skipped }
}

/// Whether River's `init` text starts Mandara: a `riverctl spawn …` of it
/// or a plain command line running it (`mandara &`), not a bind running
/// `mandara msg …`.
pub fn river_starts_mandara(init: &str) -> bool {
    init.lines().map(shell_words).any(|w| {
        let is_mandara = |s: &str| {
            s.split_whitespace().next().is_some_and(|p| p == "mandara" || p.ends_with("/mandara")) && !s.split_whitespace().any(|a| a == "msg")
        };
        match w.as_slice() {
            [ctl, spawn, cmd, ..] if ctl.ends_with("riverctl") && spawn == "spawn" => {
                is_mandara(cmd)
            }
            [first, ..] => {
                (first == "mandara" || first.ends_with("/mandara")) && !w.iter().any(|a| a == "msg")
            }
            [] => false,
        }
    })
}

/// A Mandara window rule's regex as a River glob (`riverctl(1)`: a `*` may
/// start and / or end it; nothing else is special). `^org\.app$` → `org.app`,
/// `^steam_app_.*` → `steam_app_*`; `None` when the regex needs more than
/// that (alternatives, classes, wildcards inside).
pub fn river_glob(regex: &str) -> Option<String> {
    let mut r = regex.trim();
    let (mut lead, mut trail) = (true, true);
    if let Some(rest) = r.strip_prefix('^') {
        r = rest;
        lead = false;
    }
    if let Some(rest) = r.strip_suffix('$').filter(|rest| !rest.ends_with('\\')) {
        r = rest;
        trail = false;
    }
    if let Some(rest) = r.strip_prefix(".*") {
        r = rest;
        lead = true;
    }
    if let Some(rest) = r.strip_suffix(".*") {
        r = rest;
        trail = true;
    }
    let mut out = String::new();
    let mut chars = r.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(chars.next().filter(|c| !c.is_alphanumeric())?),
            '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$' => {
                return None;
            }
            c if c.is_control() => return None,
            c => out.push(c),
        }
    }
    if out.is_empty() {
        return None;
    }
    Some(format!("{}{out}{}", if lead { "*" } else { "" }, if trail { "*" } else { "" }))
}

/// A River colour argument (`0xRRGGBBAA`).
fn river_color(hex: &str) -> String {
    format!("0x{}", hex_bare(hex))
}

/// The tag bitmask a `global` rule gives new windows: every tag.
const RIVER_ALL_TAGS: u32 = u32::MAX;

/// The River commands (argv for `river-control`, as `riverctl` takes them)
/// that carry Mandara's settings, and the notes on what was left out.
/// Every command and option comes from `riverctl(1)`; the tests run each
/// through River itself. River has no gaps, rounded corners, blur, shadows,
/// animations, opacity rules or output settings of its own: those Studio
/// settings are left out (and locked in Studio).
pub fn river_commands(cfg: &ManagedConfig) -> (Vec<Vec<String>>, Vec<String>) {
    let c = cfg.config;
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut cmd = |words: &[&str]| out.push(words.iter().map(|w| w.to_string()).collect());
    if let Some(w) = c.border_width {
        cmd(&["border-width", &w.to_string()]);
    }
    if let Some(bc) = &cfg.borders {
        cmd(&["border-color-focused", &river_color(&bc.active)]);
        cmd(&["border-color-unfocused", &river_color(&bc.inactive)]);
        cmd(&["border-color-urgent", &river_color(&bc.urgent)]);
    }
    for words in crate::input::river_commands(&c.input) {
        out.push(words);
    }
    let mut skipped = Vec::new();
    for r in &c.window_rules {
        let mut matchers: Vec<String> = Vec::new();
        let mut usable = true;
        for (flag, regex) in [("-app-id", &r.app_id), ("-title", &r.title)] {
            if let Some(regex) = regex {
                match river_glob(regex) {
                    Some(glob) => matchers.extend([flag.to_string(), glob]),
                    None => usable = false,
                }
            }
        }
        if !usable || matchers.is_empty() {
            if r.floating.is_some() || r.global.is_some() {
                skipped.push(format!("rule for {} skipped: River matches with simple globs (a `*` at the start or end)", r.app_id.as_deref().or(r.title.as_deref()).unwrap_or("?")));
            }
            continue;
        }
        if let Some(f) = r.floating {
            out.push(matchers.iter().cloned().chain([if f { "float" } else { "no-float" }.to_string()]).fold(vec!["rule-add".to_string()], |mut v, w| {
                v.push(w);
                v
            }));
        }
        if r.global == Some(true) {
            out.push(matchers.iter().cloned().chain(["tags".to_string(), RIVER_ALL_TAGS.to_string()]).fold(vec!["rule-add".to_string()], |mut v, w| {
                v.push(w);
                v
            }));
        }
    }
    let (binds, bind_skips) = split_binds(cfg, Target::River);
    skipped.extend(bind_skips);
    for k in binds {
        let mods = if k.mods.is_empty() { "None".to_string() } else { k.mods.join("+") };
        out.push(vec!["map".into(), "normal".into(), mods, k.key.clone(), "spawn".into(), k.bind.action.shell_command_with(&cfg.mandara_bin)]);
    }
    (out, skipped)
}

/// One command as a `riverctl` line (shell-quoted).
pub fn riverctl_line(words: &[String]) -> String {
    std::iter::once("riverctl".to_string()).chain(words.iter().map(|w| shell_quote(w))).collect::<Vec<_>>().join(" ")
}

/// `~/.config/mandara/generated/river.conf`: the record of the River
/// commands Mandara sends (one `riverctl` line each). River doesn't read
/// it; see [`river_commands`].
pub fn emit_river(cfg: &ManagedConfig) -> Emitted {
    let (commands, skipped) = river_commands(cfg);
    let mut s = String::new();
    let _ = writeln!(s, "# {HEADER}");
    let _ = writeln!(s, "# River doesn't read this file: Mandara sends these commands over river-control when the shell starts and on every change.");
    let _ = writeln!(s, "# To start Mandara with River, add this line to ~/.config/river/init:\n#   {}\n", river_spawn_line(&cfg.mandara_bin));
    for words in &commands {
        let _ = writeln!(s, "{}", riverctl_line(words));
    }
    for note in &skipped {
        let _ = writeln!(s, "# {note}");
    }
    Emitted { text: s, skipped }
}

/// The commands recorded in a `river.conf` (see [`emit_river`]).
pub fn parse_river_record(text: &str) -> Vec<Vec<String>> {
    text.lines().map(shell_words).filter(|w| w.first().is_some_and(|c| c == "riverctl")).map(|w| w[1..].to_vec()).filter(|w| !w.is_empty()).collect()
}

/// The commands that take back what `previous` set and the next sync may
/// not set again: Mandara's own binds (`unmap`) and rules (`rule-del`).
/// Settings such as colours are simply set again.
pub fn river_undo(previous: &[Vec<String>]) -> Vec<Vec<String>> {
    previous
        .iter()
        .filter_map(|w| match w.first().map(String::as_str) {
            Some("map") if w.len() >= 4 => Some(vec!["unmap".to_string(), w[1].clone(), w[2].clone(), w[3].clone()]),
            Some("rule-add") if w.len() >= 2 => {
                // `rule-add [matchers] action [args]` → `rule-del [matchers] action`.
                let mut del = vec!["rule-del".to_string()];
                let mut i = 1;
                while i + 1 < w.len() && w[i].starts_with('-') {
                    del.extend([w[i].clone(), w[i + 1].clone()]);
                    i += 2;
                }
                // Float and no-float share a list, as do the others.
                del.push(match w.get(i).map(String::as_str) {
                    Some("no-float") => "float".to_string(),
                    Some(action) => action.to_string(),
                    None => return None,
                });
                Some(del)
            }
            _ => None,
        })
        .collect()
}

pub fn emit(target: Target, cfg: &ManagedConfig) -> Emitted {
    match target {
        Target::Mango => emit_mango(cfg),
        Target::Niri => emit_niri(cfg),
        Target::Hyprland => emit_hyprland(cfg),
        Target::Umbriel => emit_umbriel(cfg),
        Target::Sway => emit_sway(cfg),
        Target::River => emit_river(cfg),
        Target::KWin => Emitted { text: "# KWin settings remain in Plasma System Settings. Mandara reads native Plasma protocols.\n".into(), skipped: Vec::new() },
        // Aqueous documents no include directive, so no drop-in line could
        // be checker-verified: its settings stay in its own TOML files and
        // Mandara only talks to it over `$AQUEOUS_SOCKET`. Never written.
        Target::Aqueous => Emitted { text: "# Aqueous settings remain in its own files in ~/.config/aqueous. Mandara reads its IPC socket and writes nothing for it.\n".into(), skipped: Vec::new() },
        // labwc's rc.xml has no include Mandara's drop-in could use safely,
        // and COSMIC keeps its settings in COSMIC Settings: no line could
        // be checker-verified, so nothing is written for either.
        Target::Labwc => Emitted { text: "# labwc settings remain in ~/.config/labwc. Mandara reads the ext-workspace protocol and writes nothing for it.\n".into(), skipped: Vec::new() },
        Target::Cosmic => Emitted { text: "# COSMIC settings remain in COSMIC Settings. Mandara reads the ext-workspace protocol and writes nothing for it.\n".into(), skipped: Vec::new() },
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
        // Umbriel's includes are TOML arrays, read whole-file by
        // [`includes_in`].
        Target::Mango | Target::Umbriel => None,
        // River's init is a program, not a config with includes.
        Target::River | Target::KWin | Target::Aqueous | Target::Labwc | Target::Cosmic => None,
        // `include <path>`: a shell word (`~`, `$VAR`, globs), expanded by
        // [`included_files`].
        Target::Sway => line.strip_prefix("include").filter(|r| r.starts_with(char::is_whitespace)).map(|r| r.trim().to_string()).filter(|r| !r.is_empty()),
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

/// The files one config file's `text` includes, as written: one per include
/// line (MangoWM, Niri, Hyprland; commented lines skipped), or Umbriel's
/// `[include] files` and `[include.optional] files` (with `$VAR` /
/// `${VAR}` expanded, as Umbriel does).
pub(crate) fn includes_in(target: Target, text: &str) -> Vec<String> {
    if target == Target::Umbriel {
        return umbriel_includes(text);
    }
    text.lines().filter(|l| !l.trim_start().starts_with(['#', '/']) && !l.trim_start().starts_with("--")).filter_map(|l| include_path(target, l)).collect()
}

/// Umbriel `[include] files` then `[include.optional] files`.
pub fn umbriel_includes(text: &str) -> Vec<String> {
    let Ok(doc) = text.parse::<toml::Table>() else { return Vec::new();
    };
    let Some(include) = doc.get("include").and_then(toml::Value::as_table) else { return Vec::new();
    };
    let files = |t: Option<&toml::Table>| -> Vec<String> { t.and_then(|t| t.get("files")).and_then(toml::Value::as_array).map(|a| {
                a.iter().filter_map(|v| v.as_str()).map(expand_env).collect()
            }).unwrap_or_default() };
    let mut out = files(Some(include));
    out.extend(files(include.get("optional").and_then(toml::Value::as_table)));
    out
}

/// `$VAR` and `${VAR}` replaced from the environment (unset: empty), the
/// expansion Umbriel applies to include paths.
fn expand_env(path: &str) -> String {
    let mut out = String::new();
    let mut rest = path;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let (name, tail) = if let Some(braced) = after.strip_prefix('{') {
            match braced.find('}') {
                Some(end) => (&braced[..end], &braced[end + 1..]),
                None => ("", after),
            }
        } else {
            let end = after.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(after.len());
            (&after[..end], &after[end..])
        };
        if name.is_empty() {
            out.push('$');
        } else {
            out.push_str(&std::env::var(name).unwrap_or_default());
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// Expands `source=`/`include` paths relative to `base`.
pub(crate) fn resolve_include(base: &Path, raw: &str) -> PathBuf {
    let raw = raw.trim().trim_matches('"');
    let p = paths::expand_tilde(raw);
    if p.is_absolute() { p } else { base.parent().unwrap_or(Path::new(".")).join(p) }
}

/// The files `file` (whose contents are `text`) includes, resolved: one per
/// include line, or, on Sway, every file a shell-word path names (sway
/// expands `~`, `$VAR` and globs such as `/etc/sway/config.d/*`, and skips
/// a pattern that matches nothing).
pub(crate) fn included_files(target: Target, file: &Path, text: &str) -> Vec<PathBuf> {
    includes_in(target, text)
        .into_iter()
        .flat_map(|raw| match target {
            Target::Sway => sway_include_files(file, &raw),
            _ => vec![resolve_include(file, &raw)],
        })
        .collect()
}

/// Shortcut scans fail visibly rather than retaining an unbounded include glob.
pub(crate) fn included_files_bounded(target: Target, file: &Path, text: &str, limit: usize) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for raw in includes_in(target, text) {
        let words = if target == Target::Sway { sway_include_words(&raw) } else { vec![raw] };
        for word in words {
            let path = resolve_include(file, &expand_env(&word));
            // Linux's pathname limit also bounds matching work on a hand-edited glob.
            anyhow::ensure!(path.as_os_str().len() <= 4096, "include path is too long");
            let remaining = limit.saturating_sub(files.len());
            let matches = if target == Target::Sway && has_glob(&path.to_string_lossy()) {
                glob_path_bounded(&path, remaining)?
            } else { vec![path] };
            anyhow::ensure!(matches.len() <= remaining, "too many included shortcut files");
            files.extend(matches);
        }
    }
    Ok(files)
}

/// Sway include line → files, as sway 1.12 reads it: the rest of the line
/// with an argument's quotes removed, then split into shell words
/// (`wordexp`): `include a.conf b.conf` and `include "a.conf b.conf"` both
/// load two files, `my\ dir/x.conf` is one. Each word has `$VAR` and `~`
/// expanded, is taken relative to the including file and globbed (`*`,
/// `?`) in every component (`conf.d/*/bindings.conf`); a word's matches are
/// sorted, like `wordexp` returns them, and a pattern that matches nothing
/// is skipped.
fn sway_include_files(base: &Path, raw: &str) -> Vec<PathBuf> {
    sway_include_words(raw)
        .into_iter()
        .flat_map(|word| {
            let path = resolve_include(base, &expand_env(&word));
            if has_glob(&path.to_string_lossy()) { glob_path(&path) } else { vec![PathBuf::from(unescape_glob(&path.to_string_lossy()))] }
        })
        .collect()
}

/// Shell words with escaped wildcard occurrences retained for glob matching.
fn sway_include_words(raw: &str) -> Vec<String> {
    // Sway's own argument handling first: an argument that starts with a
    // quote loses its (unescaped) quotes, backslashes stay for `wordexp`.
    let mut line = String::new();
    for arg in sway_args(raw.trim()) {
        if !line.is_empty() {
            line.push(' ');
        }
        if arg.starts_with(['"', '\'']) {
            let mut escaped = false;
            for c in arg.chars() {
                match c {
                    '"' | '\'' if !escaped => {}
                    '\\' => {
                        escaped = !escaped;
                        line.push(c);
                        continue;
                    }
                    c => line.push(c),
                }
                escaped = false;
            }
        } else {
            line.push_str(&arg);
        }
    }
    // Then shell words: blanks split, quotes group, `\` escapes.
    let (mut words, mut word, mut started) = (Vec::new(), String::new(), false);
    let mut chars = line.chars();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => match chars.next() {
                Some(n @ ('"' | '\\' | '$' | '`')) => word.push(n),
                Some(n) => {
                    word.push('\\');
                    word.push(n);
                }
                None => word.push('\\'),
            },
            (Some(_), c) => {
                if matches!(c, '*' | '?' | '\\') { word.push('\\'); }
                word.push(c);
            }
            (None, '\'' | '"') => {
                quote = Some(c);
                started = true;
            }
            (None, '\\') => {
                if let Some(n) = chars.next() {
                    if matches!(n, '*' | '?' | '\\') { word.push('\\'); }
                    word.push(n);
                    started = true;
                }
            }
            (None, c) if c.is_whitespace() => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                started = false;
            }
            (None, c) => word.push(c),
        }
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    words
}

/// Sway's split of a command line into arguments: blanks outside quotes
/// separate them; quotes and backslashes are kept.
fn sway_args(line: &str) -> Vec<String> {
    let (mut args, mut arg, mut quote, mut escaped) = (Vec::new(), String::new(), None, false);
    for c in line.chars() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if let Some(q) = quote {
            if c == q {
                quote = None;
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
        } else if c.is_whitespace() {
            if !arg.is_empty() {
                args.push(std::mem::take(&mut arg));
            }
            continue;
        }
        arg.push(c);
    }
    if !arg.is_empty() {
        args.push(arg);
    }
    args
}

/// Whether the pattern contains an unescaped wildcard.
fn has_glob(pattern: &str) -> bool {
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => { chars.next(); }
            '*' | '?' => return true,
            _ => {}
        }
    }
    false
}

fn unescape_glob(pattern: &str) -> String {
    let mut chars = pattern.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        out.push(if c == '\\' { chars.next().unwrap_or(c) } else { c });
    }
    out
}

/// The existing paths an absolute glob names (`*`, `?` in any component),
/// sorted.
fn glob_path(pattern: &Path) -> Vec<PathBuf> {
    glob_path_bounded(pattern, usize::MAX).unwrap_or_default()
}

fn glob_path_bounded(pattern: &Path, limit: usize) -> Result<Vec<PathBuf>> {
    // Include scans must finish even when a glob matches nothing in a huge tree.
    const MAX_ENTRIES: usize = 65_536;
    let mut examined = 0;
    let mut found = vec![PathBuf::new()];
    for part in pattern.components() {
        let name = part.as_os_str().to_string_lossy();
        if !has_glob(&name) {
            for p in &mut found {
                p.push(unescape_glob(&name));
            }
            continue;
        }
        let matcher = glob::Pattern::new(&name);
        let mut next = Vec::new();
        for dir in &found {
            let entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error).with_context(|| dir.display().to_string()),
            };
            for entry in entries {
                examined += 1;
                anyhow::ensure!(examined <= MAX_ENTRIES, "include glob exceeds the directory scan budget");
                let entry = entry?;
                if matcher.matches(&entry.file_name().to_string_lossy()) {
                    anyhow::ensure!(next.len() < limit, "too many included shortcut files");
                    next.push(entry.path());
                }
            }
        }
        found = next;
    }
    found.retain(|p| p.exists());
    found.sort();
    Ok(found)
}

/// Shell glob match of one path component (`*` and `?`; hidden files only
/// match a pattern that starts with `.`).
#[cfg(test)]
fn glob_match(pattern: &str, name: &str) -> bool {
    glob::Pattern::new(pattern).matches(name)
}

/// Chords bound in the user's own config, following `source=` / `include`
/// (but never Mandara's generated file).
pub fn existing_chords(target: Target) -> BTreeSet<String> {
    existing_chords_at(target, &target.user_config())
}

/// Native editors use the running compositor's config, including custom paths.
pub fn existing_chords_at(target: Target, root: &Path) -> BTreeSet<String> {
    if target == Target::Mango {
        return mango_keys::with_user_keys(|keys| {
            mango_existing_chords(root, keys)
        });
    }
    if target == Target::Sway {
        return sway_existing_chords(root);
    }
    let mut out = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    let mut visited = BTreeSet::new();
    let generated = target.generated_file();
    while let Some(file) = stack.pop() {
        // Never count a Mandara drop-in (ours, or another copy's) as the
        // user's own binds.
        if !visited.insert(file.clone()) || file == generated || is_generated(&file) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else { continue;
        };
        out.extend(parse_chords(target, &text));
        stack.extend(included_files(target, &file, &text));
    }
    out
}

/// Sway reads `include`d files in place, and a `set $mod Mod4` in the main
/// config holds in every file included after it (the usual
/// `include /etc/sway/config.d/*` binds `$mod+…`): the user's binds are
/// walked in order with the variables known at each point.
fn sway_existing_chords(root: &Path) -> BTreeSet<String> {
    sway_config_binds(&[root.to_path_buf()]).into_iter().map(|(_, bind)| bind.chord).collect()
}

/// MangoWM reads `source=` files in place and keeps one key mode across
/// them, so the user's binds are walked in order. Mandara's binds live in
/// the key mode in force where its drop-in is sourced (`default` when it
/// isn't sourced yet); only user binds that can meet that mode count.
fn mango_existing_chords(root: &Path, keys: &mango_keys::MangoKeys) -> BTreeSet<String> {
    struct Walk<'k> {
        keys: &'k mango_keys::MangoKeys,
        visited: BTreeSet<PathBuf>,
        binds: Vec<(String, String)>,
        ours: Option<String>,
    }
    fn walk(w: &mut Walk, file: &Path, mode: String) -> String {
        if !w.visited.insert(file.to_path_buf()) {
            return mode;
        }
        let Ok(text) = std::fs::read_to_string(file) else { return mode;
        };
        let mut mode = mode;
        // Split the file at its `source=` lines so each part starts in the
        // mode the previous part (or sourced file) left.
        let mut chunk = String::new();
        for line in text.lines() {
            let sourced = (!line.trim_start().starts_with('#')).then(|| include_path(Target::Mango, line)).flatten();
            match sourced {
                Some(path) => {
                    mode = flush(w, &mut chunk, mode);
                    let path = resolve_include(file, &path);
                    if path == Target::Mango.generated_file() || is_generated(&path) {
                        w.ours.get_or_insert_with(|| mode.clone());
                    } else {
                        mode = walk(w, &path, mode);
                    }
                }
                None => {
                    chunk.push_str(line);
                    chunk.push('\n');
                }
            }
        }
        flush(w, &mut chunk, mode)
    }
    fn flush(w: &mut Walk, chunk: &mut String, mode: String) -> String {
        let (binds, mode) = mango_binds(chunk, &mode, w.keys);
        w.binds.extend(binds.into_iter().map(|b| (b.chord, b.mode)));
        chunk.clear();
        mode
    }
    let mut w = Walk { keys, visited: BTreeSet::new(), binds: Vec::new(), ours: None };
    walk(&mut w, root, mango_keys::DEFAULT_MODE.to_string());
    let ours = w.ours.unwrap_or_else(|| mango_keys::DEFAULT_MODE.to_string());
    w.binds.into_iter().filter(|(_, mode)| mango_keys::modes_meet(mode, &ours)).map(|(chord, _)| chord).collect()
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
        Target::Umbriel => umbriel_chords(text),
        Target::Sway => sway_binds(text).into_iter().map(|b| (b.line, b.chord)).collect(),
        Target::River => river_binds(text).into_iter().map(|b| (b.line, b.chord)).collect(),
        Target::KWin | Target::Aqueous | Target::Labwc | Target::Cosmic => Vec::new(),
    }
}

/// One `riverctl map` of River's `init`: 0-based line, chord, command.
#[derive(Clone, Debug, PartialEq)]
pub struct RiverBind {
    pub line: usize,
    pub chord: String,
    pub command: String,
}

/// The `riverctl map [-flags] normal MODS KEY command…` lines of an `init`
/// script (normal mode; maps built in shell loops or with variables can't
/// be read and are left out).
pub fn river_binds(text: &str) -> Vec<RiverBind> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let words = shell_words(line);
        let Some(at) = words.iter().position(|w| w == "riverctl" || w.ends_with("/riverctl")) else { continue;
        };
        let mut rest = words[at + 1..].iter().map(String::as_str);
        if rest.next() != Some("map") {
            continue;
        }
        let mut args: Vec<&str> = Vec::new();
        let mut rest = rest.peekable();
        while let Some(w) = rest.next() {
            match w {
                "-release" | "-repeat" => {}
                "-layout" => {
                    rest.next();
                }
                w => args.push(w),
            }
        }
        let [mode, mods, key, command @ ..] = args.as_slice() else { continue;
        };
        if *mode != "normal" || mods.contains('$') || key.contains('$') {
            continue;
        }
        let mods = mods.split('+').filter(|m| !m.eq_ignore_ascii_case("none"));
        out.push(RiverBind { line: i, chord: chord(mods, &keysym_for(key)), command: command.join(" ") });
    }
    out
}

/// One sway key bind: its 0-based line, its chord and the command it runs.
#[derive(Clone, Debug, PartialEq)]
pub struct SwayBind {
    pub line: usize,
    pub chord: String,
    pub command: String,
    /// A complete bindsym directive with the variables at this line expanded.
    pub expanded: String,
}

/// Sway `bindsym` binds of the default mode, as sway reads them: `set
/// $name value` variables substituted, `--flags` skipped, `bindsym { … }`
/// blocks read line by line, binds inside `mode "…" { … }` blocks left out
/// (they only act in that mode). `bindcode` binds name raw keycodes and
/// are left out too.
pub fn sway_binds(text: &str) -> Vec<SwayBind> {
    sway_binds_with(text, &mut Vec::new())
}

/// Sets `$name` in `vars`: a second `set $mod …` replaces the first (sway
/// uses the latest value from there on), and names stay longest first, so
/// `$mod` never eats `$modShift`.
pub(crate) fn set_var(vars: &mut Vec<(String, String)>, name: &str, value: &str) {
    match vars.iter_mut().find(|(n, _)| n == name) {
        Some((_, v)) => *v = value.to_string(),
        None => {
            vars.push((name.to_string(), value.to_string()));
            vars.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
        }
    }
}

/// [`sway_binds`] with the variables already set before `text` (and
/// those it sets added).
fn sway_binds_with(text: &str, vars: &mut Vec<(String, String)>) -> Vec<SwayBind> {
    sway_binds_in(text, vars, &mut Vec::new())
}

fn sway_binds_in(text: &str, vars: &mut Vec<(String, String)>, blocks: &mut Vec<String>) -> Vec<SwayBind> {
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "}" {
            blocks.pop();
            continue;
        }
        let mut line = line.to_string();
        if let Some(rest) = line.strip_prefix("set ")
            && let Some((name, value)) = rest.trim().split_once(char::is_whitespace)
            && name.starts_with('$')
        {
            let value = vars.iter().fold(value.trim().to_string(), |value, (name, replacement)| {
                    value.replace(name, replacement)
                });
            set_var(vars, name, &value);
            continue;
        }
        for (name, value) in vars.iter() {
            line = line.replace(name.as_str(), value);
        }
        let opens_block = line.ends_with('{');
        let body = line.trim_end_matches('{').trim_end();
        let in_bind_block = blocks.last().is_some_and(|b| b == "bindsym") && blocks.len() == 1;
        if opens_block {
            blocks.push(body.split_whitespace().next().unwrap_or_default().to_string());
            continue;
        }
        let words: Vec<&str> = if in_bind_block {
            body.split_whitespace().collect()
        } else if blocks.is_empty() && body.split_whitespace().next() == Some("bindsym") {
            body.split_whitespace().skip(1).collect()
        } else {
            continue;
        };
        let mut words = words.into_iter().skip_while(|w| w.starts_with("--"));
        let Some(combo) = words.next() else { continue };
        let mut parts: Vec<&str> = combo.split('+').filter(|p| !p.is_empty()).collect();
        let Some(key) = parts.pop() else { continue };
        let command = words.collect::<Vec<_>>().join(" ");
        let expanded = if in_bind_block { format!("bindsym {body}") } else { body.to_string() };
        out.push(SwayBind { line: i, chord: chord(parts.into_iter(), &keysym_for(key)), command, expanded });
    }
    out
}

/// The table a TOML header line opens (`[keybinds]` → `keybinds`,
/// `[[window_rule]]` → `window_rule`), quotes and spaces kept as written.
pub(crate) fn toml_header(line: &str) -> Option<&str> {
    let t = toml_code(line).trim();
    let inner = t.strip_prefix("[[").and_then(|r| r.strip_suffix("]]")).or_else(|| t.strip_prefix('[').and_then(|r| r.strip_suffix(']')))?;
    Some(inner.trim())
}

/// A TOML line without its trailing comment (`#` outside strings).
pub(crate) fn toml_code(line: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (Some('"'), '\\') if !escaped => {
                escaped = true;
                continue;
            }
            (Some(q), c) if c == q && !escaped => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, '#') => return &line[..i],
            _ => {}
        }
        escaped = false;
    }
    line
}

/// The key of a TOML `key = value` line, unquoted (`"Mod+T" = …` → `Mod+T`).
pub(crate) fn toml_line_key(line: &str) -> Option<String> {
    let t = toml_code(line).trim_start();
    if let Some(q) = t.chars().next().filter(|c| *c == '"' || *c == '\'') {
        let end = t[1..].find(q)? + 1;
        t[end + 1..].trim_start().starts_with('=').then(|| t[1..end].replace("\\\"", "\""))
    } else {
        let (key, _) = t.split_once('=')?;
        let key = key.trim();
        (!key.is_empty() && !key.starts_with('[')).then(|| key.to_string())
    }
}

/// Umbriel `[keybinds]` chords with their 0-based line (`"Mod+T" = …`;
/// `Mod` is Super, Umbriel's default `mod_key`). Submap binds
/// (`submap[name],Key`) only act inside their submap and are left out.
fn umbriel_chords(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut in_binds = false;
    for (i, line) in text.lines().enumerate() {
        if let Some(header) = toml_header(line) {
            in_binds = header == "keybinds";
            continue;
        }
        let Some(key) = toml_line_key(line).filter(|_| in_binds) else { continue;
        };
        if key.starts_with("submap[") {
            continue;
        }
        let mut parts: Vec<&str> = key.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
        let Some(k) = parts.pop() else { continue };
        out.push((i, chord(parts.into_iter(), &keysym_for(k))));
    }
    out
}

fn mango_chords(text: &str) -> Vec<(usize, String)> {
    crate::mango_keys::with_user_keys(|keys| {
        mango_binds(text, mango_keys::DEFAULT_MODE, keys).0.into_iter().filter(|b| mango_keys::modes_meet(&b.mode, mango_keys::DEFAULT_MODE)).map(|b| (b.line, b.chord)).collect()
    })
}

/// One MangoWM key bind: its 0-based line, its chord with the key as
/// MangoWM resolves it (see [`mango_keys`]) and its key mode.
pub(crate) struct MangoBind {
    pub line: usize,
    pub chord: String,
    pub mode: String,
}

/// MangoWM's key binds in `text`, starting in key mode `mode`; also returns
/// the key mode in force at the end of the file (a sourced file carries
/// it on).
pub(crate) fn mango_binds(text: &str, mode: &str, keys: &mango_keys::MangoKeys) -> (Vec<MangoBind>, String) {
    let mut mode = mode.to_string();
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue;
        };
        let key = key.trim();
        if key == mango_keys::KEY_MODE_KEY {
            mode = value.trim().to_string();
            continue;
        }
        let Some(flags) = mango_keys::bind_flags(key) else { continue;
        };
        let mut parts = value.splitn(3, ',');
        if let (Some(mods), Some(name)) = (parts.next(), parts.next()) {
            out.push(MangoBind { line: i, chord: mango_chord(mods.split('+'), keys.canonical(name, mango_keys::keysym_only(flags))), mode: mode.clone() });
        }
    }
    (out, mode)
}

/// A MangoWM chord: modifiers as MangoWM reads them (`super_l` is Super,
/// `none` is nothing) and an already-resolved key.
fn mango_chord<'a>(mods: impl Iterator<Item = &'a str>, key: String) -> String {
    let mods: Vec<String> = mods.map(|m| m.trim().to_ascii_lowercase()).map(|m| {
            mango_keys::modifier_code(&m).map_or_else(|| m.trim_end_matches("_l").trim_end_matches("_r").to_string(), str::to_string)
        }).collect();
    chord(mods.iter().map(String::as_str), &key)
}

/// The chord a Mandara bind takes in `target`'s config: MangoWM writes a
/// plain `bind`, so its key resolves like the user's own binds do.
pub fn target_chord(target: Target, bind: &Keybind) -> String {
    match target {
        Target::Mango => crate::mango_keys::with_user_keys(|keys| {
            mango_chord(bind.mods.iter().map(String::as_str), keys.canonical(&bind.keysym(), false))
        }),
        _ => bind.chord(),
    }
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
            let Some(first) = lua_first_arg(args) else { continue;
            };
            let Some(combo) = lua_concat(first, &vars) else { continue;
            };
            let mut parts: Vec<&str> = combo.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
            let Some(key) = parts.pop() else { continue };
            out.push((i, chord(hypr_mods(&parts.join(" ")).into_iter(), &keysym_for(key))));
            continue;
        }
        let Some(rest) = line.strip_prefix("bind").and_then(|r| r.split_once('=')).map(|(_, r)| r) else { continue;
        };
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
    // Nothing is written where the compositor keeps its own settings
    // (KWin's note above is only Mandara's record of that).
    if let Some(note) = target.settings_elsewhere().filter(|_| target != Target::KWin) {
        anyhow::bail!("{}", mandara_i18n::t(note));
    }
    let path = target.generated_file();
    // Options the config doctor found this compositor version rejects stay
    // out (commented, with the reason).
    let text = crate::doctor::state::apply_unsupported(target, &path, &emitted.text);
    let mut text = text;
    for kind in [
        crate::compositor_theme::Kind::AppColors,
        crate::compositor_theme::Kind::Displays,
    ] {
        let fragment = crate::compositor_theme::file(target, kind);
        text = crate::compositor_theme::compose(target, kind, &text, &fragment, fragment.is_file())?;
    }
    crate::store::write_atomic(&path, text.as_bytes())?;
    Ok(path)
}

/// Whether the user's config already includes the generated file.
pub fn include_installed(target: Target) -> bool {
    include_installed_at(target, &target.user_config())
}

fn include_installed_at(target: Target, config: &Path) -> bool {
    // Nothing to include where Mandara writes no drop-in.
    if target.self_configured() { return true; }
    if target == Target::River {
        // Nothing to include: what counts is that init starts Mandara.
        return std::fs::read_to_string(config).is_ok_and(|t| river_starts_mandara(&t));
    }
    let needle = target.generated_file();
    let needle_name = needle.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
    std::fs::read_to_string(config)
        .map(|t| {
            t.lines().any(|l| {
                !l.trim_start().starts_with(['#', '/']) && !l.trim_start().starts_with("--") && l.contains("mandara/generated/") && l.contains(&needle_name)
            })
        })
        .unwrap_or(false)
}

/// Whether a resolved path lies in the Nix store, which nobody edits.
fn in_nix_store(real: &Path) -> bool {
    real.starts_with("/nix/store")
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
    if added
        && let crate::conflicts::Validation::Invalid { tool, report } = crate::conflicts::validate(target) {
            match before {
                Some(original) => crate::store::write_atomic(&path, original.as_bytes())?,
                // A Sway config Mandara started from the system one goes again.
                None => {
                    let _ = std::fs::remove_file(&path);
                }
            }
            anyhow::bail!("{tool} rejects {} with Mandara's include line, so it was not added:\n{report}", path.display());
        }
    Ok(added)
}

fn install_include_at(target: Target, path: &Path) -> Result<bool> {
    if include_installed_at(target, path) {
        return Ok(false);
    }
    if target == Target::River {
        anyhow::bail!("Mandara doesn't edit River's init ({}); it sends its settings over river-control instead. To start Mandara with River, add this line to it:\n  {}", path.display(), target.include_line());
    }
    // A config Home Manager or NixOS made is a link into the read-only Nix
    // store: the line goes into its Nix source instead.
    if let Ok(real) = std::fs::canonicalize(path)
        && in_nix_store(&real)
    {
        let msg = mandara_i18n::tf("{path} is made by Nix (Home Manager or NixOS) and is read-only: add this line to its Nix configuration, rebuild, and run `mandara setup` again: {line}", &[("path", &path.display()), ("line", &target.include_line())]);
        anyhow::bail!("{msg}");
    }
    if target == Target::Sway && !path.exists() {
        seed_sway_config(path, Path::new(SWAY_SYSTEM_CONFIG))?;
    }
    let original = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    // Umbriel's include is a TOML array: worked out (and the file checked
    // to be TOML) before anything is written.
    let umbriel = match target {
        Target::Umbriel => Some(umbriel_add_include(&original, &target.generated_file().display().to_string()).with_context(|| format!("adding Mandara's include to {}", path.display()))?),
        _ => None,
    };
    let backup = path.with_extension(format!("{}.mandara-backup", path.extension().and_then(|e| e.to_str()).unwrap_or("conf")));
    std::fs::write(&backup, &original).with_context(|| format!("writing backup {}", backup.display()))?;
    let comment = match target {
        Target::Niri => "// Mandara Shell (generated rules, binds and colors)",
        Target::Hyprland if Target::hyprland_lua() => {
            "-- Mandara Shell (generated rules, binds and colors)"
        }
        _ => INCLUDE_COMMENT,
    };
    let updated = match umbriel {
        Some(Some(text)) => text,
        // Already listed (a commented-out copy aside): nothing to change.
        Some(None) => return Ok(false),
        None => {
            let sep = if original.ends_with('\n') { "" } else { "\n" };
            format!("{original}{sep}\n{comment}\n{}\n", target.include_line())
        }
    };
    crate::store::write_atomic(path, updated.as_bytes())?;
    Ok(true)
}

/// Starts the user's sway config as a copy of [`SWAY_SYSTEM_CONFIG`] when
/// they have none (`Ok(false)` when they do, or there is nothing to copy).
pub fn seed_sway_user_config() -> Result<bool> {
    let (user, system) = (Target::Sway.user_config(), Path::new(SWAY_SYSTEM_CONFIG));
    if user.exists() || !system.is_file() {
        return Ok(false);
    }
    seed_sway_config(&user, system).map(|()| true)
}

/// A Sway user without a config of their own runs the system-wide one:
/// theirs starts as a copy of it (sway's documented first step), so adding
/// the include changes nothing else. Never overwrites an existing file.
fn seed_sway_config(path: &Path, system: &Path) -> Result<()> {
    let text = std::fs::read_to_string(system).with_context(|| {
        format!("no sway config at {} and no {} to start one from", path.display(), system.display())
    })?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path).with_context(|| format!("creating {}", path.display()))?;
    std::io::Write::write_all(&mut file, text.as_bytes()).with_context(|| format!("writing {}", path.display()))
}

/// The comment above Mandara's include line (`#` dialects).
const INCLUDE_COMMENT: &str = "# Mandara Shell (generated rules, binds and colors)";

/// Umbriel config `text` with `path` added to `[include.optional] files`,
/// keeping everything else as written (comments, order, formatting): the
/// path is appended to an existing list, or a new `[include.optional]`
/// table is added (inside an inline `include = { … }` when the user wrote
/// one). `None` when `path` is already included (required or optional).
/// Fails when the file isn't valid TOML or `include` isn't a table.
pub fn umbriel_add_include(text: &str, path: &str) -> Result<Option<String>> {
    use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};
    let mut doc: DocumentMut = text.parse().context("not valid TOML")?;
    let listed = |files: Option<&Item>| {
        files.and_then(Item::as_array).is_some_and(|a| a.iter().any(|v| v.as_str() == Some(path)))
    };
    if let Some(include) = doc.get("include").and_then(Item::as_table_like)
        && (listed(include.get("files")) || include.get("optional").and_then(Item::as_table_like).is_some_and(|o| listed(o.get("files"))))
    {
        return Ok(None);
    }
    let include = doc.entry("include").or_insert_with(|| {
        let mut t = Table::new();
        t.set_implicit(true);
        Item::Table(t)
    });
    let inline = include.as_inline_table().is_some();
    let include = include.as_table_like_mut().context("`include` is not a table")?;
    if inline {
        let optional = include.entry("optional").or_insert(Item::Value(Value::InlineTable(InlineTable::new())));
        let optional = optional.as_table_like_mut().context("`include.optional` is not a table")?;
        let files = optional.entry("files").or_insert(Item::Value(Value::Array(Array::new())));
        files.as_array_mut().context("`include.optional.files` is not a list")?.push(path);
    } else if let Some(optional) = include.get_mut("optional") {
        let optional = optional.as_table_like_mut().context("`include.optional` is not a table")?;
        let files = optional.entry("files").or_insert(Item::Value(Value::Array(Array::new())));
        files.as_array_mut().context("`include.optional.files` is not a list")?.push(path);
    } else {
        let mut optional = Table::new();
        optional.decor_mut().set_prefix(format!("\n{INCLUDE_COMMENT}\n"));
        let mut files = Array::new();
        files.push(path);
        optional.insert("files", Item::Value(Value::Array(files)));
        include.insert("optional", Item::Table(optional));
    }
    Ok(Some(doc.to_string()))
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
                windows: crate::window_motion::WindowMotion { style: crate::window_motion::WindowStyle::Snap, speed: 1.0 },
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
        let render = |rule: WindowRule| {
            emit_hyprland_lua(&managed(&CompositorConfig { window_rules: vec![rule], keybinds: Vec::new(), ..Default::default() })).text
        };
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

    #[test]
    fn exec_keybind_alias_keeps_the_program_and_writes_canonical_spawn() {
        let bind: Keybind = toml::from_str(
            r#"mods = ["super"]
key = "h"
[action]
kind = "exec"
value = "hyprctl dispatch 'hl.dsp.focus({ workspace = 2 })'"
"#,
        )
        .unwrap();
        assert_eq!(
            bind.action,
            BindAction::Spawn("hyprctl dispatch 'hl.dsp.focus({ workspace = 2 })'".into())
        );
        let saved = toml::to_string(&bind).unwrap();
        assert!(saved.contains(r#"kind = "spawn""#), "{saved}");
        assert_eq!(toml::from_str::<Keybind>(&saved).unwrap(), bind);
    }

    #[test]
    fn saved_target_ids_match_the_setup_command() {
        for target in Target::ALL {
            assert_eq!(serde_json::to_value(target).unwrap(), serde_json::json!(target.id()));
            assert_eq!(serde_json::from_value::<Target>(serde_json::json!(target.id())).unwrap(), target);
            assert_eq!(Target::parse(target.id()), Some(target));
        }
    }

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

    /// Issue #39: Home Manager's compositor configs are links into the Nix
    /// store; Mandara never tries to write there (a dotfiles link elsewhere
    /// is still written through).
    #[test]
    fn nix_store_configs_are_left_to_nix() {
        assert!(in_nix_store(Path::new("/nix/store/8f1x-home-manager-files/.config/hypr/hyprland.conf")));
        assert!(!in_nix_store(Path::new("/home/u/dotfiles/hypr/hyprland.conf")));
        assert!(!in_nix_store(Path::new("/nix/storefront/hyprland.conf")));
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
        let cfg = CompositorConfig {
            window_rules: vec![WindowRule { app_id: Some("^evil$".into()), title: Some("x\nexec = rm -rf ~\r\n, float on".into()), floating: Some(true), ..Default::default() }],
            ..Default::default()
        };
        for (name, text) in [("mango", emit_mango(&managed(&cfg, &[])).text), ("hyprland", emit_hyprland(&managed(&cfg, &[])).text), ("niri", emit_niri(&managed(&cfg, &[])).text)] {
            assert!(!text.lines().any(|l| l.trim_start().starts_with("exec")), "{name}: a title started a config line:\n{text}");
        }
        let mango = emit_mango(&managed(&cfg, &[])).text;
        let rule = mango.lines().find(|l| l.contains("app_id:^evil$")).unwrap();
        assert!(rule.contains("title:xexec = rm -rf ~. float on"), "commas can't split the rule: {rule}");
        assert_eq!(rule_value("a,b\u{7}c"), "a.bc");
        assert_eq!(kdl_str("a\nb\""), "\"a\\nb\\\"\"");
    }

    /// The floating panels' surface gets its rule only where the
    /// compositor's own checker has accepted it.
    #[test]
    fn floating_surface_rules_only_where_checked() {
        let cfg = CompositorConfig::default();
        let m = managed(&cfg, &[]);
        assert!(emit_mango(&m).text.contains("layer_rule=no_animation:1,no_shadow:1,no_blur:1,layer_name:mandara-floating"));
        assert!(emit_niri(&m).text.contains("match namespace=\"^mandara-floating$\""));
        assert!(!emit_hyprland(&m).text.contains("mandara-floating"), "not verified with Hyprland's checker");
        assert!(!emit_umbriel(&m).text.contains("mandara-floating"), "not verified with Umbriel's checker");
    }

    /// The wallpaper transition's surface: no animation, shadow or blur on
    /// MangoWM (its checker accepted it); elsewhere nothing yet.
    #[test]
    fn wallpaper_transition_surface_rules_only_where_checked() {
        let cfg = CompositorConfig::default();
        let m = managed(&cfg, &[]);
        assert!(emit_mango(&m).text.contains("layer_rule=no_animation:1,no_shadow:1,no_blur:1,layer_name:mandara-wallpaper-transition"));
        // `^mandara-wallpaper$` is anchored: the wallpaper's own rules don't match it.
        assert!(!emit_niri(&m).text.contains("mandara-wallpaper-transition"), "not verified with Niri's checker");
        assert!(!emit_hyprland(&m).text.contains("mandara-wallpaper-transition"), "not verified with Hyprland's checker");
        assert!(!emit_umbriel(&m).text.contains("mandara-wallpaper-transition"), "not verified with Umbriel's checker");
    }

    #[test]
    fn mango_output_skips_conflicts_and_uses_real_keys() {
        let cfg = CompositorConfig::default();
        let out = emit_mango(&managed(&cfg, &["super+space", "alt+ctrl+delete"]));
        assert!(out.text.contains("layer_rule=no_animation:1,no_shadow:1,no_blur:1,layer_name:mandara-bar"));
        assert!(out.text.contains("focus_color=0x5eead4ff"));
        assert!(!out.text.contains("border_px"), "width is left alone unless configured");
        let sized = CompositorConfig { border_width: Some(3), ..Default::default() };
        assert!(emit_mango(&managed(&sized, &[])).text.contains("border_px=3"), "Mango's key is border_px");
        assert!(out.text.contains("window_rule=is_floating:1,app_id:^org\\.mandara\\.studio$"));
        assert!(out.text.contains("bind=SUPER+ALT,space,spawn_shell,mandara msg launcher toggle"));
        assert!(!out.text.contains("bind=SUPER,space,"));
        assert_eq!(out.skipped.len(), 3, "{:?}", out.skipped);
        assert!(out.text.contains("XF86AudioPlay,spawn") != out.text.contains("XF86AudioPause,spawn"), "Play and Pause are one key to MangoWM: only one is written");
        assert!(out.text.contains("bind=NONE,XF86AudioMute,spawn_shell,mandara msg audio mute-all toggle"), "new installs: the mute key mutes all sound");
        assert!(out.text.contains("bind=NONE,XF86MonBrightnessUp,spawn_shell,mandara msg brightness up"), "laptop brightness keys work out of the box");
        assert!(out.text.contains("bind=NONE,XF86MonBrightnessDown,spawn_shell,mandara msg brightness down"));
        assert!(!emit_mango(&managed(&cfg, &["xf86monbrightnessup"])).text.contains("XF86MonBrightnessUp,spawn"), "your own brightness key wins");
        assert!(!emit_mango(&managed(&cfg, &["xf86audiomute"])).text.contains("XF86AudioMute,spawn"), "your own mute key wins");
    }

    /// Runs MangoWM's own config check (`mango -p`) on `user` (which
    /// sources Mandara's drop-in, written next to it) in a private home and
    /// returns every warning and error it prints, or `None` when MangoWM
    /// isn't installed (CI).
    fn mango_check(user: &str, generated: &str) -> Option<Vec<String>> {
        std::process::Command::new("mango").arg("-v").output().ok()?;
        let dir = tempfile::tempdir().unwrap();
        let gen_path = dir.path().join("generated.conf");
        std::fs::write(&gen_path, generated).unwrap();
        let root = dir.path().join("config.conf");
        std::fs::write(&root, format!("{user}\nsource_optional={}\n", gen_path.display())).unwrap();
        let out = std::process::Command::new("mango").arg("-p").arg("-c").arg(&root).env_clear().env("HOME", dir.path()).env("XDG_RUNTIME_DIR", dir.path()).env("PATH", std::env::var_os("PATH").unwrap_or_default()).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        let plain: String = text.split('\u{1b}').enumerate().map(|(i, part)| {
                if i == 0 { part } else { part.split_once('m').map_or(part, |(_, rest)| rest)
                }
            }).collect();
        Some(plain.lines().filter(|l| l.contains("WARN") || l.contains("ERROR")).map(|l| l.to_string()).chain(plain.lines().filter(|l| l.trim_start().starts_with("File")).map(|l| l.to_string())).collect())
    }

    /// The Niri release the drop-in was last checked against with Niri's own
    /// parser (26.04).
    const NIRI_CHECKED: (u32, u32) = (26, 4);

    /// Everything the emitters can write, filled in.
    fn mango_kitchen_sink() -> CompositorConfig {
        let mut cfg = CompositorConfig { border_width: Some(2), autostart: true, ..Default::default() };
        cfg.window_rules.push(WindowRule {
            app_id: Some("^pavucontrol$".into()),
            title: Some("Volume".into()),
            floating: Some(true),
            global: Some(true),
            overlay: Some(false),
            unfocused_opacity: Some(0.9),
            focused_opacity: Some(1.0),
            no_blur: Some(true),
            no_shadow: Some(true),
            confine_pointer: Some(false),
            force_tiled_state: Some(true),
        });
        cfg.tag_rules.push(TagRule { id: 2, monitor: Some("DP-1".into()), layout: Some("scroller".into()), scroller_default_proportion: Some(0.6), master_factor: Some(0.55), master_count: Some(1) });
        cfg.outputs.push(crate::outputs::OutputConfig { name: "DP-1".into(), enabled: true, width: Some(2560), height: Some(1440), refresh_mhz: Some(144_000), scale: Some(1.25), x: Some(0), y: Some(0), ..Default::default() });
        cfg
    }

    /// The real compositor accepts Mandara's drop-in without a single
    /// warning, beside a config that binds the media keys itself (the case
    /// that once produced "Key binding conflict": Play and Pause share a
    /// key).
    #[test]
    fn mango_itself_accepts_the_drop_in() {
        let user = "bindl=NONE,XF86AudioPlay,spawn_shell,playerctl play-pause\nbindl=NONE,XF86AudioMute,spawn_shell,wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle\nbind=SUPER,space,spawn,fuzzel\nkey_mode=resize\nbind=NONE,Escape,setkeymode,default\nkey_mode=default";
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("config.conf");
        std::fs::write(&root, user).unwrap();
        let existing = mango_existing_chords(&root, &mango_keys::MangoKeys::us());
        for cfg in [CompositorConfig::default(), mango_kitchen_sink()] {
            let managed = ManagedConfig { existing_chords: existing.clone(), ..managed(&cfg, &[]) };
            let out = emit_mango(&managed);
            let Some(problems) = mango_check(user, &out.text) else {
                eprintln!("mango isn't installed; skipped");
                return;
            };
            assert!(problems.is_empty(), "mango -p complained:\n{}\n--- drop-in ---\n{}", problems.join("\n"), out.text);
        }
    }

    /// Every window animation style, written beside a full config, passes
    /// MangoWM's and Niri's own checkers (each skipped when not installed).
    #[test]
    fn window_animations_pass_the_compositors_checkers() {
        use crate::window_motion::{WindowMotion, WindowStyle};
        let user = "focused_opacity=1.0";
        let niri_user = "binds {\n    Mod+Space { spawn \"fuzzel\"; }\n}";
        let existing = parse_chords(Target::Niri, niri_user);
        let existing: Vec<&str> = existing.iter().map(String::as_str).collect();
        for style in WindowStyle::ALL {
            for speed in [0.25, 1.0, 3.0] {
                let mut cfg = mango_kitchen_sink();
                cfg.look.windows = WindowMotion { style, speed };
                let out = emit_mango(&managed(&cfg, &[]));
                assert_eq!(out.text.contains("animation_type_open="), style != WindowStyle::Compositor);
                if let Some(problems) = mango_check(user, &out.text) {
                    assert!(problems.is_empty(), "mango -p complained about {style:?}:\n{}\n--- drop-in ---\n{}", problems.join("\n"), out.text);
                }
                let out = emit_niri(&ManagedConfig { version: Some(NIRI_CHECKED), ..managed(&cfg, &existing) });
                if let Some(errors) = niri_check(niri_user, &out.text) {
                    assert!(errors.is_empty(), "niri validate failed for {style:?}:\n{errors}\n--- drop-in ---\n{}", out.text);
                }
                // Animations switched off: no window animations at all.
                cfg.look.animations = Some(false);
                assert!(!emit_mango(&managed(&cfg, &[])).text.contains("animation_type_open="));
                assert!(!emit_niri(&managed(&cfg, &existing)).text.contains("window-open"));
            }
        }
    }

    /// Niri's own check (`niri validate`), like [`mango_check`]: `Some(errors)`
    /// (empty when valid), or `None` when Niri isn't installed.
    fn niri_check(user: &str, generated: &str) -> Option<String> {
        std::process::Command::new("niri").arg("--version").output().ok()?;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mandara.kdl"), generated).unwrap();
        let root = dir.path().join("config.kdl");
        std::fs::write(&root, format!("{user}\ninclude \"mandara.kdl\"\n")).unwrap();
        let out = std::process::Command::new("niri").arg("validate").arg("-c").arg(&root).env("HOME", dir.path()).output().unwrap();
        Some(if out.status.success() { String::new() } else { String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr) })
    }

    /// Umbriel's own check (`umbriel config validate`), like
    /// [`mango_check`]: `Some(diagnostics)` (empty when valid), or `None`
    /// when Umbriel isn't installed.
    fn umbriel_check(user: &str, generated: &str) -> Option<String> {
        std::process::Command::new("umbriel").arg("--version").output().ok()?;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mandara.toml"), generated).unwrap();
        let root = dir.path().join("config.toml");
        std::fs::write(&root, format!("[include.optional]\nfiles = [\"mandara.toml\"]\n\n{user}\n")).unwrap();
        let out = std::process::Command::new("umbriel").args(["config", "validate", "-c"]).arg(&root).env_clear().env("HOME", dir.path()).env("XDG_RUNTIME_DIR", dir.path()).env("PATH", std::env::var_os("PATH").unwrap_or_default()).output().unwrap();
        Some(if out.status.success() { String::new() } else { String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr) })
    }

    #[test]
    fn umbriel_itself_accepts_the_drop_in() {
        let user = "[keybinds]\n\"XF86AudioPlay\" = \"spawn playerctl play-pause\"\n\"Mod+Space\" = \"spawn fuzzel\"";
        let existing = parse_chords(Target::Umbriel, user);
        let existing: Vec<&str> = existing.iter().map(String::as_str).collect();
        for cfg in [CompositorConfig::default(), mango_kitchen_sink()] {
            let out = emit_umbriel(&managed(&cfg, &existing));
            let Some(errors) = umbriel_check(user, &out.text) else {
                eprintln!("umbriel isn't installed; skipped");
                return;
            };
            assert!(errors.is_empty(), "umbriel config validate failed:\n{errors}\n--- drop-in ---\n{}", out.text);
        }
    }

    #[test]
    fn niri_itself_accepts_the_drop_in() {
        let user = "binds {\n    XF86AudioPlay allow-when-locked=true { spawn \"playerctl\" \"play-pause\"; }\n    Mod+Space { spawn \"fuzzel\"; }\n}";
        let existing = parse_chords(Target::Niri, user);
        let existing: Vec<&str> = existing.iter().map(String::as_str).collect();
        for cfg in [CompositorConfig::default(), mango_kitchen_sink()] {
            let out = emit_niri(&ManagedConfig { version: Some(NIRI_CHECKED), ..managed(&cfg, &existing) });
            let Some(errors) = niri_check(user, &out.text) else {
                eprintln!("niri isn't installed; skipped");
                return;
            };
            assert!(errors.is_empty(), "niri validate failed:\n{errors}\n--- drop-in ---\n{}", out.text);
        }
    }

    #[test]
    fn mango_conflicts_follow_keycodes_and_key_modes() {
        let dir = tempfile::tempdir().unwrap();
        let generated = Target::Mango.generated_file();
        std::fs::write(dir.path().join("bind.conf"), "bindl=NONE,XF86AudioPlay,spawn_shell,playerctl play-pause\nkey_mode=resize\nbind=SUPER,r,resizewin,+10,0\n").unwrap();
        let root = dir.path().join("config.conf");
        std::fs::write(&root, format!("bind=code:64,code:24,killclient\nsource=./bind.conf\nkey_mode=default\nsource_optional={}\n", generated.display())).unwrap();
        let keys = mango_keys::MangoKeys::us();
        let chords = mango_existing_chords(&root, &keys);
        let pause = Keybind::new(&[], "XF86AudioPause", BindAction::Mandara("media play-pause".into()));
        let alt_q = Keybind::new(&["alt"], "q", BindAction::Mandara("x".into()));
        let super_r = Keybind::new(&["super"], "r", BindAction::Mandara("x".into()));
        let ours = |b: &Keybind| {
            mango_chord(b.mods.iter().map(String::as_str), keys.canonical(&b.keysym(), false))
        };
        assert!(chords.contains(&ours(&pause)), "Pause lands on the Play key: {chords:?}");
        assert!(chords.contains(&ours(&alt_q)), "code:64 is Alt and code:24 is q");
        assert!(!chords.contains(&ours(&super_r)), "a resize-mode bind can't clash with a default-mode one");
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
        let spawn = |mods: &[&str], key: &str| {
            Keybind::new(mods, key, BindAction::Spawn("notify-send hi".into()))
        };
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
    fn umbriel_output() {
        let cfg = CompositorConfig::default();
        let out = emit_umbriel(&managed(&cfg, &["super+b"]));
        let doc: toml::Table = out.text.parse().unwrap_or_else(|e| panic!("{e}\n{}", out.text));
        assert!(out.text.contains("\"Super+space\" = \"spawn mandara msg launcher toggle\""), "{}", out.text);
        assert!(out.text.contains("\"XF86AudioMute\" = \"spawn mandara msg audio mute-all toggle\""));
        assert!(out.skipped.iter().any(|n| n.contains("already bound")), "Super+B is the user's: {:?}", out.skipped);
        assert_eq!(doc["colors"]["border"]["focused"].as_str(), Some("#5EEAD4FF"));
        assert_eq!(doc["colors"]["border"]["unfocused"].as_str(), Some("#1F2937FF"));
        let layers = doc["layer_rule"].as_array().unwrap();
        let shell = layers[0]["match"]["namespace"].as_array().unwrap();
        assert_eq!(shell.len(), SHELL_SURFACES.len());
        assert!(shell.iter().any(|n| n.as_str() == Some("^mandara-bar$")) && layers[0]["blur"].as_bool() == Some(false));
        assert_eq!(layers[1]["match"]["namespace"].as_str(), Some("^mandara-lock$"));
        let rules = doc["window_rule"].as_array().unwrap();
        assert_eq!(rules[0]["match"]["app_id"].as_str(), Some("^org\\.mandara\\.studio$"));
        assert_eq!(rules[0]["default_floating"].as_bool(), Some(true));
        // Nothing the user didn't set in Studio.
        for key in ["general", "layout", "appearance", "animation", "hot_corners", "input", "output"] {
            assert!(!doc.contains_key(key), "{key} written without a Studio setting\n{}", out.text);
        }
    }

    #[test]
    fn umbriel_look_rules_and_corners() {
        let mut cfg = CompositorConfig {
            autostart: true,
            border_width: Some(3),
            disable_builtin_hot_corners: true,
            look: CompositorLook { gaps_inner: Some(12), gaps_outer: Some(10), rounding: Some(14), blur: Some(true), shadows: Some(false), animations: Some(false), active_opacity: Some(1.0), inactive_opacity: Some(0.85), gaps_follow_bar: false, ..Default::default() },
            keybinds: vec![Keybind::new(&["hyper"], "x", BindAction::Spawn("foot".into()))],
            ..Default::default()
        };
        cfg.window_rules = vec![WindowRule { app_id: Some("^mpv$".into()), title: Some("a \"quoted\"\ttitle".into()), floating: Some(false), global: Some(true), no_blur: Some(true), no_shadow: Some(true), confine_pointer: Some(true), focused_opacity: Some(0.9), ..Default::default() }];
        let out = emit_umbriel(&managed(&cfg, &[]));
        let doc: toml::Table = out.text.parse().unwrap_or_else(|e| panic!("{e}\n{}", out.text));
        assert_eq!(doc["general"]["autostart"].as_array().unwrap()[0].as_str(), Some("mandara"));
        assert_eq!(doc["layout"]["gap"].as_integer(), Some(12));
        assert_eq!(doc["layout"]["struts"]["top"].as_integer(), Some(-2), "outer 10 = gap 12 - 2");
        assert_eq!((doc["appearance"]["border_width"].as_integer(), doc["appearance"]["corner_radius"].as_integer()), (Some(3), Some(14)));
        assert_eq!(doc["appearance"]["blur"]["enabled"].as_bool(), Some(true));
        assert_eq!(doc["appearance"]["shadow"]["enabled"].as_bool(), Some(false));
        assert_eq!(doc["animation"]["enabled"].as_bool(), Some(false));
        for corner in UMBRIEL_HOT_CORNERS {
            assert_eq!(doc["hot_corners"][corner]["enabled"].as_bool(), Some(false));
        }
        let rules = doc["window_rule"].as_array().unwrap();
        // Blur is opt-in per window: the look's blur adds a rule for all.
        assert!(rules.iter().any(|r| r.get("match").is_none() && r["blur"].as_bool() == Some(true)));
        assert!(rules.iter().any(|r| {
            r.get("match").and_then(|m| m.get("is_focused")).and_then(toml::Value::as_bool) == Some(false) && r["opacity"].as_float() == Some(0.85)
        }));
        let mpv = rules.iter().find(|r| {
                r.get("match").and_then(|m| m.get("app_id")).is_some_and(|a| a.as_str() == Some("^mpv$")) && r.get("default_pinned").is_some()
            }).unwrap();
        assert_eq!(mpv["match"]["title"].as_str(), Some("a \"quoted\"\ttitle"));
        assert_eq!((mpv["default_floating"].as_bool(), mpv["blur"].as_bool(), mpv["shadow"].as_bool(), mpv["confine_pointer"].as_bool()), (Some(false), Some(false), Some(false), Some(true)));
        assert!(rules.iter().any(|r| {
            r.get("match").and_then(|m| m.get("is_focused")).and_then(toml::Value::as_bool) == Some(true) && r["match"].get("app_id").is_some() && r["opacity"].as_float() == Some(0.9)
        }));
        // Umbriel binds Super, Ctrl, Alt and Shift only.
        assert!(out.skipped.iter().any(|n| n.contains("modifier this compositor can't bind")), "{:?}", out.skipped);
    }

    #[test]
    fn umbriel_include_is_added_to_the_toml() {
        let path = "/home/u/.config/mandara/generated/umbriel.toml";
        let files = |text: &str| -> Vec<String> { umbriel_includes(text) };
        // No include yet: a new commented `[include.optional]` table, the
        // user's text kept verbatim before it.
        let user = "# my config\n[general]\nautostart = [\"kitty\"] # terminal\n\n[[window_rule]]\nmatch.app_id = \"^mpv$\"\nopacity = 0.9\n";
        let out = umbriel_add_include(user, path).unwrap().unwrap();
        assert!(out.starts_with(user), "user content first, untouched:\n{out}");
        assert!(out.contains("# Mandara Shell (generated rules, binds and colors)\n[include.optional]"), "{out}");
        assert_eq!(files(&out), vec![path]);
        assert_eq!(umbriel_add_include(&out, path).unwrap(), None, "already included");
        // Existing optional list: appended to it, nothing else added.
        let listed = "[include.optional]\nfiles = [\n  \"~/.config/umbriel/extra.toml\",\n]\n\n[keybinds]\n\"Mod+T\" = \"spawn foot\"\n";
        let out = umbriel_add_include(listed, path).unwrap().unwrap();
        assert_eq!(files(&out), vec!["~/.config/umbriel/extra.toml".to_string(), path.to_string()]);
        assert_eq!(out.matches("[include.optional]").count(), 1, "{out}");
        assert!(out.contains("[keybinds]\n\"Mod+T\" = \"spawn foot\""));
        // Required includes only: an optional table beside them.
        let required = "[include]\nfiles = [\"appearance.toml\"]\n";
        let out = umbriel_add_include(required, path).unwrap().unwrap();
        out.parse::<toml::Table>().unwrap();
        assert_eq!(files(&out), vec!["appearance.toml".to_string(), path.to_string()]);
        // An inline table stays inline.
        let inline = "include = { files = [\"a.toml\"] }\n[general]\nmod_key = \"Super\"\n";
        let out = umbriel_add_include(inline, path).unwrap().unwrap();
        assert_eq!(files(&out), vec!["a.toml".to_string(), path.to_string()], "{out}");
        // Listed as a required include already: nothing to add.
        assert_eq!(umbriel_add_include(&format!("[include]\nfiles = [\"{path}\"]\n"), path).unwrap(), None);
        // Not TOML, or `include` not a table: refused, nothing written.
        assert!(umbriel_add_include("[general\n", path).is_err());
        assert!(umbriel_add_include("include = 3\n", path).is_err());
        // `$VAR` / `${VAR}` expand as Umbriel expands them.
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(files("[include]\nfiles = [\"${HOME}/a.toml\", \"$HOME/b.toml\"]\n"), vec![format!("{home}/a.toml"), format!("{home}/b.toml")]);
    }

    #[test]
    fn umbriel_include_install_keeps_a_backup_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        let user = "[general]\nmod_key = \"Super\"\n";
        std::fs::write(&file, user).unwrap();
        assert!(install_include_at(Target::Umbriel, &file).unwrap());
        let after = std::fs::read_to_string(&file).unwrap();
        assert!(after.starts_with(user) && include_installed_at(Target::Umbriel, &file), "{after}");
        assert!(!install_include_at(Target::Umbriel, &file).unwrap(), "second install is a no-op");
        assert_eq!(std::fs::read_to_string(dir.path().join("config.toml.mandara-backup")).unwrap(), user);
        // A broken file is refused before anything is written.
        std::fs::write(&file, "[general\n").unwrap();
        assert!(install_include_at(Target::Umbriel, &file).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "[general\n");
    }

    #[test]
    fn umbriel_binds_are_read_from_keybinds() {
        let text = "[general]\nmod_key = \"Super\"\n\"Mod+Q\" = \"not a bind\"\n\n[keybinds]\n\"Mod+Space\" = \"spawn fuzzel\" # launcher\n\"Mod+Shift+Return\" = { action = \"spawn kitty\", repeat = false }\n'Ctrl+Alt+Delete' = \"session-quit\"\n# \"Mod+W\" = \"spawn waypaper\"\n\"submap[resize],Escape\" = \"submap reset\"\n\"XF86AudioMute\" = \"spawn wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle\"\n\n[[window_rule]]\nmatch.app_id = \"^x$\"\n";
        assert_eq!(chords_by_line(Target::Umbriel, text), vec![(5, "super+space".to_string()), (6, "shift+super+return".to_string()), (7, "alt+ctrl+delete".to_string()), (10, "xf86audiomute".to_string())]);
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

    /// A user's sway config: variables, a bind block, a mode, flags.
    const SWAY_USER: &str = "set $mod Mod4\nset $menu fuzzel\nbindsym $mod+space exec $menu\nbindsym --locked XF86AudioPlay exec playerctl play-pause\nbindsym {\n    $mod+Shift+e exec swaynag -t warning -m Exit\n    Mod1+Tab focus next\n}\nmode \"resize\" {\n    bindsym $mod+b mode \"default\"\n}\nbindcode 133+24 kill\n";

    #[test]
    fn sway_binds_follow_variables_blocks_and_modes() {
        let binds = sway_binds(SWAY_USER);
        let chords: Vec<&str> = binds.iter().map(|b| b.chord.as_str()).collect();
        assert_eq!(chords, ["super+space", "xf86audioplay", "shift+super+e", "alt+tab"], "mode binds and bindcode are left out");
        assert_eq!(binds[0].command, "exec fuzzel", "variables substituted");
        assert_eq!((binds[0].line, binds[2].line), (2, 5));
        assert_eq!(parse_chords(Target::Sway, "# bindsym Mod4+q kill\nbindsym --release --no-warn Mod4+Ctrl+comma exec x\n"), vec!["ctrl+super+comma"]);
    }

    /// `$mod` set in the main config holds in the files it includes later,
    /// the way sway reads them.
    #[test]
    fn sway_existing_chords_carry_variables_into_includes() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir_all(d.join("config.d")).unwrap();
        std::fs::write(d.join("config"), "set $mod Mod4\nbindsym $mod+Return exec foot\ninclude config.d/*\n").unwrap();
        std::fs::write(d.join("config.d/50-menu.conf"), "set $menu wofi\nbindsym $mod+space exec $menu\nbindsym $mod+Shift+s exec grim\n").unwrap();
        let chords = sway_existing_chords(&d.join("config"));
        assert_eq!(chords.into_iter().collect::<Vec<_>>(), ["shift+super+s", "super+return", "super+space"]);
    }

    /// What sway 1.12 loaded for each line (`sway --validate --verbose`,
    /// its "Loading config from" lines, in a container).
    #[test]
    fn sway_includes_expand_like_sway() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        for f in ["conf.d/a/bindings.conf", "conf.d/b/bindings.conf", "conf.d/b/other.conf", "conf.d/a/file*.conf", "conf.d/b/file1.conf", "one.conf", "two.conf", "my dir/x.conf"] {
            std::fs::create_dir_all(d.join(f).parent().unwrap()).unwrap();
            std::fs::write(d.join(f), "").unwrap();
        }
        let config = d.join("config");
        let files = |line: &str| sway_include_files(&config, line);
        // A wildcard in a directory component.
        assert_eq!(files("conf.d/*/bindings.conf"), [d.join("conf.d/a/bindings.conf"), d.join("conf.d/b/bindings.conf")]);
        assert!(files("nothing/*/x.conf").is_empty(), "a pattern that matches nothing is skipped");
        // Every path of one include, quoted together or not.
        assert_eq!(files("one.conf two.conf"), [d.join("one.conf"), d.join("two.conf")]);
        assert_eq!(files(&format!("\"{0}/one.conf {0}/two.conf\"", d.display())), [d.join("one.conf"), d.join("two.conf")]);
        // An escaped blank keeps one word; quotes alone don't (sway drops
        // them before `wordexp` splits).
        assert_eq!(files("my\\ dir/x.conf"), [d.join("my dir/x.conf")]);
        assert_eq!(files("\"my\\ dir/x.conf\""), [d.join("my dir/x.conf")]);
        assert_eq!(files("\"my dir/x.conf\""), [d.join("my"), d.join("dir/x.conf")]);
        // An escaped wildcard is literal.
        assert_eq!(files("conf.d/\\*/bindings.conf"), [d.join("conf.d/*/bindings.conf")]);
        assert_eq!(files("conf.d/*/file\\*.conf"), [d.join("conf.d/a/file*.conf")]);
    }

    /// `set $mod Mod1` then `set $mod Mod4`: sway uses the latest value
    /// (checked in sway 1.12: "After replacement: bindsym Mod4+x").
    #[test]
    fn sway_variables_take_their_latest_value() {
        let binds = sway_binds("set $mod Mod1\nbindsym $mod+a exec a\nset $mod Mod4\nset $modShift Mod4+Shift\nbindsym $mod+x exec x\nbindsym $modShift+y exec y\n");
        assert_eq!(binds.iter().map(|b| b.chord.as_str()).collect::<Vec<_>>(), ["alt+a", "super+x", "shift+super+y"]);
    }

    #[test]
    fn sway_output() {
        let cfg = CompositorConfig { autostart: true, ..Default::default() };
        let existing = parse_chords(Target::Sway, SWAY_USER);
        let existing: Vec<&str> = existing.iter().map(String::as_str).collect();
        let out = emit_sway(&managed(&cfg, &existing));
        let t = &out.text;
        assert!(t.contains("\nexec mandara\n"), "{t}");
        assert!(t.contains("for_window [app_id=\"^org\\.mandara\\.studio$\"] floating enable\nfor_window [class=\"^org\\.mandara\\.studio$\"] floating enable"), "{t}");
        assert!(t.contains("client.focused #5eead4ff #5eead4ff #000000ff #93c5fdff #5eead4ff"), "dark text on a light accent: {t}");
        assert!(t.contains("client.unfocused #1f2937ff #1f2937ff #ffffffff #1f2937ff #1f2937ff"), "{t}");
        assert!(t.contains("bindsym Mod4+Alt+space exec mandara msg launcher toggle"), "{t}");
        assert!(t.contains("bindsym XF86AudioMute exec mandara msg audio mute-all toggle"), "{t}");
        assert!(!t.contains("bindsym Mod4+space ") && !t.contains("bindsym XF86AudioPlay "), "the user's binds win: {t}");
        assert!(out.skipped.iter().any(|n| n.contains("already bound")), "{:?}", out.skipped);
        // Nothing the user didn't set in Studio.
        for absent in ["gaps", "default_border", "input ", "seat ", "output ", "focus_follows_mouse"] {
            assert!(!t.contains(absent), "{absent} written without a Studio setting\n{t}");
        }
        let user = ManagedConfig { user_autostarts: true, ..managed(&cfg, &[]) };
        assert!(!emit_sway(&user).text.contains("\nexec "), "the user's config starts Mandara already");
    }

    #[test]
    fn sway_look_rules_and_quoting() {
        let mut cfg = CompositorConfig {
            border_width: Some(3),
            keybinds: vec![
                Keybind::new(&["super"], "x", BindAction::Spawn("notify-send a; notify-send 'b, c'".into())),
                Keybind::new(&["hyper"], "y", BindAction::Spawn("true".into())),
                Keybind::new(&["super"], "z", BindAction::Spawn("notify-send a; notify-send \"b\"".into())),
            ],
            ..Default::default()
        };
        cfg.look = CompositorLook { gaps_inner: Some(12), gaps_outer: Some(10), rounding: Some(9), blur: Some(true), shadows: Some(true), animations: Some(false), active_opacity: Some(0.9), inactive_opacity: Some(0.8), gaps_follow_bar: false, ..Default::default() };
        cfg.window_rules = vec![WindowRule { title: Some("Volume \"x\"\nexec rm".into()), floating: Some(true), global: Some(true), no_blur: Some(true), ..Default::default() }];
        let t = emit_sway(&managed(&cfg, &[])).text;
        assert!(t.contains("gaps inner 12\ngaps outer -2\n"), "the outer gap is on top of the inner one: {t}");
        assert!(t.contains("default_border pixel 3\ndefault_floating_border pixel 3"), "{t}");
        assert!(t.contains("for_window [title=\"Volume .x.exec rm\"] floating enable, sticky enable\n"), "{t}");
        assert!(!t.lines().any(|l| l.starts_with("exec rm")), "a title can't add a config line");
        assert!(t.contains("bindsym Mod4+x exec \"notify-send a; notify-send 'b, c'\"\n"), "{t}");
        assert!(!t.contains("bindsym Mod4+z") && t.contains("# Super+z → `notify-send a; notify-send \"b\"` skipped"), "no escape for a quote: {t}");
        assert!(t.contains("bindsym Mod3+y exec true"), "Hyper is xkb's Mod3: {t}");
        for absent in ["rounding", "blur", "shadow", "animation", "opacity", "Hyper"] {
            assert!(!t.contains(absent), "{absent}: sway has no such setting\n{t}");
        }
    }

    #[test]
    fn sway_include_lines_and_files() {
        assert_eq!(sway_include_line("/home/u/.config/mandara/generated/sway.conf"), "include /home/u/.config/mandara/generated/sway.conf");
        assert_eq!(sway_include_line("/home/u/My Config/sway.conf"), "include /home/u/My\\ Config/sway.conf");
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir_all(d.join("config.d")).unwrap();
        std::fs::create_dir_all(d.join("with space")).unwrap();
        for f in ["config.d/50-a.conf", "config.d/10-b.conf", "config.d/.hidden", "with space/x.conf"] {
            std::fs::write(d.join(f), "bindsym Mod4+q kill\n").unwrap();
        }
        let main = d.join("config");
        let text = "include config.d/*\ninclude with\\ space/x.conf\n# include nothing.conf\ninclude_not_this x\n";
        std::fs::write(&main, text).unwrap();
        let files = included_files(Target::Sway, &main, text);
        assert_eq!(files, vec![d.join("config.d/10-b.conf"), d.join("config.d/50-a.conf"), d.join("with space/x.conf")], "globs sorted, hidden files skipped, escapes undone");
        assert!(glob_match("*.conf", "a.conf") && glob_match("a?c", "abc") && !glob_match("*.conf", "a.kdl"));
    }

    #[test]
    fn sway_include_install_starts_from_the_system_config() {
        let dir = tempfile::tempdir().unwrap();
        let system = dir.path().join("etc-sway-config");
        std::fs::write(&system, "set $mod Mod4\ninclude /etc/sway/config.d/*\n").unwrap();
        let user = dir.path().join("sway/config");
        seed_sway_config(&user, &system).unwrap();
        assert!(seed_sway_config(&user, &system).is_err(), "an existing config is never overwritten");
        assert!(install_include_at(Target::Sway, &user).unwrap());
        let text = std::fs::read_to_string(&user).unwrap();
        assert!(text.starts_with("set $mod Mod4\ninclude /etc/sway/config.d/*\n"), "{text}");
        assert!(text.trim_end().ends_with(&Target::Sway.include_line()), "{text}");
        assert!(include_installed_at(Target::Sway, &user));
        assert!(!install_include_at(Target::Sway, &user).unwrap(), "idempotent");
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".i3")).unwrap();
        std::fs::write(home.join(".i3/config"), "").unwrap();
        assert_eq!(sway_user_config(&dir.path().join("cfg"), &home), home.join(".i3/config"), "sway's own search order");
        assert_eq!(sway_user_config(dir.path(), &home), user, "~/.config/sway/config comes before ~/.i3/config");
    }

    /// Sway's own check through the config doctor (`sway --validate
    /// --verbose`, headless) on `user` ending in Mandara's include line:
    /// every problem it reports, or `None` when sway isn't installed (CI's
    /// `compositor-checkers` job installs it).
    fn sway_check(user: &str, generated: &str) -> Option<Vec<String>> {
        let dir = tempfile::tempdir().unwrap();
        let dropin = dir.path().join("generated/sway.conf");
        std::fs::create_dir_all(dropin.parent().unwrap()).unwrap();
        std::fs::write(&dropin, generated).unwrap();
        let root = dir.path().join("config");
        std::fs::write(&root, format!("{user}\n{}\n", sway_include_line(&dropin.display().to_string()))).unwrap();
        let env = crate::doctor::CheckEnv { home: Some(dir.path().to_path_buf()), ..Default::default() };
        let report = crate::doctor::check_with(Target::Sway, &root, &env);
        if report.status == crate::doctor::CheckerStatus::NotInstalled {
            return None;
        }
        assert_eq!(report.status, crate::doctor::CheckerStatus::Ran, "{report:#?}");
        assert!(report.files.contains(&dropin), "sway read the drop-in through the include line: {:?}", report.files);
        Some(report.diagnostics.iter().map(|d| format!("{}: {}", d.place().unwrap_or_default(), d.message)).collect())
    }

    /// A user's River init: maps (one in a loop, which can't be read), a
    /// bar and Mandara started the usual way.
    const RIVER_INIT: &str = "#!/bin/sh\nriverctl map normal Super Space spawn 'fuzzel'\nriverctl map -repeat normal Super+Shift E exit\nriverctl map normal None XF86AudioPlay spawn \"playerctl play-pause\"\nriverctl map locked None XF86AudioMute spawn 'wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle'\nfor i in $(seq 1 9); do\n    riverctl map normal Super $i set-focused-tags $((1 << ($i - 1)))\ndone\nriverctl spawn waybar\nriverctl default-layout rivertile\nrivertile -view-padding 6 &\n";

    #[test]
    fn river_init_binds_and_autostart() {
        let chords: Vec<String> = river_binds(RIVER_INIT).into_iter().map(|b| b.chord).collect();
        assert_eq!(chords, ["super+space", "shift+super+e", "xf86audioplay"], "normal mode only; a loop variable can't be read");
        assert!(!river_starts_mandara(RIVER_INIT));
        assert!(river_starts_mandara("riverctl spawn /home/u/.local/bin/mandara\n"));
        assert!(river_starts_mandara("mandara &\n"));
        assert!(!river_starts_mandara("riverctl map normal Super B spawn 'mandara msg sidebar toggle'\n"), "a bind isn't a start");
        assert_eq!(river_spawn_line("/home/u/My Apps/mandara"), "riverctl spawn '/home/u/My Apps/mandara'");
        assert_eq!(shell_words("riverctl map normal Super Q spawn 'a b' \"c \\\"d\\\"\" # x"), ["riverctl", "map", "normal", "Super", "Q", "spawn", "a b", "c \"d\""]);
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_words(&shell_quote("it's")), ["it's"]);
    }

    #[test]
    fn regexes_become_river_globs() {
        assert_eq!(river_glob("^org\\.mandara\\.studio$").as_deref(), Some("org.mandara.studio"));
        assert_eq!(river_glob("^steam_app_.*").as_deref(), Some("steam_app_*"));
        assert_eq!(river_glob("Volume").as_deref(), Some("*Volume*"), "an unanchored regex matches anywhere");
        assert_eq!(river_glob(".*Firefox$").as_deref(), Some("*Firefox"));
        for unusable in ["^(a|b)$", "^a.c$", "^[ab]$", "^a+$", "", "^\\d$"] {
            assert_eq!(river_glob(unusable), None, "{unusable}");
        }
    }

    #[test]
    fn river_commands_skip_the_users_keys_and_undo_their_own() {
        let mut cfg = CompositorConfig { border_width: Some(2), ..Default::default() };
        cfg.window_rules.push(WindowRule { app_id: Some("^pavucontrol$".into()), title: Some("Volume".into()), floating: Some(true), global: Some(true), no_blur: Some(true), ..Default::default() });
        cfg.window_rules.push(WindowRule { app_id: Some("^(a|b)$".into()), floating: Some(true), ..Default::default() });
        let existing = parse_chords(Target::River, RIVER_INIT);
        let existing: Vec<&str> = existing.iter().map(String::as_str).collect();
        let (commands, skipped) = river_commands(&managed(&cfg, &existing));
        let lines: Vec<String> = commands.iter().map(|w| riverctl_line(w)).collect();
        let has = |l: &str| lines.iter().any(|x| x == l);
        assert!(has("riverctl border-width 2") && has("riverctl border-color-focused 0x5eead4ff") && has("riverctl border-color-urgent 0xf87171ff"), "{lines:#?}");
        assert!(has("riverctl rule-add -app-id org.mandara.studio float"), "{lines:#?}");
        assert!(has("riverctl rule-add -app-id pavucontrol -title '*Volume*' float") && has("riverctl rule-add -app-id pavucontrol -title '*Volume*' tags 4294967295"), "{lines:#?}");
        assert!(has("riverctl map normal Super+Alt space spawn 'mandara msg launcher toggle'"), "{lines:#?}");
        assert!(!lines.iter().any(|l| l.contains(" Super space ") || l.contains(" XF86AudioPlay ")), "the init's own keys win: {lines:#?}");
        assert!(skipped.iter().any(|s| s.contains("simple globs")) && skipped.iter().any(|s| s.contains("already bound")), "{skipped:?}");
        let text = emit_river(&managed(&cfg, &existing)).text;
        assert!(text.contains("#   riverctl spawn mandara"), "the line to add to init: {text}");
        assert_eq!(parse_river_record(&text), commands, "the record reads back");
        let undo = river_undo(&commands);
        assert!(undo.contains(&vec!["unmap".to_string(), "normal".into(), "Super+Alt".into(), "space".into()]));
        assert!(undo.contains(&vec!["rule-del".to_string(), "-app-id".into(), "pavucontrol".into(), "-title".into(), "*Volume*".into(), "tags".into()]));
        assert!(undo.iter().all(|u| u[0] == "unmap" || u[0] == "rule-del"), "settings are just set again: {undo:?}");
    }

    #[test]
    fn river_init_is_never_edited() {
        let dir = tempfile::tempdir().unwrap();
        let init = dir.path().join("init");
        std::fs::write(&init, RIVER_INIT).unwrap();
        let err = install_include_at(Target::River, &init).unwrap_err().to_string();
        assert!(err.contains("doesn't edit River's init") && err.contains("riverctl spawn"), "{err}");
        assert_eq!(std::fs::read_to_string(&init).unwrap(), RIVER_INIT);
        assert!(!include_installed_at(Target::River, &init));
        std::fs::write(&init, format!("{RIVER_INIT}riverctl spawn /usr/bin/mandara\n")).unwrap();
        assert!(include_installed_at(Target::River, &init), "init starts Mandara: nothing to add");
    }

    /// River has no config checker: every command Mandara would send goes
    /// to a headless River through riverctl (the same protocol), beside an
    /// init with the user's own maps; each must be accepted without a
    /// word, and taking Mandara's own binds and rules back must be too.
    #[test]
    fn river_itself_accepts_the_commands() {
        let existing = parse_chords(Target::River, RIVER_INIT);
        let existing: Vec<&str> = existing.iter().map(String::as_str).collect();
        let mut sink = mango_kitchen_sink();
        sink.keybinds.push(Keybind::new(&["super", "ctrl", "alt", "shift", "hyper", "mod5"], ",", BindAction::Mandara("launcher toggle".into())));
        sink.keybinds.push(Keybind::new(&["super"], "x", BindAction::Spawn("notify-send a; notify-send \"b, c\"".into())));
        sink.window_rules.push(WindowRule { app_id: Some("^steam_app_.*".into()), floating: Some(false), ..Default::default() });
        let mut all = Vec::new();
        for cfg in [CompositorConfig::default(), sink] {
            let (commands, _) = river_commands(&ManagedConfig { mandara_bin: "/home/u/.local/bin/mandara".into(), ..managed(&cfg, &existing) });
            all.extend(river_undo(&commands));
            all.extend(commands.clone());
            all.extend(river_undo(&commands));
            all.extend(commands);
        }
        let Some(failures) = crate::river_check::run_commands_with_init(RIVER_INIT, &all) else { return;
        };
        assert!(failures.is_empty(), "River rejected Mandara's commands:\n{}", failures.join("\n"));
    }

    /// Sway accepts Mandara's drop-in without an error or an overwritten
    /// bind, beside a config with its own binds.
    #[test]
    fn sway_itself_accepts_the_drop_in() {
        let existing = parse_chords(Target::Sway, SWAY_USER);
        let existing: Vec<&str> = existing.iter().map(String::as_str).collect();
        let mut sink = mango_kitchen_sink();
        sink.keybinds.push(Keybind::new(&["super", "ctrl", "alt", "shift", "hyper", "mod5"], ",", BindAction::Mandara("launcher toggle".into())));
        sink.keybinds.push(Keybind::new(&["super"], "x", BindAction::Spawn("notify-send a; notify-send 'b, c'".into())));
        sink.look = CompositorLook { gaps_inner: Some(4), gaps_outer: Some(16), ..Default::default() };
        for cfg in [CompositorConfig::default(), sink] {
            let out = emit_sway(&ManagedConfig { mandara_bin: "/home/u/.local/bin/mandara".into(), ..managed(&cfg, &existing) });
            let Some(problems) = sway_check(SWAY_USER, &out.text) else {
                eprintln!("sway isn't installed; skipped");
                return;
            };
            assert!(problems.is_empty(), "sway --validate complained:\n{}\n--- drop-in ---\n{}", problems.join("\n"), out.text);
        }
    }
}
