//! The texts of the weather, world clock and countdown widgets at a moment,
//! in each widget's language, digits and calendar ([`super::lingo`]).

use chrono::{DateTime, Datelike as _, Local, NaiveDate, Offset as _, Utc};
use mandara_config::desktop_widgets::{CountdownWidget, WeatherWidget, WorldClockWidget, ranges};
use mandara_i18n::tk;
use mandara_services::world_clock as wc;

use super::layout::{CityLine, CountdownStrings, WeatherStrings};
use super::lingo::{self, Lingo};
use super::{Forecast, WidgetScene};

/// Seconds per hour and minute.
const S_PER_HOUR: i32 = 3600;
const S_PER_MIN: i32 = 60;
/// What joins the parts of a line.
const DOT: &str = " · ";

/// The weather widget's texts (empty strings while there is no forecast).
pub fn weather(w: &WeatherWidget, scene: &WidgetScene, now: &DateTime<Local>) -> WeatherStrings {
    let l = Lingo::of(&w.locale, scene.rtl, None);
    let Some(f) = scene.forecast.as_ref() else {
        return WeatherStrings { icon: "cloud", temp: "—".into(), condition: l.word(tk("Weather unavailable")), rtl: l.rtl, ..WeatherStrings::default() };
    };
    let deg = |c: f32| l.digits(scene.units.format_degrees(c));
    let details = w.details.then(|| {
        let (speed, unit) = scene.units.speed(f.wind);
        [
            mandara_i18n::fill(&l.word(tk("Feels like {temp}")), &[("temp", &deg(f.feels_like))]),
            mandara_i18n::fill(&l.word(tk("Humidity {n}%")), &[("n", &l.digits(f.humidity.to_string()))]),
            l.digits(format!("{} {unit}", speed.round() as i32)),
        ]
        .join(DOT)
    });
    let today = f.timezone.map_or_else(|| now.date_naive(), |tz| now.with_timezone(&tz).date_naive());
    let days = f
        .days
        .iter()
        .filter(|(d, ..)| *d > today)
        .take(w.days() as usize)
        .map(|(d, icon, min, max)| (l.date(d, "%a"), *icon, format!("{} {}", deg(*max), deg(*min))))
        .collect();
    WeatherStrings {
        today: f.days.iter().find(|(date, ..)| *date == today).map(|(_, _, min, max)| (*min, *max)),
        icon: f.icon,
        temp: deg(f.temp),
        condition: l.word(&f.condition),
        place: if w.location { f.place.clone() } else { String::new() },
        details: details.unwrap_or_default(),
        days,
        rtl: l.rtl,
    }
}

/// `+2 h`, `−5 h 30 m` (empty when it is the same time here).
fn offset(l: &Lingo, seconds: i32) -> String {
    if seconds == 0 {
        return String::new();
    }
    let sign = if seconds > 0 { "+" } else { "−" };
    let (h, m) = (seconds.abs() / S_PER_HOUR, seconds.abs() % S_PER_HOUR / S_PER_MIN);
    let text = match m {
        0 => format!("{sign}{h} {}", l.word(tk("h"))),
        _ => format!("{sign}{h} {} {m} {}", l.word(tk("h")), l.word(tk("m"))),
    };
    l.digits(text)
}

/// The world clock's cities at `now`.
pub fn cities(w: &WorldClockWidget, scene: &WidgetScene, now: &DateTime<Local>) -> Vec<CityLine> {
    let l = Lingo::of(&w.locale, scene.rtl, None);
    let entries = if w.cities.is_empty() { &scene.world_cities } else { &w.cities };
    let utc = now.with_timezone(&Utc);
    let here = now.offset().fix().local_minus_utc();
    let mut seen = Vec::new();
    entries
        .iter()
        .filter_map(|e| wc::resolve(e))
        .filter(|p| {
            let fresh = !seen.contains(&p.key);
            seen.push(p.key.clone());
            fresh
        })
        .take(ranges::WORLD_CLOCK_CITIES)
        .map(|p| {
            let there = utc.with_timezone(&p.tz);
            let shift = there.offset().fix().local_minus_utc() - here;
            let days = (there.date_naive() - now.date_naive()).num_days();
            let mut note = Vec::new();
            if w.offsets {
                note.extend(Some(offset(&l, shift)).filter(|s| !s.is_empty()));
                match days {
                    1 => note.push(l.word(tk("Tomorrow"))),
                    -1 => note.push(l.word(tk("Yesterday"))),
                    _ => {}
                }
            }
            let name = match (l.arabic(), p.arabic) {
                (true, Some(ar)) => ar.to_string(),
                _ => p.name.clone(),
            };
            let night = p.coords.is_some_and(|(lat, lon)| wc::sky(wc::solar_altitude(lat, lon, utc)) == wc::Sky::Night);
            let time = if scene.h24 { there.format("%H:%M").to_string() } else { there.format("%-I:%M").to_string() };
            CityLine { name, time: l.digits(time), note: note.join(DOT), night }
        })
        .collect()
}

/// The day a countdown counts to: its date, else the next New Year's Day.
pub fn countdown_target(c: &CountdownWidget, today: NaiveDate) -> NaiveDate {
    NaiveDate::parse_from_str(c.date.trim(), "%Y-%m-%d").ok().unwrap_or_else(|| NaiveDate::from_ymd_opt(today.year() + 1, 1, 1).unwrap_or(today))
}

/// The countdown's texts at `now`.
pub fn countdown(c: &CountdownWidget, scene: &WidgetScene, now: &DateTime<Local>) -> CountdownStrings {
    let l = Lingo::of(&c.locale, scene.rtl, None);
    let today = now.date_naive();
    let target = countdown_target(c, today);
    let days = (target - today).num_days();
    let (label, count) = match days {
        0 => (l.word(tk("Today")), String::new()),
        d if d > 0 => (l.word(tk("Days until")), l.digits(d.to_string())),
        d if c.count_up => (l.word(tk("Days since")), l.digits((-d).to_string())),
        _ => (l.word(tk("Today")), String::new()),
    };
    let title = match c.title.trim() {
        "" if c.date.trim().is_empty() => l.word(tk("New Year")),
        t => t.to_string(),
    };
    let hijri = lingo::hijri_of(target);
    let date = if c.show_date { lingo::dated(&l, &target, tk("%-d %B %Y"), Some(hijri), DOT) } else { String::new() };
    CountdownStrings { label, count, title, date, rtl: l.rtl }
}

/// A forecast for previews and Studio's gallery: an example, never shown
/// as the real weather.
/// The preview's hours (Studio's gallery shows the widget's shape).
const SAMPLE_FIRST_HOUR: u32 = 9;
const SAMPLE_HOURS: [(&str, f32); 12] = [("sun", 19.0), ("sun", 21.0), ("sun", 23.0), ("cloud-sun", 24.0), ("cloud-sun", 25.0), ("cloud-sun", 26.0), ("cloud", 25.0), ("cloud", 24.0), ("rain", 22.0), ("rain", 21.0), ("cloud-moon", 20.0), ("cloud-moon", 19.0)];

/// Synthetic weather for labelled previews, anchored to the supplied date.
pub fn sample_forecast(today: NaiveDate) -> Forecast {
    const SAMPLE_DAYS: [(&str, f32, f32); 4] = [("sun", 19.0, 27.0), ("cloud-sun", 18.0, 25.0), ("rain", 16.0, 21.0), ("sun", 17.0, 24.0)];
    Forecast {
        icon: "cloud-sun",
        temp: 23.0,
        feels_like: 24.0,
        humidity: 58,
        wind: 14.0,
        condition: tk("Partly cloudy").to_string(),
        place: mandara_config::desktop::defaults::PLACE.to_string(),
        days: std::iter::once((today, "cloud-sun", 19.0, 26.0)).chain(SAMPLE_DAYS.iter().enumerate().map(|(i, (icon, min, max))| (today + chrono::Days::new(i as u64 + 1), *icon, *min, *max))).collect(),
        hours: SAMPLE_HOURS.iter().enumerate().map(|(i, (icon, t))| ((SAMPLE_FIRST_HOUR + i as u32) % 24, *icon, *t)).collect(),
        kind: 1,
        is_day: true,
        timezone: None,
    }
}

/// `now` at a time zone offset, for tests.
#[cfg(test)]
fn at(y: i32, m: u32, d: u32) -> DateTime<Local> {
    use chrono::TimeZone as _;
    Local.with_ymd_and_hms(y, m, d, 12, 0, 0).single().expect("a local noon")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_ranges_follow_render_time_in_the_forecast_timezone() {
        let first = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        let mut forecast = sample_forecast(first);
        forecast.timezone = Some("America/New_York".parse().unwrap());
        let mut scene = WidgetScene { forecast: Some(forecast), ..Default::default() };
        let now = |value: &str| DateTime::parse_from_rfc3339(value).unwrap().with_timezone(&Local);
        let before = now("2026-11-06T04:59:59Z");
        let after = now("2026-11-06T05:00:00Z");
        let widget = WeatherWidget::default();
        assert_eq!(weather(&widget, &scene, &before).today, Some((19.0, 26.0)));
        assert_eq!(weather(&widget, &scene, &after).today, Some((19.0, 27.0)));
        scene.forecast.as_mut().unwrap().days.retain(|(date, ..)| *date == first);
        assert_eq!(weather(&widget, &scene, &after).today, None, "yesterday's range is never reused offline");
    }

    #[test]
    fn countdowns_count_to_the_date_and_past_it() {
        let scene = WidgetScene::default();
        let c = CountdownWidget { date: "2026-12-25".into(), title: "Holiday".into(), ..Default::default() };
        let s = countdown(&c, &scene, &at(2026, 12, 20));
        assert_eq!((s.count.as_str(), s.title.as_str()), ("5", "Holiday"));
        assert_eq!(countdown(&c, &scene, &at(2026, 12, 25)).count, "", "on the day: today");
        assert_eq!(countdown(&c, &scene, &at(2026, 12, 27)).count, "2", "then days since");
        let off = CountdownWidget { count_up: false, ..c };
        assert_eq!(countdown(&off, &scene, &at(2026, 12, 27)).count, "");
        // No date: the next New Year.
        assert_eq!(countdown_target(&CountdownWidget::default(), NaiveDate::from_ymd_opt(2026, 3, 1).expect("date")), NaiveDate::from_ymd_opt(2027, 1, 1).expect("date"));
    }

    #[test]
    fn weather_texts_use_the_forecast_and_the_language() {
        let now = at(2026, 11, 1);
        let scene = WidgetScene { forecast: Some(sample_forecast(now.date_naive())), ..WidgetScene::default() };
        let w = WeatherWidget { locale: mandara_config::desktop_widgets::TextOptions { language: "ar".into(), ..Default::default() }, ..Default::default() };
        let s = weather(&w, &scene, &now);
        assert!(s.rtl && s.temp.starts_with('٢'), "{}", s.temp);
        assert_eq!(s.days.len(), w.days().min(4) as usize);
        let none = weather(&WeatherWidget::default(), &WidgetScene::default(), &now);
        assert!(none.days.is_empty() && none.place.is_empty());
    }

    #[test]
    fn world_clock_cities_resolve_and_note_their_offsets() {
        let scene = WidgetScene { world_cities: vec!["Tokyo".into(), "Tokyo".into(), "nowhere-at-all".into()], ..WidgetScene::default() };
        let list = cities(&WorldClockWidget::default(), &scene, &Local::now());
        assert_eq!(list.len(), 1, "unknown and repeated cities are left out");
        assert_eq!(list[0].time.len(), 5);
    }
}
