//! Bounded change notifications for plugin folders, including recreated roots.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt as _;
use std::path::PathBuf;

use notify::Watcher as _;

pub(super) struct Watch {
    watcher: notify::RecommendedWatcher,
    roots: Vec<PathBuf>,
    /// Device, inode and recursive mode all identify a watch registration.
    watched: HashMap<PathBuf, (u64, u64, bool)>,
    active: HashMap<PathBuf, (u64, u64, bool)>,
}

impl Watch {
    /// Called on the blocking worker; absent roots use their nearest existing parent.
    pub fn new(roots: Vec<PathBuf>) -> notify::Result<(Self, async_channel::Receiver<()>)> {
        let filter = roots.clone();
        let (tx, rx) = async_channel::bounded(1);
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if event.is_ok_and(|e| {
                !matches!(e.kind, notify::EventKind::Access(_))
                    && e.paths.iter().any(|p| {
                        filter
                            .iter()
                            .any(|root| p.starts_with(root) || root.starts_with(p))
                    })
            }) {
                let _ = tx.try_send(());
            }
        })?;
        let mut watch = Self {
            watcher,
            roots,
            watched: HashMap::new(),
            active: HashMap::new(),
        };
        watch.rearm();
        Ok((watch, rx))
    }

    /// Keep unchanged watches; replace one when its directory's inode changes.
    pub fn rearm(&mut self) {
        self.active.clear();
        for root in &self.roots {
            let Some((path, metadata)) = root.ancestors().find_map(|path| {
                path.metadata()
                    .ok()
                    .filter(|m| m.is_dir())
                    .map(|m| (path, m))
            }) else {
                continue;
            };
            let identity = (metadata.dev(), metadata.ino(), path == root);
            self.active
                .entry(path.to_path_buf())
                .and_modify(|known| known.2 |= identity.2)
                .or_insert(identity);
        }
        for (path, &identity) in &self.active {
            if self.watched.get(path) == Some(&identity) {
                continue;
            }
            if self.watched.remove(path).is_some() {
                let _ = self.watcher.unwatch(path);
            }
            let mode = if identity.2 {
                notify::RecursiveMode::Recursive
            } else {
                notify::RecursiveMode::NonRecursive
            };
            match self.watcher.watch(path, mode) {
                Ok(()) => {
                    self.watched.insert(path.to_path_buf(), identity);
                }
                Err(error) => tracing::warn!("watching {}: {error}", path.display()),
            }
        }
        self.watched.retain(|path, _| {
            if self.active.contains_key(path) {
                return true;
            }
            let _ = self.watcher.unwatch(path);
            false
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_recreated_folders_replace_the_parent_watch() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("plugins");
        let (mut watch, rx) = Watch::new(vec![root.clone()]).unwrap();
        assert!(watch.watched.contains_key(dir.path()));
        std::fs::create_dir(&root).unwrap();
        watch.rearm();
        assert!(watch.watched.contains_key(&root));
        assert!(!watch.watched.contains_key(dir.path()));
        std::fs::remove_dir(&root).unwrap();
        watch.rearm();
        assert!(watch.watched.contains_key(dir.path()));
        std::fs::create_dir(&root).unwrap();
        watch.rearm();
        let metadata = root.metadata().unwrap();
        assert_eq!(watch.watched[&root], (metadata.dev(), metadata.ino(), true));
        assert_eq!(watch.watched.len(), 1);
        while rx.try_recv().is_ok() {}
        for i in 0..100 {
            std::fs::write(root.join(format!("plugin-{i}")), b"changed").unwrap();
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while rx.is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(rx.len(), 1, "a burst keeps one wake-up");
        let before = watch.watched.clone();
        watch.rearm();
        assert_eq!(
            watch.watched, before,
            "unchanged roots retain their registrations"
        );
    }

    #[test]
    fn a_shared_ancestor_keeps_the_recursive_registration() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let (mut watch, _) = Watch::new(vec![root.join("absent"), root.clone()]).unwrap();
        assert!(watch.watched[&root].2);
        watch.rearm();
        assert!(watch.watched[&root].2);
    }
}
