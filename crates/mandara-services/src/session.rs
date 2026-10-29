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
            use std::io::Write;
            let mut stream = std::os::unix::net::UnixStream::connect(sock).map_err(|e| e.to_string())?;
            return writeln!(stream, "dispatch quit").map_err(|e| e.to_string());
        }
        let conn = crate::runtime::system().await.map_err(err)?;
        const DEST: &str = "org.freedesktop.login1";
        match self {
            SessionAction::Lock | SessionAction::Logout => {
                let session = proxy(&conn, DEST, "/org/freedesktop/login1/session/auto", "org.freedesktop.login1.Session").await.map_err(err)?;
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
    } else if let Some(path) = uri.strip_prefix("file://") {
        // Drives send encoded URIs (`/run/media/u/My%20Disk`).
        let decoded = crate::mpris::percent_decode(path);
        let path = std::path::Path::new(&decoded);
        if !path.is_dir() {
            return None;
        }
        (file_manager?, path.display().to_string())
    } else {
        return None;
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
