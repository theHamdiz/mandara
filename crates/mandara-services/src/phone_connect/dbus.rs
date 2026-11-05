use std::future::Future;
use std::time::Duration;

use futures::FutureExt as _;
use mandara_i18n::{t, tf};

use super::{Action, Backend, Battery, Device};

const DESTINATION: &str = "org.kde.kdeconnect";
const ROOT: &str = "/modules/kdeconnect";
const DAEMON: &str = "org.kde.kdeconnect.daemon";
const DEVICE: &str = "org.kde.kdeconnect.device";
/// Bound a complete refresh, including a missing or stalled daemon.
const TIMEOUT: Duration = Duration::from_secs(5);
/// A desktop device list should stay small even if a peer reports bad data.
const MAX_DEVICES: usize = 64;
const MAX_ID_BYTES: usize = 256;

#[derive(Default)]
pub struct KdeConnect;

/// Build a device object path only from a valid single-segment id.
fn path(id: &str) -> Result<String, String> {
    if id.is_empty() || id.len() > MAX_ID_BYTES || !id.bytes().all(|ch| ch.is_ascii_alphanumeric() || ch == b'_') {
        return Err(t("Invalid phone identifier").into());
    }
    Ok(format!("{ROOT}/devices/{id}"))
}

async fn bounded<T>(work: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    futures::pin_mut!(work);
    futures::select! {
        result = work.fuse() => result,
        _ = async_io::Timer::after(TIMEOUT).fuse() => Err(t("KDE Connect did not respond in time").into()),
    }
}

fn error(error: impl std::fmt::Display) -> String {
    tf("Could not reach KDE Connect: {error}", &[("error", &error)]).to_string()
}

/// Read one device and the capabilities its daemon actually exposes.
async fn device(conn: &zbus::Connection, id: &str) -> Result<Device, String> {
    let path = path(id)?;
    let proxy = crate::actor::proxy(conn, DESTINATION, &path, DEVICE).await.map_err(error)?;
    let name = proxy.get_property("name").await.map_err(error)?;
    let kind = proxy.get_property("type").await.map_err(error)?;
    let paired = proxy.get_property("isPaired").await.map_err(error)?;
    let reachable = proxy.get_property("isReachable").await.map_err(error)?;
    let incoming_pair = proxy.get_property("isPairRequestedByPeer").await.unwrap_or(false);
    let plugins: Vec<String> = if paired && reachable { proxy.call("loadedPlugins", &()).await.map_err(error)? } else { Vec::new() };
    let battery = if plugins.iter().any(|id| id == "kdeconnect_battery") {
        let path = format!("{path}/battery");
        let interface = format!("{DEVICE}.battery");
        let battery = crate::actor::proxy(conn, DESTINATION, &path, &interface).await.map_err(error)?;
        let has_battery = battery.get_property::<bool>("hasBattery").await.unwrap_or(true);
        let percent = battery.get_property::<i32>("charge").await.ok().and_then(|charge| u8::try_from(charge).ok()).filter(|charge| *charge <= 100);
        let charging = battery.get_property("isCharging").await.unwrap_or(false);
        percent.filter(|_| has_battery).map(|percent| Battery { percent, charging })
    } else { None };
    Ok(Device { id: id.into(), name, kind, reachable, paired, incoming_pair, battery, plugins })
}

/// Skip unreadable device objects without hiding the remaining devices.
pub(super) async fn devices_on(conn: &zbus::Connection) -> Result<Vec<Device>, String> {
    let daemon = crate::actor::proxy(conn, DESTINATION, ROOT, DAEMON).await.map_err(error)?;
    let ids: Vec<String> = daemon.call("devices", &(false, false)).await.map_err(error)?;
    let mut devices = Vec::new();
    for id in ids.into_iter().take(MAX_DEVICES) {
        match device(conn, &id).await {
            Ok(device) => devices.push(device),
            Err(error) => tracing::debug!("skipping KDE Connect device: {error}"),
        }
    }
    devices.sort_by(|a, b| b.connected().cmp(&a.connected()).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then_with(|| a.id.cmp(&b.id)));
    Ok(devices)
}

/// Re-read pairing, reachability and capabilities before sending an explicit action.
pub(super) async fn act_on(conn: &zbus::Connection, id: &str, action: Action) -> Result<(), String> {
    let device = device(conn, id).await?;
    action.validate(&device)?;
    let base = path(id)?;
    let (suffix, method) = match &action {
        Action::Ring => ("findmyphone", "ring"),
        Action::Ping => ("ping", "sendPing"),
        Action::ShareText(_) => ("share", "shareText"),
        Action::Pair => ("", "requestPairing"),
        Action::AcceptPair => ("", "acceptPairing"),
        Action::RejectPair => ("", "cancelPairing"),
        Action::Unpair => ("", "unpair"),
    };
    let (path, interface) = if suffix.is_empty() { (base, DEVICE.into()) } else { (format!("{base}/{suffix}"), format!("{DEVICE}.{suffix}")) };
    let proxy = crate::actor::proxy(conn, DESTINATION, &path, &interface).await.map_err(error)?;
    match action {
        Action::ShareText(text) => proxy.call::<_, _, ()>(method, &(text,)).await.map_err(error),
        _ => proxy.call::<_, _, ()>(method, &()).await.map_err(error),
    }
}

impl Backend for KdeConnect {
    /// List phone state on the shared session bus within the bounded request lifetime.
    fn devices(&self) -> Result<Vec<Device>, String> {
        crate::runtime::block_on(bounded(async {
            let conn = crate::runtime::session().await.map_err(error)?;
            devices_on(&conn).await
        }))
    }

    /// Run a phone action through the session bus with the common request deadline.
    fn act(&self, id: &str, action: Action) -> Result<(), String> {
        crate::runtime::block_on(bounded(async {
            let conn = crate::runtime::session().await.map_err(error)?;
            act_on(&conn, id, action).await
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Device identifiers cannot select another D-Bus object or add path components.
    #[test]
    fn ids_cannot_escape_the_device_object() {
        assert_eq!(path("phone_01").unwrap(), "/modules/kdeconnect/devices/phone_01");
        for id in ["", "../other", "phone/battery", "x-y", "a.b"] {
            assert!(path(id).is_err());
        }
    }
}
