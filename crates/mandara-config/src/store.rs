//! Reactive TOML-backed configuration store.
//!
//! The store is the single in-memory source of truth: Studio sliders call
//! [`ConfigStore::update`] (cheap, no I/O) every frame while dragging, and a
//! debounced [`ConfigStore::flush`] persists atomically afterwards. External
//! edits are picked up through [`ConfigStore::watch`]; our own writes are
//! recognised by content hash and ignored.

mod publication;
pub use publication::{PublishedWriteError, published_stamp};

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::schema::ShellConfig;

/// Outcome of re-reading the config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadOutcome {
    /// File content equals what we last wrote or loaded.
    Unchanged,
    /// External edit applied.
    Changed,
}

#[derive(Clone)]
pub struct ConfigStore {
    path: PathBuf,
    current: Arc<ShellConfig>,
    /// The last disk snapshot is the merge base for unsaved local edits.
    /// Shared with `current` until an edit, and again after saving.
    saved: Arc<ShellConfig>,
    dirty: bool,
    /// Hash of the file bytes we last read or wrote.
    disk_hash: Option<u64>,
    /// Defaults loaded after a parse error must never replace the user's file.
    load_error: Option<String>,
}

/// A shared snapshot for a serialized background save.
pub struct SaveRequest {
    store: ConfigStore,
    queued: Arc<ShellConfig>,
    base_hash: Option<u64>,
}

pub struct Saved {
    store: ConfigStore,
    queued: Arc<ShellConfig>,
    base_hash: Option<u64>,
}

/// A disk read paired with its merge base, completed away from the UI.
pub struct ReloadRequest {
    store: ConfigStore,
    queued: Arc<ShellConfig>,
    base_hash: Option<u64>,
}
pub struct Reloaded {
    store: ConfigStore,
    queued: Arc<ShellConfig>,
    base_hash: Option<u64>,
}
impl ReloadRequest {
    pub fn read(mut self) -> Result<Reloaded> {
        self.store.reload_from_disk()?;
        Ok(Reloaded {
            store: self.store,
            queued: self.queued,
            base_hash: self.base_hash,
        })
    }
}

impl SaveRequest {
    pub fn write(mut self) -> Result<Saved> {
        self.store.flush()?;
        Ok(Saved {
            store: self.store,
            queued: self.queued,
            base_hash: self.base_hash,
        })
    }
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

const HEADER: &str = "\
# Mandara Shell configuration.
# Edited live by Mandara Studio; hand edits are picked up automatically.
# Compositor rules are emitted to ~/.config/mandara/generated/ — your own
# compositor config files are never modified beyond a single include line.

";

impl ConfigStore {
    /// Opens `~/.config/mandara/config.toml`, creating it with defaults.
    pub fn open_default() -> Result<Self> {
        Self::open(crate::paths::config_file())
    }

    /// Opens a config file, writing defaults if it does not exist yet. A file
    /// that fails to parse is reported and the defaults are used in memory
    /// (saving stays disabled until a valid file is reloaded).
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let current = Arc::new(ShellConfig::default());
        let mut store = Self {
            path,
            saved: current.clone(),
            current, dirty: false, disk_hash: None, load_error: None };
        if store.path.exists() {
            let bytes = fs::read(&store.path).with_context(|| format!("reading {}", store.path.display()))?;
            store.disk_hash = Some(hash_bytes(&bytes));
            match parse(&bytes) {
                Ok(cfg) => {
                    store.current = Arc::new(cfg);
                    store.saved = store.current.clone();
                }
                Err(err) => {
                    store.load_error = Some(format!("{err:#}"));
                    tracing::error!("{}: {err:#}; using defaults in memory without saving", store.path.display());
                }
            }
        } else {
            let mut fresh = ShellConfig::default();
            crate::desktop_widgets::migrate(&mut fresh);
            store.current = Arc::new(fresh);
            store.dirty = true;
            store.flush()?;
        }
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self) -> &ShellConfig {
        &self.current
    }

    /// Cheap shared snapshot for handing to other threads.
    pub fn snapshot(&self) -> Arc<ShellConfig> {
        self.current.clone()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn reload_request(&self) -> ReloadRequest {
        ReloadRequest {
            store: self.clone(),
            queued: self.current.clone(),
            base_hash: self.disk_hash,
        }
    }

    /// Retain edits made while a background read was in flight. An older
    /// read cannot replace a newer disk snapshot already incorporated here.
    pub fn finish_reload(&mut self, loaded: Reloaded) -> Result<ReloadOutcome> {
        if self.disk_hash != loaded.base_hash && self.disk_hash != loaded.store.disk_hash {
            return Ok(ReloadOutcome::Unchanged);
        }
        let merged = crate::patch::ConfigPatch::between(&loaded.queued, &self.current)?
            .apply(&loaded.store.current)?;
        let changed = merged != *self.current;
        self.saved = loaded.store.saved;
        self.current = if merged == *self.saved {
            self.saved.clone()
        } else {
            Arc::new(merged)
        };
        self.disk_hash = loaded.store.disk_hash;
        self.load_error = loaded.store.load_error;
        // A reload can predate a save whose rename succeeded but whose sync failed.
        // Only a completed save acknowledges local writes.
        self.dirty = self.dirty || self.current != self.saved;
        Ok(if changed {
            ReloadOutcome::Changed
        } else {
            ReloadOutcome::Unchanged
        })
    }

    pub fn save_request(&self) -> Option<SaveRequest> {
        self.dirty.then(|| SaveRequest {
            store: self.clone(),
            queued: self.current.clone(),
            base_hash: self.disk_hash,
        })
    }

    /// Incorporate a completed save without losing edits made while it ran.
    /// A newer file-watcher snapshot takes precedence over an older completion.
    pub fn finish_save(&mut self, saved: Saved) -> Result<bool> {
        if self.disk_hash != saved.base_hash && self.disk_hash != saved.store.disk_hash {
            return Ok(false);
        }
        let pending = crate::patch::ConfigPatch::between(&saved.queued, &self.current)?;
        let merged = pending.apply(&saved.store.current)?;
        let changed = merged != *self.current;
        self.saved = saved.store.current;
        self.current = if merged == *self.saved {
            self.saved.clone()
        } else {
            Arc::new(merged)
        };
        self.dirty = self.current != self.saved;
        self.disk_hash = saved.store.disk_hash;
        Ok(changed)
    }

    /// Mutates the in-memory config. Returns `true` if anything changed.
    /// Does no I/O; call [`ConfigStore::flush`] to persist.
    pub fn update(&mut self, f: impl FnOnce(&mut ShellConfig)) -> bool {
        let mut next = (*self.current).clone();
        f(&mut next);
        if next == *self.current {
            return false;
        }
        self.current = Arc::new(next);
        self.dirty = true;
        true
    }

    /// Replaces the whole config.
    pub fn replace(&mut self, config: ShellConfig) -> bool {
        self.update(|c| *c = config)
    }

    /// Atomically persists the config if it is dirty (write to a temp file in
    /// the same directory, fsync, rename).
    pub fn flush(&mut self) -> Result<()> { self.flush_with(|| Ok(())) }

    fn flush_with(&mut self, before_publish: impl FnOnce() -> Result<()>) -> Result<()> {
        self.flush_using(before_publish, publication::sync_directory)
    }

    fn flush_using(&mut self, before_publish: impl FnOnce() -> Result<()>, mut sync_directory: impl FnMut(&Path) -> std::io::Result<()>) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(error) = &self.load_error {
            anyhow::bail!("refusing to replace invalid config {}: {error}", self.path.display());
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        // Lock a stable sibling: atomic replacement changes the config's inode.
        // Every process holds this through the read, merge and rename.
        let target = symlink_target(&self.path);
        let target = target.canonicalize().unwrap_or(target);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let directory = mandara_theme::templates::WriteLock::new(&target)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(target.with_extension("toml.lock"))?;
        lock.lock().context("locking configuration")?;
        let before = mandara_theme::templates::FileStamp::at(directory.target())?;
        // Another process may have saved since the watcher's last event.
        // Merge before writing so Studio cannot undo a shell-side change.
        if target.exists() {
            let bytes = fs::read(&target).with_context(||format!("reading {}",target.display()))?;
            self.reload_bytes(&bytes, |_| publication::confirm(&directory, &bytes, &mut sync_directory))?;
        }
        if !self.dirty {
            return Ok(());
        }
        let body = toml::to_string_pretty(&*self.current).context("serializing config")?;
        let contents = format!("{HEADER}{body}");
        before_publish()?;
        write_atomic_checked_using(&directory, before, contents.as_bytes(), sync_directory)?;
        self.disk_hash = Some(hash_bytes(contents.as_bytes()));
        self.saved = self.current.clone();
        self.dirty = false;
        Ok(())
    }

    /// Re-reads the file. Our own writes (same hash) are reported as
    /// unchanged. Parse errors leave the current config in place.
    pub fn reload_from_disk(&mut self) -> Result<ReloadOutcome> {
        self.reload_using(publication::sync_directory)
    }

    fn reload_using(&mut self, sync_directory: impl FnOnce(&Path) -> std::io::Result<()>) -> Result<ReloadOutcome> {
        let bytes = fs::read(&self.path).with_context(|| format!("reading {}", self.path.display()))?;
        self.reload_bytes(&bytes, |path| {
            let lock = mandara_theme::templates::WriteLock::new(path)?;
            publication::confirm(&lock, &bytes, sync_directory)
        })
    }

    fn reload_bytes(&mut self,bytes: &[u8], confirm_saved: impl FnOnce(&Path) -> Result<()>) -> Result<ReloadOutcome> {
        let hash = hash_bytes(bytes);
        if Some(hash) == self.disk_hash {
            if self.dirty && self.current == self.saved {
                confirm_saved(&self.path)?;
                self.dirty = false;
            }
            return Ok(ReloadOutcome::Unchanged);
        }
        let cfg = Arc::new(parse(bytes)?);
        let next = if self.dirty && self.load_error.is_none() {
            Arc::new(crate::patch::ConfigPatch::between(&self.saved, &self.current)?.apply(&cfg)?)
        } else {
            cfg.clone()
        };
        // A previous rename may have published these edits before its sync failed.
        if self.dirty && next == cfg { confirm_saved(&self.path)?; }
        self.load_error = None;
        self.disk_hash = Some(hash);
        self.dirty = next != cfg;
        self.saved = cfg;
        if next == self.current {
            return Ok(ReloadOutcome::Unchanged);
        }
        self.current = next;
        Ok(ReloadOutcome::Changed)
    }

    /// Starts watching the config file. Each relevant filesystem event sends
    /// `()` on the returned channel (debounced by `debounce`); the receiver
    /// should call [`ConfigStore::reload_from_disk`].
    pub fn watch(&self, debounce: Duration) -> Result<ConfigWatcher> {
        ConfigWatcher::new(&self.path, debounce)
    }
}

fn parse(bytes: &[u8]) -> Result<ShellConfig> {
    let text = std::str::from_utf8(bytes).context("config is not valid UTF-8")?;
    let mut cfg: ShellConfig = toml::from_str(text).context("parsing config TOML")?;
    cfg.migrate_legacy();
    crate::desktop_widgets::migrate(&mut cfg);
    crate::upgrade::upgrade(&mut cfg);
    cfg.integrations.app_themes.validate()?;
    Ok(cfg)
}

/// Writes `contents` to `path` atomically, creating parent directories.
///
/// A symlinked file (stow, home-manager, a dotfiles repo) is written
/// through: its target gets the new contents and the link stays. The
/// file's permissions are kept (`mimeapps.list` at 0600 stays 0600).
pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let target = symlink_target(path);
    let lock = mandara_theme::templates::WriteLock::new(&target)?;
    write_atomic_locked(&lock, contents).map(|_| ())
}

/// Publish under an existing read-modify-write lock and return the written inode.
/// The receipt comes from the temporary file, before another writer can replace it.
pub fn write_atomic_locked(lock: &mandara_theme::templates::WriteLock, contents: &[u8]) -> Result<mandara_theme::templates::FileStamp> {
    let before = mandara_theme::templates::FileStamp::at(lock.target())?;
    write_atomic_checked(lock, before, contents)
}

/// Refuse a stale merge, including replacement with identical bytes.
pub fn write_atomic_checked(lock: &mandara_theme::templates::WriteLock, before: Option<mandara_theme::templates::FileStamp>, contents: &[u8]) -> Result<mandara_theme::templates::FileStamp> {
    write_atomic_checked_using(lock, before, contents, publication::sync_directory)
}

fn write_atomic_checked_using(lock: &mandara_theme::templates::WriteLock, before: Option<mandara_theme::templates::FileStamp>, contents: &[u8], sync_directory: impl FnOnce(&Path) -> std::io::Result<()>) -> Result<mandara_theme::templates::FileStamp> {
    lock.check_stamp(before)?;
    let path = lock.target();
    let permissions = fs::metadata(path).ok().map(|metadata| metadata.permissions());
    let dir = path.parent().context("config path has no parent directory")?;
    let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or("config");
    static NEXT_WRITE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let revision = NEXT_WRITE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let tmp = dir.join(format!(".{file_name}.{}.{stamp}.{revision}.tmp", std::process::id()));
    let mut temporary_created = false;
    let result = (|| {
        let mut file = private_temporary(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        temporary_created = true;
        file.write_all(contents)?;
        if let Some(permissions) = permissions { file.set_permissions(permissions)?; }
        file.sync_all()?;
        let written = mandara_theme::templates::FileStamp::of(&file)?;
        lock.check_stamp(before)?;
        fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
        temporary_created = false;
        sync_directory(dir).map_err(|error| PublishedWriteError::new(path, written, error)).with_context(|| format!("syncing {}", dir.display()))?;
        Ok(written)
    })();
    if result.is_err() && temporary_created { fs::remove_file(&tmp).with_context(|| format!("removing incomplete write {}", tmp.display()))?; }
    result
}

/// Unpublished config bytes stay private before final permissions are restored.
fn private_temporary(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
}

/// The file a chain of symlinks at `path` ends at (even when it doesn't
/// exist yet); `path` itself when it isn't a link.
pub(crate) fn symlink_target(path: &Path) -> std::path::PathBuf {
    let mut current = path.to_path_buf();
    // A cycle never ends: give up after as many hops as Linux allows.
    for _ in 0..MAX_SYMLINK_HOPS {
        let Ok(meta) = fs::symlink_metadata(&current) else { break;
        };
        if !meta.file_type().is_symlink() {
            break;
        }
        let Ok(next) = fs::read_link(&current) else { break;
        };
        current = if next.is_absolute() { next } else { current.parent().map(|d| d.join(&next)).unwrap_or(next) };
    }
    current
}

/// Linux's `MAXSYMLINKS`.
const MAX_SYMLINK_HOPS: usize = 40;

/// Keeps a `notify` watcher alive and exposes debounced change pings.
///
/// No thread waits between changes: the first event of a burst starts a
/// short-lived timer thread that fires once the burst has been quiet for
/// the debounce interval, then exits.
pub struct ConfigWatcher {
    watcher: RecommendedWatcher,
    target: PathBuf,
    watched: Vec<PathBuf>,
    names: Arc<std::sync::Mutex<Vec<PathBuf>>>,
    rx: async_channel::Receiver<()>,
}

/// Debounce state shared by the notify callback and a burst's timer.
struct Burst {
    /// A timer thread is running for the current burst.
    pending: std::sync::atomic::AtomicBool,
    /// When the latest event of the burst arrived.
    last: std::sync::Mutex<std::time::Instant>,
}

impl ConfigWatcher {
    fn new(path: &Path, debounce: Duration) -> Result<Self> {
        use std::sync::atomic::Ordering;
        let (tx, rx) = async_channel::bounded::<()>(1);
        let target = path.to_path_buf();
        let names = Arc::new(std::sync::Mutex::new(Vec::<PathBuf>::new()));
        let matching = names.clone();
        let burst = std::sync::Arc::new(Burst { pending: false.into(), last: std::sync::Mutex::new(std::time::Instant::now()) });

        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)) {
                return;
            }
            // Editors and our own atomic writes replace the file via rename,
            // so watch the directory and match on file name.
            if !event.paths.iter().any(|path| {
                matching
                    .lock().unwrap_or_else(|error| error.into_inner())
                    .contains(path)
            }) {
                return;
            }
            *burst.last.lock().unwrap_or_else(|e| e.into_inner()) = std::time::Instant::now();
            if burst.pending.swap(true, Ordering::AcqRel) {
                return;
            }
            let (timer_burst, timer_tx) = (burst.clone(), tx.clone());
            let spawned = std::thread::Builder::new().name("mandara-config-debounce".into()).spawn(move || {
                let (burst, tx) = (timer_burst, timer_tx);
                // Coalesce bursts (editors write several events per save).
                loop {
                    std::thread::sleep(debounce);
                    let quiet = burst.last.lock().unwrap_or_else(|e| e.into_inner()).elapsed() >= debounce;
                    if quiet {
                        break;
                    }
                }
                burst.pending.store(false, Ordering::Release);
                let _ = tx.try_send(());
            });
            if spawned.is_err() {
                burst.pending.store(false, Ordering::Release);
                let _ = tx.try_send(());
            }
        })?;
        let mut result = Self {
            watcher,
            target,
            watched: Vec::new(),
            names,
            rx,
        };
        result.refresh()?;
        Ok(result)
    }

    /// Follow retargeted dotfile links as well as atomic writes to their targets.
    pub fn refresh(&mut self) -> Result<()> {
        self.refresh_with(|watcher, path| watcher.unwatch(path).map_err(Into::into))
    }

    fn refresh_with(&mut self, mut unwatch: impl FnMut(&mut notify::RecommendedWatcher, &Path) -> Result<()>) -> Result<()> {
        let mut paths = Vec::new();
        let mut path = self.target.clone();
        for _ in 0..MAX_SYMLINK_HOPS {
            paths.push(path.clone());
            let Ok(next) = fs::read_link(&path) else {
                break;
            };
            path = if next.is_absolute() {
                next
            } else {
                path.parent()
                    .context("config link has no parent")?
                    .join(next)
            };
        }
        let mut parents = Vec::new();
        for path in &paths {
            let parent = path
                .parent().context("config path has no parent")?
                .to_path_buf();
            if !parents.contains(&parent) {
                parents.push(parent);
            }
        }
        for path in &parents {
            if !self.watched.contains(path) {
                self.watcher.watch(path, RecursiveMode::NonRecursive)?;
            }
        }
        for path in &self.watched {
            if !parents.contains(path)
                && let Err(error) = unwatch(&mut self.watcher, path)
            {
                tracing::debug!("unwatching {}: {error:#}", path.display());
            }
        }
        *self.names.lock().unwrap_or_else(|error| error.into_inner()) = paths;
        self.watched = parents;
        Ok(())
    }

    pub fn receiver(&self) -> async_channel::Receiver<()> {
        self.rx.clone()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn unpublished_configuration_is_never_group_or_world_readable() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("private.tmp");
        let temporary = super::private_temporary(&path).unwrap();
        assert_eq!(temporary.metadata().unwrap().permissions().mode() & 0o077, 0);
        assert!(super::private_temporary(&path).is_err(), "an existing temporary is never followed or truncated");
    }

    #[test]
    fn a_file_changed_after_merging_is_kept_then_merged_on_retry() {
        let root = tempfile::tempdir().unwrap(); let path = root.path().join("config.toml");
        let mut store = ConfigStore::open(&path).unwrap();
        store.update(|config| config.bar.height = 30.0); store.flush().unwrap();
        store.update(|config| config.bar.height = 32.0);
        let mut edited = store.get().clone(); edited.bar.height = 34.0; edited.compositor.manage = !edited.compositor.manage;
        let bytes = toml::to_string_pretty(&edited).unwrap();
        assert!(store.flush_with(|| fs::write(&path, &bytes).map_err(Into::into)).is_err());
        assert_eq!(ConfigStore::open(&path).unwrap().get().bar.height, 34.0);
        assert!(store.is_dirty()); store.flush().unwrap();
        let saved = ConfigStore::open(&path).unwrap();
        assert_eq!(saved.get().bar.height, 32.0);
        assert_eq!(saved.get().compositor.manage, edited.compositor.manage);
    }

    use super::*;
    use crate::schema::BarLayout;
    use mandara_theme::ThemeSource;

    /// Dotfile managers symlink files like `mimeapps.list`: a write goes to
    /// the link's target, the link stays, and the mode is kept.
    #[test]
    fn atomic_writes_follow_symlinks_and_keep_permissions() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("dotfiles");
        fs::create_dir_all(&repo).unwrap();
        let real = repo.join("mimeapps.list");
        fs::write(&real, "[Default Applications]\n").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        let config = dir.path().join("config");
        fs::create_dir_all(&config).unwrap();
        let link = config.join("mimeapps.list");
        std::os::unix::fs::symlink("../dotfiles/mimeapps.list", &link).unwrap();
        write_atomic(&link, b"[Default Applications]\ntext/plain=gedit.desktop\n").unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link survives");
        assert!(fs::read_to_string(&real).unwrap().contains("gedit"), "the target has the new contents");
        assert_eq!(fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o600);
        // A dangling link creates its target; a plain new file still works.
        let dangling = config.join("new.list");
        std::os::unix::fs::symlink(repo.join("new.list"), &dangling).unwrap();
        write_atomic(&dangling, b"x").unwrap();
        assert_eq!(fs::read_to_string(repo.join("new.list")).unwrap(), "x");
        let plain = config.join("sub/plain.toml");
        write_atomic(&plain, b"y").unwrap();
        assert_eq!(fs::read_to_string(&plain).unwrap(), "y");
        // A link cycle doesn't hang.
        let (a, b) = (config.join("a"), config.join("b"));
        std::os::unix::fs::symlink(&b, &a).unwrap();
        std::os::unix::fs::symlink(&a, &b).unwrap();
        let _ = write_atomic(&a, b"z");
    }

    #[test]
    fn a_queued_save_merges_into_a_retargeted_dotfile_without_touching_its_old_target() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.toml");
        let new = dir.path().join("new.toml");
        ConfigStore::open(&old).unwrap();
        let mut other = ConfigStore::open(&new).unwrap();
        other.update(|config|config.bar.height = 37.0);other.flush().unwrap();
        let link = dir.path().join("config.toml");
        std::os::unix::fs::symlink(&old,&link).unwrap();
        let mut store = ConfigStore::open(&link).unwrap();
        store.update(|config|config.bar.opacity = 0.7);
        let request = store.save_request().unwrap();
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&new,&link).unwrap();
        request.write().unwrap();
        assert_eq!(ConfigStore::open(&new).unwrap().get().bar.height,37.0);
        assert_eq!(ConfigStore::open(&new).unwrap().get().bar.opacity,0.7);
        assert_ne!(ConfigStore::open(&old).unwrap().get().bar.opacity,0.7);
        assert_eq!(std::fs::read_link(link).unwrap(),new);
    }

    #[test]
    fn creates_defaults_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.toml");
        let store = ConfigStore::open(&path).unwrap();
        assert!(path.exists());
        // A new config starts with the depth clock as a desktop widget.
        let mut fresh = ShellConfig::default();
        crate::desktop_widgets::migrate(&mut fresh);
        assert_eq!(*store.get(), fresh);
        let reopened = ConfigStore::open(&path).unwrap();
        assert_eq!(*reopened.get(), fresh);
    }

    #[test]
    fn update_is_in_memory_until_flush() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut store = ConfigStore::open(&path).unwrap();
        assert!(store.update(|c| c.bar.layout = BarLayout::ContiguousIsland));
        assert!(store.is_dirty());
        assert!(!store.update(|c| c.bar.layout = BarLayout::ContiguousIsland), "no-op update reports unchanged");
        assert_eq!(ConfigStore::open(&path).unwrap().get().bar.layout, BarLayout::MinimalEdge);
        store.flush().unwrap();
        assert!(!store.is_dirty());
        assert_eq!(ConfigStore::open(&path).unwrap().get().bar.layout, BarLayout::ContiguousIsland);
    }

    #[test]
    fn flushing_before_restart_preserves_the_whole_preference_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut store = ConfigStore::open(&path).unwrap();
        store.update(|cfg| {
            cfg.theme.mode = crate::ThemeMode::Light;
            cfg.theme.source = ThemeSource::Preset { preset: "pharos-night".into() };
            cfg.theme.adjustments.hue_shift = 11.0;
            cfg.bar.height = 37.0;
            cfg.bar.left = vec!["workspaces".into(), "launcher".into()];
            cfg.sidebar.width = 431.0;
            cfg.sidebar.look.card_padding = 19.0;
            cfg.side_bars.enabled = true;
            cfg.typography.studio_font = "JetBrains Mono".into();
            cfg.typography.scale = 1.15;
            cfg.plugins.enabled = vec!["notes".into(), "todo".into()];
            cfg.plugins.disabled = vec!["weather".into()];
            cfg.plugins.settings.insert("notes".into(), [("text".into(), toml::Value::String("Keep this preference".into()))].into_iter().collect());
            cfg.audio.overamplify = false;
            cfg.notifications.compact = true;
            let place = &mut cfg.desktop.widgets.items[0].place;
            place.x = -0.13;
            place.y = -0.23;
            place.scale = 0.73;
            place.rotation = 13.0;
            // The legacy clock mirrors its widget geometry when loaded.
            crate::desktop_widgets::migrate(cfg);
        });
        let expected = store.snapshot();
        assert_ne!(ConfigStore::open(&path).unwrap().get(), expected.as_ref());
        store.flush().unwrap();
        assert_eq!(ConfigStore::open(&path).unwrap().get(), expected.as_ref());
        assert!(!store.is_dirty());
    }

    #[test]
    fn self_writes_are_ignored_and_external_edits_apply() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut store = ConfigStore::open(&path).unwrap();
        assert_eq!(store.reload_from_disk().unwrap(), ReloadOutcome::Unchanged);

        fs::write(
            &path,
            "[theme.source]\nkind = \"preset\"\npreset = \"montaza-twilight\"\n\n[motion]\nstiffness = 400.0\n",
        )
        .unwrap();
        assert_eq!(store.reload_from_disk().unwrap(), ReloadOutcome::Changed);
        assert_eq!(store.get().theme.source, ThemeSource::Preset { preset: "montaza-twilight".into() });
        assert_eq!(store.get().motion.stiffness, 400.0);
        // Untouched sections keep defaults.
        assert_eq!(store.get().motion.damping, 24.0);
        assert_eq!(store.get().bar.center, vec!["clock".to_string(), "media".to_string()]);
    }

    #[test]
    fn an_older_shell_save_cannot_undo_pending_studio_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut shell = ConfigStore::open(&path).unwrap();
        let mut studio = ConfigStore::open(&path).unwrap();
        shell.update(|c| c.bar.height = 35.0);
        studio.update(|c| c.bar.height = 41.0);
        shell.flush().unwrap();
        studio.reload_from_disk().unwrap();
        assert_eq!(studio.get().bar.height, 41.0);
        assert!(studio.is_dirty(), "the latest edit still needs saving");
        studio.flush().unwrap();
        assert_eq!(ConfigStore::open(&path).unwrap().get().bar.height, 41.0);
    }

    #[test]
    fn a_flush_merges_display_changes_even_before_the_file_watcher_fires() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut shell = ConfigStore::open(&path).unwrap();
        let mut studio = ConfigStore::open(&path).unwrap();
        studio.update(|c| c.desktop.widgets.items.clear());
        shell.update(|c| {
            c.compositor.outputs.push(crate::outputs::OutputConfig {
                name: "DP-1".into(),
                scale: Some(1.25),
                ..Default::default()
            })
        });
        shell.flush().unwrap();
        studio.flush().unwrap();
        let reopened = ConfigStore::open(&path).unwrap();
        assert!(reopened.get().desktop.widgets.items.is_empty());
        assert_eq!(reopened.get().compositor.outputs[0].scale, Some(1.25));
    }

    #[test]
    fn simultaneous_writers_merge_under_one_file_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut first = ConfigStore::open(&path).unwrap();
        let mut second = ConfigStore::open(&path).unwrap();
        first.update(|config| config.bar.height = 41.0);
        second.update(|config| config.motion.mass = 2.0);
        let barrier = Arc::new(std::sync::Barrier::new(2));
        std::thread::scope(|scope| {
            let other = barrier.clone();
            scope.spawn(move || {
                other.wait();
                first.flush().unwrap();
            });
            scope.spawn(move || {
                barrier.wait();
                second.flush().unwrap();
            });
        });
        let reopened = ConfigStore::open(path).unwrap();
        assert_eq!(reopened.get().bar.height, 41.0);
        assert_eq!(reopened.get().motion.mass, 2.0);
    }

    #[test]
    fn a_background_save_completion_preserves_later_edits_and_disk_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut studio = ConfigStore::open(&path).unwrap();
        let mut shell = ConfigStore::open(&path).unwrap();
        studio.update(|config| config.bar.height = 40.0);
        let queued = studio.save_request().unwrap();
        studio.update(|config| config.bar.height = 43.0);
        shell.update(|config| config.motion.mass = 2.0);
        shell.flush().unwrap();
        assert!(studio.finish_save(queued.write().unwrap()).unwrap());
        assert_eq!(studio.get().bar.height, 43.0);
        assert_eq!(studio.get().motion.mass, 2.0);
        assert!(studio.is_dirty());
        studio.flush().unwrap();
        assert_eq!(ConfigStore::open(path).unwrap().get(), studio.get());
    }

    #[test]
    fn broken_file_keeps_previous_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut store = ConfigStore::open(&path).unwrap();
        store.update(|c| c.motion.mass = 2.0);
        store.flush().unwrap();
        fs::write(&path, "this is = = not toml").unwrap();
        assert!(store.reload_from_disk().is_err());
        assert_eq!(store.get().motion.mass, 2.0);
        // Opening a broken file falls back to defaults without clobbering it.
        let mut reopened = ConfigStore::open(&path).unwrap();
        assert_eq!(*reopened.get(), ShellConfig::default());
        reopened.update(|c| c.theme.mode = crate::ThemeMode::Light);
        assert!(reopened.flush().is_err(), "an edit must not save defaults over the original preferences");
        assert_eq!(fs::read_to_string(&path).unwrap(), "this is = = not toml");
        fs::write(&path, "[theme]\nmode = \"light\"\n\n[motion]\nmass = 3.0\n").unwrap();
        reopened.reload_from_disk().unwrap();
        reopened.update(|c| c.bar.height = 37.0);
        reopened.flush().unwrap();
        let repaired = ConfigStore::open(&path).unwrap();
        assert_eq!(repaired.get().theme.mode, crate::ThemeMode::Light);
        assert_eq!(repaired.get().motion.mass, 3.0);
        assert_eq!(repaired.get().bar.height, 37.0);
    }

    #[test]
    fn background_reload_keeps_new_local_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut store = ConfigStore::open(&path).unwrap();
        let request = store.reload_request();
        let mut external = ConfigStore::open(&path).unwrap();
        external.update(|config| config.motion.mass = 3.0);
        external.flush().unwrap();
        let read = request.read().unwrap();
        store.update(|config| config.motion.stiffness = 220.0);
        assert_eq!(store.finish_reload(read).unwrap(), ReloadOutcome::Changed);
        assert_eq!(
            (store.get().motion.mass, store.get().motion.stiffness),
            (3.0, 220.0)
        );
        assert!(store.is_dirty());
    }

    #[test]
    fn watcher_follows_symlink_targets_and_retargeting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let target = first.join("prefs.toml");
        fs::write(&target, "").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let store = ConfigStore::open(&path).unwrap();
        let mut watcher = store.watch(Duration::from_millis(10)).unwrap();
        let rx = watcher.receiver();
        let changed = || {
            (0..100).any(|_| {
                std::thread::sleep(Duration::from_millis(10));
                rx.try_recv().is_ok()
            })
        };
        write_atomic(&target, b"[motion]\nmass = 3.0\n").unwrap();
        assert!(changed());
        fs::remove_file(&path).unwrap();
        let next = second.join("settings.toml");
        fs::write(&next, "").unwrap();
        std::os::unix::fs::symlink(&next, &path).unwrap();
        assert!(changed());
        watcher.refresh().unwrap();
        while rx.try_recv().is_ok() {}
        write_atomic(&next, b"[motion]\nmass = 4.0\n").unwrap();
        assert!(changed());
    }

    #[test]
    fn watcher_reports_external_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let store = ConfigStore::open(&path).unwrap();
        let watcher = store.watch(Duration::from_millis(30)).unwrap();
        let rx = watcher.receiver();
        std::thread::sleep(Duration::from_millis(50));
        fs::write(&path, "[motion]\nmass = 3.0\n").unwrap();
        let got = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(20));
            rx.try_recv().is_ok()
        });
        assert!(got, "watcher did not fire");
    }
    #[test]
    fn failed_unwatch_does_not_leave_retargeted_configuration_names_stale() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old");
        let next = root.path().join("next");
        fs::create_dir(&old).unwrap();
        fs::create_dir(&next).unwrap();
        let link = root.path().join("config");
        let original = old.join("config");
        let replacement = next.join("config");
        fs::write(&original, "personal").unwrap();
        fs::write(&replacement, "next").unwrap();
        std::os::unix::fs::symlink(&original, &link).unwrap();
        let mut watcher = ConfigWatcher::new(&link, Duration::ZERO).unwrap();
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&replacement, &link).unwrap();
        fs::remove_file(&original).unwrap();
        fs::remove_dir(&old).unwrap();
        let mut attempted = false;
        watcher.refresh_with(|_, path| {
            assert_eq!(path, old);
            attempted = true;
            anyhow::bail!("injected stale-watch failure")
        }).unwrap();
        assert!(attempted);
        assert!(watcher.watched.contains(&next));
        assert!(!watcher.watched.contains(&old));
        assert!(watcher.names.lock().unwrap().contains(&replacement));
        watcher.refresh().unwrap();
    }

}
