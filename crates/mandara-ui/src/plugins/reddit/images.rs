//! Previews decoded at the size they are drawn, one at a time on the
//! plugin's worker, kept within a byte budget and dropped when the panel
//! closes. A result that arrives after the cache was cleared is discarded.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use gpui::RenderImage;

use crate::plugins::render_image::PreparedImage;

/// Decoded previews kept at once (BGRA bytes); the least recently drawn go first.
pub(super) const BUDGET_BYTES: usize = 16 * 1024 * 1024;
/// Pictures waiting for the worker; older requests than this were scrolled past.
pub(super) const MAX_QUEUED: usize = 32;
const RGBA: usize = 4;
/// A blurred preview is decoded this many times smaller, then scaled up.
const BLUR_FACTOR: u32 = 16;
/// The tallest a reader picture is drawn, relative to its width.
pub(super) const MAX_ASPECT: f32 = 1.5;

/// One picture at one drawn size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Key(u64);

impl Key {
    pub fn new(url: &str, width: u32, height: u32, blur: bool) -> Self {
        let mut hasher = DefaultHasher::new();
        (url, width, height, blur).hash(&mut hasher);
        Self(hasher.finish())
    }
}

/// What the worker should fetch and how to draw it.
#[derive(Clone, Debug)]
pub(super) struct Request {
    pub key: Key,
    pub url: Box<str>,
    pub width: u32,
    pub height: u32,
    pub blur: bool,
}

pub(super) enum Slot {
    Loading,
    Ready(Arc<RenderImage>),
    Failed,
}

struct Entry {
    slot: Slot,
    bytes: usize,
    used: u64,
}

#[derive(Default)]
pub(super) struct Images {
    entries: HashMap<Key, Entry>,
    queue: std::collections::VecDeque<Request>,
    tick: u64,
}

impl Images {
    /// A new frame: pictures drawn from now on count as recently used.
    pub fn frame(&mut self) { self.tick = self.tick.wrapping_add(1); }

    /// The picture for `request`, queueing it the first time it is drawn.
    pub fn get(&mut self, request: impl FnOnce() -> Request, key: Key) -> Option<&Slot> {
        let tick = self.tick;
        if !self.entries.contains_key(&key) {
            if self.queue.len() >= MAX_QUEUED { self.queue.pop_front().map(|old| self.entries.remove(&old.key)); }
            self.entries.insert(key, Entry { slot: Slot::Loading, bytes: 0, used: tick });
            self.queue.push_back(request());
        }
        let entry = self.entries.get_mut(&key)?;
        entry.used = tick;
        Some(&entry.slot)
    }

    /// The next picture for the worker.
    pub fn next(&mut self) -> Option<Request> { self.queue.pop_front() }

    /// Store a decoded picture, evicting older ones beyond the budget.
    /// Returns images that left the cache, for the GPU atlas to forget.
    pub fn finish(&mut self, key: Key, result: Option<Arc<RenderImage>>, bytes: usize) -> Vec<Arc<RenderImage>> {
        let Some(entry) = self.entries.get_mut(&key) else { return result.into_iter().collect() };
        match result {
            Some(image) => { entry.slot = Slot::Ready(image); entry.bytes = bytes; }
            None => entry.slot = Slot::Failed,
        }
        self.evict()
    }

    fn total(&self) -> usize { self.entries.values().map(|entry| entry.bytes).sum() }

    fn evict(&mut self) -> Vec<Arc<RenderImage>> {
        let mut gone = Vec::new();
        while self.total() > BUDGET_BYTES {
            let tick = self.tick;
            let Some(key) = self.entries.iter().filter(|(_, entry)| entry.bytes > 0 && entry.used != tick).min_by_key(|(_, entry)| entry.used).map(|(key, _)| *key) else { break };
            if let Some(Entry { slot: Slot::Ready(image), .. }) = self.entries.remove(&key) { gone.push(image); }
        }
        gone
    }

    /// Everything decoded and queued, for the panel's close.
    pub fn clear(&mut self) -> Vec<Arc<RenderImage>> {
        self.queue.clear();
        self.queue.shrink_to_fit();
        let gone = self.entries.drain().filter_map(|(_, entry)| match entry.slot { Slot::Ready(image) => Some(image), _ => None }).collect();
        self.entries.shrink_to_fit();
        gone
    }

    pub fn bytes(&self) -> usize { self.total() }
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn queued(&self) -> usize { self.queue.len() }
}

/// Decode `bytes` to exactly `width × height` (cropped to fill), blurred on
/// request. The retained handle is allocated before the decoder's buffers.
pub(super) fn decode(bytes: Vec<u8>, width: u32, height: u32, blur: bool) -> Option<(Arc<RenderImage>, usize)> {
    let rendered = PreparedImage::new();
    let picture = crate::plugins::render_image::decode_limited(&bytes)?;
    drop(bytes);
    let rgba = if blur {
        let small = picture.resize_to_fill((width / BLUR_FACTOR).max(1), (height / BLUR_FACTOR).max(1), image::imageops::FilterType::Triangle);
        drop(picture);
        image::imageops::resize(&small.into_rgba8(), width, height, image::imageops::FilterType::Triangle)
    } else {
        let sized = picture.resize_to_fill(width, height, image::imageops::FilterType::Triangle);
        drop(picture);
        sized.into_rgba8()
    };
    let bytes = rgba.len();
    Some((rendered.finish(rgba), bytes.max(width as usize * height as usize * RGBA)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(key: Key) -> Request { Request { key, url: "https://i.redd.it/x.png".into(), width: 2, height: 2, blur: false } }

    fn image() -> Arc<RenderImage> {
        PreparedImage::new().finish(image::RgbaImage::from_raw(1, 1, vec![1, 2, 3, 4]).unwrap())
    }

    #[test]
    fn the_cache_stays_within_its_budget_and_keeps_pictures_drawn_this_frame() {
        let mut images = Images::default();
        let keys: Vec<Key> = (0..4).map(|n| Key::new(&format!("https://i.redd.it/{n}.png"), 10, 10, false)).collect();
        images.frame();
        for key in &keys { images.get(|| request(*key), *key); }
        assert_eq!(images.queued(), 4);
        while images.next().is_some() {}
        let third = BUDGET_BYTES / 3;
        assert!(images.finish(keys[0], Some(image()), third).is_empty());
        images.frame();
        images.get(|| unreachable!("already known"), keys[1]);
        images.finish(keys[1], Some(image()), third);
        images.finish(keys[2], Some(image()), third);
        let gone = images.finish(keys[3], Some(image()), third);
        assert_eq!(gone.len(), 1, "the oldest undrawn picture leaves");
        assert!(images.bytes() <= BUDGET_BYTES);
        assert!(matches!(images.get(|| unreachable!(), keys[1]), Some(Slot::Ready(_))), "a picture drawn this frame stays");
        let cleared = images.clear();
        assert_eq!(cleared.len(), 3);
        assert_eq!((images.len(), images.bytes(), images.queued()), (0, 0, 0));
    }

    #[test]
    fn a_late_result_after_clearing_is_not_kept_and_the_queue_is_bounded() {
        let mut images = Images::default();
        for n in 0..(MAX_QUEUED + 10) {
            let key = Key::new(&n.to_string(), 1, 1, false);
            images.get(|| request(key), key);
        }
        assert_eq!(images.queued(), MAX_QUEUED);
        let key = Key::new("late", 1, 1, false);
        images.clear();
        let returned = images.finish(key, Some(image()), 4);
        assert_eq!(returned.len(), 1, "handed back to be dropped, never stored");
        assert_eq!(images.len(), 0);
    }

    #[test]
    fn decoding_produces_the_drawn_size_and_blurs_on_request() {
        let mut source = image::RgbaImage::new(64, 32);
        for (x, _, pixel) in source.enumerate_pixels_mut() { *pixel = if x % 2 == 0 { image::Rgba([255, 255, 255, 255]) } else { image::Rgba([0, 0, 0, 255]) }; }
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(source).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let (sharp, bytes) = decode(png.clone(), 32, 16, false).unwrap();
        assert_eq!((sharp.size(0).width.0, sharp.size(0).height.0, bytes), (32, 16, 32 * 16 * 4));
        let (blurred, _) = decode(png, 32, 16, true).unwrap();
        let pixels = blurred.as_bytes(0).unwrap();
        let spread = pixels.chunks(4).map(|p| p[0]).max().unwrap() - pixels.chunks(4).map(|p| p[0]).min().unwrap();
        assert!(spread < 64, "a blurred preview has no hard stripes ({spread})");
        assert!(decode(b"not an image".to_vec(), 4, 4, false).is_none());
        // The decoder runs with Feeds' limits: a picture wider than they
        // allow is refused before its pixels are allocated.
        let mut wide = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(13_000, 1)).write_to(&mut std::io::Cursor::new(&mut wide), image::ImageFormat::Png).unwrap();
        assert!(decode(wide, 4, 4, false).is_none(), "a 13000-pixel-wide picture is over the decode limit");
    }
}
