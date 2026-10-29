//! Pixels of the desktop background, shared by the live software renderer
//! ([`super`]) and the offscreen preview (`mandara msg wallpaper
//! render-preview`), so the preview is exactly what the desktop shows.
//!
//! Everything is in *physical* pixels of the output. The wallpaper is
//! scaled once to the output with its fill mode; the clock layer is split
//! into small planes (halo, digits, date pill, audio ring) and the depth
//! subject is cropped to where the clock layer can be under it, so a
//! per-minute redraw only touches the digits.

use std::path::Path;

use image::{DynamicImage, GrayImage, RgbImage, RgbaImage};
use mandara_config::desktop::WallpaperFill;
use mandara_config::{ClockStyle, WallpaperConfig};
use mandara_core::shm_canvas::{Rect, SCALE_DENOMINATOR, snap};
use mandara_theme::Color;
use tiny_skia::{FillRule, FilterQuality, Paint, PathBuilder, Pixmap, PixmapMut, PixmapPaint, Transform};

use crate::desktop::{self, ClockLayout, DateLine, Ink};
use crate::lock::paint::{self as lp, Font, TextPainter};

/// Side of the downscaled copy used to pick the clock's ink.
pub const TONE_MAP_PX: u32 = 160;
/// The halo is a wide Gaussian glow: its plane is drawn this many times
/// smaller and scaled up by the compositor (`wp_viewporter`).
pub const HALO_DOWNSCALE: u32 = 8;
/// Halo geometry, as fractions of the digit size / clock block height
/// (the GPUI layer's `BoxShadow`: blur radius = Gaussian σ).
const HALO_HALF_W_EM: f32 = 1.2;
const HALO_HALF_H_BLOCK: f32 = 0.42;
const HALO_SIGMA_EM: f32 = 0.9;
/// How far (in σ) the Gaussian tail is drawn, and how far it can still be
/// seen under a depth subject (beyond ~2σ the glow is below 1/255).
const HALO_DRAW_SIGMAS: f32 = 3.0;
const HALO_VISIBLE_SIGMAS: f32 = 2.0;
/// Gaussian-integral samples per pixel (as GPUI's shadow shader).
const HALO_SAMPLES: usize = 4;
/// Slack around the digits' advance box for glyph overhang, in em.
const DIGIT_PAD_EM: f32 = 0.15;
const DIGIT_PAD_V_EM: f32 = 0.25;
/// Date pill metrics in logical px at k = 1 (as the GPUI pill).
const PILL_H: f32 = 34.0;
const PILL_PAD: f32 = 16.0;
const PILL_GAP: f32 = 10.0;
const PILL_TEXT: f32 = 14.0;
const PILL_HIJRI_GAP: f32 = 5.0;
const PILL_WEATHER_GAP: f32 = 6.0;
const PILL_ICON: f32 = 15.0;
/// Separator dot diameter and pill border, logical px (not scaled by k).
const PILL_DOT: f32 = 3.0;
const PILL_BORDER: f32 = 1.0;
const PILL_WEIGHT: u16 = 500;
const PILL_HIJRI_WEIGHT: u16 = 600;
const PILL_LABEL_OPACITY: f32 = 0.75;
/// Opacity of the pill's border and separator dots (of the ink colour).
const PILL_BORDER_ALPHA: f32 = 0.14;
const PILL_DOT_ALPHA: f32 = 0.45;
/// Audio ring: segments, stroke widths (logical px), amplitude (fraction
/// of the radius) and opacities of the resting circle and the spectrum.
const RING_STEPS: usize = 144;
const RING_BASE_WIDTH: f32 = 1.0;
const RING_WAVE_WIDTH: f32 = 2.0;
pub const RING_AMP: f32 = 0.09;
const RING_BASE_ALPHA: f32 = 0.16;
const RING_WAVE_ALPHA: f32 = 0.5;
/// Anti-aliasing margin around stroked shapes, logical px.
const AA_MARGIN: f32 = 2.0;
/// Viewbox of the bundled icons.
const ICON_VIEWBOX: f32 = 24.0;

// ── Background ───────────────────────────────────────────────────────────

/// A wallpaper scaled to the output, with what the clock needs from it.
pub struct Background {
    /// Opaque premultiplied RGBA at the output's physical size.
    pub pixmap: Pixmap,
    /// Small copy for picking the clock's ink.
    pub tone: RgbImage,
}

/// Places `img` on a `w × h` canvas for `fill` (bands in `fill_color`).
pub fn background(img: &DynamicImage, fill: WallpaperFill, (w, h): (u32, u32), fill_color: Color) -> Result<Background, String> {
    use image::imageops::FilterType::Triangle;
    let mut pm = Pixmap::new(w, h).ok_or("bad canvas size")?;
    let [r, g, b, _] = fill_color.to_rgba8();
    pm.fill(tiny_skia::Color::from_rgba8(r, g, b, u8::MAX));
    let (iw, ih) = (img.width().max(1), img.height().max(1));
    let centered = |cw: u32, ch: u32| ((w as i32 - cw as i32) / 2, (h as i32 - ch as i32) / 2);
    match fill {
        // The cached display copy usually has the output's size already.
        WallpaperFill::Cover | WallpaperFill::Stretch if (iw, ih) == (w, h) => over(&mut pm, &img.to_rgba8(), 0, 0),
        WallpaperFill::Cover => over(&mut pm, &crate::wallpaper::cover(img, w, h).to_rgba8(), 0, 0),
        WallpaperFill::Stretch => over(&mut pm, &img.resize_exact(w, h, Triangle).to_rgba8(), 0, 0),
        WallpaperFill::Contain => {
            let s = (w as f32 / iw as f32).min(h as f32 / ih as f32);
            let (fw, fh) = (((iw as f32 * s).round() as u32).max(1), ((ih as f32 * s).round() as u32).max(1));
            let fit = if (fw, fh) == (iw, ih) { img.to_rgba8() } else { img.resize_exact(fw, fh, Triangle).to_rgba8() };
            let (x, y) = centered(fw, fh);
            over(&mut pm, &fit, x, y);
        }
        WallpaperFill::Center => {
            let (x, y) = centered(iw, ih);
            over(&mut pm, &img.to_rgba8(), x, y);
        }
        WallpaperFill::Tile => {
            let (cols, rows, k) = crate::wallpaper_fill::tile_grid(w as f32, h as f32, iw as f32, ih as f32);
            let (tw, th) = (((iw as f32 * k).round() as u32).max(1), ((ih as f32 * k).round() as u32).max(1));
            let tile = if (tw, th) == (iw, ih) { img.to_rgba8() } else { img.resize_exact(tw, th, Triangle).to_rgba8() };
            for row in 0..rows {
                for col in 0..cols {
                    over(&mut pm, &tile, (col * tw) as i32, (row * th) as i32);
                }
            }
        }
    }
    let tone = tone_map(&pm);
    Ok(Background { pixmap: pm, tone })
}

/// Straight RGBA `src` over the opaque `dst` at `(x, y)`, clipped.
fn over(dst: &mut Pixmap, src: &RgbaImage, x: i32, y: i32) {
    let (dw, dh) = (dst.width() as i32, dst.height() as i32);
    let (sw, sh) = (src.width() as i32, src.height() as i32);
    let (x0, y0, x1, y1) = (x.max(0), y.max(0), (x + sw).min(dw), (y + sh).min(dh));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let data = dst.data_mut();
    let raw = src.as_raw();
    for dy in y0..y1 {
        let sy = dy - y;
        let s_row = &raw[((sy * sw + (x0 - x)) * 4) as usize..((sy * sw + (x1 - x)) * 4) as usize];
        let d_row = &mut data[((dy * dw + x0) * 4) as usize..((dy * dw + x1) * 4) as usize];
        for (d, s) in d_row.chunks_exact_mut(4).zip(s_row.chunks_exact(4)) {
            match s[3] {
                u8::MAX => d.copy_from_slice(&[s[0], s[1], s[2], u8::MAX]),
                0 => {}
                a => {
                    let (a, inv) = (a as u32, 255 - a as u32);
                    for c in 0..3 {
                        d[c] = ((s[c] as u32 * a + d[c] as u32 * inv + 127) / 255) as u8;
                    }
                }
            }
        }
    }
}

/// Box-averaged copy at most [`TONE_MAP_PX`] on its long side.
fn tone_map(pm: &Pixmap) -> RgbImage {
    let (w, h) = (pm.width().max(1), pm.height().max(1));
    let k = (TONE_MAP_PX as f32 / w.max(h) as f32).min(1.0);
    let (tw, th) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
    let data = pm.data();
    RgbImage::from_fn(tw, th, |tx, ty| {
        let (x0, x1) = (tx * w / tw, ((tx + 1) * w / tw).max(tx * w / tw + 1).min(w));
        let (y0, y1) = (ty * h / th, ((ty + 1) * h / th).max(ty * h / th + 1).min(h));
        let mut sum = [0u64; 3];
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y * w + x) * 4) as usize;
                for c in 0..3 {
                    sum[c] += data[i + c] as u64;
                }
            }
        }
        let n = ((x1 - x0) * (y1 - y0)).max(1) as u64;
        image::Rgb([(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8])
    })
}

// ── Depth subject (layer 3) ──────────────────────────────────────────────

/// The subject's pixels (wallpaper × mask alpha, premultiplied) inside its
/// bounding box, and a small map of the aligned alpha for coverage queries.
pub struct CutSource {
    pub rect: Rect,
    /// `None` once painted: the live canvas releases the pixels (a 4K
    /// subject is ~25 MiB) and cuts them out again only for a repaint.
    pub pixmap: Option<Pixmap>,
    pub map: GrayImage,
}

impl CutSource {
    /// The same subject without its pixels (geometry and coverage map only).
    pub fn released(&self) -> Self {
        Self { rect: self.rect, pixmap: None, map: self.map.clone() }
    }
}

/// Cuts the subject described by `mask` out of the scaled background.
pub fn cut_source(bg: &Pixmap, mask: &Path) -> Result<Option<CutSource>, String> {
    // The background is a cover crop of the wallpaper; so is the mask.
    let alpha = crate::depth::align(&crate::depth::load_alpha(mask)?, (bg.width(), bg.height()), true);
    Ok(cut_from_alpha(bg, &alpha))
}

/// Cuts the subject out of the scaled background with `alpha` (the mask
/// at the background's size).
pub fn cut_from_alpha(bg: &Pixmap, alpha: &GrayImage) -> Option<CutSource> {
    let (w, h) = (bg.width(), bg.height());
    let map_w = crate::depth::COVERAGE_MAP_W;
    let map = image::imageops::thumbnail(alpha, map_w, (map_w * h / w.max(1)).max(1));
    let (x, y, cw, ch) = crate::depth::alpha_bounds(alpha)?;
    let mut pm = Pixmap::new(cw, ch)?;
    let src = bg.data();
    let a_raw = alpha.as_raw();
    let dst = pm.data_mut();
    for row in 0..ch {
        for col in 0..cw {
            let (sx, sy) = (x + col, y + row);
            let a = a_raw[(sy * w + sx) as usize] as u32;
            if a == 0 {
                continue;
            }
            let si = ((sy * w + sx) * 4) as usize;
            let di = ((row * cw + col) * 4) as usize;
            for c in 0..4 {
                dst[di + c] = ((src[si + c] as u32 * a + 127) / 255) as u8;
            }
        }
    }
    Some(CutSource { rect: Rect::new(x as i32, y as i32, cw, ch), pixmap: Some(pm), map })
}

// ── Clock layer geometry ────────────────────────────────────────────────

/// Font family names (resolved against what is installed).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct FontNames {
    pub ui: String,
    pub display: String,
    pub arabic: String,
    /// Monospace (desktop widgets).
    pub mono: String,
}

impl From<&crate::fonts::Fonts> for FontNames {
    fn from(f: &crate::fonts::Fonts) -> Self {
        Self { ui: f.ui.to_string(), display: f.display.to_string(), arabic: f.arabic.to_string(), mono: f.mono.to_string() }
    }
}

/// What the clock layer shows on one output.
pub struct DecorInput<'a> {
    /// Output size in logical px and its scale ×120.
    pub logical: (f32, f32),
    pub scale120: u32,
    pub cfg: &'a WallpaperConfig,
    pub lines: &'a [String],
    pub date: Option<&'a DateLine>,
    pub ink: Ink,
    pub fonts: &'a FontNames,
    /// Edges the clock keeps clear of (the bar), logical px.
    pub safe: mandara_config::desktop_widgets::Insets,
}

impl DecorInput<'_> {
    pub fn scale(&self) -> f32 {
        self.scale120 as f32 / SCALE_DENOMINATOR as f32
    }

    pub fn layout(&self) -> ClockLayout {
        desktop::layout_in(self.logical.0, self.logical.1, self.cfg, self.safe)
    }
}

/// One element of the date pill.
#[derive(Clone, Debug, PartialEq)]
enum PillItem {
    Text { text: String, arabic: bool, opacity: f32, w: f32 },
    Dot,
    Icon(&'static str),
    Gap(f32),
}

/// Where each clock-layer plane goes (physical px, snapped to the grid).
#[derive(Clone, Debug, PartialEq)]
pub struct DecorGeometry {
    pub layout: ClockLayout,
    pub scale: f32,
    pub output: Rect,
    /// Halo plane: area and its (smaller) buffer size.
    pub halo: Rect,
    pub halo_buffer: (u32, u32),
    /// Where the halo is still visible (for the depth cut-out).
    pub halo_visible: Rect,
    pub digits: Rect,
    /// Each line's text origin (top-left, physical, absolute).
    lines: Vec<(f32, f32)>,
    pub pill: Option<Rect>,
    pill_items: Vec<PillItem>,
    pill_box: (f32, f32, f32, f32),
    pub ring: Rect,
}

/// Unpatched GPUI drew a variable font at its default instance whatever
/// weight was asked for; Mandara's vendored text system applies the `wght`
/// axis like cosmic-text does, so the desktop clock uses the requested
/// weights. `true` mimics the old GPUI look (variable faces at their
/// default weight, e.g. regular rather than bold stacked digits).
const MATCH_GPUI_VARIABLE_WEIGHTS: bool = false;

/// Faces already inspected: `(family, weight) → default weight` of a
/// variable face (`None` for a static one).
static VARIABLE_FACES: std::sync::Mutex<Vec<((String, u16), Option<u16>)>> = std::sync::Mutex::new(Vec::new());

/// Default `wght` of the face `family` / `weight` resolves to, when that
/// face is variable.
fn variable_default(text: &mut TextPainter, family: &str, weight: u16) -> Option<u16> {
    let key = (family.to_string(), weight);
    if let Ok(cache) = VARIABLE_FACES.lock()
        && let Some((_, v)) = cache.iter().find(|(k, _)| *k == key)
    {
        return *v;
    }
    let db = text.font_system().db();
    let families = [cosmic_text::fontdb::Family::Name(family)];
    let found = db
        .query(&cosmic_text::fontdb::Query { families: &families, weight: cosmic_text::fontdb::Weight(weight), ..Default::default() })
        .and_then(|id| db.with_face_data(id, sfnt_default_weight).flatten());
    if let Ok(mut cache) = VARIABLE_FACES.lock() {
        cache.push((key, found));
    }
    found
}

/// The weight GPUI would draw `family` at.
fn shown_weight(text: &mut TextPainter, family: &str, weight: u16) -> u16 {
    if MATCH_GPUI_VARIABLE_WEIGHTS { variable_default(text, family, weight).unwrap_or(weight) } else { weight }
}

/// Default of the `wght` axis of an sfnt face (`fvar`), if it has one.
fn sfnt_default_weight(data: &[u8], index: u32) -> Option<u16> {
    let be16 = |o: usize| data.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize);
    let be32 = |o: usize| data.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
    // A collection (`ttcf`) lists each face's offset table.
    let base = if data.get(0..4) == Some(b"ttcf") { be32(12 + 4 * index as usize)? } else { 0 };
    let tables = be16(base + 4)?;
    let fvar = (0..tables).find_map(|i| {
        let rec = base + 12 + 16 * i;
        (data.get(rec..rec + 4) == Some(b"fvar")).then(|| be32(rec + 8)).flatten()
    })?;
    let (axes, count, size) = (be16(fvar + 4)?, be16(fvar + 8)?, be16(fvar + 10)?);
    (0..count).find_map(|i| {
        let a = fvar + axes + i * size;
        // Tag, then min / default / max as 16.16 fixed.
        (data.get(a..a + 4) == Some(b"wght")).then(|| be32(a + 8).map(|v| (v >> 16) as u16)).flatten()
    })
}

fn digit_font<'a>(style: ClockStyle, fonts: &'a FontNames, size: f32, text: &mut TextPainter) -> Font<'a> {
    let (family, weight) = match style {
        ClockStyle::Stacked => (fonts.display.as_str(), 700),
        ClockStyle::Classic => (fonts.display.as_str(), 300),
        ClockStyle::Arabic => (fonts.arabic.as_str(), 600),
    };
    Font { family, size, weight: shown_weight(text, family, weight), tabular: true, tracking: 0.0 }
}

fn pill_font<'a>(fonts: &'a FontNames, arabic: bool, size: f32, text: &mut TextPainter) -> Font<'a> {
    let (family, weight) = if arabic { (fonts.arabic.as_str(), PILL_HIJRI_WEIGHT) } else { (fonts.ui.as_str(), PILL_WEIGHT) };
    Font { family, size, weight: shown_weight(text, family, weight), tabular: true, tracking: 0.0 }
}

impl DecorGeometry {
    pub fn new(input: &DecorInput<'_>, text: &mut TextPainter) -> Self {
        let s = input.scale();
        let l = input.layout();
        let g = input.scale120;
        let output = Rect::new(0, 0, (input.logical.0 * s).round() as u32, (input.logical.1 * s).round() as u32);
        let (cx, cy) = (l.center.0 * s, l.center.1 * s);
        let px = |v: f32| v * s;

        // Halo: GPUI's blurred rounded box around the digits.
        let (hw, hh, sigma) = (px(l.size * HALO_HALF_W_EM), px(l.block_h * HALO_HALF_H_BLOCK), px(l.size * HALO_SIGMA_EM));
        let around = |sigmas: f32| snap(Rect::from_edges(cx - hw - sigmas * sigma, cy - hh - sigmas * sigma, cx + hw + sigmas * sigma, cy + hh + sigmas * sigma), g).intersect(&output);
        let halo = around(HALO_DRAW_SIGMAS);
        let halo_visible = around(HALO_VISIBLE_SIGMAS);
        let halo_buffer = (halo.w.div_ceil(HALO_DOWNSCALE).max(1), halo.h.div_ceil(HALO_DOWNSCALE).max(1));

        // Digits, one centred line each.
        let font = digit_font(input.cfg.clock_style, input.fonts, px(l.size), text);
        let block_top = cy - px(l.block_h) / 2.0;
        let mut lines = Vec::new();
        let mut max_w: f32 = 0.0;
        for (i, line) in input.lines.iter().enumerate() {
            let (w, lh) = text.measure(line, &font);
            let top = block_top + i as f32 * px(l.line_h) + (px(l.line_h) - lh) / 2.0;
            lines.push((cx - w / 2.0, top));
            max_w = max_w.max(w);
        }
        let (pad, vpad) = (px(l.size * DIGIT_PAD_EM), px(l.size * DIGIT_PAD_V_EM));
        let digits = snap(Rect::from_edges(cx - max_w / 2.0 - pad, block_top - vpad, cx + max_w / 2.0 + pad, block_top + px(l.block_h) + vpad), g).intersect(&output);

        // Date pill: measured items, centred under the clock.
        let mut pill_items = Vec::new();
        let mut pill = None;
        let mut pill_box = (0.0, 0.0, 0.0, 0.0);
        if let Some(d) = input.date {
            let k = l.k;
            let size = px(PILL_TEXT * k);
            let mut measure = |t: &str, arabic: bool| {
                let font = pill_font(input.fonts, arabic, size, text);
                text.measure(t, &font).0
            };
            let txt = |t: &str, arabic: bool, opacity: f32, m: &mut dyn FnMut(&str, bool) -> f32| PillItem::Text { text: t.to_string(), arabic, opacity, w: m(t, arabic) };
            pill_items.push(txt(&d.gregorian, false, 1.0, &mut measure));
            pill_items.push(PillItem::Gap(px(PILL_GAP * k)));
            pill_items.push(PillItem::Dot);
            pill_items.push(PillItem::Gap(px(PILL_GAP * k)));
            // Right-to-left by hand (day · month · year read from the right).
            pill_items.push(txt(&d.hijri_ar.2, true, 1.0, &mut measure));
            pill_items.push(PillItem::Gap(px(PILL_HIJRI_GAP * k)));
            pill_items.push(txt(d.hijri_ar.1, true, 1.0, &mut measure));
            pill_items.push(PillItem::Gap(px(PILL_HIJRI_GAP * k)));
            pill_items.push(txt(&d.hijri_ar.0, true, 1.0, &mut measure));
            if let Some((icon, temp, label)) = &d.weather {
                pill_items.push(PillItem::Gap(px(PILL_GAP * k)));
                pill_items.push(PillItem::Dot);
                pill_items.push(PillItem::Gap(px(PILL_GAP * k)));
                pill_items.push(PillItem::Icon(icon));
                pill_items.push(PillItem::Gap(px(PILL_WEATHER_GAP * k)));
                pill_items.push(txt(temp, false, 1.0, &mut measure));
                pill_items.push(PillItem::Gap(px(PILL_WEATHER_GAP * k)));
                pill_items.push(txt(label, false, PILL_LABEL_OPACITY, &mut measure));
            }
            let inner: f32 = pill_items
                .iter()
                .map(|i| match i {
                    PillItem::Text { w, .. } => *w,
                    PillItem::Dot => px(PILL_DOT),
                    PillItem::Icon(_) => px(PILL_ICON * k),
                    PillItem::Gap(g) => *g,
                })
                .sum();
            let (w, h) = (inner + 2.0 * px(PILL_PAD * k), px(PILL_H * k));
            pill_box = (cx - w / 2.0, px(l.pill_top), w, h);
            let r = snap(Rect::from_edges(pill_box.0 - px(AA_MARGIN), pill_box.1 - px(AA_MARGIN), pill_box.0 + w + px(AA_MARGIN), pill_box.1 + h + px(AA_MARGIN)), g).intersect(&output);
            pill = (!r.is_empty()).then_some(r);
        }

        // Audio ring: the breathing circle at full amplitude.
        let reach = px(l.ring_r * (1.0 + RING_AMP) + RING_WAVE_WIDTH + AA_MARGIN);
        let ring = snap(Rect::from_edges(cx - reach, cy - reach, cx + reach, cy + reach), g).intersect(&output);

        Self { layout: l, scale: s, output, halo, halo_buffer, halo_visible, digits, lines, pill, pill_items, pill_box, ring }
    }

    /// The pill's box as fractions of the output (coverage queries).
    pub fn pill_fraction(&self) -> (f32, f32, f32, f32) {
        let (w, h) = (self.output.w.max(1) as f32, self.output.h.max(1) as f32);
        let b = self.pill_box;
        (b.0 / w, b.1 / h, (b.0 + b.2) / w, (b.1 + b.3) / h)
    }

    /// The digits' box as fractions of the output (coverage queries).
    pub fn digits_fraction(&self, style: ClockStyle) -> (f32, f32, f32, f32) {
        let (lw, lh) = (self.output.w.max(1) as f32 / self.scale, self.output.h.max(1) as f32 / self.scale);
        let b = desktop::digit_box(&self.layout, style);
        (b.0 / lw, b.1 / lh, b.2 / lw, b.3 / lh)
    }

    /// Everything the clock layer can draw over (where a depth subject must
    /// be redrawn on top): halo, digits, the pill when it sits below the
    /// subject, and the ring when it is enabled.
    pub fn reach(&self, pill_below: bool, ring: bool) -> Rect {
        let mut r = self.halo_visible.union(&self.digits);
        if pill_below && let Some(p) = self.pill {
            r = r.union(&p);
        }
        if ring {
            r = r.union(&self.ring);
        }
        r
    }
}

// ── Clock layer painters ────────────────────────────────────────────────

fn gaussian(x: f32, sigma: f32) -> f32 {
    (-(x * x) / (2.0 * sigma * sigma)).exp() / ((2.0 * std::f32::consts::PI).sqrt() * sigma)
}

/// Abramowitz–Stegun style approximation (GPUI's shader).
fn erf(v: f32) -> f32 {
    let (s, a) = (v.signum(), v.abs());
    let r1 = 1.0 + (0.278_393 + (0.230_389 + (0.000_972 + 0.078_108 * a) * a) * a) * a;
    let r2 = r1 * r1;
    s - s / (r2 * r2)
}

fn blur_along_x(x: f32, y: f32, sigma: f32, corner: f32, half: (f32, f32)) -> f32 {
    let delta = (half.1 - corner - y.abs()).min(0.0);
    let curved = half.0 - corner + (corner * corner - delta * delta).max(0.0).sqrt();
    let k = std::f32::consts::FRAC_1_SQRT_2 / sigma;
    let lo = 0.5 + 0.5 * erf((x - curved) * k);
    let hi = 0.5 + 0.5 * erf((x + curved) * k);
    hi - lo
}

/// Coverage of a blurred rounded box (GPUI's `fs_shadow`) at `p` relative
/// to the box centre.
fn shadow_alpha(p: (f32, f32), half: (f32, f32), corner: f32, sigma: f32) -> f32 {
    let (low, high) = (p.1 - half.1, p.1 + half.1);
    let start = (-HALO_DRAW_SIGMAS * sigma).clamp(low, high);
    let end = (HALO_DRAW_SIGMAS * sigma).clamp(low, high);
    let step = (end - start) / HALO_SAMPLES as f32;
    let mut y = start + step * 0.5;
    let mut alpha = 0.0;
    for _ in 0..HALO_SAMPLES {
        alpha += blur_along_x(p.0, p.1 - y, sigma, corner, half) * gaussian(y, sigma) * step;
        y += step;
    }
    alpha
}

/// The legibility halo into its (downscaled) buffer.
pub fn paint_halo(pm: &mut PixmapMut<'_>, geo: &DecorGeometry, ink: &Ink) {
    let (s, l) = (geo.scale, &geo.layout);
    let (cx, cy) = (l.center.0 * s, l.center.1 * s);
    let half = (l.size * HALO_HALF_W_EM * s, l.block_h * HALO_HALF_H_BLOCK * s);
    let corner = (l.size * s).min(half.0).min(half.1);
    let sigma = l.size * HALO_SIGMA_EM * s;
    let (bw, bh) = (pm.width(), pm.height());
    let (fx, fy) = (geo.halo.w as f32 / bw as f32, geo.halo.h as f32 / bh as f32);
    let [r, g, b, a] = ink.halo.to_rgba8();
    let a = a as f32 / 255.0;
    let data = pm.data_mut();
    for y in 0..bh {
        let py = geo.halo.y as f32 + (y as f32 + 0.5) * fy - cy;
        for x in 0..bw {
            let px = geo.halo.x as f32 + (x as f32 + 0.5) * fx - cx;
            let cov = (shadow_alpha((px, py), half, corner, sigma) * a).clamp(0.0, 1.0);
            let i = ((y * bw + x) * 4) as usize;
            data[i] = (r as f32 * cov).round() as u8;
            data[i + 1] = (g as f32 * cov).round() as u8;
            data[i + 2] = (b as f32 * cov).round() as u8;
            data[i + 3] = (cov * 255.0).round() as u8;
        }
    }
}

/// The clock digits into the digits plane.
pub fn paint_digits(pm: &mut PixmapMut<'_>, text: &mut TextPainter, geo: &DecorGeometry, input: &DecorInput<'_>) {
    let font = digit_font(input.cfg.clock_style, input.fonts, geo.layout.size * geo.scale, text);
    let (ox, oy) = (geo.digits.x as f32, geo.digits.y as f32);
    for (line, (x, y)) in input.lines.iter().zip(&geo.lines) {
        text.draw(pm, line, &font, x - ox, y - oy, input.ink.fg, 1.0);
    }
}

/// The surface tone of the date pill.
fn pill_surface(ink: &Ink) -> Color {
    if ink.light { Color::oklch(0.1, 0.0, 0.0).with_alpha(0.2) } else { Color::oklch(1.0, 0.0, 0.0).with_alpha(0.38) }
}

/// A bundled icon at `size` px, tinted with `color`.
pub fn icon_pixmap(name: &str, size: u32, color: Color) -> Option<Pixmap> {
    let svg = crate::icons::svg_source(name)?;
    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).ok()?;
    let mut pm = Pixmap::new(size.max(1), size.max(1))?;
    let k = size as f32 / ICON_VIEWBOX;
    resvg::render(&tree, Transform::from_scale(k, k), &mut pm.as_mut());
    let [r, g, b, a] = color.to_rgba8();
    for px in pm.data_mut().chunks_exact_mut(4) {
        let cov = px[3] as u32 * a as u32 / 255;
        px[0] = (r as u32 * cov / 255) as u8;
        px[1] = (g as u32 * cov / 255) as u8;
        px[2] = (b as u32 * cov / 255) as u8;
        px[3] = cov as u8;
    }
    Some(pm)
}

/// The date pill (Gregorian · Hijri · weather) into its plane.
pub fn paint_pill(pm: &mut PixmapMut<'_>, text: &mut TextPainter, geo: &DecorGeometry, input: &DecorInput<'_>) {
    let Some(rect) = geo.pill else { return };
    let (s, k) = (geo.scale, geo.layout.k);
    let ink = &input.ink;
    let (ox, oy) = (rect.x as f32, rect.y as f32);
    let (x, y, w, h) = (geo.pill_box.0 - ox, geo.pill_box.1 - oy, geo.pill_box.2, geo.pill_box.3);
    lp::fill(pm, &lp::rounded_rect(x, y, w, h, h / 2.0), &lp::solid(pill_surface(ink), 1.0));
    let bw = PILL_BORDER * s;
    lp::stroke(pm, &lp::rounded_rect(x + bw / 2.0, y + bw / 2.0, w - bw, h - bw, (h - bw) / 2.0), &lp::solid(ink.fg.with_alpha(PILL_BORDER_ALPHA), 1.0), bw);
    let size = PILL_TEXT * k * s;
    let mid = y + h / 2.0;
    let mut cx = x + PILL_PAD * k * s;
    for item in &geo.pill_items {
        match item {
            PillItem::Text { text: t, arabic, opacity, w } => {
                let font = pill_font(input.fonts, *arabic, size, text);
                let (_, lh) = text.measure(t, &font);
                text.draw(pm, t, &font, cx, mid - lh / 2.0, ink.fg, *opacity);
                cx += w;
            }
            PillItem::Dot => {
                let d = PILL_DOT * s;
                lp::fill(pm, &lp::circle(cx + d / 2.0, mid, d / 2.0), &lp::solid(ink.fg.with_alpha(PILL_DOT_ALPHA), 1.0));
                cx += d;
            }
            PillItem::Icon(name) => {
                let side = PILL_ICON * k * s;
                if let Some(icon) = icon_pixmap(name, side.round() as u32, ink.fg) {
                    pm.draw_pixmap((cx).round() as i32, (mid - side / 2.0).round() as i32, icon.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
                }
                cx += side;
            }
            PillItem::Gap(g) => cx += g,
        }
    }
}

/// The audio ring (`bands` live levels, `ring` its fade `0..=1`).
pub fn paint_ring(pm: &mut PixmapMut<'_>, geo: &DecorGeometry, bands: &[f32], ring: f32, accent: Color) {
    let s = geo.scale;
    let l = &geo.layout;
    let (cx, cy) = (l.center.0 * s - geo.ring.x as f32, l.center.1 * s - geo.ring.y as f32);
    let r = l.ring_r * s;
    // Each line is filled as a band (outer contour, inner contour, even-odd)
    // instead of stroked: same pixels, a fraction of tiny-skia's stroker cost.
    let band = |r_of: &dyn Fn(f32) -> f32, width: f32| {
        let mut pb = PathBuilder::new();
        for side in [width / 2.0, -width / 2.0] {
            for i in 0..=RING_STEPS {
                let t = i as f32 / RING_STEPS as f32;
                let a = t * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                let rr = (r_of(t) + side).max(0.0);
                let (x, y) = (cx + rr * a.cos(), cy + rr * a.sin());
                if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
            }
            pb.close();
        }
        pb.finish()
    };
    let paint = |alpha: f32| -> Paint<'static> { lp::solid(accent.with_alpha(alpha), 1.0) };
    if let Some(p) = band(&|_| r, RING_BASE_WIDTH * s) {
        pm.fill_path(&p, &paint(RING_BASE_ALPHA * ring), FillRule::EvenOdd, Transform::identity(), None);
    }
    let amp = r * RING_AMP;
    if let Some(p) = band(&|t| desktop::ring_radius(r, bands, t, amp), RING_WAVE_WIDTH * s) {
        pm.fill_path(&p, &paint(RING_WAVE_ALPHA * ring), FillRule::EvenOdd, Transform::identity(), None);
    }
}

/// The part of the depth subject inside `rect` (absolute, physical), at
/// `opacity` (how much of the clock it may hide).
pub fn paint_cut(pm: &mut PixmapMut<'_>, cut: &CutSource, rect: Rect, opacity: f32) {
    let Some(pixels) = &cut.pixmap else { return };
    let paint = PixmapPaint { opacity: opacity.clamp(0.0, 1.0), ..PixmapPaint::default() };
    pm.draw_pixmap(cut.rect.x - rect.x, cut.rect.y - rect.y, pixels.as_ref(), &paint, Transform::identity(), None);
}

/// Draws a plane's pixels over a full-output pixmap the way the compositor
/// composes it (`dest` absolute; a smaller buffer is scaled bilinearly).
pub fn compose(dst: &mut PixmapMut<'_>, plane: &Pixmap, dest: Rect, opacity: f32) {
    let (sx, sy) = (dest.w as f32 / plane.width().max(1) as f32, dest.h as f32 / plane.height().max(1) as f32);
    let quality = if (sx - 1.0).abs() < f32::EPSILON && (sy - 1.0).abs() < f32::EPSILON { FilterQuality::Nearest } else { FilterQuality::Bilinear };
    let paint = PixmapPaint { opacity, quality, ..PixmapPaint::default() };
    dst.draw_pixmap(0, 0, plane.as_ref(), &paint, Transform::from_row(sx, 0.0, 0.0, sy, dest.x as f32, dest.y as f32), None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_place_the_image() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(40, 20, image::Rgba([200, 10, 10, 255])));
        let black = Color::hex("#000000");
        let cover = background(&img, WallpaperFill::Cover, (30, 30), black).unwrap();
        assert!(cover.pixmap.data().chunks_exact(4).all(|p| p[0] > 190 && p[3] == 255));
        let contain = background(&img, WallpaperFill::Contain, (40, 40), black).unwrap();
        let at = |pm: &Pixmap, x: u32, y: u32| pm.pixel(x, y).unwrap().red();
        assert_eq!(at(&contain.pixmap, 20, 2), 0, "band above");
        assert!(at(&contain.pixmap, 20, 20) > 190, "image in the middle");
        let center = background(&img, WallpaperFill::Center, (60, 60), black).unwrap();
        assert_eq!(at(&center.pixmap, 5, 30), 0);
        assert!(at(&center.pixmap, 30, 30) > 190);
        let tile = background(&img, WallpaperFill::Tile, (100, 50), black).unwrap();
        assert!(at(&tile.pixmap, 95, 45) > 190, "tiles reach the far corner");
        assert!(cover.tone.width() <= TONE_MAP_PX && cover.tone.get_pixel(0, 0).0[0] > 190);
    }

    #[test]
    fn translucent_images_blend_over_the_fill() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(10, 10, image::Rgba([255, 255, 255, 128])));
        let bg = background(&img, WallpaperFill::Stretch, (10, 10), Color::hex("#000000")).unwrap();
        let p = bg.pixmap.pixel(5, 5).unwrap();
        assert_eq!(p.alpha(), 255);
        assert!((120..=135).contains(&p.red()));
    }

    #[test]
    fn halo_matches_a_gaussian_box() {
        let (half, sigma) = ((50.0, 20.0), 10.0);
        let centre = shadow_alpha((0.0, 0.0), half, 10.0, sigma);
        assert!(centre > 0.95 && centre <= 1.01, "{centre}");
        let edge = shadow_alpha((50.0, 0.0), half, 10.0, sigma);
        assert!((edge - 0.5).abs() < 0.05, "half at the edge: {edge}");
        assert!(shadow_alpha((50.0 + 3.0 * sigma, 0.0), half, 10.0, sigma) < 0.01);
    }

    #[test]
    fn decor_planes_are_snapped_and_inside_the_output() {
        let cfg = WallpaperConfig::default();
        let fonts = FontNames { ui: "Inter".into(), display: "Inter".into(), arabic: "Cairo".into(), mono: "JetBrains Mono".into() };
        let date = DateLine { gregorian: "Tuesday, 27 October".into(), hijri_ar: ("٥".into(), "جمادى الأولى", "١٤٤٨".into()), hijri_latin: String::new(), weather: Some(("sun", "23°".into(), "Clear".into())) };
        let lines = vec!["21".to_string(), "05".to_string()];
        let ink = desktop::ink_for(None, Color::oklch(0.6, 0.1, 200.0));
        let input = DecorInput { logical: (2560.0, 1440.0), scale120: 180, cfg: &cfg, lines: &lines, date: Some(&date), ink, fonts: &fonts, safe: Default::default() };
        let mut text = TextPainter::new();
        let geo = DecorGeometry::new(&input, &mut text);
        assert_eq!(geo.output, Rect::new(0, 0, 3840, 2160));
        let step = mandara_core::shm_canvas::grid_step(180) as i32;
        for r in [geo.halo, geo.digits, geo.ring, geo.pill.unwrap()] {
            assert!(!r.is_empty());
            assert_eq!(r.intersect(&geo.output), r, "{r:?} inside");
            assert!(r.x % step == 0 && r.y % step == 0, "{r:?} on the grid");
        }
        assert!(geo.halo_buffer.0 * HALO_DOWNSCALE >= geo.halo.w);
        let p = geo.pill.unwrap();
        assert!(p.y > geo.digits.y && (p.x + p.w as i32 / 2 - 1920).abs() < 8, "pill centred under the clock: {p:?}");
        // The digits plane paints something.
        let mut pm = Pixmap::new(geo.digits.w, geo.digits.h).unwrap();
        paint_digits(&mut pm.as_mut(), &mut text, &geo, &input);
        assert!(pm.data().chunks_exact(4).any(|p| p[3] > 200));
        let mut pill = Pixmap::new(p.w, p.h).unwrap();
        paint_pill(&mut pill.as_mut(), &mut text, &geo, &input);
        assert!(pill.data().chunks_exact(4).any(|p| p[3] > 0));
    }

    #[test]
    fn reads_the_default_weight_of_variable_faces() {
        // Minimal sfnt: one table record (`fvar`) and one `wght` axis.
        let mut f = vec![0u8; 12 + 16];
        f[4..6].copy_from_slice(&1u16.to_be_bytes());
        f[12..16].copy_from_slice(b"fvar");
        f[20..24].copy_from_slice(&28u32.to_be_bytes());
        let mut fvar = vec![0u8; 16];
        fvar[4..6].copy_from_slice(&16u16.to_be_bytes());
        fvar[8..10].copy_from_slice(&1u16.to_be_bytes());
        fvar[10..12].copy_from_slice(&20u16.to_be_bytes());
        let mut axis = vec![0u8; 20];
        axis[0..4].copy_from_slice(b"wght");
        axis[8..12].copy_from_slice(&(400u32 << 16).to_be_bytes());
        f.extend(fvar);
        f.extend(axis);
        assert_eq!(sfnt_default_weight(&f, 0), Some(400));
        f[12..16].copy_from_slice(b"glyf");
        assert_eq!(sfnt_default_weight(&f, 0), None);
    }

    #[test]
    fn cut_source_crops_to_the_subject() {
        let dir = std::env::temp_dir().join(format!("mandara-cut-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mask = dir.join("m.png");
        GrayImage::from_fn(40, 20, |_, y| image::Luma([if y >= 10 { 255 } else { 0 }])).save(&mask).unwrap();
        let mut bg = Pixmap::new(40, 20).unwrap();
        bg.fill(tiny_skia::Color::from_rgba8(100, 150, 200, 255));
        let cut = cut_source(&bg, &mask).unwrap().unwrap();
        assert_eq!(cut.rect, Rect::new(0, 10, 40, 10));
        assert_eq!(cut.pixmap.as_ref().unwrap().pixel(3, 3).unwrap().green(), 150);
        let released = cut.released();
        assert!(released.pixmap.is_none());
        assert_eq!((released.rect, released.map.dimensions()), (cut.rect, cut.map.dimensions()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
