//! The moon's phase from the time alone: the mean synodic month from a
//! reference new moon. Good to within about a day, which is what a phase
//! name and an illumination percentage need.

/// Mean synodic month, days.
pub const SYNODIC: f64 = 29.530_588_853;
/// A reference new moon: 2000-01-06 18:14 UTC, as Unix milliseconds.
const NEW_MOON_MS: f64 = 947_182_440_000.0;
/// Milliseconds per day.
const DAY_MS: f64 = 86_400_000.0;
/// The eight phases as the colour emoji font draws them (as seen from the
/// northern hemisphere: a waxing moon lit on the right). Each ends in
/// U+FE0F so the emoji form is used, not a text font's flat symbol.
const PHASE_GLYPHS: [&str; 8] = ["🌑\u{FE0F}", "🌒\u{FE0F}", "🌓\u{FE0F}", "🌔\u{FE0F}", "🌕\u{FE0F}", "🌖\u{FE0F}", "🌗\u{FE0F}", "🌘\u{FE0F}"];

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Hemisphere {
    North,
    South,
}

/// The emoji of phase `i` as seen from `hemisphere` (the south sees the
/// disc mirrored).
pub fn glyph(i: usize, hemisphere: Hemisphere) -> &'static str {
    match hemisphere {
        Hemisphere::North => PHASE_GLYPHS[i % 8],
        Hemisphere::South => PHASE_GLYPHS[(8 - i % 8) % 8],
    }
}

pub struct Moon {
    /// Days since the last new moon.
    pub age: f64,
}

impl Moon {
    /// The moon at `ms` (Unix milliseconds).
    pub fn at(ms: f64) -> Self {
        Self { age: ((ms - NEW_MOON_MS) / DAY_MS).rem_euclid(SYNODIC) }
    }

    /// A new and a full moon (for small marks beside their names).
    pub fn new_moon() -> Self {
        Self { age: 0.0 }
    }

    pub fn full_moon() -> Self {
        Self { age: SYNODIC / 2.0 }
    }

    /// Lit fraction of the disc, 0..=1.
    pub fn illumination(&self) -> f64 {
        (1.0 - (std::f64::consts::TAU * self.age / SYNODIC).cos()) / 2.0
    }

    pub fn name(&self) -> &'static str {
        match self.age / SYNODIC * 8.0 {
            p if p < 0.5 => "New moon",
            p if p < 1.5 => "Waxing crescent",
            p if p < 2.5 => "First quarter",
            p if p < 3.5 => "Waxing gibbous",
            p if p < 4.5 => "Full moon",
            p if p < 5.5 => "Waning gibbous",
            p if p < 6.5 => "Last quarter",
            p if p < 7.5 => "Waning crescent",
            _ => "New moon",
        }
    }

    pub fn days_to_full(&self) -> f64 {
        (SYNODIC / 2.0 - self.age).rem_euclid(SYNODIC)
    }

    pub fn days_to_new(&self) -> f64 {
        SYNODIC - self.age
    }

    /// Which of the eight phases (0 = new, 4 = full).
    pub fn phase_index(&self) -> usize {
        ((self.age / SYNODIC * 8.0).round() as usize) % 8
    }

    pub fn waxing(&self) -> bool {
        self.age < SYNODIC / 2.0
    }

    /// Which side of the disc is lit, as seen from `hemisphere`.
    pub fn lit_side(&self, hemisphere: Hemisphere) -> &'static str {
        match (self.waxing(), hemisphere) {
            (true, Hemisphere::North) | (false, Hemisphere::South) => "Lit on the right",
            _ => "Lit on the left",
        }
    }

    /// Which edge is lit, in a word: "Right" or "Left".
    pub fn lit_edge(&self, hemisphere: Hemisphere) -> &'static str {
        if self.lit_side(hemisphere) == "Lit on the right" { "Right" } else { "Left" }
    }

    /// The `i`-th coming full or new moon from this one: whether it is a
    /// full moon, and in how many days.
    pub fn event(&self, i: usize) -> (bool, f64) {
        let (full, new) = (self.days_to_full(), self.days_to_new());
        let (first_full, first) = if full < new { (true, full) } else { (false, new) };
        let full_now = if i % 2 == 0 { first_full } else { !first_full };
        (full_now, first + i as f64 * SYNODIC / 2.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_follow_the_cycle() {
        let new = Moon::at(NEW_MOON_MS);
        assert_eq!((new.name(), new.phase_index()), ("New moon", 0));
        let full = Moon::at(NEW_MOON_MS + SYNODIC / 2.0 * DAY_MS);
        assert_eq!(full.name(), "Full moon");
        assert!(full.illumination() > 0.99);
        let (is_full, days) = new.event(0);
        assert!(is_full && (days - SYNODIC / 2.0).abs() < 1e-6);
        assert!(!new.event(1).0, "full and new alternate");
    }
}
