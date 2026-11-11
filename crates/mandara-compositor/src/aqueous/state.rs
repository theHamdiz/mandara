//! Aqueous shell state (`compositor/protocol/aqueous-shell-v1.md`): atomic
//! snapshot and delta batches of output, workspace, window, seat, keyboard
//! and session entities, mapped onto Mandara's model.
//!
//! Every id and sequence is a string scoped to the compositor session.
//! Workspaces are per output, with a one-based display `number` (what the
//! bar shows and clicks), so they map to [`NavigationModel::Workspaces`]
//! keyed by the output's connector name.

use anyhow::{Result, bail};
use serde::Deserialize;

use super::wire::{STATE_SCHEMA, valid_counter};
use crate::{ClientInfo, CompositorEvent, MonitorInfo, NavigationModel, WorkspaceStatus};

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Output {
    pub id: String,
    /// The connector name (`DP-1`).
    pub name: String,
    /// Part of the desktop (also while powered off).
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Global logical bounds.
    #[serde(default)]
    pub bounds: Rect,
    #[serde(default = "one")]
    pub scale: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Workspace {
    pub id: String,
    /// The runtime id of its output.
    pub output: String,
    #[serde(default)]
    pub name: String,
    /// One-based display position on its output.
    pub number: u32,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub urgent: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Window {
    /// The ext-foreign-toplevel identifier.
    pub id: String,
    #[serde(default)]
    pub app_id: Option<String>,
    /// X11 class (Xwayland windows have no app id).
    #[serde(default)]
    pub class: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// Runtime workspace id.
    #[serde(default)]
    pub workspace: Option<String>,
    /// Runtime output id.
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub geometry: Option<Rect>,
    /// Focused by at least one seat.
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub floating: bool,
    #[serde(default)]
    pub minimized: bool,
    #[serde(default)]
    pub fullscreen: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Seat {
    /// The seat's name.
    pub id: String,
    /// The selected output (it stays while layer surfaces have focus).
    #[serde(default)]
    pub output: Option<String>,
    /// The focused window.
    #[serde(default)]
    pub window: Option<String>,
    /// The active keyboard group.
    #[serde(default)]
    pub keyboard: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Keyboard {
    pub id: String,
    #[serde(default)]
    pub layouts: Vec<String>,
    /// Zero-based effective layout.
    #[serde(default)]
    pub index: usize,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Session {
    pub locked: bool,
    /// The unambiguous default seat, if any.
    pub default_seat: Option<String>,
}

/// One upserted entity. Kinds Mandara doesn't follow (`keyboard_device`,
/// `unmanaged_window`, future ones) are accepted and ignored.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entity {
    Output(Output),
    Workspace(Workspace),
    Window(Window),
    Seat(Seat),
    Keyboard(Keyboard),
    Session(Session),
    #[serde(other)]
    Other,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BatchType {
    Snapshot,
    Delta,
}

/// One atomic batch (a `snapshot` result, or a `state` event's).
#[derive(Debug, Deserialize)]
pub struct Batch {
    pub schema: u32,
    pub session: String,
    pub sequence: String,
    pub base_sequence: Option<String>,
    #[serde(rename = "type")]
    pub kind: BatchType,
    #[serde(default)]
    pub upsert: Vec<Entity>,
    #[serde(default)]
    pub removed: Vec<String>,
}

fn yes() -> bool {
    true
}

fn one() -> f64 {
    1.0
}

/// Which entity kinds a batch touched (what has to be derived again).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changed(u8);

impl Changed {
    pub const OUTPUTS: Self = Self(1);
    pub const WORKSPACES: Self = Self(1 << 1);
    pub const WINDOWS: Self = Self(1 << 2);
    pub const SEATS: Self = Self(1 << 3);
    pub const KEYBOARDS: Self = Self(1 << 4);
    pub const SESSION: Self = Self(1 << 5);
    pub const ALL: Self = Self(0b11_1111);

    pub const fn any(self, of: Self) -> bool {
        self.0 & of.0 != 0
    }

    fn add(&mut self, kind: Self) {
        self.0 |= kind.0;
    }
}

trait Keyed {
    fn key(&self) -> &str;
}

macro_rules! keyed {
    ($($t:ty),*) => { $(impl Keyed for $t { fn key(&self) -> &str { &self.id } })* };
}
keyed!(Output, Workspace, Window, Seat, Keyboard);

fn upsert<T: Keyed>(list: &mut Vec<T>, item: T) {
    match list.iter_mut().find(|e| e.key() == item.key()) {
        Some(slot) => *slot = item,
        None => list.push(item),
    }
}

fn remove<T: Keyed>(list: &mut Vec<T>, id: &str) {
    if let Some(i) = list.iter().position(|e| e.key() == id) {
        list.remove(i);
    }
}

/// Removal key kinds the shell schema defines.
const REMOVABLE: [&str; 8] = ["output", "workspace", "window", "unmanaged_window", "seat", "keyboard", "keyboard_device", "session"];

/// The committed desktop as the accepted batches describe it.
#[derive(Debug, Default)]
pub struct Model {
    /// The session token the batches must carry.
    pub session: String,
    /// The last accepted sequence (`None` until the first snapshot).
    pub sequence: Option<String>,
    pub outputs: Vec<Output>,
    pub workspaces: Vec<Workspace>,
    pub windows: Vec<Window>,
    pub seats: Vec<Seat>,
    pub keyboards: Vec<Keyboard>,
    pub state: Session,
}

impl Model {
    pub fn new(session: &str) -> Self {
        Self { session: session.to_string(), ..Self::default() }
    }

    /// Whether a snapshot has been installed.
    pub fn ready(&self) -> bool {
        self.sequence.is_some()
    }

    /// Validates `batch` completely, then installs it (all or nothing). A
    /// delta must name the last accepted sequence as its base: a broken
    /// sequence, another session or an unknown schema is an error, and the
    /// caller starts over with a fresh subscription.
    pub fn apply(&mut self, batch: Batch) -> Result<Changed> {
        if batch.schema != STATE_SCHEMA {
            bail!("aqueous: unsupported state schema {}", batch.schema);
        }
        if batch.session != self.session {
            bail!("aqueous: a batch from another compositor session");
        }
        if !valid_counter(&batch.sequence) {
            bail!("aqueous: malformed sequence {:?}", batch.sequence);
        }
        match batch.kind {
            BatchType::Snapshot if batch.base_sequence.is_some() || !batch.removed.is_empty() => bail!("aqueous: a snapshot with a base or removals"),
            BatchType::Delta if batch.base_sequence.is_none() || batch.base_sequence != self.sequence => {
                bail!("aqueous: delta {} continues {:?}, not the accepted {:?}", batch.sequence, batch.base_sequence, self.sequence)
            }
            _ => {}
        }
        for key in &batch.removed {
            match key.split_once(':') {
                Some((kind, id)) if !id.is_empty() && REMOVABLE.contains(&kind) => {}
                _ => bail!("aqueous: malformed removal {key:?}"),
            }
        }
        let mut changed = Changed::default();
        if batch.kind == BatchType::Snapshot {
            // Cleared in place: the collections keep their capacity.
            self.outputs.clear();
            self.workspaces.clear();
            self.windows.clear();
            self.seats.clear();
            self.keyboards.clear();
            self.state = Session::default();
            changed = Changed::ALL;
        }
        for key in &batch.removed {
            let Some((kind, id)) = key.split_once(':') else { continue };
            let kind = match kind {
                "output" => {
                    remove(&mut self.outputs, id);
                    Changed::OUTPUTS
                }
                "workspace" => {
                    remove(&mut self.workspaces, id);
                    Changed::WORKSPACES
                }
                "window" => {
                    remove(&mut self.windows, id);
                    Changed::WINDOWS
                }
                "seat" => {
                    remove(&mut self.seats, id);
                    Changed::SEATS
                }
                "keyboard" => {
                    remove(&mut self.keyboards, id);
                    Changed::KEYBOARDS
                }
                "session" => {
                    self.state = Session::default();
                    Changed::SESSION
                }
                _ => continue,
            };
            changed.add(kind);
        }
        for entity in batch.upsert {
            let kind = match entity {
                Entity::Output(o) => {
                    upsert(&mut self.outputs, o);
                    Changed::OUTPUTS
                }
                Entity::Workspace(w) => {
                    upsert(&mut self.workspaces, w);
                    Changed::WORKSPACES
                }
                Entity::Window(w) => {
                    upsert(&mut self.windows, w);
                    Changed::WINDOWS
                }
                Entity::Seat(s) => {
                    upsert(&mut self.seats, s);
                    Changed::SEATS
                }
                Entity::Keyboard(k) => {
                    upsert(&mut self.keyboards, k);
                    Changed::KEYBOARDS
                }
                Entity::Session(s) => {
                    self.state = s;
                    Changed::SESSION
                }
                Entity::Other => continue,
            };
            changed.add(kind);
        }
        self.sequence = Some(batch.sequence);
        Ok(changed)
    }

    pub fn output(&self, id: &str) -> Option<&Output> {
        self.outputs.iter().find(|o| o.id == id)
    }

    pub fn output_named(&self, name: &str) -> Option<&Output> {
        self.outputs.iter().find(|o| o.name == name)
    }

    /// The seat commands act for: the session's default seat, else the
    /// only (or first) one.
    pub fn seat(&self) -> Option<&Seat> {
        self.state.default_seat.as_deref().and_then(|id| self.seats.iter().find(|s| s.id == id)).or_else(|| self.seats.first())
    }

    /// The seat to name in a command: only needed when there are several
    /// (an omitted seat requires exactly one).
    pub fn command_seat(&self) -> Option<&str> {
        (self.seats.len() > 1).then(|| self.seat().map(|s| s.id.as_str())).flatten()
    }

    /// The runtime id of the selected output.
    pub fn focused_output(&self) -> Option<&str> {
        self.seat().and_then(|s| s.output.as_deref()).filter(|id| self.output(id).is_some()).or_else(|| self.outputs.iter().find(|o| o.enabled).map(|o| o.id.as_str()))
    }

    /// The focused window: the seat's, else one any seat focuses.
    pub fn focused_window(&self) -> Option<&Window> {
        match self.seat() {
            Some(seat) => seat.window.as_deref().and_then(|id| self.windows.iter().find(|w| w.id == id)),
            None => self.windows.iter().find(|w| w.focused),
        }
    }

    /// The workspace numbered `number` on output `monitor` (a connector
    /// name; the selected output when empty).
    pub fn workspace_on(&self, monitor: &str, number: u32) -> Option<&Workspace> {
        let output = if monitor.is_empty() { self.focused_output()? } else { self.output_named(monitor)?.id.as_str() };
        self.workspaces.iter().find(|w| w.output == output && w.number == number)
    }

    /// The window the shell knows as `client` ([`crate::umbriel::client_id`]
    /// of its identifier), or the focused one.
    pub fn window(&self, client: Option<u64>) -> Option<&Window> {
        match client {
            Some(id) => self.windows.iter().find(|w| crate::umbriel::client_id(&w.id) == id),
            None => self.focused_window(),
        }
    }

    /// Workspaces per enabled output, by number.
    pub fn navigation(&self) -> Vec<(String, NavigationModel)> {
        let mut out: Vec<(String, NavigationModel)> = Vec::with_capacity(self.outputs.len());
        for output in self.outputs.iter().filter(|o| o.enabled) {
            let mut workspaces: Vec<WorkspaceStatus> = self
                .workspaces
                .iter()
                .filter(|w| w.output == output.id)
                .map(|w| WorkspaceStatus {
                    id: w.number,
                    name: if w.name.is_empty() { w.number.to_string() } else { w.name.clone() },
                    active: w.active,
                    occupied: self.windows.iter().any(|win| win.workspace.as_deref() == Some(w.id.as_str())),
                    urgent: w.urgent,
                    layout: String::new(),
                })
                .collect();
            workspaces.sort_by_key(|w| w.id);
            out.push((output.name.clone(), NavigationModel::Workspaces { workspaces }));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn client(&self, w: &Window, focused: Option<&str>) -> ClientInfo {
        let workspace = w.workspace.as_deref().and_then(|id| self.workspaces.iter().find(|ws| ws.id == id));
        let output = w.output.as_deref().or(workspace.map(|ws| ws.output.as_str())).and_then(|id| self.output(id));
        ClientInfo {
            id: crate::umbriel::client_id(&w.id),
            title: w.title.clone().unwrap_or_default(),
            app_id: w.app_id.clone().or_else(|| w.class.clone()).unwrap_or_default(),
            monitor: output.map(|o| o.name.clone()).unwrap_or_default(),
            tags: workspace.map(|ws| vec![ws.number]).unwrap_or_default(),
            focused: focused == Some(w.id.as_str()),
            floating: w.floating,
            fullscreen: w.fullscreen,
            minimized: w.minimized,
            geometry: w.geometry.map(|g| (g.x, g.y, g.width, g.height)),
        }
    }

    pub fn clients(&self) -> Vec<ClientInfo> {
        let focused = self.focused_window().map(|w| w.id.as_str());
        self.windows.iter().map(|w| self.client(w, focused)).collect()
    }

    pub fn focused(&self) -> Option<ClientInfo> {
        self.focused_window().map(|w| self.client(w, Some(&w.id)))
    }

    pub fn monitors(&self) -> Vec<MonitorInfo> {
        let selected = self.focused_output();
        self.outputs
            .iter()
            .filter(|o| o.enabled)
            .map(|o| MonitorInfo { name: o.name.clone(), x: o.bounds.x, y: o.bounds.y, width: o.bounds.width, height: o.bounds.height, scale: o.scale as f32, active: selected == Some(o.id.as_str()) })
            .collect()
    }

    /// The seat's effective keyboard layout name.
    pub fn keyboard_layout(&self) -> Option<&str> {
        let group = self.seat()?.keyboard.as_deref()?;
        let keyboard = self.keyboards.iter().find(|k| k.id == group)?;
        keyboard.layouts.get(keyboard.index).map(String::as_str)
    }
}

/// What the shell was last told, so only changes are sent.
#[derive(Default)]
pub struct Diff {
    navigation: Vec<(String, NavigationModel)>,
    clients: Option<Vec<ClientInfo>>,
    focused: Option<Option<ClientInfo>>,
    monitors: Option<Vec<MonitorInfo>>,
    layout: Option<String>,
}

impl Diff {
    /// Appends the events `changed` kinds caused to `out`. Only what
    /// depends on a touched kind is derived again.
    pub fn events(&mut self, model: &Model, changed: Changed, out: &mut Vec<CompositorEvent>) {
        const PLACEMENT: Changed = Changed(Changed::OUTPUTS.0 | Changed::WORKSPACES.0 | Changed::WINDOWS.0);
        if changed.any(PLACEMENT) {
            for (monitor, nav) in model.navigation() {
                match self.navigation.iter_mut().find(|(m, _)| *m == monitor) {
                    Some((_, last)) if *last == nav => {}
                    Some((_, last)) => {
                        *last = nav.clone();
                        out.push(CompositorEvent::Navigation { monitor, model: nav });
                    }
                    None => {
                        self.navigation.push((monitor.clone(), nav.clone()));
                        out.push(CompositorEvent::Navigation { monitor, model: nav });
                    }
                }
            }
            let clients = model.clients();
            if self.clients.as_ref() != Some(&clients) {
                self.clients = Some(clients.clone());
                out.push(CompositorEvent::Clients(clients));
            }
        }
        if changed.any(Changed(PLACEMENT.0 | Changed::SEATS.0 | Changed::SESSION.0)) {
            let focused = model.focused();
            if self.focused.as_ref() != Some(&focused) {
                self.focused = Some(focused.clone());
                out.push(CompositorEvent::FocusedClient(focused));
            }
        }
        if changed.any(Changed(Changed::OUTPUTS.0 | Changed::SEATS.0 | Changed::SESSION.0)) {
            let monitors = model.monitors();
            if self.monitors.as_ref() != Some(&monitors) {
                self.monitors = Some(monitors.clone());
                out.push(CompositorEvent::Monitors(monitors));
            }
        }
        if changed.any(Changed(Changed::KEYBOARDS.0 | Changed::SEATS.0 | Changed::SESSION.0))
            && let Some(name) = model.keyboard_layout()
            && self.layout.as_deref() != Some(name)
        {
            self.layout = Some(name.to_string());
            out.push(CompositorEvent::KeyboardLayout(name.to_string()));
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const SESSION: &str = "0123456789abcdef0123456789abcdef";

    /// A two-output desktop in Aqueous's snapshot shape (entities in no
    /// particular order, runtime ids unrelated to numbers or names).
    pub fn snapshot_batch(sequence: &str) -> String {
        format!(
            r#"{{"schema":1,"session":"{SESSION}","sequence":"{sequence}","base_sequence":null,"type":"snapshot","upsert":[
            {{"kind":"workspace","id":"12","output":"2","name":"3","number":3,"active":false,"urgent":false}},
            {{"kind":"workspace","id":"2","output":"1","name":"web","number":2,"active":false,"urgent":true}},
            {{"kind":"session","id":"session","locked":false,"default_seat":"seat0","overview_output":null,"overview_window":null}},
            {{"kind":"workspace","id":"11","output":"2","name":"2","number":2,"active":false,"urgent":false}},
            {{"kind":"output","id":"1","name":"DP-1","enabled":true,"powered":true,"bounds":{{"x":0,"y":0,"width":2560,"height":1440}},"usable_bounds":{{"x":0,"y":0,"width":2560,"height":1440}},"scale":1.5,"transform":"normal","active_workspace":"1"}},
            {{"kind":"workspace","id":"10","output":"2","name":"1","number":1,"active":true,"urgent":false}},
            {{"kind":"seat","id":"seat0","output":"1","window":"w-a","focus_kind":"window","keyboard":"5"}},
            {{"kind":"workspace","id":"1","output":"1","name":"1","number":1,"active":true,"urgent":false}},
            {{"kind":"keyboard","id":"5","seat":"seat0","layouts":["English (US)","Arabic"],"index":0}},
            {{"kind":"keyboard_device","id":"6","name":"AT keyboard","seat":"seat0","group":"5","virtual":false}},
            {{"kind":"window","id":"w-a","backend":"xdg","app_id":"foot","class":null,"title":"shell","workspace":"1","output":"1","geometry":{{"x":10,"y":20,"width":800,"height":600}},"outer_geometry":{{"x":8,"y":18,"width":804,"height":604}},"layout":"tiled","focused":true,"visible":true,"floating":false,"minimized":false,"maximized":false,"fullscreen":false,"skip_taskbar":false,"skip_switcher":false,"always_above":false,"always_below":false,"snapped":false,"fixed_position":false,"can_minimize":false,"can_maximize":false,"can_activate":true,"icon":null}},
            {{"kind":"window","id":"w-b","backend":"xwayland","app_id":null,"class":"Steam","title":null,"workspace":"11","output":"2","geometry":{{"x":2560,"y":0,"width":1280,"height":720}},"outer_geometry":{{"x":2560,"y":0,"width":1280,"height":720}},"layout":"floating","focused":false,"visible":false,"floating":true,"minimized":false,"maximized":false,"fullscreen":false,"skip_taskbar":false,"skip_switcher":false,"always_above":false,"always_below":false,"snapped":false,"fixed_position":false,"can_minimize":true,"can_maximize":true,"can_activate":true}},
            {{"kind":"output","id":"2","name":"HDMI-A-1","enabled":true,"powered":true,"bounds":{{"x":2560,"y":0,"width":1920,"height":1080}},"usable_bounds":{{"x":2560,"y":0,"width":1920,"height":1080}},"scale":1,"transform":"normal","active_workspace":"10"}}
        ],"removed":[]}}"#
        )
    }

    pub fn batch(text: &str) -> Batch {
        serde_json::from_str(text).unwrap()
    }

    fn installed() -> Model {
        let mut m = Model::new(SESSION);
        assert_eq!(m.apply(batch(&snapshot_batch("1"))).unwrap(), Changed::ALL);
        m
    }

    fn workspaces(nav: &[(String, NavigationModel)], monitor: &str) -> Vec<(u32, String, bool, bool, bool)> {
        let (_, NavigationModel::Workspaces { workspaces }) = nav.iter().find(|(m, _)| m == monitor).unwrap() else { panic!() };
        workspaces.iter().map(|w| (w.id, w.name.clone(), w.active, w.occupied, w.urgent)).collect()
    }

    #[test]
    fn a_snapshot_becomes_per_output_workspaces_clients_and_monitors() {
        let m = installed();
        let nav = m.navigation();
        assert_eq!(nav.iter().map(|(o, _)| o.as_str()).collect::<Vec<_>>(), ["DP-1", "HDMI-A-1"]);
        assert_eq!(workspaces(&nav, "DP-1"), [(1, "1".into(), true, true, false), (2, "web".into(), false, false, true)]);
        assert_eq!(workspaces(&nav, "HDMI-A-1"), [(1, "1".into(), true, false, false), (2, "2".into(), false, true, false), (3, "3".into(), false, false, false)]);
        let clients = m.clients();
        assert_eq!(clients.len(), 2);
        assert_eq!((clients[0].app_id.as_str(), clients[0].monitor.as_str(), clients[0].tags.clone(), clients[0].focused, clients[0].geometry), ("foot", "DP-1", vec![1], true, Some((10, 20, 800, 600))));
        assert_eq!((clients[1].app_id.as_str(), clients[1].title.as_str(), clients[1].monitor.as_str(), clients[1].tags.clone(), clients[1].floating), ("Steam", "", "HDMI-A-1", vec![2], true), "Xwayland: the class stands in for the app id");
        assert_eq!(m.focused().map(|c| c.title), Some("shell".into()));
        let monitors = m.monitors();
        assert_eq!(monitors.iter().map(|o| (o.name.as_str(), o.width, o.scale, o.active)).collect::<Vec<_>>(), [("DP-1", 2560, 1.5, true), ("HDMI-A-1", 1920, 1.0, false)]);
        assert_eq!(m.keyboard_layout(), Some("English (US)"));
        assert_eq!(m.workspace_on("HDMI-A-1", 2).map(|w| w.id.as_str()), Some("11"));
        assert_eq!(m.workspace_on("", 2).map(|w| w.id.as_str()), Some("2"), "the selected output");
        assert_eq!(m.workspace_on("HDMI-A-1", 9), None);
        assert_eq!(m.window(Some(crate::umbriel::client_id("w-b"))).map(|w| w.id.as_str()), Some("w-b"));
        assert_eq!(m.command_seat(), None, "one seat: commands leave it out");
    }

    #[test]
    fn deltas_replace_and_remove_entities_atomically() {
        let mut m = installed();
        let delta = format!(
            r#"{{"schema":1,"session":"{SESSION}","sequence":"4","base_sequence":"1","type":"delta","upsert":[
                {{"kind":"workspace","id":"2","output":"1","name":"web","number":2,"active":true,"urgent":false}},
                {{"kind":"workspace","id":"1","output":"1","name":"1","number":1,"active":false,"urgent":false}},
                {{"kind":"seat","id":"seat0","output":"1","window":null,"focus_kind":"none","keyboard":"5"}}
            ],"removed":["window:w-a","keyboard_device:6"]}}"#
        );
        let changed = m.apply(batch(&delta)).unwrap();
        assert!(changed.any(Changed::WORKSPACES) && changed.any(Changed::WINDOWS) && changed.any(Changed::SEATS) && !changed.any(Changed::OUTPUTS));
        assert_eq!(m.sequence.as_deref(), Some("4"));
        assert_eq!(workspaces(&m.navigation(), "DP-1"), [(1, "1".into(), false, false, false), (2, "web".into(), true, false, false)]);
        assert_eq!(m.windows.len(), 1);
        assert_eq!(m.focused(), None);
        // A delta against an old base is refused, and nothing changes.
        let stale = format!(r#"{{"schema":1,"session":"{SESSION}","sequence":"5","base_sequence":"1","type":"delta","upsert":[{{"kind":"output","id":"1","name":"X","enabled":false}}],"removed":[]}}"#);
        assert!(m.apply(batch(&stale)).is_err());
        assert_eq!(m.sequence.as_deref(), Some("4"));
        assert_eq!(m.outputs[0].name, "DP-1");
        // A malformed removal key rejects the whole batch first.
        let bad = format!(r#"{{"schema":1,"session":"{SESSION}","sequence":"5","base_sequence":"4","type":"delta","upsert":[{{"kind":"output","id":"1","name":"X","enabled":false}}],"removed":["window"]}}"#);
        assert!(m.apply(batch(&bad)).is_err());
        assert_eq!(m.outputs[0].name, "DP-1", "all or nothing");
        // Another session or schema is refused.
        let other = snapshot_batch("9").replace(SESSION, "ffffffffffffffffffffffffffffffff");
        assert!(m.apply(batch(&other)).is_err());
        assert!(m.apply(batch(&snapshot_batch("9").replace("\"schema\":1", "\"schema\":2"))).is_err());
        // A fresh snapshot replaces everything (the window comes back).
        assert_eq!(m.apply(batch(&snapshot_batch("9"))).unwrap(), Changed::ALL);
        assert_eq!(m.windows.len(), 2);
    }

    #[test]
    fn a_delta_needs_an_installed_snapshot() {
        let mut m = Model::new(SESSION);
        let delta = format!(r#"{{"schema":1,"session":"{SESSION}","sequence":"2","base_sequence":"1","type":"delta","upsert":[],"removed":[]}}"#);
        assert!(m.apply(batch(&delta)).is_err());
        assert!(!m.ready());
        let with_base = snapshot_batch("1").replace("\"base_sequence\":null", "\"base_sequence\":\"0\"");
        assert!(m.apply(batch(&with_base)).is_err(), "a snapshot has no base");
    }

    #[test]
    fn only_changes_reach_the_shell() {
        let mut m = installed();
        let mut diff = Diff::default();
        let mut out = Vec::new();
        diff.events(&m, Changed::ALL, &mut out);
        assert_eq!(out.iter().filter(|e| matches!(e, CompositorEvent::Navigation { .. })).count(), 2);
        assert!(out.iter().any(|e| matches!(e, CompositorEvent::Clients(c) if c.len() == 2)));
        assert!(out.iter().any(|e| matches!(e, CompositorEvent::FocusedClient(Some(c)) if c.app_id == "foot")));
        assert!(out.iter().any(|e| matches!(e, CompositorEvent::Monitors(m) if m.len() == 2)));
        assert!(out.contains(&CompositorEvent::KeyboardLayout("English (US)".into())));
        // A rename on one output: only that output's workspaces.
        out.clear();
        let rename = format!(r#"{{"schema":1,"session":"{SESSION}","sequence":"3","base_sequence":"1","type":"delta","upsert":[{{"kind":"workspace","id":"12","output":"2","name":"final 🐟 \"quoted\"","number":3,"active":false,"urgent":false}}],"removed":[]}}"#);
        let changed = m.apply(batch(&rename)).unwrap();
        diff.events(&m, changed, &mut out);
        assert!(matches!(&out[..], [CompositorEvent::Navigation { monitor, model: NavigationModel::Workspaces { workspaces } }] if monitor == "HDMI-A-1" && workspaces[2].name == "final 🐟 \"quoted\""), "{out:?}");
        // A layout switch: only the layout.
        out.clear();
        let layout = format!(r#"{{"schema":1,"session":"{SESSION}","sequence":"5","base_sequence":"3","type":"delta","upsert":[{{"kind":"keyboard","id":"5","seat":"seat0","layouts":["English (US)","Arabic"],"index":1}}],"removed":[]}}"#);
        let changed = m.apply(batch(&layout)).unwrap();
        diff.events(&m, changed, &mut out);
        assert_eq!(out, [CompositorEvent::KeyboardLayout("Arabic".into())]);
    }
}
