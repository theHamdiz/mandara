use std::collections::HashSet;

use mandara_i18n::{t, tf};

use super::{Entry, Feed, MAX_ENTRIES, MAX_FEED_BYTES, web_url};

/// Headlines remain small enough to shape without blocking a frame.
const MAX_TITLE_CHARS: usize = 512;
/// Browser links and identifiers are bounded independently of the feed.
const MAX_LINK_BYTES: usize = 8192;
const MAX_ID_BYTES: usize = 2048;

/// Collapse headline whitespace without inventing a title for an empty entry.
pub(super) fn title(text: &str) -> String {
    let mut chars = text.trim().chars();
    let mut title: String = chars.by_ref().take(MAX_TITLE_CHARS).collect();
    if chars.next().is_some() { title.push('…'); }
    title
}

/// Keep usable web links and bound saved text on character boundaries.
pub(super) fn bounded_entry(mut entry: Entry) -> Option<Entry> {
    if entry.link.len() > MAX_LINK_BYTES || !web_url(&entry.link) { return None; }
    entry.title = title(&entry.title);
    if entry.title.is_empty() { return None; }
    if entry.id.is_empty() || entry.id.len() > MAX_ID_BYTES { entry.id.clone_from(&entry.link); }
    Some(entry)
}

/// Parse a bounded subscription, resolving relative links against its source.
pub fn parse(url: &str, bytes: &[u8]) -> Result<Feed, String> {
    if bytes.len() > MAX_FEED_BYTES {
        return Err(t("This feed is too large to read").into());
    }
    let feed = feed_rs::parser::Builder::new().base_uri(Some(url)).build().parse(bytes).map_err(|error| tf("Could not read this feed: {error}", &[("error", &error)]).to_string())?;
    let mut seen = HashSet::new();
    let entries = feed.entries.into_iter().filter_map(|entry| {
        let link = entry.links.into_iter().find(|link| link.rel.as_deref().is_none_or(|rel| rel == "alternate") && web_url(&link.href))?.href;
        let entry = bounded_entry(Entry { id: entry.id, title: entry.title?.content, link, published: entry.published.or(entry.updated).map(|date| date.timestamp()) })?;
        seen.insert(entry.link.clone()).then_some(entry)
    }).take(MAX_ENTRIES).collect();
    Ok(Feed { url: url.into(), title: feed.title.map(|value| title(&value.content)).filter(|title| !title.is_empty()).unwrap_or_else(|| title(url)), entries, ..Feed::default() })
}
