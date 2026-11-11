//! Comic burst: a cel-shaded pop-art backdrop. Speed rays burst from a
//! focus in alternating flat colors, inked edges between them, a halftone
//! of dots that grow toward the corners, and a hard-edged ink vignette.
//! Everything is flat fills with hard (antialiased) edges: no gradients
//! but the dots' size, as on a printed comic page.

use crate::color::Lab;
use crate::render::{Ctx, Scene};

/// Rays at 0 % and 100 % ray count.
const RAYS: (f32, f32) = (10.0, 36.0);
/// The focus sits this far from the centre at most (screen heights).
const FOCUS_SPREAD: f32 = 0.22;
/// Ink line between two rays at 100 % ink, in radians at the focus.
const INK_RAY: f32 = 0.012;
/// The inner disc (the burst's centre) radius at 100 % size.
const CORE: f32 = 0.16;
/// Its ink ring's width (screen heights).
const CORE_INK: f32 = 0.008;
/// Halftone dot pitch (screen heights) and the largest radius as a share
/// of the pitch; dots start at this distance from the focus.
const DOT_PITCH: f32 = 0.03;
const DOT_MAX: f32 = 0.46;
const DOT_FROM: f32 = 0.18;
/// Dots read as dots from this many pixels per pitch.
const DOTS_RESOLVED: f32 = 6.0;
/// The ink of the dots and lines: OKLab lightness and chroma kept.
const INK_L: f32 = 0.16;
const INK_CHROMA: f32 = 0.2;
/// Dots darken their ray by this share at full size.
const DOT_DEPTH: f32 = 0.55;
/// Light wallpapers keep this share of the dots' and lines' ink.
const LIGHT_INK: f32 = 0.45;
/// Hard ink frame round the screen (screen heights) at 100 % frame.
const FRAME: f32 = 0.02;
/// Loop: the rays turn by one colour period (three rays) per cycle, so
/// the last frame meets the first.
const SPIN_RAYS: f32 = 3.0;

pub struct Comic {
    focus: (f32, f32),
    rays: f32,
    turn: f32,
    colors: [Lab; 3],
    ink: Lab,
    core: f32,
    ink_k: f32,
    dots: f32,
    frame: f32,
    depth: f32,
    half_w: f32,
    px: f32,
}

impl Comic {
    pub fn new(ctx: &Ctx) -> Self {
        let mut rng = ctx.rng(1);
        let s = &ctx.scheme;
        let n = s.len();
        // Three flat colors from the palette's top stops (vivid, then the
        // next two), so neighbouring rays always differ.
        let pick = |i: usize| s.stop((n - 1).saturating_sub(i).max(1).min(n - 1));
        let colors = [pick(0), pick(1), pick(2)];
        let deep = s.deep();
        let ink = Lab::from_lch(INK_L, deep.chroma() * INK_CHROMA, deep.hue());
        let focus = (rng.range(-FOCUS_SPREAD, FOCUS_SPREAD) * ctx.aspect, rng.range(-FOCUS_SPREAD, FOCUS_SPREAD));
        let rays = (RAYS.0 + (RAYS.1 - RAYS.0) * ctx.u("rays")).round().max(4.0);
        Comic {
            focus,
            rays,
            turn: rng.f() * std::f32::consts::TAU + ctx.t * SPIN_RAYS * std::f32::consts::TAU / rays,
            colors,
            ink,
            core: CORE * ctx.u("core"),
            ink_k: ctx.u("ink"),
            dots: ctx.u("halftone"),
            frame: FRAME * ctx.u("frame"),
            depth: if ctx.recipe.light { LIGHT_INK } else { 1.0 },
            half_w: ctx.aspect / 2.0,
            px: ctx.px,
        }
    }

    /// Coverage of an edge at signed distance `d` (screen heights).
    #[inline]
    fn cover(&self, d: f32) -> f32 {
        (0.5 - d / self.px).clamp(0.0, 1.0)
    }
}

impl Scene for Comic {
    fn shade(&self, x: f32, y: f32) -> Lab {
        let (dx, dy) = (x - self.focus.0, y - self.focus.1);
        let r = (dx * dx + dy * dy).sqrt();
        let a = dy.atan2(dx) + self.turn;
        let sector = std::f32::consts::TAU / self.rays;
        let k = (a / sector).floor();
        let i = (k.rem_euclid(3.0)) as usize;
        let mut c = self.colors[i % 3];
        // Ink between the rays: distance to the nearest ray edge, as an arc.
        let into = a - k * sector;
        let edge = into.min(sector - into) * r;
        // Thinner than a pixel, a line shows as its share of the pixel.
        let line = INK_RAY * self.ink_k * r.min(1.0);
        let ink = if line < self.px { (line / self.px) * self.cover(edge - self.px * 0.5) } else { self.cover(edge - line) };
        c = c.mix(self.ink, ink * self.depth);
        // Halftone dots on a 45° grid, growing with the distance.
        if self.dots > 0.0 {
            let (u, v) = ((x + y) / std::f32::consts::SQRT_2, (x - y) / std::f32::consts::SQRT_2);
            let (cu, cv) = ((u / DOT_PITCH).round() * DOT_PITCH, (v / DOT_PITCH).round() * DOT_PITCH);
            let d = ((u - cu) * (u - cu) + (v - cv) * (v - cv)).sqrt();
            let grow = ((r - DOT_FROM) / (0.9 - DOT_FROM)).clamp(0.0, 1.0) * self.dots;
            let rad = grow * DOT_MAX * DOT_PITCH;
            // Dots smaller than a pixel average out to the share of the
            // area they cover, at every resolution alike.
            let mean = std::f32::consts::PI * rad * rad / (DOT_PITCH * DOT_PITCH);
            let sharp = (DOT_PITCH / self.px / DOTS_RESOLVED).clamp(0.0, 1.0);
            let cover = self.cover(d - rad) * sharp + mean * (1.0 - sharp);
            c = c.mix(self.ink, cover * DOT_DEPTH * self.depth * f32::from(rad > 0.0));
        }
        // The burst's flat core with its ink ring.
        if self.core > 0.0 {
            let ring = self.cover(r - self.core);
            let body = self.cover(r - self.core + CORE_INK);
            c = c.mix(self.ink, ring).mix(self.colors[0].lighten(0.08), body);
        }
        // A hard ink frame round the screen, like a comic panel.
        if self.frame > 0.0 {
            let inside = (self.half_w - x.abs()).min(0.5 - y.abs());
            c = c.mix(self.ink, self.cover(inside - self.frame));
        }
        c
    }
}
