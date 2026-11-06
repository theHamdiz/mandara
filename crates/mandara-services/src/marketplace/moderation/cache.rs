//! Last valid policy, with bounded freshness and explicit failure behavior.

use std::io::Read as _;
use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::policy::Policy;
use super::transport::{MAX_POLICY_BYTES, Response, Transport};

/// Reuse successful validations for up to a minute. Failed refreshes can
/// retry on the next store action; there is no polling while idle.
pub const FRESH_FOR: Duration = Duration::from_secs(60);
/// After a day without validation, refuse new installs and updates. Searches
/// still apply every known ban until a valid replacement is received.
pub const MAX_INSTALL_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// JSON escaping can expand every source byte, plus metadata.
const MAX_CACHE_BYTES: u64 = MAX_POLICY_BYTES * 6 + 4096;
/// Allow the peer's 60-second request timeout plus scheduling margin.
const LOCK_WAIT: Duration = Duration::from_secs(75);
/// A waiting store action checks for a completed peer refresh without spinning.
const LOCK_RETRY: Duration = Duration::from_millis(50);

#[derive(Serialize, Deserialize)]
struct Stored {
    text: String,
    etag: Option<String>,
    validated_at: u64,
}

/// Reads bounded cache metadata. Missing, unreadable, oversized or malformed
/// JSON is a cache miss; the caller must still validate its policy text.
fn read(path: &Path) -> Option<Stored> {
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(MAX_CACHE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_CACHE_BYTES { return None; }
    serde_json::from_slice(&bytes).ok()
}

fn read_validated(path: &Path) -> Option<(Stored, Policy)> {
    read(path).and_then(|stored| Policy::parse(&stored.text).ok().map(|policy| (stored, policy)))
}

fn age(stored: &Stored, now: u64) -> u64 {
    now.checked_sub(stored.validated_at).unwrap_or(u64::MAX)
}

/// A failed refresh never renews validation or discards known deny rules.
fn retained(cached: Option<(Stored, Policy)>, now: u64, installing: bool) -> Option<Policy> {
    cached.filter(|(stored, _)| !installing || age(stored, now) <= MAX_INSTALL_AGE.as_secs()).map(|(_, policy)| policy)
}

/// Atomically replaces the cache with a validated document. A write failure
/// is logged and leaves the previous cache intact; this request can still
/// use the freshly validated policy even when it cannot persist it.
fn save(path: &Path, stored: &Stored) {
    let result = (|| -> Result<(), String> {
        let parent = path.parent().ok_or("missing policy cache directory")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        temporary.write_all(&serde_json::to_vec(stored).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        temporary.as_file().sync_all().map_err(|e| e.to_string())?;
        temporary.persist(path).map_err(|e| e.error.to_string())?;
        Ok(())
    })();
    if let Err(error) = result { tracing::warn!("could not cache store policy: {error}"); }
}

/// Clock and transport are supplied by the caller for deterministic tests.
pub fn load(path: &Path, transport: &dyn Transport, now: u64, installing: bool) -> Result<Policy, String> {
    load_with_lock_wait(path, transport, now, installing, LOCK_WAIT)
}

fn load_with_lock_wait(path: &Path, transport: &dyn Transport, now: u64, installing: bool, wait: Duration) -> Result<Policy, String> {
    let started = std::time::Instant::now();
    // Waiting for a peer or the network must count toward the cache's age.
    let clock = || now.saturating_add(started.elapsed().as_secs());
    // Lock a separate, stable inode: saving replaces the cache file itself.
    // Shell and Studio must both reread the cache after acquiring this lock.
    let _file_lock = match refresh_lock(path, wait) {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
            // Saving replaces the cache atomically. Reading a complete snapshot
            // is safe without ownership; refreshing or writing it is not.
            return retained(read_validated(path), clock(), installing).ok_or_else(|| error.to_string());
        }
        Err(error) => return Err(error.to_string()),
    };
    let cached = read_validated(path);
    let checked_at = clock();
    if let Some((stored, policy)) = &cached
        && age(stored, checked_at) < FRESH_FOR.as_secs() { return Ok(policy.clone()); }
    let fetched = transport.fetch(cached.as_ref().and_then(|(stored, _)| stored.etag.as_deref()));
    let failure = match fetched {
        Ok(Response::Modified { text, etag }) => match Policy::parse(&text) {
            Ok(policy) => { save(path, &Stored { text, etag, validated_at: checked_at }); return Ok(policy); }
            Err(error) => error,
        },
        Ok(Response::NotModified) => match cached {
            Some((mut stored, policy)) => { stored.validated_at = checked_at; save(path, &stored); return Ok(policy); }
            None => "store policy returned 304 without a cached policy".into(),
        },
        Err(error) => error,
    };
    retained(cached, clock(), installing).ok_or(failure)
}

/// Waits at most `wait` for exclusive refresh ownership. The returned
/// file holds the lock until dropped; its separate inode survives cache
/// replacement. Failure to open or lock it prevents an uncoordinated refresh.
fn refresh_lock(path: &Path, wait: Duration) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    let file = std::fs::File::options().read(true).write(true).create(true).truncate(false).open(path.with_extension("lock"))?;
    let started = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                let remaining = wait.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out waiting for the community store policy lock"));
                }
                std::thread::sleep(remaining.min(LOCK_RETRY));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use super::*;

    struct Mock { response: RefCell<Result<Response, String>>, etags: RefCell<Vec<Option<String>>> }
    impl Transport for Mock {
        fn fetch(&self, etag: Option<&str>) -> Result<Response, String> {
            self.etags.borrow_mut().push(etag.map(str::to_string));
            std::mem::replace(&mut *self.response.borrow_mut(), Err("offline".into()))
        }
    }
    fn mock(response: Result<Response, String>) -> Mock { Mock { response: RefCell::new(response), etags: RefCell::default() } }
    fn modified(text: &str) -> Result<Response, String> { Ok(Response::Modified { text: text.into(), etag: Some("v1".into()) }) }

    #[test]
    fn first_failure_is_closed_and_empty_document_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        assert!(load(&path, &mock(Err("offline".into())), 100, true).is_err());
        assert!(load(&path, &mock(Err("offline".into())), 100, false).is_err());
        assert!(load(&path, &mock(modified("broken = [")), 100, true).is_err());
        assert!(load(&path, &mock(modified("")), 100, true).is_ok());
        assert!(load(&path, &mock(Err("offline".into())), 101, true).is_ok());
    }

    #[test]
    fn fresh_cache_avoids_requests_and_corrupt_cache_requires_validation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        load(&path, &mock(modified("")), 100, true).unwrap();
        let offline = mock(Err("offline".into()));
        load(&path, &offline, 101, false).unwrap();
        assert!(offline.etags.borrow().is_empty());
        std::fs::write(&path, "{").unwrap();
        assert!(load(&path, &offline, 102, true).is_err());
        assert!(load(&path, &mock(Ok(Response::NotModified)), 103, true).is_err());
        load(&path, &mock(modified("version = 1")), 104, true).unwrap();
    }

    #[test]
    fn cache_writes_do_not_clobber_another_writers_staging_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        let other_stage = path.with_extension("new");
        std::fs::write(&other_stage, "another process is writing").unwrap();
        load(&path, &mock(modified("version = 1")), 100, true).unwrap();
        assert_eq!(std::fs::read_to_string(other_stage).unwrap(), "another process is writing");
        assert!(read(&path).is_some());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3, "only the cache, lock and unrelated staging file remain");
    }

    /// A stuck peer must not keep a store request waiting indefinitely.
    #[test]
    fn busy_refresh_lock_times_out_without_taking_ownership() {
        use std::sync::mpsc;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        let held = refresh_lock(&path, Duration::ZERO).unwrap();
        let contender_path = path.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let contender = std::thread::spawn(move || {
            let result = refresh_lock(&contender_path, LOCK_RETRY * 2).map(|_| ());
            done_tx.send(result).unwrap();
        });
        let result = done_rx.recv_timeout(Duration::from_secs(2));
        // Release and join even if the regression waited too long.
        drop(held);
        contender.join().unwrap();
        assert_eq!(result.unwrap().unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        refresh_lock(&path, Duration::ZERO).unwrap();
    }

    #[test]
    fn lock_timeouts_retain_eligible_policy_without_refreshing_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        load(&path, &mock(modified("[[publishers]]\nlogin = 'alice'")), 100, true).unwrap();
        let before = std::fs::read(&path).unwrap();
        let held = refresh_lock(&path, Duration::ZERO).unwrap();
        let transport = mock(modified(""));
        let repository = super::super::policy::Repository { id: None, full_name: "alice/weather".into(), owner: super::super::policy::Publisher { id: None, login: "alice".into() } };
        let limit = 100 + MAX_INSTALL_AGE.as_secs();
        for (now, installing) in [(101, true), (limit, true), (limit + 1, false)] {
            let policy = load_with_lock_wait(&path, &transport, now, installing, Duration::ZERO).unwrap();
            assert!(policy.decision(&repository).denied, "timeout fallback must preserve known bans");
        }
        assert!(load_with_lock_wait(&path, &transport, limit + 1, true, Duration::ZERO).is_err());
        assert!(load_with_lock_wait(&path, &transport, 99, true, Duration::ZERO).is_err(), "a future validation timestamp must not authorize installation");
        assert!(transport.etags.borrow().is_empty(), "a contender must not fetch without the refresh lock");
        assert_eq!(std::fs::read(&path).unwrap(), before, "timeout fallback must not renew validation or change the cache");
        let independent = std::fs::File::open(path.with_extension("lock")).unwrap();
        assert!(matches!(independent.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
        drop(held);
    }

    #[test]
    fn time_waiting_for_the_lock_counts_against_install_freshness() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        load(&path, &mock(modified("")), 100, true).unwrap();
        let _held = refresh_lock(&path, Duration::ZERO).unwrap();
        let limit = 100 + MAX_INSTALL_AGE.as_secs();
        let transport = mock(modified(""));
        assert!(load_with_lock_wait(&path, &transport, limit, true, LOCK_RETRY * 20).is_err());
        assert_eq!(read(&path).unwrap().validated_at, 100);
        assert!(transport.etags.borrow().is_empty());
    }

    #[test]
    fn lock_timeouts_require_a_valid_cache_for_browsing_and_installing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        let _held = refresh_lock(&path, Duration::ZERO).unwrap();
        let transport = mock(modified(""));
        for installing in [false, true] {
            assert!(load_with_lock_wait(&path, &transport, 100, installing, Duration::ZERO).is_err());
            for bytes in [b"{".to_vec(), serde_json::to_vec(&Stored { text: "version = 99".into(), etag: None, validated_at: 100 }).unwrap()] {
                std::fs::write(&path, bytes).unwrap();
                assert!(load_with_lock_wait(&path, &transport, 100, installing, Duration::ZERO).is_err());
            }
            std::fs::remove_file(&path).unwrap();
        }
        assert!(transport.etags.borrow().is_empty());
    }

    #[test]
    fn other_lock_errors_do_not_enable_cache_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        load(&path, &mock(modified("")), 100, true).unwrap();
        std::fs::remove_file(path.with_extension("lock")).unwrap();
        std::fs::create_dir(path.with_extension("lock")).unwrap();
        let transport = mock(modified(""));
        for installing in [false, true] {
            assert!(load_with_lock_wait(&path, &transport, 101, installing, Duration::ZERO).is_err());
        }
        assert!(transport.etags.borrow().is_empty());
    }

    #[test]
    fn independent_handles_serialize_refresh_and_reuse_the_completed_policy() {
        use std::sync::{Arc, atomic::{AtomicUsize, Ordering}, mpsc};

        struct Paused {
            entered: mpsc::Sender<()>,
            resume: mpsc::Receiver<()>,
        }
        impl Transport for Paused {
            fn fetch(&self, _: Option<&str>) -> Result<Response, String> {
                self.entered.send(()).unwrap();
                self.resume.recv().unwrap();
                modified("[[publishers]]\nlogin = 'alice'")
            }
        }
        struct Counted(Arc<AtomicUsize>);
        impl Transport for Counted {
            fn fetch(&self, _: Option<&str>) -> Result<Response, String> {
                self.0.fetch_add(1, Ordering::Relaxed);
                modified("")
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let first_path = path.clone();
        let first = std::thread::spawn(move || load(&first_path, &Paused { entered: entered_tx, resume: resume_rx }, 100, true));
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        // Open independently, rather than cloning the first handle: these
        // are the lock semantics used by separate Mandara processes.
        let independent = std::fs::File::options().read(true).write(true).open(path.with_extension("lock")).unwrap();
        assert!(matches!(independent.try_lock(), Err(std::fs::TryLockError::WouldBlock)));

        let calls = Arc::new(AtomicUsize::new(0));
        let second_calls = calls.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let second = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            done_tx.send(load(&path, &Counted(second_calls), 101, true)).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(done_rx.recv_timeout(Duration::from_millis(50)), Err(mpsc::RecvTimeoutError::Timeout)));
        resume_tx.send(()).unwrap();
        first.join().unwrap().unwrap();
        let policy = done_rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
        second.join().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 0, "the waiting caller must reread the freshly saved policy");
        let repository = super::super::policy::Repository { id: None, full_name: "alice/weather".into(), owner: super::super::policy::Publisher { id: None, login: "alice".into() } };
        assert!(policy.decision(&repository).denied);
        independent.try_lock().unwrap();
    }

    #[test]
    fn etags_unblock_and_failures_never_discard_a_known_ban() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        let repository = super::super::policy::Repository { id: Some(1), full_name: "alice/weather".into(), owner: super::super::policy::Publisher { id: Some(2), login: "alice".into() } };
        assert!(load(&path, &mock(modified("[[publishers]]\nlogin = 'alice'")), 100, true).unwrap().decision(&repository).denied);
        let unchanged = mock(Ok(Response::NotModified));
        assert!(load(&path, &unchanged, 200, true).unwrap().decision(&repository).denied);
        assert_eq!(*unchanged.etags.borrow(), vec![Some("v1".into())]);
        assert!(load(&path, &mock(modified("version = 99")), 300, true).unwrap().decision(&repository).denied);
        assert!(load(&path, &mock(Err("offline".into())), 400, true).unwrap().decision(&repository).denied);
        let expired = 200 + MAX_INSTALL_AGE.as_secs() + 1;
        assert!(load(&path, &mock(Err("offline".into())), expired, true).is_err());
        assert!(load(&path, &mock(Err("offline".into())), expired, false).unwrap().decision(&repository).denied);
        assert!(!load(&path, &mock(modified("")), expired, true).unwrap().decision(&repository).denied);
    }
}
