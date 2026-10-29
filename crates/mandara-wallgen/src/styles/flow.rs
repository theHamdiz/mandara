//! Silk flow: fine threads sweeping across the screen like brushed silk.
//! The threads are level lines of a stream function made of a tilted ramp
//! plus a domain-warped noise, so they run in long open curves (no closed
//! loops) and never cross. A thread's width is true screen distance (level
//! offset over the field gradient): crisp, evenly antialiased and the same
//! weight at every resolution. A soft ribbon of light runs through the
//! weave, threads fade in and out along their length, and each glows.

use crate::color::Lab;
use crate::fmath::{cos, exp, fract, lerp, sin, smoothstep};
use crate::noise::{Fractal, hash3, unit};
use crate::render::{Ctx, Scene};

/// Threads per screen height at 0 % and 100 % density.
const LINES_MIN: f32 = 30.0;
const LINES_MAX: f32 = 150.0;
/// Warp frequency (cycles per screen height) at 0 % and 100 % scale.
const FREQ_MIN: f32 = 1.6;
const FREQ_MAX: f32 = 0.35;
/// Bend of the stream lines (screen heights) at 100 % turbulence.
const BEND: f32 = 0.42;
/// Second-order warp: the bends themselves curl.
const CURL: f32 = 0.6;
/// Thread half-width (screen heights) at 0 % and 100 % weight.
const WIDTH_MIN: f32 = 0.00025;
const WIDTH_MAX: f32 = 0.0012;
/// Glow falloff distance (screen heights) and strength at 100 %.
const GLOW_DIST: f32 = 0.0035;
const GLOW_GAIN: f32 = 0.35;
/// Finite-difference step for the field gradient (screen heights).
const EPS: f32 = 0.0004;
/// The ribbon of light: half-width (in field units) and floor brightness.
const RIBBON_WIDTH: f32 = 0.22;
const RIBBON_FLOOR: f32 = 0.18;
/// Light a thread adds at full brightness.
const THREAD_LIGHT: f32 = 0.9;
/// Frequency of the fade field along threads (cycles per screen height).
const FADE_FREQ: f32 = 1.1;
/// The flow's angle (radians from horizontal) lies in ± this range.
const ANGLE_RANGE: f32 = 0.45;
/// Ground ramp tones (dark, light) and the ground's floor away from the
/// ribbon.
const GROUND_TONE: f32 = 0.12;
const LIGHT_GROUND_TONE: f32 = 0.14;
const GROUND_FLOOR: f32 = 0.3;
/// Warp noise octaves and gain; curl noise frequency (share of the warp's),
/// octaves, and how far it shifts the warp.
const WARP_OCTAVES: u32 = 3;
const WARP_GAIN: f32 = 0.45;
const CURL_FREQ: f32 = 1.7;
const CURL_OCTAVES: u32 = 2;
const CURL_SHIFT: f32 = 0.3;
/// The ribbon's offset range (stream units) and width scale range.
const RIBBON_SHIFT: f32 = 0.15;
const RIBBON_SCALE: (f32, f32) = (0.7, 1.3);
/// The dimmest thread's brightness.
const BRIGHT_FLOOR: f32 = 0.45;
/// Thread fade: noise octaves and stretch, per-thread jitter, and the noise
/// values where a thread fades in and is fully shown.
const FADE_OCTAVES: u32 = 3;
const FADE_STRETCH: f32 = 0.6;
const FADE_JITTER: f32 = 0.8;
const FADE_FROM: f32 = -0.45;
const FADE_TO: f32 = 0.5;
/// Thread color: the ramp span it drifts over and the drift frequency per
/// unit of color spread.
const HUE_LOW: f32 = 0.55;
const HUE_SPAN: f32 = 0.4;
const HUE_FREQ: f32 = 5.0;
/// Line antialiasing ramp (pixels), and the thread spacing (pixels) over
/// which crowded threads turn into an even sheen.
const AA_SHARE: f32 = 0.75;
const CROWD_FROM: f32 = 1.5;
const CROWD_TO: f32 = 4.0;

pub struct Flow {
    ground_a: Lab,
    ground_b: Lab,
    warp_a: Fractal,
    warp_b: Fractal,
    fade_f: Fractal,
    dir: (f32, f32),
    bend: f32,
    lines: f32,
    width: f32,
    glow: f32,
    fade: f32,
    spread: f32,
    ribbon: (f32, f32),
    z: f32,
    px: f32,
    line_seed: u32,
    scheme: crate::palette::Scheme,
}

impl Flow {
    pub fn new(ctx: &Ctx) -> Self {
        let s = &ctx.scheme;
        let mut rng = ctx.rng(41);
        let freq = lerp(FREQ_MIN, FREQ_MAX, ctx.u("scale"));
        let angle = rng.range(-ANGLE_RANGE, ANGLE_RANGE);
        let (ground_a, ground_b) = if s.light { (s.ramp(0.0), s.ramp(LIGHT_GROUND_TONE)) } else { (s.deep(), s.ramp(GROUND_TONE)) };
        Flow {
            ground_a,
            ground_b,
            warp_a: Fractal::new(rng.seed32(), freq, WARP_OCTAVES).gain(WARP_GAIN),
            warp_b: Fractal::new(rng.seed32(), freq * CURL_FREQ, CURL_OCTAVES),
            fade_f: Fractal::new(rng.seed32(), FADE_FREQ, FADE_OCTAVES),
            dir: (cos(angle), sin(angle)),
            bend: ctx.u("turbulence") * BEND,
            lines: lerp(LINES_MIN, LINES_MAX, ctx.u("density")),
            width: lerp(WIDTH_MIN, WIDTH_MAX, ctx.u("thickness")),
            glow: ctx.u("glow") * GLOW_GAIN,
            fade: ctx.u("fade"),
            spread: ctx.u("color_spread"),
            ribbon: (rng.range(-RIBBON_SHIFT, RIBBON_SHIFT), rng.range(RIBBON_SCALE.0, RIBBON_SCALE.1)),
            z: ctx.z(crate::noise::LOOP_PERIOD),
            px: ctx.px,
            line_seed: rng.seed32(),
            scheme: s.clone(),
        }
    }

    /// Stream function: distance across the flow plus warped bends.
    fn field(&self, x: f32, y: f32) -> f32 {
        let c = self.warp_b.at(x, y, self.z, self.px) * CURL;
        let w = self.warp_a.at(x + c * CURL_SHIFT, y - c * CURL_SHIFT, self.z, self.px);
        -x * self.dir.1 + y * self.dir.0 + self.bend * w
    }
}

impl Scene for Flow {
    fn shade(&self, x: f32, y: f32) -> Lab {
        let f = self.field(x, y);
        let fx = (self.field(x + EPS, y) - f) / EPS;
        let fy = (self.field(x, y + EPS) - f) / EPS;
        let grad = (fx * fx + fy * fy).sqrt().max(1e-4);
        // Ground: deep, brightening toward the ribbon.
        let rib = exp(-((f - self.ribbon.0) / (RIBBON_WIDTH * self.ribbon.1)).powi(2));
        let c = self.ground_a.mix_vivid(self.ground_b, GROUND_FLOOR + (1.0 - GROUND_FLOOR) * rib);
        let k = f * self.lines;
        // Distance to the nearest thread, in screen heights.
        let dist = (fract(k + 0.5) - 0.5).abs() / (self.lines * grad);
        let thread = (k + 0.5).floor() as i32;
        let bright = BRIGHT_FLOOR + (1.0 - BRIGHT_FLOOR) * unit(hash3(thread, 0, 0, self.line_seed));
        let along = self.fade_f.at(x * FADE_STRETCH, y * FADE_STRETCH, self.z, self.px) + (unit(hash3(thread, 1, 0, self.line_seed)) - 0.5) * FADE_JITTER;
        let vis = lerp(1.0, smoothstep(FADE_FROM, FADE_TO, along), self.fade);
        // Color drifts across the weave and along the ribbon.
        let hue_t = HUE_LOW + HUE_SPAN * (0.5 + 0.5 * sin(f * HUE_FREQ * self.spread + unit(hash3(thread, 2, 0, self.line_seed)) * self.spread * 2.0));
        let color = self.scheme.ramp(hue_t);
        // Threads thinner than a pixel keep their energy: dim, not vanish.
        let w = self.width.max(self.px * 0.5);
        let energy = (self.width / w).min(1.0);
        let aa = self.px * AA_SHARE;
        let core = 1.0 - smoothstep(w - aa, w + aa, dist);
        let halo = self.glow * exp(-dist / GLOW_DIST);
        // Where threads crowd closer than a pixel they would moiré: blend
        // them into an even sheen instead.
        let spacing_px = 1.0 / (self.lines * grad * self.px);
        let crowd = smoothstep(CROWD_FROM, CROWD_TO, spacing_px);
        let sheen = (1.0 - crowd) * (self.width * 2.0 * self.lines * grad).min(1.0);
        let a = ((core * energy * crowd + sheen) * THREAD_LIGHT + halo * crowd) * bright * vis * (RIBBON_FLOOR + (1.0 - RIBBON_FLOOR) * rib);
        super::mark(c, color, a, self.scheme.light)
    }
}
