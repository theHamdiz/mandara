//! Aqueous IPC v1 framing and envelopes (`compositor/protocol/aqueous-ipc-v1.md`
//! in the Aqueous tree): one UTF-8 JSON object per LF-terminated frame,
//! requests `{"ipc":1,"id":"N","session":"…","op":…,"params":{…}}`, replies
//! `{"ipc":1,"id":"N","ok":true,"result":{…}}` or `"ok":false,"error":{…}`,
//! and state events `{"ipc":1,"event":"state","delivery":"N","batch":{…}}`.

use std::fmt;
use std::io::{BufRead, BufReader, Read, Write};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::state::Batch;

/// The IPC envelope version (`"ipc":1`).
pub const IPC_VERSION: u32 = 1;
/// The shell state schema inside batches (`"schema":1`).
pub const STATE_SCHEMA: u32 = 1;
/// Local hard limit of one request, encoded, without the LF (Aqueous
/// advertises 64 KiB; the smaller of the two applies).
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
/// Local hard limit of one server frame (a 4 MiB batch plus its envelope,
/// what Aqueous advertises).
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024 + 64 * 1024;
/// Read buffer capacity a connection keeps between frames: a large
/// snapshot's buffer is released once it has been parsed.
pub const RETAINED_FRAME_CAPACITY: usize = 64 * 1024;
/// Request ids are decimal strings of at most 20 digits (a `u64`).
const MAX_ID_DIGITS: usize = 20;

/// `{}`: the params of `hello`, `snapshot` and `subscribe`.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Empty {}

/// `ack` params.
#[derive(Serialize)]
pub struct Ack<'a> {
    pub delivery: &'a str,
}

/// `command` params.
#[derive(Serialize)]
pub struct Command<'a, F: Serialize> {
    pub action: &'a str,
    pub fields: F,
}

#[derive(Serialize)]
struct Request<'a, P: Serialize> {
    ipc: u32,
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<&'a str>,
    op: &'a str,
    params: P,
}

/// Encodes requests with strictly increasing ids into one reused buffer.
pub struct RequestWriter {
    next_id: u64,
    id: String,
    buf: Vec<u8>,
    max: usize,
}

impl Default for RequestWriter {
    fn default() -> Self {
        Self { next_id: 1, id: String::with_capacity(MAX_ID_DIGITS), buf: Vec::with_capacity(256), max: MAX_REQUEST_BYTES }
    }
}

impl RequestWriter {
    /// Lowers the request limit to what the server advertised.
    pub fn limit(&mut self, advertised: usize) {
        self.max = advertised.clamp(1, MAX_REQUEST_BYTES);
    }

    /// Writes one request and returns its id.
    pub fn send(&mut self, out: &mut impl Write, session: Option<&str>, op: &str, params: impl Serialize) -> Result<&str> {
        use fmt::Write as _;
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or_else(|| anyhow::anyhow!("aqueous: request ids exhausted"))?;
        self.id.clear();
        let _ = write!(self.id, "{id}");
        self.buf.clear();
        serde_json::to_writer(&mut self.buf, &Request { ipc: IPC_VERSION, id: &self.id, session, op, params })?;
        if self.buf.len() > self.max {
            bail!("aqueous: a {op} request of {} bytes exceeds the {}-byte limit", self.buf.len(), self.max);
        }
        self.buf.push(b'\n');
        out.write_all(&self.buf)?;
        out.flush()?;
        if self.buf.capacity() > RETAINED_FRAME_CAPACITY {
            self.buf = Vec::with_capacity(256);
        }
        Ok(&self.id)
    }
}

/// Reads LF-terminated frames of at most `max` bytes. A frame may arrive
/// in any number of reads (split anywhere, even inside a UTF-8 sequence).
pub struct FrameReader<R> {
    inner: BufReader<R>,
    buf: Vec<u8>,
    max: usize,
}

impl<R: Read> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        Self { inner: BufReader::with_capacity(8192, inner), buf: Vec::new(), max: MAX_FRAME_BYTES }
    }

    /// Lowers the frame limit to what the server advertised.
    pub fn limit(&mut self, advertised: usize) {
        self.max = advertised.clamp(1, MAX_FRAME_BYTES);
    }

    pub fn get_ref(&self) -> &R {
        self.inner.get_ref()
    }

    /// The next frame without its LF; `None` when the peer closed the
    /// connection between frames. An oversized frame, a connection closed
    /// inside one, or invalid UTF-8 is an error (the connection must close).
    pub fn next_frame(&mut self) -> Result<Option<&str>> {
        if self.buf.capacity() > RETAINED_FRAME_CAPACITY {
            self.buf = Vec::new();
        }
        self.buf.clear();
        loop {
            let available = match self.inner.fill_buf() {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            };
            if available.is_empty() {
                if self.buf.is_empty() {
                    return Ok(None);
                }
                bail!("aqueous: the connection closed inside a frame");
            }
            let newline = available.iter().position(|&b| b == b'\n');
            let part = &available[..newline.unwrap_or(available.len())];
            if self.buf.len() + part.len() > self.max {
                bail!("aqueous: a frame exceeds the {}-byte limit", self.max);
            }
            self.buf.extend_from_slice(part);
            let used = newline.map_or(part.len(), |i| i + 1);
            self.inner.consume(used);
            if newline.is_some() {
                break;
            }
        }
        Ok(Some(std::str::from_utf8(&self.buf).map_err(|_| anyhow::anyhow!("aqueous: a frame is not valid UTF-8"))?))
    }
}

/// `"error"` of a failed reply.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct IpcError {
    /// `invalid`, `not_found`, `locked`, `unsupported`, `busy`,
    /// `ambiguous_seat`, `unavailable`, `stale_session`, …
    pub code: String,
    pub message: String,
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.message.is_empty() || self.message == self.code { write!(f, "aqueous: {}", self.code) } else { write!(f, "aqueous: {} ({})", self.message, self.code) }
    }
}

impl std::error::Error for IpcError {}

/// The capabilities a `hello` advertises (the ones Mandara uses; others
/// are ignored).
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Capabilities {
    pub state: bool,
    pub commands: bool,
    pub keyboard: bool,
    pub overview: bool,
}

/// Every result field Mandara reads, for any operation (unknown fields
/// are ignored, as the contract allows).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Reply {
    // hello
    pub session: Option<String>,
    pub schema: Option<u32>,
    pub max_request_bytes: Option<usize>,
    pub max_frame_bytes: Option<usize>,
    pub capabilities: Capabilities,
    // snapshot
    pub batch: Option<Batch>,
    // subscribe
    pub subscribed: Option<bool>,
    // ack
    pub acked: Option<String>,
    // command: `applied` or `accepted`
    pub status: Option<String>,
    pub sequence: Option<String>,
}

/// Any server frame: a reply (`id`, `ok`) or an event (`event`,
/// `delivery`, `batch`).
#[derive(Debug, Deserialize)]
pub struct Frame {
    pub ipc: u32,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub ok: Option<bool>,
    #[serde(default)]
    pub result: Option<Reply>,
    #[serde(default)]
    pub error: Option<IpcError>,
    #[serde(default)]
    pub event: Option<String>,
    #[serde(default)]
    pub delivery: Option<String>,
    #[serde(default)]
    pub batch: Option<Batch>,
}

impl Frame {
    pub fn parse(text: &str) -> Result<Self> {
        let frame: Frame = serde_json::from_str(text).map_err(|e| anyhow::anyhow!("aqueous: malformed frame: {e}"))?;
        if frame.ipc != IPC_VERSION {
            bail!("aqueous: unsupported IPC version {}", frame.ipc);
        }
        Ok(frame)
    }

    /// The reply to request `id`: its result, or its error.
    pub fn into_reply(self, id: &str) -> Result<Reply> {
        if self.event.is_some() {
            bail!("aqueous: an event arrived while waiting for reply {id}");
        }
        if self.id.as_deref() != Some(id) {
            bail!("aqueous: reply {:?} doesn't answer request {id}", self.id);
        }
        match self.ok {
            Some(true) => Ok(self.result.unwrap_or_default()),
            Some(false) => Err(self.error.unwrap_or_default().into()),
            None => bail!("aqueous: reply {id} has no `ok`"),
        }
    }
}

/// A session token: 32 lowercase hex characters.
pub fn valid_session(token: &str) -> bool {
    token.len() == 32 && token.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Sequences and delivery ids: nonempty decimal strings of at most 20 digits.
pub fn valid_counter(s: &str) -> bool {
    (1..=MAX_ID_DIGITS).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hands out its bytes `step` at a time, like a socket splitting frames.
    struct Trickle<'a> {
        data: &'a [u8],
        step: usize,
    }

    impl Read for Trickle<'_> {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let n = self.step.min(self.data.len()).min(out.len());
            out[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    #[test]
    fn frames_split_anywhere_are_reassembled() {
        let text = "{\"a\":\"🐟\"}\n{\"b\":2}\n".as_bytes();
        for step in [1, 2, 3, 5, 64] {
            let mut r = FrameReader::new(Trickle { data: text, step });
            assert_eq!(r.next_frame().unwrap(), Some("{\"a\":\"🐟\"}"), "step {step}");
            assert_eq!(r.next_frame().unwrap(), Some("{\"b\":2}"));
            assert_eq!(r.next_frame().unwrap(), None);
        }
    }

    #[test]
    fn oversized_truncated_and_invalid_frames_are_rejected() {
        let mut r = FrameReader::new(Trickle { data: b"0123456789\n", step: 4 });
        r.limit(8);
        assert!(r.next_frame().unwrap_err().to_string().contains("limit"));
        let mut r = FrameReader::new(Trickle { data: b"{\"half\":", step: 3 });
        assert!(r.next_frame().unwrap_err().to_string().contains("inside a frame"));
        let mut r = FrameReader::new(Trickle { data: b"\xff\xfe\n", step: 1 });
        assert!(r.next_frame().unwrap_err().to_string().contains("UTF-8"));
        // A big frame's buffer is not kept for the next small one.
        let big = format!("{}\nx\n", "y".repeat(RETAINED_FRAME_CAPACITY * 2));
        let mut r = FrameReader::new(Trickle { data: big.as_bytes(), step: 8192 });
        assert_eq!(r.next_frame().unwrap().map(str::len), Some(RETAINED_FRAME_CAPACITY * 2));
        assert_eq!(r.next_frame().unwrap(), Some("x"));
        assert!(r.buf.capacity() <= RETAINED_FRAME_CAPACITY);
    }

    #[test]
    fn requests_carry_increasing_ids_and_the_session() {
        let mut w = RequestWriter::default();
        let mut out = Vec::new();
        assert_eq!(w.send(&mut out, None, "hello", Empty {}).unwrap(), "1");
        assert_eq!(w.send(&mut out, Some("0123456789abcdef0123456789abcdef"), "ack", Ack { delivery: "7" }).unwrap(), "2");
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines[0], serde_json::json!({"ipc":1,"id":"1","op":"hello","params":{}}));
        assert_eq!(lines[1], serde_json::json!({"ipc":1,"id":"2","session":"0123456789abcdef0123456789abcdef","op":"ack","params":{"delivery":"7"}}));
        assert!(text.ends_with('\n') && !text.trim_end().contains("\n\n"));
        w.limit(16);
        let err = w.send(&mut Vec::new(), None, "command", Command { action: "workspace.rename", fields: serde_json::json!({"name": "x".repeat(32)}) }).unwrap_err();
        assert!(err.to_string().contains("limit"), "{err}");
    }

    #[test]
    fn replies_match_their_request_and_surface_errors() {
        let ok = Frame::parse(r#"{"ipc":1,"id":"3","ok":true,"result":{"acked":"1"}}"#).unwrap();
        assert_eq!(ok.into_reply("3").unwrap().acked.as_deref(), Some("1"));
        let err = Frame::parse(r#"{"ipc":1,"id":"6","ok":false,"error":{"code":"stale_session","message":"stale_session"}}"#).unwrap().into_reply("6").unwrap_err();
        assert_eq!(err.downcast_ref::<IpcError>().map(|e| e.code.as_str()), Some("stale_session"));
        assert!(Frame::parse(r#"{"ipc":1,"id":"4","ok":true,"result":{}}"#).unwrap().into_reply("5").is_err(), "wrong id");
        assert!(Frame::parse(r#"{"ipc":2,"id":"1","ok":true}"#).is_err(), "unknown IPC version");
        assert!(Frame::parse("{not json").is_err());
        assert!(Frame::parse(r#"{"ipc":1,"event":"state","delivery":"1"}"#).unwrap().into_reply("1").is_err(), "an event is no reply");
        assert!(valid_session("6b94a179d09456846b94a179d0945684"));
        assert!(!valid_session("6B94A179D09456846B94A179D0945684") && !valid_session("abc"));
        assert!(valid_counter("12") && !valid_counter("") && !valid_counter("1a") && !valid_counter(&"9".repeat(21)));
    }
}
