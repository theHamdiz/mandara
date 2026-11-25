//! Weather actor: current conditions, a 12-hour strip and a 5-day forecast
//! from Open-Meteo (free, no API key), refreshed every 15 minutes or on
//! request. WMO weather codes are mapped to a small set of conditions the
//! UI knows how to animate.

use std::time::Duration;

use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};

use crate::actor::{Envelope, ServiceHandle, reply};

const REFRESH: Duration = Duration::from_secs(15 * 60);
const RETRY: Duration = Duration::from_secs(60);

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Condition {
    #[default]
    Clear,
    PartlyCloudy,
    Cloudy,
    Fog,
    Drizzle,
    Rain,
    Snow,
    Thunder,
}

impl Condition {
    /// Maps a WMO 4677 weather interpretation code.
    pub fn from_wmo(code: u32) -> Self {
        match code {
            0 => Condition::Clear,
            1 | 2 => Condition::PartlyCloudy,
            3 => Condition::Cloudy,
            45 | 48 => Condition::Fog,
            51..=57 => Condition::Drizzle,
            61..=67 | 80..=82 => Condition::Rain,
            71..=77 | 85 | 86 => Condition::Snow,
            95..=99 => Condition::Thunder,
            _ => Condition::Cloudy,
        }
    }

    /// The condition's name in the current language.
    pub fn label(self) -> &'static str {
        match self {
            Condition::Clear => t("Clear"),
            Condition::PartlyCloudy => t("Partly cloudy"),
            Condition::Cloudy => t("Cloudy"),
            Condition::Fog => t("Fog"),
            Condition::Drizzle => t("Drizzle"),
            Condition::Rain => t("Rain"),
            Condition::Snow => t("Snow"),
            Condition::Thunder => t("Thunderstorm"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Hour {
    /// Local hour of day, 0–23.
    pub hour: u32,
    pub temp: f32,
    pub condition: Condition,
    pub is_day: bool,
    /// Precipitation probability, percent.
    pub precip: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Day {
    /// `YYYY-MM-DD`.
    pub date: String,
    pub min: f32,
    pub max: f32,
    pub condition: Condition,
    pub precip: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Weather {
    /// °C (every temperature in this module is Celsius).
    pub temp: f32,
    pub feels_like: f32,
    pub humidity: u8,
    /// km/h.
    pub wind: f32,
    pub condition: Condition,
    pub is_day: bool,
    /// Local `HH:MM`.
    pub sunrise: String,
    pub sunset: String,
    pub hours: Vec<Hour>,
    pub days: Vec<Day>,
    /// Unix seconds of the fetch.
    pub fetched_at: i64,
    /// The forecast location's IANA time zone (`Africa/Cairo`), when known.
    pub timezone: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct WeatherState {
    pub weather: Option<Weather>,
    pub error: Option<String>,
    pub loading: bool,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct WeatherQuery {
    pub latitude: f64,
    pub longitude: f64,
    /// Minutes between fetches (`weather.refresh_minutes`; at least
    /// [`MIN_REFRESH_MIN`]).
    pub refresh_minutes: u32,
}

/// Shortest refresh interval honoured, minutes (Open-Meteo updates
/// roughly every 15 min; faster polling only costs requests).
pub const MIN_REFRESH_MIN: u32 = 5;
/// Seconds per minute.
const S_PER_MIN: u64 = 60;

impl WeatherQuery {
    fn refresh(&self) -> Duration {
        if self.refresh_minutes == 0 {
            return REFRESH;
        }
        Duration::from_secs(u64::from(self.refresh_minutes.max(MIN_REFRESH_MIN)) * S_PER_MIN)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum WeatherCommand {
    Refresh,
    /// New location (from the plugin's settings); refetches. Data is always
    /// metric (°C, km/h); the UI converts to the user's units.
    Configure(WeatherQuery),
}

pub type WeatherService = ServiceHandle<WeatherState, WeatherCommand>;

// ── Open-Meteo response ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct Response {
    current: Current,
    hourly: Hourly,
    daily: Daily,
    /// With `timezone=auto`: the coordinates' own time zone.
    #[serde(default)]
    timezone: String,
}

#[derive(Deserialize)]
struct Current {
    time: String,
    temperature_2m: f32,
    apparent_temperature: f32,
    relative_humidity_2m: f32,
    weather_code: u32,
    wind_speed_10m: f32,
    is_day: u8,
}

#[derive(Deserialize)]
struct Hourly {
    time: Vec<String>,
    temperature_2m: Vec<f32>,
    weather_code: Vec<u32>,
    is_day: Vec<u8>,
    precipitation_probability: Vec<Option<f32>>,
}

#[derive(Deserialize)]
struct Daily {
    time: Vec<String>,
    temperature_2m_max: Vec<f32>,
    temperature_2m_min: Vec<f32>,
    weather_code: Vec<u32>,
    sunrise: Vec<String>,
    sunset: Vec<String>,
    precipitation_probability_max: Vec<Option<f32>>,
}

fn url(q: WeatherQuery) -> String {
    format!(
        "https://api.open-meteo.com/v1/forecast?latitude={:.4}&longitude={:.4}\
         &current=temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m,is_day\
         &hourly=temperature_2m,weather_code,is_day,precipitation_probability\
         &daily=temperature_2m_max,temperature_2m_min,weather_code,sunrise,sunset,precipitation_probability_max\
         &timezone=auto&forecast_days=6&temperature_unit=celsius&wind_speed_unit=kmh",
        q.latitude,
        q.longitude
    )
}

/// `HH:MM` from an ISO local timestamp (`2026-10-27T06:12`).
fn clock(iso: &str) -> String {
    iso.split('T').nth(1).map(|t| t.chars().take(5).collect()).unwrap_or_default()
}

fn hour_of(iso: &str) -> Option<u32> {
    clock(iso).split(':').next()?.parse().ok()
}

/// Turns the raw response into the UI model (next 12 hours from "now").
fn parse(body: &str) -> Result<Weather, String> {
    let r: Response = serde_json::from_str(body).map_err(|e| tf("Unexpected weather data: {error}", &[("error", &e)]))?;
    // Hourly times are local; start at the current hour.
    let now_hour = r.current.time.chars().take(13).collect::<String>();
    let start = r.hourly.time.iter().position(|t| t.starts_with(&now_hour)).unwrap_or(0);
    let hours = (start..r.hourly.time.len())
        .take(12)
        .map(|i| Hour {
            hour: hour_of(&r.hourly.time[i]).unwrap_or(0),
            temp: r.hourly.temperature_2m.get(i).copied().unwrap_or_default(),
            condition: Condition::from_wmo(r.hourly.weather_code.get(i).copied().unwrap_or(3)),
            is_day: r.hourly.is_day.get(i).copied().unwrap_or(1) == 1,
            precip: r.hourly.precipitation_probability.get(i).copied().flatten().unwrap_or(0.0).round() as u8,
        })
        .collect();
    let days = (0..r.daily.time.len())
        .take(5)
        .map(|i| Day {
            date: r.daily.time[i].clone(),
            min: r.daily.temperature_2m_min.get(i).copied().unwrap_or_default(),
            max: r.daily.temperature_2m_max.get(i).copied().unwrap_or_default(),
            condition: Condition::from_wmo(r.daily.weather_code.get(i).copied().unwrap_or(3)),
            precip: r.daily.precipitation_probability_max.get(i).copied().flatten().unwrap_or(0.0).round() as u8,
        })
        .collect();
    Ok(Weather {
        temp: r.current.temperature_2m,
        feels_like: r.current.apparent_temperature,
        humidity: r.current.relative_humidity_2m.round() as u8,
        wind: r.current.wind_speed_10m,
        condition: Condition::from_wmo(r.current.weather_code),
        is_day: r.current.is_day == 1,
        sunrise: r.daily.sunrise.first().map(|s| clock(s)).unwrap_or_default(),
        sunset: r.daily.sunset.first().map(|s| clock(s)).unwrap_or_default(),
        hours,
        days,
        fetched_at: chrono::Local::now().timestamp(),
        timezone: r.timezone,
    })
}

/// Whole-request timeout for the forecast API.
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
/// States buffered for the UI.
const UPDATE_QUEUE: usize = 4;
/// Commands buffered from the UI and CLI.
const COMMAND_QUEUE: usize = 16;

/// An HTTP agent for one refresh. Refreshes are many minutes apart, so a
/// kept agent only held its idle connection, TLS session and ~0.5 MiB of
/// I/O buffers between them.
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(HTTP_TIMEOUT)).build().into()
}

fn fetch(q: WeatherQuery) -> Result<Weather, String> {
    crate::offline::check()?;
    let mut response = agent().get(&url(q)).header("User-Agent", concat!("mandara-shell/", env!("CARGO_PKG_VERSION"))).call().map_err(|e| tf("Weather request failed: {error}", &[("error", &e)]))?;
    let body = response.body_mut().read_to_string().map_err(|e| tf("Weather request failed: {error}", &[("error", &e)]))?;
    parse(&body)
}

/// The IANA time zone at a place (one small Open-Meteo request; blocking).
pub fn timezone_at(latitude: f64, longitude: f64) -> Result<String, String> {
    #[derive(Deserialize)]
    struct Reply {
        timezone: String,
    }
    crate::offline::check()?;
    let url = format!("https://api.open-meteo.com/v1/forecast?latitude={latitude:.4}&longitude={longitude:.4}&timezone=auto&forecast_days=1");
    let mut response = agent().get(&url).header("User-Agent", concat!("mandara-shell/", env!("CARGO_PKG_VERSION"))).call().map_err(|e| e.to_string())?;
    let body = response.body_mut().read_to_string().map_err(|e| e.to_string())?;
    serde_json::from_str::<Reply>(&body).map(|r| r.timezone).map_err(|e| e.to_string())
}

pub fn spawn_weather_service(initial: WeatherQuery) -> WeatherService {
    let (tx, updates) = async_channel::bounded(UPDATE_QUEUE);
    let (commands, rx) = async_channel::bounded::<Envelope<WeatherCommand>>(COMMAND_QUEUE);
    crate::runtime::spawn("weather", async move {
        use futures::FutureExt as _;
        let mut query = initial;
        let mut state = WeatherState { loading: true, ..Default::default() };
        let mut pending: Vec<Option<crate::actor::Reply>> = Vec::new();
        loop {
            state.loading = true;
            if tx.send(state.clone()).await.is_err() {
                return;
            }
            let wait = match crate::runtime::unblock(move || fetch(query)).await {
                Ok(w) => {
                    state = WeatherState { weather: Some(w), error: None, loading: false };
                    for r in pending.drain(..) {
                        reply(&r, Ok("refreshed".into()));
                    }
                    query.refresh()
                }
                Err(err) => {
                    tracing::warn!("{err}");
                    state.error = Some(err.clone());
                    state.loading = false;
                    for r in pending.drain(..) {
                        reply(&r, Err(err.clone()));
                    }
                    RETRY
                }
            };
            if tx.send(state.clone()).await.is_err() {
                return;
            }
            // Sleep until the next refresh, waking early for commands.
            futures::select! {
                cmd = rx.recv().fuse() => match cmd {
                    Ok(Envelope { command, reply: r }) => {
                        if let WeatherCommand::Configure(q) = command {
                            query = q;
                        }
                        pending.push(r);
                    }
                    Err(_) => return,
                },
                _ = crate::runtime::sleep(wait).fuse() => {}
            }
        }
    });
    ServiceHandle::new(updates, commands)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_wmo_codes() {
        assert_eq!(Condition::from_wmo(0), Condition::Clear);
        assert_eq!(Condition::from_wmo(2), Condition::PartlyCloudy);
        assert_eq!(Condition::from_wmo(45), Condition::Fog);
        assert_eq!(Condition::from_wmo(63), Condition::Rain);
        assert_eq!(Condition::from_wmo(81), Condition::Rain);
        assert_eq!(Condition::from_wmo(73), Condition::Snow);
        assert_eq!(Condition::from_wmo(96), Condition::Thunder);
        assert_eq!(Condition::from_wmo(53), Condition::Drizzle);
    }

    #[test]
    fn parses_open_meteo_shape() {
        let body = r#"{
          "current": {"time":"2026-10-27T08:00","temperature_2m":24.3,"apparent_temperature":25.1,"relative_humidity_2m":61,"weather_code":2,"wind_speed_10m":14.2,"is_day":1},
          "hourly": {"time":["2026-10-27T07:00","2026-10-27T08:00","2026-10-27T09:00"],"temperature_2m":[23.0,24.3,25.0],"weather_code":[1,2,61],"is_day":[1,1,1],"precipitation_probability":[0,5,null]},
          "daily": {"time":["2026-10-27","2026-10-28"],"temperature_2m_max":[27.0,26.0],"temperature_2m_min":[19.0,18.5],"weather_code":[2,61],"sunrise":["2026-10-27T06:12","2026-10-28T06:13"],"sunset":["2026-10-27T17:05","2026-10-28T17:04"],"precipitation_probability_max":[10,70]}
        }"#;
        let w = parse(body).unwrap();
        assert_eq!(w.condition, Condition::PartlyCloudy);
        assert_eq!(w.hours.len(), 2, "starts at the current hour");
        assert_eq!(w.hours[0].hour, 8);
        assert_eq!(w.hours[1].condition, Condition::Rain);
        assert_eq!(w.hours[1].precip, 0);
        assert_eq!(w.days[1].precip, 70);
        assert_eq!((w.sunrise.as_str(), w.sunset.as_str()), ("06:12", "17:05"));
    }
}
