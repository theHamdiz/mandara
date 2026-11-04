//! The shortcuts cheatsheet's model: normalized chords split into keycaps
//! (`shift+super+e` → `Super` `Shift` `E`), what a bind line of MangoWM,
//! Niri, Hyprland, Umbriel, Sway or River does (`bind=SUPER,Q,killclient` → "Close
//! window"), and which group a bind belongs to (Mandara, windows,
//! workspaces / tags, apps, media & system, other).
//!
//! Pure functions over text: the shell reads the config files (through
//! [`mandara_config::conflicts::user_binds`]) once when the sheet opens and
//! feeds the binds here.

use mandara_config::compositor::Target;
use mandara_i18n::{t, tf, tk};
use serde::Serialize;

/// Modifiers in display order: `(normalized name, keycap label)`.
const MODS: &[(&str, &str)] = &[("super", tk("Super")), ("ctrl", tk("Ctrl")), ("alt", tk("Alt")), ("shift", tk("Shift")), ("hyper", tk("Hyper")), ("mod5", tk("AltGr"))];

/// Named keys (lowercase keysym or compositor spelling) → keycap label.
const KEY_LABELS: &[(&str, &str)] = &[
    ("return", tk("Enter")),
    ("kp_enter", tk("Enter")),
    ("space", tk("Space")),
    ("escape", tk("Esc")),
    ("backspace", tk("Backspace")),
    ("delete", tk("Del")),
    ("insert", tk("Ins")),
    ("tab", tk("Tab")),
    ("iso_left_tab", tk("Tab")),
    ("print", tk("PrtSc")),
    ("pause", tk("Pause")),
    ("menu", tk("Menu")),
    ("home", tk("Home")),
    ("end", tk("End")),
    ("prior", tk("PgUp")),
    ("page_up", tk("PgUp")),
    ("next", tk("PgDn")),
    ("page_down", tk("PgDn")),
    ("up", "↑"),
    ("down", "↓"),
    ("left", "←"),
    ("right", "→"),
    ("comma", ","),
    ("period", "."),
    ("slash", "/"),
    ("backslash", "\\"),
    ("minus", "-"),
    ("equal", "="),
    ("plus", "+"),
    ("bracketleft", "["),
    ("bracketright", "]"),
    ("semicolon", ";"),
    ("apostrophe", "'"),
    ("grave", "`"),
    ("xf86audioraisevolume", tk("Volume up")),
    ("xf86audiolowervolume", tk("Volume down")),
    ("xf86audiomute", tk("Mute")),
    ("xf86audiomicmute", tk("Mic mute")),
    ("xf86audioplay", tk("Play")),
    ("xf86audiopause", tk("Pause")),
    ("xf86audionext", tk("Next track")),
    ("xf86audioprev", tk("Previous track")),
    ("xf86audiostop", tk("Stop")),
    ("xf86monbrightnessup", tk("Brightness up")),
    ("xf86monbrightnessdown", tk("Brightness down")),
    ("xf86kbdbrightnessup", tk("Keyboard light up")),
    ("xf86kbdbrightnessdown", tk("Keyboard light down")),
    ("xf86poweroff", tk("Power")),
    ("xf86calculator", tk("Calculator")),
    ("mouse:272", tk("Left click")),
    ("mouse:273", tk("Right click")),
    ("mouse:274", tk("Middle click")),
    ("mouse_down", tk("Scroll down")),
    ("mouse_up", tk("Scroll up")),
    ("wheelscrolldown", tk("Scroll down")),
    ("wheelscrollup", tk("Scroll up")),
    ("wheelscrollleft", tk("Scroll left")),
    ("wheelscrollright", tk("Scroll right")),
    ("btn_left", tk("Left click")),
    ("btn_right", tk("Right click")),
    ("btn_middle", tk("Middle click")),
];

/// The keycaps of a normalized chord (`alt+ctrl+delete` → `Ctrl` `Alt`
/// `Del`): modifiers in a fixed order, then the key.
pub fn keycaps(chord: &str) -> Vec<String> {
    // Normalized chords spell `+` as `plus`, so splitting is safe.
    let mut mods: Vec<&str> = chord.split('+').filter(|p| !p.is_empty()).collect();
    let Some(key) = mods.pop() else { return Vec::new() };
    let mut caps: Vec<String> = MODS.iter().filter(|(name, _)| mods.contains(name)).map(|(_, label)| t(label).to_string()).collect();
    caps.push(key_label(key));
    caps
}

/// The keycap label of one key (`return` → `Enter`, `q` → `Q`,
/// `xf86audioraisevolume` → `Volume up`, `f5` → `F5`).
pub fn key_label(key: &str) -> String {
    let k = key.trim().to_ascii_lowercase();
    if let Some((_, label)) = KEY_LABELS.iter().find(|(name, _)| *name == k) {
        return t(label).to_string();
    }
    if k.chars().count() == 1 {
        return k.to_uppercase();
    }
    if k.len() > 1 && k.starts_with('f') && k[1..].chars().all(|c| c.is_ascii_digit()) {
        return k.to_uppercase();
    }
    if let Some(code) = k.strip_prefix("code:") {
        return tf("Key {code}", &[("code", &code)]);
    }
    if let Some(rest) = k.strip_prefix("xf86") {
        return title_case(rest);
    }
    if let Some(n) = k.strip_prefix("kp_") {
        return tf("Keypad {key}", &[("key", &key_label(n))]);
    }
    title_case(&k)
}

fn title_case(s: &str) -> String {
    let s = s.replace('_', " ");
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Where a bind sits in the sheet.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Group {
    Mandara,
    Windows,
    Workspaces,
    Apps,
    Media,
    Other,
}

impl Group {
    pub const ALL: [Group; 6] = [Group::Mandara, Group::Windows, Group::Workspaces, Group::Apps, Group::Media, Group::Other];

    /// Display name; `tags` names the workspace group for MangoWM.
    pub fn label(self, tags: bool) -> &'static str {
        match self {
            Group::Mandara => t("Mandara"),
            Group::Windows => t("Windows"),
            Group::Workspaces if tags => t("Tags"),
            Group::Workspaces => t("Workspaces"),
            Group::Apps => t("Apps"),
            Group::Media => t("Media & system"),
            Group::Other => t("Other"),
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Group::Mandara => "sparkles",
            Group::Windows => "window",
            Group::Workspaces => "grid",
            Group::Apps => "package",
            Group::Media => "volume-high",
            Group::Other => "keyboard",
        }
    }
}

/// What a bind does, read from its config text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BindText {
    /// The action as written, tidied (`spawn ghostty`, `focus-column-left`).
    pub action: String,
    /// The bind's own description (Hyprland `bindd`, Niri
    /// `hotkey-overlay-title`), when it has one.
    pub description: Option<String>,
}

/// Collapses whitespace runs to single spaces.
fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The action and description of one bind from its config `body` (see
/// [`mandara_config::conflicts::UserBind::body`]).
pub fn bind_text(target: Target, body: &str) -> BindText {
    let body = body.trim();
    match target {
        Target::KWin => BindText { action: squash(body), description: None },
        Target::Niri => niri_text(body),
        Target::Umbriel => {
            let value = body.split_once('=').map(|(_, v)| v.trim()).unwrap_or(body);
            BindText { action: squash(value.trim_matches(|c| c == '"' || c == '\'')), description: None }
        }
        // `bindsym [--flags] Mod4+q kill`, or `Mod4+q kill` inside a
        // `bindsym { … }` block.
        Target::Sway => {
            let words = body.strip_prefix("bindsym").unwrap_or(body).split_whitespace().skip_while(|w| w.starts_with("--")).skip(1);
            BindText { action: words.collect::<Vec<_>>().join(" "), description: None }
        }
        // `riverctl map [-flags] normal Super Q close` in River's init.
        Target::River => {
            let words = mandara_config::compositor::shell_words(body);
            let after_map = words.iter().position(|w| w == "map").map_or(&words[..], |at| &words[at + 1..]);
            // Options come before the mode: `-release`, `-repeat`, and
            // `-layout <index>`, whose value goes too (as in
            // `river_binds`). Words after the key are the action's own.
            let mut rest = after_map.iter().map(String::as_str).peekable();
            while let Some(flag) = rest.next_if(|w| w.starts_with('-')) {
                if flag == "-layout" {
                    rest.next();
                }
            }
            BindText { action: rest.skip(3).collect::<Vec<_>>().join(" "), description: None }
        }
        Target::Hyprland if body.contains("hl.bind(") => lua_text(body),
        Target::Hyprland | Target::Mango => {
            let Some((keyword, value)) = body.split_once('=') else { return BindText { action: squash(body), description: None } };
            let keyword = keyword.trim();
            // Hyprland `bindd` / `binded` …: a description after the key.
            let described = target == Target::Hyprland && keyword.strip_prefix("bind").is_some_and(|flags| flags.contains('d'));
            let fields: Vec<&str> = value.splitn(if described { 5 } else { 4 }, ',').map(str::trim).collect();
            let (description, rest) = if described { (fields.get(2).map(|d| d.to_string()), fields.get(3..).unwrap_or_default()) } else { (None, fields.get(2..).unwrap_or_default()) };
            BindText { action: squash(&rest.iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ")), description: description.filter(|d| !d.is_empty()) }
        }
    }
}

/// `Mod+T hotkey-overlay-title="Open a terminal" { spawn "alacritty"; }`.
fn niri_text(body: &str) -> BindText {
    let head = body.split('{').next().unwrap_or_default();
    let description = head.find("hotkey-overlay-title=").and_then(|at| {
        let rest = head[at + "hotkey-overlay-title=".len()..].trim_start().strip_prefix('"')?;
        rest.find('"').map(|end| rest[..end].to_string())
    });
    let inner = match (body.find('{'), body.rfind('}')) {
        (Some(open), Some(close)) if close > open => &body[open + 1..close],
        _ => "",
    };
    let action = inner.split(';').map(|s| squash(s.trim())).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("; ");
    BindText { action, description }
}

/// `hl.bind(mod .. " + Return", hl.exec("kitty"))` → `exec("kitty")`.
fn lua_text(body: &str) -> BindText {
    let Some(at) = body.find("hl.bind(") else { return BindText::default() };
    let args = &body[at + "hl.bind(".len()..];
    let (mut depth, mut quote) = (0i32, None::<char>);
    let mut start = None;
    let mut end = args.len();
    for (i, c) in args.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(' | '{' | '[') => depth += 1,
            (None, ')' | '}' | ']') if depth == 0 => {
                end = i;
                break;
            }
            (None, ')' | '}' | ']') => depth -= 1,
            (None, ',') if depth == 0 && start.is_none() => start = Some(i + 1),
            _ => {}
        }
    }
    let action = start.map(|s| squash(args[s..end.max(s)].trim().trim_start_matches("hl."))).unwrap_or_default();
    BindText { action, description: None }
}

/// The first word of an action, lowercased with `-` / `.` read as `_`,
/// and the rest (`focus-column-left` → `focus_column_left`).
fn verb(action: &str) -> (String, String) {
    let a = action.trim().trim_start_matches("hl.");
    let (head, rest) = match a.find(|c: char| c.is_whitespace() || c == '(') {
        Some(i) => (&a[..i], a[i..].trim_start_matches('(').trim_end_matches(')').trim()),
        None => (a, ""),
    };
    (head.to_ascii_lowercase().replace(['-', '.'], "_"), rest.trim_matches(|c| c == '"' || c == '\'').trim().to_string())
}

/// Programs that belong to media and system controls.
const SYSTEM_PROGRAMS: &[&str] = &[
    "wpctl", "pactl", "pamixer", "amixer", "playerctl", "brightnessctl", "light", "grim", "slurp", "grimblast", "hyprshot", "flameshot", "swappy", "swaylock",
    "hyprlock", "gtklock", "loginctl", "systemctl", "wlogout", "poweroff", "reboot", "notify_send", "makoctl", "dunstctl", "swaync_client",
];

/// The program a spawn / exec action runs (`spawn "sh" "-c" "…"` keeps `sh`).
fn program(rest: &str) -> String {
    let first = rest.split_whitespace().next().unwrap_or_default().trim_matches(|c| c == '"' || c == '\'' || c == ',');
    first.rsplit('/').next().unwrap_or(first).to_ascii_lowercase().replace('-', "_")
}

fn is_spawn(v: &str) -> bool {
    matches!(v, "spawn" | "exec" | "spawn_sh" | "exec_cmd" | "spawn_shell")
}

/// Which group a bind belongs to, from its action and key.
pub fn group_of(action: &str, chord: &str) -> Group {
    let lower = action.to_ascii_lowercase();
    if lower.contains("mandara") {
        let words = lower.split("msg").nth(1).unwrap_or(&lower).trim_start().to_string();
        return mandara_group(&words);
    }
    let key = chord.rsplit('+').next().unwrap_or_default();
    let (v, rest) = verb(action);
    if is_spawn(&v) {
        return if SYSTEM_PROGRAMS.contains(&program(&rest).as_str()) || key.starts_with("xf86") || key == "print" { Group::Media } else { Group::Apps };
    }
    if key.starts_with("xf86") || key == "print" {
        return Group::Media;
    }
    let has = |needles: &[&str]| needles.iter().any(|n| v.contains(n));
    if has(&["quit", "exit", "reload", "dpms", "power_off", "screenshot", "suspend", "lock", "logout", "pass", "keymode", "submap", "inhibit", "debug"]) {
        Group::Media
    } else if has(&["workspace", "tag", "monitor", "mon", "overview", "layout", "scroller", "special"]) || matches!(v.as_str(), "view" | "toggleview" | "viewtoleft" | "viewtoright" | "viewcrossmon") {
        Group::Workspaces
    } else if has(&[
        "kill", "close", "focus", "move", "swap", "exchange", "resize", "float", "fullscreen", "maximize", "minimize", "consume", "expel", "center", "pin", "group",
        "column", "window", "ratio", "width", "height", "stack", "client", "zoom", "cycle", "split", "pseudo", "master", "gaps", "opacity",
    ]) {
        Group::Windows
    } else {
        Group::Other
    }
}

/// Mandara's own commands (`audio volume up`) by area.
fn mandara_group(words: &str) -> Group {
    let first = words.split_whitespace().next().unwrap_or_default();
    match first {
        "audio" | "media" | "brightness" | "screenshot" | "record" | "idle" | "keep-awake" | "session" | "lock" | "night-light" => Group::Media,
        "switcher" | "window" | "windows" => Group::Windows,
        "workspace" | "workspaces" | "tag" => Group::Workspaces,
        _ => Group::Mandara,
    }
}

/// An action's first argument: a quoted one whole (Sway's
/// `workspace "dev ops"` names "dev ops"), otherwise its first word.
fn first_arg(rest: &str) -> String {
    let rest = rest.trim_start();
    if let Some(q) = rest.chars().next().filter(|c| *c == '"' || *c == '\'')
        && let Some((name, _)) = rest[1..].split_once(q)
    {
        return name.to_string();
    }
    rest.split_whitespace().next().unwrap_or_default().trim_matches(|c| c == '"' || c == ',').to_string()
}

/// A friendly description for common compositor actions across MangoWM,
/// Niri and Hyprland (`killclient` → "Close window"); `None` when the
/// action is better shown as written.
pub fn describe(action: &str) -> Option<String> {
    let (v, rest) = verb(action);
    // Sway: `workspace number 3`, `move container to workspace number 3`.
    let sway_move = (v == "move").then(|| ["container to workspace ", "window to workspace ", "to workspace "].iter().find_map(|p| rest.strip_prefix(p)).map(str::to_string)).flatten();
    let rest = sway_move.clone().unwrap_or(rest);
    let rest = rest.strip_prefix("--no-auto-back-and-forth ").map_or_else(|| rest.clone(), str::to_string);
    let rest = rest.strip_prefix("number ").map_or_else(|| rest.clone(), str::to_string);
    let arg = first_arg(&rest);
    // Sway takes the rest of a `workspace` command as the name, quoted or
    // not (`workspace "dev ops"` and `workspace dev ops` both name
    // "dev ops"; checked in sway 1.12).
    let ws_name = rest.trim().trim_matches(|c| c == '"' || c == '\'').trim().to_string();
    let dir = || match arg.to_ascii_lowercase().as_str() {
        "l" | "left" => Some(t("left")),
        "r" | "right" => Some(t("right")),
        "u" | "up" | "t" => Some(t("up")),
        "d" | "down" | "b" => Some(t("down")),
        _ => None,
    };
    let suffix_dir = |s: &str| {
        if s.ends_with("left") {
            Some(t("left"))
        } else if s.ends_with("right") {
            Some(t("right"))
        } else if s.ends_with("up") {
            Some(t("up"))
        } else if s.ends_with("down") {
            Some(t("down"))
        } else {
            None
        }
    };
    Some(match v.as_str() {
        _ if is_spawn(&v) => {
            let name = rest.trim().trim_matches(|c| c == '"' || c == '\'');
            let name = if program(name) == "sh" || program(name) == "bash" { name.rsplit(['"', '\'']).find(|s| !s.trim().is_empty() && !s.contains("-c")).unwrap_or(name).trim() } else { name };
            if name.is_empty() {
                return None;
            }
            tf("Run {program}", &[("program", &name)])
        }
        "killclient" | "killactive" | "close_window" | "kill" | "close" => t("Close window").to_string(),
        "quit" | "exit" => t("Quit the compositor").to_string(),
        "reload_config" | "load_config_file" | "reload" => t("Reload the config").to_string(),
        "togglefloating" | "toggle_window_floating" | "setfloating" | "toggle_float" => t("Toggle floating").to_string(),
        "floating" if arg == "toggle" => t("Toggle floating").to_string(),
        "togglefullscreen" | "fullscreen" | "fullscreen_window" | "togglefakefullscreen" | "toggle_fullscreen" => t("Toggle fullscreen").to_string(),
        "togglemaximizescreen" | "maximize_column" | "maximize" | "maximize_window_to_edges" => t("Maximize").to_string(),
        "minimized" | "minimize" | "toggle_minimize" => t("Minimize").to_string(),
        "toggleoverview" | "toggle_overview" | "overview" => t("Overview").to_string(),
        "focusdir" | "movefocus" | "focus" if dir().is_some() => tf("Focus {direction}", &[("direction", &dir()?)]),
        "exchange_client" | "swapwindow" | "movewindow" | "move" if sway_move.is_none() && dir().is_some() => tf("Move window {direction}", &[("direction", &dir()?)]),
        "move" if sway_move.is_some() && !ws_name.is_empty() => tf("Move window to {place} {n}", &[("place", &t("workspace")), ("n", &ws_name)]),
        "focusstack" | "cyclenext" => t("Focus next window").to_string(),
        "workspace" if !ws_name.is_empty() => tf("Go to {place} {n}", &[("place", &t("workspace")), ("n", &ws_name)]),
        "view" | "focus_workspace" if !arg.is_empty() => tf("Go to {place} {n}", &[("place", &if v == "view" { t("tag") } else { t("workspace") }), ("n", &arg)]),
        "tag" | "movetoworkspace" | "movetoworkspacesilent" | "move_column_to_workspace" | "move_window_to_workspace" if !arg.is_empty() => {
            tf("Move window to {place} {n}", &[("place", &if v == "tag" { t("tag") } else { t("workspace") }), ("n", &arg)])
        }
        "toggleview" if !arg.is_empty() => tf("Also show tag {n}", &[("n", &arg)]),
        "toggletag" if !arg.is_empty() => tf("Also put the window on tag {n}", &[("n", &arg)]),
        "setlayout" | "switch_layout" | "layout" => t("Switch layout").to_string(),
        "power_off_monitors" | "dpms" => t("Turn the screens off").to_string(),
        "screenshot" | "screenshot_screen" | "screenshot_window" => t("Take a screenshot").to_string(),
        "show_hotkey_overlay" => t("Show the shortcuts overlay").to_string(),
        s if s.starts_with("focus_column") || s.starts_with("focus_window") || s.starts_with("focus_monitor") => tf("Focus {direction}", &[("direction", &suffix_dir(s)?)]),
        s if s.starts_with("move_column") || s.starts_with("move_window") => tf("Move window {direction}", &[("direction", &suffix_dir(s)?)]),
        _ => return None,
    })
}

/// A short name for a Mandara command a key runs (`panel toggle clipboard`
/// → "Clipboard history"); `None` for commands without one, which then
/// show their catalog summary.
pub fn mandara_label(words: &str) -> Option<&'static str> {
    let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(t(match words.as_str() {
        "launcher toggle" => tk("Launcher"),
        "sidebar toggle" => tk("Sidebar"),
        "session menu toggle" => tk("Power menu"),
        "screenshot region" => tk("Screenshot of a region"),
        "screenshot screen" => tk("Screenshot of the screen"),
        "screenshot window" => tk("Screenshot of a window"),
        "record region" => tk("Record a region"),
        "record stop" => tk("Stop recording"),
        "record start" => tk("Start recording"),
        "record toggle" => tk("Start / stop recording"),
        "record screen" => tk("Record the screen"),
        "record window" => tk("Record a window"),
        "record pause-toggle" => tk("Pause or resume recording"),
        "record mic" | "record mic toggle" => tk("Mute the recorded microphone"),
        "panel toggle clipboard" => tk("Clipboard history"),
        "theme mode toggle" => tk("Light / dark"),
        "wallpaper picker toggle" => tk("Wallpaper picker"),
        "switcher next" => tk("Next window (Alt+Tab)"),
        "switcher prev" => tk("Previous window"),
        "audio mute-all toggle" => tk("Mute all sound"),
        "audio volume up" => tk("Volume up"),
        "audio volume down" => tk("Volume down"),
        "audio mic mute" => tk("Mute the microphone"),
        "brightness up" => tk("Brightness up"),
        "brightness down" => tk("Brightness down"),
        "media play-pause" => tk("Play / pause"),
        "media next" => tk("Next track"),
        "media previous" => tk("Previous track"),
        "pomodoro toggle" => tk("Pomodoro"),
        "keybinds toggle" => tk("Shortcuts cheatsheet"),
        "studio" => tk("Mandara Studio"),
        _ => return None,
    }))
}

/// One line of the cheatsheet.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Entry {
    /// Normalized chord (`shift+super+e`).
    pub chord: String,
    /// Keycap labels, modifiers first.
    pub keys: Vec<String>,
    /// The action as configured (a Mandara command, a program, a
    /// compositor dispatcher).
    pub action: String,
    /// What it does, in words.
    pub description: String,
    pub group: Group,
    /// `mandara` for Mandara's own binds, else the config file.
    pub source: String,
    /// 1-based line in `source` (user binds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// Lowercased text the search matches against.
    #[serde(skip)]
    pub haystack: String,
}

impl Entry {
    pub fn new(chord: String, action: String, description: String, group: Group, source: String, line: Option<usize>) -> Self {
        let keys = keycaps(&chord);
        let haystack = format!("{} {} {} {} {} {}", keys.join(" "), chord, action, description, group.label(false), source).to_lowercase();
        Self { chord, keys, action, description, group, source, line, haystack }
    }

    /// Every word of `query` appears somewhere in the entry.
    pub fn matches(&self, query: &str) -> bool {
        query.split_whitespace().all(|w| self.haystack.contains(&w.to_lowercase()))
    }
}

/// An entry for a user's bind (`body` from the config file).
pub fn user_entry(target: Target, chord: &str, body: &str, source: String, line: usize) -> Entry {
    let text = bind_text(target, body);
    let description = text.description.clone().or_else(|| describe(&text.action)).unwrap_or_else(|| text.action.clone());
    let group = group_of(&text.action, chord);
    Entry::new(chord.to_string(), text.action, description, group, source, Some(line))
}

/// Groups `entries` in sheet order (each group's entries sorted by
/// description), dropping empty groups.
pub fn grouped(entries: &[Entry]) -> Vec<(Group, Vec<&Entry>)> {
    Group::ALL
        .into_iter()
        .filter_map(|g| {
            let mut list: Vec<&Entry> = entries.iter().filter(|e| e.group == g).collect();
            list.sort_by(|a, b| a.description.to_lowercase().cmp(&b.description.to_lowercase()).then_with(|| a.chord.cmp(&b.chord)));
            (!list.is_empty()).then_some((g, list))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_bind_has_a_short_name() {
        for b in mandara_config::compositor::default_keybinds() {
            if let mandara_config::compositor::BindAction::Mandara(words) = &b.action {
                assert!(mandara_label(words).is_some(), "no label for `{words}`");
            }
        }
        assert_eq!(mandara_label("panel  toggle clipboard"), Some("Clipboard history"));
        assert_eq!(mandara_label("weather refresh"), None);
    }

    #[test]
    fn chords_split_into_ordered_keycaps() {
        assert_eq!(keycaps("alt+ctrl+delete"), ["Ctrl", "Alt", "Del"]);
        assert_eq!(keycaps("shift+super+e"), ["Super", "Shift", "E"]);
        assert_eq!(keycaps("super+return"), ["Super", "Enter"]);
        assert_eq!(keycaps("xf86audioraisevolume"), ["Volume up"]);
        assert_eq!(keycaps("super+mouse:272"), ["Super", "Left click"]);
        assert_eq!(keycaps("alt+f4"), ["Alt", "F4"]);
        assert_eq!(keycaps("super+prior"), ["Super", "PgUp"]);
        assert_eq!(keycaps("ctrl+comma"), ["Ctrl", ","]);
        assert_eq!(keycaps("super+code:24"), ["Super", "Key 24"]);
        assert_eq!(keycaps("super+xf86launch1"), ["Super", "Launch1"]);
        assert!(keycaps("").is_empty());
    }

    #[test]
    fn bind_text_reads_each_compositor() {
        assert_eq!(bind_text(Target::Mango, "bind=SUPER,Return,spawn,ghostty").action, "spawn ghostty");
        assert_eq!(bind_text(Target::Mango, "bind=SUPER,q,killclient,").action, "killclient");
        assert_eq!(bind_text(Target::Mango, "bind=SUPER,1,view,1").action, "view 1");
        let d = bind_text(Target::Hyprland, "bindd = SUPER, Q, Open a terminal, exec, kitty --single");
        assert_eq!((d.action.as_str(), d.description.as_deref()), ("exec kitty --single", Some("Open a terminal")));
        assert_eq!(bind_text(Target::Hyprland, "bind = SUPER SHIFT, 2, movetoworkspace, 2").action, "movetoworkspace 2");
        let n = bind_text(Target::Niri, "Mod+T hotkey-overlay-title=\"Open a Terminal\" { spawn \"alacritty\"; }");
        assert_eq!((n.action.as_str(), n.description.as_deref()), ("spawn \"alacritty\"", Some("Open a Terminal")));
        assert_eq!(bind_text(Target::Niri, "    Mod+Shift+E {\n        quit;\n    }").action, "quit");
        assert_eq!(bind_text(Target::Hyprland, "hl.bind(mod .. \" + Return\", hl.exec(\"kitty\"))").action, "exec(\"kitty\")");
        assert_eq!(bind_text(Target::Umbriel, "\"Mod+T\" = \"spawn foot\"").action, "spawn foot");
        assert_eq!(bind_text(Target::Sway, "bindsym --locked --no-warn $mod+Return exec foot -e btop").action, "exec foot -e btop");
        assert_eq!(bind_text(Target::Sway, "Mod4+Shift+q kill").action, "kill", "a line of a `bindsym {{ … }}` block");
        assert_eq!(bind_text(Target::River, "riverctl map -repeat normal Super+Shift Return spawn 'foot -e btop'").action, "spawn foot -e btop");
        // `-layout` takes a value: both go, the action stays whole.
        assert_eq!(bind_text(Target::River, "riverctl map -layout 1 normal Super Q close").action, "close");
        assert_eq!(bind_text(Target::River, "riverctl map -release -layout 0 normal Super H send-layout-cmd rivertile main-ratio -0.05").action, "send-layout-cmd rivertile main-ratio -0.05");
        for (action, want) in [("close", "Close window"), ("toggle-float", "Toggle floating"), ("toggle-fullscreen", "Toggle fullscreen"), ("spawn foot", "Run foot")] {
            assert_eq!(describe(action).as_deref(), Some(want), "river `{action}`");
        }
        for (action, want) in [
            ("exec foot", "Run foot"),
            ("kill", "Close window"),
            ("workspace number 3", "Go to workspace 3"),
            ("move container to workspace number 3", "Move window to workspace 3"),
            // A quoted name keeps every word.
            ("workspace \"dev ops\"", "Go to workspace dev ops"),
            ("workspace --no-auto-back-and-forth number \"2 web\"", "Go to workspace 2 web"),
            ("move container to workspace \"dev ops\"", "Move window to workspace dev ops"),
            ("workspace dev ops", "Go to workspace dev ops"),
            ("focus left", "Focus left"),
            ("move right", "Move window right"),
            ("floating toggle", "Toggle floating"),
            ("fullscreen toggle", "Toggle fullscreen"),
            ("layout toggle split", "Switch layout"),
            ("reload", "Reload the config"),
        ] {
            assert_eq!(describe(action).as_deref(), Some(want), "sway `{action}`");
        }
    }

    #[test]
    fn binds_land_in_sensible_groups() {
        assert_eq!(group_of("mandara msg launcher toggle", "super+space"), Group::Mandara);
        assert_eq!(group_of("mandara msg audio volume up", "xf86audioraisevolume"), Group::Media);
        assert_eq!(group_of("mandara msg switcher next", "alt+tab"), Group::Windows);
        assert_eq!(group_of("exec mandara msg sidebar toggle", "super+b"), Group::Mandara);
        assert_eq!(group_of("spawn ghostty", "super+return"), Group::Apps);
        assert_eq!(group_of("exec wpctl set-volume @DEFAULT_SINK@ 5%+", "xf86audioraisevolume"), Group::Media);
        assert_eq!(group_of("spawn grim", "print"), Group::Media);
        assert_eq!(group_of("killclient", "super+q"), Group::Windows);
        assert_eq!(group_of("focus-column-left", "super+h"), Group::Windows);
        assert_eq!(group_of("view 3", "super+3"), Group::Workspaces);
        assert_eq!(group_of("toggletag 3", "ctrl+super+3"), Group::Workspaces);
        assert_eq!(group_of("focus-workspace-down", "super+j"), Group::Workspaces);
        assert_eq!(group_of("movetoworkspace 2", "shift+super+2"), Group::Workspaces);
        assert_eq!(group_of("quit", "shift+super+e"), Group::Media);
        assert_eq!(group_of("xyzzy", "super+z"), Group::Other);
    }

    #[test]
    fn common_actions_get_words() {
        assert_eq!(describe("killclient").as_deref(), Some("Close window"));
        assert_eq!(describe("spawn ghostty").as_deref(), Some("Run ghostty"));
        assert_eq!(describe("spawn \"alacritty\"").as_deref(), Some("Run alacritty"));
        assert_eq!(describe("focusdir left").as_deref(), Some("Focus left"));
        assert_eq!(describe("focus-column-right").as_deref(), Some("Focus right"));
        assert_eq!(describe("view 2").as_deref(), Some("Go to tag 2"));
        assert_eq!(describe("workspace 4").as_deref(), Some("Go to workspace 4"));
        assert_eq!(describe("movetoworkspace 4").as_deref(), Some("Move window to workspace 4"));
        assert_eq!(describe("something-custom"), None);
    }

    #[test]
    fn entries_search_and_group_in_order() {
        let a = user_entry(Target::Mango, "super+return", "bind=SUPER,Return,spawn,ghostty", "/x/config.conf".into(), 3);
        let b = user_entry(Target::Mango, "super+q", "bind=SUPER,q,killclient,", "/x/config.conf".into(), 4);
        let c = Entry::new("super+space".into(), "launcher toggle".into(), "Open the launcher".into(), Group::Mandara, "mandara".into(), None);
        assert!(a.matches("ghostty") && a.matches("SUPER enter") && !a.matches("firefox"));
        assert!(b.matches("close win"));
        let entries = vec![a, b, c];
        let groups: Vec<Group> = grouped(&entries).into_iter().map(|(g, _)| g).collect();
        assert_eq!(groups, [Group::Mandara, Group::Windows, Group::Apps]);
    }
}
