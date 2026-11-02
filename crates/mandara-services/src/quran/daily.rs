//! The ayah of the day: a pure function of the date, the same on every
//! surface and every machine. Days walk a fixed shuffle of the ayahs that
//! read well on a card (not a few letters, not a page long), so an ayah
//! comes back only after every other one has had its day.

use chrono::{Datelike as _, NaiveDate};

use super::text::Quran;

/// Ayahs shorter or longer than this (characters) don't make a card.
pub const MIN_CHARS: usize = 40;
pub const MAX_CHARS: usize = 230;
/// The shuffle's seed (any odd constant; fixed so days stay stable).
const SEED: u64 = 0x5155_5241_4E21_D00D;

/// splitmix64: a fast, well-mixed hash of `x`.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// The global numbers of the ayahs that fit a card, in a fixed shuffled
/// order.
pub fn candidates(q: &Quran) -> Vec<u16> {
    let mut list: Vec<u16> = q.ayahs.iter().enumerate().filter(|(_, a)| (MIN_CHARS..=MAX_CHARS).contains(&a.text.chars().count())).map(|(i, _)| i as u16 + 1).collect();
    list.sort_by_key(|g| mix(SEED ^ u64::from(*g)));
    list
}

/// The ayah of `day` (global number) from `candidates` (`None` when there
/// are none).
pub fn pick(candidates: &[u16], day: NaiveDate) -> Option<u16> {
    if candidates.is_empty() {
        return None;
    }
    let n = i64::from(day.num_days_from_ce()).rem_euclid(candidates.len() as i64);
    candidates.get(n as usize).copied()
}

#[cfg(test)]
mod tests {
    use super::super::text::tests::sample;
    use super::*;

    #[test]
    fn every_day_has_a_stable_ayah_that_fits() {
        let mut q = sample();
        for (i, a) in q.ayahs.iter_mut().enumerate() {
            a.text = "ك".repeat(10 + i % 300);
        }
        let c = candidates(&q);
        assert!(!c.is_empty());
        assert!(c.iter().all(|g| (MIN_CHARS..=MAX_CHARS).contains(&q.ayah_at(*g).unwrap().text.chars().count())));
        let day = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        assert_eq!(pick(&c, day), pick(&candidates(&q), day), "stable");
        // A run of days shows different ayahs until the list comes round.
        let week: std::collections::HashSet<u16> = (0..7).filter_map(|d| pick(&c, day + chrono::Duration::days(d))).collect();
        assert_eq!(week.len(), 7);
        let back = day + chrono::Duration::days(c.len() as i64);
        assert_eq!(pick(&c, back), pick(&c, day));
        // The shuffle isn't the plain order.
        assert!(c.windows(2).any(|w| w[1] < w[0]));
        assert_eq!(pick(&[], day), None);
    }
}
