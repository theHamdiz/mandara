//! Wallpaper transitions drawn per pixel on the GPU: GPUI's transition
//! primitive (`Window::paint_transition`, `vendor/gpui_wgpu` patch 16) samples
//! the outgoing and incoming images, plus a small helper image for Sketch,
//! and evaluates the chosen transition in one fragment shader. No tiles, so
//! no seams.
//!
//! This module holds what runs on the CPU:
//!
//! * which transitions use the shader and how the config maps to its
//!   parameters ([`code`], [`Look`]);
//! * the helper image ([`build_aux`]): the new wallpaper's subject mask, the
//!   distance from it and the distance from the nearest line, at half
//!   resolution, built once per change on the background job that loaded
//!   the wallpaper, then uploaded and dropped;
//! * [`Player`] and its element: it uploads the images on the first frame,
//!   frees their CPU copies at once, plays the transition and removes the
//!   textures when it ends.
//!
//! The default software wallpaper shows these through a temporary GPU
//! surface (`backdrop::overlay`); the GPU wallpaper paints them itself.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{App, Bounds, DevicePixels, Element, ElementId, GlobalElementId, ImageId, InspectorElementId, IntoElement, LayoutId, Pixels, Position, Style, TransitionImage, TransitionParams, Window, size};
use image::GrayImage;
use mandara_config::desktop::{SketchFill, SketchInk, SketchOptions, SketchOrder, WallpaperFill, WallpaperTransition};
use mandara_theme::Color;

/// Alpha of the faint outline left over the wallpaper
/// ([`SketchOptions::keep_outline`]).
pub const KEEP_ALPHA: f32 = 0.16;
/// The wash over the old image while Sketch draws: its alpha.
const WASH_ALPHA: f32 = 0.55;
/// Line colours of the Ink and White choices, and the washes under them.
const INK_BLACK: (u8, u8, u8) = (0x1b, 0x1a, 0x1f);
const PAPER: (u8, u8, u8) = (0xf3, 0xee, 0xe4);
const NIGHT: (u8, u8, u8) = (0x0a, 0x0c, 0x10);
/// Lines darker than this (OK lightness) get the paper wash.
const DARK_LINE: f32 = 0.5;
/// Hue bins for the wallpaper's dominant colour, and the lightness it is
/// lifted to (lines must read over the dark wash).
const HUE_BINS: usize = 12;
const INK_LIGHTNESS: f32 = 0.72;
/// Shatter: about this many shards on a 2560 × 1440 (logical) output at
/// full intensity, and the bounds.
const SHARDS_REFERENCE: f32 = 100.0;
const SHARDS_REFERENCE_AREA: f32 = 2560.0 * 1440.0;
const SHARDS_RANGE: (f32, f32) = (40.0, 120.0);
/// The helper image is at most this long (it is half the output's size).
const AUX_MAX_EDGE: u32 = 2048;
/// Distances in the helper image: from a line, in helper pixels, and from
/// the subject, as a share of the diagonal, that map to 1.
const AUX_LINE_RANGE_PX: f32 = 48.0;
const AUX_SUBJECT_RANGE: f32 = 0.6;
/// Edge strength (Sobel of the luminance) that starts and saturates a line,
/// and the strength that seeds the distance from lines. The shader uses the
/// same ramp.
const EDGE_RAMP: (f32, f32) = (0.16, 0.42);
const LINE_SEED: f32 = 0.35;
/// Mask alpha at or above this is the subject.
const SUBJECT_ALPHA: u8 = 128;
/// Rec. 709 luma weights.
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];
/// Shortest transition played (a zero length would divide by zero).
const MIN_DURATION: Duration = Duration::from_millis(1);

/// The shader's code for `kind` (`None`: it draws nothing).
pub fn code(kind: WallpaperTransition) -> Option<u32> {
    use WallpaperTransition as T;
    Some(match kind {
        T::Fade => gpui::TRANSITION_KIND_FADE,
        T::Wipe => gpui::TRANSITION_KIND_WIPE,
        T::Grow => gpui::TRANSITION_KIND_GROW,
        T::Ripple => gpui::TRANSITION_KIND_RIPPLE,
        T::Tide => gpui::TRANSITION_KIND_TIDE,
        T::Tear => gpui::TRANSITION_KIND_TEAR,
        T::Glitch => gpui::TRANSITION_KIND_GLITCH,
        T::Shatter => gpui::TRANSITION_KIND_SHATTER,
        T::Dissolve => gpui::TRANSITION_KIND_DISSOLVE,
        T::Sketch => gpui::TRANSITION_KIND_SKETCH,
        T::None => return None,
    })
}

/// Whether `kind` needs the shader. A fade and a wipe are cropped or faded
/// by the compositor on the software wallpaper (and drawn as layers on the
/// GPU one); a cut draws nothing.
pub fn uses_shader(kind: WallpaperTransition) -> bool {
    !matches!(kind, WallpaperTransition::Fade | WallpaperTransition::Wipe | WallpaperTransition::None)
}

/// What a change with `kind` draws: reduced motion makes every animated
/// transition a plain fade (a cut stays a cut).
pub fn effective(kind: WallpaperTransition, reduced: bool) -> WallpaperTransition {
    if reduced && kind != WallpaperTransition::None { WallpaperTransition::Fade } else { kind }
}

/// Shatter's grid on a `logical` output: about [`SHARDS_REFERENCE`] shards
/// on a 2560 × 1440 output, fewer on smaller ones and at a lower motion
/// `intensity`, within [`SHARDS_RANGE`], in cells about as wide as tall.
pub fn shatter_grid(logical: (f32, f32), intensity: f32) -> [u32; 2] {
    let (w, h) = (logical.0.max(1.0), logical.1.max(1.0));
    let area = (w * h / SHARDS_REFERENCE_AREA).sqrt();
    let wanted = (SHARDS_REFERENCE * area * (0.5 + 0.5 * intensity.clamp(0.0, 1.0))).clamp(SHARDS_RANGE.0, SHARDS_RANGE.1);
    let aspect = w / h;
    let rows = (wanted / aspect).sqrt().round().max(1.0);
    let cols = (wanted / rows).round().max(1.0);
    let max = gpui::TRANSITION_MAX_SHARDS;
    let cols = (cols as u32).min(max);
    [cols, (rows as u32).min(max / cols)]
}

fn rgba(c: Color, alpha: f32) -> [f32; 4] {
    let [r, g, b, _] = c.to_rgba8();
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, alpha]
}

fn rgb8((r, g, b): (u8, u8, u8)) -> Color {
    Color::from_rgba8(r, g, b, 255)
}

/// The colour of Sketch's lines and the wash under them.
pub fn sketch_colours(ink: SketchInk, accent: Color, dominant: Color) -> (Color, Color) {
    let line = match ink {
        SketchInk::Accent => accent.with_alpha(1.0),
        SketchInk::Ink => rgb8(INK_BLACK),
        SketchInk::White => rgb8((0xff, 0xff, 0xff)),
        SketchInk::Wallpaper => dominant,
    };
    (line, if line.lightness() < DARK_LINE { rgb8(PAPER) } else { rgb8(NIGHT) })
}

/// The picture's dominant colour (chroma-weighted hue bins of a small
/// copy), lifted to [`INK_LIGHTNESS`] so it reads as a line; a grey
/// picture gives a light grey.
pub fn dominant(rgb: &image::RgbImage) -> Color {
    let mut weight = [0f32; HUE_BINS];
    let mut sum = [[0f32; 3]; HUE_BINS];
    for p in rgb.pixels() {
        let c = Color::from_rgba8(p.0[0], p.0[1], p.0[2], 255);
        let chroma = c.chroma();
        let l = c.lightness();
        // Near-black and near-white pixels carry no colour worth drawing.
        if chroma < 0.03 || !(0.15..=0.95).contains(&l) {
            continue;
        }
        let bin = ((c.hue().rem_euclid(360.0) / 360.0 * HUE_BINS as f32) as usize).min(HUE_BINS - 1);
        weight[bin] += chroma;
        for (acc, v) in sum[bin].iter_mut().zip(p.0) {
            *acc += v as f32 * chroma;
        }
    }
    let (best, w) = weight.iter().enumerate().fold((0, 0.0f32), |acc, (i, &w)| if w > acc.1 { (i, w) } else { acc });
    if w <= f32::EPSILON {
        return Color::oklch(INK_LIGHTNESS + 0.1, 0.0, 0.0);
    }
    let avg = |k: usize| (sum[best][k] / w).round().clamp(0.0, 255.0) as u8;
    let c = Color::from_rgba8(avg(0), avg(1), avg(2), 255);
    c.with_lightness(c.lightness().max(INK_LIGHTNESS)).with_chroma(c.chroma().max(0.08)).gamut_mapped()
}

/// Everything about a transition's look that the frame doesn't change,
/// resolved once when it starts.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub kind: WallpaperTransition,
    /// Where it starts and the drawing order's point, fractions of the output.
    pub origin: [f32; 2],
    pub focus: [f32; 2],
    pub seed: u32,
    pub intensity: f32,
    /// Output size, logical px, and device px per logical px.
    pub logical: (f32, f32),
    pub scale: f32,
    pub rtl: bool,
    /// The theme's accent (Dissolve's burning edge, Sketch's accent ink).
    pub accent: Color,
    /// The new wallpaper's dominant colour (Sketch's "wallpaper" ink).
    pub dominant: Color,
    pub sketch: SketchOptions,
    /// The helper image's red channel is a subject mask.
    pub subject: bool,
}

impl Look {
    /// The shader's parameters at linear `progress` (0..1), `time` seconds in.
    pub fn params(&self, progress: f32, time: f32) -> TransitionParams {
        let sk = &self.sketch;
        let (line, wash) = sketch_colours(sk.ink, self.accent, self.dominant);
        let mut flags = 0;
        if self.subject {
            flags |= gpui::TRANSITION_FLAG_SUBJECT;
        }
        if self.rtl {
            flags |= gpui::TRANSITION_FLAG_RTL;
        }
        if sk.glow {
            flags |= gpui::TRANSITION_FLAG_GLOW;
        }
        let glow = if self.kind == WallpaperTransition::Sketch { rgba(line, 0.9) } else { rgba(self.accent.with_lightness(self.accent.lightness().max(0.72)), 1.0) };
        TransitionParams {
            kind: code(self.kind).unwrap_or(gpui::TRANSITION_KIND_FADE),
            flags,
            progress: progress.clamp(0.0, 1.0),
            time: time.max(0.0),
            origin: self.origin,
            focus: self.focus,
            seed: self.seed,
            intensity: self.intensity.clamp(0.0, 1.0),
            scale: self.scale,
            line_width: sk.line_px() * self.scale,
            line: rgba(line, 1.0),
            glow,
            wash: rgba(wash, WASH_ALPHA),
            draw_share: sk.draw(),
            fill_style: match sk.fill_style {
                SketchFill::Fade => 0,
                SketchFill::Bleed => 1,
                SketchFill::Wipe => 2,
            },
            draw_order: match sk.draw_order {
                SketchOrder::Subject => 0,
                SketchOrder::TopDown => 1,
                SketchOrder::Center => 2,
                SketchOrder::Pointer => 3,
            },
            grid: shatter_grid(self.logical, self.intensity),
        }
    }
}

/// `point` (logical px) as a fraction of `size`, or the centre when it is
/// unknown or outside.
pub fn fraction(point: Option<(f32, f32)>, size: (f32, f32)) -> [f32; 2] {
    match point {
        Some((x, y)) if (0.0..size.0).contains(&x) && (0.0..size.1).contains(&y) => [x / size.0.max(1.0), y / size.1.max(1.0)],
        _ => [0.5, 0.5],
    }
}

// ── Images ───────────────────────────────────────────────────────────────

/// One image for the shader: straight BGRA, as the sprite atlas takes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub size: (u32, u32),
    pub bgra: Vec<u8>,
}

impl Image {
    /// Opaque premultiplied RGBA (a `tiny_skia` pixmap's bytes), swizzled in
    /// place: alpha is 255, so premultiplied equals straight.
    pub fn from_rgba(size: (u32, u32), mut rgba: Vec<u8>) -> Self {
        for p in rgba.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
        }
        Self { size, bgra: rgba }
    }
}

/// The three images of a transition.
#[derive(Debug)]
pub struct Frames {
    pub from: Image,
    pub to: Image,
    pub aux: Option<Image>,
}

/// The subject mask Mandara already has for `source`: yours, the one the
/// depth effect's current mode made, or one the hand-tuned mode made
/// before. Nothing is generated for a transition (the depth effect does
/// that, once, after the wallpaper is shown), so a wallpaper's first
/// change uses its edges alone.
pub fn existing_mask(source: &Path, depth: Option<crate::depth::Request>) -> Option<PathBuf> {
    use crate::depth::{Request, generated_path, user_mask_path};
    if let Some(user) = user_mask_path(source).filter(|p| p.is_file()) {
        return Some(user);
    }
    let hand = Request { mode: mandara_config::DepthMode::HandTuned, split: None };
    depth.into_iter().chain([hand]).map(|r| generated_path(source, r)).find(|p| p.is_file())
}

/// `mask` aligned to the helper image of a wallpaper shown with `fill` at
/// `output` size, if the two line up (a cover crop, a stretch); `None` for
/// fills where they no longer do.
pub fn aux_mask(mask: &Path, fill: WallpaperFill, output: (u32, u32)) -> Option<GrayImage> {
    let cropped = match fill {
        WallpaperFill::Cover => true,
        WallpaperFill::Stretch => false,
        _ => return None,
    };
    let alpha = crate::depth::load_alpha(mask).map_err(|e| tracing::debug!("transition mask: {e}")).ok()?;
    Some(crate::depth::align(&alpha, aux_size(output), cropped))
}

/// Size of the helper image for an `output` of that size.
pub fn aux_size((w, h): (u32, u32)) -> (u32, u32) {
    let (w, h) = ((w / 2).max(1), (h / 2).max(1));
    let k = (AUX_MAX_EDGE as f32 / w.max(h) as f32).min(1.0);
    (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1))
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Two-pass 3-4 chamfer distance (in pixels) from every `true` pixel of
/// `seeds`, into `dist` (reused; units of 1/3 px).
fn chamfer(seeds: impl Fn(usize) -> bool, (w, h): (usize, usize), dist: &mut Vec<u16>) {
    const FAR: u16 = u16::MAX - 8;
    dist.clear();
    dist.extend((0..w * h).map(|i| if seeds(i) { 0 } else { FAR }));
    let at = |d: &[u16], x: isize, y: isize| -> u16 { if x < 0 || y < 0 || x >= w as isize || y >= h as isize { FAR } else { d[y as usize * w + x as usize] } };
    for y in 0..h as isize {
        for x in 0..w as isize {
            let i = y as usize * w + x as usize;
            let best = dist[i].min(at(dist, x - 1, y).saturating_add(3)).min(at(dist, x, y - 1).saturating_add(3)).min(at(dist, x - 1, y - 1).saturating_add(4)).min(at(dist, x + 1, y - 1).saturating_add(4));
            dist[i] = best;
        }
    }
    for y in (0..h as isize).rev() {
        for x in (0..w as isize).rev() {
            let i = y as usize * w + x as usize;
            let best = dist[i].min(at(dist, x + 1, y).saturating_add(3)).min(at(dist, x, y + 1).saturating_add(3)).min(at(dist, x + 1, y + 1).saturating_add(4)).min(at(dist, x - 1, y + 1).saturating_add(4));
            dist[i] = best;
        }
    }
}

/// The helper image of an incoming wallpaper (`rgba`: opaque RGBA at
/// `size`), at [`aux_size`]: red = subject mask (`mask`, already aligned
/// at that size), green = distance from the subject (0 inside), blue =
/// distance from the nearest line, alpha 255. With `keep`, the outline is
/// also drawn faintly into `rgba` in that colour (the "keep the outline"
/// option), so whatever shows the wallpaper afterwards has it.
pub fn build_aux(rgba: &mut [u8], size: (u32, u32), mask: Option<&GrayImage>, keep: Option<[u8; 3]>) -> Image {
    let (aw, ah) = aux_size(size);
    let (w, h) = (size.0 as usize, size.1 as usize);
    let (aw_, ah_) = (aw as usize, ah as usize);
    // Luminance at the helper's size (box filter).
    let mut luma = vec![0f32; aw_ * ah_];
    for (ay, row) in luma.chunks_exact_mut(aw_).enumerate() {
        let (y0, y1) = (ay * h / ah_, ((ay + 1) * h / ah_).max(ay * h / ah_ + 1).min(h));
        for (ax, v) in row.iter_mut().enumerate() {
            let (x0, x1) = (ax * w / aw_, ((ax + 1) * w / aw_).max(ax * w / aw_ + 1).min(w));
            let mut sum = 0.0;
            for y in y0..y1 {
                for p in rgba[(y * w + x0) * 4..(y * w + x1) * 4].as_chunks::<4>().0 {
                    sum += LUMA[0] * p[0] as f32 + LUMA[1] * p[1] as f32 + LUMA[2] * p[2] as f32;
                }
            }
            *v = sum / (((y1 - y0) * (x1 - x0)) as f32 * 255.0);
        }
    }
    // A 3 × 3 box blur first, so specks (stars, grain) seed no lines of
    // their own while real contours stay strong.
    let at = |buf: &[f32], x: isize, y: isize| buf[(y.clamp(0, ah_ as isize - 1) as usize) * aw_ + x.clamp(0, aw_ as isize - 1) as usize];
    let mut edge = vec![0f32; aw_ * ah_];
    for y in 0..ah_ as isize {
        for x in 0..aw_ as isize {
            let mut sum = 0.0;
            for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (0, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                sum += at(&luma, x + dx, y + dy);
            }
            edge[y as usize * aw_ + x as usize] = sum / 9.0;
        }
    }
    // Edge strength (Sobel) of the blurred luminance, into the first buffer.
    std::mem::swap(&mut luma, &mut edge);
    for y in 0..ah_ as isize {
        for x in 0..aw_ as isize {
            let l = |dx: isize, dy: isize| at(&luma, x + dx, y + dy);
            let gx = (l(1, -1) + 2.0 * l(1, 0) + l(1, 1)) - (l(-1, -1) + 2.0 * l(-1, 0) + l(-1, 1));
            let gy = (l(-1, 1) + 2.0 * l(0, 1) + l(1, 1)) - (l(-1, -1) + 2.0 * l(0, -1) + l(1, -1));
            edge[y as usize * aw_ + x as usize] = smoothstep(EDGE_RAMP.0, EDGE_RAMP.1, (gx * gx + gy * gy).sqrt());
        }
    }
    drop(luma);
    let mask = mask.filter(|m| m.dimensions() == (aw, ah));
    let inside = |i: usize| mask.is_some_and(|m| m.as_raw()[i] >= SUBJECT_ALPHA);
    // The subject's contour is a line too.
    let contour = |i: usize| {
        let (x, y) = (i % aw_, i / aw_);
        inside(i) && (x == 0 || y == 0 || x + 1 == aw_ || y + 1 == ah_ || !inside(i - 1) || !inside(i + 1) || !inside(i - aw_) || !inside(i + aw_))
    };
    let mut out = vec![255u8; aw_ * ah_ * 4];
    let mut dist = Vec::new();
    chamfer(|i| edge[i] > LINE_SEED || contour(i), (aw_, ah_), &mut dist);
    for (p, d) in out.as_chunks_mut::<4>().0.iter_mut().zip(&dist) {
        p[0] = ((*d as f32 / 3.0 / AUX_LINE_RANGE_PX).min(1.0) * 255.0).round() as u8;
    }
    if let Some(m) = mask {
        chamfer(inside, (aw_, ah_), &mut dist);
        let range = ((aw * aw + ah * ah) as f32).sqrt() * AUX_SUBJECT_RANGE;
        for ((p, d), a) in out.as_chunks_mut::<4>().0.iter_mut().zip(&dist).zip(m.as_raw()) {
            p[1] = ((*d as f32 / 3.0 / range).min(1.0) * 255.0).round() as u8;
            p[2] = *a;
        }
    } else {
        for p in out.as_chunks_mut::<4>().0 {
            (p[1], p[2]) = (255, 0);
        }
    }
    drop(dist);
    if let Some(ink) = keep {
        keep_outline(rgba, size, &edge, (aw_, ah_), mask, ink);
    }
    Image { size: (aw, ah), bgra: out }
}

/// Draws the outline ([`KEEP_ALPHA`]) into the full-size `rgba`: the edge
/// strength and subject contour, upsampled bilinearly.
fn keep_outline(rgba: &mut [u8], (w, h): (u32, u32), edge: &[f32], (aw, ah): (usize, usize), mask: Option<&GrayImage>, ink: [u8; 3]) {
    let sample = |buf: &dyn Fn(usize) -> f32, fx: f32, fy: f32| {
        let (x0, y0) = (fx.floor().clamp(0.0, aw as f32 - 1.0) as usize, fy.floor().clamp(0.0, ah as f32 - 1.0) as usize);
        let (x1, y1) = ((x0 + 1).min(aw - 1), (y0 + 1).min(ah - 1));
        let (tx, ty) = ((fx - x0 as f32).clamp(0.0, 1.0), (fy - y0 as f32).clamp(0.0, 1.0));
        let top = buf(y0 * aw + x0) * (1.0 - tx) + buf(y0 * aw + x1) * tx;
        let bottom = buf(y1 * aw + x0) * (1.0 - tx) + buf(y1 * aw + x1) * tx;
        top * (1.0 - ty) + bottom * ty
    };
    let edge_at = |i: usize| edge[i];
    let mask_at = |i: usize| mask.map_or(0.0, |m| m.as_raw()[i] as f32 / 255.0);
    let (kx, ky) = (aw as f32 / w as f32, ah as f32 / h as f32);
    for (y, row) in rgba.chunks_exact_mut(w as usize * 4).enumerate() {
        let fy = (y as f32 + 0.5) * ky - 0.5;
        for (x, p) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let fx = (x as f32 + 0.5) * kx - 0.5;
            let mut a = sample(&edge_at, fx, fy);
            if mask.is_some() {
                let m = sample(&mask_at, fx, fy);
                a = a.max(1.0 - ((m - 0.5).abs() * 6.0).min(1.0));
            }
            let a = a * KEEP_ALPHA;
            if a > 0.0 {
                for c in 0..3 {
                    p[c] = (p[c] as f32 * (1.0 - a) + ink[c] as f32 * a).round() as u8;
                }
            }
        }
    }
}

// ── Playback ─────────────────────────────────────────────────────────────

/// One image the player uploads: its atlas identity, and its pixels until
/// the first frame put them on the GPU.
struct Upload {
    id: ImageId,
    size: gpui::Size<DevicePixels>,
    bytes: Option<Vec<u8>>,
}

impl Upload {
    fn new(image: Image) -> Self {
        // A fresh id from GPUI's own image counter; the empty image is dropped.
        let id = gpui::RenderImage::new(smallvec::SmallVec::<[image::Frame; 1]>::new()).id;
        Self { id, size: size(DevicePixels(image.size.0 as i32), DevicePixels(image.size.1 as i32)), bytes: Some(image.bgra) }
    }

    fn texture(&self) -> TransitionImage<'_> {
        TransitionImage { id: self.id, size: self.size, bytes: self.bytes.as_deref() }
    }
}

/// How a played transition stands.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Not painted yet.
    Waiting,
    Playing,
    /// The last frame (progress 1) was painted.
    Done,
    /// The GPU lost an image (device loss): end it now.
    Lost,
}

/// Plays one transition: owns its images, frees their CPU copies after
/// the first frame and their textures when [`Self::release`] runs (or
/// with the window).
pub struct Player {
    look: Look,
    from: Upload,
    to: Upload,
    aux: Option<Upload>,
    duration: Duration,
    start: Option<Instant>,
    state: State,
}

/// A shared player, for the element that paints it.
pub type Shared = Rc<RefCell<Player>>;

impl Player {
    pub fn new(look: Look, frames: Frames, duration: Duration) -> Self {
        let subject = look.subject && frames.aux.is_some();
        Self {
            look: Look { subject, ..look },
            from: Upload::new(frames.from),
            to: Upload::new(frames.to),
            aux: frames.aux.map(Upload::new),
            duration: duration.max(MIN_DURATION),
            start: None,
            state: State::Waiting,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Linear progress at `now` (0 until the first frame).
    pub fn progress(&self, now: Instant) -> f32 {
        self.start.map_or(0.0, |s| (now.saturating_duration_since(s).as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0))
    }

    /// Bytes still held on the CPU (0 once uploaded).
    pub fn cpu_bytes(&self) -> usize {
        [Some(&self.from), Some(&self.to), self.aux.as_ref()].into_iter().flatten().filter_map(|u| u.bytes.as_ref()).map(Vec::capacity).sum()
    }

    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window) {
        if matches!(self.state, State::Lost) {
            return;
        }
        let now = Instant::now();
        let start = *self.start.get_or_insert(now);
        let progress = self.progress(now);
        let params = self.look.params(progress, now.saturating_duration_since(start).as_secs_f32());
        let uploading = self.cpu_bytes();
        let painted = window.paint_transition(bounds, self.from.texture(), self.to.texture(), self.aux.as_ref().map(Upload::texture), params);
        if uploading > 0 {
            tracing::info!("wallpaper transition: {:.1} MiB uploaded in {:.1} ms", uploading as f64 / 1_048_576.0, now.elapsed().as_secs_f64() * 1000.0);
        }
        match painted {
            Ok(true) => {
                // The atlas copied them to the GPU: no CPU copy stays.
                for u in [Some(&mut self.from), Some(&mut self.to), self.aux.as_mut()].into_iter().flatten() {
                    u.bytes = None;
                }
                self.state = if progress >= 1.0 { State::Done } else { State::Playing };
            }
            Ok(false) => self.state = State::Lost,
            Err(err) => {
                tracing::warn!("wallpaper transition: {err:#}");
                self.state = State::Lost;
            }
        }
    }

    /// Keeps only the incoming image: the outgoing one and the helper image
    /// leave the atlas, and every later frame shows the end state (the GPU
    /// wallpaper keeping Sketch's outline).
    pub fn settle(&mut self, window: &mut Window) {
        if self.from.id == self.to.id {
            return;
        }
        for u in [Some(&mut self.from), self.aux.as_mut()].into_iter().flatten() {
            u.bytes = None;
            window.drop_transition_image(u.id);
        }
        self.from = Upload { id: self.to.id, size: self.to.size, bytes: None };
        self.aux = None;
        self.look.subject = false;
    }

    /// Removes the images from the window's atlas (their textures go with
    /// them) and drops anything still held.
    pub fn release(&mut self, window: &mut Window) {
        for u in [Some(&mut self.from), Some(&mut self.to), self.aux.as_mut()].into_iter().flatten() {
            u.bytes = None;
            window.drop_transition_image(u.id);
        }
    }
}

/// The element painting a [`Player`] over its parent (absolute, full size).
pub struct TransitionElement(pub Shared);

impl IntoElement for TransitionElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for TransitionElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style { position: Position::Absolute, ..Style::default() };
        style.inset.top = gpui::px(0.0).into();
        style.inset.left = gpui::px(0.0).into();
        style.size.width = gpui::relative(1.0).into();
        style.size.height = gpui::relative(1.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), _: &mut Window, _: &mut App) {}

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window, _: &mut App) {
        self.0.borrow_mut().paint(bounds, window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(kind: WallpaperTransition) -> Look {
        Look {
            kind,
            origin: [0.5, 0.5],
            focus: [0.25, 0.75],
            seed: 3,
            intensity: 1.0,
            logical: (2560.0, 1440.0),
            scale: 1.5,
            rtl: false,
            accent: Color::hex("#2ec4b6"),
            dominant: Color::hex("#d08040"),
            sketch: SketchOptions::default(),
            subject: true,
        }
    }

    #[test]
    fn every_animated_transition_has_a_shader_code() {
        let codes: Vec<_> = WallpaperTransition::ALL.iter().filter_map(|k| code(*k)).collect();
        assert_eq!(codes.len(), WallpaperTransition::ALL.len() - 1, "only None draws nothing");
        let mut sorted = codes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), codes.len(), "codes are distinct");
        assert!(codes.iter().all(|c| *c <= gpui::TRANSITION_KIND_SKETCH));
        assert_eq!(code(WallpaperTransition::Tear), Some(gpui::TRANSITION_KIND_TEAR));
        assert_eq!(code(WallpaperTransition::Sketch), Some(gpui::TRANSITION_KIND_SKETCH));
    }

    #[test]
    fn the_shader_draws_everything_the_compositor_cannot() {
        use WallpaperTransition as T;
        for kind in [T::Grow, T::Ripple, T::Tide, T::Tear, T::Glitch, T::Shatter, T::Dissolve, T::Sketch] {
            assert!(uses_shader(kind), "{kind:?}");
        }
        for kind in [T::Fade, T::Wipe, T::None] {
            assert!(!uses_shader(kind), "{kind:?}");
        }
        assert_eq!(effective(T::Shatter, true), T::Fade, "reduced motion fades");
        assert_eq!(effective(T::None, true), T::None);
        assert_eq!(effective(T::Tear, false), T::Tear);
    }

    #[test]
    fn shards_follow_the_screen_and_intensity() {
        let count = |g: [u32; 2]| g[0] * g[1];
        let big = shatter_grid((2560.0, 1440.0), 1.0);
        assert!((90..=110).contains(&count(big)), "{big:?}");
        let small = shatter_grid((1280.0, 720.0), 1.0);
        assert!(count(small) < count(big) && count(small) >= 40, "{small:?}");
        let calm = shatter_grid((2560.0, 1440.0), 0.0);
        assert!(count(calm) < count(big) && count(calm) >= 40, "{calm:?}");
        let huge = shatter_grid((7680.0, 4320.0), 1.0);
        assert!(count(huge) <= 120 + 8, "{huge:?}");
        // Cells about as wide as tall.
        let cell = (2560.0 / big[0] as f32, 1440.0 / big[1] as f32);
        assert!((cell.0 / cell.1 - 1.0).abs() < 0.25, "{cell:?}");
        let tall = shatter_grid((1080.0, 1920.0), 1.0);
        assert!(tall[1] > tall[0]);
        assert!(count(shatter_grid((0.0, 0.0), 1.0)) >= 1);
    }

    #[test]
    fn params_carry_the_look() {
        let p = look(WallpaperTransition::Sketch).params(1.4, 2.0);
        assert_eq!(p.kind, gpui::TRANSITION_KIND_SKETCH);
        assert_eq!(p.progress, 1.0, "clamped");
        assert_eq!(p.line_width, SketchOptions::default().line_px() * 1.5);
        assert_ne!(p.flags & gpui::TRANSITION_FLAG_SUBJECT, 0);
        assert_ne!(p.flags & gpui::TRANSITION_FLAG_GLOW, 0, "glow is on by default");
        assert_eq!(p.flags & gpui::TRANSITION_FLAG_RTL, 0);
        assert_eq!((p.fill_style, p.draw_order), (1, 0), "bleed, subject first");
        assert_eq!(p.wash[3], WASH_ALPHA);
        let rtl = Look { rtl: true, ..look(WallpaperTransition::Wipe) }.params(0.5, 0.5);
        assert_ne!(rtl.flags & gpui::TRANSITION_FLAG_RTL, 0);
        let shatter = look(WallpaperTransition::Shatter).params(0.5, 0.5);
        assert_eq!(shatter.grid, shatter_grid((2560.0, 1440.0), 1.0));
        // Dissolve burns in the accent.
        let d = look(WallpaperTransition::Dissolve).params(0.5, 0.5);
        assert!(d.glow[1] > d.glow[0], "the teal accent: {:?}", d.glow);
        assert_eq!(fraction(Some((640.0, 360.0)), (1280.0, 720.0)), [0.5, 0.5]);
        assert_eq!(fraction(Some((320.0, 180.0)), (1280.0, 720.0)), [0.25, 0.25]);
        assert_eq!(fraction(Some((-5.0, 180.0)), (1280.0, 720.0)), [0.5, 0.5], "off the output: the centre");
        assert_eq!(fraction(None, (1280.0, 720.0)), [0.5, 0.5]);
    }

    #[test]
    fn sketch_ink_and_wash() {
        let accent = Color::hex("#2ec4b6");
        let (line, wash) = sketch_colours(SketchInk::Ink, accent, accent);
        assert!(line.lightness() < DARK_LINE && wash.lightness() > 0.9, "dark ink on paper");
        let (line, wash) = sketch_colours(SketchInk::White, accent, accent);
        assert!(line.lightness() > 0.9 && wash.lightness() < 0.2, "white on night");
        let grey = image::RgbImage::from_pixel(8, 8, image::Rgb([128, 128, 128]));
        assert!(dominant(&grey).chroma() < 0.01, "a grey picture has no hue");
        let teal = image::RgbImage::from_pixel(8, 8, image::Rgb([20, 150, 140]));
        assert!(dominant(&teal).lightness() >= INK_LIGHTNESS - 0.01);
    }

    #[test]
    fn helper_image_marks_lines_and_the_subject() {
        // A 64 × 32 picture: dark left half, light right half (one vertical
        // edge at x = 32); the subject is a square in the left half.
        let (w, h) = (64u32, 32u32);
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..h {
            for x in 0..w {
                let v = if x < 32 { 20 } else { 230 };
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let before = rgba.clone();
        let (aw, ah) = aux_size((w, h));
        assert_eq!((aw, ah), (32, 16));
        let mask = GrayImage::from_fn(aw, ah, |x, y| image::Luma([if (4..10).contains(&x) && (4..10).contains(&y) { 255 } else { 0 }]));
        let aux = build_aux(&mut rgba, (w, h), Some(&mask), None);
        assert_eq!(rgba, before, "without keep, the picture is untouched");
        assert_eq!(aux.size, (32, 16));
        let px = |x: u32, y: u32| &aux.bgra[((y * aw + x) * 4) as usize..][..4];
        // BGRA: blue = line distance, green = subject distance, red = mask.
        assert_eq!(px(16, 8)[0], 0, "on the edge");
        assert!(px(30, 8)[0] > px(20, 8)[0], "farther from the edge");
        assert_eq!(px(6, 6)[2], 255, "inside the subject");
        assert_eq!(px(6, 6)[1], 0, "no distance inside");
        assert!(px(28, 14)[1] > px(12, 6)[1], "distance grows away from the subject");
        assert!(aux.bgra.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
        // Without a mask: no subject anywhere.
        let mut plain = before.clone();
        let aux = build_aux(&mut plain, (w, h), None, None);
        assert!(aux.bgra.as_chunks::<4>().0.iter().all(|p| p[2] == 0 && p[1] == 255));
        // With keep, the edge is drawn faintly into the picture, nothing else.
        let mut kept = before.clone();
        build_aux(&mut kept, (w, h), None, Some([255, 0, 0]));
        let at = |buf: &[u8], x: u32, y: u32| buf[((y * w + x) * 4) as usize];
        assert!(at(&kept, 32, 16) > at(&before, 32, 16) || at(&kept, 31, 16) > at(&before, 31, 16), "the edge is tinted red");
        assert_eq!(at(&kept, 4, 16), at(&before, 4, 16), "flat areas are untouched");
    }

    #[test]
    fn the_helper_is_half_size_and_bounded() {
        assert_eq!(aux_size((3840, 2160)), (1920, 1080));
        assert_eq!(aux_size((7680, 4320)), (2048, 1152));
        assert_eq!(aux_size((1, 1)), (1, 1));
    }

    #[test]
    fn images_swizzle_in_place() {
        let img = Image::from_rgba((1, 2), vec![1, 2, 3, 255, 4, 5, 6, 255]);
        assert_eq!(img.bgra, vec![3, 2, 1, 255, 6, 5, 4, 255]);
    }

    #[test]
    fn players_count_their_cpu_copies() {
        let frame = |n: usize| Image { size: (2, 2), bgra: vec![0; n] };
        let p = Player::new(look(WallpaperTransition::Tear), Frames { from: frame(16), to: frame(16), aux: None }, Duration::from_secs(1));
        assert_eq!(p.cpu_bytes(), 32);
        assert_eq!(p.state(), State::Waiting);
        assert_eq!(p.progress(Instant::now()), 0.0, "nothing plays before the first frame");
        assert!(!p.look.subject, "no helper image, no subject");
    }
}
