//! One Aqueous IPC connection: `hello` first, then requests that each wait
//! for their reply (v1 allows one outstanding request per connection), or
//! a subscription's state events.

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use serde::Serialize;

use super::state::Batch;
use super::wire::{Capabilities, Empty, Frame, FrameReader, IpcError, Reply, RequestWriter, STATE_SCHEMA, valid_counter, valid_session};

pub struct Conn {
    stream: UnixStream,
    reader: FrameReader<UnixStream>,
    writer: RequestWriter,
    /// The compositor session token `hello` returned.
    pub session: String,
    pub capabilities: Capabilities,
}

impl Conn {
    /// Connects and performs `hello` within `deadline` (reads and writes
    /// keep that deadline until [`Conn::set_deadline`] changes it).
    pub fn open(path: &Path, deadline: Duration) -> Result<Self> {
        let stream = UnixStream::connect(path).with_context(|| format!("connecting {}", path.display()))?;
        stream.set_read_timeout(Some(deadline))?;
        stream.set_write_timeout(Some(deadline))?;
        let reader = FrameReader::new(stream.try_clone()?);
        let mut conn = Self { stream, reader, writer: RequestWriter::default(), session: String::new(), capabilities: Capabilities::default() };
        let hello = conn.request("hello", Empty {})?;
        let session = hello.session.filter(|s| valid_session(s)).ok_or_else(|| anyhow!("aqueous: hello returned no valid session token"))?;
        if hello.schema != Some(STATE_SCHEMA) {
            bail!("aqueous: unsupported state schema {:?}", hello.schema);
        }
        if let Some(limit) = hello.max_request_bytes {
            conn.writer.limit(limit);
        }
        if let Some(limit) = hello.max_frame_bytes {
            conn.reader.limit(limit);
        }
        conn.session = session;
        conn.capabilities = hello.capabilities;
        Ok(conn)
    }

    /// The read deadline; `None` waits for as long as it takes (an
    /// established subscription has no idle timeout).
    pub fn set_deadline(&self, deadline: Option<Duration>) -> Result<()> {
        // The reader's descriptor is a duplicate of the same socket.
        self.stream.set_read_timeout(deadline)?;
        Ok(())
    }

    /// Whether the compositor has closed (or broken) this idle request
    /// connection. Nothing is ever sent on it unasked, so any readiness
    /// before a request is the end of the stream or an error; checked
    /// before reusing it, so a command never goes to a dead connection.
    pub fn closed(&self) -> bool {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        let mut fds = [PollFd::new(&self.stream, PollFlags::IN)];
        poll(&mut fds, Some(&Timespec { tv_sec: 0, tv_nsec: 0 })).map_or(true, |ready| ready > 0)
    }

    /// Sends `op` and waits for its reply. A server error is an
    /// [`IpcError`]; anything else means the connection is unusable.
    pub fn request(&mut self, op: &str, params: impl Serialize) -> Result<Reply> {
        let session = (!self.session.is_empty()).then_some(self.session.as_str());
        let id = self.writer.send(&mut (&self.stream), session, op, params)?;
        let text = self.reader.next_frame()?.ok_or_else(|| anyhow!("aqueous: the connection closed before reply {id}"))?;
        Frame::parse(text)?.into_reply(id)
    }

    /// The next state event of a subscribed connection: its delivery id
    /// and batch. `None` when the compositor closed the connection.
    pub fn next_event(&mut self) -> Result<Option<(String, Batch)>> {
        let Some(text) = self.reader.next_frame()? else { return Ok(None) };
        let frame = Frame::parse(text)?;
        match (frame.event.as_deref(), frame.delivery, frame.batch) {
            (Some("state"), Some(delivery), Some(batch)) if valid_counter(&delivery) => Ok(Some((delivery, batch))),
            (Some("state"), ..) => bail!("aqueous: a malformed state event"),
            // Only state events are defined; a reply here was never asked for.
            (Some(other), ..) => bail!("aqueous: unknown event {other:?}"),
            (None, ..) => bail!("aqueous: an unsolicited reply on the event connection"),
        }
    }
}

/// Whether an error leaves the connection usable: the server answered
/// with an error for that request only (not a stale session).
pub fn keeps_connection(err: &anyhow::Error) -> bool {
    err.downcast_ref::<IpcError>().is_some_and(|e| e.code != "stale_session")
}
