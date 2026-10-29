//! Profile photos: square crops with zoom / position, resized to
//! [`AVATAR_SIZE`] in pure Rust, plus procedural Mandara preset avatars
//! (Oklab gradients, Alexandrian motifs and a monogram) — no downloads.
//!
//! The result is written as a PNG under `~/.cache/mandara/avatars/` and
//! handed to AccountsService's `SetIconFile`, which copies it into
//! `/var/lib/AccountsService/icons/`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use image::{DynamicImage, ImageFormat, RgbaImage, imageops::FilterType};
use mandara_i18n::tk;
use mandara_theme::Color;

/// Side of the saved avatar (what greeters and lock screens expect).
pub const AVATAR_SIZE: u32 = 256;
/// Picked photos are shrunk to this before editing (fast previews, no
/// giant GPU textures).
pub const MAX_SOURCE_SIDE: u32 = 1024;
pub const MIN_ZOOM: f32 = 1.0;
pub const MAX_ZOOM: f32 = 4.0;
/// accounts-daemon refuses icons larger than this.
pub const MAX_ICON_BYTES: u64 = 1_048_576;

/// Which part of the photo becomes the avatar.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Crop {
    /// 1 = the largest centered square; 2 = half its side; …
    pub zoom: f32,
    /// Focus point as fractions of the photo's width / height.
    pub center_x: f32,
    pub center_y: f32,
}

impl Default for Crop {
    fn default() -> Self {
        Crop { zoom: MIN_ZOOM, center_x: 0.5, center_y: 0.5 }
    }
}

/// The square `(x, y, side)` in pixels that `crop` selects from a
/// `width`×`height` photo, kept inside the photo.
pub fn crop_rect(width: u32, height: u32, crop: Crop) -> (u32, u32, u32) {
    let zoom = crop.zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    let side = ((width.min(height) as f32 / zoom).round() as u32).clamp(1, width.min(height).max(1));
    let half = side as f32 / 2.0;
    let cx = (crop.center_x.clamp(0.0, 1.0) * width as f32).clamp(half, width as f32 - half);
    let cy = (crop.center_y.clamp(0.0, 1.0) * height as f32).clamp(half, height as f32 - half);
    let x = ((cx - half).round().max(0.0) as u32).min(width.saturating_sub(side));
    let y = ((cy - half).round().max(0.0) as u32).min(height.saturating_sub(side));
    (x, y, side)
}

/// Crops and resizes to an `AVATAR_SIZE` square.
pub fn render_crop(src: &DynamicImage, crop: Crop) -> RgbaImage {
    let (x, y, side) = crop_rect(src.width(), src.height(), crop);
    src.crop_imm(x, y, side, side).resize_exact(AVATAR_SIZE, AVATAR_SIZE, FilterType::Lanczos3).to_rgba8()
}

/// `~/.cache/mandara/avatars/`, created on demand.
pub fn cache_dir() -> PathBuf {
    let dir = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("mandara").join("avatars");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn stamp() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default()
}

/// A picked photo, shrunk for editing.
#[derive(Clone, Debug, PartialEq)]
pub struct Prepared {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
}

fn open_any(path: &Path) -> Result<DynamicImage, String> {
    image::ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(|e| format!("{}: {e}", path.display()))?
        .decode()
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Decodes `path` (PNG / JPEG / WebP), shrinks it to `MAX_SOURCE_SIDE`
/// and stores a working copy for the editor's preview.
pub fn prepare_source(path: &Path) -> Result<Prepared, String> {
    let img = open_any(path)?;
    let img = if img.width().max(img.height()) > MAX_SOURCE_SIDE { img.resize(MAX_SOURCE_SIDE, MAX_SOURCE_SIDE, FilterType::Triangle) } else { img };
    let out = cache_dir().join(format!("source-{}.png", stamp()));
    img.to_rgba8().save_with_format(&out, ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(Prepared { path: out, width: img.width(), height: img.height() })
}

/// Writes the cropped avatar for `SetIconFile`; returns its path.
pub fn write_avatar(source: &Path, crop: Crop) -> Result<PathBuf, String> {
    let img = open_any(source)?;
    save_png(&render_crop(&img, crop), "avatar")
}

fn save_png(img: &RgbaImage, stem: &str) -> Result<PathBuf, String> {
    let out = cache_dir().join(format!("{stem}-{}.png", stamp()));
    img.save_with_format(&out, ImageFormat::Png).map_err(|e| e.to_string())?;
    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    if size > MAX_ICON_BYTES {
        return Err(format!("{} is larger than 1 MB", out.display()));
    }
    Ok(out)
}

// ───────────────────────────── presets ─────────────────────────────

/// A decorative layer over a preset's gradient.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Motif {
    /// Corniche sea waves along the bottom.
    Waves,
    /// A low sun disc over the horizon.
    Sun,
    /// A lighthouse beam sweeping from a corner.
    Beam,
    /// A thin gold ring.
    Ring,
    /// Sea-glass pebbles.
    Pebbles,
    /// A limestone arch.
    Arch,
    /// A crescent moon.
    Crescent,
    /// A scatter of stars.
    Stars,
}

#[derive(Copy, Clone, Debug)]
pub struct Preset {
    /// `tk()`-marked English name.
    pub name: &'static str,
    /// Oklch (lightness, chroma, hue) of the top-left and bottom-right stops.
    pub from: (f32, f32, f32),
    pub to: (f32, f32, f32),
    pub motif: Motif,
}

pub const PRESETS: &[Preset] = &[
    Preset { name: tk("Corniche"), from: (0.42, 0.11, 248.0), to: (0.74, 0.12, 192.0), motif: Motif::Waves },
    Preset { name: tk("Montaza twilight"), from: (0.36, 0.14, 285.0), to: (0.72, 0.15, 35.0), motif: Motif::Sun },
    Preset { name: tk("Pharos"), from: (0.82, 0.14, 75.0), to: (0.40, 0.13, 300.0), motif: Motif::Beam },
    Preset { name: tk("Obsidian"), from: (0.22, 0.02, 160.0), to: (0.46, 0.11, 160.0), motif: Motif::Ring },
    Preset { name: tk("Sea glass"), from: (0.58, 0.10, 200.0), to: (0.86, 0.09, 165.0), motif: Motif::Pebbles },
    Preset { name: tk("Limestone"), from: (0.86, 0.07, 80.0), to: (0.60, 0.13, 45.0), motif: Motif::Arch },
    Preset { name: tk("Rose"), from: (0.70, 0.11, 0.0), to: (0.48, 0.12, 320.0), motif: Motif::Crescent },
    Preset { name: tk("Night bay"), from: (0.30, 0.09, 270.0), to: (0.52, 0.16, 300.0), motif: Motif::Stars },
];

fn oklch((l, c, h): (f32, f32, f32)) -> Color {
    Color::oklch(l, c, h).gamut_mapped()
}

/// Soft white for motifs and the monogram.
const INK: [u8; 3] = [255, 255, 255];
const MOTIF_ALPHA: f32 = 0.16;
const MONOGRAM_ALPHA: f32 = 0.96;
const SHADOW_ALPHA: f32 = 0.22;
/// Monogram cap size relative to the avatar side (one / two letters).
const MONOGRAM_SCALE_ONE: f32 = 0.50;
const MONOGRAM_SCALE_TWO: f32 = 0.40;
const GRADIENT_STEPS: usize = 256;
/// Fonts tried for the monogram, in order (Latin + Arabic first).
const MONOGRAM_FAMILIES: &[&str] = &["Alexandria", "Inter", "Cairo", "Noto Sans", "Noto Sans Arabic", "Noto Sans CJK SC", "Noto Sans Devanagari", "DejaVu Sans"];
const MONOGRAM_WEIGHT: u16 = 600;

fn fonts() -> &'static fontdb::Database {
    static DB: OnceLock<fontdb::Database> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        db
    })
}

/// The first installed font that has every glyph of `text`.
fn monogram_font(text: &str) -> Option<ab_glyph::FontVec> {
    use ab_glyph::{Font as _, VariableFont as _};
    let db = fonts();
    let names: Vec<fontdb::Family> = MONOGRAM_FAMILIES.iter().map(|n| fontdb::Family::Name(n)).chain(std::iter::once(fontdb::Family::SansSerif)).collect();
    names.iter().find_map(|family| {
        let id = db.query(&fontdb::Query { families: std::slice::from_ref(family), weight: fontdb::Weight(MONOGRAM_WEIGHT), ..Default::default() })?;
        let mut font = db.with_face_data(id, |data, index| ab_glyph::FontVec::try_from_vec_and_index(data.to_vec(), index).ok())??;
        font.set_variation(b"wght", f32::from(MONOGRAM_WEIGHT));
        text.chars().all(|c| font.glyph_id(c).0 != 0).then_some(font)
    })
}

/// Blends straight `rgb` at `alpha` into an opaque pixel.
fn blend(img: &mut RgbaImage, x: i32, y: i32, rgb: [u8; 3], alpha: f32) {
    if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
        return;
    }
    let a = alpha.clamp(0.0, 1.0);
    let px = img.get_pixel_mut(x as u32, y as u32);
    for (dst, src) in px.0.iter_mut().zip(rgb) {
        *dst = (f32::from(src) * a + f32::from(*dst) * (1.0 - a)).round() as u8;
    }
}

/// Anti-aliased coverage of a shape given as a signed distance (negative
/// inside), one pixel wide.
fn coverage(distance: f32) -> f32 {
    (0.5 - distance).clamp(0.0, 1.0)
}

fn shade(img: &mut RgbaImage, alpha: f32, sdf: impl Fn(f32, f32) -> f32) {
    let (w, h) = img.dimensions();
    for y in 0..h {
        for x in 0..w {
            let c = coverage(sdf(x as f32 + 0.5, y as f32 + 0.5));
            if c > 0.0 {
                blend(img, x as i32, y as i32, INK, alpha * c);
            }
        }
    }
}

fn circle(cx: f32, cy: f32, r: f32) -> impl Fn(f32, f32) -> f32 {
    move |x, y| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r
}

fn draw_motif(img: &mut RgbaImage, motif: Motif) {
    let s = img.width() as f32;
    match motif {
        Motif::Waves => {
            for (i, base) in [0.74f32, 0.86].into_iter().enumerate() {
                let phase = i as f32 * 1.7;
                shade(img, MOTIF_ALPHA, move |x, y| {
                    let crest = s * base + (x / s * std::f32::consts::TAU * 1.5 + phase).sin() * s * 0.03;
                    crest - y
                });
            }
        }
        Motif::Sun => shade(img, MOTIF_ALPHA * 1.3, circle(s * 0.5, s * 0.92, s * 0.30)),
        Motif::Beam => shade(img, MOTIF_ALPHA, move |x, y| {
            // A wedge from the top-left corner, ±12° around the diagonal.
            let angle = (y / x.max(1e-3)).atan().to_degrees();
            ((angle - 45.0).abs() - 12.0) * (x * x + y * y).sqrt() * 0.02
        }),
        Motif::Ring => {
            let (c, r, w) = (s * 0.5, s * 0.40, s * 0.012);
            shade(img, MOTIF_ALPHA * 1.6, move |x, y| (((x - c).powi(2) + (y - c).powi(2)).sqrt() - r).abs() - w);
        }
        Motif::Pebbles => {
            for (fx, fy, fr) in [(0.18, 0.22, 0.10), (0.84, 0.30, 0.07), (0.78, 0.84, 0.13), (0.14, 0.80, 0.06)] {
                shade(img, MOTIF_ALPHA, circle(s * fx, s * fy, s * fr));
            }
        }
        Motif::Arch => {
            let (cx, top, half, w) = (s * 0.5, s * 0.30, s * 0.30, s * 0.014);
            shade(img, MOTIF_ALPHA * 1.4, move |x, y| {
                let d = if y < top + half { ((x - cx).powi(2) + (y - top - half).powi(2)).sqrt() - half } else { (x - cx).abs() - half };
                d.abs() - w
            });
        }
        Motif::Crescent => {
            let (outer, inner) = (circle(s * 0.78, s * 0.22, s * 0.13), circle(s * 0.83, s * 0.18, s * 0.11));
            shade(img, MOTIF_ALPHA * 1.5, move |x, y| outer(x, y).max(-inner(x, y)));
        }
        Motif::Stars => {
            for (fx, fy, fr) in [(0.16, 0.18, 0.012), (0.82, 0.14, 0.016), (0.70, 0.30, 0.008), (0.26, 0.86, 0.010), (0.88, 0.76, 0.012), (0.10, 0.52, 0.008)] {
                shade(img, MOTIF_ALPHA * 4.0, circle(s * fx, s * fy, s * fr));
            }
        }
    }
}

fn draw_monogram(img: &mut RgbaImage, text: &str) {
    use ab_glyph::{Font as _, ScaleFont as _};
    let Some(font) = monogram_font(text) else { return };
    let s = img.width() as f32;
    let size = s * if text.chars().count() > 1 { MONOGRAM_SCALE_TWO } else { MONOGRAM_SCALE_ONE } / 0.72;
    let scale = ab_glyph::PxScale::from(size);
    let sf = font.as_scaled(scale);
    // Measure the ink box so the letters sit optically centered.
    let mut caret = 0.0;
    let mut outlines = Vec::new();
    for c in text.chars() {
        let id = sf.glyph_id(c);
        if let Some(og) = font.outline_glyph(id.with_scale_and_position(scale, ab_glyph::point(caret, 0.0))) {
            outlines.push(og);
        }
        caret += sf.h_advance(id);
    }
    let (Some(min_x), Some(max_x), Some(min_y), Some(max_y)) = (
        outlines.iter().map(|o| o.px_bounds().min.x).reduce(f32::min),
        outlines.iter().map(|o| o.px_bounds().max.x).reduce(f32::max),
        outlines.iter().map(|o| o.px_bounds().min.y).reduce(f32::min),
        outlines.iter().map(|o| o.px_bounds().max.y).reduce(f32::max),
    ) else {
        return;
    };
    let dx = (s - (max_x - min_x)) / 2.0 - min_x;
    let dy = (s - (max_y - min_y)) / 2.0 - min_y;
    let shadow_offset = (s * 0.008).max(1.0);
    for (offset, rgb, alpha) in [(shadow_offset, [0u8, 0, 0], SHADOW_ALPHA), (0.0, INK, MONOGRAM_ALPHA)] {
        for og in &outlines {
            let b = og.px_bounds();
            og.draw(|gx, gy, cov| {
                blend(img, (b.min.x + dx + offset) as i32 + gx as i32, (b.min.y + dy + offset) as i32 + gy as i32, rgb, alpha * cov);
            });
        }
    }
}

/// Renders preset `index` with `initials` (empty = motif only) at `size`.
pub fn render_preset(index: usize, initials: &str, size: u32) -> RgbaImage {
    let preset = PRESETS[index % PRESETS.len()];
    let (from, to) = (oklch(preset.from), oklch(preset.to));
    let ramp: Vec<[u8; 4]> = (0..GRADIENT_STEPS).map(|i| from.mix(to, i as f32 / (GRADIENT_STEPS - 1) as f32).to_rgba8()).collect();
    let span = (2 * size.max(1) - 1) as f32;
    let mut img = RgbaImage::from_fn(size, size, |x, y| {
        let t = (x + y) as f32 / span;
        let [r, g, b, _] = ramp[((t * (GRADIENT_STEPS - 1) as f32).round() as usize).min(GRADIENT_STEPS - 1)];
        image::Rgba([r, g, b, u8::MAX])
    });
    draw_motif(&mut img, preset.motif);
    let initials = initials.trim();
    if !initials.is_empty() {
        draw_monogram(&mut img, initials);
    }
    img
}

fn hex_of(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

/// The preset as a cached PNG (rendered once per preset + initials).
pub fn preset_file(index: usize, initials: &str, size: u32) -> Result<PathBuf, String> {
    let path = cache_dir().join(format!("preset-{}-{}-{size}.png", index % PRESETS.len(), hex_of(initials.trim())));
    if !path.is_file() {
        let tmp = path.with_extension(format!("{}.tmp", stamp()));
        render_preset(index, initials, size).save_with_format(&tmp, ImageFormat::Png).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_rect_stays_inside() {
        // Landscape photo, default crop: the centered square.
        assert_eq!(crop_rect(400, 200, Crop::default()), (100, 0, 200));
        // Zoom 2 → half the side, centered.
        assert_eq!(crop_rect(400, 200, Crop { zoom: 2.0, ..Default::default() }), (150, 50, 100));
        // Focus at the far left edge clamps to the border.
        assert_eq!(crop_rect(400, 200, Crop { zoom: 2.0, center_x: 0.0, center_y: 0.0 }), (0, 0, 100));
        assert_eq!(crop_rect(400, 200, Crop { zoom: 2.0, center_x: 1.0, center_y: 1.0 }), (300, 100, 100));
        // Zoom is clamped.
        assert_eq!(crop_rect(100, 100, Crop { zoom: 100.0, ..Default::default() }).2, 25);
        assert_eq!(crop_rect(100, 100, Crop { zoom: 0.1, ..Default::default() }), (0, 0, 100));
        // Degenerate images don't panic.
        assert_eq!(crop_rect(1, 1, Crop::default()), (0, 0, 1));
    }

    #[test]
    fn renders_a_square_avatar() {
        let src = DynamicImage::ImageRgba8(RgbaImage::from_fn(300, 120, |x, _| if x < 150 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 255]) }));
        let out = render_crop(&src, Crop { zoom: 2.0, center_x: 0.1, center_y: 0.5 });
        assert_eq!(out.dimensions(), (AVATAR_SIZE, AVATAR_SIZE));
        // The crop sits in the red half.
        assert!(out.get_pixel(AVATAR_SIZE / 2, AVATAR_SIZE / 2).0[0] > 200);
    }

    #[test]
    fn presets_are_opaque_gradients() {
        for i in 0..PRESETS.len() {
            let img = render_preset(i, "", 64);
            assert_eq!(img.dimensions(), (64, 64));
            assert!(img.pixels().all(|p| p.0[3] == u8::MAX));
            assert_ne!(img.get_pixel(0, 0), img.get_pixel(63, 63), "{} is a gradient", PRESETS[i].name);
        }
    }

    #[test]
    fn round_trips_through_files() {
        let dir = std::env::temp_dir().join(format!("mandara-avatar-test-{}", stamp()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("photo.png");
        RgbaImage::from_pixel(1600, 900, image::Rgba([10, 120, 200, 255])).save(&src).unwrap();
        let prepared = prepare_source(&src).unwrap();
        assert_eq!((prepared.width, prepared.height), (MAX_SOURCE_SIDE, 576));
        let out = write_avatar(&prepared.path, Crop::default()).unwrap();
        let img = image::open(&out).unwrap();
        assert_eq!((img.width(), img.height()), (AVATAR_SIZE, AVATAR_SIZE));
        assert!(std::fs::metadata(&out).unwrap().len() < MAX_ICON_BYTES);
        let _ = std::fs::remove_file(out);
        let _ = std::fs::remove_file(prepared.path);
        let _ = std::fs::remove_dir_all(dir);
    }
}
