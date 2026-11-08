//! Full-color artwork for built-in plugins (Studio tiles and detail modals):
//! a 128 px and a 256 px icon and a 960×540 cover per plugin, embedded in
//! the binary. `cargo xtask art` builds them from `assets/plugins/<id>/`
//! into `assets/plugin-art/` (committed: they are build inputs) together
//! with the table below.
//!
//! The bytes are embedded compressed (icons as lossless WebP, covers as
//! JPEG, or lossless WebP when transparent); each image is wrapped in
//! one shared `Arc<Image>` on first use, so GPUI decodes it once and its
//! image cache keeps serving the same texture.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use gpui::{Image, ImageFormat, RenderImage};

/// (id, 1× icon, 2× icon, cover, cover format), generated.
type Entry = (&'static str, &'static [u8], &'static [u8], &'static [u8], ImageFormat);
const ART: &[Entry] = include!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/plugin-art/index.rs"));

/// Side of the 1× icon in pixels; the 2× icon is twice that.
pub const ICON_PX: f32 = 128.0;

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
enum Variant {
    Icon,
    Icon2x,
    Cover,
}

thread_local! {
    static CACHE: RefCell<HashMap<(&'static str, Variant), Arc<Image>>> = RefCell::new(HashMap::new());
}

fn entry(id: &str) -> Option<&'static Entry> {
    ART.iter().find(|e| e.0 == id)
}

fn cached(id: &str, variant: Variant) -> Option<Arc<Image>> {
    let e = entry(id)?;
    let key = (e.0, variant);
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return Some(hit);
    }
    let (bytes, format) = match variant {
        Variant::Icon => (e.1, ImageFormat::Webp),
        Variant::Icon2x => (e.2, ImageFormat::Webp),
        Variant::Cover => (e.3, e.4),
    };
    let image = Arc::new(Image::from_bytes(format, bytes.to_vec()));
    CACHE.with(|c| c.borrow_mut().insert(key, image.clone()));
    Some(image)
}

/// The Mandara app icon (256 px): Studio's About page and the launcher
/// button's default icon.
const BRAND_ICON: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/brand/mandara-256.png"));

thread_local! {
    static BRAND: Arc<Image> = Arc::new(Image::from_bytes(ImageFormat::Png, BRAND_ICON.to_vec()));
}

/// The Mandara app icon, decoded once and shared.
pub fn brand_icon() -> Arc<Image> {
    BRAND.with(Arc::clone)
}

/// A built-in plugin's icon for drawing at `scale` device pixels per 1×
/// icon pixel (`side × window scale ÷ ICON_PX`): above 1 the 256 px
/// variant keeps it sharp.
pub fn icon(id: &str, scale: f32) -> Option<Arc<Image>> {
    cached(id, if scale > 1.0 { Variant::Icon2x } else { Variant::Icon })
}

/// A built-in plugin's 16:9 cover.
pub fn cover(id: &str) -> Option<Arc<Image>> {
    cached(id, Variant::Cover)
}

/// Whether built-in `id` has embedded artwork (icon and cover).
pub fn has_art(id: &str) -> bool {
    entry(id).is_some()
}

/// Bytes per decoded RGBA / BGRA pixel.
const BGRA_CHANNELS: usize = 4;

/// Releases the art handed out so far: each icon and cover decode and its
/// texture (see `gpui::ImageSource::evict`) and the embedded bytes' copies.
/// The next draw decodes again. Studio calls it, from `App::defer`, when it
/// leaves the pages that show plugin art.
pub fn release(cx: &mut gpui::App) {
    for image in take_handed_out() {
        gpui::ImageSource::Image(image).evict(cx);
    }
}

/// Empties the cache of handed-out art (capacity included).
fn take_handed_out() -> impl Iterator<Item = Arc<Image>> {
    CACHE.with(|c| std::mem::take(&mut *c.borrow_mut())).into_values()
}

/// Releases the brand icon's decode and texture (its 256 px source stays).
pub fn release_brand(cx: &mut gpui::App) {
    gpui::ImageSource::Image(brand_icon()).evict(cx);
}

/// Built-in `id`'s cover cropped to fill `w × h` pixels, ready to draw
/// (BGRA): decoded at the size it is shown, never upscaled past the
/// source (a larger box keeps the aspect at the source's resolution).
/// Blocking: call it off the UI thread.
pub fn cover_thumb(id: &str, w: u32, h: u32) -> Option<Arc<RenderImage>> {
    let picture = image::load_from_memory(entry(id)?.3).ok()?;
    Some(fill(picture, w, h))
}

/// [`cover_thumb`] for an image file (a third-party plugin's `cover.png`).
pub fn file_thumb(path: &std::path::Path, w: u32, h: u32) -> Option<Arc<RenderImage>> {
    let picture = image::open(path).ok()?;
    Some(fill(picture, w, h))
}

fn fill(picture: image::DynamicImage, w: u32, h: u32) -> Arc<RenderImage> {
    let (w, h) = (w.max(1), h.max(1));
    // Never upscale: shrink the box until it fits inside the source.
    let k = (picture.width() as f32 / w as f32).min(picture.height() as f32 / h as f32).min(1.0);
    let (w, h) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
    // Cropped and shrunk in place: no float copy of the whole cover.
    let mut rgba = crate::wallpaper::cover(&picture, w, h).into_rgba8();
    drop(picture);
    for px in rgba.as_chunks_mut::<BGRA_CHANNELS>().0 {
        px.swap(0, 2);
    }
    Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(rgba)]))
}

/// The 1× icon (kept for existing callers).
pub fn plugin_art(id: &str) -> Option<Arc<Image>> {
    icon(id, 1.0)
}

/// Every plugin with embedded art.
pub fn ids() -> impl Iterator<Item = &'static str> {
    ART.iter().map(|e| e.0)
}

/// Total embedded bytes (diagnostics, tests).
pub fn embedded_bytes() -> usize {
    ART.iter().map(|e| e.1.len() + e.2.len() + e.3.len()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same ceiling `cargo xtask art` enforces.
    const BUDGET_BYTES: usize = include!("../../../assets/artwork/embedded-budget.rs");

    fn dimensions(bytes: &[u8]) -> (u32, u32) {
        image::load_from_memory(bytes).map(|i| (i.width(), i.height())).unwrap()
    }

    #[test]
    fn every_entry_decodes_at_its_size() {
        assert!(ART.len() >= 24, "built-in art missing — run `cargo xtask art`");
        for (id, small, large, cover, format) in ART {
            assert_eq!(dimensions(small), (128, 128), "{id}");
            assert_eq!(dimensions(large), (256, 256), "{id}");
            assert_eq!(dimensions(cover), (960, 540), "{id}");
            let guessed = image::guess_format(cover).unwrap();
            assert_eq!(*format == ImageFormat::Jpeg, guessed == image::ImageFormat::Jpeg, "{id}");
        }
    }

    #[test]
    fn art_is_cached_and_within_budget() {
        assert!(embedded_bytes() < BUDGET_BYTES, "{} bytes", embedded_bytes());
        let a = cover("weather").unwrap();
        let b = cover("weather").unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(!Arc::ptr_eq(&icon("weather", 1.0).unwrap(), &icon("weather", 2.0).unwrap()));
        assert!(icon("no-such-plugin", 1.0).is_none());
    }

    /// Releasing hands every cached image over (for eviction) and frees the
    /// table; the next use wraps the bytes afresh, with the same GPUI id.
    #[test]
    fn released_art_is_handed_over_and_rebuilt_on_use() {
        let before = icon("weather", 1.0).unwrap();
        let _ = cover("weather").unwrap();
        let taken: Vec<_> = take_handed_out().collect();
        assert!(taken.len() >= 2 && taken.iter().any(|i| Arc::ptr_eq(i, &before)));
        assert!(CACHE.with(|c| c.borrow().is_empty() && c.borrow().capacity() == 0));
        let after = icon("weather", 1.0).unwrap();
        assert!(!Arc::ptr_eq(&before, &after));
        assert_eq!(before.id(), after.id(), "same bytes, same decode key");
    }

    #[test]
    fn thumbs_fill_the_box_without_upscaling() {
        let small = cover_thumb("weather", 320, 180).unwrap();
        assert_eq!((small.size(0).width.0, small.size(0).height.0), (320, 180));
        // A wide hero crop keeps its aspect at the source's resolution.
        let wide = cover_thumb("weather", 3000, 1000).unwrap();
        assert_eq!((wide.size(0).width.0, wide.size(0).height.0), (960, 320));
        assert!(cover_thumb("no-such-plugin", 10, 10).is_none());
    }
}
