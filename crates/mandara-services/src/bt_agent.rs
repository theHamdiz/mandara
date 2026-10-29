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
    next: AtomicU64,
}

impl Shared {
    fn new(events: async_channel::Sender<PairingEvent>) -> Self {
        Self { events, pending: Mutex::new(HashMap::new()), next: AtomicU64::new(1) }
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
        let _ = self.events.try_send(PairingEvent::Request(PairingRequest { id, device, address, icon, kind }));
        let answer = futures::future::select(Box::pin(rx.recv()), Box::pin(async_io::Timer::after(timeout))).await;
        match answer {
            futures::future::Either::Left((Ok(reply), _)) => Some(reply),
            _ => {
                self.pending.lock().remove(&id);
                let _ = self.events.try_send(PairingEvent::Cancelled(id));
                None
            }
        }
    }

    /// Display-only: show it, no answer expected.
    fn show(&self, device: String, address: String, icon: String, kind: PairingKind) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let _ = self.events.try_send(PairingEvent::Request(PairingRequest { id, device, address, icon, kind }));
    }

    /// Cancels every open request (BlueZ `Cancel`).
    fn cancel_all(&self) {
        let ids: Vec<u64> = self.pending.lock().drain().map(|(id, _)| id).collect();
        for id in ids {
            let _ = self.events.try_send(PairingEvent::Cancelled(id));
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

    async fn request_pin_code(&self, device: OwnedObjectPath) -> Result<String, AgentError> {
        match self.ask(&device, PairingKind::EnterPin).await? {
            PairingReply::Pin(_, pin) if valid_pin(&pin) => Ok(pin),
            _ => Err(AgentError::Rejected("no valid PIN".into())),
        }
    }

    async fn display_pin_code(&self, device: OwnedObjectPath, pincode: String) -> Result<(), AgentError> {
        let (name, address, icon) = self.describe(&device).await;
        self.shared.show(name, address, icon, PairingKind::DisplayPin { pin: pincode });
        Ok(())
    }

    async fn request_passkey(&self, device: OwnedObjectPath) -> Result<u32, AgentError> {
        match self.ask(&device, PairingKind::EnterPasskey).await? {
            PairingReply::Passkey(_, p) if p <= PASSKEY_MAX => Ok(p),
            _ => Err(AgentError::Rejected("no valid passkey".into())),
        }
    }

    async fn display_passkey(&self, device: OwnedObjectPath, passkey: u32, entered: u16) {
        let (name, address, icon) = self.describe(&device).await;
        self.shared.show(name, address, icon, PairingKind::DisplayPasskey { passkey, entered });
    }

    async fn request_confirmation(&self, device: OwnedObjectPath, passkey: u32) -> Result<(), AgentError> {
        self.ask(&device, PairingKind::Confirm { passkey }).await.map(|_| ())
    }

    async fn request_authorization(&self, device: OwnedObjectPath) -> Result<(), AgentError> {
        self.ask(&device, PairingKind::Authorize).await.map(|_| ())
    }

    async fn authorize_service(&self, device: OwnedObjectPath, uuid: String) -> Result<(), AgentError> {
        self.ask(&device, PairingKind::AuthorizeService { uuid }).await.map(|_| ())
    }

    fn cancel(&self) {
        self.shared.cancel_all();
    }
}

/// Registers the agent (unless `enabled` is false) and serves it on the
/// service runtime for the life of the shell.
pub fn spawn_pairing_agent(enabled: bool) -> PairingAgent {
    let (tx, events) = async_channel::bounded(EVENT_QUEUE);
    let shared = Arc::new(Shared::new(tx.clone()));
    let agent = PairingAgent { events, shared: shared.clone() };
    if !enabled {
        let _ = tx.try_send(PairingEvent::Status(AgentStatus::Disabled));
        return agent;
    }
    crate::runtime::spawn("bluetooth-agent", async move {
        match register(shared).await {
            Ok(conn) => {
                tracing::info!("bluetooth: pairing agent registered");
                let _ = tx.try_send(PairingEvent::Status(AgentStatus::Registered));
                let _conn = conn;
                std::future::pending::<()>().await;
            }
            Err(e) => {
                tracing::info!("bluetooth: pairing agent not registered ({e})");
                let _ = tx.try_send(PairingEvent::Status(AgentStatus::Unavailable(e)));
            }
        }
    });
    agent
}

async fn register(shared: Arc<Shared>) -> Result<zbus::Connection, String> {
    let agent = Agent { shared, conn: Mutex::new(None) };
    let builder = zbus::connection::Builder::system().and_then(|b| b.serve_at(AGENT_PATH, agent)).map_err(|e| e.to_string())?;
    let conn = crate::runtime::connect(builder).await.map_err(|e| e.to_string())?;
    // The agent looks devices up over its own connection.
    if let Ok(iface) = conn.object_server().interface::<_, Agent>(AGENT_PATH).await {
        *iface.get().await.conn.lock() = Some(conn.clone());
    }
    let manager = crate::actor::proxy(&conn, "org.bluez", "/org/bluez", "org.bluez.AgentManager1").await.map_err(|e| e.to_string())?;
    let path = OwnedObjectPath::try_from(AGENT_PATH).map_err(|e| e.to_string())?;
    manager.call::<_, _, ()>("RegisterAgent", &(path.clone(), CAPABILITY)).await.map_err(|e| e.to_string())?;
    if let Err(e) = manager.call::<_, _, ()>("RequestDefaultAgent", &(path,)).await {
        tracing::info!("bluetooth: agent registered but not default ({e})");
    }
    Ok(conn)
}

/// A UI-only agent for `mandara msg bluetooth pairing-demo`: one fake
/// request per kind, answers logged.
pub fn demo_agent(kind: PairingKind) -> PairingAgent {
    let (tx, events) = async_channel::bounded(EVENT_QUEUE);
    let shared = Arc::new(Shared::new(tx));
    shared.show("Demo headphones".into(), "00:11:22:33:44:55".into(), "audio-headset".into(), kind);
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
        shared.cancel_all();
        assert_eq!(events.try_recv().unwrap(), PairingEvent::Cancelled(7));
    }
}
