//! Software painting for the lock screen: tiny-skia shapes plus cosmic-text
//! (HarfBuzz-grade shaping, Arabic/RTL included) glyph rendering into a
//! premultiplied RGBA pixmap. Coordinates are physical pixels; callers
//! multiply logical sizes by the surface scale.

use std::collections::HashMap;

use cosmic_text::{Attrs, Buffer, Family, FeatureTag, FontFeatures, FontSystem, Metrics, Shaping, SwashCache, Weight};
use mandara_theme::Color;
use tiny_skia::{
    FillRule, FilterQuality, GradientStop, LinearGradient, Paint, Path, PathBuilder, Pattern, PixmapMut, Point, RadialGradient,
    SpreadMode, Stroke, Transform,
};

/// Straight (non-premultiplied) sRGB color with an extra opacity factor.
pub fn skia(c: Color, opacity: f32) -> tiny_skia::Color {
    let [r, g, b, a] = c.to_rgba8();
    tiny_skia::Color::from_rgba8(r, g, b, ((a as f32) * opacity.clamp(0.0, 1.0)).round() as u8)
}

pub fn solid(c: Color, opacity: f32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(skia(c, opacity));
    p.anti_alias = true;
    p
}

/// A rounded rectangle (circular corners, radius clamped to half the side).
pub fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    const K: f32 = 0.552_284_8;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + r * K, y, x + w, y + r - r * K, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + r * K, x + w - r + r * K, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - r * K, y + h, x, y + h - r + r * K, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - r * K, x + r - r * K, y, x + r, y);
    pb.close();
    pb.finish()
}

pub fn circle(cx: f32, cy: f32, r: f32) -> Option<Path> {
    PathBuilder::from_circle(cx, cy, r.max(0.1))
}

pub fn fill(pm: &mut PixmapMut<'_>, path: &Option<Path>, paint: &Paint<'_>) {
    if let Some(p) = path {
        pm.fill_path(p, paint, FillRule::Winding, Transform::identity(), None);
    }
}

pub fn stroke(pm: &mut PixmapMut<'_>, path: &Option<Path>, paint: &Paint<'_>, width: f32) {
    if let Some(p) = path {
        pm.stroke_path(p, paint, &Stroke { width, ..Stroke::default() }, Transform::identity(), None);
    }
}

/// A two-stop linear gradient paint between two points.
pub fn linear(from: (f32, f32), to: (f32, f32), a: tiny_skia::Color, b: tiny_skia::Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.anti_alias = true;
    if let Some(shader) = LinearGradient::new(
        Point::from_xy(from.0, from.1),
        Point::from_xy(to.0, to.1),
        vec![GradientStop::new(0.0, a), GradientStop::new(1.0, b)],
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        p.shader = shader;
    } else {
        p.set_color(a);
    }
    p
}

/// A soft radial glow fading to transparent.
pub fn glow(pm: &mut PixmapMut<'_>, cx: f32, cy: f32, r: f32, color: tiny_skia::Color) {
    let transparent = tiny_skia::Color::from_rgba(color.red(), color.green(), color.blue(), 0.0).unwrap_or(tiny_skia::Color::TRANSPARENT);
    let Some(shader) = RadialGradient::new(
        Point::from_xy(cx, cy),
        Point::from_xy(cx, cy),
        r,
        vec![GradientStop::new(0.0, color), GradientStop::new(1.0, transparent)],
        SpreadMode::Pad,
        Transform::identity(),
    ) else {
        return;
    };
    let mut p = Paint::default();
    p.shader = shader;
    p.anti_alias = true;
    if let Some(rect) = tiny_skia::Rect::from_xywh(cx - r, cy - r, r * 2.0, r * 2.0) {
        pm.fill_rect(rect, &p, Transform::identity(), None);
    }
}

/// A cheap soft shadow: stacked, expanding rounded rects with falling alpha.
pub fn soft_shadow(pm: &mut PixmapMut<'_>, x: f32, y: f32, w: f32, h: f32, r: f32, spread: f32, alpha: f32) {
    const STEPS: usize = 10;
    for i in (0..STEPS).rev() {
        let k = i as f32 / STEPS as f32;
        let e = spread * k;
        let a = alpha * (1.0 - k).powi(2) / STEPS as f32 * 2.2;
        fill(pm, &rounded_rect(x - e, y - e + spread * 0.25, w + e * 2.0, h + e * 2.0, r + e), &solid(Color::hex("#000000"), a));
    }
}

/// The Mandara chromatic-glass rim: a 1px directional border, bright at the
/// top-left fading to shadow at the bottom-right.
pub fn glass_rim(pm: &mut PixmapMut<'_>, path: &Option<Path>, x: f32, y: f32, w: f32, h: f32, light: Color, shadow: Color, intensity: f32, width: f32) {
    let paint = linear((x, y), (x + w, y + h), skia(light, intensity), skia(shadow, intensity * 0.8));
    stroke(pm, path, &paint, width);
}

/// Paints an image pixmap scaled into a circle.
pub fn image_circle(pm: &mut PixmapMut<'_>, image: &tiny_skia::Pixmap, cx: f32, cy: f32, r: f32, opacity: f32) {
    let scale = (2.0 * r) / image.width().min(image.height()) as f32;
    let tx = cx - image.width() as f32 * scale / 2.0;
    let ty = cy - image.height() as f32 * scale / 2.0;
    let mut p = Paint::default();
    p.anti_alias = true;
    p.shader = Pattern::new(image.as_ref(), SpreadMode::Pad, FilterQuality::Bicubic, opacity.clamp(0.0, 1.0), Transform::from_row(scale, 0.0, 0.0, scale, tx, ty));
    fill(pm, &circle(cx, cy, r), &p);
}

/// Paints an image covering the whole pixmap (center crop).
pub fn image_cover(pm: &mut PixmapMut<'_>, image: &tiny_skia::Pixmap) {
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    let scale = (w / image.width() as f32).max(h / image.height() as f32);
    let tx = (w - image.width() as f32 * scale) / 2.0;
    let ty = (h - image.height() as f32 * scale) / 2.0;
    let mut p = Paint::default();
    p.shader = Pattern::new(image.as_ref(), SpreadMode::Pad, FilterQuality::Bilinear, 1.0, Transform::from_row(scale, 0.0, 0.0, scale, tx, ty));
    if let Some(rect) = tiny_skia::Rect::from_xywh(0.0, 0.0, w, h) {
        pm.fill_rect(rect, &p, Transform::identity(), None);
    }
}

/// Converts straight RGBA bytes into a premultiplied pixmap.
pub fn pixmap_from_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<tiny_skia::Pixmap> {
    let mut pm = tiny_skia::Pixmap::new(width, height)?;
    for (d, s) in pm.data_mut().chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
        let a = s[3] as u32;
        d[0] = ((s[0] as u32 * a + 127) / 255) as u8;
        d[1] = ((s[1] as u32 * a + 127) / 255) as u8;
        d[2] = ((s[2] as u32 * a + 127) / 255) as u8;
        d[3] = s[3];
    }
    Some(pm)
}

/// Adds ±`amount` (0–1) procedural luminance noise to kill gradient banding.
pub fn grain(pm: &mut PixmapMut<'_>, amount: f32) {
    let width = pm.width() as usize;
    let amp = (amount * 255.0).max(0.0);
    for (i, px) in pm.data_mut().chunks_exact_mut(4).enumerate() {
        // Integer hash of the pixel position (stable between frames).
        let (x, y) = ((i % width) as u32, (i / width) as u32);
        let mut h = x.wrapping_mul(374_761_393) ^ y.wrapping_mul(668_265_263);
        h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
        let n = ((h >> 16) & 0xff) as f32 / 255.0 - 0.5;
        let d = n * 2.0 * amp;
        let a = px[3] as f32;
        for c in &mut px[..3] {
            *c = (*c as f32 + d).clamp(0.0, a) as u8;
        }
    }
}

/// Multiplies every pixel (premultiplied) by `opacity`.
pub fn fade(pm: &mut PixmapMut<'_>, opacity: f32) {
    if opacity >= 0.999 {
        return;
    }
    let k = (opacity.clamp(0.0, 1.0) * 256.0) as u32;
    for b in pm.data_mut() {
        *b = ((*b as u32 * k) >> 8) as u8;
    }
}

// ─────────────────────────────── text ───────────────────────────────

/// What text to draw and how.
#[derive(Clone, Debug, PartialEq)]
pub struct Font<'a> {
    pub family: &'a str,
    /// Physical pixels.
    pub size: f32,
    pub weight: u16,
    /// OpenType `tnum` (clocks never jitter).
    pub tabular: bool,
    /// Extra spacing between letters, em fraction.
    pub tracking: f32,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    text: String,
    family: String,
    size: u32,
    weight: u16,
    tabular: bool,
    tracking: i32,
}

/// Shapes and rasterizes text; shaped buffers and glyph bitmaps are cached.
pub struct TextPainter {
    fonts: FontSystem,
    glyphs: SwashCache,
    shaped: HashMap<Key, Buffer>,
    /// Families for characters the requested family lacks (Arabic in Inter),
    /// best first; see [`crate::fonts::fallback_chain`].
    fallbacks: Vec<String>,
}

/// The `wght` variation axis.
const WGHT: [u8; 4] = *b"wght";

impl Default for TextPainter {
    fn default() -> Self {
        Self::new()
    }
}

impl TextPainter {
    pub fn new() -> Self {
        let fonts = FontSystem::new();
        let mut installed: Vec<String> =
            fonts.db().faces().filter_map(|face| face.families.first().map(|(name, _)| name.clone())).collect();
        installed.sort_unstable();
        installed.dedup();
        let fallbacks = crate::fonts::fallback_chain(&mandara_config::TypographyConfig::default().arabic_font, &installed);
        Self { fonts, glyphs: SwashCache::new(), shaped: HashMap::new(), fallbacks }
    }

    /// Frees the cached glyph bitmaps (the shaped text stays). For painters
    /// whose text changes rarely, like the desktop clock: its bitmaps are
    /// copied into a plane once a minute and the cache only grew.
    pub fn release_glyphs(&mut self) {
        self.glyphs = SwashCache::new();
    }

    /// Puts `arabic` (the configured Arabic family) first in the fallback chain.
    pub fn set_arabic_font(&mut self, arabic: &str) {
        let mut installed: Vec<String> =
            self.fonts.db().faces().filter_map(|face| face.families.first().map(|(name, _)| name.clone())).collect();
        installed.sort_unstable();
        installed.dedup();
        let fallbacks = crate::fonts::fallback_chain(arabic, &installed);
        if fallbacks != self.fallbacks {
            self.fallbacks = fallbacks;
            self.shaped.clear();
        }
    }

    /// The face `family` resolves to at `weight` (upright).
    fn face(&self, family: &str, weight: u16) -> Option<cosmic_text::fontdb::ID> {
        self.fonts.db().query(&cosmic_text::fontdb::Query {
            families: &[Family::Name(family)],
            weight: Weight(weight),
            ..Default::default()
        })
    }

    fn covers(&mut self, id: cosmic_text::fontdb::ID, ch: char) -> bool {
        self.fonts.get_font(id, Weight::NORMAL).is_some_and(|font| font.as_swash().charmap().map(ch) != 0)
    }

    /// The weight to ask `family` for: exact on a variable axis, else the
    /// nearest static weight it ships (cosmic-text would otherwise skip the
    /// family and pick any face of that exact weight).
    fn family_weight(&mut self, family: &str, weight: u16) -> u16 {
        let ids: Vec<(cosmic_text::fontdb::ID, u16)> = self
            .fonts
            .db()
            .faces()
            .filter(|face| face.style == cosmic_text::Style::Normal && face.families.iter().any(|(name, _)| name == family))
            .map(|face| (face.id, face.weight.0))
            .collect();
        let variable = ids.iter().any(|(id, _)| {
            self.fonts.get_font(*id, Weight(weight)).is_some_and(|font| {
                font.as_swash()
                    .variations()
                    .find_by_tag(u32::from_be_bytes(WGHT))
                    .is_some_and(|axis| (axis.min_value()..=axis.max_value()).contains(&f32::from(weight)))
            })
        });
        let available: Vec<u16> = ids.iter().map(|(_, w)| *w).collect();
        if variable { weight } else { gpui_wgpu::css_nearest_weight(weight, &available).unwrap_or(weight) }
    }

    /// Splits `text` into `(range, family index)` runs: `None` for the
    /// requested family, `Some(i)` for `fallbacks[i]` where it lacks a glyph.
    /// ASCII spaces and punctuation join a neighbouring fallback run:
    /// cosmic-text resolves a punctuation-only span's direction on its own,
    /// which would move the ":" of Arabic "مجدول: التبديل" to the line's end.
    fn script_runs(&mut self, text: &str, primary: Option<cosmic_text::fontdb::ID>) -> Vec<(std::ops::Range<usize>, Option<usize>)> {
        let chain: Vec<Option<cosmic_text::fontdb::ID>> =
            self.fallbacks.clone().iter().map(|family| self.face(family, Weight::NORMAL.0)).collect();
        let neutral = |ch: char| ch.is_ascii() && !ch.is_ascii_alphanumeric();
        // Per char: `Err(())` for neutrals (resolved below), else its slot.
        let mut slots: Vec<(usize, char, Result<Option<usize>, ()>)> = Vec::with_capacity(text.len());
        let mut current: Option<usize> = None;
        for (start, ch) in text.char_indices() {
            let slot = if neutral(ch) {
                Err(())
            } else if ch.is_ascii() || primary.is_some_and(|id| self.covers(id, ch)) {
                Ok(None)
            } else if current.and_then(|i| chain[i]).is_some_and(|id| self.covers(id, ch)) {
                Ok(current)
            } else {
                Ok((0..chain.len()).find(|i| chain[*i].is_some_and(|id| self.covers(id, ch))))
            };
            if let Ok(slot) = slot {
                current = slot;
            }
            slots.push((start, ch, slot));
        }
        let mut runs: Vec<(std::ops::Range<usize>, Option<usize>)> = Vec::new();
        for ix in 0..slots.len() {
            let (start, ch, slot) = slots[ix];
            let slot = match slot {
                Ok(slot) => slot,
                Err(()) => {
                    let before = slots[..ix].iter().rev().find_map(|(_, _, s)| s.ok());
                    let after = slots[ix + 1..].iter().find_map(|(_, _, s)| s.ok());
                    [before, after]
                        .into_iter()
                        .flatten()
                        .flatten()
                        .find(|i| chain[*i].is_some_and(|id| self.covers(id, ch)))
                }
            };
            let end = start + ch.len_utf8();
            match runs.last_mut() {
                Some((range, last)) if *last == slot => range.end = end,
                _ => runs.push((start..end, slot)),
            }
        }
        runs
    }

    /// The font database (e.g. to inspect which face a family resolves to).
    pub fn font_system(&mut self) -> &mut FontSystem {
        &mut self.fonts
    }

    fn key(text: &str, font: &Font<'_>) -> Key {
        Key {
            text: text.to_string(),
            family: font.family.to_string(),
            size: (font.size * 64.0) as u32,
            weight: font.weight,
            tabular: font.tabular,
            tracking: (font.tracking * 1000.0) as i32,
        }
    }

    /// Shapes `text` once; returns its cache key.
    fn shape(&mut self, text: &str, font: &Font<'_>) -> Key {
        let key = Self::key(text, font);
        if self.shaped.len() > 96 && !self.shaped.contains_key(&key) {
            self.shaped.clear();
        }
        if self.shaped.contains_key(&key) {
            return key;
        }
        let primary = self.face(font.family, font.weight);
        let runs = self.script_runs(text, primary);
        let weights: Vec<u16> =
            self.fallbacks.clone().iter().map(|family| self.family_weight(family, font.weight)).collect();
        let fonts = &mut self.fonts;
        let mut buffer = Buffer::new(fonts, Metrics::new(font.size, (font.size * 1.25).ceil()));
        buffer.set_size(None, None);
        let mut features = FontFeatures::new();
        if font.tabular {
            features.enable(FeatureTag::new(b"tnum"));
        }
        let attrs = Attrs::new()
            .family(Family::Name(font.family))
            .weight(Weight(font.weight))
            .font_features(features)
            .letter_spacing(font.tracking);
        let spans = runs.into_iter().map(|(range, slot)| {
            let span = match slot {
                Some(i) => attrs.clone().family(Family::Name(&self.fallbacks[i])).weight(Weight(weights[i])),
                None => attrs.clone(),
            };
            (&text[range], span)
        });
        buffer.set_rich_text(spans, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(fonts, false);
        self.shaped.insert(key.clone(), buffer);
        key
    }

    /// `(width, line height)` in physical pixels.
    pub fn measure(&mut self, text: &str, font: &Font<'_>) -> (f32, f32) {
        let key = self.shape(text, font);
        let Some(buffer) = self.shaped.get(&key) else { return (0.0, font.size) };
        let mut w: f32 = 0.0;
        let mut h: f32 = 0.0;
        for run in buffer.layout_runs() {
            w = w.max(run.line_w);
            h += run.line_height;
        }
        (w, h.max(font.size * 1.25))
    }

    /// Draws `text` with its top-left at `(x, y)`.
    pub fn draw(&mut self, pm: &mut PixmapMut<'_>, text: &str, font: &Font<'_>, x: f32, y: f32, color: Color, opacity: f32) {
        if opacity <= 0.003 || text.is_empty() {
            return;
        }
        let [r, g, b, a] = color.to_rgba8();
        let base = cosmic_text::Color::rgba(r, g, b, a);
        let (ox, oy) = (x.round() as i32, y.round() as i32);
        let (pw, ph) = (pm.width() as i32, pm.height() as i32);
        let key = self.shape(text, font);
        // Split borrows: the buffer lives in `shaped`; the font system and
        // glyph cache are separate fields.
        let Self { fonts, glyphs, shaped, .. } = self;
        let Some(buffer) = shaped.get_mut(&key) else { return };
        let data = pm.data_mut();
        buffer.draw(fonts, glyphs, base, |gx, gy, w, h, c| {
            let alpha = c.a() as f32 / 255.0 * opacity;
            if alpha <= 0.0 {
                return;
            }
            let (sr, sg, sb) = (c.r() as f32 * alpha, c.g() as f32 * alpha, c.b() as f32 * alpha);
            let inv = 1.0 - alpha;
            for yy in 0..h as i32 {
                let py = oy + gy + yy;
                if py < 0 || py >= ph {
                    continue;
                }
                for xx in 0..w as i32 {
                    let px = ox + gx + xx;
                    if px < 0 || px >= pw {
                        continue;
                    }
                    let i = ((py * pw + px) * 4) as usize;
                    let d = &mut data[i..i + 4];
                    d[0] = (sr + d[0] as f32 * inv).min(255.0) as u8;
                    d[1] = (sg + d[1] as f32 * inv).min(255.0) as u8;
                    d[2] = (sb + d[2] as f32 * inv).min(255.0) as u8;
                    d[3] = (alpha * 255.0 + d[3] as f32 * inv).min(255.0) as u8;
                }
            }
        });
    }

    /// Draws `text` horizontally centered on `cx`; returns its size.
    pub fn draw_centered(&mut self, pm: &mut PixmapMut<'_>, text: &str, font: &Font<'_>, cx: f32, y: f32, color: Color, opacity: f32) -> (f32, f32) {
        let (w, h) = self.measure(text, font);
        self.draw(pm, text, font, cx - w / 2.0, y, color, opacity);
        (w, h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_rect_clamps_radius() {
        assert!(rounded_rect(0.0, 0.0, 10.0, 4.0, 50.0).is_some());
        let p = rounded_rect(0.0, 0.0, 100.0, 50.0, 25.0).unwrap();
        let b = p.bounds();
        assert!((b.width() - 100.0).abs() < 0.01 && (b.height() - 50.0).abs() < 0.01);
    }

    #[test]
    fn premultiplies_and_fades() {
        let pm = pixmap_from_rgba(1, 1, &[255, 128, 0, 128]).unwrap();
        assert_eq!(pm.data(), &[128, 64, 0, 128]);
        let mut pm = pm;
        fade(&mut pm.as_mut(), 0.5);
        assert_eq!(pm.data(), &[64, 32, 0, 64]);
    }

    #[test]
    fn grain_keeps_premultiplied_invariant() {
        let mut pm = tiny_skia::Pixmap::new(16, 16).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(40, 40, 40, 200));
        grain(&mut pm.as_mut(), 0.015);
        for px in pm.data().chunks_exact(4) {
            assert!(px[0] <= px[3] && px[1] <= px[3] && px[2] <= px[3]);
        }
    }

    #[test]
    fn arabic_uses_the_fallback_chain_at_every_weight() {
        let mut painter = TextPainter::new();
        let Some(first) = painter.fallbacks.first().cloned() else { return };
        let sample = "الساعة المظهر";
        for weight in [300, 400, 500, 600, 700] {
            let font = Font { family: "Inter", size: 16.0, weight, tabular: false, tracking: 0.0 };
            let key = painter.shape(sample, &font);
            let buffer = &painter.shaped[&key];
            for run in buffer.layout_runs() {
                for glyph in run.glyphs.iter().filter(|g| !sample[g.start..].starts_with(' ')) {
                    let face = painter.fonts.db().face(glyph.font_id).expect("face");
                    assert_eq!(face.families[0].0, first, "weight {weight}: Arabic drawn with {}", face.post_script_name);
                    assert_ne!(glyph.glyph_id, 0, "weight {weight}: .notdef");
                }
            }
        }
    }

    #[test]
    fn punctuation_inside_arabic_keeps_its_place() {
        let mut painter = TextPainter::new();
        if painter.fallbacks.is_empty() {
            return;
        }
        let text = "مجدول: التبديل";
        let font = Font { family: "Inter", size: 16.0, weight: 600, tabular: false, tracking: 0.0 };
        let key = painter.shape(text, &font);
        let glyphs: Vec<(usize, f32)> =
            painter.shaped[&key].layout_runs().flat_map(|run| run.glyphs.iter().map(|g| (g.start, g.x))).collect();
        let x_of = |index: usize| glyphs.iter().find(|(start, _)| *start == index).map(|(_, x)| *x).expect("glyph");
        let colon = x_of(text.find(':').expect("colon"));
        let (first, last) = (x_of(0), x_of(text.char_indices().last().expect("char").0));
        assert!(last < colon && colon < first, "colon at {colon} outside {last}..{first}");
    }

    #[test]
    fn arabic_glued_to_latin_is_shaped_and_ordered() {
        let mut painter = TextPainter::new();
        let Some(first) = painter.fallbacks.first().cloned() else { return };
        let font = Font { family: "Inter", size: 16.0, weight: 400, tabular: false, tracking: 0.0 };
        for (text, before, after, rtl) in
            [("الشبكة وBluetooth", "الشبكة", "Bluetooth", true), ("MangoWM وHyprland", "MangoWM", "Hyprland", false), ("Alt+Tab وAlt+Shift+Tab", "Alt+Tab", "Alt+Shift+Tab", false)]
        {
            let key = painter.shape(text, &font);
            let glyphs: Vec<(usize, f32, cosmic_text::fontdb::ID, u16)> =
                painter.shaped[&key].layout_runs().flat_map(|run| run.glyphs.iter().map(|g| (g.start, g.x, g.font_id, g.glyph_id))).collect();
            let waw_at = text.find('و').expect("waw");
            let &(_, waw_x, waw_face, waw_glyph) = glyphs.iter().find(|g| g.0 == waw_at).expect("waw glyph");
            let face = painter.fonts.db().face(waw_face).expect("face").families[0].0.clone();
            assert_eq!(face, first, "{text}: و drawn with {face}");
            let isolated = painter.fonts.get_font(waw_face, Weight::NORMAL).map(|f| f.as_swash().charmap().map('و'));
            assert_eq!(isolated, Some(waw_glyph), "{text}: و not in its isolated form");
            let span = |word: &str, from: usize| {
                let start = text[from..].find(word).expect("word") + from;
                let xs: Vec<f32> = glyphs.iter().filter(|g| (start..start + word.len()).contains(&g.0)).map(|g| g.1).collect();
                (xs.iter().copied().fold(f32::MAX, f32::min), xs.iter().copied().fold(f32::MIN, f32::max))
            };
            let ((before_min, before_max), (after_min, after_max)) = (span(before, 0), span(after, waw_at));
            if rtl {
                assert!(after_max < waw_x && waw_x < before_min, "{text}: expected {after} | و | {before}");
            } else {
                assert!(before_max < waw_x && waw_x < after_min, "{text}: expected {before} | و | {after}");
            }
        }
    }
}
