//! Limited mode: Reddit's public Atom feeds, for readers without an app.
//!
//! The feeds carry titles, authors, links, times, the community and the
//! post or comment body as HTML; they carry no scores, comment counts or
//! reply structure, and (since 2026) no thumbnails. A picture post links to
//! its image on `i.redd.it`, which the reader shows. Reddit allows about one
//! anonymous feed request a minute, so every request waits for the shared
//! [`Pacer`].

use std::time::{Duration, Instant};

use mandara_i18n::t;

use super::budget::Limits;
use super::parse::bounded;
use super::{Comment, Image, Kind, Listing, Node, PAGE_SIZE, Post, Query, Sort, Thread};

/// At most one feed request in this interval, whatever the headers say.
pub const RSS_INTERVAL: Duration = Duration::from_secs(60);
/// The longest a 429 or reset hint may hold the queue.
const MAX_PAUSE: Duration = Duration::from_secs(600);
/// A comments feed lists at most this many comments.
const COMMENTS_LIMIT: usize = 100;
const MAX_TITLE: usize = 300;
const MAX_SHORT: usize = 64;
const MAX_TEXT: usize = 40_000;
/// Hosts whose links are pictures the reader can show.
const PICTURE_HOSTS: [&str; 2] = ["i.redd.it", "preview.redd.it"];

/// One request a minute for the whole shell, longer when Reddit says so.
#[derive(Debug, Default)]
pub struct Pacer {
    next: Option<Instant>,
}

impl Pacer {
    pub const fn new() -> Self { Self { next: None } }

    /// How long until the next feed request may go out.
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        self.next.filter(|next| *next > now).map(|next| next - now)
    }

    /// A request went out now: the next one waits a full interval.
    pub fn sent(&mut self, now: Instant) {
        self.next = Some(self.next.map_or(now + RSS_INTERVAL, |next| next.max(now + RSS_INTERVAL)));
    }

    /// Reddit's reply: a used-up window or a 429 holds the queue longer.
    pub fn observe(&mut self, status: u16, limits: Limits, now: Instant) {
        let hint = |secs: Option<f32>| secs.map(|secs| Duration::from_secs_f32(secs).min(MAX_PAUSE));
        let mut until = None;
        if limits.remaining.is_some_and(|left| left < 1.0) && let Some(reset) = hint(limits.reset) { until = Some(now + reset); }
        if status == 429 {
            let pause = hint(limits.retry_after).into_iter().chain(hint(limits.reset)).max().unwrap_or(RSS_INTERVAL).max(RSS_INTERVAL);
            until = Some(now + pause);
        }
        if let Some(until) = until { self.next = Some(self.next.map_or(until, |next| next.max(until))); }
    }
}

/// The feed of one page of a query.
pub fn listing_path(query: &Query, after: Option<&str>) -> String {
    let sub = query.subreddit.as_str();
    let mut path = match &query.search {
        Some(words) => format!("/r/{sub}/search.rss?q={}&restrict_sr=on&sort={}&t={}&", super::encode(words), query.sort.search(), query.span.as_str()),
        None if query.sort == Sort::Top => format!("/r/{sub}/top/.rss?t={}&", query.span.as_str()),
        None => format!("/r/{sub}/{}/.rss?", query.sort.as_str()),
    };
    path.push_str(&format!("limit={PAGE_SIZE}"));
    if let Some(after) = after.filter(|after| after.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')) {
        path.push_str(&format!("&after={after}"));
    }
    path
}

/// The comments feed of a post: under its permalink when known.
pub fn comments_path(post: &str, permalink: Option<&str>) -> String {
    match permalink.filter(|link| link.starts_with("/r/") && link.ends_with('/') && !link.contains(['?', '#'])) {
        Some(link) => format!("{link}.rss?limit={COMMENTS_LIMIT}"),
        None => format!("/comments/{post}/.rss?limit={COMMENTS_LIMIT}"),
    }
}

fn invalid() -> String { t("Reddit sent a reply Mandara could not read").into() }

/// Decode the HTML entities Reddit uses.
fn entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else { out.push('&'); rest = &rest[1..]; continue };
        let name = &rest[1..end];
        let decoded = match name {
            "amp" => Some('&'), "lt" => Some('<'), "gt" => Some('>'), "quot" => Some('"'), "apos" => Some('\''), "nbsp" => Some(' '),
            _ => name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")).and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|dec| dec.parse().ok())).and_then(char::from_u32),
        };
        match decoded {
            Some(c) => { if c != '\u{200B}' { out.push(c); } rest = &rest[end + 1..]; }
            None => { out.push('&'); rest = &rest[1..]; }
        }
    }
    out.push_str(rest);
    out
}

/// The value of `name="…"` in a tag's attributes.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let at = tag.find(&format!("{name}=\""))? + name.len() + 2;
    let end = tag[at..].find('"')?;
    Some(&tag[at..at + end])
}

/// Reddit's rendered HTML back into Markdown the reader draws itself:
/// paragraphs, emphasis, code, lists, quotes, links and spoilers. Every
/// other tag is dropped; text is never interpreted as HTML.
pub fn markdown(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut links: Vec<Option<String>> = Vec::new();
    let mut pre = false;
    let mut code = false;
    let mut rest = html;
    while !rest.is_empty() {
        let Some(open) = rest.find('<') else { push_text(&mut out, rest, pre || code); break };
        push_text(&mut out, &rest[..open], pre || code);
        rest = &rest[open..];
        if let Some(comment) = rest.strip_prefix("<!--") {
            rest = comment.find("-->").map_or("", |end| &comment[end + 3..]);
            continue;
        }
        let Some(close) = rest.find('>') else { break };
        let tag = &rest[1..close];
        rest = &rest[close + 1..];
        let closing = tag.starts_with('/');
        let name = tag.trim_start_matches('/').split([' ', '/']).next().unwrap_or("").to_ascii_lowercase();
        match (name.as_str(), closing) {
            ("p" | "div" | "table" | "tr", true) => out.push_str("\n\n"),
            ("br", _) => out.push('\n'),
            ("hr", _) => out.push_str("\n\n---\n\n"),
            ("strong" | "b", _) => out.push_str("**"),
            ("em" | "i", _) => out.push('*'),
            ("del" | "s", _) => out.push_str("~~"),
            ("code", _) if !pre => { out.push('`'); code = !closing; }
            ("pre", false) => { out.push_str("\n\n```\n"); pre = true; }
            ("pre", true) => { out.push_str("\n```\n\n"); pre = false; }
            ("li", false) => out.push_str("\n- "),
            ("ul" | "ol", _) => out.push('\n'),
            ("blockquote", false) => out.push_str("\n\n> "),
            ("blockquote", true) => out.push_str("\n\n"),
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => out.push_str("\n\n### "),
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => out.push_str("\n\n"),
            ("span", false) if attribute(tag, "class").is_some_and(|class| class.contains("md-spoiler-text")) => { out.push_str(">!"); links.push(Some(String::new())); }
            ("a", false) => { out.push('['); links.push(attribute(tag, "href").map(entities)); }
            ("a", true) => match links.pop().flatten() {
                Some(href) if !href.is_empty() => { out.push_str("]("); out.push_str(&href.replace([' ', ')'], "")); out.push(')'); }
                _ => out.push(']'),
            },
            ("span", true) if links.last().is_some_and(|link| link.as_deref() == Some("")) => { links.pop(); out.push_str("!<"); }
            _ => {}
        }
    }
    out.trim().to_owned()
}

fn push_text(out: &mut String, text: &str, pre: bool) {
    let text = entities(text);
    if pre { out.push_str(&text); return; }
    for c in text.chars() {
        if matches!(c, '*' | '_' | '`' | '[' | ']' | '\\' | '#' | '~') { out.push('\\'); }
        out.push(if c == '\n' { ' ' } else { c });
    }
}

/// The self text of a post or a comment's body: the `md` block only.
fn body(html: &str) -> Option<&str> {
    let start = html.find("<div class=\"md\">")? + "<div class=\"md\">".len();
    let end = html.rfind("<!-- SC_ON -->").and_then(|end| html[..end].rfind("</div>")).filter(|end| *end >= start)?;
    Some(&html[start..end])
}

/// The `[link]` of a post entry: where the post points.
fn linked(html: &str) -> Option<String> {
    let end = html.find(">[link]</a>")?;
    let open = html[..end].rfind("<a ")?;
    attribute(&html[open..end], "href").map(entities).filter(|url| crate::web_api::web_url(url))
}

fn host(url: &str) -> &str {
    url.split_once("://").map_or("", |(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(""))
}

fn name(entry: &feed_rs::model::Entry) -> Box<str> {
    let name = entry.authors.first().and_then(|person| person.name.as_deref()).unwrap_or("");
    bounded(name.trim().trim_start_matches("/u/").trim_start_matches("u/"), MAX_SHORT)
}

fn time(entry: &feed_rs::model::Entry) -> i64 {
    entry.published.or(entry.updated).map_or(0, |at| at.timestamp())
}

fn post(entry: &feed_rs::model::Entry, keep_text: bool) -> Option<Post> {
    let id: Box<str> = super::post_id(entry.id.strip_prefix("t3_")?)?.into();
    let title = bounded(entry.title.as_ref()?.content.trim(), MAX_TITLE);
    let html = entry.content.as_ref().and_then(|content| content.body.as_deref()).unwrap_or("");
    let permalink: Box<str> = entry.links.first().map(|link| link.href.as_str()).and_then(|href| href.strip_prefix("https://www.reddit.com"))
        .filter(|path| path.starts_with("/r/")).unwrap_or("").into();
    let url = linked(html);
    let host = url.as_deref().map(host).unwrap_or("").to_ascii_lowercase();
    let picture = url.as_deref().filter(|_| PICTURE_HOSTS.contains(&host.as_str())).map(|url| Image { url: url.into(), width: 0, height: 0 });
    let thumb = entry.media.iter().flat_map(|media| &media.thumbnails).find_map(|thumb| {
        PICTURE_HOSTS.iter().chain(["a.thumbs.redditmedia.com", "b.thumbs.redditmedia.com"].iter()).any(|known| host_of(&thumb.image.uri) == *known)
            .then(|| Image { url: thumb.image.uri.as_str().into(), width: thumb.image.width.unwrap_or(0), height: thumb.image.height.unwrap_or(0) })
    }).or_else(|| picture.clone());
    let text = body(html).map(markdown).unwrap_or_default();
    // A gallery links to its page on reddit.com; the browser shows it.
    let gallery = url.as_deref().is_some_and(|url| url.contains("reddit.com/gallery/"));
    let self_post = !gallery && url.as_deref().is_none_or(|url| url.contains(&*id) && host.ends_with("reddit.com"));
    let kind = if picture.is_some() { Kind::Image }
        else if host == "v.redd.it" { Kind::Video }
        else if self_post { Kind::Text }
        else { Kind::Link };
    Some(Post {
        id,
        subreddit: bounded(entry.categories.first().map_or("", |category| category.term.as_str()), MAX_SHORT),
        title,
        author: name(entry),
        domain: if self_post { Box::default() } else { host.into() },
        permalink,
        url: url.filter(|_| !self_post).map(Into::into),
        created: time(entry),
        kind,
        thumb,
        media: picture.into_iter().collect(),
        text: if keep_text { bounded(&text, MAX_TEXT) } else { Box::default() },
        limited: true,
        ..Post::default()
    })
}

fn host_of(url: &str) -> String { host(url).to_ascii_lowercase() }

fn feed(bytes: &[u8]) -> Result<feed_rs::model::Feed, String> {
    feed_rs::parser::parse(bytes).map_err(|_| invalid())
}

/// A page of posts from a community feed. The next page starts after the
/// last post of a full page.
pub fn listing(bytes: &[u8]) -> Result<Listing, String> {
    let feed = feed(bytes)?;
    let mut posts: Vec<Post> = Vec::with_capacity(feed.entries.len());
    for entry in &feed.entries {
        if let Some(post) = post(entry, false) && !posts.iter().any(|seen| seen.id == post.id) { posts.push(post); }
    }
    let after = (posts.len() >= PAGE_SIZE).then(|| posts.last().map(|post| format!("t3_{}", post.id).into_boxed_str())).flatten();
    Ok(Listing { posts, after, limited: true })
}

/// A post and its comments from a comments feed. The feed is flat, so
/// every comment sits at the first level.
pub fn thread(bytes: &[u8]) -> Result<Thread, String> {
    let feed = feed(bytes)?;
    let post = feed.entries.iter().find_map(|entry| post(entry, true)).ok_or_else(invalid)?;
    let nodes = feed.entries.iter().filter_map(|entry| {
        let id = entry.id.strip_prefix("t1_")?;
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric()) { return None; }
        let html = entry.content.as_ref().and_then(|content| content.body.as_deref()).unwrap_or("");
        let text = markdown(body(html).unwrap_or(html));
        Some(Node::Comment(Comment {
            id: id.into(), author: name(entry), body: bounded(&text, MAX_TEXT), score: 0, score_hidden: true, created: time(entry),
            depth: 0, op: name(entry) == post.author, stickied: false, distinguished: false, edited: false,
        }))
    }).take(COMMENTS_LIMIT).collect();
    Ok(Thread { post, nodes, truncated: false, limited: true })
}

