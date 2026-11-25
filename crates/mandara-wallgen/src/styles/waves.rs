//! Waves: stacked bands of tinted glass. Each band's crest is a sum of
//! sines (whole cycles per loop, so a video loops seamlessly), filled with
//! a vertical gradient, edged with a bright glassy rim, casting a soft
//! shadow onto the band behind, and slightly translucent.

use crate::color::Lab;
use crate::fmath::{cos, exp, lerp, sin, smoothstep};
use crate::render::{Ctx, Scene};

/// The first crest sits this far from the top, the last this far down
/// (screen heights from the top).
const FIRST: f32 = 0.18;
const LAST: f32 = 0.86;
/// Crest amplitude (screen heights) at 100 %.
const AMPLITUDE: f32 = 0.09;
/// Base wave frequency (radians per screen height) at 0 % and 100 %.
const FREQ_MIN: f32 = 2.0;
const FREQ_MAX: f32 = 9.0;
/// Tilt at ±100 %: crest rise per screen height across.
const TILT: f32 = 0.22;
/// Rim: a tight bright line and a broad glow under the crest.
const RIM_CORE: f32 = 0.0022;
const RIM_GLOW: f32 = 0.03;
const RIM_LIGHT: f32 = 0.9;
/// Shadow a band casts on the one behind: reach (screen heights) and depth.
const SHADOW_REACH: f32 = 0.05;
const SHADOW_DEPTH: f32 = 0.35;
/// Lightness drop from a band's crest to its bottom.
const BODY_FALL: f32 = 0.08;
/// Most bands a recipe can ask for (the `bands` range).
const MAX_BANDS: usize = 12;
/// Crest harmonics: frequency multiples and amplitudes, and the per-band
/// frequency scale range.
const HARMONIC_FREQ: [f32; 3] = [1.0, 1.9, 3.3];
const HARMONIC_AMP: [f32; 3] = [1.0, 0.45, 0.2];
const FREQ_SCALE: (f32, f32) = (0.8, 1.2);
/// Band tones along the ramp, back to front (dark, light), and the sky's.
const BAND_TONES: (f32, f32) = (0.85, 0.06);
const LIGHT_BAND_TONES: (f32, f32) = (0.12, 0.85);
const SKY_TONE: f32 = 0.12;
const LIGHT_SKY_TONE: f32 = 0.08;
/// Body falloff depth (screen heights below the crest).
const BODY_DEPTH: f32 = 0.4;
/// Rim: lift of its color, its glow's share, its tint toward the band, and
/// the share light mode keeps.
const RIM_LIFT: f32 = 0.1;
const RIM_GLOW_SHARE: f32 = 0.35;
const RIM_TINT: f32 = 0.35;
const LIGHT_RIM: f32 = 0.5;
/// At 100 % translucency a band lets this share of what is behind through.
const TRANSLUCENCY: f32 = 0.6;

struct Band {
    base: f32,
    harmonics: [(f32, f32, f32); 3],
    color: Lab,
}

pub struct Waves {
    bands: Vec<Band>,
    sky: (Lab, Lab),
    amp: f32,
    tilt: f32,
    rim: f32,
    shadow: f32,
    translucency: f32,
    rim_color: Lab,
    edge: f32,
    light: bool,
}

impl Waves {
    pub fn new(ctx: &Ctx) -> Self {
        let n = ctx.n("bands");
        let s = &ctx.scheme;
        let mut rng = ctx.rng(91);
        let freq = lerp(FREQ_MIN, FREQ_MAX, ctx.u("frequency"));
        let motion = ctx.motion();
        let bands = (0..n)
            .map(|i| {
                let k = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 };
                // Each harmonic: frequency, phase now (loop-aware) and weight.
                let mut h = [(0.0, 0.0, 0.0); 3];
                for (j, hj) in h.iter_mut().enumerate() {
                    let f = freq * HARMONIC_FREQ[j] * rng.range(FREQ_SCALE.0, FREQ_SCALE.1);
                    // Whole cycles per loop keep it seamless; no motion stops them.
                    let cycles = if motion > 0.0 { [1.0, -1.0, 2.0][j] } else { 0.0 };
                    *hj = (f, ctx.angle(cycles, rng.f()), HARMONIC_AMP[j]);
                }
                Band {
                    base: lerp(FIRST, LAST, k) - 0.5,
                    harmonics: h,
                    color: if s.light { s.ramp(lerp(LIGHT_BAND_TONES.0, LIGHT_BAND_TONES.1, k)) } else { s.ramp(lerp(BAND_TONES.0, BAND_TONES.1, k)) },
                }
            })
            .collect();
        let sky = if s.light { (s.ramp(0.0), s.ramp(LIGHT_SKY_TONE)) } else { (s.deep(), s.ramp(SKY_TONE)) };
        Waves {
            bands,
            sky,
            amp: ctx.u("amplitude") * AMPLITUDE,
            tilt: ctx.u("tilt") * TILT,
            rim: ctx.u("rim"),
            shadow: ctx.u("shadow"),
            translucency: ctx.u("translucency"),
            rim_color: s.bright().lighten(RIM_LIFT),
            edge: ctx.edge_width(1.0),
            light: s.light,
        }
    }

    /// Crest height and slope of band `b` at `x`.
    fn crest(&self, b: &Band, x: f32) -> (f32, f32) {
        let (mut y, mut dy) = (b.base + self.tilt * x, self.tilt);
        for &(f, p, w) in &b.harmonics {
            y += self.amp * w * sin(f * x + p);
            dy += self.amp * w * f * cos(f * x + p);
        }
        (y, dy)
    }
}

impl Scene for Waves {
    fn shade(&self, x: f32, y: f32) -> Lab {
        let mut c = self.sky.0.mix(self.sky.1, (y + 0.5).clamp(0.0, 1.0));
        let mut crests = [(0.0f32, 0.0f32); MAX_BANDS];
        for (slot, b) in crests.iter_mut().zip(&self.bands) {
            *slot = self.crest(b, x);
        }
        for (i, b) in self.bands.iter().enumerate() {
            let (top, slope) = crests[i];
            // Vertical distance → true distance to the crest line.
            let d = (y - top) / (1.0 + slope * slope).sqrt();
            if d < -self.edge {
                continue;
            }
            let cov = smoothstep(-self.edge, self.edge, d);
            let mut bc = b.color;
            bc.l -= BODY_FALL * smoothstep(0.0, BODY_DEPTH, d);
            // Glassy rim just under the crest.
            let rim = self.rim * RIM_LIGHT * (exp(-d.max(0.0) / RIM_CORE) + RIM_GLOW_SHARE * exp(-d.max(0.0) / RIM_GLOW));
            bc = super::light(bc, self.rim_color.mix(b.color, RIM_TINT), rim * if self.light { LIGHT_RIM } else { 1.0 });
            // Shadow from the next band in front, just above its crest.
            if let Some(&(next, ns)) = crests[..self.bands.len()].get(i + 1) {
                let above = (next - y) / (1.0 + ns * ns).sqrt();
                if above > 0.0 {
                    bc.l *= 1.0 - self.shadow * SHADOW_DEPTH * exp(-above / SHADOW_REACH);
                }
            }
            // Translucent glass lets the layer behind show through.
            let behind = c;
            c = behind.mix(bc, cov * (1.0 - self.translucency * TRANSLUCENCY));
        }
        c
    }
}
