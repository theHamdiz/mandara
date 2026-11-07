//! Debounced, ordered JSON writes and receipts for desktop edits and commands.

use super::{Productivity, Saved, SAVE_DEBOUNCE, store_path};
use gpui::Context;

/// Receipts are bounded independently of the latest coalesced save snapshot.
const MAX_WAITERS: usize = 32;

/// A reserved reply slot is bound to a revision only after its mutation succeeds.
pub(crate) struct Reservation {
    reply: async_channel::Sender<Result<(), String>>,
    result: async_channel::Receiver<Result<(), String>>,
}

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

    /// Reserve capacity before a command can change the shared book.
    fn reserve(&mut self) -> Result<Reservation, String> {
        if self.waiters.len() >= MAX_WAITERS { return Err(mandara_i18n::t("Busy").to_owned()); }
        let (reply, result) = async_channel::bounded(1);
        self.waiters.push((u64::MAX, reply.clone()));
        Ok(Reservation { reply, result })
    }

    /// Bind the already-owned slot to the actual resulting revision.
    fn bind(&mut self, reservation: Reservation) -> async_channel::Receiver<Result<(), String>> {
        if let Some(index) = self.waiters.iter().position(|(_, reply)| reply.same_channel(&reservation.reply)) {
            if self.dirty() { self.waiters[index].0 = self.revision; }
            else { let (_, reply) = self.waiters.remove(index); let _ = reply.try_send(Ok(())); }
        }
        reservation.result
    }

    /// A rejected mutation frees its slot without scheduling a write.
    fn cancel(&mut self, reservation: Reservation) {
        self.waiters.retain(|(_, reply)| !reply.same_channel(&reservation.reply));
    }

    /// One bounded waiter covers all changes through the current revision.
    #[cfg(test)]
    fn receipt(&mut self) -> Result<async_channel::Receiver<Result<(), String>>, String> {
        let reservation = self.reserve()?;
        Ok(self.bind(reservation))
    }

    /// Failure answers every waiter; success acknowledges only included revisions.
    fn finish(&mut self, revision: u64, result: &Result<(), String>) -> bool {
        self.inflight = false;
        if result.is_ok() { self.saved = revision; }
        self.waiters.retain(|(wanted, reply)| {
            if *wanted != u64::MAX && (*wanted <= revision || result.is_err()) { let _ = reply.try_send(result.clone()); false } else { true }
        });
        result.is_ok() && self.dirty()
    }
}

impl Productivity {
    /// Claim a bounded reply slot before applying a command or desktop mutation.
    pub(crate) fn reserve_todos(&mut self) -> Result<Reservation, String> {
        if !self.ready { return Err(mandara_i18n::t("Loading…").to_owned()); }
        self.todo_writes.reserve()
    }

    /// Failed validation cannot retain a slot or change persistence ordering.
    pub(crate) fn cancel_todos(&mut self, reservation: Reservation) { self.todo_writes.cancel(reservation); }

    /// Start persistence only after the reserved mutation has produced its revision.
    pub(crate) fn commit_todos(&mut self, reservation: Reservation, cx: &mut Context<Self>) -> async_channel::Receiver<Result<(), String>> {
        let result = self.todo_writes.bind(reservation);
        if self.todo_writes.dirty() { self.start_save(true, cx); }
        result
    }
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
    fn reservations_reject_overflow_before_mutation_and_bind_only_after_success() {
        let mut writes = TodoWrites::default();
        let reserved: Vec<_> = (0..MAX_WAITERS).map(|_| writes.reserve().unwrap()).collect();
        assert!(writes.reserve().is_err());
        assert_eq!(writes.revision, 0, "overflow cannot advance a mutation revision");
        let mut reserved = reserved.into_iter();
        writes.cancel(reserved.next().unwrap());
        let replacement = writes.reserve().unwrap();
        writes.changed();
        let pending = writes.bind(replacement);
        assert!(pending.try_recv().is_err());
        writes.finish(writes.revision, &Ok(()));
        assert_eq!(pending.try_recv(), Ok(Ok(())));
        assert_eq!(writes.waiters.len(), MAX_WAITERS - 1, "unbound reservations survive unrelated completions");
        for reservation in reserved { writes.cancel(reservation); }
        assert!(writes.waiters.is_empty());
    }

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
