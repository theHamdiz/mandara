//! Compositor conflicts — what other setups leave in a user's MangoWM, Niri,
//! Hyprland, Umbriel or Sway config that would fight Mandara — and how to
//! retire them safely.
//!
//! Lessons from real migrations: an old launcher (fuzzel, rofi…) or wallpaper
//! switcher bound to Super+Space / Ctrl+Shift+W silently wins over Mandara's
//! binds; a second bar (waybar…) draws over the top edge; a second
//! notification daemon (mako, dunst…) keeps the D-Bus name; a wallpaper
//! daemon (swww, hyprpaper…) paints over Mandara's background. This module
//!
//! - scans the user's config **and every file it sources/includes**,
//! - reports each conflict with file and line,
//! - retires chosen conflicts by commenting the lines out with a
//!   `mandara-retired:` marker after a timestamped backup of every touched
//!   file (`~/.local/share/mandara/retired/<stamp>/…`), and
//! - can restore any backup.
//!
//! Callers validate with [`validate`] after changing anything and restore the
//! backup if the compositor rejects the result.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::compositor::{BindAction, CompositorConfig, Keybind, Target, chord, chords_by_line, included_files, is_generated, target_chord, toml_code, toml_header, toml_line_key};

/// What a conflicting autostart program does.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    Bar,
    Notifications,
    Wallpaper,
    Shell,
    Clipboard,
}

impl Role {
    pub fn describe(self) -> &'static str {
        match self {
            Role::Bar => "another bar (Mandara draws its own)",
            Role::Notifications => "another notification daemon (it holds the notifications D-Bus name)",
            Role::Wallpaper => "another wallpaper daemon (it paints over Mandara's background)",
            Role::Shell => "another desktop shell",
            Role::Clipboard => "another clipboard history (Mandara keeps its own; optional)",
        }
    }

    /// Clipboard watchers only duplicate work; everything else conflicts.
    pub fn is_conflict(self) -> bool {
        self != Role::Clipboard
    }
}

/// Programs that overlap with Mandara when started alongside it.
pub const KNOWN: &[(&str, Role)] = &[
    ("waybar", Role::Bar),
    // Sway's own bar, started by a `bar { … }` block.
    ("swaybar", Role::Bar),
    ("ironbar", Role::Bar),
    ("yambar", Role::Bar),
    ("sfwbar", Role::Bar),
    ("nwg-panel", Role::Bar),
    ("hyprpanel", Role::Bar),
    ("eww", Role::Bar),
    ("ags", Role::Shell),
    ("astal", Role::Shell),
    ("quickshell", Role::Shell),
    ("qs", Role::Shell),
    ("caelestia", Role::Shell),
    ("caelestia-shell", Role::Shell),
    ("noctalia-shell", Role::Shell),
    ("noctalia", Role::Shell),
    ("dms", Role::Shell),
    ("wayle", Role::Shell),
    ("mako", Role::Notifications),
    ("dunst", Role::Notifications),
    ("swaync", Role::Notifications),
    ("fnott", Role::Notifications),
    ("nwg-dock", Role::Bar),
    ("nwg-dock-hyprland", Role::Bar),
    ("hyprdock", Role::Bar),
    ("swww-daemon", Role::Wallpaper),
    ("swww", Role::Wallpaper),
    // swww's new name.
    ("awww-daemon", Role::Wallpaper),
    ("awww", Role::Wallpaper),
    ("waypaper", Role::Wallpaper),
    ("swaybg", Role::Wallpaper),
    ("hyprpaper", Role::Wallpaper),
    ("wpaperd", Role::Wallpaper),
    ("mpvpaper", Role::Wallpaper),
    ("cliphist", Role::Clipboard),
    ("clipman", Role::Clipboard),
];

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Kind {
    /// A chord Mandara binds is bound to something else. `replaces` names
    /// the tool Mandara takes over from (an old launcher, wallpaper picker,
    /// screenshot tool, power menu or another shell) — those are worth
    /// retiring. Other binds are the user's own: Mandara keeps them and
    /// skips its bind.
    Keybind { chord: String, mandara: String, replaces: Option<String> },
    /// A program started with the compositor overlaps with Mandara.
    Autostart { program: String, role: Role },
    /// A bind on a key Mandara doesn't use opens a tool Mandara replaces
    /// (an old launcher on Alt+Space…): retiring it re-binds that key to the
    /// Mandara equivalent, so muscle memory keeps working.
    Takeover { chord: String, tool: String, mandara: String },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Conflict {
    pub file: PathBuf,
    /// 0-based line numbers (several for multi-line Niri binds).
    pub lines: Vec<usize>,
    /// The first offending line, trimmed.
    pub text: String,
    pub kind: Kind,
}

impl Conflict {
    /// Worth retiring (clipboard duplicates are only informational).
    pub fn is_conflict(&self) -> bool {
        match &self.kind {
            Kind::Keybind { replaces, .. } => replaces.is_some(),
            Kind::Autostart { role, .. } => role.is_conflict(),
            Kind::Takeover { .. } => true,
        }
    }

    pub fn summary(&self) -> String {
        match &self.kind {
            Kind::Keybind { chord, mandara, replaces: Some(tool) } => format!("{chord} opens {tool}, which Mandara replaces (`{mandara}`)"),
            Kind::Keybind { chord, mandara, replaces: None } => format!("{chord} is yours — kept; Mandara skips its `{mandara}` bind"),
            Kind::Autostart { program, role } => format!("{program} starts with the compositor: {}", role.describe()),
            Kind::Takeover { chord, tool, mandara } => format!("{chord} opens {tool}; Mandara can take this key over (`{mandara}`)"),
        }
    }
}

/// The user's config file and every file it sources / includes (except
/// Mandara's own generated drop-in), in discovery order.
pub fn config_files(target: Target) -> Vec<PathBuf> {
    config_files_from(target, &target.user_config())
}


pub fn config_files_from(target: Target, root: &Path) -> Vec<PathBuf> {
    let generated = target.generated_file();
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(file) = stack.pop() {
        if !seen.insert(file.clone()) || file == generated || is_generated(&file) || !file.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let mut children: Vec<PathBuf> = included_files(target, &file, &text);
        children.reverse();
        out.push(file);
        stack.extend(children);
    }
    out
}

fn comment_prefix(target: Target, file: &Path) -> &'static str {
    match target {
        Target::Niri => "//",
        Target::Hyprland if file.extension().is_some_and(|e| e == "lua") => "--",
        _ => "#",
    }
}

fn is_comment(target: Target, line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('#') || t.starts_with("//") || (target == Target::Hyprland && t.starts_with("--"))
}

/// Executable names on a command line (paths and launch wrappers stripped).
fn programs_in(command: &str) -> Vec<String> {
    command
        .split(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ';' || c == '&' || c == '|' || c == '(' || c == ')' || c == ',')
        .filter(|w| !w.is_empty() && !w.starts_with('-'))
        .map(|w| w.rsplit('/').next().unwrap_or(w).to_string())
        .collect()
}

/// The command started by an autostart line, if it is one.
fn autostart_command(target: Target, line: &str) -> Option<String> {
    let t = line.trim();
    match target {
        Target::KWin => None,
        // MangoWM's keyword is `exec_once`; `exec-once` is kept for older configs.
        Target::Mango => ["exec_once=", "exec-once=", "exec="].iter().find_map(|p| t.strip_prefix(p)).map(str::to_string),
        Target::Hyprland => {
            if let Some(rest) = t.strip_prefix("exec-once").or_else(|| t.strip_prefix("exec")) {
                return rest.trim_start().strip_prefix('=').map(|r| r.trim().to_string());
            }
            // Lua: the command string of `hl.exec_cmd("…")` (autostarts
            // run inside `hl.on("hyprland.start", …)`).
            t.split_once("hl.exec_cmd(").and_then(|(_, rest)| lua_string(rest))
        }
        Target::Niri => ["spawn-at-startup ", "spawn-sh-at-startup "].iter().find_map(|p| t.strip_prefix(p)).map(str::to_string),
        // A TOML array that can span lines: read whole-file by
        // [`umbriel_autostarts`].
        Target::Umbriel => None,
        // `riverctl spawn 'cmd'`, or a program started in the background
        // (`waybar &`) by River's init script.
        Target::River => {
            let words = crate::compositor::shell_words(t);
            match words.as_slice() {
                [ctl, spawn, cmd, ..] if ctl.ends_with("riverctl") && spawn == "spawn" => Some(cmd.clone()),
                [first, ..] if t.ends_with('&') && !first.ends_with("riverctl") => Some(t.trim_end_matches('&').trim().to_string()),
                _ => None,
            }
        }
        // `exec [--no-startup-id] cmd` (start-up) and `exec_always cmd`
        // (every reload).
        Target::Sway => ["exec ", "exec_always "].iter().find_map(|p| t.strip_prefix(p)).map(|r| r.trim_start().trim_start_matches("--no-startup-id").trim().to_string()),
    }
}

/// What a Sway config starts without an `exec` line: a `bar { … }` block
/// runs swaybar (or its `swaybar_command`) and `output … bg …` runs swaybg.
/// `(first line, last line, program, role)`, 0-based.
fn sway_builtin_autostarts(lines: &[&str]) -> Vec<(usize, usize, String, Role)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        if is_comment(Target::Sway, t) {
            i += 1;
            continue;
        }
        let first = t.split_whitespace().next().unwrap_or_default();
        if first == "bar" && t.ends_with('{') {
            let mut depth = 0i32;
            let mut end = i;
            let mut program = "swaybar".to_string();
            for (j, l) in lines.iter().enumerate().skip(i) {
                let l = l.trim();
                if !is_comment(Target::Sway, l) {
                    depth += l.matches('{').count() as i32 - l.matches('}').count() as i32;
                    if let Some(cmd) = l.strip_prefix("swaybar_command") {
                        program = programs_in(cmd).into_iter().next().unwrap_or(program);
                    }
                }
                end = j;
                if depth <= 0 {
                    break;
                }
            }
            let role = KNOWN.iter().find(|(n, _)| *n == program).map_or(Role::Bar, |(_, r)| *r);
            out.push((i, end, program, role));
            i = end + 1;
            continue;
        }
        if first == "output" && t.split_whitespace().skip(2).take(1).any(|w| w == "bg" || w == "background") {
            out.push((i, i, "swaybg".into(), Role::Wallpaper));
        }
        i += 1;
    }
    out
}

/// The TOML string literals on one line (comments skipped), as written
/// (quotes included) and as values.
fn toml_literals(line: &str) -> Vec<(String, String)> {
    let code = toml_code(line);
    let mut out = Vec::new();
    let mut chars = code.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if c != '"' && c != '\'' {
            continue;
        }
        let mut value = String::new();
        let mut end = None;
        while let Some((i, ch)) = chars.next() {
            match ch {
                '\\' if c == '"' => {
                    if let Some((_, esc)) = chars.next() {
                        value.push(match esc {
                            'n' => '\n',
                            't' => '\t',
                            other => other,
                        });
                    }
                }
                q if q == c => {
                    end = Some(i);
                    break;
                }
                ch => value.push(ch),
            }
        }
        if let Some(end) = end {
            out.push((code[start..=end].to_string(), value));
        }
    }
    out
}

/// Umbriel `general.autostart` entries with their 0-based line: `[general]`
/// `autostart = [ … ]` (or a top-level `general.autostart`), one array that
/// may span several lines.
pub(crate) fn umbriel_autostarts(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut section = String::new();
    let mut in_array = false;
    for (i, line) in text.lines().enumerate() {
        if !in_array && let Some(h) = toml_header(line) {
            section = h.to_string();
            continue;
        }
        let code = toml_code(line);
        if !in_array {
            let key = toml_line_key(line).unwrap_or_default();
            let starts = (section == "general" && key == "autostart") || (section.is_empty() && key == "general.autostart");
            if !starts {
                continue;
            }
            in_array = true;
        }
        let value_part = if code.contains('=') && toml_line_key(line).is_some() { code.split_once('=').map(|(_, v)| v).unwrap_or(code) } else { code };
        out.extend(toml_literals(value_part).into_iter().map(|(_, v)| (i, v)));
        // The array ends at a `]` outside strings.
        let unquoted: String = toml_literals(value_part).iter().fold(value_part.to_string(), |acc, (lit, _)| acc.replacen(lit.as_str(), "", 1));
        if unquoted.contains(']') {
            in_array = false;
        }
    }
    out
}

/// Retires one Umbriel autostart entry: its string is taken out of the
/// array line (the rest of the list stays as written) and noted in a
/// trailing `mandara-retired:` comment.
fn umbriel_retire_autostart(line: &str, program: &str) -> String {
    let mut kept = toml_code(line).to_string();
    let mut retired = Vec::new();
    for (lit, value) in toml_literals(line) {
        if programs_in(&value).iter().any(|p| p == program)
            && let Some(at) = kept.find(&lit)
        {
            // Take a following comma with it, else a preceding one.
            let after = kept[at + lit.len()..].trim_start();
            let (from, to) = if after.starts_with(',') {
                let comma = kept[at + lit.len()..].find(',').map(|c| at + lit.len() + c + 1).unwrap_or(at + lit.len());
                let spaces = kept[comma..].len() - kept[comma..].trim_start().len();
                (at, comma + spaces)
            } else {
                let before = kept[..at].trim_end();
                let from = if before.ends_with(',') { before.len() - 1 } else { at };
                (from, at + lit.len())
            };
            kept.replace_range(from..to, "");
            retired.push(lit);
        }
    }
    // The line's own comment stays, after the marker.
    let own = line[toml_code(line).len()..].trim_start_matches('#').trim();
    let own = if own.is_empty() { String::new() } else { format!(" · {own}") };
    let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
    let (kept, sep) = if kept.trim().is_empty() { (indent.as_str(), "") } else { (kept.trim_end(), " ") };
    format!("{kept}{sep}# mandara-retired: {}{own}", retired.join(", "))
}

/// The first Lua string literal in `s` (`"waybar") end)` → `waybar`).
fn lua_string(s: &str) -> Option<String> {
    let start = s.find(['"', '\''])?;
    let q = s.as_bytes()[start] as char;
    let len = s[start + 1..].find(q)?;
    Some(s[start + 1..start + 1 + len].replace("\\\"", "\""))
}

/// Other desktop shells: Quickshell-based ones (Caelestia, Noctalia v4, Dux /
/// DMS), Noctalia v5's native `noctalia`, and AGS-style shells.
const SHELLS: &[&str] = &["caelestia", "qs", "quickshell", "noctalia-shell", "noctalia", "dms", "ags", "astal", "hyprpanel"];

/// Tools whose job a Mandara action takes over, by action.
fn replaced_tool(mandara_action: &str, bind: &str) -> Option<String> {
    const LAUNCHERS: &[&str] = &["fuzzel", "rofi", "wofi", "tofi", "anyrun", "walker", "ulauncher", "bemenu", "bemenu-run", "dmenu", "dmenu_run", "wmenu", "wmenu-run", "nwg-drawer", "nwg-menu", "kickoff", "sirula", "onagre", "gauntlet", "vicinae"];
    const SCREENSHOT: &[&str] = &["grim", "slurp", "hyprshot", "grimblast", "flameshot", "satty", "swappy", "wayshot", "gpu-screen-recorder", "wf-recorder"];
    const SESSION: &[&str] = &["wlogout", "wleave", "nwg-bar", "nwgbar"];
    const CLIPBOARD: &[&str] = &["cliphist", "clipman"];
    let words = programs_in(bind);
    let has = |list: &[&str]| words.iter().find(|w| list.contains(&w.as_str())).cloned();
    if let Some(shell) = has(SHELLS) {
        return Some(shell);
    }
    let a = mandara_action;
    if a.starts_with("launcher") {
        return has(LAUNCHERS);
    }
    if a.starts_with("wallpaper") {
        return words.iter().find(|w| w.contains("wallpaper") || w.contains("wallpick") || *w == "waypaper").cloned();
    }
    if a.starts_with("screenshot") || a.starts_with("record") {
        return has(SCREENSHOT);
    }
    if a.starts_with("session") {
        return has(SESSION);
    }
    if a.contains("clipboard") {
        return has(CLIPBOARD).or_else(|| has(LAUNCHERS));
    }
    None
}

/// The Mandara action replacing a tool, for binds on keys Mandara doesn't
/// use.
fn takeover_action(bind: &str) -> Option<(String, &'static str)> {
    for action in ["launcher toggle", "wallpaper picker toggle", "screenshot region", "session menu toggle", "panel toggle clipboard"] {
        if let Some(tool) = replaced_tool(action, bind) {
            // Another shell's IPC can't be mapped to one action safely.
            if SHELLS.contains(&tool.as_str()) {
                return None;
            }
            return Some((tool, action));
        }
    }
    None
}

/// Mandara binds to add for retired takeover conflicts.
pub fn takeover_keybinds(conflicts: &[Conflict]) -> Vec<Keybind> {
    conflicts
        .iter()
        .filter_map(|c| match &c.kind {
            Kind::Takeover { chord, mandara, .. } => {
                let mut parts: Vec<&str> = chord.split('+').collect();
                let key = parts.pop()?.to_string();
                Some(Keybind::new(&parts, &key, BindAction::Mandara(mandara.clone())))
            }
            _ => None,
        })
        .collect()
}

/// Chord → Mandara action, for the binds Mandara manages.
fn mandara_chords(target: Target, cfg: &CompositorConfig) -> Vec<(String, String)> {
    cfg.keybinds.iter().map(|k| (target_chord(target, k), k.action.shell_command_with("mandara").trim_start_matches("mandara msg ").to_string())).collect()
}

/// Line ranges of Niri binds: (first line, last line, chord).
fn niri_binds(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_binds = false;
    let mut open: Option<(usize, String, i32)> = None;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.starts_with("//") {
            continue;
        }
        if depth == 0 && line.starts_with("binds") {
            in_binds = true;
        } else if in_binds && depth == 1 && open.is_none()
            && let Some(first) = line.split_whitespace().next().filter(|w| !w.starts_with('}')) {
                let mut pieces: Vec<&str> = first.split('+').collect();
                let key = pieces.pop().unwrap_or_default();
                open = Some((i, chord(pieces.into_iter(), key), depth));
            }
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        if let Some((start, ch, at)) = open.clone()
            && depth <= at {
                out.push((start, i, ch));
                open = None;
            }
        if depth <= 0 {
            in_binds = false;
            depth = 0;
        }
    }
    out
}

/// Keybind chords bound in one config file's `text`: (first line, last
/// line, chord), 0-based, commented-out binds skipped. Hyprland is parsed
/// as a whole file, so `$mainMod`-style variables (hyprlang) and `local`
/// strings joined into `hl.bind(…)` chords (0.57 Lua) resolve; Niri binds
/// span their `{ … }` block.
pub fn file_binds(target: Target, text: &str) -> Vec<(usize, usize, String)> {
    file_binds_after(target, "", text)
}

/// [`file_binds`] of a file read after `earlier`: the Sway `set $var …`
/// lines of the files before it in the tree (a `$mod` set in the main
/// config holds in the files it includes).
fn file_binds_after(target: Target, earlier: &str, text: &str) -> Vec<(usize, usize, String)> {
    match target {
        Target::Niri => niri_binds(text),
        Target::Sway if !earlier.is_empty() => {
            let offset = earlier.lines().count();
            file_binds_after(target, "", &format!("{earlier}{text}")).into_iter().filter(|(i, ..)| *i >= offset).map(|(i, j, c)| (i - offset, j - offset, c)).collect()
        }
        _ => {
            let lines: Vec<&str> = text.lines().collect();
            chords_by_line(target, text).into_iter().filter(|(i, _)| lines.get(*i).is_some_and(|l| !is_comment(target, l))).map(|(i, c)| (i, i, c)).collect()
        }
    }
}

/// A file's Sway `set $var …` lines, for the files read after it.
fn sway_sets(target: Target, text: &str) -> String {
    if target != Target::Sway {
        return String::new();
    }
    text.lines().filter(|l| l.trim_start().starts_with("set ")).map(|l| format!("{}\n", l.trim())).collect()
}

/// One keybind from the user's own compositor config (Studio's shortcut
/// conflicts and the shortcuts cheatsheet).
#[derive(Clone, Debug, PartialEq)]
pub struct UserBind {
    /// Normalized chord (`shift+super+e`).
    pub chord: String,
    pub file: PathBuf,
    /// 1-based line number of the bind's first line.
    pub line: usize,
    /// The bind's first line, trimmed.
    pub text: String,
    /// Every line of the bind (a Niri bind spans its `{ … }` block), with
    /// Hyprland `$variables` substituted so `exec, $menu` names the program.
    pub body: String,
    /// A tool Mandara replaces (old launcher, wallpaper picker…).
    pub replaces: Option<String>,
}

/// Hyprland `$name = value` and Sway `set $name value` definitions in
/// `lines`, longest names first so `$mainModShift` is substituted before `$mainMod`.
fn hypr_vars(target: Target, lines: &[&str]) -> Vec<(String, String)> {
    let defs: Vec<(String, String)> = match target {
        Target::Hyprland => lines.iter().filter_map(|l| l.trim().strip_prefix('$')?.split_once('=').map(|(n, val)| (format!("${}", n.trim()), val.trim().to_string()))).collect(),
        // Sway: `set $name value`; a later `set` of the same name replaces
        // the earlier one.
        Target::Sway => lines.iter().filter_map(|l| l.trim().strip_prefix("set ")?.trim().split_once(char::is_whitespace).filter(|(n, _)| n.starts_with('$')).map(|(n, val)| (n.to_string(), val.trim().to_string()))).collect(),
        _ => return Vec::new(),
    };
    let mut v = Vec::new();
    for (name, value) in defs {
        crate::compositor::set_var(&mut v, &name, &value);
    }
    v
}

/// Binds in `text` (one file) with their line numbers: the same whole-file
/// parse as the conflict scanner ([`file_binds`]), so Hyprland's `$mainMod`
/// variables and Lua `hl.bind` chords built from `local`s count.
pub fn binds_in(target: Target, file: &Path, text: &str) -> Vec<UserBind> {
    binds_in_after(target, file, text, "")
}

/// [`binds_in`] for a file read after `earlier` (see [`file_binds_after`]).
fn binds_in_after(target: Target, file: &Path, text: &str, earlier: &str) -> Vec<UserBind> {
    let lines: Vec<&str> = text.lines().collect();
    if target == Target::Sway {
        let offset = earlier.lines().count();
        return crate::compositor::sway_binds(&format!("{earlier}{text}"))
            .into_iter()
            .filter(|b| b.line >= offset)
            .map(|b| {
                let start = b.line - offset;
                UserBind { chord: b.chord, file: file.to_path_buf(), line: start + 1, text: lines[start].trim().to_string(), body: b.expanded, replaces: None }
            })
            .collect();
    }
    let vars = hypr_vars(target, &earlier.lines().chain(lines.iter().copied()).collect::<Vec<_>>());
    file_binds_after(target, earlier, text)
        .into_iter()
        .map(|(start, end, chord)| {
            let block = lines.get(start..=end.min(lines.len().saturating_sub(1))).map(|l| l.join("\n")).unwrap_or_default();
            let body = vars.iter().fold(block, |b, (n, val)| b.replace(n.as_str(), val));
            UserBind { chord, file: file.to_path_buf(), line: start + 1, text: lines.get(start).map_or_else(String::new, |l| l.trim().to_string()), body, replaces: None }
        })
        .collect()
}

fn sway_user_binds(files: &[PathBuf]) -> Vec<UserBind> {
    crate::compositor::sway_config_binds(files).into_iter().map(|(file, bind)| {
        let text = std::fs::read_to_string(&file).ok().and_then(|text| text.lines().nth(bind.line).map(|line| line.trim().to_string())).unwrap_or_default();
        UserBind { chord: bind.chord, file, line: bind.line + 1, text, body: bind.expanded, replaces: None }
    }).collect()
}

/// Every bind in the user's config tree for `target` (never Mandara's
/// drop-in), keyed by chord: later files win, like the compositor. Binds
/// whose tool Mandara replaces carry it in [`UserBind::replaces`].
pub fn user_binds(target: Target, cfg: &CompositorConfig) -> std::collections::BTreeMap<String, UserBind> {
    let mut map = std::collections::BTreeMap::new();
    let mut earlier = String::new();
    let files = config_files(target);
    if target == Target::Sway {
        map.extend(sway_user_binds(&files).into_iter().map(|bind| (bind.chord.clone(), bind)));
    }
    for file in files.into_iter().filter(|_| target != Target::Sway) {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        for b in binds_in_after(target, &file, &text, &earlier) {
            map.insert(b.chord.clone(), b);
        }
        earlier.push_str(&sway_sets(target, &text));
    }
    for c in scan(target, cfg) {
        if let Kind::Keybind { chord, replaces: Some(tool), .. } = c.kind
            && let Some(b) = map.get_mut(&chord)
        {
            b.replaces = Some(tool);
        }
    }
    map
}

/// Conflicts in the user's config tree for `target` against `cfg`.
pub fn scan(target: Target, cfg: &CompositorConfig) -> Vec<Conflict> {
    scan_files(target, cfg, &config_files(target))
}

pub fn scan_files(target: Target, cfg: &CompositorConfig, files: &[PathBuf]) -> Vec<Conflict> {
    let sway = if target == Target::Sway { sway_user_binds(files) } else { Vec::new() };
    let ours = mandara_chords(target, cfg);
    let mut out = Vec::new();
    let mut earlier = String::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        let lines: Vec<&str> = text.lines().collect();
        // Keybinds.
        let binds = if target == Target::Sway { sway.iter().filter(|b| b.file == *file).cloned().collect() } else { binds_in_after(target, file, &text, &earlier) };
        earlier.push_str(&sway_sets(target, &text));
        for bind in binds {
            let start = bind.line - 1;
            let end = start + bind.body.lines().count().saturating_sub(1);
            let (ch, body) = (bind.chord, bind.body);
            if body.contains("mandara") {
                continue;
            }
            if let Some((_, action)) = ours.iter().find(|(c, _)| *c == ch) {
                let replaces = replaced_tool(action, &body);
                out.push(Conflict { file: file.clone(), lines: (start..=end).collect(), text: lines[start].trim().to_string(), kind: Kind::Keybind { chord: ch, mandara: action.clone(), replaces } });
            } else if let Some((tool, action)) = takeover_action(&body) {
                out.push(Conflict { file: file.clone(), lines: (start..=end).collect(), text: lines[start].trim().to_string(), kind: Kind::Takeover { chord: ch, tool, mandara: action.to_string() } });
            }
        }
        // Autostarted programs.
        let autostarts: Vec<(usize, String)> = if target == Target::Umbriel {
            umbriel_autostarts(&text)
        } else {
            lines.iter().enumerate().filter(|(_, l)| !is_comment(target, l)).filter_map(|(i, l)| autostart_command(target, l).map(|cmd| (i, cmd))).collect()
        };
        if target == Target::Sway {
            for (start, end, program, role) in sway_builtin_autostarts(&lines) {
                out.push(Conflict { file: file.clone(), lines: (start..=end).collect(), text: lines[start].trim().to_string(), kind: Kind::Autostart { program, role } });
            }
        }
        for (i, cmd) in autostarts {
            let l = lines[i];
            if cmd.contains("mandara") {
                continue;
            }
            let programs = programs_in(&cmd);
            if let Some((name, role)) = KNOWN.iter().find(|(name, _)| programs.iter().any(|p| p == name)) {
                out.push(Conflict { file: file.clone(), lines: vec![i], text: l.trim().to_string(), kind: Kind::Autostart { program: (*name).to_string(), role: *role } });
            }
            // A login script (`exec_once=~/.config/mango/autostart.sh`)
            // starts programs too: look inside it.
            if let Some(script) = startup_script(&cmd)
                && !out.iter().any(|c| c.file == script)
            {
                out.extend(scan_script(&script));
            }
        }
    }
    out
}

/// Programs that only run the script after them.
const SCRIPT_WRAPPERS: &[&str] = &["sh", "bash", "dash", "zsh", "exec", "env", "setsid", "nohup"];

/// Largest login script that is read (anything bigger is not a script).
const MAX_SCRIPT_BYTES: u64 = 1 << 20;

/// The shell script an autostart command runs, if it is one in the home
/// folder (`~/…`, `$HOME/…` or an absolute path under it).
fn startup_script(command: &str) -> Option<PathBuf> {
    startup_script_in(command, &std::env::var_os("HOME").map(PathBuf::from)?)
}

fn startup_script_in(command: &str, home: &Path) -> Option<PathBuf> {
    // Words unquoted first: Niri writes argv as KDL strings
    // (`spawn-at-startup "sh" "-c" "~/autostart.sh"`).
    let first = command.split_whitespace().map(|w| w.trim_matches(|c| c == '"' || c == '\'')).find(|w| !w.is_empty() && !w.starts_with('-') && !SCRIPT_WRAPPERS.contains(w))?;
    let path = if let Some(rest) = first.strip_prefix("~/").or_else(|| first.strip_prefix("$HOME/")) { home.join(rest) } else { PathBuf::from(first) };
    (path.is_absolute() && path.starts_with(home) && is_script(&path)).then_some(path)
}

/// A small shell script: `.sh`, or a `#!` line naming a shell.
fn is_script(path: &Path) -> bool {
    if !path.is_file() || std::fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX) > MAX_SCRIPT_BYTES {
        return false;
    }
    path.extension().is_some_and(|e| e == "sh")
        || std::fs::read_to_string(path).ok().and_then(|t| t.lines().next().map(str::to_string)).is_some_and(|l| l.starts_with("#!") && l.contains("sh"))
}

/// Known programs a login script starts, one conflict per line.
fn scan_script(script: &Path) -> Vec<Conflict> {
    let Ok(text) = std::fs::read_to_string(script) else { return Vec::new() };
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with('#') && !l.contains("mandara") && !l.trim_start().starts_with(SCRIPT_RETIRED))
        .filter_map(|(i, l)| {
            let programs = programs_in(l);
            KNOWN.iter().find(|(name, _)| programs.iter().any(|p| p == name)).map(|(name, role)| Conflict {
                file: script.to_path_buf(),
                lines: vec![i],
                text: l.trim().to_string(),
                kind: Kind::Autostart { program: (*name).to_string(), role: *role },
            })
        })
        .collect()
}

/// How a retired line starts in a shell script: `:` is the shell's no-op, so
/// the line stays a command and an `if` / `case` body never ends up empty.
const SCRIPT_RETIRED: &str = ": # mandara-retired:";

/// Where retire backups go.
pub fn backups_root() -> PathBuf {
    crate::paths::data_dir().join("retired")
}

/// Comments out the conflicts' lines (marker `mandara-retired:`), after
/// copying every touched file into a new timestamped backup folder, which is
/// returned (with a `manifest.txt` listing original paths).
pub fn retire(target: Target, conflicts: &[Conflict]) -> Result<PathBuf> {
    retire_into(target, conflicts, &backups_root())
}

/// [`retire`] with its backups under `root`. Folder names carry the time in
/// nanoseconds, so two retires in the same second never share a folder.
pub fn retire_into(target: Target, conflicts: &[Conflict], root: &Path) -> Result<PathBuf> {
    if target == Target::River && !conflicts.is_empty() {
        anyhow::bail!("Mandara doesn't edit River's init: remove or comment these lines yourself:\n{}", conflicts.iter().map(|c| format!("  {}:{}: {}", c.file.display(), c.lines.first().map_or(0, |l| l + 1), c.text)).collect::<Vec<_>>().join("\n"));
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let dir = root.join(format!("{}-{:09}-{}", now.as_secs(), now.subsec_nanos(), format!("{target:?}").to_lowercase()));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let files: BTreeSet<&PathBuf> = conflicts.iter().map(|c| &c.file).collect();
    let mut manifest = String::new();
    for (n, file) in files.iter().enumerate() {
        let text = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
        let backup = dir.join(format!("{n}-{}", file.file_name().and_then(|f| f.to_str()).unwrap_or("config")));
        std::fs::write(&backup, &text).with_context(|| format!("writing {}", backup.display()))?;
        manifest.push_str(&format!("{}\t{}\n", backup.file_name().unwrap_or_default().to_string_lossy(), file.display()));
        let retire_lines: BTreeSet<usize> = conflicts.iter().filter(|c| &c.file == *file).flat_map(|c| c.lines.iter().copied()).collect();
        let prefix = comment_prefix(target, file);
        let script = is_script(file);
        let mut out: Vec<String> = Vec::new();
        // Umbriel autostarts are entries of one array: only the entry goes.
        let umbriel_programs: Vec<(usize, &str)> = if target == Target::Umbriel {
            conflicts
                .iter()
                .filter(|c| &c.file == *file)
                .filter_map(|c| match &c.kind {
                    Kind::Autostart { program, .. } => Some(c.lines.iter().map(move |l| (*l, program.as_str()))),
                    _ => None,
                })
                .flatten()
                .collect()
        } else {
            Vec::new()
        };
        for (i, line) in text.lines().enumerate() {
            let programs: Vec<&str> = umbriel_programs.iter().filter(|(l, _)| *l == i).map(|(_, p)| *p).collect();
            if !programs.is_empty() && !script {
                out.push(programs.iter().fold(line.to_string(), |l, p| umbriel_retire_autostart(&l, p)));
            } else if retire_lines.contains(&i) {
                let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
                if script {
                    out.push(format!("{indent}{SCRIPT_RETIRED} {}", line.trim_start()));
                } else {
                    out.push(format!("{indent}{prefix} mandara-retired: {}", line.trim_start()));
                }
            } else {
                out.push(line.to_string());
            }
        }
        let mut new = out.join("\n");
        if text.ends_with('\n') {
            new.push('\n');
        }
        crate::store::write_atomic(file, new.as_bytes())?;
        // A login script must still parse; otherwise put it back untouched.
        if script && !std::process::Command::new("sh").arg("-n").arg(file).status().is_ok_and(|s| s.success()) {
            crate::store::write_atomic(file, text.as_bytes())?;
            anyhow::bail!("retiring lines in {} would break the script; it was left unchanged", file.display());
        }
    }
    std::fs::write(dir.join("manifest.txt"), manifest)?;
    Ok(dir)
}

/// Puts every file of a retire backup back where it was.
pub fn restore(backup: &Path) -> Result<Vec<PathBuf>> {
    let manifest = std::fs::read_to_string(backup.join("manifest.txt")).with_context(|| format!("{} is not a Mandara backup", backup.display()))?;
    let mut restored = Vec::new();
    for line in manifest.lines() {
        let Some((name, original)) = line.split_once('\t') else { continue };
        let text = std::fs::read_to_string(backup.join(name))?;
        crate::store::write_atomic(Path::new(original), text.as_bytes())?;
        restored.push(PathBuf::from(original));
    }
    Ok(restored)
}

/// Most recent retire backups first.
pub fn backups() -> Vec<PathBuf> {
    let mut list: Vec<PathBuf> = std::fs::read_dir(backups_root()).map(|d| d.flatten().map(|e| e.path()).filter(|p| p.join("manifest.txt").exists()).collect()).unwrap_or_default();
    list.sort();
    list.reverse();
    list
}

/// Outcome of asking the compositor to check its config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Validation {
    Valid { tool: &'static str },
    /// No checker installed (or the compositor isn't installed here).
    Unchecked,
    Invalid { tool: &'static str, report: String },
}

/// Runs the compositor's own config checker on the user's config:
/// `mango -p -c`, `niri validate -c`, `Hyprland --verify-config -c`,
/// `umbriel config validate -c`, `sway --validate --config`.
pub fn validate(target: Target) -> Validation {
    validate_file(target, &target.user_config())
}

/// [`validate`] for any config file of `target`'s dialect.
pub fn validate_file(target: Target, config: &Path) -> Validation {
    let (tool, args): (&'static str, Vec<String>) = match target {
        Target::Mango => ("mango", vec!["-p".into(), "-c".into(), config.display().to_string()]),
        Target::Niri => ("niri", vec!["validate".into(), "-c".into(), config.display().to_string()]),
        Target::Hyprland => ("Hyprland", vec!["--verify-config".into(), "-c".into(), config.display().to_string()]),
        // Exits non-zero when it finds a problem (and works without a
        // running compositor).
        Target::Umbriel => ("umbriel", vec!["config".into(), "validate".into(), "-c".into(), config.display().to_string()]),
        Target::Sway => ("sway", vec!["--validate".into(), "--config".into(), config.display().to_string()]),
        // River has no config checker (its config is a program).
        Target::River | Target::KWin => return Validation::Unchecked,
    };
    let (success, report) = if target == Target::Sway {
        // Sway builds its (headless, software) backend before it reads the
        // config, even to validate: through the doctor's runner (its own
        // private runtime dir, no display, a time limit). If that can't be
        // set up, the config is left unchecked rather than checked against
        // the running session.
        let argv: Vec<std::ffi::OsString> = std::iter::once(tool.into()).chain(args.iter().map(Into::into)).collect();
        match crate::doctor::run_isolated(&argv, crate::doctor::SWAY_CHECK_ENV) {
            Some(out) => (out.success, format!("{}{}", out.stdout, out.stderr)),
            None => return Validation::Unchecked,
        }
    } else {
        let Ok(out) = std::process::Command::new(tool).args(&args).stdin(std::process::Stdio::null()).output() else { return Validation::Unchecked };
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    };
    let failed = match target {
        // mango -p prints problems; it exits 0 either way.
        Target::Mango => report.contains("ERROR"),
        // Sway exits 0 when the error is in an included file; only the
        // level's place counts (a config line echoed in an `[INFO]` line
        // may say `[ERROR]` itself).
        Target::Sway => !success || crate::doctor::sway_has_error(&report),
        _ => !success,
    };
    if failed { Validation::Invalid { tool, report: report.trim().to_string() } } else { Validation::Valid { tool } }
}

#[cfg(test)]
mod tests {
    #[test]
    fn login_scripts_are_followed_and_retired_safely() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".config/mango");
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("autostart.sh");
        // A shell started inside an `if` body: commenting it out would leave
        // the body empty and break the script.
        std::fs::write(&script, "#!/bin/sh\nif [ -n \"$NESTED\" ]; then\n\twayle panel start &\nfi\nfcitx5 -d &\nwayle panel start >>\"$LOG\" 2>&1 &\n").unwrap();
        assert_eq!(startup_script_in("~/.config/mango/autostart.sh", home.path()).as_deref(), Some(script.as_path()));
        assert_eq!(startup_script_in("/usr/bin/waybar", home.path()), None, "only scripts in the home folder");
        let found = scan_script(&script);
        assert_eq!(found.iter().map(|c| c.lines[0]).collect::<Vec<_>>(), vec![2, 5], "both wayle lines, not fcitx5");
        assert!(found.iter().all(|c| matches!(&c.kind, Kind::Autostart { program, role: Role::Shell } if program == "wayle")));
        let backups = tempfile::tempdir().unwrap();
        let backup = retire_into(Target::Mango, &found, backups.path()).unwrap();
        let text = std::fs::read_to_string(&script).unwrap();
        assert!(text.contains("\t: # mandara-retired: wayle panel start &"), "{text}");
        assert!(std::process::Command::new("sh").arg("-n").arg(&script).status().unwrap().success(), "the script still parses");
        assert!(scan_script(&script).is_empty(), "retired lines are not found again");
        restore(&backup).unwrap();
        assert_eq!(scan_script(&script).len(), 2);
    }

    #[test]
    fn mango_exec_once_lines_are_autostarts() {
        assert_eq!(autostart_command(Target::Mango, "exec_once=wayle panel start").as_deref(), Some("wayle panel start"));
        assert_eq!(autostart_command(Target::Mango, "exec-once=waybar").as_deref(), Some("waybar"));
    }

    use super::*;
    use crate::compositor::include_path;

    fn cfg() -> CompositorConfig {
        CompositorConfig::default()
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mandara-conflicts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn mango_binds_autostarts_and_includes() {
        let d = tmp("mango");
        std::fs::write(d.join("config.conf"), "source=./bind.conf\nexec-once=waybar &\nexec-once=wl-paste --watch cliphist store\nexec-once=mandara\n").unwrap();
        std::fs::write(d.join("bind.conf"), "bind=SUPER,space,spawn,fuzzel\nbind=CTRL+SHIFT,w,spawn_shell,~/bin/wallpicker\n# bind=SUPER,space,spawn,rofi\nbind=SUPER,Return,spawn,foot\n").unwrap();
        let files = config_files_from(Target::Mango, &d.join("config.conf"));
        assert_eq!(files.len(), 2);
        let found = scan_files(Target::Mango, &cfg(), &files);
        let keys: Vec<String> = found.iter().filter_map(|c| match &c.kind { Kind::Keybind { chord, .. } => Some(chord.clone()), _ => None }).collect();
        assert!(keys.iter().any(|k| k.to_lowercase().contains("space")), "{keys:?}");
        assert!(keys.iter().any(|k| k.to_lowercase().contains('w')), "{keys:?}");
        assert!(!found.iter().any(|c| c.text.contains("rofi")), "commented binds are ignored");
        assert!(found.iter().any(|c| matches!(&c.kind, Kind::Autostart { program, .. } if program == "waybar")));
        let clip = found.iter().find(|c| matches!(&c.kind, Kind::Autostart { program, .. } if program == "cliphist")).unwrap();
        assert!(!clip.is_conflict());
        // Retire, check markers, then restore byte-for-byte.
        let original = std::fs::read_to_string(d.join("bind.conf")).unwrap();
        let backup_dir = {
            let only: Vec<Conflict> = found.iter().filter(|c| c.is_conflict()).cloned().collect();
            let dir = d.join("backup");
            std::fs::create_dir_all(&dir).unwrap();
            // Use retire() but redirect its backup root via a manual copy check.
            let b = retire_into(Target::Mango, &only, &dir).unwrap();
            assert!(b.join("manifest.txt").exists());
            b
        };
        let retired = std::fs::read_to_string(d.join("bind.conf")).unwrap();
        assert!(retired.contains("# mandara-retired: bind=SUPER,space,spawn,fuzzel"));
        assert!(retired.contains("bind=SUPER,Return,spawn,foot"));
        assert!(scan_files(Target::Mango, &cfg(), &files).iter().all(|c| !c.is_conflict()));
        restore(&backup_dir).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("bind.conf")).unwrap(), original);
        std::fs::remove_dir_all(&backup_dir).ok();
        std::fs::remove_dir_all(d).ok();
    }

    /// Umbriel: binds in `[keybinds]`, autostarts as entries of one
    /// `general.autostart` array (single- or multi-line), includes in
    /// `[include] files`. Retiring takes only the entry out of the array.
    #[test]
    fn umbriel_binds_autostarts_and_includes() {
        let d = tmp("umbriel");
        std::fs::write(
            d.join("config.toml"),
            "[general]\nautostart = [\"waybar\", \"kitty\"] # start\n\n[include]\nfiles = [\"binds.toml\"]\n\n[include.optional]\nfiles = [\"/home/u/.config/mandara/generated/umbriel.toml\"]\n",
        )
        .unwrap();
        std::fs::write(d.join("binds.toml"), "[keybinds]\n\"Mod+Space\" = \"spawn fuzzel\"\n\"Mod+Return\" = \"spawn kitty\"\n\"Alt+Space\" = \"spawn rofi -show drun\"\n").unwrap();
        let files = config_files_from(Target::Umbriel, &d.join("config.toml"));
        assert_eq!(files, vec![d.join("config.toml"), d.join("binds.toml")], "includes followed, Mandara's drop-in skipped");
        let found = scan_files(Target::Umbriel, &cfg(), &files);
        assert!(found.iter().any(|c| matches!(&c.kind, Kind::Keybind { chord, replaces: Some(tool), .. } if chord == "super+space" && tool == "fuzzel")), "{found:?}");
        assert!(found.iter().any(|c| matches!(&c.kind, Kind::Takeover { chord, tool, .. } if chord == "alt+space" && tool == "rofi")), "{found:?}");
        let bar = found.iter().find(|c| matches!(&c.kind, Kind::Autostart { program, .. } if program == "waybar")).expect("waybar autostart");
        assert_eq!(bar.lines, vec![1]);
        // Multi-line arrays: one conflict per entry line.
        let multi = "[general]\nautostart = [\n  \"swww-daemon\",\n  \"kitty\",\n  \"mako\",\n]\n";
        assert_eq!(umbriel_autostarts(multi), vec![(2, "swww-daemon".into()), (3, "kitty".into()), (4, "mako".into())]);
        assert_eq!(umbriel_autostarts("general.autostart = [\"a\"]\n[x]\nautostart = [\"b\"]\n"), vec![(0, "a".into())]);
        // Retire: only waybar leaves the list; kitty and the comment stay.
        let backups = d.join("backup");
        let only: Vec<Conflict> = found.iter().filter(|c| matches!(c.kind, Kind::Autostart { .. })).cloned().collect();
        let b = retire_into(Target::Umbriel, &only, &backups).unwrap();
        let text = std::fs::read_to_string(d.join("config.toml")).unwrap();
        assert!(text.contains("autostart = [\"kitty\"] # mandara-retired: \"waybar\" · start"), "{text}");
        text.parse::<toml::Table>().expect("still TOML");
        assert!(scan_files(Target::Umbriel, &cfg(), &files).iter().all(|c| !matches!(c.kind, Kind::Autostart { .. })), "retired entries are not found again");
        assert_eq!(umbriel_retire_autostart("  \"mako\",", "mako"), "  # mandara-retired: \"mako\"");
        assert_eq!(umbriel_retire_autostart("autostart = [\"kitty\", \"waybar\"]", "waybar"), "autostart = [\"kitty\"] # mandara-retired: \"waybar\"");
        restore(&b).unwrap();
        assert!(std::fs::read_to_string(d.join("config.toml")).unwrap().contains("[\"waybar\", \"kitty\"]"));
        std::fs::remove_dir_all(d).ok();
    }

    /// Sway: binds (variables substituted), `exec` lines, the built-in bar
    /// block and `output … bg` wallpaper; retiring comments the whole bar
    /// block and sway's own checker still accepts the result.
    #[test]
    fn sway_binds_bars_and_wallpapers() {
        let d = tmp("sway");
        std::fs::create_dir_all(d.join("config.d")).unwrap();
        std::fs::write(
            d.join("config"),
            "set $mod Mod4\nset $menu wofi --show drun\nbindsym $mod+space exec $menu\nbindsym $mod+Return exec foot\nexec --no-startup-id mako\nexec_always swaync\noutput * bg ~/wall.png fill\nbar {\n    position top\n    status_command while date; do sleep 1; done\n}\ninclude config.d/*\n",
        )
        .unwrap();
        std::fs::write(d.join("config.d/50-bar.conf"), "bar {\n    swaybar_command waybar\n}\nbindsym $mod+Shift+w exec $menu\n").unwrap();
        let files = config_files_from(Target::Sway, &d.join("config"));
        assert_eq!(files, vec![d.join("config"), d.join("config.d/50-bar.conf")]);
        let found = scan_files(Target::Sway, &cfg(), &files);
        let program = |c: &Conflict| match &c.kind {
            Kind::Autostart { program, .. } => Some(program.clone()),
            _ => None,
        };
        let mut programs: Vec<String> = found.iter().filter_map(program).collect();
        programs.sort();
        assert_eq!(programs, ["mako", "swaybar", "swaybg", "swaync", "waybar"], "{found:#?}");
        let bar = found.iter().find(|c| program(c).as_deref() == Some("swaybar")).unwrap();
        assert_eq!(bar.lines, vec![7, 8, 9, 10], "the whole block");
        assert!(found.iter().any(|c| matches!(&c.kind, Kind::Keybind { chord, replaces: Some(tool), .. } if chord == "super+space" && tool == "wofi")), "{found:#?}");
        // `$mod` and `$menu` from the main config hold in the included file.
        assert!(found.iter().any(|c| c.file == d.join("config.d/50-bar.conf") && c.lines == vec![3] && matches!(&c.kind, Kind::Takeover { chord, tool, .. } if chord == "shift+super+w" && tool == "wofi")), "{found:#?}");
        let b = retire_into(Target::Sway, &found.iter().filter(|c| c.is_conflict()).cloned().collect::<Vec<_>>(), &d.join("backup")).unwrap();
        let text = std::fs::read_to_string(d.join("config")).unwrap();
        assert!(text.contains("# mandara-retired: bar {\n    # mandara-retired: position top"), "{text}");
        assert!(text.contains("bindsym $mod+Return exec foot"), "the user's own binds stay");
        if let Validation::Invalid { report, .. } = validate_file(Target::Sway, &d.join("config")) {
            panic!("sway rejects the retired config:\n{report}\n{text}");
        }
        assert!(scan_files(Target::Sway, &cfg(), &files).iter().all(|c| !c.is_conflict()), "retired lines are not found again");
        restore(&b).unwrap();
        assert!(std::fs::read_to_string(d.join("config")).unwrap().contains("\nbar {\n    position top"));
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn sway_bind_commands_keep_the_variable_value_at_their_line() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config");
        let text = "set $mod Mod4\nset $menu wofi\nbindsym $mod+space exec $menu\nset $menu mandara\nbindsym $mod+x exec $menu\n";
        std::fs::write(&file, text).unwrap();
        let binds = binds_in(Target::Sway, &file, text);
        assert_eq!(binds[0].body, "bindsym Mod4+space exec wofi");
        assert_eq!(binds[1].body, "bindsym Mod4+x exec mandara");
        let found = scan_files(Target::Sway, &cfg(), &[file]);
        assert!(found.iter().any(|c| matches!(&c.kind, Kind::Keybind { chord, replaces: Some(tool), .. } if chord == "super+space" && tool == "wofi")), "{found:?}");
    }

    /// River: maps and autostarts in the init script are found, but never
    /// retired (Mandara doesn't edit init).
    #[test]
    fn river_init_is_scanned_not_edited() {
        let d = tmp("river");
        let init = d.join("init");
        let text = "#!/bin/sh\nriverctl map normal Super Space spawn 'wofi --show drun'\nriverctl spawn waybar\nmako &\nriverctl map normal Super Return spawn foot\n";
        std::fs::write(&init, text).unwrap();
        let found = scan_files(Target::River, &cfg(), std::slice::from_ref(&init));
        let mut programs: Vec<&str> = found.iter().filter_map(|c| match &c.kind {
            Kind::Autostart { program, .. } => Some(program.as_str()),
            _ => None,
        }).collect();
        programs.sort();
        assert_eq!(programs, ["mako", "waybar"], "{found:#?}");
        assert!(found.iter().any(|c| matches!(&c.kind, Kind::Keybind { chord, replaces: Some(tool), .. } if chord == "super+space" && tool == "wofi")), "{found:#?}");
        let err = retire_into(Target::River, &found, &d.join("backup")).unwrap_err().to_string();
        assert!(err.contains("doesn't edit River's init"), "{err}");
        assert_eq!(std::fs::read_to_string(&init).unwrap(), text, "untouched");
        assert_eq!(validate_file(Target::River, &init), Validation::Unchecked);
        std::fs::remove_dir_all(d).ok();
    }

    /// Sway's check runs isolated (the doctor's runner) and still tells a
    /// good config from a broken one, an included file's error too.
    #[test]
    fn sway_validation_tells_good_from_broken() {
        if std::process::Command::new("sway").arg("--version").output().is_err() {
            eprintln!("sway isn't installed; skipped");
            return;
        }
        let d = tmp("sway-validate");
        std::fs::write(d.join("good"), "bindsym Mod4+q kill\n").unwrap();
        std::fs::write(d.join("child.conf"), "frobnicate on\n").unwrap();
        std::fs::write(d.join("broken"), format!("include {}\n", d.join("child.conf").display())).unwrap();
        assert_eq!(validate_file(Target::Sway, &d.join("good")), Validation::Valid { tool: "sway" });
        assert!(matches!(validate_file(Target::Sway, &d.join("broken")), Validation::Invalid { report, .. } if report.contains("frobnicate")));
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn niri_multiline_binds_and_spawns() {
        let d = tmp("niri");
        std::fs::write(
            d.join("config.kdl"),
            "binds {\n    Mod+Space { spawn \"fuzzel\"; }\n    Mod+T {\n        spawn \"alacritty\";\n    }\n    Ctrl+Shift+W hotkey-overlay-title=\"x\" {\n        spawn \"wallpicker\";\n    }\n}\nspawn-at-startup \"waybar\"\nspawn-sh-at-startup \"swww-daemon && swww img a.png\"\n",
        )
        .unwrap();
        let found = scan_files(Target::Niri, &cfg(), &[d.join("config.kdl")]);
        let wall = found.iter().find(|c| c.text.starts_with("Ctrl+Shift+W")).expect("ctrl+shift+w bind");
        assert_eq!(wall.lines, vec![5, 6, 7], "multi-line bind spans its block");
        assert!(found.iter().any(|c| c.text.starts_with("Mod+Space")));
        assert!(!found.iter().any(|c| c.text.starts_with("Mod+T")));
        assert!(found.iter().any(|c| matches!(&c.kind, Kind::Autostart { program, .. } if program == "waybar")));
        assert!(found.iter().any(|c| matches!(&c.kind, Kind::Autostart { role: Role::Wallpaper, .. })));
        let b = retire_into(Target::Niri, &found, &d.join("backup")).unwrap();
        let text = std::fs::read_to_string(d.join("config.kdl")).unwrap();
        assert!(text.contains("// mandara-retired: Ctrl+Shift+W") && text.contains("// mandara-retired: }"));
        // Braces stay balanced outside comments.
        let open = text.lines().filter(|l| !l.trim_start().starts_with("//")).map(|l| l.matches('{').count() as i32 - l.matches('}').count() as i32).sum::<i32>();
        assert_eq!(open, 0);
        restore(&b).unwrap();
        std::fs::remove_dir_all(&b).ok();
        std::fs::remove_dir_all(d).ok();
    }

    /// Niri's autostart forms, its optional includes and login scripts.
    #[test]
    fn niri_autostarts_scripts_and_optional_includes() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".config/niri");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("autostart.sh"), "#!/bin/sh
qs -c noctalia-shell &
awww-daemon &
fcitx5 -d
").unwrap();
        std::fs::write(dir.join("binds.kdl"), "binds {
    Mod+Space { spawn \"fuzzel\"; }
}
").unwrap();
        std::fs::write(
            dir.join("config.kdl"),
            "include optional=true \"binds.kdl\"\nspawn-at-startup \"sh\" \"-c\" \"~/.config/niri/autostart.sh\"\nspawn-sh-at-startup \"~/.config/niri/autostart.sh --quiet\"\nspawn-at-startup \"waybar\"\n".replace("\\\"", "\""),
        )
        .unwrap();
        let files = config_files_from(Target::Niri, &dir.join("config.kdl"));
        assert_eq!(files.len(), 2, "optional=true include followed: {files:?}");
        let found = scan_files(Target::Niri, &cfg(), &files);
        assert!(found.iter().any(|c| matches!(&c.kind, Kind::Keybind { replaces: Some(t), .. } if t == "fuzzel")), "{found:?}");
        for line in ["spawn-at-startup \"sh\" \"-c\" \"~/.config/niri/autostart.sh\"", "spawn-sh-at-startup \"~/.config/niri/autostart.sh --quiet\""] {
            let cmd = autostart_command(Target::Niri, line).unwrap();
            assert_eq!(startup_script_in(&cmd, home.path()).as_deref(), Some(dir.join("autostart.sh").as_path()), "{line}");
        }
        let script = scan_script(&dir.join("autostart.sh"));
        let programs: Vec<&str> = script.iter().filter_map(|c| match &c.kind { Kind::Autostart { program, .. } => Some(program.as_str()), _ => None }).collect();
        assert_eq!(programs, ["qs", "awww-daemon"]);
        assert_eq!(include_path(Target::Niri, "include \"~/.config/mandara/generated/niri.kdl\"").as_deref(), Some("~/.config/mandara/generated/niri.kdl"));
    }

    /// Hyprland: `$var` modifiers, Lua `local` modifiers and `require`d
    /// files, Lua autostarts of a login script.
    #[test]
    fn hyprland_variables_lua_binds_and_scripts() {
        let d = tmp("hypr-vars");
        std::fs::write(d.join("hyprland.conf"), "$mainMod = SUPER\n$menu = wofi --show drun\nbind = $mainMod, SPACE, exec, $menu\nbind = $mainMod, R, exec, rofi -show run\nexec-once = ~/.config/hypr/start.sh\n").unwrap();
        let found = scan_files(Target::Hyprland, &cfg(), &[d.join("hyprland.conf")]);
        assert!(found.iter().any(|c| matches!(&c.kind, Kind::Keybind { chord, replaces: Some(t), .. } if chord == "super+space" && t == "wofi")), "{found:?}");
        assert!(found.iter().any(|c| matches!(&c.kind, Kind::Takeover { chord, tool, .. } if chord == "super+r" && tool == "rofi")), "{found:?}");
        std::fs::write(d.join("binds.lua"), "local mainMod = \"SUPER\"\nhl.bind(mainMod .. \" + SPACE\", hl.dsp.exec_cmd(\"fuzzel\"))\nhl.bind(\"SUPER + Q\", hl.dsp.exec_cmd(\"kitty\"))\nfor i = 1, 9 do hl.bind(mainMod .. \" + \" .. i, hl.dsp.focus({ workspace = i })) end\n").unwrap();
        std::fs::write(d.join("hyprland.lua"), "require(\"binds\")\nhl.on(\"hyprland.start\", function()\n  hl.exec_cmd(\"~/.config/hypr/start.sh\")\n  hl.exec_cmd(\"hyprpanel\")\nend)\n").unwrap();
        let files = config_files_from(Target::Hyprland, &d.join("hyprland.lua"));
        assert_eq!(files, vec![d.join("hyprland.lua"), d.join("binds.lua")]);
        let lua = scan_files(Target::Hyprland, &cfg(), &files);
        assert!(lua.iter().any(|c| matches!(&c.kind, Kind::Keybind { chord, replaces: Some(t), .. } if chord == "super+space" && t == "fuzzel")), "{lua:?}");
        assert!(lua.iter().any(|c| matches!(&c.kind, Kind::Autostart { program, .. } if program == "hyprpanel")));
        assert_eq!(chords_by_line(Target::Hyprland, &std::fs::read_to_string(d.join("binds.lua")).unwrap()), vec![(1, "super+space".to_string()), (2, "super+q".to_string())], "loop binds can't be resolved");
        let cmd = autostart_command(Target::Hyprland, "  hl.exec_cmd(\"~/.config/hypr/start.sh\")").unwrap();
        assert_eq!(cmd, "~/.config/hypr/start.sh");
        assert_eq!(include_path(Target::Hyprland, "pcall(dofile, \"/home/u/.config/mandara/generated/hyprland.lua\")").as_deref(), Some("/home/u/.config/mandara/generated/hyprland.lua"));
        assert_eq!(include_path(Target::Hyprland, "pcall(dofile, os.getenv(\"HOME\") .. \"/x.lua\")"), None);
        assert_eq!(include_path(Target::Hyprland, "source = ~/.config/hypr/binds.conf").as_deref(), Some("~/.config/hypr/binds.conf"));
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn mandara_autostart_is_not_a_bind() {
        use crate::compositor::starts_mandara;
        assert!(starts_mandara("hl.on(\"hyprland.start\", function() hl.exec_cmd(\"/usr/bin/mandara\") end)"));
        assert!(!starts_mandara("hl.bind(\"SUPER + B\", hl.dsp.exec_cmd(\"mandara msg sidebar toggle\"))"), "a Lua bind runs `mandara msg`, it doesn't start the shell");
        assert!(starts_mandara("spawn-sh-at-startup \"mandara\""));
        assert!(starts_mandara("spawn-at-startup \"/home/u/.local/bin/mandara\""));
        assert!(starts_mandara("exec-once = mandara"));
        assert!(!starts_mandara("// spawn-at-startup \"mandara\""));
    }

    #[test]
    fn hyprland_conf_and_lua() {
        let d = tmp("hypr");
        std::fs::write(d.join("hyprland.conf"), "bind = SUPER, SPACE, exec, rofi -show drun\nexec-once = hyprpaper\nexec-once = uwsm app -- mako\nbind = SUPER, Q, exec, kitty\n").unwrap();
        let found = scan_files(Target::Hyprland, &cfg(), &[d.join("hyprland.conf")]);
        assert!(found.iter().any(|c| matches!(&c.kind, Kind::Keybind { .. })));
        assert!(found.iter().any(|c| matches!(&c.kind, Kind::Autostart { program, .. } if program == "hyprpaper")));
        assert!(found.iter().any(|c| matches!(&c.kind, Kind::Autostart { program, .. } if program == "mako")));
        std::fs::write(d.join("hyprland.lua"), "hl.on(\"hyprland.start\", function()\n  hl.exec_cmd(\"waybar\")\nend)\n").unwrap();
        let lua = scan_files(Target::Hyprland, &cfg(), &[d.join("hyprland.lua")]);
        assert!(lua.iter().any(|c| matches!(&c.kind, Kind::Autostart { program, .. } if program == "waybar")));
        let b = retire_into(Target::Hyprland, &lua, &d.join("backup")).unwrap();
        assert!(std::fs::read_to_string(d.join("hyprland.lua")).unwrap().contains("-- mandara-retired: hl.exec_cmd(\"waybar\")"));
        std::fs::remove_dir_all(&b).ok();
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn user_binds_carry_lines_and_bodies_per_compositor() {
        let f = Path::new("/x/config.conf");
        let mango = binds_in(Target::Mango, f, "blur=1\nbind=SUPER,Return,spawn,ghostty\n# bind=SUPER,x,nope\nbind=CTRL+ALT,Delete,quit\n");
        assert_eq!(mango.iter().map(|b| (b.chord.as_str(), b.line)).collect::<Vec<_>>(), vec![("super+return", 2), ("alt+ctrl+delete", 4)]);
        let niri = binds_in(Target::Niri, f, "binds {\n    Mod+T { spawn \"alacritty\"; }\n    // Mod+X { quit; }\n    Mod+Shift+E {\n        quit;\n    }\n}\nlayout { gaps 8; }\n");
        assert_eq!(niri.iter().map(|b| (b.chord.as_str(), b.line)).collect::<Vec<_>>(), vec![("super+t", 2), ("shift+super+e", 4)]);
        assert_eq!(niri[1].body, "    Mod+Shift+E {\n        quit;\n    }", "a Niri bind spans its block");
        let hypr = binds_in(Target::Hyprland, f, "$menu = fuzzel\nbind = SUPER, Q, exec, kitty\n#bind = SUPER, W, exec, x\nbind = SUPER, D, exec, $menu\n");
        assert_eq!(hypr.len(), 2);
        assert_eq!(hypr[0].text, "bind = SUPER, Q, exec, kitty");
        assert_eq!(hypr[1].body, "bind = SUPER, D, exec, fuzzel", "variables are substituted");
    }

    #[test]
    fn hyprland_variables_and_lua_locals_resolve_in_user_binds() {
        let f = Path::new("/x/hyprland.conf");
        let conf = "$mainMod = SUPER\n$alt = ALT\nbind = $mainMod, Q, exec, kitty\nbind = $mainMod SHIFT, E, exit\n# bind = $mainMod, W, exec, x\nbindl = $alt, F1, exec, y\n";
        let got: Vec<(String, usize)> = binds_in(Target::Hyprland, f, conf).into_iter().map(|b| (b.chord, b.line)).collect();
        assert_eq!(got, vec![("super+q".to_string(), 3), ("shift+super+e".to_string(), 4), ("alt+f1".to_string(), 6)]);
        let lua = Path::new("/x/hyprland.lua");
        let text = "local mod = \"SUPER\"\nlocal smod = mod .. \" + SHIFT\"\nhl.bind(mod .. \" + Return\", hl.exec(\"kitty\"))\n-- hl.bind(mod .. \" + X\", hl.exec(\"x\"))\nhl.bind(smod .. \" + Q\", hl.kill())\n";
        let got: Vec<(String, usize)> = binds_in(Target::Hyprland, lua, text).into_iter().map(|b| (b.chord, b.line)).collect();
        assert_eq!(got, vec![("super+return".to_string(), 3), ("shift+super+q".to_string(), 5)]);
    }
}
