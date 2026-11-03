//! Where language packs live, how they're loaded and installed, and the
//! hooks that let the shell download a missing one.
//!
//! # Locations
//!
//! A pack belongs to exactly one Mandara version: it is only accepted when
//! its header names the running version ([`crate::pack::Pack::parse`]).
//! Packs for version `V` are looked for, in order, in:
//!
//! 1. `$XDG_DATA_HOME/mandara/locales/V/` (`~/.local/share/…`): what the
//!    shell downloads, and what `mandara install` copies from a tarball;
//! 2. `<exe dir>/../share/mandara/locales/V/`: an unpacked release tarball,
//!    or a package that installs the binary in `/usr/bin`;
//! 3. `$XDG_DATA_DIRS/mandara/locales/V/` (`/usr/local/share` and
//!    `/usr/share` by default, and always both): packages, and the copy
//!    Mandara Login uses.
//!
//! The first valid pack wins. English is the keys themselves and needs no
//! pack; any other language without one falls back to English, exactly
//! as an untranslated string does.
//!
//! # Never changed under a live mapping
//!
//! Packs are memory-mapped read-only and the mapping lives as long as the
//! process. Writing into a mapped file in place (or truncating it) could
//! make a later read fault with `SIGBUS`. So Mandara never does: [`install`]
//! writes a temporary file in the same folder, syncs it and **renames** it
//! over the old name. A rename only swaps the directory entry: a process
//! that mapped the old file keeps reading the old inode, which the kernel
//! frees when the last mapping goes, and every later load maps the new one.
//! Pruning old versions ([`prune`]) only unlinks, which is just as safe.
//! Package managers replace files the same way (unlink or rename). Only a
//! person overwriting a pack in place (`cp` onto it) could break a running
//! shell, as with any mapped library.
//!
//! # Loading and hooks
//!
//! The first lookup in a language maps its pack (a few hundred KB, checked
//! in one pass, well under a millisecond). When none is found, lookups
//! fall back to English and the [`on_missing`] hook runs once: the shell
//! uses it to download the pack, then calls [`reload`], which maps the new
//! file and tells every [`on_loaded`] listener (the UI re-renders). Debug
//! builds without a pack compile `locales/<code>.toml` from the source tree
//! instead, so `cargo test` and `cargo run` stay translated; release
//! builds never read the sources.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::Lang;
use crate::pack::{self, Pack, PackError};

/// The folder under a data dir that holds packs (`mandara/locales`).
pub const DIR: &str = "mandara/locales";

/// `ar` → `ar.mlang`.
pub fn file_name(lang: Lang) -> String {
    format!("{}.{}", lang.code(), pack::EXTENSION)
}

/// The Mandara version packs must match: the running build's.
pub fn running_version() -> &'static str {
    mandara_version::version()
}

/// `$XDG_DATA_HOME`, or `~/.local/share`.
pub fn user_data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/")).join(".local/share"))
}

/// The user's folder of packs for `version` (where downloads go).
pub fn user_dir(version: &str) -> PathBuf {
    user_data_dir().join(DIR).join(version)
}

/// The system data dirs: `$XDG_DATA_DIRS`, then `/usr/local/share` and
/// `/usr/share` when it doesn't list them.
fn system_data_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var("XDG_DATA_DIRS").unwrap_or_default().split(':').filter(|d| d.starts_with('/')).map(PathBuf::from).collect();
    for d in ["/usr/local/share", "/usr/share"] {
        if !dirs.iter().any(|x| x == Path::new(d)) {
            dirs.push(PathBuf::from(d));
        }
    }
    dirs
}

/// Every folder searched for `version`'s packs, in order.
pub fn search_dirs(version: &str) -> Vec<PathBuf> {
    let mut dirs = vec![user_dir(version)];
    if let Some(share) = std::env::current_exe().ok().and_then(|exe| exe.parent()?.parent().map(|p| p.join("share"))) {
        dirs.push(share.join(DIR).join(version));
    }
    dirs.extend(system_data_dirs().into_iter().map(|d| d.join(DIR).join(version)));
    let mut seen = Vec::with_capacity(dirs.len());
    dirs.retain(|d| {
        let new = !seen.contains(d);
        seen.push(d.clone());
        new
    });
    dirs
}

/// Maps `path` read-only and validates it as `lang`'s pack for `version`.
/// The mapping is leaked: it lives for the rest of the process.
pub fn open(path: &Path, version: &str, lang: Lang) -> Result<&'static Pack<'static>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let len = file.metadata().map_err(|e| format!("{}: {e}", path.display()))?.len();
    if len == 0 {
        return Err(format!("{}: {}", path.display(), PackError::NotAPack));
    }
    // SAFETY: the map is read-only and the file is never written in place
    // (installs rename a new file over it, see the module docs), so the
    // bytes can't change or shrink while mapped.
    let map = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| format!("{}: {e}", path.display()))?;
    let map: &'static memmap2::Mmap = Box::leak(Box::new(map));
    let data: &'static [u8] = map;
    match Pack::parse(data, version, lang) {
        Ok(p) => Ok(Box::leak(Box::new(p))),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// The first valid pack for `lang` and `version` in [`search_dirs`].
pub fn find(lang: Lang, version: &str) -> Option<(PathBuf, &'static Pack<'static>)> {
    let name = file_name(lang);
    for dir in search_dirs(version) {
        let path = dir.join(&name);
        if !path.is_file() {
            continue;
        }
        match open(&path, version, lang) {
            Ok(p) => return Some((path, p)),
            Err(e) => eprintln!("mandara-i18n: ignoring {e}"),
        }
    }
    None
}

/// Validates `bytes` as `lang`'s pack for `version` and writes it into
/// `dir` as `<code>.mlang`: a temporary file, synced, then renamed over
/// the old one (never written in place; see the module docs).
pub fn install(lang: Lang, version: &str, bytes: &[u8], dir: &Path) -> Result<PathBuf, String> {
    // A bad download never reaches the folder.
    Pack::parse(bytes, version, lang).map_err(|e| e.to_string())?;
    write_atomic(&dir.join(file_name(lang)), bytes)
}

/// Writes `bytes` to `path` through a synced temporary file and a rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    use std::io::Write as _;
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = dir.join(format!(".{}.{}.tmp", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        // The rename itself is durable once the folder is synced.
        std::fs::File::open(dir).and_then(|d| d.sync_all())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}: {e}", path.display()));
    }
    Ok(path.to_path_buf())
}

/// Removes the user's pack folders for every version except `keep` (the
/// running version and the one an update installs, so a rollback still has
/// its packs). Returns the folders removed. Unlinking is safe for packs a
/// process still maps (see the module docs).
pub fn prune(keep: &[&str]) -> Vec<PathBuf> {
    prune_in(&user_data_dir().join(DIR), keep)
}

/// [`prune`] under an explicit `root` (`…/mandara/locales`).
pub fn prune_in(root: &Path, keep: &[&str]) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else { return removed };
    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if !path.is_dir() || keep.contains(&name.as_str()) {
            continue;
        }
        if std::fs::remove_dir_all(&path).is_ok() {
            removed.push(path);
        }
    }
    removed
}

// ── Loaded packs ─────────────────────────────────────────────────────────

/// Slot state: nothing tried yet.
const UNTRIED: u8 = 0;
/// Slot state: a pack is loaded.
const READY: u8 = 1;
/// Slot state: no pack; lookups fall back to English.
const MISSING: u8 = 2;

struct Slot {
    pack: AtomicPtr<Pack<'static>>,
    state: AtomicU8,
    path: Mutex<Option<PathBuf>>,
}

static SLOTS: [Slot; Lang::ALL.len()] = [const { Slot { pack: AtomicPtr::new(std::ptr::null_mut()), state: AtomicU8::new(UNTRIED), path: Mutex::new(None) } }; Lang::ALL.len()];
/// Serializes loads (two threads asking for one language map it once).
static LOAD: Mutex<()> = Mutex::new(());
/// Debug builds: compile the source catalogs when no pack is found.
static SOURCE_FALLBACK: AtomicBool = AtomicBool::new(cfg!(any(debug_assertions, test)));

type Hook = Box<dyn Fn(Lang) + Send + Sync>;
static ON_MISSING: OnceLock<Hook> = OnceLock::new();
static ON_LOADED: Mutex<Vec<Hook>> = Mutex::new(Vec::new());

/// Runs `f` (once per language and process, on the thread that asked)
/// when a language is used and has no valid pack. The shell downloads it
/// from there. Only the first hook set counts; languages found missing
/// before it was set are handed to it right away.
pub fn on_missing(f: impl Fn(Lang) + Send + Sync + 'static) {
    if ON_MISSING.set(Box::new(f)).is_err() {
        return;
    }
    let missing: Vec<Lang> = Lang::ALL.into_iter().filter(|l| SLOTS[*l as usize].state.load(Ordering::Acquire) == MISSING).collect();
    if let Some(hook) = ON_MISSING.get() {
        missing.into_iter().for_each(hook);
    }
}

/// Runs `f` after [`reload`] loaded a pack (the UI re-renders).
pub fn on_loaded(f: impl Fn(Lang) + Send + Sync + 'static) {
    ON_LOADED.lock().unwrap_or_else(|e| e.into_inner()).push(Box::new(f));
}

/// Stops debug builds from compiling the source catalogs when a pack is
/// missing (tests of the fallback to English). Release builds never do.
#[doc(hidden)]
pub fn disable_source_fallback() {
    SOURCE_FALLBACK.store(false, Ordering::Relaxed);
}

/// The source catalog compiled in memory (debug builds without a pack).
fn from_source(lang: Lang, version: &str) -> Option<&'static Pack<'static>> {
    if !cfg!(any(debug_assertions, test)) || !SOURCE_FALLBACK.load(Ordering::Relaxed) {
        return None;
    }
    let path = source_catalog(lang);
    let text = std::fs::read_to_string(&path).ok()?;
    let bytes = match crate::Catalog::parse(&text).and_then(|c| pack::build(lang, version, c.iter())) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("mandara-i18n: {}: {e}", path.display());
            return None;
        }
    };
    let data: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    Pack::parse(data, version, lang).ok().map(|p| &*Box::leak(Box::new(p)))
}

/// `locales/<code>.toml` in the source tree (debug builds and tooling).
pub fn source_catalog(lang: Lang) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("locales").join(format!("{}.toml", lang.code()))
}

/// The loaded pack for `lang` (loading it on first use), `None` for English
/// and for a language without a pack.
#[inline]
pub fn pack(lang: Lang) -> Option<&'static Pack<'static>> {
    let slot = &SLOTS[lang as usize];
    let p = slot.pack.load(Ordering::Acquire);
    if !p.is_null() {
        // SAFETY: slots only ever hold pointers from `Box::leak`.
        return Some(unsafe { &*p });
    }
    if lang == Lang::En || slot.state.load(Ordering::Acquire) == MISSING {
        return None;
    }
    load(lang)
}

#[cold]
fn load(lang: Lang) -> Option<&'static Pack<'static>> {
    let slot = &SLOTS[lang as usize];
    let missing = {
        let _guard = LOAD.lock().unwrap_or_else(|e| e.into_inner());
        if slot.state.load(Ordering::Acquire) != UNTRIED {
            let p = slot.pack.load(Ordering::Acquire);
            // SAFETY: as in `pack`.
            return (!p.is_null()).then(|| unsafe { &*p });
        }
        let version = running_version();
        let found = find(lang, version).map(|(path, p)| (Some(path), p)).or_else(|| from_source(lang, version).map(|p| (None, p)));
        match found {
            Some((path, p)) => {
                *slot.path.lock().unwrap_or_else(|e| e.into_inner()) = path;
                slot.pack.store(std::ptr::from_ref(p).cast_mut(), Ordering::Release);
                slot.state.store(READY, Ordering::Release);
                return Some(p);
            }
            None => {
                slot.state.store(MISSING, Ordering::Release);
                true
            }
        }
    };
    if missing && let Some(hook) = ON_MISSING.get() {
        hook(lang);
    }
    None
}

/// Looks for `lang`'s pack again (after a download) and swaps it in. The
/// previous pack stays mapped, so strings already handed out stay valid.
/// `true` when a pack is loaded now; [`on_loaded`] listeners run then.
pub fn reload(lang: Lang) -> bool {
    if lang == Lang::En {
        return true;
    }
    let slot = &SLOTS[lang as usize];
    let loaded = {
        let _guard = LOAD.lock().unwrap_or_else(|e| e.into_inner());
        match find(lang, running_version()) {
            Some((path, p)) => {
                *slot.path.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
                slot.pack.store(std::ptr::from_ref(p).cast_mut(), Ordering::Release);
                slot.state.store(READY, Ordering::Release);
                true
            }
            None => false,
        }
    };
    if loaded {
        for hook in ON_LOADED.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            hook(lang);
        }
    }
    loaded
}

/// Where `lang`'s loaded pack came from (`None`: not loaded, English, or
/// compiled from the sources in a debug build).
pub fn loaded_from(lang: Lang) -> Option<PathBuf> {
    SLOTS[lang as usize].path.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Whether `lang` can be shown translated right now (English always can).
pub fn is_available(lang: Lang) -> bool {
    lang == Lang::En || pack(lang).is_some()
}
