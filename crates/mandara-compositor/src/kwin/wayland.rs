//! Native Plasma protocol connections and event handlers.

use std::collections::BTreeMap;
use std::time::Instant;

use anyhow::{Context as _, Result, bail};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_callback, wl_output, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1 as xdg_manager, zxdg_output_v1 as xdg_output};
use wayland_protocols::ext::idle_notify::v1::client::{ext_idle_notification_v1 as idle_notification, ext_idle_notifier_v1 as idle_notifier};
use wayland_protocols_plasma::plasma_window_management::client::{org_kde_plasma_window as window, org_kde_plasma_window_management as manager};

use super::protocol::{org_kde_plasma_virtual_desktop as desktop, org_kde_plasma_virtual_desktop_management as desktops};
use super::state::{Desktop, State};

/// UUID-based window access and client geometry are available by version 18.
const WINDOW_VERSION: u32 = 18;
/// UUID-based window discovery and lookup arrived together in version 13.
const MIN_WINDOW_VERSION: u32 = 13;
/// Per-output desktop activation arrived in version 4.
const DESKTOP_VERSION: u32 = 4;
/// Names and logical geometry are available at these protocol versions.
const OUTPUT_VERSION: u32 = 4;
const XDG_VERSION: u32 = 3;
/// Seat capabilities needed by idle notifications; these protocols are stable.
const SEAT_VERSION: u32 = 8;
const IDLE_VERSION: u32 = 1;
const DPMS_VERSION: u32 = 1;

pub(super) struct Wl {
    pub state: State,
    pub windows: BTreeMap<String, window::OrgKdePlasmaWindow>,
    pub desktops: BTreeMap<String, desktop::OrgKdePlasmaVirtualDesktop>,
    pub seat: Option<wl_seat::WlSeat>,
    pub idle_notifier: Option<idle_notifier::ExtIdleNotifierV1>,
    pub idle_note: Option<idle_notification::ExtIdleNotificationV1>,
    pub power: super::power::Power,
    outputs: BTreeMap<u32, (wl_output::WlOutput, Option<xdg_output::ZxdgOutputV1>)>,
    xdg: Option<xdg_manager::ZxdgOutputManagerV1>,
    bound: bool,
    synced: u64,
}

impl Wl {
    fn watch_output(&mut self, registry: &wl_registry::WlRegistry, name: u32, version: u32, qh: &QueueHandle<Self>) {
        if self.outputs.contains_key(&name) { return; }
        let output: wl_output::WlOutput = registry.bind(name, version.min(OUTPUT_VERSION), qh, name);
        self.state.outputs.entry(name).or_default();
        self.power.watch(&output, name, qh);
        let xdg = self.xdg.as_ref().map(|xdg| xdg.get_xdg_output(&output, qh, name));
        self.outputs.insert(name, (output, xdg));
    }
}

pub(super) fn open() -> Result<(Connection, EventQueue<Wl>, Wl)> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<Wl>(&conn)?;
    let qh = queue.handle();
    let _desktops = globals.bind::<desktops::OrgKdePlasmaVirtualDesktopManagement, _, _>(&qh, 1..=DESKTOP_VERSION, ()).context("org_kde_plasma_virtual_desktop_management")?;
    let _windows: manager::OrgKdePlasmaWindowManagement = globals.bind(&qh, MIN_WINDOW_VERSION..=WINDOW_VERSION, ()).context("org_kde_plasma_window_management")?;
    let xdg = Some(globals.bind(&qh, 1..=XDG_VERSION, ()).context("zxdg_output_manager_v1")?);
    let seat = globals.bind(&qh, 1..=SEAT_VERSION, ()).ok();
    let idle_notifier = globals.bind(&qh, 1..=IDLE_VERSION, ()).ok();
    let power = super::power::Power::new(globals.bind(&qh, 1..=DPMS_VERSION, ()).ok());
    let mut state = Wl { state: State::default(), windows: BTreeMap::new(), desktops: BTreeMap::new(), outputs: BTreeMap::new(), seat, idle_notifier, idle_note: None, power, xdg, bound: true, synced: 0 };
    for global in globals.contents().clone_list() {
        if global.interface == wl_output::WlOutput::interface().name {
            state.watch_output(globals.registry(), global.name, global.version, &qh);
        }
    }
    // Desktop and window events create child objects; their initial state
    // follows the second sync rather than the manager's first sync.
    roundtrip(&conn, &mut queue, &mut state)?;
    roundtrip(&conn, &mut queue, &mut state)?;
    Ok((conn, queue, state))
}

/// Bounds command/query round trips; a stalled compositor cannot hold a caller.
pub(super) fn roundtrip(conn: &Connection, queue: &mut EventQueue<Wl>, state: &mut Wl) -> Result<()> {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    let synced = state.synced;
    conn.display().sync(&queue.handle(), ());
    let deadline = Instant::now() + crate::REQUEST_TIMEOUT;
    loop {
        queue.dispatch_pending(state)?;
        if state.synced != synced {
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
            Ok(_) => { guard.read()?; }
            Err(rustix::io::Errno::INTR) => continue,
            Err(err) => return Err(err.into()),
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for Wl {
    fn event(state: &mut Self, _: &wl_callback::WlCallback, _: wl_callback::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        state.synced = state.synced.wrapping_add(1);
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Wl {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<idle_notifier::ExtIdleNotifierV1, ()> for Wl {
    fn event(_: &mut Self, _: &idle_notifier::ExtIdleNotifierV1, _: idle_notifier::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<idle_notification::ExtIdleNotificationV1, ()> for Wl {
    fn event(state: &mut Self, _: &idle_notification::ExtIdleNotificationV1, event: idle_notification::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            idle_notification::Event::Idled => state.state.idle = true,
            idle_notification::Event::Resumed => state.state.idle = false,
            _ => {}
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Wl {
    fn event(state: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &GlobalListContents, _: &Connection, qh: &QueueHandle<Self>) {
        match event {
            wl_registry::Event::Global { name, interface, version } if state.bound && interface == wl_output::WlOutput::interface().name => state.watch_output(registry, name, version, qh),
            wl_registry::Event::GlobalRemove { name } => {
                state.power.remove(name);
                if let Some((output, xdg)) = state.outputs.remove(&name) {
                    if let Some(xdg) = xdg { xdg.destroy(); }
                    if output.version() >= 3 { output.release(); }
                }
                if let Some(output) = state.state.outputs.remove(&name) {
                    state.state.active_outputs.remove(&output.name);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for Wl {
    fn event(state: &mut Self, _: &wl_output::WlOutput, event: wl_output::Event, id: &u32, _: &Connection, _: &QueueHandle<Self>) {
        let output = state.state.outputs.entry(*id).or_default();
        match event {
            wl_output::Event::Name { name } => output.name = name,
            wl_output::Event::Scale { factor } => output.scale = factor,
            wl_output::Event::Geometry { transform, .. } => output.rotated = transform.into_result().is_ok_and(|t| matches!(t, wl_output::Transform::_90 | wl_output::Transform::_270 | wl_output::Transform::Flipped90 | wl_output::Transform::Flipped270)),
            wl_output::Event::Mode { flags, width, height, .. } if flags.into_result().is_ok_and(|f| f.contains(wl_output::Mode::Current)) => {
                output.physical_width = width;
                output.physical_height = height;
            }
            _ => {}
        }
    }
}

impl Dispatch<xdg_manager::ZxdgOutputManagerV1, ()> for Wl {
    fn event(_: &mut Self, _: &xdg_manager::ZxdgOutputManagerV1, _: xdg_manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<xdg_output::ZxdgOutputV1, u32> for Wl {
    fn event(state: &mut Self, _: &xdg_output::ZxdgOutputV1, event: xdg_output::Event, id: &u32, _: &Connection, _: &QueueHandle<Self>) {
        let output = state.state.outputs.entry(*id).or_default();
        match event {
            xdg_output::Event::Name { name } => output.name = name,
            xdg_output::Event::LogicalPosition { x, y } => { output.x = x; output.y = y; }
            xdg_output::Event::LogicalSize { width, height } => { output.width = width; output.height = height; }
            _ => {}
        }
    }
}

impl Dispatch<desktops::OrgKdePlasmaVirtualDesktopManagement, ()> for Wl {
    fn event(state: &mut Self, proxy: &desktops::OrgKdePlasmaVirtualDesktopManagement, event: desktops::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        match event {
            desktops::Event::DesktopCreated { desktop_id, position } => {
                let desktop = proxy.get_virtual_desktop(desktop_id.clone(), qh, desktop_id.clone());
                state.state.desktops.insert(desktop_id.clone(), Desktop { position, ..Default::default() });
                state.desktops.insert(desktop_id, desktop);
            }
            desktops::Event::DesktopRemoved { desktop_id } => {
                state.state.desktops.remove(&desktop_id);
                state.desktops.remove(&desktop_id);
                state.state.active_outputs.retain(|_, id| id != &desktop_id);
                if state.state.active_global.as_ref() == Some(&desktop_id) { state.state.active_global = None; }
            }
            _ => {}
        }
    }
}

impl Dispatch<desktop::OrgKdePlasmaVirtualDesktop, String> for Wl {
    fn event(state: &mut Self, _: &desktop::OrgKdePlasmaVirtualDesktop, event: desktop::Event, uuid: &String, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            desktop::Event::Name { name } => state.state.desktops.entry(uuid.clone()).or_default().name = name,
            desktop::Event::Position { index } => state.state.desktops.entry(uuid.clone()).or_default().position = index,
            desktop::Event::Activated => state.state.active_global = Some(uuid.clone()),
            desktop::Event::Deactivated if state.state.active_global.as_ref() == Some(uuid) => state.state.active_global = None,
            desktop::Event::OutputEntered { output_name } => { state.state.active_outputs.insert(output_name, uuid.clone()); }
            _ => {}
        }
    }
}

impl Dispatch<manager::OrgKdePlasmaWindowManagement, ()> for Wl {
    fn event(state: &mut Self, proxy: &manager::OrgKdePlasmaWindowManagement, event: manager::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let manager::Event::WindowWithUuid { uuid, .. } = event {
            let window = proxy.get_window_by_uuid(uuid.clone(), qh, uuid.clone());
            state.windows.insert(uuid.clone(), window);
            state.state.windows.entry(uuid).or_default();
        }
    }
}

impl Dispatch<window::OrgKdePlasmaWindow, String> for Wl {
    fn event(state: &mut Self, proxy: &window::OrgKdePlasmaWindow, event: window::Event, uuid: &String, _: &Connection, _: &QueueHandle<Self>) {
        if matches!(event, window::Event::Unmapped) {
            state.windows.remove(uuid);
            state.state.windows.remove(uuid);
            proxy.destroy();
            return;
        }
        let window = state.state.windows.entry(uuid.clone()).or_default();
        match event {
            window::Event::TitleChanged { title } => window.title = title,
            window::Event::AppIdChanged { app_id } => window.app_id = app_id,
            window::Event::StateChanged { flags } => window.flags = flags,
            window::Event::VirtualDesktopEntered { id } => { if !window.desktops.contains(&id) { window.desktops.push(id); } }
            window::Event::VirtualDesktopLeft { is: id } => window.desktops.retain(|d| *d != id),
            window::Event::Geometry { x, y, width, height } => window.geometry = Some((x, y, width.min(i32::MAX as u32) as i32, height.min(i32::MAX as u32) as i32)),
            window::Event::ParentWindow { parent } => window.parent = parent.and_then(|p| p.data::<String>().cloned()),
            _ => {}
        }
    }
}
