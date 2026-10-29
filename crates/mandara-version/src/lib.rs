//! Mandara's build identity. Every workspace crate and plugin is a
//! *component* with its own semantic version; `versions.toml` records the
//! content hash each version was released with. Builds embed:
//!
//! - each component's version — or, when its sources changed since that
//!   version was recorded, the next patch as a `-dev` pre-release;
//! - its content hash, the git revision (with a dirty flag), target,
//!   profile and build time;
//! - for builds made by `cargo xtask build`, the **source fingerprint**
//!   (SHA-256 over every build input, toolchain, target, profile and
//!   features); other builds show `dev` instead.
//!
//! `cargo xtask bump` turns changed components into real releases (semver
//! from Conventional Commits) and records their new hashes.

pub mod manifest;

/// One component's identity in this build.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Component {
    pub name: &'static str,
    /// `binary`, `library`, `plugin` or `tool`.
    pub kind: &'static str,
    pub version: &'static str,
    /// Content hash (hex, 128-bit) of the component and its dependencies.
    pub hash: &'static str,
    /// Sources differ from the recorded release.
    pub changed: bool,
}

include!(concat!(env!("OUT_DIR"), "/build_info.rs"));

/// A component by name (`mandara-app`, `mandara-ui`, `plugin-moon-phase`, …).
pub fn component(name: &str) -> Option<&'static Component> {
    COMPONENTS.iter().find(|c| c.name == name)
}

/// The shell's version (the `mandara-app` component).
pub fn version() -> &'static str {
    component("mandara-app").map(|c| c.version).unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// Hex digits of the content hash shown in the long version.
const CONTENT_HASH_SHOWN: usize = 12;

/// The short source fingerprint (`3f9c2a1b7e40`), or `dev` for builds not
/// made by `cargo xtask build`.
pub fn source_fingerprint() -> &'static str {
    if SOURCE_FINGERPRINT.is_empty() { "dev" } else { &SOURCE_FINGERPRINT[..manifest::FINGERPRINT_SHORT_LEN.min(SOURCE_FINGERPRINT.len())] }
}

/// `days` since 1970-01-01 → (year, month, day), proleptic Gregorian.
/// Howard Hinnant's `civil_from_days`: the constants are the calendar's
/// (a 400-year era of 146 097 days, March-based years so leap days fall
/// last, 153-day five-month cycles).
pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
    /// Days from 0000-03-01 to 1970-01-01.
    const EPOCH_SHIFT: i64 = 719_468;
    const DAYS_PER_ERA: i64 = 146_097;
    const YEARS_PER_ERA: i64 = 400;
    const DAYS_PER_4_YEARS: i64 = 1460;
    const DAYS_PER_CENTURY: i64 = 36_524;
    const DAYS_PER_YEAR: i64 = 365;
    const LEAP_EVERY: i64 = 4;
    const CENTURY: i64 = 100;
    /// Days in the five-month (Mar–Jul / Aug–Dec) cycle.
    const DAYS_PER_5_MONTHS: i64 = 153;
    const MONTHS_PER_CYCLE: i64 = 5;
    /// Month index of January in a March-based year.
    const JANUARY_INDEX: i64 = 10;
    const MARCH: i64 = 3;
    const MONTHS_PER_YEAR: i64 = 12;
    let z = days + EPOCH_SHIFT;
    let era = z.div_euclid(DAYS_PER_ERA);
    let doe = z - era * DAYS_PER_ERA;
    let yoe = (doe - doe / DAYS_PER_4_YEARS + doe / DAYS_PER_CENTURY - doe / (DAYS_PER_ERA - 1)) / DAYS_PER_YEAR;
    let doy = doe - (DAYS_PER_YEAR * yoe + yoe / LEAP_EVERY - yoe / CENTURY);
    let mp = (MONTHS_PER_CYCLE * doy + 2) / DAYS_PER_5_MONTHS;
    let d = doy - (DAYS_PER_5_MONTHS * mp + 2) / MONTHS_PER_CYCLE + 1;
    let m = if mp < JANUARY_INDEX { mp + MARCH } else { mp + MARCH - MONTHS_PER_YEAR };
    let y = yoe + era * YEARS_PER_ERA + i64::from(m <= 2);
    (y, m, d)
}

/// `0.4.2 (fingerprint 5d41402abc4b, 3f9c2a1b7e40, content 9f8e17ab4c21,
/// 2026-10-27, x86_64-unknown-linux-gnu)`.
pub fn long_version() -> String {
    /// Seconds per day.
    const SECS_PER_DAY: i64 = 86_400;
    let app = component("mandara-app");
    let date = {
        let (y, m, d) = civil_from_days(BUILD_UNIX.div_euclid(SECS_PER_DAY));
        format!("{y:04}-{m:02}-{d:02}")
    };
    format!(
        "{} (fingerprint {}, {}{}, content {}, {date}, {}{})",
        version(),
        source_fingerprint(),
        GIT_SHA,
        if GIT_DIRTY { "-dirty" } else { "" },
        app.map(|a| &a.hash[..CONTENT_HASH_SHOWN.min(a.hash.len())]).unwrap_or("?"),
        TARGET,
        if PROFILE == "release" { "" } else { ", debug" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_every_workspace_crate() {
        for name in ["mandara-app", "mandara-ui", "mandara-core", "mandara-version"] {
            let c = component(name).unwrap_or_else(|| panic!("{name} missing"));
            assert_eq!(c.hash.len(), 32);
            assert!(!c.version.is_empty());
        }
        assert!(long_version().contains(GIT_SHA));
        assert!(long_version().contains(&format!("fingerprint {}", source_fingerprint())));
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2000-02-29 (leap day of a century leap year) and 2026-10-27.
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(20_753), (2026, 10, 27));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }
}
