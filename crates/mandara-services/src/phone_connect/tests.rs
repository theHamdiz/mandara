use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

use super::*;

fn phone() -> Device {
    Device { id: "phone_01".into(), name: "Demo phone".into(), kind: "phone".into(), reachable: true, paired: true, incoming_pair: false, battery: None, plugins: vec!["kdeconnect_findmyphone".into(), "kdeconnect_ping".into(), "kdeconnect_share".into()] }
}

#[test]
fn offline_or_unpaired_devices_never_receive_content_or_ring_requests() {
    for (paired, reachable) in [(false, true), (true, false), (false, false)] {
        let device = Device { paired, reachable, ..phone() };
        for action in [Action::Ring, Action::Ping, Action::ShareText("private text".into())] {
            assert!(action.validate(&device).is_err());
        }
    }
    assert!(Action::ShareText("  ".into()).validate(&phone()).is_err());
    assert!(Action::ShareText("x".repeat(MAX_SHARED_TEXT_BYTES + 1)).validate(&phone()).is_err());
    assert!(Action::Ring.validate(&Device { plugins: Vec::new(), ..phone() }).is_err());
}

#[test]
fn pairing_requires_an_actual_request_or_a_reachable_unpaired_device() {
    assert!(Action::AcceptPair.validate(&phone()).is_err());
    let device = Device { paired: false, incoming_pair: true, ..phone() };
    assert!(Action::AcceptPair.validate(&device).is_ok());
    assert!(Action::RejectPair.validate(&device).is_ok());
    assert!(Action::Pair.validate(&device).is_ok());
    assert!(Action::Unpair.validate(&device).is_err());
}

struct Daemon;
#[zbus::interface(name = "org.kde.kdeconnect.daemon")]
impl Daemon {
    #[zbus(name = "devices")]
    fn devices(&self, _reachable: bool, _paired: bool) -> Vec<String> { vec!["phone_01".into()] }
}

struct Phone { paired: bool }
#[zbus::interface(name = "org.kde.kdeconnect.device")]
impl Phone {
    #[zbus(property, name = "name")]
    fn name(&self) -> &str { "Demo phone" }
    #[zbus(property, name = "type")]
    fn kind(&self) -> &str { "phone" }
    #[zbus(property, name = "isPaired")]
    fn paired(&self) -> bool { self.paired }
    #[zbus(property, name = "isReachable")]
    fn reachable(&self) -> bool { true }
    #[zbus(property, name = "isPairRequestedByPeer")]
    fn incoming(&self) -> bool { false }
    #[zbus(name = "loadedPlugins")]
    fn plugins(&self) -> Vec<String> { vec!["kdeconnect_battery".into(), "kdeconnect_findmyphone".into()] }
}

struct PhoneBattery;
#[zbus::interface(name = "org.kde.kdeconnect.device.battery")]
impl PhoneBattery {
    #[zbus(property, name = "charge")]
    fn charge(&self) -> i32 { 73 }
    #[zbus(property, name = "isCharging")]
    fn charging(&self) -> bool { true }
    #[zbus(property, name = "hasBattery")]
    fn has_battery(&self) -> bool { true }
}

struct Ringer(Arc<AtomicUsize>);
#[zbus::interface(name = "org.kde.kdeconnect.device.findmyphone")]
impl Ringer {
    #[zbus(name = "ring")]
    fn ring(&self) { self.0.fetch_add(1, Ordering::Relaxed); }
}

async fn fixture(paired: bool) -> (zbus::Connection, zbus::Connection, Arc<AtomicUsize>) {
    let rings = Arc::new(AtomicUsize::new(0));
    let (client, server) = std::os::unix::net::UnixStream::pair().unwrap();
    let server = zbus::connection::Builder::async_io_unix_stream(server).server(zbus::Guid::generate()).unwrap().p2p()
        .serve_at("/modules/kdeconnect", Daemon).unwrap()
        .serve_at("/modules/kdeconnect/devices/phone_01", Phone { paired }).unwrap()
        .serve_at("/modules/kdeconnect/devices/phone_01/battery", PhoneBattery).unwrap()
        .serve_at("/modules/kdeconnect/devices/phone_01/findmyphone", Ringer(rings.clone())).unwrap();
    let client = zbus::connection::Builder::async_io_unix_stream(client).p2p();
    let (client, server) = futures::join!(crate::runtime::connect(client), crate::runtime::connect(server));
    (client.unwrap(), server.unwrap(), rings)
}

#[test]
fn native_bus_properties_and_actions_use_kde_connect_names() {
    crate::runtime::block_on(async {
        let (client, _server, rings) = fixture(true).await;
        let devices = super::dbus::devices_on(&client).await.unwrap();
        assert_eq!(devices[0].battery, Some(Battery { percent: 73, charging: true }));
        super::dbus::act_on(&client, "phone_01", Action::Ring).await.unwrap();
        assert_eq!(rings.load(Ordering::Relaxed), 1);
    });
}

#[test]
fn the_bus_backend_rechecks_pairing_before_an_action() {
    crate::runtime::block_on(async {
        let (client, _server, rings) = fixture(false).await;
        assert!(super::dbus::act_on(&client, "phone_01", Action::Ring).await.is_err());
        assert_eq!(rings.load(Ordering::Relaxed), 0);
    });
}
