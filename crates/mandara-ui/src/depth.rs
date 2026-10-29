//! Depth layer 3 of the wallpaper canvas: the foreground subject alpha mask
//! that is drawn *over* the desktop clock so mountains, buildings or people
//! in the photo overlap the typography.
//!
//! Masks come from (in order) the user — any PNG with alpha (or a grayscale
//! matte) named after the wallpaper's file stem in
//! `~/.local/share/mandara/depth_masks/` — or a pure-Rust sky / subject
//! segmentation cached as `~/.cache/mandara/depth_masks/<wallpaper-hash>.png`.
//!
//! The segmentation is a deliberately conservative heuristic: estimate the
//! sky from the top band (Oklab colour + edge density), grow it downward row
//! by row with local colour continuity, a drifting colour model (gradient
//! skies) and edge barriers, drop floating foreground islands (clouds,
//! birds, the moon), clean up morphologically, and feather the edge with a
//! colour-guided refinement at full resolution. A battery of confidence
//! checks rejects anything doubtful; a rejected wallpaper simply gets no
//! depth effect (remembered in `<hash>.none`) instead of a botched cut-out.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use gpui::{App, AppContext as _, Context, Entity, Global};
use image::{DynamicImage, GrayImage, RgbImage, RgbaImage};
use mandara_config::DepthMask;

/// Longest side of the image the segmentation runs on.
const ANALYSIS_MAX: u32 = 640;
/// Longest side of the stored mask.
const MASK_MAX: u32 = 1600;
/// Per-pixel Oklab gradient above which a pixel is an edge (sky stops).
const EDGE: f32 = 0.035;
/// Neighbour colour step allowed while growing the sky.
const LOCAL: f32 = 0.03;
/// Distance from the drifting sky model allowed while growing.
const GLOBAL: f32 = 0.12;

// ── Paths ────────────────────────────────────────────────────────────────

/// Stable hash of a wallpaper file (path, size, mtime): FNV-1a, so cache
/// names survive toolchain upgrades.
pub fn wallpaper_hash(path: &Path) -> String {
    let meta = std::fs::metadata(path).ok();
    let mtime = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    let text = format!("{}\u{1f}{}\u{1f}{mtime}", path.display(), meta.map(|m| m.len()).unwrap_or(0));
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Generated mask for `wallpaper`.
pub fn generated_path(wallpaper: &Path) -> PathBuf {
    mandara_config::paths::depth_masks_dir().join(format!("{}.png", wallpaper_hash(wallpaper)))
}

/// Marker recording that segmentation declined this wallpaper (and why).
fn miss_path(wallpaper: &Path) -> PathBuf {
    mandara_config::paths::depth_masks_dir().join(format!("{}.none", wallpaper_hash(wallpaper)))
}

/// Where a user-provided mask for `wallpaper` lives.
pub fn user_mask_path(wallpaper: &Path) -> Option<PathBuf> {
    let stem = wallpaper.file_stem()?;
    Some(mandara_config::paths::data_dir().join("depth_masks").join(stem).with_extension("png"))
}

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

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
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
    let small = img.resize(ANALYSIS_MAX, ANALYSIS_MAX, image::imageops::FilterType::Triangle).to_rgb8();
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
    let full: RgbImage = if (ow, oh) == (img.width(), img.height()) { img.to_rgb8() } else { img.resize_exact(ow, oh, image::imageops::FilterType::Triangle).to_rgb8() };
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

// ── Masks on disk ────────────────────────────────────────────────────────

/// Alpha of a mask image: its alpha channel when it has real transparency,
/// else its luminance (a grayscale matte).
pub fn load_alpha(path: &Path) -> Result<GrayImage, String> {
    let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        if rgba.pixels().any(|p| p.0[3] < 250) {
            return Ok(GrayImage::from_fn(rgba.width(), rgba.height(), |x, y| image::Luma([rgba.get_pixel(x, y).0[3]])));
        }
    }
    Ok(img.to_luma8())
}

fn write_mask(alpha: &GrayImage, out: &Path) -> Result<(), String> {
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let rgba = RgbaImage::from_fn(alpha.width(), alpha.height(), |x, y| image::Rgba([255, 255, 255, alpha.get_pixel(x, y).0[0]]));
    let tmp = out.with_extension("tmp");
    rgba.save_with_format(&tmp, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, out).map_err(|e| e.to_string())
}

/// A resolved mask and where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub path: PathBuf,
    /// `custom` or `generated`.
    pub source: &'static str,
    pub detail: String,
}

/// Finds or generates the mask for `wallpaper` (blocking; run off the UI
/// thread). `Err` explains why there is none. `force` regenerates.
pub fn resolve(wallpaper: &Path, mode: DepthMask, force: bool) -> Result<Resolved, String> {
    if mode == DepthMask::Off {
        return Err("depth mask is off".into());
    }
    if mandara_media::is_video(wallpaper) {
        return Err("video wallpapers have no depth subject (it moves)".into());
    }
    if let Some(user) = user_mask_path(wallpaper).filter(|p| p.is_file()) {
        return Ok(Resolved { path: user, source: "custom", detail: String::new() });
    }
    if mode == DepthMask::Custom {
        let want = user_mask_path(wallpaper).map(|p| p.display().to_string()).unwrap_or_default();
        return Err(format!("no custom mask (add {want})"));
    }
    let (out, miss) = (generated_path(wallpaper), miss_path(wallpaper));
    if force {
        let _ = std::fs::remove_file(&out);
        let _ = std::fs::remove_file(&miss);
    }
    if out.is_file() {
        return Ok(Resolved { path: out, source: "generated", detail: "cached".into() });
    }
    if let Ok(reason) = std::fs::read_to_string(&miss) {
        return Err(format!("skipped: {}", reason.trim()));
    }
    let img = image::open(wallpaper).map_err(|e| format!("{}: {e}", wallpaper.display()))?;
    let seg = segment(&img);
    match seg.alpha {
        Some(alpha) => {
            write_mask(&alpha, &out)?;
            Ok(Resolved { path: out, source: "generated", detail: seg.reason })
        }
        None => {
            if let Some(dir) = miss.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&miss, &seg.reason);
            Err(format!("skipped: {}", seg.reason))
        }
    }
}

/// The mask aligned to a displayed image of `size`: the display copy is
/// either the original (stretch to fit) or its centre crop (`cropped`).
pub fn align(alpha: &GrayImage, size: (u32, u32), cropped: bool) -> GrayImage {
    let (w, h) = size;
    if !cropped || (alpha.width(), alpha.height()) == (w, h) {
        return image::imageops::resize(alpha, w, h, image::imageops::FilterType::Triangle);
    }
    let dynimg = DynamicImage::ImageLuma8(alpha.clone());
    crate::wallpaper::cover(&dynimg, w, h).to_luma8()
}

/// Layer 3 pixels: the displayed wallpaper with the subject alpha, as
/// straight BGRA (what GPUI's `RenderImage` expects), plus a small copy of
/// the aligned alpha for [`coverage`] queries.
pub fn cutout(display: &Path, source: &Path, mask: &Path) -> Result<CutoutImage, String> {
    let shown = image::open(display).map_err(|e| format!("{}: {e}", display.display()))?.to_rgb8();
    let alpha = align(&load_alpha(mask)?, (shown.width(), shown.height()), display != source);
    let small = image::imageops::thumbnail(&alpha, COVERAGE_MAP_W, (COVERAGE_MAP_W * alpha.height() / alpha.width().max(1)).max(1));
    // Only the subject's pixels differ from the wallpaper drawn beneath, so
    // the layer-3 texture is cropped to the mask's opaque bounding box:
    // outside it the cut-out is fully transparent. A full-output BGRA cut-out
    // was ~33 MB in RAM and again in the GPU atlas.
    let crop = alpha_bounds(&alpha).unwrap_or((0, 0, 1, 1));
    let (cx0, cy0, cw, ch) = crop;
    let bgra = RgbaImage::from_fn(cw, ch, |x, y| {
        let p = shown.get_pixel(cx0 + x, cy0 + y).0;
        image::Rgba([p[2], p[1], p[0], alpha.get_pixel(cx0 + x, cy0 + y).0[0]])
    });
    Ok(CutoutImage { bgra, map: small, full: (shown.width(), shown.height()), crop })
}

/// Width of the coarse subject-coverage map, in px.
pub(crate) const COVERAGE_MAP_W: u32 = 256;

/// A layer-3 cut-out cropped to its subject.
pub struct CutoutImage {
    /// Premultiplied-free BGRA pixels of the crop.
    pub bgra: RgbaImage,
    /// Small aligned alpha of the whole frame (for coverage queries).
    pub map: GrayImage,
    /// Size of the full displayed image the crop was taken from.
    pub full: (u32, u32),
    /// The crop `(x, y, w, h)` in full-image pixels.
    pub crop: (u32, u32, u32, u32),
}

/// Bounding box `(x, y, w, h)` of the non-zero pixels, or `None` if empty.
pub(crate) fn alpha_bounds(alpha: &GrayImage) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in alpha.enumerate_pixels() {
        if p.0[0] > 0 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    (x0 <= x1 && y0 <= y1).then(|| (x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

/// Where a crop of an image drawn with `object-fit: cover` into a
/// `viewport` lands, as `(x, y, w, h)` in viewport units.
pub fn cover_rect(full: (u32, u32), crop: (u32, u32, u32, u32), viewport: (f32, f32)) -> (f32, f32, f32, f32) {
    let (iw, ih) = (full.0.max(1) as f32, full.1.max(1) as f32);
    let scale = (viewport.0 / iw).max(viewport.1 / ih);
    let (ox, oy) = ((viewport.0 - iw * scale) / 2.0, (viewport.1 - ih * scale) / 2.0);
    (ox + crop.0 as f32 * scale, oy + crop.1 as f32 * scale, crop.2 as f32 * scale, crop.3 as f32 * scale)
}

/// Mean subject alpha (`0..=1`) over a rectangle given in fractions of the
/// frame `(x0, y0, x1, y1)`.
pub fn coverage(map: &GrayImage, rect: (f32, f32, f32, f32)) -> f32 {
    let (w, h) = (map.width() as f32, map.height() as f32);
    let x0 = (rect.0.clamp(0.0, 1.0) * w) as u32;
    let x1 = ((rect.2.clamp(0.0, 1.0) * w).ceil() as u32).clamp(x0 + 1, map.width());
    let y0 = (rect.1.clamp(0.0, 1.0) * h) as u32;
    let y1 = ((rect.3.clamp(0.0, 1.0) * h).ceil() as u32).clamp(y0 + 1, map.height());
    if x0 >= map.width() || y0 >= map.height() {
        return 0.0;
    }
    let (mut sum, mut n) = (0u64, 0u64);
    for y in y0..y1 {
        for x in x0..x1 {
            sum += map.get_pixel(x, y).0[0] as u64;
            n += 1;
        }
    }
    sum as f32 / (n.max(1) * 255) as f32
}

/// How much of the cut-out to draw: a subject may overlap the clock, but
/// never swallow it (fades out between 45 % and 70 % coverage).
pub fn overlap_alpha(coverage: f32) -> f32 {
    1.0 - smoothstep(0.45, 0.7, coverage)
}

// ── Shared state (one job per wallpaper, all outputs) ────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum MaskStatus {
    Pending,
    Ready(Resolved),
    Unavailable(String),
}

type Reply = async_channel::Sender<Result<String, String>>;

#[derive(Default)]
pub struct DepthMasks {
    states: HashMap<(PathBuf, DepthMask), MaskStatus>,
    waiters: HashMap<(PathBuf, DepthMask), Vec<Reply>>,
    /// Bumped on regeneration so views rebuild their cut-outs.
    pub generation: u64,
}

#[derive(Clone)]
struct GlobalDepthMasks(Entity<DepthMasks>);
impl Global for GlobalDepthMasks {}

pub fn masks(cx: &mut App) -> Entity<DepthMasks> {
    if let Some(g) = cx.try_global::<GlobalDepthMasks>() {
        return g.0.clone();
    }
    let entity = cx.new(|_| DepthMasks::default());
    cx.set_global(GlobalDepthMasks(entity.clone()));
    entity
}

fn describe(status: &MaskStatus) -> Result<String, String> {
    match status {
        MaskStatus::Ready(r) => Ok(serde_json::json!({ "mask": r.path, "source": r.source, "detail": r.detail }).to_string()),
        MaskStatus::Unavailable(why) => Err(why.clone()),
        MaskStatus::Pending => Err("still generating".into()),
    }
}

impl DepthMasks {
    /// The mask status for `wallpaper`, starting a background job when it
    /// isn't known yet.
    pub fn status(&mut self, wallpaper: &Path, mode: DepthMask, cx: &mut Context<Self>) -> MaskStatus {
        if mode == DepthMask::Off {
            return MaskStatus::Unavailable("off".into());
        }
        let key = (wallpaper.to_path_buf(), mode);
        if let Some(s) = self.states.get(&key) {
            return s.clone();
        }
        self.start(key, false, None, cx);
        MaskStatus::Pending
    }

    /// Peeks without starting anything.
    pub fn known(&self, wallpaper: &Path, mode: DepthMask) -> Option<MaskStatus> {
        self.states.get(&(wallpaper.to_path_buf(), mode)).cloned()
    }

    /// Discards the cached mask for `wallpaper` and builds it again; `reply`
    /// receives the outcome.
    pub fn regenerate(&mut self, wallpaper: &Path, mode: DepthMask, reply: Option<Reply>, cx: &mut Context<Self>) {
        self.states.retain(|(p, _), _| p != wallpaper);
        self.generation += 1;
        self.start((wallpaper.to_path_buf(), mode), true, reply, cx);
        cx.notify();
    }

    fn start(&mut self, key: (PathBuf, DepthMask), force: bool, reply: Option<Reply>, cx: &mut Context<Self>) {
        if let Some(r) = reply {
            self.waiters.entry(key.clone()).or_default().push(r);
        }
        if !force && self.states.get(&key) == Some(&MaskStatus::Pending) {
            return;
        }
        self.states.insert(key.clone(), MaskStatus::Pending);
        let (path, mode) = key.clone();
        let task = cx.background_executor().spawn(async move { resolve(&path, mode, force) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let status = match result {
                    Ok(r) => MaskStatus::Ready(r),
                    Err(e) => {
                        tracing::info!("depth mask for {}: {e}", key.0.display());
                        MaskStatus::Unavailable(e)
                    }
                };
                for w in this.waiters.remove(&key).unwrap_or_default() {
                    let _ = w.try_send(describe(&status));
                }
                this.states.insert(key, status);
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutout_crop_bounds_and_cover_mapping() {
        let mut a = GrayImage::new(10, 8);
        a.put_pixel(2, 3, image::Luma([200]));
        a.put_pixel(6, 5, image::Luma([1]));
        assert_eq!(alpha_bounds(&a), Some((2, 3, 5, 3)));
        assert_eq!(alpha_bounds(&GrayImage::new(4, 4)), None);
        // Same aspect: cover is a plain scale.
        assert_eq!(cover_rect((100, 50), (10, 5, 20, 10), (200.0, 100.0)), (20.0, 10.0, 40.0, 20.0));
        // Wider image than viewport: centred horizontally.
        let r = cover_rect((200, 100), (0, 0, 200, 100), (100.0, 100.0));
        assert_eq!(r, (-50.0, 0.0, 200.0, 100.0));
    }

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

    #[test]
    fn mask_alpha_from_alpha_or_luminance() {
        let dir = std::env::temp_dir().join(format!("mandara-depth-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let with_alpha = dir.join("a.png");
        RgbaImage::from_fn(4, 4, |x, _| image::Rgba([10, 20, 30, if x < 2 { 0 } else { 255 }])).save(&with_alpha).unwrap();
        let a = load_alpha(&with_alpha).unwrap();
        assert_eq!((a.get_pixel(0, 0).0[0], a.get_pixel(3, 0).0[0]), (0, 255));
        let matte = dir.join("m.png");
        GrayImage::from_fn(4, 4, |x, _| image::Luma([if x < 2 { 0 } else { 255 }])).save(&matte).unwrap();
        let m = load_alpha(&matte).unwrap();
        assert_eq!((m.get_pixel(0, 0).0[0], m.get_pixel(3, 0).0[0]), (0, 255));
        // Aligned to a centre-cropped display copy.
        let aligned = align(&m, (2, 4), true);
        assert_eq!((aligned.width(), aligned.height()), (2, 4));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn coverage_and_overlap_limit() {
        let map = GrayImage::from_fn(100, 100, |_, y| image::Luma([if y >= 50 { 255 } else { 0 }]));
        assert!(coverage(&map, (0.0, 0.0, 1.0, 0.4)) < 0.01);
        assert!((coverage(&map, (0.0, 0.0, 1.0, 1.0)) - 0.5).abs() < 0.02);
        assert!(coverage(&map, (0.2, 0.6, 0.8, 0.9)) > 0.99);
        assert_eq!(overlap_alpha(0.2), 1.0);
        assert_eq!(overlap_alpha(0.9), 0.0);
    }

    #[test]
    fn hash_is_stable_and_file_specific() {
        let a = wallpaper_hash(Path::new("/nonexistent/a.jpg"));
        assert_eq!(a, wallpaper_hash(Path::new("/nonexistent/a.jpg")));
        assert_ne!(a, wallpaper_hash(Path::new("/nonexistent/b.jpg")));
        assert_eq!(a.len(), 16);
        assert!(user_mask_path(Path::new("/w/dunes.jpg")).unwrap().ends_with("depth_masks/dunes.png"));
    }

    /// Segments every image in `$MANDARA_DEPTH_DIR` and writes red-tinted
    /// overlays to `$MANDARA_DEPTH_OUT` for eyeballing the heuristic:
    /// `MANDARA_DEPTH_DIR=~/Pictures/Wallpapers MANDARA_DEPTH_OUT=/tmp/d cargo test -p mandara-ui -- --ignored depth_folder`.
    #[test]
    #[ignore]
    fn depth_folder_overlays() {
        let (Ok(dir), Ok(out)) = (std::env::var("MANDARA_DEPTH_DIR"), std::env::var("MANDARA_DEPTH_OUT")) else { return };
        let out = PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let limit: usize = std::env::var("MANDARA_DEPTH_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
        let (mut ok, mut total) = (0, 0);
        for path in crate::wallpaper::list(Path::new(&dir)).into_iter().take(limit) {
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
