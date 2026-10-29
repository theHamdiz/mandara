//! The styles. Each builds a [`crate::render::Scene`] from a
//! [`crate::render::Ctx`] once per render (layout, colors, noise seeds) and
//! then shades pixels independently. Coordinates are in screen heights, so
//! a 1080p and an 8K render are the same picture; antialiasing comes from
//! analytic edge coverage over `ctx.px` (one pixel).

pub mod aurora;
pub mod bokeh;
pub mod contours;
pub mod corniche;
pub mod dunes;
pub mod flow;
pub mod glass;
pub mod landscape;
pub mod liquid;
pub mod low_poly;
pub mod mesh;
pub mod waves;

use crate::color::Lab;
use crate::fmath::{cos, exp};
use crate::noise::{Rng, hash3, unit};

/// Star cells are this many screen heights wide.
const STAR_CELL: f32 = 0.022;
/// A star's core radius (screen heights) before the pixel floor.
const STAR_RADIUS: f32 = 0.0007;
/// Stars are never drawn smaller than this many pixels (they would flicker
/// in and out between resolutions).
const STAR_MIN_PX: f32 = 0.7;
/// Brightness distribution: higher = fewer bright stars.
const STAR_FALLOFF: f32 = 3.0;
/// How much a star's brightness swings when twinkling in a video loop.
const TWINKLE: f32 = 0.35;
/// A star sits at least this share of its cell from the cell's edge, within
/// the remaining room…
const STAR_MARGIN: f32 = 0.2;
const STAR_ROOM: f32 = 0.6;
/// …and its brightness spans from this floor by this much.
const STAR_FLOOR: f32 = 0.25;
const STAR_SPAN: f32 = 0.75;
/// A glow's energy split between its tight core and its wide bloom.
const GLOW_CORE_SHARE: f32 = 0.55;
const GLOW_BLOOM_SHARE: f32 = 0.45;
/// Candidates per point for best-candidate (Mitchell) spreading.
const SPREAD_CANDIDATES: usize = 12;

/// Brightness (0–1) of the starfield at `(x, y)`: one hashed star per cell,
/// a power-law brightness spread and a seamless twinkle at loop phase `t`.
pub(crate) fn stars(seed: u32, x: f32, y: f32, density: f32, px: f32, t: f32) -> f32 {
    let (cx, cy) = ((x / STAR_CELL).floor(), (y / STAR_CELL).floor());
    let (ix, iy) = (cx as i32, cy as i32);
    let h = hash3(ix, iy, 0, seed);
    if unit(h) > density {
        return 0.0;
    }
    let h2 = hash3(ix, iy, 1, seed);
    let h3 = hash3(ix, iy, 2, seed);
    // Keep the star inside its cell so neighbours never need checking.
    let sx = (cx + STAR_MARGIN + STAR_ROOM * unit(h2)) * STAR_CELL;
    let sy = (cy + STAR_MARGIN + STAR_ROOM * unit(h3)) * STAR_CELL;
    let mag = crate::fmath::pow(unit(hash3(ix, iy, 3, seed)), STAR_FALLOFF);
    let r = (STAR_RADIUS * (0.5 + mag)).max(px * STAR_MIN_PX);
    let d2 = (x - sx) * (x - sx) + (y - sy) * (y - sy);
    // Energy-preserving: a star drawn bigger than its core (pixel floor) is
    // dimmed so it looks the same at every resolution.
    let energy = (STAR_RADIUS * (0.5 + mag) / r).powi(2).min(1.0);
    let twinkle = 1.0 - TWINKLE * 0.5 * (1.0 + cos(std::f32::consts::TAU * (t + unit(hash3(ix, iy, 4, seed)))));
    (STAR_FLOOR + STAR_SPAN * mag) * energy * twinkle * exp(-d2 / (r * r))
}

/// `n` points spread evenly (best-candidate sampling) over the rectangle
/// `x ∈ [-w/2, w/2]`, `y ∈ [-h/2, h/2]`.
pub(crate) fn spread(rng: &mut Rng, n: usize, w: f32, h: f32) -> Vec<(f32, f32)> {
    let mut pts: Vec<(f32, f32)> = Vec::with_capacity(n);
    for _ in 0..n {
        let mut best = (0.0, 0.0);
        let mut best_d = -1.0;
        for _ in 0..SPREAD_CANDIDATES {
            let c = (rng.range(-w / 2.0, w / 2.0), rng.range(-h / 2.0, h / 2.0));
            let d = pts.iter().map(|p| (p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).fold(f32::MAX, f32::min);
            if d > best_d {
                best_d = d;
                best = c;
            }
        }
        pts.push(best);
    }
    pts
}

/// OKLCh hue (radians) that [`coolest`] leans toward: sky blue.
const COOL_HUE: f32 = 4.4;

/// A scheme's coolest colorful stop (the one nearest blue in hue, weighted
/// by chroma): twilight skies and shadows on pale ground.
pub(crate) fn coolest(s: &crate::palette::Scheme) -> Lab {
    let cool = |c: Lab| c.chroma() * (1.0 + cos(c.hue() - COOL_HUE));
    (1..s.len()).map(|i| s.stop(i)).max_by(|a, b| cool(*a).total_cmp(&cool(*b))).unwrap_or(s.bright())
}

/// A soft glow falloff: two exponentials (a tight core and a wide bloom).
#[inline]
pub(crate) fn glow(d: f32, core: f32, bloom: f32) -> f32 {
    GLOW_CORE_SHARE * exp(-d / core) + GLOW_BLOOM_SHARE * exp(-d / bloom)
}

/// Light added over `c` in linear light (glows, stars, glints, lamps).
#[inline]
pub(crate) fn light(c: Lab, light: Lab, amount: f32) -> Lab {
    c.add_light(light, amount)
}

/// How much darker than its color a light-mode "ink" stroke is laid down.
const INK_DARKEN: f32 = 0.12;
/// Ink coverage at full amount.
const INK_STRENGTH: f32 = 0.85;
/// Ink is laid down this much more strongly than the same mark as light
/// (a dark mark on a pale ground needs more coverage to read).
const INK_GAIN: f32 = 2.5;

/// Light on a dark ground, ink on a pale one: in dark mode `amount` of
/// `color` is added as light; in light mode the same mark is laid down as
/// a slightly darker, vivid wash (adding light to a pale ground would
/// make it vanish).
#[inline]
pub(crate) fn mark(c: Lab, color: Lab, amount: f32, light_mode: bool) -> Lab {
    if !light_mode {
        return light(c, color, amount);
    }
    if amount <= 0.0 {
        return c;
    }
    c.mix_vivid(color.lighten(-INK_DARKEN), INK_STRENGTH * (1.0 - exp(-amount * INK_GAIN)))
}
