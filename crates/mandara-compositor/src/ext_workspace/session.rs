//! One Wayland connection: outputs (wl_output + xdg-output), workspaces
//! (`ext_workspace_manager_v1`), windows and, optionally, an idle
//! notification. Windows come from `zwlr_foreign_toplevel_manager_v1`
//! where the compositor offers it (labwc), else from
//! `ext_foreign_toplevel_list_v1` with COSMIC's toplevel info and manager
//! on top (cosmic-comp).

use std::collections::BTreeMap;
use std::io::{PipeReader, Read as _};
use std::time::Instant;

use anyhow::{Context as _, Result, anyhow, bail};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_callback, wl_output, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, event_created_child};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{ext_foreign_toplevel_handle_v1 as ext_handle, ext_foreign_toplevel_list_v1 as ext_list};
use wayland_protocols::ext::idle_notify::v1::client::{ext_idle_notification_v1 as idle_notification, ext_idle_notifier_v1 as idle_notifier};
use wayland_protocols::ext::workspace::v1::client::{ext_workspace_group_handle_v1 as group, ext_workspace_handle_v1 as workspace, ext_workspace_manager_v1 as manager};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1 as xdg_manager, zxdg_output_v1 as xdg_output};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{zwlr_foreign_toplevel_handle_v1 as wlr_handle, zwlr_foreign_toplevel_manager_v1 as wlr_manager};

use super::cosmic::info::{zcosmic_toplevel_handle_v1 as cosmic_handle, zcosmic_toplevel_info_v1 as cosmic_info};
use super::cosmic::management::zcosmic_toplevel_manager_v1 as cosmic_manager;
use super::state::{Event, Request, State};
use crate::kwin::WindowAction;

const MANAGER_VERSION: u32 = 1;
const OUTPUT_VERSION: u32 = 4;
const XDG_VERSION: u32 = 3;
const SEAT_VERSION: u32 = 8;
const WLR_TOPLEVEL_VERSION: u32 = 3;
const EXT_LIST_VERSION: u32 = 1;
/// `get_cosmic_toplevel` arrived in version 2, ext-workspace membership in 3.
const COSMIC_INFO_VERSION: u32 = 3;
const COSMIC_INFO_MIN: u32 = 2;
/// `move_to_ext_workspace` arrived in version 4.
const COSMIC_MANAGER_VERSION: u32 = 4;
const COSMIC_MOVE_SINCE: u32 = 4;
const IDLE_VERSION: u32 = 1;

/// `zwlr_foreign_toplevel_handle_v1` / `zcosmic_toplevel_handle_v1` states
/// (the same numbers in both).
const STATE_MAXIMIZED: u32 = 0;
const STATE_MINIMIZED: u32 = 1;
const STATE_ACTIVATED: u32 = 2;
const STATE_FULLSCREEN: u32 = 3;

/// What a session binds.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// The workspace manager (the compositor driver); the window list alone
    /// otherwise (the shared window service).
    pub workspaces: bool,
}

pub(super) struct Wl {
    pub state: State,
    /// A batch completed since the last [`Session::take_published`].
    published: bool,
    synced: u64,
    seat: Option<wl_seat::WlSeat>,
    workspace_manager: Option<manager::ExtWorkspaceManagerV1>,
    workspaces: BTreeMap<u32, workspace::ExtWorkspaceHandleV1>,
    groups: BTreeMap<u32, group::ExtWorkspaceGroupHandleV1>,
    /// Registry name → (output, its xdg-output).
    outputs: BTreeMap<u32, (wl_output::WlOutput, Option<xdg_output::ZxdgOutputV1>)>,
    xdg: Option<xdg_manager::ZxdgOutputManagerV1>,
    wlr_windows: BTreeMap<u32, wlr_handle::ZwlrForeignToplevelHandleV1>,
    ext_windows: BTreeMap<u32, ext_handle::ExtForeignToplevelHandleV1>,
    cosmic_info: Option<cosmic_info::ZcosmicToplevelInfoV1>,
    /// By the ext handle's id.
    cosmic_windows: BTreeMap<u32, cosmic_handle::ZcosmicToplevelHandleV1>,
    cosmic_manager: Option<cosmic_manager::ZcosmicToplevelManagerV1>,
    idle_notifier: Option<idle_notifier::ExtIdleNotifierV1>,
    idle_note: Option<idle_notification::ExtIdleNotificationV1>,
    /// A window list is bound (wlr or ext).
    windows: bool,
}

impl Wl {
    fn apply(&mut self, event: Event) {
        self.published |= self.state.apply(event);
    }

    fn watch_output(&mut self, registry: &wl_registry::WlRegistry, name: u32, version: u32, qh: &QueueHandle<Self>) {
        if self.outputs.contains_key(&name) {
            return;
        }
        let output: wl_output::WlOutput = registry.bind(name, version.min(OUTPUT_VERSION), qh, ());
        let xdg = self.xdg.as_ref().map(|xdg| xdg.get_xdg_output(&output, qh, output.id().protocol_id()));
        self.outputs.insert(name, (output, xdg));
    }

    fn output_offset(&self, output: u32) -> (i32, i32) {
        self.state.outputs.get(&output).map_or((0, 0), |o| (o.x, o.y))
    }
}

pub struct Session {
    connection: Connection,
    queue: EventQueue<Wl>,
    wl: Wl,
}

impl Session {
    pub fn open(options: Options) -> Result<Self> {
        let connection = Connection::connect_to_env().context("ext-workspace: no Wayland display")?;
        let (globals, mut queue) = registry_queue_init::<Wl>(&connection).context("ext-workspace: reading the globals")?;
        let qh = queue.handle();
        let xdg = globals.bind(&qh, 1..=XDG_VERSION, ()).ok();
        let seat = globals.bind(&qh, 1..=SEAT_VERSION, ()).ok();
        let workspace_manager = if options.workspaces { globals.bind(&qh, 1..=MANAGER_VERSION, ()).ok() } else { None };
        let wlr: Option<wlr_manager::ZwlrForeignToplevelManagerV1> = globals.bind(&qh, 1..=WLR_TOPLEVEL_VERSION, ()).ok();
        // COSMIC's info needs the ext list's handles, so it is bound first.
        let (cosmic_info, cosmic_manager) = if wlr.is_none() {
            (globals.bind(&qh, COSMIC_INFO_MIN..=COSMIC_INFO_VERSION, ()).ok(), globals.bind(&qh, 1..=COSMIC_MANAGER_VERSION, ()).ok())
        } else {
            (None, None)
        };
        let mut wl = Wl {
            state: State::default(),
            published: true,
            synced: 0,
            seat,
            workspace_manager,
            workspaces: BTreeMap::new(),
            groups: BTreeMap::new(),
            outputs: BTreeMap::new(),
            xdg,
            wlr_windows: BTreeMap::new(),
            ext_windows: BTreeMap::new(),
            cosmic_info,
            cosmic_windows: BTreeMap::new(),
            cosmic_manager,
            idle_notifier: globals.bind(&qh, 1..=IDLE_VERSION, ()).ok(),
            idle_note: None,
            windows: wlr.is_some(),
        };
        if wlr.is_none() {
            let list: Option<ext_list::ExtForeignToplevelListV1> = globals.bind(&qh, 1..=EXT_LIST_VERSION, ()).ok();
            wl.windows = list.is_some();
        }
        for global in globals.contents().clone_list() {
            if global.interface == wl_output::WlOutput::interface().name {
                wl.watch_output(globals.registry(), global.name, global.version, &qh);
            }
        }
        // Globals' first events create child objects; their own first state
        // follows the second round trip.
        roundtrip(&connection, &mut queue, &mut wl)?;
        roundtrip(&connection, &mut queue, &mut wl)?;
        Ok(Self { connection, queue, wl })
    }

    pub fn state(&self) -> &State {
        &self.wl.state
    }

    /// Whether the compositor offers `ext_workspace_manager_v1`.
    pub fn has_workspaces(&self) -> bool {
        self.wl.workspace_manager.is_some()
    }

    /// Whether the compositor offers a window list.
    pub fn has_windows(&self) -> bool {
        self.wl.windows
    }

    /// Whether a batch completed since the last call.
    pub fn take_published(&mut self) -> bool {
        std::mem::take(&mut self.wl.published)
    }

    pub fn roundtrip(&mut self) -> Result<()> {
        roundtrip(&self.connection, &mut self.queue, &mut self.wl)
    }

    /// Sends `requests`, in order.
    pub fn execute(&mut self, requests: &[Request]) -> Result<()> {
        for request in requests {
            match *request {
                Request::Activate { workspace } => self.wl.workspaces.get(&workspace).ok_or_else(|| anyhow!("{}", mandara_i18n::t("Workspace not found")))?.activate(),
                Request::Commit => self.wl.workspace_manager.as_ref().ok_or_else(|| anyhow!("{}", mandara_i18n::t("Not supported by this compositor")))?.commit(),
                Request::MoveWindow { window, workspace, output } => {
                    let manager = self.wl.cosmic_manager.as_ref().filter(|m| m.version() >= COSMIC_MOVE_SINCE).ok_or_else(|| anyhow!("{}", mandara_i18n::t("Not supported by this compositor")))?;
                    let handle = self.wl.cosmic_windows.get(&window).ok_or_else(|| anyhow!("{}", mandara_i18n::t("The window is no longer available")))?;
                    let target = self.wl.workspaces.get(&workspace).ok_or_else(|| anyhow!("{}", mandara_i18n::t("Workspace not found")))?;
                    let output = self.wl.outputs.values().map(|(o, _)| o).find(|o| o.id().protocol_id() == output).ok_or_else(|| anyhow!("ext-workspace: the output is gone"))?;
                    manager.move_to_ext_workspace(handle, target, output);
                }
            }
        }
        self.connection.flush()?;
        Ok(())
    }

    /// Whether windows can be moved between workspaces here.
    pub fn can_move_windows(&self) -> bool {
        self.wl.cosmic_manager.as_ref().is_some_and(|m| m.version() >= COSMIC_MOVE_SINCE)
    }

    pub fn window_action(&mut self, id: u64, action: &WindowAction) -> Result<()> {
        let gone = || anyhow!("{}", mandara_i18n::t("The window is no longer available"));
        let key = u32::try_from(id).map_err(|_| gone())?;
        if let Some(h) = self.wl.wlr_windows.get(&key) {
            match *action {
                WindowAction::Activate => {
                    h.unset_minimized();
                    h.activate(self.wl.seat.as_ref().context("no seat")?);
                }
                WindowAction::Close => h.close(),
                WindowAction::Minimized(on) => if on { h.set_minimized() } else { h.unset_minimized() },
                WindowAction::Maximized(on) => if on { h.set_maximized() } else { h.unset_maximized() },
                WindowAction::Fullscreen(on) if h.version() >= 2 => if on { h.set_fullscreen(None) } else { h.unset_fullscreen() },
                WindowAction::Fullscreen(_) => bail!("{}", mandara_i18n::t("Not supported by this compositor")),
            }
        } else {
            let h = self.wl.cosmic_windows.get(&key).ok_or_else(gone)?;
            let m = self.wl.cosmic_manager.as_ref().ok_or_else(|| anyhow!("{}", mandara_i18n::t("Not supported by this compositor")))?;
            match *action {
                WindowAction::Activate => {
                    m.unset_minimized(h);
                    m.activate(h, self.wl.seat.as_ref().context("no seat")?);
                }
                WindowAction::Close => m.close(h),
                WindowAction::Minimized(on) => if on { m.set_minimized(h) } else { m.unset_minimized(h) },
                WindowAction::Maximized(on) => if on { m.set_maximized(h) } else { m.unset_maximized(h) },
                WindowAction::Fullscreen(on) => if on { m.set_fullscreen(h, None) } else { m.unset_fullscreen(h) },
            }
        }
        self.connection.flush()?;
        Ok(())
    }

    /// Reports idle after `milliseconds` without input (0 = off).
    pub fn idle_timeout(&mut self, milliseconds: u32) -> Result<()> {
        if let Some(previous) = self.wl.idle_note.take() {
            previous.destroy();
        }
        self.wl.apply(Event::Idle(false));
        if milliseconds > 0
            && let (Some(notifier), Some(seat)) = (&self.wl.idle_notifier, &self.wl.seat)
        {
            self.wl.idle_note = Some(notifier.get_idle_notification(milliseconds, seat, &self.queue.handle(), ()));
        }
        self.connection.flush()?;
        Ok(())
    }

    /// Sleeps until an event or a wake-up arrives; `false` once `wake`'s
    /// writer is gone (the owner stopped).
    pub fn wait(&mut self, wake: Option<&PipeReader>) -> Result<bool> {
        use rustix::event::{PollFd, PollFlags, poll};
        if self.queue.dispatch_pending(&mut self.wl)? > 0 {
            return Ok(true);
        }
        self.connection.flush()?;
        let Some(guard) = self.queue.prepare_read() else { return Ok(true) };
        let fd = guard.connection_fd();
        let mut fds = vec![PollFd::new(&fd, PollFlags::IN)];
        if let Some(wake) = wake {
            fds.push(PollFd::new(wake, PollFlags::IN));
        }
        match poll(&mut fds, None) {
            Ok(_) if fds[0].revents().intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL) => bail!("{}", mandara_i18n::t("The compositor connection closed")),
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => return Ok(true),
            Err(err) => return Err(err.into()),
        }
        let wl_ready = fds[0].revents().contains(PollFlags::IN);
        let wake_ready = fds.get(1).is_some_and(|fd| fd.revents().contains(PollFlags::IN));
        let wake_closed = fds.get(1).is_some_and(|fd| fd.revents().intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL));
        drop(fds);
        if wl_ready {
            guard.read()?;
            self.queue.dispatch_pending(&mut self.wl)?;
        } else {
            drop(guard);
        }
        if let Some(wake) = wake
            && wake_ready
        {
            let mut bytes = [0u8; 64];
            if matches!((&*wake).read(&mut bytes), Ok(0)) {
                return Ok(false);
            }
        }
        Ok(!(wake_closed && !wake_ready))
    }
}

/// A round trip bounded by [`crate::REQUEST_TIMEOUT`]: a stalled compositor
/// cannot hold the caller.
fn roundtrip(conn: &Connection, queue: &mut EventQueue<Wl>, wl: &mut Wl) -> Result<()> {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    let synced = wl.synced;
    conn.display().sync(&queue.handle(), ());
    let deadline = Instant::now() + crate::REQUEST_TIMEOUT;
    loop {
        queue.dispatch_pending(wl)?;
        if wl.synced != synced {
            return Ok(());
        }
        conn.flush()?;
        let Some(guard) = queue.prepare_read() else { continue };
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("{}", mandara_i18n::t("The compositor did not respond in time"));
        }
        let timeout = Timespec { tv_sec: remaining.as_secs() as i64, tv_nsec: i64::from(remaining.subsec_nanos()) };
        let fd = guard.connection_fd();
        let mut fds = [PollFd::new(&fd, PollFlags::IN)];
        match poll(&mut fds, Some(&timeout)) {
            Ok(0) => bail!("{}", mandara_i18n::t("The compositor did not respond in time")),
            Ok(_) if fds[0].revents().intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL) => bail!("{}", mandara_i18n::t("The compositor connection closed")),
            Ok(_) => {
                guard.read()?;
            }
            Err(rustix::io::Errno::INTR) => continue,
            Err(err) => return Err(err.into()),
        }
    }
}

/// A `WEnum` bitfield's bits, known or not.
fn bits<T>(value: WEnum<T>, known: fn(&T) -> u32) -> u32 {
    match value {
        WEnum::Value(v) => known(&v),
        WEnum::Unknown(raw) => raw,
    }
}

/// An array argument of native-endian `u32`s.
pub fn u32s(raw: &[u8]) -> Vec<u32> {
    raw.as_chunks::<4>().0.iter().map(|c| u32::from_ne_bytes(*c)).collect()
}

fn window_state(window: u32, raw: &[u8]) -> Event {
    let states = u32s(raw);
    let has = |s| states.contains(&s);
    Event::WindowState { window, activated: has(STATE_ACTIVATED), minimized: has(STATE_MINIMIZED), maximized: has(STATE_MAXIMIZED), fullscreen: has(STATE_FULLSCREEN) }
}

impl Dispatch<wl_callback::WlCallback, ()> for Wl {
    fn event(wl: &mut Self, _: &wl_callback::WlCallback, _: wl_callback::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        wl.synced = wl.synced.wrapping_add(1);
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Wl {
    fn event(wl: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &GlobalListContents, _: &Connection, qh: &QueueHandle<Self>) {
        match event {
            wl_registry::Event::Global { name, interface, version } if interface == wl_output::WlOutput::interface().name => wl.watch_output(registry, name, version, qh),
            wl_registry::Event::GlobalRemove { name } => {
                if let Some((output, xdg)) = wl.outputs.remove(&name) {
                    let id = output.id().protocol_id();
                    if let Some(xdg) = xdg {
                        xdg.destroy();
                    }
                    if output.version() >= 3 {
                        output.release();
                    }
                    wl.apply(Event::OutputGone { output: id });
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for Wl {
    fn event(wl: &mut Self, proxy: &wl_output::WlOutput, event: wl_output::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let output = proxy.id().protocol_id();
        match event {
            wl_output::Event::Name { name } => wl.apply(Event::OutputName { output, name }),
            wl_output::Event::Scale { factor } => wl.apply(Event::OutputScale { output, factor }),
            wl_output::Event::Mode { flags, width, height, .. } if flags.into_result().is_ok_and(|f| f.contains(wl_output::Mode::Current)) => wl.apply(Event::OutputMode { output, width, height }),
            wl_output::Event::Done => wl.apply(Event::OutputDone),
            _ => {}
        }
    }
}

impl Dispatch<xdg_manager::ZxdgOutputManagerV1, ()> for Wl {
    fn event(_: &mut Self, _: &xdg_manager::ZxdgOutputManagerV1, _: xdg_manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<xdg_output::ZxdgOutputV1, u32> for Wl {
    fn event(wl: &mut Self, _: &xdg_output::ZxdgOutputV1, event: xdg_output::Event, output: &u32, _: &Connection, _: &QueueHandle<Self>) {
        let output = *output;
        match event {
            xdg_output::Event::LogicalPosition { x, y } => wl.apply(Event::OutputPosition { output, x, y }),
            xdg_output::Event::LogicalSize { width, height } => wl.apply(Event::OutputSize { output, width, height }),
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Wl {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<manager::ExtWorkspaceManagerV1, ()> for Wl {
    fn event(wl: &mut Self, _: &manager::ExtWorkspaceManagerV1, event: manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            manager::Event::WorkspaceGroup { workspace_group } => {
                let group = workspace_group.id().protocol_id();
                wl.groups.insert(group, workspace_group);
                wl.apply(Event::GroupCreated { group });
            }
            manager::Event::Workspace { workspace } => {
                let id = workspace.id().protocol_id();
                wl.workspaces.insert(id, workspace);
                wl.apply(Event::WorkspaceCreated { workspace: id });
            }
            manager::Event::Done => wl.apply(Event::Done),
            manager::Event::Finished => {
                wl.workspace_manager = None;
            }
            _ => {}
        }
    }

    event_created_child!(Wl, manager::ExtWorkspaceManagerV1, [
        manager::EVT_WORKSPACE_GROUP_OPCODE => (group::ExtWorkspaceGroupHandleV1, ()),
        manager::EVT_WORKSPACE_OPCODE => (workspace::ExtWorkspaceHandleV1, ()),
    ]);
}

impl Dispatch<group::ExtWorkspaceGroupHandleV1, ()> for Wl {
    fn event(wl: &mut Self, proxy: &group::ExtWorkspaceGroupHandleV1, event: group::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let group = proxy.id().protocol_id();
        match event {
            group::Event::OutputEnter { output } => wl.apply(Event::GroupOutput { group, output: output.id().protocol_id(), entered: true }),
            group::Event::OutputLeave { output } => wl.apply(Event::GroupOutput { group, output: output.id().protocol_id(), entered: false }),
            group::Event::WorkspaceEnter { workspace } => wl.apply(Event::GroupWorkspace { group, workspace: workspace.id().protocol_id(), entered: true }),
            group::Event::WorkspaceLeave { workspace } => wl.apply(Event::GroupWorkspace { group, workspace: workspace.id().protocol_id(), entered: false }),
            group::Event::Removed => {
                wl.apply(Event::GroupRemoved { group });
                if let Some(handle) = wl.groups.remove(&group) {
                    handle.destroy();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<workspace::ExtWorkspaceHandleV1, ()> for Wl {
    fn event(wl: &mut Self, proxy: &workspace::ExtWorkspaceHandleV1, event: workspace::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let id = proxy.id().protocol_id();
        match event {
            workspace::Event::Name { name } => wl.apply(Event::WorkspaceName { workspace: id, name }),
            workspace::Event::Coordinates { coordinates } => wl.apply(Event::WorkspaceCoordinates { workspace: id, coordinates: u32s(&coordinates) }),
            workspace::Event::State { state } => wl.apply(Event::WorkspaceState { workspace: id, state: bits(state, workspace::State::bits) }),
            workspace::Event::Capabilities { capabilities } => wl.apply(Event::WorkspaceCapabilities { workspace: id, capabilities: bits(capabilities, workspace::WorkspaceCapabilities::bits) }),
            workspace::Event::Removed => {
                wl.apply(Event::WorkspaceRemoved { workspace: id });
                if let Some(handle) = wl.workspaces.remove(&id) {
                    handle.destroy();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wlr_manager::ZwlrForeignToplevelManagerV1, ()> for Wl {
    fn event(wl: &mut Self, _: &wlr_manager::ZwlrForeignToplevelManagerV1, event: wlr_manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wlr_manager::Event::Toplevel { toplevel } = event {
            wl.wlr_windows.insert(toplevel.id().protocol_id(), toplevel);
        }
    }

    event_created_child!(Wl, wlr_manager::ZwlrForeignToplevelManagerV1, [
        wlr_manager::EVT_TOPLEVEL_OPCODE => (wlr_handle::ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<wlr_handle::ZwlrForeignToplevelHandleV1, ()> for Wl {
    fn event(wl: &mut Self, proxy: &wlr_handle::ZwlrForeignToplevelHandleV1, event: wlr_handle::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let window = proxy.id().protocol_id();
        match event {
            wlr_handle::Event::Title { title } => wl.apply(Event::WindowTitle { window, title }),
            wlr_handle::Event::AppId { app_id } => wl.apply(Event::WindowAppId { window, app_id }),
            wlr_handle::Event::State { state } => wl.apply(window_state(window, &state)),
            wlr_handle::Event::OutputEnter { output } => wl.apply(Event::WindowOutput { window, output: output.id().protocol_id(), entered: true }),
            wlr_handle::Event::OutputLeave { output } => wl.apply(Event::WindowOutput { window, output: output.id().protocol_id(), entered: false }),
            wlr_handle::Event::Done => wl.apply(Event::WindowDone),
            wlr_handle::Event::Closed => {
                wl.apply(Event::WindowClosed { window });
                if let Some(handle) = wl.wlr_windows.remove(&window) {
                    handle.destroy();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ext_list::ExtForeignToplevelListV1, ()> for Wl {
    fn event(wl: &mut Self, _: &ext_list::ExtForeignToplevelListV1, event: ext_list::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let ext_list::Event::Toplevel { toplevel } = event {
            let window = toplevel.id().protocol_id();
            if let Some(info) = &wl.cosmic_info {
                wl.cosmic_windows.insert(window, info.get_cosmic_toplevel(&toplevel, qh, window));
            }
            wl.ext_windows.insert(window, toplevel);
        }
    }

    event_created_child!(Wl, ext_list::ExtForeignToplevelListV1, [
        ext_list::EVT_TOPLEVEL_OPCODE => (ext_handle::ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ext_handle::ExtForeignToplevelHandleV1, ()> for Wl {
    fn event(wl: &mut Self, proxy: &ext_handle::ExtForeignToplevelHandleV1, event: ext_handle::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let window = proxy.id().protocol_id();
        match event {
            ext_handle::Event::Title { title } => wl.apply(Event::WindowTitle { window, title }),
            ext_handle::Event::AppId { app_id } => wl.apply(Event::WindowAppId { window, app_id }),
            ext_handle::Event::Done => wl.apply(Event::WindowDone),
            ext_handle::Event::Closed => {
                wl.apply(Event::WindowClosed { window });
                if let Some(cosmic) = wl.cosmic_windows.remove(&window) {
                    cosmic.destroy();
                }
                if let Some(handle) = wl.ext_windows.remove(&window) {
                    handle.destroy();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<cosmic_info::ZcosmicToplevelInfoV1, ()> for Wl {
    fn event(wl: &mut Self, _: &cosmic_info::ZcosmicToplevelInfoV1, event: cosmic_info::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            cosmic_info::Event::Done => wl.apply(Event::WindowDone),
            cosmic_info::Event::Finished => wl.cosmic_info = None,
            _ => {}
        }
    }

    // Only version 1 announces handles itself; Mandara binds 2 and up.
    event_created_child!(Wl, cosmic_info::ZcosmicToplevelInfoV1, [
        cosmic_info::EVT_TOPLEVEL_OPCODE => (cosmic_handle::ZcosmicToplevelHandleV1, 0),
    ]);
}

impl Dispatch<cosmic_handle::ZcosmicToplevelHandleV1, u32> for Wl {
    fn event(wl: &mut Self, _: &cosmic_handle::ZcosmicToplevelHandleV1, event: cosmic_handle::Event, window: &u32, _: &Connection, _: &QueueHandle<Self>) {
        let window = *window;
        match event {
            cosmic_handle::Event::State { state } => wl.apply(window_state(window, &state)),
            cosmic_handle::Event::OutputEnter { output } => wl.apply(Event::WindowOutput { window, output: output.id().protocol_id(), entered: true }),
            cosmic_handle::Event::OutputLeave { output } => wl.apply(Event::WindowOutput { window, output: output.id().protocol_id(), entered: false }),
            cosmic_handle::Event::ExtWorkspaceEnter { workspace } => wl.apply(Event::WindowWorkspace { window, workspace: workspace.id().protocol_id(), entered: true }),
            cosmic_handle::Event::ExtWorkspaceLeave { workspace } => wl.apply(Event::WindowWorkspace { window, workspace: workspace.id().protocol_id(), entered: false }),
            cosmic_handle::Event::Geometry { output, x, y, width, height } => {
                // Relative to `output`: the window's first output places it.
                let output = output.id().protocol_id();
                let first = wl.state.windows.get(&window).and_then(|w| w.outputs.first().copied());
                if first.is_none_or(|f| f == output) {
                    let (ox, oy) = wl.output_offset(output);
                    wl.apply(Event::WindowGeometry { window, rect: (ox.saturating_add(x), oy.saturating_add(y), width, height) });
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<cosmic_manager::ZcosmicToplevelManagerV1, ()> for Wl {
    fn event(_: &mut Self, _: &cosmic_manager::ZcosmicToplevelManagerV1, _: cosmic_manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<idle_notifier::ExtIdleNotifierV1, ()> for Wl {
    fn event(_: &mut Self, _: &idle_notifier::ExtIdleNotifierV1, _: idle_notifier::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<idle_notification::ExtIdleNotificationV1, ()> for Wl {
    fn event(wl: &mut Self, _: &idle_notification::ExtIdleNotificationV1, event: idle_notification::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            idle_notification::Event::Idled => wl.apply(Event::Idle(true)),
            idle_notification::Event::Resumed => wl.apply(Event::Idle(false)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrays_and_states_decode_like_the_protocol_says() {
        let raw: Vec<u8> = [STATE_ACTIVATED, STATE_MAXIMIZED].iter().flat_map(|v| v.to_ne_bytes()).collect();
        assert_eq!(window_state(5, &raw), Event::WindowState { window: 5, activated: true, minimized: false, maximized: true, fullscreen: false });
        assert!(u32s(&[1, 2, 3]).is_empty(), "a partial value is ignored");
        assert_eq!(bits(WEnum::<workspace::State>::Unknown(6), workspace::State::bits), 6);
        assert_eq!(bits(WEnum::Value(workspace::State::Active | workspace::State::Hidden), workspace::State::bits), super::super::state::STATE_ACTIVE | super::super::state::STATE_HIDDEN);
    }
}
