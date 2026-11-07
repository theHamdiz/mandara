//! A cancellable, bounded worker process. No detached model runtime survives it.

use super::{Event, IDLE_TIMEOUT, MAX_QUERY_BYTES, Request, compiler, embeddings, models};
use anyhow::{Result, ensure};
use std::{
    io::{BufRead, Read, Write},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

/// Bound catalog transport and worker output before allocating or parsing JSON.
const MAX_WIRE_BYTES: u64 = 4 * 1024 * 1024;
/// Only an active worker is polled; no timer or thread exists while idle.
const CANCEL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);
/// Small CPU models saturate memory bandwidth before a large pool helps.
const MAX_INFERENCE_THREADS: usize = 4;
static ACTIVE: std::sync::Mutex<Vec<std::sync::Weak<AtomicBool>>> = std::sync::Mutex::new(Vec::new());
static INFERENCE: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Active(std::sync::Weak<AtomicBool>);
impl Active {
    /// Track a weak cancellation handle while its request is active or queued.
    fn new(cancel: &Arc<AtomicBool>) -> Self {
        let weak = Arc::downgrade(cancel);
        ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).push(weak.clone());
        Self(weak)
    }
}
impl Drop for Active {
    fn drop(&mut self) {
        let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
        active.retain(|w| !w.ptr_eq(&self.0));
        if active.is_empty() {
            *active = Vec::new();
        }
    }
}

/// Cancel inference owned by this shell or Studio process, including queued work.
pub fn purge() {
    for cancel in ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).iter().filter_map(std::sync::Weak::upgrade) {
        cancel.store(true, Ordering::Release);
    }
}

/// Count queued and running requests without retaining their model state.
pub fn active_requests() -> usize {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).len()
}

/// Dropping a view or replacing its query cancels its owned work.
#[derive(Default)]
pub struct Job(Arc<AtomicBool>);
impl Job {
    /// Share the cancellation flag with work owned by this view.
    pub fn cancellation(&self) -> Arc<AtomicBool> {
        self.0.clone()
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Flush one bounded-protocol event so retrieval can appear before compilation.
fn emit(event: &Event) -> Result<()> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(&mut out, event)?;
    writeln!(out)?;
    out.flush()?;
    Ok(())
}

/// Invoked only by the hidden CLI subcommand. It has no dispatcher or session
/// connection, and returns only a preview. The parent owns execution.
pub fn worker_main(settings: impl FnOnce() -> Vec<super::catalog::Command>) -> Result<()> {
    let result = (|| {
        let mut bytes = Vec::new();
        std::io::stdin().take(MAX_WIRE_BYTES + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() as u64 <= MAX_WIRE_BYTES, "agent request too large");
        let mut request: Request = serde_json::from_slice(&bytes)?;
        request.catalog.extend(settings());
        ensure!((request.prepare_index || !request.query.trim().is_empty()) && request.query.len() <= MAX_QUERY_BYTES, "invalid agent query");
        ensure!(!request.catalog.is_empty(), "empty command catalog");
        let cancel = AtomicBool::new(false);
        models::check(&models::ROUTER, request.prepare_index, &cancel)?;
        let mut embedder = embeddings::Embedder::load()?;
        let vectors = embeddings::index(&mut embedder, &request.catalog)?;
        if request.prepare_index {
            return emit(&Event::Ready { actions: vec![], words: vec![], inference_ms: 0.0 });
        }
        let mut compiler = if request.engine == super::Engine::Qwen && !request.query.is_ascii() {
            models::check(models::compiler(request.engine), false, &cancel)?;
            Some(compiler::Compiler::load(request.engine)?)
        } else {
            None
        };
        let retrieval_query = if let Some(compiler) = &mut compiler { compiler.translate(&request.query)? } else { request.query.clone() };
        let start = Instant::now();
        let query = embedder.embed(&retrieval_query, true)?;
        let matches = embeddings::retrieve(&query, &request.catalog, &vectors);
        let commands = matches.iter().map(|m| m.command.clone()).collect::<Vec<_>>();
        emit(&Event::Matches { matches, retrieval_ms: start.elapsed().as_secs_f64() * 1000.0 })?;
        drop((embedder, vectors));
        let mut compiler = match compiler {
            Some(compiler) => compiler,
            None => {
                models::check(models::compiler(request.engine), false, &cancel)?;
                compiler::Compiler::load(request.engine)?
            }
        };
        emit(&compiler.compile(&request.query, &commands)?)
    })();
    if let Err(error) = result {
        emit(&Event::Error { message: format!("{error:#}") })?;
    }
    Ok(())
}

/// Linux procfs keeps the running inode executable after an atomic update.
fn worker_executable() -> Result<std::path::PathBuf> {
    #[cfg(target_os = "linux")]
    {
        Ok("/proc/self/exe".into())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(std::env::current_exe()?)
    }
}

struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Reclaim;
impl Drop for Reclaim {
    fn drop(&mut self) {
        crate::memory::request_trim();
    }
}

/// Blocking supervisor: always call on the existing background-work mechanism.
/// The owned child is killed and reaped on cancellation, timeout and errors.
pub fn run(request: Request, cancel: Arc<AtomicBool>, events: async_channel::Sender<Event>) -> Result<()> {
    let _reclaim = Reclaim;
    if cancel.load(Ordering::Acquire) {
        return Ok(());
    }
    let deadline = if request.prepare_index { super::INDEX_SETUP_TIMEOUT } else { IDLE_TIMEOUT };
    let start = Instant::now();
    let _active = Active::new(&cancel);
    let _permit = loop {
        if cancel.load(Ordering::Acquire) || events.is_closed() {
            return Ok(());
        }
        ensure!(start.elapsed() < deadline, "agent idle timeout");
        match INFERENCE.try_lock() {
            Ok(permit) => break permit,
            Err(std::sync::TryLockError::Poisoned(error)) => break error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(CANCEL_INTERVAL),
        }
    };
    let threads = std::thread::available_parallelism().map_or(1, usize::from).min(MAX_INFERENCE_THREADS).to_string();
    let mut child = Child(
        Command::new(worker_executable()?)
            .arg("agent-worker")
            .env("RAYON_NUM_THREADS", &threads)
            .env("CANDLE_NUM_THREADS", &threads)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let mut input = child.0.stdin.take().ok_or_else(|| anyhow::anyhow!("missing worker input"))?;
    serde_json::to_writer(&mut input, &request)?;
    drop(input);
    drop(request);
    let output = child.0.stdout.take().ok_or_else(|| anyhow::anyhow!("missing worker output"))?;
    let flags = rustix::fs::fcntl_getfl(&output)?;
    rustix::fs::fcntl_setfl(&output, flags | rustix::fs::OFlags::NONBLOCK)?;
    let mut output = std::io::BufReader::new(output);
    let mut pending = Vec::new();
    loop {
        if cancel.load(Ordering::Acquire) || events.is_closed() {
            return Ok(());
        }
        ensure!(start.elapsed() < deadline, "agent idle timeout");
        match output.fill_buf() {
            Ok([]) => break,
            Ok(bytes) => {
                ensure!(pending.len() + bytes.len() <= MAX_WIRE_BYTES as usize, "agent response too large");
                let n = bytes.len();
                pending.extend_from_slice(bytes);
                output.consume(n);
                while let Some(end) = pending.iter().position(|&b| b == b'\n') {
                    let event = serde_json::from_slice::<Event>(&pending[..end])?;
                    let _ = events.try_send(event);
                    pending.drain(..=end);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(CANCEL_INTERVAL),
            Err(error) => return Err(error.into()),
        }
    }
    ensure!(pending.is_empty(), "incomplete agent response");
    ensure!(child.0.wait()?.success(), "agent worker failed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn purge_cancels_active_requests_and_releases_tracking_storage() {
        let first = Arc::new(AtomicBool::new(false));
        let second = Arc::new(AtomicBool::new(false));
        let a = Active::new(&first);
        let b = Active::new(&second);
        assert_eq!(active_requests(), 2);
        purge();
        assert!(first.load(Ordering::Acquire));
        assert!(second.load(Ordering::Acquire));
        drop((a, b));
        assert_eq!(active_requests(), 0);
        assert_eq!(ACTIVE.lock().unwrap().capacity(), 0);
    }

    #[test]
    fn dropping_the_view_signals_cancellation_without_starting_a_runtime() {
        let job = Job::default();
        let cancel = job.cancellation();
        assert!(!cancel.load(Ordering::Acquire));
        drop(job);
        assert!(cancel.load(Ordering::Acquire));
    }
    /// A private copy of the test executable models a shell replaced by an update.
    #[test]
    #[cfg(target_os = "linux")]
    fn replaced_executables_can_still_start_a_worker() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("worker");
        std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        std::fs::write(dir.path().join(".agent-worker-fixture"), b"owned test copy").unwrap();
        let output = Command::new(&exe)
            .args(["--exact", "command_agent::worker::tests::deleted_executable_child", "--ignored", "--test-threads=1"])
            .env("MANDARA_AGENT_TEST_EXE", &exe)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
        assert!(!exe.exists());
    }

    /// Runs only inside the owned fixture created by the parent regression test.
    #[test]
    #[ignore = "subprocess fixture for replaced_executables_can_still_start_a_worker"]
    #[cfg(target_os = "linux")]
    fn deleted_executable_child() {
        let Some(expected) = std::env::var_os("MANDARA_AGENT_TEST_EXE") else {
            return;
        };
        let expected = std::path::PathBuf::from(expected);
        assert!(expected.starts_with(std::env::temp_dir()));
        assert_eq!(expected.file_name().unwrap(), "worker");
        assert_eq!(std::fs::read(expected.parent().unwrap().join(".agent-worker-fixture")).unwrap(), b"owned test copy");
        assert_eq!(std::env::current_exe().unwrap(), expected);
        std::fs::remove_file(expected).unwrap();
        let status = Command::new(worker_executable().unwrap()).arg("--list").env_remove("MANDARA_AGENT_TEST_EXE").output().unwrap().status;
        assert!(status.success());
    }

    #[test]
    fn cancelled_jobs_never_spawn_a_child() {
        let (tx, rx) = async_channel::bounded(2);
        let request = Request { query: "x".into(), catalog: vec![], engine: super::super::Engine::FunctionGemma, prepare_index: false };
        assert!(run(request, Arc::new(AtomicBool::new(true)), tx).is_ok());
        assert!(rx.try_recv().is_err());
    }
}
