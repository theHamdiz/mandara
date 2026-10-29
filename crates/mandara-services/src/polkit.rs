//! Polkit authentication agent (`org.freedesktop.PolicyKit1.AuthenticationAgent`).
//!
//! When an app asks polkit for a privileged action that needs a password,
//! polkitd calls `BeginAuthentication` on the agent registered for the
//! user's session. Mandara registers one on the system bus (subject:
//! `unix-session` with the logind session id) unless another agent already
//! holds the session — then polkitd refuses and we just log it.
//!
//! The password is never checked here: it goes to polkit's own helper,
//! which runs PAM as root and reports the result straight to polkitd. The
//! helper is reached through the socket-activated
//! `/run/polkit/agent-helper.socket` (polkit ≥ 126), falling back to the
//! classic setuid `polkit-agent-helper-1`. Its line protocol:
//!
//! ```text
//! → <user>\n            (socket mode only; setuid mode takes it as argv[1])
//! → <cookie>\n
//! ← PAM_PROMPT_ECHO_OFF Password:      answer with "<secret>\n"
//! ← PAM_PROMPT_ECHO_ON <prompt>        answer with "<text>\n"
//! ← PAM_ERROR_MSG <text> / PAM_TEXT_INFO <text>
//! ← SUCCESS | FAILURE
//! ```
//!
//! The UI (a modal dialog in mandara-ui) talks to this actor through
//! [`PolkitEvent`]s and [`PolkitReply`]s keyed by the request's cookie.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::Arc;

use futures::FutureExt as _;
use mandara_i18n::t;
use parking_lot::Mutex;
use serde::Serialize;
use zbus::interface;
use zbus::zvariant::{OwnedValue, Value};

/// Events (prompts, results) buffered for the UI and replies buffered for
/// a session; a dialog exchanges a handful of each.
const EVENT_QUEUE: usize = 64;
const REPLY_QUEUE: usize = 16;
/// Helper output lines buffered before the reader thread waits.
const HELPER_LINE_QUEUE: usize = 32;

pub const AGENT_PATH: &str = "/org/mandara/PolicyKit1/AuthenticationAgent";
pub const HELPER_SOCKET: &str = "/run/polkit/agent-helper.socket";
pub const HELPER_PATHS: [&str; 3] =
    ["/usr/lib/polkit-1/polkit-agent-helper-1", "/usr/libexec/polkit-agent-helper-1", "/usr/lib/policykit-1/polkit-agent-helper-1"];

/// Someone who may authenticate a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AdminUser {
    pub uid: u32,
    pub name: String,
    /// Full name from GECOS (falls back to `name`).
    pub display: String,
}

/// A pending authentication, as shown in the dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct AuthRequest {
    pub cookie: String,
    pub action_id: String,
    pub message: String,
    pub icon_name: String,
    pub details: Vec<(String, String)>,
    /// The current user first when eligible.
    pub users: Vec<AdminUser>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Authorized,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentStatus {
    Registered { session: String },
    /// Another agent (polkit-gnome, KDE, hyprpolkitagent…) holds the session.
    AnotherAgent(String),
    Disabled,
    Unavailable(String),
}

/// Agent → UI.
#[derive(Clone, Debug, PartialEq)]
pub enum PolkitEvent {
    Begin(AuthRequest),
    /// The helper asks for input (`echo = false`: a password).
    Prompt { cookie: String, user: String, text: String, echo: bool },
    Message { cookie: String, text: String, error: bool },
    /// The last answer was wrong; a fresh prompt follows.
    Retry { cookie: String, message: String },
    Close { cookie: String, outcome: Outcome },
    Status(AgentStatus),
}

/// UI → agent.
#[derive(Clone, Debug, PartialEq)]
pub enum PolkitReply {
    SelectUser { cookie: String, user: String },
    Answer { cookie: String, text: String },
    Cancel { cookie: String },
}

impl PolkitReply {
    fn cookie(&self) -> &str {
        match self {
            PolkitReply::SelectUser { cookie, .. } | PolkitReply::Answer { cookie, .. } | PolkitReply::Cancel { cookie } => cookie,
        }
    }
}

/// Handle held by the UI.
#[derive(Clone)]
pub struct PolkitAgent {
    pub events: async_channel::Receiver<PolkitEvent>,
    replies: async_channel::Sender<PolkitReply>,
    /// Closed by [`PolkitAgent::stop`]: the agent drops its bus
    /// connection, which unregisters it from polkit.
    stop: async_channel::Sender<()>,
    /// Set by [`PolkitAgent::stop`] (shared by every clone).
    stopped: Arc<std::sync::atomic::AtomicBool>,
}

impl PolkitAgent {
    /// A handle over arbitrary channels (previews and tests).
    pub fn from_channels(events: async_channel::Receiver<PolkitEvent>, replies: async_channel::Sender<PolkitReply>) -> Self {
        Self { events, replies, stop: async_channel::bounded(1).0, stopped: Arc::default() }
    }

    pub fn reply(&self, reply: PolkitReply) {
        let _ = self.replies.try_send(reply);
    }

    /// Unregisters the agent (General → Authentication dialogs off);
    /// pending requests are left to polkit, which cancels them.
    pub fn stop(&self) {
        self.stopped.store(true, std::sync::atomic::Ordering::Relaxed);
        self.stop.close();
    }

    /// Whether [`PolkitAgent::stop`] was called on any clone.
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(std::sync::atomic::Ordering::Relaxed)
    }
}

// ───────────────────────────── helper protocol ─────────────────────────────

/// One line from `polkit-agent-helper-1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelperLine {
    Prompt { text: String, echo: bool },
    Error(String),
    Info(String),
    Success,
    Failure,
    Unknown(String),
}

/// Reverses `g_strescape` (`\n`, `\t`, `\\`, `\"`, octal `\NNN`, …).
pub fn unescape(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' || i + 1 == bytes.len() {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        i += 1;
        match bytes[i] {
            b'0'..=b'7' => {
                let mut v: u32 = 0;
                let mut n = 0;
                while n < 3 && i < bytes.len() && (b'0'..=b'7').contains(&bytes[i]) {
                    v = v * 8 + (bytes[i] - b'0') as u32;
                    i += 1;
                    n += 1;
                }
                out.push(v as u8);
                continue;
            }
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'v' => out.push(11),
            other => out.push(other),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_helper_line(line: &str) -> HelperLine {
    let line = line.trim_end_matches(['\n', '\r']);
    let rest = |prefix: &str| line.strip_prefix(prefix).map(|r| unescape(r.strip_prefix(' ').unwrap_or(r)));
    if let Some(text) = rest("PAM_PROMPT_ECHO_OFF") {
        HelperLine::Prompt { text, echo: false }
    } else if let Some(text) = rest("PAM_PROMPT_ECHO_ON") {
        HelperLine::Prompt { text, echo: true }
    } else if let Some(text) = rest("PAM_ERROR_MSG") {
        HelperLine::Error(text)
    } else if let Some(text) = rest("PAM_TEXT_INFO") {
        HelperLine::Info(text)
    } else if line == "SUCCESS" {
        HelperLine::Success
    } else if line == "FAILURE" {
        HelperLine::Failure
    } else {
        HelperLine::Unknown(line.to_string())
    }
}

/// A running helper: its output, its input, and how to stop it.
pub struct HelperConn {
    pub reader: Box<dyn BufRead + Send>,
    pub writer: Box<dyn Write + Send>,
    pub kill: Box<dyn FnMut() + Send>,
}

/// Starts helpers (the real one, or a mock in tests).
pub trait HelperSpawner: Send + Sync {
    fn spawn(&self, user: &str, cookie: &str) -> std::io::Result<HelperConn>;
}

/// polkit's helper: socket-activated first, setuid binary second.
pub struct SystemHelper;

impl HelperSpawner for SystemHelper {
    fn spawn(&self, user: &str, cookie: &str) -> std::io::Result<HelperConn> {
        use std::os::unix::net::UnixStream;
        match UnixStream::connect(HELPER_SOCKET) {
            Ok(stream) => {
                let mut writer = stream.try_clone()?;
                writeln!(writer, "{user}")?;
                writeln!(writer, "{cookie}")?;
                writer.flush()?;
                let reader = BufReader::new(stream.try_clone()?);
                return Ok(HelperConn {
                    reader: Box::new(reader),
                    writer: Box::new(writer),
                    kill: Box::new(move || {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                    }),
                });
            }
            Err(err) => tracing::debug!("polkit: {HELPER_SOCKET} unavailable ({err}); trying the setuid helper"),
        }
        let path = HELPER_PATHS
            .iter()
            .find(|p| std::path::Path::new(p).exists())
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "polkit-agent-helper-1 not found"))?;
        let mut child = std::process::Command::new(path)
            .arg(user)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let mut stdin = child.stdin.take().ok_or_else(|| std::io::Error::other("helper stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| std::io::Error::other("helper stdout"))?;
        writeln!(stdin, "{cookie}")?;
        stdin.flush()?;
        Ok(HelperConn {
            reader: Box::new(BufReader::new(stdout)),
            writer: Box::new(stdin),
            kill: Box::new(move || {
                let _ = child.kill();
                let _ = child.wait();
            }),
        })
    }
}

/// Reads helper lines on a thread (`None` = the helper hung up).
fn read_lines(mut reader: Box<dyn BufRead + Send>) -> async_channel::Receiver<Option<HelperLine>> {
    let (tx, rx) = async_channel::bounded(HELPER_LINE_QUEUE);
    let _ = std::thread::Builder::new().name("mandara-polkit-helper".into()).spawn(move || {
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => {
                    let _ = tx.send_blocking(None);
                    break;
                }
                Ok(_) => {
                    if tx.send_blocking(Some(parse_helper_line(&line))).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

fn wipe(s: &mut String) {
    // SAFETY: zero bytes are valid UTF-8.
    unsafe { s.as_bytes_mut().fill(0) };
    s.clear();
}

/// Drives one request to its end: runs the helper for the chosen user,
/// relays prompts and answers, retries after a wrong password, follows user
/// switches, and stops on cancel.
pub async fn run_session(
    spawner: &dyn HelperSpawner,
    events: &async_channel::Sender<PolkitEvent>,
    replies: &async_channel::Receiver<PolkitReply>,
    cookie: &str,
    mut user: String,
) -> Outcome {
    let close = |outcome| {
        let _ = events.try_send(PolkitEvent::Close { cookie: cookie.to_string(), outcome });
        outcome
    };
    loop {
        let HelperConn { reader, mut writer, mut kill } = match spawner.spawn(&user, cookie) {
            Ok(conn) => conn,
            Err(err) => {
                tracing::error!("polkit: cannot start the authentication helper: {err}");
                let _ = events.try_send(PolkitEvent::Message { cookie: cookie.into(), text: t("The authentication helper is not available").into(), error: true });
                return close(Outcome::Failed);
            }
        };
        let lines = read_lines(reader);
        let mut answered = false;
        let restart = loop {
            futures::select! {
                line = lines.recv().fuse() => match line.ok().flatten() {
                    Some(HelperLine::Prompt { text, echo }) => {
                        let _ = events.try_send(PolkitEvent::Prompt { cookie: cookie.into(), user: user.clone(), text, echo });
                    }
                    Some(HelperLine::Error(text)) => {
                        let _ = events.try_send(PolkitEvent::Message { cookie: cookie.into(), text, error: true });
                    }
                    Some(HelperLine::Info(text)) => {
                        let _ = events.try_send(PolkitEvent::Message { cookie: cookie.into(), text, error: false });
                    }
                    Some(HelperLine::Success) => {
                        kill();
                        return close(Outcome::Authorized);
                    }
                    Some(HelperLine::Unknown(l)) => tracing::debug!("polkit: unexpected helper line {l:?}"),
                    Some(HelperLine::Failure) | None => {
                        kill();
                        if !answered {
                            // It failed before asking anything: nothing a
                            // retry could fix (broken PAM, bad cookie).
                            tracing::warn!("polkit: the helper failed before prompting");
                            return close(Outcome::Failed);
                        }
                        let _ = events.try_send(PolkitEvent::Retry { cookie: cookie.into(), message: t("That didn't work — try again").into() });
                        break true;
                    }
                },
                reply = replies.recv().fuse() => match reply {
                    Ok(PolkitReply::Answer { mut text, .. }) => {
                        let ok = writeln!(writer, "{text}").and_then(|_| writer.flush()).is_ok();
                        wipe(&mut text);
                        answered |= ok;
                    }
                    Ok(PolkitReply::SelectUser { user: next, .. }) => {
                        kill();
                        user = next;
                        break true;
                    }
                    Ok(PolkitReply::Cancel { .. }) | Err(_) => {
                        kill();
                        return close(Outcome::Cancelled);
                    }
                },
            }
        };
        if !restart {
            return close(Outcome::Failed);
        }
    }
}

// ─────────────────────────────── identities ───────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Identity {
    User(u32),
    Group(u32),
}

/// Parses one `(sa{sv})` polkit identity.
pub fn parse_identity(kind: &str, details: &HashMap<String, OwnedValue>) -> Option<Identity> {
    let id = |key: &str| details.get(key).and_then(|v| u32::try_from(v).ok().or_else(|| i32::try_from(v).ok().map(|i| i as u32)));
    match kind {
        "unix-user" => id("uid").map(Identity::User),
        "unix-group" => id("gid").map(Identity::Group),
        _ => None,
    }
}

/// Expands identities into users (groups → members, incl. primary-group
/// members), current user first, deduplicated.
pub fn expand_identities(identities: &[Identity], passwd: &str, group: &str, me: u32) -> Vec<AdminUser> {
    struct Pw {
        uid: u32,
        gid: u32,
        name: String,
        display: String,
    }
    let users: Vec<Pw> = passwd
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            if f.len() < 7 {
                return None;
            }
            let full = f[4].split(',').next().unwrap_or("").trim();
            Some(Pw { uid: f[2].parse().ok()?, gid: f[3].parse().ok()?, name: f[0].into(), display: if full.is_empty() { f[0].into() } else { full.into() } })
        })
        .collect();
    let members = |gid: u32| -> Vec<String> {
        group
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split(':').collect();
                (f.len() >= 4 && f[2].parse::<u32>().ok() == Some(gid)).then(|| f[3].split(',').filter(|m| !m.is_empty()).map(String::from).collect::<Vec<_>>())
            })
            .flatten()
            .collect()
    };
    let mut out: Vec<AdminUser> = Vec::new();
    let mut push = |pw: &Pw| {
        if !out.iter().any(|u| u.uid == pw.uid) {
            out.push(AdminUser { uid: pw.uid, name: pw.name.clone(), display: pw.display.clone() });
        }
    };
    for identity in identities {
        match *identity {
            Identity::User(uid) => users.iter().filter(|u| u.uid == uid).for_each(&mut push),
            Identity::Group(gid) => {
                let names = members(gid);
                users.iter().filter(|u| u.gid == gid || names.contains(&u.name)).for_each(&mut push);
            }
        }
    }
    if let Some(i) = out.iter().position(|u| u.uid == me) {
        let mine = out.remove(i);
        out.insert(0, mine);
    }
    out
}

// ─────────────────────────────── D-Bus side ───────────────────────────────

struct Shared {
    events: async_channel::Sender<PolkitEvent>,
    sessions: Mutex<HashMap<String, async_channel::Sender<PolkitReply>>>,
    spawner: Arc<dyn HelperSpawner>,
}

struct Agent {
    shared: Arc<Shared>,
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.PolicyKit1.Error")]
enum PolkitError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Failed(String),
    Cancelled(String),
}

#[interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl Agent {
    async fn begin_authentication(
        &self,
        action_id: String,
        message: String,
        icon_name: String,
        details: HashMap<String, String>,
        cookie: String,
        identities: Vec<(String, HashMap<String, OwnedValue>)>,
    ) -> Result<(), PolkitError> {
        let parsed: Vec<Identity> = identities.iter().filter_map(|(k, d)| parse_identity(k, d)).collect();
        let (passwd, group) = crate::runtime::unblock(|| (std::fs::read_to_string("/etc/passwd").unwrap_or_default(), std::fs::read_to_string("/etc/group").unwrap_or_default())).await;
        let users = expand_identities(&parsed, &passwd, &group, rustix::process::getuid().as_raw());
        let Some(first) = users.first().map(|u| u.name.clone()) else {
            return Err(PolkitError::Failed("no user can authenticate this request".into()));
        };
        tracing::info!("polkit: authentication requested for {action_id} ({} candidate user(s))", users.len());
        let (tx, rx) = async_channel::bounded(REPLY_QUEUE);
        self.shared.sessions.lock().insert(cookie.clone(), tx);
        let mut details: Vec<(String, String)> = details.into_iter().collect();
        details.sort();
        let _ = self.shared.events.try_send(PolkitEvent::Begin(AuthRequest { cookie: cookie.clone(), action_id, message, icon_name, details, users }));
        let spawner = self.shared.spawner.clone();
        let events = self.shared.events.clone();
        let outcome = run_session(spawner.as_ref(), &events, &rx, &cookie, first).await;
        self.shared.sessions.lock().remove(&cookie);
        match outcome {
            Outcome::Authorized => Ok(()),
            Outcome::Cancelled => Err(PolkitError::Cancelled("Authentication was cancelled".into())),
            Outcome::Failed => Err(PolkitError::Failed("Authentication failed".into())),
        }
    }

    async fn cancel_authentication(&self, cookie: String) -> Result<(), PolkitError> {
        if let Some(tx) = self.shared.sessions.lock().get(&cookie) {
            let _ = tx.try_send(PolkitReply::Cancel { cookie: cookie.clone() });
        }
        Ok(())
    }
}

/// Registers the agent for this session (unless `enabled` is false) and
/// serves requests on the service runtime.
pub fn spawn_polkit_agent(enabled: bool) -> PolkitAgent {
    let (events_tx, events) = async_channel::bounded(EVENT_QUEUE);
    let (replies, replies_rx) = async_channel::bounded::<PolkitReply>(REPLY_QUEUE);
    let (stop, stop_rx) = async_channel::bounded::<()>(1);
    let agent = PolkitAgent { events, replies, stop, stopped: Arc::default() };
    if !enabled {
        let _ = events_tx.try_send(PolkitEvent::Status(AgentStatus::Disabled));
        return agent;
    }
    let shared = Arc::new(Shared { events: events_tx.clone(), sessions: Mutex::new(HashMap::new()), spawner: Arc::new(SystemHelper) });
    // Route UI replies to the session they belong to.
    {
        let shared = shared.clone();
        crate::runtime::spawn("polkit-router", async move {
            while let Ok(reply) = replies_rx.recv().await {
                if let Some(tx) = shared.sessions.lock().get(reply.cookie()) {
                    let _ = tx.try_send(reply);
                }
            }
        });
    }
    crate::runtime::spawn("polkit", async move {
        {
            let status = match register(shared).await {
                Ok((conn, session)) => {
                    tracing::info!("polkit: authentication agent registered for session {session}");
                    let _ = events_tx.try_send(PolkitEvent::Status(AgentStatus::Registered { session }));
                    // Serve until stopped: dropping the connection makes
                    // polkit forget the agent (its bus name is gone).
                    let _ = stop_rx.recv().await;
                    drop(conn);
                    tracing::info!("polkit: authentication agent stopped");
                    let _ = events_tx.try_send(PolkitEvent::Status(AgentStatus::Disabled));
                    return;
                }
                Err(status) => status,
            };
            match &status {
                AgentStatus::AnotherAgent(msg) => tracing::info!("polkit: not registering — another authentication agent is running ({msg})"),
                AgentStatus::Unavailable(msg) => tracing::warn!("polkit: authentication agent unavailable: {msg}"),
                _ => {}
            }
            let _ = events_tx.try_send(PolkitEvent::Status(status));
        }
    });
    agent
}

async fn register(shared: Arc<Shared>) -> Result<(zbus::Connection, String), AgentStatus> {
    let unavailable = |e: zbus::Error| AgentStatus::Unavailable(e.to_string());
    let builder = zbus::connection::Builder::system().and_then(|b| b.serve_at(AGENT_PATH, Agent { shared })).map_err(unavailable)?;
    let conn = crate::runtime::connect(builder).await.map_err(unavailable)?;
    let session = crate::logind::session_id(&conn).await.map_err(unavailable)?;
    let authority =
        crate::actor::proxy(&conn, "org.freedesktop.PolicyKit1", "/org/freedesktop/PolicyKit1/Authority", "org.freedesktop.PolicyKit1.Authority")
            .await
            .map_err(unavailable)?;
    let mut subject: HashMap<&str, Value> = HashMap::new();
    subject.insert("session-id", Value::from(session.as_str()));
    let locale = std::env::var("LC_ALL").or_else(|_| std::env::var("LANG")).unwrap_or_else(|_| "en_US.UTF-8".into());
    // polkit declares the object path as a string (`s`), not `o`.
    match authority.call::<_, _, ()>("RegisterAuthenticationAgent", &(("unix-session", subject), locale.as_str(), AGENT_PATH)).await {
        Ok(()) => Ok((conn, session)),
        Err(zbus::Error::MethodError(name, msg, _)) if msg.as_deref().is_some_and(|m| m.contains("already exists")) => {
            Err(AgentStatus::AnotherAgent(msg.unwrap_or_else(|| name.to_string())))
        }
        Err(err) => Err(unavailable(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// General → Authentication dialogs off: no agent is registered and the
    /// UI learns why; `stop` reaches every clone of a handle.
    #[test]
    fn disabled_agent_reports_and_handles_stop() {
        let agent = spawn_polkit_agent(false);
        assert!(matches!(agent.events.try_recv(), Ok(PolkitEvent::Status(AgentStatus::Disabled))));
        let clone = agent.clone();
        assert!(!clone.is_stopped());
        agent.stop();
        assert!(clone.is_stopped(), "stopping one handle stops the agent for all");
        let (_etx, erx) = async_channel::bounded(1);
        let (rtx, _rrx) = async_channel::bounded(1);
        let preview = PolkitAgent::from_channels(erx, rtx);
        assert!(!preview.is_stopped());
        preview.stop();
        assert!(preview.is_stopped());
    }

    #[test]
    fn parses_helper_lines() {
        assert_eq!(parse_helper_line("PAM_PROMPT_ECHO_OFF Password: \n"), HelperLine::Prompt { text: "Password: ".into(), echo: false });
        assert_eq!(parse_helper_line("PAM_PROMPT_ECHO_ON login:"), HelperLine::Prompt { text: "login:".into(), echo: true });
        assert_eq!(parse_helper_line("PAM_ERROR_MSG Account locked\\nfor 10 minutes"), HelperLine::Error("Account locked\nfor 10 minutes".into()));
        assert_eq!(parse_helper_line("PAM_TEXT_INFO caf\\303\\251 \\\"x\\\""), HelperLine::Info("café \"x\"".into()));
        assert_eq!(parse_helper_line("SUCCESS\n"), HelperLine::Success);
        assert_eq!(parse_helper_line("FAILURE"), HelperLine::Failure);
        assert_eq!(parse_helper_line("weird"), HelperLine::Unknown("weird".into()));
    }

    const PASSWD: &str = "root:x:0:0:root:/root:/bin/bash\nlayla:x:1000:1000:Layla Nour,,,:/home/layla:/bin/fish\nsara:x:1001:998::/home/sara:/bin/bash\nbob:x:1002:1002:Bob:/home/bob:/bin/sh\n";
    const GROUP: &str = "root:x:0:root\nwheel:x:998:layla,bob\nusers:x:100:\n";

    #[test]
    fn expands_identities_with_current_user_first() {
        let users = expand_identities(&[Identity::User(0), Identity::Group(998)], PASSWD, GROUP, 1002);
        let names: Vec<&str> = users.iter().map(|u| u.name.as_str()).collect();
        // bob (me) first; sara is in wheel through her primary group.
        assert_eq!(names, vec!["bob", "root", "layla", "sara"]);
        assert_eq!(users[2].display, "Layla Nour");
        assert!(expand_identities(&[Identity::Group(4242)], PASSWD, GROUP, 1000).is_empty());
    }

    #[test]
    fn parses_identity_variants() {
        let mut d = HashMap::new();
        d.insert("uid".to_string(), OwnedValue::from(1000u32));
        assert_eq!(parse_identity("unix-user", &d), Some(Identity::User(1000)));
        let mut g = HashMap::new();
        g.insert("gid".to_string(), OwnedValue::from(998u32));
        assert_eq!(parse_identity("unix-group", &g), Some(Identity::Group(998)));
        assert_eq!(parse_identity("unix-netgroup", &g), None);
    }

    /// A scripted stand-in for polkit-agent-helper-1: prompts once, then
    /// accepts the password "right".
    struct MockHelper {
        spawns: Arc<Mutex<Vec<(String, String)>>>,
        kills: Arc<AtomicUsize>,
        /// Fail immediately without prompting.
        broken: bool,
    }

    impl HelperSpawner for MockHelper {
        fn spawn(&self, user: &str, cookie: &str) -> std::io::Result<HelperConn> {
            let (ours, theirs) = UnixStream::pair()?;
            self.spawns.lock().push((user.into(), cookie.into()));
            let broken = self.broken;
            std::thread::spawn(move || {
                let mut out = theirs.try_clone().unwrap();
                let mut input = BufReader::new(theirs);
                if broken {
                    let _ = writeln!(out, "FAILURE");
                    return;
                }
                let _ = writeln!(out, "PAM_TEXT_INFO Touch nothing");
                let _ = writeln!(out, "PAM_PROMPT_ECHO_OFF Password: ");
                let mut answer = String::new();
                if input.read_line(&mut answer).unwrap_or(0) == 0 {
                    return;
                }
                let _ = writeln!(out, "{}", if answer.trim_end() == "right" { "SUCCESS" } else { "FAILURE" });
            });
            let kills = self.kills.clone();
            let killer = ours.try_clone()?;
            Ok(HelperConn {
                reader: Box::new(BufReader::new(ours.try_clone()?)),
                writer: Box::new(ours),
                kill: Box::new(move || {
                    kills.fetch_add(1, Ordering::SeqCst);
                    let _ = killer.shutdown(std::net::Shutdown::Both);
                }),
            })
        }
    }

    fn mock(broken: bool) -> MockHelper {
        MockHelper { spawns: Arc::new(Mutex::new(Vec::new())), kills: Arc::new(AtomicUsize::new(0)), broken }
    }

    /// Plays the UI: answers prompts from `answers`, records events.
    fn drive(helper: &MockHelper, answers: Vec<PolkitReply>) -> (Outcome, Vec<PolkitEvent>) {
        let (etx, erx) = async_channel::unbounded();
        let (rtx, rrx) = async_channel::unbounded();
        let ui = std::thread::spawn(move || {
            let mut answers = answers.into_iter();
            let mut seen = Vec::new();
            while let Ok(ev) = erx.recv_blocking() {
                let done = matches!(ev, PolkitEvent::Close { .. });
                if matches!(ev, PolkitEvent::Prompt { .. })
                    && let Some(a) = answers.next()
                {
                    rtx.send_blocking(a).unwrap();
                }
                seen.push(ev);
                if done {
                    break;
                }
            }
            seen
        });
        let outcome = futures::executor::block_on(run_session(helper, &etx, &rrx, "cookie-1", "layla".into()));
        drop(etx);
        (outcome, ui.join().unwrap())
    }

    fn answer(text: &str) -> PolkitReply {
        PolkitReply::Answer { cookie: "cookie-1".into(), text: text.into() }
    }

    #[test]
    fn correct_password_authorizes() {
        let helper = mock(false);
        let (outcome, events) = drive(&helper, vec![answer("right")]);
        assert_eq!(outcome, Outcome::Authorized);
        assert_eq!(helper.spawns.lock().as_slice(), &[("layla".to_string(), "cookie-1".to_string())]);
        assert!(events.contains(&PolkitEvent::Message { cookie: "cookie-1".into(), text: "Touch nothing".into(), error: false }));
        assert!(events.contains(&PolkitEvent::Prompt { cookie: "cookie-1".into(), user: "layla".into(), text: "Password: ".into(), echo: false }));
        assert_eq!(events.last(), Some(&PolkitEvent::Close { cookie: "cookie-1".into(), outcome: Outcome::Authorized }));
    }

    #[test]
    fn wrong_password_retries_with_a_fresh_helper() {
        let helper = mock(false);
        let (outcome, events) = drive(&helper, vec![answer("wrong"), answer("right")]);
        assert_eq!(outcome, Outcome::Authorized);
        assert_eq!(helper.spawns.lock().len(), 2);
        assert_eq!(events.iter().filter(|e| matches!(e, PolkitEvent::Retry { .. })).count(), 1);
    }

    #[test]
    fn switching_user_restarts_for_that_user() {
        let helper = mock(false);
        let switch = PolkitReply::SelectUser { cookie: "cookie-1".into(), user: "root".into() };
        let (outcome, _) = drive(&helper, vec![switch, answer("right")]);
        assert_eq!(outcome, Outcome::Authorized);
        let spawns = helper.spawns.lock();
        assert_eq!(spawns.iter().map(|(u, _)| u.as_str()).collect::<Vec<_>>(), vec!["layla", "root"]);
    }

    #[test]
    fn cancel_kills_the_helper() {
        let helper = mock(false);
        let (outcome, events) = drive(&helper, vec![PolkitReply::Cancel { cookie: "cookie-1".into() }]);
        assert_eq!(outcome, Outcome::Cancelled);
        assert!(helper.kills.load(Ordering::SeqCst) >= 1);
        assert_eq!(events.last(), Some(&PolkitEvent::Close { cookie: "cookie-1".into(), outcome: Outcome::Cancelled }));
    }

    #[test]
    fn a_helper_that_fails_before_prompting_ends_the_request() {
        let helper = mock(true);
        let (outcome, _) = drive(&helper, vec![]);
        assert_eq!(outcome, Outcome::Failed);
        assert_eq!(helper.spawns.lock().len(), 1, "no retry loop on a broken helper");
    }
}
