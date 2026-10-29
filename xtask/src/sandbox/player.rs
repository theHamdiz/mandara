//! The sandbox's demo media: an MPRIS2 player ("Corniche Radio", fictional
//! tracks with procedural album art) on the private session bus, which loops
//! the demo track into the private sound server while playing; and demo
//! notifications.

use std::collections::HashMap;
use std::process::Child;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use super::demo::{self, rgb};
use super::paint::Color;
use super::Sandbox;
use crate::headless;
use crate::proc::{self, Lifetime};
use crate::Result;

/// One fictional track.
pub struct Track {
    pub title: &'static str,
    pub artist: &'static str,
    pub album: &'static str,
    pub seconds: u64,
    /// Sky colors of its cover (top, middle, horizon).
    pub colors: [Color; 3],
}

pub const TRACKS: [Track; 3] = [
    Track { title: "Low Tide at Dusk", artist: "Harbor Lights", album: "Sea Glass", seconds: 214, colors: [rgb(58, 40, 120), rgb(238, 120, 110), rgb(255, 196, 120)] },
    Track { title: "Lanterns on the Corniche", artist: "Harbor Lights", album: "Sea Glass", seconds: 187, colors: [rgb(20, 50, 110), rgb(60, 150, 170), rgb(190, 230, 210)] },
    Track { title: "Montaza Garden Walk", artist: "The Quiet Pier", album: "Evening Tides", seconds: 241, colors: [rgb(40, 20, 70), rgb(150, 60, 140), rgb(250, 170, 150)] },
];

const BUS_NAME: &str = "org.mpris.MediaPlayer2.cornicheradio";
const PATH: &str = "/org/mpris/MediaPlayer2";
const IDENTITY: &str = "Corniche Radio";
const DESKTOP_ENTRY: &str = "org.mandara.demo.music";
const ICON: &str = "multimedia-audio-player";
/// How often the player checks for the end of a track.
const TICK: Duration = Duration::from_secs(1);
/// How often the audio loop checks the playback state.
const AUDIO_POLL: Duration = Duration::from_millis(200);
/// Time the player needs to claim its bus name (used by `audio play`).
pub const STARTUP: Duration = Duration::from_secs(1);
/// Microseconds per second (MPRIS positions and lengths).
const MICROS: f64 = 1_000_000.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    Playing,
    Paused,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Self::Playing => "Playing",
            Self::Paused => "Paused",
        }
    }
}

/// Playback state: track, status and position.
#[derive(Debug)]
struct Playback {
    index: usize,
    status: Status,
    started: Instant,
    offset: f64,
}

impl Playback {
    fn new() -> Self {
        Self { index: 0, status: Status::Playing, started: Instant::now(), offset: 0.0 }
    }

    /// Seconds into the current track.
    fn position(&self) -> f64 {
        self.offset + if self.status == Status::Playing { self.started.elapsed().as_secs_f64() } else { 0.0 }
    }

    fn seek_to(&mut self, seconds: f64) {
        self.offset = seconds.max(0.0);
        self.started = Instant::now();
    }

    fn change(&mut self, delta: isize) {
        self.index = (self.index as isize + delta).rem_euclid(TRACKS.len() as isize) as usize;
        self.seek_to(0.0);
    }

    fn set(&mut self, status: Status) {
        let at = self.position();
        self.seek_to(at);
        self.status = status;
    }
}

type Shared = Arc<Mutex<Playback>>;

fn lock(shared: &Shared) -> MutexGuard<'_, Playback> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

struct Root;

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {}
    fn quit(&self) {}
    #[zbus(property)]
    fn identity(&self) -> String {
        IDENTITY.into()
    }
    #[zbus(property)]
    fn desktop_entry(&self) -> String {
        DESKTOP_ENTRY.into()
    }
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

struct Player {
    shared: Shared,
    /// `file://` URLs of the covers, per track.
    art: Vec<String>,
}

impl Player {
    fn set_status(&self, status: Option<Status>) {
        let mut p = lock(&self.shared);
        let next = status.unwrap_or(if p.status == Status::Playing { Status::Paused } else { Status::Playing });
        p.set(next);
    }
}

fn insert(map: &mut HashMap<String, OwnedValue>, key: &str, value: Value<'_>) {
    if let Ok(v) = OwnedValue::try_from(value) {
        map.insert(key.to_string(), v);
    }
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    async fn play_pause(&self, #[zbus(signal_emitter)] e: SignalEmitter<'_>) -> zbus::fdo::Result<()> {
        self.set_status(None);
        Ok(self.playback_status_changed(&e).await?)
    }
    async fn play(&self, #[zbus(signal_emitter)] e: SignalEmitter<'_>) -> zbus::fdo::Result<()> {
        self.set_status(Some(Status::Playing));
        Ok(self.playback_status_changed(&e).await?)
    }
    async fn pause(&self, #[zbus(signal_emitter)] e: SignalEmitter<'_>) -> zbus::fdo::Result<()> {
        self.set_status(Some(Status::Paused));
        Ok(self.playback_status_changed(&e).await?)
    }
    async fn stop(&self, #[zbus(signal_emitter)] e: SignalEmitter<'_>) -> zbus::fdo::Result<()> {
        self.set_status(Some(Status::Paused));
        Ok(self.playback_status_changed(&e).await?)
    }
    async fn next(&self, #[zbus(signal_emitter)] e: SignalEmitter<'_>) -> zbus::fdo::Result<()> {
        lock(&self.shared).change(1);
        Ok(self.metadata_changed(&e).await?)
    }
    async fn previous(&self, #[zbus(signal_emitter)] e: SignalEmitter<'_>) -> zbus::fdo::Result<()> {
        lock(&self.shared).change(-1);
        Ok(self.metadata_changed(&e).await?)
    }
    async fn set_position(&self, _track_id: ObjectPath<'_>, position: i64, #[zbus(signal_emitter)] e: SignalEmitter<'_>) -> zbus::fdo::Result<()> {
        lock(&self.shared).seek_to(position as f64 / MICROS);
        Ok(Self::seeked(&e, position).await?)
    }
    async fn seek(&self, offset: i64, #[zbus(signal_emitter)] e: SignalEmitter<'_>) -> zbus::fdo::Result<()> {
        let at = {
            let mut p = lock(&self.shared);
            let to = p.position() + offset as f64 / MICROS;
            p.seek_to(to);
            p.position()
        };
        Ok(Self::seeked(&e, (at * MICROS) as i64).await?)
    }
    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> String {
        lock(&self.shared).status.as_str().into()
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        let index = lock(&self.shared).index;
        let t = &TRACKS[index];
        let mut m = HashMap::new();
        if let Ok(id) = ObjectPath::try_from(format!("/org/mandara/demo/track/{index}")) {
            insert(&mut m, "mpris:trackid", Value::from(id));
        }
        insert(&mut m, "xesam:title", Value::from(t.title));
        insert(&mut m, "xesam:artist", Value::from(vec![t.artist]));
        insert(&mut m, "xesam:album", Value::from(t.album));
        insert(&mut m, "mpris:length", Value::from((t.seconds as f64 * MICROS) as i64));
        insert(&mut m, "mpris:artUrl", Value::from(self.art[index].as_str()));
        m
    }
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        (lock(&self.shared).position() * MICROS) as i64
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn loop_status(&self) -> String {
        "None".into()
    }
    #[zbus(property)]
    fn shuffle(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

/// Runs the demo player until killed (`cargo xtask sandbox audio stop`).
pub fn run(sb: &Sandbox) -> Result<()> {
    // Only ever started by `audio play`, with the sandbox environment.
    if std::env::var_os("XDG_RUNTIME_DIR").as_deref() != Some(sb.runtime.as_os_str()) {
        return Err("refusing: the demo player runs only in the sandbox environment (cargo xtask sandbox audio play)".into());
    }
    let shared: Shared = Arc::new(Mutex::new(Playback::new()));
    let art = (0..TRACKS.len()).map(|i| format!("file://{}", demo::cover_path(sb, i).display())).collect();
    let player = Player { shared: shared.clone(), art };
    let address = headless::bus_address(&sb.runtime);
    let conn = zbus::block_on(async { zbus::connection::Builder::address(address.as_str())?.name(BUS_NAME)?.serve_at(PATH, Root)?.serve_at(PATH, player)?.build().await })
        .map_err(|e| format!("demo player: {e}"))?;

    let (audio, sandbox) = (shared.clone(), sb.clone());
    std::thread::spawn(move || audio_loop(&audio, &sandbox));

    loop {
        std::thread::sleep(TICK);
        let ended = {
            let mut p = lock(&shared);
            let over = p.position() >= TRACKS[p.index].seconds as f64;
            if over {
                p.change(1);
            }
            over
        };
        if ended {
            let emitted: zbus::Result<()> = zbus::block_on(async {
                let iface = conn.object_server().interface::<_, Player>(PATH).await?;
                iface.get().await.metadata_changed(iface.signal_emitter()).await
            });
            if let Err(e) = emitted {
                eprintln!("demo player: {e}");
            }
        }
    }
}

/// Loops the demo track while playing; stops it while paused.
fn audio_loop(shared: &Shared, sb: &Sandbox) {
    let (wav, log) = (demo::track_path(sb), sb.logs().join("paplay.log"));
    let mut child: Option<Child> = None;
    loop {
        let (playing, title) = {
            let p = lock(shared);
            (p.status == Status::Playing, TRACKS[p.index].title)
        };
        match (child.as_mut(), playing) {
            (Some(c), true) => {
                if !matches!(c.try_wait(), Ok(None)) {
                    child = None;
                }
            }
            (None, true) => {
                let mut cmd = sb.command("paplay");
                cmd.args([
                    format!("--client-name={IDENTITY}"),
                    format!("--stream-name={title}"),
                    format!("--property=application.name={IDENTITY}"),
                    format!("--property=application.icon_name={ICON}"),
                    format!("--property=application.id={DESKTOP_ENTRY}"),
                ])
                .arg(&wav);
                match proc::spawn(&mut cmd, &log, Lifetime::Bound) {
                    Ok(c) => child = Some(c),
                    Err(e) => eprintln!("demo player: {e}"),
                }
            }
            (Some(c), false) => {
                proc::stop_child(c);
                child = None;
            }
            (None, false) => {}
        }
        std::thread::sleep(AUDIO_POLL);
    }
}

/// A demo notification.
struct Note {
    app: &'static str,
    icon: &'static str,
    summary: &'static str,
    body: &'static str,
    /// Action key / label pairs, flattened.
    actions: &'static [&'static str],
    desktop_entry: &'static str,
}

const NOTES: [Note; 3] = [
    Note { app: "Calendar", icon: "office-calendar", summary: "Design review in 15 minutes", body: "Studio · Corniche room — bring the new palette boards", actions: &[], desktop_entry: "org.mandara.demo.calendar" },
    Note { app: IDENTITY, icon: ICON, summary: "Now playing", body: "Low Tide at Dusk — Harbor Lights", actions: &[], desktop_entry: DESKTOP_ENTRY },
    Note {
        app: "Mail",
        icon: "internet-mail",
        summary: "Mariam Adel",
        body: "The lighthouse photos look great — can we use the third one for the cover?",
        actions: &["reply", "Reply", "archive", "Archive"],
        desktop_entry: "org.mandara.demo.mail",
    },
];
/// Pause between demo notifications.
const NOTE_PAUSE: Duration = Duration::from_millis(400);
/// `expire_timeout` for the server's default.
const DEFAULT_TIMEOUT: i32 = -1;

/// Sends the demo notifications on the sandbox session bus.
pub fn notify(sb: &Sandbox) -> Result<()> {
    let address = headless::bus_address(&sb.runtime);
    zbus::block_on(async {
        let conn = zbus::connection::Builder::address(address.as_str())?.build().await?;
        for (i, n) in NOTES.iter().enumerate() {
            if i > 0 {
                std::thread::sleep(NOTE_PAUSE);
            }
            let hints: HashMap<&str, Value<'_>> = HashMap::from([("desktop-entry", Value::from(n.desktop_entry))]);
            conn.call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "Notify",
                &(n.app, 0u32, n.icon, n.summary, n.body, n.actions.to_vec(), hints, DEFAULT_TIMEOUT),
            )
            .await?;
        }
        Ok::<(), zbus::Error>(())
    })
    .map_err(|e| format!("notify: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_wrap_both_ways() {
        let mut p = Playback::new();
        p.change(-1);
        assert_eq!(p.index, TRACKS.len() - 1);
        p.change(1);
        assert_eq!(p.index, 0);
    }

    #[test]
    fn pausing_freezes_the_position() {
        let mut p = Playback::new();
        p.seek_to(10.0);
        p.set(Status::Paused);
        let at = p.position();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(p.position(), at);
        assert!(at >= 10.0);
    }
}
