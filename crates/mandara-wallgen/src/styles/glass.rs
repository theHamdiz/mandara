//! Glass: large frosted panes floating over a lit backdrop, in the spirit
//! of Mandara's chromatic-glass material. Each pane is a long rotated
//! rounded rectangle (a ribbon, a pill or a slab) crossing the frame.
//!
//! The backdrop is a soft color field strewn with light orbs, so the glass
//! has something to bend: seen through a pane, the backdrop and the panes
//! behind it shift (thick glass displaces the view), bend at the bevelled
//! rim, split into their color channels there (prism dispersion) and blur
//! (frost). Blur is analytic: orbs are Gaussians, and a Gaussian seen
//! through a Gaussian frost is a wider Gaussian, so frost costs nothing.
//!
//! Panes composite back to front. Each refracts the panes behind it
//! (drawn flat, so the cost stays linear) and the shadows those cast, but
//! never its own shadow (that falls far behind a floating pane, and seen
//! through itself it read as a stain). Glass passes most light: its
//! shadow is faint, and light focused by the rim pools in a bright,
//! faintly spectral caustic inside it. The rim carries the 1 px-style
//! directional specular light, with a bevel highlight just inside it.

use crate::color::Lab;
use crate::fmath::{atan2, cos, exp, lerp, pow, sin, smoothstep};
use crate::render::{Ctx, Scene};

// Panes.

/// Pane half-thickness (screen heights) at 0 % and 100 % size, and the
/// per-pane scale range.
const SIZE_MIN: f32 = 0.07;
const SIZE_MAX: f32 = 0.24;
const SIZE_SCALE: (f32, f32) = (0.65, 1.2);
/// Pane half-length as a share of the screen width (they run off it).
const RIBBON_LENGTH: (f32, f32) = (0.55, 0.9);
/// The panes' common angle lies within ± this (radians)…
const RIBBON_ANGLE: f32 = 0.7;
/// …and each deviates from it by up to this, so they cross.
const RIBBON_SPREAD: f32 = 0.45;
/// Pane centres spread over this share of the screen (width and height).
const GATHER: f32 = 0.8;
/// Panes turn by this much (radians) and drift this far around a loop.
const TURN: f32 = 0.05;
const DRIFT: f32 = 0.03;

// Backdrop.

/// Color field: blob radius (screen heights), count, and the ground's
/// weight in the blend.
const FIELD_RADIUS: f32 = 0.32;
const FIELD_BLOBS: usize = 4;
const FIELD_GROUND: f32 = 0.6;
/// Field blob tones: the ramp span they take (deep to mid, so the orbs
/// and the glass glow against them), and the share of accent mixed in.
const FIELD_TONE: (f32, f32) = (0.2, 0.55);
const FIELD_ACCENT: f32 = 0.3;
/// Light orbs: count, radius range (screen heights) and light added at
/// the centre.
const ORBS: usize = 5;
const ORB_RADIUS: (f32, f32) = (0.07, 0.17);
const ORB_LIGHT: (f32, f32) = (0.35, 0.75);
/// Orbs spread over this share of the screen.
const ORB_SPREAD: f32 = 1.05;

// Refraction.

/// View shift through the pane's body (screen heights) at 100 %
/// refraction, against the light (thick glass displaces what is behind).
const THICK_SHIFT: f32 = 0.05;
/// Rim lens: extra outward bend (screen heights) at the edge at 100 %, and
/// the distance inside the edge over which it fades.
const REFRACT: f32 = 0.06;
const LENS: f32 = 0.05;
/// Dispersion at 100 %: the red and blue channels bend this much more and
/// less than green.
const DISPERSE: f32 = 0.3;
/// Dispersion only splits the rim lens where its bend exceeds this share.
const DISPERSE_FROM: f32 = 0.02;
/// Frost blur radius (screen heights) at 100 %.
const FROST_BLUR: f32 = 0.07;
/// Frost at 100 %: lightness lift and the share of chroma it removes.
const FROST_LIFT: f32 = 0.06;
const FROST_DESATURATE: f32 = 0.18;
/// Pane tint: how much of its palette color a pane lays over what it shows.
const TINT: f32 = 0.14;

// Rim and surface.

/// Rim line width (screen heights) and the light / shade it adds.
const RIM_WIDTH: f32 = 0.0022;
const RIM_LIGHT: f32 = 1.4;
const RIM_SHADE: f32 = 0.12;
/// The rim lights the sides facing the light along this power of the
/// facing (tighter than cosine), in this near-white (lightness, chroma
/// kept); the bevel's and face highlights take the second white.
const RIM_FOCUS: f32 = 1.5;
const RIM_WHITE: (f32, f32) = (0.98, 0.2);
const HIGHLIGHT_WHITE: (f32, f32) = (0.99, 0.15);
/// The key light leans this far toward the light angle (the rest faces
/// the screen).
const KEY_TILT: f32 = 0.5;
/// Pane edges take this share of the softness setting.
const EDGE_SOFTNESS: f32 = 0.3;
/// Bevel: width inside the edge (screen heights), its normal tilt, and the
/// half-vector cosines between which its highlight rises.
const BEVEL: f32 = 0.014;
const BEVEL_TILT: f32 = 0.9;
const BEVEL_FROM: f32 = 0.9;
const BEVEL_TO: f32 = 0.985;
const BEVEL_LIGHT: f32 = 0.55;
/// Broad inner edge glow (Fresnel): falloff and lift.
const FRESNEL_WIDTH: f32 = 0.03;
const FRESNEL_LIFT: f32 = 0.05;
/// Sheen across a face at 100 % rim.
const SHEEN: f32 = 0.03;
/// Panes drawn flat (as seen through a nearer pane) keep this share of
/// their rim.
const FLAT_RIM: f32 = 0.6;

// Shadows and caustics.

/// Shadow: offset away from the light, blur and (faint: glass is clear)
/// depth.
const SHADOW_OFFSET: f32 = 0.05;
const SHADOW_BLUR: f32 = 0.03;
const SHADOW_DEPTH: f32 = 0.14;
/// The dark line under the rim (light bent away by the bevel): width and
/// depth.
const EDGE_SHADOW_WIDTH: f32 = 0.012;
const EDGE_SHADOW: f32 = 0.12;
/// Caustic pool just inside the shadow's edge: inset, width and light at
/// 100 %; and the light the pane's body focuses into its whole shadow.
const CAUSTIC_INSET: f32 = 0.03;
const CAUSTIC_WIDTH: f32 = 0.025;
const CAUSTIC_LIGHT: f32 = 0.35;
const POOL_LIGHT: f32 = 0.1;
/// How fast the caustic runs through the spectrum (radians per screen
/// height), and its color's lightness and chroma.
const CAUSTIC_SPECTRUM: f32 = 60.0;
const CAUSTIC_L: f32 = 0.88;
const CAUSTIC_C: f32 = 0.05;
/// The caustic leans toward its pane's tint: lift and share.
const CAUSTIC_TINT: (f32, f32) = (0.3, 0.4);
/// Light mode lays orbs down as a vivid wash this strong (per unit of
/// light).
const LIGHT_ORB_MIX: f32 = 0.9;
/// Light mode keeps this share of glows, caustics and frost lift.
const LIGHT_SOFTEN: f32 = 0.5;
/// Finite-difference step for the edge gradient (screen heights).
const EPS: f32 = 0.0006;

struct Shape {
    x: f32,
    y: f32,
    hw: f32,
    hh: f32,
    radius: f32,
    cos_a: f32,
    sin_a: f32,
    tint: Lab,
}

impl Shape {
    /// Signed distance (negative inside) of the rounded rectangle.
    #[inline]
    fn sdf(&self, x: f32, y: f32) -> f32 {
        let (dx, dy) = (x - self.x, y - self.y);
        let (lx, ly) = (dx * self.cos_a + dy * self.sin_a, -dx * self.sin_a + dy * self.cos_a);
        let (qx, qy) = (lx.abs() - self.hw + self.radius, ly.abs() - self.hh + self.radius);
        let outside = (qx.max(0.0) * qx.max(0.0) + qy.max(0.0) * qy.max(0.0)).sqrt();
        outside + qx.max(qy).min(0.0) - self.radius
    }

    /// Unit outward normal of the edge nearest `(x, y)`.
    #[inline]
    fn normal(&self, x: f32, y: f32, d: f32) -> (f32, f32) {
        let gx = (self.sdf(x + EPS, y) - d) / EPS;
        let gy = (self.sdf(x, y + EPS) - d) / EPS;
        let gl = (gx * gx + gy * gy).sqrt().max(1e-6);
        (gx / gl, gy / gl)
    }
}

struct Blob {
    x: f32,
    y: f32,
    color: Lab,
}

struct Orb {
    x: f32,
    y: f32,
    s2: f32,
    light: f32,
    color: Lab,
}

pub struct Glass {
    ground: Lab,
    field: Vec<Blob>,
    orbs: Vec<Orb>,
    /// Back to front.
    shapes: Vec<Shape>,
    refraction: f32,
    dispersion: f32,
    frost: f32,
    rim: f32,
    caustics: f32,
    light: (f32, f32),
    half: (f32, f32, f32),
    edge: f32,
    light_mode: bool,
}

impl Glass {
    pub fn new(ctx: &Ctx) -> Self {
        let s = &ctx.scheme;
        let mut rng = ctx.rng(101);
        let field = super::spread(&mut rng, FIELD_BLOBS, ctx.aspect, 1.0)
            .into_iter()
            .enumerate()
            .map(|(i, (x, y))| Blob { x, y, color: s.ramp(lerp(FIELD_TONE.0, FIELD_TONE.1, i as f32 / (FIELD_BLOBS - 1) as f32)).mix_vivid(s.accent(i), FIELD_ACCENT) })
            .collect();
        let orbs = super::spread(&mut rng, ORBS, ctx.aspect * ORB_SPREAD, ORB_SPREAD)
            .into_iter()
            .enumerate()
            .map(|(i, (x, y))| {
                let r = rng.range(ORB_RADIUS.0, ORB_RADIUS.1);
                Orb { x, y, s2: r * r, light: rng.range(ORB_LIGHT.0, ORB_LIGHT.1), color: s.accent(i + 1) }
            })
            .collect();
        let n = ctx.n("shapes");
        let size = lerp(SIZE_MIN, SIZE_MAX, ctx.u("size"));
        let round = ctx.u("roundness");
        // Long panes crossing the frame, angled around one family direction
        // so they read as a composition, not scattered objects.
        let family = rng.range(-RIBBON_ANGLE, RIBBON_ANGLE);
        let shapes = super::spread(&mut rng, n, ctx.aspect * GATHER, GATHER)
            .into_iter()
            .enumerate()
            .map(|(i, (x, y))| {
                let (hw, hh) = (ctx.aspect * rng.range(RIBBON_LENGTH.0, RIBBON_LENGTH.1), size * rng.range(SIZE_SCALE.0, SIZE_SCALE.1));
                let (dx, dy) = ctx.orbit(1.0, rng.f(), DRIFT);
                let a = family + rng.range(-RIBBON_SPREAD, RIBBON_SPREAD) + TURN * ctx.motion() * sin(ctx.angle(1.0, rng.f()));
                Shape { x: x + dx, y: y + dy, hw, hh, radius: hw.min(hh) * round, cos_a: cos(a), sin_a: sin(a), tint: s.accent(i + 2) }
            })
            .collect();
        let light = ctx.light_dir();
        // Half vector between a raised key light and the viewer.
        let rise = (1.0 - KEY_TILT * KEY_TILT).sqrt();
        let (hx, hy, hz) = (light.0 * KEY_TILT, light.1 * KEY_TILT, rise + 1.0);
        let hl = (hx * hx + hy * hy + hz * hz).sqrt();
        Glass {
            ground: if s.light { s.ramp(0.0) } else { s.deep() },
            field,
            orbs,
            shapes,
            refraction: ctx.u("refraction"),
            dispersion: ctx.u("dispersion") * DISPERSE,
            frost: ctx.u("frost"),
            rim: ctx.u("rim"),
            caustics: ctx.u("caustics"),
            light,
            half: (hx / hl, hy / hl, hz / hl),
            edge: ctx.edge_width(EDGE_SOFTNESS),
            light_mode: s.light,
        }
    }

    #[inline]
    fn soften(&self) -> f32 {
        if self.light_mode { LIGHT_SOFTEN } else { 1.0 }
    }

    /// The backdrop seen through a blur of radius `blur`: the color field
    /// (too broad for frost to change) and the orbs (Gaussians widened by
    /// the blur, energy kept).
    fn backdrop(&self, x: f32, y: f32, blur: f32) -> Lab {
        let (mut sum, mut w) = (self.ground.scale(FIELD_GROUND), FIELD_GROUND);
        let mut chroma = self.ground.chroma() * FIELD_GROUND;
        for b in &self.field {
            let d2 = (x - b.x) * (x - b.x) + (y - b.y) * (y - b.y);
            let wi = exp(-d2 / (2.0 * FIELD_RADIUS * FIELD_RADIUS));
            sum = sum.add(b.color.scale(wi));
            chroma += b.color.chroma() * wi;
            w += wi;
        }
        let mut c = sum.scale(1.0 / w).with_chroma(chroma / w);
        let b2 = blur * blur;
        for o in &self.orbs {
            let s2 = o.s2 + b2;
            let d2 = (x - o.x) * (x - o.x) + (y - o.y) * (y - o.y);
            let a = o.light * (o.s2 / s2) * exp(-d2 / (2.0 * s2));
            // Light on a dark ground; a vivid wash on a pale one.
            c = if self.light_mode { c.mix_vivid(o.color, (a * LIGHT_ORB_MIX).min(1.0)) } else { super::light(c, o.color, a) };
        }
        c
    }

    /// Shadow and caustics of `shape` on what lies behind it.
    fn cast(&self, shape: &Shape, x: f32, y: f32, mut c: Lab, blur: f32) -> Lab {
        let ds = shape.sdf(x + self.light.0 * SHADOW_OFFSET, y + self.light.1 * SHADOW_OFFSET);
        let soft = SHADOW_BLUR + blur;
        if ds > soft * 2.0 {
            return c;
        }
        let inside = 1.0 - smoothstep(-soft, soft, ds);
        // A faint shadow, darkest in a line under the rim (light the bevel
        // bent away), with focused light pooling just inside it.
        let rim_line = exp(-(ds / (EDGE_SHADOW_WIDTH + blur)).powi(2));
        c.l *= 1.0 - SHADOW_DEPTH * inside - EDGE_SHADOW * rim_line;
        if self.caustics > 0.0 {
            let w = CAUSTIC_WIDTH + blur;
            let band = exp(-((ds + CAUSTIC_INSET) / w).powi(2));
            let hue = atan2(y - shape.y, x - shape.x) * 2.0 + ds * CAUSTIC_SPECTRUM;
            let spectral = Lab::from_lch(CAUSTIC_L, CAUSTIC_C, hue).mix(shape.tint.lighten(CAUSTIC_TINT.0), CAUSTIC_TINT.1);
            let amount = self.caustics * self.soften() * (CAUSTIC_LIGHT * band + POOL_LIGHT * inside);
            c = super::light(c, spectral, amount);
        }
        c
    }

    /// The backdrop with the shadows of the first `behind` panes, and those
    /// panes drawn flat over it (as seen through a nearer pane).
    fn behind(&self, behind: usize, x: f32, y: f32, blur: f32) -> Lab {
        let mut c = self.backdrop(x, y, blur);
        for s in &self.shapes[..behind] {
            c = self.cast(s, x, y, c, blur);
        }
        for s in &self.shapes[..behind] {
            let d = s.sdf(x, y);
            let w = self.edge + blur;
            if d > w {
                continue;
            }
            let cov = smoothstep(w, -w, d);
            let rim = exp(-(d.abs() / (RIM_WIDTH + blur)).powi(2)) * self.rim * FLAT_RIM;
            let mut g = c.mix_vivid(s.tint, TINT).lighten(self.frost * FROST_LIFT * self.soften());
            g = super::light(g, Lab::new(RIM_WHITE.0, g.a * RIM_WHITE.1, g.b * RIM_WHITE.1), rim * self.soften());
            c = c.mix(g, cov);
        }
        c
    }

    /// Pane `i` at `(x, y)` (signed distance `d`): the refracted, frosted,
    /// dispersed view of what lies behind it, then its rim and bevel.
    fn pane(&self, i: usize, x: f32, y: f32, d: f32) -> Lab {
        let shape = &self.shapes[i];
        let (nx, ny) = shape.normal(x, y, d);
        let blur = self.frost * FROST_BLUR;
        // Thick glass shifts the view behind it; the bevelled rim bends rays
        // outward on top of that.
        let shift = self.refraction * THICK_SHIFT;
        let bend = self.refraction * REFRACT * exp(d.min(0.0) / LENS);
        let (ox, oy) = (nx * bend - self.light.0 * shift, ny * bend - self.light.1 * shift);
        let mut c = if self.dispersion > 0.0 && bend > self.refraction * REFRACT * DISPERSE_FROM {
            // Each channel bends by a slightly different amount at the rim.
            let k = self.dispersion;
            let at = |s: f32| self.behind(i, x + nx * bend * s - self.light.0 * shift, y + ny * bend * s - self.light.1 * shift, blur).to_linear();
            let (r, g, b) = (at(1.0 + k)[0], at(1.0)[1], at(1.0 - k)[2]);
            Lab::from_linear([r.max(0.0), g.max(0.0), b.max(0.0)])
        } else {
            self.behind(i, x + ox, y + oy, blur)
        };
        // Frost: a milky lift; and the pane's own faint tint.
        c = c.saturate(1.0 - self.frost * FROST_DESATURATE).lighten(self.frost * FROST_LIFT * self.soften());
        c = c.mix_vivid(shape.tint, TINT);
        // A soft sheen along the light across the face.
        let along = ((x - shape.x) * self.light.0 + (y - shape.y) * self.light.1) / shape.hw.min(shape.hh);
        c.l += SHEEN * self.rim * along.clamp(-1.0, 1.0);
        // Fresnel: the glass brightens toward its edge.
        c.l += FRESNEL_LIFT * self.rim * exp(d.min(0.0) / FRESNEL_WIDTH);
        // Bevel: inside the edge the surface tilts outward and catches the
        // key light as a crisp highlight.
        let tilt = BEVEL_TILT * exp(d.min(0.0) / BEVEL);
        let (sx, sy, sz) = (nx * tilt, ny * tilt, 1.0);
        let sl = (sx * sx + sy * sy + sz * sz).sqrt();
        let nh = (sx * self.half.0 + sy * self.half.1 + sz * self.half.2) / sl;
        let white = Lab::new(HIGHLIGHT_WHITE.0, c.a * HIGHLIGHT_WHITE.1, c.b * HIGHLIGHT_WHITE.1);
        c = super::light(c, white, BEVEL_LIGHT * self.rim * smoothstep(BEVEL_FROM, BEVEL_TO, nh) * tilt / BEVEL_TILT);
        // The directional specular rim (the chromatic-glass edge): bright
        // where it faces the light, shaded opposite.
        let facing = nx * self.light.0 + ny * self.light.1;
        let rim = exp(-(d.abs() / RIM_WIDTH).powi(2)) * self.rim;
        c = super::light(c, white, rim * RIM_LIGHT * pow(facing.max(0.0), RIM_FOCUS));
        c.l -= rim * RIM_SHADE * (-facing).max(0.0);
        c
    }
}

impl Scene for Glass {
    fn shade(&self, x: f32, y: f32) -> Lab {
        // The backdrop with every pane's shadow, then the panes back to front.
        let mut c = self.backdrop(x, y, 0.0);
        for s in &self.shapes {
            c = self.cast(s, x, y, c, 0.0);
        }
        for (i, s) in self.shapes.iter().enumerate() {
            let d = s.sdf(x, y);
            if d > self.edge {
                continue;
            }
            let cov = smoothstep(self.edge, -self.edge, d);
            c = c.mix(self.pane(i, x, y, d), cov);
        }
        c
    }
}
