//! A tiny seeded random number generator (SplitMix64) for the games: the
//! same seed always deals the same puzzle and the same tiles, on every
//! machine and every version, which a library generator can't promise.

/// SplitMix64's increment (the golden ratio, as a 64-bit odd constant).
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;
const MIX_1: u64 = 0xBF58_476D_1CE4_E5B9;
const MIX_2: u64 = 0x94D0_49BB_1331_11EB;

/// SplitMix64: one `u64` of state, statistically fine for games.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(GOLDEN);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(MIX_1);
        z = (z ^ (z >> 27)).wrapping_mul(MIX_2);
        z ^ (z >> 31)
    }

    /// A uniform value in `0..n` (`n > 0`), without modulo bias.
    pub fn below(&mut self, n: u32) -> u32 {
        debug_assert!(n > 0);
        // Lemire's multiply-shift with rejection.
        let n64 = u64::from(n);
        loop {
            let m = (self.next_u64() >> 32) * n64;
            if (m as u32) >= (n.wrapping_neg() % n) {
                return (m >> 32) as u32;
            }
        }
    }

    /// Shuffles `items` in place (Fisher–Yates).
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i as u32 + 1) as usize;
            items.swap(i, j);
        }
    }
}

/// A fresh seed from the clock and the process (new games).
pub fn fresh_seed() -> u64 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let mut r = Rng::new(nanos ^ u64::from(std::process::id()).rotate_left(32));
    r.next_u64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_stream() {
        let (mut a, mut b) = (Rng::new(42), Rng::new(42));
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        // A known value keeps the stream stable across versions.
        assert_eq!(Rng::new(0).next_u64(), 0xE220_A839_7B1D_CDAF);
    }

    #[test]
    fn below_stays_in_range_and_covers_it() {
        let mut r = Rng::new(7);
        let mut seen = [0u32; 6];
        for _ in 0..6000 {
            seen[r.below(6) as usize] += 1;
        }
        assert!(seen.iter().all(|&n| n > 800), "{seen:?}");
    }

    #[test]
    fn shuffle_is_a_permutation() {
        let mut v: Vec<u8> = (0..81).collect();
        Rng::new(3).shuffle(&mut v);
        let mut sorted = v.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..81).collect::<Vec<u8>>());
        assert_ne!(v, sorted);
    }
}
