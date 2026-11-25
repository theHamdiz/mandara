//! Bluetooth pairing agent (`org.bluez.Agent1`).
//!
//! BlueZ calls the default agent whenever pairing needs the user: confirm a
//! six-digit passkey shown on both devices, type a PIN / passkey shown on
//! the other device, or show one to type there, and authorize an untrusted
//! device or service. Mandara registers one on the system bus
//! (`KeyboardDisplay` capability, made the default agent) and forwards each
//! request to the shell UI as a [`PairingEvent`]; the dialog answers with a
//! [`PairingReply`]. Requests nobody answers are rejected after
//! [`ANSWER_TIMEOUT`] (BlueZ gives up on its own a little later).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use zbus::interface;
use zbus::zvariant::OwnedObjectPath;

pub const AGENT_PATH: &str = "/org/mandara/bluez/agent";
/// What the agent can do: show codes and read typed ones.
const CAPABILITY: &str = "KeyboardDisplay";
/// A request left unanswered this long is rejected.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(60);
/// How long to wait before registering again after a failed attempt.
const REREGISTER_AFTER: Duration = Duration::from_secs(30);
/// Events buffered for the UI.
const EVENT_QUEUE: usize = 16;
/// Largest passkey BlueZ accepts (six digits).
pub const PASSKEY_MAX: u32 = 999_999;

/// What BlueZ asks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PairingKind {
    /// Does this passkey match the one on the other device?
    Confirm { passkey: u32 },
    /// Type the PIN shown on (or set for) the other device.
    EnterPin,
    /// Type the six-digit passkey shown on the other device.
    EnterPasskey,
    /// Show this PIN; type it on the other device.
    DisplayPin { pin: String },
    /// Show this passkey; type it on the other device (`entered` digits so far).
    DisplayPasskey { passkey: u32, entered: u16 },
    /// Allow this (unpaired) device to pair?
    Authorize,
    /// Allow this device to use a service?
    AuthorizeService { uuid: String },
}

impl PairingKind {
    /// Display-only requests need no answer.
    pub fn is_display(&self) -> bool {
        matches!(self, PairingKind::DisplayPin { .. } | PairingKind::DisplayPasskey { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PairingRequest {
    pub id: u64,
    pub device: String,
    pub address: String,
    /// BlueZ device icon name (`audio-headset`, `input-keyboard`).
    pub icon: String,
    pub kind: PairingKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum AgentStatus {
    Registered,
    Disabled,
    Unavailable(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairingEvent {
    Status(AgentStatus),
    Request(PairingRequest),
    /// BlueZ cancelled the request (or it timed out).
    Cancelled(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairingReply {
    /// Confirm / authorize.
    Accept(u64),
    Pin(u64, String),
    Passkey(u64, u32),
    Reject(u64),
}

impl PairingReply {
    pub fn id(&self) -> u64 {
        match self {
            PairingReply::Accept(id) | PairingReply::Pin(id, _) | PairingReply::Passkey(id, _) | PairingReply::Reject(id) => *id,
        }
    }
}

/// The UI side: events in, replies out.
#[derive(Clone)]
pub struct PairingAgent {
    pub events: async_channel::Receiver<PairingEvent>,
    shared: Arc<Shared>,
}

impl PairingAgent {
    pub fn reply(&self, reply: PairingReply) {
        self.shared.answer(reply);
    }
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Rejected(String),
    Canceled(String),
}

/// Pending requests by id and the event queue.
struct Shared {
    events: async_channel::Sender<PairingEvent>,
    pending: Mutex<HashMap<u64, async_channel::Sender<PairingReply>>>,
    /// The open display-only request of each device (its request id), so
    /// `DisplayPasskey`'s per-digit updates change one dialog in place.
    displays: Mutex<HashMap<String, u64>>,
    next: AtomicU64,
}

impl Shared {
    fn new(events: async_channel::Sender<PairingEvent>) -> Self {
        Self { events, pending: Mutex::new(HashMap::new()), displays: Mutex::new(HashMap::new()), next: AtomicU64::new(1) }
    }

    fn answer(&self, reply: PairingReply) {
        if let Some(tx) = self.pending.lock().remove(&reply.id()) {
            let _ = tx.try_send(reply);
        }
    }

    /// Shows `kind` and waits for the user's answer (or the timeout).
    async fn ask(&self, device: String, address: String, icon: String, kind: PairingKind, timeout: Duration) -> Option<PairingReply> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = async_channel::bounded(1);
        self.pending.lock().insert(id, tx);
        crate::actor::send_event(&self.events, PairingEvent::Request(PairingRequest { id, device, address, icon, kind }), "bluetooth pairing").await;
        let answer = futures::future::select(Box::pin(rx.recv()), Box::pin(async_io::Timer::after(timeout))).await;
        match answer {
            futures::future::Either::Left((Ok(reply), _)) => Some(reply),
            _ => {
                self.pending.lock().remove(&id);
                crate::actor::send_event(&self.events, PairingEvent::Cancelled(id), "bluetooth pairing").await;
                None
            }
        }
    }

    /// Display-only: show it, no answer expected. A device has one display
    /// request: showing again (`DisplayPasskey` fires per typed digit)
    /// re-sends the same request id, which the UI applies in place.
    async fn show(&self, device_path: &str, name: String, address: String, icon: String, kind: PairingKind) {
        let id = *self.displays.lock().entry(device_path.to_string()).or_insert_with(|| self.next.fetch_add(1, Ordering::Relaxed));
        crate::actor::send_event(&self.events, PairingEvent::Request(PairingRequest { id, device: name, address, icon, kind }), "bluetooth pairing").await;
    }

    /// Cancels every open request, display-only ones included (BlueZ `Cancel`).
    async fn cancel_all(&self) {
        let mut ids: Vec<u64> = self.pending.lock().drain().map(|(id, _)| id).collect();
        ids.extend(self.displays.lock().drain().map(|(_, id)| id));
        for id in ids {
            crate::actor::send_event(&self.events, PairingEvent::Cancelled(id), "bluetooth pairing").await;
        }
    }
}

/// Validates a typed passkey (six digits at most).
pub fn parse_passkey(s: &str) -> Option<u32> {
    let s = s.trim();
    if s.is_empty() || s.len() > 6 || !s.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok().filter(|v| *v <= PASSKEY_MAX)
}

/// Validates a typed PIN (1–16 printable characters, BlueZ's legacy limit).
pub fn valid_pin(s: &str) -> bool {
    const PIN_MAX: usize = 16;
    !s.is_empty() && s.len() <= PIN_MAX && s.chars().all(|c| c.is_ascii_graphic())
}

/// A passkey as six zero-padded digits (`012345`).
pub fn format_passkey(p: u32) -> String {
    format!("{p:06}")
}

struct Agent {
    shared: Arc<Shared>,
    conn: Mutex<Option<zbus::Connection>>,
}

impl Agent {
    /// Alias, address and icon of a device.
    async fn describe(&self, device: &OwnedObjectPath) -> (String, String, String) {
        let conn = self.conn.lock().clone();
        let Some(conn) = conn else { return (device.to_string(), String::new(), String::new()) };
        let Ok(p) = crate::actor::proxy(&conn, "org.bluez", device.as_str(), "org.bluez.Device1").await else {
            return (device.to_string(), String::new(), String::new());
        };
        let address: String = p.get_property("Address").await.unwrap_or_default();
        let alias: String = p.get_property("Alias").await.unwrap_or_else(|_| address.clone());
        let icon: String = p.get_property("Icon").await.unwrap_or_default();
        (alias, address, icon)
    }

    /// Refuses callers other than bluetoothd (or root).
    async fn check(&self, conn: &zbus::Connection, header: &zbus::message::Header<'_>) -> Result<(), AgentError> {
        if crate::actor::caller_is(conn, header, "org.bluez").await { Ok(()) } else { Err(AgentError::Rejected("only bluetoothd may call the agent".into())) }
    }

    /// Like [`Agent::ask`] for yes/no questions: only an explicit accept counts.
    async fn ask_yes(&self, device: &OwnedObjectPath, kind: PairingKind) -> Result<(), AgentError> {
        match self.ask(device, kind).await? {
            PairingReply::Accept(_) => Ok(()),
            _ => Err(AgentError::Rejected("rejected by the user".into())),
        }
    }

    async fn ask(&self, device: &OwnedObjectPath, kind: PairingKind) -> Result<PairingReply, AgentError> {
        let (name, address, icon) = self.describe(device).await;
        match self.shared.ask(name, address, icon, kind, ANSWER_TIMEOUT).await {
            Some(PairingReply::Reject(_)) => Err(AgentError::Rejected("rejected by the user".into())),
            Some(reply) => Ok(reply),
            None => Err(AgentError::Canceled("no answer".into())),
        }
    }
}

#[interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self) {
        tracing::info!("bluetooth: pairing agent released by BlueZ");
    }

    async fn request_pin_code(&self, #[zbus(connection)] conn: &zbus::Connection, #[zbus(header)] header: zbus::message::Header<'_>, device: OwnedObjectPath) -> Result<String, AgentError> {
        self.check(conn, &header).await?;
        match self.ask(&device, PairingKind::EnterPin).await? {
            PairingReply::Pin(_, pin) if valid_pin(&pin) => Ok(pin),
            _ => Err(AgentError::Rejected("no valid PIN".into())),
        }
    }

    async fn display_pin_code(&self, #[zbus(connection)] conn: &zbus::Connection, #[zbus(header)] header: zbus::message::Header<'_>, device: OwnedObjectPath, pincode: String) -> Result<(), AgentError> {
        self.check(conn, &header).await?;
        let (name, address, icon) = self.describe(&device).await;
        self.shared.show(device.as_str(), name, address, icon, PairingKind::DisplayPin { pin: pincode }).await;
        Ok(())
    }

    async fn request_passkey(&self, #[zbus(connection)] conn: &zbus::Connection, #[zbus(header)] header: zbus::message::Header<'_>, device: OwnedObjectPath) -> Result<u32, AgentError> {
        self.check(conn, &header).await?;
        match self.ask(&device, PairingKind::EnterPasskey).await? {
            PairingReply::Passkey(_, p) if p <= PASSKEY_MAX => Ok(p),
            _ => Err(AgentError::Rejected("no valid passkey".into())),
        }
    }

    async fn display_passkey(&self, #[zbus(connection)] conn: &zbus::Connection, #[zbus(header)] header: zbus::message::Header<'_>, device: OwnedObjectPath, passkey: u32, entered: u16) -> Result<(), AgentError> {
        self.check(conn, &header).await?;
        let (name, address, icon) = self.describe(&device).await;
        self.shared.show(device.as_str(), name, address, icon, PairingKind::DisplayPasskey { passkey, entered }).await;
        Ok(())
    }

    async fn request_confirmation(&self, #[zbus(connection)] conn: &zbus::Connection, #[zbus(header)] header: zbus::message::Header<'_>, device: OwnedObjectPath, passkey: u32) -> Result<(), AgentError> {
        self.check(conn, &header).await?;
        self.ask_yes(&device, PairingKind::Confirm { passkey }).await
    }

    async fn request_authorization(&self, #[zbus(connection)] conn: &zbus::Connection, #[zbus(header)] header: zbus::message::Header<'_>, device: OwnedObjectPath) -> Result<(), AgentError> {
        self.check(conn, &header).await?;
        self.ask_yes(&device, PairingKind::Authorize).await
    }

    async fn authorize_service(&self, #[zbus(connection)] conn: &zbus::Connection, #[zbus(header)] header: zbus::message::Header<'_>, device: OwnedObjectPath, uuid: String) -> Result<(), AgentError> {
        self.check(conn, &header).await?;
        self.ask_yes(&device, PairingKind::AuthorizeService { uuid }).await
    }

    async fn cancel(&self, #[zbus(connection)] conn: &zbus::Connection, #[zbus(header)] header: zbus::message::Header<'_>) -> Result<(), AgentError> {
        self.check(conn, &header).await?;
        self.shared.cancel_all().await;
        Ok(())
    }
}

/// Serves the agent on the service runtime for the life of the shell and
/// (re-)registers it with BlueZ whenever bluetoothd appears on the bus, so a
/// bluetoothd that starts late or restarts does not leave pairing dead.
pub fn spawn_pairing_agent(enabled: bool) -> PairingAgent {
    let (tx, events) = async_channel::bounded(EVENT_QUEUE);
    let shared = Arc::new(Shared::new(tx.clone()));
    let agent = PairingAgent { events, shared: shared.clone() };
    if !enabled {
        let _ = tx.try_send(PairingEvent::Status(AgentStatus::Disabled));
        return agent;
    }
    crate::runtime::spawn("bluetooth-agent", async move {
        let conn = match serve(shared).await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::info!("bluetooth: pairing agent not registered ({e})");
                crate::actor::send_event(&tx, PairingEvent::Status(AgentStatus::Unavailable(e)), "bluetooth pairing").await;
                return;
            }
        };
        let mut owners = crate::actor::owner_changes(&conn, "org.bluez").await.ok();
        let mut last: Option<AgentStatus> = None;
        loop {
            let status = match register(&conn).await {
                Ok(()) => {
                    tracing::info!("bluetooth: pairing agent registered");
                    AgentStatus::Registered
                }
                Err(e) => {
                    tracing::info!("bluetooth: pairing agent not registered ({e})");
                    AgentStatus::Unavailable(e)
                }
            };
            let registered = status == AgentStatus::Registered;
            if last.as_ref() != Some(&status) {
                last = Some(status.clone());
                crate::actor::send_event(&tx, PairingEvent::Status(status), "bluetooth pairing").await;
            }
            // Registered: wait for bluetoothd to come back (it forgets
            // agents when it restarts). Not registered: also retry on a timer.
            let retry = (!registered).then_some(REREGISTER_AFTER);
            match owners.as_mut() {
                Some(owners) => {
                    if !crate::actor::wait_for_owner(owners, retry).await {
                        return;
                    }
                }
                None => crate::runtime::sleep(REREGISTER_AFTER).await,
            }
        }
    });
    agent
}

/// The bus connection that serves the agent object.
async fn serve(shared: Arc<Shared>) -> Result<zbus::Connection, String> {
    let agent = Agent { shared, conn: Mutex::new(None) };
    let builder = zbus::connection::Builder::system().and_then(|b| b.serve_at(AGENT_PATH, agent)).map_err(|e| e.to_string())?;
    let conn = crate::runtime::connect(builder).await.map_err(|e| e.to_string())?;
    // The agent looks devices up over its own connection.
    if let Ok(iface) = conn.object_server().interface::<_, Agent>(AGENT_PATH).await {
        *iface.get().await.conn.lock() = Some(conn.clone());
    }
    Ok(conn)
}

/// Registers the served agent with BlueZ and asks to make it the default.
async fn register(conn: &zbus::Connection) -> Result<(), String> {
    let manager = crate::actor::proxy(conn, "org.bluez", "/org/bluez", "org.bluez.AgentManager1").await.map_err(|e| e.to_string())?;
    let path = OwnedObjectPath::try_from(AGENT_PATH).map_err(|e| e.to_string())?;
    manager.call::<_, _, ()>("RegisterAgent", &(path.clone(), CAPABILITY)).await.map_err(|e| e.to_string())?;
    if let Err(e) = manager.call::<_, _, ()>("RequestDefaultAgent", &(path,)).await {
        tracing::info!("bluetooth: agent registered but not default ({e})");
    }
    Ok(())
}

/// A UI-only agent for `mandara msg bluetooth pairing-demo`: one fake
/// request per kind, answers logged.
pub fn demo_agent(kind: PairingKind) -> PairingAgent {
    let (tx, events) = async_channel::bounded(EVENT_QUEUE);
    let shared = Arc::new(Shared::new(tx));
    crate::runtime::block_on(shared.show("/demo", "Demo headphones".into(), "00:11:22:33:44:55".into(), "audio-headset".into(), kind));
    PairingAgent { events, shared }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passkeys_and_pins() {
        assert_eq!(parse_passkey("012345"), Some(12345));
        assert_eq!(parse_passkey(" 999999 "), Some(999_999));
        assert_eq!(parse_passkey("1234567"), None);
        assert_eq!(parse_passkey("12a4"), None);
        assert_eq!(parse_passkey(""), None);
        assert!(valid_pin("0000") && !valid_pin("") && !valid_pin("has space"));
        assert_eq!(format_passkey(42), "000042");
        assert!(PairingKind::DisplayPin { pin: "1".into() }.is_display() && !PairingKind::Authorize.is_display());
    }

    #[test]
    fn requests_route_answers_by_id() {
        let (tx, events) = async_channel::bounded(EVENT_QUEUE);
        let shared = Arc::new(Shared::new(tx));
        let s2 = shared.clone();
        let ask = std::thread::spawn(move || crate::runtime::block_on(s2.ask("Buds".into(), "AA".into(), String::new(), PairingKind::Confirm { passkey: 123456 }, ANSWER_TIMEOUT)));
        let PairingEvent::Request(req) = crate::runtime::block_on(events.recv()).unwrap() else { panic!("expected a request") };
        assert_eq!(req.kind, PairingKind::Confirm { passkey: 123456 });
        // A reply for another id is ignored.
        shared.answer(PairingReply::Reject(req.id + 100));
        shared.answer(PairingReply::Accept(req.id));
        assert_eq!(ask.join().unwrap(), Some(PairingReply::Accept(req.id)));
        assert!(shared.pending.lock().is_empty());
    }

    #[test]
    fn unanswered_requests_time_out_and_cancel() {
        let (tx, events) = async_channel::bounded(EVENT_QUEUE);
        let shared = Shared::new(tx);
        let r = crate::runtime::block_on(shared.ask("Kbd".into(), "BB".into(), String::new(), PairingKind::EnterPasskey, Duration::from_millis(20)));
        assert_eq!(r, None);
        let _request = events.try_recv().unwrap();
        assert!(matches!(events.try_recv().unwrap(), PairingEvent::Cancelled(_)));
        // BlueZ Cancel ends everything pending.
        shared.pending.lock().insert(7, async_channel::bounded(1).0);
        crate::runtime::block_on(shared.cancel_all());
        assert_eq!(events.try_recv().unwrap(), PairingEvent::Cancelled(7));
    }

    /// `DisplayPasskey` fires per typed digit: one request id per device,
    /// updated in place, and BlueZ `Cancel` clears it.
    #[test]
    fn display_requests_update_in_place_and_cancel() {
        let (tx, events) = async_channel::bounded(EVENT_QUEUE);
        let shared = Shared::new(tx);
        let show = |path: &str, entered| crate::runtime::block_on(shared.show(path, "Kbd".into(), "AA".into(), String::new(), PairingKind::DisplayPasskey { passkey: 123456, entered }));
        show("/dev_a", 0);
        show("/dev_a", 1);
        show("/dev_b", 0);
        let id = |e: PairingEvent| match e {
            PairingEvent::Request(r) => r.id,
            other => panic!("expected a request, got {other:?}"),
        };
        let (first, second, other) = (id(events.try_recv().unwrap()), id(events.try_recv().unwrap()), id(events.try_recv().unwrap()));
        assert_eq!(first, second, "the same device keeps one request id");
        assert_ne!(first, other);
        crate::runtime::block_on(shared.cancel_all());
        let mut cancelled: Vec<u64> = (0..2)
            .map(|_| match events.try_recv().unwrap() {
                PairingEvent::Cancelled(id) => id,
                other => panic!("expected a cancel, got {other:?}"),
            })
            .collect();
        cancelled.sort();
        assert_eq!(cancelled, vec![first.min(other), first.max(other)]);
        assert!(shared.displays.lock().is_empty());
    }
}
