//! "Use my image": your SVG or PNG (or JPEG / WebP) as the boot splash, a
//! GRUB theme and a Limine wallpaper.
//!
//! The image is checked before anything reads it: files and pixel sizes
//! are capped, and an SVG may not script, embed other documents or point
//! anywhere outside itself (only `#fragment` and `data:image/…`
//! references; the renderer is also told to load nothing from disk). It is
//! then drawn at exactly the sizes the boot screens need.
//!
//! * [`write_splash`] — a Plymouth `two-step` theme (Plymouth's own
//!   plugin draws the password prompt, so an encrypted disk still asks
//!   properly): your image as the watermark on your background colour, an
//!   optional ring or dots spinner, and a matching password field.
//! * [`write_grub`] — a GRUB theme: your image on the background at the
//!   menu's resolution, a centred menu with a pill highlight in your
//!   accent colour, GRUB's built-in font.
//! * [`write_wallpaper`] — a PNG at the Limine menu's resolution.

use std::path::Path;

use image::{Rgba, RgbaImage};
use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};
use resvg::usvg;
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

/// Largest image file accepted.
pub const MAX_FILE: u64 = 24 << 20;
/// Largest SVG accepted (text).
pub const MAX_SVG: u64 = 4 << 20;
/// Largest raster side accepted, and the largest output side.
pub const MAX_SIDE: u32 = 8192;
/// Spinner frames and their side at 1080p.
const SPINNER_FRAMES: u32 = 24;
const SPINNER_PX_1080: f32 = 44.0;
/// Password field and bullet.
const ENTRY_W_1080: f32 = 300.0;
const ENTRY_H_1080: f32 = 38.0;
const BULLET_PX_1080: f32 = 9.0;
const LOCK_PX_1080: f32 = 22.0;
/// GRUB menu metrics at 1080p.
const GRUB_ITEM_H: f32 = 40.0;
const GRUB_SELECT_PAD: f32 = 4.0;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Spinner {
    #[default]
    Ring,
    Dots,
    None,
}

impl Spinner {
    pub const ALL: [Spinner; 3] = [Spinner::Ring, Spinner::Dots, Spinner::None];

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "ring" => Some(Spinner::Ring),
            "dots" => Some(Spinner::Dots),
            "none" | "off" => Some(Spinner::None),
            _ => None,
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Spinner::Ring => "ring",
            Spinner::Dots => "dots",
            Spinner::None => "none",
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Position {
    #[default]
    Center,
    Upper,
    /// Below a centred menu (Limine draws its entries in the middle).
    Lower,
}

impl Position {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "center" | "centre" | "middle" => Some(Position::Center),
            "upper" | "top" => Some(Position::Upper),
            "lower" | "bottom" => Some(Position::Lower),
            _ => None,
        }
    }

    /// The image's vertical centre, as a share of the screen height.
    pub fn y(self) -> f32 {
        match self {
            Position::Center => 0.5,
            Position::Upper => 0.38,
            Position::Lower => 0.74,
        }
    }
}

/// How the image is laid out, and in which colours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Options {
    /// `RRGGBB`.
    pub background: [u8; 3],
    /// Spinner, password field and menu highlight.
    pub accent: [u8; 3],
    pub spinner: Spinner,
    /// The image's height as a share of the screen's (`0.1..=0.8`).
    pub scale: f32,
    pub position: Position,
    /// Fill the whole screen with the image (photos) instead of placing
    /// it on the background colour (logos). `None`: decided by the image
    /// (an opaque picture fills, an SVG or a transparent one is placed).
    pub fill: Option<bool>,
}

impl Default for Options {
    fn default() -> Self {
        Self { background: [0x0b, 0x10, 0x1a], accent: [0x7c, 0xd5, 0xc4], spinner: Spinner::Ring, scale: 0.28, position: Position::Center, fill: None }
    }
}

pub const SCALE_RANGE: (f32, f32) = (0.1, 0.8);

impl Options {
    pub fn clamped(mut self) -> Self {
        self.scale = if self.scale.is_finite() { self.scale.clamp(SCALE_RANGE.0, SCALE_RANGE.1) } else { Options::default().scale };
        self
    }
}

/// A checked picture, ready to draw at any size.
pub enum Source {
    Raster(RgbaImage),
    Svg(Box<usvg::Tree>),
}

impl Source {
    /// Its natural size in pixels.
    pub fn size(&self) -> (f32, f32) {
        match self {
            Source::Raster(i) => (i.width() as f32, i.height() as f32),
            Source::Svg(t) => (t.size().width(), t.size().height()),
        }
    }

    /// Whether it should fill the screen when [`Options::fill`] is unset:
    /// an opaque picture does; an SVG or a picture with transparency is a
    /// logo.
    pub fn is_photo(&self) -> bool {
        match self {
            Source::Raster(i) => i.pixels().all(|p| p[3] == u8::MAX),
            Source::Svg(_) => false,
        }
    }

    /// Drawn to exactly `w × h` pixels (aspect kept by the caller).
    pub fn draw(&self, w: u32, h: u32) -> RgbaImage {
        let (w, h) = (w.clamp(1, MAX_SIDE), h.clamp(1, MAX_SIDE));
        match self {
            Source::Raster(i) => image::imageops::resize(i, w, h, image::imageops::FilterType::Lanczos3),
            Source::Svg(tree) => {
                let Some(mut pixmap) = Pixmap::new(w, h) else { return RgbaImage::new(w, h) };
                let s = tree.size();
                resvg::render(tree, Transform::from_scale(w as f32 / s.width(), h as f32 / s.height()), &mut pixmap.as_mut());
                unpremultiply(pixmap)
            }
        }
    }

    /// Fitted inside `max_w × max_h`, aspect kept.
    pub fn fit(&self, max_w: f32, max_h: f32) -> RgbaImage {
        let (w, h) = self.size();
        let k = (max_w / w).min(max_h / h);
        self.draw((w * k).round().max(1.0) as u32, (h * k).round().max(1.0) as u32)
    }

    /// Covering `w × h` (cropped to the middle), aspect kept.
    pub fn cover(&self, w: u32, h: u32) -> RgbaImage {
        let (sw, sh) = self.size();
        let k = (w as f32 / sw).max(h as f32 / sh);
        let (dw, dh) = (((sw * k).ceil() as u32).max(w), ((sh * k).ceil() as u32).max(h));
        let big = self.draw(dw, dh);
        image::imageops::crop_imm(&big, (dw - w) / 2, (dh - h) / 2, w, h).to_image()
    }
}

fn unpremultiply(pixmap: Pixmap) -> RgbaImage {
    let (w, h) = (pixmap.width(), pixmap.height());
    let mut data = pixmap.take();
    for px in data.as_chunks_mut::<4>().0 {
        let a = px[3];
        if a > 0 && a < u8::MAX {
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * 255 + u16::from(a) / 2) / u16::from(a)).min(255) as u8;
            }
        }
    }
    RgbaImage::from_raw(w, h, data).unwrap_or_else(|| RgbaImage::new(w, h))
}

/// Refuses an SVG that scripts, embeds other documents, declares entities
/// or points outside itself.
pub fn check_svg(text: &str) -> Result<(), String> {
    let lower = text.to_ascii_lowercase();
    let refuse = || t("This SVG loads other files or runs scripts, so it can't be used at boot").to_string();
    for bad in ["<script", "<foreignobject", "<!entity", "javascript:", "<iframe", "<object", "<embed", "@import", "<!doctype"] {
        if lower.contains(bad) {
            return Err(refuse());
        }
    }
    // Every href must be a fragment or inline image data.
    let mut rest = lower.as_str();
    while let Some(at) = rest.find("href") {
        let after = rest[at + 4..].trim_start();
        if let Some(value) = after.strip_prefix('=') {
            let value = value.trim_start();
            let v = value.trim_start_matches(['"', '\'']).trim_start();
            if !(v.starts_with('#') || v.starts_with("data:image/")) {
                return Err(refuse());
            }
        }
        rest = &rest[at + 4..];
    }
    // CSS url(): fragments and inline data only.
    let mut rest = lower.as_str();
    while let Some(at) = rest.find("url(") {
        let v = rest[at + 4..].trim_start().trim_start_matches(['"', '\'']);
        if !(v.starts_with('#') || v.starts_with("data:image/")) {
            return Err(refuse());
        }
        rest = &rest[at + 4..];
    }
    Ok(())
}

/// Reads and checks `path`.
pub fn load(path: &Path) -> Result<Source, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(tf("{path} isn't a file", &[("path", &path.display())]));
    }
    if meta.len() > MAX_FILE {
        return Err(tf("The image is larger than {size}", &[("size", &"24 MB")]));
    }
    let svg = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("svg"));
    if svg {
        if meta.len() > MAX_SVG {
            return Err(tf("The image is larger than {size}", &[("size", &"4 MB")]));
        }
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        return svg_source(&text);
    }
    let mut reader = image::ImageReader::open(path).and_then(|r| r.with_guessed_format()).map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(512 << 20);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| tf("The image can't be read: {error}", &[("error", &e)]))?;
    Ok(Source::Raster(img.into_rgba8()))
}

/// Parses checked SVG text with nothing loaded from outside.
pub fn svg_source(text: &str) -> Result<Source, String> {
    check_svg(text)?;
    let opts = usvg::Options { resources_dir: None, image_href_resolver: usvg::ImageHrefResolver { resolve_data: usvg::ImageHrefResolver::default_data_resolver(), resolve_string: Box::new(|_, _| None) }, ..Default::default() };
    let tree = usvg::Tree::from_str(text, &opts).map_err(|e| tf("The image can't be read: {error}", &[("error", &e)]))?;
    let s = tree.size();
    if !(s.width() >= 1.0 && s.height() >= 1.0 && s.width() <= 100_000.0 && s.height() <= 100_000.0) {
        return Err(t("The SVG has no usable size").to_string());
    }
    Ok(Source::Svg(Box::new(tree)))
}

fn rgba(c: [u8; 3], a: u8) -> Rgba<u8> {
    Rgba([c[0], c[1], c[2], a])
}

fn paint(c: [u8; 3], alpha: f32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c[0], c[1], c[2], (alpha.clamp(0.0, 1.0) * 255.0) as u8);
    p.anti_alias = true;
    p
}

fn save(img: &RgbaImage, path: &Path) -> Result<(), String> {
    img.save_with_format(path, image::ImageFormat::Png).map_err(|e| format!("{}: {e}", path.display()))
}

fn save_pixmap(pixmap: Pixmap, path: &Path) -> Result<(), String> {
    save(&unpremultiply(pixmap), path)
}

/// `src` over `dst` at `(x, y)`.
fn over(dst: &mut RgbaImage, src: &RgbaImage, x: i64, y: i64) {
    image::imageops::overlay(dst, src, x, y);
}

/// The screen as the boot shows it: the background colour (or the picture
/// covering it) with the image placed by `opts`.
pub fn screen(src: &Source, opts: &Options, (w, h): (u32, u32)) -> RgbaImage {
    let (w, h) = (w.clamp(16, MAX_SIDE), h.clamp(16, MAX_SIDE));
    if opts.fill.unwrap_or_else(|| src.is_photo()) {
        return src.cover(w, h);
    }
    let mut out = RgbaImage::from_pixel(w, h, rgba(opts.background, u8::MAX));
    let logo = logo(src, opts, (w, h));
    let x = (i64::from(w) - i64::from(logo.width())) / 2;
    let y = (h as f32 * opts.position.y()) as i64 - i64::from(logo.height()) / 2;
    over(&mut out, &logo, x, y);
    out
}

/// The image at its boot size (`scale` of the screen height, at most 80 %
/// of its width).
fn logo(src: &Source, opts: &Options, (w, h): (u32, u32)) -> RgbaImage {
    let o = opts.clone().clamped();
    src.fit(w as f32 * 0.8, h as f32 * o.scale)
}

fn k1080(h: u32) -> f32 {
    (h as f32 / 1080.0).max(0.5)
}

fn hex(c: [u8; 3]) -> String {
    format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// A rounded rectangle path.
fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

fn arc(cx: f32, cy: f32, r: f32, start: f32, sweep: f32) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let steps = 32;
    for i in 0..=steps {
        let a = start + sweep * i as f32 / steps as f32;
        let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
        if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
    }
    pb.finish()
}

/// Frame `i` of the spinner (`None` for no spinner).
fn spinner_frame(spinner: Spinner, accent: [u8; 3], side: u32, i: u32) -> Option<Pixmap> {
    let s = side as f32;
    let mut pm = Pixmap::new(side, side)?;
    let phase = i as f32 / SPINNER_FRAMES as f32;
    {
        match spinner {
            Spinner::Ring => {
                let stroke = Stroke { width: s * 0.1, line_cap: tiny_skia::LineCap::Round, ..Default::default() };
                let r = s * 0.4;
                if let Some(track) = arc(s / 2.0, s / 2.0, r, 0.0, std::f32::consts::TAU) {
                    pm.stroke_path(&track, &paint(accent, 0.18), &stroke, Transform::identity(), None);
                }
                if let Some(head) = arc(s / 2.0, s / 2.0, r, phase * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2, std::f32::consts::TAU * 0.28) {
                    pm.stroke_path(&head, &paint(accent, 1.0), &stroke, Transform::identity(), None);
                }
            }
            Spinner::Dots => {
                for d in 0..3 {
                    let wave = ((phase - d as f32 / 6.0) * std::f32::consts::TAU).sin() * 0.5 + 0.5;
                    let r = s * (0.08 + 0.05 * wave);
                    let cx = s * (0.2 + 0.3 * d as f32);
                    let mut pb = PathBuilder::new();
                    pb.push_circle(cx, s / 2.0, r);
                    if let Some(c) = pb.finish() {
                        pm.fill_path(&c, &paint(accent, 0.35 + 0.65 * wave), FillRule::Winding, Transform::identity(), None);
                    }
                }
            }
            Spinner::None => return None,
        }
    }
    Some(pm)
}

fn spinner_frames(dir: &Path, spinner: Spinner, accent: [u8; 3], side: u32) -> Result<(), String> {
    for i in 0..SPINNER_FRAMES {
        let Some(pm) = spinner_frame(spinner, accent, side, i) else { return Ok(()) };
        save_pixmap(pm, &dir.join(format!("throbber-{:04}.png", i + 1)))?;
    }
    Ok(())
}

/// Where the splash's spinner sits (share of the screen height).
fn spinner_y(opts: &Options, fill: bool) -> f32 {
    if fill { 0.85 } else { (opts.position.y() + opts.scale / 2.0 + 0.1).min(0.9) }
}

/// The splash as it will look, drawn at `size` (a preview: the screen
/// and a spinner frame where Plymouth puts it).
pub fn preview(src: &Source, opts: &Options, size: (u32, u32)) -> RgbaImage {
    let opts = opts.clone().clamped();
    let mut out = screen(src, &opts, size);
    let (w, h) = out.dimensions();
    let side = (SPINNER_PX_1080 * k1080(h)).round().max(8.0) as u32;
    if let Some(frame) = spinner_frame(opts.spinner, opts.accent, side, SPINNER_FRAMES / 3) {
        let frame = unpremultiply(frame);
        let fill = opts.fill.unwrap_or_else(|| src.is_photo());
        let y = (h as f32 * spinner_y(&opts, fill)) as i64 - i64::from(side) / 2;
        over(&mut out, &frame, (i64::from(w) - i64::from(side)) / 2, y);
    }
    out
}

/// The password field, bullet and lock two-step needs.
fn password_widgets(dir: &Path, accent: [u8; 3], background: [u8; 3], k: f32) -> Result<(), String> {
    let light = luminance(background) < 0.5;
    let ink = if light { [0xff, 0xff, 0xff] } else { [0x10, 0x14, 0x1c] };
    let (ew, eh) = ((ENTRY_W_1080 * k).round() as u32, (ENTRY_H_1080 * k).round() as u32);
    let mut entry = Pixmap::new(ew, eh).ok_or("entry")?;
    if let Some(p) = rounded(1.0, 1.0, ew as f32 - 2.0, eh as f32 - 2.0, eh as f32 / 2.0) {
        entry.fill_path(&p, &paint(ink, 0.10), FillRule::Winding, Transform::identity(), None);
        entry.stroke_path(&p, &paint(accent, 0.55), &Stroke { width: 1.5 * k, ..Default::default() }, Transform::identity(), None);
    }
    save_pixmap(entry, &dir.join("entry.png"))?;
    let b = (BULLET_PX_1080 * k).round().max(4.0) as u32;
    let mut bullet = Pixmap::new(b, b).ok_or("bullet")?;
    let mut pb = PathBuilder::new();
    pb.push_circle(b as f32 / 2.0, b as f32 / 2.0, b as f32 / 2.0 - 0.5);
    if let Some(c) = pb.finish() {
        bullet.fill_path(&c, &paint(ink, 0.92), FillRule::Winding, Transform::identity(), None);
    }
    save_pixmap(bullet, &dir.join("bullet.png"))?;
    let l = (LOCK_PX_1080 * k).round().max(10.0) as u32;
    let lf = l as f32;
    let mut lock = Pixmap::new(l, l).ok_or("lock")?;
    if let Some(body) = rounded(lf * 0.15, lf * 0.45, lf * 0.7, lf * 0.5, lf * 0.1) {
        lock.fill_path(&body, &paint(accent, 1.0), FillRule::Winding, Transform::identity(), None);
    }
    if let Some(shackle) = arc(lf / 2.0, lf * 0.45, lf * 0.22, std::f32::consts::PI, std::f32::consts::PI) {
        lock.stroke_path(&shackle, &paint(accent, 1.0), &Stroke { width: lf * 0.1, ..Default::default() }, Transform::identity(), None);
    }
    save_pixmap(lock, &dir.join("lock.png"))?;
    Ok(())
}

fn luminance(c: [u8; 3]) -> f32 {
    (0.2126 * f32::from(c[0]) + 0.7152 * f32::from(c[1]) + 0.0722 * f32::from(c[2])) / 255.0
}

/// Writes the Plymouth theme [`super::SPLASH_NAME`] into `dir` (empty)
/// for a `screen` (physical pixels).
pub fn write_splash(dir: &Path, src: &Source, opts: &Options, screen: (u32, u32)) -> Result<(), String> {
    let opts = opts.clone().clamped();
    let (w, h) = (screen.0.clamp(320, MAX_SIDE), screen.1.clamp(240, MAX_SIDE));
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let k = k1080(h);
    let fill = opts.fill.unwrap_or_else(|| src.is_photo());
    // A photo fills the screen as the watermark itself; a logo is placed.
    let watermark = if fill { src.cover(w, h) } else { logo(src, &opts, (w, h)) };
    save(&watermark, &dir.join("watermark.png"))?;
    spinner_frames(dir, opts.spinner, opts.accent, (SPINNER_PX_1080 * k).round() as u32)?;
    password_widgets(dir, opts.accent, opts.background, k)?;
    let name = super::SPLASH_NAME;
    let bg = hex(opts.background);
    let bar = hex(opts.accent);
    let wm_y = if fill { 0.5 } else { opts.position.y() };
    let spin_y = spinner_y(&opts, fill);
    let body = format!(
        "[Plymouth Theme]\nName=Mandara splash\nDescription=Made by Mandara Boot look from your image\nModuleName=two-step\n\n[two-step]\nImageDir={themes}/{name}\nDialogHorizontalAlignment=.5\nDialogVerticalAlignment={spin_y:.3}\nTitleHorizontalAlignment=.5\nTitleVerticalAlignment=.382\nHorizontalAlignment=.5\nVerticalAlignment={spin_y:.3}\nWatermarkHorizontalAlignment=.5\nWatermarkVerticalAlignment={wm_y:.3}\nTransition=none\nTransitionDuration=0.0\nBackgroundStartColor=0x{bg}\nBackgroundEndColor=0x{bg}\nProgressBarBackgroundColor=0x{bg}\nProgressBarForegroundColor=0x{bar}\nMessageBelowAnimation=true\n\n[boot-up]\nUseEndAnimation=false\n\n[shutdown]\nUseEndAnimation=false\n\n[reboot]\nUseEndAnimation=false\n",
        themes = super::detect::PLYMOUTH_THEMES
    );
    std::fs::write(dir.join(format!("{name}.plymouth")), body).map_err(|e| e.to_string())
}

/// Writes the GRUB theme [`super::GRUB_IMAGE_NAME`] into `dir` for a menu
/// of `size` pixels (`GRUB_GFXMODE`).
pub fn write_grub(dir: &Path, src: &Source, opts: &Options, size: (u32, u32)) -> Result<(), String> {
    let opts = opts.clone().clamped();
    let (w, h) = (size.0.clamp(640, MAX_SIDE), size.1.clamp(480, MAX_SIDE));
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let fill = opts.fill.unwrap_or_else(|| src.is_photo());
    // A placed logo sits higher so the menu has room below it.
    let mut menu_opts = opts.clone();
    if !fill {
        menu_opts.position = Position::Upper;
        menu_opts.scale = opts.scale.min(0.3);
    }
    let bg = screen(src, &menu_opts, (w, h));
    save(&bg, &dir.join("background.png"))?;
    let k = k1080(h);
    let item_h = (GRUB_ITEM_H * k).round() as u32;
    // The pill highlight: left cap, centre, right cap.
    let pad = (GRUB_SELECT_PAD * k).round();
    let cap = item_h / 2;
    let Some(mut left) = Pixmap::new(cap.max(1), item_h) else { return Err("highlight".into()) };
    let Some(mut right) = Pixmap::new(cap.max(1), item_h) else { return Err("highlight".into()) };
    let Some(mut centre) = Pixmap::new(4, item_h) else { return Err("highlight".into()) };
    let accent = paint(opts.accent, 0.32);
    let r = item_h as f32 / 2.0 - pad;
    let mut pb = PathBuilder::new();
    pb.push_circle(cap as f32, item_h as f32 / 2.0, r);
    if let Some(c) = pb.finish() {
        left.fill_path(&c, &accent, FillRule::Winding, Transform::identity(), None);
        right.fill_path(&c, &accent, FillRule::Winding, Transform::from_translate(-(cap as f32), 0.0), None);
    }
    if let Some(rect) = tiny_skia::Rect::from_xywh(0.0, pad, 4.0, item_h as f32 - 2.0 * pad) {
        centre.fill_rect(rect, &accent, Transform::identity(), None);
    }
    save_pixmap(left, &dir.join("select_w.png"))?;
    save_pixmap(centre, &dir.join("select_c.png"))?;
    save_pixmap(right, &dir.join("select_e.png"))?;
    let light_bg = luminance(average(&bg)) > 0.6;
    let (item, selected, dim) = if light_bg { ("#30343c", "#0b0f16", "#5a606c") } else { ("#c9d1dc", "#ffffff", "#8a93a3") };
    let menu_top = if fill { 40 } else { 56 };
    let body = format!(
        "# Made by Mandara Boot look from your image (Studio → Themes → Boot).\ntitle-text: \"\"\ndesktop-image: \"background.png\"\ndesktop-color: \"#{bg}\"\nterminal-font: \"Unifont Regular 16\"\nterminal-left: \"10%\"\nterminal-top: \"10%\"\nterminal-width: \"80%\"\nterminal-height: \"80%\"\n\n+ boot_menu {{\n  left = 30%\n  top = {menu_top}%\n  width = 40%\n  height = 30%\n  item_font = \"Unifont Regular 16\"\n  item_color = \"{item}\"\n  selected_item_color = \"{selected}\"\n  item_height = {item_h}\n  item_padding = {ipad}\n  item_spacing = {ispace}\n  icon_width = 0\n  icon_height = 0\n  item_icon_space = 0\n  selected_item_pixmap_style = \"select_*.png\"\n  scrollbar = false\n}}\n\n+ label {{\n  top = 92%\n  left = 0\n  width = 100%\n  height = 24\n  align = \"center\"\n  id = \"__timeout__\"\n  text = \"Booting in %d seconds\"\n  color = \"{dim}\"\n  font = \"Unifont Regular 16\"\n}}\n",
        bg = hex(opts.background),
        ipad = (14.0 * k).round() as u32,
        ispace = (6.0 * k).round() as u32,
    );
    std::fs::write(dir.join("theme.txt"), body).map_err(|e| e.to_string())
}

fn average(img: &RgbaImage) -> [u8; 3] {
    let step = ((img.width() * img.height()) / 4096).max(1) as usize;
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for p in img.pixels().step_by(step) {
        r += u64::from(p[0]);
        g += u64::from(p[1]);
        b += u64::from(p[2]);
        n += 1;
    }
    let n = n.max(1);
    [(r / n) as u8, (g / n) as u8, (b / n) as u8]
}

/// Writes the Limine wallpaper (`size`: the menu's resolution).
pub fn write_wallpaper(path: &Path, src: &Source, opts: &Options, size: (u32, u32)) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    save(&screen(src, opts, size), path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOGO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="32" viewBox="0 0 64 32"><rect width="64" height="32" rx="8" fill="#e0a040"/><circle cx="32" cy="16" r="8" fill="#ffffff"/></svg>"##;

    #[test]
    fn hostile_svgs_are_refused() {
        for bad in [
            r#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><image xlink:href="/etc/shadow" width="9" height="9"/></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg"><image href="https://example.com/x.png" width="9" height="9"/></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg"><image href = 'file:///etc/passwd'/></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg"><foreignObject><div/></foreignObject></svg>"#,
            r#"<!DOCTYPE svg [<!ENTITY x SYSTEM "file:///etc/passwd">]><svg xmlns="http://www.w3.org/2000/svg">&x;</svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg"><style>@import url(https://x/y.css);</style></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg"><rect style="fill:url(https://x/p.svg#a)" width="9" height="9"/></svg>"#,
        ] {
            assert!(svg_source(bad).is_err(), "{bad}");
        }
        assert!(check_svg(r##"<svg><use href="#a"/><image href="data:image/png;base64,AAAA"/><rect fill="url(#g)"/></svg>"##).is_ok());
        assert!(svg_source("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"0\" height=\"0\"/>").is_err(), "no size");
    }

    #[test]
    fn oversized_and_broken_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.png");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(MAX_FILE + 1).unwrap();
        assert!(load(&big).is_err());
        let junk = dir.path().join("junk.png");
        std::fs::write(&junk, b"not a png").unwrap();
        assert!(load(&junk).is_err());
        // A PNG header claiming a 100000 × 100000 image.
        let mut huge = Vec::new();
        huge.extend_from_slice(b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR");
        huge.extend_from_slice(&100_000u32.to_be_bytes());
        huge.extend_from_slice(&100_000u32.to_be_bytes());
        huge.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        let hp = dir.path().join("huge.png");
        std::fs::write(&hp, huge).unwrap();
        assert!(load(&hp).is_err(), "dimensions over the cap");
        assert!(load(dir.path()).is_err(), "a folder");
    }

    #[test]
    fn splash_theme_is_complete_and_valid() {
        let dir = tempfile::tempdir().unwrap();
        let theme = dir.path().join(super::super::SPLASH_NAME);
        let src = svg_source(LOGO).unwrap();
        assert!(!src.is_photo());
        let opts = Options { scale: 5.0, ..Options::default() };
        write_splash(&theme, &src, &opts, (1920, 1080)).unwrap();
        let info = super::super::theme::check_plymouth(&theme, super::super::SPLASH_NAME, Some(&["two-step".to_string()])).unwrap();
        assert_eq!(info.module, "two-step");
        for f in ["watermark.png", "entry.png", "bullet.png", "lock.png", "throbber-0001.png", "throbber-0024.png"] {
            assert!(theme.join(f).is_file(), "{f}");
        }
        let wm = image::open(theme.join("watermark.png")).unwrap();
        assert!(wm.height() <= (1080.0 * SCALE_RANGE.1) as u32 + 1 && wm.width() <= 1536 + 1, "scale is clamped: {}x{}", wm.width(), wm.height());
        assert_eq!(wm.width(), wm.height() * 2, "aspect kept");
        // No spinner: no throbber frames.
        let none = dir.path().join("none");
        write_splash(&none, &src, &Options { spinner: Spinner::None, ..Options::default() }, (1280, 800)).unwrap();
        assert!(!none.join("throbber-0001.png").exists() && none.join("lock.png").is_file());
    }

    #[test]
    fn grub_theme_and_wallpaper_are_drawn_at_the_menu_size() {
        let dir = tempfile::tempdir().unwrap();
        let src = Source::Raster(RgbaImage::from_pixel(400, 300, Rgba([200, 30, 40, 255])));
        assert!(src.is_photo());
        let theme = dir.path().join(super::super::GRUB_IMAGE_NAME);
        write_grub(&theme, &src, &Options::default(), (1920, 1080)).unwrap();
        let info = super::super::theme::check_grub(&theme).unwrap();
        assert_eq!(info.desktop_image.as_deref(), Some("background.png"));
        let bg = image::open(theme.join("background.png")).unwrap();
        assert_eq!((bg.width(), bg.height()), (1920, 1080));
        assert!(theme.join("select_c.png").is_file() && theme.join("select_w.png").is_file());
        let wall = dir.path().join("w/wallpaper.png");
        write_wallpaper(&wall, &svg_source(LOGO).unwrap(), &Options::default(), (1280, 720)).unwrap();
        let w = image::open(&wall).unwrap().into_rgba8();
        assert_eq!(w.dimensions(), (1280, 720));
        assert_eq!(w.get_pixel(2, 2).0, [0x0b, 0x10, 0x1a, 255], "a logo sits on the background colour");
    }
}
