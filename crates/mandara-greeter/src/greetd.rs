//! The greetd IPC protocol and a PAM-conversation driver.
//!
//! Wire format (see `greetd-ipc(7)`): each message is a native-endian `u32`
//! byte length followed by that many bytes of JSON, over the Unix socket in
//! `$GREETD_SOCK`. The greeter sends requests and reads exactly one response
//! per request.
//!
//! [`Conversation`] runs a login over any [`Transport`] (the real socket,
//! or the demo's fake daemon): it answers secret prompts with the password
//! the user already typed, surfaces info / error messages, asks the UI for
//! any further prompts (OTP codes, "new password"), and cancels cleanly on
//! failure so the next attempt starts fresh.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    CreateSession { username: String },
    PostAuthMessageResponse { response: Option<String> },
    StartSession { cmd: Vec<String>, env: Vec<String> },
    CancelSession,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMessageType {
    Visible,
    Secret,
    Info,
    Error,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorType {
    AuthError,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Success,
    Error { error_type: ErrorType, description: String },
    AuthMessage { auth_message_type: AuthMessageType, auth_message: String },
}

/// Largest response we accept (greetd's are tiny).
const MAX_MESSAGE: u32 = 1 << 20;

/// Frames a message: native-endian length + JSON.
pub fn encode(req: &Request) -> io::Result<Vec<u8>> {
    // The JSON holds the password: wipe this copy once it is framed (the
    // caller wipes the framed one).
    let mut body = serde_json::to_vec(req).map_err(io::Error::other)?;
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&(body.len() as u32).to_ne_bytes());
    out.extend_from_slice(&body);
    wipe_bytes(&mut body);
    Ok(out)
}

/// Reads one framed response.
pub fn read_response(r: &mut impl Read) -> io::Result<Response> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_ne_bytes(len);
    if len > MAX_MESSAGE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("greetd message of {len} bytes")));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// One request, one response.
pub trait Transport: Send {
    fn roundtrip(&mut self, req: &Request) -> io::Result<Response>;
}

/// How long the daemon may take to answer one request. Longer than any
/// PAM step a person waits through (a fingerprint reader gives up after
/// 30 s, a slow directory login takes a few seconds), short enough that a
/// hung daemon ends in an error on screen instead of a login that never
/// finishes.
const GREETD_IO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// The real daemon. A failed request leaves the connection out of step with
/// greetd (a late answer would be read as the reply to the next request),
/// so it is dropped and the next request connects afresh.
pub struct GreetdSocket {
    path: PathBuf,
    timeout: std::time::Duration,
    stream: Option<UnixStream>,
}

impl GreetdSocket {
    pub fn connect(path: &Path) -> io::Result<Self> {
        Self::connect_with_timeout(path, GREETD_IO_TIMEOUT)
    }

    fn connect_with_timeout(path: &Path, timeout: std::time::Duration) -> io::Result<Self> {
        let mut socket = Self { path: path.to_path_buf(), timeout, stream: None };
        socket.stream = Some(socket.open()?);
        Ok(socket)
    }

    fn open(&self) -> io::Result<UnixStream> {
        let stream = UnixStream::connect(&self.path)?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;
        Ok(stream)
    }

    /// Connects to `$GREETD_SOCK`.
    pub fn from_env() -> io::Result<Self> {
        let path = std::env::var_os("GREETD_SOCK").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "GREETD_SOCK is not set (not started by greetd)"))?;
        Self::connect(Path::new(&path))
    }
}

impl Transport for GreetdSocket {
    fn roundtrip(&mut self, req: &Request) -> io::Result<Response> {
        let mut stream = match self.stream.take() {
            Some(stream) => stream,
            None => self.open()?,
        };
        let mut msg = encode(req)?;
        let res = stream.write_all(&msg).and_then(|_| read_response(&mut stream));
        wipe_bytes(&mut msg);
        match res {
            Ok(res) => {
                self.stream = Some(stream);
                Ok(res)
            }
            // A socket read that timed out reports "would block"; say what happened.
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => Err(io::ErrorKind::TimedOut.into()),
            Err(e) => Err(e),
        }
    }
}

/// The demo's stand-in daemon: one secret prompt; any non-empty password
/// is accepted except the literal `wrong` (to preview the failure shake).
/// It never authenticates anything against the system.
#[derive(Default)]
pub struct DemoGreetd {
    user: Option<String>,
    prompted: bool,
    /// Simulated PAM latency per request (so the UI's busy state shows).
    pub delay: std::time::Duration,
}

impl DemoGreetd {
    pub fn with_delay(delay: std::time::Duration) -> Self {
        Self { delay, ..Default::default() }
    }
}

impl Transport for DemoGreetd {
    fn roundtrip(&mut self, req: &Request) -> io::Result<Response> {
        std::thread::sleep(self.delay);
        Ok(match req {
            Request::CreateSession { username } => {
                self.user = Some(username.clone());
                self.prompted = true;
                Response::AuthMessage { auth_message_type: AuthMessageType::Secret, auth_message: "Password:".into() }
            }
            Request::PostAuthMessageResponse { response } => {
                if !self.prompted {
                    return Ok(Response::Error { error_type: ErrorType::Error, description: "no prompt pending".into() });
                }
                self.prompted = false;
                match response.as_deref() {
                    Some(p) if !p.is_empty() && p != "wrong" => Response::Success,
                    _ => Response::Error { error_type: ErrorType::AuthError, description: "Authentication failed".into() },
                }
            }
            Request::StartSession { .. } => Response::Success,
            Request::CancelSession => {
                self.user = None;
                self.prompted = false;
                Response::Success
            }
        })
    }
}

/// What the UI should do next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// The daemon wants more input; show `message` as the field's prompt.
    Prompt { secret: bool, message: String },
    /// Credentials accepted; call [`Conversation::start`].
    Authenticated,
    /// Wrong credentials (the session was cancelled).
    Denied(String),
    /// Anything else went wrong (the session was cancelled).
    Failed(String),
}

/// Messages to show alongside a step (PAM info / error lines).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notes {
    pub info: Vec<String>,
    pub errors: Vec<String>,
}

pub struct Conversation<T: Transport> {
    transport: T,
    active: bool,
}

impl<T: Transport> Conversation<T> {
    pub fn new(transport: T) -> Self {
        Self { transport, active: false }
    }

    /// Starts a login for `user`, answering the first secret prompt with
    /// `secret` (the password typed before pressing Enter).
    pub fn begin(&mut self, user: &str, secret: Option<String>) -> (Step, Notes) {
        if self.active {
            self.cancel();
        }
        self.active = true;
        let res = self.transport.roundtrip(&Request::CreateSession { username: user.to_string() });
        self.drive(res, secret)
    }

    /// Answers the pending prompt.
    pub fn respond(&mut self, answer: Option<String>) -> (Step, Notes) {
        let res = self.answer(answer);
        self.drive(res, None)
    }

    /// Starts the session (after [`Step::Authenticated`]).
    pub fn start(&mut self, cmd: Vec<String>, env: Vec<String>) -> Result<(), String> {
        match self.transport.roundtrip(&Request::StartSession { cmd, env }) {
            Ok(Response::Success) => {
                self.active = false;
                clear_console();
                Ok(())
            }
            Ok(Response::Error { description, .. }) => {
                self.cancel();
                Err(description)
            }
            Ok(other) => {
                self.cancel();
                Err(format!("unexpected reply to start_session: {other:?}"))
            }
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn cancel(&mut self) {
        if std::mem::take(&mut self.active) {
            let _ = self.transport.roundtrip(&Request::CancelSession);
        }
    }

    /// Sends the answer to a prompt and wipes the request (it is the
    /// password) once the daemon has replied.
    fn answer(&mut self, response: Option<String>) -> io::Result<Response> {
        let mut req = Request::PostAuthMessageResponse { response };
        let res = self.transport.roundtrip(&req);
        if let Request::PostAuthMessageResponse { response: Some(r) } = &mut req {
            wipe(r);
        }
        res
    }

    /// Runs the conversation, then wipes the password if no prompt took it.
    fn drive(&mut self, res: io::Result<Response>, mut secret: Option<String>) -> (Step, Notes) {
        let out = self.drive_prompts(res, &mut secret);
        if let Some(s) = secret.as_mut() {
            wipe(s);
        }
        out
    }

    fn drive_prompts(&mut self, mut res: io::Result<Response>, secret: &mut Option<String>) -> (Step, Notes) {
        let mut notes = Notes::default();
        // A PAM stack may interleave many info/error messages; bound it.
        for _ in 0..64 {
            match res {
                Err(e) => {
                    self.active = false;
                    return (Step::Failed(e.to_string()), notes);
                }
                Ok(Response::Success) => return (Step::Authenticated, notes),
                Ok(Response::Error { error_type, description }) => {
                    self.cancel();
                    let step = match error_type {
                        ErrorType::AuthError => Step::Denied(description),
                        ErrorType::Error => Step::Failed(description),
                    };
                    return (step, notes);
                }
                Ok(Response::AuthMessage { auth_message_type, auth_message }) => match auth_message_type {
                    AuthMessageType::Info | AuthMessageType::Error => {
                        let text = auth_message.trim().to_string();
                        if !text.is_empty() {
                            if auth_message_type == AuthMessageType::Info { notes.info.push(text) } else { notes.errors.push(text) }
                        }
                        res = self.answer(None);
                    }
                    AuthMessageType::Secret if secret.is_some() => {
                        res = self.answer(secret.take());
                    }
                    kind => {
                        return (Step::Prompt { secret: kind == AuthMessageType::Secret, message: auth_message.trim().to_string() }, notes);
                    }
                },
            }
        }
        self.cancel();
        (Step::Failed("too many authentication messages".into()), notes)
    }
}

impl<T: Transport> Drop for Conversation<T> {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Overwrites a buffer that held a password before it is freed.
pub fn wipe_bytes(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        // SAFETY: writing through a valid &mut u8; volatile so the store
        // isn't optimised away as dead.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
}

/// Overwrites and clears a password string.
pub fn wipe(s: &mut String) {
    // SAFETY: zero bytes are valid UTF-8.
    wipe_bytes(unsafe { s.as_bytes_mut() });
    s.clear();
}

/// Blanks the login VT, hides its cursor and switches it to graphics mode
/// so nothing shows on it between the greeter and the desktop. Best effort:
/// only a real VT is touched. The session command is also wrapped with
/// [`crate::handoff`] because greetd resets the VT to text after we exit.
fn clear_console() {
    crate::handoff::prepare_vt();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn requests_serialize_like_greetd_expects() {
        let j = |r: &Request| serde_json::to_string(r).unwrap();
        assert_eq!(j(&Request::CreateSession { username: "hamdi".into() }), r#"{"type":"create_session","username":"hamdi"}"#);
        assert_eq!(j(&Request::PostAuthMessageResponse { response: Some("pw".into()) }), r#"{"type":"post_auth_message_response","response":"pw"}"#);
        assert_eq!(j(&Request::PostAuthMessageResponse { response: None }), r#"{"type":"post_auth_message_response","response":null}"#);
        assert_eq!(
            j(&Request::StartSession { cmd: vec!["niri-session".into()], env: vec!["XDG_SESSION_TYPE=wayland".into()] }),
            r#"{"type":"start_session","cmd":["niri-session"],"env":["XDG_SESSION_TYPE=wayland"]}"#
        );
        assert_eq!(j(&Request::CancelSession), r#"{"type":"cancel_session"}"#);
    }

    #[test]
    fn responses_parse() {
        let p = |s: &str| serde_json::from_str::<Response>(s).unwrap();
        assert_eq!(p(r#"{"type":"success"}"#), Response::Success);
        assert_eq!(
            p(r#"{"type":"error","error_type":"auth_error","description":"nope"}"#),
            Response::Error { error_type: ErrorType::AuthError, description: "nope".into() }
        );
        assert_eq!(
            p(r#"{"type":"auth_message","auth_message_type":"secret","auth_message":"Password: "}"#),
            Response::AuthMessage { auth_message_type: AuthMessageType::Secret, auth_message: "Password: ".into() }
        );
    }

    #[test]
    fn framing_is_native_endian_length_prefixed() {
        let msg = encode(&Request::CancelSession).unwrap();
        let len = u32::from_ne_bytes(msg[..4].try_into().unwrap()) as usize;
        assert_eq!(len, msg.len() - 4);
        assert_eq!(&msg[4..], br#"{"type":"cancel_session"}"#);

        let body = br#"{"type":"success"}"#;
        let mut wire = (body.len() as u32).to_ne_bytes().to_vec();
        wire.extend_from_slice(body);
        assert_eq!(read_response(&mut &wire[..]).unwrap(), Response::Success);
        // Truncated and oversized frames are errors, not panics.
        assert!(read_response(&mut &wire[..10]).is_err());
        let huge = (MAX_MESSAGE + 1).to_ne_bytes();
        assert!(read_response(&mut &huge[..]).is_err());
    }

    #[test]
    fn socket_roundtrip_against_a_fake_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("greetd.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut len = [0u8; 4];
            s.read_exact(&mut len).unwrap();
            let mut body = vec![0u8; u32::from_ne_bytes(len) as usize];
            s.read_exact(&mut body).unwrap();
            let req: Request = serde_json::from_slice(&body).unwrap();
            assert_eq!(req, Request::CreateSession { username: "alice".into() });
            let reply = br#"{"type":"auth_message","auth_message_type":"secret","auth_message":"Password:"}"#;
            s.write_all(&(reply.len() as u32).to_ne_bytes()).unwrap();
            s.write_all(reply).unwrap();
        });
        let mut sock = GreetdSocket::connect(&path).unwrap();
        let res = sock.roundtrip(&Request::CreateSession { username: "alice".into() }).unwrap();
        assert!(matches!(res, Response::AuthMessage { auth_message_type: AuthMessageType::Secret, .. }));
        server.join().unwrap();
    }

    /// A daemon that stops answering ends the request with an error after
    /// the timeout, and the next request uses a new connection instead of
    /// the one a late reply would have confused.
    #[test]
    fn a_silent_daemon_times_out_and_the_next_request_reconnects() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("greetd.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let read_request = |s: &mut UnixStream| {
                let mut len = [0u8; 4];
                s.read_exact(&mut len).unwrap();
                let mut body = vec![0u8; u32::from_ne_bytes(len) as usize];
                s.read_exact(&mut body).unwrap();
            };
            // First connection: takes the request, never answers.
            let (mut silent, _) = listener.accept().unwrap();
            read_request(&mut silent);
            // The retry arrives on a second connection, which does answer.
            let (mut live, _) = listener.accept().unwrap();
            read_request(&mut live);
            let reply = br#"{"type":"success"}"#;
            live.write_all(&(reply.len() as u32).to_ne_bytes()).unwrap();
            live.write_all(reply).unwrap();
            done_rx.recv().unwrap();
            drop(silent);
        });
        let mut sock = GreetdSocket::connect_with_timeout(&path, std::time::Duration::from_millis(100)).unwrap();
        let err = sock.roundtrip(&Request::CancelSession).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(sock.roundtrip(&Request::CancelSession).unwrap(), Response::Success);
        done_tx.send(()).unwrap();
        server.join().unwrap();
    }

    /// Scripted daemon: records requests, replays canned responses.
    struct Script {
        replies: VecDeque<Response>,
        seen: std::sync::Arc<std::sync::Mutex<Vec<Request>>>,
    }

    impl Transport for Script {
        fn roundtrip(&mut self, req: &Request) -> io::Result<Response> {
            self.seen.lock().unwrap().push(req.clone());
            Ok(self.replies.pop_front().unwrap_or(Response::Success))
        }
    }

    fn script(replies: Vec<Response>) -> (Conversation<Script>, std::sync::Arc<std::sync::Mutex<Vec<Request>>>) {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        (Conversation::new(Script { replies: replies.into(), seen: seen.clone() }), seen)
    }

    fn msg(kind: AuthMessageType, text: &str) -> Response {
        Response::AuthMessage { auth_message_type: kind, auth_message: text.into() }
    }

    #[test]
    fn password_login_answers_the_secret_prompt() {
        let (mut c, seen) = script(vec![msg(AuthMessageType::Secret, "Password:"), Response::Success, Response::Success]);
        let (step, notes) = c.begin("alice", Some("hunter2".into()));
        assert_eq!(step, Step::Authenticated);
        assert_eq!(notes, Notes::default());
        c.start(vec!["sway".into()], vec![]).unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen[1], Request::PostAuthMessageResponse { response: Some("hunter2".into()) });
        assert!(matches!(seen[2], Request::StartSession { .. }));
    }

    #[test]
    fn multi_step_pam_with_info_and_otp() {
        let (mut c, seen) = script(vec![
            msg(AuthMessageType::Info, "Welcome back"),
            msg(AuthMessageType::Secret, "Password:"),
            msg(AuthMessageType::Error, "Your password expires in 3 days"),
            msg(AuthMessageType::Visible, "Verification code:"),
            Response::Success,
        ]);
        let (step, notes) = c.begin("bob", Some("pw".into()));
        assert_eq!(step, Step::Prompt { secret: false, message: "Verification code:".into() });
        assert_eq!(notes.info, vec!["Welcome back".to_string()]);
        assert_eq!(notes.errors, vec!["Your password expires in 3 days".to_string()]);
        let (step, _) = c.respond(Some("123456".into()));
        assert_eq!(step, Step::Authenticated);
        let seen = seen.lock().unwrap();
        // Info and error messages are acknowledged with a null response.
        assert_eq!(seen[1], Request::PostAuthMessageResponse { response: None });
        assert_eq!(seen[2], Request::PostAuthMessageResponse { response: Some("pw".into()) });
        assert_eq!(seen[3], Request::PostAuthMessageResponse { response: None });
        assert_eq!(seen[4], Request::PostAuthMessageResponse { response: Some("123456".into()) });
    }

    #[test]
    fn second_secret_prompt_goes_to_the_ui() {
        let (mut c, _) = script(vec![msg(AuthMessageType::Secret, "Password:"), msg(AuthMessageType::Secret, "New password:")]);
        let (step, _) = c.begin("carol", Some("old".into()));
        assert_eq!(step, Step::Prompt { secret: true, message: "New password:".into() });
    }

    #[test]
    fn auth_failure_cancels_the_session() {
        let (mut c, seen) = script(vec![
            msg(AuthMessageType::Secret, "Password:"),
            Response::Error { error_type: ErrorType::AuthError, description: "Authentication failed".into() },
        ]);
        let (step, _) = c.begin("dave", Some("bad".into()));
        assert_eq!(step, Step::Denied("Authentication failed".into()));
        assert_eq!(seen.lock().unwrap().last(), Some(&Request::CancelSession));
        // A new attempt starts from create_session again.
        drop(seen);
    }

    #[test]
    fn demo_daemon_accepts_any_nonempty_password() {
        let mut c = Conversation::new(DemoGreetd::default());
        assert_eq!(c.begin("me", Some("demo".into())).0, Step::Authenticated);
        assert!(c.start(vec!["true".into()], vec![]).is_ok());
        assert!(matches!(c.begin("me", Some(String::new())).0, Step::Denied(_)));
        assert!(matches!(c.begin("me", Some("wrong".into())).0, Step::Denied(_)));
        assert_eq!(c.begin("me", Some("anything".into())).0, Step::Authenticated);
    }

    #[test]
    fn wipe_zeroes_and_clears() {
        let mut s = String::from("secret");
        wipe(&mut s);
        assert!(s.is_empty());
    }
}
