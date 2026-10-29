//! Open windows, compositor-neutrally, over `zwlr_foreign_toplevel_manager_v1`
//! (MangoWM, Hyprland, Niri, Sway…): app id, title, state and outputs of
//! every toplevel, kept in most-recently-used order (the order windows were
//! last activated), plus activate / close / minimize / maximize /
//! fullscreen requests for the dock and the Alt-Tab switcher.
//!
//! The same connection carries an `ext_idle_notifier_v1` notification so
//! screen time can stop counting while you are away.

use std::sync::Mutex;
use std::sync::mpsc;

use serde::Serialize;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_output, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};
use wayland_protocols::ext::idle_notify::v1::client::{ext_idle_notification_v1, ext_idle_notifier_v1};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{zwlr_foreign_toplevel_handle_v1 as handle, zwlr_foreign_toplevel_manager_v1 as manager};

use crate::actor::{Envelope, ServiceHandle, reply};

/// Highest protocol versions used.
const MANAGER_VERSION: u32 = 3;
const OUTPUT_VERSION: u32 = 4;
const SEAT_VERSION: u32 = 8;
const IDLE_VERSION: u32 = 1;
/// Command queue between the UI and the Wayland thread.
const COMMAND_QUEUE: usize = 32;

/// One open window.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Toplevel {
    /// Stable for the window's lifetime (the protocol object id).
    pub id: u64,
    pub app_id: String,
    pub title: String,
    pub activated: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    /// Names of the outputs the window is on.
    pub outputs: Vec<String>,
    /// The window this one is a dialog of.
    pub parent: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ToplevelsState {
    /// The compositor offers foreign-toplevel management.
    pub available: bool,
    /// Most recently activated first.
    pub toplevels: Vec<Toplevel>,
    /// No input for the configured idle timeout (see
    /// [`ToplevelCommand::IdleTimeout`]).
    pub idle: bool,
}

impl ToplevelsState {
    pub fn get(&self, id: u64) -> Option<&Toplevel> {
        self.toplevels.iter().find(|t| t.id == id)
    }

    /// The focused window.
    pub fn active(&self) -> Option<&Toplevel> {
        self.toplevels.iter().find(|t| t.activated)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToplevelCommand {
    Activate(u64),
    Close(u64),
    SetMinimized(u64, bool),
    SetMaximized(u64, bool),
    SetFullscreen(u64, bool),
    /// Report idle after this many milliseconds without input (0 = off).
    IdleTimeout(u32),
}

pub type ToplevelService = ServiceHandle<ToplevelsState, ToplevelCommand>;

/// Moves `id` to the front of the most-recently-used order.
pub fn touch(order: &mut Vec<u64>, id: u64) {
    order.retain(|x| *x != id);
    order.insert(0, id);
}

/// Decodes a `state` array (native-endian `u32` values).
pub fn decode_states(raw: &[u8]) -> Vec<u32> {
    raw.chunks_exact(4).map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// Per-handle data accumulated until `done`.
#[derive(Default)]
struct Pending {
    app_id: String,
    title: String,
    states: Vec<u32>,
    outputs: Vec<wl_output::WlOutput>,
    parent: Option<u64>,
}

struct State {
    seat: Option<wl_seat::WlSeat>,
    handles: Vec<(handle::ZwlrForeignToplevelHandleV1, Toplevel)>,
    order: Vec<u64>,
    idle: bool,
    dirty: bool,
}

impl State {
    fn handle(&self, id: u64) -> Option<&handle::ZwlrForeignToplevelHandleV1> {
        self.handles.iter().find(|(_, t)| t.id == id).map(|(h, _)| h)
    }

    fn snapshot(&self) -> ToplevelsState {
        let mut toplevels: Vec<Toplevel> = self.handles.iter().map(|(_, t)| t.clone()).collect();
        let rank = |id: u64| self.order.iter().position(|x| *x == id).unwrap_or(usize::MAX);
        toplevels.sort_by_key(|t| rank(t.id));
        ToplevelsState { available: true, toplevels, idle: self.idle }
    }
}

fn output_name(output: &wl_output::WlOutput) -> String {
    output.data::<Mutex<String>>().and_then(|n| n.lock().ok().map(|n| n.clone())).unwrap_or_default()
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &GlobalListContents, _: &Connection, qh: &QueueHandle<Self>) {
        // Outputs plugged in later.
        if let wl_registry::Event::Global { name, interface, version } = event
            && interface == wl_output::WlOutput::interface().name
        {
            registry.bind::<wl_output::WlOutput, _, _>(name, version.min(OUTPUT_VERSION), qh, Mutex::new(String::new()));
        }
    }
}

impl Dispatch<wl_output::WlOutput, Mutex<String>> for State {
    fn event(_: &mut Self, _: &wl_output::WlOutput, event: wl_output::Event, name: &Mutex<String>, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_output::Event::Name { name: n } = event
            && let Ok(mut slot) = name.lock()
        {
            *slot = n;
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<manager::ZwlrForeignToplevelManagerV1, ()> for State {
    fn event(state: &mut Self, _: &manager::ZwlrForeignToplevelManagerV1, event: manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let manager::Event::Toplevel { toplevel } = event {
            let id = u64::from(toplevel.id().protocol_id());
            state.handles.push((toplevel, Toplevel { id, ..Default::default() }));
            // New windows go to the back until they are activated.
            state.order.push(id);
        }
    }

    event_created_child!(State, manager::ZwlrForeignToplevelManagerV1, [
        manager::EVT_TOPLEVEL_OPCODE => (handle::ZwlrForeignToplevelHandleV1, Mutex::new(Pending::default())),
    ]);
}

impl Dispatch<handle::ZwlrForeignToplevelHandleV1, Mutex<Pending>> for State {
    fn event(state: &mut Self, proxy: &handle::ZwlrForeignToplevelHandleV1, event: handle::Event, pending: &Mutex<Pending>, _: &Connection, _: &QueueHandle<Self>) {
        let id = u64::from(proxy.id().protocol_id());
        let Ok(mut p) = pending.lock() else { return };
        match event {
            handle::Event::Title { title } => p.title = title,
            handle::Event::AppId { app_id } => p.app_id = app_id,
            handle::Event::OutputEnter { output } => {
                if !p.outputs.contains(&output) {
                    p.outputs.push(output);
                }
            }
            handle::Event::OutputLeave { output } => p.outputs.retain(|o| *o != output),
            handle::Event::State { state: raw } => p.states = decode_states(&raw),
            handle::Event::Parent { parent } => p.parent = parent.map(|h| u64::from(h.id().protocol_id())),
            handle::Event::Done => {
                let has = |s: handle::State| p.states.contains(&(s as u32));
                let next = Toplevel {
                    id,
                    app_id: p.app_id.clone(),
                    title: p.title.clone(),
                    activated: has(handle::State::Activated),
                    minimized: has(handle::State::Minimized),
                    maximized: has(handle::State::Maximized),
                    fullscreen: has(handle::State::Fullscreen),
                    outputs: p.outputs.iter().map(output_name).filter(|n| !n.is_empty()).collect(),
                    parent: p.parent,
                };
                if next.activated {
                    touch(&mut state.order, id);
                }
                if let Some((_, slot)) = state.handles.iter_mut().find(|(_, t)| t.id == id)
                    && *slot != next
                {
                    *slot = next;
                    state.dirty = true;
                }
            }
            handle::Event::Closed => {
                state.handles.retain(|(_, t)| t.id != id);
                state.order.retain(|x| *x != id);
                proxy.destroy();
                state.dirty = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<ext_idle_notifier_v1::ExtIdleNotifierV1, ()> for State {
    fn event(_: &mut Self, _: &ext_idle_notifier_v1::ExtIdleNotifierV1, _: ext_idle_notifier_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ext_idle_notification_v1::ExtIdleNotificationV1, ()> for State {
    fn event(state: &mut Self, _: &ext_idle_notification_v1::ExtIdleNotificationV1, event: ext_idle_notification_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let idle = matches!(event, ext_idle_notification_v1::Event::Idled);
        if state.idle != idle {
            state.idle = idle;
            state.dirty = true;
        }
    }
}

/// Carries out one command on the Wayland thread.
fn apply(state: &State, command: &ToplevelCommand) -> Result<String, String> {
    let need = |id: u64| state.handle(id).ok_or_else(|| mandara_i18n::tf("No window {id}", &[("id", &id)]));
    match *command {
        ToplevelCommand::Activate(id) => {
            let seat = state.seat.as_ref().ok_or("no seat")?;
            let h = need(id)?;
            // Bring a minimized window back first.
            h.unset_minimized();
            h.activate(seat);
            Ok("activated".into())
        }
        ToplevelCommand::Close(id) => {
            need(id)?.close();
            Ok("closed".into())
        }
        ToplevelCommand::SetMinimized(id, on) => {
            let h = need(id)?;
            if on { h.set_minimized() } else { h.unset_minimized() }
            Ok(if on { "minimized".into() } else { "restored".into() })
        }
        ToplevelCommand::SetMaximized(id, on) => {
            let h = need(id)?;
            if on { h.set_maximized() } else { h.unset_maximized() }
            Ok(if on { "maximized".into() } else { "restored".into() })
        }
        ToplevelCommand::SetFullscreen(id, on) => {
            let h = need(id)?;
            if h.version() < 2 {
                return Err("the compositor can't toggle fullscreen".into());
            }
            if on { h.set_fullscreen(None) } else { h.unset_fullscreen() }
            Ok(if on { "fullscreen".into() } else { "restored".into() })
        }
        ToplevelCommand::IdleTimeout(_) => Ok("ok".into()),
    }
}

/// Runs the Wayland side until the connection drops.
fn run(updates: async_channel::Sender<ToplevelsState>, commands: mpsc::Receiver<Envelope<ToplevelCommand>>, wake: std::io::PipeReader) -> Result<(), String> {
    use rustix::event::{PollFd, PollFlags, poll};
    use std::io::Read as _;
    let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let seat: Option<wl_seat::WlSeat> = globals.bind(&qh, 1..=SEAT_VERSION, ()).ok();
    for g in globals.contents().clone_list() {
        if g.interface == wl_output::WlOutput::interface().name {
            globals.registry().bind::<wl_output::WlOutput, _, _>(g.name, g.version.min(OUTPUT_VERSION), &qh, Mutex::new(String::new()));
        }
    }
    let _manager: manager::ZwlrForeignToplevelManagerV1 = match globals.bind(&qh, 1..=MANAGER_VERSION, ()) {
        Ok(m) => m,
        Err(err) => {
            let _ = updates.send_blocking(ToplevelsState::default());
            return Err(format!("zwlr_foreign_toplevel_manager_v1: {err}"));
        }
    };
    let notifier: Option<ext_idle_notifier_v1::ExtIdleNotifierV1> = globals.bind(&qh, 1..=IDLE_VERSION, ()).ok();
    let mut idle_note: Option<ext_idle_notification_v1::ExtIdleNotificationV1> = None;
    let mut state = State { seat, handles: Vec::new(), order: Vec::new(), idle: false, dirty: true };
    queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
    let mut wake_buf = [0u8; 64];
    loop {
        queue.dispatch_pending(&mut state).map_err(|e| e.to_string())?;
        // Commands queued by the UI.
        while let Ok(Envelope { command, reply: r }) = commands.try_recv() {
            if let ToplevelCommand::IdleTimeout(ms) = command {
                if let Some(old) = idle_note.take() {
                    old.destroy();
                }
                if state.idle {
                    state.idle = false;
                    state.dirty = true;
                }
                if let (Some(n), Some(seat), true) = (&notifier, &state.seat, ms > 0) {
                    idle_note = Some(n.get_idle_notification(ms, seat, &qh, ()));
                }
            }
            reply(&r, apply(&state, &command));
        }
        conn.flush().map_err(|e| e.to_string())?;
        if std::mem::take(&mut state.dirty) && updates.send_blocking(state.snapshot()).is_err() {
            return Ok(());
        }
        let Some(guard) = queue.prepare_read() else { continue };
        let (wl_ready, wake_ready) = {
            let wl_fd = guard.connection_fd();
            let mut fds = [PollFd::new(&wl_fd, PollFlags::IN), PollFd::new(&wake, PollFlags::IN)];
            match poll(&mut fds, None) {
                Ok(_) => (fds[0].revents().contains(PollFlags::IN), fds[1].revents().contains(PollFlags::IN)),
                Err(rustix::io::Errno::INTR) => (false, false),
                Err(e) => return Err(e.to_string()),
            }
        };
        if wl_ready {
            guard.read().map_err(|e| e.to_string())?;
        } else {
            drop(guard);
        }
        if wake_ready {
            let _ = (&wake).read(&mut wake_buf);
        }
    }
}

/// Starts the toplevel tracker on its own thread.
pub fn spawn_toplevel_service() -> ToplevelService {
    use std::io::Write as _;
    let (tx, updates) = async_channel::unbounded();
    let (commands, rx) = async_channel::bounded::<Envelope<ToplevelCommand>>(COMMAND_QUEUE);
    let (cmd_tx, cmd_rx) = mpsc::channel();
    match std::io::pipe() {
        Ok((wake_rx, mut wake_tx)) => {
            let _ = std::thread::Builder::new().name("mandara-toplevels".into()).spawn(move || {
                if let Err(err) = run(tx, cmd_rx, wake_rx) {
                    tracing::warn!("window list: {err}");
                }
            });
            // Bridge the async command queue to the Wayland thread and wake it.
            crate::runtime::spawn("toplevels", async move {
                while let Ok(envelope) = rx.recv().await {
                    if cmd_tx.send(envelope).is_err() {
                        break;
                    }
                    let _ = wake_tx.write(&[1]);
                }
            });
        }
        Err(err) => tracing::warn!("window list: {err}"),
    }
    ServiceHandle::new(updates, commands)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mru_order_moves_activated_to_front() {
        let mut order = vec![1, 2, 3];
        touch(&mut order, 3);
        assert_eq!(order, [3, 1, 2]);
        touch(&mut order, 9);
        assert_eq!(order, [9, 3, 1, 2]);
    }

    #[test]
    fn decodes_state_arrays() {
        let raw: Vec<u8> = [2u32, 0].iter().flat_map(|v| v.to_ne_bytes()).collect();
        assert_eq!(decode_states(&raw), [2, 0]);
        assert!(decode_states(&[1, 2, 3]).is_empty());
    }

    #[test]
    fn state_lookups() {
        let s = ToplevelsState {
            available: true,
            toplevels: vec![Toplevel { id: 4, activated: true, ..Default::default() }, Toplevel { id: 7, ..Default::default() }],
            idle: false,
        };
        assert_eq!(s.active().map(|t| t.id), Some(4));
        assert!(s.get(7).is_some() && s.get(8).is_none());
    }
}
