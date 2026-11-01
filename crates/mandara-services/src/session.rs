//! Session / power actions and idle inhibition over logind's D-Bus API.

use mandara_i18n::{t, tf};
use serde::Serialize;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionAction {
    Lock,
    Logout,
    Suspend,
    Hibernate,
    Reboot,
    Shutdown,
}

impl SessionAction {
    pub const ALL: [SessionAction; 6] = [
        SessionAction::Lock,
        SessionAction::Logout,
        SessionAction::Suspend,
        SessionAction::Hibernate,
        SessionAction::Reboot,
        SessionAction::Shutdown,
    ];

    pub fn id(self) -> &'static str {
        match self {
            SessionAction::Lock => "lock",
            SessionAction::Logout => "logout",
            SessionAction::Suspend => "suspend",
            SessionAction::Hibernate => "hibernate",
            SessionAction::Reboot => "reboot",
            SessionAction::Shutdown => "shutdown",
        }
    }

    /// The action's name in the current language.
    pub fn label(self) -> &'static str {
        match self {
            SessionAction::Lock => t("Lock"),
            SessionAction::Logout => t("Log out"),
            SessionAction::Suspend => t("Sleep"),
            SessionAction::Hibernate => t("Hibernate"),
            SessionAction::Reboot => t("Restart"),
            SessionAction::Shutdown => t("Shut down"),
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }

    /// Whether the action ends the session or powers the machine off.
    pub fn is_destructive(self) -> bool {
        matches!(self, SessionAction::Logout | SessionAction::Reboot | SessionAction::Shutdown)
    }

    /// The logind call this action makes (documentation / diagnostics).
    pub fn describe(self) -> &'static str {
        match self {
            SessionAction::Lock => "login1.Session.Lock",
            SessionAction::Logout => "compositor quit / login1.Session.Terminate",
            SessionAction::Suspend => "login1.Manager.Suspend",
            SessionAction::Hibernate => "login1.Manager.Hibernate",
            SessionAction::Reboot => "login1.Manager.Reboot",
            SessionAction::Shutdown => "login1.Manager.PowerOff",
        }
    }

    /// Runs the action over logind's D-Bus API (no external tools). Logout
    /// asks MangoWM to quit when it is running, else terminates the session.
    pub fn run(self) -> Result<(), String> {
        crate::runtime::block_on(self.call())
    }

    async fn call(self) -> Result<(), String> {
        use crate::actor::proxy;
        let err = |e: zbus::Error| tf("{action} failed ({call}): {error}", &[("action", &self.label()), ("call", &self.describe()), ("error", &e)]);
        if self == SessionAction::Logout
            && let Some(sock) = std::env::var_os("MANGO_INSTANCE_SIGNATURE")
        {
            match mango_quit(std::path::Path::new(&sock)) {
                Ok(()) => return Ok(()),
                // A stale socket (MangoWM restarted, or a shell started
                // outside it): end the session through logind instead.
                Err(MangoQuit::Unreachable(e)) => tracing::warn!("log out: MangoWM's socket: {e}; asking logind"),
                Err(MangoQuit::Refused(e)) => return Err(tf("{action} failed ({call}): {error}", &[("action", &self.label()), ("call", &"MangoWM dispatch quit"), ("error", &e)])),
            }
        }
        let conn = crate::runtime::system().await.map_err(err)?;
        const DEST: &str = "org.freedesktop.login1";
        match self {
            SessionAction::Lock | SessionAction::Logout => {
                // This shell's own session (the one the lock screen answers
                // for), not logind's "auto" guess.
                let path = crate::logind::session_path(&conn).await.map_err(err)?;
                let session = proxy(&conn, DEST, path.as_str(), "org.freedesktop.login1.Session").await.map_err(err)?;
                let method = if self == SessionAction::Lock { "Lock" } else { "Terminate" };
                session.call::<_, _, ()>(method, &()).await.map_err(err)
            }
            _ => {
                let manager = proxy(&conn, DEST, "/org/freedesktop/login1", "org.freedesktop.login1.Manager").await.map_err(err)?;
                let method = match self {
                    SessionAction::Suspend => "Suspend",
                    SessionAction::Hibernate => "Hibernate",
                    SessionAction::Reboot => "Reboot",
                    _ => "PowerOff",
                };
                // `interactive = true` lets polkit prompt when required.
                manager.call::<_, _, ()>(method, &(true,)).await.map_err(err)
            }
        }
    }
}

/// How long MangoWM gets to acknowledge `dispatch quit`.
const MANGO_REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Why MangoWM didn't quit.
#[derive(Debug, PartialEq)]
enum MangoQuit {
    /// Its socket couldn't be reached.
    Unreachable(String),
    /// It answered with an error.
    Refused(String),
}

/// Asks MangoWM to quit (ending the session it runs) and waits for its
/// answer: MangoWM only carries out a command once it has written the reply,
/// so a client that hangs up straight after sending it never logs out.
fn mango_quit(sock: &std::path::Path) -> Result<(), MangoQuit> {
    use std::io::{BufRead as _, Write as _};
    let mut stream = std::os::unix::net::UnixStream::connect(sock).map_err(|e| MangoQuit::Unreachable(e.to_string()))?;
    let _ = stream.set_read_timeout(Some(MANGO_REPLY_TIMEOUT));
    writeln!(stream, "dispatch quit").map_err(|e| MangoQuit::Unreachable(e.to_string()))?;
    let mut reply = String::new();
    // No reply (it quit before answering, or timed out) still counts: the
    // command was delivered.
    let _ = std::io::BufReader::new(&stream).read_line(&mut reply);
    mango_reply_error(&reply).map_or(Ok(()), |e| Err(MangoQuit::Refused(e)))
}

/// The error in a MangoWM reply line (`{"error":"…"}`), if any.
fn mango_reply_error(reply: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(reply.trim()).ok()?;
    v.get("error").and_then(|e| e.as_str()).map(str::to_string)
}

/// Holds a logind idle/sleep inhibitor; dropping it releases the lock.
pub struct Inhibitor(#[allow(dead_code)] std::os::fd::OwnedFd);

/// Blocks idle and sleep (`Inhibit("idle:sleep", …, "block")`).
pub fn inhibit_idle(why: &str) -> Result<Inhibitor, String> {
    let why = why.to_string();
    crate::runtime::block_on(async move {
        {
            let conn = crate::runtime::system().await.map_err(|e| e.to_string())?;
            let manager = crate::actor::proxy(&conn, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager")
                .await
                .map_err(|e| e.to_string())?;
            let fd: zbus::zvariant::OwnedFd = manager
                .call("Inhibit", &("idle:sleep", "Mandara Shell", why.as_str(), "block"))
                .await
                .map_err(|e| tf("Could not keep the screen awake: {error}", &[("error", &e)]))?;
            Ok(Inhibitor(fd.into()))
        }
    })
}

/// Apps chosen in Studio → Applications for links / folders Mandara opens
/// (`None` = the system default through the portal).
static PREFERRED: std::sync::RwLock<(Option<String>, Option<String>)> = std::sync::RwLock::new((None, None));

/// Sets the preferred browser and file manager commands (empty = default).
pub fn set_preferred_openers(browser: &str, file_manager: &str) {
    let pick = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    if let Ok(mut p) = PREFERRED.write() {
        *p = (pick(browser), pick(file_manager));
    }
}

/// The argv a preferred app would open `uri` with, if one is set for it.
pub fn preferred_argv(uri: &str, browser: Option<&str>, file_manager: Option<&str>) -> Option<Vec<String>> {
    let (program, target) = if uri.starts_with("http://") || uri.starts_with("https://") {
        (browser?, uri.to_string())
    } else {
        let path = uri.strip_prefix("file://")?;
        // Drives send encoded URIs (`/run/media/u/My%20Disk`).
        let decoded = crate::mpris::percent_decode(path);
        let path = std::path::Path::new(&decoded);
        if !path.is_dir() {
            return None;
        }
        (file_manager?, path.display().to_string())
    };
    let mut argv: Vec<String> = program.split_whitespace().map(String::from).collect();
    argv.push(target);
    Some(argv)
}

/// Opens a URL with the user's default handler through the desktop portal
/// (`org.freedesktop.portal.OpenURI`) — pure D-Bus, no `xdg-open` — or with
/// the browser / file manager picked in Studio → Applications.
pub fn open_uri(uri: &str) -> Result<(), String> {
    let preferred = PREFERRED.read().ok().and_then(|p| preferred_argv(uri, p.0.as_deref(), p.1.as_deref()));
    if let Some(argv) = preferred {
        use std::os::unix::process::CommandExt as _;
        return crate::reaper::spawn(std::process::Command::new(&argv[0]).args(&argv[1..]).process_group(0)).map(|_| ()).map_err(|e| tf("Could not open {uri}: {error}", &[("uri", &uri), ("error", &e)]));
    }
    let uri = uri.to_string();
    crate::runtime::block_on(async move {
        {
            let conn = crate::runtime::session().await.map_err(|e| e.to_string())?;
            let portal = crate::actor::proxy(&conn, "org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop", "org.freedesktop.portal.OpenURI")
                .await
                .map_err(|e| e.to_string())?;
            let options: std::collections::HashMap<&str, zbus::zvariant::Value> = std::collections::HashMap::new();
            portal
                .call::<_, _, zbus::zvariant::OwnedObjectPath>("OpenURI", &("", uri.as_str(), options))
                .await
                .map(|_| ())
                .map_err(|e| tf("Could not open {uri}: {error}", &[("uri", &uri), ("error", &e)]))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_destructiveness() {
        for a in SessionAction::ALL {
            assert_eq!(SessionAction::from_id(a.id()), Some(a));
            assert!(a.describe().contains("login1") || a == SessionAction::Logout);
        }
        assert!(SessionAction::Shutdown.is_destructive());
        assert!(!SessionAction::Lock.is_destructive());
    }

    /// The session dock's Log out did nothing on MangoWM: it hung up right
    /// after sending `dispatch quit`, and MangoWM only acts on a command
    /// once it has answered. A fake MangoWM that answers late checks that
    /// the answer is awaited.
    #[test]
    fn logout_waits_for_mango_to_answer() {
        use std::io::{BufRead as _, Write as _};
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("mango.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(&stream).read_line(&mut line).unwrap();
            std::thread::sleep(ANSWER_AFTER);
            (&stream).write_all(b"{\"success\":true}\n").unwrap();
            line
        });
        const ANSWER_AFTER: std::time::Duration = std::time::Duration::from_millis(100);
        let start = std::time::Instant::now();
        assert_eq!(mango_quit(&sock), Ok(()));
        assert!(start.elapsed() >= ANSWER_AFTER, "returned before MangoWM answered");
        assert_eq!(server.join().unwrap(), "dispatch quit\n");
        assert!(matches!(mango_quit(&dir.path().join("missing.sock")), Err(MangoQuit::Unreachable(_))));
        assert_eq!(mango_reply_error(r#"{"error":"unknown dispatcher"}"#).as_deref(), Some("unknown dispatcher"));
        assert_eq!(mango_reply_error(r#"{"success":true}"#), None);
        assert_eq!(mango_reply_error(""), None);
    }

    #[test]
    fn preferred_openers() {
        assert_eq!(preferred_argv("https://x.org", Some("firefox --new-tab"), None), Some(vec!["firefox".into(), "--new-tab".into(), "https://x.org".into()]));
        assert_eq!(preferred_argv("https://x.org", None, Some("thunar")), None);
        let dir = std::env::temp_dir();
        assert_eq!(preferred_argv(&format!("file://{}", dir.display()), None, Some("thunar")), Some(vec!["thunar".into(), dir.display().to_string()]));
        assert_eq!(preferred_argv("trash:///", Some("b"), Some("f")), None);
        // A folder with a space arrives percent-encoded (drives).
        let spaced = tempfile::tempdir().unwrap();
        let folder = spaced.path().join("My Disk");
        std::fs::create_dir(&folder).unwrap();
        let uri = format!("file://{}", folder.display().to_string().replace(' ', "%20"));
        assert_eq!(preferred_argv(&uri, None, Some("nautilus -w")), Some(vec!["nautilus".into(), "-w".into(), folder.display().to_string()]));
    }
}
