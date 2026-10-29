//! Applications & system integration: preferred apps, default apps
//! (`mimeapps.list`) and XDG autostart entries.
//!
//! All three edit the user's own files *non-destructively*:
//!
//! * `~/.config/mimeapps.list` — only the keys Mandara sets in
//!   `[Default Applications]` are rewritten; comments, other keys, other
//!   groups (`[Added Associations]`…) and their order are kept verbatim.
//! * `~/.config/autostart/*.desktop` — enabling / disabling an entry only
//!   toggles its `Hidden=` key (XDG Autostart spec). A system entry from
//!   `/etc/xdg/autostart` is disabled by a user override with
//!   `Hidden=true`, never by touching the system file. Mandara only deletes
//!   entries it created itself (`X-Mandara-Created=true`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

/// `[apps]`: apps Mandara itself launches (terminal for updates and
/// "open in terminal", file manager for drives / downloads, browser for
/// links). Empty = detect.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppsConfig {
    /// Desktop id or command (`com.mitchellh.ghostty`, `kitty`).
    pub terminal: String,
    pub file_manager: String,
    pub browser: String,
    /// Window app ids → desktop entries, for apps whose windows don't name
    /// their `.desktop` file (icons and names in the bar, dock, switcher).
    pub id_map: Vec<AppIdAlias>,
    /// Mandara starts the enabled XDG autostart entries once per login,
    /// unless systemd's `xdg-desktop-autostart.target` already does. Off
    /// by default: compositor configs often start the same apps
    /// (`exec-once`), which would then run twice.
    pub run_autostart: bool,
}

/// `pattern` (an app id; `*` matches any run of characters) → `desktop`
/// (a desktop id, with or without `.desktop`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppIdAlias {
    pub pattern: String,
    pub desktop: String,
}

/// Case-insensitive glob match with `*` wildcards.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.to_lowercase(), text.to_lowercase());
    let parts: Vec<&str> = p.split('*').collect();
    if parts.len() == 1 {
        return p == t;
    }
    let mut rest = t.as_str();
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match rest.find(part) {
            Some(at) if i > 0 || at == 0 => rest = &rest[at + part.len()..],
            _ => return false,
        }
    }
    parts.last().is_some_and(|last| last.is_empty() || t.ends_with(last))
}

/// The desktop id stem an app id maps to (`None` = no alias).
pub fn alias_for<'a>(aliases: &'a [AppIdAlias], app_id: &str) -> Option<&'a str> {
    aliases.iter().find(|a| !a.pattern.trim().is_empty() && glob_match(a.pattern.trim(), app_id)).map(|a| a.desktop.trim().trim_end_matches(".desktop")).filter(|d| !d.is_empty())
}

/// Terminals tried in order when none is configured.
pub const TERMINAL_CANDIDATES: &[&str] = &["ghostty", "kitty", "foot", "alacritty", "wezterm", "konsole", "gnome-terminal", "xfce4-terminal", "xterm"];
/// File managers tried in order when none is configured.
pub const FILE_MANAGER_CANDIDATES: &[&str] = &["nautilus", "dolphin", "thunar", "nemo", "pcmanfm-qt", "pcmanfm", "yazi"];

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(bin).is_file()))
}

impl AppsConfig {
    /// The terminal command: configured (desktop id → its `Exec` binary),
    /// else the first installed candidate.
    pub fn terminal_command(&self) -> Option<String> {
        resolve_command(&self.terminal).or_else(|| TERMINAL_CANDIDATES.iter().find(|c| on_path(c)).map(|c| c.to_string()))
    }

    /// The file manager command: configured, else the `inode/directory`
    /// default, else the first installed candidate.
    pub fn file_manager_command(&self) -> Option<String> {
        resolve_command(&self.file_manager)
            .or_else(|| default_for(&mimeapps_file(), "inode/directory").and_then(|id| resolve_command(&id)))
            .or_else(|| FILE_MANAGER_CANDIDATES.iter().find(|c| on_path(c)).map(|c| c.to_string()))
    }

    /// The browser command when one is configured (`None` = the
    /// `x-scheme-handler/https` default, through the desktop portal).
    pub fn browser_command(&self) -> Option<String> {
        resolve_command(&self.browser)
    }
}

/// A desktop id (`foo.desktop` or `foo`) → the program of its `Exec`; any
/// other non-empty value is taken as a command.
pub fn resolve_command(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let id = value.strip_suffix(".desktop").unwrap_or(value);
    if let Some(entry) = find_desktop(&format!("{id}.desktop")) {
        return Some(exec_program(&entry.exec));
    }
    Some(value.to_string())
}

/// The `Exec` line without field codes (`%u`, `%F`…).
pub fn exec_program(exec: &str) -> String {
    exec.split_whitespace().filter(|w| !(w.len() == 2 && w.starts_with('%'))).collect::<Vec<_>>().join(" ")
}

// ── Desktop entries ─────────────────────────────────────────────────────────

/// The parts of a `.desktop` file Mandara needs.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct DesktopEntry {
    /// File name (`firefox.desktop`).
    pub id: String,
    pub name: String,
    pub exec: String,
    pub icon: String,
    pub comment: String,
    pub mime_types: Vec<String>,
    pub categories: Vec<String>,
    pub no_display: bool,
    pub hidden: bool,
    pub terminal: bool,
    #[serde(skip)]
    pub path: PathBuf,
}

/// Parses the `[Desktop Entry]` group (unlocalized keys).
pub fn parse_desktop(id: &str, text: &str) -> DesktopEntry {
    let mut e = DesktopEntry { id: id.into(), ..Default::default() };
    let mut in_main = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        let list = || v.split(';').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect::<Vec<_>>();
        match k {
            "Name" => e.name = v.into(),
            "Exec" => e.exec = v.into(),
            "Icon" => e.icon = v.into(),
            "Comment" => e.comment = v.into(),
            "MimeType" => e.mime_types = list(),
            "Categories" => e.categories = list(),
            "NoDisplay" => e.no_display = v == "true",
            "Hidden" => e.hidden = v == "true",
            "Terminal" => e.terminal = v == "true",
            _ => {}
        }
    }
    e
}

/// `applications/` folders, user first (earlier wins on duplicate ids).
pub fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(d) = dirs::data_dir() {
        dirs.push(d.join("applications"));
    }
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').filter(|d| !d.is_empty()).map(|d| PathBuf::from(d).join("applications")));
    dirs
}

/// Every desktop entry in `dirs` (first occurrence of an id wins).
pub fn desktop_entries_in(dirs: &[PathBuf]) -> Vec<DesktopEntry> {
    let mut out: BTreeMap<String, DesktopEntry> = BTreeMap::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for f in entries.flatten() {
            let id = f.file_name().to_string_lossy().to_string();
            if !id.ends_with(".desktop") || out.contains_key(&id) {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(f.path()) {
                let mut e = parse_desktop(&id, &text);
                e.path = f.path();
                out.insert(id, e);
            }
        }
    }
    out.into_values().collect()
}

pub fn desktop_entries() -> Vec<DesktopEntry> {
    desktop_entries_in(&application_dirs())
}

pub fn find_desktop(id: &str) -> Option<DesktopEntry> {
    application_dirs().into_iter().map(|d| d.join(id)).find(|p| p.is_file()).and_then(|p| {
        let mut e = parse_desktop(id, &std::fs::read_to_string(&p).ok()?);
        e.path = p;
        Some(e)
    })
}

// ── Default apps (mimeapps.list) ────────────────────────────────────────────

/// A default-app category and the MIME types it sets together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppCategory {
    pub id: &'static str,
    /// English label (`tk`-marked by the UI).
    pub label: &'static str,
    pub icon: &'static str,
    /// The first type decides what is shown as current; all are written.
    pub mime_types: &'static [&'static str],
}

pub const APP_CATEGORIES: &[AppCategory] = &[
    AppCategory { id: "browser", label: "Web browser", icon: "globe", mime_types: &["x-scheme-handler/https", "x-scheme-handler/http", "text/html", "application/xhtml+xml"] },
    AppCategory { id: "mail", label: "Email", icon: "external", mime_types: &["x-scheme-handler/mailto"] },
    AppCategory { id: "files", label: "File manager", icon: "folder", mime_types: &["inode/directory"] },
    AppCategory { id: "text", label: "Text editor", icon: "note", mime_types: &["text/plain", "text/markdown", "application/x-shellscript"] },
    AppCategory { id: "image", label: "Images", icon: "image", mime_types: &["image/png", "image/jpeg", "image/webp", "image/gif", "image/avif", "image/svg+xml"] },
    AppCategory { id: "video", label: "Videos", icon: "play", mime_types: &["video/mp4", "video/x-matroska", "video/webm", "video/quicktime"] },
    AppCategory { id: "audio", label: "Music", icon: "music", mime_types: &["audio/mpeg", "audio/flac", "audio/ogg", "audio/x-wav", "audio/mp4"] },
    AppCategory { id: "pdf", label: "Documents (PDF)", icon: "note", mime_types: &["application/pdf"] },
    AppCategory { id: "archive", label: "Archives", icon: "package", mime_types: &["application/zip", "application/x-7z-compressed", "application/x-tar", "application/gzip"] },
];

pub fn category(id: &str) -> Option<&'static AppCategory> {
    APP_CATEGORIES.iter().find(|c| c.id == id)
}

/// `$XDG_CONFIG_HOME/mimeapps.list`.
pub fn mimeapps_file() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config")).join("mimeapps.list")
}

const DEFAULTS_GROUP: &str = "[Default Applications]";

/// The `[Default Applications]` entries of a mimeapps.list text.
pub fn parse_defaults(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    let mut in_group = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_group = t == DEFAULTS_GROUP;
            continue;
        }
        if in_group && let Some((k, v)) = t.split_once('=') {
            out.insert(k.trim().to_string(), v.split(';').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect());
        }
    }
    out
}

/// The default desktop id for `mime` in the mimeapps.list at `path`.
pub fn default_for(path: &Path, mime: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_defaults(&text).remove(mime).and_then(|v| v.into_iter().next())
}

/// `text` with `mime=desktop_id;` set in `[Default Applications]` for each
/// mime type. Existing keys are replaced in place; missing ones are added at
/// the end of the group (which is created at the end of the file when
/// absent). Everything else is preserved byte for byte.
pub fn set_defaults_text(text: &str, mimes: &[&str], desktop_id: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let mut remaining: Vec<&str> = mimes.to_vec();
    let mut group: Option<(usize, usize)> = None; // (header index, last content index)
    let mut in_group = false;
    for (i, line) in lines.iter_mut().enumerate() {
        let t = line.trim().to_string();
        if t.starts_with('[') {
            in_group = t == DEFAULTS_GROUP;
            if in_group && group.is_none() {
                group = Some((i, i));
            }
            continue;
        }
        if !in_group {
            continue;
        }
        if !t.is_empty()
            && let Some(g) = group.as_mut()
        {
            g.1 = i;
        }
        if let Some((k, _)) = t.split_once('=')
            && let Some(pos) = remaining.iter().position(|m| *m == k.trim())
        {
            *line = format!("{}={desktop_id};", k.trim());
            remaining.remove(pos);
        }
    }
    if !remaining.is_empty() {
        let new: Vec<String> = remaining.iter().map(|m| format!("{m}={desktop_id};")).collect();
        match group {
            Some((_, last)) => {
                for (n, l) in new.into_iter().enumerate() {
                    lines.insert(last + 1 + n, l);
                }
            }
            None => {
                if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                    lines.push(String::new());
                }
                lines.push(DEFAULTS_GROUP.into());
                lines.extend(new);
            }
        }
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') || text.is_empty() {
        out.push('\n');
    }
    out
}

/// Sets `desktop_id` as the default app of `category` in the mimeapps.list
/// at `path` (backed up once to `mimeapps.list.mandara-backup`).
pub fn set_default_at(path: &Path, category: &AppCategory, desktop_id: &str) -> Result<()> {
    let desktop_id = desktop_id.trim();
    if !desktop_id.ends_with(".desktop") || desktop_id.contains(['/', ';', '\n']) {
        bail!("not a desktop id: {desktop_id:?}");
    }
    let before = std::fs::read_to_string(path).unwrap_or_default();
    let backup = path.with_file_name("mimeapps.list.mandara-backup");
    if !before.is_empty() && !backup.exists() {
        std::fs::write(&backup, &before).with_context(|| format!("backing up {}", path.display()))?;
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::store::write_atomic(path, set_defaults_text(&before, category.mime_types, desktop_id).as_bytes())
}

pub fn set_default(category: &AppCategory, desktop_id: &str) -> Result<()> {
    set_default_at(&mimeapps_file(), category, desktop_id)
}

/// Apps that declare a MIME type of `category`, by name.
pub fn candidates<'a>(entries: &'a [DesktopEntry], category: &AppCategory) -> Vec<&'a DesktopEntry> {
    let mut out: Vec<&DesktopEntry> = entries.iter().filter(|e| !e.hidden && e.mime_types.iter().any(|m| category.mime_types.contains(&m.as_str()))).collect();
    out.sort_by_key(|e| e.name.to_lowercase());
    out
}

// ── Autostart ───────────────────────────────────────────────────────────────

/// Where an autostart entry comes from.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutostartOrigin {
    /// `~/.config/autostart` only.
    User,
    /// `/etc/xdg/autostart` (possibly overridden by a user copy).
    System,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AutostartEntry {
    pub id: String,
    pub name: String,
    pub exec: String,
    pub icon: String,
    pub comment: String,
    pub enabled: bool,
    pub origin: AutostartOrigin,
    /// Created by Mandara (may be removed).
    pub mandara: bool,
    /// Limited to some desktops (`OnlyShowIn=GNOME;`) and so skipped by
    /// other sessions' autostart runners.
    pub only_show_in: Vec<String>,
    /// Desktops that skip it (`NotShowIn=KDE;`).
    pub not_show_in: Vec<String>,
    /// Started only when this program exists (`TryExec=`).
    pub try_exec: String,
}

const MANDARA_CREATED: &str = "X-Mandara-Created=true";

/// The autostart folders: `(user, system)`.
pub fn autostart_dirs() -> (PathBuf, Vec<PathBuf>) {
    let user = dirs::config_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config")).join("autostart");
    let system = std::env::var("XDG_CONFIG_DIRS").unwrap_or_else(|_| "/etc/xdg".into());
    (user, system.split(':').filter(|d| !d.is_empty()).map(|d| PathBuf::from(d).join("autostart")).collect())
}

fn only_show_in(text: &str) -> Vec<String> {
    desktop_list(text, "OnlyShowIn=")
}

/// A `;`-separated key of the `[Desktop Entry]` group.
fn desktop_list(text: &str, key: &str) -> Vec<String> {
    text.lines().find_map(|l| l.trim().strip_prefix(key)).map(|v| v.split(';').filter(|s| !s.is_empty()).map(String::from).collect()).unwrap_or_default()
}

impl AutostartEntry {
    /// Whether a session whose `XDG_CURRENT_DESKTOP` is `desktops` starts
    /// it (`OnlyShowIn` / `NotShowIn`, compared case-insensitively).
    pub fn shown_in(&self, desktops: &[String]) -> bool {
        let has = |list: &[String]| list.iter().any(|d| desktops.iter().any(|x| x.eq_ignore_ascii_case(d)));
        (self.only_show_in.is_empty() || has(&self.only_show_in)) && !has(&self.not_show_in)
    }
}

/// The autostart entries a login should start, in order: enabled, meant
/// for this desktop, with a command, and whose `TryExec` program exists.
pub fn autostart_to_run<'a>(entries: &'a [AutostartEntry], desktops: &[String], program_exists: impl Fn(&str) -> bool) -> Vec<&'a AutostartEntry> {
    entries
        .iter()
        .filter(|e| e.enabled && e.shown_in(desktops) && !e.exec.trim().is_empty())
        .filter(|e| e.try_exec.trim().is_empty() || program_exists(e.try_exec.trim()))
        .collect()
}

/// `XDG_CURRENT_DESKTOP` as a list (`Hyprland`, `mango:wlroots`).
pub fn current_desktops() -> Vec<String> {
    std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().split(':').filter(|d| !d.is_empty()).map(String::from).collect()
}

/// Autostart entries: user files override system ones of the same id.
pub fn autostart_entries_in(user: &Path, system: &[PathBuf]) -> Vec<AutostartEntry> {
    let mut out: BTreeMap<String, AutostartEntry> = BTreeMap::new();
    let read = |dir: &Path, origin: AutostartOrigin, out: &mut BTreeMap<String, AutostartEntry>| {
        let Ok(files) = std::fs::read_dir(dir) else { return };
        for f in files.flatten() {
            let id = f.file_name().to_string_lossy().to_string();
            if !id.ends_with(".desktop") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(f.path()) else { continue };
            let e = parse_desktop(&id, &text);
            let gnome_off = text.lines().any(|l| l.trim() == "X-GNOME-Autostart-enabled=false");
            let entry = AutostartEntry {
                id: id.clone(),
                name: if e.name.is_empty() { id.trim_end_matches(".desktop").to_string() } else { e.name },
                exec: e.exec,
                icon: e.icon,
                comment: e.comment,
                enabled: !e.hidden && !gnome_off,
                origin,
                mandara: text.contains(MANDARA_CREATED),
                only_show_in: only_show_in(&text),
                not_show_in: desktop_list(&text, "NotShowIn="),
                try_exec: text.lines().find_map(|l| l.trim().strip_prefix("TryExec=")).unwrap_or_default().trim().to_string(),
            };
            match out.get_mut(&id) {
                // A user copy of a system entry: keep "System" (it can't be
                // removed, only disabled) but take the user's state.
                Some(existing) if origin == AutostartOrigin::User => {
                    existing.enabled = entry.enabled;
                    existing.mandara = entry.mandara;
                }
                Some(_) => {}
                None => {
                    out.insert(id, entry);
                }
            }
        }
    };
    for dir in system.iter().rev() {
        read(dir, AutostartOrigin::System, &mut out);
    }
    read(user, AutostartOrigin::User, &mut out);
    let mut list: Vec<AutostartEntry> = out.into_values().collect();
    list.sort_by_key(|e| e.name.to_lowercase());
    list
}

pub fn autostart_entries() -> Vec<AutostartEntry> {
    let (user, system) = autostart_dirs();
    autostart_entries_in(&user, &system)
}

/// `text` with `key=value` set in `[Desktop Entry]` (replaced in place, or
/// appended to the group).
fn set_key(text: &str, key: &str, value: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let mut in_main = false;
    let mut last_main = None;
    for (i, line) in lines.iter_mut().enumerate() {
        let t = line.trim().to_string();
        if t.starts_with('[') {
            in_main = t == "[Desktop Entry]";
            if in_main {
                last_main = Some(i);
            }
            continue;
        }
        if in_main {
            if !t.is_empty() {
                last_main = Some(i);
            }
            if t.split_once('=').is_some_and(|(k, _)| k.trim() == key) {
                *line = format!("{key}={value}");
                return lines.join("\n") + "\n";
            }
        }
    }
    match last_main {
        Some(i) => lines.insert(i + 1, format!("{key}={value}")),
        None => {
            lines.insert(0, "[Desktop Entry]".into());
            lines.insert(1, format!("{key}={value}"));
        }
    }
    lines.join("\n") + "\n"
}

/// Enables or disables autostart entry `id` by its `Hidden=` key, in the
/// user folder (a system entry gets a user override copy).
pub fn set_autostart_enabled_in(user: &Path, system: &[PathBuf], id: &str, enabled: bool) -> Result<()> {
    if id.contains('/') || !id.ends_with(".desktop") {
        bail!("not an autostart entry id: {id:?}");
    }
    let target = user.join(id);
    let source = if target.is_file() { Some(target.clone()) } else { system.iter().map(|d| d.join(id)).find(|p| p.is_file()) };
    let Some(source) = source else { bail!("no autostart entry {id}") };
    let text = std::fs::read_to_string(&source).with_context(|| format!("reading {}", source.display()))?;
    let mut updated = set_key(&text, "Hidden", if enabled { "false" } else { "true" });
    if text.lines().any(|l| l.trim().starts_with("X-GNOME-Autostart-enabled=")) {
        updated = set_key(&updated, "X-GNOME-Autostart-enabled", if enabled { "true" } else { "false" });
    }
    std::fs::create_dir_all(user)?;
    crate::store::write_atomic(&target, updated.as_bytes())
}

pub fn set_autostart_enabled(id: &str, enabled: bool) -> Result<()> {
    let (user, system) = autostart_dirs();
    set_autostart_enabled_in(&user, &system, id, enabled)
}

/// A file-name-safe id from a name (`My Script` → `my-script`).
fn slug(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "entry".into() } else { s }
}

/// Adds an autostart entry: a copy of the app `desktop_id`, or a command
/// (`name` + `command`). Returns the new id. Never overwrites a file.
pub fn add_autostart_in(user: &Path, apps: &[PathBuf], what: &AutostartNew) -> Result<String> {
    std::fs::create_dir_all(user)?;
    let (id, text) = match what {
        AutostartNew::App(desktop_id) => {
            let source = apps.iter().map(|d| d.join(desktop_id)).find(|p| p.is_file()).with_context(|| format!("no app {desktop_id}"))?;
            let text = std::fs::read_to_string(&source)?;
            (desktop_id.clone(), set_key(&set_key(&text, "Hidden", "false"), "X-Mandara-Created", "true"))
        }
        AutostartNew::Command { name, command } => {
            if command.trim().is_empty() {
                bail!("the command is empty");
            }
            let name = if name.trim().is_empty() { command.split_whitespace().next().unwrap_or("command").to_string() } else { name.trim().to_string() };
            let text = format!("[Desktop Entry]\nType=Application\nName={name}\nExec={}\nTerminal=false\n{MANDARA_CREATED}\n", command.trim());
            (format!("{}.desktop", slug(&name)), text)
        }
    };
    let base = id.trim_end_matches(".desktop").to_string();
    let mut id = id;
    let mut n = 1;
    while user.join(&id).exists() {
        n += 1;
        id = format!("{base}-{n}.desktop");
    }
    crate::store::write_atomic(&user.join(&id), text.as_bytes())?;
    Ok(id)
}

#[derive(Clone, Debug, PartialEq)]
pub enum AutostartNew {
    App(String),
    Command { name: String, command: String },
}

pub fn add_autostart(what: &AutostartNew) -> Result<String> {
    add_autostart_in(&autostart_dirs().0, &application_dirs(), what)
}

/// Removes an autostart entry Mandara created; others can only be disabled.
pub fn remove_autostart_in(user: &Path, id: &str) -> Result<()> {
    let path = user.join(id);
    if id.contains('/') {
        bail!("not an autostart entry id: {id:?}");
    }
    let text = std::fs::read_to_string(&path).with_context(|| format!("no autostart entry {id}"))?;
    if !text.contains(MANDARA_CREATED) {
        bail!("{id} wasn't added by Mandara; disable it instead");
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

pub fn remove_autostart(id: &str) -> Result<()> {
    remove_autostart_in(&autostart_dirs().0, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIMEAPPS: &str = "# my associations\n[Added Associations]\ntext/plain=org.gnome.TextEditor.desktop;\n\n[Default Applications]\n# keep me\nx-scheme-handler/https=firefox.desktop;\nimage/png=org.gnome.Loupe.desktop;\n\n[Removed Associations]\nvideo/mp4=vlc.desktop;\n";

    #[test]
    fn mimeapps_edit_preserves_everything_else() {
        let browser = category("browser").unwrap();
        let out = set_defaults_text(MIMEAPPS, browser.mime_types, "zen.desktop");
        assert!(out.starts_with("# my associations\n[Added Associations]\ntext/plain=org.gnome.TextEditor.desktop;\n\n[Default Applications]\n# keep me\nx-scheme-handler/https=zen.desktop;\n"));
        assert!(out.contains("image/png=org.gnome.Loupe.desktop;"));
        assert!(out.ends_with("[Removed Associations]\nvideo/mp4=vlc.desktop;\n"), "{out}");
        let defaults = parse_defaults(&out);
        for m in browser.mime_types {
            assert_eq!(defaults[*m], vec!["zen.desktop"], "{m}");
        }
        // New keys land inside the group, before the blank separator.
        let group = out.split("[Removed Associations]").next().unwrap();
        assert!(group.trim_end().ends_with("application/xhtml+xml=zen.desktop;"), "{group}");
        // Idempotent.
        assert_eq!(set_defaults_text(&out, browser.mime_types, "zen.desktop"), out);
    }

    #[test]
    fn mimeapps_group_is_created_when_missing() {
        let out = set_defaults_text("[Added Associations]\na/b=x.desktop;\n", &["inode/directory"], "thunar.desktop");
        assert_eq!(out, "[Added Associations]\na/b=x.desktop;\n\n[Default Applications]\ninode/directory=thunar.desktop;\n");
        assert_eq!(set_defaults_text("", &["a/b"], "x.desktop"), "[Default Applications]\na/b=x.desktop;\n");
    }

    #[test]
    fn set_default_backs_up_once_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mimeapps.list");
        std::fs::write(&path, MIMEAPPS).unwrap();
        let files = category("files").unwrap();
        set_default_at(&path, files, "thunar.desktop").unwrap();
        set_default_at(&path, files, "nautilus.desktop").unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("mimeapps.list.mandara-backup")).unwrap(), MIMEAPPS);
        assert_eq!(default_for(&path, "inode/directory").as_deref(), Some("nautilus.desktop"));
        assert!(set_default_at(&path, files, "../evil").is_err());
    }

    #[test]
    fn parses_desktop_entries_and_candidates() {
        let e = parse_desktop("firefox.desktop", "[Desktop Entry]\nName=Firefox\nName[ar]=فايرفوكس\nExec=firefox %u\nMimeType=text/html;x-scheme-handler/https;\n\n[Desktop Action new]\nName=New Window\nExec=firefox --new-window\n");
        assert_eq!(e.name, "Firefox");
        assert_eq!(exec_program(&e.exec), "firefox");
        let entries = vec![e, parse_desktop("vlc.desktop", "[Desktop Entry]\nName=VLC\nMimeType=video/mp4;\n")];
        let browsers = candidates(&entries, category("browser").unwrap());
        assert_eq!(browsers.len(), 1);
        assert_eq!(browsers[0].id, "firefox.desktop");
    }

    #[test]
    fn autostart_toggle_add_remove_in_a_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let (user, system, apps) = (dir.path().join("autostart"), dir.path().join("xdg/autostart"), dir.path().join("applications"));
        std::fs::create_dir_all(&system).unwrap();
        std::fs::create_dir_all(&apps).unwrap();
        let sys_text = "[Desktop Entry]\nName=Tracker\nExec=tracker-miner\nX-GNOME-Autostart-enabled=true\n";
        std::fs::write(system.join("tracker.desktop"), sys_text).unwrap();
        std::fs::write(apps.join("ghostty.desktop"), "[Desktop Entry]\nName=Ghostty\nExec=ghostty\n").unwrap();
        let systems = vec![system.clone()];
        let list = autostart_entries_in(&user, &systems);
        assert_eq!(list.len(), 1);
        assert!(list[0].enabled && list[0].origin == AutostartOrigin::System);
        // Disabling writes a user override; the system file is untouched.
        set_autostart_enabled_in(&user, &systems, "tracker.desktop", false).unwrap();
        assert_eq!(std::fs::read_to_string(system.join("tracker.desktop")).unwrap(), sys_text);
        let over = std::fs::read_to_string(user.join("tracker.desktop")).unwrap();
        assert!(over.contains("Hidden=true") && over.contains("X-GNOME-Autostart-enabled=false"), "{over}");
        assert!(!autostart_entries_in(&user, &systems)[0].enabled);
        set_autostart_enabled_in(&user, &systems, "tracker.desktop", true).unwrap();
        assert!(autostart_entries_in(&user, &systems)[0].enabled);
        // Adding an app and a command.
        let id = add_autostart_in(&user, std::slice::from_ref(&apps), &AutostartNew::App("ghostty.desktop".into())).unwrap();
        assert_eq!(id, "ghostty.desktop");
        let cmd = add_autostart_in(&user, &[apps.clone()], &AutostartNew::Command { name: "Sync notes".into(), command: "rclone sync a b".into() }).unwrap();
        assert_eq!(cmd, "sync-notes.desktop");
        let again = add_autostart_in(&user, &[apps.clone()], &AutostartNew::Command { name: "Sync notes".into(), command: "x".into() }).unwrap();
        assert_eq!(again, "sync-notes-2.desktop", "never overwrites");
        let list = autostart_entries_in(&user, &systems);
        assert_eq!(list.iter().filter(|e| e.mandara).count(), 3);
        // Only Mandara's own entries can be removed.
        remove_autostart_in(&user, &cmd).unwrap();
        assert!(remove_autostart_in(&user, "tracker.desktop").is_err());
        assert!(user.join("tracker.desktop").exists());
    }

    /// Which entries a login starts: the Studio toggle (`Hidden=`), the
    /// desktop filters and `TryExec`, all from files in a sandbox.
    #[test]
    fn autostart_run_list_follows_toggles_desktops_and_try_exec() {
        let dir = tempfile::tempdir().unwrap();
        let (user, system) = (dir.path().join("autostart"), dir.path().join("xdg/autostart"));
        std::fs::create_dir_all(&system).unwrap();
        let write = |name: &str, body: &str| std::fs::write(system.join(name), format!("[Desktop Entry]\nType=Application\nName={name}\n{body}")).unwrap();
        write("plain.desktop", "Exec=plain --tray\n");
        write("gnome-only.desktop", "Exec=g\nOnlyShowIn=GNOME;\n");
        write("not-mango.desktop", "Exec=n\nNotShowIn=mango;KDE;\n");
        write("for-mango.desktop", "Exec=m\nOnlyShowIn=MANGO;\n");
        write("missing.desktop", "Exec=missing\nTryExec=/nonexistent/missing\n");
        write("present.desktop", "Exec=present\nTryExec=sh\n");
        write("off.desktop", "Exec=off\nX-GNOME-Autostart-enabled=false\n");
        write("no-exec.desktop", "\n");
        let systems = vec![system.clone()];
        set_autostart_enabled_in(&user, &systems, "plain.desktop", false).unwrap();
        let entries = autostart_entries_in(&user, &systems);
        let desktops = vec!["mango".to_string(), "wlroots".to_string()];
        let exists = |p: &str| p == "sh";
        let ids = |list: Vec<&AutostartEntry>| list.into_iter().map(|e| e.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(autostart_to_run(&entries, &desktops, exists)), ["for-mango.desktop", "present.desktop"]);
        set_autostart_enabled_in(&user, &systems, "plain.desktop", true).unwrap();
        let entries = autostart_entries_in(&user, &systems);
        assert!(ids(autostart_to_run(&entries, &desktops, exists)).contains(&"plain.desktop".to_string()), "turning it back on in Studio starts it");
        assert!(ids(autostart_to_run(&entries, &["GNOME".into()], exists)).contains(&"gnome-only.desktop".to_string()));
        assert!(!AppsConfig::default().run_autostart, "opt-in: compositor configs often start the same apps");
    }

    #[test]
    fn app_id_aliases() {
        assert!(glob_match("chrome-*-Default", "chrome-abc123-Default"));
        assert!(glob_match("steam_app_*", "steam_app_570"));
        assert!(!glob_match("steam_app_*", "xsteam_app_570"));
        assert!(glob_match("*.exe", "game.EXE"));
        assert!(glob_match("Code", "code"));
        assert!(!glob_match("Code", "code-oss"));
        let list = vec![AppIdAlias { pattern: "steam_app_*".into(), desktop: "steam.desktop".into() }, AppIdAlias { pattern: " ".into(), desktop: "x".into() }];
        assert_eq!(alias_for(&list, "steam_app_570"), Some("steam"));
        assert_eq!(alias_for(&list, "firefox"), None);
    }

    #[test]
    fn exec_field_codes_are_stripped() {
        assert_eq!(exec_program("thunar %F"), "thunar");
        assert_eq!(exec_program("env A=1 app --x %U"), "env A=1 app --x");
        assert_eq!(resolve_command("  "), None);
        assert_eq!(resolve_command("definitely-not-an-app-xyz").as_deref(), Some("definitely-not-an-app-xyz"));
    }
}
