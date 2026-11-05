//! One application catalog shared by docks, refreshed by filesystem events.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use notify::Watcher as _;

/// Package managers replace several files together; scan after that burst.
const SETTLE: Duration = Duration::from_millis(100);

struct Watch {
    watcher: notify::RecommendedWatcher,
    roots: Vec<PathBuf>,
    watching: BTreeSet<PathBuf>,
}

impl Watch {
    fn new(roots: Vec<PathBuf>) -> notify::Result<(Self, async_channel::Receiver<()>)> {
        let (tx, rx) = async_channel::bounded(1);
        let filter = roots.clone();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if event.is_ok_and(|e| !e.kind.is_access() && e.paths.iter().any(|path| filter.iter().any(|root| root.starts_with(path) || path.starts_with(root)))) {
                let _ = tx.try_send(());
            }
        })?;
        let mut watch = Self { watcher, roots, watching: BTreeSet::new() };
        watch.refresh()?;
        Ok((watch, rx))
    }

    fn refresh(&mut self) -> notify::Result<()> {
        let wanted: BTreeSet<_> = self.roots.iter().filter_map(|root| root.ancestors().find(|p| p.is_dir())).flat_map(|root| {
            std::iter::once(root.to_path_buf()).chain(root.parent().map(PathBuf::from))
        }).collect();
        for path in wanted.difference(&self.watching) { self.watcher.watch(path, notify::RecursiveMode::NonRecursive)?; }
        for path in self.watching.difference(&wanted) { let _ = self.watcher.unwatch(path); }
        self.watching = wanted;
        Ok(())
    }
}

/// Native directory notifications; no periodic application scan.
pub struct Catalog {
    pub apps: Rc<[super::App]>,
    _changes: Option<Task<()>>,
}

#[derive(Clone)]
struct Shared(Entity<Catalog>);
impl Global for Shared {}

pub fn init(cx: &mut App) -> Entity<Catalog> {
    if let Some(shared) = cx.try_global::<Shared>() { return shared.0.clone(); }
    let catalog = cx.new(Catalog::new);
    cx.set_global(Shared(catalog.clone()));
    catalog
}

impl Catalog {
    fn new(cx: &mut Context<Self>) -> Self {
        let roots = super::data_dirs().into_iter().map(|root| root.join("applications")).collect();
        let changes = match Watch::new(roots) {
            Ok((watch, rx)) => {
                let watch = Arc::new(Mutex::new(watch));
                Some(cx.spawn(async move |this, cx| {
                    while rx.recv().await.is_ok() {
                        cx.background_executor().timer(SETTLE).await;
                        while rx.try_recv().is_ok() {}
                        let watch = watch.clone();
                        let apps = cx.background_spawn(async move {
                            if let Err(error) = watch.lock().unwrap_or_else(|e| e.into_inner()).refresh() { tracing::warn!("watching application folders: {error}"); }
                            super::refresh_entry_index();
                            crate::icon_theme::refresh_files();
                            super::discover()
                        }).await;
                        if this.update(cx, |this, cx| { this.apps = apps.into(); cx.notify(); }).is_err() { break; }
                    }
                }))
            }
            Err(error) => { tracing::warn!("watching application folders: {error}"); None }
        };
        Self { apps: super::discover().into(), _changes: changes }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// A missing notification must fail the fixture rather than hang it.
    const TIMEOUT: Duration = Duration::from_secs(3);
    const WAIT: Duration = Duration::from_millis(10);

    fn receive(rx: &async_channel::Receiver<()>) {
        let deadline = std::time::Instant::now() + TIMEOUT;
        while rx.try_recv().is_err() {
            assert!(std::time::Instant::now() < deadline, "application change was not reported");
            std::thread::sleep(WAIT);
        }
    }

    #[test]
    fn installations_and_removals_refresh_a_previously_missing_app_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("exports/share/applications");
        let (mut watch, rx) = Watch::new(vec![root.clone()]).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        receive(&rx);
        watch.refresh().unwrap();
        while rx.try_recv().is_ok() {}
        let entry = root.join("new-app.desktop");
        std::fs::write(&entry, "[Desktop Entry]\nName=New app\nExec=new-app\n").unwrap();
        receive(&rx);
        assert!(super::super::discover_in(&[dir.path().join("exports/share")]).iter().any(|a| a.id == "new-app"));
        while rx.try_recv().is_ok() {}
        std::fs::remove_file(entry).unwrap();
        receive(&rx);
        assert!(super::super::discover_in(&[dir.path().join("exports/share")]).is_empty());
    }
}
