//! Low poly: a jittered grid of vertices, each quad split along a hashed
//! diagonal, every facet flat-colored from a palette gradient and lit by
//! its true 3D normal. Facet edges are antialiased by rotated-grid
//! supersampling, only on pixels within a pixel of an edge, so the cost
//! stays one sample almost everywhere.

use crate::color::Lab;
use crate::fmath::{lerp, smoothstep};
use crate::noise::{Fractal, hash3, unit};
use crate::render::{Ctx, Scene};

/// Vertices move at most this fraction of a cell at 100 % irregularity
/// (under ½ keeps every quad convex).
const JITTER: f32 = 0.4;
/// Vertex height range, in cells, at 100 % relief.
const HEIGHT: f32 = 0.55;
/// Height of the light above the screen plane (normal-lighting z).
const LIGHT_Z: f32 = 0.9;
/// Lightness swing of facet shading at 100 % relief.
const SHADE_SWING: f32 = 0.22;
/// Color variation: smooth field amplitude and per-facet jitter (ramp units).
const VARY_FIELD: f32 = 0.22;
const VARY_FACET: f32 = 0.07;
/// Edge-line half-width (screen heights) and the lightness it adds.
const EDGE_WIDTH: f32 = 0.0009;
const EDGE_LIFT: f32 = 0.18;
/// Vertices drift on circles of this radius (cells) around a loop.
const DRIFT: f32 = 0.06;
/// Rotated-grid supersampling offsets (pixels).
const RGSS: [(f32, f32); 4] = [(0.125, -0.375), (0.375, 0.125), (-0.125, 0.375), (-0.375, -0.125)];
/// Color variation field: frequency (per screen height) and octaves.
const VARY_FREQ: f32 = 1.4;
const VARY_OCTAVES: u32 = 3;
/// The ramp span the gradient covers.
const GRADIENT_SPAN: (f32, f32) = (0.08, 0.92);
/// Facet shading gain.
const SHADE_GAIN: f32 = 2.5;
/// Edge-line antialiasing ramp (pixels), and the edge lightness sign and
/// share in light mode (pale facets get darker seams).
const AA_SHARE: f32 = 0.7;
const LIGHT_EDGE: f32 = -0.6;

pub struct LowPoly {
    cell: f32,
    jitter: f32,
    height: f32,
    seed: u32,
    vary: Fractal,
    variation: f32,
    relief: f32,
    edges: f32,
    grad: (f32, f32),
    extent: f32,
    light: (f32, f32, f32),
    scheme: crate::palette::Scheme,
    drift: f32,
    t: f32,
    px: f32,
    preview: bool,
}

type V3 = (f32, f32, f32);

impl LowPoly {
    pub fn new(ctx: &Ctx) -> Self {
        let cell = 1.0 / ctx.p("cells");
        let ga = ctx.p("gradient_angle").to_radians();
        let (lx, ly) = ctx.light_dir();
        let ln = (lx * lx + ly * ly + LIGHT_Z * LIGHT_Z).sqrt();
        let grad = (crate::fmath::cos(ga), -crate::fmath::sin(ga));
        LowPoly {
            cell,
            jitter: ctx.u("jitter") * JITTER,
            height: ctx.u("relief") * HEIGHT * cell,
            seed: ctx.seed32(71),
            vary: Fractal::new(ctx.seed32(72), VARY_FREQ, VARY_OCTAVES),
            variation: ctx.u("variation"),
            relief: ctx.u("relief"),
            edges: ctx.u("edges"),
            grad,
            extent: (grad.0.abs() * ctx.aspect + grad.1.abs()) * 0.5,
            light: (lx / ln, ly / ln, LIGHT_Z / ln),
            scheme: ctx.scheme.clone(),
            drift: DRIFT * ctx.motion(),
            t: ctx.t,
            px: ctx.px,
            preview: ctx.preview,
        }
    }

    /// Vertex `(i, j)` in screen space, with its height.
    fn vertex(&self, i: i32, j: i32) -> V3 {
        let h = |k| unit(hash3(i, j, k, self.seed));
        let a = std::f32::consts::TAU * (self.t + h(3));
        let jx = (h(0) - 0.5) * 2.0 * self.jitter + self.drift * crate::fmath::cos(a);
        let jy = (h(1) - 0.5) * 2.0 * self.jitter + self.drift * crate::fmath::sin(a);
        ((i as f32 + jx) * self.cell, (j as f32 + jy) * self.cell, (h(2) - 0.5) * self.height)
    }

    /// The facet containing `(x, y)` and the distance to its nearest edge.
    fn locate(&self, x: f32, y: f32) -> ([V3; 3], f32, (i32, i32, i32)) {
        let (ci, cj) = ((x / self.cell).floor() as i32, (y / self.cell).floor() as i32);
        let mut fallback = None;
        for (di, dj) in [(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
            let (i, j) = (ci + di, cj + dj);
            let (a, b, c, d) = (self.vertex(i, j), self.vertex(i + 1, j), self.vertex(i + 1, j + 1), self.vertex(i, j + 1));
            let tris = if hash3(i, j, 9, self.seed) & 1 == 0 { [[a, b, c], [a, c, d]] } else { [[a, b, d], [b, c, d]] };
            for (k, t) in tris.into_iter().enumerate() {
                let e = edge_distances(t, x, y);
                if e >= 0.0 {
                    return (t, e, (i, j, k as i32));
                }
                if fallback.is_none() {
                    fallback = Some((t, 0.0, (i, j, k as i32)));
                }
            }
        }
        // Unreachable with convex quads; keep the nearest candidate.
        fallback.unwrap_or(([(0.0, 0.0, 0.0); 3], 0.0, (0, 0, 0)))
    }

    fn facet_color(&self, t: [V3; 3], id: (i32, i32, i32)) -> Lab {
        let (cx, cy) = ((t[0].0 + t[1].0 + t[2].0) / 3.0, (t[0].1 + t[1].1 + t[2].1) / 3.0);
        let g = 0.5 + 0.5 * (cx * self.grad.0 + cy * self.grad.1) / self.extent;
        let jitter = unit(hash3(id.0, id.1, 11 + id.2, self.seed)) - 0.5;
        let pos = lerp(GRADIENT_SPAN.0, GRADIENT_SPAN.1, g) + self.variation * (VARY_FIELD * self.vary.at(cx, cy, 0.0, self.cell) + VARY_FACET * jitter);
        let mut c = self.scheme.ramp(pos.clamp(0.0, 1.0));
        // Flat shading from the facet's 3D normal.
        let (ux, uy, uz) = (t[1].0 - t[0].0, t[1].1 - t[0].1, t[1].2 - t[0].2);
        let (vx, vy, vz) = (t[2].0 - t[0].0, t[2].1 - t[0].1, t[2].2 - t[0].2);
        let (mut nx, mut ny, mut nz) = (uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx);
        if nz < 0.0 {
            (nx, ny, nz) = (-nx, -ny, -nz);
        }
        let n = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-9);
        // Screen y points down; the light vector is in the same axes.
        let lambert = (nx * self.light.0 + ny * self.light.1 + nz * self.light.2) / n;
        c.l += self.relief * SHADE_SWING * (lambert - self.light.2) * SHADE_GAIN;
        c
    }

    fn sample(&self, x: f32, y: f32) -> (Lab, f32) {
        let (t, e, id) = self.locate(x, y);
        (self.facet_color(t, id), e)
    }
}

/// Signed distance from `(x, y)` to the nearest edge of triangle `t`,
/// positive inside (orientation-independent).
fn edge_distances(t: [V3; 3], x: f32, y: f32) -> f32 {
    let area = (t[1].0 - t[0].0) * (t[2].1 - t[0].1) - (t[1].1 - t[0].1) * (t[2].0 - t[0].0);
    let s = if area < 0.0 { -1.0 } else { 1.0 };
    let mut m = f32::MAX;
    for k in 0..3 {
        let (a, b) = (t[k], t[(k + 1) % 3]);
        let (ex, ey) = (b.0 - a.0, b.1 - a.1);
        let len = (ex * ex + ey * ey).sqrt().max(1e-9);
        let d = s * (ex * (y - a.1) - ey * (x - a.0)) / len;
        m = m.min(d);
    }
    m
}

impl Scene for LowPoly {
    fn shade(&self, x: f32, y: f32) -> Lab {
        let (mut c, e) = self.sample(x, y);
        if e < self.px && !self.preview {
            // Near an edge: average four rotated-grid samples.
            let mut sum = Lab::default();
            for (ox, oy) in RGSS {
                sum = sum.add(self.sample(x + ox * self.px, y + oy * self.px).0);
            }
            c = sum.scale(1.0 / RGSS.len() as f32);
        }
        if self.edges > 0.0 {
            let w = EDGE_WIDTH.max(self.px * 0.5);
            let energy = (EDGE_WIDTH / w).min(1.0);
            let line = 1.0 - smoothstep(w - self.px * AA_SHARE, w + self.px * AA_SHARE, e.max(0.0));
            let sign = if self.scheme.light { LIGHT_EDGE } else { 1.0 };
            c.l += EDGE_LIFT * sign * self.edges * line * energy;
        }
        c
    }
}
