//! IPC over a Unix socket at `$XDG_RUNTIME_DIR/mandara.sock`.
//!
//! Protocol (one request per connection, newline-terminated JSON):
//!
//! ```text
//! → {"args": ["audio", "volume", "set", "40"]}
//! ← {"ok": true, "data": "40%"}
//! ← {"ok": false, "error": "usage: audio volume set <percent>"}
//! ```
//!
//! A bare line of words (`audio volume set 40`) is accepted too, so the
//! socket is easy to drive from shell scripts (`socat`).

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How long a client has, in total, to send its request line.
const CLIENT_READ_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the server waits for a client to take the answer.
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// The longest request line accepted. A `config apply` carries the whole
/// config as one argument, which is far smaller than this.
const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
/// Requests served at once; the next connection is refused with an error
/// instead of getting a thread, so a stuck shell cannot pile up threads.
const MAX_CLIENTS: usize = 32;
/// Requests waiting for the shell's foreground loop.
const REQUEST_QUEUE: usize = 64;
/// The request the supervisor's watchdog sends. It is answered by the
/// shell's foreground (UI) thread, so an answer proves that thread is
/// running and not only the IPC thread.
pub const WATCHDOG_COMMAND: &str = "watchdog";
/// How long the shell has to answer [`WATCHDOG_COMMAND`].
pub const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a client waits for the shell's answer.
const REPLY_TIMEOUT: Duration = Duration::from_secs(15);
/// Display discovery, apply and confirmation each have a bounded protocol wait.
const DISPLAY_REPLY_TIMEOUT: Duration = Duration::from_secs(30);
/// Model transfers may be large; body-stall limits remain in the downloader.
const MODEL_REPLY_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Icon and cursor theme downloads can be ~100 MB on a slow link; the
/// downloader keeps its own connect and body-stall limits.
const THEME_REPLY_TIMEOUT: Duration = Duration::from_secs(65 * 60);
/// Boot look downloads themes, waits for your password and rebuilds the
/// initramfs, which can take minutes.
const BOOT_REPLY_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Whether `args` is the supervisor's liveness probe.
pub fn is_watchdog(args: &[String]) -> bool {
    matches!(args, [command] if command == WATCHDOG_COMMAND)
}

fn reply_timeout(args: &[String]) -> Duration {
    match (args.first().map(String::as_str), args.get(1).map(String::as_str)) {
        (Some(WATCHDOG_COMMAND), None) => WATCHDOG_TIMEOUT,
        (Some("permissions"), Some(_)) => {
            mandara_services::app_permissions::OPERATION_TIMEOUT + CLIENT_READ_TIMEOUT
        }
        (Some("backups"), Some(command)) if command != "cancel" => {
            mandara_services::backups::OPERATION_TIMEOUT + CLIENT_READ_TIMEOUT
        }
        (Some("ai"), Some("chat")) if args.get(2).is_some_and(|command| command == "send") => {
            mandara_services::local_ai::chat::CHAT_TIMEOUT + CLIENT_READ_TIMEOUT
        }
        (Some("ai"), Some("models")) => {
            mandara_services::local_ai::chat::CHAT_TIMEOUT + CLIENT_READ_TIMEOUT
        }
        (Some("ai"), Some("load" | "unload")) => MODEL_REPLY_TIMEOUT,
        (Some("models"), Some("download")) => MODEL_REPLY_TIMEOUT,
        (Some("theme"), Some("apps" | "export")) => THEME_REPLY_TIMEOUT,
        (Some("display"), _) => DISPLAY_REPLY_TIMEOUT,
        (Some("icons" | "cursor"), Some("install" | "import")) => THEME_REPLY_TIMEOUT,
        (Some("boot"), Some("install" | "import" | "apply" | "restore")) => BOOT_REPLY_TIMEOUT,
        (Some("agent"), Some("prepare")) => {
            mandara_services::command_agent::INDEX_SETUP_TIMEOUT + CLIENT_READ_TIMEOUT
        }
        (Some("agent"), Some("preview")) => {
            mandara_services::command_agent::IDLE_TIMEOUT + CLIENT_READ_TIMEOUT
        }
        _ => REPLY_TIMEOUT,
    }
}
/// Stack of a per-connection server thread: it only parses one line and
/// waits for the reply, so the default 2 MiB stack is waste.
const CLIENT_THREAD_STACK: usize = 128 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub struct WireRequest {
    pub args: Vec<String>,
}

/// [`WireRequest`] borrowed, for sending without copying the arguments (a
/// `config apply` carries the whole config as one argument).
#[derive(Serialize)]
struct WireRequestRef<'a> {
    args: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl WireResponse {
    pub fn ok(data: Value) -> Self {
        Self { ok: true, data: Some(data), error: None }
    }

    pub fn err(error: impl Into<String>) -> Self {
        Self { ok: false, data: None, error: Some(error.into()) }
    }
}

/// Parses a request line: JSON `{"args": [...]}` or plain words.
pub fn parse_request(line: &str) -> Result<Vec<String>> {
    let line = line.trim();
    if line.starts_with('{') {
        let req: WireRequest = serde_json::from_str(line).context("invalid JSON request")?;
        return Ok(req.args);
    }
    let words: Vec<String> = line.split_whitespace().map(String::from).collect();
    if words.is_empty() {
        bail!("empty request");
    }
    Ok(words)
}

/// A request from a client plus the channel to answer it on.
pub struct Request {
    pub args: Vec<String>,
    pub reply: async_channel::Sender<WireResponse>,
}

/// Limits on what one connection may cost the shell.
#[derive(Clone, Copy)]
struct Limits {
    /// Total time to receive the request line.
    read: Duration,
    /// Longest request line.
    max_request: usize,
    /// How long the shell has to answer a request.
    reply: fn(&[String]) -> Duration,
}

const LIMITS: Limits = Limits { read: CLIENT_READ_TIMEOUT, max_request: MAX_REQUEST_BYTES, reply: reply_timeout };

/// Counts the connections being served; a [`Permit`] frees its slot on drop.
#[derive(Clone)]
struct Slots {
    active: Arc<AtomicUsize>,
    max: usize,
}

struct Permit(Arc<AtomicUsize>);

impl Slots {
    fn new(max: usize) -> Self {
        Self { active: Arc::new(AtomicUsize::new(0)), max }
    }

    /// A slot, or `None` when `max` connections are already being served.
    fn acquire(&self) -> Option<Permit> {
        // Take the slot first and give it back if it was past the cap, so
        // concurrent callers can never both end up over it.
        if self.active.fetch_add(1, Ordering::AcqRel) >= self.max {
            self.active.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(Permit(self.active.clone()))
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Binds the socket, refusing to start if another instance answers on it and
/// cleaning up a stale socket file otherwise.
pub fn serve(path: &Path) -> Result<async_channel::Receiver<Request>> {
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            bail!("another Mandara instance is already running ({})", path.display());
        }
        std::fs::remove_file(path).with_context(|| format!("removing stale socket {}", path.display()))?;
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let listener = UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
    // Only this user talks to the shell, whatever the umask or the folder's mode.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).with_context(|| format!("restricting {}", path.display()))?;
    let (tx, rx) = async_channel::bounded::<Request>(REQUEST_QUEUE);
    let slots = Slots::new(MAX_CLIENTS);
    std::thread::Builder::new().name("mandara-ipc".into()).spawn(move || accept_loop(&listener, &tx, &slots, LIMITS))?;
    Ok(rx)
}

fn accept_loop(listener: &UnixListener, tx: &async_channel::Sender<Request>, slots: &Slots, limits: Limits) {
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(err) => {
                // Out of file descriptors and the like: retrying at once would spin.
                tracing::warn!("ipc accept failed: {err}");
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        let Some(permit) = slots.acquire() else {
            refuse(stream, "too many requests at once");
            continue;
        };
        let tx = tx.clone();
        let spawned = std::thread::Builder::new().name("mandara-ipc-client".into()).stack_size(CLIENT_THREAD_STACK).spawn(move || {
            let _permit = permit;
            if let Err(err) = handle_client(stream, &tx, limits) {
                tracing::debug!("ipc client error: {err:#}");
            }
        });
        if let Err(err) = spawned {
            tracing::warn!("ipc: could not start a client thread: {err}");
        }
    }
}

/// Answers a connection that gets no thread. The error is a few bytes into
/// an empty socket buffer, so the short write timeout is never reached by a
/// well-behaved client and bounds a hostile one.
fn refuse(mut stream: UnixStream, reason: &str) {
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    if let Ok(mut out) = serde_json::to_vec(&WireResponse::err(reason)) {
        out.push(b'\n');
        let _ = stream.write_all(&out);
    }
}

/// Reads one `\n`-terminated line of at most `max` bytes (without the
/// newline), giving the whole read `budget` rather than each `read` call, so
/// a client that drips bytes cannot hold the connection. End of input also
/// ends the line.
pub fn read_line_bounded(stream: &UnixStream, max: usize, budget: Duration) -> Result<String> {
    let deadline = Instant::now() + budget;
    // Requests are a few words; `fill_buf` re-fills for longer lines, so a
    // small buffer avoids an 8 KiB allocation on every connection.
    let mut reader = BufReader::with_capacity(1024, stream);
    let mut line = Vec::new();
    loop {
        let left = deadline.checked_duration_since(Instant::now()).filter(|left| !left.is_zero()).context("timed out waiting for the request")?;
        stream.set_read_timeout(Some(left))?;
        let chunk = match reader.fill_buf() {
            Ok(chunk) => chunk,
            Err(err) if matches!(err.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => bail!("timed out waiting for the request"),
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err.into()),
        };
        if chunk.is_empty() {
            break;
        }
        let (take, done) = match chunk.iter().position(|&b| b == b'\n') {
            Some(newline) => (newline, true),
            None => (chunk.len(), false),
        };
        if line.len() + take > max {
            bail!("request is larger than {max} bytes");
        }
        line.extend_from_slice(&chunk[..take]);
        reader.consume(take + usize::from(done));
        if done {
            break;
        }
    }
    String::from_utf8(line).context("request is not valid UTF-8")
}

/// Waits for the shell's answer, at most `timeout`.
fn wait_reply(rx: &async_channel::Receiver<WireResponse>, timeout: Duration) -> WireResponse {
    use futures::future::{Either, select};
    futures::executor::block_on(async {
        let reply = std::pin::pin!(rx.recv());
        let deadline = std::pin::pin!(mandara_services::runtime::sleep(timeout));
        match select(reply, deadline).await {
            Either::Left((Ok(response), _)) => response,
            Either::Left((Err(_), _)) => WireResponse::err("no reply"),
            Either::Right(_) => WireResponse::err("the shell did not answer in time"),
        }
    })
}

fn handle_client(stream: UnixStream, tx: &async_channel::Sender<Request>, limits: Limits) -> Result<()> {
    stream.set_write_timeout(Some(CLIENT_WRITE_TIMEOUT))?;
    let response = match read_line_bounded(&stream, limits.max_request, limits.read) {
        Err(err) => WireResponse::err(format!("{err:#}")),
        Ok(line) => match parse_request(&line) {
            Err(err) => WireResponse::err(format!("{err:#}")),
            Ok(args) => {
                let timeout = (limits.reply)(&args);
                let (reply_tx, reply_rx) = async_channel::bounded(1);
                match tx.try_send(Request { args, reply: reply_tx }) {
                    Ok(()) => wait_reply(&reply_rx, timeout),
                    Err(async_channel::TrySendError::Full(_)) => WireResponse::err("the shell is too busy to take another request"),
                    Err(async_channel::TrySendError::Closed(_)) => bail!("shell is shutting down"),
                }
            }
        },
    };
    // Serialized straight into the socket, one write for the whole line.
    let mut out = serde_json::to_vec(&response)?;
    out.push(b'\n');
    let mut writer = stream;
    writer.write_all(&out)?;
    Ok(())
}

/// Client side: sends `args`, returns the decoded response.
pub fn send(path: &Path, args: &[String]) -> Result<WireResponse> {
    let mut stream =
        UnixStream::connect(path).with_context(|| {
        format!("is Mandara running? could not connect to {}", path.display())
    })?;
    stream.set_read_timeout(Some(reply_timeout(args)))?;
    let mut request = serde_json::to_vec(&WireRequestRef { args })?;
    request.push(b'\n');
    stream.write_all(&request)?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf)?;
    serde_json::from_slice(buf.trim_ascii()).context("invalid response from shell")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn explicit_model_work_can_outlive_the_interactive_reply_budget() {
        let words=|s:&str|s.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert_eq!(reply_timeout(&words("ping")),REPLY_TIMEOUT);
        assert_eq!(reply_timeout(&words("agent status")),REPLY_TIMEOUT);
        assert_eq!(reply_timeout(&words("agent preview dark mode")),mandara_services::command_agent::IDLE_TIMEOUT+CLIENT_READ_TIMEOUT);
        assert_eq!(reply_timeout(&words("agent prepare")),mandara_services::command_agent::INDEX_SETUP_TIMEOUT+CLIENT_READ_TIMEOUT);
        assert_eq!(reply_timeout(&words("models download agent-qwen")),MODEL_REPLY_TIMEOUT);
        assert_eq!(reply_timeout(&words("icons install papirus/dark")),THEME_REPLY_TIMEOUT);
        assert_eq!(reply_timeout(&words("cursor import https://example.org/c.tar.xz --unverified")),THEME_REPLY_TIMEOUT);
        assert_eq!(reply_timeout(&words("icons list")),REPLY_TIMEOUT);
        assert_eq!(
            reply_timeout(&words("theme apps enable community:rofi")),
            THEME_REPLY_TIMEOUT
        );
        assert_eq!(reply_timeout(&words("theme export")), THEME_REPLY_TIMEOUT);
        assert_eq!(
            reply_timeout(&words("display confirm")),
            DISPLAY_REPLY_TIMEOUT
        );
        assert_eq!(reply_timeout(&words("boot apply splash rings")),BOOT_REPLY_TIMEOUT);
        assert_eq!(reply_timeout(&words("boot status")),REPLY_TIMEOUT);
    }

    #[test]
    fn explicit_storage_and_chat_jobs_keep_their_backend_reply_budgets() {
        let words = |value: &str| value.split_whitespace().map(String::from).collect::<Vec<_>>();
        for command in ["backups start", "backups check", "backups init", "backups restore id /copies", "backups list", "backups storage confirm", "backups cleanup confirm"] {
            assert_eq!(reply_timeout(&words(command)), mandara_services::backups::OPERATION_TIMEOUT + CLIENT_READ_TIMEOUT);
        }
        for command in ["permissions apps", "permissions inspect app system", "permissions set app system shared network false", "permissions undo"] {
            assert_eq!(reply_timeout(&words(command)), mandara_services::app_permissions::OPERATION_TIMEOUT + CLIENT_READ_TIMEOUT);
        }
        for command in ["ai chat send hello", "ai models"] {
            assert_eq!(reply_timeout(&words(command)), mandara_services::local_ai::chat::CHAT_TIMEOUT + CLIENT_READ_TIMEOUT);
        }
        for command in ["ai load model", "ai unload all"] {
            assert_eq!(reply_timeout(&words(command)), MODEL_REPLY_TIMEOUT);
        }
        for command in ["backups cancel", "ai chat stop", "ai chat new", "ai loaded", "ping"] {
            assert_eq!(reply_timeout(&words(command)), REPLY_TIMEOUT);
        }
    }

    #[test]
    fn parses_json_and_plain_requests() {
        assert_eq!(parse_request(r#"{"args":["theme","set","rose-pine"]}"#).unwrap(), vec!["theme", "set", "rose-pine"]);
        assert_eq!(parse_request("audio volume set 40\n").unwrap(), vec!["audio", "volume", "set", "40"]);
        assert!(parse_request("   ").is_err());
        assert!(parse_request("{not json").is_err());
    }

    #[test]
    fn round_trip_over_socket() {
        let dir = std::env::temp_dir().join(format!("mandara-ipc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.sock");
        let rx = serve(&path).unwrap();
        assert!(serve(&path).is_err(), "a second instance must refuse to start");
        std::thread::spawn(move || {
            while let Ok(req) = rx.recv_blocking() {
                let response = match req.args.as_slice() {
                    [p] if p == "ping" => WireResponse::ok(json!("pong")),
                    _ => WireResponse::err("unknown"),
                };
                let _ = req.reply.send_blocking(response);
            }
        });
        assert_eq!(send(&path, &["ping".into()]).unwrap(), WireResponse::ok(json!("pong")));
        assert!(!send(&path, &["nope".into()]).unwrap().ok);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the socket is private to the user");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn pair() -> (UnixStream, UnixStream) {
        UnixStream::pair().unwrap()
    }

    /// Small limits; `impatient` gives the shell only 150 ms to answer.
    fn quick(impatient: bool) -> Limits {
        fn short(_: &[String]) -> Duration {
            Duration::from_millis(150)
        }
        fn long(_: &[String]) -> Duration {
            Duration::from_secs(10)
        }
        Limits { read: Duration::from_millis(300), max_request: 64, reply: if impatient { short } else { long } }
    }

    fn answer_of(mut client: UnixStream) -> WireResponse {
        let mut buf = Vec::new();
        client.read_to_end(&mut buf).unwrap();
        serde_json::from_slice(buf.trim_ascii()).unwrap()
    }

    #[test]
    fn the_watchdog_request_is_recognised_exactly() {
        assert!(is_watchdog(&["watchdog".to_string()]));
        assert!(!is_watchdog(&["watchdog".to_string(), "x".to_string()]));
        assert!(!is_watchdog(&["ping".to_string()]));
        assert_eq!(reply_timeout(&["watchdog".to_string()]), WATCHDOG_TIMEOUT);
    }

    #[test]
    fn a_line_is_read_up_to_its_newline_or_the_end_of_input() {
        let (mut client, server) = pair();
        client.write_all(b"ping\nignored").unwrap();
        assert_eq!(read_line_bounded(&server, 64, Duration::from_secs(1)).unwrap(), "ping");
        let (mut client, server) = pair();
        client.write_all(b"audio volume").unwrap();
        drop(client);
        assert_eq!(read_line_bounded(&server, 64, Duration::from_secs(1)).unwrap(), "audio volume");
    }

    #[test]
    fn an_oversized_line_is_refused() {
        let (mut client, server) = pair();
        client.write_all(&[b'a'; 65]).unwrap();
        assert!(read_line_bounded(&server, 64, Duration::from_secs(1)).unwrap_err().to_string().contains("larger"));
        let (mut client, server) = pair();
        client.write_all(&[b'a'; 64]).unwrap();
        client.write_all(b"\n").unwrap();
        assert_eq!(read_line_bounded(&server, 64, Duration::from_secs(1)).unwrap().len(), 64);
    }

    #[test]
    fn a_client_dripping_bytes_runs_out_of_the_total_budget() {
        let (client, server) = pair();
        let dripper = std::thread::spawn(move || {
            let mut client = client;
            // Each byte is well inside any per-read timeout.
            for _ in 0..100 {
                if client.write_all(b"a").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let began = Instant::now();
        let err = read_line_bounded(&server, 1024, Duration::from_millis(200)).unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err:#}");
        assert!(began.elapsed() < Duration::from_secs(1));
        drop(server);
        dripper.join().unwrap();
    }

    #[test]
    fn slots_cap_the_clients_and_free_on_drop() {
        let slots = Slots::new(2);
        let (a, b) = (slots.acquire().unwrap(), slots.acquire().unwrap());
        assert!(slots.acquire().is_none());
        drop(a);
        let c = slots.acquire().unwrap();
        assert!(slots.acquire().is_none());
        drop((b, c));
        assert!(slots.acquire().is_some());
    }

    #[test]
    fn a_shell_that_never_answers_frees_the_client_thread() {
        let (client, server) = pair();
        let (tx, rx) = async_channel::bounded::<Request>(4);
        (&client).write_all(b"{\"args\":[\"ping\"]}\n").unwrap();
        let began = Instant::now();
        handle_client(server, &tx, quick(true)).unwrap();
        let response = answer_of(client);
        assert!(!response.ok);
        assert!(response.error.unwrap().contains("did not answer"));
        assert!(began.elapsed() < Duration::from_secs(5));
        // The request itself was queued; it just was not answered in time.
        assert_eq!(rx.try_recv().unwrap().args, ["ping"]);
    }

    #[test]
    fn a_full_queue_is_reported_instead_of_blocking() {
        let (tx, _rx) = async_channel::bounded::<Request>(1);
        let (hold, _) = async_channel::bounded(1);
        tx.try_send(Request { args: vec!["first".into()], reply: hold }).unwrap();
        let (client, server) = pair();
        (&client).write_all(b"ping\n").unwrap();
        handle_client(server, &tx, quick(false)).unwrap();
        assert!(answer_of(client).error.unwrap().contains("too busy"));
    }

    #[test]
    fn a_bad_request_gets_an_error_answer() {
        let (tx, _rx) = async_channel::bounded::<Request>(1);
        let (client, server) = pair();
        (&client).write_all(&[b'x'; 100]).unwrap();
        handle_client(server, &tx, quick(false)).unwrap();
        assert!(answer_of(client).error.unwrap().contains("larger"));
    }

    #[test]
    fn connections_beyond_the_cap_are_refused_while_others_are_served() {
        let dir = std::env::temp_dir().join(format!("mandara-ipc-cap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cap.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let (tx, rx) = async_channel::bounded::<Request>(4);
        let slots = Slots::new(1);
        let limits = Limits { read: Duration::from_secs(2), max_request: 64, reply: |_| Duration::from_secs(5) };
        std::thread::spawn(move || accept_loop(&listener, &tx, &slots, limits));
        // The first client takes the only slot and sends nothing yet.
        let mut first = UnixStream::connect(&path).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let second = UnixStream::connect(&path).unwrap();
        let refused = answer_of(second);
        assert!(refused.error.unwrap().contains("too many"));
        // The first one is still served, and its slot is free afterwards.
        std::thread::spawn(move || {
            let request = rx.recv_blocking().unwrap();
            let _ = request.reply.send_blocking(WireResponse::ok(json!("pong")));
        });
        first.write_all(b"ping\n").unwrap();
        assert_eq!(answer_of(first), WireResponse::ok(json!("pong")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
