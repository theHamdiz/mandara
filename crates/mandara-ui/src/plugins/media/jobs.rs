//! One active job and one latest request for a view.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The latest key a view wants decoded, with at most one job running for
/// it. A generation counter, shared with the running job, marks every
/// change of request so a superseded job's result is dropped.
pub(super) struct Latest<K> {
    wanted: Option<K>,
    generation: Arc<AtomicU64>,
    running: Option<u64>,
    completed: Option<u64>,
}

impl<K> Default for Latest<K> {
    fn default() -> Self {
        Self {
            wanted: None,
            generation: Arc::new(AtomicU64::new(0)),
            running: None,
            completed: None,
        }
    }
}

impl<K: Clone + PartialEq> Latest<K> {
    /// The key the view currently wants (`None` once it no longer shows one).
    pub(super) fn wanted(&self) -> Option<&K> {
        self.wanted.as_ref()
    }

    /// Replaces the wanted key. The same key again is a no-op; a new one
    /// bumps the generation, so the running job's result goes stale, and
    /// needs a fresh job.
    pub(super) fn request(&mut self, wanted: Option<K>) {
        if self.wanted == wanted {
            return;
        }
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.wanted = wanted;
        self.completed = None;
    }

    /// The job to run for the wanted key, unless one is already running or
    /// this generation already completed. The caller runs it off the UI
    /// thread and hands it back to [`Latest::finish`].
    pub(super) fn start(&mut self) -> Option<Job<K>> {
        if self.running.is_some() {
            return None;
        }
        let expected = self.generation.load(Ordering::Acquire);
        if self.completed == Some(expected) {
            return None;
        }
        let key = self.wanted.as_ref()?.clone();
        self.running = Some(expected);
        Some(Job {
            key,
            generation: self.generation.clone(),
            expected,
        })
    }

    /// Completing an old job releases its worker slot without accepting its result.
    pub(super) fn finish(&mut self, job: &Job<K>) -> bool {
        if self.running != Some(job.expected) {
            return false;
        }
        self.running = None;
        if !job.current() {
            return false;
        }
        self.completed = Some(job.expected);
        true
    }

    /// In-flight work still occupies the slot until it actually finishes.
    pub(super) fn clear(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.wanted = None;
        self.completed = None;
    }
}

impl<K> Drop for Latest<K> {
    /// A dropped view (hidden, plugin replaced) makes its running job
    /// stale, so the decoder skips or discards the work.
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}

/// One decode of `key`, valid while the shared generation still equals
/// `expected`.
pub(super) struct Job<K> {
    key: K,
    generation: Arc<AtomicU64>,
    expected: u64,
}

impl<K> Job<K> {
    /// Whether no newer request, clear or drop happened since it started.
    fn current(&self) -> bool {
        self.generation.load(Ordering::Acquire) == self.expected
    }

    /// Stale queued jobs skip decoding; jobs superseded during decoding drop their output.
    pub(super) fn run<R>(&self, decode: impl FnOnce(&K) -> R) -> Option<R> {
        if !self.current() {
            return None;
        }
        let result = decode(&self.key);
        self.current().then_some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Barrier, atomic::AtomicUsize};

    #[test]
    fn rapid_changes_coalesce_and_returning_to_an_old_key_is_a_new_request() {
        let mut queue = Latest::default();
        queue.request(Some("a"));
        let a = queue.start().unwrap();
        queue.request(Some("b"));
        queue.request(Some("a"));
        assert!(queue.start().is_none(), "one decoder remains in flight");
        assert!(!queue.finish(&a));
        let latest = queue.start().unwrap();
        assert_eq!(latest.run(|key| *key), Some("a"));
        assert!(queue.finish(&latest));
        queue.request(Some("a"));
        assert!(
            queue.start().is_none(),
            "a completed request is not decoded every render"
        );
    }

    #[test]
    fn close_and_reopen_wait_for_the_old_worker_and_reject_its_result() {
        let mut queue = Latest::default();
        queue.request(Some("cover"));
        let old = queue.start().unwrap();
        queue.clear();
        queue.request(Some("cover"));
        assert!(queue.start().is_none());
        assert!(
            old.run(|_| panic!("cancelled work must not decode"))
                .is_none()
        );
        assert!(!queue.finish(&old));
        let reopened = queue.start().unwrap();
        assert!(queue.finish(&reopened));
        assert!(
            !queue.finish(&old),
            "an old completion cannot finish a newer job"
        );
    }

    #[test]
    fn superseding_a_running_decode_drops_its_output_before_publication() {
        struct Output(Arc<AtomicUsize>);
        impl Drop for Output {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let mut queue = Latest::default();
        queue.request(Some(1));
        let job = queue.start().unwrap();
        let entered = Arc::new(Barrier::new(2));
        let resume = Arc::new(Barrier::new(2));
        let dropped = Arc::new(AtomicUsize::new(0));
        let worker = {
            let (entered, resume, dropped) = (entered.clone(), resume.clone(), dropped.clone());
            std::thread::spawn(move || {
                job.run(|_| {
                    entered.wait();
                    resume.wait();
                    Output(dropped)
                })
            })
        };
        entered.wait();
        queue.request(Some(2));
        resume.wait();
        assert!(worker.join().unwrap().is_none());
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dropping_the_view_cancels_queued_work() {
        let mut queue = Latest::default();
        queue.request(Some(1));
        let job = queue.start().unwrap();
        drop(queue);
        assert!(
            job.run(|_| panic!("destroyed view must not decode"))
                .is_none()
        );
    }
}
