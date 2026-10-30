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

/// The GPUI display id of a [`OutputGeometry`] by name. GPUI derives a
/// display's [`gpui::PlatformDisplay::uuid`] from its `wl_output` name
/// (UUID v5, DNS namespace), so this matches it.
pub fn output_uuid(name: &str) -> uuid::Uuid {
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, name.as_bytes())
}

/// One GPUI display as far as pairing needs it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayKey {
    pub id: gpui::DisplayId,
    /// From the output name, when the compositor sent one.
    pub uuid: Option<uuid::Uuid>,
    /// Top-left corner in the compositor's layout.
    pub origin: (i32, i32),
}

impl DisplayKey {
    pub fn of(display: &dyn gpui::PlatformDisplay) -> Self {
        let b = display.bounds();
        Self { id: display.id(), uuid: display.uuid().ok(), origin: (f32::from(b.origin.x).round() as i32, f32::from(b.origin.y).round() as i32) }
    }
}

/// Pairs every display with its output's geometry: by name, else by
/// position, else by order. GPUI and the xdg-output query bind outputs on
/// separate connections, so their lists can come in different orders (a
/// compositor reload re-announces outputs), and pairing by index gave each
/// surface the other monitor's size.
pub fn pair_displays(displays: &[DisplayKey], outputs: &[OutputGeometry]) -> Vec<(gpui::DisplayId, Option<OutputGeometry>)> {
    let mut taken = vec![false; outputs.len()];
    let pick = |found: Option<usize>, taken: &mut Vec<bool>| {
        let i = found.filter(|i| !taken[*i])?;
        taken[i] = true;
        Some(outputs[i].clone())
    };
    // Names first, for every display, so a position or order match can't
    // claim an output that belongs to another display by name.
    let mut paired: Vec<Option<OutputGeometry>> = displays.iter().map(|d| pick(d.uuid.and_then(|u| outputs.iter().position(|o| !o.name.is_empty() && output_uuid(&o.name) == u)), &mut taken)).collect();
    for (d, slot) in displays.iter().zip(paired.iter_mut()).filter(|(_, s)| s.is_none()) {
        *slot = pick(outputs.iter().position(|o| (o.x, o.y) == d.origin), &mut taken);
    }
    // Then list order, and finally any output still free.
    for (i, slot) in paired.iter_mut().enumerate().filter(|(_, s)| s.is_none()) {
        *slot = pick((i < outputs.len()).then_some(i), &mut taken);
        if slot.is_none() {
            let free = taken.iter().position(|t| !t);
            *slot = pick(free, &mut taken);
        }
    }
    displays.iter().map(|d| d.id).zip(paired).collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn geo(name: &str, x: i32, y: i32, w: i32, h: i32) -> OutputGeometry {
        OutputGeometry { name: name.into(), x, y, width: w, height: h }
    }

    fn display(id: u64, name: Option<&str>, origin: (i32, i32)) -> DisplayKey {
        DisplayKey { id: gpui::DisplayId::new(id), uuid: name.map(output_uuid), origin }
    }

    /// Issue #8: after `hyprctl reload` GPUI listed HDMI-A-1 first while
    /// xdg-output listed eDP-1 first; pairing by index swapped the sizes.
    #[test]
    fn displays_pair_by_name_whatever_the_order() {
        let outputs = [geo("eDP-1", 0, 0, 1366, 768), geo("HDMI-A-1", 1366, 0, 1920, 1080)];
        let displays = [display(7, Some("HDMI-A-1"), (1366, 0)), display(3, Some("eDP-1"), (0, 0))];
        let paired = pair_displays(&displays, &outputs);
        assert_eq!(paired[0].1.as_ref().map(|g| (g.name.as_str(), g.width)), Some(("HDMI-A-1", 1920)));
        assert_eq!(paired[1].1.as_ref().map(|g| (g.name.as_str(), g.width)), Some(("eDP-1", 1366)));
        assert_eq!(paired[0].0, gpui::DisplayId::new(7));
    }

    #[test]
    fn without_names_position_then_order_decide() {
        let outputs = [geo("eDP-1", 0, 0, 1366, 768), geo("HDMI-A-1", -1920, -570, 1920, 1080)];
        // No names: the negative-coordinates layout from the report.
        let paired = pair_displays(&[display(1, None, (-1920, -570)), display(2, None, (0, 0))], &outputs);
        assert_eq!(paired.iter().map(|(_, g)| g.as_ref().unwrap().width).collect::<Vec<_>>(), [1920, 1366]);
        // Nothing to go by: list order, and every output used once.
        let paired = pair_displays(&[display(1, None, (5, 5)), display(2, None, (9, 9))], &outputs);
        assert_eq!(paired.iter().map(|(_, g)| g.as_ref().unwrap().name.as_str()).collect::<Vec<_>>(), ["eDP-1", "HDMI-A-1"]);
        // More displays than measured outputs: the extra one gets none.
        let paired = pair_displays(&[display(1, Some("eDP-1"), (0, 0)), display(2, Some("DP-9"), (4000, 0)), display(3, None, (7, 7))], &outputs[..1]);
        assert!(paired[0].1.is_some() && paired[1].1.is_none() && paired[2].1.is_none());
        assert_eq!(output_uuid("eDP-1"), uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"eDP-1"));
    }
}
