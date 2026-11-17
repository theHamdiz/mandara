//! BlueZ fixtures use a private socket pair; neither desktop bus is opened.

use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}};

use super::*;

/// Enough RSSI traffic to fill the actor's subscription before a method reply.
const SIGNAL_BURST: usize = crate::actor::SIGNAL_QUEUE * 2;
/// The negative control must demonstrate blocked replies without slowing CI.
const BLOCKED_LIMIT: Duration = Duration::from_secs(1);
/// Bounds every positive assertion, including failures in the mock server.
const TEST_LIMIT: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Snapshot {
    resumed: AtomicBool,
    burst: AtomicBool,
    calls: AtomicUsize,
}

struct Bluez(Arc<Snapshot>);

#[zbus::interface(name = "org.freedesktop.DBus.ObjectManager")]
impl Bluez {
    async fn get_managed_objects(&self, #[zbus(connection)] conn: &Connection) -> Managed {
        self.0.calls.fetch_add(1, Ordering::Relaxed);
        if self.0.burst.swap(false, Ordering::Relaxed) {
            for rssi in 0..SIGNAL_BURST {
                conn.emit_signal(None::<&str>, "/org/bluez/hci0", "org.freedesktop.DBus.Properties", "PropertiesChanged", &(rssi as u32,)).await.unwrap();
            }
        }
        let resumed = self.0.resumed.load(Ordering::Relaxed);
        let adapter = HashMap::from([
            ("Powered".into(), true.into()),
            ("Discovering".into(), (!resumed).into()),
        ]);
        let device = HashMap::from([
            ("Alias".into(), OwnedValue::from(zbus::zvariant::Str::from("Headphones"))),
            ("Connected".into(), resumed.into()),
            ("Paired".into(), true.into()),
        ]);
        HashMap::from([
            (OwnedObjectPath::try_from("/org/bluez/hci0").unwrap(), HashMap::from([("org.bluez.Adapter1".into(), adapter)])),
            (OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA_BB").unwrap(), HashMap::from([("org.bluez.Device1".into(), device)])),
        ])
    }
}

async fn fixture(burst: bool) -> (Connection, Connection, Arc<Snapshot>, zbus::MessageStream) {
    let snapshot = Arc::new(Snapshot { burst: AtomicBool::new(burst), ..Default::default() });
    let (client, server) = std::os::unix::net::UnixStream::pair().unwrap();
    let server = zbus::connection::Builder::async_io_unix_stream(server).server(zbus::Guid::generate()).unwrap().p2p().serve_at("/", Bluez(snapshot.clone())).unwrap();
    let client = zbus::connection::Builder::async_io_unix_stream(client).p2p();
    let (client, server) = futures::join!(runtime::connect(client), runtime::connect(server));
    let client = client.unwrap();
    let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).build();
    let signals = zbus::MessageStream::for_match_rule(rule, &client, Some(crate::actor::SIGNAL_QUEUE)).await.unwrap();
    (client, server.unwrap(), snapshot, signals)
}

async fn within<T>(limit: Duration, future: impl std::future::Future<Output = T>) -> Option<T> {
    futures::pin_mut!(future);
    futures::select! {
        result = future.fuse() => Some(result),
        _ = futures::FutureExt::fuse(async_io::Timer::after(limit)) => None,
    }
}

#[test]
fn unconsumed_bluez_signals_can_block_the_state_reply() {
    runtime::block_on(async {
        let (client, _server, _, _signals) = fixture(true).await;
        assert!(within(BLOCKED_LIMIT, read_state(&client, None)).await.is_none(), "negative control must reproduce the blocked socket reader");
    });
}

#[test]
fn signal_bursts_and_silent_resume_refresh_publish_authoritative_state() {
    runtime::block_on(async {
        let (client, _server, snapshot, signals) = fixture(true).await;
        let (state_tx, states) = async_channel::bounded::<BluetoothState>(UPDATE_QUEUE);
        let (commands, incoming) = async_channel::bounded::<Envelope<BluetoothCommand>>(COMMAND_QUEUE);
        let exercise = async move {
            let first = within(TEST_LIMIT, states.recv()).await.unwrap().unwrap();
            assert!(first.discovering && !first.devices[0].connected);
            // Resume changed BlueZ without delivering a corresponding signal.
            snapshot.resumed.store(true, Ordering::Relaxed);
            let (reply_tx, reply_rx) = async_channel::bounded(1);
            commands.send(Envelope { command: BluetoothCommand::Refresh, reply: Some(reply_tx) }).await.unwrap();
            let refreshed = within(TEST_LIMIT, states.recv()).await.unwrap().unwrap();
            assert!(!refreshed.discovering && refreshed.devices[0].connected);
            let response = within(TEST_LIMIT, reply_rx.recv()).await.unwrap().unwrap().unwrap();
            let response: serde_json::Value = serde_json::from_str(&response).unwrap();
            assert_eq!(response["discovering"], false);
            assert_eq!(response["devices"][0]["connected"], true);
            assert!(snapshot.calls.load(Ordering::Relaxed) < SIGNAL_BURST, "signals must coalesce into state reads");
            drop(commands);
        };
        let actor = run_with_signals(&client, signals, &state_tx, &incoming);
        let (result, ()) = within(TEST_LIMIT, futures::future::join(actor, exercise)).await.expect("actor must exit when command handles are dropped");
        result.unwrap();
    });
}
