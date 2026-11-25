//! The subscription loop. A change in sway is several events (a window
//! opening is `new`, `focus` and often a workspace event), so messages are
//! gathered into a burst, folded into what must be refetched ([`Dirty`]),
//! and answered with one fetch of each.

use std::io::Write as _;
use std::os::unix::net::UnixStream;
use std::time::Instant;

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

use super::{EVENTS, SwayDriver, SwayState, SwayWorkspace, encode, event, keyboard_layout_name, msg, parse_header, read_message};
use crate::backend::CompositorBackend;
use crate::model::CompositorEvent;
use crate::stream::{Backoff, Burst, Dedup, Inbox, Next, Pending, SkipLog};

/// What a burst of subscription messages asks for.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Dirty {
    /// The workspace list changed.
    workspaces: bool,
    /// The layout tree changed: windows, focus, titles, occupancy.
    tree: bool,
    outputs: bool,
    /// Mode and layout changes carry their value: sent as they came.
    inline: Vec<CompositorEvent>,
    shutdown: bool,
}

impl Dirty {
    /// Folds in one message of the subscription. A payload that can't be
    /// read is an error for the caller to skip: the next message is
    /// unaffected.
    pub fn note(&mut self, kind: u32, payload: &[u8]) -> Result<()> {
        match kind {
            event::WORKSPACE => {
                self.workspaces = true;
                self.tree = true;
            }
            event::WINDOW => {
                let body: Value = serde_json::from_slice(payload)?;
                match body.get("change").and_then(Value::as_str) {
                    // Marks are no part of what the shell shows.
                    Some("mark") => {}
                    // A window moved between workspaces reorders them.
                    Some("move") => {
                        self.workspaces = true;
                        self.tree = true;
                    }
                    // The window list, its focus and titles live in the tree;
                    // the workspace list is unchanged by them (a switch of
                    // workspace is a workspace event of its own).
                    _ => self.tree = true,
                }
            }
            event::OUTPUT => {
                self.outputs = true;
                self.workspaces = true;
                self.tree = true;
            }
            event::MODE => {
                let body: Value = serde_json::from_slice(payload)?;
                self.inline.push(CompositorEvent::Keymode(body.get("change").and_then(Value::as_str).unwrap_or_default().to_string()));
            }
            event::INPUT => {
                let body: Value = serde_json::from_slice(payload)?;
                let layout_change = matches!(body.get("change").and_then(Value::as_str), Some("xkb_layout" | "xkb_keymap"));
                self.inline.extend(body.get("input").filter(|_| layout_change).and_then(keyboard_layout_name).map(CompositorEvent::KeyboardLayout));
            }
            event::SHUTDOWN => self.shutdown = true,
            _ => {}
        }
        Ok(())
    }

    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The next whole frame of `pending`: `(type, payload)`.
pub(super) fn take_frame(pending: &mut Pending) -> Result<Option<(u32, Vec<u8>)>> {
    let Some(header) = pending.bytes.first_chunk::<{ super::HEADER_LEN }>() else { return Ok(None) };
    let (len, kind) = parse_header(header)?;
    let end = super::HEADER_LEN + len;
    if pending.bytes.len() < end {
        return Ok(None);
    }
    let payload = pending.bytes[super::HEADER_LEN..end].to_vec();
    pending.bytes.drain(..end);
    Ok(Some((kind, payload)))
}

/// Streams the subscription until the receiver is dropped, reconnecting
/// with backoff whenever it ends.
pub(super) fn watch(driver: &SwayDriver, tx: &async_channel::Sender<CompositorEvent>) {
    let mut backoff = Backoff::new(crate::RECONNECT_DELAY);
    let mut dedup = Dedup::default();
    loop {
        let began = Instant::now();
        match connected(driver, tx, &mut dedup) {
            Ok(true) => return,
            Ok(false) => {}
            Err(err) => tracing::debug!("sway event stream ended: {err:#}"),
        }
        if tx.is_closed() {
            return;
        }
        std::thread::sleep(backoff.after(began.elapsed()));
    }
}

/// One connection's worth of events; `true` when the receiver is gone.
fn connected(driver: &SwayDriver, tx: &async_channel::Sender<CompositorEvent>, dedup: &mut Dedup) -> Result<bool> {
    let mut stream = UnixStream::connect(&driver.socket).with_context(|| format!("connecting {}", driver.socket.display()))?;
    stream.set_read_timeout(Some(crate::REQUEST_TIMEOUT))?;
    stream.write_all(&encode(msg::SUBSCRIBE, serde_json::to_string(&EVENTS)?.as_bytes()))?;
    let (kind, body) = read_message(&mut stream)?;
    let reply: Value = serde_json::from_slice(&body)?;
    if kind != msg::SUBSCRIBE || reply.get("success").and_then(Value::as_bool) != Some(true) {
        bail!("sway: subscription refused: {reply}");
    }
    let mut inbox = Inbox::new(stream);
    let mut state = SwayState::default();
    let mut workspaces = Vec::new();
    // Query only after subscribing, so intervening changes are queued.
    if !dedup.send(tx, driver.snapshot(&mut state, &mut workspaces)) {
        return Ok(true);
    }
    let mut skipped = SkipLog::default();
    loop {
        let (kind, payload) = match inbox.next(None, take_frame)? {
            Next::Item(frame) => frame,
            Next::Idle => continue,
            Next::Closed => return Ok(false),
        };
        let mut dirty = Dirty::default();
        if let Err(err) = dirty.note(kind, &payload) {
            skipped.skip("sway event", &err);
        }
        if dirty.is_empty() {
            continue;
        }
        let open = inbox.burst(
            Burst::EVENTS,
            take_frame,
            |(kind, payload)| {
                if let Err(err) = dirty.note(kind, &payload) {
                    skipped.skip("sway event", &err);
                }
            },
        )?;
        let events = driver.refresh(&dirty, &mut state, &mut workspaces);
        if !dedup.send(tx, events) {
            return Ok(true);
        }
        if dirty.shutdown {
            bail!("sway is shutting down");
        }
        if !open {
            return Ok(false);
        }
    }
}

impl SwayDriver {
    /// The events that bring the shell up to date with `dirty`. A fetch that
    /// fails leaves its part of the shell as it was.
    fn refresh(&self, dirty: &Dirty, state: &mut SwayState, workspaces: &mut Vec<SwayWorkspace>) -> Vec<CompositorEvent> {
        let mut out = Vec::new();
        if dirty.outputs {
            match self.get_monitors() {
                Ok(monitors) => out.push(CompositorEvent::Monitors(monitors)),
                Err(err) => tracing::debug!("sway outputs: {err:#}"),
            }
        }
        if dirty.workspaces {
            match self.workspaces() {
                Ok(fresh) => *workspaces = fresh,
                Err(err) => tracing::debug!("sway workspaces: {err:#}"),
            }
        }
        if dirty.tree || dirty.workspaces {
            match self.tree() {
                Ok(tree) => out.extend(state.update(workspaces, &tree)),
                Err(err) => tracing::debug!("sway tree: {err:#}"),
            }
        }
        out.extend(dirty.inline.iter().cloned());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn dirty(events: &[(u32, &str)]) -> Dirty {
        let mut dirty = Dirty::default();
        for (kind, body) in events {
            dirty.note(*kind, body.as_bytes()).unwrap();
        }
        dirty
    }

    #[test]
    fn a_window_event_refetches_the_tree_and_only_a_move_the_workspaces() {
        for change in ["new", "close", "focus", "title", "urgent", "floating", "fullscreen_mode"] {
            let d = dirty(&[(event::WINDOW, &format!(r#"{{"change":"{change}"}}"#))]);
            assert_eq!(d, Dirty { tree: true, ..Dirty::default() }, "{change}");
        }
        assert_eq!(dirty(&[(event::WINDOW, r#"{"change":"move"}"#)]), Dirty { tree: true, workspaces: true, ..Dirty::default() });
        assert!(dirty(&[(event::WINDOW, r#"{"change":"mark"}"#)]).is_empty(), "marks change nothing shown");
    }

    #[test]
    fn a_burst_of_events_folds_into_one_refresh() {
        let burst = dirty(&[
            (event::WINDOW, r#"{"change":"new"}"#),
            (event::WINDOW, r#"{"change":"focus"}"#),
            (event::WORKSPACE, r#"{"change":"focus"}"#),
            (event::WINDOW, r#"{"change":"title"}"#),
            (event::OUTPUT, "{}"),
        ]);
        assert_eq!(burst, Dirty { workspaces: true, tree: true, outputs: true, ..Dirty::default() });
    }

    #[test]
    fn mode_and_layout_ride_in_their_events_and_bad_payloads_are_errors() {
        let d = dirty(&[
            (event::MODE, r#"{"change":"resize"}"#),
            (event::INPUT, r#"{"change":"xkb_layout","input":{"type":"keyboard","xkb_active_layout_name":"Arabic"}}"#),
            (event::INPUT, r#"{"change":"added","input":{"type":"keyboard","xkb_active_layout_name":"French"}}"#),
        ]);
        assert_eq!(d.inline, [CompositorEvent::Keymode("resize".into()), CompositorEvent::KeyboardLayout("Arabic".into())]);
        assert!(!d.workspaces && !d.tree);
        let mut d = Dirty::default();
        assert!(d.note(event::WINDOW, b"{not json").is_err());
        assert!(d.is_empty(), "a bad event changes nothing");
        d.note(event::SHUTDOWN, b"{}").unwrap();
        assert!(d.shutdown);
    }

    #[test]
    fn frames_are_cut_whole_across_reads() {
        let mut bytes = encode(event::MODE, br#"{"change":"a"}"#);
        bytes.extend(encode(event::WINDOW, b"{}"));
        let mut pending = Pending::default();
        pending.bytes.extend_from_slice(&bytes[..10]);
        assert!(take_frame(&mut pending).unwrap().is_none(), "a partial header");
        pending.bytes.extend_from_slice(&bytes[10..bytes.len() - 3]);
        assert_eq!(take_frame(&mut pending).unwrap(), Some((event::MODE, br#"{"change":"a"}"#.to_vec())));
        assert!(take_frame(&mut pending).unwrap().is_none(), "a partial payload");
        pending.bytes.extend_from_slice(&bytes[bytes.len() - 3..]);
        assert_eq!(take_frame(&mut pending).unwrap(), Some((event::WINDOW, b"{}".to_vec())));
        assert!(pending.bytes.is_empty());
        pending.bytes.extend_from_slice(b"not an i3-ipc frame at all");
        assert!(take_frame(&mut pending).is_err());
    }

    /// A sway stand-in: answers the queries from the captured fixtures,
    /// counts them, and keeps the subscriber's stream to push events on.
    struct Sway {
        dir: std::path::PathBuf,
        counts: Arc<Mutex<Vec<u32>>>,
        subscriber: Arc<Mutex<Option<UnixStream>>>,
    }

    impl Drop for Sway {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sway").join(name)).unwrap()
    }

    impl Sway {
        fn start(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("mdr-sway-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let listener = UnixListener::bind(dir.join("sway.sock")).unwrap();
            let counts = Arc::new(Mutex::new(Vec::new()));
            let subscriber = Arc::new(Mutex::new(None));
            let (log, slot) = (counts.clone(), subscriber.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { return };
                    let (log, slot) = (log.clone(), slot.clone());
                    std::thread::spawn(move || {
                        loop {
                            let mut header = [0u8; super::super::HEADER_LEN];
                            if stream.read_exact(&mut header).is_err() {
                                return;
                            }
                            let (len, kind) = parse_header(&header).unwrap();
                            let mut payload = vec![0u8; len];
                            let _ = stream.read_exact(&mut payload);
                            log.lock().unwrap().push(kind);
                            let reply = match kind {
                                msg::SUBSCRIBE => r#"{"success":true}"#.to_string(),
                                msg::GET_WORKSPACES => fixture("workspaces.json"),
                                msg::GET_TREE => fixture("tree.json"),
                                msg::GET_OUTPUTS => fixture("outputs.json"),
                                msg::GET_BINDING_STATE => fixture("binding_state.json"),
                                _ => "[]".to_string(),
                            };
                            let _ = stream.write_all(&encode(kind, reply.as_bytes()));
                            if kind == msg::SUBSCRIBE {
                                *slot.lock().unwrap() = stream.try_clone().ok();
                                return;
                            }
                        }
                    });
                }
            });
            Self { dir, counts, subscriber }
        }

        fn driver(&self) -> SwayDriver {
            SwayDriver::with_socket(self.dir.join("sway.sock"))
        }

        fn push(&self, events: &[(u32, &str)]) {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(stream) = self.subscriber.lock().unwrap().as_mut() {
                    let bytes: Vec<u8> = events.iter().flat_map(|(kind, body)| encode(*kind, body.as_bytes())).collect();
                    stream.write_all(&bytes).unwrap();
                    return;
                }
                assert!(Instant::now() < deadline, "the watcher never subscribed");
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn count(&self, kind: u32) -> usize {
            self.counts.lock().unwrap().iter().filter(|k| **k == kind).count()
        }
    }

    fn settle(rx: &async_channel::Receiver<CompositorEvent>) -> Vec<CompositorEvent> {
        std::thread::sleep(Duration::from_millis(400));
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    #[test]
    fn a_burst_of_window_and_workspace_events_costs_one_fetch_of_each() {
        let sway = Sway::start("burst");
        let rx = sway.driver().subscribe_events();
        let initial = settle(&rx);
        assert!(initial.iter().any(|e| matches!(e, CompositorEvent::Navigation { .. })) && initial.iter().any(|e| matches!(e, CompositorEvent::Monitors(_))));
        let (workspaces, trees) = (sway.count(msg::GET_WORKSPACES), sway.count(msg::GET_TREE));
        assert_eq!((workspaces, trees), (1, 1), "the snapshot is one of each");

        sway.push(&[
            (event::WINDOW, r#"{"change":"new"}"#),
            (event::WINDOW, r#"{"change":"focus"}"#),
            (event::WORKSPACE, r#"{"change":"focus"}"#),
            (event::WINDOW, r#"{"change":"title"}"#),
        ]);
        let after = settle(&rx);
        assert_eq!((sway.count(msg::GET_WORKSPACES), sway.count(msg::GET_TREE)), (2, 2), "four events, one fetch of each");
        assert!(after.is_empty(), "the fixtures did not change: nothing is resent: {after:?}");

        // A title change needs the tree only.
        sway.push(&[(event::WINDOW, r#"{"change":"title"}"#)]);
        settle(&rx);
        assert_eq!((sway.count(msg::GET_WORKSPACES), sway.count(msg::GET_TREE)), (2, 3));
        // Marks need nothing.
        sway.push(&[(event::WINDOW, r#"{"change":"mark"}"#), (event::WINDOW, "{garbage")]);
        settle(&rx);
        assert_eq!((sway.count(msg::GET_WORKSPACES), sway.count(msg::GET_TREE)), (2, 3));
        // And a bad payload did not end the subscription.
        sway.push(&[(event::MODE, r#"{"change":"resize"}"#)]);
        assert_eq!(settle(&rx), [CompositorEvent::Keymode("resize".into())]);
    }
}
