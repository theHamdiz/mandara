//! Bounded HTTP for feeds, discovery pages and pictures: gzip, validated
//! redirects, conditional requests and the server's own retry advice.

use std::io::Read as _;
use std::time::Duration;

use mandara_i18n::{t, tf};

use super::web_url;

/// A stalled server must not hold a refresh indefinitely.
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Redirect hops followed at most; each hop is checked like the first.
pub const MAX_REDIRECTS: usize = 5;
const USER_AGENT: &str = concat!("mandara-shell/", env!("CARGO_PKG_VERSION"), " (feed reader)");

/// Why a request failed, and how long the server asked us to wait.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub message: String,
    pub retry_after: Option<u64>,
}

impl From<String> for Failure {
    fn from(message: String) -> Self { Self { message, retry_after: None } }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self { Self { message: message.into(), retry_after: None } }
}

/// A successful answer.
#[derive(Debug)]
pub enum Fetched {
    /// The validators still match: nothing new.
    NotModified,
    Body(Body),
}

#[derive(Debug, Default)]
pub struct Body {
    pub bytes: Vec<u8>,
    /// Where the content came from after redirects.
    pub final_url: String,
    /// Every hop was a permanent redirect (the subscription moved).
    pub moved: bool,
    pub charset: Option<String>,
    pub etag: Option<String>,
    pub modified: Option<String>,
}

pub(super) fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(REQUEST_TIMEOUT)).max_redirects(0).http_status_as_error(false).build().into()
}

/// One GET with conditional headers. Redirects are followed by hand, at
/// most [`MAX_REDIRECTS`] times, only to web addresses and never from
/// https down to http. The body is read up to `limit` bytes, decompressed
/// when the server gzipped it.
pub(super) fn get(agent: &ureq::Agent, url: &str, accept: &str, validators: (Option<&str>, Option<&str>), limit: usize) -> Result<Fetched, Failure> {
    crate::offline::check()?;
    let mut current = url.to_owned();
    let mut moved = true;
    for _ in 0..=MAX_REDIRECTS {
        if !web_url(&current) { return Err(t("Use an http:// or https:// feed address without a password").into()); }
        let mut request = agent.get(&current).header("User-Agent", USER_AGENT).header("Accept", accept).header("Accept-Encoding", "gzip");
        if let Some(etag) = validators.0 { request = request.header("If-None-Match", etag); }
        if let Some(modified) = validators.1 { request = request.header("If-Modified-Since", modified); }
        let mut response = request.call().map_err(|error| Failure::from(tf("Could not reach the server: {error}", &[("error", &error)]).to_string()))?;
        let status = response.status();
        let header = |name: &str| response.headers().get(name).and_then(|value| value.to_str().ok()).map(str::to_owned);
        if status == ureq::http::StatusCode::NOT_MODIFIED { return Ok(Fetched::NotModified); }
        if status.is_redirection() {
            let location = header("location").ok_or_else(|| Failure::from(t("The server redirected without saying where")))?;
            let next = url::Url::parse(&current).and_then(|base| base.join(&location)).map_err(|_| Failure::from(t("The server redirected to an address that is not valid")))?;
            if current.starts_with("https:") && next.scheme() == "http" { return Err(t("The server redirected from a secure address to an insecure one").into()); }
            moved &= matches!(status.as_u16(), 301 | 308);
            current = next.into();
            continue;
        }
        if status.as_u16() == 429 || status.as_u16() == 503 {
            let retry_after = header("retry-after").and_then(|value| retry_after(&value));
            return Err(Failure { message: tf("The server is busy (HTTP {status}); trying again later", &[("status", &status.as_u16())]).to_string(), retry_after });
        }
        if !status.is_success() {
            return Err(tf("The server answered HTTP {status}", &[("status", &status.as_u16())]).to_string().into());
        }
        let charset = header("content-type").and_then(|value| split_content_type(&value).1);
        let gzipped = header("content-encoding").is_some_and(|value| value.trim().eq_ignore_ascii_case("gzip"));
        let etag = header("etag");
        let modified = header("last-modified");
        let mut bytes = Vec::new();
        let failed = |error: std::io::Error| Failure::from(tf("Could not download it: {error}", &[("error", &error)]).to_string());
        let reader = response.body_mut().as_reader().take((limit + 1) as u64);
        if gzipped {
            flate2::read::GzDecoder::new(reader).take((limit + 1) as u64).read_to_end(&mut bytes).map_err(failed)?;
        } else {
            let mut reader = reader;
            reader.read_to_end(&mut bytes).map_err(failed)?;
        }
        if bytes.len() > limit { return Err(t("This download is too large").into()); }
        return Ok(Fetched::Body(Body { bytes, moved: moved && current != url, final_url: current, charset, etag, modified }));
    }
    Err(t("The server redirected too many times").into())
}

/// `text/xml; charset=ISO-8859-1` → (`text/xml`, `ISO-8859-1`).
fn split_content_type(value: &str) -> (Option<String>, Option<String>) {
    let mut parts = value.split(';');
    let mime = parts.next().map(|mime| mime.trim().to_ascii_lowercase()).filter(|mime| !mime.is_empty());
    let charset = parts.find_map(|part| part.trim().strip_prefix("charset=").or_else(|| part.trim().strip_prefix("CHARSET="))).map(|value| value.trim_matches(['"', '\'']).to_owned());
    (mime, charset)
}

/// Seconds from a `Retry-After` header (delay seconds or an HTTP date).
fn retry_after(value: &str) -> Option<u64> {
    if let Ok(seconds) = value.trim().parse::<u64>() { return Some(seconds); }
    let at = chrono::DateTime::parse_from_rfc2822(value.trim()).ok()?;
    u64::try_from(at.timestamp() - chrono::Utc::now().timestamp()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_types_and_retry_advice_parse() {
        assert_eq!(split_content_type("text/XML; charset=\"ISO-8859-1\""), (Some("text/xml".into()), Some("ISO-8859-1".into())));
        assert_eq!(split_content_type("application/rss+xml"), (Some("application/rss+xml".into()), None));
        assert_eq!(retry_after(" 120 "), Some(120));
        assert_eq!(retry_after("Wed, 21 Oct 2015 07:28:00 GMT"), None, "a past date means no extra wait");
        assert_eq!(retry_after("soon"), None);
    }
}
