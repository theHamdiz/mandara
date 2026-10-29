//! Landscape: a twilight sky with a sun (or a pale moon), and mountain
//! ridges receding into atmospheric haze: each farther ridge is lighter and
//! bluer, mist pools in the valleys, crests facing the sun catch a rim of
//! light, and stars come out above a dark sky.

use crate::color::Lab;
use crate::fmath::{exp, lerp, pow, smoothstep};
use crate::noise::Fractal;
use crate::render::{Ctx, Scene};

/// Sun disc radius (screen heights) at 0 % and 100 % size.
const SUN_MIN: f32 = 0.018;
const SUN_MAX: f32 = 0.075;
/// Sun glow: core and bloom falloff distances (screen heights).
const GLOW_CORE: f32 = 0.07;
const GLOW_BLOOM: f32 = 0.38;
/// Horizontal band of light along the horizon under the sun.
const HORIZON_BAND: f32 = 0.06;
/// Light the sun glow and the horizon band add at 100 % glow.
const GLOW_GAIN: f32 = 0.7;
const BAND_GAIN: f32 = 0.25;
/// The farthest ridge's crest sits this far above the horizon…
const FAR_LIFT: f32 = 0.10;
/// …and the nearest reaches this far below it (fraction of the ground).
const NEAR_DROP: f32 = 0.62;
/// Ridge height: amplitude of the farthest and nearest ridges.
const AMP_FAR: f32 = 0.05;
const AMP_NEAR: f32 = 0.16;
/// Ridge profile frequency (cycles per screen height), far and near.
const FREQ_FAR: f32 = 2.6;
const FREQ_NEAR: f32 = 0.9;
/// Valley mist: how far below a crest it starts and reaches full density.
const MIST_START: f32 = 0.02;
const MIST_DEPTH: f32 = 0.22;
/// Rim light on sunward crests: thickness (screen heights) and strength.
const RIM_WIDTH: f32 = 0.002;
const RIM_STRENGTH: f32 = 0.18;
/// Mist drifts this far (screen heights) around its loop.
const MIST_DRIFT: f32 = 0.08;
/// Edge softness of the far ridges relative to the near ones (haze blur).
const FAR_SOFT: f32 = 1.5;
/// …and of the nearest.
const NEAR_SOFT: f32 = 0.6;
/// In light mode the nearest ridge takes the ramp only this far (the
/// most saturated accent reads muddy as a large foreground).
const LIGHT_NEAR_RAMP: f32 = 0.7;
/// …and the farthest ridge starts this far up it (light, dark).
const LIGHT_FAR_RAMP: f32 = 0.3;
const FAR_RAMP: f32 = 0.32;
/// The sun's reach across the screen at ±100 % (share of the width) and
/// the share of the sky height it climbs at 100 % sun height.
const SUN_REACH: f32 = 0.42;
const SUN_CLIMB: f32 = 0.9;
/// Sky: ramp tones of the top and horizon and the sun's share of it (dark;
/// light mode), the gradient curve, the sky's share of the haze color, and
/// how narrow the horizon band is across the screen.
const SKY_TOP: f32 = 0.02;
const SKY_LOW: f32 = 0.55;
const SKY_SUN: f32 = 0.3;
const LIGHT_SKY_TOP: f32 = 0.12;
const LIGHT_SKY_SUN: f32 = 0.25;
const SKY_CURVE: f32 = 1.6;
const HAZE_SKY: f32 = 0.25;
const BAND_NARROW: f32 = 4.0;
/// The sun disc is lifted this much above the sun color.
const DISC_LIFT: f32 = 0.05;
/// Stars: color, gain, density share, and the sky height over which they
/// come out above the horizon.
const STARLIGHT: Lab = Lab::new(0.97, 0.0, 0.01);
const STAR_GAIN: f32 = 2.5;
const STAR_DENSITY: f32 = 0.6;
const STAR_FADE: f32 = 0.35;
/// Ridges: the near ridges' drop curve, amplitude at 0 % ruggedness and its
/// gain per unit, frequency scale range, octaves, gain at 0 % and its gain
/// per unit of ruggedness, the noise rows per unit of depth, and how sharp
/// ridged peaks are.
const DROP_CURVE: f32 = 1.6;
const AMP_CALM: f32 = 0.6;
const AMP_RUGGED: f32 = 0.8;
const FREQ_SCALE: (f32, f32) = (0.85, 1.15);
const RIDGE_OCTAVES: u32 = 7;
const RIDGE_GAIN: f32 = 0.45;
const RIDGE_GAIN_RUGGED: f32 = 0.12;
const RIDGE_ROWS: f32 = 7.0;
const RIDGE_SHARPNESS: f32 = 1.4;
/// Haze falls off with nearness along this curve.
const HAZE_CURVE: f32 = 1.2;
/// Mist: the share the nearest ridge loses, noise frequency and octaves,
/// and its density base and variation.
const MIST_NEAR_FADE: f32 = 0.7;
const MIST_FREQ: f32 = 3.0;
const MIST_OCTAVES: u32 = 4;
const MIST_BASE: f32 = 0.65;
const MIST_VARY: f32 = 0.35;
/// Rim light reaches this share of the width from the sun; light mode keeps
/// this share of it.
const RIM_REACH: f32 = 0.25;
const LIGHT_RIM: f32 = 0.4;

struct Ridge {
    base: f32,
    amp: f32,
    noise: Fractal,
    ridged: f32,
    color: Lab,
    haze: f32,
    mist: f32,
    soft: f32,
}

pub struct Landscape {
    horizon: f32,
    sky_top: Lab,
    sky_low: Lab,
    sun: Lab,
    sun_at: (f32, f32),
    sun_r: f32,
    glow: f32,
    haze_color: Lab,
    ridges: Vec<Ridge>,
    stars: f32,
    star_seed: u32,
    mist_noise: Fractal,
    mist_off: (f32, f32),
    sun_width: f32,
    ctx_px: f32,
    t: f32,
    edge_soft: f32,
    light: bool,
}

impl Landscape {
    pub fn new(ctx: &Ctx) -> Self {
        let s = &ctx.scheme;
        let horizon = ctx.u("horizon") - 0.5;
        let sun = s.sun();
        let sun_at = (ctx.u("sun_x") * ctx.aspect * SUN_REACH, horizon - ctx.u("sun_height") * (horizon + 0.5) * SUN_CLIMB);
        let (sky_top, sky_low) = if s.light { (s.ramp(LIGHT_SKY_TOP), s.ramp(0.0).mix(sun, LIGHT_SKY_SUN)) } else { (s.ramp(SKY_TOP), s.ramp(SKY_LOW).mix(sun, SKY_SUN)) };
        let haze_color = sky_low.mix(sky_top, HAZE_SKY);
        let n = ctx.n("layers");
        let rug = ctx.u("ruggedness");
        let mut rng = ctx.rng(5);
        let ridges = (0..n)
            .map(|i| {
                let k = if n > 1 { i as f32 / (n - 1) as f32 } else { 1.0 };
                let ground = 0.5 - horizon;
                let base = horizon - FAR_LIFT * (1.0 - k) + ground * NEAR_DROP * pow(k, DROP_CURVE);
                let near = if s.light { s.ramp(lerp(LIGHT_FAR_RAMP, LIGHT_NEAR_RAMP, k)) } else { s.ramp(lerp(FAR_RAMP, 0.0, k)) };
                Ridge {
                    base,
                    amp: lerp(AMP_FAR, AMP_NEAR, k) * (AMP_CALM + AMP_RUGGED * rug),
                    noise: Fractal::new(rng.seed32(), lerp(FREQ_FAR, FREQ_NEAR, k) * rng.range(FREQ_SCALE.0, FREQ_SCALE.1), RIDGE_OCTAVES).gain(RIDGE_GAIN + RIDGE_GAIN_RUGGED * rug),
                    ridged: rug,
                    color: near,
                    haze: ctx.u("haze") * pow(1.0 - k, HAZE_CURVE),
                    mist: ctx.u("mist") * (1.0 - MIST_NEAR_FADE * k),
                    soft: lerp(FAR_SOFT, NEAR_SOFT, k),
                }
            })
            .collect();
        Landscape {
            horizon,
            sky_top,
            sky_low,
            sun,
            sun_at,
            sun_r: lerp(SUN_MIN, SUN_MAX, ctx.u("sun_size")),
            glow: ctx.u("glow"),
            haze_color,
            ridges,
            stars: if s.light { 0.0 } else { ctx.u("stars") },
            star_seed: ctx.seed32(6),
            mist_noise: Fractal::new(ctx.seed32(7), MIST_FREQ, MIST_OCTAVES),
            mist_off: ctx.orbit(1.0, 0.0, MIST_DRIFT),
            sun_width: ctx.aspect,
            ctx_px: ctx.px,
            t: ctx.t,
            edge_soft: ctx.edge_width(1.0),
            light: s.light,
        }
    }

    fn crest(&self, r: &Ridge, x: f32) -> f32 {
        let n = r.noise.at(x, r.base * RIDGE_ROWS, 0.0, self.ctx_px);
        // Ridged noise (sharp peaks) blended with smooth rolling hills.
        let ridged = 1.0 - (n.abs() * RIDGE_SHARPNESS).min(1.0);
        let h = lerp(0.5 + 0.5 * n, ridged * ridged, r.ridged);
        r.base - r.amp * h
    }
}

impl Scene for Landscape {
    fn shade(&self, x: f32, y: f32) -> Lab {
        // Sky.
        let v = ((y + 0.5) / (self.horizon + 0.5)).clamp(0.0, 1.0);
        let mut c = self.sky_top.mix_vivid(self.sky_low, pow(v, SKY_CURVE));
        let (dx, dy) = (x - self.sun_at.0, y - self.sun_at.1);
        let d = (dx * dx + dy * dy).sqrt();
        let band = exp(-((y - self.horizon) / HORIZON_BAND).powi(2)) * exp(-(dx / self.sun_width).powi(2) * BAND_NARROW);
        c = super::light(c, self.sun, self.glow * (GLOW_GAIN * super::glow(d, GLOW_CORE, GLOW_BLOOM) + BAND_GAIN * band));
        if self.stars > 0.0 && y < self.horizon {
            let fade = smoothstep(self.horizon, self.horizon - STAR_FADE, y) * (1.0 - smoothstep(0.5, 0.0, d));
            let s = super::stars(self.star_seed, x, y, self.stars * STAR_DENSITY, self.ctx_px, self.t) * fade;
            c = super::light(c, STARLIGHT, s * STAR_GAIN);
        }
        let disc = 1.0 - smoothstep(self.sun_r - self.edge_soft, self.sun_r + self.edge_soft, d);
        c = c.mix(self.sun.lighten(DISC_LIFT), disc);
        // Ridges, far to near.
        for r in &self.ridges {
            let crest = self.crest(r, x);
            let below = y - crest;
            let w = self.edge_soft * r.soft;
            if below < -w {
                continue;
            }
            let cov = smoothstep(-w, w, below);
            let mut rc = r.color.mix(self.haze_color, r.haze);
            // Mist pools below each crest, drifting in a loop.
            let m = smoothstep(MIST_START, MIST_DEPTH, below) * (MIST_BASE + MIST_VARY * self.mist_noise.at(x + self.mist_off.0, y * 2.0 + self.mist_off.1, 0.0, self.ctx_px));
            rc = rc.mix(self.haze_color, r.mist * m.clamp(0.0, 1.0));
            // Crests facing the sun catch a thin rim of light.
            let toward = exp(-((x - self.sun_at.0) / (self.sun_width * RIM_REACH)).powi(2));
            let rim = exp(-below.max(0.0) / RIM_WIDTH) * RIM_STRENGTH * self.glow * toward * if self.light { LIGHT_RIM } else { 1.0 };
            rc = super::light(rc, self.sun, rim);
            c = c.mix(rc, cov);
        }
        c
    }
}
