//! `cargo xtask mosaic OUT.png --cols N [options] TILE…`: lays screenshots
//! out as a grid of rounded tiles with a soft shadow on a transparent
//! background, each with an optional label under it. The README's Looks
//! image is made with it, once for dark and once for light mode:
//!
//! ```text
//! cargo xtask mosaic looks-dark.png --cols 2 --theme dark \
//!     look-corniche-dusk.png="Corniche Dusk" look-pharos-ink.png="Pharos Ink" …
//! ```
//!
//! A `TILE` is `PATH` or `PATH=LABEL`. Every tile is scaled to cover
//! `--width` × (width × the first tile's aspect ratio) and cropped to its
//! centre. `--theme dark|light` picks the label colour and the shadow's
//! strength for the page the mosaic sits on.

use ab_glyph::{Font as _, FontRef, PxScale, ScaleFont as _};
use image::{Rgba, RgbaImage, imageops};

use crate::{Result, opt};

const USAGE: &str = "usage: cargo xtask mosaic OUT.png --cols N [--width PX] [--gap PX] [--radius PX] [--theme dark|light] [--label-size PX] TILE… (TILE = PATH or PATH=LABEL)";
/// Default tile width, gap between tiles and corner radius (px).
const WIDTH: u32 = 1280;
const GAP: u32 = 56;
const RADIUS: f32 = 28.0;
/// Default label text size (px) and the room a label row takes under a tile.
const LABEL_SIZE: f32 = 34.0;
const LABEL_ROW: f32 = 2.2;
/// The shadow: blur, offset and opacity, per theme.
const SHADOW_SIGMA: f32 = 18.0;
const SHADOW_OFFSET: (i64, i64) = (0, 14);
const SHADOW_DARK: ([u8; 3], f32) = ([0, 0, 0], 0.55);
const SHADOW_LIGHT: ([u8; 3], f32) = ([0x1a, 0x1f, 0x3a], 0.28);
/// Label colours on a dark and on a light page.
const LABEL_DARK: [u8; 3] = [0xe6, 0xe8, 0xf2];
const LABEL_LIGHT: [u8; 3] = [0x24, 0x29, 0x3d];
/// The label font (Inter, like the plugin covers); `MANDARA_MOSAIC_FONT`
/// overrides it with any TrueType / OpenType file.
const FONT: &str = "/usr/share/fonts/inter/Inter.ttc";
const FONT_ENV: &str = "MANDARA_MOSAIC_FONT";
/// The face of a font collection used for labels.
const FONT_FACE: u32 = 0;

/// The layout options of one mosaic.
#[derive(Clone, Copy, Debug)]
struct Layout {
    cols: u32,
    width: u32,
    gap: u32,
    radius: f32,
    dark: bool,
    label_size: f32,
}

pub fn mosaic(args: &[String]) -> Result<()> {
    let out = args.first().filter(|a| !a.starts_with("--")).ok_or(USAGE)?;
    let num = |name: &str, default: u32| -> Result<u32> { opt(args, name).map(|v| v.parse::<u32>().map_err(|_| format!("{name}: want a number"))).transpose().map(|v| v.unwrap_or(default)) };
    let layout = Layout {
        cols: num("--cols", 2)?.max(1),
        width: num("--width", WIDTH)?.max(16),
        gap: num("--gap", GAP)?,
        radius: num("--radius", RADIUS as u32)? as f32,
        dark: match opt(args, "--theme").as_deref() {
            None | Some("dark") => true,
            Some("light") => false,
            Some(other) => return Err(format!("--theme {other}: want dark or light")),
        },
        label_size: num("--label-size", LABEL_SIZE as u32)? as f32,
    };
    let tiles = tile_args(&args[1..]);
    if tiles.is_empty() {
        return Err(USAGE.into());
    }
    let mut images = Vec::new();
    for (path, label) in &tiles {
        let img = image::open(path).map_err(|e| format!("{path}: {e}"))?.to_rgba8();
        images.push((img, label.clone()));
    }
    let font_path = std::env::var(FONT_ENV).unwrap_or_else(|_| FONT.into());
    let needs_font = images.iter().any(|(_, l)| l.is_some());
    let font_data = if needs_font { std::fs::read(&font_path).map_err(|e| format!("{font_path}: {e} (set {FONT_ENV} to a font file)"))? } else { Vec::new() };
    let font = if needs_font { Some(FontRef::try_from_slice_and_index(&font_data, FONT_FACE).map_err(|e| format!("{font_path}: {e}"))?) } else { None };
    let canvas = compose(&images, layout, font.as_ref());
    canvas.save(out).map_err(|e| format!("{out}: {e}"))?;
    println!("{out} {}x{}", canvas.width(), canvas.height());
    Ok(())
}

/// The `TILE` arguments (everything that is not an option or its value).
fn tile_args(args: &[String]) -> Vec<(String, Option<String>)> {
    const VALUED: [&str; 6] = ["--cols", "--width", "--gap", "--radius", "--theme", "--label-size"];
    let mut tiles = Vec::new();
    let mut skip = false;
    for a in args {
        if std::mem::take(&mut skip) {
            continue;
        }
        if VALUED.contains(&a.as_str()) {
            skip = true;
            continue;
        }
        match a.split_once('=') {
            Some((path, label)) => tiles.push((path.to_string(), Some(label.to_string()))),
            None => tiles.push((a.clone(), None)),
        }
    }
    tiles
}

/// The mosaic of `images` (each with its label).
fn compose(images: &[(RgbaImage, Option<String>)], l: Layout, font: Option<&FontRef>) -> RgbaImage {
    let (fw, fh) = images.first().map_or((16, 9), |(i, _)| i.dimensions());
    let tile_h = (l.width as f32 * fh as f32 / fw.max(1) as f32).round() as u32;
    let labelled = font.is_some() && images.iter().any(|(_, label)| label.is_some());
    let label_row = if labelled { (l.label_size * LABEL_ROW).round() as u32 } else { 0 };
    let rows = (images.len() as u32).div_ceil(l.cols);
    let cols = l.cols.min(images.len() as u32).max(1);
    // Room around the grid for the shadow.
    let pad = (SHADOW_SIGMA * 3.0).ceil() as u32 + SHADOW_OFFSET.1.unsigned_abs() as u32;
    let cell_h = tile_h + label_row;
    let w = pad * 2 + cols * l.width + (cols - 1) * l.gap;
    let h = pad * 2 + rows * cell_h + rows.saturating_sub(1) * l.gap;
    let mut canvas = RgbaImage::new(w, h);
    let (shadow_rgb, shadow_alpha) = if l.dark { SHADOW_DARK } else { SHADOW_LIGHT };
    let mut tiles = Vec::new();
    for (i, (img, label)) in images.iter().enumerate() {
        let (c, r) = (i as u32 % l.cols, i as u32 / l.cols);
        let x = pad + c * (l.width + l.gap);
        let y = pad + r * (cell_h + l.gap);
        let tile = rounded(&cover(img, l.width, tile_h), l.radius);
        tiles.push((tile, x, y, label));
    }
    // Shadows first, so a tile's shadow never falls over its neighbour.
    for (tile, x, y, _) in &tiles {
        shadow(&mut canvas, tile, (*x as i64, *y as i64), shadow_rgb, shadow_alpha);
    }
    for (tile, x, y, label) in &tiles {
        over(&mut canvas, tile, *x as i64, *y as i64);
        if let (Some(font), Some(label)) = (font, label) {
            let color = if l.dark { LABEL_DARK } else { LABEL_LIGHT };
            draw_text(&mut canvas, font, label, l.label_size, (*x as f32 + l.width as f32 / 2.0, (*y + tile_h) as f32 + l.label_size * 1.35), color);
        }
    }
    canvas
}

/// `img` scaled to cover `w` × `h`, cropped to its centre.
fn cover(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let (iw, ih) = img.dimensions();
    let scale = (w as f32 / iw as f32).max(h as f32 / ih as f32);
    let (sw, sh) = (((iw as f32 * scale).round() as u32).max(w), ((ih as f32 * scale).round() as u32).max(h));
    let scaled = imageops::resize(img, sw, sh, imageops::FilterType::Lanczos3);
    imageops::crop_imm(&scaled, (sw - w) / 2, (sh - h) / 2, w, h).to_image()
}

/// `img` with anti-aliased rounded corners of `radius` px.
fn rounded(img: &RgbaImage, radius: f32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let mut out = img.clone();
    for (x, y, px) in out.enumerate_pixels_mut() {
        let coverage = corner_coverage(x as f32 + 0.5, y as f32 + 0.5, w as f32, h as f32, radius);
        px[3] = (f32::from(px[3]) * coverage).round() as u8;
    }
    out
}

/// How much of the pixel centred at `(px, py)` lies inside a `w` × `h`
/// rectangle with corners of radius `r` (0–1, a one-pixel ramp).
fn corner_coverage(px: f32, py: f32, w: f32, h: f32, r: f32) -> f32 {
    let r = r.min(w / 2.0).min(h / 2.0);
    let cx = px.clamp(r, w - r);
    let cy = py.clamp(r, h - r);
    let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
    (r - d + 0.5).clamp(0.0, 1.0)
}

/// A soft shadow of `tile`'s shape, offset by [`SHADOW_OFFSET`].
fn shadow(canvas: &mut RgbaImage, tile: &RgbaImage, at: (i64, i64), rgb: [u8; 3], alpha: f32) {
    let pad = (SHADOW_SIGMA * 3.0).ceil() as u32;
    let mut mask = image::GrayImage::new(tile.width() + pad * 2, tile.height() + pad * 2);
    for (x, y, p) in tile.enumerate_pixels() {
        mask.put_pixel(x + pad, y + pad, image::Luma([(f32::from(p[3]) * alpha).round() as u8]));
    }
    let blurred = imageops::blur(&mask, SHADOW_SIGMA);
    let mut layer = RgbaImage::new(blurred.width(), blurred.height());
    for (px, a) in layer.pixels_mut().zip(blurred.pixels()) {
        *px = Rgba([rgb[0], rgb[1], rgb[2], a[0]]);
    }
    over(canvas, &layer, at.0 + SHADOW_OFFSET.0 - pad as i64, at.1 + SHADOW_OFFSET.1 - pad as i64);
}

/// Source-over of `top` onto `canvas` at `(x, y)` (straight alpha).
fn over(canvas: &mut RgbaImage, top: &RgbaImage, x: i64, y: i64) {
    for (tx, ty, src) in top.enumerate_pixels() {
        let (cx, cy) = (x + i64::from(tx), y + i64::from(ty));
        if cx < 0 || cy < 0 || cx >= i64::from(canvas.width()) || cy >= i64::from(canvas.height()) {
            continue;
        }
        let dst = canvas.get_pixel_mut(cx as u32, cy as u32);
        *dst = blend(*dst, *src);
    }
}

fn blend(dst: Rgba<u8>, src: Rgba<u8>) -> Rgba<u8> {
    let sa = f32::from(src[3]) / 255.0;
    let da = f32::from(dst[3]) / 255.0;
    let oa = sa + da * (1.0 - sa);
    if oa <= 0.0 {
        return Rgba([0, 0, 0, 0]);
    }
    let ch = |i: usize| ((f32::from(src[i]) * sa + f32::from(dst[i]) * da * (1.0 - sa)) / oa).round().clamp(0.0, 255.0) as u8;
    Rgba([ch(0), ch(1), ch(2), (oa * 255.0).round() as u8])
}

/// `text` centred on `center` (x) with its baseline at `center.1`.
fn draw_text(canvas: &mut RgbaImage, font: &FontRef, text: &str, size: f32, center: (f32, f32), rgb: [u8; 3]) {
    let scale = PxScale::from(size);
    let scaled = font.as_scaled(scale);
    let mut glyphs = Vec::new();
    let mut caret = 0.0f32;
    let mut last = None;
    for ch in text.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(prev) = last {
            caret += scaled.kern(prev, id);
        }
        glyphs.push(id.with_scale_and_position(scale, ab_glyph::point(caret, 0.0)));
        caret += scaled.h_advance(id);
        last = Some(id);
    }
    let x0 = center.0 - caret / 2.0;
    for g in glyphs {
        let Some(outline) = font.outline_glyph(g) else { continue };
        let b = outline.px_bounds();
        outline.draw(|gx, gy, c| {
            let (x, y) = (x0 + b.min.x + gx as f32, center.1 + b.min.y + gy as f32);
            if x < 0.0 || y < 0.0 || x >= canvas.width() as f32 || y >= canvas.height() as f32 {
                return;
            }
            let dst = canvas.get_pixel_mut(x as u32, y as u32);
            *dst = blend(*dst, Rgba([rgb[0], rgb[1], rgb[2], (c.clamp(0.0, 1.0) * 255.0).round() as u8]));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(cols: u32) -> Layout {
        Layout { cols, width: 64, gap: 8, radius: 8.0, dark: true, label_size: LABEL_SIZE }
    }

    #[test]
    fn tiles_are_parsed_with_and_without_labels() {
        let args: Vec<String> = ["--cols", "2", "a.png=Corniche Dusk", "--theme", "light", "b.png"].map(String::from).to_vec();
        assert_eq!(tile_args(&args), vec![("a.png".into(), Some("Corniche Dusk".into())), ("b.png".into(), None)]);
    }

    #[test]
    fn grid_size_follows_columns_tiles_and_shadow_room() {
        let img = RgbaImage::from_pixel(160, 90, Rgba([10, 20, 30, 255]));
        let images: Vec<_> = (0..3).map(|_| (img.clone(), None)).collect();
        let out = compose(&images, layout(2), None);
        let pad = (SHADOW_SIGMA * 3.0).ceil() as u32 + SHADOW_OFFSET.1.unsigned_abs() as u32;
        assert_eq!(out.width(), pad * 2 + 2 * 64 + 8);
        assert_eq!(out.height(), pad * 2 + 2 * 36 + 8);
        // The background stays transparent and a tile's middle is opaque.
        assert_eq!(out.get_pixel(0, 0)[3], 0);
        assert_eq!(*out.get_pixel(pad + 32, pad + 18), Rgba([10, 20, 30, 255]));
    }

    #[test]
    fn corners_are_cut_smoothly() {
        let tile = rounded(&RgbaImage::from_pixel(40, 40, Rgba([255, 255, 255, 255])), 10.0);
        assert_eq!(tile.get_pixel(0, 0)[3], 0);
        assert_eq!(tile.get_pixel(20, 20)[3], 255);
        assert_eq!(tile.get_pixel(20, 0)[3], 255);
        let edge = tile.get_pixel(3, 2)[3];
        assert!(edge > 0 && edge < 255, "anti-aliased corner, got {edge}");
    }

    #[test]
    fn cover_fills_and_crops_to_the_centre() {
        let mut img = RgbaImage::from_pixel(200, 100, Rgba([0, 0, 0, 255]));
        for y in 0..100 {
            for x in 50..150 {
                img.put_pixel(x, y, Rgba([255, 0, 0, 255]));
            }
        }
        let out = cover(&img, 50, 50);
        assert_eq!(out.dimensions(), (50, 50));
        assert!(out.get_pixel(25, 25)[0] > 200);
    }
}
