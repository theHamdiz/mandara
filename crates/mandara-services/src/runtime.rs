//! The one async runtime every service actor runs on.
//!
//! Before this module each actor owned an OS thread (`zbus::block_on`) and a
//! private D-Bus connection, and every zbus connection started its own
//! executor thread — dozens of mostly idle threads. Now:
//!
//! * [`spawn`] runs an actor as a task on a single executor driven by
//!   [`WORKER_THREADS`] named threads (`mandara-svc-N`);
//! * [`session`] / [`system`] hand out one shared connection per bus, built
//!   without zbus's internal executor thread — its socket reader runs as a
//!   task on the same executor;
//! * [`connect`] builds a private connection the same way for actors that
//!   own a well-known name or serve objects;
//! * [`unblock`] moves blocking work (files, processes, HTTP) to a thread
//!   of its own that ends with the work, so the executor threads never
//!   stall and nothing idles. The `blocking` crate's pool (still used inside
//!   zbus and smol) never shrinks below its limit (500): every thread it
//!   ever started woke twice a second forever, so it is capped at
//!   [`BLOCKING_POOL_THREADS`].
//!
//! Timers come from `async-io` (its reactor thread is shared process-wide).

use std::future::Future;
use std::sync::Once;
use std::time::Duration;

use async_executor::Executor;
use async_lock::OnceCell;
use zbus::Connection;

/// Executor threads. Two, so one slow task can never freeze every service.
pub const WORKER_THREADS: usize = 2;
/// Messages a connection queues per stream before its reader waits for the
/// consumer (zbus default: 64). Shared connections carry several actors'
/// streams, so they get more headroom.
pub const SHARED_MAX_QUEUED: usize = 256;
/// Thread-name prefix of the executor threads.
const WORKER_NAME: &str = "mandara-svc";
/// Threads the `blocking` crate's pool may keep (zbus / smol internals).
pub const BLOCKING_POOL_THREADS: usize = 2;
/// Name of the short-lived threads [`unblock`] runs work on.
const UNBLOCK_NAME: &str = "mandara-unblock";

static EXECUTOR: Executor<'static> = Executor::new();
static START: Once = Once::new();
static SESSION: OnceCell<Connection> = OnceCell::new();
static SYSTEM: OnceCell<Connection> = OnceCell::new();

thread_local! {
    /// Set on the executor threads (see [`block_on`]).
    static ON_WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn start_workers() {
    START.call_once(|| {
        if let Some(limit) = std::num::NonZeroUsize::new(BLOCKING_POOL_THREADS) {
            blocking::set_max_blocking_threads(limit);
        }
        for i in 0..WORKER_THREADS {
            let spawned = std::thread::Builder::new()
                .name(format!("{WORKER_NAME}-{i}"))
                .spawn(|| {
                    ON_WORKER.with(|w| w.set(true));
                    async_io::block_on(EXECUTOR.run(futures::future::pending::<()>()))
                });
            if let Err(e) = spawned {
                tracing::error!("services runtime: cannot start worker {i}: {e}");
            }
        }
    });
}

/// Runs `future` on the shared service runtime (detached). `name` labels
/// it in logs when it panics.
pub fn spawn<F>(name: &'static str, future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    start_workers();
    EXECUTOR
        .spawn(async move {
            use futures::FutureExt as _;
            if std::panic::AssertUnwindSafe(future).catch_unwind().await.is_err() {
                tracing::error!("service {name} panicked");
            }
        })
        .detach();
}

/// Runs blocking `f` on a thread of its own and awaits its result (the
/// `blocking` pool only when no thread can be started).
pub async fn unblock<T, F>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = futures::channel::oneshot::channel();
    let job = std::sync::Arc::new(std::sync::Mutex::new(Some((f, tx))));
    let run = {
        let job = job.clone();
        move || {
            let taken = job.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some((f, tx)) = taken {
                let _ = tx.send(f());
            }
        }
    };
    if std::thread::Builder::new().name(UNBLOCK_NAME.into()).spawn(run).is_err() {
        let taken = job.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some((f, _)) = taken {
            return blocking::unblock(f).await;
        }
    }
    match rx.await {
        Ok(value) => value,
        // The thread panicked: surface it like the pool would.
        Err(_) => panic!("unblock: the blocking task panicked"),
    }
}

/// An async sleep on the shared reactor.
pub async fn sleep(duration: Duration) {
    async_io::Timer::after(duration).await;
}

/// Drives a connection built without its internal executor on the runtime.
fn drive(conn: &Connection) {
    // Only the executor is held: once the last `Connection` clone drops, its
    // tasks are cancelled (which schedules them one last time), the
    // executor empties and this task ends.
    let executor = conn.executor().clone();
    spawn("zbus-connection", async move {
        loop {
            executor.tick().await;
            if executor.is_empty() {
                break;
            }
        }
    });
}

/// Builds a connection that runs on this runtime (no zbus thread).
pub async fn connect(builder: zbus::connection::Builder<'_>) -> zbus::Result<Connection> {
    start_workers();
    let conn = builder.internal_executor(false).build().await?;
    drive(&conn);
    Ok(conn)
}

/// The shared session-bus connection.
pub async fn session() -> zbus::Result<Connection> {
    SESSION.get_or_try_init(|| async { connect(zbus::connection::Builder::session()?.max_queued(SHARED_MAX_QUEUED)).await }).await.cloned()
}

/// The shared system-bus connection.
pub async fn system() -> zbus::Result<Connection> {
    SYSTEM.get_or_try_init(|| async { connect(zbus::connection::Builder::system()?.max_queued(SHARED_MAX_QUEUED)).await }).await.cloned()
}

/// Blocks the calling (non-runtime) thread on `future`, e.g. for a CLI
/// one-shot or a GPUI background task. Never call this from a service task:
/// blocking an executor thread can starve the connection readers it waits
/// on (a debug build panics; a release build logs it).
pub fn block_on<T>(future: impl Future<Output = T>) -> T {
    if ON_WORKER.with(|w| w.get()) {
        debug_assert!(false, "runtime::block_on called on a service executor thread");
        tracing::error!("runtime::block_on called on a service executor thread; use .await instead");
    }
    async_io::block_on(future)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawned_tasks_run_and_unblock_returns() {
        let (tx, rx) = async_channel::bounded(1);
        spawn("test", async move {
            let v = unblock(|| 21 * 2).await;
            let _ = tx.send(v).await;
        });
        assert_eq!(block_on(rx.recv()).ok(), Some(42));
    }

    #[test]
    fn a_panicking_task_does_not_kill_the_runtime() {
        spawn("boom", async { panic!("boom") });
        let (tx, rx) = async_channel::bounded(1);
        spawn("after", async move {
            let _ = tx.send(()).await;
        });
        assert!(block_on(rx.recv()).is_ok());
    }
}
