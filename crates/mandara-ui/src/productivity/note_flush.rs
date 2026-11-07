//! Live edits retain one active flush and one latest model, not a queue of bodies.

#[derive(Default)]
pub(super) struct NoteFlush {
    running: bool,
}

impl NoteFlush {
    pub fn running(&self) -> bool { self.running }

    /// Only a submitted flush takes ownership; a failed enqueue remains retryable.
    pub fn begin(&mut self) { self.running = true; }

    /// Running IO always completes before another replaceable snapshot is sent.
    pub fn finish(&mut self) { self.running = false; }

    /// Structural intents are ordering barriers. Failed writes never spin on retry.
    pub fn should_flush(&self, latest_succeeded: bool, barrier_pending: bool, dirty: bool) -> bool {
        !self.running && latest_succeeded && !barrier_pending && dirty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_changes_keep_one_flush_owned_until_its_receipt() {
        let mut flush = NoteFlush::default();
        assert!(flush.should_flush(true, false, true));
        flush.begin();
        for _ in 0..100 { assert!(!flush.should_flush(true, false, true)); }
        flush.finish();
        assert!(flush.should_flush(true, false, true));
        assert!(!flush.should_flush(true, false, false));
    }

    #[test]
    fn later_structural_intents_and_failures_prevent_an_older_flush_from_replaying_a_body() {
        let mut flush = NoteFlush::default();
        flush.begin();
        flush.finish();
        assert!(!flush.should_flush(true, true, true), "a queued append/move owns the next ordering boundary");
        assert!(!flush.should_flush(false, false, true), "failure stays retryable without an automatic retry loop");
        assert!(flush.should_flush(true, false, true));
    }
}
