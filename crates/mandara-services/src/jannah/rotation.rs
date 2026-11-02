//! "Today": one reminder per period, the same on every surface. The period
//! is the local day (or hour); periods alternate Qur'an and hadith, and
//! each kind walks a shuffled order of all its items, so nothing repeats
//! until every item was shown once (the next round has a new order). A
//! surface with little room skips ahead to the next item that fits. The
//! order is a pure function of the date, so every surface, the CLI and
//! tomorrow's shell agree without storing anything but the user's own
//! nudges (`shift`). Also "today's step": one path a day.

use chrono::{Datelike as _, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike as _};

use super::catalog::{Catalog, ItemId, is_avoid};
use super::paths::{PATHS, Path};

/// How often today's reminder changes.
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

/// Which items take turns.
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

/// Seeds of the two kinds' orders and the step order.
const SEED_QURAN: u64 = 0x6A61_6E6E_6168_0001;
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

/// The `k`-th item of a kind's endless sequence: round `k / n`, place
/// `k % n` in that round's order.
fn nth(pool: &[ItemId], seed: u64, k: i64) -> Option<ItemId> {
    let n = pool.len();
    if n == 0 {
        return None;
    }
    let len = n as i64;
    let o = order(n, seed, k.div_euclid(len));
    Some(pool[o[k.rem_euclid(len) as usize]])
}

/// The item for `slot` (a period plus the user's nudges). With both
/// sources, even slots are verses and odd slots hadith; a kind with no
/// items gives its turns to the other. With `limit`, an item longer than
/// that many characters is skipped for the next one of its kind that fits
/// (`None` when none does).
pub fn pick(cat: &Catalog, sources: Sources, slot: i64, limit: Option<usize>) -> Option<ItemId> {
    let quran: Vec<ItemId> = cat.verse_ids().collect();
    let hadith: Vec<ItemId> = cat.hadith_ids().collect();
    let (pool, seed, k) = match sources {
        Sources::Quran if !quran.is_empty() => (&quran, SEED_QURAN, slot),
        Sources::Hadith if !hadith.is_empty() => (&hadith, SEED_HADITH, slot),
        _ if quran.is_empty() => (&hadith, SEED_HADITH, slot),
        _ if hadith.is_empty() => (&quran, SEED_QURAN, slot),
        Sources::Quran => (&quran, SEED_QURAN, slot),
        Sources::Hadith => (&hadith, SEED_HADITH, slot),
        Sources::Both if slot.rem_euclid(2) == 0 => (&quran, SEED_QURAN, slot.div_euclid(2)),
        Sources::Both => (&hadith, SEED_HADITH, slot.div_euclid(2)),
    };
    let fits = |id: ItemId| limit.is_none_or(|l| cat.get(id).is_some_and(|i| i.len <= l));
    // Two rounds' worth of steps always cover one whole round.
    (0..2 * pool.len() as i64).filter_map(|step| nth(pool, seed, k + step)).find(|id| fits(*id))
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
    use super::super::catalog::catalog;
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
    fn the_same_slot_always_gives_the_same_item() {
        let c = catalog();
        for slot in [-5, 0, 1, 739_000, 739_001] {
            assert_eq!(pick(c, Sources::Both, slot, None), pick(c, Sources::Both, slot, None));
        }
        assert_eq!(order(40, SEED_QURAN, 7), order(40, SEED_QURAN, 7));
        assert_ne!(order(40, SEED_QURAN, 7), order(40, SEED_QURAN, 8), "each round has its own order");
    }

    #[test]
    fn a_round_shows_every_item_once() {
        let c = catalog();
        let n = c.verses as i64;
        let start = 739_000 * n;
        let seen: HashSet<ItemId> = (start..start + n).filter_map(|s| pick(c, Sources::Quran, s, None)).collect();
        assert_eq!(seen.len() as i64, n);
        let hadith = (c.items.len() - c.verses) as i64;
        let seen: HashSet<ItemId> = (0..hadith).filter_map(|s| pick(c, Sources::Hadith, s, None)).collect();
        assert_eq!(seen.len() as i64, hadith);
        // Both: the verses' round spans twice as many slots.
        let seen: HashSet<ItemId> = (0..2 * n).filter_map(|s| pick(c, Sources::Both, s, None)).filter(|id| c.get(*id).is_some_and(|i| i.is_verse())).collect();
        assert_eq!(seen.len() as i64, n);
    }

    #[test]
    fn both_sources_alternate() {
        let c = catalog();
        for slot in 100..120 {
            let item = c.get(pick(c, Sources::Both, slot, None).unwrap()).unwrap();
            assert_eq!(item.is_verse(), slot % 2 == 0, "slot {slot}");
        }
        assert!(c.get(pick(c, Sources::Hadith, 100, None).unwrap()).is_some_and(|i| !i.is_verse()));
        assert!(c.get(pick(c, Sources::Quran, 101, None).unwrap()).is_some_and(|i| i.is_verse()));
    }

    #[test]
    fn small_surfaces_skip_long_items() {
        let c = catalog();
        let shortest = c.items[..c.verses].iter().map(|i| i.len).min().unwrap();
        for slot in (0..40).step_by(2) {
            let id = pick(c, Sources::Both, slot, Some(shortest)).unwrap();
            assert!(c.get(id).unwrap().len <= shortest);
            // An item that fits is not skipped.
            let free = pick(c, Sources::Both, slot, None).unwrap();
            if c.get(free).unwrap().len <= shortest {
                assert_eq!(id, free);
            }
        }
        assert_eq!(pick(c, Sources::Quran, 0, Some(0)), None, "nothing fits");
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
