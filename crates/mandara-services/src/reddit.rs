//! Reddit through its official Data API.
//!
//! Since mid-2026 Reddit answers anonymous `.json` requests with HTTP 403
//! and limits anonymous RSS to about one request a minute, so every request
//! goes through OAuth: the user's own "installed app" client id (no secret)
//! gets an application-only bearer token with the documented
//! `installed_client` grant, and listings, threads and replies come from
//! `oauth.reddit.com`. The token lives only in memory and expires within an
//! hour; nothing is written to disk. Requests are bounded, never follow
//! redirects, honour the `X-Ratelimit-*` headers and back off after a 429.

mod budget;
mod client;
mod parse;
pub mod rss;

pub use budget::{Budget, Limits};
pub use client::{Endpoints, Error, Http, Limited, Response, Session, Status, Transport, limited, user_agent};
pub use parse::{listing, more_children, thread};
pub use rss::Pacer;

use serde::Serialize;

/// Posts requested per page: Reddit's default, small enough to show quickly.
pub const PAGE_SIZE: usize = 25;
/// Comments requested with a thread, and the reply depth. Deeper replies
/// arrive as "continue this thread" stubs.
pub const THREAD_LIMIT: usize = 500;
pub const THREAD_DEPTH: u8 = 10;
/// The most comments one thread keeps, including replies loaded later.
pub const MAX_COMMENTS: usize = 2000;
/// Reddit accepts at most this many ids in one `morechildren` call.
pub const MORE_BATCH: usize = 100;
/// Gallery images kept per post.
pub const MAX_GALLERY: usize = 20;

/// A community name as Reddit validates it: letters, digits and
/// underscores. `all` and `popular` are Reddit's own front pages.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Subreddit(Box<str>);

/// Reddit's limit for a community name.
const MAX_NAME: usize = 21;

impl Subreddit {
    /// Accept `rust`, `r/rust`, `/r/rust/` or a reddit.com community address.
    pub fn parse(text: &str) -> Option<Self> {
        let mut name = text.trim();
        for prefix in ["https://", "http://"] {
            if let Some(rest) = name.strip_prefix(prefix) {
                name = rest.split_once('/').map(|(_, path)| path).unwrap_or("");
                break;
            }
        }
        let name = name.trim_start_matches('/');
        let name = name.strip_prefix("r/").or_else(|| name.strip_prefix("R/")).unwrap_or(name);
        let name = name.split(['/', '?', '#']).next().unwrap_or("");
        let valid = (2..=MAX_NAME).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        valid.then(|| Self(name.into()))
    }

    pub fn as_str(&self) -> &str { &self.0 }

    /// Reddit compares community names without case.
    pub fn same(&self, other: &str) -> bool { self.0.eq_ignore_ascii_case(other) }
}

impl std::fmt::Display for Subreddit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "r/{}", self.0) }
}

/// A post id in base 36, without the `t3_` kind prefix.
pub fn post_id(text: &str) -> Option<&str> {
    let id = text.strip_prefix("t3_").unwrap_or(text);
    ((1..=16).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric())).then_some(id)
}

/// Find the post in a reddit.com or redd.it address (`/comments/<id>/…`).
pub fn post_in_url(url: &str) -> Option<&str> {
    let rest = url.trim().strip_prefix("https://").or_else(|| url.trim().strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    let host = host.to_ascii_lowercase();
    if host == "redd.it" { return post_id(path.split(['/', '?', '#']).next()?); }
    if !(host == "reddit.com" || host.ends_with(".reddit.com")) { return None; }
    let mut parts = path.split('/');
    while let Some(part) = parts.next() {
        if part == "comments" { return post_id(parts.next()?); }
    }
    None
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Sort { #[default] Hot, New, Top, Rising }

impl Sort {
    pub const ALL: [Sort; 4] = [Sort::Hot, Sort::New, Sort::Top, Sort::Rising];
    pub fn as_str(self) -> &'static str {
        match self { Self::Hot => "hot", Self::New => "new", Self::Top => "top", Self::Rising => "rising" }
    }
    pub fn parse(text: &str) -> Option<Self> { Self::ALL.into_iter().find(|sort| sort.as_str() == text) }
    /// The search order closest to this listing order.
    fn search(self) -> &'static str {
        match self { Self::New => "new", Self::Top => "top", Self::Hot | Self::Rising => "relevance" }
    }
}

/// The time range of a top listing or a search.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Span { Hour, #[default] Day, Week, Month, Year, All }

impl Span {
    pub const ALL: [Span; 6] = [Span::Hour, Span::Day, Span::Week, Span::Month, Span::Year, Span::All];
    pub fn as_str(self) -> &'static str {
        match self { Self::Hour => "hour", Self::Day => "day", Self::Week => "week", Self::Month => "month", Self::Year => "year", Self::All => "all" }
    }
    pub fn parse(text: &str) -> Option<Self> { Self::ALL.into_iter().find(|span| span.as_str() == text) }
}

/// What one post list shows: a community, its order and an optional search.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Query {
    pub subreddit: Subreddit,
    pub sort: Sort,
    pub span: Span,
    pub search: Option<Box<str>>,
}

/// Searches longer than this are refused before they reach a request.
pub const MAX_SEARCH: usize = 256;

impl Query {
    /// The API path of one page. `count` is the number of posts already
    /// shown; Reddit uses it with `after` to number the next page.
    pub fn path(&self, after: Option<&str>, count: usize) -> String {
        let sub = self.subreddit.as_str();
        let mut path = match &self.search {
            Some(words) => format!("/r/{sub}/search?q={}&restrict_sr=1&type=link&sort={}&t={}", encode(words), self.sort.search(), self.span.as_str()),
            None if self.sort == Sort::Top => format!("/r/{sub}/top?t={}", self.span.as_str()),
            None => format!("/r/{sub}/{}?", self.sort.as_str()),
        };
        if !path.ends_with('?') { path.push('&'); }
        path.push_str(&format!("limit={PAGE_SIZE}&raw_json=1"));
        if let Some(after) = after.filter(|after| after.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')) {
            path.push_str(&format!("&after={after}&count={count}"));
        }
        path
    }
}

/// The comments of a post, best first, with enough depth for most threads.
pub fn thread_path(post: &str) -> String {
    format!("/comments/{post}?limit={THREAD_LIMIT}&depth={THREAD_DEPTH}&sort=confidence&raw_json=1")
}

/// A "continue this thread" stub: the comment's own subtree.
pub fn continue_path(post: &str, comment: &str) -> String {
    format!("/comments/{post}/_/{comment}?limit={THREAD_LIMIT}&depth={THREAD_DEPTH}&sort=confidence&raw_json=1")
}

/// Up to [`MORE_BATCH`] hidden replies of a post.
pub fn more_path(post: &str, children: &[Box<str>]) -> String {
    let ids: Vec<&str> = children.iter().take(MORE_BATCH).map(|id| &**id).filter(|id| id.bytes().all(|b| b.is_ascii_alphanumeric())).collect();
    format!("/api/morechildren?api_type=json&link_id=t3_{post}&children={}&limit_children=false&sort=confidence&raw_json=1", ids.join(","))
}

/// The post's page on reddit.com, for the browser and for copying.
pub fn web_link(permalink: &str) -> String {
    format!("https://www.reddit.com{permalink}")
}

/// Percent-encode a query value.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Image {
    pub url: Box<str>,
    pub width: u32,
    pub height: u32,
}

/// What opening a post shows besides its text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind { #[default] Text, Link, Image, Gallery, Video }

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Post {
    pub id: Box<str>,
    pub subreddit: Box<str>,
    pub title: Box<str>,
    pub author: Box<str>,
    pub flair: Option<Box<str>>,
    pub domain: Box<str>,
    /// The post's path on reddit.com (`/r/…/comments/…/`).
    pub permalink: Box<str>,
    /// The linked page of a link post (web addresses only).
    pub url: Option<Box<str>>,
    pub score: i64,
    pub comments: u64,
    pub created: i64,
    pub kind: Kind,
    pub nsfw: bool,
    pub spoiler: bool,
    pub stickied: bool,
    pub locked: bool,
    /// From the public feeds (limited mode): no score or comment count.
    pub limited: bool,
    /// The list thumbnail, the smallest preview at least as wide as asked.
    pub thumb: Option<Image>,
    /// The picture or gallery the reader shows, at the reader's width.
    pub media: Vec<Image>,
    /// Self text in Reddit's Markdown; empty in lists, which never show it.
    pub text: Box<str>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Listing {
    pub posts: Vec<Post>,
    /// The cursor of the next page; `None` at the end.
    pub after: Option<Box<str>>,
    /// Read from the public feeds (limited mode).
    pub limited: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Comment {
    pub id: Box<str>,
    pub author: Box<str>,
    /// Reddit's Markdown.
    pub body: Box<str>,
    pub score: i64,
    pub score_hidden: bool,
    pub created: i64,
    pub depth: u8,
    /// Written by the post's author.
    pub op: bool,
    pub stickied: bool,
    /// Posted as a moderator or administrator.
    pub distinguished: bool,
    pub edited: bool,
}

/// Replies Reddit left out: `count` of them, loaded with `children`. A stub
/// without children is "continue this thread" for its parent comment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct More {
    pub parent: Box<str>,
    pub children: Vec<Box<str>>,
    pub count: u32,
    pub depth: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Node { Comment(Comment), More(More) }

impl Node {
    pub fn depth(&self) -> u8 {
        match self { Self::Comment(comment) => comment.depth, Self::More(more) => more.depth }
    }
}

/// A post with its comments in reading order (depth first).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Thread {
    pub post: Post,
    pub nodes: Vec<Node>,
    /// The thread had more comments than [`MAX_COMMENTS`].
    pub truncated: bool,
    /// Read from the public feeds (limited mode).
    pub limited: bool,
}

/// Image widths a request picks previews for, in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Want {
    pub thumb: u32,
    pub media: u32,
}

#[cfg(test)]
mod tests;
