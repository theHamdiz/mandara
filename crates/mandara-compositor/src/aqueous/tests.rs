//! The driver against a mock Aqueous socket: the handshake and session
//! token, snapshots, acknowledged deltas, command shapes, malformed input
//! and reconnection.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::state::tests::{SESSION, snapshot_batch};
use super::*;

const WAIT: Duration = Duration::from_secs(10);

fn hello_reply(session: &str, max_frame_bytes: usize) -> String {
    json!({"ipc":1,"id":"1","ok":true,"result":{"session":session,"schema":1,"max_request_bytes":65536,"max_frame_bytes":max_frame_bytes,"max_batch_bytes":4194304,"max_pending_requests":1,"max_clients":16,"max_state_bytes":2097152,"max_depth":16,"capabilities":{"state":true,"commands":true,"keyboard":true,"overview":true,"shortcut_inhibition":true}}}).to_string()
}

/// One accepted client of the mock compositor.
struct Peer {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Peer {
    fn accept(listener: &UnixListener) -> Self {
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        Self { reader: BufReader::new(stream.try_clone().unwrap()), writer: stream }
    }

    fn recv(&mut self) -> Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        assert!(line.ends_with('\n'), "a request is one LF-terminated line: {line:?}");
        serde_json::from_str(&line).unwrap()
    }

    fn send(&mut self, frame: &str) {
        // Frames are single lines: compact what the tests write readably.
        let compact = serde_json::to_string(&serde_json::from_str::<Value>(frame).unwrap()).unwrap();
        writeln!(self.writer, "{compact}").unwrap();
    }

    fn reply(&mut self, id: &str, result: Value) {
        self.send(&json!({"ipc":1,"id":id,"ok":true,"result":result}).to_string());
    }

    fn expect(&mut self, id: &str, op: &str, params: Value) {
        assert_eq!(self.recv(), json!({"ipc":1,"id":id,"session":SESSION,"op":op,"params":params}));
    }

    fn hello(&mut self) {
        assert_eq!(self.recv(), json!({"ipc":1,"id":"1","op":"hello","params":{}}), "hello first, without a session");
        self.send(&hello_reply(SESSION, 4_259_840));
    }

    fn state(&mut self, delivery: &str, batch: &str) {
        let batch: Value = serde_json::from_str(batch).unwrap();
        self.send(&json!({"ipc":1,"event":"state","delivery":delivery,"batch":batch}).to_string());
    }
}

fn listen() -> (tempfile::TempDir, PathBuf, UnixListener) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ipc.sock");
    let listener = UnixListener::bind(&path).unwrap();
    (dir, path, listener)
}

fn snapshot_value(sequence: &str) -> Value {
    serde_json::from_str(&snapshot_batch(sequence)).unwrap()
}

#[test]
fn queries_and_commands_use_the_request_connection() {
    let (_dir, path, listener) = listen();
    let (done_tx, done) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut p = Peer::accept(&listener);
        p.hello();
        p.expect("2", "snapshot", json!({}));
        p.reply("2", json!({"batch": snapshot_value("1")}));
        // No subscription yet: a command is resolved against a snapshot.
        p.expect("3", "snapshot", json!({}));
        p.reply("3", json!({"batch": snapshot_value("1")}));
        p.expect("4", "command", json!({"action":"workspace.activate","fields":{"id":"11"}}));
        p.send(r#"{"ipc":1,"id":"4","ok":false,"error":{"code":"stale_session","message":"stale_session"}}"#);
        // A stale session drops that connection; the next command opens
        // a new one (ids start over) and is not a replay of the first.
        let mut p = Peer::accept(&listener);
        p.hello();
        p.expect("2", "snapshot", json!({}));
        p.reply("2", json!({"batch": snapshot_value("2")}));
        p.expect("3", "command", json!({"action":"window.move","fields":{"id":"w-a","workspace":"2"}}));
        p.reply("3", json!({"status":"applied","sequence":"3"}));
        // A server error for one command keeps the connection.
        p.expect("4", "snapshot", json!({}));
        p.reply("4", json!({"batch": snapshot_value("3")}));
        p.expect("5", "command", json!({"action":"overview.toggle","fields":{"output":"1"}}));
        p.send(r#"{"ipc":1,"id":"5","ok":false,"error":{"code":"unavailable","message":"Overview needs a window"}}"#);
        p.expect("6", "snapshot", json!({}));
        p.reply("6", json!({"batch": snapshot_value("4")}));
        p.expect("7", "command", json!({"action":"keyboard.next","fields":{}}));
        p.reply("7", json!({"status":"applied","sequence":"5"}));
        done_tx.send(()).unwrap();
    });
    let driver = AqueousDriver::at(path, Duration::from_millis(10));
    let nav = driver.get_navigation_state().unwrap();
    assert_eq!(nav.iter().map(|(m, _)| m.as_str()).collect::<Vec<_>>(), ["DP-1", "HDMI-A-1"]);
    // These return at once: the request thread does the I/O.
    let started = Instant::now();
    driver.select_primary("HDMI-A-1", 2).unwrap();
    driver.move_client_to(None, 2).unwrap();
    driver.dispatch_raw("overview.toggle", &[]).unwrap();
    driver.switch_keyboard_layout().unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    done.recv_timeout(WAIT).expect("every request reached the compositor");
    server.join().unwrap();
}

#[test]
fn a_request_connection_the_compositor_closed_is_reopened_before_a_command() {
    let (_dir, path, listener) = listen();
    let (closed_tx, closed) = std::sync::mpsc::channel();
    let (done_tx, done) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut p = Peer::accept(&listener);
        p.hello();
        p.expect("2", "snapshot", json!({}));
        p.reply("2", json!({"batch": snapshot_value("1")}));
        drop(p);
        closed_tx.send(()).unwrap();
        // The command arrives on a fresh connection, never on the dead one.
        let mut p = Peer::accept(&listener);
        p.hello();
        p.expect("2", "snapshot", json!({}));
        p.reply("2", json!({"batch": snapshot_value("2")}));
        p.expect("3", "command", json!({"action":"workspace.activate","fields":{"id":"12"}}));
        p.reply("3", json!({"status":"applied","sequence":"3"}));
        done_tx.send(()).unwrap();
    });
    let driver = AqueousDriver::at(path, Duration::from_millis(10));
    assert_eq!(driver.get_monitors().unwrap().len(), 2);
    closed.recv_timeout(WAIT).unwrap();
    driver.select_primary("HDMI-A-1", 3).unwrap();
    done.recv_timeout(WAIT).expect("the command was sent on a new connection");
    server.join().unwrap();
}

fn collect(rx: &async_channel::Receiver<CompositorEvent>, until: impl Fn(&[CompositorEvent]) -> bool) -> Vec<CompositorEvent> {
    let deadline = Instant::now() + WAIT;
    let mut got = Vec::new();
    while !until(&got) {
        assert!(Instant::now() < deadline, "timed out with {got:?}");
        match rx.try_recv() {
            Ok(e) => got.push(e),
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    got
}

fn navigation_count(events: &[CompositorEvent]) -> usize {
    events.iter().filter(|e| matches!(e, CompositorEvent::Navigation { .. })).count()
}

#[test]
fn the_event_stream_acks_each_batch_and_reconnects() {
    let (_dir, path, listener) = listen();
    let (release_tx, release) = std::sync::mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        let mut p = Peer::accept(&listener);
        p.hello();
        p.expect("2", "subscribe", json!({}));
        p.reply("2", json!({"subscribed": true}));
        p.state("1", &snapshot_batch("1"));
        p.expect("3", "ack", json!({"delivery":"1"}));
        p.reply("3", json!({"acked":"1"}));
        p.state("2", &format!(r#"{{"schema":1,"session":"{SESSION}","sequence":"4","base_sequence":"1","type":"delta","upsert":[{{"kind":"workspace","id":"12","output":"2","name":"final 🐟 \"quoted\"","number":3,"active":false,"urgent":false}}],"removed":[]}}"#));
        p.expect("4", "ack", json!({"delivery":"2"}));
        p.reply("4", json!({"acked":"2"}));
        // A delta against a sequence the client never accepted: the
        // client must drop the stream rather than continue.
        p.state("3", &format!(r#"{{"schema":1,"session":"{SESSION}","sequence":"9","base_sequence":"7","type":"delta","upsert":[],"removed":[]}}"#));
        let mut line = String::new();
        assert_eq!(p.reader.read_line(&mut line).unwrap(), 0, "no ack for a broken sequence, the connection closes: {line:?}");
        // It comes back with a fresh subscription and snapshot.
        let mut p = Peer::accept(&listener);
        p.hello();
        p.expect("2", "subscribe", json!({}));
        p.reply("2", json!({"subscribed": true}));
        p.state("1", &snapshot_batch("12"));
        p.expect("3", "ack", json!({"delivery":"1"}));
        p.reply("3", json!({"acked":"1"}));
        let _ = release.recv_timeout(WAIT);
    });
    let driver = AqueousDriver::at(path, Duration::from_millis(10));
    let rx = driver.subscribe_events();
    let first = collect(&rx, |e| navigation_count(e) == 2 && e.iter().any(|e| matches!(e, CompositorEvent::KeyboardLayout(_))));
    assert!(first.iter().any(|e| matches!(e, CompositorEvent::FocusedClient(Some(c)) if c.app_id == "foot" && c.monitor == "DP-1")), "{first:?}");
    assert!(first.iter().any(|e| matches!(e, CompositorEvent::Monitors(m) if m.len() == 2)));
    assert!(first.iter().any(|e| matches!(e, CompositorEvent::Clients(c) if c.len() == 2)));
    let delta = collect(&rx, |e| navigation_count(e) == 1);
    assert!(matches!(&delta[..], [CompositorEvent::Navigation { monitor, model: NavigationModel::Workspaces { workspaces } }] if monitor == "HDMI-A-1" && workspaces[2].name == "final 🐟 \"quoted\""), "{delta:?}");
    // After the reconnect, everything is sent again from the new snapshot.
    let again = collect(&rx, |e| navigation_count(e) == 2 && e.iter().any(|e| matches!(e, CompositorEvent::Monitors(_))));
    assert!(again.iter().any(|e| matches!(e, CompositorEvent::Navigation { monitor, model: NavigationModel::Workspaces { workspaces } } if monitor == "HDMI-A-1" && workspaces[2].name == "3")));
    // With a live model, queries need no request connection (the mock
    // would never answer one).
    let deadline = Instant::now() + WAIT;
    while !driver.shared.live().ready() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let started = Instant::now();
    assert_eq!(driver.get_monitors().unwrap().len(), 2);
    assert_eq!(driver.get_active_client().unwrap().map(|c| c.title), Some("shell".into()));
    assert!(started.elapsed() < Duration::from_secs(1));
    release_tx.send(()).unwrap();
    server.join().unwrap();
}

#[test]
fn malformed_and_oversized_frames_are_refused() {
    let (_dir, path, listener) = listen();
    let server = std::thread::spawn(move || {
        // Not a session token.
        let mut p = Peer::accept(&listener);
        p.recv();
        p.send(&hello_reply("NOT-HEX", 4_259_840));
        // Not JSON.
        let mut p = Peer::accept(&listener);
        p.recv();
        writeln!(p.writer, "{{\"ipc\":1,\"id\":").unwrap();
        // A reply to another request.
        let mut p = Peer::accept(&listener);
        p.recv();
        p.send(&hello_reply(SESSION, 4_259_840).replace("\"id\":\"1\"", "\"id\":\"7\""));
        // A frame over the advertised limit on a subscription.
        let mut p = Peer::accept(&listener);
        p.recv();
        p.send(&hello_reply(SESSION, 1024));
        p.expect("2", "subscribe", json!({}));
        p.reply("2", json!({"subscribed": true}));
        writeln!(p.writer, "{}", "x".repeat(2048)).unwrap();
        // An event while an ack is pending.
        let mut p = Peer::accept(&listener);
        p.hello();
        p.expect("2", "subscribe", json!({}));
        p.reply("2", json!({"subscribed": true}));
        p.state("1", &snapshot_batch("1"));
        p.expect("3", "ack", json!({"delivery":"1"}));
        p.state("2", &snapshot_batch("2"));
    });
    let open = || Conn::open(&path, Duration::from_secs(5));
    assert!(open().is_err(), "an invalid session token");
    assert!(open().is_err(), "malformed JSON");
    assert!(open().is_err(), "a reply to another id");
    let mut conn = open().unwrap();
    assert!(conn.request("subscribe", Empty {}).unwrap().subscribed == Some(true));
    assert!(conn.next_event().unwrap_err().to_string().contains("limit"));
    let mut conn = open().unwrap();
    conn.request("subscribe", Empty {}).unwrap();
    let (delivery, _) = conn.next_event().unwrap().unwrap();
    assert!(conn.request("ack", Ack { delivery: &delivery }).unwrap_err().to_string().contains("event"));
    server.join().unwrap();
}

fn model() -> Model {
    let mut m = Model::new(SESSION);
    m.apply(state::tests::batch(&snapshot_batch("1"))).unwrap();
    m
}

#[test]
fn intents_resolve_to_documented_actions() {
    let m = model();
    let raw = |action: &str, fields: Value| Intent::Raw { action: action.into(), fields: serde_json::from_value(fields).unwrap() };
    let show = |(action, fields): (String, Map<String, Value>)| (action, Value::Object(fields));
    assert_eq!(show(resolve(&Intent::Activate { monitor: "DP-1".into(), number: 2 }, &m).unwrap()), ("workspace.activate".into(), json!({"id":"2"})));
    assert_eq!(show(resolve(&Intent::Activate { monitor: String::new(), number: 1 }, &m).unwrap()), ("workspace.activate".into(), json!({"id":"1"})));
    assert!(resolve(&Intent::Activate { monitor: "DP-9".into(), number: 1 }, &m).is_err());
    assert!(resolve(&Intent::Activate { monitor: "DP-1".into(), number: 7 }, &m).is_err());
    assert_eq!(show(resolve(&Intent::Move { client: Some(crate::umbriel::client_id("w-b")), number: 3 }, &m).unwrap()), ("window.move".into(), json!({"id":"w-b","workspace":"12"})), "its own output");
    assert!(resolve(&Intent::Move { client: Some(42), number: 1 }, &m).is_err());
    assert_eq!(show(resolve(&raw("window.close", json!({})), &m).unwrap()), ("window.close".into(), json!({"id":"w-a"})), "the focused window");
    assert_eq!(show(resolve(&raw("window.close", json!({"id":"w-b"})), &m).unwrap()), ("window.close".into(), json!({"id":"w-b"})));
    assert_eq!(show(resolve(&raw("overview.show", json!({})), &m).unwrap()), ("overview.show".into(), json!({"output":"1"})));
    assert_eq!(show(resolve(&raw("session.reload", json!({})), &m).unwrap()), ("session.reload".into(), json!({})));
    // With two seats, the default one is named.
    let mut two = model();
    two.seats.push(state::Seat { id: "seat1".into(), output: None, window: None, keyboard: None });
    assert_eq!(show(resolve(&Intent::Activate { monitor: "DP-1".into(), number: 1 }, &two).unwrap()), ("workspace.activate".into(), json!({"id":"1","seat":"seat0"})));
}

#[test]
fn unsupported_requests_say_so_without_touching_the_socket() {
    let driver = AqueousDriver::at(PathBuf::from("/nonexistent/aqueous/ipc.sock"), Duration::from_millis(10));
    assert_eq!(driver.kind(), CompositorKind::Aqueous);
    assert_eq!(driver.target(), mandara_config::compositor::Target::Aqueous);
    assert!(driver.cycle_layout("DP-1").is_err());
    assert!(driver.reload_config().is_ok(), "nothing of Mandara's to reload");
    assert!(driver.sync_managed_config(&Default::default(), None).is_err(), "no drop-in for Aqueous");
    assert!(driver.dispatch_raw("Overview Toggle", &[]).is_err());
    assert!(driver.dispatch_raw("window.close", &["[1]"]).is_err());
    assert!(driver.dispatch_raw("window.close", &["{}", "{}"]).is_err());
    assert!(driver.jobs.lock().unwrap().is_none(), "nothing was started");
    // Once the event stream owns the state, a query never waits on the
    // socket (some callers are on the UI thread): no state, no answer.
    driver.shared.subscribed.store(true, Ordering::Release);
    let started = Instant::now();
    assert!(driver.get_monitors().is_err());
    assert!(started.elapsed() < Duration::from_millis(100));
    assert!(driver.jobs.lock().unwrap().is_none(), "no request thread for a query");
}

#[test]
fn the_socket_comes_only_from_the_environment() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ipc.sock");
    let _listener = UnixListener::bind(&path).unwrap();
    let driver = AqueousDriver::from_var(Some(path.clone().into())).expect("an absolute path to a socket");
    assert_eq!(driver.shared.socket, path);
    assert!(AqueousDriver::from_var(None).is_none(), "never guessed");
    assert!(AqueousDriver::from_var(Some("".into())).is_none());
    assert!(AqueousDriver::from_var(Some("aqueous/ipc.sock".into())).is_none(), "relative paths are refused");
    assert!(AqueousDriver::from_var(Some(dir.path().join("missing.sock").into())).is_none());
    assert!(driver.jobs.lock().unwrap().is_none(), "construction never connects");
}

#[test]
fn backoff_grows_with_jitter_up_to_a_cap() {
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
    b.reset();
    assert!(b.delay() < Duration::from_millis(1250));
}
