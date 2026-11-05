//! News subscriptions. Requests run only when a visible reader asks for
//! them; validators avoid downloading an unchanged feed again.

mod parse;
mod cache;
pub use cache::{Cache, Snapshot};

use std::io::Read as _;
use std::time::Duration;

use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};

/// A feed is a small index, not an article archive. Bound both its response
/// and its retained entries before it reaches the interface or cache.
pub const MAX_FEED_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 100;
/// A stalled subscription must not hold the reader's refresh indefinitely.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub link: String,
    pub published: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Feed {
    pub url: String,
    pub title: String,
    pub entries: Vec<Entry>,
    pub etag: Option<String>,
    pub modified: Option<String>,
}

pub use parse::parse;

/// A reader keeps the last successful feed when its source is unavailable.
pub trait Source {
    /// `None` means the server confirmed that `previous` is still current.
    fn fetch(&self, url: &str, previous: Option<&Feed>) -> Result<Option<Feed>, String>;
}

pub struct HttpSource {
    agent: ureq::Agent,
}

impl Default for HttpSource {
    fn default() -> Self {
        Self { agent: ureq::Agent::config_builder().timeout_global(Some(REQUEST_TIMEOUT)).max_redirects(0).build().into() }
    }
}

/// Only web links are ever handed to the browser.
pub use crate::web_api::web_url;

impl Source for HttpSource {
    fn fetch(&self, url: &str, previous: Option<&Feed>) -> Result<Option<Feed>, String> {
        crate::offline::check()?;
        self.fetch_online(url, previous)
    }
}

impl HttpSource {
    fn fetch_online(&self, url: &str, previous: Option<&Feed>) -> Result<Option<Feed>, String> {
        if !web_url(url) {
            return Err(t("Use an http:// or https:// feed address without a password").into());
        }
        let previous = previous.filter(|feed| feed.url == url);
        let mut request = self.agent.get(url).header("User-Agent", concat!("mandara-shell/", env!("CARGO_PKG_VERSION"))).header("Accept", "application/atom+xml, application/rss+xml, application/feed+json, application/xml, text/xml");
        if let Some(value) = previous.and_then(|feed| feed.etag.as_deref()) {
            request = request.header("If-None-Match", value);
        }
        if let Some(value) = previous.and_then(|feed| feed.modified.as_deref()) {
            request = request.header("If-Modified-Since", value);
        }
        let failed = |error: &dyn std::fmt::Display| tf("Could not refresh this feed: {error}", &[("error", error)]).to_string();
        let mut response = request.call().map_err(|error| failed(&error))?;
        if response.status().is_redirection() && response.status() != ureq::http::StatusCode::NOT_MODIFIED {
            return Err(t("The server redirected this request; enter its final address in Studio").into());
        }
        if response.status() == ureq::http::StatusCode::NOT_MODIFIED {
            return previous.map(|_| None).ok_or_else(|| t("The feed server returned no content").to_string());
        }
        let header = |name| response.headers().get(name).and_then(|value| value.to_str().ok()).map(str::to_owned);
        let etag = header("etag");
        let modified = header("last-modified");
        let mut bytes = Vec::new();
        response.body_mut().as_reader().take((MAX_FEED_BYTES + 1) as u64).read_to_end(&mut bytes).map_err(|error| failed(&error))?;
        let mut feed = parse(url, &bytes)?;
        feed.etag = etag;
        feed.modified = modified;
        Ok(Some(feed))
    }
}

#[cfg(test)]
mod tests;
