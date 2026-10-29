//! Screenshot output: crop, PNG-encode, save to `~/Pictures/Screenshots`
//! and/or put on the clipboard — all in-process Rust (capture comes from
//! [`crate::screencopy`], clipboard from `wl-clipboard-rs`).

use std::io::Cursor;
use std::path::PathBuf;

use mandara_i18n::tf;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CaptureResult {
    pub path: Option<PathBuf>,
    pub copied: bool,
    pub width: u32,
    pub height: u32,
}

/// `~/Pictures/Screenshots` (honours `XDG_PICTURES_DIR` via `dirs`).
pub fn screenshot_dir() -> PathBuf {
    dirs::picture_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("Pictures")).join("Screenshots")
}

pub fn encode_png(img: &image::RgbaImage) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).map_err(|e| tf("Encoding PNG failed: {error}", &[("error", &e)]))?;
    Ok(out)
}

/// Offers `png` on the Wayland clipboard. The selection is served from a
/// background thread until another client takes ownership.
pub fn copy_png(png: Vec<u8>) -> Result<(), String> {
    crate::clipboard::serve(png, Some("image/png".into()))
}

/// `dir/stem.ext`, or `dir/stem-2.ext`, `-3`… if taken (never overwrites).
pub fn unique_path(dir: &std::path::Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..).map(|n| dir.join(format!("{stem}-{n}.{ext}"))).find(|p| !p.exists()).unwrap_or(first)
}

/// Saves and/or copies an image with the default options.
pub fn deliver(img: &image::RgbaImage, save: bool, copy: bool) -> Result<CaptureResult, String> {
    deliver_with(img, &mandara_config::workflow::ScreenshotSettings::default(), save, copy)
}

/// The folder screenshots are saved to: the setting (`~` allowed), else
/// `~/Pictures/Screenshots`.
pub fn output_dir(setting: &str) -> PathBuf {
    let s = setting.trim();
    if s.is_empty() { screenshot_dir() } else { mandara_config::paths::expand_tilde(s) }
}

/// A file name from a `strftime` pattern; invalid patterns fall back to
/// the default one, and path separators are replaced.
pub fn file_stem(pattern: &str, now: chrono::DateTime<chrono::Local>) -> String {
    use chrono::format::{Item, StrftimeItems};
    let valid = |p: &str| !p.trim().is_empty() && !StrftimeItems::new(p).any(|i| matches!(i, Item::Error));
    let pattern = if valid(pattern) { pattern } else { mandara_config::workflow::defaults::SCREENSHOT_FILENAME };
    let stem = now.format(pattern).to_string().replace(['/', '\\', '\0'], "-");
    let stem = stem.trim().trim_start_matches('.').to_string();
    if stem.is_empty() { "Screenshot".into() } else { stem }
}

/// Encodes for saving: PNG, JPEG at `quality`, or lossless WebP.
pub fn encode(img: &image::RgbaImage, format: mandara_config::workflow::ShotFormat, quality: u8) -> Result<Vec<u8>, String> {
    use mandara_config::workflow::ShotFormat;
    let mut out = Vec::new();
    let failed = |e: &dyn std::fmt::Display| tf("Encoding the screenshot failed: {error}", &[("error", &e.to_string())]);
    match format {
        ShotFormat::Png => return encode_png(img),
        ShotFormat::Jpeg => {
            let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100)).encode_image(&rgb).map_err(|e| failed(&e))?;
        }
        ShotFormat::Webp => {
            image::codecs::webp::WebPEncoder::new_lossless(&mut out).encode(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8).map_err(|e| failed(&e))?;
        }
    }
    Ok(out)
}

/// Feeds `png` to `command`'s stdin (e.g. `satty -f -`), detached.
pub fn pipe_to(command: &str, png: Vec<u8>) -> Result<(), String> {
    use std::io::Write as _;
    use std::os::unix::process::CommandExt as _;
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("{command}: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    std::thread::spawn(move || {
        let _ = stdin.write_all(&png);
        drop(stdin);
        let _ = child.wait();
    });
    Ok(())
}

/// Saves (folder, name pattern, format), copies (PNG) and / or pipes an
/// image per the screenshot settings.
pub fn deliver_with(img: &image::RgbaImage, opts: &mandara_config::workflow::ScreenshotSettings, save: bool, copy: bool) -> Result<CaptureResult, String> {
    let png = encode_png(img)?;
    let mut result = CaptureResult { path: None, copied: false, width: img.width(), height: img.height() };
    if save {
        let dir = output_dir(&opts.directory);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = unique_path(&dir, &file_stem(&opts.filename, chrono::Local::now()), opts.format.extension());
        let bytes = if opts.format == mandara_config::workflow::ShotFormat::Png { png.clone() } else { encode(img, opts.format, opts.quality)? };
        std::fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        result.path = Some(path);
    }
    if !opts.pipe_command.trim().is_empty()
        && let Err(err) = pipe_to(opts.pipe_command.trim(), png.clone())
    {
        tracing::warn!("screenshot pipe: {err}");
    }
    if copy {
        copy_png(png)?;
        result.copied = true;
    }
    Ok(result)
}

/// Human summary of a delivery.
pub fn describe(r: &CaptureResult) -> String {
    match (&r.path, r.copied) {
        (Some(p), true) => tf("{width}×{height} saved to {path} and copied", &[("width", &r.width), ("height", &r.height), ("path", &p.display())]),
        (Some(p), false) => tf("{width}×{height} saved to {path}", &[("width", &r.width), ("height", &r.height), ("path", &p.display())]),
        (None, true) => tf("{width}×{height} copied to clipboard", &[("width", &r.width), ("height", &r.height)]),
        (None, false) => tf("{width}×{height} captured", &[("width", &r.width), ("height", &r.height)]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_png_and_describes() {
        let img = image::RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 255]));
        let png = encode_png(&img).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        let r = CaptureResult { path: None, copied: true, width: 3, height: 2 };
        assert_eq!(describe(&r), "3×2 copied to clipboard");
        assert!(screenshot_dir().ends_with("Screenshots"));
    }

    #[test]
    fn names_formats_and_folders() {
        use chrono::TimeZone as _;
        let now = chrono::Local.with_ymd_and_hms(2026, 10, 27, 9, 5, 7).unwrap();
        assert_eq!(file_stem("Shot_%Y-%m-%d_%H%M%S", now), "Shot_2026-10-27_090507");
        assert_eq!(file_stem("%Y/%m", now), "2026-10");
        assert_eq!(file_stem("bad %Q pattern", now), "Screenshot_2026-10-27_09-05-07");
        assert_eq!(file_stem("", now), "Screenshot_2026-10-27_09-05-07");
        assert!(output_dir("").ends_with("Screenshots"));
        assert!(output_dir("~/Shots").ends_with("Shots") && !output_dir("~/Shots").starts_with("~"));
        let img = image::RgbaImage::from_pixel(4, 3, image::Rgba([10, 20, 30, 255]));
        use mandara_config::workflow::ShotFormat;
        assert_eq!(&encode(&img, ShotFormat::Jpeg, 80).unwrap()[..2], &[0xFF, 0xD8]);
        assert_eq!(&encode(&img, ShotFormat::Webp, 80).unwrap()[8..12], b"WEBP");
        assert_eq!(&encode(&img, ShotFormat::Png, 80).unwrap()[1..4], b"PNG");
    }
}

/// A shape drawn over a screenshot, in image pixel coordinates.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Annotation {
    pub kind: AnnotationKind,
    pub from: (f32, f32),
    pub to: (f32, f32),
    /// Straight (non-premultiplied) RGBA.
    pub color: [u8; 4],
    pub width: f32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AnnotationKind {
    Arrow,
    Rect,
    Ellipse,
}

/// The arrowhead's two barb end points for an arrow `from → to`.
pub fn arrow_barbs(from: (f32, f32), to: (f32, f32), width: f32) -> [(f32, f32); 2] {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len = (dx * dx + dy * dy).sqrt().max(1e-3);
    let (ux, uy) = (dx / len, dy / len);
    let head = (width * 4.5).max(12.0).min(len * 0.6);
    let spread = 0.5f32; // ~29°
    let (c, s) = (spread.cos(), spread.sin());
    let back = |sign: f32| (to.0 - head * (ux * c - sign * uy * s), to.1 - head * (uy * c + sign * ux * s));
    [back(1.0), back(-1.0)]
}

/// Rasterizes `shapes` onto `img` (anti-aliased, round caps and joins).
pub fn annotate(img: &mut image::RgbaImage, shapes: &[Annotation]) {
    use tiny_skia::{LineCap, LineJoin, Paint, PathBuilder, PixmapMut, Rect, Stroke, Transform};
    if shapes.is_empty() {
        return;
    }
    let (w, h) = img.dimensions();
    // Opaque screenshots: straight RGBA equals premultiplied.
    let Some(mut pixmap) = PixmapMut::from_bytes(img.as_mut(), w, h) else { return };
    for s in shapes {
        let mut pb = PathBuilder::new();
        let (x0, y0, x1, y1) = (s.from.0.min(s.to.0), s.from.1.min(s.to.1), s.from.0.max(s.to.0), s.from.1.max(s.to.1));
        match s.kind {
            AnnotationKind::Arrow => {
                pb.move_to(s.from.0, s.from.1);
                pb.line_to(s.to.0, s.to.1);
                let [a, b] = arrow_barbs(s.from, s.to, s.width);
                pb.move_to(a.0, a.1);
                pb.line_to(s.to.0, s.to.1);
                pb.line_to(b.0, b.1);
            }
            AnnotationKind::Rect => {
                if let Some(r) = Rect::from_ltrb(x0, y0, x1.max(x0 + 1.0), y1.max(y0 + 1.0)) {
                    pb.push_rect(r);
                }
            }
            AnnotationKind::Ellipse => {
                if let Some(r) = Rect::from_ltrb(x0, y0, x1.max(x0 + 1.0), y1.max(y0 + 1.0)) {
                    pb.push_oval(r);
                }
            }
        }
        let Some(path) = pb.finish() else { continue };
        let mut paint = Paint::default();
        let [r, g, b, a] = s.color;
        paint.set_color_rgba8(r, g, b, a);
        paint.anti_alias = true;
        let stroke = Stroke { width: s.width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

#[cfg(test)]
mod annotate_tests {
    use super::*;

    #[test]
    fn draws_each_shape_kind() {
        for kind in [AnnotationKind::Arrow, AnnotationKind::Rect, AnnotationKind::Ellipse] {
            let mut img = image::RgbaImage::from_pixel(100, 100, image::Rgba([0, 0, 0, 255]));
            annotate(&mut img, &[Annotation { kind, from: (20.0, 20.0), to: (80.0, 80.0), color: [255, 0, 0, 255], width: 4.0 }]);
            let red = img.pixels().filter(|p| p.0[0] > 200 && p.0[1] < 60).count();
            assert!(red > 50, "{kind:?} painted {red} red pixels");
            assert!(img.pixels().all(|p| p.0[3] == 255), "stays opaque");
        }
    }

    #[test]
    fn arrow_barbs_point_back() {
        let [a, b] = arrow_barbs((0.0, 0.0), (100.0, 0.0), 4.0);
        assert!(a.0 < 100.0 && b.0 < 100.0);
        assert!((a.1 + b.1).abs() < 1e-3, "symmetric");
    }
}
