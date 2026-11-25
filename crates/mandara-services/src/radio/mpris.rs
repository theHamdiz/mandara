//! The radio as an MPRIS player ("Mandara Radio"), so media keys, the
//! media panel and island, the lock screen and other apps' controls see
//! and drive it. The server exists only while a station is chosen (from
//! play until stop); it runs as a task on the shared service runtime with
//! a private connection (no thread of its own) and releases its bus name
//! when [`Server`] is dropped.

use std::collections::HashMap;
use std::sync::Weak;

use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use super::icy::artist_and_title;
use super::player::{Inner, Player, Request, Status};

pub const BUS_NAME: &str = "org.mpris.MediaPlayer2.mandara_radio";
const PATH: &str = "/org/mpris/MediaPlayer2";
pub const IDENTITY: &str = "Mandara Radio";

/// A running MPRIS server; dropping it ends the task and frees the name.
pub struct Server {
    tx: async_channel::Sender<()>,
}

impl Server {
    pub(super) fn start(player: &Player) -> Self {
        let (tx, rx) = async_channel::bounded(1);
        let weak = player.downgrade();
        crate::runtime::spawn("radio-mpris", async move {
            if let Err(e) = serve(weak, rx).await {
                tracing::warn!("radio: MPRIS unavailable: {e}");
            }
        });
        Self { tx }
    }

    /// The state changed: tell the bus (coalesced; never blocks).
    pub(super) fn ping(&self) {
        let _ = self.tx.try_send(());
    }
}

async fn serve(weak: Weak<Inner>, changes: async_channel::Receiver<()>) -> zbus::Result<()> {
    let builder = zbus::connection::Builder::session()?.name(BUS_NAME)?.serve_at(PATH, Root { player: weak.clone() })?.serve_at(PATH, PlayerIface { player: weak })?;
    let conn = crate::runtime::connect(builder).await?;
    let mut last: Option<(String, HashMap<String, OwnedValue>, f64)> = None;
    while changes.recv().await.is_ok() {
        let iface = conn.object_server().interface::<_, PlayerIface>(PATH).await?;
        let emitter = iface.signal_emitter();
        let p = iface.get().await;
        let now = (p.playback_status(), p.metadata(), p.volume());
        let before = last.as_ref();
        if before.is_none_or(|b| b.0 != now.0) {
            p.playback_status_changed(emitter).await?;
        }
        if before.is_none_or(|b| b.1 != now.1) {
            p.metadata_changed(emitter).await?;
        }
        if before.is_none_or(|b| (b.2 - now.2).abs() > 1e-6) {
            p.volume_changed(emitter).await?;
        }
        last = Some(now);
    }
    Ok(())
}

struct Root {
    player: Weak<Inner>,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {
        if let Some(p) = Player::from_weak(&self.player) {
            p.request(Request::Raise);
        }
    }
    fn quit(&self) {
        if let Some(p) = Player::from_weak(&self.player) {
            p.stop();
        }
    }
    #[zbus(property)]
    fn identity(&self) -> String {
        IDENTITY.into()
    }
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
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

struct PlayerIface {
    player: Weak<Inner>,
}

fn insert(map: &mut HashMap<String, OwnedValue>, key: &str, value: Value<'_>) {
    if let Ok(v) = OwnedValue::try_from(value) {
        map.insert(key.to_string(), v);
    }
}

/// An object path element from a station id (`[A-Za-z0-9_]` only).
fn track_path(id: &str) -> String {
    let clean: String = id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    format!("/org/mandara/radio/{}", if clean.is_empty() { "station" } else { &clean })
}

/// MPRIS metadata for what is playing.
pub fn metadata(station_id: &str, station: &str, title: Option<&str>) -> HashMap<String, OwnedValue> {
    let mut m = HashMap::new();
    if let Ok(path) = ObjectPath::try_from(track_path(station_id)) {
        insert(&mut m, "mpris:trackid", Value::from(path));
    }
    let (artist, song) = title.map(artist_and_title).unwrap_or((None, station));
    insert(&mut m, "xesam:title", Value::from(song));
    insert(&mut m, "xesam:artist", Value::from(vec![artist.unwrap_or(station)]));
    insert(&mut m, "xesam:album", Value::from(station));
    m
}

impl PlayerIface {
    fn with<R: Default>(&self, f: impl FnOnce(&Player) -> R) -> R {
        Player::from_weak(&self.player).map(|p| f(&p)).unwrap_or_default()
    }
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl PlayerIface {
    fn play_pause(&self) {
        self.with(|p| {
            p.toggle();
        });
    }
    fn play(&self) {
        self.with(|p| {
            if !p.status().active() {
                p.resume();
            }
        });
    }
    fn pause(&self) {
        self.with(|p| {
            p.pause();
        });
    }
    fn stop(&self) {
        self.with(|p| {
            p.pause();
        });
    }
    fn next(&self) {
        self.with(|p| p.request(Request::Next));
    }
    fn previous(&self) {
        self.with(|p| p.request(Request::Previous));
    }
    fn seek(&self, _offset: i64) {}
    fn set_position(&self, _track_id: ObjectPath<'_>, _position: i64) {}
    fn open_uri(&self, _uri: &str) {}
    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> String {
        let status = self.with(Player::status);
        match status {
            Status::Stopped => "Stopped",
            Status::Paused => "Paused",
            _ => "Playing",
        }
        .into()
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        self.with(|p| p.with(|s| s.tune.as_ref().map(|t| metadata(&t.id, &t.name, s.title.as_deref())).unwrap_or_default()))
    }
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        0
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
    fn volume(&self) -> f64 {
        self.with(|p| if p.muted() { 0.0 } else { f64::from(p.volume()) })
    }
    #[zbus(property)]
    fn set_volume(&mut self, volume: f64) {
        self.with(|p| {
            p.set_muted(false);
            p.set_volume(volume as f32);
        });
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
        false
    }
    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_splits_icy_titles() {
        let m = metadata("soma-groove-salad", "SomaFM Groove Salad", Some("Harbor Lights - Low Tide"));
        let title: String = m["xesam:title"].try_clone().unwrap().try_into().unwrap();
        let album: String = m["xesam:album"].try_clone().unwrap().try_into().unwrap();
        let artist: Vec<String> = m["xesam:artist"].try_clone().unwrap().try_into().unwrap();
        assert_eq!((title.as_str(), album.as_str(), artist), ("Low Tide", "SomaFM Groove Salad", vec!["Harbor Lights".to_string()]));
        let path: ObjectPath<'_> = m["mpris:trackid"].try_clone().unwrap().try_into().unwrap();
        assert_eq!(path.as_str(), "/org/mandara/radio/soma_groove_salad");
        // No title yet: the station stands in.
        let m = metadata("custom-1", "Corniche FM", None);
        let title: String = m["xesam:title"].try_clone().unwrap().try_into().unwrap();
        assert_eq!(title, "Corniche FM");
        assert!(!m.contains_key("mpris:length"), "live: no length");
        assert_eq!(track_path("--"), "/org/mandara/radio/__");
    }
}
