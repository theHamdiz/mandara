//! HTTP for the updater: offline mode first, a whole-request timeout, a
//! size cap, GitHub's rate-limit message (shared with the marketplace) and
//! byte progress.

use std::io::Read as _;
use std::time::Duration;

use mandara_i18n::{t, tf};
use ureq::http::StatusCode;

/// Whole-request timeout (a full shell tarball is tens of MB).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
/// Read granularity, and how often progress is reported.
const CHUNK: usize = 64 << 10;

/// A request that didn't produce a body.
#[derive(Debug)]
pub enum Fetch {
    NotFound,
    Failed(String),
}

impl From<Fetch> for String {
    fn from(f: Fetch) -> Self {
        match f {
            Fetch::NotFound => tf("Couldn't reach GitHub: {error}", &[("error", &StatusCode::NOT_FOUND)]),
            Fetch::Failed(e) => e,
        }
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(REQUEST_TIMEOUT)).http_status_as_error(false).build().into()
}

/// GETs `url`, reading at most `limit` bytes (more is an error) and calling
/// `on_bytes` with the running total.
pub fn get(url: &str, limit: u64, on_bytes: &dyn Fn(u64)) -> Result<Vec<u8>, Fetch> {
    crate::offline::check().map_err(Fetch::Failed)?;
    let reach = |e: &dyn std::fmt::Display| Fetch::Failed(tf("Couldn't reach GitHub: {error}", &[("error", &e)]));
    let mut r = agent()
        .get(url)
        .header("User-Agent", concat!("mandara-shell/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json, application/octet-stream")
        .call()
        .map_err(|e| reach(&e))?;
    if let Some(seconds) = crate::marketplace::rate_limit_wait(&r) {
        return Err(Fetch::Failed(tf("GitHub's limit for anonymous requests was reached. Try again in {seconds} s.", &[("seconds", &seconds)])));
    }
    match r.status() {
        StatusCode::NOT_FOUND => return Err(Fetch::NotFound),
        s if !s.is_success() => return Err(reach(&s)),
        _ => {}
    }
    let mut reader = r.body_mut().with_config().limit(limit.saturating_add(1)).reader();
    let mut out = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = reader.read(&mut buf).map_err(|e| reach(&e))?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if out.len() as u64 > limit {
            return Err(Fetch::Failed(t("The download is larger than expected").to_string()));
        }
        on_bytes(out.len() as u64);
    }
    Ok(out)
}
