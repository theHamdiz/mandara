//! Process allocator upkeep requested after cached data is released.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

/// No scheduled or executing trim.
const IDLE: u8 = 0;
/// At least one release needs a trim.
const REQUESTED: u8 = 1;
/// The blocking worker is executing the hook.
const RUNNING: u8 = 2;

struct Requests(AtomicU8);

impl Requests {
    const fn new() -> Self {
        Self(AtomicU8::new(IDLE))
    }

    /// Only the transition from idle schedules a worker.
    fn request(&self) -> bool {
        self.0.fetch_or(REQUESTED, Ordering::AcqRel) == IDLE
    }

    fn begin(&self) {
        self.0.store(RUNNING, Ordering::Release);
    }

    /// A release during the trim gets one further pass on this worker.
    fn again(&self) -> bool {
        self.0
            .compare_exchange(RUNNING, IDLE, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    }
}

static HOOK: OnceLock<fn()> = OnceLock::new();
static REQUESTS: Requests = Requests::new();

/// Bootstrap installs its allocator's hook before any UI can request upkeep.
/// Repeated initialization retains the first process-wide hook.
pub fn install_trim_hook(hook: fn()) {
    HOOK.get_or_init(|| hook);
}

/// Schedule allocator upkeep after dropping a cache. Calls coalesce; the hook
/// runs off the UI and service executor threads, with no additional idle timer.
/// Untuned and non-glibc builds install no hook and return immediately.
pub fn request_trim() {
    let Some(&hook) = HOOK.get() else { return };
    if !REQUESTS.request() {
        return;
    }
    crate::runtime::spawn("allocator-trim", async move {
        crate::runtime::unblock(move || {
            loop {
                REQUESTS.begin();
                hook();
                if !REQUESTS.again() {
                    break;
                }
            }
        })
        .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Barrier};

    #[test]
    fn bursts_share_one_worker_and_later_releases_are_not_lost() {
        let requests = Arc::new(Requests::new());
        assert!(requests.request());
        for _ in 0..100 {
            assert!(!requests.request());
        }
        let entered = Arc::new(Barrier::new(2));
        let resume = Arc::new(Barrier::new(2));
        let passes = Arc::new(AtomicUsize::new(0));
        let worker = {
            let (requests, entered, resume, passes) = (
                requests.clone(),
                entered.clone(),
                resume.clone(),
                passes.clone(),
            );
            std::thread::spawn(move || {
                loop {
                    requests.begin();
                    if passes.fetch_add(1, Ordering::SeqCst) == 0 {
                        entered.wait();
                        resume.wait();
                    }
                    if !requests.again() {
                        break;
                    }
                }
            })
        };
        entered.wait();
        for _ in 0..100 {
            assert!(!requests.request());
        }
        resume.wait();
        worker.join().unwrap();
        assert_eq!(passes.load(Ordering::SeqCst), 2);
        assert!(
            requests.request(),
            "a release after worker completion schedules a fresh worker"
        );
        requests.begin();
        assert!(!requests.again());
    }

    #[test]
    fn requests_arriving_before_the_hook_are_covered_by_that_pass() {
        let requests = Requests::new();
        assert!(requests.request());
        assert!(!requests.request());
        requests.begin();
        assert!(!requests.again());
        assert_eq!(requests.0.load(Ordering::Acquire), IDLE);
    }
}
