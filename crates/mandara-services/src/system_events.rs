//! System changes worth a sound or an on-screen display: the charger being
//! plugged in / out (UPower `OnBattery`), the power profile switching
//! (power-profiles-daemon `ActiveProfile`, either bus generation) and a
//! drive being plugged in or removed (UDisks2 adding or removing a `Drive`
//! object). One task listens to those signals on the system bus and costs
//! nothing in between.

use futures::StreamExt as _;
use futures::stream::BoxStream;
use zbus::fdo::{ObjectManagerProxy, PropertiesChanged, PropertiesProxy};
use zbus::names::InterfaceName;

const UPOWER: (&str, &str, &str) = ("org.freedesktop.UPower", "/org/freedesktop/UPower", "org.freedesktop.UPower");
/// power-profiles-daemon: current and legacy bus names.
const PROFILES: [(&str, &str, &str); 2] = [
    ("org.freedesktop.UPower.PowerProfiles", "/org/freedesktop/UPower/PowerProfiles", "org.freedesktop.UPower.PowerProfiles"),
    ("net.hadess.PowerProfiles", "/net/hadess/PowerProfiles", "net.hadess.PowerProfiles"),
];
/// UDisks2's object manager and its drive interface.
const UDISKS: (&str, &str) = ("org.freedesktop.UDisks2", "/org/freedesktop/UDisks2");
const UDISKS_DRIVE: &str = "org.freedesktop.UDisks2.Drive";
const UPDATE_QUEUE: usize = 8;

#[derive(Clone, Debug, PartialEq)]
pub enum SystemEvent {
    /// The machine switched to / from battery power.
    OnBattery(bool),
    /// The active power profile changed (`power-saver`, `balanced`, `performance`).
    PowerProfile(String),
    /// A drive was plugged in (`true`) or removed (`false`).
    Drive(bool),
}

async fn properties(conn: &zbus::Connection, (dest, path, _): (&'static str, &'static str, &'static str)) -> zbus::Result<PropertiesProxy<'static>> {
    PropertiesProxy::builder(conn).destination(dest)?.path(path)?.cache_properties(zbus::proxy::CacheProperties::No).build().await
}

async fn udisks(conn: &zbus::Connection) -> zbus::Result<ObjectManagerProxy<'static>> {
    ObjectManagerProxy::builder(conn).destination(UDISKS.0)?.path(UDISKS.1)?.build().await
}

/// A charger or power-profile change from a `PropertiesChanged` signal.
fn properties_event(signal: &PropertiesChanged) -> Option<SystemEvent> {
    let args = signal.args().ok()?;
    let iface: &InterfaceName = args.interface_name();
    let changed = args.changed_properties();
    if iface.as_str() == UPOWER.2 {
        changed.get("OnBattery").and_then(|v| bool::try_from(v).ok()).map(SystemEvent::OnBattery)
    } else if PROFILES.iter().any(|p| p.2 == iface.as_str()) {
        changed.get("ActiveProfile").and_then(|v| String::try_from(v.try_clone().ok()?).ok()).map(SystemEvent::PowerProfile)
    } else {
        None
    }
}

/// Follows charger, power-profile and drive changes until the receiver is
/// dropped.
pub fn watch() -> async_channel::Receiver<SystemEvent> {
    let (tx, rx) = async_channel::bounded(UPDATE_QUEUE);
    crate::runtime::spawn("system-events", async move {
        let Ok(conn) = crate::runtime::system().await else { return };
        let mut streams: Vec<BoxStream<'static, Option<SystemEvent>>> = Vec::new();
        if let Ok(p) = properties(&conn, UPOWER).await
            && let Ok(s) = p.receive_properties_changed().await
        {
            streams.push(s.map(|signal| properties_event(&signal)).boxed());
        }
        for daemon in PROFILES {
            if let Ok(p) = properties(&conn, daemon).await
                && let Ok(s) = p.receive_properties_changed().await
            {
                streams.push(s.map(|signal| properties_event(&signal)).boxed());
            }
        }
        // Drives only: block devices, jobs and loop devices come and go too.
        if let Ok(om) = udisks(&conn).await {
            if let Ok(s) = om.receive_interfaces_added().await {
                streams.push(s.map(|signal| signal.args().ok().filter(|a| a.interfaces_and_properties().keys().any(|i| i.as_str() == UDISKS_DRIVE)).map(|_| SystemEvent::Drive(true))).boxed());
            }
            if let Ok(s) = om.receive_interfaces_removed().await {
                streams.push(s.map(|signal| signal.args().ok().filter(|a| a.interfaces().iter().any(|i| i.as_str() == UDISKS_DRIVE)).map(|_| SystemEvent::Drive(false))).boxed());
            }
        }
        let mut all = futures::stream::select_all(streams);
        while let Some(event) = all.next().await {
            if let Some(event) = event
                && tx.send(event).await.is_err()
            {
                return;
            }
        }
    });
    rx
}
