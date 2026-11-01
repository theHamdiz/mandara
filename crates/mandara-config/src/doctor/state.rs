//! What the doctor remembers between runs (`~/.local/state/mandara/`):
//!
//! - `doctor.json`: the last report per compositor with the config files'
//!   sizes and mtimes and the compositor version it was made with, so a
//!   start-up check only runs the checker when one of those changed; the
//!   last clean state (to tell "removed in this update" from "mistyped");
//!   which problems were already announced; and snoozes.
//! - `doctor-unsupported.json`: options Mandara stops emitting for one
//!   compositor version because that version rejected them. An entry only
//!   applies to the exact version it was learned on, so an update gets to
//!   try the option again.
//!
//! "Remind me later" snoozes a compositor's problems until its config files
//! or its version change: no timer, and a new problem is never hidden by an
//! old snooze.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{CheckEnv, DiagKind, Owner, Report, check_with, config_tree, installed_version};
use crate::compositor::{Target, toml_header, toml_line_key};

/// A file's identity for change detection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub path: PathBuf,
    pub size: u64,
    pub modified_s: u64,
    pub modified_ns: u32,
}

/// Sizes and mtimes of `files` (a missing file has none).
pub fn stamps(files: &[PathBuf]) -> Vec<Stamp> {
    files
        .iter()
        .filter_map(|f| {
            let meta = std::fs::metadata(f).ok()?;
            let m = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
            Some(Stamp { path: f.clone(), size: meta.len(), modified_s: m.as_secs(), modified_ns: m.subsec_nanos() })
        })
        .collect()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Entry {
    report: Option<Report>,
    stamps: Vec<Stamp>,
    /// The version and files of the last run without problems.
    clean: Option<(Option<String>, Vec<Stamp>)>,
    /// Ids already announced with a notification.
    announced: BTreeSet<String>,
    /// Snoozed while the files and version are these.
    snoozed: Option<(Option<String>, Vec<Stamp>)>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct StateFile {
    targets: BTreeMap<String, Entry>,
}

fn state_path() -> PathBuf {
    crate::paths::state_dir().join("doctor.json")
}

fn load(path: &Path) -> StateFile {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(path: &Path, state: &StateFile) {
    if let Ok(bytes) = serde_json::to_vec_pretty(state)
        && let Err(err) = crate::store::write_atomic(path, &bytes)
    {
        tracing::warn!("config doctor state: {err:#}");
    }
}

/// Serialises read-modify-write of the state file within this process.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn with_state<T>(path: &Path, f: impl FnOnce(&mut StateFile) -> T) -> T {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut state = load(path);
    let out = f(&mut state);
    save(path, &state);
    out
}

fn key(target: Target) -> String {
    target.name().to_lowercase()
}

/// The last report for `target`, if any.
pub fn last_report(target: Target) -> Option<Report> {
    load(&state_path()).targets.get(&key(target)).and_then(|e| e.report.clone())
}

/// The user's config checked, unless nothing changed since the last check
/// (same files, sizes, mtimes and compositor version): then the stored
/// report. The boolean says whether the checker ran.
pub fn check_cached(target: Target) -> (Report, bool) {
    check_cached_in(target, &target.user_config(), &CheckEnv { version: installed_version(target), ..CheckEnv::default() }, &state_path())
}

/// [`check_cached`] for any config tree and state file.
pub fn check_cached_in(target: Target, root: &Path, env: &CheckEnv, path: &Path) -> (Report, bool) {
    let now = stamps(&config_tree(target, root));
    let version = env.version.clone();
    let stored = load(path).targets.get(&key(target)).cloned().unwrap_or_default();
    if let Some(report) = &stored.report
        && stored.stamps == now
        && report.version == version
        && report.root == root
        && version.is_some()
    {
        return (report.clone(), false);
    }
    let report = check_with(target, root, env);
    (record_in(report, path), true)
}

/// Stores a report (after a check or a fix) and returns it with problems
/// that appeared with a compositor update marked [`DiagKind::RemovedOption`].
pub fn record(report: Report) -> Report {
    record_in(report, &state_path())
}

fn record_in(mut report: Report, path: &Path) -> Report {
    with_state(path, |state| {
        let entry = state.targets.entry(key(report.target)).or_default();
        let now = stamps(&report.files);
        // A user option the last clean run accepted, in a file that hasn't
        // changed since, rejected by a new version: removed in the update.
        if let Some((clean_version, clean_stamps)) = &entry.clean
            && *clean_version != report.version
        {
            for d in report.diagnostics.iter_mut().filter(|d| d.owner == Owner::User && d.kind == DiagKind::UnknownOption) {
                let unchanged = d.file.as_ref().is_some_and(|f| now.iter().find(|s| &s.path == f).is_some_and(|s| clean_stamps.contains(s)));
                if unchanged {
                    d.kind = DiagKind::RemovedOption;
                }
            }
        }
        if report.is_clean() {
            entry.clean = Some((report.version.clone(), now.clone()));
            entry.announced.clear();
        }
        entry.stamps = now;
        entry.report = Some(report.clone());
    });
    report
}

/// Problems not announced yet (and not snoozed); marks them announced.
pub fn take_unannounced(report: &Report) -> Vec<super::Diagnostic> {
    take_unannounced_in(report, &state_path())
}

fn take_unannounced_in(report: &Report, path: &Path) -> Vec<super::Diagnostic> {
    with_state(path, |state| {
        let entry = state.targets.entry(key(report.target)).or_default();
        if entry.snoozed.as_ref().is_some_and(|(v, s)| *v == report.version && *s == stamps(&report.files)) {
            return Vec::new();
        }
        let fresh: Vec<super::Diagnostic> = report.diagnostics.iter().filter(|d| d.severity != super::Severity::Info && !entry.announced.contains(&d.id)).cloned().collect();
        entry.announced.extend(fresh.iter().map(|d| d.id.clone()));
        fresh
    })
}

/// "Remind me later": quiet until `target`'s config files or version
/// change.
pub fn snooze(report: &Report) {
    with_state(&state_path(), |state| {
        let entry = state.targets.entry(key(report.target)).or_default();
        entry.snoozed = Some((report.version.clone(), stamps(&report.files)));
        entry.announced.extend(report.diagnostics.iter().map(|d| d.id.clone()));
    });
}

/// Whether `report`'s problems are snoozed.
pub fn is_snoozed(report: &Report) -> bool {
    load(&state_path()).targets.get(&key(report.target)).and_then(|e| e.snoozed.clone()).is_some_and(|(v, s)| v == report.version && s == stamps(&report.files))
}

// ── Options this compositor version rejects ───────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Unsupported {
    /// `(compositor, version, option)`.
    skip: BTreeSet<(String, String, String)>,
}

/// Where the "unsupported on this version" list lives.
pub fn unsupported_path() -> PathBuf {
    crate::paths::state_dir().join("doctor-unsupported.json")
}

fn read_unsupported(path: &Path) -> Unsupported {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn edit_unsupported<T>(path: &Path, f: impl FnOnce(&mut Unsupported) -> T) -> T {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut list = read_unsupported(path);
    let out = f(&mut list);
    if let Ok(bytes) = serde_json::to_vec_pretty(&list)
        && let Err(err) = crate::store::write_atomic(path, &bytes)
    {
        tracing::warn!("config doctor: {err:#}");
    }
    out
}

/// Stops emitting `option` for `target` `version` (list at `path`). `true`
/// when it is new.
pub fn add_unsupported(path: &Path, target: Target, version: &str, option: &str) -> bool {
    edit_unsupported(path, |l| l.skip.insert((key(target), version.to_string(), option.to_string())))
}

pub fn remove_unsupported(path: &Path, target: Target, version: &str, option: &str) {
    edit_unsupported(path, |l| l.skip.remove(&(key(target), version.to_string(), option.to_string())));
}

/// Options Mandara leaves out for `target` at `version`.
pub fn unsupported(path: &Path, target: Target, version: &str) -> Vec<String> {
    read_unsupported(path).skip.into_iter().filter(|(t, v, _)| *t == key(target) && v == version).map(|(_, _, o)| o).collect()
}

/// Whether any option is on the list for `target` (any version): lets the
/// emitter skip asking the compositor for its version when nothing is.
pub fn has_unsupported(path: &Path, target: Target) -> bool {
    read_unsupported(path).skip.iter().any(|(t, ..)| *t == key(target))
}

/// The drop-in `text` with every line setting one of `options` commented
/// out (whole nodes and tables), each with a note saying why.
pub fn filter_emitted(target: Target, file: &Path, text: &str, options: &[String], version: &str) -> String {
    if options.is_empty() {
        return text.to_string();
    }
    let lines: Vec<&str> = text.lines().collect();
    let prefix = super::fix::comment_prefix(target, file);
    let mut skip: BTreeSet<usize> = BTreeSet::new();
    let mut notes: BTreeMap<usize, String> = BTreeMap::new();
    let mut section = String::new();
    let mut blocks: Vec<String> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if super::is_comment_line(target, line) {
            continue;
        }
        let (name, set) = match target {
            Target::Umbriel => {
                if let Some(h) = toml_header(line) {
                    section = h.to_string();
                    (h.to_string(), String::new())
                } else {
                    let k = toml_line_key(line).unwrap_or_default();
                    (k.clone(), if section.is_empty() { k } else { format!("{section}.{k}") })
                }
            }
            _ => {
                let k = super::checker(target).line_key(line).unwrap_or_default();
                let path = blocks.iter().chain(std::iter::once(&k)).cloned().collect::<Vec<_>>().join(".");
                (k, path)
            }
        };
        let matches = |o: &String| {
            let o = o.replace(':', ".");
            o == name || o == set || set.ends_with(&format!(".{o}"))
        };
        if !name.is_empty() && options.iter().any(matches) {
            let (start, end) = super::fix::node_range(target, file, &lines, i);
            skip.extend(start..=end);
            notes.insert(start, name.clone());
        }
        // Brace blocks (Niri nodes, hyprlang categories) name nested keys.
        if matches!(target, Target::Niri | Target::Hyprland) {
            let opens = line.matches('{').count();
            let closes = line.matches('}').count();
            if opens > closes
                && let Some(word) = line.split_whitespace().next()
            {
                blocks.push(word.trim_end_matches('{').to_string());
            }
            for _ in 0..closes.saturating_sub(opens) {
                blocks.pop();
            }
        }
    }
    let mut out = String::with_capacity(text.len());
    for (i, line) in lines.iter().enumerate() {
        if let Some(name) = notes.get(&i) {
            out.push_str(&format!("{prefix} Config doctor: {} {version} rejects `{name}`, so Mandara leaves it out for this version.\n", target.name()));
        }
        if skip.contains(&i) {
            out.push_str(&format!("{prefix} {line}\n"));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !text.ends_with('\n') {
        out.pop();
    }
    out
}

/// The drop-in text with the options the installed version rejects left
/// out (the text unchanged when there are none).
pub fn apply_unsupported(target: Target, file: &Path, text: &str) -> String {
    let list = unsupported_path();
    if !has_unsupported(&list, target) {
        return text.to_string();
    }
    let Some(version) = installed_version(target) else { return text.to_string() };
    filter_emitted(target, file, text, &unsupported(&list, target, &version), &version)
}
