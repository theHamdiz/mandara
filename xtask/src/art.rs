//! `cargo xtask art`: Mandara's artwork pipeline, in Rust.
//!
//! - `art` builds the artwork embedded in the binaries. For every built-in
//!   plugin in `assets/plugins/<id>/` it writes
//!   `crates/mandara-ui/assets/plugin-art/<id>/` with `icon.png` (128 px),
//!   `icon@2x.png` (256 px) and `cover.jpg` (960×540; `cover.png` when the
//!   source has transparency), plus `index.rs`, the table
//!   `mandara_ui::plugin_art` embeds. Example plugins under
//!   `plugins/examples/<id>` ship their own art in their package and are
//!   skipped. The generated files are committed: they are build inputs.
//! - `art --compose [--only ID] [--out DIR]` re-composes the 1600×900
//!   marketplace covers from `assets/catalog.json`: generated background,
//!   motif linework, the icon, an optional real panel capture and the title.
//!   Without `--out`, it also exports the 256 px icon and refreshes the
//!   asset inventory.
//! - `art --check` validates the brand marks, plugin icons, covers, the
//!   catalog, the asset index and the embedded set (also run by
//!   `cargo test -p xtask`).

use std::collections::{BTreeMap, BTreeSet};
use std::f32::consts::PI;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use ab_glyph::{Font as _, FontRef, PxScale, ScaleFont as _};
use image::{GenericImageView as _, ImageEncoder as _, Rgba, RgbaImage, imageops};
use resvg::{tiny_skia, usvg};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{Result, flag, opt, root};

// ── Embedded art ──────────────────────────────────────────────────────────

/// Marketplace cover size (the source art).
pub const COVER_SIZE: (u32, u32) = (1600, 900);
/// Embedded cover size: sharp in Studio's detail modal, a fraction of the bytes.
const EMBED_COVER_SIZE: (u32, u32) = (960, 540);
/// JPEG quality of embedded covers: no visible banding on the glass gradients.
const COVER_JPEG_QUALITY: u8 = 82;
/// Embedded icon sizes.
const ICON_1X: u32 = 128;
const ICON_2X: u32 = 256;
/// About 100 KiB per built-in plugin's icon pair and cover, with room for a
/// few more plugins before the next artwork budget review.
const EMBED_BUDGET_BYTES: u64 = 9 * 1024 * 1024 / 2;
/// Where the embedded art goes, relative to the repository root.
const EMBED_DIR: &str = "crates/mandara-ui/assets/plugin-art";
/// The table `plugin_art.rs` includes.
const INDEX_FILE: &str = "index.rs";

// ── Cover composition ────────────────────────────────────────────────────

/// Largest accepted marketplace cover PNG.
const COVER_PNG_BUDGET_BYTES: u64 = 600_000;
const PALETTE_COLORS: usize = 256;
/// NeuQuant sampling factor (1 = every pixel, slowest and best; 10 is the
/// library's recommended balance).
const NEUQUANT_SAMPLE_FACTOR: i32 = 10;
/// Title color.
const INK: [u8; 3] = [0xFF, 0xF3, 0xDB];
/// The floating layers' drop shadow.
const SHADOW_COLOR: [u8; 3] = [0x03, 0x0B, 0x18];
const SHADOW_ALPHA: f32 = 100.0 / 255.0;
const SHADOW_OFFSET: (i64, i64) = (2, 27);
const SHADOW_BLUR_SIGMA: f32 = 24.0;
/// Trims float noise (cos 90° ≠ 0) before rounding a rotated size up.
const ROTATE_EPSILON: f32 = 1e-3;
/// Gaussian support: three sigmas each side.
const BLUR_SUPPORT_SIGMAS: f32 = 3.0;
/// Icon side next to a panel capture, and alone.
const ICON_WITH_PANEL: u32 = 264;
const ICON_ALONE: u32 = 500;
const ICON_PANEL_X: f32 = 259.0;
const ICON_PANEL_Y_ONE_LINE: f32 = 442.0;
const ICON_PANEL_Y_TWO_LINES: f32 = 475.0;
const ICON_ALONE_CENTER: (f32, f32) = (1130.0, 450.0);
/// The box a panel capture is fitted into, and its center.
const PANEL_BOX: (u32, u32) = (720, 770);
const PANEL_CENTER: (f32, f32) = (1150.0, 450.0);
/// Title placement and size (shrunk until every line fits the limit).
const TITLE_ORIGIN: (f32, f32) = (112.0, 112.0);
const TITLE_LINE_GAP: f32 = 9.0;
const TITLE_SIZE_WITH_PANEL: f32 = 72.0;
const TITLE_SIZE_ALONE: f32 = 82.0;
const TITLE_LIMIT_WITH_PANEL: f32 = 690.0;
const TITLE_LIMIT_ALONE: f32 = 740.0;
/// Title font (`MANDARA_COVER_FONT` overrides) and its face in the collection.
const TITLE_FONT: &str = "/usr/share/fonts/inter/Inter.ttc";
const TITLE_FONT_FACE: u32 = 0;

/// Eye contours leave the title clear while framing the privacy icon.
const WATCH_MOTIF: &str = r#"<path d="M450 490Q960-70 1490 490Q960 1030 450 490Z"/><path d="M530 490Q960 40 1410 490Q960 920 530 490Z"/><circle cx="980" cy="490" r="250"/><circle cx="980" cy="490" r="194"/>"#;
/// Nested shelves echo the dock's low, rounded silhouette.
const SHELF_MOTIF: &str = r#"<path d="M460 595v55q0 68 68 68h880q68 0 68-68v-55M510 560v55q0 58 58 58h800q58 0 58-58v-55M560 525v55q0 48 48 48h720q48 0 48-48v-55"/><path d="M650 430v-95q0-30 30-30h80q30 0 30 30v95M890 430V245q0-30 30-30h80q30 0 30 30v185M1130 430v-95q0-30 30-30h80q30 0 30 30v95"/>"#;
/// An open parcel, with broad flaps rather than a closed container cube.
const PACKAGE_MOTIF: &str = r#"<path d="M750 365l260 130 260-130M750 365v280l260 130 260-130V365M1010 495v280M750 365l-145 100 260 130 145-100 145 100 260-130-145-100M750 365l260-130 260 130"/><path d="M1010 115v240m-90-90 90 90 90-90"/>"#;
/// A tall hourglass differentiates app limits from circular clock motifs.
const HOURGLASS_MOTIF: &str = r#"<path d="M770 145h480M770 785h480M805 145c0 175 35 205 205 320-170 115-205 145-205 320M1215 145c0 175-35 205-205 320 170 115 205 145 205 320"/><path d="M850 195c0 130 45 170 160 250 115-80 160-120 160-250M850 735c0-130 45-170 160-250 115 80 160 120 160 250M875 685h270"/>"#;
/// Flowing contours and a single star suggest generated light and terrain.
const AURORA_MOTIF: &str = r#"<path d="M420 730C730 820 690 160 1030 320S1300 760 1570 270M420 690C710 780 710 130 1050 290S1320 720 1570 230M420 650C690 740 730 100 1070 260S1340 680 1570 190M420 610C670 700 750 70 1090 230S1360 640 1570 150"/><path d="M1370 92q0 62 62 62-62 0-62 62 0-62-62-62 62 0 62-62Z"/>"#;

// ── Checks ───────────────────────────────────────────────────────────────

const BRAND_NAMES: &[&str] = &["mandara", "mandara-studio", "mandara-login"];
const BRAND_EXPORT_SIZES: &[u32] = &[512, 256, 128];
const BRAND_VIEWBOX: &str = "0 0 1024 1024";
const BRAND_TILE_RADIUS: &str = "216";
const BRAND_EXPORT_BUDGET_BYTES: u64 = 150_000;
/// An export with fewer colors has lost its gradients.
const BRAND_MIN_COLORS: usize = 128;
/// The main mark must still read at favicon size: enough light shoreline
/// pixels survive a 16 px render (resvg anti-aliases slightly softer than
/// librsvg, which the original check used with 12).
const BRAND_SMALL_RENDER: u32 = 16;
const BRAND_SMALL_MIN_LIGHT_PIXELS: usize = 10;
const SHORELINE: &str = "M232 648c0-124 24-224 114-224s86 196 166 196 76-196 166-196 114 100 114 224";
const ICON_VIEWBOX: &str = "0 0 512 512";
const ICON_SVG_BUDGET_BYTES: u64 = 12_000;
/// resvg and librsvg anti-alias edges slightly differently: an export
/// matches its source when its mean channel difference stays below this.
const ICON_MATCH_MEAN_DIFF: f64 = 1.5;
/// Plugins whose covers float a real panel capture.
const SCREENSHOT_IDS: &[&str] = &["media", "resources", "weather", "prayer-times", "processes", "audio", "bluetooth", "notifications", "language", "pomodoro", "color-picker", "soundscapes", "world-clock", "net-speed", "authenticator", "drives", "containers", "services", "power", "updates", "dock"];
/// A cover whose channels all vary less than this is empty or flat.
const COVER_MIN_STDDEV: f64 = 20.0;
/// Motif previews render at a tenth of the cover size.
const MOTIF_PREVIEW: (u32, u32) = (160, 90);
/// Pixels that keep their original values inside a panel's rounded corners.
const PANEL_MASK_MARGIN: u32 = 2;

const SVG_NS_FORBIDDEN: &[&str] = &["text", "image", "script"];

#[derive(Clone, Debug, Deserialize)]
pub struct Record {
    pub id: String,
    pub name: String,
    pub palette: String,
    pub motif: String,
    pub angle: f32,
    pub panel: Option<String>,
    #[serde(default)]
    pub crop: Option<[u32; 4]>,
    #[serde(default)]
    pub radius: Option<f32>,
}

fn assets() -> PathBuf {
    root().join("assets")
}

pub fn catalog() -> Result<Vec<Record>> {
    let path = assets().join("catalog.json");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn load_rgba(path: &Path) -> Result<RgbaImage> {
    Ok(image::open(path).map_err(|e| format!("{}: {e}", path.display()))?.to_rgba8())
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Built-in plugins with art: every `assets/plugins/<id>` that isn't an
/// example plugin (those package their own icon and cover).
pub fn builtin_ids() -> Result<Vec<String>> {
    let dir = assets().join("plugins");
    let mut ids: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|id| !root().join("plugins/examples").join(id).exists())
        .collect();
    ids.sort();
    Ok(ids)
}

// ── Pixels ───────────────────────────────────────────────────────────────

fn unpremultiply(data: &[u8], w: u32, h: u32) -> RgbaImage {
    let mut out = RgbaImage::new(w, h);
    for (px, src) in out.pixels_mut().zip(data.as_chunks::<4>().0) {
        let a = src[3];
        *px = if a == 0 { Rgba([0, 0, 0, 0]) } else { Rgba([unmul(src[0], a), unmul(src[1], a), unmul(src[2], a), a]) };
    }
    out
}

fn unmul(c: u8, a: u8) -> u8 {
    ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8
}

fn premultiplied(img: &RgbaImage) -> RgbaImage {
    let mut out = img.clone();
    for px in out.pixels_mut() {
        let a = px[3] as u32;
        for c in 0..3 {
            px[c] = ((px[c] as u32 * a + 127) / 255) as u8;
        }
    }
    out
}

/// Renders an SVG document at `w`×`h` (stretched to fill).
pub fn render_svg(data: &[u8], w: u32, h: u32) -> Result<RgbaImage> {
    let tree = usvg::Tree::from_data(data, &usvg::Options::default()).map_err(|e| format!("svg: {e}"))?;
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or("zero-sized render")?;
    let size = tree.size();
    resvg::render(&tree, tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height()), &mut pixmap.as_mut());
    Ok(unpremultiply(pixmap.data(), w, h))
}

/// Straight-alpha source-over of `src` onto `dst` at (`x`, `y`).
fn over(dst: &mut RgbaImage, src: &RgbaImage, x: i64, y: i64) {
    let (dw, dh) = (dst.width() as i64, dst.height() as i64);
    for (sx, sy, s) in src.enumerate_pixels() {
        let (dx, dy) = (x + sx as i64, y + sy as i64);
        if s[3] == 0 || dx < 0 || dy < 0 || dx >= dw || dy >= dh {
            continue;
        }
        let d = dst.get_pixel_mut(dx as u32, dy as u32);
        let sa = s[3] as f32 / 255.0;
        let da = d[3] as f32 / 255.0;
        let oa = sa + da * (1.0 - sa);
        for c in 0..3 {
            d[c] = ((s[c] as f32 * sa + d[c] as f32 * da * (1.0 - sa)) / oa).round().clamp(0.0, 255.0) as u8;
        }
        d[3] = (oa * 255.0).round() as u8;
    }
}

/// Resizes in premultiplied space so transparent edges don't fringe.
fn resize(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let scaled = imageops::resize(&premultiplied(img), w, h, imageops::FilterType::Lanczos3);
    unpremultiply(scaled.as_raw(), w, h)
}

/// Rotates counter-clockwise by `degrees`, growing the canvas to fit
/// (bilinear, premultiplied).
fn rotate_expand(img: &RgbaImage, degrees: f32) -> RgbaImage {
    if degrees == 0.0 {
        return img.clone();
    }
    let (sin, cos) = degrees.to_radians().sin_cos();
    let (w, h) = (img.width() as f32, img.height() as f32);
    let ow = (w * cos.abs() + h * sin.abs() - ROTATE_EPSILON).ceil() as u32;
    let oh = (w * sin.abs() + h * cos.abs() - ROTATE_EPSILON).ceil() as u32;
    let src = premultiplied(img);
    let sample = |x: i64, y: i64| -> [f32; 4] {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            return [0.0; 4];
        }
        let p = src.get_pixel(x as u32, y as u32);
        [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
    };
    let mut out = vec![0u8; (ow * oh * 4) as usize];
    for oy in 0..oh {
        for ox in 0..ow {
            let u = ox as f32 + 0.5 - ow as f32 / 2.0;
            let v = oy as f32 + 0.5 - oh as f32 / 2.0;
            // Inverse of a visual counter-clockwise turn in y-down space.
            let sx = u * cos - v * sin + w / 2.0 - 0.5;
            let sy = u * sin + v * cos + h / 2.0 - 0.5;
            let (x0, y0) = (sx.floor(), sy.floor());
            let (fx, fy) = (sx - x0, sy - y0);
            let (x0, y0) = (x0 as i64, y0 as i64);
            let (a, b, c, d) = (sample(x0, y0), sample(x0 + 1, y0), sample(x0, y0 + 1), sample(x0 + 1, y0 + 1));
            let i = ((oy * ow + ox) * 4) as usize;
            for k in 0..4 {
                let top = a[k] + (b[k] - a[k]) * fx;
                let bottom = c[k] + (d[k] - c[k]) * fx;
                out[i + k] = (top + (bottom - top) * fy).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    unpremultiply(&out, ow, oh)
}

/// Rounded-rectangle coverage of the pixel at (`x`, `y`) in a `w`×`h` box.
fn rounded_coverage(x: u32, y: u32, w: u32, h: u32, r: f32) -> f32 {
    let px = (x as f32 + 0.5 - w as f32 / 2.0).abs();
    let py = (y as f32 + 0.5 - h as f32 / 2.0).abs();
    let qx = px - (w as f32 / 2.0 - r);
    let qy = py - (h as f32 / 2.0 - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    let sd = outside + qx.max(qy).min(0.0) - r;
    (0.5 - sd).clamp(0.0, 1.0)
}

/// A panel capture cropped from its screenshot with rounded corners.
pub fn panel_image(record: &Record) -> Result<RgbaImage> {
    let path = record.panel.as_ref().ok_or("no panel")?;
    let [l, t, r, b] = record.crop.ok_or("panel without crop")?;
    let radius = record.radius.ok_or("panel without radius")?;
    let source = load_rgba(&root().join(path))?;
    let mut panel = source.view(l, t, r - l, b - t).to_image();
    let (w, h) = panel.dimensions();
    for (x, y, px) in panel.enumerate_pixels_mut() {
        px[3] = (px[3] as f32 * rounded_coverage(x, y, w, h, radius)).round() as u8;
    }
    Ok(panel)
}

/// Fits inside `bounds`, keeping the aspect ratio.
fn contain(img: &RgbaImage, bounds: (u32, u32)) -> RgbaImage {
    let (w, h) = img.dimensions();
    let scale = (bounds.0 as f32 / w as f32).min(bounds.1 as f32 / h as f32);
    resize(img, ((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1))
}

/// Composites `item` centered at `center`, rotated, over a soft shadow.
fn float_layer(canvas: &mut RgbaImage, item: &RgbaImage, center: (f32, f32), angle: f32) {
    let item = rotate_expand(item, angle);
    let x = (center.0 - item.width() as f32 / 2.0).round() as i64;
    let y = (center.1 - item.height() as f32 / 2.0).round() as i64;
    let pad = (SHADOW_BLUR_SIGMA * BLUR_SUPPORT_SIGMAS).ceil() as u32;
    let mut alpha = image::GrayImage::new(item.width() + pad * 2, item.height() + pad * 2);
    for (ix, iy, p) in item.enumerate_pixels() {
        alpha.put_pixel(ix + pad, iy + pad, image::Luma([(p[3] as f32 * SHADOW_ALPHA).round() as u8]));
    }
    let blurred = imageops::blur(&alpha, SHADOW_BLUR_SIGMA);
    let mut shadow = RgbaImage::new(blurred.width(), blurred.height());
    for (px, a) in shadow.pixels_mut().zip(blurred.pixels()) {
        *px = Rgba([SHADOW_COLOR[0], SHADOW_COLOR[1], SHADOW_COLOR[2], a[0]]);
    }
    over(canvas, &shadow, x + SHADOW_OFFSET.0 - pad as i64, y + SHADOW_OFFSET.1 - pad as i64);
    over(canvas, &item, x, y);
}

/// The accent the motif linework is drawn in, per palette.
fn accent(palette: &str) -> Result<&'static str> {
    Ok(match palette {
        "corniche" => "#85E1D5",
        "twilight" => "#ECC9B4",
        "obsidian" => "#B3D5B2",
        other => return Err(format!("unknown palette {other}")),
    })
}

/// The thematic linework behind a cover, as a 1600×900 SVG.
pub fn motif_svg(motif: &str, palette: &str) -> Result<String> {
    let accent = accent(palette)?;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1600\" height=\"900\" viewBox=\"0 0 1600 900\"><g fill=\"none\" stroke=\"{accent}\" stroke-width=\"3\" stroke-linecap=\"round\" stroke-linejoin=\"round\" opacity=\".32\">"
    );
    let mut push = |s: String| svg.push_str(&s);
    match motif {
        "watch" => push(WATCH_MOTIF.into()),
        "shelf" => push(SHELF_MOTIF.into()),
        "package" => push(PACKAGE_MOTIF.into()),
        "hourglass" => push(HOURGLASS_MOTIF.into()),
        "aurora" => push(AURORA_MOTIF.into()),
        "waves" | "sound" => {
            for j in 0..5 {
                let points: Vec<String> = (40..1550)
                    .step_by(5)
                    .map(|x| {
                        let xf = x as f32;
                        let y = 645.0 + j as f32 * 22.0 + (xf / 57.0 + j as f32 * 0.4).sin() * (32.0 + 56.0 * (xf / 330.0).sin().powi(2));
                        format!("{xf:.1},{y:.1}")
                    })
                    .collect();
                push(format!("<path d=\"M{}\"/>", points.join(" L")));
            }
        }
        "orbits" | "globe" | "peers" => {
            for j in 0..3 {
                push(format!("<ellipse cx=\"920\" cy=\"495\" rx=\"{}\" ry=\"{}\" transform=\"rotate({} 920 495)\"/>", 510 - j * 70, 235 - j * 29, -23 + j * 22));
            }
            if motif == "peers" {
                push("<path stroke-width=\"12\" d=\"M530 640h54m-27-27v54M1370 230h54m-27-27v54\"/>".into());
            }
        }
        "radar" | "focus" | "power" | "timer" => {
            for radius in [174, 226, 298, 384] {
                push(format!("<circle cx=\"1010\" cy=\"465\" r=\"{radius}\"/>"));
            }
            if motif == "radar" {
                push("<path d=\"M1010 465l370-195M1010 465l190 325\"/>".into());
            } else {
                for angle in (0..360).step_by(30) {
                    let a = angle as f32 * PI / 180.0;
                    push(format!("<circle cx=\"{:.3}\" cy=\"{:.3}\" r=\"4\" fill=\"{accent}\" stroke=\"none\"/>", 1010.0 + a.cos() * 415.0, 465.0 + a.sin() * 415.0));
                }
            }
        }
        "moon" | "dawn" => {
            push("<path stroke-width=\"5\" d=\"M1170 105c-262-45-459 207-338 422 83 148 260 189 410 107-183 1-319-167-280-340 24-105 103-169 208-189Z\"/>".into());
            for j in 0..44 {
                push(format!("<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{accent}\" stroke=\"none\"/>", (j * 137 + 69) % 1550 + 20, (j * 83 + 21) % 800 + 40, 1.5 + (j % 3) as f32));
            }
        }
        "bars" | "circuit" => {
            for j in 0..30 {
                let h = 35.0 + ((j as f32 * 0.73).sin() * 0.5 + 0.5) * 130.0;
                push(format!("<path stroke-width=\"10\" d=\"M{} 745v-{h:.1}\"/>", 470 + j * 33));
            }
            if motif == "circuit" {
                push("<path d=\"M420 380h110l110-110h240M1430 470h-150l-110 110H870\"/>".into());
            }
        }
        "keys" | "tiles" | "capture" | "frame" | "menu" => {
            for j in 0..4 {
                let (x, y) = (640 + j * 95, 180 + j * 92);
                push(format!("<rect x=\"{x}\" y=\"{y}\" width=\"440\" height=\"290\" rx=\"56\" transform=\"rotate(-12 {} {})\"/>", x + 220, y + 145));
            }
        }
        "paper" | "tasks" | "clipboard" => {
            for j in 0..3 {
                push(format!("<path d=\"M{} {}q180-95 350 0t350 0v340q-180 95-350 0t-350 0Z\"/>", 570 + j * 120, 270 + j * 65));
            }
        }
        "sun" | "landscape" => {
            push("<circle cx=\"1130\" cy=\"300\" r=\"150\"/>".into());
            for j in 0..5 {
                push(format!("<path d=\"M400 {}Q700 {} 1000 {}T1640 {}\"/>", 640 + j * 32, 380 + j * 50, 620 + j * 22, 565 + j * 35));
            }
        }
        "signal" => {
            for radius in [140.0f32, 240.0, 340.0, 440.0, 540.0] {
                push(format!(
                    "<path d=\"M{:.1} {:.1}Q1050 {:.1} {:.1} {:.1}\"/>",
                    1050.0 - radius * 0.8,
                    675.0 - radius * 0.6,
                    675.0 - radius * 1.7,
                    1050.0 + radius * 0.8,
                    675.0 - radius * 0.6
                ));
            }
        }
        "bell" => {
            for j in 0..4 {
                push(format!("<path d=\"M{} {}C{} {} {} {} {} {}\"/>", 700 - j * 55, 585 + j * 25, 760 - j * 55, 160 - j * 20, 1220 + j * 55, 160 - j * 20, 1280 + j * 55, 585 + j * 25));
            }
        }
        other => return Err(format!("unknown cover motif {other}")),
    }
    svg.push_str("</g></svg>");
    Ok(svg)
}

/// Cover title lines: long names break where they read best.
fn title_lines(name: &str) -> Vec<&str> {
    match name {
        "Active window" => vec!["Active", "window"],
        "Keyboard layout" => vec!["Keyboard", "layout"],
        "Process hunter" => vec!["Process", "hunter"],
        "Quick settings" => vec!["Quick", "settings"],
        "App installer" => vec!["App", "installer"],
        "Wallpaper generator" => vec!["Wallpaper", "generator"],
        other => vec![other],
    }
}

/// Horizontal advance of `line` (with kerning).
fn line_width(font: &FontRef, scale: PxScale, line: &str) -> f32 {
    let scaled = font.as_scaled(scale);
    let mut width = 0.0;
    let mut prev = None;
    for ch in line.chars() {
        let id = font.glyph_id(ch);
        if let Some(p) = prev {
            width += scaled.kern(p, id);
        }
        width += scaled.h_advance(id);
        prev = Some(id);
    }
    width
}

/// `size` px per em → ab_glyph's scale (which is the ascent-to-descent height).
fn em_scale(font: &FontRef, size: f32) -> PxScale {
    let units = font.units_per_em().unwrap_or(1000.0);
    PxScale::from(size * font.height_unscaled() / units)
}

fn draw_title(canvas: &mut RgbaImage, name: &str, has_panel: bool, font: &FontRef) {
    let lines = title_lines(name);
    let (mut size, limit) = if has_panel { (TITLE_SIZE_WITH_PANEL, TITLE_LIMIT_WITH_PANEL) } else { (TITLE_SIZE_ALONE, TITLE_LIMIT_ALONE) };
    while lines.iter().map(|l| line_width(font, em_scale(font, size), l)).fold(0.0, f32::max) > limit {
        size -= 1.0;
    }
    let scale = em_scale(font, size);
    let scaled = font.as_scaled(scale);
    let (w, h) = (canvas.width() as i32, canvas.height() as i32);
    for (i, line) in lines.iter().enumerate() {
        // Top of the line at the ascender (Pillow's default anchor).
        let baseline = TITLE_ORIGIN.1 + i as f32 * (size + TITLE_LINE_GAP) + scaled.ascent();
        let mut x = TITLE_ORIGIN.0;
        let mut prev = None;
        for ch in line.chars() {
            let id = font.glyph_id(ch);
            if let Some(p) = prev {
                x += scaled.kern(p, id);
            }
            let glyph = id.with_scale_and_position(scale, ab_glyph::point(x, baseline));
            if let Some(outline) = font.outline_glyph(glyph) {
                let b = outline.px_bounds();
                outline.draw(|gx, gy, coverage| {
                    let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                    if px < 0 || py < 0 || px >= w || py >= h {
                        return;
                    }
                    let d = canvas.get_pixel_mut(px as u32, py as u32);
                    let a = coverage.clamp(0.0, 1.0);
                    for c in 0..3 {
                        d[c] = (INK[c] as f32 * a + d[c] as f32 * (1.0 - a)).round() as u8;
                    }
                });
            }
            x += scaled.h_advance(id);
            prev = Some(id);
        }
    }
}

/// Saves an opaque cover as an indexed PNG: its own palette when it has at
/// most 256 colors, otherwise NeuQuant with Floyd–Steinberg dithering.
pub fn save_cover_png(img: &RgbaImage, path: &Path) -> Result<()> {
    let (w, h) = img.dimensions();
    let mut unique: BTreeMap<[u8; 3], u8> = BTreeMap::new();
    for p in img.pixels() {
        let key = [p[0], p[1], p[2]];
        if !unique.contains_key(&key) {
            if unique.len() == PALETTE_COLORS {
                unique.clear();
                break;
            }
            let next = unique.len() as u8;
            unique.insert(key, next);
        }
    }
    let (palette, indices): (Vec<u8>, Vec<u8>) = if !unique.is_empty() {
        let mut palette = vec![0u8; unique.len() * 3];
        for (rgb, i) in &unique {
            palette[*i as usize * 3..*i as usize * 3 + 3].copy_from_slice(rgb);
        }
        (palette, img.pixels().map(|p| unique[&[p[0], p[1], p[2]]]).collect())
    } else {
        let rgba: Vec<u8> = img.pixels().flat_map(|p| [p[0], p[1], p[2], 255]).collect();
        let nq = color_quant::NeuQuant::new(NEUQUANT_SAMPLE_FACTOR, PALETTE_COLORS, &rgba);
        let palette = nq.color_map_rgb();
        // Floyd–Steinberg: carry each pixel's error to its neighbours.
        let mut work: Vec<[f32; 3]> = img.pixels().map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect();
        let mut indices = vec![0u8; (w * h) as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let i = y * w as usize + x;
                let c = work[i].map(|v| v.round().clamp(0.0, 255.0) as u8);
                let idx = nq.index_of(&[c[0], c[1], c[2], 255]);
                indices[i] = idx as u8;
                let err: [f32; 3] = std::array::from_fn(|k| work[i][k] - palette[idx * 3 + k] as f32);
                let mut spread = |dx: isize, dy: usize, f: f32| {
                    let nx = x as isize + dx;
                    if nx < 0 || nx >= w as isize || y + dy >= h as usize {
                        return;
                    }
                    let j = (y + dy) * w as usize + nx as usize;
                    for k in 0..3 {
                        work[j][k] += err[k] * f;
                    }
                };
                spread(1, 0, 7.0 / 16.0);
                spread(-1, 1, 3.0 / 16.0);
                spread(0, 1, 5.0 / 16.0);
                spread(1, 1, 1.0 / 16.0);
            }
        }
        (palette, indices)
    };
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    encoder.set_color(png::ColorType::Indexed);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_palette(palette);
    encoder.set_compression(png::Compression::High);
    encoder.set_filter(png::Filter::NoFilter);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&indices).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())
}

/// Composes one marketplace cover.
pub fn compose(record: &Record, font: &FontRef) -> Result<RgbaImage> {
    let mut canvas = load_rgba(&assets().join("artwork").join(format!("{}.png", record.palette)))?;
    if canvas.dimensions() != COVER_SIZE {
        return Err(format!("artwork {} is not {}×{}", record.palette, COVER_SIZE.0, COVER_SIZE.1));
    }
    let motif = render_svg(motif_svg(&record.motif, &record.palette)?.as_bytes(), COVER_SIZE.0, COVER_SIZE.1)?;
    over(&mut canvas, &motif, 0, 0);
    let has_panel = record.panel.is_some();
    let icon_size = if has_panel { ICON_WITH_PANEL } else { ICON_ALONE };
    let svg_path = assets().join("plugins").join(&record.id).join("icon.svg");
    let svg = std::fs::read(&svg_path).map_err(|e| format!("{}: {e}", svg_path.display()))?;
    let icon = render_svg(&svg, icon_size, icon_size)?;
    if has_panel {
        let y = if title_lines(&record.name).len() == 2 { ICON_PANEL_Y_TWO_LINES } else { ICON_PANEL_Y_ONE_LINE };
        float_layer(&mut canvas, &icon, (ICON_PANEL_X, y), 0.0);
        let panel = contain(&panel_image(record)?, PANEL_BOX);
        float_layer(&mut canvas, &panel, PANEL_CENTER, record.angle);
    } else {
        float_layer(&mut canvas, &icon, ICON_ALONE_CENTER, record.angle);
    }
    draw_title(&mut canvas, &record.name, has_panel, font);
    Ok(canvas)
}

fn compose_all(args: &[String]) -> Result<()> {
    let font_path = std::env::var("MANDARA_COVER_FONT").unwrap_or_else(|_| TITLE_FONT.into());
    let font_data = std::fs::read(&font_path).map_err(|e| format!("{font_path}: {e} (set MANDARA_COVER_FONT to an Inter font)"))?;
    let font = FontRef::try_from_slice_and_index(&font_data, TITLE_FONT_FACE).map_err(|e| format!("{font_path}: {e}"))?;
    let only = opt(args, "--only");
    let out = opt(args, "--out").map(PathBuf::from);
    for record in catalog()?.iter().filter(|r| only.as_deref().is_none_or(|o| o == r.id)) {
        let cover = compose(record, &font)?;
        let target = match &out {
            Some(dir) => {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                dir.join(format!("{}.png", record.id))
            }
            None => assets().join("plugins").join(&record.id).join("cover.png"),
        };
        save_cover_png(&cover, &target)?;
        let size = std::fs::metadata(&target).map_err(|e| e.to_string())?.len();
        if size > COVER_PNG_BUDGET_BYTES {
            return Err(format!("{} exceeds the marketplace budget ({size} > {COVER_PNG_BUDGET_BYTES} bytes)", target.display()));
        }
        if out.is_none() {
            let dir = assets().join("plugins").join(&record.id);
            export_icon(&dir.join("icon.svg"), &dir.join("icon.png"))?;
        }
        println!("{}: {} KB", record.id, size / 1000);
    }
    if out.is_none() {
        write_inventory(&assets(), &root())?;
    }
    Ok(())
}

fn export_icon(source: &Path, target: &Path) -> Result<()> {
    let svg = std::fs::read(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let image = render_svg(&svg, ICON_2X, ICON_2X)?;
    write(target, &encode_png(&image)?)?;
    Ok(())
}

// ── Embedding ────────────────────────────────────────────────────────────

fn encode_png(img: &RgbaImage) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(&mut out, image::codecs::png::CompressionType::Best, image::codecs::png::FilterType::Adaptive)
        .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

fn encode_jpeg(img: &RgbaImage) -> Result<Vec<u8>> {
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, COVER_JPEG_QUALITY)
        .write_image(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

fn write(path: &Path, bytes: &[u8]) -> Result<u64> {
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(bytes.len() as u64)
}

/// One plugin's embedded files; returns (bytes written, cover file name).
fn embed_one(id: &str, out: &Path) -> Result<(u64, &'static str)> {
    let src = assets().join("plugins").join(id);
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let mut total = 0;
    let icon_png = src.join("icon.png");
    let icon = load_rgba(&icon_png)?;
    // 2×: the 256 px export as shipped (already optimized), else resized.
    total += if icon.dimensions() == (ICON_2X, ICON_2X) {
        let bytes = std::fs::read(&icon_png).map_err(|e| e.to_string())?;
        write(&out.join("icon@2x.png"), &bytes)?
    } else {
        write(&out.join("icon@2x.png"), &encode_png(&resize(&icon, ICON_2X, ICON_2X))?)?
    };
    // 1×: rendered from the vector source when there is one (crisper than a
    // downscale), else resized.
    let small = match std::fs::read(src.join("icon.svg")) {
        Ok(svg) => render_svg(&svg, ICON_1X, ICON_1X)?,
        Err(_) => resize(&icon, ICON_1X, ICON_1X),
    };
    total += write(&out.join("icon.png"), &encode_png(&small)?)?;
    let cover = load_rgba(&src.join("cover.png"))?;
    let cover = resize(&cover, EMBED_COVER_SIZE.0, EMBED_COVER_SIZE.1);
    let opaque = cover.pixels().all(|p| p[3] == u8::MAX);
    let (name, stale) = if opaque { ("cover.jpg", "cover.png") } else { ("cover.png", "cover.jpg") };
    let _ = std::fs::remove_file(out.join(stale));
    total += write(&out.join(name), &if opaque { encode_jpeg(&cover)? } else { encode_png(&cover)? })?;
    Ok((total, name))
}

fn embed_all() -> Result<()> {
    let out_root = root().join(EMBED_DIR);
    std::fs::create_dir_all(&out_root).map_err(|e| e.to_string())?;
    let ids = builtin_ids()?;
    // Drop art of plugins that no longer exist.
    for e in std::fs::read_dir(&out_root).map_err(|e| e.to_string())?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if e.path().is_dir() && !ids.contains(&name) {
            std::fs::remove_dir_all(e.path()).map_err(|e| e.to_string())?;
        }
    }
    let mut index = String::from(
        "// @generated by `cargo xtask art` from assets/plugins/<id>/ — do not edit.\n\
         // (id, 1× icon, 2× icon, cover, cover format)\n&[\n",
    );
    let mut total = 0;
    for id in &ids {
        let (bytes, cover) = embed_one(id, &out_root.join(id))?;
        total += bytes;
        let format = if cover.ends_with(".jpg") { "Jpeg" } else { "Png" };
        let path = |file: &str| format!("include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/assets/plugin-art/{id}/{file}\"))");
        let _ = writeln!(index, "    (\"{id}\", {}, {}, {}, gpui::ImageFormat::{format}),", path("icon.png"), path("icon@2x.png"), path(cover));
        println!("{id:>16}: {:>4} KB", bytes / 1024);
    }
    index.push_str("]\n");
    write(&out_root.join(INDEX_FILE), index.as_bytes())?;
    write_inventory(&assets(), &root())?;
    println!("{} plugins, {} KB embedded (budget {} KB)", ids.len(), total / 1024, EMBED_BUDGET_BYTES / 1024);
    if total > EMBED_BUDGET_BYTES {
        return Err(format!("embedded art is {total} bytes, over the {EMBED_BUDGET_BYTES}-byte budget"));
    }
    Ok(())
}

// ── Checks ───────────────────────────────────────────────────────────────

/// Collects failures instead of stopping at the first.
#[derive(Default)]
pub struct Report(pub Vec<String>);

impl Report {
    fn ensure(&mut self, ok: bool, msg: impl FnOnce() -> String) {
        if !ok {
            self.0.push(msg());
        }
    }

    fn try_or<T>(&mut self, r: Result<T>) -> Option<T> {
        r.map_err(|e| self.0.push(e)).ok()
    }
}

fn parse_svg(path: &Path) -> Result<String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    roxmltree::Document::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(text)
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX)
}

fn is_rgba8_png(path: &Path) -> bool {
    image::ImageReader::open(path).ok().and_then(|r| r.with_guessed_format().ok()).and_then(|r| r.decode().ok()).is_some_and(|i| i.color() == image::ColorType::Rgba8)
}

pub fn check_brand(r: &mut Report) {
    let brand = assets().join("brand");
    for name in BRAND_NAMES {
        let path = brand.join(format!("{name}.svg"));
        if let Some(text) = r.try_or(parse_svg(&path)) {
            let doc = roxmltree::Document::parse(&text).expect("parsed above");
            let root = doc.root_element();
            r.ensure(root.attribute("viewBox") == Some(BRAND_VIEWBOX), || format!("{name}.svg: viewBox must be {BRAND_VIEWBOX}"));
            r.ensure(!doc.descendants().any(|n| SVG_NS_FORBIDDEN.contains(&n.tag_name().name())), || format!("{name}.svg: no text, image or script elements"));
            let shoreline = doc.descendants().find(|n| n.tag_name().name() == "path" && n.attribute("id") == Some("shoreline"));
            r.ensure(shoreline.and_then(|n| n.attribute("d")) == Some(SHORELINE), || format!("{name}.svg: the shared shoreline path changed"));
            let tile = doc.descendants().find(|n| n.tag_name().name() == "rect" && n.attribute("id") == Some("tile-shape"));
            r.ensure(tile.and_then(|n| n.attribute("rx")) == Some(BRAND_TILE_RADIUS), || format!("{name}.svg: tile radius must be {BRAND_TILE_RADIUS}"));
        }
        for &size in BRAND_EXPORT_SIZES {
            let path = brand.join(format!("{name}-{size}.png"));
            let Some(img) = r.try_or(load_rgba(&path)) else { continue };
            r.ensure(img.dimensions() == (size, size), || format!("{}: must be {size}×{size}", path.display()));
            r.ensure(is_rgba8_png(&path), || format!("{}: must be RGBA", path.display()));
            r.ensure(img.get_pixel(0, 0)[3] == 0, || format!("{}: corners must be transparent", path.display()));
            r.ensure(img.get_pixel(size / 2, size / 2)[3] == u8::MAX, || format!("{}: center must be opaque", path.display()));
            let colors: BTreeSet<[u8; 4]> = img.pixels().map(|p| p.0).collect();
            r.ensure(colors.len() > BRAND_MIN_COLORS, || format!("{}: only {} colors", path.display(), colors.len()));
            r.ensure(file_size(&path) < BRAND_EXPORT_BUDGET_BYTES, || format!("{}: over {BRAND_EXPORT_BUDGET_BYTES} bytes", path.display()));
        }
    }
    let symbolic = brand.join("mandara-symbolic.svg");
    if let Some(text) = r.try_or(parse_svg(&symbolic)) {
        let doc = roxmltree::Document::parse(&text).expect("parsed above");
        for n in doc.descendants().filter(|n| n.is_element()) {
            for attr in ["fill", "stroke"] {
                if let Some(v) = n.attribute(attr) {
                    r.ensure(v == "none" || v == "currentColor", || format!("mandara-symbolic.svg: {attr}=\"{v}\" (only none / currentColor)"));
                }
            }
        }
        r.ensure(!doc.descendants().any(|n| matches!(n.tag_name().name(), "defs" | "text")), || "mandara-symbolic.svg: no defs or text".into());
    }
    if let Some(small) = r.try_or(std::fs::read(brand.join("mandara.svg")).map_err(|e| e.to_string()).and_then(|svg| render_svg(&svg, BRAND_SMALL_RENDER, BRAND_SMALL_RENDER))) {
        let light = small.pixels().filter(|p| p[0] > 110 && p[1] > 155 && p[3] > 180).count();
        r.ensure(light >= BRAND_SMALL_MIN_LIGHT_PIXELS, || format!("mandara.svg: the mark fades at {BRAND_SMALL_RENDER} px ({light} light pixels)"));
        r.ensure(small.get_pixel(0, 0)[3] == 0, || "mandara.svg: small render corners must be transparent".into());
    }
}

pub fn check_icons(r: &mut Report, records: &[Record]) {
    let mut hashes = BTreeSet::new();
    for rec in records {
        let dir = assets().join("plugins").join(&rec.id);
        let svg_path = dir.join("icon.svg");
        if let Some(text) = r.try_or(parse_svg(&svg_path)) {
            let doc = roxmltree::Document::parse(&text).expect("parsed above");
            r.ensure(doc.root_element().attribute("viewBox") == Some(ICON_VIEWBOX), || format!("{}: viewBox must be {ICON_VIEWBOX}", svg_path.display()));
            r.ensure(file_size(&svg_path) < ICON_SVG_BUDGET_BYTES, || format!("{}: over {ICON_SVG_BUDGET_BYTES} bytes", svg_path.display()));
            let ids: Vec<&str> = doc.descendants().filter_map(|n| n.attribute("id")).collect();
            let unique: BTreeSet<&str> = ids.iter().copied().collect();
            r.ensure(ids.len() == unique.len(), || format!("{}: duplicate ids", svg_path.display()));
            r.ensure(!doc.descendants().any(|n| SVG_NS_FORBIDDEN.contains(&n.tag_name().name())), || format!("{}: no text, image or script elements", svg_path.display()));
            let external = doc.descendants().flat_map(|n| n.attributes()).any(|a| a.name() == "href" && !a.value().starts_with('#'));
            r.ensure(!external, || format!("{}: only internal (#) references", svg_path.display()));
        }
        let png_path = dir.join("icon.png");
        let Some(export) = r.try_or(load_rgba(&png_path)) else { continue };
        r.ensure(export.dimensions() == (ICON_2X, ICON_2X), || format!("{}: must be {ICON_2X}×{ICON_2X}", png_path.display()));
        r.ensure(export.get_pixel(0, 0)[3] == 0, || format!("{}: corners must be transparent", png_path.display()));
        r.ensure(export.get_pixel(ICON_2X / 2, ICON_2X / 2)[3] == u8::MAX, || format!("{}: center must be opaque", png_path.display()));
        if let Some(rendered) = r.try_or(std::fs::read(&svg_path).map_err(|e| e.to_string()).and_then(|svg| render_svg(&svg, ICON_2X, ICON_2X))) {
            let diff = mean_diff(&export, &rendered);
            r.ensure(diff < ICON_MATCH_MEAN_DIFF, || format!("{}: differs from icon.svg (mean channel difference {diff:.2})", png_path.display()));
        }
        hashes.insert(sha256(export.as_raw()));
    }
    r.ensure(hashes.len() == records.len(), || "two plugins share an identical icon".into());
}

fn mean_diff(a: &RgbaImage, b: &RgbaImage) -> f64 {
    if a.dimensions() != b.dimensions() {
        return f64::INFINITY;
    }
    let sum: u64 = a.as_raw().iter().zip(b.as_raw()).map(|(x, y)| x.abs_diff(*y) as u64).sum();
    sum as f64 / a.as_raw().len() as f64
}

fn channel_stddev(img: &RgbaImage) -> f64 {
    let n = (img.width() * img.height()) as f64;
    (0..3)
        .map(|c| {
            let mean = img.pixels().map(|p| p[c] as f64).sum::<f64>() / n;
            (img.pixels().map(|p| (p[c] as f64 - mean).powi(2)).sum::<f64>() / n).sqrt()
        })
        .fold(0.0, f64::max)
}

pub fn check_covers(r: &mut Report, records: &[Record]) {
    let mut hashes = BTreeSet::new();
    for rec in records {
        let path = assets().join("plugins").join(&rec.id).join("cover.png");
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                r.0.push(format!("missing cover {}: {e}", path.display()));
                continue;
            }
        };
        r.ensure(image::guess_format(&bytes).ok() == Some(image::ImageFormat::Png), || format!("{}: must be a PNG", path.display()));
        r.ensure(bytes.len() as u64 <= COVER_PNG_BUDGET_BYTES, || format!("{}: over {COVER_PNG_BUDGET_BYTES} bytes", path.display()));
        r.ensure(hashes.insert(sha256(&bytes)), || format!("{}: identical to another cover", path.display()));
        let Some(cover) = r.try_or(image::load_from_memory(&bytes).map(|i| i.to_rgba8()).map_err(|e| format!("{}: {e}", path.display()))) else { continue };
        r.ensure(cover.dimensions() == COVER_SIZE, || format!("{}: must be {}×{}", path.display(), COVER_SIZE.0, COVER_SIZE.1));
        r.ensure(cover.pixels().all(|p| p[3] == u8::MAX), || format!("{}: must be opaque", path.display()));
        let stddev = channel_stddev(&cover);
        r.ensure(stddev > COVER_MIN_STDDEV, || format!("{}: looks empty or flat (stddev {stddev:.1})", path.display()));
    }
}

pub fn check_catalog(r: &mut Report, records: &[Record]) {
    let ids: BTreeSet<&str> = records.iter().map(|x| x.id.as_str()).collect();
    r.ensure(ids.len() == records.len(), || "catalog.json: duplicate ids".into());
    let dirs: BTreeSet<String> = std::fs::read_dir(assets().join("plugins")).map(|d| d.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
    r.ensure(dirs.iter().map(String::as_str).collect::<BTreeSet<_>>() == ids, || "catalog.json: ids don't match assets/plugins/".into());
    let panels: BTreeSet<&str> = records.iter().filter(|x| x.panel.is_some()).map(|x| x.id.as_str()).collect();
    r.ensure(panels == SCREENSHOT_IDS.iter().copied().collect(), || "catalog.json: the panel captures changed".into());
    for rec in records.iter().filter(|x| x.panel.is_some()) {
        let expected = format!("docs/screenshots/{}.png", rec.id);
        r.ensure(rec.panel.as_deref() == Some(expected.as_str()), || format!("{}: panel must be {expected}", rec.id));
        let (Some([left, top, right, bottom]), Some(radius)) = (rec.crop, rec.radius) else {
            r.0.push(format!("{}: panel without crop / radius", rec.id));
            continue;
        };
        let Some((w, h)) = r.try_or(image::image_dimensions(root().join(&expected)).map_err(|e| format!("{expected}: {e}"))) else { continue };
        r.ensure(right <= w && bottom <= h && right > left && bottom > top, || format!("{}: crop outside the capture", rec.id));
        r.ensure(radius > 0.0 && radius <= (right.saturating_sub(left)).min(bottom.saturating_sub(top)) as f32 / 2.0, || format!("{}: bad corner radius", rec.id));
    }
}

pub fn check_readme(r: &mut Report, records: &[Record]) {
    let readme = std::fs::read_to_string(assets().join("README.md")).unwrap_or_default();
    for rec in records {
        for file in ["icon.svg", "icon.png", "cover.png"] {
            let link = format!("plugins/{}/{file}", rec.id);
            r.ensure(readme.contains(&link), || format!("assets/README.md doesn't link {link}"));
        }
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let p = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if p.is_dir() { walk(&p, out)? } else { out.push(p) }
    }
    Ok(())
}

fn inventory(dir: &Path, base: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    walk(dir, &mut files)?;
    let mut paths = files.iter().map(|p| {
        p.strip_prefix(base).map(|p| p.to_string_lossy().to_string()).map_err(|e| format!("{}: {e}", p.display()))
    }).collect::<Result<Vec<_>>>()?;
    paths.sort();
    Ok(paths)
}

fn write_inventory(dir: &Path, base: &Path) -> Result<()> {
    let path = dir.join("FILES.txt");
    let relative = path.strip_prefix(base).map_err(|e| format!("{}: {e}", path.display()))?.to_string_lossy().to_string();
    let mut paths = inventory(dir, base)?;
    if !paths.contains(&relative) {
        paths.push(relative);
        paths.sort();
    }
    write(&path, format!("{}\n", paths.join("\n")).as_bytes())?;
    Ok(())
}

pub fn check_inventory(r: &mut Report) {
    let listed: Vec<String> = std::fs::read_to_string(assets().join("FILES.txt")).unwrap_or_default().lines().map(str::to_string).collect();
    let Some(actual) = r.try_or(inventory(&assets(), &root())) else { return };
    for missing in actual.iter().filter(|f| !listed.contains(f)) {
        r.0.push(format!("assets/FILES.txt doesn't list {missing}"));
    }
    for gone in listed.iter().filter(|f| !actual.contains(f)) {
        r.0.push(format!("assets/FILES.txt lists a missing file {gone}"));
    }
}

pub fn check_artwork(r: &mut Report, records: &[Record]) {
    let prompts: serde_json::Value = std::fs::read_to_string(assets().join("artwork/prompts.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    for palette in ["corniche", "twilight", "obsidian"] {
        r.ensure(prompts.get(palette).and_then(|p| p.as_str()).is_some_and(|p| p.contains("No text")), || format!("prompts.json: the {palette} prompt must say \"No text\""));
        let dims = image::image_dimensions(assets().join("artwork").join(format!("{palette}.png"))).ok();
        r.ensure(dims == Some(COVER_SIZE), || format!("artwork/{palette}.png must be {}×{}", COVER_SIZE.0, COVER_SIZE.1));
    }
    for rec in records {
        match motif_svg(&rec.motif, &rec.palette).and_then(|svg| render_svg(svg.as_bytes(), MOTIF_PREVIEW.0, MOTIF_PREVIEW.1)) {
            Ok(img) => r.ensure(img.pixels().any(|p| p[3] > 0), || format!("{}: motif {} renders nothing", rec.id, rec.motif)),
            Err(e) => r.0.push(format!("{}: {e}", rec.id)),
        }
    }
}

pub fn check_panel_masks(r: &mut Report, records: &[Record]) {
    for rec in records.iter().filter(|x| x.panel.is_some()) {
        let (Some(panel), Some([l, t, right, b]), Some(radius)) = (r.try_or(panel_image(rec)), rec.crop, rec.radius) else { continue };
        let Some(source) = r.try_or(load_rgba(&root().join(rec.panel.as_deref().unwrap_or_default()))) else { continue };
        let expected = source.view(l, t, right - l, b - t).to_image();
        let m = radius.ceil() as u32 + PANEL_MASK_MARGIN;
        let (w, h) = panel.dimensions();
        let interior_same = (m..h.saturating_sub(m)).all(|y| (m..w.saturating_sub(m)).all(|x| panel.get_pixel(x, y) == expected.get_pixel(x, y)));
        r.ensure(interior_same, || format!("{}: the panel mask changed pixels inside the corners", rec.id));
        r.ensure(panel.get_pixel(0, 0)[3] == 0, || format!("{}: the panel mask leaves a corner fringe", rec.id));
    }
}

/// The embedded set is complete, current and within budget.
pub fn check_embedded(r: &mut Report) {
    let Some(ids) = r.try_or(builtin_ids()) else { return };
    let dir = root().join(EMBED_DIR);
    let index = std::fs::read_to_string(dir.join(INDEX_FILE)).unwrap_or_default();
    let mut total = 0;
    for id in &ids {
        r.ensure(index.contains(&format!("(\"{id}\"")), || format!("{EMBED_DIR}/{INDEX_FILE} lacks {id} — run `cargo xtask art`"));
        for (file, side) in [("icon.png", ICON_1X), ("icon@2x.png", ICON_2X)] {
            let path = dir.join(id).join(file);
            total += file_size(&path).min(EMBED_BUDGET_BYTES);
            r.ensure(image::image_dimensions(&path).ok() == Some((side, side)), || format!("{}: must be {side}×{side} — run `cargo xtask art`", path.display()));
        }
        let cover = ["cover.jpg", "cover.png"].iter().map(|f| dir.join(id).join(f)).find(|p| p.is_file());
        match cover {
            Some(path) => {
                total += file_size(&path);
                r.ensure(image::image_dimensions(&path).ok() == Some(EMBED_COVER_SIZE), || format!("{}: must be {}×{}", path.display(), EMBED_COVER_SIZE.0, EMBED_COVER_SIZE.1));
            }
            None => r.0.push(format!("{EMBED_DIR}/{id}: no cover — run `cargo xtask art`")),
        }
    }
    r.ensure(total <= EMBED_BUDGET_BYTES, || format!("embedded art is {total} bytes, over the {EMBED_BUDGET_BYTES}-byte budget"));
}

pub fn check_all() -> Report {
    let mut r = Report::default();
    let records = r.try_or(catalog()).unwrap_or_default();
    check_brand(&mut r);
    check_icons(&mut r, &records);
    check_covers(&mut r, &records);
    check_catalog(&mut r, &records);
    check_readme(&mut r, &records);
    check_inventory(&mut r);
    check_artwork(&mut r, &records);
    check_panel_masks(&mut r, &records);
    check_embedded(&mut r);
    r
}

pub fn art(args: &[String]) -> Result<()> {
    if flag(args, "--check") {
        let report = check_all();
        if report.0.is_empty() {
            println!("artwork: all checks passed");
            return Ok(());
        }
        for failure in &report.0 {
            eprintln!("  ✗ {failure}");
        }
        return Err(format!("{} artwork check(s) failed", report.0.len()));
    }
    if flag(args, "--compose") {
        return compose_all(args);
    }
    embed_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest store icon still needs a recognizable, bright central mark.
    const STORE_ICON_SIDE: u32 = 32;
    /// Keeps the tile's specular rim outside the glyph check.
    const STORE_GLYPH_INSET: u32 = 7;
    /// Enough cream or gold remains to carry a bold outline at store size.
    const STORE_GLYPH_MIN_PIXELS: usize = 24;
    /// Lower channel bounds distinguish the warm glyph from every tile family.
    const STORE_GLYPH_MIN_RGB: [u8; 3] = [220, 180, 120];
    /// Excludes faint anti-aliasing at the transparent tile corners.
    const STORE_GLYPH_MIN_ALPHA: u8 = 220;
    /// An average channel separation of this size keeps related tiles distinct.
    const DISTINCT_TILE_MIN_CHANNEL_DIFF: u32 = 24;
    /// At most this share of the combined bright glyph pixels may overlap.
    const DISTINCT_GLYPH_MAX_OVERLAP: f64 = 0.4;
    /// New tiles use the same outer glass geometry as their siblings.
    const HOUSE_TILE_GEOMETRY: [(&str, &str); 5] = [("x", "16"), ("y", "16"), ("width", "480"), ("height", "480"), ("rx", "108")];
    const NEW_PLUGIN_IDS: &[&str] = &["privacy", "tray", "dock", "app-installer", "screen-time", "mini-stats", "wallpaper-generator"];

    fn assert_clean(check: impl FnOnce(&mut Report)) {
        let mut r = Report::default();
        check(&mut r);
        assert!(r.0.is_empty(), "{}", r.0.join("\n"));
    }

    fn records() -> Vec<Record> {
        catalog().expect("assets/catalog.json")
    }

    #[test]
    fn brand_sources_and_exports() {
        assert_clean(check_brand);
    }

    #[test]
    fn plugin_icons_match_their_sources() {
        assert_clean(|r| check_icons(r, &records()));
    }

    #[test]
    fn covers_fit_the_marketplace() {
        assert_clean(|r| check_covers(r, &records()));
    }

    #[test]
    fn catalog_preserves_panel_provenance() {
        assert_clean(|r| check_catalog(r, &records()));
    }

    #[test]
    fn index_links_every_plugin_deliverable() {
        assert_clean(|r| check_readme(r, &records()));
    }

    #[test]
    fn file_inventory_matches_the_delivery() {
        assert_clean(check_inventory);
    }

    #[test]
    fn artwork_and_motifs_render() {
        assert_clean(|r| check_artwork(r, &records()));
    }

    #[test]
    fn new_motifs_render_distinct_linework_in_every_palette() {
        for palette in ["corniche", "twilight", "obsidian"] {
            let mut hashes = BTreeSet::new();
            for motif in ["watch", "shelf", "package", "hourglass", "aurora"] {
                let svg = motif_svg(motif, palette).unwrap();
                let image = render_svg(svg.as_bytes(), MOTIF_PREVIEW.0, MOTIF_PREVIEW.1).unwrap();
                assert!(image.pixels().any(|p| p[3] > 0), "{motif}: empty in {palette}");
                assert!(hashes.insert(sha256(image.as_raw())), "{motif}: duplicate in {palette}");
            }
        }
    }

    #[test]
    fn new_icons_keep_house_geometry_and_read_at_store_size() {
        for id in NEW_PLUGIN_IDS {
            let source = std::fs::read_to_string(assets().join("plugins").join(id).join("icon.svg")).unwrap();
            let doc = roxmltree::Document::parse(&source).unwrap();
            let tile = doc.descendants().find(|n| n.tag_name().name() == "rect" && n.attribute("fill") == Some("url(#tile)")).unwrap();
            for (attribute, value) in HOUSE_TILE_GEOMETRY {
                assert_eq!(tile.attribute(attribute), Some(value), "{id}: {attribute}");
            }
            let image = render_svg(source.as_bytes(), STORE_ICON_SIDE, STORE_ICON_SIDE).unwrap();
            let bright = image.enumerate_pixels().filter(|(x, y, p)| {
                *x >= STORE_GLYPH_INSET && *x < STORE_ICON_SIDE - STORE_GLYPH_INSET
                    && *y >= STORE_GLYPH_INSET && *y < STORE_ICON_SIDE - STORE_GLYPH_INSET
                    && p[3] >= STORE_GLYPH_MIN_ALPHA
                    && p.0[..3].iter().zip(STORE_GLYPH_MIN_RGB).all(|(channel, minimum)| *channel >= minimum)
            }).count();
            assert!(bright >= STORE_GLYPH_MIN_PIXELS, "{id}: glyph fades at {STORE_ICON_SIDE} px ({bright} bright pixels)");
        }
    }

    #[test]
    fn wallpaper_generator_has_distinct_tile_and_glyph_at_store_sizes() {
        let render = |id: &str, side| {
            let svg = std::fs::read(assets().join("plugins").join(id).join("icon.svg")).unwrap();
            render_svg(&svg, side, side).unwrap()
        };
        let is_glyph = |pixel: &Rgba<u8>| {
            pixel[3] >= STORE_GLYPH_MIN_ALPHA
                && pixel.0[..3].iter().zip(STORE_GLYPH_MIN_RGB).all(|(channel, minimum)| *channel >= minimum)
        };
        for side in [STORE_ICON_SIDE, ICON_1X] {
            let generator = render("wallpaper-generator", side);
            // Halfway down the left tile edge is clear of both the rim and glyph.
            let tile_sample = (side / 8, side / 2);
            let generator_tile = generator.get_pixel(tile_sample.0, tile_sample.1);
            for sibling in ["wallpaper", "soundscapes", "media"] {
                let other = render(sibling, side);
                let other_tile = other.get_pixel(tile_sample.0, tile_sample.1);
                let tile_diff: u32 = generator_tile.0[..3].iter().zip(&other_tile.0[..3]).map(|(a, b)| u32::from(a.abs_diff(*b))).sum();
                assert!(tile_diff >= DISTINCT_TILE_MIN_CHANNEL_DIFF * 3, "wallpaper-generator and {sibling}: similar tile colors at {side} px ({tile_diff}/3 mean channel difference)");

                let mut shared = 0;
                let mut combined = 0;
                for (a, b) in generator.pixels().zip(other.pixels()) {
                    let (a, b) = (is_glyph(a), is_glyph(b));
                    shared += usize::from(a && b);
                    combined += usize::from(a || b);
                }
                assert!(combined > 0, "wallpaper-generator and {sibling}: no visible glyphs at {side} px");
                let overlap = shared as f64 / combined as f64;
                assert!(overlap <= DISTINCT_GLYPH_MAX_OVERLAP, "wallpaper-generator and {sibling}: glyph overlap {overlap:.2} at {side} px");
            }
        }
    }

    #[test]
    fn long_art_titles_wrap_without_shrinking_the_family_type() {
        assert_eq!(title_lines("Wallpaper generator"), ["Wallpaper", "generator"]);
        assert_eq!(title_lines("App installer"), ["App", "installer"]);
        assert_eq!(title_lines("System tray"), ["System tray"]);
    }

    #[test]
    fn icon_export_preserves_the_vector_render() {
        let dir = std::env::temp_dir().join(format!("mandara-art-icon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = assets().join("plugins/clock/icon.svg");
        let target = dir.join("icon.png");
        export_icon(&source, &target).unwrap();
        let expected = render_svg(&std::fs::read(source).unwrap(), ICON_2X, ICON_2X).unwrap();
        assert_eq!(load_rgba(&target).unwrap(), expected);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn inventory_refresh_is_sorted_complete_and_repeatable() {
        let base = std::env::temp_dir().join(format!("mandara-art-inventory-{}", std::process::id()));
        let dir = base.join("assets");
        std::fs::create_dir_all(dir.join("plugins/test")).unwrap();
        std::fs::write(dir.join("plugins/test/icon.svg"), "vector").unwrap();
        std::fs::write(dir.join("catalog.json"), "[]").unwrap();
        write_inventory(&dir, &base).unwrap();
        let first = std::fs::read_to_string(dir.join("FILES.txt")).unwrap();
        assert_eq!(first, "assets/FILES.txt\nassets/catalog.json\nassets/plugins/test/icon.svg\n");
        write_inventory(&dir, &base).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("FILES.txt")).unwrap(), first);
        std::fs::remove_file(dir.join("plugins/test/icon.svg")).unwrap();
        write_inventory(&dir, &base).unwrap();
        assert!(!std::fs::read_to_string(dir.join("FILES.txt")).unwrap().contains("icon.svg"));
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn panel_mask_removes_fringe_and_preserves_interior() {
        assert_clean(|r| check_panel_masks(r, &records()));
    }

    #[test]
    fn embedded_art_is_current_and_within_budget() {
        assert_clean(check_embedded);
    }

    #[test]
    fn png_writer_keeps_dimensions_and_opaque_color() {
        let dir = std::env::temp_dir().join(format!("mandara-art-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("flat.png");
        let img = RgbaImage::from_pixel(MOTIF_PREVIEW.0, MOTIF_PREVIEW.1, Rgba([0x40, 0xC8, 0xBD, 255]));
        save_cover_png(&img, &path).unwrap();
        let saved = load_rgba(&path).unwrap();
        assert_eq!(saved.dimensions(), MOTIF_PREVIEW);
        assert_eq!(saved.get_pixel(80, 45).0, [64, 200, 189, 255]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Mean channel error a dithered 256-color gradient may show.
    const DITHER_MEAN_DIFF: f64 = 8.0;

    #[test]
    fn quantized_png_stays_close_to_a_gradient() {
        let dir = std::env::temp_dir().join(format!("mandara-art-q-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gradient.png");
        let img = RgbaImage::from_fn(MOTIF_PREVIEW.0, MOTIF_PREVIEW.1, |x, y| Rgba([(x * 255 / MOTIF_PREVIEW.0) as u8, (y * 255 / MOTIF_PREVIEW.1) as u8, 128, 255]));
        save_cover_png(&img, &path).unwrap();
        let saved = load_rgba(&path).unwrap();
        // Dithering trades per-pixel error for smooth gradients.
        assert!(mean_diff(&img, &saved) < DITHER_MEAN_DIFF, "{}", mean_diff(&img, &saved));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rotation_expands_and_keeps_content() {
        let img = RgbaImage::from_pixel(40, 20, Rgba([255, 0, 0, 255]));
        let turned = rotate_expand(&img, 90.0);
        assert_eq!(turned.dimensions(), (20, 40));
        assert_eq!(turned.get_pixel(10, 20)[0], 255);
        assert_eq!(turned.get_pixel(10, 20)[3], 255);
    }

    #[test]
    fn rounded_mask_is_transparent_in_the_corner_and_solid_inside() {
        assert_eq!(rounded_coverage(0, 0, 100, 60, 20.0), 0.0);
        assert_eq!(rounded_coverage(50, 30, 100, 60, 20.0), 1.0);
        assert_eq!(rounded_coverage(50, 0, 100, 60, 20.0), 1.0);
    }

    #[test]
    fn unpremultiply_restores_color_and_clears_transparent_pixels() {
        let data = [64, 32, 16, 128, 12, 24, 36, 255, 99, 88, 77, 0];
        let image = unpremultiply(&data, 3, 1);
        assert_eq!(image.get_pixel(0, 0).0, [128, 64, 32, 128]);
        assert_eq!(image.get_pixel(1, 0).0, [12, 24, 36, 255]);
        assert_eq!(image.get_pixel(2, 0).0, [0, 0, 0, 0]);
    }
}
