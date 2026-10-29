//! Colour app icons decoded at the size they are drawn.
//!
//! `img(path)` decodes through GPUI's resource loader at the file's own
//! size: an SVG app icon renders at twice its intrinsic size (a 512 px
//! canvas becomes a 1024² BGRA bitmap, 4 MiB) and a PNG at whatever the
//! theme ships. The dock's Applications grid showed ~100 of them at 44 px
//! and held ~90 MiB. [`icon`] renders through [`SizedIcon`], an asset keyed
//! by path *and* device-pixel size, so each icon costs its drawn size
//! (66² × 4 B ≈ 17 KiB at 44 px × 1.5); [`forget`] drops one again.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use gpui::{App, Asset, ImageCacheError, ImageSource, RenderImage, Window};

/// Device pixels are rounded up to a multiple of this, so a size that
/// springs a little (magnification, hover) reuses one bitmap.
const SIZE_STEP_PX: u32 = 8;
/// Largest bitmap edge decoded for an icon.
const MAX_EDGE_PX: u32 = 512;

/// A colour icon file at a device-pixel edge.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct IconKey {
    pub path: Arc<Path>,
    pub edge: u32,
}

/// [`Asset`] that decodes [`IconKey`]s off the UI thread.
pub enum SizedIcon {}

impl Asset for SizedIcon {
    type Source = IconKey;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(source: Self::Source, cx: &mut App) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        cx.background_executor().spawn(async move { decode(&source.path, source.edge) })
    }
}

/// The device-pixel edge an icon of `size` logical px is decoded at.
pub fn edge_for(size: f32, scale: f32) -> u32 {
    let px = (size.max(1.0) * scale.max(0.5)).ceil() as u32;
    px.div_ceil(SIZE_STEP_PX).saturating_mul(SIZE_STEP_PX).clamp(SIZE_STEP_PX, MAX_EDGE_PX)
}

thread_local! {
    /// The edge each icon file was last drawn at, so a size change keeps
    /// drawing that bitmap while the new one decodes instead of blanking
    /// for a frame or two (one small entry per icon file).
    static LAST_EDGE: RefCell<HashMap<Arc<Path>, u32>> = RefCell::new(HashMap::new());
}

/// An image source that draws `path` decoded for `size` logical px.
pub fn source(path: &Path, size: f32) -> ImageSource {
    let path: Arc<Path> = Arc::from(path);
    ImageSource::Custom(Arc::new(move |window: &mut Window, cx: &mut App| {
        let edge = edge_for(size, window.scale_factor());
        match window.use_asset::<SizedIcon>(&IconKey { path: path.clone(), edge }, cx) {
            Some(ready) => {
                if ready.is_ok() {
                    LAST_EDGE.with_borrow_mut(|last| last.insert(path.clone(), edge));
                }
                Some(ready)
            }
            None => {
                let previous = LAST_EDGE.with_borrow(|last| last.get(&path).copied()).filter(|e| *e != edge)?;
                window.get_asset::<SizedIcon>(&IconKey { path: path.clone(), edge: previous }, cx)
            }
        }
    }))
}

/// Drops `path`'s decoded bitmap for `size` (and its atlas texture in
/// `window`); it is decoded again when next drawn.
pub fn forget(path: &Path, size: f32, window: &mut Window, cx: &mut App) {
    let key = IconKey { path: Arc::from(path), edge: edge_for(size, window.scale_factor()) };
    if let Some(Ok(image)) = window.get_asset::<SizedIcon>(&key, cx) {
        cx.drop_image(image, Some(window));
    }
    cx.remove_asset::<SizedIcon>(&key);
}

/// [`forget`] without a window (the window and its atlas are gone): drops
/// the decoded bitmap drawn at `size` on a `scale` output.
pub fn forget_cached(path: &Path, size: f32, scale: f32, cx: &mut App) {
    cx.remove_asset::<SizedIcon>(&IconKey { path: Arc::from(path), edge: edge_for(size, scale) });
}

fn decode(path: &Path, edge: u32) -> Result<Arc<RenderImage>, ImageCacheError> {
    let bytes = std::fs::read(path)?;
    let is_svg = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg") || e.eq_ignore_ascii_case("svgz"));
    let rgba = if is_svg { render_svg(&bytes, edge)? } else { scale_raster(&bytes, edge)? };
    Ok(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(to_bgra(rgba))])))
}

/// The SVG fitted into an `edge`² canvas (aspect kept, centred).
fn render_svg(bytes: &[u8], edge: u32) -> Result<image::RgbaImage, ImageCacheError> {
    let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default()).map_err(|e| ImageCacheError::Asset(e.to_string().into()))?;
    let size = tree.size();
    let k = edge as f32 / size.width().max(size.height()).max(1.0);
    let (w, h) = (((size.width() * k).round() as u32).max(1), ((size.height() * k).round() as u32).max(1));
    let mut pm = tiny_skia::Pixmap::new(w, h).ok_or_else(|| ImageCacheError::Asset("bad icon size".into()))?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(k, k), &mut pm.as_mut());
    // tiny-skia is premultiplied; RenderImage wants straight alpha.
    let mut data = pm.take();
    for px in data.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    image::RgbaImage::from_raw(w, h, data).ok_or_else(|| ImageCacheError::Asset("bad icon buffer".into()))
}

/// A raster icon, shrunk to fit `edge` when larger (never enlarged: GPUI
/// scales it up on the GPU).
fn scale_raster(bytes: &[u8], edge: u32) -> Result<image::RgbaImage, ImageCacheError> {
    let img = image::load_from_memory(bytes).map_err(|e| ImageCacheError::Image(Arc::new(e)))?;
    if img.width().max(img.height()) <= edge {
        return Ok(img.into_rgba8());
    }
    Ok(img.resize(edge, edge, image::imageops::FilterType::Triangle).into_rgba8())
}

fn to_bgra(mut rgba: image::RgbaImage) -> image::RgbaImage {
    for px in rgba.pixels_mut() {
        px.0.swap(0, 2);
    }
    rgba
}

/// Whether a path is drawn through [`source`] (everything `img` handles
/// as a still image: SVG, PNG, JPEG, WebP).
pub fn handles(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "svg" | "svgz" | "png" | "jpg" | "jpeg" | "webp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_round_up_and_stay_bounded() {
        assert_eq!(edge_for(44.0, 1.5), 72);
        assert_eq!(edge_for(44.0, 1.0), 48);
        assert_eq!(edge_for(0.0, 1.0), SIZE_STEP_PX);
        assert_eq!(edge_for(4000.0, 2.0), MAX_EDGE_PX);
    }

    #[test]
    fn svg_and_png_decode_at_the_drawn_size() {
        let dir = std::env::temp_dir().join(format!("mandara-icon-raster-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let svg = dir.join("big.svg");
        std::fs::write(&svg, r##"<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="512"><rect width="1024" height="512" fill="#ff0000"/></svg>"##).unwrap();
        let img = decode(&svg, 72).unwrap();
        assert_eq!((img.size(0).width.0, img.size(0).height.0), (72, 36), "fitted, aspect kept");
        // BGRA: red lands in the third byte.
        let first = &img.as_bytes(0).unwrap()[..4];
        assert_eq!((first[2], first[0], first[3]), (255, 0, 255));
        let png = dir.join("big.png");
        image::RgbaImage::from_pixel(256, 256, image::Rgba([0, 0, 255, 255])).save(&png).unwrap();
        assert_eq!(decode(&png, 48).unwrap().size(0).width.0, 48);
        let small = dir.join("small.png");
        image::RgbaImage::from_pixel(16, 16, image::Rgba([0, 0, 255, 255])).save(&small).unwrap();
        assert_eq!(decode(&small, 48).unwrap().size(0).width.0, 16, "never enlarged");
        assert!(handles(&svg) && handles(&png) && !handles(&dir.join("x.xpm")));
        let _ = std::fs::remove_dir_all(dir);
    }
}
