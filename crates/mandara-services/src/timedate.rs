//! The system clock's time zone and network time, through systemd's
//! `timedated` (`org.freedesktop.timedate1`, system bus). Changes ask for
//! permission through polkit (`interactive`), which Mandara's own polkit
//! agent shows; Mandara never writes `/etc/localtime` itself.
//!
//! Mandara only shows the system clock, so a wrong time zone shows up
//! everywhere (bar, lock screen, prayer times). [`mismatch`] compares it
//! with the time zone of the shared location, so Studio can offer the fix.

use chrono::{DateTime, Offset as _, TimeZone as _, Utc};
use serde::Serialize;

const TIMEDATE: &str = "org.freedesktop.timedate1";
const TIMEDATE_PATH: &str = "/org/freedesktop/timedate1";

/// What the system clock is set to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct TimeStatus {
    /// IANA name, e.g. `Africa/Cairo`.
    pub timezone: String,
    /// Network time (NTP) is switched on.
    pub ntp: bool,
    /// A network time service is installed (systemd-timesyncd, chrony…).
    pub can_ntp: bool,
    /// The clock is synchronized with a time server.
    pub synced: bool,
}

fn fail(e: zbus::Error) -> String {
    format!("timedated: {e}")
}

async fn timedate() -> Result<zbus::Proxy<'static>, String> {
    let conn = crate::runtime::system().await.map_err(fail)?;
    crate::actor::proxy(&conn, TIMEDATE, TIMEDATE_PATH, TIMEDATE).await.map_err(fail)
}

/// The clock's time zone and network-time state.
pub async fn status() -> Result<TimeStatus, String> {
    let p = timedate().await?;
    Ok(TimeStatus {
        timezone: p.get_property("Timezone").await.map_err(fail)?,
        ntp: p.get_property("NTP").await.unwrap_or(false),
        can_ntp: p.get_property("CanNTP").await.unwrap_or(false),
        synced: p.get_property("NTPSynchronized").await.unwrap_or(false),
    })
}

/// Sets the system time zone (asks for permission).
pub async fn set_timezone(zone: &str) -> Result<(), String> {
    if !is_zone(zone) {
        return Err(mandara_i18n::tf("“{zone}” isn't a time zone", &[("zone", &zone)]));
    }
    timedate().await?.call::<_, _, ()>("SetTimezone", &(zone, true)).await.map_err(fail)
}

/// Switches network time on or off (asks for permission).
pub async fn set_ntp(on: bool) -> Result<(), String> {
    timedate().await?.call::<_, _, ()>("SetNTP", &(on, true)).await.map_err(fail)
}

/// Every time zone the system knows, sorted (`ListTimezones`, else the
/// built-in database).
pub async fn zones() -> Result<Vec<String>, String> {
    let mut list: Vec<String> = match timedate().await {
        Ok(p) => p.call("ListTimezones", &()).await.unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    if list.is_empty() {
        list = chrono_tz::TZ_VARIANTS.iter().map(|tz| tz.name().to_string()).collect();
    }
    list.sort();
    Ok(list)
}

/// Whether `zone` is an IANA time zone name.
pub fn is_zone(zone: &str) -> bool {
    zone.parse::<chrono_tz::Tz>().is_ok()
}

/// Seconds east of UTC that `zone` is at `at`.
pub fn offset_seconds(zone: &str, at: DateTime<Utc>) -> Option<i32> {
    let tz: chrono_tz::Tz = zone.parse().ok()?;
    Some(tz.offset_from_utc_datetime(&at.naive_utc()).fix().local_minus_utc())
}

/// `UTC+03:00` for an offset in seconds.
pub fn offset_label(seconds: i32) -> String {
    const SECS_PER_MIN: i32 = 60;
    const MINS_PER_HOUR: i32 = 60;
    let sign = if seconds < 0 { '-' } else { '+' };
    let minutes = seconds.abs() / SECS_PER_MIN;
    format!("UTC{sign}{:02}:{:02}", minutes / MINS_PER_HOUR, minutes % MINS_PER_HOUR)
}

/// The location's time zone when the clock shows, or within the coming
/// year will show, a different time there: Riyadh's clock in Cairo agrees
/// in summer but is an hour off in winter. Zones that keep the same time
/// all year (`Asia/Riyadh` / `Asia/Baghdad`) are fine. `None` when they
/// agree or either is unknown.
pub fn mismatch<'a>(system: &str, location: &'a str, at: DateTime<Utc>) -> Option<&'a str> {
    /// Samples a month apart, a year ahead (every daylight-saving rule).
    const SAMPLES: i64 = 13;
    const DAYS_APART: i64 = 30;
    if location.is_empty() || system == location {
        return None;
    }
    for k in 0..SAMPLES {
        let when = at + chrono::Duration::days(k * DAYS_APART);
        if offset_seconds(system, when)? != offset_seconds(location, when)? {
            return Some(location);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn offsets_follow_daylight_saving() {
        // Egypt's summer time ends at midnight on 2026-10-29/30.
        assert_eq!(offset_seconds("Africa/Cairo", at("2026-10-29T12:00:00Z")), Some(3 * 3600));
        assert_eq!(offset_seconds("Africa/Cairo", at("2026-10-30T12:00:00Z")), Some(2 * 3600));
        assert_eq!(offset_seconds("Asia/Riyadh", at("2026-10-30T12:00:00Z")), Some(3 * 3600));
        assert_eq!(offset_seconds("Not/AZone", at("2026-10-30T12:00:00Z")), None);
        assert_eq!(offset_label(3 * 3600), "UTC+03:00");
        assert_eq!(offset_label(-(5 * 3600 + 30 * 60)), "UTC-05:30");
    }

    #[test]
    fn a_different_clock_is_a_mismatch_but_an_alias_is_not() {
        // The case from the report: Riyadh's clock in Alexandria, an hour
        // ahead once Egypt's summer time ended (but not before).
        let after = at("2026-10-30T12:00:00Z");
        assert_eq!(mismatch("Asia/Riyadh", "Africa/Cairo", after), Some("Africa/Cairo"));
        // In summer they agree now, but not this winter: still a mismatch.
        assert_eq!(mismatch("Asia/Riyadh", "Africa/Cairo", at("2026-07-01T12:00:00Z")), Some("Africa/Cairo"));
        assert_eq!(mismatch("Asia/Riyadh", "Asia/Baghdad", after), None);
        assert_eq!(mismatch("Africa/Cairo", "Africa/Cairo", after), None);
        assert_eq!(mismatch("Africa/Cairo", "", after), None);
        assert!(is_zone("Africa/Cairo") && !is_zone("Cairo"));
    }

    /// Reads the real clock over the system bus (read-only; run with
    /// `cargo test -p mandara-services timedate -- --ignored`).
    #[test]
    #[ignore = "needs systemd-timedated on the system bus"]
    fn reads_the_real_clock() {
        let s = crate::runtime::block_on(status()).unwrap();
        assert!(is_zone(&s.timezone), "{s:?}");
        let zones = crate::runtime::block_on(zones()).unwrap();
        assert!(zones.iter().any(|z| z == "Africa/Cairo"));
        eprintln!("{s:?}, {} zones", zones.len());
    }
}
