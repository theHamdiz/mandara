//! Secret Service unlock prompts: a native GCR system prompter.
//!
//! When an app asks gnome-keyring for a secret in a locked collection (or
//! creates a keyring), the daemon asks the session's *system prompter* for
//! the password. GNOME Shell is one; without it `gcr-prompter` (a GTK
//! window) gets D-Bus activated. Mandara serves the same protocol so the
//! prompt is a Mandara dialog:
//!
//! * well-known name `org.gnome.keyring.SystemPrompter`, object
//!   `/org/gnome/keyring/Prompter`, interface
//!   `org.gnome.keyring.internal.Prompter`:
//!   `BeginPrompting(o callback)`, `PerformPrompt(o callback, s type,
//!   a{sv} properties, s exchange)`, `StopPrompting(o callback)`;
//! * replies go to the caller's `org.gnome.keyring.internal.Prompter.Callback`
//!   object: `PromptReady(s reply, a{sv} properties, s exchange)` (reply
//!   `""` when ready for the first prompt, then `"yes"` / `"no"`) and
//!   `PromptDone()`;
//! * the password travels encrypted through gcr's secret exchange
//!   ([`exchange`], `[sx-aes-1]`).
//!
//! Like gcr's prompter in single mode, one caller prompts at a time; others
//! wait (FIFO) until it calls `StopPrompting` or leaves the bus. Mandara
//! never fights another prompter: the name is requested without
//! replacement, so with GNOME Shell (or a running `gcr-prompter`) we wait
//! in the bus queue and only serve once it is gone; we allow replacement,
//! so a prompter that insists can take over.
//!
//! The UI (a dialog in mandara-ui) talks to this actor through
//! [`KeyringEvent`]s and [`KeyringReply`]s keyed by a prompt id.

pub mod exchange;

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt as _;
use parking_lot::Mutex;
use zbus::fdo;
use zbus::interface;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};
use zeroize::Zeroizing;

pub use exchange::{ExchangeError, SecretExchange};

pub const BUS_NAME: &str = "org.gnome.keyring.SystemPrompter";
pub const PROMPTER_PATH: &str = "/org/gnome/keyring/Prompter";
pub const PROMPTER_INTERFACE: &str = "org.gnome.keyring.internal.Prompter";
pub const CALLBACK_INTERFACE: &str = "org.gnome.keyring.internal.Prompter.Callback";
pub const TYPE_PASSWORD: &str = "password";
pub const TYPE_CONFIRM: &str = "confirm";
pub const REPLY_NONE: &str = "";
pub const REPLY_YES: &str = "yes";
pub const REPLY_NO: &str = "no";

/// Events buffered for the UI; replies buffered from it.
const EVENT_QUEUE: usize = 64;
const REPLY_QUEUE: usize = 16;
/// Calls to callers, queued in order.
const OUTGOING_QUEUE: usize = 64;
/// A method handler's reply is written after it returns; callbacks it
/// triggers wait this long so the caller always sees the reply first (gcr
/// returns from `BeginPrompting` before it sends `PromptReady`).
const AFTER_REPLY: Duration = Duration::from_millis(15);
/// A caller that never answers a callback is given up after this long.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(10);

// ─────────────────────────────── properties ───────────────────────────────

/// The `GcrPrompt` properties a caller sets (only changed ones are sent per
/// `PerformPrompt`, so they accumulate per caller).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PromptProps {
    pub title: String,
    pub message: String,
    pub description: String,
    /// Shown after a failed attempt ("The unlock password was incorrect").
    pub warning: String,
    /// Creating a keyring: ask for the password twice.
    pub password_new: bool,
    pub password_strength: i32,
    /// e.g. "Automatically unlock this keyring whenever I'm logged in".
    pub choice_label: String,
    pub choice_chosen: bool,
    /// X11 window id or Wayland handle of the asking app (informational).
    pub caller_window: String,
    pub continue_label: String,
    pub cancel_label: String,
}

impl PromptProps {
    /// Applies one `a{sv}` dictionary; returns the names it did not know.
    pub fn apply(&mut self, props: &HashMap<String, OwnedValue>) -> Vec<String> {
        let mut unknown = Vec::new();
        for (name, value) in props {
            // Be lenient with callers that box values twice (gcr does so
            // in the other direction, see `PropValue::to_wire`).
            let mut value: &Value<'_> = value;
            while let Value::Value(inner) = value {
                value = inner;
            }
            let text = || <&str>::try_from(value).map(str::to_string).unwrap_or_default();
            let flag = || bool::try_from(value).unwrap_or(false);
            match name.as_str() {
                "title" => self.title = text(),
                "message" => self.message = text(),
                "description" => self.description = text(),
                "warning" => self.warning = text(),
                "password-new" => self.password_new = flag(),
                "password-strength" => self.password_strength = i32::try_from(value).unwrap_or(0),
                "choice-label" => self.choice_label = text(),
                "choice-chosen" => self.choice_chosen = flag(),
                "caller-window" => self.caller_window = text(),
                "continue-label" => self.continue_label = text(),
                "cancel-label" => self.cancel_label = text(),
                _ => unknown.push(name.clone()),
            }
        }
        unknown
    }

    /// What the prompt reports back with its answer (the user may have
    /// ticked the choice; a new password has a strength).
    fn reported(&self) -> Vec<(String, PropValue)> {
        let mut out = Vec::new();
        if !self.choice_label.is_empty() {
            out.push(("choice-chosen".to_string(), PropValue::Bool(self.choice_chosen)));
        }
        if self.password_new {
            out.push(("password-strength".to_string(), PropValue::Int(self.password_strength)));
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropValue {
    Str(String),
    Bool(bool),
    Int(i32),
}

impl PropValue {
    /// As `PromptReady` carries it: gcr's prompter adds each property as
    /// `g_variant_new_variant (value)` under `{sv}`, i.e. a variant inside
    /// the dictionary's variant, and the client unwraps it with
    /// `g_variant_get_variant` (a plain value trips a GLib critical and is
    /// lost).
    fn to_wire(&self) -> Value<'_> {
        let inner = match self {
            PropValue::Str(s) => Value::from(s.as_str()),
            PropValue::Bool(b) => Value::from(*b),
            PropValue::Int(i) => Value::from(*i),
        };
        Value::Value(Box::new(inner))
    }
}

/// Strength of a new password for the meter (and gcr's
/// `password-strength`): 0 for empty, then one point per character class
/// used (lower, upper, digit, other) and one for length ≥ 12, capped at 4;
/// anything shorter than 8 characters scores at most 1.
pub fn password_strength(password: &str) -> i32 {
    const SHORT: usize = 8;
    const LONG: usize = 12;
    const MAX: i32 = 4;
    if password.is_empty() {
        return 0;
    }
    let classes = [
        password.chars().any(char::is_lowercase),
        password.chars().any(char::is_uppercase),
        password.chars().any(|c| c.is_ascii_digit()),
        password.chars().any(|c| !c.is_alphanumeric()),
    ];
    let score = classes.iter().filter(|c| **c).count() as i32 + i32::from(password.chars().count() >= LONG);
    let len = password.chars().count();
    if len < SHORT { 1 } else { score.clamp(1, MAX) }
}

// ─────────────────────────────── UI protocol ───────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PromptKind {
    Password,
    Confirm,
}

/// A prompt to show (a repeated id updates the open dialog: a retry).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptRequest {
    pub id: u64,
    pub kind: PromptKind,
    pub props: PromptProps,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrompterStatus {
    /// We own the name and answer prompts.
    Serving,
    /// Another prompter (GNOME Shell, gcr-prompter) owns it; we wait in the
    /// bus queue.
    AnotherPrompter,
    Disabled,
    Unavailable(String),
}

/// Prompter → UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyringEvent {
    Show(PromptRequest),
    /// The caller stopped prompting (or vanished): close the dialog.
    Close { id: u64 },
    Status(PrompterStatus),
}

/// UI → prompter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyringReply {
    Password { id: u64, password: Zeroizing<String>, choice: bool },
    Confirm { id: u64, choice: bool },
    Cancel { id: u64 },
}

impl KeyringReply {
    pub fn id(&self) -> u64 {
        match self {
            KeyringReply::Password { id, .. } | KeyringReply::Confirm { id, .. } | KeyringReply::Cancel { id } => *id,
        }
    }
}

// ─────────────────────────────── state machine ───────────────────────────────

/// A caller: its unique bus name and callback object path.
pub type CallerKey = (String, String);

/// What the state machine wants done.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Call `PromptReady` on the caller.
    Ready { key: CallerKey, reply: &'static str, props: Vec<(String, PropValue)>, exchange: String },
    /// Call `PromptDone` on the caller.
    Done { key: CallerKey },
    Ui(KeyringEvent),
}

#[derive(Debug, PartialEq, Eq)]
pub enum PromptError {
    AlreadyBegun,
    /// No such caller, or it is waiting behind another one.
    NotActive,
    /// A prompt is already showing for it.
    InProgress,
    InvalidExchange,
    InvalidType(String),
}

impl From<PromptError> for fdo::Error {
    fn from(e: PromptError) -> Self {
        match e {
            PromptError::AlreadyBegun => fdo::Error::Failed("Already begun prompting for this prompt callback".into()),
            PromptError::NotActive => fdo::Error::Failed("Not yet ready to prompt for this callback".into()),
            PromptError::InProgress => fdo::Error::Failed("Already performing a prompt for this prompt callback".into()),
            PromptError::InvalidExchange => fdo::Error::InvalidArgs("Invalid secret exchange received".into()),
            PromptError::InvalidType(t) => fdo::Error::InvalidArgs(format!("Invalid type argument: {t}")),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase {
    /// Waiting behind the active caller.
    Queued,
    /// `PromptReady` sent; waiting for `PerformPrompt` or `StopPrompting`.
    Ready,
    /// The dialog is up.
    Prompting(PromptKind),
}

struct Caller {
    key: CallerKey,
    id: u64,
    exchange: SecretExchange,
    props: PromptProps,
    phase: Phase,
    /// The user cancelled while the caller was busy with the last answer:
    /// its next prompt is refused without showing.
    cancelled: bool,
}

/// gcr's single-mode prompter as a pure state machine (no I/O): inputs are
/// D-Bus calls and UI replies, outputs are [`Action`]s.
pub struct PrompterState {
    callers: VecDeque<Caller>,
    next_id: u64,
}

impl Default for PrompterState {
    fn default() -> Self {
        Self { callers: VecDeque::new(), next_id: 1 }
    }
}

impl PrompterState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Callers known (active + waiting).
    pub fn len(&self) -> usize {
        self.callers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.callers.is_empty()
    }

    pub fn has_sender(&self, name: &str) -> bool {
        self.callers.iter().any(|c| c.key.0 == name)
    }

    pub fn begin(&mut self, key: CallerKey) -> Result<Vec<Action>, PromptError> {
        self.begin_with(key, SecretExchange::new().map_err(|_| PromptError::InvalidExchange)?)
    }

    /// [`PrompterState::begin`] with a given key pair (tests).
    pub fn begin_with(&mut self, key: CallerKey, exchange: SecretExchange) -> Result<Vec<Action>, PromptError> {
        if self.callers.iter().any(|c| c.key == key) {
            return Err(PromptError::AlreadyBegun);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.callers.push_back(Caller { key, id, exchange, props: PromptProps::default(), phase: Phase::Queued, cancelled: false });
        Ok(self.next_ready())
    }

    pub fn perform(&mut self, key: &CallerKey, kind: &str, props: &HashMap<String, OwnedValue>, exchange: &str) -> Result<Vec<Action>, PromptError> {
        let caller = self.callers.front_mut().filter(|c| &c.key == key).ok_or(PromptError::NotActive)?;
        let kind = match kind {
            TYPE_PASSWORD => PromptKind::Password,
            TYPE_CONFIRM => PromptKind::Confirm,
            other => return Err(PromptError::InvalidType(other.to_string())),
        };
        match caller.phase {
            Phase::Ready => {}
            Phase::Prompting(_) => return Err(PromptError::InProgress),
            Phase::Queued => return Err(PromptError::NotActive),
        }
        // gcr receives the caller's exchange before applying properties; a
        // caller never sends a secret to the prompter, so drop any.
        caller.exchange.receive(exchange).map_err(|_| PromptError::InvalidExchange)?;
        let unknown = caller.props.apply(props);
        if !unknown.is_empty() {
            tracing::debug!("keyring prompt: ignoring unknown properties {unknown:?}");
        }
        if caller.cancelled {
            caller.cancelled = false;
            let exchange = caller.exchange.send(None).map_err(|_| PromptError::InvalidExchange)?;
            return Ok(vec![Action::Ready { key: caller.key.clone(), reply: REPLY_NO, props: caller.props.reported(), exchange }]);
        }
        caller.phase = Phase::Prompting(kind);
        Ok(vec![Action::Ui(KeyringEvent::Show(PromptRequest { id: caller.id, kind, props: caller.props.clone() }))])
    }

    /// The caller is done (`StopPrompting`): gcr answers with `PromptDone`.
    pub fn stop(&mut self, key: &CallerKey) -> Vec<Action> {
        let Some(ix) = self.callers.iter().position(|c| &c.key == key) else { return Vec::new() };
        let caller = self.callers.remove(ix).expect("index from position");
        let mut out = vec![Action::Done { key: caller.key }];
        if ix == 0 {
            out.push(Action::Ui(KeyringEvent::Close { id: caller.id }));
            out.extend(self.next_ready());
        }
        out
    }

    /// The user answered the dialog.
    pub fn reply(&mut self, reply: KeyringReply) -> Vec<Action> {
        let Some(caller) = self.callers.front_mut().filter(|c| c.id == reply.id()) else { return Vec::new() };
        let Phase::Prompting(kind) = caller.phase else {
            // Cancelled while the caller checks the last answer: refuse
            // whatever it asks next (e.g. a retry) instead of reopening.
            if matches!(reply, KeyringReply::Cancel { .. }) && caller.phase == Phase::Ready {
                caller.cancelled = true;
                return vec![Action::Ui(KeyringEvent::Close { id: caller.id })];
            }
            return Vec::new();
        };
        let (answer, secret, close) = match reply {
            KeyringReply::Password { password, choice, .. } if kind == PromptKind::Password => {
                caller.props.choice_chosen = choice;
                if caller.props.password_new {
                    caller.props.password_strength = password_strength(&password);
                }
                (REPLY_YES, Some(password), false)
            }
            KeyringReply::Confirm { choice, .. } if kind == PromptKind::Confirm => {
                caller.props.choice_chosen = choice;
                (REPLY_YES, None, false)
            }
            KeyringReply::Cancel { .. } => (REPLY_NO, None, true),
            other => {
                tracing::debug!("keyring prompt: reply {:?} does not fit a {kind:?} prompt", std::mem::discriminant(&other));
                return Vec::new();
            }
        };
        let exchange = match caller.exchange.send(secret.as_deref().map(String::as_str)) {
            Ok(exchange) => exchange,
            Err(err) => {
                tracing::warn!("keyring prompt: cannot encrypt the reply: {err}");
                return Vec::new();
            }
        };
        caller.phase = Phase::Ready;
        let mut out = vec![Action::Ready { key: caller.key.clone(), reply: answer, props: caller.props.reported(), exchange }];
        if close {
            out.push(Action::Ui(KeyringEvent::Close { id: caller.id }));
        }
        out
    }

    /// Every caller under `name` left the bus (gcr: no `PromptDone`).
    pub fn vanished(&mut self, name: &str) -> Vec<Action> {
        let front = self.callers.front().filter(|c| c.key.0 == name).map(|c| c.id);
        self.callers.retain(|c| c.key.0 != name);
        let Some(id) = front else { return Vec::new() };
        let mut out = vec![Action::Ui(KeyringEvent::Close { id })];
        out.extend(self.next_ready());
        out
    }

    /// A callback call failed: forget that caller without `PromptDone`.
    pub fn failed(&mut self, key: &CallerKey) -> Vec<Action> {
        let Some(ix) = self.callers.iter().position(|c| &c.key == key) else { return Vec::new() };
        let caller = self.callers.remove(ix).expect("index from position");
        if ix != 0 {
            return Vec::new();
        }
        let mut out = vec![Action::Ui(KeyringEvent::Close { id: caller.id })];
        out.extend(self.next_ready());
        out
    }

    /// The prompter goes away: tell every caller it is done.
    pub fn shutdown(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        if let Some(front) = self.callers.front() {
            out.push(Action::Ui(KeyringEvent::Close { id: front.id }));
        }
        out.extend(self.callers.drain(..).map(|c| Action::Done { key: c.key }));
        out
    }

    /// Lets the front caller start: `PromptReady("", {}, begin)`.
    fn next_ready(&mut self) -> Vec<Action> {
        match self.callers.front_mut() {
            Some(c) if c.phase == Phase::Queued => {
                c.phase = Phase::Ready;
                vec![Action::Ready { key: c.key.clone(), reply: REPLY_NONE, props: Vec::new(), exchange: c.exchange.begin() }]
            }
            _ => Vec::new(),
        }
    }
}

// ─────────────────────────────── handle ───────────────────────────────

/// Handle held by the UI.
#[derive(Clone)]
pub struct KeyringPrompter {
    pub events: async_channel::Receiver<KeyringEvent>,
    replies: async_channel::Sender<KeyringReply>,
    /// Closed by [`KeyringPrompter::stop`]: pending callers get
    /// `PromptDone` and the name is released.
    stop: async_channel::Sender<()>,
    stopped: Arc<std::sync::atomic::AtomicBool>,
}

impl KeyringPrompter {
    /// A handle over arbitrary channels (previews and tests).
    pub fn from_channels(events: async_channel::Receiver<KeyringEvent>, replies: async_channel::Sender<KeyringReply>) -> Self {
        Self { events, replies, stop: async_channel::bounded(1).0, stopped: Arc::default() }
    }

    pub fn reply(&self, reply: KeyringReply) {
        let _ = self.replies.try_send(reply);
    }

    /// Stops serving (General → Keyring unlock prompts off).
    pub fn stop(&self) {
        self.stopped.store(true, std::sync::atomic::Ordering::Relaxed);
        self.stop.close();
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(std::sync::atomic::Ordering::Relaxed)
    }
}

// ─────────────────────────────── D-Bus side ───────────────────────────────

struct Shared {
    state: Mutex<PrompterState>,
    events: async_channel::Sender<KeyringEvent>,
    outgoing: async_channel::Sender<(Action, bool)>,
}

impl Shared {
    /// Routes actions: UI events now, callbacks through the ordered queue
    /// (`after_reply`: issued from a method handler).
    fn dispatch(&self, actions: Vec<Action>, after_reply: bool) {
        for action in actions {
            match action {
                Action::Ui(event) => {
                    let _ = self.events.try_send(event);
                }
                other => {
                    let _ = self.outgoing.try_send((other, after_reply));
                }
            }
        }
    }
}

struct Prompter {
    shared: Arc<Shared>,
}

fn caller_key(header: &zbus::message::Header<'_>, path: &ObjectPath<'_>) -> fdo::Result<CallerKey> {
    let sender = header.sender().ok_or_else(|| fdo::Error::Failed("no sender".into()))?;
    Ok((sender.to_string(), path.to_string()))
}

#[interface(name = "org.gnome.keyring.internal.Prompter")]
impl Prompter {
    async fn begin_prompting(&self, #[zbus(header)] header: zbus::message::Header<'_>, callback: ObjectPath<'_>) -> fdo::Result<()> {
        let key = caller_key(&header, &callback)?;
        tracing::info!("keyring prompt: {} begins prompting", key.0);
        let actions = self.shared.state.lock().begin(key)?;
        self.shared.dispatch(actions, true);
        Ok(())
    }

    async fn perform_prompt(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        callback: ObjectPath<'_>,
        r#type: String,
        properties: HashMap<String, OwnedValue>,
        exchange: String,
    ) -> fdo::Result<()> {
        let key = caller_key(&header, &callback)?;
        let actions = self.shared.state.lock().perform(&key, &r#type, &properties, &exchange)?;
        self.shared.dispatch(actions, true);
        Ok(())
    }

    async fn stop_prompting(&self, #[zbus(header)] header: zbus::message::Header<'_>, callback: ObjectPath<'_>) -> fdo::Result<()> {
        let key = caller_key(&header, &callback)?;
        let actions = self.shared.state.lock().stop(&key);
        self.shared.dispatch(actions, true);
        Ok(())
    }
}

/// Calls a caller's callback object.
async fn call_back(conn: &zbus::Connection, action: &Action) -> zbus::Result<()> {
    let (key, method) = match action {
        Action::Ready { key, .. } => (key, "PromptReady"),
        Action::Done { key } => (key, "PromptDone"),
        Action::Ui(_) => return Ok(()),
    };
    let call = async {
        match action {
            Action::Ready { reply, props, exchange, .. } => {
                let dict: HashMap<&str, Value<'_>> = props.iter().map(|(k, v)| (k.as_str(), v.to_wire())).collect();
                conn.call_method(Some(key.0.as_str()), key.1.as_str(), Some(CALLBACK_INTERFACE), method, &(*reply, dict, exchange.as_str())).await
            }
            _ => conn.call_method(Some(key.0.as_str()), key.1.as_str(), Some(CALLBACK_INTERFACE), method, &()).await,
        }
    };
    futures::select! {
        r = call.fuse() => r.map(|_| ()),
        _ = crate::runtime::sleep(CALLBACK_TIMEOUT).fuse() => Err(zbus::Error::Failure(format!("{method} timed out"))),
    }
}

/// Sends queued callbacks in order; a failed `PromptReady` drops the caller.
async fn sender(conn: zbus::Connection, shared: Arc<Shared>, rx: async_channel::Receiver<(Action, bool)>) {
    while let Ok((action, after_reply)) = rx.recv().await {
        if after_reply {
            crate::runtime::sleep(AFTER_REPLY).await;
        }
        if let Err(err) = call_back(&conn, &action).await
            && let Action::Ready { key, .. } = &action
        {
            tracing::warn!("keyring prompt: PromptReady to {} failed: {err}", key.0);
            let actions = shared.state.lock().failed(key);
            shared.dispatch(actions, false);
        }
    }
}

/// Starts the prompter on the session bus (unless `enabled` is false).
pub fn spawn_keyring_prompt(enabled: bool) -> KeyringPrompter {
    spawn_with(enabled, None)
}

/// Starts the prompter on the bus at `address` (private test buses).
pub fn spawn_keyring_prompt_at(address: &str) -> KeyringPrompter {
    spawn_with(true, Some(address.to_string()))
}

fn spawn_with(enabled: bool, address: Option<String>) -> KeyringPrompter {
    let (events_tx, events) = async_channel::bounded(EVENT_QUEUE);
    let (replies, replies_rx) = async_channel::bounded::<KeyringReply>(REPLY_QUEUE);
    let (stop, stop_rx) = async_channel::bounded::<()>(1);
    let handle = KeyringPrompter { events, replies, stop, stopped: Arc::default() };
    if !enabled {
        let _ = events_tx.try_send(KeyringEvent::Status(PrompterStatus::Disabled));
        return handle;
    }
    crate::runtime::spawn("keyring-prompt", async move {
        let status = |s| {
            let _ = events_tx.try_send(KeyringEvent::Status(s));
        };
        let (out_tx, out_rx) = async_channel::bounded(OUTGOING_QUEUE);
        let shared = Arc::new(Shared { state: Mutex::new(PrompterState::new()), events: events_tx.clone(), outgoing: out_tx });
        let conn = match connect(address.as_deref(), shared.clone()).await {
            Ok(conn) => conn,
            Err(err) => {
                tracing::warn!("keyring prompt: unavailable: {err}");
                return status(PrompterStatus::Unavailable(err.to_string()));
            }
        };
        crate::runtime::spawn("keyring-prompt-sender", sender(conn.clone(), shared.clone(), out_rx));
        // UI answers.
        {
            let shared = shared.clone();
            crate::runtime::spawn("keyring-prompt-replies", async move {
                while let Ok(reply) = replies_rx.recv().await {
                    let actions = shared.state.lock().reply(reply);
                    shared.dispatch(actions, false);
                }
            });
        }
        let flags = fdo::RequestNameFlags::AllowReplacement.into();
        match conn.request_name_with_flags(BUS_NAME, flags).await {
            Ok(fdo::RequestNameReply::PrimaryOwner | fdo::RequestNameReply::AlreadyOwner) => {
                tracing::info!("keyring prompt: serving {BUS_NAME}");
                status(PrompterStatus::Serving);
            }
            Ok(_) => {
                tracing::info!("keyring prompt: another prompter owns {BUS_NAME}; waiting in the queue");
                status(PrompterStatus::AnotherPrompter);
            }
            Err(err) => {
                tracing::warn!("keyring prompt: cannot request {BUS_NAME}: {err}");
                return status(PrompterStatus::Unavailable(err.to_string()));
            }
        }
        watch_bus(&conn, &shared, &stop_rx).await;
        // Stopped: callers are told we're done, then the name goes with
        // the connection.
        let actions = shared.state.lock().shutdown();
        for action in &actions {
            if let Action::Ui(event) = action {
                let _ = events_tx.try_send(event.clone());
            } else {
                let _ = call_back(&conn, action).await;
            }
        }
        let _ = conn.release_name(BUS_NAME).await;
        tracing::info!("keyring prompt: stopped; {BUS_NAME} released");
        status(PrompterStatus::Disabled);
    });
    handle
}

async fn connect(address: Option<&str>, shared: Arc<Shared>) -> zbus::Result<zbus::Connection> {
    let builder = match address {
        Some(addr) => zbus::connection::Builder::address(addr)?,
        None => zbus::connection::Builder::session()?,
    };
    crate::runtime::connect(builder.serve_at(PROMPTER_PATH, Prompter { shared })?).await
}

/// Follows name ownership (ours, and callers leaving) until stopped.
async fn watch_bus(conn: &zbus::Connection, shared: &Arc<Shared>, stop: &async_channel::Receiver<()>) {
    use futures::StreamExt as _;
    let Ok(dbus) = fdo::DBusProxy::new(conn).await else {
        let _ = stop.recv().await;
        return;
    };
    let (Ok(mut owners), Ok(mut acquired), Ok(mut lost)) =
        (dbus.receive_name_owner_changed().await, dbus.receive_name_acquired().await, dbus.receive_name_lost().await)
    else {
        let _ = stop.recv().await;
        return;
    };
    let status = |s| {
        let _ = shared.events.try_send(KeyringEvent::Status(s));
    };
    loop {
        futures::select! {
            _ = stop.recv().fuse() => return,
            sig = owners.next().fuse() => {
                let Some(sig) = sig else { return };
                let Ok(args) = sig.args() else { continue };
                if args.new_owner().is_none() {
                    let name = args.name().to_string();
                    let mut state = shared.state.lock();
                    if state.has_sender(&name) {
                        tracing::info!("keyring prompt: {name} left the bus");
                        let actions = state.vanished(&name);
                        drop(state);
                        shared.dispatch(actions, false);
                    }
                }
            }
            sig = acquired.next().fuse() => {
                if sig.is_some_and(|s| s.args().is_ok_and(|a| a.name() == BUS_NAME)) {
                    tracing::info!("keyring prompt: now serving {BUS_NAME}");
                    status(PrompterStatus::Serving);
                }
            }
            sig = lost.next().fuse() => {
                if sig.is_some_and(|s| s.args().is_ok_and(|a| a.name() == BUS_NAME)) {
                    // Another prompter replaced us: let our callers go.
                    tracing::info!("keyring prompt: another prompter took over {BUS_NAME}");
                    let actions = shared.state.lock().shutdown();
                    shared.dispatch(actions, false);
                    status(PrompterStatus::AnotherPrompter);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: &str) -> CallerKey {
        (format!(":1.{n}"), "/org/gnome/keyring/Prompt/p1".into())
    }

    fn props(pairs: &[(&str, Value<'_>)]) -> HashMap<String, OwnedValue> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.try_to_owned().unwrap())).collect()
    }

    /// Plays gnome-keyring's side: its own exchange, fed by PromptReady.
    struct FakeCaller {
        key: CallerKey,
        exchange: SecretExchange,
    }

    impl FakeCaller {
        fn new(n: &str) -> Self {
            Self { key: key(n), exchange: SecretExchange::new().unwrap() }
        }
        /// Takes a PromptReady; returns (reply, props, secret).
        fn ready(&mut self, action: &Action) -> (&'static str, Vec<(String, PropValue)>, Option<String>) {
            let Action::Ready { key, reply, props, exchange } = action else { panic!("expected Ready, got {action:?}") };
            assert_eq!(key, &self.key);
            let secret = self.exchange.receive(exchange).unwrap().map(|s| s.to_string());
            (*reply, props.clone(), secret)
        }
        fn exchange(&self) -> String {
            self.exchange.send(None).unwrap()
        }
    }

    fn show(actions: &[Action]) -> PromptRequest {
        match actions {
            [Action::Ui(KeyringEvent::Show(req))] => req.clone(),
            other => panic!("expected one Show, got {other:?}"),
        }
    }

    #[test]
    fn unlock_flow_with_a_retry() {
        let mut st = PrompterState::new();
        let mut gk = FakeCaller::new("7");
        // BeginPrompting → PromptReady("", {}, public key only).
        let begin = st.begin(gk.key.clone()).unwrap();
        assert_eq!(begin.len(), 1);
        assert_eq!(gk.ready(&begin[0]), (REPLY_NONE, vec![], None));
        assert_eq!(st.begin(gk.key.clone()), Err(PromptError::AlreadyBegun));
        // PerformPrompt(password) → the dialog.
        let p = props(&[
            ("title", Value::from("Unlock Keyring")),
            ("description", Value::from("An application wants access to the keyring “Login”, but it is locked")),
            ("choice-label", Value::from("Automatically unlock this keyring whenever I’m logged in")),
            ("password-new", Value::from(false)),
            ("x-unknown", Value::from(3u32)),
        ]);
        let req = show(&st.perform(&gk.key, TYPE_PASSWORD, &p, &gk.exchange()).unwrap());
        assert_eq!(req.kind, PromptKind::Password);
        assert_eq!(req.props.title, "Unlock Keyring");
        assert!(!req.props.choice_label.is_empty());
        // While it shows, a second PerformPrompt is refused.
        assert_eq!(st.perform(&gk.key, TYPE_PASSWORD, &HashMap::new(), &gk.exchange()), Err(PromptError::InProgress));
        // The user types; PromptReady("yes") carries the encrypted secret.
        let out = st.reply(KeyringReply::Password { id: req.id, password: Zeroizing::new("wrong".into()), choice: true });
        let (reply, reported, secret) = gk.ready(&out[0]);
        assert_eq!((reply, secret.as_deref()), (REPLY_YES, Some("wrong")));
        assert_eq!(reported, vec![("choice-chosen".to_string(), PropValue::Bool(true))]);
        // Retry: gnome-keyring sends only the changed warning; the rest stays.
        let retry = show(&st.perform(&gk.key, TYPE_PASSWORD, &props(&[("warning", Value::from("The unlock password was incorrect"))]), &gk.exchange()).unwrap());
        assert_eq!(retry.id, req.id, "same dialog");
        assert_eq!(retry.props.warning, "The unlock password was incorrect");
        assert_eq!(retry.props.title, "Unlock Keyring");
        assert!(retry.props.choice_chosen, "the choice the user made sticks");
        let out = st.reply(KeyringReply::Password { id: req.id, password: Zeroizing::new("right".into()), choice: true });
        assert_eq!(gk.ready(&out[0]).2.as_deref(), Some("right"));
        // StopPrompting → PromptDone + the dialog closes.
        let out = st.stop(&gk.key);
        assert_eq!(out, vec![Action::Done { key: gk.key.clone() }, Action::Ui(KeyringEvent::Close { id: req.id })]);
        assert!(st.is_empty());
    }

    #[test]
    fn cancel_replies_no_and_closes() {
        let mut st = PrompterState::new();
        let mut gk = FakeCaller::new("8");
        gk.ready(&st.begin(gk.key.clone()).unwrap()[0]);
        let req = show(&st.perform(&gk.key, TYPE_PASSWORD, &HashMap::new(), &gk.exchange()).unwrap());
        let out = st.reply(KeyringReply::Cancel { id: req.id });
        assert_eq!(out.len(), 2);
        assert_eq!(gk.ready(&out[0]), (REPLY_NO, vec![], None));
        assert_eq!(out[1], Action::Ui(KeyringEvent::Close { id: req.id }));
        // A late reply for the same prompt is ignored.
        assert!(st.reply(KeyringReply::Password { id: req.id, password: Zeroizing::new("x".into()), choice: false }).is_empty());
    }

    #[test]
    fn cancel_while_checking_refuses_the_retry() {
        let mut st = PrompterState::new();
        let mut gk = FakeCaller::new("12");
        gk.ready(&st.begin(gk.key.clone()).unwrap()[0]);
        let req = show(&st.perform(&gk.key, TYPE_PASSWORD, &HashMap::new(), &gk.exchange()).unwrap());
        let out = st.reply(KeyringReply::Password { id: req.id, password: Zeroizing::new("wrong".into()), choice: false });
        assert_eq!(gk.ready(&out[0]).0, REPLY_YES);
        // The user gives up while gnome-keyring checks the password.
        assert_eq!(st.reply(KeyringReply::Cancel { id: req.id }), vec![Action::Ui(KeyringEvent::Close { id: req.id })]);
        // Its retry is answered "no" at once, without a dialog.
        let out = st.perform(&gk.key, TYPE_PASSWORD, &props(&[("warning", Value::from("The unlock password was incorrect"))]), &gk.exchange()).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(gk.ready(&out[0]), (REPLY_NO, vec![], None));
        // A later prompt shows normally again.
        assert!(matches!(&st.perform(&gk.key, TYPE_CONFIRM, &HashMap::new(), &gk.exchange()).unwrap()[..], [Action::Ui(KeyringEvent::Show(_))]));
    }

    #[test]
    fn new_keyring_reports_strength_and_confirm_works() {
        let mut st = PrompterState::new();
        let mut gk = FakeCaller::new("9");
        gk.ready(&st.begin(gk.key.clone()).unwrap()[0]);
        let req = show(&st.perform(&gk.key, TYPE_PASSWORD, &props(&[("password-new", Value::from(true))]), &gk.exchange()).unwrap());
        assert!(req.props.password_new);
        let out = st.reply(KeyringReply::Password { id: req.id, password: Zeroizing::new("Sea-glass 1".into()), choice: false });
        let (reply, reported, secret) = gk.ready(&out[0]);
        assert_eq!((reply, secret.as_deref()), (REPLY_YES, Some("Sea-glass 1")));
        assert_eq!(reported, vec![("password-strength".to_string(), PropValue::Int(4))]);
        // Then a confirm prompt ("store unencrypted?") on the same caller.
        let c = show(&st.perform(&gk.key, TYPE_CONFIRM, &props(&[("continue-label", Value::from("Continue"))]), &gk.exchange()).unwrap());
        assert_eq!(c.kind, PromptKind::Confirm);
        // A password answer doesn't fit a confirm prompt.
        assert!(st.reply(KeyringReply::Password { id: c.id, password: Zeroizing::new("x".into()), choice: false }).is_empty());
        let out = st.reply(KeyringReply::Confirm { id: c.id, choice: false });
        assert_eq!(gk.ready(&out[0]).0, REPLY_YES);
        assert_eq!(st.perform(&gk.key, "pinentry", &HashMap::new(), &gk.exchange()), Err(PromptError::InvalidType("pinentry".into())));
        assert_eq!(st.perform(&gk.key, TYPE_PASSWORD, &HashMap::new(), "garbage"), Err(PromptError::InvalidExchange));
    }

    #[test]
    fn callers_wait_their_turn() {
        let mut st = PrompterState::new();
        let mut a = FakeCaller::new("10");
        let mut b = FakeCaller::new("11");
        a.ready(&st.begin(a.key.clone()).unwrap()[0]);
        assert!(st.begin(b.key.clone()).unwrap().is_empty(), "b waits");
        assert_eq!(st.perform(&b.key, TYPE_PASSWORD, &HashMap::new(), &b.exchange()), Err(PromptError::NotActive));
        let out = st.stop(&a.key);
        assert!(matches!(&out[0], Action::Done { key } if key == &a.key));
        assert_eq!(b.ready(out.last().unwrap()).0, REPLY_NONE, "b is ready once a stops");
        // b leaves the bus mid-prompt: no PromptDone, the dialog closes.
        let req = show(&st.perform(&b.key, TYPE_PASSWORD, &HashMap::new(), &b.exchange()).unwrap());
        assert_eq!(st.vanished(&b.key.0), vec![Action::Ui(KeyringEvent::Close { id: req.id })]);
        assert!(st.is_empty());
    }

    #[test]
    fn vanishing_or_failing_waiters_and_shutdown() {
        let mut st = PrompterState::new();
        let a = FakeCaller::new("20");
        let b = FakeCaller::new("21");
        let c = FakeCaller::new("22");
        st.begin(a.key.clone()).unwrap();
        st.begin(b.key.clone()).unwrap();
        st.begin(c.key.clone()).unwrap();
        // A waiter leaving changes nothing for the active caller.
        assert!(st.vanished(&b.key.0).is_empty());
        assert_eq!(st.len(), 2);
        // The active caller's PromptReady failing hands over to c.
        let out = st.failed(&a.key);
        assert!(matches!(&out[..], [Action::Ui(KeyringEvent::Close { .. }), Action::Ready { key, reply: REPLY_NONE, .. }] if key == &c.key));
        let out = st.shutdown();
        assert!(out.contains(&Action::Done { key: c.key.clone() }));
        assert!(st.is_empty());
    }

    #[test]
    fn strength_meter_scores() {
        assert_eq!(password_strength(""), 0);
        assert_eq!(password_strength("abc"), 1);
        assert_eq!(password_strength("aB3$x"), 1, "short passwords stay weak");
        assert_eq!(password_strength("abcdefGH"), 2);
        assert_eq!(password_strength("abcDEF12"), 3);
        assert_eq!(password_strength("abcDEF12!"), 4);
        assert_eq!(password_strength("كلمة سر طويلة جدا"), 2);
    }

    #[test]
    fn disabled_prompter_reports_and_stops() {
        let handle = spawn_keyring_prompt(false);
        assert_eq!(handle.events.try_recv(), Ok(KeyringEvent::Status(PrompterStatus::Disabled)));
        let clone = handle.clone();
        handle.stop();
        assert!(clone.is_stopped());
    }
}
