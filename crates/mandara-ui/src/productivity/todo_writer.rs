//! Debounced, ordered JSON writes and receipts for desktop edits and commands.

use super::{Productivity, Saved, SAVE_DEBOUNCE, store_path};
use gpui::Context;

/// Receipts are bounded independently of the latest coalesced save snapshot.
const MAX_WAITERS: usize = 32;

#[derive(Default)]
pub(super) struct TodoWrites {
    revision: u64,
    saved: u64,
    inflight: bool,
    waiters: Vec<(u64, async_channel::Sender<Result<(), String>>)>,
}

impl TodoWrites {
    /// Multiple unsent changes advance one eventual snapshot, without retaining each book.
    fn changed(&mut self) { self.revision += 1; }
    /// A failed write keeps the newest revision eligible for an explicit retry.
    fn dirty(&self) -> bool { self.saved != self.revision }
    /// Marks executing IO so an explicit flush cannot cancel its task.
    fn begin(&mut self) -> u64 { self.inflight = true; self.revision }

    /// One bounded waiter covers all changes through the current revision.
    fn receipt(&mut self) -> Result<async_channel::Receiver<Result<(), String>>, String> {
        if self.waiters.len() >= MAX_WAITERS { return Err(mandara_i18n::t("Busy").to_owned()); }
        let (reply, result) = async_channel::bounded(1);
        if self.dirty() { self.waiters.push((self.revision, reply)); }
        else { let _ = reply.try_send(Ok(())); }
        Ok(result)
    }

    /// Failure answers every waiter; success acknowledges only included revisions.
    fn finish(&mut self, revision: u64, result: &Result<(), String>) -> bool {
        self.inflight = false;
        if result.is_ok() { self.saved = revision; }
        self.waiters.retain(|(wanted, reply)| {
            if *wanted <= revision || result.is_err() { let _ = reply.try_send(result.clone()); false } else { true }
        });
        result.is_ok() && self.dirty()
    }
}

impl Productivity {
    /// Updates the shared model immediately and schedules one coalesced disk write.
    pub(super) fn save(&mut self, cx: &mut Context<Self>) {
        self.todo_writes.changed();
        self.start_save(false, cx);
        cx.notify();
    }

    /// Explicit saves may accelerate a debounce, but never replace executing IO.
    fn start_save(&mut self, mut immediate: bool, cx: &mut Context<Self>) {
        if !self.ready { return; }
        if self.save_task.is_some() {
            if immediate && !self.todo_writes.inflight { self.save_task = None; }
            else { return; }
        }
        self.save_task = Some(cx.spawn(async move |this, cx| {
            loop {
                if !immediate { cx.background_executor().timer(SAVE_DEBOUNCE).await; }
                // One snapshot per dispatched write, rather than per keystroke.
                let Ok((revision, saved)) = this.update(cx, |p, _| (
                    p.todo_writes.begin(),
                    Saved { lists: p.book.lists.clone(), todos: p.book.todos.clone(), notes: p.legacy_notes.clone(), filter: p.filter },
                )) else { break };
                let result = mandara_services::runtime::unblock(move || persist(saved)).await;
                let again = this.update(cx, |p, cx| {
                    p.todo_error = result.as_ref().err().cloned();
                    cx.notify();
                    let again = p.todo_writes.finish(revision, &result);
                    immediate = !p.todo_writes.waiters.is_empty();
                    if let Err(error) = &result { tracing::warn!("saving to-dos: {error}"); }
                    if !again { p.save_task = None; }
                    again
                }).unwrap_or(false);
                if !again { break; }
            }
        }));
    }

    /// Retries the retained dirty snapshot without replaying its user actions.
    pub(crate) fn retry_todos(&mut self, cx: &mut Context<Self>) {
        if self.todo_writes.dirty() { self.start_save(true, cx); }
    }

    /// Waits for the current revision's actual atomic write; failures are retryable.
    pub(crate) fn persist_todos(&mut self, cx: &mut Context<Self>) -> Result<async_channel::Receiver<Result<(), String>>, String> {
        if !self.ready { return Err(mandara_i18n::t("Loading…").to_owned()); }
        let result = self.todo_writes.receipt()?;
        if self.todo_writes.dirty() { self.start_save(true, cx); }
        Ok(result)
    }
}

/// Serialization and filesystem calls execute together outside the UI thread.
fn persist(saved: Saved) -> Result<(), String> {
    let path = store_path();
    persist_to(&path, &saved).map_err(|error| format!("{error:#}"))
}

/// A missing file is a new book; unreadable or malformed existing data is an error.
pub(super) fn load_saved(path: &std::path::Path) -> anyhow::Result<Saved> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Saved::default()),
        Err(error) => Err(error.into()),
    }
}

/// Keeps the previous JSON intact until the complete replacement can be renamed.
fn persist_to(path: &std::path::Path, saved: &Saved) -> anyhow::Result<()> {
    let json = serde_json::to_vec_pretty(saved)?;
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    mandara_config::write_atomic(path, &json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_revisions_are_coalesced_without_acknowledging_them_before_their_write() {
        let mut writes = TodoWrites::default();
        writes.changed();
        let first = writes.receipt().unwrap();
        let dispatched = writes.revision;
        for _ in 0..20 { writes.changed(); }
        let latest = writes.receipt().unwrap();
        assert!(writes.finish(dispatched, &Ok(())));
        assert_eq!(first.try_recv(), Ok(Ok(())));
        assert!(latest.try_recv().is_err());
        assert!(!writes.finish(writes.revision, &Ok(())));
        assert_eq!(latest.try_recv(), Ok(Ok(())));
        assert!(!writes.dirty());
    }

    #[test]
    fn failure_answers_all_pending_receipts_and_keeps_the_latest_revision_retryable() {
        let mut writes = TodoWrites::default();
        writes.changed();
        let first = writes.receipt().unwrap();
        let dispatched = writes.revision;
        writes.changed();
        let newer = writes.receipt().unwrap();
        let error = Err("disk unavailable".to_owned());
        assert!(!writes.finish(dispatched, &error));
        assert_eq!(first.try_recv(), Ok(error.clone()));
        assert_eq!(newer.try_recv(), Ok(error));
        assert!(writes.dirty());
        let retry = writes.receipt().unwrap();
        assert!(retry.try_recv().is_err());
        assert!(!writes.finish(writes.revision, &Ok(())));
        assert_eq!(retry.try_recv(), Ok(Ok(())));
    }

    #[test]
    fn pending_receipts_are_bounded_while_unsent_mutations_only_advance_one_revision() {
        let mut writes = TodoWrites::default();
        writes.changed();
        let receipts: Vec<_> = (0..MAX_WAITERS).map(|_| writes.receipt().unwrap()).collect();
        assert!(writes.receipt().is_err());
        for _ in 0..100 { writes.changed(); }
        assert_eq!(writes.waiters.len(), MAX_WAITERS);
        assert!(!writes.finish(writes.revision, &Ok(())));
        for receipt in receipts { assert_eq!(receipt.try_recv(), Ok(Ok(()))); }
    }

    #[test]
    fn an_executing_write_stays_owned_until_completion_even_if_a_new_receipt_arrives() {
        let mut writes = TodoWrites::default();
        writes.changed();
        let revision = writes.begin();
        assert!(writes.inflight);
        writes.changed();
        let newer = writes.receipt().unwrap();
        assert!(writes.inflight, "an explicit save may cancel a debounce, never executing IO");
        assert!(writes.finish(revision, &Ok(())));
        assert!(!writes.inflight);
        assert!(newer.try_recv().is_err());
        let revision = writes.begin();
        assert!(!writes.finish(revision, &Ok(())));
        assert_eq!(newer.try_recv(), Ok(Ok(())));
    }

    #[test]
    fn failed_atomic_write_can_be_retried_without_changing_the_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("blocked");
        std::fs::write(&parent, b"not a directory").unwrap();
        let path = parent.join("productivity.json");
        let saved = Saved::default();
        assert!(persist_to(&path, &saved).is_err());
        std::fs::remove_file(parent).unwrap();
        persist_to(&path, &saved).unwrap();
        let roundtrip: Saved = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(roundtrip.filter, saved.filter);
        assert!(roundtrip.todos.is_empty());
    }

    #[test]
    fn invalid_existing_json_is_reported_instead_of_becoming_an_empty_book() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("productivity.json");
        let bytes = b"{unfinished user data";
        std::fs::write(&path, bytes).unwrap();
        assert!(load_saved(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::remove_file(&path).unwrap();
        assert!(load_saved(&path).unwrap().todos.is_empty());
    }
}
