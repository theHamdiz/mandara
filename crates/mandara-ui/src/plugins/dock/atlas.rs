//! Disk-backed dock icon atlas: decode → optional squircle bake → cache under
//! `~/.cache/mandara/dock-icons/`, then hand a bounded RGBA tile to GPUI and
//! drop the CPU buffer.
//!
//! Cap is 128×128 RGBA8 (64 KiB). Content is hashed with `sha2` (workspace).

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};

thread_local! {
    /// Resolved atlas paths for this process (avoids disk `is_file` / bake on every paint).
    /// Keyed by source path pointer + edge + squircle so paints only bump a refcount.
    static RESOLVED: RefCell<HashMap<(usize, u32, bool), Arc<Path>>> = RefCell::new(HashMap::new());
}

/// Largest dock atlas edge, device pixels.
pub const ATLAS_EDGE: u32 = 128;
/// Optical inset of irregular icons inside a baked squircle (share of edge).
const INSET_FRAC: f32 = 0.14;
/// Superellipse exponent for continuous-curvature squircles.
const SQUIRCLE_N: f32 = 4.5;
/// Top specular highlight alpha on baked tiles.
const SPECULAR_A: u8 = 46; // ≈ 0.18 × 255
/// Corner alpha samples used to detect an already-squircle silhouette.
const CORNER_PROBE: u32 = 3;
/// Fill ratio above which an opaque rounded square is left alone.
const FULL_BLEED_FILL: f32 = 0.82;

/// XDG cache directory for normalised dock tiles.
pub fn cache_dir() -> PathBuf {
    mandara_config::paths::cache_dir().join("dock-icons")
}

/// Content-addressed cache path for a source file + bake options.
pub fn cache_path(source: &Path, edge: u32, squircle: bool) -> PathBuf {
    let mut h = Sha256::new();
    h.update(source.to_string_lossy().as_bytes());
    if let Ok(meta) = fs::metadata(source) {
        h.update(meta.len().to_le_bytes());
        if let Ok(modified) = meta.modified()
            && let Ok(d) = modified.duration_since(std::time::UNIX_EPOCH)
        {
            h.update(d.as_secs().to_le_bytes());
            h.update(d.subsec_nanos().to_le_bytes());
        }
    }
    h.update(edge.to_le_bytes());
    h.update([u8::from(squircle)]);
    let dig = h.finalize();
    // PNG so GPUI / `icon_raster` can load the tile without a custom decoder.
    cache_dir().join(format!("{dig:x}.png"))
}

/// Load a cached `edge`² tile, or `None` if missing / wrong size.
pub fn load_cached(path: &Path, edge: u32) -> Option<image::RgbaImage> {
    let img = image::open(path).ok()?.into_rgba8();
    if img.width() != edge || img.height() != edge {
        return None;
    }
    Some(img)
}

/// Persist an `edge`² tile as PNG (creates the cache directory).
pub fn store_cached(path: &Path, rgba: &image::RgbaImage) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("png.tmp");
    rgba.save(&tmp).map_err(std::io::Error::other)?;
    fs::rename(tmp, path)?;
    Ok(())
}

/// Resolve a dock icon file to a cached atlas path (baking when needed).
/// Returns the cache path after ensuring the tile exists; CPU buffers are dropped.
/// Remembers successful resolutions in-process so paints never re-stat the disk.
pub fn ensure_atlas(path: &Arc<Path>, edge: u32, auto_squircle: bool) -> Option<Arc<Path>> {
    let edge = edge.clamp(8, ATLAS_EDGE);
    let key = (Arc::as_ptr(path) as *const () as usize, edge, auto_squircle);
    if let Some(hit) = RESOLVED.with(|c| c.borrow().get(&key).cloned()) {
        return Some(hit);
    }
    let cache = cache_path(path, edge, auto_squircle);
    if !cache.is_file() {
        // `normalize_file` writes the cache and drops the full-res decode buffers.
        let tile = normalize_file(path.as_ref(), edge, auto_squircle)?;
        drop(tile);
        if !cache.is_file() {
            return None;
        }
    }
    let cached: Arc<Path> = Arc::from(cache.into_boxed_path());
    RESOLVED.with(|c| c.borrow_mut().insert(key, cached.clone()));
    Some(cached)
}

/// Whether `img` already looks like a full-bleed rounded square / squircle
/// (opaque centre, transparent corners, high fill).
pub fn looks_like_squircle(img: &image::RgbaImage) -> bool {
    let (w, h) = img.dimensions();
    if w < 8 || h < 8 {
        return false;
    }
    let mut opaque = 0u32;
    let total = w.saturating_mul(h).max(1);
    for px in img.pixels() {
        if px.0[3] > 24 {
            opaque += 1;
        }
    }
    let fill = opaque as f32 / total as f32;
    if fill < FULL_BLEED_FILL {
        return false;
    }
    // Corners should be mostly transparent for a rounded silhouette.
    let mut corner_alpha = 0u32;
    let mut samples = 0u32;
    for (x0, y0) in [(0, 0), (w - CORNER_PROBE, 0), (0, h - CORNER_PROBE), (w - CORNER_PROBE, h - CORNER_PROBE)] {
        for dy in 0..CORNER_PROBE {
            for dx in 0..CORNER_PROBE {
                let x = x0.saturating_add(dx).min(w - 1);
                let y = y0.saturating_add(dy).min(h - 1);
                corner_alpha += u32::from(img.get_pixel(x, y).0[3]);
                samples += 1;
            }
        }
    }
    let mean = corner_alpha as f32 / samples.max(1) as f32;
    mean < 40.0
}

/// Sample the dominant opaque hue (average RGB of opaque pixels) for a
/// subtle top-down tonal wash behind irregular icons.
fn dominant_rgb(img: &image::RgbaImage) -> [u8; 3] {
    let mut sum = [0u64; 3];
    let mut n = 0u64;
    for px in img.pixels() {
        if px.0[3] > 64 {
            sum[0] += u64::from(px.0[0]);
            sum[1] += u64::from(px.0[1]);
            sum[2] += u64::from(px.0[2]);
            n += 1;
        }
    }
    if n == 0 {
        return [36, 40, 48];
    }
    [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8]
}

/// Continuous-curvature squircle mask: `|x|^n + |y|^n <= 1` in [-1,1]².
fn squircle_mask(x: f32, y: f32, n: f32) -> f32 {
    let ax = x.abs().powf(n);
    let ay = y.abs().powf(n);
    if ax + ay <= 1.0 {
        1.0
    } else {
        0.0
    }
}

/// Bake `src` into an `edge`² freestanding squircle tile (inset + tonal
/// wash + top specular). The source is consumed; callers must drop it.
pub fn bake_squircle(src: image::RgbaImage, edge: u32) -> image::RgbaImage {
    let edge = edge.clamp(8, ATLAS_EDGE);
    let mut out = image::RgbaImage::new(edge, edge);
    let tone = dominant_rgb(&src);
    let inset = (edge as f32 * INSET_FRAC).round().max(1.0);
    let inner = (edge as f32 - inset * 2.0).max(1.0);
    let (sw, sh) = src.dimensions();
    let scale = inner / (sw.max(sh).max(1) as f32);
    let dw = (sw as f32 * scale).round().max(1.0);
    let dh = (sh as f32 * scale).round().max(1.0);
    let ox = ((edge as f32 - dw) * 0.5).round();
    let oy = ((edge as f32 - dh) * 0.5).round();
    let half = (edge as f32 - 1.0) * 0.5;

    for y in 0..edge {
        for x in 0..edge {
            let nx = (x as f32 - half) / half;
            let ny = (y as f32 - half) / half;
            let mask = squircle_mask(nx, ny, SQUIRCLE_N);
            if mask <= 0.0 {
                continue;
            }
            // Soft top-down tonal wash from the icon's dominant colour.
            let t = y as f32 / edge as f32;
            let wash = [
                ((tone[0] as f32) * (0.55 + 0.25 * (1.0 - t))) as u8,
                ((tone[1] as f32) * (0.55 + 0.25 * (1.0 - t))) as u8,
                ((tone[2] as f32) * (0.55 + 0.25 * (1.0 - t))) as u8,
                (38.0 * mask) as u8,
            ];
            let mut px = wash;
            // Sample the inset icon.
            let ix = ((x as f32 - ox) / scale).floor() as i32;
            let iy = ((y as f32 - oy) / scale).floor() as i32;
            if ix >= 0 && iy >= 0 && (ix as u32) < sw && (iy as u32) < sh {
                let s = src.get_pixel(ix as u32, iy as u32).0;
                if s[3] > 0 {
                    let a = f32::from(s[3]) / 255.0 * mask;
                    for (channel, src_c) in px[..3].iter_mut().zip(s[..3].iter()) {
                        *channel = (f32::from(*src_c) * a + f32::from(*channel) * (1.0 - a)) as u8;
                    }
                    px[3] = ((a * 255.0) as u8).max(px[3]);
                }
            }
            // 1 px top inner specular.
            if y == (inset as u32).saturating_add(1).min(edge - 1) && mask > 0.5 {
                let a = f32::from(SPECULAR_A) / 255.0;
                for channel in &mut px[..3] {
                    *channel = (255.0 * a + f32::from(*channel) * (1.0 - a)) as u8;
                }
                px[3] = px[3].max(SPECULAR_A);
            }
            out.put_pixel(x, y, image::Rgba(px));
        }
    }
    out
}

/// Fit `src` into an `edge`² canvas (centred, aspect kept) without a plate.
pub fn fit_tile(src: image::RgbaImage, edge: u32) -> image::RgbaImage {
    let edge = edge.clamp(8, ATLAS_EDGE);
    let (sw, sh) = src.dimensions();
    if sw == edge && sh == edge {
        return src;
    }
    let scale = edge as f32 / sw.max(sh).max(1) as f32;
    let nw = (sw as f32 * scale).round().max(1.0) as u32;
    let nh = (sh as f32 * scale).round().max(1.0) as u32;
    let resized = image::imageops::resize(&src, nw, nh, image::imageops::FilterType::Triangle);
    drop(src);
    let mut out = image::RgbaImage::new(edge, edge);
    let ox = (edge - nw) / 2;
    let oy = (edge - nh) / 2;
    image::imageops::overlay(&mut out, &resized, i64::from(ox), i64::from(oy));
    out
}

/// Decode `path` at most `edge`, optionally bake a squircle, using the disk
/// cache. Returns RGBA8; caller uploads then drops.
pub fn normalize_file(path: &Path, edge: u32, auto_squircle: bool) -> Option<image::RgbaImage> {
    let edge = edge.clamp(8, ATLAS_EDGE);
    let cache = cache_path(path, edge, auto_squircle);
    if let Some(hit) = load_cached(&cache, edge) {
        return Some(hit);
    }
    let bytes = fs::read(path).ok()?;
    let is_svg = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg") || e.eq_ignore_ascii_case("svgz"));
    let mut rgba = if is_svg { render_svg(&bytes, edge)? } else { decode_raster(&bytes, edge)? };
    let tile = if auto_squircle && !looks_like_squircle(&rgba) {
        bake_squircle(std::mem::take(&mut rgba), edge)
    } else {
        fit_tile(std::mem::take(&mut rgba), edge)
    };
    let _ = store_cached(&cache, &tile);
    Some(tile)
}

fn render_svg(bytes: &[u8], edge: u32) -> Option<image::RgbaImage> {
    let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    let k = edge as f32 / size.width().max(size.height()).max(1.0);
    let (w, h) = (((size.width() * k).round() as u32).max(1), ((size.height() * k).round() as u32).max(1));
    let mut pm = tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(k, k), &mut pm.as_mut());
    let mut data = pm.take();
    for px in data.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    image::RgbaImage::from_raw(w, h, data)
}

fn decode_raster(bytes: &[u8], edge: u32) -> Option<image::RgbaImage> {
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    if img.width().max(img.height()) <= edge {
        return Some(img);
    }
    Some(image::imageops::resize(&img, edge, edge, image::imageops::FilterType::Triangle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_edge_is_128() {
        assert_eq!(ATLAS_EDGE, 128);
    }

    #[test]
    fn irregular_icons_bake_and_cache() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("blob.png");
        // A small opaque blob in the centre — not a full-bleed squircle.
        let mut img = image::RgbaImage::new(64, 64);
        for y in 20..44 {
            for x in 20..44 {
                img.put_pixel(x, y, image::Rgba([200, 40, 40, 255]));
            }
        }
        img.save(&src).unwrap();
        assert!(!looks_like_squircle(&img));
        let tile = normalize_file(&src, 64, true).expect("bake");
        assert_eq!(tile.dimensions(), (64, 64));
        assert!(tile.pixels().any(|p| p.0[3] > 0));
        let again = normalize_file(&src, 64, true).expect("cache hit");
        assert_eq!(tile.as_raw(), again.as_raw());
    }

    #[test]
    fn full_bleed_squircle_is_detected() {
        let mut img = image::RgbaImage::new(64, 64);
        let half = 31.5_f32;
        for y in 0..64 {
            for x in 0..64 {
                let nx = (x as f32 - half) / half;
                let ny = (y as f32 - half) / half;
                let a = if nx.abs().powf(4.5) + ny.abs().powf(4.5) <= 1.0 { 255 } else { 0 };
                img.put_pixel(x, y, image::Rgba([30, 140, 220, a]));
            }
        }
        assert!(looks_like_squircle(&img));
        let fit = fit_tile(img, 128);
        assert_eq!(fit.dimensions(), (128, 128));
    }

    #[test]
    fn cache_paths_differ_by_options() {
        let p = Path::new("/tmp/demo.png");
        assert_ne!(cache_path(p, 128, true), cache_path(p, 128, false));
        assert_ne!(cache_path(p, 64, true), cache_path(p, 128, true));
    }
}
