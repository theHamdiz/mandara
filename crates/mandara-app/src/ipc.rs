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
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How long the server waits for a client's request line.
const CLIENT_READ_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a client waits for the shell's answer.
const REPLY_TIMEOUT: Duration = Duration::from_secs(15);
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
    let (tx, rx) = async_channel::unbounded::<Request>();
    std::thread::Builder::new().name("mandara-ipc".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            let _ = std::thread::Builder::new().name("mandara-ipc-client".into()).stack_size(CLIENT_THREAD_STACK).spawn(move || {
                if let Err(err) = handle_client(stream, &tx) {
                    tracing::debug!("ipc client error: {err:#}");
                }
            });
        }
    })?;
    Ok(rx)
}

fn handle_client(stream: UnixStream, tx: &async_channel::Sender<Request>) -> Result<()> {
    stream.set_read_timeout(Some(CLIENT_READ_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let response = match parse_request(&line) {
        Err(err) => WireResponse::err(format!("{err:#}")),
        Ok(args) => {
            let (reply_tx, reply_rx) = async_channel::bounded(1);
            tx.send_blocking(Request { args, reply: reply_tx }).context("shell is shutting down")?;
            reply_rx.recv_blocking().unwrap_or_else(|_| WireResponse::err("no reply"))
        }
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
        UnixStream::connect(path).with_context(|| format!("is Mandara running? could not connect to {}", path.display()))?;
    stream.set_read_timeout(Some(REPLY_TIMEOUT))?;
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
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
