//! Coalesced folder changes, including reconciliation after watcher errors.

use notify::Watcher as _;
use super::NoteStore;

impl NoteStore {
    /// The worker installs watches alongside the filesystem work they observe.
    pub fn watch_changes(&mut self, changed: async_channel::Sender<()>) -> anyhow::Result<()> {
        let mut watcher = notify::recommended_watcher(move |event| forward(event, &changed))?;
        watcher.watch(&self.view.root, notify::RecursiveMode::Recursive)?;
        self.watcher = Some(watcher);
        Ok(())
    }
}

fn forward(event: notify::Result<notify::Event>, changed: &async_channel::Sender<()>) {
    let needs_scan = match event {
        Ok(event) => !event.kind.is_access(),
        Err(error) => {
            tracing::warn!("watching notes: {error}");
            true
        }
    };
    if needs_scan { let _ = changed.try_send(()); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_failures_request_one_reconciliation_and_access_events_do_not_loop() {
        let (changed, pending) = async_channel::bounded(1);
        let access = notify::Event::new(notify::EventKind::Access(notify::event::AccessKind::Any));
        forward(Ok(access), &changed);
        assert!(pending.try_recv().is_err());
        for _ in 0..4 {
            forward(Err(std::io::Error::other("fixture watch error").into()), &changed);
        }
        assert_eq!(pending.len(), 1);
        assert_eq!(pending.try_recv(), Ok(()));
        assert!(pending.try_recv().is_err());
    }
}
