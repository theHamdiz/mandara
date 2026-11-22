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
    /// The bar and its side bars, without the theme or desktop widgets.
    Bar,
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
            ProfileScope::Bar => ["bar", "side_bars"].map(str::to_string).to_vec(),
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

pub(crate) fn to_table(cfg: &ShellConfig) -> Result<Table> {
    Table::try_from(cfg).context("serializing the config")
}

pub(crate) fn get_path<'a>(t: &'a Table, path: &str) -> Option<&'a Value> {
    let mut parts = path.split('.');
    let mut cur = t.get(parts.next()?)?;
    for p in parts {
        cur = cur.as_table()?.get(p)?;
    }
    Some(cur)
}

/// [`get_path`] for other crates (a value at a dotted path).
pub fn get_path_pub<'a>(t: &'a Table, path: &str) -> Option<&'a Value> {
    get_path(t, path)
}

pub(crate) fn set_path(t: &mut Table, path: &str, value: Option<Value>) {
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

/// What a plugin may do is decided by the person at the grant prompt, never
/// by a file: `next` takes `current`'s grants and its enabled / disabled
/// lists, whatever a profile or an imported settings file said about them.
/// (`ConfigStore::update` accepts any grant of a plugin that had none.)
pub fn keep_plugin_authority(next: &mut ShellConfig, current: &ShellConfig) {
    next.plugins.grants = current.plugins.grants.clone();
    next.plugins.enabled = current.plugins.enabled.clone();
    next.plugins.disabled = current.plugins.disabled.clone();
}

/// `cfg` with `profile` applied. Plugin permissions and which plugins are
/// switched on stay as they are in `cfg`.
pub fn apply(cfg: &ShellConfig, profile: &Profile) -> Result<ShellConfig> {
    let mut next: ShellConfig = if profile.paths.is_empty() {
        Value::Table(profile.config.clone()).try_into().context("the profile is not a valid config")?
    } else {
        let mut table = to_table(cfg)?;
        for p in &profile.paths {
            set_path(&mut table, p, get_path(&profile.config, p).cloned());
        }
        Value::Table(table).try_into().context("the profile doesn't fit this version's config")?
    };
    keep_plugin_authority(&mut next, cfg);
    Ok(next)
}

/// Largest profile file read (1 MiB; a full config is a few tens of KiB).
const PROFILE_MAX_BYTES: u64 = 1 << 20;

fn read_profile(path: &Path) -> Option<Profile> {
    toml::from_str::<Profile>(&crate::store::read_to_string_capped(path, PROFILE_MAX_BYTES).ok()?).ok()
}

/// Saves `profile` as `<slug>.toml`. A profile of the same name is replaced
/// in the file it came from; a different profile whose name happens to share
/// the slug (`Night mode` / `night-mode`) is never overwritten.
pub fn save_in(dir: &Path, profile: &Profile) -> Result<PathBuf> {
    let name = profile.name.trim();
    if name.is_empty() {
        bail!("a profile needs a name");
    }
    std::fs::create_dir_all(dir)?;
    let path = match entries_in(dir).into_iter().find(|(_, p)| p.name.trim().eq_ignore_ascii_case(name)) {
        Some((path, _)) => path,
        None => {
            let path = dir.join(format!("{}.toml", slug(name)));
            if let Some(other) = read_profile(&path) {
                bail!("{path:?} already holds the profile {:?}; pick another name", other.name);
            }
            path
        }
    };
    let text = toml::to_string_pretty(profile).context("serializing the profile")?;
    crate::store::write_atomic(&path, text.as_bytes())?;
    Ok(path)
}

/// Every readable profile with the file it was loaded from, by name.
fn entries_in(dir: &Path) -> Vec<(PathBuf, Profile)> {
    let mut out: Vec<(PathBuf, Profile)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .filter_map(|p| read_profile(&p).map(|profile| (p, profile)))
        .collect();
    out.sort_by_key(|(path, p)| (p.name.to_lowercase(), path.clone()));
    out
}

fn find_entry(dir: &Path, name: &str) -> Option<(PathBuf, Profile)> {
    let want = slug(name);
    let mut entries = entries_in(dir);
    // The exact name wins over a profile that merely shares its slug.
    let at = entries.iter().position(|(_, p)| p.name.trim().eq_ignore_ascii_case(name.trim())).or_else(|| entries.iter().position(|(_, p)| slug(&p.name) == want))?;
    Some(entries.swap_remove(at))
}

pub fn list_in(dir: &Path) -> Vec<Profile> {
    entries_in(dir).into_iter().map(|(_, p)| p).collect()
}

/// The profile named `name` (case-insensitive, or its slug).
pub fn find_in(dir: &Path, name: &str) -> Option<Profile> {
    find_entry(dir, name).map(|(_, p)| p)
}

/// Deletes the file the profile was loaded from, whatever it is called.
pub fn delete_in(dir: &Path, name: &str) -> Result<()> {
    let (path, _) = find_entry(dir, name).with_context(|| format!("no profile named {name:?}"))?;
    std::fs::remove_file(&path).with_context(|| format!("deleting {}", path.display()))
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

/// How many backup folders are kept; older ones are deleted when a new
/// backup is made.
pub const MAX_BACKUPS: usize = 20;

/// Copies a config folder into a backup. Symlinks (a dotfiles repo linking
/// whole folders or files) are followed so the backup holds their contents; a
/// link that leads nowhere is recreated as a link. `generated/` (rebuilt by
/// the shell on the next sync), `*.tmp` leftovers of interrupted atomic
/// writes, sockets and pipes are not copied.
pub fn copy_config_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut ancestors = vec![from.canonicalize()?];
    copy_tree(from, to, &mut ancestors, true)
}

fn copy_tree(from: &Path, to: &Path, ancestors: &mut Vec<PathBuf>, top: bool) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        if (top && name_text == "generated") || name_text.ends_with(".tmp") {
            continue;
        }
        let (path, dest) = (entry.path(), to.join(&name));
        let kind = entry.file_type()?;
        // `metadata` follows links; an error is a link to nothing.
        let resolved = if kind.is_symlink() { std::fs::metadata(&path) } else { entry.metadata() };
        match resolved {
            Ok(meta) if meta.is_dir() => {
                let real = path.canonicalize()?;
                // A link back to a folder being copied would never end.
                if ancestors.contains(&real) {
                    continue;
                }
                ancestors.push(real);
                let copied = copy_tree(&path, &dest, ancestors, false);
                ancestors.pop();
                copied?;
            }
            Ok(meta) if meta.is_file() => {
                std::fs::copy(&path, &dest)?;
            }
            Ok(_) => {}
            Err(_) if kind.is_symlink() => std::os::unix::fs::symlink(std::fs::read_link(&path)?, &dest)?,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Deletes all but the newest `keep` backup folders in `dir`. Only folders
/// named like Mandara's own (`YYYYMMDD-HHMMSS-<reason>`) are ever removed.
pub fn prune_backups(dir: &Path, keep: usize) {
    let is_backup = |name: &str| {
        let b = name.as_bytes();
        b.len() > 16 && b[..8].iter().all(u8::is_ascii_digit) && b[8] == b'-' && b[9..15].iter().all(u8::is_ascii_digit) && b[15] == b'-'
    };
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && is_backup(&e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .collect();
    // Stamps sort chronologically.
    found.sort();
    let excess = found.len().saturating_sub(keep);
    for old in &found[..excess] {
        if let Err(error) = std::fs::remove_dir_all(old) {
            tracing::warn!("removing old backup {}: {error}", old.display());
        }
    }
}

/// Copies `~/.config/mandara` to `<backups>/<stamp>-<reason>` (the layout
/// Studio → Maintenance lists and restores), then drops the oldest backups
/// beyond [`MAX_BACKUPS`].
pub fn backup_config_dir(stamp: &str, reason: &str) -> Result<PathBuf> {
    let root = backups_dir();
    let dest = root.join(format!("{stamp}-{reason}"));
    let src = crate::paths::config_dir();
    if src.exists() {
        copy_config_tree(&src, &dest).with_context(|| format!("backing up to {}", dest.display()))?;
    } else {
        std::fs::create_dir_all(&dest)?;
    }
    prune_backups(&root, MAX_BACKUPS);
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
        let full = table.clone();
        table = diff(&table, &to_table(&ShellConfig::default())?);
        // A table that made it into the export says which defaults it was
        // written for, or importing it would upgrade it as a 0.1 table.
        for (section, key) in VERSIONED {
            // Sections nest with dots (`desktop.widgets`).
            let mut out = Some(&mut table);
            let mut all = Some(&full);
            for part in section.split('.') {
                out = out.and_then(|t| t.get_mut(part)).and_then(Value::as_table_mut);
                all = all.and_then(|t| t.get(part)).and_then(Value::as_table);
            }
            if let (Some(out), Some(v)) = (out, all.and_then(|t| t.get(*key))) {
                out.insert(key.to_string(), v.clone());
            }
        }
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

/// Sections that carry the version of the defaults they were written for
/// (see [`crate::upgrade`]), with that key.
const VERSIONED: &[(&str, &str)] = &[("plugins", "defaults_version"), ("integrations", "version"), ("compositor", "keybinds_version"), ("lock", "look_version"), ("desktop.widgets", "version"), ("dock", "pins_version")];

/// Parses an exported (or hand-written) config. Missing values take their
/// defaults, so a changes-only export imports as "defaults + changes"; an
/// export from an older version is upgraded like a config file.
///
/// The result is the config to switch to from `current`. Plugin permissions
/// (`plugins.grants`) and which plugins are enabled or disabled are never
/// taken from the file: a shared export must not hand a plugin capabilities
/// the person didn't approve. They stay as in `current`; permissions are
/// given in Studio → Plugins as usual.
pub fn import_text(text: &str, current: &ShellConfig) -> Result<ShellConfig> {
    let table: Table = toml::from_str(text).context("not a TOML file")?;
    if contains_redacted(&table) {
        bail!("this export was redacted (personal values replaced); import an unredacted export instead");
    }
    let mut cfg: ShellConfig = Value::Table(table).try_into().context("the file is not a Mandara config")?;
    crate::upgrade::upgrade(&mut cfg);
    keep_plugin_authority(&mut cfg, current);
    Ok(cfg)
}

/// Largest settings file [`import_text`] callers read (a full export is a few
/// tens of KiB).
pub const IMPORT_MAX_BYTES: u64 = 4 << 20;

/// Parses a damaged `config.toml`, keeping every value that still fits:
/// whole sections first, then a broken section key by key. Returns the
/// config and the dotted paths that were dropped (defaults used instead).
/// Text that isn't TOML at all salvages nothing.
pub fn salvage(text: &str) -> (ShellConfig, Vec<String>) {
    if let Ok(cfg) = toml::from_str::<ShellConfig>(text) {
        return (cfg, Vec::new());
    }
    let Ok(found) = toml::from_str::<Table>(text) else { return (ShellConfig::default(), vec![String::from("<file>")]) };
    // Every section has its own defaults, so a trial holds just the section
    // under test: deserializing it costs that section, not the whole config.
    let fits = |section: &str, value: &Value| Value::Table(Table::from_iter([(section.to_string(), value.clone())])).try_into::<ShellConfig>().is_ok();
    let defaults = to_table(&ShellConfig::default()).unwrap_or_default();
    let mut out = defaults.clone();
    let mut dropped = Vec::new();
    for (section, value) in found {
        if fits(&section, &value) {
            out.insert(section, value);
            continue;
        }
        // Keep the section's good keys on top of its defaults.
        let Value::Table(keys) = value else {
            dropped.push(section);
            continue;
        };
        let mut kept = match defaults.get(&section) {
            Some(Value::Table(base)) => base.clone(),
            _ => Table::new(),
        };
        for (key, v) in keys {
            let mut trial = kept.clone();
            trial.insert(key.clone(), v);
            if fits(&section, &Value::Table(trial.clone())) {
                kept = trial;
            } else {
                dropped.push(format!("{section}.{key}"));
            }
        }
        out.insert(section, Value::Table(kept));
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
    fn bar_profile_round_trips_without_changing_theme_widgets_or_plugin_choices() {
        let dir = tempfile::tempdir().unwrap();
        let mut original = tweaked();
        original.bar.left = vec!["launcher".into(), "clock".into()];
        original.side_bars.enabled = true;
        let profile = snapshot(&original, "Desk bar", "", ProfileScope::Bar, "").unwrap();
        save_in(dir.path(), &profile).unwrap();
        let loaded = find_in(dir.path(), "Desk bar").unwrap();
        assert_eq!(loaded.scope, ProfileScope::Bar);
        assert_eq!(loaded.paths, ["bar", "side_bars"]);
        let mut current = ShellConfig::default();
        current.desktop.widgets.items.push(crate::desktop_widgets::DesktopWidget::default());
        current.plugins.disabled.push("weather".into());
        let applied = apply(&current, &loaded).unwrap();
        assert_eq!(applied.bar, original.bar);
        assert_eq!(applied.side_bars, original.side_bars);
        assert_eq!(applied.desktop, current.desktop);
        assert_eq!(applied.theme, current.theme);
        assert_eq!(applied.plugins, current.plugins);
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
        let current = ShellConfig::default();
        assert_eq!(import_text(&text, &current).unwrap(), tweaked());
        let (full, _) = export_text(&tweaked(), ExportOptions::default()).unwrap();
        assert_eq!(import_text(&full, &current).unwrap(), tweaked());
    }

    /// A shared settings file must not grant plugin permissions or switch
    /// plugins on: only the person's own grants and choices survive.
    #[test]
    fn imports_and_full_profiles_never_bring_plugin_permissions() {
        let mut shared = tweaked();
        shared.plugins.set_granted("evil", "wallpaper.live", true);
        shared.plugins.enabled.push("evil".into());
        shared.plugins.disabled.push("weather".into());
        let (text, _) = export_text(&shared, ExportOptions { changes_only: false, redact: false }).unwrap();
        assert!(text.contains("evil"), "fixture must carry the grant");

        let mut mine = ShellConfig::default();
        mine.plugins.set_granted("moon-phase", "media.read", true);
        mine.plugins.disabled.push("clock".into());
        let imported = import_text(&text, &mine).unwrap();
        assert_eq!(imported.plugins.grants, mine.plugins.grants);
        assert_eq!((&imported.plugins.enabled, &imported.plugins.disabled), (&mine.plugins.enabled, &mine.plugins.disabled));
        assert_eq!(imported.bar.height, 28.0, "everything else is imported");

        let full = snapshot(&shared, "Shared", "", ProfileScope::Full, "").unwrap();
        let applied = apply(&mine, &full).unwrap();
        assert_eq!(applied.plugins.grants, mine.plugins.grants);
        assert_eq!(applied.plugins.enabled, mine.plugins.enabled);
        assert_eq!(applied.bar.height, 28.0);

        // A hand-made profile that lists the grants as one of its paths gets nowhere either.
        let mut sneaky = snapshot_paths(&shared, "Sneaky", "", &["plugins.grants", "plugins.enabled", "bar"], "").unwrap();
        sneaky.scope = ProfileScope::Look;
        let applied = apply(&mine, &sneaky).unwrap();
        assert_eq!(applied.plugins.grants, mine.plugins.grants);
        assert!(applied.plugins.enabled.is_empty());
        assert_eq!(applied.bar.height, 28.0);
    }

    #[test]
    fn profiles_that_share_a_slug_never_overwrite_or_delete_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let first = snapshot(&tweaked(), "Night mode", "", ProfileScope::Look, "").unwrap();
        save_in(dir.path(), &first).unwrap();
        let clash = snapshot(&ShellConfig::default(), "night-mode", "", ProfileScope::Look, "").unwrap();
        assert!(save_in(dir.path(), &clash).is_err(), "same file name, different profile");
        assert_eq!(find_in(dir.path(), "Night mode").unwrap(), first);
        // Saving the same profile again replaces it, whatever the case.
        let again = snapshot(&tweaked(), "NIGHT MODE", "again", ProfileScope::Look, "").unwrap();
        save_in(dir.path(), &again).unwrap();
        assert_eq!(list_in(dir.path()), vec![again]);

        // A hand-made file whose name differs from its file name.
        let other = snapshot(&ShellConfig::default(), "c", "", ProfileScope::Look, "").unwrap();
        std::fs::write(dir.path().join("b.toml"), toml::to_string_pretty(&other).unwrap()).unwrap();
        let moved = snapshot(&ShellConfig::default(), "b", "", ProfileScope::Look, "").unwrap();
        std::fs::write(dir.path().join("a.toml"), toml::to_string_pretty(&moved).unwrap()).unwrap();
        delete_in(dir.path(), "b").unwrap();
        assert!(!dir.path().join("a.toml").exists(), "the file the profile came from goes");
        assert_eq!(find_in(dir.path(), "c").unwrap(), other, "b.toml (profile c) stays");
        // Saving "b" now would collide with b.toml, which holds "c".
        assert!(save_in(dir.path(), &moved).is_err());
    }

    #[test]
    fn backups_follow_symlinks_skip_generated_and_prune() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let (config, dotfiles) = (root.path().join("config"), root.path().join("dotfiles"));
        std::fs::create_dir_all(config.join("generated")).unwrap();
        std::fs::create_dir_all(dotfiles.join("looks")).unwrap();
        std::fs::write(config.join("config.toml"), "a").unwrap();
        std::fs::write(config.join("generated/niri.kdl"), "x").unwrap();
        std::fs::write(config.join(".config.toml.1.2.3.tmp"), "half").unwrap();
        std::fs::write(dotfiles.join("looks/mine.toml"), "look").unwrap();
        symlink(&dotfiles.join("looks"), config.join("looks")).unwrap();
        symlink(&config, dotfiles.join("looks/loop")).unwrap();
        symlink("/nonexistent/target", config.join("dangling")).unwrap();
        let backup = root.path().join("backups/20260101-000000-test");
        copy_config_tree(&config, &backup).unwrap();
        assert_eq!(std::fs::read_to_string(backup.join("config.toml")).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(backup.join("looks/mine.toml")).unwrap(), "look", "a linked folder is copied");
        assert!(!backup.join("looks/loop/config.toml").exists(), "a link back into the tree is not followed");
        assert!(!backup.join("generated").exists() && !backup.join(".config.toml.1.2.3.tmp").exists());
        assert!(std::fs::symlink_metadata(backup.join("dangling")).unwrap().file_type().is_symlink());

        let dir = root.path().join("backups");
        for n in 1..=5 {
            std::fs::create_dir_all(dir.join(format!("2026010{n}-000000-x"))).unwrap();
        }
        std::fs::create_dir_all(dir.join("mine")).unwrap();
        prune_backups(&dir, 3);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["20260103-000000-x", "20260104-000000-x", "20260105-000000-x", "mine"], "oldest go, other folders stay");
    }

    #[test]
    fn redacted_exports_hide_and_refuse_import() {
        let (text, sensitive) = export_text(&tweaked(), ExportOptions { changes_only: true, redact: true }).unwrap();
        assert!(sensitive.is_empty());
        assert!(text.contains("latitude = \"<redacted>\"") && !text.contains("31.2"));
        assert!(import_text(&text, &ShellConfig::default()).is_err());
        assert!(import_text("not = [toml", &ShellConfig::default()).is_err());
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
