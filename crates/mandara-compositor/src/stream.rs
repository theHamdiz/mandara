//! What the event-stream drivers share: reconnection backoff, a socket
//! reader that bounds each message and coalesces a burst of them, a filter
//! that drops events the shell already has, and the latest state a watcher
//! publishes for one-shot queries.

use std::collections::HashMap;
use std::io::Read as _;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::model::CompositorEvent;

/// The longest pause between reconnection attempts.
pub(crate) const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);
/// How long a stream must have lived for its next failure to start the
/// backoff over (a server that accepts and drops at once never does).
pub(crate) const STABLE_STREAM: Duration = Duration::from_secs(10);

/// Exponential reconnection delay with jitter, from `min` up to
/// [`MAX_RECONNECT_DELAY`].
pub(crate) struct Backoff {
    min: Duration,
    next: Duration,
}

impl Backoff {
    pub fn new(min: Duration) -> Self {
        Self { min, next: min }
    }

    pub fn reset(&mut self) {
        self.next = self.min;
    }

    pub fn delay(&mut self) -> Duration {
        let base = self.next;
        self.next = (base * 2).min(MAX_RECONNECT_DELAY.max(self.min));
        // Up to a quarter more, so several clients don't retry in step.
        let span = u64::try_from((base / 4).as_nanos()).unwrap_or(u64::MAX).max(1);
        let noise = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| u64::from(d.subsec_nanos()));
        base + Duration::from_nanos(noise % span)
    }

    /// The pause after a stream that lasted `lasted`: the backoff starts
    /// over when the stream was healthy, and keeps growing when it only
    /// flapped.
    pub fn after(&mut self, lasted: Duration) -> Duration {
        if lasted >= STABLE_STREAM {
            self.reset();
        }
        self.delay()
    }
}

/// Bytes read from a socket and not yet consumed as messages.
#[derive(Default)]
pub(crate) struct Pending {
    pub bytes: Vec<u8>,
    /// Discarding the rest of a line that outgrew [`MAX_LINE`].
    skipping: bool,
}

/// The longest line accepted from a compositor (a window list is a few
/// hundred KiB at most).
pub(crate) const MAX_LINE: usize = 16 << 20;
/// Bytes asked of the socket at a time.
const CHUNK: usize = 16 << 10;

/// The next complete line of `pending`, without its newline. A line longer
/// than `max` is dropped (the whole of it, however it arrives), never
/// buffered.
pub(crate) fn take_line(pending: &mut Pending, max: usize) -> Option<String> {
    loop {
        let Some(end) = pending.bytes.iter().position(|b| *b == b'\n') else {
            if pending.bytes.len() > max {
                pending.bytes.clear();
                pending.skipping = true;
                tracing::debug!("compositor: dropping a line longer than {max} bytes");
            }
            return None;
        };
        let line: Vec<u8> = pending.bytes.drain(..=end).collect();
        if std::mem::take(&mut pending.skipping) {
            continue;
        }
        if line.len() > max + 1 {
            tracing::debug!("compositor: dropped a line longer than {max} bytes");
            continue;
        }
        let text = String::from_utf8_lossy(&line[..end]);
        let text = text.trim_end_matches('\r');
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
}

/// One attempt to read a message.
pub(crate) enum Next<T> {
    Item(T),
    /// Nothing more arrived in the time allowed.
    Idle,
    /// The peer closed the stream.
    Closed,
}

/// A stream socket read with a timeout and without losing partial
/// messages to it.
pub(crate) struct Inbox {
    stream: UnixStream,
    pending: Pending,
}

impl Inbox {
    pub fn new(stream: UnixStream) -> Self {
        Self { stream, pending: Pending::default() }
    }

    /// The next message `parse` cuts from what has arrived, waiting up to
    /// `wait` (forever for `None`) for the rest of it.
    pub fn next<T>(&mut self, wait: Option<Duration>, mut parse: impl FnMut(&mut Pending) -> Result<Option<T>>) -> Result<Next<T>> {
        let deadline = wait.map(|w| Instant::now() + w);
        let mut chunk = [0u8; CHUNK];
        loop {
            if let Some(item) = parse(&mut self.pending)? {
                return Ok(Next::Item(item));
            }
            let remaining = match deadline {
                Some(at) => match at.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) {
                    Some(d) => Some(d),
                    None => return Ok(Next::Idle),
                },
                None => None,
            };
            self.stream.set_read_timeout(remaining)?;
            match self.stream.read(&mut chunk) {
                Ok(0) => return Ok(Next::Closed),
                Ok(n) => self.pending.bytes.extend_from_slice(&chunk[..n]),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return Ok(Next::Idle),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// The next line.
    pub fn line(&mut self, wait: Option<Duration>) -> Result<Next<String>> {
        self.next(wait, |pending| Ok(take_line(pending, MAX_LINE)))
    }

    /// [`Self::burst`] of lines.
    pub fn burst_lines(&mut self, burst: Burst, each: impl FnMut(String)) -> Result<bool> {
        self.burst(burst, |pending| Ok(take_line(pending, MAX_LINE)), each)
    }

    /// Hands every message of one burst to `each` and returns whether the
    /// stream is still open. A burst ends once the stream has been quiet
    /// for [`Burst::quiet`] or [`Burst::limit`] after it began.
    pub fn burst<T>(&mut self, burst: Burst, mut parse: impl FnMut(&mut Pending) -> Result<Option<T>>, mut each: impl FnMut(T)) -> Result<bool> {
        let began = Instant::now();
        while let Some(wait) = burst.wait(began, Instant::now()) {
            match self.next(Some(wait), &mut parse)? {
                Next::Item(item) => each(item),
                Next::Idle => return Ok(true),
                Next::Closed => return Ok(false),
            }
        }
        Ok(true)
    }
}

/// How a burst of events is coalesced into one refresh: events arriving
/// within `quiet` of the last one join it, up to `limit` after the first
/// (a stream that never pauses still refreshes).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Burst {
    pub quiet: Duration,
    pub limit: Duration,
}

impl Burst {
    /// A compositor emits the several events of one change back to back:
    /// well inside a frame.
    pub const EVENTS: Burst = Burst { quiet: Duration::from_millis(25), limit: Duration::from_millis(200) };

    /// How much longer to listen at `now`, for a burst that began at
    /// `began`; `None` once `limit` is spent.
    pub fn wait(&self, began: Instant, now: Instant) -> Option<Duration> {
        let left = self.limit.checked_sub(now.saturating_duration_since(began)).filter(|d| !d.is_zero())?;
        Some(left.min(self.quiet))
    }
}

/// Drops an event the shell already has: the last one sent of each kind
/// (per monitor for navigation).
#[derive(Default)]
pub(crate) struct Dedup {
    last: HashMap<(std::mem::Discriminant<CompositorEvent>, Option<String>), CompositorEvent>,
}

impl Dedup {
    /// `event` if it differs from the last of its kind.
    pub fn fresh(&mut self, event: CompositorEvent) -> Option<CompositorEvent> {
        let monitor = match &event {
            CompositorEvent::Navigation { monitor, .. } => Some(monitor.clone()),
            _ => None,
        };
        let key = (std::mem::discriminant(&event), monitor);
        if self.last.get(&key) == Some(&event) {
            return None;
        }
        self.last.insert(key, event.clone());
        Some(event)
    }

    /// Sends what is new in `events`; `false` once the receiver is gone.
    pub fn send(&mut self, tx: &async_channel::Sender<CompositorEvent>, events: impl IntoIterator<Item = CompositorEvent>) -> bool {
        events.into_iter().filter_map(|e| self.fresh(e)).all(|e| tx.send_blocking(e).is_ok())
    }
}

/// Logs the first message that had to be skipped at `warn` and the rest at
/// `debug`, so a persistent unparsable event can't flood the log.
#[derive(Default)]
pub(crate) struct SkipLog {
    warned: bool,
}

impl SkipLog {
    pub fn skip(&mut self, what: &str, err: &dyn std::fmt::Display) {
        if std::mem::replace(&mut self.warned, true) {
            tracing::debug!("{what}: skipped: {err}");
        } else {
            tracing::warn!("{what}: skipping what it can't parse (further ones logged at debug): {err}");
        }
    }
}

/// The newest state a watcher thread has seen, for queries to answer from
/// instead of opening a connection of their own.
pub(crate) struct Live<T>(Mutex<Option<Arc<T>>>);

impl<T> Live<T> {
    pub const fn new() -> Self {
        Self(Mutex::new(None))
    }

    pub fn publish(&self, state: Arc<T>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(state);
    }

    /// Forgets the state: its watcher is gone, so it would only go stale.
    pub fn clear(&self) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    pub fn get(&self) -> Option<Arc<T>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ClientInfo, NavigationModel};
    use std::io::Write as _;

    fn pending(bytes: &[u8]) -> Pending {
        Pending { bytes: bytes.to_vec(), skipping: false }
    }

    #[test]
    fn backoff_grows_to_the_cap_and_restarts_after_a_healthy_stream() {
        let mut b = Backoff::new(Duration::from_secs(1));
        let first = b.delay();
        assert!(first >= Duration::from_secs(1) && first < Duration::from_millis(1250));
        let second = b.delay();
        assert!(second >= Duration::from_secs(2) && second < Duration::from_millis(2500));
        for _ in 0..10 {
            b.delay();
        }
        let capped = b.delay();
        assert!(capped >= MAX_RECONNECT_DELAY && capped < MAX_RECONNECT_DELAY + MAX_RECONNECT_DELAY / 4);
        // A stream that dropped at once keeps the long pause...
        assert!(b.after(Duration::from_millis(50)) >= MAX_RECONNECT_DELAY);
        // ...one that was healthy starts over.
        assert!(b.after(STABLE_STREAM) < Duration::from_millis(1250));
    }

    #[test]
    fn lines_are_cut_whole_and_overlong_ones_dropped() {
        let mut p = pending(b"one\r\n\ntwo\npart");
        assert_eq!(take_line(&mut p, 16).as_deref(), Some("one"));
        assert_eq!(take_line(&mut p, 16).as_deref(), Some("two"));
        assert_eq!(take_line(&mut p, 16), None);
        p.bytes.extend_from_slice(b"ial\n");
        assert_eq!(take_line(&mut p, 16).as_deref(), Some("partial"));
        // Longer than allowed, already complete.
        let mut p = pending(b"0123456789abcdefXY\nok\n");
        assert_eq!(take_line(&mut p, 16).as_deref(), Some("ok"));
        // Longer than allowed and still arriving: nothing is kept, and the
        // rest of that line is discarded when its end comes.
        let mut p = pending(b"0123456789abcdefXY");
        assert_eq!(take_line(&mut p, 16), None);
        assert!(p.bytes.is_empty() && p.skipping);
        p.bytes.extend_from_slice(b"more of it\nnext\n");
        assert_eq!(take_line(&mut p, 16).as_deref(), Some("next"));
        assert!(!p.skipping);
    }

    #[test]
    fn a_burst_ends_when_quiet_or_at_its_limit() {
        let burst = Burst { quiet: Duration::from_millis(25), limit: Duration::from_millis(100) };
        let began = Instant::now();
        let at = |ms| began + Duration::from_millis(ms);
        assert_eq!(burst.wait(began, at(0)), Some(Duration::from_millis(25)));
        assert_eq!(burst.wait(began, at(90)), Some(Duration::from_millis(10)), "never past the limit");
        assert_eq!(burst.wait(began, at(100)), None);
        assert_eq!(burst.wait(began, at(500)), None);
    }

    #[test]
    fn a_burst_of_lines_is_one_batch_and_a_pause_ends_it() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        let mut inbox = Inbox::new(reader);
        writer.write_all(b"workspace>>1\nworkspacev2>>1,one\nfocusedmon>>DP-1,1\n").unwrap();
        let burst = Burst { quiet: Duration::from_millis(40), limit: Duration::from_secs(5) };
        let mut got = Vec::new();
        let open = inbox.burst(burst, |p| Ok(take_line(p, MAX_LINE)), |l| got.push(l)).unwrap();
        assert!(open);
        assert_eq!(got, ["workspace>>1", "workspacev2>>1,one", "focusedmon>>DP-1,1"]);
        // A partial line survives the pause that ended the burst.
        writer.write_all(b"half").unwrap();
        assert!(matches!(inbox.line(Some(Duration::from_millis(20))).unwrap(), Next::Idle));
        writer.write_all(b" a line\n").unwrap();
        assert!(matches!(inbox.line(Some(Duration::from_secs(5))).unwrap(), Next::Item(l) if l == "half a line"));
        drop(writer);
        assert!(matches!(inbox.line(Some(Duration::from_secs(5))).unwrap(), Next::Closed));
    }

    #[test]
    fn a_stream_that_never_pauses_still_ends_its_burst() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        let mut inbox = Inbox::new(reader);
        let feeder = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_millis(600);
            while Instant::now() < until {
                if writer.write_all(b"title\n").is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let burst = Burst { quiet: Duration::from_millis(250), limit: Duration::from_millis(100) };
        let began = Instant::now();
        let mut count = 0;
        assert!(inbox.burst(burst, |p| Ok(take_line(p, MAX_LINE)), |_| count += 1).unwrap());
        assert!(began.elapsed() < Duration::from_millis(400), "ended by the limit, not the pause: {:?}", began.elapsed());
        assert!(count > 1);
        drop(inbox);
        feeder.join().unwrap();
    }

    #[test]
    fn events_the_shell_already_has_are_dropped() {
        let nav = |monitor: &str, active: u32| CompositorEvent::Navigation {
            monitor: monitor.into(),
            model: NavigationModel::Workspaces {
                workspaces: vec![crate::model::WorkspaceStatus { id: active, name: active.to_string(), active: true, occupied: false, urgent: false, layout: String::new() }],
            },
        };
        let mut dedup = Dedup::default();
        assert!(dedup.fresh(nav("DP-1", 1)).is_some());
        assert!(dedup.fresh(nav("DP-1", 1)).is_none(), "the same workspace state again");
        assert!(dedup.fresh(nav("DP-2", 1)).is_some(), "another monitor is its own state");
        assert!(dedup.fresh(nav("DP-1", 2)).is_some());
        assert!(dedup.fresh(nav("DP-1", 1)).is_some(), "going back is a change");
        let title = |t: &str| CompositorEvent::FocusedClient(Some(ClientInfo { title: t.into(), ..Default::default() }));
        assert!(dedup.fresh(title("a")).is_some() && dedup.fresh(title("a")).is_none() && dedup.fresh(title("b")).is_some());
        assert!(dedup.fresh(CompositorEvent::FocusedClient(None)).is_some() && dedup.fresh(CompositorEvent::FocusedClient(None)).is_none());
        let (tx, rx) = async_channel::bounded(8);
        assert!(dedup.send(&tx, [title("b"), CompositorEvent::Keymode("x".into()), CompositorEvent::Keymode("x".into())]));
        assert_eq!(std::iter::from_fn(|| rx.try_recv().ok()).count(), 2, "the repeated title and the repeated keymode");
        drop(rx);
        assert!(!dedup.send(&tx, [CompositorEvent::Keymode("y".into())]), "a closed receiver stops the watcher");
    }

    #[test]
    fn live_state_is_served_until_its_watcher_clears_it() {
        let live = Live::new();
        assert!(live.get().is_none());
        live.publish(Arc::new(1));
        live.publish(Arc::new(2));
        assert_eq!(live.get().as_deref(), Some(&2));
        live.clear();
        assert!(live.get().is_none());
    }
}
