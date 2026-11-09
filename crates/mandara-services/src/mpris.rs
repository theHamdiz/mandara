//! MPRIS2 media actor.
//!
//! Watches every `org.mpris.MediaPlayer2.*` name on the session bus, follows
//! the most relevant player (the most recently playing one), and publishes a
//! [`MediaState`] snapshot whenever anything changes (`PropertiesChanged`,
//! `Seeked`, players appearing or leaving). Progress is extrapolated by the
//! UI from `position` + `position_at` + `rate`; while playing the position
//! is re-read every [`RESYNC_PLAYING`] to correct drift. Album art is
//! resolved (`file://` paths directly, `http(s)` downloaded into
//! `~/.cache/mandara/art/`) and quantized into seed colors for the Media
//! Island's ambient glow; at most [`ART_CACHE_CAPACITY`] URLs are kept.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use futures::{FutureExt as _, StreamExt as _, select};
use mandara_theme::SeedColor;
use parking_lot::Mutex;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};
use zbus::{Connection, MatchRule, MessageStream, proxy};

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
/// Position re-read while playing (the UI extrapolates in between).
const RESYNC_PLAYING: Duration = Duration::from_secs(5);
/// Fallback re-read while nothing plays (signals cover real changes).
const RESYNC_IDLE: Duration = Duration::from_secs(30);
/// Snapshots buffered for the UI.
const UPDATE_QUEUE: usize = 16;
/// Commands buffered from the UI and CLI.
const COMMAND_QUEUE: usize = 32;
/// Queued `PropertiesChanged` / `Seeked` and `NameOwnerChanged` signals.
const PROPS_QUEUE: usize = 64;
const OWNER_QUEUE: usize = 16;
/// Album-art URLs remembered (path + seed colors); the oldest is evicted.
pub const ART_CACHE_CAPACITY: usize = 64;
/// Seed colors extracted from album art.
const ART_SEEDS: usize = 4;
/// Largest album-art download accepted, bytes.
const ART_MAX_BYTES: u64 = 16 * 1024 * 1024;

#[proxy(interface = "org.mpris.MediaPlayer2", default_path = "/org/mpris/MediaPlayer2")]
trait MediaPlayer2 {
    #[zbus(property)]
    fn identity(&self) -> zbus::Result<String>;
}

#[proxy(interface = "org.mpris.MediaPlayer2.Player", default_path = "/org/mpris/MediaPlayer2")]
trait Player {
    fn play_pause(&self) -> zbus::Result<()>;
    fn next(&self) -> zbus::Result<()>;
    fn previous(&self) -> zbus::Result<()>;
    fn set_position(&self, track_id: &ObjectPath<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn metadata(&self) -> zbus::Result<HashMap<String, OwnedValue>>;
    #[zbus(property)]
    fn position(&self) -> zbus::Result<i64>;
    #[zbus(property)]
    fn rate(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn can_go_next(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn can_go_previous(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn can_seek(&self) -> zbus::Result<bool>;
    /// Whether the player accepts control at all (MPRIS `CanControl`).
    #[zbus(property)]
    fn can_control(&self) -> zbus::Result<bool>;
    /// The player's own volume, 0.0–1.0 (MPRIS `Volume`).
    #[zbus(property)]
    fn volume(&self) -> zbus::Result<f64>;
    /// Sets the player's own volume (MPRIS `Volume`), not its audio stream.
    #[zbus(property)]
    fn set_volume(&self, volume: f64) -> zbus::Result<()>;
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

impl PlaybackStatus {
    fn parse(s: &str) -> Self {
        match s {
            "Playing" => PlaybackStatus::Playing,
            "Paused" => PlaybackStatus::Paused,
            _ => PlaybackStatus::Stopped,
        }
    }
}

/// Snapshot of the followed player.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaState {
    pub bus_name: String,
    pub identity: String,
    pub status: PlaybackStatus,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art_url: Option<String>,
    /// Local file for the album art, once resolved.
    pub art_path: Option<PathBuf>,
    /// Ranked album-art clusters (empty until extracted).
    pub art_seeds: Vec<SeedColor>,
    pub length: Option<Duration>,
    /// Position at `position_at`; interpolate with [`MediaState::position_now`].
    pub position: Duration,
    pub position_at: Instant,
    pub rate: f64,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    pub can_seek: bool,
    /// The player's own volume (`0..=1`, its `Volume`) when you can change
    /// it: the player accepts control and declares `Volume` writable. A
    /// player that keeps its own level puts it back on its sound stream
    /// whenever it plays or pauses, so this, not the stream's volume, is
    /// the level that lasts ([`MediaCommand::SetVolume`]).
    pub volume: Option<f64>,
    track_id: Option<String>,
}

/// A stopped player with nothing loaded (tests and previews build on it).
impl Default for MediaState {
    fn default() -> Self {
        Self {
            bus_name: String::new(),
            identity: String::new(),
            status: PlaybackStatus::Stopped,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            art_url: None,
            art_path: None,
            art_seeds: Vec::new(),
            length: None,
            position: Duration::ZERO,
            position_at: Instant::now(),
            rate: 1.0,
            can_go_next: false,
            can_go_previous: false,
            can_seek: false,
            volume: None,
            track_id: None,
        }
    }
}

impl MediaState {
    /// Playback position extrapolated to `now`.
    pub fn position_now(&self, now: Instant) -> Duration {
        let mut pos = self.position;
        if self.status == PlaybackStatus::Playing {
            pos += now.saturating_duration_since(self.position_at).mul_f64(self.rate.max(0.0));
        }
        match self.length {
            Some(len) => pos.min(len),
            None => pos,
        }
    }

    /// A live stream (internet radio, a live broadcast): a player that
    /// reports no length and can't seek, so there is no progress to show
    /// and its position only counts time since it tuned in. Views show
    /// "Live" instead of a progress line and times.
    pub fn is_live(&self) -> bool {
        !self.bus_name.is_empty() && self.length.is_none() && !self.can_seek
    }

    /// Progress `0..=1` (0 when the length is unknown).
    pub fn progress(&self, now: Instant) -> f32 {
        match self.length {
            Some(len) if !len.is_zero() => (self.position_now(now).as_secs_f32() / len.as_secs_f32()).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }

    /// Equality ignoring the moving position (used to suppress redundant
    /// updates while paused).
    fn same_track_state(&self, other: &MediaState) -> bool {
        self.bus_name == other.bus_name
            && self.status == other.status
            && self.title == other.title
            && self.artist == other.artist
            && self.art_path == other.art_path
            && self.art_seeds.len() == other.art_seeds.len()
            && self.length == other.length
            && self.volume == other.volume
    }
}

/// Commands accepted by the media actor.
#[derive(Clone, Debug, PartialEq)]
pub enum MediaCommand {
    PlayPause,
    Next,
    Previous,
    /// Seek to an absolute position.
    SeekTo(Duration),
    /// Sets the player's own volume (`0..=1`; only a player whose
    /// [`MediaState::volume`] is known is sent one).
    SetVolume(f64),
    /// Re-pick the followed player (after [`configure`]).
    Refresh,
}

/// `[media]` preferences the actor follows (ignored players, following
/// whichever starts playing).
fn prefs() -> &'static Mutex<mandara_config::alerts::MediaConfig> {
    static PREFS: std::sync::OnceLock<Mutex<mandara_config::alerts::MediaConfig>> = std::sync::OnceLock::new();
    PREFS.get_or_init(Default::default)
}

/// Applies `[media]`; send [`MediaCommand::Refresh`] afterwards to re-pick.
pub fn configure(media: &mandara_config::alerts::MediaConfig) {
    *prefs().lock() = media.clone();
}

/// A command and, for [`MediaService::request`], where its result goes.
type Envelope = (MediaCommand, Option<async_channel::Sender<Result<String, String>>>);

/// Handle to the running media actor.
#[derive(Clone)]
pub struct MediaService {
    pub updates: async_channel::Receiver<Option<MediaState>>,
    commands: async_channel::Sender<Envelope>,
}

impl MediaService {
    pub fn send(&self, command: MediaCommand) {
        if self.commands.try_send((command, None)).is_err() {
            tracing::warn!("media actor is not running");
        }
    }

    /// Sends `command` and yields once the player has been told: a
    /// slider's write, so it keeps one write in flight at a time.
    pub fn request(&self, command: MediaCommand) -> async_channel::Receiver<Result<String, String>> {
        let (tx, rx) = async_channel::bounded(1);
        if self.commands.try_send((command, Some(tx))).is_err() {
            tracing::warn!("media actor is not running");
        }
        rx
    }
}

/// Spawns the actor on the shared service runtime.
pub fn spawn_media_service() -> MediaService {
    let (update_tx, updates) = async_channel::bounded(UPDATE_QUEUE);
    let (commands, command_rx) = async_channel::bounded(COMMAND_QUEUE);
    crate::runtime::spawn("mpris", async move {
        if let Err(err) = run(update_tx, command_rx).await {
            tracing::error!("MPRIS actor stopped: {err:#}");
        }
    });
    MediaService { updates, commands }
}

/// Resolved album art shared between the actor and download threads. An
/// entry with no path is either in flight or failed; either way it is not
/// re-fetched.
#[derive(Clone, Default)]
struct ArtEntry {
    path: Option<PathBuf>,
    seeds: Vec<SeedColor>,
}

/// Resolved art by URL, plus insertion order for eviction.
#[derive(Default)]
struct ArtMap {
    entries: HashMap<String, ArtEntry>,
    order: std::collections::VecDeque<String>,
}

impl ArtMap {
    fn insert(&mut self, url: String, entry: ArtEntry) {
        if self.entries.insert(url.clone(), entry).is_none() {
            self.order.push_back(url);
            while self.order.len() > ART_CACHE_CAPACITY {
                if let Some(old) = self.order.pop_front() {
                    self.entries.remove(&old);
                }
            }
        }
    }
}

type ArtCache = Arc<Mutex<ArtMap>>;

/// The MPRIS actor: follows players on its own session connection, sends
/// each new state of the followed player on `updates` and runs `commands`
/// against it until either channel closes.
async fn run(updates: async_channel::Sender<Option<MediaState>>, commands: async_channel::Receiver<Envelope>) -> Result<()> {
    // Private connection: its signal streams must never stall other actors.
    let conn = crate::runtime::connect(zbus::connection::Builder::session()?).await?;
    let dbus = zbus::fdo::DBusProxy::new(&conn).await?;

    let props_rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.DBus.Properties")?
        .member("PropertiesChanged")?
        .path(MPRIS_PATH)?
        .build();
    let seeked_rule = MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(PLAYER_IFACE)?.member("Seeked")?.path(MPRIS_PATH)?.build();
    let owner_rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")?
        .interface("org.freedesktop.DBus")?
        .member("NameOwnerChanged")?
        .arg0ns("org.mpris.MediaPlayer2")?
        .build();
    let mut props = MessageStream::for_match_rule(props_rule, &conn, Some(PROPS_QUEUE)).await?.fuse();
    let mut seeks = MessageStream::for_match_rule(seeked_rule, &conn, Some(PROPS_QUEUE)).await?.fuse();
    let mut owners = MessageStream::for_match_rule(owner_rule, &conn, Some(OWNER_QUEUE)).await?.fuse();

    let art: ArtCache = Arc::default();
    // One pending "art ready" wake-up is enough (it triggers a re-read).
    let (art_ready_tx, art_ready) = async_channel::bounded::<()>(1);
    let mut followed: Option<String> = None;
    let mut last: Option<MediaState> = None;
    // Whether the followed player declares `Volume` writable (introspected
    // once per player it follows).
    let mut writable: Option<(String, bool)> = None;

    // Every wake-up (signal, art ready, tick or command) re-reads the state.
    loop {
        let state = snapshot(&conn, &dbus, &mut followed, &art, &art_ready_tx, &mut writable).await;
        let changed = match (&last, &state) {
            (Some(a), Some(b)) => !a.same_track_state(b) || b.status == PlaybackStatus::Playing,
            (None, None) => false,
            _ => true,
        };
        if changed {
            if updates.send(state.clone()).await.is_err() {
                return Ok(());
            }
            last = state;
        }
        let playing = last.as_ref().is_some_and(|s| s.status == PlaybackStatus::Playing);
        let tick = async_io::Timer::after(if playing { RESYNC_PLAYING } else { RESYNC_IDLE });
        select! {
            _ = props.next() => {}
            _ = seeks.next() => {}
            _ = owners.next() => {}
            _ = art_ready.recv().fuse() => {}
            _ = futures::FutureExt::fuse(tick) => {}
            command = commands.recv().fuse() => {
                let Ok((command, reply)) = command else { return Ok(()) };
                let result = match (command, followed.as_deref(), last.as_ref()) {
                    (MediaCommand::Refresh, _, _) => Ok(()),
                    (command, Some(bus), Some(state)) => execute(&conn, bus, state, command).await.map_err(|err| {
                        tracing::warn!("MPRIS command failed: {err:#}");
                        err.to_string()
                    }),
                    _ => Err("no media player".to_string()),
                };
                if let Some(reply) = reply {
                    let _ = reply.try_send(result.map(|()| String::new()));
                }
            }
        }
    }
}

async fn player(conn: &Connection, bus: &str) -> zbus::Result<PlayerProxy<'static>> {
    PlayerProxy::builder(conn)
        .destination(bus.to_owned())?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
}

async fn execute(conn: &Connection, bus: &str, state: &MediaState, command: MediaCommand) -> zbus::Result<()> {
    let p = player(conn, bus).await?;
    match command {
        MediaCommand::PlayPause => p.play_pause().await,
        MediaCommand::Next => p.next().await,
        MediaCommand::Previous => p.previous().await,
        MediaCommand::Refresh => Ok(()),
        MediaCommand::SetVolume(v) if state.volume.is_some() => p.set_volume(v.clamp(0.0, 1.0)).await,
        MediaCommand::SetVolume(_) => Ok(()),
        MediaCommand::SeekTo(pos) => {
            let Some(track) = &state.track_id else { return Ok(()) };
            let path = ObjectPath::try_from(track.as_str())?;
            p.set_position(&path, i64::try_from(pos.as_micros()).unwrap_or(i64::MAX)).await
        }
    }
}

/// Picks the followed player and reads its full state.
async fn snapshot(
    conn: &Connection,
    dbus: &zbus::fdo::DBusProxy<'_>,
    followed: &mut Option<String>,
    art: &ArtCache,
    art_ready: &async_channel::Sender<()>,
    writable: &mut Option<(String, bool)>,
) -> Option<MediaState> {
    let names: Vec<String> = dbus
        .list_names()
        .await
        .ok()?
        .into_iter()
        .map(|n| n.to_string())
        .filter(|n| n.starts_with(MPRIS_PREFIX))
        .collect();
    let prefs = prefs().lock().clone();
    // Ignored players: matched by bus name here, by identity below.
    let mut names = names;
    if !prefs.ignore_players.is_empty() {
        let mut kept = Vec::with_capacity(names.len());
        for name in names {
            let identity = identity_of(conn, &name).await;
            if !prefs.ignores(&identity, &name) {
                kept.push(name);
            }
        }
        names = kept;
    }
    if names.is_empty() {
        *followed = None;
        return None;
    }
    // Prefer a playing player; stick with the current one otherwise.
    let mut statuses = Vec::with_capacity(names.len());
    for name in &names {
        let status = match player(conn, name).await {
            Ok(p) => p.playback_status().await.map(|s| PlaybackStatus::parse(&s)).unwrap_or(PlaybackStatus::Stopped),
            Err(_) => PlaybackStatus::Stopped,
        };
        statuses.push((name.clone(), status));
    }
    let current_playing = followed
        .as_ref()
        .and_then(|f| statuses.iter().find(|(n, _)| n == f))
        .is_some_and(|(_, s)| *s == PlaybackStatus::Playing);
    let current_valid = followed.as_ref().is_some_and(|f| names.contains(f));
    if !current_playing && !(current_valid && !prefs.follow_playing) {
        if let Some((name, _)) = statuses.iter().find(|(_, s)| *s == PlaybackStatus::Playing) {
            *followed = Some(name.clone());
        } else if followed.as_ref().is_none_or(|f| !names.contains(f)) {
            let pick = statuses
                .iter()
                .find(|(_, s)| *s == PlaybackStatus::Paused)
                .or_else(|| statuses.first())
                .map(|(n, _)| n.clone());
            *followed = pick;
        }
    }
    let bus = followed.clone()?;
    if writable.as_ref().is_none_or(|(b, _)| *b != bus) {
        let xml = match zbus::fdo::IntrospectableProxy::builder(conn).destination(bus.clone()).and_then(|b| b.path(MPRIS_PATH)) {
            Ok(b) => match b.cache_properties(zbus::proxy::CacheProperties::No).build().await {
                Ok(proxy) => proxy.introspect().await.unwrap_or_default(),
                Err(_) => String::new(),
            },
            Err(_) => String::new(),
        };
        *writable = Some((bus.clone(), volume_writable(&xml)));
    }
    let volume_writable = writable.as_ref().is_some_and(|(_, w)| *w);
    read_state(conn, &bus, art, art_ready, volume_writable).await.ok()
}

/// Whether introspection `xml` declares the player interface's `Volume`
/// writable (some players report a read-only constant instead).
pub fn volume_writable(xml: &str) -> bool {
    // An element's attribute (either quote style) within its opening tag.
    fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
        let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
        tag.split_whitespace().find_map(|a| a.strip_prefix(name)?.strip_prefix('=')).and_then(|v| v.get(1..)).and_then(|v| v.split(['"', '\'']).next())
    }
    xml.split("<interface").skip(1).filter(|i| attr(i, "name") == Some(PLAYER_IFACE)).any(|iface| {
        let iface = &iface[..iface.find("</interface>").unwrap_or(iface.len())];
        iface.split("<property").skip(1).any(|p| attr(p, "name") == Some("Volume") && attr(p, "access") == Some("readwrite"))
    })
}

/// The volume of a player whose `Volume` is writable, when you can change
/// it: the player accepts control and reports a level.
pub fn player_volume(can_control: bool, volume: Option<f64>) -> Option<f64> {
    volume.filter(|v| can_control && v.is_finite()).map(|v| v.max(0.0))
}

/// A player's `Identity` (empty when it doesn't answer).
async fn identity_of(conn: &Connection, bus: &str) -> String {
    match MediaPlayer2Proxy::builder(conn).destination(bus.to_owned()) {
        Ok(b) => match b.cache_properties(zbus::proxy::CacheProperties::No).build().await {
            Ok(mp) => mp.identity().await.unwrap_or_default(),
            Err(_) => String::new(),
        },
        Err(_) => String::new(),
    }
}

/// Reads `bus`'s current media state. Album art is loaded through `art`
/// and announced on `art_ready`; the player's own volume is included only
/// when `volume_writable` (checked once per player).
async fn read_state(conn: &Connection, bus: &str, art: &ArtCache, art_ready: &async_channel::Sender<()>, volume_writable: bool) -> Result<MediaState> {
    let p = player(conn, bus).await?;
    let identity = match MediaPlayer2Proxy::builder(conn)
        .destination(bus.to_owned())?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
    {
        Ok(mp) => mp.identity().await.unwrap_or_default(),
        Err(_) => String::new(),
    };
    let identity = if identity.is_empty() { bus.trim_start_matches(MPRIS_PREFIX).to_string() } else { identity };
    let status = PlaybackStatus::parse(&p.playback_status().await.unwrap_or_default());
    let meta = p.metadata().await.unwrap_or_default();
    let meta = Metadata::parse(&meta);
    let position = Duration::from_micros(p.position().await.unwrap_or(0).max(0) as u64);

    let (art_path, art_seeds) = match &meta.art_url {
        Some(url) => resolve_art(url, art, art_ready),
        None => (None, Vec::new()),
    };
    Ok(MediaState {
        bus_name: bus.to_string(),
        identity,
        status,
        title: meta.title,
        artist: meta.artist,
        album: meta.album,
        art_url: meta.art_url,
        art_path,
        art_seeds,
        length: meta.length,
        position,
        position_at: Instant::now(),
        rate: p.rate().await.unwrap_or(1.0),
        can_go_next: p.can_go_next().await.unwrap_or(false),
        can_go_previous: p.can_go_previous().await.unwrap_or(false),
        can_seek: p.can_seek().await.unwrap_or(false),
        // A read-only `Volume` isn't worth the two calls.
        volume: if volume_writable { player_volume(p.can_control().await.unwrap_or(false), p.volume().await.ok()) } else { None },
        track_id: meta.track_id,
    })
}

/// Parsed `Metadata` dictionary.
#[derive(Debug, Default, PartialEq)]
pub struct Metadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art_url: Option<String>,
    pub length: Option<Duration>,
    pub track_id: Option<String>,
}

impl Metadata {
    pub fn parse(map: &HashMap<String, OwnedValue>) -> Self {
        let string = |key: &str| -> Option<String> { map.get(key).and_then(|v| String::try_from(v.try_clone().ok()?).ok()) };
        let artist = map
            .get("xesam:artist")
            .and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok())
            .map(|a| a.join(", "))
            .or_else(|| string("xesam:artist"))
            .unwrap_or_default();
        let length = map.get("mpris:length").and_then(|v| {
            let v = v.try_clone().ok()?;
            i64::try_from(v.try_clone().ok()?)
                .ok()
                .or_else(|| u64::try_from(v).ok().map(|u| u as i64))
        });
        let track_id = map
            .get("mpris:trackid")
            .and_then(|v| OwnedObjectPath::try_from(v.try_clone().ok()?).ok())
            .map(|p| p.to_string())
            .or_else(|| string("mpris:trackid"));
        Self {
            title: string("xesam:title").unwrap_or_default(),
            artist,
            album: string("xesam:album").unwrap_or_default(),
            art_url: string("mpris:artUrl").filter(|u| !u.is_empty()),
            length: length.filter(|l| *l > 0).map(|l| Duration::from_micros(l as u64)),
            track_id,
        }
    }
}

/// Decodes `%XX` escapes in a `file://` URL path.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn art_cache_dir() -> PathBuf {
    mandara_config::paths::cache_dir().join("art")
}

/// Returns resolved art for `url`, kicking off a background download /
/// extraction the first time it is seen.
fn resolve_art(url: &str, cache: &ArtCache, ready: &async_channel::Sender<()>) -> (Option<PathBuf>, Vec<SeedColor>) {
    {
        let mut map = cache.lock();
        if let Some(entry) = map.entries.get(url) {
            return (entry.path.clone(), entry.seeds.clone());
        }
        // In flight: not re-fetched while the download runs.
        map.insert(url.to_string(), ArtEntry::default());
    }
    let (url, cache, ready) = (url.to_string(), cache.clone(), ready.clone());
    crate::runtime::spawn("mpris-art", async move {
        let entry = crate::runtime::unblock(move || {
            let path = fetch_art(&url);
            let seeds = path.as_ref().and_then(|p| mandara_theme::material::extract_seeds(p, ART_SEEDS).ok()).unwrap_or_default();
            (url, ArtEntry { path, seeds })
        })
        .await;
        let (url, entry) = entry;
        cache.lock().insert(url, entry);
        let _ = ready.try_send(());
    });
    (None, Vec::new())
}

/// Extensions cached album art is saved under.
const ART_EXTENSIONS: [&str; 5] = ["jpg", "png", "webp", "gif", "img"];

/// The cache extension for a decoded header's format (`img` when unknown).
fn art_extension(format: Option<image::ImageFormat>) -> &'static str {
    match format {
        Some(image::ImageFormat::Jpeg) => "jpg",
        Some(image::ImageFormat::Png) => "png",
        Some(image::ImageFormat::WebP) => "webp",
        Some(image::ImageFormat::Gif) => "gif",
        _ => "img",
    }
}

fn fetch_art(url: &str) -> Option<PathBuf> {
    if let Some(path) = url.strip_prefix("file://") {
        let path = PathBuf::from(percent_decode(path));
        return path.exists().then_some(path);
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) || crate::offline::is_offline() {
        return None;
    }
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    let dir = art_cache_dir();
    let stem = format!("{:016x}", hasher.finish());
    // Cached as `<hash>.<ext>` (the format's usual extension), so anything
    // that goes by the name can read it too.
    if let Some(cached) = ART_EXTENSIONS.iter().map(|ext| dir.join(format!("{stem}.{ext}"))).find(|p| p.exists()) {
        return Some(cached);
    }
    let result = (|| -> Result<PathBuf> {
        std::fs::create_dir_all(&dir)?;
        let bytes = ureq::get(url).call()?.body_mut().with_config().limit(ART_MAX_BYTES).read_to_vec()?;
        // Validate the header (format + size) without decoding the pixels.
        let reader = image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format()?;
        let ext = art_extension(reader.format());
        reader.into_dimensions()?;
        let path = dir.join(format!("{stem}.{ext}"));
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)?;
        Ok(path)
    })();
    match result {
        Ok(path) => Some(path),
        Err(err) => {
            tracing::debug!("album art download failed for {url}: {err:#}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    #[test]
    fn cached_art_gets_its_formats_extension() {
        assert_eq!(art_extension(Some(image::ImageFormat::Jpeg)), "jpg");
        assert_eq!(art_extension(Some(image::ImageFormat::WebP)), "webp");
        assert_eq!(art_extension(None), "img");
        assert!(ART_EXTENSIONS.contains(&art_extension(Some(image::ImageFormat::Png))));
    }

    fn ov(v: Value<'_>) -> OwnedValue {
        v.try_to_owned().unwrap()
    }

    /// Introspection as a player answers it (trimmed): `Volume` writable.
    const RW_VOLUME: &str = r#"<node><interface name="org.mpris.MediaPlayer2"><property name="Identity" type="s" access="read"/></interface>
<interface name="org.mpris.MediaPlayer2.Player"><method name="PlayPause"/><property name="PlaybackStatus" type="s" access="read"/>
<property type="d" name="Volume" access="readwrite"><annotation name="org.freedesktop.DBus.Property.EmitsChangedSignal" value="true"/></property>
<property name="CanControl" type="b" access="read"/></interface></node>"#;

    #[test]
    fn a_player_volume_counts_only_when_declared_writable() {
        assert!(volume_writable(RW_VOLUME));
        assert!(!volume_writable(&RW_VOLUME.replace(r#"access="readwrite""#, r#"access="read""#)), "a read-only constant");
        assert!(!volume_writable(&RW_VOLUME.replace(r#"name="Volume""#, r#"name="Rate""#)), "no Volume at all");
        assert!(!volume_writable(&RW_VOLUME.replace("org.mpris.MediaPlayer2.Player", "org.example.Other")), "only the player interface's");
        assert!(!volume_writable(""), "no answer");
        assert!(volume_writable("<interface name='org.mpris.MediaPlayer2.Player'><property access='readwrite' name='Volume' type='d'/></interface>"), "single quotes");
    }

    #[test]
    fn a_player_volume_needs_control_and_a_finite_level() {
        assert_eq!(player_volume(true, Some(0.653)), Some(0.653));
        assert_eq!(player_volume(false, Some(0.653)), None, "CanControl false: hands off");
        assert_eq!(player_volume(true, None), None);
        assert_eq!(player_volume(true, Some(f64::NAN)), None);
        assert_eq!(player_volume(true, Some(-0.2)), Some(0.0));
    }

    #[test]
    fn parses_metadata() {
        let mut map = HashMap::new();
        map.insert("xesam:title".to_string(), ov(Value::from("Ya Mustafa")));
        map.insert("xesam:artist".to_string(), ov(Value::from(vec!["Bob Azzam", "Orchestra"])));
        map.insert("xesam:album".to_string(), ov(Value::from("Alexandria 1960")));
        map.insert("mpris:length".to_string(), ov(Value::from(215_000_000_i64)));
        map.insert("mpris:artUrl".to_string(), ov(Value::from("file:///tmp/cover%20art.png")));
        let m = Metadata::parse(&map);
        assert_eq!(m.title, "Ya Mustafa");
        assert_eq!(m.artist, "Bob Azzam, Orchestra");
        assert_eq!(m.album, "Alexandria 1960");
        assert_eq!(m.length, Some(Duration::from_secs(215)));
        assert_eq!(m.art_url.as_deref(), Some("file:///tmp/cover%20art.png"));
    }

    #[test]
    fn unsigned_length_and_missing_fields() {
        let mut map = HashMap::new();
        map.insert("mpris:length".to_string(), ov(Value::from(5_000_000_u64)));
        let m = Metadata::parse(&map);
        assert_eq!(m.length, Some(Duration::from_secs(5)));
        assert_eq!(m.title, "");
        assert!(m.art_url.is_none());
    }

    #[test]
    fn decodes_file_urls() {
        assert_eq!(percent_decode("/home/u/My%20Music/a%2Bb.jpg"), "/home/u/My Music/a+b.jpg");
        assert_eq!(percent_decode("/plain/path"), "/plain/path");
        assert_eq!(percent_decode("/bad%zzescape%"), "/bad%zzescape%");
    }

    #[test]
    fn position_extrapolates_while_playing() {
        let t0 = Instant::now();
        let state = MediaState {
            bus_name: "x".into(),
            identity: "x".into(),
            status: PlaybackStatus::Playing,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            art_url: None,
            art_path: None,
            art_seeds: Vec::new(),
            length: Some(Duration::from_secs(100)),
            position: Duration::from_secs(10),
            position_at: t0,
            rate: 1.0,
            can_go_next: true,
            can_go_previous: true,
            can_seek: true,
            volume: None,
            track_id: None,
        };
        assert_eq!(state.position_now(t0 + Duration::from_secs(5)), Duration::from_secs(15));
        assert!((state.progress(t0 + Duration::from_secs(40)) - 0.5).abs() < 1e-4);
        let paused = MediaState { status: PlaybackStatus::Paused, ..state };
        assert_eq!(paused.position_now(t0 + Duration::from_secs(5)), Duration::from_secs(10));
    }

    /// A stream with no length that can't seek is live; a track with a
    /// length, a seekable player still loading its length, and no player
    /// at all are not.
    #[test]
    fn live_streams_have_no_length_and_no_seeking() {
        let radio = MediaState { bus_name: "org.mpris.MediaPlayer2.mandara.radio".into(), status: PlaybackStatus::Playing, ..MediaState::default() };
        assert!(radio.is_live());
        assert_eq!(radio.progress(Instant::now() + Duration::from_secs(30)), 0.0);
        assert!(!MediaState { length: Some(Duration::from_secs(200)), ..radio.clone() }.is_live());
        assert!(!MediaState { can_seek: true, ..radio.clone() }.is_live());
        assert!(!MediaState::default().is_live(), "nothing playing is not live");
    }
}
