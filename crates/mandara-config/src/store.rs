//! Reactive TOML-backed configuration store.
//!
//! The store is the single in-memory source of truth: Studio sliders call
//! [`ConfigStore::update`] (cheap, no I/O) every frame while dragging, and a
//! debounced [`ConfigStore::flush`] persists atomically afterwards. External
//! edits are picked up through [`ConfigStore::watch`]; our own writes are
//! recognised by content hash and ignored.

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

pub struct ConfigStore {
    path: PathBuf,
    current: Arc<ShellConfig>,
    dirty: bool,
    /// Hash of the file bytes we last read or wrote.
    disk_hash: Option<u64>,
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
    /// (the broken file is left untouched so no user data is lost).
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let mut store = Self { path, current: Arc::new(ShellConfig::default()), dirty: false, disk_hash: None };
        if store.path.exists() {
            let bytes = fs::read(&store.path).with_context(|| format!("reading {}", store.path.display()))?;
            store.disk_hash = Some(hash_bytes(&bytes));
            match parse(&bytes) {
                Ok(cfg) => store.current = Arc::new(cfg),
                Err(err) => tracing::error!("{}: {err:#}; using defaults in memory", store.path.display()),
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
    pub fn flush(&mut self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let body = toml::to_string_pretty(&*self.current).context("serializing config")?;
        let contents = format!("{HEADER}{body}");
        write_atomic(&self.path, contents.as_bytes())?;
        self.disk_hash = Some(hash_bytes(contents.as_bytes()));
        self.dirty = false;
        Ok(())
    }

    /// Re-reads the file. Our own writes (same hash) are reported as
    /// unchanged. Parse errors leave the current config in place.
    pub fn reload_from_disk(&mut self) -> Result<ReloadOutcome> {
        let bytes = fs::read(&self.path).with_context(|| format!("reading {}", self.path.display()))?;
        let hash = hash_bytes(&bytes);
        if Some(hash) == self.disk_hash {
            return Ok(ReloadOutcome::Unchanged);
        }
        let cfg = parse(&bytes)?;
        self.disk_hash = Some(hash);
        if cfg == *self.current {
            return Ok(ReloadOutcome::Unchanged);
        }
        self.current = Arc::new(cfg);
        self.dirty = false;
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
    Ok(cfg)
}

/// Writes `contents` to `path` atomically, creating parent directories.
///
/// A symlinked file (stow, home-manager, a dotfiles repo) is written
/// through: its target gets the new contents and the link stays. The
/// file's permissions are kept (`mimeapps.list` at 0600 stays 0600).
pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let target = symlink_target(path);
    let path = target.as_path();
    let permissions = fs::metadata(path).ok().map(|m| m.permissions());
    let dir = path.parent().context("config path has no parent directory")?;
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("config");
    let tmp = dir.join(format!(".{file_name}.{}.tmp", std::process::id()));
    {
        let mut f = fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(contents)?;
        f.sync_all()?;
    }
    if let Some(p) = permissions {
        fs::set_permissions(&tmp, p).with_context(|| format!("copying permissions to {}", tmp.display()))?;
    }
    fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

/// The file a chain of symlinks at `path` ends at (even when it doesn't
/// exist yet); `path` itself when it isn't a link.
fn symlink_target(path: &Path) -> std::path::PathBuf {
    let mut current = path.to_path_buf();
    // A cycle never ends: give up after as many hops as Linux allows.
    for _ in 0..MAX_SYMLINK_HOPS {
        let Ok(meta) = fs::symlink_metadata(&current) else { break };
        if !meta.file_type().is_symlink() {
            break;
        }
        let Ok(next) = fs::read_link(&current) else { break };
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
    _watcher: RecommendedWatcher,
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
        let target_name = target.file_name().map(|n| n.to_owned());
        let burst = std::sync::Arc::new(Burst { pending: false.into(), last: std::sync::Mutex::new(std::time::Instant::now()) });

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)) {
                return;
            }
            // Editors and our own atomic writes replace the file via rename,
            // so watch the directory and match on file name.
            if !event.paths.iter().any(|p| p.file_name().map(|n| n.to_owned()) == target_name) {
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
        // Watch the parent directory: atomic renames replace the inode.
        let dir = target.parent().context("config path has no parent")?;
        watcher.watch(dir, RecursiveMode::NonRecursive)?;

        Ok(Self { _watcher: watcher, rx })
    }

    pub fn receiver(&self) -> async_channel::Receiver<()> {
        self.rx.clone()
    }
}

#[cfg(test)]
mod tests {
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
        let reopened = ConfigStore::open(&path).unwrap();
        assert_eq!(*reopened.get(), ShellConfig::default());
        assert_eq!(fs::read_to_string(&path).unwrap(), "this is = = not toml");
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
}
