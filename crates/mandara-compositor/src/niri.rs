//! Niri driver: JSON requests and the event stream over `$NIRI_SOCKET`.
//!
//! Requests are single JSON values (`"Workspaces"`, `{"Action": …}`) answered
//! with `{"Ok": …}` / `{"Err": …}`; `"EventStream"` turns the connection into
//! a stream of JSON events (`WorkspacesChanged`, `WorkspaceActivated`,
//! `WindowFocusChanged`, `WindowsChanged`, …).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::backend::CompositorBackend;
use crate::model::{ClientInfo, CompositorEvent, CompositorKind, MonitorInfo, NavigationModel, WorkspaceStatus};

#[derive(Clone, Debug, Deserialize)]
pub struct NiriWorkspace {
    pub id: u64,
    pub idx: u32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default)]
    pub is_focused: bool,
    #[serde(default)]
    pub is_urgent: bool,
    #[serde(default)]
    pub active_window_id: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct NiriWindow {
    pub id: u64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<u64>,
    #[serde(default)]
    pub is_focused: bool,
    #[serde(default)]
    pub is_floating: bool,
}

impl NiriWindow {
    fn client(&self, workspaces: &[NiriWorkspace]) -> ClientInfo {
        let ws = workspaces.iter().find(|w| Some(w.id) == self.workspace_id);
        ClientInfo {
            id: self.id,
            title: self.title.clone().unwrap_or_default(),
            app_id: self.app_id.clone().unwrap_or_default(),
            monitor: ws.and_then(|w| w.output.clone()).unwrap_or_default(),
            tags: ws.map(|w| vec![w.idx]).unwrap_or_default(),
            focused: self.is_focused,
            floating: self.is_floating,
            fullscreen: false,
            geometry: None,
        }
    }
}

/// Groups workspaces into per-output navigation models.
pub fn navigation(workspaces: &[NiriWorkspace], windows: &[NiriWindow]) -> Vec<(String, NavigationModel)> {
    let mut by_output: HashMap<String, Vec<WorkspaceStatus>> = HashMap::new();
    for w in workspaces {
        let occupied = w.active_window_id.is_some() || windows.iter().any(|win| win.workspace_id == Some(w.id));
        by_output.entry(w.output.clone().unwrap_or_default()).or_default().push(WorkspaceStatus {
            id: w.idx,
            name: w.name.clone().unwrap_or_else(|| w.idx.to_string()),
            active: w.is_active,
            occupied,
            urgent: w.is_urgent,
        });
    }
    let mut out: Vec<(String, NavigationModel)> = by_output
        .into_iter()
        .map(|(output, mut list)| {
            list.sort_by_key(|w| w.id);
            (output, NavigationModel::Workspaces { workspaces: list })
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Unwraps `{"Ok": {"<Kind>": payload}}`.
pub fn unwrap_reply(reply: &Value, kind: &str) -> Result<Value> {
    if let Some(err) = reply.get("Err") {
        bail!("niri: {err}");
    }
    let ok = reply.get("Ok").ok_or_else(|| anyhow!("niri: malformed reply {reply}"))?;
    Ok(ok.get(kind).cloned().unwrap_or_else(|| ok.clone()))
}

/// Workspaces and windows as niri's event stream reports them.
#[derive(Default)]
pub struct NiriState {
    pub workspaces: Vec<NiriWorkspace>,
    pub windows: Vec<NiriWindow>,
}

impl NiriState {
    /// Applies one event-stream line (niri 26.04 `Event`) and returns what
    /// the shell needs to hear: the window list and per-output workspaces
    /// whenever windows or workspaces change, the focused window when focus
    /// or its title changes.
    pub fn apply(&mut self, mut event: Value) -> Result<Vec<CompositorEvent>> {
        let mut nav = false;
        let mut focus = false;
        if let Some(ws) = event.pointer_mut("/WorkspacesChanged/workspaces") {
            self.workspaces = serde_json::from_value(ws.take())?;
            nav = true;
        } else if let Some(act) = event.get("WorkspaceActivated") {
            let id = act.get("id").and_then(Value::as_u64);
            let focused = act.get("focused").and_then(Value::as_bool).unwrap_or(false);
            let output = self.workspaces.iter().find(|w| Some(w.id) == id).and_then(|w| w.output.clone());
            for w in self.workspaces.iter_mut() {
                if w.output == output {
                    w.is_active = Some(w.id) == id;
                }
                if focused {
                    w.is_focused = Some(w.id) == id;
                }
            }
            nav = true;
        } else if let Some(e) = event.get("WorkspaceActiveWindowChanged") {
            let id = e.get("workspace_id").and_then(Value::as_u64);
            if let Some(w) = self.workspaces.iter_mut().find(|w| Some(w.id) == id) {
                w.active_window_id = e.get("active_window_id").and_then(Value::as_u64);
            }
            nav = true;
        } else if let Some(e) = event.get("WorkspaceUrgencyChanged") {
            let id = e.get("id").and_then(Value::as_u64);
            if let Some(w) = self.workspaces.iter_mut().find(|w| Some(w.id) == id) {
                w.is_urgent = e.get("urgent").and_then(Value::as_bool).unwrap_or(false);
            }
            nav = true;
        } else if let Some(wins) = event.pointer_mut("/WindowsChanged/windows") {
            self.windows = serde_json::from_value(wins.take())?;
            nav = true;
            focus = true;
        } else if let Some(win) = event.pointer_mut("/WindowOpenedOrChanged/window") {
            let win: NiriWindow = serde_json::from_value(win.take())?;
            // A focused window takes focus from every other one.
            if win.is_focused {
                self.windows.iter_mut().for_each(|w| w.is_focused = false);
                focus = true;
            }
            match self.windows.iter_mut().find(|w| w.id == win.id) {
                Some(old) => {
                    focus |= old.is_focused != win.is_focused;
                    *old = win;
                }
                None => self.windows.push(win),
            }
            nav = true;
        } else if let Some(id) = event.pointer("/WindowClosed/id").and_then(Value::as_u64) {
            focus = self.windows.iter().any(|w| w.id == id && w.is_focused);
            self.windows.retain(|w| w.id != id);
            nav = true;
        } else if let Some(f) = event.get("WindowFocusChanged") {
            let id = f.get("id").and_then(Value::as_u64);
            for w in self.windows.iter_mut() {
                w.is_focused = Some(w.id) == id;
            }
            focus = true;
        }
        let mut out = Vec::new();
        if focus {
            out.push(CompositorEvent::FocusedClient(self.windows.iter().find(|w| w.is_focused).map(|w| w.client(&self.workspaces))));
        }
        if nav {
            out.push(CompositorEvent::Clients(self.windows.iter().map(|w| w.client(&self.workspaces)).collect()));
            out.extend(navigation(&self.workspaces, &self.windows).into_iter().map(|(monitor, model)| CompositorEvent::Navigation { monitor, model }));
        }
        Ok(out)
    }
}

pub struct NiriDriver {
    socket: PathBuf,
}

impl NiriDriver {
    pub fn from_env() -> Option<Self> {
        let socket = PathBuf::from(std::env::var_os("NIRI_SOCKET")?);
        socket.exists().then_some(Self { socket })
    }

    fn request(&self, body: Value) -> Result<Value> {
        let mut stream = UnixStream::connect(&self.socket).with_context(|| format!("connecting {}", self.socket.display()))?;
        stream.set_read_timeout(Some(crate::REQUEST_TIMEOUT))?;
        writeln!(stream, "{body}")?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    }

    fn workspaces(&self) -> Result<Vec<NiriWorkspace>> {
        Ok(serde_json::from_value(unwrap_reply(&self.request(json!("Workspaces"))?, "Workspaces")?)?)
    }

    fn windows(&self) -> Result<Vec<NiriWindow>> {
        Ok(serde_json::from_value(unwrap_reply(&self.request(json!("Windows"))?, "Windows")?)?)
    }

    fn action(&self, action: Value) -> Result<()> {
        unwrap_reply(&self.request(json!({ "Action": action }))?, "Handled").map(|_| ())
    }
}

/// A `WorkspaceReferenceArg` for the bar's per-output index `idx` on
/// `monitor`: the workspace's unique `Id` when it is known (an `Index`
/// alone always means the *focused* output), else the index.
pub fn workspace_reference(workspaces: &[NiriWorkspace], monitor: &str, idx: u32) -> Value {
    match workspaces.iter().find(|w| w.idx == idx && w.output.as_deref() == Some(monitor)) {
        Some(w) => json!({ "Id": w.id }),
        None => json!({ "Index": idx }),
    }
}

/// The `SwitchLayout` action to the next keyboard layout.
pub fn switch_layout_action() -> Value {
    json!({ "SwitchLayout": { "layout": "Next" } })
}

/// `(major, minor)` from a niri version string such as `26.04 (abc123)`.
pub fn parse_version(v: &str) -> Option<(u32, u32)> {
    let mut parts = v.trim().split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty());
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

impl CompositorBackend for NiriDriver {
    fn version(&self) -> Option<(u32, u32)> {
        let reply = self.request(json!("Version")).ok()?;
        let v = unwrap_reply(&reply, "Version").ok()?;
        parse_version(v.as_str()?)
    }

    fn kind(&self) -> CompositorKind {
        CompositorKind::Niri
    }

    fn subscribe_events(&self) -> async_channel::Receiver<CompositorEvent> {
        let (tx, rx) = async_channel::bounded(crate::EVENT_QUEUE);
        let socket = self.socket.clone();
        let _ = std::thread::Builder::new().name("niri-events".into()).spawn(move || {
            let state = Arc::new(Mutex::new(NiriState::default()));
            loop {
                let result = (|| -> Result<()> {
                    let mut stream = UnixStream::connect(&socket)?;
                    writeln!(stream, "\"EventStream\"")?;
                    for line in BufReader::new(stream).lines() {
                        let event: Value = serde_json::from_str(&line?)?;
                        let out = state.lock().map_err(|_| anyhow!("poisoned"))?.apply(event)?;
                        for e in out {
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
        Ok(navigation(&self.workspaces()?, &self.windows().unwrap_or_default()))
    }

    fn get_active_client(&self) -> Result<Option<ClientInfo>> {
        let ws = self.workspaces().unwrap_or_default();
        let reply = unwrap_reply(&self.request(json!("FocusedWindow"))?, "FocusedWindow")?;
        if reply.is_null() {
            return Ok(None);
        }
        let win: NiriWindow = serde_json::from_value(reply)?;
        Ok(Some(win.client(&ws)))
    }

    fn get_all_clients(&self) -> Result<Vec<ClientInfo>> {
        let ws = self.workspaces().unwrap_or_default();
        Ok(self.windows()?.iter().map(|w| w.client(&ws)).collect())
    }

    fn get_monitors(&self) -> Result<Vec<MonitorInfo>> {
        let outputs = unwrap_reply(&self.request(json!("Outputs"))?, "Outputs")?;
        let map = outputs.as_object().cloned().unwrap_or_default();
        Ok(map
            .into_iter()
            .map(|(name, o)| {
                let logical = o.get("logical").cloned().unwrap_or(Value::Null);
                let n = |k: &str| logical.get(k).and_then(Value::as_i64).unwrap_or(0) as i32;
                MonitorInfo {
                    name,
                    x: n("x"),
                    y: n("y"),
                    width: n("width"),
                    height: n("height"),
                    scale: logical.get("scale").and_then(Value::as_f64).unwrap_or(1.0) as f32,
                    active: true,
                }
            })
            .collect())
    }

    fn select_primary(&self, monitor: &str, id: u32) -> Result<()> {
        self.action(json!({ "FocusWorkspace": { "reference": workspace_reference(&self.workspaces().unwrap_or_default(), monitor, id) } }))
    }

    fn toggle_secondary(&self, monitor: &str, id: u32) -> Result<()> {
        self.select_primary(monitor, id)
    }

    fn move_client_to(&self, client_id: Option<u64>, id: u32) -> Result<()> {
        // Index `id` on the window's own output (the bar shows per-output
        // indices); the focused output when the window is unknown.
        let workspaces = self.workspaces().unwrap_or_default();
        let windows = self.windows().unwrap_or_default();
        let window = windows.iter().find(|w| Some(w.id) == client_id).or_else(|| windows.iter().find(|w| w.is_focused));
        let output = window.and_then(|w| workspaces.iter().find(|ws| Some(ws.id) == w.workspace_id)).and_then(|ws| ws.output.clone()).unwrap_or_default();
        self.action(json!({ "MoveWindowToWorkspace": { "window_id": client_id, "reference": workspace_reference(&workspaces, &output, id), "focus": false } }))
    }

    fn toggle_client_tag(&self, client_id: Option<u64>, tag: u32) -> Result<()> {
        self.move_client_to(client_id, tag)
    }

    fn cycle_layout(&self, _monitor: &str) -> Result<()> {
        self.action(json!({ "SwitchPresetColumnWidth": {} }))
    }

    fn dispatch_raw(&self, cmd: &str, args: &[&str]) -> Result<()> {
        // `cmd` is an action name; args may be a single JSON object.
        let payload: Value = match args.first() {
            Some(a) => serde_json::from_str(a).unwrap_or(Value::Object(Default::default())),
            None => Value::Object(Default::default()),
        };
        self.action(json!({ cmd: payload }))
    }

    fn reload_config(&self) -> Result<()> {
        // Niri watches its config file and reloads automatically.
        Ok(())
    }

    fn switch_keyboard_layout(&self) -> Result<()> {
        self.action(switch_layout_action())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_layout_is_the_next_layout_action() {
        // Niri's IPC: {"Action":{"SwitchLayout":{"layout":"Next"}}}.
        assert_eq!(json!({ "Action": switch_layout_action() }).to_string(), r#"{"Action":{"SwitchLayout":{"layout":"Next"}}}"#);
    }

    #[test]
    fn parses_niri_versions() {
        assert_eq!(parse_version("26.04 (8c11b2b)"), Some((26, 4)));
        assert_eq!(parse_version("25.11"), Some((25, 11)));
        assert_eq!(parse_version("unknown"), None);
    }

    const WORKSPACES: &str = r#"{"Ok":{"Workspaces":[
        {"id":5,"idx":2,"name":null,"output":"DP-1","is_active":false,"is_focused":false,"active_window_id":null},
        {"id":4,"idx":1,"name":"web","output":"DP-1","is_active":true,"is_focused":true,"active_window_id":11},
        {"id":9,"idx":1,"name":null,"output":"HDMI-A-1","is_active":true,"is_focused":false,"active_window_id":null}
    ]}}"#;

    #[test]
    fn groups_workspaces_per_output() {
        let reply: Value = serde_json::from_str(WORKSPACES).unwrap();
        let ws: Vec<NiriWorkspace> = serde_json::from_value(unwrap_reply(&reply, "Workspaces").unwrap()).unwrap();
        let nav = navigation(&ws, &[]);
        assert_eq!(nav.len(), 2);
        let (out, NavigationModel::Workspaces { workspaces }) = &nav[0] else { panic!() };
        assert_eq!(out, "DP-1");
        assert_eq!(workspaces[0].name, "web");
        assert!(workspaces[0].active && workspaces[0].occupied);
        assert!(!workspaces[1].occupied);
        assert_eq!(nav[0].1.active_mask(), 0b1);
    }

    #[test]
    fn surfaces_errors() {
        let err: Value = serde_json::from_str(r#"{"Err":"no such workspace"}"#).unwrap();
        assert!(unwrap_reply(&err, "Handled").unwrap_err().to_string().contains("no such workspace"));
    }

    /// Event-stream lines in niri 26.04's serde shape (niri-ipc `Event`).
    #[test]
    fn event_stream_keeps_windows_and_focus_current() {
        let win = |id: u64, ws: u64, title: &str, focused: bool| {
            format!(r#"{{"id":{id},"title":"{title}","app_id":"foot","pid":42,"workspace_id":{ws},"is_focused":{focused},"is_floating":false,"is_urgent":false,"layout":{{"pos_in_scrolling_layout":[1,1],"tile_size":[960.0,1040.0],"window_size":[956,1036],"tile_pos_in_workspace_view":null,"window_offset_in_tile":[2.0,2.0]}},"focus_timestamp":{{"secs":1,"nanos":0}}}}"#)
        };
        let mut st = NiriState::default();
        let ev = |s: &str| serde_json::from_str::<Value>(s).unwrap();
        let ws = r#"{"WorkspacesChanged":{"workspaces":[{"id":4,"idx":1,"name":null,"output":"HEADLESS-1","is_urgent":false,"is_active":true,"is_focused":true,"active_window_id":null},{"id":5,"idx":2,"name":null,"output":"HEADLESS-1","is_urgent":false,"is_active":false,"is_focused":false,"active_window_id":null}]}}"#;
        assert!(st.apply(ev(ws)).unwrap().iter().any(|e| matches!(e, CompositorEvent::Navigation { .. })));
        st.apply(ev(&format!(r#"{{"WindowsChanged":{{"windows":[{}]}}}}"#, win(11, 4, "one", true)))).unwrap();
        // A new focused window: the list grows, focus moves, title known.
        let out = st.apply(ev(&format!(r#"{{"WindowOpenedOrChanged":{{"window":{}}}}}"#, win(12, 5, "two", true)))).unwrap();
        assert!(out.iter().any(|e| matches!(e, CompositorEvent::Clients(c) if c.len() == 2)), "{out:?}");
        assert!(out.iter().any(|e| matches!(e, CompositorEvent::FocusedClient(Some(c)) if c.title == "two" && c.tags == vec![2])));
        assert_eq!(st.windows.iter().filter(|w| w.is_focused).count(), 1);
        // A title change of the focused window reaches the bar.
        let out = st.apply(ev(&format!(r#"{{"WindowOpenedOrChanged":{{"window":{}}}}}"#, win(12, 5, "two — edited", true)))).unwrap();
        assert!(out.iter().any(|e| matches!(e, CompositorEvent::FocusedClient(Some(c)) if c.title == "two — edited")));
        // Closing it drops it from the dock / switcher list and clears focus.
        let out = st.apply(ev(r#"{"WindowClosed":{"id":12}}"#)).unwrap();
        assert!(out.iter().any(|e| matches!(e, CompositorEvent::Clients(c) if c.len() == 1)));
        assert!(out.iter().any(|e| matches!(e, CompositorEvent::FocusedClient(None))));
        st.apply(ev(r#"{"WindowFocusChanged":{"id":11}}"#)).unwrap();
        st.apply(ev(r#"{"WorkspaceActivated":{"id":5,"focused":true}}"#)).unwrap();
        st.apply(ev(r#"{"WorkspaceUrgencyChanged":{"id":4,"urgent":true}}"#)).unwrap();
        let out = st.apply(ev(r#"{"WorkspaceActiveWindowChanged":{"workspace_id":5,"active_window_id":null}}"#)).unwrap();
        let Some(CompositorEvent::Navigation { model: NavigationModel::Workspaces { workspaces }, .. }) = out.iter().find(|e| matches!(e, CompositorEvent::Navigation { .. })) else { panic!("{out:?}") };
        assert!(workspaces[1].active && !workspaces[0].active && workspaces[0].urgent && workspaces[0].occupied);
        // Unrelated events change nothing.
        assert!(st.apply(ev(r#"{"KeyboardLayoutSwitched":{"idx":1}}"#)).unwrap().is_empty());
    }

    #[test]
    fn workspace_clicks_target_the_bars_output() {
        let ws: Vec<NiriWorkspace> = serde_json::from_value(unwrap_reply(&serde_json::from_str(WORKSPACES).unwrap(), "Workspaces").unwrap()).unwrap();
        // Index 1 exists on both outputs: the HDMI bar must reach HDMI's.
        assert_eq!(workspace_reference(&ws, "HDMI-A-1", 1), json!({ "Id": 9 }));
        assert_eq!(workspace_reference(&ws, "DP-1", 1), json!({ "Id": 4 }));
        assert_eq!(workspace_reference(&ws, "DP-1", 7), json!({ "Index": 7 }));
    }

    #[test]
    fn window_to_client() {
        let ws: Vec<NiriWorkspace> = serde_json::from_value(unwrap_reply(&serde_json::from_str(WORKSPACES).unwrap(), "Workspaces").unwrap()).unwrap();
        let win: NiriWindow = serde_json::from_str(r#"{"id":11,"title":"Docs","app_id":"firefox","workspace_id":4,"is_focused":true}"#).unwrap();
        let c = win.client(&ws);
        assert_eq!(c.monitor, "DP-1");
        assert_eq!(c.tags, vec![1]);
    }
}
