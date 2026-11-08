//! One decoder per view, including page changes and close/reopen races.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Thumbnails a clipboard view still needs, decoded by one sequential
/// worker at a time. The generation, shared with the running batch,
/// changes on every clear or drop so that batch stops early.
pub(super) struct Queue<T> {
    pending: Vec<T>,
    generation: Arc<AtomicU64>,
    running: bool,
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self {
            pending: Vec::new(),
            generation: Arc::new(AtomicU64::new(0)),
            running: false,
        }
    }
}

impl<T> Queue<T> {
    /// Queues more thumbnails; they join the next batch, never the running one.
    pub(super) fn extend(&mut self, jobs: impl IntoIterator<Item = T>) {
        self.pending.extend(jobs);
    }

    /// Takes everything queued as one batch unless a worker is already
    /// running (or nothing is queued).
    pub(super) fn start(&mut self) -> Option<Batch<T>> {
        if self.running || self.pending.is_empty() {
            return None;
        }
        self.running = true;
        Some(Batch {
            jobs: std::mem::take(&mut self.pending),
            generation: self.generation.clone(),
            expected: self.generation.load(Ordering::Acquire),
        })
    }

    /// Called only after the previous worker has actually finished.
    pub(super) fn finish(&mut self) -> Option<Batch<T>> {
        self.running = false;
        self.start()
    }

    /// Whether a result from the batch of generation `expected` still applies.
    pub(super) fn current(&self, expected: u64) -> bool {
        self.generation.load(Ordering::Acquire) == expected
    }

    /// Forgets the queue and its capacity (a page change or close) and makes
    /// the running batch stale. That batch keeps the slot until it finishes.
    pub(super) fn clear(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.pending.clear();
        self.pending.shrink_to_fit();
    }
}

impl<T> Drop for Queue<T> {
    /// A closed view makes its running batch stale, so it decodes nothing more.
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}

pub(super) struct Batch<T> {
    jobs: Vec<T>,
    generation: Arc<AtomicU64>,
    expected: u64,
}

impl<T> Batch<T> {
    /// The generation this batch was started under.
    pub(super) fn generation(&self) -> u64 {
        self.expected
    }

    /// Decoding stays sequential; at most one completed image waits for the UI.
    pub(super) async fn run<R>(
        self,
        mut decode: impl FnMut(T) -> R,
        results: async_channel::Sender<R>,
    ) {
        for job in self.jobs {
            if self.generation.load(Ordering::Acquire) != self.expected || results.is_closed() {
                break;
            }
            let result = decode(job);
            if self.generation.load(Ordering::Acquire) != self.expected {
                break;
            }
            if results.send(result).await.is_err() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_pages_wait_for_the_existing_worker_even_after_reopening() {
        let mut queue = Queue::default();
        queue.extend([1, 2]);
        let first = queue.start().unwrap();
        queue.extend([3]);
        assert!(queue.start().is_none());
        queue.clear();
        queue.extend([4, 5]);
        assert!(
            queue.start().is_none(),
            "cancelling does not make an in-flight decode disappear"
        );
        assert!(!queue.current(first.generation()));
        let next = queue.finish().unwrap();
        assert_eq!(next.jobs, [4, 5]);
        assert!(queue.current(next.generation()));
        assert!(queue.finish().is_none());
    }

    #[test]
    fn results_stream_in_order_and_a_close_during_decode_stops_the_batch() {
        let mut queue = Queue::default();
        queue.extend([1, 2, 3]);
        let batch = queue.start().unwrap();
        let (tx, rx) = async_channel::bounded(1);
        let generation = queue.generation.clone();
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(0);
        let (resume_tx, resume_rx) = std::sync::mpsc::sync_channel(0);
        let worker = std::thread::spawn(move || {
            let mut decoded = Vec::new();
            mandara_services::runtime::block_on(batch.run(
                |job| {
                    decoded.push(job);
                    if job == 2 {
                        entered_tx.send(()).unwrap();
                        resume_rx.recv().unwrap();
                    }
                    job
                },
                tx,
            ));
            decoded
        });
        assert_eq!(
            rx.recv_blocking().unwrap(),
            1,
            "first result arrives before the batch finishes"
        );
        entered_rx.recv().unwrap();
        generation.fetch_add(1, Ordering::AcqRel);
        resume_tx.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), [1, 2]);
        assert!(
            rx.recv_blocking().is_err(),
            "a cancelled result never reaches the cache"
        );
    }

    #[test]
    fn destroying_the_view_cancels_queued_work() {
        let mut queue = Queue::default();
        queue.extend([1, 2]);
        let batch = queue.start().unwrap();
        drop(queue);
        let (tx, rx) = async_channel::bounded::<()>(1);
        mandara_services::runtime::block_on(
            batch.run(|_| panic!("closed view must not decode"), tx),
        );
        assert!(rx.recv_blocking().is_err());
    }
}
