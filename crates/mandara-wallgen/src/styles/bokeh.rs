//! Bokeh: out-of-focus lights at several depths. Each depth layer is a
//! jittered grid of discs (so a pixel only checks its 3 × 3 neighbouring
//! cells); far layers are small and dim, near ones large and soft. Discs
//! have the slightly brighter rim of a real lens's circle of confusion,
//! gather along a diagonal band, and add light over a deep gradient.

use crate::color::Lab;
use crate::fmath::{exp, lerp, smoothstep};
use crate::noise::{hash3, unit};
use crate::render::{Ctx, Scene};

/// Depth layers, far to near.
const LAYERS: usize = 4;
/// Cell size of the farthest and nearest layers (screen heights).
const CELL_FAR: f32 = 0.07;
const CELL_NEAR: f32 = 0.24;
/// Disc radius relative to its cell at 100 % size.
const RADIUS: f32 = 0.42;
/// Edge softness relative to the radius at 100 % defocus.
const DEFOCUS: f32 = 0.45;
/// Light a disc adds (far and near layers) at 100 % intensity.
const LIGHT_FAR: f32 = 0.22;
const LIGHT_NEAR: f32 = 0.42;
/// Rim: where the brighter ring sits (fraction of the radius) and its gain.
const RIM_AT: f32 = 0.9;
const RIM_GAIN: f32 = 0.6;
/// A disc's body brightness relative to its rim.
const FILL: f32 = 0.8;
/// Diagonal band: its half-width (screen heights) and the floor density
/// outside it at 100 % band.
const BAND_WIDTH: f32 = 0.42;
const BAND_FLOOR: f32 = 0.1;
/// Drift radius of a disc around its loop (fraction of its cell).
const DRIFT: f32 = 0.08;
/// Twinkle depth of a disc's brightness around its loop.
const TWINKLE: f32 = 0.2;
/// Disc radius at 0 % size (share of the 100 % radius), the smallest disc
/// of a layer (share of its radius), and how far a disc strays in its cell.
const RADIUS_MIN: f32 = 0.45;
const SIZE_SPREAD: f32 = 0.6;
const JITTER: f32 = 0.7;
/// Edge softness of an in-focus disc (share of its radius).
const FOCUS_SOFT: f32 = 0.04;
/// Rim ring: softness (in edge widths) and width (share of the radius).
const RIM_SOFT: f32 = 1.5;
const RIM_WIDTH: f32 = 0.08;
/// A disc picks among this many accents, lifted this much.
const ACCENT_PICKS: f32 = 4.0;
const DISC_LIFT: f32 = 0.04;
/// Density range (share of cells lit) at 0 % and 100 %.
const DENSITY: (f32, f32) = (0.15, 0.85);
/// The band's angle range (radians), its offset range (screen heights),
/// how much wider the ground's light is than the band, and the glow it
/// adds (light mode keeps a share).
const BAND_ANGLE: (f32, f32) = (-0.9, -0.3);
const BAND_SHIFT: f32 = 0.1;
const GROUND_BAND: f32 = 1.6;
const BAND_GLOW: f32 = 0.25;
const LIGHT_BAND_GLOW: f32 = 0.4;
/// Ground tones along the ramp (dark, light).
const GROUND_TONE: f32 = 0.22;
const LIGHT_GROUND_TONE: f32 = 0.2;

struct Layer {
    cell: f32,
    radius: f32,
    soft: f32,
    light: f32,
    seed: u32,
}

pub struct Bokeh {
    ground_a: Lab,
    ground_b: Lab,
    glow: Lab,
    layers: Vec<Layer>,
    density: f32,
    rim: f32,
    band: f32,
    band_dir: (f32, f32),
    band_off: f32,
    scheme: crate::palette::Scheme,
    t: f32,
    motion: f32,
    px: f32,
}

impl Bokeh {
    pub fn new(ctx: &Ctx) -> Self {
        let s = &ctx.scheme;
        let mut rng = ctx.rng(61);
        let size = ctx.u("size");
        let layers = (0..LAYERS)
            .map(|i| {
                let k = i as f32 / (LAYERS - 1) as f32;
                let cell = lerp(CELL_FAR, CELL_NEAR, k);
                let radius = cell * RADIUS * lerp(RADIUS_MIN, 1.0, size);
                Layer {
                    cell,
                    radius,
                    soft: radius * (FOCUS_SOFT + DEFOCUS * ctx.u("blur") * lerp(0.5, 1.0, k)),
                    light: lerp(LIGHT_FAR, LIGHT_NEAR, k) * ctx.u("brightness"),
                    seed: rng.seed32(),
                }
            })
            .collect();
        let angle = rng.range(BAND_ANGLE.0, BAND_ANGLE.1);
        let (ground_a, ground_b) = if s.light { (s.ramp(0.0), s.ramp(LIGHT_GROUND_TONE)) } else { (s.deep(), s.ramp(GROUND_TONE)) };
        Bokeh {
            ground_a,
            ground_b,
            glow: s.accent(0),
            layers,
            density: lerp(DENSITY.0, DENSITY.1, ctx.u("density")),
            rim: ctx.u("rim"),
            band: ctx.u("band"),
            band_dir: (crate::fmath::cos(angle), crate::fmath::sin(angle)),
            band_off: rng.range(-BAND_SHIFT, BAND_SHIFT),
            scheme: s.clone(),
            t: ctx.t,
            motion: ctx.motion(),
            px: ctx.px,
        }
    }

    /// Distance from the band's centre line.
    fn band_dist(&self, x: f32, y: f32) -> f32 {
        (-x * self.band_dir.1 + y * self.band_dir.0 - self.band_off).abs()
    }
}

impl Scene for Bokeh {
    fn shade(&self, x: f32, y: f32) -> Lab {
        let bd = self.band_dist(x, y);
        let mut c = self.ground_a.mix(self.ground_b, exp(-(bd / (BAND_WIDTH * GROUND_BAND)).powi(2)));
        c = super::light(c, self.glow, BAND_GLOW * exp(-(bd / BAND_WIDTH).powi(2)) * if self.scheme.light { LIGHT_BAND_GLOW } else { 1.0 });
        for (li, l) in self.layers.iter().enumerate() {
            let (cx, cy) = ((x / l.cell).floor() as i32, (y / l.cell).floor() as i32);
            for jy in cy - 1..=cy + 1 {
                for jx in cx - 1..=cx + 1 {
                    let h = hash3(jx, jy, 0, l.seed);
                    let px0 = (jx as f32 + 0.5) * l.cell;
                    let py0 = (jy as f32 + 0.5) * l.cell;
                    // Fewer discs away from the band.
                    let keep = self.density * lerp(1.0, BAND_FLOOR + (1.0 - BAND_FLOOR) * exp(-(self.band_dist(px0, py0) / BAND_WIDTH).powi(2)), self.band);
                    if unit(h) > keep {
                        continue;
                    }
                    let phase = unit(hash3(jx, jy, 1, l.seed));
                    let a = std::f32::consts::TAU * (self.t + phase);
                    let drift = DRIFT * l.cell * self.motion;
                    let ox = (unit(hash3(jx, jy, 2, l.seed)) - 0.5) * l.cell * JITTER + drift * crate::fmath::cos(a);
                    let oy = (unit(hash3(jx, jy, 3, l.seed)) - 0.5) * l.cell * JITTER + drift * crate::fmath::sin(a);
                    let r = l.radius * lerp(SIZE_SPREAD, 1.0, unit(hash3(jx, jy, 4, l.seed)));
                    let d = ((x - px0 - ox).powi(2) + (y - py0 - oy).powi(2)).sqrt();
                    if d > r + l.soft + self.px {
                        continue;
                    }
                    let w = l.soft + self.px;
                    let inside = 1.0 - smoothstep(r - w, r + w, d);
                    let rim = (1.0 - smoothstep(0.0, w * RIM_SOFT + r * RIM_WIDTH, (d - r * RIM_AT).abs())) * self.rim * RIM_GAIN;
                    let twinkle = 1.0 - TWINKLE * self.motion.min(1.0) * 0.5 * (1.0 + crate::fmath::cos(a * 2.0));
                    let color = self.scheme.accent(li + (unit(hash3(jx, jy, 5, l.seed)) * ACCENT_PICKS) as usize).lighten(DISC_LIFT);
                    c = super::mark(c, color, l.light * inside * (FILL + rim) * twinkle, self.scheme.light);
                }
            }
        }
        c
    }
}
