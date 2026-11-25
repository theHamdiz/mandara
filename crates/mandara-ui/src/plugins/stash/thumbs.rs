//! Tile pictures: decoded off the UI thread by one sequential worker, at
//! exactly the physical size a tile draws (a cover crop), kept in a small
//! LRU bounded by bytes. A shelf that is minimized or closed releases its
//! pictures (CPU and GPU) and the cache gives its room back.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::RenderImage;

use super::super::decode_queue::Queue;
use super::super::render_image::PreparedImage;

/// Decoded pictures kept, bytes (≈ 40 tiles at 2×).
pub const BUDGET_BYTES: usize = 6 * 1024 * 1024;
/// Largest side a picture may have before it is decoded.
const MAX_DIM: u32 = 16_000;
/// The most one decode may allocate.
const MAX_ALLOC: u64 = 512 * 1024 * 1024;
const RGBA: usize = 4;

/// What a thumbnail is: an item's picture at an edge (physical px), or a
/// fitted preview of `(w, h)` for quick look.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub item: u64,
    pub w: u32,
    pub h: u32,
    /// Cover crop (tiles) or fit (quick look).
    pub cover: bool,
}

pub enum Thumb {
    Loading,
    Ready(Arc<RenderImage>),
    Failed,
}

struct Slot {
    thumb: Thumb,
    shelf: u64,
    bytes: usize,
    used: u64,
}

/// A decode to run.
pub struct Job {
    pub key: Key,
    pub path: PathBuf,
}

#[derive(Default)]
pub struct Thumbs {
    slots: HashMap<Key, Slot>,
    tick: u64,
    pub(super) queue: Queue<Job>,
}

impl Thumbs {
    /// Starts a new frame of use (least-recently-used order).
    pub fn frame(&mut self) {
        self.tick += 1;
    }

    /// The picture for `key`, queuing its decode on a miss (call
    /// [`super::kick_thumbs`] after rendering when this returns `None`).
    pub fn get(&mut self, shelf: u64, key: Key, path: &Path) -> Option<&Thumb> {
        let tick = self.tick;
        if let std::collections::hash_map::Entry::Vacant(e) = self.slots.entry(key) {
            e.insert(Slot { thumb: Thumb::Loading, shelf, bytes: 0, used: tick });
            self.queue.extend([Job { key, path: path.to_path_buf() }]);
        }
        let slot = self.slots.get_mut(&key)?;
        slot.used = tick;
        Some(&slot.thumb)
    }

    /// A decode finished (`None`: it failed). Results for slots released
    /// meanwhile are dropped, never cached again.
    pub fn land(&mut self, key: Key, image: Option<Arc<RenderImage>>, mut drop_image: impl FnMut(Arc<RenderImage>)) {
        let Some(slot) = self.slots.get_mut(&key) else {
            if let Some(image) = image {
                drop_image(image);
            }
            return;
        };
        match image {
            Some(image) => {
                slot.bytes = key.w as usize * key.h as usize * RGBA;
                slot.thumb = Thumb::Ready(image);
            }
            None => slot.thumb = Thumb::Failed,
        }
        self.enforce_budget(drop_image);
    }

    pub fn bytes(&self) -> usize {
        self.slots.values().map(|s| s.bytes).sum()
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Evicts least-recently-used pictures (never ones used this frame).
    fn enforce_budget(&mut self, mut drop_image: impl FnMut(Arc<RenderImage>)) {
        while self.bytes() > BUDGET_BYTES {
            let tick = self.tick;
            let Some(victim) = self.slots.iter().filter(|(_, s)| s.bytes > 0 && s.used != tick).min_by_key(|(_, s)| s.used).map(|(k, _)| *k) else { break };
            if let Some(Slot { thumb: Thumb::Ready(image), .. }) = self.slots.remove(&victim) {
                drop_image(image);
            }
        }
    }

    /// Releases a shelf's pictures (it was minimized or closed). Returns
    /// whether anything was freed.
    pub fn release_shelf(&mut self, shelf: u64, mut drop_image: impl FnMut(Arc<RenderImage>)) -> bool {
        let before = self.slots.len();
        self.slots.retain(|_, s| {
            if s.shelf != shelf {
                return true;
            }
            if let Thumb::Ready(image) = std::mem::replace(&mut s.thumb, Thumb::Failed) {
                drop_image(image);
            }
            false
        });
        let freed = self.slots.len() != before;
        if freed {
            self.slots.shrink_to_fit();
        }
        freed
    }

    /// Releases one item's pictures (it was removed).
    pub fn release_item(&mut self, item: u64, mut drop_image: impl FnMut(Arc<RenderImage>)) {
        self.slots.retain(|k, s| {
            if k.item != item {
                return true;
            }
            if let Thumb::Ready(image) = std::mem::replace(&mut s.thumb, Thumb::Failed) {
                drop_image(image);
            }
            false
        });
    }

    /// Releases everything and stops queued decodes (Stash switched off).
    pub fn release_all(&mut self, mut drop_image: impl FnMut(Arc<RenderImage>)) {
        self.queue.clear();
        for (_, slot) in self.slots.drain() {
            if let Thumb::Ready(image) = slot.thumb {
                drop_image(image);
            }
        }
        self.slots.shrink_to_fit();
    }
}

/// Decodes `path` at `key`'s size: a cover crop for tiles, a fit for quick
/// look. The retained image header is allocated before the decoder's
/// buffers, and the full-size image is dropped before the pixels are
/// swizzled.
pub fn decode(path: &Path, key: Key) -> Option<Arc<RenderImage>> {
    let prepared = PreparedImage::new();
    let mut reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIM);
    limits.max_image_height = Some(MAX_DIM);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let picture = reader.decode().ok()?;
    let rgba = if key.cover {
        picture.resize_to_fill(key.w.max(1), key.h.max(1), image::imageops::FilterType::Triangle).into_rgba8()
    } else {
        picture.resize(key.w.max(1), key.h.max(1), image::imageops::FilterType::Triangle).into_rgba8()
    };
    drop(picture);
    Some(prepared.finish(rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32) -> Arc<RenderImage> {
        Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(image::RgbaImage::new(w, h))]))
    }

    #[test]
    fn the_cache_is_bounded_and_released_per_shelf() {
        let mut t = Thumbs::default();
        let mut dropped = 0;
        // 120×120 ×4 = 57.6 KB each: 200 of them exceed the budget.
        for i in 0..200u64 {
            t.frame();
            let key = Key { item: i, w: 120, h: 120, cover: true };
            assert!(matches!(t.get(i % 2, key, Path::new("/x")), Some(Thumb::Loading)));
            t.land(key, Some(image(1, 1)), |_| dropped += 1);
            assert!(t.bytes() <= BUDGET_BYTES);
        }
        assert!(dropped > 0, "old pictures were evicted");
        let before = t.len();
        assert!(t.release_shelf(0, |_| dropped += 1));
        assert!(t.len() < before);
        assert!(t.slots.values().all(|s| s.shelf == 1));
        t.release_all(|_| {});
        assert_eq!((t.len(), t.bytes()), (0, 0));
        // A late result for a released slot is dropped, not cached.
        let mut late = 0;
        t.land(Key { item: 1, w: 120, h: 120, cover: true }, Some(image(1, 1)), |_| late += 1);
        assert_eq!((late, t.len()), (1, 0));
    }

    #[test]
    fn decodes_at_the_size_asked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.png");
        image::RgbaImage::from_pixel(640, 320, image::Rgba([10, 20, 30, 255])).save(&path).unwrap();
        let cover = decode(&path, Key { item: 1, w: 96, h: 96, cover: true }).unwrap();
        assert_eq!((cover.size(0).width.0, cover.size(0).height.0), (96, 96));
        let fit = decode(&path, Key { item: 1, w: 200, h: 200, cover: false }).unwrap();
        assert_eq!((fit.size(0).width.0, fit.size(0).height.0), (200, 100));
        assert!(decode(&dir.path().join("missing.png"), Key { item: 1, w: 9, h: 9, cover: true }).is_none());
    }
}
