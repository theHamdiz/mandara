//! NetworkManager actor (system bus): connectivity, Wi-Fi radio, the active
//! access point and nearby networks; connect / disconnect / toggle commands.
//!
//! Event-driven: NetworkManager's signals (state, radio, active connection,
//! access-point strength) trigger a debounced re-read;
//! a slow fallback poll covers anything a signal missed.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;
use zbus::Connection;
use zbus::zvariant::{OwnedObjectPath, Value};

use crate::actor::{Envelope, ServiceHandle, Wake, drain_signals, next_wake, proxy, reply, signals_from};
use crate::runtime;

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const AP_IFACE: &str = "org.freedesktop.NetworkManager.AccessPoint";
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";
/// `NMDeviceType` of a Wi-Fi device.
const DEVICE_TYPE_WIFI: u32 = 2;
/// `NM80211ApFlags`: the AP requires privacy (WEP or better).
const AP_FLAG_PRIVACY: u32 = 0x1;
/// `NMState` bounds: CONNECTING (40) up to, not including, CONNECTED_LOCAL (50);
/// CONNECTED_GLOBAL is 70.
const NM_STATE_CONNECTING: u32 = 40;
const NM_STATE_CONNECTED_GLOBAL: u32 = 70;
/// Signals coalesced into one re-read (a scan fires dozens at once).
const DEBOUNCE: Duration = Duration::from_millis(400);
/// Re-read even without signals this often.
const FALLBACK_POLL: Duration = Duration::from_secs(60);
/// State updates buffered for the UI.
const UPDATE_QUEUE: usize = 8;
/// Commands buffered from the UI and CLI.
const COMMAND_QUEUE: usize = 32;
/// Wi-Fi strength tiers: lower bound (percent) of tiers 1–4.
const STRENGTH_TIERS: [u8; 4] = [20, 40, 60, 80];

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct AccessPoint {
    pub ssid: String,
    /// Signal strength `0..=100`.
    pub strength: u8,
    pub secured: bool,
    pub active: bool,
    pub frequency_mhz: u32,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Connectivity {
    #[default]
    Disconnected,
    Connecting,
    Ethernet,
    Wifi,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct NetworkState {
    pub available: bool,
    pub connectivity: Connectivity,
    pub wifi_enabled: bool,
    pub wifi_hardware: bool,
    /// Name of the primary connection (SSID or wired profile name).
    pub primary: String,
    /// Active Wi-Fi strength `0..=100` (0 when not on Wi-Fi).
    pub strength: u8,
    /// Nearby networks, strongest first, deduplicated by SSID.
    pub access_points: Vec<AccessPoint>,
    /// Active VPN connections (NetworkManager VPN plugins and WireGuard).
    pub vpns: Vec<String>,
    /// Mandara's hotspot is running.
    pub hotspot: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NetworkCommand {
    SetWifi(bool),
    Scan,
    Connect { ssid: String, password: Option<String> },
    Disconnect,
}

pub type NetworkService = ServiceHandle<NetworkState, NetworkCommand>;

pub fn spawn_network_service() -> NetworkService {
    let (tx, updates) = async_channel::bounded(UPDATE_QUEUE);
    let (commands, rx) = async_channel::bounded(COMMAND_QUEUE);
    runtime::spawn("network", async move {
        // A private connection: its signal stream must never stall other
        // actors' method replies on a shared one.
        let conn = match zbus::connection::Builder::system() {
            Ok(b) => runtime::connect(b).await,
            Err(e) => Err(e),
        };
        let conn = match conn {
            Ok(c) => c,
            Err(err) => {
                tracing::warn!("network: no system bus ({err})");
                let _ = tx.send(NetworkState::default()).await;
                return;
            }
        };
        if let Err(err) = run(&conn, &tx, &rx).await {
            tracing::warn!("network: {err:#}");
        }
    });
    ServiceHandle::new(updates, commands)
}

async fn run(conn: &Connection, tx: &async_channel::Sender<NetworkState>, rx: &async_channel::Receiver<Envelope<NetworkCommand>>) -> Result<()> {
    let mut signals = signals_from(conn, NM).await?;
    let mut last: Option<NetworkState> = None;
    loop {
        drain_signals(&mut signals);
        let state = read_state(conn).await.unwrap_or_default();
        if last.as_ref() != Some(&state) {
            if tx.send(state.clone()).await.is_err() {
                return Ok(());
            }
            last = Some(state);
        }
        match next_wake(&mut signals, rx, DEBOUNCE, FALLBACK_POLL).await {
            Wake::Command(Envelope { command, reply: r }) => {
                let result = execute(conn, command).await.map_err(|e| format!("{e:#}"));
                reply(&r, result);
            }
            Wake::Refresh => {}
            Wake::Closed => return Ok(()),
        }
    }
}


/// `NM_ACTIVE_CONNECTION_STATE_ACTIVATED`: a VPN counts as connected only
/// then (not while activating — a VPN that then fails would announce
/// "connected" and "disconnected").
const NM_ACTIVE_CONNECTION_ACTIVATED: u32 = 2;

/// Active-connection types that are VPNs.
fn is_vpn_kind(kind: &str) -> bool {
    matches!(kind, "vpn" | "wireguard")
}

fn ssid_from_bytes(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim_end_matches('\0').to_string()
}

async fn devices(conn: &Connection) -> Result<Vec<(OwnedObjectPath, u32)>> {
    let nm = proxy(conn, NM, NM_PATH, NM).await?;
    let paths: Vec<OwnedObjectPath> = nm.call("GetDevices", &()).await?;
    let mut out = Vec::new();
    for path in paths {
        let dev = proxy(conn, NM, path.as_str(), "org.freedesktop.NetworkManager.Device").await?;
        let kind: u32 = dev.get_property("DeviceType").await.unwrap_or(0);
        out.push((path, kind));
    }
    Ok(out)
}

async fn wifi_device(conn: &Connection) -> Result<Option<OwnedObjectPath>> {
    Ok(devices(conn).await?.into_iter().find(|(_, k)| *k == DEVICE_TYPE_WIFI).map(|(p, _)| p))
}

/// One access point, read with a single `GetAll` round trip.
async fn read_ap(conn: &Connection, path: &str) -> Result<AccessPoint> {
    let props = proxy(conn, NM, path, PROPERTIES_IFACE).await?;
    let all: HashMap<String, zbus::zvariant::OwnedValue> = props.call("GetAll", &(AP_IFACE,)).await?;
    let get_u32 = |key: &str| all.get(key).and_then(|v| u32::try_from(v).ok()).unwrap_or(0);
    let ssid: Vec<u8> = all.get("Ssid").and_then(|v| v.try_clone().ok()).and_then(|v| Vec::<u8>::try_from(v).ok()).unwrap_or_default();
    Ok(AccessPoint {
        ssid: ssid_from_bytes(&ssid),
        strength: all.get("Strength").and_then(|v| u8::try_from(v).ok()).unwrap_or(0),
        secured: get_u32("Flags") & AP_FLAG_PRIVACY != 0 || get_u32("WpaFlags") != 0 || get_u32("RsnFlags") != 0,
        active: false,
        frequency_mhz: get_u32("Frequency"),
    })
}

async fn read_state(conn: &Connection) -> Result<NetworkState> {
    let nm = proxy(conn, NM, NM_PATH, NM).await?;
    let nm_state: u32 = nm.get_property("State").await?;
    let mut state = NetworkState {
        available: true,
        wifi_enabled: nm.get_property("WirelessEnabled").await.unwrap_or(false),
        wifi_hardware: nm.get_property("WirelessHardwareEnabled").await.unwrap_or(false),
        ..Default::default()
    };
    let primary: OwnedObjectPath = nm.get_property("PrimaryConnection").await?;
    if primary.as_str() != "/" {
        let active = proxy(conn, NM, primary.as_str(), "org.freedesktop.NetworkManager.Connection.Active").await?;
        state.primary = active.get_property("Id").await.unwrap_or_default();
        let kind: String = active.get_property("Type").await.unwrap_or_default();
        state.connectivity = match kind.as_str() {
            "802-11-wireless" => Connectivity::Wifi,
            "802-3-ethernet" => Connectivity::Ethernet,
            _ if nm_state >= NM_STATE_CONNECTED_GLOBAL => Connectivity::Ethernet,
            _ => Connectivity::Disconnected,
        };
    } else if (NM_STATE_CONNECTING..NM_STATE_CONNECTED_GLOBAL).contains(&nm_state) {
        state.connectivity = Connectivity::Connecting;
    }

    let active: Vec<OwnedObjectPath> = nm.get_property("ActiveConnections").await.unwrap_or_default();
    for path in active {
        let Ok(a) = proxy(conn, NM, path.as_str(), "org.freedesktop.NetworkManager.Connection.Active").await else { continue };
        let kind: String = a.get_property("Type").await.unwrap_or_default();
        let id: String = a.get_property("Id").await.unwrap_or_default();
        if is_vpn_kind(&kind) {
            let active_state: u32 = a.get_property("State").await.unwrap_or_default();
            if active_state == NM_ACTIVE_CONNECTION_ACTIVATED {
                state.vpns.push(id);
            }
        } else if id == crate::nm_admin::HOTSPOT_ID {
            state.hotspot = true;
        }
    }
    state.vpns.sort();

    if let Some(dev) = wifi_device(conn).await? {
        let wifi = proxy(conn, NM, dev.as_str(), "org.freedesktop.NetworkManager.Device.Wireless").await?;
        let active_ap: OwnedObjectPath = wifi.get_property("ActiveAccessPoint").await.unwrap_or_else(|_| OwnedObjectPath::default());
        let aps: Vec<OwnedObjectPath> = wifi.call("GetAllAccessPoints", &()).await.unwrap_or_default();
        let mut by_ssid: HashMap<String, AccessPoint> = HashMap::new();
        for path in aps {
            let Ok(mut ap) = read_ap(conn, path.as_str()).await else { continue };
            if ap.ssid.is_empty() {
                continue;
            }
            ap.active = path == active_ap;
            if ap.active {
                state.strength = ap.strength;
            }
            let entry = by_ssid.entry(ap.ssid.clone()).or_insert_with(|| ap.clone());
            if ap.active || (ap.strength > entry.strength && !entry.active) {
                *entry = ap;
            }
        }
        let mut list: Vec<AccessPoint> = by_ssid.into_values().collect();
        list.sort_by(|a, b| b.active.cmp(&a.active).then(b.strength.cmp(&a.strength)).then(a.ssid.cmp(&b.ssid)));
        state.access_points = list;
    }
    Ok(state)
}

async fn execute(conn: &Connection, command: NetworkCommand) -> Result<String> {
    let nm = proxy(conn, NM, NM_PATH, NM).await?;
    match command {
        NetworkCommand::SetWifi(on) => {
            nm.set_property("WirelessEnabled", on).await?;
            Ok(format!("wifi {}", if on { "on" } else { "off" }))
        }
        NetworkCommand::Scan => {
            let dev = wifi_device(conn).await?.ok_or_else(|| anyhow::anyhow!(mandara_i18n::t("No Wi-Fi device")))?;
            let wifi = proxy(conn, NM, dev.as_str(), "org.freedesktop.NetworkManager.Device.Wireless").await?;
            let opts: HashMap<&str, Value<'_>> = HashMap::new();
            let _: () = wifi.call("RequestScan", &(opts,)).await?;
            Ok("scan requested".into())
        }
        NetworkCommand::Disconnect => {
            let dev = wifi_device(conn).await?.ok_or_else(|| anyhow::anyhow!(mandara_i18n::t("No Wi-Fi device")))?;
            let d = proxy(conn, NM, dev.as_str(), "org.freedesktop.NetworkManager.Device").await?;
            let _: () = d.call("Disconnect", &()).await?;
            Ok("disconnected".into())
        }
        NetworkCommand::Connect { ssid, password } => {
            let dev = wifi_device(conn).await?.ok_or_else(|| anyhow::anyhow!(mandara_i18n::t("No Wi-Fi device")))?;
            // Reuse a saved profile for this SSID if one exists.
            let settings = proxy(conn, NM, "/org/freedesktop/NetworkManager/Settings", "org.freedesktop.NetworkManager.Settings").await?;
            let saved: Vec<OwnedObjectPath> = settings.call("ListConnections", &()).await?;
            for path in saved {
                let c = proxy(conn, NM, path.as_str(), "org.freedesktop.NetworkManager.Settings.Connection").await?;
                let cfg: HashMap<String, HashMap<String, zbus::zvariant::OwnedValue>> = c.call("GetSettings", &()).await?;
                let matches = cfg
                    .get("802-11-wireless")
                    .and_then(|w| w.get("ssid"))
                    .and_then(|v| Vec::<u8>::try_from(v.try_clone().ok()?).ok())
                    .is_some_and(|bytes| ssid_from_bytes(&bytes) == ssid);
                if matches && password.is_none() {
                    let root = OwnedObjectPath::try_from("/")?;
                    let _: OwnedObjectPath = nm.call("ActivateConnection", &(path, dev.clone(), root)).await?;
                    return Ok(format!("activating saved profile for {ssid}"));
                }
            }
            // New connection: NetworkManager fills in the rest from the AP.
            let mut wireless: HashMap<&str, Value<'_>> = HashMap::new();
            wireless.insert("ssid", Value::from(ssid.as_bytes().to_vec()));
            let mut conn_settings: HashMap<&str, HashMap<&str, Value<'_>>> = HashMap::new();
            conn_settings.insert("802-11-wireless", wireless);
            if let Some(psk) = &password {
                let mut sec: HashMap<&str, Value<'_>> = HashMap::new();
                sec.insert("key-mgmt", Value::from("wpa-psk"));
                sec.insert("psk", Value::from(psk.as_str()));
                conn_settings.insert("802-11-wireless-security", sec);
            }
            let root = OwnedObjectPath::try_from("/")?;
            let _: (OwnedObjectPath, OwnedObjectPath) = nm.call("AddAndActivateConnection", &(conn_settings, dev, root)).await?;
            Ok(format!("connecting to {ssid}"))
        }
    }
}

/// Icon tier for a strength value (`0..=4`).
pub fn strength_tier(strength: u8) -> u8 {
    STRENGTH_TIERS.iter().filter(|lower| strength >= **lower).count() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssid_bytes_and_tiers() {
        assert_eq!(ssid_from_bytes(b"Corniche-Main\0\0"), "Corniche-Main");
        assert_eq!(strength_tier(5), 0);
        assert_eq!(strength_tier(65), 3);
        assert_eq!(strength_tier(100), 4);
    }
}
