//! Plasma owns secure locking on KWin, which does not expose ext-session-lock.
//! A successful Lock reply is only a request; GetActive/ActiveChanged confirms it.

use std::time::Duration;
use futures::{FutureExt as _, StreamExt as _};
use mandara_i18n::t;

const DEST: &str = "org.freedesktop.ScreenSaver";
const PATH: &str = "/ScreenSaver";
/// Bound the complete request, including an unresponsive locker or bus.
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);

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

/// Tracks real lock/unlock transitions without polling. This also observes
/// locks started by Plasma's shortcuts and its own idle timer.
pub fn watch(on_change: impl Fn(bool) + Send + Sync + 'static) {
    crate::runtime::spawn("plasma-lock", async move {
        let result: zbus::Result<()> = async {
            let conn = crate::runtime::session().await?;
            let proxy = crate::actor::proxy(&conn, DEST, PATH, DEST).await?;
            let mut signals = proxy.receive_signal("ActiveChanged").await?;
            on_change(proxy.call("GetActive", &()).await?);
            while let Some(msg) = signals.next().await {
                on_change(msg.body().deserialize()?);
            }
            Ok(())
        }.await;
        if let Err(e) = result { tracing::warn!("Plasma lock notifications: {e}"); }
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
