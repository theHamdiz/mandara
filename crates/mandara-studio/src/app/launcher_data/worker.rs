//! One coalesced preparation loop; no blocking work runs on an async executor.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{BackgroundExecutor, FutureExt as _};
use mandara_services::runtime;
use mandara_ui::launcher_ext::Usage;

use super::model::{Key, Prepared, Roots, Snapshot, Source};
use super::watch::{Revisions, Watch};

/// Package transactions replace multiple desktop files and the usage writer
/// renames an atomic temporary file. One short settling period absorbs them.
const FILE_SETTLE: Duration = Duration::from_millis(100);

pub(super) struct NativeSource {
    roots: Roots,
}

impl NativeSource {
    pub fn new() -> Self {
        let state = dirs::state_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/state"));
        let preferences = dirs::config_dir().map(|root| ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini", "kdeglobals"].map(|path| root.join(path)).into()).unwrap_or_default();
        Self { roots: Roots { applications: mandara_ui::apps::application_dirs(), usage: state.join("mandara/launcher-usage.toml"), preferences } }
    }
}

impl Source for NativeSource {
    fn roots(&self) -> &Roots { &self.roots }
    fn catalog(&self, key: &Key) -> Vec<mandara_ui::apps::App> { mandara_ui::apps::discover_for_theme(key.language, key.icon_theme.as_deref()) }
    fn usage(&self) -> Usage { read_usage(&self.roots.usage) }
}

/// Preserve the existing absent/invalid-history behavior without doing its
/// read and TOML parse during element construction.
pub(super) fn read_usage(path: &std::path::Path) -> Usage {
    std::fs::read_to_string(path).ok().and_then(|text| toml::from_str(&text).ok()).unwrap_or_default()
}

pub(super) struct Lease {
    request: Arc<Mutex<Key>>,
    wake: async_channel::Sender<()>,
    revisions: Arc<Revisions>,
}

impl Lease {
    /// The borrowed comparison happens before cloning a changed request.
    pub fn request(&self, key: Key) {
        let mut current = self.request.lock().unwrap_or_else(|error| error.into_inner());
        if *current != key {
            *current = key;
            let _ = self.wake.try_send(());
        }
    }

    pub fn accepts(&self, snapshot: &Snapshot) -> bool {
        snapshot.key == *self.request.lock().unwrap_or_else(|error| error.into_inner()) && snapshot.revisions == self.revisions.get()
    }
}

impl Drop for Lease {
    fn drop(&mut self) { self.wake.close(); }
}

/// The service task owns its watcher. Closing the lease wakes it immediately;
/// any already-running scan finishes off-thread and its result is discarded.
/// The watcher and retained catalog are also destroyed on the blocking worker.
pub(super) fn start(key: Key, executor: BackgroundExecutor) -> (Lease, async_channel::Receiver<Arc<Snapshot>>) {
    let (wake, events) = async_channel::bounded(1);
    let (updates, receiver) = async_channel::bounded(1);
    let revisions = Arc::new(Revisions::default());
    let request = Arc::new(Mutex::new(key));
    let lease = Lease { request: request.clone(), wake: wake.clone(), revisions: revisions.clone() };
    runtime::spawn("studio launcher data", async move {
        let initial_revisions = revisions.clone();
        let initial_wake = wake.clone();
        let (mut prepared, mut watch) = runtime::unblock(move || {
            let source = Arc::new(NativeSource::new());
            let watch = match Watch::new(source.roots().clone(), initial_revisions, initial_wake) {
                Ok(watch) => Some(watch),
                Err(error) => { tracing::warn!("watching launcher data: {error}"); None }
            };
            (Prepared::new(source), watch)
        }).await;
        let mut watched = None;
        loop {
            if events.is_closed() { break; }
            let key = request.lock().unwrap_or_else(|error| error.into_inner()).clone();
            let revision = revisions.get();
            let now = chrono::Utc::now().timestamp();
            let refresh_watch = watched != Some(revision);
            let next_key = key.clone();
            let result = runtime::unblock(move || {
                if refresh_watch && let Some(watch) = &mut watch && let Err(error) = watch.refresh() { tracing::warn!("watching launcher data: {error}"); }
                let snapshot = prepared.snapshot(&next_key, revision, now);
                let deadline = prepared.next_refresh(&next_key, now);
                (prepared, watch, Arc::new(snapshot), deadline)
            }).await;
            let (next_prepared, next_watch, snapshot, deadline) = result;
            prepared = next_prepared;
            watch = next_watch;
            watched = Some(revision);
            if events.is_closed() { break; }
            if key != *request.lock().unwrap_or_else(|error| error.into_inner()) || revision != revisions.get() { continue; }
            if updates.send(snapshot).await.is_err() { break; }
            let received = match deadline {
                Some(deadline) => match events.recv().with_timeout(deadline, &executor).await { Ok(result) => result, Err(_) => continue },
                None => events.recv().await,
            };
            if received.is_err() { break; }
            if revision != revisions.get() { runtime::sleep(FILE_SETTLE).await; }
            while events.try_recv().is_ok() {}
        }
        runtime::unblock(move || drop((prepared, watch))).await;
    });
    (lease, receiver)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Replaceable setting requests occupy one wake slot; old settings are
    /// rejected and leaving closes the worker's wait even with no timer.
    #[test]
    fn requests_coalesce_reject_stale_settings_and_close_on_leave() {
        let (wake, events) = async_channel::bounded(1);
        let first = Key::new(mandara_i18n::Lang::En, 14.0, None);
        let lease = Lease { request: Arc::new(Mutex::new(first.clone())), wake, revisions: Arc::new(Revisions::default()) };
        for half_life in 1..100 { lease.request(Key::new(mandara_i18n::Lang::Ar, half_life as f32, None)); }
        assert_eq!(events.len(), 1);
        assert!(!lease.accepts(&Snapshot { key: first, revisions: (0, 0), apps: Arc::from([]), rows: Arc::from([]) }));
        assert!(lease.accepts(&Snapshot { key: Key::new(mandara_i18n::Lang::Ar, 99.0, None), revisions: (0, 0), apps: Arc::from([]), rows: Arc::from([]) }));
        drop(lease);
        assert!(events.is_closed());
    }
}
