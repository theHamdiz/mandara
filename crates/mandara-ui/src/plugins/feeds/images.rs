//! Feed pictures: card thumbnails, feed icons and (when allowed) article
//! images. Only pictures on screen are requested. One sequential worker
//! downloads each once into a bounded disk cache and decodes it at the
//! physical size it is drawn at; decoded images live in a small LRU
//! budget and are all released when the panel closes.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::rc::Rc;
use std::cell::RefCell;
use std::sync::Arc;

use gpui::{App, RenderImage};
use sha2::{Digest as _, Sha256};

use super::super::decode_queue::Queue;
use super::super::panel_fx::ViewSet;
use super::super::render_image::PreparedImage;

/// Decoded pictures held at once (BGRA bytes), least recently drawn
/// evicted first. About forty large thumbnails at 1.5×.
const BUDGET_BYTES: usize = 12 * 1024 * 1024;
/// The disk cache of downloaded pictures: files and bytes at most.
const DISK_MAX_FILES: usize = 600;
const DISK_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Decoded pictures are never larger than this on either side.
const MAX_SIDE_PX: u32 = 2048;
/// Decoder limits for untrusted files (dimensions and allocation).
const DECODE_MAX_DIM: u32 = 12_000;
const DECODE_MAX_ALLOC: u64 = 256 * 1024 * 1024;
/// Results waiting for the UI while the worker continues.
const RESULTS_QUEUED: usize = 1;
const BGRA: usize = 4;

/// How a picture fills its box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fit {
    /// Cropped to fill the box (thumbnails, icons).
    Fill,
    /// Scaled down to fit the box, keeping its shape (article images).
    Contain,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key { url: Arc<str>, w: u32, h: u32, fit: Fit }

enum State { Loading, Ready(Arc<RenderImage>), Failed }

struct Slot { state: State, bytes: usize, used: u64 }

type Job = (Key, String);
type Done = (Key, Option<Arc<RenderImage>>);

#[derive(Default)]
pub struct Images {
    slots: HashMap<Key, Slot>,
    queue: Queue<Job>,
    pending: Vec<Job>,
    tick: u64,
}

pub type Shared = Rc<RefCell<Images>>;

impl Images {
    /// Start a frame: pictures not asked for again become evictable.
    pub fn frame(&mut self) { self.tick += 1; }

    /// The decoded picture for `url` at `w × h` physical pixels, queueing
    /// it when it is not loaded yet. `None` while loading or failed.
    pub fn get(&mut self, url: &str, w: u32, h: u32, fit: Fit) -> Option<Arc<RenderImage>> {
        let (w, h) = (w.clamp(1, MAX_SIDE_PX), h.clamp(1, MAX_SIDE_PX));
        let tick = self.tick;
        let key = Key { url: url.into(), w, h, fit };
        if let Some(slot) = self.slots.get_mut(&key) {
            slot.used = tick;
            return match &slot.state { State::Ready(image) => Some(image.clone()), _ => None };
        }
        self.slots.insert(key.clone(), Slot { state: State::Loading, bytes: 0, used: tick });
        self.pending.push((key, url.to_owned()));
        None
    }

    /// Whether this picture failed (the view shows its fallback).
    pub fn failed(&self, url: &str, w: u32, h: u32, fit: Fit) -> bool {
        let key = Key { url: url.into(), w: w.clamp(1, MAX_SIDE_PX), h: h.clamp(1, MAX_SIDE_PX), fit };
        matches!(self.slots.get(&key), Some(Slot { state: State::Failed, .. }))
    }

    fn decoded(&self) -> usize { self.slots.values().map(|slot| slot.bytes).sum() }

    /// Evict least recently drawn pictures (never this frame's) over budget.
    fn enforce_budget(&mut self, mut drop_image: impl FnMut(Arc<RenderImage>)) {
        while self.decoded() > BUDGET_BYTES {
            let Some(victim) = self.slots.iter().filter(|(_, slot)| slot.bytes > 0 && slot.used != self.tick).min_by_key(|(_, slot)| slot.used).map(|(key, _)| key.clone()) else { break };
            if let Some(Slot { state: State::Ready(image), .. }) = self.slots.remove(&victim) { drop_image(image); }
        }
    }

    /// Release every picture and stop the worker's batch (panel closed).
    pub fn clear(&mut self, mut drop_image: impl FnMut(Arc<RenderImage>)) -> bool {
        self.queue.clear();
        self.pending.clear();
        self.pending.shrink_to_fit();
        let had = !self.slots.is_empty();
        for (_, slot) in self.slots.drain() { if let State::Ready(image) = slot.state { drop_image(image); } }
        self.slots.shrink_to_fit();
        had
    }

    pub fn is_empty(&self) -> bool { self.slots.is_empty() }
}

/// Start the worker for this frame's new requests.
pub fn start(images: &Shared, views: ViewSet, cx: &mut App) {
    let mut batch = {
        let mut images = images.borrow_mut();
        if images.pending.is_empty() { return; }
        let jobs = std::mem::take(&mut images.pending);
        images.queue.extend(jobs);
        images.queue.start()
    };
    if batch.is_none() { return; }
    let weak = Rc::downgrade(images);
    cx.spawn(async move |cx| {
        while let Some(jobs) = batch {
            let generation = jobs.generation();
            let (tx, rx) = async_channel::bounded::<Done>(RESULTS_QUEUED);
            let task = cx.background_executor().spawn(async move {
                jobs.run(|(key, url): Job| { let image = load(&url, key.w, key.h, key.fit); (key, image) }, tx).await;
            });
            while let Ok((key, image)) = rx.recv().await {
                let Some(images) = weak.upgrade() else { break };
                {
                    let mut images = images.borrow_mut();
                    if !images.queue.current(generation) {
                        if let Some(image) = image { cx.update(|cx| cx.drop_image(image, None)); }
                        continue;
                    }
                    match images.slots.get_mut(&key) {
                        Some(slot) => match image {
                            Some(image) => {
                                let size = image.size(0);
                                slot.bytes = size.width.0.max(0) as usize * size.height.0.max(0) as usize * BGRA;
                                slot.state = State::Ready(image);
                            }
                            None => slot.state = State::Failed,
                        },
                        None => if let Some(image) = image { cx.update(|cx| cx.drop_image(image, None)); },
                    }
                    images.enforce_budget(|image| cx.update(|cx| cx.drop_image(image, None)));
                }
                views.notify_async(cx);
            }
            drop(rx);
            task.await;
            // The last decode may have outlived a close.
            mandara_services::memory::request_trim();
            let Some(images) = weak.upgrade() else { break };
            batch = images.borrow_mut().queue.finish();
        }
    }).detach();
}

fn disk_dir() -> PathBuf { mandara_config::paths::cache_dir().join("feeds").join("images") }

fn disk_path(url: &str) -> PathBuf { disk_dir().join(format!("{:x}", Sha256::digest(url))) }

/// The picture's bytes from the disk cache, or downloaded into it.
fn bytes(url: &str) -> Option<Vec<u8>> {
    let path = disk_path(url);
    if let Ok(bytes) = std::fs::read(&path) {
        // Touch it so the cache keeps what is still drawn.
        let _ = std::fs::File::options().append(true).open(&path).and_then(|file| file.set_modified(std::time::SystemTime::now()));
        return Some(bytes);
    }
    let bytes = mandara_services::feeds::download_image(url).ok()?;
    // Written beside its final name, then renamed: a reader never sees half a file.
    let partial = path.with_extension("part");
    let _ = std::fs::create_dir_all(disk_dir()).and_then(|()| {
        std::fs::File::create(&partial)?.write_all(&bytes)?;
        std::fs::rename(&partial, &path)
    });
    prune_disk();
    Some(bytes)
}

/// Keep the disk cache within its file and byte limits, oldest first.
fn prune_disk() {
    let Ok(dir) = std::fs::read_dir(disk_dir()) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = dir.flatten()
        .filter_map(|entry| { let meta = entry.metadata().ok()?; meta.is_file().then(|| (meta.modified().unwrap_or(std::time::UNIX_EPOCH), meta.len(), entry.path())) })
        .collect();
    let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
    if files.len() <= DISK_MAX_FILES && total <= DISK_MAX_BYTES { return; }
    files.sort_by_key(|(modified, _, _)| *modified);
    let mut count = files.len();
    for (_, len, path) in files {
        if count <= DISK_MAX_FILES && total <= DISK_MAX_BYTES { break; }
        if std::fs::remove_file(&path).is_ok() { count -= 1; total = total.saturating_sub(len); }
    }
}

/// Download (or read) and decode one picture at its draw size.
fn load(url: &str, w: u32, h: u32, fit: Fit) -> Option<Arc<RenderImage>> {
    // The retained header precedes transient download and decoder storage.
    let prepared = PreparedImage::new();
    let bytes = bytes(url)?;
    let picture = decode(&bytes)?;
    drop(bytes);
    let rgba = match fit {
        Fit::Fill => picture.resize_to_fill(w, h, image::imageops::FilterType::Triangle).into_rgba8(),
        Fit::Contain if picture.width() > w || picture.height() > h => picture.resize(w, h, image::imageops::FilterType::Triangle).into_rgba8(),
        Fit::Contain => picture.into_rgba8(),
    };
    Some(prepared.finish(rgba))
}

fn decode(bytes: &[u8]) -> Option<image::DynamicImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(DECODE_MAX_DIM);
    limits.max_image_height = Some(DECODE_MAX_DIM);
    limits.max_alloc = Some(DECODE_MAX_ALLOC);
    reader.limits(limits);
    reader.decode().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(images: &mut Images, url: &str, side: u32) {
        let key = Key { url: url.into(), w: side, h: side, fit: Fit::Fill };
        let image = PreparedImage::new().finish(image::RgbaImage::new(side, side));
        images.slots.insert(key, Slot { state: State::Ready(image), bytes: (side * side) as usize * BGRA, used: images.tick });
    }

    #[test]
    fn requests_queue_once_and_the_budget_evicts_least_recent() {
        let mut images = Images::default();
        images.frame();
        assert!(images.get("https://example.org/a.jpg", 100, 100, Fit::Fill).is_none());
        assert!(images.get("https://example.org/a.jpg", 100, 100, Fit::Fill).is_none());
        assert_eq!(images.pending.len(), 1, "a loading picture is requested once");
        // 1024² BGRA is 4 MiB: four of them exceed the 12 MiB budget.
        for n in 0..4 { images.frame(); ready(&mut images, &format!("https://example.org/{n}.jpg"), 1024); }
        let mut dropped = 0;
        images.enforce_budget(|_| dropped += 1);
        assert_eq!(dropped, 1);
        assert!(images.decoded() <= BUDGET_BYTES);
        assert!(!images.slots.keys().any(|key| &*key.url == "https://example.org/0.jpg"), "the oldest went first");
        let mut released = 0;
        assert!(images.clear(|_| released += 1));
        assert_eq!(released, 3);
        assert!(images.is_empty() && images.pending.is_empty());
    }

    #[test]
    fn decoding_resizes_to_the_draw_size_and_refuses_garbage() {
        let mut png = Vec::new();
        image::RgbaImage::new(640, 320).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let picture = decode(&png).unwrap();
        let filled = picture.resize_to_fill(96, 96, image::imageops::FilterType::Triangle);
        assert_eq!((filled.width(), filled.height()), (96, 96));
        assert!(decode(b"not an image").is_none());
    }
}
