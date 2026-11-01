//! Mandara's hand-tuned subject finder, the depth plugin's default mode
//! (no download): a deliberately conservative sky / subject segmentation.
//! Estimate the sky from the top band (Oklab colour + edge density), grow it
//! downward row by row with local colour continuity, a drifting colour model
//! (gradient skies) and edge barriers, drop floating foreground islands
//! (clouds, birds, the moon), clean up morphologically, and feather the edge
//! with a colour-guided refinement at full resolution. A battery of
//! confidence checks rejects anything doubtful; a rejected wallpaper simply
//! gets no depth effect instead of a botched cut-out.

use std::sync::OnceLock;

use image::{DynamicImage, GrayImage, RgbImage};

use super::MASK_MAX;

/// Longest side of the image the segmentation runs on.
const ANALYSIS_MAX: u32 = 640;
/// Per-pixel Oklab gradient above which a pixel is an edge (sky stops).
const EDGE: f32 = 0.035;
/// Neighbour colour step allowed while growing the sky.
const LOCAL: f32 = 0.03;
/// Distance from the drifting sky model allowed while growing.
const GLOBAL: f32 = 0.12;

// ── Colour ───────────────────────────────────────────────────────────────

type Lab = [f32; 3];

fn linear_lut() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = [0.0; 256];
        for (i, v) in t.iter_mut().enumerate() {
            let c = i as f32 / 255.0;
            *v = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        }
        t
    })
}

pub(crate) fn oklab(rgb: [u8; 3]) -> Lab {
    let lut = linear_lut();
    let (r, g, b) = (lut[rgb[0] as usize], lut[rgb[1] as usize], lut[rgb[2] as usize]);
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn dist(a: Lab, b: Lab) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn mix(a: Lab, b: Lab, t: f32) -> Lab {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

pub(super) fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ── Segmentation ─────────────────────────────────────────────────────────

/// Outcome of [`segment`].
#[derive(Debug)]
pub struct Segmentation {
    /// Subject alpha (255 = foreground) at mask resolution; `None` when the
    /// heuristic wasn't confident.
    pub alpha: Option<GrayImage>,
    /// Fraction of the frame classified as sky.
    pub sky_fraction: f32,
    /// Human-readable verdict.
    pub reason: String,
}

/// A float field over the analysis grid.
struct Grid<T> {
    w: usize,
    h: usize,
    data: Vec<T>,
}

impl<T: Copy> Grid<T> {
    fn at(&self, x: usize, y: usize) -> T {
        self.data[y * self.w + x]
    }
}

fn box_blur_lab(src: &Grid<Lab>) -> Grid<Lab> {
    let (w, h) = (src.w, src.h);
    let mut tmp = vec![[0.0f32; 3]; w * h];
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(1), (x + 1).min(w - 1));
            let n = (b - a + 1) as f32;
            let mut s = [0.0; 3];
            for xx in a..=b {
                let p = src.at(xx, y);
                s = [s[0] + p[0], s[1] + p[1], s[2] + p[2]];
            }
            tmp[y * w + x] = [s[0] / n, s[1] / n, s[2] / n];
        }
    }
    let mut out = vec![[0.0f32; 3]; w * h];
    for y in 0..h {
        let (a, b) = (y.saturating_sub(1), (y + 1).min(h - 1));
        let n = (b - a + 1) as f32;
        for x in 0..w {
            let mut s = [0.0; 3];
            for yy in a..=b {
                let p = tmp[yy * w + x];
                s = [s[0] + p[0], s[1] + p[1], s[2] + p[2]];
            }
            out[y * w + x] = [s[0] / n, s[1] / n, s[2] / n];
        }
    }
    Grid { w, h, data: out }
}

/// Sobel gradient magnitude over all three Oklab channels, scaled so a hard
/// step of ΔE reads ≈ ΔE.
fn gradient(lab: &Grid<Lab>) -> Grid<f32> {
    let (w, h) = (lab.w, lab.h);
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let (ym, yp) = (y.saturating_sub(1), (y + 1).min(h - 1));
        for x in 0..w {
            let (xm, xp) = (x.saturating_sub(1), (x + 1).min(w - 1));
            let mut sum = 0.0;
            for c in 0..3 {
                let p = |xx: usize, yy: usize| lab.at(xx, yy)[c];
                let gx = (p(xp, ym) + 2.0 * p(xp, y) + p(xp, yp)) - (p(xm, ym) + 2.0 * p(xm, y) + p(xm, yp));
                let gy = (p(xm, yp) + 2.0 * p(x, yp) + p(xp, yp)) - (p(xm, ym) + 2.0 * p(x, ym) + p(xp, ym));
                sum += gx * gx + gy * gy;
            }
            out[y * w + x] = sum.sqrt() / 4.0;
        }
    }
    Grid { w, h, data: out }
}

fn median_lab(samples: &mut [Lab]) -> Lab {
    let mut out = [0.0; 3];
    for (c, o) in out.iter_mut().enumerate() {
        samples.sort_by(|a, b| a[c].total_cmp(&b[c]));
        *o = samples[samples.len() / 2][c];
    }
    out
}

/// Square min (erode) or max (dilate) filter of radius `r` on a boolean mask.
fn morph(mask: &[bool], w: usize, h: usize, r: usize, dilate: bool) -> Vec<bool> {
    let pass = |src: &[bool], horizontal: bool| -> Vec<bool> {
        let mut out = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                let (lo, hi, fixed) = if horizontal { (x.saturating_sub(r), (x + r).min(w - 1), y) } else { (y.saturating_sub(r), (y + r).min(h - 1), x) };
                let mut acc = !dilate;
                for i in lo..=hi {
                    let v = if horizontal { src[fixed * w + i] } else { src[i * w + fixed] };
                    if dilate { acc |= v } else { acc &= v }
                }
                out[y * w + x] = acc;
            }
        }
        out
    };
    let a = pass(mask, true);
    pass(&a, false)
}

/// Segments the subject (foreground) from the sky / backdrop of `img`.
pub fn segment(img: &DynamicImage) -> Segmentation {
    let decline = |reason: &str, sky: f32| Segmentation { alpha: None, sky_fraction: sky, reason: reason.to_string() };
    if img.width() < 64 || img.height() < 64 {
        return decline("image too small", 0.0);
    }
    let small = crate::wallpaper::fit_within(img, ANALYSIS_MAX, ANALYSIS_MAX).to_rgb8();
    let (w, h) = (small.width() as usize, small.height() as usize);
    let raw = Grid { w, h, data: small.pixels().map(|p| oklab(p.0)).collect() };
    let lab = box_blur_lab(&raw);
    let grad = gradient(&lab);

    // 1 · Sky model from the top band.
    let band = (h / 20).max(3);
    let mut samples: Vec<Lab> = (0..band).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| lab.at(x, y)).collect();
    let median = median_lab(&mut samples);
    let spread = samples.iter().map(|s| dist(*s, median)).sum::<f32>() / samples.len() as f32;
    let busy = (0..band).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|&(x, y)| grad.at(x, y) > EDGE).count() as f32 / (band * w) as f32;
    if spread > 0.07 || busy > 0.3 {
        return decline("no clear sky or backdrop along the top", 0.0);
    }
    // Textured skies (stars, wisps, grain) raise the edge bar.
    let mut band_grad: Vec<f32> = (0..band).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| grad.at(x, y)).collect();
    band_grad.sort_by(f32::total_cmp);
    let edge = (band_grad[band_grad.len() * 9 / 10] * 2.0).clamp(EDGE, EDGE * 2.0);

    // 2 · Grow the sky downward, row by row.
    let mut sky = vec![false; w * h];
    let seed_tol = (spread * 2.5).max(0.05);
    let mut model = median;
    let mut row_model: Vec<Lab> = vec![median; h];
    let mut last_row = h;
    for y in 0..h {
        let cand = |x: usize| grad.at(x, y) < edge && dist(lab.at(x, y), if y < band { median } else { model }) < if y < band { seed_tol } else { GLOBAL };
        if y < band {
            for x in 0..w {
                sky[y * w + x] = cand(x);
            }
        } else {
            for x in 0..w {
                if !cand(x) {
                    continue;
                }
                let c = lab.at(x, y);
                let above = [x.wrapping_sub(1), x, x + 1].into_iter().filter(|&xx| xx < w).any(|xx| sky[(y - 1) * w + xx] && dist(c, lab.at(xx, y - 1)) < LOCAL);
                sky[y * w + x] = above;
            }
            // Spread sideways along the row (under clouds, around poles).
            for x in 1..w {
                if !sky[y * w + x] && sky[y * w + x - 1] && cand(x) && dist(lab.at(x, y), lab.at(x - 1, y)) < LOCAL {
                    sky[y * w + x] = true;
                }
            }
            for x in (0..w - 1).rev() {
                if !sky[y * w + x] && sky[y * w + x + 1] && cand(x) && dist(lab.at(x, y), lab.at(x + 1, y)) < LOCAL {
                    sky[y * w + x] = true;
                }
            }
        }
        let (mut sum, mut n) = ([0.0f32; 3], 0usize);
        for x in 0..w {
            if sky[y * w + x] {
                let c = lab.at(x, y);
                sum = [sum[0] + c[0], sum[1] + c[1], sum[2] + c[2]];
                n += 1;
            }
        }
        if n == 0 && y >= band {
            last_row = y;
            break;
        }
        if n > 0 {
            let mean = [sum[0] / n as f32, sum[1] / n as f32, sum[2] / n as f32];
            model = if y < band { mean } else { mix(model, mean, 0.2) };
        }
        row_model[y] = model;
    }
    for m in row_model.iter_mut().skip(last_row) {
        *m = model;
    }

    // 3 · Foreground = not sky; keep only parts grounded in the frame
    // (touching the bottom, or a side edge reaching low enough).
    let mut fg: Vec<bool> = sky.iter().map(|s| !s).collect();
    let mut label = vec![u32::MAX; w * h];
    let mut stack = Vec::new();
    let mut comp = 0u32;
    for start in 0..w * h {
        if !fg[start] || label[start] != u32::MAX {
            continue;
        }
        let mut members = Vec::new();
        let (mut bottom, mut side, mut max_y) = (false, false, 0usize);
        label[start] = comp;
        stack.push(start);
        while let Some(i) = stack.pop() {
            members.push(i);
            let (x, y) = (i % w, i / w);
            bottom |= y == h - 1;
            side |= x == 0 || x == w - 1;
            max_y = max_y.max(y);
            let mut visit = |j: usize| {
                if fg[j] && label[j] == u32::MAX {
                    label[j] = comp;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        let grounded = bottom || (side && max_y as f32 >= h as f32 * 0.6);
        if !grounded {
            for i in members {
                fg[i] = false;
            }
        }
        comp += 1;
    }

    // 4 · Morphological cleanup: open (drop spurs / noise), close (notches).
    let fg = morph(&morph(&fg, w, h, 1, false), w, h, 1, true);
    let fg = morph(&morph(&fg, w, h, 2, true), w, h, 2, false);

    // 5 · Confidence.
    let total = (w * h) as f32;
    let fg_count = fg.iter().filter(|v| **v).count() as f32;
    let sky_fraction = 1.0 - fg_count / total;
    let top_sky = (0..w).filter(|&x| !fg[x]).count() as f32 / w as f32;
    let bottom_fg = (0..w).filter(|&x| fg[(h - 1) * w + x]).count() as f32 / w as f32;
    let (mut boundary, mut contrast, mut crisp) = (0usize, 0.0f32, 0usize);
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            if fg[i] && (!fg[i - 1] || !fg[i + 1] || !fg[i - w] || !fg[i + w]) {
                boundary += 1;
                // A real outline is a local step: compare a few pixels
                // inside the subject with a few pixels out in the sky (a
                // growth that merely ran out of tolerance in a gradient sky
                // has no step and reads as soft).
                let (dx, dy): (isize, isize) = if !fg[i - w] { (0, 1) } else if !fg[i + w] { (0, -1) } else if !fg[i - 1] { (1, 0) } else { (-1, 0) };
                let at = |k: isize| {
                    let ix = (x as isize + k * dx).clamp(0, w as isize - 1) as usize;
                    let iy = (y as isize + k * dy).clamp(0, h as isize - 1) as usize;
                    lab.at(ix, iy)
                };
                let d = dist(at(3), at(-3));
                contrast += d;
                crisp += (d >= 0.06) as usize;
            }
        }
    }
    let contrast = if boundary > 0 { contrast / boundary as f32 } else { 0.0 };
    let crisp = if boundary > 0 { crisp as f32 / boundary as f32 } else { 0.0 };
    let jagged = boundary as f32 / w as f32;
    // Subject pixels that look just like the sky beside them: the sky was
    // only partly found.
    let (mut fg_upper, mut sky_like) = (0usize, 0usize);
    for y in 0..last_row.min(h) {
        for x in 0..w {
            if fg[y * w + x] {
                fg_upper += 1;
                sky_like += (dist(lab.at(x, y), row_model[y]) < 0.05) as usize;
            }
        }
    }
    let sky_like = if fg_upper > 0 { sky_like as f32 / fg_upper as f32 } else { 0.0 };
    let stats = format!("sky {:.0}%, top {:.2}, bottom {:.2}, contrast {:.2}, crisp {:.2}, jagged {:.1}, sky-like {:.2}", sky_fraction * 100.0, top_sky, bottom_fg, contrast, crisp, jagged, sky_like);
    let decline = |reason: &str, sky: f32| decline(&format!("{reason} ({stats})"), sky);
    if !(0.17..=0.85).contains(&sky_fraction) {
        return decline(&format!("sky covers {:.0}% of the frame", sky_fraction * 100.0), sky_fraction);
    }
    if top_sky < 0.5 {
        return decline("the subject fills the top edge", sky_fraction);
    }
    if bottom_fg < 0.5 {
        return decline("no subject grounded at the bottom", sky_fraction);
    }
    if contrast < 0.08 || crisp < 0.75 {
        return decline("subject and sky are too similar", sky_fraction);
    }
    if sky_like > 0.2 {
        return decline("part of the sky looks like the subject", sky_fraction);
    }
    if jagged > 10.0 {
        return decline("outline too fragmented", sky_fraction);
    }

    // 6 · Feathered alpha at mask resolution with colour-guided edges.
    let mut soft: Vec<f32> = fg.iter().map(|v| if *v { 1.0 } else { 0.0 }).collect();
    for _ in 0..2 {
        let g = Grid { w, h, data: soft.iter().map(|v| [*v, 0.0, 0.0]).collect() };
        soft = box_blur_lab(&g).data.into_iter().map(|v| v[0]).collect();
    }
    let scale = (MASK_MAX as f32 / img.width().max(img.height()) as f32).min(1.0);
    let (ow, oh) = (((img.width() as f32 * scale).round() as u32).max(1), ((img.height() as f32 * scale).round() as u32).max(1));
    let full: RgbImage = if (ow, oh) == (img.width(), img.height()) { img.to_rgb8() } else { crate::wallpaper::shrink(img, ow, oh).to_rgb8() };
    let sample = |fx: f32, fy: f32| -> f32 {
        let (fx, fy) = (fx.clamp(0.0, (w - 1) as f32), fy.clamp(0.0, (h - 1) as f32));
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let a = soft[y0 * w + x0] * (1.0 - tx) + soft[y0 * w + x1] * tx;
        let b = soft[y1 * w + x0] * (1.0 - tx) + soft[y1 * w + x1] * tx;
        a * (1.0 - ty) + b * ty
    };
    let (sx, sy) = (w as f32 / ow as f32, h as f32 / oh as f32);
    let alpha = GrayImage::from_fn(ow, oh, |x, y| {
        let a = sample((x as f32 + 0.5) * sx - 0.5, (y as f32 + 0.5) * sy - 0.5);
        let mut out = smoothstep(0.3, 0.7, a);
        if a > 0.02 && a < 0.98 {
            // Inside the transition band: pixels that look like the local
            // sky go transparent, unlike ones stay — crisp foliage edges.
            let row = ((y as f32 + 0.5) * sy).min((h - 1) as f32) as usize;
            let unlike = smoothstep(0.03, 0.10, dist(oklab(full.get_pixel(x, y).0), row_model[row]));
            let k = 0.8 * (1.0 - (2.0 * a - 1.0).abs());
            out = out * (1.0 - k) + unlike * k;
        }
        image::Luma([(out * 255.0).round() as u8])
    });
    Segmentation { alpha: Some(alpha), sky_fraction, reason: stats }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Blue gradient sky over a dark green "mountain" ridge.
    fn landscape(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            let ridge = h as f32 * (0.55 + 0.12 * (x as f32 / w as f32 * 6.0).sin());
            if (y as f32) < ridge {
                let t = y as f32 / ridge;
                image::Rgb([(40.0 + 120.0 * t) as u8, (90.0 + 110.0 * t) as u8, (200.0 + 40.0 * t) as u8])
            } else {
                image::Rgb([30, 70 + (x % 7) as u8, 35])
            }
        }))
    }

    #[test]
    fn finds_a_ridge_under_a_gradient_sky() {
        let img = landscape(960, 540);
        let seg = segment(&img);
        let alpha = seg.alpha.unwrap_or_else(|| panic!("declined: {}", seg.reason));
        let (w, h) = (alpha.width(), alpha.height());
        // Sky transparent, ground opaque, well inside each region.
        assert!(alpha.get_pixel(w / 2, h / 10).0[0] < 10);
        assert!(alpha.get_pixel(w / 2, h - 5).0[0] > 245);
        assert!((0.3..0.7).contains(&seg.sky_fraction), "{}", seg.sky_fraction);
    }

    #[test]
    fn floating_clouds_stay_sky() {
        let mut img = landscape(960, 540).to_rgb8();
        for y in 60..110 {
            for x in 300..520 {
                img.put_pixel(x, y, image::Rgb([245, 245, 250]));
            }
        }
        let seg = segment(&DynamicImage::ImageRgb8(img));
        let alpha = seg.alpha.expect("confident");
        let s = alpha.width() as f32 / 960.0;
        assert!(alpha.get_pixel((410.0 * s) as u32, (85.0 * s) as u32).0[0] < 10, "cloud is not a subject");
    }

    #[test]
    fn declines_flat_and_busy_images() {
        let flat = DynamicImage::ImageRgb8(RgbImage::from_pixel(800, 450, image::Rgb([20, 30, 60])));
        assert!(segment(&flat).alpha.is_none());
        let noise = DynamicImage::ImageRgb8(RgbImage::from_fn(800, 450, |x, y| {
            let v = ((x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) >> 7) as u8;
            image::Rgb([v, v.wrapping_mul(3), v.wrapping_mul(7)])
        }));
        assert!(segment(&noise).alpha.is_none());
        // Subject fills the top: nothing to tuck the clock behind.
        let tall = DynamicImage::ImageRgb8(RgbImage::from_fn(800, 450, |x, _| if x < 400 { image::Rgb([200, 40, 40]) } else { image::Rgb([40, 40, 200]) }));
        assert!(segment(&tall).alpha.is_none());
    }

    /// Segments every image in `$MANDARA_DEPTH_DIR` and writes red-tinted
    /// overlays to `$MANDARA_DEPTH_OUT` for eyeballing the heuristic:
    /// `MANDARA_DEPTH_DIR=~/Pictures/Wallpapers MANDARA_DEPTH_OUT=/tmp/d cargo test -p mandara-ui -- --ignored depth_folder`.
    #[test]
    #[ignore]
    fn depth_folder_overlays() {
        let (Ok(dir), Ok(out)) = (std::env::var("MANDARA_DEPTH_DIR"), std::env::var("MANDARA_DEPTH_OUT")) else { return };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let limit: usize = std::env::var("MANDARA_DEPTH_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
        let (mut ok, mut total) = (0, 0);
        for path in crate::wallpaper::list(std::path::Path::new(&dir)).into_iter().take(limit) {
            let Ok(img) = image::open(&path) else { continue };
            total += 1;
            let t = std::time::Instant::now();
            let seg = segment(&img);
            let ms = t.elapsed().as_millis();
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            println!("{name}: {} ({ms} ms){}", seg.reason, if seg.alpha.is_some() { " MASK" } else { "" });
            if let Some(alpha) = seg.alpha {
                ok += 1;
                let base = img.resize_exact(alpha.width(), alpha.height(), image::imageops::FilterType::Triangle).to_rgb8();
                let overlay = RgbImage::from_fn(alpha.width(), alpha.height(), |x, y| {
                    let a = alpha.get_pixel(x, y).0[0] as f32 / 255.0;
                    let p = base.get_pixel(x, y).0;
                    image::Rgb([(p[0] as f32 * (1.0 - 0.5 * a) + 255.0 * 0.5 * a) as u8, (p[1] as f32 * (1.0 - 0.5 * a)) as u8, (p[2] as f32 * (1.0 - 0.5 * a)) as u8])
                });
                let small = image::imageops::thumbnail(&overlay, 640, 640 * overlay.height() / overlay.width().max(1));
                small.save(out.join(format!("{name}.jpg"))).unwrap();
            }
        }
        println!("masks: {ok} / {total}");
    }
}
