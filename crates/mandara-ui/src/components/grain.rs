//! Surface grain (`mandara_config::grain::Grain`): one shared renderer any
//! surface can add as its last child.
//!
//! * [`overlay`] (or [`overlay_with`]) returns an absolutely positioned
//!   layer that fills its parent, clips itself to the given corners and
//!   takes no pointer events; `None` while grain is off or its tile is
//!   still being drawn.
//! * A tile is [`TILE_PX`] device pixels square and drawn once per
//!   [`TileKey`] (style, speck size, contrast, seed, scale factor and, for
//!   palette-toned grain, the two tones) on the background executor, never
//!   on the UI thread and never per frame. One tile is drawn at a time;
//!   other requests wait and redraw their windows when it lands, then ask
//!   again (so a dragged slider coalesces to its latest value).
//! * Tiles are painted 1:1 in device pixels (crisp at fractional scales)
//!   and anchored to the window's origin, so the grain never swims while a
//!   surface grows, slides or mirrors for right-to-left languages.
//! * The cache is process-wide and bounded ([`CACHE_MAX`] tiles, least
//!   recently used first out). Evicted tiles leave every window's sprite
//!   atlas and the allocator is asked to trim (`memory::request_trim`);
//!   [`release`] empties it the same way.

use std::sync::Arc;

use gpui::{AnyElement, AnyWindowHandle, App, AppContext as _, Bounds, Corners, Global, IntoElement, ParentElement, Pixels, RenderImage, Styled, Window, canvas, div, point, px, size};
use mandara_config::grain::{Grain, GrainStyle, GrainTint};
use mandara_theme::{Color, MandaraPalette};

use crate::components::surfaces::Surface as _;
use crate::style::ClipRounded as _;

/// Side of a grain tile, device pixels: small (64 KiB) and, filled with
/// random specks, without a visible repeat.
pub const TILE_PX: u32 = 128;
/// Most tiles kept: a few surfaces with different grain, plus one or two
/// left over from a setting being changed.
pub const CACHE_MAX: usize = 6;

/// Quantization of the key's numbers: speck size in quarter pixels,
/// contrast in twentieths, the scale factor in hundredths.
const SCALE_STEPS: f32 = 4.0;
const CONTRAST_STEPS: f32 = 20.0;
const FACTOR_STEPS: f32 = 100.0;
/// Palette-toned specks: lightness of the light and dark tone, and the
/// share of the surface's chroma they keep (capped, so they stay grain).
const TONE_LIGHT_L: f32 = 0.96;
const TONE_DARK_L: f32 = 0.14;
const TONE_CHROMA_SHARE: f32 = 1.6;
const TONE_CHROMA_MAX: f32 = 0.06;
/// Film: the clump grid is this many times coarser than the specks, and
/// how much of each speck comes from its clump.
const FILM_CLUMP: u32 = 2;
const FILM_CLUMP_SHARE: f32 = 0.45;
/// Paper: fibres run this many specks long, only their strongest part
/// shows (above the threshold), over a faint tooth of fine grain.
const PAPER_FIBRE: u32 = 7;
const PAPER_THRESHOLD: f32 = 0.45;
const PAPER_TOOTH: f32 = 0.28;
/// Speckle: share of specks that fleck, the share of those that are light,
/// a fleck's weakest strength, and the faint grain between them.
const SPECKLE_DENSITY: f32 = 0.035;
const SPECKLE_LIGHT: f32 = 0.12;
const SPECKLE_MIN: f32 = 0.6;
const SPECKLE_GROUND: f32 = 0.12;

/// What a tile is drawn from (two grains with the same key share one).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub style: GrainStyle,
    scale_q: u16,
    contrast_q: u16,
    pub seed: u32,
    factor_q: u16,
    /// Light and dark speck colours (sRGB) for palette-toned grain.
    tones: Option<[[u8; 3]; 2]>,
}

/// An 8-bit sRGB channel.
fn channel(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * f32::from(u8::MAX)).round() as u8
}

fn rgb(c: Color) -> [u8; 3] {
    let s = c.to_srgba();
    [channel(s.red), channel(s.green), channel(s.blue)]
}

impl TileKey {
    /// The tile `grain` needs at `scale_factor` over a surface of
    /// `palette`, or `None` while it is off.
    pub fn new(grain: &Grain, scale_factor: f32, palette: &MandaraPalette) -> Option<Self> {
        if !grain.is_on() {
            return None;
        }
        let q = |v: f32, steps: f32| (v * steps).round().clamp(1.0, f32::from(u16::MAX)) as u16;
        let tones = (grain.tint == GrainTint::Surface).then(|| {
            let tone = palette.surface_base;
            let chroma = (tone.chroma() * TONE_CHROMA_SHARE).min(TONE_CHROMA_MAX);
            [rgb(Color::oklch(TONE_LIGHT_L, chroma, tone.hue())), rgb(Color::oklch(TONE_DARK_L, chroma, tone.hue()))]
        });
        Some(Self { style: grain.style, scale_q: q(grain.scale(), SCALE_STEPS), contrast_q: q(grain.contrast(), CONTRAST_STEPS), seed: grain.seed, factor_q: q(scale_factor, FACTOR_STEPS), tones })
    }

    fn scale(&self) -> f32 {
        f32::from(self.scale_q) / SCALE_STEPS
    }

    fn contrast(&self) -> f32 {
        f32::from(self.contrast_q) / CONTRAST_STEPS
    }

    fn factor(&self) -> f32 {
        f32::from(self.factor_q) / FACTOR_STEPS
    }

    /// Specks across a tile: a whole number, so the tile repeats seamlessly.
    fn cells(&self) -> u32 {
        ((TILE_PX as f32 / (self.scale() * self.factor())).round() as u32).clamp(1, TILE_PX)
    }
}

/// A well-mixed 32-bit hash of a lattice point.
fn hash(seed: u32, x: u32, y: u32) -> u32 {
    let mut h = seed ^ x.wrapping_mul(0x27D4_EB2F) ^ y.wrapping_mul(0x1656_67B1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85EB_CA77);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE3D);
    h ^ (h >> 16)
}

/// `[0, 1)` from 16 bits of `h` at `shift`.
fn unit(h: u32, shift: u32) -> f32 {
    f32::from((h >> shift) as u16) / (f32::from(u16::MAX) + 1.0)
}

/// A signed speck: light or dark, mostly faint (the mean of two uniforms).
fn fine(seed: u32, x: u32, y: u32) -> f32 {
    let h = hash(seed, x, y);
    let m = (unit(h, 0) + unit(h, 16)) / 2.0;
    if h & 1 == 0 { m } else { -m }
}

/// Smooth value noise on a `period`-wide lattice that wraps, sampled at
/// `(x, y)` in lattice units, signed.
fn value(seed: u32, x: f32, y: f32, period: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (tx, ty) = (x - x0, y - y0);
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let p = period.max(1);
    let at = |dx: u32, dy: u32| {
        let (cx, cy) = ((x0 as u32 + dx) % p, (y0 as u32 + dy) % p);
        unit(hash(seed, cx, cy), 0) * 2.0 - 1.0
    };
    let top = at(0, 0) + (at(1, 0) - at(0, 0)) * sx;
    let bottom = at(0, 1) + (at(1, 1) - at(0, 1)) * sx;
    top + (bottom - top) * sy
}

/// The signed strength of speck `(x, y)` of a tile `cells` specks across.
fn speck(style: GrainStyle, seed: u32, x: u32, y: u32, cells: u32) -> f32 {
    match style {
        GrainStyle::None => 0.0,
        GrainStyle::Fine => fine(seed, x, y),
        GrainStyle::Film => {
            let period = (cells / FILM_CLUMP).max(1);
            let k = period as f32 / cells as f32;
            let clump = value(seed ^ 0x9E37_79B9, x as f32 * k, y as f32 * k, period);
            (clump * FILM_CLUMP_SHARE + fine(seed, x, y) * (1.0 - FILM_CLUMP_SHARE)).clamp(-1.0, 1.0)
        }
        GrainStyle::Paper => {
            let period = (cells / PAPER_FIBRE).max(1);
            let k = period as f32 / cells as f32;
            // Each row of specks has its own fibres, lighter or darker.
            let row = hash(seed ^ 0x51ED_270B, 0, y);
            let fibre = value(seed ^ row, x as f32 * k, 0.0, period).abs();
            let strand = ((fibre - PAPER_THRESHOLD) / (1.0 - PAPER_THRESHOLD)).max(0.0);
            let sign = if row & 1 == 0 { 1.0 } else { -1.0 };
            (strand * sign + fine(seed, x, y) * PAPER_TOOTH).clamp(-1.0, 1.0)
        }
        GrainStyle::Speckle => {
            let h = hash(seed ^ 0x2545_F491, x, y);
            if unit(h, 0) < SPECKLE_DENSITY {
                let m = SPECKLE_MIN + (1.0 - SPECKLE_MIN) * unit(h, 16);
                if unit(hash(seed, y, x), 0) < SPECKLE_LIGHT { m } else { -m }
            } else {
                fine(seed, x, y) * SPECKLE_GROUND
            }
        }
    }
}

/// A tile's straight-alpha BGRA pixels (GPUI's sprite order).
pub fn tile_pixels(key: &TileKey) -> image::RgbaImage {
    let cells = key.cells();
    let gamma = 1.0 / key.contrast();
    let [light, dark] = key.tones.unwrap_or([[u8::MAX; 3], [0; 3]]);
    image::RgbaImage::from_fn(TILE_PX, TILE_PX, |x, y| {
        // Device pixel → speck; specks of a fractional width alternate by a
        // pixel, which random grain hides.
        let (cx, cy) = (x * cells / TILE_PX, y * cells / TILE_PX);
        let s = speck(key.style, key.seed, cx, cy, cells);
        let a = channel(s.abs().powf(gamma));
        let [r, g, b] = if s >= 0.0 { light } else { dark };
        image::Rgba([b, g, r, a])
    })
}

/// The process-wide tile cache, its single job in flight and the windows
/// waiting for a tile.
#[derive(Default)]
pub struct TileCache {
    /// Least recently used first.
    entries: Vec<(TileKey, Arc<RenderImage>)>,
    in_flight: Option<TileKey>,
    waiters: Vec<AnyWindowHandle>,
}
impl Global for TileCache {}

impl TileCache {
    /// The tile for `key`, marked as just used.
    pub fn get(&mut self, key: &TileKey) -> Option<Arc<RenderImage>> {
        let i = self.entries.iter().position(|(k, _)| k == key)?;
        let entry = self.entries.remove(i);
        let image = entry.1.clone();
        self.entries.push(entry);
        Some(image)
    }

    /// A tile of the same style at the same scale factor, shown while the
    /// exact one is drawn.
    fn stand_in(&self, key: &TileKey) -> Option<Arc<RenderImage>> {
        self.entries.iter().rev().find(|(k, _)| k.style == key.style && k.factor_q == key.factor_q).map(|(_, i)| i.clone())
    }

    /// Keeps `image` for `key`; returns the tiles pushed out past [`CACHE_MAX`].
    pub fn insert(&mut self, key: TileKey, image: Arc<RenderImage>) -> Vec<Arc<RenderImage>> {
        self.entries.retain(|(k, _)| *k != key);
        self.entries.push((key, image));
        let over = self.entries.len().saturating_sub(CACHE_MAX);
        self.entries.drain(..over).map(|(_, i)| i).collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Drops `images` from every window's atlas and asks for an allocator trim.
fn forget(images: Vec<Arc<RenderImage>>, cx: &mut App) {
    if images.is_empty() {
        return;
    }
    for image in images {
        cx.drop_image(image, None);
    }
    mandara_services::memory::request_trim();
}

/// The tile for `key`, or a stand-in while it is drawn (the job starts
/// here when none is running; `redraw` redraws when it lands).
fn tile(key: TileKey, redraw: Option<AnyWindowHandle>, cx: &mut App) -> Option<Arc<RenderImage>> {
    let cache = cx.default_global::<TileCache>();
    if let Some(image) = cache.get(&key) {
        return Some(image);
    }
    if let Some(handle) = redraw
        && !cache.waiters.iter().any(|w| w.window_id() == handle.window_id())
    {
        cache.waiters.push(handle);
    }
    if cache.in_flight.is_none() {
        cache.in_flight = Some(key);
        let job = cx.background_spawn(async move { Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(tile_pixels(&key))])) });
        cx.spawn(async move |cx| {
            let image = job.await;
            cx.update(|cx| {
                let cache = cx.default_global::<TileCache>();
                cache.in_flight = None;
                let evicted = cache.insert(key, image);
                let waiters = std::mem::take(&mut cache.waiters);
                forget(evicted, cx);
                for w in waiters {
                    let _ = w.update(cx, |_, window, _| window.refresh());
                }
            });
        })
        .detach();
    }
    cx.default_global::<TileCache>().stand_in(&key)
}

/// Grain over its parent, clipped to corners of radius `radius` (already
/// scaled); `None` while off or not drawn yet.
pub fn overlay(grain: &Grain, radius: f32, palette: &MandaraPalette, window: &Window, cx: &mut App) -> Option<AnyElement> {
    overlay_with(grain, Corners::all(px(radius.max(0.0))), palette, window.scale_factor(), Some(window.window_handle()), cx)
}

/// [`overlay`] with per-corner radii, an explicit scale factor and the
/// window to redraw when the tile lands (`None`: the caller redraws on its
/// own, e.g. a settings page re-rendered by the change).
pub fn overlay_with(grain: &Grain, corners: Corners<Pixels>, palette: &MandaraPalette, scale_factor: f32, redraw: Option<AnyWindowHandle>, cx: &mut App) -> Option<AnyElement> {
    let key = TileKey::new(grain, scale_factor, palette)?;
    let image = tile(key, redraw, cx)?;
    let layer = canvas(|_, _, _| {}, move |bounds, _, window, _| paint(bounds, &image, window)).absolute().inset_0();
    Some(div().absolute().inset_0().with_corners(corners).clip_rounded().opacity(grain.amount()).child(layer).into_any_element())
}

/// A surface family whose grain is prepared once per window render and
/// then drawn by every surface of that family without an `App` (cards are
/// built in many places that have none).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Slot {
    /// Studio's window behind its pages.
    Studio,
    /// Cards and grouped surfaces (the card design's surface).
    Cards,
    /// Bars and their capsules.
    Bars,
    /// The launcher.
    Launcher,
    /// Drop-down panels and plugin panels.
    Panels,
}

impl Slot {
    const COUNT: usize = 5;

    fn index(self) -> usize {
        self as usize
    }
}

thread_local! {
    /// Each slot's tile and strength, as last prepared on this thread.
    static SLOTS: std::cell::RefCell<[Option<(Arc<RenderImage>, f32)>; Slot::COUNT]> = const { std::cell::RefCell::new([const { None }; Slot::COUNT]) };
}

/// Prepares `slot` for this render of `window`: its tile for `grain` (a
/// cached one at once, or drawn in the background with a redraw when it
/// lands), or nothing while grain is off. Cheap when unchanged.
pub fn prepare(slot: Slot, grain: &Grain, palette: &MandaraPalette, window: &Window, cx: &mut App) {
    let ready = TileKey::new(grain, window.scale_factor(), palette).and_then(|key| tile(key, Some(window.window_handle()), cx)).map(|image| (image, grain.amount()));
    SLOTS.with(|s| s.borrow_mut()[slot.index()] = ready);
}

/// `slot`'s grain over its parent, clipped to `corners`; `None` while it is
/// off or not drawn yet. Add it as the surface's first child so its
/// content draws over the grain.
pub fn layer(slot: Slot, corners: Corners<Pixels>) -> Option<AnyElement> {
    let (image, amount) = SLOTS.with(|s| s.borrow()[slot.index()].clone())?;
    let paint_layer = canvas(|_, _, _| {}, move |bounds, _, window, _| paint(bounds, &image, window)).absolute().inset_0();
    Some(div().absolute().inset_0().with_corners(corners).clip_rounded().opacity(amount).child(paint_layer).into_any_element())
}

/// Forgets every slot (their tiles stay in the cache until evicted).
pub fn clear_slots() {
    SLOTS.with(|s| *s.borrow_mut() = [const { None }; Slot::COUNT]);
}

/// Repeats `image` over `bounds`, one texel per device pixel, the tiles
/// anchored to the window's origin.
fn paint(bounds: Bounds<Pixels>, image: &Arc<RenderImage>, window: &mut Window) {
    let step = TILE_PX as f32 / window.scale_factor().max(f32::EPSILON);
    let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let (x1, y1) = (x0 + f32::from(bounds.size.width), y0 + f32::from(bounds.size.height));
    let mut y = (y0 / step).floor() * step;
    while y < y1 {
        let mut x = (x0 / step).floor() * step;
        while x < x1 {
            let at = Bounds { origin: point(px(x), px(y)), size: size(px(step), px(step)) };
            // An atlas failure only loses the grain for this frame.
            let _ = window.paint_image(bounds, at, Corners::default(), image.clone(), 0, false);
            x += step;
        }
        y += step;
    }
}

/// Empties the tile cache (every surface turned its grain off, or memory
/// is being released): the tiles leave every atlas and the allocator trims.
pub fn release(cx: &mut App) {
    let Some(cache) = cx.try_global::<TileCache>() else { return };
    if cache.is_empty() {
        return;
    }
    let images = std::mem::take(&mut cx.global_mut::<TileCache>().entries).into_iter().map(|(_, i)| i).collect();
    forget(images, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(style: GrainStyle, scale: f32, factor: f32) -> TileKey {
        let palette = mandara_theme::engine::default_palette(true);
        TileKey::new(&Grain { scale, ..Grain::with_style(style) }, factor, &palette).unwrap()
    }

    fn image() -> Arc<RenderImage> {
        Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(image::RgbaImage::new(1, 1))]))
    }

    #[test]
    fn off_grain_needs_no_tile() {
        let palette = mandara_theme::engine::default_palette(false);
        assert!(TileKey::new(&Grain::default(), 1.0, &palette).is_none());
        assert!(TileKey::new(&Grain { amount: 0.0, ..Grain::with_style(GrainStyle::Fine) }, 1.0, &palette).is_none());
    }

    /// Keys quantize (a slider's tiny moves share a tile) and clamp.
    #[test]
    fn keys_quantize_and_clamp() {
        assert_eq!(key(GrainStyle::Fine, 1.01, 1.5), key(GrainStyle::Fine, 1.02, 1.5));
        assert_ne!(key(GrainStyle::Fine, 1.0, 1.5), key(GrainStyle::Fine, 2.0, 1.5));
        assert_eq!(key(GrainStyle::Fine, 99.0, 1.0), key(GrainStyle::Fine, 3.0, 1.0), "speck size is clamped");
        let palette = mandara_theme::engine::default_palette(true);
        let toned = TileKey::new(&Grain { tint: GrainTint::Surface, ..Grain::with_style(GrainStyle::Film) }, 1.0, &palette).unwrap();
        assert!(toned.tones.is_some() && key(GrainStyle::Film, 1.0, 1.0).tones.is_none());
    }

    #[test]
    fn the_cache_is_bounded_and_least_recently_used_goes_first() {
        let mut cache = TileCache::default();
        let keys: Vec<TileKey> = (0..CACHE_MAX as u32 + 3).map(|s| TileKey { seed: s, ..key(GrainStyle::Fine, 1.0, 1.0) }).collect();
        for k in &keys[..CACHE_MAX] {
            assert!(cache.insert(*k, image()).is_empty());
        }
        assert!(cache.get(&keys[0]).is_some(), "touched: now the newest");
        let evicted = cache.insert(keys[CACHE_MAX], image());
        assert_eq!(evicted.len(), 1);
        assert_eq!(cache.len(), CACHE_MAX);
        assert!(cache.get(&keys[0]).is_some() && cache.get(&keys[1]).is_none(), "the oldest untouched tile went");
        assert!(cache.insert(keys[CACHE_MAX], image()).is_empty(), "replacing a key never grows the cache");
        for k in &keys[CACHE_MAX + 1..] {
            cache.insert(*k, image());
        }
        assert_eq!(cache.len(), CACHE_MAX);
    }

    /// Every style draws a faint, balanced texture that tiles seamlessly
    /// (whole specks per tile) and differs from the others.
    #[test]
    fn styles_are_distinct_balanced_and_seamless() {
        let mut means = Vec::new();
        for style in [GrainStyle::Fine, GrainStyle::Film, GrainStyle::Paper, GrainStyle::Speckle] {
            for (scale, factor) in [(1.0, 1.0), (1.0, 1.5), (2.5, 1.25), (3.0, 2.0)] {
                let k = key(style, scale, factor);
                let px = tile_pixels(&k);
                assert_eq!(px.dimensions(), (TILE_PX, TILE_PX));
                // Specks keep their size in logical px at any scale factor.
                let speck_px = TILE_PX as f32 / k.cells() as f32 / factor;
                assert!((speck_px - scale).abs() <= scale * 0.1 + 0.05, "{style:?} {scale} @{factor}: {speck_px}");
                // The first and last speck columns are whole specks of the
                // same lattice: the tile meets itself without a seam.
                assert_eq!((TILE_PX - 1) * k.cells() / TILE_PX, k.cells() - 1);
            }
            let px = tile_pixels(&key(style, 1.0, 1.0));
            let mean = px.pixels().map(|p| f32::from(p.0[3])).sum::<f32>() / (TILE_PX * TILE_PX) as f32;
            assert!(mean > 0.0 && mean < 200.0, "{style:?}: {mean}");
            means.push(mean);
        }
        // Speckle is mostly clean; fine grain is busiest.
        assert!(means[3] < means[0], "{means:?}");
        let a = tile_pixels(&key(GrainStyle::Fine, 1.0, 1.0));
        let b = tile_pixels(&TileKey { seed: 7, ..key(GrainStyle::Fine, 1.0, 1.0) });
        assert_ne!(a, b, "the seed changes the field");
    }

    #[test]
    fn value_noise_wraps_at_its_period() {
        for p in [1, 3, 9] {
            for y in 0..p {
                let (a, b) = (value(5, 0.0, y as f32, p), value(5, p as f32, y as f32, p));
                assert!((a - b).abs() < 1e-6, "period {p}");
            }
        }
    }

    #[test]
    fn contrast_spreads_the_specks() {
        let soft = tile_pixels(&TileKey { contrast_q: (0.5 * CONTRAST_STEPS) as u16, ..key(GrainStyle::Fine, 1.0, 1.0) });
        let hard = tile_pixels(&TileKey { contrast_q: (1.5 * CONTRAST_STEPS) as u16, ..key(GrainStyle::Fine, 1.0, 1.0) });
        let mean = |i: &image::RgbaImage| i.pixels().map(|p| f32::from(p.0[3])).sum::<f32>();
        assert!(mean(&hard) > mean(&soft));
    }
}
