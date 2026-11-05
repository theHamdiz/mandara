//! Hyprland driver: `.socket.sock` for requests (`j/workspaces`,
//! `dispatch workspace 3`) and `.socket2.sock` for the event stream
//! (`workspace>>3`, `activewindow>>class,title`, …).

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

use crate::backend::CompositorBackend;
use crate::model::{ClientInfo, CompositorEvent, CompositorKind, MonitorInfo, NavigationModel, WorkspaceStatus};

/// Hyprland 0.57 leaves `id` out for named and special workspaces and
/// tags each with a `type`; older versions give special ones negative ids.
#[derive(Clone, Debug, Deserialize)]
pub struct HyprWorkspace {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub monitor: String,
    #[serde(default)]
    pub windows: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct HyprMonitor {
    #[serde(default)]
    pub id: Option<i64>,
    pub name: String,
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    #[serde(default)]
    pub width: i32,
    #[serde(default)]
    pub height: i32,
    #[serde(default = "one")]
    pub scale: f32,
    #[serde(default)]
    pub focused: bool,
    #[serde(rename = "activeWorkspace")]
    pub active_workspace: Option<HyprWsRef>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct HyprWsRef {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default, rename = "type")]
    pub kind: String,
}

/// A workspace the bar lists: numbered, not special (negative ids are
/// special workspaces before 0.57).
const SPECIAL_KIND: &str = "special";

fn numbered(id: Option<i64>, kind: &str) -> Option<u32> {
    id.filter(|&i| i > 0 && kind != SPECIAL_KIND).and_then(|i| u32::try_from(i).ok())
}

#[derive(Clone, Debug, Deserialize)]
pub struct HyprClient {
    pub address: String,
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub monitor: i64,
    pub workspace: Option<HyprWsRef>,
    #[serde(default)]
    pub floating: bool,
    #[serde(default)]
    pub fullscreen: serde_json::Value,
    #[serde(default)]
    pub at: Option<[i32; 2]>,
    #[serde(default)]
    pub size: Option<[i32; 2]>,
}

fn one() -> f32 {
    1.0
}

impl HyprClient {
    fn id(&self) -> u64 {
        u64::from_str_radix(self.address.trim_start_matches("0x"), 16).unwrap_or(0)
    }

    /// Resolve connector names when available; retain geometry if monitor metadata is missing.
    fn client(&self, focused: bool, monitors: &[HyprMonitor]) -> ClientInfo {
        ClientInfo {
            id: self.id(),
            title: self.title.clone(),
            app_id: self.class.clone(),
            monitor: monitors.iter().find(|m| m.id == Some(self.monitor)).map(|m| m.name.clone()).unwrap_or_default(),
            tags: self.workspace.as_ref().and_then(|w| numbered(w.id, &w.kind)).map(|id| vec![id]).unwrap_or_default(),
            focused,
            floating: self.floating,
            fullscreen: self.fullscreen.as_bool().unwrap_or(false) || self.fullscreen.as_i64().unwrap_or(0) > 0,
            minimized: false,
            geometry: match (self.at, self.size) {
                (Some([x, y]), Some([w, h])) if w > 0 && h > 0 => Some((x, y, w, h)),
                _ => None,
            },
        }
    }
}

/// Per-monitor workspace lists (special workspaces with negative ids hidden).
pub fn navigation(workspaces: &[HyprWorkspace], monitors: &[HyprMonitor]) -> Vec<(String, NavigationModel)> {
    monitors
        .iter()
        .map(|m| {
            let active = m.active_workspace.as_ref().and_then(|w| numbered(w.id, &w.kind));
            let mut list: Vec<WorkspaceStatus> = workspaces
                .iter()
                .filter(|w| w.monitor == m.name)
                .filter_map(|w| numbered(w.id, &w.kind).map(|id| (id, w)))
                .map(|(id, w)| WorkspaceStatus {
                    id,
                    name: if w.name.is_empty() { id.to_string() } else { w.name.clone() },
                    active: Some(id) == active,
                    occupied: w.windows > 0,
                    urgent: false,
                    layout: String::new(),
                })
                .collect();
            list.sort_by_key(|w| w.id);
            (m.name.clone(), NavigationModel::Workspaces { workspaces: list })
        })
        .collect()
}

/// Splits a socket2 line `event>>data`.
pub fn parse_event(line: &str) -> Option<(&str, &str)> {
    line.split_once(">>")
}

/// `j/version` reply (only the field Mandara needs).
#[derive(Clone, Debug, Deserialize)]
pub struct HyprVersion {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub tag: String,
}

impl HyprVersion {
    pub fn parsed(&self) -> Option<(u32, u32)> {
        mandara_config::compositor::parse_version(&self.version).or_else(|| mandara_config::compositor::parse_version(&self.tag))
    }
}

/// Hyprland 0.55 added the Lua config.
const LUA_CONFIG_SINCE: (u32, u32) = (0, 55);

/// Whether a running Hyprland takes Lua dispatches: always from 0.57
/// (hyprlang is gone), on 0.55–0.56 when it loaded `hyprland.lua`.
pub fn speaks_lua(version: Option<(u32, u32)>, lua_config_exists: bool) -> bool {
    match version {
        Some(v) if mandara_config::compositor::hyprland_reads_lua(v) => true,
        Some(v) => v >= LUA_CONFIG_SINCE && lua_config_exists,
        None => lua_config_exists,
    }
}

/// Lua string literal.
fn lua_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"))
}

/// A number stays a number, anything else becomes a Lua string.
fn lua_selector(s: &str) -> String {
    if s.parse::<i64>().is_ok() { s.to_string() } else { lua_str(s) }
}

/// Translates a hyprlang dispatch (`workspace 3`, `movetoworkspacesilent
/// 3,address:0x…`) into the Lua expression Hyprland 0.57's `dispatch`
/// request evaluates (`hl.dsp.focus({ workspace = 3 })`). Text that already
/// is Lua (`hl.dsp…(…)`) passes through.
pub fn lua_dispatch(cmd: &str, args: &str) -> Result<String> {
    let args = args.trim();
    if cmd.contains('(') || cmd.starts_with("hl.") {
        return Ok(format!("{cmd} {args}").trim().to_string());
    }
    Ok(match cmd {
        "workspace" => format!("hl.dsp.focus({{ workspace = {} }})", lua_selector(args)),
        "focuswindow" => format!("hl.dsp.focus({{ window = {} }})", lua_str(args)),
        "movefocus" => format!("hl.dsp.focus({{ direction = {} }})", lua_str(args)),
        "movetoworkspace" | "movetoworkspacesilent" => {
            let (ws, window) = args.split_once(',').map(|(w, c)| (w.trim(), Some(c.trim()))).unwrap_or((args, None));
            let mut fields = vec![format!("workspace = {}", lua_selector(ws))];
            if cmd == "movetoworkspacesilent" {
                fields.push("follow = false".into());
            }
            if let Some(w) = window {
                fields.push(format!("window = {}", lua_str(w)));
            }
            format!("hl.dsp.window.move({{ {} }})", fields.join(", "))
        }
        "togglesplit" | "swapsplit" => format!("hl.dsp.layout({})", lua_str(cmd)),
        "layoutmsg" => format!("hl.dsp.layout({})", lua_str(args)),
        "dpms" => format!("hl.dsp.dpms({{ action = {} }})", lua_str(if args.is_empty() { "toggle" } else { args })),
        "exec" => format!("hl.dsp.exec_cmd({})", lua_str(args)),
        "exit" => "hl.dsp.exit()".into(),
        // Plugin dispatchers live under `hl.plugin.<plugin>.<name>`.
        plugin if plugin.contains(':') => {
            let (ns, name) = plugin.split_once(':').unwrap_or_default();
            if args.is_empty() { format!("hl.plugin.{ns}.{name}()") } else { format!("hl.plugin.{ns}.{name}({})", lua_str(args)) }
        }
        other => bail!("Hyprland's Lua config takes Lua dispatches (e.g. `hl.dsp.window.float({{ action = \"toggle\" }})`); no translation for `{other}`"),
    })
}

/// A Lua `dispatch` reply: `ok` (or nothing) on success, `error: …` lines
/// otherwise.
fn lua_reply_ok(reply: &str) -> bool {
    let r = reply.trim();
    r.is_empty() || r == "ok" || r.starts_with("ok\n")
}

pub struct HyprlandDriver {
    dir: PathBuf,
    /// Running version (`j/version`).
    version: Option<(u32, u32)>,
    /// Dispatches are Lua expressions (0.57+, or a Lua config on 0.55+).
    lua: bool,
}

impl HyprlandDriver {
    pub fn from_env() -> Option<Self> {
        let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)?;
        let dir = runtime.join("hypr").join(sig);
        if !dir.join(".socket.sock").exists() {
            return None;
        }
        let mut driver = Self { dir, version: None, lua: false };
        driver.version = driver.json::<HyprVersion>("version").ok().and_then(|v| v.parsed());
        driver.lua = speaks_lua(driver.version, mandara_config::compositor::hyprland_lua_config_exists());
        Some(driver)
    }

    fn request(&self, cmd: &str) -> Result<String> {
        let path = self.dir.join(".socket.sock");
        let mut stream = UnixStream::connect(&path).with_context(|| format!("connecting {}", path.display()))?;
        stream.set_read_timeout(Some(crate::REQUEST_TIMEOUT))?;
        stream.write_all(cmd.as_bytes())?;
        let mut out = String::new();
        stream.read_to_string(&mut out)?;
        Ok(out)
    }

    /// Query only after socket2 is connected, so intervening changes are queued.
    fn initial_events(&self) -> Vec<CompositorEvent> {
        let mut events = self.navigation_events();
        if let Ok(focused) = self.get_active_client() { events.push(CompositorEvent::FocusedClient(focused)); }
        if let Ok(monitors) = self.get_monitors() { events.push(CompositorEvent::Monitors(monitors)); }
        events
    }

    fn navigation_events(&self) -> Vec<CompositorEvent> {
        self.get_navigation_state().unwrap_or_default().into_iter().map(|(monitor, model)| CompositorEvent::Navigation { monitor, model })
            .chain(self.get_all_clients().ok().map(CompositorEvent::Clients)).collect()
    }

    fn json<T: serde::de::DeserializeOwned>(&self, what: &str) -> Result<T> {
        Ok(serde_json::from_str(&self.request(&format!("j/{what}"))?)?)
    }

    /// Runs a dispatcher, given in hyprlang form (`workspace 3`); Lua
    /// Hyprlands get its [`lua_dispatch`] translation.
    fn dispatch(&self, line: &str) -> Result<()> {
        let (cmd, args) = line.split_once(' ').unwrap_or((line, ""));
        if self.lua {
            let expr = lua_dispatch(cmd, args)?;
            let reply = self.request(&format!("dispatch {expr}"))?;
            if !lua_reply_ok(&reply) {
                bail!("hyprland: {}", reply.trim());
            }
            return Ok(());
        }
        let reply = self.request(&format!("dispatch {line}"))?;
        if reply.trim() != "ok" {
            bail!("hyprland: {}", reply.trim());
        }
        Ok(())
    }
}

impl CompositorBackend for HyprlandDriver {
    fn kind(&self) -> CompositorKind {
        CompositorKind::Hyprland
    }

    fn version(&self) -> Option<(u32, u32)> {
        self.version
    }

    fn subscribe_events(&self) -> async_channel::Receiver<CompositorEvent> {
        let (tx, rx) = async_channel::bounded(crate::EVENT_QUEUE);
        let (dir, version, lua) = (self.dir.clone(), self.version, self.lua);
        let _ = std::thread::Builder::new().name("hypr-events".into()).spawn(move || {
            let driver = HyprlandDriver { dir: dir.clone(), version, lua };
            loop {
                let Ok(stream) = UnixStream::connect(dir.join(".socket2.sock")) else {
                    if tx.is_closed() {
                        return;
                    }
                    std::thread::sleep(crate::RECONNECT_DELAY);
                    continue;
                };
                for event in driver.initial_events() {
                    if tx.send_blocking(event).is_err() { return; }
                }
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let Some((event, _)) = parse_event(&line) else { continue };
                    let events: Vec<CompositorEvent> = match event {
                        "workspace" | "workspacev2" | "createworkspace" | "destroyworkspace" | "focusedmon" | "openwindow"
                        | "closewindow" | "movewindow" | "urgent" => driver.navigation_events(),
                        "activewindow" | "activewindowv2" | "windowtitle" | "windowtitlev2" => {
                            vec![CompositorEvent::FocusedClient(driver.get_active_client().ok().flatten())]
                        }
                        "monitoradded" | "monitorremoved" => vec![CompositorEvent::Monitors(driver.get_monitors().unwrap_or_default())],
                        _ => Vec::new(),
                    };
                    for e in events {
                        if tx.send_blocking(e).is_err() {
                            return;
                        }
                    }
                }
                if tx.is_closed() {
                    return;
                }
                std::thread::sleep(crate::RECONNECT_DELAY);
            }
        });
        rx
    }

    fn get_navigation_state(&self) -> Result<Vec<(String, NavigationModel)>> {
        Ok(navigation(&self.json::<Vec<HyprWorkspace>>("workspaces")?, &self.json::<Vec<HyprMonitor>>("monitors")?))
    }

    /// Read the focused window; an unavailable monitor lookup only leaves its connector unknown.
    fn get_active_client(&self) -> Result<Option<ClientInfo>> {
        let raw = self.request("j/activewindow")?;
        if raw.trim() == "{}" || raw.trim().is_empty() {
            return Ok(None);
        }
        let client = serde_json::from_str::<HyprClient>(&raw)?;
        let monitors = self.json::<Vec<HyprMonitor>>("monitors").unwrap_or_default();
        Ok(Some(client.client(true, &monitors)))
    }

    /// Keep the client snapshot even when the separate monitor request fails.
    fn get_all_clients(&self) -> Result<Vec<ClientInfo>> {
        let clients = self.json::<Vec<HyprClient>>("clients")?;
        let monitors = self.json::<Vec<HyprMonitor>>("monitors").unwrap_or_default();
        Ok(clients.iter().map(|c| c.client(false, &monitors)).collect())
    }

    fn get_monitors(&self) -> Result<Vec<MonitorInfo>> {
        Ok(self
            .json::<Vec<HyprMonitor>>("monitors")?
            .into_iter()
            .map(|m| MonitorInfo { name: m.name, x: m.x, y: m.y, width: m.width, height: m.height, scale: m.scale, active: m.focused })
            .collect())
    }

    fn select_primary(&self, _monitor: &str, id: u32) -> Result<()> {
        self.dispatch(&format!("workspace {id}"))
    }

    fn toggle_secondary(&self, monitor: &str, id: u32) -> Result<()> {
        self.select_primary(monitor, id)
    }

    fn move_client_to(&self, client_id: Option<u64>, id: u32) -> Result<()> {
        match client_id {
            Some(addr) => self.dispatch(&format!("movetoworkspacesilent {id},address:0x{addr:x}")),
            None => self.dispatch(&format!("movetoworkspacesilent {id}")),
        }
    }

    fn toggle_client_tag(&self, client_id: Option<u64>, tag: u32) -> Result<()> {
        self.move_client_to(client_id, tag)
    }

    fn cycle_layout(&self, _monitor: &str) -> Result<()> {
        self.dispatch("togglesplit")
    }

    fn dispatch_raw(&self, cmd: &str, args: &[&str]) -> Result<()> {
        self.dispatch(&format!("{cmd} {}", args.join(" ")))
    }

    fn reload_config(&self) -> Result<()> {
        self.request("reload").map(|_| ())
    }

    fn switch_keyboard_layout(&self) -> Result<()> {
        expect_ok(&self.request(SWITCH_LAYOUT_REQUEST)?)
    }

    fn apply_cursor(&self, theme: Option<&str>, size: Option<u32>) -> Result<()> {
        let fallback = std::env::var("XCURSOR_THEME").ok();
        match setcursor_request(theme, size, fallback.as_deref()) {
            Some(req) => expect_ok(&self.request(&req)?),
            None => Ok(()),
        }
    }
}

/// `hyprctl switchxkblayout all next`: every keyboard to its next layout.
/// A request, not a dispatcher.
pub const SWITCH_LAYOUT_REQUEST: &str = "switchxkblayout all next";
/// The cursor size Hyprland's `setcursor` gets when only a theme is set.
const DEFAULT_CURSOR_SIZE: u32 = 24;
/// The theme `setcursor` gets when only a size is set and `XCURSOR_THEME`
/// is unset (the XCursor spec's fallback name).
const DEFAULT_CURSOR_THEME: &str = "default";

/// `setcursor <theme> <size>` for the live cursor, or `None` when neither is
/// set (the user's own config keeps the cursor). `setcursor` needs both, so
/// a missing half falls back to the session's theme / the default size.
pub fn setcursor_request(theme: Option<&str>, size: Option<u32>, session_theme: Option<&str>) -> Option<String> {
    let theme = theme.map(str::trim).filter(|t| !t.is_empty());
    if theme.is_none() && size.is_none() {
        return None;
    }
    let theme = theme.or(session_theme.map(str::trim).filter(|t| !t.is_empty())).unwrap_or(DEFAULT_CURSOR_THEME);
    Some(format!("setcursor {theme} {}", size.unwrap_or(DEFAULT_CURSOR_SIZE)))
}

fn expect_ok(reply: &str) -> Result<()> {
    if reply.trim() != "ok" {
        bail!("hyprland: {}", reply.trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_cursor_and_layout_requests() {
        assert_eq!(setcursor_request(None, None, Some("Adwaita")), None, "nothing set: the user's cursor stays");
        assert_eq!(setcursor_request(Some("Bibata-Modern-Ice"), Some(32), None).as_deref(), Some("setcursor Bibata-Modern-Ice 32"));
        assert_eq!(setcursor_request(Some(" Bibata "), None, None).as_deref(), Some("setcursor Bibata 24"));
        assert_eq!(setcursor_request(None, Some(40), Some("Adwaita")).as_deref(), Some("setcursor Adwaita 40"));
        assert_eq!(setcursor_request(Some(""), Some(40), None).as_deref(), Some("setcursor default 40"));
        assert_eq!(SWITCH_LAYOUT_REQUEST, "switchxkblayout all next");
        assert!(expect_ok("ok\n").is_ok() && expect_ok("Invalid dispatcher").is_err());
    }

    #[test]
    fn workspaces_per_monitor_hide_special() {
        let ws: Vec<HyprWorkspace> = serde_json::from_str(
            r#"[{"id":1,"name":"1","monitor":"DP-1","windows":2},{"id":3,"name":"3","monitor":"DP-1","windows":0},{"id":-98,"name":"special:magic","monitor":"DP-1","windows":1}]"#,
        )
        .unwrap();
        let mons: Vec<HyprMonitor> = serde_json::from_str(r#"[{"name":"DP-1","width":2560,"height":1440,"scale":1.5,"focused":true,"activeWorkspace":{"id":3,"name":"3"}}]"#).unwrap();
        let nav = navigation(&ws, &mons);
        let NavigationModel::Workspaces { workspaces } = &nav[0].1 else { panic!() };
        assert_eq!(workspaces.len(), 2);
        assert!(workspaces[0].occupied && !workspaces[0].active);
        assert!(workspaces[1].active && !workspaces[1].occupied);
    }

    /// `j/…` replies recorded from Hyprland 0.57's formatters
    /// (src/ipc/s1/Commands.cpp at v0.57.0): `id` only on numbered
    /// workspaces, a `type` on every one, `stableId` a string.
    #[test]
    fn hyprland_057_replies() {
        let ws: Vec<HyprWorkspace> = serde_json::from_str(
            r#"[{"address":"1","id":1,"type":"regular","name":"1","monitor":"HEADLESS-1","monitorID":0,"windows":1,"hasfullscreen":false,"lastwindow":"0x55d0c0a1b2c0","lastwindowtitle":"foot","ispersistent":false,"tiledLayout":"dwindle"},
                {"address":"name:web","type":"regular","name":"web","monitor":"HEADLESS-1","monitorID":0,"windows":0,"hasfullscreen":false,"lastwindow":"0x0","lastwindowtitle":"","ispersistent":false,"tiledLayout":"dwindle"},
                {"address":"special:magic","type":"special","name":"special:magic","monitor":"HEADLESS-1","monitorID":0,"windows":1,"hasfullscreen":false,"lastwindow":"0x0","lastwindowtitle":"","ispersistent":false,"tiledLayout":"dwindle"}]"#,
        )
        .unwrap();
        let mons: Vec<HyprMonitor> = serde_json::from_str(
            r#"[{"id":0,"name":"HEADLESS-1","description":"Headless","make":"","model":"","serial":"","width":1920,"height":1080,"physicalWidth":0,"physicalHeight":0,"refreshRate":60.00000,"x":0,"y":0,
                "activeWorkspace":{"address":"1","id":1,"type":"regular","name":"1"},"specialWorkspace":{"address":"","type":"","name":""},"reserved":[0,32,0,0],"scale":1.25,"transform":0,"focused":true,"dpmsStatus":true,"vrr":false}]"#,
        )
        .unwrap();
        let nav = navigation(&ws, &mons);
        let NavigationModel::Workspaces { workspaces } = &nav[0].1 else { panic!() };
        assert_eq!(workspaces.len(), 1, "named and special workspaces have no number: {workspaces:?}");
        assert!(workspaces[0].active && workspaces[0].occupied);
        let c: HyprClient = serde_json::from_str(
            r#"{"address":"0x55d0c0a1b2c0","mapped":true,"hidden":false,"visible":true,"acceptsInput":true,"at":[10,42],"size":[900,600],"workspace":{"address":"special:magic","type":"special","name":"special:magic"},"floating":true,"monitor":0,"class":"foot","title":"htop","initialClass":"foot","initialTitle":"foot","pid":42,"xwayland":false,"pinned":false,"pinFullscreened":false,"fullscreen":0,"fullscreenClient":0,"fullscreenHandler":"","allowedOverFullscreen":false,"grouped":[],"tags":[],"swallowing":"0x0","focusHistoryID":0,"inhibitingIdle":false,"xdgTag":"","xdgDescription":"","contentType":"none","tearingHint":false,"stableId":"1a"}"#,
        )
        .unwrap();
        let info = c.client(true, &[]);
        assert!(info.tags.is_empty() && info.floating && info.geometry == Some((10, 42, 900, 600)));
        let v: HyprVersion = serde_json::from_str(r#"{"branch":"","commit":"0aa6745","version":"0.57.0","dirty":false,"commit_message":"","commit_date":"","tag":"v0.57.0","commits":"0"}"#).unwrap();
        assert_eq!(v.parsed(), Some((0, 57)));
    }

    #[test]
    fn lua_dispatches_for_hyprland_057() {
        assert!(speaks_lua(Some((0, 57)), false));
        assert!(speaks_lua(Some((0, 55)), true) && !speaks_lua(Some((0, 55)), false) && !speaks_lua(Some((0, 54)), true));
        assert!(speaks_lua(None, true) && !speaks_lua(None, false));
        let lua = |c: &str, a: &str| lua_dispatch(c, a).unwrap();
        assert_eq!(lua("workspace", "3"), "hl.dsp.focus({ workspace = 3 })");
        assert_eq!(lua("focuswindow", "address:0x55d0c0a1b2c0"), "hl.dsp.focus({ window = \"address:0x55d0c0a1b2c0\" })");
        assert_eq!(lua("movefocus", "l"), "hl.dsp.focus({ direction = \"l\" })");
        assert_eq!(lua("workspace", "name:web"), "hl.dsp.focus({ workspace = \"name:web\" })");
        assert_eq!(lua("movetoworkspacesilent", "4,address:0x55d0c0a1b2c0"), "hl.dsp.window.move({ workspace = 4, follow = false, window = \"address:0x55d0c0a1b2c0\" })");
        assert_eq!(lua("movetoworkspacesilent", "4"), "hl.dsp.window.move({ workspace = 4, follow = false })");
        assert_eq!(lua("togglesplit", ""), "hl.dsp.layout(\"togglesplit\")");
        assert_eq!(lua("dpms", "off"), "hl.dsp.dpms({ action = \"off\" })");
        assert_eq!(lua("hyprexpo:expo", "toggle"), "hl.plugin.hyprexpo.expo(\"toggle\")");
        assert_eq!(lua("hl.dsp.window.float({ action = \"toggle\" })", ""), "hl.dsp.window.float({ action = \"toggle\" })");
        assert!(lua_dispatch("togglefloating", "").is_err());
        assert!(lua_reply_ok("ok") && lua_reply_ok("") && !lua_reply_ok("error: hl.focus: bad workspace"));
    }

    #[test]
    fn client_monitor_ids_resolve_to_connector_names() {
        let monitors: Vec<HyprMonitor> = serde_json::from_str(
            r#"[{"id":7,"name":"HDMI-A-1"},{"id":3,"name":"DP-3","x":-2560,"width":2560,"height":1440,"focused":true}]"#,
        ).unwrap();
        let client: HyprClient = serde_json::from_str(
            r#"{"address":"0x7","monitor":3,"workspace":{"id":2},"at":[-2500,40],"size":[900,600]}"#,
        ).unwrap();
        let info = client.client(true, &monitors);
        assert_eq!(
            info.monitor, "DP-3",
            "monitor IDs are not positions in the monitor list"
        );
        assert_eq!(info.geometry, Some((-2500, 40, 900, 600)));
        assert!(
            client.client(false, &[]).monitor.is_empty(),
            "a disconnected output must not look like a connector name"
        );
    }

    #[test]
    fn events_and_clients() {
        assert_eq!(parse_event("activewindow>>kitty,~"), Some(("activewindow", "kitty,~")));
        assert_eq!(parse_event("garbage"), None);
        let c: HyprClient = serde_json::from_str(r#"{"address":"0x55d0c0a1b2c0","class":"kitty","title":"zsh","monitor":0,"workspace":{"id":2,"name":"2"},"floating":false,"fullscreen":0}"#).unwrap();
        let info = c.client(true, &[]);
        assert_eq!(info.id, 0x55d0c0a1b2c0);
        assert_eq!(info.tags, vec![2]);
        assert!(!info.fullscreen);
    }

    /// A monitor lookup failure must not erase valid clients or hide a malformed client reply.
    #[test]
    fn client_queries_survive_monitor_errors_but_keep_their_own_errors() {
        use std::os::unix::net::UnixListener;
        use std::time::{Duration, Instant};
        /// Bound fixture waits without spinning while the driver connects.
        const POLL: Duration = Duration::from_millis(10);
        struct Fixture(PathBuf);
        impl Drop for Fixture { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
        let dir = Fixture(std::env::temp_dir().join(format!("mdr-hypr-clients-{}", std::process::id())));
        std::fs::create_dir_all(&dir.0).unwrap();
        let requests = UnixListener::bind(dir.0.join(".socket.sock")).unwrap();
        requests.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let client = r#"{"address":"0x7","class":"foot","title":"Available window","monitor":3,"workspace":{"id":2}}"#;
            for (command, reply) in [
                ("j/activewindow", client.to_string()), ("j/monitors", "monitor lookup unavailable".into()),
                ("j/clients", format!("[{client}]")), ("j/monitors", String::new()),
                ("j/activewindow", "invalid client reply".into()), ("j/clients", "invalid clients reply".into()),
            ] {
                let deadline = Instant::now() + crate::REQUEST_TIMEOUT;
                let mut stream = loop {
                    match requests.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => std::thread::sleep(POLL),
                        Err(e) => panic!("client fixture socket: {e}"),
                    }
                };
                stream.set_read_timeout(Some(crate::REQUEST_TIMEOUT)).unwrap();
                let mut input = vec![0; command.len()];
                stream.read_exact(&mut input).unwrap();
                assert_eq!(input, command.as_bytes());
                stream.write_all(reply.as_bytes()).unwrap();
            }
        });
        let driver = HyprlandDriver { dir: dir.0.clone(), version: Some((0, 57)), lua: true };
        let focused = driver.get_active_client();
        let clients = driver.get_all_clients();
        let focused_error = driver.get_active_client();
        let clients_error = driver.get_all_clients();
        server.join().unwrap();
        let focused = focused.unwrap().unwrap();
        assert_eq!(focused.id, 7);
        assert_eq!(focused.title, "Available window");
        assert!(focused.focused && focused.monitor.is_empty());
        let clients = clients.unwrap();
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].id, 7);
        assert!(clients[0].monitor.is_empty());
        assert!(focused_error.is_err() && clients_error.is_err(), "client-query failures must still propagate");
    }

    /// Subscribers receive the current desktop before waiting for subsequent socket events.
    #[test]
    fn subscription_emits_a_fresh_snapshot_without_waiting_for_an_event() {
        use std::os::unix::net::UnixListener;
        use std::time::{Duration, Instant};
        /// Socket fixtures must finish even when a regression omits a request.
        const POLL: Duration = Duration::from_millis(10);
        struct Fixture(PathBuf);
        impl Drop for Fixture { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
        let dir = Fixture(std::env::temp_dir().join(format!("mdr-hypr-snapshot-{}", std::process::id())));
        std::fs::create_dir_all(&dir.0).unwrap();
        let events = UnixListener::bind(dir.0.join(".socket2.sock")).unwrap();
        let requests = UnixListener::bind(dir.0.join(".socket.sock")).unwrap();
        for listener in [&events, &requests] { listener.set_nonblocking(true).unwrap(); }
        let accept = |listener: &UnixListener| {
            let deadline = Instant::now() + crate::REQUEST_TIMEOUT;
            loop {
                match listener.accept() {
                    Ok((stream, _)) => return stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => std::thread::sleep(POLL),
                    Err(e) => panic!("snapshot socket: {e}"),
                }
            }
        };
        let server = std::thread::spawn(move || {
            let _stream = accept(&events);
            let monitors = r#"[{"name":"DP-1","activeWorkspace":{"id":2},"width":1707,"height":960}]"#;
            let focused = r#"{"address":"0x7","title":"Fresh window","workspace":{"id":2}}"#;
            for (command, reply) in [("j/workspaces", r#"[{"id":2,"name":"2","monitor":"DP-1","windows":1}]"#.to_string()), ("j/monitors", monitors.into()), ("j/clients", format!("[{focused}]")), ("j/monitors", monitors.into()), ("j/activewindow", focused.into()), ("j/monitors", monitors.into()), ("j/monitors", monitors.into())] {
                let mut stream = accept(&requests);
                stream.set_read_timeout(Some(crate::REQUEST_TIMEOUT)).unwrap();
                let mut input = vec![0; command.len()];
                stream.read_exact(&mut input).unwrap();
                assert_eq!(input, command.as_bytes());
                stream.write_all(reply.as_bytes()).unwrap();
            }
        });
        let driver = HyprlandDriver { dir: dir.0.clone(), version: Some((0, 57)), lua: true };
        let rx = driver.subscribe_events();
        let deadline = Instant::now() + crate::REQUEST_TIMEOUT;
        let mut initial = Vec::new();
        while initial.len() < 4 && Instant::now() < deadline {
            match rx.try_recv() { Ok(event) => initial.push(event), Err(_) => std::thread::sleep(POLL) }
        }
        drop(rx);
        server.join().unwrap();
        assert_eq!(initial.len(), 4);
        assert!(matches!(&initial[0], CompositorEvent::Navigation { monitor, model: NavigationModel::Workspaces { workspaces } } if monitor == "DP-1" && workspaces[0].id == 2));
        assert!(matches!(&initial[1], CompositorEvent::Clients(clients) if clients[0].id == 7));
        assert!(matches!(&initial[2], CompositorEvent::FocusedClient(Some(client)) if client.title == "Fresh window"));
        assert!(matches!(&initial[3], CompositorEvent::Monitors(monitors) if monitors[0].width == 1707));
    }

}
