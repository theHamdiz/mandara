//! Aurora: curtains of light hanging over a starry night. Each curtain is
//! a ribbon whose lower hem meanders across the sky and comes and goes
//! along its length; above the hem the light streams upward in fine
//! vertical rays, fading and shifting from the first accent to the next.
//! Where the hem swings steeply the curtain is seen edge-on, and it
//! brightens, as real folds do. A tiered pine forest and soft hills close
//! the frame.

use crate::color::Lab;
use crate::fmath::{exp, lerp, pow, smoothstep};
use crate::noise::{Fractal, hash3, unit};
use crate::render::{Ctx, Scene};

/// Curtain hems sit in this band (screen heights from the top).
const HEM_TOP: f32 = 0.34;
const HEM_BOTTOM: f32 = 0.6;
/// Curtain height (screen heights) at 0 % and 100 %.
const HEIGHT_MIN: f32 = 0.12;
const HEIGHT_MAX: f32 = 0.5;
/// Hem meander amplitude (screen heights) at 100 % waviness, and frequency.
const WAVE_AMP: f32 = 0.34;
const WAVE_FREQ: f32 = 0.3;
/// Along its length a curtain fades in and out at this frequency.
const PRESENCE_FREQ: f32 = 0.45;
/// Rays: frequency across (cycles per screen height), vertical stretch, and
/// how deeply they comb the light at 100 % ray detail.
const RAY_FREQ: f32 = 22.0;
const RAY_STRETCH: f32 = 0.06;
const RAY_DEPTH: f32 = 0.85;
/// Thickness of the bright hem line (screen heights).
const HEM_LINE: f32 = 0.006;
/// Fold brightening per unit of hem slope.
const FOLD_GAIN: f32 = 1.4;
/// Light a curtain adds at 100 % intensity.
const LIGHT: f32 = 1.1;
/// Pines: cell width, width and height ranges (screen heights), tiers.
const TREE_CELL: f32 = 0.016;
const TREE_W: (f32, f32) = (0.016, 0.034);
const TREE_H: (f32, f32) = (0.045, 0.13);
const TIERS: f32 = 5.0;
/// Rolling hills under the trees: amplitude (screen heights).
const HILL_AMP: f32 = 0.05;
/// Hem sway (screen heights) around a loop.
const SWAY: f32 = 0.03;
/// The aurora's glow on the low sky.
const SKY_GLOW: f32 = 0.12;
/// Curtains: hem jitter (screen heights), height scale range, the weakest
/// curtain's strength, and the span of noise rows they draw from.
const HEM_JITTER: f32 = 0.03;
const CURTAIN_SCALE: (f32, f32) = (0.7, 1.15);
const CURTAIN_STRENGTH_MIN: f32 = 0.7;
const ROW_SPAN: f32 = 50.0;
/// Hem meander noise: frequency scale range, octaves and gain.
const WAVE_FREQ_SCALE: (f32, f32) = (0.8, 1.3);
const WAVE_OCTAVES: u32 = 4;
const WAVE_GAIN: f32 = 0.45;
/// Presence noise octaves, and the noise values where a curtain fades in
/// and is fully present.
const PRESENCE_OCTAVES: u32 = 2;
const PRESENCE_FROM: f32 = -0.25;
const PRESENCE_TO: f32 = 0.45;
/// Ray noise: octaves, gain, how much rays follow the hem's meander, and
/// their contrast.
const RAY_OCTAVES: u32 = 3;
const RAY_GAIN: f32 = 0.55;
const RAY_FOLLOW: f32 = 0.08;
const RAY_CONTRAST: f32 = 0.6;
/// Step (screen heights) for the hem's slope (fold brightening).
const SLOPE_STEP: f32 = 0.01;
/// Curtains are skipped this many hem lines below the hem and curtain
/// heights above it.
const HEM_REACH: f32 = 4.0;
const CURTAIN_REACH: f32 = 3.0;
/// Curtain body: fade per curtain height, the share that lingers high up,
/// the bright share near the hem and its fade, and the hem line's glow.
const BODY_FADE: f32 = 1.8;
const BODY_FLOOR: f32 = 0.4;
const BODY_BRIGHT: f32 = 0.6;
const BODY_HEM_FADE: f32 = 5.0;
const HEM_GLOW: f32 = 0.7;
/// The color turns from the low to the high accent between these curtain
/// heights, lifted this much.
const HUE_SHIFT_FROM: f32 = 0.1;
const HUE_SHIFT_TO: f32 = 1.2;
const CURTAIN_LIFT: f32 = 0.06;
/// Night sky: the horizon's ramp tone and the gradient curve.
const NIGHT_HORIZON: f32 = 0.14;
const SKY_CURVE: f32 = 1.3;
/// Stars: their color, brightness gain and density share.
const STARLIGHT: Lab = Lab::new(0.95, -0.005, 0.01);
const STAR_GAIN: f32 = 0.9;
const STAR_DENSITY: f32 = 0.7;
/// Night land: ground and hill darkening below the deepest stop.
const NIGHT_GROUND: f32 = -0.05;
const NIGHT_HILLS: f32 = -0.02;
/// Hills: noise frequency, octaves and the noise row they sample; land is
/// only shaded this far (screen heights) above its highest reach.
const HILL_FREQ: f32 = 1.1;
const HILL_OCTAVES: u32 = 4;
const HILL_ROW: f32 = 3.3;
const LAND_MARGIN: f32 = 0.02;
/// Pines: share of empty cells, how far below the hill line they root,
/// and each tier's tucked-in width and flare.
const TREE_GAPS: f32 = 0.18;
const TREE_ROOT: f32 = 0.01;
const TIER_TUCK: f32 = 0.55;
const TIER_FLARE: f32 = 0.45;
/// Light mode (twilight): the zenith's lightness, the least chroma it
/// keeps, and the sun's share of the pale horizon…
const TWILIGHT_ZENITH_L: f32 = 0.74;
const TWILIGHT_ZENITH_C: f32 = 0.07;
const TWILIGHT_GLOW: f32 = 0.35;
/// …the land: how far below the zenith it sits, its chroma, and how much
/// darker the near ground is…
const TWILIGHT_LAND: f32 = -0.26;
const TWILIGHT_LAND_CHROMA: f32 = 0.7;
const TWILIGHT_NEAR: f32 = -0.06;
/// …curtains: lift above the sky behind them, chroma boost and coverage
/// gain…
const TWILIGHT_CURTAIN_LIFT: f32 = 0.08;
/// Curtains never get paler than this (they would wash out to white).
const TWILIGHT_CURTAIN_MAX_L: f32 = 0.93;
const TWILIGHT_CURTAIN_CHROMA: f32 = 1.5;
const TWILIGHT_CURTAIN_GAIN: f32 = 2.6;
/// …and the share of stars already out.
const TWILIGHT_STARS: f32 = 0.35;

/// The twilight zenith of a light scheme: its coolest colorful stop, set
/// to a deep pastel.
fn twilight_zenith(s: &crate::palette::Scheme) -> Lab {
    let best = super::coolest(s);
    Lab::from_lch(TWILIGHT_ZENITH_L, best.chroma().max(TWILIGHT_ZENITH_C), best.hue())
}

struct Curtain {
    hem: f32,
    height: f32,
    meander: Fractal,
    presence: Fractal,
    rays: Fractal,
    low: Lab,
    high: Lab,
    strength: f32,
    row: f32,
}

pub struct Aurora {
    sky_top: Lab,
    sky_low: Lab,
    curtains: Vec<Curtain>,
    intensity: f32,
    rays: f32,
    wave: f32,
    stars: f32,
    star_seed: u32,
    horizon: f32,
    hills: Fractal,
    tree_seed: u32,
    ground: Lab,
    hill_color: Lab,
    z: f32,
    sway: (f32, f32),
    px: f32,
    t: f32,
    edge: f32,
    light_mode: bool,
}

impl Aurora {
    pub fn new(ctx: &Ctx) -> Self {
        let s = &ctx.scheme;
        let n = ctx.n("curtains");
        let mut rng = ctx.rng(21);
        let height = lerp(HEIGHT_MIN, HEIGHT_MAX, ctx.u("height"));
        let curtains = (0..n)
            .map(|i| {
                let k = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.5 };
                Curtain {
                    hem: lerp(HEM_TOP, HEM_BOTTOM, k) + rng.range(-HEM_JITTER, HEM_JITTER) - 0.5,
                    height: height * rng.range(CURTAIN_SCALE.0, CURTAIN_SCALE.1),
                    meander: Fractal::new(rng.seed32(), WAVE_FREQ * rng.range(WAVE_FREQ_SCALE.0, WAVE_FREQ_SCALE.1), WAVE_OCTAVES).gain(WAVE_GAIN),
                    presence: Fractal::new(rng.seed32(), PRESENCE_FREQ, PRESENCE_OCTAVES),
                    rays: Fractal::new(rng.seed32(), 1.0, RAY_OCTAVES).gain(RAY_GAIN),
                    low: s.accent(i),
                    high: s.accent(i + 1),
                    strength: rng.range(CURTAIN_STRENGTH_MIN, 1.0),
                    row: rng.range(0.0, ROW_SPAN),
                }
            })
            .collect();
        // Light mode is the bright end of twilight: a deep pastel zenith
        // over a warm pale horizon, silhouettes muted by the dusk air.
        let (sky_top, sky_low) = if s.light { (twilight_zenith(s), s.ramp(0.0).mix_vivid(s.sun(), TWILIGHT_GLOW)) } else { (s.deep(), s.ramp(NIGHT_HORIZON)) };
        let dusk_land = sky_top.lighten(TWILIGHT_LAND).saturate(TWILIGHT_LAND_CHROMA);
        Aurora {
            sky_top,
            sky_low,
            curtains,
            intensity: ctx.u("intensity"),
            rays: ctx.u("rays"),
            wave: ctx.u("waviness") * WAVE_AMP,
            stars: ctx.u("stars") * if s.light { TWILIGHT_STARS } else { 1.0 },
            star_seed: ctx.seed32(22),
            horizon: ctx.u("horizon") - 0.5,
            hills: Fractal::new(ctx.seed32(23), HILL_FREQ, HILL_OCTAVES),
            tree_seed: ctx.seed32(24),
            ground: if s.light { dusk_land.lighten(TWILIGHT_NEAR) } else { s.deep().lighten(NIGHT_GROUND) },
            hill_color: if s.light { dusk_land } else { s.deep().lighten(NIGHT_HILLS) },
            z: ctx.z(crate::noise::LOOP_PERIOD),
            sway: ctx.orbit(1.0, 0.0, SWAY),
            px: ctx.px,
            t: ctx.t,
            edge: ctx.edge_width(1.0),
            light_mode: s.light,
        }
    }

    /// Coverage of the pine forest at `(x, y)`; `top` is the hill line.
    fn forest(&self, x: f32, y: f32, top: f32) -> f32 {
        let c = (x / TREE_CELL).floor() as i32;
        let mut cov: f32 = 0.0;
        for k in c - 2..=c + 2 {
            let r = |salt| unit(hash3(k, salt, 0, self.tree_seed));
            if r(0) < TREE_GAPS {
                continue;
            }
            let w = lerp(TREE_W.0, TREE_W.1, r(1));
            let h = lerp(TREE_H.0, TREE_H.1, r(2) * r(2));
            let cx = (k as f32 + 0.5) * TREE_CELL + (r(3) - 0.5) * TREE_CELL;
            let base = top + TREE_ROOT;
            let q = (base - y) / h;
            if !(0.0..=1.0).contains(&q) {
                continue;
            }
            // Tiered silhouette: each tier flares out then tucks in.
            let tier = TIER_TUCK + TIER_FLARE * (1.0 - (q * TIERS).fract());
            let half = 0.5 * w * (1.0 - q) * tier;
            let d = half - (x - cx).abs();
            cov = cov.max(smoothstep(-self.edge, self.edge, d));
        }
        cov
    }
}

impl Scene for Aurora {
    fn shade(&self, x: f32, y: f32) -> Lab {
        let v = ((y + 0.5) / (self.horizon + 0.5)).clamp(0.0, 1.0);
        let mut c = self.sky_top.mix_vivid(self.sky_low, pow(v, SKY_CURVE));
        if self.stars > 0.0 {
            c = super::light(c, STARLIGHT, STAR_GAIN * super::stars(self.star_seed, x, y, self.stars * STAR_DENSITY, self.px, self.t));
        }
        let mut glow = 0.0f32;
        let mut glow_color = self.sky_low;
        for cu in &self.curtains {
            let mx = x + self.sway.0;
            let m = cu.meander.at(mx, cu.row, self.z, self.px);
            let m2 = cu.meander.at(mx + SLOPE_STEP, cu.row, self.z, self.px);
            let slope = ((m2 - m) / SLOPE_STEP * self.wave).abs();
            let hem = cu.hem + self.wave * m + self.sway.1;
            let above = hem - y;
            if above < -HEM_LINE * HEM_REACH || above > cu.height * CURTAIN_REACH {
                continue;
            }
            let presence = smoothstep(PRESENCE_FROM, PRESENCE_TO, cu.presence.at(x, cu.row, self.z, self.px));
            if presence <= 0.0 {
                continue;
            }
            let h = (above / cu.height).max(0.0);
            // A sharp bright hem, then light streaming upward.
            let hem_line = exp(-(above / HEM_LINE).powi(2));
            let body = smoothstep(-HEM_LINE, HEM_LINE * 2.0, above) * exp(-h * BODY_FADE) * (BODY_FLOOR + BODY_BRIGHT * exp(-h * BODY_HEM_FADE));
            // Rays: noise across the (warped) curtain, stretched upward.
            let r = cu.rays.at((x + m * RAY_FOLLOW) * RAY_FREQ, (y + self.sway.1) * RAY_FREQ * RAY_STRETCH + cu.row, self.z, self.px * RAY_FREQ);
            let comb = 1.0 - RAY_DEPTH * self.rays * (0.5 - RAY_CONTRAST * r).clamp(0.0, 1.0);
            let fold = 1.0 + FOLD_GAIN * slope.min(1.0);
            let a = (HEM_GLOW * hem_line + body * comb) * fold * presence * cu.strength;
            let color = cu.low.mix_vivid(cu.high, smoothstep(HUE_SHIFT_FROM, HUE_SHIFT_TO, h)).lighten(CURTAIN_LIFT);
            c = if self.light_mode {
                // On a bright sky a curtain shows as luminous pastel color:
                // lighter and more vivid than the sky behind it.
                let mut glow = color.saturate(TWILIGHT_CURTAIN_CHROMA);
                glow.l = (glow.l.max(c.l) + TWILIGHT_CURTAIN_LIFT).min(TWILIGHT_CURTAIN_MAX_L);
                c.mix_vivid(glow, 1.0 - exp(-self.intensity * LIGHT * a * TWILIGHT_CURTAIN_GAIN))
            } else {
                super::light(c, color, self.intensity * LIGHT * a)
            };
            if a * exp(-h) > glow {
                glow = a * exp(-h);
                glow_color = cu.low;
            }
        }
        // The aurora lights the sky near the horizon a little.
        c = super::light(c, glow_color, SKY_GLOW * self.intensity * glow);
        // Hills and pines.
        if y > self.horizon - TREE_H.1 - HILL_AMP - LAND_MARGIN {
            let top = self.horizon - HILL_AMP * (0.5 + 0.5 * self.hills.at(x, HILL_ROW, 0.0, self.px));
            let hill = smoothstep(top - self.edge, top + self.edge, y);
            let trees = self.forest(x, y, top);
            c = c.mix(self.hill_color, hill.max(trees));
            // Ground darkens toward the viewer.
            c = c.mix(self.ground, smoothstep(top, 0.5, y) * hill);
        }
        c
    }
}
