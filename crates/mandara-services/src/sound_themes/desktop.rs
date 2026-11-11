//! The sound theme for apps (Studio → Themes → Sounds, "Apply to apps"):
//! the places freedesktop sound consumers read it from, changed only where
//! they already exist and only the one key, the way Mandara's theme sync
//! leaves your dotfiles alone.
//!
//! * **GTK / GNOME and libcanberra** — `org.gnome.desktop.sound
//!   theme-name` through `gsettings` (when it is installed and has the
//!   schema), and `gtk-sound-theme-name` in `~/.config/gtk-3.0/settings.ini`
//!   and `gtk-4.0/settings.ini` when those files exist.
//! * **KDE / Plasma and KWin** — `[Sounds] Theme=` in `~/.config/kdeglobals`
//!   when the file exists or a Plasma session runs: KNotification, and so
//!   KWin and Plasma's own notifications, plays from it.
//! * Other compositors play no sounds of their own; nothing to set there.
//!
//! The first time Mandara sets a theme it records each value it is about to
//! change ([`Saved`]); "Your desktop's" ([`apply`] with `None`) puts those
//! values back and forgets them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// GSettings schema and key apps read the sound theme from.
pub const GTK_SCHEMA: &str = "org.gnome.desktop.sound";
pub const GTK_THEME_KEY: &str = "theme-name";
/// `settings.ini` group and key.
const GTK_GROUP: &str = "Settings";
const GTK_INI_KEY: &str = "gtk-sound-theme-name";
const GTK_DIRS: [&str; 2] = ["gtk-3.0", "gtk-4.0"];
/// `kdeglobals` group and key.
const KDE_GROUP: &str = "Sounds";
const KDE_KEY: &str = "Theme";
const KDEGLOBALS: &str = "kdeglobals";
/// How long a `gsettings` call may take.
const GSETTINGS_TIMEOUT: Duration = Duration::from_secs(4);
const GSETTINGS_POLL: Duration = Duration::from_millis(20);

/// The desktop's own values from before Mandara first set a theme.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    /// Whether the values below were recorded (Mandara has set a theme).
    pub recorded: bool,
    /// `org.gnome.desktop.sound theme-name`, when gsettings had it.
    pub gsettings: Option<String>,
    /// Each file key Mandara set, with its earlier value (`None`: the key
    /// wasn't there).
    pub files: Vec<SavedKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedKey {
    pub path: PathBuf,
    pub group: String,
    pub key: String,
    pub value: Option<String>,
}

/// Reads and writes GSettings keys.
pub trait Settings {
    fn get(&self, schema: &str, key: &str) -> Option<String>;
    fn set(&self, schema: &str, key: &str, value: &str) -> bool;
}

/// The `gsettings` program (absent: every call answers `None` / `false`).
pub struct Gsettings;

impl Gsettings {
    fn run(args: &[&str]) -> Option<String> {
        let program = crate::updates::find_program("gsettings")?;
        let mut child = std::process::Command::new(program).args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().ok()?;
        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut out = String::new();
                    use std::io::Read as _;
                    child.stdout.take()?.read_to_string(&mut out).ok()?;
                    return status.success().then_some(out);
                }
                Ok(None) if start.elapsed() < GSETTINGS_TIMEOUT => std::thread::sleep(GSETTINGS_POLL),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
    }
}

impl Settings for Gsettings {
    fn get(&self, schema: &str, key: &str) -> Option<String> {
        Self::run(&["get", schema, key]).map(|v| unquote(v.trim()))
    }

    fn set(&self, schema: &str, key: &str, value: &str) -> bool {
        Self::run(&["set", schema, key, &quote(value)]).is_some()
    }
}

/// A GVariant string literal.
fn quote(v: &str) -> String {
    format!("'{}'", v.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// The text of a printed GVariant string (`'ocean'` → `ocean`).
fn unquote(v: &str) -> String {
    let inner = v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')).or_else(|| v.strip_prefix('"').and_then(|s| s.strip_suffix('"')));
    match inner {
        Some(s) => s.replace("\\'", "'").replace("\\\\", "\\"),
        None => v.to_string(),
    }
}

/// `key`'s value in `[group]` of an ini text.
pub fn ini_get(text: &str, group: &str, key: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines() {
        let l = line.trim();
        if let Some(name) = l.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            inside = name == group;
        } else if inside && let Some((k, v)) = l.split_once('=') && k.trim() == key {
            return Some(v.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// `text` with `key` in `[group]` set to `value` (replaced in place, added at
/// the group's end, or with the group appended), or removed (`None`).
/// Every other line, comment and group stays as it was.
pub fn ini_set(text: &str, group: &str, key: &str, value: Option<&str>) -> String {
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let mut inside = false;
    let mut group_end: Option<usize> = None;
    let mut found: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        let l = line.trim();
        if let Some(name) = l.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            inside = name == group;
            if inside {
                group_end = Some(i);
            }
            continue;
        }
        if inside {
            if !l.is_empty() {
                group_end = Some(i);
            }
            if found.is_none() && l.split_once('=').is_some_and(|(k, _)| k.trim() == key) {
                found = Some(i);
            }
        }
    }
    match (found, value) {
        (Some(i), Some(v)) => lines[i] = format!("{key}={v}"),
        (Some(i), None) => {
            lines.remove(i);
        }
        (None, Some(v)) => match group_end {
            Some(end) => lines.insert(end + 1, format!("{key}={v}")),
            None => {
                if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                    lines.push(String::new());
                }
                lines.push(format!("[{group}]"));
                lines.push(format!("{key}={v}"));
            }
        },
        (None, None) => return text.to_string(),
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Where the sound theme lives for apps.
#[derive(Clone, Debug)]
pub struct DesktopEnv {
    /// `~/.config` (`$XDG_CONFIG_HOME`).
    pub config: PathBuf,
    /// A Plasma session runs (`kdeglobals` is written even if new).
    pub plasma: bool,
}

impl DesktopEnv {
    pub fn current() -> Self {
        let plasma = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|p| p.eq_ignore_ascii_case("KDE")))
            || mandara_compositor::detect() == Some(mandara_compositor::CompositorKind::KWin);
        Self { config: dirs::config_dir().unwrap_or_default(), plasma }
    }

    /// The files edited, as `(path, group, key)`: the GTK `settings.ini`
    /// files that exist, and `kdeglobals` when it exists or Plasma runs.
    pub fn targets(&self) -> Vec<(PathBuf, &'static str, &'static str)> {
        let mut out: Vec<(PathBuf, &'static str, &'static str)> = GTK_DIRS.iter().map(|d| self.config.join(d).join("settings.ini")).filter(|p| p.is_file()).map(|p| (p, GTK_GROUP, GTK_INI_KEY)).collect();
        let kde = self.config.join(KDEGLOBALS);
        if kde.is_file() || self.plasma {
            out.push((kde, KDE_GROUP, KDE_KEY));
        }
        out
    }
}

/// What [`apply`] changed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Applied {
    /// `gsettings` took the theme (or the earlier value).
    pub gsettings: bool,
    pub files: Vec<PathBuf>,
}

fn write_key(path: &Path, group: &str, key: &str, value: Option<&str>) -> Result<(), String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let next = ini_set(&text, group, key, value);
    if next == text {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    mandara_config::write_atomic(path, next.as_bytes()).map_err(|e| format!("{}: {e:#}", path.display()))
}

/// Sets `theme` for apps (`None`: puts back what was there before Mandara
/// first set one). `saved` records the earlier values; persist it after.
pub fn apply(theme: Option<&str>, env: &DesktopEnv, gs: &dyn Settings, saved: &mut Saved) -> Result<Applied, String> {
    let mut done = Applied::default();
    let Some(theme) = theme else {
        if !saved.recorded {
            return Ok(done);
        }
        if let Some(before) = saved.gsettings.take() {
            done.gsettings = gs.set(GTK_SCHEMA, GTK_THEME_KEY, &before);
        }
        for k in std::mem::take(&mut saved.files) {
            write_key(&k.path, &k.group, &k.key, k.value.as_deref())?;
            done.files.push(k.path);
        }
        saved.recorded = false;
        return Ok(done);
    };
    // Absent schema (no GNOME settings installed): left alone.
    let current = gs.get(GTK_SCHEMA, GTK_THEME_KEY);
    if !saved.recorded {
        saved.gsettings = current.clone();
    }
    if current.is_some() {
        done.gsettings = gs.set(GTK_SCHEMA, GTK_THEME_KEY, theme);
    }
    for (path, group, key) in env.targets() {
        if !saved.files.iter().any(|k| k.path == path && k.group == group && k.key == key) {
            let before = std::fs::read_to_string(&path).ok().and_then(|t| ini_get(&t, group, key));
            saved.files.push(SavedKey { path: path.clone(), group: group.to_string(), key: key.to_string(), value: before });
        }
        write_key(&path, group, key, Some(theme))?;
        done.files.push(path);
    }
    saved.recorded = true;
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// GSettings in memory; `None` when the schema isn't installed.
    struct Fake(Option<RefCell<HashMap<String, String>>>);

    impl Settings for Fake {
        fn get(&self, schema: &str, key: &str) -> Option<String> {
            self.0.as_ref()?.borrow().get(&format!("{schema}/{key}")).cloned()
        }
        fn set(&self, schema: &str, key: &str, value: &str) -> bool {
            let Some(m) = &self.0 else { return false };
            m.borrow_mut().insert(format!("{schema}/{key}"), value.to_string());
            true
        }
    }

    fn with_gtk(theme: &str) -> Fake {
        Fake(Some(RefCell::new(HashMap::from([(format!("{GTK_SCHEMA}/{GTK_THEME_KEY}"), theme.to_string())]))))
    }

    #[test]
    fn ini_edits_touch_only_their_key() {
        let text = "# mine\n[Settings]\ngtk-theme-name=Adwaita\ngtk-sound-theme-name=freedesktop\n\n[Other]\nx=1\n";
        let set = ini_set(text, "Settings", "gtk-sound-theme-name", Some("ocean"));
        assert_eq!(set, "# mine\n[Settings]\ngtk-theme-name=Adwaita\ngtk-sound-theme-name=ocean\n\n[Other]\nx=1\n");
        assert_eq!(ini_get(&set, "Settings", "gtk-sound-theme-name").as_deref(), Some("ocean"));
        assert_eq!(ini_get(&set, "Other", "gtk-sound-theme-name"), None);
        // Added at the end of its group, before the next one.
        let added = ini_set("[Settings]\na=1\n\n[Other]\nb=2\n", "Settings", "k", Some("v"));
        assert_eq!(added, "[Settings]\na=1\nk=v\n\n[Other]\nb=2\n");
        // A missing group is appended; removal leaves the rest.
        let appended = ini_set("[General]\nx=1\n", "Sounds", "Theme", Some("ocean"));
        assert_eq!(appended, "[General]\nx=1\n\n[Sounds]\nTheme=ocean\n");
        assert_eq!(ini_set(&appended, "Sounds", "Theme", None), "[General]\nx=1\n\n[Sounds]\n");
        assert_eq!(ini_set("a\n", "G", "k", None), "a\n", "nothing to remove: unchanged");
    }

    #[test]
    fn gvariant_strings_round_trip() {
        assert_eq!(quote("it's"), "'it\\'s'");
        assert_eq!(unquote(&quote("it's \\ ok")), "it's \\ ok");
        assert_eq!(unquote("'ocean'"), "ocean");
    }

    #[test]
    fn apply_writes_only_existing_files_and_restores_them() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        std::fs::create_dir_all(config.join("gtk-3.0")).unwrap();
        let gtk3 = config.join("gtk-3.0/settings.ini");
        std::fs::write(&gtk3, "[Settings]\ngtk-theme-name=Adwaita\n").unwrap();
        let env = DesktopEnv { config: config.clone(), plasma: false };
        let gs = with_gtk("freedesktop");
        let mut saved = Saved::default();

        let done = apply(Some("ocean"), &env, &gs, &mut saved).unwrap();
        assert!(done.gsettings);
        assert_eq!(done.files, vec![gtk3.clone()], "gtk-4.0 and kdeglobals don't exist: left alone");
        assert!(!config.join("gtk-4.0").exists() && !config.join("kdeglobals").exists());
        assert_eq!(std::fs::read_to_string(&gtk3).unwrap(), "[Settings]\ngtk-theme-name=Adwaita\ngtk-sound-theme-name=ocean\n");
        assert_eq!(gs.get(GTK_SCHEMA, GTK_THEME_KEY).as_deref(), Some("ocean"));
        assert_eq!(saved.gsettings.as_deref(), Some("freedesktop"));

        // A second theme keeps the first recorded values.
        apply(Some("oxygen"), &env, &gs, &mut saved).unwrap();
        assert_eq!(saved.gsettings.as_deref(), Some("freedesktop"));
        assert_eq!(saved.files.len(), 1);

        // Back to the desktop's own: the key goes again, gsettings returns.
        apply(None, &env, &gs, &mut saved).unwrap();
        assert_eq!(std::fs::read_to_string(&gtk3).unwrap(), "[Settings]\ngtk-theme-name=Adwaita\n");
        assert_eq!(gs.get(GTK_SCHEMA, GTK_THEME_KEY).as_deref(), Some("freedesktop"));
        assert_eq!(saved, Saved::default());
    }

    #[test]
    fn plasma_gets_kdeglobals_and_missing_gsettings_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().to_path_buf();
        std::fs::write(config.join("kdeglobals"), "[General]\nColorScheme=Mandara\n\n[Sounds]\nTheme=freedesktop\n").unwrap();
        let env = DesktopEnv { config: config.clone(), plasma: false };
        let gs = Fake(None);
        let mut saved = Saved::default();
        let done = apply(Some("ocean"), &env, &gs, &mut saved).unwrap();
        assert!(!done.gsettings);
        assert_eq!(ini_get(&std::fs::read_to_string(config.join("kdeglobals")).unwrap(), "Sounds", "Theme").as_deref(), Some("ocean"));
        apply(None, &env, &gs, &mut saved).unwrap();
        assert_eq!(std::fs::read_to_string(config.join("kdeglobals")).unwrap(), "[General]\nColorScheme=Mandara\n\n[Sounds]\nTheme=freedesktop\n");
        // In a Plasma session a missing kdeglobals is created.
        let fresh = tempfile::tempdir().unwrap();
        let env = DesktopEnv { config: fresh.path().to_path_buf(), plasma: true };
        apply(Some("ocean"), &env, &gs, &mut Saved::default()).unwrap();
        assert_eq!(std::fs::read_to_string(fresh.path().join("kdeglobals")).unwrap(), "[Sounds]\nTheme=ocean\n");
        // Nothing recorded: restoring changes nothing.
        let mut none = Saved::default();
        assert_eq!(apply(None, &env, &gs, &mut none).unwrap(), Applied::default());
    }
}
