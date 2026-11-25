//! Visit-owned native notifications, including folders installed after opening.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use notify::Watcher as _;

use super::model::Roots;

/// Change counters the watcher bumps and the loop compares: one for the
/// app catalog (desktop files, icon-theme settings), one for usage.
#[derive(Default)]
pub(super) struct Revisions {
    catalog: AtomicU64,
    usage: AtomicU64,
    #[cfg(test)]
    catalog_events: std::sync::Mutex<std::collections::BTreeMap<PathBuf, u64>>,
}

impl Revisions {
    /// The current `(catalog, usage)` revisions.
    pub fn get(&self) -> (u64, u64) { (self.catalog.load(Ordering::Acquire), self.usage.load(Ordering::Acquire)) }
    /// Bumps the counters an event touched.
    fn changed(&self, catalog: bool, usage: bool) {
        if catalog { self.catalog.fetch_add(1, Ordering::AcqRel); }
        if usage { self.usage.fetch_add(1, Ordering::AcqRel); }
    }
}

/// The visit's native watcher and the directories it currently watches.
pub(super) struct Watch {
    watcher: notify::RecommendedWatcher,
    roots: Roots,
    watching: BTreeSet<PathBuf>,
    wake: Option<async_channel::Sender<()>>,
}

impl Watch {
    /// Setup, refreshing and teardown run on the blocking worker, never the
    /// UI. A capacity-one wake channel coalesces a package manager's burst.
    pub fn new(roots: Roots, revisions: Arc<Revisions>, wake: async_channel::Sender<()>) -> notify::Result<Self> {
        let filter = roots.clone();
        let shutdown = wake.clone();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if wake.is_closed() { return; }
            #[cfg(test)]
            let observed_paths = event.as_ref().ok().map(|event| event.paths.clone());
            let (catalog, usage) = match event {
                Ok(event) if event.kind.is_access() => return,
                Ok(event) => (
                    event.paths.iter().any(|path| filter.applications.iter().any(|root| root.starts_with(path) || path.starts_with(root)) || filter.preferences.iter().any(|file| file == path || file.starts_with(path))),
                    event.paths.iter().any(|path| path == &filter.usage || filter.usage.starts_with(path)),
                ),
                Err(error) => { tracing::warn!("launcher files: {error}"); (true, true) }
            };
            if catalog || usage {
                revisions.changed(catalog, usage);
                #[cfg(test)]
                if catalog && let Some(paths) = observed_paths {
                    let revision = revisions.get().0;
                    revisions.catalog_events.lock().unwrap().extend(paths.into_iter().map(|path| (path, revision)));
                }
                let _ = wake.try_send(());
            }
        })?;
        let mut watch = Self { watcher, roots, watching: BTreeSet::new(), wake: None };
        watch.refresh()?;
        // A failed setup must leave the page's initial preparation usable.
        watch.wake = Some(shutdown);
        Ok(watch)
    }

    /// Watch the nearest existing directory and its parent. Creation or
    /// replacement moves the watch down toward a previously missing root.
    pub fn refresh(&mut self) -> notify::Result<()> {
        let wanted: BTreeSet<_> = self.roots.applications.iter().map(PathBuf::as_path).chain(self.roots.usage.parent()).chain(self.roots.preferences.iter().filter_map(|path| path.parent())).filter_map(|root| {
            root.ancestors().find(|path| path.is_dir())
        }).flat_map(|root| std::iter::once(root.to_path_buf()).chain(root.parent().map(PathBuf::from))).collect();
        // Replacement directories keep their path but have a new native watch.
        for path in &wanted { self.watcher.watch(path, notify::RecursiveMode::NonRecursive)?; }
        for path in self.watching.difference(&wanted) {
            if let Err(error) = self.watcher.unwatch(path) && !matches!(&error.kind, notify::ErrorKind::WatchNotFound) { return Err(error); }
        }
        self.watching = wanted;
        Ok(())
    }
}

impl Drop for Watch {
    /// Closes the wake queue before the watcher goes (see below).
    fn drop(&mut self) {
        // notify's native drop only requests event-loop shutdown. Close our
        // queue now, while its callback may still own a sender on that thread.
        if let Some(wake) = &self.wake { wake.close(); }
    }
}

#[cfg(test)]
mod tests;
