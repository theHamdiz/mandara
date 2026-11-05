//! Assemble physical captures in logical monitor order. Mixed-scale outputs
//! share the highest pixel density, and gaps in the layout stay transparent.

use anyhow::{Context as _, Result, bail};
use super::{Capture, RGBA_BYTES};

/// Bound a combined desktop to 64 million RGBA pixels before allocating.
const MAX_PIXELS: u64 = 64 * 1024 * 1024;

pub fn composite(captures: &[Capture]) -> Result<image::RgbaImage> {
    if captures.is_empty() { bail!("no outputs to capture"); }
    let mut bounds = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    let mut scale = 1.0f64;
    for c in captures {
        let (x, y, w, h) = c.logical;
        if w <= 0 || h <= 0 || c.width == 0 || c.height == 0 { bail!("invalid capture geometry"); }
        let bytes = u64::from(c.width).checked_mul(u64::from(c.height)).and_then(|n| n.checked_mul(RGBA_BYTES as u64)).context("capture size overflow")?;
        if bytes != c.rgba.len() as u64 { bail!("incomplete output capture"); }
        bounds.0 = bounds.0.min(i64::from(x));
        bounds.1 = bounds.1.min(i64::from(y));
        bounds.2 = bounds.2.max(i64::from(x) + i64::from(w));
        bounds.3 = bounds.3.max(i64::from(y) + i64::from(h));
        scale = scale.max(f64::from(c.width) / f64::from(w)).max(f64::from(c.height) / f64::from(h));
    }
    let dimension = |n: i64| -> Result<u32> {
        let pixels = (n as f64 * scale).round();
        if !pixels.is_finite() || pixels < 1.0 || pixels > u32::MAX as f64 { bail!("combined capture dimensions overflow"); }
        Ok(pixels as u32)
    };
    let (width, height) = (dimension(bounds.2 - bounds.0)?, dimension(bounds.3 - bounds.1)?);
    if u64::from(width) * u64::from(height) > MAX_PIXELS { bail!("combined capture exceeds the image size limit"); }
    let mut image = image::RgbaImage::new(width, height);
    for c in captures {
        let x = ((i64::from(c.logical.0) - bounds.0) as f64 * scale).round() as u32;
        let y = ((i64::from(c.logical.1) - bounds.1) as f64 * scale).round() as u32;
        // Round shared edges once; independently rounding widths can make
        // adjacent fractional-scale displays overlap by a pixel.
        let right = dimension(i64::from(c.logical.0) + i64::from(c.logical.2) - bounds.0)?;
        let bottom = dimension(i64::from(c.logical.1) + i64::from(c.logical.3) - bounds.1)?;
        let (w, h) = (right - x, bottom - y);
        let source = c.to_image();
        if (w, h) == source.dimensions() {
            image::imageops::replace(&mut image, &source, i64::from(x), i64::from(y));
        } else {
            let scaled = image::imageops::resize(&source, w, h, image::imageops::FilterType::Triangle);
            image::imageops::replace(&mut image, &scaled, i64::from(x), i64::from(y));
        }
    }
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(logical: (i32, i32, i32, i32), scale: u32, color: [u8; 4]) -> Capture {
        let (width, height) = (logical.2 as u32 * scale, logical.3 as u32 * scale);
        Capture { output: "fixture".into(), logical, width, height, rgba: image::RgbaImage::from_pixel(width, height, image::Rgba(color)).into_raw() }
    }

    #[test]
    fn mixed_scale_negative_origins_and_layout_gaps_are_preserved() {
        let left = capture((-2, 0, 2, 2), 1, [255, 0, 0, 255]);
        let right = capture((1, -1, 2, 2), 2, [0, 255, 0, 255]);
        let image = composite(&[left, right]).unwrap();
        assert_eq!(image.dimensions(), (10, 6));
        assert_eq!(image.get_pixel(0, 2).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(6, 0).0, [0, 255, 0, 255]);
        assert_eq!(image.get_pixel(4, 2).0, [0, 0, 0, 0]);
    }

    #[test]
    fn malformed_frames_and_unbounded_layouts_are_rejected() {
        assert!(composite(&[]).is_err());
        let mut short = capture((0, 0, 1, 1), 1, [0; 4]);
        short.rgba.clear();
        assert!(composite(&[short]).is_err());
        assert!(composite(&[capture((i32::MIN, 0, 1, 1), 1, [0; 4]), capture((i32::MAX, 0, 1, 1), 1, [0; 4])]).is_err());
    }

    #[test]
    fn fractional_edges_do_not_depend_on_capture_order() {
        let left = capture((0, 0, 1, 2), 1, [255, 0, 0, 255]);
        let middle = capture((1, 0, 1, 2), 1, [0, 255, 0, 255]);
        let right = Capture { output: "fractional".into(), logical: (2, 0, 2, 2), width: 3, height: 3, rgba: image::RgbaImage::from_pixel(3, 3, image::Rgba([0, 0, 255, 255])).into_raw() };
        let forward = composite(&[left.clone(), middle.clone(), right.clone()]).unwrap();
        let reverse = composite(&[right, middle, left]).unwrap();
        assert_eq!(forward, reverse);
        assert_eq!(forward.dimensions(), (6, 3));
        assert_eq!(forward.get_pixel(2, 0).0, [0, 255, 0, 255]);
        assert_eq!(forward.get_pixel(3, 0).0, [0, 0, 255, 255]);
    }
}
