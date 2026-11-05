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
    // Lock a separate, stable inode: saving replaces the cache file itself.
    // Shell and Studio must both reread the cache after acquiring this lock.
    let _file_lock = refresh_lock(path).map_err(|e| e.to_string())?;
    let cached = read(path).and_then(|stored| Policy::parse(&stored.text).ok().map(|policy| (stored, policy)));
    let age = |stored: &Stored| now.checked_sub(stored.validated_at).unwrap_or(u64::MAX);
    if let Some((stored, policy)) = &cached
        && age(stored) < FRESH_FOR.as_secs() { return Ok(policy.clone()); }
    let fetched = transport.fetch(cached.as_ref().and_then(|(stored, _)| stored.etag.as_deref()));
    let failure = match fetched {
        Ok(Response::Modified { text, etag }) => match Policy::parse(&text) {
            Ok(policy) => { save(path, &Stored { text, etag, validated_at: now }); return Ok(policy); }
            Err(error) => error,
        },
        Ok(Response::NotModified) => match cached {
            Some((mut stored, policy)) => { stored.validated_at = now; save(path, &stored); return Ok(policy); }
            None => "store policy returned 304 without a cached policy".into(),
        },
        Err(error) => error,
    };
    if let Some((stored, policy)) = cached
        && (!installing || age(&stored) <= MAX_INSTALL_AGE.as_secs()) { return Ok(policy); }
    Err(failure)
}

/// Waits for exclusive refresh ownership across processes. The returned
/// file holds the lock until dropped; its separate inode survives cache
/// replacement. Failure to open or lock it prevents an uncoordinated refresh.
fn refresh_lock(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    let file = std::fs::File::options().read(true).write(true).create(true).truncate(false).open(path.with_extension("lock"))?;
    file.lock()?;
    Ok(file)
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
