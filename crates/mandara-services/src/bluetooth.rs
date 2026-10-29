//! BlueZ actor (system bus): adapters (power, discovery, which one to use),
//! devices with their connection / pairing / trust state and battery level,
//! and device management (pair, trust, rename, forget). Event-driven:
//! BlueZ's signals (`PropertiesChanged`, `InterfacesAdded/Removed`) trigger
//! a debounced re-read; a slow fallback poll covers anything a signal missed.
//!
//! With auto-connect on, trusted paired devices are reconnected when the
//! adapter powers on (including at startup and after resume, when the
//! adapter comes back). Pairing prompts are answered by
//! [`crate::bt_agent`].

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Result, anyhow};
use serde::Serialize;
use zbus::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use crate::actor::{Envelope, ServiceHandle, Wake, drain_signals, next_wake, proxy, reply, signals_from};
use crate::runtime;

const BLUEZ: &str = "org.bluez";
/// Signals coalesced into one re-read (discovery fires RSSI updates).
const DEBOUNCE: Duration = Duration::from_millis(400);
/// Re-read even without signals this often.
const FALLBACK_POLL: Duration = Duration::from_secs(60);
/// State updates buffered for the UI.
const UPDATE_QUEUE: usize = 8;
/// Commands buffered from the UI and CLI.
const COMMAND_QUEUE: usize = 32;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct BluetoothDevice {
    pub name: String,
    pub address: String,
    pub icon: String,
    pub connected: bool,
    pub paired: bool,
    /// Trusted devices may connect without confirmation (and are the ones
    /// auto-connect reconnects).
    pub trusted: bool,
    pub battery: Option<u8>,
    /// The adapter it was seen on (`hci0`).
    pub adapter: String,
    #[serde(skip)]
    path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct BluetoothAdapter {
    /// `hci0`.
    pub name: String,
    pub address: String,
    pub alias: String,
    pub powered: bool,
    /// The adapter Mandara uses.
    pub selected: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct BluetoothState {
    pub available: bool,
    pub powered: bool,
    pub discovering: bool,
    /// Connected first, then paired, then by name (selected adapter only).
    pub devices: Vec<BluetoothDevice>,
    pub adapters: Vec<BluetoothAdapter>,
}

impl BluetoothState {
    pub fn connected_count(&self) -> usize {
        self.devices.iter().filter(|d| d.connected).count()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum BluetoothCommand {
    SetPower(bool),
    SetDiscovery(bool),
    /// Device matched by address or (case-insensitive) name.
    Connect(String),
    Disconnect(String),
    /// Pairs (and trusts) a discovered device; the agent shows any prompt.
    Pair(String),
    SetTrusted(String, bool),
    /// Sets the device's alias (the name shown everywhere).
    Rename(String, String),
    /// Removes the pairing (`Adapter1.RemoveDevice`).
    Forget(String),
    /// Which adapter to use (`hci1`); `None` = the first.
    SelectAdapter(Option<String>),
    /// Reconnect trusted devices whenever the adapter powers on.
    SetAutoConnect(bool),
    /// Connects every trusted, paired device that isn't connected.
    ConnectTrusted,
}

/// Actor-side preferences set by commands.
#[derive(Clone, Debug, Default)]
struct Prefs {
    adapter: Option<String>,
    auto_connect: bool,
}

pub type BluetoothService = ServiceHandle<BluetoothState, BluetoothCommand>;

type Managed = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

pub fn spawn_bluetooth_service() -> BluetoothService {
    let (tx, updates) = async_channel::bounded(UPDATE_QUEUE);
    let (commands, rx) = async_channel::bounded(COMMAND_QUEUE);
    runtime::spawn("bluetooth", async move {
        // Private connection: its signal stream must never stall other
        // actors' replies on a shared one.
        let conn = match zbus::connection::Builder::system() {
            Ok(b) => runtime::connect(b).await,
            Err(e) => Err(e),
        };
        let Ok(conn) = conn else {
            let _ = tx.send(BluetoothState::default()).await;
            return;
        };
        if let Err(err) = run(&conn, &tx, &rx).await {
            tracing::warn!("bluetooth: {err:#}");
        }
    });
    ServiceHandle::new(updates, commands)
}

async fn run(conn: &Connection, tx: &async_channel::Sender<BluetoothState>, rx: &async_channel::Receiver<Envelope<BluetoothCommand>>) -> Result<()> {
    let mut signals = signals_from(conn, BLUEZ).await?;
    let mut last: Option<BluetoothState> = None;
    let mut prefs = Prefs::default();
    loop {
        drain_signals(&mut signals);
        let state = read_state(conn, prefs.adapter.as_deref()).await.unwrap_or_default();
        // Powered on (or first seen powered): reconnect trusted devices.
        let powered_on = state.powered && !last.as_ref().is_some_and(|l| l.powered);
        if last.as_ref() != Some(&state) {
            if tx.send(state.clone()).await.is_err() {
                return Ok(());
            }
            last = Some(state.clone());
        }
        if powered_on && prefs.auto_connect {
            let n = connect_trusted(conn, &state).await;
            if n > 0 {
                tracing::info!("bluetooth: auto-connecting {n} trusted device(s)");
            }
        }
        match next_wake(&mut signals, rx, DEBOUNCE, FALLBACK_POLL).await {
            Wake::Command(Envelope { command, reply: r }) => {
                let current = last.clone().unwrap_or_default();
                let result = match command {
                    BluetoothCommand::SelectAdapter(name) => {
                        prefs.adapter = name.filter(|n| !n.trim().is_empty());
                        Ok(format!("adapter {}", prefs.adapter.as_deref().unwrap_or("auto")))
                    }
                    BluetoothCommand::SetAutoConnect(on) => {
                        let newly = on && !prefs.auto_connect;
                        prefs.auto_connect = on;
                        if newly && current.powered {
                            connect_trusted(conn, &current).await;
                        }
                        Ok(format!("auto-connect {}", if on { "on" } else { "off" }))
                    }
                    command => execute(conn, &current, prefs.adapter.as_deref(), command).await.map_err(|e| format!("{e:#}")),
                };
                reply(&r, result);
            }
            Wake::Refresh => {}
            Wake::Closed => return Ok(()),
        }
    }
}

fn prop<T: TryFrom<OwnedValue>>(props: &HashMap<String, OwnedValue>, key: &str) -> Option<T> {
    props.get(key).and_then(|v| T::try_from(v.try_clone().ok()?).ok())
}

async fn managed(conn: &Connection) -> Result<Managed> {
    let om = proxy(conn, BLUEZ, "/", "org.freedesktop.DBus.ObjectManager").await?;
    Ok(om.call("GetManagedObjects", &()).await?)
}

/// `hci0` from `/org/bluez/hci0[/dev_…]`.
fn adapter_of(path: &str) -> String {
    path.trim_start_matches("/org/bluez/").split('/').next().unwrap_or_default().to_string()
}

/// The adapter to use: the preferred one when present, else the first.
fn pick_adapter(adapters: &[String], preferred: Option<&str>) -> Option<String> {
    preferred.and_then(|p| adapters.iter().find(|a| a.as_str() == p).cloned()).or_else(|| {
        let mut sorted = adapters.to_vec();
        sorted.sort();
        sorted.into_iter().next()
    })
}

async fn read_state(conn: &Connection, preferred: Option<&str>) -> Result<BluetoothState> {
    let objects = managed(conn).await?;
    let mut state = BluetoothState::default();
    let names: Vec<String> = objects.iter().filter(|(_, i)| i.contains_key("org.bluez.Adapter1")).map(|(p, _)| adapter_of(p.as_str())).collect();
    let selected = pick_adapter(&names, preferred);
    for (path, ifaces) in &objects {
        if let Some(adapter) = ifaces.get("org.bluez.Adapter1") {
            let name = adapter_of(path.as_str());
            let is_selected = selected.as_deref() == Some(name.as_str());
            let powered = prop::<bool>(adapter, "Powered").unwrap_or(false);
            state.available = true;
            if is_selected {
                state.powered = powered;
                state.discovering = prop::<bool>(adapter, "Discovering").unwrap_or(false);
            }
            state.adapters.push(BluetoothAdapter {
                name,
                address: prop::<String>(adapter, "Address").unwrap_or_default(),
                alias: prop::<String>(adapter, "Alias").unwrap_or_default(),
                powered,
                selected: is_selected,
            });
        }
        if let Some(dev) = ifaces.get("org.bluez.Device1") {
            if selected.as_deref() != Some(adapter_of(path.as_str()).as_str()) {
                continue;
            }
            let name = prop::<String>(dev, "Alias").or_else(|| prop(dev, "Name")).unwrap_or_default();
            let address = prop::<String>(dev, "Address").unwrap_or_default();
            let paired = prop::<bool>(dev, "Paired").unwrap_or(false);
            let connected = prop::<bool>(dev, "Connected").unwrap_or(false);
            // Unnamed, unpaired devices are noise during discovery.
            if name.is_empty() || (!paired && !connected && name.replace('-', ":") == address) {
                continue;
            }
            state.devices.push(BluetoothDevice {
                name,
                address,
                icon: prop::<String>(dev, "Icon").unwrap_or_default(),
                connected,
                paired,
                trusted: prop::<bool>(dev, "Trusted").unwrap_or(false),
                battery: ifaces.get("org.bluez.Battery1").and_then(|b| prop::<u8>(b, "Percentage")),
                adapter: adapter_of(path.as_str()),
                path: path.to_string(),
            });
        }
    }
    state.devices.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.paired.cmp(&a.paired)).then(a.name.cmp(&b.name)));
    state.adapters.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(state)
}

async fn adapter_path(conn: &Connection, preferred: Option<&str>) -> Result<OwnedObjectPath> {
    let objects = managed(conn).await?;
    let names: Vec<String> = objects.iter().filter(|(_, i)| i.contains_key("org.bluez.Adapter1")).map(|(p, _)| adapter_of(p.as_str())).collect();
    let name = pick_adapter(&names, preferred).ok_or_else(|| anyhow!(mandara_i18n::t("No Bluetooth adapter")))?;
    Ok(OwnedObjectPath::try_from(format!("/org/bluez/{name}"))?)
}

/// Trusted, paired devices that aren't connected.
fn reconnect_candidates(state: &BluetoothState) -> Vec<&BluetoothDevice> {
    state.devices.iter().filter(|d| d.trusted && d.paired && !d.connected).collect()
}

/// Starts connecting every reconnect candidate; returns how many.
async fn connect_trusted(conn: &Connection, state: &BluetoothState) -> usize {
    let Ok((ops, _)) = plan(state, "", &BluetoothCommand::ConnectTrusted) else { return 0 };
    let mut n = 0;
    for op in &ops {
        // Fire and forget: a device out of range must not stall the actor.
        if send(conn, op, true).await.is_ok() {
            n += 1;
        }
    }
    n
}

fn find_device<'a>(state: &'a BluetoothState, query: &str) -> Result<&'a BluetoothDevice> {
    state
        .devices
        .iter()
        .find(|d| d.address.eq_ignore_ascii_case(query) || d.name.eq_ignore_ascii_case(query))
        .ok_or_else(|| anyhow!(mandara_i18n::tf("No device matching \"{query}\"", &[("query", &query)])))
}

/// One BlueZ mutation: what [`plan`] decides and [`execute`] sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlueOp {
    /// `org.freedesktop.DBus.Properties.Set` of a boolean or string.
    SetBool { path: String, interface: &'static str, property: &'static str, value: bool },
    SetString { path: String, interface: &'static str, property: &'static str, value: String },
    /// A method call without arguments (`Connect`, `StartDiscovery`).
    Call { path: String, interface: &'static str, method: &'static str },
    /// `Adapter1.RemoveDevice(device)`.
    RemoveDevice { adapter: String, device: String },
}

const ADAPTER_IFACE: &str = "org.bluez.Adapter1";
const DEVICE_IFACE: &str = "org.bluez.Device1";

/// The BlueZ calls a command makes, in order, and the summary it returns.
/// Pure: `adapter_path` is the selected adapter's object path (only the
/// adapter commands use it). Actor-only commands plan nothing.
pub fn plan(state: &BluetoothState, adapter_path: &str, command: &BluetoothCommand) -> Result<(Vec<BlueOp>, String)> {
    let device = |query: &str| find_device(state, query);
    let call = |path: &str, method: &'static str| BlueOp::Call { path: path.to_string(), interface: DEVICE_IFACE, method };
    Ok(match command {
        BluetoothCommand::SetPower(on) => (vec![BlueOp::SetBool { path: adapter_path.into(), interface: ADAPTER_IFACE, property: "Powered", value: *on }], format!("bluetooth {}", if *on { "on" } else { "off" })),
        BluetoothCommand::SetDiscovery(on) => (
            vec![BlueOp::Call { path: adapter_path.into(), interface: ADAPTER_IFACE, method: if *on { "StartDiscovery" } else { "StopDiscovery" } }],
            format!("discovery {}", if *on { "started" } else { "stopped" }),
        ),
        BluetoothCommand::Connect(query) => {
            let dev = device(query)?;
            (vec![call(&dev.path, "Connect")], format!("connected {}", dev.name))
        }
        BluetoothCommand::Disconnect(query) => {
            let dev = device(query)?;
            (vec![call(&dev.path, "Disconnect")], format!("disconnected {}", dev.name))
        }
        BluetoothCommand::Pair(query) => {
            let dev = device(query)?;
            // Prompts (PIN, passkey) reach the registered agent; a paired
            // device is only trusted.
            let mut ops = if dev.paired { Vec::new() } else { vec![call(&dev.path, "Pair")] };
            ops.push(BlueOp::SetBool { path: dev.path.clone(), interface: DEVICE_IFACE, property: "Trusted", value: true });
            (ops, format!("paired {}", dev.name))
        }
        BluetoothCommand::SetTrusted(query, on) => {
            let dev = device(query)?;
            (vec![BlueOp::SetBool { path: dev.path.clone(), interface: DEVICE_IFACE, property: "Trusted", value: *on }], format!("{} {}", if *on { "trusted" } else { "untrusted" }, dev.name))
        }
        BluetoothCommand::Rename(query, name) => {
            let dev = device(query)?;
            let name = name.trim().to_string();
            if name.is_empty() {
                anyhow::bail!(mandara_i18n::t("The name is empty"));
            }
            (vec![BlueOp::SetString { path: dev.path.clone(), interface: DEVICE_IFACE, property: "Alias", value: name.clone() }], format!("renamed {} to {name}", dev.name))
        }
        BluetoothCommand::Forget(query) => {
            let dev = device(query)?;
            (vec![BlueOp::RemoveDevice { adapter: format!("/org/bluez/{}", dev.adapter), device: dev.path.clone() }], format!("forgot {}", dev.name))
        }
        BluetoothCommand::ConnectTrusted => {
            let ops: Vec<BlueOp> = reconnect_candidates(state).iter().map(|d| call(&d.path, "Connect")).collect();
            let n = ops.len();
            (ops, format!("connecting {n} trusted device(s)"))
        }
        // Handled by the actor loop.
        BluetoothCommand::SelectAdapter(_) | BluetoothCommand::SetAutoConnect(_) => (Vec::new(), String::new()),
    })
}

/// Sends one planned call.
async fn send(conn: &Connection, op: &BlueOp, no_reply: bool) -> Result<()> {
    match op {
        BlueOp::SetBool { path, interface, property, value } => proxy(conn, BLUEZ, path, interface).await?.set_property(property, *value).await?,
        BlueOp::SetString { path, interface, property, value } => proxy(conn, BLUEZ, path, interface).await?.set_property(property, value.as_str()).await?,
        BlueOp::Call { path, interface, method } if no_reply => proxy(conn, BLUEZ, path, interface).await?.call_noreply(*method, &()).await?,
        BlueOp::Call { path, interface, method } => {
            let _: () = proxy(conn, BLUEZ, path, interface).await?.call(*method, &()).await?;
        }
        BlueOp::RemoveDevice { adapter, device } => {
            let target = OwnedObjectPath::try_from(device.as_str())?;
            let _: () = proxy(conn, BLUEZ, adapter, ADAPTER_IFACE).await?.call("RemoveDevice", &(target,)).await?;
        }
    }
    Ok(())
}

async fn execute(conn: &Connection, state: &BluetoothState, adapter: Option<&str>, command: BluetoothCommand) -> Result<String> {
    let adapter_path = match command {
        BluetoothCommand::SetPower(_) | BluetoothCommand::SetDiscovery(_) => adapter_path(conn, adapter).await?.to_string(),
        _ => String::new(),
    };
    if matches!(command, BluetoothCommand::ConnectTrusted) {
        // Best effort: one device out of range doesn't stop the others.
        let n = connect_trusted(conn, state).await;
        return Ok(format!("connecting {n} trusted device(s)"));
    }
    let (ops, summary) = plan(state, &adapter_path, &command)?;
    for op in &ops {
        send(conn, op, false).await?;
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_devices_by_name_or_address() {
        let state = BluetoothState {
            available: true,
            powered: true,
            discovering: false,
            devices: vec![
                BluetoothDevice { name: "WH-1000XM5".into(), address: "AA:BB".into(), connected: true, trusted: true, paired: true, ..Default::default() },
                BluetoothDevice { name: "MX Keys".into(), address: "CC:DD".into(), trusted: true, paired: true, ..Default::default() },
                BluetoothDevice { name: "Speaker".into(), address: "EE:FF".into(), paired: true, ..Default::default() },
            ],
            adapters: Vec::new(),
        };
        assert!(find_device(&state, "wh-1000xm5").is_ok());
        assert!(find_device(&state, "aa:bb").is_ok());
        assert!(find_device(&state, "nope").is_err());
        assert_eq!(state.connected_count(), 1);
        let names: Vec<&str> = reconnect_candidates(&state).iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["MX Keys"], "only trusted, paired, disconnected devices");
    }

    fn mock_state() -> BluetoothState {
        let dev = |name: &str, addr: &str, connected, trusted, paired| BluetoothDevice { name: name.into(), address: addr.into(), connected, trusted, paired, adapter: "hci0".into(), path: format!("/org/bluez/hci0/dev_{}", addr.replace(':', "_")), ..Default::default() };
        BluetoothState { available: true, powered: true, discovering: false, devices: vec![dev("Buds", "AA:BB", true, true, true), dev("MX Keys", "CC:DD", false, true, true), dev("Speaker", "EE:FF", false, false, false)], adapters: Vec::new() }
    }

    /// Every Studio / CLI Bluetooth action as the exact BlueZ calls it
    /// makes, without a bus.
    #[test]
    fn commands_plan_the_right_bluez_calls() {
        use BluetoothCommand as C;
        let st = mock_state();
        let a = "/org/bluez/hci0";
        let ops = |c: C| plan(&st, a, &c).unwrap().0;
        assert_eq!(ops(C::SetPower(false)), [BlueOp::SetBool { path: a.into(), interface: ADAPTER_IFACE, property: "Powered", value: false }]);
        assert_eq!(ops(C::SetDiscovery(true)), [BlueOp::Call { path: a.into(), interface: ADAPTER_IFACE, method: "StartDiscovery" }]);
        assert_eq!(ops(C::Connect("mx keys".into())), [BlueOp::Call { path: "/org/bluez/hci0/dev_CC_DD".into(), interface: DEVICE_IFACE, method: "Connect" }]);
        assert_eq!(ops(C::Disconnect("AA:BB".into())), [BlueOp::Call { path: "/org/bluez/hci0/dev_AA_BB".into(), interface: DEVICE_IFACE, method: "Disconnect" }]);
        // Pairing a new device pairs then trusts; a paired one is only trusted.
        let speaker = "/org/bluez/hci0/dev_EE_FF".to_string();
        assert_eq!(ops(C::Pair("Speaker".into())), [BlueOp::Call { path: speaker.clone(), interface: DEVICE_IFACE, method: "Pair" }, BlueOp::SetBool { path: speaker.clone(), interface: DEVICE_IFACE, property: "Trusted", value: true }]);
        assert_eq!(ops(C::Pair("Buds".into())).len(), 1);
        assert_eq!(ops(C::SetTrusted("Speaker".into(), true)), [BlueOp::SetBool { path: speaker.clone(), interface: DEVICE_IFACE, property: "Trusted", value: true }]);
        assert_eq!(ops(C::Rename("Speaker".into(), "  Kitchen  ".into())), [BlueOp::SetString { path: speaker.clone(), interface: DEVICE_IFACE, property: "Alias", value: "Kitchen".into() }]);
        assert!(plan(&st, a, &C::Rename("Speaker".into(), "  ".into())).is_err(), "an empty name is refused");
        assert_eq!(ops(C::Forget("EE:FF".into())), [BlueOp::RemoveDevice { adapter: a.into(), device: speaker }]);
        // Auto-connect / after sleep: trusted, paired, disconnected only.
        assert_eq!(ops(C::ConnectTrusted), [BlueOp::Call { path: "/org/bluez/hci0/dev_CC_DD".into(), interface: DEVICE_IFACE, method: "Connect" }]);
        assert!(ops(C::SetAutoConnect(true)).is_empty() && ops(C::SelectAdapter(None)).is_empty(), "actor-only commands touch nothing");
        assert!(plan(&st, a, &C::Connect("nope".into())).is_err());
    }

    #[test]
    fn adapters_are_picked_by_preference() {
        assert_eq!(adapter_of("/org/bluez/hci1/dev_AA_BB"), "hci1");
        assert_eq!(adapter_of("/org/bluez/hci0"), "hci0");
        let list = vec!["hci1".to_string(), "hci0".to_string()];
        assert_eq!(pick_adapter(&list, None).as_deref(), Some("hci0"));
        assert_eq!(pick_adapter(&list, Some("hci1")).as_deref(), Some("hci1"));
        assert_eq!(pick_adapter(&list, Some("hci9")).as_deref(), Some("hci0"), "a missing adapter falls back");
        assert_eq!(pick_adapter(&[], None), None);
    }
}
