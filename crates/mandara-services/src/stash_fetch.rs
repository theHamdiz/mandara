//! Stash's one network request, made only when you click it: a picture
//! behind a dropped link, or a site's icon. HTTPS only (redirects too),
//! size-capped, with timeouts, and refused in offline mode. Blocking: call
//! it off the UI thread.

use std::io::Read as _;
use std::time::Duration;

use mandara_i18n::tf;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const GLOBAL_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_REDIRECTS: u32 = 4;
const UA: &str = concat!("mandara-shell/", env!("CARGO_PKG_VERSION"));

/// A fetched body and its declared type.
pub struct Fetched {
    pub bytes: Vec<u8>,
    /// `Content-Type` without parameters, lower-case.
    pub mime: Option<String>,
}

/// Fetches `url` (HTTPS only), reading at most `limit` bytes. An image
/// fetch (`want_image`) refuses a body that is not declared as a picture.
pub fn fetch_https(url: &str, limit: u64, want_image: bool) -> Result<Fetched, String> {
    if !url.trim().to_ascii_lowercase().starts_with("https://") {
        return Err(mandara_i18n::t("Only https:// links are fetched").to_string());
    }
    crate::offline::check()?;
    let agent: ureq::Agent = ureq::Agent::config_builder().https_only(true).max_redirects(MAX_REDIRECTS).timeout_connect(Some(CONNECT_TIMEOUT)).timeout_global(Some(GLOBAL_TIMEOUT)).build().into();
    let mut response = agent.get(url.trim()).header("User-Agent", UA).call().map_err(|e| tf("Download failed: {error}", &[("error", &e)]))?;
    let mime = response.headers().get("content-type").and_then(|v| v.to_str().ok()).map(|v| v.split(';').next().unwrap_or(v).trim().to_ascii_lowercase());
    if want_image && !mime.as_deref().is_some_and(|m| m.starts_with("image/")) {
        return Err(tf("Not a picture ({type})", &[("type", &mime.as_deref().unwrap_or("?"))]));
    }
    let mut bytes = Vec::new();
    response.body_mut().with_config().limit(limit).reader().take(limit).read_to_end(&mut bytes).map_err(|e| tf("Download failed: {error}", &[("error", &e)]))?;
    Ok(Fetched { bytes, mime })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_is_fetched() {
        for url in ["http://example.org/a.png", "file:///etc/passwd", "ftp://x/y", "javascript:alert(1)"] {
            assert!(fetch_https(url, 10, true).is_err(), "{url}");
        }
    }
}
