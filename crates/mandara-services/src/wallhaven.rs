//! Wallhaven (wallhaven.cc) wallpapers for the picker: search (SFW,
//! general category, no API key needed), cached previews, and full-image
//! downloads into Mandara's data dir. Used when no local wallpaper folder
//! is set up, or on demand from the picker.

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use mandara_i18n::{t, tf};
use serde::Deserialize;

/// Connect timeout for wallhaven.cc (network safety bound).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Body timeout for wallhaven.cc downloads (network safety bound).
const BODY_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Clone, Debug, Deserialize)]
struct Thumbs {
    large: String,
}

#[derive(Clone, Debug, Deserialize)]
struct Raw {
    id: String,
    path: String,
    resolution: String,
    thumbs: Thumbs,
    #[serde(default)]
    colors: Vec<String>,
}

#[derive(Deserialize)]
struct Meta {
    current_page: u32,
    last_page: u32,
}

#[derive(Deserialize)]
struct Response {
    data: Vec<Raw>,
    meta: Meta,
}

/// One search result.
#[derive(Clone, Debug, PartialEq)]
pub struct Wall {
    pub id: String,
    /// Full-resolution image URL.
    pub url: String,
    pub preview_url: String,
    pub resolution: String,
    pub colors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub walls: Vec<Wall>,
    pub page: u32,
    pub last_page: u32,
}

#[derive(Clone, Debug)]
pub struct Query {
    /// Free text; empty = the toplist.
    pub text: String,
    /// `toplist`, `date_added`, `views`, `favorites`, `random`, `relevance`.
    pub sorting: String,
    /// Minimum resolution, e.g. `1920x1080`.
    pub atleast: String,
    pub page: u32,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_connect(Some(CONNECT_TIMEOUT)).timeout_recv_body(Some(BODY_TIMEOUT)).build().into()
}

const UA: &str = concat!("mandara-shell/", env!("CARGO_PKG_VERSION"));

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The search URL (SFW + general only).
pub fn search_url(q: &Query) -> String {
    let sorting = if q.text.trim().is_empty() && q.sorting == "relevance" { "toplist" } else { q.sorting.as_str() };
    format!(
        "https://wallhaven.cc/api/v1/search?q={}&categories=100&purity=100&sorting={}&order=desc&topRange=1y&atleast={}&page={}",
        encode(q.text.trim()),
        encode(sorting),
        encode(&q.atleast),
        q.page.max(1)
    )
}

fn parse(body: &str) -> Result<Page, String> {
    let r: Response = serde_json::from_str(body).map_err(|e| tf("Unexpected Wallhaven response: {error}", &[("error", &e)]))?;
    Ok(Page {
        walls: r.data.into_iter().map(|w| Wall { id: w.id, url: w.path, preview_url: w.thumbs.large, resolution: w.resolution, colors: w.colors }).collect(),
        page: r.meta.current_page,
        last_page: r.meta.last_page,
    })
}

pub fn search(q: &Query) -> Result<Page, String> {
    crate::offline::check()?;
    let mut response = agent().get(&search_url(q)).header("User-Agent", UA).call().map_err(|e| tf("Wallhaven search failed: {error}", &[("error", &e)]))?;
    let body = response.body_mut().read_to_string().map_err(|e| tf("Wallhaven search failed: {error}", &[("error", &e)]))?;
    parse(&body)
}

fn get_bytes(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    crate::offline::check()?;
    let mut response = agent().get(url).header("User-Agent", UA).call().map_err(|e| tf("Download failed: {error}", &[("error", &e)]))?;
    let mut out = Vec::new();
    response.body_mut().with_config().limit(limit).reader().read_to_end(&mut out).map_err(|e| tf("Download failed: {error}", &[("error", &e)]))?;
    Ok(out)
}

fn ext(url: &str) -> &str {
    url.rsplit('.').next().filter(|e| matches!(*e, "jpg" | "jpeg" | "png" | "webp")).unwrap_or("jpg")
}

/// Where previews are cached.
pub fn preview_dir() -> PathBuf {
    mandara_config::paths::cache_dir().join("wallhaven")
}

/// Where downloaded wallpapers are kept.
pub fn download_dir() -> PathBuf {
    mandara_config::paths::data_dir().join("wallpapers").join("wallhaven")
}

/// The cached preview image for `w`, fetching it if needed.
pub fn preview(w: &Wall) -> Result<PathBuf, String> {
    let dir = preview_dir();
    let path = dir.join(format!("{}.{}", w.id, ext(&w.preview_url)));
    if path.exists() {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let bytes = get_bytes(&w.preview_url, 8 << 20)?;
    let part = path.with_extension("part");
    std::fs::write(&part, bytes).map_err(|e| format!("{}: {e}", part.display()))?;
    std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// The full-resolution wallpaper on disk, downloading it if needed.
pub fn download(w: &Wall) -> Result<PathBuf, String> {
    let dir = download_dir();
    let path = dir.join(format!("wallhaven-{}.{}", w.id, ext(&w.url)));
    if path.exists() {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let bytes = get_bytes(&w.url, 64 << 20)?;
    image::guess_format(&bytes).map_err(|_| t("Wallhaven returned something that isn't an image").to_string())?;
    let part = path.with_extension("part");
    std::fs::write(&part, bytes).map_err(|e| format!("{}: {e}", part.display()))?;
    std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// The Wallhaven id of a file this module wrote (preview or download).
pub fn id_of(path: &std::path::Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let in_ours = path.starts_with(preview_dir()) || path.starts_with(download_dir());
    in_ours.then(|| stem.trim_start_matches("wallhaven-").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search_response() {
        let body = r##"{"data":[{"id":"abc123","url":"https://wallhaven.cc/w/abc123","short_url":"x","views":1,"favorites":1,"source":"","purity":"sfw","category":"general","dimension_x":3840,"dimension_y":2160,"resolution":"3840x2160","ratio":"1.78","file_size":1,"file_type":"image/jpeg","created_at":"","colors":["#336699"],"path":"https://w.wallhaven.cc/full/ab/wallhaven-abc123.jpg","thumbs":{"large":"https://th.wallhaven.cc/lg/ab/abc123.jpg","original":"o","small":"s"}}],"meta":{"current_page":1,"last_page":7,"per_page":24,"total":150,"query":null,"seed":null}}"##;
        let p = parse(body).unwrap();
        assert_eq!(p.walls[0].id, "abc123");
        assert_eq!(p.walls[0].url, "https://w.wallhaven.cc/full/ab/wallhaven-abc123.jpg");
        assert_eq!((p.page, p.last_page), (1, 7));
    }

    #[test]
    fn url_is_sfw_and_encoded() {
        let u = search_url(&Query { text: "sea & sky".into(), sorting: "relevance".into(), atleast: "1920x1080".into(), page: 2 });
        assert!(u.contains("purity=100") && u.contains("q=sea+%26+sky") && u.contains("page=2"));
        let top = search_url(&Query { text: String::new(), sorting: "relevance".into(), atleast: "1920x1080".into(), page: 1 });
        assert!(top.contains("sorting=toplist"));
    }
}
