//! Disk-backed dock icon atlas: decode → optional squircle bake → cache under
//! `~/.cache/mandara/dock-icons/`, then hand a bounded RGBA tile to GPUI and
//! drop the CPU buffer.
//!
//! Cap is 128×128 RGBA8 (64 KiB). Content is hashed with `sha2` (workspace).
//! Bakes never run on the UI thread: [`peek_or_queue`] only returns a ready
//! tile, and schedules a background bake on miss.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use sha2::{Digest, Sha256};

/// Largest dock atlas edge, device pixels.
pub const ATLAS_EDGE: u32 = 128;
/// Cap for source icon files (same order as stack image previews).
pub const ICON_MAX_BYTES: u64 = 4 * 1024 * 1024;
/// Optical inset of irregular icons inside a baked squircle (share of edge).
const INSET_FRAC: f32 = 0.10;
/// Superellipse exponent for continuous-curvature squircles.
const SQUIRCLE_N: f32 = 4.5;
/// Soft mask edge width in normalised units (~1 px at 128).
const MASK_AA: f32 = 0.04;
/// Corner alpha samples used to detect an already-squircle silhouette.
const CORNER_PROBE: u32 = 3;
/// Fill ratio above which an opaque rounded square is left alone.
const FULL_BLEED_FILL: f32 = 0.82;

type CacheKey = (PathBuf, u32, bool);

/// Ready atlas paths (or remembered misses). Shared with bake threads.
static RESOLVED: LazyLock<Mutex<HashMap<CacheKey, Option<Arc<Path>>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Bakes already scheduled for this process.
static PENDING: LazyLock<Mutex<HashSet<CacheKey>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Bumped when a background bake finishes so the dock can redraw.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Current bake generation (dock compares against its last seen value).
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// Drops in-process atlas lookups (icon theme / config changed).
pub fn clear() {
    if let Ok(mut c) = RESOLVED.lock() {
        c.clear();
    }
    if let Ok(mut p) = PENDING.lock() {
        p.clear();
    }
}

/// XDG cache directory for normalised dock tiles.
pub fn cache_dir() -> PathBuf {
    mandara_config::paths::cache_dir().join("dock-icons")
}

fn key(source: &Path, edge: u32, squircle: bool) -> CacheKey {
    (source.to_path_buf(), edge, squircle)
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
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    rgba.save(&tmp).map_err(std::io::Error::other)?;
    fs::rename(tmp, path)?;
    Ok(())
}

/// Ready atlas path for a paint, or `None` while a bake is pending / failed.
/// Never decodes or bakes on the calling (UI) thread.
pub fn peek_or_queue(path: &Arc<Path>, edge: u32, auto_squircle: bool) -> Option<Arc<Path>> {
    let edge = edge.clamp(8, ATLAS_EDGE);
    if !auto_squircle {
        return None;
    }
    let k = key(path, edge, auto_squircle);
    if let Ok(map) = RESOLVED.lock()
        && let Some(hit) = map.get(&k)
    {
        return hit.clone();
    }
    let cache = cache_path(path, edge, auto_squircle);
    if cache.is_file() {
        let cached: Arc<Path> = Arc::from(cache.into_boxed_path());
        if let Ok(mut map) = RESOLVED.lock() {
            map.insert(k.clone(), Some(cached.clone()));
        }
        return Some(cached);
    }
    let already = match PENDING.lock() {
        Ok(mut pending) => !pending.insert(k.clone()),
        Err(_) => return None,
    };
    if already {
        return None;
    }
    let src = path.clone();
    let bake_key = k.clone();
    let spawned = std::thread::Builder::new()
        .name("mandara-dock-atlas".into())
        .spawn(move || {
            let outcome = bake_to_disk(&src, edge, auto_squircle);
            if let Ok(mut map) = RESOLVED.lock() {
                map.insert(bake_key.clone(), outcome);
            }
            if let Ok(mut pending) = PENDING.lock() {
                pending.remove(&bake_key);
            }
            GENERATION.fetch_add(1, Ordering::Relaxed);
        });
    if spawned.is_err()
        && let Ok(mut pending) = PENDING.lock()
    {
        pending.remove(&k);
    }
    None
}

fn bake_to_disk(path: &Path, edge: u32, auto_squircle: bool) -> Option<Arc<Path>> {
    let cache = cache_path(path, edge, auto_squircle);
    if cache.is_file() {
        return Some(Arc::from(cache.into_boxed_path()));
    }
    let tile = normalize_file(path, edge, auto_squircle)?;
    match store_cached(&cache, &tile) {
        Ok(()) => Some(Arc::from(cache.into_boxed_path())),
        Err(_) => {
            // Disk write failed: still remember the miss so we don't spin.
            None
        }
    }
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

/// Continuous-curvature squircle coverage with a soft AA rim.
fn squircle_coverage(x: f32, y: f32, n: f32) -> f32 {
    let v = x.abs().powf(n) + y.abs().powf(n);
    if v <= 1.0 - MASK_AA {
        1.0
    } else if v >= 1.0 + MASK_AA {
        0.0
    } else {
        let t = ((1.0 + MASK_AA) - v) / (2.0 * MASK_AA);
        t.clamp(0.0, 1.0)
    }
}

/// Bake `src` into an `edge`² freestanding squircle tile (inset only — no plate wash).
pub fn bake_squircle(src: image::RgbaImage, edge: u32) -> image::RgbaImage {
    let edge = edge.clamp(8, ATLAS_EDGE);
    let mut out = image::RgbaImage::new(edge, edge);
    let _tone = dominant_rgb(&src);
    let inset = (edge as f32 * INSET_FRAC).round().max(1.0);
    let inner = (edge as f32 - inset * 2.0).max(1.0);
    let (sw, sh) = src.dimensions();
    let scale = inner / (sw.max(sh).max(1) as f32);
    let nw = (sw as f32 * scale).round().max(1.0) as u32;
    let nh = (sh as f32 * scale).round().max(1.0) as u32;
    let resized = image::imageops::resize(&src, nw, nh, image::imageops::FilterType::Triangle);
    drop(src);
    let ox = ((edge - nw) / 2) as i32;
    let oy = ((edge - nh) / 2) as i32;
    let half = (edge as f32 - 1.0) * 0.5;

    for y in 0..edge {
        for x in 0..edge {
            let nx = (x as f32 - half) / half;
            let ny = (y as f32 - half) / half;
            let mask = squircle_coverage(nx, ny, SQUIRCLE_N);
            if mask <= 0.0 {
                continue;
            }
            let mut px = [0u8; 4];
            let sx = x as i32 - ox;
            let sy = y as i32 - oy;
            if sx >= 0 && sy >= 0 && (sx as u32) < nw && (sy as u32) < nh {
                let s = resized.get_pixel(sx as u32, sy as u32).0;
                if s[3] > 0 {
                    let a = f32::from(s[3]) / 255.0 * mask;
                    px = [s[0], s[1], s[2], (a * 255.0) as u8];
                }
            }
            // Specular on the squircle's top rim only (first opaque row per column).
            if mask > 0.5 && y > 0 {
                let above = squircle_coverage((x as f32 - half) / half, ((y - 1) as f32 - half) / half, SQUIRCLE_N);
                if above < 0.25 {
                    let a = 0.22 * mask;
                    for c in &mut px[..3] {
                        *c = (255.0 * a + f32::from(*c) * (1.0 - a)) as u8;
                    }
                    px[3] = px[3].max((a * 255.0) as u8);
                }
            }
            if px[3] > 0 {
                out.put_pixel(x, y, image::Rgba(px));
            }
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
/// cache. Returns RGBA8; caller uploads then drops. Safe for background threads.
pub fn normalize_file(path: &Path, edge: u32, auto_squircle: bool) -> Option<image::RgbaImage> {
    let edge = edge.clamp(8, ATLAS_EDGE);
    let cache = cache_path(path, edge, auto_squircle);
    if let Some(hit) = load_cached(&cache, edge) {
        return Some(hit);
    }
    let meta = fs::metadata(path).ok()?;
    if meta.len() > ICON_MAX_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    if bytes.len() as u64 > ICON_MAX_BYTES {
        return None;
    }
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
    let (sw, sh) = img.dimensions();
    if sw.max(sh) <= edge {
        return Some(img);
    }
    // Keep aspect ratio (never stretch to a square).
    let scale = edge as f32 / sw.max(sh) as f32;
    let nw = (sw as f32 * scale).round().max(1.0) as u32;
    let nh = (sh as f32 * scale).round().max(1.0) as u32;
    Some(image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle))
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
        // Keep tests out of the developer's real cache.
        // SAFETY: single-threaded test; restored when `dir` drops with the process.
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", dir.path());
        }
        let src = dir.path().join("blob.png");
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
        // No mid-icon white stripe from a misplaced specular row.
        let mid = tile.get_pixel(32, 20).0;
        assert!(mid[3] == 0 || mid[0] < 250, "specular must not paint a full white row through content");
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

    #[test]
    fn oversized_sources_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", dir.path());
        }
        let src = dir.path().join("huge.bin");
        fs::write(&src, vec![0u8; (ICON_MAX_BYTES as usize) + 8]).unwrap();
        assert!(normalize_file(&src, 64, true).is_none());
    }
}
