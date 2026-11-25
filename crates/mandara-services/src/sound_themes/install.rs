//! Installing sound themes into `~/.local/share/sounds/<Theme>/`
//! (`$XDG_DATA_HOME/sounds`) and removing them again.
//!
//! * **Catalog** — the pinned release file is downloaded and checked
//!   exactly as cursor themes are (`cursors::install::download`: size and
//!   SHA-256 must match, HTTPS only).
//! * **Your own** — an archive or a folder you pick, or an `https://` link
//!   you paste (unverified: callers ask before installing it).
//! * **Unpack** — pure-Rust readers into a hidden scratch folder beside the
//!   sounds folder; unsafe paths, links leaving the archive, devices and
//!   FIFOs are refused or skipped (`app_install::archive`), and unpacking
//!   stops past [`MAX_UNPACKED`].
//! * **Assemble** — only the theme is copied out: `index.theme`, sound
//!   files (`.oga`, `.ogg`, `.wav`, `.disabled`, each under
//!   [`MAX_SOUND_FILE`]), links between them, and the archive's licence
//!   and credit files; at most [`MAX_FILES`] files and [`MAX_THEME_BYTES`].
//!   Releases that leave their layout to an install step are assembled by
//!   copying and renaming, never by running anything.
//! * **Manifest** — `~/.local/share/mandara/sound-themes.json` lists what
//!   Mandara installed (and what it changed on the desktop, see
//!   [`super::desktop`]). A folder that exists but isn't listed belongs to
//!   you or a package and is never replaced or removed.

use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

use super::catalog::{Layout, SoundTheme};
use super::desktop::Saved;
use crate::cursors::install::{Phase, Progress};

/// Largest download accepted (every catalog file is far below it).
pub const MAX_DOWNLOAD: u64 = 64 << 20;
/// Unpacking stops past this many bytes (archive bombs, huge repositories).
pub const MAX_UNPACKED: u64 = 256 << 20;
/// What an installed theme may hold.
pub const MAX_THEME_BYTES: u64 = 64 << 20;
pub const MAX_FILES: usize = 2048;
/// Largest single sound file kept (a theme's sounds are seconds long).
pub const MAX_SOUND_FILE: u64 = 16 << 20;
/// Largest `index.theme`, licence or credit file read or kept.
const MAX_TEXT_FILE: u64 = 1 << 20;
/// Folder depth searched and copied below a theme's root.
const MAX_DEPTH: usize = 4;
const MANIFEST_FILE: &str = "sound-themes.json";
/// Prefix of the scratch folders unpacking happens in.
const SCRATCH_PREFIX: &str = ".mandara-sound-";
/// Download / hash chunk.
const CHUNK: usize = 64 << 10;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const BODY_TIMEOUT: Duration = Duration::from_secs(180);
const SOUND_EXTENSIONS: [&str; 4] = ["oga", "ogg", "wav", "disabled"];
/// Licence and credit files kept with a theme (name prefixes, any case).
const CREDIT_PREFIXES: [&str; 8] = ["copying", "copyright", "license", "licence", "credits", "authors", "readme", "notice"];
const INDEX: &str = "index.theme";
const SOUND_THEME_GROUP: &str = "[Sound Theme]";
const STEREO: &str = "stereo";
/// The longest theme name accepted for a folder.
const MAX_NAME: usize = 64;
const OGG_MAGIC: &[u8] = b"OggS";
const RIFF_MAGIC: &[u8] = b"RIFF";
const WAVE_MAGIC: &[u8] = b"WAVE";

/// Where themes go and where Mandara keeps its records.
#[derive(Clone, Debug)]
pub struct Env {
    /// `~/.local/share/sounds` (`$XDG_DATA_HOME/sounds`).
    pub sounds: PathBuf,
    /// Downloads in progress (`~/.cache/mandara/sounds`).
    pub cache: PathBuf,
    /// The install manifest.
    pub manifest: PathBuf,
}

impl Env {
    pub fn current() -> Self {
        let data = dirs::data_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"));
        Self { sounds: data.join("sounds"), cache: mandara_config::paths::cache_dir().join("sounds"), manifest: mandara_config::paths::data_dir().join(MANIFEST_FILE) }
    }

    /// Everything under `root` (tests, sandboxes).
    pub fn under(root: &Path) -> Self {
        Self { sounds: root.join("sounds"), cache: root.join("cache"), manifest: root.join(MANIFEST_FILE) }
    }
}

/// Where an installed theme came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Source {
    Catalog { id: String },
    File { path: PathBuf },
    Url { url: String },
}

/// A theme Mandara installed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    /// The folder's name (the theme name apps are given).
    pub theme: String,
    pub source: Source,
    /// SHA-256 of the archive it came from (empty for a folder).
    pub sha256: String,
    pub dir: PathBuf,
    /// Unix seconds.
    pub installed_at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub themes: Vec<Installed>,
    /// The desktop's sound settings before Mandara first changed them.
    pub desktop: Saved,
}

impl Manifest {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        mandara_config::write_atomic(path, text.as_bytes()).map_err(|e| format!("{e:#}"))
    }

    pub fn get(&self, theme: &str) -> Option<&Installed> {
        self.themes.iter().find(|i| i.theme == theme)
    }
}

/// The themes Mandara installed (and may remove).
pub fn installed(env: &Env) -> Vec<Installed> {
    Manifest::load(&env.manifest).themes
}

fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) { Err(t("Cancelled").to_string()) } else { Ok(()) }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Whether `name` is usable as a theme folder name.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME && !name.starts_with('.') && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '+' | ' '))
}

/// A folder name from anything (a picked folder's or archive's name).
pub fn sanitize_name(raw: &str) -> String {
    let mut out: String = raw.trim().chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '+') { c } else { '-' }).collect();
    out = out.trim_matches(|c| c == '.' || c == '-').to_string();
    out.truncate(out.char_indices().nth(MAX_NAME).map_or(out.len(), |(i, _)| i));
    if out.is_empty() { "imported-sounds".to_string() } else { out }
}

/// Removes a scratch folder when dropped (success or failure).
struct Scratch(PathBuf);

impl Scratch {
    fn new(sounds: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(sounds).map_err(|e| format!("{}: {e}", sounds.display()))?;
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        Ok(Self(sounds.join(format!("{SCRATCH_PREFIX}{}-{nanos}", std::process::id()))))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Unpacks `archive_path` into `dest`, stopping past `cap` bytes
/// ([`MAX_UNPACKED`]) or when `cancel` is set.
fn unpack_capped(archive_path: &Path, dest: &Path, cap: u64, progress: &dyn Fn(Progress), cancel: &AtomicBool) -> Result<(), String> {
    let total = std::fs::metadata(archive_path).map(|m| m.len()).unwrap_or(0);
    let stop = AtomicBool::new(false);
    let over = AtomicBool::new(false);
    progress(Progress { phase: Phase::Unpack, done: 0, total });
    let result = crate::cursors::install::unpack(archive_path, dest, &stop, &|done| {
        if done > cap {
            over.store(true, Ordering::Relaxed);
            stop.store(true, Ordering::Relaxed);
        } else if cancel.load(Ordering::Relaxed) {
            stop.store(true, Ordering::Relaxed);
        }
        // The unpacked size isn't known up front: shown against the
        // archive's size, capped.
        progress(Progress { phase: Phase::Unpack, done: done.min(total), total });
    });
    if over.load(Ordering::Relaxed) {
        return Err(t("The archive unpacks to more than a sound theme should hold").to_string());
    }
    cancelled(cancel)?;
    result.map(|_| ())
}

fn is_sound_file_name(name: &str) -> bool {
    Path::new(name).extension().and_then(|e| e.to_str()).is_some_and(|e| SOUND_EXTENSIONS.iter().any(|s| e.eq_ignore_ascii_case(s)))
}

fn is_credit_file_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    CREDIT_PREFIXES.iter().any(|p| lower.starts_with(p)) || lower.ends_with(".license")
}

/// Whether `path`'s first bytes are an Ogg or a WAV file (`.disabled`
/// markers are empty and count as sounds).
fn plays(path: &Path) -> bool {
    if path.extension().is_some_and(|e| e == "disabled") {
        return true;
    }
    let mut head = [0u8; 12];
    let Ok(n) = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)) else { return false };
    let head = &head[..n];
    head.starts_with(OGG_MAGIC) || (head.starts_with(RIFF_MAGIC) && head.get(8..12) == Some(WAVE_MAGIC))
}

/// Counts what a theme being assembled holds.
#[derive(Default)]
struct Budget {
    files: usize,
    bytes: u64,
}

impl Budget {
    fn take(&mut self, bytes: u64) -> Result<(), String> {
        self.files += 1;
        self.bytes += bytes;
        if self.files > MAX_FILES || self.bytes > MAX_THEME_BYTES {
            return Err(t("The theme holds more files than a sound theme should").to_string());
        }
        Ok(())
    }
}

/// Copies `from` (a regular file of at most `limit` bytes) to `to`, as a
/// plain `0644` file.
fn copy_file(from: &Path, to: &Path, limit: u64, budget: &mut Budget) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let meta = std::fs::metadata(from).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > limit {
        return Ok(());
    }
    budget.take(meta.len())?;
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut src = std::fs::File::open(from).map_err(|e| e.to_string())?.take(limit);
    let mut out = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o644).open(to).map_err(|e| format!("{}: {e}", to.display()))?;
    std::io::copy(&mut src, &mut out).map_err(|e| e.to_string())?;
    Ok(())
}

/// Copies the theme under `src` into `out`: sound files, `index.theme` at
/// the top, licence and credit files, and links. A link to a sound inside
/// the theme stays a link; one to a sound elsewhere in the unpacked archive
/// (`bounds`) becomes a copy; any other is dropped.
fn copy_tree(src: &Path, out: &Path, bounds: &Path, budget: &mut Budget) -> Result<(), String> {
    let src_real = std::fs::canonicalize(src).map_err(|e| e.to_string())?;
    let bounds_real = std::fs::canonicalize(bounds).map_err(|e| e.to_string())?;
    let mut stack = vec![(src.to_path_buf(), out.to_path_buf(), 0usize)];
    while let Some((dir, dest, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = e.path();
            let to = dest.join(&name);
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if depth < MAX_DEPTH {
                    stack.push((path, to, depth + 1));
                }
            } else if ft.is_symlink() {
                if !is_sound_file_name(&name) {
                    continue;
                }
                let Ok(target) = std::fs::canonicalize(&path) else { continue };
                if !target.is_file() {
                    continue;
                }
                if target.starts_with(&src_real) {
                    let link = std::fs::read_link(&path).map_err(|e| e.to_string())?;
                    budget.take(0)?;
                    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
                    std::os::unix::fs::symlink(link, &to).map_err(|e| e.to_string())?;
                } else if target.starts_with(&bounds_real) {
                    copy_file(&target, &to, MAX_SOUND_FILE, budget)?;
                }
            } else if ft.is_file() {
                if is_sound_file_name(&name) {
                    copy_file(&path, &to, MAX_SOUND_FILE, budget)?;
                } else if (depth == 0 && name == INDEX) || is_credit_file_name(&name) {
                    copy_file(&path, &to, MAX_TEXT_FILE, budget)?;
                }
            }
        }
    }
    Ok(())
}

/// Licence and credit files in the folders above `src` up to `bounds`
/// (an archive's top folder), copied into `out` beside the theme's own.
fn copy_credits_above(src: &Path, bounds: &Path, out: &Path, budget: &mut Budget) -> Result<(), String> {
    let mut dir = src.parent();
    while let Some(d) = dir {
        if !d.starts_with(bounds) {
            break;
        }
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.file_type().is_ok_and(|t| t.is_file()) && is_credit_file_name(&name) && !out.join(&name).exists() {
                copy_file(&e.path(), &out.join(&name), MAX_TEXT_FILE, budget)?;
            }
        }
        if d == bounds {
            break;
        }
        dir = d.parent();
    }
    Ok(())
}

fn read_text(path: &Path) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut text = String::new();
    file.take(MAX_TEXT_FILE).read_to_string(&mut text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(text)
}

/// Whether `dir` is a usable sound theme: an `index.theme` with a
/// `[Sound Theme]` group and at least one playable sound.
pub fn is_sound_theme(dir: &Path) -> bool {
    let real_dir = std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir());
    if !real_dir || !read_text(&dir.join(INDEX)).is_ok_and(|t| t.lines().any(|l| l.trim() == SOUND_THEME_GROUP)) {
        return false;
    }
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() && depth < MAX_DEPTH {
                stack.push((p, depth + 1));
            } else if p.is_file() && is_sound_file_name(&e.file_name().to_string_lossy()) && plays(&p) {
                return true;
            }
        }
    }
    false
}

/// A minimal `index.theme` for a folder of sounds without one.
fn generated_index(name: &str) -> String {
    format!("{SOUND_THEME_GROUP}\nName={name}\nDirectories={STEREO}\n\n[{STEREO}]\nOutputProfile={STEREO}\n")
}

/// Builds the theme in `out` from `src` (inside the unpacked `bounds`).
fn assemble(src: &Path, bounds: &Path, layout: &Layout, aliases: &[(&str, &str)], out: &Path) -> Result<(), String> {
    let mut budget = Budget::default();
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    match layout {
        Layout::Ready => copy_tree(src, out, bounds, &mut budget)?,
        Layout::Index(text) => copy_with_index(src, bounds, out, text, &mut budget)?,
        Layout::Template { file, replace } => {
            let mut text = read_text(&src.join(file))?;
            for (from, to) in *replace {
                text = text.replace(from, to);
            }
            copy_with_index(src, bounds, out, &text, &mut budget)?;
        }
        Layout::Renamed(files) => {
            copy_file(&src.join(INDEX), &out.join(INDEX), MAX_TEXT_FILE, &mut budget)?;
            for (from, to) in *files {
                let from = src.join(from);
                let ext = from.extension().and_then(|e| e.to_str()).filter(|e| is_sound_file_name(&format!("x.{e}"))).ok_or_else(|| tf("{file} isn't a sound file", &[("file", &from.display())]))?;
                // Inside the archive and a real file (no link out of it).
                let real = std::fs::canonicalize(&from).map_err(|e| format!("{}: {e}", from.display()))?;
                if !real.starts_with(std::fs::canonicalize(bounds).map_err(|e| e.to_string())?) {
                    return Err(t("The archive contains unsafe paths").to_string());
                }
                copy_file(&real, &out.join(STEREO).join(format!("{to}.{ext}")), MAX_SOUND_FILE, &mut budget)?;
            }
            for e in std::fs::read_dir(src).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if e.file_type().is_ok_and(|t| t.is_file()) && is_credit_file_name(&name) {
                    copy_file(&e.path(), &out.join(&name), MAX_TEXT_FILE, &mut budget)?;
                }
            }
        }
    }
    finish(src, bounds, aliases, out, &mut budget)
}

/// Copies the sounds under `src` and writes `index` as the theme's index.
fn copy_with_index(src: &Path, bounds: &Path, out: &Path, index: &str, budget: &mut Budget) -> Result<(), String> {
    copy_tree(src, out, bounds, budget)?;
    std::fs::write(out.join(INDEX), index).map_err(|e| e.to_string())
}

/// Credits from above the theme, the release's aliases, then the check
/// that a playable theme came out.
fn finish(src: &Path, bounds: &Path, aliases: &[(&str, &str)], out: &Path, budget: &mut Budget) -> Result<(), String> {
    copy_credits_above(src, bounds, out, budget)?;
    let stereo = out.join(STEREO);
    for (name, target) in aliases {
        let has = |stem: &str| crate::sounds::EXTENSIONS.iter().map(|e| (e, stereo.join(format!("{stem}.{e}")))).find(|(_, p)| p.exists());
        if has(name).is_none()
            && let Some((ext, _)) = has(target)
        {
            std::os::unix::fs::symlink(format!("{target}.{ext}"), stereo.join(format!("{name}.{ext}"))).map_err(|e| e.to_string())?;
        }
    }
    if !is_sound_theme(out) {
        return Err(t("This isn't a sound theme (no index.theme with sounds)").to_string());
    }
    Ok(())
}

/// Moves the assembled theme `built` into place as `name`, replacing only
/// Mandara's own earlier copy, and records it.
fn put_in_place(built: &Path, name: &str, source: Source, sha256: &str, env: &Env) -> Result<Installed, String> {
    let target = env.sounds.join(name);
    let mut manifest = Manifest::load(&env.manifest);
    let ours = manifest.get(name).is_some();
    if std::fs::symlink_metadata(&target).is_ok() {
        if !ours {
            return Err(tf("{theme} is already installed (not by Mandara), so it was left as it is", &[("theme", &name)]));
        }
        std::fs::remove_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    std::fs::rename(built, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    let entry = Installed { theme: name.to_string(), source, sha256: sha256.to_string(), dir: target, installed_at: now_secs() };
    manifest.themes.retain(|i| i.theme != name);
    manifest.themes.push(entry.clone());
    manifest.save(&env.manifest)?;
    Ok(entry)
}

/// Refuses early when `name` can't be (re)installed.
fn check_target(name: &str, env: &Env) -> Result<(), String> {
    if !valid_name(name) {
        return Err(tf("“{theme}” isn't a valid theme name", &[("theme", &name)]));
    }
    let ours = Manifest::load(&env.manifest).get(name).is_some();
    if std::fs::symlink_metadata(env.sounds.join(name)).is_ok() && !ours {
        return Err(tf("{theme} is already installed (not by Mandara), so it was left as it is", &[("theme", &name)]));
    }
    Ok(())
}

/// Installs catalog `theme` from an already verified `archive_path`.
pub fn install_archive(archive_path: &Path, theme: &SoundTheme, sha256: &str, env: &Env, progress: &dyn Fn(Progress), cancel: &AtomicBool) -> Result<Installed, String> {
    check_target(theme.theme, env)?;
    let scratch = Scratch::new(&env.sounds)?;
    let unpacked = scratch.0.join("x");
    unpack_capped(archive_path, &unpacked, MAX_UNPACKED, progress, cancel)?;
    let src = unpacked.join(theme.asset.folder);
    if !src.starts_with(&unpacked) || !std::fs::symlink_metadata(&src).is_ok_and(|m| m.is_dir()) {
        return Err(tf("The download has no sound theme at {folder}", &[("folder", &theme.asset.folder)]));
    }
    let built = scratch.0.join("theme");
    assemble(&src, &unpacked, &theme.layout, theme.aliases, &built)?;
    cancelled(cancel)?;
    let entry = put_in_place(&built, theme.theme, Source::Catalog { id: theme.id.to_string() }, sha256, env)?;
    drop(scratch);
    progress(Progress { phase: Phase::Done, done: 1, total: 1 });
    Ok(entry)
}

/// Downloads, verifies and installs catalog `theme`. The download is
/// deleted afterwards.
pub fn install(theme: &SoundTheme, env: &Env, progress: &dyn Fn(Progress), cancel: &AtomicBool) -> Result<Installed, String> {
    check_target(theme.theme, env)?;
    let archive_path = crate::cursors::install::download(&theme.asset, &env.cache, progress, cancel)?;
    let result = install_archive(&archive_path, theme, theme.asset.sha256, env, progress, cancel);
    let _ = std::fs::remove_file(&archive_path);
    result
}

/// The sound theme folder inside `dir`: `dir` itself or the first folder
/// below it (breadth first) with an `index.theme` holding `[Sound Theme]`,
/// else one with a `stereo` folder of sounds.
pub fn find_theme_root(dir: &Path) -> Option<PathBuf> {
    let mut level = vec![dir.to_path_buf()];
    let mut loose: Option<PathBuf> = None;
    for _ in 0..=MAX_DEPTH {
        let mut next = Vec::new();
        for d in &level {
            if !std::fs::symlink_metadata(d).is_ok_and(|m| m.is_dir()) {
                continue;
            }
            if read_text(&d.join(INDEX)).is_ok_and(|t| t.lines().any(|l| l.trim() == SOUND_THEME_GROUP)) {
                return Some(d.clone());
            }
            let stereo = d.join(STEREO);
            if loose.is_none() && std::fs::read_dir(&stereo).into_iter().flatten().flatten().any(|e| is_sound_file_name(&e.file_name().to_string_lossy())) {
                loose = Some(d.clone());
            }
            let mut children: Vec<PathBuf> = std::fs::read_dir(d).into_iter().flatten().flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && !e.file_name().to_string_lossy().starts_with('.')).map(|e| e.path()).collect();
            children.sort();
            next.extend(children);
        }
        level = next;
    }
    loose
}

/// Installs the theme found in `src_dir` (inside `bounds`) under a name
/// from `fallback_name` when the folder has none of its own.
fn install_found(src_dir: &Path, bounds: &Path, fallback_name: &str, source: Source, sha256: &str, scratch: &Scratch, env: &Env) -> Result<Installed, String> {
    let root = find_theme_root(src_dir).ok_or_else(|| t("No sound theme found there (no index.theme or stereo folder with sounds)").to_string())?;
    let name = if root == bounds { sanitize_name(fallback_name) } else { sanitize_name(&root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()) };
    check_target(&name, env)?;
    let built = scratch.0.join("theme");
    if root.join(INDEX).is_file() {
        assemble(&root, bounds, &Layout::Ready, &[], &built)?;
    } else {
        // A folder of sounds without an index: one is written for it.
        let mut budget = Budget::default();
        std::fs::create_dir_all(&built).map_err(|e| e.to_string())?;
        copy_with_index(&root, bounds, &built, &generated_index(&name), &mut budget)?;
        finish(&root, bounds, &[], &built, &mut budget)?;
    }
    put_in_place(&built, &name, source, sha256, env)
}

/// Installs a sound theme from a folder or an archive you picked.
pub fn import(path: &Path, env: &Env, progress: &dyn Fn(Progress), cancel: &AtomicBool) -> Result<Installed, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let source = Source::File { path: path.to_path_buf() };
    let stem = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = stem.split(".tar").next().unwrap_or(&stem).trim_end_matches(".zip").to_string();
    let scratch = Scratch::new(&env.sounds)?;
    let entry = if meta.is_dir() {
        let real = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
        if std::fs::canonicalize(&env.sounds).is_ok_and(|s| real.starts_with(s)) {
            return Err(t("That theme is already in your sounds folder").to_string());
        }
        install_found(&real, &real, &stem, source, "", &scratch, env)?
    } else {
        let sha = crate::cursors::install::sha256_file(path)?;
        let unpacked = scratch.0.join("x");
        unpack_capped(path, &unpacked, MAX_UNPACKED, progress, cancel)?;
        install_found(&unpacked, &unpacked, &stem, source, &sha, &scratch, env)?
    };
    drop(scratch);
    progress(Progress { phase: Phase::Done, done: 1, total: 1 });
    Ok(entry)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Downloads `url` (HTTPS only, at most [`MAX_DOWNLOAD`]) into `cache` and
/// returns the file and its SHA-256. Unverified: nothing pins it.
pub fn download_url(url: &str, cache: &Path, progress: &dyn Fn(Progress), cancel: &AtomicBool) -> Result<(PathBuf, String), String> {
    if !url.starts_with("https://") {
        return Err(t("Only https:// links can be installed").to_string());
    }
    crate::offline::check()?;
    std::fs::create_dir_all(cache).map_err(|e| format!("{}: {e}", cache.display()))?;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_connect(Some(CONNECT_TIMEOUT)).timeout_recv_body(Some(BODY_TIMEOUT)).https_only(true).build().into();
    let mut response = agent.get(url).header("User-Agent", concat!("mandara-shell/", env!("CARGO_PKG_VERSION"))).call().map_err(|e| tf("Download failed: {error}", &[("error", &e)]))?;
    let total = response.body().content_length().unwrap_or(0).min(MAX_DOWNLOAD);
    let mut reader = response.body_mut().with_config().limit(MAX_DOWNLOAD + 1).reader();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let part = cache.join(format!("url-{}-{nanos}.part", std::process::id()));
    let result = (|| {
        let mut out = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
        let mut hasher = sha2::Sha256::new();
        let mut buf = vec![0u8; CHUNK];
        let mut got = 0u64;
        loop {
            cancelled(cancel)?;
            let n = reader.read(&mut buf).map_err(|e| tf("Download failed: {error}", &[("error", &e)]))?;
            if n == 0 {
                break;
            }
            got += n as u64;
            if got > MAX_DOWNLOAD {
                return Err(t("The download is larger than a sound theme should be").to_string());
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            progress(Progress { phase: Phase::Download, done: got, total: total.max(got) });
        }
        out.sync_all().map_err(|e| e.to_string())?;
        let sha = hex(&hasher.finalize());
        let file = cache.join(&sha);
        std::fs::rename(&part, &file).map_err(|e| e.to_string())?;
        Ok((file, sha))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// Installs a sound theme from an `https://` link (an archive). The caller
/// has asked whether to trust it.
pub fn import_url(url: &str, env: &Env, progress: &dyn Fn(Progress), cancel: &AtomicBool) -> Result<Installed, String> {
    let (file, sha) = download_url(url, &env.cache, progress, cancel)?;
    let stem = url.rsplit('/').next().unwrap_or_default();
    let stem = stem.split(".tar").next().unwrap_or(stem).trim_end_matches(".zip").to_string();
    let result = (|| {
        let scratch = Scratch::new(&env.sounds)?;
        let unpacked = scratch.0.join("x");
        unpack_capped(&file, &unpacked, MAX_UNPACKED, progress, cancel)?;
        let entry = install_found(&unpacked, &unpacked, &stem, Source::Url { url: url.to_string() }, &sha, &scratch, env)?;
        progress(Progress { phase: Phase::Done, done: 1, total: 1 });
        Ok(entry)
    })();
    let _ = std::fs::remove_file(&file);
    result
}

/// Removes a theme Mandara installed. Folders Mandara didn't install are
/// refused, and only the exact folder it recorded is deleted.
pub fn remove(theme: &str, env: &Env) -> Result<Installed, String> {
    let mut manifest = Manifest::load(&env.manifest);
    let refused = || tf("{theme} wasn't installed by Mandara, so it stays", &[("theme", &theme)]);
    let Some(entry) = manifest.get(theme).cloned() else { return Err(refused()) };
    let expected = env.sounds.join(theme);
    if !valid_name(theme) || entry.dir != expected {
        return Err(refused());
    }
    match std::fs::symlink_metadata(&expected) {
        // Replaced by a link since: drop the record, leave the link.
        Ok(m) if m.file_type().is_symlink() => {}
        Ok(_) => std::fs::remove_dir_all(&expected).map_err(|e| format!("{}: {e}", expected.display()))?,
        Err(_) => {}
    }
    manifest.themes.retain(|i| i.theme != theme);
    manifest.save(&env.manifest)?;
    Ok(entry)
}

/// Leftover scratch folders from an interrupted install.
pub fn clean_scratch(env: &Env) {
    for e in std::fs::read_dir(&env.sounds).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with(SCRATCH_PREFIX) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

#[cfg(test)]
mod tests;
