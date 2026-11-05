//! River driver: River's own Wayland protocols, spoken directly.
//!
//! River (the classic, monolithic line: 0.3, `river-classic` where both are
//! packaged) has no IPC socket. A client learns its state from
//! `zriver_status_manager_v1` (`river-status-unstable-v1`, v4): per output
//! the focused tags (a 32-bit mask), every view's tags, the tags with an
//! urgent view and the layout's name; per seat the focused output, the
//! focused view's title and the active mode. It changes things with
//! `zriver_control_v1` (`river-control-unstable-v1`, v1): the commands
//! `riverctl(1)` documents (`set-focused-tags`, `toggle-focused-tags`,
//! `set-view-tags`, `toggle-view-tags`, `map`, …), one argument at a time,
//! answered by a success or failure callback. Bindings for both protocols
//! are generated from River's own XML (`protocols/`).
//!
//! River's tags are a bitmask, so the bar models them as
//! [`NavigationModel::BitmaskTags`] like MangoWM's: left-click shows one tag,
//! right-click toggles a tag into view, middle-click toggles a tag on the
//! focused window.
//!
//! What River doesn't tell a client is said here plainly: the status
//! protocol has no window list and doesn't say which window carries which
//! tags. Windows come from `zwlr_foreign_toplevel_manager_v1` instead
//! (title, app id, focus, fullscreen, output), so the bar can't put app
//! icons on River's tags and clients carry no tags. Output geometry comes
//! from `zxdg_output_manager_v1`. River 0.4 and later moved window
//! management into separate programs and no longer offer these protocols:
//! [`RiverDriver::from_env`] then finds nothing to drive.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context as _, Result, anyhow, bail};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_output, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, event_created_child};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{zwlr_foreign_toplevel_handle_v1 as handle, zwlr_foreign_toplevel_manager_v1 as toplevels};

use crate::backend::CompositorBackend;
use crate::model::{ClientInfo, CompositorEvent, CompositorKind, MonitorInfo, NavigationModel, TagStatus};

/// River's protocols, generated from the XML in River's own source tree.
pub mod protocol {
    pub mod status {
        use wayland_client;
        use wayland_client::protocol::*;
        pub mod __interfaces {
            use wayland_client::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!("protocols/river-status-unstable-v1.xml");
        }
        use self::__interfaces::*;
        wayland_scanner::generate_client_code!("protocols/river-status-unstable-v1.xml");
    }
    pub mod control {
        use wayland_client;
        use wayland_client::protocol::*;
        pub mod __interfaces {
            use wayland_client::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!("protocols/river-control-unstable-v1.xml");
        }
        use self::__interfaces::*;
        wayland_scanner::generate_client_code!("protocols/river-control-unstable-v1.xml");
    }
}

use protocol::control::{zriver_command_callback_v1 as callback, zriver_control_v1 as control};
use protocol::status::{zriver_output_status_v1 as output_status, zriver_seat_status_v1 as seat_status, zriver_status_manager_v1 as status_manager};

/// Protocol versions used (the highest River 0.3 offers).
const STATUS_VERSION: u32 = 4;
const CONTROL_VERSION: u32 = 1;
const OUTPUT_VERSION: u32 = 4;
const SEAT_VERSION: u32 = 8;
const XDG_OUTPUT_VERSION: u32 = 3;
const TOPLEVEL_VERSION: u32 = 3;
/// Tags the bar shows (more appear when one above is focused or in use).
pub const TAG_COUNT: u32 = 9;
/// `zwlr_foreign_toplevel_handle_v1` states.
const STATE_ACTIVATED: u32 = 2;
const STATE_FULLSCREEN: u32 = 3;
/// rivertile's main locations, in the order the layout pill cycles them.
const RIVERTILE_LOCATIONS: [&str; 4] = ["left", "top", "right", "bottom"];
/// The layout generator whose locations the pill knows.
const RIVERTILE: &str = "rivertile";

/// One status or window event, by object (an output's or window's protocol
/// id): what the Wayland handlers and the recorded fixtures both feed
/// [`RiverState::apply`].
#[derive(Clone, Debug, PartialEq)]
pub enum RiverEvent {
    OutputName { output: u32, name: String },
    OutputGone { output: u32 },
    /// The output's current mode, in physical pixels.
    OutputMode { output: u32, width: i32, height: i32 },
    OutputPosition { output: u32, x: i32, y: i32 },
    /// Logical size (xdg-output).
    OutputSize { output: u32, width: i32, height: i32 },
    FocusedTags { output: u32, tags: u32 },
    ViewTags { output: u32, tags: Vec<u32> },
    UrgentTags { output: u32, tags: u32 },
    LayoutName { output: u32, name: Option<String> },
    FocusedOutput(u32),
    UnfocusedOutput(u32),
    FocusedView(String),
    Mode(String),
    WindowTitle { window: u32, title: String },
    WindowAppId { window: u32, app_id: String },
    WindowState { window: u32, activated: bool, fullscreen: bool },
    WindowOutput { window: u32, output: u32, entered: bool },
    WindowClosed { window: u32 },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RiverOutput {
    pub name: String,
    pub focused_tags: u32,
    pub view_tags: Vec<u32>,
    pub urgent_tags: u32,
    pub layout: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub mode_width: i32,
    pub mode_height: i32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RiverWindow {
    pub title: String,
    pub app_id: String,
    pub activated: bool,
    pub fullscreen: bool,
    pub outputs: Vec<u32>,
}

/// Everything River has said, by object id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RiverState {
    pub outputs: BTreeMap<u32, RiverOutput>,
    pub focused_output: Option<u32>,
    pub focused_title: String,
    pub mode: String,
    pub windows: BTreeMap<u32, RiverWindow>,
}

impl RiverState {
    pub fn apply(&mut self, event: RiverEvent) {
        use RiverEvent::*;
        match event {
            OutputName { output, name } => self.outputs.entry(output).or_default().name = name,
            OutputGone { output } => {
                self.outputs.remove(&output);
            }
            OutputMode { output, width, height } => {
                let o = self.outputs.entry(output).or_default();
                (o.mode_width, o.mode_height) = (width, height);
            }
            OutputPosition { output, x, y } => {
                let o = self.outputs.entry(output).or_default();
                (o.x, o.y) = (x, y);
            }
            OutputSize { output, width, height } => {
                let o = self.outputs.entry(output).or_default();
                (o.width, o.height) = (width, height);
            }
            FocusedTags { output, tags } => self.outputs.entry(output).or_default().focused_tags = tags,
            ViewTags { output, tags } => self.outputs.entry(output).or_default().view_tags = tags,
            UrgentTags { output, tags } => self.outputs.entry(output).or_default().urgent_tags = tags,
            LayoutName { output, name } => self.outputs.entry(output).or_default().layout = name.unwrap_or_default(),
            FocusedOutput(output) => self.focused_output = Some(output),
            UnfocusedOutput(output) => {
                if self.focused_output == Some(output) {
                    self.focused_output = None;
                }
            }
            FocusedView(title) => self.focused_title = title,
            Mode(mode) => self.mode = mode,
            WindowTitle { window, title } => self.windows.entry(window).or_default().title = title,
            WindowAppId { window, app_id } => self.windows.entry(window).or_default().app_id = app_id,
            WindowState { window, activated, fullscreen } => {
                let w = self.windows.entry(window).or_default();
                (w.activated, w.fullscreen) = (activated, fullscreen);
            }
            WindowOutput { window, output, entered } => {
                let w = self.windows.entry(window).or_default();
                w.outputs.retain(|o| *o != output);
                if entered {
                    w.outputs.push(output);
                }
            }
            WindowClosed { window } => {
                self.windows.remove(&window);
            }
        }
    }

    fn output_name(&self, id: u32) -> String {
        self.outputs.get(&id).map(|o| o.name.clone()).unwrap_or_default()
    }

    /// Each named output's tags: 1–9, plus any higher one focused or in use.
    pub fn navigation(&self) -> Vec<(String, NavigationModel)> {
        let mut named: Vec<&RiverOutput> = self.outputs.values().filter(|o| !o.name.is_empty()).collect();
        named.sort_by(|a, b| a.name.cmp(&b.name));
        named
            .into_iter()
            .map(|o| {
                let occupied = o.view_tags.iter().fold(0u32, |m, t| m | t);
                let highest = 32 - (o.focused_tags | occupied | o.urgent_tags).leading_zeros();
                let tags = (1..=TAG_COUNT.max(highest))
                    .map(|id| {
                        let bit = 1u32 << (id - 1);
                        let client_count = o.view_tags.iter().filter(|t| *t & bit != 0).count() as u32;
                        TagStatus { id, active: o.focused_tags & bit != 0, occupied: client_count > 0, urgent: o.urgent_tags & bit != 0, client_count, layout: o.layout.clone() }
                    })
                    .collect();
                (o.name.clone(), NavigationModel::BitmaskTags { tags, active_layout: o.layout.clone(), keymode: self.mode.clone() })
            })
            .collect()
    }

    /// Windows (from foreign-toplevel; River doesn't say their tags).
    pub fn clients(&self) -> Vec<ClientInfo> {
        self.windows
            .iter()
            .map(|(id, w)| ClientInfo {
                id: u64::from(*id),
                title: w.title.clone(),
                app_id: w.app_id.clone(),
                monitor: w.outputs.first().map(|o| self.output_name(*o)).unwrap_or_default(),
                tags: Vec::new(),
                focused: w.activated,
                floating: false,
                fullscreen: w.fullscreen,
                minimized: false,
                geometry: None,
            })
            .collect()
    }

    /// The focused window: the activated toplevel, else (without
    /// foreign-toplevel) just the title River's seat status gives.
    pub fn focused_client(&self) -> Option<ClientInfo> {
        self.clients().into_iter().find(|c| c.focused).or_else(|| {
            (!self.focused_title.is_empty()).then(|| ClientInfo { title: self.focused_title.clone(), monitor: self.focused_output.map(|o| self.output_name(o)).unwrap_or_default(), focused: true, ..Default::default() })
        })
    }

    pub fn monitors(&self) -> Vec<MonitorInfo> {
        let mut named: Vec<(&u32, &RiverOutput)> = self.outputs.iter().filter(|(_, o)| !o.name.is_empty()).collect();
        named.sort_by(|a, b| a.1.name.cmp(&b.1.name));
        named
            .into_iter()
            .map(|(id, o)| MonitorInfo {
                name: o.name.clone(),
                x: o.x,
                y: o.y,
                width: o.width,
                height: o.height,
                // The longer sides match whatever the rotation.
                scale: match (o.width.max(o.height), o.mode_width.max(o.mode_height)) {
                    (logical, mode) if logical > 0 && mode > 0 => mode as f32 / logical as f32,
                    _ => 1.0,
                },
                active: self.focused_output == Some(*id),
            })
            .collect()
    }

    pub fn focused_output_name(&self) -> Option<String> {
        self.focused_output.map(|o| self.output_name(o)).filter(|n| !n.is_empty())
    }
}

/// The bitmask of one tag (`1..=32`).
pub fn tag_mask(id: u32) -> Result<u32> {
    (1..=32).contains(&id).then(|| 1u32 << (id - 1)).ok_or_else(|| anyhow!("river: tag {id} is out of range (1–32)"))
}

/// `view_tags`' array: native-endian 32-bit masks, one per view.
pub fn view_tags(bytes: &[u8]) -> Vec<u32> {
    bytes.as_chunks::<4>().0.iter().map(|c| u32::from_ne_bytes(*c)).collect()
}

/// The Wayland side: bound globals, the state and a command's answer.
struct Wl {
    state: RiverState,
    seat: Option<wl_seat::WlSeat>,
    status: Option<status_manager::ZriverStatusManagerV1>,
    control: Option<control::ZriverControlV1>,
    xdg: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    /// Bind outputs, status and windows (a query), or only what a command
    /// needs.
    full: bool,
    windows: BTreeMap<u32, handle::ZwlrForeignToplevelHandleV1>,
    /// Registry name → output object, to notice an output unplugged.
    output_globals: BTreeMap<u32, u32>,
    answer: Option<std::result::Result<String, String>>,
}

impl Wl {
    fn watch_output(&mut self, registry: &wl_registry::WlRegistry, name: u32, version: u32, qh: &QueueHandle<Self>) {
        let output: wl_output::WlOutput = registry.bind(name, version.min(OUTPUT_VERSION), qh, name);
        let id = output.id().protocol_id();
        self.output_globals.insert(name, id);
        self.state.outputs.entry(id).or_default();
        if let Some(status) = &self.status {
            status.get_river_output_status(&output, qh, id);
        }
        if let Some(xdg) = &self.xdg {
            xdg.get_xdg_output(&output, qh, id);
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Wl {
    fn event(state: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &GlobalListContents, _: &Connection, qh: &QueueHandle<Self>) {
        // Outputs plugged in or out later.
        match event {
            wl_registry::Event::Global { name, interface, version } if state.full && interface == wl_output::WlOutput::interface().name => state.watch_output(registry, name, version, qh),
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(output) = state.output_globals.remove(&name) {
                    state.state.apply(RiverEvent::OutputGone { output });
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for Wl {
    fn event(state: &mut Self, proxy: &wl_output::WlOutput, event: wl_output::Event, _: &u32, _: &Connection, _: &QueueHandle<Self>) {
        let output = proxy.id().protocol_id();
        match event {
            wl_output::Event::Name { name } => state.state.apply(RiverEvent::OutputName { output, name }),
            wl_output::Event::Mode { flags, width, height, .. } if flags.into_result().is_ok_and(|f| f.contains(wl_output::Mode::Current)) => state.state.apply(RiverEvent::OutputMode { output, width, height }),
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Wl {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<status_manager::ZriverStatusManagerV1, ()> for Wl {
    fn event(_: &mut Self, _: &status_manager::ZriverStatusManagerV1, _: status_manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<output_status::ZriverOutputStatusV1, u32> for Wl {
    fn event(state: &mut Self, _: &output_status::ZriverOutputStatusV1, event: output_status::Event, output: &u32, _: &Connection, _: &QueueHandle<Self>) {
        let output = *output;
        state.state.apply(match event {
            output_status::Event::FocusedTags { tags } => RiverEvent::FocusedTags { output, tags },
            output_status::Event::ViewTags { tags } => RiverEvent::ViewTags { output, tags: view_tags(&tags) },
            output_status::Event::UrgentTags { tags } => RiverEvent::UrgentTags { output, tags },
            output_status::Event::LayoutName { name } => RiverEvent::LayoutName { output, name: Some(name) },
            output_status::Event::LayoutNameClear => RiverEvent::LayoutName { output, name: None },
        });
    }
}

impl Dispatch<seat_status::ZriverSeatStatusV1, ()> for Wl {
    fn event(state: &mut Self, _: &seat_status::ZriverSeatStatusV1, event: seat_status::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        state.state.apply(match event {
            seat_status::Event::FocusedOutput { output } => RiverEvent::FocusedOutput(output.id().protocol_id()),
            seat_status::Event::UnfocusedOutput { output } => RiverEvent::UnfocusedOutput(output.id().protocol_id()),
            seat_status::Event::FocusedView { title } => RiverEvent::FocusedView(title),
            seat_status::Event::Mode { name } => RiverEvent::Mode(name),
        });
    }
}

impl Dispatch<control::ZriverControlV1, ()> for Wl {
    fn event(_: &mut Self, _: &control::ZriverControlV1, _: control::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<callback::ZriverCommandCallbackV1, ()> for Wl {
    fn event(state: &mut Self, _: &callback::ZriverCommandCallbackV1, event: callback::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        state.answer = match event {
            callback::Event::Success { output } => Some(Ok(output)),
            callback::Event::Failure { failure_message } => Some(Err(failure_message)),
        };
    }
}

impl Dispatch<zxdg_output_manager_v1::ZxdgOutputManagerV1, ()> for Wl {
    fn event(_: &mut Self, _: &zxdg_output_manager_v1::ZxdgOutputManagerV1, _: zxdg_output_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, u32> for Wl {
    fn event(state: &mut Self, _: &zxdg_output_v1::ZxdgOutputV1, event: zxdg_output_v1::Event, output: &u32, _: &Connection, _: &QueueHandle<Self>) {
        let output = *output;
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => state.state.apply(RiverEvent::OutputPosition { output, x, y }),
            zxdg_output_v1::Event::LogicalSize { width, height } => state.state.apply(RiverEvent::OutputSize { output, width, height }),
            _ => {}
        }
    }
}

impl Dispatch<toplevels::ZwlrForeignToplevelManagerV1, ()> for Wl {
    fn event(state: &mut Self, _: &toplevels::ZwlrForeignToplevelManagerV1, event: toplevels::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let toplevels::Event::Toplevel { toplevel } = event {
            state.windows.insert(toplevel.id().protocol_id(), toplevel);
        }
    }

    event_created_child!(Wl, toplevels::ZwlrForeignToplevelManagerV1, [
        toplevels::EVT_TOPLEVEL_OPCODE => (handle::ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<handle::ZwlrForeignToplevelHandleV1, ()> for Wl {
    fn event(state: &mut Self, proxy: &handle::ZwlrForeignToplevelHandleV1, event: handle::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let window = proxy.id().protocol_id();
        state.state.apply(match event {
            handle::Event::Title { title } => RiverEvent::WindowTitle { window, title },
            handle::Event::AppId { app_id } => RiverEvent::WindowAppId { window, app_id },
            handle::Event::State { state: bytes } => {
                let states = view_tags(&bytes);
                RiverEvent::WindowState { window, activated: states.contains(&STATE_ACTIVATED), fullscreen: states.contains(&STATE_FULLSCREEN) }
            }
            handle::Event::OutputEnter { output } => RiverEvent::WindowOutput { window, output: output.id().protocol_id(), entered: true },
            handle::Event::OutputLeave { output } => RiverEvent::WindowOutput { window, output: output.id().protocol_id(), entered: false },
            handle::Event::Closed => {
                state.windows.remove(&window);
                RiverEvent::WindowClosed { window }
            }
            _ => return,
        });
    }
}

/// A connection to River with what `full` asks for bound and its first
/// answers read (two round trips: globals, then each object's first state).
fn open(full: bool) -> Result<(Connection, EventQueue<Wl>, Wl)> {
    let conn = Connection::connect_to_env().context("river: no Wayland display")?;
    let (globals, mut queue) = registry_queue_init::<Wl>(&conn).context("river: reading the globals")?;
    let qh = queue.handle();
    let mut wl = Wl { state: RiverState::default(), seat: None, status: None, control: None, xdg: None, full, windows: BTreeMap::new(), output_globals: BTreeMap::new(), answer: None };
    wl.seat = globals.bind(&qh, 1..=SEAT_VERSION, ()).ok();
    wl.control = globals.bind(&qh, 1..=CONTROL_VERSION, ()).ok();
    if full {
        wl.status = globals.bind(&qh, 1..=STATUS_VERSION, ()).ok();
        wl.xdg = globals.bind(&qh, 1..=XDG_OUTPUT_VERSION, ()).ok();
        let _toplevels: Option<toplevels::ZwlrForeignToplevelManagerV1> = globals.bind(&qh, 1..=TOPLEVEL_VERSION, ()).ok();
        let outputs: Vec<(u32, u32)> = globals.contents().clone_list().into_iter().filter(|g| g.interface == wl_output::WlOutput::interface().name).map(|g| (g.name, g.version)).collect();
        for (name, version) in outputs {
            wl.watch_output(globals.registry(), name, version, &qh);
        }
        if let (Some(status), Some(seat)) = (&wl.status, &wl.seat) {
            status.get_river_seat_status(seat, &qh, ());
        }
    }
    queue.roundtrip(&mut wl).context("river: first round trip")?;
    queue.roundtrip(&mut wl).context("river: second round trip")?;
    Ok((conn, queue, wl))
}

/// Whether the running compositor speaks River's control and status
/// protocols (asked once per process; `false` without a Wayland display).
pub fn running() -> bool {
    static ANSWER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ANSWER.get_or_init(|| {
        let Ok(conn) = Connection::connect_to_env() else { return false };
        let Ok((globals, _)) = registry_queue_init::<Wl>(&conn) else { return false };
        let has = |name: &str| globals.contents().with_list(|list| list.iter().any(|g| g.interface == name));
        has(control::ZriverControlV1::interface().name) && has(status_manager::ZriverStatusManagerV1::interface().name)
    })
}

/// Runs one River command (`riverctl`'s argv) and returns its output.
pub fn run_command(words: &[&str]) -> Result<String> {
    let (_conn, mut queue, mut wl) = open(false)?;
    let (Some(control), Some(seat)) = (wl.control.clone(), wl.seat.clone()) else { bail!("river: no river-control (River 0.4 and later dropped it)") };
    for w in words {
        control.add_argument(w.to_string());
    }
    control.run_command(&seat, &queue.handle(), ());
    while wl.answer.is_none() {
        queue.blocking_dispatch(&mut wl).context("river: waiting for the command's answer")?;
    }
    match wl.answer.take() {
        Some(Ok(output)) => Ok(output),
        Some(Err(message)) => bail!("river: {} ({})", message, words.join(" ")),
        None => unreachable!("loop ends on an answer"),
    }
}

/// The state the status protocol, xdg-output and foreign-toplevel report.
pub fn snapshot() -> Result<RiverState> {
    Ok(open(true)?.2.state)
}

/// What changed since the last state sent: navigation per output, the
/// window list, focus, monitors and the mode.
#[derive(Default)]
pub struct RiverDiff {
    last: Option<RiverState>,
}

impl RiverDiff {
    pub fn events(&mut self, now: &RiverState) -> Vec<CompositorEvent> {
        let first = self.last.is_none();
        let before = self.last.take().unwrap_or_default();
        let mut out = Vec::new();
        let old_nav: BTreeMap<String, NavigationModel> = before.navigation().into_iter().collect();
        for (monitor, model) in now.navigation() {
            if old_nav.get(&monitor) != Some(&model) {
                out.push(CompositorEvent::Navigation { monitor, model });
            }
        }
        if before.windows != now.windows || before.outputs.keys().ne(now.outputs.keys()) {
            out.push(CompositorEvent::Clients(now.clients()));
        }
        let focused = now.focused_client();
        if before.focused_client() != focused || first {
            out.push(CompositorEvent::FocusedClient(focused));
        }
        if before.monitors() != now.monitors() {
            out.push(CompositorEvent::Monitors(now.monitors()));
        }
        if before.mode != now.mode {
            out.push(CompositorEvent::Keymode(now.mode.clone()));
        }
        self.last = Some(now.clone());
        out
    }
}

pub struct RiverDriver {
    /// rivertile main location the layout pill moves to next.
    next_location: AtomicUsize,
}

impl RiverDriver {
    /// A driver when the compositor is River with its control and status
    /// protocols (classic River, 0.3).
    pub fn from_env() -> Option<Self> {
        running().then(|| Self { next_location: AtomicUsize::new(1) })
    }

    fn run(&self, words: &[&str]) -> Result<()> {
        run_command(words).map(|_| ())
    }

    /// Focus `monitor` first when it isn't the focused output.
    fn on_output(&self, monitor: &str) -> Result<()> {
        if !monitor.is_empty() && snapshot()?.focused_output_name().as_deref() != Some(monitor) {
            self.run(&["focus-output", monitor])?;
        }
        Ok(())
    }

    /// Focuses window `id` (a foreign-toplevel id) through that protocol.
    fn focus_window(&self, id: u64) -> Result<()> {
        let (_conn, mut queue, mut wl) = open(true)?;
        let handle = u32::try_from(id).ok().and_then(|id| wl.windows.get(&id).cloned()).context("river: no such window")?;
        let seat = wl.seat.clone().context("river: no seat")?;
        handle.activate(&seat);
        queue.roundtrip(&mut wl)?;
        Ok(())
    }
}

impl CompositorBackend for RiverDriver {
    fn kind(&self) -> CompositorKind {
        CompositorKind::River
    }

    fn version(&self) -> Option<(u32, u32)> {
        mandara_config::compositor::installed_version(mandara_config::compositor::Target::River)
    }

    fn subscribe_events(&self) -> async_channel::Receiver<CompositorEvent> {
        let (tx, rx) = async_channel::bounded(crate::EVENT_QUEUE);
        let _ = std::thread::Builder::new().name("river-status".into()).spawn(move || {
            loop {
                let result = (|| -> Result<bool> {
                    let (_conn, mut queue, mut wl) = open(true)?;
                    let mut diff = RiverDiff::default();
                    loop {
                        for e in diff.events(&wl.state) {
                            if tx.send_blocking(e).is_err() {
                                return Ok(true);
                            }
                        }
                        queue.blocking_dispatch(&mut wl)?;
                    }
                })();
                if matches!(result, Ok(true)) || tx.is_closed() {
                    return;
                }
                std::thread::sleep(crate::RECONNECT_DELAY);
            }
        });
        rx
    }

    fn get_navigation_state(&self) -> Result<Vec<(String, NavigationModel)>> {
        Ok(snapshot()?.navigation())
    }

    fn get_active_client(&self) -> Result<Option<ClientInfo>> {
        Ok(snapshot()?.focused_client())
    }

    fn get_all_clients(&self) -> Result<Vec<ClientInfo>> {
        Ok(snapshot()?.clients())
    }

    fn get_monitors(&self) -> Result<Vec<MonitorInfo>> {
        Ok(snapshot()?.monitors())
    }

    fn select_primary(&self, monitor: &str, id: u32) -> Result<()> {
        let mask = tag_mask(id)?.to_string();
        self.on_output(monitor)?;
        self.run(&["set-focused-tags", &mask])
    }

    fn toggle_secondary(&self, monitor: &str, id: u32) -> Result<()> {
        let mask = tag_mask(id)?.to_string();
        self.on_output(monitor)?;
        self.run(&["toggle-focused-tags", &mask])
    }

    fn move_client_to(&self, client_id: Option<u64>, id: u32) -> Result<()> {
        let mask = tag_mask(id)?.to_string();
        if let Some(c) = client_id {
            self.focus_window(c)?;
        }
        self.run(&["set-view-tags", &mask])
    }

    fn toggle_client_tag(&self, client_id: Option<u64>, tag: u32) -> Result<()> {
        let mask = tag_mask(tag)?.to_string();
        if let Some(c) = client_id {
            self.focus_window(c)?;
        }
        self.run(&["toggle-view-tags", &mask])
    }

    fn cycle_layout(&self, _monitor: &str) -> Result<()> {
        // River's layouts come from a separate generator; rivertile's main
        // area can move round the screen (`rivertile(1)`).
        let state = snapshot()?;
        let layout = state.focused_output.and_then(|o| state.outputs.get(&o)).map(|o| o.layout.clone()).unwrap_or_default();
        if !layout.starts_with(RIVERTILE) {
            bail!("river: the layout generator ({}) has no layouts Mandara can cycle", if layout.is_empty() { "none" } else { &layout });
        }
        let i = self.next_location.fetch_add(1, Ordering::Relaxed) % RIVERTILE_LOCATIONS.len();
        self.run(&["send-layout-cmd", RIVERTILE, &format!("main-location {}", RIVERTILE_LOCATIONS[i])])
    }

    fn dispatch_raw(&self, cmd: &str, args: &[&str]) -> Result<()> {
        let words: Vec<&str> = std::iter::once(cmd).chain(args.iter().copied()).collect();
        self.run(&words)
    }

    fn reload_config(&self) -> Result<()> {
        // Nothing to reload: Mandara's settings were sent as commands by
        // `sync_managed_config`, and River re-reads no file.
        Ok(())
    }

    fn switch_keyboard_layout(&self) -> Result<()> {
        bail!("river: no command switches the keyboard layout (an xkb option such as grp:alt_shift_toggle does)")
    }

    fn apply_cursor(&self, theme: Option<&str>, size: Option<u32>) -> Result<()> {
        let Some(theme) = theme.map(str::trim).filter(|t| !t.is_empty()).or(size.map(|_| "default")) else { return Ok(()) };
        let size = size.map(|s| s.to_string());
        let words: Vec<&str> = ["xcursor-theme", theme].into_iter().chain(size.as_deref()).collect();
        self.run(&words)
    }

    /// River reads no drop-in: the commands are sent now. Mandara's binds
    /// and rules from the last sync are taken back first (so a removed one
    /// goes, and none is reported as overwritten), then every command runs;
    /// one River refuses is reported, the rest still apply.
    fn sync_managed_config(&self, config: &mandara_config::compositor::CompositorConfig, borders: Option<mandara_config::compositor::BorderColors>) -> Result<(std::path::PathBuf, mandara_config::compositor::Emitted)> {
        use mandara_config::compositor::{Target, parse_river_record, river_undo};
        let previous = std::fs::read_to_string(Target::River.generated_file()).map(|t| parse_river_record(&t)).unwrap_or_default();
        let (path, mut emitted) = crate::backend::write_dropin(Target::River, config, borders, self.version())?;
        for words in river_undo(&previous) {
            let words: Vec<&str> = words.iter().map(String::as_str).collect();
            let _ = run_command(&words);
        }
        for words in parse_river_record(&emitted.text) {
            let refs: Vec<&str> = words.iter().map(String::as_str).collect();
            match run_command(&refs) {
                Ok(output) if !output.trim().is_empty() => emitted.skipped.push(format!("{}: {}", refs.join(" "), output.trim())),
                Ok(_) => {}
                Err(err) => emitted.skipped.push(format!("{err:#}")),
            }
        }
        Ok((path, emitted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// River's status events as a client received them from a headless
    /// River 0.3.17 (`WAYLAND_DEBUG=1` of one [`snapshot`], see
    /// `tests/fixtures/river/`): two headless outputs, foot windows on
    /// HEADLESS-2's tags 1 (two), 2 and 3, tag 3 urgent (a bell),
    /// rivertile laying them out, tag 1 shown.
    fn recorded() -> RiverState {
        let text = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/river/status.log")).unwrap();
        let mut state = RiverState::default();
        for e in parse_debug_log(&text) {
            state.apply(e);
        }
        state
    }

    /// `WAYLAND_DEBUG` lines (` <- interface@id.event, (args)`) as events.
    fn parse_debug_log(text: &str) -> Vec<RiverEvent> {
        let mut out = Vec::new();
        let mut status_output: BTreeMap<u32, u32> = BTreeMap::new();
        let mut xdg_output: BTreeMap<u32, u32> = BTreeMap::new();
        for line in text.lines() {
            if let Some(req) = line.split_once(" -> ").map(|(_, r)| r) {
                // Requests that tie status / xdg objects to their output:
                // `get_river_output_status(new id zriver_output_status_v1@9, wl_output@5)`.
                let Some((head, args)) = req.split_once('(') else { continue };
                let ids: Vec<u32> = args.split(['@', ',', ')', ' ']).filter_map(|w| w.parse().ok()).collect();
                if head.ends_with(".get_river_output_status") && ids.len() >= 2 {
                    status_output.insert(ids[0], ids[1]);
                } else if head.ends_with(".get_xdg_output") && ids.len() >= 2 {
                    xdg_output.insert(ids[0], ids[1]);
                }
                continue;
            }
            let Some(ev) = line.split_once(" <- ").map(|(_, r)| r) else { continue };
            let Some((target, args)) = ev.split_once(", (") else { continue };
            let args = args.trim_end_matches(')');
            let Some((object, event)) = target.split_once('.') else { continue };
            let Some((interface, id)) = object.split_once('@') else { continue };
            let id: u32 = id.parse().unwrap();
            // `Some("text")`.
            let string = || args.trim_start_matches("Some(").trim_end_matches(')').trim_matches('"').to_string();
            // `[4, 0, 0, 0, 2, 0, 0, 0]`: the bytes, in decimal.
            let bytes = || -> Vec<u8> { args.trim_start_matches('[').trim_end_matches(']').split(", ").filter(|b| !b.is_empty()).map(|b| b.trim().parse().unwrap()).collect() };
            let number = |i: usize| args.split(", ").nth(i).and_then(|a| a.trim().parse::<i64>().ok()).unwrap_or_default();
            let object_arg = || args.split('@').nth(1).and_then(|n| n.trim_end_matches(')').parse::<u32>().ok()).unwrap_or_default();
            out.push(match (interface, event) {
                ("wl_output", "name") => RiverEvent::OutputName { output: id, name: string() },
                ("wl_output", "mode") if number(0) & 1 == 1 => RiverEvent::OutputMode { output: id, width: number(1) as i32, height: number(2) as i32 },
                ("zxdg_output_v1", "logical_position") => RiverEvent::OutputPosition { output: xdg_output[&id], x: number(0) as i32, y: number(1) as i32 },
                ("zxdg_output_v1", "logical_size") => RiverEvent::OutputSize { output: xdg_output[&id], width: number(0) as i32, height: number(1) as i32 },
                ("zriver_output_status_v1", "focused_tags") => RiverEvent::FocusedTags { output: status_output[&id], tags: number(0) as u32 },
                ("zriver_output_status_v1", "urgent_tags") => RiverEvent::UrgentTags { output: status_output[&id], tags: number(0) as u32 },
                ("zriver_output_status_v1", "view_tags") => RiverEvent::ViewTags { output: status_output[&id], tags: view_tags(&bytes()) },
                ("zriver_output_status_v1", "layout_name") => RiverEvent::LayoutName { output: status_output[&id], name: Some(string()) },
                ("zriver_output_status_v1", "layout_name_clear") => RiverEvent::LayoutName { output: status_output[&id], name: None },
                ("zriver_seat_status_v1", "focused_output") => RiverEvent::FocusedOutput(object_arg()),
                ("zriver_seat_status_v1", "unfocused_output") => RiverEvent::UnfocusedOutput(object_arg()),
                ("zriver_seat_status_v1", "focused_view") => RiverEvent::FocusedView(string()),
                ("zriver_seat_status_v1", "mode") => RiverEvent::Mode(string()),
                ("zwlr_foreign_toplevel_handle_v1", "title") => RiverEvent::WindowTitle { window: id, title: string() },
                ("zwlr_foreign_toplevel_handle_v1", "app_id") => RiverEvent::WindowAppId { window: id, app_id: string() },
                ("zwlr_foreign_toplevel_handle_v1", "output_enter") => RiverEvent::WindowOutput { window: id, output: object_arg(), entered: true },
                ("zwlr_foreign_toplevel_handle_v1", "state") => {
                    let states = view_tags(&bytes());
                    RiverEvent::WindowState { window: id, activated: states.contains(&STATE_ACTIVATED), fullscreen: states.contains(&STATE_FULLSCREEN) }
                }
                ("zwlr_foreign_toplevel_handle_v1", "closed") => RiverEvent::WindowClosed { window: id },
                _ => continue,
            });
        }
        out
    }

    #[test]
    fn tags_come_from_the_recorded_status() {
        let state = recorded();
        let nav = state.navigation();
        assert_eq!(nav.iter().map(|(o, _)| o.as_str()).collect::<Vec<_>>(), ["HEADLESS-1", "HEADLESS-2"], "{state:#?}");
        let NavigationModel::BitmaskTags { tags, active_layout, keymode } = &nav[1].1 else { panic!("{nav:?}") };
        assert_eq!(tags.len(), TAG_COUNT as usize);
        assert!(tags[0].active && !tags[1].active, "{tags:#?}");
        assert!(tags[0].occupied && tags[1].occupied && tags[2].occupied && !tags[3].occupied, "{tags:#?}");
        assert_eq!(tags[0].client_count, 2);
        assert!(tags[2].urgent && !tags[0].urgent, "{tags:#?}");
        assert_eq!(active_layout, "rivertile - left");
        assert_eq!(keymode, "normal");
        assert_eq!(nav[1].1.active_mask(), 0b1);
        let NavigationModel::BitmaskTags { tags, active_layout, .. } = &nav[0].1 else { panic!("{nav:?}") };
        assert!(tags.iter().all(|t| !t.occupied) && active_layout.is_empty(), "an empty output has no layout yet");
    }

    #[test]
    fn windows_and_outputs_come_from_the_recorded_protocols() {
        let state = recorded();
        let clients = state.clients();
        assert_eq!(clients.len(), 4, "{clients:#?}");
        assert!(clients.iter().any(|c| c.app_id == "term-bell" && c.title == "bell" && !c.monitor.is_empty()), "{clients:#?}");
        assert!(clients.iter().all(|c| c.tags.is_empty()), "River doesn't say a window's tags");
        assert_eq!(clients.iter().filter(|c| c.focused).count(), 1);
        assert!(state.focused_client().is_some_and(|c| !c.app_id.is_empty()));
        let m = state.monitors();
        assert_eq!(m.iter().map(|m| (m.name.as_str(), m.width, m.height, m.scale)).collect::<Vec<_>>(), [("HEADLESS-1", 1280, 720, 1.0), ("HEADLESS-2", 1280, 720, 1.0)]);
        assert_eq!(m.iter().filter(|m| m.active).map(|m| m.name.as_str()).collect::<Vec<_>>(), ["HEADLESS-2"]);
        assert!(clients.iter().all(|c| c.monitor == "HEADLESS-2"));
    }

    #[test]
    fn tag_masks_and_view_tags() {
        assert_eq!(tag_mask(1).unwrap(), 1);
        assert_eq!(tag_mask(9).unwrap(), 256);
        assert_eq!(tag_mask(32).unwrap(), 1 << 31);
        assert!(tag_mask(0).is_err() && tag_mask(33).is_err());
        let bytes: Vec<u8> = [1u32, 6].iter().flat_map(|t| t.to_ne_bytes()).collect();
        assert_eq!(view_tags(&bytes), [1, 6]);
        // A tag above 9 that is in use shows up too.
        let mut s = RiverState::default();
        s.apply(RiverEvent::OutputName { output: 3, name: "DP-1".into() });
        s.apply(RiverEvent::ViewTags { output: 3, tags: vec![1 << 11] });
        let NavigationModel::BitmaskTags { tags, .. } = &s.navigation()[0].1 else { panic!() };
        assert_eq!(tags.len(), 12);
        assert!(tags[11].occupied && tags[11].client_count == 1);
    }

    #[test]
    fn rotated_outputs_keep_their_scale() {
        let mut s = RiverState::default();
        s.apply(RiverEvent::OutputName { output: 1, name: "eDP-1".into() });
        s.apply(RiverEvent::OutputMode { output: 1, width: 2880, height: 1800 });
        // Rotated 90° at 1.5: the logical size is 1200 × 1920.
        s.apply(RiverEvent::OutputSize { output: 1, width: 1200, height: 1920 });
        assert_eq!(s.monitors()[0].scale, 1.5);
    }

    #[test]
    fn the_diff_sends_only_changes() {
        let state = recorded();
        let mut diff = RiverDiff::default();
        let first = diff.events(&state);
        assert!(first.iter().any(|e| matches!(e, CompositorEvent::Navigation { .. })) && first.iter().any(|e| matches!(e, CompositorEvent::FocusedClient(Some(_)))));
        assert!(diff.events(&state).is_empty());
        let mut next = state.clone();
        let output = *next.outputs.keys().next().unwrap();
        next.apply(RiverEvent::FocusedTags { output, tags: 0b10 });
        next.apply(RiverEvent::Mode("resize".into()));
        let out = diff.events(&next);
        assert_eq!(out.iter().filter(|e| matches!(e, CompositorEvent::Navigation { .. })).count(), 2, "the tags of one output, the mode on both: {out:#?}");
        assert!(out.contains(&CompositorEvent::Keymode("resize".into())));
        assert!(!out.iter().any(|e| matches!(e, CompositorEvent::Clients(_))));
    }

    /// One snapshot of a running River (`MANDARA_TEST_RIVER=1`): run alone
    /// with `WAYLAND_DEBUG=1`, its log is the `status.log` fixture.
    #[test]
    fn live_river_snapshot() {
        if std::env::var_os("MANDARA_TEST_RIVER").is_none() {
            return;
        }
        let state = snapshot().unwrap();
        assert!(!state.navigation().is_empty(), "{state:#?}");
    }

    /// End to end against a running River when `MANDARA_TEST_RIVER=1` (a
    /// throwaway headless one, through `WAYLAND_DISPLAY`: this switches its
    /// tags). Skipped otherwise.
    #[test]
    fn live_river_round_trip() {
        if std::env::var_os("MANDARA_TEST_RIVER").is_none() {
            eprintln!("MANDARA_TEST_RIVER isn't set; skipped");
            return;
        }
        let driver = RiverDriver::from_env().expect("River with river-control and river-status");
        let events = driver.subscribe_events();
        assert!(matches!(events.recv_blocking().unwrap(), CompositorEvent::Navigation { .. }));
        let monitors = driver.get_monitors().unwrap();
        assert!(!monitors.is_empty());
        let output = monitors.iter().find(|m| m.active).unwrap_or(&monitors[0]).name.clone();
        driver.select_primary(&output, 4).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut seen = false;
        while std::time::Instant::now() < deadline && !seen {
            if let Ok(CompositorEvent::Navigation { model, .. }) = events.recv_blocking() {
                seen = model.active_mask() == 1 << 3;
            }
        }
        assert!(seen, "tag 4 shown through the status protocol");
        driver.toggle_secondary(&output, 1).unwrap();
        let nav = driver.get_navigation_state().unwrap();
        assert!(nav.iter().any(|(o, m)| *o == output && m.active_mask() == 0b1001), "{output}: {nav:#?}");
        assert!(driver.dispatch_raw("frobnicate", &[]).unwrap_err().to_string().contains("unknown command"));
        driver.apply_cursor(Some("Adwaita"), Some(32)).unwrap();
        let clients = driver.get_all_clients().unwrap();
        if let Some(c) = clients.first() {
            driver.move_client_to(Some(c.id), 2).unwrap();
        }
        driver.select_primary(&output, 1).unwrap();
        eprintln!("live river: {:?}\nclients: {clients:?}", driver.get_navigation_state().unwrap());
    }
}
