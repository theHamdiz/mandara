//! Periods and orders. The panel and sidebar draw a fresh reminder each
//! time they open ([`super::recent`]); the desktop widgets change theirs
//! once a period (the local day, or hour: [`period`]). "Today's step" is
//! one path a day, a pure function of the date, every path once before
//! any repeats. The shuffled rounds ([`round_order`], [`nth_index`]) also
//! drive Hadith's hadith of the day.

use chrono::{Datelike as _, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike as _};

use super::catalog::is_avoid;
use super::paths::{PATHS, Path};

/// How often the desktop widgets' reminder changes.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Rotate {
    #[default]
    Daily,
    Hourly,
}

impl Rotate {
    pub fn parse(s: &str) -> Self {
        if s.eq_ignore_ascii_case("hourly") { Rotate::Hourly } else { Rotate::Daily }
    }
}

/// Which items Jannah picks from.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Sources {
    #[default]
    Both,
    Quran,
    Hadith,
}

impl Sources {
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "quran" => Sources::Quran,
            "hadith" => Sources::Hadith,
            _ => Sources::Both,
        }
    }
}

const HOURS_PER_DAY: i64 = 24;

/// The period `now` falls in: days since 0001-01-01 (or hours).
pub fn period(now: NaiveDateTime, rotate: Rotate) -> i64 {
    let day = i64::from(now.date().num_days_from_ce());
    match rotate {
        Rotate::Daily => day,
        Rotate::Hourly => day * HOURS_PER_DAY + i64::from(now.hour()),
    }
}

/// When the period after `now`'s starts (local midnight, or the next hour).
pub fn next_boundary(now: NaiveDateTime, rotate: Rotate) -> NaiveDateTime {
    match rotate {
        Rotate::Daily => (now.date() + Duration::days(1)).and_time(NaiveTime::MIN),
        Rotate::Hourly => now.date().and_time(NaiveTime::MIN) + Duration::hours(i64::from(now.hour()) + 1),
    }
}

/// The step order's seed (the tests reuse the others' old seeds).
#[cfg(test)]
const SEED_QURAN: u64 = 0x6A61_6E6E_6168_0001;
#[cfg(test)]
const SEED_HADITH: u64 = 0x6A61_6E6E_6168_0002;
const SEED_STEP: u64 = 0x6A61_6E6E_6168_0003;
/// SplitMix64 constants.
const MIX_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
const MIX_A: u64 = 0xBF58_476D_1CE4_E5B9;
const MIX_B: u64 = 0x94D0_49BB_1331_11EB;

/// A SplitMix64 step: deterministic on every platform.
fn mix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(MIX_GAMMA);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(MIX_A);
    z = (z ^ (z >> 27)).wrapping_mul(MIX_B);
    z ^ (z >> 31)
}

/// The order of round `round` over `n` items: a Fisher–Yates shuffle
/// seeded by the kind and the round.
pub fn order(n: usize, seed: u64, round: i64) -> Vec<usize> {
    let mut v: Vec<usize> = (0..n).collect();
    let mut state = seed ^ (round as u64).wrapping_mul(MIX_GAMMA);
    for i in (1..n).rev() {
        let j = (mix(&mut state) % (i as u64 + 1)) as usize;
        v.swap(i, j);
    }
    v
}

/// The fewest slots before an item may come back where one round of a
/// shuffled order ends and the next begins (within a round nothing
/// repeats at all). A pool too small for it gets a third of its size.
pub const RECENT: usize = 8;

/// How many items at the end of a round the next round's start keeps
/// clear of, for a pool of `n` (none below three items: two alternate,
/// one is all there is).
pub fn guard(n: usize) -> usize {
    if n < 3 { 0 } else { RECENT.min(n / 3) }
}

/// The last `w` places of round `round`'s plain [`order`] (`w` at most
/// [`RECENT`]), without shuffling the whole round: Fisher–Yates fixes the
/// last place first, so its first `w` swaps decide them.
fn plain_tail(n: usize, seed: u64, round: i64, w: usize) -> ([usize; RECENT], usize) {
    let w = w.min(RECENT).min(n.saturating_sub(1));
    // The positions the swaps touched and what they hold now (everything
    // else still holds its own index).
    let mut moved = [(0usize, 0usize); 2 * RECENT];
    let mut len = 0;
    let get = |moved: &[(usize, usize)], pos: usize| moved.iter().find(|(p, _)| *p == pos).map_or(pos, |(_, v)| *v);
    let mut state = seed ^ (round as u64).wrapping_mul(MIX_GAMMA);
    for i in (n - w..n).rev() {
        let j = (mix(&mut state) % (i as u64 + 1)) as usize;
        let (vi, vj) = (get(&moved[..len], i), get(&moved[..len], j));
        for (pos, value) in [(i, vj), (j, vi)] {
            match moved[..len].iter_mut().find(|(p, _)| *p == pos) {
                Some(slot) => slot.1 = value,
                None => {
                    moved[len] = (pos, value);
                    len += 1;
                }
            }
        }
    }
    let mut out = [0usize; RECENT];
    for (k, slot) in out.iter_mut().take(w).enumerate() {
        *slot = get(&moved[..len], n - w + k);
    }
    (out, w)
}

/// The order of round `round` over `n` items with no repeat across its
/// start: the round's first [`guard`] places never hold one of the
/// previous round's last ones (each such item trades places with one from
/// the middle of the round, so every round's end stays its plain order's
/// and the rule never looks further back). Two items always alternate.
pub fn round_order(n: usize, seed: u64, round: i64) -> Vec<usize> {
    if n <= 2 {
        return order(n, seed, 0);
    }
    let mut v = order(n, seed, round);
    let w = guard(n);
    let (tail, w) = plain_tail(n, seed, round.wrapping_sub(1), w);
    let tail = &tail[..w];
    // n >= 3w: the middle always has enough items the tail doesn't hold.
    let mut k = w;
    for j in 0..w {
        if !tail.contains(&v[j]) {
            continue;
        }
        while k < n - w && tail.contains(&v[k]) {
            k += 1;
        }
        if k < n - w {
            v.swap(j, k);
            k += 1;
        }
    }
    v
}

/// Place `k` of an endless sequence over a pool of `n`: the round and the
/// pool index there (`None` for an empty pool).
pub fn nth_index(n: usize, seed: u64, k: i64) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let len = n as i64;
    round_order(n, seed, k.div_euclid(len)).get(k.rem_euclid(len) as usize).copied()
}

/// Where place `k` falls in its round: `(1-based place, pool size)`.
pub fn round_place(n: usize, k: i64) -> Option<(usize, usize)> {
    (n > 0).then(|| (k.rem_euclid(n as i64) as usize + 1, n))
}

/// The paths a day's step is chosen from (all but "keep away from").
pub fn step_paths() -> Vec<&'static Path> {
    PATHS.iter().filter(|p| !is_avoid(&p.group)).collect()
}

/// Today's step: one path a day, every path once before any repeats.
pub fn step(date: NaiveDate) -> Option<&'static Path> {
    let paths = step_paths();
    let n = paths.len();
    if n == 0 {
        return None;
    }
    let day = i64::from(date.num_days_from_ce());
    let o = order(n, SEED_STEP, day.div_euclid(n as i64));
    Some(paths[o[day.rem_euclid(n as i64) as usize]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn at(y: i32, m: u32, d: u32, h: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, 30, 0).unwrap()
    }

    #[test]
    fn periods_and_boundaries() {
        let now = at(2026, 11, 2, 14);
        assert_eq!(period(now, Rotate::Hourly), period(now, Rotate::Daily) * 24 + 14);
        assert_eq!(period(at(2026, 11, 3, 0), Rotate::Daily), period(now, Rotate::Daily) + 1);
        assert_eq!(next_boundary(now, Rotate::Daily), NaiveDate::from_ymd_opt(2026, 11, 3).unwrap().and_hms_opt(0, 0, 0).unwrap());
        assert_eq!(next_boundary(now, Rotate::Hourly), NaiveDate::from_ymd_opt(2026, 11, 2).unwrap().and_hms_opt(15, 0, 0).unwrap());
        assert_eq!(next_boundary(at(2026, 12, 31, 23), Rotate::Hourly), NaiveDate::from_ymd_opt(2027, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap());
        assert_eq!(Rotate::parse("Hourly"), Rotate::Hourly);
        assert_eq!(Sources::parse("nonsense"), Sources::Both);
    }

    #[test]
    fn the_same_place_always_gives_the_same_item() {
        for k in [-5, 0, 1, 739_000, 739_001] {
            assert_eq!(nth_index(40, SEED_QURAN, k), nth_index(40, SEED_QURAN, k));
        }
        assert_eq!(order(40, SEED_QURAN, 7), order(40, SEED_QURAN, 7));
        assert_ne!(order(40, SEED_QURAN, 7), order(40, SEED_QURAN, 8), "each round has its own order");
    }

    #[test]
    fn a_round_shows_every_item_once() {
        for n in [1usize, 2, 37, 230] {
            let len = n as i64;
            let start = 739_000 * len;
            let seen: HashSet<usize> = (start..start + len).filter_map(|k| nth_index(n, SEED_HADITH, k)).collect();
            assert_eq!(seen.len(), n);
        }
    }

    /// Every `window` consecutive places of the sequence over `n` hold
    /// distinct items, across many round boundaries.
    fn assert_no_repeat_within(n: usize, window: usize) {
        let len = n as i64;
        for round in [-3i64, -1, 0, 1, 2, 57, 739_000] {
            let from = round * len - window as i64;
            let seq: Vec<usize> = (from..from + 2 * len + window as i64).map(|k| nth_index(n, SEED_QURAN, k).unwrap()).collect();
            for w in seq.windows(window) {
                let set: HashSet<_> = w.iter().collect();
                assert_eq!(set.len(), window, "n {n}, round {round}: {w:?}");
            }
        }
    }

    #[test]
    fn next_never_repeats_what_was_just_shown() {
        // Large pools: no item comes back within RECENT + 1 places, even
        // where one round ends and the next begins.
        for n in [24, 40, 97, 300, 6236] {
            assert_no_repeat_within(n, RECENT + 1);
        }
        // Small pools get a third of their size.
        for n in [3, 4, 5, 6, 9, 12, 20] {
            assert_no_repeat_within(n, guard(n) + 1);
            assert!(guard(n) >= 1);
        }
        // Two items alternate; one item is all there is; none is nothing.
        let two: Vec<usize> = (-4..8).map(|k| nth_index(2, SEED_HADITH, k).unwrap()).collect();
        assert!(two.windows(2).all(|w| w[0] != w[1]), "{two:?}");
        assert!((-3..3).all(|k| nth_index(1, SEED_HADITH, k) == Some(0)));
        assert_eq!(nth_index(0, SEED_HADITH, 5), None);
        assert_eq!(round_order(0, SEED_HADITH, 3), Vec::<usize>::new());
    }

    #[test]
    fn every_round_is_still_a_whole_permutation() {
        for n in [1, 2, 3, 7, 25, 100] {
            for round in [-2, 0, 1, 99] {
                let mut o = round_order(n, SEED_STEP, round);
                o.sort_unstable();
                assert_eq!(o, (0..n).collect::<Vec<_>>(), "n {n} round {round}");
            }
        }
        // Without the guard, plain rounds do bring an item straight back.
        let w = guard(24);
        assert!((0..200).any(|r| { let (prev, next) = (order(24, SEED_QURAN, r - 1), order(24, SEED_QURAN, r)); next[..w].iter().any(|i| prev[24 - w..].contains(i)) }));
        // Only a round's start may change, and only rarely: the tail is
        // its plain order's (what the next round's guard reads).
        for round in 0..50 {
            let (plain, adjusted) = (order(60, SEED_QURAN, round), round_order(60, SEED_QURAN, round));
            let w = guard(60);
            assert_eq!(plain[60 - w..], adjusted[60 - w..]);
            // Today's item moves only where a swap happened: at most two
            // places per clash, so existing days barely change.
            assert!(plain.iter().zip(&adjusted).filter(|(a, b)| a != b).count() <= 2 * w);
            assert_eq!(plain_tail(60, SEED_QURAN, round, w).0[..w], plain[60 - w..]);
        }
    }

    #[test]
    fn places_count_through_the_round() {
        let n = 230;
        assert_eq!(round_place(n, 0), Some((1, n)));
        assert_eq!(round_place(n, n as i64 + 11), Some((12, n)));
        assert_eq!(round_place(n, -1), Some((n, n)));
        assert_eq!(round_place(0, 4), None);
        assert_eq!(round_place(3, 7), Some((2, 3)));
    }

    #[test]
    fn one_step_a_day_never_from_keep_away() {
        let paths = step_paths();
        let start = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let day0 = i64::from(start.num_days_from_ce());
        let n = paths.len() as i64;
        // Align to a round so the window holds exactly one.
        let first = start + Duration::days(n - day0.rem_euclid(n));
        let keys: HashSet<&str> = (0..n).map(|d| step(first + Duration::days(d)).unwrap().key).collect();
        assert_eq!(keys.len() as i64, n);
        assert!((0..60).all(|d| step(start + Duration::days(d)).is_some_and(|p| !is_avoid(&p.group))));
        assert_eq!(step(start).map(|p| p.key), step(start).map(|p| p.key));
    }
}
