//! Your own Looks: children of a built-in Look, saved in
//! `~/.config/mandara/looks/<id>.toml`, exported and imported as one
//! small portable TOML file.
//!
//! # The model: a delta over the parent
//!
//! A child Look records its built-in **parent** and only what you changed
//! on top of it ([`Delta`]): the Look-owned settings ([`projection`]) whose
//! values differ from the parent's, the ones you unset, and — when you
//! removed one of the parent's desktop widgets — the parent's widgets you
//! kept. Applying the child applies the parent, then the delta. When a
//! future release improves the parent, every setting you didn't change
//! follows it. A child is always one step from a built-in Look: a child of
//! one of your own Looks is recorded against that Look's parent, with the
//! whole difference.
//!
//! # The file
//!
//! ```toml
//! format = 1
//! id = "corniche-dusk-custom"
//! name = "Corniche Dusk · Custom"
//! author = ""
//! parent = "corniche-dusk"
//! created = "2026-11-11"
//! wallpaper_colors = false
//! unset = []
//!
//! [settings.bar]
//! position = "bottom"
//!
//! [settings.typography]
//! ui_font = "Cairo"
//! ```
//!
//! `widgets` (an array of desktop widgets) appears only when the Look's own
//! widgets differ from the parent's.
//!
//! # What a file may hold
//!
//! A Look carries no assets and nothing personal: no images (its palette is
//! a seed, a built-in theme, or "your wallpaper's colors" as a flag that
//! the importer's own wallpaper drives), no file paths, no commands, no
//! output names, no pinned apps, no plugin settings or credentials. On
//! export those are left out; on import a file holding any of them, an
//! unknown key, a value that doesn't fit, a key outside what a Look owns,
//! more than [`MAX_FILE_BYTES`], or a newer [`FORMAT`] is refused. Nothing
//! in a file is ever run. An imported Look gets an id of its own and, when
//! its name is taken, a numbered name.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use toml::{Table, Value};

use crate::desktop_widgets::{DesktopWidget, WidgetKind};
use crate::looks::{self, LookDef, LookOptions};
use crate::ShellConfig;

/// The file format this build writes and reads (newer files are refused).
pub const FORMAT: u32 = 1;
/// Largest Look file read or imported.
pub const MAX_FILE_BYTES: usize = 64 * 1024;
/// Longest name and author.
pub const MAX_NAME_CHARS: usize = 64;
/// Longest id (the file name).
pub const MAX_ID_CHARS: usize = 48;
/// Most of your Looks read from the folder.
pub const MAX_LOOKS: usize = 200;
/// Longest string value, and most entries in a list, in a Look's settings.
const MAX_VALUE_CHARS: usize = 256;
const MAX_LIST: usize = 64;
/// The name an exported file ends in.
pub const EXPORT_SUFFIX: &str = ".mandara-look.toml";

/// Everything a Look owns, as config paths ([`projection`]).
const OWNED: &[&str] = &[
    "theme.source",
    "theme.mode",
    "theme.adjustments",
    "bar",
    "side_bars",
    "typography",
    "style",
    "dock.magnification",
    "dock.position",
    "dock.alignment",
    "dock.media_chip",
    "motion.stiffness",
    "motion.damping",
    "motion.mass",
    "motion.speed",
    "launcher.look",
    "notifications.position",
    "hot_corners.enabled",
    "hot_corners.top_left.action",
    "hot_corners.top_right.action",
    "hot_corners.bottom_left.action",
    "hot_corners.bottom_right.action",
];
/// Bookkeeping inside the owned sections, never part of a Look.
const BOOKKEEPING: &[&str] = &["bar.style_items", "bar.layout_items", "bar.yours", "bar.preset_kept", "bar.monitors", "side_bars.monitors"];
/// Item lists: your own items are taken out of them.
const ZONES: &[&str] = &[
    "bar.left",
    "bar.center",
    "bar.right",
    "side_bars.left.top",
    "side_bars.left.middle",
    "side_bars.left.bottom",
    "side_bars.right.top",
    "side_bars.right.middle",
    "side_bars.right.bottom",
];
/// Keys never written to or read from a Look file, at any depth.
const FORBIDDEN_KEYS: &[&str] = &["command", "monitors", "displays", "path", "file", "exec", "url"];

/// What a child Look changes on top of its parent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Delta {
    /// Look-owned settings with your values (nested like the config).
    pub settings: Table,
    /// Look-owned settings you unset (dotted paths).
    pub unset: Vec<String>,
    /// The Look's own desktop widgets, when they differ from the parent's.
    pub widgets: Option<Vec<DesktopWidget>>,
}

impl Delta {
    pub fn is_empty(&self) -> bool {
        self.settings.is_empty() && self.unset.is_empty() && self.widgets.is_none()
    }

    /// Writes the delta's settings into `c`.
    pub(crate) fn merge_into(&self, c: &mut ShellConfig) {
        if self.settings.is_empty() && self.unset.is_empty() {
            return;
        }
        let Ok(mut t) = crate::profiles::to_table(c) else { return };
        let mut set = Vec::new();
        looks::leaves(&self.settings, "", &mut set);
        for (path, v) in set {
            crate::profiles::set_path(&mut t, &path, Some(v));
        }
        for path in &self.unset {
            crate::profiles::set_path(&mut t, path, None);
        }
        match Value::Table(t).try_into::<ShellConfig>() {
            Ok(next) => *c = next,
            Err(err) => tracing::warn!("looks: your look's settings don't fit this version: {err}"),
        }
    }
}

/// FNV-1a, 64 bit: a fingerprint stable across builds.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3))
}

/// The Look-owned part of `cfg`: [`OWNED`] without the bookkeeping, your
/// own bar items taken out of the item lists, and the light / dark mode
/// left out while something else (the clock, the desktop) decides it.
pub fn projection(cfg: &ShellConfig) -> Table {
    let Ok(full) = crate::profiles::to_table(cfg) else { return Table::new() };
    let mut out = Table::new();
    for p in OWNED {
        if let Some(v) = crate::profiles::get_path(&full, p) {
            crate::profiles::set_path(&mut out, p, Some(v.clone()));
        }
    }
    if cfg.theme.auto != crate::ThemeAuto::Manual {
        crate::profiles::set_path(&mut out, "theme.mode", None);
    }
    for b in BOOKKEEPING {
        crate::profiles::set_path(&mut out, b, None);
    }
    let yours: Vec<String> = crate::bar_items::yours(cfg).into_iter().map(|m| m.id).collect();
    for z in ZONES {
        if let Some(Value::Array(list)) = crate::profiles::get_path(&out, z) {
            let kept: Vec<Value> = list.iter().filter(|v| !v.as_str().is_some_and(|s| yours.iter().any(|y| y == s))).cloned().collect();
            crate::profiles::set_path(&mut out, z, Some(Value::Array(kept)));
        }
    }
    out
}

/// Fingerprint of what the active Look owns in `cfg`: its settings
/// ([`projection`]) and its widgets still as it placed them.
pub fn owned_print(cfg: &ShellConfig) -> String {
    let mut text = toml::to_string(&projection(cfg)).unwrap_or_default();
    for w in looks::look_widgets(cfg) {
        if cfg.looks.widgets.iter().any(|o| o.id == w.id && o.print == looks::widget_print(&w)) {
            text.push('|');
            text.push_str(&w.id);
        }
    }
    format!("{:016x}", fnv1a(text.as_bytes()))
}

/// `a`'s values that differ from `b`'s (recursing through tables), and the
/// paths `b` has that `a` lacks.
fn diff(a: &Table, b: &Table, prefix: &str, set: &mut Table, unset: &mut Vec<String>) {
    for (k, v) in a {
        let path = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
        match (v, b.get(k)) {
            (Value::Table(x), Some(Value::Table(y))) => {
                let mut sub = Table::new();
                diff(x, y, &path, &mut sub, unset);
                if !sub.is_empty() {
                    set.insert(k.clone(), Value::Table(sub));
                }
            }
            (v, Some(w)) if v == w => {}
            (v, _) => {
                set.insert(k.clone(), v.clone());
            }
        }
    }
    for k in b.keys().filter(|k| !a.contains_key(*k)) {
        unset.push(if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") });
    }
}

/// What `cfg` changes over `parent` applied to it: the Look-owned
/// settings, and the parent's widgets you kept when you removed some.
pub fn delta_over(parent: &LookDef, cfg: &ShellConfig) -> Delta {
    let base = parent.apply(cfg, LookOptions { wallpaper_colors: cfg.looks.wallpaper_colors });
    let (mut settings, mut unset) = (Table::new(), Vec::new());
    diff(&projection(cfg), &projection(&base), "", &mut settings, &mut unset);
    // A palette from the wallpaper is the flag, not the file.
    if matches!(cfg.theme.source, mandara_theme::ThemeSource::Wallpaper { .. }) {
        if let Some(Value::Table(theme)) = settings.get_mut("theme") {
            theme.remove("source");
        }
        unset.retain(|u| !u.starts_with("theme.source"));
    }
    if let Some(Value::Table(theme)) = settings.get("theme")
        && theme.is_empty()
    {
        settings.remove("theme");
    }
    // The Look's widgets: those it placed that you left as they were.
    let kept: Vec<DesktopWidget> = cfg.desktop.widgets.items.iter().filter(|w| cfg.looks.widgets.iter().any(|o| o.id == w.id && o.print == looks::widget_print(w))).cloned().collect();
    let all_there = cfg.looks.widgets.len() == kept.len() && cfg.looks.widgets_tracked;
    let widgets = if all_there { None } else { Some(kept.into_iter().map(portable_widget).collect()) };
    Delta { settings, unset, widgets }
}

/// One of your Looks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserLook {
    pub format: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub author: String,
    /// The built-in Look it derives from.
    pub parent: String,
    /// When it was made (`YYYY-MM-DD`).
    #[serde(default)]
    pub created: String,
    /// Its palette comes from the wallpaper (the parent's scheme variant).
    #[serde(default)]
    pub wallpaper_colors: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unset: Vec<String>,
    #[serde(default)]
    pub settings: Table,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widgets: Option<Vec<DesktopWidget>>,
}

/// A cheap entry for the gallery: no settings, no widgets.
#[derive(Clone, Debug, PartialEq)]
pub struct Descriptor {
    pub id: String,
    pub name: String,
    pub author: String,
    pub parent: &'static LookDef,
}

impl UserLook {
    pub fn parent(&self) -> Option<&'static LookDef> {
        looks::find(&self.parent)
    }

    pub fn delta(&self) -> Delta {
        Delta { settings: self.settings.clone(), unset: self.unset.clone(), widgets: self.widgets.clone() }
    }

    pub fn descriptor(&self) -> Option<Descriptor> {
        Some(Descriptor { id: self.id.clone(), name: self.name.clone(), author: self.author.clone(), parent: self.parent()? })
    }

    /// `cfg` with this Look applied: the parent, then your changes.
    pub fn apply(&self, cfg: &ShellConfig, opts: LookOptions) -> Result<ShellConfig> {
        let parent = self.parent().with_context(|| format!("the look {:?} is not in this version", self.parent))?;
        let opts = LookOptions { wallpaper_colors: opts.wallpaper_colors || self.wallpaper_colors };
        Ok(parent.apply_layered(cfg, opts, Some(&self.delta()), &self.id))
    }

    /// A new child of `parent` holding `delta`, named after it.
    pub fn child(parent: &'static LookDef, delta: Delta, taken: &[String], created: &str) -> Self {
        let mut name = format!("{} · Custom", parent.name);
        let mut n = 2;
        while taken.iter().any(|t| t.eq_ignore_ascii_case(&name)) {
            name = format!("{} · Custom {n}", parent.name);
            n += 1;
        }
        let mut look = UserLook { format: FORMAT, id: String::new(), name, author: String::new(), parent: parent.id.into(), created: created.into(), wallpaper_colors: false, unset: delta.unset, settings: delta.settings, widgets: delta.widgets };
        look.id = unique_id(&look.name, &[]);
        look
    }

    pub fn set_delta(&mut self, delta: Delta) {
        self.settings = delta.settings;
        self.unset = delta.unset;
        self.widgets = delta.widgets;
    }

    /// The file text (always the safe subset: [`sanitize`]).
    pub fn to_text(&self) -> Result<String> {
        let mut clean = self.clone();
        sanitize(&mut clean);
        Ok(format!("# A Mandara Look (Studio → Themes → Looks, `mandara msg look import`).\n{}", toml::to_string(&clean).context("writing the look")?))
    }
}

/// A file-name-safe id from `name`, unlike the built-in ids and `taken`.
pub fn unique_id(name: &str, taken: &[String]) -> String {
    let base: String = crate::profiles::slug(name).chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(MAX_ID_CHARS - 4).collect();
    let base = base.trim_matches('-').to_string();
    let base = if base.is_empty() { "look".to_string() } else { base };
    let free = |id: &str| looks::find(id).is_none() && !taken.iter().any(|t| t == id);
    if free(&base) {
        return base;
    }
    (2..).map(|n| format!("{base}-{n}")).find(|id| free(id)).unwrap_or(base)
}

/// Whether `id` is a safe file-name id (lowercase letters, digits, `-`).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID_CHARS && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !id.starts_with('-')
}

/// A widget as a Look may carry it: on every display.
fn portable_widget(mut w: DesktopWidget) -> DesktopWidget {
    w.place.displays.clear();
    w
}

/// Widget kinds a Look may carry (no images, no personal data).
fn portable_kind(kind: &WidgetKind) -> bool {
    match kind {
        WidgetKind::Clock(_) | WidgetKind::Monitor(_) | WidgetKind::Visualizer(_) | WidgetKind::Date(_) | WidgetKind::Progress(_) | WidgetKind::Decoration(_) => true,
        WidgetKind::Card(c) => [crate::desktop_cards::NOW_PLAYING, crate::desktop_cards::CALENDAR, crate::desktop_cards::WAVE, crate::desktop_cards::FRAMED_BARS, crate::desktop_cards::MIRRORED_BARS].contains(&c.card.as_str()),
        _ => false,
    }
}

fn drop_forbidden(t: &mut Table) {
    t.retain(|k, _| !FORBIDDEN_KEYS.contains(&k.as_ref()));
    for (_, v) in t.iter_mut() {
        if let Value::Table(sub) = v {
            drop_forbidden(sub);
        }
    }
}

/// Leaves out what a Look file never holds: a wallpaper palette's image
/// (kept as the flag), a theme that isn't built in, forbidden keys, keys
/// outside what a Look owns, and widgets that aren't portable.
pub fn sanitize(look: &mut UserLook) {
    if let Some(Value::Table(theme)) = look.settings.get_mut("theme")
        && let Some(Value::Table(src)) = theme.get("source")
    {
        let kind = src.get("kind").and_then(Value::as_str).unwrap_or_default();
        let builtin = src.get("preset").and_then(Value::as_str).is_some_and(builtin_theme);
        if kind == "wallpaper" {
            look.wallpaper_colors = true;
            theme.remove("source");
        } else if kind == "preset" && !builtin {
            theme.remove("source");
        }
    }
    drop_forbidden(&mut look.settings);
    let mut leaves = Vec::new();
    looks::leaves(&look.settings, "", &mut leaves);
    for (path, _) in leaves {
        if !owned(&path) {
            crate::profiles::set_path(&mut look.settings, &path, None);
        }
    }
    prune(&mut look.settings);
    look.unset.retain(|p| owned(p));
    if let Some(w) = &mut look.widgets {
        w.retain(|w| portable_kind(&w.kind));
        for x in w.iter_mut() {
            x.place.displays.clear();
        }
    }
}

/// Drops tables left empty.
fn prune(t: &mut Table) {
    for (_, v) in t.iter_mut() {
        if let Value::Table(sub) = v {
            prune(sub);
        }
    }
    t.retain(|_, v| !matches!(v, Value::Table(sub) if sub.is_empty()));
}

fn builtin_theme(id: &str) -> bool {
    mandara_theme::catalog::builtin().iter().any(|t| t.id == id)
}

/// Whether `path` lies inside what a Look owns (and isn't bookkeeping).
fn owned(path: &str) -> bool {
    let inside = |root: &str| path == root || path.starts_with(&format!("{root}."));
    OWNED.iter().any(|o| inside(o)) && !BOOKKEEPING.iter().any(|b| inside(b)) && !path.split('.').any(|k| FORBIDDEN_KEYS.contains(&k))
}

fn check_text(what: &str, s: &str, max: usize, empty_ok: bool) -> Result<()> {
    if (!empty_ok && s.trim().is_empty()) || s.chars().count() > max || s.chars().any(char::is_control) {
        bail!("the look's {what} is empty, too long or has control characters");
    }
    Ok(())
}

/// Values that could name a file or reach outside: paths, `..`, `~`.
fn path_like(s: &str) -> bool {
    s.contains('/') || s.contains('\\') || s.contains("..") || s.starts_with('~')
}

fn check_value(path: &str, v: &Value) -> Result<()> {
    match v {
        Value::String(s) => {
            if s.chars().count() > MAX_VALUE_CHARS || path_like(s) || s.chars().any(char::is_control) {
                bail!("{path}: this value isn't allowed in a look");
            }
        }
        Value::Array(a) => {
            if a.len() > MAX_LIST {
                bail!("{path}: too many entries");
            }
            for x in a {
                check_value(path, x)?;
            }
        }
        Value::Table(t) => {
            for (k, x) in t {
                check_value(&format!("{path}.{k}"), x)?;
            }
        }
        Value::Float(f) if !f.is_finite() => bail!("{path}: not a number"),
        _ => {}
    }
    Ok(())
}

/// Reads and strictly checks a Look file's text (see the module docs).
pub fn parse(text: &str) -> Result<UserLook> {
    if text.len() > MAX_FILE_BYTES {
        bail!("the file is larger than a look can be ({} KiB)", MAX_FILE_BYTES / 1024);
    }
    let look: UserLook = toml::from_str(text).context("this is not a Mandara look file")?;
    if look.format == 0 || look.format > FORMAT {
        bail!("this look was made by a newer Mandara (format {}); update to import it", look.format);
    }
    check_text("name", &look.name, MAX_NAME_CHARS, false)?;
    check_text("author", &look.author, MAX_NAME_CHARS, true)?;
    check_text("date", &look.created, MAX_NAME_CHARS, true)?;
    if !valid_id(&look.id) {
        bail!("the look's id {:?} isn't a plain name", look.id);
    }
    let Some(parent) = looks::find(&look.parent) else { bail!("the look is based on {:?}, which this version doesn't have", look.parent) };
    let mut leaves = Vec::new();
    looks::leaves(&look.settings, "", &mut leaves);
    if leaves.len() > MAX_LOOKS * 4 {
        bail!("the look changes too many settings");
    }
    for (path, v) in &leaves {
        if !owned(path) {
            bail!("{path}: a look can't change this setting");
        }
        check_value(path, v)?;
        if path.starts_with("hot_corners.") && v.as_str() == Some("command") {
            bail!("{path}: a look can't run commands");
        }
    }
    if let Some(Value::Table(theme)) = look.settings.get("theme")
        && let Some(Value::Table(src)) = theme.get("source")
    {
        match src.get("kind").and_then(Value::as_str) {
            Some("seed") => {}
            Some("preset") if src.get("preset").and_then(Value::as_str).is_some_and(builtin_theme) => {}
            _ => bail!("theme.source: a look's colours come from a seed, a built-in theme or your wallpaper"),
        }
    }
    if look.unset.len() > MAX_LIST {
        bail!("the look unsets too many settings");
    }
    for path in &look.unset {
        if !owned(path) || path_like(path) {
            bail!("{path}: a look can't change this setting");
        }
    }
    if let Some(widgets) = &look.widgets {
        if widgets.len() > crate::desktop_widgets::ranges::MAX_WIDGETS {
            bail!("the look has too many desktop widgets");
        }
        for w in widgets {
            if !portable_kind(&w.kind) || !w.place.displays.is_empty() {
                bail!("a look can't carry this desktop widget ({})", w.kind.id());
            }
            let t = Table::try_from(w).context("a desktop widget")?;
            check_value("widgets", &Value::Table(t))?;
        }
    }
    // Every value must fit: applied over the parent, each reads back as
    // written (an unknown key or a wrong value doesn't).
    let applied = parent.apply_layered(&ShellConfig::default(), LookOptions::default(), Some(&look.delta()), &look.id);
    let back = crate::profiles::to_table(&applied)?;
    for (path, v) in &leaves {
        if ZONES.contains(&path.as_str()) {
            continue;
        }
        if crate::profiles::get_path(&back, path) != Some(v) {
            bail!("{path}: this value doesn't fit");
        }
    }
    Ok(look)
}

/// An imported Look's own id and name: a fresh id, and a numbered name
/// when one of `existing` has it.
pub fn adopt(mut look: UserLook, existing: &[UserLook]) -> UserLook {
    let names: Vec<&str> = existing.iter().map(|l| l.name.as_str()).chain(looks::LOOKS.iter().map(|l| l.name)).collect();
    if names.iter().any(|n| n.eq_ignore_ascii_case(&look.name)) {
        let base = look.name.clone();
        look.name = (2..).map(|n| format!("{base} ({n})")).find(|c| !names.iter().any(|n| n.eq_ignore_ascii_case(c))).unwrap_or(base);
    }
    let ids: Vec<String> = existing.iter().map(|l| l.id.clone()).collect();
    look.id = unique_id(&look.name, &ids);
    sanitize(&mut look);
    look
}

/// A built-in Look as a file (no changes over itself).
pub fn builtin_file(look: &LookDef, created: &str) -> UserLook {
    UserLook { format: FORMAT, id: look.id.into(), name: look.name.into(), author: String::new(), parent: look.id.into(), created: created.into(), wallpaper_colors: false, unset: Vec::new(), settings: Table::new(), widgets: None }
}

// ── The folder ──────────────────────────────────────────────────────────

/// `~/.config/mandara/looks`.
pub fn looks_dir() -> PathBuf {
    crate::paths::config_dir().join("looks")
}

fn file_in(dir: &Path, id: &str) -> Result<PathBuf> {
    if !valid_id(id) {
        bail!("{id:?} isn't a look id");
    }
    Ok(dir.join(format!("{id}.toml")))
}

/// Every Look in `dir` (unreadable or invalid files are skipped), by name.
pub fn list_in(dir: &Path) -> Vec<UserLook> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<UserLook> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .take(MAX_LOOKS)
        .filter_map(|p| {
            let text = std::fs::read_to_string(&p).ok()?;
            match parse(&text) {
                Ok(l) if p.file_stem().is_some_and(|s| s == l.id.as_str()) => Some(l),
                Ok(_) => None,
                Err(err) => {
                    tracing::warn!("looks: skipping {}: {err:#}", p.display());
                    None
                }
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

pub fn find_in(dir: &Path, id: &str) -> Option<UserLook> {
    let text = std::fs::read_to_string(file_in(dir, id).ok()?).ok()?;
    parse(&text).ok().filter(|l| l.id == id)
}

pub fn save_in(dir: &Path, look: &UserLook) -> Result<PathBuf> {
    let path = file_in(dir, &look.id)?;
    if looks::find(&look.id).is_some() {
        bail!("{} is a built-in look", look.id);
    }
    crate::write_atomic(&path, look.to_text()?.as_bytes())?;
    Ok(path)
}

pub fn delete_in(dir: &Path, id: &str) -> Result<()> {
    let path = file_in(dir, id)?;
    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))
}

pub fn list() -> Vec<UserLook> {
    list_in(&looks_dir())
}

pub fn find(id: &str) -> Option<UserLook> {
    find_in(&looks_dir(), id)
}

pub fn save(look: &UserLook) -> Result<PathBuf> {
    save_in(&looks_dir(), look)
}

pub fn delete(id: &str) -> Result<()> {
    delete_in(&looks_dir(), id)
}

/// Imports `text` into `dir` (checked, given its own id): the saved Look.
pub fn import_in(dir: &Path, text: &str) -> Result<UserLook> {
    let look = adopt(parse(text)?, &list_in(dir));
    save_in(dir, &look)?;
    Ok(look)
}

/// Renames `id` (its id, and so its file, stays).
pub fn rename_in(dir: &Path, id: &str, name: &str) -> Result<UserLook> {
    check_text("name", name, MAX_NAME_CHARS, false)?;
    let mut look = find_in(dir, id).with_context(|| format!("no look {id:?}"))?;
    look.name = name.trim().to_string();
    save_in(dir, &look)?;
    Ok(look)
}

/// A copy of `id` under a new name and id.
pub fn duplicate_in(dir: &Path, id: &str) -> Result<UserLook> {
    let source = find_in(dir, id).with_context(|| format!("no look {id:?}"))?;
    let copy = adopt(UserLook { name: format!("{} copy", source.name), ..source }, &list_in(dir));
    save_in(dir, &copy)?;
    Ok(copy)
}

/// The text to export for `id`: one of yours, or a built-in Look.
pub fn export_text_in(dir: &Path, id: &str, created: &str) -> Result<String> {
    match looks::find(id) {
        Some(l) => builtin_file(l, created).to_text(),
        None => find_in(dir, id).with_context(|| format!("no look {id:?}"))?.to_text(),
    }
}

// ── Noticing customizations ─────────────────────────────────────────────

/// What changed since the active Look was applied.
#[derive(Clone, Debug, PartialEq)]
pub enum Customized {
    /// Nothing (or no Look is active).
    No,
    /// Nothing that matters, but the fingerprint moved (a newer version
    /// writes the same settings differently): store this one.
    Restamp(String),
    /// You changed what the Look owns: `delta` over the built-in `parent`.
    Changed { parent: &'static LookDef, delta: Delta },
}

/// Compares `cfg` with its active Look (`active`: the user Look when one
/// is active). Cheap when nothing changed: one fingerprint.
pub fn customized(cfg: &ShellConfig, active: Option<&UserLook>) -> Customized {
    if cfg.looks.active.is_empty() {
        return Customized::No;
    }
    let print = owned_print(cfg);
    if print == cfg.looks.print {
        return Customized::No;
    }
    let parent = match active {
        Some(l) => l.parent(),
        None => looks::find(&cfg.looks.active),
    };
    let Some(parent) = parent else { return Customized::No };
    let delta = delta_over(parent, cfg);
    let current = active.map(UserLook::delta).unwrap_or_default();
    if same_delta(&delta, &current) {
        Customized::Restamp(print)
    } else {
        Customized::Changed { parent, delta }
    }
}

/// Deltas equal up to widget ids (a re-applied Look numbers them afresh).
fn same_delta(a: &Delta, b: &Delta) -> bool {
    let prints = |w: &Option<Vec<DesktopWidget>>| w.as_ref().map(|w| w.iter().map(looks::widget_print).collect::<Vec<_>>());
    a.settings == b.settings && a.unset == b.unset && prints(&a.widgets) == prints(&b.widgets)
}

#[cfg(test)]
mod tests;
