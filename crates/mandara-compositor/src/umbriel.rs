//! Umbriel driver: newline-delimited JSON over `$UMBRIEL_SOCKET`.
//!
//! Every request and reply is one JSON object per line (Umbriel's
//! `docs/user/ipc.md`). Queries are `{"cmd":"workspaces"}`,
//! `{"cmd":"windows"}`, `{"cmd":"focused-window"}`, `{"cmd":"outputs"}`,
//! answered with `{"ok": …}` or `{"err": "…"}`. Actions are typed requests,
//! `{"cmd":"action","action":"workspace-switch","workspace":2,"output":"DP-1"}`.
//! `{"cmd":"subscribe","events":[…]}` turns the connection into a stream of
//! `{"event": "<family>", "data": …}` lines: first the current state of each
//! family, then a full snapshot (never a delta) whenever one changes.
//!
//! Umbriel has per-output, linear workspaces (dynamic or fixed, each with
//! its own layout: scrolling, dwindle, master or stacking), so it maps to
//! [`NavigationModel::Workspaces`] with the one-based position as the id
//! (the number `workspace-switch` takes), and its keybind submaps to the
//! bar's keymode badge.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::backend::CompositorBackend;
use crate::model::{ClientInfo, CompositorEvent, CompositorKind, MonitorInfo, NavigationModel, WorkspaceStatus};

/// The event families the shell follows (Umbriel's subscription names).
pub const EVENT_FAMILIES: [&str; 5] = ["workspaces", "windows", "outputs", "submap", "keyboard_layout"];

/// One entry of `{"cmd":"workspaces"}` / the `workspaces` event.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct UmbrielWorkspace {
    /// Stable id (a string; it survives moves between outputs).
    pub id: String,
    /// Display name: an explicit name, or the position for anonymous ones.
    pub name: String,
    pub named: bool,
    /// One-based position on its output.
    pub index: u32,
    pub output: String,
    pub active: bool,
    /// Active on the pointer-preferred (focused) output.
    pub focused: bool,
    pub occupied: bool,
    pub window_count: u32,
    /// Id of the window it last focused, `""` when none.
    pub focused_window: String,
    pub urgent: bool,
    /// `scrolling`, `dwindle`, `master` or `stacking`.
    pub layout: String,
}

/// One entry of `{"cmd":"windows"}` / the `windows` event.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct UmbrielWindow {
    /// Stable id (the ext-foreign-toplevel identifier, a string).
    pub id: String,
    pub app_id: String,
    pub title: String,
    /// The id of its workspace.
    pub workspace: String,
    pub output: String,
    pub floating: bool,
    pub fullscreen: bool,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub w: Option<i32>,
    pub h: Option<i32>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Xy {
    x: i32,
    y: i32,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Size {
    width: i32,
    height: i32,
}

/// One entry of `{"cmd":"outputs"}` / the `outputs` event (the fields the
/// shell uses).
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct UmbrielOutput {
    pub name: String,
    /// Part of the desktop (disconnected-but-listed outputs are not).
    pub enabled: bool,
    pub focused: bool,
    position: Xy,
    logical_size: Size,
    pub scale: f64,
}

impl Default for UmbrielOutput {
    fn default() -> Self {
        Self { name: String::new(), enabled: true, focused: false, position: Xy::default(), logical_size: Size::default(), scale: 1.0 }
    }
}

impl UmbrielOutput {
    fn monitor(&self) -> MonitorInfo {
        MonitorInfo { name: self.name.clone(), x: self.position.x, y: self.position.y, width: self.logical_size.width, height: self.logical_size.height, scale: self.scale as f32, active: self.focused }
    }
}

/// Enabled outputs as monitors.
pub fn monitors(outputs: &[UmbrielOutput]) -> Vec<MonitorInfo> {
    outputs.iter().filter(|o| o.enabled).map(UmbrielOutput::monitor).collect()
}

/// FNV-1a offset basis and prime (64-bit).
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// The numeric id the shell keeps for an Umbriel window id (a string):
/// a stable 64-bit FNV-1a hash, mapped back through the window list when
/// the shell acts on it.
pub fn client_id(id: &str) -> u64 {
    id.bytes().fold(FNV_OFFSET, |h, b| (h ^ u64::from(b)).wrapping_mul(FNV_PRIME))
}

/// The id of the window that has focus: the focused workspace's last
/// focused window (Umbriel's `focused` on a workspace means "active on the
/// focused output").
pub fn focused_window_id(workspaces: &[UmbrielWorkspace]) -> Option<&str> {
    workspaces.iter().find(|w| w.focused).map(|w| w.focused_window.as_str()).filter(|id| !id.is_empty())
}

impl UmbrielWindow {
    fn client(&self, workspaces: &[UmbrielWorkspace], focused: Option<&str>) -> ClientInfo {
        let ws = workspaces.iter().find(|w| w.id == self.workspace);
        ClientInfo {
            id: client_id(&self.id),
            title: self.title.clone(),
            app_id: self.app_id.clone(),
            monitor: if self.output.is_empty() { ws.map(|w| w.output.clone()).unwrap_or_default() } else { self.output.clone() },
            tags: ws.map(|w| vec![w.index]).unwrap_or_default(),
            focused: focused == Some(self.id.as_str()),
            floating: self.floating,
            fullscreen: self.fullscreen,
            minimized: false,
            geometry: match (self.x, self.y, self.w, self.h) {
                (Some(x), Some(y), Some(w), Some(h)) => Some((x, y, w, h)),
                _ => None,
            },
        }
    }
}

/// Every window as a client.
pub fn clients(windows: &[UmbrielWindow], workspaces: &[UmbrielWorkspace]) -> Vec<ClientInfo> {
    let focused = focused_window_id(workspaces);
    windows.iter().map(|w| w.client(workspaces, focused)).collect()
}

/// The focused window as a client.
pub fn focused_client(windows: &[UmbrielWindow], workspaces: &[UmbrielWorkspace]) -> Option<ClientInfo> {
    let focused = focused_window_id(workspaces)?;
    windows.iter().find(|w| w.id == focused).map(|w| w.client(workspaces, Some(focused)))
}

/// Groups workspaces into per-output navigation models (positions in
/// order; outputs by name).
pub fn navigation(workspaces: &[UmbrielWorkspace]) -> Vec<(String, NavigationModel)> {
    let mut by_output: BTreeMap<String, Vec<&UmbrielWorkspace>> = BTreeMap::new();
    for w in workspaces {
        by_output.entry(w.output.clone()).or_default().push(w);
    }
    by_output
        .into_iter()
        .map(|(output, mut list)| {
            list.sort_by_key(|w| w.index);
            let workspaces = list
                .into_iter()
                .map(|w| WorkspaceStatus { id: w.index, name: if w.name.is_empty() { w.index.to_string() } else { w.name.clone() }, active: w.active, occupied: w.occupied, urgent: w.urgent, layout: w.layout.clone() })
                .collect();
            (output, NavigationModel::Workspaces { workspaces })
        })
        .collect()
}

/// The `ok` payload of a reply line, or its `err` as an error.
pub fn unwrap_reply(reply: &Value) -> Result<Value> {
    if let Some(err) = reply.get("err") {
        bail!("umbriel: {}", err.as_str().map_or_else(|| err.to_string(), str::to_string));
    }
    reply.get("ok").cloned().ok_or_else(|| anyhow!("umbriel: malformed reply {reply}"))
}

/// A typed action request: `{"cmd":"action","action":<name>, …fields}`.
pub fn action_request(action: &str, fields: Value) -> Value {
    let mut body = Map::new();
    body.insert("cmd".into(), json!("action"));
    body.insert("action".into(), json!(action));
    if let Value::Object(extra) = fields {
        // `cmd` and `action` are ours; everything else is the action's.
        body.extend(extra.into_iter().filter(|(k, _)| k != "cmd" && k != "action"));
    }
    Value::Object(body)
}

/// `workspace-switch` to position `index` (as a number: a string would be a
/// name), on `output` when one is given.
pub fn workspace_switch(index: u32, output: &str) -> Value {
    let mut fields = json!({ "workspace": index });
    if !output.is_empty() {
        fields["output"] = json!(output);
    }
    action_request("workspace-switch", fields)
}

/// The request subscribing to every family the shell follows.
pub fn subscribe_request() -> Value {
    json!({ "cmd": "subscribe", "events": EVENT_FAMILIES })
}

/// The active layout's name in a `keyboard_layout` event
/// (`{"names": […], "current_index": n}`).
pub fn keyboard_layout_name(data: &Value) -> Option<String> {
    let index = usize::try_from(data.get("current_index")?.as_u64()?).ok()?;
    data.get("names")?.as_array()?.get(index)?.as_str().map(str::to_string)
}

/// Workspaces and windows as Umbriel's event stream reports them, plus the
/// focused client last sent (snapshots repeat it; the bar only needs
/// changes).
#[derive(Default)]
pub struct UmbrielState {
    pub workspaces: Vec<UmbrielWorkspace>,
    pub windows: Vec<UmbrielWindow>,
    last_focused: Option<Option<ClientInfo>>,
}

impl UmbrielState {
    /// Applies one event-stream line and returns what the shell needs to
    /// hear. An `{"err": …}` line (an unknown family) is an error: Umbriel
    /// closes that subscription.
    pub fn apply(&mut self, line: &Value) -> Result<Vec<CompositorEvent>> {
        if line.get("err").is_some() {
            unwrap_reply(line)?;
        }
        let data = line.get("data").cloned().unwrap_or(Value::Null);
        let mut out = Vec::new();
        match line.get("event").and_then(Value::as_str) {
            Some("workspaces") => {
                self.workspaces = serde_json::from_value(data)?;
                out.extend(navigation(&self.workspaces).into_iter().map(|(monitor, model)| CompositorEvent::Navigation { monitor, model }));
                // Positions (the clients' tags) and focus follow workspaces.
                out.push(CompositorEvent::Clients(clients(&self.windows, &self.workspaces)));
                self.push_focus(&mut out);
            }
            Some("windows") => {
                self.windows = serde_json::from_value(data)?;
                out.push(CompositorEvent::Clients(clients(&self.windows, &self.workspaces)));
                self.push_focus(&mut out);
            }
            Some("outputs") => {
                let outputs: Vec<UmbrielOutput> = serde_json::from_value(data)?;
                out.push(CompositorEvent::Monitors(monitors(&outputs)));
            }
            // The active submap's name, or null outside one.
            Some("submap") => out.push(CompositorEvent::Keymode(data.as_str().unwrap_or_default().to_string())),
            Some("keyboard_layout") => {
                if let Some(name) = keyboard_layout_name(&data) {
                    out.push(CompositorEvent::KeyboardLayout(name));
                }
            }
            _ => {}
        }
        Ok(out)
    }

    fn push_focus(&mut self, out: &mut Vec<CompositorEvent>) {
        let focused = focused_client(&self.windows, &self.workspaces);
        if self.last_focused.as_ref() != Some(&focused) {
            self.last_focused = Some(focused.clone());
            out.push(CompositorEvent::FocusedClient(focused));
        }
    }
}

pub struct UmbrielDriver {
    socket: PathBuf,
}

impl UmbrielDriver {
    /// `$UMBRIEL_SOCKET`, or `$XDG_RUNTIME_DIR/umbriel-$WAYLAND_DISPLAY.sock`
    /// (Umbriel's documented fallback), when it exists.
    pub fn from_env() -> Option<Self> {
        let socket = match std::env::var_os("UMBRIEL_SOCKET") {
            Some(s) => PathBuf::from(s),
            None => {
                let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
                let display = std::env::var("WAYLAND_DISPLAY").ok()?;
                runtime.join(format!("umbriel-{display}.sock"))
            }
        };
        socket.exists().then_some(Self { socket })
    }

    fn request(&self, body: &Value) -> Result<Value> {
        let mut stream = UnixStream::connect(&self.socket).with_context(|| format!("connecting {}", self.socket.display()))?;
        stream.set_read_timeout(Some(crate::REQUEST_TIMEOUT))?;
        writeln!(stream, "{body}")?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    }

    fn query<T: serde::de::DeserializeOwned>(&self, cmd: &str) -> Result<T> {
        Ok(serde_json::from_value(unwrap_reply(&self.request(&json!({ "cmd": cmd }))?)?)?)
    }

    fn workspaces(&self) -> Result<Vec<UmbrielWorkspace>> {
        self.query("workspaces")
    }

    fn windows(&self) -> Result<Vec<UmbrielWindow>> {
        self.query("windows")
    }

    fn send(&self, request: &Value) -> Result<()> {
        unwrap_reply(&self.request(request)?).map(|_| ())
    }

    fn action(&self, action: &str, fields: Value) -> Result<()> {
        self.send(&action_request(action, fields))
    }
}

impl CompositorBackend for UmbrielDriver {
    fn kind(&self) -> CompositorKind {
        CompositorKind::Umbriel
    }

    fn version(&self) -> Option<(u32, u32)> {
        mandara_config::compositor::installed_version(mandara_config::compositor::Target::Umbriel)
    }

    fn subscribe_events(&self) -> async_channel::Receiver<CompositorEvent> {
        let (tx, rx) = async_channel::bounded(crate::EVENT_QUEUE);
        let socket = self.socket.clone();
        let _ = std::thread::Builder::new().name("umbriel-events".into()).spawn(move || {
            loop {
                let result = (|| -> Result<()> {
                    let mut stream = UnixStream::connect(&socket)?;
                    writeln!(stream, "{}", subscribe_request())?;
                    let mut state = UmbrielState::default();
                    for line in BufReader::new(stream).lines() {
                        let line: Value = serde_json::from_str(&line?)?;
                        for e in state.apply(&line)? {
                            if tx.send_blocking(e).is_err() {
                                return Ok(());
                            }
                        }
                    }
                    bail!("stream closed")
                })();
                if result.is_ok() || tx.is_closed() {
                    return;
                }
                std::thread::sleep(crate::RECONNECT_DELAY);
            }
        });
        rx
    }

    fn get_navigation_state(&self) -> Result<Vec<(String, NavigationModel)>> {
        Ok(navigation(&self.workspaces()?))
    }

    fn get_active_client(&self) -> Result<Option<ClientInfo>> {
        // `{"err":"no focused window"}` when nothing has focus (a socket
        // failure is still an error).
        let reply = self.request(&json!({ "cmd": "focused-window" }))?;
        if reply.get("err").is_some() {
            return Ok(None);
        }
        let window: UmbrielWindow = serde_json::from_value(unwrap_reply(&reply)?)?;
        let workspaces = self.workspaces().unwrap_or_default();
        Ok(Some(window.client(&workspaces, Some(window.id.as_str()))))
    }

    fn get_all_clients(&self) -> Result<Vec<ClientInfo>> {
        Ok(clients(&self.windows()?, &self.workspaces().unwrap_or_default()))
    }

    fn get_monitors(&self) -> Result<Vec<MonitorInfo>> {
        Ok(monitors(&self.query::<Vec<UmbrielOutput>>("outputs")?))
    }

    fn select_primary(&self, monitor: &str, id: u32) -> Result<()> {
        self.send(&workspace_switch(id, monitor))
    }

    fn toggle_secondary(&self, monitor: &str, id: u32) -> Result<()> {
        // No composite views: a workspace switch, as on Niri and Hyprland.
        self.select_primary(monitor, id)
    }

    fn move_client_to(&self, client_id: Option<u64>, id: u32) -> Result<()> {
        // Umbriel's window moves act on the focused window: another window
        // is focused first. Position `id` on the window's own output (the
        // bar shows per-output positions).
        let windows = self.windows().unwrap_or_default();
        let workspaces = self.workspaces().unwrap_or_default();
        let focused = focused_window_id(&workspaces).map(str::to_string);
        let window = match client_id {
            Some(cid) => Some(windows.iter().find(|w| crate::umbriel::client_id(&w.id) == cid).context("umbriel: no such window")?),
            None => focused.as_deref().and_then(|f| windows.iter().find(|w| w.id == f)),
        };
        if let Some(w) = window.filter(|w| Some(&w.id) != focused.as_ref()) {
            self.action("window-focus", json!({ "window": w.id }))?;
        }
        let output = window.map(|w| w.output.clone()).filter(|o| !o.is_empty()).or_else(|| workspaces.iter().find(|w| w.focused).map(|w| w.output.clone())).unwrap_or_default();
        let mut fields = json!({ "workspace": id });
        if !output.is_empty() {
            fields["output"] = json!(output);
        }
        self.action("window-move-to-workspace-silent", fields)
    }

    fn toggle_client_tag(&self, client_id: Option<u64>, tag: u32) -> Result<()> {
        self.move_client_to(client_id, tag)
    }

    fn cycle_layout(&self, _monitor: &str) -> Result<()> {
        // The active workspace's layout: scrolling → dwindle → master →
        // stacking.
        self.action("workspace-set-layout", json!({ "layout": "toggle" }))
    }

    fn dispatch_raw(&self, cmd: &str, args: &[&str]) -> Result<()> {
        match args {
            [] => self.action(cmd, Value::Null),
            // Typed fields as one JSON object (`{"workspace":2}`).
            [one] if one.trim_start().starts_with('{') => self.action(cmd, serde_json::from_str(one).context("umbriel: the argument is not a JSON object")?),
            // Words, as `umbriel msg <action> [args…]` parses them.
            words => {
                let out = std::process::Command::new("umbriel").arg("msg").arg(cmd).args(words).env("UMBRIEL_SOCKET", &self.socket).stdin(std::process::Stdio::null()).output().context("running `umbriel msg`")?;
                if out.status.success() {
                    Ok(())
                } else {
                    bail!("umbriel: {}", String::from_utf8_lossy(&out.stderr).trim())
                }
            }
        }
    }

    fn reload_config(&self) -> Result<()> {
        // Umbriel also watches the config and its includes; this applies
        // the drop-in at once.
        self.action("config-reload", Value::Null)
    }

    fn switch_keyboard_layout(&self) -> Result<()> {
        self.action("keyboard-layout-next", Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `{"cmd":"workspaces"}` in Umbriel's shape (two outputs, a named
    /// workspace, the focused one with its last focused window).
    const WORKSPACES: &str = r#"{"ok":[
        {"id":"ws-3","name":"2","named":false,"index":2,"output":"DP-1","active":false,"focused":false,"occupied":false,"window_count":0,"focused_window":"","urgent":true,"layout":"dwindle","layout_override":false},
        {"id":"ws-1","name":"WEB","named":true,"index":1,"output":"DP-1","active":true,"focused":true,"occupied":true,"window_count":2,"focused_window":"a1b2","urgent":false,"layout":"scrolling","layout_override":false},
        {"id":"ws-7","name":"1","named":false,"index":1,"output":"HDMI-A-1","active":true,"focused":false,"occupied":true,"window_count":1,"focused_window":"c3d4","urgent":false,"layout":"master","layout_override":true}
    ]}"#;

    const WINDOWS: &str = r#"{"ok":[
        {"id":"a1b2","workspace":"ws-1","scratchpad":"","active":true,"app_id":"firefox","title":"Docs","xdg_tag":"","content_type":"none","floating":false,"title_bar":false,"fullscreen":false,"maximized":false,"maximized_to_edges":false,"pinned":false,"visible":true,"output":"DP-1","parent":"","column":0,"row":0,"tabbed":false,"tab_index":-1,"tab_hidden":false,"minimizable":false,"minimized":false,"region":null,"focused":true,"urgent":false,"xwayland":false,"pid":42,"x":10,"y":20,"w":800,"h":600},
        {"id":"e5f6","workspace":"ws-1","app_id":"foot","title":"shell","floating":true,"fullscreen":false,"output":"DP-1","focused":false,"pid":43,"x":0,"y":0,"w":400,"h":300},
        {"id":"c3d4","workspace":"ws-7","app_id":"mpv","title":"film","floating":false,"fullscreen":true,"output":"HDMI-A-1","focused":true,"pid":44}
    ]}"#;

    fn workspaces() -> Vec<UmbrielWorkspace> {
        serde_json::from_value(unwrap_reply(&serde_json::from_str(WORKSPACES).unwrap()).unwrap()).unwrap()
    }

    fn windows() -> Vec<UmbrielWindow> {
        serde_json::from_value(unwrap_reply(&serde_json::from_str(WINDOWS).unwrap()).unwrap()).unwrap()
    }

    #[test]
    fn workspaces_are_per_output_positions() {
        let nav = navigation(&workspaces());
        assert_eq!(nav.iter().map(|(o, _)| o.as_str()).collect::<Vec<_>>(), ["DP-1", "HDMI-A-1"]);
        let (_, NavigationModel::Workspaces { workspaces }) = &nav[0] else { panic!("{nav:?}") };
        assert_eq!(workspaces.iter().map(|w| (w.id, w.name.as_str())).collect::<Vec<_>>(), [(1, "WEB"), (2, "2")], "sorted by position; named kept");
        assert!(workspaces[0].active && workspaces[0].occupied && !workspaces[0].urgent);
        assert!(!workspaces[1].active && workspaces[1].urgent);
        assert_eq!(workspaces[0].layout, "scrolling");
        assert_eq!(nav[0].1.active_mask(), 0b1);
    }

    #[test]
    fn windows_become_clients_with_focus_from_the_focused_workspace() {
        let (ws, wins) = (workspaces(), windows());
        let all = clients(&wins, &ws);
        assert_eq!(all.len(), 3);
        // Only the focused workspace's window has focus (`focused` on a
        // window is per workspace, so mpv's own flag doesn't count).
        assert_eq!(all.iter().filter(|c| c.focused).map(|c| c.app_id.as_str()).collect::<Vec<_>>(), ["firefox"]);
        let ff = &all[0];
        assert_eq!((ff.monitor.as_str(), ff.tags.clone(), ff.geometry), ("DP-1", vec![1], Some((10, 20, 800, 600))));
        assert_eq!(ff.id, client_id("a1b2"));
        assert_ne!(client_id("a1b2"), client_id("c3d4"));
        let mpv = &all[2];
        assert!(mpv.fullscreen && mpv.geometry.is_none() && mpv.tags == vec![1] && mpv.monitor == "HDMI-A-1");
        assert!(all[1].floating);
        assert_eq!(focused_client(&wins, &ws).map(|c| c.title), Some("Docs".into()));
    }

    #[test]
    fn outputs_become_monitors() {
        let outputs: Vec<UmbrielOutput> = serde_json::from_value(unwrap_reply(&serde_json::from_str(
            r#"{"ok":[
                {"name":"DP-1","description":"Dell","make":"Dell","model":"U2720Q","serial":"X","config_name":"Dell U2720Q X","physical_size":{"width_mm":600,"height_mm":340},"enabled":true,"powered":true,"focused":true,"active_workspace":"ws-1","position":{"x":0,"y":0},"logical_size":{"width":2560,"height":1440},"usable_area":{"x":0,"y":30,"width":2560,"height":1410},"transform":"normal","scale":1.5,"adaptive_sync":null,"hdr_active":false,"direct_scanout":null,"modes":[]},
                {"name":"HDMI-A-1","enabled":false,"focused":false,"position":{"x":0,"y":0},"logical_size":{"width":0,"height":0},"scale":1.0}
            ]}"#,
        ).unwrap()).unwrap()).unwrap();
        let m = monitors(&outputs);
        assert_eq!(m.len(), 1, "disabled outputs are not monitors");
        assert_eq!((m[0].name.as_str(), m[0].width, m[0].height, m[0].scale, m[0].active), ("DP-1", 2560, 1440, 1.5, true));
    }

    #[test]
    fn requests_are_typed_actions() {
        assert_eq!(workspace_switch(2, "DP-1"), json!({"cmd":"action","action":"workspace-switch","workspace":2,"output":"DP-1"}));
        // A position is a number: the string "2" would select a name.
        assert_eq!(workspace_switch(3, ""), json!({"cmd":"action","action":"workspace-switch","workspace":3}));
        assert_eq!(action_request("workspace-set-layout", json!({"layout":"toggle","action":"x"})), json!({"cmd":"action","action":"workspace-set-layout","layout":"toggle"}));
        assert_eq!(action_request("config-reload", Value::Null), json!({"cmd":"action","action":"config-reload"}));
        assert_eq!(subscribe_request(), json!({"cmd":"subscribe","events":["workspaces","windows","outputs","submap","keyboard_layout"]}));
    }

    #[test]
    fn errors_are_surfaced() {
        let err = unwrap_reply(&json!({"err":"no focused window"})).unwrap_err().to_string();
        assert!(err.contains("no focused window"), "{err}");
        assert_eq!(unwrap_reply(&json!({"ok":null})).unwrap(), Value::Null);
        assert!(unwrap_reply(&json!({"surprise":1})).is_err());
    }

    /// Event-stream lines in Umbriel's shape: initial snapshots, then full
    /// snapshots on change.
    #[test]
    fn event_stream_keeps_state_current() {
        let ev = |family: &str, data: Value| json!({ "event": family, "data": data });
        let mut st = UmbrielState::default();
        let wins: Value = unwrap_reply(&serde_json::from_str(WINDOWS).unwrap()).unwrap();
        let ws: Value = unwrap_reply(&serde_json::from_str(WORKSPACES).unwrap()).unwrap();
        // Windows arrive before workspaces: no focus known yet.
        let out = st.apply(&ev("windows", wins.clone())).unwrap();
        assert!(out.iter().any(|e| matches!(e, CompositorEvent::Clients(c) if c.len() == 3)));
        assert!(out.iter().any(|e| matches!(e, CompositorEvent::FocusedClient(None))));
        let out = st.apply(&ev("workspaces", ws.clone())).unwrap();
        assert_eq!(out.iter().filter(|e| matches!(e, CompositorEvent::Navigation { .. })).count(), 2);
        assert!(out.iter().any(|e| matches!(e, CompositorEvent::FocusedClient(Some(c)) if c.title == "Docs" && c.tags == vec![1])), "{out:?}");
        // The same snapshot again: focus unchanged, not re-sent.
        let out = st.apply(&ev("workspaces", ws.clone())).unwrap();
        assert!(!out.iter().any(|e| matches!(e, CompositorEvent::FocusedClient(_))));
        // A title change of the focused window reaches the bar.
        let mut renamed = wins.clone();
        renamed[0]["title"] = json!("Docs — edited");
        let out = st.apply(&ev("windows", renamed)).unwrap();
        assert!(out.iter().any(|e| matches!(e, CompositorEvent::FocusedClient(Some(c)) if c.title == "Docs — edited")));
        // Submaps drive the keymode badge; null leaves it.
        assert_eq!(st.apply(&ev("submap", json!("resize"))).unwrap(), vec![CompositorEvent::Keymode("resize".into())]);
        assert_eq!(st.apply(&ev("submap", Value::Null)).unwrap(), vec![CompositorEvent::Keymode(String::new())]);
        assert_eq!(st.apply(&ev("keyboard_layout", json!({"names":["English (US)","Arabic"],"current_index":1}))).unwrap(), vec![CompositorEvent::KeyboardLayout("Arabic".into())]);
        assert!(matches!(&st.apply(&ev("outputs", json!([{"name":"DP-1","enabled":true,"focused":true,"position":{"x":0,"y":0},"logical_size":{"width":1920,"height":1080},"scale":1.0}]))).unwrap()[..], [CompositorEvent::Monitors(m)] if m.len() == 1));
        // Families the shell doesn't follow change nothing; an error line
        // ends the stream.
        assert!(st.apply(&ev("theme", json!({}))).unwrap().is_empty());
        assert!(st.apply(&json!({"err":"unknown subscription event: x"})).is_err());
    }
}
