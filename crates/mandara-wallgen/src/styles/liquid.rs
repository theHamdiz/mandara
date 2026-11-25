//! Liquid: an abstract field of glossy liquid, never a lone object. The
//! shape is a metaball field: a few streams (Gaussian tubes along wavy
//! centre lines that cross the whole frame and run off its edges, their
//! width swelling and thinning) plus beads of varied scale riding them.
//! The fields add, so wherever forms meet they fuse in a smooth union (the
//! metaball form of the smooth minimum Mandara's bar uses for its pills).
//! Each stream is a depth layer: it fuses with its own beads and passes
//! over the layers behind, shading them.
//!
//! The surface has a real height (an elliptical cross-section: steep at
//! the rim, round over the body), lit by a key light whose crisp-edged
//! highlight streaks along each form's lit flank, a broad sheen, Fresnel
//! rim light
//! and a Beer–Lambert inner depth (thick liquid richer and deeper, thin
//! edges clearer). Color drifts along each stream and blends where they
//! merge. Behind lies a rich gradient ground with a soft glow, a shadow
//! falling away from the light and contact occlusion at the edges. In a
//! video loop the streams sway and the beads orbit, merging and parting.

use crate::color::Lab;
use crate::fmath::{cos, exp, fract, lerp, pow, sin, smoothstep};
use crate::noise::{Rng, noise2};
use crate::render::{Ctx, Scene};

// Layout.

/// Streams per blob count (the rest of the count are beads), and limits.
const BLOBS_PER_STREAM: usize = 2;
const MAX_STREAMS: usize = 4;
/// Stream centre lines sit within ± this (screen heights) of the centre,
/// spread evenly, then nudged by up to `STREAM_JITTER`.
const STREAM_BAND: f32 = 0.34;
const STREAM_JITTER: f32 = 0.08;
/// The streams' common direction lies within ± this (radians) of the
/// horizontal; each turns from it by up to `STREAM_SPREAD`.
const STREAM_ANGLE: f32 = 0.55;
const STREAM_SPREAD: f32 = 0.45;
/// Centre-line waves: amplitude (screen heights) and frequency (radians
/// per screen height) of the long and the short harmonic.
const WAVE_LONG: (f32, f32) = (0.16, 2.2);
const WAVE_SHORT: (f32, f32) = (0.05, 5.5);
/// Visible stream half-width (screen heights) at 0 % and 100 % size, and
/// the per-stream scale range.
const WIDTH_MIN: f32 = 0.06;
const WIDTH_MAX: f32 = 0.16;
const WIDTH_SCALE: (f32, f32) = (0.6, 1.5);
/// Width swells along a stream: depth (share) and frequency range.
const BULGE: f32 = 0.55;
const BULGE_FREQ: (f32, f32) = (1.8, 3.6);
/// Bead radius (screen heights, visible) at 0 % and 100 % size, and the
/// per-bead scale range (a few large, most smaller).
const BEAD_MIN: f32 = 0.08;
const BEAD_MAX: f32 = 0.2;
const BEAD_SCALE: (f32, f32) = (0.45, 1.6);
/// Beads ride their stream within ± this share of the screen width (past
/// 0.5 they sit across the frame edge).
const BEAD_REACH: f32 = 0.62;
/// Beads sit this far (in stream widths) off their stream's centre line.
const BEAD_OFFSET: f32 = 0.45;

// The field.

/// Iso level of the surface at 0 % and 100 % merge: lower fuses forms
/// from further apart (sizes are normalized so they do not change).
const ISO_APART: f32 = 0.7;
const ISO_MERGED: f32 = 0.28;
/// Finite-difference step for the field gradient (screen heights).
const EPS: f32 = 0.0008;
/// Organic warp of the whole field: frequency (per screen height) and
/// displacement (screen heights).
const WARP_FREQ: f32 = 1.1;
const WARP: f32 = 0.035;
/// Offset decorrelating the warp's second field from its first.
const WARP_Y_OFFSET: (f32, f32) = (17.3, -9.1);
/// Per-stream wave frequency scale range.
const WAVE_FREQ_SCALE: (f32, f32) = (0.7, 1.3);

// Surface.

/// Surface height (screen heights) at 0 % and 100 % depth.
const HEIGHT_MIN: f32 = 0.04;
const HEIGHT_MAX: f32 = 0.16;
/// Steepest slope a surface normal may take (the rim is near vertical).
const MAX_SLOPE: f32 = 8.0;
/// The key light is tilted this far toward the light angle (the rest is
/// straight at the screen).
const KEY_TILT: f32 = 0.55;
/// Diffuse wrap and weight.
const WRAP: f32 = 0.35;
const DIFFUSE: f32 = 0.16;
/// Diffuse light at which a surface is neither lifted nor dimmed.
const DIFFUSE_LEVEL: f32 = 0.75;
/// Fresnel rim color (lift and chroma kept), the fill reflection's lift,
/// and the highlights' near white (lightness, chroma kept).
const FRESNEL_TINT: (f32, f32) = (0.2, 0.8);
const FILL_LIFT: f32 = 0.2;
const HIGHLIGHT_WHITE: (f32, f32) = (0.99, 0.08);
/// Streak highlight: the normal·half-vector cosines where it starts and
/// is full, at 0 % and 100 % gloss (glossier is crisper), and strength.
const STREAK_FROM: (f32, f32) = (0.955, 0.982);
const STREAK_TO: (f32, f32) = (0.985, 0.996);
const STREAK: f32 = 0.85;
/// A tight glint at the brightest point of a streak: exponent and strength.
const GLINT_POWER: f32 = 2000.0;
const GLINT: f32 = 0.6;
/// Broad sheen: exponent at 0 % and 100 % gloss, and strength.
const SHEEN_POWER: (f32, f32) = (3.0, 10.0);
const SHEEN: f32 = 0.22;
/// A dimmer fill reflection from the opposite side, colored by the ground.
const FILL_IN: f32 = 0.86;
const FILL_OUT: f32 = 0.6;
const FILL: f32 = 0.18;
/// Fresnel rim: exponent and strength.
const FRESNEL_POWER: f32 = 3.0;
const FRESNEL: f32 = 0.35;
/// Inner depth at 100 %: lightness lost and chroma gained by thick liquid,
/// and how clear the thinnest edge is (share of the ground showing).
const DEPTH_DARK: f32 = 0.16;
const DEPTH_RICH: f32 = 0.35;
const EDGE_CLEAR: f32 = 0.25;
/// Color drifts along a stream at this frequency (radians per screen height).
const TONE_FREQ: (f32, f32) = (1.2, 2.6);

// Ground.

/// How far up the ramp the ground gradient climbs (dark, light).
const GROUND_SPAN: f32 = 0.4;
const LIGHT_GROUND_SPAN: f32 = 0.22;
/// Ground glow: its ramp tone (dark, light), strength and radius
/// (screen heights).
const GLOW_TONE: f32 = 0.5;
const GLOW_TONE_LIGHT: f32 = 0.35;
const GROUND_GLOW: f32 = 0.4;
const GLOW_RADIUS: f32 = 0.55;
/// The glow sits within ± these shares of the width and height.
const GLOW_REACH: (f32, f32) = (0.4, 0.3);
/// Shadow: offset along the light (screen heights), the share of the iso
/// level where it starts, and its depth at 100 %.
const SHADOW_OFFSET: f32 = 0.04;
const SHADOW_FROM: f32 = 0.25;
const SHADOW_DEPTH: f32 = 0.45;
/// Contact occlusion just outside the edge: where it starts (share of
/// the iso level) and its depth.
const CONTACT_FROM: f32 = 0.45;
const CONTACT: f32 = 0.2;
/// Light mode keeps this share of shadows and rim light.
const LIGHT_SOFTEN: f32 = 0.5;

// Motion.

/// Stream phases sway by this much (radians) around a loop, beads orbit
/// this far (screen heights) and the warp drifts this far.
const SWAY: f32 = 0.6;
const ORBIT: f32 = 0.05;
const WARP_DRIFT: f32 = 0.25;
/// Hue turn (radians) along a stream's color drift, the lightness it
/// gains, and the chroma it keeps.
const HUE_TURN: f32 = 0.44;
const TONE_SHIFT: f32 = 0.08;
const TONE_CHROMA: f32 = 1.1;
/// OKLCh hues (radians) of the yellows whose turn toward green would go
/// olive.
const OLIVE_FROM: f32 = 0.7;
const OLIVE_TO: f32 = 2.6;
/// Golden-ratio step between consecutive tones.
const GOLDEN: f32 = 0.618_034;
/// Tone of the first form along the golden-ratio sequence.
const TONE_START: f32 = 0.2;

struct Stream {
    cos_a: f32,
    sin_a: f32,
    offset: f32,
    waves: [(f32, f32, f32); 2],
    width: f32,
    bulge: (f32, f32),
    tone: (f32, f32),
    colors: (Lab, Lab),
}

impl Stream {
    /// Field and tone (0–1 along the stream) at `(x, y)`.
    #[inline]
    fn at(&self, x: f32, y: f32) -> (f32, f32) {
        let u = x * self.cos_a + y * self.sin_a;
        let v = -x * self.sin_a + y * self.cos_a - self.offset;
        let (mut c, mut dc) = (0.0, 0.0);
        for (a, f, p) in self.waves {
            c += a * sin(f * u + p);
            dc += a * f * cos(f * u + p);
        }
        // Distance across the (sloping) centre line.
        let d2 = (v - c) * (v - c) / (1.0 + dc * dc);
        let w = self.width * (1.0 + BULGE * sin(self.bulge.0 * u + self.bulge.1));
        (exp(-d2 / (w * w)), 0.5 + 0.5 * sin(self.tone.0 * u + self.tone.1))
    }

    /// The point on the centre line at `u`, in screen coordinates.
    fn point(&self, u: f32) -> (f32, f32) {
        let c: f32 = self.waves.iter().map(|&(a, f, p)| a * sin(f * u + p)).sum::<f32>() + self.offset;
        (u * self.cos_a - c * self.sin_a, u * self.sin_a + c * self.cos_a)
    }
}

struct Bead {
    x: f32,
    y: f32,
    inv_r2: f32,
    color: Lab,
}

impl Bead {
    #[inline]
    fn at(&self, x: f32, y: f32) -> f32 {
        exp(-((x - self.x) * (x - self.x) + (y - self.y) * (y - self.y)) * self.inv_r2)
    }
}

/// One depth layer: a stream and the beads that fuse with it.
struct Layer {
    stream: Stream,
    beads: Vec<Bead>,
}

impl Layer {
    /// The layer's field at a warped position.
    #[inline]
    fn field(&self, x: f32, y: f32) -> f32 {
        self.stream.at(x, y).0 + self.beads.iter().map(|b| b.at(x, y)).sum::<f32>()
    }

    /// Liquid color: each form's color weighted by its share of the field.
    fn color(&self, x: f32, y: f32) -> Lab {
        let (f, tone) = self.stream.at(x, y);
        let c = self.stream.colors.0.mix_vivid(self.stream.colors.1, tone);
        let (mut sum, mut w, mut chroma) = (c.scale(f), f, c.chroma() * f);
        for b in &self.beads {
            let f = b.at(x, y);
            sum = sum.add(b.color.scale(f));
            chroma += b.color.chroma() * f;
            w += f;
        }
        let w = w.max(1e-6);
        sum.scale(1.0 / w).with_chroma(chroma / w)
    }
}

pub struct Liquid {
    /// Back to front.
    layers: Vec<Layer>,
    iso: f32,
    height: f32,
    gloss: f32,
    depth: f32,
    shadow: f32,
    light: (f32, f32),
    key: (f32, f32, f32),
    /// Blinn half vector between the key light and the view.
    half: (f32, f32, f32),
    ground_a: Lab,
    ground_b: Lab,
    glow_at: (f32, f32),
    glow_color: Lab,
    warp_seed: u32,
    warp_off: (f32, f32),
    edge: f32,
    light_mode: bool,
}

impl Liquid {
    pub fn new(ctx: &Ctx) -> Self {
        let s = &ctx.scheme;
        let mut rng = ctx.rng(81);
        let n = ctx.n("blobs");
        let size = ctx.u("size");
        let iso = lerp(ISO_APART, ISO_MERGED, ctx.u("merge"));
        // A Gaussian of width w crosses the iso level at w·√ln(1/iso):
        // normalize so the visible size does not depend on the merge.
        let norm = 1.0 / (1.0 / iso).ln().sqrt();
        let n_streams = (n / BLOBS_PER_STREAM).clamp(1, MAX_STREAMS);
        let family = rng.range(-STREAM_ANGLE, STREAM_ANGLE);
        let sway = SWAY * ctx.motion() * sin(ctx.angle(1.0, 0.0));
        let tone_of = |i: usize| fract(TONE_START + i as f32 * GOLDEN);
        let mut layers: Vec<Layer> = (0..n_streams)
            .map(|i| {
                let k = if n_streams > 1 { i as f32 / (n_streams - 1) as f32 * 2.0 - 1.0 } else { 0.0 };
                let a = family + rng.range(-STREAM_SPREAD, STREAM_SPREAD);
                let wave = |rng: &mut Rng, (amp, freq): (f32, f32), dir: f32| (amp * rng.range(0.5, 1.0), freq * rng.range(WAVE_FREQ_SCALE.0, WAVE_FREQ_SCALE.1), rng.range(0.0, std::f32::consts::TAU) + dir * sway);
                let stream = Stream {
                    cos_a: cos(a),
                    sin_a: sin(a),
                    offset: k * STREAM_BAND + rng.range(-STREAM_JITTER, STREAM_JITTER),
                    waves: [wave(&mut rng, WAVE_LONG, 1.0), wave(&mut rng, WAVE_SHORT, -1.0)],
                    width: lerp(WIDTH_MIN, WIDTH_MAX, size) * rng.range(WIDTH_SCALE.0, WIDTH_SCALE.1) * norm,
                    bulge: (rng.range(BULGE_FREQ.0, BULGE_FREQ.1), rng.range(0.0, std::f32::consts::TAU) - sway),
                    tone: (rng.range(TONE_FREQ.0, TONE_FREQ.1), rng.range(0.0, std::f32::consts::TAU)),
                    colors: {
                        // Along the flow the color turns its hue (the Mandara accent
                        // gradient) and lightness, never crossing a muddy midpoint.
                        let base = s.accent(i);
                        let h = base.hue();
                        // Yellows always turn toward orange (toward green they go olive).
                        let yellow = (OLIVE_FROM..OLIVE_TO).contains(&h);
                        let turn = if yellow || i % 2 == 1 { -HUE_TURN } else { HUE_TURN };
                        (base, Lab::from_lch(base.l + TONE_SHIFT, base.chroma() * TONE_CHROMA, h + turn))
                    },
                };
                Layer { stream, beads: Vec::new() }
            })
            .collect();
        for j in 0..n.saturating_sub(n_streams).max(1) {
            let layer = &mut layers[j % n_streams];
            let st = &layer.stream;
            let (px, py) = st.point(rng.range(-BEAD_REACH, BEAD_REACH) * ctx.aspect);
            let side = rng.range(-BEAD_OFFSET, BEAD_OFFSET) * st.width;
            let (ox, oy) = ctx.orbit(1.0, rng.f(), ORBIT);
            let r = lerp(BEAD_MIN, BEAD_MAX, size) * rng.range(BEAD_SCALE.0, BEAD_SCALE.1) * norm;
            let color = st.colors.0.mix_vivid(st.colors.1, tone_of(j + 3));
            layer.beads.push(Bead { x: px - side * st.sin_a + ox, y: py + side * st.cos_a + oy, inv_r2: 1.0 / (r * r), color });
        }
        let light = ctx.light_dir();
        let kz = (1.0 - KEY_TILT * KEY_TILT).sqrt();
        let (ground_a, ground_b) = if s.light { (s.ramp(0.0), s.ramp(LIGHT_GROUND_SPAN)) } else { (s.deep(), s.ramp(GROUND_SPAN)) };
        Liquid {
            layers,
            iso,
            height: lerp(HEIGHT_MIN, HEIGHT_MAX, ctx.u("depth")),
            gloss: ctx.u("gloss"),
            depth: ctx.u("depth"),
            shadow: ctx.u("shadow") * if s.light { LIGHT_SOFTEN } else { 1.0 },
            light,
            key: (light.0 * KEY_TILT, light.1 * KEY_TILT, kz),
            half: {
                let (hx, hy, hz) = (light.0 * KEY_TILT, light.1 * KEY_TILT, kz + 1.0);
                let l = (hx * hx + hy * hy + hz * hz).sqrt();
                (hx / l, hy / l, hz / l)
            },
            ground_a,
            ground_b,
            glow_at: (rng.range(-GLOW_REACH.0, GLOW_REACH.0) * ctx.aspect, rng.range(-GLOW_REACH.1, GLOW_REACH.1)),
            glow_color: if s.light { s.ramp(GLOW_TONE_LIGHT) } else { s.ramp(GLOW_TONE) },
            warp_seed: ctx.seed32(82),
            warp_off: ctx.orbit(1.0, 0.0, WARP_DRIFT),
            edge: ctx.edge_width(1.0),
            light_mode: s.light,
        }
    }

    /// The warped position of `(x, y)` (slow organic flow).
    #[inline]
    fn warp(&self, x: f32, y: f32) -> (f32, f32) {
        let (wx, wy) = ((x + self.warp_off.0) * WARP_FREQ, (y + self.warp_off.1) * WARP_FREQ);
        (x + WARP * noise2(wx, wy, self.warp_seed), y + WARP * noise2(wx + WARP_Y_OFFSET.0, wy + WARP_Y_OFFSET.1, self.warp_seed))
    }

    fn ground(&self, x: f32, y: f32) -> Lab {
        let g = (0.5 - 0.5 * (x * self.light.0 + y * self.light.1)).clamp(0.0, 1.0);
        let c = self.ground_a.mix_vivid(self.ground_b, smoothstep(0.0, 1.0, g));
        let (gx, gy) = (x - self.glow_at.0, y - self.glow_at.1);
        let glow = GROUND_GLOW * exp(-(gx * gx + gy * gy) / (GLOW_RADIUS * GLOW_RADIUS));
        c.mix_vivid(self.glow_color, glow)
    }
}

impl Liquid {
    /// `layer`'s liquid over `under` at warped `(x, y)`: its shadow and
    /// contact occlusion on what lies below, then its glossy surface.
    fn composite(&self, layer: &Layer, x: f32, y: f32, mut under: Lab) -> Lab {
        // Shadow falling away from the light, and contact occlusion.
        let fs = layer.field(x + self.light.0 * SHADOW_OFFSET, y + self.light.1 * SHADOW_OFFSET);
        under.l *= 1.0 - self.shadow * SHADOW_DEPTH * smoothstep(self.iso * SHADOW_FROM, self.iso, fs);
        let f = layer.field(x, y);
        under.l *= 1.0 - self.shadow * CONTACT * smoothstep(self.iso * CONTACT_FROM, self.iso, f);
        if f < self.iso * CONTACT_FROM {
            return under;
        }
        let gx = (layer.field(x + EPS, y) - f) / EPS;
        let gy = (layer.field(x, y + EPS) - f) / EPS;
        let gl = (gx * gx + gy * gy).sqrt().max(1e-4);
        // Distance to the surface edge (the field's excess over its slope).
        let d = (f - self.iso) / gl;
        let cov = smoothstep(-self.edge, self.edge, d);
        if cov <= 0.0 {
            return under;
        }
        // Height h = H·√(excess): a Gaussian tube's excess falls as
        // 1 − d²/w², so its cross-section is a true ellipse, round across
        // the whole body and vertical at the rim.
        let excess = ((f - self.iso) / (1.0 - self.iso)).max(1e-5);
        let thick = excess.sqrt().min(1.0);
        let dh = (self.height * 0.5 / (excess.sqrt() * (1.0 - self.iso))).min(MAX_SLOPE / gl);
        let (nx, ny, nz) = (-gx * dh, -gy * dh, 1.0);
        let nl = (nx * nx + ny * ny + nz * nz).sqrt();
        let (nx, ny, nz) = (nx / nl, ny / nl, nz / nl);
        let (kx, ky, kz) = self.key;
        // Reflection of the view (straight at the screen) about the normal.
        let (rx, ry, rz) = (2.0 * nz * nx, 2.0 * nz * ny, 2.0 * nz * nz - 1.0);
        let r_key = rx * kx + ry * ky + rz * kz;
        let r_fill = -rx * kx - ry * ky + rz * kz;
        let mut b = layer.color(x, y);
        // Inner depth: thick liquid is deeper and richer.
        let deep = self.depth * thick;
        b = b.lighten(-DEPTH_DARK * deep).saturate(1.0 + DEPTH_RICH * deep);
        let diffuse = ((nx * kx + ny * ky + nz * kz + WRAP) / (1.0 + WRAP)).max(0.0);
        b.l += DIFFUSE * (diffuse - DIFFUSE_LEVEL);
        // Thin edges are clearer: a little of what lies below shows through.
        b = b.mix(under, EDGE_CLEAR * (1.0 - thick) * (1.0 - thick));
        let soften = if self.light_mode { LIGHT_SOFTEN } else { 1.0 };
        let fresnel = pow(1.0 - nz.max(0.0), FRESNEL_POWER);
        b = super::light(b, b.lighten(FRESNEL_TINT.0).saturate(FRESNEL_TINT.1), FRESNEL * soften * fresnel);
        let fill = smoothstep(FILL_OUT, FILL_IN, r_fill);
        b = super::light(b, self.ground_b.mix(self.glow_color, 0.5).lighten(FILL_LIFT), FILL * fill);
        let white = Lab::new(HIGHLIGHT_WHITE.0, b.a * HIGHLIGHT_WHITE.1, b.b * HIGHLIGHT_WHITE.1);
        let sheen = pow(r_key.max(0.0), lerp(SHEEN_POWER.0, SHEEN_POWER.1, self.gloss));
        // The streak: a crisp-edged highlight where the normal meets the half
        // vector; on a tube that is a line along its lit flank.
        let (hx, hy, hz) = self.half;
        let nh = nx * hx + ny * hy + nz * hz;
        let (from, to) = (lerp(STREAK_FROM.0, STREAK_FROM.1, self.gloss), lerp(STREAK_TO.0, STREAK_TO.1, self.gloss));
        let streak = smoothstep(from, to, nh);
        let glint = pow(nh.max(0.0), GLINT_POWER);
        b = super::light(b, white, self.gloss * (STREAK * streak + SHEEN * sheen + GLINT * glint));
        under.mix(b, cov)
    }
}

impl Scene for Liquid {
    fn shade(&self, x: f32, y: f32) -> Lab {
        let (wx, wy) = self.warp(x, y);
        let mut c = self.ground(x, y);
        for layer in &self.layers {
            c = self.composite(layer, wx, wy, c);
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::Scheme;
    use crate::recipe::Recipe;
    use crate::style::Style;

    /// Border samples per frame edge.
    const BORDER: usize = 96;

    #[test]
    fn liquid_is_a_field_running_off_the_frame() {
        let aspect = 16.0 / 9.0;
        for seed in [1u64, 7, 42, 1234, 5, 99] {
            for blobs in [2.0, 7.0, 16.0] {
                for size in [0.0, 50.0, 100.0] {
                    let mut r = Recipe::new(Style::Liquid, seed);
                    r.set_param("blobs", blobs);
                    r.set_param("size", size);
                    let ctx = Ctx { recipe: &r, scheme: Scheme::new(&r.palette, false), aspect, px: 1.0 / 180.0, t: 0.0, preview: false };
                    let liquid = Liquid::new(&ctx);
                    let inside = |x: f32, y: f32| {
                        let (wx, wy) = liquid.warp(x, y);
                        liquid.layers.iter().any(|l| l.field(wx, wy) > liquid.iso)
                    };
                    let along = |i: usize| i as f32 / (BORDER - 1) as f32;
                    let (hw, hh) = (aspect / 2.0, 0.5);
                    let edges = [
                        (0..BORDER).any(|i| inside(-hw, -hh + along(i))),
                        (0..BORDER).any(|i| inside(hw, -hh + along(i))),
                        (0..BORDER).any(|i| inside(-hw + aspect * along(i), -hh)),
                        (0..BORDER).any(|i| inside(-hw + aspect * along(i), hh)),
                    ];
                    let touched = edges.iter().filter(|e| **e).count();
                    let tag = format!("seed {seed} blobs {blobs} size {size}");
                    assert!(touched >= 2, "{tag}: liquid touches only {touched} frame edge(s)");
                    if size != 50.0 {
                        continue;
                    }
                    // At the default size, a field: neither a speck nor a flood.
                    let grid = 40;
                    let cover = (0..grid * grid).filter(|k| inside(-hw + aspect * along(k % grid * BORDER / grid), -hh + along(k / grid * BORDER / grid))).count() as f32 / (grid * grid) as f32;
                    assert!((0.15..0.9).contains(&cover), "{tag}: liquid covers {cover:.2} of the frame");
                }
            }
        }
    }
}
