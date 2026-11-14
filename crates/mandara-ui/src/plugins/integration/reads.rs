//! A running read and one replaceable successor; closing drops the successor.

pub(super) struct Slot<T> {
    running: bool,
    next: Option<T>,
}

impl<T> Default for Slot<T> {
    fn default() -> Self { Self { running: false, next: None } }
}

impl<T> Slot<T> {
    pub fn push(&mut self, read: T) -> Option<T> {
        if self.running { self.next = Some(read); None }
        else { self.running = true; Some(read) }
    }

    pub fn complete(&mut self) -> Option<T> {
        let next = self.next.take();
        self.running = next.is_some();
        next
    }

    pub fn clear(&mut self) -> bool { self.next.take().is_some() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

    struct Read(usize, Arc<AtomicUsize>);
    impl Drop for Read { fn drop(&mut self) { self.1.fetch_add(1, Ordering::Relaxed); } }

    #[test]
    fn rapid_replacements_run_only_the_active_and_latest_reads() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut slot = Slot::default();
        let active = slot.push(Read(0, dropped.clone())).unwrap();
        for id in 1..=100 { assert!(slot.push(Read(id, dropped.clone())).is_none()); }
        assert_eq!(dropped.load(Ordering::Relaxed), 99);
        drop(active);
        let last = slot.complete().unwrap();
        assert_eq!(last.0, 100);
        assert!(slot.complete().is_none());
        drop(last);
        assert_eq!(dropped.load(Ordering::Relaxed), 101);
    }

    #[test]
    fn close_releases_queued_reads_and_reopen_waits_for_the_cancelled_read() {
        let mut slot = Slot::default();
        assert_eq!(slot.push(1), Some(1));
        assert_eq!(slot.push(2), None);
        assert!(slot.clear());
        assert_eq!(slot.push(3), None);
        assert_eq!(slot.complete(), Some(3));
        assert_eq!(slot.complete(), None);
        assert_eq!(slot.push(4), Some(4));
    }
}
