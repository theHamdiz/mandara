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

/// Handle to the running media actor.
#[derive(Clone)]
pub struct MediaService {
    pub updates: async_channel::Receiver<Option<MediaState>>,
    commands: async_channel::Sender<MediaCommand>,
}

impl MediaService {
    pub fn send(&self, command: MediaCommand) {
        if self.commands.try_send(command).is_err() {
            tracing::warn!("media actor is not running");
        }
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

async fn run(updates: async_channel::Sender<Option<MediaState>>, commands: async_channel::Receiver<MediaCommand>) -> Result<()> {
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

    // Every wake-up (signal, art ready, tick or command) re-reads the state.
    loop {
        let state = snapshot(&conn, &dbus, &mut followed, &art, &art_ready_tx).await;
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
                let Ok(command) = command else { return Ok(()) };
                if command == MediaCommand::Refresh {
                    continue;
                }
                if let (Some(bus), Some(state)) = (followed.clone(), last.clone()) {
                    if let Err(err) = execute(&conn, &bus, &state, command).await {
                        tracing::warn!("MPRIS command failed: {err:#}");
                    }
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
    read_state(conn, &bus, art, art_ready).await.ok()
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

async fn read_state(conn: &Connection, bus: &str, art: &ArtCache, art_ready: &async_channel::Sender<()>) -> Result<MediaState> {
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
    let path = dir.join(format!("{:016x}", hasher.finish()));
    if path.exists() {
        return Some(path);
    }
    let result = (|| -> Result<()> {
        std::fs::create_dir_all(&dir)?;
        let bytes = ureq::get(url).call()?.body_mut().with_config().limit(ART_MAX_BYTES).read_to_vec()?;
        // Validate the header (format + size) without decoding the pixels.
        image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format()?.into_dimensions()?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    })();
    match result {
        Ok(()) => Some(path),
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

    fn ov(v: Value<'_>) -> OwnedValue {
        v.try_to_owned().unwrap()
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
            track_id: None,
        };
        assert_eq!(state.position_now(t0 + Duration::from_secs(5)), Duration::from_secs(15));
        assert!((state.progress(t0 + Duration::from_secs(40)) - 0.5).abs() < 1e-4);
        let paused = MediaState { status: PlaybackStatus::Paused, ..state };
        assert_eq!(paused.position_now(t0 + Duration::from_secs(5)), Duration::from_secs(10));
    }
}
