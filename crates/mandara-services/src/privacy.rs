//! Privacy monitor: which apps are using the microphone, the camera or
//! sharing the screen right now. Each watcher runs only while its
//! indicator is enabled and costs nothing while nothing changes:
//!
//! - **Microphone**: PulseAudio-protocol source outputs (PipeWire's
//!   pipewire-pulse speaks it), re-read on `SOURCE_OUTPUT` events. Monitor
//!   sources (visualizers), peak meters, corked streams and Mandara itself
//!   don't count.
//! - **Camera**: inotify open / close events on `/dev/video*` (device nodes
//!   do report them) trigger a scan of `/proc/*/fd` for processes holding a
//!   video device. Without inotify the scan runs every
//!   `privacy.camera_poll_s` instead.
//! - **Screen sharing**: a D-Bus *monitor* connection watches the desktop
//!   portal's `ScreenCast` / `RemoteDesktop` `Start` calls, their `Response`
//!   and the sessions' `Close` / `Closed` (best effort: tools that use
//!   wlr-screencopy directly bypass the portal).

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mandara_config::alerts::PrivacyConfig;
use parking_lot::Mutex;
use pulseaudio::protocol::{self, Command};
use regex::{Regex, RegexBuilder};
use serde::Serialize;

use crate::audio::Pulse;

/// Watcher threads check their stop flag at least this often.
const ALIVE_CHECK: Duration = Duration::from_secs(5);
/// Bursts of events (an app opening a device several times) settle first.
const SETTLE: Duration = Duration::from_millis(150);
/// Back-off before reconnecting to a vanished sound server / bus.
const RECONNECT: Duration = Duration::from_secs(3);
/// States buffered for the UI.
const UPDATE_QUEUE: usize = 8;
const VIDEO_PREFIX: &str = "video";
const DEV: &str = "/dev";
const PROC: &str = "/proc";
const PORTAL_PATH_REQUEST: &str = "/org/freedesktop/portal/desktop/request";
/// pavucontrol & co. open peak-detect streams to draw meters.
const PEAK_RESAMPLER: &str = "peaks";
/// Portal response code for "the user allowed it".
const PORTAL_OK: u32 = 0;

/// One app using a device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AppUse {
    pub name: String,
    pub pid: Option<u32>,
    /// Icon name the app announced, if any.
    pub icon: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PrivacyState {
    pub microphone: Vec<AppUse>,
    pub camera: Vec<AppUse>,
    pub screen_share: Vec<AppUse>,
}

impl PrivacyState {
    pub fn any(&self) -> bool {
        !(self.microphone.is_empty() && self.camera.is_empty() && self.screen_share.is_empty())
    }
}

/// Compiled ignore lists.
#[derive(Default)]
struct Ignore {
    microphone: Vec<Regex>,
    camera: Vec<Regex>,
    screen: Vec<Regex>,
}

fn compile(list: &[String]) -> Vec<Regex> {
    list.iter().map(|p| p.trim()).filter(|p| !p.is_empty()).filter_map(|p| RegexBuilder::new(p).case_insensitive(true).build().or_else(|_| Regex::new(&regex::escape(p))).ok()).collect()
}

fn ignored(list: &[Regex], app: &AppUse) -> bool {
    list.iter().any(|re| re.is_match(&app.name))
}

struct Shared {
    raw: Mutex<PrivacyState>,
    ignore: Mutex<Ignore>,
    tx: async_channel::Sender<PrivacyState>,
    last: Mutex<Option<PrivacyState>>,
}

impl Shared {
    /// Publishes the filtered state when it changed.
    fn publish(&self) {
        let state = {
            let raw = self.raw.lock();
            let ig = self.ignore.lock();
            PrivacyState {
                microphone: raw.microphone.iter().filter(|a| !ignored(&ig.microphone, a)).cloned().collect(),
                camera: raw.camera.iter().filter(|a| !ignored(&ig.camera, a)).cloned().collect(),
                screen_share: raw.screen_share.iter().filter(|a| !ignored(&ig.screen, a)).cloned().collect(),
            }
        };
        let mut last = self.last.lock();
        if last.as_ref() != Some(&state) {
            *last = Some(state.clone());
            let _ = self.tx.force_send(state);
        }
    }

    fn set(&self, f: impl FnOnce(&mut PrivacyState)) {
        f(&mut self.raw.lock());
        self.publish();
    }
}

/// A running watcher; dropping the flag's last `true` stops it.
struct Watcher(Arc<AtomicBool>);

impl Drop for Watcher {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

/// Handle to the privacy monitor. Dropping it stops every watcher.
pub struct PrivacyMonitor {
    pub updates: async_channel::Receiver<PrivacyState>,
    shared: Arc<Shared>,
    mic: Option<Watcher>,
    camera: Option<Watcher>,
    screen: Option<Watcher>,
    camera_poll: Duration,
}

impl PrivacyMonitor {
    pub fn new(cfg: &PrivacyConfig) -> Self {
        let (tx, updates) = async_channel::bounded(UPDATE_QUEUE);
        let shared = Arc::new(Shared { raw: Mutex::default(), ignore: Mutex::default(), tx, last: Mutex::new(None) });
        let mut m = Self { updates, shared, mic: None, camera: None, screen: None, camera_poll: Duration::ZERO };
        m.configure(cfg);
        m
    }

    /// Starts / stops watchers to match `cfg` and refreshes the ignore lists.
    pub fn configure(&mut self, cfg: &PrivacyConfig) {
        *self.shared.ignore.lock() = Ignore { microphone: compile(&cfg.ignore_microphone), camera: compile(&cfg.ignore_camera), screen: compile(&cfg.ignore_screen_share) };
        let on = cfg.indicators;
        let poll = Duration::from_secs(cfg.camera_poll_s.max(1));
        toggle(&mut self.mic, on && cfg.microphone, || spawn_mic(self.shared.clone()));
        if self.camera_poll != poll {
            self.camera = None;
            self.camera_poll = poll;
        }
        toggle(&mut self.camera, on && cfg.camera, || spawn_camera(self.shared.clone(), poll));
        toggle(&mut self.screen, on && cfg.screen_share, || spawn_screen(self.shared.clone()));
        // Clear what stopped being watched.
        let (mic, cam, scr) = (self.mic.is_some(), self.camera.is_some(), self.screen.is_some());
        self.shared.set(|s| {
            if !mic {
                s.microphone.clear();
            }
            if !cam {
                s.camera.clear();
            }
            if !scr {
                s.screen_share.clear();
            }
        });
    }
}

fn toggle(slot: &mut Option<Watcher>, want: bool, start: impl FnOnce() -> Watcher) {
    match (slot.is_some(), want) {
        (false, true) => *slot = Some(start()),
        (true, false) => *slot = None,
        _ => {}
    }
}

// ── Microphone ───────────────────────────────────────────────────────────

fn prop(props: &protocol::Props, p: protocol::Prop) -> Option<String> {
    props.get(p).map(|b| String::from_utf8_lossy(b.strip_suffix(&[0]).unwrap_or(b)).trim().to_string()).filter(|s| !s.is_empty())
}

fn read_mic(p: &mut Pulse) -> Result<Vec<AppUse>, String> {
    let sources: protocol::SourceInfoList = p.request(Command::GetSourceInfoList)?;
    let outputs: protocol::SourceOutputInfoList = p.request(Command::GetSourceOutputInfoList)?;
    let monitors: Vec<u32> = sources.iter().filter(|s| s.monitor_of_sink_index.is_some()).map(|s| s.index).collect();
    let me = std::process::id();
    let mut apps: Vec<AppUse> = Vec::new();
    for o in outputs {
        let pid = prop(&o.props, protocol::Prop::ApplicationProcessId).and_then(|p| p.parse::<u32>().ok());
        let peaks = o.resample_method.as_ref().is_some_and(|m| m.to_bytes() == PEAK_RESAMPLER.as_bytes());
        if monitors.contains(&o.source_index) || o.corked || peaks || pid == Some(me) {
            continue;
        }
        let name = prop(&o.props, protocol::Prop::ApplicationName).or_else(|| prop(&o.props, protocol::Prop::ApplicationProcessBinary)).unwrap_or_else(|| o.name.to_string_lossy().into_owned());
        if !apps.iter().any(|a| a.name == name) {
            apps.push(AppUse { name, pid, icon: prop(&o.props, protocol::Prop::ApplicationIconName) });
        }
    }
    Ok(apps)
}

fn spawn_mic(shared: Arc<Shared>) -> Watcher {
    let alive = Arc::new(AtomicBool::new(true));
    let flag = alive.clone();
    let _ = std::thread::Builder::new().name("mandara-privacy-mic".into()).spawn(move || {
        while flag.load(Ordering::Relaxed) {
            let result = (|| -> Result<(), String> {
                let mut p = Pulse::connect("Mandara Shell (privacy)")?;
                let mut events = Pulse::connect("Mandara Shell (privacy events)")?;
                events.sock.get_ref().set_read_timeout(Some(ALIVE_CHECK)).map_err(|e| e.to_string())?;
                events.ack(Command::Subscribe(protocol::SubscriptionMask::SOURCE_OUTPUT | protocol::SubscriptionMask::SOURCE))?;
                let apps = read_mic(&mut p)?;
                shared.set(|s| s.microphone = apps);
                loop {
                    if !flag.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    match protocol::read_command_message(&mut events.sock, events.version) {
                        Ok((_, Command::SubscribeEvent(_))) => {
                            std::thread::sleep(SETTLE);
                            let apps = read_mic(&mut p)?;
                            shared.set(|s| s.microphone = apps);
                        }
                        Ok(_) => {}
                        Err(protocol::ProtocolError::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                        Err(e) => return Err(e.to_string()),
                    }
                }
            })();
            if let Err(err) = result {
                tracing::debug!("privacy: microphone watcher: {err}");
                shared.set(|s| s.microphone.clear());
                std::thread::sleep(RECONNECT);
            }
        }
        shared.set(|s| s.microphone.clear());
    });
    Watcher(alive)
}

// ── Camera ───────────────────────────────────────────────────────────────

fn video_devices() -> Vec<PathBuf> {
    let Ok(dir) = std::fs::read_dir(DEV) else { return Vec::new() };
    dir.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(VIDEO_PREFIX)).map(|e| e.path()).collect()
}

/// A process's short name.
fn comm(pid: u32) -> Option<String> {
    std::fs::read_to_string(Path::new(PROC).join(pid.to_string()).join("comm")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Processes (readable by us) holding a video device open.
pub fn camera_users() -> Vec<AppUse> {
    let Ok(procs) = std::fs::read_dir(PROC) else { return Vec::new() };
    let mut apps: Vec<AppUse> = Vec::new();
    for entry in procs.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        // Other users' fd tables aren't readable: skipped cheaply.
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else { continue };
        let holds = fds.flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|t| t.parent() == Some(Path::new(DEV)) && t.file_name().and_then(OsStr::to_str).is_some_and(|n| n.starts_with(VIDEO_PREFIX))));
        if holds && let Some(name) = comm(pid) {
            if !apps.iter().any(|a| a.name == name) {
                apps.push(AppUse { name, pid: Some(pid), icon: None });
            }
        }
    }
    apps
}

fn spawn_camera(shared: Arc<Shared>, poll: Duration) -> Watcher {
    let alive = Arc::new(AtomicBool::new(true));
    let flag = alive.clone();
    let _ = std::thread::Builder::new().name("mandara-privacy-camera".into()).spawn(move || {
        let scan = |shared: &Shared| {
            let users = camera_users();
            shared.set(|s| s.camera = users);
        };
        scan(&shared);
        match camera_inotify(&shared, &flag, &scan) {
            Ok(()) => {}
            Err(err) => {
                tracing::info!("privacy: camera notifications unavailable ({err}); checking every {poll:?}");
                while flag.load(Ordering::Relaxed) {
                    std::thread::sleep(poll);
                    scan(&shared);
                }
            }
        }
        shared.set(|s| s.camera.clear());
    });
    Watcher(alive)
}

/// Follows open / close of every `/dev/video*` (and devices appearing).
fn camera_inotify(shared: &Shared, alive: &AtomicBool, scan: &dyn Fn(&Shared)) -> Result<(), String> {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    use rustix::fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags};
    let fd = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK).map_err(|e| e.to_string())?;
    let device_flags = WatchFlags::OPEN | WatchFlags::CLOSE_WRITE | WatchFlags::CLOSE_NOWRITE;
    let mut watched: HashMap<PathBuf, i32> = HashMap::new();
    let rewatch = |watched: &mut HashMap<PathBuf, i32>| {
        let now = video_devices();
        watched.retain(|p, _| now.contains(p));
        for dev in now {
            if !watched.contains_key(&dev)
                && let Ok(wd) = inotify::add_watch(&fd, &dev, device_flags)
            {
                watched.insert(dev, wd);
            }
        }
    };
    inotify::add_watch(&fd, DEV, WatchFlags::CREATE | WatchFlags::DELETE).map_err(|e| e.to_string())?;
    rewatch(&mut watched);
    let mut buf = [std::mem::MaybeUninit::<u8>::uninit(); 4096];
    let timeout = Timespec { tv_sec: ALIVE_CHECK.as_secs() as i64, tv_nsec: 0 };
    while alive.load(Ordering::Relaxed) {
        let mut fds = [PollFd::new(&fd, PollFlags::IN)];
        if poll(&mut fds, Some(&timeout)).map_err(|e| e.to_string())? == 0 {
            continue;
        }
        // Drain, then let the burst settle before one scan.
        let mut devices_changed = false;
        let mut relevant = false;
        let mut reader = inotify::Reader::new(&fd, &mut buf);
        while let Ok(ev) = reader.next() {
            let name = ev.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if ev.events().intersects(ReadFlags::CREATE | ReadFlags::DELETE) {
                if name.starts_with(VIDEO_PREFIX) {
                    devices_changed = true;
                    relevant = true;
                }
            } else {
                relevant = true;
            }
        }
        if devices_changed {
            rewatch(&mut watched);
        }
        if relevant {
            std::thread::sleep(SETTLE);
            // Our own scan only reads links, it never opens the devices.
            scan(shared);
        }
    }
    Ok(())
}

// ── Screen sharing ───────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Session {
    sender: String,
    app: AppUse,
    active: bool,
}

/// The request object path the portal answers a `Start` call on.
fn request_path(sender: &str, token: &str) -> String {
    format!("{PORTAL_PATH_REQUEST}/{}/{token}", sender.trim_start_matches(':').replace('.', "_"))
}

async fn sender_app(conn: &zbus::Connection, sender: &str) -> AppUse {
    let pid = async {
        let dbus = zbus::fdo::DBusProxy::new(conn).await.ok()?;
        let name = zbus::names::BusName::try_from(sender.to_string()).ok()?;
        dbus.get_connection_unix_process_id(name).await.ok()
    }
    .await;
    let name = pid.and_then(comm).unwrap_or_else(|| sender.to_string());
    AppUse { name, pid, icon: None }
}

fn spawn_screen(shared: Arc<Shared>) -> Watcher {
    let alive = Arc::new(AtomicBool::new(true));
    let flag = alive.clone();
    crate::runtime::spawn("privacy-screen", async move {
        while flag.load(Ordering::Relaxed) {
            if let Err(err) = watch_portal(&shared, &flag).await {
                tracing::debug!("privacy: screen-share watcher: {err}");
            }
            shared.set(|s| s.screen_share.clear());
            if flag.load(Ordering::Relaxed) {
                crate::runtime::sleep(RECONNECT).await;
            }
        }
    });
    Watcher(alive)
}

async fn watch_portal(shared: &Shared, alive: &AtomicBool) -> zbus::Result<()> {
    use futures::{FutureExt as _, StreamExt as _};
    // A normal connection for PID lookups; a separate one becomes the monitor.
    let lookup = crate::runtime::session().await?;
    let monitor = crate::runtime::connect(zbus::connection::Builder::session()?).await?;
    let rule = |kind: zbus::message::Type, iface: &'static str, member: &'static str| -> zbus::Result<zbus::MatchRule<'static>> {
        Ok(zbus::MatchRule::builder().msg_type(kind).interface(iface)?.member(member)?.build())
    };
    use zbus::message::Type::{MethodCall, Signal};
    let rules = vec![
        rule(MethodCall, "org.freedesktop.portal.ScreenCast", "Start")?,
        rule(MethodCall, "org.freedesktop.portal.RemoteDesktop", "Start")?,
        rule(MethodCall, "org.freedesktop.portal.Session", "Close")?,
        rule(Signal, "org.freedesktop.portal.Session", "Closed")?,
        rule(Signal, "org.freedesktop.portal.Request", "Response")?,
        rule(Signal, "org.freedesktop.DBus", "NameOwnerChanged")?,
    ];
    zbus::fdo::MonitoringProxy::new(&monitor).await?.become_monitor(&rules, 0).await?;
    let mut stream = zbus::MessageStream::from(monitor).fuse();
    // session path → session; request path → session path.
    let mut sessions: HashMap<String, Session> = HashMap::new();
    let mut requests: HashMap<String, String> = HashMap::new();
    let publish = |sessions: &HashMap<String, Session>| {
        let mut apps: Vec<AppUse> = Vec::new();
        for s in sessions.values().filter(|s| s.active) {
            if !apps.iter().any(|a| a.name == s.app.name) {
                apps.push(s.app.clone());
            }
        }
        apps.sort_by(|a, b| a.name.cmp(&b.name));
        shared.set(|st| st.screen_share = apps);
    };
    loop {
        let msg = futures::select! {
            m = stream.next() => match m { Some(Ok(m)) => m, Some(Err(e)) => return Err(e), None => return Ok(()) },
            _ = crate::runtime::sleep(ALIVE_CHECK).fuse() => {
                if !alive.load(Ordering::Relaxed) { return Ok(()) }
                continue;
            }
        };
        let header = msg.header();
        let member = header.member().map(|m| m.as_str().to_string()).unwrap_or_default();
        let sender = header.sender().map(|s| s.as_str().to_string()).unwrap_or_default();
        let path = header.path().map(|p| p.as_str().to_string()).unwrap_or_default();
        match (header.message_type(), member.as_str()) {
            (MethodCall, "Start") => {
                type StartArgs = (zbus::zvariant::OwnedObjectPath, String, HashMap<String, zbus::zvariant::OwnedValue>);
                let Ok((session, _parent, options)) = msg.body().deserialize::<StartArgs>() else { continue };
                let token = options.get("handle_token").and_then(|v| String::try_from(v.try_clone().ok()?).ok());
                let app = sender_app(&lookup, &sender).await;
                let session = session.as_str().to_string();
                // Active once the user accepts (or at once when we can't
                // see the answer).
                let active = token.is_none();
                if let Some(t) = token {
                    requests.insert(request_path(&sender, &t), session.clone());
                }
                sessions.insert(session, Session { sender, app, active });
                publish(&sessions);
            }
            (Signal, "Response") => {
                let Some(session) = requests.remove(&path) else { continue };
                let code = msg.body().deserialize::<(u32, HashMap<String, zbus::zvariant::OwnedValue>)>().map(|(c, _)| c).unwrap_or(u32::MAX);
                if code == PORTAL_OK {
                    if let Some(s) = sessions.get_mut(&session) {
                        s.active = true;
                    }
                } else {
                    sessions.remove(&session);
                }
                publish(&sessions);
            }
            (MethodCall, "Close") | (Signal, "Closed") => {
                if sessions.remove(&path).is_some() {
                    publish(&sessions);
                }
            }
            (Signal, "NameOwnerChanged") => {
                let Ok((name, _old, new)) = msg.body().deserialize::<(String, String, String)>() else { continue };
                if new.is_empty() && sessions.values().any(|s| s.sender == name) {
                    sessions.retain(|_, s| s.sender != name);
                    requests.retain(|_, sp| sessions.contains_key(sp));
                    publish(&sessions);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_paths_follow_the_portal_convention() {
        assert_eq!(request_path(":1.234", "mandara7"), "/org/freedesktop/portal/desktop/request/1_234/mandara7");
    }

    #[test]
    fn ignore_lists_filter_published_state() {
        let (tx, rx) = async_channel::bounded(4);
        let shared = Shared { raw: Mutex::default(), ignore: Mutex::default(), tx, last: Mutex::new(None) };
        *shared.ignore.lock() = Ignore { microphone: compile(&["^easy".into(), "[bad".into()]), ..Default::default() };
        let app = |n: &str| AppUse { name: n.into(), pid: None, icon: None };
        shared.set(|s| s.microphone = vec![app("EasyEffects"), app("Firefox"), app("x[bad]")]);
        let st = rx.try_recv().unwrap();
        assert_eq!(st.microphone.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), vec!["Firefox"]);
        assert!(st.any());
        // Unchanged state isn't re-sent.
        shared.publish();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn device_node_inotify_reports_opens() {
        // The mechanism the camera watcher relies on, shown on /dev/null.
        use rustix::fs::inotify::{self, CreateFlags, WatchFlags};
        let fd = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK).unwrap();
        inotify::add_watch(&fd, "/dev/null", WatchFlags::OPEN).unwrap();
        drop(std::fs::File::open("/dev/null").unwrap());
        let mut buf = [std::mem::MaybeUninit::<u8>::uninit(); 1024];
        let mut reader = inotify::Reader::new(&fd, &mut buf);
        assert!(reader.next().is_ok(), "no IN_OPEN event from a device node");
    }

    #[test]
    fn camera_scan_runs_without_devices() {
        // Must not panic or report this test process.
        let users = camera_users();
        assert!(users.iter().all(|u| u.pid != Some(std::process::id())));
    }
}
