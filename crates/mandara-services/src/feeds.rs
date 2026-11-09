//! News subscriptions: RSS 0.9x/1.0/2.0, Atom and JSON Feed through one
//! tolerant parser, conditional and gzipped requests with validated
//! redirects, per-feed backoff, site discovery, OPML, and a bounded local
//! history of headlines, read and starred flags and cleaned-up articles.
//! Requests run only when a visible reader, an explicit command or the
//! opt-in background refresh asks for them, always off the UI thread.

mod cache;
pub mod catalog;
mod discover;
pub mod html;
mod http;
pub mod opml;
mod parse;

pub use cache::{Cache, Limits, Snapshot, Counts};
pub use discover::Candidate;
pub use html::{Article, Block, Mark, Rich};
pub use http::Failure;
pub use parse::{Parsed, parse, parse_full};

use mandara_i18n::t;
use serde::{Deserialize, Serialize};

/// A feed is a small index, not an article archive. Bound both its response
/// and its retained entries before it reaches the interface or cache.
pub const MAX_FEED_BYTES: usize = 4 * 1024 * 1024;
/// Entries read from one download (and kept per feed) at most.
pub const MAX_ENTRIES: usize = 200;
/// A thumbnail or favicon download is never larger than this.
pub const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
const ACCEPT_FEEDS: &str = "application/atom+xml, application/rss+xml, application/feed+json, application/rdf+xml;q=0.9, application/xml;q=0.8, text/xml;q=0.8, */*;q=0.5";
const ACCEPT_PAGES: &str = "text/html, application/xhtml+xml, application/atom+xml, application/rss+xml, application/feed+json, application/xml;q=0.9, */*;q=0.5";
const ACCEPT_IMAGES: &str = "image/webp, image/png, image/jpeg, image/*;q=0.8";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub link: String,
    pub published: Option<i64>,
    /// A plain-text excerpt (a few lines at most).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// The picture the feed declares for it (a web address).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// When this reader first saw it: sorts undated entries and bounds history.
    #[serde(default)]
    pub seen: i64,
}

impl Entry {
    /// Its time on the timeline: the publication date, or when it appeared.
    pub fn when(&self) -> i64 { self.published.unwrap_or(self.seen) }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Feed {
    /// The subscription's address (the cache key).
    pub url: String,
    pub title: String,
    pub entries: Vec<Entry>,
    pub etag: Option<String>,
    pub modified: Option<String>,
    /// The site's home page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    /// The feed's own icon or logo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Where the feed really lives when that differs from `url`: found on
    /// the site's page, or the target of a permanent redirect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved: Option<String>,
    /// Cleaned-up article content, aligned with `entries`, on its way to
    /// the article store (never kept in a snapshot).
    #[serde(skip)]
    pub articles: Vec<Article>,
}

/// One subscription in the configuration (`[[plugins.settings.feeds.feeds]]`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    pub url: String,
    /// Your name for it; empty uses the feed's own title.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub folder: String,
    /// A `#rrggbb` accent; empty picks one from the name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub color: String,
}

/// A reader keeps the last successful feed when its source is unavailable.
pub trait Source {
    /// `None` means the server confirmed that `previous` is still current.
    /// Respect offline mode before issuing any feed request.
    fn fetch(&self, url: &str, previous: Option<&Feed>) -> Result<Option<Feed>, Failure>;
}

pub struct HttpSource {
    agent: ureq::Agent,
}

impl Default for HttpSource {
    fn default() -> Self { Self { agent: http::agent() } }
}

/// Only web links are ever handed to the browser.
pub use crate::web_api::web_url;

impl Source for HttpSource {
    /// Reject offline requests before entering the HTTP refresh path.
    fn fetch(&self, url: &str, previous: Option<&Feed>) -> Result<Option<Feed>, Failure> {
        crate::offline::check()?;
        self.fetch_online(url, previous)
    }
}

impl HttpSource {
    /// Read one bounded feed. A web page instead of a feed is searched for
    /// the feed it announces, which is then read in its place.
    fn fetch_online(&self, url: &str, previous: Option<&Feed>) -> Result<Option<Feed>, Failure> {
        if !web_url(url) { return Err(t("Use an http:// or https:// feed address without a password").into()); }
        let previous = previous.filter(|feed| feed.url == url);
        let address = previous.and_then(|feed| feed.resolved.as_deref()).unwrap_or(url);
        let validators = previous.map_or((None, None), |feed| (feed.etag.as_deref(), feed.modified.as_deref()));
        let body = match http::get(&self.agent, address, ACCEPT_FEEDS, validators, MAX_FEED_BYTES)? {
            http::Fetched::NotModified => return previous.map(|_| None).ok_or_else(|| t("The feed server returned no content").into()),
            http::Fetched::Body(body) => body,
        };
        let resolved = (body.moved || address != url).then(|| if body.moved { body.final_url.clone() } else { address.to_owned() });
        // Relative links resolve against the address that served the feed;
        // the subscription keeps its own key.
        match parse_full(&body.final_url, &body.bytes, body.charset.as_deref()) {
            Ok(mut parsed) => {
                parsed.feed.url = url.to_owned();
                parsed.feed.etag = body.etag;
                parsed.feed.modified = body.modified;
                parsed.feed.resolved = resolved;
                parsed.feed.articles = parsed.articles;
                Ok(Some(parsed.feed))
            }
            Err(error) if parse::looks_like_html(&body.bytes) => {
                let page = String::from_utf8_lossy(&body.bytes);
                let Some(found) = discover::links(&body.final_url, &page).into_iter().next() else {
                    return Err(t("This address is a web page without a feed; find its RSS or Atom link").into());
                };
                drop(page);
                let http::Fetched::Body(feed_body) = http::get(&self.agent, &found.url, ACCEPT_FEEDS, (None, None), MAX_FEED_BYTES)? else {
                    return Err(t("The feed server returned no content").into());
                };
                let mut parsed = parse_full(&feed_body.final_url, &feed_body.bytes, feed_body.charset.as_deref()).map_err(|_| Failure::from(error))?;
                parsed.feed.url = url.to_owned();
                parsed.feed.etag = feed_body.etag;
                parsed.feed.modified = feed_body.modified;
                parsed.feed.resolved = Some(found.url);
                parsed.feed.articles = parsed.articles;
                Ok(Some(parsed.feed))
            }
            Err(error) => Err(error.into()),
        }
    }
}

/// A typed address as a web URL (`example.org/feed` → `https://…`).
pub fn normalize_address(input: &str) -> Result<String, String> {
    let input = input.trim();
    let url = if input.contains("://") { input.to_owned() } else { format!("https://{input}") };
    if web_url(&url) && (url.contains('.') || url.contains("localhost")) { Ok(url) } else { Err(t("Use an http:// or https:// feed address without a password").into()) }
}

/// The feeds at an address: the address itself when it is a feed, the
/// feeds its page announces, or a feed at a conventional path.
pub fn discover(input: &str) -> Result<Vec<Candidate>, String> {
    crate::offline::check()?;
    let url = normalize_address(input)?;
    let agent = http::agent();
    let failure = |error: Failure| error.message;
    let body = match http::get(&agent, &url, ACCEPT_PAGES, (None, None), MAX_FEED_BYTES).map_err(failure)? {
        http::Fetched::Body(body) => body,
        http::Fetched::NotModified => return Err(t("The feed server returned no content").into()),
    };
    let address = if body.moved { body.final_url.clone() } else { url.clone() };
    if let Ok(parsed) = parse_full(&address, &body.bytes, body.charset.as_deref()) {
        return Ok(vec![Candidate { url: address, title: parsed.feed.title }]);
    }
    if parse::looks_like_html(&body.bytes) {
        let found = discover::links(&body.final_url, &String::from_utf8_lossy(&body.bytes));
        if !found.is_empty() { return Ok(found); }
    }
    drop(body);
    for candidate in discover::common(&url) {
        if let Ok(http::Fetched::Body(body)) = http::get(&agent, &candidate, ACCEPT_FEEDS, (None, None), MAX_FEED_BYTES)
            && let Ok(parsed) = parse_full(&candidate, &body.bytes, body.charset.as_deref())
            && !parsed.feed.entries.is_empty()
        {
            return Ok(vec![Candidate { url: candidate, title: parsed.feed.title }]);
        }
    }
    Err(t("No feed found at this address").into())
}

/// Download a picture (a thumbnail, an icon or an article image).
pub fn download_image(url: &str) -> Result<Vec<u8>, String> {
    match http::get(&http::agent(), url, ACCEPT_IMAGES, (None, None), MAX_IMAGE_BYTES).map_err(|error| error.message)? {
        http::Fetched::Body(body) => Ok(body.bytes),
        http::Fetched::NotModified => Err(t("The feed server returned no content").into()),
    }
}

#[cfg(test)]
mod tests;
