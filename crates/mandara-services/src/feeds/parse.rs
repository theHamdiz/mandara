//! RSS 0.9x / 1.0 (RDF) / 2.0, Atom and JSON Feed into one model, with
//! the repairs real feeds need: stray bytes before the XML, a missing or
//! wrong encoding, HTML entities XML doesn't define, bare ampersands,
//! markup in titles, relative links, missing or impossible dates.

use std::borrow::Cow;
use std::collections::HashSet;

use mandara_i18n::{t, tf};

use super::html::{self, Article};
use super::{Entry, Feed, MAX_ENTRIES, MAX_FEED_BYTES, web_url};

/// Headlines remain small enough to shape without blocking a frame.
const MAX_TITLE_CHARS: usize = 512;
/// The excerpt under a headline.
pub const MAX_SUMMARY_CHARS: usize = 280;
/// Browser links and identifiers are bounded independently of the feed.
const MAX_LINK_BYTES: usize = 8192;
const MAX_ID_BYTES: usize = 2048;
const MAX_AUTHOR_CHARS: usize = 80;
/// Junk a server may print before the document (PHP warnings, blank lines).
const MAX_LEADING_JUNK: usize = 4096;
/// Dates before this are a broken feed, not history (2000-01-01).
const EARLIEST_DATE: i64 = 946_684_800;
/// A date this far ahead is a wrong time zone, not a scheduled post.
const FUTURE_SLACK_SECS: i64 = 2 * 86_400;

/// Collapse headline whitespace without inventing a title for an empty entry.
pub(super) fn title(text: &str) -> String {
    let mut chars = text.trim().chars();
    let mut title: String = chars.by_ref().take(MAX_TITLE_CHARS).collect();
    if chars.next().is_some() { title.push('…'); }
    title
}

/// A title that may carry markup or (double-)escaped entities.
fn clean_title(text: &str) -> String {
    if !text.contains(['<', '&']) { return title(&collapse(text)); }
    let once = html::plain_text(text, MAX_TITLE_CHARS);
    // `&amp;amp;` and `&lt;b&gt;` survive one pass in double-escaped feeds.
    if once.contains(['<', '&']) { html::plain_text(&once, MAX_TITLE_CHARS) } else { once }
}

fn collapse(text: &str) -> Cow<'_, str> {
    let text = text.trim();
    if text.contains(|c: char| c.is_whitespace() && c != ' ') || text.contains("  ") { Cow::Owned(text.split_whitespace().collect::<Vec<_>>().join(" ")) } else { Cow::Borrowed(text) }
}

/// Keep usable web links and bound saved text on character boundaries.
pub(super) fn bounded_entry(mut entry: Entry) -> Option<Entry> {
    if entry.link.len() > MAX_LINK_BYTES || !web_url(&entry.link) { return None; }
    entry.title = title(&entry.title);
    if entry.title.is_empty() { return None; }
    if entry.id.is_empty() || entry.id.len() > MAX_ID_BYTES { entry.id.clone_from(&entry.link); }
    if entry.summary.chars().count() > MAX_SUMMARY_CHARS + 1 { entry.summary = html::truncate_chars(std::mem::take(&mut entry.summary), MAX_SUMMARY_CHARS); }
    entry.image = entry.image.filter(|image| image.len() <= MAX_LINK_BYTES && web_url(image));
    if let Some(author) = &mut entry.author && author.chars().count() > MAX_AUTHOR_CHARS { *author = html::truncate_chars(std::mem::take(author), MAX_AUTHOR_CHARS); }
    Some(entry)
}

/// Bytes ready for the XML parser: no BOM or junk before the document,
/// and UTF-8 when the declared (or served) encoding is missing or wrong.
pub(super) fn normalize<'a>(bytes: &'a [u8], charset: Option<&str>) -> Cow<'a, [u8]> {
    let mut start = if bytes.starts_with(b"\xEF\xBB\xBF") { 3 } else { 0 };
    let body = &bytes[start..];
    let first = body.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(body.len());
    if body.get(first).is_some_and(|b| *b != b'<' && *b != b'{') && let Some(at) = body[..body.len().min(MAX_LEADING_JUNK)].iter().position(|b| *b == b'<') {
        start += at;
    } else {
        start += first;
    }
    let body = &bytes[start..];
    if body.first() == Some(&b'{') { return Cow::Borrowed(body); }
    let declared = declared_encoding(body);
    let utf8 = std::str::from_utf8(body).is_ok();
    let label = match (&declared, charset) {
        // A declaration the bytes contradict (a "UTF-8" feed in Latin-1).
        (Some(label), _) if label.eq_ignore_ascii_case("utf-8") && !utf8 => Some("windows-1252"),
        (Some(_), _) => None,
        (None, Some(served)) if !served.eq_ignore_ascii_case("utf-8") => Some(served),
        (None, _) if !utf8 => Some("windows-1252"),
        (None, _) => None,
    };
    let Some(encoding) = label.and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes())) else { return Cow::Borrowed(body) };
    let (text, _, _) = encoding.decode(body);
    let mut text = text.into_owned();
    if declared.is_some() && let Some(end) = text.find("?>") && text.starts_with("<?xml") {
        // The text is UTF-8 now; say so.
        text.replace_range(..end + 2, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    }
    Cow::Owned(text.into_bytes())
}

/// The `encoding="…"` of an XML declaration.
fn declared_encoding(body: &[u8]) -> Option<String> {
    let head = body.strip_prefix(b"<?xml")?;
    let end = head.iter().position(|b| *b == b'>')?;
    let decl = std::str::from_utf8(&head[..end]).ok()?;
    let at = decl.find("encoding")?;
    let value = decl[at + "encoding".len()..].trim_start().strip_prefix('=')?.trim_start();
    let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    value[1..].split(quote).next().map(str::to_owned)
}

/// XML defines five named entities; feeds use HTML's. Rewrite the rest
/// as numbers, and lone ampersands as `&amp;`, for a second attempt.
fn repair_entities(bytes: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut out = String::with_capacity(text.len() + 64);
    let mut rest = text;
    let mut changed = false;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let name_len = rest[1..].bytes().take_while(|b| b.is_ascii_alphanumeric() || *b == b'#').count();
        let terminated = rest.as_bytes().get(1 + name_len) == Some(&b';');
        let name = &rest[1..1 + name_len];
        if terminated && (matches!(name, "amp" | "lt" | "gt" | "quot" | "apos") || name.starts_with('#')) {
            out.push_str(&rest[..name_len + 2]);
            rest = &rest[name_len + 2..];
        } else if let Some(c) = terminated.then(|| single(&html::decode(&rest[..name_len + 2]))).flatten() {
            out.push_str(&format!("&#{};", c as u32));
            rest = &rest[name_len + 2..];
            changed = true;
        } else {
            out.push_str("&amp;");
            rest = &rest[1..];
            changed = true;
        }
    }
    out.push_str(rest);
    changed.then(|| out.into_bytes())
}

/// The one character a decoded reference became, if it was known.
fn single(decoded: &str) -> Option<char> {
    let mut chars = decoded.chars();
    chars.next().filter(|_| chars.next().is_none())
}

/// What a parse produces: the feed's headlines and, aligned with them,
/// each article's cleaned-up content.
pub struct Parsed {
    pub feed: Feed,
    pub articles: Vec<Article>,
}

/// Parse a bounded subscription, resolving relative links against its source.
pub fn parse(url: &str, bytes: &[u8]) -> Result<Feed, String> { parse_full(url, bytes, None).map(|parsed| parsed.feed) }

/// [`parse`], keeping article content and honouring the served charset.
pub fn parse_full(url: &str, bytes: &[u8], charset: Option<&str>) -> Result<Parsed, String> {
    if bytes.len() > MAX_FEED_BYTES { return Err(t("This feed is too large to read").into()); }
    let bytes = normalize(bytes, charset);
    let (feed, bytes) = match read(url, &bytes) {
        Ok(feed) => (feed, bytes),
        Err(first) => match repair_entities(&bytes) {
            Some(repaired) => (read(url, &repaired).map_err(|_| first)?, Cow::Owned(repaired)),
            None => return Err(first),
        },
    };
    let images = declared_images(&bytes, feed.entries.len());
    drop(bytes);
    Ok(convert(url, feed, images, chrono::Utc::now().timestamp()))
}

/// One parse with a fresh parser (its state is not reusable after an
/// error). A panic inside the parser on hostile input is an error too.
fn read(url: &str, bytes: &[u8]) -> Result<feed_rs::model::Feed, String> {
    let unreadable = |error: &dyn std::fmt::Display| tf("Could not read this feed: {error}", &[("error", error)]).to_string();
    match std::panic::catch_unwind(|| feed_rs::parser::Builder::new().base_uri(Some(url)).build().parse(bytes)) {
        Ok(Ok(feed)) => Ok(feed),
        Ok(Err(error)) => Err(unreadable(&error)),
        Err(_) => Err(unreadable(&t("the feed's structure is broken"))),
    }
}

/// Pictures entries declare that the parser drops: a Media RSS thumbnail
/// outside a `media:content` (BBC, Guardian…), and JSON Feed's `image`.
/// Aligned with the parser's entries; `None` when the counts disagree.
fn declared_images(bytes: &[u8], entries: usize) -> Option<Vec<Option<String>>> {
    if bytes.first() == Some(&b'{') {
        #[derive(serde::Deserialize)]
        struct Item { image: Option<String>, banner_image: Option<String> }
        #[derive(serde::Deserialize)]
        struct Items { #[serde(default)] items: Vec<Item> }
        let items: Items = serde_json::from_slice(bytes).ok()?;
        let images: Vec<Option<String>> = items.items.into_iter().map(|item| item.image.or(item.banner_image)).collect();
        return (images.len() == entries).then_some(images);
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let mut images = Vec::new();
    let mut inside = false;
    for token in html::Tokens::new(text) {
        match &token {
            html::Token::Start { name, closed: false, .. } if name == "item" || name == "entry" => { inside = true; images.push(None); }
            html::Token::End(name) if name == "item" || name == "entry" => inside = false,
            html::Token::Start { name, .. } if inside && (name.ends_with(":thumbnail") || name == "thumbnail") => {
                if let Some(slot) = images.last_mut() && slot.is_none() { *slot = token.attr("url").map(str::to_owned); }
            }
            _ => {}
        }
    }
    (images.len() == entries).then_some(images)
}

/// Whether bytes look like a web page rather than a feed.
pub fn looks_like_html(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(2048)];
    let head = String::from_utf8_lossy(head).to_ascii_lowercase();
    head.contains("<!doctype html") || head.contains("<html")
}

/// A sane timestamp, or none (the reader then uses when it first saw it).
fn date(value: Option<chrono::DateTime<chrono::Utc>>, now: i64) -> Option<i64> {
    let at = value?.timestamp();
    if at < EARLIEST_DATE || at > now + FUTURE_SLACK_SECS { return None; }
    Some(at.min(now))
}

fn convert(url: &str, feed: feed_rs::model::Feed, declared: Option<Vec<Option<String>>>, now: i64) -> Parsed {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    let mut articles = Vec::new();
    let base = url::Url::parse(url).ok();
    let mut declared = declared.map(Vec::into_iter);
    for entry in feed.entries {
        let declared = declared.as_mut().and_then(Iterator::next).flatten().and_then(|src| { let src = html::resolve(base.as_ref(), &src)?; (!html::is_tracker(&src)).then_some(src) });
        if entries.len() >= MAX_ENTRIES { break; }
        let Some(link) = entry.links.iter().find(|link| link.rel.as_deref().is_none_or(|rel| rel == "alternate") && web_url(&link.href))
            .or_else(|| entry.links.iter().find(|link| web_url(&link.href) && link.rel.as_deref() != Some("enclosure")))
            .map(|link| link.href.clone())
            .or_else(|| web_url(&entry.id).then(|| entry.id.clone())) else { continue };
        let article_base = url::Url::parse(&link).ok().or_else(|| base.clone());
        let body = entry.content.as_ref().and_then(|content| content.body.as_deref()).filter(|body| !body.trim().is_empty());
        let summary_html = entry.summary.as_ref().map(|summary| summary.content.as_str()).filter(|summary| !summary.trim().is_empty());
        let rich = body.or(summary_html).unwrap_or_default();
        let is_markup = |text: &str| text.contains('<') || text.contains('&');
        let article = if is_markup(rich) { html::sanitize(rich, Some(&link)) } else { html::paragraphs(rich) };
        let summary = summary_html.or(body).map(|text| if is_markup(text) { html::plain_text(text, MAX_SUMMARY_CHARS) } else { html::truncate_chars(collapse(text).into_owned(), MAX_SUMMARY_CHARS) }).unwrap_or_default();
        let title_text = entry.title.as_ref().map(|title| clean_title(&title.content)).filter(|title| !title.is_empty())
            .unwrap_or_else(|| html::truncate_chars(summary.clone(), 120));
        let summary = if summary == title_text { String::new() } else { summary };
        let image = image(&entry.media, &entry.links).or(declared).or_else(|| html::first_image(rich, article_base.as_ref()));
        let author = entry.authors.iter().chain(&entry.contributors).find_map(|person| person.name.as_deref().map(str::trim).filter(|name| !name.is_empty()))
            .map(|name| clean_title(author_name(name)));
        let published = date(entry.published, now).or_else(|| date(entry.updated, now));
        let Some(entry) = bounded_entry(Entry { id: entry.id, title: title_text, link, published, summary, image, author, seen: now }) else { continue };
        if seen.insert(entry.link.clone()) {
            entries.push(entry);
            articles.push(article);
        }
    }
    let site = feed.links.iter().find(|link| link.rel.as_deref().is_none_or(|rel| rel == "alternate") && link.media_type.as_deref().is_none_or(|kind| kind.contains("html")) && web_url(&link.href))
        .map(|link| link.href.clone());
    let icon = feed.icon.iter().chain(&feed.logo).map(|image| image.uri.as_str()).find(|uri| web_url(uri) && uri.len() <= MAX_LINK_BYTES).map(str::to_owned);
    let feed_title = feed.title.map(|value| clean_title(&value.content)).filter(|title| !title.is_empty()).unwrap_or_else(|| title(url));
    Parsed { feed: Feed { url: url.into(), title: feed_title, entries, site, icon, ..Feed::default() }, articles }
}

/// `jane@example.org (Jane Doe)` → `Jane Doe`.
fn author_name(name: &str) -> &str {
    if let Some(open) = name.find('(') && let Some(close) = name.rfind(')') && close > open + 1 && name[..open].contains('@') {
        return name[open + 1..close].trim();
    }
    // Some parsers already split the address off and leave `(Jane Doe)`.
    name.strip_prefix('(').and_then(|inner| inner.strip_suffix(')')).map_or(name, str::trim)
}

/// The picture an entry declares: a Media RSS thumbnail, an image
/// enclosure or media content.
fn image(media: &[feed_rs::model::MediaObject], links: &[feed_rs::model::Link]) -> Option<String> {
    let thumbnail = media.iter().flat_map(|object| &object.thumbnails).map(|thumb| thumb.image.uri.as_str()).find(|uri| web_url(uri) && !html::is_tracker(uri));
    let content = || media.iter().flat_map(|object| &object.content).filter(|content| content.content_type.as_ref().is_none_or(|kind| kind.as_str().starts_with("image")))
        .filter_map(|content| content.url.as_ref()).map(url::Url::as_str).find(|uri| web_url(uri) && !html::is_tracker(uri) && !uri.ends_with(".mp3") && !uri.ends_with(".mp4"));
    let enclosure = || links.iter().filter(|link| link.rel.as_deref() == Some("enclosure") && link.media_type.as_deref().is_some_and(|kind| kind.starts_with("image/"))).map(|link| link.href.as_str()).find(|uri| web_url(uri));
    thumbnail.or_else(content).or_else(enclosure).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leading_junk_bom_and_wrong_encodings_are_repaired() {
        let feed = b"\xEF\xBB\xBF\n\nWarning: something<?xml version=\"1.0\"?><rss/>";
        assert!(normalize(feed, None).starts_with(b"<?xml"));
        let latin = b"<?xml version=\"1.0\" encoding=\"utf-8\"?><rss><channel><title>Caf\xe9</title></channel></rss>";
        let fixed = normalize(latin, None);
        assert!(std::str::from_utf8(&fixed).unwrap().contains("Café"));
        let served = b"<rss><channel><title>Caf\xe9</title></channel></rss>";
        assert!(std::str::from_utf8(&normalize(served, Some("ISO-8859-1"))).unwrap().contains("Café"));
        assert!(matches!(normalize(b"<rss/>", Some("utf-8")), Cow::Borrowed(_)));
        assert!(normalize(b"  {\"version\":1}", None).starts_with(b"{"));
    }

    #[test]
    fn html_entities_and_bare_ampersands_get_a_second_chance() {
        let repaired = String::from_utf8(repair_entities(b"<t>AT&T &nbsp; &amp; &#8212; &hellip;</t>").unwrap()).unwrap();
        assert_eq!(repaired, "<t>AT&amp;T &#160; &amp; &#8212; &#8230;</t>");
        assert!(repair_entities(b"<t>fine &amp; dandy</t>").is_none());
    }

    #[test]
    fn titles_lose_markup_and_double_escaping() {
        assert_eq!(clean_title("  <b>Bold</b> &amp;amp; news  "), "Bold & news");
        assert_eq!(clean_title("Plain\n  title"), "Plain title");
        assert_eq!(author_name("jane@example.org (Jane Doe)"), "Jane Doe");
        assert_eq!(author_name("Jane (the editor)"), "Jane (the editor)");
    }

    #[test]
    fn impossible_dates_fall_back_to_first_seen() {
        let now = 1_800_000_000;
        let at = |s: i64| chrono::DateTime::from_timestamp(s, 0);
        assert_eq!(date(at(now - 60), now), Some(now - 60));
        assert_eq!(date(at(now + 3600), now), Some(now), "a slightly wrong time zone is clamped to now");
        assert_eq!(date(at(now + 30 * 86_400), now), None);
        assert_eq!(date(at(0), now), None);
    }
}
