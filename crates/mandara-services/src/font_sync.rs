//! Push Mandara's typefaces to the desktop (opt-in): fontconfig aliases,
//! GTK / gsettings, Qt (qt5ct / qt6ct) and KDE (`kdeglobals`). Web browsers
//! on Linux follow fontconfig, so the browsers toggle shares that drop-in.
//!
//! Writes are recorded in `~/.config/mandara/generated/font-sync.json` and
//! restored when a target (or the master switch) is turned off, the same
//! way icon-theme apply works: a key someone changed since Mandara set it
//! is left alone.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::icons::apply::{ini_get, ini_set};

/// Record of what Mandara last wrote for system fonts.
const RECORD_FILE: &str = "font-sync.json";
/// Drop-in under `~/.config/fontconfig/conf.d/`.
const FONTCONFIG_REL: &str = "fontconfig/conf.d/50-mandara-fonts.conf";
/// Point size used for GTK / Qt / KDE desktop fonts (shell scale is separate).
const POINT_SIZE: u32 = 11;
const GSETTINGS_SCHEMA: &str = "org.gnome.desktop.interface";

/// Which desktop targets to write.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Targets {
    pub fontconfig: bool,
    pub gtk: bool,
    pub qt: bool,
    pub kde: bool,
    /// Same fontconfig aliases browsers honour on Linux.
    pub browsers: bool,
}

impl Targets {
    pub fn any(self) -> bool {
        self.fontconfig || self.gtk || self.qt || self.kde || self.browsers
    }

    pub fn wants_fontconfig(self) -> bool {
        self.fontconfig || self.browsers
    }
}

/// Families Mandara pushes to the desktop.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Families {
    pub ui: String,
    pub display: String,
    pub mono: String,
}

impl Families {
    pub fn from_typography(t: &mandara_config::TypographyConfig) -> Self {
        Self { ui: t.ui_font.clone(), display: t.display_font.clone(), mono: t.mono_font.clone() }
    }

    fn gtk_ui(&self) -> String {
        format!("{} {POINT_SIZE}", self.ui)
    }

    fn gtk_mono(&self) -> String {
        format!("{} {POINT_SIZE}", self.mono)
    }

    fn gtk_document(&self) -> String {
        format!("{} {POINT_SIZE}", self.display)
    }

    fn qt_font(&self, family: &str) -> String {
        // qt5ct / qt6ct accept a bare family or a Qt font string.
        format!("{family},{POINT_SIZE},-1,5,50,0,0,0,0,0")
    }

    fn kde_font(&self, family: &str) -> String {
        format!("{family},{POINT_SIZE},-1,5,50,0,0,0,0,0")
    }
}

/// One file or gsettings key Mandara touched.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Before {
    /// Relative config path, `fontconfig`, or a `gsettings:<key>` id.
    pub target: String,
    /// Which Studio target owns this write (`fontconfig` covers browsers).
    pub group: String,
    pub value: Option<String>,
    #[serde(default)]
    pub created: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub families: Option<Families>,
    pub before: Vec<Before>,
}

#[derive(Clone, Debug)]
pub struct ApplyEnv {
    pub config: PathBuf,
    pub record: PathBuf,
}

impl ApplyEnv {
    pub fn current() -> Self {
        Self {
            config: dirs::config_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config")),
            record: mandara_config::paths::generated_dir().join(RECORD_FILE),
        }
    }

    pub fn under(root: &Path) -> Self {
        Self { config: root.join("config"), record: root.join(RECORD_FILE) }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    pub changed: Vec<String>,
    pub kept: Vec<String>,
}

/// `gsettings` for the three interface font keys.
pub trait FontGsettings {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, value: &str) -> bool;
}

pub struct FontGsettingsCommand;

impl FontGsettings for FontGsettingsCommand {
    fn get(&self, key: &str) -> Option<String> {
        let out = std::process::Command::new("gsettings").args(["get", GSETTINGS_SCHEMA, key]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
        out.status.success().then(|| unquote(String::from_utf8_lossy(&out.stdout).trim()))
    }

    fn set(&self, key: &str, value: &str) -> bool {
        std::process::Command::new("gsettings")
            .args(["set", GSETTINGS_SCHEMA, key, value])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
}

fn unquote(s: &str) -> String {
    s.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')).unwrap_or(s).to_string()
}

fn load(path: &Path) -> Option<Record> {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok())
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    mandara_config::write_atomic(path, text.as_bytes()).map_err(|e| format!("{e:#}"))
}

fn save_record(env: &ApplyEnv, record: &Record) -> Result<(), String> {
    let text = serde_json::to_string_pretty(record).map_err(|e| e.to_string())?;
    write(&env.record, &text)
}

fn fontconfig_body(f: &Families) -> String {
    format!(
        concat!(
            "<?xml version=\"1.0\"?>\n",
            "<!DOCTYPE fontconfig SYSTEM \"urn:fontconfig:fonts.dtd\">\n",
            "<!-- Mandara: Studio → Typography → System fonts. -->\n",
            "<fontconfig>\n",
            "  <alias>\n",
            "    <family>sans-serif</family>\n",
            "    <prefer><family>{ui}</family></prefer>\n",
            "  </alias>\n",
            "  <alias>\n",
            "    <family>serif</family>\n",
            "    <prefer><family>{display}</family></prefer>\n",
            "  </alias>\n",
            "  <alias>\n",
            "    <family>monospace</family>\n",
            "    <prefer><family>{mono}</family></prefer>\n",
            "  </alias>\n",
            "</fontconfig>\n"
        ),
        ui = xml_escape(&f.ui),
        display = xml_escape(&f.display),
        mono = xml_escape(&f.mono),
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn remember(record: &mut Record, target: &str, group: &str, value: Option<String>, created: bool) {
    if !record.before.iter().any(|b| b.target == target) {
        record.before.push(Before { target: target.into(), group: group.into(), value, created });
    }
}

fn only_headers(text: &str) -> bool {
    text.lines().map(str::trim).all(|l| l.is_empty() || (l.starts_with('[') && l.ends_with(']')))
}

struct IniWrite {
    file: &'static str,
    section: &'static str,
    key: &'static str,
    group: &'static str,
    only_existing: bool,
    value: String,
}

fn ini_target(file: &str, key: &str) -> String {
    format!("{file}::{key}")
}

/// Persist ownership after each successful mutation so a later failure still
/// leaves a restore record for what already landed.
fn checkpoint(env: &ApplyEnv, record: &Record) -> Result<(), String> {
    if record.before.is_empty() {
        match std::fs::remove_file(&env.record) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    } else {
        save_record(env, record)
    }
}

fn remove_file_ok(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn apply_ini(env: &ApplyEnv, record: &mut Record, report: &mut Report, w: IniWrite) -> Result<(), String> {
    let path = env.config.join(w.file);
    let current = std::fs::read_to_string(&path).ok();
    if current.is_none() && w.only_existing {
        return Ok(());
    }
    let text = current.clone().unwrap_or_default();
    let previous = ini_get(&text, w.section, w.key);
    let target = ini_target(w.file, w.key);
    remember(record, &target, w.group, previous.clone(), current.is_none());
    // Own the key before mutating so a write error still restores correctly.
    checkpoint(env, record)?;
    if previous.as_deref() != Some(w.value.as_str()) {
        write(&path, &ini_set(&text, w.section, w.key, Some(&w.value)))?;
        if !report.changed.iter().any(|c| c == w.file) {
            report.changed.push(w.file.into());
        }
        checkpoint(env, record)?;
    }
    Ok(())
}

fn apply_gsettings(env: &ApplyEnv, gs: &dyn FontGsettings, record: &mut Record, report: &mut Report, key: &str, value: &str) -> Result<(), String> {
    let target = format!("gsettings:{key}");
    let Some(previous) = gs.get(key) else { return Ok(()) };
    remember(record, &target, "gtk", Some(previous.clone()), false);
    checkpoint(env, record)?;
    if previous != value {
        if !gs.set(key, value) {
            return Err(format!("gsettings set {GSETTINGS_SCHEMA} {key} failed"));
        }
        report.changed.push(target);
        checkpoint(env, record)?;
    }
    Ok(())
}

fn apply_fontconfig(env: &ApplyEnv, record: &mut Record, report: &mut Report, families: &Families) -> Result<(), String> {
    let path = env.config.join(FONTCONFIG_REL);
    let body = fontconfig_body(families);
    let current = std::fs::read_to_string(&path).ok();
    remember(record, FONTCONFIG_REL, "fontconfig", current.clone(), current.is_none());
    checkpoint(env, record)?;
    if current.as_deref() != Some(body.as_str()) {
        write(&path, &body)?;
        report.changed.push(FONTCONFIG_REL.into());
        checkpoint(env, record)?;
    }
    Ok(())
}

/// Writes the enabled targets; keeps the first record of what was there.
pub fn apply(families: &Families, targets: Targets, env: &ApplyEnv, gsettings: &dyn FontGsettings) -> Result<Report, String> {
    if !targets.any() {
        return restore_groups(env, gsettings, None);
    }
    let mut record = load(&env.record).unwrap_or_default();
    let mut report = Report::default();
    // So a mid-apply restore knows which values Mandara meant to own.
    record.families = Some(families.clone());
    checkpoint(env, &record)?;

    // Drop writes for groups the user turned off.
    let keep = |g: &str| match g {
        "fontconfig" => targets.wants_fontconfig(),
        "gtk" => targets.gtk,
        "qt" => targets.qt,
        "kde" => targets.kde,
        _ => false,
    };
    let off: Vec<String> = record.before.iter().map(|b| b.group.clone()).filter(|g| !keep(g)).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    if !off.is_empty() {
        let partial = restore_groups_inner(env, gsettings, &mut record, Some(&off))?;
        report.changed.extend(partial.changed);
        report.kept.extend(partial.kept);
        checkpoint(env, &record)?;
    }

    if targets.wants_fontconfig() {
        apply_fontconfig(env, &mut record, &mut report, families)?;
    }
    if targets.gtk {
        apply_ini(
            env,
            &mut record,
            &mut report,
            IniWrite { file: "gtk-3.0/settings.ini", section: "Settings", key: "gtk-font-name", group: "gtk", only_existing: false, value: families.gtk_ui() },
        )?;
        apply_ini(
            env,
            &mut record,
            &mut report,
            IniWrite { file: "gtk-4.0/settings.ini", section: "Settings", key: "gtk-font-name", group: "gtk", only_existing: false, value: families.gtk_ui() },
        )?;
        apply_gsettings(env, gsettings, &mut record, &mut report, "font-name", &families.gtk_ui())?;
        apply_gsettings(env, gsettings, &mut record, &mut report, "document-font-name", &families.gtk_document())?;
        apply_gsettings(env, gsettings, &mut record, &mut report, "monospace-font-name", &families.gtk_mono())?;
    }
    if targets.qt {
        let general = families.qt_font(&families.ui);
        let fixed = families.qt_font(&families.mono);
        for (file, key, value) in [
            ("qt5ct/qt5ct.conf", "general", general.as_str()),
            ("qt5ct/qt5ct.conf", "fixed", fixed.as_str()),
            ("qt6ct/qt6ct.conf", "general", general.as_str()),
            ("qt6ct/qt6ct.conf", "fixed", fixed.as_str()),
        ] {
            apply_ini(
                env,
                &mut record,
                &mut report,
                IniWrite { file, section: "Fonts", key, group: "qt", only_existing: true, value: value.into() },
            )?;
        }
    }
    if targets.kde {
        let ui = families.kde_font(&families.ui);
        let mono = families.kde_font(&families.mono);
        for (key, value) in [("font", ui.as_str()), ("menuFont", ui.as_str()), ("toolBarFont", ui.as_str()), ("smallestReadableFont", ui.as_str()), ("fixed", mono.as_str())] {
            apply_ini(env, &mut record, &mut report, IniWrite { file: "kdeglobals", section: "General", key, group: "kde", only_existing: false, value: value.into() })?;
        }
    }

    checkpoint(env, &record)?;
    Ok(report)
}

/// Restores every recorded write (master switch off).
pub fn restore(env: &ApplyEnv, gsettings: &dyn FontGsettings) -> Result<Report, String> {
    restore_groups(env, gsettings, None)
}

fn persist_record(env: &ApplyEnv, record: &Record) -> Result<(), String> {
    if record.before.is_empty() {
        remove_file_ok(&env.record)
    } else {
        save_record(env, record)
    }
}

fn restore_groups(env: &ApplyEnv, gsettings: &dyn FontGsettings, only: Option<&[String]>) -> Result<Report, String> {
    let mut record = load(&env.record).unwrap_or_default();
    if record.before.is_empty() {
        let _ = std::fs::remove_file(&env.record);
        return Ok(Report::default());
    }
    match restore_groups_inner(env, gsettings, &mut record, only) {
        Ok(report) => {
            persist_record(env, &record)?;
            Ok(report)
        }
        Err(err) => {
            // Keep remaining ownership so a retry can finish the restore.
            let _ = persist_record(env, &record);
            Err(err)
        }
    }
}

fn restore_groups_inner(env: &ApplyEnv, gsettings: &dyn FontGsettings, record: &mut Record, only: Option<&[String]>) -> Result<Report, String> {
    let mut report = Report::default();
    let applied = record.families.clone();
    let mut pending = std::mem::take(&mut record.before);
    let mut keep = Vec::new();
    let idx = 0;
    while idx < pending.len() {
        let before = &pending[idx];
        if only.is_some_and(|groups| !groups.iter().any(|g| g == &before.group)) {
            keep.push(pending.remove(idx));
            continue;
        }
        if before.target.starts_with("gsettings:") {
            let key = before.target.trim_start_matches("gsettings:").to_string();
            let target = before.target.clone();
            let previous = before.value.clone();
            let Some(now) = gsettings.get(&key) else {
                pending.remove(idx);
                continue;
            };
            let mandara = applied.as_ref().map(|f| match key.as_str() {
                "font-name" => f.gtk_ui(),
                "document-font-name" => f.gtk_document(),
                "monospace-font-name" => f.gtk_mono(),
                _ => String::new(),
            });
            if mandara.as_ref() == Some(&now) {
                match previous {
                    Some(ref old) if gsettings.set(&key, old) => {
                        report.changed.push(target);
                        pending.remove(idx);
                    }
                    Some(_) => {
                        keep.push(pending.remove(idx));
                        keep.extend(pending.drain(idx..));
                        record.before = keep;
                        return Err(format!("gsettings set {GSETTINGS_SCHEMA} {key} failed during restore"));
                    }
                    None => {
                        report.changed.push(target);
                        pending.remove(idx);
                    }
                }
            } else {
                report.kept.push(target);
                keep.push(pending.remove(idx));
            }
            continue;
        }
        if before.target == FONTCONFIG_REL {
            let path = env.config.join(FONTCONFIG_REL);
            let Ok(now) = std::fs::read_to_string(&path) else {
                pending.remove(idx);
                continue;
            };
            let ours = applied.as_ref().map(fontconfig_body);
            if ours.as_deref() != Some(now.as_str()) {
                report.kept.push(before.target.clone());
                keep.push(pending.remove(idx));
                continue;
            }
            let created = before.created;
            let old = before.value.clone();
            if let Err(err) = (|| -> Result<(), String> {
                if created {
                    remove_file_ok(&path)?;
                    if let Some(dir) = path.parent() {
                        let _ = std::fs::remove_dir(dir);
                    }
                } else if let Some(old) = &old {
                    write(&path, old)?;
                } else {
                    remove_file_ok(&path)?;
                }
                Ok(())
            })() {
                keep.push(pending.remove(idx));
                keep.extend(pending.drain(idx..));
                record.before = keep;
                return Err(err);
            }
            report.changed.push(before.target.clone());
            pending.remove(idx);
            continue;
        }
        let Some((rel, section, key)) = ini_meta(&before.target) else {
            keep.push(pending.remove(idx));
            continue;
        };
        let path = env.config.join(rel);
        let Ok(text) = std::fs::read_to_string(&path) else {
            pending.remove(idx);
            continue;
        };
        let now = ini_get(&text, section, key);
        let expected = applied.as_ref().and_then(|f| expected_ini(rel, key, f));
        if now != expected {
            report.kept.push(before.target.clone());
            keep.push(pending.remove(idx));
            continue;
        }
        let next = ini_set(&text, section, key, before.value.as_deref());
        let created = before.created;
        if let Err(err) = (|| -> Result<(), String> {
            if created && only_headers(&next) {
                remove_file_ok(&path)?;
                if let Some(dir) = path.parent().filter(|d| *d != env.config) {
                    let _ = std::fs::remove_dir(dir);
                }
            } else {
                write(&path, &next)?;
            }
            Ok(())
        })() {
            keep.push(pending.remove(idx));
            keep.extend(pending.drain(idx..));
            record.before = keep;
            return Err(err);
        }
        if !report.changed.iter().any(|c| c == rel) {
            report.changed.push(rel.into());
        }
        pending.remove(idx);
    }
    record.before = keep;
    if record.before.is_empty() {
        record.families = None;
    }
    Ok(report)
}

fn ini_meta(target: &str) -> Option<(&str, &'static str, &str)> {
    let (file, key) = target.split_once("::")?;
    let section = match (file, key) {
        ("gtk-3.0/settings.ini" | "gtk-4.0/settings.ini", "gtk-font-name") => "Settings",
        ("qt5ct/qt5ct.conf" | "qt6ct/qt6ct.conf", "general" | "fixed") => "Fonts",
        ("kdeglobals", "font" | "menuFont" | "toolBarFont" | "smallestReadableFont" | "fixed") => "General",
        _ => return None,
    };
    Some((file, section, key))
}

fn expected_ini(file: &str, key: &str, f: &Families) -> Option<String> {
    match (file, key) {
        ("gtk-3.0/settings.ini" | "gtk-4.0/settings.ini", "gtk-font-name") => Some(f.gtk_ui()),
        ("qt5ct/qt5ct.conf" | "qt6ct/qt6ct.conf", "general") => Some(f.qt_font(&f.ui)),
        ("qt5ct/qt5ct.conf" | "qt6ct/qt6ct.conf", "fixed") => Some(f.qt_font(&f.mono)),
        ("kdeglobals", "fixed") => Some(f.kde_font(&f.mono)),
        ("kdeglobals", "font" | "menuFont" | "toolBarFont" | "smallestReadableFont") => Some(f.kde_font(&f.ui)),
        _ => None,
    }
}

/// Reconcile config → desktop: apply when enabled, restore when not.
pub fn reconcile(cfg: &mandara_config::ShellConfig, env: &ApplyEnv, gsettings: &dyn FontGsettings) -> Result<Report, String> {
    let sync = &cfg.typography.sync;
    if !sync.enabled {
        return restore(env, gsettings);
    }
    let targets = Targets {
        fontconfig: sync.fontconfig,
        gtk: sync.gtk,
        qt: sync.qt,
        kde: sync.kde,
        browsers: sync.browsers,
    };
    apply(&Families::from_typography(&cfg.typography), targets, env, gsettings)
}

/// Whether Mandara currently owns a system-font record.
pub fn active(env: &ApplyEnv) -> bool {
    load(&env.record).is_some_and(|r| !r.before.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeGs(RefCell<std::collections::BTreeMap<String, String>>);

    impl FontGsettings for FakeGs {
        fn get(&self, key: &str) -> Option<String> {
            self.0.borrow().get(key).cloned()
        }
        fn set(&self, key: &str, value: &str) -> bool {
            self.0.borrow_mut().insert(key.into(), value.into());
            true
        }
    }

    fn families() -> Families {
        Families { ui: "Inter".into(), display: "Alexandria".into(), mono: "CaskaydiaCove Nerd Font Mono".into() }
    }

    #[test]
    fn apply_and_restore_round_trip() {
        let root = tempfile::tempdir().unwrap();
        let env = ApplyEnv::under(root.path());
        let cfg = &env.config;
        std::fs::create_dir_all(cfg.join("gtk-3.0")).unwrap();
        std::fs::create_dir_all(cfg.join("qt6ct")).unwrap();
        std::fs::write(cfg.join("gtk-3.0/settings.ini"), "[Settings]\ngtk-font-name=Old 10\n").unwrap();
        std::fs::write(cfg.join("qt6ct/qt6ct.conf"), "[Fonts]\ngeneral=Old,10,-1,5,50,0,0,0,0,0\n").unwrap();
        let gs = FakeGs::default();
        gs.0.borrow_mut().insert("font-name".into(), "Old 10".into());
        gs.0.borrow_mut().insert("document-font-name".into(), "OldDoc 10".into());
        gs.0.borrow_mut().insert("monospace-font-name".into(), "OldMono 10".into());

        let targets = Targets { fontconfig: true, gtk: true, qt: true, kde: true, browsers: true };
        let report = apply(&families(), targets, &env, &gs).unwrap();
        assert!(report.changed.iter().any(|c| c.contains("fontconfig")));
        assert!(cfg.join(FONTCONFIG_REL).exists());
        assert!(std::fs::read_to_string(cfg.join("gtk-3.0/settings.ini")).unwrap().contains("Inter 11"));
        assert_eq!(gs.get("font-name").as_deref(), Some("Inter 11"));
        assert!(active(&env));

        let back = restore(&env, &gs).unwrap();
        assert!(back.changed.iter().any(|c| c.contains("gtk-3.0")));
        assert_eq!(std::fs::read_to_string(cfg.join("gtk-3.0/settings.ini")).unwrap(), "[Settings]\ngtk-font-name=Old 10\n");
        assert_eq!(gs.get("font-name").as_deref(), Some("Old 10"));
        assert!(!cfg.join(FONTCONFIG_REL).exists());
        assert!(!active(&env));
    }

    #[test]
    fn master_off_via_reconcile_restores() {
        let root = tempfile::tempdir().unwrap();
        let env = ApplyEnv::under(root.path());
        let gs = FakeGs::default();
        let mut cfg = mandara_config::ShellConfig::default();
        cfg.typography.sync.enabled = true;
        reconcile(&cfg, &env, &gs).unwrap();
        assert!(env.config.join(FONTCONFIG_REL).exists());
        cfg.typography.sync.enabled = false;
        reconcile(&cfg, &env, &gs).unwrap();
        assert!(!env.config.join(FONTCONFIG_REL).exists());
    }

    #[test]
    fn turning_off_one_target_restores_only_that_group() {
        let root = tempfile::tempdir().unwrap();
        let env = ApplyEnv::under(root.path());
        let gs = FakeGs::default();
        let all = Targets { fontconfig: true, gtk: true, qt: false, kde: false, browsers: false };
        apply(&families(), all, &env, &gs).unwrap();
        assert!(env.config.join(FONTCONFIG_REL).exists());
        assert!(env.config.join("gtk-3.0/settings.ini").exists());
        let no_fc = Targets { fontconfig: false, gtk: true, qt: false, kde: false, browsers: false };
        apply(&families(), no_fc, &env, &gs).unwrap();
        assert!(!env.config.join(FONTCONFIG_REL).exists(), "fontconfig taken back");
        assert!(env.config.join("gtk-3.0/settings.ini").exists(), "gtk stays");
    }

    /// gsettings that refuse every set — used to prove ownership is retained.
    struct FailSetGs(FakeGs);

    impl FontGsettings for FailSetGs {
        fn get(&self, key: &str) -> Option<String> {
            self.0.get(key)
        }
        fn set(&self, _key: &str, _value: &str) -> bool {
            false
        }
    }

    #[test]
    fn apply_checkpoints_before_later_target_fails() {
        let root = tempfile::tempdir().unwrap();
        let env = ApplyEnv::under(root.path());
        let gs = FailSetGs(FakeGs::default());
        gs.0.0.borrow_mut().insert("font-name".into(), "Old 10".into());
        let targets = Targets { fontconfig: true, gtk: true, qt: false, kde: false, browsers: false };
        assert!(apply(&families(), targets, &env, &gs).is_err());
        assert!(env.config.join(FONTCONFIG_REL).exists(), "fontconfig already written");
        assert!(active(&env), "ownership persisted for restore");
        let record = load(&env.record).expect("record");
        assert!(record.before.iter().any(|b| b.target == FONTCONFIG_REL));
        assert!(record.before.iter().any(|b| b.target == "gsettings:font-name"));
    }

    #[test]
    fn gsettings_restore_failure_keeps_ownership() {
        let root = tempfile::tempdir().unwrap();
        let env = ApplyEnv::under(root.path());
        let gs = FakeGs::default();
        gs.0.borrow_mut().insert("font-name".into(), "Old 10".into());
        let targets = Targets { fontconfig: false, gtk: true, qt: false, kde: false, browsers: false };
        apply(&families(), targets, &env, &gs).unwrap();
        assert_eq!(gs.get("font-name").as_deref(), Some("Inter 11"));
        let failing = FailSetGs(FakeGs(RefCell::new(gs.0.borrow().clone())));
        assert!(restore(&env, &failing).is_err());
        assert!(active(&env), "failed restore must keep the record");
        let record = load(&env.record).expect("record");
        assert!(record.before.iter().any(|b| b.target == "gsettings:font-name"));
    }
}
