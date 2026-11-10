//! The caller's logind session: its id (for the polkit agent's subject) and
//! its `Lock` signal (`loginctl lock-session`, idle daemons, the session
//! menu's Lock button), which the shell answers with Mandara's lock screen.
//!
//! FreeBSD has no logind; ConsoleKit2 serves the same manager API
//! (`PowerOff`, `Suspend`, `Inhibit`, `PrepareForSleep`…) and session
//! `Lock` signal under its own names, which [`bus`] picks.

use futures::StreamExt as _;
use zbus::zvariant::OwnedObjectPath;

use crate::actor::proxy;

/// The login manager's D-Bus names: logind, or ConsoleKit2 on FreeBSD.
pub mod bus {
    #[cfg(not(target_os = "freebsd"))]
    mod names {
        pub const DEST: &str = "org.freedesktop.login1";
        pub const MANAGER_PATH: &str = "/org/freedesktop/login1";
        pub const MANAGER: &str = "org.freedesktop.login1.Manager";
        pub const SESSION: &str = "org.freedesktop.login1.Session";
        /// How diagnostics name the manager (`login1.Manager.PowerOff`).
        pub const SHORT: &str = "login1";
    }
    #[cfg(target_os = "freebsd")]
    mod names {
        pub const DEST: &str = "org.freedesktop.ConsoleKit";
        pub const MANAGER_PATH: &str = "/org/freedesktop/ConsoleKit/Manager";
        pub const MANAGER: &str = "org.freedesktop.ConsoleKit.Manager";
        pub const SESSION: &str = "org.freedesktop.ConsoleKit.Session";
        pub const SHORT: &str = "ConsoleKit";
    }
    pub use names::*;
}

#[cfg(not(target_os = "freebsd"))]
const DEST: &str = bus::DEST;

/// FreeBSD: this process's ConsoleKit2 session (`GetSessionForUnixProcess`;
/// a login whose PAM stack runs `pam_ck_connector` has one).
#[cfg(target_os = "freebsd")]
pub async fn session_path(conn: &zbus::Connection) -> zbus::Result<OwnedObjectPath> {
    let manager = proxy(conn, bus::DEST, bus::MANAGER_PATH, bus::MANAGER).await?;
    manager.call("GetSessionForUnixProcess", &(std::process::id(),)).await
}

/// Object path of the session this process belongs to: `$XDG_SESSION_ID`,
/// else the session of our PID, else the user's display session.
#[cfg(not(target_os = "freebsd"))]
pub async fn session_path(conn: &zbus::Connection) -> zbus::Result<OwnedObjectPath> {
    let manager = proxy(conn, DEST, "/org/freedesktop/login1", "org.freedesktop.login1.Manager").await?;
    if let Ok(id) = std::env::var("XDG_SESSION_ID")
        && !id.is_empty()
        && let Ok(path) = manager.call::<_, _, OwnedObjectPath>("GetSession", &(id.as_str(),)).await
    {
        return Ok(path);
    }
    if let Ok(path) = manager.call::<_, _, OwnedObjectPath>("GetSessionByPID", &(std::process::id(),)).await {
        return Ok(path);
    }
    let uid = rustix::process::getuid().as_raw();
    let user_path: OwnedObjectPath = manager.call("GetUser", &(uid,)).await?;
    let user = proxy(conn, DEST, user_path.as_str(), "org.freedesktop.login1.User").await?;
    let (_, path): (String, OwnedObjectPath) = user.get_property("Display").await?;
    if path.as_str() == "/" {
        return Err(zbus::Error::Failure("no logind session for this user".into()));
    }
    Ok(path)
}

/// The session id (`"3"`, `"c2"`) of this process's logind session.
#[cfg(not(target_os = "freebsd"))]
pub async fn session_id(conn: &zbus::Connection) -> zbus::Result<String> {
    if let Ok(id) = std::env::var("XDG_SESSION_ID")
        && !id.is_empty()
    {
        return Ok(id);
    }
    let path = session_path(conn).await?;
    let session = proxy(conn, DEST, path.as_str(), "org.freedesktop.login1.Session").await?;
    session.get_property::<String>("Id").await}

/// Calls `on_lock` (on the blocking pool: it may spawn the locker)
/// whenever logind asks this session to lock. Runs until the process exits.
pub fn watch_lock_requests(on_lock: impl Fn() + Send + Sync + 'static) {
    let on_lock = std::sync::Arc::new(on_lock);
    crate::runtime::spawn("logind-lock", async move {
        let result: zbus::Result<()> = async {
            let conn = crate::runtime::system().await?;
            let path = session_path(&conn).await?;
            let session = proxy(&conn, bus::DEST, path.as_str(), bus::SESSION).await?;
            let mut locks = session.receive_signal("Lock").await?;
            tracing::info!("lock screen: answering logind lock requests for {}", path.as_str());
            while locks.next().await.is_some() {
                let f = on_lock.clone();
                crate::runtime::unblock(move || f()).await;
            }
            Ok(())
        }
        .await;
        if let Err(err) = result {
            tracing::warn!("lock screen: not listening for logind lock requests: {err}");
        }
    });
}
