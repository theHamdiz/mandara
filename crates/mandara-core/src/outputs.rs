//! Logical output geometry via `zxdg_output_manager_v1`.
//!
//! GPUI derives display bounds from `wl_output`'s integer scale, which is
//! wrong under fractional scaling (a 4K panel at 1.5× reports 1920 px instead
//! of its true 2560 logical px). Layer surfaces must be sized in logical
//! pixels, so we ask the compositor directly over a connection of our own.

use std::sync::Mutex;

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
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

impl OutputGeometry {
    /// Both logical dimensions must arrive before a surface can use them.
    pub fn has_size(&self) -> bool {
        self.width > 0 && self.height > 0
    }
}

/// One `wl_output` global and what is known of it. The proxies are `None`
/// only for the entries unit tests build.
struct Tracked {
    /// The registry name, which identifies the output until it goes away.
    global: u32,
    proxies: Option<(wl_output::WlOutput, zxdg_output_v1::ZxdgOutputV1)>,
    geometry: OutputGeometry,
}

#[derive(Default)]
struct State {
    manager: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    outputs: Vec<Tracked>,
    /// An output was bound since this was last cleared: its first events
    /// are still to come.
    bound: bool,
}

impl State {
    /// Folds one xdg-output event into output `global`.
    fn on_xdg(&mut self, global: u32, event: zxdg_output_v1::Event) {
        let Some(geo) = self.outputs.iter_mut().find(|o| o.global == global).map(|o| &mut o.geometry) else { return };
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

    /// Forgets output `global` (unplugged), releasing what was bound for it.
    fn remove(&mut self, global: u32) {
        let Some(i) = self.outputs.iter().position(|o| o.global == global) else { return };
        if let Some((output, xdg)) = self.outputs.remove(i).proxies {
            xdg.destroy();
            if output.version() >= 3 {
                output.release();
            }
        }
    }

    /// Binds output `name` and asks for its logical geometry.
    fn bind(&mut self, registry: &wl_registry::WlRegistry, name: u32, version: u32, qh: &QueueHandle<Self>) {
        let Some(manager) = &self.manager else { return };
        let output: wl_output::WlOutput = registry.bind(name, version.min(4), qh, ());
        let xdg = manager.get_xdg_output(&output, qh, name);
        self.outputs.push(Tracked { global: name, proxies: Some((output, xdg)), geometry: OutputGeometry::default() });
        self.bound = true;
    }

    /// Every output that has a logical size, in the order they appeared.
    fn geometries(&self) -> Vec<OutputGeometry> {
        self.outputs.iter().map(|o| o.geometry.clone()).filter(OutputGeometry::has_size).collect()
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(state: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &GlobalListContents, _: &Connection, qh: &QueueHandle<Self>) {
        match event {
            wl_registry::Event::Global { name, interface, version } if interface == wl_output::WlOutput::interface().name => state.bind(registry, name, version, qh),
            wl_registry::Event::GlobalRemove { name } => state.remove(name),
            _ => {}
        }
    }
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

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, u32> for State {
    fn event(state: &mut Self, _: &zxdg_output_v1::ZxdgOutputV1, event: zxdg_output_v1::Event, global: &u32, _: &Connection, _: &QueueHandle<Self>) {
        state.on_xdg(*global, event);
    }
}

/// A connection kept for [`query_outputs`]: the registry keeps it informed
/// of outputs plugged and unplugged, so a query only waits for the events
/// of whatever appeared since the last one.
struct Session {
    _conn: Connection,
    queue: EventQueue<State>,
    state: State,
}

impl Session {
    /// A connection with every current output bound and its geometry read.
    fn open() -> anyhow::Result<Self> {
        let conn = Connection::connect_to_env()?;
        let (globals, queue) = registry_queue_init::<State>(&conn)?;
        let qh = queue.handle();
        let mut state = State { manager: globals.bind(&qh, 1..=3, ()).ok(), ..State::default() };
        let existing: Vec<_> = globals.contents().with_list(|list| list.iter().filter(|g| g.interface == wl_output::WlOutput::interface().name).cloned().collect());
        for global in existing {
            state.bind(globals.registry(), global.name, global.version, &qh);
        }
        let mut session = Self { _conn: conn, queue, state };
        session.settle()?;
        Ok(session)
    }

    /// Round trips until nothing new was bound, so what a previous round's
    /// outputs announced has arrived (bounded: a compositor adding outputs
    /// without end is not worth waiting for).
    fn settle(&mut self) -> anyhow::Result<()> {
        for _ in 0..MAX_ROUNDS {
            self.state.bound = false;
            self.queue.roundtrip(&mut self.state)?;
            if !self.state.bound {
                break;
            }
        }
        Ok(())
    }
}

/// Most round trips one query waits for newly appearing outputs.
const MAX_ROUNDS: usize = 8;

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
/// position, else by order when the name is unavailable. GPUI and the xdg-output query bind outputs on
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
    let unnamed = |d: &DisplayKey| d.uuid.is_none() || outputs.iter().all(|output| output.name.is_empty());
    for (d, slot) in displays.iter().zip(paired.iter_mut()).filter(|(_, s)| s.is_none()) {
        *slot = pick(outputs.iter().enumerate().position(|(i, o)| !taken[i] && (o.x, o.y) == d.origin && (d.uuid.is_none() || o.name.is_empty())), &mut taken);
    }
    // Then list order, and finally any output still free.
    for (i, slot) in paired.iter_mut().enumerate().filter(|(i, s)| s.is_none() && unnamed(&displays[*i])) {
        *slot = pick((i < outputs.len()).then_some(i), &mut taken);
        if slot.is_none() {
            let free = taken.iter().position(|t| !t);
            *slot = pick(free, &mut taken);
        }
    }
    displays.iter().map(|d| d.id).zip(paired).collect()
}

/// Queries every output's logical geometry, in the order they appeared.
/// Returns an empty list if the compositor lacks `zxdg_output_manager_v1`.
///
/// The connection is kept between calls (every caller shares it), and a
/// query costs one round trip. One that failed is dropped, and the next
/// call, or this one when the connection had been idle, connects afresh.
pub fn query_outputs() -> anyhow::Result<Vec<OutputGeometry>> {
    static SESSION: Mutex<Option<Session>> = Mutex::new(None);
    let mut slot = SESSION.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(session) = slot.as_mut() {
        match session.settle() {
            Ok(()) => return Ok(session.state.geometries()),
            // The compositor went away or restarted: this connection is
            // dead, so try a new one before giving up.
            Err(_) => *slot = None,
        }
    }
    let session = slot.insert(Session::open()?);
    Ok(session.state.geometries())
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

    fn tracked(global: u32) -> Tracked {
        Tracked { global, proxies: None, geometry: OutputGeometry::default() }
    }

    #[test]
    fn xdg_events_land_on_their_own_output_and_survive_an_unplug() {
        let mut state = State::default();
        state.outputs.extend([tracked(10), tracked(11), tracked(12)]);
        for (global, name, x, w) in [(10, "eDP-1", 0, 1366), (11, "HDMI-A-1", 1366, 1920), (12, "DP-2", 3286, 2560)] {
            state.on_xdg(global, zxdg_output_v1::Event::Name { name: name.into() });
            state.on_xdg(global, zxdg_output_v1::Event::LogicalPosition { x, y: 0 });
            state.on_xdg(global, zxdg_output_v1::Event::LogicalSize { width: w, height: 1080 });
        }
        // An event for an output already gone changes nothing.
        state.on_xdg(99, zxdg_output_v1::Event::LogicalSize { width: 1, height: 1 });
        state.remove(11);
        state.remove(11);
        // The next output keeps its own geometry, not the removed one's slot.
        state.on_xdg(12, zxdg_output_v1::Event::LogicalSize { width: 3840, height: 2160 });
        let names: Vec<_> = state.geometries().into_iter().map(|g| (g.name, g.width)).collect();
        assert_eq!(names, [("eDP-1".to_string(), 1366), ("DP-2".to_string(), 3840)]);
    }

    #[test]
    fn an_output_without_a_size_yet_is_not_listed() {
        let mut state = State::default();
        state.outputs.push(tracked(5));
        state.on_xdg(5, zxdg_output_v1::Event::LogicalSize { width: 2560, height: 0 });
        assert!(state.geometries().is_empty());
        state.on_xdg(5, zxdg_output_v1::Event::LogicalSize { width: 2560, height: 1440 });
        assert_eq!(state.geometries().len(), 1);
    }

    #[test]
    fn logical_geometry_is_ready_only_after_both_dimensions_arrive() {
        for (width, height) in [(0, 0), (2560, 0), (0, 1440), (-1, 1440), (2560, -1)] {
            assert!(!geo("DP-3", 0, 0, width, height).has_size());
        }
        assert!(geo("DP-3", 0, 0, 2560, 1440).has_size());
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

    #[test]
    fn a_stale_named_output_never_claims_an_unmatched_display() {
        let outputs = [
            OutputGeometry { name: "DP-1".into(), width: 2560, height: 1440, ..Default::default() },
            OutputGeometry { name: "DP-2".into(), x: 2560, width: 1920, height: 1080, ..Default::default() },
        ];
        let paired = pair_displays(&[display(1, Some("DP-2"), (2560, 0)), display(2, Some("DP-3"), (0, 0))], &outputs);
        assert_eq!(paired[0].1.as_ref().unwrap().name, "DP-2");
        assert!(paired[1].1.is_none());
        let unnamed = [OutputGeometry { name: String::new(), ..outputs[0].clone() }];
        assert!(pair_displays(&[display(2, Some("DP-3"), (0, 0))], &unnamed)[0].1.is_some());
    }
    #[test]
    fn mixed_names_keep_unnamed_logical_geometry_without_claiming_stale_outputs() {
        let outputs = [geo("stale", 0, 0, 640, 480), geo("", 0, 0, 1707, 960), geo("DP-2", 1707, 0, 1920, 1080)];
        let displays = [display(1, Some("DP-1"), (0, 0)), display(2, Some("DP-2"), (1707, 0))];
        let paired = pair_displays(&displays, &outputs);
        assert_eq!(paired[0].1.as_ref().map(|o| (o.width, o.height)), Some((1707, 960)));
        assert_eq!(paired[1].1.as_ref().map(|o| o.name.as_str()), Some("DP-2"));
        let elsewhere = pair_displays(&[display(3, Some("DP-3"), (9999, 0))], &outputs);
        assert!(elsewhere[0].1.is_none());
    }

}
