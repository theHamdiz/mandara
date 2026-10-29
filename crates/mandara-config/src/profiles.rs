//! Named settings profiles, export / import and redaction.
//!
//! A profile is a TOML file in `~/.config/mandara/profiles/` holding a
//! snapshot of some config paths. A **look** profile carries only the
//! appearance (theme, bar, motion, typography, wallpaper, sidebar, window
//! look) so switching "Work" ↔ "Night" never touches plugins, keybinds or
//! accounts; a **full** profile carries everything. Applying a profile
//! replaces exactly the paths it lists — a path the profile lists but whose
//! value is unset (an optional value) is unset again.
//!
//! Exports are a whole `config.toml` (optionally only the values that
//! differ from the defaults), with a privacy note for sensitive keys.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use toml::{Table, Value};

use crate::ShellConfig;

/// What a profile snapshots.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileScope {
    /// Appearance only.
    #[default]
    Look,
    /// The whole config.
    Full,
}

/// Config paths of a [`ProfileScope::Look`] profile (dotted).
pub const LOOK_PATHS: &[&str] = &["theme", "appearance", "bar", "motion", "typography", "wallpaper", "sidebar", "compositor.look", "compositor.border_width"];

impl ProfileScope {
    pub fn paths(self) -> Vec<String> {
        match self {
            ProfileScope::Look => LOOK_PATHS.iter().map(|p| p.to_string()).collect(),
            ProfileScope::Full => Vec::new(),
        }
    }
}

/// A saved profile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub scope: ProfileScope,
    /// `YYYY-MM-DD HH:MM`, local time.
    #[serde(default)]
    pub saved: String,
    /// Dotted paths this profile replaces (empty for a full profile).
    #[serde(default)]
    pub paths: Vec<String>,
    /// The values (a config subset, or the whole config).
    #[serde(default)]
    pub config: Table,
}

pub fn profiles_dir() -> PathBuf {
    crate::paths::config_dir().join("profiles")
}

/// A file-name-safe slug (`Night mode` → `night-mode`).
pub fn slug(name: &str) -> String {
    let s: String = name.trim().chars().map(|c| if c.is_alphanumeric() { c.to_lowercase().next().unwrap_or(c) } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "profile".into() } else { s }
}

fn to_table(cfg: &ShellConfig) -> Result<Table> {
    Ok(Table::try_from(cfg).context("serializing the config")?)
}

fn get_path<'a>(t: &'a Table, path: &str) -> Option<&'a Value> {
    let mut parts = path.split('.');
    let mut cur = t.get(parts.next()?)?;
    for p in parts {
        cur = cur.as_table()?.get(p)?;
    }
    Some(cur)
}

fn set_path(t: &mut Table, path: &str, value: Option<Value>) {
    let parts: Vec<&str> = path.split('.').collect();
    let mut cur = t;
    for p in &parts[..parts.len() - 1] {
        let next = cur.entry(p.to_string()).or_insert_with(|| Value::Table(Table::new()));
        let Some(table) = next.as_table_mut() else { return };
        cur = table;
    }
    let last = parts[parts.len() - 1];
    match value {
        Some(v) => {
            cur.insert(last.into(), v);
        }
        None => {
            cur.remove(last);
        }
    }
}

/// Snapshots `cfg` as a profile.
pub fn snapshot(cfg: &ShellConfig, name: &str, description: &str, scope: ProfileScope, saved: &str) -> Result<Profile> {
    let full = to_table(cfg)?;
    let paths = scope.paths();
    let config = if paths.is_empty() {
        full
    } else {
        let mut sub = Table::new();
        for p in &paths {
            set_path(&mut sub, p, get_path(&full, p).cloned());
        }
        sub
    };
    Ok(Profile { name: name.trim().to_string(), description: description.trim().to_string(), scope, saved: saved.into(), paths, config })
}

/// Snapshots exactly `paths` of `cfg` (dotted, e.g. `dock.media_chip`) as a
/// look profile: applying it later restores those values and nothing else.
pub fn snapshot_paths(cfg: &ShellConfig, name: &str, description: &str, paths: &[&str], saved: &str) -> Result<Profile> {
    let full = to_table(cfg)?;
    let mut sub = Table::new();
    for p in paths {
        set_path(&mut sub, p, get_path(&full, p).cloned());
    }
    Ok(Profile {
        name: name.trim().to_string(),
        description: description.trim().to_string(),
        scope: ProfileScope::Look,
        saved: saved.into(),
        paths: paths.iter().map(|p| p.to_string()).collect(),
        config: sub,
    })
}

/// `cfg` with `profile` applied.
pub fn apply(cfg: &ShellConfig, profile: &Profile) -> Result<ShellConfig> {
    if profile.paths.is_empty() {
        return Value::Table(profile.config.clone()).try_into().context("the profile is not a valid config");
    }
    let mut table = to_table(cfg)?;
    for p in &profile.paths {
        set_path(&mut table, p, get_path(&profile.config, p).cloned());
    }
    Value::Table(table).try_into().context("the profile doesn't fit this version's config")
}

pub fn save_in(dir: &Path, profile: &Profile) -> Result<PathBuf> {
    if profile.name.trim().is_empty() {
        bail!("a profile needs a name");
    }
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.toml", slug(&profile.name)));
    let text = toml::to_string_pretty(profile).context("serializing the profile")?;
    crate::store::write_atomic(&path, text.as_bytes())?;
    Ok(path)
}

pub fn list_in(dir: &Path) -> Vec<Profile> {
    let mut out: Vec<Profile> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| toml::from_str::<Profile>(&std::fs::read_to_string(e.path()).ok()?).ok())
        .collect();
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

/// The profile named `name` (case-insensitive, or its slug).
pub fn find_in(dir: &Path, name: &str) -> Option<Profile> {
    let want = slug(name);
    list_in(dir).into_iter().find(|p| p.name.eq_ignore_ascii_case(name.trim()) || slug(&p.name) == want)
}

pub fn delete_in(dir: &Path, name: &str) -> Result<()> {
    let p = find_in(dir, name).with_context(|| format!("no profile named {name:?}"))?;
    std::fs::remove_file(dir.join(format!("{}.toml", slug(&p.name))))?;
    Ok(())
}

pub fn save(profile: &Profile) -> Result<PathBuf> {
    save_in(&profiles_dir(), profile)
}

pub fn list() -> Vec<Profile> {
    list_in(&profiles_dir())
}

pub fn find(name: &str) -> Option<Profile> {
    find_in(&profiles_dir(), name)
}

pub fn delete(name: &str) -> Result<()> {
    delete_in(&profiles_dir(), name)
}

/// Studio → Maintenance's backup folder, `~/.local/share/mandara/backups`.
pub fn backups_dir() -> PathBuf {
    crate::paths::data_dir().join("backups")
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

/// Copies `~/.config/mandara` to `<backups>/<stamp>-<reason>` (the layout
/// Studio → Maintenance lists and restores).
pub fn backup_config_dir(stamp: &str, reason: &str) -> Result<PathBuf> {
    let dest = backups_dir().join(format!("{stamp}-{reason}"));
    let src = crate::paths::config_dir();
    if src.exists() {
        copy_dir(&src, &dest).with_context(|| format!("backing up to {}", dest.display()))?;
    } else {
        std::fs::create_dir_all(&dest)?;
    }
    Ok(dest)
}

/// `cfg` with one top-level section (`bar`, `compositor`…) back at its
/// defaults.
pub fn reset_section(cfg: &ShellConfig, section: &str) -> Result<ShellConfig> {
    let mut table = to_table(cfg)?;
    let defaults = to_table(&ShellConfig::default())?;
    if !defaults.contains_key(section) && !table.contains_key(section) {
        bail!("no config section {section:?}");
    }
    match defaults.get(section) {
        Some(v) => {
            table.insert(section.to_string(), v.clone());
        }
        None => {
            table.remove(section);
        }
    }
    Value::Table(table).try_into().context("resetting the section")
}

/// Top-level section names of the config.
pub fn sections() -> Vec<String> {
    to_table(&ShellConfig::default()).map(|t| t.keys().cloned().collect()).unwrap_or_default()
}

// ── Export / import ─────────────────────────────────────────────────────────

/// `a` minus every value equal to `b`'s (recursively through tables).
fn diff(a: &Table, b: &Table) -> Table {
    let mut out = Table::new();
    for (k, v) in a {
        match (v, b.get(k)) {
            (Value::Table(x), Some(Value::Table(y))) => {
                let d = diff(x, y);
                if !d.is_empty() {
                    out.insert(k.clone(), Value::Table(d));
                }
            }
            (v, Some(w)) if v == w => {}
            (v, _) => {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

/// Key names whose values may be personal (locations, keys, accounts).
const SENSITIVE_KEYS: &[&str] = &["latitude", "longitude", "city", "location", "api_key", "apikey", "token", "password", "secret", "key", "email", "username", "address"];

fn is_sensitive(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    SENSITIVE_KEYS.iter().any(|s| k == *s || k.ends_with(&format!("_{s}")))
}

/// Dotted paths of sensitive values in `t`.
pub fn sensitive_paths(t: &Table) -> Vec<String> {
    fn walk(t: &Table, prefix: &str, out: &mut Vec<String>) {
        for (k, v) in t {
            let path = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
            match v {
                Value::Table(sub) => walk(sub, &path, out),
                _ if is_sensitive(k) => out.push(path),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(t, "", &mut out);
    out
}

/// Replaces sensitive values with `"<redacted>"`.
pub fn redact(t: &mut Table) {
    for (k, v) in t.iter_mut() {
        match v {
            Value::Table(sub) => redact(sub),
            _ if is_sensitive(k) => *v = Value::String("<redacted>".into()),
            _ => {}
        }
    }
}

/// Export options.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportOptions {
    /// Only values that differ from the defaults (a short, portable file).
    pub changes_only: bool,
    /// Replace personal values (for sharing / bug reports).
    pub redact: bool,
}

/// The export text and the sensitive paths it contains (unredacted).
pub fn export_text(cfg: &ShellConfig, opts: ExportOptions) -> Result<(String, Vec<String>)> {
    let mut table = to_table(cfg)?;
    if opts.changes_only {
        table = diff(&table, &to_table(&ShellConfig::default())?);
    }
    let sensitive = sensitive_paths(&table);
    if opts.redact {
        redact(&mut table);
    }
    let body = toml::to_string_pretty(&table).context("serializing the config")?;
    let kind = if opts.changes_only { "values changed from the defaults" } else { "the full configuration" };
    let header = format!("# Mandara Shell settings export ({kind}).\n# Import with `mandara msg config import <file>` or Studio → Profiles & backup.\n");
    Ok((format!("{header}\n{body}"), if opts.redact { Vec::new() } else { sensitive }))
}

/// Parses an exported (or hand-written) config. Missing values take their
/// defaults, so a changes-only export imports as "defaults + changes".
pub fn import_text(text: &str) -> Result<ShellConfig> {
    let table: Table = toml::from_str(text).context("not a TOML file")?;
    if contains_redacted(&table) {
        bail!("this export was redacted (personal values replaced); import an unredacted export instead");
    }
    Value::Table(table).try_into().context("the file is not a Mandara config")
}

/// Parses a damaged `config.toml`, keeping every value that still fits:
/// whole sections first, then a broken section key by key. Returns the
/// config and the dotted paths that were dropped (defaults used instead).
/// Text that isn't TOML at all salvages nothing.
pub fn salvage(text: &str) -> (ShellConfig, Vec<String>) {
    if let Ok(cfg) = toml::from_str::<ShellConfig>(text) {
        return (cfg, Vec::new());
    }
    let Ok(found) = toml::from_str::<Table>(text) else { return (ShellConfig::default(), vec![String::from("<file>")]) };
    let fits = |t: &Table| Value::Table(t.clone()).try_into::<ShellConfig>().is_ok();
    let mut out = to_table(&ShellConfig::default()).unwrap_or_default();
    let mut dropped = Vec::new();
    for (section, value) in found {
        let mut trial = out.clone();
        trial.insert(section.clone(), value.clone());
        if fits(&trial) {
            out = trial;
            continue;
        }
        // Keep the section's good keys on top of its defaults.
        let Value::Table(keys) = value else {
            dropped.push(section);
            continue;
        };
        for (key, v) in keys {
            let mut trial = out.clone();
            let entry = trial.entry(section.clone()).or_insert_with(|| Value::Table(Table::new()));
            match entry.as_table_mut() {
                Some(t) => {
                    t.insert(key.clone(), v);
                }
                None => *entry = Value::Table(Table::from_iter([(key.clone(), v)])),
            }
            if fits(&trial) {
                out = trial;
            } else {
                dropped.push(format!("{section}.{key}"));
            }
        }
    }
    let cfg = Value::Table(out).try_into().unwrap_or_default();
    (cfg, dropped)
}

fn contains_redacted(t: &Table) -> bool {
    t.values().any(|v| match v {
        Value::Table(sub) => contains_redacted(sub),
        Value::String(s) => s == "<redacted>",
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tweaked() -> ShellConfig {
        let mut c = ShellConfig::default();
        c.bar.height = 28.0;
        c.compositor.look.gaps_inner = Some(4);
        c.general.clock_24h = false;
        c.plugins.settings.entry("prayer-times".into()).or_default().insert("latitude".into(), Value::Float(31.2));
        c
    }

    #[test]
    fn look_profile_round_trip_and_scope() {
        let dir = tempfile::tempdir().unwrap();
        let p = snapshot(&tweaked(), "Night mode", "dim", ProfileScope::Look, "2026-10-27 18:00").unwrap();
        save_in(dir.path(), &p).unwrap();
        assert!(dir.path().join("night-mode.toml").is_file());
        let loaded = find_in(dir.path(), "night MODE").unwrap();
        assert_eq!(loaded, p);
        // Applying to a default config takes the look but not general/plugins.
        let applied = apply(&ShellConfig::default(), &loaded).unwrap();
        assert_eq!(applied.bar.height, 28.0);
        assert_eq!(applied.compositor.look.gaps_inner, Some(4));
        assert!(applied.general.clock_24h, "general isn't part of a look profile");
        assert!(applied.plugins.settings.is_empty());
        // An unset optional in the profile is unset again on apply.
        let plain = snapshot(&ShellConfig::default(), "Plain", "", ProfileScope::Look, "").unwrap();
        assert_eq!(apply(&tweaked(), &plain).unwrap().compositor.look.gaps_inner, None);
        assert_eq!(list_in(dir.path()).len(), 1);
        delete_in(dir.path(), "night-mode").unwrap();
        assert!(list_in(dir.path()).is_empty());
    }

    #[test]
    fn full_profile_replaces_everything() {
        let p = snapshot(&tweaked(), "All", "", ProfileScope::Full, "").unwrap();
        assert_eq!(apply(&ShellConfig::default(), &p).unwrap(), tweaked());
    }

    #[test]
    fn changes_only_export_imports_back() {
        let (text, sensitive) = export_text(&tweaked(), ExportOptions { changes_only: true, redact: false }).unwrap();
        assert!(text.contains("height = 28.0") && !text.contains("[launcher]"), "{text}");
        assert_eq!(sensitive, vec!["plugins.settings.prayer-times.latitude"]);
        assert_eq!(import_text(&text).unwrap(), tweaked());
        let (full, _) = export_text(&tweaked(), ExportOptions::default()).unwrap();
        assert_eq!(import_text(&full).unwrap(), tweaked());
    }

    #[test]
    fn redacted_exports_hide_and_refuse_import() {
        let (text, sensitive) = export_text(&tweaked(), ExportOptions { changes_only: true, redact: true }).unwrap();
        assert!(sensitive.is_empty());
        assert!(text.contains("latitude = \"<redacted>\"") && !text.contains("31.2"));
        assert!(import_text(&text).is_err());
        assert!(import_text("not = [toml").is_err());
    }

    #[test]
    fn section_reset() {
        let reset = reset_section(&tweaked(), "bar").unwrap();
        assert_eq!(reset.bar, ShellConfig::default().bar);
        assert!(!reset.general.clock_24h, "other sections stay");
        assert!(reset_section(&tweaked(), "nope").is_err());
        assert!(sections().contains(&"compositor".to_string()));
    }

    #[test]
    fn salvage_keeps_what_still_parses() {
        let text = "[bar]\nheight = 30.0\nlayout = \"sideways\"\n[general]\nclock_24h = false\n[motion]\nstiffness = \"stiff\"\ndamping = 30.0\n";
        assert!(toml::from_str::<ShellConfig>(text).is_err(), "fixture must be broken");
        let (cfg, dropped) = salvage(text);
        assert_eq!(cfg.bar.height, 30.0, "good keys of a broken section survive");
        assert_eq!(cfg.bar.layout, ShellConfig::default().bar.layout);
        assert!(!cfg.general.clock_24h, "intact sections survive whole");
        assert_eq!(cfg.motion.damping, 30.0);
        assert_eq!(dropped, vec!["bar.layout".to_string(), "motion.stiffness".to_string()]);
        // A valid file comes back untouched; garbage gives defaults.
        let good = toml::to_string(&tweaked()).unwrap();
        assert_eq!(salvage(&good), (tweaked(), Vec::new()));
        assert_eq!(salvage("not = [toml").0, ShellConfig::default());
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("  Work / Day  "), "work-day");
        assert_eq!(slug("ليل"), "ليل");
        assert_eq!(slug("!!"), "profile");
    }
}
