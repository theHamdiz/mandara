//! Reversible fixes for doctor diagnostics.
//!
//! Every fix follows the same steps: take a timestamped backup of each file
//! it touches (`~/.local/share/mandara/doctor-backups/<stamp>-<compositor>/`
//! with a `manifest.txt`), write atomically, run the compositor's checker
//! again, and put the backup back when the result is worse than before
//! (more errors, more warnings, or the checker no longer runs).
//!
//! - [`Fix::Comment`] comments the offending line out (the whole node or
//!   table when it spans several lines) with the dialect's comment syntax
//!   and a `mandara-doctor:` marker, so it is easy to find and restore.
//! - [`Fix::Remove`] deletes the same lines.
//! - [`Fix::Regenerate`] rewrites Mandara's own drop-in. When the problem is
//!   an option this compositor version rejects, it is first added to the
//!   "unsupported on this version" list ([`super::state`]), so the emitter
//!   leaves it out from then on.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use super::{CheckEnv, CheckerStatus, Owner, Report, check_with, is_comment_line, state};
use crate::compositor::{Target, is_generated};

/// The marker a commented-out line carries.
pub const MARKER: &str = "mandara-doctor:";
/// Folder name of a backup that records a file which didn't exist.
const ABSENT: &str = "-";

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Fix {
    /// Comment the line (node, table) out.
    Comment,
    /// Delete the line (node, table).
    Remove,
    /// Rewrite Mandara's drop-in (dropping options this version rejects).
    Regenerate,
}

impl Fix {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "comment" => Some(Fix::Comment),
            "remove" | "delete" => Some(Fix::Remove),
            "regenerate" | "regen" => Some(Fix::Regenerate),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Fix::Comment => "comment",
            Fix::Remove => "remove",
            Fix::Regenerate => "regenerate",
        }
    }
}

/// What a fix did.
#[derive(Clone, Debug, Serialize)]
pub struct Outcome {
    pub fix: Fix,
    /// The backup folder (restore with [`restore`]).
    pub backup: Option<PathBuf>,
    /// The fix made things worse, so it was undone.
    pub rolled_back: bool,
    /// The diagnostic is gone after the fix.
    pub resolved: bool,
    /// The check after the fix (after the rollback, when there was one).
    pub after: Report,
}

/// Where fixes keep their backups and the "unsupported on this version"
/// list.
#[derive(Clone, Debug)]
pub struct Store {
    pub backups: PathBuf,
    pub unsupported: PathBuf,
}

impl Default for Store {
    fn default() -> Self {
        Self { backups: backups_root(), unsupported: state::unsupported_path() }
    }
}

/// Applies `fix` to diagnostic `id` of `report`, re-checks with `env`, and
/// rolls back when the config got worse. `regenerate` rewrites Mandara's
/// drop-in (the caller knows the shell's config).
pub fn apply(report: &Report, id: &str, fix: Fix, env: &CheckEnv, regenerate: &dyn Fn() -> Result<()>) -> Result<Outcome> {
    apply_in(report, id, fix, env, regenerate, &Store::default())
}

/// [`apply`] with its own backup folder and list.
pub fn apply_in(report: &Report, id: &str, fix: Fix, env: &CheckEnv, regenerate: &dyn Fn() -> Result<()>, store: &Store) -> Result<Outcome> {
    let backups = store.backups.as_path();
    let Some(d) = report.find(id) else { bail!("no problem {id} in the last check; check again") };
    let target = report.target;
    match fix {
        Fix::Comment | Fix::Remove => {
            if d.owner != Owner::User {
                bail!("{id} is in Mandara's generated file; regenerate it instead");
            }
            if d.read_only || target == Target::River {
                bail!("Mandara never edits {}: change it yourself", d.file.as_deref().map_or_else(|| target.name().to_string(), |f| f.display().to_string()));
            }
            let (Some(file), Some(line), Some(text)) = (d.file.clone(), d.line, d.text.clone()) else { bail!("{id} has no line to change") };
            if is_generated(&file) || !report.files.contains(&file) {
                bail!("{} is not one of your {} config files", file.display(), target.name());
            }
            let original = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
            let lines: Vec<&str> = original.lines().collect();
            let idx = line.checked_sub(1).filter(|i| *i < lines.len()).context("that line is no longer in the file; check again")?;
            if lines[idx].trim() != text {
                bail!("{}:{line} changed since the check; check again", file.display());
            }
            let (start, end) = node_range(target, &file, &lines, idx);
            let mut out: Vec<String> = Vec::with_capacity(lines.len());
            for (i, l) in lines.iter().enumerate() {
                if (start..=end).contains(&i) {
                    if fix == Fix::Comment {
                        let indent: String = l.chars().take_while(|c| c.is_whitespace()).collect();
                        out.push(format!("{indent}{} {MARKER} {}", comment_prefix(target, &file), l.trim_start()));
                    }
                } else {
                    out.push((*l).to_string());
                }
            }
            let mut new = out.join("\n");
            if original.ends_with('\n') {
                new.push('\n');
            }
            let backup = backup_files(target, std::slice::from_ref(&file), backups)?;
            crate::store::write_atomic(&file, new.as_bytes())?;
            Ok(settle(report, id, fix, env, backup))
        }
        Fix::Regenerate => {
            if d.owner != Owner::Mandara {
                bail!("{id} is in your own config; comment it out or remove it instead");
            }
            let generated = d.file.clone().unwrap_or_else(|| target.generated_file());
            let learned = match (&d.key, &report.version) {
                (Some(key), Some(version)) if d.kind == super::DiagKind::UnsupportedByVersion => state::add_unsupported(&store.unsupported, target, version, key).then(|| (version.clone(), key.clone())),
                _ => None,
            };
            let backup = backup_files(target, &[generated], backups)?;
            if let Err(err) = regenerate() {
                restore(&backup)?;
                if let Some((version, key)) = &learned {
                    state::remove_unsupported(&store.unsupported, target, version, key);
                }
                return Err(err.context("regenerating Mandara's drop-in"));
            }
            let outcome = settle(report, id, fix, env, backup);
            if outcome.rolled_back
                && let Some((version, key)) = &learned
            {
                state::remove_unsupported(&store.unsupported, target, version, key);
            }
            Ok(outcome)
        }
    }
}

/// Re-checks after a change; restores `backup` when the result is worse.
fn settle(before: &Report, id: &str, fix: Fix, env: &CheckEnv, backup: PathBuf) -> Outcome {
    let d = before.find(id).cloned();
    let mut after = check_with(before.target, &before.root, env);
    let worse = is_worse(before, &after);
    if worse && restore(&backup).is_ok() {
        after = check_with(before.target, &before.root, env);
    }
    let resolved = !worse && d.is_some_and(|d| !after.diagnostics.iter().any(|x| x.message == d.message && x.file == d.file && x.kind == d.kind));
    Outcome { fix, backup: Some(backup), rolled_back: worse, resolved, after }
}

pub use super::comparison::is_worse;

/// Comment syntax per dialect.
pub fn comment_prefix(target: Target, file: &Path) -> &'static str {
    match target {
        Target::Niri => "//",
        Target::Hyprland if file.extension().is_some_and(|e| e == "lua") => "--",
        _ => "#",
    }
}

/// The lines a problem at `idx` spans: a Niri node through its closing
/// brace, a TOML table through its last key or a multi-line array, a Lua
/// call through its closing parenthesis, a hyprlang line continued with
/// `\`. 0-based, inclusive.
pub fn node_range(target: Target, file: &Path, lines: &[&str], idx: usize) -> (usize, usize) {
    let last = lines.len().saturating_sub(1);
    match target {
        Target::Umbriel => {
            if crate::compositor::toml_header(lines[idx]).is_some() {
                // A whole table: up to the next header, trailing blanks kept.
                let mut end = idx;
                for (i, l) in lines.iter().enumerate().skip(idx + 1) {
                    if crate::compositor::toml_header(l).is_some() {
                        break;
                    }
                    if !l.trim().is_empty() {
                        end = i;
                    }
                }
                return (idx, end);
            }
            balanced(target, lines, idx, &['[', '{'], &[']', '}'])
        }
        // A block (`input * { … }`, `bar { … }`) through its closing brace.
        Target::Niri | Target::Sway => balanced(target, lines, idx, &['{'], &['}']),
        Target::Hyprland if file.extension().is_some_and(|e| e == "lua") => balanced(target, lines, idx, &['(', '{', '['], &[')', '}', ']']),
        Target::Hyprland => {
            let mut end = idx;
            while end < last && lines[end].trim_end().ends_with('\\') {
                end += 1;
            }
            let (_, block_end) = balanced(target, lines, idx, &['{'], &['}']);
            (idx, end.max(block_end))
        }
        Target::Mango | Target::River | Target::KWin | Target::Aqueous | Target::Labwc | Target::Cosmic => (idx, idx),
    }
}

/// From `idx` until the brackets it opens are closed again (strings and
/// comments skipped); just `idx` when it opens none.
fn balanced(target: Target, lines: &[&str], idx: usize, open: &[char], close: &[char]) -> (usize, usize) {
    let mut depth = 0i32;
    for (i, l) in lines.iter().enumerate().skip(idx) {
        if i > idx && is_comment_line(target, l) {
            continue;
        }
        depth += bracket_delta(target, l, open, close);
        if depth <= 0 {
            return (idx, i);
        }
    }
    // Never closed: only the line itself (the checker reports the rest).
    (idx, idx)
}

/// Opening minus closing brackets on a line, outside strings and comments.
fn bracket_delta(target: Target, line: &str, open: &[char], close: &[char]) -> i32 {
    let mut delta = 0;
    let mut quote: Option<char> = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == '\\' {
                    chars.next();
                } else if c == q {
                    quote = None;
                }
            }
            None => {
                let comment = match target {
                    Target::Niri => c == '/' && chars.peek() == Some(&'/'),
                    Target::Hyprland => c == '#' || (c == '-' && chars.peek() == Some(&'-')),
                    _ => c == '#',
                };
                if comment {
                    break;
                }
                if c == '"' || (c == '\'' && target != Target::Niri) {
                    quote = Some(c);
                } else if open.contains(&c) {
                    delta += 1;
                } else if close.contains(&c) {
                    delta -= 1;
                }
            }
        }
    }
    delta
}

/// Where doctor backups go.
pub fn backups_root() -> PathBuf {
    crate::paths::data_dir().join("doctor-backups")
}

/// Copies `files` into a new timestamped folder under `root` (with a
/// `manifest.txt`: backup name, tab, original path; `-` for a file that
/// didn't exist yet).
pub fn backup_files(target: Target, files: &[PathBuf], root: &Path) -> Result<PathBuf> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let dir = root.join(format!("{}-{:09}-{}", now.as_secs(), now.subsec_nanos(), target.name().to_lowercase()));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut manifest = String::new();
    for (n, file) in files.iter().enumerate() {
        if file.is_file() {
            let name = format!("{n}-{}", file.file_name().and_then(|f| f.to_str()).unwrap_or("config"));
            std::fs::copy(file, dir.join(&name)).with_context(|| format!("backing up {}", file.display()))?;
            manifest.push_str(&format!("{name}\t{}\n", file.display()));
        } else {
            manifest.push_str(&format!("{ABSENT}\t{}\n", file.display()));
        }
    }
    std::fs::write(dir.join("manifest.txt"), manifest)?;
    Ok(dir)
}

/// Puts a backup back: every file as it was (a file that didn't exist is
/// removed again).
pub fn restore(backup: &Path) -> Result<Vec<PathBuf>> {
    let manifest = std::fs::read_to_string(backup.join("manifest.txt")).with_context(|| format!("{} is not a Mandara backup", backup.display()))?;
    let mut restored = Vec::new();
    for line in manifest.lines() {
        let Some((name, original)) = line.split_once('\t') else { continue };
        let original = PathBuf::from(original);
        if name == ABSENT {
            if original.is_file() {
                std::fs::remove_file(&original).with_context(|| format!("removing {}", original.display()))?;
            }
        } else {
            let bytes = std::fs::read(backup.join(name))?;
            crate::store::write_atomic(&original, &bytes)?;
        }
        restored.push(original);
    }
    Ok(restored)
}

/// Doctor backups, newest first.
pub fn backups() -> Vec<PathBuf> {
    let mut list: Vec<PathBuf> = std::fs::read_dir(backups_root()).map(|d| d.flatten().map(|e| e.path()).filter(|p| p.join("manifest.txt").exists()).collect()).unwrap_or_default();
    list.sort();
    list.reverse();
    list
}

/// Rounds of "leave out what this version rejects, rewrite, check again"
/// before a drop-in write gives up and puts the old file back (each round
/// learns every rejected option at once; more rounds only help when a
/// removal reveals another problem).
const MAX_HEAL_ROUNDS: usize = 3;

/// What [`guarded_write`] did.
#[derive(Clone, Debug, Serialize)]
pub struct Guarded {
    /// The check after the write (after a rollback, when there was one).
    pub after: Report,
    /// Options left out because this compositor version rejected them.
    pub left_out: Vec<String>,
    /// The new drop-in made the config worse, so the old one is back.
    pub rolled_back: bool,
}

/// Writes Mandara's drop-in with `write`, then checks the config: options
/// of Mandara's that this compositor version rejects are put on the
/// version's unsupported list and the drop-in written again; if the config
/// is still worse than `before`, the previous drop-in is put back (it is
/// kept in memory: drop-ins are rewritten often, so no backup folders).
/// Without a working checker (`before` didn't run) it only writes.
pub fn guarded_write(before: &Report, env: &CheckEnv, write: &dyn Fn() -> Result<()>) -> Result<Guarded> {
    guarded_write_in(before, env, write, &before.target.generated_file(), &Store::default())
}

/// [`guarded_write`] of the drop-in at `generated`, with its own store.
pub fn guarded_write_in(before: &Report, env: &CheckEnv, write: &dyn Fn() -> Result<()>, generated: &Path, store: &Store) -> Result<Guarded> {
    if before.status != CheckerStatus::Ran {
        write()?;
        return Ok(Guarded { after: before.clone(), left_out: Vec::new(), rolled_back: false });
    }
    let target = before.target;
    let lock = mandara_theme::templates::WriteLock::new(generated)?;
    let previous = std::fs::read(generated).ok();
    write()?;
    let mut written = mandara_theme::templates::FileStamp::at(lock.target())?;
    let mut after = check_with(target, &before.root, env);
    let mut left_out = Vec::new();
    for _ in 0..MAX_HEAL_ROUNDS {
        let rejected: Vec<String> = after.diagnostics.iter().filter(|d| d.owner == Owner::Mandara && d.kind == super::DiagKind::UnsupportedByVersion).filter_map(|d| d.key.clone()).collect();
        let Some(version) = after.version.clone() else { break };
        let learned: Vec<String> = rejected.into_iter().filter(|k| state::add_unsupported(&store.unsupported, target, &version, k)).collect();
        if learned.is_empty() {
            break;
        }
        left_out.extend(learned);
        lock.check_stamp(written)?;
        write()?;
        written = mandara_theme::templates::FileStamp::at(lock.target())?;
        after = check_with(target, &before.root, env);
    }
    let rolled_back = is_worse(before, &after);
    if rolled_back {
        lock.check_stamp(written)?;
        match &previous {
            Some(bytes) => crate::store::write_atomic(generated, bytes)?,
            None => std::fs::remove_file(generated).with_context(|| format!("removing {}", generated.display()))?,
        }
        after = check_with(target, &before.root, env);
    }
    Ok(Guarded { after, left_out, rolled_back })
}
