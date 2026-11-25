//! Dunes: a sand sea under a low side sun, in the manner of desert
//! photography. The terrain is an analytic heightfield of barchanoid
//! ridges: each crest has the true dune profile (a long windward slope
//! rising to a knife-sharp brink, then a steep slip face), its height swells
//! and dies away along its length, and its tall middle lags upwind of its
//! low ends, so crests bow into crescents with horns pointing downwind.
//!
//! Each pixel marches its view ray only between the planes of the highest
//! and lowest sand (a bounded, mostly geometric march), bisects the hit,
//! and shades it analytically: the sun's diffuse light with cast shadows
//! computed in profile space (a crest's shadow line falls across the slip
//! face and the trough behind it), cool sky fill in the shadows, wind
//! ripples combed into the lit windward faces, and aerial haze that melts
//! distant dunes into the glowing horizon. Crest silhouettes are
//! antialiased by cone tracing.

use crate::color::Lab;
use crate::fmath::{cos, exp, fract, lerp, pow, sin, smoothstep};
use crate::noise::noise2;
use crate::render::{Ctx, Scene};

// Camera and terrain.

/// Camera height above the lowest sand (world units). Always above the
/// tallest dune, so a ray only meets sand between two planes.
const EYE: f32 = 1.0;
/// Dune spacing (world units) at 0 % and 100 % dune size.
const SPACING_MIN: f32 = 1.5;
const SPACING_MAX: f32 = 4.4;
/// Tallest dune height as a share of the spacing (kept under `EYE`).
const HEIGHT_SHARE: f32 = 0.11;
/// Windward share of a period at 0 % and 100 % crest sharpness.
const WINDWARD_MIN: f32 = 0.6;
const WINDWARD_MAX: f32 = 0.8;
/// Sharpness at which the brink turns from rounded to a knife edge.
const BRINK_FROM: f32 = 0.15;
const BRINK_TO: f32 = 0.75;
/// Crest meander: frequency (per spacing) and strength (periods).
const MEANDER_FREQ: f32 = 0.16;
const MEANDER: f32 = 1.1;
/// Frequency ratio of the meander's second octave.
const MEANDER_LACUNARITY: f32 = 2.3;
/// Height swell: frequencies along and across the crests (per spacing).
const SWELL_ALONG: f32 = 0.3;
const SWELL_ACROSS: f32 = 0.21;
/// Swell remap: noise below `SWELL_LO` is a flat corridor, above `SWELL_HI`
/// a full dune; the lowest dunes keep `SWELL_FLOOR` of the height.
const SWELL_LO: f32 = -0.55;
const SWELL_HI: f32 = 0.6;
const SWELL_FLOOR: f32 = 0.18;
/// How far (periods) a dune's tall middle lags upwind of its low horns.
const BOW: f32 = 0.45;
/// Wind direction off the view axis (radians), away from the sun's side,
/// and how much a seed may turn it.
const WIND_TURN: f32 = 1.0;
const WIND_JITTER: f32 = 0.2;
/// Composition: candidate terrain shifts tried per render, the range they
/// span (spacings), and the middle-distance points (spacings across and
/// ahead) that should stand on tall dunes, so the foreground frames a
/// field of crests rather than one face filling the view.
const LAYOUT_TRIES: usize = 12;
const LAYOUT_RANGE: f32 = 40.0;
const MIDGROUND: [(f32, f32); 6] = [(-0.9, 1.5), (0.0, 1.4), (0.9, 1.5), (-1.4, 2.4), (0.0, 2.3), (1.4, 2.4)];
/// Mean height of the dune profile (what a dune far away flattens to).
const PROFILE_MEAN: f32 = 0.45;
/// Slip-face shape: one `SLIP_TOE`-th of it follows q^1.5 (a concave toe),
/// the rest is a straight avalanche face (higher is straighter).
const SLIP_TOE: f32 = 1.5;

// Level of detail.

/// Dune relief fades between these ground-footprint shares of a period.
const LOD_FROM: f32 = 0.04;
const LOD_TO: f32 = 0.2;
/// Lit share a fully flattened dune field averages to (so a far dune
/// looks like its mean, not like a flat lit plane).
const FAR_TONE: f32 = 0.55;

// Raymarch.

/// Lipschitz bound of the heightfield's slope (step safety).
const SLOPE_BOUND: f32 = 1.1;
/// Smallest step: a share of the distance travelled while the ray is
/// well clear of the sand…
const STEP_SHARE: f32 = 0.006;
/// …and this many pixels once it passes within `NEAR_PIXELS` of it, so
/// no crest tip is stepped over.
const STEP_PIXELS: f32 = 1.0;
const NEAR_PIXELS: f32 = 6.0;
/// Step cap (a ray that runs out lands at the far plane).
const MAX_STEPS: u32 = 120;
/// Bisection steps refining a hit.
const REFINE: u32 = 6;
/// A ray passing within this many pixels of a crest (or landing this
/// close to a brink) is on an edge and is supersampled…
const EDGE_PIXELS: f32 = 1.0;
/// …with these extra rays (rotated grid, in pixels).
const EDGE_TAPS: [(f32, f32); 4] = [(0.125, 0.375), (-0.375, 0.125), (-0.125, -0.375), (0.375, -0.125)];
/// A crest is passed once the clearance climbs this many pixels above its
/// dip.
const EDGE_RISE: f32 = 1.0;
/// Edges count only where a pixel's ground footprint is under this share
/// of a period (farther, the relief has faded and there are none).
const EDGE_RELIEF: f32 = 0.08;

// Light.

/// Sun azimuth off the view axis (radians) at 0 % sun position, and how
/// much further it swings at ±100 %.
const LIGHT_AZIMUTH: f32 = 1.15;
const LIGHT_SWING: f32 = 0.5;
/// Sun elevation (radians) at 0 % and 100 % sun height.
const ELEV_MIN: f32 = 0.12;
const ELEV_MAX: f32 = 0.5;
/// Diffuse light at which a face is fully lit (dark, light mode: bright
/// air keeps more gradation across the lit faces).
const LIT_FULL: f32 = 0.5;
const LIGHT_LIT_FULL: f32 = 0.55;
/// Shadow penumbra: share of the shadow's length.
const PENUMBRA: f32 = 0.06;
/// Sky fill: lightness a face gains per unit of upward normal.
const SKY_FILL: f32 = 0.06;
/// Upward normal at which the sky fill is neutral.
const SKY_FILL_LEVEL: f32 = 0.9;
/// Share of the fill tone that open sky lends a level shadowed face.
const SHADOW_FILL: f32 = 0.4;
/// Warm bounce light in the shadows near lit sand.
const BOUNCE: f32 = 0.05;
/// Brink highlight: width (phase units) and lift of the lit crest edge.
const BRINK_WIDTH: f32 = 0.012;
const BRINK_LIGHT: f32 = 0.06;
/// The brink highlight is never narrower than this many pixels (it dims
/// as it widens, so it never aliases into dots).
const BRINK_PIXELS: f32 = 2.0;

// Ripples.

/// Ripples per world unit, and the normal tilt they comb in at 100 %.
const RIPPLE_FREQ: f32 = 42.0;
const RIPPLE_TILT: f32 = 0.22;
/// Ripple crest wander: frequency (per world unit) and depth (ripples).
const RIPPLE_WARP_FREQ: f32 = 1.3;
const RIPPLE_WARP: f32 = 2.2;
/// Ripple asymmetry (second harmonic share: steep lee, gentle stoss).
const RIPPLE_SKEW: f32 = 0.35;
/// Ripple depth varies in patches: frequency (per world unit) and how
/// much of the depth the faintest patches lose.
const RIPPLE_PATCH_FREQ: f32 = 0.8;
const RIPPLE_PATCHES: f32 = 0.7;
/// Offset decorrelating the patch field from the ripple warp (same seed).
const RIPPLE_PATCH_OFFSET: f32 = 5.3;
/// Ripples cover the windward face between these shares of it (fade in
/// from the trough, fade out below the brink).
const RIPPLE_FACE: (f32, f32, f32, f32) = (0.04, 0.3, 0.9, 0.99);
/// Ripples fade out between these footprints (in ripple wavelengths).
const RIPPLE_LOD_FROM: f32 = 0.12;
const RIPPLE_LOD_TO: f32 = 0.45;
/// Ripples drift this many wavelengths per loop.
const RIPPLE_CYCLES: f32 = 1.0;

// Sky and air.

/// Sun disc radius and glow falloffs (screen heights).
const SUN_R: f32 = 0.028;
const GLOW_CORE: f32 = 0.05;
const GLOW_BLOOM: f32 = 0.22;
/// The disc is lifted this much above the sun color (dark mode), or
/// mixed this far toward white (light mode).
const DISC_LIFT: f32 = 0.05;
const LIGHT_DISC_WHITE: f32 = 0.75;
/// Light mode adds the disc as this much light.
const LIGHT_DISC_GAIN: f32 = 0.6;
/// How much the haze veils the sun disc.
const SUN_VEIL: f32 = 0.35;
/// The sun's screen position: share of the half-width at ±100 %, and the
/// share of the sky height it climbs at 100 % sun height.
const SUN_REACH: f32 = 0.42;
const SUN_CLIMB: f32 = 0.9;
/// Light the sun glow adds at 100 % glow, and the horizon band under it.
const GLOW_GAIN: f32 = 0.65;
const BAND_GAIN: f32 = 0.2;
/// Share of the sun's bloom that reaches the top of the sky.
const ZENITH_GLOW: f32 = 0.25;
/// Horizon band height (screen heights).
const HORIZON_BAND: f32 = 0.07;
/// Sky gradient: where (share of the sky height) the zenith tone has
/// turned to the mid tone, and the curve of the fade into the horizon
/// (higher keeps the horizon glow lower).
const SKY_MID_AT: f32 = 0.7;
const SKY_CURVE: f32 = 2.4;
/// Haze: the distance (world units) over which, at 100 %, the clear share
/// of the view falls by a factor of e.
const HAZE_DIST: f32 = 12.0;
/// How much of the palette tints the warm horizon and the sunlit sand
/// (the rest is the sun's own color: warm light, cool palette shadows).
const SKY_LOW_TINT: f32 = 0.15;
const LIT_TINT: f32 = 0.12;
/// Dusk sand is lit this much below the sun's own lightness.
const DUSK_DIM: f32 = -0.2;
/// Dusk light is richer than the sun's pale disc.
const DUSK_CHROMA: f32 = 1.15;
/// Light mode: the sun's share of the pale horizon, and the ground's share
/// of the sunlit sand.
const LIGHT_SKY_SUN: f32 = 0.3;
const LIGHT_LIT_TINT: f32 = 0.45;
/// Light-mode sunlit sand sits this far below the pale sun (so the faces
/// keep their modelling against the bright air).
const LIGHT_LIT_DIM: f32 = -0.03;
/// Light mode keeps this share of the haze (bright air is clearer).
const LIGHT_HAZE: f32 = 0.6;
/// Ramp tones at dusk (dark mode): zenith, mid sky, the horizon's tint,
/// the lit sand's tint, shadowed sand and its sky fill…
const DUSK_ZENITH: f32 = 0.08;
const DUSK_MID: f32 = 0.34;
const DUSK_HORIZON: f32 = 0.62;
const DUSK_LIT: f32 = 0.9;
const DUSK_SHADE: f32 = 0.12;
const DUSK_FILL: f32 = 0.3;
/// …and in light mode: zenith, mid sky and the lit sand's ground; shadows
/// take the palette's coolest stop at this lightness and least chroma,
/// and their sky fill is this much lighter.
const LIGHT_ZENITH: f32 = 0.42;
const LIGHT_MID: f32 = 0.2;
const LIGHT_LIT: f32 = 0.08;
const LIGHT_SHADE_L: f32 = 0.72;
const LIGHT_SHADE_C: f32 = 0.05;
const LIGHT_FILL_LIFT: f32 = 0.08;
/// The haze breathes a quarter loop out of step with the glow.
const HAZE_PHASE: f32 = 0.25;
/// Haze strength at 0 % (a little air always remains).
const HAZE_FLOOR: f32 = 0.25;
/// How much the glow and haze swell around a loop (heat over the sand).
const BREATH: f32 = 0.1;

/// The sand at one ground point: height and the profile state shading
/// needs.
struct Sand {
    h: f32,
    /// Position within the period (0 = trough, `windward` = crest).
    p: f32,
    /// Local crest height (world units, after level of detail).
    crest: f32,
    /// Level of detail kept (1 = full relief, 0 = flattened).
    detail: f32,
}

pub struct Dunes {
    horizon: f32,
    sky_top: Lab,
    sky_mid: Lab,
    sky_low: Lab,
    sun: Lab,
    /// The sun disc (near white in light mode, where the sky is pale).
    disc: Lab,
    sun_at: (f32, f32),
    sun_dir: (f32, f32, f32),
    glow: f32,
    lit: Lab,
    shade_c: Lab,
    fill: Lab,
    spacing: f32,
    height: f32,
    windward: f32,
    brink: f32,
    /// Wind frame: unit vector the wind blows toward (x, z).
    wind: (f32, f32),
    /// Tangent of the sun's elevation in the wind's cross-section, and
    /// whether it shines from upwind.
    shadow_tan: f32,
    from_upwind: bool,
    seeds: [u32; 4],
    /// Shift of the terrain fields (spacings), chosen so tall dunes stand
    /// in the middle distance.
    offset: (f32, f32),
    ripples: f32,
    haze: f32,
    ripple_phase: f32,
    px: f32,
    edge: f32,
    light_mode: bool,
    preview: bool,
    lit_full: f32,
}

impl Dunes {
    pub fn new(ctx: &Ctx) -> Self {
        let s = &ctx.scheme;
        let horizon = ctx.u("horizon") - 0.5;
        let sun = s.sun();
        let sx = ctx.u("sun_x");
        let sun_at = (sx * ctx.aspect * SUN_REACH, horizon - ctx.u("sun_height") * (horizon + 0.5) * SUN_CLIMB);
        let side = if sx >= 0.0 { 1.0 } else { -1.0 };
        // Raking side light: the sun stands well off to the side it glows on.
        let az = side * LIGHT_AZIMUTH + sx * LIGHT_SWING;
        let elev = lerp(ELEV_MIN, ELEV_MAX, ctx.u("sun_height"));
        let sun_dir = (sin(az) * cos(elev), sin(elev), cos(az) * cos(elev));
        // The wind blows away from the viewer and from the sun's side, so the
        // long windward faces turn toward the light.
        let mut rng = ctx.rng(35);
        let wa = -side * (WIND_TURN + rng.range(-WIND_JITTER, WIND_JITTER));
        let wind = (sin(wa), cos(wa));
        let along = sun_dir.0 * wind.0 + sun_dir.2 * wind.1;
        let shadow_tan = sun_dir.1 / along.abs().max(1e-3);
        let (sky_top, sky_mid, sky_low, lit, shade_c, fill) = if s.light {
            // A pale afternoon desert: bright air, warm sand, shadows in the
            // palette's coolest pastel.
            let cool = super::coolest(s);
            let shade = Lab::from_lch(LIGHT_SHADE_L, cool.chroma().max(LIGHT_SHADE_C), cool.hue());
            let sky_low = s.ramp(0.0).mix_vivid(sun, LIGHT_SKY_SUN);
            (s.ramp(LIGHT_ZENITH), s.ramp(LIGHT_MID), sky_low, sun.mix_vivid(s.ramp(LIGHT_LIT), LIGHT_LIT_TINT).lighten(LIGHT_LIT_DIM), shade, shade.lighten(LIGHT_FILL_LIFT))
        } else {
            // Dusk: glowing horizon, gold lit faces, deep cool shadows.
            let sky_low = sun.mix_vivid(s.ramp(DUSK_HORIZON), SKY_LOW_TINT).lighten(DUSK_DIM * 0.5);
            (s.ramp(DUSK_ZENITH), s.ramp(DUSK_MID), sky_low, sun.mix_vivid(s.ramp(DUSK_LIT), LIT_TINT).lighten(DUSK_DIM).saturate(DUSK_CHROMA), s.ramp(DUSK_SHADE), s.ramp(DUSK_FILL))
        };
        let breath = |phase| 1.0 + BREATH * ctx.motion() * cos(ctx.angle(1.0, phase));
        let spacing = lerp(SPACING_MIN, SPACING_MAX, ctx.u("scale"));
        let mut dunes = Dunes {
            horizon,
            sky_top,
            sky_mid,
            sky_low,
            sun,
            disc: if s.light { sun.mix(Lab::new(1.0, 0.0, 0.0), LIGHT_DISC_WHITE) } else { sun.lighten(DISC_LIFT) },
            sun_at,
            sun_dir,
            glow: ctx.u("glow") * breath(0.0),
            lit,
            shade_c,
            fill,
            spacing,
            height: spacing * HEIGHT_SHARE,
            windward: lerp(WINDWARD_MIN, WINDWARD_MAX, ctx.u("sharpness")),
            brink: smoothstep(BRINK_FROM, BRINK_TO, ctx.u("sharpness")),
            wind,
            shadow_tan,
            from_upwind: along < 0.0,
            seeds: [ctx.seed32(31), ctx.seed32(32), ctx.seed32(33), ctx.seed32(34)],
            offset: (0.0, 0.0),
            ripples: ctx.u("ripples"),
            haze: lerp(HAZE_FLOOR, 1.0, ctx.u("haze")) * breath(HAZE_PHASE) * if s.light { LIGHT_HAZE } else { 1.0 },
            ripple_phase: RIPPLE_CYCLES * ctx.t,
            px: ctx.px,
            edge: ctx.edge_width(1.0),
            light_mode: s.light,
            preview: ctx.preview,
            lit_full: if s.light { LIGHT_LIT_FULL } else { LIT_FULL },
        };
        dunes.choose_layout(&mut rng);
        dunes
    }

    /// Tries a few shifts of the terrain fields and keeps the one whose
    /// middle distance swells highest, so no seed opens on an empty corridor.
    fn choose_layout(&mut self, rng: &mut crate::noise::Rng) {
        let mut best = (f32::MIN, (0.0, 0.0));
        for _ in 0..LAYOUT_TRIES {
            self.offset = (rng.range(-LAYOUT_RANGE, LAYOUT_RANGE), rng.range(-LAYOUT_RANGE, LAYOUT_RANGE));
            let score: f32 = MIDGROUND.iter().map(|&(x, z)| self.sand(x * self.spacing, z * self.spacing, 0.0).crest).sum();
            if score > best.0 {
                best = (score, self.offset);
            }
        }
        self.offset = best.1;
    }

    /// The dune profile over one period (0–1): a windward rise to the
    /// brink at `windward`, then the slip face down to the next trough.
    #[inline]
    fn profile(&self, p: f32) -> f32 {
        let k = self.windward;
        if p < k {
            let u = p / k;
            // Rounded brink (smoothstep) or knife edge (u²(2 − u) meets the
            // crest still climbing).
            let round = u * u * (3.0 - 2.0 * u);
            let knife = u * u * (2.0 - u);
            round + (knife - round) * self.brink
        } else {
            let q = 1.0 - (p - k) / (1.0 - k);
            // A straight avalanche face with a soft concave toe (q^1.5).
            q * q.sqrt() * (1.0 / SLIP_TOE) + q * (1.0 - 1.0 / SLIP_TOE)
        }
    }

    /// Sand at world `(x, z)`; `foot` is the ground footprint of a pixel.
    #[inline]
    fn sand(&self, x: f32, z: f32, foot: f32) -> Sand {
        let inv = 1.0 / self.spacing;
        let u = (x * self.wind.0 + z * self.wind.1) * inv + self.offset.0;
        let v = (z * self.wind.0 - x * self.wind.1) * inv + self.offset.1;
        let [s0, s1, s2, _] = self.seeds;
        let m = noise2(u * MEANDER_FREQ, v * MEANDER_FREQ, s0) + 0.5 * noise2(u * MEANDER_FREQ * MEANDER_LACUNARITY, v * MEANDER_FREQ * MEANDER_LACUNARITY, s1);
        let sw = smoothstep(SWELL_LO, SWELL_HI, noise2(u * SWELL_ACROSS, v * SWELL_ALONG, s2));
        let amp = SWELL_FLOOR + (1.0 - SWELL_FLOOR) * sw;
        // Tall middles lag upwind of their low horns: crescents.
        let p = fract(u + MEANDER * m + BOW * sw);
        let detail = 1.0 - smoothstep(LOD_FROM, LOD_TO, foot * inv);
        let prof = PROFILE_MEAN + (self.profile(p) - PROFILE_MEAN) * detail;
        let crest = self.height * amp * (PROFILE_MEAN + (1.0 - PROFILE_MEAN) * detail);
        Sand { h: self.height * amp * prof, p, crest, detail }
    }

    #[inline]
    fn h(&self, x: f32, z: f32, foot: f32) -> f32 {
        self.sand(x, z, foot).h
    }

    /// Sky light along screen row `y` at column `x` (the sand's haze uses
    /// the horizon row).
    fn sky(&self, x: f32, y: f32, disc: bool) -> Lab {
        let v = ((y + 0.5) / (self.horizon + 0.5)).clamp(0.0, 1.0);
        // Deep zenith to the palette's blue, then a plain (desaturating) fade
        // into the warm horizon, as dusk air does.
        let mut c = self.sky_top.mix_vivid(self.sky_mid, smoothstep(0.0, SKY_MID_AT, v)).mix(self.sky_low, pow(v, SKY_CURVE));
        let (dx, dy) = (x - self.sun_at.0, y - self.sun_at.1);
        let d = (dx * dx + dy * dy).sqrt();
        let band = exp(-((y - self.horizon) / HORIZON_BAND).powi(2)) * exp(-(dx * dx));
        // The bloom thins toward the zenith so the deep sky stays clean.
        let reach = ZENITH_GLOW + (1.0 - ZENITH_GLOW) * v;
        c = super::light(c, self.sun, self.glow * (GLOW_GAIN * reach * super::glow(d, GLOW_CORE, GLOW_BLOOM) + BAND_GAIN * band));
        if disc {
            let cov = 1.0 - smoothstep(SUN_R - self.edge, SUN_R + self.edge, d);
            let cov = cov * (1.0 - SUN_VEIL * self.haze);
            // On a pale sky the disc is added as light (a painted near-white
            // disc would read darker than the glow around it).
            c = if self.light_mode { super::light(c, self.disc, cov * LIGHT_DISC_GAIN) } else { c.mix(self.disc, cov) };
        }
        c
    }

    /// Shaded sand at the hit `t` along the ray (`dx, dy, dz`), seen at
    /// screen `(sx, sy)`; `grain` is the ground footprint per unit `t`.
    /// Also returns the ground distance from the hit to the nearest brink.
    fn shade_sand(&self, t: f32, dir: (f32, f32, f32), grain: f32, sx: f32) -> (Lab, f32) {
        let (x, z) = (dir.0 * t, dir.2 * t);
        let foot = t * grain;
        let here = self.sand(x, z, foot);
        let e = foot.max(1e-3);
        let hx = (self.h(x + e, z, foot) - here.h) / e;
        let hz = (self.h(x, z + e, foot) - here.h) / e;
        let (mut nx, ny, mut nz) = (-hx, 1.0, -hz);
        let k = self.windward;
        // Wind ripples, only on the windward faces (avalanche faces are
        // smooth), fading with distance.
        let wavelength = 1.0 / RIPPLE_FREQ;
        let rip_lod = 1.0 - smoothstep(RIPPLE_LOD_FROM, RIPPLE_LOD_TO, foot / wavelength);
        if self.ripples > 0.0 && rip_lod > 0.0 && here.p < k {
            let q = here.p / k;
            let on_face = smoothstep(RIPPLE_FACE.0, RIPPLE_FACE.1, q) * (1.0 - smoothstep(RIPPLE_FACE.2, RIPPLE_FACE.3, q));
            let along = x * self.wind.0 + z * self.wind.1;
            let across = z * self.wind.0 - x * self.wind.1;
            let warp = noise2(along * RIPPLE_WARP_FREQ, across * RIPPLE_WARP_FREQ, self.seeds[3]);
            // Ripple trains come and go: their depth follows a second field.
            let depth = 1.0 - RIPPLE_PATCHES * (0.5 + 0.5 * noise2(along * RIPPLE_PATCH_FREQ + RIPPLE_PATCH_OFFSET, across * RIPPLE_PATCH_FREQ, self.seeds[3]));
            let ph = std::f32::consts::TAU * (along * RIPPLE_FREQ + RIPPLE_WARP * warp + self.ripple_phase);
            let r = sin(ph) + RIPPLE_SKEW * sin(2.0 * ph);
            let tilt = RIPPLE_TILT * self.ripples * r * on_face * rip_lod * here.detail * depth;
            nx -= tilt * self.wind.0;
            nz -= tilt * self.wind.1;
        }
        let nl = (nx * nx + ny * ny + nz * nz).sqrt();
        let (nx, ny, nz) = (nx / nl, ny / nl, nz / nl);
        let (lx, ly, lz) = self.sun_dir;
        let lambert = (nx * lx + ny * ly + nz * lz).max(0.0);
        // Cast shadow, in profile space: the line from the brink that shades
        // it falls at the sun's slope across the slip face and the trough.
        let behind = if self.from_upwind { fract(here.p - k) } else { fract(k - here.p) } * self.spacing;
        let line = here.crest - behind * self.shadow_tan;
        let pen = PENUMBRA * behind + foot;
        let lit_share = smoothstep(-pen, pen, here.h - line);
        let sunlit = lambert * (1.0 - here.detail * (1.0 - lit_share));
        let mut tone = smoothstep(0.0, self.lit_full, sunlit);
        tone = FAR_TONE + (tone - FAR_TONE) * here.detail;
        let mut c = self.shade_c.mix_vivid(self.fill, SHADOW_FILL * ny * ny);
        // Sun and shade add in linear light, as irradiance does: penumbrae
        // warm quickly instead of passing through a green OKLab midpoint.
        let (a, b) = (c.to_linear(), self.lit.to_linear());
        c = Lab::from_linear([0, 1, 2].map(|i| lerp(a[i].max(0.0), b[i].max(0.0), tone)));
        // Cool sky fill on upward faces, warm bounce in shadows next to light.
        c.l += SKY_FILL * (ny - SKY_FILL_LEVEL) + BOUNCE * (1.0 - tone) * lit_share;
        // The lit brink catches a fine highlight.
        let width = BRINK_WIDTH.max(foot / self.spacing * BRINK_PIXELS);
        let brink = exp(-((here.p - k) / width).powi(2)) * here.detail * lit_share * (BRINK_WIDTH / width);
        c = super::light(c, self.sun, BRINK_LIGHT * brink * if self.light_mode { 0.5 } else { 1.0 });
        // Aerial perspective: distant dunes melt into the horizon's glow.
        let haze = 1.0 - exp(-t * self.haze / HAZE_DIST);
        let to_brink = (here.p - k).abs().min(1.0 - (here.p - k).abs()) * self.spacing;
        (c.mix_vivid(self.sky(sx, self.horizon, false), haze), to_brink)
    }
}

impl Dunes {
    /// One view ray through screen `(x, y)`: its color, and whether it
    /// lies on a crest edge (it grazed a crest, or hit within a pixel of
    /// a brink) and so needs supersampling.
    fn trace(&self, x: f32, y: f32) -> (Lab, bool) {
        // View ray: camera at height EYE, screen at focal distance 1, the
        // horizon shifted (a rising lens) to its row.
        let (dx, dy, dz) = (x, self.horizon - y, 1.0);
        let len = (dx * dx + dy * dy + dz * dz).sqrt();
        let dir = (dx / len, dy / len, dz / len);
        if dir.1 >= 0.0 {
            return (self.sky(x, y, true), false);
        }
        let down = -dir.1;
        // Pixel cone (world units per unit t) and its footprint on the ground.
        let cone = self.px / len;
        let grain = cone / down.max(1e-4);
        // The ray only meets sand between the tallest dune's plane and the floor.
        let t_top = (EYE - self.height) / down;
        let t_floor = EYE / down;
        let mut t = t_top;
        let mut prev = t;
        let (mut low, mut grazed) = (f32::MAX, false);
        let inv_rate = 1.0 / (SLOPE_BOUND + down);
        let relief_foot = self.spacing * EDGE_RELIEF;
        for _ in 0..MAX_STEPS {
            let py = EYE - down * t;
            let clear = py - self.h(dir.0 * t, dir.2 * t, t * grain);
            if clear < 0.0 {
                break;
            }
            let px_clear = clear / (t * cone);
            // A crest pass: the clearance dipped near the sand and rose again
            // (only crests that keep their relief have knife edges).
            if px_clear < low {
                low = px_clear;
            } else if low < EDGE_PIXELS && px_clear > low + EDGE_RISE && t * grain < relief_foot {
                grazed = true;
            }
            prev = t;
            let floor = if px_clear < NEAR_PIXELS { t * cone * STEP_PIXELS } else { t * STEP_SHARE };
            t = (t + (clear * inv_rate).max(floor)).min(t_floor);
            if t >= t_floor {
                break;
            }
        }
        // Bisect between the last clear point and the hit.
        let (mut lo, mut hi) = (prev, t);
        for _ in 0..REFINE {
            let mid = 0.5 * (lo + hi);
            if EYE - down * mid < self.h(dir.0 * mid, dir.2 * mid, mid * grain) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        let (c, brink) = self.shade_sand(hi, dir, grain, x);
        let foot = hi * grain;
        (c, grazed || (brink < foot * EDGE_PIXELS && foot < relief_foot))
    }
}

impl Scene for Dunes {
    fn shade(&self, x: f32, y: f32) -> Lab {
        let (c, edge) = self.trace(x, y);
        if !edge || self.preview {
            return c;
        }
        // Crest edges: a rotated-grid supersample (knife edges stay clean).
        let mut sum = c;
        for (ox, oy) in EDGE_TAPS {
            sum = sum.add(self.trace(x + ox * self.px, y + oy * self.px).0);
        }
        sum.scale(1.0 / (EDGE_TAPS.len() + 1) as f32)
    }
}

