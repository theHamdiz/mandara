//! Sway driver: the i3 / Sway IPC over `$SWAYSOCK` (`sway-ipc(7)`).
//!
//! Every message is framed as the magic `i3-ipc`, the payload's length and
//! the message type (both 32-bit, native byte order: little-endian on the
//! machines Mandara runs on), then a JSON payload. Requests used here are
//! `RUN_COMMAND`, `GET_WORKSPACES`, `SUBSCRIBE`, `GET_OUTPUTS`, `GET_TREE`,
//! `GET_VERSION`, `GET_BINDING_STATE` and `GET_INPUTS`; events (the type's
//! high bit set) arrive on a subscribed connection.
//!
//! Sway's workspaces are dynamic and live on one output each, so they map
//! to [`NavigationModel::Workspaces`] per output. A numbered workspace's id
//! is its number (what `workspace number N` takes); named ones without a
//! number get stable ids from their name, above every number (see
//! [`workspace_ids`]). Windows
//! come from the layout tree: a Wayland window's `app_id`, an X11 window's
//! class, its title, pid, floating, fullscreen and urgency. The binding
//! mode drives the bar's keymode badge and each workspace's container
//! layout (`splith`, `splitv`, `tabbed`, `stacked`) its layout pill.
//!
//! The parsers are tested against output captured from a real (headless)
//! sway 1.12 in `tests/fixtures/sway/`.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::Value;

use crate::backend::CompositorBackend;
use crate::model::{ClientInfo, CompositorEvent, CompositorKind, MonitorInfo, NavigationModel, UNNUMBERED_ID_BASE, WorkspaceStatus};

/// The bytes every message starts with.
pub const MAGIC: &[u8; 6] = b"i3-ipc";
/// Magic, payload length and message type.
const HEADER_LEN: usize = MAGIC.len() + 8;
/// Largest payload accepted (a layout tree is a few hundred KiB at most).
const MAX_PAYLOAD: usize = 64 << 20;

/// Request types (`sway-ipc(7)`).
pub mod msg {
    pub const RUN_COMMAND: u32 = 0;
    pub const GET_WORKSPACES: u32 = 1;
    pub const SUBSCRIBE: u32 = 2;
    pub const GET_OUTPUTS: u32 = 3;
    pub const GET_TREE: u32 = 4;
    pub const GET_VERSION: u32 = 7;
    pub const GET_BINDING_STATE: u32 = 12;
    pub const GET_INPUTS: u32 = 100;
}

/// Event types: the request range with the high bit set.
pub mod event {
    pub const WORKSPACE: u32 = 0x8000_0000;
    pub const OUTPUT: u32 = 0x8000_0001;
    pub const MODE: u32 = 0x8000_0002;
    pub const WINDOW: u32 = 0x8000_0003;
    pub const SHUTDOWN: u32 = 0x8000_0006;
    pub const INPUT: u32 = 0x8000_0015;
}

/// The events the shell follows (`SUBSCRIBE` payload names).
pub const EVENTS: [&str; 6] = ["workspace", "window", "output", "mode", "shutdown", "input"];

/// One framed message.
pub fn encode(kind: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Reads one framed message: `(type, payload)`.
pub fn read_message(r: &mut impl Read) -> Result<(u32, Vec<u8>)> {
    let mut header = [0u8; HEADER_LEN];
    r.read_exact(&mut header)?;
    if &header[..MAGIC.len()] != MAGIC {
        bail!("sway: not an i3-ipc message");
    }
    let word = |at: usize| u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]]);
    let (len, kind) = (word(MAGIC.len()) as usize, word(MAGIC.len() + 4));
    if len > MAX_PAYLOAD {
        bail!("sway: a {len}-byte message is too large");
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok((kind, payload))
}

/// A rectangle in layout (logical) coordinates.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// One entry of `GET_WORKSPACES`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SwayWorkspace {
    /// The workspace's number, `-1` for a name without one.
    pub num: i32,
    pub name: String,
    pub visible: bool,
    pub focused: bool,
    pub urgent: bool,
    pub output: String,
}

/// One entry of `GET_OUTPUTS`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SwayOutput {
    pub name: String,
    /// Part of the desktop (a disabled output is listed but inactive).
    pub active: bool,
    pub focused: bool,
    pub rect: Rect,
    pub scale: f64,
}

/// An X11 window's properties.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct WindowProperties {
    pub class: Option<String>,
    pub instance: Option<String>,
    pub title: Option<String>,
}

/// A node of `GET_TREE` (and the container in window events).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SwayNode {
    pub id: i64,
    /// `root`, `output`, `workspace`, `con` or `floating_con`.
    #[serde(rename = "type")]
    pub kind: String,
    pub name: Option<String>,
    pub app_id: Option<String>,
    pub pid: Option<i64>,
    pub focused: bool,
    pub urgent: bool,
    /// 0 none, 1 on its workspace, 2 global.
    pub fullscreen_mode: u8,
    pub layout: String,
    pub num: Option<i32>,
    pub rect: Rect,
    pub window_properties: Option<WindowProperties>,
    pub nodes: Vec<SwayNode>,
    pub floating_nodes: Vec<SwayNode>,
}

impl SwayNode {
    /// A window (a view), not a split container.
    pub fn is_window(&self) -> bool {
        matches!(self.kind.as_str(), "con" | "floating_con") && self.nodes.is_empty() && (self.app_id.is_some() || self.window_properties.is_some() || self.pid.is_some())
    }

    /// Wayland `app_id`, else the X11 class.
    pub fn app(&self) -> String {
        self.app_id.clone().filter(|a| !a.is_empty()).or_else(|| self.window_properties.as_ref().and_then(|p| p.class.clone())).unwrap_or_default()
    }
}

/// The scratchpad's hidden output.
const SCRATCH_OUTPUT: &str = "__i3";

/// The id a workspace without a number asks for: a hash of its name in
/// the range from [`UNNUMBERED_ID_BASE`] (FNV-1a, so it's the same in
/// every snapshot and every run).
fn unnumbered_id(name: &str) -> u32 {
    const FNV_OFFSET: u32 = 0x811c_9dc5;
    const FNV_PRIME: u32 = 0x0100_0193;
    let hash = name.bytes().fold(FNV_OFFSET, |h, b| (h ^ u32::from(b)).wrapping_mul(FNV_PRIME));
    UNNUMBERED_ID_BASE | (hash & !UNNUMBERED_ID_BASE)
}

/// The bar's id for each workspace, in order: its number when it has one,
/// otherwise an id from its name above every possible number
/// ([`UNNUMBERED_ID_BASE`]). A named workspace keeps its id while other
/// workspaces come and go (a bar click captured a moment ago still finds
/// it), and never takes the id of a numbered workspace that doesn't exist
/// yet (`workspace 3` stays reachable while `web` exists). Two names whose
/// hashes meet take the next free id, in sway's order.
pub fn workspace_ids(workspaces: &[SwayWorkspace]) -> Vec<u32> {
    let mut taken = std::collections::BTreeSet::new();
    workspaces
        .iter()
        .map(|w| match u32::try_from(w.num) {
            Ok(n) => n,
            Err(_) => {
                let mut id = unnumbered_id(&w.name);
                while !taken.insert(id) {
                    id = if id == u32::MAX { UNNUMBERED_ID_BASE } else { id + 1 };
                }
                id
            }
        })
        .collect()
}

/// What the tree says about each workspace (by name): whether it holds a
/// window, and its container layout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeWorkspace {
    pub occupied: bool,
    pub layout: String,
}

/// Windows and workspaces of a layout tree.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub workspaces: BTreeMap<String, TreeWorkspace>,
    /// `(window, output, workspace name, floating)`.
    pub windows: Vec<(SwayNode, String, String, bool)>,
}

impl Tree {
    pub fn of(root: &SwayNode) -> Tree {
        let mut tree = Tree::default();
        for output in root.nodes.iter().filter(|o| o.kind == "output" && o.name.as_deref() != Some(SCRATCH_OUTPUT)) {
            let out_name = output.name.clone().unwrap_or_default();
            for ws in output.nodes.iter().filter(|w| w.kind == "workspace") {
                let ws_name = ws.name.clone().unwrap_or_default();
                let before = tree.windows.len();
                for n in &ws.nodes {
                    collect(n, &out_name, &ws_name, false, &mut tree.windows);
                }
                for n in &ws.floating_nodes {
                    collect(n, &out_name, &ws_name, true, &mut tree.windows);
                }
                tree.workspaces.insert(ws_name, TreeWorkspace { occupied: tree.windows.len() > before, layout: ws.layout.clone() });
            }
        }
        tree
    }
}

fn collect(node: &SwayNode, output: &str, workspace: &str, floating: bool, out: &mut Vec<(SwayNode, String, String, bool)>) {
    let floating = floating || node.kind == "floating_con";
    if node.is_window() {
        let mut leaf = node.clone();
        leaf.nodes.clear();
        leaf.floating_nodes.clear();
        out.push((leaf, output.to_string(), workspace.to_string(), floating));
    }
    for n in node.nodes.iter().chain(&node.floating_nodes) {
        collect(n, output, workspace, floating, out);
    }
}

/// Workspaces grouped per output (sway's order within each), with
/// occupancy and layout from the tree.
pub fn navigation(workspaces: &[SwayWorkspace], tree: &Tree) -> Vec<(String, NavigationModel)> {
    let mut by_output: BTreeMap<String, Vec<WorkspaceStatus>> = BTreeMap::new();
    for (w, id) in workspaces.iter().zip(workspace_ids(workspaces)) {
        let t = tree.workspaces.get(&w.name).cloned().unwrap_or_default();
        by_output.entry(w.output.clone()).or_default().push(WorkspaceStatus { id, name: w.name.clone(), active: w.visible, occupied: t.occupied, urgent: w.urgent, layout: t.layout });
    }
    by_output.into_iter().map(|(output, workspaces)| (output, NavigationModel::Workspaces { workspaces })).collect()
}

/// Every window as a client (its workspace's id as its tag).
pub fn clients(workspaces: &[SwayWorkspace], tree: &Tree) -> Vec<ClientInfo> {
    let ids: BTreeMap<&str, u32> = workspaces.iter().map(|w| w.name.as_str()).zip(workspace_ids(workspaces)).collect();
    tree.windows
        .iter()
        .map(|(n, output, ws, floating)| ClientInfo {
            id: u64::try_from(n.id).unwrap_or_default(),
            title: n.name.clone().unwrap_or_default(),
            app_id: n.app(),
            monitor: output.clone(),
            tags: ids.get(ws.as_str()).map(|id| vec![*id]).unwrap_or_default(),
            focused: n.focused,
            floating: *floating,
            fullscreen: n.fullscreen_mode > 0,
            geometry: (n.rect.width > 0 && n.rect.height > 0).then_some((n.rect.x, n.rect.y, n.rect.width, n.rect.height)),
        })
        .collect()
}

/// Active outputs as monitors.
pub fn monitors(outputs: &[SwayOutput]) -> Vec<MonitorInfo> {
    outputs
        .iter()
        .filter(|o| o.active)
        .map(|o| MonitorInfo { name: o.name.clone(), x: o.rect.x, y: o.rect.y, width: o.rect.width, height: o.rect.height, scale: o.scale as f32, active: o.focused })
        .collect()
}

/// A `RUN_COMMAND` reply: one result per command; the first failure's
/// message as an error.
pub fn command_result(reply: &Value) -> Result<()> {
    let list = reply.as_array().ok_or_else(|| anyhow!("sway: malformed command reply {reply}"))?;
    match list.iter().find(|r| r.get("success").and_then(Value::as_bool) != Some(true)) {
        Some(failed) => bail!("sway: {}", failed.get("error").and_then(Value::as_str).unwrap_or("the command failed")),
        None => Ok(()),
    }
}

/// A workspace (or output) name as one sway command argument, so the
/// command reaches that very workspace. Sway keeps a backslash in such an
/// argument (`workspace "a \"b\""` makes a workspace named `a \"b\"`;
/// checked in sway 1.12), so nothing is escaped: the name goes inside the
/// quote it doesn't contain (`'a "b"'`). Sway itself can't write a name
/// with both kinds of quote (a command or config line can't make one),
/// so that case only keeps the old fallback of dropping `"`.
fn quoted(name: &str) -> String {
    if !name.contains('"') {
        format!("\"{name}\"")
    } else if !name.contains('\'') {
        format!("'{name}'")
    } else {
        format!("\"{}\"", name.replace('"', ""))
    }
}

/// The command that shows the workspace the bar calls `id`: by number,
/// by name for one without a number, or (when it doesn't exist yet) a new
/// numbered one on `output`.
pub fn select_command(workspaces: &[SwayWorkspace], id: u32, output: &str) -> String {
    match workspaces.iter().zip(workspace_ids(workspaces)).find(|(_, i)| *i == id) {
        Some((w, _)) if w.num >= 0 => format!("workspace number {}", w.num),
        Some((w, _)) => format!("workspace {}", quoted(&w.name)),
        None if !output.is_empty() => format!("focus output {}; workspace number {id}", quoted(output)),
        None => format!("workspace number {id}"),
    }
}

/// The command that moves a window (the focused one for `None`) to the
/// workspace the bar calls `id`, without following it.
pub fn move_command(workspaces: &[SwayWorkspace], client: Option<u64>, id: u32) -> String {
    let target = match workspaces.iter().zip(workspace_ids(workspaces)).find(|(_, i)| *i == id) {
        Some((w, _)) if w.num < 0 => quoted(&w.name),
        _ => format!("number {id}"),
    };
    let criteria = client.map(|c| format!("[con_id={c}] ")).unwrap_or_default();
    format!("{criteria}move container to workspace {target}")
}

/// The active layout's name in an `input` event or a `GET_INPUTS` entry.
pub fn keyboard_layout_name(input: &Value) -> Option<String> {
    (input.get("type")?.as_str()? == "keyboard").then(|| input.get("xkb_active_layout_name")?.as_str().map(str::to_string)).flatten()
}

/// What the event stream needs to compare against: the last state sent.
#[derive(Default)]
pub struct SwayState {
    nav: BTreeMap<String, NavigationModel>,
    clients: Option<Vec<ClientInfo>>,
    focused: Option<Option<ClientInfo>>,
}

impl SwayState {
    /// Events for a fresh workspace list and tree, only for what changed.
    pub fn update(&mut self, workspaces: &[SwayWorkspace], tree: &Tree) -> Vec<CompositorEvent> {
        let mut out = Vec::new();
        let nav: BTreeMap<String, NavigationModel> = navigation(workspaces, tree).into_iter().collect();
        for (monitor, model) in &nav {
            if self.nav.get(monitor) != Some(model) {
                out.push(CompositorEvent::Navigation { monitor: monitor.clone(), model: model.clone() });
            }
        }
        self.nav = nav;
        let all = clients(workspaces, tree);
        let focused = all.iter().find(|c| c.focused).cloned();
        if self.clients.as_ref() != Some(&all) {
            self.clients = Some(all.clone());
            out.push(CompositorEvent::Clients(all));
        }
        if self.focused.as_ref() != Some(&focused) {
            self.focused = Some(focused.clone());
            out.push(CompositorEvent::FocusedClient(focused));
        }
        out
    }
}

pub struct SwayDriver {
    socket: PathBuf,
}

impl SwayDriver {
    /// `$SWAYSOCK` (sway exports it, and `I3SOCK`, to everything it starts).
    pub fn from_env() -> Option<Self> {
        let socket = PathBuf::from(std::env::var_os("SWAYSOCK")?);
        socket.exists().then_some(Self { socket })
    }

    pub fn with_socket(socket: impl Into<PathBuf>) -> Self {
        Self { socket: socket.into() }
    }

    /// One request on its own connection.
    fn request(&self, kind: u32, payload: &str) -> Result<Value> {
        let mut stream = UnixStream::connect(&self.socket).with_context(|| format!("connecting {}", self.socket.display()))?;
        stream.set_read_timeout(Some(crate::REQUEST_TIMEOUT))?;
        stream.write_all(&encode(kind, payload.as_bytes()))?;
        let (reply_kind, body) = read_message(&mut stream)?;
        if reply_kind != kind {
            bail!("sway: reply of type {reply_kind} to request {kind}");
        }
        Ok(serde_json::from_slice(&body)?)
    }

    fn query<T: serde::de::DeserializeOwned>(&self, kind: u32) -> Result<T> {
        Ok(serde_json::from_value(self.request(kind, "")?)?)
    }

    pub fn workspaces(&self) -> Result<Vec<SwayWorkspace>> {
        self.query(msg::GET_WORKSPACES)
    }

    pub fn tree(&self) -> Result<Tree> {
        Ok(Tree::of(&self.query::<SwayNode>(msg::GET_TREE)?))
    }

    /// Runs sway commands (`;`-separated).
    pub fn run(&self, command: &str) -> Result<()> {
        command_result(&self.request(msg::RUN_COMMAND, command)?)
    }

    fn keymode(&self) -> Result<String> {
        Ok(self.request(msg::GET_BINDING_STATE, "")?.get("name").and_then(Value::as_str).unwrap_or_default().to_string())
    }

    fn keyboard_layout(&self) -> Option<String> {
        self.request(msg::GET_INPUTS, "").ok()?.as_array()?.iter().find_map(keyboard_layout_name)
    }

    /// The state the bar needs on (re)subscribing.
    fn snapshot(&self, state: &mut SwayState) -> Vec<CompositorEvent> {
        let mut out = match (self.workspaces(), self.tree()) {
            (Ok(ws), Ok(tree)) => state.update(&ws, &tree),
            _ => Vec::new(),
        };
        out.extend(self.get_monitors().ok().map(CompositorEvent::Monitors));
        out.extend(self.keymode().ok().map(CompositorEvent::Keymode));
        out.extend(self.keyboard_layout().map(CompositorEvent::KeyboardLayout));
        out
    }

    /// Events for one message of the subscription; `Ok(None)` when sway is
    /// shutting down.
    fn on_event(&self, kind: u32, payload: &[u8], state: &mut SwayState) -> Result<Option<Vec<CompositorEvent>>> {
        Ok(Some(match kind {
            event::WORKSPACE | event::WINDOW => state.update(&self.workspaces()?, &self.tree()?),
            event::OUTPUT => {
                let mut out = vec![CompositorEvent::Monitors(self.get_monitors()?)];
                out.extend(state.update(&self.workspaces()?, &self.tree()?));
                out
            }
            event::MODE => {
                let body: Value = serde_json::from_slice(payload)?;
                vec![CompositorEvent::Keymode(body.get("change").and_then(Value::as_str).unwrap_or_default().to_string())]
            }
            event::INPUT => {
                let body: Value = serde_json::from_slice(payload)?;
                let layout_change = matches!(body.get("change").and_then(Value::as_str), Some("xkb_layout" | "xkb_keymap"));
                body.get("input").filter(|_| layout_change).and_then(keyboard_layout_name).map(CompositorEvent::KeyboardLayout).into_iter().collect()
            }
            event::SHUTDOWN => return Ok(None),
            _ => Vec::new(),
        }))
    }
}

impl CompositorBackend for SwayDriver {
    fn kind(&self) -> CompositorKind {
        CompositorKind::Sway
    }

    fn version(&self) -> Option<(u32, u32)> {
        let v = self.request(msg::GET_VERSION, "").ok()?;
        Some((u32::try_from(v.get("major")?.as_u64()?).ok()?, u32::try_from(v.get("minor")?.as_u64()?).ok()?))
    }

    fn subscribe_events(&self) -> async_channel::Receiver<CompositorEvent> {
        let (tx, rx) = async_channel::bounded(crate::EVENT_QUEUE);
        let socket = self.socket.clone();
        let _ = std::thread::Builder::new().name("sway-events".into()).spawn(move || {
            let driver = SwayDriver { socket };
            loop {
                let result = (|| -> Result<bool> {
                    let mut stream = UnixStream::connect(&driver.socket)?;
                    stream.write_all(&encode(msg::SUBSCRIBE, serde_json::to_string(&EVENTS)?.as_bytes()))?;
                    let (kind, body) = read_message(&mut stream)?;
                    let reply: Value = serde_json::from_slice(&body)?;
                    if kind != msg::SUBSCRIBE || reply.get("success").and_then(Value::as_bool) != Some(true) {
                        bail!("sway: subscription refused: {reply}");
                    }
                    let mut state = SwayState::default();
                    for e in driver.snapshot(&mut state) {
                        if tx.send_blocking(e).is_err() {
                            return Ok(true);
                        }
                    }
                    loop {
                        let (kind, payload) = read_message(&mut stream)?;
                        let Some(events) = driver.on_event(kind, &payload, &mut state)? else { bail!("sway is shutting down") };
                        for e in events {
                            if tx.send_blocking(e).is_err() {
                                return Ok(true);
                            }
                        }
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
        Ok(navigation(&self.workspaces()?, &self.tree()?))
    }

    fn get_active_client(&self) -> Result<Option<ClientInfo>> {
        Ok(clients(&self.workspaces()?, &self.tree()?).into_iter().find(|c| c.focused))
    }

    fn get_all_clients(&self) -> Result<Vec<ClientInfo>> {
        Ok(clients(&self.workspaces()?, &self.tree()?))
    }

    fn get_monitors(&self) -> Result<Vec<MonitorInfo>> {
        Ok(monitors(&self.query::<Vec<SwayOutput>>(msg::GET_OUTPUTS)?))
    }

    fn select_primary(&self, monitor: &str, id: u32) -> Result<()> {
        self.run(&select_command(&self.workspaces()?, id, monitor))
    }

    fn toggle_secondary(&self, monitor: &str, id: u32) -> Result<()> {
        // No composite views: a workspace switch, as on Niri and Hyprland.
        self.select_primary(monitor, id)
    }

    fn move_client_to(&self, client_id: Option<u64>, id: u32) -> Result<()> {
        self.run(&move_command(&self.workspaces()?, client_id, id))
    }

    fn toggle_client_tag(&self, client_id: Option<u64>, tag: u32) -> Result<()> {
        // A window is on one workspace: a move.
        self.move_client_to(client_id, tag)
    }

    fn cycle_layout(&self, _monitor: &str) -> Result<()> {
        // The focused container's layout: stacking → tabbed → splith → splitv.
        self.run("layout toggle all")
    }

    fn dispatch_raw(&self, cmd: &str, args: &[&str]) -> Result<()> {
        let mut command = cmd.to_string();
        for a in args {
            command.push(' ');
            command.push_str(a);
        }
        self.run(&command)
    }

    fn reload_config(&self) -> Result<()> {
        self.run("reload")
    }

    fn switch_keyboard_layout(&self) -> Result<()> {
        self.run("input type:keyboard xkb_switch_layout next")
    }

    fn apply_cursor(&self, theme: Option<&str>, size: Option<u32>) -> Result<()> {
        // `seat * xcursor_theme <theme> [<size>]` applies at once (and sets
        // XCURSOR_THEME / XCURSOR_SIZE for apps sway starts later).
        let Some(theme) = theme.map(str::trim).filter(|t| !t.is_empty() && !t.contains(char::is_whitespace)).or(size.map(|_| "default")) else { return Ok(()) };
        let size = size.map(|s| format!(" {s}")).unwrap_or_default();
        self.run(&format!("seat * xcursor_theme {theme}{size}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sway").join(name)).unwrap()
    }

    fn workspaces() -> Vec<SwayWorkspace> {
        serde_json::from_str(&fixture("workspaces.json")).unwrap()
    }

    fn tree() -> Tree {
        Tree::of(&serde_json::from_str(&fixture("tree.json")).unwrap())
    }

    #[test]
    fn frames_round_trip() {
        let bytes = encode(msg::GET_TREE, b"");
        assert_eq!(&bytes[..6], b"i3-ipc");
        assert_eq!(&bytes[6..14], &[0, 0, 0, 0, 4, 0, 0, 0]);
        let mut framed = encode(event::WINDOW, br#"{"change":"focus"}"#);
        framed.extend(encode(msg::RUN_COMMAND, b"[]"));
        let mut r = std::io::Cursor::new(framed);
        assert_eq!(read_message(&mut r).unwrap(), (event::WINDOW, br#"{"change":"focus"}"#.to_vec()));
        assert_eq!(read_message(&mut r).unwrap(), (msg::RUN_COMMAND, b"[]".to_vec()));
        assert!(read_message(&mut std::io::Cursor::new(b"not-ipc\0\0\0\0\0\0\0".to_vec())).is_err());
        let mut huge = encode(0, b"");
        huge[6..10].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read_message(&mut std::io::Cursor::new(huge)).is_err(), "an absurd length is refused, not allocated");
    }

    /// Captured from sway 1.12 (headless, two outputs): workspaces 1, 3 and
    /// `web` on HEADLESS-1 (a floating X11 window on 3), 5 on HEADLESS-2
    /// with an urgent window.
    #[test]
    fn workspaces_are_per_output_with_tree_occupancy() {
        let (ws, tree) = (workspaces(), tree());
        let web = unnumbered_id("web");
        assert_eq!(workspace_ids(&ws), vec![1, 3, web, 5], "a named workspace's id is above every number");
        assert!(crate::model::is_unnumbered(web));
        let nav = navigation(&ws, &tree);
        assert_eq!(nav.iter().map(|(o, _)| o.as_str()).collect::<Vec<_>>(), ["HEADLESS-1", "HEADLESS-2"]);
        let NavigationModel::Workspaces { workspaces } = &nav[0].1 else { panic!("{nav:?}") };
        assert_eq!(workspaces.iter().map(|w| (w.id, w.name.as_str(), w.active, w.occupied)).collect::<Vec<_>>(), [(1, "1", true, true), (3, "3", false, true), (web, "web", false, true)]);
        assert_eq!(workspaces[0].layout, "splith");
        let NavigationModel::Workspaces { workspaces } = &nav[1].1 else { panic!("{nav:?}") };
        assert!(workspaces[0].urgent && workspaces[0].active && workspaces[0].id == 5);
        assert_eq!(nav[0].1.active_mask(), 0b1);
    }

    #[test]
    fn windows_come_from_the_tree() {
        let all = clients(&workspaces(), &tree());
        assert_eq!(all.len(), 4, "{all:#?}");
        let focused: Vec<&ClientInfo> = all.iter().filter(|c| c.focused).collect();
        assert_eq!(focused.len(), 1);
        assert_eq!((focused[0].app_id.as_str(), focused[0].tags.as_slice(), focused[0].monitor.as_str()), ("foot", &[1][..], "HEADLESS-1"));
        let xterm = all.iter().find(|c| c.app_id == "XTermLegacy").expect("an X11 window by its class");
        assert!(xterm.floating && xterm.tags == vec![3] && !xterm.fullscreen);
        assert_eq!(xterm.geometry, Some((716, 382, 488, 318)));
        let web = all.iter().find(|c| c.app_id == "org.example.Web").unwrap();
        assert_eq!(web.tags, vec![unnumbered_id("web")], "the named workspace's id");
        assert!(all.iter().any(|c| c.monitor == "HEADLESS-2" && c.tags == vec![5]));
        assert!(all.iter().all(|c| c.id > 0 && !c.title.is_empty()));
    }

    #[test]
    fn outputs_become_monitors() {
        let outputs: Vec<SwayOutput> = serde_json::from_str(&fixture("outputs.json")).unwrap();
        let m = monitors(&outputs);
        assert_eq!(m.iter().map(|m| (m.name.as_str(), m.x, m.width, m.height, m.scale, m.active)).collect::<Vec<_>>(), [("HEADLESS-1", 0, 1920, 1080, 1.0, true), ("HEADLESS-2", 1920, 1920, 1080, 1.0, false)]);
        let disabled: Vec<SwayOutput> = serde_json::from_str(r#"[{"name":"eDP-1","active":false,"focused":false,"rect":{"x":0,"y":0,"width":0,"height":0},"scale":-1.0}]"#).unwrap();
        assert!(monitors(&disabled).is_empty());
    }

    #[test]
    fn command_replies() {
        let reply = |name: &str| serde_json::from_str::<Value>(&fixture(name)).unwrap();
        assert!(command_result(&reply("command_ok.json")).is_ok());
        let err = command_result(&reply("command_err.json")).unwrap_err().to_string();
        assert!(err.contains("Unknown/invalid command 'frobnicate'"), "{err}");
        assert!(command_result(&reply("command_mixed.json")).is_err(), "one failure fails the list");
        let version: Value = serde_json::from_str(&fixture("version.json")).unwrap();
        assert_eq!((version["major"].as_u64(), version["minor"].as_u64()), (Some(1), Some(12)));
        assert_eq!(serde_json::from_str::<Value>(&fixture("binding_state_resize.json")).unwrap()["name"], "resize");
    }

    #[test]
    fn commands_for_the_bar() {
        let ws = workspaces();
        assert_eq!(select_command(&ws, 3, "HEADLESS-1"), "workspace number 3");
        let web = unnumbered_id("web");
        assert_eq!(select_command(&ws, web, ""), "workspace \"web\"");
        // Numbered workspace 6 doesn't exist yet: still reachable with `web` there.
        assert_eq!(select_command(&ws, 6, ""), "workspace number 6");
        assert_eq!(move_command(&ws, None, 6), "move container to workspace number 6");
        assert_eq!(select_command(&ws, 4, "HEADLESS-2"), "focus output \"HEADLESS-2\"; workspace number 4", "a new workspace opens on the clicked bar's output");
        assert_eq!(move_command(&ws, Some(13), 3), "[con_id=13] move container to workspace number 3");
        assert_eq!(move_command(&ws, None, web), "move container to workspace \"web\"");
        assert_eq!(move_command(&ws, None, 9), "move container to workspace number 9");
    }

    #[test]
    fn named_workspace_ids_stay_put() {
        let named = |name: &str, num: i32| SwayWorkspace { num, name: name.into(), output: "HEADLESS-1".into(), ..Default::default() };
        let before = workspace_ids(&[named("1", 1), named("web", -1), named("mail", -1)]);
        // Workspaces come and go (a higher number, one closed): `mail` keeps
        // its id, so a bar click captured before still finds it.
        let after = workspace_ids(&[named("mail", -1), named("1", 1), named("9", 9)]);
        assert_eq!(before[2], after[0]);
        assert!(before.iter().chain(&after).filter(|i| crate::model::is_unnumbered(**i)).all(|i| *i > i32::MAX as u32));
        // Two names on the same hash take distinct ids.
        let ws = [named("web", -1), named("web", -1)];
        let ids = workspace_ids(&ws);
        assert_ne!(ids[0], ids[1]);
    }

    /// What each name must look like for sway 1.12 to reach that very
    /// workspace (`swaymsg` against a headless sway: backslashes are kept,
    /// so quotes are chosen, never escaped).
    #[test]
    fn names_are_quoted_for_sway() {
        assert_eq!(quoted("web"), "\"web\"");
        assert_eq!(quoted("dev ops; x,y"), "\"dev ops; x,y\"");
        assert_eq!(quoted("it's"), "\"it's\"");
        assert_eq!(quoted("a \"b\""), "'a \"b\"'");
        assert_eq!(quoted("back\\slash"), "\"back\\slash\"");
        let ws = [SwayWorkspace { num: -1, name: "say \"hi\"".into(), ..Default::default() }];
        assert_eq!(select_command(&ws, workspace_ids(&ws)[0], ""), "workspace 'say \"hi\"'");
    }

    /// The real event stream (type, payload), as a subscriber read it from
    /// sway 1.12: workspace and window events, the output and mode events.
    #[test]
    fn captured_events_are_understood() {
        let lines = fixture("events.tsv");
        let mut kinds = std::collections::BTreeSet::new();
        for line in lines.lines() {
            let (kind, body) = line.split_once('\t').unwrap();
            let kind: u32 = kind.parse().unwrap();
            let body: Value = serde_json::from_str(body).unwrap();
            kinds.insert(kind);
            match kind {
                event::WINDOW => {
                    let c: SwayNode = serde_json::from_value(body["container"].clone()).unwrap();
                    assert!(c.id > 0 && matches!(c.kind.as_str(), "con" | "floating_con"), "{body}");
                }
                event::WORKSPACE => assert!(body["current"]["type"] == "workspace", "{body}"),
                event::MODE => assert!(body["change"].is_string()),
                _ => {}
            }
        }
        assert!([msg::SUBSCRIBE, event::WORKSPACE, event::OUTPUT, event::MODE, event::WINDOW].iter().all(|k| kinds.contains(k)), "{kinds:?}");
    }

    #[test]
    fn state_only_sends_changes() {
        let (ws, tree) = (workspaces(), tree());
        let mut st = SwayState::default();
        let first = st.update(&ws, &tree);
        assert_eq!(first.iter().filter(|e| matches!(e, CompositorEvent::Navigation { .. })).count(), 2);
        assert!(first.iter().any(|e| matches!(e, CompositorEvent::FocusedClient(Some(c)) if c.app_id == "foot")));
        assert!(st.update(&ws, &tree).is_empty(), "nothing changed");
        let mut renamed = tree.clone();
        let focused = renamed.windows.iter_mut().find(|w| w.0.focused).unwrap();
        focused.0.name = Some("renamed".into());
        let out = st.update(&ws, &renamed);
        assert!(out.iter().any(|e| matches!(e, CompositorEvent::FocusedClient(Some(c)) if c.title == "renamed")));
        assert!(!out.iter().any(|e| matches!(e, CompositorEvent::Navigation { .. })), "a title change leaves the workspaces alone");
    }

    /// End to end against a running sway, when `MANDARA_TEST_SWAYSOCK`
    /// names its socket (a throwaway headless one: this switches its
    /// workspaces). Skipped otherwise.
    #[test]
    fn live_sway_round_trip() {
        let Some(socket) = std::env::var_os("MANDARA_TEST_SWAYSOCK") else {
            eprintln!("MANDARA_TEST_SWAYSOCK isn't set; skipped");
            return;
        };
        let driver = SwayDriver::with_socket(socket);
        assert!(driver.version().is_some_and(|v| v >= (1, 0)));
        let monitors = driver.get_monitors().unwrap();
        assert!(!monitors.is_empty(), "a headless output");
        let events = driver.subscribe_events();
        let first = events.recv_blocking().unwrap();
        assert!(matches!(first, CompositorEvent::Navigation { .. }), "the snapshot comes first: {first:?}");
        let output = monitors[0].name.clone();
        driver.select_primary(&output, 7).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut seen = false;
        while std::time::Instant::now() < deadline && !seen {
            if let Ok(CompositorEvent::Navigation { model: NavigationModel::Workspaces { workspaces }, .. }) = events.recv_blocking() {
                seen = workspaces.iter().any(|w| w.id == 7 && w.active);
            }
        }
        assert!(seen, "workspace 7 shown through the event stream");
        let nav = driver.get_navigation_state().unwrap();
        assert!(nav.iter().any(|(o, m)| *o == output && m.active_mask() & (1 << 6) != 0), "{nav:?}");
        assert!(driver.dispatch_raw("frobnicate", &[]).unwrap_err().to_string().contains("frobnicate"), "sway's error comes back");
        driver.cycle_layout(&output).unwrap();
        driver.apply_cursor(Some("Adwaita"), Some(32)).unwrap();
        driver.select_primary(&output, 1).unwrap();
        eprintln!("live sway: {nav:?}");
    }

    /// `GET_INPUTS` entries and `input` events (`sway-ipc(7)`): the active
    /// layout of a keyboard; other devices say nothing.
    #[test]
    fn keyboard_layouts() {
        let kb: Value = serde_json::json!({"identifier":"1:1:AT_Translated_Set_2_keyboard","type":"keyboard","xkb_layout_names":["English (US)","Arabic"],"xkb_active_layout_index":1,"xkb_active_layout_name":"Arabic"});
        assert_eq!(keyboard_layout_name(&kb).as_deref(), Some("Arabic"));
        assert_eq!(keyboard_layout_name(&serde_json::json!({"type":"pointer"})), None);
    }
}
