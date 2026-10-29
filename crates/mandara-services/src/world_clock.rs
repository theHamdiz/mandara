//! World clock logic: a curated city list (English + Arabic names, IATA-ish
//! codes, coordinates) on top of the IANA time zone database (`chrono-tz`),
//! city search across both, UTC offsets relative to the local zone, and the
//! sun's position (NOAA solar equations) for day / night indicators.
//!
//! Pure functions only — the UI decides when to evaluate them (never while
//! the panel and bar chip are hidden).

use chrono::{DateTime, Datelike, Duration, NaiveDate, Offset, TimeZone, Timelike, Utc};
pub use chrono_tz::Tz;

/// A curated city: display names, a short code for the bar, its zone and
/// coordinates (for sunrise / sunset).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct City {
    pub name: &'static str,
    pub arabic: &'static str,
    pub code: &'static str,
    pub zone: &'static str,
    pub lat: f64,
    pub lon: f64,
}

const fn c(name: &'static str, arabic: &'static str, code: &'static str, zone: &'static str, lat: f64, lon: f64) -> City {
    City { name, arabic, code, zone, lat, lon }
}

/// Cities people look up most, plus the Arab world in depth.
pub const CITIES: &[City] = &[
    c("Alexandria", "الإسكندرية", "ALY", "Africa/Cairo", 31.2001, 29.9187),
    c("Cairo", "القاهرة", "CAI", "Africa/Cairo", 30.0444, 31.2357),
    c("Mecca", "مكة المكرمة", "MEC", "Asia/Riyadh", 21.4225, 39.8262),
    c("Medina", "المدينة المنورة", "MED", "Asia/Riyadh", 24.4686, 39.6142),
    c("Riyadh", "الرياض", "RUH", "Asia/Riyadh", 24.7136, 46.6753),
    c("Jeddah", "جدة", "JED", "Asia/Riyadh", 21.4858, 39.1925),
    c("Dubai", "دبي", "DXB", "Asia/Dubai", 25.2048, 55.2708),
    c("Abu Dhabi", "أبوظبي", "AUH", "Asia/Dubai", 24.4539, 54.3773),
    c("Doha", "الدوحة", "DOH", "Asia/Qatar", 25.2854, 51.5310),
    c("Kuwait City", "الكويت", "KWI", "Asia/Kuwait", 29.3759, 47.9774),
    c("Manama", "المنامة", "BAH", "Asia/Bahrain", 26.2285, 50.5860),
    c("Muscat", "مسقط", "MCT", "Asia/Muscat", 23.5880, 58.3829),
    c("Amman", "عمّان", "AMM", "Asia/Amman", 31.9454, 35.9284),
    c("Jerusalem", "القدس", "JRS", "Asia/Jerusalem", 31.7683, 35.2137),
    c("Gaza", "غزة", "GZA", "Asia/Gaza", 31.5017, 34.4668),
    c("Beirut", "بيروت", "BEY", "Asia/Beirut", 33.8938, 35.5018),
    c("Damascus", "دمشق", "DAM", "Asia/Damascus", 33.5138, 36.2765),
    c("Baghdad", "بغداد", "BGW", "Asia/Baghdad", 33.3152, 44.3661),
    c("Sana'a", "صنعاء", "SAH", "Asia/Aden", 15.3694, 44.1910),
    c("Tehran", "طهران", "THR", "Asia/Tehran", 35.6892, 51.3890),
    c("Istanbul", "إسطنبول", "IST", "Europe/Istanbul", 41.0082, 28.9784),
    c("Ankara", "أنقرة", "ANK", "Europe/Istanbul", 39.9334, 32.8597),
    c("Khartoum", "الخرطوم", "KRT", "Africa/Khartoum", 15.5007, 32.5599),
    c("Tripoli", "طرابلس", "TIP", "Africa/Tripoli", 32.8872, 13.1913),
    c("Tunis", "تونس", "TUN", "Africa/Tunis", 36.8065, 10.1815),
    c("Algiers", "الجزائر", "ALG", "Africa/Algiers", 36.7538, 3.0588),
    c("Casablanca", "الدار البيضاء", "CMN", "Africa/Casablanca", 33.5731, -7.5898),
    c("Rabat", "الرباط", "RBA", "Africa/Casablanca", 34.0209, -6.8416),
    c("Lagos", "لاغوس", "LOS", "Africa/Lagos", 6.5244, 3.3792),
    c("Nairobi", "نيروبي", "NBO", "Africa/Nairobi", -1.2921, 36.8219),
    c("Addis Ababa", "أديس أبابا", "ADD", "Africa/Addis_Ababa", 9.0300, 38.7400),
    c("Johannesburg", "جوهانسبرغ", "JNB", "Africa/Johannesburg", -26.2041, 28.0473),
    c("Cape Town", "كيب تاون", "CPT", "Africa/Johannesburg", -33.9249, 18.4241),
    c("London", "لندن", "LON", "Europe/London", 51.5074, -0.1278),
    c("Dublin", "دبلن", "DUB", "Europe/Dublin", 53.3498, -6.2603),
    c("Lisbon", "لشبونة", "LIS", "Europe/Lisbon", 38.7223, -9.1393),
    c("Madrid", "مدريد", "MAD", "Europe/Madrid", 40.4168, -3.7038),
    c("Paris", "باريس", "PAR", "Europe/Paris", 48.8566, 2.3522),
    c("Brussels", "بروكسل", "BRU", "Europe/Brussels", 50.8503, 4.3517),
    c("Amsterdam", "أمستردام", "AMS", "Europe/Amsterdam", 52.3676, 4.9041),
    c("Berlin", "برلين", "BER", "Europe/Berlin", 52.5200, 13.4050),
    c("Zurich", "زيورخ", "ZRH", "Europe/Zurich", 47.3769, 8.5417),
    c("Rome", "روما", "ROM", "Europe/Rome", 41.9028, 12.4964),
    c("Vienna", "فيينا", "VIE", "Europe/Vienna", 48.2082, 16.3738),
    c("Prague", "براغ", "PRG", "Europe/Prague", 50.0755, 14.4378),
    c("Warsaw", "وارسو", "WAW", "Europe/Warsaw", 52.2297, 21.0122),
    c("Copenhagen", "كوبنهاغن", "CPH", "Europe/Copenhagen", 55.6761, 12.5683),
    c("Oslo", "أوسلو", "OSL", "Europe/Oslo", 59.9139, 10.7522),
    c("Stockholm", "ستوكهولم", "STO", "Europe/Stockholm", 59.3293, 18.0686),
    c("Helsinki", "هلسنكي", "HEL", "Europe/Helsinki", 60.1699, 24.9384),
    c("Athens", "أثينا", "ATH", "Europe/Athens", 37.9838, 23.7275),
    c("Kyiv", "كييف", "KBP", "Europe/Kyiv", 50.4501, 30.5234),
    c("Moscow", "موسكو", "MOW", "Europe/Moscow", 55.7558, 37.6173),
    c("Reykjavik", "ريكيافيك", "REK", "Atlantic/Reykjavik", 64.1466, -21.9426),
    c("New York", "نيويورك", "NYC", "America/New_York", 40.7128, -74.0060),
    c("Washington", "واشنطن", "WAS", "America/New_York", 38.9072, -77.0369),
    c("Toronto", "تورونتو", "YTO", "America/Toronto", 43.6532, -79.3832),
    c("Chicago", "شيكاغو", "CHI", "America/Chicago", 41.8781, -87.6298),
    c("Denver", "دنفر", "DEN", "America/Denver", 39.7392, -104.9903),
    c("Los Angeles", "لوس أنجلوس", "LAX", "America/Los_Angeles", 34.0522, -118.2437),
    c("San Francisco", "سان فرانسيسكو", "SFO", "America/Los_Angeles", 37.7749, -122.4194),
    c("Seattle", "سياتل", "SEA", "America/Los_Angeles", 47.6062, -122.3321),
    c("Vancouver", "فانكوفر", "YVR", "America/Vancouver", 49.2827, -123.1207),
    c("Anchorage", "أنكوريج", "ANC", "America/Anchorage", 61.2181, -149.9003),
    c("Honolulu", "هونولولو", "HNL", "Pacific/Honolulu", 21.3069, -157.8583),
    c("Mexico City", "مكسيكو سيتي", "MEX", "America/Mexico_City", 19.4326, -99.1332),
    c("Bogotá", "بوغوتا", "BOG", "America/Bogota", 4.7110, -74.0721),
    c("Lima", "ليما", "LIM", "America/Lima", -12.0464, -77.0428),
    c("Santiago", "سانتياغو", "SCL", "America/Santiago", -33.4489, -70.6693),
    c("São Paulo", "ساو باولو", "SAO", "America/Sao_Paulo", -23.5505, -46.6333),
    c("Rio de Janeiro", "ريو دي جانيرو", "RIO", "America/Sao_Paulo", -22.9068, -43.1729),
    c("Buenos Aires", "بوينس آيرس", "BUE", "America/Argentina/Buenos_Aires", -34.6037, -58.3816),
    c("Karachi", "كراتشي", "KHI", "Asia/Karachi", 24.8607, 67.0011),
    c("Islamabad", "إسلام آباد", "ISB", "Asia/Karachi", 33.6844, 73.0479),
    c("Delhi", "دلهي", "DEL", "Asia/Kolkata", 28.6139, 77.2090),
    c("Mumbai", "مومباي", "BOM", "Asia/Kolkata", 19.0760, 72.8777),
    c("Bengaluru", "بنغالور", "BLR", "Asia/Kolkata", 12.9716, 77.5946),
    c("Kathmandu", "كاتماندو", "KTM", "Asia/Kathmandu", 27.7172, 85.3240),
    c("Dhaka", "دكا", "DAC", "Asia/Dhaka", 23.8103, 90.4125),
    c("Bangkok", "بانكوك", "BKK", "Asia/Bangkok", 13.7563, 100.5018),
    c("Kuala Lumpur", "كوالالمبور", "KUL", "Asia/Kuala_Lumpur", 3.1390, 101.6869),
    c("Singapore", "سنغافورة", "SIN", "Asia/Singapore", 1.3521, 103.8198),
    c("Jakarta", "جاكرتا", "JKT", "Asia/Jakarta", -6.2088, 106.8456),
    c("Manila", "مانيلا", "MNL", "Asia/Manila", 14.5995, 120.9842),
    c("Hong Kong", "هونغ كونغ", "HKG", "Asia/Hong_Kong", 22.3193, 114.1694),
    c("Shanghai", "شنغهاي", "SHA", "Asia/Shanghai", 31.2304, 121.4737),
    c("Beijing", "بكين", "BJS", "Asia/Shanghai", 39.9042, 116.4074),
    c("Taipei", "تايبيه", "TPE", "Asia/Taipei", 25.0330, 121.5654),
    c("Seoul", "سيول", "SEL", "Asia/Seoul", 37.5665, 126.9780),
    c("Tokyo", "طوكيو", "TYO", "Asia/Tokyo", 35.6762, 139.6503),
    c("Perth", "بيرث", "PER", "Australia/Perth", -31.9505, 115.8605),
    c("Melbourne", "ملبورن", "MEL", "Australia/Melbourne", -37.8136, 144.9631),
    c("Sydney", "سيدني", "SYD", "Australia/Sydney", -33.8688, 151.2093),
    c("Auckland", "أوكلاند", "AKL", "Pacific/Auckland", -36.8485, 174.7633),
];

/// Cities shown until the user picks their own.
pub const DEFAULT_CITIES: &[&str] = &["Alexandria", "London", "New York", "Tokyo"];

/// Most search results returned.
pub const MAX_RESULTS: usize = 8;

/// Default working day (local hours of each city).
pub const WORK_START_HOUR: u32 = 9;
pub const WORK_END_HOUR: u32 = 17;

/// Sun altitude at sunrise / sunset (refraction + solar radius), degrees.
const SUNRISE_ALTITUDE: f64 = -0.833;
/// Civil twilight ends at this altitude, degrees.
pub const CIVIL_TWILIGHT: f64 = -6.0;
/// Julian date of the Unix epoch and of J2000.0.
const JD_UNIX_EPOCH: f64 = 2_440_587.5;
const JD_J2000: f64 = 2_451_545.0;
const SECS_PER_DAY: f64 = 86_400.0;

/// A place on the clock: a curated city or a bare IANA zone.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    /// The config entry that produced it (city name or zone id).
    pub key: String,
    pub name: String,
    pub arabic: Option<&'static str>,
    pub code: String,
    pub tz: Tz,
    /// Latitude / longitude when known (curated cities).
    pub coords: Option<(f64, f64)>,
}

impl Place {
    fn from_city(city: &City) -> Option<Self> {
        Some(Self {
            key: city.name.to_string(),
            name: city.name.to_string(),
            arabic: Some(city.arabic),
            code: city.code.to_string(),
            tz: city.zone.parse().ok()?,
            coords: Some((city.lat, city.lon)),
        })
    }

    fn from_zone(id: &str) -> Option<Self> {
        let tz: Tz = id.parse().ok()?;
        let name = zone_display_name(tz.name());
        Some(Self { key: tz.name().to_string(), code: short_code(&name), name, arabic: None, tz, coords: None })
    }

    /// The local time here at `at`.
    pub fn time(&self, at: DateTime<Utc>) -> DateTime<Tz> {
        at.with_timezone(&self.tz)
    }

    /// Seconds east of UTC at `at`.
    pub fn utc_offset(&self, at: DateTime<Utc>) -> i32 {
        self.time(at).offset().fix().local_minus_utc()
    }
}

/// `America/Argentina/Buenos_Aires` → `Buenos Aires`.
pub fn zone_display_name(id: &str) -> String {
    id.rsplit('/').next().unwrap_or(id).replace('_', " ")
}

/// Three-letter bar code from a name (`Lisbon` → `LIS`).
pub fn short_code(name: &str) -> String {
    let letters: String = name.chars().filter(|c| c.is_alphabetic()).take(3).collect();
    letters.to_uppercase()
}

/// Resolves a config entry: a curated city (English or Arabic name, or
/// code) or an IANA zone id (`Europe/Lisbon`, `UTC`).
pub fn resolve(entry: &str) -> Option<Place> {
    let e = entry.trim();
    if e.is_empty() {
        return None;
    }
    let folded = fold(e);
    if let Some(city) = CITIES.iter().find(|c| fold(c.name) == folded || fold(c.arabic) == folded || c.code.eq_ignore_ascii_case(e)) {
        return Place::from_city(city);
    }
    // Zone ids, case-insensitively (`europe/lisbon`).
    let zone = Place::from_zone(e).or_else(|| chrono_tz::TZ_VARIANTS.iter().find(|tz| tz.name().eq_ignore_ascii_case(e)).and_then(|tz| Place::from_zone(tz.name())))?;
    // A zone named after a curated city gets its coordinates.
    match CITIES.iter().find(|c| c.zone == zone.tz.name() && fold(c.name) == fold(&zone.name)) {
        Some(city) => Place::from_city(city).map(|p| Place { key: zone.key, ..p }),
        None => Some(zone),
    }
}

/// Lowercases and strips accents / Arabic diacritics and letter variants so
/// `sao paulo` finds `São Paulo` and `مكه` finds `مكة`.
pub fn fold(s: &str) -> String {
    s.chars()
        .filter_map(|ch| {
            Some(match ch {
                // Arabic short vowels, shadda, sukun, superscript alef, tatweel.
                '\u{064B}'..='\u{0652}' | '\u{0670}' | '\u{0640}' => return None,
                'أ' | 'إ' | 'آ' | 'ٱ' => 'ا',
                'ة' => 'ه',
                'ى' => 'ي',
                'á' | 'à' | 'â' | 'ä' | 'ã' | 'Á' | 'À' | 'Â' | 'Ä' | 'Ã' => 'a',
                'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'e',
                'í' | 'ì' | 'î' | 'ï' | 'Í' | 'Î' => 'i',
                'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'Ó' | 'Ô' | 'Ö' | 'Õ' => 'o',
                'ú' | 'ù' | 'û' | 'ü' | 'Ú' | 'Ü' => 'u',
                'ç' | 'Ç' => 'c',
                'ñ' | 'Ñ' => 'n',
                '_' | '\'' | '’' => ' ',
                other => other.to_lowercase().next().unwrap_or(other),
            })
        })
        .collect()
}

/// A search hit.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    /// What to store in the cities list.
    pub key: String,
    pub name: String,
    pub arabic: Option<&'static str>,
    /// The IANA zone id.
    pub zone: String,
}

/// Searches curated cities (English / Arabic names, codes) first, then the
/// whole tz database (`lisbon`, `Europe/`, `kolkata`). Prefix matches rank
/// above substring matches.
pub fn search(query: &str) -> Vec<Match> {
    let q = fold(query.trim());
    if q.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(u8, Match)> = Vec::new();
    for city in CITIES {
        let (name, arabic, zone) = (fold(city.name), fold(city.arabic), fold(city.zone));
        let rank = if name.starts_with(&q) || arabic.starts_with(&q) || city.code.eq_ignore_ascii_case(query.trim()) {
            0
        } else if name.contains(&q) || arabic.contains(&q) {
            1
        } else if zone.contains(&q) {
            3
        } else {
            continue;
        };
        scored.push((rank, Match { key: city.name.into(), name: city.name.into(), arabic: Some(city.arabic), zone: city.zone.into() }));
    }
    for tz in chrono_tz::TZ_VARIANTS.iter() {
        let id = tz.name();
        // Skip deprecated aliases (`US/Eastern`, `Etc/GMT+3` is still useful).
        if !id.contains('/') && id != "UTC" {
            continue;
        }
        let name = zone_display_name(id);
        let (fname, fid) = (fold(&name), fold(id));
        let rank = if fname.starts_with(&q) {
            2
        } else if fid.contains(&q) {
            4
        } else {
            continue;
        };
        // A curated city already covers this zone under the same name.
        if scored.iter().any(|(_, m)| fold(&m.name) == fname) {
            continue;
        }
        scored.push((rank, Match { key: id.into(), name, arabic: None, zone: id.into() }));
    }
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    scored.into_iter().map(|(_, m)| m).take(MAX_RESULTS).collect()
}

/// `+3 h`, `−5 h 30 m`, `same time` style offset of `seconds` vs local,
/// as `(sign, hours, minutes)`.
pub fn split_offset(seconds: i32) -> (i8, u32, u32) {
    let sign = seconds.signum() as i8;
    let abs = seconds.unsigned_abs();
    (sign, abs / 3600, (abs % 3600) / 60)
}

/// Calendar-day difference between a remote local date and the local date
/// (`-1` yesterday, `1` tomorrow).
pub fn day_delta(remote: NaiveDate, local: NaiveDate) -> i64 {
    (remote - local).num_days()
}

/// A formatted clock reading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockText {
    pub time: String,
    /// `AM` / `PM` in 12-hour mode.
    pub meridiem: Option<&'static str>,
}

pub fn format_time(hour: u32, minute: u32, h24: bool) -> ClockText {
    if h24 {
        ClockText { time: format!("{hour:02}:{minute:02}"), meridiem: None }
    } else {
        let h12 = match hour % 12 {
            0 => 12,
            h => h,
        };
        ClockText { time: format!("{h12}:{minute:02}"), meridiem: Some(if hour < 12 { "AM" } else { "PM" }) }
    }
}

/// Whether a local hour falls in the working day `[start, end)`.
pub fn is_working_hour(hour: u32, start: u32, end: u32) -> bool {
    if start <= end { (start..end).contains(&hour) } else { hour >= start || hour < end }
}

// ── Sun position (NOAA / Astronomical Almanac low-precision formulas) ────

fn julian_day(at: DateTime<Utc>) -> f64 {
    at.timestamp() as f64 / SECS_PER_DAY + JD_UNIX_EPOCH + at.timestamp_subsec_millis() as f64 / 1000.0 / SECS_PER_DAY
}

/// Solar declination (radians) and equation of time (minutes) at `at`.
fn solar_coordinates(at: DateTime<Utc>) -> (f64, f64) {
    let n = julian_day(at) - JD_J2000;
    let mean_long = (280.460 + 0.985_647_4 * n).rem_euclid(360.0);
    let anomaly = (357.528 + 0.985_600_3 * n).rem_euclid(360.0).to_radians();
    let ecliptic = (mean_long + 1.915 * anomaly.sin() + 0.020 * (2.0 * anomaly).sin()).to_radians();
    let obliquity = (23.439 - 0.000_000_4 * n).to_radians();
    let declination = (obliquity.sin() * ecliptic.sin()).asin();
    let right_ascension = (obliquity.cos() * ecliptic.sin()).atan2(ecliptic.cos()).to_degrees().rem_euclid(360.0);
    let mut eot_deg = mean_long - right_ascension;
    if eot_deg > 180.0 {
        eot_deg -= 360.0;
    } else if eot_deg < -180.0 {
        eot_deg += 360.0;
    }
    (declination, eot_deg * 4.0)
}

/// The sun's altitude above the horizon (degrees) at `at` for `lat`/`lon`.
pub fn solar_altitude(lat: f64, lon: f64, at: DateTime<Utc>) -> f64 {
    let (decl, eot_min) = solar_coordinates(at);
    let utc_hours = at.hour() as f64 + at.minute() as f64 / 60.0 + at.second() as f64 / 3600.0;
    let solar_time = utc_hours + lon / 15.0 + eot_min / 60.0;
    let hour_angle = (15.0 * (solar_time - 12.0)).to_radians();
    let phi = lat.to_radians();
    (phi.sin() * decl.sin() + phi.cos() * decl.cos() * hour_angle.cos()).clamp(-1.0, 1.0).asin().to_degrees()
}

/// Daylight at a location on a day.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Daylight {
    Normal { sunrise: DateTime<Utc>, sunset: DateTime<Utc> },
    /// The sun never sets (polar summer).
    PolarDay,
    /// The sun never rises (polar winter).
    PolarNight,
}

/// Sunrise / sunset (UTC) for the UTC calendar day of `date` at `lat`/`lon`.
pub fn daylight(lat: f64, lon: f64, date: NaiveDate) -> Daylight {
    let noon_guess = Utc.with_ymd_and_hms(date.year(), date.month(), date.day(), 12, 0, 0).single().unwrap_or_else(Utc::now);
    let (decl, eot_min) = solar_coordinates(noon_guess);
    let phi = lat.to_radians();
    let cos_h0 = (SUNRISE_ALTITUDE.to_radians().sin() - phi.sin() * decl.sin()) / (phi.cos() * decl.cos());
    if cos_h0 < -1.0 {
        return Daylight::PolarDay;
    }
    if cos_h0 > 1.0 {
        return Daylight::PolarNight;
    }
    let h0_hours = cos_h0.acos().to_degrees() / 15.0;
    // Solar noon in UTC minutes after midnight.
    let noon_min = 720.0 - 4.0 * lon - eot_min;
    let midnight = noon_guess - Duration::hours(12);
    let at = |minutes: f64| midnight + Duration::seconds((minutes * 60.0).round() as i64);
    Daylight::Normal { sunrise: at(noon_min - h0_hours * 60.0), sunset: at(noon_min + h0_hours * 60.0) }
}

/// Sky phase from a solar altitude.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Sky {
    Day,
    Twilight,
    Night,
}

pub fn sky(altitude: f64) -> Sky {
    if altitude > SUNRISE_ALTITUDE {
        Sky::Day
    } else if altitude > CIVIL_TWILIGHT {
        Sky::Twilight
    } else {
        Sky::Night
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn every_curated_zone_parses() {
        for city in CITIES {
            assert!(city.zone.parse::<Tz>().is_ok(), "{} → {}", city.name, city.zone);
            assert!((-90.0..=90.0).contains(&city.lat) && (-180.0..=180.0).contains(&city.lon), "{}", city.name);
            assert_eq!(city.code.len(), 3, "{}", city.name);
            assert!(!city.arabic.is_empty());
        }
        for name in DEFAULT_CITIES {
            assert!(resolve(name).is_some(), "{name}");
        }
    }

    #[test]
    fn resolves_names_codes_arabic_and_zones() {
        assert_eq!(resolve("cairo").unwrap().tz, chrono_tz::Africa::Cairo);
        assert_eq!(resolve("الإسكندرية").unwrap().name, "Alexandria");
        assert_eq!(resolve("مكة المكرمة").unwrap().tz, chrono_tz::Asia::Riyadh);
        assert_eq!(resolve("NYC").unwrap().name, "New York");
        let lisbon = resolve("europe/lisbon").unwrap();
        assert_eq!(lisbon.name, "Lisbon");
        assert_eq!(lisbon.coords, Some((38.7223, -9.1393)), "curated coords win when the name matches");
        let zone = resolve("America/Argentina/Ushuaia").unwrap();
        assert_eq!(zone.name, "Ushuaia");
        assert_eq!(zone.code, "USH");
        assert!(zone.coords.is_none());
        assert!(resolve("Atlantis/Nowhere").is_none());
        assert!(resolve("  ").is_none());
    }

    #[test]
    fn search_ranks_and_folds() {
        let hits = search("sao");
        assert_eq!(hits[0].name, "São Paulo");
        let arabic = search("مكه");
        assert_eq!(arabic[0].name, "Mecca", "taa marbuta folds to haa");
        let zone_hits = search("kolkata");
        assert!(zone_hits.iter().any(|m| m.zone == "Asia/Kolkata"));
        assert!(search("lon")[0].name == "London");
        assert!(search("").is_empty());
        assert!(search("e").len() <= MAX_RESULTS);
        // A tz-only city.
        assert!(search("ushuaia").iter().any(|m| m.key == "America/Argentina/Ushuaia"));
    }

    #[test]
    fn offsets_follow_daylight_saving() {
        let ny = resolve("New York").unwrap();
        assert_eq!(ny.utc_offset(utc(2026, 1, 15, 12, 0)), -5 * 3600);
        assert_eq!(ny.utc_offset(utc(2026, 7, 15, 12, 0)), -4 * 3600);
        let kathmandu = resolve("Kathmandu").unwrap();
        assert_eq!(split_offset(kathmandu.utc_offset(utc(2026, 1, 1, 0, 0))), (1, 5, 45));
        assert_eq!(split_offset(-(3 * 3600 + 30 * 60)), (-1, 3, 30));
        assert_eq!(split_offset(0), (0, 0, 0));
    }

    #[test]
    fn twelve_and_twenty_four_hour_text() {
        assert_eq!(format_time(0, 5, false), ClockText { time: "12:05".into(), meridiem: Some("AM") });
        assert_eq!(format_time(13, 0, false).time, "1:00");
        assert_eq!(format_time(13, 0, false).meridiem, Some("PM"));
        assert_eq!(format_time(7, 9, true), ClockText { time: "07:09".into(), meridiem: None });
    }

    #[test]
    fn working_hours_wrap_midnight() {
        assert!(is_working_hour(9, 9, 17));
        assert!(!is_working_hour(17, 9, 17));
        assert!(is_working_hour(23, 22, 6));
        assert!(is_working_hour(2, 22, 6));
        assert!(!is_working_hour(12, 22, 6));
    }

    #[test]
    fn london_equinox_sunrise_and_sunset() {
        // Almanac: 2026-03-20 London sunrise ≈ 06:03 UTC, sunset ≈ 18:14 UTC.
        let Daylight::Normal { sunrise, sunset } = daylight(51.5074, -0.1278, NaiveDate::from_ymd_opt(2026, 3, 20).unwrap()) else { panic!("normal day") };
        let near = |t: DateTime<Utc>, h: u32, m: u32| (t - utc(2026, 3, 20, h, m)).num_minutes().abs() <= 6;
        assert!(near(sunrise, 6, 3), "sunrise {sunrise}");
        assert!(near(sunset, 18, 14), "sunset {sunset}");
    }

    #[test]
    fn alexandria_altitude_is_plausible() {
        let (lat, lon) = (31.2001, 29.9187);
        // Local solar noon in late June: the sun is high (~82°).
        let noon = solar_altitude(lat, lon, utc(2026, 6, 21, 10, 0));
        assert!((78.0..=84.0).contains(&noon), "noon altitude {noon}");
        assert_eq!(sky(solar_altitude(lat, lon, utc(2026, 6, 21, 22, 0))), Sky::Night);
        assert_eq!(sky(noon), Sky::Day);
    }

    #[test]
    fn polar_days_and_nights() {
        let june = NaiveDate::from_ymd_opt(2026, 6, 21).unwrap();
        let december = NaiveDate::from_ymd_opt(2026, 12, 21).unwrap();
        assert_eq!(daylight(78.22, 15.65, june), Daylight::PolarDay);
        assert_eq!(daylight(78.22, 15.65, december), Daylight::PolarNight);
    }

    #[test]
    fn day_delta_counts_calendar_days() {
        let d = |day| NaiveDate::from_ymd_opt(2026, 10, day).unwrap();
        assert_eq!(day_delta(d(28), d(27)), 1);
        assert_eq!(day_delta(d(26), d(27)), -1);
    }
}
