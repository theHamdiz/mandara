//! Conditional requests to the policy repository, independent of store mirrors.

use super::super::{agent, rate_limit_wait};

/// The policy is data maintained independently of shell releases.
pub const POLICY_URL: &str = "https://api.github.com/repos/theHamdiz/mandara-store-policy/contents/policy.toml?ref=main";
/// Enough for thousands of rules while bounding memory and cache reads.
pub(super) const MAX_POLICY_BYTES: u64 = 1 << 20;

pub enum Response {
    Modified { text: String, etag: Option<String> },
    NotModified,
}

pub trait Transport {
    fn fetch(&self, etag: Option<&str>) -> Result<Response, String>;
}

pub(super) struct Github;

impl Transport for Github {
    /// Conditionally reads the central policy repository, independently of
    /// marketplace mirrors. Offline mode prevents any request; transport
    /// failures leave cache retention decisions to the caller.
    fn fetch(&self, etag: Option<&str>) -> Result<Response, String> {
        crate::offline::check()?;
        let mut request = agent().get(POLICY_URL)
            .header("User-Agent", concat!("mandara-shell/", env!("CARGO_PKG_VERSION")))
            .header("Accept", "application/vnd.github.raw+json");
        if let Some(etag) = etag { request = request.header("If-None-Match", etag); }
        decode(request.call().map_err(|e| e.to_string())?)
    }
}

/// Accepts a complete raw document or a not-modified response. Other statuses,
/// oversized bodies and invalid UTF-8 fail without producing policy text;
/// TOML validation belongs to the cache loader.
fn decode(mut response: ureq::http::Response<ureq::Body>) -> Result<Response, String> {
    if let Some(wait) = rate_limit_wait(&response) { return Err(format!("GitHub rate limit; retry after {wait}s")); }
    if response.status() == ureq::http::StatusCode::NOT_MODIFIED { return Ok(Response::NotModified); }
    if response.status() != ureq::http::StatusCode::OK { return Err(response.status().to_string()); }
    let etag = response.headers().get("etag").and_then(|s| s.to_str().ok()).map(str::to_string);
    let text = response.body_mut().with_config().limit(MAX_POLICY_BYTES).read_to_string().map_err(|e| e.to_string())?;
    Ok(Response::Modified { text, etag })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, bytes: Vec<u8>) -> ureq::http::Response<ureq::Body> {
        ureq::http::Response::builder().status(status).header("etag", "fixture-v1").body(ureq::Body::builder().data(bytes)).unwrap()
    }

    #[test]
    fn empty_file_requires_a_complete_response_and_preserves_etag() {
        assert!(matches!(decode(response(200, Vec::new())), Ok(Response::Modified { text, etag: Some(etag) }) if text.is_empty() && etag == "fixture-v1"));
        assert!(matches!(decode(response(304, Vec::new())), Ok(Response::NotModified)));
        for status in [204, 206, 404, 500] { assert!(decode(response(status, Vec::new())).is_err()); }
        assert!(decode(response(200, vec![b'x'; MAX_POLICY_BYTES as usize + 1])).is_err());
        assert!(decode(response(200, vec![0xff])).is_err());
    }
}
