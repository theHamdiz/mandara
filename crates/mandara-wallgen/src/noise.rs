//! Seeded, deterministic noise: an integer hash, improved-Perlin gradient
//! noise that can loop in time (its lattice wraps along `z`), and fractal
//! sums whose octave count follows the pixel size, so detail finer than a
//! pixel is never summed (no aliasing at 1080p, full detail at 8K).

use crate::fmath::{cos, sin};

/// Each fractal octave is turned by this angle (radians) so the lattice
/// axes of successive octaves never line up into visible grid artefacts.
const OCTAVE_TWIST: f32 = 0.643;
/// Octaves stop once one noise cell is smaller than this many pixels…
const LOD_STOP: f32 = 2.0;
/// …and the last one fades in between this many pixels and the stop.
const LOD_FADE: f32 = 4.0;
/// Lattice cells a looping field travels through per loop. One cell is
/// degenerate (Perlin noise at depths 0, ½ and 1 of a one-cell wrap is the
/// same field), so loops use two.
pub const LOOP_PERIOD: i32 = 2;
/// Hard cap on octaves (an 8K render of the finest style needs ~9).
const MAX_OCTAVES: u32 = 10;

/// 32-bit integer hash of three lattice coordinates and a seed
/// (multiply–xorshift avalanche; every input bit reaches every output bit).
#[inline]
pub fn hash3(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    let mut h = seed ^ (x as u32).wrapping_mul(0x8DA6_B343) ^ (y as u32).wrapping_mul(0xD816_3841) ^ (z as u32).wrapping_mul(0xCB1A_B31F);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    h
}

/// Hash → uniform value in [0, 1).
#[inline]
pub fn unit(h: u32) -> f32 {
    (h >> 8) as f32 * (1.0 / (1u32 << 24) as f32)
}

/// SplitMix64 step: the generator behind [`Rng`].
#[inline]
pub fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A tiny deterministic random stream for scene layout (blob positions,
/// layer counts…). Never used per pixel.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(splitmix(seed))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = splitmix(self.0);
        self.0
    }

    /// Uniform [0, 1).
    pub fn f(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / (1u64 << 24) as f32)
    }

    /// Uniform in [a, b).
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }

    /// Approximately standard normal (sum of four uniforms).
    pub fn normal(&mut self) -> f32 {
        let s: f32 = (0..4).map(|_| self.f()).sum();
        (s - 2.0) * 3f32.sqrt()
    }

    /// Uniform index below `n` (`n > 0`).
    pub fn below(&mut self, n: usize) -> usize {
        ((self.next_u64() >> 11) % n.max(1) as u64) as usize
    }

    /// A 32-bit seed for a noise field.
    pub fn seed32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Dot product of one of Perlin's twelve edge gradients with `(x, y, z)`.
#[inline]
fn grad(h: u32, x: f32, y: f32, z: f32) -> f32 {
    match h & 15 {
        0 | 12 => x + y,
        1 | 14 => -x + y,
        2 => x - y,
        3 => -x - y,
        4 => x + z,
        5 => -x + z,
        6 => x - z,
        7 => -x - z,
        8 => y + z,
        9 | 13 => -y + z,
        10 => y - z,
        _ => -y - z,
    }
}

/// Improved Perlin noise in roughly [-1, 1]. With `period > 0` the lattice
/// wraps along `z` every `period` units, so `z = t · period` for a loop
/// phase `t ∈ [0, 1)` returns to the same field at `t = 1`.
#[inline]
pub fn noise3(x: f32, y: f32, z: f32, seed: u32, period: i32) -> f32 {
    let (fx, fy, fz) = (x.floor(), y.floor(), z.floor());
    let (ix, iy) = (fx as i32, fy as i32);
    let iz = fz as i32;
    let (x, y, z) = (x - fx, y - fy, z - fz);
    let wrap = |k: i32| if period > 0 { k.rem_euclid(period) } else { k };
    let (z0, z1) = (wrap(iz), wrap(iz + 1));
    let (u, v, w) = (fade(x), fade(y), fade(z));
    let c = |dx: i32, dy: i32, zi: i32, dz: f32| grad(hash3(ix + dx, iy + dy, zi, seed), x - dx as f32, y - dy as f32, z - dz);
    let x00 = c(0, 0, z0, 0.0) + u * (c(1, 0, z0, 0.0) - c(0, 0, z0, 0.0));
    let x10 = c(0, 1, z0, 0.0) + u * (c(1, 1, z0, 0.0) - c(0, 1, z0, 0.0));
    let x01 = c(0, 0, z1, 1.0) + u * (c(1, 0, z1, 1.0) - c(0, 0, z1, 1.0));
    let x11 = c(0, 1, z1, 1.0) + u * (c(1, 1, z1, 1.0) - c(0, 1, z1, 1.0));
    let y0 = x00 + v * (x10 - x00);
    let y1 = x01 + v * (x11 - x01);
    y0 + w * (y1 - y0)
}

/// Dot product of one of eight unit-ish 2D gradients with `(x, y)`.
#[inline]
fn grad2(h: u32, x: f32, y: f32) -> f32 {
    match h & 7 {
        0 => x + y,
        1 => -x + y,
        2 => x - y,
        3 => -x - y,
        4 => x * GRAD2_AXIS,
        5 => -x * GRAD2_AXIS,
        6 => y * GRAD2_AXIS,
        _ => -y * GRAD2_AXIS,
    }
}

/// Axis gradients are lengthened to match the diagonals' reach.
const GRAD2_AXIS: f32 = std::f32::consts::SQRT_2;

/// 2D Perlin gradient noise in roughly [-1, 1]: half the hashes of
/// [`noise3`], for static fields evaluated many times per pixel (a
/// raymarched heightfield).
#[inline]
pub fn noise2(x: f32, y: f32, seed: u32) -> f32 {
    let (fx, fy) = (x.floor(), y.floor());
    let (ix, iy) = (fx as i32, fy as i32);
    let (x, y) = (x - fx, y - fy);
    let (u, v) = (fade(x), fade(y));
    let c = |dx: i32, dy: i32| grad2(hash3(ix + dx, iy + dy, 0, seed), x - dx as f32, y - dy as f32);
    let (c00, c10, c01, c11) = (c(0, 0), c(1, 0), c(0, 1), c(1, 1));
    let y0 = c00 + u * (c10 - c00);
    let y1 = c01 + u * (c11 - c01);
    (y0 + v * (y1 - y0)) * NOISE2_SCALE
}

/// 2D Perlin noise peaks near ±0.7; this brings it to about ±1.
const NOISE2_SCALE: f32 = 1.4;

/// A fractal (fBm) sum of [`noise3`] octaves.
#[derive(Clone, Copy, Debug)]
pub struct Fractal {
    pub seed: u32,
    /// Frequency of the first octave, in cycles per screen height.
    pub freq: f32,
    /// Most octaves summed (fewer when a cell would be under two pixels).
    pub octaves: u32,
    /// Frequency multiplier per octave.
    pub lacunarity: f32,
    /// Amplitude multiplier per octave.
    pub gain: f32,
    /// Loop period along `z` (see [`noise3`]); 0 for none.
    pub period: i32,
}

impl Fractal {
    pub fn new(seed: u32, freq: f32, octaves: u32) -> Self {
        Fractal { seed, freq, octaves, lacunarity: 2.0, gain: 0.5, period: LOOP_PERIOD }
    }

    pub fn gain(mut self, gain: f32) -> Self {
        self.gain = gain;
        self
    }

    /// Sum at `(x, y)` (screen-height units) and loop depth `z`, for a
    /// pixel `px` screen-heights wide. Normalized so the result stays in
    /// roughly [-1, 1] whatever the octave count.
    #[inline]
    pub fn at(&self, x: f32, y: f32, z: f32, px: f32) -> f32 {
        let (s, c) = (sin(OCTAVE_TWIST), cos(OCTAVE_TWIST));
        let (mut x, mut y) = (x * self.freq, y * self.freq);
        let mut amp = 1.0;
        let mut freq = self.freq;
        let mut sum = 0.0;
        for i in 0..self.octaves.min(MAX_OCTAVES) {
            // Pixels per noise cell at this octave.
            let cell_px = 1.0 / (freq * px);
            if cell_px < LOD_STOP && i > 0 {
                break;
            }
            let weight = if i == 0 { 1.0 } else { ((cell_px - LOD_STOP) / (LOD_FADE - LOD_STOP)).clamp(0.0, 1.0) };
            sum += amp * weight * noise3(x, y, z, self.seed.wrapping_add(i * 0x9E37), self.period);
            (x, y) = (x * c - y * s, x * s + y * c);
            x *= self.lacunarity;
            y *= self.lacunarity;
            freq *= self.lacunarity;
            amp *= self.gain;
        }
        // Normalize by the full (uncut) amplitude sum so the field's range
        // does not change with resolution.
        let (mut full, mut a) = (0.0, 1.0);
        for _ in 0..self.octaves.min(MAX_OCTAVES) {
            full += a;
            a *= self.gain;
        }
        sum / full * NOISE_SCALE
    }
}

/// Perlin noise rarely leaves ±0.7; this stretches fractal sums toward ±1.
const NOISE_SCALE: f32 = 1.6;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_bounded_and_varied() {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for i in 0..5000 {
            let n = noise3(i as f32 * 0.137, i as f32 * 0.071, 0.3, 9, 0);
            assert!(n.is_finite() && n.abs() <= 1.1);
            lo = lo.min(n);
            hi = hi.max(n);
        }
        assert!(hi - lo > 1.0, "{lo}..{hi}");
    }

    #[test]
    fn noise_loops_along_z() {
        for i in 0..200 {
            let (x, y) = (i as f32 * 0.31, i as f32 * 0.17);
            let a = noise3(x, y, 0.0, 4, 3);
            let b = noise3(x, y, 3.0, 4, 3);
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn fractal_octaves_follow_pixel_size() {
        let f = Fractal::new(1, 3.0, 8);
        // Same point, coarse vs fine pixels: both finite, and the coarse one
        // is the low-pass of the fine one (close, not equal).
        let coarse = f.at(0.3, 0.4, 0.0, 1.0 / 64.0);
        let fine = f.at(0.3, 0.4, 0.0, 1.0 / 4320.0);
        assert!(coarse.is_finite() && fine.is_finite());
        assert!((coarse - fine).abs() < 0.6);
    }

    #[test]
    fn rng_is_deterministic() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..10 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let n: f32 = (0..4000).map(|_| a.normal()).sum::<f32>() / 4000.0;
        assert!(n.abs() < 0.1);
    }
}
