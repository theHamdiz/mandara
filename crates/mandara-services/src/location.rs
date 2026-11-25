//! Finding the user's location, only ever on request:
//!
//! * **GeoClue2** (`org.freedesktop.GeoClue2`, system bus) — the desktop's
//!   location service, which honours its own permission agent.
//! * **IP geolocation** — one HTTPS request to a public service; used only
//!   when the user presses “Locate me” with the IP mode chosen.
//! * **City search** — Open-Meteo's geocoding API (no key) for typed names.
//!
//! Nothing here runs on its own: callers decide when (Studio buttons and
//! `mandara msg location …`).

use std::time::Duration;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use zbus::zvariant::OwnedObjectPath;

/// A located or searched place.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub latitude: f64,
    pub longitude: f64,
    /// City (or best label) for display.
    pub name: String,
    /// Region / country detail for search results.
    pub detail: String,
    /// IANA time zone (`Africa/Cairo`) when the lookup knows it; empty
    /// otherwise (GeoClue, typed coordinates).
    #[serde(default)]
    pub timezone: String,
}

/// Request timeout for the web lookups.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
/// IP geolocation endpoint (HTTPS, JSON: `latitude`, `longitude`, `city`, `country_name`).
pub const IP_LOOKUP_URL: &str = "https://ipapi.co/json/";
/// Open-Meteo geocoding search endpoint.
pub const GEOCODE_URL: &str = "https://geocoding-api.open-meteo.com/v1/search";
/// Search results returned.
const SEARCH_RESULTS: usize = 6;
/// How long GeoClue may take to produce a fix.
const GEOCLUE_TIMEOUT: Duration = Duration::from_secs(15);
/// GeoClue accuracy level "city" (`GClueAccuracyLevel`).
const GEOCLUE_ACCURACY_CITY: u32 = 4;
/// GeoClue identifies clients by their desktop file id.
const DESKTOP_ID: &str = "org.mandara.shell";
const GEOCLUE: &str = "org.freedesktop.GeoClue2";
const GEOCLUE_MANAGER_PATH: &str = "/org/freedesktop/GeoClue2/Manager";
const GEOCLUE_MANAGER: &str = "org.freedesktop.GeoClue2.Manager";
const GEOCLUE_CLIENT: &str = "org.freedesktop.GeoClue2.Client";
const GEOCLUE_LOCATION: &str = "org.freedesktop.GeoClue2.Location";

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(HTTP_TIMEOUT)).build().into()
}

#[derive(Deserialize)]
struct IpReply {
    latitude: Option<f64>,
    longitude: Option<f64>,
    #[serde(default)]
    city: String,
    #[serde(default)]
    region: String,
    #[serde(default)]
    country_name: String,
    #[serde(default)]
    timezone: String,
}

/// Parses an IP-lookup reply.
fn parse_ip(body: &str) -> Result<Place, String> {
    let r: IpReply = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let (Some(latitude), Some(longitude)) = (r.latitude, r.longitude) else { return Err("the lookup service returned no coordinates".into()) };
    let detail = [r.region, r.country_name].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
    Ok(Place { latitude, longitude, name: r.city, detail, timezone: r.timezone })
}

/// One IP-based lookup (blocking; run off the UI thread).
pub fn locate_by_ip() -> Result<Place, String> {
    crate::offline::check()?;
    let body = agent().get(IP_LOOKUP_URL).call().map_err(|e| e.to_string())?.body_mut().read_to_string().map_err(|e| e.to_string())?;
    parse_ip(&body)
}

#[derive(Deserialize)]
struct GeoReply {
    #[serde(default)]
    results: Vec<GeoResult>,
}

#[derive(Deserialize)]
struct GeoResult {
    name: String,
    latitude: f64,
    longitude: f64,
    #[serde(default)]
    admin1: Option<String>,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    timezone: String,
}

fn parse_search(body: &str) -> Result<Vec<Place>, String> {
    let r: GeoReply = serde_json::from_str(body).map_err(|e| e.to_string())?;
    Ok(r.results
        .into_iter()
        .map(|g| Place {
            latitude: g.latitude,
            longitude: g.longitude,
            detail: [g.admin1, g.country].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", "),
            name: g.name,
            timezone: g.timezone,
        })
        .collect())
}

/// Searches places by name (blocking). `lang` is a two-letter code.
pub fn search(query: &str, lang: &str) -> Result<Vec<Place>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    crate::offline::check()?;
    let body = agent()
        .get(GEOCODE_URL)
        .query("name", query)
        .query("count", SEARCH_RESULTS.to_string())
        .query("language", lang)
        .query("format", "json")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    parse_search(&body)
}

/// Asks GeoClue2 for one city-level fix (async; system bus).
pub async fn locate_geoclue() -> Result<Place, String> {
    let fail = |e: zbus::Error| format!("GeoClue: {e}");
    let conn = crate::runtime::system().await.map_err(fail)?;
    let manager = crate::actor::proxy(&conn, GEOCLUE, GEOCLUE_MANAGER_PATH, GEOCLUE_MANAGER).await.map_err(fail)?;
    let client_path: OwnedObjectPath = manager.call("GetClient", &()).await.map_err(fail)?;
    let client = crate::actor::proxy(&conn, GEOCLUE, client_path.as_str(), GEOCLUE_CLIENT).await.map_err(fail)?;
    client.set_property("DesktopId", DESKTOP_ID).await.map_err(|e| format!("GeoClue: {e}"))?;
    client.set_property("RequestedAccuracyLevel", GEOCLUE_ACCURACY_CITY).await.map_err(|e| format!("GeoClue: {e}"))?;
    let mut updates = client.receive_signal("LocationUpdated").await.map_err(fail)?;
    client.call::<_, _, ()>("Start", &()).await.map_err(fail)?;
    let wait = async {
        while let Some(msg) = updates.next().await {
            if let Ok((_old, new)) = msg.body().deserialize::<(OwnedObjectPath, OwnedObjectPath)>() {
                return Some(new);
            }
        }
        None
    };
    let timeout = async {
        async_io::Timer::after(GEOCLUE_TIMEOUT).await;
        None
    };
    let location = futures::future::select(Box::pin(wait), Box::pin(timeout)).await.factor_first().0;
    let _ = client.call::<_, _, ()>("Stop", &()).await;
    // Most often GeoClue has no lookup service to ask: Mozilla's closed in
    // 2024 and many distributions (Arch among them) ship none set up.
    let path = location.ok_or_else(|| mandara_i18n::t("Your system's location service (GeoClue) couldn't find you. Many Linux systems ship it without a lookup service. Choose IP lookup, or type your city and press Find.").to_string())?;
    let loc = crate::actor::proxy(&conn, GEOCLUE, path.as_str(), GEOCLUE_LOCATION).await.map_err(fail)?;
    let latitude: f64 = loc.get_property("Latitude").await.map_err(fail)?;
    let longitude: f64 = loc.get_property("Longitude").await.map_err(fail)?;
    let name: String = loc.get_property("Description").await.unwrap_or_default();
    Ok(Place { latitude, longitude, name, detail: String::new(), timezone: String::new() })
}

/// Rounds coordinates to ~1 km so the stored location stays coarse.
pub fn coarse(v: f64) -> f64 {
    const PLACES: f64 = 100.0;
    (v * PLACES).round() / PLACES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ip_lookup_replies() {
        let p = parse_ip(r#"{"ip":"x","city":"Alexandria","region":"Alexandria","country_name":"Egypt","latitude":31.2,"longitude":29.9,"timezone":"Africa/Cairo"}"#).unwrap();
        assert_eq!((p.name.as_str(), p.detail.as_str(), p.timezone.as_str()), ("Alexandria", "Alexandria, Egypt", "Africa/Cairo"));
        assert!((p.latitude - 31.2).abs() < 1e-9);
        assert!(parse_ip(r#"{"error":true,"reason":"RateLimited"}"#).is_err());
        assert!(parse_ip("not json").is_err());
    }

    #[test]
    fn parses_geocoding_results() {
        let body = r#"{"results":[{"name":"Alexandria","latitude":31.2,"longitude":29.92,"admin1":"Alexandria","country":"Egypt","timezone":"Africa/Cairo"},{"name":"Alexandria","latitude":38.8,"longitude":-77.05,"country":"United States"}]}"#;
        let r = parse_search(body).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[1].detail, "United States");
        assert_eq!((r[0].timezone.as_str(), r[1].timezone.as_str()), ("Africa/Cairo", ""), "the time zone comes along when the service knows it");
        assert!(parse_search("{}").unwrap().is_empty(), "no matches → empty");
        assert_eq!(coarse(31.28456), 31.28);
    }
}
