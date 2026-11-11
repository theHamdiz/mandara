//! Aqueous driver: its IPC v1 socket at `$AQUEOUS_SOCKET`.
//!
//! Aqueous derives from River but renamed its protocols, so the River
//! driver doesn't apply. Its shell IPC is newline-delimited JSON over a
//! private Unix socket whose absolute path the compositor exports to its
//! session; Mandara never guesses or scans for one. Two connections, as
//! the contract asks:
//!
//! * the event connection (`aqueous-events` thread): `hello`, `subscribe`,
//!   then one acknowledged state batch at a time — a full snapshot, then
//!   deltas against the last accepted sequence — kept in one shared
//!   [`state::Model`]; only what changed reaches the bar. A broken stream
//!   drops the model and reconnects with bounded, jittered backoff.
//! * the request connection (`aqueous-requests` thread, started on first
//!   use): `snapshot` queries and typed commands from a bounded queue, so
//!   a click on a workspace pill never waits on the socket. Commands are
//!   resolved against the live model (bar numbers and output names to
//!   Aqueous's runtime ids) and never replayed after a failure.
//!
//! Aqueous keeps its settings in its own hot-reloaded TOML files and
//! documents no include directive, so Mandara writes no drop-in for it
//! ([`mandara_config::compositor::Target::self_configured`]).

mod conn;
pub mod state;
pub mod wire;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Map, Value};

use crate::backend::CompositorBackend;
use crate::model::{ClientInfo, CompositorEvent, CompositorKind, MonitorInfo, NavigationModel};
use conn::{Conn, keeps_connection};
use state::{Diff, Model};
use wire::{Ack, Command, Empty};

/// Handshake, query and command deadline (the contract's 5 s).
const REQUEST_DEADLINE: Duration = Duration::from_secs(5);
/// Deadline of the subscription's initial snapshot (the contract's 8 s).
const SUBSCRIBE_DEADLINE: Duration = Duration::from_secs(8);
/// How long a query waits for the request thread (a queued command ahead
/// of it may take a full deadline).
const QUERY_WAIT: Duration = Duration::from_secs(12);
/// Commands queued for the request connection before new ones are
/// refused as busy (the contract proposes 32).
const COMMAND_QUEUE: usize = 32;
/// The longest pause between reconnection attempts.
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);

/// What a command means in the bar's terms, resolved against the model
/// on the request thread.
#[derive(Debug, PartialEq)]
enum Intent {
    /// Workspace `number` of output `monitor` (the selected one when empty).
    Activate { monitor: String, number: u32 },
    /// Moves a window (the focused one for `None`) to workspace `number`
    /// of its own output.
    Move { client: Option<u64>, number: u32 },
    /// Any documented action with its fields.
    Raw { action: String, fields: Map<String, Value> },
}

enum Job {
    Snapshot(SyncSender<Result<Model>>),
    Command(Intent),
}

struct Shared {
    socket: PathBuf,
    /// The event connection's model (ready once a snapshot is installed).
    live: Mutex<Model>,
    /// The first pause before reconnecting.
    reconnect: Duration,
    /// An event thread owns the state: queries never do I/O any more
    /// (some run on the UI thread), they read the live model or fail fast.
    subscribed: AtomicBool,
}

impl Shared {
    fn live(&self) -> MutexGuard<'_, Model> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub struct AqueousDriver {
    shared: Arc<Shared>,
    jobs: Mutex<Option<SyncSender<Job>>>,
}

impl AqueousDriver {
    /// `$AQUEOUS_SOCKET` when it is an absolute path to an existing socket.
    /// Construction runs on the UI thread and never connects.
    pub fn from_env() -> Option<Self> {
        Self::from_var(std::env::var_os("AQUEOUS_SOCKET"))
    }

    fn from_var(var: Option<std::ffi::OsString>) -> Option<Self> {
        let socket = PathBuf::from(var.filter(|s| !s.is_empty())?);
        (socket.is_absolute() && socket.exists()).then(|| Self::at(socket, crate::RECONNECT_DELAY))
    }

    fn at(socket: PathBuf, reconnect: Duration) -> Self {
        Self { shared: Arc::new(Shared { socket, live: Mutex::new(Model::default()), reconnect, subscribed: AtomicBool::new(false) }), jobs: Mutex::new(None) }
    }

    /// Queues `job` for the request thread, starting it on first use.
    fn submit(&self, job: Job) -> Result<()> {
        let mut jobs = self.jobs.lock().unwrap_or_else(PoisonError::into_inner);
        let job = match jobs.as_ref() {
            Some(tx) => match tx.try_send(job) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Full(_)) => bail!("aqueous: {COMMAND_QUEUE} commands are already waiting"),
                // Its thread is gone: start another.
                Err(TrySendError::Disconnected(job)) => job,
            },
            None => job,
        };
        let (tx, rx) = std::sync::mpsc::sync_channel(COMMAND_QUEUE);
        let shared = self.shared.clone();
        std::thread::Builder::new().name("aqueous-requests".into()).spawn(move || serve_requests(&shared, &rx)).context("starting the Aqueous request thread")?;
        tx.try_send(job).map_err(|_| anyhow!("aqueous: the request thread stopped"))?;
        *jobs = Some(tx);
        Ok(())
    }

    fn command(&self, intent: Intent) -> Result<()> {
        self.submit(Job::Command(intent))
    }

    /// `f` over the live model. Before the event stream starts (the
    /// shell's first load, on a background thread), over a fresh snapshot
    /// instead; once it runs, a query never waits on the socket: while it
    /// reconnects, there is no state to answer from.
    fn query<T>(&self, f: impl FnOnce(&Model) -> T) -> Result<T> {
        {
            let live = self.shared.live();
            if live.ready() {
                return Ok(f(&live));
            }
        }
        if self.shared.subscribed.load(Ordering::Acquire) {
            bail!("aqueous: waiting for the compositor's state");
        }
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.submit(Job::Snapshot(tx))?;
        let model = rx.recv_timeout(QUERY_WAIT).map_err(|_| anyhow!("aqueous: no snapshot within {QUERY_WAIT:?}"))??;
        Ok(f(&model))
    }
}

/// The request thread: one connection (opened on demand, dropped when it
/// fails), one job at a time, until the driver is dropped.
fn serve_requests(shared: &Shared, jobs: &Receiver<Job>) {
    let mut conn: Option<Conn> = None;
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Snapshot(reply) => {
                let _ = reply.send(fetch_snapshot(shared, &mut conn));
            }
            Job::Command(intent) => {
                if let Err(err) = run_command(shared, &mut conn, &intent) {
                    tracing::warn!("aqueous: {intent:?}: {err:#}");
                }
            }
        }
    }
}

/// `op` on the request connection (opened first when needed). A failure
/// other than a plain server error drops it; a command is never replayed.
fn request(shared: &Shared, slot: &mut Option<Conn>, op: &str, params: impl serde::Serialize) -> Result<wire::Reply> {
    if slot.as_ref().is_some_and(Conn::closed) {
        *slot = None;
    }
    let conn = match slot {
        Some(conn) => conn,
        None => slot.insert(Conn::open(&shared.socket, REQUEST_DEADLINE)?),
    };
    let result = conn.request(op, params);
    if let Err(err) = &result
        && !keeps_connection(err)
    {
        *slot = None;
    }
    result
}

fn fetch_snapshot(shared: &Shared, slot: &mut Option<Conn>) -> Result<Model> {
    let reply = request(shared, slot, "snapshot", Empty {})?;
    let batch = reply.batch.ok_or_else(|| anyhow!("aqueous: snapshot without a batch"))?;
    let session = slot.as_ref().map(|c| c.session.as_str()).unwrap_or_default();
    let mut model = Model::new(session);
    model.apply(batch)?;
    Ok(model)
}

fn run_command(shared: &Shared, slot: &mut Option<Conn>, intent: &Intent) -> Result<()> {
    if slot.as_ref().is_none_or(Conn::closed) {
        *slot = Some(Conn::open(&shared.socket, REQUEST_DEADLINE)?);
    }
    let session = slot.as_ref().map(|c| c.session.clone()).unwrap_or_default();
    // The live model when it belongs to this compositor session; a fresh
    // snapshot otherwise (before the subscription's first batch).
    let resolved = {
        let live = shared.live();
        (live.ready() && live.session == session).then(|| resolve(intent, &live))
    };
    let (action, fields) = match resolved {
        Some(resolved) => resolved?,
        None => resolve(intent, &fetch_snapshot(shared, slot)?)?,
    };
    let reply = request(shared, slot, "command", Command { action: &action, fields })?;
    match reply.status.as_deref() {
        Some("applied" | "accepted") => Ok(()),
        other => bail!("aqueous: {action} returned {other:?}"),
    }
}

/// An intent as an Aqueous action and its fields.
fn resolve(intent: &Intent, model: &Model) -> Result<(String, Map<String, Value>)> {
    let mut fields = Map::new();
    let action = match intent {
        Intent::Activate { monitor, number } => {
            let ws = model.workspace_on(monitor, *number).ok_or_else(|| anyhow!("aqueous: no workspace {number} on {}", if monitor.is_empty() { "the focused output" } else { monitor }))?;
            fields.insert("id".into(), ws.id.clone().into());
            "workspace.activate".to_string()
        }
        Intent::Move { client, number } => {
            let window = model.window(*client).ok_or_else(|| anyhow!("aqueous: no such window"))?;
            let workspace = window.workspace.as_deref().and_then(|id| model.workspaces.iter().find(|w| w.id == id));
            let output = window.output.as_deref().or(workspace.map(|w| w.output.as_str())).or_else(|| model.focused_output()).ok_or_else(|| anyhow!("aqueous: the window has no output"))?;
            let target = model.workspaces.iter().find(|w| w.output == output && w.number == *number).ok_or_else(|| anyhow!("aqueous: no workspace {number} on the window's output"))?;
            fields.insert("id".into(), window.id.clone().into());
            fields.insert("workspace".into(), target.id.clone().into());
            "window.move".to_string()
        }
        Intent::Raw { action, fields: given } => {
            fields.clone_from(given);
            // Targets the bar can't name: the focused output / window.
            if matches!(action.as_str(), "overview.show" | "overview.toggle") && !fields.contains_key("output") {
                let output = model.focused_output().ok_or_else(|| anyhow!("aqueous: no output for {action}"))?;
                fields.insert("output".into(), output.to_string().into());
            }
            if action.starts_with("window.") && !fields.contains_key("id") {
                let window = model.focused_window().ok_or_else(|| anyhow!("aqueous: no focused window for {action}"))?;
                fields.insert("id".into(), window.id.clone().into());
            }
            action.clone()
        }
    };
    // An omitted seat needs exactly one seat.
    if matches!(action.as_str(), "workspace.activate" | "window.activate" | "keyboard.set" | "keyboard.next" | "switcher.next" | "switcher.previous" | "switcher.dismiss")
        && !fields.contains_key("seat")
        && let Some(seat) = model.command_seat()
    {
        fields.insert("seat".into(), seat.to_string().into());
    }
    Ok((action, fields))
}

/// Exponential reconnection delay with jitter, from `min` up to
/// [`MAX_RECONNECT_DELAY`].
struct Backoff {
    min: Duration,
    next: Duration,
}

impl Backoff {
    fn new(min: Duration) -> Self {
        Self { min, next: min }
    }

    fn reset(&mut self) {
        self.next = self.min;
    }

    fn delay(&mut self) -> Duration {
        let base = self.next;
        self.next = (base * 2).min(MAX_RECONNECT_DELAY.max(self.min));
        // Up to a quarter more, so several clients don't retry in step.
        let span = u64::try_from((base / 4).as_nanos()).unwrap_or(u64::MAX).max(1);
        let noise = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| u64::from(d.subsec_nanos()));
        base + Duration::from_nanos(noise % span)
    }
}

/// The event thread: subscribes, keeps the shared model current and sends
/// what changed, reconnecting until the shell stops listening.
fn watch_events(shared: &Shared, tx: &async_channel::Sender<CompositorEvent>) {
    let mut backoff = Backoff::new(shared.reconnect);
    loop {
        let result = stream_events(shared, tx, &mut backoff);
        // A broken stream invalidates the model until a fresh snapshot.
        *shared.live() = Model::default();
        match result {
            Ok(()) => return,
            Err(err) => tracing::debug!("aqueous: event stream: {err:#}"),
        }
        if tx.is_closed() {
            return;
        }
        std::thread::sleep(backoff.delay());
    }
}

/// One subscription, until it breaks (`Err`) or the receiver is gone (`Ok`).
fn stream_events(shared: &Shared, tx: &async_channel::Sender<CompositorEvent>, backoff: &mut Backoff) -> Result<()> {
    let mut conn = Conn::open(&shared.socket, REQUEST_DEADLINE)?;
    conn.set_deadline(Some(SUBSCRIBE_DEADLINE))?;
    if conn.request("subscribe", Empty {})?.subscribed != Some(true) {
        bail!("aqueous: subscribe was not confirmed");
    }
    *shared.live() = Model::new(&conn.session);
    let mut diff = Diff::default();
    let mut out = Vec::new();
    let mut idle = false;
    loop {
        let (delivery, batch) = conn.next_event()?.ok_or_else(|| anyhow!("aqueous: the event stream closed"))?;
        {
            let mut live = shared.live();
            let changed = live.apply(batch)?;
            diff.events(&live, changed, &mut out);
        }
        // Installed: acknowledged so the next batch can come.
        let acked = conn.request("ack", Ack { delivery: &delivery })?.acked;
        if acked.as_deref() != Some(delivery.as_str()) {
            bail!("aqueous: ack {delivery} answered with {acked:?}");
        }
        if !idle {
            // An established subscription has no idle timeout.
            conn.set_deadline(None)?;
            backoff.reset();
            idle = true;
        }
        for event in out.drain(..) {
            if tx.send_blocking(event).is_err() {
                return Ok(());
            }
        }
    }
}

impl CompositorBackend for AqueousDriver {
    fn kind(&self) -> CompositorKind {
        CompositorKind::Aqueous
    }

    fn subscribe_events(&self) -> async_channel::Receiver<CompositorEvent> {
        let (tx, rx) = async_channel::bounded(crate::EVENT_QUEUE);
        let shared = self.shared.clone();
        match std::thread::Builder::new().name("aqueous-events".into()).spawn(move || watch_events(&shared, &tx)) {
            Ok(_) => self.shared.subscribed.store(true, Ordering::Release),
            Err(err) => tracing::warn!("aqueous: starting the event thread: {err}"),
        }
        rx
    }

    fn get_navigation_state(&self) -> Result<Vec<(String, NavigationModel)>> {
        self.query(Model::navigation)
    }

    fn get_active_client(&self) -> Result<Option<ClientInfo>> {
        self.query(Model::focused)
    }

    fn get_all_clients(&self) -> Result<Vec<ClientInfo>> {
        self.query(Model::clients)
    }

    fn get_monitors(&self) -> Result<Vec<MonitorInfo>> {
        self.query(Model::monitors)
    }

    fn select_primary(&self, monitor: &str, id: u32) -> Result<()> {
        self.command(Intent::Activate { monitor: monitor.to_string(), number: id })
    }

    fn toggle_secondary(&self, monitor: &str, id: u32) -> Result<()> {
        // No composite views: a workspace switch.
        self.select_primary(monitor, id)
    }

    fn move_client_to(&self, client_id: Option<u64>, id: u32) -> Result<()> {
        self.command(Intent::Move { client: client_id, number: id })
    }

    fn toggle_client_tag(&self, client_id: Option<u64>, tag: u32) -> Result<()> {
        self.move_client_to(client_id, tag)
    }

    fn cycle_layout(&self, _monitor: &str) -> Result<()> {
        // Aqueous's IPC has no layout command (layouts stay in its own
        // files and `aqueousctl layout`).
        bail!("{}", mandara_i18n::t("Not supported by this compositor"))
    }

    /// `cmd` is an Aqueous action (`overview.toggle`, `window.close`,
    /// `keyboard.next`, …); `args` is empty or one JSON object of its
    /// fields. Overview actions default to the selected output and window
    /// actions to the focused window.
    fn dispatch_raw(&self, cmd: &str, args: &[&str]) -> Result<()> {
        if cmd.is_empty() || !cmd.bytes().all(|b| b.is_ascii_lowercase() || b == b'.' || b == b'_') {
            bail!("aqueous: {cmd:?} is not an action name");
        }
        let fields = match args {
            [] => Map::new(),
            [one] => serde_json::from_str(one).context("aqueous: the fields are not a JSON object")?,
            _ => bail!("aqueous: an action takes one JSON object of fields"),
        };
        self.command(Intent::Raw { action: cmd.to_string(), fields })
    }

    fn reload_config(&self) -> Result<()> {
        // Mandara writes nothing Aqueous reads; it reloads its own files.
        Ok(())
    }

    fn switch_keyboard_layout(&self) -> Result<()> {
        self.command(Intent::Raw { action: "keyboard.next".into(), fields: Map::new() })
    }

    fn sync_managed_config(&self, _config: &mandara_config::compositor::CompositorConfig, _borders: Option<mandara_config::compositor::BorderColors>) -> Result<(PathBuf, mandara_config::compositor::Emitted)> {
        bail!("{}", mandara_i18n::t(mandara_config::compositor::AQUEOUS_SETTINGS))
    }
}

#[cfg(test)]
mod tests;
