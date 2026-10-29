//! How the desktop background places its image (`wallpaper.fill`) and how
//! a new image replaces the old one (`wallpaper.transition`).

use std::path::{Path, PathBuf};

use gpui::{AnyElement, IntoElement, ObjectFit, ParentElement, Styled, StyledImage as _, div, img, px};
use mandara_config::desktop::{WallpaperFill, WallpaperTransition};
use mandara_theme::Color;

/// Most tiles drawn for [`WallpaperFill::Tile`] (tiny images are scaled up
/// in whole steps until they fit the cap).
const MAX_TILES: u32 = 256;
/// Re-encode quality of prepared copies.
const JPEG_QUALITY: u8 = 92;

/// Prepares the image to draw for `fill` on an output of `w × h` physical
/// pixels: a cover-cropped copy (Cover), a fit-inside copy (Contain /
/// Stretch), a centre crop no larger than the output (Center) or the image
/// itself (Tile, downscaled to fit if larger). Video clips are prepared
/// from their first frame. Returns the file and its pixel size.
pub fn prepare(path: &Path, fill: WallpaperFill, w: u32, h: u32) -> Result<(PathBuf, (u32, u32)), String> {
    prepare_in(path, fill, w, h, &mandara_config::paths::cache_dir())
}

/// [`prepare`] with prepared copies stored under `cache`.
pub fn prepare_in(path: &Path, fill: WallpaperFill, w: u32, h: u32, cache: &Path) -> Result<(PathBuf, (u32, u32)), String> {
    // A video is prepared from its poster (first frame).
    let source = crate::wallpaper::still_in(path, cache)?;
    let path = source.as_path();
    let derived = |tag: &str| cache.join("wallpapers").join(crate::wallpaper::cache_key(path, tag));
    let (iw, ih) = image::image_dimensions(path).map_err(|e| format!("{}: {e}", path.display()))?;
    match fill {
        WallpaperFill::Cover => {
            // Same as the picker's display copy: no larger than the output.
            if iw <= w + w / 4 && ih <= h + h / 4 {
                return Ok((path.to_path_buf(), (iw, ih)));
            }
            let out = derived(&format!("{w}x{h}"));
            if !out.exists() {
                let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
                crate::wallpaper::save_jpeg(&crate::wallpaper::cover(&img, w, h), &out, JPEG_QUALITY)?;
            }
            Ok((out, (w, h)))
        }
        WallpaperFill::Contain | WallpaperFill::Stretch | WallpaperFill::Tile => {
            if iw <= w && ih <= h {
                return Ok((path.to_path_buf(), (iw, ih)));
            }
            let scale = (w as f32 / iw as f32).min(h as f32 / ih as f32);
            let (fw, fh) = (((iw as f32 * scale).round() as u32).max(1), ((ih as f32 * scale).round() as u32).max(1));
            let out = derived(&format!("fit{fw}x{fh}"));
            if !out.exists() {
                let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
                crate::wallpaper::save_jpeg(&img.resize_exact(fw, fh, image::imageops::FilterType::Triangle), &out, JPEG_QUALITY)?;
            }
            Ok((out, (fw, fh)))
        }
        WallpaperFill::Center => {
            if iw <= w && ih <= h {
                return Ok((path.to_path_buf(), (iw, ih)));
            }
            let (cw, ch) = (iw.min(w), ih.min(h));
            let out = derived(&format!("center{cw}x{ch}"));
            if !out.exists() {
                let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
                crate::wallpaper::save_jpeg(&img.crop_imm((iw - cw) / 2, (ih - ch) / 2, cw, ch), &out, JPEG_QUALITY)?;
            }
            Ok((out, (cw, ch)))
        }
    }
}

/// Tile grid `(columns, rows, tile scale)` covering `vw × vh` logical px
/// with tiles of `tw × th` logical px, at most [`MAX_TILES`] tiles.
pub fn tile_grid(vw: f32, vh: f32, tw: f32, th: f32) -> (u32, u32, f32) {
    let (tw, th) = (tw.max(1.0), th.max(1.0));
    let mut k = 1.0_f32;
    loop {
        let cols = (vw / (tw * k)).ceil().max(1.0) as u32;
        let rows = (vh / (th * k)).ceil().max(1.0) as u32;
        if cols * rows <= MAX_TILES {
            return (cols, rows, k);
        }
        k += 1.0;
    }
}

/// The background image layer for `fill` in a `vw × vh` logical viewport.
/// `size` is the prepared image's pixel size, `scale` the output scale.
pub fn layer(path: &Path, size: (u32, u32), fill: WallpaperFill, (vw, vh): (f32, f32), scale: f32) -> AnyElement {
    let natural = (size.0 as f32 / scale.max(f32::EPSILON), size.1 as f32 / scale.max(f32::EPSILON));
    let full = |fit: ObjectFit| img(path.to_path_buf()).absolute().left_0().top_0().w(px(vw)).h(px(vh)).object_fit(fit).into_any_element();
    match fill {
        WallpaperFill::Cover => full(ObjectFit::Cover),
        WallpaperFill::Contain => full(ObjectFit::Contain),
        WallpaperFill::Stretch => full(ObjectFit::Fill),
        WallpaperFill::Center => img(path.to_path_buf())
            .absolute()
            .left(px((vw - natural.0) / 2.0))
            .top(px((vh - natural.1) / 2.0))
            .w(px(natural.0))
            .h(px(natural.1))
            .object_fit(ObjectFit::Fill)
            .into_any_element(),
        WallpaperFill::Tile => {
            let (cols, rows, k) = tile_grid(vw, vh, natural.0, natural.1);
            let (tw, th) = (natural.0 * k, natural.1 * k);
            let mut grid = div().absolute().left_0().top_0().w(px(vw)).h(px(vh)).overflow_hidden();
            for r in 0..rows {
                for c in 0..cols {
                    grid = grid.child(img(path.to_path_buf()).absolute().left(px(c as f32 * tw)).top(px(r as f32 * th)).w(px(tw)).h(px(th)).object_fit(ObjectFit::Fill));
                }
            }
            grid.into_any_element()
        }
    }
}

/// The fill colour behind non-covering modes (`#rrggbb`; black if invalid).
pub fn fill_color(hex: &str) -> Color {
    Color::from_hex(hex.trim()).unwrap_or(Color::hex("#000000"))
}

/// Wraps the incoming layer for `transition` at eased progress `t`
/// (`0..=1`) in a `vw × vh` viewport; `rtl` wipes from the right.
pub fn reveal(layer: AnyElement, transition: WallpaperTransition, t: f32, (vw, vh): (f32, f32), rtl: bool) -> AnyElement {
    let t = t.clamp(0.0, 1.0);
    match transition {
        WallpaperTransition::Fade => div().absolute().inset_0().size_full().opacity(t).child(layer).into_any_element(),
        WallpaperTransition::None => layer,
        WallpaperTransition::Wipe => {
            let w = vw * t;
            let x = if rtl { vw - w } else { 0.0 };
            // A clip of growing width; the image inside stays put.
            div().absolute().top_0().left(px(x)).w(px(w)).h(px(vh)).overflow_hidden().child(div().absolute().top_0().left(px(-x)).w(px(vw)).h(px(vh)).child(layer)).into_any_element()
        }
        WallpaperTransition::Grow => {
            let (w, h) = (vw * t, vh * t);
            let (x, y) = ((vw - w) / 2.0, (vh - h) / 2.0);
            div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).overflow_hidden().child(div().absolute().left(px(-x)).top(px(-y)).w(px(vw)).h(px(vh)).child(layer)).into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_cover_the_viewport_within_the_cap() {
        let (c, r, k) = tile_grid(2560.0, 1440.0, 256.0, 256.0);
        assert_eq!((c, r, k), (10, 6, 1.0));
        let (c, r, k) = tile_grid(2560.0, 1440.0, 8.0, 8.0);
        assert!(c * r <= MAX_TILES && k > 1.0);
        assert!(c as f32 * 8.0 * k >= 2560.0 && r as f32 * 8.0 * k >= 1440.0);
    }

    #[test]
    fn prepared_copies_match_the_fill_mode() {
        let dir = std::env::temp_dir().join(format!("mandara-fill-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("big.png");
        image::RgbImage::from_pixel(400, 100, image::Rgb([10, 20, 30])).save(&src).unwrap();
        // Larger than a 200 × 200 output: fit keeps the 4:1 aspect.
        let cache = dir.join("cache");
        let (_, size) = prepare_in(&src, WallpaperFill::Contain, 200, 200, &cache).unwrap();
        assert_eq!(size, (200, 50));
        let (_, size) = prepare_in(&src, WallpaperFill::Center, 200, 200, &cache).unwrap();
        assert_eq!(size, (200, 100), "centre crop, never upscaled");
        let (out, size) = prepare_in(&src, WallpaperFill::Cover, 100, 100, &cache).unwrap();
        assert!(out.starts_with(&cache) && size == (100, 100));
        let (p, size) = prepare_in(&src, WallpaperFill::Tile, 800, 800, &cache).unwrap();
        assert_eq!((p, size), (src.clone(), (400, 100)), "small enough: drawn as is");
        assert_eq!(fill_color("nope"), Color::hex("#000000"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
