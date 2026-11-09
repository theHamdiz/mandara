//! The public radio-browser.info directory: an optional search, made only
//! when someone searches it (and the directory is switched on), with a
//! bounded number of results and a bounded answer size.

use std::fmt::Write as _;
use std::time::Duration;

use serde::Deserialize;

use super::station::{MAX_NAME_CHARS, clean_name, validate_url};

/// The directory's load-balanced address.
pub const API: &str = "https://all.api.radio-browser.info";
/// Results asked for, at most.
pub const MAX_RESULTS: usize = 50;
pub const DEFAULT_RESULTS: usize = 25;
/// Largest answer read (bytes).
const MAX_BODY: u64 = 512 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);
/// Longest query sent (characters).
pub const MAX_QUERY_CHARS: usize = 64;

/// One directory station.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The directory's id for it.
    pub uuid: String,
    pub name: String,
    pub url: String,
    pub homepage: String,
    pub country: String,
    /// Codec as the directory lists it (`MP3`, `AAC+`…).
    pub codec: String,
    pub kbps: u32,
    pub tags: String,
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    stationuuid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    url_resolved: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    homepage: String,
    #[serde(default)]
    countrycode: String,
    #[serde(default)]
    codec: String,
    #[serde(default)]
    bitrate: u32,
    #[serde(default)]
    tags: String,
    #[serde(default)]
    hls: u8,
}

fn short(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((end, _)) => s[..end].to_string(),
        None => s.to_string(),
    }
}

/// Keeps entries Mandara can play: a valid address, no HLS, no codec it
/// can't decode.
pub fn parse(body: &[u8], limit: usize) -> Result<Vec<Found>, String> {
    let raw: Vec<Raw> = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(raw.len().min(limit));
    for r in raw {
        if out.len() >= limit {
            break;
        }
        let url = if r.url_resolved.trim().is_empty() { r.url.trim() } else { r.url_resolved.trim() };
        let codec = r.codec.trim().to_ascii_uppercase();
        if r.hls != 0 || validate_url(url).is_err() || codec.contains("OPUS") || codec.contains("FLAC") || r.name.trim().is_empty() {
            continue;
        }
        out.push(Found {
            uuid: short(r.stationuuid.trim(), 40),
            name: clean_name(&short(&r.name, MAX_NAME_CHARS * 2)),
            url: url.to_string(),
            homepage: Some(r.homepage.trim()).filter(|h| validate_url(h).is_ok()).unwrap_or("").to_string(),
            country: short(r.countrycode.trim(), 2).to_ascii_uppercase(),
            codec: short(&codec, 12),
            kbps: r.bitrate.min(10_000),
            tags: short(r.tags.trim(), 120),
        });
    }
    Ok(out)
}

/// The search address for `query` (by name, most listened first).
pub fn search_url(query: &str, limit: usize) -> String {
    let q = short(query.trim(), MAX_QUERY_CHARS);
    let mut enc = String::with_capacity(q.len() * 3);
    for b in q.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            enc.push(char::from(b));
        } else {
            let _ = write!(enc, "%{b:02X}");
        }
    }
    format!("{API}/json/stations/search?name={enc}&limit={}&hidebroken=true&order=clickcount&reverse=true", limit.clamp(1, MAX_RESULTS))
}

/// Searches the directory (blocking; refused while offline). Up to `limit`
/// playable results.
pub fn search(query: &str, limit: usize) -> Result<Vec<Found>, String> {
    crate::offline::check()?;
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, MAX_RESULTS);
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(TIMEOUT)).build().into();
    // Ask for a few more: some are filtered out.
    let mut resp = agent
        .get(&search_url(query, (limit + limit / 2).min(MAX_RESULTS)))
        .header("User-Agent", super::http::USER_AGENT)
        .call()
        .map_err(|e| mandara_i18n::tf("The station directory didn't answer: {error}", &[("error", &e)]))?;
    let body = resp.body_mut().with_config().limit(MAX_BODY).read_to_vec().map_err(|e| e.to_string())?;
    parse(&body, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_filtered_and_bounded() {
        let body = br#"[
          {"stationuuid":"a","name":"  Corniche FM ","url_resolved":"https://a.example/live","url":"","homepage":"https://a.example","countrycode":"eg","codec":"MP3","bitrate":128,"tags":"jazz,chill","hls":0},
          {"stationuuid":"b","name":"HLS One","url_resolved":"https://b.example/x.m3u8","codec":"AAC","hls":1},
          {"stationuuid":"c","name":"Opus","url_resolved":"https://c.example/o","codec":"OPUS"},
          {"stationuuid":"d","name":"Bad","url_resolved":"javascript:x","codec":"MP3"},
          {"stationuuid":"e","name":"Fallback","url_resolved":"","url":"http://e.example:8000/","codec":"aac+","bitrate":64},
          {"stationuuid":"f","name":"","url_resolved":"https://f.example/","codec":"MP3"},
          {"stationuuid":"g","name":"Third","url_resolved":"https://g.example/","codec":"MP3"}
        ]"#;
        let found = parse(body, 10).unwrap();
        assert_eq!(found.iter().map(|f| f.uuid.as_str()).collect::<Vec<_>>(), ["a", "e", "g"]);
        assert_eq!(found[0].name, "Corniche FM");
        assert_eq!(found[0].country, "EG");
        assert_eq!(found[1].url, "http://e.example:8000/");
        assert_eq!(found[1].codec, "AAC+");
        assert_eq!(parse(body, 2).unwrap().len(), 2, "the limit holds");
        assert!(parse(b"{not json", 10).is_err());
        assert!(parse(b"[]", 10).unwrap().is_empty());
    }

    #[test]
    fn queries_are_encoded_and_bounded() {
        let url = search_url("  jazz & blues/αβ ", 500);
        assert!(url.starts_with("https://all.api.radio-browser.info/json/stations/search?name=jazz%20%26%20blues%2F%CE%B1%CE%B2&"), "{url}");
        assert!(url.contains("&limit=50&"), "{url}");
        let long = search_url(&"x".repeat(500), 10);
        assert!(long.contains(&format!("name={}&", "x".repeat(MAX_QUERY_CHARS))));
    }
}
