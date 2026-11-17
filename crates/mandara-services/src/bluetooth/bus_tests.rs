//! BlueZ fixtures use a private socket pair; neither desktop bus is opened.

use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}};

use super::*;

/// Enough RSSI traffic to fill the actor's subscription before a method reply.
const SIGNAL_BURST: usize = crate::actor::SIGNAL_QUEUE * 2;
/// The negative control must demonstrate blocked replies without slowing CI.
const BLOCKED_LIMIT: Duration = Duration::from_secs(1);
/// Bounds every positive assertion, including failures in the mock server.
const TEST_LIMIT: Duration = Duration::from_secs(5);
/// Allow the private server to observe an unexpected no-reply Connect call.
const NO_CONNECT_LIMIT: Duration = Duration::from_millis(50);

#[derive(Default)]
struct Snapshot {
    resumed: AtomicBool,
    burst: AtomicBool,
    calls: AtomicUsize,
    failed: AtomicBool,
    powered_off: AtomicBool,
}

struct Bluez(Arc<Snapshot>);

#[zbus::interface(name = "org.freedesktop.DBus.ObjectManager")]
impl Bluez {
    async fn get_managed_objects(&self, #[zbus(connection)] conn: &Connection) -> zbus::fdo::Result<Managed> {
        self.0.calls.fetch_add(1, Ordering::Relaxed);
        if self.0.failed.load(Ordering::Relaxed) {
            return Err(zbus::fdo::Error::Failed("temporary fixture read failure".into()));
        }
        if self.0.burst.swap(false, Ordering::Relaxed) {
            for _ in 0..SIGNAL_BURST {
                let properties = HashMap::from([("RSSI", OwnedValue::from(-40_i16))]);
                conn.emit_signal(None::<&str>, "/org/bluez/hci0/dev_AA_BB", "org.freedesktop.DBus.Properties", "PropertiesChanged", &(DEVICE_IFACE, properties, Vec::<String>::new())).await.map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
            }
        }
        let resumed = self.0.resumed.load(Ordering::Relaxed);
        let adapter = HashMap::from([
            ("Powered".into(), (!self.0.powered_off.load(Ordering::Relaxed)).into()),
            ("Discovering".into(), (!resumed).into()),
        ]);
        let device = HashMap::from([
            ("Alias".into(), OwnedValue::from(zbus::zvariant::Str::from("Headphones"))),
            ("Connected".into(), resumed.into()),
            ("Paired".into(), true.into()),
            ("Trusted".into(), true.into()),
        ]);
        Ok(HashMap::from([
            (OwnedObjectPath::try_from("/org/bluez/hci0").unwrap(), HashMap::from([("org.bluez.Adapter1".into(), adapter)])),
            (OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA_BB").unwrap(), HashMap::from([("org.bluez.Device1".into(), device)])),
        ]))
    }
}

struct Device(async_channel::Sender<()>);

#[zbus::interface(name = "org.bluez.Device1")]
impl Device {
    fn connect(&self) { let _ = self.0.try_send(()); }
}

async fn changed(conn: &Connection) {
    conn.emit_signal(None::<&str>, "/org/bluez/hci0", "org.freedesktop.DBus.Properties", "PropertiesChanged", &(ADAPTER_IFACE, HashMap::<String, OwnedValue>::new(), Vec::<String>::new())).await.unwrap();
}

async fn request(commands: &async_channel::Sender<Envelope<BluetoothCommand>>, command: BluetoothCommand) -> Result<String, String> {
    let (reply, response) = async_channel::bounded(1);
    commands.send(Envelope { command, reply: Some(reply) }).await.unwrap();
    within(TEST_LIMIT, response.recv()).await.unwrap().unwrap()
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
        let (client, server, snapshot, signals) = fixture(true).await;
        let (state_tx, states) = async_channel::bounded::<BluetoothState>(UPDATE_QUEUE);
        let (commands, incoming) = async_channel::bounded::<Envelope<BluetoothCommand>>(COMMAND_QUEUE);
        let exercise = async move {
            let first = within(TEST_LIMIT, states.recv()).await.unwrap().unwrap();
            assert!(first.discovering && !first.devices[0].connected);
            snapshot.resumed.store(true, Ordering::Relaxed);
            changed(&server).await;
            let signalled = within(TEST_LIMIT, states.recv()).await.unwrap().unwrap();
            assert!(!signalled.discovering && signalled.devices[0].connected);
            // Resume changed BlueZ without delivering a corresponding signal.
            snapshot.resumed.store(false, Ordering::Relaxed);
            let (reply_tx, reply_rx) = async_channel::bounded(1);
            commands.send(Envelope { command: BluetoothCommand::Refresh, reply: Some(reply_tx) }).await.unwrap();
            let refreshed = within(TEST_LIMIT, states.recv()).await.unwrap().unwrap();
            assert!(refreshed.discovering && !refreshed.devices[0].connected);
            let response = within(TEST_LIMIT, reply_rx.recv()).await.unwrap().unwrap().unwrap();
            let response: serde_json::Value = serde_json::from_str(&response).unwrap();
            assert_eq!(response["discovering"], true);
            assert_eq!(response["devices"][0]["connected"], false);
            assert!(snapshot.calls.load(Ordering::Relaxed) < SIGNAL_BURST, "signals must coalesce into state reads");
            drop(commands);
        };
        let actor = run_with_signals(&client, signals, &state_tx, &incoming);
        let (result, ()) = within(TEST_LIMIT, futures::future::join(actor, exercise)).await.expect("actor must exit when command handles are dropped");
        result.unwrap();
    });
}

#[test]
fn failed_refresh_keeps_the_last_successful_state() {
    runtime::block_on(async {
        let (client, server, snapshot, signals) = fixture(false).await;
        let (tx, states) = async_channel::bounded::<BluetoothState>(UPDATE_QUEUE);
        let (commands, rx) = async_channel::bounded::<Envelope<BluetoothCommand>>(COMMAND_QUEUE);
        let exercise = async move {
            let first = within(TEST_LIMIT, states.recv()).await.unwrap().unwrap();
            assert!(first.available && first.powered);
            snapshot.failed.store(true, Ordering::Relaxed);
            assert!(request(&commands, BluetoothCommand::Refresh).await.unwrap_err().contains("temporary fixture read failure"));
            assert!(states.try_recv().is_err(), "a failed read must not replace the known adapter with an empty state");
            snapshot.failed.store(false, Ordering::Relaxed);
            snapshot.resumed.store(true, Ordering::Relaxed);
            changed(&server).await;
            let recovered = within(TEST_LIMIT, states.recv()).await.unwrap().unwrap();
            assert!(recovered.available && recovered.devices[0].connected);
            drop(commands);
        };
        let (result, ()) = within(TEST_LIMIT, futures::future::join(run_with_signals(&client, signals, &tx, &rx), exercise)).await.unwrap();
        result.unwrap();
    });
}

#[test]
fn explicit_refresh_does_not_connect_devices_but_power_signals_still_do() {
    runtime::block_on(async {
        let (client, server, snapshot, signals) = fixture(false).await;
        snapshot.powered_off.store(true, Ordering::Relaxed);
        let (connected, connections) = async_channel::bounded(1);
        server.object_server().at("/org/bluez/hci0/dev_AA_BB", Device(connected)).await.unwrap();
        let (tx, states) = async_channel::bounded::<BluetoothState>(UPDATE_QUEUE);
        let (commands, rx) = async_channel::bounded::<Envelope<BluetoothCommand>>(COMMAND_QUEUE);
        let exercise = async move {
            assert!(!within(TEST_LIMIT, states.recv()).await.unwrap().unwrap().powered);
            request(&commands, BluetoothCommand::SetAutoConnect(true)).await.unwrap();
            request(&commands, BluetoothCommand::Refresh).await.unwrap();
            snapshot.powered_off.store(false, Ordering::Relaxed);
            request(&commands, BluetoothCommand::Refresh).await.unwrap();
            assert!(within(TEST_LIMIT, states.recv()).await.unwrap().unwrap().powered);
            assert!(within(NO_CONNECT_LIMIT, connections.recv()).await.is_none(), "explicit refresh must never connect a device");
            snapshot.powered_off.store(true, Ordering::Relaxed);
            changed(&server).await;
            assert!(!within(TEST_LIMIT, states.recv()).await.unwrap().unwrap().powered);
            snapshot.powered_off.store(false, Ordering::Relaxed);
            changed(&server).await;
            assert!(within(TEST_LIMIT, states.recv()).await.unwrap().unwrap().powered);
            within(TEST_LIMIT, connections.recv()).await.unwrap().unwrap();
            drop(commands);
        };
        let (result, ()) = within(TEST_LIMIT, futures::future::join(run_with_signals(&client, signals, &tx, &rx), exercise)).await.unwrap();
        result.unwrap();
    });
}
