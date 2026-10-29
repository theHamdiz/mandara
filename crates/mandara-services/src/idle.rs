//! Idle detection and display power over Wayland.
//!
//! One small client of its own (like the clipboard and lock screen) on a
//! dedicated thread:
//!
//! * **`ext-idle-notify-v1`** — one `ext_idle_notification_v1` per
//!   [`IdleTimer`]; the compositor sends `idled` after the timeout without
//!   input and `resumed` on the next input. Plain notifications honour
//!   apps' idle inhibitors (a playing video); timers with
//!   `ignore_inhibitors` use `get_input_idle_notification` (protocol v2)
//!   when the compositor has it.
//! * **`wlr-output-power-management-unstable-v1`** — turns outputs off and
//!   on (DPMS). When the compositor lacks it, [`IdleEvent::Screens`]
//!   reports `ok: false` and the caller falls back to compositor IPC.
//!
//! Setting new timers replaces every notification (each one's clock
//! restarts from "now", which is exactly what an inhibitor ending or a
//! power-source change needs).

use std::os::fd::{AsFd as _, OwnedFd};
use std::time::Duration;

use serde::Serialize;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_output, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, Proxy as _, QueueHandle, WEnum};
use wayland_protocols::ext::idle_notify::v1::client::{ext_idle_notification_v1, ext_idle_notifier_v1};
use wayland_protocols_wlr::output_power_management::v1::client::{zwlr_output_power_manager_v1, zwlr_output_power_v1};

/// `ext_idle_notifier_v1` version with `get_input_idle_notification`.
const INPUT_IDLE_VERSION: u32 = 2;
/// Events buffered for the consumer.
const EVENT_QUEUE: usize = 64;
/// Commands buffered from the consumer.
const COMMAND_QUEUE: usize = 16;

/// One idle timeout to watch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct IdleTimer {
    /// Caller-chosen id echoed in [`IdleEvent`]s.
    pub id: u32,
    pub timeout: Duration,
    /// Fire even while an app inhibits idle (protocol v2; else inhibited
    /// like the rest).
    pub ignore_inhibitors: bool,
}

/// What the compositor offers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct IdleCapabilities {
    pub idle_notify: bool,
    /// `get_input_idle_notification` (ext-idle-notify v2).
    pub input_idle: bool,
    pub output_power: bool,
    pub outputs: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdleEvent {
    /// Connected; what is available.
    Ready(IdleCapabilities),
    Idled(u32),
    Resumed(u32),
    /// Result of [`IdleHandle::set_screens`]: `ok` is false when no output
    /// could be switched through wlr-output-power-management.
    Screens { on: bool, ok: bool },
    /// The connection failed or the protocol is missing.
    Failed(String),
}

enum Command {
    Timers(Vec<IdleTimer>),
    Screens(bool),
}

/// Handle to the idle thread; it exits when every handle is dropped.
#[derive(Clone)]
pub struct IdleHandle {
    commands: async_channel::Sender<Command>,
    wake: Option<std::sync::Arc<OwnedFd>>,
}

impl IdleHandle {
    fn send(&self, command: Command) {
        if self.commands.try_send(command).is_ok()
            && let Some(wake) = &self.wake
        {
            let _ = rustix::io::write(wake.as_fd(), &1u64.to_ne_bytes());
        }
    }

    /// Replaces every watched timeout.
    pub fn set_timers(&self, timers: Vec<IdleTimer>) {
        self.send(Command::Timers(timers));
    }

    /// Turns every output off (`false`) or on.
    pub fn set_screens(&self, on: bool) {
        self.send(Command::Screens(on));
    }
}

impl Drop for IdleHandle {
    fn drop(&mut self) {
        // The last handle: wake the thread so it notices the closed channel.
        // Two references remain then: this handle's and the thread's.
        if let Some(wake) = &self.wake
            && std::sync::Arc::strong_count(wake) <= 2
        {
            self.commands.close();
            let _ = rustix::io::write(wake.as_fd(), &1u64.to_ne_bytes());
        }
    }
}

struct Output {
    global: u32,
    output: wl_output::WlOutput,
    power: Option<zwlr_output_power_v1::ZwlrOutputPowerV1>,
}

struct State {
    seat: Option<wl_seat::WlSeat>,
    notifier: Option<ext_idle_notifier_v1::ExtIdleNotifierV1>,
    power: Option<zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1>,
    outputs: Vec<Output>,
    notifications: Vec<ext_idle_notification_v1::ExtIdleNotificationV1>,
    events: async_channel::Sender<IdleEvent>,
}

impl State {
    fn emit(&self, event: IdleEvent) {
        let _ = self.events.try_send(event);
    }

    fn set_timers(&mut self, timers: &[IdleTimer], qh: &QueueHandle<Self>) {
        for n in self.notifications.drain(..) {
            n.destroy();
        }
        let (Some(notifier), Some(seat)) = (&self.notifier, &self.seat) else { return };
        for t in timers {
            let ms = u32::try_from(t.timeout.as_millis()).unwrap_or(u32::MAX).max(1);
            let n = if t.ignore_inhibitors && notifier.version() >= INPUT_IDLE_VERSION {
                notifier.get_input_idle_notification(ms, seat, qh, t.id)
            } else {
                notifier.get_idle_notification(ms, seat, qh, t.id)
            };
            self.notifications.push(n);
        }
    }

    fn set_screens(&mut self, on: bool, qh: &QueueHandle<Self>) -> bool {
        let Some(manager) = &self.power else { return false };
        let mode = if on { zwlr_output_power_v1::Mode::On } else { zwlr_output_power_v1::Mode::Off };
        for out in &mut self.outputs {
            let power = out.power.get_or_insert_with(|| manager.get_output_power(&out.output, qh, out.global));
            power.set_mode(mode);
        }
        !self.outputs.is_empty()
    }
}

/// Starts the idle thread. Timers start empty: call
/// [`IdleHandle::set_timers`].
pub fn spawn() -> (IdleHandle, async_channel::Receiver<IdleEvent>) {
    let (events, rx) = async_channel::bounded(EVENT_QUEUE);
    let (commands, command_rx) = async_channel::bounded(COMMAND_QUEUE);
    let wake = match rustix::event::eventfd(0, rustix::event::EventfdFlags::CLOEXEC | rustix::event::EventfdFlags::NONBLOCK) {
        Ok(fd) => std::sync::Arc::new(fd),
        Err(e) => {
            // Without a wake-up fd the thread can't be driven: report it and
            // hand out a handle whose commands go nowhere.
            let _ = events.try_send(IdleEvent::Failed(e.to_string()));
            commands.close();
            return (IdleHandle { commands, wake: None }, rx);
        }
    };
    let thread_wake = wake.clone();
    let wake = Some(wake);
    let failed = events.clone();
    let spawned = std::thread::Builder::new().name("mandara-idle".into()).spawn(move || {
        if let Err(e) = run(events, command_rx, &thread_wake) {
            tracing::warn!("idle: {e:#}");
            let _ = failed.try_send(IdleEvent::Failed(format!("{e:#}")));
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("idle: cannot start: {e}");
    }
    (IdleHandle { commands, wake }, rx)
}

fn run(events: async_channel::Sender<IdleEvent>, commands: async_channel::Receiver<Command>, wake: &OwnedFd) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let conn = Connection::connect_to_env().context("connecting to the Wayland compositor")?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).context("reading Wayland globals")?;
    let qh = queue.handle();
    let mut state = State { seat: None, notifier: None, power: None, outputs: Vec::new(), notifications: Vec::new(), events };
    state.notifier = globals.bind(&qh, 1..=INPUT_IDLE_VERSION, ()).ok();
    state.power = globals.bind(&qh, 1..=1, ()).ok();
    state.seat = globals.bind(&qh, 1..=7, ()).ok();
    globals.contents().with_list(|list| {
        for g in list.iter().filter(|g| g.interface == "wl_output") {
            let output = globals.registry().bind::<wl_output::WlOutput, _, _>(g.name, g.version.min(4), &qh, ());
            state.outputs.push(Output { global: g.name, output, power: None });
        }
    });
    let caps = IdleCapabilities {
        idle_notify: state.notifier.is_some() && state.seat.is_some(),
        input_idle: state.notifier.as_ref().is_some_and(|n| n.version() >= INPUT_IDLE_VERSION),
        output_power: state.power.is_some(),
        outputs: state.outputs.len(),
    };
    state.emit(IdleEvent::Ready(caps));
    if !caps.idle_notify {
        tracing::warn!("idle: the compositor has no ext-idle-notify-v1; idle stages are unavailable");
    }
    loop {
        // Commands first (the eventfd only says "something arrived").
        let mut buf = [0u8; 8];
        let _ = rustix::io::read(wake.as_fd(), &mut buf);
        loop {
            match commands.try_recv() {
                Ok(Command::Timers(timers)) => state.set_timers(&timers, &qh),
                Ok(Command::Screens(on)) => {
                    let ok = state.set_screens(on, &qh);
                    state.emit(IdleEvent::Screens { on, ok });
                }
                Err(async_channel::TryRecvError::Empty) => break,
                Err(async_channel::TryRecvError::Closed) => {
                    for n in state.notifications.drain(..) {
                        n.destroy();
                    }
                    let _ = conn.flush();
                    return Ok(());
                }
            }
        }
        queue.dispatch_pending(&mut state).context("Wayland dispatch")?;
        conn.flush().context("Wayland flush")?;
        let Some(guard) = queue.prepare_read() else { continue };
        let fd = guard.connection_fd();
        let mut fds = [rustix::event::PollFd::new(&fd, rustix::event::PollFlags::IN), rustix::event::PollFd::new(wake, rustix::event::PollFlags::IN)];
        match rustix::event::poll(&mut fds, None) {
            Ok(_) if fds[0].revents().contains(rustix::event::PollFlags::IN) => {
                if let Err(err) = guard.read()
                    && !matches!(err, wayland_client::backend::WaylandError::Io(ref e) if e.kind() == std::io::ErrorKind::WouldBlock)
                {
                    return Err(err).context("reading Wayland events");
                }
            }
            Ok(_) => drop(guard),
            Err(rustix::io::Errno::INTR) => drop(guard),
            Err(err) => return Err(err).context("poll"),
        }
        queue.dispatch_pending(&mut state).context("Wayland dispatch")?;
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(state: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &GlobalListContents, _: &Connection, qh: &QueueHandle<Self>) {
        match event {
            wl_registry::Event::Global { name, interface, version } if interface == "wl_output" => {
                let output = registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ());
                state.outputs.push(Output { global: name, output, power: None });
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(i) = state.outputs.iter().position(|o| o.global == name) {
                    let out = state.outputs.remove(i);
                    if let Some(p) = out.power {
                        p.destroy();
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ext_idle_notification_v1::ExtIdleNotificationV1, u32> for State {
    fn event(state: &mut Self, _: &ext_idle_notification_v1::ExtIdleNotificationV1, event: ext_idle_notification_v1::Event, id: &u32, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            ext_idle_notification_v1::Event::Idled => state.emit(IdleEvent::Idled(*id)),
            ext_idle_notification_v1::Event::Resumed => state.emit(IdleEvent::Resumed(*id)),
            _ => {}
        }
    }
}

impl Dispatch<zwlr_output_power_v1::ZwlrOutputPowerV1, u32> for State {
    fn event(state: &mut Self, power: &zwlr_output_power_v1::ZwlrOutputPowerV1, event: zwlr_output_power_v1::Event, global: &u32, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_output_power_v1::Event::Failed => {
                power.destroy();
                if let Some(out) = state.outputs.iter_mut().find(|o| o.global == *global) {
                    out.power = None;
                }
            }
            zwlr_output_power_v1::Event::Mode { mode: WEnum::Value(mode) } => tracing::debug!("idle: output {global} power {mode:?}"),
            _ => {}
        }
    }
}

macro_rules! ignore_events {
    ($($ty:ty),* $(,)?) => {$(
        impl Dispatch<$ty, ()> for State {
            fn event(_: &mut Self, _: &$ty, _: <$ty as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}

ignore_events!(wl_output::WlOutput, wl_seat::WlSeat, ext_idle_notifier_v1::ExtIdleNotifierV1, zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1);

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs only inside a Wayland session; never turns screens off and uses
    /// an hour-long timeout so nothing fires.
    #[test]
    fn binds_globals_without_side_effects() {
        if std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return;
        }
        let (handle, events) = spawn();
        let ready = events.recv_blocking().expect("idle thread event");
        match ready {
            IdleEvent::Ready(caps) => {
                eprintln!("idle capabilities: {caps:?}");
                assert!(caps.outputs > 0, "{caps:?}");
                handle.set_timers(vec![IdleTimer { id: 1, timeout: Duration::from_secs(3600), ignore_inhibitors: false }]);
            }
            IdleEvent::Failed(e) => panic!("{e}"),
            other => panic!("unexpected {other:?}"),
        }
        drop(handle);
    }
}
