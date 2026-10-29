//! Compositor conflicts — what other setups leave in a user's MangoWM, Niri
//! or Hyprland config that would fight Mandara — and how to retire them
//! safely.
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

use crate::compositor::{BindAction, CompositorConfig, Keybind, Target, chord, chords_by_line, include_path, is_generated, resolve_include};

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
        let mut children: Vec<PathBuf> = text.lines().filter(|l| !is_comment(target, l)).filter_map(|l| include_path(target, l)).map(|p| resolve_include(&file, &p)).collect();
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
    }
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
fn mandara_chords(cfg: &CompositorConfig) -> Vec<(String, String)> {
    cfg.keybinds.iter().map(|k| (k.chord(), k.action.shell_command_with("mandara").trim_start_matches("mandara msg ").to_string())).collect()
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
        } else if in_binds && depth == 1 && open.is_none() {
            if let Some(first) = line.split_whitespace().next().filter(|w| !w.starts_with('}')) {
                let mut pieces: Vec<&str> = first.split('+').collect();
                let key = pieces.pop().unwrap_or_default();
                open = Some((i, chord(pieces.into_iter(), key), depth));
            }
        }
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        if let Some((start, ch, at)) = open.clone() {
            if depth <= at {
                out.push((start, i, ch));
                open = None;
            }
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
    match target {
        Target::Niri => niri_binds(text),
        _ => {
            let lines: Vec<&str> = text.lines().collect();
            chords_by_line(target, text).into_iter().filter(|(i, _)| lines.get(*i).is_some_and(|l| !is_comment(target, l))).map(|(i, c)| (i, i, c)).collect()
        }
    }
}

/// Conflicts in the user's config tree for `target` against `cfg`.
pub fn scan(target: Target, cfg: &CompositorConfig) -> Vec<Conflict> {
    scan_files(target, cfg, &config_files(target))
}

pub fn scan_files(target: Target, cfg: &CompositorConfig, files: &[PathBuf]) -> Vec<Conflict> {
    let ours = mandara_chords(cfg);
    let mut out = Vec::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        let lines: Vec<&str> = text.lines().collect();
        // Keybinds.
        let binds = file_binds(target, &text);
        // Hyprland `$var = value` lines, substituted into bind bodies so
        // `exec, $menu` names the program behind it.
        let vars: Vec<(String, String)> = if target == Target::Hyprland {
            let mut v: Vec<(String, String)> = lines.iter().filter_map(|l| l.trim().strip_prefix('$')?.split_once('=').map(|(n, val)| (format!("${}", n.trim()), val.trim().to_string()))).collect();
            v.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
            v
        } else {
            Vec::new()
        };
        for (start, end, ch) in binds {
            let body = vars.iter().fold(lines[start..=end].join("\n"), |b, (n, val)| b.replace(n.as_str(), val));
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
        for (i, l) in lines.iter().enumerate() {
            if is_comment(target, l) {
                continue;
            }
            let Some(cmd) = autostart_command(target, l) else { continue };
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
        for (i, line) in text.lines().enumerate() {
            if retire_lines.contains(&i) {
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
/// `mango -p -c`, `niri validate -c`, `Hyprland --verify-config -c`.
pub fn validate(target: Target) -> Validation {
    let config = target.user_config();
    let (tool, args): (&'static str, Vec<String>) = match target {
        Target::Mango => ("mango", vec!["-p".into(), "-c".into(), config.display().to_string()]),
        Target::Niri => ("niri", vec!["validate".into(), "-c".into(), config.display().to_string()]),
        Target::Hyprland => ("Hyprland", vec!["--verify-config".into(), "-c".into(), config.display().to_string()]),
    };
    let Ok(out) = std::process::Command::new(tool).args(&args).output() else { return Validation::Unchecked };
    let report = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let failed = match target {
        // mango -p prints problems; it exits 0 either way.
        Target::Mango => report.contains("ERROR"),
        _ => !out.status.success(),
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
}
