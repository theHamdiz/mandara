//! Triangle-filter resizing one output row at a time.
//!
//! `image`'s `resize` (and `DynamicImage::resize_exact`) filters the whole
//! picture vertically into an `f32` RGBA copy of `source width × new
//! height` pixels (16 bytes each) before filtering it horizontally. Bringing
//! a 1920 × 1080 wallpaper to a 4K output allocated 63 MiB for that copy,
//! a 2880 × 1620 one 95 MiB, on top of the source, a cropped copy of it and
//! the result, every time a wallpaper smaller than the output loaded (and
//! 53 MiB more for a 1600 × 900 depth mask brought to the same size).
//!
//! [`triangle`] and [`triangle_gray`] do the same arithmetic in the same
//! order (so the pixels are identical) with one filtered row of `f32`s alive
//! at a time, and read the cropped region in place. They are also faster:
//! 1920 × 1080 → 3840 × 2160 takes 53 instead of 96 ms.

use image::{DynamicImage, GrayImage, ImageBuffer, Pixel};

/// The triangle (bilinear) kernel, as `image` defines it.
fn kernel(x: f32) -> f32 {
    if x.abs() < 1.0 { 1.0 - x.abs() } else { 0.0 }
}

/// Source taps of one output sample: the first source index and the
/// normalized weights from there on.
struct Taps {
    first: usize,
    weights: Vec<f32>,
}

/// The taps of every output sample when `len` source samples become `out`,
/// computed exactly like `image`'s `vertical_sample` / `horizontal_sample`
/// (triangle filter, support 1).
fn taps(len: u32, out: u32) -> Vec<Taps> {
    let ratio = len as f32 / out as f32;
    let sratio = if ratio < 1.0 { 1.0 } else { ratio };
    let support = sratio;
    (0..out)
        .map(|o| {
            let input = (o as f32 + 0.5) * ratio;
            let left = ((input - support).floor() as i64).clamp(0, i64::from(len) - 1);
            let right = ((input + support).ceil() as i64).clamp(left + 1, i64::from(len));
            let centre = input - 0.5;
            let mut weights: Vec<f32> = (left..right).map(|i| kernel((i as f32 - centre) / sratio)).collect();
            let sum: f32 = weights.iter().sum();
            for w in &mut weights {
                *w /= sum;
            }
            Taps { first: left as usize, weights }
        })
        .collect()
}

/// The `(x, y, width, height)` region of `img` resized to `w × h` with the
/// triangle filter: the pixels of
/// `img.crop_imm(x, y, width, height).resize_exact(w, h, FilterType::Triangle)`
/// without the cropped copy or the full-size float buffer. 8-bit RGB, RGBA
/// and gray are filtered in place; other layouts take `image`'s path.
pub fn triangle(img: &DynamicImage, (x, y, cw, ch): (u32, u32, u32, u32), w: u32, h: u32) -> DynamicImage {
    let region_ok = cw > 0 && ch > 0 && x.saturating_add(cw) <= img.width() && y.saturating_add(ch) <= img.height() && w > 0 && h > 0;
    match img {
        DynamicImage::ImageRgb8(b) if region_ok => DynamicImage::ImageRgb8(resize_px::<3, image::Rgb<u8>>(b.as_raw(), b.width(), (x, y, cw, ch), w, h)),
        DynamicImage::ImageRgba8(b) if region_ok => DynamicImage::ImageRgba8(resize_px::<4, image::Rgba<u8>>(b.as_raw(), b.width(), (x, y, cw, ch), w, h)),
        DynamicImage::ImageLuma8(b) if region_ok => DynamicImage::ImageLuma8(resize_px::<1, image::Luma<u8>>(b.as_raw(), b.width(), (x, y, cw, ch), w, h)),
        // The whole picture needs no cropped copy first.
        other if (x, y, cw, ch) == (0, 0, other.width(), other.height()) => other.resize_exact(w, h, image::imageops::FilterType::Triangle),
        other => other.crop_imm(x, y, cw, ch).resize_exact(w, h, image::imageops::FilterType::Triangle),
    }
}

/// A whole gray image (a depth mask) resized to `w × h`: the pixels of
/// `imageops::resize(img, w, h, FilterType::Triangle)` (see [`triangle`]).
pub fn triangle_gray(img: &GrayImage, w: u32, h: u32) -> GrayImage {
    let (iw, ih) = img.dimensions();
    if iw == 0 || ih == 0 || w == 0 || h == 0 {
        return image::imageops::resize(img, w, h, image::imageops::FilterType::Triangle);
    }
    resize_px::<1, image::Luma<u8>>(img.as_raw(), iw, (0, 0, iw, ih), w, h)
}

/// [`triangle`] for `N`-channel 8-bit pixels in `src` (`stride` pixels per
/// row).
fn resize_px<const N: usize, P: Pixel<Subpixel = u8>>(src: &[u8], stride: u32, (x0, y0, cw, ch): (u32, u32, u32, u32), w: u32, h: u32) -> ImageBuffer<P, Vec<u8>> {
    let (stride, x0, y0) = (stride as usize, x0 as usize, y0 as usize);
    if (w, h) == (cw, ch) {
        // Same size: a copy of the region, as `image` makes it.
        let line = cw as usize * N;
        let out: Vec<u8> = (0..ch as usize).flat_map(|y| &src[((y0 + y) * stride + x0) * N..][..line]).copied().collect();
        return ImageBuffer::from_raw(w, h, out).unwrap_or_else(|| ImageBuffer::new(w, h));
    }
    let cw = cw as usize;
    let rows = taps(ch, h);
    let cols = taps(cw as u32, w);
    let mut out = vec![0u8; w as usize * h as usize * N];
    // One vertically filtered row of the region.
    let mut row = vec![0f32; cw * N];
    for (oy, t) in rows.iter().enumerate() {
        for (x, acc) in row.as_chunks_mut::<N>().0.iter_mut().enumerate() {
            let mut sum = [0f32; N];
            for (i, w) in t.weights.iter().enumerate() {
                let at = ((y0 + t.first + i) * stride + x0 + x) * N;
                for c in 0..N {
                    sum[c] += f32::from(src[at + c]) * w;
                }
            }
            *acc = sum;
        }
        let line = &mut out[oy * w as usize * N..(oy + 1) * w as usize * N];
        for (px, t) in line.as_chunks_mut::<N>().0.iter_mut().zip(&cols) {
            let mut sum = [0f32; N];
            for (j, w) in t.weights.iter().enumerate() {
                let at = (t.first + j) * N;
                for c in 0..N {
                    sum[c] += row[at + c] * w;
                }
            }
            for c in 0..N {
                px[c] = sum[c].clamp(0.0, f32::from(u8::MAX)).round() as u8;
            }
        }
    }
    ImageBuffer::from_raw(w, h, out).unwrap_or_else(|| ImageBuffer::new(w, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::imageops::FilterType;

    /// Deterministic noise with smooth areas and hard edges.
    fn picture(w: u32, h: u32, alpha: bool, seed: u32) -> DynamicImage {
        let mut s = seed.wrapping_mul(2_654_435_761).max(1);
        let mut next = move || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s
        };
        let mut px = |x: u32, y: u32| -> [u8; 4] {
            let n = next();
            let band = if (x / 7 + y / 5).is_multiple_of(3) { 255 } else { 0 };
            [(x * 255 / w.max(1)) as u8, band, n as u8, if alpha { (n >> 8) as u8 } else { u8::MAX }]
        };
        if alpha {
            DynamicImage::ImageRgba8(image::RgbaImage::from_fn(w, h, |x, y| image::Rgba(px(x, y))))
        } else {
            DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
                let [r, g, b, _] = px(x, y);
                image::Rgb([r, g, b])
            }))
        }
    }

    fn reference(img: &DynamicImage, region: (u32, u32, u32, u32), w: u32, h: u32) -> DynamicImage {
        img.crop_imm(region.0, region.1, region.2, region.3).resize_exact(w, h, FilterType::Triangle)
    }

    #[test]
    fn matches_image_resize_exactly() {
        // Upscales (a wallpaper smaller than the output), mild shrinks,
        // mixed axes, crops, one-pixel edges and same-size copies.
        let cases: [((u32, u32), (u32, u32, u32, u32), (u32, u32)); 9] = [
            ((192, 108), (0, 0, 192, 108), (384, 216)),
            ((288, 162), (0, 0, 288, 162), (384, 216)),
            ((300, 200), (10, 20, 240, 135), (384, 216)),
            ((200, 125), (0, 0, 200, 125), (160, 100)),
            ((97, 61), (3, 0, 90, 61), (71, 140)),
            ((1, 1), (0, 0, 1, 1), (5, 3)),
            ((64, 1), (0, 0, 64, 1), (100, 4)),
            ((50, 40), (0, 0, 50, 40), (50, 40)),
            ((33, 47), (5, 6, 20, 30), (19, 31)),
        ];
        for (seed, &((sw, sh), region, (w, h))) in cases.iter().enumerate() {
            let gray = picture(sw, sh, false, seed as u32 + 100).into_luma8();
            if (region.0, region.1, region.2, region.3) == (0, 0, sw, sh) {
                let theirs = image::imageops::resize(&gray, w, h, FilterType::Triangle);
                assert!(triangle_gray(&gray, w, h) == theirs, "gray {sw}x{sh} -> {w}x{h}: pixels differ");
            }
            let gray = DynamicImage::ImageLuma8(gray);
            assert!(triangle(&gray, region, w, h).as_bytes() == reference(&gray, region, w, h).as_bytes(), "gray {sw}x{sh} {region:?} -> {w}x{h}: pixels differ");
            for alpha in [false, true] {
                let img = picture(sw, sh, alpha, seed as u32 + 1);
                let ours = triangle(&img, region, w, h);
                let theirs = reference(&img, region, w, h);
                assert_eq!(ours.color(), theirs.color(), "{sw}x{sh} {region:?} -> {w}x{h}");
                assert!(ours.as_bytes() == theirs.as_bytes(), "{sw}x{sh} {region:?} -> {w}x{h} alpha {alpha}: pixels differ");
            }
        }
    }

    #[test]
    fn other_layouts_and_bad_regions_take_images_path() {
        let gray16 = DynamicImage::ImageLuma16(image::ImageBuffer::from_fn(40, 30, |x, y| image::Luma([(x * 900 + y * 7) as u16])));
        assert_eq!(triangle(&gray16, (0, 0, 40, 30), 80, 60).as_bytes(), reference(&gray16, (0, 0, 40, 30), 80, 60).as_bytes());
        // A part of it still goes through the crop.
        assert_eq!(triangle(&gray16, (4, 2, 30, 20), 45, 30).as_bytes(), reference(&gray16, (4, 2, 30, 20), 45, 30).as_bytes());
        let rgb = picture(40, 30, false, 9);
        // A region reaching past the picture is clamped by `crop_imm`.
        assert_eq!(triangle(&rgb, (30, 20, 40, 30), 16, 12).as_bytes(), reference(&rgb, (30, 20, 40, 30), 16, 12).as_bytes());
    }
}
