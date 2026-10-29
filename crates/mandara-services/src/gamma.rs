//! Screen colour temperature over `wlr-gamma-control-unstable-v1`
//! (MangoWM, Hyprland, Sway and other wlroots compositors; Niri doesn't
//! implement it).
//!
//! One thread owns its own Wayland connection, binds every `wl_output`
//! (following hotplug), and eases the ramps toward the requested
//! [`GammaTarget`]. It sleeps in `poll(2)` between commands and only ticks
//! while a fade runs. Once the target is neutral again the gamma controls
//! are released, which restores the compositor's original ramps (and frees
//! them for other tools).

use std::io::{Seek as _, Write as _};
use std::os::fd::{AsFd as _, OwnedFd};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::gamma_control::v1::client::{zwlr_gamma_control_manager_v1 as manager, zwlr_gamma_control_v1 as control};

use crate::night_light::{GammaTarget, ease, fill_ramp};

/// Fade frame interval (≈ 30 fps is smooth for colour; ramps are cheap).
const FADE_FRAME: Duration = Duration::from_millis(33);
/// `wl_output` version bound (v4 adds the connector name event).
const WL_OUTPUT_VERSION: u32 = 4;
/// Upper bound on a gamma table, entries per channel (guards a bogus size).
const MAX_RAMP_SIZE: u32 = 1 << 16;

/// Whether the compositor can do night light.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GammaSupport {
    #[default]
    Unknown,
    Supported,
    /// No `zwlr_gamma_control_manager_v1` (Niri, GNOME, KDE).
    Unsupported,
    /// No Wayland display reachable.
    NoDisplay,
}

/// Live state of the gamma thread.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct GammaStatus {
    pub support: GammaSupport,
    /// Outputs currently tinted.
    pub outputs: Vec<String>,
    /// Outputs whose gamma another client holds (wlsunset, gammastep,
    /// hyprsunset…).
    pub busy: Vec<String>,
    /// The ramp shown now (mid-fade values included).
    pub current: Option<GammaTarget>,
}

enum Cmd {
    Set(GammaTarget, Duration),
    Quit,
}

/// Handle to the gamma thread; dropping it restores the screens.
pub struct Gamma {
    tx: mpsc::Sender<Cmd>,
    wake: Arc<OwnedFd>,
    status: Arc<Mutex<GammaStatus>>,
}

impl Gamma {
    /// Starts the thread (cheap: it sleeps until told to tint).
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel();
        let wake = Arc::new(rustix::event::eventfd(0, rustix::event::EventfdFlags::CLOEXEC | rustix::event::EventfdFlags::NONBLOCK).expect("eventfd"));
        let status = Arc::new(Mutex::new(GammaStatus::default()));
        let (w, s) = (wake.clone(), status.clone());
        let _ = std::thread::Builder::new().name("mandara-gamma".into()).spawn(move || {
            if let Err(err) = run(rx, w, s.clone()) {
                tracing::warn!("night light: {err:#}");
                let mut st = s.lock();
                if st.support == GammaSupport::Unknown {
                    st.support = GammaSupport::NoDisplay;
                }
            }
        });
        Self { tx, wake, status }
    }

    /// Eases every output to `target` over `fade` (zero = at once).
    pub fn set(&self, target: GammaTarget, fade: Duration) {
        let _ = self.tx.send(Cmd::Set(target, fade));
        self.poke();
    }

    pub fn status(&self) -> GammaStatus {
        self.status.lock().clone()
    }

    fn poke(&self) {
        let _ = rustix::io::write(self.wake.as_fd(), &1u64.to_ne_bytes());
    }
}

impl Drop for Gamma {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Quit);
        self.poke();
    }
}

struct Output {
    global: u32,
    proxy: wl_output::WlOutput,
    name: String,
    control: Option<control::ZwlrGammaControlV1>,
    /// Entries per channel, once the compositor reported it.
    size: Option<u32>,
    /// Another client owns this output's gamma.
    busy: bool,
}

struct State {
    outputs: Vec<Output>,
    manager: Option<manager::ZwlrGammaControlManagerV1>,
    /// Outputs whose ramp must be (re)sent.
    dirty: bool,
}

impl State {
    fn add_output(&mut self, registry: &wl_registry::WlRegistry, global: u32, version: u32, qh: &QueueHandle<Self>) {
        let proxy: wl_output::WlOutput = registry.bind(global, version.min(WL_OUTPUT_VERSION), qh, global);
        self.outputs.push(Output { global, proxy, name: format!("output-{global}"), control: None, size: None, busy: false });
        self.dirty = true;
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(state: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &GlobalListContents, _: &Connection, qh: &QueueHandle<Self>) {
        match event {
            wl_registry::Event::Global { name, interface, version } if interface == wl_output::WlOutput::interface().name => state.add_output(registry, name, version, qh),
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(i) = state.outputs.iter().position(|o| o.global == name) {
                    let o = state.outputs.remove(i);
                    if let Some(c) = o.control {
                        c.destroy();
                    }
                    if o.proxy.version() >= 3 {
                        o.proxy.release();
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(state: &mut Self, _: &wl_output::WlOutput, event: wl_output::Event, global: &u32, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_output::Event::Name { name } = event
            && let Some(o) = state.outputs.iter_mut().find(|o| o.global == *global)
        {
            o.name = name;
        }
    }
}

impl Dispatch<manager::ZwlrGammaControlManagerV1, ()> for State {
    fn event(_: &mut Self, _: &manager::ZwlrGammaControlManagerV1, _: manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<control::ZwlrGammaControlV1, u32> for State {
    fn event(state: &mut Self, proxy: &control::ZwlrGammaControlV1, event: control::Event, global: &u32, _: &Connection, _: &QueueHandle<Self>) {
        let Some(o) = state.outputs.iter_mut().find(|o| o.global == *global) else { return };
        match event {
            control::Event::GammaSize { size } => {
                o.size = Some(size.min(MAX_RAMP_SIZE));
                state.dirty = true;
            }
            control::Event::Failed => {
                tracing::warn!("night light: gamma of {} is held by another app (or the output is gone)", o.name);
                proxy.destroy();
                o.control = None;
                o.size = None;
                o.busy = true;
            }
            _ => {}
        }
    }
}

/// An in-flight fade.
struct Fade {
    from: GammaTarget,
    to: GammaTarget,
    start: Instant,
    duration: Duration,
}

impl Fade {
    fn value(&self, now: Instant) -> (GammaTarget, bool) {
        if self.duration.is_zero() {
            return (self.to, true);
        }
        let t = now.duration_since(self.start).as_secs_f32() / self.duration.as_secs_f32();
        if t >= 1.0 { (self.to, true) } else { (self.from.lerp(self.to, ease(t)), false) }
    }
}

/// Writes one ramp into a memfd and hands it to the compositor.
fn send_ramp(control: &control::ZwlrGammaControlV1, size: u32, target: GammaTarget, scratch: &mut Vec<u16>) -> std::io::Result<()> {
    fill_ramp(size as usize, target, scratch);
    let fd = rustix::fs::memfd_create("mandara-gamma", rustix::fs::MemfdFlags::CLOEXEC)?;
    let mut file = std::fs::File::from(fd);
    let bytes: Vec<u8> = scratch.iter().flat_map(|v| v.to_ne_bytes()).collect();
    file.write_all(&bytes)?;
    file.rewind()?;
    control.set_gamma(file.as_fd());
    Ok(())
}

fn run(rx: mpsc::Receiver<Cmd>, wake: Arc<OwnedFd>, status: Arc<Mutex<GammaStatus>>) -> anyhow::Result<()> {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();
    let manager: Option<manager::ZwlrGammaControlManagerV1> = globals.bind(&qh, 1..=1, ()).ok();
    let mut state = State { outputs: Vec::new(), manager, dirty: false };
    status.lock().support = if state.manager.is_some() { GammaSupport::Supported } else { GammaSupport::Unsupported };
    let outputs: Vec<_> = globals.contents().with_list(|l| l.iter().filter(|g| g.interface == wl_output::WlOutput::interface().name).map(|g| (g.name, g.version)).collect());
    for (global, version) in outputs {
        state.add_output(globals.registry(), global, version, &qh);
    }
    queue.roundtrip(&mut state)?;

    let mut current = GammaTarget::NEUTRAL;
    let mut fade: Option<Fade> = None;
    let mut scratch = Vec::new();
    loop {
        // Commands (the newest target wins; fades start from what shows now).
        let mut quit = false;
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                Cmd::Set(to, duration) => {
                    if to != current || fade.as_ref().is_some_and(|f| f.to != to) {
                        fade = Some(Fade { from: current, to, start: Instant::now(), duration });
                    }
                }
                Cmd::Quit => quit = true,
            }
        }
        if quit {
            fade = Some(Fade { from: current, to: GammaTarget::NEUTRAL, start: Instant::now(), duration: Duration::ZERO });
        }
        if let Some(f) = &fade {
            let (value, done) = f.value(Instant::now());
            if value != current {
                current = value;
                state.dirty = true;
            }
            if done {
                fade = None;
            }
        }
        if state.dirty {
            state.dirty = false;
            apply(&mut state, current, &qh, &mut scratch);
            let mut st = status.lock();
            st.outputs = state.outputs.iter().filter(|o| o.control.is_some()).map(|o| o.name.clone()).collect();
            st.busy = state.outputs.iter().filter(|o| o.busy).map(|o| o.name.clone()).collect();
            st.current = (!current.is_neutral()).then_some(current);
        }
        conn.flush()?;
        if quit {
            return Ok(());
        }
        queue.dispatch_pending(&mut state)?;
        let Some(guard) = queue.prepare_read() else { continue };
        let (wl_ready, _) = {
            let wl_fd = guard.connection_fd();
            let mut fds = [PollFd::new(&wl_fd, PollFlags::IN), PollFd::new(wake.as_ref(), PollFlags::IN)];
            let timeout = fade.as_ref().map(|_| Timespec { tv_sec: 0, tv_nsec: FADE_FRAME.as_nanos() as i64 });
            match poll(&mut fds, timeout.as_ref()) {
                Ok(_) | Err(rustix::io::Errno::INTR) => {}
                Err(e) => return Err(e.into()),
            }
            (!fds[0].revents().is_empty(), !fds[1].revents().is_empty())
        };
        if wl_ready {
            match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.into()),
            }
        } else {
            drop(guard);
        }
        let mut buf = [0u8; 8];
        let _ = rustix::io::read(wake.as_fd(), &mut buf);
        queue.dispatch_pending(&mut state)?;
    }
}

/// Sends `target` to every output: binds controls while tinted, releases
/// them (restoring the original ramps) once neutral.
fn apply(state: &mut State, target: GammaTarget, qh: &QueueHandle<State>, scratch: &mut Vec<u16>) {
    let Some(manager) = state.manager.clone() else { return };
    for o in &mut state.outputs {
        if target.is_neutral() {
            if let Some(c) = o.control.take() {
                c.destroy();
            }
            o.size = None;
            o.busy = false;
            continue;
        }
        if o.busy {
            continue;
        }
        match (&o.control, o.size) {
            (None, _) => o.control = Some(manager.get_gamma_control(&o.proxy, qh, o.global)),
            (Some(c), Some(size)) if size > 0 => {
                if let Err(err) = send_ramp(c, size, target, scratch) {
                    tracing::warn!("night light: {}: {err}", o.name);
                }
            }
            _ => {}
        }
    }
}

/// Briefly checks whether the running compositor offers gamma control
/// (one roundtrip on a short-lived connection).
pub fn probe() -> GammaSupport {
    struct Probe;
    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Probe {
        fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
    }
    let Ok(conn) = Connection::connect_to_env() else { return GammaSupport::NoDisplay };
    let Ok((globals, _)) = registry_queue_init::<Probe>(&conn) else { return GammaSupport::NoDisplay };
    let found = globals.contents().with_list(|l| l.iter().any(|g| g.interface == manager::ZwlrGammaControlManagerV1::interface().name));
    if found { GammaSupport::Supported } else { GammaSupport::Unsupported }
}
