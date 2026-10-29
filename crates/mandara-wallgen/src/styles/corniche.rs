//! Corniche: the Mediterranean from Alexandria's seafront at golden hour.
//! A sky with long sunlit clouds, the sun low over the horizon, a distant
//! headland with the lights of the coast road coming on, and the sea in
//! perspective: waves compress toward the horizon (their detail fades with
//! the pixel footprint, so there is no aliasing), the water mirrors the
//! sky at grazing angles, and a glitter path runs from the sun to the shore.

use crate::color::Lab;
use crate::fmath::{exp, lerp, pow, smoothstep};
use crate::noise::{Fractal, hash3, unit};
use crate::render::{Ctx, Scene};

/// Sun disc radius (screen heights) at 0 % and 100 % size.
const SUN_MIN: f32 = 0.016;
const SUN_MAX: f32 = 0.06;
/// Sun glow falloffs (screen heights).
const GLOW_CORE: f32 = 0.06;
const GLOW_BLOOM: f32 = 0.42;
/// Perspective: world depth of a row `d` below the horizon is `DEPTH / d`.
const DEPTH: f32 = 0.06;
/// Wave field frequencies across (x) and along (depth) the view.
const WAVE_FREQ_X: f32 = 0.55;
const WAVE_FREQ_Z: f32 = 2.2;
/// How strongly wave slopes brighten (facing the sky) or darken the water.
const WAVE_SHADE: f32 = 0.10;
/// Glitter path: half-width at the horizon and growth toward the viewer.
const GLITTER_WIDTH: f32 = 0.025;
const GLITTER_SPREAD: f32 = 0.55;
/// Sparkle threshold on the fine wave field (higher = sparser glints).
const SPARKLE_FROM: f32 = 0.35;
const SPARKLE_TO: f32 = 0.8;
/// Clouds: frequencies (stretched horizontally), coverage threshold, and
/// the band of sky they live in (fraction of the sky height from the top).
const CLOUD_FREQ: f32 = 1.4;
const CLOUD_STRETCH: f32 = 5.0;
const CLOUD_TOP: f32 = 0.15;
const CLOUD_BOTTOM: f32 = 0.92;
/// Distant coast: height (screen heights) at 100 % and how far it reaches
/// across the screen (fraction of the width).
const COAST_HEIGHT: f32 = 0.03;
const COAST_REACH: f32 = 0.42;
/// Coast-road lights: spacing (screen heights) and dot radius.
const LIGHT_SPACING: f32 = 0.006;
const LIGHT_RADIUS: f32 = 0.0011;
/// Waves drift this far (world units) around a loop.
const WAVE_DRIFT: f32 = 0.6;
/// Clouds drift this far (screen heights) around a loop, starting a
/// quarter turn out of step with the waves.
const CLOUD_DRIFT: f32 = 0.05;
const CLOUD_PHASE: f32 = 0.25;
/// The sun's reach across the screen at ±100 % (share of the width).
const SUN_REACH: f32 = 0.4;
/// Sky: ramp tones of the top and the horizon and the sun's share of it
/// (dark; light mode), the gradient curve, and the glow's gain and vertical
/// squash.
const SKY_TOP: f32 = 0.08;
const SKY_LOW: f32 = 0.62;
const SKY_SUN: f32 = 0.45;
const LIGHT_SKY_TOP: f32 = 0.15;
const LIGHT_SKY_SUN: f32 = 0.3;
const SKY_CURVE: f32 = 1.8;
const GLOW_GAIN: f32 = 1.7;
const GLOW_SQUASH: f32 = 1.6;
/// The sun disc is lifted this much above the sun color.
const DISC_LIFT: f32 = 0.05;
/// Clouds: octaves, gain, ramp tone (dark, light), the band's fades at its
/// top (above, below) and bottom, the coverage thresholds (start and its
/// shift per unit of cloud, full shift), how far from the sun they are lit
/// and how much, how much sky they take, and their opacity.
const CLOUD_OCTAVES: u32 = 6;
const CLOUD_GAIN: f32 = 0.55;
const CLOUD_TONE: f32 = 0.12;
const LIGHT_CLOUD_TONE: f32 = 0.3;
const CLOUD_TOP_FADE: (f32, f32) = (0.1, 0.15);
const CLOUD_BOTTOM_FADE: f32 = 0.25;
const CLOUD_FROM: (f32, f32) = (0.55, 0.6);
const CLOUD_FULL: f32 = 0.45;
const CLOUD_LIT_REACH: f32 = 0.45;
const CLOUD_LIT: f32 = 0.75;
const CLOUD_AIR: f32 = 0.2;
const CLOUD_OPACITY: f32 = 0.9;
/// Sea: far (ramp tone, share) and near tones, dark and light, and how
/// much the far sea is dimmed.
const SEA_FAR: (f32, f32) = (0.3, 0.55);
const SEA_FAR_DIM: f32 = -0.06;
const SEA_NEAR: f32 = 0.03;
const LIGHT_SEA_FAR: (f32, f32) = (0.35, 0.4);
const LIGHT_SEA_NEAR: f32 = 0.7;
/// Waves: octaves; slope step (world depth) and gain, its clamp; the rows
/// (screen heights below the horizon) over which the sea turns from far to
/// near, that curve, the wave shading far and near, and the mirror falloff.
const WAVE_OCTAVES: u32 = 7;
const SLOPE_STEP: f32 = 0.05;
const SLOPE_GAIN: f32 = 20.0;
const SLOPE_MAX: f32 = 1.5;
const NEAR_ROWS: f32 = 0.45;
const SEA_CURVE: f32 = 0.7;
const WAVE_FAR: f32 = 0.3;
const WAVE_NEAR: f32 = 0.7;
const MIRROR_POWER: i32 = 6;
/// Fine glitter waves: frequency, octaves, gain and depth stretch; slope
/// share in a sparkle, rows below the horizon where sparkles start, and
/// the light of the sun path and its sparkles.
const FINE_FREQ: f32 = 3.0;
const FINE_OCTAVES: u32 = 5;
const FINE_GAIN: f32 = 0.6;
const FINE_STRETCH: f32 = 6.0;
const SPARKLE_SLOPE: f32 = 0.15;
const SPARKLE_START: f32 = 0.03;
const PATH_LIGHT: f32 = 0.35;
const SPARKLE_LIGHT: f32 = 2.2;
/// Coast: noise frequency and octaves, height base and relief, the share
/// of its reach over which it rises, and its tone (ramp, sky share; dark
/// and light).
const COAST_FREQ: f32 = 9.0;
const COAST_OCTAVES: u32 = 5;
const COAST_BASE: f32 = 0.55;
const COAST_RELIEF: f32 = 0.45;
const COAST_RISE: f32 = 0.6;
const COAST_TONE: (f32, f32) = (0.1, 0.28);
const LIGHT_COAST: (f32, f32) = (0.4, 0.4);
/// Coast-road lamps: density, how high up the headland they climb, their
/// pixel floor, lift and light.
const LAMP_DENSITY: f32 = 0.55;
const LAMP_RISE: f32 = 0.35;
const LAMP_MIN_PX: f32 = 0.7;
const LAMP_LIFT: f32 = 0.08;
const LAMP_LIGHT: f32 = 2.0;

pub struct Corniche {
    horizon: f32,
    sky_top: Lab,
    sky_low: Lab,
    sun: Lab,
    sun_at: (f32, f32),
    sun_r: f32,
    glow: f32,
    clouds: f32,
    cloud: Fractal,
    cloud_off: (f32, f32),
    cloud_dark: Lab,
    sea_far: Lab,
    sea_near: Lab,
    waves: f32,
    wave: Fractal,
    fine: Fractal,
    z: f32,
    wave_off: (f32, f32),
    glitter: f32,
    coast: f32,
    coast_side: f32,
    coast_noise: Fractal,
    coast_color: Lab,
    light_seed: u32,
    lamps: bool,
    aspect: f32,
    px: f32,
    edge: f32,
}

impl Corniche {
    pub fn new(ctx: &Ctx) -> Self {
        let s = &ctx.scheme;
        let horizon = ctx.u("horizon") - 0.5;
        let sun = s.sun();
        let sun_at = (ctx.u("sun_x") * ctx.aspect * SUN_REACH, horizon - ctx.u("sun_height") * (horizon + 0.5));
        let (sky_top, sky_low) = if s.light { (s.ramp(LIGHT_SKY_TOP), s.ramp(0.0).mix(sun, LIGHT_SKY_SUN)) } else { (s.ramp(SKY_TOP), s.ramp(SKY_LOW).mix(sun, SKY_SUN)) };
        let (sea_far, sea_near) = if s.light {
            (sky_low.mix(s.ramp(LIGHT_SEA_FAR.0), LIGHT_SEA_FAR.1), s.ramp(LIGHT_SEA_NEAR))
        } else {
            (sky_low.mix(s.ramp(SEA_FAR.0), SEA_FAR.1).lighten(SEA_FAR_DIM), s.ramp(SEA_NEAR))
        };
        let mut rng = ctx.rng(11);
        Corniche {
            horizon,
            sky_top,
            sky_low,
            sun,
            sun_at,
            sun_r: lerp(SUN_MIN, SUN_MAX, ctx.u("sun_size")),
            glow: ctx.u("glow"),
            clouds: ctx.u("clouds"),
            cloud: Fractal::new(rng.seed32(), CLOUD_FREQ, CLOUD_OCTAVES).gain(CLOUD_GAIN),
            cloud_off: ctx.orbit(1.0, CLOUD_PHASE, CLOUD_DRIFT),
            cloud_dark: if s.light { s.ramp(LIGHT_CLOUD_TONE) } else { s.ramp(CLOUD_TONE) },
            sea_far,
            sea_near,
            waves: ctx.u("waves"),
            wave: Fractal::new(rng.seed32(), 1.0, WAVE_OCTAVES).gain(0.5),
            fine: Fractal::new(rng.seed32(), FINE_FREQ, FINE_OCTAVES).gain(FINE_GAIN),
            z: ctx.z(crate::noise::LOOP_PERIOD),
            wave_off: ctx.orbit(1.0, 0.0, WAVE_DRIFT),
            glitter: ctx.u("glitter"),
            coast: ctx.u("coast"),
            coast_side: if rng.f() < 0.5 { -1.0 } else { 1.0 },
            coast_noise: Fractal::new(rng.seed32(), COAST_FREQ, COAST_OCTAVES),
            coast_color: if s.light { s.ramp(LIGHT_COAST.0).mix(sky_low, LIGHT_COAST.1) } else { s.ramp(COAST_TONE.0).mix(sky_low, COAST_TONE.1) },
            light_seed: rng.seed32(),
            lamps: !s.light,
            aspect: ctx.aspect,
            px: ctx.px,
            edge: ctx.edge_width(1.0),
        }
    }

    fn sky(&self, x: f32, y: f32) -> Lab {
        let v = ((y + 0.5) / (self.horizon + 0.5)).clamp(0.0, 1.0);
        let mut c = self.sky_top.mix_vivid(self.sky_low, pow(v, SKY_CURVE));
        let (dx, dy) = (x - self.sun_at.0, y - self.sun_at.1);
        let d = (dx * dx + dy * dy * GLOW_SQUASH).sqrt();
        c = super::light(c, self.sun, self.glow * GLOW_GAIN * super::glow(d, GLOW_CORE, GLOW_BLOOM));
        if self.clouds > 0.0 {
            let band = smoothstep(CLOUD_TOP - CLOUD_TOP_FADE.0, CLOUD_TOP + CLOUD_TOP_FADE.1, v) * (1.0 - smoothstep(CLOUD_BOTTOM - CLOUD_BOTTOM_FADE, CLOUD_BOTTOM, v));
            let (cx, cy) = (x + self.cloud_off.0, y + self.cloud_off.1);
            let n = self.cloud.at(cx, cy * CLOUD_STRETCH, 0.0, self.px);
            let density = smoothstep(CLOUD_FROM.0 - CLOUD_FROM.1 * self.clouds, 1.0 - CLOUD_FULL * self.clouds, n) * band;
            // Clouds near the sun are lit from below; far ones are dusk-dark.
            let lit = exp(-d / CLOUD_LIT_REACH);
            let cc = self.cloud_dark.mix(self.sun, CLOUD_LIT * lit).mix(self.sky_low, CLOUD_AIR);
            c = c.mix(cc, density * CLOUD_OPACITY);
            // Silver lining where a cloud thins near the sun.
            c = super::light(c, self.sun, density * (1.0 - density) * 2.0 * lit * self.glow);
        }
        let disc = 1.0 - smoothstep(self.sun_r - self.edge, self.sun_r + self.edge, (dx * dx + dy * dy).sqrt());
        c.mix(self.sun.lighten(DISC_LIFT), disc * smoothstep(self.horizon + self.edge, self.horizon - self.edge, y))
    }
}

impl Scene for Corniche {
    fn shade(&self, x: f32, y: f32) -> Lab {
        let mut c;
        if y < self.horizon + self.edge {
            c = self.sky(x, y);
        } else {
            c = self.sea_far;
        }
        if y > self.horizon - self.edge {
            let d = (y - self.horizon).max(self.px * 0.5);
            // World coordinates of this row: depth and across.
            let wz = DEPTH / d;
            let wx = x * wz;
            // Pixel footprint in world units (depth changes fastest).
            let foot = self.px * DEPTH / (d * d);
            let ox = self.wave_off.0;
            let oz = self.wave_off.1;
            let h = self.wave.at(wx * WAVE_FREQ_X + ox, wz * WAVE_FREQ_Z + oz, self.z, foot * WAVE_FREQ_Z);
            let h2 = self.wave.at(wx * WAVE_FREQ_X + ox, (wz + SLOPE_STEP) * WAVE_FREQ_Z + oz, self.z, foot * WAVE_FREQ_Z);
            let slope = (h2 - h) * SLOPE_GAIN;
            // Fresnel-ish: grazing water near the horizon mirrors the sky.
            let near = smoothstep(0.0, NEAR_ROWS, d);
            let mut sea = self.sea_far.mix(self.sea_near, pow(near, SEA_CURVE));
            sea.l += WAVE_SHADE * self.waves * slope.clamp(-SLOPE_MAX, SLOPE_MAX) * (WAVE_FAR + WAVE_NEAR * near);
            sea = sea.mix(self.sky_low, (1.0 - near).powi(MIRROR_POWER) * 0.5);
            // Sun path: a soft reflection plus glints on wave slopes.
            let half = GLITTER_WIDTH + GLITTER_SPREAD * d;
            let col = exp(-((x - self.sun_at.0) / half).powi(2));
            let fine = self.fine.at(wx * 2.0 + ox, wz * FINE_STRETCH + oz, self.z, foot * FINE_STRETCH);
            let sparkle = smoothstep(SPARKLE_FROM, SPARKLE_TO, fine + slope * SPARKLE_SLOPE) * smoothstep(0.0, SPARKLE_START, d);
            let glints = self.glitter * col * (PATH_LIGHT * self.glow + SPARKLE_LIGHT * sparkle);
            sea = super::light(sea, self.sun, glints);
            let cov = smoothstep(self.horizon - self.edge, self.horizon + self.edge, y);
            c = c.mix(sea, cov);
        }
        if self.coast > 0.0 {
            // A low headland on one side, tapering out toward the middle.
            let edge_x = self.coast_side * self.aspect * 0.5;
            let reach = self.aspect * COAST_REACH;
            let k = 1.0 - ((x - edge_x).abs() / reach).clamp(0.0, 1.0);
            if k > 0.0 {
                let n = COAST_BASE + COAST_RELIEF * self.coast_noise.at(x, 0.0, 0.0, self.px);
                let top = self.horizon - COAST_HEIGHT * self.coast * smoothstep(0.0, COAST_RISE, k) * n;
                let cov = smoothstep(top - self.edge, top + self.edge, y) * (1.0 - smoothstep(self.horizon - self.edge, self.horizon + self.edge * 0.5, y));
                c = c.mix(self.coast_color, cov);
                if self.lamps {
                    // Lights of the coast road along the headland's foot.
                    let cell = (x / LIGHT_SPACING).floor();
                    let h = hash3(cell as i32, 0, 0, self.light_seed);
                    if unit(h) < LAMP_DENSITY * k {
                        let lx = (cell + 0.5) * LIGHT_SPACING;
                        let ly = self.horizon - self.edge * 0.5 - unit(hash3(cell as i32, 1, 0, self.light_seed)) * COAST_HEIGHT * self.coast * LAMP_RISE * k;
                        let r = LIGHT_RADIUS.max(self.px * LAMP_MIN_PX);
                        let e = (LIGHT_RADIUS / r).powi(2).min(1.0);
                        let dd = ((x - lx).powi(2) + (y - ly).powi(2)) / (r * r);
                        c = super::light(c, self.sun.lighten(LAMP_LIFT), LAMP_LIGHT * e * exp(-dd));
                    }
                }
            }
        }
        c
    }
}
