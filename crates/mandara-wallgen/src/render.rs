//! The raster core: per-pixel scene evaluation, the shared finishing pass
//! (tone, color, vignette) and quantization, split into row bands across
//! every core.
//!
//! Each pixel is a pure function of the recipe, the size and the loop
//! phase, so the output is identical whatever the thread count or band
//! order, and nothing larger than the output image is ever allocated (an
//! 8K render needs its 100 MB RGB buffer and nothing else).

use std::sync::Mutex;

use image::RgbImage;

use crate::color::Lab;
use crate::dither;
use crate::fmath::{cos, sin, smoothstep};
use crate::noise::Rng;
use crate::palette::Scheme;
use crate::params::*;
use crate::recipe::Recipe;
use crate::style::Style;

/// Rows per work unit handed to a thread (each band also shades the row
/// above it for the dither's gradient, so longer bands waste less).
const BAND_ROWS: usize = 16;
/// Dithering spreads each 8-bit band edge over this share of the image
/// height (16 px at 4K: about a quarter of a degree on a desktop display,
/// wide enough that the eye sees no step), and never fewer pixels than
/// the next constant…
const DITHER_SPREAD: f32 = 0.0075;
const DITHER_SPREAD_MIN_PX: f32 = 6.0;
/// …and fades out on gradients steeper than these (levels per pixel),
/// where bands are narrower than a pixel.
const DITHER_EDGE_FROM: f32 = 1.0;
const DITHER_EDGE_TO: f32 = 3.0;
/// Antialiasing: an edge ramps over this many pixels.
pub(crate) const AA_PIXELS: f32 = 1.0;
/// At 100 % softness an edge is blurred over this many screen heights.
const SOFT_EDGE: f32 = 0.006;
/// Grain at 100 % has this standard deviation, in 8-bit levels.
const GRAIN_LEVELS: f32 = 5.0;
/// Contrast pivots around this OKLab lightness.
const CONTRAST_PIVOT: f32 = 0.45;
/// Temperature at ±100 shifts OKLab `b` (blue ↔ yellow) by this much…
const TEMPERATURE_B: f32 = 0.03;
/// …and `a` (green ↔ magenta) by this much, toward warm red.
const TEMPERATURE_A: f32 = 0.008;
/// The vignette starts at this radius (fraction of the half-diagonal)…
const VIGNETTE_START: f32 = 0.35;
/// …and darkens lightness by up to this factor at 100 %.
const VIGNETTE_DEPTH: f32 = 0.55;
/// Light wallpapers take this share of the vignette (darkening a pale
/// ground looks dirty, not moody).
const VIGNETTE_LIGHT: f32 = 0.35;

/// Everything a style needs to build its scene for one render.
pub(crate) struct Ctx<'a> {
    pub recipe: &'a Recipe,
    pub scheme: Scheme,
    /// Width / height.
    pub aspect: f32,
    /// One pixel, in screen heights (after zoom).
    pub px: f32,
    /// Loop phase in [0, 1).
    pub t: f32,
    /// Live preview: styles may skip refinements that only matter at full size.
    pub preview: bool,
}

impl Ctx<'_> {
    /// Parameter value (default when missing, clamped).
    pub fn p(&self, key: &str) -> f32 {
        self.recipe.param(key)
    }

    /// A percent parameter as 0–1.
    pub fn u(&self, key: &str) -> f32 {
        self.recipe.param(key) / 100.0
    }

    /// A count parameter.
    pub fn n(&self, key: &str) -> usize {
        self.recipe.param(key).max(0.0) as usize
    }

    /// A layout random stream, distinct per `salt`.
    pub fn rng(&self, salt: u64) -> Rng {
        Rng::new(self.recipe.seed ^ self.recipe.style.salt() ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15))
    }

    /// A noise-field seed, distinct per `salt`.
    pub fn seed32(&self, salt: u64) -> u32 {
        self.rng(salt).seed32()
    }

    /// Half-width of an antialiased edge: one pixel plus `soft` × the
    /// softness blur (styles ramp coverage with `smoothstep(-w, w, d)`).
    pub fn edge_width(&self, soft: f32) -> f32 {
        self.px * AA_PIXELS + soft * self.u(SOFTNESS) * SOFT_EDGE
    }

    /// Unit vector pointing toward the light (screen axes, y down).
    pub fn light_dir(&self) -> (f32, f32) {
        let a = self.p(LIGHT_ANGLE).to_radians();
        (cos(a), -sin(a))
    }

    /// Motion amplitude (0–2).
    pub fn motion(&self) -> f32 {
        self.u(MOTION)
    }

    /// `k` whole cycles per loop at `phase` (turns): angle in radians.
    pub fn angle(&self, k: f32, phase: f32) -> f32 {
        std::f32::consts::TAU * (k * self.t + phase)
    }

    /// A point orbiting `k` times per loop on a circle of `radius` (scaled
    /// by the motion setting): seamless at `t = 1`.
    pub fn orbit(&self, k: f32, phase: f32, radius: f32) -> (f32, f32) {
        let a = self.angle(k, phase);
        let r = radius * self.motion();
        (r * cos(a), r * sin(a))
    }

    /// Noise depth for a field that morphs once around its `period` per loop.
    pub fn z(&self, period: i32) -> f32 {
        self.t * period as f32
    }
}

/// A style's per-render state: evaluates a pixel.
pub(crate) trait Scene: Sync {
    /// OKLab color at `(x, y)` in screen heights: origin at the screen's
    /// centre, `y` down, `x ∈ [-aspect/2, aspect/2]`, `y ∈ [-½, ½]`.
    fn shade(&self, x: f32, y: f32) -> Lab;
}

fn build<'a>(ctx: &'a Ctx<'a>) -> Box<dyn Scene + 'a> {
    use crate::styles::*;
    match ctx.recipe.style {
        Style::Mesh => Box::new(mesh::Mesh::new(ctx)),
        Style::Landscape => Box::new(landscape::Landscape::new(ctx)),
        Style::Corniche => Box::new(corniche::Corniche::new(ctx)),
        Style::Aurora => Box::new(aurora::Aurora::new(ctx)),
        Style::Dunes => Box::new(dunes::Dunes::new(ctx)),
        Style::Flow => Box::new(flow::Flow::new(ctx)),
        Style::Contours => Box::new(contours::Contours::new(ctx)),
        Style::Bokeh => Box::new(bokeh::Bokeh::new(ctx)),
        Style::LowPoly => Box::new(low_poly::LowPoly::new(ctx)),
        Style::Liquid => Box::new(liquid::Liquid::new(ctx)),
        Style::Waves => Box::new(waves::Waves::new(ctx)),
        Style::Glass => Box::new(glass::Glass::new(ctx)),
    }
}

/// The common finishing pass, precomputed from the recipe.
struct Finish {
    exposure: f32,
    contrast: f32,
    saturation: f32,
    hue: (f32, f32),
    warm: (f32, f32),
    vignette: f32,
    grain: f32,
    dither: f32,
}

impl Finish {
    fn new(r: &Recipe) -> Self {
        let hue = r.param(HUE_SHIFT).to_radians();
        let temp = r.param(TEMPERATURE) / 100.0;
        let vignette = r.param(VIGNETTE) / 100.0 * VIGNETTE_DEPTH * if r.light { VIGNETTE_LIGHT } else { 1.0 };
        Finish {
            // Exposure is in stops of linear light; OKLab L is its cube root.
            exposure: crate::fmath::exp2(r.param(EXPOSURE) / 3.0),
            contrast: r.param(CONTRAST) / 100.0,
            saturation: r.param(SATURATION) / 100.0,
            hue: (cos(hue), sin(hue)),
            warm: (temp * TEMPERATURE_A, temp * TEMPERATURE_B),
            vignette,
            grain: r.param(GRAIN) / 100.0 * GRAIN_LEVELS,
            dither: r.param(DITHER) / 100.0,
        }
    }

    /// Tone and color in OKLab; `r` is the distance from the centre as a
    /// fraction of the half-diagonal.
    #[inline]
    fn apply(&self, c: Lab, r: f32) -> Lab {
        let mut c = c.rotate(self.hue.0, self.hue.1);
        c.l = (c.l * self.exposure - CONTRAST_PIVOT) * self.contrast + CONTRAST_PIVOT;
        c.a = c.a * self.saturation + self.warm.0;
        c.b = c.b * self.saturation + self.warm.1;
        if self.vignette > 0.0 {
            // A smoothstep raised to 1.5: slow to start, firm in the corners.
            let v = smoothstep(VIGNETTE_START, 1.0, r);
            c.l *= 1.0 - self.vignette * v * v.sqrt();
        }
        c
    }

    /// Quantizes an encoded [0, 1] value to 8 bits with the blue-noise
    /// threshold (and grain).
    #[inline]
    fn quantize(&self, v: f32, offset: f32) -> u8 {
        (v * 255.0 + offset).floor().clamp(0.0, 255.0) as u8
    }
}

/// Splits `buf` (rows of `row_len` items) into bands and runs `f(row, data)`
/// for every row on every core.
fn par_rows<T: Send>(buf: &mut [T], row_len: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    par_bands(buf, row_len, |first, band| {
        for (i, row) in band.chunks_mut(row_len).enumerate() {
            f(first + i, row);
        }
    });
}

/// Splits `buf` (rows of `row_len` items) into bands of [`BAND_ROWS`] rows
/// and runs `f(first_row, band)` for each on every core.
fn par_bands<T: Send>(buf: &mut [T], row_len: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    if row_len == 0 || buf.is_empty() {
        return;
    }
    let bands: Vec<(usize, &mut [T])> = buf.chunks_mut(row_len * BAND_ROWS).enumerate().collect();
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(bands.len()).max(1);
    let queue = Mutex::new(bands.into_iter());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let next = queue.lock().map(|mut q| q.next()).unwrap_or(None);
                    let Some((band, data)) = next else { break };
                    f(band * BAND_ROWS, data);
                }
            });
        }
    });
}

/// A prepared render: scene, finishing pass and the pixel → scene mapping.
struct Frame<'a> {
    scene: Box<dyn Scene + 'a>,
    finish: Finish,
    width: u32,
    height: u32,
    zoom: f32,
    pan: (f32, f32),
}

impl Frame<'_> {
    /// Finished OKLab color of pixel `(x, y)`.
    #[inline]
    fn pixel(&self, x: u32, y: u32) -> Lab {
        let h = self.height as f32;
        let sx = (x as f32 + 0.5 - self.width as f32 * 0.5) / h;
        let sy = (y as f32 + 0.5) / h - 0.5;
        let c = self.scene.shade((sx - self.pan.0) / self.zoom, (sy - self.pan.1) / self.zoom);
        let aspect = self.width as f32 / h;
        let r = (sx * sx + sy * sy).sqrt() / (0.25 * aspect * aspect + 0.25).sqrt();
        self.finish.apply(c, r)
    }
}

fn with_frame<R>(recipe: &Recipe, width: u32, height: u32, t: f32, preview: bool, run: impl FnOnce(&Frame) -> R) -> R {
    let (width, height) = (width.max(1), height.max(1));
    let zoom = recipe.param(ZOOM) / 100.0;
    let ctx = Ctx {
        recipe,
        scheme: Scheme::new(&recipe.palette, recipe.light),
        aspect: width as f32 / height as f32,
        px: 1.0 / (height as f32 * zoom),
        t: t.rem_euclid(1.0),
        preview,
    };
    let frame = Frame { scene: build(&ctx), finish: Finish::new(recipe), width, height, zoom, pan: (recipe.param(PAN_X) / 100.0, recipe.param(PAN_Y) / 100.0) };
    run(&frame)
}

/// 8-bit sRGB render at loop phase `t`, dithered against banding.
///
/// The dither follows the local gradient (in 8-bit levels per pixel, from
/// the left/right and upper neighbours): a flat area has no bands to hide
/// and an edge steeper than a level per pixel has none either, so only
/// shallow ramps are dithered, and only as much as it takes to spread each
/// band edge over a [`DITHER_SPREAD`] share of the height. That hides every band while
/// leaving plateaus and edges exact, which is what keeps lossless files
/// small.
pub(crate) fn render_rgb8(recipe: &Recipe, width: u32, height: u32, t: f32, preview: bool) -> RgbImage {
    let (width, height) = (width.max(1), height.max(1));
    let w = width as usize;
    let mut img = RgbImage::new(width, height);
    with_frame(recipe, width, height, t, preview, |frame| {
        let f = &frame.finish;
        let spread = (height as f32 * DITHER_SPREAD).max(DITHER_SPREAD_MIN_PX);
        let shade_row = |y: u32, out: &mut Vec<[f32; 3]>| {
            out.clear();
            out.extend((0..width).map(|x| frame.pixel(x, y).to_srgb().map(|v| v * 255.0)));
        };
        par_bands(&mut img, w * 3, |first, band| {
            let (mut prev, mut cur) = (Vec::with_capacity(w), Vec::with_capacity(w));
            // The row above the band, for the vertical gradient.
            if first > 0 {
                shade_row(first as u32 - 1, &mut prev);
            }
            for (i, row) in band.chunks_mut(w * 3).enumerate() {
                let y = (first + i) as u32;
                shade_row(y, &mut cur);
                for (x, px) in row.chunks_exact_mut(3).enumerate() {
                    let c = cur[x];
                    let l = cur[x.saturating_sub(1)];
                    let r = cur[(x + 1).min(w - 1)];
                    let up = prev.get(x).copied().unwrap_or(c);
                    let mut g = 0.0f32;
                    for k in 0..3 {
                        g = g.max(((r[k] - l[k]) * 0.5).abs()).max((c[k] - up[k]).abs());
                    }
                    let amount = f.dither * dither_amount(g, spread);
                    // Threshold centred on ½ so no dither is plain rounding.
                    let mut offset = 0.5 + amount * (dither::threshold(x as u32, y) - 0.5);
                    if f.grain > 0.0 {
                        offset += f.grain * dither::grain(x as u32, y);
                    }
                    for k in 0..3 {
                        px[k] = f.quantize(c[k] / 255.0, offset);
                    }
                }
                std::mem::swap(&mut prev, &mut cur);
            }
        });
    });
    img
}

/// Dither strength (0–1) for a local gradient of `g` levels per pixel.
#[inline]
fn dither_amount(g: f32, spread: f32) -> f32 {
    (g * spread).min(1.0) * (1.0 - smoothstep(DITHER_EDGE_FROM, DITHER_EDGE_TO, g))
}

/// 16-bit sRGB render (no dither: the video encoder quantizes) for AV1.
pub(crate) fn render_rgb16(recipe: &Recipe, width: u32, height: u32, t: f32) -> Vec<u16> {
    let (width, height) = (width.max(1), height.max(1));
    let mut buf = vec![0u16; width as usize * height as usize * 3];
    with_frame(recipe, width, height, t, false, |frame| {
        par_rows(&mut buf, width as usize * 3, |y, row| {
            for (x, px) in row.chunks_exact_mut(3).enumerate() {
                let rgb = frame.pixel(x as u32, y as u32).to_srgb();
                for i in 0..3 {
                    px[i] = (rgb[i] * 65535.0 + 0.5) as u16;
                }
            }
        });
    });
    buf
}
