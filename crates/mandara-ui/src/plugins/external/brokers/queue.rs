//! Shared limits for blocking broker work, including cancelled requests.

use std::collections::VecDeque;

/// Callers of the same short lookup share one result, with a bounded waiter list.
pub(super) struct Batch<T, const LIMIT: usize>(Option<Vec<T>>);

impl<T, const LIMIT: usize> Batch<T, LIMIT> {
    pub const fn new() -> Self { Self(None) }

    pub fn join(&mut self, waiter: T) -> Result<bool, T> {
        match &mut self.0 {
            Some(waiters) if waiters.len() < LIMIT => { waiters.push(waiter); Ok(false) }
            Some(_) => Err(waiter),
            None if LIMIT > 0 => { self.0 = Some(vec![waiter]); Ok(true) }
            None => Err(waiter),
        }
    }

    pub fn finish(&mut self) -> Vec<T> { self.0.take().unwrap_or_default() }
}

pub(super) struct Queue<T, const RUNNING: usize, const WAITING: usize> {
    waiting: VecDeque<T>,
    running: usize,
}

impl<T, const RUNNING: usize, const WAITING: usize> Queue<T, RUNNING, WAITING> {
    pub const fn new() -> Self { Self { waiting: VecDeque::new(), running: 0 } }

    pub fn push(&mut self, job: T) -> Result<(), T> {
        if self.waiting.len() >= WAITING { return Err(job); }
        self.waiting.push_back(job);
        Ok(())
    }

    pub fn has_room(&self) -> bool { self.waiting.len() < WAITING }

    pub fn next_job(&mut self) -> Option<T> {
        self.next_job_where(|_| true)
    }

    pub fn next_job_where(&mut self, available: impl FnMut(&T) -> bool) -> Option<T> {
        if self.running >= RUNNING { return None; }
        let index = self.waiting.iter().position(available)?;
        let job = self.waiting.remove(index)?;
        self.running += 1;
        if self.waiting.is_empty() { self.waiting.shrink_to_fit(); }
        Some(job)
    }

    pub fn complete(&mut self) {
        self.running = self.running.checked_sub(1).expect("a broker completion owns a running slot");
    }

    pub fn retain(&mut self, keep: impl FnMut(&T) -> bool) {
        self.waiting.retain(keep);
        if self.waiting.is_empty() { self.waiting.shrink_to_fit(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_waiters_share_one_job_and_release_their_storage_after_delivery() {
        let mut batch = Batch::<_, 2>::new();
        assert_eq!(batch.join(1), Ok(true));
        assert_eq!(batch.join(2), Ok(false));
        assert_eq!(batch.join(3), Err(3));
        assert_eq!(batch.finish(), [1, 2]);
        assert!(batch.0.is_none());
        assert_eq!(batch.join(4), Ok(true));
        assert_eq!(batch.finish(), [4]);
    }

    #[test]
    fn running_and_waiting_work_have_independent_limits() {
        let mut queue = Queue::<_, 2, 3>::new();
        for id in 0..3 { queue.push(id).unwrap(); }
        assert_eq!(queue.push(4), Err(4));
        assert_eq!(queue.next_job(), Some(0));
        assert_eq!(queue.next_job(), Some(1));
        assert_eq!(queue.next_job(), None);
        queue.push(5).unwrap();
        queue.retain(|id| *id != 2);
        queue.complete();
        assert_eq!(queue.next_job(), Some(5));
        assert_eq!(queue.next_job(), None);
        queue.complete();
        queue.complete();
        assert_eq!(queue.next_job(), None);
        assert_eq!(queue.waiting.capacity(), 0);
    }
}
