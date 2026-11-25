//! Contours: a topographic map. A domain-warped terrain is shaded with a
//! soft hypsometric tint and hillshade from the light angle, and drawn with
//! glowing contour lines colored by elevation through the palette. Line
//! width is true screen distance (level offset over the terrain gradient),
//! so lines are crisp and even at every resolution; every fifth is an
//! emphasised index line, and where lines crowd closer than a few pixels
//! they fade rather than moiré.

use crate::color::Lab;
use crate::fmath::{exp, fract, lerp, smoothstep};
use crate::noise::Fractal;
use crate::render::{Ctx, Scene};

/// Terrain frequency (cycles per screen height) at 0 % and 100 % scale.
const FREQ_MIN: f32 = 2.4;
const FREQ_MAX: f32 = 0.55;
/// Warp displacement at 100 % (terrain units).
const WARP: f32 = 0.8;
/// Line half-width (screen heights) at 0 % and 100 % weight.
const WIDTH_MIN: f32 = 0.0003;
const WIDTH_MAX: f32 = 0.0015;
/// Every this many lines is an index line, this much wider at 100 %.
const INDEX_EVERY: i32 = 5;
const INDEX_WIDEN: f32 = 1.0;
/// Finite-difference step for gradients (screen heights).
const EPS: f32 = 0.0005;
/// Hillshade: vertical exaggeration and lightness swing at 100 %.
const RELIEF_SCALE: f32 = 0.05;
const RELIEF_SWING: f32 = 0.09;
/// Elevation tint: ramp span used by the fill at 100 %.
const TINT_SPAN: f32 = 0.35;
/// Light a line adds (and its glow), at full emphasis.
const LINE_LIGHT: f32 = 0.55;
const GLOW_DIST: f32 = 0.003;
const GLOW_GAIN: f32 = 0.18;
/// Lines closer than this many pixels fade out, and are fully back at
/// this multiple of it.
const CROWD_PX: f32 = 3.0;
const CROWD_CLEAR: f32 = 1.5;
/// Terrain noise octaves and gain; warp noise frequency (share of the
/// terrain's), octaves, and the second warp field's offset.
const TERRAIN_OCTAVES: u32 = 4;
const TERRAIN_GAIN: f32 = 0.42;
const WARP_FREQ: f32 = 0.6;
const WARP_OCTAVES: u32 = 2;
const WARP_Y_SHIFT: (f32, f32) = (-2.1, 4.4);
/// The tint runs between these elevations.
const TINT_FROM: f32 = 0.05;
const TINT_TO: f32 = 0.95;
/// Light mode keeps this share of the hillshade.
const LIGHT_RELIEF: f32 = 0.6;
/// Line antialiasing ramp (pixels).
const AA_SHARE: f32 = 0.75;
/// Emphasis of ordinary and (per unit of index setting) index lines.
const LINE_EMPHASIS: f32 = 0.55;
const INDEX_EMPHASIS: f32 = 0.6;
/// Light mode: ink lines are this much darker than their color and laid
/// down this much more strongly.
const INK_DARKEN: f32 = -0.15;
const INK_GAIN: f32 = 1.2;

pub struct Contours {
    terrain: Fractal,
    warp_x: Fractal,
    warp_y: Fractal,
    warp: f32,
    levels: f32,
    width: f32,
    relief: f32,
    fill: f32,
    index: f32,
    scheme: crate::palette::Scheme,
    light: (f32, f32),
    z: f32,
    px: f32,
}

impl Contours {
    pub fn new(ctx: &Ctx) -> Self {
        let freq = lerp(FREQ_MIN, FREQ_MAX, ctx.u("scale"));
        Contours {
            terrain: Fractal::new(ctx.seed32(51), freq, TERRAIN_OCTAVES).gain(TERRAIN_GAIN),
            warp_x: Fractal::new(ctx.seed32(52), freq * WARP_FREQ, WARP_OCTAVES),
            warp_y: Fractal::new(ctx.seed32(53), freq * WARP_FREQ, WARP_OCTAVES),
            warp: ctx.u("warp") * WARP / freq,
            levels: ctx.p("levels"),
            width: lerp(WIDTH_MIN, WIDTH_MAX, ctx.u("line_width")),
            relief: ctx.u("relief"),
            fill: ctx.u("fill"),
            index: ctx.u("index"),
            scheme: ctx.scheme.clone(),
            light: ctx.light_dir(),
            z: ctx.z(crate::noise::LOOP_PERIOD),
            px: ctx.px,
        }
    }

    /// Elevation in [0, 1].
    fn height(&self, x: f32, y: f32) -> f32 {
        let wx = self.warp_x.at(x, y, self.z, self.px);
        let wy = self.warp_y.at(x + WARP_Y_SHIFT.0, y + WARP_Y_SHIFT.1, self.z, self.px);
        (0.5 + 0.5 * self.terrain.at(x + self.warp * wx, y + self.warp * wy, self.z, self.px)).clamp(0.0, 1.0)
    }
}

impl Scene for Contours {
    fn shade(&self, x: f32, y: f32) -> Lab {
        let h = self.height(x, y);
        let hx = (self.height(x + EPS, y) - h) / EPS;
        let hy = (self.height(x, y + EPS) - h) / EPS;
        let e = smoothstep(TINT_FROM, TINT_TO, h);
        // Soft hypsometric tint: deep lowlands, slightly lifted highlands.
        let mut c = self.scheme.ramp(lerp(0.0, TINT_SPAN * self.fill, e));
        let (nx, ny) = (-hx * RELIEF_SCALE, -hy * RELIEF_SCALE);
        let shade = (nx * self.light.0 + ny * self.light.1) / (1.0 + nx * nx + ny * ny).sqrt();
        c.l += self.relief * RELIEF_SWING * shade.clamp(-1.0, 1.0) * if self.scheme.light { LIGHT_RELIEF } else { 1.0 };
        // Contour lines, colored by their elevation.
        let grad = (hx * hx + hy * hy).sqrt().max(1e-4);
        let k = h * self.levels;
        let dist = (fract(k + 0.5) - 0.5).abs() / (self.levels * grad);
        let level = (k + 0.5).floor() as i32;
        let is_index = level.rem_euclid(INDEX_EVERY) == 0;
        let width = if is_index { self.width * (1.0 + INDEX_WIDEN * self.index) } else { self.width };
        let w = width.max(self.px * 0.5);
        let energy = (width / w).min(1.0);
        let aa = self.px * AA_SHARE;
        let line = (1.0 - smoothstep(w - aa, w + aa, dist)) * energy;
        let glow = GLOW_GAIN * exp(-dist / GLOW_DIST);
        let spacing_px = 1.0 / (self.levels * grad * self.px);
        let crowd = smoothstep(CROWD_PX * 0.5, CROWD_PX * CROWD_CLEAR, spacing_px);
        let emphasis = if is_index { INDEX_EMPHASIS + INDEX_EMPHASIS * self.index } else { LINE_EMPHASIS };
        let color = self.scheme.ramp(lerp(0.5, 1.0, e));
        if self.scheme.light {
            // On a pale map, lines are ink: darker and more colorful.
            c = c.mix_vivid(color.lighten(INK_DARKEN), (line * emphasis * INK_GAIN + glow * 0.5).min(1.0) * crowd);
        } else {
            c = super::light(c, color, (line * LINE_LIGHT + glow) * emphasis * crowd);
        }
        c
    }
}
