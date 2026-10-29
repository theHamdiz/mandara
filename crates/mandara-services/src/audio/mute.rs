//! Global mute ("Mute all sound"): one switch that silences every output.
//!
//! Muting the default output is not enough on PipeWire setups with virtual
//! sinks: EasyEffects' `easyeffects_sink` is a null sink with
//! `monitor.channel-volumes = false`, so its mute never reaches the monitor
//! EasyEffects records from, and the processed sound keeps playing on the
//! hardware sink. Global mute therefore mutes **every** sink (hardware and
//! virtual) and, as belt-and-braces, every app stream. While it is on, a
//! guard thread mutes sinks and streams that appear or get unmuted.
//!
//! Each sink's and stream's previous mute is recorded first and restored
//! exactly on release, so things you had muted yourself stay muted. The
//! record lives in `~/.local/state/mandara/audio-mute.toml` and survives a
//! shell restart (global mute is re-applied on start).
//!
//! WirePlumber remembers per-app stream mutes (`restore-stream`): an app
//! whose stream ended while muted would start its next stream muted after
//! release. Such apps (and sinks unplugged while muted) are kept as
//! *pending*: their next stream / reappearance is unmuted once, within
//! [`PENDING_TTL`].

use std::collections::BTreeSet;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pulseaudio::protocol::{self, Command};
use serde::{Deserialize, Serialize};

use super::{MIXER_ALIVE_CHECK, Pulse, RECONNECT_DELAY, prop};

/// How long an app / sink muted by a released global mute is watched for
/// its next appearance (to undo WirePlumber's remembered mute).
pub const PENDING_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Stream properties WirePlumber keys remembered stream state on, in its
/// order of preference (`restore-stream`).
const RESTORE_KEYS: [(&str, protocol::Prop); 4] = [
    ("media.role", protocol::Prop::MediaRole),
    ("application.id", protocol::Prop::ApplicationId),
    ("application.name", protocol::Prop::ApplicationName),
    ("media.name", protocol::Prop::MediaName),
];

/// A sink or app stream as the mute logic sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    /// Server index (stable while the object lives).
    pub index: u32,
    /// Identity across reconnects: the sink name, or a stream's restore key.
    pub key: String,
    pub muted: bool,
}

/// What global mute needs from the sound server (mocked in tests).
pub trait MuteBackend {
    fn sinks(&mut self) -> Result<Vec<Node>, String>;
    fn streams(&mut self) -> Result<Vec<Node>, String>;
    /// One sink, `None` when it is gone.
    fn sink(&mut self, index: u32) -> Option<Node>;
    /// One stream, `None` when it is gone.
    fn stream(&mut self, index: u32) -> Option<Node>;
    fn set_sink_mute(&mut self, index: u32, mute: bool) -> Result<(), String>;
    fn set_stream_mute(&mut self, index: u32, mute: bool) -> Result<(), String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Facility {
    Sink,
    Stream,
}

/// A sink or stream's mute from before global mute.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub key: String,
    pub index: u32,
    pub was_muted: bool,
}

/// A sink / app to unmute once when it next appears (see the module docs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub facility: Facility,
    pub key: String,
    /// Unix seconds when global mute was released.
    pub since: u64,
}

/// How many sinks / streams a switch changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    pub sinks: usize,
    pub streams: usize,
}

/// The persisted global-mute record.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MuteState {
    pub enabled: bool,
    /// Every sink seen while on, by name.
    pub sinks: Vec<Saved>,
    /// Streams seen while on (ended ones are dropped on the next sweep).
    pub streams: Vec<Saved>,
    /// Restore keys of streams this mute silenced (for WirePlumber's
    /// remembered mutes, see the module docs).
    pub muted_apps: BTreeSet<String>,
    pub pending: Vec<Pending>,
}

/// Unix seconds now.
pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default()
}

/// `~/.local/state/mandara/audio-mute.toml`.
pub fn state_path() -> PathBuf {
    dirs::state_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/state")).join("mandara").join("audio-mute.toml")
}

/// Whether global mute is on, per the saved record (for other processes,
/// e.g. Studio).
pub fn is_enabled() -> bool {
    MuteState::load(&state_path()).enabled
}

/// A stream's identity the way WirePlumber remembers it.
pub fn restore_key(props: &protocol::Props, fallback: &str) -> String {
    RESTORE_KEYS
        .iter()
        .find_map(|(label, p)| prop(props, *p).map(|v| format!("{label}:{v}")))
        .unwrap_or_else(|| format!("node.name:{fallback}"))
}

impl MuteState {
    /// The saved record, or the default (off) when missing or unreadable.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    /// Writes the record atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    /// Whether the guard must watch the server (on, or pending fix-ups).
    pub fn wants_watch(&self) -> bool {
        self.enabled || !self.pending.is_empty()
    }

    /// Drops pending fix-ups older than [`PENDING_TTL`].
    pub fn prune(&mut self, now: u64) {
        self.pending.retain(|p| now.saturating_sub(p.since) < PENDING_TTL.as_secs());
    }

    fn take_pending(&mut self, facility: Facility, key: &str) -> bool {
        let before = self.pending.len();
        self.pending.retain(|p| !(p.facility == facility && p.key == key));
        self.pending.len() != before
    }

    /// Records a sink's mute (once per name). Returns whether it needs muting.
    fn record_sink(&mut self, node: &Node) -> bool {
        if let Some(s) = self.sinks.iter_mut().find(|s| s.key == node.key) {
            s.index = node.index;
        } else {
            // Muted only because a released global mute left it so: not the user's.
            let ours = self.take_pending(Facility::Sink, &node.key);
            self.sinks.push(Saved { key: node.key.clone(), index: node.index, was_muted: node.muted && !ours });
        }
        !node.muted
    }

    /// Records a stream's mute (once per index). Returns whether it needs muting.
    fn record_stream(&mut self, node: &Node) -> bool {
        if !self.streams.iter().any(|s| s.index == node.index) {
            // A new stream of an app this mute silenced starts muted because
            // WirePlumber remembered our mute: that is not the user's.
            let ours = self.muted_apps.contains(&node.key) | self.take_pending(Facility::Stream, &node.key);
            let was_muted = node.muted && !ours;
            if !was_muted {
                self.muted_apps.insert(node.key.clone());
            }
            self.streams.push(Saved { key: node.key.clone(), index: node.index, was_muted });
        }
        !node.muted
    }

    /// Turns global mute on: records every sink and stream, saves the
    /// record (`save`), then mutes them. Also re-applies after a restart.
    pub fn engage(&mut self, b: &mut impl MuteBackend, save: impl FnOnce(&Self)) -> Result<Report, String> {
        let (sinks, streams) = (b.sinks()?, b.streams()?);
        self.enabled = true;
        let todo_sinks: Vec<u32> = sinks.iter().filter(|n| self.record_sink(n)).map(|n| n.index).collect();
        let todo_streams: Vec<u32> = streams.iter().filter(|n| self.record_stream(n)).map(|n| n.index).collect();
        // Ended streams' records are no longer needed (their apps stay in `muted_apps`).
        self.streams.retain(|s| streams.iter().any(|n| n.index == s.index));
        save(self);
        let mut report = Report::default();
        for index in todo_sinks {
            report.sinks += usize::from(quietly(b.set_sink_mute(index, true)));
        }
        for index in todo_streams {
            report.streams += usize::from(quietly(b.set_stream_mute(index, true)));
        }
        Ok(report)
    }

    /// Turns global mute off: unmutes exactly what it muted, then saves.
    pub fn release(&mut self, b: &mut impl MuteBackend, now: u64, save: impl FnOnce(&Self)) -> Result<Report, String> {
        let (sinks, streams) = (b.sinks()?, b.streams()?);
        let mut report = Report::default();
        let mut pending = Vec::new();
        for saved in self.sinks.iter().filter(|s| !s.was_muted) {
            match sinks.iter().find(|n| n.key == saved.key) {
                Some(n) if n.muted => report.sinks += usize::from(quietly(b.set_sink_mute(n.index, false))),
                Some(_) => {}
                None => pending.push(Pending { facility: Facility::Sink, key: saved.key.clone(), since: now }),
            }
        }
        for n in &streams {
            let saved = self.streams.iter().find(|s| s.index == n.index);
            // Streams the guard missed count by app.
            let ours = saved.map(|s| !s.was_muted).unwrap_or_else(|| self.muted_apps.contains(&n.key));
            if ours && n.muted {
                report.streams += usize::from(quietly(b.set_stream_mute(n.index, false)));
            }
        }
        // Apps with no live stream keep WirePlumber's remembered mute: fix
        // their next stream.
        for key in self.muted_apps.iter().filter(|k| !streams.iter().any(|n| &n.key == *k)) {
            pending.push(Pending { facility: Facility::Stream, key: key.clone(), since: now });
        }
        self.enabled = false;
        self.sinks.clear();
        self.streams.clear();
        self.muted_apps.clear();
        for p in pending {
            if !self.pending.iter().any(|q| q.facility == p.facility && q.key == p.key) {
                self.pending.push(p);
            }
        }
        save(self);
        Ok(report)
    }

    /// Your own mute of a stream (`muted` is its mute on the server). While
    /// global mute is on every stream is muted, so this is the mute release
    /// will leave it with.
    pub fn own_mute(&self, index: u32, key: &str, muted: bool) -> bool {
        if !self.enabled {
            return muted;
        }
        match self.streams.iter().find(|s| s.index == index) {
            Some(saved) => saved.was_muted,
            // Not recorded yet: muted is yours unless it is our remembered mute.
            None => muted && !self.muted_apps.contains(key),
        }
    }

    /// You muted / unmuted an app stream while global mute is on: changes
    /// what release restores, never the snapshot of anything else. Returns
    /// whether the stream must be muted on the server now (it slipped past
    /// the guard).
    pub fn set_own_stream_mute(&mut self, node: &Node, muted: bool) -> bool {
        match self.streams.iter_mut().find(|s| s.index == node.index) {
            Some(saved) => saved.was_muted = muted,
            None => self.streams.push(Saved { key: node.key.clone(), index: node.index, was_muted: muted }),
        }
        if muted {
            // The app's mute is now yours: release must not unmute its later
            // streams as WirePlumber leftovers (unless another of its live
            // streams still counts as ours).
            if !self.streams.iter().any(|s| s.key == node.key && !s.was_muted) {
                self.muted_apps.remove(&node.key);
            }
        } else {
            self.muted_apps.insert(node.key.clone());
        }
        muted && !node.muted
    }

    /// A sink or stream appeared or changed. While on, it is recorded and
    /// muted; while off, a pending one is unmuted once. Returns whether the
    /// record changed (needs saving).
    pub fn on_event(&mut self, b: &mut impl MuteBackend, facility: Facility, index: u32, save: impl FnOnce(&Self)) -> bool {
        let node = match facility {
            Facility::Sink => b.sink(index),
            Facility::Stream => b.stream(index),
        };
        let Some(node) = node else { return false };
        if self.enabled {
            let before = self.clone();
            let needs_mute = match facility {
                Facility::Sink => self.record_sink(&node),
                Facility::Stream => self.record_stream(&node),
            };
            let changed = *self != before;
            if changed {
                save(self);
            }
            if needs_mute {
                apply(b, facility, node.index, true);
            }
            changed
        } else if self.take_pending(facility, &node.key) {
            save(self);
            if node.muted {
                apply(b, facility, node.index, false);
            }
            true
        } else {
            false
        }
    }
}

/// Mutes / unmutes one sink or stream; whether it worked.
fn apply(b: &mut impl MuteBackend, facility: Facility, index: u32, mute: bool) -> bool {
    quietly(match facility {
        Facility::Sink => b.set_sink_mute(index, mute),
        Facility::Stream => b.set_stream_mute(index, mute),
    })
}

/// Logs a failed mute change (the object may have just vanished).
fn quietly(result: Result<(), String>) -> bool {
    match result {
        Ok(()) => true,
        Err(err) => {
            tracing::debug!("audio mute-all: {err}");
            false
        }
    }
}

// ── The live server ──────────────────────────────────────────────────────

impl MuteBackend for Pulse {
    fn sinks(&mut self) -> Result<Vec<Node>, String> {
        let list: protocol::SinkInfoList = self.request(Command::GetSinkInfoList)?;
        Ok(list.into_iter().map(|s| Node { index: s.index, key: s.name.to_string_lossy().into_owned(), muted: s.muted }).collect())
    }

    fn streams(&mut self) -> Result<Vec<Node>, String> {
        let list: protocol::SinkInputInfoList = self.request(Command::GetSinkInputInfoList)?;
        Ok(list.into_iter().map(|s| Node { index: s.index, key: restore_key(&s.props, &s.name.to_string_lossy()), muted: s.muted }).collect())
    }

    fn sink(&mut self, index: u32) -> Option<Node> {
        let s: protocol::SinkInfo = self.request(Command::GetSinkInfo(protocol::GetSinkInfo { index: Some(index), name: None })).ok()?;
        Some(Node { index: s.index, key: s.name.to_string_lossy().into_owned(), muted: s.muted })
    }

    fn stream(&mut self, index: u32) -> Option<Node> {
        let s: protocol::SinkInputInfo = self.request(Command::GetSinkInputInfo(index)).ok()?;
        Some(Node { index: s.index, key: restore_key(&s.props, &s.name.to_string_lossy()), muted: s.muted })
    }

    fn set_sink_mute(&mut self, index: u32, mute: bool) -> Result<(), String> {
        self.ack(Command::SetSinkMute(protocol::SetDeviceMuteParams { device_index: Some(index), device_name: None::<CString>, mute }))
    }

    fn set_stream_mute(&mut self, index: u32, mute: bool) -> Result<(), String> {
        self.ack(Command::SetSinkInputMute(protocol::SetStreamMuteParams { index, mute }))
    }
}

/// The shell's global mute: the record plus the guard thread that keeps
/// new sinks / streams muted while it is on.
#[derive(Clone)]
pub struct GlobalMute {
    state: Arc<Mutex<MuteState>>,
    path: Arc<PathBuf>,
    watching: Arc<AtomicBool>,
}

impl GlobalMute {
    /// Loads the saved record from `path` (does not touch the server).
    pub fn load(path: PathBuf) -> Self {
        let mut state = MuteState::load(&path);
        state.prune(now());
        Self { state: Arc::new(Mutex::new(state)), path: Arc::new(path), watching: Arc::default() }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MuteState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn saver(&self) -> impl Fn(&MuteState) + '_ {
        |s: &MuteState| {
            if let Err(err) = s.save(&self.path) {
                tracing::warn!("audio mute-all: saving {}: {err}", self.path.display());
            }
        }
    }

    pub fn enabled(&self) -> bool {
        self.lock().enabled
    }

    /// A copy of the record (for `audio mute-all status`).
    pub fn snapshot(&self) -> MuteState {
        self.lock().clone()
    }

    /// Switches global mute on or off through `b` and saves the record.
    /// Does not start the guard: call [`Self::ensure_watch`] after it.
    pub fn set(&self, b: &mut impl MuteBackend, on: bool) -> Result<Report, String> {
        let mut s = self.lock();
        if on { s.engage(b, self.saver()) } else if s.enabled { s.release(b, now(), self.saver()) } else { Ok(Report::default()) }
    }

    /// Records your mute of app streams while global mute is on (see
    /// [`MuteState::set_own_stream_mute`]) and saves. Returns `false`, doing
    /// nothing, when global mute is off: mute the streams directly then.
    pub fn set_own_stream_mute(&self, b: &mut impl MuteBackend, indices: &[u32], muted: bool) -> Result<bool, String> {
        let mut s = self.lock();
        if !s.enabled {
            return Ok(false);
        }
        let mut changed = false;
        for &index in indices {
            let Some(node) = b.stream(index) else { continue };
            changed = true;
            if s.set_own_stream_mute(&node, muted) {
                quietly(b.set_stream_mute(index, true));
            }
        }
        if changed {
            self.saver()(&s);
        }
        Ok(true)
    }

    /// Starts the guard thread when there is something to watch (no-op if
    /// it already runs). It exits on its own once nothing is left.
    pub fn ensure_watch(&self) {
        if !self.lock().wants_watch() || self.watching.swap(true, Ordering::AcqRel) {
            return;
        }
        let this = self.clone();
        let spawned = std::thread::Builder::new().name("mandara-audio-mute".into()).spawn(move || {
            loop {
                if let Err(err) = this.watch() {
                    tracing::debug!("audio mute-all guard: {err}");
                    std::thread::sleep(RECONNECT_DELAY);
                }
                this.watching.store(false, Ordering::Release);
                // Re-check after clearing the flag so a switch made meanwhile isn't lost.
                if !this.lock().wants_watch() || this.watching.swap(true, Ordering::AcqRel) {
                    return;
                }
            }
        });
        if spawned.is_err() {
            self.watching.store(false, Ordering::Release);
        }
    }

    /// One guard connection: catch up, then follow sink / stream events
    /// until nothing is left to watch.
    fn watch(&self) -> Result<(), String> {
        let mut p = Pulse::connect("Mandara Shell (mute-all)")?;
        let mut events = Pulse::connect("Mandara Shell (mute-all events)")?;
        events.sock.get_ref().set_read_timeout(Some(MIXER_ALIVE_CHECK)).map_err(|e| e.to_string())?;
        events.ack(Command::Subscribe(protocol::SubscriptionMask::SINK | protocol::SubscriptionMask::SINK_INPUT))?;
        {
            // Re-apply after a restart or reconnect (events may have been missed).
            let mut s = self.lock();
            if s.enabled {
                s.engage(&mut p, self.saver())?;
            }
        }
        loop {
            {
                let mut s = self.lock();
                s.prune(now());
                if !s.wants_watch() {
                    return Ok(());
                }
            }
            match protocol::read_command_message(&mut events.sock, events.version) {
                Ok((_, Command::SubscribeEvent(ev))) if ev.event_type != protocol::SubscriptionEventType::Removed => {
                    let facility = match ev.event_facility {
                        protocol::SubscriptionEventFacility::Sink => Facility::Sink,
                        protocol::SubscriptionEventFacility::SinkInput => Facility::Stream,
                        _ => continue,
                    };
                    let Some(index) = ev.index else { continue };
                    self.lock().on_event(&mut p, facility, index, self.saver());
                }
                Ok(_) => {}
                Err(protocol::ProtocolError::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// An in-memory sound server.
    #[derive(Default)]
    struct Mock {
        sinks: BTreeMap<u32, Node>,
        streams: BTreeMap<u32, Node>,
    }

    impl Mock {
        fn sink(&mut self, index: u32, key: &str, muted: bool) {
            self.sinks.insert(index, Node { index, key: key.into(), muted });
        }
        fn stream(&mut self, index: u32, key: &str, muted: bool) {
            self.streams.insert(index, Node { index, key: key.into(), muted });
        }
        fn muted(&self) -> (Vec<bool>, Vec<bool>) {
            (self.sinks.values().map(|n| n.muted).collect(), self.streams.values().map(|n| n.muted).collect())
        }
    }

    impl MuteBackend for Mock {
        fn sinks(&mut self) -> Result<Vec<Node>, String> {
            Ok(self.sinks.values().cloned().collect())
        }
        fn streams(&mut self) -> Result<Vec<Node>, String> {
            Ok(self.streams.values().cloned().collect())
        }
        fn sink(&mut self, index: u32) -> Option<Node> {
            self.sinks.get(&index).cloned()
        }
        fn stream(&mut self, index: u32) -> Option<Node> {
            self.streams.get(&index).cloned()
        }
        fn set_sink_mute(&mut self, index: u32, mute: bool) -> Result<(), String> {
            self.sinks.get_mut(&index).map(|n| n.muted = mute).ok_or_else(|| "no such sink".into())
        }
        fn set_stream_mute(&mut self, index: u32, mute: bool) -> Result<(), String> {
            self.streams.get_mut(&index).map(|n| n.muted = mute).ok_or_else(|| "no such stream".into())
        }
    }

    fn server() -> Mock {
        let mut m = Mock::default();
        m.sink(53, "alsa_output.usb", false);
        m.sink(69, "alsa_output.hdmi", true); // muted by the user
        m.sink(86, "easyeffects_sink", false);
        m.stream(100, "application.name:Brave", false);
        m.stream(101, "application.name:mpv", true); // muted by the user
        m
    }

    #[test]
    fn engage_mutes_everything_and_release_restores_exactly() {
        let mut m = server();
        let before = m.muted();
        let mut s = MuteState::default();
        let r = s.engage(&mut m, |_| {}).unwrap();
        assert_eq!(r, Report { sinks: 2, streams: 1 });
        assert!(m.sinks.values().chain(m.streams.values()).all(|n| n.muted));
        let r = s.release(&mut m, 0, |_| {}).unwrap();
        assert_eq!(r, Report { sinks: 2, streams: 1 });
        assert_eq!(m.muted(), before, "user mutes stay, the rest come back");
        assert!(!s.wants_watch(), "nothing pending: every stream was still live");
    }

    #[test]
    fn snapshot_is_saved_before_anything_is_muted() {
        let mut m = server();
        let mut s = MuteState::default();
        let mut seen = None;
        s.engage(&mut m, |st| seen = Some(st.clone())).unwrap();
        let seen = seen.unwrap();
        assert!(seen.enabled && seen.sinks.len() == 3 && seen.streams.len() == 2);
        assert_eq!(seen.sinks.iter().filter(|s| s.was_muted).count(), 1);
    }

    #[test]
    fn new_and_unmuted_objects_are_muted_while_on() {
        let mut m = server();
        let mut s = MuteState::default();
        s.engage(&mut m, |_| {}).unwrap();
        // A notification sound and a plugged-in headset.
        m.stream(102, "media.role:event", false);
        assert!(s.on_event(&mut m, Facility::Stream, 102, |_| {}));
        assert!(m.streams[&102].muted);
        m.sink(90, "bluez_output.headset", false);
        s.on_event(&mut m, Facility::Sink, 90, |_| {});
        assert!(m.sinks[&90].muted);
        // Something else unmutes a sink: muted again.
        m.sinks.get_mut(&53).unwrap().muted = false;
        assert!(!s.on_event(&mut m, Facility::Sink, 53, |_| {}), "already recorded");
        assert!(m.sinks[&53].muted);
        s.release(&mut m, 0, |_| {}).unwrap();
        assert!(!m.streams[&102].muted && !m.sinks[&90].muted && !m.sinks[&53].muted);
        assert!(m.sinks[&69].muted && m.streams[&101].muted);
    }

    #[test]
    fn remembered_app_mutes_are_undone() {
        let mut m = server();
        let mut s = MuteState::default();
        s.engage(&mut m, |_| {}).unwrap();
        // Brave's stream ends; its next one starts muted (WirePlumber remembered).
        m.streams.remove(&100);
        m.stream(103, "application.name:Brave", true);
        s.on_event(&mut m, Facility::Stream, 103, |_| {});
        // A short sound from an app that ends while muted.
        m.stream(104, "media.role:event", false);
        s.on_event(&mut m, Facility::Stream, 104, |_| {});
        m.streams.remove(&104);
        s.release(&mut m, 1_000, |_| {}).unwrap();
        assert!(!m.streams[&103].muted, "not the user's mute");
        assert_eq!(s.pending, vec![Pending { facility: Facility::Stream, key: "media.role:event".into(), since: 1_000 }]);
        assert!(s.wants_watch());
        // Its next stream arrives muted from WirePlumber: unmuted once.
        m.stream(105, "media.role:event", true);
        assert!(s.on_event(&mut m, Facility::Stream, 105, |_| {}));
        assert!(!m.streams[&105].muted && s.pending.is_empty());
        // Later mutes of that app are the user's business.
        m.stream(106, "media.role:event", true);
        assert!(!s.on_event(&mut m, Facility::Stream, 106, |_| {}));
        assert!(m.streams[&106].muted);
    }

    #[test]
    fn unplugged_sinks_are_fixed_when_they_return() {
        let mut m = server();
        let mut s = MuteState::default();
        s.engage(&mut m, |_| {}).unwrap();
        let usb = m.sinks.remove(&53).unwrap();
        s.release(&mut m, 5, |_| {}).unwrap();
        assert_eq!(s.pending.len(), 1);
        m.sink(120, &usb.key, true);
        s.on_event(&mut m, Facility::Sink, 120, |_| {});
        assert!(!m.sinks[&120].muted);
        // Pending fix-ups expire.
        s.pending.push(Pending { facility: Facility::Sink, key: "x".into(), since: 0 });
        s.prune(PENDING_TTL.as_secs());
        assert!(s.pending.is_empty());
    }

    #[test]
    fn persists_and_reapplies_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mandara").join("audio-mute.toml");
        let mut m = server();
        let before = m.muted();
        let mut s = MuteState::default();
        s.engage(&mut m, |st| st.save(&path).unwrap()).unwrap();
        // The shell restarts: everything is muted now, the record remembers why.
        let mut again = MuteState::load(&path);
        assert_eq!(again, s);
        m.sink(91, "new_sink", false);
        again.engage(&mut m, |st| st.save(&path).unwrap()).unwrap();
        assert!(m.sinks[&91].muted);
        again.release(&mut m, 0, |st| st.save(&path).unwrap()).unwrap();
        m.sinks.remove(&91);
        assert_eq!(m.muted(), before);
        assert!(!MuteState::load(&path).enabled);
        assert_eq!(MuteState::load(&dir.path().join("missing.toml")), MuteState::default());
    }

    #[test]
    fn per_app_choices_while_on_change_only_that_app() {
        let mut m = server();
        let mut s = MuteState::default();
        s.engage(&mut m, |_| {}).unwrap();
        let snapshot = (s.sinks.clone(), s.streams.iter().filter(|x| x.index != 100 && x.index != 101).cloned().collect::<Vec<_>>());
        // You mute Brave and unmute mpv while everything is silent.
        let brave = m.streams[&100].clone();
        assert!(!s.set_own_stream_mute(&brave, true), "already muted by global mute: nothing to do now");
        let mpv = m.streams[&101].clone();
        s.set_own_stream_mute(&mpv, false);
        assert!(m.streams.values().all(|n| n.muted), "global mute stays in force");
        assert!(s.own_mute(100, &brave.key, true) && !s.own_mute(101, &mpv.key, true));
        // Toggling back and forth doesn't pile up records.
        s.set_own_stream_mute(&brave, false);
        s.set_own_stream_mute(&brave, true);
        assert_eq!(s.streams.len(), 2);
        assert_eq!((s.sinks.clone(), s.streams.iter().filter(|x| x.index != 100 && x.index != 101).cloned().collect::<Vec<_>>()), snapshot, "nothing else's snapshot changed");
        // A new stream keeps being muted while on.
        m.stream(102, "media.role:event", false);
        s.on_event(&mut m, Facility::Stream, 102, |_| {});
        assert!(m.streams[&102].muted);
        s.release(&mut m, 0, |_| {}).unwrap();
        assert!(m.streams[&100].muted, "your mute of Brave is kept");
        assert!(!m.streams[&101].muted, "your unmute of mpv applies");
        assert!(!m.streams[&102].muted && m.sinks[&69].muted && !m.sinks[&53].muted);
        assert!(s.pending.is_empty(), "Brave's mute is yours, not a WirePlumber leftover");
    }

    #[test]
    fn own_mute_follows_the_server_while_off_and_the_record_while_on() {
        let mut m = server();
        let mut s = MuteState::default();
        assert!(s.own_mute(101, "application.name:mpv", true) && !s.own_mute(100, "application.name:Brave", false));
        s.engage(&mut m, |_| {}).unwrap();
        assert!(!s.own_mute(100, "application.name:Brave", true), "muted only by global mute");
        assert!(s.own_mute(101, "application.name:mpv", true));
        // Not recorded yet: a remembered mute of an app we silenced isn't yours.
        assert!(!s.own_mute(200, "application.name:Brave", true));
        assert!(s.own_mute(201, "application.name:vlc", true));
        // An app stream the guard hasn't seen yet and you mute: muted now.
        let vlc = Node { index: 201, key: "application.name:vlc".into(), muted: false };
        m.streams.insert(201, vlc.clone());
        assert!(s.set_own_stream_mute(&vlc, true));
    }

    #[test]
    fn global_set_own_stream_mute_only_while_on() {
        let dir = tempfile::tempdir().unwrap();
        let g = GlobalMute::load(dir.path().join("audio-mute.toml"));
        let mut m = server();
        assert_eq!(g.set_own_stream_mute(&mut m, &[100], true), Ok(false), "off: caller mutes directly");
        g.set(&mut m, true).unwrap();
        assert_eq!(g.set_own_stream_mute(&mut m, &[100, 999], true), Ok(true));
        assert!(MuteState::load(&dir.path().join("audio-mute.toml")).streams.iter().any(|s| s.index == 100 && s.was_muted), "saved");
        g.set(&mut m, false).unwrap();
        assert!(m.streams[&100].muted && !m.sinks[&53].muted);
    }

    #[test]
    fn restore_keys_follow_wireplumber_order() {
        let mut props = protocol::Props::new();
        assert_eq!(restore_key(&props, "x"), "node.name:x");
        props.set(protocol::Prop::ApplicationName, CString::new("Brave").unwrap());
        assert_eq!(restore_key(&props, "x"), "application.name:Brave");
        props.set(protocol::Prop::MediaRole, CString::new("event").unwrap());
        assert_eq!(restore_key(&props, "x"), "media.role:event");
    }
}
