//! `matte OUT.png [--region X0 Y0 X1 Y1] [--margin PX]`: a transparent cut-out
//! of the sandbox's open surfaces (Studio, panels) by difference matting —
//! one shot over a black wallpaper, one over a white one. Faint alpha (drop
//! shadows) is cut, anti-aliased edges are kept, and the result is cropped to
//! the surfaces' bounds. `--region` (physical pixels) limits the search;
//! `--margin` forces everything deeper than that inside the bounds opaque,
//! for windows whose content changes between the two shots.
//!
//! Pick a region around the window that leaves out the bar and the dock;
//! for Studio at the default 2560x1440: `matte studio.png --region 0 70 2560
//! 1320 --margin 40`.

use std::time::Duration;

use image::{Rgba, RgbaImage, RgbImage};

use super::{OUTPUT, Sandbox, demo};
use crate::{Result, opt};

/// Alpha below this is shadow (cut); above `ALPHA_SOLID` fully opaque.
const ALPHA_CUT: f32 = 0.55;
const ALPHA_SOLID: f32 = 0.92;
/// A pixel counts as part of the surface for its bounds above this.
const BOUNDS_ALPHA: f32 = 0.5;
/// Smallest alpha used to un-premultiply edge colors.
const MIN_ALPHA: f32 = 1e-3;
/// Time for a wallpaper change to finish its crossfade.
const WALLPAPER_SETTLE: Duration = Duration::from_millis(2200);

pub fn matte(sb: &Sandbox, args: &[String]) -> Result<()> {
    sb.verify()?;
    let usage = "usage: cargo xtask sandbox matte OUT.png [--region X0 Y0 X1 Y1] [--margin PX]";
    let out = args.first().filter(|a| !a.starts_with("--")).ok_or(usage)?;
    let region = match args.iter().position(|a| a == "--region") {
        Some(i) => {
            let n: Vec<u32> = args.get(i + 1..i + 5).ok_or(usage)?.iter().map(|v| v.parse().map_err(|_| usage)).collect::<std::result::Result<_, _>>()?;
            Some((n[0], n[1], n[2], n[3]))
        }
        None => None,
    };
    let margin = opt(args, "--margin").map(|m| m.parse::<u32>().map_err(|_| usage)).transpose()?;

    let (black, white) = demo::matte_paths(sb);
    let cap = sb.root.join("cap");
    std::fs::create_dir_all(&cap).map_err(|e| e.to_string())?;
    let mut shots = Vec::new();
    for (wall, name) in [(black, "matte-black.png"), (white, "matte-white.png")] {
        sb.msg(&["wallpaper".into(), "output".into(), OUTPUT.into(), wall.display().to_string()], true)?;
        std::thread::sleep(WALLPAPER_SETTLE);
        let path = super::shot(sb, &cap.join(name).display().to_string())?;
        shots.push(image::open(&path).map_err(|e| format!("{}: {e}", path.display()))?.to_rgb8());
    }
    sb.msg(&["wallpaper".into(), "output".into(), OUTPUT.into(), "clear".into()], true)?;
    let img = compose(&shots[0], &shots[1], region, margin).ok_or("nothing to cut out: no opaque surface in the region")?;
    img.save(out).map_err(|e| format!("{out}: {e}"))?;
    println!("{out} {}x{}", img.width(), img.height());
    Ok(())
}

/// The cut-out from shots over black (`b`) and white (`w`).
fn compose(b: &RgbImage, w: &RgbImage, region: Option<(u32, u32, u32, u32)>, margin: Option<u32>) -> Option<RgbaImage> {
    let (width, height) = b.dimensions();
    let (x0, y0, x1, y1) = region.unwrap_or((0, 0, width, height));
    let (x1, y1) = (x1.min(width).min(w.width()), y1.min(height).min(w.height()));
    let (rw, rh) = (x1.checked_sub(x0)?, y1.checked_sub(y0)?);
    let idx = |x: u32, y: u32| (y * rw + x) as usize;
    let mut alpha = vec![0.0f32; (rw * rh) as usize];
    let mut cut = vec![0.0f32; alpha.len()];
    for y in 0..rh {
        for x in 0..rw {
            let (pb, pw) = (b.get_pixel(x0 + x, y0 + y).0, w.get_pixel(x0 + x, y0 + y).0);
            let diff = (0..3).map(|i| f32::from(pw[i]) - f32::from(pb[i])).sum::<f32>() / 3.0;
            let a = (1.0 - diff / f32::from(u8::MAX)).clamp(0.0, 1.0);
            alpha[idx(x, y)] = a;
            cut[idx(x, y)] = ((a - ALPHA_CUT) / (ALPHA_SOLID - ALPHA_CUT)).clamp(0.0, 1.0);
        }
    }
    let bounds = |cut: &[f32], min: f32| -> Option<(u32, u32, u32, u32)> {
        let (mut l, mut t, mut r, mut btm) = (u32::MAX, u32::MAX, 0, 0);
        for y in 0..rh {
            for x in 0..rw {
                if cut[idx(x, y)] > min {
                    (l, t, r, btm) = (l.min(x), t.min(y), r.max(x), btm.max(y));
                }
            }
        }
        (l <= r).then_some((l, t, r, btm))
    };
    if let Some(m) = margin {
        let (l, t, r, btm) = bounds(&cut, BOUNDS_ALPHA)?;
        for y in t..=btm {
            for x in l..=r {
                let deep_rows = y >= t + m && y + m <= btm;
                let deep_cols = x >= l + m && x + m <= r;
                if deep_rows || deep_cols {
                    cut[idx(x, y)] = 1.0;
                }
            }
        }
    }
    let (l, t, r, btm) = bounds(&cut, 0.0)?;
    let mut out = RgbaImage::new(r - l + 1, btm - t + 1);
    for (x, y, px) in out.enumerate_pixels_mut() {
        let (sx, sy) = (l + x, t + y);
        let (a, c) = (alpha[idx(sx, sy)], cut[idx(sx, sy)]);
        let src = b.get_pixel(x0 + sx, y0 + sy).0;
        let color = |v: u8| if c >= 1.0 { v } else { (f32::from(v) / a.max(MIN_ALPHA)).clamp(0.0, f32::from(u8::MAX)) as u8 };
        *px = Rgba([color(src[0]), color(src[1]), color(src[2]), (c * f32::from(u8::MAX)) as u8]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn cuts_an_opaque_square_out() {
        // A 4x4 red square on 10x10: identical over black and white.
        let mut b = RgbImage::from_pixel(10, 10, Rgb([0, 0, 0]));
        let mut w = RgbImage::from_pixel(10, 10, Rgb([255, 255, 255]));
        for y in 3..7 {
            for x in 3..7 {
                b.put_pixel(x, y, Rgb([200, 10, 10]));
                w.put_pixel(x, y, Rgb([200, 10, 10]));
            }
        }
        let out = compose(&b, &w, None, None).unwrap();
        assert_eq!(out.dimensions(), (4, 4));
        assert_eq!(out.get_pixel(0, 0).0, [200, 10, 10, 255]);
        assert!(compose(&b, &w, Some((0, 0, 2, 2)), None).is_none());
    }

    #[test]
    fn margin_fills_the_interior() {
        // A frame whose middle changed between the shots still comes out opaque.
        let mut b = RgbImage::from_pixel(12, 12, Rgb([0, 0, 0]));
        let mut w = RgbImage::from_pixel(12, 12, Rgb([255, 255, 255]));
        for y in 2..10 {
            for x in 2..10 {
                b.put_pixel(x, y, Rgb([50, 50, 50]));
                w.put_pixel(x, y, Rgb([50, 50, 50]));
            }
        }
        w.put_pixel(5, 5, Rgb([255, 255, 255]));
        assert!(compose(&b, &w, None, None).unwrap().get_pixel(3, 3).0[3] < u8::MAX);
        assert_eq!(compose(&b, &w, None, Some(2)).unwrap().get_pixel(3, 3).0[3], u8::MAX);
    }
}
