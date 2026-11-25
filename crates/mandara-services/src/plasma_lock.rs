//! Plasma owns secure locking on KWin, which does not expose ext-session-lock.
//! A successful Lock reply is only a request; GetActive/ActiveChanged confirms it.

use std::sync::Arc;
use std::time::Duration;
use futures::{FutureExt as _, StreamExt as _};
use mandara_i18n::t;

const DEST: &str = "org.freedesktop.ScreenSaver";
const PATH: &str = "/ScreenSaver";
/// Bound the complete request, including an unresponsive locker or bus.
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
/// First retry when Plasma's screensaver bus is not up yet.
const WATCH_RETRY_MIN: Duration = Duration::from_secs(2);
/// Cap on Plasma lock-watch reconnect backoff.
const WATCH_RETRY_MAX: Duration = Duration::from_secs(120);

async fn request_on(conn: &zbus::Connection) -> zbus::Result<bool> {
    let proxy = crate::actor::proxy(conn, DEST, PATH, DEST).await?;
    let mut signals = proxy.receive_signal("ActiveChanged").await?;
    if proxy.call::<_, _, bool>("GetActive", &()).await? { return Ok(false); }
    proxy.call::<_, _, ()>("Lock", &()).await?;
    if proxy.call::<_, _, bool>("GetActive", &()).await? { return Ok(true); }
    while let Some(msg) = signals.next().await {
        if msg.body().deserialize::<bool>()? { return Ok(true); }
    }
    Err(zbus::Error::Failure(t("The lock screen did not confirm that the session is locked").into()))
}

async fn confirmed(conn: &zbus::Connection, timeout: Duration) -> Result<bool, String> {
    futures::select! {
        result = request_on(conn).fuse() => result.map_err(|e| e.to_string()),
        _ = futures::FutureExt::fuse(async_io::Timer::after(timeout)) => Err(t("The lock screen did not confirm that the session is locked").into()),
    }
}

/// Requests Plasma's secure locker and waits for its confirmation.
/// Run off the UI thread, as with the other power actions.
pub fn lock() -> Result<bool, String> {
    crate::runtime::block_on(async {
        let conn = crate::runtime::session().await.map_err(|e| e.to_string())?;
        confirmed(&conn, LOCK_TIMEOUT).await
    })
}

/// One subscription's worth of [`watch`]; returns when the signal stream ends.
async fn watch_once(on_change: &Arc<dyn Fn(bool) + Send + Sync>, backoff: &mut crate::runtime::Backoff) -> zbus::Result<()> {
    let conn = crate::runtime::session().await?;
    let proxy = crate::actor::proxy(&conn, DEST, PATH, DEST).await?;
    let mut signals = proxy.receive_signal("ActiveChanged").await?;
    let active: bool = proxy.call("GetActive", &()).await?;
    backoff.reset();
    notify(on_change, active).await;
    while let Some(msg) = signals.next().await {
        notify(on_change, msg.body().deserialize()?).await;
    }
    Ok(())
}

/// Calls `on_change` on the blocking pool: it may do real work.
async fn notify(on_change: &Arc<dyn Fn(bool) + Send + Sync>, active: bool) {
    let f = on_change.clone();
    crate::runtime::unblock(move || f(active)).await;
}

/// Tracks real lock/unlock transitions without polling. This also observes
/// locks started by Plasma's shortcuts and its own idle timer. A Plasma
/// that is not up yet is retried with backoff.
pub fn watch(on_change: impl Fn(bool) + Send + Sync + 'static) {
    let on_change: Arc<dyn Fn(bool) + Send + Sync> = Arc::new(on_change);
    crate::runtime::spawn("plasma-lock", async move {
        let mut backoff = crate::runtime::Backoff::new(WATCH_RETRY_MIN, WATCH_RETRY_MAX);
        let mut warned = false;
        loop {
            if let Err(e) = watch_once(&on_change, &mut backoff).await {
                if warned {
                    tracing::debug!("Plasma lock notifications: {e}");
                } else {
                    warned = true;
                    tracing::warn!("Plasma lock notifications: {e}");
                }
            }
            crate::runtime::sleep(backoff.next_delay()).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Locker { active: AtomicBool, accept: bool }

    #[zbus::interface(name = "org.freedesktop.ScreenSaver")]
    impl Locker {
        fn get_active(&self) -> bool { self.active.load(Ordering::Relaxed) }
        async fn lock(&self, #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>) -> zbus::fdo::Result<()> {
            if self.accept {
                self.active.store(true, Ordering::Relaxed);
                Self::active_changed(&emitter, true).await?;
            }
            Ok(())
        }
        #[zbus(signal)]
        async fn active_changed(emitter: &zbus::object_server::SignalEmitter<'_>, active: bool) -> zbus::Result<()>;
    }

    async fn fixture(accept: bool) -> (zbus::Connection, zbus::Connection) {
        let (client, server) = std::os::unix::net::UnixStream::pair().unwrap();
        let server = zbus::connection::Builder::async_io_unix_stream(server).server(zbus::Guid::generate()).unwrap().p2p().serve_at(PATH, Locker { active: AtomicBool::new(false), accept }).unwrap();
        let client = zbus::connection::Builder::async_io_unix_stream(client).p2p();
        let (client, server) = futures::join!(crate::runtime::connect(client), crate::runtime::connect(server));
        (client.unwrap(), server.unwrap())
    }

    #[test]
    fn lock_requires_confirmation_and_an_already_locked_session_starts_nothing() {
        crate::runtime::block_on(async {
            let (client, _server) = fixture(true).await;
            assert_eq!(confirmed(&client, LOCK_TIMEOUT).await, Ok(true));
            assert_eq!(confirmed(&client, LOCK_TIMEOUT).await, Ok(false));
        });
    }

    #[test]
    fn a_successful_method_reply_without_a_lock_is_an_error() {
        /// A silent fixture must time out quickly without extending the suite.
        const TEST_TIMEOUT: Duration = Duration::from_millis(100);
        crate::runtime::block_on(async {
            let (client, _server) = fixture(false).await;
            assert!(confirmed(&client, TEST_TIMEOUT).await.is_err());
        });
    }
}
