//! Making an icon theme the desktop's: the settings GTK and Qt / KDE apps
//! read, changed one key at a time and recorded so "Follow your desktop"
//! puts back exactly what was there.
//!
//! | Where | Key |
//! |---|---|
//! | `gsettings` (when installed) | `org.gnome.desktop.interface icon-theme` |
//! | `~/.config/gtk-3.0/settings.ini`, `gtk-4.0/settings.ini` | `[Settings] gtk-icon-theme-name` |
//! | `~/.config/kdeglobals` | `[Icons] Theme` |
//! | `~/.config/qt5ct/qt5ct.conf`, `qt6ct/qt6ct.conf` (only when present) | `[Appearance] icon_theme` |
//!
//! Every other line of those files stays as it is. The first apply records
//! each key's earlier value (or that it was missing, or that the file was)
//! in `~/.config/mandara/generated/icon-theme.json`. Going back restores a
//! key only while it still holds the value Mandara set: a change made
//! since, in the file or another app, wins and is left alone. Mandara's
//! own icons (`appearance.icon_theme`) are the caller's to set.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const RECORD_FILE: &str = "icon-theme.json";
const GSETTINGS_SCHEMA: &str = "org.gnome.desktop.interface";
const GSETTINGS_KEY: &str = "icon-theme";

/// One `[section] key` in one file.
#[derive(Clone, Debug, PartialEq, Eq)]
struct IniKey {
    file: &'static str,
    section: &'static str,
    key: &'static str,
    /// Written only when the file already exists.
    only_existing: bool,
}

const INI_KEYS: [IniKey; 5] = [
    IniKey { file: "gtk-3.0/settings.ini", section: "Settings", key: "gtk-icon-theme-name", only_existing: false },
    IniKey { file: "gtk-4.0/settings.ini", section: "Settings", key: "gtk-icon-theme-name", only_existing: false },
    IniKey { file: "kdeglobals", section: "Icons", key: "Theme", only_existing: false },
    IniKey { file: "qt5ct/qt5ct.conf", section: "Appearance", key: "icon_theme", only_existing: true },
    IniKey { file: "qt6ct/qt6ct.conf", section: "Appearance", key: "icon_theme", only_existing: true },
];

/// The `gsettings` tool, replaceable in tests.
pub trait Gsettings {
    /// The current value, `None` when the schema isn't installed.
    fn get(&self) -> Option<String>;
    fn set(&self, value: &str) -> bool;
}

/// The real `gsettings` command (absent: nothing to do).
pub struct GsettingsCommand;

impl Gsettings for GsettingsCommand {
    fn get(&self) -> Option<String> {
        let out = std::process::Command::new("gsettings").args(["get", GSETTINGS_SCHEMA, GSETTINGS_KEY]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
        out.status.success().then(|| unquote(String::from_utf8_lossy(&out.stdout).trim()))
    }

    fn set(&self, value: &str) -> bool {
        std::process::Command::new("gsettings")
            .args(["set", GSETTINGS_SCHEMA, GSETTINGS_KEY, value])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
}

/// `'Papirus'` → `Papirus`.
fn unquote(s: &str) -> String {
    s.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')).unwrap_or(s).to_string()
}

/// What one key held before Mandara set it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Before {
    /// `file` (relative to the config folder) or `gsettings`.
    pub target: String,
    /// Its earlier value; `None` when the key was missing.
    pub value: Option<String>,
    /// The file didn't exist.
    #[serde(default)]
    pub created: bool,
}

/// The values Mandara set and what they replaced.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The theme Mandara made the desktop's.
    pub theme: String,
    pub before: Vec<Before>,
}

/// Where apply writes (a temp folder in tests).
#[derive(Clone, Debug)]
pub struct ApplyEnv {
    /// `~/.config`.
    pub config: PathBuf,
    /// The record file.
    pub record: PathBuf,
}

impl ApplyEnv {
    pub fn current() -> Self {
        Self { config: dirs::config_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config")), record: mandara_config::paths::generated_dir().join(RECORD_FILE) }
    }

    pub fn under(root: &Path) -> Self {
        Self { config: root.join("config"), record: root.join(RECORD_FILE) }
    }
}

/// The theme Mandara last made the desktop's (`None`: it follows the
/// desktop's own setting).
pub fn applied(env: &ApplyEnv) -> Option<String> {
    load(&env.record).map(|r| r.theme)
}

fn load(path: &Path) -> Option<Record> {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok())
}

/// Reads `[section] key` from INI text (the last occurrence wins).
pub fn ini_get(text: &str, section: &str, key: &str) -> Option<String> {
    let mut active = false;
    let mut value = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with(['#', ';']) {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            active = name == section;
            continue;
        }
        if active && let Some((k, v)) = line.split_once('=') && k.trim() == key {
            value = Some(v.trim().to_string());
        }
    }
    value
}

/// `text` with `[section] key` set to `value` (or removed with `None`).
/// Every other line, comment and blank stays; a missing section is added
/// at the end.
pub fn ini_set(text: &str, section: &str, key: &str, value: Option<&str>) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut active = false;
    let mut seen_section = false;
    let mut done = false;
    let mut section_end: Option<usize> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            if active && !done {
                section_end = Some(out.len());
            }
            active = name == section;
            seen_section |= active;
            out.push(line.to_string());
            continue;
        }
        if active && !trimmed.starts_with(['#', ';']) && let Some((k, _)) = trimmed.split_once('=') && k.trim() == key {
            if !done && let Some(v) = value {
                out.push(format!("{key}={v}"));
            }
            done = true;
            continue;
        }
        out.push(line.to_string());
    }
    if active && !done {
        section_end = Some(out.len());
    }
    if !done && let Some(v) = value {
        let line = format!("{key}={v}");
        match section_end.filter(|_| seen_section) {
            Some(mut at) => {
                // Before the blank lines that close the section.
                while at > 0 && out[at - 1].trim().is_empty() {
                    at -= 1;
                }
                out.insert(at, line);
            }
            None => {
                if out.last().is_some_and(|l| !l.trim().is_empty()) {
                    out.push(String::new());
                }
                out.push(format!("[{section}]"));
                out.push(line);
            }
        }
    }
    let mut s = out.join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    s
}

/// Whether `text` holds nothing but empty `[section]` headers.
fn only_headers(text: &str) -> bool {
    text.lines().map(str::trim).all(|l| l.is_empty() || (l.starts_with('[') && l.ends_with(']')))
}

/// What an apply or a restore changed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    /// Files (relative to the config folder) and `gsettings` changed.
    pub changed: Vec<String>,
    /// Keys left alone because someone changed them since Mandara did.
    pub kept: Vec<String>,
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    mandara_config::write_atomic(path, text.as_bytes()).map_err(|e| format!("{e:#}"))
}

/// Makes `theme` the desktop's icon theme (see the module docs), keeping
/// the first record of what was there before.
pub fn apply(theme: &str, env: &ApplyEnv, gsettings: &dyn Gsettings) -> Result<Report, String> {
    if !crate::theme_pkg::valid_name(theme) {
        return Err(format!("{theme:?} isn't an icon theme name"));
    }
    let mut record = load(&env.record).unwrap_or_default();
    let mut report = Report::default();
    for k in &INI_KEYS {
        let path = env.config.join(k.file);
        let current = std::fs::read_to_string(&path).ok();
        if current.is_none() && k.only_existing {
            continue;
        }
        let text = current.clone().unwrap_or_default();
        let value = ini_get(&text, k.section, k.key);
        if !record.before.iter().any(|b| b.target == k.file) {
            record.before.push(Before { target: k.file.to_string(), value: value.clone(), created: current.is_none() });
        }
        if value.as_deref() != Some(theme) {
            write(&path, &ini_set(&text, k.section, k.key, Some(theme)))?;
            report.changed.push(k.file.to_string());
        }
    }
    if let Some(value) = gsettings.get() {
        if !record.before.iter().any(|b| b.target == "gsettings") {
            record.before.push(Before { target: "gsettings".into(), value: Some(value.clone()), created: false });
        }
        if value != theme && gsettings.set(theme) {
            report.changed.push("gsettings".into());
        }
    }
    record.theme = theme.to_string();
    let text = serde_json::to_string_pretty(&record).map_err(|e| e.to_string())?;
    write(&env.record, &text)?;
    Ok(report)
}

/// Puts back what [`apply`] replaced, wherever the value is still the one
/// Mandara set, and forgets the record. Nothing to do without one.
pub fn restore(env: &ApplyEnv, gsettings: &dyn Gsettings) -> Result<Report, String> {
    let Some(record) = load(&env.record) else { return Ok(Report::default()) };
    let mut report = Report::default();
    for before in &record.before {
        if before.target == "gsettings" {
            match (gsettings.get(), &before.value) {
                (Some(now), Some(old)) if now == record.theme => {
                    if gsettings.set(old) {
                        report.changed.push(before.target.clone());
                    }
                }
                (Some(_), _) => report.kept.push(before.target.clone()),
                (None, _) => {}
            }
            continue;
        }
        let Some(k) = INI_KEYS.iter().find(|k| k.file == before.target) else { continue };
        let path = env.config.join(k.file);
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        if ini_get(&text, k.section, k.key).as_deref() != Some(record.theme.as_str()) {
            report.kept.push(before.target.clone());
            continue;
        }
        let next = ini_set(&text, k.section, k.key, before.value.as_deref());
        if before.created && only_headers(&next) {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
            // Its folder too, when Mandara's file was all it held.
            if let Some(dir) = path.parent().filter(|d| *d != env.config) {
                let _ = std::fs::remove_dir(dir);
            }
        } else {
            write(&path, &next)?;
        }
        report.changed.push(before.target.clone());
    }
    match std::fs::remove_file(&env.record) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeGsettings(RefCell<Option<String>>);

    impl Gsettings for FakeGsettings {
        fn get(&self) -> Option<String> {
            self.0.borrow().clone()
        }
        fn set(&self, value: &str) -> bool {
            *self.0.borrow_mut() = Some(value.to_string());
            true
        }
    }

    #[test]
    fn ini_set_touches_only_its_key() {
        let text = "# mine\n[Settings]\ngtk-theme-name=Adwaita\ngtk-icon-theme-name=Old\n\n[Other]\ngtk-icon-theme-name=keep\n";
        let next = ini_set(text, "Settings", "gtk-icon-theme-name", Some("Papirus"));
        assert_eq!(next, "# mine\n[Settings]\ngtk-theme-name=Adwaita\ngtk-icon-theme-name=Papirus\n\n[Other]\ngtk-icon-theme-name=keep\n");
        assert_eq!(ini_get(&next, "Settings", "gtk-icon-theme-name").as_deref(), Some("Papirus"));
        assert_eq!(ini_get(&next, "Other", "gtk-icon-theme-name").as_deref(), Some("keep"));
        // Added inside its section, before the blank line closing it.
        let added = ini_set("[Settings]\ngtk-theme-name=A\n\n[X]\na=1\n", "Settings", "gtk-icon-theme-name", Some("P"));
        assert_eq!(added, "[Settings]\ngtk-theme-name=A\ngtk-icon-theme-name=P\n\n[X]\na=1\n");
        // A missing section goes at the end; removing drops only the key.
        assert_eq!(ini_set("[General]\na=1\n", "Icons", "Theme", Some("P")), "[General]\na=1\n\n[Icons]\nTheme=P\n");
        assert_eq!(ini_set("", "Icons", "Theme", Some("P")), "[Icons]\nTheme=P\n");
        assert_eq!(ini_set("[Icons]\nTheme=P\nOther=1\n", "Icons", "Theme", None), "[Icons]\nOther=1\n");
        // Commented-out keys stay comments.
        assert_eq!(ini_set("[Icons]\n#Theme=x\n", "Icons", "Theme", Some("P")), "[Icons]\n#Theme=x\nTheme=P\n");
    }

    #[test]
    fn apply_writes_only_the_expected_keys_and_restore_puts_them_back() {
        let root = tempfile::tempdir().unwrap();
        let env = ApplyEnv::under(root.path());
        let cfg = &env.config;
        std::fs::create_dir_all(cfg.join("gtk-3.0")).unwrap();
        std::fs::create_dir_all(cfg.join("qt6ct")).unwrap();
        let gtk3 = "[Settings]\ngtk-theme-name=Adwaita-dark\ngtk-icon-theme-name=Adwaita\ngtk-font-name=Inter 11\n";
        let qt6 = "[Appearance]\nstyle=Fusion\nicon_theme=breeze\n\n[Fonts]\ngeneral=Inter\n";
        std::fs::write(cfg.join("gtk-3.0/settings.ini"), gtk3).unwrap();
        std::fs::write(cfg.join("qt6ct/qt6ct.conf"), qt6).unwrap();
        let gs = FakeGsettings(RefCell::new(Some("Adwaita".into())));
        let report = apply("Papirus-Dark", &env, &gs).unwrap();
        assert_eq!(report.changed, vec!["gtk-3.0/settings.ini", "gtk-4.0/settings.ini", "kdeglobals", "qt6ct/qt6ct.conf", "gsettings"]);
        assert_eq!(std::fs::read_to_string(cfg.join("gtk-3.0/settings.ini")).unwrap(), gtk3.replace("gtk-icon-theme-name=Adwaita", "gtk-icon-theme-name=Papirus-Dark"));
        assert_eq!(std::fs::read_to_string(cfg.join("gtk-4.0/settings.ini")).unwrap(), "[Settings]\ngtk-icon-theme-name=Papirus-Dark\n");
        assert_eq!(std::fs::read_to_string(cfg.join("kdeglobals")).unwrap(), "[Icons]\nTheme=Papirus-Dark\n");
        assert_eq!(std::fs::read_to_string(cfg.join("qt6ct/qt6ct.conf")).unwrap(), qt6.replace("icon_theme=breeze", "icon_theme=Papirus-Dark"));
        assert!(!cfg.join("qt5ct").exists(), "qt5ct isn't set up: left alone");
        assert_eq!(gs.get().as_deref(), Some("Papirus-Dark"));
        assert_eq!(applied(&env).as_deref(), Some("Papirus-Dark"));
        // Nothing else appeared in the config folder.
        let mut made: Vec<String> = walk(cfg);
        made.sort();
        assert_eq!(made, vec!["gtk-3.0/settings.ini", "gtk-4.0/settings.ini", "kdeglobals", "qt6ct/qt6ct.conf"]);
        // A second theme keeps the first record of what was there.
        apply("Tela", &env, &gs).unwrap();
        // Someone changes kdeglobals by hand meanwhile: that one stays.
        std::fs::write(cfg.join("kdeglobals"), "[Icons]\nTheme=breeze-dark\n").unwrap();
        let back = restore(&env, &gs).unwrap();
        assert_eq!(back.kept, vec!["kdeglobals"]);
        assert_eq!(std::fs::read_to_string(cfg.join("gtk-3.0/settings.ini")).unwrap(), gtk3);
        assert_eq!(std::fs::read_to_string(cfg.join("qt6ct/qt6ct.conf")).unwrap(), qt6);
        assert!(!cfg.join("gtk-4.0/settings.ini").exists(), "a file Mandara created goes again");
        assert_eq!(std::fs::read_to_string(cfg.join("kdeglobals")).unwrap(), "[Icons]\nTheme=breeze-dark\n");
        assert_eq!(gs.get().as_deref(), Some("Adwaita"));
        assert_eq!(applied(&env), None);
        // Restoring again does nothing.
        assert_eq!(restore(&env, &gs).unwrap(), Report::default());
    }

    #[test]
    fn without_gsettings_or_with_bad_names_nothing_breaks() {
        let root = tempfile::tempdir().unwrap();
        let env = ApplyEnv::under(root.path());
        let gs = FakeGsettings::default();
        assert!(apply("../evil", &env, &gs).is_err());
        assert!(!env.config.exists());
        apply("Papirus", &env, &gs).unwrap();
        assert_eq!(gs.get(), None, "no schema: not set");
        restore(&env, &gs).unwrap();
        assert!(walk(&env.config).is_empty());
    }

    fn walk(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                if e.path().is_dir() {
                    stack.push(e.path());
                } else {
                    out.push(e.path().strip_prefix(dir).unwrap().to_string_lossy().into_owned());
                }
            }
        }
        out
    }
}
