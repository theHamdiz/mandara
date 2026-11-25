//! Actual prayer boundaries around a day, without a temporary event list.

use chrono::NaiveDateTime;
use mandara_services::prayer::{Prayer, PrayerTimes};

type Event = (Prayer, NaiveDateTime);

/// Keeps the existing boundary fallbacks at chrono's first and last dates.
pub(super) fn surrounding_prayers(now: NaiveDateTime, days: &[PrayerTimes; 3]) -> (Event, Event) {
    let date = now.date();
    let [yesterday, today, tomorrow] = days;
    let events = date
        .pred_opt()
        .map(|date| (Prayer::Isha, date.and_time(yesterday.get(Prayer::Isha))))
        .into_iter()
        .chain(
            today
                .times
                .iter()
                .map(|(prayer, time)| (*prayer, date.and_time(*time))),
        )
        .chain(
            date.succ_opt()
                .map(|date| (Prayer::Fajr, date.and_time(tomorrow.get(Prayer::Fajr)))),
        );
    let next = events
        .clone()
        .find(|(prayer, time)| prayer.is_prayer() && *time > now)
        .unwrap_or((Prayer::Fajr, now));
    let prev = events
        .rev()
        .find(|(prayer, time)| prayer.is_prayer() && *time <= now)
        .unwrap_or((Prayer::Isha, now));
    (prev, next)
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, NaiveTime};

    use super::*;

    fn day(date: NaiveDate) -> PrayerTimes {
        PrayerTimes {
            date,
            times: [
                Prayer::Fajr,
                Prayer::Sunrise,
                Prayer::Dhuhr,
                Prayer::Asr,
                Prayer::Maghrib,
                Prayer::Isha,
            ]
            .map(|prayer| {
                let hour = match prayer {
                    Prayer::Fajr => 5,
                    Prayer::Sunrise => 6,
                    Prayer::Dhuhr => 12,
                    Prayer::Asr => 15,
                    Prayer::Maghrib => 18,
                    Prayer::Isha => 20,
                };
                (prayer, NaiveTime::from_hms_opt(hour, 0, 0).unwrap())
            }),
        }
    }

    /// The iterator matches the previous materialized timeline on every boundary.
    #[test]
    fn prayer_boundaries_match_at_midnight_sunrise_and_calendar_limits() {
        for date in [
            NaiveDate::MIN,
            NaiveDate::from_ymd_opt(2026, 10, 27).unwrap(),
            NaiveDate::MAX,
        ] {
            let days = [
                day(date.pred_opt().unwrap_or(date)),
                day(date),
                day(date.succ_opt().unwrap_or(date)),
            ];
            let mut old = Vec::with_capacity(8);
            if let Some(previous) = date.pred_opt() {
                old.push((Prayer::Isha, previous.and_time(days[0].get(Prayer::Isha))));
            }
            old.extend(
                days[1]
                    .times
                    .map(|(prayer, time)| (prayer, date.and_time(time))),
            );
            if let Some(next) = date.succ_opt() {
                old.push((Prayer::Fajr, next.and_time(days[2].get(Prayer::Fajr))));
            }
            for hour in 0..24 {
                for second in [0, 1, 59] {
                    let now = date.and_hms_opt(hour, 0, second).unwrap();
                    let expected_next = old
                        .iter()
                        .copied()
                        .find(|(prayer, time)| prayer.is_prayer() && *time > now)
                        .unwrap_or((Prayer::Fajr, now));
                    let expected_prev = old
                        .iter()
                        .copied()
                        .rev()
                        .find(|(prayer, time)| prayer.is_prayer() && *time <= now)
                        .unwrap_or((Prayer::Isha, now));
                    assert_eq!(
                        surrounding_prayers(now, &days),
                        (expected_prev, expected_next),
                        "{now}"
                    );
                }
            }
        }
    }
}
