//! Mesh gradient: a few color points whose Gaussian influences are blended
//! as a weighted average in OKLab (a true mesh gradient, not stacked
//! translucent blobs), over a deep ground, with a slow organic domain warp
//! and an optional swirl.
//!
//! Each point also carries a height, blended the same way, so the mesh has
//! a gentle relief like draped silk: it is lit softly from the light angle
//! (depth without detail), catches a broad, grain-free sheen where it
//! turns toward the light, and the brightest color blooms with a soft
//! luminous glow.

use crate::color::Lab;
use crate::fmath::{cos, exp, pow, sin};
use crate::noise::Fractal;
use crate::render::{Ctx, Scene};

/// Blend radius (screen heights) at 0 % and 100 % blend size.
const SPREAD_MIN: f32 = 0.14;
const SPREAD_MAX: f32 = 0.62;
/// Warp displacement at 100 %, in screen heights.
const WARP_AMOUNT: f32 = 0.32;
/// Frequency of the warp field (cycles per screen height).
const WARP_FREQ: f32 = 0.9;
/// Warp octaves, and the offset decorrelating its second field.
const WARP_OCTAVES: u32 = 4;
const WARP_Y_OFFSET: (f32, f32) = (3.7, -1.3);
/// Swirl at ±100 % turns the centre by this many radians…
const SWIRL_TURN: f32 = 2.4;
/// …fading out over this radius.
const SWIRL_RADIUS: f32 = 0.55;
/// The deep ground's weight at 100 % shadow depth.
const DEPTH_WEIGHT: f32 = 0.55;
/// Color points sit up to this far outside the screen edge.
const BLEED: f32 = 0.12;
/// Orbit radius of a point in a video loop (screen heights).
const ORBIT: f32 = 0.07;
/// Relief: slope of the silk per unit of height difference at 100 % depth,
/// the finite-difference step (screen heights), and the lightness the
/// soft light adds or takes per unit of facing.
const RELIEF: f32 = 1.2;
const RELIEF_EPS: f32 = 0.008;
const RELIEF_SHADE: f32 = 0.4;
/// Soft folds in the silk: frequency (per screen height), octaves, octave
/// gain and height (relative to the mesh's own relief).
const FOLD_FREQ: f32 = 1.1;
const FOLD_OCTAVES: u32 = 2;
const FOLD_GAIN: f32 = 0.3;
const FOLD_HEIGHT: f32 = 0.12;
/// Folds run long: they are stretched this much along the light's
/// perpendicular, like draped cloth.
const FOLD_STRETCH: f32 = 3.0;
/// The light is raised this far toward the viewer (the rest is the light
/// angle): a soft, high light.
const LIGHT_RISE: f32 = 0.8;
/// Silk sheen: the half-vector exponent (broad) and the light it adds at
/// 100 % sheen.
const SHEEN_POWER: f32 = 24.0;
const SHEEN_LIGHT: f32 = 0.6;
/// The sheen's color: lift and chroma kept.
const SHEEN_TINT: (f32, f32) = (0.25, 0.7);
/// The ground's height (the points take random heights in 0–1).
const GROUND_HEIGHT: f32 = -0.5;
/// Luminous bloom: radius (screen heights), the light it adds at 100 %
/// sheen, and how far toward white its color is lifted.
const BLOOM_RADIUS: f32 = 0.4;
const BLOOM_LIGHT: f32 = 0.45;
const BLOOM_WHITE: f32 = 0.35;
/// Light mode keeps this share of the sheen and bloom.
const LIGHT_SOFTEN: f32 = 0.5;

struct Point {
    x: f32,
    y: f32,
    color: Lab,
    chroma: f32,
    height: f32,
}

pub struct Mesh {
    points: Vec<Point>,
    inv_two_sigma2: f32,
    ground: Lab,
    ground_weight: f32,
    warp: (Fractal, Fractal),
    warp_amount: f32,
    z: f32,
    swirl: f32,
    sheen: f32,
    relief: f32,
    folds: Fractal,
    light: (f32, f32, f32),
    half: (f32, f32, f32),
    bloom_at: (f32, f32),
    bloom: Lab,
    light_mode: bool,
    px: f32,
}

impl Mesh {
    pub fn new(ctx: &Ctx) -> Self {
        let n = ctx.n("points");
        let mut rng = ctx.rng(1);
        let pts = super::spread(&mut rng, n, ctx.aspect + 2.0 * BLEED, 1.0 + 2.0 * BLEED);
        let s = &ctx.scheme;
        // Colors: every stop above the ground gets used before any repeats,
        // in a seed-shuffled order.
        let mut order: Vec<usize> = (1..s.len()).collect();
        for i in (1..order.len()).rev() {
            order.swap(i, rng.below(i + 1));
        }
        let points: Vec<Point> = pts
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| {
                let (dx, dy) = ctx.orbit(1.0, rng.f(), ORBIT);
                let color = s.stop(order[i % order.len()]);
                Point { x: x + dx, y: y + dy, color, chroma: color.chroma(), height: rng.f() }
            })
            .collect();
        let spread = SPREAD_MIN + (SPREAD_MAX - SPREAD_MIN) * ctx.u("spread");
        // The bloom sits on the brightest point.
        let brightest = points.iter().max_by(|a: &&Point, b: &&Point| a.color.l.total_cmp(&b.color.l)).map_or((0.0, 0.0, s.bright()), |p| (p.x, p.y, p.color));
        let (lx, ly) = ctx.light_dir();
        let flat = (1.0 - LIGHT_RISE * LIGHT_RISE).sqrt();
        let light = (lx * flat, ly * flat, LIGHT_RISE);
        let hl = (light.0 * light.0 + light.1 * light.1 + (light.2 + 1.0) * (light.2 + 1.0)).sqrt();
        Mesh {
            points,
            inv_two_sigma2: 1.0 / (2.0 * spread * spread),
            ground: s.deep(),
            ground_weight: ctx.u("depth") * DEPTH_WEIGHT,
            warp: (Fractal::new(ctx.seed32(2), WARP_FREQ, WARP_OCTAVES), Fractal::new(ctx.seed32(3), WARP_FREQ, WARP_OCTAVES)),
            warp_amount: ctx.u("warp") * WARP_AMOUNT,
            z: ctx.z(crate::noise::LOOP_PERIOD),
            swirl: ctx.u("swirl") * SWIRL_TURN,
            sheen: ctx.u("sheen") * if s.light { LIGHT_SOFTEN } else { 1.0 },
            relief: ctx.u("depth") * RELIEF,
            folds: Fractal::new(ctx.seed32(4), FOLD_FREQ, FOLD_OCTAVES).gain(FOLD_GAIN),
            light,
            half: (light.0 / hl, light.1 / hl, (light.2 + 1.0) / hl),
            bloom_at: (brightest.0, brightest.1),
            bloom: brightest.2.mix(Lab::new(1.0, 0.0, 0.0), BLOOM_WHITE),
            light_mode: s.light,
            px: ctx.px,
        }
    }
}

impl Mesh {
    /// The blended color and height at warped `(x, y)`.
    #[inline]
    fn blend(&self, x: f32, y: f32) -> (Lab, f32) {
        let mut sum = self.ground.scale(self.ground_weight);
        let mut wsum = self.ground_weight;
        let mut chroma = self.ground.chroma() * self.ground_weight;
        let mut height = GROUND_HEIGHT * self.ground_weight;
        for p in &self.points {
            let d2 = (x - p.x) * (x - p.x) + (y - p.y) * (y - p.y);
            let w = exp(-d2 * self.inv_two_sigma2);
            sum = sum.add(p.color.scale(w));
            chroma += p.chroma * w;
            height += p.height * w;
            wsum += w;
        }
        let inv = 1.0 / wsum.max(1e-6);
        // Weighted chroma, so where two hues meet the blend stays vivid.
        (sum.scale(inv).with_chroma(chroma * inv), height * inv)
    }

    /// The blended height alone (for the relief's slope).
    #[inline]
    fn height(&self, x: f32, y: f32) -> f32 {
        let (mut height, mut wsum) = (GROUND_HEIGHT * self.ground_weight, self.ground_weight);
        for p in &self.points {
            let w = exp(-((x - p.x) * (x - p.x) + (y - p.y) * (y - p.y)) * self.inv_two_sigma2);
            height += p.height * w;
            wsum += w;
        }
        height / wsum.max(1e-6)
    }

    /// The silk's folds at `(x, y)`.
    #[inline]
    fn fold(&self, x: f32, y: f32) -> f32 {
        let (lx, ly) = (self.light.0, self.light.1);
        let ll = (lx * lx + ly * ly).sqrt().max(1e-6);
        let (across, along) = ((x * lx + y * ly) / ll, (y * lx - x * ly) / ll);
        FOLD_HEIGHT * self.folds.at(across, along / FOLD_STRETCH, self.z, self.px)
    }
}

impl Scene for Mesh {
    fn shade(&self, x: f32, y: f32) -> Lab {
        let (sx, sy) = (x, y);
        let wx = self.warp.0.at(x, y, self.z, self.px);
        let wy = self.warp.1.at(x + WARP_Y_OFFSET.0, y + WARP_Y_OFFSET.1, self.z, self.px);
        let (mut x, mut y) = (x + wx * self.warp_amount, y + wy * self.warp_amount);
        if self.swirl != 0.0 {
            let r2 = x * x + y * y;
            let a = self.swirl * exp(-r2 / (SWIRL_RADIUS * SWIRL_RADIUS));
            let (s, c) = (sin(a), cos(a));
            (x, y) = (x * c - y * s, x * s + y * c);
        }
        let (mut c, h) = self.blend(x, y);
        if self.relief > 0.0 || self.sheen > 0.0 {
            // The silk's relief, lit softly: depth without detail.
            let f = self.fold(x, y);
            let hx = (self.height(x + RELIEF_EPS, y) + self.fold(x + RELIEF_EPS, y) - h - f) / RELIEF_EPS * self.relief;
            let hy = (self.height(x, y + RELIEF_EPS) + self.fold(x, y + RELIEF_EPS) - h - f) / RELIEF_EPS * self.relief;
            let nl = (hx * hx + hy * hy + 1.0).sqrt();
            let (nx, ny, nz) = (-hx / nl, -hy / nl, 1.0 / nl);
            let (lx, ly, lz) = self.light;
            c.l += RELIEF_SHADE * (nx * lx + ny * ly + nz * lz - lz);
            // A broad sheen where the silk turns toward the light.
            let (ax, ay, az) = self.half;
            let sheen = pow((nx * ax + ny * ay + nz * az).max(0.0), SHEEN_POWER) - pow(az, SHEEN_POWER);
            c = super::light(c, c.lighten(SHEEN_TINT.0).saturate(SHEEN_TINT.1), self.sheen * SHEEN_LIGHT * sheen.max(0.0));
        }
        // The brightest color blooms softly (in screen space, unwarped).
        let (bx, by) = (sx - self.bloom_at.0, sy - self.bloom_at.1);
        let bloom = self.sheen * BLOOM_LIGHT * exp(-(bx * bx + by * by) / (BLOOM_RADIUS * BLOOM_RADIUS));
        if self.light_mode { c.mix_vivid(self.bloom, bloom) } else { super::light(c, self.bloom, bloom) }
    }
}
