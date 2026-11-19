//! Session / power actions and idle inhibition over logind's D-Bus API
//! (ConsoleKit2's on FreeBSD, with the base system's `shutdown` and
//! `acpiconf` as the fallback).

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
    #[cfg(target_os = "freebsd")]
    pub fn describe(self) -> &'static str {
        match self {
            SessionAction::Lock => "ConsoleKit.Session.Lock",
            SessionAction::Logout => "compositor quit",
            SessionAction::Suspend => "ConsoleKit.Manager.Suspend / acpiconf -s 3",
            SessionAction::Hibernate => "ConsoleKit.Manager.Hibernate",
            SessionAction::Reboot => "ConsoleKit.Manager.Reboot / shutdown -r now",
            SessionAction::Shutdown => "ConsoleKit.Manager.PowerOff / shutdown -p now",
        }
    }

    /// The logind call this action makes (documentation / diagnostics).
    #[cfg(not(target_os = "freebsd"))]
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
    /// asks the running compositor to quit first (MangoWM, Niri, Hyprland,
    /// Sway); only when that socket is missing does it fall back to logind
    /// `Session.Terminate`.
    pub fn run(self) -> Result<(), String> {
        crate::runtime::block_on(self.call())
    }

    async fn call(self) -> Result<(), String> {
        use crate::actor::proxy;
        let err = |e: zbus::Error| tf("{action} failed ({call}): {error}", &[("action", &self.label()), ("call", &self.describe()), ("error", &e)]);
        if self == SessionAction::Logout {
            match compositor_quit() {
                Ok(how) => {
                    tracing::info!("log out: {how}");
                    return Ok(());
                }
                // No compositor socket, or a stale one: end the session
                // through logind instead.
                Err(CompositorQuit::Unreachable(e)) => tracing::warn!("log out: {e}; asking logind"),
                Err(CompositorQuit::Refused { call, error }) => {
                    return Err(tf("{action} failed ({call}): {error}", &[("action", &self.label()), ("call", &call), ("error", &error)]));
                }
            }
        }
        #[cfg(target_os = "freebsd")]
        if self == SessionAction::Logout {
            // ConsoleKit2 sessions can't be terminated over D-Bus; the
            // compositor ends the session when it quits.
            return Err(tf("{action} failed ({call}): {error}", &[("action", &self.label()), ("call", &self.describe()), ("error", &t("Quit the compositor to log out"))]));
        }
        use crate::logind::bus::{DEST, MANAGER, MANAGER_PATH, SESSION};
        match self {
            SessionAction::Lock | SessionAction::Logout => {
                let conn = crate::runtime::system().await.map_err(err)?;
                // This shell's own session (the one the lock screen answers
                // for), not logind's "auto" guess.
                let path = crate::logind::session_path(&conn).await.map_err(err)?;
                let session = proxy(&conn, DEST, path.as_str(), SESSION).await.map_err(err)?;
                let method = if self == SessionAction::Lock { "Lock" } else { "Terminate" };
                session.call::<_, _, ()>(method, &()).await.map_err(err)
            }
            _ => {
                let method = match self {
                    SessionAction::Suspend => "Suspend",
                    SessionAction::Hibernate => "Hibernate",
                    SessionAction::Reboot => "Reboot",
                    _ => "PowerOff",
                };
                // A system bus that can't be reached counts like a manager
                // that isn't on it (FreeBSD falls back below).
                let result = async {
                    let conn = crate::runtime::system().await?;
                    let manager = proxy(&conn, DEST, MANAGER_PATH, MANAGER).await?;
                    // `interactive = true` lets polkit prompt when required.
                    manager.call::<_, _, ()>(method, &(true,)).await
                }
                .await;
                // FreeBSD without ConsoleKit2: the base system's own tools,
                // as root through pkexec and Mandara's password prompt.
                #[cfg(target_os = "freebsd")]
                if let Err(e) = &result
                    && crate::freebsd::power::bus_missing(e)
                {
                    return crate::freebsd::power::run(method, crate::freebsd::power::Via::Pkexec).map_err(|e| tf("{action} failed ({call}): {error}", &[("action", &self.label()), ("call", &self.describe()), ("error", &e)]));
                }
                result.map_err(err)
            }
        }
    }
}

/// How long a compositor gets to acknowledge a quit.
const QUIT_REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Why the compositor didn't quit.
#[derive(Debug, PartialEq)]
enum CompositorQuit {
    /// No known compositor socket, or it couldn't be reached.
    Unreachable(String),
    /// It answered with an error.
    Refused { call: String, error: String },
}

/// Asks the running compositor to quit so its session wrapper (niri-session,
/// etc.) can tear down `graphical-session.target` cleanly. logind
/// `Session.Terminate` alone kills that wrapper first and leaves long-lived
/// user services (like `niri.service`) behind — see #209.
fn compositor_quit() -> Result<&'static str, CompositorQuit> {
    compositor_quit_with(|key| std::env::var_os(key))
}

/// Looks up compositor sockets through `env` so tests can pass fixed values
/// without mutating process-wide environment variables.
fn compositor_quit_with(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Result<&'static str, CompositorQuit> {
    let mut last_unreachable = None;
    let mut try_one = |label: &'static str, result: Result<(), CompositorQuit>| -> Option<Result<&'static str, CompositorQuit>> {
        match result {
            Ok(()) => Some(Ok(label)),
            Err(CompositorQuit::Refused { call, error }) => Some(Err(CompositorQuit::Refused { call, error })),
            Err(CompositorQuit::Unreachable(e)) => {
                last_unreachable = Some(e);
                None
            }
        }
    };
    if let Some(sock) = env("MANGO_INSTANCE_SIGNATURE")
        && let Some(done) = try_one("MangoWM dispatch quit", mango_quit(std::path::Path::new(&sock)))
    {
        return done;
    }
    if let Some(sock) = env("NIRI_SOCKET")
        && let Some(done) = try_one("niri Action::Quit", niri_quit(std::path::Path::new(&sock)))
    {
        return done;
    }
    if let Some(sig) = env("HYPRLAND_INSTANCE_SIGNATURE") {
        let result = match env("XDG_RUNTIME_DIR") {
            Some(runtime) => hyprland_exit(std::path::Path::new(&runtime).join("hypr").join(sig).join(".socket.sock")),
            None => Err(CompositorQuit::Unreachable("HYPRLAND_INSTANCE_SIGNATURE set but XDG_RUNTIME_DIR is missing".into())),
        };
        if let Some(done) = try_one("hyprland dispatch exit", result) {
            return done;
        }
    }
    if let Some(sock) = env("SWAYSOCK").or_else(|| env("I3SOCK"))
        && let Some(done) = try_one("sway/i3 exit", sway_exit(std::path::Path::new(&sock)))
    {
        return done;
    }
    Err(CompositorQuit::Unreachable(last_unreachable.unwrap_or_else(|| "no compositor quit socket in the environment".into())))
}

/// Asks MangoWM to quit (ending the session it runs) and waits for its
/// answer: MangoWM only carries out a command once it has written the reply,
/// so a client that hangs up straight after sending it never logs out.
fn mango_quit(sock: &std::path::Path) -> Result<(), CompositorQuit> {
    use std::io::{BufRead as _, Write as _};
    let mut stream = std::os::unix::net::UnixStream::connect(sock).map_err(|e| CompositorQuit::Unreachable(format!("MangoWM socket: {e}")))?;
    let _ = stream.set_read_timeout(Some(QUIT_REPLY_TIMEOUT));
    writeln!(stream, "dispatch quit").map_err(|e| CompositorQuit::Unreachable(format!("MangoWM socket: {e}")))?;
    let mut reply = String::new();
    // No reply (it quit before answering, or timed out) still counts: the
    // command was delivered.
    let _ = std::io::BufReader::new(&stream).read_line(&mut reply);
    match mango_reply_error(&reply) {
        Some(error) => Err(CompositorQuit::Refused { call: "MangoWM dispatch quit".into(), error }),
        None => Ok(()),
    }
}

/// The error in a MangoWM reply line (`{"error":"…"}`), if any.
fn mango_reply_error(reply: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(reply.trim()).ok()?;
    v.get("error").and_then(|e| e.as_str()).map(str::to_string)
}

/// `{"Action":{"Quit":{"skip_confirmation":true}}}` on `$NIRI_SOCKET`.
fn niri_quit(sock: &std::path::Path) -> Result<(), CompositorQuit> {
    use std::io::{BufRead as _, Write as _};
    let mut stream = std::os::unix::net::UnixStream::connect(sock).map_err(|e| CompositorQuit::Unreachable(format!("niri socket: {e}")))?;
    let _ = stream.set_read_timeout(Some(QUIT_REPLY_TIMEOUT));
    // skip_confirmation: the session menu already confirmed.
    writeln!(stream, r#"{{"Action":{{"Quit":{{"skip_confirmation":true}}}}}}"#).map_err(|e| CompositorQuit::Unreachable(format!("niri socket: {e}")))?;
    let mut reply = String::new();
    match std::io::BufReader::new(&stream).read_line(&mut reply) {
        // Clean close with no bytes: niri quit before answering.
        Ok(0) => Ok(()),
        Ok(_) => match niri_reply_error(&reply) {
            Some(error) => Err(CompositorQuit::Refused { call: "niri Action::Quit".into(), error }),
            None => Ok(()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
            Err(CompositorQuit::Unreachable(format!("niri socket: {e}")))
        }
        // Broken pipe / connection reset after accept: treat as delivered.
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof || e.kind() == std::io::ErrorKind::ConnectionReset || e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(CompositorQuit::Unreachable(format!("niri socket: {e}"))),
    }
}

/// The error in a niri reply (`{"Err":…}`), if any.
fn niri_reply_error(reply: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(reply.trim()).ok()?;
    v.get("Err").map(|e| match e {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

/// `dispatch exit` on Hyprland's `.socket.sock`.
fn hyprland_exit(sock: impl AsRef<std::path::Path>) -> Result<(), CompositorQuit> {
    use std::io::{Read as _, Write as _};
    let sock = sock.as_ref();
    let mut stream = std::os::unix::net::UnixStream::connect(sock).map_err(|e| CompositorQuit::Unreachable(format!("Hyprland socket: {e}")))?;
    let _ = stream.set_read_timeout(Some(QUIT_REPLY_TIMEOUT));
    stream.write_all(b"dispatch exit").map_err(|e| CompositorQuit::Unreachable(format!("Hyprland socket: {e}")))?;
    let mut reply = String::new();
    match stream.read_to_string(&mut reply) {
        Ok(0) => Ok(()),
        Ok(_) => {
            let trimmed = reply.trim();
            if trimmed.is_empty() || trimmed == "ok" || trimmed.starts_with("ok\n") {
                Ok(())
            } else {
                Err(CompositorQuit::Refused { call: "hyprland dispatch exit".into(), error: trimmed.to_string() })
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
            Err(CompositorQuit::Unreachable(format!("Hyprland socket: {e}")))
        }
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof || e.kind() == std::io::ErrorKind::ConnectionReset || e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(CompositorQuit::Unreachable(format!("Hyprland socket: {e}"))),
    }
}

/// i3-compatible `exit` on `$SWAYSOCK` / `$I3SOCK`.
fn sway_exit(sock: &std::path::Path) -> Result<(), CompositorQuit> {
    // Prefer the library path when linked; otherwise speak the i3 IPC payload
    // by hand so Mandara stays free of an optional sway dependency here.
    use std::io::{Read as _, Write as _};
    let mut stream = std::os::unix::net::UnixStream::connect(sock).map_err(|e| CompositorQuit::Unreachable(format!("Sway/i3 socket: {e}")))?;
    let _ = stream.set_read_timeout(Some(QUIT_REPLY_TIMEOUT));
    let payload = b"exit";
    let mut message = Vec::with_capacity(14 + payload.len());
    message.extend_from_slice(b"i3-ipc");
    message.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
    message.extend_from_slice(&0u32.to_ne_bytes()); // RUN_COMMAND
    message.extend_from_slice(payload);
    stream.write_all(&message).map_err(|e| CompositorQuit::Unreachable(format!("Sway/i3 socket: {e}")))?;
    let mut header = [0u8; 14];
    match stream.read_exact(&mut header) {
        Ok(()) => {}
        // Compositor gone before answering: the exit was delivered.
        Err(_) => return Ok(()),
    }
    if &header[..6] != b"i3-ipc" {
        return Err(CompositorQuit::Refused { call: "sway/i3 exit".into(), error: t("malformed IPC reply").into() });
    }
    let len = u32::from_ne_bytes(header[6..10].try_into().unwrap()) as usize;
    let mut body = vec![0u8; len.min(4096)];
    let _ = stream.read_exact(&mut body);
    Ok(())
}

/// Holds a logind idle/sleep inhibitor; dropping it releases the lock.
pub struct Inhibitor(#[allow(dead_code)] std::os::fd::OwnedFd);

/// Blocks idle and sleep (`Inhibit("idle:sleep", …, "block")`).
pub fn inhibit_idle(why: &str) -> Result<Inhibitor, String> {
    let why = why.to_string();
    crate::runtime::block_on(async move {
        {
            let conn = crate::runtime::system().await.map_err(|e| e.to_string())?;
            use crate::logind::bus;
            let manager = crate::actor::proxy(&conn, bus::DEST, bus::MANAGER_PATH, bus::MANAGER)
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
            assert!(a.describe().contains(crate::logind::bus::SHORT) || a == SessionAction::Logout);
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
        assert!(matches!(mango_quit(&dir.path().join("missing.sock")), Err(CompositorQuit::Unreachable(_))));
        assert_eq!(mango_reply_error(r#"{"error":"unknown dispatcher"}"#).as_deref(), Some("unknown dispatcher"));
        assert_eq!(mango_reply_error(r#"{"success":true}"#), None);
        assert_eq!(mango_reply_error(""), None);
    }

    /// Niri must get `Quit` with skip_confirmation so niri-session can stop
    /// `niri.service`; Terminate alone leaves that service running (#209).
    #[test]
    fn logout_asks_niri_to_quit_without_confirmation() {
        use std::io::{BufRead as _, Write as _};
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("niri.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(&stream).read_line(&mut line).unwrap();
            (&stream).write_all(b"{\"Ok\":{\"Handled\":null}}\n").unwrap();
            line
        });
        assert_eq!(niri_quit(&sock), Ok(()));
        let sent = server.join().unwrap();
        assert!(sent.contains(r#""Quit""#) && sent.contains(r#""skip_confirmation":true"#), "{sent}");
        assert_eq!(niri_reply_error(r#"{"Err":"busy"}"#).as_deref(), Some("busy"));
        assert_eq!(niri_reply_error(r#"{"Ok":{"Handled":null}}"#), None);
    }

    #[test]
    fn logout_asks_hyprland_to_exit() {
        use std::io::{Read as _, Write as _};
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("hypr.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = vec![0u8; 64];
            let n = stream.read(&mut buf).unwrap();
            stream.write_all(b"ok").unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        assert_eq!(hyprland_exit(&sock), Ok(()));
        assert_eq!(server.join().unwrap(), "dispatch exit");
    }

    #[test]
    fn compositor_quit_prefers_niri_over_logind_when_socket_is_set() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("niri.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            use std::io::{BufRead as _, Write as _};
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(&stream).read_line(&mut line).unwrap();
            (&stream).write_all(b"{\"Ok\":{\"Handled\":null}}\n").unwrap();
            line
        });
        let sock_os = sock.as_os_str().to_owned();
        let env = |key: &str| match key {
            "NIRI_SOCKET" => Some(sock_os.clone()),
            _ => None,
        };
        assert_eq!(compositor_quit_with(env), Ok("niri Action::Quit"));
        assert!(server.join().unwrap().contains("Quit"));
    }

    #[test]
    fn compositor_quit_skips_a_stale_mango_socket_and_tries_niri() {
        let dir = tempfile::tempdir().unwrap();
        let niri = dir.path().join("niri.sock");
        let listener = std::os::unix::net::UnixListener::bind(&niri).unwrap();
        let server = std::thread::spawn(move || {
            use std::io::{BufRead as _, Write as _};
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(&stream).read_line(&mut line).unwrap();
            (&stream).write_all(b"{\"Ok\":{\"Handled\":null}}\n").unwrap();
            line
        });
        let stale = dir.path().join("missing-mango.sock");
        let niri_os = niri.as_os_str().to_owned();
        let stale_os = stale.as_os_str().to_owned();
        let env = |key: &str| match key {
            "MANGO_INSTANCE_SIGNATURE" => Some(stale_os.clone()),
            "NIRI_SOCKET" => Some(niri_os.clone()),
            _ => None,
        };
        assert_eq!(compositor_quit_with(env), Ok("niri Action::Quit"));
        assert!(server.join().unwrap().contains("Quit"));
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
