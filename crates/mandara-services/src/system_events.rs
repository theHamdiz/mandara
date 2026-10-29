//! System changes worth a sound or an on-screen display: the charger being
//! plugged in / out (UPower `OnBattery`) and the power profile switching
//! (power-profiles-daemon `ActiveProfile`, either bus generation). One task
//! listens to `PropertiesChanged` on the system bus and costs nothing in
//! between.

use futures::StreamExt as _;
use zbus::fdo::PropertiesProxy;
use zbus::names::InterfaceName;

const UPOWER: (&str, &str, &str) = ("org.freedesktop.UPower", "/org/freedesktop/UPower", "org.freedesktop.UPower");
/// power-profiles-daemon: current and legacy bus names.
const PROFILES: [(&str, &str, &str); 2] = [
    ("org.freedesktop.UPower.PowerProfiles", "/org/freedesktop/UPower/PowerProfiles", "org.freedesktop.UPower.PowerProfiles"),
    ("net.hadess.PowerProfiles", "/net/hadess/PowerProfiles", "net.hadess.PowerProfiles"),
];
const UPDATE_QUEUE: usize = 8;

#[derive(Clone, Debug, PartialEq)]
pub enum SystemEvent {
    /// The machine switched to / from battery power.
    OnBattery(bool),
    /// The active power profile changed (`power-saver`, `balanced`, `performance`).
    PowerProfile(String),
}

async fn properties(conn: &zbus::Connection, (dest, path, _): (&'static str, &'static str, &'static str)) -> zbus::Result<PropertiesProxy<'static>> {
    PropertiesProxy::builder(conn).destination(dest)?.path(path)?.cache_properties(zbus::proxy::CacheProperties::No).build().await
}

/// Follows charger and power-profile changes until the receiver is dropped.
pub fn watch() -> async_channel::Receiver<SystemEvent> {
    let (tx, rx) = async_channel::bounded(UPDATE_QUEUE);
    crate::runtime::spawn("system-events", async move {
        let Ok(conn) = crate::runtime::system().await else { return };
        let mut streams = Vec::new();
        if let Ok(p) = properties(&conn, UPOWER).await
            && let Ok(s) = p.receive_properties_changed().await
        {
            streams.push(s.boxed());
        }
        for daemon in PROFILES {
            if let Ok(p) = properties(&conn, daemon).await
                && let Ok(s) = p.receive_properties_changed().await
            {
                streams.push(s.boxed());
            }
        }
        let mut all = futures::stream::select_all(streams);
        while let Some(signal) = all.next().await {
            let Ok(args) = signal.args() else { continue };
            let iface: &InterfaceName = args.interface_name();
            let changed = args.changed_properties();
            let event = if iface.as_str() == UPOWER.2 {
                changed.get("OnBattery").and_then(|v| bool::try_from(v).ok()).map(SystemEvent::OnBattery)
            } else if PROFILES.iter().any(|p| p.2 == iface.as_str()) {
                changed.get("ActiveProfile").and_then(|v| String::try_from(v.try_clone().ok()?).ok()).map(SystemEvent::PowerProfile)
            } else {
                None
            };
            if let Some(event) = event
                && tx.send(event).await.is_err()
            {
                return;
            }
        }
    });
    rx
}
