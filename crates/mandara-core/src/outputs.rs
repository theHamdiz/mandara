//! Logical output geometry via `zxdg_output_manager_v1`.
//!
//! GPUI derives display bounds from `wl_output`'s integer scale, which is
//! wrong under fractional scaling (a 4K panel at 1.5× reports 1920 px instead
//! of its true 2560 logical px). Layer surfaces must be sized in logical
//! pixels, so we ask the compositor directly with a short-lived connection.

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};

/// Logical geometry of one output.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutputGeometry {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Default)]
struct State {
    outputs: Vec<(wl_output::WlOutput, OutputGeometry)>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(_: &mut Self, _: &wl_output::WlOutput, _: wl_output::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<zxdg_output_manager_v1::ZxdgOutputManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &zxdg_output_manager_v1::ZxdgOutputManagerV1,
        _: zxdg_output_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, usize> for State {
    fn event(
        state: &mut Self,
        _: &zxdg_output_v1::ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some((_, geo)) = state.outputs.get_mut(*index) else { return };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => {
                geo.x = x;
                geo.y = y;
            }
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                geo.width = width;
                geo.height = height;
            }
            zxdg_output_v1::Event::Name { name } => geo.name = name,
            _ => {}
        }
    }
}

/// Queries every output's logical geometry, in registry order. Returns an
/// empty list if the compositor lacks `zxdg_output_manager_v1`.
pub fn query_outputs() -> anyhow::Result<Vec<OutputGeometry>> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();
    let mut state = State::default();

    let manager: zxdg_output_manager_v1::ZxdgOutputManagerV1 = match globals.bind(&qh, 1..=3, ()) {
        Ok(manager) => manager,
        Err(_) => return Ok(Vec::new()),
    };
    let output_globals: Vec<_> = globals.contents().with_list(|list| {
        list.iter().filter(|g| g.interface == wl_output::WlOutput::interface().name).cloned().collect()
    });
    for global in output_globals {
        let output: wl_output::WlOutput = globals.registry().bind(global.name, global.version.min(4), &qh, ());
        let index = state.outputs.len();
        manager.get_xdg_output(&output, &qh, index);
        state.outputs.push((output, OutputGeometry::default()));
    }
    queue.roundtrip(&mut state)?;
    queue.roundtrip(&mut state)?;
    Ok(state.outputs.into_iter().map(|(_, geo)| geo).filter(|g| g.width > 0).collect())
}
