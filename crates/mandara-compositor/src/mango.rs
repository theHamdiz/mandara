//! Native MangoWM driver.
//!
//! Talks to the Unix socket named by `$MANGO_INSTANCE_SIGNATURE` with
//! newline-terminated text commands (`get all-tags`, `watch focusing-client`,
//! `dispatch view,3`), receiving JSON lines back. Every `watch` stream is
//! multiplexed on one thread with `poll(2)` (line buffers reused, typed
//! parsing without `serde_json::Value` trees) and feeds a shared event
//! channel; streams reconnect individually with backoff.

use std::io::{BufRead, BufReader, Read as _, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

use crate::model::{ClientInfo, CompositorEvent, MonitorInfo, NavigationModel, TagStatus};

/// Raw JSON shapes from mango.
#[derive(Deserialize)]
struct RawTag {
    index: u32,
    is_active: bool,
    #[serde(default)]
    is_urgent: bool,
    #[serde(default)]
    layout: String,
    #[serde(default)]
    client_count: u32,
}

#[derive(Deserialize)]
struct RawMonitorTags {
    monitor: String,
    tags: Vec<RawTag>,
}

#[derive(Deserialize)]
struct RawAllTags {
    all_tags: Vec<RawMonitorTags>,
}

/// A client object; `id` is absent when `get focusing-client` has nothing
/// focused.
#[derive(Deserialize)]
struct RawClient {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    appid: String,
    #[serde(default)]
    monitor: String,
    #[serde(default)]
    tags: Vec<u32>,
    #[serde(default)]
    is_focused: bool,
    #[serde(default)]
    is_floating: bool,
    #[serde(default)]
    is_fullscreen: bool,
    #[serde(default)]
    x: i32,
    #[serde(default)]
    y: i32,
    #[serde(default)]
    width: i32,
    #[serde(default)]
    height: i32,
}

impl RawClient {
    fn into_client(self) -> Option<ClientInfo> {
        let c = self;
        Some(ClientInfo {
            id: c.id?,
            title: c.title,
            app_id: c.appid,
            monitor: c.monitor,
            tags: c.tags,
            focused: c.is_focused,
            floating: c.is_floating,
            fullscreen: c.is_fullscreen,
            geometry: (c.width > 0 && c.height > 0).then_some((c.x, c.y, c.width, c.height)),
        })
    }
}

#[derive(Deserialize)]
struct RawMonitor {
    name: String,
    #[serde(default)]
    active: bool,
    #[serde(default)]
    x: i32,
    #[serde(default)]
    y: i32,
    #[serde(default)]
    width: i32,
    #[serde(default)]
    height: i32,
    #[serde(default = "one")]
    scale: f32,
}

fn one() -> f32 {
    1.0
}

/// Reconnect backoff of a watch stream: first delay and ceiling.
const BACKOFF_START: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(5);
/// Bytes read from a stream per `read(2)`.
const READ_CHUNK: usize = 16 * 1024;
/// Topics the watcher follows.
const TOPICS: [&str; 6] = ["all-tags", "focusing-client", "all-clients", "all-monitors", "keymode", "keyboardlayout"];

/// `{"error": …}` replies (other fields are skipped, not materialized).
#[derive(Deserialize)]
struct ErrorReply {
    #[serde(default)]
    error: Option<serde_json::Value>,
}

/// `{"keymode": "…"}`.
#[derive(Deserialize)]
struct RawKeymode {
    #[serde(default)]
    keymode: String,
}

/// `{"layout": "…"}`.
#[derive(Deserialize)]
struct RawLayout {
    #[serde(default)]
    layout: String,
}

pub struct MangoDriver {
    socket: PathBuf,
}

/// Parses `get all-tags` / `watch all-tags` output.
pub fn parse_all_tags(json: &str, keymode: &str) -> Result<Vec<(String, NavigationModel)>> {
    let raw: RawAllTags = serde_json::from_str(json).context("parsing all-tags")?;
    Ok(raw
        .all_tags
        .into_iter()
        .map(|m| {
            let active_layout = m.tags.iter().find(|t| t.is_active).map(|t| t.layout.clone()).unwrap_or_default();
            let tags = m
                .tags
                .into_iter()
                .map(|t| TagStatus {
                    id: t.index,
                    active: t.is_active,
                    occupied: t.client_count > 0,
                    urgent: t.is_urgent,
                    client_count: t.client_count,
                    layout: t.layout,
                })
                .collect();
            (m.monitor, NavigationModel::BitmaskTags { tags, active_layout, keymode: keymode.to_string() })
        })
        .collect())
}

/// Parses a single client object (`get focusing-client`). Mango replies with
/// an object lacking `id` (or `{}`) when nothing is focused.
pub fn parse_client(json: &str) -> Option<ClientInfo> {
    serde_json::from_str::<RawClient>(json).ok()?.into_client()
}

/// Parses `get all-clients` / `watch all-clients` output.
pub fn parse_clients(json: &str) -> Result<Vec<ClientInfo>> {
    #[derive(Deserialize)]
    struct Raw {
        clients: Vec<RawClient>,
    }
    let raw: Raw = serde_json::from_str(json).context("parsing clients")?;
    Ok(raw.clients.into_iter().filter_map(RawClient::into_client).collect())
}

pub fn parse_monitors(json: &str) -> Result<Vec<MonitorInfo>> {
    #[derive(Deserialize)]
    struct Raw {
        monitors: Vec<RawMonitor>,
    }
    let raw: Raw = serde_json::from_str(json).context("parsing monitors")?;
    Ok(raw
        .monitors
        .into_iter()
        .map(|m| MonitorInfo { name: m.name, x: m.x, y: m.y, width: m.width, height: m.height, scale: m.scale, active: m.active })
        .collect())
}

impl MangoDriver {
    /// Connects using `$MANGO_INSTANCE_SIGNATURE`.
    pub fn from_env() -> Option<Self> {
        let socket = PathBuf::from(std::env::var_os("MANGO_INSTANCE_SIGNATURE")?);
        socket.exists().then_some(Self { socket })
    }

    pub fn with_socket(socket: impl Into<PathBuf>) -> Self {
        Self { socket: socket.into() }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Sends a one-shot request and returns the first JSON line.
    pub fn request(&self, command: &str) -> Result<String> {
        let mut stream = UnixStream::connect(&self.socket).with_context(|| format!("connecting {}", self.socket.display()))?;
        stream.set_read_timeout(Some(crate::REQUEST_TIMEOUT))?;
        writeln!(stream, "{command}")?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        if line.trim_start().starts_with('{')
            && let Ok(ErrorReply { error: Some(err) }) = serde_json::from_str::<ErrorReply>(&line)
        {
            bail!("mango: {err}");
        }
        Ok(line)
    }

    pub fn navigation(&self) -> Result<Vec<(String, NavigationModel)>> {
        let keymode = self.keymode().unwrap_or_default();
        parse_all_tags(&self.request("get all-tags")?, &keymode)
    }

    pub fn focused_client(&self) -> Result<Option<ClientInfo>> {
        Ok(parse_client(&self.request("get focusing-client")?))
    }

    pub fn monitors(&self) -> Result<Vec<MonitorInfo>> {
        parse_monitors(&self.request("get all-monitors")?)
    }

    pub fn keymode(&self) -> Result<String> {
        Ok(serde_json::from_str::<RawKeymode>(&self.request("get keymode")?)?.keymode)
    }

    /// `dispatch <func>,<args...>`.
    pub fn dispatch(&self, func: &str, args: &[&str]) -> Result<()> {
        let mut cmd = format!("dispatch {func}");
        for a in args {
            cmd.push(',');
            cmd.push_str(a);
        }
        self.request(&cmd).map(|_| ())
    }

    /// Left-click on a tag pill: show only this tag.
    pub fn view(&self, tag: u32) -> Result<()> {
        self.dispatch("view", &[&tag.to_string()])
    }

    /// Right-click: toggle this tag into the composite view.
    pub fn toggle_view(&self, tag: u32) -> Result<()> {
        self.dispatch("toggleview", &[&tag.to_string()])
    }

    /// Middle-click: toggle this tag on the focused client.
    pub fn toggle_tag(&self, tag: u32) -> Result<()> {
        self.dispatch("toggletag", &[&tag.to_string()])
    }

    pub fn cycle_layout(&self) -> Result<()> {
        self.dispatch("switch_layout", &[])
    }

    /// Starts the persistent watch streams (one thread, `poll(2)` over
    /// every socket); every update is sent on the returned channel. Streams
    /// reconnect with backoff if mango restarts.
    pub fn subscribe(&self) -> async_channel::Receiver<CompositorEvent> {
        let (tx, rx) = async_channel::bounded(crate::EVENT_QUEUE);
        let socket = self.socket.clone();
        let _ = std::thread::Builder::new().name("mango-watch".into()).spawn(move || watch_all(&socket, &tx));
        rx
    }
}

/// One watch stream's connection and line buffer.
struct Watch {
    topic: &'static str,
    stream: Option<UnixStream>,
    buf: Vec<u8>,
    retry_at: Instant,
    backoff: Duration,
}

impl Watch {
    fn connect(&mut self, socket: &Path) {
        let opened = UnixStream::connect(socket).and_then(|mut s| {
            writeln!(s, "watch {}", self.topic)?;
            s.set_nonblocking(true)?;
            Ok(s)
        });
        match opened {
            Ok(s) => {
                self.stream = Some(s);
                self.buf.clear();
            }
            Err(err) => self.fail(&anyhow::Error::from(err)),
        }
    }

    fn fail(&mut self, err: &anyhow::Error) {
        tracing::debug!("mango watch {} ended: {err:#}", self.topic);
        self.stream = None;
        self.retry_at = Instant::now() + self.backoff;
        self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
    }
}

/// Converts one JSON line of `topic` into events. `keymode` is shared by
/// every stream (tag models carry the current keymode).
fn line_events(topic: &str, line: &str, keymode: &mut String, out: &mut Vec<CompositorEvent>) -> Result<()> {
    match topic {
        "all-tags" => out.extend(parse_all_tags(line, keymode)?.into_iter().map(|(monitor, model)| CompositorEvent::Navigation { monitor, model })),
        "focusing-client" => out.push(CompositorEvent::FocusedClient(parse_client(line))),
        "all-clients" => out.push(CompositorEvent::Clients(parse_clients(line)?)),
        "all-monitors" => out.push(CompositorEvent::Monitors(parse_monitors(line)?)),
        "keymode" => {
            *keymode = serde_json::from_str::<RawKeymode>(line)?.keymode;
            out.push(CompositorEvent::Keymode(keymode.clone()));
        }
        "keyboardlayout" => out.push(CompositorEvent::KeyboardLayout(parse_keyboard_layout(line)?)),
        _ => {}
    }
    Ok(())
}

/// Reads what is available on `w`, emitting an event per complete line.
/// `Ok(false)` means the receiver is gone.
fn pump(w: &mut Watch, chunk: &mut [u8], keymode: &mut String, events: &mut Vec<CompositorEvent>, tx: &async_channel::Sender<CompositorEvent>) -> Result<bool> {
    let Some(stream) = w.stream.as_mut() else { return Ok(true) };
    loop {
        match stream.read(chunk) {
            Ok(0) => bail!("stream closed"),
            Ok(n) => w.buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let mut start = 0;
    while let Some(nl) = w.buf[start..].iter().position(|b| *b == b'\n') {
        let line = std::str::from_utf8(&w.buf[start..start + nl])?.trim();
        start += nl + 1;
        if line.is_empty() {
            continue;
        }
        events.clear();
        line_events(w.topic, line, keymode, events)?;
        for event in events.drain(..) {
            if tx.send_blocking(event).is_err() {
                return Ok(false);
            }
        }
        // A good line resets the reconnect backoff.
        w.backoff = BACKOFF_START;
    }
    w.buf.drain(..start);
    Ok(true)
}

/// Follows every topic until the receiver is dropped.
fn watch_all(socket: &Path, tx: &async_channel::Sender<CompositorEvent>) {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    let now = Instant::now();
    let mut watches: Vec<Watch> = TOPICS.iter().map(|topic| Watch { topic, stream: None, buf: Vec::new(), retry_at: now, backoff: BACKOFF_START }).collect();
    let mut chunk = vec![0u8; READ_CHUNK];
    let mut keymode = String::new();
    let mut events = Vec::new();
    loop {
        if tx.is_closed() {
            return;
        }
        let now = Instant::now();
        for w in watches.iter_mut().filter(|w| w.stream.is_none() && w.retry_at <= now) {
            w.connect(socket);
        }
        // Sleep until a stream has data, or the next reconnect is due.
        let next_retry = watches.iter().filter(|w| w.stream.is_none()).map(|w| w.retry_at).min();
        let timeout = next_retry.map(|at| {
            let d = at.saturating_duration_since(Instant::now());
            Timespec { tv_sec: d.as_secs() as i64, tv_nsec: i64::from(d.subsec_nanos()) }
        });
        let live: Vec<usize> = (0..watches.len()).filter(|i| watches[*i].stream.is_some()).collect();
        let ready: Vec<bool> = {
            let mut fds: Vec<PollFd<'_>> = live.iter().filter_map(|i| watches[*i].stream.as_ref()).map(|s| PollFd::new(s, PollFlags::IN)).collect();
            match poll(&mut fds, timeout.as_ref()) {
                Ok(_) => fds.iter().map(|f| !f.revents().is_empty()).collect(),
                Err(rustix::io::Errno::INTR) => continue,
                Err(err) => {
                    tracing::warn!("mango watch: poll failed: {err}");
                    std::thread::sleep(BACKOFF_MAX);
                    continue;
                }
            }
        };
        for (i, is_ready) in live.into_iter().zip(ready) {
            if !is_ready {
                continue;
            }
            match pump(&mut watches[i], &mut chunk, &mut keymode, &mut events, tx) {
                Ok(true) => {}
                Ok(false) => return,
                Err(err) => watches[i].fail(&err),
            }
        }
    }
}

impl crate::backend::CompositorBackend for MangoDriver {
    fn kind(&self) -> crate::model::CompositorKind {
        crate::model::CompositorKind::MangoWM
    }

    fn subscribe_events(&self) -> async_channel::Receiver<CompositorEvent> {
        self.subscribe()
    }

    fn get_navigation_state(&self) -> Result<Vec<(String, NavigationModel)>> {
        self.navigation()
    }

    fn get_active_client(&self) -> Result<Option<ClientInfo>> {
        self.focused_client()
    }

    fn get_all_clients(&self) -> Result<Vec<ClientInfo>> {
        parse_clients(&self.request("get all-clients")?)
    }

    fn get_monitors(&self) -> Result<Vec<MonitorInfo>> {
        self.monitors()
    }

    fn select_primary(&self, _monitor: &str, id: u32) -> Result<()> {
        self.view(id)
    }

    fn toggle_secondary(&self, _monitor: &str, id: u32) -> Result<()> {
        self.toggle_view(id)
    }

    fn move_client_to(&self, client_id: Option<u64>, id: u32) -> Result<()> {
        let tag = id.to_string();
        match client_id {
            Some(c) => self.request(&format!("dispatch tag,{tag} client,{c}")).map(|_| ()),
            None => self.dispatch("tag", &[&tag]),
        }
    }

    fn toggle_client_tag(&self, client_id: Option<u64>, tag: u32) -> Result<()> {
        let t = tag.to_string();
        match client_id {
            Some(c) => self.request(&format!("dispatch toggletag,{t} client,{c}")).map(|_| ()),
            None => self.toggle_tag(tag),
        }
    }

    fn cycle_layout(&self, _monitor: &str) -> Result<()> {
        MangoDriver::cycle_layout(self)
    }

    fn dispatch_raw(&self, cmd: &str, args: &[&str]) -> Result<()> {
        self.dispatch(cmd, args)
    }

    fn reload_config(&self) -> Result<()> {
        self.dispatch("reload_config", &[])
    }
}

/// Parses `{"layout":"English (US)"}` (`get` / `watch keyboardlayout`).
pub fn parse_keyboard_layout(json: &str) -> Result<String> {
    Ok(serde_json::from_str::<RawLayout>(json)?.layout)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAGS: &str = r#"{"all_tags":[{"monitor":"HDMI-A-2","tags":[{"index":1,"is_active":false,"is_urgent":false,"layout":"T","client_count":1},{"index":2,"is_active":true,"is_urgent":false,"layout":"S","client_count":0},{"index":3,"is_active":true,"is_urgent":true,"layout":"S","client_count":2}]}]}"#;

    #[test]
    fn parses_keyboard_layout() {
        assert_eq!(parse_keyboard_layout(r#"{"layout":"English (US)"}"#).unwrap(), "English (US)");
    }

    #[test]
    fn parses_tags_into_bitmask_model() {
        let nav = parse_all_tags(TAGS, "default").unwrap();
        assert_eq!(nav.len(), 1);
        let (monitor, model) = &nav[0];
        assert_eq!(monitor, "HDMI-A-2");
        assert_eq!(model.active_mask(), 0b110);
        let NavigationModel::BitmaskTags { tags, active_layout, keymode } = model else { panic!() };
        assert_eq!(active_layout, "S");
        assert_eq!(keymode, "default");
        assert!(tags[0].occupied && !tags[1].occupied && tags[2].urgent);
    }

    #[test]
    fn parses_focused_client_or_none() {
        let c = parse_client(r#"{"id":24,"title":"Claude","appid":"com.anthropic.Claude","monitor":"HDMI-A-2","tags":[3],"is_focused":true}"#).unwrap();
        assert_eq!(c.title, "Claude");
        assert_eq!(c.app_id, "com.anthropic.Claude");
        assert!(c.focused);
        assert!(parse_client("{}").is_none());
        assert!(parse_client("not json").is_none());
    }

    #[test]
    fn parses_monitors() {
        let m = parse_monitors(r#"{"monitors":[{"name":"HDMI-A-2","active":true,"x":0,"y":0,"width":2560,"height":1440,"scale":1.5}]}"#).unwrap();
        assert_eq!(m[0].width, 2560);
        assert_eq!(m[0].scale, 1.5);
    }

    #[test]
    fn one_thread_follows_every_topic_and_shares_the_keymode() {
        use std::os::unix::net::UnixListener;
        let dir = std::env::temp_dir().join(format!("mandara-mango-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let sock = dir.join("mango.sock");
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).unwrap();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut conn = conn;
                    let mut line = String::new();
                    BufReader::new(conn.try_clone().unwrap()).read_line(&mut line).unwrap();
                    let reply = match line.trim() {
                        "watch keymode" => "{\"keymode\":\"insert\"}\n".to_string(),
                        // Sent later, so the shared keymode is already known.
                        "watch all-tags" => {
                            std::thread::sleep(Duration::from_millis(200));
                            format!("{TAGS}\n")
                        }
                        _ => return,
                    };
                    let _ = conn.write_all(reply.as_bytes());
                    std::thread::sleep(Duration::from_secs(2));
                });
            }
        });
        let rx = MangoDriver::with_socket(&sock).subscribe();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut nav = None;
        while Instant::now() < deadline && nav.is_none() {
            if let Ok(CompositorEvent::Navigation { model, .. }) = rx.recv_blocking() {
                nav = Some(model);
            }
        }
        let Some(NavigationModel::BitmaskTags { keymode, .. }) = nav else { panic!("no navigation event") };
        assert_eq!(keymode, "insert");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_socket_if_available() {
        // Exercised only inside a running MangoWM session.
        if let Some(driver) = MangoDriver::from_env() {
            let nav = driver.navigation().unwrap();
            assert!(!nav.is_empty());
            assert!(!driver.monitors().unwrap().is_empty());
        }
    }
}
