//! The plugin side of the broker: requests a plugin queued, replies waiting
//! for it, open subscriptions and the image handles it holds. Everything
//! is bounded ([`crate::extension`]'s limits) and owned by the plugin's
//! store, so it goes when the instance does.
//!
//! The host's brokers (in the shell) take the queued requests after each
//! call, act on them on their own threads or the UI's, and deliver the
//! results here; the next `extension-event` hands them to the plugin. A
//! plugin never waits: `submit` returns an id at once.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use crate::extension::{self, Access, Capability};

/// A request a plugin queued.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub id: u64,
    pub capability: Capability,
    pub operation: String,
    /// The JSON object it sent.
    pub payload: serde_json::Map<String, serde_json::Value>,
}

impl Request {
    pub fn subscription(&self) -> bool {
        self.capability.is_subscription(&self.operation)
    }
}

/// Where an image handle points, and why the plugin may see it (checked
/// again against the live grants before drawing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageOrigin {
    /// A file in the plugin's own `assets/` folder.
    Asset,
    /// A wallpaper (`wallpaper.read`).
    Wallpaper,
    /// A file in a folder the person granted (`files.folders`).
    Folder(PathBuf),
    /// The playing track's cover (`media.read`).
    MediaArt,
}

impl ImageOrigin {
    /// The capability the origin needs (none for the plugin's own assets).
    pub fn capability(&self) -> Option<Capability> {
        match self {
            Self::Asset => None,
            Self::Wallpaper => Some(Capability::WallpaperRead),
            Self::Folder(_) => Some(Capability::Folders),
            Self::MediaArt => Some(Capability::MediaRead),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageEntry {
    pub path: PathBuf,
    pub origin: ImageOrigin,
}

/// Largest image a plugin may ship in `assets/`.
pub const MAX_ASSET_BYTES: u64 = 512 * 1024;

/// Image extensions a handle may point at.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];

/// Whether `path` names an image file by extension.
pub fn is_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// A request the host took and hasn't answered (a subscription stays open
/// until cancelled).
struct Open {
    id: u64,
    subscription: bool,
    capability: Capability,
    /// A second capability it relies on (an `events` topic's read one).
    also: Option<Capability>,
}

impl Open {
    fn allowed(&self, access: &Access) -> bool {
        access.allows(&self.capability) && self.also.as_ref().is_none_or(|c| access.allows(c))
    }
}

/// The read capability an `events` request's topic needs, if any.
fn topic_capability(r: &Request) -> Option<Capability> {
    if r.capability != Capability::Events {
        return None;
    }
    let topic = r.payload.get("topic").and_then(serde_json::Value::as_str)?;
    extension::EVENT_TOPICS.iter().find(|(t, _)| *t == topic).and_then(|(_, c)| c.clone())
}

/// A result waiting to be handed over.
struct Waiting {
    id: u64,
    result: Result<String, String>,
    capability: Capability,
    also: Option<Capability>,
}

#[derive(Default)]
pub struct Broker {
    /// Queued and not yet taken by the host.
    queued: Vec<Request>,
    /// Taken and not yet answered.
    outstanding: Vec<Open>,
    /// Subscriptions the plugin cancelled (the host stops their sources).
    cancelled: Vec<u64>,
    /// Results waiting for the next `extension-event`, with the
    /// capabilities they rely on (re-checked when grants change).
    inbox: VecDeque<Waiting>,
    images: VecDeque<(u64, ImageEntry)>,
}

impl Broker {
    /// A fresh id from one counter for the whole process (requests and
    /// image handles alike): a reloaded plugin never reuses an id, so a late
    /// result for its old instance can't answer a new request.
    fn issue(&mut self) -> u64 {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    fn open(&self, subscription: bool) -> usize {
        self.outstanding.iter().filter(|o| o.subscription == subscription).count() + self.queued.iter().filter(|r| r.subscription() == subscription).count()
    }

    /// `broker.submit`: checks the capability against `access`, the
    /// payload's size and shape and the queue limits, then queues it.
    pub fn submit(&mut self, access: &Access, activated: bool, capability: &str, operation: String, payload: &str) -> Result<u64, String> {
        let capability = Capability::parse(capability)?;
        if let Some(why) = access.refuse_now(&capability, &operation, activated) {
            return Err(why);
        }
        if payload.len() > extension::MAX_PAYLOAD_BYTES {
            return Err(format!("the payload is {} bytes (at most {})", payload.len(), extension::MAX_PAYLOAD_BYTES));
        }
        let payload = match payload.trim() {
            "" => serde_json::Map::new(),
            text => match serde_json::from_str::<serde_json::Value>(text) {
                Ok(serde_json::Value::Object(map)) => map,
                Ok(_) => return Err("the payload must be a JSON object".into()),
                Err(e) => return Err(format!("the payload isn't JSON: {e}")),
            },
        };
        if capability == Capability::Events {
            let topic = payload.get("topic").and_then(serde_json::Value::as_str).unwrap_or_default();
            let Some((_, needs)) = extension::EVENT_TOPICS.iter().find(|(t, _)| *t == topic) else {
                return Err(format!("unknown events topic `{topic}`"));
            };
            if let Some(needs) = needs.as_ref().filter(|c| !access.allows(c)) {
                return Err(format!("the `{topic}` topic also needs `{needs}`"));
            }
        }
        let subscription = capability.is_subscription(&operation);
        let (open, limit) = if subscription { (self.open(true), extension::MAX_SUBSCRIPTIONS) } else { (self.open(false), extension::MAX_PENDING_REQUESTS) };
        if open >= limit {
            return Err(format!("too many {} at once (at most {limit})", if subscription { "subscriptions" } else { "waiting requests" }));
        }
        let id = self.issue();
        self.queued.push(Request { id, capability, operation, payload });
        Ok(id)
    }

    /// `broker.cancel`.
    pub fn cancel(&mut self, id: u64) {
        self.queued.retain(|r| r.id != id);
        if let Some(at) = self.outstanding.iter().position(|o| o.id == id)
            && self.outstanding.remove(at).subscription
        {
            self.cancelled.push(id);
        }
        self.inbox.retain(|w| w.id != id);
    }

    /// Requests queued since the last take; they are outstanding until
    /// answered (subscriptions until cancelled).
    pub fn take_requests(&mut self) -> Vec<Request> {
        let taken = std::mem::take(&mut self.queued);
        self.outstanding.extend(taken.iter().map(|r| Open { id: r.id, subscription: r.subscription(), capability: r.capability.clone(), also: topic_capability(r) }));
        taken
    }

    /// Subscriptions cancelled since the last take.
    pub fn take_cancelled(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.cancelled)
    }

    /// Whether `id` still waits for an answer (or is an open subscription).
    pub fn is_open(&self, id: u64) -> bool {
        self.outstanding.iter().any(|o| o.id == id)
    }

    /// Stores a result for the plugin. A request's answer closes it; a
    /// subscription's update replaces an undelivered older one. Results for
    /// cancelled or unknown ids are dropped; an oversized one becomes an
    /// error; past [`extension::MAX_INBOX`] the oldest result goes.
    pub fn deliver(&mut self, access: &Access, id: u64, result: Result<String, String>) -> bool {
        let Some(at) = self.outstanding.iter().position(|o| o.id == id) else { return false };
        // Re-checked on every delivery: a revoked grant stops the data at
        // once, even before the revocation reached `close_where`.
        if !self.outstanding[at].allowed(access) {
            let open = self.outstanding.remove(at);
            if open.subscription {
                self.cancelled.push(open.id);
            }
            return false;
        }
        let subscription = self.outstanding[at].subscription;
        let (capability, also) = (self.outstanding[at].capability.clone(), self.outstanding[at].also.clone());
        if !subscription {
            self.outstanding.remove(at);
        }
        let result = match result {
            Ok(body) if body.len() > extension::MAX_REPLY_BYTES => Err(format!("the reply was larger than {} bytes", extension::MAX_REPLY_BYTES)),
            other => other,
        };
        if subscription && let Some(slot) = self.inbox.iter_mut().find(|w| w.id == id) {
            slot.result = result;
            return true;
        }
        if self.inbox.len() >= extension::MAX_INBOX {
            self.inbox.pop_front();
        }
        self.inbox.push_back(Waiting { id, result, capability, also });
        true
    }

    /// Closes the open requests and subscriptions whose capability `close`
    /// picks (a revoked grant), and the queued ones: later results for them
    /// are dropped, and closed subscriptions are reported as cancelled.
    pub fn close_where(&mut self, access: &Access) {
        // Results already waiting are dropped too: nothing read under a
        // revoked grant is handed over afterwards.
        self.inbox.retain(|w| access.allows(&w.capability) && w.also.as_ref().is_none_or(|c| access.allows(c)));
        self.queued.retain(|r| access.allows(&r.capability) && topic_capability(r).is_none_or(|c| access.allows(&c)));
        let cancelled = &mut self.cancelled;
        self.outstanding.retain(|o| {
            let shut = !o.allowed(access);
            if shut && o.subscription {
                cancelled.push(o.id);
            }
            !shut
        });
    }

    /// Requests or cancellations the host hasn't taken yet.
    pub fn has_queued(&self) -> bool {
        !self.queued.is_empty() || !self.cancelled.is_empty()
    }

    pub fn has_replies(&self) -> bool {
        !self.inbox.is_empty()
    }

    pub fn take_replies(&mut self) -> Vec<(u64, Result<String, String>)> {
        self.inbox.drain(..).map(|w| (w.id, w.result)).collect()
    }

    /// A handle for `entry` (the same one if it already has one). Past
    /// [`extension::MAX_IMAGE_HANDLES`] the oldest handle is released.
    pub fn issue_image(&mut self, entry: ImageEntry) -> u64 {
        if let Some((id, _)) = self.images.iter().find(|(_, e)| *e == entry) {
            return *id;
        }
        if self.images.len() >= extension::MAX_IMAGE_HANDLES {
            self.images.pop_front();
        }
        let id = self.issue();
        self.images.push_back((id, entry));
        id
    }

    pub fn image(&self, handle: u64) -> Option<&ImageEntry> {
        self.images.iter().find(|(id, _)| *id == handle).map(|(_, e)| e)
    }

    /// Releases handles whose origin is no longer allowed (a revoked grant).
    pub fn release_images(&mut self, mut keep: impl FnMut(&ImageEntry) -> bool) {
        self.images.retain(|(_, e)| keep(e));
        if self.images.capacity() > extension::MAX_IMAGE_HANDLES && self.images.len() < extension::MAX_IMAGE_HANDLES / 2 {
            self.images.shrink_to_fit();
        }
    }

    pub fn image_count(&self) -> usize {
        self.images.len()
    }
}

/// `broker.asset-image`: an image in `<plugin dir>/assets/`. The name is a
/// plain file name (no folders) with an image extension, and the file must
/// really be inside `assets/` (a symlink pointing out is refused).
pub fn asset_path(plugin_dir: &Path, name: &str) -> Result<PathBuf, String> {
    let plain = !name.is_empty() && name.len() <= extension::MAX_NAME_LEN && !name.starts_with('.') && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if !plain || !is_image(Path::new(name)) {
        return Err(format!("`{name}` isn't a PNG, JPEG or WebP file name in assets/"));
    }
    let assets = plugin_dir.join("assets");
    let path = assets.join(name);
    let (Ok(real), Ok(root)) = (path.canonicalize(), assets.canonicalize()) else { return Err(format!("assets/{name} doesn't exist")) };
    if !real.starts_with(&root) || !real.is_file() {
        return Err(format!("assets/{name} isn't a file inside assets/"));
    }
    // Decoded once per drawn size; a small file keeps that cheap.
    if std::fs::metadata(&real).map(|m| m.len()).unwrap_or(u64::MAX) > MAX_ASSET_BYTES {
        return Err(format!("assets/{name} is larger than {} KiB", MAX_ASSET_BYTES / 1024));
    }
    Ok(real)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access() -> Access {
        Access::new(&["media.read".into(), "media.control".into(), "net:api.example.com".into()], &["media.read".into(), "media.control".into(), "net:api.example.com".into()])
    }

    #[test]
    fn submit_checks_grants_payloads_and_limits() {
        let mut b = Broker::default();
        let a = access();
        assert!(b.submit(&a, true, "wallpaper.set", "apply".into(), "{}").unwrap_err().contains("permissions"));
        assert!(b.submit(&Access::new(&["media.read".into()], &[]), true, "media.read", "status".into(), "").unwrap_err().contains("granted"));
        assert!(b.submit(&a, true, "media.read", "status".into(), "[1]").unwrap_err().contains("object"));
        assert!(b.submit(&a, true, "media.read", "status".into(), &"x".repeat(extension::MAX_PAYLOAD_BYTES + 1)).unwrap_err().contains("bytes"));
        for _ in 0..extension::MAX_PENDING_REQUESTS {
            b.submit(&a, true, "media.control", "next".into(), "{}").unwrap();
        }
        assert!(b.submit(&a, true, "media.control", "next".into(), "{}").unwrap_err().contains("too many"), "the queue is bounded");
        for _ in 0..extension::MAX_SUBSCRIPTIONS {
            b.submit(&a, true, "media.read", "subscribe".into(), "").unwrap();
        }
        assert!(b.submit(&a, true, "media.read", "subscribe".into(), "").is_err(), "subscriptions are bounded");
        // Taking requests doesn't free their slots: they're outstanding.
        let taken = b.take_requests();
        assert_eq!(taken.len(), extension::MAX_PENDING_REQUESTS + extension::MAX_SUBSCRIPTIONS);
        assert!(b.submit(&a, true, "media.control", "next".into(), "{}").is_err());
        assert!(b.deliver(&a, taken[0].id, Ok("{}".into())));
        assert!(b.submit(&a, true, "media.control", "next".into(), "{}").is_ok(), "an answer frees its slot");
    }

    #[test]
    fn replies_are_bounded_coalesced_and_dropped_after_cancel() {
        let mut b = Broker::default();
        let a = access();
        let sub = b.submit(&a, true, "media.read", "subscribe".into(), "").unwrap();
        let req = b.submit(&a, true, "media.read", "status".into(), "").unwrap();
        b.take_requests();
        assert!(b.deliver(&a, sub, Ok("1".into())) && b.deliver(&a, sub, Ok("2".into())));
        assert_eq!(b.take_replies(), vec![(sub, Ok("2".into()))], "an undelivered update is replaced");
        assert!(b.deliver(&a, req, Ok("x".repeat(extension::MAX_REPLY_BYTES + 1))));
        assert!(b.take_replies()[0].1.as_ref().unwrap_err().contains("larger"));
        assert!(!b.deliver(&a, req, Ok("again".into())), "an answered request is closed");
        b.cancel(sub);
        assert_eq!(b.take_cancelled(), [sub]);
        assert!(!b.deliver(&a, sub, Ok("late".into())), "no updates after cancel");
        assert!(!b.has_replies());
    }

    #[test]
    fn replies_waiting_when_a_grant_is_revoked_are_dropped() {
        let mut b = Broker::default();
        let a = access();
        let status = b.submit(&a, false, "media.read", "status".into(), "").unwrap();
        let next = b.submit(&a, false, "media.control", "next".into(), "").unwrap();
        b.take_requests();
        assert!(b.deliver(&a, status, Ok("{\"title\":\"private\"}".into())));
        assert!(b.deliver(&a, next, Ok("{}".into())));
        b.close_where(&Access::new(&["media.read".into(), "media.control".into()], &["media.control".into()]));
        assert_eq!(b.take_replies().iter().map(|(id, _)| *id).collect::<Vec<_>>(), [next], "the media.read answer never reaches the plugin");
    }

    #[test]
    fn ids_are_never_reused_across_instances() {
        let (mut one, mut two) = (Broker::default(), Broker::default());
        let a = access();
        let first = one.submit(&a, false, "media.read", "status".into(), "").unwrap();
        let second = two.submit(&a, false, "media.read", "status".into(), "").unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn revoking_a_topics_read_capability_closes_it() {
        let mut b = Broker::default();
        let both = Access::new(&["events".into(), "windows.read".into()], &["events".into(), "windows.read".into()]);
        let sub = b.submit(&both, false, "events", "subscribe".into(), "{\"topic\": \"windows\"}").unwrap();
        b.take_requests();
        let events_only = Access::new(&["events".into(), "windows.read".into()], &["events".into()]);
        assert!(!b.deliver(&events_only, sub, Ok("{}".into())), "no window titles once windows.read is revoked");
        assert_eq!(b.take_cancelled(), [sub]);
    }

    #[test]
    fn event_topics_need_their_read_capability() {
        let mut b = Broker::default();
        let a = Access::new(&["events".into(), "windows.read".into()], &["events".into()]);
        assert!(b.submit(&a, false, "events", "subscribe".into(), "{\"topic\": \"theme\"}").is_ok());
        assert!(b.submit(&a, false, "events", "subscribe".into(), "{\"topic\": \"windows\"}").unwrap_err().contains("windows.read"));
        assert!(b.submit(&a, false, "events", "subscribe".into(), "{\"topic\": \"keystrokes\"}").unwrap_err().contains("unknown"));
    }

    #[test]
    fn revoking_closes_what_relied_on_the_grant() {
        let mut b = Broker::default();
        let a = access();
        let sub = b.submit(&a, true, "media.read", "subscribe".into(), "").unwrap();
        let next = b.submit(&a, true, "media.control", "next".into(), "").unwrap();
        b.take_requests();
        let queued = b.submit(&a, true, "media.read", "status".into(), "").unwrap();
        b.close_where(&Access::new(&["media.read".into(), "media.control".into()], &["media.control".into()]));
        assert_eq!(b.take_cancelled(), [sub]);
        assert!(!b.is_open(sub) && b.is_open(next));
        assert!(b.take_replies().is_empty());
        assert!(b.take_requests().iter().all(|r| r.id != queued), "queued media.read requests are dropped");
    }

    #[test]
    fn image_handles_are_reused_and_bounded() {
        let mut b = Broker::default();
        let entry = |n: usize| ImageEntry { path: PathBuf::from(format!("/x/{n}.png")), origin: ImageOrigin::Asset };
        let first = b.issue_image(entry(0));
        assert_eq!(b.issue_image(entry(0)), first);
        for n in 1..=extension::MAX_IMAGE_HANDLES {
            b.issue_image(entry(n));
        }
        assert_eq!(b.image_count(), extension::MAX_IMAGE_HANDLES);
        assert!(b.image(first).is_none(), "the oldest handle was released");
        b.release_images(|e| e.path != Path::new("/x/1.png"));
        assert_eq!(b.image_count(), extension::MAX_IMAGE_HANDLES - 1);
    }

    #[test]
    fn assets_stay_inside_the_folder() {
        let root = std::env::temp_dir().join(format!("mandara-assets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/moon.png"), b"png").unwrap();
        std::fs::write(root.join("secret.png"), b"png").unwrap();
        std::os::unix::fs::symlink(root.join("secret.png"), root.join("assets/link.png")).unwrap();
        assert!(asset_path(&root, "moon.png").is_ok());
        for bad in ["../secret.png", "link.png", "moon.svg", ".hidden.png", "a/b.png", "missing.png", ""] {
            assert!(asset_path(&root, bad).is_err(), "{bad}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
