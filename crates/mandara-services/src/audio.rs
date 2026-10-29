//! Audio actor: default sink / source volume and mute over the PulseAudio
//! native protocol (pure Rust via the `pulseaudio` crate; works with
//! PipeWire's pipewire-pulse). A second connection subscribes to server
//! events so changes from any app show up instantly.

use std::ffi::CString;
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use pulseaudio::protocol::{self, ChannelVolume, Command, Volume};
use serde::Serialize;

use crate::actor::{Envelope, ServiceHandle, reply};

/// Read timeout of the PulseAudio-protocol socket (IPC safety bound).
const PULSE_READ_TIMEOUT: Duration = Duration::from_secs(5);

pub mod mute;

/// Upper bound for boosted volume.
pub const MAX_VOLUME: f32 = 1.5;

/// The volume ceiling in effect (`[audio] overamplify`), as `f32` bits.
static VOLUME_LIMIT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3FC0_0000); // 1.5

/// Sets the highest volume commands may set (`1.0` or [`MAX_VOLUME`]).
/// Size of the native protocol's auth cookie.
const PULSE_COOKIE_LEN: usize = 256;

/// The auth cookie for the PulseAudio protocol: the user's cookie file
/// when there is one (`$PULSE_COOKIE`, `~/.config/pulse/cookie`), else 256
/// zero bytes. pipewire-pulse (1.6.10, verified against a private server
/// with no cookie file anywhere) rejects an empty or short cookie as
/// `Invalid` but accepts any 256-byte one; PipeWire never writes a cookie
/// file itself, so a fresh account has none.
pub fn pulse_cookie() -> Vec<u8> {
    pulseaudio::cookie_path_from_env().and_then(|p| std::fs::read(p).ok()).filter(|c| c.len() == PULSE_COOKIE_LEN).unwrap_or_else(|| vec![0; PULSE_COOKIE_LEN])
}

pub fn set_volume_limit(limit: f32) {
    VOLUME_LIMIT.store(limit.clamp(0.0, MAX_VOLUME).to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// The highest volume commands may set.
pub fn volume_limit() -> f32 {
    f32::from_bits(VOLUME_LIMIT.load(std::sync::atomic::Ordering::Relaxed))
}
const NORM: f32 = 0x10000 as f32;
/// Re-read the server state at least this often (events cover changes).
const FALLBACK_POLL: Duration = Duration::from_secs(10);
/// Quiet time that coalesces a burst of server events into one re-read.
const EVENT_COALESCE: Duration = Duration::from_millis(40);
/// Back-off before reconnecting to a vanished server.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
/// Read timeout of the mixer watcher (how often it checks `alive()`).
const MIXER_ALIVE_CHECK: Duration = Duration::from_secs(1);
/// States buffered for the UI.
const UPDATE_QUEUE: usize = 8;
/// Commands buffered from the UI and CLI.
const COMMAND_QUEUE: usize = 32;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Channel {
    /// Linear volume, `1.0` = 100 %.
    pub volume: f32,
    pub muted: bool,
    /// The device description ("Built-in Audio Analog Stereo").
    pub name: String,
    /// The device's server name (stable key for `[audio]` labels / hiding).
    pub device: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct AudioState {
    pub available: bool,
    pub output: Channel,
    pub input: Channel,
    /// Global mute ("Mute all sound", [`mute`]) is on.
    pub all_muted: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AudioCommand {
    SetVolume(f32),
    /// Relative change, e.g. `0.05`.
    AdjustVolume(f32),
    ToggleMute,
    SetMicVolume(f32),
    ToggleMicMute,
    /// Global mute on / off: every sink and app stream ([`mute`]).
    SetMuteAll(bool),
    ToggleMuteAll,
    /// Per-app volume (linear) for a group of app streams (sink inputs).
    SetStreamVolume(Vec<u32>, f32),
    /// Per-app mute for a group of app streams. While global mute is on this
    /// records your choice for when it is released instead ([`mute`]).
    SetStreamMute(Vec<u32>, bool),
    /// Sends app streams to another output (a [`Device::name`]).
    MoveStreams(Vec<u32>, String),
}

pub type AudioService = ServiceHandle<AudioState, AudioCommand>;

/// One authenticated protocol connection (shared with the sounds and
/// privacy actors).
pub(crate) struct Pulse {
    pub(crate) sock: BufReader<UnixStream>,
    pub(crate) version: u16,
    seq: u32,
}

impl Pulse {
    pub(crate) fn connect(name: &str) -> Result<Self, String> {
        let path = pulseaudio::socket_path_from_env().ok_or("no PulseAudio/PipeWire socket")?;
        let stream = UnixStream::connect(path).map_err(|e| e.to_string())?;
        stream.set_read_timeout(Some(PULSE_READ_TIMEOUT)).map_err(|e| e.to_string())?;
        let mut sock = BufReader::new(stream);
        let cookie = pulse_cookie();
        let auth = protocol::AuthParams { version: protocol::MAX_VERSION, supports_shm: false, supports_memfd: false, cookie };
        protocol::write_command_message(sock.get_mut(), 0, &Command::Auth(auth), protocol::MAX_VERSION).map_err(|e| e.to_string())?;
        let (_, reply) = protocol::read_reply_message::<protocol::AuthReply>(&mut sock, protocol::MAX_VERSION).map_err(|e| e.to_string())?;
        let version = std::cmp::min(protocol::MAX_VERSION, reply.version);
        let mut props = protocol::Props::new();
        props.set(protocol::Prop::ApplicationName, CString::new(name).map_err(|e| e.to_string())?);
        protocol::write_command_message(sock.get_mut(), 1, &Command::SetClientName(props), version).map_err(|e| e.to_string())?;
        protocol::read_reply_message::<protocol::SetClientNameReply>(&mut sock, version).map_err(|e| e.to_string())?;
        Ok(Self { sock, version, seq: 2 })
    }

    pub(crate) fn request<T: protocol::CommandReply>(&mut self, cmd: Command) -> Result<T, String> {
        self.seq += 1;
        protocol::write_command_message(self.sock.get_mut(), self.seq, &cmd, self.version).map_err(|e| e.to_string())?;
        protocol::read_reply_message::<T>(&mut self.sock, self.version).map(|(_, r)| r).map_err(|e| e.to_string())
    }

    /// Writes raw sample data to a stream channel (uploads, playback).
    pub(crate) fn write_data(&mut self, channel: u32, data: &[u8]) -> Result<(), String> {
        protocol::write_memblock(self.sock.get_mut(), channel, data, 0).map_err(|e| e.to_string())
    }

    pub(crate) fn ack(&mut self, cmd: Command) -> Result<(), String> {
        self.seq += 1;
        protocol::write_command_message(self.sock.get_mut(), self.seq, &cmd, self.version).map_err(|e| e.to_string())?;
        protocol::read_ack_message(&mut self.sock).map(|_| ()).map_err(|e| e.to_string())
    }

    fn defaults(&mut self) -> Result<(Option<CString>, Option<CString>), String> {
        let info: protocol::ServerInfo = self.request(Command::GetServerInfo)?;
        Ok((info.default_sink_name, info.default_source_name))
    }

    fn sink(&mut self, name: CString) -> Result<protocol::SinkInfo, String> {
        self.request(Command::GetSinkInfo(protocol::GetSinkInfo { index: None, name: Some(name) }))
    }

    fn source(&mut self, name: CString) -> Result<protocol::SourceInfo, String> {
        self.request(Command::GetSourceInfo(protocol::GetSourceInfo { index: None, name: Some(name) }))
    }
}

/// Average linear volume of a channel set (`1.0` = 100 %).
pub fn linear(cv: &ChannelVolume) -> f32 {
    let ch = cv.channels();
    if ch.is_empty() {
        return 0.0;
    }
    ch.iter().map(|v| v.as_u32() as f32 / NORM).sum::<f32>() / ch.len() as f32
}

/// A channel set of `channels` channels at linear volume `v`.
pub fn uniform(channels: usize, v: f32) -> ChannelVolume {
    let mut cv = ChannelVolume::empty();
    let raw = Volume::from_u32_clamped((v.clamp(0.0, MAX_VOLUME) * NORM).round() as u32);
    for _ in 0..channels.max(1) {
        cv.push(raw);
    }
    cv
}

fn text(c: &Option<CString>) -> String {
    c.as_ref().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn read_state(p: &mut Pulse) -> Result<AudioState, String> {
    let (sink, source) = p.defaults()?;
    let mut state = AudioState { available: true, ..Default::default() };
    if let Some(name) = sink {
        let s = p.sink(name)?;
        state.output = Channel { volume: linear(&s.cvolume), muted: s.muted, name: text(&s.description), device: s.name.to_string_lossy().into_owned() };
    }
    // No microphone: the server names a placeholder default source that
    // doesn't resolve; the output is still there.
    if let Some(name) = source
        && let Ok(s) = p.source(name)
    {
        state.input = Channel { volume: linear(&s.cvolume), muted: s.muted, name: text(&s.description), device: s.name.to_string_lossy().into_owned() };
    }
    Ok(state)
}

/// Switches global mute and starts its guard (it stops on its own).
fn set_mute_all(p: &mut Pulse, global: &mute::GlobalMute, on: bool) -> Result<String, String> {
    let r = global.set(p, on)?;
    global.ensure_watch();
    let what = if on { "all sound muted" } else { "sound restored" };
    Ok(format!("{what} ({} outputs, {} streams changed)", r.sinks, r.streams))
}

fn execute(p: &mut Pulse, state: &AudioState, command: AudioCommand, global: &mute::GlobalMute) -> Result<String, String> {
    let (sink, source) = p.defaults()?;
    let set_sink = |p: &mut Pulse, v: f32| -> Result<String, String> {
        let v = v.clamp(0.0, volume_limit());
        let name = sink.clone().ok_or("no default output")?;
        let channels = p.sink(name.clone())?.cvolume.channels().len();
        p.ack(Command::SetSinkVolume(protocol::SetDeviceVolumeParams { device_index: None, device_name: Some(name), volume: uniform(channels, v) }))?;
        Ok(format!("{:.0}%", v.clamp(0.0, MAX_VOLUME) * 100.0))
    };
    match command {
        AudioCommand::SetVolume(v) => set_sink(p, v),
        AudioCommand::AdjustVolume(d) => set_sink(p, state.output.volume + d),
        AudioCommand::SetMicVolume(v) => {
            let v = v.clamp(0.0, volume_limit());
            let name = source.ok_or("no default input")?;
            let channels = p.source(name.clone())?.cvolume.channels().len();
            p.ack(Command::SetSourceVolume(protocol::SetDeviceVolumeParams { device_index: None, device_name: Some(name), volume: uniform(channels, v) }))?;
            Ok(format!("mic {:.0}%", v.clamp(0.0, MAX_VOLUME) * 100.0))
        }
        // Unmuting the output while everything is muted means "sound back".
        AudioCommand::ToggleMute if global.enabled() => set_mute_all(p, global, false),
        AudioCommand::SetMuteAll(on) => set_mute_all(p, global, on),
        AudioCommand::ToggleMuteAll => set_mute_all(p, global, !global.enabled()),
        AudioCommand::ToggleMute => {
            let name = sink.ok_or("no default output")?;
            let mute = !p.sink(name.clone())?.muted;
            p.ack(Command::SetSinkMute(protocol::SetDeviceMuteParams { device_index: None, device_name: Some(name), mute }))?;
            Ok(if mute { "muted" } else { "unmuted" }.into())
        }
        AudioCommand::ToggleMicMute => {
            let name = source.ok_or("no default input")?;
            let mute = !p.source(name.clone())?.muted;
            p.ack(Command::SetSourceMute(protocol::SetDeviceMuteParams { device_index: None, device_name: Some(name), mute }))?;
            Ok(if mute { "mic muted" } else { "mic unmuted" }.into())
        }
        AudioCommand::SetStreamVolume(indices, v) => {
            let v = v.clamp(0.0, volume_limit());
            for &index in &indices {
                let info: protocol::SinkInputInfo = p.request(Command::GetSinkInputInfo(index))?;
                let channels = info.cvolume.channels().len();
                p.ack(Command::SetSinkInputVolume(protocol::SetStreamVolumeParams { index, volume: uniform(channels, v) }))?;
            }
            Ok(format!("{:.0}%", v * 100.0))
        }
        AudioCommand::SetStreamMute(indices, mute) => {
            // While everything is muted, an app's mute is a choice for later.
            if !global.set_own_stream_mute(p, &indices, mute)? {
                for &index in &indices {
                    p.ack(Command::SetSinkInputMute(protocol::SetStreamMuteParams { index, mute }))?;
                }
            }
            Ok(if mute { "muted" } else { "unmuted" }.into())
        }
        AudioCommand::MoveStreams(indices, name) => {
            let device = CString::new(name).map_err(|e| e.to_string())?;
            for &index in &indices {
                p.ack(Command::MoveSinkInput(protocol::MoveStreamParams { index: Some(index), device_index: None, device_name: Some(device.clone()) }))?;
            }
            Ok("moved".into())
        }
    }
}

// ── Per-app mixer and device pickers ─────────────────────────────────────
// Synchronous one-shot calls on their own connection (Studio's mixer page
// and the `audio streams` / `audio devices` commands); [`watch_mixer`]
// pushes change notifications.

/// One application playback stream (a PulseAudio sink input).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Stream {
    pub index: u32,
    /// The application's name (`application.name`, else the stream name).
    pub app: String,
    /// What it plays (`media.name`).
    pub media: String,
    /// Icon name from `application.icon_name`, if the app sets one.
    pub icon: Option<String>,
    /// Executable name (`application.process.binary`), for icon lookup.
    pub binary: Option<String>,
    /// `application.id` / Wayland app id when present.
    pub app_id: Option<String>,
    pub volume: f32,
    pub muted: bool,
    /// Paused (corked) streams are listed but dimmed.
    pub corked: bool,
    pub sink: u32,
    pub volume_writable: bool,
    /// Your own mute: `muted` normally; while global mute is on (which
    /// mutes every stream) the mute it will restore on release.
    pub own_muted: bool,
}

/// An output (sink) or input (source) device.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Device {
    pub index: u32,
    /// Server name, used to select the device.
    pub name: String,
    pub description: String,
    pub is_default: bool,
    pub volume: f32,
    pub muted: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Mixer {
    pub streams: Vec<Stream>,
    pub outputs: Vec<Device>,
    /// Inputs without sink monitors.
    pub inputs: Vec<Device>,
    /// Global mute is on (from the shell's saved record).
    pub all_muted: bool,
}

fn prop(props: &protocol::Props, p: protocol::Prop) -> Option<String> {
    props
        .get(p)
        .map(|b| String::from_utf8_lossy(b.strip_suffix(&[0]).unwrap_or(b)).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn read_mixer(p: &mut Pulse) -> Result<Mixer, String> {
    let (default_sink, default_source) = p.defaults()?;
    let inputs: protocol::SinkInputInfoList = p.request(Command::GetSinkInputInfoList)?;
    let sinks: protocol::SinkInfoList = p.request(Command::GetSinkInfoList)?;
    let sources: protocol::SourceInfoList = p.request(Command::GetSourceInfoList)?;
    let global = mute::MuteState::load(&mute::state_path());
    let streams = inputs
        .into_iter()
        .map(|s| {
            let app = prop(&s.props, protocol::Prop::ApplicationName).unwrap_or_else(|| s.name.to_string_lossy().into_owned());
            Stream {
                index: s.index,
                media: prop(&s.props, protocol::Prop::MediaName).filter(|m| *m != app).unwrap_or_default(),
                icon: prop(&s.props, protocol::Prop::ApplicationIconName),
                binary: prop(&s.props, protocol::Prop::ApplicationProcessBinary),
                app_id: prop(&s.props, protocol::Prop::ApplicationId),
                own_muted: global.own_mute(s.index, &mute::restore_key(&s.props, &s.name.to_string_lossy()), s.muted),
                app,
                volume: linear(&s.cvolume),
                muted: s.muted,
                corked: s.corked,
                sink: s.sink_index,
                volume_writable: s.volume_writable || s.has_volume,
            }
        })
        .collect();
    let outputs = sinks
        .into_iter()
        .map(|s| Device {
            index: s.index,
            is_default: default_sink.as_ref() == Some(&s.name),
            description: s.description.as_ref().map(|d| d.to_string_lossy().into_owned()).unwrap_or_else(|| s.name.to_string_lossy().into_owned()),
            name: s.name.to_string_lossy().into_owned(),
            volume: linear(&s.cvolume),
            muted: s.muted,
        })
        .collect();
    let inputs = sources
        .into_iter()
        .filter(|s| s.monitor_of_sink_index.is_none())
        .map(|s| Device {
            index: s.index,
            is_default: default_source.as_ref() == Some(&s.name),
            description: s.description.as_ref().map(|d| d.to_string_lossy().into_owned()).unwrap_or_else(|| s.name.to_string_lossy().into_owned()),
            name: s.name.to_string_lossy().into_owned(),
            volume: linear(&s.cvolume),
            muted: s.muted,
        })
        .collect();
    Ok(Mixer { streams, outputs, inputs, all_muted: global.enabled })
}

/// Application streams plus output / input devices, read once.
pub fn mixer() -> Result<Mixer, String> {
    read_mixer(&mut Pulse::connect("Mandara Mixer")?)
}

/// Sets one application stream's volume (linear, `1.0` = 100 %).
pub fn set_stream_volume(index: u32, volume: f32) -> Result<(), String> {
    let mut p = Pulse::connect("Mandara Mixer")?;
    let info: protocol::SinkInputInfo = p.request(Command::GetSinkInputInfo(index))?;
    let channels = info.cvolume.channels().len();
    p.ack(Command::SetSinkInputVolume(protocol::SetStreamVolumeParams { index, volume: uniform(channels, volume) }))
}

pub fn set_stream_mute(index: u32, mute: bool) -> Result<(), String> {
    Pulse::connect("Mandara Mixer")?.ack(Command::SetSinkInputMute(protocol::SetStreamMuteParams { index, mute }))
}

/// Makes `name` (a [`Device::name`]) the default output.
pub fn set_default_output(name: &str) -> Result<(), String> {
    let name = CString::new(name).map_err(|e| e.to_string())?;
    Pulse::connect("Mandara Mixer")?.ack(Command::SetDefaultSink(name))
}

/// Makes `name` the default input.
pub fn set_default_input(name: &str) -> Result<(), String> {
    let name = CString::new(name).map_err(|e| e.to_string())?;
    Pulse::connect("Mandara Mixer")?.ack(Command::SetDefaultSource(name))
}

/// Calls `on_change` (from a background thread) whenever a stream, device
/// or the server defaults change. The watcher stops once `on_change`
/// returns `false` or `alive()` turns false (checked at least every second).
pub fn watch_mixer(alive: impl Fn() -> bool + Send + 'static, on_change: impl Fn() -> bool + Send + 'static) {
    let _ = std::thread::Builder::new().name("mandara-mixer-events".into()).spawn(move || {
        while alive() {
            let result = (|| -> Result<bool, String> {
                let mut p = Pulse::connect("Mandara Mixer (events)")?;
                p.sock.get_ref().set_read_timeout(Some(MIXER_ALIVE_CHECK)).map_err(|e| e.to_string())?;
                let mask = protocol::SubscriptionMask::SINK_INPUT | protocol::SubscriptionMask::SINK | protocol::SubscriptionMask::SOURCE | protocol::SubscriptionMask::SERVER;
                p.ack(Command::Subscribe(mask))?;
                loop {
                    if !alive() {
                        return Ok(false);
                    }
                    match protocol::read_command_message(&mut p.sock, p.version) {
                        Ok((_, Command::SubscribeEvent(_))) => {
                            // Coalesce bursts (a new stream sends several events).
                            std::thread::sleep(EVENT_COALESCE);
                            if !on_change() {
                                return Ok(false);
                            }
                        }
                        Ok(_) => {}
                        Err(protocol::ProtocolError::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                        Err(e) => return Err(e.to_string()),
                    }
                }
            })();
            if matches!(result, Ok(false)) {
                return;
            }
            std::thread::sleep(RECONNECT_DELAY);
        }
    });
}

// ── Per-app outputs ──────────────────────────────────────────────────────

/// Whether stream properties match a route's app pattern (app name, binary,
/// app id or media role; case-insensitive).
pub fn route_matches(pattern: &str, props: &protocol::Props) -> bool {
    let want = pattern.trim().to_lowercase();
    !want.is_empty()
        && [protocol::Prop::ApplicationName, protocol::Prop::ApplicationProcessBinary, protocol::Prop::ApplicationId, protocol::Prop::MediaRole]
            .into_iter()
            .filter_map(|k| prop(props, k))
            .any(|v| v.to_lowercase() == want)
}

/// Moves new app streams to the output their route names. Runs while
/// `routes` is non-empty; replacing the routes restarts it.
pub struct RouteWatcher(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for RouteWatcher {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Starts routing (`None` when there's nothing to route).
pub fn route_streams(routes: Vec<mandara_config::alerts::AudioRoute>) -> Option<RouteWatcher> {
    use std::sync::atomic::Ordering;
    let routes: Vec<_> = routes.into_iter().filter(|r| !r.app.trim().is_empty() && !r.output.trim().is_empty()).collect();
    if routes.is_empty() {
        return None;
    }
    let alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flag = alive.clone();
    let _ = std::thread::Builder::new().name("mandara-audio-routes".into()).spawn(move || {
        while flag.load(Ordering::Relaxed) {
            let result = (|| -> Result<(), String> {
                let mut p = Pulse::connect("Mandara Shell (routes)")?;
                let mut events = Pulse::connect("Mandara Shell (route events)")?;
                events.sock.get_ref().set_read_timeout(Some(MIXER_ALIVE_CHECK)).map_err(|e| e.to_string())?;
                events.ack(Command::Subscribe(protocol::SubscriptionMask::SINK_INPUT))?;
                loop {
                    if !flag.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    match protocol::read_command_message(&mut events.sock, events.version) {
                        Ok((_, Command::SubscribeEvent(ev)))
                            if ev.event_facility == protocol::SubscriptionEventFacility::SinkInput && ev.event_type == protocol::SubscriptionEventType::New =>
                        {
                            let Some(index) = ev.index else { continue };
                            let Ok(info) = p.request::<protocol::SinkInputInfo>(Command::GetSinkInputInfo(index)) else { continue };
                            if let Some(route) = routes.iter().find(|r| route_matches(&r.app, &info.props)) {
                                let name = CString::new(route.output.trim()).map_err(|e| e.to_string())?;
                                if let Err(err) = p.ack(Command::MoveSinkInput(protocol::MoveStreamParams { index: Some(index), device_index: None, device_name: Some(name) })) {
                                    tracing::debug!("audio: routing stream {index} to {}: {err}", route.output);
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(protocol::ProtocolError::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                        Err(e) => return Err(e.to_string()),
                    }
                }
            })();
            if let Err(err) = result {
                tracing::debug!("audio routes: {err}");
                std::thread::sleep(RECONNECT_DELAY);
            }
        }
    });
    Some(RouteWatcher(alive))
}

/// Runs blocking PulseAudio work with the actor's connection on the
/// blocking pool and hands the connection back.
async fn with_pulse<T: Send + 'static>(conn: Option<Pulse>, f: impl FnOnce(&mut Option<Pulse>) -> T + Send + 'static) -> (Option<Pulse>, T) {
    crate::runtime::unblock(move || {
        let mut conn = conn;
        let out = f(&mut conn);
        (conn, out)
    })
    .await
}

pub fn spawn_audio_service() -> AudioService {
    let (tx, updates) = async_channel::bounded(UPDATE_QUEUE);
    let (commands, rx) = async_channel::bounded::<Envelope<AudioCommand>>(COMMAND_QUEUE);
    // Server events (sink / source / server changes) wake the actor. One
    // pending wake-up is enough: it triggers a full re-read.
    let (poke_tx, poke_rx) = async_channel::bounded::<()>(1);
    // A dedicated thread: the Pulse protocol client blocks in `read(2)`,
    // which costs nothing while the server is quiet.
    let _ = std::thread::Builder::new().name("mandara-audio-events".into()).spawn(move || {
        loop {
            let result = (|| -> Result<(), String> {
                let mut p = Pulse::connect("Mandara Shell (events)")?;
                p.sock.get_ref().set_read_timeout(None).map_err(|e| e.to_string())?;
                p.ack(Command::Subscribe(protocol::SubscriptionMask::SINK | protocol::SubscriptionMask::SOURCE | protocol::SubscriptionMask::SERVER))?;
                loop {
                    let (_, event) = protocol::read_command_message(&mut p.sock, p.version).map_err(|e| e.to_string())?;
                    if matches!(event, Command::SubscribeEvent(_)) && poke_tx.force_send(()).is_err() {
                        return Ok(());
                    }
                }
            })();
            if result.is_ok() {
                return;
            }
            std::thread::sleep(RECONNECT_DELAY);
        }
    });
    // Global mute: re-applied on start when it was on (the guard sweeps).
    let global = mute::GlobalMute::load(mute::state_path());
    global.ensure_watch();
    crate::runtime::spawn("audio", async move {
        use futures::FutureExt as _;
        let mut conn: Option<Pulse> = None;
        let mut last: Option<AudioState> = None;
        loop {
            let all_muted = global.enabled();
            let (c, state) = with_pulse(conn.take(), move |conn| {
                if conn.is_none() {
                    *conn = Pulse::connect("Mandara Shell").ok();
                }
                match conn.as_mut().map(read_state) {
                    Some(Ok(s)) => AudioState { all_muted, ..s },
                    Some(Err(_)) => {
                        *conn = None; // reconnect next round
                        AudioState::default()
                    }
                    None => AudioState::default(),
                }
            })
            .await;
            conn = c;
            if last.as_ref() != Some(&state) {
                if tx.send(state.clone()).await.is_err() {
                    return;
                }
                last = Some(state.clone());
            }
            // Wait for a command, a server event, or the slow fallback poll.
            futures::select! {
                cmd = rx.recv().fuse() => {
                    let Ok(Envelope { command, reply: r }) = cmd else { return };
                    let global = global.clone();
                    let (c, result) = with_pulse(conn.take(), move |conn| match conn.as_mut() {
                        Some(p) => execute(p, &state, command, &global),
                        None => Err(mandara_i18n::t("The audio server is unavailable").into()),
                    })
                    .await;
                    conn = c;
                    reply(&r, result);
                }
                poke = poke_rx.recv().fuse() => {
                    if poke.is_ok() {
                        // Coalesce the rest of the burst.
                        crate::runtime::sleep(EVENT_COALESCE).await;
                        while poke_rx.try_recv().is_ok() {}
                    } else {
                        crate::runtime::sleep(FALLBACK_POLL).await;
                    }
                }
                _ = crate::runtime::sleep(FALLBACK_POLL).fuse() => {}
            }
        }
    });
    ServiceHandle::new(updates, commands)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_pulse_cookie_is_always_full_size() {
        // pipewire-pulse refuses an empty or short cookie.
        assert_eq!(super::pulse_cookie().len(), super::PULSE_COOKIE_LEN);
    }

    use super::*;

    #[test]
    fn volume_conversions() {
        let cv = uniform(2, 0.5);
        assert_eq!(cv.channels().len(), 2);
        assert!((linear(&cv) - 0.5).abs() < 1e-4);
        assert!((linear(&uniform(1, 9.0)) - MAX_VOLUME).abs() < 1e-4, "clamped to the boost ceiling");
        assert_eq!(linear(&ChannelVolume::empty()), 0.0);
    }

    /// Read-only: lists streams and devices without changing anything.
    #[test]
    fn mixer_lists_devices_if_available() {
        if Pulse::connect("mandara-test").is_err() {
            return;
        }
        let m = mixer().unwrap();
        assert!(m.outputs.iter().filter(|d| d.is_default).count() <= 1);
        assert!(m.inputs.iter().all(|d| !d.name.ends_with(".monitor")), "sink monitors are not inputs");
        assert!(m.streams.iter().all(|s| !s.app.is_empty()));
    }

    #[test]
    fn props_strip_nul_terminators() {
        let mut props = protocol::Props::new();
        props.set(protocol::Prop::ApplicationName, CString::new("Firefox").unwrap());
        assert_eq!(prop(&props, protocol::Prop::ApplicationName).as_deref(), Some("Firefox"));
        assert_eq!(prop(&props, protocol::Prop::MediaName), None);
    }

    #[test]
    fn live_server_if_available() {
        if let Ok(mut p) = Pulse::connect("mandara-test") {
            let state = read_state(&mut p).unwrap();
            assert!(state.available);
        }
    }
}
